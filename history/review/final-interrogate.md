# Final pass: triage and act-on items

Three fresh read-only seats reviewed the design on 2026-10-26 after the leases-and-tasks change:

- `review/final-security.md`, from a security-reviewer, has findings F1 to F15.
- `review/final-correctness.md`, from a reviewer, has findings FP-1 to FP-15.
- `review/final-consistency.md`, from a reviewer, has findings G-1 to G-13.

All three seats ran on anthropic/claude-opus-5-5. That is the same model family that wrote the arena candidates and the earlier fixes. None of the three instances wrote any of the text it reviewed.

The lead read all 43 findings in full. Every finding is acted on. None is dismissed, because each cites the text it breaks and gives a fix an implementer would otherwise have to invent.

Where findings overlap, they are merged into one item below. Where the lead's fix differs from a reviewer's, the item says so.

**Line caps.** Caps rise to 1,400 lines for the base and 850 for the layer. Both `scripts/check_cloudflare.py` and `scripts/graftyard_templates/check_design.py` are changed. The documents are now the implementation specification for other agents, so a missing type costs more than length.

## Slice B, the base: `cloudflare/DESIGN.md`, `design/SHARED.md`, `cloudflare/cost.py`

**FX-1. Release entries cannot be replayed or rolled back below an enabled format** (F1, FP-6, FP-14).

- **Binding of signed entries.**
  - Every signed `Release` entry signs over `(install_id, group, prev_digest)`.
  - `Keys` carries `generation`, and the group accepts only latest + 1.
  - Intents carry `id`, `issued_at`, and `not_after`.
  - An `undo` is valid only while its parent intent is the current one.
  - `Halt` and `Abort` name their intent.
- **Rollback barrier.** The `Release` group's `decide` refuses `SiteDay`, `EdgePointer`, `OpsPointer`, and `Abort` when the target release does not read every enabled format, and pages. `Abort` means "the newest uploaded release that reads every enabled format". Wire in SHARED's swap-back rule and `rollbackBarrier`.
- **Restore entries.**
  - Add `ReleaseEntry::RepoRestore { repos, refs, at, reason }` and `RepoReset { repo, at, reason }` to the customer-key list. They are not an `Intent`, so the vendor cannot sign them.
  - `trib-ops` commits `RestoredTo` or `ResetTo` as a command that depends on `ChainId::Release`. Its request id is the `Release` index, which makes it idempotent. `to` is the last index with `at <= t`.
  - In managed, a restore is a support action under the cell's customer key.
- **Gates.** Milestone 5 adds two rows:
  - Replaying every historical `Release` entry after `Keys { vendor: None }` is refused 100 of 100.
  - After `FormatsEnable`, undo-to-N and `Abort` are refused.

**FX-2. The commit call is split, so admission queueing never looks like a dead leader** (FP-2, FP-13).

- **The split.** The leader answers within `CALL_DEADLINE` with `Admitted { ticket }` or `NotLeader`. The edge waits for the outcome on a second call with a deadline of `AGENT_WAIT` plus one round.
- **What deadlines govern.** Deadlines, hedges, retries, `SiteHealth`, and `suspect` apply only to receipts and to member-to-member calls.
- **Two-colo threshold.** It gates only withdrawing promises from a leader that is still renewing. A follower whose promise has lapsed campaigns on one report, or on its own failed read-index.
- **Revocation guarantee.** Restate it as "binds every command decided after it". The leader re-reads the credential's shard for any command that waited longer than `READ_CACHE`. Each distinct `Authz` dependency is read once per entry, after the command set is frozen, and that hop is added to the entries-per-second math.
- **Gates.**
  - The Milestone 0 burst and 5,000-agent rows pass only with zero leader changes and zero `suspect` reports.
  - The RTO row runs once from a single colo and once with blackholed calls.
  - Milestone 3 adds this row: revoke while 100 pushes are queued, and none is acknowledged after the revocation's index.

**FX-3. `verify` replays the entry instead of checking against the fold at k** (FP-1).

- `verify` replays the entry's events in order on a scratch transaction over the fold at k, and checks each `expect` against that running state.
- If both followers on the leader's release refuse, the leader treats it as its own defect: it truncates the uncommitted tail and alarms.
- Add a Milestone 2 row: one entry holding a 5-PR queue group plus a `PrSync` batch commits on all three sites.

**FX-4. Leader-lease safety** (FP-3, FP-4).

- **Transfers.**
  - AppendEntries carries `transfer: Option<(to, transfer_id)>`. Acks to rounds that carry a transfer count toward no lease.
  - A voter honors a transfer vote only for the `transfer_id` it last accepted.
  - Abandoning a transfer sends `transfer: None` and needs one full round before `fresh`. `transfer_id` is persisted in `MemberState`.
- **No-op first.** `fresh` and read-index answer only after the current term's `Noop` is committed and applied.
- **Quarantine.** It covers only members with two or more boots in the window, or whose last leadership ended by `STEP_DOWN`. A voter waives it after no leader for 2 x `ELECTION_AFTER`. A promise withdrawn on `suspect` is restored after 2 x the `suspect` window, or on a higher term.
- **Gate.** Add a Milestone 0 row: every member on all three sites resets within 60 s under 100 writes/s with two-colo `suspect` reports, and every group commits within 12 s.

**FX-5. Spare and residency sites get releases** (FP-5).

- Every release adds one site day per spare and residency site, gated by a canary group on that site that replays a corpus of current formats.
- Each spare's images are kept until that spare's own site day.
- `FormatsEnable` counts spares through the canary.
- `ReplaceMember` refuses a spare that cannot read every enabled format.
- The Milestone 6 drill runs after two release rolls and one `FormatsEnable`.

**FX-6. The restore window survives GC, and a whole-install restore re-applies revocations** (FP-7, F11, FP-15).

- **GC roots.** A `ref_log` fold table keeps every ref value from the last 35 days. GC roots are the fold's refs plus `ref_log`. Cruft expires 14 days after an object leaves that set. `RestoredTo` reads the same table.
- **Snapshots.** Keep at least one snapshot per tier at or below every index inside the window.
- **Vault.** The vault deletes a pack no sooner than 35 days after the install's GC deletes it.
- **Restore re-application.** A whole-install restore re-applies revocation-class entries newer than the cutoff from the vault log. Those are token, `jti`, and SSH key revocations, uninstalls, member removals, `Keys`, and `LifeboatRemove`. The new genesis `Keys` replaces the restored `Release` group's keys. The installation-token signing key is rotated and old `kid`s are dropped.
- **Archives.** Archives and snapshots are keyed by `(chain, from, to, digest)`. An `Archived { key, from, to, digest }` record commits before followers truncate, and they truncate only through a committed range they can HEAD in two buckets. Snapshots go to two buckets.
- **Gates.**
  - Milestone 0: delete a branch and force-push on day 1, restore to day 0 on day 30, and the objects come back.
  - Milestone 7: a PAT revoked after the cutoff fails after restore.

**FX-7. Ids, `Tokens` and `Names` entries, suspension, stars, and notifications have homes** (FP-9, FP-10, FP-11; also G-3's `Tokens`, `Names`, and `Actions` parts).

- **Ids.**
  - `DbId` is cell (8 bits) | block (29) | offset (16).
  - A per-cell `Ids` group leases blocks to group leaders. Each group commits `IdBlock { base, len }` before use.
  - Issue numbers stay per-repository counters.
  - The managed front routes `/repositories/{id}`, `/user/{id}`, node ids, and installation ids by the cell bits.
- **Names and Tokens events.**
  - Define `NamesEvent`: `LoginClaimed`, `Account { suspended, sessions_not_before }`, `FormerName`, `Retired`, `Shortcode`, `EmailClaimed`, `AppRegistered`, and `InstallationRegistered { id, app, owner }`.
  - Define `TokensEvent`: `Stored`, `Revoked`, `SshKey`, `SshKeyRemoved`, `JtiRevoked`, `LifeboatRead`, and `VerifyingKey`.
  - Define `ActionsEvent` from SHARED's `JobEvent`.
  - Define `name_shard` and `account_shard`.
- **Suspension and credentials.** Write-class requests read the actor's `Account` row through `fresh`. Credential creation commits first on its `Tokens` shard, which is the uniqueness point for SSH fingerprints, and a finisher then completes the `Identity` side.
- **Stars, watches, follows, and notifications.**
  - Stars, watches, subscriptions, follows, and notification thread state become `IdentityEvent`s on the actor's group.
  - A consumer writes coalesced counts in the bot class to each repository's metadata tier.
  - Notifications, feeds, `/issues`, and `/user/repos` come from a derived per-account `Inbox` Durable Object, rebuilt from cursors.
  - State the lag as a known deviation in SHARED and price it in `cloudflare/cost.py`. If the total moves, update every base total in every document. The agent writer is told separately.

**FX-8. Pointers are read without putting traffic on the `Release` group** (FP-12).

- Each isolate caches pointers with a compiled TTL, refreshes from any member's applied `Release` state, never moves backward by index, fails static, and never blocks a request.
- Every pinned call is a `fetch()`-based binding, never RPC. Quote the version-overrides page.
- The edge reports the version it ran, and `trib-front` alarms on a mismatch.
- An `EdgeRoll` that would drop a version an undo still names is refused.
- The managed front reads each cell's pointers over signed HTTPS.

**FX-9. Lease and Actions-lease extension uses a recorded stall** (FP-8, G-5, and G-10's `Expired` part). The base owns this mechanism, and the layer cites it.

- **Extension.** Extend only by a recorded stall window. It starts at the earliest durable marker: a follower's persisted `suspect` time, a candidate's campaign start, or the leader's persisted `stalled_since`. The next committed entry carries `ExtendLeases { since, by }`. Idle time never counts.
- **Reads and expiry.** Reads compare against the leader's leased clock. Expiry needs no command. The feed emits `Expired` at the first entry with `at >= expires`, and clients time expiry themselves from `expires`.
- **Actions.** Job leases use the same rule, and a dead runner's job fails at its lease end.
- **Gates.** Add these rows:
  - An idle 60 s lease, then an overlapping Exclusive acquire 10 minutes later, succeeds.
  - A dead runner's job fails at 10 minutes.

**FX-10. The drain-only key cannot drain the install** (F14).

- The group refuses a drain-key `SiteMode Drain` while another site is drained.
- Pre-vote lets a drained member campaign when no undrained voter is available.
- Add a Milestone 0 row that submits two drains directly.

**FX-11. Hot pool in `max_instances`, the child-group constant, and SHARED's format rule** (G-11, G-2, G-12).

- **Hot pool.** Add the hot pool to `shards()` and `max_instances` in `cloudflare/cost.py`, to the Cost table, and to the Scale row, with headroom about 3.9x. Say which instance size the pool uses at 50 and 300 developers.
- **Child groups.** Define `CHILD_GROUP_AT` in base Shape as member SQLite bytes (8 GiB). It caps the tier at 10 GB minus `LOG_MAX_BYTES`, core, and margin. The agent writer updates `cost_agents.py` and the month counts.
- **Format rule.** SHARED states that an entry in a format not yet enabled is refused, and that the tier tag only lets the core tier skip decoding a metadata body.

**FX-12. Undefined base types and stale reuse wording** (G-13's `Health`, G-3's base parts, G-12).

- Define `Health`.
- Retitle "Reused from `DESIGN.md` without change" to "Reused with the additions below".
- Add `provenance` and `protected` to SHARED's `Push`, or point SHARED's `Push` at the base's. Fix the Usage call site.
- `Consumer` gains the insights consumer.
- `ReleaseEntry` and `Intent` gain `McpPointer`, `ControllerPointer`, `McpRoll`, and `ControllerRoll`, under FX-1's binding rules.

## Slice A, the agent layer: `cloudflare/AGENT-LAYER.md`, the agent block in `cloudflare/DESIGN.md` Shape (byte-identical in both), journey 9 and the agent Scale and Milestone 0 rows of the base, `cloudflare/cost_agents.py`

**FX-20. `AgentTask` has an owner, tiers, and transition rules** (F2, G-7, G-1).

- **Fields.** Add `app: AppId`, `installation`, and `creator: Actor`. `requester` becomes `Option<UserId>`.
- **Minting.** Minting requires `task.app == jwt.app`, and requires the task's repository to be among the token's repositories.
- **Prefix.** `prefix` is derived from `session`, never taken from the body.
- **States.** Terminal states are final. Only the cancel endpoint or the host timer sets `cancelled` or `timed_out`.
- **Claims.** `InstallationClaims` gains `task`. Edit SHARED's struct.
- **Tiers.**
  - `AgentTask`'s core part goes in the core tier: `session`, `kind`, `state`, `prefix`, `requester`, and `app`.
  - Its `prompt`, `summary`, and `pr` go in the metadata tier.
  - `Lease`, `AgentGate`, `AgentPolicy`, and the `AgentApp` flag go in the core tier.
  - `PathSet`, provenance, and the feed ring go in the metadata tier.
  - Add them to `tier_of` and to SHARED's core-tier list.
- **Controller row.** Restate the controller's bad-deploy row now that the controller can create tasks.

**FX-21. Who creates tasks, and how the controller hears** (F8, G-6).

- **Org flag.** An org setting `agents_enabled`, off by default, records operator decision 1: offered and off until an org enables it. `POST` returns 422 when it is off. Add a step to the "Enable agents" checklist and a sentence for managed mode.
- **Who may create.**
  - A Graftyard-run task needs a human user credential with `contents: write`. It cannot be an installation token, an `AgentApp`'s user token, or a demoted or agent-session actor.
  - The controller App may create one only from an issue whose latest `Assigned` entry names its bot.
  - Other Apps create External tasks only.
- **Snapshot and caps.** Snapshot the issue at the create index. Record the creator. Cap open tasks per requester and per repository, with named constants marked [guess].
- **Notification.** Task create and cancel reach the controller as a first-party-only delivery on the event pipeline to its webhook hostname. GHES clients never see it. State this as the one exception to "no new webhook event".

**FX-22. Lease holders, Exclusive limits, plan-block rules, and revocation** (F5, F6, F13, G-4).

- **Holder.** The holder is `(actor, app, session)`. App tokens must name a session on acquire: the task claim, or for third-party Apps an External task the same App created. Define `ActorId` from SHARED's `Actor`. Add a per-App cap below `LEASES_PER_REPO`.
- **Exclusive.**
  - An Exclusive lease needs a live task of the holder, at most one per task.
  - Every Exclusive glob has a literal first path segment, so `**` and a top-level `*` are refused.
  - Total lifetime is capped at 4 h, after which the lease drops to Shared.
- **Conflicts.** This is a lead change from LT-ACT-1. Exclusive acquire conflicts only with other Exclusive leases. Shared leases are returned as overlaps and never block. Under `AgentsExclusive`, only task-held Exclusive leases refuse pushes.
- **Plan-block leases.** They exist only for authors or Apps with push access, are attributed to the editor, count toward the per-holder cap, and have their own sub-cap per repository.
- **Revocation.** The lease check drops leases whose holder fails the owner `Authz` read that `decide` already does. Uninstalling or suspending an App releases its leases and cancels its tasks. External tasks go to `timed_out` after a named idle limit.
- **Rescope.** Drop `LeaseChange::Rescoped`, because renew changes nothing but `expires` (G-10).

**FX-23. `decide` gets changed paths, fork pushes see the base, and one rule defines an agent** (F3, F4, F7, F15).

- **Changed paths.**
  - `validate` returns `ChangedPaths { refname, paths <= PATHS_MAX, truncated }` for each agent or agent-prefix ref update, and `Push` carries it. `PathSet.paths` comes from it.
  - `decide` matches it against live Exclusive leases. When `truncated` is set, a directory counts as overlapping.
  - Exclusive globs are indexed by literal prefix.
  - REST ref writes by an `Agent`, or under an agent prefix, run `validate` on an empty staged pack.
- **Fork pushes.**
  - With a `task` claim, `Push::deps()` includes the base repository's chain, read `fresh`, and `decide` checks task liveness, `AgentPolicy`, and leases there.
  - The merge base is taken against the base's default-branch tip.
  - Add a fork-mode ceiling row: a push after cancel is refused.
- **Who is an agent.**
  - One rule in both blocks: an App is `ActorKind::Agent` when it is one of Graftyard's Apps, or when an admin flagged it and set `confine`. A flagged App without `confine` is `Bot`.
  - Rewrite A5 so Y is a confined App.
  - State that `AgentsExclusive` binds neither MCP nor user tokens.
  - Run the Milestone 0 row once per actor kind.
- **Trailers.** Only `TributarySession`, `ClaudeSession`, `AmpThreadId`, and `AssistedBy` classify a push as agent. `CoAuthoredBy` never does, and a trailer never demotes a user-token push on its own.

**FX-24. The human gate cannot be satisfied by an agent** (F9).

- **Gate source.** `AgentGate.satisfied_by` comes only from an approval made with a credential that is neither an `AgentApp`'s nor MCP-issued. MCP tokens carry the grant id as a claim, and the host enforces `WriteScope`.
- **Code Mode.** `codemode.request()` runs through the same tool deny-list.
- **Vendor tasks.** A vendor carries `requester` through its External task.

**FX-25. Read permissions and the feed's load** (F10, F12, F6's feed part).

- **Permissions.** `GET /tasks/{id}/log`, prompts, session pages, and the feed's path fields need `contents: read`. Every text value in the feed goes through the trust view.
- **Polling limits.** Polling must be authenticated, with at most 2 concurrent polls per token and a per-IP limit.
- **Unfiltered polls.** A poll without `paths` batches for `FEED_BATCH_ALL`, 10 s [guess].
- **Who serves.** Feed polls are served by followers and learners, never the leader, and they count toward `LEARNER_AT`.
- **Recompute.** Recompute A1's feed request rate with 500 pollers split as the script assumes, then propagate every figure.

**FX-26. Stale statements and undefined layer types** (G-3's layer parts, G-9, G-13, G-8, G-2's layer parts).

- **Event types.** Add one "Agent-layer entries" block to the base Shape agent block. It defines:
  - `RepoEvent` variants: `Assigned`, `IssueEdited`, task create and transition, `AgentGate`, `AgentPolicy`, `PathSet`, `LeaseAcquire`, `LeaseRenew`, `LeaseRelease`, and `ExtendLeases`, with their fields;
  - `AuthzEvent::AgentApp` and `IdentityEvent::McpGrant`.
  Each variant has its `tier_of`.
- **Stale statements.**
  - AGENT-LAYER "Open questions" on child groups becomes: "The operator kept the default (`FRAME.md`, answer 4). Child groups stay at Milestone 7."
  - Restate the "No new component sits on a base commit or read path" paragraph to cover lease and task commands, the `AgentsExclusive` check, and feed polls.
  - Mark AG-ACT-17's `tributary-plan` sentence as superseded by LT-ACT-1.
- **Undefined types.** Define `Step`, `Harness`, `PrAction`, `PathGlob`, and `RefPattern`. Globs use gitignore-style syntax, anchored unless they start with `**/`. State the overlap rule.
- **Child-group months.** Update `cost_agents.py` and the month counts to `CHILD_GROUP_AT` as member bytes (FX-11).
- **`alternatives/AWS.md`.** Paste the base-plus-agents row from the regenerated `research/costs.md`. Note that the AWS figures include no agent layer, and correct the sentence comparing orders at 2,000 developers.

## Acceptance for both slices

- `python3 scripts/check_cloudflare.py --final cloudflare/DESIGN.md` and `--layer cloudflare/AGENT-LAYER.md` exit 0 under the new caps.
- The agent block is byte-identical in both documents.
- `python3 cloudflare/cost.py`, `python3 cloudflare/cost_agents.py`, and `python3 cloudflare/cost_agents.py --a1` match every figure the documents cite. `python3 scripts/costs_report.py` regenerates `research/costs.md`.
- Each item's Milestone row exists and names a pass and a fail.
- A "Final pass" resolution entry, one line per FX item, sits in each document's Review resolutions and lists the sections each item touched.
