# AM1-WP3 results: external tasks and the task read endpoints (the in-memory half)

These results are from 2026-11-08, on branch `am1-wp3` (worktree `.worktrees/am1-wp3`), cut from `main` at `e2c67b5`. Nothing was deployed, pushed, or merged. AM1-WP3's own gate is LF-1. The package also carries LM-1's "base journeys pass unchanged" (`build/BUILD-PLAN.md`, AM1-WP3). This is the in-memory half. Every gate below ran on three real `graftyard_raft::Member<AuthzCmd>` of the install's `Authz(Genesis.account)` chain and three real `Member<RepoCmd>` per repository, using the harness AM1-WP2 built (`crates/git/tests/common/mod.rs`, moved out of `agent_gates.rs`). Each repository leader read the install chain through `fresh` once per entry, and read the owner's `Authz` as a `dep`. The hosted rerun comes later. The section "What the hosted half needs" lists it.

## Gates

| Row | What ran | Pass line | Result |
| --- | --- | --- | --- |
| LF-1 | 10 repositories, each its own three members, owned by one account. A confined vendor App (`CLAUDE`) installed on all 10 created 100 `external` tasks, 10 per repository, with two requesters. A third of them were moved to `in_progress`. A second confined App (`MCP`) created one task per repository as the control. Before the uninstall, 100 of 100 read live. The uninstall is the event `AuthzOp::Uninstall` decides (`Installation { row: None, not_before: at }`), applied to the owner's `Authz` fold that every repository leader reads. After it, at the next read through `graftyard_api::tasks`: 100 of 100 read `cancelled` in `GET .../tasks` (paged by 3 through `Link`), 100 of 100 in `GET .../tasks/{id}`, 100 of 100 under `?state=cancelled&app_id=`, and 0 under `state=queued`. The reads wrote 0 task events on all 10 logs. Each task's next command was a cancel by its requester or the repository admin. It recorded `TaskUpdated { state: Cancelled }` for 100 of 100, and rendered from the `&Acked` as 409 `{state: "cancelled"}` for 100 of 100. The App's own `PATCH` was refused 10 of 10 (its token no longer reaches the repository) and wrote nothing. The control App's tasks stayed `queued`, 10 of 10 (`crates/api/tests/task_api.rs`, `lf1_uninstall_cancels_every_task_with_no_sweep`) | Every task reads `cancelled` at the next read with no sweep command, and its next command records it | Pass (in memory) |
| LM-1 "base journeys pass unchanged" | `cargo test --workspace` (every base suite: push, issues, prsync, merge queue, shard, encoding, the gh, git-lfs, and release replays, surfaces, raft). The `GRAFTYARD_REPLIES` dump of `graftyard-api`'s tests on this branch was compared with `main`'s using `diff -r`. `agent_bytes.golden` (AM1-WP2's byte-identity gate) checked unchanged | Unchanged | Pass. 120 of 120 reply files identical. `agent_bytes` passes against the golden file pinned at `49d63f6`, untouched |
| B0-36 / B0-37 (LM-1 rerun, carried from AM1-WP2) | `agent_gates.rs`, rerun on the task-aware code | As AM1-WP2 | Pass. Same counts as AM1-WP2: B0-37 10,000 pushes, 0 parse failures, every forged trailer `Claimed`, 0 wire `Session` stored, every `refs/graftyard/` update `Hidden`. A forged `Session` is still dropped, because no task of the pusher's App names it |

## Task rules and linking, on the same members (`crates/git/tests/task_gates.rs`)

- **Linking (where AM1-WP2 refused every Graftyard-run push).** A `GraftyardRun` task of `RUN` (recorded with `TaskCreated` in 32.12, because Graftyard-run creation is AM3-WP3's), 100 rounds:
  - 100 of 100 pushes under a token whose `task` claim names the live task, inside `refs/heads/graftyard/<uuid>/…`, were accepted. They crossed the member wire in 32.12.
  - 100 of 100 of their `Graftyard-Session` trailers were stored as `Source::Session(task)`, linked by the edge's `agent::link` and kept by `decide`'s `own_sessions`.
  - 100 of 100 claim-less pushes were refused `NoLiveTask`. 100 of 100 claimed pushes outside the prefix were refused `OutsidePrefix`. 200 of 200 to another task's prefix or to the bare `refs/heads/graftyard/<uuid>` were refused `OutsidePrefix`. 100 of 100 with a claim on a task that does not exist were refused `NoLiveTask`.
  - The same trailer pushed by `CLAUDE` stays `Claimed`. A `Session` that `CLAUDE` forges on the wire is dropped.
  - Another Graftyard-run App whose claim names `RUN`'s task is refused `NoLiveTask`. After the task is paused (`waiting_for_user`), and again after it is cancelled, the next claimed push is refused `NoLiveTask`.
  - Entries: a claimed push is 32.12, and an unclaimed App push stays 32.8.
- **`graftyard-workspace`** (a `GraftyardRun` `AgentApp` with that slug):
  - It creates `refs/graftyard/sessions/<id>` for its live task, and moves it from the tip it names. A stale tip is refused.
  - Its delete, its write of another session's ref, and the same write by `RUN`, `CLAUDE`, the owner, or an unclaimed workspace token are all refused `Hidden`.
  - It may push the task's branches inside the prefix.
- **`AgentMeta` from the task.**
  - A draft PR opened under the claim records `session` and `requester`. The requester's approval writes no `AgentGate`, and another writer's does.
  - `X-Graftyard-Agent-Session` (the `IssueCall.session` wire field `agent_session`) records the session and requester only for a task of the opening App. Another App's session records neither. Both go through the JSON issue-command wire.
- **External task rules.**
  - Who may create:
    - Only an `AgentApp`'s installation token may create a task, and only in its own App's namespace. A plain App gets 403 and a user gets 403.
    - Another App's `<app_id>.name` is 422 `invalid`.
    - A malformed name is 422.
    - A duplicate is 422 `already_exists`, both as a bare name and as `<app_id>.name`.
  - `PATCH`:
    - Only the task's App may send it. Another App or a user gets 403.
    - Never `cancelled` or `timed_out`: 422.
    - `pr_number` must name a pull request: 422.
    - `summary` is stored in Metadata.
  - Cancel:
    - The requester, a repository admin, or the task's App may cancel.
    - A reader or another writer gets 403. A user without access gets 404.
  - Final states:
    - They stay final: `PATCH`, and cancel, of a `cancelled` or `completed` task are 409 with the state, and write 0 entries.
    - Every task entry was 32.12.
- **Caps** (`open_task_caps`):
  - 100 open per requester, then 422. Another requester and the App's own (no requester) still have room.
  - A cancel or a `completed` frees a place.
  - 2,000 open per repository, then 422.
- **`TASK_IDLE`** (`an_idle_task_times_out_and_its_next_command_records_it`):
  - A task with no update for 24 h reads `timed_out` with no sweep, and 0 entries are written.
  - An empty `PATCH` before then keeps the other task live.
  - The next command records `timed_out` and answers 409 with that state.
- **Minting** (`a_task_token_needs_a_live_run_task_of_the_app_on_its_repository`). `task::token_task` on the fold a token request reads:
  - It accepts the task's App and the install's `graftyard-workspace`.
  - It refuses:
    - another App;
    - `repositories` missing (all, or empty);
    - a second repository;
    - an unknown task;
    - an external task;
    - an uninstalled App;
    - a reinstall after the task was created.
- **The read endpoints' shape** (`the_read_endpoints_serve_the_reference_shape`):
  - The fields follow the API reference's `task`.
  - `prompt` is only present with `contents: read`, and `leases` only on the single GET (`[]` until AM1-WP4).
  - Lists are in id order, with filters by `state`, `app_id`, `requester` (a login), and `issue_number`.
  - `per_page` and `Link` `rel="next"` carry an opaque `after`. An invalid `after` or `state` is 422.
  - Without `metadata: read` the reply is 404. `/log` is 404 until AM3 pushes the session log.
  - A create's reply is 201, rendered from the `&Acked`.

## Planted bugs (`tests/am1/wp3/plant.py`)

PLANTS_TABLE

## Bugs the gates found in my own code

1. **A Graftyard-run `SessionId` could not cross the JSON issue-command wire.** `SessionId::Run(u128)` serialized with serde's derived form, and `serde_json` has no 128-bit integers ("number out of range"). So any `IssueCall` that named a UUID session failed to parse on the member: a task command on a Graftyard-run task, `X-Graftyard-Agent-Session`, or a `ghs_` claim in the JSON form. `SessionId` now serializes as its string form in human-readable formats (JSON, the API, the grant) and keeps the derived enum form in postcard and the canonical encoding (the fold rows and AM1-WP2's `provenance` rows). Bytes stored by AM1-WP2 are unchanged. `crates/fold/src/canon.rs` now answers `is_human_readable() == false`, as postcard does. Tested by `a_session_is_its_string_in_json_and_its_enum_in_postcard` and caught by plant 23.
2. **A task survived a reinstall.** `installed` first checked only that a row existed for the App. An uninstall followed by a reinstall then revived every old task, which an old token never could. `installed` now needs `task.created >= not_before`, as `app_permission` needs `iat >= not_before`.

## What was built

- **Identity:**
  - `SessionId` moved to `graftyard_identity::agents`, because the `task` claim carries one. JSON uses its string form.
  - `InstallationClaims.task`, in the JWT payload only when set.
  - `Grant.task`, which the minter copies.
- **Repository (format 32.12, `REPO_FORMAT_TASKS`):**
  - Events `TaskCreated` (tag 31, Core and Metadata) and `TaskUpdated` (tag 32, Core; also Metadata when it sets `pr` or `summary`).
  - Core rows `agent_task` (`TaskCore`, which carries `installation`, `created`, and `updated`) and `agent_task_open`. The metadata row is `agent_task_meta`.
  - The task claim is encoded after the principal in 32.12 pushes and in `principal_bytes` (only when set). `Push`, `IssueCall`, and `MergeCmd` take 32.12 only with a task op, a claim, or a session, so no other command changes format or bytes.
- **`crates/git/src/task.rs`:**
  - Types: `TaskKind`, `TaskApiState`, `TaskActor`, `AgentTask`.
  - Derived states: `effective` and `installed`.
  - Caps, `run_refs`, `live_run_task`, `own_sessions`, and `token_task`.
  - `decide` for `CreateTask` (external), `UpdateTask`, and `CancelTask`. A derived final state is recorded by the task's next command, which answers `TaskFinal` (409).
- **`decide` wiring:**
  - `Push::decide` binds a claimed token to its live `GraftyardRun` task: `AgentActor.task` and `requester`, prefix `refs/heads/graftyard/<uuid>/*/**`. A claim on a paused, cancelled, or timed-out task binds nothing.
  - `graftyard-workspace` writes its task's session ref, never a delete.
  - Provenance keeps a `Session` only for a task of the pushing App on this repository, or the token's task.
  - `OpenPr` takes `AgentMeta.session` and `requester` from the claim or `X-Graftyard-Agent-Session`.
  - Task commands go through `IssueCmd::decide_task`. An App needs `issues: write` plus `contents: read`.
- **API (`crates/api/src/tasks.rs`):**
  - The routes under `/api/graftyard/v1/repos/{o}/{r}/tasks…` (never `/api/v3/agents/*`).
  - `Reader::of` (a user's role, or an installation's `metadata` and `contents`).
  - `task_json`, `get`, `list` with `Link`, `log`, `parse_create` (external only; a Graftyard-run create is 422 as while `agents_enabled` is off), `parse_patch`, `render_ok` from `&Acked`, and `refused`.
  - `hosted::What::Task`. `graftyard_api::agents::route` now also recognizes task paths.
- **Edge** (in `workers/edge/src/agents.rs` where possible):
  - `agents::serve` routes task paths to `api::tasks`. It sits beside `agent_policy`, because it needs `api.rs`'s private `read` and `commit_issue`. It is served only once `FormatsEnable` contains 32.12; until then 503.
  - GraphQL `createPullRequest` passes `X-Graftyard-Agent-Session` into `IssueCall.session`, only on an agent `OpenPr` and only with 32.12 enabled.
  - `POST /app/installations/{id}/access_tokens` takes `graftyard_task`. `api::task_claim` reads the task's repository and runs `token_task`, which sets `Grant.task`. A request without the field is unchanged.
  - A push under a claimed token is refused `Unsupported` until 32.12 is enabled.
  - `lib.rs` changed only in that gate.
- **Site:** `Repo::reads()` adds 32.12.

## Formats and tags taken

| What | Taken |
| --- | --- |
| Repository command format | 32.12 (`REPO_FORMAT_TASKS`). 32.10 and 32.11 left to the other agents |
| Authz format | none (0x12.6 not needed) |
| `RepoEvent` tags | 31 `TaskCreated`, 32 `TaskUpdated` |
| `IssueOp` | `CreateTask`, `UpdateTask`, `CancelTask` (serde variants, inside 32.12 only) |
| `IssueOut` | `Task`, `TaskFinal` |
| `IssueErr` | `Task(TaskErr)` |
| Core tables | `agent_task`, `agent_task_open`. Metadata: `agent_task_meta` |
| Wire fields | `IssueCall` `agent_session` (32.12 only). The 32.12 push encoding adds an option of the task claim after the principal |

## Checks

CHECKS

## Assumptions

- **Graftyard-run task creation, `agents_enabled`, and the controller's delivery are AM3-WP3's.** A `POST` without `kind: external` returns 422 with the `agents_enabled`-off message. The gates seed Graftyard-run tasks with a 32.12 `TaskCreated` record.
- **Workspace forks are AM3-WP3's.** `token_task` accepts only the task's own repository, so a token with `graftyard_task` covers exactly one repository.
- **`leases` is `[]` and `/log` is 404** until AM1-WP4 and AM3 push leases and the session log.
- **The cancel answer for a derived final state** is 409 `{state}`, the reply the reference gives for a final task, even though the command recorded the state. The reference names 409 for "a final task" only.
- **An installation token's `metadata: read`** is implied by any installation that reaches the repository. `contents: read` needs both the claims and the row (`Reader::of`).
- **`requester` in an external create** is a login the edge resolves through `Names`. An unknown login is 422 on `requester`.
- **`installed` reuses the token rule.** The installation must reach the repository, and `not_before` must not be later than the task's create time.

## What the hosted half needs

1. A site day with this release (the site reads 32.12), then `FormatsEnable` for 32.12. Until then the task API answers 503, `graftyard_task` answers 503, and a claimed push is refused `Unsupported`.
2. The edge's `receive_pack` still sends no `provenance` (AM1-WP2 left `validate`'s trailer parsing to the git cache). Hosted linking needs `validate` to return `Provenance`, and the edge to `agent::link` it with `task::app_sessions` (read from the fold) plus the token's claim before the commit.
3. A `graftyard-workspace` App registered on the install with a `GraftyardRun` row, and AM3-WP3's controller creating Graftyard-run tasks (`agents_enabled`, the first-party delivery). Until then only external tasks exist on a hosted install.
4. The hosted LF-1 rerun: 10 real repositories on a test install, 100 external tasks from a test `AgentApp`, uninstall through the hosted `DELETE` of the installation, then the reads and the next commands through the edge.
5. `graft-ops` signs `Grant.task` as is (it deserializes `Grant`). The hosted check is that a claimed token verifies on the edge with its `task`, and that a 32.12 push under it reaches `decide`.
6. The fetch half of B0-37 (session refs advertised only to a v2 `ref-prefix`), now that `graftyard-workspace` can write `refs/graftyard/sessions/<id>`.
7. Leases (AM1-WP4): a cancel or finish releasing the task's leases in the same command, `leases` in the single GET, and LF-1's lease half.

## Design sentences these findings contradict or leave open

Neither `design/` nor `decisions/` was edited.

1. `design/AGENT-LAYER.md`, Shape (and `design/DESIGN.md`'s agent block): "`prefix` is `refs/heads/graftyard/<uuid>/`, matched as the `RefPattern` `refs/heads/graftyard/<uuid>/**`, so a ref needs at least one segment after the id." In the `RefPattern` dialect `**` matches zero or more segments, so `…/<uuid>/**` also matches `refs/heads/graftyard/<uuid>` itself. The code stores the design's pattern as `prefix` and matches pushes against `…/<uuid>/*/**`. The gate checks that the bare id is refused.
2. Same block: "`TaskUpdated { … }` // Core for state, Metadata for pr and summary". `TASK_IDLE` is measured from the task's last update, which `decide` must read from the core tier, so every `TaskUpdated` writes Core (an empty `PATCH` too). Metadata is written as well when it sets `pr` or `summary`.
3. Same block: "Core tier: `session`, `kind`, `state`, `prefix`, `requester`, and `app`, about 100 bytes." `decide` and every read need `installation` and the times (created and last update) to derive `cancelled` and `timed_out` without the metadata tier, so those are core too.
4. Same block: "a task whose `installation` the owner `Authz` read no longer finds active reads as Cancelled". "Active" is not defined. The code takes the token rule: the installation row exists for the task's App, reaches the repository, and its `not_before` is not later than the task's creation. Otherwise uninstall then reinstall would revive every old task.
5. Same block: "`pub issue: Option<IssueRef>`" and "`pub pr: Option<PrNumber>`". Neither type exists in the code. An external task names an issue of its own repository, so the code stores `IssueNumber`.
6. Same block: "`pub enum SessionId { Run(Uuid), … }`", whose "string form in URLs, JSON, and `X-Graftyard-Agent-Session` is the UUID". This is correct, but the code's derived serde form put a 128-bit integer in JSON, which the issue-command wire cannot carry (bug 1). The design is right; the code now follows it.
7. `design/AGENT-LAYER.md`, "API reference": "`/repos/{owner}/{repo}/tasks/{id}/cancel: {post: {200: task, 409: {message, state} for a final task}}`". It does not say what a cancel of a task that reads final only by derivation (uninstalled, idle) answers, although "the task's next command records the state". The code records it and answers 409 with the derived state.
8. AM1-WP2's contradiction 5 ("`decide` records `Session` only when the pusher is an App's installation token and the commit names … an `AgentTask` of that App, live or final") now holds for real tasks. It also holds for the token's own claimed task, which the design's sentence does not name but implies through `graftyard-workspace`.
