# B0-02: a pipeline to each voter, a loop-lag step-down, and entries and calls sized from the data (2026-11-13)

The operator's decisions on `carried-7.md` (C20): check the near-follower gaps of `m0c7b-ab11`, `ab02`, `ab14` and
`ab16` for the far path's own limits; build (b'), AppendEntries pipelined to a voter, if the far path waited on one call
per peer or on the output gate; build (2b), a leader that steps down when its own loop is behind; then the gate,
8 of 8 one-minute runs at 300 offered and 8 of 8 at 330, commit p99 at most 600 ms, `arrival=mem`, standard-4 shard
containers, a new group per run, no window selection. Branch `m0-carried-8`; install `graft-m3d`.

## Verdict

**B0-02: passes, 16 of 16, on the final tree, with one open item** (`a034334` on main `84d29c2`): 8 of 8 at 300 offered and 8 of 8 at 330,
commit p99 249 to 579 ms, worst 579. The tree adds two fixes to the 12-of-16 tree below, both by operator decision:
`ENTRY_SOFT` at 100,000 bytes, and `APPEND_BYTES_MAX` at 100,000 bytes with pipelining stopped only on a lossy link.
On the same tree, the long sweep (live half 4,000 seeds from 10000 at an absolute 30 s), Porcupine, `long_liveness`,
plants (15 of 16; the miss is below) and the full checks pass. Heavy turns did not come back with the larger
entries. Open: in r300-3 a `LOOP_LAG` trip at the run's start moved leadership to weur, and from there the apac voter
was lossy and fell 215 entries behind; `APPEND_BYTES_MAX` was sized on the enam to apac link only.

## Gate on the final tree (`a034334` on main `84d29c2`)

Site day 15:44 to 15:54Z, `SHARD_INSTANCE=standard-4`, image `sha256:15f6ed1b…`; runs 15:55 to 16:30Z, each on a new
group, `arrival=mem`, rates alternating run by run, no window selection. 0 failed commands. CPU from
`durableObjectsInvocationsAdaptiveGroups`, heavy turns from Workers observability, both for the leader's object over
the run's minute (`tests/m0carried/gate8.py`).

| Run | Throughput | Commit p99 (ms) | Leader confirm p99 (ms) | Commands per entry | CPU p999 (ms) | 80 ms+ `/commit` turns | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |
| r300-1 | 280 | 291 | 42 | 38.5 | 25.2 | 0 | Pass |
| r300-2 | 280 | 385 | 53 | 47.7 | 26.2 | 1 | Pass |
| r300-3 | 260 | 554 | 107 | 46.4 | 32.0 | 1 | Pass |
| r300-4 | 275 | 341 | 62 | 41.7 | 30.5 | 3 | Pass |
| r300-5 | 279 | 579 | 176 | 49.7 | 43.9 | 4 | Pass |
| r300-6 | 280 | 427 | 106 | 44.2 | 49.3 | 1 | Pass |
| r300-7 | 280 | 526 | 60 | 54.5 | 23.3 | 0 | Pass |
| r300-8 | 278 | 350 | 71 | 38.4 | 26.9 | 1 | Pass |
| r330-1 | 307 | 520 | 74 | 57.4 | 26.5 | 0 | Pass |
| r330-2 | 307 | 559 | 87 | 61.7 | 31.2 | 3 | Pass |
| r330-3 | 308 | 490 | 88 | 49.9 | 30.4 | 4 | Pass |
| r330-4 | 308 | 332 | 62 | 39.8 | 25.6 | 1 | Pass |
| r330-5 | 307 | 354 | 54 | 44.7 | 23.8 | 2 | Pass |
| r330-6 | 307 | 493 | 125 | 46.6 | 36.4 | 3 | Pass |
| r330-7 | 308 | 459 | 125 | 47.3 | 35.2 | 4 | Pass |
| r330-8 | 307 | 454 | 75 | 50.1 | 27.6 | 0 | Pass |

- **Heavy turns.** 28 `/commit` turns of 80 ms CPU or more over 16 runs (0 to 4 a run), against 24 on the 60 KB tree
  (`m0c8f`, 0 to 6 a run). CPU p999 was 23 to 49 ms, median 30, against 15 to 56 ms, median 27, on the 60 KB tree
  (one outlier there at 850 ms, r300-1). `carried-7`'s uncapped smoke at 85 commands an entry had 11 heavy turns in
  20 s and CPU p999 98 ms; at 38 to 62 commands an entry here, neither came back. No run's heavy turns tracked its
  commit p99: the two worst runs had 4 (r300-5, 579 ms) and 1 (r300-3, 554 ms).
- **Confirm p99 by entry size**, over all 16 runs: 0 to 40 KB 76 ms (n 1,300), 40 to 60 KB 102 ms (4,086), 60 to
  80 KB 117 ms (891), 80 to 100 KB 108 ms (160), 100 to 130 KB 131 ms (70). The 60 KB tree, for comparison: 78, 76
  and 128 ms up to 80 KB, and no larger entries. `carried-6` had 168 ms at 80 to 100 KB and 300 ms at 100 to 130 KB on
  standard-2 shards [measured]. Per-run bins are in `raw-carried-8/gate8-m0c8h.jsonl`.
- **r300-3: a `LOOP_LAG` step-down in the run's first second.** Site 0's first confirms read 300, 200, 259 and 205 ms;
  two whole ticks within 2 s tripped `LoopLag` 64 s before the run's end, and leadership moved to site 1 (weur), which
  led the rest of the run: 260 commands/s, commit p99 554 ms, 1 failed command. From weur the far voter (apac, RTT
  255 ms) was the slow link all run: 31 of its bodies took 530 to 1,798 ms to arrive at 45 to 102 KB, and 6 of the 32 calls the
  leader's `peers` ring kept (the run's last 10 s) missed `CALL_DEADLINE` and answered late, 1.0 to 2.3 s. That share
  (19%) passes `commit_errors`, so the link counted as lossy and the leader sent it one call at a time, as rule 13
  intends; the near voter (enam, RTT 98 ms) made every quorum. The apac voter ended the run 215 entries behind (178
  against 393), catching up one call at a time. `APPEND_BYTES_MAX` was sized on enam to apac; weur to apac is slower
  at the same sizes [measured]. The run passed, but a run that loses its near voter while led from weur would commit
  on that link.
- **Shape 3** did not recur: no run's first sixth stood out apart from r300-3's step-down.

Raw: `raw-carried-8/` (bench rows and gzipped `/stats` for `m0c8h-*`, and the gate8 reports for `m0c8h` and `m0c8f`).

## Simulator and checks on the final tree

- Long sweep, `RAFT_SEEDS=2000 RAFT_STEPS=10000`, live half 4,000 seeds from 10000 at the absolute 30 s bound: passed.
  Longest stretches 15.7 s (harsh, seed 11431), 27.2 s (gray, 11551), 24.3 s (live, 10361). Porcupine `-seg 2000`: Ok
  on all three histories (6,000, 6,000, 12,000 segments).
- `long_liveness` (60 seeds) and every raft suite: pass.
- Plants, `tests/m0carried/plant8.py`: 15 of 16 caught. Missed: "the entry cap back at 60 KB". No simulator test
  separates 60 from 100 KB: the load net's storage confirms at once and its links cost 0.3 ms a KB, so its round does
  not grow with the entry the way a Durable Object's confirm and the far body do; a B0-02-shaped run at 330/s gave
  240 ms at both. The hosted gates are the evidence (12 of 16 at 60 KB, 16 of 16 at 100 KB).
- Kept tests added: `a_far_voter_behind_catches_up_in_calls_that_answer_in_time` (14 entries behind, 0 misses; fails
  at 1.5 MB); `a_voter_is_pipelined_unless_its_link_is_lossy` (one miss keeps PIPELINE calls, a lossy link gets 1);
  `a_lagging_follower_catches_up_in_calls_under_the_byte_cap` now asserts that every call starting behind the
  leader's last entry carries one, with entries four times `APPEND_BYTES_MAX` (fails if a call over the bound may
  carry none).
- Full checks: fmt, workspace clippy, wasm32 clippy for site and edge with and without `test-faults` and for ops,
  `cargo test --workspace` (78 suites), `go test ./...`, and the three `scripts/check_*.py`: pass.

## Earlier gate on main `84d29c2` before the two fixes (12 of 16)


Site day 13:30 to 13:41Z, `SHARD_INSTANCE=standard-4`, image `sha256:15f6ed1b…`; runs 13:43 to 14:15Z, each on a new
group, `arrival=mem`, rates alternating run by run, no window selection. 0 failed commands in all 16 runs.

| Run | Throughput | Commit p99 (ms) | Leader confirm p99 (ms) | Commands per entry | Round p50 (ms) | Near entries at the cap | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |
| r300-1 | 280 | 425 | 89 | 49.7 | 168 | 40% | Pass |
| r300-2 | 280 | 564 | 58 | 41.6 | 138 | 8% | Pass |
| r300-3 | 280 | 319 | 39 | 43.8 | 149 | 6% | Pass |
| r300-4 | 280 | 452 | 68 | 41.4 | 137 | 6% | Pass |
| r300-5 | 280 | 328 | 37 | 44.2 | 148 | 8% | Pass |
| r300-6 | 278 | 261 | 96 | 32.5 | 109 | 1% | Pass |
| r300-7 | 280 | 377 | 84 | 45.3 | 152 | 13% | Pass |
| r300-8 | 280 | **605** | 122 | 44.8 | 149 | 15% | Between |
| r330-1 | 301 | **646** | 58 | 54.5 | 169 | 82% | Between |
| r330-2 | 302 | 339 | 63 | 47.5 | 145 | 22% | Pass |
| r330-3 | 308 | 343 | 34 | 46.9 | 144 | 23% | Pass |
| r330-4 | 300 | **1,495** | 147 | 55.3 | 174 | 91% | Fail |
| r330-5 | 308 | 393 | 60 | 50.4 | 154 | 39% | Pass |
| r330-6 | 247 | **1,635** | 67 | 50.3 | 339 | 31% | Fail |
| r330-7 | 308 | 374 | 77 | 44.6 | 136 | 19% | Pass |
| r330-8 | 308 | 583 | 87 | 54.4 | 165 | 79% | Pass |

"Near entries at the cap" is the share of the leader's AppendEntries to the near follower of 58 KB or more, which is
`ENTRY_SOFT` full. No run had a near call of 400 ms or more except r330-6, and no run tripped `LOOP_LAG` or had a
clamped leader (clamp share 0.02 to 0.03).

### Three failure shapes

1. **Capacity under the cap (r330-4, r330-1; and r330-8 at 583, the diagnostic r330-4 at 593).** With one entry in
   flight, a group commits at most one `ENTRY_SOFT` entry per round: about 56 commands of 1 KB per round. At a round p50
   of 165 to 174 ms (near RTT 95 to 108 ms) that is 322 to 339 commands/s, against 300 to 308 offered, a utilization of
   0.91 to 0.93. Entries then fill to the cap (79 to 91%) and the queue grows: in r330-4 the queue p90 per sixth of the
   run went 311, 608, 986, 369, 688, 914 ms. Over all 32 runs on this code, every run at a utilization of 0.91 or more
   had commit p99 of 583 ms or more, and every run at 0.85 or less had 452 ms or less, apart from r300-8 (605) and
   r300-2 (564) in shape 3. This is the cost the cap decision's flip names ("capped entries do not lower commit p99"), met at 330 on a slow
   near round [measured].
2. **A far follower that falls behind and stays behind (r330-6).** The far follower's 40 to 60 KB calls took 300 to
   670 ms, longer than four entry intervals, so the pipeline filled (4 calls out) and the next call carried several
   entries. Calls of 216 and 304 KB took 808 and 892 ms, and from 585 KB up they missed `CALL_DEADLINE` (1 s) and
   answered late. 12 far calls of 319 to 717 KB carrying 7 to 14 entries each missed it. A missed call turns off pipelining
   to that voter for the `STEP_DOWN` window (`failed_to`), so each next call carried the whole backlog and missed again.
   When the near follower then slowed (6 calls of 400 ms or more), `CommitErrors` tripped `STEP_DOWN` 46 s into the run
   and moved leadership to the far site, where the round p50 was 339 ms and throughput fell to 247 commands/s [measured].
   The transfer is unguarded by rule 13, which covers only `/transfer` and drain.
3. **One slow stretch at low utilization (r300-8 at 605).** Commit p99 in the first sixth of r300-8 was 691 ms and 350
   to 453 ms after it; the leader's confirms ran 122 to 172 ms in its first 5 s. r300-2 (564, passing) had the same kind
   of stretch in its third sixth (652 ms). Neither had a slow near call or a clamped leader [measured; cause not found].

Raw: `raw-carried-8/` (bench rows and gzipped `/stats` for `m0c8f-*`, `m0c8g-*` and the smoke `m0c8-smoke`).

### Simulator on that tree

- Long sweep, `RAFT_SEEDS=2000 RAFT_STEPS=10000`, live half 4,000 seeds from 10000 at the absolute 30 s bound:
  passed. Longest stretches 16.4 s (harsh, seed 10100), 25.7 s (gray, 11247), 23.0 s (live, 10249). Porcupine
  `-seg 2000`: Ok on all three histories (6,000, 6,000, 12,000 segments).
- `long_liveness` (60 seeds) and every raft suite: pass.
- Plants, `tests/m0carried/plant8.py`: 11 of 11 caught. Two earlier mutants were dropped as equivalent: "a failed call
  leaves the pipe" (the retry is a forced call, which clears it) and "a forced call keeps the pipe" (each lost call's
  failure report clears it). "Pipelining to a voter with a failed call in the window" replaced them; the new kept test
  `a_voter_with_a_failed_call_gets_one_call_at_a_time` catches it.
- Full checks: fmt, workspace clippy, wasm32 clippy for site and edge with and without `test-faults` and for ops,
  `cargo test --workspace` (78 suites), `go test ./...`, and the three `scripts/check_*.py`: pass.

## Diagnostic gate on `15ac8bd` plus rule 12 (not the gate)

Site day 11:10 to 11:19Z, `SHARD_INSTANCE=standard-4`, image `sha256:15f6ed1b…`; runs 11:21 to 11:52Z, each on a new
group, `arrival=mem`, rates alternating run by run. 0 failed commands in all 16 runs, no leader change, no STEP_DOWN,
no alarm in any run. The leader was on site 0 in every run.

| Run | Throughput (commands/s) | Commit p99 (ms) | Leader confirm p99 (ms) | Commands per entry | CPU p999 (ms) | 80 ms+ `/commit` turns |
| --- | --- | --- | --- | --- | --- | --- |
| r300-1 | 280 | 294 | 30 | 40.2 | 12.9 | 0 |
| r300-2 | 280 | 311 | 34 | 41.1 | 22.2 | 0 |
| r300-3 | 280 | 386 | 30 | 48.5 | 15.5 | 0 |
| r300-4 | 280 | 392 | 42 | 49.6 | 16.9 | 0 |
| r300-5 | 280 | 301 | 29 | 40.3 | 17.7 | 0 |
| r300-6 | 280 | 322 | 50 | 45.5 | n/a | n/a |
| r300-7 | 280 | 249 | 57 | 33.3 | 17.4 | 0 |
| r300-8 | 280 | 365 | 52 | 48.0 | 18.7 | 0 |
| r330-1 | 308 | 280 | 79 | 42.5 | 19.5 | 0 |
| r330-2 | 308 | 354 | 63 | 47.6 | 25.2 | 0 |
| r330-3 | 308 | 356 | 50 | 46.7 | 20.6 | 0 |
| r330-4 | 306 | 593 | 117 | 55.0 | 23.1 | 0 |
| r330-5 | 308 | 273 | 36 | 39.8 | 15.5 | 0 |
| r330-6 | 308 | 245 | 36 | 37.0 | 17.7 | 0 |
| r330-7 | 308 | 395 | 68 | 50.1 | n/a | n/a |
| r330-8 | 308 | 258 | 20 | 37.9 | 17.5 | 0 |

The 300 offered runs measure 280 commands/s and the 330 runs 306 to 308, as in `carried-6` and `carried-7` (the bench
paces in 1 s windows). CPU and turns come from `durableObjectsInvocationsAdaptiveGroups` and Workers observability; two
runs had no rows there yet when read. r330-4 came within 7 ms of the line, with the highest confirm p99 of the 16.

Against the earlier gates on the same install and rate: `carried-6` passed 14 of 16, and `carried-7`'s four A/B arms
passed 23 of 32. Here 16 of 16 passed. That is consistent with the pipeline closing the near-gap class, but 16 runs
cannot separate a fix from luck at the earlier failure rate [inference].

Raw: `raw-carried-8/` (bench rows and gzipped `/stats` for `m0c8g-*` and the smoke `m0c8-smoke`).

## Near-gap traces

`m0c7b-ab11`, `ab02`, `ab14` and `ab16`, 9 near calls of 400 ms or more:

- For 9 of 9 late entries, the far call for an earlier entry was still outstanding when the late entry was appended,
  with 50 to 1,235 ms left. The entry reached the far follower only on the next call, 50 to 984 ms after its append.
- 0 of 9 were held behind an earlier unconfirmed leader write. Over the four runs, 21 to 36 of 140 to 167 far calls
  went out during an unconfirmed transaction, and none of the 9 did.
- The far follower answered promptly. Its own time per call was p50 10 to 26 ms and p90 40 to 73 ms, after RTT and
  body. Its confirm was p50 4 to 6 ms and p99 28 to 53 ms. The body arrival grows with size: 50 to 100 KB p50 46 ms,
  200 to 250 KB p50 388 ms, 300 to 350 KB p50 546 ms. The outstanding call was often a large catch-up.
- Commit was set by the far answer in 6 of 9. Pipelined, the far round would have been 233 to 567 ms; one entry
  stayed at 732 ms and one fell from 1,630 to 1,199 ms (outstanding bodies of 607 and 951 ms).

So the two followers did not pause together. The far path was serialized behind one call per peer, which is what
(b') removes. The smoke `m0c8-smoke` (20 s at 330, commit p99 346 ms) pipelined 384 of 388 far calls.

## LOOP_LAG

While a Durable Object's loop is behind, workerd clamps `Date.now()` to the overdue 100 ms tick, so a confirm that
starts and ends late reads a whole number of ticks. Over 82 one-minute B0-02 runs (`carried-6`, `carried-7`), "two
confirms within 2 s, each at least 200 ms and an exact multiple of 100 ms" matched 11 of 25 failing runs and 1 of 57
passing (`m0c6-ab04`, at 600 ms). Three in 5 s gave the same false positive and 10 matches; four in 5 s gave none and 8.
A peer-stamped lag was rejected because its worst 5 s median reached 1,290 ms in a passing run. No gate run above
tripped it.

## Simulator, on `15ac8bd` plus rule 12 (diagnostic tree)

- Long sweep, `RAFT_SEEDS=2000 RAFT_STEPS=10000`, live half 4,000 seeds from 10000: passed, with longest stretches
  16.4 s (harsh, seed 10100), 25.7 s (gray, 11247) and 27.3 s (live, 10256). Porcupine `-seg 2000`: Ok on all three
  histories (6,000, 6,000 and 12,000 segments).
- Widened live window, 10000 to 13999 at an absolute 30 s: 0 over 30 s; 3 over 25 s (10256 at 27.3 s, 13376 at
  26.7 s, 13489 at 25.1 s).
- `long_liveness` and the raft suites in release: pass.
- Without rule 12, one live seed was over: 12217 at 30.9 s (a half-hung site 1 that keeps CommitErrors).
- These numbers belong to the diagnostic tree. They are rerun on the rebased tree.

## Cost

`graft-m3d` from 11:05 to 12:00Z cost $3.06 and from 13:25 to 14:40Z $2.94
(`INSTALL=graft-m3d python3 tests/fanout/m2bcost.py`): two site days and 32 runs. From 15:40 to 16:50Z, the final
site day and gate, $3.00. The total for this thread's B0-02 work is about $28 of the $55 cap.
