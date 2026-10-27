# Agent-layer arena synthesis

## Base

Candidate A, GHES-native and subtract first. Agents are clients of the GHES 3.22 surface Tributary must serve anyway. Third-party agents arrive as GitHub Apps. Tributary-run agents are first-party Apps driven by a controller in a separate agents account, running in Sandboxes that never hold a token. The new surfaces are a stateless remote MCP server on its own hostname, an opt-in trust view beside the untouched `body`, provenance, and agent admission. Artifacts plays no role.

## Scores

| Candidate | Lead | Cross-judge |
| --- | --- | --- |
| A, GHES-native, subtract first | 50.5 | 47.0 |
| B, Artifacts as the agent workspace tier | 39.0 | 30.0 |
| C, agent-native coordination host | 49.5 | 43.0 |

The rubric's weights sum to 10.5, so its maximum is 52.5, not the 50 it states. Both score sets use the same weights, so the order is unaffected.

I scored A and C a point apart. The judge (`review/cloudflare-agents-judge.md`) recomputed the A1 git CPU from `cloudflare/cost.py` constants and found that only A counted the base's per-push test merge (6.25 vCPU of 11.5). C's 6.96 and B's 4.9 undersize the shard that must carry a site loss. That and A's smaller surface decide it.

## What every candidate found about the base

All three found that the monorepo's group carries a 500-agent fleet (16 to 19 commands a second against the 300 a second Milestone 0 gate, and about 290 to 390 leader requests a second against 1,000), and that the network's single git cache shard per site does not (11.5 vCPU against 4). These base changes follow:

1. Hot networks fan out to k shards per warm site (`fanout`, up to 8), sized with C's clone-cache model (one miss per new tip, plus agent CI on fresh tips).
2. Test merges are lazy when the base branch moves. Recomputing every open PR's merge on each base move would need about 125 vCPU, and GHES 3.22 documents `mergeable: null` while a background job runs.
3. Admission classes on each group leader (`Human`, `MergeQueue`, `Bot`, `Agent`), so an agent burst cannot delay merges or humans.
4. A smaller inline pack cap on hot groups, so an entry still holds several pushes.
5. Hot clone-cache keys written to both warm sites' buckets.

## Grafts onto A

1. From C, `ActorKind::Agent` refusals compiled into `decide` for Tributary's Apps and any App an admin flags: no approve, merge, ready-for-review, bypass, ref update outside the prefix, or change under protected paths. This confines in-repo agents, which A's fork-only design could not, and lets Tributary-run agents work in-repo where private forks are forbidden. Workspace forks stay an option.
2. From C, chain-resident provenance. Trailers and `refs/notes/ai` are parsed during `validate` and never fail a push. Session logs are blobs under a hidden ref, so the vault, lifeboat, and `RestoredTo` carry them. This replaces A's session buckets as the record of truth.
3. From C, the advisory merge-tree conflict predictor on the second warm site under a compiled CPU cap, at most 8 pairs a push, paused while that site leads. It reads A's quorum `PathSet` rows instead of a new coordination object.
4. From C, `tributary/agent-gate` on by default for Tributary-run agents' PRs only, and the burst and 5,000-agent Milestone 0 rows.
5. From B, neutral `tributary/overlap` and `tributary/provenance` check runs, so vendor agents see overlaps through the plain Checks API.
6. From B, a GET-only package proxy in the Sandbox egress allowlist, with its low-rate exfiltration risk stated.

## Rejected

- B's Artifacts workspace tier. Its measured benefit, 5x fewer canonical pushes, comes from the Lander's 10-minute cadence, which needs no Artifacts. It adds a beta store with no SLA, a commit-rewriting Lander for repositories over the cap, a seeder, and $811 a month at 2,000 developers.
- C's coordination object with leases and a live feed, C's task API, and C's ref lanes, for now. Leases are advisory either way, overlaps come from `PathSet` rows and check runs, and lanes would move only 24% of A1's commands. If lanes are ever built, they need a parent `Abort { txn }` record before any lane resolves on timeout.
- Code Mode by default. It stays a priced opt-in.

## Why not Artifacts

As the canonical store, nothing changed (`research/artifacts-recheck.md`): a 1 GB cap on new repositories, one logical instance per repository, beta with no SLA, no stated RPO or RTO, and no runtime off Cloudflare. As a workspace tier, it can't hold the 5 GB monorepo, can't import a private repository, issues tokens that cover a whole repository with no branch scope and no veto, and forks appear to copy. Tributary forks already share packs and get their own group. Two of the base's objections were weaker than written and are corrected: v1-only push is invisible to unmodified clients, and `filter` is documented as unsupported only on v1.

**Flip condition.** Artifacts earns one role, a scratch tier for best-of-N attempts that never become pull requests, when all of these hold: Milestone 0 shows forks share objects, Cloudflare raises the per-repository cap above the install's largest network in writing, and Artifacts reaches GA under an SLA. No branch a pull request points at would live there.

**Superseded on 2026-10-26.** Review item AG-ACT-23 (`review/cloudflare-agents-interrogate.md`) dropped two reasons above: private import (seeding by push works) and runtime parity (BYOC and self-hosted run in a Cloudflare account the customer owns). It also changed the flip condition. GA under an SLA is now a preference, not a requirement, and the scratch tier falls back to a workspace fork on any Artifacts error. `cloudflare/AGENT-LAYER.md`, "Artifacts' role", is the current statement.

**Flip condition for the base.** Start from C if the operator makes leases, a live who-is-working-on-what feed, and a task API must-haves for the first release.

## Verification

All three `cost_agents.py` scripts ran. `scripts/check_cloudflare.py --layer` passes candidate A and fails B and C on table format only. The integrated `cloudflare/AGENT-LAYER.md` is checked with `--layer`, and `cloudflare/DESIGN.md` with `--final`.
