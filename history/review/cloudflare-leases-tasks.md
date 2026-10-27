# Leases, activity feed, and task API in the first release

The operator decided on 2026-10-26 that leases and a task API ship in the agent layer's first release (`FRAME.md`, "Operator decision, 2026-10-26, agents answered and graftyard", answer 2). The arena's flip condition said "start from C" if leases, a live who-is-working-on-what feed, and a task API were must-haves (`cloudflare/AGENT-LAYER.md`, Synthesis decision). This file is the lead's contract for adding them. A writer applies it to `cloudflare/AGENT-LAYER.md`, `cloudflare/DESIGN.md`, and `cloudflare/cost_agents.py`.

## Decision: graft C's features onto A, not rebase on C

- **Why not rebase.** A won on its git CPU accounting and its smaller surface (`review/cloudflare-agents-judge.md`). Neither depends on whether leases exist. C's leases and task API are features, and they can be added to A. C's ref lanes stay rejected, because the operator did not ask for them and they move under a quarter of A1's commands.
- **Why no coordination Durable Object.** C put leases in a new `RepoCoord` object per repository. A lease is a few hundred bytes of state about one repository, read with that repository's refs. The repository's group already gives it RPO zero, an 11.5 s failover, the vault, the lifeboat, and `RestoredTo`. A new object class would add a failure domain with none of those (principles: subtract-before-you-add, redesign-from-first-principles).
- **Precedent in the base.** Actions job leases already live in the Actions group, and "job leases extend by the outage length on recovery" (`cloudflare/DESIGN.md`, Failure modes, `actions state`). Agent leases follow the same pattern.

## LT-ACT items

### LT-ACT-1. Leases are chain state on the repository's group

Add to the agent block of Shape, identical in `cloudflare/AGENT-LAYER.md` and `cloudflare/DESIGN.md`:

```rust
/// A claim by one holder on paths or refs of one repository. Fold rows on the repository's group, written by
/// `LeaseAcquire`, `LeaseRenew`, and `LeaseRelease` commands. A lease is live while the leader's entry time `at` is before
/// `expires`. Expiry is evaluated when read and in `decide`, so no command is needed to expire a lease. Expired rows are
/// dropped in batches by the next compaction.
pub struct Lease { pub id: LeaseId /* ULID */, pub holder: LeaseHolder, pub scope: LeaseScope, pub mode: LeaseMode,
                   pub intent: Box<str> /* <= 280 bytes, untrusted text, rendered through the trust view */,
                   pub granted: Timestamp, pub expires: Timestamp }
pub struct LeaseHolder { pub actor: ActorId, pub app: Option<AppId>, pub task: Option<TaskId> }
pub struct LeaseScope { pub paths: SmallVec<[PathGlob; 8]> /* <= 32 globs */, pub refs: SmallVec<[RefPattern; 2]> }
/// Shared conflicts only with another holder's Exclusive lease. Exclusive conflicts with any other holder's live
/// lease whose scope overlaps.
pub enum LeaseMode { Shared, Exclusive }
pub const LEASE_TTL: Duration = Duration::from_secs(600);        // default; a holder may ask for 60 s to 3,600 s
pub const LEASES_PER_HOLDER: u16 = 8;                           // live leases per holder per repository
pub const LEASES_PER_REPO: u16 = 4_000;                         // live leases per repository
/// Repository policy. Advisory never refuses a push. AgentsExclusive refuses an `ActorKind::Agent` push whose changed
/// paths, from the same tree diff that yields `ProtectedHits`, overlap another holder's live Exclusive lease. Humans,
/// bots, and the merge queue are never refused by a lease.
pub enum LeaseEnforce { Advisory, AgentsExclusive }
```

- `AgentPolicy` gains `leases: LeaseEnforce`, default `Advisory`.
- Acquire returns the lease plus every other live lease that overlaps it. A conflicting acquire returns 409 with those leases and creates nothing.
- Renew moves `expires` forward and changes nothing else. Release deletes the row. A holder may release or renew only its own leases. A repository admin may release any lease.
- **Who may acquire.** Any token with push access to the repository. An `ActorKind::Agent` token may name refs only inside its task's prefix, or inside its App's `confine` prefix.
- **Time.** Expiry compares against the entry's `at`, which the leader stamps. Write the skew bound that follows from the base's `LEASE`, `DRIFT`, and `ELECTION_AFTER`. The writer must state whether a leader change can make a lease look live up to about `DRIFT` longer, and accept that as advisory slack.
- **Outage rule.** As with Actions job leases, a lease whose group could not commit during an outage is extended by the outage length on recovery. Leases never expire because the host was down.
- **Admission.** Lease commands from agent tokens take `AdmitClass::Agent`.
- **Unify with `PathSet.planned`.** The controller acquires a Shared lease on its planned paths at task start instead of writing a `tributary-plan` block. The plan-block parser stays as the GHES-compatible way to declare intent. A `tributary-plan` block in a draft PR body becomes a Shared lease held by the PR's author or App. It lives while the draft is open and at most 24 h after its last push. `PathSet.planned` is then read from leases, so one concept carries planned paths.

### LT-ACT-2. The read side is a long-polled activity feed

- `GET /api/tributary/v1/repos/{owner}/{repo}/activity?cursor=<c>&wait=<s>`, where `wait` is at most 25. It returns the events after the cursor and the next cursor. The events are:
  - lease acquired, released, and expired, plus a renewal that changes scope or mode;
  - task created and task state changed;
  - agent pushes (ref, head, path summary);
  - overlap and conflict verdict changes;
  - agent PRs opened, readied, closed, and merged.
- **Source.** The feed is the group's log read through a fold, a ring of the last 10,000 events or 7 days, whichever is smaller. The cursor is (group, index). A cursor older than the ring returns 410 with the oldest cursor, and the client reloads leases and tasks through the list endpoints.
- **Serving.** Any member or learner serves the feed after read-index (`fresh` semantics for followers). A request with `wait` is held by the member until its applied index passes the cursor or the wait ends. Members are always active, so holding requests adds no duration charge [inference]. The writer must cite the Durable Objects pricing statement for this, or mark it [unverified].
- **Visibility.** Read access to the repository. Intent text and PR titles go through the trust view.
- **MCP.** Add the tools `coordination.lease`, `coordination.renew`, `coordination.release`, and `coordination.activity`. `coordination.overlaps` also returns leases.
- **Subtractions.** The feed has no WebSocket and adds no new webhook event in the first release. GHES clients would not subscribe to an unknown event, and long-poll serves the operator's "who is working on what". Record both under Alternatives.

### LT-ACT-3. The task API is the `AgentTask` row made public

- **Path.** Under `/api/tributary/v1/repos/{owner}/{repo}/tasks`, never under `/api/v3/agents/*` (R24).
- **States.** The states copy GitHub's agent tasks API: `queued`, `in_progress`, `completed`, `failed`, `idle`, `waiting_for_user`, `timed_out`, and `cancelled` (`research/agents.md` 1.5). Map the controller's `TaskState` onto them in one table.
- **Endpoints.**
  - `POST` creates a task. A writer creates a Tributary-run task from `issue` or from `prompt`. A prompt is at most 64 KiB, is stored on the task row, and is trusted as writer text.
  - An `AgentApp` creates an external task with `kind: external` to report its own work. It gets no prefix tokens and stays under its `confine`.
  - `GET` lists tasks, filtered by `state`, `app`, `requester`, and `issue`. `GET /{id}` returns one task.
  - `POST /{id}/cancel` is allowed for the requester, a repository admin, or the task's App.
  - `PATCH /{id}` lets the task's own App set `state`, `pr`, and `summary`.
  - `GET /{id}/log` returns the versioned `SessionLog` JSON that session pages already serve.
- **One write path.** The controller creates its tasks through this same API with its App token. The controller-only `POST /_tributary/repos/{owner}/{repo}/agent-tasks` is removed (principle: subtract-before-you-add). `AgentTask` stays the fold row. It gains the `kind`, `state`, `prompt`, `pr`, and `summary` fields, and the `tributary_task` token claim still binds to it.
- **Leases.** A task's leases are listed on the task, and cancelling or finishing a task releases them in the same command.
- **Admission.** Task commands from human tokens are `Human`, and from agent tokens are `Agent`.
- **Permissions.** For fine-grained PATs and Apps, map create and cancel to `issues: write` plus `contents: read`, and reads to `metadata: read`. The writer checks this against how GHES maps other writes and states it in the GHES compatibility table as additive.
- **Assignment stays.** Assigning the bot to an issue creates the same task. The task API adds a prompt-only path and a list, and does not replace assignment.

### LT-ACT-4. Journeys

- **Rewrite A5.**
  - X's controller takes a Shared lease on `pkg/auth/` at task start.
  - Y's agent, a third-party App, calls `coordination.activity` or the feed, sees X's lease, and takes a Shared lease on its own paths. With `AgentsExclusive` set, a push by Y into X's Exclusive scope is refused with a text that names the lease and its holder.
  - Replace step 5's sentence "No lease stops duplicate work" with what leases now do and do not stop. Duplicates are visible before work starts, and they are refused only under `AgentsExclusive`.
- **Add to A3 or as a new journey.** A platform engineer creates a prompt-only task through `POST .../tasks`, polls `activity`, and cancels it. The state sequence uses GitHub's names.
- **A1 numbers.** Add lease renewals at `LEASE_TTL / 3` for 500 agents, about 2.5 commands/s [inference], and feed long-polls at one request per agent per 25 s, about 20 requests/s, spread over members and learners. Recompute 22.8 commands/s, the agent-class share, and the leader request count with `cost_agents.py --a1`, and update every place that cites them in both documents.

### LT-ACT-5. Scale, failure modes, GHES table, deployment, cost, build order

- **Scale.** Add rows for live leases per repository against `LEASES_PER_REPO` and member bytes, for feed requests per group against the 1,000 req/s per-object soft limit, and for task rows.
- **Failure modes.** Add leases, the feed, and the task API to the `edge and fold additions` component. Write each of its eight rows so it states what happens to leases:
  - **region:** RPO zero for leases, and the outage rule applies;
  - **bad-config:** a wrong `LeaseEnforce` refuses only agents;
  - **noisy-tenant:** per-holder and per-repository caps.
- **GHES compatibility table.** Add rows for the lease API, the activity feed, the task API, and plan-block leases, all additive under `/api/tributary/v1`. Remove the `/_tributary/.../agent-tasks` row.
- **Deployment modes.** The same in every mode. On workerd evaluation, leases and the feed run on the one site.
- **Cost.** Add lease commands, feed requests, and task rows to `cloudflare/cost_agents.py` as [guess] constants with names (`LEASE_RENEW_S`, `FEED_WAIT_S`). Rerun `python3 cloudflare/cost_agents.py` and `--a1`, and paste the output tables. Update `cloudflare/DESIGN.md` Cost and Summary if the totals move.
- **Build order.**
  - AM1 builds leases, the feed, external tasks, and the task read endpoints.
  - AM3 builds Tributary-run task creation through the API.
  - Add a Milestone 0 row in the layer:
    - Under leader kills during the A1 replay, no two holders hold overlapping Exclusive leases beyond the stated skew bound.
    - Expiry survives a failover and an outage as the outage rule says.
    - 500 long-pollers see each event within 2 s at p99.
    - `AgentsExclusive` refuses 100 of 100 conflicting agent pushes and 0 of 100 human pushes.

### LT-ACT-6. Rewrite the text that rejects leases and a task API

- **Tradeoffs.** Replace "We accept no path leases" with "We accept advisory leases by default. `AgentsExclusive` refuses agents only, so a human can always push, and two agents without leases can still duplicate work."
- **Alternatives.**
  - Keep C's coordination Durable Object rejected, with the reason above.
  - Replace "A Tributary task REST API" with the adopted design, and say what was subtracted: the controller-only path, WebSockets, and a new webhook event.
- **Synthesis decision.**
  - Record that the operator's 2026-10-26 answer met the flip condition's features.
  - Record that the lead grafted them onto A instead of rebasing, for the reasons in this file.
  - Keep the arena's scores as they were.
- **Summary.** Add one sentence per document.
- **`FRAME.md`** already records the decision. Do not edit it.

### LT-ACT-7. Stale statements found by the lead's read

- `cloudflare/AGENT-LAYER.md` Summary (line 11) and A1 step 3 (line 242) say a member holds 3.0 GB after 30 days. The base's Scale row says about 7.0 GB with the 4 GB metadata tier (`cloudflare/DESIGN.md`, Scale, member bytes). Make both match the base and `cost_agents.py --a1`.
- The AG-ACT-10 entry in Review resolutions says 3.0 GB. Change it to say the figure was later reseeded with the 4 GB tier, and name trail review 7 (`review/trail-review-7.md`).

## Acceptance

- `python3 scripts/check_cloudflare.py --final cloudflare/DESIGN.md` and `python3 scripts/check_cloudflare.py --layer cloudflare/AGENT-LAYER.md` exit 0. The layer cap is 700 lines. If the writer cannot fit within it after tightening, it stops and reports the line count, and does not edit the checker.
- The agent block in Shape is byte-identical in both documents.
- Every number in the documents equals the output of `python3 cloudflare/cost_agents.py`, `python3 cloudflare/cost_agents.py --a1`, and `python3 cloudflare/cost.py`.
- No text says that leases or a task API are absent or rejected, except C's coordination object and its WebSocket.
- A "Leases and tasks" resolution entry lists LT-ACT-1 to 7 and the sections each touched.
