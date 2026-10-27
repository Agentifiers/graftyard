#!/usr/bin/env python3
"""Monthly Cloudflare list cost of one Tributary install (candidate 1: one Durable Object per aggregate).

Usage:
	python3 arena/cloudflare/candidate-1/cost.py                        # summary, every tier and topology
	python3 arena/cloudflare/candidate-1/cost.py --detail small paired  # line items for one tier and topology
	python3 arena/cloudflare/candidate-1/cost.py --cheapest             # cheapest reliable setup for 50 developers

Workload: TIERS and ACTIVITY from scripts/cost_model.py, unchanged. Prices: the cf.* entries in
research/prices.json. Every per-operation count below (DO calls per request, rows per write, CPU per
request, container sizes, cache hit share, SSH share) is a [guess] from DESIGN.md until Milestone 0
measures it. Not included: the vendor's fee, Actions runner compute (as in scripts/cost_model.py), an
Enterprise contract (unpriced), and tax.
"""

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts"))
from cost_model import ACTIVITY, TIERS  # noqa: E402

DAYS = 30.4
SECONDS = 730 * 3600
BUSINESS_SECONDS = 10 * 3600 * 22  # hours a scale-to-zero container pool is awake [guess]

PRICES = {p["id"]: p["usd"] for p in json.loads((ROOT / "research" / "prices.json").read_text()) if p["id"].startswith("cf.")}


def p(pid):
	return PRICES[pid]


# Container instance types (research/cloudflare.md, Containers): vCPU, GiB memory, GB disk.
INSTANCE = {"lite": (1 / 16, 0.25, 2), "basic": (0.25, 1, 4), "standard-1": (0.5, 4, 8),
	"standard-2": (1, 6, 12), "standard-3": (2, 8, 16), "standard-4": (4, 12, 20)}

# Per tier: zone plan, warm git-worker pool per serving region, SSH gateways per serving region.
SHAPE = {
	"small": dict(plan="cf.plan.pro.monthly", workers=("standard-1", 2), ssh=("basic", 2)),
	"medium": dict(plan="cf.plan.business.monthly", workers=("standard-3", 2), ssh=("basic", 2)),
	"large": dict(plan="cf.plan.business.monthly", workers=("standard-4", 4), ssh=("standard-1", 2)),
}
SSH_INCLUDED_GB = {"cf.plan.pro.monthly": 5, "cf.plan.business.monthly": 10}

GUESS = dict(
	do_calls_api=2.1,        # AuthzReplica + one aggregate, plus 5% token or login cache misses
	do_calls_fetch=3,        # AuthzReplica, Repo snapshot, git-worker container object
	do_calls_push=5,         # AuthzReplica, Repo advertise, git-worker, Network register, Repo commit
	do_calls_job=25,         # Actions run and scope object calls per job
	consumers=6,             # event consumers subscribed per entry (hooks, actions, prsync, notify, search, audit)
	resident_s=0.25,         # billed DO wall time per request at 128 MB
	rows_push=12, rows_meta=15, rows_job=50, rows_entry_overhead=3,  # cursor advances, archive delete, alarm
	worker_cpu_ms_api=4, worker_cpu_ms_git=6, worker_cpu_ms_per_push_mb=2,
	cache_hit=0.8,           # CI shallow clones served from the clone cache in R2
	ssh_share=0.3,           # share of developer fetch and push bytes that use SSH
	push_cpu_s=1.5, clone_cpu_s_per_100mb=1.0, repack_cpu_s_per_gb=20,
	metadata_copies_paired=2,
	lease_renew_s=2.5,       # Paired: standby lease renewal while a primary is active
	active_objects_per_dev=0.4,  # chain objects resident during business hours, per developer
)


def model(tier, topo, cheap=False):
	t, a, g, s = TIERS[tier], ACTIVITY, GUESS, SHAPE[tier]
	paired = topo == "paired"
	copies = 2 if paired else 1
	devs = t["devs"]
	month = lambda per_dev_day: devs * per_dev_day * DAYS
	pushes, jobs, fetches = month(a["pushes"]), month(a["ci_jobs"]), month(a["dev_fetches"])
	api, meta, emails = month(a["api_requests"]), month(a["metadata_writes"]), month(a["emails"])
	entries = pushes + meta + jobs * 3            # pushes, metadata writes, required check conclusions
	git_requests = 2 * (pushes + jobs + fetches)  # discovery request plus the POST
	deliveries = pushes * 2 + meta
	L = {}

	L["Workers Paid plan"] = p("cf.workers.paid.subscription")
	plan = s["plan"]
	L[f"Zone plan ({plan.split('.')[2].title()}, monthly) for Spectrum SSH and request body size"] = (
		20 if cheap else p(plan))

	requests = api + git_requests + deliveries + entries * g["consumers"] / 10
	L["Workers requests"] = max(0, requests - 10e6) / 1e6 * p("cf.workers.requests")
	cpu_ms = (api * g["worker_cpu_ms_api"] + git_requests * g["worker_cpu_ms_git"]
		+ pushes * a["push_mb"] * g["worker_cpu_ms_per_push_mb"] + deliveries * 3)
	L["Workers CPU"] = max(0, cpu_ms - 30e6) / 1e6 * p("cf.workers.cpu")

	do_calls = (api * g["do_calls_api"] + (jobs + fetches) * g["do_calls_fetch"] + pushes * g["do_calls_push"]
		+ jobs * g["do_calls_job"] + meta * 2 + entries * g["consumers"] * 2)
	active_s = devs * g["active_objects_per_dev"] * BUSINESS_SECONDS
	if paired:
		do_calls += entries + active_s / g["lease_renew_s"]  # standby applies and lease renewals
	L["Durable Objects requests"] = max(0, do_calls - 1e6) / 1e6 * p("cf.do.requests")

	w_kind, w_count = ("standard-1", 1) if cheap else s["workers"]
	ssh_kind, ssh_count = ("lite", 2) if cheap else s["ssh"]
	regions = 2 if paired else 1
	worker_seconds = BUSINESS_SECONDS if cheap else SECONDS
	container_instances_s = (w_count * worker_seconds * (1 + (0.5 if paired else 0)) + ssh_count * SECONDS * regions)
	gb_s = do_calls * g["resident_s"] * 0.125 + container_instances_s * 0.125
	L["Durable Objects duration (chain objects and container objects)"] = max(0, gb_s - 400_000) / 1e6 * p("cf.do.duration")

	rows = (pushes * g["rows_push"] + meta * g["rows_meta"] + jobs * g["rows_job"]
		+ entries * (g["consumers"] + g["rows_entry_overhead"]))
	rows *= copies
	L["Durable Objects SQLite rows written"] = max(0, rows - 50e6) / 1e6 * p("cf.do.sqlite.rows_written")
	meta_gb = (t["core_gb"] + t["aux_gb"]) * (g["metadata_copies_paired"] if paired else 1)
	L["Durable Objects SQLite storage"] = max(0, meta_gb - 5) * p("cf.do.sqlite.storage")

	churn = 1 + a["large_repo_share"] * 14 / 7    # superseded packs kept 14 days after compaction
	pack_gb = t["git_gb"] * churn * copies
	cache_gb = t["git_gb"] * 0.1
	archive_gb = entries * 1 / 1e6 * 12 * copies  # 1 KB entries, a year of chain archive
	stored = pack_gb + cache_gb + archive_gb + t["actions_gb"]
	L["R2 storage (packs in each copy, clone cache, chain archive, Actions)"] = max(0, stored - 10) * p("cf.r2.standard.storage")
	class_a = (pushes * (1 + 2 * copies) + entries / 100 * copies + jobs * 20
		+ jobs * (1 - g["cache_hit"]) * 0.2)
	class_b = jobs * 20 + jobs * g["cache_hit"] + (pushes + fetches) * 0.5
	L["R2 Class A operations"] = max(0, class_a - 1e6) / 1e6 * p("cf.r2.standard.class_a")
	L["R2 Class B operations"] = max(0, class_b - 10e6) / 1e6 * p("cf.r2.standard.class_b")

	ci_gb = jobs * t["checkout_mb"] / 1024
	dev_gb = fetches * a["dev_fetch_mb"] / 1024
	push_gb = pushes * a["push_mb"] / 1024
	vcpu = lambda kind: INSTANCE[kind][0]
	mem = lambda kind: INSTANCE[kind][1]
	disk = lambda kind: INSTANCE[kind][2]
	standby_warm = 0.5 if paired else 0   # half the pool kept warm in the standby region
	w_s = w_count * worker_seconds * (1 + standby_warm)
	s_s = ssh_count * SECONDS * regions
	mem_gib_s = w_s * mem(w_kind) + s_s * mem(ssh_kind)
	disk_gb_s = w_s * disk(w_kind) + s_s * disk(ssh_kind)
	cpu_s = (pushes * g["push_cpu_s"] + (ci_gb * (1 - g["cache_hit"]) + dev_gb) * 10 * g["clone_cpu_s_per_100mb"] / 1
		+ t["git_gb"] * a["large_repo_share"] * 4.3 * g["repack_cpu_s_per_gb"])
	L["Containers memory (git workers, SSH gateways)"] = max(0, mem_gib_s - 25 * 3600) * p("cf.containers.memory")
	L["Containers CPU (index-pack, upload-pack, repack)"] = max(0, cpu_s - 375 * 60) * p("cf.containers.vcpu")
	L["Containers disk"] = max(0, disk_gb_s - 200 * 3600) * p("cf.containers.disk")
	egress_gb = ci_gb * (1 - g["cache_hit"]) + dev_gb
	L["Containers egress (uncached clone and fetch bytes)"] = max(0, egress_gb - 1024) * p("cf.containers.egress.na_eu")

	ssh_gb = (dev_gb + push_gb) * g["ssh_share"]
	L["Spectrum SSH overage"] = max(0, ssh_gb - SSH_INCLUDED_GB[plan]) * p("cf.spectrum.ssh.overage")

	queue_ops = entries * g["consumers"] * 3
	L["Queues (dirty markers)"] = max(0, queue_ops - 1e6) / 1e6 * p("cf.queues.operations")
	logs = t["logs_gb_day"] * DAYS
	L["Workers Logs (observability ingest and storage)"] = (max(0, logs - 50) * p("cf.observability.ingest")
		+ max(0, logs * 7 / 30 - 10) * p("cf.observability.storage"))
	points = git_requests + pushes
	L["Workers Analytics Engine (traffic counters)"] = max(0, points - 10e6) / 1e6 * p("cf.analytics_engine.writes")
	audit_gb = entries * 1 / 1e6
	L["Basin Pipelines and Catalog (audit log)"] = (audit_gb * p("cf.basin.pipelines.sink_parquet")
		+ max(0, entries / 1000 - 1e6) / 1e6 * p("cf.basin.catalog.operations"))
	L["Email Sending"] = max(0, emails - 3000) / 1000 * p("cf.email.sending")
	return L


def money(x):
	return f"${x:,.0f}"


def summary():
	print("| Developers | Regional | Paired |")
	print("| --- | --- | --- |")
	for tier in ("small", "medium", "large"):
		r, pd = sum(model(tier, "regional").values()), sum(model(tier, "paired").values())
		print(f"| {TIERS[tier]['devs']:,} | {money(r)} | {money(pd)} |")
	print(f"\nCheapest reliable setup for 50 developers (Regional, Pro billed annually, scale-to-zero git workers, "
		f"two lite SSH gateways): {money(sum(model('small', 'regional', cheap=True).values()))}")


def detail(tier, topo, cheap=False):
	lines = model(tier, topo, cheap)
	total = sum(lines.values())
	print("| Line item | Per month | Share |")
	print("| --- | --- | --- |")
	for name, cost in sorted(lines.items(), key=lambda kv: -kv[1]):
		print(f"| {name} | {money(cost)} | {cost / total:.0%} |")
	print(f"| Total | {money(total)} | 100% |")


if __name__ == "__main__":
	args = sys.argv[1:]
	if args[:1] == ["--detail"]:
		detail(args[1], args[2])
	elif args[:1] == ["--cheapest"]:
		detail("small", "regional", cheap=True)
	else:
		summary()
