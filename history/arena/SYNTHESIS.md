# Arena synthesis for the Tributary design

`DESIGN.md` is candidate 3 (refs as DSQL rows) with the grafts below. This note records the base, every graft and its source, each rejection, and the verification result.

## Runners

| Candidate | Seat | Direction | Lines | Structure check |
| --- | --- | --- | --- | --- |
| `candidate-1` | `pstack-panel-a` | Spokes-style per-reference quorum on local NVMe, S3 as fencing virtual replica | 773 | OK |
| `candidate-2` | `pstack-panel-b` | S3 write-ahead log with a compare-and-swap register, disks as cache | 823 | OK |
| `candidate-3` | `pstack-panel-c` | Refs, pack manifest, PRs, and outbox as rows in one DSQL database per cell | 774 | OK |

No runner dropped out. The three panel seats had no model configured, so every runner and the judge ran the same model. Each runner got a different storage bet so that the candidates differed in shape. Convergence on the parts the bets did not fix is therefore weaker evidence than convergence across model families would be.

## Base

Candidate 3. The lead scored 19, 26, and 28 of 30. The cross-judge scored 19, 25, and 26 (`review/arena-judge.md`). Both picked candidate 3 under the rubric's rule: one transaction boundary and four types (`Acked`, `DurablePack`, `Change<T>`, `Decision::Hidden`) carry the invariants, so a maintainer extends it by adding rows inside `run_tx`.

The decision flips if DSQL cannot carry a hot repository. Milestone 0 in `DESIGN.md` settles that in one day before any git code. Candidate 2 is the named fallback base.

## Convergence

All three runners chose these without coordination, so the design ships them as agreed: DSQL for metadata, one GHES hostname per tenant with no shared namespace, GHES 3.22 as the REST and GraphQL target, `use_v2_flow: true`, a Rust port of the C# `GitHub.Actions.WorkflowParser`, Firecracker runners in separate accounts reaching cells only through the public edge, stateless `ghs_` installation tokens, seeded first-party OAuth client ids, expand and contract migrations with barriers only on contract, signed config bundles with no runtime flag service, and no GPUI app at launch.

## Grafts

| Graft | Source | Where in `DESIGN.md` |
| --- | --- | --- |
| Warm DSQL DR cluster fed by CDC for Regional cells, replacing restore from backups | candidate 1, judge graft 1 | Failure modes, DSQL `region` row |
| Monthly and quarterly Region flips, weekly AZ evacuation, failed practice blocks deploys | candidate 1, judge graft 2 | Availability, Drills |
| Durability SLO row with a daily replay of every real push | candidate 1, judge graft 3 | SLOs |
| Velocity caps on compaction, refill, and GC | candidate 1, judge graft 4 | Availability, Velocity caps |
| Bucket policy that requires `If-None-Match` on pack keys | candidate 2, judge graft 5 | Repository storage, Reachability |
| `pr-sync` writes pull shards coalesced last-writer-wins, behind user pushes | candidate 2, judge graft 6 | DSQL limits table, 40001 row |
| Per-repository push admission and per-tenant push caps | candidate 2, judge graft 7 | Repository storage, access patterns |
| Two-release rule for stored formats | candidate 2, judge graft 8 | Change safety, Schema migrations |
| Unapproved SHA-pin annotation names the matching fork SHA | candidate 2, judge graft 9 | Actions, toolkit section |
| Milestone 0 one-day DSQL benchmark that can flip the base | judge flip condition | Build order |

## Changes the lead made beyond the grafts

| Change | Reason |
| --- | --- |
| SSH resolves the tenant from per-cell unique key fingerprints, with one managed host key set | All three candidates routed SSH by hostname, but SSH has no SNI, owner names collide across tenants in a cell, and the host key is sent before authentication. Candidate 1 noticed the missing SNI, but its DNS answer stops at the cell. |
| PR head is the head branch's ref row, and `refs/pull/N/*` are derived refs that `pr-sync` maintains | Removes candidate 3's 100-open-PRs-per-branch cap and per-PR fan-out from the push transaction, per the judge, without weakening the merge invariant. |
| "Known deviations from GitHub" table with an oracle per deviation | The judge found the 1,000-ref atomic push cap was a client-visible limit missing from the wire table. |
| Stated the cost of the Rust receive-pack and its oracle | The judge found the cost unstated. Rust owns receive-pack so that `report_status` can require `&Acked`. |
| Fixed "four 99.99% targets" to two | The table has two. |

## Rejections

| Rejected | Source | Reason |
| --- | --- | --- |
| Per-reference quorum with burned fence seqs | candidate 1 | The judge found a stalled-coordinator path that breaks multi-ref atomicity, and candidate 1 labels it least proven. It also has the most moving parts: quorum, placement, sweepers, two fence stores, two event origins. |
| Serving shimmed bytes for SHA-pinned actions by default | candidate 1 | A SHA pin is a promise about bytes. Candidate 3's approved pin map keeps that promise unless an org admin opts in. |
| Identity snapshot pushed to S3 so git survives a DSQL outage | candidate 2 | In the base, refs live in DSQL, so git cannot outlive a DSQL outage anyway. The snapshot would add a pipeline without adding availability. |
| Branch protection stored as log entries | candidate 2 | The base has no log. Rules are rows read in the same snapshot as the push. |
| Node.js SSR tier | candidates 1 and 2 | Candidate 3's edge serves the shell and inlines the first route's query, which keeps a runtime and a fleet out of every mode. |
| Subdomain isolation on by default | candidate 1 | `research/clients.md` implication 1 says isolation should be optional because GCM strips those prefixes. |

## Verification

`python3 scripts/check_design.py DESIGN.md` runs after the grafts and again after the interrogate revision. `decisions.tsv` records each result.
