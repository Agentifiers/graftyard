#!/usr/bin/env python3
"""Monthly Cloudflare list cost of one Tributary install for one company, managed-services design (candidate 2).

Usage:
	python3 arena/cloudflare/candidate-2/cost.py                 # summary for 50, 300, 2,000 developers
	python3 arena/cloudflare/candidate-2/cost.py --detail medium  # line items for one tier
	python3 arena/cloudflare/candidate-2/cost.py --cheapest       # cheapest reliable setups for 50 developers
	python3 arena/cloudflare/candidate-2/cost.py --sensitivity    # what moves the total

Workload: TIERS and ACTIVITY from scripts/cost_model.py, unchanged. Prices: every cf.* entry in
research/prices.json. Every coefficient in SHAPE below is a guess about this design's request shape
until Milestone 0 measures it. Excluded, as in research/costs.md: vendor fee, CI runner compute,
support plans, tax. Enterprise zone plans are contract priced, so the model prices the Business plan.
"""

import json
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts"))
from cost_model import TIERS, ACTIVITY, DAYS  # noqa: E402

HOURS = 730
P = {p["id"]: p["usd"] for p in json.loads((ROOT / "research" / "prices.json").read_text()) if p["id"].startswith("cf.")}

SHAPE = dict(
	artifacts_share=0.6,      # share of git requests that hit repositories in the Artifacts tier (the rest are Large)
	ci_cache_hit=0.8,         # CI clones answered from the R2 clone cache
	merges_per_push=0.3,      # pull request merges per push
	content_reads=0.1,        # share of API requests that read git objects through the engine
	git_http_per_fetch=3,     # info/refs, ls-refs, fetch
	git_http_per_push=2,      # info/refs, receive-pack
	runner_http_per_job=150,  # broker polls, renewals, step updates, log appends
	do_per_api=1.2, do_per_fetch=1, do_per_push=4, do_per_merge=4, do_per_job=40, do_per_meta=2,
	do_active_s=0.03, do_wakes_per_req=0.05, do_idle_s=10, do_gb=0.128,
	rows_w_push=12, rows_w_merge=20, rows_w_meta=6, rows_w_job=30, rows_w_event=2, rows_r_api=25,
	d1_miss=0.1, d1_rows_per_lookup=3,
	art_ops_push=3, art_ops_merge=3, art_ops_fetch=1, art_ops_content=2, content_cache_hit=0.8, fork_overhead=0.1,
	cpu_ms_api=4, cpu_ms_git=2, cpu_ms_per_mb=1, cpu_ms_runner=1,
	events_push=3, events_job=3, events_meta=1, events_merge=4, queue_ops_per_msg=3, deliveries_per_event=0.5,
	large_host_gb=8,          # packs per standard-4 Large host: 20 GB disk holds the packs plus a repack's temporary copy
	large_hours=220,          # warm hours per Large host per month (10 h on 22 working days), asleep otherwise
	large_cpu_active=0.15,
	compute_per_devs=150, compute_hours=365, compute_cpu_active=0.2,
	ssh_share=0.5,            # share of developer git bytes over SSH (CI uses HTTPS)
	packlog_factor=1.2,       # R2 copy of Artifacts-tier repositories: consolidated pack plus raw push packs
	large_churn=1.5, large_copies=2,  # cruft and superseded packs kept 14 days; two R2 buckets before ack
	clone_cache_factor=0.2,
	logs_free_gb=50,
)
INSTANCE = {"basic": (0.25, 1, 4), "standard-2": (1, 6, 12), "standard-3": (2, 8, 16), "standard-4": (4, 12, 20)}


def container(kind, count, hours, cpu_active):
	vcpu, gib, disk = INSTANCE[kind]
	s = count * hours * 3600
	return s * (vcpu * cpu_active * P["cf.containers.vcpu"] + gib * P["cf.containers.memory"] + disk * P["cf.containers.disk"])


def over(x, free):
	return max(0.0, x - free)


def model(tier, plan="business", ssh=True, packlog=True, compute_floor=1, **override):
	t, a, s = TIERS[tier], dict(ACTIVITY), dict(SHAPE, **override)
	if "large_repo_share" in override:
		a["large_repo_share"] = override["large_repo_share"]
	devs = t["devs"]
	m = lambda per_dev_day: devs * per_dev_day * DAYS
	pushes, jobs, fetches, api, meta = m(a["pushes"]), m(a["ci_jobs"]), m(a["dev_fetches"]), m(a["api_requests"]), m(a["metadata_writes"])
	merges = pushes * s["merges_per_push"]
	art, large_share = s["artifacts_share"], 1 - s["artifacts_share"]
	ci_gb = jobs * t["checkout_mb"] / 1024
	dev_gb = fetches * a["dev_fetch_mb"] / 1024
	push_gb = pushes * a["push_mb"] / 1024
	lines = {}

	lines["Workers Paid subscription"] = P["cf.workers.paid.subscription"]
	lines["Zone plan (" + plan + ")"] = {"free": 0, "pro": P["cf.plan.pro.monthly"], "business": P["cf.plan.business.monthly"]}[plan]

	git_req = (fetches + jobs) * s["git_http_per_fetch"] + pushes * s["git_http_per_push"]
	reqs = git_req + api + jobs * s["runner_http_per_job"] + merges
	lines["Workers requests"] = over(reqs / 1e6, 10) * P["cf.workers.requests"]
	cpu = (api * s["cpu_ms_api"] + git_req * s["cpu_ms_git"] + (ci_gb + dev_gb + push_gb) * 1024 * s["cpu_ms_per_mb"]
		+ jobs * s["runner_http_per_job"] * s["cpu_ms_runner"])
	lines["Workers CPU"] = over(cpu / 1e6, 30) * P["cf.workers.cpu"]

	do_req = (api * s["do_per_api"] + (fetches + jobs) * s["do_per_fetch"] + pushes * s["do_per_push"]
		+ merges * s["do_per_merge"] + jobs * s["do_per_job"] + meta * s["do_per_meta"])
	lines["Durable Objects requests"] = over(do_req / 1e6, 1) * P["cf.do.requests"]
	gbs = do_req * (s["do_active_s"] + s["do_wakes_per_req"] * s["do_idle_s"]) * s["do_gb"]
	lines["Durable Objects duration"] = over(gbs / 1e6, 0.4) * P["cf.do.duration"]
	events = pushes * s["events_push"] + jobs * s["events_job"] + meta * s["events_meta"] + merges * s["events_merge"]
	rows_w = (pushes * s["rows_w_push"] + merges * s["rows_w_merge"] + meta * s["rows_w_meta"] + jobs * s["rows_w_job"]
		+ events * s["rows_w_event"])
	rows_r = api * s["rows_r_api"] + do_req * 5
	lines["Durable Objects SQLite rows"] = (over(rows_w / 1e6, 50) * P["cf.do.sqlite.rows_written"]
		+ over(rows_r / 1e6, 25_000) * P["cf.do.sqlite.rows_read"])
	lines["Durable Objects SQLite storage"] = over(t["core_gb"] + t["aux_gb"], 5) * P["cf.do.sqlite.storage"]

	lookups = (api + git_req + jobs * s["runner_http_per_job"]) * s["d1_miss"]
	lines["D1 directory (rows and storage)"] = (over(lookups * s["d1_rows_per_lookup"] / 1e6, 25_000) * P["cf.d1.rows_read"]
		+ over((meta * 0.05 + pushes * 0.01) / 1e6, 50) * P["cf.d1.rows_written"] + over(1 + devs * 0.001, 5) * P["cf.d1.storage"])

	art_ops = art * (pushes * s["art_ops_push"] + merges * s["art_ops_merge"] + fetches * s["art_ops_fetch"]
		+ jobs * (1 - s["ci_cache_hit"]) * s["art_ops_fetch"]
		+ api * s["content_reads"] * (1 - s["content_cache_hit"]) * s["art_ops_content"])
	lines["Artifacts operations"] = over(art_ops / 1000, 10) * P["cf.artifacts.operations"]
	art_gb = t["git_gb"] * (1 - a["large_repo_share"]) * (1 + s["fork_overhead"])
	lines["Artifacts storage"] = over(art_gb, 1) * P["cf.artifacts.storage"]

	large_gb = t["git_gb"] * a["large_repo_share"]
	r2_gb = (large_gb * s["large_churn"] * s["large_copies"] + t["git_gb"] * s["clone_cache_factor"] + t["actions_gb"]
		+ (art_gb * s["packlog_factor"] if packlog else 0))
	lines["R2 storage (packs, pack log, clone cache, Actions)"] = over(r2_gb, 10) * P["cf.r2.standard.storage"]
	class_a = pushes * (4 if packlog else 1) + merges * 2 + jobs * (1 - s["ci_cache_hit"]) * 0.2 + jobs * 20
	class_b = jobs * s["ci_cache_hit"] + jobs * 20 + fetches * large_share * 0.1
	lines["R2 operations"] = (over(class_a / 1e6, 1) * P["cf.r2.standard.class_a"] + over(class_b / 1e6, 10) * P["cf.r2.standard.class_b"])

	hosts = max(1, math.ceil(large_gb / s["large_host_gb"]))
	lines[f"Containers: Large-tier git hosts ({hosts} x standard-4, warm 220 h a month)"] = container(
		"standard-4", hosts, s["large_hours"], s["large_cpu_active"])
	pool = max(compute_floor, math.ceil(devs / s["compute_per_devs"]))
	lines[f"Containers: git compute pool ({pool} x standard-2, warm 12 h a day)"] = container(
		"standard-2", pool, s["compute_hours"], s["compute_cpu_active"])
	if ssh:
		lines["Containers: SSH fronts (2 x basic, always on)"] = container("basic", 2, HOURS, 0.1)
	served_large = (dev_gb + ci_gb * (1 - s["ci_cache_hit"])) * large_share
	lines["Containers egress (bytes served by Large hosts) [inference]"] = over(served_large, 1024) * P["cf.containers.egress.na_eu"]

	msgs = events * (1 + s["deliveries_per_event"])
	lines["Queues"] = over(msgs * s["queue_ops_per_msg"] / 1e6, 1) * P["cf.queues.operations"]
	steps = devs * 2 * 4.3 * 5 + pushes * 0.001 * 20
	lines["Workflows (consolidation, graduation, recovery)"] = over(steps, 500_000) / 100_000 * P["cf.workflows.steps"]
	lines["Email Sending"] = over(m(a["emails"]), 3000) / 1000 * P["cf.email.sending"]
	if ssh:
		ssh_gb = (dev_gb + push_gb) * s["ssh_share"]
		free = {"pro": 5, "business": 10}.get(plan, 0)
		lines["Spectrum SSH data"] = over(ssh_gb, free) * P["cf.spectrum.ssh.overage"]
	logs_gb = t["logs_gb_day"] * DAYS
	lines["Workers Logs (observability ingest and storage)"] = (over(logs_gb, s["logs_free_gb"]) * P["cf.observability.ingest"]
		+ over(logs_gb * 7 / DAYS, 10) * P["cf.observability.storage"])
	lines["Analytics Engine (traffic counters)"] = over(git_req / 1e6, 10) * P["cf.analytics_engine.writes"]
	audit_gb = events * 1024 / 1e9
	lines["Basin Pipelines and Catalog (audit log)"] = audit_gb * P["cf.basin.pipelines.sink_parquet"]
	return lines


def money(x):
	return f"${x:,.0f}"


def summary():
	print("| Developers | Monthly list cost | Per developer | Largest line |")
	print("| --- | --- | --- | --- |")
	for tier in ("small", "medium", "large"):
		lines = model(tier)
		total = sum(lines.values())
		top = max(lines, key=lines.get)
		print(f"| {TIERS[tier]['devs']:,} | {money(total)} | {money(total / TIERS[tier]['devs'])} | {top} ({money(lines[top])}) |")


def detail(tier):
	lines = model(tier)
	total = sum(lines.values())
	print("| Line item | Per month | Share |")
	print("| --- | --- | --- |")
	for k, v in sorted(lines.items(), key=lambda kv: -kv[1]):
		print(f"| {k} | {money(v)} | {v / total:.0%} |")
	print(f"| Total | {money(total)} | 100% |")


def sensitivity():
	cases = [
		("As designed", {}),
		("Artifacts holds every repository (a 10 GB cap), no Large tier", dict(large_repo_share=0.0, artifacts_share=1.0)),
		("Own engine for every repository: no Artifacts, all in the Large tier", dict(large_repo_share=1.0, artifacts_share=0.0)),
		("Clone cache hit rate 50% instead of 80%", dict(ci_cache_hit=0.5)),
		("SSH carries 20% of developer bytes instead of 50%", dict(ssh_share=0.2)),
		("Large hosts warm all month instead of 220 h", dict(large_hours=730)),
	]
	print("| Case | 50 | 300 | 2,000 |")
	print("| --- | --- | --- | --- |")
	for name, o in cases:
		print(f"| {name} | " + " | ".join(money(sum(model(t, **o).values())) for t in ("small", "medium", "large")) + " |")


def cheapest():
	rows = [
		("as-designed", "Business zone, SSH, pack log, one compute container", model("small")),
		("pro", "Pro zone: SSH with 5 GB included, 100 MB HTTPS push limit", model("small", plan="pro")),
		("https-only", "Free zone, no SSH fronts or Spectrum, 100 MB HTTPS push limit", model("small", plan="free", ssh=False)),
	]
	print("| Config | Configuration | Per month | Per developer |")
	print("| --- | --- | --- | --- |")
	for name, desc, lines in rows:
		total = sum(lines.values())
		print(f"| `{name}` | {desc} | {money(total)} | {money(total / 50)} |")


if __name__ == "__main__":
	args = sys.argv[1:]
	if args[:1] == ["--detail"]:
		detail(args[1] if len(args) > 1 else "small")
	elif args[:1] == ["--sensitivity"]:
		sensitivity()
	elif args[:1] == ["--cheapest"]:
		cheapest()
	else:
		summary()
