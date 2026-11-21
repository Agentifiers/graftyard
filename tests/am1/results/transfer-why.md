# transfer-why: a line at every transfer's start, with its cause (2026-11-21, code only)

The orchestrator (T-01a17bda) asked for this after B0-33 m64r33 (`tests/am1/results/b033-m64.md`). That row's one
leader change, site 0 term 2 → site 1 term 3 at 00:14:13Z, came from a transfer that no line named: step-busy held
all 4 `CommitErrors` trips (`to: None`), and there was no `TransferAbandoned`, no `NoLead` and no `/transfer`. This
branch is logging only. No rule changed. No hosted spend.

## Every path that sends `TimeoutNow`, against m64r33 at 00:14:13

Each path goes through `start_transfer` (`crates/raft/src/member/transfer.rs`), and `tick_transfer` then sends
`TimeoutNow` to the target once it holds the leader's last entry.

| Path | Where | Logs today | Could it fire at 00:14:13? |
|---|---|---|---|
| `STEP_DOWN` → `step_down_to` | `health.rs`, `check_health` | `StepDown { why, to: Some(..) }` | **Yes, the only fit.** Site 0's confirms at 00:14:09.535 and 09.835 were 200 ms each on whole 100 ms ticks, with `lag_begin` about 10.4 s before: that meets `LOOP_LAG` (2 in 2 s, each ≥ 200 ms). step-busy holds only `CommitErrors`. But no `StepDown … to: Some` line exists near then (searches for `LoopLag`, `to: Some`, `stepDownWindow`). |
| `hand_off` after a `NoLead` refusal | `leader.rs`, refusal handling | `NoLead { site, refusal }` first | No: 0 `NoLead` lines. |
| `hand_off` from `maybe_hand_off`, drained or `no_lead_until` | `health.rs` | nothing | No: no `site_modes` PUT in the window (only GETs), and no release change. |
| Home return | `health.rs`, `maybe_hand_off` | nothing | No: site 0 is the group's home, and it was leading. |
| `Member::transfer`: `/transfer`, `/move` | `member.rs` | nothing | No: 0 such requests in site a's Workers Logs. The only `/campaign` calls were 2 at 00:08 (setup). |

**What the trace shows** (`tests/fanout/raw/tsave-m64r33.jsonl.xz`, `terms-m64r33.jsonl` on `b033-m64`):
- At 00:14:12.682 sites 0 and 2 are followers in term 3, both voted for site 1, with promises about 2.9 s out. Only a
  transfer vote sets `promise_until` to `CAMPAIGN_DEADLINE` (3 s) [measured].
- Site 0's last `AppendQueued` (entry 349) is at 00:14:09.535 and its last call to site 1 at 00:14:10.453. A started
  transfer stops `pump` [measured].
- **Reading:** a `LoopLag` step-down to site 1 whose `StepDown` line is missing [inference]. Site a has a canceled
  alarm invocation at 00:14:10.000 (wall 472 ms) and another at 00:14:13.510. That the line was lost with a canceled
  invocation is [unverified].
- Two of the five causes (drained or `no_lead_until`, and home return) had no line at all on main. The `STEP_DOWN` path
  had one, and it is missing here. A line at the transfer's start covers all five. Its trace row keeps the cause even
  when Workers Logs drops the line.

## The change

- `crates/raft/src/member`:
  - `Alarm::TransferStarted { to, why: TransferWhy }`, with `TransferWhy` one of `StepDown`, `Drained`, `NoLead`,
    `Home` and `Asked`. `TransferWhy` is exported from `lib.rs`.
  - `start_transfer` takes `why` in place of `step_down: bool`; `step_down` is now `why == StepDown`. The alarm is
    pushed only after every check passed, just before `broadcast_all`, so a refused transfer logs nothing.
  - `hand_off` takes the cause: `NoLead` from a refusal, and from `maybe_hand_off` `NoLead` when `no_lead()` holds,
    else `Drained`. `Member::transfer` passes `Asked`, and the home move passes `Home`.
- `workers/site/src/host.rs`:
  - `alarm_line` already prints every alarm, with the group: `member alarm <do name> <do id>: TransferStarted { to:
    SiteId(1), why: StepDown }`.
  - On a test-faults build, `Trace::Transfer` (kind 32, appended) has `peer` = the target and `batch` =
    `transfer_why_code` (1 to 5, in declaration order, append-only).
- `decisions/OPEN-QUESTIONS.md`, F108: the row names the new line and the trace row. The row count is unchanged (221).

## Tests

- `crates/raft/tests/leadership.rs`. A `started()` helper collects each `TransferStarted` as (leader, target, cause):
  - `transfer_hands_leadership_to_a_caught_up_target`: `[(A, B, Asked)]`, once. The refused second transfer (`Busy`)
    and the follower's (`NotLeader`) log nothing.
  - `a_refused_transfer_is_abandoned_at_once_and_leaves_no_lease`: alarms are now 2, a start and its abandon.
  - `commit_errors_trip_and_the_probe_picks_a_healthy_target`: `[(A, target, StepDown)]`.
  - `leadership_returns_home_after_it_recovers_and_catches_up`: none while home is failing, then `[(B, A, Home)]`.
  - `a_drained_site_hands_off_and_takes_no_leadership`: `[(A, C, Drained)]`.
  - `an_older_refuser_makes_the_leader_truncate_and_hand_off`: the first start is A's, `NoLead`.
- `workers/site/src/host.rs`, `a_member_alarm_line_names_its_group`: the codes are `[1, 2, 3, 4, 5]`, and a start's
  line names its group, target and cause.

## Plants

`tests/m0carried/plant72.py`: 10 of 10 caught on the final tree, exit 0.

| # | Mutant | Verdict |
|---|---|---|
| 0 | a started transfer logs nothing | CAUGHT |
| 1 | the alarm names the leader, not the target | CAUGHT |
| 2 | the alarm is pushed before the checks, so a refused transfer logs a start | CAUGHT |
| 3 | `/transfer` names another cause | CAUGHT |
| 4 | a step-down names another cause | CAUGHT |
| 5 | the move home names another cause | CAUGHT |
| 6 | a drained hand-off is logged as `NoLead` | CAUGHT |
| 7 | a refusal's hand-off is logged as `Drained` | CAUGHT |
| 8 | the trace row's cause codes shift | CAUGHT |
| 9 | the line names no group | CAUGHT |

## Long sweep and Porcupine

`RAFT_HISTORY=/tmp/sw/hist.jsonl RAFT_SEEDS=2000 RAFT_STEPS=10000 cargo test -q -p graftyard-raft --release --test sim
-- --ignored --nocapture`: exit 0, 1,070 s. Every count matches main's last recorded sweep
(`tests/am1/results/step-busy.md`) counter for counter:

| Phase | Step-downs / holds | Longest stuck stretch |
|---|---|---|
| harsh | 0 / 8,206 | 14,707 ms (seed 10439) |
| gray | 2,133 / 10,374 | 28,501 ms (seed 11893) |
| live (4,000 seeds) | 1,747 / 10,251 | 27,362 ms (seed 12553) |

Porcupine (`tests/m0/porcupine`, `-seg 2000`), each exit 0: `hist.jsonl` Ok, 146,836 operations; `hist.jsonl.gray` Ok,
2,336,111 operations.

## Checks

Each by its exit code, on the branch from main `546382691`:
- `cargo fmt --all -- --check` 0; `cargo clippy --workspace --all-targets -- -D warnings` 0;
- wasm32 clippy `-D warnings`: site 0, and 0 with `test-faults`; edge 0 and 0; ops 0, and 101 with `test-faults`,
  which ops does not have ("does not contain this feature: test-faults"; expected);
- `cargo test --workspace --no-fail-fast` 0: 1,331 passed, 0 failed;
- `go test ./...` in `containers/git/server` 0;
- `scripts/check_design.py` 0, `check_names.py` 0, `check_links.py` 0 (with `.worktrees` moved aside).
