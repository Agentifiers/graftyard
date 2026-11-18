#!/usr/bin/env python3
"""Monthly Cloudflare list cost of Graftyard on Cloudflare (three-site quorum chains on Durable Objects).

Usage:
	python3 scripts/cost.py                       # summary for every tier and variant, shard counts, egress split
	python3 scripts/cost.py --detail large design
	python3 scripts/cost.py --ssh-option            # what the opt-in cloudflared SSH option adds on Pro and Business

Workload comes from scripts/cost_model.py (TIERS, ACTIVITY). Prices are the cf.* entries in research/prices.json.
Every per-operation constant below (requests per git operation, CPU ms, rows per entry, container sizes, hit rates,
working sets, leader split, site-loss rate) is a [guess] until Milestone 0 measures it, except the git CPU constants
tagged [measured]. Excluded, as in
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

Learners are priced in scripts/cost_agents.py: the largest network's REST and GraphQL reads are whole-tier dumps (K9).
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
FOLD_PUTS = 6                      # fold rows an entry puts per member: a `DefineLabel` puts 6 [measured, K12]
ENTRY_ROWS = 17                    # rows written per entry per member: each fold put 2 (a tier table's rowid table and
                                   # its key's automatic index), `log` 1, `request` 2, `raft_state` 1 or 2. The leader
                                   # wrote 19 for one `DefineLabel`, 2 of them test-only [measured, graft-m3f site day
                                   # m3frs, 2026-11-18, K12; the split is inference, `tests/am1/results/lm1-rows-m3f.md`]
LEASE_MISS = 0.25                  # share of reads that find the leader lease expired and pay one read-index RPC
WALL_S = dict(read=0.005, commit=0.12, append=0.005, lease=0.07)
CI_HIT = 0.7                       # clone-cache hit rate: about 3.75 CI checkouts per pushed SHA, the first one misses
# Git CPU per operation. Measured on torvalds/linux (6.4 GiB pack, 12.0 million objects) in M0-WP9, B0-34.
MERGE_VCPU_S = 0.05                # merge-tree test or queue merge: p50 5 to 45 ms by base age, pulls 28 ms, p99 at
                                   # most 338 ms [measured, tests/m0/results/m0-wp9.md, B0-34]
PUSH_VCPU_S = 0.2                  # receive side of a one-commit push, index-pack plus connectivity: 0.14 s on 8 vCPU,
                                   # 0.20 s in a standard-4 [measured, tests/m0/results/m0-wp9.md, B0-34]
CI_MISS_VCPU_S = 0.75              # composed depth-1 clone-cache miss: the tip's cached depth-1 pack plus a pack of the
                                   # wanted commit's missing objects, 0.72 to 0.74 s [measured, tests/m0/results/m0-wp9.md,
                                   # B0-34]
BASE_RENEW_EVERY = 20              # the composed base is rebuilt once per 20 moves of the network's default tip; a tip
                                   # move is a pull request merge (w["merges"])
BASE_RENEW_VCPU_S = 45             # one stock depth-1 build of a 6 GB network, 25 to 46 s [measured,
                                   # tests/m0/results/m0-wp9.md, B0-34]. TIERS has no repository size per tier, so every
                                   # network is priced as the 6 GB one, an upper bound
FETCH_VCPU_S = 0.3
REPACK_VCPU_S_PER_GB = 30
SSH_SHARE = 0.2                    # share of developer git bytes over SSH (Spectrum), on plans in SSH_PLANS
SSH_PLANS = ()                     # SSH needs Enterprise for BYOC: Spectrum refuses a Tunnel origin on Pro [measured,
                                   # tests/m0/results/m0-wp7.md, B0-24], Business until measured [unverified]. No
                                   # tier here is on Enterprise, so the default tiers are HTTPS only: no SSH fronts, bytes or fee
SSH_OPTION_PLANS = ("cf.plan.pro.monthly", "cf.plan.business.monthly")  # the opt-in SSH option: a Tunnel public
                                   # hostname of type ssh, reached by `cloudflared access ssh` (FRAME.md, "Operator
                                   # decision, 2026-10-31, SSH ingress"). Off by default, so the default totals exclude
                                   # it; --ssh-option prices it. Tunnel bytes are not billed and Spectrum is not in the
                                   # path, so it adds the SSH fronts and the SSH bytes crossing them (container egress)
SSH_OPTION = False                 # set by --ssh-option only
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
ACCOUNT_VCPU = 15000               # Containers default account limit (limits page, 2026-11-15)
# Hot pool (FX-11): counted in each site's shard start cap at every tier, so it uses the tier's
# instance size. Expected hot networks [guess 1] times FANOUT_MAX - 1.
HOT_POOL_PER_SITE = 1 * (8 - 1)
# Idle pool shards keep the site's hot networks warm, so a step up in k needs no rehydrate (B0-34, r8: about 290 s from
# saturation to k = 2 serving when cold). The pool is warm only for networks hot in the last hour, so it runs the
# working day plus that hour [guess], on the expected hot network's two warm sites (one for solo). Only the 2,000-developer
# tier has a hot network: at 50 and 300 developers the largest network's git CPU, 0.05 and 0.32 vCPU in
# `scripts/cost_agents.py`, never reaches `fanout`'s up line, so the pool there stays reserved and bills nothing.
HOT_POOL_WARM_TIERS = ("large",)
HOT_POOL_WARM_HOURS = 8 + 1

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
SPECTRUM_ADDON_USD = 0.0           # [unverified] the Spectrum "Paid add-on" fee: no list price found; Enterprise only
SPARE_SITES = 3                    # eeur, wnam, oc: script and empty bucket, no members and no containers
CANARY_PER_HOUR = 6                # readiness canary put and get per spare bucket
SITE_LOSS_PER_YEAR = 1             # `ReplaceMember` runs (signed SiteLost or 24 h of zero acks) [guess]
GROUPS_PER_DEV = 8                 # repository, network, owner, and Actions groups per developer [guess]
TOKEN_GROUPS = 3 * 1024            # token shards, three home families
REPLACE_STEPS = 6                  # Workflow steps per group replaced, and the same again for RestorePlacement
ROW_KB = 0.62                      # member SQLite bytes per fold row, in KB: (dbBytes - logBytes) / rows on graft-m4a's
                                   # repo:65536, 619 to 622 B on two voters and 740 B on its lost one, 6,171 metadata rows
                                   # and 32 core rows; an upper bound on bytes (request rows, FTS5, and snap_log are in
                                   # dbBytes), so a lower bound on rows per GB. Snapshot frames hold 65 B per metadata row
                                   # and 129 B per core row; m2-wp1's seeded comments 2,997 B [measured, 2026-11-18,
                                   # decisions/OPEN-QUESTIONS.md K10]. One figure for both tiers: the core tier is unmeasured
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

# ---- the daily fold audit's refold (K10; `decisions/DECISIONS.md`, "audit-moved" and "`refold-stream`") ----
# A refold stages each tier's newest snapshot as pages (`audit_page`, one row per page) and applies the log tail
# through a view over them, saving each batch's changed rows (`audit_overlay`); its end deletes both. It writes no tier
# table. Each site's cron visits SAMPLE chains per run, 96 runs a day, and a chain at most once a day. Only a chain that
# wrote its fold since its last matching audit is refolded; an unmoved one is compared from the voters' digest rings,
# with no write. The member storage priced below is taken as all repository tiers, an upper bound.
AUDIT_VISITS_PER_SITE = 96 * 4     # cron runs a day x SAMPLE, per site homing chains (`workers/site/src/fold_audit.rs`)
REPO_CHAINS_PER_DEV = 4            # repository chains per developer, of GROUPS_PER_DEV [guess]
AUDIT_MOVED_SHARE = 0.3            # share of tier rows in chains that wrote since their last audit [guess]
SNAP_ROW_B = 406_726 / 6_203       # snapshot frame bytes per fold row: j4's metadata snapshot [measured, graft-m4a,
                                   # 2026-11-18, K10]
AUDIT_PAGE_KB = 256                # staged page bytes (`graftyard_fold::PAGE_BYTES`)
AUDIT_MB_S = 269.5 / 57.7          # snapshot MB a refold reads a second: m3-wp5's deep refold of repo:65536, scratch
                                   # writes included [measured, tests/m3/results/m3-wp5.md]
AUDIT_BATCH_S = 0.02               # member time per batch besides the read [guess]
AUDIT_FIXED_ROWS = 26              # rows an audit writes besides its pages, saved changes and a job row per batch: the
                                   # job's start and prepare writes and delete, the report, the cron's `audit_seen` mark,
                                   # 12 and 26 on two refolds [measured upper bound, graft-m3f site day m3frs, 2026-11-18]
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


def ssh_share(tier):
	"""Share of developer git bytes over SSH: zero on plans where SSH cannot run (SSH_PLANS)."""
	return SSH_SHARE if PLAN[tier] in SSH_PLANS else 0


def ssh_option(tier):
	"""True when the opt-in cloudflared SSH option is priced for this tier's plan."""
	return SSH_OPTION and PLAN[tier] in SSH_OPTION_PLANS


def ssh_fronts(tier):
	"""SSH front containers (sshd and cloudflared, one per site), on plans in SSH_PLANS or with the SSH option."""
	return SSH_CONTAINERS[0] if PLAN[tier] in SSH_PLANS or ssh_option(tier) else 0


def shards(tier):
	"""Git cache shards: one warm copy of the daily working set, and what any two sites reserve."""
	t = TIERS[tier]
	name, vcpu, gib, disk = INSTANCE[tier]
	working_set_gb = t["git_gb"] * WORKING_SET[tier]
	n = max(1, math.ceil(working_set_gb / (disk - SCRATCH_GB)))
	# The start cap per site is n plus the hot pool, so any two sites can hold both warm copies of every network after a
	# site loss, and fan-out draws on the pool with no site day.
	max_per_site = n + HOT_POOL_PER_SITE
	reserved_vcpu = len(SITES) * max_per_site * vcpu + ssh_fronts(tier) * SSH_CONTAINERS[1]
	return dict(instance=name, vcpu=vcpu, gib=gib, disk=disk, working_set_gb=working_set_gb, shards=n,
		warm=n * WARM_COPIES, hot_pool=HOT_POOL_PER_SITE, start_cap_per_site=max_per_site,
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
	rows_w = w["entries"] * members * ENTRY_ROWS
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

	# The fold audit: each visit to a chain that moved refolds its tiers. Rows written: each staged page put and deleted,
	# each changed row saved and deleted, a job row per batch, and the report. Rows read: the tail's log rows, each
	# changed row's page and saved-change reads, and a job row per batch. R2: one Class B range read per MiB of snapshot.
	chains = t["devs"] * REPO_CHAINS_PER_DEV
	visits = min(1.0, AUDIT_VISITS_PER_SITE * len(SITES if members == 3 else "A") / chains)
	refolds = chains * visits * AUDIT_MOVED_SHARE * DAYS
	snap_mb = (t["core_gb"] + t["aux_gb"]) * 1024 * 1024 / ROW_KB * SNAP_ROW_B / 2**20 / chains
	tail = w["repo_entries"] / DAYS / chains / visits
	batches = snap_mb + tail / 8 + 1
	pages = snap_mb * 1024 / AUDIT_PAGE_KB
	audit_w = refolds * (2 * pages + 2 * FOLD_PUTS * tail + batches + AUDIT_FIXED_ROWS)
	audit_r = refolds * (tail + 2 * FOLD_PUTS * tail + batches)
	audit_gbs = refolds * (snap_mb / AUDIT_MB_S + batches * AUDIT_BATCH_S) * DO_GB
	lines["Fold audit refolds (repository chains that moved since their last match)"] = (
		added(rows_w + inbox_writes * INBOX_ROWS, audit_w, INCLUDED["do_rows_w"] * site_inc) / 1e6 * price("cf.do.sqlite.rows_written")
		+ audit_r / 1e6 * price("cf.do.sqlite.rows_read")
		+ refolds * snap_mb / 1e6 * price("cf.r2.standard.class_b")
		+ audit_gbs / 1e6 * price("cf.do.duration"))

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
		mem_gibs = n * s["gib"] * SECONDS + ssh_fronts(tier) * SSH_CONTAINERS[2] * SECONDS
		disk_gbs = n * s["disk"] * SECONDS + ssh_fronts(tier) * SSH_CONTAINERS[3] * SECONDS
		repack_gb = t["git_gb"] * ACTIVITY["large_repo_share"] * 4.3
		vcpu_s = (w["pushes"] * PUSH_VCPU_S + w["fetches"] * FETCH_VCPU_S + w["jobs"] * (1 - CI_HIT) * CI_MISS_VCPU_S
			+ w["merges"] * (MERGE_VCPU_S + BASE_RENEW_VCPU_S / BASE_RENEW_EVERY) + repack_gb * REPACK_VCPU_S_PER_GB)
		# Developer fetches and clone-cache misses leave a container once (the Worker tees a miss to R2 and the first
		# client). Pushed packs never leave a container: the Worker tees the request body to two buckets while the
		# container only reads it to validate. Repacks upload one copy per pack site. SSH bytes cross the SSH front.
		egress_gb = (w["dev_gb"] + w["ci_gb"] * (1 - CI_HIT) + repack_gb * min(copies, 2)
			+ (w["dev_gb"] + w["push_gb"]) * (SSH_SHARE if ssh_option(tier) else ssh_share(tier)))
		site_split = {"A": 1.0} if variant == "solo" else split
		lines["Containers memory (provisioned)"] = over(mem_gibs, INCLUDED["c_mem_gibs"] * site_inc) * price("cf.containers.memory")
		lines["Containers vCPU (active)"] = over(vcpu_s, INCLUDED["c_vcpu_s"] * site_inc) * price("cf.containers.vcpu")
		lines["Containers disk (provisioned)"] = over(disk_gbs, INCLUDED["c_disk_gbs"] * site_inc) * price("cf.containers.disk")
		lines["Containers egress by site region and leader split"] = egress_cost(egress_gb, site_split, site_inc)
		# Hot pool kept warm in working hours, priced above the warm shards' memory and disk and their allowances.
		pool = HOT_POOL_PER_SITE * (1 if variant == "solo" else WARM_COPIES) if tier in HOT_POOL_WARM_TIERS else 0
		pool_s = pool * HOT_POOL_WARM_HOURS / 24 * SECONDS
		added = lambda base, extra, inc: over(base + extra, inc) - over(base, inc)
		lines["Containers: hot pool kept warm for hot networks, working hours"] = (
			added(mem_gibs, pool_s * s["gib"], INCLUDED["c_mem_gibs"] * site_inc) * price("cf.containers.memory")
			+ added(disk_gbs, pool_s * s["disk"], INCLUDED["c_disk_gbs"] * site_inc) * price("cf.containers.disk"))

	ssh_gb = (w["dev_gb"] + w["push_gb"]) * ssh_share(tier)
	ssh_inc = 5 if PLAN[tier] == "cf.plan.pro.monthly" else 10
	lines["Spectrum SSH data"] = over(ssh_gb, ssh_inc) * price("cf.spectrum.ssh.overage")
	lines["Spectrum SSH add-on fee [unverified, no list price found]"] = SPECTRUM_ADDON_USD if PLAN[tier] in SSH_PLANS else 0
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
	print("| Developers | Working set | Instance | Shards | Warm instances | Hot pool per site | Start cap per site "
		"| Reserved vCPU |")
	print("| --- | --- | --- | --- | --- | --- | --- | --- |")
	for tier in TIERS:
		s = shards(tier)
		print(f"| {TIERS[tier]['devs']:,} | {s['working_set_gb']:,.0f} GB | {s['instance']} | {s['shards']} | {s['warm']} "
			f"| {s['hot_pool']} | {s['start_cap_per_site']} | {s['reserved_vcpu']:,.1f} of {ACCOUNT_VCPU:,}, "
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


def ssh_option_report():
	"""What turning on the cloudflared SSH option adds to the default install, per tier, without changing the defaults."""
	global SSH_OPTION
	print("| Developers | Plan | Default | With the SSH option | Added | Of which SSH front containers | Of which egress |")
	print("| --- | --- | --- | --- | --- | --- | --- |")
	keys = ("Containers memory (provisioned)", "Containers disk (provisioned)", "Containers vCPU (active)")
	egress = "Containers egress by site region and leader split"
	for tier in TIERS:
		base = model(tier)
		SSH_OPTION = True
		try:
			with_opt = model(tier)
		finally:
			SSH_OPTION = False
		fronts = sum(with_opt[k] - base[k] for k in keys)
		added = sum(with_opt.values()) - sum(base.values())
		plan = PLAN[tier].split(".")[2].capitalize()
		print(f"| {TIERS[tier]['devs']:,} | {plan} | {money(sum(base.values()))} | {money(sum(with_opt.values()))} "
			f"| {money(added)} | {money(fronts)} | {money(with_opt[egress] - base[egress])} |")


if __name__ == "__main__":
	args = sys.argv[1:]
	if args[:1] == ["--detail"]:
		detail(args[1], args[2])
	elif args[:1] == ["--ssh-option"]:
		ssh_option_report()
	else:
		summary()
