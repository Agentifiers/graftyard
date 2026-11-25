#!/usr/bin/env python3
"""Added monthly Cloudflare cost of the agent layer (`design/AGENT-LAYER.md`) on top of `scripts/cost.py`.

Usage, from the repository root:
	python3 scripts/cost_agents.py          # line items at 50, 300, and 2,000 developers, hot-network sizing, Code Mode
	python3 scripts/cost_agents.py --a1     # also the journey A1 per-object rates
	python3 scripts/cost_agents.py --levers # each cost lever alone and the combined scenarios, base plus agents
	python3 scripts/cost_agents.py --lever shard-mem,edge-compose   # the tables with levers on (`scripts/cost.py`, LEVERS)

Method. The base cost model (`scripts/cost.py`, default `design` variant) runs twice per tier, once as is and once
with the agent workload added to its ACTIVITY. The difference is what agents add to the base's own lines. New pieces
are priced on top: test merges on agent pushes, hot-network fan-out shards for the tier's largest network, agent-tip
clone-cache entries in both warm buckets, the advisory conflict predictor at its 8-pair budget, `graft-mcp`, fold rows
for path sets and provenance, the hot-group log archive and stored `.idx` files, session logs kept in packs, the
activity feed's long-poll hold time, and the agents account. Lease commands, task rows, and feed requests grow the
base's own lines. Sandbox compute and model tokens for Graftyard-run agents are priced in a separate table outside
the comparison with the base, which excludes runner compute. Every per-operation constant here is a [guess] until
Milestone 0 measures it, except those tagged [measured]: PREDICT_VCPU_S, PART_VCPU_S, META_DISK, SMALL_ROW_DISK, and
the git CPU constants taken from `scripts/cost.py`.

Push rates (AG-ACT-22). Every monthly line, the fan-out sizing included, uses 5 pushes per task. Journey A1's push
every 2 minutes (10 per task) is a separate peak case printed by `--a1`. One clone-cache miss model serves CPU, egress,
and storage. A tip misses on its first checkout, so an agent CI job, at 3 jobs for 5 pushes, always checks out a fresh
tip and misses, while agent clones at the moving default tip share CI's keys and its 70% hit rate.

Started from history/arena/cloudflare-agents/candidate-a/cost_agents.py. Changes, per history/arena/cloudflare-agents/SYNTHESIS.md:
fan-out sizing counts the per-push test merge; the hot-network shards for the largest network at each size are inside
the totals; session records moved from two agents-account buckets into packs under a hidden ref; overlap and
provenance check runs, the agent gate, the predictor, the clone-cache double write, and the package proxy are added.
Code Mode stays a separate opt-in table. Review resolutions (`history/review/cloudflare-agents-interrogate.md`): AG-ACT-22's
single push rate and miss model, AG-ACT-19's Sandbox and model-token table, AG-ACT-10's log cap and member bytes,
AG-ACT-11's admission classes, AG-ACT-12 and 13's fan-out sizing with the test-merge budget, AG-ACT-16's rate limits.
`history/review/cloudflare-leases-tasks.md` adds lease commands, feed requests, and task rows (LT-ACT-5). The final pass
(`history/review/final-interrogate.md`) splits feed pollers by `paths` with FEED_BATCH_ALL and moves feed polls off the leader
(FX-25), counts child-group months on member bytes against CHILD_GROUP_AT (FX-26), and takes the hot pool from
`cost.py`'s `shards()` (FX-11).

Agent workload (history/arena/cloudflare-agents/TASK.md): per developer per day, 3 agent tasks. Each task makes 5 pushes to
its own branch, 60 REST calls, 2 clones (half partial), 1 draft pull request, 3 CI jobs, and 30 MCP tool calls if the
agent uses MCP (half do). A task runs 20 minutes.
"""

import importlib.util
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location("cf_cost", ROOT / "scripts" / "cost.py")
cf = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cf)
price, DAYS, SECONDS, TIERS = cf.price, cf.DAYS, cf.SECONDS, cf.TIERS

# ---- agent workload (TASK.md) ----
TASKS = 3                  # agent tasks per developer per day
PUSHES, REST, CLONES, PRS, CI_JOBS = 5, 60, 2, 1, 3
MCP_CALLS, MCP_SHARE = 30, 0.5
TASK_MIN = 20
CLONE_SHALLOW = 0.5        # share of agent clones that are depth-1 and so composable; the rest are blobless (a filter)

# ---- per-operation guesses ----
REST_WRITES = 9            # of the 60 REST calls, writes that become chain commands (comments, edits, labels) [guess]
PRSYNC_EXTRA = 0.5         # PrSync commands per agent push beyond the base's 0.3 (4 of 5 pushes hit an open PR) [guess]
CHECK_WRITES_PER_PUSH = 0.5  # `graftyard/overlap` and `graftyard/provenance` check runs, written only when a verdict
                           # changes (AG-ACT-10): each of the two changes on about one push in four [guess]
GATE_ROWS = 1              # human approval per agent PR, which writes `AgentGate`; the status renders from the fold
TASK_ROWS = 1              # `AgentTask` row per task that has one, created through the task API before tokens are minted
TEST_MERGES_PER_PUSH = 1.0  # lazy test merges still run once per head push, each a test-merge record [guess]
REST_PER_MCP = 1.5         # REST calls graft-mcp makes per MCP tool call [guess]
MCP_AUTH_REQS = 10         # per MCP session: PRM, AS metadata, CIMD fetch, consent, authorize, callback, token,
                           # scoped token, unscoped-token delete, refresh with a second scoping
MCP_CPU_MS = 3             # graft-mcp CPU per tool call (JSON-RPC, schema check, trust view) [guess]
EMAILS_PER_TASK = 1        # review-request notification per agent PR [guess]
RUN_SHARE = 0.5            # share of agent tasks that are Graftyard-run (the rest are third-party apps) [guess]
PACKAGE_GETS = 50          # registry GETs per Graftyard-run task through the outbound router [guess]
OUTBOUND_PER_TASK = 60 + 2 * PUSHES + 3 * CLONES + 100 + PACKAGE_GETS   # REST, git, model, package calls [guess]
WEBHOOKS_PER_TASK = 15     # deliveries from the install to the controller per Graftyard-run task [guess]; each also
                           # runs `graft-agents-front` in the agents account before `graft-controller`
RELEASE_READS_PER_WEBHOOK = 1.0   # upper bound: a `graft-agents-front` isolate refreshes its `Release` view only on a
                           # request, at most once per POINTER_TTL, from the install's edge and one `Release` member
HEARTBEAT_S = 30           # `graftyard_git::taskbox::HEARTBEAT`: one `TaskBox` alarm per task this often [guess]
BOX_ROWS_PER_CALL = 1      # the box written back to its storage on each alarm, report, and outbound request
                           # (`workers/controller/src/taskbox.rs`); each alarm also sets the next, one more row
SESSION_PUSHES = 1         # one push of the session log blob under refs/graftyard/sessions/ per Graftyard-run task
SESSION_KB = 200           # compressed, redacted session log per task (candidate C) [guess]
SESSION_MONTHS = 12        # retention; the default is the org's audit-log retention, priced at 12 months [guess]
PACK_COPIES = 3 + 1        # three site buckets plus the vault's locked copy
AGENT_PUSH_KIND = "Refs,Provenance,PathSet"   # an agent push's entry; the base prices each push as `Refs` alone, so the
                           # `Provenance` and `PathSet` rows are this layer's line (cf.REPO_KIND_ROWS) [measured]
MEMBERS = 3
PUSH_GC_DAYS = 14          # agent branch bytes kept until GC drops them after merge or close
PREDICT_MEAN_PAIRS = 1.80  # path-overlapping pairs per head push on a real fleet's pushes: 5,637 pairs for 3,134 pushes
                           # [measured, tests/m0/results/m0-wp16.md, L0-05]; the second warm site is sized for these
PREDICT_CPU = 0.5          # compiled share of the second warm site's shard vCPU the predictor may use
PREDICT_PAIRS = 8          # the compiled budget of path-overlapping open PRs per head push, priced in full
PREDICT_VCPU_S = 0.37      # one predicted pair: land the head on its base, then merge-tree against the other ref, more
                           # than one test merge [measured, tests/m0/results/m0-wp16.md, L0-05: one predicted pair on
                           # torvalds/linux costs 0.37 vCPU-s, 8,449 container CPU-s over 23,013 pairs in the padded
                           # L0-06 run]
CACHE_DAYS = 1             # an agent-tip clone-cache entry lives a day under an R2 lifecycle rule [guess]
DYN_INCLUDED = 1000        # unique Dynamic Workers per month included
CODEMODE_CALLS = MCP_CALLS  # opt-in Code Mode: every tool call is a search() or execute(), each a new Dynamic Worker

# ---- leases, activity feed, and task API (LT-ACT-1 to 5) ----
LEASE_TTL_S = 600          # `LEASE_TTL`, the default a holder asks for
LEASE_RENEW_S = LEASE_TTL_S / 3   # holders renew at a third of the TTL [guess]
LEASES_PER_TASK = 1        # one Shared lease per agent task on its planned paths [guess]
LEASE_EDGES = 2            # acquire and release commands per lease [guess]
LEASE_ROW_B = 600          # live `Lease` row with its globs [guess]
FEED_WAIT_S = 25           # long-poll `wait`, the maximum; each working agent keeps one poll held [guess]
FEED_EARLY = 1.0           # early returns per held poll with `paths`, from events that overlap them [guess]
FEED_ALL_SHARE = 0.5       # share of pollers that send no `paths` and so return on any event [guess]
FEED_BATCH_ALL_S = 10      # `FEED_BATCH_ALL`: a poll without `paths` batches this long, and the busy group always has
                           # an event, so each such poller makes one request per batch (FX-25) [guess]
FEED_RING, FEED_EVENT_B = 10_000, 300   # events kept in the feed's ring fold, and bytes per event [guess]
TASK_API_SHARE = 1.0       # share of agent tasks with an `AgentTask` row: every Graftyard-run task, and every
                           # third-party task reported as `kind: external`, an upper bound [guess]
TASK_STATE_WRITES = 3      # task state changes per task, such as in_progress, a PR, and completed [guess]
LEASE_CMDS_PER_TASK = LEASES_PER_TASK * (LEASE_EDGES + TASK_MIN * 60 / LEASE_RENEW_S)
FEED_POLLS_PER_TASK = TASK_MIN * 60 * ((1 - FEED_ALL_SHARE) * (1 + FEED_EARLY) / FEED_WAIT_S
	+ FEED_ALL_SHARE / FEED_BATCH_ALL_S)
TASK_CMDS_PER_TASK = TASK_API_SHARE * (TASK_ROWS + TASK_STATE_WRITES)

# ---- one clone-cache miss model (AG-ACT-22) ----
# A tip misses on its first checkout and hits after (cost.py's CI_HIT: about 3.75 CI checkouts per pushed SHA). An
# agent task runs 3 CI jobs for 5 pushes, so each agent CI job is the first checkout of its tip and misses.
AGENT_CI_HIT = max(0.0, 1 - PUSHES / CI_JOBS)
CLONE_HIT = cf.CI_HIT      # agent clones are shallow or blobless at the moving default tip, so they share CI's keys
CACHE_ENTRIES_PER_TASK = CI_JOBS * (1 - AGENT_CI_HIT) + CLONES * (1 - CLONE_HIT)   # new clone-cache entries per task

# ---- hot-network fan-out (base change 1, AG-ACT-12 and AG-ACT-13) ----
PUSH_EVERY_S = 120         # A1 peak case only: each agent pushes its branch every 2 minutes
WORKDAY_MIN = 480          # agent tasks run inside an 8-hour working day [guess]
PEAK = 2                   # peak concurrency over the working-day average [guess]
HOT_SHARE = 1.0            # share of peak agents on the install's largest network; at 2,000 developers this is A1 [guess]
A1_MERGES_PER_MIN = 10     # merge-queue merges on the A1 monorepo, scaled by developers at smaller tiers [guess]
FANOUT_TARGET = 0.75       # share of the most loaded shard's vCPU that fan-out plans to use
PART_VCPU_S = 0.5          # one composed-miss part per tip move, built once per network by the shard that took the merge:
                           # 0.3 to 0.8 CPU-s a part, 2 to 2.3 for a 400 to 650 object move, about 0.5 at the mean
                           # [measured, tests/m0/results/m0-wp9.md, B0-34, third pass]
TEST_MERGE_BUDGET = 0.25   # compiled share of the merging shard's vCPU that test merges may use (AG-ACT-13): a network's
                           # merges all run on its first shard on the leader's site, so k does not raise the budget
FANOUT_MAX = 8
WARM_SITES = 2

# ---- hot-group storage (AG-ACT-10) ----
LOG_MAX_GB = 2             # LOG_MAX_BYTES per member [guess]
ARCHIVE_DAYS = 35          # archived entries kept for `RestoredTo`, as the base's 35-day restore window
ARCHIVE_BUCKETS = 2
IDX_KB = 2                 # stored `.idx` of an inline pack on a hot network [guess]
CMD_BYTES = 300            # average non-push command in the log [guess]
PR_ROW_KB = 2              # pull request, `AgentMeta`, and its two refs [guess]
CHECK_ROW_B = 300          # check run or status row, kept 90 days as GHES does [guess]
PROV_ROW_B = 150           # provenance row per pushed commit [guess]
TASK_ROW_B = 1024          # `AgentTask`: about 100 B core part plus prompt, summary, and PR in metadata [guess]
PATHSET_KB = 16            # live `PathSet` per open agent PR or ref; deleted when the PR closes [guess]
BASE_META_GB = 4           # the base's largest-repository metadata tier (`design/DESIGN.md`, Scale) [guess]
CHILD_GROUP_AT_GB = 8      # `CHILD_GROUP_AT`, 8 GiB of member SQLite bytes, at which the metadata tier moves to a child
                           # group (base Milestone 7, FX-11); every GB figure here is GiB
META_DISK = 1.16           # SQLite bytes on disk per row byte of the metadata tier: 3.93 GB of rows took 4.57 GB
                           # [measured, tests/m0/results/m0-wp9.md, B0-30]
SMALL_ROW_DISK = 1.4       # the same for small agent rows (PR, check, provenance, task): 1.27 GB of rows took about
                           # 1.8 GB [measured, tests/m0/results/m0-wp9.md, B0-30]

# ---- learners for the hot group (K9, tests/am1/results/learner-dumps.md, tests/am1/results/narrow-reads.md) ----
API_TIER_READS_S = 200     # journey 9's API reads a second at 500 agents (B0-29's `read:api`)
API_READ_WEIGHT = 1        # narrow reads each counts in `GroupLoad`: since narrow-reads an API read answers the rows of
                           # its page or object, not the 1 MB whole tiers that weighed 50 (a learner served about 20 of
                           # those a second [measured, tests/am1/results/learner-dumps.md]), so it counts once as every
                           # other narrow read does [inference, tests/am1/results/narrow-reads.md]
LEARNER_HOURS = 9          # hours a day the hot group's reads keep its learners active: the 8-hour working day plus the
                           # hour, as the hot pool; a learner with no reads is evicted and bills no duration [guess]
LEARNER_AT, LEARNER_READS = 500, 1000   # `graftyard_types::placement`
A1_ENTRIES_S = 10          # entries a second on the A1 group at peak (about 10 entries/s, AGENT-LAYER's Scale)

# ---- admission and rate limits (AG-ACT-11, AG-ACT-16) ----
GATE_CMDS = 300            # base Milestone 0 group-commit gate, commands/s
AGENT_SHARE, AGENT_FLOOR = 0.5, 0.2
MANAGED_HOURLY = (5000, 15000)   # GitHub installation-token limits per hour, standard to Enterprise Cloud
MANAGED_CONTENT = (80, 500)      # content-generating requests per minute and per hour

# ---- not in the totals: Graftyard-run agent compute (AG-ACT-19) ----
SANDBOX = ("standard-2", 1, 6, 12)   # vCPU, GiB, GB disk (https://developers.cloudflare.com/containers/platform/pricing/)
SANDBOX_CPU = 0.5          # active share of the Sandbox vCPU over a task [guess]
MODEL_USD_PER_TASK = 2.0   # model tokens per Graftyard-run task, the middle of a $1 to $3 range [guess]

AGENT_INCLUDED = dict(workers_req=10e6, workers_cpu=30e6, do_req=1e6, do_gbs=400_000, do_rows=50e6,
	c_mem_gibs=25 * 3600, c_vcpu_s=375 * 60, c_disk_gbs=200 * 3600)


def over(used, included):
	return max(0.0, used - included)


def per_month(devs, per_dev_day):
	return devs * per_dev_day * DAYS


def base_lines(tier, activity=None, logs_factor=1.0, ssh_share=None, ci_hit=None):
	"""Run the base model with a patched ACTIVITY, logs volume, SSH share, and CI hit rate, then restore them."""
	saved = (cf.ACTIVITY, cf.TIERS, cf.SSH_SHARE, cf.CI_HIT)
	try:
		if activity is not None:
			cf.ACTIVITY = activity
		tiers = {k: dict(v) for k, v in saved[1].items()}
		tiers[tier]["logs_gb_day"] *= logs_factor
		cf.TIERS = tiers
		if ssh_share is not None:
			cf.SSH_SHARE = ssh_share
		if ci_hit is not None:
			cf.CI_HIT = ci_hit
		return cf.model(tier, "design")
	finally:
		cf.ACTIVITY, cf.TIERS, cf.SSH_SHARE, cf.CI_HIT = saved


def edge_requests_per_dev_day(a):
	return a["api_requests"] + (a["pushes"] + a["dev_fetches"] + a["ci_jobs"]) * cf.GIT_REQS


def agent_activity(tier, with_mcp):
	"""ACTIVITY with the agent workload added. Clones ride as fetches whose bytes are only the clone-cache misses."""
	a0, t = cf.ACTIVITY, TIERS[tier]
	a = dict(a0)
	run = TASKS * RUN_SHARE
	a["pushes"] += TASKS * PUSHES + run * SESSION_PUSHES
	a["ci_jobs"] += TASKS * CI_JOBS
	checks = TASKS * PUSHES * CHECK_WRITES_PER_PUSH
	rows = TASKS * PRS * GATE_ROWS + TASKS * TASK_CMDS_PER_TASK
	leases = TASKS * LEASE_CMDS_PER_TASK
	a["api_requests"] += TASKS * REST + checks + rows + leases + TASKS * FEED_POLLS_PER_TASK
	a["metadata_writes"] += (TASKS * (PRS + REST_WRITES) + TASKS * PUSHES * (PRSYNC_EXTRA + TEST_MERGES_PER_PUSH)
		+ checks + rows + leases)
	# The agent writes by `RepoEvent` kind, for their measured rows (cf.meta_fold_rows); REST writes take the base's mix.
	renews = TASK_MIN * 60 / LEASE_RENEW_S
	a["meta_kinds"] = {"PrOpened": TASKS * PRS, cf.PRSYNC_KIND: TASKS * PUSHES * PRSYNC_EXTRA,
		"TestMerged": TASKS * PUSHES * TEST_MERGES_PER_PUSH, "CheckRun": checks, "AgentGate": TASKS * PRS * GATE_ROWS,
		"TaskCreated": TASKS * TASK_API_SHARE * TASK_ROWS, "TaskUpdated": TASKS * TASK_API_SHARE * TASK_STATE_WRITES,
		"LeaseAcquire": TASKS * LEASES_PER_TASK, "LeaseRelease": TASKS * LEASES_PER_TASK,
		"LeaseRenew": TASKS * LEASES_PER_TASK * renews}
	assert abs(sum(a["meta_kinds"].values()) + TASKS * REST_WRITES - (a["metadata_writes"] - a0["metadata_writes"])) < 1e-9
	a["emails"] += TASKS * EMAILS_PER_TASK
	clones = TASKS * CLONES
	clone_mb_out = t["checkout_mb"] * (1 - CLONE_HIT) * cf.miss_out(cf.COMPOSED_SHARE * CLONE_SHALLOW)
	a["dev_fetch_mb"] = (a0["dev_fetches"] * a0["dev_fetch_mb"] + clones * clone_mb_out) / (a0["dev_fetches"] + clones)
	a["dev_fetches"] += clones
	if with_mcp:
		a["api_requests"] += TASKS * MCP_SHARE * MCP_CALLS * (1 + REST_PER_MCP)
	# Agents push and clone over HTTPS only, so hold the base's SSH bytes constant.
	base_bytes = a0["dev_fetches"] * a0["dev_fetch_mb"] + a0["pushes"] * a0["push_mb"]
	new_bytes = a["dev_fetches"] * a["dev_fetch_mb"] + a["pushes"] * a["push_mb"]
	ssh_share = cf.SSH_SHARE * base_bytes / new_bytes
	logs_factor = edge_requests_per_dev_day(a) / edge_requests_per_dev_day(a0)
	# One miss model: base CI jobs keep the base hit rate, agent CI jobs miss on their fresh tips.
	ci_hit = (a0["ci_jobs"] * cf.CI_HIT + TASKS * CI_JOBS * AGENT_CI_HIT) / a["ci_jobs"]
	return a, ssh_share, logs_factor, ci_hit


def shard_vcpu(agents, merges_per_min, pushes_per_task):
	"""vCPU-s per second on the network's home-site git cache shards, every task spread over the k by rendezvous."""
	turnover = agents / (TASK_MIN * 60)
	pushes = turnover * pushes_per_task
	return {
		"index-pack (branch and session-log pushes)": (pushes + turnover * RUN_SHARE * SESSION_PUSHES) * cf.PUSH_VCPU_S,
		"test merge per head push": pushes * TEST_MERGES_PER_PUSH * cf.MERGE_VCPU_S,
		"agent CI clone-cache misses": turnover * CI_JOBS * (1 - AGENT_CI_HIT) * cf.CI_MISS_VCPU_S,
		"agent clone misses": turnover * CLONES * (1 - CLONE_HIT) * cf.CI_MISS_VCPU_S,
		"merge-queue merges": merges_per_min / 60 * cf.MERGE_VCPU_S,
		"composed-miss base renewal": merges_per_min / 60 / cf.BASE_RENEW_EVERY * cf.BASE_RENEW_VCPU_S,
		"composed-miss part builds": merges_per_min / 60 * PART_VCPU_S,
	}


def fanout(vcpu_load, vcpu):
	"""k per warm site: the most loaded shard under FANOUT_TARGET. Validation and misses spread over the k by
	rendezvous, so the most loaded shard carries about the average [inference]; Milestone 0 measures the skew. Test
	merges run on the network's first shard alone, so k does not raise TEST_MERGE_BUDGET: head-push merges must fit one
	shard's share, which `merge_budget_fits` checks."""
	load = sum(vcpu_load.values())
	return min(FANOUT_MAX, max(1, math.ceil(load / (FANOUT_TARGET * vcpu))))


def merge_budget_fits(vcpu_load, vcpu):
	"""Head-push test merges inside TEST_MERGE_BUDGET of the one shard that runs them (`design/DESIGN.md`, Shape,
	`MergeBudget`)."""
	return vcpu_load["test merge per head push"] <= TEST_MERGE_BUDGET * vcpu


def instance_month(tier):
	name, vcpu, gib, disk = cf.INSTANCE[tier]
	return name, vcpu, gib * SECONDS * price("cf.containers.memory") + disk * SECONDS * price("cf.containers.disk")


def hot_network(tier, pushes_per_task=PUSHES):
	"""The tier's largest network: peak agents on it, its shard vCPU, k per warm site, and the extra warm shards."""
	devs = TIERS[tier]["devs"]
	agents = devs * TASKS * TASK_MIN / WORKDAY_MIN * PEAK * HOT_SHARE
	merges = A1_MERGES_PER_MIN * devs / 2000
	vl = shard_vcpu(agents, merges, pushes_per_task)
	name, vcpu, per = instance_month(tier)
	k = fanout(vl, vcpu)
	# The second warm site also runs the predictor, so it takes the larger of k and the shards the measured mean
	# overlap needs under PREDICT_CPU. One shard saturated at A1's rate (p95 364 s against L0-05's 30 s).
	k2 = max(k, predict_k(agents, vcpu, pushes_per_task))
	extra = (k - 1) + (k2 - 1)
	return dict(agents=agents, load=sum(vl.values()), vl=vl, instance=name, vcpu=vcpu, k=k, k2=k2, extra=extra, per=per,
		cost=extra * per)


def predict_k(agents, vcpu, pushes_per_task=PUSHES):
	"""Shards on the second warm site that the predictor needs at the measured mean overlap, under PREDICT_CPU."""
	pushes = agents / (TASK_MIN * 60) * pushes_per_task
	demand = pushes * PREDICT_MEAN_PAIRS * PREDICT_VCPU_S
	return min(FANOUT_MAX, max(1, math.ceil(demand / (PREDICT_CPU * vcpu))))


def learner_cost(tier):
	"""Learners on the tier's largest network: enough to serve its API reads at API_READ_WEIGHT narrow reads each,
	scaled by its peak agents against A1's 500, up to MAX_LEARNERS 16. Each holds both tiers in SQLite (the base's
	4 GB tier, the log at LOG_MAX_BYTES, and 30 days of agent rows), applies every entry, and is active while it serves
	reads, so it bills storage, duration at 128 MB, applied rows, and its appends and reads as DO requests."""
	scale = hot_network(tier)["agents"] / 500
	reads = API_TIER_READS_S * scale
	# `GroupLoad` counts an API read as API_READ_WEIGHT narrow ones, and the group's other reads (CI ref reads, the
	# rest of A1's) once; `place_learners` adds ceil(load / LEARNER_READS) above LEARNER_AT.
	load = reads * API_READ_WEIGHT + (33 + 89) * scale
	n = 0 if load < LEARNER_AT else min(16, math.ceil(load / LEARNER_READS))
	member_gb = BASE_META_GB * META_DISK + LOG_MAX_GB + 1.65
	active_s = LEARNER_HOURS * 3600 * DAYS
	storage = n * member_gb * price("cf.do.sqlite.storage")
	duration = n * active_s * cf.DO_GB / 1e6 * price("cf.do.duration")
	# Entries a month on the hot group: its agent pushes at A1's entries per push (10 entries/s for 500 pushes/120 s).
	entries = hot_storage(tier)["pushes"] * A1_ENTRIES_S / (500 / PUSH_EVERY_S)
	rows = n * entries * cf.repo_entry_rows(AGENT_PUSH_KIND) / 1e6 * price("cf.do.sqlite.rows_written")
	# One append per entry per learner, and the API reads they take from the voters while active.
	reqs = (n * entries + (reads * active_s if n else 0.0)) / 1e6 * price("cf.do.requests")
	return dict(n=n, reads=reads, load=load, gb=member_gb, storage=storage, duration=duration, rows=rows, reqs=reqs,
		cost=storage + duration + rows + reqs)


def hot_storage(tier):
	"""Monthly inline bytes on the hot group, what LOG_MAX_BYTES archives, and the stored `.idx` count."""
	devs = TIERS[tier]["devs"]
	pushes = per_month(devs, TASKS * PUSHES) * HOT_SHARE
	inline_gb = pushes * cf.ACTIVITY["inline_push_kb"] / 1024 / 1024
	archive_gb = over(inline_gb, LOG_MAX_GB)
	idx = pushes if hot_network(tier)["k"] > 1 else 0.0
	return dict(pushes=pushes, inline_gb=inline_gb, archive_gb=archive_gb, idx=idx)


def lines(tier):
	devs = TIERS[tier]["devs"]
	l0 = base_lines(tier)
	a1, s1, f1, h1 = agent_activity(tier, with_mcp=False)
	l1 = base_lines(tier, a1, f1, s1, h1)
	a2, s2, f2, h2 = agent_activity(tier, with_mcp=True)
	l2 = base_lines(tier, a2, f2, s2, h2)
	diff = {k: l1[k] - l0[k] for k in l0}
	out = {}
	agent_pushes = per_month(devs, TASKS * PUSHES)
	tasks = per_month(devs, TASKS)
	vcpu_price = price("cf.containers.vcpu")
	r2_gb, r2_a = price("cf.r2.standard.storage"), price("cf.r2.standard.class_a") / 1e6

	out["Base lines grown by agent git, REST, CI, check runs, tasks, leases, and feed polls (breakdown below)"] = sum(
		diff.values())
	out["Containers: test merges on agent head pushes (active vCPU)"] = (agent_pushes * TEST_MERGES_PER_PUSH
		* cf.MERGE_VCPU_S * vcpu_price)
	hot = hot_network(tier)
	out["Hot-network fan-out: extra warm shards for the largest network"] = hot["cost"]
	# One miss model: a new entry per agent CI checkout and per clone miss, kept a day. The base prices the clone cache
	# as 10% of git bytes, so agent-tip entries in the first warm bucket are priced here, and Class A is in the base.
	entries = tasks * CACHE_ENTRIES_PER_TASK
	entry_gb_month = entries * TIERS[tier]["checkout_mb"] / 1024 * CACHE_DAYS / DAYS
	out["Clone cache: agent-tip entries in the first warm bucket"] = entry_gb_month * r2_gb
	# Base change 5: every clone-cache entry of the hot network also goes to the second warm site's bucket.
	out["Clone cache: hot-network entries copied to the second warm bucket"] = HOT_SHARE * (entries * r2_a
		+ entry_gb_month * r2_gb)
	out["Conflict predictor on the second warm site, 8 pairs a push (active vCPU)"] = (agent_pushes * PREDICT_PAIRS
		* PREDICT_VCPU_S * vcpu_price)

	# graft-mcp: MCP requests and the REST behind them, plus session auth requests and grant and scoped-token entries.
	sessions = per_month(devs, TASKS * MCP_SHARE)
	auth_rows = sessions * 2 * MEMBERS * cf.BLOB_ENTRY_ROWS   # a scoped-token hash on Tokens, a grant on Identity
	out["graft-mcp: tool calls and REST behind them (base lines)"] = sum(l2.values()) - sum(l1.values())
	out["graft-mcp: CPU, consent and session auth, grant and scoped-token entries"] = (
		per_month(devs, TASKS * MCP_SHARE * MCP_CALLS) * MCP_CPU_MS / 1e6 * price("cf.workers.cpu")
		+ sessions * MCP_AUTH_REQS / 1e6 * price("cf.workers.requests")
		+ auth_rows / 1e6 * price("cf.do.sqlite.rows_written"))

	# K9: learners on the largest network's group. Each holds both tiers in SQLite, applies every entry, and is active
	# while it serves reads, so it bills storage, duration (128 MB per active object), applied rows, and its share of
	# the group's reads and appends as DO requests. Scaled by the tier's peak agents against A1's 500.
	lc = learner_cost(tier)
	out["Learners on the largest network's group (storage, duration, applied rows, requests; K9)"] = lc["cost"]
	fold_rows = cf.kind_rows(AGENT_PUSH_KIND) - cf.kind_rows(cf.PUSH_KIND)
	out["Folds: PathSet and provenance rows"] = (agent_pushes * fold_rows * MEMBERS / 1e6
		* price("cf.do.sqlite.rows_written"))
	# Each working agent keeps one feed poll held on a member. Priced as the base prices DO wall time, per request with
	# no sharing, so an upper bound: on a group that commits every second the member is active anyway.
	share = cf.FEED_SHARE if "feed-shared" in cf.LEVERS else 1
	out["Activity feed: long-poll hold time on members (upper bound)"] = (tasks * TASK_MIN * 60 * cf.DO_GB / 1e6
		* price("cf.do.duration") / share)
	hs = hot_storage(tier)
	out["R2: hot-group log archive and stored .idx of inline packs (two buckets)"] = (
		hs["archive_gb"] * ARCHIVE_BUCKETS * ARCHIVE_DAYS / DAYS * r2_gb
		+ hs["idx"] * ARCHIVE_BUCKETS * (r2_a + IDX_KB / 1024 / 1024 * ARCHIVE_DAYS / DAYS * r2_gb))
	push_gb = devs * TASKS * PUSHES * cf.ACTIVITY["push_mb"] * PUSH_GC_DAYS / 1024
	out["R2: agent branch packs until GC (3 copies plus vault)"] = push_gb * PACK_COPIES * r2_gb
	run_tasks = per_month(devs, TASKS * RUN_SHARE)
	log_gb = run_tasks * SESSION_KB / 1024 / 1024 * SESSION_MONTHS
	out["Session logs in stored packs, 12 months (3 copies plus vault)"] = log_gb * PACK_COPIES * r2_gb

	# Agents account for Graftyard-run agents: its own plan and allowances. Sandbox compute and models are separate.
	# Every webhook runs `graft-agents-front`, then `graft-controller`, so it counts twice.
	reqs = run_tasks * (OUTBOUND_PER_TASK + 2 * WEBHOOKS_PER_TASK)
	cpu = run_tasks * (OUTBOUND_PER_TASK * 1 + WEBHOOKS_PER_TASK * 3)
	out["Agents account: Workers Paid"] = price("cf.workers.paid.subscription")
	out["Agents account: front, outbound router, and webhooks (Workers requests and CPU)"] = (
		over(reqs, AGENT_INCLUDED["workers_req"]) / 1e6 * price("cf.workers.requests")
		+ over(cpu, AGENT_INCLUDED["workers_cpu"]) / 1e6 * price("cf.workers.cpu"))
	# `GET /api/graftyard/v1/release/entries` from the agents account: an edge request and a member request each, priced
	# with no allowance because the install's own traffic uses it up.
	out["Install: `Release` reads by the agents account (edge and member requests)"] = (run_tasks * WEBHOOKS_PER_TASK
		* RELEASE_READS_PER_WEBHOOK / 1e6 * (price("cf.workers.requests") + price("cf.do.requests")))
	# One `TaskBox` Durable Object per Graftyard-run task (DECISIONS "`controller-taskbox`"): its start, an alarm every
	# HEARTBEAT_S, and every outbound request the Sandbox makes, which the router hands to the box. Duration is an
	# upper bound: the box is priced active for the whole task, since it awaits each forwarded request, model calls
	# among them, and an idle box between alarms is billed nothing (`cf.do.duration`).
	alarms = TASK_MIN * 60 / HEARTBEAT_S
	box_reqs = run_tasks * (1 + alarms + OUTBOUND_PER_TASK)
	box_gbs = run_tasks * TASK_MIN * 60 * cf.DO_GB
	box_rows = run_tasks * ((1 + alarms + OUTBOUND_PER_TASK) * BOX_ROWS_PER_CALL + alarms)
	out["Agents account: TaskBox Durable Objects (requests, duration upper bound, alarm and box rows)"] = (
		over(box_reqs, AGENT_INCLUDED["do_req"]) / 1e6 * price("cf.do.requests")
		+ over(box_gbs, AGENT_INCLUDED["do_gbs"]) / 1e6 * price("cf.do.duration")
		+ over(box_rows, AGENT_INCLUDED["do_rows"]) / 1e6 * price("cf.do.sqlite.rows_written"))
	return out, diff


def agent_compute(tier):
	"""Not in the totals: Sandbox compute and model tokens for Graftyard-run tasks, in the agents account."""
	run_tasks = per_month(TIERS[tier]["devs"], TASKS * RUN_SHARE)
	_, vcpu, gib, disk = SANDBOX
	secs = run_tasks * TASK_MIN * 60
	sandbox = (over(secs * gib, AGENT_INCLUDED["c_mem_gibs"]) * price("cf.containers.memory")
		+ over(secs * vcpu * SANDBOX_CPU, AGENT_INCLUDED["c_vcpu_s"]) * price("cf.containers.vcpu")
		+ over(secs * disk, AGENT_INCLUDED["c_disk_gbs"]) * price("cf.containers.disk"))
	return dict(tasks=run_tasks, sandbox=sandbox, model=run_tasks * MODEL_USD_PER_TASK)


def codemode_optin(tier):
	"""Opt-in Code Mode: each search() or execute() loads model-written code, one Dynamic Worker per call."""
	devs = TIERS[tier]["devs"]
	calls = per_month(devs, TASKS * MCP_SHARE * CODEMODE_CALLS)
	return over(calls, DYN_INCLUDED) * price("cf.dynamic_workers.instance")


def a1_rates():
	agents, task_s = 500, TASK_MIN * 60
	pushes = agents / PUSH_EVERY_S
	per_task = task_s / PUSH_EVERY_S
	turnover = agents / task_s
	agent_cmds = {
		"agent pushes": pushes,
		"PrSync for those pushes": pushes,
		"test-merge records": pushes * TEST_MERGES_PER_PUSH,
		"draft PR opens": turnover * PRS,
		"agent REST writes": turnover * REST_WRITES,
		"AgentTask entries and task state changes": turnover * TASK_CMDS_PER_TASK,
		"lease acquire and release": turnover * LEASES_PER_TASK * LEASE_EDGES,
		f"lease renewals every {LEASE_RENEW_S:.0f} s": agents * LEASES_PER_TASK / LEASE_RENEW_S,
		"RequiredCheck from agent CI": turnover * CI_JOBS,
		"overlap and provenance check runs on a verdict change": pushes * CHECK_WRITES_PER_PUSH,
		"session-log pushes": turnover * RUN_SHARE * SESSION_PUSHES,
	}
	other_cmds = {
		"human approvals of agent PRs (AgentGate)": turnover * PRS * GATE_ROWS,
		"human, queue, and bot commands": 10 / 60 * 2 + 50 / 60 + 50 / 60,
	}
	agent = sum(agent_cmds.values())
	other = sum(other_cmds.values())
	total = agent + other
	fork_moved = pushes + agent_cmds["session-log pushes"]
	reads = {
		"CI ref reads": 33, "human API reads": 200, "agent REST reads": turnover * (REST - REST_WRITES),
		"MCP-backed reads": agents * MCP_SHARE * MCP_CALLS / task_s * REST_PER_MCP, "push ref reads": pushes * 2,
		"agent clone ref reads": turnover * CLONES, "PathSet scans for overlaps and the predictor": pushes,
	}
	leader = sum(reads.values()) + total
	# FX-25: feed polls go only to followers and learners, which answer from their applied log past the cursor with no
	# read-index call, so the leader sees none. They count toward the group's read rate for LEARNER_AT.
	feed_paths = agents * (1 - FEED_ALL_SHARE) / FEED_WAIT_S * (1 + FEED_EARLY)
	feed_all = agents * FEED_ALL_SHARE / FEED_BATCH_ALL_S
	feed = feed_paths + feed_all
	vl = shard_vcpu(agents, A1_MERGES_PER_MIN, per_task)
	load = sum(vl.values())
	k = fanout(vl, 4)
	monthly_k = hot_network("large")["k"]
	per = instance_month("large")[2]
	budget = TEST_MERGE_BUDGET * 4
	assert merge_budget_fits(vl, 4), "journey 9's head-push merges past one standard-4's TEST_MERGE_BUDGET"
	predict = pushes * PREDICT_PAIRS * PREDICT_VCPU_S
	predict_mean = pushes * PREDICT_MEAN_PAIRS * PREDICT_VCPU_S
	k2 = max(k, predict_k(agents, 4, per_task))
	cap = PREDICT_CPU * k2 * 4
	per_agent = agent / agents
	admitted = AGENT_SHARE * GATE_CMDS
	print("| A1 quantity | Value |")
	print("| --- | --- |")
	for name, v in agent_cmds.items():
		print(f"| Agent-class command: {name} | {v:.2f}/s |")
	for name, v in other_cmds.items():
		print(f"| Human, merge-queue, or bot command: {name} | {v:.2f}/s |")
	print(f"| Monorepo group commands, agents in-repo | {total:.1f}/s against the 300/s gate ({GATE_CMDS / total:.0f}x); "
		f"agent class {agent:.1f}/s |")
	print(f"| Monorepo group commands, Graftyard-run agents on a workspace fork | {total - fork_moved:.1f}/s "
		f"({GATE_CMDS / (total - fork_moved):.0f}x); fork group {fork_moved:.2f}/s |")
	print(f"| Monorepo leader requests | {leader:.0f}/s against 1,000/s ({1000 / leader:.1f}x), none of them feed polls |")
	print(f"| Activity feed | {feed:.0f} requests/s from {agents} pollers: {feed_paths:.0f}/s from "
		f"{agents * (1 - FEED_ALL_SHARE):.0f} with `paths` (half on timeout, half early), {feed_all:.0f}/s from "
		f"{agents * FEED_ALL_SHARE:.0f} without `paths` at FEED_BATCH_ALL {FEED_BATCH_ALL_S} s; about "
		f"{feed / (MEMBERS - 1):.0f}/s on each follower against 1,000/s; group reads {sum(reads.values()) + feed:.0f}/s "
		f"against LEARNER_AT 500 |")
	print(f"| Live leases | {agents * LEASES_PER_TASK:.0f} of LEASES_PER_REPO 4,000, "
		f"{agents * LEASES_PER_TASK * LEASE_ROW_B / 2**20:.2f} MB per member; feed ring "
		f"{FEED_RING * FEED_EVENT_B / 2**20:.1f} MB |")
	print(f"| Inline bytes, 40 KB average | {pushes * 40 / 1024:.2f} MB/s against 15 MB/s at 10 entries/s |")
	print(f"| Inline bytes, every push 1 MiB | {pushes:.2f} MB/s against 2.0 MB/s measured from an apac leader (one 1 MiB "
		f"pack per entry, round p50 0.55 s to enam) |")
	print(f"| Git cache vCPU for the network | {load:.2f} ({', '.join(f'{n} {v:.2f}' for n, v in vl.items())}) |")
	print(f"| fanout k per warm site (standard-4) | {k}, {k * 4} vCPU, {load / (k * 4):.0%} used; test-merge budget "
		f"{budget:.0f} vCPU for {vl['test merge per head push']:.2f} of head-push merges; {(k - 1) + (k2 - 1)} extra "
		f"warm shards with the predictor's k = {k2} on the second site, ${((k - 1) + (k2 - 1)) * per:,.0f} a month if held; the monthly sizing at {PUSHES} pushes a task "
		f"gives k = {monthly_k} |")
	print(f"| Hot pool per site | {cf.shards('large')['hot_pool']} instances; reserved vCPU at 2,000 developers "
		f"{cf.shards('large')['reserved_vcpu']:,.1f} of {cf.ACCOUNT_VCPU:,} |")
	print(f"| Conflict predictor demand on the second warm site | {predict_mean:.1f} vCPU at the measured "
		f"{PREDICT_MEAN_PAIRS} pairs a push, so k = {k2} there, cap {cap:.0f} vCPU; {predict:.1f} vCPU if every push used all "
		f"{PREDICT_PAIRS} pairs ({min(1, cap / predict):.0%} of pairs at peak) |")
	print(f"| Eager test-merge recompute on every base move (500 PRs, 10 moves/min) | "
		f"{agents * A1_MERGES_PER_MIN / 60 * cf.MERGE_VCPU_S:.0f} vCPU |")
	gets = agents / 30 * cf.MERGE_VCPU_S
	print(f"| 500 clients polling every 5 s, at most one compute per PR per 30 s | {gets:.0f} vCPU asked, "
		f"{max(0.0, budget - vl['test merge per head push']):.2f} vCPU of budget left after head pushes |")
	print(f"| Workflow approvals a minute under the default `ci: Approve` | {turnover * CI_JOBS * 60:.0f} |")
	x10 = agent * 10
	print(f"| Same mix at 5,000 agents | {x10:.0f} agent-class commands/s offered, {other:.1f}/s human and queue; "
		f"AGENT_SHARE {AGENT_SHARE} of {GATE_CMDS}/s admits {admitted:.0f}/s, about {admitted / per_agent:,.0f} agents per "
		f"group; AGENT_FLOOR keeps {AGENT_FLOOR * GATE_CMDS:.0f}/s from Bot |")
	tok_h = REST * 60 / TASK_MIN
	content_h = (REST_WRITES + PRS) * 60 / TASK_MIN
	inst_h = agents * RUN_SHARE * tok_h
	print(f"| Rate limit per minted token (one Graftyard-run task) | {tok_h:.0f} requests/h against {MANAGED_HOURLY[0]:,} "
		f"({MANAGED_HOURLY[0] / tok_h:.0f}x); {content_h:.0f} content writes/h against {MANAGED_CONTENT[1]} |")
	print(f"| Same fleet keyed per installation | {inst_h:,.0f} requests/h against {MANAGED_HOURLY[0]:,} to "
		f"{MANAGED_HOURLY[1]:,} ({inst_h / MANAGED_HOURLY[1]:.0f}x to {inst_h / MANAGED_HOURLY[0]:.0f}x over) |")
	# 30 days of A1: the 2,000-developer month of agent tasks on the monorepo, at A1's push every 2 minutes.
	tasks30 = 2000 * TASKS * 30
	pushes30 = tasks30 * per_task
	cmds_per_task = agent / turnover
	log_gb = (pushes30 * cf.ACTIVITY["inline_push_kb"] * 1024 + tasks30 * (cmds_per_task - per_task) * CMD_BYTES) / 2**30
	# Row bytes on disk: SMALL_ROW_DISK for the small agent rows, META_DISK for the metadata tier (B0-30).
	live_gb = (agents * (PATHSET_KB * 1024 + LEASES_PER_TASK * LEASE_ROW_B) + FEED_RING * FEED_EVENT_B) / 2**30
	monthly_growth = SMALL_ROW_DISK * (tasks30 * PRS * PR_ROW_KB * 1024 + pushes30 * PROV_ROW_B
		+ tasks30 * TASK_API_SHARE * TASK_ROW_B) / 2**30
	checks_month = SMALL_ROW_DISK * (pushes30 * CHECK_WRITES_PER_PUSH + tasks30 * CI_JOBS) * CHECK_ROW_B / 2**30
	rows_gb = monthly_growth + checks_month + live_gb
	meta_gb = BASE_META_GB * META_DISK
	kept = min(log_gb, LOG_MAX_GB)
	checks_steady = checks_month * 3
	# FX-11 and FX-26: CHILD_GROUP_AT is member bytes, so the clock runs on the whole member: tier, full log, live rows,
	# check rows that fill for 90 days, and growth.
	fixed = meta_gb + LOG_MAX_GB + live_gb
	months = (CHILD_GROUP_AT_GB - fixed) / (checks_month + monthly_growth)
	if months > 3:
		months = (CHILD_GROUP_AT_GB - fixed - checks_steady) / monthly_growth
	print(f"| Member bytes after 30 days of A1 on a group seeded with the base's {BASE_META_GB} GB metadata tier | "
		f"{meta_gb + kept + rows_gb:.2f} GB on disk: the tier {meta_gb:.2f} GB, log {kept:.1f} GB at LOG_MAX_BYTES "
		f"({log_gb:.0f} GB uncapped, the rest archived), agent rows {rows_gb:.2f} GB; growing "
		f"{(checks_month + monthly_growth) / 30:.3f} GB a day |")
	print(f"| Member bytes of the A1 monorepo against CHILD_GROUP_AT | {fixed:.1f} GB fixed (tier, full log, live rows), "
		f"plus {checks_month:.2f} GB a month of check runs and statuses for 90 days (then steady at "
		f"{checks_steady:.1f} GB) and {monthly_growth:.2f} GB a month of PRs, provenance, and tasks; "
		f"{CHILD_GROUP_AT_GB} GiB of member bytes in about {months:.1f} months ({months * 30:.0f} days) |")


def main():
	tiers = list(TIERS)
	table, diffs = {}, {}
	for t in tiers:
		table[t], diffs[t] = lines(t)
	devs = [TIERS[t]["devs"] for t in tiers]
	head = " | ".join(f"{d:,} devs" for d in devs)
	sep = " | ".join("---" for _ in tiers)
	print(f"| Added line item | {head} |")
	print(f"| --- | {sep} |")
	for k in table[tiers[0]]:
		print(f"| {k} | " + " | ".join(f"${table[t][k]:,.0f}" for t in tiers) + " |")
	totals = [sum(table[t].values()) for t in tiers]
	print("| **Added total** | " + " | ".join(f"**${x:,.0f}**" for x in totals) + " |")
	base = [sum(base_lines(t).values()) for t in tiers]
	print("| Base total (`scripts/cost.py`, design) | " + " | ".join(f"${x:,.0f}" for x in base) + " |")
	print("| Added share of base | " + " | ".join(f"{a / b:.0%}" for a, b in zip(totals, base)) + " |")
	tasks = [per_month(TIERS[t]["devs"], TASKS) for t in tiers]
	print("| Added cost per agent task | " + " | ".join(f"${a / n:.4f}" for a, n in zip(totals, tasks)) + " |")
	sess = [per_month(TIERS[t]["devs"], TASKS * MCP_SHARE) for t in tiers]
	mcp = [sum(v for k, v in table[t].items() if k.startswith("graft-mcp")) for t in tiers]
	print("| MCP cost per session (30 calls) | " + " | ".join(f"${m / s:.5f}" for m, s in zip(mcp, sess)) + " |")
	print()
	print(f"| Base line grown by agents | {head} |")
	print(f"| --- | {sep} |")
	big = diffs[tiers[-1]]
	shown = [k for k in sorted(big, key=lambda k: -big[k]) if big[k] >= 5]
	for k in shown:
		print(f"| {k} | " + " | ".join(f"${diffs[t][k]:,.0f}" for t in tiers) + " |")
	print("| Other base lines | " + " | ".join(
		f"${sum(v for k, v in diffs[t].items() if k not in shown):,.0f}" for t in tiers) + " |")
	print()
	print("| Largest network, 5 pushes a task | Peak agents on it | Shard vCPU | Instance | k per warm site "
		"| Extra warm shards | Per month |")
	print("| --- | --- | --- | --- | --- | --- | --- |")
	for t in tiers:
		h = hot_network(t)
		print(f"| {TIERS[t]['devs']:,} devs | {h['agents']:,.1f} | {h['load']:.2f} | {h['instance']} | {h['k']} "
			f"| {h['extra']} | ${h['cost']:,.0f} |")
	print()
	print("| Learners on the largest network | API reads/s | `GroupLoad` | Learners | GB each | Storage | Duration "
		"| Applied rows | Requests | Per month |")
	print("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
	for t in tiers:
		c = learner_cost(t)
		print(f"| {TIERS[t]['devs']:,} devs | {c['reads']:.0f} | {c['load']:,.0f} | {c['n']} | {c['gb']:.1f} | ${c['storage']:,.0f} "
			f"| ${c['duration']:,.0f} | ${c['rows']:,.0f} | ${c['reqs']:,.0f} | ${c['cost']:,.0f} |")
	print()
	print(f"| Not in the totals: Graftyard-run agent compute (agents account) | {head} |")
	print(f"| --- | {sep} |")
	ac = [agent_compute(t) for t in tiers]
	print("| Graftyard-run tasks a month | " + " | ".join(f"{c['tasks']:,.0f}" for c in ac) + " |")
	print(f"| Sandbox compute, {SANDBOX[0]} for {TASK_MIN} minutes at {SANDBOX_CPU:.0%} CPU | "
		+ " | ".join(f"${c['sandbox']:,.0f}" for c in ac) + " |")
	print(f"| Model tokens at ${MODEL_USD_PER_TASK:.2f} a task [guess] | " + " | ".join(f"${c['model']:,.0f}" for c in ac) + " |")
	print("| Per Graftyard-run task, Sandbox plus model | " + " | ".join(
		f"${(c['sandbox'] + c['model']) / c['tasks']:.3f}" for c in ac) + " |")
	print()
	print(f"| Not in the totals: opt-in Code Mode | {head} |")
	print(f"| --- | {sep} |")
	print("| Every MCP call as a new Dynamic Worker | " + " | ".join(f"${codemode_optin(t):,.0f}" for t in tiers) + " |")
	print("| Per MCP session | " + " | ".join(f"${codemode_optin(t) / s:.3f}" for t, s in zip(tiers, sess)) + " |")
	if "--a1" in sys.argv[1:]:
		print()
		a1_rates()


def totals(tier):
	"""Base (`cost.py`, design) and agent layer totals with the levers now on."""
	return sum(cf.model(tier).values()), sum(lines(tier)[0].values())


def with_levers(names, f):
	saved = set(cf.LEVERS)
	cf.LEVERS.clear()
	cf.LEVERS.update(names)
	try:
		return f()
	finally:
		cf.LEVERS.clear()
		cf.LEVERS.update(saved)


LOW_RISK = ("edge-compose", "upload-once", "logs-sample", "r2-ia", "feed-shared")   # without-rowid is not built
SCENARIOS = {
	"S1: accepted or low risk": LOW_RISK,
	"S2: S1 + shard-class (recommended)": LOW_RISK + ("shard-class",),
	"S3: S2 + pool-2 (deferred to a B0-34 pass)": LOW_RISK + ("shard-class", "pool-2"),
	"S2 with shard-class-lean (1 vCPU ordinary shards)": LOW_RISK + ("shard-class-lean",),
	"S2 + ws-half (if the working set measures at half)": LOW_RISK + ("shard-class", "ws-half"),
	"Correction: as built today (fill-as-built)": ("fill-as-built",),
	"S2 against as built": LOW_RISK + ("shard-class", "fill-as-built"),
}


def lever_report():
	"""Each lever alone, then the combined scenarios, at 300 and 2,000 developers, base plus agents."""
	tiers = ("medium", "large")
	base = {t: with_levers((), lambda t=t: totals(t)) for t in tiers}
	print("| Lever | 300 devs total | Saved | 2,000 devs total | Saved | Of which base | Of which agents |")
	print("| --- | --- | --- | --- | --- | --- | --- |")
	b0 = {t: sum(base[t]) for t in tiers}
	print(f"| Default (design, with agents) | ${b0['medium']:,.0f} | | ${b0['large']:,.0f} | | ${base['large'][0]:,.0f} "
		f"| ${base['large'][1]:,.0f} |")
	rows = []
	for name in cf.LEVER_NAMES:
		got = {t: with_levers((name,), lambda t=t: totals(t)) for t in tiers}
		rows.append((b0["large"] - sum(got["large"]), name, got))
	for saved, name, got in sorted(rows, reverse=True):
		m, l = sum(got["medium"]), sum(got["large"])
		print(f"| {name} | ${m:,.0f} | ${b0['medium'] - m:,.0f} ({(b0['medium'] - m) / b0['medium']:.0%}) | ${l:,.0f} "
			f"| ${saved:,.0f} ({saved / b0['large']:.0%}) | ${base['large'][0] - got['large'][0]:,.0f} "
			f"| ${base['large'][1] - got['large'][1]:,.0f} |")
	print()
	print("| Scenario | Levers | 300 devs | Saved | 2,000 devs (base + agents) | Saved |")
	print("| --- | --- | --- | --- | --- | --- |")
	for name, levers in SCENARIOS.items():
		got = {t: with_levers(levers, lambda t=t: totals(t)) for t in tiers}
		m, l = sum(got["medium"]), sum(got["large"])
		print(f"| {name} | {', '.join(levers)} | ${m:,.0f} | {(b0['medium'] - m) / b0['medium']:.0%} "
			f"| ${l:,.0f} (${got['large'][0]:,.0f} + ${got['large'][1]:,.0f}) | {(b0['large'] - l) / b0['large']:.0%} |")


if __name__ == "__main__":
	sys.argv[1:] = cf.set_levers(sys.argv[1:])
	if "--levers" in sys.argv[1:]:
		lever_report()
	else:
		main()
