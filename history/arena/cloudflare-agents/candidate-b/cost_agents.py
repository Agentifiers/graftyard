#!/usr/bin/env python3
"""Added monthly cost of the agent layer, candidate B (Artifacts as the agent workspace tier).

Usage, from the repository root:
	python3 arena/cloudflare-agents/candidate-b/cost_agents.py              # totals, line items, A1 rates
	python3 arena/cloudflare-agents/candidate-b/cost_agents.py --detail large workspace

Install-side load is priced by running `cloudflare/cost.py` twice, once with `ACTIVITY` as given and once with the
agent workload added to it, and taking the difference. Agent-account lines (Artifacts, Workflows, TaskAgent and
coordinator Durable Objects, the baseline seeder container, session transcripts) are priced here with the `cf.*`
prices that `cloudflare/cost.py` reads. Runner and model compute (agent Sandboxes, CI runners, model tokens) are
excluded, as in the base. Every per-task constant below is a [guess] until Milestone 0, except the workload numbers
from `arena/cloudflare-agents/TASK.md`.

Variants:
	workspace   the design: Tributary-run agents work in per-task Artifacts forks; a Lander pushes into Tributary
	            every LAND_EVERY_MIN minutes and at the end; clones go to Artifacts.
	direct      the fallback and the no-Artifacts install: the same agents push and clone against Tributary itself.
"""

import contextlib
import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
_spec = importlib.util.spec_from_file_location("cf_cost", ROOT / "cloudflare" / "cost.py")
cf = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cf)
TIERS, ACTIVITY, DAYS, price, over = cf.TIERS, cf.ACTIVITY, cf.DAYS, cf.price, cf.over

# ---- agent workload, per developer per day (TASK.md) ----
TASKS = 3
PER_TASK = dict(pushes=5, rest=60, clones=2, partial_share=0.5, prs=1, ci_jobs=3, mcp_calls=30, mcp_share=0.5,
	minutes=20)

# ---- design constants [guess] ----
LAND_EVERY_MIN = 10                 # Lander checkpoint cadence; a 20-minute task lands twice
LANDS_PER_TASK = PER_TASK["minutes"] // LAND_EVERY_MIN
REST_PER_MCP_CALL = 1.5             # edge REST calls behind one MCP tool call
OVERLAP_CHECKS_PER_UPDATE = 1       # coordinator check-run write per landed update (debounced in direct mode)
BASELINE_SYNCS_PER_DEV_DAY = 1.0    # default-branch moves per developer per day, coalesced to one sync a minute
FORK_GB = 0.3                       # average fork size; forks assumed to copy objects (artifacts-recheck, s.3)
FORK_ALIVE_H = PER_TASK["minutes"] / 60 + 1.0   # task plus one hour of grace after the last land
BASELINE_SHARE = 0.3 * 0.6          # share of git_gb held as baselines: repos with agent work, cut by cap and snapshot
COUNT_READ_OPS = True               # worst case: readFile, log, and token mints bill as Artifacts operations
READFILE_PER_TASK = 10
LOG_PER_EVENT = 1
SEEDER = dict(vcpu=0.25, gib=1, disk_gb=4, active_s=30)  # `basic` container per baseline sync
TRANSCRIPT_MB = 1.0                 # session transcript per task, kept 12 months (steady state)
TRANSCRIPT_MONTHS = 12
STEPS = dict(event=2, land=6, sync=4, task=4)          # Workflow steps
AGENT_REQS_PER_TASK = dict(task_rpc=25, model_proxy=100, events=PER_TASK["pushes"])
AGENT_DO = dict(task_reqs=30, coord_reqs=10, wall_s=0.02, rows=60, kb=50)
LANDER_CPU_MS = 200

ART_OPS_FREE, ART_GB_FREE = 10_000, 1
ACCOUNT_FREE = cf.INCLUDED


def tasks_month(devs):
	return devs * TASKS * DAYS


def install_delta(variant):
	"""Per developer per day additions to cloudflare/cost.py ACTIVITY for the install account."""
	t = PER_TASK
	mcp_rest = TASKS * t["mcp_share"] * t["mcp_calls"] * REST_PER_MCP_CALL
	d = dict(api_requests=TASKS * t["rest"] + mcp_rest, metadata_writes=TASKS * t["prs"])
	if variant == "direct":
		d["pushes"] = TASKS * t["pushes"]
		d["ci_jobs"] = TASKS * (t["ci_jobs"] + t["clones"])   # agent clones priced like CI checkouts (clone cache)
		d["metadata_writes"] += TASKS * t["pushes"] * OVERLAP_CHECKS_PER_UPDATE * 0.4  # debounced to 2 per task
	else:
		d["pushes"] = TASKS * LANDS_PER_TASK
		d["ci_jobs"] = TASKS * t["ci_jobs"]
		d["dev_fetches"] = BASELINE_SYNCS_PER_DEV_DAY          # the seeder fetches each default-branch move
		d["metadata_writes"] += TASKS * LANDS_PER_TASK * OVERLAP_CHECKS_PER_UPDATE
	return d


@contextlib.contextmanager
def activity_plus(delta):
	saved = dict(ACTIVITY)
	for k, v in delta.items():
		ACTIVITY[k] = ACTIVITY[k] + v
	try:
		yield
	finally:
		ACTIVITY.clear()
		ACTIVITY.update(saved)


def install_lines(tier, variant):
	base = cf.model(tier, "design")
	with activity_plus(install_delta(variant)):
		added = cf.model(tier, "design")
	out = {}
	for k in added:
		diff = added[k] - base.get(k, 0.0)
		if abs(diff) >= 0.005:
			out[f"Install: {k}"] = diff
	return out


def artifacts_ops_per_task():
	t = PER_TASK
	ops = 1 + t["clones"] + t["pushes"] + LANDS_PER_TASK + 1      # fork, clones, pushes, Lander fetches, delete
	if COUNT_READ_OPS:
		ops += 1 + READFILE_PER_TASK + (t["pushes"] + LANDS_PER_TASK) * LOG_PER_EVENT   # token mint, readFile, log
	return ops


def agent_lines(tier, variant):
	t = TIERS[tier]
	devs = t["devs"]
	n = tasks_month(devs)
	syncs = devs * BASELINE_SYNCS_PER_DEV_DAY * DAYS
	lines = {"Agent account: Workers Paid subscription": price("cf.workers.paid.subscription")}

	# Workers requests and CPU: TaskAgent RPCs, the Sandbox outbound handler (git, MCP, model proxy), event intake.
	git_proxy = (PER_TASK["clones"] + PER_TASK["pushes"]) * cf.GIT_REQS
	mcp = PER_TASK["mcp_share"] * PER_TASK["mcp_calls"]
	reqs = n * (sum(AGENT_REQS_PER_TASK.values()) + git_proxy + mcp)
	cpu = reqs * 2
	if variant == "workspace":
		reqs += n * LANDS_PER_TASK * 4 + syncs * 3
		cpu += n * LANDS_PER_TASK * LANDER_CPU_MS
	lines["Agent account: Workers requests"] = over(reqs, ACCOUNT_FREE["workers_req"]) / 1e6 * price("cf.workers.requests")
	lines["Agent account: Workers CPU"] = over(cpu, ACCOUNT_FREE["workers_cpu"]) / 1e6 * price("cf.workers.cpu")

	# Durable Objects: one TaskAgent per task, one coordinator per active repository. Hibernated between events.
	do_reqs = n * (AGENT_DO["task_reqs"] + AGENT_DO["coord_reqs"])
	gbs = do_reqs * AGENT_DO["wall_s"] * cf.DO_GB
	rows = n * AGENT_DO["rows"]
	do_gb = n * AGENT_DO["kb"] / 1024 / 1024           # task rows kept one month
	lines["Agent account: Durable Objects (TaskAgent, coordinator)"] = (
		over(do_reqs, ACCOUNT_FREE["do_req"]) / 1e6 * price("cf.do.requests")
		+ over(gbs, ACCOUNT_FREE["do_gbs"]) / 1e6 * price("cf.do.duration")
		+ over(rows, ACCOUNT_FREE["do_rows_w"]) / 1e6 * price("cf.do.sqlite.rows_written")
		+ over(do_gb, ACCOUNT_FREE["do_storage"]) * price("cf.do.sqlite.storage"))

	# Workflows: one step pair per Artifacts push event; Lander and baseline sync runs; task start and end.
	steps = n * STEPS["task"]
	if variant == "workspace":
		steps += n * (PER_TASK["pushes"] * STEPS["event"] + LANDS_PER_TASK * STEPS["land"]) + syncs * STEPS["sync"]
	lines["Agent account: Workflows steps"] = steps / 1e5 * price("cf.workflows.steps")

	# Session transcripts in R2, twelve months retained at steady state.
	tr_gb = n * TRANSCRIPT_MONTHS * TRANSCRIPT_MB / 1024
	lines["Agent account: R2 session transcripts (12 months)"] = (over(tr_gb, ACCOUNT_FREE["r2_storage"])
		* price("cf.r2.standard.storage") + over(n, ACCOUNT_FREE["r2_a"]) / 1e6 * price("cf.r2.standard.class_a"))

	if variant == "workspace":
		ops = n * artifacts_ops_per_task() + syncs
		lines["Artifacts operations"] = over(ops, ART_OPS_FREE) / 1000 * price("cf.artifacts.operations")
		base_gb = t["git_gb"] * BASELINE_SHARE
		fork_gb = n * FORK_GB * FORK_ALIVE_H / cf.HOURS
		lines["Artifacts storage: baselines"] = over(base_gb, ART_GB_FREE) * price("cf.artifacts.storage")
		lines["Artifacts storage: per-task forks (copy assumed)"] = fork_gb * price("cf.artifacts.storage")
		s = SEEDER
		sec = syncs * s["active_s"]
		lines["Agent account: baseline seeder container"] = (
			over(sec * s["gib"], ACCOUNT_FREE["c_mem_gibs"]) * price("cf.containers.memory")
			+ over(sec * s["vcpu"], ACCOUNT_FREE["c_vcpu_s"]) * price("cf.containers.vcpu")
			+ over(sec * s["disk_gb"], ACCOUNT_FREE["c_disk_gbs"]) * price("cf.containers.disk"))
	return lines


def model(tier, variant="workspace"):
	lines = install_lines(tier, variant)
	lines.update(agent_lines(tier, variant))
	return lines


def money(x):
	return f"${x:,.0f}"


def a1():
	"""Journey A1 per-repository rates on the monorepo, added by the agent fleet. Base figures from DESIGN.md."""
	n, push_s, task_s = 500, 120, PER_TASK["minutes"] * 60
	starts = n / task_s
	rows = []
	for variant in ("direct", "workspace"):
		pushes = n / push_s if variant == "direct" else n / (LAND_EVERY_MIN * 60)
		ckpt = pushes * cf.ENTRY_PER_PUSH
		checks = (n / 300) if variant == "direct" else pushes            # overlap check runs, debounced 5 min
		jobs = starts * PER_TASK["ci_jobs"]
		cmds = ckpt + starts * PER_TASK["prs"] + jobs * cf.ENTRY_PER_JOB_REPO + checks
		rest = starts * PER_TASK["rest"] + starts * PER_TASK["mcp_share"] * PER_TASK["mcp_calls"] * REST_PER_MCP_CALL
		clones = starts * PER_TASK["clones"] if variant == "direct" else 1 / 60
		leader = pushes + rest + clones + jobs + checks
		vcpu = (pushes * cf.PUSH_VCPU_S + clones * 0.3 * cf.CI_MISS_VCPU_S + jobs * 0.05 * cf.CI_MISS_VCPU_S
			+ checks * 0.3)
		rows.append((variant, n / push_s, pushes, cmds, leader, vcpu, starts))
	return rows


def summary():
	global COUNT_READ_OPS
	print("Added monthly cost of the agent layer (runner and model compute excluded)")
	print()
	print("| Developers | Tasks per month | workspace (design) | direct (no Artifacts) | Artifacts premium |")
	print("| --- | --- | --- | --- | --- |")
	for tier in TIERS:
		w, d = sum(model(tier, "workspace").values()), sum(model(tier, "direct").values())
		print(f"| {TIERS[tier]['devs']:,} | {tasks_month(TIERS[tier]['devs']):,.0f} | {money(w)} | {money(d)} | {money(w - d)} |")
	print()
	for tier in TIERS:
		detail(tier, "workspace")
		print()
	print("Journey A1, one monorepo, 500 agents, added load (base: ~2 entries/s, ~250 leader req/s, ~2 shard vCPU)")
	print()
	print("| Mode | Agent pushes/s | Pushes into Tributary/s | Group commands/s | Leader req/s | Shard vCPU | Task starts/s |")
	print("| --- | --- | --- | --- | --- | --- | --- |")
	for v, ap, p, c, l, cpu, s in a1():
		print(f"| {v} | {ap:.2f} | {p:.2f} | {c:.2f} | {l:.1f} | {cpu:.2f} | {s:.2f} |")
	print()
	print(f"Artifacts operations per task: {artifacts_ops_per_task()} (COUNT_READ_OPS={COUNT_READ_OPS})")
	saved, COUNT_READ_OPS = COUNT_READ_OPS, False
	cheap = [sum(model(tier, "workspace").values()) for tier in TIERS]
	print(f"Sensitivity, only fork, clone, push, Lander fetch, and delete billed ({artifacts_ops_per_task()} ops per "
		"task): workspace total " + ", ".join(money(c) for c in cheap) + " at 50, 300, 2,000 developers")
	COUNT_READ_OPS = saved


def detail(tier, variant):
	lines = model(tier, variant)
	total = sum(lines.values())
	print(f"| Line item ({TIERS[tier]['devs']:,} developers, {variant}) | Per month |")
	print("| --- | --- |")
	for k, v in sorted(lines.items(), key=lambda kv: -kv[1]):
		print(f"| {k} | {money(v)} |")
	print(f"| Total | {money(total)} |")


if __name__ == "__main__":
	args = sys.argv[1:]
	if args[:1] == ["--detail"]:
		detail(args[1], args[2])
	else:
		summary()
