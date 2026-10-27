# Cloudflare arena rubric

Judge-only. Candidates never see this file. Score each candidate and the AWS design (`DESIGN.md`) from 1 to 5 on each criterion, multiply by the weight, and sum. The maximum is 50. The AWS design is scored on the same rubric so the operator can compare the two platforms. It is not a candidate and cannot be picked.

The operator's goals are, in order, less downtime and better scale than GitHub, with GHES 3.22 wire compatibility and managed, BYOC, and self-hosted modes (`FRAME.md`). The Cloudflare design may use only Cloudflare products (`FRAME.md`, "Operator decision, 2026-10-26, Cloudflare").

## Criteria

1. **Downtime, weight 3.** Score the GitHub incident classes in `research/uptime.md` findings 40 and 43 to 49, weighted by incident-minutes, and the Cloudflare incidents in `research/cloudflare-uptime.md`. A Cloudflare-only design cannot fail over to another provider, so score how much keeps working during a Cloudflare-wide event, a Cloudflare control-plane outage, a data center loss, and a region loss, and whether the design states each honestly. 5 means every database-class GitHub incident is removed or contained, an acknowledged push survives a region loss, and the provider-wide exposure is stated with what still works. 1 means one Cloudflare product's failure stops core surfaces everywhere and the design does not say so.
2. **Scale, weight 2.** The busiest single repository it carries, with the limiter named and labeled, against the per-object, per-request, and per-account limits in `research/cloudflare.md`. Clones of large repositories through Worker and Container limits. Fleet scale toward 3.35 billion pushes a month (`research/git-storage.md` finding 5). Hot objects and how they spread.
3. **Correctness, weight 2.** No acknowledged push is lost. Merges are atomic for same-repository and fork pull requests. Reads see the caller's own writes on the journeys clients rely on. Revocation has a stated bound. Every weakening is stated.
4. **Build and operate risk, weight 2.** How much novel protocol and platform code the team writes, how much rests on beta or private-beta products, whether git runs as upstream git or a reimplementation, and whether Milestone 0 can falsify the design cheaply.
5. **Install fit, weight 0.5.** Managed, BYOC, and self-hosted forms per component, and what a customer must provide, including whether a self-hosted mode exists at all on Cloudflare-only technology.
6. **Cost, weight 0.5.** Monthly list cost at 50, 300, and 2,000 developers from `research/prices.json` `cf.*` entries, against `research/costs.md`.

## Fact checks

Check every Cloudflare fact a score depends on against the cited source or the Cloudflare documentation, especially Durable Objects durability and placement, Workers request limits, Containers limits and status, R2 consistency and conditional writes, and Queues delivery. A false fact that a criterion rests on caps that criterion at 2 until fixed. List each check as confirmed, refuted, or unverified, with the URL.

## Output

A table of scores per criterion per design, weighted totals, the fact checks, the most serious flaw in each design, what each loser does better than the winner, and a recommended base with the single condition that would flip it. Add one paragraph on where Cloudflare beats AWS for this product and where it loses, with evidence.
