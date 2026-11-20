# step-agree and alarm-steps (2026-11-20, code only)

The orchestrator (T-01a17bda) asked for Fable's recommendation 4 after B0-35 m60 (`tests/m4/results/m4-wp4.md`, "B0-35
on a test-faults build after site day 60"): stop self-inflicted leader changes. In the same request, it asked for f0's
CPU attribution and per-step alarm timing. No hosted spend.

## Change 1: `step-agree` (`crates/raft`)

- **Design.** `design/DESIGN.md`, "Gray region"; `decisions/DECISIONS.md`, "`step-agree`"; `decisions/OPEN-QUESTIONS.md`,
  F105 (the host does not set the verdict yet). B0-06's gray-site row must rerun.
- **Member** (`crates/raft/src/member/health.rs`).
  - `check_health`: after `tripped()`, a leader whose group is `hot` holds (`Alarm::StepDown { to: None }`, one
    STEP_DOWN window) unless `site_sick` is set.
  - `hot` counts committed pushes over HOT_WINDOW, so a group that stops committing stops being hot, and its next trip
    moves it. No separate progress clock.
  - `Member::set_site_sick(now, sick)`: the host's entry for both other sites' `SiteHealth` verdict.
- **Tests** (`crates/raft/tests/leadership.rs`, "step-agree"), through `step` on a three-member cluster pushing
  30 a second:
  - `a_loaded_leader_that_still_commits_holds_through_its_own_confirm_trip`: six 750 ms confirms trip Confirm, and A
    holds once, stays leader and commits 50 or more entries more. Fails on main (plant 0).
  - `a_hot_leader_on_a_site_the_others_call_sick_still_moves`: the same trip with `set_site_sick(true)` transfers.
  - `a_hot_leader_that_stopped_committing_is_not_kept`: every AppendEntries reply lost, so nothing commits. A holds
    first, then is no longer leader.
  - `a_quiet_groups_own_trip_still_transfers`: no pushes, main's transfer.

## Change 2: `alarm-steps` (`workers/site`)

`workers/site/src/alarm_steps.rs`: the Repo family's alarm times each of split, load, prsync, merge_queue, hookseq,
activity, actions_trigger and snap. When any one took 100 ms or more, it logs one line, `{do} alarm steps over 100ms:
prsync 2288ms, …; alarm Nms`. A quick alarm logs nothing. Each step ends with a zero-length timer, because the Workers
clock moves only on I/O [doc, Workers, "Performance and timers"]. Test `an_alarm_names_only_its_slow_steps`.

## f0's CPU on m60

In `decisions/OPEN-QUESTIONS.md`, F104. The 2,652 ms learner `/raft` was a read-index batch while entry 4006 (110,751 B)
went to 8 learners and 2 followers. No snapshot or dump ran, and no send path was found unbounded. The cause is not
attributed; the next row's alarm-steps lines name the alarm's step.

## Plants

`tests/m0carried/plant60.py`, 6 of 6 caught:

| # | Mutant | Verdict |
| --- | --- | --- |
| 0 | main's rule: every trip moves the leader, hot or not | CAUGHT |
| 1 | the other sites' verdict is ignored | CAUGHT |
| 2 | every group holds, hot or not | CAUGHT |
| 3 | the hold is for a quiet group, not a hot one | CAUGHT |
| 4 | the host's verdict is never stored | CAUGHT |
| 5 | a held trip is a transfer to nobody, with no hold window | CAUGHT |

Mutant 5 was missed at first, because the hold test allowed any number of holds. It now asserts exactly one hold in
the window.

`tests/m4/wp4/plant.py` 162 to 164, 3 of 3 caught: a step of exactly 100 ms left out; each step timed from the
alarm's start; a quick alarm logging an empty line.

## Long sweep

`RAFT_HISTORY=/tmp/sw/hist.jsonl RAFT_SEEDS=2000 RAFT_STEPS=10000 cargo test -q -p graftyard-raft --release --test sim
long_sweep -- --ignored --nocapture`, on this branch and on main `49667ee3` (a worktree, same command):

| phase | branch step-downs / holds | main step-downs / holds | longest stuck stretch, branch | longest stuck stretch, main |
|---|---|---|---|---|
| harsh | 0 / 8,206 | 0 / 8,206 | 14,707 ms (seed 10439) | 14,707 ms (seed 10439) |
| gray | 2,133 / 10,374 | 2,133 / 10,374 | 28,501 ms (seed 11893) | 28,501 ms (seed 11893) |
| live (4,000 seeds) | 1,782 / 10,443 | 1,782 / 10,443 | 25,191 ms (seed 13897) | 25,191 ms (seed 13897) |

The branch passed in 1,074 s, and every phase matches main. The sweep's gray faults are site-wide, and no sim group
stays above HOT_PUSHES through its own trip often enough to move these counts [inference]. The rule is pinned by the
targeted tests above, not by the sweep.

## Porcupine

`tests/m0/porcupine`, built, `-seg 2000`, on the branch's histories, each exit 0: `hist.jsonl` Ok, 146,836
operations; `.gray` Ok, 2,336,111; `.live` Ok, 5,289,017 (12,000 segments).

## Checks

All exit 0, each checked by its exit code:
- `cargo fmt --check`;
- `cargo clippy --workspace --all-targets -D warnings`;
- wasm32 clippy for site and edge, each with and without `test-faults`, and for front and runners;
- `cargo test --workspace`: 1,303 passed, 0 failed (on the branch rebased onto main `b655c323`);
- `node --test workers/runners/tests/*.test.mjs`: ok;
- `go test ./...` in `containers/git/server`, `containers/ssh/server` and `tests/m0/porcupine`: ok;
- `scripts/check_design.py`, `check_names.py` and `check_links.py`: OK.

The first full-check run failed `cargo test --workspace` on "No space left on device" (the orb's disk), not on a
test; it passed after old target directories were removed. The long sweep and Porcupine ran before the rebase onto `b655c323`, which changes no `crates/raft` file.
