# The far leg under adaptive append: hosted A/B (2026-11-14)

The operator's item from LM-1's m25 A/B (lm1-memory round 13): with `m0-adaptive-append`, the far follower's
AppendEntries p99 rose from 459 to 701 ms on fresh-repository journey 9, and re-sends from 4.6% to 15.5%. Explain it,
fix it if the re-sends are the cost, and merge adaptive only if its far p99 and commit p99 aren't worse beyond the
run spread. Two runs per arm, journey 9's mix, traces on. Branch `m0-adaptive-append`, rebased onto main `0f09638`.

## Verdict

**Adaptive append is not worse; its commit p99 is lower** [measured, `graft-m3d`, one pair per arm plus a second
main pair]. On journey 9's mix at 30 commands/s:

| Arm | Run | Commit p50 / p99 (ms) | Round p50 / p99 | Leader confirm p99 | Far call p99 (pair) | Near call p99 (pair) | Commands per entry |
|---|---|---|---|---|---|---|---|
| main `0f09638` | r30-1 | 218 / 577 | 131 / 244 | 42 | 373 | 247 | 4.2 |
| main `0f09638` | r30-2 | 217 / 474 | 130 / 239 | 67 | | | 4.2 |
| main `0f09638`, second pair | r30-1 | 202 / 516 | 123 / 229 | 36 | 682 | 227 | 3.9 |
| main `0f09638`, second pair | r30-2 | 192 / 469 | 116 / 221 | 30 | | | 3.7 |
| adaptive `510d6c1` | r30-1 | 191 / 409 | 117 / 210 | 40 | 594 | 212 | 3.7 |
| adaptive `510d6c1` | r30-2 | 190 / 416 | 116 / 213 | 29 | | | 3.7 |

- **Commit p99:** adaptive 409 and 416 ms, against main's 469 to 577 ms over four runs. Lower in both runs, and
  under the lowest main run by 53 ms.
- **Far call p99:** adaptive 594 ms, inside main's two pairs on the same code, 373 and 682 ms. The same tree moved
  309 ms from one pair to the next, so LM-1's +242 ms (one run per arm on different deploys) is within that spread
  [measured]. The far p50 is 264 ms on adaptive and 185 and 265 ms on main's pairs.
- **Near call p99:** 212 ms on adaptive, 247 and 227 ms on main.
- **Re-sends** are 2.6% of far calls on adaptive, against 0.9% and 3.7% on main's pairs. A re-send cost p99 839 ms
  on adaptive and 327 and 868 ms on main. They are not the cost of the far p99: first sends alone are p99 571 ms.

Every run carried 28 commands/s with 0 failed, the leader on site 0 (enam) at term 2, and no leader change.

## Where the far call's time goes

From the leader's trace (`python3 tests/m0carried/farsplit.py tests/m0/results/raw-farleg far-main-r30
far-adapt-r30`, and `.../raw-farleg/same-code far-main2-r30`).

**By call size** (p50 / p99 ms):

| Size | main pair 1 | main pair 2 | adaptive |
|---|---|---|---|
| under 20 KB | 171 / 257 | 241 / 427 | 243 / 457 |
| 20 to 40 KB | 193 / 322 | 269 / 515 | 266 / 494 |
| 40 to 60 KB | 208 / 257 | 298 / 483 | 290 / 553 |
| 60 to 80 KB | 228 / 502 | 325 / 678 | 333 / 654 |
| 80 KB and over | 255 / 632 | 386 / 868 | 371 / 863 |

The far link's time rises with call size on every arm. Adaptive matches main's second pair in every size bin, so the
pair-to-pair difference is the link on the day, not the code [inference]. Main's first pair had the faster link.

**By far calls already out at issue** (p50 / p99 ms): adaptive 266 / 534 at 0, 257 / 549 at 1, 263 / 539 at 2,
298 / 731 at 3, 345 / 746 at 4 (n 28). Main's second pair: 307 / 822, 267 / 627, 260 / 572, 318 / 771, and 317 / 435
at 4 (n 3). A third or fourth call out costs about 50 to 80 ms more at p50 on both trees. Adaptive sends more calls
with 2 or more out (1,429 of 1,940 against 1,306 of 1,849), as its catch-up pipelining does, and the far p99 does not
rise with that [measured].

**Re-send causes (adaptive, from `AppendAnswered`):** 1,809 ok answers and 131 refused for a gap (a call that arrived
before the call ahead of it, 6.8% of far calls), none refused by `verify` or otherwise. Of 51 re-sends: 27 followed
the call ahead answering (the dropped calls behind a gap refusal go again), 10 a gap refusal directly, 14 other (no
answer within 5 ms before). No re-send followed a transport failure or a call out 1 s. Main has no `AppendAnswered`
row, so its causes are by timing only: 4 and 17 after the call ahead answered, 12 and 52 other.

LM-1's 15.5% re-sends do not reproduce at this load; its runs carried journey 9 at 500 agents with learners and the
dependency reads, which this bench does not have.

**The near leg** (LM-1's round 16 puts it on the critical path: round p99 746 ms, near 687, far 867, at 500 agents):
- Here the near call is p50 116 to 130 ms and p99 212 to 247 ms on every arm, and the round p99 is 210 to 244 ms, so
  at 30 commands/s the round is the near call.
- By size, near calls of 80 KB and over take p99 504 and 523 ms on main's pairs and 232 ms on adaptive; under 60 KB
  they stay under 213 ms on every arm.
- Far calls out do not slow the near call: on adaptive, near p99 is 196 to 225 ms with 1 to 4 far calls out.
- Near calls almost never overlap each other (5 of 1,943 with one already out), so the near leg is one call a round.

So the near leg's tail at this load is its large calls. B0-29's 687 ms near p99 at 500 agents is outside what these
runs reach; LM-1's 600 s gate row is splitting it.

## The merge with narrow-reads-3 (`send_one`)

The rebase onto main put adaptive's per-call loop beside narrow-reads-3's learner pipelining for reads. In
`crates/raft/src/member/leader.rs`:
- `send_append` calls `send_one(peer, force, false)`, then repeats `send_one(peer, false, false)` while it sends, up
  to PIPELINE calls each within the link's bound (adaptive).
- `append_for_read` calls `send_one(peer, false, true)`: `read` pipelines a learner as a voter (narrow-reads-3's
  `append_to(peer, false, true)`).
- `send_one` takes adaptive's `pipe_lossy` and `bound` and narrow-reads-3's `voter = read || is_voter`.
- `on_append_res`: adaptive's reorder rule keeps the calls ahead of a gap-refused call. Main also emptied the pipe on
  a refusal of a call no longer in it, and adaptive's rewrite lost that; it is restored (`else if !res.ok`).

What covers it:
- `a_learner_read_index_answer_follows_the_entries_it_needs` (`crates/raft/tests/scenarios.rs`, narrow-reads-3)
  covers the learner pipelining in `send_one`: the call that follows one in flight, the call from the first
  unacknowledged entry after a refusal emptied the pipe, three calls to a learner three calls' bytes behind, and none
  on a lossy link. It failed on the first merge, without the restored refusal rule, and passes now.
- `tests/m0carried/plant9.py` mutants 12 and 13: "a learner is not pipelined for a read" (`voter` without `read`) and
  "a refusal of a call no longer in the pipe keeps it". Both caught; 13 fails exactly that test.

## Runs and checks on the rebased tree (`510d6c1`, on main `0f09638`)

- **Raft tests:** pass.
- **Plants** (`python3 tests/m0carried/plant9.py`): 14 of 14 caught.
- **Long sweep** (`RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=...`): passes. Longest stretch with a healthy quorum,
  requests waiting and none completing: HARSH 16.4 s (seed 10499), GRAY 24.4 s (seed 11626), live 24.7 s (seed
  11968, 4,000 seeds from 10000), against main's 15.7, 25.2 and 23.8 s (`tests/m0/results/looplag-handoff.md`), each
  under the absolute 30 s bound.
- **Porcupine** (`go run . -seg 2000`): Ok on all three histories, 167,225, 2,221,859 and 5,900,987 operations.
- **Full checks:** pass. fmt; workspace clippy; wasm32 clippy for site and edge with and without `test-faults`, and
  ops; `cargo test --workspace` 879 tests in 78 suites, none failing; `go test` in `containers/git/server`; and the
  three `scripts/check_*.py`.

## Method

- `tests/m0carried/farleg.sh`: one site day of the arm's tree on `graft-m3d` (`SHARD_INSTANCE=standard-4`,
  `--test-faults`, image `sha256:15f6ed1b…`), the bench edge redeployed, then two 120 s runs of
  `node tests/m1/wp2/run.ts bench --rate 30 --arrival mem --mix j9`, each on a new group, with `BENCH_RAW=1`.
- `--mix j9` (`tests/m1/wp2/edge.ts`, `benchValue`): journey 9's group commands as the follower links see them. 4.17
  of 27.6 commands/s are agent pushes with a 10 to 70 KB pack inline, 5% of those not inline; the rest about 1 KB.
- `Trace::AppendAnswered = 20` (`workers/site/src/host.rs`, test-faults builds): each traced call's reply code (0 ok,
  1 gap, 2 `verify`, 3 other) and first entry. Appended after narrow-reads-3's 17 to 19.
- **A mistake, and what it gave.** The first adaptive site day reused main's wasm: both arms shared one Cargo target
  directory, and worker-build finished in 0.11 s. Its trace had no `AppendAnswered` row, which showed it. Those two
  runs are a second pair on main's code, kept as `tests/m0/results/raw-farleg/same-code/far-main2-*`, and are the
  spread measure above. `farleg.sh` now builds each arm in its own target directory, and the adaptive arm was rerun.
- Raw rows: `tests/m0/results/raw-farleg/`.

## Cost

`graft-m3d` from 10:49Z to 12:00Z cost $0.33 (`INSTALL=graft-m3d python3 tests/fanout/m2bcost.py
2026-11-14T10:49:00Z 2026-11-14T12:00:00Z`): three site days and six runs. With the earlier $32, this thread's hosted
spend is about $32.3 of the $55 cap.
