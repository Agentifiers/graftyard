# Judge: agent layer for Tributary on Cloudflare

Inputs: `arena/cloudflare-agents/candidate-a/`, `candidate-b/`, `candidate-c/` (each `DESIGN.md` and `cost_agents.py`), scored against `review/cloudflare-agents-rubric.md`. Labels: [measured] means I ran it, read it in the file, or read it on the cited doc page on 2026-10-26. [inferred] means my reasoning from measured facts. [guess] means a judgment with no direct evidence.

## Verdict

Build on **candidate A** (GHES-native, subtract first). It scores 47.0 of 50, against 43.0 for C and 30.0 for B [measured, arithmetic below]. A adds the least code and no new store. It is the only candidate whose A1 git CPU count includes the base's per-push test merge, and all five GHES 3.22 facts it rests on hold on the 3.22 docs [measured]. Graft C's compiled agent rules in `decide`, its merge-tree conflict predictor, its chain-resident provenance, and its clone-cache model. Graft B's overlap and provenance check runs and its package-proxy egress. Do not adopt B's Artifacts workspace tier. Its main measured benefit, 5x fewer canonical pushes, comes from the Lander's 10-minute cadence, which needs no Artifacts [inferred].

**Flip condition.** Start from C instead if the operator makes leases, a live who-is-working-on-what feed, and a task API must-haves for the first release. Those features are the core of C and would be most of what A has to graft. In rubric terms, the pick flips when Agent usefulness weighs 3.5 or more instead of 1.5 [measured, arithmetic: A = 42.5 + 3w, C = 35.5 + 5w].

## Scores

| Criterion | Weight | A | B | C |
| --- | --- | --- | --- | --- |
| Uptime and safety | 3 | 5 | 3 | 4 |
| Scale under agent load | 2 | 4 | 2 | 4 |
| GHES compatibility | 2 | 5 | 4 | 4 |
| Agent usefulness | 1.5 | 3 | 3 | 5 |
| Build and operate risk | 1 | 5 | 2 | 3 |
| Install fit | 0.5 | 4 | 3 | 5 |
| Cost | 0.5 | 5 | 2 | 4 |
| **Weighted total (of 50)** | | **47.0** | **30.0** | **43.0** |

Cost scripts as run from the repository root [measured]:

| Candidate | 50 devs | 300 devs | 2,000 devs | Not in the total |
| --- | --- | --- | --- | --- |
| A | $11 | $120 | $1,112 | A1 monorepo shards $495, opt-in Code Mode $5,470 at 2,000 |
| B | $33 | $241 | $1,989 | Direct-only variant $13, $129, $1,178. A1 fan-out about $165 |
| C | $13 | $126 | $1,457 | Includes $165 of hot-network shards at 2,000 |

## Load-bearing facts checked

### Git cache shard vCPU in A1

All three agree that the network's git cache shard, not the group, is the bottleneck [measured, each script's A1 output]. Their numbers differ by more than 2x. I recomputed the A1 shard load with `cloudflare/cost.py` constants (`PUSH_VCPU_S` 0.5, `FETCH_VCPU_S` 0.3, `CI_MISS_VCPU_S` 1.0, `MERGE_VCPU_S` 1.5, `CI_HIT` 0.7) [measured].

| Term | vCPU-s/s | A counts | B counts | C counts |
| --- | --- | --- | --- | --- |
| index-pack for 4.17 pushes/s | 2.08 | yes | yes | yes, plus 0.21 for session-log pushes |
| Test merge per head push (`PrSync` moves `refs/pull/N/merge` "within seconds", base `DESIGN.md` line 480, `cloudflare/DESIGN.md` line 777) | 6.25 | yes | no | no |
| CI clone-cache misses | 1.67 (base 95% gate) to 4.17 (one miss per new tip) | 1.67 | about 2, base guess | 4.17 |
| Agent CI on fresh agent tips (1.25 jobs/s, all miss) | 1.25 | no | 0.06 (prices them at a 95% hit rate) | inside the 4.17 |
| Agent clones and fetches | 0.25 to 1.25 | 1.25 | 0.25 | 0.25 |
| Queue merges | 0.25 | yes | no | yes |
| **Stated total** | | **11.5** | **4.9** Direct | **6.96** |

- A's arithmetic reproduces exactly (2.08 + 6.25 + 1.25 + 1.65 + 0.25) [measured]. Counting agent CI on fresh tips gives 11.75, and C's one-miss-per-new-tip model gives 14.7 [measured, arithmetic]. A's k = 4 per warm site (16 vCPU) still carries that, at about 1.1x after a site loss [inferred].
- B's 4.9 is below the floor of 5.5 vCPU that holds even with no test merges [measured, arithmetic]. With the base's per-push test merge, Direct mode needs about 12 vCPU, so B's k = 2 (8 vCPU) at "61%" does not hold [inferred]. B's Milestone 0 row (shard CPU at most 70%) would catch it.
- C's miss model is the most physical, but it omits the test merge. With it, home work is about 13.2 vCPU-s/s and needs K = 5 at C's 70% target, not 3 [inferred]. C's predictor uses its own 0.2 vCPU-s per `merge-tree --name-only`. At the base's 1.5 for a merge it would be 25 vCPU on the failover site [measured, arithmetic]. The compiled 50% CPU cap and 8-pairs-per-push budget contain that, so the predictor degrades instead of overloading [inferred].

### A's eager test merges at about 125 vCPU

500 open PRs x 10 base moves a minute x 1.5 vCPU-s = 125 vCPU [measured, arithmetic and `--a1` output]. The premise is real, because the base keeps `refs/pull/N/merge` current through `PrSync`. The magnitude rests on `MERGE_VCPU_S` = 1.5, a base [guess] that includes the tree-diff check. At C's 0.2 vCPU-s it would still be 16.7 vCPU, four `standard-4` shards [measured, arithmetic]. So lazy test merges are needed at any plausible constant [inferred]. GHES 3.22 documents that "If the value is null, then GitHub Enterprise Server has started a background job to compute the mergeability" (https://docs.github.com/en/enterprise-server@3.22/rest/pulls/pulls) [measured]. A's `mergeable: null` tag as wire-compatible holds.

### C's ref lanes and two-phase atomic push

- The 24% relief figure holds. Lanes move 4.17 agent pushes and 0.42 session-log pushes of 18.9 commands [measured, `--detail`]. PRs stay on the parent, so lanes are rightly deferred.
- The snapshot rule ("read lanes first, then the parent, and show a prepared ref only if the parent holds `Decide { commit: true }`") is consistent for a single transaction [inferred].
- The abort path has a hole. A lane past `until` "reads the parent's decision through `fresh`", but no parent abort record is defined. If a lane finds no decision and resolves abort while a delayed parent `Decide { commit: true }` then commits, the push is torn. The read rule would also show the lane ref at its new value [inferred]. The fix is a parent-side `Abort { txn }` command that a lane commits through the parent before resolving. Lanes are designed but not built, and Milestone 0 row 15 tests torn snapshots, so this does not move C's score below 4 on Scale.

### B's Lander parent rewrite for snapshot baselines

- The trees land unchanged, as B says. Every landed commit gets a new id, which B accepts [inferred]. Incremental lands work only if the rewrite is deterministic (same message, author, committer, and mapped parent give the same id). B does not say so, and the `Land` type carries no commit map [inferred].
- A5 contradicts the Lander. B's agent "rebases in its fork on the refreshed baseline, and lands again", but `Land.expect` is "never a force update", so a rebased branch fails `NotDescendant` [inferred]. Merging the new baseline works. Rebasing does not.
- Synthetic snapshot chains accumulate every blob version they sync, so a snapshot baseline grows toward the 1 GB cap over time and needs periodic re-rooting, which B does not mention [inferred].
- B says rewritten commits "lose agent signatures, which Tributary re-signs server-side (AG 149)". AG 5.9 says "GHES 3.22 can sign web commits server-side". That covers web commits, not pushed ones, and a host cannot re-sign a pushed commit without changing its id again [measured, `research/agents.md` line 149; inferred]. No score rests on it.

### GHES 3.22 claims (docs.github.com, enterprise-server@3.22, read 2026-10-26)

| Claim | Who relies on it | Result |
| --- | --- | --- |
| Push rulesets apply to forks. "Push rules apply to the entire fork network ... the same push rulesets will also apply to your forked repository" (`/pull-requests/reference/forks`) | A (workspace forks), B and C (path rules) | Holds [measured] |
| App `workflows` permission. "If your app specifically needs to access or edit Actions files in the `.github/workflows` directory, request the 'Workflows' repository permission" (`/apps/.../choosing-permissions-for-a-github-app`). The permissions-required page has a "Workflows" section | A | Holds [measured]. That git push enforces it on 3.22 is [inferred] from the github.com behavior. A labels it unverified |
| Author self-approval. "Pull request authors cannot approve their own pull requests" (`/.../approving-a-pull-request-with-required-reviews`) | A states it, B marks it unverified, C enforces it in `decide` anyway | Holds [measured] |
| `mergeable: null` while a background job computes | A | Holds [measured] |
| `POST /applications/{client_id}/token/scoped`. "Use a non-scoped user access token to create a repository-scoped and/or permission-scoped user access token" (`/rest/apps/apps`), Basic auth with client id and secret | A (MCP narrowing) | Holds [measured] |
| Fork PR workflow policy. "Require approval for fork pull request workflows" and "Run workflows from fork pull requests ... read-only permission, and with no access to secrets" for private and internal repositories | A (A3, A4) | Holds [measured] |
| Private forks "inherit the permissions structure of the upstream repository" (team permissions only) | A | Holds [measured]. That a second fork in the same org is refused is still [unverified], as A says |

No candidate states a refuted GHES fact that a score rests on.

### MCP authorization against the 2026-07-28 spec

The spec (https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization) requires OAuth 2.1, RFC 9728 metadata, and audience validation per RFC 8707. It says "MCP servers MUST NOT accept or transit any other tokens" and that Dynamic Client Registration "is deprecated" in favor of CIMD. It recommends the RFC 9207 `iss` parameter (SHOULD) [measured].

| Point | A | B | C |
| --- | --- | --- | --- |
| Discovery | 401 with `WWW-Authenticate`, PRM at `mcp.<host>/.well-known/oauth-protected-resource` | Same, PRM at root while the resource is `/mcp`. Allowed because the header names it [inferred] | Path-inserted PRM and AS metadata on the GHES host, flagged [unverified] for collisions |
| Client id | CIMD, `redirect_uris` checked | CIMD, no DCR | CIMD |
| Token | Sealed, stateless, audience `mcp.<host>`, wraps a `ghu_` narrowed by `token/scoped` | Hashed in `Tokens` groups as `TokenKind::Mcp`, RFC 8693 exchange per call | Hashed in `Tokens` groups, narrowed per call in process |
| No passthrough | Holds. The client token never reaches `/api/v3`, and the upstream token is the server's own | Holds | Holds |
| RFC 9207 `iss` | Not mentioned | Not mentioned | Not mentioned |

All three conform on the MUSTs [inferred]. A's stateless refresh cannot detect replay by itself. It relies on GHES refresh tokens being single-use [inferred, not checked]. A gets narrowing enforced by the host, which is the strongest of the three, because it needs no new token kind in the base [inferred].

## Per-criterion reasons

**Uptime and safety (3).**
- A, 5. No new component on any base commit or read path. `trib-mcp` and the controller are REST clients, and the fold additions ride existing commands [measured, A Shape and Failure modes]. Blast radius is stated for every component and failure value. A1 survives a gray enam with numbers (83 pushes and 660 reads exposed, k = 4 on the other site) [measured, A line 175]. A4 is bounded by host-enforced GHES primitives, all verified above (App permission ceiling, fork push rulesets, missing `workflows`, fork CI without secrets).
- B, 3. The agent account has no binding into the install, and every write lands through the public receive path [measured, B Shape]. Safety bounds are good. The A8 gray incident sends 500 tasks to Direct within 120 s onto k = 2 shards that the base's own constants say are undersized by about half, so the A1 incident claim does not hold as written [inferred, shard table].
- C, 4. The strongest host safety. Compiled `decide` refusals for `ActorKind::Agent` cover approve, merge, ready, bypass, out-of-prefix, and protected paths [measured, C Shape]. `RepoCoord` stays off the path, with an `n = 1` twin, and the clone cache is double-written for gray buckets. It loses a point because it puts more on the commit path (provenance parsing in `validate`, session commands on the hot group) and its failover-site CPU omits the test merge [inferred].

**Scale under agent load (2).**
- A, 4. Per-object rates for the group (15.8/s, 19x), leader (289/s), entry bytes, `Authz`, Workflows, and Queues. `AdmitClass` bounds contention, with a burst gate. Costs are linear in tasks, which A states [measured, A Scale].
- B, 2. Its headline shard sizing (4.9 vCPU, k = 2) falls below the base-constant floor (shard table). There is no class admission on the group, only a per-network 429 above 80% shard CPU. Workspace mode does make canonical load scale with lands, not pushes [measured, B A1].
- C, 4. The most complete. It has 18.9 commands/s, 387 leader requests/s, a 5,000-agent row at 180/s, a burst row, admission by class, and lanes held behind a gate [measured, `--detail`]. It omits the test merge and has the lane abort hole.

**GHES compatibility (2).**
- A, 5. Every surface is tagged. New features sit on their own hostnames (`mcp.<host>`, `agents.<host>`), and A explains the `/{owner}` collision. Additions are opt-in through a media type. Lazy merges stay inside documented 3.22 behavior. A2 needs no client code.
- B, 4. Tags are complete and Artifacts stays out of band. Workflow approval is on by default for its own App, which copies Copilot's guardrail (AG 1.7). The misapplied server-side re-signing claim costs nothing here.
- C, 4. Its task, feed, lease, provenance, agent-view, policy, MCP, and OAuth paths sit under `/api/tributary/` and `/.well-known/*` on the GHES host itself, and whether 3.22 serves anything at those well-known paths is still open [measured, C GHES table]. Hidden `refs/tributary/sessions/*` must also stay hidden from v0 advertisement and from mirror fetches, which C does not address [inferred]. Everything else is tagged correctly.

**Agent usefulness (1.5).**
- A, 3. Path-set overlaps on the quorum, on-request `check_pair`, a trust view, session pages, and MCP. Vendor agents see overlaps only through an opt-in media type, and there is no feed or lease.
- B, 3. Neutral `tributary/overlap` and `tributary/provenance` check runs reach vendor agents through the plain Checks API. Artifacts `readFile` and fast forks help Tributary-run agents. Reviewers see work up to 10 minutes late.
- C, 5. A live feed, leases, merge-tree conflict prediction, a task API with GitHub's states, budgets, an agent merge gate, and a provenance API from chain state. It answers all four of the blog's questions.

**Build and operate risk (1).** A, 5: one Worker, one optional account, fold fields, and numeric Milestone 0 rows. B, 2: a beta product, a commit-rewriting Lander, a seeder container, two DO classes, and two git paths. C, 3: two DO classes, a Workflow, a Rust MCP server, a task API, a predictor, and provenance in `Push`. All three have numeric gates.

**Install fit (0.5).** A, 4: the same in all modes, workerd runs static MCP only, and the controller is not offered there. B, 3: the workspace tier is off in workerd, the lifeboat, and FedRAMP, and BYOC needs a fourth account. C, 5: the same script everywhere, workerd runs `RepoCoord`, provenance, and MCP, and the lifeboat serves provenance.

**Cost (0.5).** A, 5, lowest at all three sizes. C, 4, about $345 more at 2,000 developers, of which $165 is the hot shards that A leaves outside its total. B, 2: the Artifacts premium of $811 at 2,000 developers rests on unverified operation billing.

## Journeys where they differ

- **A1.** Group commands are 15.8/s (A), about 7.8/s added Direct (B), and 18.9/s (C), all under 7% of the 300/s gate. The difference is how many REST writes and check writes each counts [measured]. Shard CPU is the real split (see the table above).
- **A3.** A writes to `tributary/**` in a workspace fork in a separate org, with `contents: read` on the base. Merging is impossible by permission ceiling. B writes to a per-task Artifacts fork, and the Lander lands every 10 minutes as an App confined to its prefix by `decide`. C writes in-repo under `refs/heads/tributary/<issue>-<id>/` with `decide` refusals. A's version depends on customers allowing private forks across orgs, and fork CI runs without secrets by default [measured, A Tradeoffs].
- **A5.** A learns of overlaps from quorum `PathSet` rows, with merge-tree only on request. B uses an advisory coordinator DO, with compare every 5 minutes and check runs. C uses leases, a feed, and merge-tree against up to 8 overlapping branches within 30 s. Only C tells a vendor agent about a predicted textual conflict before merge, and only if the repository opts in.
- **A6.** A keeps session records in agents-account buckets, with a PR-body summary on the quorum. B keeps transcripts in agent-account R2 and puts notes and the PR body in Tributary. C keeps sessions as chain events and log blobs in packs, so the vault, lifeboat, and `RestoredTo` carry them. C is the only one whose "why" survives a restore.
- **A7.** All three meet the MCP MUSTs. A narrows with GHES's own `token/scoped`, B exchanges with RFC 8693, and C narrows in process.
- **A8.** A's other warm site carries 11.5 vCPU on k = 4. B falls back en masse to Direct on k = 2. C carries 6.96 on K = 3, and the uncounted test merge makes that short [inferred].

## Most serious flaw per candidate

- **A.** Tributary-run agents depend on one private fork per repository in a separate workspace org. Customers that forbid private forks cannot use them, a second fork per owner is unverified, and fork CI runs without secrets by default. In-repo agents (all vendor fleets) get no host-side prefix confinement, and vendor agents cannot see overlaps without opting in.
- **B.** The value it credits to Artifacts mostly comes from the Lander's 10-minute cadence. An outbound handler that batches Direct pushes on the same cadence would give the 5x cut with no beta store, no commit rewriting, and no $811 a month [inferred]. Its A1 shard sizing is about half what the base's constants give.
- **C.** It undercounts git CPU on the site that must carry failover. The per-push test merge is omitted, and predictor cost rests on its own 0.2 vCPU-s guess. It also carries the largest new surface, and the lane 2PC has no abort record.

## Is "why not Artifacts" answered correctly?

- **A. Correct, but incomplete on the workspace tier.** Every cited fact matches `research/artifacts-recheck.md`: the 1 GB cap, "single logical instance", no SLA, public-only import with `REMOTE_AUTH_REQUIRED`, whole-repository tokens, no veto, forks that appear to copy, and no off-Cloudflare runtime [measured]. It rejects the tier on the monorepo and on import. It does not consider B's two answers, seeding by push (`create()` returns a token, and v0/v1 push is accepted) and snapshot baselines. Its flip condition is sound: a cap above the largest repo, branch-scoped tokens or a veto, an SLA, and Milestone 0 showing shared forks and v2 `filter`.
- **B. Correct on facts, and the most faithful to the recheck's implication 2.** Its eight-row rejection table matches the recheck [measured]. Its seeding path is consistent with the recheck's binding and protocol rows. Its case for using Artifacts anyway is weak for the reason above. Its flip conditions are concrete (fork p99 over 300 s, `MEMORY_LIMIT` over 1%, event loss over 1%).
- **C. Correct, and the best argued.** It adds design-specific reasons that hold. Prediction needs every branch in the network's object store, and the write-time veto lives in `decide`. Its cost note checks out: 6,000 x 6 x 30.4 is about 1.09 million operations, or about $162 [measured, arithmetic]. Its flip condition, a best-of-N scratch tier only if forks share objects, the cap rises, and Artifacts reaches GA, is the narrowest correct role.

All three correctly say that the base's "push is v1 only" and "`filter` unsupported" objections are weaker than written (recheck implication 3) [measured].

## Graft list onto A

1. **From C, `ActorKind::Agent` refusals in `decide`** for Tributary's Apps and any App an admin flags. They refuse approve, merge, ready-for-review, bypass, ref updates outside the prefix, and `PROTECTED_PATHS`. This gives vendor fleets pushing in-repo the host confinement that A says rulesets cannot give, and it lets Tributary-run agents work in-repo where private forks are forbidden.
2. **From C, the clone-cache model and double write.** Count one miss per new tip (plus agent CI on fresh tips) in A's `fanout` sizing. Write hot-network clone-cache keys to both warm sites' buckets.
3. **From C, the advisory merge-tree predictor** on the second warm site under a compiled 50% CPU cap, paused while that site leads, with at most 8 pairs per push. Feed it from A's quorum `PathSet` rows instead of a new DO. Milestone 0 row 6 (no missed conflicts on current tips, at most 5% false, p95 at most 30 s) comes with it.
4. **From C, chain-resident provenance.** Parse trailers and `refs/notes/ai` during `validate`, and never fail a push on a parse error. Store session logs as blobs under a hidden ref so the vault and lifeboat carry them. This replaces A's agents-account buckets as the record of truth.
5. **From C, `tributary/agent-gate` on by default for Tributary-run agents' PRs only.** It requires approval by a writer who is neither the agent nor the requester, so it does not touch Renovate or Dependabot (R11). Also take C's burst row (500 pushes in 1 s) and the 5,000-agent row.
6. **From B, neutral `tributary/overlap` and `tributary/provenance` check runs.** Vendor agents then see overlaps through the plain Checks API, with no opt-in media type.
7. **From B, a GET-only package-proxy egress target** with an allowlist. A's sandbox reaches only the host and the model, so it cannot install dependencies. Note the low-rate exfiltration risk, as B does.
8. **Keep from A:** lazy test merges, `AdmitClass`, `fanout`, the stateless `trib-mcp` with `token/scoped` narrowing on its own hostname, the opt-in trust view, and "Artifacts plays no role" with C's best-of-N flip condition.
9. **If C's lanes are ever built,** add a parent `Abort { txn }` command before any lane resolves on timeout.
