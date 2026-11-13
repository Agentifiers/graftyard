# B0-02: an entry byte cap (2026-11-13)

The operator's decisions on `carried-6.md` (C20): drop the send reorder (b) and the far-follower pipeline (b'), and
build an entry byte cap of about 80 KB, justified from `carried-6`'s confirm-by-size table, with the long sweep,
Porcupine, a kept test and plants; then A/B it hosted on `graft-m3d` with standard-4 shards, a new group per run,
`arrival=mem`, no window selection, reporting per-request CPU p999 and the count of 80 ms+ `/commit` turns beside
commit p99; then the gate, 8 of 8 one-minute runs at 300 offered and 8 of 8 at 330, commit p99 at most 600 ms. Branch
`m0-carried-7`; install `graft-m3d`, this thread's alone; `SHARD_INSTANCE=standard-4` at both site days.

## Verdict

**B0-02: not passed, and the gate was not run.** The cap does not close it. Two same-window A/Bs of 16 runs each, every
run on a new group:

| A/B | Arm | Passed (commit p99 at most 600 ms) | Commit p99 of each run (ms) |
| --- | --- | --- | --- |
| 1 (sites `d568c12`) | 80 KB cap | 7 of 8 | 388, 274, 343, 290, 295, **1,103**, 292, 336 |
| 1 | no cap (`ENTRY_MAX`) | 7 of 8 | **701**, 345, 283, 451, 510, 489, 266, 299 |
| 2 (sites `2a99a65`) | 60 KB cap | 4 of 8 | 244, 455, 296, **2,000**, 531, **1,643**, **1,116**, 278 |
| 2 | 80 KB cap | 5 of 8 | **982**, 297, 251, 307, **957**, 453, **728**, **1,068** |

Over the four arms, 23 of 32 runs passed, and no arm separates from another. A/B 1 ran from 03:50 to 04:16Z and A/B 2
from 04:40 to 05:05Z. Both A/Bs run 330 offered, `arrival=mem`, with the arms alternating run by run. 0 failed commands
in all 32 runs.

The heavy turns and CPU that separated `carried-6`'s bad runs do not separate these. Per run, from the leader's
object: the share of its traced events on one millisecond phase (the clamp), per-request CPU p999
(`durableObjectsInvocationsAdaptiveGroups`), and `/commit` invocations of 80 ms CPU or more (Workers observability):

| Failing run | Arm | Commit p99 | Leader confirm p99 | Clamp (worst 4 s window) | CPU p999 | 80 ms+ `/commit` | Near calls of 400 ms or more |
| --- | --- | --- | --- | --- | --- | --- | --- |
| A/B 1 ab02 | none | 701 | 300 | 0.23 | 55 ms | 7 | 4 |
| A/B 1 ab11 | 80 KB | 1,103 | 146 | 0.11 | 43 ms | 6 | 2 |
| A/B 2 ab07 | 60 KB | 2,000 | 600 | **1.00** | 53 ms | 6 | 8 |
| A/B 2 ab11 | 60 KB | 1,643 | 39 | 0.08 | 21 ms | **0** | 2 (1,630 ms) |
| A/B 2 ab13 | 60 KB | 1,116 | 281 | 0.11 | 45 ms | 8 | 1 |
| A/B 2 ab02 | 80 KB | 982 | 35 | 0.11 | 19 ms | **0** | 2 (954 ms) |
| A/B 2 ab10 | 80 KB | 957 | 81 | 0.14 | 29 ms | 1 | 1 |
| A/B 2 ab14 | 80 KB | 728 | 41 | 0.10 | 29 ms | **0** | 2 (1,178 ms) |
| A/B 2 ab16 | 80 KB | 1,068 | 45 | 0.10 | 23 ms | **0** | 3 |

Passing runs had CPU p999 of 14 to 33 ms and 0 to 3 heavy `/commit` turns. A/B 2's ab09 passed at 531 ms with 3. The 20 s
smoke before A/B 1 ran ENTRY_MAX at 85 commands per entry: 98 ms p999, 11 heavy turns and 1,000 ms commit p99. The 80
KB smoke ran 42 commands per entry: 14 ms, 0 heavy turns and 276 ms. So big entries do make heavy turns, and the cap
removes those. But the failures are two other shapes:

1. **A lagging loop (ab07).** For 12 s the leader's trace sat on one millisecond (clamp share 1.00 at 44 s), its
   confirms read 300 to 1,000 ms in multiples of 100, and commit p99 reached 2,470 ms in that stretch. Its entries were
   49 to 61 KB, under the cap. This is `carried-6`'s r300-7 signature, on an ORD leader again.
2. **One near-follower call held about 1 s, with nothing slow on the leader (A/B 2's ab11, ab02, ab14 and ab16).** In
   ab11 the near follower's own trace has no `/raft` arrival from 12.75 s to 14.13 s. Its last confirm took 12 ms,
   and it traced nothing at all in the gap, its 100 ms ticker included. The leader issued the call at 12.62 s, with its last confirm at
   27 ms and no clamp. The call carried 56 KB and ended at 1,630 ms. The far call was also slow, so the quorum waited.
   Every one of the four runs had CPU p999 of 19 to 29 ms and 0 heavy turns. The stretch did not involve the
   leader's CPU, the entry's size or the leader's storage.

## Why the cap moved from 80 KB to 60 KB, and why that did not help

A/B 1's capped failure, ab11, had its entries pinned at 81 KB, and 80 KB was where the near follower's body arrival
stepped. Over A/B 1 and all of `carried-6`'s runs (`raftIn`, near and far follower), bodies of up to 66 KiB took 60 ms
or more in 0 to 2% of calls, against 9 to 39% at 68 to 83 KiB:

| Body (KiB) | 56–60 | 61–66 | 67–70 | 72–74 | 76–79 | 80–83 |
| --- | --- | --- | --- | --- | --- | --- |
| Share of bodies at 60 ms or more | 0.00–0.01 | 0.01–0.02 | 0.04–0.14 | 0.17–0.22 | 0.22–0.39 | 0.25–0.30 |

The bare-object control (`graft-m4-gateprobe`, no write), on the same account and at the same time, showed no such
step. Array and stream bodies of 56 to 84 KB arrived at p90 1 to 13 ms, with at most 1% at 60 ms or more. So the step is the member's (its
bodies arrive alongside its own writes and sends), not the platform's alone [inference]. I lowered `ENTRY_SOFT` to
60,000 bytes, under the step, and A/B 2 compared that against 80 KB. 60 KB passed 4 of 8, so the step is not what fails
B0-02 either. `ENTRY_SOFT` stays at 60,000: it is still under the measured step, and capacity counts it.

## Rows and checks

- **The cap** (`crates/raft/src/lib.rs`, `ENTRY_SOFT`, the one constant). `cut` fills an entry past its first command
  only while it stays within the cap, and the first command always goes, up to `ENTRY_MAX`. Admission's capacity counts
  entries of the cap. `Member::set_entry_soft` is the host's override; only test-faults `/stats?entry=max|soft|<bytes>`
  uses it, for the same-window A/B (`tests/m0carried/ab7.sh`, `run.ts --entry`).
- **Kept test.** `a_backlog_splits_into_entries_within_entry_soft_in_order` (`crates/raft/tests/scenarios.rs`): 300
  commands of about 1 KB behind an entry in flight split into capped entries, each command once and in order, every
  ticket `Ok`, and the followers' logs match. It fails without the cap. `admission.rs` adds a capacity bound from
  `ENTRY_SOFT`. `a_bot_flood_cannot_take_the_agent_floor` now uses 4 KB agent commands, because at 60 KB a 10 KB
  command filled the floor's share of a round with bytes rather than with the floor rule. It still fails (0.0/s) with
  the floor removed.
- **Plants** (`tests/m0carried/plant.py`): 8 of 8 caught, on `d568c12` and again on `7841eca` (main `020b3e5`
  merged, whose `cut_fresh` now calls the capped `cut`). They plant: no cap; a
  half cap; the first command no longer always going; an off-by-one; Human bytes not counted; capacity from `ENTRY_MAX`;
  the constant raised to `ENTRY_MAX`; a member that starts uncapped.
- **Long sweep** (`RAFT_SEEDS=2000 RAFT_STEPS=10000`, release, `RAFT_HISTORY`) passed on `d568c12` (993 s), `2a99a65`
  (808 s) and `7841eca` (823 s). Porcupine (`tests/m0/porcupine`, `-seg 2000`) passed on all three, 6,000 segments
  each: on `2a99a65` HARSH 156,414 operations `Ok`, GRAY 2,244,942 and live 2,740,401; on `7841eca` HARSH 150,828,
  GRAY 2,042,240 and live 2,601,072. `long_liveness` (`RAFT_LIVE_SEEDS=60`): calls failing 30%,
  p50 5.4 s, max 11.5 s; slow confirms, p50 9.4 s, max 13.2 s, both 60 of 60 by `STEP_DOWN`; failing writes p50 11.4 s,
  max 16.2 s, as in `carried-2`.
- **Full checks** on `d568c12` and `7841eca` (fmt; workspace clippy; wasm32 clippy for site and edge with and without
  `test-faults`, and ops; `cargo test --workspace`; `go test ./...` in `containers/git/server`; the three
  `scripts/check_*.py`). `cargo test --workspace` passed 760 and failed 0 on `d568c12`, and 774 and 0 on `7841eca`.
- **Site days** on `graft-m3d`, both with `SHARD_INSTANCE=standard-4`: 03:37 to 03:47Z from `d568c12` (origin/main
  `a9d1645` merged) and 04:31 to 04:40Z from `2a99a65`. `graft-m3d-edge` and `graft-m3d-ops` were not touched. Only
  the bench edge `graft-m3d-wp2-edge` was redeployed. No shard containers ran. LF-13's Tokens precreate needs its rerun
  after these site days (`install-m3d.md`).
- **C20's send half** is closed in DECISIONS ("The far follower's held sends get no host change").

Raw: `raw-carried-7/` (bench rows and gzipped `/stats` for `m0c7s-*`, `m0c7-ab*` and `m0c7b-ab*`).

## Cost

`graft-m3d` from 02:35 to 05:11Z cost $6.62 (`INSTALL=graft-m3d python3 tests/fanout/m2bcost.py`), covering both site
days and both A/Bs. The total for this thread's B0-02 work is $18.53 of the $40 cap.

## LM-1's B0-29 leader, for comparison

LM-1 sent the heavy-turn data of its B0-29 leader (rows-m19 on `graft-m2b`, 500 agents, standard-2, main `a9d1645` plus
`0c7cbc2`, no cap, `repo:131289`, 02:39:40 to 02:48:30Z, 63,742 invocations from Workers Logs). CPU per 10 s bin
correlates 0.83 with `/commit` wall p90 (partial 0.82 controlling for invocation count, which alone gives 0.35). By route:
`/raft` 51.6% of 94.2 s (9 turns of 100 ms or more, max 363 ms), local `/entries` 16.1% (none), `/read` 15.0% (14,
max 256 ms), `/commit` 12.2% (2, max 212 ms), alarm 4.7% (3, max 205 ms). On a leader, `/raft` is mostly the
`AppendRes` turn that pumps the next entry, so its 190 to 363 ms turns fit the shape the cap bounds [inference: their
entry sizes were not checked]. The `/read` turns are reads served on the leader, which the cap does not touch.

## For the operator

The cap removes big-entry heavy turns, but B0-02 fails without them. What remains is a loop lag on the leader (ab07)
and about 1 s gaps in which the near follower receives nothing while the leader is idle (4 runs). Neither moves with
entry size. The next step under the earlier decision is the loop-lag step-down (2b), and it would cover ab07 only. The
near-follower gap needs its own explanation first: one follower object takes no input for about 1.4 s while its own
storage is fast. That looks like the platform pausing the follower's isolate or its inbound delivery [inference]. If
so, it is a gray fault on the follower, which the quorum rule tolerates only when the other follower is close enough.
Here the other follower is 160 to 250 ms away.
