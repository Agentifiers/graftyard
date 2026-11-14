# M4-WP3, the Actions engine: results

Branch `m4-actions`, from main 2b42214e. The shapes are in `tests/m4/results/m4-wp3-design.md`, the runner contract in `tests/m4/wp3/runner-protocol.md`, and the decisions in `decisions/DECISIONS.md`, "M4-WP3: the Actions shard's commands, and `RunOwner`'s writes, 2026-11-14".

## Status

| Row | In memory | Hosted (graft-m4a) |
| --- | --- | --- |
| BF-4 | Pass, also with the real runner killed | Not run |
| BF-37 | Parser and filter built (`merge_group`, `pull_request` `enqueued`/`dequeued`); trigger not wired | Not run |
| BF-54 | Not built | Waits for the step-3 estimate's approval (lead, 2026-11-14) |
| BF-55 | Not built | Not run |
| BF-56 | Pass (both kills) | Not run |
| BF-59 | Not built | Not run |

A row passes only hosted, with an unmodified `actions/runner`. The in-memory halves below check the engine's rules on real members first.

## What is built

- `crates/actions`: `JobState`, `transition`, `derive_run_status`, `LabelSet`, `actions_shard`, the shard's fold (`ActionsFold`, an overlay scratch) and commands (`ActionsCmd`, format 0x15.1), and `RunOwner`'s pure round (`run_owner`).
- `crates/git/src/run_checks.rs`: `RepoCmd::RunChecks`, format 32.21, `RunOwner`'s check suite, check runs, and required checks on the repository chain.
- `crates/actions/src/workflow`: the workflow parser, a Rust port of the runner's `WorkflowParser` with its expression grammar, `MatrixBuilder`, and the `on:` filters. See "The parser" below.
- Wire tags: the Actions enums and `RunChecksOut`, appended to `crates/git/tests/wire_tags.golden`. Main's last `RepoEvent` tag is still 42. No `RepoEvent` or `IssueOp` variant was added.

## In memory: `crates/actions/tests/gates.rs`

Three real `Member<ActionsCmd>`s of one owner's shard and three real `Member<RepoCmd>`s of the repository. Simulated runners commit what the run service will commit. The leader's host commits `ExpireLeases` at the fold's next deadline, and `RunOwner` runs `run_owner::round` against both groups. Time moves in 500 ms steps.

- **BF-4.** One job on one runner renews every 60 s for 7 minutes, which commits 1 or 2 lease rewrites (`LEASE_REWRITE_UNDER`). Then the runner is killed and the group left idle. The job completes `Failure` at its lease end, within 10 minutes of the last renew. After the kill the only entries are the lease alarm's and `RunOwner`'s.
- **BF-56, commit path.** Two runs of 6 jobs on two open PR heads, every job required by the branch rule, with `needs` chains: build, then test, then deploy, and lint, then report, then notify. Lint fails, so report and notify are skipped. The second run is cancelled mid-run. The repository's leader site is killed before `RunOwner` writes anything and recovers by election. Every required check has its conclusion within 60 s of its job's `Completed`, and every check-run version committed at most once, in order.
- **BF-56, `RunOwner`.** The same, with `RunOwner` killed for 20 s after a round's repository writes and before its shard half. The cursor stays at 0, the replayed round's repository writes commit nothing new, and the row's pass line holds.
- **One runner at a time.** Two runners matched to one job in one entry: the second is refused `Stale`. A runner whose labels lack the job's is refused, and a second run for one trigger is refused `Duplicate`.
- **The abort path.** A repository deleted mid-run answers `Gone`, and `RunOwner` cancels the run's unfinished jobs.

`cargo test -p graftyard-actions`: 24 unit tests, 6 gate tests, and the parser's differential test pass.

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

## The run service with an unmodified runner: `crates/actions/examples/runsvc_host.rs`

The run service's pure half is `crates/actions/src/runsvc.rs` (JIT config, the OAuth assertion, job tokens, the job message, steps, cancellation) and `crates/actions/src/relay.rs` (LogRelay's tail and its viewer messages). The example host runs them in memory on three real `Member<ActionsCmd>`s behind the run service's HTTP and WebSocket routes, and `tests/m4/wp3/run-host.sh` drives `actions/runner` 2.338.2, unmodified, against it.

- **A matrix completes** (`tests/m4/wp3/run/ci.yml`, 2 jobs, ephemeral runners): both jobs `Completed Success`, step logs PUT to the blob route, and 15 live-log frames arrived over `FeedStreamUrl` [measured].
- **BF-4 with the real runner** (`long.yml`, `sleep 1200`): the runner tree was killed with `kill -9` at 09:28:15Z. Its last `renewjob` had answered `lockedUntil` 09:37:56Z, and the leader's `ExpireLeases` alarm completed the job `Failure` at 09:37:56.766Z, 10 minutes after that renew, inside the row's 10 [measured]. After the kill the only commit was the alarm's.
- **Found:** a job token's `orch_id` must hold no spaces, because the runner sends it in its `User-Agent` (`tests/m4/wp3/runner-protocol.md`).

## The parser: `crates/actions/src/workflow`

- **`template`** reads YAML (`yaml-rust2`, default features off, which builds for wasm32) into template tokens and validates them against `workflow-v1.0.json`, the oracle's schema with descriptions removed, so allowed contexts, required properties, and error text come from the same data the runner uses.
- **`expr`** ports `@actions/expressions`' lexer, parser, and evaluator, with the oracle's error text, and JavaScript's and .NET's number printing (`G15` names matrix jobs).
- **`mod`** converts as `convertWorkflowTemplate` does: events and their filters, jobs, `needs` checked for unknown jobs and cycles, `if` given `success() &&` unless it calls a status function, step ids (`__run`, `__run_2`, `__owner_repo`, `__self`, a docker image's), and reusable-workflow calls (parsed, not run).
- **`matrix`** ports the C# `ConvertToStrategy` and `MatrixBuilder`. The npm oracle has none, so its tests are GitHub's documented include and exclude examples plus id and name rules read from the C# source, with expected values written by hand.
- **`filter`** decides `on:` for push branches, tags, and paths, `pull_request` and `merge_group` activity types and base branches, and `workflow_dispatch`. Pattern rules are act's `workflowpattern`, checked against GitHub's cheat-sheet rows.

**Differential test** (`crates/actions/tests/workflows.rs`): 59 workflows, 43 of them public (ripgrep, tokio, cargo, serde, TypeScript, node, cli, uv, ruff, rust-analyzer, home-assistant, next.js, react, deno, cpython, vscode, docker/build-push-action, workerd, and others) and 16 written for the port, each parsed by the port and compared with `@actions/workflow-parser` 0.3.61's JSON (`*.oracle.json`, written by `tests/m4/wp3/oracle.mjs`), errors included. All 59 match. Two differences are allowed and recorded: a multi-line string's expressions are read from its value, as the C# parser does, not from its raw YAML source (`07-scalars.yml`), and a YAML syntax error must match in position, not in the YAML library's wording (`14-bad-yaml.yml`).

**Plants** 18 to 40 in `tests/m4/wp3/plant.py`: all 23 caught. The first run missed one, "a job name falls back to its id only when absent", because no workflow had `name: ''`. `16-names.yml` covers it now.

## Checks

`cargo test -p graftyard-git -p graftyard-actions -p graftyard-api`, `cargo clippy -p graftyard-actions -p graftyard-git --all-targets -- -D warnings`, `cargo clippy -p graftyard-site --target wasm32-unknown-unknown -- -D warnings`, `cargo fmt --all -- --check`, and `scripts/check_design.py`, `check_names.py`, and `check_links.py` all pass.

## Cost

No hosted spend in step 2 so far: nothing has been deployed.
