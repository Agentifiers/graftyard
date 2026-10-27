# Arena: Tributary on Cloudflare only

## What changed

`DESIGN.md` builds Tributary on AWS: per-repository chains in DynamoDB, packs in S3, node-local SQLite folds, upstream git on EC2 nodes. The operator now asks what Tributary would look like built entirely from Cloudflare technologies, with no AWS, GCP, or Azure service anywhere (`FRAME.md`, "Operator decision, 2026-10-26, Cloudflare"). The goals do not change: GHES 3.22 wire compatibility for unmodified clients, less downtime than GitHub first, better scale second, Rust services, a React web app, and managed, BYOC, and self-hosted modes as far as Cloudflare allows them.

Design the Cloudflare version from first principles (`skill://principle-redesign-from-first-principles`). Reuse anything in `DESIGN.md` that does not depend on AWS by naming the section. Replace the rest.

## Read first

1. `../../FRAME.md` in full.
2. `../../research/cloudflare.md` (products, statuses, limits, prices), `../../research/cloudflare-uptime.md` (Cloudflare incidents and failure domains), and `../../research/cloudflare-git.md` (git, runners, search, email, and installs on Cloudflare). Every Cloudflare product you use must appear in the first table of `research/cloudflare.md`, with the status given there.
3. `../../DESIGN.md`: Summary, Shape and its subsections, Availability, Deployment modes, Install operations, Build order, Tradeoffs accepted. This is the AWS design you are compared with.
4. `../../research/uptime.md` findings 40 to 49 (what takes GitHub down) and `../../research/git-storage.md` findings 1 to 31 and 44 to 49.
5. `../../research/costs.md` and the `cf.*` entries in `../../research/prices.json`.
6. `skill://architect/references/runner-prompt.md` and `skill://architect/references/design-red-flags.md`.

Search the Cloudflare documentation for any fact your design rests on that the research files do not cover, and cite what you read. Label anything else [inference] or [guess].

## Your direction

Your brief names one. Do not converge toward the others.

## Journeys to trace

For each, name every Cloudflare product and location each step touches, what is durable when the client is told "done", and what the client sees on its next read.

1. A developer atomically pushes three branches to a busy monorepo while its merge queue merges 10 pull requests a minute, bots move 50 refs a minute, and CI clones it 2,000 times a minute. Then someone clones a 5 GB repository and pushes a 2 GB pack.
2. A pull request from a fork is merged while a review and a required check land. The author then runs `gh pr view` and `gh pr list` at once.
3. A user creates an issue labeled `bug` and immediately runs `gh issue list --label bug`, then `gh api repos/o/r/issues?sort=updated`.
4. An org owner removes a member and revokes a token. The member's next push, over HTTPS and over SSH.
5. The data center, and then the whole region, holding a repository's newest writes is lost in the middle of a push.
6. Cloudflare has a global outage like the one of November 18, 2025, and separately a control-plane and API outage like the one of November 2, 2023. Say exactly what still works for a developer.
7. A bad Tributary release multiplies identity reads tenfold, and another writes a record shape the previous release cannot read.
8. A company runs Tributary in its own Cloudflare account (BYOC), and another wants it fully self-hosted. Say what each gets.

## Output

One file, `DESIGN.md`, in the directory your brief names, plus a cost script in the same directory. Hard cap 700 lines. Plain sentences per `skill://unslop`: no em dashes, no en dashes, no curly quotes. Headings in this order:

```
# <title naming your direction>
## Summary
## Usage (caller's view)
## Stack
## Shape
## Commit and read paths
## Scale
## Downtime
## Failure modes
## Deployment modes
## Cost
## Build order from zero
## Compared with the AWS design
## Tradeoffs accepted
## Alternatives considered
## Open questions and risks
```

- **Summary.** At most 12 lines: the bet, the products, and what a GitHub user and a Tributary maintainer would each notice compared with `DESIGN.md`.
- **Stack.** A table with the header `| Product | Status | Used for |`, one row per Cloudflare product the design uses, with the product name and status exactly as in `research/cloudflare.md`.
- **Shape.** Rust type sketches for every stored record and the key signatures, with the invariants they encode. Trace each dominant access pattern.
- **Scale.** Numbers with labels against the limits in `research/cloudflare.md`.
- **Downtime.** A table with the header `| Incident | Evidence | This design | Effect |`, one row per GitHub incident class in `research/uptime.md` findings 40 and 43 to 49, and one row per major Cloudflare incident in `research/cloudflare-uptime.md`. Effect is `removed`, `contained`, `mitigated`, or `exposed`. Then RPO and RTO per durability class.
- **Failure modes.** A table with the header `| Component | Failure | Blast radius | Containment |`, one row for each of `instance`, `data-center`, `region`, `provider-wide`, `control-plane`, `bad-deploy`, `bad-config`, `noisy-tenant` for every stateful component.
- **Deployment modes.** A table with the header `| Component | Managed | BYOC | Self-hosted |`, one row per stateful component in Failure modes, same first-cell text. Then what each mode requires of the customer.
- **Cost.** Monthly list cost at 50, 300, and 2,000 developers, using the workload in `scripts/cost_model.py` (`TIERS`, `ACTIVITY`) and `cf.*` prices, from a script in your directory. Add the cheapest reliable setup for 50 developers.
- **Build order from zero.** Milestones in order, each with a gate. Milestone 0 names the measurements that would make the design wrong, with numeric pass and fail values.
- **Compared with the AWS design.** A table of the differences that matter to users, to maintainers, and to the operator, with evidence.

Before your final reply, run `python3 ../../scripts/check_cloudflare.py <your DESIGN.md>` from this directory and fix every failure. It checks structure only.

## Final reply

At most 12 lines: the path, the two decisions you are least sure of, and the one Cloudflare fact that would most change your design if it were false.
