#!/usr/bin/env python3
"""Added monthly Cloudflare cost of candidate C's agent layer (agent-native coordination host) on top of the base.

Usage (from the repository root):
	python3 arena/cloudflare-agents/candidate-c/cost_agents.py            # added cost at 50, 300, 2,000 developers
	python3 arena/cloudflare-agents/candidate-c/cost_agents.py --detail   # every line item per tier, A1 and A7 numbers

Method.
1. Base lines move with agent load. The agent workload from arena/cloudflare-agents/TASK.md is folded into a copy of
   ACTIVITY (pushes, API reads, metadata writes, CI jobs), cloudflare/cost.py `model(tier, "design")` runs with and
   without it, and the difference per line is the added cost. Free allowances are therefore applied exactly as the
   base applies them.
2. New components are priced from their first unit, as if the base had already used every free allowance. At 50
   developers that overstates them, because the base leaves most allowances unused (`cost.py --detail small design`).
3. Runner (Sandbox) compute and model compute are excluded, as in the base and the task.
Every per-operation constant below is a [guess] until Milestone 0 measures it.
"""

import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "cloudflare"))
sys.path.insert(0, str(ROOT / "scripts"))
import cost as base  # noqa: E402  (cloudflare/cost.py)
from cost_model import DAYS, HOURS, TIERS, price  # noqa: E402

SECONDS = HOURS * 3600

# ---- agent workload, per developer per day (TASK.md "Agent workload") ----
AGENT = dict(tasks=3, pushes=5, rest=60, clones=2, partial_share=0.5, prs=1, ci_jobs=3, mcp_calls=30, mcp_share=0.5,
	task_minutes=20)

# ---- per-operation guesses for this design ----
REST_WRITES = 10            # of the 60 REST calls per task, the writes (comments, PR edits, labels) [guess]
SESSION_ENTRIES = 2         # SessionOpened and SessionClosed commands on the repository group
SESSION_LOG_PUSHES = 1      # one push of the session log blob to refs/tributary/sessions/<id> per task
SESSION_LOG_KB = 200        # compressed session log per task [guess]
SESSION_RETAIN_MONTHS = 13  # default retention, so A6 works at six months with margin [guess]
EDGE_CALLS_PER_TOOL = 1.5   # edge API reads behind one MCP tool call [guess]
FULL_CLONE_X = 4            # a full agent clone is this many shallow checkouts in bytes; partial is one [guess]
CLONE_VCPU_S = 1.0          # vCPU-s per clone-cache miss (base CI_MISS_VCPU_S)
PREDICT_PAIRS = 4           # open agent branches with overlapping paths, merged against each pushed head [guess]
MERGE_TREE_VCPU_S = 0.2     # one `git merge-tree --write-tree --name-only` on a warm network [guess]
COORD_REQS_PER_TASK = 30    # RepoCoord requests: push and PR events, leases, heartbeats, reads, predictions [guess]
COORD_ROWS_PER_REQ = 2
COORD_WALL_S = 0.005
QUEUE_MSGS_PER_TASK = 9     # coordination fan-out from chain cursors: 5 pushes, PR open, session open and close, CI
TRIB_RUN_SHARE = 0.5        # share of tasks that are Tributary-run (Workflows, TaskAgent DO, Sandbox) [guess]
WORKFLOW_STEPS = 16         # provision, clone, run, push, PR, gates, close [guess]
TASK_ALARMS = 20            # TaskAgent heartbeat alarm every 60 s for a 20-minute task
OUTBOUND_CALLS = 230        # sandbox egress through the outbound handler: git, REST, about 150 model calls [guess]
OUTBOUND_CPU_MS = 2
BUDGET_REQS_PER_TASK = 20   # OrgBudget increments, batched every 60 s per task, plus reads [guess]
MCP_OAUTH_REQS = 5          # protected-resource and AS metadata, CIMD fetch, authorize, token, per MCP session
MCP_CPU_MS = 5
MCP_TOKEN_ROWS = 7          # one Tokens entry per MCP session: log row plus fold rows, on three members
WORKDAY_H = 8              # agent tasks run inside an 8-hour working day [guess]
PEAK_FACTOR = 2            # peak concurrency over the working-day average [guess]
HOT_NETWORK_SHARE = 0.5    # share of an install's agents working on its largest network [guess]
SHARD_UTIL = 0.7           # target utilization of a shard's vCPU; a standard-4 then carries 2.8 vCPU-s per second
WARM_SITES = 2


def home_shard_vcpu(agents):
	"""vCPU-s per second on a network's home-site shards from `agents` concurrent agents (A1 model, scaled)."""
	push_rate = agents / 120 + agents / (AGENT["task_minutes"] * 60)       # branch pushes plus session-log pushes
	return (push_rate * base.PUSH_VCPU_S + agents / 120 * CLONE_VCPU_S
		+ agents / (AGENT["task_minutes"] * 60) * AGENT["clones"] * (1 - base.CI_HIT) * CLONE_VCPU_S)


def hot_extra(tier):
	"""Extra shards per warm site for the tier's largest network. Prediction pauses during a site loss, so each warm
	site is sized for the home work only, and prediction uses the second warm site's spare capacity."""
	tasks_day = TIERS[tier]["devs"] * AGENT["tasks"]
	concurrent = tasks_day * AGENT["task_minutes"] / (WORKDAY_H * 60) * PEAK_FACTOR * HOT_NETWORK_SHARE
	per_shard = base.INSTANCE[tier][1] * SHARD_UTIL
	return max(0, math.ceil(home_shard_vcpu(concurrent) / per_shard) - 1), concurrent


def agent_activity():
	"""The base ACTIVITY plus the agent workload, in the base's own units."""
	a, g = dict(base.ACTIVITY), AGENT
	t = g["tasks"]
	a["pushes"] += t * (g["pushes"] + SESSION_LOG_PUSHES)
	a["api_requests"] += t * (g["rest"] - REST_WRITES + g["mcp_calls"] * g["mcp_share"] * EDGE_CALLS_PER_TOOL)
	a["metadata_writes"] += t * (g["prs"] + REST_WRITES + SESSION_ENTRIES)
	a["ci_jobs"] += t * g["ci_jobs"]
	return a


def base_delta(tier):
	"""Per-line difference of cloudflare/cost.py with and without the agent workload."""
	before = base.model(tier, "design")
	saved = base.ACTIVITY
	base.ACTIVITY = agent_activity()
	try:
		after = base.model(tier, "design")
	finally:
		base.ACTIVITY = saved
	d = {f"Base: {k}": after[k] - before.get(k, 0.0) for k in after}
	# Agents push and fetch over HTTPS (the Sandbox outbound handler, vendor installation tokens), never Spectrum SSH.
	# The base applies SSH_SHARE to every push, so its Spectrum delta is removed here. The container egress delta still
	# carries that share of agent push bytes, which overstates it slightly.
	d["Base: Spectrum SSH data"] = 0.0
	return d


def new_lines(tier):
	t = TIERS[tier]
	tasks = t["devs"] * AGENT["tasks"] * DAYS
	pushes = tasks * AGENT["pushes"]
	mcp_sessions = tasks * AGENT["mcp_share"]
	mcp_calls = mcp_sessions * AGENT["mcp_calls"]
	trib = tasks * TRIB_RUN_SHARE
	L = {}

	# Agent clones: half partial (one shallow checkout of bytes), half full. Hits come from the R2 clone cache.
	clones = tasks * AGENT["clones"]
	clone_mb = t["checkout_mb"] * (AGENT["partial_share"] + (1 - AGENT["partial_share"]) * FULL_CLONE_X)
	miss = clones * (1 - base.CI_HIT)
	L["Agent clones: container egress on clone-cache misses"] = miss * clone_mb / 1024 * price(
		"cf.containers.egress.na_eu", "north-america-europe")
	L["Agent clones: container vCPU on misses"] = miss * CLONE_VCPU_S * price("cf.containers.vcpu")
	L["Agent clones: R2 Class B on hits, Workers requests"] = (clones * base.CI_HIT / 1e6 * price("cf.r2.standard.class_b")
		+ clones * base.GIT_REQS / 1e6 * price("cf.workers.requests"))

	# Hot networks get extra git cache shards on both warm sites (base change, A1), sized from peak concurrency.
	name, vcpu, gib, disk = base.INSTANCE[tier]
	n = hot_extra(tier)[0] * WARM_SITES
	L["Hot-network shards (extra warm shards on both warm sites)"] = n * SECONDS * (gib * price("cf.containers.memory")
		+ disk * price("cf.containers.disk"))

	# Conflict prediction runs on the network's non-home warm shard, active vCPU only.
	L["Conflict prediction (merge-tree on the second warm site)"] = (pushes * PREDICT_PAIRS * MERGE_TREE_VCPU_S
		* price("cf.containers.vcpu"))

	# RepoCoord Durable Objects (leases, activity feed, predictions) in the trib-agents script.
	coord_reqs = tasks * COORD_REQS_PER_TASK
	L["RepoCoord objects (requests, rows, duration)"] = (coord_reqs / 1e6 * price("cf.do.requests")
		+ coord_reqs * COORD_ROWS_PER_REQ / 1e6 * price("cf.do.sqlite.rows_written")
		+ coord_reqs * COORD_WALL_S * base.DO_GB / 1e6 * price("cf.do.duration")
		+ coord_reqs / 1e6 * price("cf.workers.requests"))
	L["Coordination fan-out (Queues)"] = tasks * QUEUE_MSGS_PER_TASK * 3 / 1e6 * price("cf.queues.operations")

	# Tributary-run tasks: Workflows, TaskAgent object alarms, outbound handler, budgets. Sandbox compute excluded.
	# Agent Sandboxes run in their own agent account so a fleet cannot take the runner account's vCPU from Actions.
	L["Agent account (Workers Paid; Sandbox compute excluded)"] = price("cf.workers.paid.subscription")
	L["Task Workflows (steps)"] = trib * WORKFLOW_STEPS / 1e5 * price("cf.workflows.steps")
	L["TaskAgent objects (alarms, budget counters)"] = trib * (TASK_ALARMS + BUDGET_REQS_PER_TASK) * (
		price("cf.do.requests") + price("cf.do.sqlite.rows_written")) / 1e6
	L["Sandbox outbound handler (requests, CPU)"] = trib * OUTBOUND_CALLS * (price("cf.workers.requests")
		+ OUTBOUND_CPU_MS * price("cf.workers.cpu")) / 1e6

	# Remote MCP endpoint and its OAuth, tokens kept in the Tokens groups (no KV).
	L["Remote MCP endpoint (requests, CPU, token entries)"] = ((mcp_calls + mcp_sessions * MCP_OAUTH_REQS)
		* price("cf.workers.requests") / 1e6 + mcp_calls * MCP_CPU_MS * price("cf.workers.cpu") / 1e6
		+ mcp_sessions * (3 * price("cf.do.requests") + MCP_TOKEN_ROWS * price("cf.do.sqlite.rows_written")) / 1e6)

	# Session logs: git blobs under a hidden ref, so packs carry them (three R2 copies plus the vault copy).
	log_gb_month = tasks * SESSION_LOG_KB / 1024 / 1024
	L["Session logs (steady-state R2, 3 copies plus vault)"] = (log_gb_month * SESSION_RETAIN_MONTHS * 4
		* price("cf.r2.standard.storage"))
	return L


def lines(tier):
	out = base_delta(tier)
	out.update(new_lines(tier))
	return out


def money(x):
	return f"${x:,.0f}"


def a1():
	"""Journey A1 rates on one monorepo group, against the base's limits."""
	agents, interval = 500, 120
	task_s = AGENT["task_minutes"] * 60
	turnover = agents / task_s
	r = {}
	r["agent pushes"] = agents / interval
	r["PrSync (head moved on an open draft PR)"] = r["agent pushes"]
	r["draft PR opens"] = turnover * AGENT["prs"]
	r["agent REST writes"] = turnover * REST_WRITES
	r["check-run writes (3 per CI job)"] = turnover * AGENT["ci_jobs"] * 3
	r["session open, close, and log push"] = turnover * (SESSION_ENTRIES + SESSION_LOG_PUSHES)
	r["merge-queue merges (base, 10/min)"] = 10 / 60
	r["bot ref moves (base, 50/min)"] = 50 / 60
	cmds = sum(r.values())
	reads = {"CI ref reads (2,000 clones/min)": 2000 / 60, "human API reads (base guess)": 200,
		"agent REST reads": turnover * (AGENT["rest"] - REST_WRITES),
		"MCP-driven reads": agents * AGENT["mcp_share"] * AGENT["mcp_calls"] / task_s * EDGE_CALLS_PER_TOOL,
		"agent clones": turnover * AGENT["clones"], "push ref reads": r["agent pushes"],
		"follower read-index (base cap)": 100}
	leader = cmds + sum(reads.values())
	new_tips = r["agent pushes"]
	shard = {"push validation": (r["agent pushes"] + turnover) * base.PUSH_VCPU_S,
		"clone-cache misses (one per new tip)": new_tips * CLONE_VCPU_S,
		"agent clone misses": turnover * AGENT["clones"] * (1 - base.CI_HIT) * CLONE_VCPU_S,
		"merges": 10 / 60 * base.MERGE_VCPU_S}
	predict = r["agent pushes"] * PREDICT_PAIRS * MERGE_TREE_VCPU_S
	coord = agents * AGENT["mcp_share"] * AGENT["mcp_calls"] / task_s * 0.3 + r["agent pushes"] * 2 + agents / 300 + turnover * 4
	print("| A1 write command | per second |")
	print("| --- | --- |")
	for k, v in r.items():
		print(f"| {k} | {v:.2f} |")
	print(f"| total commands on the group | {cmds:.1f} (gate 300/s: {300 / cmds:.1f}x headroom) |")
	human = r["merge-queue merges (base, 10/min)"] + r["bot ref moves (base, 50/min)"]
	x10 = (cmds - human) * 10 + human
	print(f"| same mix at 5,000 agents (agent commands x10) | {x10:.0f} ({x10 / 300:.0%} of the gate) |")
	print(f"| inline pack bytes | {(r['agent pushes'] + turnover) * base.ACTIVITY['inline_push_kb']:.0f} KB/s against 15,000 KB/s (10 entries/s x 1.5 MB) |")
	print()
	print("| A1 leader request | per second |")
	print("| --- | --- |")
	for k, v in reads.items():
		print(f"| {k} | {v:.1f} |")
	print(f"| leader total, with commands | {leader:.0f} of 1,000 ({1000 / leader:.1f}x headroom) |")
	print()
	print("| A1 home-site shard work | vCPU-s per second |")
	print("| --- | --- |")
	for k, v in shard.items():
		print(f"| {k} | {v:.2f} |")
	tot = sum(shard.values())
	print(f"| total on one standard-4 (4 vCPU) | {tot:.2f} ({tot / 4:.0%}); at 70% target needs {math.ceil(tot / 2.8)} shards |")
	print(f"| conflict prediction, second warm site | {predict:.2f} ({math.ceil(predict / 2.8)} shards at 70%) |")
	print(f"| after a site loss (prediction paused) | {tot:.2f} on the surviving warm site, the same {math.ceil(tot / 2.8)} shards |")
	print(f"| RepoCoord requests | {coord:.1f} of 1,000 ({1000 / coord:.0f}x headroom) |")


def a7():
	calls = AGENT["mcp_calls"]
	usd = ((calls + MCP_OAUTH_REQS) * price("cf.workers.requests") + calls * MCP_CPU_MS * price("cf.workers.cpu")
		+ calls * EDGE_CALLS_PER_TOOL * (1.5 * price("cf.do.requests") + base.CPU_MS["api"] * price("cf.workers.cpu"))
		+ 3 * price("cf.do.requests") + MCP_TOKEN_ROWS * price("cf.do.sqlite.rows_written")) / 1e6
	print(f"A7: one MCP session of {calls} tool calls costs about ${usd:.6f} at list prices "
		f"(${usd * 1e6:,.0f} per million sessions).")


def summary(detail=False):
	tiers = list(TIERS)
	all_lines = {tier: lines(tier) for tier in tiers}
	keys = list(all_lines[tiers[0]].keys())
	print("| Line item | " + " | ".join(f"{TIERS[x]['devs']:,} devs" for x in tiers) + " |")
	print("| --- |" + " --- |" * len(tiers))
	for k in keys:
		vals = [all_lines[x][k] for x in tiers]
		if not detail and max(vals) < 0.5:
			continue
		print(f"| {k} | " + " | ".join(money(v) for v in vals) + " |")
	totals = [sum(all_lines[x].values()) for x in tiers]
	base_totals = [sum(base.model(x, "design").values()) for x in tiers]
	print("| **Added per month** | " + " | ".join(f"**{money(v)}**" for v in totals) + " |")
	print("| Base per month (`cloudflare/cost.py`) | " + " | ".join(money(v) for v in base_totals) + " |")
	print("| Added share of base | " + " | ".join(f"{a / b:.0%}" for a, b in zip(totals, base_totals)) + " |")
	print("| Added per agent task | " + " | ".join(
		f"${v / (TIERS[x]['devs'] * AGENT['tasks'] * DAYS):.4f}" for v, x in zip(totals, tiers)) + " |")
	print()
	print("| Developers | Peak concurrent agents on the largest network | Home-shard vCPU-s/s | Extra shards per warm site |")
	print("| --- | --- | --- | --- |")
	for x in tiers:
		extra, conc = hot_extra(x)
		print(f"| {TIERS[x]['devs']:,} | {conc:.0f} | {home_shard_vcpu(conc):.2f} | {extra} {base.INSTANCE[x][0]} |")
	print()
	a7()
	if detail:
		print()
		a1()


if __name__ == "__main__":
	summary(detail="--detail" in sys.argv[1:])
