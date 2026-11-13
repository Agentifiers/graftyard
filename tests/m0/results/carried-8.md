# B0-02: a pipeline to each voter, and a loop-lag step-down (2026-11-13)

The operator's decisions on `carried-7.md` (C20): check the near-follower gaps of `m0c7b-ab11`, `ab02`, `ab14` and
`ab16` for the far path's own limits; build (b'), AppendEntries pipelined to a voter, if the far path waited on one call
per peer or on the output gate; build (2b), a leader that steps down when its own loop is behind; then the gate,
8 of 8 one-minute runs at 300 offered and 8 of 8 at 330, commit p99 at most 600 ms, `arrival=mem`, standard-4 shard
containers, a new group per run, no window selection. Branch `m0-carried-8`; install `graft-m3d`.

## Verdict

**B0-02: not yet recorded as passed.** The 16-run gate below passed 16 of 16 on sites built from `m0-carried-8`
(`15ac8bd`) plus F67 rule 12 alone. The operator ruled it diagnosis, not the gate: rule 12 lands on main only together
with rule 13 (narrow-reads-3), so the tree that ships differs from the tree that ran. The gate reruns, the same way, on
`m0-carried-8` rebased onto main with rules 12 and 13.

## Diagnostic gate on `15ac8bd` plus rule 12

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

## Simulator, on `15ac8bd` plus rule 12

- Long sweep, `RAFT_SEEDS=2000 RAFT_STEPS=10000`, live half 4,000 seeds from 10000: passed, with longest stretches
  16.4 s (harsh, seed 10100), 25.7 s (gray, 11247) and 27.3 s (live, 10256). Porcupine `-seg 2000`: Ok on all three
  histories (6,000, 6,000 and 12,000 segments).
- Widened live window, 10000 to 13999 at an absolute 30 s: 0 over 30 s; 3 over 25 s (10256 at 27.3 s, 13376 at
  26.7 s, 13489 at 25.1 s).
- `long_liveness` and the raft suites in release: pass.
- Without rule 12, one live seed was over: 12217 at 30.9 s (a half-hung site 1 that keeps CommitErrors).
- These numbers belong to the diagnostic tree. They are rerun on the rebased tree.

## Cost

`graft-m3d` from 11:05 to 12:00Z cost $3.06 (`INSTALL=graft-m3d python3 tests/fanout/m2bcost.py`): one site day and
the 16 runs. The total for this thread's B0-02 work is about $22 of the $55 cap.
