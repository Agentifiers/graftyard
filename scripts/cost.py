#!/usr/bin/env python3
"""Monthly Cloudflare list cost of Graftyard on Cloudflare (three-site quorum chains on Durable Objects).

Usage:
	python3 scripts/cost.py                       # summary for every tier and variant, shard counts, egress split
	python3 scripts/cost.py --detail large design

Workload comes from scripts/cost_model.py (TIERS, ACTIVITY). Prices are the cf.* entries in research/prices.json.
Every per-operation constant below (requests per git operation, CPU ms, rows per entry, container sizes, hit rates,
working sets, leader split, site-loss rate) is a [guess] until Milestone 0 measures it. Excluded, as in
scripts/cost_model.py: Actions runner compute, the vendor fee, and tax. Also excluded: any Enterprise contract, which
has no list price. Managed cells and any install that accepts pushes over 200 MB need one on top of these figures.
The plan line uses Pro or Business list.

Variants:
	design     the default: one install account with three site Worker scripts pinned to enam, weur, and apac; every
	           chain is a 3-member group whose members call each other through Durable Object bindings; packs in two
	           buckets in two sites before the ack, a third copy async; git cache shards warm on two sites; three
	           spare site scripts; a vault account and a runner account.
	separate   the account-separated option: three site accounts plus an edge account; every member RPC and every
	           edge call into a group passes one site Worker over HTTPS. Same quorum, plus the vault and runner
	           accounts.
	solo       the `Solo` placement for evaluation installs: one member per chain, one account, one pack bucket,
	           git cache shards on one site, a runner account, no vault and no spare sites.
	artifacts  the rejected alternative: git in Cloudflare Artifacts, one Durable Object per chain for metadata.

Learners for hot groups (more than 500 reads a second) are priced at zero: no group in TIERS reaches that rate [guess].
"""

import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from cost_model import ACTIVITY, DAYS, HOURS, TIERS, price  # noqa: E402

SECONDS = HOURS * 3600
DO_GB = 0.125                      # duration bills 128 MB per object regardless of use (DO pricing page)

# ---- per-operation guesses ----
GIT_REQS = 3                       # info/refs plus two POSTs per protocol v2 fetch, or info/refs plus receive-pack
CPU_MS = dict(api=4, git=8, push=40, ring=2, queue=3, front=1)
ENTRY_PER_PUSH = 1.3               # pushes plus PrSync bot entries
ENTRY_PER_JOB_REPO = 0.2           # RequiredCheck entries on repository chains
ENTRY_PER_JOB_ACTIONS = 7          # job transitions and lease rewrites on the org's Actions chain
FOLD_ROWS = 6                      # fold rows written per entry per member, plus 1 log row
LEASE_MISS = 0.25                  # share of reads that find the leader lease expired and pay one read-index RPC
WALL_S = dict(read=0.005, commit=0.12, append=0.005, lease=0.07)
CI_HIT = 0.7                       # clone-cache hit rate: about 3.75 CI checkouts per pushed SHA, the first one misses
CI_MISS_VCPU_S, PUSH_VCPU_S, FETCH_VCPU_S, MERGE_VCPU_S = 1.0, 0.5, 0.3, 1.5
REPACK_VCPU_S_PER_GB = 30
SSH_SHARE = 0.2                    # share of developer git bytes over SSH (Spectrum)
PLAN = {"small": "cf.plan.pro.monthly", "medium": "cf.plan.business.monthly", "large": "cf.plan.business.monthly"}
VARIANTS = ("design", "separate", "solo", "artifacts")

# ---- git cache shards (CF-ACT-9) ----
# A shard is one container-backed Durable Object per (site, shard). Networks map to shards by rendezvous hashing, and
# each shard keeps an LRU of networks on its disk. The shard count is the daily working set (bytes of networks fetched
# at least once a day) divided by the disk a shard can give to bare repositories. Milestone 0 measures the working set.
WORKING_SET = {"small": 0.30, "medium": 0.20, "large": 0.10}   # share of git_gb in the daily working set [guess]
INSTANCE = {"small": ("standard-2", 1, 6, 12), "medium": ("standard-3", 2, 8, 16), "large": ("standard-4", 4, 12, 20)}
SCRATCH_GB = 4                     # disk per shard kept free for index-pack, repack, and upload-pack scratch [guess]
WARM_COPIES = 2                    # each network is warm on its home site and one more
SSH_CONTAINERS = (3, 0.25, 1, 4)   # basic instances running sshd and cloudflared replicas, one per site
ACCOUNT_VCPU = 1500                # Containers default account limit
# Hot pool (FX-11): reserved per site in the shard class's `max_instances` at every tier, so it uses the tier's
# instance size. Expected hot networks [guess 1] times FANOUT_MAX - 1. Reserved, not warm, so it bills nothing idle.
HOT_POOL_PER_SITE = 1 * (8 - 1)

# ---- container egress by site region and leader split (CF-ACT-13) ----
# Fetch, clone-cache miss, repack upload, and SSH bytes leave the container on the network's leader site, which is
# the owner's preferred (home) site. The split is the share of git bytes whose owners are homed on each site [guess].
SITES = {"A": "enam", "B": "weur", "C": "apac"}
EGRESS_CLASS = {"enam": "na_eu", "weur": "na_eu", "wnam": "na_eu", "eeur": "na_eu", "apac": "other", "oc": "oc"}
EGRESS_PRICE = {"na_eu": ("cf.containers.egress.na_eu", "north-america-europe", 1024),
	"other": ("cf.containers.egress.other", "rest-of-world", 500),
	"oc": ("cf.containers.egress.oceania_korea_taiwan", "oceania-korea-taiwan", 500)}
LEADER_SPLIT = {"A": 0.5, "B": 0.3, "C": 0.2}                   # an NA-heavy install [guess]
APAC_LED = {"A": 0.15, "B": 0.05, "C": 0.8}                     # sensitivity: an install whose owners sit in APAC

# ---- Spectrum, spare sites, ReplaceMember, vault, runner account (CF-ACT-7, 10, 12, 13) ----
SPECTRUM_ADDON_USD = 0.0           # [unverified] the Pro and Business "Paid add-on" fee: no list price found
SPARE_SITES = 3                    # eeur, wnam, oc: script and empty bucket, no members and no containers
CANARY_PER_HOUR = 6                # readiness canary put and get per spare bucket
SITE_LOSS_PER_YEAR = 1             # `ReplaceMember` runs (signed SiteLost or 24 h of zero acks) [guess]
GROUPS_PER_DEV = 8                 # repository, network, owner, and Actions groups per developer [guess]
TOKEN_GROUPS = 3 * 1024            # token shards, three home families
REPLACE_STEPS = 6                  # Workflow steps per group replaced, and the same again for RestorePlacement
ROW_KB = 4                         # average SQLite row written when a new member copies a snapshot [guess]
REMOVED_DAYS = 35                  # removed members run deleteAll() after 35 days
AVG_PACK_MB = 50                   # average stored pack, for per-object pack copies [guess]
VAULT_LOCK_DAYS = 35               # bucket lock retention in the vault account
FEED_OBJECTS = DAYS * 24 * 360     # ChangeBatch objects, one every 10 s

# ---- user records and the Inbox (FX-7) ----
# Stars, watches, subscriptions, follows, and notification thread state are `IdentityEvent`s on the actor's group, and
# a consumer writes coalesced counts to repository metadata tiers. Notifications, feeds, `/issues`, and `/user/repos`
# come from a derived per-account `Inbox` Durable Object with one member, rebuilt from cursors.
USER_RECORDS = 4                   # Identity and coalesced-count entries per developer per day [guess]
INBOX_FANOUT = 3                   # inboxes each metadata write or push notifies [guess]
INBOX_READ_SHARE = 0.05            # share of API requests that read an Inbox [guess]
INBOX_ROWS = 2                     # SQLite rows written per Inbox delivery [guess]
INBOX_MB = 50                      # stored per account, pruned as GitHub prunes notifications [guess]

INCLUDED = dict(workers_req=10e6, workers_cpu=30e6, do_req=1e6, do_gbs=400_000, do_rows_w=50e6, do_storage=5,
	r2_storage=10, r2_a=1e6, r2_b=10e6, queues=1e6, email=3000, ae=10e6, obs_gb=50,
	c_mem_gibs=25 * 3600, c_vcpu_s=375 * 60, c_disk_gbs=200 * 3600, steps=0)


def over(used, included):
	return max(0.0, used - included)


def workload(tier):
	t, a = TIERS[tier], ACTIVITY
	m = lambda per_dev_day: t["devs"] * per_dev_day * DAYS
	w = dict(devs=t["devs"], pushes=m(a["pushes"]), jobs=m(a["ci_jobs"]), fetches=m(a["dev_fetches"]),
		api=m(a["api_requests"]), meta=m(a["metadata_writes"]), emails=m(a["emails"]))
	w["repo_entries"] = w["pushes"] * ENTRY_PER_PUSH + w["meta"] + w["jobs"] * ENTRY_PER_JOB_REPO
	w["actions_entries"] = w["jobs"] * ENTRY_PER_JOB_ACTIONS
	w["identity_entries"] = m(USER_RECORDS)
	w["entries"] = w["repo_entries"] + w["actions_entries"] + w["identity_entries"]
	w["merges"] = w["meta"] * 0.05
	w["git_ops"] = w["pushes"] + w["fetches"] + w["jobs"]
	w["reads"] = w["api"] + w["git_ops"]
	w["ci_gb"] = w["jobs"] * t["checkout_mb"] / 1024
	w["dev_gb"] = w["fetches"] * a["dev_fetch_mb"] / 1024
	w["push_gb"] = w["pushes"] * a["push_mb"] / 1024
	w["groups"] = t["devs"] * GROUPS_PER_DEV + TOKEN_GROUPS
	return t, w


def shards(tier):
	"""Git cache shards: one warm copy of the daily working set, and what any two sites reserve."""
	t = TIERS[tier]
	name, vcpu, gib, disk = INSTANCE[tier]
	working_set_gb = t["git_gb"] * WORKING_SET[tier]
	n = max(1, math.ceil(working_set_gb / (disk - SCRATCH_GB)))
	# max_instances per site is n plus the hot pool, so any two sites can hold both warm copies of every network after a
	# site loss, and fan-out draws on the pool with no site day.
	max_per_site = n + HOT_POOL_PER_SITE
	reserved_vcpu = len(SITES) * max_per_site * vcpu + SSH_CONTAINERS[0] * SSH_CONTAINERS[1]
	return dict(instance=name, vcpu=vcpu, gib=gib, disk=disk, working_set_gb=working_set_gb, shards=n,
		warm=n * WARM_COPIES, hot_pool=HOT_POOL_PER_SITE, max_instances_per_site=max_per_site,
		reserved_vcpu=reserved_vcpu, alarm_vcpu=0.7 * ACCOUNT_VCPU)


def egress_cost(egress_gb, split, accounts_with_sites):
	"""Container egress priced per site region; allowances are per region class per account."""
	by_class = {}
	for site, share in split.items():
		cls = EGRESS_CLASS[SITES[site]]
		by_class[cls] = by_class.get(cls, 0.0) + egress_gb * share
	total = 0.0
	for cls, gb in by_class.items():
		pid, region, included = EGRESS_PRICE[cls]
		total += over(gb, included * accounts_with_sites) * price(pid, region)
	return total


def model(tier, variant="design", split=None):
	split = split or LEADER_SPLIT
	t, w = workload(tier)
	members = 1 if variant in ("solo", "artifacts") else 3
	accounts = 4 if variant == "separate" else 1
	vault = variant in ("design", "separate")
	lines = {}

	lines["Workers Paid subscriptions (install)"] = accounts * price("cf.workers.paid.subscription")
	lines["Zone plan (list; managed and pushes over 200 MB need Enterprise, not priced)"] = price(PLAN[tier])

	# Workers: requests enter at graft-front and reach the edge over a service binding, which bills no second request.
	# In the one-account default, member RPCs are Durable Object binding calls and cost DO requests only. In the
	# account-separated option every member RPC, and every edge call into a group, also passes one site Worker.
	followers = members - 1
	append_rpcs = w["entries"] * followers
	lease_rpcs = w["reads"] * LEASE_MISS if members > 1 else 0.0
	ring_reqs = (append_rpcs + lease_rpcs + w["reads"] * 1.5 + w["entries"]) if variant == "separate" else 0.0
	queue_msgs = (w["pushes"] * 3 + w["jobs"] * 3)
	vault_pulls = (w["pushes"] + FEED_OBJECTS) if vault else 0.0
	edge_reqs = w["api"] + w["git_ops"] * GIT_REQS + queue_msgs + vault_pulls
	cpu = (w["api"] * CPU_MS["api"] + (w["fetches"] + w["jobs"]) * GIT_REQS * CPU_MS["git"]
		+ w["pushes"] * CPU_MS["push"] + ring_reqs * CPU_MS["ring"] + queue_msgs * CPU_MS["queue"]
		+ (w["api"] + w["git_ops"] * GIT_REQS) * CPU_MS["front"])
	lines["Workers requests"] = over(edge_reqs + ring_reqs, INCLUDED["workers_req"] * accounts) / 1e6 * price("cf.workers.requests")
	lines["Workers CPU (graft-front, edge, sites)"] = over(cpu, INCLUDED["workers_cpu"] * accounts) / 1e6 * price("cf.workers.cpu")

	# Durable Objects: leaders, followers, identity and Authz reads, lease rounds. Allowances are per account; the
	# account-separated option holds members in three site accounts.
	do_reqs = w["reads"] * 1.5 + w["entries"] * members + lease_rpcs
	gbs = DO_GB * (w["reads"] * WALL_S["read"] + w["entries"] * WALL_S["commit"]
		+ append_rpcs * WALL_S["append"] + lease_rpcs * WALL_S["lease"])
	rows_w = w["entries"] * members * (1 + FOLD_ROWS)
	inline_gb = w["pushes"] * ACTIVITY["inline_push_kb"] / 1024 / 1024 * 0.25   # inline packs live about a week
	do_gb = (t["core_gb"] + t["aux_gb"] + inline_gb) * members
	site_inc = 3 if variant == "separate" else 1
	lines["Durable Objects requests"] = over(do_reqs, INCLUDED["do_req"] * site_inc) / 1e6 * price("cf.do.requests")
	lines["Durable Objects duration"] = over(gbs, INCLUDED["do_gbs"] * site_inc) / 1e6 * price("cf.do.duration")
	lines["Durable Objects SQLite rows written"] = over(rows_w, INCLUDED["do_rows_w"] * site_inc) / 1e6 * price("cf.do.sqlite.rows_written")
	lines["Durable Objects SQLite storage"] = over(do_gb, INCLUDED["do_storage"] * site_inc) * price("cf.do.sqlite.storage")
	# Inbox: one derived Durable Object per account, one member, fed by the Notify consumer and read by notification,
	# feed, `/issues`, and `/user/repos` requests. Priced as what it adds above the DO usage and allowances just counted.
	inbox_writes = (w["meta"] + w["pushes"]) * INBOX_FANOUT
	inbox_reqs = inbox_writes + w["api"] * INBOX_READ_SHARE
	added = lambda base, extra, inc: over(base + extra, inc) - over(base, inc)
	lines["Inbox Durable Objects (notifications, feeds, cross-repository lists)"] = (
		added(do_reqs, inbox_reqs, INCLUDED["do_req"] * site_inc) / 1e6 * price("cf.do.requests")
		+ added(gbs, DO_GB * inbox_reqs * WALL_S["read"], INCLUDED["do_gbs"] * site_inc) / 1e6 * price("cf.do.duration")
		+ added(rows_w, inbox_writes * INBOX_ROWS, INCLUDED["do_rows_w"] * site_inc) / 1e6 * price("cf.do.sqlite.rows_written")
		+ added(do_gb, w["devs"] * INBOX_MB / 1024, INCLUDED["do_storage"] * site_inc) * price("cf.do.sqlite.storage"))

	# Git bytes: R2 packs (or Artifacts), clone cache, Actions blobs.
	churn = 1 + ACTIVITY["large_repo_share"] * 14 / 7          # cruft kept 14 days, weekly full repack above 1 GiB
	copies = 3 if members == 3 else 1
	if variant == "artifacts":
		art_ops = w["git_ops"] * 2
		lines["Artifacts storage"] = over(t["git_gb"], 1) * price("cf.artifacts.storage")
		lines["Artifacts operations"] = over(art_ops, 10_000) / 1000 * price("cf.artifacts.operations")
		r2_gb = t["actions_gb"]
	else:
		r2_gb = t["git_gb"] * churn * copies + t["git_gb"] * 0.1 + t["actions_gb"]
	r2_a = w["pushes"] * 2 * min(copies, 2) + w["jobs"] * (1 - CI_HIT) + w["jobs"] * 20 + w["entries"] / 500
	r2_b = w["jobs"] * CI_HIT + w["fetches"] * 0.2 + w["jobs"] * 20 + vault_pulls
	lines["R2 storage (pack copies, clone cache, Actions)"] = over(r2_gb, INCLUDED["r2_storage"] * site_inc) * price("cf.r2.standard.storage")
	lines["R2 Class A"] = over(r2_a, INCLUDED["r2_a"] * site_inc) / 1e6 * price("cf.r2.standard.class_a")
	lines["R2 Class B"] = over(r2_b, INCLUDED["r2_b"] * site_inc) / 1e6 * price("cf.r2.standard.class_b")

	# Containers: git cache shards warm on two sites (one site for solo), SSH fronts. Warm shards never sleep, so they
	# carry a day of load with no new instance starts (control-plane row).
	if variant != "artifacts":
		s = shards(tier)
		n = s["shards"] if variant == "solo" else s["warm"]
		mem_gibs = n * s["gib"] * SECONDS + SSH_CONTAINERS[0] * SSH_CONTAINERS[2] * SECONDS
		disk_gbs = n * s["disk"] * SECONDS + SSH_CONTAINERS[0] * SSH_CONTAINERS[3] * SECONDS
		repack_gb = t["git_gb"] * ACTIVITY["large_repo_share"] * 4.3
		vcpu_s = (w["pushes"] * PUSH_VCPU_S + w["fetches"] * FETCH_VCPU_S + w["jobs"] * (1 - CI_HIT) * CI_MISS_VCPU_S
			+ w["merges"] * MERGE_VCPU_S + repack_gb * REPACK_VCPU_S_PER_GB)
		# Developer fetches and clone-cache misses leave a container once (the Worker tees a miss to R2 and the first
		# client). Pushed packs never leave a container: the Worker tees the request body to two buckets while the
		# container only reads it to validate. Repacks upload one copy per pack site. SSH bytes cross the SSH front.
		egress_gb = (w["dev_gb"] + w["ci_gb"] * (1 - CI_HIT) + repack_gb * min(copies, 2)
			+ (w["dev_gb"] + w["push_gb"]) * SSH_SHARE)
		site_split = {"A": 1.0} if variant == "solo" else split
		lines["Containers memory (provisioned)"] = over(mem_gibs, INCLUDED["c_mem_gibs"] * site_inc) * price("cf.containers.memory")
		lines["Containers vCPU (active)"] = over(vcpu_s, INCLUDED["c_vcpu_s"] * site_inc) * price("cf.containers.vcpu")
		lines["Containers disk (provisioned)"] = over(disk_gbs, INCLUDED["c_disk_gbs"] * site_inc) * price("cf.containers.disk")
		lines["Containers egress by site region and leader split"] = egress_cost(egress_gb, site_split, site_inc)

	ssh_gb = (w["dev_gb"] + w["push_gb"]) * SSH_SHARE
	ssh_inc = 5 if PLAN[tier] == "cf.plan.pro.monthly" else 10
	lines["Spectrum SSH data"] = over(ssh_gb, ssh_inc) * price("cf.spectrum.ssh.overage")
	lines["Spectrum SSH add-on fee [unverified, no list price found]"] = SPECTRUM_ADDON_USD
	lines["Queues (webhooks, notifications, indexing)"] = over(queue_msgs * 3, INCLUDED["queues"]) / 1e6 * price("cf.queues.operations")
	lines["Email Sending"] = over(w["emails"], INCLUDED["email"]) / 1000 * price("cf.email.sending")
	lines["Analytics Engine (per-repo counters, SLO points)"] = over(w["reads"] + w["entries"], INCLUDED["ae"]) / 1e6 * price(
		"cf.analytics_engine.writes")
	logs_gb = t["logs_gb_day"] * DAYS
	lines["Workers Logs ingest and storage"] = (over(logs_gb, INCLUDED["obs_gb"]) * price("cf.observability.ingest")
		+ over(logs_gb * 7 / 30, 10) * price("cf.observability.storage"))
	audit_gb = w["entries"] * 2 / 1024 / 1024
	lines["Basin Pipelines (audit and SLO history)"] = audit_gb * price("cf.basin.pipelines.sink_parquet")

	if variant in ("design", "separate"):
		# Spare sites: no members, no containers, an empty bucket probed by a canary. Billed as Class A and B only.
		canary = SPARE_SITES * CANARY_PER_HOUR * 24 * DAYS
		lines["Spare sites (eeur, wnam, oc; scripts and empty buckets)"] = canary / 1e6 * (
			price("cf.r2.standard.class_a") + price("cf.r2.standard.class_b"))
		# ReplaceMember, amortized: a Workflow per group, a snapshot copy into new members, packs copied once per
		# network into the spare bucket for a month, removed members kept 35 days, and RestorePlacement doing it back.
		member_gb = t["core_gb"] + t["aux_gb"]
		event = (2 * w["groups"] * REPLACE_STEPS / 1e5 * price("cf.workflows.steps")
			+ 2 * member_gb * 1024 * 1024 / ROW_KB / 1e6 * price("cf.do.sqlite.rows_written")
			+ 2 * member_gb * REMOVED_DAYS / 30 * price("cf.do.sqlite.storage")
			+ t["git_gb"] * price("cf.r2.standard.storage")
			+ 2 * t["git_gb"] * 1024 / AVG_PACK_MB / 1e6 * price("cf.r2.standard.class_a"))
		lines["ReplaceMember and RestorePlacement (one site loss a year, amortized)"] = event * SITE_LOSS_PER_YEAR / 12
	else:
		lines["Workflows (imports, moves)"] = 0.0

	if vault:
		# Vault account: its own Workers Paid plan; one locked copy of packs (with the cruft a lock retains), member
		# snapshots, and the change feed; one Class A per pack, idx, feed object, and snapshot.
		vault_gb = t["git_gb"] * churn * VAULT_LOCK_DAYS / 30 + t["core_gb"] + t["aux_gb"] + w["entries"] * 2 / 1024 / 1024
		vault_a = w["pushes"] * 2 + FEED_OBJECTS + w["groups"] * 4
		lines["Vault account (plan, locked R2 copy, writes)"] = (price("cf.workers.paid.subscription")
			+ over(vault_gb, INCLUDED["r2_storage"]) * price("cf.r2.standard.storage")
			+ over(vault_a, INCLUDED["r2_a"]) / 1e6 * price("cf.r2.standard.class_a"))
	if variant != "artifacts":
		lines["Runner account (Workers Paid; runner compute excluded)"] = price("cf.workers.paid.subscription")
	return lines


def money(x):
	return f"${x:,.0f}"


def summary():
	print("| Developers | Variant | Per month | Per developer |")
	print("| --- | --- | --- | --- |")
	for tier in TIERS:
		for variant in VARIANTS:
			total = sum(model(tier, variant).values())
			devs = TIERS[tier]["devs"]
			print(f"| {devs:,} | {variant} | {money(total)} | {money(total / devs)} |")
	print()
	print("| Developers | Working set | Instance | Shards | Warm instances | Hot pool per site | max_instances per site "
		"| Reserved vCPU |")
	print("| --- | --- | --- | --- | --- | --- | --- | --- |")
	for tier in TIERS:
		s = shards(tier)
		print(f"| {TIERS[tier]['devs']:,} | {s['working_set_gb']:,.0f} GB | {s['instance']} | {s['shards']} | {s['warm']} "
			f"| {s['hot_pool']} | {s['max_instances_per_site']} | {s['reserved_vcpu']:,.1f} of {ACCOUNT_VCPU:,}, "
			f"{ACCOUNT_VCPU / s['reserved_vcpu']:.1f}x (alarm at {s['alarm_vcpu']:,.0f}) |")
	print()
	print("| Developers | Container egress, NA-heavy split | Container egress, APAC-led split | Default total, APAC-led |")
	print("| --- | --- | --- | --- |")
	for tier in TIERS:
		key = "Containers egress by site region and leader split"
		na, ap = model(tier, "design")[key], model(tier, "design", APAC_LED)
		print(f"| {TIERS[tier]['devs']:,} | {money(na)} | {money(ap[key])} | {money(sum(ap.values()))} |")


def detail(tier, variant):
	lines = model(tier, variant)
	total = sum(lines.values())
	print(f"| Line item ({tier}, {variant}) | Per month | Share |")
	print("| --- | --- | --- |")
	for k, v in sorted(lines.items(), key=lambda kv: -kv[1]):
		print(f"| {k} | {money(v)} | {v / total:.0%} |")
	print(f"| Total | {money(total)} | 100% |")


if __name__ == "__main__":
	args = sys.argv[1:]
	if args[:1] == ["--detail"]:
		detail(args[1], args[2])
	else:
		summary()
