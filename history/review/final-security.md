# Final pass: security (FinalSecurity, security-reviewer, read-only)

I found 15 issues: 3 blockers, 9 majors and 3 minors. All three of your seeds hold, and each is worse than stated:

- **Seed 1:** under `AgentsExclusive`, one lease can block every agent push in the repository indefinitely.
- **Seed 2:** the refusal cannot be computed at all, because `decide` never receives the changed paths.
- **Seed 3:** anyone who can file an issue, or any App with `issues: write`, can launch a trusted Tributary-run task.

Two earlier fixes from the AWS design were lost in the Cloudflare design: short-lived signed intents, and re-applying revocations after a restore. This was a read-only pass; nothing was edited or run, and claims I could not check are tagged.

## Findings

### F1. Blocker: signed `Release` entries can be replayed, so a revoked vendor key comes back
**Where:** cloudflare/DESIGN.md:360-387 (`ReleaseEntry`, `Keys`, `SignedIntent`), 354-355 (`StaleMode`), 809 (revocation runbook)

**Gap:** The only check on a `Release` entry is that its signer appears in the latest `Keys`. No entry carries an install id, a sequence number, the previous entry's digest, or an expiry.
- An old `Keys { vendor: Some(V) }` signed by the still-current customer key can be resubmitted after the revocation runbook. It verifies and makes `V` trusted again.
- The same works for other old customer-signed entries:
  - `LifeboatAdd` brings back a removed lifeboat key, which can pull log tails from every group.
  - `StaleMode` makes every write return 503 for 4 hours by default.
  - `SiteLost` triggers `ReplaceMember` on a healthy site.
  - `Resume` undoes a `Halt`.
- Pre-signed undo pointers (`EdgePointer` or `OpsPointer` back to N) can be replayed long after N+2 ships, which downgrades the edge.
- Vendor intents carry no install binding, so one install's intent can be replayed on another.

**Evidence:**
- The design says entries are submitted "through the edge" (809) and that the vault and lifeboat export cover every chain, the `Release` group included (811). The lifeboat host is "not a hardened sandbox" (785).
- The AWS design had `Intent { id, issued_at, not_after }` and `Halt { intent }` (DESIGN.md:1008, 1050; review/interrogate.md OP-ACT-5). Neither carried over.
- The CR-ACT-2 disposition (review/cloudflare-interrogate.md:165) is incomplete: checking the key does not prevent replay.

**Fix:**
- Every entry signs over `(install_id, group, seq = latest+1 or prev_digest)`.
- `Keys` carries a `generation`, and the group refuses any generation that is not exactly the latest plus one.
- Intents get `not_after`.
- An `undo` is valid only while its parent intent is the current one.
- `Halt` and `Abort` name their intent.
- Add a Milestone 5 row: replaying every historical entry after `Keys { vendor: None }` is refused 100 of 100.

### F2. Blocker: nothing ties an `AgentTask` to an App, and task state has no transition rules
**Where:** cloudflare/AGENT-LAYER.md:79-87, 272, 276; DESIGN.md:296-304; design/SHARED.md:288-289

**Gap:** `AgentTask` has no `app`, `installation`, or `creator` field. Yet four rules depend on that binding:
- minting must check `tributary_task` against the task;
- "`PATCH /{id}` lets the task's App" change it;
- cancel is open to "the task's App";
- `X-Tributary-Agent-Session` is "accepted only for a session the same App opened" (AG-ACT-5).

None of these can be enforced. Consequences:
- **Forged provenance:** any App can mint a token naming another App's Tributary-run task. Its pushes then link as `Source::Session`.
- **Forged task state:** that App can also PATCH the other task's state, `pr`, or `summary`.
- **Cancel can be undone:** `PATCH` sets `state` with no rules, so a cancelled task can be set back to `in_progress`. That makes it live, and token minting works for it again.
- **`requester` is undefined** when an App creates a task, even though the field is a non-optional `UserId`.
- **`prefix`** is never stated to be derived by the host rather than taken from the request body.
- **SHARED is stale:** `InstallationClaims` has no `task` claim.
- **The controller failure row no longer contains anything:** it says `decide` "still refuses … pushes with no live `AgentTask`", but a compromised controller can now create live tasks at will through `POST`. That disposition was right for AG-ACT-4 when written; the task API reopened it.

**Fix:**
- Add `app`, `installation` and `creator: Actor` to `AgentTask`.
- Minting requires `task.app == jwt.app` and the task's repository among the token's `repositories`.
- Derive `prefix` from `session`, never from the body.
- Terminal states (`completed`, `failed`, `timed_out`, `cancelled`) are final. Only the cancel endpoint or the host timer can set `cancelled` or `timed_out`.
- Add `task` to `InstallationClaims`.

### F3. Blocker: `decide` never receives the changed paths, so the `AgentsExclusive` refusal cannot run
**Where:** cloudflare/AGENT-LAYER.md:96-99, 125-127, 155; DESIGN.md:342-345, 427-428; design/SHARED.md:61, 211-212

**Gap:**
- `Push` carries `updates`, `atomicity`, `pack`, `actor`, `options`, `request`, `provenance` and `protected`, and nothing else.
- `ProtectedHits.paths` holds only the paths that match the protected list. `validate` returns `(PackIndex, Provenance, ProtectedHits)`.
- Leases change between `validate` and `decide`, so `validate` cannot check them in advance.
- `decide` therefore has no path set to compare with live Exclusive leases.
- `PathSet.paths` has no declared source either.
- REST ref-only writes (`POST`/`PATCH /git/refs`, `POST /merges`) stage no pack. SHARED:61 says "refs through `commit`", so `validate`, and with it `protected`, may never run for them [inference]. The Milestone 0 refusal row tests REST but names no mechanism.

**Fix:**
- `validate` returns `ChangedPaths { refname, paths ≤ PATHS_MAX, truncated }` for each agent or agent-prefix ref update, and `Push` carries it.
- `decide` matches it against live Exclusive leases. When `truncated` is set, a directory counts as overlapping.
- Index Exclusive globs by literal prefix so the check fits in group commit.
- Every REST ref write by an `Agent`, or under an agent prefix, runs `validate` on an empty staged pack.

### F4. Major: in fork mode (the default), the fork group's `decide` cannot see the base's task, leases, or policy, so cancelling does not stop pushes
**Where:** cloudflare/AGENT-LAYER.md:8, 289, 314, 318, 321; DESIGN.md:229-230

**Gap:**
- Pushes land "on the fork's own group", while "tasks and leases stay on the base".
- On the fork, `decide` cannot check "no live `AgentTask`", cannot link `Source::Session` ("because the pusher holds the matching `AgentTask`"), and cannot apply `AgentPolicy.extra_paths` or `AgentsExclusive`.
- After Bob cancels, the router's cached token keeps pushing to the fork until its `exp`.
- The fork is created `default_branch_only`, and its ruleset blocks every ref except `refs/heads/tributary/**`. Its default branch therefore stays frozen, and a merge base taken against it reports upstream workflow changes as protected hits [inference].
- `deps()` exists (DESIGN:229, "e.g. the owner's Authz"), but no agent dependency is declared.

**Fix:**
- When a token has a `task` claim, `Push::deps()` includes the base repository's chain, read `fresh`.
- `decide` checks the task's liveness, `AgentPolicy` and leases there.
- Compute the merge base against the base's default-branch tip.
- Add a fork-mode ceiling row: a push after cancel is refused.

### F5. Major (seed 1): any push token can lock out every agent with Exclusive leases that renew indefinitely
**Where:** cloudflare/AGENT-LAYER.md:118-127, 274

**Gap:**
- "Any token with push access may acquire", including humans, unflagged Apps, workflow `GITHUB_TOKEN`s with `contents: write`, and MCP users.
- Scope may be `**`: up to 32 globs per lease and 8 leases per holder.
- TTL goes up to 3,600 s, and "Renew moves `expires`" with no limit on total lifetime. Only an admin can release.
- Under `AgentsExclusive`, one such lease refuses every `ActorKind::Agent` push in the repository.
- The reverse attack also works: a Shared `**` lease makes every Exclusive acquire return 409.
- Holders are keyed by `(actor, app, task)`. Vendor tokens carry no task, so a whole vendor fleet shares 8 leases, against the Scale row's "500, one per agent".

**Fix:**
- Exclusive leases need a live task or PR whose ref the holder names, at most 1 per holder.
- Cap Exclusive breadth: no `**` or top-level `*`, and at most N tracked paths, checked on the shard at acquire.
- Cap total lifetime (for example 4 h, after which the lease drops to Shared).
- Under `AgentsExclusive`, leases held by non-agents do not refuse pushes unless an admin set them.
- Give vendor agents a per-agent holder through External tasks (needs F2).

### F6. Major: plan-block leases skip the push-access rule, so outsiders can fill a public repository's lease cap
**Where:** cloudflare/AGENT-LAYER.md:274, 394; FRAME.md:21

**Gap:**
- "A block in a draft PR body becomes a Shared lease held by the PR's author or App". No push-access check applies.
- Managed hosts public repositories that anyone can fork and open PRs against.
- Each such lease lives "at most 24 h after its last push", and a push to the fork branch refreshes it.
- **Computation:** 4,000 leases (`LEASES_PER_REPO`) ÷ 8 per holder = 500 sockpuppet accounts. If plan leases are not counted per holder, one account at 500 PRs an hour gets there in 8 h, inside the 24 h window. After that, "an acquire past either cap is refused", including the controller's task-start lease.
- A writer who edits someone else's PR body creates a lease attributed to that author.
- The feed returns the `Lease` (with its untrusted `intent`) and `AgentPush.paths` as raw JSON. Every polling agent receives that outsider text.

**Fix:**
- Plan-block leases only for authors (or Apps) with push access, and attributed to the editor.
- They count toward the per-holder cap, and plan leases get their own sub-cap per repository.
- Add trust-view fields to every text value in the feed.

### F7. Major (seed 2): only `ActorKind::Agent` is ever refused, and the documents disagree about who that is
**Where:** cloudflare/AGENT-LAYER.md:7, 68, 131-134, 329-332, 590; DESIGN.md:285

**Gap:** Three statements conflict:
- base block: "Tributary's agent Apps, plus any App an admin flags";
- layer: "a ThirdParty App is one only when `confine` is set";
- Summary: flagging an App "confines it to a branch prefix".

The layer reading has consequences:
- A5's "Y (a vendor App)" is refused only if Y is confined.
- The `Tributary MCP` App "acts with its user's rights", and user tokens sending the agent-session header "grant nothing". Neither is ever refused by leases, protected paths, or the `tributary/` status rule.
- The Milestone 0 row "100 of 100 agent pushes refused" leaves its actor kind undefined.

**Fix:**
- One rule in both blocks: flagging requires `confine`, or flagged-but-unconfined Apps are explicitly `Bot`.
- Rewrite A5 so Y is confined.
- State that `AgentsExclusive` does not bind MCP or user tokens.
- Run the Milestone 0 row once per actor kind.

### F8. Major (seed 3): anyone who can file an issue can create a trusted Tributary-run task
**Where:** cloudflare/AGENT-LAYER.md:276, 321, 393; the `Task.snapshot` field in Shape

**Gap:**
- Creating a task needs `issues: write` plus `contents: read`. That is the bar for filing an issue, which read-role users meet, any App with issues permission meets, and on public managed repositories any signed-in user meets [unverified for fine-grained cross-owner tokens].
- The controller runs any Tributary-run task, whoever created it (A3 step 10).
- A confined vendor App, or a prompt-injected one, can create a Tributary-run task. The resulting agent pushes `refs/heads/tributary/<task>/` with `contents: write` in-repo, outside the vendor's `confine`. "Stays under its `confine`" covers External tasks only.
- The 64 KiB prompt is "trusted as the writer's text" with no trust view. Text the vendor agent read in an issue body therefore becomes trusted input.
- `POST` with an `issue` has no `Assigned` snapshot, so the rule that pauses a task when a non-writer edits the issue never applies.
- Nothing caps tasks per creator. BYOC turns rate limits off, and each row can hold 64 KiB.

**Fix:**
- Creating a Tributary-run task requires a human user credential with `contents: write`. Not an installation token, not an AgentApp's `ghu_` token, not a demoted or agent-session actor.
- Other Apps may create External tasks only.
- The controller App may create a task only from an issue whose latest `Assigned` entry names its bot.
- Render the prompt through the trust view and record its creator.
- Snapshot at the create index.
- Cap open tasks per requester and per repository.

### F9. Major: an agent can satisfy the human merge gate through MCP; Code Mode skips the approve/merge restriction
**Where:** cloudflare/AGENT-LAYER.md:88-90, 132-134, 142, 216-219, 232-233

**Gap:**
- **Code Mode bypass:** approve and merge are refused only in `trib-mcp`'s tool layer. `codemode.request()` is "bound to `upstream`" with no allowlist, so model-written code can approve a review or merge directly. The host does not refuse it, because MCP is not `ActorKind::Agent`.
- **MCP approvals count as human:** an APPROVE made through the user's `ghu_` token is the user's approval under GHES semantics. It can therefore become `AgentGate.satisfied_by`, which lets one agent pass another agent's human gate.
- **Vendor PRs:** `AgentMeta.requester` is `None`, so the user who prompted a vendor agent may approve its PR.

**Fix:**
- `decide` takes `satisfied_by` only from a credential that is no AgentApp's.
- Enforce `WriteScope` on the host, with the grant id as a claim.
- Run `codemode.request()` through the same tool deny-list.
- Let a vendor carry `requester` through its External task.

### F10. Major: session logs and prompts are readable with only `metadata: read`
**Where:** cloudflare/AGENT-LAYER.md:276, 393

**Gap:**
- "Reads need `metadata: read`", and that covers `GET /{id}/log` and `GET /{id}` with its prompt.
- A `SessionLog` holds tool outputs, diffs and test output, which is effectively source code.
- Every App installation, and every fine-grained token, has `metadata: read` [unverified].
- So an App installed with issues-only permissions can read private code. Feed paths leak file names the same way.

**Fix:** `contents: read` for `/log`, prompts, session pages, and the feed's path fields.

### F11. Major: a whole-install restore undoes every revocation made after the cutoff
**Where:** cloudflare/DESIGN.md:811-816, 935

**Gap:** Restore rebuilds every chain, `Release`, `Tokens` and `Identity` included, as of the cutoff. Nothing re-applies what happened afterwards:
- the vendor key revoked during the incident;
- deleted PATs, `jti` revocation rows, removed SSH keys;
- uninstalls, member removals, `LifeboatRemove`.

The installation-token signing key is not rotated either. Old verifying keys come back in `Identity`, so tokens minted with a key stolen in the compromise still verify.

**Evidence:**
- The AWS design "replays every revocation record newer than t" (DESIGN.md:1197; review/interrogate.md OP-ACT-4; selfhost-rewalk N2).
- The vault keeps entries past the cutoff.
- The Milestone 7 gate checks only that PATs and SSH keys still work.

**Fix:**
- Re-apply revocation-class entries newer than the cutoff from the vault log.
- The new genesis `Keys` replaces the restored `Release` group.
- Rotate the signing key and drop every old `kid`.
- Add a gate: a PAT revoked after the cutoff fails after restore.

### F12. Major: the feed's request rate is about 11 times what A1 assumes, and anonymous readers can drive it
**Where:** cloudflare/AGENT-LAYER.md:178-180, 188, 291, 459; FRAME.md:21

**Gap:**
- A poll with no `paths` returns on any event, then waits `FEED_BATCH` (1 s).
- A1 has at least 6.7 feed events/s: 4.17 pushes, 1.67 task changes, 0.83 lease acquires and releases.
- Each poller therefore makes about 0.87 requests/s, about 435/s for 500 pollers, against the 40/s assumed.
- Polls are held on voting members. "One feed request a second" per poller has no identity to count anonymous public readers by.

**Fix:**
- Authenticated polling only, with per-token and per-IP limits on concurrent polls.
- Serve from learners only.
- Count feed polls toward `LEARNER_AT`.
- Recompute A1.

### F13. Minor: leases and External tasks outlive revocation
**Where:** cloudflare/AGENT-LAYER.md:81, 110-118, 274; design/SHARED.md:328

**Gap:**
- A lease is live while `at < expires`, and nothing re-checks whether its holder is still authorized.
- Uninstalling an App, or removing a user's write access, leaves their Exclusive leases in place for up to 3,600 s, plus the outage extension.
- Plan-block leases need no renewal, so they persist for 24 h.
- External tasks never expire.

**Fix:**
- The lease check drops leases whose holder fails the owner `Authz` check `decide` already reads.
- Uninstalling or suspending an App also releases its leases and cancels its tasks.
- External tasks go to `timed_out` when idle.

### F14. Minor: the drain-only key can drain every site at once
**Where:** cloudflare/DESIGN.md:203-207, 365, 376, 542, 894

**Gap:**
- "Never drains a second site while one is drained" is `trib-ops` behavior, not a rule in the `Release` group's `decide`.
- A bad `trib-ops` or a leaked drain key can drain all three sites.
- With every site drained, transfers have no target and drained members refuse to campaign. After any leader loss, writes stop install-wide [inference].

**Fix:**
- The group refuses a drain-key `SiteMode Drain` while another site is drained.
- Pre-vote allows a drained member to campaign when no undrained voter is available.
- Add a Milestone 0 row that submits two drains directly to the group.

### F15. Minor: a `Co-authored-by` trailer moves a human push into the agent lane
**Where:** cloudflare/AGENT-LAYER.md:37, 105, 318

**Gap:**
- A push is classed Agent when "`validate` finds an agent trailer", which is never defined.
- `TrailerKey` includes `CoAuthoredBy`, which is common on human pair-programming commits.
- Such a human push queues behind a full agent bucket and gets `ng … agent admission full`. That breaks "Human and merge-queue commands keep their lane".

**Fix:** Only `TributarySession`, `ClaudeSession`, `AmpThreadId` and `AssistedBy` classify a push, and a trailer never demotes a user-token push on its own.

## What I reviewed
- **cloudflare/AGENT-LAYER.md, all of it:** the Shape blocks, Leases and Tasks prose, A1–A8, GHES table, failure modes, Milestone 0 rows, tradeoffs, and resolutions.
- **The mirrored agent block** in cloudflare/DESIGN.md:250-359.
- **DESIGN.md base sections you named:**
  - Release group and `Keys` (360-387);
  - Identity (431-453);
  - journeys 4 and 7;
  - the release control, vault and lifeboat failure rows;
  - Install operations: Genesis, Keys, the revocation and rotation runbooks, Vault, Whole-install restore, Lifeboat;
  - Milestones 5–7.
- **Other sources:**
  - design/SHARED.md for tokens, `Push` and `rest:git`;
  - FRAME.md's operator decisions;
  - review/cloudflare-leases-tasks.md, the two interrogate files, and cloudflare-agents-security.md;
  - the AWS DESIGN.md for the earlier intent and restore fixes.

I did not run the cost scripts; the arithmetic above is from the documents' own constants.