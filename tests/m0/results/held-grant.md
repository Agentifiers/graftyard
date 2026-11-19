# held-grant: the merge's slow tier read is the leader refusing its own held read (2026-11-19)

Code only, no hosted run, no deploy. Branch `leader-keep` from main 3e63a8df. This follows item 5 of
`tests/m0/results/b029-tail.md` (section 3), which found that each slow REST merge `tier_ahead` walked the followers. The
traces are B0-29 run 7, A-B-A row 1 and the within-network control (`tests/fanout/raw/tsave-m2bb29{h,j,n}.jsonl.xz`).

## The hypothesis was wrong [measured]

The hypothesis was that a learner's answer naming no leader at the current term clears the edge's cached leader, so the
next merge-rows read walks s1 → s2 → s0. A scratch lookup test shows the cache is cleared that way. But that path would
start with a `Walk` post, and none of the slow lookups did. Grouping each edge isolate's merge-rows posts into lookups by
`SentBy` gives:

| | lookup shapes |
|---|---|
| run 7 | s0L ×70, s0L s0L ×2, s0W ×2 (cold isolates), s0L s1W s2W s0L ×1 |
| row 1 | s0L ×48, s0W ×1, s0L s0L ×1, s0L s1W s2W s0L ×1 |
| control | s0L ×67, s0W ×1, s0L s1W s2W s0L ×2, s0L s1W s2W s0L s1W s2W s0L ×1 |

Every slow merge first posted the leader it held, s0, which answered a 200 in 28–32 ms by Workers Logs, and only then
walked. In `graftyard_lookup::lookup`, a 200 that makes the leader's post walk is `{"error": {"notLeader": ..}}`.

## The real cause [measured]

`Host::release` answered a granted read through `Member::fresh`, which is good only within the grant's own input. A
leader read's output is held for the deferred transaction (`Host::dispatch`) when an earlier input wrote and sent
AppendEntries early. Any input that steps the member before that flush (a `/raft` call, an answer to its own call,
another read, a write) changes `step`. So `fresh` refused the grant `notLeader`, naming the leader itself, and the edge
walked.

`tests/m0/cuttrace/heldgrant.py` orders site 0's trace rows; one isolate, so log order is execution order. For each
leader plain read it marks whether the read is held (after a deferring `Stepped` row, before its `Flushed`) and whether
another input stepped first, then counts the isolates that posted a `Walk` within 2 s:

| read | run 7 | row 1 | control |
|---|---|---|---|
| a merge's rows (`rows`, code 3), held and stepped past | 1 of 1 walked | 1 of 1 | 4 of 4 |
| a merge's rows, held, nothing stepped first | 0 of 1 | 0 of 0 | 0 of 3 |
| a merge's rows, not held | 0 of 75 | 0 of 51 | 0 of 68 |
| `prsync` (code 4), held and stepped past | 0 of 0 | 0 of 0 | 1 of 1 |
| `ranges` (code 13), held and stepped past | 234 of 244 | 91 of 92 | 193 of 195 |
| `ranges`, held, nothing stepped first | 50 of 135 | 17 of 81 | 36 of 121 |
| `ranges`, not held | 1,143 of 4,575 | 399 of 2,862 | 784 of 4,332 |

- The six slow merges in the three traces are exactly the six merge reads that were held and stepped past.
- `ranges` reads walk for other reasons too (about 25% when not held), but 96–99% walk when held and stepped past.
- That class is 3–5% of the leader's plain reads: 257 of 5,482, 95 of 3,359 and 209 of 5,131.
- The leader's confirms, a dump rebuild and LM-1's read gate are not involved. The refusal happens after the grant, in
  the release.
- For LM-1's shed-gate logging: these never reach `gate`. They are counted nowhere on `/stats` today.

## The fix

`Host::release` answers through `released_view`, which is `Member::fresh_after`: the member's state when the output is
released, at or above the grant's floor, refused after a restart. That rule already serves a follower's or a learner's
late answer (`narrow-reads-3`). Design text: `design/DESIGN.md`, Shape, "The read path". Decision:
`decisions/DECISIONS.md`, "`held-grant`".

The fix is in `workers/site`, not `workers/edge`, as the brief expected; the edge was doing the right thing with a wrong
answer. The safety rule the brief asked to keep still holds at the edge: `lookup` still drops a leader that answers
`notLeader`, and `learn_leader` drops one whose answer is from a term below the highest seen. Neither changed.

## Checks

- Regression test: `host::tests::a_read_released_after_later_inputs_answers_at_the_state_then`. It fails on main's
  `fresh`.
- Plants: `python3 tests/m0carried/plant49.py`, 3 of 3 caught (main's rule; the release calling `fresh` directly; a
  grant answering across a restart).
- Full checks (`tests/am1/runs/checks.sh`): fmt, workspace clippy, wasm32 clippy for site and edge with and without
  `test-faults` and ops, `cargo test --workspace` (1,229 passed, 0 failed), `go test` in `containers/git/server`, and the
  three design checks: all pass. Nothing in `crates/raft` changed, so no sweep or Porcupine run.

## Predicted effect [inference]

Capped at 400 ms, `tier_ahead` takes merge p99 from 1,782 / 1,596 / 2,862 to about 1,393 / 1,342 / 1,759
(`b029-tail.md`, section 3). Each removed refusal also saves the edge two follower posts and one more leader post. The
next B0-29 row on an edge day measures it; the flip is in the decision.
