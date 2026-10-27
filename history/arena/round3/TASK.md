# Arena round 3: a worldwide global tier

## What changed

`DESIGN.md` puts the namespace's `global` tier in one Aurora DSQL multi-Region cluster in the North American Region set, because DSQL clusters cannot span continents. Identity writes from Europe and Asia-Pacific pay a cross-continent round trip, a North American Region-set event stops identity writes worldwide, and identity facts are never stored in a customer's own geography. The operator now allows DynamoDB global tables "if that helps worldwide" (`FRAME.md`, "Operator decision, 2026-10-24, later").

Redesign the global tier as if worldwide operation had been a requirement from day one (`skill://principle-redesign-from-first-principles`), and say exactly how `DESIGN.md` changes. Use DynamoDB global tables only where they help. Saying that they do not help somewhere is a valid answer.

## Read first

1. `../../FRAME.md`.
2. `../../DESIGN.md`, especially "Namespace and cell placement", "Identity and tokens", "Cross-cell features", the `global`, Cell replica, Front cell, and Inter-cell log failure rows, Milestone 0, and "Review resolutions" NS-1 and NS-ACT-1 to NS-ACT-8. These guarantees must survive or you must say exactly how they weaken: no request reads a remote store; a replica never answers "no" about a fact it may not have seen (`Lookup::Behind`, fleet-relative freshness); `revoke` coverage counts edges; a cut-off cell fails write grants closed after 30 s; renames bump one row; claims are race-free.
3. `../round2/candidate-2/DESIGN.md`, which already designed a DynamoDB MRSC claim registry with pending, claim, activate sagas.
4. `../../research/uptime.md` findings 26 to 31 and `../../review/round2-judge.md` fact flags. Search the AWS documentation for anything your design rests on: MRSC Region sets and write latency, whether MRSC tables have streams and what they order, MREC conflict resolution and partial transactions, and DynamoDB Streams ordering. Cite what you read. Label anything else [inference] or [unverified].
5. `skill://architect/references/runner-prompt.md` and `skill://architect/references/design-red-flags.md`.

## Your direction

Your brief names one. Do not converge toward the other.

## Journeys to trace

Name every store and Region each step touches, and whether any step waits on another continent.

1. A user in Frankfurt signs up as `lena`, creates a personal access token, and pushes to a repository hosted in a North American cell within one second.
2. A user in Tokyo renames their login while the whole North American Region set is impaired.
3. An org owner in Virginia removes a member who lives in Frankfurt. The member's next push, through a Frankfurt front, must fail within the design's stated bound.
4. An anonymous request for an unknown login reaches a front in Sydney while one Region's replication is lagging.
5. A whole continent's Regions are lost for good.

## Output

One file, `DESIGN.md`, in the directory your brief names. Hard cap 320 lines. Plain sentences per `skill://unslop`: no em dashes, no en dashes, no curly quotes. Headings in this order:

```
# <title naming your direction>
## Problem
## Usage (caller's view)
## Shape
## Failure modes
## Residency
## Changes to DESIGN.md
## Tradeoffs accepted
## Alternatives considered
## Open questions and risks
```

- **Shape.** Rust type sketches for every new store and the key signatures, with the invariants they encode.
- **Failure modes.** A table with the header `| Component | Failure | Blast radius | Containment |`, with one row for each of `instance`, `az`, `region`, `control-plane`, `bad-deploy`, `bad-config`, `noisy-tenant` for each new stateful component, plus a `continent` row for each.
- **Residency.** What identity data a European or Asia-Pacific account keeps in its own geography, and what must leave it, and why.
- **Changes to DESIGN.md.** Deletions first, then replacements, then additions, keyed by section.

## Final reply

At most 12 lines: the path, the two decisions you are least sure of, and the one AWS fact that would most change your design if it were false.
