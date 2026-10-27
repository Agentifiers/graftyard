# Foundation attack: the agent layer's pick, its base changes, and its installs

Inputs: `cloudflare/AGENT-LAYER.md`, `cloudflare/cost_agents.py`, the agent changes in `cloudflare/DESIGN.md`, `cloudflare/cost.py`, `arena/cloudflare-agents/SYNTHESIS.md` and `TASK.md`, `review/cloudflare-agents-judge.md`, `research/agents.md`, and `research/artifacts-recheck.md`, read on 2026-10-26. Labels: [measured] means I ran it, read it in the file, or read it on the cited page. [inferred] means my reasoning from measured facts. [guess] means a judgment with no direct evidence. Line numbers are of the files as read.

## Verdict

Holds with changes. GHES-native agents with no Artifacts and no coordination object is still the right pick for agent load, because none of the defects below is fixed by candidate C's coordination object or by candidate B's workspace tier, and both of those inherit the same five base changes [inferred]. Artifacts still earns no role, although two of the reasons given for that are wrong for this design (finding 10). What does not hold is the base changes as written. At the layer's own rates the monorepo group's members reach the 10 GB Durable Object cap, after which every write to that repository fails, humans included (finding 1). The admission classes protect neither humans nor fleets, because the commands agents cause ride the Bot lane and agents on user tokens ride the Human lane (finding 2). Fan-out hides a shard at 109% and has no control loop (finding 3), lazy test merges turn eager again under GHES's own resubmit-on-null contract (finding 4), and two of `decide`'s new refusals have no inputs (finding 5). The layer is agent-friendly at its MCP and Checks surfaces, but its default CI gate stops the agent's test loop and contradicts A1's CI rates (finding 6), three new runtime pieces have no home or release path in BYOC and self-hosted (finding 7), and the cost totals are not the upper bounds they claim (finding 9).

## Findings

### 1. Blocker. Hot groups overflow the 10 GB per-object storage cap

Files: `cloudflare/DESIGN.md` lines 249-277 (no storage rule in the agent block), 326, 510. `cloudflare/AGENT-LAYER.md` lines 72-82, 138-148, 210-228 (no member-bytes row). `cloudflare/cost.py` line 178. `cloudflare/cost_agents.py` lines 41, 65, 78.

Failure path. A typical agent push is inline, because the 40 KB average is under `INLINE_MAX_HOT`, so its pack lives in the entry, and every member keeps 30 days of log (DESIGN 510) or about a week (cost.py 178). Session-log blobs of 200 KB are also under 256 KiB, so they inline too. All three members store the same entries, so they fill together. When they pass 10 GB, their SQLite writes fail, the group commits nothing, and every push, merge, and comment on the monorepo fails until an operator deletes data.

Evidence.
- "Storage per Durable Object | 10 GB", and at that limit "write operations ... will fail with ... `SQLITE_FULL`" (https://developers.cloudflare.com/durable-objects/platform/limits/) [measured].
- The cost script puts every peak agent on the largest network (`HOT_SHARE = 1.0`, line 78) and 16.5 agent pushes per developer per day into the base workload. That is 1,003,200 pushes a month at 2,000 developers, or 41 GB a month at 40 KiB [measured, arithmetic from the script's constants]. At cost.py's own one-week life that is 10.3 GB per member on one group, before the 4 GB metadata tier the base already assumes for its largest repository (DESIGN 510). At the base's 30 days of log it is 41 GB [inferred].
- Session logs at the script's 200 KB add about 18.7 GB a month to whichever group the Tributary-run agent pushed to [inferred]. Fork mode does not avoid it, because the one workspace fork group per repository then carries every Tributary-run push and log, about 9 GB at a one-week life [inferred].
- Metadata grows as well. 6,000 agent PRs a day land on one repository (`arena/cloudflare-agents/TASK.md`), each with about 10 `tributary/*` check runs at two per head push, a `PathSet` of up to 1,000 paths, `AgentMeta`, provenance rows, and two `refs/pull/N/*` refs. At 300 bytes per check run and 2 KB per PR, the tier gains about 1.6 GB of check runs at a 90-day steady state plus 0.4 GB of PRs a month, which reaches the base's 8 GB child-group threshold in about 6 months [guess sizes, inferred arithmetic]. Child groups are base Milestone 7, while AM1 ships these rows with base Milestones 2 and 3.

Fix.
- Add `LOG_MAX_BYTES` (for example 2 GB) per member beside the 30-day rule. Above it the leader archives the oldest entries to `R2Key::Archive` in two buckets before followers truncate them, so inline packs stay fetchable for rehydration and RPO stays zero.
- Store every push under `SESSION_REFS` as `PackRef::Stored`, whatever its size.
- Implement GHES's documented checks and statuses retention, "retained for 90 days" and settable from 1 to 400 days (https://docs.github.com/en/enterprise-server@3.22/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository) [measured]. Write `tributary/*` check runs only when the verdict changes (the layer's own open question, AGENT-LAYER 436), and delete `PathSet` rows when a PR closes.
- Add a Scale row for member bytes on a hot group, counting log, folds, and refs, and a base Milestone 0 row of 30 days of the A1 replay with member SQLite under 6 GB and zero `SQLITE_FULL`. Gate AM1 on that row or on child groups.

### 2. Major. Admission classes protect neither humans nor agent fleets

Files: `cloudflare/DESIGN.md` lines 250-255, 516, 614, 847. `cloudflare/AGENT-LAYER.md` lines 34-35, 68-70, 145, 148, 215, 289. `cloudflare/cost_agents.py` lines 239-250, 264, 283.

Failure path.
- Caused commands sit outside the cap. Only commands whose actor is an `AgentApp` are `Agent`. PrSync for agent pushes is a bot command (cost.py, "PrSync bot entries"), the overlap and provenance check runs are "Admitted as `Bot`" (AGENT-LAYER 289), and agent CI's `RequiredCheck` comes from Actions. At A1 these are 14.2 of the 22.7 non-human commands a second [measured, `--a1`]. AGENT-LAYER 35 puts Bot in the first lane with Human and MergeQueue, while DESIGN 250 names only Human and MergeQueue. On the layer's reading, agent-class load is 0.017 commands/s per agent and the Bot load it causes is 0.028. The 150/s agent cap binds only above about 8,800 agents, the 300/s group saturates at about 6,600, and above about 10,500 the first lane alone fills the group and no agent command is admitted [inferred from the script's rates]. So "150/s admitted, about 3,300 agents per group" (AGENT-LAYER 148, 215, DESIGN 516) is wrong both ways. At 5,000 agents nothing is throttled, and humans share a 229/s first lane with 142/s of Bot work caused by agents.
- Agents with human credentials are Human. Claude Code CLI, Codex CLI, and Copilot CLI push with the developer's own `gh` and git credentials, Devin documents a PAT path, Factory uses an OAuth App, and Cursor's self-hosted pools run "up to 200 workers per user" (`research/agents.md` section 2, 2.6, 2.9, 3.5) [measured]. All of these are `ActorKind::User`, so `AdmitClass::Human` with no cap [inferred]. An unflagged vendor App is Bot, in the same first lane.
- "AGENT_SHARE of the group's measured command rate" (DESIGN 251) reads as a share of throughput. Agents are 92% of A1's commands, so a literal implementation holds them to the human rate at ordinary A1 load. A1 step 6 means 0.5 of the 300/s capacity instead (AGENT-LAYER 145) [inferred].

Fix.
- Class caused commands by origin. PrSync, test-merge records, check runs, and statuses for a PR whose `AgentMeta` is set, or for a ref under an `AgentActor` prefix, are `Agent` and count against the share.
- Define the cap as a token bucket of `AGENT_SHARE` times the group's measured capacity (entries a second times commands per entry at the current byte mix), and add a floor, for example 0.2 of capacity, that Bot traffic cannot take.
- Demote by behavior. Any actor, user tokens included, above 60 write commands a minute on one group is admitted there as `Agent` (GitHub's content-creation limit is 80 a minute, `research/agents.md` 3.7). Fair-queue by App or user inside the class.
- State the lane order once in both files and recompute the 5,000-agent rows with classed numbers.

### 3. Major. Fan-out hides a shard at 109% and has no control loop

Files: `cloudflare/AGENT-LAYER.md` lines 42-46, 144, 222, 300. `cloudflare/DESIGN.md` lines 165, 326, 343-352, 518, 520, 627, 629, 848. `cloudflare/cost_agents.py` lines 140-149, 161-169.

Failure path.
- "Pushes validate on the first" of the k shards (AGENT-LAYER 42), so index-pack's 2.19 vCPU stays on one shard while the other 10.9 vCPU spreads over five. That shard carries 2.19 + 10.9 / 5 = 4.37 vCPU on a 4 vCPU `standard-4`, 109%, while the average is 66% [inferred from `--a1`]. No k up to 8 fixes it (2.19 + 10.9 / 8 = 3.55, 89%), because `fanout` sizes on the average. Human pushes to the monorepo validate on the same shard, and `AdmitClass` lives at the group leader, not in the shard's queue, so a human push waits behind agent test merges and clone misses.
- Each of the 2k warm shards must ingest every pushed pack before it can serve tips that include it, and `PackRef::Inline` has no `PackId` and so no stored `.idx` (DESIGN 326). The model counts index-pack once per push [inferred].
- `shards_for(network, site, count, k)` takes top-k by rendezvous among `count` shards, with `SHARDS = [2, 5, 25]` (DESIGN 350-352). If the k are existing shards, the $660 line and "max_instances rises by 4 per site" (DESIGN 518) are not needed, but each of them also keeps its own networks and a 5 GB copy of the monorepo on 16 GB of usable disk. If they are new instances, `count` changes and rendezvous moves about 4 in 29 of all other networks to cold shards on that site each time [inferred].
- `NetworkLoad` has no stated source or cadence. The one analogous function, `place_learners`, runs from a daily `trib-ops` cron over Analytics Engine (DESIGN 165), and raising `max_instances` "is a site-day operation" (DESIGN 520). A network that turns hot after install, or a second hot network, gets no extra shards until three site days run and none during a control-plane outage. Meanwhile it asks 13.1 vCPU of 4 [inferred].

Fix.
- Validate on any caught-up shard of the k, chosen by rendezvous on the push id, and size k on the most loaded shard, not the average.
- Keep `count` fixed. Give each site a hot pool of H times (`FANOUT_MAX` - 1) instances reserved at install from the operator's expected number of hot networks, and let `shards_for` choose inside the pool.
- Compute `fanout` in band. The network's first shard reports its CPU p99 and queue depth each minute, and the network group's leader commits k with hysteresis, with no API call or cron.
- Carry `AdmitClass` into the shard's work queue. Human and merge-queue validates and merges go first, and agent test merges, predictions, and agent clone misses are shed first, which GHES allows as `mergeable: null`.
- Store an `.idx` for inline packs, or count 2k ingests in `shard_vcpu`. Extend DESIGN 848 with a network that turns hot under the A1 replay with the API unreachable, passing at human push p99 of at most 3 s.

### 4. Major. Lazy test merges become eager under GHES's resubmit contract

Files: `cloudflare/DESIGN.md` lines 260-262, 519, 849. `cloudflare/AGENT-LAYER.md` lines 47-48, 144, 240. `cloudflare/cost_agents.py` line 50.

Failure path. With main moving 10 times a minute, every open PR's test merge is `Stale` within 6 s. GHES says a null `mergeable` means "a background job" has started and the client should "resubmit the request" (https://docs.github.com/en/enterprise-server@3.22/rest/pulls/pulls) [measured], and PrSync computes on every GET. Each poll of a stale PR costs a 1.5 vCPU-s merge, so the bound is the 125 vCPU eager case the change exists to avoid [inferred]. If one in five of A1's 30 agent REST and MCP reads a second is a PR GET, that adds 9 vCPU, 70% on top of the 13.1, and PR pages and GraphQL `mergeable` on list queries add more [inferred]. DESIGN 849 tests base moves with no readers.

CI timing. `pull_request` runs that a head push triggers get their merge in time, because PrSync computes on the push. Runs triggered by `labeled`, `ready_for_review`, `reopened`, `pull_request_review`, and `pull_request_review_comment` use the "Last merge commit on the `GITHUB_REF` branch", and "Workflows will not run on `pull_request` activity if the pull request has a merge conflict", so the dispatcher needs a fresh merge before it decides (https://docs.github.com/en/enterprise-server@3.22/actions/reference/workflows-and-actions/events-that-trigger-workflows) [measured]. "CI request" is not defined, so those runs either start on a stale merge or queue behind GET-triggered work [inferred]. Every agent PR goes through `ready_for_review` by a human, so this is the common case.

Fix.
- Coalesce to one compute per (PR, base tip), and let GETs trigger at most one compute per PR per 30 s, which bounds GET work at 25 vCPU for 500 PRs before shedding.
- Add a compiled per-network test-merge budget, for example 25% of the k shards, with priority to head pushes and Actions dispatch, then single-PR GETs, and never list fields. Over budget, return `null` or GraphQL `UNKNOWN` without queueing.
- Define "CI request" as Actions dispatch for every `pull_request`-family event.
- Add to DESIGN 849 500 clients polling their PR every 5 s while main moves 10 times a minute, passing at test-merge vCPU within the budget and dispatch p99 of at most 10 s. Count GET-triggered merges in `TEST_MERGES_PER_PUSH`.

### 5. Major. Two of `decide`'s new refusals have no inputs

Files: `cloudflare/DESIGN.md` lines 263-272, 354, 850. `cloudflare/AGENT-LAYER.md` lines 50-55, 68-76, 121-127, 162, 166, 173, 279, 388.

Failure path.
- Per-task prefix and requester. `AgentActor.prefix` is meant to be `refs/heads/tributary/<task>/` (DESIGN 268), but the only host-side source is `AgentApp.confine`, one pattern per App (AGENT-LAYER 70). Installation tokens take only `repositories` and `permissions` (`research/agents.md` 4.2), and the per-task check is the outbound handler, controller code in the agents account (AGENT-LAYER 126). So the host can refuse at most refs outside `refs/heads/tributary/**`. A bad controller, which the failure table says `decide` caps "at `tributary/<task>/` pushes" (AGENT-LAYER 279), can force-push every other task's branch and open PR head, and the Milestone 0 row in which a "Workspace token pushes ... to another task's prefix" and is refused 100 of 100 times (AGENT-LAYER 388) cannot pass for a token used directly [inferred]. `AgentMeta.requester`, whom the agent gate excludes, has no defined source. If the controller supplies it, a bad controller can name another user and let the requester approve their own agent's PR [inferred].
- Changed paths. Refusing "changes under PROTECTED_PATHS" needs the paths each new commit touches. `validate` returns `(PackIndex, Provenance)` (DESIGN 354), `Push` gains only `provenance`, and `decide` is pure over the fold, so no path data reaches it [measured, from the signatures].

Fix.
- Before it mints, the controller commits `AgentTask { session, prefix, issue }` to the base group as its App. The outbound handler adds `X-Tributary-Agent-Session` to every request, receive-pack included. `decide` checks each ref against that task's prefix and refuses an `Agent` push with no live task. Take `requester` from the actor of the issue's `Assigned` entry that names the bot, which the host itself committed (AGENT-LAYER 162), never from the controller.
- `validate` returns, per ref update, the `PROTECTED_PATHS` hits of every new commit (one not reachable from existing refs) against each of its parents, and `Push` carries them beside `provenance`. Add a token with no session header and a merge commit that touches `.github/workflows/` to DESIGN 850.

### 6. Major. The default CI gate stops the agent loop and contradicts A1's CI rates

Files: `cloudflare/DESIGN.md` lines 265-266. `cloudflare/AGENT-LAYER.md` lines 52-53, 140, 144, 168, 243.

Failure path. Every Actions run on an `ActorKind::Agent` push waits as `action_required` until a writer approves it (DESIGN 265). In fork mode GHES's "Require approval for fork pull request workflows" means "Workflow runs on pull requests from collaborators without write permission will require approval" (3.22 Actions settings page, URL in finding 1) [measured]. A1 counts 1.25 agent CI jobs a second, their `RequiredCheck` commands, and their clone-cache misses (AGENT-LAYER 140, 144). Under the default that is 75 approvals a minute from 50 humans, or the CI never runs and agents get no test result before a human looks [inferred]. Flagging a vendor App turns the same gate on for it, which gives admins a reason not to flag, and that loses the confinement and admission of finding 2 [inferred]. GitHub added a repository setting to skip this approval for Copilot because the default "slows down the feedback loop for validating Copilot's work and finding out if tests pass" (https://github.blog/changelog/2026-03-13-optionally-skip-approval-for-copilot-coding-agent-actions-workflows) [measured].

Fix. Add `ci_without_approval: bool` to `AgentReviewPolicy`, default false as on GitHub. When it is true, `Agent` runs start at once with a read-only `GITHUB_TOKEN` and no secrets, which is 3.22's "Run workflows from fork pull requests" behavior, and `PROTECTED_PATHS` already keeps workflow edits out. State that A1's CI terms assume the setting on, and add approvals a minute as an A1 quantity under the default.

### 7. Major. Three new runtime pieces have no home or release path

Files: `cloudflare/AGENT-LAYER.md` lines 30, 88-93, 102-117, 203, 254, 262, 270-271, 279, 307, 309-322, 402-404. `cloudflare/DESIGN.md` lines 79, 418, 705.

Failure path.
- `tributary-insights` is called a REST client (AGENT-LAYER 307), but `GitCacheShard::predict` is a method on a Durable Object in the site scripts, which a REST client cannot call. The App is missing from the new-pieces list (30), the deployment table (309-322), and the failure components (262), so nothing triggers `predict` or turns a `Verdict` into an `InsightCheck` [measured, from the text].
- `trib-front` is the only reader of `EdgePointer` (DESIGN 79, 418), and `mcp.<host>` is served by `trib-mcp` itself (AGENT-LAYER 102). A version override is a header set by the caller, and overrides "only apply to `fetch()`-based service binding calls" (https://developers.cloudflare.com/workers/versions-and-deployments/version-overrides/) [measured]. So `McpPointer` has no reader, and `trib-mcp`'s calls into the edge carry no `EdgePointer` override. During an edge swap-back with the API down, MCP traffic still reaches the bad edge version at its rollout share, and the AM2 gate (402) cannot pass as built [inferred].
- The controller has no deployer, signer, or rollback. `trib-ops` lives in the install account, the vendor holds no Workers permission in BYOC (DESIGN 705), and "Its own account and release" (AGENT-LAYER 279) names no mechanism. The `tributary-agent` and `tributary-workspace` private keys come from manifest conversions, `trib-mcp` needs the `Tributary MCP` App's client secret for `token/scoped`, and none of the three has a provisioning or `Rotate` path. `agents.<host>` (AGENT-LAYER 254) cannot be a Custom Domain of a Worker in the agents account while the zone lives in the install account, since "You cannot create a Custom Domain ... on a zone you do not own" (https://developers.cloudflare.com/workers/configuration/routing/custom-domains/) [inferred].

Fix.
- Run the insights consumer in the site scripts. PrSync's apply enqueues one message per head push, and a consumer with the shard binding calls `predict` and posts the check run through the edge as the App. Give it eight failure rows and a deployment row.
- Route `mcp.<host>` through `trib-front`, which sets the `trib-mcp` and edge overrides in one dictionary header, and have `trib-mcp` forward that header on its edge calls. Ship the `trib-front` change as a `FrontRoll`.
- Add `tributary agents init --account $G`, which runs the manifest flows, writes the keys into the agents account's Secrets Store, and registers a webhook hostname the agents account owns. Run a `trib-ops` instance there that executes `ControllerRoll` and `ControllerPointer` intents read over HTTPS from the install's `Release` group, and add `Rotate` for the App keys and the client secret.

### 8. Minor. The hot inline cap gives up R2-outage pushes it does not need to

Files: `cloudflare/DESIGN.md` lines 256-259, 618-619, 899. `cloudflare/AGENT-LAYER.md` lines 39-41, 142, 216.

Failure path. During an R2-wide write outage like 2025-02-06 (59 minutes) or 2025-03-21 (67 minutes), a hot group commits only pushes up to 256 KiB, so human and agent pushes between 256 KiB and 1 MiB to the monorepo fail where the base kept them committing (DESIGN 619). A1's own worst case, every push at 1 MiB, needs 4.17 MB/s against 15 MB/s on enam and 9 MB/s on apac (AGENT-LAYER 142) [measured, `--a1`], so the cap matters only for bursts, which admission already paces. The base tradeoff (DESIGN 899) still says pushes "over 1 MiB" fail.

Fix. Apply `INLINE_MAX_HOT` only while `PackStore::place` succeeds. When placement fails or a bucket's readiness probe is red, inline up to `INLINE_MAX`, and let finding 1's byte bound archive the extra bytes after recovery. Update DESIGN 619 and 899.

### 9. Minor. The cost totals are not upper bounds

Files: `cloudflare/cost_agents.py` lines 41, 55, 68, 75, 140-149, 196. `cloudflare/AGENT-LAYER.md` lines 12, 144, 324-376. `cloudflare/DESIGN.md` lines 795, 799. `cloudflare/cost.py` lines 49, 195.

Failure path and evidence.
- Two push rates. Sizing uses A1's push every 2 minutes (`PUSH_EVERY_S = 120`, ten per 20-minute task), and every monthly line uses five per task (`PUSHES = 5`). TASK.md states both. With ten, the 2,000-developer total is $2,328 [measured, script patched in memory, file unchanged].
- The predictor line, called "an upper bound" (AGENT-LAYER 376), prices 4 pairs against a budget of 8. With 8 pairs and ten pushes the total is $2,547 and the predictor line alone is $438 [measured, same method].
- Two miss models. The CPU term counts one clone-cache miss per new tip, "which includes agent CI's first clone of each fresh tip" (AGENT-LAYER 144), while egress gives agent CI jobs and clones the base's 70% hit rate (`CLONE_HIT = cf.CI_HIT`, line 55), which cost.py derives from "about 3.75 CI checkouts per pushed SHA" (cost.py 49). Agent tasks have 3 CI jobs for 5 pushes. If every agent CI checkout misses, agent CI egress rises from 12.8 TB to 43.8 TB a month, about $860 more at the 50/30/20 leader split [inferred]. The same consistent model cuts the CPU miss term from 4.17 to about 1.5 vCPU, k to 4, and the fan-out line by $165 [inferred].
- Agent-tip clone-cache entries in the first warm bucket are not priced, because cost.py prices the clone cache as a fixed 10% of git bytes (cost.py 195). That is about $57 a month at 2,000 developers [inferred].
- Net, the 2,000-developer figure is about $2,500 to $3,400, 27% to 36% of the base, not $1,985 and 21%. The 50- and 300-developer totals move to $25 and $338 before the egress correction [measured for the patched runs, inferred for the range].

Fix. Use one push rate for monthly lines and label the A1 sizing as a separate peak case, derive the agent CI hit rate from the same miss model as the CPU term, price 8 pairs or drop "upper bound", add the first-bucket storage, and update both cost tables, the summary (AGENT-LAYER 12), and DESIGN 795 and 799.

### 10. Minor. "Why not Artifacts" is right, but two of its reasons are not

Files: `cloudflare/AGENT-LAYER.md` lines 10, 18-20, 423. `cloudflare/DESIGN.md` line 925. `arena/cloudflare-agents/SYNTHESIS.md`, "Why not Artifacts".

Failure path and evidence.
- "No off-Cloudflare runtime was found for BYOC or self-hosted parity" (AGENT-LAYER 18) does not apply to this design, where BYOC and self-hosted both run in a Cloudflare account the customer owns (DESIGN, Deployment modes; TASK.md, Fixed ground), and Artifacts needs only Workers Paid (`research/artifacts-recheck.md` section 1) [measured]. Only workerd evaluation and the lifeboat lack it [inferred].
- "Import takes only public HTTPS remotes" (AGENT-LAYER 19, 423) is answered by seeding through push, since `create()` returns a remote and an initial token and v0 and v1 push is accepted (recheck section 2; judge, "Is why not Artifacts answered correctly") [measured from those files].
- Whole-repository tokens with no veto (AGENT-LAYER 20) do not matter for a per-task scratch repository, whose only consumer is the bring-back through Tributary's receive path, where `decide` runs. They would matter only if PR heads lived there, which the flip condition already excludes [inferred].
- The reasons that decide it hold. The 1 GB cap rules out the 5 GB monorepo, a repository is "a single logical instance" whose status degraded with Durable Objects twice, it is beta with no SLA, prediction needs branches inside the network's shard, and same-network forks already share packs (recheck sections 1 and 3; DESIGN journey 2) [measured from those files]. On uptime first, only a role off every human path can pass, the best-of-N scratch tier, because its failure delays agent attempts and nothing else. GA under an SLA is then an agent-reliability preference, not an uptime condition [inferred].

Fix. Drop the BYOC-parity and import reasons from AGENT-LAYER 10, 18-19, and 423 and DESIGN 925, restate the safety bullet as mattering only for PR heads, and add to the flip condition that the scratch tier falls back to a workspace fork on any Artifacts error.

## Checked and found sound

- The A1 arithmetic reproduces from `cloudflare/cost.py` constants: 24.7 commands/s, about 302 leader requests/s, 13.1 vCPU of git CPU, and 125 vCPU for eager merges. The 300 commands/s gate leaves about 10 times headroom at 500 agents even after adding a test-merge record per head push [measured, `--a1`; inferred for the addition].
- The new fold rows do not threaten group throughput. Two rows per push on about 10 entries a second is small next to the base's seven rows per entry per member, and the `PathSet` scans at 4.17/s are indexed reads on the leader [inferred]. Their risk is storage (finding 1).
- `trib-mcp` meets the 2026-07-28 MUSTs: RFC 9728 metadata, audience-bound tokens, CIMD instead of dynamic registration, no token passthrough, host-enforced narrowing through `POST /applications/{client_id}/token/scoped`, and the RFC 9207 `iss` parameter [measured against the judge's spec check and the layer's A7].
- No new component binds into a group member, and the controller reaches the install only over HTTPS as a GitHub App, so an agents-account outage, provider-wide or control-plane, stops Tributary-run agents and nothing on the host [measured, AGENT-LAYER Shape and A8].
- workerd defines a `workerLoader` binding (`src/workerd/server/workerd.capnp` line 516 on `main`), so the layer's [unverified] "Code Mode needs a Worker Loader in workerd" resolves to available. Only `openApiMcpServer()` on SDK v2 stays open [measured].
