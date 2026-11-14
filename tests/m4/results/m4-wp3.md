# M4-WP3, the Actions engine: results

Branch `m4-actions`, from main 2b42214e. The shapes are in `tests/m4/results/m4-wp3-design.md`, the runner contract in `tests/m4/wp3/runner-protocol.md`, and the decisions in `decisions/DECISIONS.md`, "M4-WP3: the Actions shard's commands, and `RunOwner`'s writes, 2026-11-14".

## Status

| Row | In memory | Hosted (graft-m4a) |
| --- | --- | --- |
| BF-4 | Pass, also with the real runner killed | Pass (run 1179649): failed at its lease end, 10 min after the renew answered, 10 min 4 s after it was sent |
| BF-37 | Parser and filter built (`merge_group`, `pull_request` `enqueued`/`dequeued`); trigger not wired | Not run |
| BF-54 | Not built | Waits for the step-3 estimate's approval (lead, 2026-11-14) |
| BF-55 | Not built | Not run |
| BF-56 | Pass (both kills) | Not run |
| BF-59 | `graft-ops` open, reseal, and rewrap (PyNaCl oracle); key rules (`crates/identity/tests/sealed.rs`) | **Pass** (19:07Z to 19:31Z): repository, environment, and org secrets read right before and after both rotations; a fork-PR job sees none; no plaintext in 1,296 committed entries, 34 run files, runner diagnostics, or Workers Logs |

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
- **Routing.** A runner's `clientId` holds its owner, registration shard, and id, and each token names its Actions shard, so the edge routes every run-service call with no lookup. Job GUIDs carry the full 53-bit id. The matrix run passed again after the change: 2 jobs `Success`, 154 live-log frames, 12 blobs [measured].
- **Found:** a job token's `orch_id` must hold no spaces, because the runner sends it in its `User-Agent` (`tests/m4/wp3/runner-protocol.md`).

## Hosted on graft-m4a: the run service and LogRelay

Site day 28 (14:15Z) put the `Actions` family, `/runsvc`, `RunnerBroker`, `LogRelay`, and `RUN_SERVICE_KEY` (Secrets Store) on all three sites. Edge days 10 and 11 by M4 web put `workers/edge/src/actions.rs` on the edge, with `RUN_SERVICE_PUBLIC`. Site day 29 (14:45Z) fixed the one hosted failure. `tests/m4/wp3/hosted-run.mjs` drives an unmodified `actions/runner` 2.338.2 through product routes only: it registers an ephemeral runner (`POST /api/graftyard/v1/repos/{o}/{r}/actions/runners`), writes its JIT config with `examples/jitconfig.rs`, and dispatches a workflow read from the git shard's `/ui/blob`.

- **Found on day 28:** a new Actions shard's first command answered 422 `NeedIdBlock`. `/runsvc` waited on its ticket without handing over the command, so the host's lease loop had nothing to re-send after leasing a block. The command now goes with the ticket, as `/commit`'s does (`f0282e80`).
- **Live log, 14:47Z** (`tests/m4/wp3/hosted/live.yml`, one line a second for 10 minutes): the runner took job 1114114 of run 1114113 on `m3org/wp3-live` (shard `actions:7100001.8`) 4 s after dispatch. A viewer on `/_t/logrelay/repos/m3org/wp3-live/jobs/1114114` got `hello` with the tail, then `line` messages p50 339 ms and at most 375 ms after the runner printed them, over 23 lines [measured]. The relay's `line.at` is to the second.
- **GHES reads** on the same run: `actions/runs/{id}`, `runs/{id}/jobs`, and `jobs/{id}` answer 200, with the runner's step report as `steps` (1 and 2 `completed`/`success`, 3 `in_progress`, 4 `queued`) [measured].
- **BF-4 hosted** (`tests/m4/wp3/hosted/bf4.yml`, one long step): the runner took job 1179650 of run 1179649 on `m3org/wp3-bf4` (shard `actions:7100001.11`). Its last `renewjob` was at 14:52:47Z and answered `lockedUntil` 15:02:50Z. The runner tree was killed at 14:53:22Z. The job read `Completed Failure` at 15:02:50.958Z, the lease end: 10 min 4 s after the renew was sent, 10 min after its answer set the lease. The row's fail line, still running at 11 minutes, did not happen [measured]. The driver process exited at about 15:01:49Z for a reason not found, so the job's state was read directly with the GHES `jobs/{id}` read.
- **Found by BF-4:** the GHES `job.completed_at` showed the last step report (14:52:47Z), because a runner killed mid-step reports no end. It is now the job's own completion time (`crates/actions/src/rest.rs`).
- **A running job's `/logs`** now answers the step logs uploaded so far, in upload order, instead of 404, as GHES serves a log so far (`workers/edge/src/actions.rs`). A step still running has no lines there until it ends.
- **BF-61's relay kill** (M4 web runs the row; this is the relay side). Site day 30 (15:40Z) added `LogRelay` `/abort`, `ctx.abort()` reached only through a binding. Job 1114120 of run 1114119 on `m3org/wp3-live` streamed through `logrelay:1114120` on site 0 with two viewers attached, and a scratch Worker bound to the `LogRelay` classes (`graft-m4a-relaykill`, deleted after) aborted it at 15:47:05.05Z. The runner noticed on its next send, about 27 s later, retried twice at once (two harmless `ResultServer` exceptions as the attempts raced), and reconnected within 1 s to a fresh relay with no tail. Its lease kept renewing and the job completed `Success` at 15:52:15Z. Its `/logs` then held all 600 counter lines with none missing (37,382 B) [measured]. M4 web's spec stalled on a page action after saving the log, so the lead asked for one clean printed rerun with no new site day: job 1114123 of run 1114122, relay on site 0, aborted at 16:20:00.113Z, behaved the same (the runner reconnected about 29 s later on its next send; `Success` at 16:25:37Z; all 600 lines in `/logs`) [measured]. The spec then printed a clean pass (`1 passed`, M4 web's `tests/m4/results/m4-wp5.md` on `m4-web-7` `f12a64e3`): 220 live lines with none over 2 s, the socket closed with no `end`, the page on saved logs 2.2 s later, and the final log holding every line the page saw live.
- **The `line.at` skew is the orb's clock.** The orb's clock runs about 5 s behind Cloudflare's: the kill Worker's `Date.now()` read 5.03 s after the orb's send, and the edge's `Date` 5 s after the orb's. The runner ran on the orb, so its timestamps, not the relay's, were behind [measured on two readings].
- **Not yet checked:** a viewer's close arrives about 10 s after `no-stream` (M4 web, edge day 10), or not within 12 s (edge day 11). The web viewer falls back on `no-stream` itself; BF-61's kill path falls back on the close, so it is checked there.

## BF-59 hosted: secrets across both rotations

Driver `tests/m4/wp3/bf59.mjs`, workflow `tests/m4/wp3/hosted/bf59.yml` (one job with `environment: bf59env`; each step prints a secret only as the first 16 hex of its SHA-256 and its length, and the variable as text). Repository `m3org/wp3-bf59` (65554), unmodified runner 2.338.2. Site day 32 (`17dff43c` / `57066014` / `30444906`), edge day 15 (`1524eb37`), then edge day 16 (`b9a67f48`) for the rotation fix below. Three random 29-character values were kept only in the run directory (mode 600).

| Step | Result |
| --- | --- |
| `gh secret set` 2.102.1: `BF59_REPO` (`-R`), `BF59_ENV` (`--env bf59env`), `BF59_ORG` (`--org m3org --visibility selected --repos wp3-bf59`); `gh variable set BF59_VAR` | each exit 0; the three list routes show the names and no value; one owner key `1478349346864660411` |
| Job 1179653 | Success; repo, env, and org each print the right hash and length 29; `var bf59-plain-variable` |
| Fork-PR job 1179656 (run marked `fork` through the dispatch route) | Success; all three print the empty value's hash `e3b0c44298fc1c14` and length 0; the variable still reads |
| `rotate-seal-key` (run 2) | 200: new key `3052770262353804737`; 4 records resealed (3 repository and environment, 1 org); keys `1478349346864660411` and `6243795946503046962` retired and dropped; afterwards no repository row or the org's fold names either |
| Job 1179659 after it | Success, all three right |
| Wrapping key: slot B created (`graft-m4a-secrets-wrap-key-b`); `graft-m4a-ops` `cb75d656` with A and B, B current; `rewrap-secrets` | 200: 1 `SealKey` and 65 hook secrets rewrapped under slot 1 across 16 repositories; a second run rewraps nothing |
| Slot A cleared: `graft-m4a-ops` `459c7362` with B alone | job 1179662 Success, all three right; a ping to hook 229398 arrived signed (`x-hub-signature-256`), 200 |
| Scan (`bf59.mjs scan`) | 0 hits for any value, its base64, or its hex in every committed entry of `repo:65554` and `authz:7100001` on all three members (1,296 entries) and in 34 run files including runner logs and diagnostics |
| Workers Logs (`tests/m4/wp3/bf59-logs.mjs`, 18:40Z to 19:31Z) | 0 hits in messages or request URLs of the edge, ops, and three site scripts (423, 148, 17,511, 7,211, 3,289 events); a control query for strings that are logged matched 6 and 18 events |

Found on the way, both fixed and redeployed before the run that counts:

- **An empty 201 broke `gh secret set`.** go-gh decodes every non-204 body, so a new secret answered 201 with no body made `gh` report "unexpected end of JSON input" although the write committed. A new secret or variable now answers 201 `{}` (`What::ActionsConfig`, kept test in `crates/api/tests/surfaces.rs`). Site day 32 and edge day 15.
- **Rotation reseals shared one request id.** The first `rotate-seal-key` resealed the org secret and one repository secret of three, and kept the old key: every repository command it committed carried the edge request's id, so the leader answered the second and third as replays of the first. Each command a rotation commits now has its own id (`commit_issue_as`). Edge day 16; the rotation run again finished the reseal and dropped both older keys.
- The driver's first job read `/logs` before the job's log was joined and through a JSON helper that cut a text body to 300 characters; it now reads raw and waits for the joined log. Jobs 1179653 and 1179656 were checked from a raw re-read of the same `/logs`.

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

`INSTALL=graft-m4a python3 tests/fanout/m2bcost.py`, 2026-11-14 14:10Z to 14:50Z (site days 28 and 29, the edge days, and the live run): $0.53, of which containers $0.51 (git shards started by the resets). The whole install 00:00Z to 14:50Z: $8.47, containers $7.66.
