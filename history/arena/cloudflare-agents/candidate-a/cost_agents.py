#!/usr/bin/env python3
"""Added monthly Cloudflare cost of the agent layer in candidate A (GHES-native, subtract first).

Usage, from the repository root:
	python3 arena/cloudflare-agents/candidate-a/cost_agents.py          # line items at 50, 300, 2,000 developers
	python3 arena/cloudflare-agents/candidate-a/cost_agents.py --a1     # also the journey A1 per-object rates

Method. The base cost model (`cloudflare/cost.py`, default `design` variant) is run twice per tier: once as is, and once
with the agent workload added to its ACTIVITY. The difference is what agents add to the base's own lines (Workers,
Durable Objects, R2, Containers, Queues, logs, email). The new components (the remote MCP endpoint `trib-mcp`, the
agents account for Tributary-run agents, session records, path-set rows, agent push bytes) are priced on top.
Runner (Sandbox) compute and model compute are excluded, as in the base. Every per-operation constant here is a
[guess] until Milestone 0 measures it.

Agent workload (TASK.md): per developer per day, 3 agent tasks. Each task makes 5 pushes to its own branch, 60 REST
calls, 2 clones (half partial), 1 draft pull request, 3 CI jobs, and 30 MCP tool calls if the agent uses MCP (half do).
A task runs 20 minutes.
"""

import importlib.util
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
_spec = importlib.util.spec_from_file_location("cf_cost", ROOT / "cloudflare" / "cost.py")
cf = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cf)
price, DAYS, SECONDS, TIERS = cf.price, cf.DAYS, cf.SECONDS, cf.TIERS

# ---- agent workload (TASK.md) ----
TASKS = 3                  # agent tasks per developer per day
PUSHES, REST, CLONES, PRS, CI_JOBS = 5, 60, 2, 1, 3
MCP_CALLS, MCP_SHARE = 30, 0.5
TASK_MIN = 20

# ---- per-operation guesses ----
REST_WRITES = 9            # of the 60 REST calls, writes that become chain commands (comments, edits, labels) [guess]
PRSYNC_EXTRA = 0.5         # PrSync commands per agent push beyond the base's 0.3 (4 of 5 pushes hit an open PR) [guess]
REST_PER_MCP = 1.5         # REST calls trib-mcp makes per MCP tool call [guess]
MCP_AUTH_REQS = 8          # per MCP session: PRM, AS metadata, CIMD fetch, authorize, callback, token, scoped token, refresh
MCP_CPU_MS = 3             # trib-mcp CPU per tool call (JSON-RPC, schema check, trust view) [guess]
EMAILS_PER_TASK = 1        # review-request notification per agent PR [guess]
CLONE_HIT = cf.CI_HIT      # agent clones are shallow or blobless at a moving tip, so they hit the clone cache like CI
TRIB_RUN_SHARE = 0.5       # share of agent tasks that are Tributary-run (the rest are third-party apps) [guess]
OUTBOUND_PER_TASK = 60 + 2 * PUSHES + 3 * CLONES + 100   # REST, git, and model calls through the outbound handler [guess]
WEBHOOKS_PER_TASK = 15     # deliveries from the install to the controller per Tributary-run task [guess]
STEPS_PER_TASK = 15        # Workflow steps per task: mint, start, 4 heartbeats, push, PR, checks, finish [guess]
SESSION_MB = 2             # session record (plan, tool calls, transcript) per Tributary-run task [guess]
SESSION_MONTHS = 12        # retention, so steady state holds 12 months of records [guess]
SESSION_COPIES = 2         # two buckets in two regions, like packs
PATH_ROWS_PER_PUSH = 1     # one changed-path-set row per pull request, rewritten by PrSync, per member
MEMBERS = 3
PUSH_GC_DAYS = 14          # agent branch bytes kept until GC drops them after merge or close
DYN_INCLUDED = 1000        # unique Dynamic Workers per month included
CODEMODE_CALLS = MCP_CALLS  # opt-in Code Mode: every tool call is a search() or execute(), each a new Dynamic Worker

AGENT_INCLUDED = dict(workers_req=10e6, workers_cpu=30e6, steps=500_000, r2_storage=10, r2_a=1e6)


def over(used, included):
	return max(0.0, used - included)


def per_month(devs, per_dev_day):
	return devs * per_dev_day * DAYS


def base_lines(tier, activity=None, logs_factor=1.0, ssh_share=None):
	"""Run the base model with a patched ACTIVITY, logs volume, and SSH share, then restore them."""
	saved = (cf.ACTIVITY, cf.TIERS, cf.SSH_SHARE)
	try:
		if activity is not None:
			cf.ACTIVITY = activity
		tiers = {k: dict(v) for k, v in saved[1].items()}
		tiers[tier]["logs_gb_day"] *= logs_factor
		cf.TIERS = tiers
		if ssh_share is not None:
			cf.SSH_SHARE = ssh_share
		return cf.model(tier, "design")
	finally:
		cf.ACTIVITY, cf.TIERS, cf.SSH_SHARE = saved


def edge_requests_per_dev_day(a):
	return a["api_requests"] + (a["pushes"] + a["dev_fetches"] + a["ci_jobs"]) * cf.GIT_REQS


def agent_activity(tier, with_mcp):
	"""ACTIVITY with the agent workload added. Clones ride as fetches whose bytes are only the clone-cache misses."""
	a0, t = cf.ACTIVITY, TIERS[tier]
	a = dict(a0)
	a["pushes"] += TASKS * PUSHES
	a["ci_jobs"] += TASKS * CI_JOBS
	a["api_requests"] += TASKS * REST
	a["metadata_writes"] += TASKS * (PRS + REST_WRITES) + TASKS * PUSHES * PRSYNC_EXTRA
	a["emails"] += TASKS * EMAILS_PER_TASK
	clones = TASKS * CLONES
	clone_mb_out = t["checkout_mb"] * (1 - CLONE_HIT)
	a["dev_fetch_mb"] = (a0["dev_fetches"] * a0["dev_fetch_mb"] + clones * clone_mb_out) / (a0["dev_fetches"] + clones)
	a["dev_fetches"] += clones
	if with_mcp:
		a["api_requests"] += TASKS * MCP_SHARE * MCP_CALLS * (1 + REST_PER_MCP)
	# Agents push and clone over HTTPS only, so hold the base's SSH bytes constant.
	base_bytes = a0["dev_fetches"] * a0["dev_fetch_mb"] + a0["pushes"] * a0["push_mb"]
	new_bytes = a["dev_fetches"] * a["dev_fetch_mb"] + a["pushes"] * a["push_mb"]
	ssh_share = cf.SSH_SHARE * base_bytes / new_bytes
	logs_factor = edge_requests_per_dev_day(a) / edge_requests_per_dev_day(a0)
	return a, ssh_share, logs_factor


def lines(tier):
	devs = TIERS[tier]["devs"]
	l0 = base_lines(tier)
	a1, s1, f1 = agent_activity(tier, with_mcp=False)
	l1 = base_lines(tier, a1, f1, s1)
	a2, s2, f2 = agent_activity(tier, with_mcp=True)
	l2 = base_lines(tier, a2, f2, s2)
	out = {}

	# Base lines that agent git, REST, and CI traffic grows, largest first.
	diff = {k: l1[k] - l0[k] for k in l0}
	for k, v in sorted(diff.items(), key=lambda kv: -kv[1]):
		if abs(v) >= 0.5:
			out[f"Base: {k}"] = v
	rest = sum(v for v in diff.values() if abs(v) < 0.5)
	out["Base: other lines (each under $0.50)"] = rest

	# trib-mcp: MCP requests and the REST behind them, plus session auth requests and scoped-token entries.
	sessions = per_month(devs, TASKS * MCP_SHARE)
	auth_reqs = sessions * MCP_AUTH_REQS
	auth_rows = sessions * MEMBERS * (1 + cf.FOLD_ROWS)   # one scoped ghu_ token hash per session on a Tokens group
	out["trib-mcp: tool calls and REST behind them (base lines)"] = sum(l2.values()) - sum(l1.values())
	out["trib-mcp: CPU, session auth, scoped-token entries"] = (
		per_month(devs, TASKS * MCP_SHARE * MCP_CALLS) * MCP_CPU_MS / 1e6 * price("cf.workers.cpu")
		+ auth_reqs / 1e6 * price("cf.workers.requests")
		+ auth_rows / 1e6 * price("cf.do.sqlite.rows_written"))

	# Changed-path sets for overlap reads, one row per agent push per member (fold rows on the repository group).
	out["Folds: changed-path-set rows for overlap reads"] = per_month(devs, TASKS * PUSHES) * PATH_ROWS_PER_PUSH * MEMBERS / 1e6 * price(
		"cf.do.sqlite.rows_written")

	# Pack bytes from agent branches until GC: three site copies plus the vault's locked copy.
	push_gb = devs * TASKS * PUSHES * cf.ACTIVITY["push_mb"] * PUSH_GC_DAYS / 1024
	out["R2: agent branch packs until GC (3 copies plus vault)"] = push_gb * (3 + 1) * price("cf.r2.standard.storage")

	# Agents account for Tributary-run agents: its own plan and allowances. Sandbox compute and models excluded.
	trib_tasks = per_month(devs, TASKS * TRIB_RUN_SHARE)
	reqs = trib_tasks * (OUTBOUND_PER_TASK + WEBHOOKS_PER_TASK + STEPS_PER_TASK)
	cpu = trib_tasks * (OUTBOUND_PER_TASK * 1 + WEBHOOKS_PER_TASK * 2 + STEPS_PER_TASK * 2)
	session_gb = trib_tasks * SESSION_MB / 1024 * SESSION_MONTHS * SESSION_COPIES
	out["Agents account: Workers Paid"] = price("cf.workers.paid.subscription")
	out["Agents account: outbound handler, webhooks, Workflow requests and CPU"] = (
		over(reqs, AGENT_INCLUDED["workers_req"]) / 1e6 * price("cf.workers.requests")
		+ over(cpu, AGENT_INCLUDED["workers_cpu"]) / 1e6 * price("cf.workers.cpu"))
	out["Agents account: Workflow steps"] = over(trib_tasks * STEPS_PER_TASK, AGENT_INCLUDED["steps"]) / 1e5 * price(
		"cf.workflows.steps")
	out["Agents account: session records in two buckets, 12 months"] = (
		over(session_gb, AGENT_INCLUDED["r2_storage"]) * price("cf.r2.standard.storage")
		+ over(trib_tasks * SESSION_COPIES, AGENT_INCLUDED["r2_a"]) / 1e6 * price("cf.r2.standard.class_a"))
	return out


def codemode_optin(tier):
	"""Opt-in Code Mode: each search() or execute() loads model-written code, one Dynamic Worker per call."""
	devs = TIERS[tier]["devs"]
	calls = per_month(devs, TASKS * MCP_SHARE * CODEMODE_CALLS)
	return over(calls, DYN_INCLUDED) * price("cf.dynamic_workers.instance")


def a1_shard_addon():
	"""Journey A1 monorepo: extra warm standard-4 git cache shards so one network's git CPU fits (see DESIGN.md, Scale)."""
	name, vcpu, gib, disk = cf.INSTANCE["large"]
	per = gib * SECONDS * price("cf.containers.memory") + disk * SECONDS * price("cf.containers.disk")
	return 6, per, 6 * per


def a1_rates():
	agents, push_every_s, task_s = 500, 120, TASK_MIN * 60
	pushes = agents / push_every_s
	prs = agents / task_s
	rest = agents * REST / task_s
	rest_w = agents * REST_WRITES / task_s
	mcp = agents * MCP_SHARE * MCP_CALLS / task_s
	jobs = agents * CI_JOBS / task_s
	humans = 10 / 60 * 2 + 50 / 60 + 50 * 1 / 60      # queue entry and merge, base bot moves, human comments
	in_repo = pushes + pushes + prs + rest_w + jobs + humans   # pushes, PrSync, PR opens, REST writes, checks, humans
	fork_base = in_repo - pushes
	reads = 33 + 200 + (rest - rest_w) + mcp * REST_PER_MCP + pushes * 2 + agents * CLONES / task_s
	vcpu = dict(index_pack=pushes * cf.PUSH_VCPU_S, test_merge=pushes * cf.MERGE_VCPU_S, agent_fetch=pushes * cf.FETCH_VCPU_S,
		ci_miss=33 * 0.05 * cf.CI_MISS_VCPU_S, queue_merge=10 / 60 * cf.MERGE_VCPU_S)
	eager = agents * 10 / 60 * cf.MERGE_VCPU_S
	print("| A1 quantity | Value |")
	print("| --- | --- |")
	print(f"| Agent pushes | {pushes:.2f}/s |")
	print(f"| Base group commands, agents push in-repo | {in_repo:.1f}/s against the 300/s Milestone 0 gate ({300 / in_repo:.0f}x) |")
	print(f"| Base group commands, agents push to a workspace fork | {fork_base:.1f}/s ({300 / fork_base:.0f}x); fork group {pushes:.2f}/s |")
	print(f"| Base leader requests | {reads + in_repo:.0f}/s against 1,000/s ({1000 / (reads + in_repo):.1f}x) |")
	print(f"| Inline bytes per second, 40 KB average | {pushes * 40 / 1024:.2f} MB/s against 15 MB/s at 10 entries/s |")
	print(f"| Inline bytes per second, every push 1 MiB | {pushes:.2f} MB/s against 15 MB/s enam, 9 MB/s apac |")
	print(f"| Git cache vCPU for the network | {sum(vcpu.values()):.1f} ({', '.join(f'{k} {v:.1f}' for k, v in vcpu.items())}) against 4 per shard |")
	print(f"| Eager test-merge recompute on every base move | {eager:.0f} vCPU |")


def main():
	tiers = list(TIERS)
	table = {t: lines(t) for t in tiers}
	big = table[tiers[-1]]
	allk = list(dict.fromkeys(k for t in tiers for k in table[t]))
	base_keys = sorted((k for k in allk if k.startswith("Base:") and "other lines" not in k), key=lambda k: -big.get(k, 0.0))
	keys = base_keys + [k for k in allk if "other lines" in k] + [k for k in allk if not k.startswith("Base:")]
	devs = [TIERS[t]["devs"] for t in tiers]
	print("| Added line item | " + " | ".join(f"{d:,} devs" for d in devs) + " |")
	print("| --- | " + " | ".join("---" for _ in tiers) + " |")
	for k in keys:
		print(f"| {k} | " + " | ".join(f"${table[t].get(k, 0.0):,.0f}" for t in tiers) + " |")
	totals = [sum(table[t].values()) for t in tiers]
	print("| **Added total** | " + " | ".join(f"**${x:,.0f}**" for x in totals) + " |")
	base = [sum(base_lines(t).values()) for t in tiers]
	print("| Base total (`cloudflare/cost.py`, design) | " + " | ".join(f"${x:,.0f}" for x in base) + " |")
	print("| Added share of base | " + " | ".join(f"{a / b:.0%}" for a, b in zip(totals, base)) + " |")
	tasks = [per_month(TIERS[t]["devs"], TASKS) for t in tiers]
	print("| Added cost per agent task | " + " | ".join(f"${a / n:.4f}" for a, n in zip(totals, tasks)) + " |")
	sess = [per_month(TIERS[t]["devs"], TASKS * MCP_SHARE) for t in tiers]
	mcp = [sum(v for k, v in table[t].items() if k.startswith("trib-mcp")) for t in tiers]
	print("| MCP cost per session (30 calls) | " + " | ".join(f"${m / s:.5f}" for m, s in zip(mcp, sess)) + " |")
	print("| Code Mode cost per session if opted in | " + " | ".join(
		f"${codemode_optin(t) / s:.3f}" for t, s in zip(tiers, sess)) + " |")
	print()
	print("Not in the totals:")
	print("| Option | " + " | ".join(f"{d:,} devs" for d in devs) + " |")
	print("| --- | " + " | ".join("---" for _ in tiers) + " |")
	print("| Opt-in Code Mode on every MCP call (Dynamic Workers created) | " + " | ".join(
		f"${codemode_optin(t):,.0f}" for t in tiers) + " |")
	n, per, total = a1_shard_addon()
	print(f"| A1 monorepo: {n} extra warm standard-4 shards (${per:,.0f} each) | n/a | n/a | ${total:,.0f} |")
	if "--a1" in sys.argv[1:]:
		print()
		a1_rates()


if __name__ == "__main__":
	main()
