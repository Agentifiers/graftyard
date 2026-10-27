# User walk: how agent-friendly is the Tributary agent layer?

Inputs: `cloudflare/AGENT-LAYER.md` (cited as AL), `cloudflare/DESIGN.md` (cited as D), `cloudflare/cost_agents.py`, `research/agents.md` (cited as RA), `research/artifacts-recheck.md`, `review/cloudflare-agents-judge.md`, and https://blog.cloudflare.com/next-git-platform-on-cloudflare. Three users were walked using only what AL and D say: (1) a coding agent on an issue in the A1 monorepo, run three ways (Tributary-run, a vendor App, and a remote MCP client); (2) a maintainer reviewing 40 agent PRs a day; (3) a platform engineer turning agents on for 300 developers in BYOC. Labels: [measured] means I read it in the file or on the cited page on 2026-10-26, or ran the arithmetic from the file's own constants. [inferred] means my reasoning from those facts. [guess] means judgment with no direct evidence.

## Verdict

**Holds with changes.** The pick is right. Agents should be GHES clients, Artifacts should play no role, and no coordination object is needed. I checked the Artifacts answer against the recheck and agree with it. The 1 GB cap, public-only import, whole-repository tokens, and missing veto all still hold [measured, `research/artifacts-recheck.md` sections 1 to 3]. The one role the flip condition keeps for Artifacts, best-of-N scratch attempts, does not need Artifacts either. A workspace fork prefix `tributary/<task>/` can hold N attempt branches that share packs [inferred, AL 22, 120 to 122].

As written, though, the layer is agent-safe more than agent-friendly. Walked as an agent, I could not get CI results without a human click on every push. Under managed rate limits my fleet hit the installation's hourly budget about three times over. I learned about other agents on my files only after my PR existed, and only through a `neutral` check run. When the group was full, I got an opaque HTTP 503 after waiting 30 s.

Walked as the maintainer, I had no cross-PR triage, no grouping of duplicate attempts, and an agent gate that is not bound to the approved head. Walked as the platform engineer, the $258 a month layer total left out the two costs I would actually pay, Sandbox compute and model tokens, and nothing capped either one.

Against the blog's asks:
- Concurrency is well covered.
- "Knowing what others work on" is late and PR-bound.
- "Conflicting changes" is advisory and reports `Stale` for 60% of pairs at peak.
- "Review everything" has gates but no triage.
- "Why" is strong for Tributary-run agents and thin for vendors and for agents that read it through an API.

Every fix below is additive or opt-in and keeps GHES 3.22 responses byte-identical for unmodified clients. The blog also says "We aren't looking for GitHub as it exists today with agents added on top" [measured]. The GHES mandate rules that out by construction, so the agent-native value has to come from the opt-in surfaces, which need to be finished.

## Findings

### 1. Blocker. Agents cannot see CI on their own pushes, and the numbers assume they can

- **Where:** AL 52 to 53, 168, 173, 243; D 265 to 266; contrast AL 140.
- **Who hits it.** The agent, Tributary-run in-repo or as a confined vendor App, at the "push, read checks, fix" step. Every push's Actions runs sit at `action_required` until a writer approves them. In fork mode, fork-PR workflows run only under the 3.22 fork policy, with a read-only token and no secrets (AL 168; judge, GHES table) [measured].
- **Failure path:**
  - The agent pushes, polls `GET /commits/{sha}/check-runs`, and sees only `action_required`. Nothing tells it whether its tests pass, so it either stops and opens an unverified draft or loops.
  - The maintainer must click "Approve and run workflows" on every push. At 40 PRs a day and 5 pushes per task (`cost_agents.py` line 41) that is 200 clicks a day. Across 300 developers at 3 tasks a day it is 4,500 run approvals a day if every agent is confined [inferred, arithmetic].
  - A1 meanwhile counts "RequiredCheck from agent CI 1.25/s" (AL 140) and agent CI clone misses (AL 144), so the load model assumes CI runs unattended. Safety and scale assume opposite behavior.
- **Evidence.** GitHub had the same problem with Copilot and fixed it. Its changelog says the approval "slows down the feedback loop for validating Copilot's work and finding out if tests pass", and GitHub added "a new repository setting to allow repository administrators to skip the human approval so workflows run immediately" (https://github.blog/changelog/2026-03-13-optionally-skip-approval-for-copilot-coding-agent-actions-workflows) [measured].
- **Fix.** Add `ci: Approve | AutoReadOnly | Auto` to the repository's agent policy (the same policy that already overrides `PROTECTED_PATHS`, AL 52).
  - The default stays `Approve`.
  - `AutoReadOnly` runs agent-push workflows at once with fork-PR semantics: a read-only `GITHUB_TOKEN`, no secrets, and no environments. Only jobs that reference secrets wait as `action_required`.
  - State in A1 which mode the 1.25/s assumes.
  - Have the controller run the repository's test command inside the Sandbox before each push, so a Tributary-run agent has a signal even under `Approve`.
  - All of this is opt-in, and every status shape is already in 3.22.

### 2. Major. In managed mode, GitHub's own rate limits stop the A1 fleet

- **Where:** AL 239, 319, 322; AL 140 (check runs 8.33/s); AL 273.
- **Who hits it.** The agent and the platform engineer. Managed mode uses "GitHub numbers on" (AL 319). On github.com an installation token gets "5,000 requests per hour", up to "12,500" or "15,000" on Enterprise Cloud, plus "no more than 80 content-generating requests per minute and no more than 500 content-generating requests per hour" (https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api) [measured].
- **Failure path:**
  - In A1, 500 agents at 60 REST calls per 20-minute task make 90,000 calls an hour. Half of them are Tributary-run, so one `tributary-agent` installation makes 45,000 an hour, 3 to 3.6 times the cap [inferred, arithmetic from `cost_agents.py` lines 41, 43, 56].
  - `tributary-insights` posts 8.33 check runs a second, 500 a minute, from one App (AL 140). That is 6 times the 80-a-minute content limit, and it exhausts the 500-an-hour limit in the first minute if check runs count as content [inferred].
  - Agents then get 403 or 429, and the overlap and provenance signals stop exactly when the fleet is busiest. The Scale table (AL 212 to 226) has no row for per-actor API budgets.
- **Fix:**
  - Key `x-ratelimit-*` for `AgentApp` installations per minted token, which the outbound handler already mints per task (AL 125 to 127), rather than per installation. The headers keep their names and shapes, and only the numbers differ, which GHES already allows because admins set them (https://docs.github.com/en/enterprise-server@3.22/rest/using-the-rest-api/rate-limits-for-the-rest-api) [measured].
  - Exempt Tributary's first-party `tributary-insights` and agent-gate writes from content limits, since they are host-internal writes [inferred].
  - Add a Scale row: per-actor requests per hour for the fleet against the managed limit.

### 3. Major. Lazy test merges turn eager again when agents poll `mergeable`

- **Where:** AL 47 to 49, 144, 221; D 260 to 262, 497, 519, 849.
- **Who hits it.** The agent at "is my PR still mergeable after main moved?" In practice that means `gh pr view --json mergeable`, a controller deciding whether to ask for a rebase (AL 181), or the PR page. `PrSync` computes the merge "on GET or CI request" (D 260) [measured].
- **Failure path:**
  - If each of 500 agents reads its PR once per base move, 10 times a minute, that costs 500 x 10 / 60 x 1.5 vCPU-s = 125 vCPU, the same eager load the change was meant to remove [inferred, arithmetic with `MERGE_VCPU_S` 1.5].
  - Polling once a minute still adds 12.5 vCPU on top of the network's 13.1. That needs 25.6 vCPU against the 20 that k = 5 gives, so the warm site that must carry failover is over its 66% plan [inferred].
  - The Milestone 0 row (D 849) tests one GET returning within 10 s, not 500 pollers.
- **Fix:**
  - Give GET-triggered merges a compiled per-network budget like the predictor's (`PREDICT_CPU`, AL 91), at most one compute per PR per base tip.
  - Order the work: queued, approved, and human-requested PRs first.
  - Answer `mergeable: null` while over budget. That is already the documented 3.22 behavior [measured, judge GHES table].
  - Change the D 849 row to "500 PRs, every author polling every 30 s, test-merge vCPU at most 8".

### 4. Major. Overlap signals arrive after the work is done and not where agents look

- **Where:** AL 81 to 93, 110, 125 to 128, 176 to 182, 410.
- **Who hits it.** The agent at "who else is working on these files?"
- **Failure path:**
  - `PathSet` is written only when `refs/pull/N/head` moves (AL 81), so nothing is known about a branch until a PR exists. Vendor agents that open the PR at the end, which is the common shape for cloud sessions and code review Apps (RA 2.1, 2.5), are invisible to everyone during the whole task.
  - The only pre-work signal is assignment, which is per issue, not per file (AL 178).
  - A Tributary-run agent cannot use the MCP `coordination.overlaps` tool. Its Sandbox holds no user token, and `McpToken` wraps only a consented `ghu_` token (AL 103 to 109, 125 to 128) [inferred]. Off-the-shelf harnesses also do not send `AGENT_MEDIA_TYPE`, so that agent sees overlaps only as a `neutral` check run after its push.
  - `neutral` checks are what agents that triage "failing checks" skip [guess].
  - The arguments of `coordination.overlaps` are not defined anywhere, so it is unclear whether it can be asked about paths before a push.
  - A5 step 4 (AL 181) has X merge first and then tells "X's agent to rebase", which reads as reversed.
- **Fix.** All of these are additive:
  - Write `PathSet` on any push under an `AgentApp`'s prefix, keyed by ref, not by PR. Post the overlap check run on the head SHA so it appears before a PR exists.
  - Add `PathSet.planned`, parsed from a fenced `tributary-plan` block in a draft PR body. The controller opens that draft at task start with an empty commit, from the plan it already has (AL 130).
  - Define `coordination.overlaps({paths?, pr?, ref?})` returning `[{pr, ref, app, session, requester, draft, last_push, shared_paths, verdict}]`.
  - Have the controller subscribe to `check_run` for its PRs and inject the overlap and conflict verdicts into the running harness as a message.
  - Fix the X and Y order in A5.

### 5. Major. Agents that are not flagged Apps get human priority, no refusals, and no attribution

- **Where:** AL 68 to 71, 153, 196, 237 to 238; D 252, 263 to 268.
- **Who hits it.** The agent as a local CLI, the MCP client, and the maintainer reading audit logs.
  - Admission class, refusals, and `actor_is_agent` all come from `AgentApp` (AL 68 to 69; D 252).
  - Claude Code CLI, Codex CLI, Copilot CLI, Aider, and Devin's PAT path all act as the user through `git` and `gh` (RA section 2 table, 2.6, 2.7) [measured]. Cursor allows "200 workers per user" (RA 3.5) [measured].
  - The `Tributary MCP` App is not listed as an `AgentKind` (AL 71), so MCP writes are attributed to the user as well [inferred].
- **Failure path:**
  - A developer running 8 local agents gets `AdmitClass::Human`, ahead of every flagged fleet, and bypasses `AGENT_SHARE`.
  - An MCP client with write consent can do anything the user can, including approve and merge, with `actor_is_agent` false.
  - The maintainer's audit stream undercounts agent work.
- **Fix:**
  - Classify a push as `AdmitClass::Agent` when `validate` finds an agent trailer (`Claude-Session`, `Amp-Thread-ID`, `Co-Authored-By` with a known agent address, `Tributary-Session`). The `Provenance` already exists before `decide` (D 271 to 272).
  - Accept `X-Tributary-Agent-Session` on every write and on git over `http.extraHeader`, not only on PR create (AL 73).
  - Make the `Tributary MCP` App an `AgentApp` with `requester = sub`. Its writes then fill `actor_is_agent` and `agent_session_id` and use the agent admission class. Approve and merge are refused unless the consent page's write scope explicitly includes them.
  - All of these are invisible to clients that do not opt in.

### 6. Major. The agent gate is not bound to the approved head, and the protected paths miss common agent files

- **Where:** AL 56 to 57, 75 to 77, 168, 244; D 269 to 270.
- **Who hits it.** The maintainer merging safely.
- **Failure path:**
  - `AgentGate { pr, satisfied_by }` is per PR, while a commit status lives on a SHA. Nothing says a new agent push after the approval resets the gate.
  - If the App re-posts success on the new head because `satisfied_by` is set, an agent, or an injection steering it, can push after approval and merge through the queue [inferred].
  - `PROTECTED_PATHS` omits `CLAUDE.md` and `GEMINI.md`, which RA 1.20 lists as instruction files [measured]. It also omits harness configuration such as `.claude/**` and `.cursor/**` [guess on their contents]. A confined agent can therefore rewrite the instructions or hooks the next human's local agent obeys.
- **Fix:**
  - Use `AgentGate { pr, head: Oid, satisfied_by }`. Post success only on the SHA named by the approving review's `commit_id`, and pending on every later head.
  - Ship the 3.22 rule "require an approval from someone other than the last person to push" (RA 4.15) in the agent ruleset template.
  - Add `**/CLAUDE.md`, `**/GEMINI.md`, `.claude/**`, `.cursor/**`, and `.github/copilot-setup-steps.yml` (`.github/workflows/` already covers the latter) to `PROTECTED_PATHS`, with the per-repository override that already exists (AL 52).

### 7. Major. A maintainer with 40 agent PRs a day has no triage view, no duplicate grouping, and mostly stale conflict predictions

- **Where:** AL 92 to 93, 147, 182, 222, 297, 410 to 411.
- **Who hits it.** The maintainer.
- **What works:**
  - Per PR, they see the App as author, a `tributary/provenance` check, a session link, and an overlap check.
  - Filtering works for App authors (`author:app/<slug>`) [inferred].
- **Failure path:**
  - No surface shows the repository's open agent PRs grouped by overlap cluster or by the issue they close. Two or three agents assigned to the same issue (GitHub's own flow is "Assign an issue to Claude, Codex, Copilot, or all three", RA 1.18) [measured] arrive as unrelated PRs. "No lease stops duplicate work. That is accepted" (AL 182).
  - The blog asks to "compare multiple changes at the same time, and decide which one should ship" [measured], and nothing does that.
  - At peak, 60% of predicted pairs report `Stale` (AL 147, 222), with no priority for PRs the maintainer is about to merge.
  - PRs made by local CLI agents carry only a trailer (finding 5).
- **Fix:**
  - With `tributary-insights` installed, apply ordinary labels `agent:<app>` and `overlap:<cluster-id>`, so every GHES client can filter and the change uses only existing features.
  - List "also addresses #42: #77 (cursor), #81 (tributary-agent)" in the overlap check, from closing references.
  - Add a `/_tributary/repos/{owner}/{repo}/agents` page beside the session pages (AL 251) that shows clusters, verdicts, CI state, and a two-PR compare.
  - Run predictions first for PRs that are approved, queued, or opened by a human request, so stale verdicts fall on PRs nobody is looking at.

### 8. Major. Overload and refusal errors do not tell an agent what to do next

- **Where:** AL 34 to 35, 148, 153, 215, 241; D 251 to 252, 614, 847.
- **Who hits it.** The agent during a rebase burst or at 5,000 agents.
- **Failure path:**
  - Admission waits up to 30 s and then answers 503 with `Retry-After` (D 251 to 252).
  - For `git push` over smart HTTP, a non-200 status means git prints only that the RPC failed with HTTP 503. It shows no reason and, as far as I know, does not honor `Retry-After` [inferred, not re-verified for git 2.5x]. The pack has already been uploaded, so a retry uploads it again.
  - For REST, GitHub's documented throttle shapes are 403 or 429 with a secondary-rate-limit message and `retry-after` [measured, rate-limit page]. A 503 reads as an outage.
  - Refusals use "GHES's existing 403 and push-rejection shapes" (AL 153) with no stated text. Agents recover from ruleset rejections only by reading the message: "Claude will read the error and pick a better branch name" (RA 5.6) [measured].
- **Fix:**
  - For git, answer HTTP 200 with report-status `ng <ref> agent admission full, retry after <n>s`, plus a `remote:` sideband line. git prints that as a remote rejection.
  - For REST, answer 429 with `retry-after` and a message containing "secondary rate limit", which existing clients and `ToolError.retry_after_s` (AL 113) already handle.
  - Define refusal texts that name the allowed prefix and the protected path. For example: `refused: tributary-agent may push only refs/heads/tributary/<task>/**`.
  - Change D 847's pass condition to match these shapes.

### 9. Major. The platform engineer's bill and setup are understated, and nothing caps agent spend

- **Where:** AL 12, 203, 315, 326, 340 to 347, 428; `cost_agents.py` lines 12 to 13, 216.
- **Who hits it.** The platform engineer enabling 300 developers in BYOC. What they must install and configure, from AL:
  - A customer-owned agents account with Workers Paid, Sandbox quota, and Secrets Store holding App and model keys (AL 203).
  - Four first-party Apps: `tributary-agent`, `tributary-workspace`, `tributary-insights`, and `Tributary MCP` (AL 92, 163, 195).
  - An `<org>-agents` workspace org per org with private forks allowed, and the 3.22 fork-PR workflow policy turned on so fork CI runs at all.
  - `AgentApp` flags and `confine` prefixes per vendor App in site admin.
  - `tributary-insights` installed "per repository" (AL 92).
  - A package allowlist, per-org concurrency caps, and `mcp.` and `agents.` hostnames in their zone.
  - No checklist or install step lists these together.
- **Failure path, cost:**
  - The layer line is $258 a month at 300 developers (AL 343), with "Runner (Sandbox) compute and model compute ... excluded" (`cost_agents.py` lines 12 to 13) [measured].
  - At the script's own workload, Tributary-run tasks are 300 x 3 x 0.5 x 30.4 = 13,680 a month. A 20-minute `standard-2` Sandbox (1 vCPU, 6 GiB, 12 GB; https://developers.cloudflare.com/containers/platform/pricing/) costs about $0.031 a task at 50% CPU, so about $424 a month [inferred, arithmetic].
  - Model tokens at $1 to $3 a task [guess] are $13,700 to $41,000 a month, 50 to 160 times the layer line.
  - The only stand-in for AI Gateway budgets is per-org concurrency caps (AL 428). They bound rate, not spend. No task wall-clock cap is stated, while Copilot caps sessions at 59 minutes (RA 1.6) [measured].
- **Fix:**
  - Add Sandbox compute as a cost row, and add a model-token row with the per-task assumption stated.
  - Have the controller's outbound handler count model usage from each response (it already attaches the model key, AL 165) and enforce per-task and per-org monthly token budgets plus a default 60-minute task cap. When a limit trips, end the task as `Failed { reason }` with an issue comment.
  - Allow `tributary-insights` to be installed on "All repositories".
  - Add an "Enable agents" checklist to Deployment modes.

### 10. Minor. "Why" is readable by people, but not cheaply by agents

- **Where:** AL 130 to 131, 186 to 189, 251.
- **Who hits it.** An agent asking why a line in `session.go` exists.
- **Failure path:**
  - GHES REST has no blame route, and Claude's hosted proxy blocks GraphQL (RA 2.2) [measured]. A REST-only agent therefore clones, runs `git blame`, and fetches each commit under `AGENT_MEDIA_TYPE` to reach a session URL.
  - The session page is HTML (AL 188), and the `SessionLog` blob format is unspecified.
  - Vendor `Claude-Session` and `Amp-Thread-ID` URLs need vendor credentials (AL 189).
  - It is also undefined whether a confined vendor App may create a PR with `draft: false`, which is how many vendor agents open PRs. Either the request is refused with no stated message, or the ready-for-review refusal is bypassed at creation.
- **Fix:**
  - Serve `/_tributary/sessions/{uuid}` as JSON under `Accept: application/json`, with a versioned `SessionLog` schema.
  - Add an MCP Provenance tool `why({path, line, ref})` that runs blame on the shard and returns commit, PR, sessions, and the issue snapshot.
  - Specify that confined third-party Apps may create non-draft PRs, since approve and merge stay refused and the gate holds, while Tributary-run Apps always create drafts.

## Checked and found sound

- "Why not Artifacts" holds on current facts: the 1 GB cap, `REMOTE_AUTH_REQUIRED` on private import, no branch-scoped tokens, and no pre-receive veto (`research/artifacts-recheck.md` sections 1 to 3) [measured]. The corrected v1-push and `filter` points in D 921 match the recheck [measured].
- Rejecting a coordination object is right for uptime. Ref-keyed `PathSet` rows on the group plus check-run webhooks (finding 4) give push-based awareness with no new Durable Object class [inferred].
- MCP authorization meets the 2026-07-28 MUSTs. It uses audience-bound tokens, no passthrough, CIMD, and host-enforced narrowing through `token/scoped`. `ToolError` carries `retry_after_s` and rate fields, which addresses the complaint in RA 3.8 [measured, AL 103 to 113, 194 to 197].
- The prompt-injection posture is sound. The trust view sits beside a byte-identical `body`, the Sandbox holds no token, egress is allowlisted, and A4 states its residual risk honestly [measured, AL 170 to 174].
- Chain-resident provenance and hidden session refs survive vault restore and lifeboat export, so "why" is not lost in a disaster (AL 189; D 851) [measured].
