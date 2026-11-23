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

## Open

- The hosted row waits for LM-1's apply cut and its row (the lead's A-then-B order for B0-33).
- Whether human p99 falls on a slow host is the row's question, not this file's. Edge refusal sheds agent work but does
  not cut apply CPU per entry at the same drain [inference].
