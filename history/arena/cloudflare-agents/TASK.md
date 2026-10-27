# Arena: the agent layer for Tributary on Cloudflare

## Question

The operator read https://blog.cloudflare.com/next-git-platform-on-cloudflare and asked "why not artifacts? how can we take it a step further and make sure this is agent-friendly?" Design the agent layer that `cloudflare/DESIGN.md` lacks, and decide what role, if any, Cloudflare Artifacts plays in it.

## Fixed ground

- The base is `cloudflare/DESIGN.md`: three-site Raft groups of SQLite Durable Objects, packs in R2, upstream git in container shards, `trib-front`, `trib-ops`, the `Release` group, one account per install. Do not redesign the base. You may propose changes to it where agent load or agent safety demands, with numbers.
- Goals in order: uptime, zero lost acknowledged pushes, scale. GHES 3.22 wire compatibility for unmodified clients is not negotiable. Nothing may change a GHES 3.22 response, default, or path for a client that does not opt in (`research/agents.md` section 9, the "conflicting" tags).
- Only Cloudflare products from `research/cloudflare.md`, plus workerd for evaluation and the lifeboat.
- Managed, BYOC (customer's Cloudflare account), and self-hosted (customer's own Cloudflare account, no vendor token).

## Read first

`research/agents.md` (all of it), `research/artifacts-recheck.md`, `cloudflare/DESIGN.md`, `research/cloudflare.md`, `research/cloudflare-uptime.md`, `FRAME.md`. Doc pages: `https://developers.cloudflare.com/<path>/index.md`.

## Agent workload (use these numbers)

Per developer per day, on top of `scripts/cost_model.py` `ACTIVITY`: 3 agent tasks. Each task makes 5 pushes to its own branch, 60 REST calls, 2 clones (half of them partial), 1 draft pull request, 3 CI jobs, and 30 MCP tool calls if the agent uses MCP (assume half do). A task runs 20 minutes. At 2,000 developers that is 6,000 tasks a day. For journey A1 also assume one monorepo with a fleet of 500 concurrent agents.

## Journeys every candidate must trace

- **A1. Fleet on one monorepo.** 500 concurrent cloud agents each push their own branch every 2 minutes and open draft PRs, while 50 humans merge through the merge queue and CI clones 2,000 times a minute. Give per-repository write and read rates, where they land, and the headroom against the 1,000 req/s per-object limit and the base's one-entry-in-flight group commit (`research/agents.md` finding 3.2, implication 8.3).
- **A2. Third-party agent vendor.** A vendor in the Claude Code or Cursor mold installs through the GitHub App manifest flow, receives webhooks, works through REST only, and revokes its token at the end (`research/agents.md` section 2).
- **A3. Tributary-run agent on an issue.** Assigned through the 3.22 assignee and `suggestedActors` surfaces, runs in a Sandbox that never holds a git token, pushes to an exclusive branch prefix, opens a draft PR with provenance, and cannot approve or merge its own work.
- **A4. Prompt injection.** An outside contributor files an issue with a hidden HTML-comment payload, and an agent is assigned. Say what the host serves the agent, what limits the blast radius, and what cannot be prevented.
- **A5. Coordination.** Two agents start work touching the same files. How each learns of the other, and what happens at merge.
- **A6. Why.** Six months later a reviewer asks why a change was made. How the session context is found.
- **A7. Remote MCP client.** An MCP client connects, authorizes, and makes 30 tool calls. Spec version, auth flow, token audience and narrowing, and cost per session.
- **A8. Installs and incidents.** The agent layer in BYOC and self-hosted, and journey A1 during a gray regional Durable Object incident.

## Required headings (in this order)

`## Summary`, `## Artifacts' role`, `## Shape`, `## Journeys`, `## Scale`, `## GHES compatibility`, `## Failure modes`, `## Deployment modes`, `## Cost`, `## Build order`, `## Tradeoffs accepted`, `## Alternatives considered`, `## Open questions and risks`.

- **Artifacts' role** must answer "why not Artifacts" with the evidence in `research/artifacts-recheck.md`, and state the condition under which your answer flips.
- **Shape** gives types and interfaces in Rust, as the base does.
- **GHES compatibility** lists every new surface, its path or field, and its tag (wire-compatible, additive, opt-in).
- **Failure modes** covers each new component with all eight failure values from the base (`instance`, `data-center`, `region`, `provider-wide`, `control-plane`, `bad-deploy`, `bad-config`, `noisy-tenant`).
- **Cost** is a script, `cost_agents.py` in your directory, that imports `cloudflare/cost.py` and the agent workload above and prints the added monthly cost at 50, 300, and 2,000 developers, with line items. Runner and model compute are excluded, as in the base.
- **Build order** has Milestone 0 rows with numeric pass and fail values.

## Rules

- At most 450 lines. Every claim carries a citation or an [inference], [guess], or [unverified] label. Plain sentences: no em dashes, no en dashes, no curly quotes, no colon joining two clauses.
- Write only `DESIGN.md` and `cost_agents.py` in your candidate directory. Run `python3 arena/cloudflare-agents/<your dir>/cost_agents.py` from the repository root before you finish.
- Final reply, at most 10 lines: line count, cost at the three sizes, Artifacts' role in one sentence, and what you are least sure of.
