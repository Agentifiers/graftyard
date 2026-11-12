# B0-02's tail on the current code (2026-11-12)

The operator asked to close B0-02's tail under the no-exceptions rule: the 656 ms run (a 73-command entry), the
1,000 ms run (a leader storage stall that tripped `STEP_DOWN` and held with `to: None`), and 300 offered, where one
run of three passed 600 ms. Branch `m0-carried-2`, from `main` at `0a1d60c`. Same load, edge, and install as
`carried.md` ("B0-01 and B0-02"): `tests/m1/wp2/run.ts bench`, 1 KB commands through `graft-m3c-wp2-edge`, one new
`Tri` register group per run led from site 0 (enam). Raw rows are in `raw-carried-2/`.

## What sets the tail

Ten one-minute runs on graft-m3c's sites at the time (lf4-rest-2's site day, unchanged raft; 04:32 to 04:51Z), with every
member's `/stats` saved (`BENCH_RAW=1`) and each leader's `Rtt` read after the run (`/rtt`, the median of 20 binding
round trips the member measured at join):

| Run | Offered | Carried /s | Near RTT (ms) | Far RTT (ms) | Commands / entry | Round p50 / p99 | Leader confirm p99 | Commit p99 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| diag-5 | 330 | 308 | 75 | 235 | 36.9 | 111 / 160 | 47 | **304** |
| diag-8 | 330 | 308 | 82 | 229 | 38.8 | 117 / 180 | 85 | **300** |
| diag-2 | 300 | 280 | 83 | 230 | 37.5 | 126 / 184 | 46 | **330** |
| diag-9 | 330 | 308 | 86 | 238 | 44.3 | 134 / 232 | 54 | **408** |
| diag-10 | 330 | 307 | 89 | 156 | 43.8 | 132 / 203 | 33 | **348** |
| diag-4 | 330 | 308 | 94 | 173 | 45.9 | 138 / 238 | 51 | **401** |
| diag-6 | 330 | 307 | 109 | 152 | 57.6 | 164 / 286 | 50 | **515** |
| diag-1 | 300 | 280 | 123 | 208 | 65 | 204 / 346 | 65 | **638** |
| diag-3 | 330 | 308 | 127 | 213 | 76.4 | 214 / 425 | 51 | **703** |
| diag-7 | 330 | 282 | 94 | 225 | 82.2 | 209 / 802 | **551** | **1,000** |

- **The commit tail follows the near follower's round trip.** The quorum is the leader and its nearer follower, so a
  round is one near round trip plus the follower's step and confirm. With one entry in flight, the commands that queue
  during a round go into the next entry, so a longer round makes larger entries, and a command waits up to one round
  queued and one round in flight: commit p99 is 2.6 to 3.3 round p50s in these runs. Over the 19 runs on both days without a leader
  stall, commit p99 correlates 0.88 with the near RTT (`Rtt`, read after the runs). Every run with a near RTT of 100
  ms or less passed (10 of 10, 286 to 408 ms); of the 9 over 100 ms (105 to 127 ms), 5 passed and 4 did not (360 to
  703 ms).
  `carried.md`'s r330-1 (656 ms) had a near RTT of 109 ms, and the 300-offered b02-2 (641 ms) 114 ms [measured].
- **Entry size amplifies the round trip; a cap would not remove it.** Across runs, a round's p50 over the near RTT
  grows with the entry: 35 to 48 ms at 37 to 46 commands (diag-5, -8, -2, -9, -10, -4), 55 ms at 58 (diag-6), 81 and
  87 ms at 65 and 76 (diag-1, -3), about 1 ms per KB of entry, which matches each run's median per-KB excess of a
  single-entry append over the near RTT (0.93 to 1.04 ms/KB in diag-4, -5, -6, and -8). Inside one run the regression
  is too noisy to show it (slopes -0.74 to +0.81 ms/KB over 1 to 237 KB). With one entry in flight an entry carries what
  queued during the last round, so round ≈ (RTT + c) / (1 − k·rate): at 330 commands/s of 1 KB and k ≈ 1 ms/KB the
  RTT is amplified about 1.5 times, which is how 75 ms becomes a 111 ms round and 127 ms a 214 ms one [inference from
  the measured rounds]. A cap on an entry's commands or bytes does not change that fixed point: a cap above what queues
  never binds, and a cap below it leaves commands for later rounds, so each waits more rounds, and below the arrival
  rate times the round the queue grows without bound. So no cap is added; the design caps only `ENTRY_MAX` (1.5 MB)
  and `AGENT_INLINE_ENTRY`. The levers are k, the per-KB cost of an append, whose cause is not measured (the
  follower's own confirm is 10 to 13 ms at p50), and the near RTT (F68).
- **The other miss is a leader storage stall.** diag-7 (1,000 ms) had leader confirms over 300 ms in 14 of 269
  windows (up to 780 ms in one 5 s bucket) and 14 AppendEntries to the far follower missing `CALL_DEADLINE`, with
  every follower confirm under 100 ms. This is `carried.md`'s r330-9 again, and F35's Durable Object storage stall,
  which a bare object with no Raft code shows (`tests/m0/results/m0-wp4.md`, "F35") [measured]. It did not trip
  `STEP_DOWN` (6 slow confirms in 10 s are needed).

## The `STEP_DOWN` hold with `to: None` (r330-9)

r330-9's leader tripped `Confirm { p99_ms: 1042, slow: 6, of: 21 }` at 23:45:46Z and held: no transfer, one window,
and the stall stayed on the leader to the end of the run. Its members show no drain, `boots` [0, 0], and no earlier
step-down, so `may_lead_target` and `RESET_QUARANTINE` passed both followers. The only rule left that empties the
candidates on a confirm trip is `rate(s) <= commit_errors` on the leader's own calls: the run had 12 failed calls, and
a window of 21 rounds gives each follower about 21 calls, so one or two misses pass 5% [inference; the member logs of
that window were not kept].

- **Cause in code.** `check_health` (`crates/raft/src/member/health.rs`) disqualified a confirm-trip target on its
  failure rate alone, while `tripped` counts a failing link only with `commit_fail_min` (3) failures as well, which F37
  added because a far follower misses `CALL_DEADLINE` about once in 10 to 15 calls with no fault.
- **Reproduced.** A new case, `a_confirm_trip_ignores_fewer_than_commit_fail_min_failures_to_a_target`
  (`crates/raft/tests/leadership.rs`): two failed calls to each follower, then 6 slow confirms. On the old code the
  leader holds with `to: None`; on the fix it transfers to B, and with three failures to B it transfers to C.
- **Fix (design first).** `design/DESIGN.md`, Shape, `Health`: on a confirm trip the target's link from the leader
  must not fail over `commit_errors` *with* `commit_fail_min` failed, as the commit-error rule counts a failing link.
  `check_health` now uses that test. Nothing else changed: the trip thresholds, the commit-error path, the probe, and
  the quarantine are as before.
- **Should a storage stall step down at this load?** Yes, by the design's own evidence: a stall of this kind on the
  leader holds the round for its length (it is the leader's own copy that counts toward the quorum), and it held in
  diag-7 and r330-9 for most of the minute. B0-06 measured the confirm rule moving such leaders off slow storage in
  2.3 to 5.6 s (`decisions/DECISIONS.md`, `STEP_DOWN`'s confirm rule). The simulator agrees at this load: with 30% of
  confirms at 600, 800, or 1,000 ms on the leader, the confirm rule moved 40, 40, and 39 of 40 seeds, first new
  leader at 4.6 to 13.3 s [measured, simulator, scratch run not kept]. A transfer costs one election-free hand-off. So
  the rule stays; the hold was the defect.

## The long sweep and Porcupine

On `m0-carried-2` with the fix (`RAFT_SEEDS=2000 RAFT_STEPS=10000`, release, `RAFT_HISTORY`): `long_sweep` passed in
426 s.

- HARSH: 20,109,327 steps, 160,083 operations (49,655 acked), 226,062 crashes, 31,225 partitions, 0 step-downs and
  10,261 holds (HARSH's crash rate quarantines every target, F42).
- GRAY: 20,081,161 steps, 2,226,349 operations (951,268 acked), 17,140 gray faults, 24,332 write failures, 1,574
  `STEP_DOWN` transfers and 11,617 holds.
- Porcupine (`tests/m0/porcupine`, `-seg 2000`): HARSH 156,742 operations `Ok`, GRAY 2,226,338 operations `Ok`, 6,000
  segments each.
- `long_liveness` (`RAFT_LIVE_SEEDS=60`): calls failing 30%: new leader off the site p50 5.4 s, max 11.5 s; slow
  confirms 30% at 1 s: p50 9.5 s, max 13.2 s; both 60 of 60 by `STEP_DOWN`, one leader change each. Failing writes
  moved by election, p50 11.4 s, max 16.2 s, as before (F41).

## B0-02 rerun

16 one-minute runs, 8 at 300 offered and 8 at 330, alternating, 05:44 to 06:06Z, on a site day from `m0-carried-2`
`5fb175f` merged with `origin/lf4-rest-2` `9612268` (`test-faults`, same shard image; sites a/b/c `c26e6cae` /
`429347f8` / `f646fb11`, deployed 05:36 to 05:44Z). No other thread ran on graft-m3c from 05:36 to 06:13Z. Each run
is a new group; `rtt` is the leader's `Rtt` to each follower.

| Run | Carried /s | Near / far RTT (ms) | Commands / entry | Round p50 | Leader confirm p99 | Commit p99 (ms) | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |
| r300-1 | 279 | 106 / 153 | 47.4 | 154 | 146 | **542** | Pass |
| r300-2 | 278 | 87 / 203 | 37.4 | 125 | 50 | **276** | Pass |
| r300-3 | 280 | 82 / 233 | 37.4 | 125 | 45 | **299** | Pass |
| r300-4 | 279 | 144 / 210 | 73.5 | 231 | 37 | **790** | Between |
| r300-5 | 279 | 90 / 200 | 38.7 | 129 | 109 | **329** | Pass |
| r300-6 | 280 | 102 / 159 | 54.5 | 178 | 47 | **515** | Pass |
| r300-7 | 279 | 90 / 172 | 38.0 | 126 | 73 | **358** | Pass |
| r300-8 | 280 | 107 / 241 | 58.3 | 188 | 63 | **563** | Pass |
| r330-1 | 308 | 121 / 157 | 75.9 | 218 | 144 | **678** | Between |
| r330-2 | 308 | 101 / 146 | 58.8 | 168 | 297 | **549** | Pass |
| r330-3 | 308 | 133 / 176 | 80.2 | 225 | 51 | **722** | Between |
| r330-4 | 308 | 115 / 242 | 61.9 | 177 | 48 | **579** | Pass |
| r330-5 | 308 | 86 / 184 | 41.3 | 126 | 35 | **309** | Pass |
| r330-6 | 308 | 99 / 227 | 50.0 | 147 | 43 | **445** | Pass |
| r330-7 | 308 | 102 / 232 | 56.4 | 165 | 160 | **498** | Pass |
| r330-8 | 308 | 132 / 178 | 78.3 | 224 | 90 | **703** | Between |

- **Verdict: not every run.** 12 of 16 pass 600 ms: 7 of 8 at 300 offered, 5 of 8 at 330. The 4 others are between
  the lines (678 to 790 ms), none over 1 s, none under 100 commands/s, 0 failed commands in all 16.
- **Every miss is placement.** The misses' near RTTs are 121 to 144 ms; every run at 115 ms or less passed. Over all
  35 runs on both days without a leader stall, commit p99 correlates 0.93 with the near RTT, and all 16 at 100 ms or
  less passed (276 to 445 ms). From 101 to 110 ms, 8 of 9 passed; from 111 ms up, 3 of 10.
- **The `STEP_DOWN` fix was not exercised hosted.** No run tripped `STEP_DOWN`; the highest leader confirm p99 was 297
  ms. Its evidence is the simulator case and the sweep above.
- **What placement would give.** One extra run on a group re-created until its near RTT was at most 100 ms
  (`m0c2-sel330-1`, 98 ms, the third of 3 tries) passed at 511 ms. Of 8 candidate groups made with the same hints, 2
  had a near RTT of 100 ms or less, and 16 of the 35 measured groups did. The run was stopped there to free graft-m3c.

### What closing it would take

The two causes the operator named are not what sets the tail. A command or byte cap does not shorten a round (above),
and the storage-stall hold is fixed but rare. The tail is set by where Durable Objects place a new group's members
inside the hinted regions, which `Placement::new` checks only for a 30 ms floor (`MIN_PAIR_RTT`). Three options, each a
design change, none made here:

1. **Bound the near RTT at placement.** `Placement::new` (`crates/types/src/placement.rs`) also refuses a `Tri` voter
   set whose leader's home has no follower within a bound (about 100 ms enam to weur), and the create path retries
   new generations, as the move path already does up to 3 times. Cost: about 2 to 4 tries per new group at today's
   placement (16 of 35 groups would pass), each try a join and 20 round trips, and some groups never pass; a home on
   apac would need its own bound. A new `PlacementError` variant goes at the end of the enum.
2. **Lower the per-KB cost of an append** (about 1 ms per KB, `carried-2.md` above), which amplifies the RTT by about
   1.5 at 330 commands/s. Its cause is not measured; the follower's own confirm is 10 to 13 ms at p50.
3. **Read B0-02 per placement**, as B0-12 already reads placement: pass on groups whose near RTT is within the bound,
   which needs an operator decision on the gate as written.

## Cost and checks

graft-m3c usage in this branch's windows (`INSTALL=graft-m3c python3 tests/fanout/m2bcost.py`): $2.53 from 04:30 to
05:00Z (the 10 diagnostic runs, on lf4-rest-2's sites) and $4.24 from 05:36 to 06:14Z (the site day and the 17
runs), $6.77 in all. Full checks on `5fb175f`: `cargo fmt --check`, workspace clippy, wasm32 clippy for site, edge,
and ops (with `test-faults` for site and edge; ops has no such feature), `cargo test --workspace` (698 passed, 0
failed), `go test ./...` in `containers/git/server`, and the three `scripts/check_*.py`: all pass.
