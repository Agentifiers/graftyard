# edge-refusal: agent writes refused at the edge at the leader's measured rate

B0-33 step 3, 2026-11-23, at $0. This file is the evidence for `decisions/DECISIONS.md`, "`edge-refusal`". Tags follow
`design/SHARED.md`, "Read this first".

**Short answer.**
- **About a third of the edge 503s were the leader's own `full`** [measured]. pfA had 2,754 of 6,754 and pgA 1,728
  of 5,205. `commit_issue_rid` and `commit_merge` rendered `{"done": {"full": …}}` as 503 "the member answered …". They
  now answer 429 with the leader's `retry-after`.
- **Most of the rest were reads shed before the commit, while the leader's agent lane was deep** [measured]. pfA had
  3,999 and pgA 3,476. Every one was "every member asked shed the read: overloaded", on the tier read of a task,
  GraphQL, renew, lease or REST write.
- **A slow leader drained its agent lane at less than its bucket said** [measured]. While agent commands stayed queued,
  pfA's leader (ATL) cut 54.6 agent commands a second at p50, pgA's (ORD) 76.3 and blB's (EWR) 108.8. The AGENT_SHARE
  bucket it sized waits from read 1.26 to 1.36 times that. So it admitted bounded pushes whose real wait passed their
  bound, and on pfA 4,689 then expired queued.
- **Refused at the edge in the `full` shape, these become clean refusals** [inference, upper bound]. The rule is the
  measured drain's wait past a 5 s bound. That covers about 6,350 of pfA's 6,754 503s and about 4,000 of pgA's 5,205.
  Most of pgA's remainder is the 02:24 stall, when the leader stopped answering and so stopped advertising.

## What the leader drained

Source: `tests/fanout/agentdrain.py TAG`, from each row's leader `Trace::Cut` rows (kind 29, `tsave-TAG.jsonl.xz`),
in 10 s windows of cuts. A window is saturated when most of its cuts left agent commands queued [measured].

| Row (leader colo) | Saturated windows | Agent commands cut a second, p10 / p50 / p90 | Bucket p50, and over the drain | Agent queue while saturated |
|---|---|---|---|---|
| pfA (ATL) | 24 | 27.7 / 54.6 / 89.6 | 69, 1.26x | 600 to 990, from 01:05 |
| pgA (ORD) | 22 | 38.6 / 76.3 / 108.8 | 104, 1.36x | 120 to 550 in the first leadership, 900 to 960 after the drop |
| blB (EWR) | 20 | 76.8 / 108.8 / 132.9 | 147, 1.35x | 60 to 300 |

- **What ends the cuts.** In saturated windows the cuts ended `AgentCmds` (64 agent commands, AGENT_CMDS_ENTRY) far
  more often than `AgentTokens`. So entries a second times 64 bounds the drain, not the bucket. A slow host makes fewer
  entries a second: pfA 0.6 to 1.9, blB 1.2 to 2.1.
- **The bucket.** It is AGENT_SHARE times ENTRY_SOFT over the moving mean command size, times entries a second. Here
  it is replayed from the cuts' soft bytes, so it is an estimate of the leader's own figure [inference].
- **The advertised wait.** That is the queue over the drain. pfA held 7 to 48 s from 01:05 on, with a p50 of about
  16 s. pgA held 1.4 to 5.0 s in its first leadership and 8.7 to 38.8 s after the 02:24:09 drop and retake.
- **What the leader refused itself.** pfA: 4,689 agent pushes expired queued and 3,774 were refused at admission.
  pgA: 3,059 of each (`rows-TAG.status`, "push refusals by stage").

## The 503s against the advert

Source: the edge's Workers Logs `fivexx` lines on graft-m2b-edge, grouped by kind and reason in 20 s buckets. That
is the same query as `tests/fanout/fivexxwhy.py`, by bucket; the extracts are `tests/fanout/raw/fivexx20-pfA.jsonl`
and `fivexx20-pgA.jsonl`. They are joined to the advert sampled 5, 10, 15 and 20 s into each bucket
(`agentdrain.py TAG FIVEXX`) [measured; the conversion is an inference].

| Row | Edge 503s | Leader `full` rendered 503 | Shed reads | Shed reads in buckets whose advert passed 5 s | Clean `full`s, at most |
|---|---|---|---|---|---|
| pfA | 6,754 | 2,754 | 3,999 | 3,593 | 6,347 |
| pgA | 5,205 | 1,728 | 3,476 | 2,277 | 4,005 |

- **Why these are upper bounds.**
  - GraphQL's sheds include the admin's and the fast user's human writes. They are never refused this way and stay 503.
  - Refusing an agent write before its read also removes reads from the learners. So in steady state the sheds should
    fall, rather than each turn into a `full`.
- **What is not covered.** pgA's 02:24:00 bucket had 833 sheds while the last advert showed 5 to 85 queued. The leader
  had stalled, so it was not answering and not advertising. The read gate's `gate_ng` and the shed 503 remain for that.

## What was built

- **Leader** (`crates/raft/src/admit.rs`, `Drain`; `member/leader.rs`, `Admission::agent_rate`).
  - The leader counts the agent commands each cut takes, and the time since the previous cut, but only when that
    previous cut left agent commands queued. The first cut after an idle gap counts nothing.
  - Over DRAIN_WINDOW (10 s) of its own clock, that is its drain. Below DRAIN_MIN (1 s [guess]) of saturated time, the
    bucket's rate stands, and the drain is never taken above it.
  - The expected wait behind `Admit::budget`, the retry hint (`retry_after_s`) and `AdmissionView::agent_wait` all use
    the drain.
- **Site** (`workers/site/src/member.rs`).
  - Every reply's `meta` from a leader carries `admit: {waitMs, retryAfterS}`.
  - `/status` carries `agentRate` and `agentWaitMs` (`/stats` is unchanged: its `json!` is at the macro's depth).
- **Lookup** (`crates/lookup`, `note_advert` and `advert`). The edge keeps each chain's last advert from the commit
  path's 200s (`heard`) and from `call_opt`'s leader answers. That is fresh for ADVERT_TTL (2 s [guess]), from the
  group's current term only, and adds no read.
- **Edge** (`workers/edge/src/lib.rs`, `advert_refusal`).
  - For a push, at `receive_pack` after the `early` refusal and before `push_context`, stage-first, validate and
    commit. The budget is what is left of AGENT_PUSH_BOUND, and the answer is `ng <ref> agent admission full, retry
    after <n>s`.
  - For a REST, GraphQL or agent-API write, at the top of `read_repo`, with a budget of AGENT_PUSH_BOUND. The answer is
    429 with `retry-after`.
  - Only an Agent-classed caller is refused (`admission`), and only one whose token reaches the repository. The
    install's `Authz` is read only once the advert already says full.
  - Each refusal sets `advert;dur=<wait ms>` in `server-timing`, apart from `bound`, for B0-32's split of `full` by
    shape.
  - Each refusal, and each Agent write sent on a lapsed advert that said full, logs an `edgefull` line. The line
    carries the isolate's running counts of both.
- **Render fix** (`workers/edge/src/api.rs`, `done_reply`). Leader `full` becomes 429 in `commit_issue_rid` and
  `commit_merge`.

## Tests

- **`crates/raft/tests/admission.rs`, `a_slow_leader_refuses_at_once_what_it_cannot_drain_within_the_bound`**
  [measured in memory].
  - Setup: rounds slowed by 300 ms, and 200 bounded (5 s) agent commands a second for 30 s.
  - With the drain: 3,082 committed, 2,918 refused at once, 0 expired, and the agent rate read 85 a second.
  - With the bucket rate (mutant): 3,137 committed, 5 refused at once, 2,858 expired queued, 2,467 committed past
    their 5 s bound, and the rate read 356 a second. The test fails.
- **`crates/raft/src/admit.rs`, `the_drain_counts_saturated_time_only` and `the_drain_needs_drain_min_of_saturated_time`.**
  An idle gap before a burst does not dilute the rate, an emptying cut ends the run, and under 1 s of saturated time
  reads nothing.
- **`crates/lookup`, `a_leader_advert_is_fresh_for_advert_ttl_and_only_from_the_current_term`.** The advert is fresh at
  2,000 ms and lapsed at 2,001. A reply without an advert keeps the last one, and a deposed leader's advert is ignored.
- **`workers/edge`, `the_edge_refuses_only_an_agent_write_whose_fresh_advert_passes_its_budget`.** A wait of 5,001 ms
  refuses and 5,000 sends. Human, Bot and MergeQueue always send. A lapsed advert that said full is a fail-open.
- **`workers/edge`, `a_token_that_cannot_reach_the_repository_is_never_refused_by_the_advert`.**
- **`workers/edge/src/api.rs`, `a_leader_full_on_an_issue_or_merge_command_is_429_with_its_retry_after`.**
- **Plant 92** (`tests/m0carried/plant92.py`): 11 of 11 caught. Mutant 0 is main's bucket rate; mutant 1, a drain not capped at the bucket, is caught by `the_advertised_agent_rate_never_passes_the_bucket`.

## Row B: plan, cleared by the lead on 2026-11-23

The lead reordered B0-33: row B (this change alone, on graft-m2b from merged main) runs before LM-1's apply-cut row,
which then gets its own row on top. LM-1, graft-m2b's owner, cleared the install for it. Nothing has run yet.

**This row reads slow hosts only. It is not typical-host behaviour.**
- On a host `tests/fanout/hostguard.py` keeps (28.4 ms or less of leader CPU a setup commit), the leader never
  advertised a wait past the 5 s bound: blB 0 of 20 saturated 10 s windows (largest wait 2.9 s), sabA 0 of 21 (3.8 s).
  On re-placed hosts it did: pfA 21 of 24 (48.2 s), pgA 7 of 22 (38.8 s) [measured, `agentdrain.py`]. So a kept host
  predicts about 0 edge refusals and reads nothing.
- So the guard is inverted (`guardrow.sh`, `GUARD_WANT=slow`): a1 loads only on a leader over the line, and a leader at
  or under it is re-placed. Unknown still fails closed. At most three placements; if none is slow, the set stops and is
  reported, and the line is not widened.
- Each reading is compared only with baselines in the same setup-CPU band: 39.7 to 46.1 ms (pfA, pgA, ncA) or 31.0
  to 31.5 ms (blA, blC2, chA).

**Build and load.** One test-faults site day on graft-m2b from merged main, with a new shard image, then pfA's load
(`costrow.sh` ROW=b033 under `guardrow.sh`, no `--seed-prs`), with `a1.mjs --advert-probe`: before the hold, the leader's
`/status` must carry `agentRate` and `agentWaitMs`, and a forced advert (`x-graft-advert` with the bed token, test-faults
only) must refuse an agent push and an agent REST write with `advert` and neither of the admin's. A failed probe exits
before load and stops the set.

**Stops.** RUN_STOP $4 a placement; DAY_STOP at graft-m2b's day at the site day's start plus $8; charged to the B0-33
research budget.

**Flip 1 is an invariant and stops the row.** a1 ends the load window on any human, merge or fast-user write that
carries the `advert` mark, writes it as `flip1` in the row, and `guardrow.sh` stops the set for the lead.

**Pass, against the matched slow baseline** [the lead's ruling]:

| Line | Baseline | Pass |
|---|---|---|
| Edge 503s | pfA 6,754, pgA 5,205 | At most half |
| Leader `full` rendered 503 | pfA 2,754, pgA 1,728 | 0 |
| Shed reads while the advert passed 5 s | pfA 3,593, pgA 2,277 | Down by at least half |
| Agent `full`s the leader refused (admission plus `expire_agents`) | pfA 3,774 + 4,689 | At most 25% |
| `ERR_HTTP2_STREAM_ERROR` on agent pushes | pfA 1,371, pgA 2,251 | At most a third |
| Human commit stage p99 | pfA 9,394, pgA 11,018 | At most two thirds, with 0 leader changes |

Human p99 within 20% of the baseline is flip 5; between the two marks, one run is inconclusive.

**The confound.** Merged main also carries reask-read and stage-first-2 on the paths this row uses. The `advert`
stage, the `edgefull` lines and the leader-`full`-as-503 count belong to edge refusal alone; the 503 total and human
p99 do not. The row reports reask-read's top-ups (`Trace::TopUp`) and stage-first's `stagefirst` marks beside them.

**Instruments** (`b033.sh` appends them to `rows-TAG.status`):
- `pushfull.py`: an `edge (advert)` stage for agent pushes, apart from `bound_at_send`, admission and `expire_agents`.
- `a1.mjs`: `api429`, every API 429 by caller (agent or not) and shape (`advert` or the leader's own), and `flip1`.
- `edgefull.py`: the `edgefull` lines from Workers Logs, per isolate (`refused`, `failOpen`, against the about one
  fail-open an isolate a chain per ADVERT_TTL ceiling) and per 10 s window, joined to the leader's cuts for flips 2 to 4.
  The cuts run on the leader's clock, 1 to 3.5 s behind Cloudflare's, so a window is read with the one before it.

## Row B: erBA, the result (2026-11-23)

One placement, run erBA, ruled on by the lead. **It reads a slow host only, not typical-host behaviour.**

| Item | erBA |
|---|---|
| Build | site day -erB from main 050eefc8, test-faults, siteday exit 0; new shard image; edge 47b8f634 at 100% (`edgever.py`) |
| Repo, a1 run | 131394, `mwt69ks9`; 5,000 agents; learners `enam:2000,weur:1000,apac:1000`; no `--seed-prs` |
| Guard | `GUARD_WANT=slow`: hostguard re-place verdict, 36.1 ms of leader CPU a setup commit (49 commits); load released |
| Probe | `--advert-probe`: all five checks passed; the leader's `agentRate` 780.9, `agentWaitMs` 112 before load |
| Load (UTC) | 11:20:56 to 11:25:56; done line 11:31:20Z |
| Cost | $1.63 for the run (`costrow-erBA.status`), against RUN_STOP $4; graft-m2b $4.99 for the day at 11:32Z |
| Sweep | rows.sh's sweep and a final `teardown.py --sweep` exited 0; no shard container left |

**The host falls between the two approved bands** (31.0 to 31.5 ms and 39.7 to 46.1 ms), so no baseline matches it.
The lines are tabled against both bands, and the lead chose neither.

[measured]

| Line | erBA | pfA / pgA (39.7 to 46.1) | blA / blC2 (31.0 to 31.5) | Lead's ruling |
|---|---|---|---|---|
| Edge 503s | 3,263 | 6,754 / 5,205 | 4,835 / 1,168 | On record against both bands |
| Leader `full` rendered 503 | 0 | 2,754 / 1,728 | | Pass |
| Shed reads while the advert passed 5 s | 753 | 3,593 / 2,277 | | Pass (down 79% / 67%) |
| Agent `full`s the leader refused | 6,433 (3,254 admission, 3,179 `expire_agents`) | 8,463 (pfA) | | **Fail**: 76% of pfA, over the 50% fail line |
| `ERR_HTTP2_STREAM_ERROR` on agent pushes | 1 | 1,371 / 2,251 | 0 / 342 | Pass |
| Human commit stage p99, push log (n) | 4,739 (119) | 9,394 / 11,018 | 7,854 / 4,244 | On record against both bands |
| Merges over 2 s at the commit stage | 23 of 44 | 22 of 27, 21 of 34 | 28 of 44, 27 of 37 | |
| Agent pushes ok | 1,498 | 190 / 2,740 | | |
| Leader changes | 0 | | | |

- **All shed 503s:** 3,263, every one "every member asked shed the read: overloaded". 753 fell in buckets whose
  advert passed 5 s; the rest fell while it was under 5 s (`agentdrain.py erBA fivexx20-erBA.jsonl`).
- **Agent `full`s by stage** (`pushfull.py`): 4,014 at the edge on the advert, 540 at `bound_at_send`, 3,254 at
  admission and 3,179 in `expire_agents`. The edge also refused 7,529 REST and agent-API writes with 429 (a1's `api429`:
  2,527 agent 429s carried `advert`, 5,010 were the leader's own).
- **The drain:** 30 saturated windows; agent commands cut a second p10/p50/p90 55.8 / 76.8 / 108.8; the bucket at 1.55
  times the drain.
- **The confound:** 2 reask-read top-ups (`Trace::TopUp`) in the whole trace, and 257 agent `full`s that staged first.

**The flips** (`decisions/DECISIONS.md`, "`edge-refusal`"):
1. **Not met: 0.** No human or merge push carried `advert`, a1 saw no non-agent 429 with it, and `flip1` is empty.
2. **Not met, but fragile.** 3 of 30 refusing windows read a leader wait under 2.5 s, exactly the 10% line. That is
   read with the window before each, as registered. Read on each window alone, it is 13 of 30.
3. **Not met.** No refusing window had an empty agent lane. Agent commands cut a second while refusing: p50 78.7.
4. **Met, on the fail-open path.** The edge refused 11,553 writes, so not few. But 6,032 Agent writes went out on a
   lapsed advert that said full, up to 164 from one isolate, more than half the refusals. An isolate that refuses
   everything hears no advert, and when its advert lapses every Agent write in flight fails open at once, not one per
   ADVERT_TTL.
   - **Correction: fail-opens explain at most a quarter of the leader's refusals** [measured]. Only 1,481 of the 6,032
     fail-opens were pushes, and the leader's 6,433 refusals are all pushes (`pushfull.py`). Per 10 s window,
     push fail-opens and leader push refusals correlate at r 0.11.
   - Each leader-refused push was replayed against the advert its leader's cuts gave about 1 s before it was sent
     (queue over the trailing 10 s drain, shifted 1.5 s for the leader's clock). For 4,644 of the 6,433, that advert
     was within the push's own budget, so the edge would have sent them under any fail-open rule
     [inference: the leader's own advert is rebuilt from the cuts, not read from the edge].
5. **Not met against the upper band** (human p99 down 50 to 57%). Against blC2, 4,739 is 112% of its 4,244.

Raws in `tests/fanout/raw/`:
- `costrow-erBA.status`, `rows-erBA.status`, `siteday-erB.status` (account redacted), `terms-erBA.jsonl`, and
  `tsave-erBA.jsonl.xz`;
- `memguard-erBA.{err,jsonl.xz}`, `arrivals-erBA.jsonl`, and `shardgate-erBA-b0-33-{0,1,2}.jsonl`;
- `edgefull-erBA.jsonl.xz`, the 17,570 `edgefull` lines, and `fivexx20-erBA.jsonl`, the 503s by 20 s bucket.

The a1 raws stay local (`tests/am1/raw/` is git-ignored): `a1-b0-33-mwt69ks9.jsonl` and its push log.

## Open

- The fail-open bound (flip 4), designed before code. Its rerun needs a guard that keeps a leader inside an approved
  band, and the lead's clearance.
- Whether human p99 falls on a slow host is the row's question, not this file's. Edge refusal sheds agent work but does
  not cut apply CPU per entry at the same drain [inference].
