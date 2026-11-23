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

## The fail-open bound and the send check: design (2026-11-23, before code)

The lead ruled erBA's leader-side refusals a Fail and asked for this design, at $0. It covers the fail-open bound and
the advert's under-read. Each rule is checked against erBA's raws (`tests/fanout/advertcf.py`). The row reads a slow
host only.

**The replay.** `advertcf.py` rebuilds the advert from the leader's cuts (`Trace::Cut`):
- The wait is `expected_wait`: the agent lane, interpolated between cuts by the interval's admitted arrivals, over the
  leader's `Drain` (saturated time only, DRAIN_WINDOW, DRAIN_MIN), plus a round (a1's `roundMs`).
- It moves the cuts to Cloudflare's clock by the leader's measured offset per 10 s: the voters' `CallLeft` against the
  leader's `AnswerArrived`, net of the idle wire, as `b033probes/host/drift.py` does. Under load that offset is -0.6 to
  -3.2 s at p50.
- An edge reads the advert 1 s after the leader sent it (`AGE`, run again at 0 and 2 s).
- Each push's check, sends and answer come from its `server-timing`.
- The replay refuses 3,516 of the 4,013 pushes the edge refused on the advert (88%), and sends 5,468 of the 6,451 it
  sent (85%) [measured agreement]. Every count below is first order: a refused push leaves the queue the others met as
  it was. So the in-time pushes it refuses are upper bounds [inference].

### Why the advert under-read: where the check sits, not the drain or the clock

Of the 6,433 pushes the leader refused, 1,481 were fail-opens. The other 5,352 the edge sent on a live advert
[measured, the stage marks; the split is the replay]:

| Why the leader refused a push the edge sent within budget | Pushes | Share |
|---|---|---|
| Its own budget was spent after the check, at the same advert | 2,959 | 55% |
| The lane grew between the check and its arrival, at the advertised drain | 1,241 | 23% |
| The trailing drain fell before it arrived | 129 | 2% |
| Within budget at arrival on the replayed wait (admission 102, expiry 128) | 230 | 4% |
| The replay refuses it at the check, but the edge sent it (replay error) | 793 | 15% |

- **The main cause is the check's position.** The check runs at `receive_pack`, before push context, validate and any
  Stage round. It compares the wait with the budget left there. The push reaches the leader with less budget left.
  - Between the check and the deciding send, these pushes spent p50 2,071 ms and p90 3,489 ms.
  - Of that, push context took p50 850 and p90 3,160 ms, validate 213 and 406, and Stage 0 and 582.
- **The other two hypotheses are small.**
  - **The trailing drain lagging a falling drain:** 2%. The next 10 s drain over the trailing 10 s drain, sampled
    every 5 s, reads p10 0.58, p50 0.93, p90 1.73. It falls a little on average, but it explains few refusals.
  - **The leader's clock lag:** none found.
    - The drain on the leader's clock and on the corrected clock is 87 and 86 a second at p50.
    - Over an expired push's wait, the offset moved p50 0 ms, and more than 500 ms for only 9% of them.
    - An advert read at 0, 1 or 2 s old moves the counterfactual by 4 to 9 points (below).
- **The 23% is the lane growing between the check and arrival.** This is close to the "queue missing commands already
  at the edge" hypothesis: commands other isolates had already sent. A check at the send narrows that gap from p50
  2.1 s to the commit's own wire time. It cannot close it.

### Rule 1, the fail-open bound

- **The rule.** While an isolate's last advert for a chain said full and has lapsed (older than ADVERT_TTL), it acts
  as follows:
  - It sends one Agent write a chain an isolate per ADVERT_TTL. That write is the probe.
  - It refuses every other Agent write in the `full` shape, with that advert's retry hint and the `advert` mark.
- **The stale bound.** After ADVERT_STALE (10 s [guess]) with no advert heard, the isolate fails fully open, and
  every write is sent as before. So an advert path that stops answering refuses agents for at most 10 s.
  - Why 10 s: on erBA a push the leader refused got its answer p99 6.5 s and at most 8.7 s after the edge's check
    [measured]. Probes every 2 s that hear nothing for 10 s mean the advert is not coming back, not that it is late.
- **Only an Agent write is ever held.** Human, Bot and MergeQueue writes, and a token that cannot reach the repository,
  are sent at once on a lapsed advert, as on a fresh one. The hold goes through the same `advert_verdict` that
  refuses on a fresh advert, so flip 1's invariant covers both. A held refusal carries `advert`, so a1's `flip1`
  catches a non-Agent one.
- **A probe the leader refuses.** Every leader answer on the commit path carries `meta.admit`, a refusal at admission
  and an expiry's answer included (`member.rs` `reply`). The probe's answer therefore sets the isolate's advert again
  through `heard` and `note_advert`.
  - If that wait still passes the bound, the isolate refuses on a fresh advert, as today.
  - If it does not, the isolate sends again.
  - A probe with no answer leaves the advert lapsed. The next probe goes one ADVERT_TTL later, and ADVERT_STALE ends
    the hold.
  - Nothing else about the probe changes the isolate's state.
- **The bound on what gets through.** A chain gets at most one probe per ADVERT_TTL from each isolate whose advert
  lapsed. On erBA that is 187 isolates. If all of them lapsed together, that would be about 94 a second, against a
  drain of p50 77. In the replay they did not lapse together: 736 probes in 300 s, 2.5 a second.

### Rule 2, the send check

- **The rule.** For a push, the edge reads the advert again at each commit send. That is `bound_at_send`'s point, and
  again before the Stage re-send. It compares the wait with what is left of the bound there.
  - It refuses in the same shape, `ng <ref> agent admission full, retry after <n>s` with `advert;dur=`, and nothing
    reaches the group.
  - This adds no read: the advert is a thread-local value.
- **REST is not covered.** REST and GraphQL writes keep their one check at the top of `read_repo`. The leader
  refusals in the 50% line are all pushes.

### Counterfactual on erBA's raws

Leader refusals (admission plus expiry), against pfA's 8,463. The 50% line is 4,231 [inference, the replay]:

| Rules | Leader refusals | Share of pfA | In-time (`ok`) pushes the edge would also refuse, of 1,498 |
|---|---|---|---|
| As run | 6,433 | 76% | 0 |
| Rule 1 alone | 5,976 | 71% | 255 |
| Rule 2 alone | 2,052 | 24% | 326 |
| **Rules 1 and 2** | **1,595** | **19%** | **581** |
| Rules 1 and 2, no credit where the replay disagreed with the edge | 2,356 | 28% | 424 |
| Rules 1 and 2, advert 2 s old | 2,144 | 25% | 789 |
| Rules 1 and 2, advert 2 s old, no credit where the replay disagreed | 2,796 | 33% | 407 |
| Rules 1 and 2, send check 500 ms stricter / looser | 982 / 2,585 | 12% / 31% | 785 / 417 |

- **Rule 1 alone does not pass the 50% line.** The fail-opens were 1,481 pushes and 4,547 REST writes, so holding them
  removes few push refusals.
  - In the replay, the hold stops 712 fail-open pushes and 2,612 REST writes.
  - It sends 312 push and 424 REST probes.
  - 457 push and 1,511 REST fail-opens fell more than 10 s after their isolate's last refusal, a lower bound on the
    advert's age, so ADVERT_STALE sends them.
- **Rule 2 carries the change**, and with both rules every variant is under 50%. The pass line in the plan was 25%:
  some variants are on it and the stricter readings are over it.
- **Over-refusal: in-time pushes the edge would refuse.** With both rules this is 424 to 789 of 1,498, against 0 as
  run. That is flip 2's cost.
  - Measured in windows, flip 2 stays at 3 of 30: the extra refusals fall in windows that were already refusing.
  - Per push it is large. An `ok` push refused here would have committed within its bound. But the leader is
    drain-bound: a push refused at the edge frees a lane slot that a push which would have expired then takes
    [inference]. First order cannot show that, so the hosted check is the drain (flip 3) and the new flip R2 below.

### Flips for the new rules

These go beside flips 1 to 5:
- **R1a (invariant).** A Human, Bot or MergeQueue write held on a lapsed advert must be 0. This is flip 1 on the new
  path. Held refusals carry `advert`, so a1's `flip1` stops the row.
- **R1b (invariant).** Any `edgefull` `held` line with an advert older than ADVERT_STALE, or a second `probe` line for
  one chain from one isolate within ADVERT_TTL, must be 0.
- **R1c.** `stale` sends exceed `held` refusals. Then probes are not bringing adverts back: a probe's answer is not
  reaching `note_advert`, so the probe path is broken.
- **R2.** The send check refuses a push in a 10 s window whose leader cuts end `Empty` with fewer than
  AGENT_CMDS_ENTRY agent commands, read with the window before it. That is the edge refusing while the leader has
  room. It may not exceed flip 2's 10% of refusing windows.
- **Under-read left.** The leader still refuses more than 50% of pfA's count with both rules live. Then the remaining
  cause is the lane growing in flight, the 23%, and the next lever is the leader's own admission arithmetic, not the
  edge.

### Instruments the code adds

- **`edgefull` lines.** Each line gets an `at` field (`check`, `send`, `stage`) and per-isolate running counts of
  `held`, `probe` and `stale`, beside `refused` and `failOpen`.
- **`pushfull.py`.** It reads `advert` as the deciding mark only when it is the push's last mark, so a send-check
  refusal after a Stage round is not counted as admission. This is B0-32's split by shape, with the same mark.
- **Plant 99.** Mutants:
  - hold a Human write on a lapsed advert;
  - no ADVERT_STALE;
  - no probe (hold everything);
  - a probe for every write;
  - the send check on `receive_pack`'s budget, not the send's;
  - no check before the Stage re-send.

### The rerun's guard

- **`guardrow.sh` gets `GUARD_WANT=band` with `GUARD_BAND`, the approved bands.**
  - The bands are 31.0 to 31.5 and 39.7 to 46.1 ms of leader CPU a setup commit. The guard reads the number
    `hostguard.py` prints.
  - It keeps a leader inside a band and re-places one outside.
  - Unknown fails closed.
  - At most three placements, as before. If none lands in a band, the set stops and is reported, and the bands are not
    widened.
- **Each line is judged against its own band's baseline.**
  - Upper band: pfA and pgA.
  - Lower band: blA and blC2 for edge 503s and human p99. For leader refusals, blA and blC2 had admission 5,769 and
    5,842 plus expiry 4,592 and 4,125 (`b033-baseline.md`), but before the drain change.
- **How likely a placement lands in a band.** Of erBA and the 12 B0-33 runs, 6 read in a band, 3 in each. The lower
  band is 0.5 ms wide, about the reading's own spread (pfA read 44.1, 45.6 and 44.6 at 30, 60 and 90 s), so a verdict
  near its edges is fragile. Three placements land in some band about 84% of the time [inference: 6 of 13 a
  placement].
- **Cost.** erBA cost $1.63 a placement. Same stops as before: RUN_STOP $4 a placement, and DAY_STOP at the day's
  start plus $8.

### Text for DESIGN.md, folded into the agent block's existing line

DESIGN.md's agent block, and AGENT-LAYER.md's byte-identical copy, change on the code branch. The sentence that now
ends "and a lapsed advert are sent" becomes:

> Human, MergeQueue, Bot and a token that cannot reach the repository are sent. A push is checked again at each commit
> send against what is left of its bound. On a lapsed advert that said full, an isolate sends one Agent write a chain
> per ADVERT_TTL as a probe, whose answer carries the next advert, and refuses the rest, until ADVERT_STALE (10 s
> [guess]) with none heard.

### Built (2026-11-23, after the lead's ruling)

The lead cleared rules 1 and 2, ADVERT_STALE at 10 s [guess], the band guard, and flip R2b (`decisions/DECISIONS.md`,
"`edge-refusal-bound`").
- **Edge** (`workers/edge/src/lib.rs`): `advert_says` answers `Hold` on a lapsed full advert up to ADVERT_STALE and
  `Stale` past it. `advert_refusal` lets the chain's probe through (`probe_goes`, `graftyard_lookup::take_probe`) and
  refuses the rest with `advert`. `receive_pack` calls it again at the first commit send (`At::Send`) and before the
  Stage re-send (`At::Stage`). `edgefull` lines carry `at`, `ageMs` and the `held`, `probe` and `stale` counts.
- **Readers.** `pushfull.py` splits `edge (advert, at send)` and `edge (advert, at stage)`, and prints flip R2b's push
  half. `edgefull.py` prints flips R1b and R1c and R2b's drain half. Both read erBA's raws unchanged.
- **Guard.** `guardrow.sh` `GUARD_WANT=band` with `GUARD_BAND` logs the band and the baseline before load: pfA/pgA for
  39.7 to 46.1 ms, blA/blC2 for 31.0 to 31.5 ms. The lower band's baselines predate the drain change. No placement in
  a band stops the set without load, which is not a fail.
- **Verdict lines for the rerun** include R2b's two numbers against erBA's: agent `ok` pushes (1,498) and agent
  commands cut a second while refusing (p50 78.7).

## Row B2: the fail-open bound and the send check, hosted (2026-11-23)

Site day -erC from main 01471da2 (test-faults), `siteday exit 0` (`siteday-erC.status`):
- site-a 68bfb824, site-b dcf765b2, site-c c1ff421f;
- edge graft-m2b-edge 32b6bdfb at 100%;
- row B's shard image `sha256:db305d1e…`, built at 050eefc8. That build carries maint-yield at 10 min, not edae3c88's
  6 min.

Between 1d1bc221 (mrB's and msA's -mr build) and 01471da2, the site and container code changed only by `ADVERT_STALE`
(`crates/raft/src/admit.rs`) and `take_probe` (`crates/lookup`). Both serve the edge.

### erC: the band guard found no band host (not a fail)

`GUARD_WANT=band`, at most 3 placements, RUN_STOP $4. All three placements read fast hosts, in neither band
(31.0 to 31.5 ms, 39.7 to 46.1 ms). None got load.

| Run | Repo | ms a setup commit | Cost |
|---|---|---|---|
| erCA | 131401 | 23.3 | $0.11 |
| erCB | 131402 | 27.1 | $0.10 |
| erCC | 131403 | 22.7 | $0.10 |

- Each run tore down and swept; `ROWerCDONE`. erCA's sweep exited 1 on a Cloudflare API handshake timeout
  (`costrow-erCA.status`, paths shown as `~`); rows.sh's teardown had already torn down 0, and erCB's and every later
  sweep exited 0 with no shard container running.
- **erCA's, erCB's and erCC's 5xx, pushfull and edgefull sections hold erBA's data**, not theirs. With no push log of
  its own, b033.sh read the newest one, erBA's. `b033.sh` now skips those readers when a1 exited 4 (this branch).
  erCA's and erCB's edgefull files were dropped; erCC's fetch was stopped by hand and its `B033DONE` appended with a
  note.

### erD: the fast set, the lead's change of plan

`GUARD_WANT=fast` (under 28.4 ms), with mrB's load: `--seed-prs 1000`, 5,000 agents, 300 s, plus `--advert-probe`.
RUN_STOP $10, DAY_STOP $20.74. The baseline is mrB and msA: the same load on fast hosts, on the -mr build.
**erDA is also B0-33's third run**, on newer code.

| Item | erDA |
|---|---|
| Repo, a1 run | 131404, `mwtl5ng2`; first placement kept, 24.7 ms a setup commit (53 commits) |
| Probe | all checks passed; `agentRate` 353.5, `agentWaitMs` 175 |
| Gates before load | learners 4/4 within one entry of 1005; shard gate warm on all three sites at 18:14:58–59Z in 2.3 to 2.8 s, 0 entries, not rehydrating |
| Load (UTC) | 18:21:20 to 18:26:20; done line 18:48:06Z |
| Cost | $3.08 for the run; graft-m2b $16.22 for the day at 18:53Z |
| Sweep | exit 0, "no shard container running"; a final `teardown.py --sweep` tore down 0 |

**edge-refusal-bound's flips, against mrB and msA (baseline) and erBA** [measured; `rows-erDA.status`,
`edgefull-erDA.jsonl.xz`, `pushfull.py`]

| Flip | erDA | mrB | msA | erBA |
|---|---|---|---|---|
| R1a / flip 1: Human, Bot or MQ write refused by the edge | **0** | 0 | 0 | 0 |
| R1b: held past ADVERT_STALE, or a second probe inside ADVERT_TTL | 0, not met | | | |
| R1c: stale sends past held refusals | 4 against 9, not met | | | |
| R2b, push half: agent `ok` pushes | **8,369**, not met | 8,065 | 8,515 | 1,498 |
| R2b, drain half: agent commands cut a second while refusing, p50 | **114.2**, not met | 89.6 | 98.9 | 78.7 |
| Leader-side refusals (admission + `expire_agents` + no commit stage) | **846** | 2,361 | 1,378 | 6,433 (admission + `expire_agents` only) |
| Old flip 2: refusing windows with a leader wait under 2.5 s | 30 of 30 | 19 of 19 | 25 of 25 | |

- **edgefull**: 2,020 lines from 218 isolates.
  - By kind: refused 1,927 push and 1 rest, held 9, probe 79, stale 4, failOpen 84.
  - Where each push met the advert:
    - refused: 77 at the check, 822 at the send, 1,029 at the stage;
    - held: 4 at the check, 5 at the send;
    - probe: 11 at the check, 68 at the send.
  - For comparison, mrB refused 74 with 10 failOpen, and msA 155 with 47.
- **pushfull**: 5,169 agent `full`s.
  - At the edge: `bound_at_send` 2,374, advert at the stage 1,034, advert at the send 836, advert at the check 79.
  - At the leader: admission 379, no commit stage 354, `expire_agents` 113.
- On fast hosts, rule 2 moved the leader's refusals to the edge. Leader-side refusals fell to 846 from mrB's 2,361 and
  msA's 1,378 (36% and 61%). Admission plus `expire_agents` fell to 492 from 1,292 and 1,181. Neither the agent pushes
  completed nor the drain fell, so R2b does not fire.

**B0-33's lines on erDA**, under main 4040b083's rulings (agent line 110/s; `suspect` counts `started` or `won`). Same
method as LM-1's mrB and msA:
- agent-class ok/s is `push:agent` plus `side:*` except `side:human`, codes ok, 200, 201 and 204, over 300 s;
- commit p99 is the push log's `commit;dur` for `kind` human or merge, result ok;
- `suspect` comes from `colos.py`, resets from `resets.py`.

| Line | erDA | mrB | msA | Median of three |
|---|---|---|---|---|
| Human commit stage p99 (n) | 2,628 (80) | 2,603 (99) | 2,196 (95) | **2,603**, within 3,912 |
| `merge` push commit stage p99 (n) | 2,466 (34) | 2,403 (40) | 1,565 (49) | 2,403 |
| `merge` pushes over 2 s | **5 of 34** | 1 of 40 | 0 of 49 | the fail column, on mrB and erDA |
| Agent-class answered ok/s | 115.9 | 107.3 | 106.8 | **107.3**: meets 110 at 90% (the lead's ruling) |
| Leader changes | 0 | 0 | 0 | pass |
| `suspect` started or won | 0 (12 Reported, all `promised`) | 0 | 0 | pass |
| Wasm resets (leader / members) | 0 / 0 | 0 / 0 | 1 / 1 | |
| Edge 503s | 5,228 | 6,077 | 8,333 | the refusal-shape line fails on all three |

**The lead's ruling: erDA is valid, and B0-33 is not met on the three runs.** Human p99, the agent line at 90%, leader
changes and `suspect` pass. Two lines fail: "any merge-queue command waiting over 2 s" (mrB and erDA), and "every
refusal in the git `ng` or REST 429 shape" (all three).

### The git cache `warming` 503s: the home shard's catch-up lag, on every run

Read from site a's `GitShard` events in Workers Logs (`git#0#net:131404`; mrB's `git#0#net:131396`):
- **The shard.** Site a's GitShard, the home shard, serves every merge plan. From load start it ingested flat out:
  each 30 s window summed about 29 s of ingest, nearly every run ended `behind: true`, 200 to 490 packs (9 to 25 MB)
  per window, and `room` waits grew from 1 to 8.7 s a window. erDA's lag lasted from 18:21:20Z to after the fetch's
  end at 18:28:30Z. mrB's curve matches window for window (483, 480, 416, 414, 290 packs, about 29 s of ingest).
- **The merges.** A merge plan whose commits the shard had not ingested waited out `MERGE_WAIT` (15 s) and answered
  `warming` (`crates/git/src/merge_cache.rs`). It failed 17 of 23 PUT merges on erDA (all from the seventh on), 9 of 22
  on mrB and 6 of 23 on msA, each at 15.0 to 16.3 s.
- **The human pushes.** On erDA, 7 human pushes got "no shard could check the pack … `warming`", each holding one of
  a1's 5 human slots 62 s, so 89 human pushes were `skipped: cap` (mrB 58, msA 55).
- **Not the harness or the site day.** The shard gate passed. The image is the one mrB and msA ran. maint-yield holds
  only non-home shards (`graftyard_git::maint_hold` needs `me != home`), and merges run on the home shard [inference
  from code; this row read no `/procs`].
- **Not the code under test.** Agent `ok` pushes, the shard's load, were 8,369 against 8,065 and 8,515.
- The lead takes this to F112 as a G row.

### (a) The merge line's 2 s tail: lane wait, not the round

**The commands behind "merges over 2 s" are Human-class.**
- The line counts a1's `merge` pushes: the admin PAT's git push to an `a1m-` ref before each PR merge
  (`tests/am1/rest/a1.mjs`, `every("merge", …)`). `crate::admission` sends a plain user token's push as Human.
- **No command in the code is classed `MergeQueue`.** `graftyard_git::agent::admit_class` answers Agent, Bot or Human.
  No caller builds `AdmitClass::MergeQueue` outside `crates/raft/tests`.
- The PUT merges themselves (`merge:only`'s 200s) never passed 2 s at the commit stage. Maxima: mrB 1,940 ms (13),
  msA 1,169 (17), erDA 1,812 (6).

So B0-33 measures no merge-queue command [measured from code]. What its fail column read is the Human lane.

Split on the leader's trace (`b033probes/humansplit.py --kind merge` with the push log; each push rides the first cut at
or after its admission, which is the push's `end` less its `commit` stage) [measured, with the clock offset's error]:

| Run | Push (s into load) | Edge commit | Freeze wait | Held | Own round | Apply | Rest (call and return) |
|---|---|---|---|---|---|---|---|
| mrB | +292 | 2,403 | 818 | 164 | 378 | 0 | 1,043 |
| erDA | +108 | 2,016 | 1,197 | 756 | 244 | 0 | −181 |
| erDA | +149 | 2,313 | 928 | 300 | 0 | 0 | 1,085 |
| erDA | +155 | 2,064 | 550 | 858 | 142 | 0 | 514 |
| erDA | +222 | 2,075 | 1,081 | 116 | 380 | 0 | 498 |
| erDA | +240 | 2,466 | 640 | 255 | 0 | 91 | 1,480 |

The columns mean:
- **Freeze wait:** admission to the cut that took the command.
- **Held:** that cut to its entry's first append, frozen behind the entry in flight.
- **Own round:** the append to the first voter ack.
- **Rest:** the edge's `commit` stage less the leader's part (the call to the leader and the outcome's return).

- **The round is never the tail.** Own rounds were 0 to 380 ms. Entry rounds ran p50 321 and p99 1,507 on erDA, and p99
  1,000 and 772 on mrB and msA.
- **The tail is lane wait plus transport.** Freeze wait plus held was 0.98 to 1.95 s, and 0.5 to 1.5 s of rest
  remained.
- **erDA's leader froze longer.** Batches cut while an entry was in flight then stayed frozen p50 222 and p99 1,687 ms
  (mrB 130 / 1,000, msA 106 / 758). A frozen batch waited 62% of wall time (mrB 47%, msA 46%), and 78% of erDA's Humans
  rode an entry holding 64 agent commands (mrB 28%).
- That fits a leader whose CPU went more to agent entries: 24.7 ms a setup commit, and load-time confirms p50 78 ms
  against mrB's 68 [inference].
- **Read against the column:** it was written when the commit line was 1 s. A Human or merge-queue command's commit
  under load is now judged by the 3,912 line, which these pushes pass. The 2 s tail is queue time in the Human lane
  and the edge's call, not a slow round.

### (b) erDA's 5,228 edge 503s

From the edge's `fivexx` lines (`fivexxwhy.py`, route and reason), joined to a1's per-kind codes, whose token gives the
caller class [measured]. a1's distinct 503s are 5,255 (it counts an upload-pack's 503 on both `read:ref` and
`read:ref:front`, and a merge's on `merge` and `merge:only`): 39 more upload-pack 503s than the edge logged (254 against
215) and 12 fewer GraphQL ones (2,586 against 2,598), whose class is not known. The table takes the edge's counts.

| Class | Route | Write? | Reason | Rule | `Retry-After` | n |
|---|---|---|---|---|---|---|
| Agent (App) | GraphQL `addComment` | write | tier rows: every member shed the read | F65's member shed (`OVERLOADED`) | 1 | 1,844 |
| Agent (App) | GraphQL `createPullRequest` (draft) | write | same | F65 shed | 1 | 436 |
| Agent (App) | REST labels POST/DELETE | write | same | F65 shed | 1 | 1,893 |
| Agent (App) | task PATCH (`side:taskEdit`, `side:standIn`) | write | same | F65 shed | 1 | 295 |
| Agent (App) | lease renew | write | same | F65 shed | 1 | 154 |
| Agent (App) | lease acquire and release | write | same | F65 shed | 1 | 47 |
| Human (admin PAT) | GraphQL `addComment` (`side:human`) | write | same | F65 shed | 1 | 157 |
| Human (member PAT) | GraphQL `addComment` (`fast-user`) | write | same | F65 shed | 1 | 149 |
| Human (admin PAT) | PUT merge | write | the git cache: `warming` | MERGE_WAIT on the home shard | 5 (`unavailable`) | 17 |
| Human (admin PAT) | receive-pack | write | no shard could check the pack: `warming` | shard validate, `VALIDATE_WAIT_HOME_S` | 2 (`shard_unavailable`) | 7 |
| Human (admin PAT) | info/refs | read | access rows: every member shed the read | F65 shed, at the read gate | 1 | 2 |
| Human (admin PAT) | upload-pack `ls-refs` | read | git cache: `warming` | shard catch-up (`warming()`) | **none** (`text`, lib.rs) | 215 |

- **Agent: 4,669, all writes, all F65 member sheds with `Retry-After: 1`, none a 429.**
- **Human: 547.** Shed writes 306, `warming` writes 24, `warming` reads 215, shed reads 2.
- **Bot: 0**, since the row has no Bot token. a1's two "adv 503" human pushes are the 2 info-refs shed reads
  [inference: same count, same route].
- The one 503 with no `Retry-After` is the upload-pack `warming` path: the shard read's non-200 branch in
  `workers/edge/src/lib.rs` (`git cache: {status} {why}`), sent with `text`.
- None of the 5,228 is the leader's `full`. Those already answer 429 (`done_reply`); a1 counted 0 API 429s on erDA.

### The smallest change that puts every agent-class refusal in the 429 shape (design only, at $0)

**The rule.** When the members shed (`OVERLOADED`) a tier or command-rows read for a write the edge classes Agent,
answer `Reply::secondary_rate_limit(1)` instead of `read_failed`'s 503. This is `gate_ng`'s rule for git pushes at the
read gate (`workers/edge/src/lib.rs`), applied to the REST, GraphQL and task API writes. Every other failed read keeps
its 503 with `Retry-After`.

**Where.** Where a write's row read returns its error with the principal in hand: `read_repo`, which REST, GraphQL and
the lease routes reach (`crate::api::read`), and the task path's `command_tier_rows` join (`workers/edge/src/api.rs`).
One helper, `shed_reply(c, who, reply)`, turns a 503 whose reason carries `OVERLOADED` into the 429. `read_failed`
keeps its shape for every other caller.
- It answers 429 only when `c.write` holds, the error carries `OVERLOADED`, and the viewer would be classed Agent.
  `crate::admission` reads that, which for an App token is the install's `AgentApp` rows; `advert_refusal` already
  calls it on the same writes.
- It is a no-op unless the viewer is an App, or a user token with an App or `X-Graftyard-Agent-Session`, the same
  `bounded` test the push uses. So it reads nothing new for a plain user token.
- GraphQL returns `read_repo`'s error `Reply` as its own HTTP status (`graphql_post`, `Err(r) => return Ok(r)`), so the
  same 429 reaches `addComment` and the draft-PR mutation. These routes already answer edge-refusal's advert refusal
  this way (`read_repo`), so the change adds no new shape, only a new cause for it.

**The GHES invariant holds.**
- A client that does not opt in gets no 429 here. A plain PAT is Human: `admission` returns `None` before any read.
- An App token not marked `AgentApp` is Bot and keeps its 503.
- Only the agent layer's own principals change. AGENTS.md's byte-diff row reads GHES 3.22 responses for non-opted-in
  clients, which this leaves byte for byte.

**What it would have done on erDA** [inference, counterfactual on the table above]:
- 4,669 agent 503s become 429 with `Retry-After: 1`, and the refusal-shape line's agent half passes. The remaining 559
  are Human.
- **The line still fails as written**, because it reads "every refusal". 306 Human shed writes, 24 Human `warming`
  writes, 2 Human shed reads and 215 Human `warming` reads stay 503.
- Making those 429 would need a Human-class refusal shape, which the design forbids: Human writes are never refused
  this way. They are not refusals in the design's sense but unavailability with `Retry-After`.
- The 215 upload-pack `warming` 503s lack even `Retry-After`. Adding `retry-after: 1` there is a one-line fix in
  that non-200 branch. It changes a 503's headers for every client, as GHES does on its own 503s
  [unverified for GHES 3.22's upload-pack], so it needs the byte-diff row's check before it ships.
- **Proposal:** the lead reads the line as "every agent-class refusal in the git `ng` or REST 429 shape; Human and Bot
  unavailability as 503 with `Retry-After`". That is an operator decision, not an edit (`FRAME.md`).

**Cost on B0-33's other lines** [inference]:
- **Agent ok/s: none directly.** The 4,669 were refused either way. a1 retries neither shape within a sample, and a
  real agent backs off on 429 as on 503.
- **Leader load: none.** The member shed already happened, and the 429 is decided after it, at the edge, from the
  same error.
- **Human p99 and merges: none.** Human writes keep their path.
- **Edge reads: none new.** The install's `Authz` is already read on these paths (`read_repo`'s advert check, the
  task path's own join) and kept for the request (`Ctx::install`).
- **`suspect` and leader changes: none.**
- **The real cost** is that an agent client which treats 429 as a rate limit may back off longer than `Retry-After`
  says. These routes already answer the advert's refusal as this 429, so agent clients already meet it [inference].

**What would show it wrong:**
- a Human or Bot write answered 429 by it, which is an invariant, as flip 1 is;
- a shed error whose write committed anyway. It cannot: the shed comes before the command.

## Open

- **The ruled B0-33 lines are not met.** "Any merge-queue command waiting over 2 s" read Human-class pushes, because no
  command is classed MergeQueue. The refusal-shape line fails on Human unavailability as well as on agent sheds. Both
  are the lead's to read (the section above).
- **The rerun of the fail-open bound and the send check** ran as Row B2 (above).
- Whether human p99 falls on a slow host is the row's question, not this file's. Edge refusal sheds agent work but does
  not cut apply CPU per entry at the same drain [inference].
