# M4-WP3, the Actions engine: the design as built

This file fixes the shapes M4-WP3 builds before the code (`AGENTS.md`, "How to change the design"). DESIGN Shape, "Identity", and SHARED, "Actions runs and jobs", "Actions", and "Multi-chain operations", Workflow run, give the types. What follows is what they leave open, each with its evidence tag, and the order the work lands in. Decisions that DESIGN or SHARED must carry go there and into `decisions/DECISIONS.md` as each lands.

The runner contract is measured in `tests/m4/wp3/runner-protocol.md`, against the unmodified `actions/runner` 2.338.2.

## Formats and tags

- **A new format family, Actions 0x15.** Families 0x10 to 0x14 are identity, 0x20 the repository, and 0x30 the `Release` group, and no branch on origin uses 0x15 (checked 2026-11-14) [measured]. `ActionsEvent` and `ActionsCmd` are new enums, so they take tags from 0, appended in `crates/git/tests/wire_tags.golden`.
- **Repository format 32.21** for `RepoEvent::ActionsSecret` and `ActionsVariable` (SHARED, Secrets and variables), with `RepoEvent` tags appended after main's last. 32.20 is `REPO_FORMAT_SPLIT` on `b030-split`, so it is left free [measured].
- **Authz** gets the org secret, variable, and `SealKey` rows in the next free Authz format at the time they land (0x12.5 is free on every branch, checked 2026-11-14) [measured].
- `IssueOp`, `RepoCmd`, and `RepoEvent` are re-checked against `origin/main` and every open branch's `wire_tags.golden` right before each tag is pinned.

## The Actions chain

- `ChainId::Actions(owner, shard)`, 16 shards per owner (DESIGN, `actions_shard`). `actions_shard(run) = hash(run.repo) % 16`, with SHA-256 of the repository id's 8 bytes, first byte mod 16 [inference: DESIGN names `hash` without one; SHA-256 is the crate's hash].
- **The fold** holds runs, jobs (state, version, spec, attempt), runners, and sessions, with no tiers [inference: an Actions shard's rows are small and short-lived, so a plain `Fold` as `Ids` and `Names` use is the simplest that fits].
- **Commands** (`ActionsCmd`): `CreateRun` (the `RunCreated` commit point, refused for a second `(chain, index, workflow path)`), `Job { job, version, event }` (applies `transition` against the job's version), `PutRunner`, `DeleteRunner`, `OpenSession`, `CloseSession`, `SetRunStatus` (only `RunOwner` sends it), and `ExpireLeases { now }`, which the leader's alarm sends and which commits `LeaseExpired` and `QueueExpired` for every job past its deadline at the entry's time.
- **Leases.** `Assigned` and `InProgress` hold `lease_until`. `acquirejob` sets it to the entry's time plus `JOB_LEASE` (10 minutes, SHARED). `renewjob` rewrites it only when less than `LEASE_REWRITE_UNDER` (5 minutes) remains, so a runner renewing every 60 s [measured, `runner-protocol.md`] writes about once every 5 minutes. `ExtendLeases` moves every lease by the recorded stall, as the repository's leases do. A lease whose job reaches `lease_until` with no renew gets `LeaseExpired`: `Completed { Failure }` from `InProgress`, back to `Queued` from `Assigned` (SHARED, Leases).
- **The queue limit** is 24 h (SHARED), from `Queued.since`.

## Run creation and `RunOwner`

- **`ActionsTrigger`** is a consumer of the repository chain, with its own cursor on the repository chain like `Webhooks`'s. For each committed push or pull request entry it reads `.github/workflows/*.yml` at the entry's SHA from the git cache, parses each, keeps those whose `on:` matches, expands the matrix, and commits `CreateRun` on `actions_shard(run)`. The `(chain, index, workflow path)` key makes a replay commit nothing twice (SHARED, Idempotent run creation).
- **`RunOwner`** is a consumer of each Actions shard, with a cursor on the shard. From `RunCreated` it appends to the repository chain the check suite, one `CheckRun` per job, and `RequiredCheck { conclusion: None }` for each job whose name a branch rule requires. From each `Completed` it appends the job's `CheckRun` conclusion and, when required, its `RequiredCheck`. It recomputes `derive_run_status` and commits `SetRunStatus` when the status changes. Every repository entry is keyed by (job, attempt), so a replay appends nothing twice. Its cursor passes a `Completed` only after the repository acknowledged or refused its entries (SHARED, Workflow run). This is BF-56.
- **`Cancel`** is the abort path. A refused repository entry, a user's cancel, or a concurrency group commits `Cancel` for every unfinished job of the run, and a running job's runner gets `JobCancellation` on its next poll [measured: the runner posts `completejob` `canceled` and exits].

## The broker

- **`RunnerBroker { owner, scope }`** is a site-script Durable Object class on its registration shard's leader site, derived only (DESIGN). It keeps, per shard it reads, the queued jobs' distinct label sets and the oldest job per set (SHARED, Label matching). It loads each shard's queued summary on start, then applies counted hints from each shard leader, and reloads a shard whose hint skips a count or that sent none for 30 s.
- **A poll** (`GET message`) is held up to `POLL_HOLD` (50 s [inference: under the runner's 100 s HTTP timeout, measured]). When a queued set is a subset of the runner's labels, the broker asks the job's shard to commit `Matched(runner)` against the job's version, and only a committed match becomes the runner's message. More than `BROKER_HELD_MAX` (2,000) held polls get 503 with `{"source": "actions-broker-listener"}` and `Retry-After: 15`, which the runner honours, since it is at least 10 s [measured]. BF-54 measures this with 2,500 real runners on Sandboxes (lead decision, 2026-11-14).
- **Sessions** are committed entries (`OpenSession`, `CloseSession`), so any broker object accepts a session another opened, and a broker restart loses no session (BF-55).

## The run service

- **Routes** under the install's host, each with the trailing `/` the runner needs [measured]: `/_services/broker/` (session, message, acknowledge), `/_services/run/` (`acquirejob`, `renewjob`, `completejob`), `/_services/results/` (twirp), `/_services/oauth/token`, and the job log blobs.
- **Credentials.** The runner's OAuth exchange verifies the RS256 assertion against `RunnerRecord.key` and answers a `Credential::RunnerJwt` token. The job token is a three-part JWT, which the runner requires [measured]: Graftyard's own EdDSA JWS, as `ghs_` tokens are signed (`crates/identity/src/stateless.rs`), with `orch_id` and the job and attempt, and it is `Credential::RuntimeToken(JobId)` [inference: the runner reads only `orch_id` and never checks the signature, so any key works; reusing the existing signer adds no key].
- **`acquirejob`** commits `Acquired` on the job's shard, then assembles the job message from the run's spec: context data and template tokens in the runner's JSON forms [measured], the job's secrets opened through `graft-ops`, and `FeedStreamUrl` naming the job's `LogRelay`.

## Workflows

- **The parser** is a Rust port of the runner's `GitHub.Actions.WorkflowParser` and `MatrixBuilder`, with `@actions/workflow-parser` 0.3.61 as the differential oracle (SHARED, Workflow engine). The schema is data: the oracle's `workflow-v1.0` with descriptions removed, so the port reads every definition the runner reads, not only the gates' subset [measured: 59 workflows match, `tests/m4/results/m4-wp3.md`]. YAML comes from `yaml-rust2` 0.10 with default features off [measured: builds for wasm32]. `MatrixBuilder` is ported from the C# source, because the npm oracle has no matrix builder.
- **`on: merge_group`** runs on the queue ref the merge queue built (`crates/git/src/queue.rs`), with `github.ref` the queue ref and `github.event_name` `merge_group`. BF-37 adds go-playground/webhooks parsing of `merge_group`, `pull_request.enqueued`, and `pull_request.dequeued`, which M4-WP1's payloads already carry.

## Logs

- **`LogRelay { job }`** is a site-script class on the job's shard leader site at `acquirejob`, named by `FeedStreamUrl`. It accepts the runner's WebSocket (text frames of `{Count, Value, StepId, StartLine}` [doc]) and web viewers' WebSockets, keeps the last 1,000 lines, and stores nothing. Its sockets hibernate. A viewer with no runner stream reads the step log blobs instead.
- **Step logs** go through the results service to the blob store as the runner PUTs them [measured: one PUT per step log], keyed by job and step.

## Secrets and variables

- **`SealKey`** per owner: X25519 from `crypto_box`'s sealed box, which opens libsodium's `crypto_box_seal` [measured 2026-11-14: a PyNaCl `SealedBox` ciphertext opened in Rust with `crypto_box` 0.9, `seal` feature, on wasm32]. Its private half is `Wrapped` under `SecretsWrapKey` by `graft-ops`, as hook secrets are (`workers/ops/src/lib.rs`).
- **Who opens.** Only job-message assembly at `acquirejob`, through `graft-ops` `POST /actions/open`, which takes the job's sealed records and the wrapped `SealKey` and answers the plaintexts for that message alone. A fork-PR job gets none (SHARED). Rotation reseals inside `graft-ops` and returns no plaintext (BF-59).

## What gates each row

| Row | Built from |
| --- | --- |
| BF-4 | `ExpireLeases` at lease end, and an unmodified runner killed mid-job |
| BF-37 | the parser's `merge_group`, `ActionsTrigger` on queue refs, and queue-ref deletion by the merge queue |
| BF-54 | the broker's held-poll cap, real runners on Sandboxes (waits for the step-3 estimate) |
| BF-55 | committed sessions and matches, the broker rebuilt on the new leader site |
| BF-56 | `RunOwner` keyed by (job, attempt), its cursor rule, and `Cancel` |
| BF-59 | `SealKey`, `Wrapped`, `graft-ops` open and rotation, fork-PR refusal |

## Order

1. `crates/actions`, pure: `JobState`, `transition`, `derive_run_status`, `WorkflowRun`, `JobSpec`, runners and sessions, the fold and `ActionsCmd`, `actions_shard`, label matching, and the kept tests and plants. BF-4's and BF-56's in-memory halves on three real members.
2. The parser and `MatrixBuilder`, with the differential oracle.
3. The run-service routes in memory against the unmodified runner through the orb's own HTTP server (the `crates/actions` logic driven by a test host), so the runner completes a job before anything is hosted.
4. Secrets, `graft-ops` open and rotation.
5. Hosted: the Actions family on the site script, `RunnerBroker`, `LogRelay`, the edge routes, the consumers, then the gates on graft-m4a.
