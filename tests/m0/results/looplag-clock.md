# looplag-clock: `LOOP_LAG` counts syncs on the isolate's clock (2026-11-20, code only)

The orchestrator granted this (T-01a17bda) after B0-29 m3fp2. That row's one leader change was a `LoopLag { clamped: 2 }`
trip on two confirms ended by one sync. They read 500 / 500 on the member clock and 532 / 533 on the isolate's clock,
and site 0 logged no lag row (`tests/m0/results/confirm-storage-m3f.md`, section 2). That meets the recorded flip of
"A leader whose loop is behind steps down (`LOOP_LAG`)" in `decisions/DECISIONS.md`. B0-35 m57's term 5 change was also
`LoopLag { clamped: 2 }`, on a product build that cannot be replayed.

## Change

- **Design.**
  - `design/DESIGN.md`, Scale, "Commit on `Tri`": the rule is two syncs within 2 s whose confirms read whole ticks on
    the isolate's clock, with m3fp2 cited.
  - DESIGN, Shape: the `Health` doc points to it.
  - `decisions/DECISIONS.md`: an amendment, 2026-11-20 (`looplag-clock`), under the `LOOP_LAG` entry, with its own
    flip.
- **Member** (`crates/raft/src/member/health.rs`, `mod.rs`).
  - `Member::confirm_end_timed` takes `ConfirmTimes { storage_ms, isolate_ms, sync }`.
  - `LOOP_LAG` counts distinct samples among the window's confirms of 200 ms or more on whole hundreds of
    `isolate_ms`. A sample is the confirm's sync, or the confirm itself when the host named no sync.
  - With no host times, as in `confirm_end`, the isolate time is the member-clock time and each confirm is its own
    sample, as before.
  - The confirm rule (storage), the own-stall exclusion and `kept` are unchanged.
  - `StepDownWindow` gains `isolate` (end, isolate ms, sync) for the log line.
- **Host** (`workers/site/src/host.rs`).
  - Each unconfirmed write records `now_ms()` at its step.
  - Each `sync()` the flush awaits gets a per-boot number.
  - Every confirm that sync ends is reported with `isolate_ms` = sync end − its step on `now_ms()`, and with that number.
  - The `stepDownWindow` line gains `"isolate": [[end, ms, sync], ...]`, with -1 for no sync.
- **Sim** (`crates/raft/tests/common/sim.rs`).
  - Each confirm is reported with its end on the site's output gate as its sync. The confirms that end at one instant
    are one sync, as on the host.
  - No separate isolate time is reported, because the sim's member clock is its isolate's. Its `LoopLag` fault's
    clamped clock still drives the rule.

## Checks

- **New tests through `step`** (`crates/raft/tests/leadership.rs`). Each fails on main's rule: health.rs's counting
  was put back to member-clock, per-confirm, and both failed.
  - `whole_ticks_on_the_member_clock_or_in_one_sync_do_not_trip_loop_lag`:
    - m3fp2's case: one sync, 500 / 500 member and 532 / 533 isolate;
    - one sync whose two confirms both read 500 on the isolate's clock;
    - two syncs on whole member-clock ticks only.
    - None of these trips.
  - `two_clamped_isolate_syncs_trip_loop_lag_and_transfer`: two syncs 1 s apart reading 300 and 400 on the isolate's
    clock, with the member clock 750 ms off the tick, trip `LoopLag { clamped: 2 }` and transfer.
  - `two_clamped_confirms_trip_loop_lag` (through `confirm_end`) passes unchanged.
- **Plants** (`tests/m0carried/plant54.py`): 4 of 4 caught.
  - the member clock's step-to-end time again;
  - each confirm one sample;
  - whole ticks not required;
  - the host logging member-clock times as the isolate's.
- **Long sweep.** `RAFT_HISTORY=/tmp/sw/hist.jsonl RAFT_SEEDS=2000 RAFT_STEPS=10000 cargo test -q -p graftyard-raft
  --release --test sim long_sweep -- --ignored --nocapture` passed in 1,106 s. Main `bebbe7f0` was run the same way
  just before it, with 1,268 s.

  | phase | branch step-downs / holds | main step-downs / holds | longest stuck stretch, branch | longest stuck stretch, main |
  |---|---|---|---|---|
  | harsh | 0 / 8,206 | 0 / 8,206 | 14,707 ms (seed 10439) | 14,707 ms (seed 10439) |
  | gray | 2,133 / 10,374 | 2,138 / 10,407 | 28,501 ms (seed 11893) | 28,501 ms (seed 11893) |
  | live (4,000 seeds) | 1,782 / 10,443 | 1,791 / 10,450 | 25,191 ms (seed 13897) | 25,191 ms (seed 13897) |

  - Harsh is identical.
  - Gray and live trip 5 and 9 fewer step-downs (0.2% and 0.5%). That is the sim's confirms that end together behind
    one site's output gate now counting as one `LOOP_LAG` sample.
  - The longest stretches are the same seeds and lengths, all under the 30 s bound.
- **Porcupine** (`tests/m0/porcupine`, built, `-seg 2000`, one file per run): `hist.jsonl` Ok 146,836 operations;
  `.gray` Ok 2,336,111; `.live` Ok 5,289,017 (12,000 segments).
  - Passing two files in one run merges their per-seed keys and reads every segment as Illegal (5,982 of 6,000).
  - So each history is checked on its own, as `tests/am1/results/write-cuts.md` lists them.
- **Full checks.** All pass:
  - `cargo fmt --check`;
  - `cargo clippy --workspace --all-targets -D warnings`;
  - wasm32 clippy for site and edge, each with and without `test-faults`;
  - wasm32 clippy for ops, front and runners (ops has no `test-faults` feature);
  - `cargo test --workspace`: 1,268 passed, 0 failed;
  - `go test ./...` in `containers/git/server`: ok;
  - `scripts/check_design.py`, `check_names.py` and `check_links.py`: OK.
