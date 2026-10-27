# Arena round 4: the foundation of a new GitHub, built fresh

## What changed

`DESIGN.md` builds Tributary, a GitHub-compatible host on AWS, on Aurora DSQL: refs, packs metadata, pull requests, issues, permissions, the event outbox, and the per-continent identity authorities are rows in DSQL clusters, with S3 for pack bytes and NVMe caches on nodes. The operator now says Tributary is "an entire new, better scalable github that has less downtime", not a port (`FRAME.md`, "Operator decision, 2026-10-24, on the store"). No foundation is fixed because an earlier round chose it. Downtime comes first, scalability second. Keeping invariants inside database transactions counts only as far as it lowers outage risk.

Design the foundation from scratch for those goals (`skill://principle-redesign-from-first-principles`). The foundation is everything whose shape depends on the choice of stores: the data model, how pushes and merges commit, how reads stay fresh, authorization and revocation, event fan-out, the worldwide namespace and identity layer, Region loss, scale limits, schema and access-pattern evolution, backups, and the managed, BYOC, and self-hosted forms. Everything else in `DESIGN.md` (wire compatibility with GHES 3.22, upstream git as the object engine, the Actions runner protocol, the web app, cells and waves as deploy units) may be reused by reference, or replaced if your foundation makes it wrong.

## Read first

1. `../../FRAME.md` in full.
2. `../../DESIGN.md`: Summary, Usage, Shape and every subsection under it, Availability, Deployment modes, Install operations, Build order, Tradeoffs accepted. This is the baseline you compete against.
3. `../../research/uptime.md` findings 26 to 31 and 40 to 49 (what takes GitHub down, and the AWS store facts), `../../research/dynamodb.md` (verified DynamoDB and DSQL facts, prices, Regions, SLAs), `../../research/git-storage.md` findings 1 to 31 and implications 1 to 12, `../../research/pricing.md`, and `../../research/byoc-grounding.md`.
4. Prior designs that used DynamoDB: `../candidate-2/DESIGN.md` (round 1, an MRSC item per repository as the ref commit point plus an S3 log) and `../round3/candidate-2/DESIGN.md` (round 3, the worldwide identity tier on DynamoDB with single-item fence commits). Their judges' verdicts are in `../../review/arena-judge.md` and `../../review/round3-judge.md`.
5. What broke in the installs last time: `../../review/selfhost-walk.md`, `../../review/byoc-walk.md`, and their re-walks.
6. `skill://architect/references/runner-prompt.md` and `skill://architect/references/design-red-flags.md`.

Search the AWS documentation for any fact your design rests on that the research files do not cover, and cite what you read. Label anything else [inference] or [guess].

## Your direction

Your brief names one. Do not converge toward the others.

## Journeys to trace

For each, name every store and Region each step touches, what is durable when the client is told "done", and what the client sees on its next read.

1. A developer atomically pushes three branches to a busy monorepo while its merge queue merges 10 pull requests a minute, bots update 50 refs a minute, and CI clones it 2,000 times a minute.
2. A pull request from a fork is merged while a reviewer submits a review and a check run completes. The author then runs `gh pr view` and `gh pr list` at once.
3. A user creates an issue with the label `bug` and immediately runs `gh issue list --label bug`, then `gh api repos/o/r/issues?sort=updated`.
4. An org owner removes a member. The member's next push must fail within the bound your design states.
5. The Region holding the newest writes of a repository is lost for good in the middle of a push, in managed mode and in a two-Region BYOC install.
6. A bad release multiplies reads on the identity data tenfold (GitHub's February 2026 incident), and another release writes a record shape that the previous release cannot read (GitHub's dropped-column incident).
7. A company self-hosts in a single AWS Region where Aurora DSQL is not offered, installs, loses data to an operator mistake, and restores to a point in time.

## Output

One file, `DESIGN.md`, in the directory your brief names, plus any script you use for costs in the same directory. Hard cap 700 lines. Plain sentences per `skill://unslop`: no em dashes, no en dashes, no curly quotes. Headings in this order:

```
# <title naming your direction>
## Summary
## Usage (caller's view)
## Shape
## Commit and read paths
## Scale
## Downtime
## Failure modes
## Deployment modes
## Cost
## Build order from zero
## Changes to DESIGN.md
## Tradeoffs accepted
## Alternatives considered
## Open questions and risks
```

- **Summary.** At most 12 lines: the bet, the stores, and what a GitHub user and a Tributary maintainer would each notice compared with `DESIGN.md`.
- **Shape.** Rust type sketches for every store's keys and records and the key signatures, with the invariants they encode. Trace each dominant access pattern through the keys.
- **Commit and read paths.** Push (up to 1,000 refs atomically), same-repository merge, fork merge, merge queue, branch-rule changes, list endpoints with GitHub's filters and sorts, authorization, revocation, and what each read guarantees after a write.
- **Scale.** Numbers with labels: the busiest single repository you can carry for pushes, merges, ref updates, and clones, and what limits it; fleet scale against GitHub's 3.35 billion pushes a month (`research/git-storage.md` finding 5); fan-out for webhooks and Actions; every hot key and how it is spread.
- **Downtime.** A table with the header `| GitHub incident | Evidence | This design | Effect |`, one row for each incident class in `research/uptime.md` findings 40 and 43 to 49 (database overload, schema change, engine upgrade, failover and partition, connection storms, config and deploy, capacity, search and DNS dependency, merge logic bugs). Effect is `removed`, `contained`, or `mitigated`. Then RPO and RTO per durability class.
- **Failure modes.** A table with the header `| Component | Failure | Blast radius | Containment |`, with one row for each of `instance`, `az`, `region`, `control-plane`, `bad-deploy`, `bad-config`, `noisy-tenant` for every stateful component.
- **Deployment modes.** A table with the header `| Component | Managed | BYOC | Self-hosted |`, one row per stateful component in Failure modes, with the same first-cell text. Then backups and point-in-time restore, and the Regions each mode can run in.
- **Cost.** Monthly AWS list cost at 50, 300, and 2,000 developers for single-Region and two-Region installs, using `../../research/prices.json` and the workload in `../../scripts/cost_model.py` (`TIERS`, `ACTIVITY`). Show the arithmetic or a script you wrote in your directory. Do not edit `scripts/cost_model.py`.
- **Build order from zero.** Milestones a team builds in order, each with a gate. Milestone 0 must name the measurements that would make your foundation wrong, with numeric pass and fail values.
- **Changes to DESIGN.md.** Deletions first, then replacements, then additions, keyed by section.

## Final reply

At most 12 lines: the path, the two decisions you are least sure of, and the one AWS fact that would most change your design if it were false.
