# Arena task: one whole-system design for Tributary

You produce one candidate design package for Tributary. Three runners work in parallel. Each runner has a different core bet, named in its brief as the direction. Design the best whole system you can around your direction. Do not hedge toward the other directions. If your direction is wrong for a subsystem, say so in "Open questions and risks" and still design that subsystem the way your direction implies, so the comparison stays honest.

Nobody builds the product in this run. Rust appears only as type sketches inside markdown code fences. Bodies are `todo!()` or pseudocode comments.

## Read first

1. `../FRAME.md`. The request, the deployment modes, the quality attributes in priority order, and the definition of done.
2. Every file in `../research/`. These are verified facts with sources. Cite them as `research/<file>.md#<finding number>` when a decision rests on one. When you rely on a fact that the research does not contain, label it [inference] or [unverified].
3. `skill://architect`, `skill://architect/references/runner-prompt.md`, `skill://architect/references/rationale-template.md`, and `skill://architect/references/design-red-flags.md`. Follow the runner discipline. Usage comes first, data structures next.

## Facts every candidate must answer

The research found these facts. Each one forces a decision. Your design must answer every one of them explicitly, and say where.

1. A whole AWS Region can be lost for good. In 2026 AWS declared me-south-1 unrecoverable and lost data that lived only in one me-central-1 zone (`research/uptime.md` findings 37 and 38). State the cross-Region RPO for an acknowledged push and for metadata. Offer a configuration where the cross-Region RPO for pushes is zero, and say what it costs.
2. Object storage is becoming the durable truth for git. GitHub is adding blob storage as a quorum member and moving to per-reference ordering. Cursor's Continuity acknowledges a push only after it is in an S3 write-ahead log, orders pushes with one S3 compare-and-swap, and treats local disks as cache (`research/git-storage.md` findings 5 to 7 and 29 to 31). Cursor sells this as Origin, a direct competitor.
3. No Rust library can serve git today. Upstream git stays the engine for pack generation and repacking (`research/git-storage.md` findings 44 to 49).
4. The Actions toolkit treats any host other than `github.com`, `*.ghe.com`, and `*.localhost` as GHES. On such a host `upload-artifact@v4` and later fail before any network call, and `actions/cache` falls back to the v1 cache API. The server hands out the tarball URL for every `uses:` action, so it is the only place a fix can go (`research/actions-protocol.md` findings 17, 43 to 49, and 57). Decide what Tributary does about this, including what happens to SHA-pinned actions.
5. The server chooses the runner protocol through `use_v2_flow` (`research/actions-protocol.md` findings 5 to 12). Choose one and say why.
6. The host must look exactly like GHES: one bare hostname on port 443, the GHES URL layout, `X-GitHub-Request-Id` and `X-GitHub-Enterprise-Version` headers, a real GHES version, and GitHub's first-party OAuth client ids seeded (`research/clients.md` findings on these, and `research/wire-census.md`). GraphQL is needed at launch by gh, VS Code Pull Requests, JetBrains, Renovate, and terraform-provider-github.
7. Installation tokens on github.com became stateless JWT-bearing `ghs_` strings on 2026-10-02. Node ids are MessagePack payloads that carry routing data. Numeric ids already exceed 32 bits and must stay below 2^53 (`research/wire-census.md`).
8. The vendor's BYOC machinery has no stage for stateful fleets, avoids schema migrations by treating each one as a rollback barrier, and cannot observe storage health (`research/byoc-grounding.md` reuse items 7 to 11). GHES and GitLab need downtime for most upgrades (`research/uptime.md` findings 53 to 56).
9. The October 2025 us-east-1 event broke launches and leases for about 14.5 hours while running instances kept working (`research/uptime.md` findings 32 to 35). Recovery must never depend on launching new capacity or on a control-plane API.

## Output

Write exactly one file, `DESIGN.md`, in the directory your brief names. Hard cap 1000 lines. Write plain sentences per `skill://unslop`. No em dashes, no en dashes, no curly quotes.

`python3 ../scripts/check_design.py <your DESIGN.md>` checks the structure below. Run it and fix every failure before you finish.

Use these headings, in this order, with exactly this text:

```
# <one-line title naming your direction>
## Problem
## Usage (caller's view)
## Shape
### Tenancy and cell placement
### Repository storage and ref transactions
### Identity and tokens
### Issues and pull requests
### Actions runs and jobs
### Event outbox
### Service and crate map
## Wire compatibility
## Availability
### SLOs
### Failure modes
### Change safety
## Deployment modes
## Actions
## Frontend and native app
## Build order
## Synthesis decision
## Tradeoffs accepted
## Alternatives considered
## Open questions and risks
## Next implementation step
```

### What each section must contain

- **Problem.** One paragraph per the rationale template, naming the constraints from FRAME.md and the research that make the shape non-obvious.
- **Usage (caller's view).** Write this before the types. Show three things. First, the operator quickstart for each deployment mode (what a self-hoster runs, what a BYOC customer grants, what a managed customer clicks). Second, three journeys traced at the wire level through your components, with the request path and every datastore touched: `git push` to a protected branch, merging a pull request, and an `actions/runner` picking up and finishing a job that uses `actions/checkout` and `actions/cache`. Third, what an unmodified `gh` CLI user types to start using a Tributary host.
- **Shape.** Data structures first. Each of the six named subsections has a ```rust fence with the core types and the key trait or function signatures for that domain, plus prose naming the invariants the types encode, per `skill://principle-model-the-domain` and `skill://principle-type-system-discipline`. State the dominant access patterns and trace them through the structures. "Service and crate map" lists every deployable service and Rust crate with one sentence each, and states which AWS services sit on the request path.
- **Wire compatibility.** A table with the header `| Surface | Tier | Oracle | Approach |`. One row for every `id` in `../research/surfaces.json`, with the id in backticks in the first cell. Tier is one of `T0` (launch blocker), `T1` (within six months of launch), `T2` (later), `T3` (best effort), or `Out` (never, with the reason). Oracle names the unmodified client and the command or test that proves the surface works. After the table, explain how the design handles the cross-cutting conventions in `research/wire-census.md`, the hostname heuristics in `research/actions-protocol.md` and `research/clients.md`, OAuth client ids for first-party clients, id ranges, and node ids.
- **Availability.**
  - "SLOs" has a table of user journeys with a monthly target percentage, the error budget in minutes, and how the SLO is measured. At least four journeys.
  - "Failure modes" has a table with the header `| Component | Failure | Blast radius | Containment |`. List every stateful component on the request path. Each component gets one row for each of these failure values, written exactly: `instance`, `az`, `region`, `control-plane`, `bad-deploy`, `bad-config`, `noisy-tenant`.
  - "Change safety" covers deploys, config, feature flags, schema migrations, and zero-downtime upgrades in every mode.
- **Deployment modes.** A table whose header names `Managed`, `BYOC`, and `Self-hosted`, with one row for each thing that varies between them. Everything not in the table is identical. Say how the design reuses the machinery in `research/byoc-grounding.md`, and what it must add.
- **Actions.** Workflow engine, runner protocol strategy, hosted runner fleet and its isolation from the control plane, cache, artifacts, logs, OIDC, and how the design copes with GitHub changing the toolkit.
- **Frontend and native app.** The React web app's architecture and how it reaches data, what keeps the UI up when backends degrade, and whether a GPUI native app earns its place, with what scope.
- **Build order.** Milestones in order. "Milestone 1" is the first verifiable milestone. Each milestone names its "Gate", the conformance oracle run that proves it.
- **Synthesis decision.** Write "Filled in by the arena lead." and nothing else.
- **Tradeoffs accepted, Alternatives considered, Open questions and risks, Next implementation step.** Per the rationale template. "Alternatives considered" names at least two structurally different whole-system shapes, each as a `### ` subsection or a `- **Name.**` bullet, with why it lost.

## Final reply

At most 20 lines. The path you wrote, the result of `check_design.py`, the three decisions you are least sure of, and the one fact that would most change your design if it turned out false.
