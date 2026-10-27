# Arena round 2: a global namespace for Tributary

## What changed

The operator decided that managed Tributary is one public, github.com-style service (`FRAME.md`, "Operator decision, 2026-10-24"). User and org logins are globally unique and share one namespace. Repository names are unique per owner. Public repositories allow anonymous reads. Users fork across owners and open pull requests across owners. Stars, follows, notifications, and search span the whole service. BYOC and self-hosted installs keep their own namespace, as each GHES instance does.

`DESIGN.md` was built on the opposite assumption: every managed tenant had its own hostname and its own namespace, and a tenant lived entirely inside one cell. Your job is to redesign the layer that assumption touched as if the global namespace had been a requirement from day one (`skill://principle-redesign-from-first-principles`), and to say exactly how the rest of `DESIGN.md` changes.

## Read first

1. `../../FRAME.md`.
2. `../../DESIGN.md` end to end. Keep what still holds: refs as rows with merges in one transaction, packs in S3, disposable node caches, the `core` and `aux` clusters, Paired durability, the per-consumer outbox, Actions, the Milestone 0 gate, and every resolution under "Review resolutions" that does not depend on tenancy.
3. `SUBTRACT.md` in this directory: what the global namespace makes obsolete.
4. `../../research/*.md` as needed. Cite as `research/<file>.md#<n>`. Label any fact not in the research [inference] or [unverified]. You may search the web for GitHub's namespace rules (login rules, case folding, rename redirects, namespace retirement) and AWS facts, and cite what you read.
5. `skill://architect/references/runner-prompt.md` and `skill://architect/references/design-red-flags.md`. Usage comes first, data structures next.

## Your direction

Your brief names one direction. Design the best namespace layer you can within it. Do not converge toward a safe middle.

## Journeys your design must trace

Trace each through your components, naming every store and cell touched and whether any step depends synchronously on something outside the cell that serves the request.

1. Anonymous `git clone https://tributary.dev/torvalds/linux.git` of a popular public repository, including rate limits for anonymous callers.
2. `bob` forks `acme/widgets` into `bob/widgets`, pushes a branch, and opens a pull request into `acme/widgets`. A maintainer merges it. Say where the merge-in-one-transaction invariant holds, or exactly how it weakens.
3. Two users race to create the org `newco`. Then `newco/a` is renamed to `newco/b`: the API answers 301 and git answers 200 for the old name (`research/wire-census.md`).
4. `alice` belongs to three orgs whose data lives in different places. She runs `gh repo list`, `GET /user/repos`, and a GraphQL `viewer { repositories }` query, stars a repository, and receives a notification.
5. A GitHub App installed on two orgs whose data lives in different places mints installation tokens and calls the API for both.

## Output

Write one file, `DESIGN.md`, in the directory your brief names. Hard cap 500 lines. Plain sentences per `skill://unslop`: no em dashes, no en dashes, no curly quotes. Use these headings in this order:

```
# <one-line title naming your direction>
## Problem
## Usage (caller's view)
## Shape
### Names and the directory
### Identity, orgs, and authorization
### Placement and routing
### Cross-cell features
## Public service concerns
## Failure modes
## Deployment modes
## Changes to DESIGN.md
## Tradeoffs accepted
## Alternatives considered
## Open questions and risks
```

- **Usage.** The five journeys, written before the types.
- **Shape.** Each subsection has a ```rust fence with core types and key signatures, plus the invariants they encode and the dominant access patterns traced through them.
- **Public service concerns.** Anonymous access and rate limits, abuse and spam, takedowns, CDN use for public content, and enterprise orgs that need data residency or SSO on a shared namespace.
- **Failure modes.** A table with the header `| Component | Failure | Blast radius | Containment |`. One row for each of `instance`, `az`, `region`, `control-plane`, `bad-deploy`, `bad-config`, `noisy-tenant` for every new stateful component on the request path.
- **Deployment modes.** How BYOC and self-hosted run the same code with their own namespace.
- **Changes to DESIGN.md.** A list keyed by `DESIGN.md` section: what is deleted, what is replaced, and what is added. Start with deletions.
- **Alternatives considered.** At least two structurally different shapes, with why each lost.

## Final reply

At most 15 lines: the path you wrote, the three decisions you are least sure of, and the one fact that would most change your design if it were false.
