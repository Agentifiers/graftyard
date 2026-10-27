#!/usr/bin/env python3
"""Monthly Cloudflare list cost of Tributary candidate 3 (three-site quorum chains on Durable Objects).

Usage:
	python3 arena/cloudflare/candidate-3/cost.py                 # summary for every tier and variant
	python3 arena/cloudflare/candidate-3/cost.py --detail large design

Workload comes from scripts/cost_model.py (TIERS, ACTIVITY). Prices are the cf.* entries in research/prices.json.
Every per-operation constant below (requests per git operation, CPU ms, rows per entry, container sizes, hit rates)
is a [guess] until Milestone 0 measures it. Excluded, as in scripts/cost_model.py: Actions runner compute, the
vendor fee, tax, and any Enterprise contract (Enterprise has no list price; the plan line uses Business list).

Variants:
	design     three site accounts (enam, wnam, weur) plus an edge account; every chain is a 3-member group; packs in
	           two buckets in two sites before the ack, a third copy async; warm git containers in two sites.
	cheap      the 50-developer floor that keeps the region-loss story: one account, three site scripts instead of
	           three accounts, Pro plan, one standard-2 git container per site in two sites.
	single     the rejected alternative: one Durable Object per chain, one account, one pack bucket, one git site.
	artifacts  the rejected alternative: git in Cloudflare Artifacts, one Durable Object per chain for metadata.
"""

import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "scripts"))
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
CONTAINERS = {                     # (count, vCPU, GiB, disk GB) git containers per tier, split over two sites
	"small": (2, 1, 6, 12), "medium": (4, 2, 8, 16), "large": (12, 4, 12, 20)}
SSH_CONTAINERS = (3, 0.25, 1, 4)   # basic instances running sshd and cloudflared replicas
PLAN = {"small": "cf.plan.pro.monthly", "medium": "cf.plan.business.monthly", "large": "cf.plan.business.monthly"}

INCLUDED = dict(workers_req=10e6, workers_cpu=30e6, do_req=1e6, do_gbs=400_000, do_rows_w=50e6, do_storage=5,
	r2_storage=10, r2_a=1e6, r2_b=10e6, queues=1e6, email=3000, ae=10e6, obs_gb=50,
	c_mem_gibs=25 * 3600, c_vcpu_s=375 * 60, c_disk_gbs=200 * 3600, c_egress=1024)


def over(used, included):
	return max(0.0, used - included)


def workload(tier):
	t, a = TIERS[tier], ACTIVITY
	m = lambda per_dev_day: t["devs"] * per_dev_day * DAYS
	w = dict(devs=t["devs"], pushes=m(a["pushes"]), jobs=m(a["ci_jobs"]), fetches=m(a["dev_fetches"]),
		api=m(a["api_requests"]), meta=m(a["metadata_writes"]), emails=m(a["emails"]))
	w["repo_entries"] = w["pushes"] * ENTRY_PER_PUSH + w["meta"] + w["jobs"] * ENTRY_PER_JOB_REPO
	w["actions_entries"] = w["jobs"] * ENTRY_PER_JOB_ACTIONS
	w["entries"] = w["repo_entries"] + w["actions_entries"]
	w["merges"] = w["meta"] * 0.05
	w["git_ops"] = w["pushes"] + w["fetches"] + w["jobs"]
	w["reads"] = w["api"] + w["git_ops"]
	w["ci_gb"] = w["jobs"] * t["checkout_mb"] / 1024
	w["dev_gb"] = w["fetches"] * a["dev_fetch_mb"] / 1024
	w["push_gb"] = w["pushes"] * a["push_mb"] / 1024
	return t, w


def model(tier, variant="design"):
	t, w = workload(tier)
	members = 1 if variant in ("single", "artifacts") else 3
	accounts = 4 if variant == "design" else 1
	lines = {}

	lines["Workers Paid subscriptions"] = accounts * price("cf.workers.paid.subscription")
	plan = "cf.plan.pro.monthly" if variant == "cheap" else PLAN[tier]
	lines["Zone plan (list; managed and 2 GB pushes need Enterprise, not priced)"] = price(plan)

	# Workers: edge requests, plus one ring Worker request per cross-account follower RPC.
	followers = members - 1
	append_rpcs = w["entries"] * followers
	lease_rpcs = w["reads"] * LEASE_MISS if members > 1 else 0.0
	# Separate site accounts: every member RPC, and every edge call into a group, passes one site Worker.
	ring_reqs = (append_rpcs + lease_rpcs + w["reads"] * 1.5 + w["entries"]) if variant == "design" else 0.0
	queue_msgs = (w["pushes"] * 3 + w["jobs"] * 3)
	edge_reqs = w["api"] + w["git_ops"] * GIT_REQS + queue_msgs
	cpu = (w["api"] * CPU_MS["api"] + (w["fetches"] + w["jobs"]) * GIT_REQS * CPU_MS["git"]
		+ w["pushes"] * CPU_MS["push"] + ring_reqs * CPU_MS["ring"] + queue_msgs * CPU_MS["queue"])
	inc = accounts if variant == "design" else 1
	lines["Workers requests"] = over(edge_reqs + ring_reqs, INCLUDED["workers_req"] * inc) / 1e6 * price("cf.workers.requests")
	lines["Workers CPU"] = over(cpu, INCLUDED["workers_cpu"] * inc) / 1e6 * price("cf.workers.cpu")

	# Durable Objects: leaders, followers, identity and Authz reads, lease rounds.
	do_reqs = w["reads"] * 1.5 + w["entries"] * members + lease_rpcs
	gbs = DO_GB * (w["reads"] * WALL_S["read"] + w["entries"] * WALL_S["commit"]
		+ append_rpcs * WALL_S["append"] + lease_rpcs * WALL_S["lease"])
	rows_w = w["entries"] * members * (1 + FOLD_ROWS)
	inline_gb = w["pushes"] * ACTIVITY["inline_push_kb"] / 1024 / 1024 * 0.25   # inline packs live about a week
	do_gb = (t["core_gb"] + t["aux_gb"] + inline_gb) * members
	ring_inc = 3 if variant == "design" else 1
	lines["Durable Objects requests"] = over(do_reqs, INCLUDED["do_req"] * ring_inc) / 1e6 * price("cf.do.requests")
	lines["Durable Objects duration"] = over(gbs, INCLUDED["do_gbs"] * ring_inc) / 1e6 * price("cf.do.duration")
	lines["Durable Objects SQLite rows written"] = over(rows_w, INCLUDED["do_rows_w"] * ring_inc) / 1e6 * price("cf.do.sqlite.rows_written")
	lines["Durable Objects SQLite storage"] = over(do_gb, INCLUDED["do_storage"] * ring_inc) * price("cf.do.sqlite.storage")

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
	r2_b = w["jobs"] * CI_HIT + w["fetches"] * 0.2 + w["jobs"] * 20
	lines["R2 storage (pack copies, clone cache, Actions)"] = over(r2_gb, INCLUDED["r2_storage"] * ring_inc) * price("cf.r2.standard.storage")
	lines["R2 Class A"] = over(r2_a, INCLUDED["r2_a"] * ring_inc) / 1e6 * price("cf.r2.standard.class_a")
	lines["R2 Class B"] = over(r2_b, INCLUDED["r2_b"] * ring_inc) / 1e6 * price("cf.r2.standard.class_b")

	# Containers: warm git caches in two sites, SSH front.
	if variant != "artifacts":
		n, vcpu, gib, disk = CONTAINERS[tier]
		if variant == "cheap":
			n, vcpu, gib, disk = 2, 1, 6, 12
		if variant == "single":
			n = max(1, n // 2)
		mem_gibs = n * gib * SECONDS + SSH_CONTAINERS[0] * SSH_CONTAINERS[2] * SECONDS
		disk_gbs = n * disk * SECONDS + SSH_CONTAINERS[0] * SSH_CONTAINERS[3] * SECONDS
		repack_gb = t["git_gb"] * ACTIVITY["large_repo_share"] * 4.3
		vcpu_s = (w["pushes"] * PUSH_VCPU_S + w["fetches"] * FETCH_VCPU_S + w["jobs"] * (1 - CI_HIT) * CI_MISS_VCPU_S
			+ w["merges"] * MERGE_VCPU_S + repack_gb * REPACK_VCPU_S_PER_GB)
		# Developer fetches and clone-cache misses leave a container once (the Worker tees a miss to R2 and the first
		# client). Pushed packs never leave a container: the Worker tees the request body to two buckets while the
		# container only reads it to validate. Repacks upload one copy per pack site. SSH bytes cross the SSH front.
		egress_gb = (w["dev_gb"] + w["ci_gb"] * (1 - CI_HIT) + repack_gb * min(copies, 2)
			+ (w["dev_gb"] + w["push_gb"]) * SSH_SHARE)
		lines["Containers memory (provisioned)"] = over(mem_gibs, INCLUDED["c_mem_gibs"]) * price("cf.containers.memory")
		lines["Containers vCPU (active)"] = over(vcpu_s, INCLUDED["c_vcpu_s"]) * price("cf.containers.vcpu")
		lines["Containers disk (provisioned)"] = over(disk_gbs, INCLUDED["c_disk_gbs"]) * price("cf.containers.disk")
		lines["Containers egress (fetches, misses, pack copies)"] = over(egress_gb, INCLUDED["c_egress"]) * price(
			"cf.containers.egress.na_eu", "north-america-europe")

	ssh_gb = (w["dev_gb"] + w["push_gb"]) * SSH_SHARE
	ssh_inc = 5 if plan == "cf.plan.pro.monthly" else 10
	lines["Spectrum SSH data"] = over(ssh_gb, ssh_inc) * price("cf.spectrum.ssh.overage")
	lines["Queues (webhooks, notifications, indexing)"] = over(queue_msgs * 3, INCLUDED["queues"]) / 1e6 * price("cf.queues.operations")
	lines["Workflows (moves, imports, member replacement)"] = 0.0
	lines["Email Sending"] = over(w["emails"], INCLUDED["email"]) / 1000 * price("cf.email.sending")
	lines["Analytics Engine (per-repo counters, SLO points)"] = over(w["reads"] + w["entries"], INCLUDED["ae"]) / 1e6 * price(
		"cf.analytics_engine.writes")
	logs_gb = t["logs_gb_day"] * DAYS
	lines["Workers Logs ingest and storage"] = (over(logs_gb, INCLUDED["obs_gb"]) * price("cf.observability.ingest")
		+ over(logs_gb * 7 / 30, 10) * price("cf.observability.storage"))
	audit_gb = w["entries"] * 2 / 1024 / 1024
	lines["Basin Pipelines (audit and SLO history)"] = audit_gb * price("cf.basin.pipelines.sink_parquet")
	return lines


def money(x):
	return f"${x:,.0f}"


def summary():
	print("| Developers | Variant | Per month | Per developer |")
	print("| --- | --- | --- | --- |")
	for tier in TIERS:
		for variant in ("design", "single", "artifacts"):
			total = sum(model(tier, variant).values())
			devs = TIERS[tier]["devs"]
			print(f"| {devs:,} | {variant} | {money(total)} | {money(total / devs)} |")
	total = sum(model("small", "cheap").values())
	print(f"| 50 | cheap | {money(total)} | {money(total / 50)} |")


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
