# M4-WP3, the Actions engine: results

Branch `m4-actions`, from main 2b42214e. The shapes are in `tests/m4/results/m4-wp3-design.md`, the runner contract in `tests/m4/wp3/runner-protocol.md`, and the decisions in `decisions/DECISIONS.md`, "M4-WP3: the Actions shard's commands, and `RunOwner`'s writes, 2026-11-14".

## Status

| Row | In memory | Hosted (graft-m4a) |
| --- | --- | --- |
| BF-4 | Pass | Not run |
| BF-37 | Not built | Not run |
| BF-54 | Not built | Waits for the step-3 estimate's approval (lead, 2026-11-14) |
| BF-55 | Not built | Not run |
| BF-56 | Pass (both kills) | Not run |
| BF-59 | Not built | Not run |

A row passes only hosted, with an unmodified `actions/runner`. The in-memory halves below check the engine's rules on real members first.

## What is built

- `crates/actions`: `JobState`, `transition`, `derive_run_status`, `LabelSet`, `actions_shard`, the shard's fold (`ActionsFold`, an overlay scratch) and commands (`ActionsCmd`, format 0x15.1), and `RunOwner`'s pure round (`run_owner`).
- `crates/git/src/run_checks.rs`: `RepoCmd::RunChecks`, format 32.21, `RunOwner`'s check suite, check runs, and required checks on the repository chain.
- Wire tags: the Actions enums and `RunChecksOut`, appended to `crates/git/tests/wire_tags.golden`. Main's last `RepoEvent` tag is still 42. No `RepoEvent` or `IssueOp` variant was added.

## In memory: `crates/actions/tests/gates.rs`

Three real `Member<ActionsCmd>`s of one owner's shard and three real `Member<RepoCmd>`s of the repository. Simulated runners commit what the run service will commit. The leader's host commits `ExpireLeases` at the fold's next deadline, and `RunOwner` runs `run_owner::round` against both groups. Time moves in 500 ms steps.

- **BF-4.** One job on one runner renews every 60 s for 7 minutes, which commits 1 or 2 lease rewrites (`LEASE_REWRITE_UNDER`). Then the runner is killed and the group left idle. The job completes `Failure` at its lease end, within 10 minutes of the last renew. After the kill the only entries are the lease alarm's and `RunOwner`'s.
- **BF-56, commit path.** Two runs of 6 jobs on two open PR heads, every job required by the branch rule, with `needs` chains: build, then test, then deploy, and lint, then report, then notify. Lint fails, so report and notify are skipped. The second run is cancelled mid-run. The repository's leader site is killed before `RunOwner` writes anything and recovers by election. Every required check has its conclusion within 60 s of its job's `Completed`, and every check-run version committed at most once, in order.
- **BF-56, `RunOwner`.** The same, with `RunOwner` killed for 20 s after a round's repository writes and before its shard half. The cursor stays at 0, the replayed round's repository writes commit nothing new, and the row's pass line holds.
- **One runner at a time.** Two runners matched to one job in one entry: the second is refused `Stale`. A runner whose labels lack the job's is refused, and a second run for one trigger is refused `Duplicate`.
- **The abort path.** A repository deleted mid-run answers `Gone`, and `RunOwner` cancels the run's unfinished jobs.

`cargo test -p graftyard-actions`: 9 unit and 6 gate tests pass.

### Plants: `tests/m4/wp3/plant.py`

| # | Mutant | Verdict |
| --- | --- | --- |
| 0 | a lease never expires a running job | caught |
| 1 | an expired lease from `InProgress` requeues the job | caught |
| 2 | every renew rewrites the lease | caught |
| 3 | a renew sets the lease past `JOB_LEASE` | caught |
| 4 | a completed job takes a cancel | caught |
| 5 | a match commits against a stale version | caught |
| 6 | a match ignores labels | caught |
| 7 | a second run for one trigger | caught |
| 8 | a stall moves a lease by more than its length | caught |
| 9 | a stall moves no lease | caught |
| 10 | a stall revives a lease that had ended before it | caught |
| 11 | a failed need still releases its dependents | caught |
| 12 | a skip does not release its own dependents | caught |
| 13 | the `RunOwner` cursor moves back | caught |
| 14 | `RunOwner` never cancels a gone repository's run | caught |
| 15 | a replayed check run moves its row back | caught |
| 16 | a completed check writes no conclusion | caught |
| 17 | every job's check row is version 1 | caught |

The first run missed four plants. Each miss led to a fix: the stall test in `shard.rs`, a `notify` job behind `report` with lint finishing last, the cursor test, and the abort-path test. One plant was dropped: "a completed run takes another status" cannot happen, because a completed run's jobs take no event, so the guard was removed instead.

## Checks

`cargo test -p graftyard-git -p graftyard-actions -p graftyard-api`, `cargo clippy -p graftyard-actions -p graftyard-git --all-targets -- -D warnings`, `cargo clippy -p graftyard-site --target wasm32-unknown-unknown -- -D warnings`, `cargo fmt --all -- --check`, and `scripts/check_design.py`, `check_names.py`, and `check_links.py` all pass.

## Cost

No hosted spend in step 2 so far: nothing has been deployed.
