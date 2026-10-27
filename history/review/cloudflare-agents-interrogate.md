# Interrogate verdict on the Cloudflare agent layer

## Intent

> `cloudflare/AGENT-LAYER.md` makes AI coding agents first-class users of Tributary on Cloudflare without changing any GHES 3.22 response for clients that do not opt in. Third-party agents are GitHub Apps. Tributary-run agents run in Sandboxes that hold no token, confined by rules in `decide`. A stateless remote MCP server, chain-resident provenance, overlap and conflict signals, and five base changes for agent load complete it. Artifacts plays no role.

## Reviewers

None of the three wrote a candidate, the synthesis, or the integration.

- `security-reviewer` attacked it as an attacker would (`review/cloudflare-agents-security.md`). 10 findings: 1 blocker, 9 major. Verdict: holds with changes. The session could not write files, so the lead saved its output unchanged.
- `reviewer` attacked the pick and the base changes and walked the installs (`review/cloudflare-agents-foundation.md`). 10 findings: 1 blocker, 6 major, 3 minor. Verdict: holds with changes.
- A `task` agent walked it as a coding agent, a maintainer, and a platform engineer (`review/cloudflare-agents-userwalk.md`). 10 findings: 1 blocker, 8 major, 1 minor. Verdict: holds with changes, and the layer is agent-safe more than agent-friendly.

No reviewer recommended changing the pick. All three agreed that Artifacts should play no role. The foundation review found two of the stated reasons wrong for this design, and the user walk found that even the best-of-N role fits in a workspace fork.

## Act on

Each item is resolved in `cloudflare/AGENT-LAYER.md` or `cloudflare/DESIGN.md` under "Review resolutions". The text here is the contract the revision follows.

**AG-ACT-1. Sandbox egress is a default-deny router.** (Security 1, blocker.) Host allowlisting lets an injected agent push the private repository to an attacker's repository on the same host, write a gist, or make the model API connect to an attacker's MCP server.
- Tributary host: allow only smart HTTP on the task's base and fork repositories and REST under `/api/v3/repos/{base or fork}/`. Refuse GraphQL, gists, registries, uploads, and every other owner path. Always overwrite `Authorization`, `Cookie`, and URL userinfo with the handler's token.
- Model host: allow only the inference path. Parse the body and refuse `mcp_servers`, server-side tool types, and file and batch endpoints. Running the model loop in the controller is the stronger option and is stated as such.
- Registries: allow only registries that do not fetch upstream on demand, or a customer mirror with that disabled. Refuse unknown Go module paths.
- Milestone 0 adds containment rows for a push to another repository on the host with a sandbox-supplied token, gist creation, a model request with `mcp_servers`, and an unknown Go module fetch, each 0 of 100.
- A4's worst-case sentence is rewritten, and "there is no gist to write to" is removed.

**AG-ACT-2. The agent gate is enforced in `Merge::decide` and bound to the head.** (Security 2, user walk 6.) `AgentGate { pr, head, satisfied_by }`. `Merge` of a PR with `AgentMeta` is refused unless `head` equals the current head and `satisfied_by` is a writer who is not the agent App, not the requester, and did not push to that head. The commit status is rendered from the fold for display only. `ActorKind::Agent` refusals add creating statuses or check runs whose context starts with `tributary/`, and the handler refuses status and check-run writes from the sandbox. The agent ruleset template includes the 3.22 rule requiring approval from someone other than the last pusher. Milestone 0 rows: approve, then agent push, then merge, refused 100 of 100, and the agent token posting `tributary/agent-gate`, refused 100 of 100.

**AG-ACT-3. `PROTECTED_PATHS` has a defined input and covers what CI and harnesses execute.** (Security 3, foundation 5, user walk 6.)
- `validate` computes, per agent ref update, the paths changed by the new tip's tree against its merge base with the default branch and against the old tip. `Push` carries the hits beside `provenance`, and `decide` refuses on any hit. This also catches an existing outsider commit pushed under an agent ref.
- The list adds `**/CLAUDE.md`, `**/GEMINI.md`, `.claude/**`, `.cursor/**`, `.cursorrules`, `.codex/**`, `.mcp.json`, `.vscode/**`, `.github/actions/**`, `.gitmodules`, CODEOWNERS, and the paths the install's harness templates read. Repository policy can add build entry points.
- In in-repo mode, workflows for agent pushes run with fork-PR semantics (read-only token, no secrets) even after approval. Secrets are released only when `AgentGate` is satisfied for that exact head.
- Milestone 0 row: pushing an existing outsider commit that changes a workflow under an agent ref is refused 100 of 100.

**AG-ACT-4. Agent authority is per task and covers REST side effects.** (Security 4, foundation 5.)
- Before minting, the controller commits `AgentTask { session, prefix, issue }` to the base group as its App. Installation tokens for Tributary-run agents carry a `task` claim, set through an additive `tributary_task` field on the token request that is checked against a live `AgentTask`. `decide` refuses an `Agent` ref update outside that task's prefix and refuses an `Agent` push with no live task.
- `requester` comes from the actor of the issue's `Assigned` entry that names the bot, never from the controller.
- `ActorKind::Agent` refusals add repository dispatch, workflow dispatch and re-run, label writes outside a per-repository allowlist, branch rename, release and tag creation, and webhook and deploy-key edits.
- The controller's failure row states its real blast radius, which is every installed repository's read access and every agent prefix. Managed uses per-tenant App keys.
- The ceiling row reruns with a raw token, bypassing the handler, so it tests `decide`.

**AG-ACT-5. Provenance is authenticated, and session logs are host-owned.** (Security 5, foundation 1.)
- `refs/tributary/` is host-owned. `decide` refuses every update there except appends by the controller App, and deletion happens only through a retention command.
- `Source::Session` is recorded only when the commit's pusher has a matching `AgentTask` or `AgentMeta`. Every other trailer is shown as claimed, unlinked, with https scheme and host validation.
- Session pages resolve through the fold under the repository's own path. `X-Tributary-Agent-Session` is accepted only for sessions the same App opened.
- Session logs are always `PackRef::Stored`. The controller runs secret scanning and redacts large tool outputs before pushing. Retention defaults to the org's audit-log retention, and the text states that a purge completes only after the vault lock expires.

**AG-ACT-6. Injection channels beyond the issue body are covered.** (Security 6.) The issue is snapshotted at the `Assigned` entry, and the task pauses if a non-writer edits the title or body afterward. File paths and trailer values in check-run text are escaped, length-capped, and labeled untrusted. Bots and every `AgentApp` leave the trusted-author set. `TrustView` applies to every text field MCP returns. Write-capable MCP tokens cannot span repositories of different visibility or owner. Webhook payloads are triggers only, re-read over REST and deduplicated by delivery id.

**AG-ACT-7. `trib-mcp` meets the spec's proxy consent rules.** (Security 7.) A consent page runs before the GHES redirect and shows the CIMD host, every redirect URI host, a warning for localhost-only redirects, and the repositories, toolsets, and write flag requested. After consent, a signed `__Host-` SameSite=Lax cookie binds the state and is checked at callback, and pages send `frame-ancestors 'none'`. Approved client ids are recorded in the user's `Identity` group, so a settings page lists and revokes each client. CIMD fetches have a 5 s timeout, a 5 KB cap, no redirects, denial of the install's own zones, HTTP caching, and per-IP rate limits on `/authorize`.

**AG-ACT-8. MCP refresh keeps its narrowing and detects replay.** (Security 8.) `McpRefresh` seals `sub`, `aud`, `repos`, `tools`, `write`, and a grant id, and every refresh calls `token/scoped` again. The unscoped `ghu_` is deleted right after scoping, with a Milestone 0 test that the `ghr_` survives [unverified]. Each grant keeps its current refresh generation as a row in the user's `Identity` group; an older generation is refused and its reuse revokes the grant. Rows: a refreshed token keeps its scope 100 of 100, and a replayed refresh revokes the grant.

**AG-ACT-9. `DELETE /installation/token` revokes stateless tokens.** (Security 9, a base defect.) On revoke, the token's `jti` and `exp` are committed to a `Tokens` shard chosen by a hash of the `jti`. Write-class requests check it on the leader, reads honor it within `READ_CACHE`, and rows expire at `exp`. This changes the base's journey 4 text and Shape. The A2 oracle row adds revoke, then push 1 s later, refused 100 of 100.

**AG-ACT-10. Hot groups stay under the 10 GB object cap.** (Foundation 1, blocker.) `LOG_MAX_BYTES` (2 GB [guess]) per member sits beside the 30-day rule. Above it, the leader archives the oldest entries to `R2Key::Archive` in two buckets before followers truncate them, so RPO stays zero. Check runs and statuses follow GHES's 90-day retention, `tributary/*` check runs are written only when a verdict changes, and `PathSet` rows are deleted when a PR closes. A Scale row gives member bytes on a hot group, and a base Milestone 0 row replays 30 days of A1 with member SQLite under 6 GB and zero `SQLITE_FULL`. AM1 is gated on that row.

**AG-ACT-11. Admission is by origin and by behavior.** (Foundation 2, user walk 5, security 10.)
- Caused commands (PrSync, test-merge records, check runs, and statuses for a PR with `AgentMeta` or a ref under an agent prefix) are `Agent` and count against the share.
- The cap is a token bucket of `AGENT_SHARE` times the group's measured capacity, with a floor of 0.2 of capacity that `Bot` cannot take. The lane order is stated once in both files.
- Any actor, user tokens included, above 60 write commands a minute on one group is admitted as `Agent` there. A push whose `validate` finds an agent trailer is `Agent`. Fair queuing by App or user runs inside the class.
- The `Tributary MCP` App is an `AgentApp` with `requester = sub`, so its writes fill `actor_is_agent` and take the agent class. Approve and merge are refused unless the consent page's write scope names them.
- Under admission pressure the leader queues only `PackRef::Stored` pushes and caps queued agent commands by count and bytes, refusing at the edge before staging.
- The 5,000-agent rows are recomputed with classed numbers.

**AG-ACT-12. Fan-out sizes on the hottest shard and controls itself in band.** (Foundation 3, security 10.) Pushes validate on any caught-up shard of the k, by rendezvous on the push id, and k is sized on the most loaded shard. `count` stays fixed, and each site reserves a hot pool at install (expected hot networks times `FANOUT_MAX - 1`), budgeted per owner. The network's first shard reports CPU p99 and queue depth each minute, and the network's group leader commits k with hysteresis, with no API call or cron. `AdmitClass` is carried into the shard work queue, so human and merge-queue validates and merges go first and agent test merges, predictions, and clone misses are shed first. Inline packs get a stored `.idx`, or `shard_vcpu` counts 2k ingests. The base Milestone 0 row adds a network that turns hot under the A1 replay with the API unreachable, passing at human push p99 of at most 3 s.

**AG-ACT-13. Lazy test merges have a budget.** (Foundation 4, user walk 3, security 10.) One compute per (PR, base tip). A GET triggers at most one compute per PR per 30 s, and only for writer and CI tokens. A compiled per-network budget of 25% of the k shards serves head pushes and Actions dispatch first, then single-PR GETs. List fields never trigger a compute. Over budget, `mergeable` is null or GraphQL `UNKNOWN`. "CI request" means Actions dispatch for every `pull_request`-family event. The base row adds 500 clients polling their PR every 5 s while main moves 10 times a minute, passing within the budget with dispatch p99 of at most 10 s.

**AG-ACT-14. Agents can get CI results without a human click.** (User walk 1, blocker; foundation 6.) The repository's agent policy gains `ci: Approve | AutoReadOnly`, default `Approve` as on GitHub. `AutoReadOnly` runs agent-push workflows at once with fork-PR semantics, and only jobs that reference secrets wait. The controller runs the repository's test command inside the Sandbox before each push, so a Tributary-run agent has a signal under `Approve`. A1 states that its CI terms assume `AutoReadOnly`, and gives approvals a minute under the default.

**AG-ACT-15. Every runtime piece has a home and a release path.** (Foundation 7.) The insights consumer runs in the site scripts. PrSync's apply enqueues one message per head push, and a consumer with the shard binding calls `predict` and posts the check run through the edge as the App. It gets eight failure rows and a deployment row. `mcp.<host>` is routed through `trib-front`, which sets the `trib-mcp` and edge overrides in one header that `trib-mcp` forwards. `tributary agents init --account $G` runs the App manifest flows, writes keys into the agents account's Secrets Store, and registers a webhook hostname that account owns. A `trib-ops` instance there executes `ControllerRoll` and `ControllerPointer` intents read from the install's `Release` group, and `Rotate` covers the App keys and client secret.

**AG-ACT-16. Managed rate limits fit a fleet.** (User walk 2.) For `AgentApp` installations, `x-ratelimit-*` is keyed per minted token, not per installation. First-party insights and agent-gate writes are exempt from content limits. A Scale row gives per-actor requests an hour against the managed limit.

**AG-ACT-17. Overlaps arrive before the work is done, and maintainers can triage.** (User walk 4 and 7.)
- `PathSet` is written for any push under an agent prefix, keyed by ref, and the overlap check run is posted on the head SHA before a PR exists. `PathSet.planned` comes from a `tributary-plan` block in a draft PR the controller opens at task start.
- `coordination.overlaps({paths?, pr?, ref?})` returns `[{pr, ref, app, session, requester, draft, last_push, shared_paths, verdict}]`. The controller subscribes to `check_run` for its PRs and passes verdicts into the running harness.
- A5's merge order is fixed.
- With `tributary-insights` installed, PRs get ordinary labels `agent:<app>` and `overlap:<cluster>`, and the overlap check lists other PRs that close the same issue. A `/_tributary/repos/{owner}/{repo}/agents` page shows clusters, verdicts, CI state, and a two-PR compare. Predictions run first for PRs that are approved, queued, or requested by a human.

**AG-ACT-18. Overload and refusals tell agents what to do.** (User walk 8.) For git, admission overflow answers HTTP 200 with report-status `ng <ref> agent admission full, retry after <n>s` and a `remote:` line. For REST, it answers 429 with `retry-after` and a "secondary rate limit" message. Refusal texts name the allowed prefix or the protected path. The base burst row's pass condition matches these shapes.

**AG-ACT-19. Agent spend is visible and capped.** (User walk 9.) The cost script adds a Sandbox compute row and a model-token row with the per-task assumption stated, both outside the comparison with the base. The controller counts model usage from each response and enforces per-task and per-org monthly token budgets and a default 60-minute task cap, ending a task as `Failed { reason }` with an issue comment. `tributary-insights` can be installed on all repositories. Deployment modes gains an "Enable agents" checklist.

**AG-ACT-20. "Why" is readable by agents.** (User walk 10.) Session pages serve JSON under `Accept: application/json` with a versioned `SessionLog` schema. An MCP `why({path, line, ref})` tool runs blame on the shard and returns commit, PR, sessions, and the issue snapshot. Confined third-party Apps may create non-draft PRs, because approve and merge stay refused and the gate holds. Tributary-run Apps always create drafts.

**AG-ACT-21. The hot inline cap yields during R2 outages.** (Foundation 8.) `INLINE_MAX_HOT` applies only while `PackStore::place` succeeds. When placement fails or a bucket's probe is red, inline up to `INLINE_MAX`, and AG-ACT-10's byte cap archives the extra after recovery. The base R2-outage rows and the tradeoff are updated.

**AG-ACT-22. The cost script is internally consistent.** (Foundation 9.) Monthly lines use one push rate (5 per task), and A1 is a separate peak case. Agent CI's clone-cache hit rate comes from the same miss model as the CPU term. The predictor is priced at 8 pairs, or "upper bound" is dropped. First-bucket clone-cache storage for agent tips is priced. Both cost tables, the summary, and the base's Cost rows are updated.

**AG-ACT-23. "Why not Artifacts" keeps only the reasons that hold.** (Foundation 10, user walk verdict.) The BYOC-parity and private-import reasons are dropped, because BYOC runs in the customer's Cloudflare account and seeding by push works. The whole-repository token reason is restated as mattering only for PR heads. The deciding reasons stay: the 1 GB cap against the monorepo, one logical instance per repository with no SLA, prediction needing branches in the network's shard, and same-network forks that already share packs. Best-of-N attempts fit in a workspace fork today. The flip condition keeps the scratch tier only if forks share objects and the cap rises in writing, falls back to a workspace fork on any Artifacts error, and treats GA under an SLA as an agent-reliability preference rather than an uptime condition.

## Consider

- Running the model loop in the controller, so the sandbox never calls a model API. It is the stronger form of AG-ACT-1, and it changes how vendor harness templates run. The writer states it as an option with its tradeoff.
- Per-tenant agents accounts in managed instead of per-tenant App keys (security 4). Recorded as the stronger isolation option.

## Dismissed

No whole finding was dismissed. `review/trail-review-7.md` flag 3 found six proposed fixes that the act-on list dropped or narrowed without a reason. Their disposition, applied in the docs on 2026-10-26:

- `ci: Auto` with secrets (user walk 1). Dismissed. It would run agent-written code with secrets before any human looked, which AG-ACT-3 forbids, and `AutoReadOnly` already gives agents a CI signal.
- `X-Tributary-Agent-Session` on every write and on git through `http.extraHeader` (user walk 5). Applied. From a user token it only marks the actor as an agent for audit and admission, and grants nothing (both agent blocks, AGENT-LAYER GHES table).
- A per-App cap on Bot commands, a hostile-owner fan-out row, and a 256 KiB burst row (security 10). The rows are applied (DESIGN Milestone 0). The per-App cap is dismissed, because fair queuing by App inside each class already bounds one App to its share of the Bot lane.
- Counting GET-triggered merges in `TEST_MERGES_PER_PUSH` (foundation 4). Dismissed. `TEST_MERGE_BUDGET` bounds GET merges to the budget left after head pushes, 0.75 vCPU at A1, which the budget's shard count already prices.
- Shipping the `trib-front` change as a `FrontRoll` (foundation 7). Applied (AGENT-LAYER AM2).
- "Human push p99 of at most 3 s" (foundation 3). Kept as "beyond transfer", because push time includes pack transfer, which grows with pack size. The base's other push rows measure the same way.

Separately, flag 2 found that AG-ACT-10's storage check ran on a fresh group. It now seeds the 4 GB metadata tier: a member holds about 7.0 GB after 30 days, and A1-class fleets on one repository wait for metadata child groups (DESIGN Scale and Milestone 0, AGENT-LAYER AM1).

## Agreement map

- **All three reviewers:** the agent CI gate contradicting A1's CI load and stopping the agent loop (AG-ACT-14, via user walk 1 and foundation 6; security 3 reached it through the secrets path) and lazy test merges turning eager under polling (AG-ACT-13).
- **Two reviewers:**
  - unbound or undefined `decide` inputs for the per-task prefix and protected paths (security and foundation);
  - agents outside `AGENT_SHARE` through user tokens and caused commands (foundation, user walk, and security 10);
  - protected paths missing harness files (security and user walk).
- **One reviewer:**
  - Security alone found host-granular egress, provenance spoofing, MCP consent and refresh, and stateless token revocation.
  - The foundation review alone found the 10 GB overflow, fan-out's hot first shard, and the missing release paths.
  - The user walk alone found rate limits, late overlaps, triage, error shapes, spend, and machine-readable "why".
