# Instructions for agents implementing Graftyard

You are building Graftyard from the design in this folder. Nothing is built yet. This file tells you which document wins, which invariants you must never break, how to pick work, how to change the design when the code proves it wrong, and what not to do. Start with `README.md` for the reading order and `design/GLOSSARY.md` for the glossary.

## Source of truth

When two documents disagree, the earlier one in this list wins.

1. `design/DESIGN.md` is the design of record. It wins over `design/SHARED.md` wherever the two disagree (`design/SHARED.md`, opening paragraph).
2. `design/SHARED.md` is the platform-independent specification the base reuses: wire compatibility, event types, tokens, issues and pull requests, folds, Actions, and retention.
3. `design/AGENT-LAYER.md` governs agent features. Its agent block in "Shape" is a copy of the one in `design/DESIGN.md`, "Shape", and the two must stay identical.
4. `FRAME.md` holds the goals, the constraints, and every operator decision. A design change that contradicts it needs an operator decision, not an edit.
5. `decisions/DECISIONS.md` says why each choice was made and what would reverse it. If it disagrees with a design document, the design document wins and the decision record is stale.
6. `build/BUILD-PLAN.md` orders the work. It does not change what the design says.
7. `research/` is evidence for the design's citations, not a list of requirements.

`history/` is never normative. It is the verbatim record of the design run, under the old name and old paths.

## Invariants never to break

Each line names the section that states the rule. Code that breaks one of these is wrong even if every test passes.

- Acknowledge a write only after the quorum rule holds: `Acked` is built only inside `outcome`, and `report_status` and every 2xx renderer take `&Acked` (`design/DESIGN.md`, "Shape", the write path and "Invariants").
- An event exists if and only if its change committed, so consumers read the log through cursors and never through a separate outbox (`design/DESIGN.md`, "Shape", "Invariants", and `design/SHARED.md`, "Event fan-out").
- A group has one entry in flight: `decide` sees the fold plus the earlier commands of the same entry, and a follower verifies entry k+1 only after it applies k (`design/DESIGN.md`, "Shape", "Invariants").
- `decide` is pure: its result depends only on the fold, the earlier commands of its entry, and the `deps` folds the leader read once through `fresh` (`design/DESIGN.md`, "Shape", the `Command` trait).
- No GHES 3.22 response changes for a client that does not opt in, and the agent layer's byte-diff row in Milestone 0 gates it (`design/AGENT-LAYER.md`, "Summary", "GHES compatibility", and "Build order").
- No request path calls the Cloudflare API, so swap-back is a signed pointer and `fanout` runs in band (`design/DESIGN.md`, "Commit and read paths", the November 2, 2023 class, and "Downtime").
- A new site-script version ships only through a site day, including a secret, variable, binding, or container config change, because any new version resets the site's members (`design/DESIGN.md`, "Install operations", Tools and Site day).
- The agent block in "Shape", from the `// ---- Agent load and agent safety` line to the next `// ----` line, stays byte-identical in `design/DESIGN.md` and `design/AGENT-LAYER.md`, and `scripts/check_design.py` fails otherwise.

## How to pick work

1. Open `build/BUILD-PLAN.md`.
2. Start with Milestone 0. Its measurements run before any git code, and any fail value sends the design back (`design/DESIGN.md`, "Build order from zero").
3. Take one work package at a time, in the order the plan gives.
4. Treat the package's gate as the definition of done. Compiling code or passing unit tests does not finish a package.
5. A hosted gate row is met at 90% of each target when the rest is a long tail; record the gap in `decisions/OPEN-QUESTIONS.md` and move on. An invariant above never counts at 90% (`FRAME.md`, "Operator decision, 2026-11-16, 90% gates and a daily spend limit").
6. If a gate fails, keep the gate as written and follow "How to change the design" below.

## How to change the design

Implementation will find places where the design is wrong. When it does, fix the design first, then the code.

1. Edit the design document that owns the claim. If the claim is in the agent block, make the same edit in both `design/DESIGN.md` and `design/AGENT-LAYER.md`.
2. Keep the evidence tags: [measured], [doc], [inference], [guess], and [unverified] (`design/SHARED.md`, "Read this first"). Change a tag only when a measurement or a document settles the claim, and cite that measurement or document.
3. Before you reverse a recorded decision, read its "Flip" line in `decisions/DECISIONS.md`. A reversal that meets no flip condition needs its own evidence.
4. If the change moves a cost, change the cost script or the price in `research/prices.json`, rerun the script, copy the printed figures into the Cost sections, and run `python3 scripts/costs_report.py`.
5. Run the three checks from the folder root:

	```
	python3 scripts/check_design.py
	python3 scripts/check_names.py
	python3 scripts/check_links.py
	```

6. Record the change. Add an unanswered question to `decisions/OPEN-QUESTIONS.md`. Add a settled choice to `decisions/DECISIONS.md` in the shape of the existing entries: decision, why, what it beat, evidence, and flip.

## Conventions

- Take cost numbers only from `scripts/cost.py` and `scripts/cost_agents.py`. Never edit `research/costs.md` by hand, because `scripts/costs_report.py` writes it.
- Use Cloudflare products only. A product qualifies only if the first table of `research/cloudflare.md` lists it with a usable status, and `design/DESIGN.md`, "Stack", copies that status. Workers KV, D1, Artifacts, and K2 are not used (`decisions/DECISIONS.md`, "KV, D1, and K2 are not used" and "Artifacts is not used").
- Run workerd only for the lifeboat and evaluation installs (`FRAME.md`, "Four Cloudflare questions still carry the run's defaults").
- Cite paths in this folder's layout, and cite design sections by heading, not by line number.
- Never write the project's old name. `scripts/check_names.py` lists any occurrence outside `history/`.
- A change under `workers/edge/` or any crate the edge links runs `bash tests/edgeboot/run.sh` and `PRODUCT=1 bash tests/edgeboot/run.sh` before it merges (CI job `edgeboot`): they serve the edge's own wasm in local workerd, which no native test or clippy does, and fail on a 500 or a wasm trap.
- A change to `workers/site/src/export_feed.rs` or `workers/site/src/registry.rs` runs `bash tests/exportbed/run.sh` before it merges (not in CI: it builds the site, about 11 minutes): it runs the site's own build as three scripts in local workerd with a local export bucket and fails unless one leader report, one round claim, one `export/` object and the queued round after it go through. m3fx49's feed carried nothing because of SQL that no native test ran, and m3fx49b's queued round arrived as `{}`.
- A change under `workers/vault/`, `cli/src/vault.rs`, or the export endpoints the vault pulls (`crates/git/src/export.rs`, `workers/edge/src/export.rs`) runs `bash tests/vaultbed/run.sh` before it merges (not in CI: it builds the site, the edge and the vault): it runs the vault's own build against exportbed's site scripts and the edge's own build in local workerd and fails unless every `export/` object reaches the vault byte for byte, a pack an entry names is copied and checked, and a revoked token moves nothing.

## What not to do

- Do not serve anything under `/api/v3/agents/*`. The task API, leases, and the activity feed live under `/api/graftyard/v1` (`design/AGENT-LAYER.md`, "Tasks and the activity feed").
- Do not add a new failure domain, such as a store, a Durable Object class, an account, or an outside dependency, without a row for each failure class in its document's "Failure modes" table and a row in "Deployment modes". The failure classes are instance, data-center, region, provider-wide, control-plane, bad-deploy, bad-config, and noisy-tenant, and `scripts/check_design.py` checks them.
- Do not read `history/` for requirements. Its paths point to the old workspace, and its documents include designs that lost.
