# Graftyard decisions

This file records the decisions that shape what you build, and the ones you might be tempted to reverse. Each entry gives the decision, why it was made, the alternative it beat, where the evidence is, and what would reverse it.

The full log is `history/decisions.tsv`, one row per decision with its reason, evidence, and result. A row number below is that row's line number in the file, so row 112 is line 112. The reviews, walks, and judge verdicts are under `history/review/`, and the design arenas are under `history/arena/`. The root `FRAME.md` restates every operator decision, and `history/FRAME.md` is the original with the verbatim quotes used here.

If this file disagrees with `design/DESIGN.md` or `design/AGENT-LAYER.md`, the design documents win and this file is stale. Evidence tags keep the design's meaning: [inference], [guess], and [unverified] mark claims that no measurement or document settles.

## Operator decisions

The operator wrote eight things that bind the design during the design run, and two more during Milestone 0. Each quote from the run is verbatim from `history/FRAME.md`; the Milestone 0 quote is from the build conversation. Everything after a quote is the run's reading, and the reading is marked where the operator did not say it.

### Managed mode has one global public namespace (2026-10-24)

> "yes it needs to have org and repo name uniquess as we will support public github style stuff"

**Answered.** Open decision 1, the tenancy model. It replaced the run's earlier reading of managed mode as many isolated tenants with one hostname each.

**Reading.** User and org logins are globally unique in one namespace, and repository names are unique per owner. The run also inferred, without the operator saying so, public repositories with anonymous read, forks and pull requests across owners, stars, follows, and search. BYOC and self-hosted installs keep their own namespace, as each GHES instance does.

**In the design.** The managed front account holds the global `Names` and `Tokens` groups, and owners live in cells behind it (see "Managed cells sit behind a front account").

**Evidence.** `history/FRAME.md`, "Operator decision, 2026-10-24". `history/decisions.tsv` rows 38 and 53. `design/DESIGN.md`, "Shape".

### DynamoDB global tables are allowed (2026-10-24, later)

> "you can use dynamodb global tables if that helps worldwide"

**Answered.** The open decision that the AWS design's `global` tier lived only in North American Regions, because Aurora DSQL clusters cannot span continents.

**In the design.** It applies only to the AWS alternative. The Cloudflare design of record uses no AWS service.

**Evidence.** `history/FRAME.md`, "Operator decision, 2026-10-24, later". `history/decisions.tsv` rows 56 and 57. `alternatives/AWS.md`.

### BYOC and self-hosted installs must work (2026-10-24, last)

> "also do a pass at the design and ensure byoc self installs would work too."

**Answered.** No default. It is a new requirement.

**Reading.** "byoc self installs" means both BYOC installs and self-hosted installs. A design passes only if every stateful component states its form in each mode and a self-hoster and a vendor SRE can walk install, upgrade, recovery, and exit against the text.

**In the design.** `design/DESIGN.md`, "Deployment modes", has a row per stateful component for each mode, and the Cloudflare design was walked as a BYOC install, a managed cell, and a self-hosted install.

**Evidence.** `history/FRAME.md`, "Operator decision, 2026-10-24, last". `history/decisions.tsv` rows 59 and 60. `history/review/cloudflare-byoc-walk.md`. `history/review/cloudflare-selfhost-walk.md`. `history/review/cloudflare-rewalk.md`.

### Build a better-scaling GitHub with less downtime, not a port (2026-10-24, on the store)

> "im not trying to port but rather build an entire new, better scalable github that has less downtime"

**Answered.** The question whether the design needs Aurora DSQL or could run entirely on DynamoDB.

**Reading.** No foundation is fixed because an earlier round chose it. Downtime and scalability outrank build convenience when foundations are compared.

**In the design.** The round-4 arena and the Cloudflare arena both weighted downtime 3 in their rubrics. That weight decided the Cloudflare base.

**Evidence.** `history/FRAME.md`, "Operator decision, 2026-10-24, on the store". `history/decisions.tsv` rows 76 and 112. `history/review/cloudflare-rubric.md`.

### Design a Cloudflare-only version (2026-10-26, Cloudflare)

> "note all these in the docs. then, use again and come up with what this would look like entirely cloudflare native. only cloudflare technologies. they have stuff like cloudflare basin, etc."

**Answered.** No default. It is a new request.

**Reading.** Two asks. First, record the cost findings in the docs. Second, design a separate version that uses only Cloudflare products, with the same goals. At the time the run read "what this would look like" as a comparison, not a replacement [inference]. Cloudflare Basin was confirmed as real and is used for analytics only.

**Evidence.** `history/FRAME.md`, "Operator decision, 2026-10-26, Cloudflare". `history/decisions.tsv` rows 103 and 108. `research/cloudflare.md`.

### Re-examine Artifacts and make the design agent-friendly (2026-10-26, agents)

> "read this. https://blog.cloudflare.com/next-git-platform-on-cloudflare why not artifacts? how can we take it a step further and make sure this is agent-friendly?"

**Answered.** No default. It reopened the rejection of Cloudflare Artifacts and added a requirement.

**Reading.** Re-check Artifacts against current facts, and design an agent layer that makes AI coding agents first-class users without changing any GHES 3.22 response for clients that do not opt in. "Agent-friendly" adds users and does not relax wire compatibility [inference].

**Evidence.** `history/FRAME.md`, "Operator decision, 2026-10-26, agents". `history/decisions.tsv` rows 133 and 134. `research/artifacts-recheck.md`. `research/agents.md`.

### Seven agent-layer answers, a final pass, and the name Graftyard (2026-10-26)

> "1 - offered is fine 2 - yes, include leases and tasks in v1 3 - sounds good as you decided 4 - default is fine 5 - sounds good 6 - sounds good 7 - ssh through spectrum read all fixes. do one final pass at everything. use for this, then clean up all notes so it is a single comprehensive folder of notes, cleaned, ready for an implementation by other agents. we are going to call this 'graftyard'."

**Answered.** The seven pending agent-layer decisions, each of which had a run default. Six kept the default and one overrode it.

| # | Question | Run default | Answer | Effect |
| --- | --- | --- | --- | --- |
| 1 | Run Graftyard's own agents (controller, agents account, Sandboxes, model keys), or only host third-party agents? | Offered, off until an org enables it | Default kept | Final pass FX-21 adds the org flag `agents_enabled`, off by default |
| 2 | Are leases, a live who-is-working-on-what feed, and a task API must-haves for the first release? | No | Overridden: yes | Leases, the activity feed, and the task API ship in the agent layer's first release. The run reads "leases" to include the read side, the feed [inference] |
| 3 | Do agent pushes get CI without a human click by default? | No: `ci: Approve`, as on GitHub, with `AutoReadOnly` opt-in per repository | Default kept | A 500-agent fleet needs about 75 approvals a minute under the default |
| 4 | Move metadata child groups (base Milestone 7) earlier for 500-agent fleets on one repository? | No | Default kept | Fleets of that size on one repository wait for Milestone 7 |
| 5 | Is a 25% test-merge budget acceptable? | Yes | Default kept | Most `mergeable` polls return null under load |
| 6 | In managed mode, per-tenant App keys or per-tenant agents accounts? | Per-tenant App keys | Default kept | Per-tenant accounts stay the upgrade path |
| 7 | Keep SSH through Spectrum, or serve HTTPS only? | Keep SSH, subject to Milestone 0's Spectrum-to-Tunnel row on each plan | Default kept | See "SSH goes through Spectrum, Tunnel, and the container outbound handler" |

**Reading of the rest.** "Use for this" reads as "use poteto mode for this". The project is renamed Graftyard and delivered as one implementation-ready folder. The final adversarial pass is under "Final pass" below.

**Evidence.** `history/FRAME.md`, "Operator decision, 2026-10-26, agents answered and graftyard", and "Pending operator decisions for the agent layer" in the section before it. `history/decisions.tsv` row 146.

### Rename the working folder in place (2026-10-26)

**Decision.** During the run the operator asked to rename the working directory "to simply 'graftyard'". The full sentence is in `history/FRAME.md`, in the same section as the seven answers. It names the old folder, so it is not repeated here.

**Effect.** The renamed folder is the deliverable. It is restructured in place by a move-only script, and the record implementers do not need moves into `history/`. Nothing is deleted.

**Evidence.** `history/FRAME.md`, "Operator decision, 2026-10-26, agents answered and graftyard". `history/decisions.tsv` row 148. `history/plans/graftyard-plan.md`.

### Keep `Tri` and gate commit p99 at 600 ms (2026-10-28)

> "ok, i approve. continue on."

The operator approved this proposal, made in the same conversation after Milestone 0's B0-01 results: keep the three-site Raft base, restate B0-01 to pass at a commit p99 of at most 600 ms and fail over 1 s, move flip condition 1's latency clause to 1 s, keep its linearizability clause, and keep one entry in flight.

**Decision.** `Tri` stays the base. B0-01 passes at commit p99 at most 600 ms and fails over 1 s, and B0-02's latency half uses 600 ms. Flip condition 1 fires on any linearizability violation in the 48-hour run or a `Tri` commit p99 above 1 s.

**Why.** Milestone 0 measured commit p99 at 399 to 690 ms over 23 runs, median 491 ms, set by the enam to weur round under load, not by the code. Neither a WebSocket transport nor pipelined entries brought it under 400 ms. Flipping to candidate 1 would buy latency, the fourth quality attribute, with RPO zero across regions, part of the second, and `FRAME.md` says the higher attribute wins. A push already spends seconds in transfer and `index-pack`, so 0.5 s of commit is a small share of it [inference].

**Beat.** Candidate 1, which acknowledges in one region and still needs its standby-loss stall fixed. Pipelined entries, which cut p99 by 70 to 100 ms at depth 4 and were worse deeper, and which break the fold that `decide` and `verify` see.

**Evidence.** `tests/m0/results/m0-wp2.md`, B0-01 and "Two levers tried for B0-01". `decisions/OPEN-QUESTIONS.md`, F2.

**Flip.** User reports that writes feel slow, or an API-heavy workload where 0.5 s on every write blocks adoption, reopen candidate 1 or a regional `Na3` default for single-continent installs.

### Measured latency is accepted, and the build continues (2026-10-28)

> "just continue, latency is fine. continue the full build of the platform"

**Decision.** The measured network latency of Milestone 0 is accepted for every latency gate built on the same commit or read-index round: B0-18 passes at learner read p99 at most 1 s (was 150 ms), and B0-29, B0-33, and LF-12 use a commit p99 of at most 600 ms (were 300, 150, and 300 ms), matching B0-01. B0-31 keeps its 1 s. Learners keep read-index; no follower lease is added.

**Why.** Each of these gates assumed an 80 ms round [guess]. Milestone 0 measured that round at p50 107 to 189 ms and p99 283 to 473 ms under load, and a learner read is one such round to the leader (`tests/m0/results/m0-wp2.md`, B0-01 and B0-18). The previous entry kept the base on the same evidence.

**Beat.** A follower lease for learners, which would answer without a round trip but moves stale-read risk onto clock drift for every learner, and counting only learners in the leader's region.

**Evidence.** `tests/m0/results/m0-wp2.md`, B0-01 and B0-18.

**Flip.** As the previous entry: user reports of slow reads on hot groups reopen the follower lease.

### B0-03 and B0-25 are restated to their measured values (2026-10-28)

> "both is fine"

The operator approved restating both gates that sat at their pass lines, rather than shortening `ELECTION_AFTER` and `LEASE` or adding a third `commit` retry.

**Decision.** B0-03 passes when a new leader commits and serves `fresh` within 13 s at p99 in both runs (was 12 s); its fail line stays over 60 s. B0-25 passes when at least 92% of pushes succeed within three attempts at 22% errors (was 96%); its fail line stays under 84%.

**Why.** B0-03 measured 11.0 to 12.9 s over 26 runs with 0 failed operations, and the terms are fixed by `ELECTION_AFTER` (10 s), the 1 s deadline, and two rounds of 110 to 190 ms. B0-25 measured 92.3% to 98.7%, median 95%, over nine runs; the 96% was arithmetic that assumed independent call failures, and an injected write failure resets the member and fails every call in flight on it. Both fail lines are far away.

**Beat.** Shortening `ELECTION_AFTER` and `LEASE` by about 1 s, which narrows the clock-drift margin behind every lease (F3). A third retry for `commit` only, which adds load during the very incident class the row models.

**Evidence.** `tests/m0/results/m0-wp2.md`, B0-03 and B0-25.

**Flip.** A B0-03 rerun over 13 s at p99, or a B0-25 run under 92% on the release code, reopens the timer change or the third retry.

### Four Cloudflare-design questions still carry the run's defaults

These are not operator decisions. The run asked them on 2026-10-26 and adopted a default for each. The operator's numbered answers match the agent-layer list in content, so these four stay unanswered [inference].

1. May Graftyard run workerd plus Docker on non-Cloudflare hardware for a read-only lifeboat, for evaluation, and for production? Default: yes, yes, no. Self-hosted then means the customer's own Cloudflare account, and the lifeboat is the only off-Cloudflare component in production.
2. May managed users get a lifeboat on vendor hardware for Cloudflare-wide outages? Default: no, so managed has no service during one.
3. Is a home-grown Raft over Durable Objects acceptable as the commit point? Default: yes, gated by Milestone 0's 48-hour linearizability run, with candidate 1 as the fallback.
4. Should BYOC require the Enterprise plan? Default: no. Pro carries 100 MB HTTPS pushes and Business 200 MB, and pushes over 200 MB need Enterprise.

**Evidence.** `history/FRAME.md`, "Pending operator decisions for the Cloudflare design". `design/DESIGN.md`, "Open questions and risks". `history/decisions.tsv` row 125.

## Foundation

### Cloudflare is the design of record

**Decision.** The Cloudflare design (`design/DESIGN.md` and `design/AGENT-LAYER.md`) is what you build. The AWS design is a summarized alternative.

**Why.** This is the run's reading of the operator's answers, not an operator statement. The operator never said to use Cloudflare instead of AWS. Every answer of 2026-10-26 is about the Cloudflare design and its agent layer, and answer 7 keeps a Cloudflare product [inference]. The choice does not rest on a score. In the Cloudflare arena the cross-judge scored the AWS design 42.5 as an ineligible reference, against 41.5 for the candidate that became the base.

**Beat.** The AWS design: chains on DynamoDB with a conditional put as the commit point, Paired cells in one continent, and SQLite folds on NVMe nodes.

**Evidence.** `history/FRAME.md`, "Operator decision, 2026-10-26, agents answered and graftyard". `history/decisions.tsv` row 146. `alternatives/AWS.md`, "Why the Cloudflare design is the design of record". `design/DESIGN.md`, "Compared with the AWS design". `history/DESIGN.md`.

**Flip.** `alternatives/AWS.md`, "When the AWS design would be the better choice", lists seven conditions, and none holds today. The main ones are a failed consensus gate whose fallback is unacceptable, a need for writes that survive a provider-wide outage, and a need for a commit point backed by a provider SLA.

### Clients see GitHub Enterprise Server 3.22

**Decision.** Graftyard targets GHES 3.22. An unmodified client works after its user points it at a Graftyard host, the same way it targets GHES.

**Why.** gh classifies any host that is not `github.com`, `*.ghe.com`, or `github.localhost` as GHES, and derives `/api/v3/` and `/api/graphql` from it. All three round-1 candidates converged on GHES 3.22.

**Beat.** No competing target was carried past round 1.

**Evidence.** `design/SHARED.md`, "Wire compatibility". `history/FRAME.md`, "What wire compatible means". `research/clients.md`. `research/wire-census.md`. `history/decisions.tsv` row 14.

**Flip.** None stated.

### Every aggregate is one chain, read through SQLite folds

**Decision.** A repository, a fork network, an owner's `Authz` and `Identity`, a token shard, a name shard, an org's Actions state, and the install's `Release` state are each one gap-free chain of entries (`ChainId`). Every read is a SQLite fold of a chain. Consumers read the log through cursors, with no separate outbox. Record formats follow the two-release rule, with no DDL anywhere.

**Why.** One commit point and one freshness rule serve every aggregate. A merge decides on every input committed before it, because refs, pull request state, reviews, rules, and required checks are entries on one chain. An event exists if and only if its change committed. No schema migration means no index, migration, or dropped-column incident.

**Beat.** Refs as rows in Aurora DSQL with a transactional outbox (AWS rounds 1 to 3), and a per-fork-network DynamoDB commit log (AWS round 4, candidate 1). The Cloudflare design keeps the chain model and replaces the table with Raft groups.

**Evidence.** `design/SHARED.md`, "Chain events and commands", "Folds", "Event fan-out", and "Record formats and the two-release rule". `design/DESIGN.md`, "Summary" and "Shape". `history/arena/round4/SYNTHESIS.md`. `history/decisions.tsv` rows 81 and 83.

**Flip.** None in the design of record.

### Three-site Raft groups of SQLite Durable Objects are the commit point

**Decision.** Every chain is a Raft group of three SQLite-backed Durable Objects, one voter on each of three sites. The default placement, `Tri`, puts the sites on enam, weur, and apac. A write is acknowledged only after its entry is on two sites in two regions. Leaders hold a 9 s lease, move by Raft leadership transfer, and lose promises through request-driven `suspect` reports from two colos. Groups above 500 reads a second add up to 16 non-voting learners.

**Why.** No Cloudflare storage product promises a cross-region RPO of zero (`research/cloudflare-uptime.md` finding 40), so Graftyard builds one. A single Durable Object "does not currently change locations after they are created" (finding 31), and a 13-day status sample shows a WEUR Durable Object incident of about 6 h 18 min (finding 20). Under the quorum a repository loses at most about 11.5 s once if its leader was there [inference]. Downtime carries weight 3.

**Beat.** Cloudflare arena candidate 1, one Durable Object per aggregate with a Paired standby and an R2 placement register. The judge found it stalls every Paired object when its standby's region is lost, and the review panel found that with the fix it still acknowledges on one region's copy after any failover. Scores were 38 lead and 35.5 judge for candidate 1, 24 and 22.5 for candidate 2 (Artifacts), and 37.5 and 41.5 for candidate 3, the pick. A write-once quorum on three R2 buckets also lost, because both 2025 R2 incidents failed 100% of writes globally.

**Cost accepted.** The team owns a Raft implementation with leases, transfer, `suspect`, pre-vote, joint consensus, learners, and `verify`. Every `Tri` write pays one intercontinental round trip, guessed at 80 ms and measured at p50 107 to 189 ms under load, so commit is about 200 ms at p50 and 450 to 700 ms at p99 (`tests/m0/results/m0-wp2.md`).

**Evidence.** `design/DESIGN.md`, "Summary", "Shape", "Tradeoffs accepted", "Alternatives considered", and "Synthesis decision". `history/arena/cloudflare/SYNTHESIS.md`. `history/review/cloudflare-judge.md`. `history/review/cloudflare-foundation-attack.md`. `history/review/cloudflare-interrogate.md`. `research/cloudflare-uptime.md`. `history/decisions.tsv` rows 110 to 112 and 116.

**Flip.** Flip condition 1: if Milestone 0's consensus gate fails, with any linearizability violation in the 48-hour Porcupine run or a `Tri` commit p99 above 1 s, candidate 1 becomes the base after its standby-loss stall is fixed. Flip condition 2: if a Durable Object's storage followers span at least two regions, candidate 1 with its `Alone` fix and `suspect` becomes the base for single-continent (`Na3`-class) and cost-sensitive installs, and `Tri` stays the base for installs that must survive a continent [inference]. M0-WP11 measured that the confirming quorum stays in the object's region, so it has not fired (`tests/m0/results/wp11.md`, (b)). M0-WP2 measured `Tri` commit p99 at 399 to 690 ms, over the clause's old 400 ms line, and the operator kept this base and moved the line to 1 s ("Keep `Tri` and gate commit p99 at 600 ms" above). Pending question 3 above asks the operator to confirm the home-grown Raft.

### One account holds three site scripts, and members talk over Durable Object bindings

**Decision.** The default install or managed cell is one Cloudflare account with three site scripts (`graft-site-a`, `graft-site-b`, `graft-site-c`). Each site script defines its site's member, git cache, and container classes and owns one pack bucket. Members reach each other through Durable Object bindings with `script_name`. Each install pre-creates one spare site per continent, already bound, so `ReplaceMember` creates no script version. A vault account and a runner account sit beside the install account. Account-separated sites are an option.

**Why.** A binding call runs inside Cloudflare's runtime and never uses the zone's public hostnames, so a partial proxy failure like December 5, 2025 should not cut replication. M0-WP11 saw a binding call arrive with none of the proxy's headers, and neither 2025 postmortem lists Durable Objects [measured, doc; the transport between colos stays undocumented]. Each group has one voter per site, so a region loss, a bad deploy of one site script, or a bad site setting removes at most one voter.

**Beat.** Account-separated sites as the default, which was candidate 3's first draft. Its members replicate over public HTTPS through site Workers, so a partial proxy failure stalls commits as well as client traffic. One account for every managed user also lost, because it puts everyone inside one account-scoped fault and meets account limits.

**Cost accepted.** An account-scoped fault reaches all three sites. The vault bounds the data loss.

**Evidence.** `design/DESIGN.md`, "Shape", "Tradeoffs accepted", and "Alternatives considered". `history/arena/cloudflare/SYNTHESIS.md`, graft 1. `history/review/cloudflare-byoc-walk.md` finding 8. `research/cloudflare-uptime.md` findings 7 and 14.

**Flip.** Milestone 0 withdraws the account-separated option if signed HTTPS through a site Worker adds more than 60 ms at p50 over a cross-script binding. The default's partial-proxy claim rests on M0-WP11 (a) (`tests/m0/results/wp11.md`). A Cloudflare incident in which proxy faults break Durable Object binding calls reopens it.

### Managed cells sit behind a front account

**Decision.** In managed mode a front account holds the zone, `graft-front`, the edge, and the global `Names` and `Tokens` groups on its own three sites. The front edge resolves an owner's cell from `Names` and calls that cell's edge over Ed25519-signed HTTPS. Inside a cell, members replicate over bindings.

**Why.** Bindings cannot cross accounts, so one edge on `graftyard.dev` cannot bind members in many cell accounts [inference from the service bindings documentation].

**Beat.** One account for every managed user.

**Evidence.** `design/DESIGN.md`, "Shape". `history/review/cloudflare-interrogate.md` CF-ACT-11. `history/review/cloudflare-byoc-walk.md` finding 8.

**Flip.** None stated. Whether Cloudflare's terms allow one service to span several accounts is open (`design/DESIGN.md`, "Open questions and risks").

### Reads are fresh by default, and stale reads need a signed flag

**Decision.** Only a leader that holds its lease answers a `fresh` read alone. Followers and learners answer after a read-index round. Stale mode starts only by a signed flag.

**Why.** A read never misses an acknowledged merge by default. Automatic stale mode fires mostly during partitions, when a minority member would serve reads that miss acknowledged merges.

**Beat.** Automatic stale mode (CF-ACT-8), and the AWS design's strong tail read on every request.

**Cost accepted.** A partitioned minority serves nothing until an operator decides. Lease safety assumes clock drift under 500 ms [guess].

**Evidence.** `design/DESIGN.md`, "Shape", "Tradeoffs accepted", and "Compared with the AWS design". `history/review/cloudflare-interrogate.md` CF-ACT-8.

**Flip.** None stated.

### `Solo` placement is for evaluation only

**Decision.** A one-member `Solo` placement exists for evaluation installs. It needs the install's signed `evaluation` flag and has no region-loss guarantee and no availability SLO.

**Why.** One Durable Object per repository loses writes, and probably reads, for the length of every regional event.

**Beat.** One Durable Object per repository as the production shape, which Artifacts, dgit, and git-on-cloudflare use. It is cheaper, and it survives only as `Solo`.

**Evidence.** `design/DESIGN.md`, "Shape", "Tradeoffs accepted", and "Alternatives considered". `history/arena/cloudflare/SYNTHESIS.md`, graft 4.

**Flip.** Flip condition 2 of the commit point, if Durable Object storage followers span regions.

### KV, D1, and K2 are not used

**Decision.** No Graftyard path uses Workers KV, D1, or K2.

**Why.** D1 has one primary in "one location in the world", replicas that "may be arbitrarily out of date", and a 10 GB cap, and it failed at 22% in June 2025. KV is eventually consistent for up to 60 s, so revocation would take 60 s for writes, and it failed about 90% of requests on 2025-06-12. K2 is in beta with a 10 GB account cap, and the chain is already the durable log.

**Beat.** D1 for metadata and identity, KV for tokens and routing, and K2 as the event log.

**Evidence.** `design/DESIGN.md`, "Summary" and "Alternatives considered". `research/cloudflare-uptime.md` findings 9, 13, 37, and 38. `research/cloudflare.md`.

**Flip.** None stated.

## Storage and git

### One group per repository, one entry in flight, and group commit

**Decision.** A repository's refs, pull requests, reviews, rules, and checks are commands on its one group. A group has one entry in flight, about 10 entries a second. Every command that arrives during a round joins the next entry, up to 1.5 MB. Each command's `decide` sees the fold plus the earlier commands of the same entry, and a follower verifies entry k+1 only after it applies entry k.

**Why.** `decide` and `verify` always see the fold they check, so a merge or a push cannot commit against a stale view. Pipelined entries break both, and without pipelining the earlier 300 entries a second could not happen.

**Beat.** Pipelined entries (CF-ACT-4). Per-ref write lanes for agent prefixes, from agent candidate C, also lost. They would move under a quarter of a 500-agent fleet's commands, because pull request records stay on the base branch's group. GitHub moved to per-reference write serialization because of agents, and the base does not match it on purpose.

**Cost accepted.** A hot repository is bounded by one group's commit rate. Oversized metadata moves to child groups at Milestone 7, which the operator kept (answer 4).

**Evidence.** `design/DESIGN.md`, "Shape", "Tradeoffs accepted", and "Milestone 0". `design/AGENT-LAYER.md`, "Alternatives considered". `design/SHARED.md`, "Chain events and commands". `history/review/cloudflare-interrogate.md` CF-ACT-4. `history/decisions.tsv` row 133.

**Flip.** Milestone 0 fails the design if group commit on one group carries under 100 commands a second. The 5,000-agent Milestone 0 row reopens ref lanes, with a parent `Abort { txn }` record, if fewer than 100 agent commands a second are admitted.

### Pack bytes are in two of three R2 buckets before acknowledgment, and small packs ride inline

**Decision.** `PackStore::place` writes a pack to the buckets of two sites in two regions, each put with `If-None-Match: *`, after `index-pack --strict` passes on the same bytes. Only then does a `DurablePack` exist, and only a `DurablePack` can enter an entry. Packs up to 1 MiB ride inline in the entry. Hot groups inline only up to 256 KiB while placement succeeds, and up to 1 MiB while it fails. A sweeper copies to the third bucket, and `ReplaceMember` copies single-copy packs first.

**Why.** Every acknowledged stored pack survives the loss of any one site with its bucket. Inline packs keep most pushes committing through an R2-wide outage, like the 59-minute 2025-02-06 event and the 67-minute 2025-03-21 event, while entries stay under the 2 MB row limit.

**Beat.** A write-once quorum on three R2 buckets as the commit point, and a five-member quorum of three Durable Objects and two R2 objects. R2 objects cannot refuse a stale round without compare-and-swap on a separate promise object.

**Cost accepted.** Pushes over 1 MiB fail during an R2-wide outage.

**Evidence.** `design/DESIGN.md`, "Shape" (`PackRef`, `DurablePack`), "Commit and read paths" (journeys 1 and 5), "Tradeoffs accepted", and "Alternatives considered". `research/cloudflare-uptime.md` findings 7 and 8. `history/review/cloudflare-interrogate.md` CF-ACT-7.

**Flip.** None stated. Milestone 0's bucket placement row checks whether R2's `location` field reports real placement.

### Git runs in git cache shards placed by rendezvous hashing

**Decision.** Upstream git (index-pack, upload-pack, merge-tree, repack) runs in containers owned by git cache shards, `GitCacheShard { site, shard }`, with a fixed count per site per tier. Networks map to shards by rendezvous hashing of (network, shard) within a site. Each network is warm on its group's home site and on the next site by rendezvous. A hot network fans out to k shards: its rendezvous shard plus shards from the site's hot pool, also by rendezvous, so the shard count never changes and no other network moves. Shards hold nothing canonical and rebuild from R2 packs and the leader's refs.

**Why.** `max_instances` defaults to 20, and a warm instance per network costs about $42 to $83 a month. The shards later moved to the `durable_object` policy, which has no `max_instances`, and the per-network shape would still cost a warm instance per network. Two warm copies of every shard mean a site loss or a control-plane outage needs no new container start.

**Beat.** A git cache per (network, site), which meant one container per active network per site.

**Cost accepted.** Container memory for warm shards is the largest line in the bill at every size, and the working set behind it is a guess. The largest repository is bounded by the 20 GB shard disk, about 15 GB of packs.

**Evidence.** `design/DESIGN.md`, "Shape" (git cache block), "Failure modes" (`git caches`), "Tradeoffs accepted", and "Alternatives considered". `history/review/cloudflare-interrogate.md` CF-ACT-9. `history/review/cloudflare-byoc-walk.md` finding 4. `history/decisions.tsv` rows 121 and 131.

**Flip.** None reverses the shape. Milestone 0 measures the daily working set, and `scripts/cost.py` derives the shard count from it. Another Milestone 0 row tests whether new containers start with the control plane out of reach.

### The host, not git, decides when a push is acknowledged

**Decision.** `Acked` is built only inside `commit`, from the acks it counted. `report_status` and every 2xx renderer take `&Acked`, so no code path can tell a client "ok" before the quorum.

**Why.** The type system enforces the acknowledgment point. Upstream git does the git work and never decides acknowledgment.

**Beat.** Letting git's hook state machine order acknowledgment, which round 1 of the AWS design rejected for the same reason.

**Evidence.** `design/DESIGN.md`, "Shape" (the write path) and "Stack". `history/decisions.tsv` row 26.

**Flip.** None stated.

### Artifacts is not used

**Decision.** Cloudflare Artifacts has no role, neither as the git store nor as an agent workspace tier.

**Why.** New repositories cap at 1 GB, against the 5 GB monorepo in journey 1. A repository is "a single logical instance", which is the single Durable Object failure domain again, and its status component has degraded with Durable Objects twice. It is in open beta, which Cloudflare's SLA excludes, with no stated RPO or RTO and no runtime off Cloudflare for the lifeboat to serve. The conflict predictor needs branches inside the network's shard, and Graftyard forks in the same network already share packs. Agent candidate B's measured gain, 5 times fewer canonical pushes, came from landing every 10 minutes, which needs no Artifacts.

**Not reasons.** v1-only push is invisible to unmodified clients, because git has no v2 push command. `filter` is documented as unsupported only among v1 capabilities. An earlier text named both as reasons, and row 140 corrected it.

**Beat.** Cloudflare arena candidate 2, Artifacts plus a confined container git engine (24 lead, 22.5 judge), and agent candidate B, an Artifacts workspace tier (39.0 lead, 30.0 judge).

**Evidence.** `design/DESIGN.md`, "Alternatives considered". `design/AGENT-LAYER.md`, "Artifacts' role". `research/artifacts-recheck.md`. `history/review/cloudflare-agents-foundation.md` finding 10. `history/review/cloudflare-agents-judge.md`. `history/decisions.tsv` rows 133, 135, and 140.

**Flip.** Artifacts earns one role, a scratch tier for best-of-N attempts that never become pull requests, only when forks share objects with their source and Cloudflare raises the per-repository cap above the install's largest network in writing. M0-WP11 measured that forks copy the source's full size (`tests/m0/results/wp11.md`, (d)), so the condition fails today. The tier falls back to a workspace fork on any Artifacts error. GA under an SLA is then a preference, not a condition.

## Release and operations

### Site scripts roll one per day after a drain, and old followers check every entry

**Decision.** Every operation that creates a new site-script version is a site day: drain the site, deploy at 1%, 10%, 50%, then 100%, then undrain. A drain moves every leader off the site within one lease by transfer. For its first day after undrain the site leads only 1% of its home groups, and the next site waits for that day to pass. Container image changes ship on their own site day. Followers on release N `verify` every entry. A refusal by an older follower of an uncommitted entry deposes the leader that wrote it. Any other refusal keeps the leader, marks the refuser unable to follow, and halts the site day. A format is enabled only after every site and every registered lifeboat runs N+1. `graft-ops` drains a sick site automatically, one site at a time, when both other sites report its missed deadlines.

**Why.** Any new version of a site script resets its members, including a secret, variable, binding, or container config change. Without a drain each site deploy costs about one bad minute, about 13 a month against a 4.3-minute read budget.

**Beat.** Deploys without a drain (CF-ACT-1), a refusal that blocks every later entry in the group (CF-ACT-3), and a leader that steps down on any refusal (CR-ACT-1). The AWS design rolled cell by cell, then by availability zone.

**Cost accepted.** Releases are slow. Once two sites run N+1, a shape N cannot read leaves each affected group on two members with no spare voter until the third site rolls forward.

**Evidence.** `design/DESIGN.md`, "Commit and read paths" (journey 7), "Install operations" (Site day), "Downtime", and "Tradeoffs accepted". `history/review/cloudflare-interrogate.md` CF-ACT-1, CF-ACT-3, CR-ACT-1, CR-ACT-6, and CR-ACT-9. `history/review/cloudflare-foundation-attack.md` F2. `history/decisions.tsv` rows 120 and 129.

**Flip.** None reverses the model. Milestone 0's site-deploy, follower-regression, refusal, and gray-site rows gate it, and a single failed write in the site-deploy row fails it.

### `graft-ops` executes signed intents inside the account

**Decision.** One stateless `graft-ops` script, with no member classes, executes intents signed by a key in the latest `Keys` entry. It uploads only bundles from the install's release bucket whose release signature verifies, with a deploy token the customer created. It owns every Cron Trigger that calls the Cloudflare API and every rotatable secret, held in Secrets Store, and it mints installation tokens. The vendor holds no Workers permission. The keys are the release-signing key (the vendor's release service), the vendor intent key, the customer intent key (offline: `Keys`, genesis, `Halt`, `Resume`, `Abort`, `SiteLost`, and lifeboat entries), a drain-only key, and ring keys. Self-hosted is BYOC with `Keys { vendor: None }`.

**Why.** No Worker can deploy or roll itself back, so the earlier "watchdog swap-back" named no actor, and several runtime jobs needed API tokens the design said it did not hold. A customer must be able to cut the vendor off mid-roll: revoke the vendor token, sign `Halt`, sign `Keys` with `vendor: None`, then `Resume` or `Abort`.

**Beat.** A vendor tool that drives every lifecycle step from outside the account, which the AWS install walks already rejected (IN-ACT-1), and the AWS design's in-account operator role.

**Evidence.** `design/DESIGN.md`, "Deployment modes", "Install operations", and "Failure modes" (`release control`). `history/review/cloudflare-interrogate.md` CF-ACT-2, CF-ACT-14, CR-ACT-2, and CR-ACT-3. `history/review/cloudflare-rewalk.md`. `history/decisions.tsv` rows 65, 128, and 129.

**Flip.** If Milestone 0 shows an account-owned token with Workers Scripts Edit and Containers Edit cannot deploy a container image, the deploy token moves to a user token held by the customer.

### Swap-back is a signed pointer, with no API call

**Decision.** A stable `graft-front` Worker forwards every request to the edge through a `fetch()`-based service binding, and the edge calls `graft-ops` the same way. Each call carries a version override only while the `Release` group's `Pointers` pin that script ("Pointers pin only during a swap-back", below). Every release intent carries pre-signed undos: a `Rollback` to N per day target, and an `EdgePointer`, `OpsPointer`, or `McpPointer` pin to N for each pinned script it rolls. `graft-ops` or an edge isolate submits one on a gate breach with no human signature. `FrontRoll` and `OpsRoll` run at 1% for a day, then 100%, with N kept.

**Why.** The 2023-11-02 control-plane and API outage lasted about 41 h. Swap-back, drain, and rollback must work without the Cloudflare API.

**Beat.** Rollback through the deployments API. It remains only for `graft-front` itself, which is the swap mechanism for everything else.

**Cost accepted.** A control-plane outage freezes a bad `graft-front` at its rollout percentage. `FrontRoll` holds it at 1% for a day to keep that share small.

**Evidence.** `design/DESIGN.md`, "Stack", "Commit and read paths" (journey 7), "Failure modes" (`front door`), and "Tradeoffs accepted". `history/review/cloudflare-interrogate.md` CF-ACT-2 and CR-ACT-7. `research/cloudflare-uptime.md` finding 2.

**Flip.** None stated. Final pass items FX-1 and FX-8 tighten how entries bind and how isolates cache pointers. The implementability walk made pointers pin-only overrides.

### Every install runs a vault account

**Decision.** A vault account that none of the install's tokens can reach runs a Worker that pulls the change feed and packs from the install's edge, with an install-scoped export token signed by the customer key. It writes them to R2 under 35-day bucket locks. The export covers every `ChainId`, including `Release`, `Names`, `Tokens`, `Identity`, and user-owned repositories. A whole-install restore rebuilds new accounts from the vault up to an operator-chosen cutoff. The vault is required in managed, BYOC, and self-hosted.

**Why.** In the default one-account shape, an account-scoped fault reaches all three sites. One leaked or misused deploy token can delete every site, and the 2025-02-06 abuse action aimed at one bucket disabled all of R2. The vault bounds the loss to the vault lag, typically about 30 s [guess], or to the chosen cutoff after a compromise that wrote through the protocol.

**Beat.** No copy outside the install account, which left account compromise with no RPO row. An install without a vault loses everything. Account separation alone contains only the intended scope of an abuse action, not a mistaken one.

**Evidence.** `design/DESIGN.md`, "Install operations" (Vault, Whole-install restore), "Downtime" (RPO and RTO per durability class), and "Deployment modes". `history/review/cloudflare-interrogate.md` CF-ACT-10 and CR-ACT-5. `research/cloudflare-uptime.md` finding 7.

**Flip.** None stated. Final pass item FX-6 keeps the vault's packs at least 35 days past the install's GC.

### A read-only lifeboat on workerd is the only off-Cloudflare component

**Decision.** BYOC and self-hosted installs may run a lifeboat, recommended as two workerd hosts on the customer's network. It holds a rebuildable copy from the change feed and serves clones and reads, never writes. Reads are opt-in per client through `graftyard lifeboat enroll`, which mints a `lifeboat:read` token that the main edge refuses. Each lifeboat registers its own key with a customer-signed `LifeboatAdd`, and `formats:enable` waits for every registered lifeboat. `graft-blob` fsyncs the file and the directory before any 200. Managed mode has no lifeboat unless the operator decides otherwise.

**Why.** During a Cloudflare-wide outage nothing on Cloudflare serves. Accepting writes outside the quorum would fork chains, which is the 2018 class of failure.

**Beat.** A writable lifeboat that promotes during a Cloudflare-wide outage, whose writes could not merge back without a stated loss. Production on workerd, because `localDisk` is experimental and `localDocker` is for development only. A second DNS provider, which was dropped.

**Cost accepted.** The lifeboat runs on a runtime that is not a hardened sandbox, so it never holds canonical state or a main-host token. It holds only a validation-zone DNS token and a read-only R2 token.

**Evidence.** `design/DESIGN.md`, "Install operations" (Lifeboat), "Deployment modes", "Tradeoffs accepted", and "Alternatives considered". `history/review/cloudflare-interrogate.md` CF-ACT-15, CF-ACT-16, CR-ACT-8, and CR-ACT-10. `history/review/cloudflare-selfhost-walk.md`.

**Flip.** Pending questions 1 and 2 under "Operator decisions". A "production" answer to question 1 brings the work listed in `history/review/cloudflare-selfhost-walk.md` F2, F3, and F5.

### SSH goes through Spectrum, Tunnel, and the container outbound handler

**Decision.** HTTPS is the default git transport on every plan. On Pro, and on Business by the same plan table [unverified], an install may turn on SSH through a Tunnel public hostname of type `ssh` that clients reach with `cloudflared access ssh` as an SSH `ProxyCommand`; that option is off by default. On Enterprise, SSH is native: Spectrum on port 22 forwards TCP to the Tunnel's virtual network. Both ingresses end at a cloudflared replica inside an SSH front container, where `graft-sshd` calls the container's outbound handler, `ssh_outbound`, which runs in the Workers runtime with the site script's bindings. The handler resolves the key fingerprint on its `Tokens` group leader and runs the same `commit` through the same bindings, so SSH never loops back through the public zone. The operator kept SSH (answer 7) and set this split on 2026-10-31 (`FRAME.md`, "Operator decision, 2026-10-31, SSH ingress").

**Why.** Workers accept no inbound TCP and containers expose no public port, so Spectrum is the only Cloudflare product that serves a public port 22, and on Pro it refuses a Tunnel origin: the API rejected `virtual_network_id` as an unknown field for a pay-as-you-go account (10012) and refused even a plain `tcp/22` app with "The requested protocol is not available" (13002) [measured, `tests/m0/results/m0-wp7.md`]; the operator's own attempt from the dashboard with a private-IP origin got 11002 (operator report, 2026-10-31, not in the results file). A virtual-network origin needs the TCP application type, which only Enterprise has [doc]. The Tunnel and SSH front half works without Spectrum: through `cloudflared access ssh`, three cloudflared replicas in `durable_object` containers served 2,634 of 2,637 sessions (99.886%) over 2.2 h with 13 container kills, one session lost to a kill, and the supervisor restarted each container in a median 0.72 s [measured, same]. That path needs cloudflared on every client, which a stock `git clone git@host:...` user lacks, so it is an option and HTTPS stays the default. HTTPS pushes are capped at 100 MB on Pro and 200 MB on Business, so the edge's refusal names the SSH option where the install has it on, SSH on Enterprise, and otherwise Enterprise or splitting the push.

**Beat.** A relay VM outside Cloudflare that terminates port 22 and forwards into the Tunnel, which breaks the Cloudflare-only constraint and adds a failure domain; the operator later accepted one for `graftyard.dev` until go-live as a dated exception ("Plain SSH through an Oracle Cloud Always Free VM until go-live (2026-11-02)"). HTTPS only on Pro with no option, which leaves Pro installs that need SSH for large pushes no path below Enterprise. SSH writes that loop back through the public zone, which candidate 1's outbound-handler graft replaced.

**Cost accepted.** The option needs cloudflared on each client and depends on Tunnel public-hostname routing; native SSH on Enterprise depends on Spectrum. Below Enterprise Spectrum is not in the path, and on Enterprise a Tunnel origin forbids Proxy Protocol, so sshd never sees the client's IP and SSH abuse limits count by key and token. The option runs 3 SSH front containers when on, and Tunnel bytes are not billed (`python3 scripts/cost.py --ssh-option`). Spectrum SSH data on Enterprise costs $1 per GB above the plan allowance, plus an add-on fee no list price was found for [unverified].

**Evidence.** `tests/m0/results/m0-wp7.md`, including B0-24a's 24 h run of record (`tests/m0/results/raw/wp7/b0-24a-r4/summary.json`). `design/DESIGN.md`, "Shape" (SSH front), "Commit and read paths" (journey 4), "Tradeoffs accepted", "Open questions and risks", and "Milestone 0". `FRAME.md`, answer 7, its 2026-10-31 note, and "Operator decision, 2026-10-31, SSH ingress". `history/FRAME.md`, answer 7. `history/review/cloudflare-interrogate.md` CF-ACT-12. `research/cloudflare-git.md` finding 26. `history/arena/cloudflare/SYNTHESIS.md`, graft 2. `history/decisions.tsv` rows 132 and 144.

**Flip.** Spectrum on Pro or Business accepting a virtual-network origin, which would make native SSH available there. Or the `cloudflared` option reaching B0-24a's Fail value (under 99% of sessions over a day with a container killed every 10 minutes) on M6-WP2's rerun on real code, or losing an acknowledged push, which would leave Pro and Business HTTPS only. A no-kill background below 99.9% on that rerun, which would make the option's failures our edge's rather than the kill rate's, reopens whether to offer it. B0-24b, Enterprise Spectrum, is not run.

**Flip fired on Pro, 2026-10-31** (`tests/m0/results/m0-wp7.md`). The original decision put SSH through Spectrum on every plan, subject to Milestone 0's Spectrum-to-Tunnel row on each plan. The Spectrum API refused the design's app on the operator's Pro zone (10012, then 13002; the operator's dashboard attempt got 11002), which executed the operator's own condition in answer 7 ("Keep SSH, subject to Milestone 0's Spectrum-to-Tunnel row on each plan"). The operator then chose the split above: HTTPS by default, the `cloudflared` option on Pro, and SSH on Enterprise.

**B0-24a ended between the lines, 2026-11-01** (`tests/m0/results/m0-wp7.md`, B0-24a; decided under the operator's delegation of 2026-10-30). Over 24.0 h with 143 container kills, the option served 28,230 of 28,347 sessions, 99.587%, between the 99.9% pass and the 99% fail. The `cloudflared` option stays as built, off by default, on Pro and Business, and its gate stays as written and carries to M6-WP2's rerun on real code, like the other between rows. It does not fail the option, for three reasons. No acknowledged push was lost: 9,418 pushes were acknowledged and all 9,418 are in the group log. 85 of the 117 failures came within 2 min of a kill: sessions in flight on a killed container, at a kill every 10 minutes, far above any expected real rate, which puts a floor of about one failure per kill under the rate. The other 32 (the stream closed before the SSH banner, no banner within `ConnectTimeout`, 90 s timeouts) are the Tunnel edge, which nothing on Graftyard's side controls, and they put the no-kill background near 99.89%, matching the 2.2 h run's 99.886%. cloudflared was ready after a kill in p50 about 2.4 s, max 9.7 s [measured, `tests/m0/results/raw/wp7/b0-24a-r4/summary.json`]. Because that background alone sits at about the pass line, the install docs tell SSH-option users that a failed session, a push included, is safe to retry, and the edge's over-cap message keeps naming HTTPS as the default. WP7's build findings (the client stanza with `ConnectTimeout 30` and keepalives, `--ha-connections 2`, sshd's 30 s login grace and client keepalives, and `graft-sshd` retrying through `ssh_outbound` for longer than a group failover) are in `build/BUILD-PLAN.md`, M6-WP2. Beat: failing the option on the number alone, which would count the test's kill rate and Cloudflare's edge against Graftyard's code; or loosening the gate to 99.5%, which changes a gate number with no new evidence.

### Hosted runners live in a separate runner account

**Decision.** Hosted Actions runners run as Sandboxes in a runner account, in managed and in BYOC. Runners reach the Actions broker over HTTPS and need no member binding. Each site caps its git cache containers in the shard object ("Git cache shards run under the `durable_object` scheduling policy").

**Why.** Actions load cannot take the git cache vCPU that must carry a site loss.

**Beat.** Runners inside the install account.

**Evidence.** `design/DESIGN.md`, "Stack" and "Failure modes" (`git caches`). `history/review/cloudflare-interrogate.md` CF-ACT-9. **Weakened, 2026-11-15:** the capacity reason was set against an account cap of 1,500 vCPU; the Containers limits page now gives 15,000 [doc: https://developers.cloudflare.com/containers/platform/limits/, read 2026-11-15], so the 2,000-developer git caches' 384 vCPU reserve is 39x under it, and runners would rarely take vCPU a site loss needs. What keeps the separate account is the security reason: a hosted runner's environment has no trust into and no route to the install's accounts other than the public front (`design/SHARED.md`, "Actions", Hosted runners), and an account boundary is what makes that hold for code a job runs.

**Flip.** An isolation primitive inside one account that gives a Sandbox no binding, credential, or route into the install's Workers, Durable Objects, and buckets as strongly as an account boundary does; or an account vCPU cap low enough again that runners and git caches compete for it, which would restore the capacity reason too.

## Agents

### Agents are GHES clients (agent candidate A)

**Decision.** Agents use the GHES 3.22 surface Graftyard must serve anyway: GitHub Apps, narrowed installation tokens, REST, webhooks, and checks. Every addition is invisible to an unmodified client. The base changes that agent load forces live in `design/DESIGN.md`: hot-network fan-out from a per-site hot pool, lazy test merges under a budget, admission classes by origin and behavior, a smaller inline cap on hot groups, a log byte cap per member, and hot clone-cache entries in both warm buckets.

**Why.** The judge recomputed the 500-agent fleet's git CPU from the base's cost constants. Only A counted the per-push test merge, 6.25 vCPU of 11.5, so C's 6.96 and B's 4.9 undersize the shard that must carry a site loss. That and A's smaller surface decided it. Scores were A 50.5 lead and 47.0 judge, C 49.5 and 43.0, B 39.0 and 30.0.

**Beat.** Candidate B, an Artifacts workspace tier beside the store. Candidate C, an agent-native coordination host with leases, a feed, a task API, and ref lanes.

**Evidence.** `design/AGENT-LAYER.md`, "Summary" and "Synthesis decision". `design/DESIGN.md`, "Summary" and "Synthesis decision". `history/arena/cloudflare-agents/SYNTHESIS.md`. `history/review/cloudflare-agents-judge.md`. `history/decisions.tsv` row 135.

**Flip.** The arena's flip condition said to start from C if leases, a live feed, and a task API were first-release must-haves. Operator answer 2 met it, and the next entry resolves it. The verdict also rests on the base's 300 commands a second gate and guessed git CPU constants, which Milestone 0's fleet replay and fan-out rows decide.

### Grafts onto A from C and B

**Decision.** Seven grafts sit on A. The evidence for all seven is `design/AGENT-LAYER.md`, "Synthesis decision", and `history/arena/cloudflare-agents/SYNTHESIS.md`.

1. From C, `ActorKind::Agent` refusals compiled into `decide` for Graftyard's Apps and any App an admin flags. Graftyard-run agents work in a workspace fork by default, or in-repo where an org forbids private forks.
2. From C, chain-resident provenance. Trailers and `refs/notes/ai` are parsed during `validate`, and session logs are blobs under the host-owned ref `refs/graftyard/sessions/`. That replaced A's session buckets, which would not survive a vault restore or reach the lifeboat.
3. From C, the advisory merge-tree conflict predictor on the second warm site, at most 8 pairs a push, under a compiled 50% CPU cap, paused while that site leads.
4. From C, the agent gate for Graftyard-run agents' pull requests, enforced in `Merge::decide` and bound to the head, and the burst and 5,000-agent Milestone 0 rows.
5. From B, neutral `graftyard/overlap` and `graftyard/provenance` check runs, so vendor agents see overlaps through the plain Checks API.
6. From B, package access in the Sandbox egress allowlist, limited to registries that never fetch upstream on demand.
7. From C, leases, a who-is-working-on-what feed, and a task API, after operator answer 2 (next entry).

**Beat.** Rejected with them: B's workspace tier, C's coordination Durable Object and its WebSocket feed, C's ref lanes, and Code Mode on by default.

**Flip.** None stated for the grafts.

### Leases and the task API are grafted onto A, not a rebase on C

**Decision.** Leases on paths and refs are chain state on the repository's group: fold rows written by `LeaseAcquire`, `LeaseRenew`, and `LeaseRelease`, live while the leader's entry time is before `expires`. A lease never expires because the host was down. The read side is a long-polled activity feed under `/api/graftyard/v1`, served from the group's log through a fold ring, with no WebSocket. The task API is the `AgentTask` row made public under `/api/graftyard/v1/repos/{owner}/{repo}/tasks`, with GitHub's agent task state names, never under `/api/v3/agents/*`. The controller uses the same public API. Leases are advisory unless a repository sets `AgentsExclusive`, which refuses agent pushes only, so a human can always push.

**Why.** A won on its git CPU accounting and its smaller surface, and neither depends on leases. A lease is a few hundred bytes about one repository, read with its refs. On the repository's group it gets RPO zero, the 11.5 s failover, the vault, the lifeboat, and `RestoredTo` for free. Actions job leases already live in a group and extend by the outage length, so agent leases follow that pattern.

**Beat.** Rebasing the layer on C. C's `RepoCoord` Durable Object per repository, which would add a failure domain with none of those properties. C's WebSocket feed. A controller-only task path under `/_graftyard/`. A new webhook event, which GHES clients would not subscribe to. C's ref lanes stay rejected, because the operator did not ask for them.

**Cost accepted.** Advisory leases by default, so two agents without leases can still duplicate work.

**Evidence.** `history/review/cloudflare-leases-tasks.md`. `design/AGENT-LAYER.md`, "Synthesis decision", "Alternatives considered", and "Tradeoffs accepted". `design/DESIGN.md`, "Synthesis decision". `history/FRAME.md`, answer 2. `history/decisions.tsv` row 163.

**Flip.** None stated. The layer's Milestone 0 leases row gates it: no overlapping Exclusive leases beyond the skew bound under leader kills, leases live across an outage, events reaching 500 long-pollers within 2 s at p99, and `AgentsExclusive` refusing conflicting agent pushes but no human push. Final pass items FX-9, FX-22, and FX-25 changed lease extension, Exclusive rules, and feed limits.

## Final pass

Three fresh read-only seats reviewed the whole design on 2026-10-26, after the leases-and-tasks change: a security reviewer (F1 to F15), a correctness reviewer (FP-1 to FP-15), and a consistency reviewer (G-1 to G-13). The lead read all 43 findings and sorted them into 19 act-on items, FX-1 to FX-12 for the base and FX-20 to FX-26 for the agent layer. None was dismissed. Two resolver writers applied them, and each design document's "Review resolutions" has a "Final pass" entry, one line per item.

**Evidence.** `history/review/final-interrogate.md`. `history/review/final-security.md`. `history/review/final-correctness.md`. `history/review/final-consistency.md`. `history/decisions.tsv` row 153.

### Exclusive acquires conflict only with Exclusive leases

**Decision.** An Exclusive acquire conflicts only with another holder's Exclusive lease. Shared leases come back as overlaps and never block it. Under `AgentsExclusive`, only task-held Exclusive leases refuse pushes.

**Why.** Under the earlier rule a Shared lease over everything could block every Exclusive acquire.

**Beat.** LT-ACT-1's rule, in which Exclusive conflicted with any overlapping live lease, and the reviewers' wording. This is the lead's change, not theirs.

**Evidence.** `history/review/final-interrogate.md` FX-22. `history/review/cloudflare-leases-tasks.md` LT-ACT-1. `history/decisions.tsv` row 153.

**Flip.** None stated.

### The document line caps rise to 1,400 and 850

**Decision.** `scripts/check_design.py` allowed 1,400 lines for `design/DESIGN.md` and 850 for `design/AGENT-LAYER.md` after the final pass. The implementability walk raised the caps to 1,900 and 1,000, and the third walk raised them to 2,200 and 1,100.

**Why.** The documents are now the implementation specification for other agents, so a missing type costs more than length.

**Beat.** The earlier caps of 1,100 lines for the base and 700 for the layer.

**Evidence.** `history/review/final-interrogate.md`, "Line caps". `history/decisions.tsv` rows 136, 153, and 168. `scripts/check_design.py`, the `cap` line.

**Flip.** None stated.

### Act-on items

Each row is one change. The evidence for every row is the item of the same id in `history/review/final-interrogate.md`.

| Item | Change |
| --- | --- |
| FX-1 | Signed `Release` entries bind to `(install_id, group, prev_digest)`, `Keys` carries a generation, and intents expire. No rollback goes below an enabled format. Restores become customer-key `RepoRestore` and `RepoReset` entries |
| FX-2 | The commit call splits into an admission receipt and an outcome wait, so queueing never looks like a dead leader. Revocation binds every command decided after it |
| FX-3 | `verify` replays the entry's events on a scratch transaction over the fold at k, instead of checking each `expect` against the fold alone |
| FX-4 | Leader-lease safety: transfers carry a `transfer_id`, `fresh` waits for the term's `Noop`, and reset quarantine is narrowed |
| FX-5 | Spare and residency sites get their own site days, gated by a canary group that replays current formats |
| FX-6 | A 35-day `ref_log` keeps GC from deleting restorable objects, archives commit before followers truncate, and a whole-install restore re-applies later revocations |
| FX-7 | Ids get a cell, block, and offset layout leased from a per-cell `Ids` group. `Names`, `Tokens`, and Actions events are defined. Stars, watches, follows, and notifications live on the actor's group with a derived `Inbox` object |
| FX-8 | Isolates cache `Release` pointers with a compiled TTL, never move backward, and never block a request |
| FX-9 | Lease and Actions job-lease extension uses a recorded stall window, never idle time. The base owns the rule and the layer cites it |
| FX-10 | The drain-only key cannot drain a second site while one is drained |
| FX-11 | The hot pool counts in `max_instances` and the Cost table, `CHILD_GROUP_AT` is 8 GiB of member SQLite, and `design/SHARED.md` states the format rule |
| FX-12 | Undefined base types such as `Health` are defined, stale reuse wording is fixed, and `Release` gains MCP and controller pointers and rolls |
| FX-20 | `AgentTask` gains `app`, `installation`, and `creator`. Minting checks `task.app`, terminal states are final, and each field has a tier |
| FX-21 | The org flag `agents_enabled`, off by default, records operator answer 1. Only a human with `contents: write`, or `graftyard-agent` from an issue whose latest `Assigned` entry names its bot, creates a Graftyard-run task, and the controller hears through a first-party-only delivery |
| FX-22 | A lease holder is `(actor, app, session)`. An Exclusive lease needs a live task and a literal first path segment, and lasts at most 4 h. Failed authorization, uninstall, and suspension drop leases |
| FX-23 | `validate` returns changed paths for `decide`, fork pushes read the base repository's chain, and one rule defines an agent App. Only `GraftyardSession`, `ClaudeSession`, `AmpThreadId`, and `AssistedBy` trailers classify a push as agent |
| FX-24 | `AgentGate` is satisfied only by an approval from a credential that is neither an `AgentApp`'s nor MCP-issued |
| FX-25 | Feed and task reads need `contents: read`, polls are authenticated and capped per token and per IP, and followers and learners serve the feed |
| FX-26 | Agent-layer event types get one block with tiers, stale statements are fixed, and `alternatives/AWS.md` gets the regenerated base-plus-agents cost row |

## Implementability walk

Two fresh read-only agents walked the folder as implementers on 2026-10-26: WalkBase over Milestones 0 to 3 (WB-1 to WB-20) and WalkOpsAgents over Milestones 5 and 6 and AM1 to AM3 (WO-1 to WO-20). The build plan writer added five sequencing gaps, R4 to R8. The lead acted on all 45 as items WK-P1 to WK-P10, WK-M1 to WK-M9, WK-R1 to WK-R14, WK-A1 to WK-A7, and WK-B1 to WK-B8, and decided five choices the walkers left open. Those five are the entries below. The walk also raised the caps in `scripts/check_design.py` to 1,900 lines for `design/DESIGN.md` and 1,000 for `design/AGENT-LAYER.md`, and kept `/api/v3/meta` in its GHES shape by serving SSH host keys at `/api/graftyard/v1/meta/ssh`.

**Evidence.** `history/review/walk-interrogate.md`. `history/review/walk-base.md`. `history/review/walk-ops-agents.md`. `history/review/walk-fix-reports.md`. `history/decisions.tsv` rows 159 and 160.

### Pointers pin only during a swap-back

**Decision.** Each pointer in the `Release` group's `Pointers` fold is `Option<VersionId>`. `None` lets the script's deployment split route the call, so a gradual deploy reaches its percentage. `Some(v)` pins every pinned call to `v`, which is the swap-back. Only a pre-signed undo (`EdgePointer`, `OpsPointer`, `McpPointer`, or `ControllerPointer`) sets `Some`, and the next committed intent that rolls the same script sets `None`. No forward pointer move exists, so no key signs one.

**Why.** A call pinned by a version override never takes the deployment's percentage split. A pointer that always named one version made every gradual edge, `graft-ops`, MCP, and controller deploy all or nothing, and no key was named to sign a forward move.

**Beat.** A `{ stable, canary, percent }` pointer with `graft-front` doing the split, and forward pointer moves signed by a named key.

**Evidence.** `history/review/walk-interrogate.md` WK-R3. `history/review/walk-ops-agents.md` G3. `design/DESIGN.md`, Shape, "Release group" (`Pointers`). `history/decisions.tsv` row 159.

**Flip.** None stated. The Milestone 5 row that submits an `EdgePointer` undo under a 1% `EdgeRoll` with the API blocked gates it (`design/DESIGN.md`, "Build order from zero").

### `Abort` names its target release

**Decision.** `Abort` is `Abort { intent, target: ReleaseId }`. `graft-ops` moves every site and pinned script the intent touched to `target`. The group refuses the `Abort`, and pages, when `target` cannot read an enabled format.

**Why.** The design computed `Abort`'s target as the newest release that reads every enabled format, while the Milestone 5 gate expected `Abort` to return every site to N, and the `FormatsEnable` row expected it refused. Two implementers would have built different `decide` logic.

**Beat.** A computed target, with a stated rule for when an abort is refused.

**Evidence.** `history/review/walk-interrogate.md` WK-R6. `history/review/walk-ops-agents.md` G8. `design/DESIGN.md`, Shape, "Release group", and "Install operations", Revocation runbook. `history/decisions.tsv` row 159.

**Flip.** None stated. The Milestone 5 gate and the `FormatsEnable` row in "Build order from zero" gate it.

### A task claim covers the task's App or the workspace App, on the repository or its workspace fork

**Decision.** A token's `task` claim is valid when its App is the task's App or the install's `graftyard-workspace`, and its repositories are the task's repository or that repository's workspace fork. Only `graftyard-workspace` pushes to the fork and writes session refs. Each of the four first-party Apps has a manifest with its permissions and events.

**Why.** In fork mode the push goes to `<org>-agents/<repo>`, a different owner with its own installation token, and the fork belongs to `graftyard-workspace`. The earlier rule, `task.app == jwt.app` on a listed repository, could not mint a fork token with a task claim, yet the token-ceiling row requires one.

**Beat.** A claim tied to one App and the base repository only.

**Evidence.** `history/review/walk-interrogate.md` WK-A2. `history/review/walk-ops-agents.md` G6. `design/AGENT-LAYER.md`, Shape (agent block and "First-party Apps"). `history/decisions.tsv` row 159.

**Flip.** None stated. The AM3 token-ceilings row gates it (`design/AGENT-LAYER.md`, "Build order").

### `graftyard apps init` registers the MCP and insights Apps in the install account

**Decision.** `graftyard apps init` runs in the install account in AM2. It registers `Graftyard MCP` and `graftyard-insights` and stores their keys and the MCP client secret in the install's Secrets Store. `graftyard agents init` registers only `graftyard-agent` and `graftyard-workspace`, in the agents account. A self-hoster without a controller still gets MCP and insights.

**Why.** `graft-mcp` runs in the install account and needs the `Graftyard MCP` client secret, and the insights consumer runs in the site scripts and needs the `graftyard-insights` key. AM2 came before AM3, yet only `agents init` registered the MCP App.

**Beat.** One `agents init` in the agents account that registers all four Apps.

**Evidence.** `history/review/walk-interrogate.md` WK-A3. `history/review/walk-ops-agents.md` G11. `design/AGENT-LAYER.md`, Shape ("First-party Apps") and "Deployment modes" ("Enable agents"). `history/decisions.tsv` row 159.

**Flip.** None stated. The `graftyard apps init` row in `design/AGENT-LAYER.md`, "Build order", gates it.

### Milestone 1 builds minimal identity

**Decision.** Milestone 1 builds the identity that real code needs: the `Ids` group, `Names` login and repository-name claims, an owner's `Authz` record with admin grants and `RepoName`, and stored-token authentication. Milestone 3 completes OAuth, Apps, installation tokens, SSO, and learners. No test-only resolver ships.

**Why.** The Milestone 1 gate pushes with unmodified git and the Milestone 2 gate runs `gh`, and both need to create a repository, resolve `o/r`, and authenticate a token, which Milestone 3 built.

**Beat.** A test-only resolver in Milestones 1 and 2, and moving all of Milestone 3 forward.

**Evidence.** `history/review/walk-interrogate.md` WK-B1. `history/review/walk-base.md` G2. `design/DESIGN.md`, "Build order from zero", "Milestones 1 to 7". `history/decisions.tsv` row 159.

**Flip.** None stated. How Milestone 1 creates its first accounts and tokens before REST and sign-in is open (`decisions/OPEN-QUESTIONS.md`, R14).

## Third implementability walk

A fresh read-only agent, WalkRest, walked Milestone 4, Milestone 7, AM4, and the `design/SHARED.md` sections the first two walks left out, because trail review 8 flags 2 and 5 found that gap. It found W1 to W15 and one minor gap. The lead acted on all 16 as items WK-X1 to WK-X13 and WK-Y1 to WK-Y3, with none dismissed. Writers X1, X2, and Y applied them, and each design document's "Review resolutions" has a "Third walk" entry. The entries below are choices the writers made beyond the item text, or inside it, that an implementer might reverse.

**Evidence.** `history/review/walk-rest.md`. `history/review/walk-rest-interrogate.md`. `history/review/walk-rest-reports.md`. `history/decisions.tsv` rows 167 to 169.

### A metadata split commits on the parent, and the parent keeps the feed

**Superseded in part, 2026-11-12.** See "A metadata split runs in band on the parent leader, keeps every commit on the parent, and reads the child narrowly": no 503 window, no `Moved` routing, and no `MetaSplitter` Workflow.

**Decision.** At `CHILD_GROUP_AT` the parent commits `MetaSplit { child: RepoMeta(repo), at }`, a core entry and the commit point. The child starts from the parent's metadata snapshot at `at` and decides nothing before `MetaSeed`, so metadata writes get 503 until then. A command that touches both tiers commits whole on the parent first, and `meta_finish` appends its metadata half to the child as an ordered `MetaHalf`. The parent keeps its log, indexes, feed ring, and issue-number counter.

**Why.** Child groups had no `ChainId`, commit point, or routing, which was the walk's one blocker (W1). Committing on the parent first keeps one commit point per command, so consumers deliver every event from the parent entry and a split changes no feed cursor.

**Beat.** Seeding the child before the split, which shrinks the 503 window but breaks the rule that the child starts from the snapshot at `at`. A child that owns the feed for metadata events, which would end pre-split cursors with 410.

**Evidence.** `history/review/walk-rest-interrogate.md` WK-X8. `history/review/walk-rest-reports.md`, WriterX2. `design/DESIGN.md`, Shape, "Chains" (`CHILD_GROUP_AT`).

**Flip.** None stated. The Milestone 7 split row measures the 503 window against 10 minutes [guess]. A longer window, or a product decision that minutes are too long, reopens seeding first (`decisions/OPEN-QUESTIONS.md`, G4).

### Actions runs shard by repository

**Decision.** `actions_shard(run)` is `hash(run.repo) % 16`. `Repo`-scope runners register on their repository's shard and `Owner`-scope runners on shard 0. An `Owner`-scope `RunnerBroker` reads all 16 shards from each shard's queued summary plus counted hints, and a job is handed out only after `Matched` commits on its shard.

**Why.** `/actions/runs` for one repository becomes a one-shard query, and a run id is found through its repository.

**Beat.** Sharding by run id, which spreads one repository's runs over all 16 shards.

**Evidence.** `history/review/walk-rest-interrogate.md` WK-X2. `history/review/walk-rest-reports.md`, WriterX1, "Decisions beyond the item text". `design/DESIGN.md`, Shape, "Identity" (`actions_shard`). `design/SHARED.md`, "Actions runs and jobs".

**Flip.** None stated.

### One `HookSequencer` per hook owner orders webhooks per (chain, hook)

**Decision.** The webhook delivery store is one `HookSequencer` object per hook owner on the owner's home site. It keeps one queue per (chain, hook) with one attempt in flight, and a retried delivery holds its queue. `HOOK_OWNER_CONCURRENCY` is 50 [guess], and hooks whose last attempt failed share at most half of the slots. The `Webhooks` cursor passes an entry once its `HookDelivery` rows are recorded, never on an attempt.

**Why.** One object per owner is what lets the store enforce the per-owner cap. Holding the queue on a retry keeps order per (chain, hook), and a slow hook delays only its own queues.

**Beat.** One object per hook, which cannot enforce an owner-wide cap.

**Evidence.** `history/review/walk-rest-interrogate.md` WK-X4 and WK-X6. `history/review/walk-rest-reports.md`, WriterX1. `design/SHARED.md`, "Event fan-out". `design/DESIGN.md`, Shape, "Identity" (`HookSequencer`).

**Flip.** An owner whose deliveries exceed one object's soft limit of 1,000 requests a second needs the store split by hook hash (`decisions/OPEN-QUESTIONS.md`, C16).

### Hosted runners use a first-party App and the existing JIT endpoint

**Decision.** `graft-runners` in the runner account authenticates as the install's `graftyard-hosted-runners` App, reads hosted demand from the `Owner`-scope broker, and calls the existing `generate-jitconfig` endpoint. `decide` refuses a hosted registration past the owner's cap, 20 [guess], so overlapping sweeps never start more runners than the cap. A user's `Owner` scope holds only hosted runners.

**Why.** The runner account reaches the install only through the public front, like a customer's runner, and needs no new credential type.

**Beat.** A new credential type for the hosted-runner service, and a cap enforced only inside `graft-runners`.

**Evidence.** `history/review/walk-rest-interrogate.md` WK-X7. `history/review/walk-rest-reports.md`, WriterX1. `design/SHARED.md`, "Actions" (Hosted runners). `design/DESIGN.md`, Shape, "Identity" (`graft-runners`).

**Flip.** None stated.

### The watcher index lives in each repository's metadata tier

**Decision.** The `Counts` consumer writes `watcher` and `thread_sub` rows to each repository's metadata tier from the actors' `UserRecord` entries. `/stargazers`, `/subscribers`, and `Notify` read them, and a refold rebuilds them.

**Why.** The index becomes chained state with a quorum, so it survives a region loss and needs no new object class.

**Beat.** A new derived Durable Object class for the index.

**Evidence.** `history/review/walk-rest-interrogate.md` WK-X9. `history/review/walk-rest-reports.md`, WriterX2, "Decisions beyond the item text". `design/DESIGN.md`, Shape, "Identity".

**Flip.** None stated.

### Search shards by residency domain, not by owner

**Decision.** `SearchShard { domain, shard, generation }` holds FTS5 tables in 16 shards per residency domain [guess], by repository hash, on the domain's first Full site under the domain's DO jurisdiction.

**Why.** A cross-owner query fans out to one domain's shards instead of to every owner's store, and residency follows the domain.

**Beat.** One search store per owner.

**Evidence.** `history/review/walk-rest-interrogate.md` WK-X10. `history/review/walk-rest-reports.md`, WriterX2. `design/DESIGN.md`, Shape, "Identity" (`SearchShard`).

**Flip.** None stated. FTS5 inside Durable Object SQLite, under the `eu` jurisdiction too, was confirmed on 2026-11-13 (`decisions/OPEN-QUESTIONS.md`, C15; `tests/m4/c15/`).

### Rate limits count in one `RateCounter` per key, with a stated error bound

**Decision.** One `RateCounter` Durable Object per `RateKey` holds the window total. Each isolate adds its local count every `RATE_FLUSH` requests, 20 [guess], or every second. So `x-ratelimit-remaining` overstates by at most 20 requests, or one second of requests, for each other isolate serving the key, and enforcement errs the same way. An isolate that cannot reach its counter counts locally against the full limit, and a lost counter refunds at most one window.

**Why.** Limits must not multiply with the number of isolates, and GitHub documents its own counts as approximate. A counter call on every request would add a Durable Object round trip to every API call.

**Beat.** An exact count with one counter call per request. WAF rate limiting rules, which count only by IP on Free and Pro and allow 1 to 100 rules per plan (`research/cloudflare.md`), so they cannot key by user, installation, or token [inference].

**Evidence.** `history/review/walk-rest-interrogate.md` WK-X13. `history/review/walk-rest-reports.md`, WriterX2. `design/SHARED.md`, "Public service concerns". `design/DESIGN.md`, Shape, "Identity" (`RateCounter`).

**Flip.** None stated. The Milestone 7 rate-limit row gates the bound.

### One `InsightVerdict` entry carries the verdict, its check runs, and its labels

**Decision.** The `Insights` consumer's cursor is the only producer of `InsightJob`, and nothing enqueues one from `apply`. `graftyard-insights` commits one `InsightVerdict` core row per ref through `POST .../insights`, and the same entry holds the changed check runs and labels as metadata. The "changed" check, the feed's `Verdict` event, and the agents pages read only that row. After a metadata split the row stays on `Repo(repo)`.

**Why.** One entry makes the "changed" check survive a crash and a redelivered job, and one producer means a leader change cannot send a second job for one entry.

**Beat.** `InsightJob` enqueued by `PrSync`'s apply, and check runs written as separate commands.

**Evidence.** `history/review/walk-rest-interrogate.md` WK-Y2. `history/review/walk-rest-reports.md`, WriterY. `design/AGENT-LAYER.md`, Shape (overlaps block).

**Flip.** None stated.

### Feed polls run only on following members, learners first, under `FEED_HELD_MAX`

**Decision.** Learners serve feed polls, and followers serve them only while no learner answers. A member holds polls only while it is following, which means its last AppendEntries left no gap, it is not catching up by `InstallSnapshot`, and it applied an entry within `FEED_BATCH` of the leader's `at` on it [guess]; a member that lags further answers its held polls and refuses new ones, as on a gap (2026-10-30). Each member holds at most `FEED_HELD_MAX`, 1,000 [guess]. At the first member that answers full, the edge answers 429 with `Retry-After` of `FEED_BATCH_ALL`, and a member that is not serving still sends the edge to up to two others (2026-10-30). If every member tried refused as lagging, the edge sends the poll to the least lagged of them only if that is a learner, which holds it under `FEED_HELD_MAX`, and otherwise answers 429 with `Retry-After` of `FEED_BATCH_ALL`, the value the full case uses (2026-10-31). The peak held count feeds `place_learners`.

**Why.** A feed poll must never run on a member that is not answering replication. "Following" by ack age was tried first and dropped, because followers on an idle group whose leader lease lapsed would stop serving the feed. Trying two more members after a full reply only moved the load: in LF-12, stopping at the first full reply cut feed calls on full members by 45 to 55% (344/s to 190/s per follower, 117 to 154/s to 52 to 78/s per learner), with client 429s within 8% and commit p99 at most 633 ms.

**Beat.** Defining "following" by ack age, and followers serving polls beside learners. Trying up to two other members after a full reply.

**Evidence.** `history/review/walk-rest-interrogate.md` WK-Y1. `history/review/walk-rest-reports.md`, WriterY. `design/AGENT-LAYER.md`, Shape (feed block). `tests/m0/results/m0-wp12.md`, LF-12 runs `r5` and `r6s`, and "Second-pass proposed design edits" 9. For the lag clause: L0-12 `t3` `human`, where a far follower fell 82 entries behind with no kill and feed p99 was 18.9 s, and `mcp`, a lagging follower for 60 s after a kill and p99 3.1 s ("Third-pass proposed design edits" 11); the threshold is unmeasured. For the last resort: L0-12 `t6`, all five kinds passed at feed p99 1,925 to 2,551 ms with 1,378 last-resort polls, every one on a learner and none on a voter, and LF-12 `r14` and `r15` held no poll on a leader ("Sixth-pass proposed design edits" 17). For the all-lagging `Retry-After`: in LF-12 `r27`, a client retrying 2 s after such a 429 turned an opening commit stall into 243,147 of them, 5,073 feed requests a second on one follower, 118 `overloaded` AppendEntries replies, and commit p99 7,699 ms (ninth pass). At 10 s, 5,000 pollers re-send at most 500 polls/s against 2,500/s at 2 s [inference: no tenth-pass run reproduced the stall], and 12 tenth-pass phases at 10 s showed no harm (`tests/m0/results/m0-wp12.md`, tenth pass, "Tenth-pass proposed design edits" 32).

**Flip.** None stated for the rule. The Milestone 0 held-poll row tests `FEED_HELD_MAX` (`decisions/OPEN-QUESTIONS.md`, A19). For the all-lagging `Retry-After`: an LF-12 run where a 10 s wait leaves pollers stale long enough to fail a feed line, or a storm at 10 s like `r27`'s.

## Milestone 0

Choices made while running Milestone 0 that change a gate or what a gate reads.

### The idle-cost row runs for 24 hours against hourly controls (2026-10-27)

**Decision.** The operator asked for a B0-15 cost estimate and said "I also don't want to wait a whole week", then approved the proposal with "get it done". The Milestone 0 idle-cost row now holds 100,000 idle groups for 24 hours across a UTC midnight instead of one week. For the run, the daily `Rtt` recheck is shortened to hourly, and 1,000 control groups that serve one request an hour run beside the idle ones.

**Why.** The row asks whether an idle group bills duration or runs an `Rtt` check. Neither depends on the length of the run, and the hourly recheck gives the rule 24 chances to fire wrongly instead of seven. The control groups must show billed duration and `Rtt` checks, so a zero on the idle groups reflects the groups, not a meter that does not report.

**Beat.** The one-week run. Its cost is about the same, since creating the groups is nearly all of it, but it takes seven days.

**Evidence.** `tests/m0/results/b0-15-estimate.md`: about $1.80 of Durable Object requests over the Workers Paid allowance, from the account's own usage in the B0-12 sample.

**Flip.** An idle cost that appears only after more than 24 hours, such as a weekly platform job that wakes objects, would be missed. Run the week if Cloudflare documents any process like that.

### Git cache shards run under the `durable_object` scheduling policy (2026-10-27)

**Decision.** Git cache shards and the SSH front start their containers from their own Durable Object with `ctx.container.start()`, under the `durable_object` scheduling policy. Each site caps its shard containers at `count + HOT_POOL` in the shard object, and each start checks the region the container reports and replaces one outside the site's container regions. A warm shard renews a 6 h inactivity timeout by alarm until 24 h after its last request. Each site-script version carries the git image as `git` and the previous release's as `git_prev`, and `ContainerDay` restarts shards onto the new image in steps, with a pre-signed `Rollback { target: Containers(site) }` that needs no API call.

**Why.** Cloudflare refused the `default` policy on this account: "This account cannot create applications with the \"default\" scheduling policy. Use \"durable_object\" instead." The operator confirmed it will not be offered: "the answer is no, move to number 2." Under `durable_object`, 90 of 90 containers started from hinted objects ran in the hinted region, and a new Worker version with a new image map restarted none of three running containers, which kept their startup image.

**Beat.** The `default` policy, which had a platform-enforced `max_instances`, a `regions` constraint, and application-wide rollouts.

**Cost accepted.** The cap and the region check are Graftyard code, not platform limits, so a bug there could exceed the cap or misplace a container, with the account's vCPU limit (1,500 when decided, 15,000 since: Containers limits page, 2026-11-15) and the 70% alarm as the only backstop. The policy is in beta. Every container process has root's capabilities, so file permissions inside the git image protect nothing. The design never relied on them, and each container starts with `enableInternet: false`.

**Evidence.** `tests/m0/results/b0-16.md`, `tests/m0/results/container-placement.json`, `tests/m0/results/container-survival.json`. https://developers.cloudflare.com/containers/configuration/scheduling-policy/ and https://developers.cloudflare.com/workers/wrangler/configuration/ ("Wrangler rejects every other Container application field for this policy, including `image`, `instance_type`, `max_instances`, rollout settings, and placement constraints"). `design/DESIGN.md`, Shape ("Git cache shards (derived)"), "Failure modes" (`git caches`), and Milestone 0 (the container placement row, B0-38 in `build/BUILD-PLAN.md`).

**Flip.** Cloudflare offers `default` to the account again, or B0-38 fails, which means the start check cannot keep a site's containers in its regions.

### Platform questions are answered by measurement (2026-10-27)

**Decision.** The operator does not expect a written answer from Cloudflare: "assume i won't hear from cloudflare on number 2. investigate and try to find the answers yourself." M0-WP11 answers its four questions from Cloudflare's documents, its postmortems, and measurements in the Milestone 0 account, and it is done when each answer is recorded with its evidence tag. B0-19 no longer waits for a written answer on scheduling.

**Why.** The questions decide a flip condition and two RTO and RPO figures, and an answer that never arrives would hold Milestone 1. Each one has a measurable form. Write confirmation time bounds how far the confirming followers sit, a binding call's headers show whether it crossed the front-line proxy, and per-repository storage shows whether forks share objects. Results (`tests/m0/results/wp11.md`): (a) a binding call carries no proxy headers; (b) followers stay in the object's region, because the slowest of 60 objects confirmed in 37 ms against 73 ms to the nearest other region; (c) a replacement starts only after 3 of 5 followers fence the old instance, so after a whole-region loss the object waits for its region; (d) Artifacts forks copy the source's full size.

**Beat.** Waiting for Cloudflare's written answers before Milestone 1, which the plan required.

**Cost accepted.** (a) stops at the request: the transport between colos is internal and undocumented, and no test can break Cloudflare's proxy. (b) rests on latency, so it shows where the three confirming followers are, not where the other two are. (c) rests on the documented fencing and on (b), not on an observed region loss.

**Evidence.** `tests/m0/results/wp11.md`, `tests/m0/results/wp11-quorum.json`. https://blog.cloudflare.com/sqlite-in-durable-objects, https://developers.cloudflare.com/durable-objects/platform/known-issues/, https://blog.cloudflare.com/18-november-2025-outage, https://blog.cloudflare.com/5-december-2025-outage. `decisions/OPEN-QUESTIONS.md`, C1, C2, C6, and A12.

**Flip.** A written answer from Cloudflare that contradicts a measurement replaces it. A later confirmation time above the nearest region pair reopens (b), and an incident where a proxy fault breaks binding calls reopens (a).

### Milestone 0 runs in one account (2026-10-27)

**Decision.** The operator said: "i'd also like to *not* create separate trial accounts and do it all in this one." Milestone 0 runs every row in the one throwaway account, with no second account and no trial account or zone. B0-17 measures the account-separated hop inside the account: a site Worker on a public hostname, called with `global_fetch_strictly_public` so the call enters through the front door like a call from another account, against a cross-script binding. B0-20, B0-21, and B0-24 run on zones added to the account, on the plans those zones carry, and a plan the account lacks is recorded as not run.

**Why.** Every row except B0-17 and the plan-dependent rows needs only one account. A cross-account call through a site Worker is a public HTTPS request that crosses the front-line proxy, and a strictly public fetch inside one account takes the same front-door path [inference: no page says a cross-account hop is routed differently]. A warm hop inside one colo measured 7 to 10 ms on `graft-m0-tools` `/hop`.

**Beat.** Three more accounts for the account-separated option and Pro, Business, and Enterprise trial zones, which the plan named.

**Cost accepted.** B0-17 does not test a real account boundary: per-account rate limits, account-scoped faults, and separate API tokens stay unmeasured. A push or SSH row on a plan the account lacks leaves that plan's claim [unverified] until an install on that plan runs it.

**Evidence.** `build/BUILD-PLAN.md`, M0-WP1, M0-WP5, M0-WP6, and M0-WP7. `design/DESIGN.md`, "Milestone 0".

**Flip.** The operator adds an account or a trial zone, or B0-17 lands within 10 ms of its 60 ms fail line, where a real account boundary could change the verdict.

### The edge fills a new tip's clone-cache entry when a push commits (2026-10-29)

**Decision.** When a push to a network with CI traffic commits, the edge asks the network's shard to fill the clone-cache entry for the new tip at the depth its CI clones used last. The shard runs one `upload-pack` per key, however many requests arrive for it, and writes the result to the bucket. Clones that still miss stream from that one run.

**Why.** B0-23 at 33 clones a second, with the tip moving 10 times a minute: every clone that misses goes through the network's shard object, which relays a 40 MB response, and a new tip takes 1.5 to 2.4 s to reach the bucket. With clones at the instant of the push, the object refused up to 44% as overloaded. With the fill on push and clones 4 to 8 s later, 100% were served from R2, at time-to-first-byte p99 406 ms.

**Beat.** Relying on the first CI clone to miss and fill, which passed only when clones lagged the push by 5 s or more (97.5 to 97.8%). A create-only claim object per key, so only one edge calls the shard and the others poll the bucket: no failures, but p99 1.6 to 41.7 s for the waiting clones. Limiting streams per key inside the object: the refusal happens in its queue before its code runs.

**Evidence.** `tests/m0/results/m0-wp6.md`, B0-23.

**Flip.** A replay where CI clones arrive before the fill lands and the object refuses them, which would add the claim with a shorter poll. A network whose CI depth varies, which would fill more than one depth.

### The clone cache fills from the shard (2026-11-15)

**Decision.** On a clone-cache miss the edge names the entry's key to the shard (`fill=<CloneKey digest>`) and streams the shard's answer to the client without keeping a copy. The container writes the whole answer to a spool file as well as to the client, and goes on writing after the client stops reading. It keeps the spool only if `upload-pack` or the composed answer ended cleanly with a flush and no `ERR`. The shard object then streams the spool into the home site's bucket under `R2Key::CloneCache` with replica 0, through a `FixedLengthStream` of its length, so R2 stores it only when the whole length arrives. One fill per key runs at a time on a container; a second request for the key in that time is served and not spooled.

**Why.** LM-1 round 24c found 0 `clone/131296/` entries in R2 after B0-34. The edge wrote its tee to R2 only when the client had read the body to the end, and B0-34's first-byte probe cancels after the first byte, so every probe missed. Each miss then ran a stock depth-1 upload-pack at about 25 CPU-s and 1.1 GB (`tests/am1/results/lm1-memory.md`, branch `lm1-memory`, "Round 24c"). The tee also buffered the whole answer in the edge isolate before the put.

**Beat.** Keeping the edge's tee and reading the store side to its end after the client goes: the edge's `wait_until` would hold a 300 MB body in wasm memory. Filling on push only (`The edge fills a new tip's clone-cache entry when a push commits`): it covers CI's depth on the new tip, not every miss.

**Evidence.** `TestCancelledFillStillSpoolsTheWholeAnswer` (`containers/git/server/compose_test.go`): a stock fill and a composed fill whose client cancels after one byte each leave the whole answer spooled with its length, and a second fill of the key is not spooled. The end-to-end check, an entry in R2 after a cancelled clone, is LM-1's next B0-34 run.

**Flip.** A hosted B0-34 that still leaves no entry after cancelled probes, or spool files that fill a shard's disk.

### Pushes over the plan's body limit get Cloudflare's 413 (2026-10-29)

> "sure, whatever you think is best"

The operator left the question, raised from M0-WP6 as G5 and now closed, to the run.

**Decision.** An HTTPS push over the zone plan's request body limit (100 MB on Pro, 200 MB on Business) ends in git's `error: RPC failed; HTTP 413`. The edge's own cap still refuses pushes between it and the plan limit with a `report-status` that names SSH on Enterprise, and on Pro and Business the `cloudflared` SSH option if the install has it on, else Enterprise or splitting the push into smaller pushes. The install docs, the push-size section of the admin guide, and the 413 runbook name the same paths for larger pushes, and install preflight still refuses a plan below the largest imported pack. They named SSH until B0-24 found native SSH needs Enterprise (`tests/m0/results/m0-wp7.md`, 2026-10-31), and the operator then added the SSH option on Pro (`FRAME.md`, "Operator decision, 2026-10-31, SSH ingress").

**Why.** Cloudflare answers 413 while git is still sending, before git reads any response, so no Worker response can reach the client. Chunked bodies are cut at the same limit (`tests/m0/results/m0-wp6.md`, B0-20 and B0-21).

**Beat.** Requiring Enterprise for BYOC (G3), which would make the limit 5 GB at a cost most BYOC customers would not pay for this alone. Asking git users to lower `http.postBuffer`, which does not change when git reads the response.

**Evidence.** `tests/m0/results/m0-wp6.md`, B0-20 and B0-21, measured on workers.dev at 100 MB, then on the Pro zone `graftyard.dev` with the same result: git saw the edge's message naming SSH for a 90 MB push against an 80 MB cap, and a bare 413 for a 120 MB push.

**Flip.** A Pro or Business zone run where the edge's response reaches git above the plan limit, or a Cloudflare setting that lets a Worker answer before the body ends. For the message: a Business run where Spectrum reaches a Tunnel origin, which would let it name native SSH there, or B0-24a failing, which would drop the SSH option from it.

### `STEP_DOWN`'s confirm rule needs six slow confirmations, and `INLINE_MAX_HOT` is 64 KiB (2026-10-30)

**Decision.** A leader steps down on storage confirm latency only when at least 6 confirmations in the 10 s window pass `confirm_p99_ms`, and a `STEP_DOWN` transfer never targets a follower inside `RESET_QUARANTINE` of its own step-down. On a confirm trip a target qualifies on the leader's own calls to it under `commit_errors`, with no probe. On a commit-error trip, where the leader's calls fail over the line to every follower by definition, it qualifies on the other follower's `linkErrors` probe of it under `commit_errors`, and a probe that cannot run disqualifies it. Among those the fewest failed calls wins, then the most caught up, and if none qualifies the leader holds. The commit-error rule also needs at least `commit_fail_min`, 3, failed calls to every follower in the window (operator delegation, 2026-10-30 and 2026-10-31). A failed call whose deadline elapsed while the leader's own storage confirm was outstanding (begun before the call and unconfirmed at its deadline) counts toward neither `commit_errors` nor `commit_fail_min`, because the confirm rule owns the leader's storage stalls (operator delegation, 2026-10-31). A hot group inlines packs only up to 64 KiB while placement succeeds. The `DEMOTE_PER_MIN` count per actor lives in the group's fold.

**Why.** M0-WP9 found every leader change in its kept runs, 7 in all, came from `STEP_DOWN` with no fault injected. 6 were on the confirm rule: a journey 9 group commits about 5 entries a second, so a 10 s window holds 40 to 80 confirmations and its p99 is its maximum, and one write of 535 to 1,550 ms moved the leader. Those changes fail B0-33. At 256 KiB, a burst's first entries held 1.3 MB of inline packs, and merges waited behind them up to 2.3 s, over B0-32's 2 s line. M0-WP4 then found that Durable Object storage stalls on its own: bare objects with no Raft code hit 3 or 4 stalls over 500 ms in one 10 s window, so a count of 3 tripped healthy leaders (F35), and with 6 B0-06 passed 9 of 9. Without the quarantine on the target, two healthy sites passed a group back and forth while a third was sick. A count in leader memory reset on a leader change and let a fast user token's writes go Human for a minute.

**Beat.** A p99 over a longer window, which would also delay the step-down a gray site needs (B0-06). A minimum of 100 samples, which on a quiet group would never trip. Keeping 256 KiB and accepting merge delay during bursts.

**Evidence.** `tests/m0/results/m0-wp9.md`, "STEP_DOWN under load", B0-32 (`-r1`, `-r2`, `-ih64r1`), and B0-33; `tests/m0/results/m0-wp4.md`, "F35". The 64 KiB cap passed in one burst run. In the fourth pass no leader stepped down on the confirm rule in ten runs, but in B0-33 `p4r4` the quarantine left SIN, whose calls had failed 60% of the time, as the only target, and SIN, overloaded as leader, lost every remaining commit; a bar on the target comes from that run (`tests/m0/results/m0-wp9.md`, fourth pass, "Proposed design edits (fourth pass)" 30). Judging targets on the leader's calls to them, as first written, could never transfer, because the commit-error trip needs both followers over `commit_errors`: in the fifth pass `p5r1` and `p5r2` logged 26 holds and 0 transfers, and the leaders were lost by election instead. The probe reading keeps a commit-error transfer and still refuses a target like `p4r4`'s SIN (fifth pass, "Proposed design edits (fifth pass)" 34). Commit errors that only log was the other choice, and lost because it leaves a sick leader in place until `suspect` reports remove it. The target's own probe of the other follower, adopted next, measures that follower too: after an election left a failing-write site as a follower, it qualified only the sick site, and a group moved onto it three times. The rule as landed in M0-WP4 (`src/raft/member.ts`) passed B0-06 12 of 12, 6 of them failing-write runs, with leaders off slow storage in 2.3 to 5.6 s, off failing calls in 2.4 to 5.2 s, and off failing writes by election in 4.9 to 13.3 s, and `dr-writes2` moved to C rather than to the sick site. Requiring both the leader's calls and the probe on every trip, as first described, leaves no target on a commit-error trip, so the leader's-calls bar applies to confirm trips only. A sick prober can refuse a healthy target, which makes the leader hold, never transfer onto the sick site. `commit_fail_min` 3: leader kills under load gave 0 follow-on changes in 90 groups against 2 in 90 without it, and leaders still left a site failing 30% of calls in 2.9 to 4.1 s and slow storage in 2.8 to 4.2 s (`tests/m0/results/m0-wp4.md`, "`STEP_DOWN` target on the leader's calls and the other follower's probe; F37 as `min3-`", proposed edits 1 to 4; `tests/m0/results/m0-wp12.md`, sixth pass, edit 19). Under M0-WP9's load the count neither decided nor blocked anything: in the eighth pass's B0-29 to B0-33 runs the one commit-error trip had 6 and 3 failed calls, so its evidence stays with M0-WP4 (`tests/m0/results/m0-wp9.md`, eighth pass, "Proposed design edits (eighth pass)" 47). For the own-stall exclusion: in LF-12 `r28` placed-written and `farhold-r30` placed, one leader confirm of 3,465 and 3,605 ms made every failed call to the near follower in the window, and the commit-error rule tripped on 3/3 failed calls and moved a healthy group, in `r30` onto the far member NRT [measured, `tests/m0/results/m0-wp12.md`, ninth pass]. Replayed offline on those records, the exclusion removed 3 of 3 and 4 of 4 failed calls to the near follower, so neither would have tripped [inference: the effect is not run]; in the tenth pass it was inert in 26 phase-members, because no confirm passed CALL_DEADLINE (`tests/m0/results/m0-wp12.md`, tenth pass, "Tenth-pass proposed design edits" 31).

**Flip.** A B0-06 rerun where the 6-write rule leaves a gray-storage leader in place past 20 s. For `commit_fail_min`: a B0-06 or B0-31 run where a leader on a failing site collects under 3 failures per follower in a window and stays past 20 s; failing writes and quiet groups (under about 10 calls per follower a window) were not run with it. A B0-31 run at 64 KiB with commit p99 over 1 s from staging every push larger than 64 KiB, or R2 Class A writes that move `scripts/cost_agents.py`'s total. For the own-stall exclusion: a gray-leader case, B0-06 slow storage or failing calls, where the exclusion delays the move past 20 s.

### GC and the writes it races each name the index they read (2026-10-30)

**Decision.** GC's `Compacted` names the index its roots were read at, and `decide` refuses it if any ref changed after that index. A push that names objects it does not send, and `RestoredTo`, name the pack-set index their connectivity walk read, and `decide` refuses either if a `Compacted` committed after it. GC resubmits its packs when no object it would delete became reachable, and replans otherwise.

**Why.** GC and these writes each read state and then commit, and `decide` cannot read R2 to recheck. M0-WP8 raced them with random timing: with every guard on, 0 objects were lost in 145 trials, 89 of which had a write commit inside GC's window. Without the `Compacted` guard, 48 of 80 trials lost 55 objects. Without the push guard, 13 of 80 lost one each. Without the `RestoredTo` guard, 3 of 40 lost one each.

**Beat.** Leaving G6 open on the grounds that GC is rare. A lock between GC and writers, which would stall pushes for a whole GC.

**Evidence.** `tests/m0/results/m0-wp8.md`, G6. Reruns: 61% of guarded GCs reran under the test's deliberately dense writes, at most 3 attempts.

**Flip.** A busy repository where GC reruns so often that it does not finish within its day, which would add a short ref freeze for GC's commit.

### Clone-cache misses are composed from the cached tip (2026-10-30)

**Decision.** When a depth-1 fetch misses the clone cache for a commit whose tree is close to a cached tip's, the git cache shard composes the answer. It sends the tip's cached depth-1 pack plus a pack of the wanted commit's objects missing from it, under one pack header and trailer. For each tip move, the shard that took the merge builds one part, the new tip's objects the base and earlier parts lack, once per network, stored beside the clone cache's tip pack for other shards and the second warm site to fetch. Every 20 moves base and parts fold into a new base. A miss composes base, parts, and a pack of the wanted commit's own new objects, and builds `refs/pull/N/merge` on the last tip whose part exists, so it never packs a tip move. A shard serves a hot network only once it holds the parts up to the tip. `scripts/cost.py` prices a CI or agent clone miss at 0.75 vCPU-s, plus one stock depth-1 build per network every 20 tip moves at 45 vCPU-s, an upper bound on the measured 1.2 to 1.7 CPU-s fold, and `scripts/cost_agents.py` adds part builds at 0.5 CPU-s a move once per network.

**Why.** B0-34 measured a stock depth-1 miss of torvalds/linux at 25 to 46 CPU-s, most of it delta search and compression over 103,000 tip objects. At journey 9's 1.5 misses a second that is 38 to 69 vCPU, which no k of at most 8 `standard-4` shards can carry, and run `b0-34-r1` saturated 7 shards with human push p99 40.5 s and 43% of agent pushes shed. A composed miss cost 0.72 to 0.74 s with time to first byte 0.35 s, and renewing a base 20 commits behind cost 2.7 s. With composed misses (`b0-34-r2`), human push p99 stayed at 0.68 to 1.56 s in every phase. The third pass showed that renewing every 20 moves leaves two gaps. A miss on the current tip packs a move whose part is not built yet: on Linux one merge added 2,300 objects, and each concurrent miss then cost 9 to 19 CPU-s and saturated the shard (`b0-34-r3`, `-r5`, `-r6`). A shard behind the tip serves from a stale base, at 17 to 75 CPU-s a miss (`b0-34-r4`). With per-move parts, catch-up before serving, and misses on the last built tip (`b0-34-r7`), a part cost 0.3 to 2 CPU-s (up to 10 for the 2,300-object merge), a fold 1.2 to 1.7 CPU-s, a miss 0.2 to 0.5 CPU-s, and clone time to first byte p99 was 0.70 to 1.03 s by phase.

**Beat.** Stock `upload-pack` misses, at the measured cost above. `pack-objects --window=0`, which cut a miss to 9.1 s for a pack 0.5% larger and still costs 12 times a composed miss. Pricing misses at the old guessed 1.0 vCPU-s, which hid the problem. A per-miss diff against a base up to 20 moves old (`b0-34-r3`), and parts built by each shard for itself at low priority (`b0-34-r4`, `-r5`).

**Evidence.** `tests/m0/results/m0-wp9.md`, B0-34: `results/raw/wp9-b0-34-gitcpu-r1.json`, `wp9-b0-34-r1.json`, `wp9-b0-34-r2.json`; third pass, runs `b0-34-r3` to `-r7`, and "Proposed design edits" 25. The composed pack, and base plus two parts plus a merge commit, passed `index-pack` and fsck locally, with a tree equal to the tip's.

**Flip.** A client whose `index-pack` refuses the composed pack, or a network whose agent checkouts are far from any cached tip. For the per-move rules: part builds that fall behind the tip at a network's merge rate, so misses wait on stale tips, or a measured CI cost from merge refs built on the last built tip rather than the current one.

**As built (2026-11-15).** The container builds and serves the chain (`containers/git/server/compose.go`), and the shard object readies it (`graftyard_git::compose`, `ShardHost::ready_chain`, `workers/site/src/shard.rs`, `Compose`). Where the code departs from the text above:

- Only a fetch with one want, `deepen 1`, no haves, no filter, SHA-1, and `ofs-delta` is composed. Everything else gets stock `upload-pack`, and so does a want that is neither a ref's tip nor behind HEAD, a want whose tree differs from the chain tip's in more than 20,000 entries, a shard with no chain, and `include-tag` when an annotated tag points at the want. Correctness first: stock is always there. Since round 26 (below) a chain behind HEAD and a want behind HEAD compose too.
- The builder is the home site's first shard, after each catch-up that moves HEAD, not "the shard that took the merge". Direct pushes move the tip with no merge, and merges run on the leader's shard, which may be another site's. Other shards install the chain the first warm bucket's `head` names and keep their old chain while a file is missing.
- `refs/pull/N/merge` is not built on an older tip. PrSync keeps its mergeable semantics, building it on the current base; the pull head's miss is composed from the chain at HEAD plus the head's own new objects.
- R2 holds, per network, `compose/<net>/base-<tip>` and `compose/<net>/part-<from>-<to>`, each a `.pack` and `.idx`, and `compose/<net>/head`, which names the chain and is written last.
- The chain step is its own background job per network. Only a composable depth-1 upload-pack on a shard that has not yet seen a chain waits for it, at most 20 s [guess]; once it has, the miss starts the step and goes on. Validates and catch-ups never wait.
- The chain's builds run at nice 19 (`gitLow` on the CPU meter), as the repack does, so they yield the vCPUs to validates and catch-ups on the same shard (LM-1 round 24c); the composed answer a client waits on runs at normal priority (`TestChainBuildsRunAtNice19`).
- The builder builds the base at warm, in the background on the first catch-up of a network with no chain, so a cold first miss, B0-34's cold probe, usually finds a chain rather than waiting out the 20 s for stock. A base costs about 11 CPU-s and a 300 MB put per warm bucket per network, paid once per base and again at each fold. A fold is a new base, so its put is the same 300 MB, but it measured 1.5 CPU-s because it reuses the chain's deltas (linux, `tests/m0/results/composed-miss.md`).
- Measured on a linux fixture (6.46 GiB, 12.0M objects, 60 first-parent moves; `tests/m0/results/composed-miss.md`): base 10.2 to 11.1 CPU-s, part p50 0.02 and at most 3.2 to 3.5 CPU-s, fold 1.49 to 1.58 CPU-s. A composed miss sent its first pack bytes at 0.011 to 0.018 s and finished in 0.77 to 0.97 s with 0.01 to 0.02 CPU-s of git work. A stock miss sent its first 64 KiB of pack at 5.0 to 5.1 s and finished in 15.4 to 15.6 s at 29.9 to 30.2 CPU-s. M0-WP9 had 0.72 to 0.74 s and 0.35 s.

**Flip (as built).** A hosted B0-34 where composed misses still fall back to stock for most CI clones (`x-graft-compose` names why), or where a part build after a big move holds the home shard long enough that misses on other shards wait past their bound.

**After LM-1 round 26 (2026-11-16).** Round 26 ran composed-miss hosted (`tests/am1/results/lm1-memory.md`, "Round 26"). Of 51 misses 36 composed, and 50 of 51 had an R2 entry within 60 s, cancelled probes included. Three things changed:

- **Hits stream.** Every one of 414 clone-cache hits answered 500 with no `x-graft-clone-cache` header. The edge read each 263 to 271 MB entry whole with `.bytes()`, past the isolate's 128 MB [inference: the events carry no message]. The edge now answers a hit with R2's body stream and the object's `content-length` (`graftyard_git::host::cached_response`). In local workerd a 300 MiB entry left the isolate's wasm memory at 1 MiB, against 301 MiB read whole (`tests/composed/bed/run.sh`).
- **A chain or a want behind HEAD composes.** 9 misses went to stock as "chain behind HEAD" and 6 as "not a ref tip". In the R2 listing each move's part landed 17 to 27 s after the first miss on its tip (once 79 s), while the tip moved about every 13 s, so a shard that installs from R2 was one or two moves behind HEAD for most of the phase; and the 6 wants were main's tip a moment before a move. Composing from the chain's tip plus the want's diff, packed with no delta search, costs a linux miss 0.26 s to its first byte 10 moves from the chain tip and 0.84 s at 30 moves, and the client's `index-pack --strict` takes it (`tests/m0/results/composed-miss.md`). Stock upload-pack already served such wants (`allowReachableSHA1InWant`). A want that is neither is still stock.
- **A composed miss waits for neither the lock nor a catch-up.** Composed misses got their first byte in batches, in the probes' log: 5 that arrived at 23:49:20 to 23:49:23Z at 23:49:28.9 to 23:49:30.1Z, and 5 at 00:03:41 to 00:03:45Z all at 00:04:34Z, the second batch with 3 stock misses that ended at the same second. A batch that ends together is a queue, and two were on every miss's path: on the container, a composed miss took the network's lock shared, so it waited behind any writer queued after a stock answer, which held the lock to the end of its spool even after its client went (round 26's stock misses ran 10 to 210 s); on the shard object, every upload-pack first waited for a catch-up, and one ran per tip move [inference: no log names which queue each batch was in]. Now the shard object asks the container for a composed answer first (`compose=only`) and waits for a catch-up only for stock, the container composes without the network's lock (it reads objects and refs only, and opens the chain's packs before it sends), and a spooled stock answer lets go of the lock once its client goes rather than at the spool's end.

The split of round 26's composed first-byte p50 8.5 s and p99 68.7 s between these causes is not measured: its probes logged only the total, and the shard object logs no time to the container's first byte. The first 12, at the warm tip with nothing ahead of them, took 1.4 to 12.0 s, against 0.01 s locally. Container memory at 11.9 of 12 GiB during the repack (LM-1's) shared those minutes.

So each miss now logs one line from the shard object, `shard miss <net> <CloneKey digest>: {...}`, with its arrival in epoch ms and, in ms after it, when `Compose::ready` returned, the `compose=only` answer (status and `x-graft-compose`), the catch-up before stock, the serving answer's headers, and its first chunk on to the edge, and `answer`, its `x-graft-compose` (`graftyard_git::compose::MissLog`). The container sends its headers with its first byte, so `headers_ms` less the step before it is the container's work, `first_ms` less `headers_ms` is the shard object's own transit, and the probe's time less `first_ms` is the edge and the network. The line is written when the first chunk goes on, through `graftyard_git::host::on_first_chunk`, which passes chunks on as JavaScript values (a 300 MiB answer left the wasm memory at 1 MiB in `tests/composed/bed/run.sh`, `FWD=1`), or when the miss ends without one.

**Flip (after round 26).** A hosted B0-34 where composed misses still wait seconds for their first byte with the lock and the catch-up out of their path, which would put the time in the container's own work or in the transit through the shard object; or a want behind a lagging chain whose composed answer the client refuses.

### Agent pushes over `INLINE_MAX_HOT` are refused while placement fails (2026-10-30)

**Decision.** While `PackStore::place` fails or a bucket's readiness probe is red, a hot group's agent pushes over `INLINE_MAX_HOT` get the agent refusal at once and are never queued. Human and MergeQueue pushes keep `INLINE_MAX` (1 MiB). B0-31's fail value becomes any Human or MergeQueue push up to 1 MiB refused while placement fails.

**Why.** One 1 MiB pack fills an entry, because `EntryBody` holds at most 1.5 MB, so a group commits at most one such push per round. From an apac leader with one entry in flight, B0-31 committed only 1.3 to 2.0 MB/s of 1 MiB inline pushes against journey 9's 4.37 MB/s. The queue filled to `AGENT_QUEUE_MAX`'s 64 MiB, agent commits waited 21 to 30 s, and 1,348 to 1,753 of 2,500 pushes were refused anyway, while Human and merge entries rode behind 1 MiB rounds at p99 1.3 to 2.0 s and 1.3 to 3.3 s. Pinning admission capacity did not help (`holdnopcap`), because the entry round is the limit. Refusing agents at once keeps rounds small for the classes that must commit.

**Beat.** Letting `INLINE_MAX_HOT` yield to `INLINE_MAX` for every push, as the design said, which fails B0-31 in all 4 placement-failing runs. Keeping the row's original fail value, which cannot pass at journey 9's rate on `Tri`.

**Evidence.** `tests/m0/results/m0-wp9.md`, B0-31, runs `b0-31-nop`, `-sd3nop`, `-holdnop`, and `-holdnopcap`, and "Proposed design edits" 10 and 11.

**Flip.** A placement-failing run where the group carries journey 9's 1 MiB agent pushes inline with Human p99 under 1 s, for example with more than one entry in flight, or an outage long enough that refused agents cost more than the delay.

### Both warm sites size a hot network's k from the larger load (2026-10-30)

**Decision.** `fanout` sets k at each warm site from the larger of the two sites' `NetworkLoad`, so the second warm site already holds k shards when the first is lost.

**Why.** In B0-34 `NetworkLoad` was per site, and only the home site was loaded, so the second warm site held k = 1. When the home site was killed, all load went to that one shard until `fanout` raised k, and each new shard first had to copy 8.76 GB: 166 to 207 s of rehydrate, with miss time to first byte at 19 to 20 s p99 for about 270 s. `scripts/cost_agents.py` already prices `(k - 1) * WARM_SITES` extra shards, so this rule makes the design match its price and adds no cost.

**Beat.** Sizing each site from its own load, which is what failed B0-34's time-to-first-byte gate inside the kill window. Counting only cache hits in the gate, which passed in every phase but hides the miss latency an agent CI job sees.

**Evidence.** `tests/m0/results/m0-wp9.md`, B0-34, run `b0-34-r2`, and "Proposed design edits" 17.

**Flip.** A network whose second warm site's extra shards cost more than the miss latency they prevent, or a rehydrate fast enough (under the time-to-first-byte gate) that raising k after a site loss is enough.

### Task commands do not demote, and the prompt reads comments as of the snapshot (2026-10-30)

**Decision.** Task API commands (create, cancel, and `PATCH` of an `AgentTask`) do not count toward `DEMOTE_PER_MIN`, so a demoted actor at task creation is one past `DEMOTE_PER_MIN` at the create's own entry without counting task commands. A Graftyard-run task's prompt reads writer comments as of the task's `snapshot`, not as they are when the Sandbox starts.

**Why.** `DEMOTE_PER_MIN` exists to class a fast actor's writes as Agent. Task creates and cancels are writes, so a human writer who created more than 60 tasks a minute demoted themselves, and the 61st create was refused as demoted: a local L0-14 trial refused 39 of carol's 100 control creates. A human creating tasks quickly is still a human, and the task caps (`TASKS_OPEN_PER_REQUESTER`, `TASKS_OPEN_PER_REPO`) already bound how many they open. For the prompt, the prototype read the issue at `snapshot` but comments as they were later, so a writer's comment after assignment could change what the agent was told with no pause and no record of which version it saw. Reading both at `snapshot` makes the prompt one recorded state, and a writer who wants the agent to see a new comment reassigns, which moves `snapshot`.

**Beat.** Counting task commands, with demotion measured at the create's own entry, which still refuses a fast human. Reading comments live, which lets the prompt change silently after assignment.

**Evidence.** `tests/m0/results/m0-wp13.md`, "Proposed design edits" 5 and 12; the local L0-14 trial (not kept) and the driver pacing in `run/agent-lib13.ts` that kept writers under 55 writes a minute. Edit 12 is [inference].

**Flip.** For task commands: an abuse case where a user token bulk-creates tasks to load a group. For the prompt: harness feedback showing that agents need to see writer comments made after assignment.

### Decisions under the operator's delegation, 2026-10-30

> "Stop waiting on me besides for the pro account. Do everything else and make the decisions. You are the pro."

Each decision below was made by the run under that delegation (`FRAME.md`, "Operator decision, 2026-10-30, delegation").

#### B0-07 counts idle groups from their next request

**Decision.** B0-07 passes when every group led on C that served a request or committed an entry in the 50 s before the drain moves within 20 s of it, and every other group led on C moves within 20 s of its next request.

**Why.** An idle leader has no request to learn of its drain on and no client to hurt, so the time that matters for it is from its next request. Its drain alarm runs for 60 s after the group's last activity, and a 50 s window keeps a group idle for 59 s from counting as active after its alarm stopped (group `-410`, 29.3 s). Over runs 11 to 14, 125 of 126 groups pass the 50 s form.

**Beat.** The gate as written, which an idle group fails through no fault the drain can fix. An alarm on every idle group, which would bill duration on idle groups that B0-15 measured at zero.

**Evidence.** `tests/m0/results/m0-wp4.md`, B0-07 runs 11 to 14.

**Flip.** An idle group whose first request after a drain waits more than one lease, which would mean the on-request hand-off is too slow for the client it serves.

#### B0-33's Human and merge-queue line is 1 s

**Decision.** B0-33 passes with Human and merge-queue commit p99 at most 1 s.

**Why.** B0-01 measured `Tri` commit p99 at 399 to 690 ms set by the round to a far follower, and the operator set its fail line at 1 s (`decisions/DECISIONS.md`, "Keep `Tri` and gate commit p99 at 600 ms"). B0-33 measured Human and merge-queue p99 of 0.85 to 1.1 s, also the round to a far follower carrying large entries, not queueing: those classes always went into the next entry. A 600 ms line would fail on network distance that admission cannot change. The other B0-33 conditions (no leader change, no `suspect`, at least 140 agent commands/s admitted) stay.

**Beat.** Accepting the miss with the line at 600 ms, which leaves a gate that cannot pass on `Tri`.

**Evidence.** `tests/m0/results/m0-wp9.md`, B0-33; `tests/m0/results/m0-wp2.md`, B0-01. The fourth pass (`p4r1` to `p4r6`) showed Human and merge-queue p99 of 1.15 to 1.49 s even with 0 leader changes, from entries of 84 to 96 agent commands and 450 KB of inline packs; the line stays at 1 s and `AGENT_INLINE_ENTRY` is the answer ("Agent inline bytes per entry are capped"). The seventh pass, with the budget on every group, held 746 to 943 ms in 3 of 3 runs, 57 to 254 ms under the line and least in the first 30 s.

**Flip.** A rerun with `INLINE_MAX_HOT` at 64 KiB where Human and merge-queue p99 comes in under 600 ms, which would restore the tighter line.

#### A PR with no recorded requester needs one more human approval

**Decision.** `AgentGate` on a PR whose `AgentMeta.requester` is empty needs one human approval more than `extra_human_approvals` asks.

**Why.** A confined App may open a PR without `X-Graftyard-Agent-Session`, and then the gate cannot exclude the requester, who could approve their own agent's work. Requiring the header would break vendors that never create tasks, which journey A2 says need zero Graftyard-specific client code.

**Beat.** Requiring the header for confined Apps. Leaving the gap.

**Evidence.** `tests/m0/results/m0-wp10.md`, "Proposed design edits" 10.

**Flip.** An L0-07 vendor oracle that cannot get a PR merged under the extra approval in normal use.

#### Agents do not write `refs/notes/ai`

**Decision.** An `ActorKind::Agent` update of `refs/notes/ai` is refused as outside its prefix. Agent commits get provenance from `Session` links and trailers.

**Why.** `refs/notes/ai` is one ref per repository. A fleet of agents each updating it would conflict on every write and serialize through `expect`, the load journey 9 avoids by giving each agent its own refs. Graftyard-run agents already link their commits through `Graftyard-Session`, and vendors through their trailers. Humans and unconfined Apps can still write notes.

**Beat.** Letting an agent write notes on commits under its prefix, which needs per-agent notes refs that git-ai readers do not look at.

**Evidence.** `tests/m0/results/m0-wp10.md`, "Proposed design edits" 20; `research/agents.md` 5.8.

**Flip.** A git-ai release that reads notes from more than one ref, or an agent harness whose only provenance is notes.

#### B0-31's commit line splits on whether placement works

**Decision.** B0-31 passes with commit p99 at most 1 s while buckets are up. While placement fails, it passes with Human and MergeQueue commit p99 at most 3 s, 0 of those pushes refused, and agent pushes over `INLINE_MAX_HOT` refused at once in the `ng` shape. It fails over 3 s, or on any Human or MergeQueue push up to 1 MiB refused while placement fails.

**Why.** An R2-wide outage is an incident, and the row's job then is that humans and the merge queue still commit. Under the outage rule, the measured tail is the round of the 6% of entries that carry a 0.5 to 1 MiB Human or merge pack from apac to the far followers. That put Human and MergeQueue commit p99 at 1.83 and 1.97 s in the two placement-failing runs, and merges alone at up to 2.99 s, with 0 of 546 refused and 4,998 of 5,000 agent 1 MiB pushes refused at once. Admission cannot shorten that round with one entry in flight.

**Beat.** Keeping 1 s throughout, which fails on network distance during an incident. Accepting a fail value at about 2 s, which hides the merge tail that reached 2.99 s.

**Evidence.** `tests/m0/results/m0-wp9.md`, B0-31 third pass, runs `b0-31o-nop4` and `b0-31o-holdnop4`, and "Proposed design edits" 23. In the fourth pass both halves passed the split gate: buckets up, commit p99 547 ms after the apac leader stepped down at 9.9 s (`b0-31-p4up2`); placement failing, Human and MergeQueue p99 1.25 s with 0 of 283 refused and the apac leader moved at 7.4 s (`b0-31o-p4nop2`). In the eighth pass, on the final code, the buckets-up half passed on an apac leader that held 600 s (commit p99 598 ms, `b0-31-p8up`), and the placement-failing half passed at Human and MergeQueue p99 1.11 s with 0 of 287 refused and one step-down from apac at 8.9 s (`b0-31o-p8nop`).

**Flip.** A measured R2 outage longer than an hour, or a customer SLO on push latency during incidents.

#### Idle hot-pool shards keep hot networks warm

**Decision.** Idle hot-pool shards keep the site's hot networks warm, LRU by `NetworkLoad`, holding the parts up to the tip, so a step up in k needs no rehydrate and serves within `fanout`'s up window. `fanout`'s hysteresis stays at up above 0.75 for 3 min and down below 0.4 for 1 h. `scripts/cost.py` prices the pool as running: 7 shards per site on both warm sites, at the tier's instance size, for a 9-hour working day (warm only for networks hot in the last hour), at the 2,000-developer tier only, where the largest network can reach the up line. That is $433 a month on the default and $217 on `Solo`.

**Why.** Cold, a step up waits for the up window and then a full rehydrate. In `b0-34-r8`, with the design's hysteresis and 3 times journey 9's Linux git rate, it took about 290 s from saturation to k = 2 serving, with clone time to first byte p99 5 to 10 s a minute meanwhile, and k = 3 took until near the end of the 20-minute run. A warm pool shard is an LRU hit, so the step costs only the window. At 50 and 300 developers the largest network's git CPU is 0.05 and 0.32 vCPU, so `fanout` never steps and the pool stays reserved.

**Beat.** A cold pool, which bills nothing but gave 290 s at 5 to 10 s TTFB. Up above 0.6 for 1 min and down below 0.3 for 1 h, which B0-34 did not test (open question A22).

**Evidence.** `tests/m0/results/m0-wp9.md`, B0-34 third pass, runs `b0-34-r7` and `b0-34-r8`, and "Proposed design edits" 27. `scripts/cost.py`, `HOT_POOL_WARM_TIERS` and `HOT_POOL_WARM_HOURS`.

**Flip.** A measured rehydrate fast enough that a step serves within 60 s, or a pool cost above the latency it saves.

#### LF-12's commit line is 1 s before `place_learners` runs (2026-10-31)

**Decision.** LF-12 passes at commit p99 at most 600 ms once `place_learners` has run and at most 1 s before it. It fails over 1 s in the placed phases, or over 2 s before placement. The other LF-12 lines are unchanged.

**Why.** The no-learners phase is the state the row exists to move a group away from: 5,000 polls held on the two followers. In `r15` that phase failed at 1,040 ms with 0 step-downs, 0 `suspect` reports, and 0 redirects, because b and c each held 1,000 polls and their feed requests rose to 316 and 275 per s while the leader logged 20 AppendEntries deadlines. Across 8 runs on these rule sets the no-learners phase failed 4 times, 3 of them step-downs (`r8`, `r11`, `r13`) that `commit_fail_min` now addresses. The phases with learners passed or sat between the lines in 9 of 10 runs of the fifth and sixth passes. How long a group spends in the no-learners state is set by how often `place_learners` runs (`decisions/OPEN-QUESTIONS.md`, A20), which now matters more.

**Beat.** Keeping 600 ms throughout, which fails the row on the state it is meant to leave. Dropping the no-learners line, which leaves no bound on a group before placement. Reading the commit line over the median of 3 runs (`tests/m0/results/m0-wp12.md`, "Seventh-pass proposed design edits" 20), under which the seventh pass would pass at phase medians 545, 470, and 696 ms: LF-12 stays a single-run gate, as B0-29 is, and `r18`'s unattributed 1,790 ms stall stays a failure to explain (`decisions/OPEN-QUESTIONS.md`, F40) (operator delegation, 2026-10-31).

**Evidence.** `tests/m0/results/m0-wp12.md`, sixth pass, LF-12 `r14` and `r15`, and "Sixth-pass proposed design edits" 18; the fourth pass's `r7`, `r8`, `r10`, `r11`, and `r13`. On the final rules with `commit_fail_min`, the seventh pass's `r16` to `r18` passed no run in full: `r18`'s placed phase failed at 1,790 ms with the leader held through 23 trips the count blocked, and `r16` and `r17` each had a phase between the lines. With the own-stall exclusion and the all-lagging `Retry-After` of `FEED_BATCH_ALL`, the tenth pass's 4 runs gave 1 pass, 3 between, and 0 fail, every miss from storage confirm stalls and the far follower's round trip, as in B0-02 and B0-29; the gate is unchanged.

**Flip.** An LF-12 placed phase over 600 ms on the final rules.

#### Agent inline bytes per entry are capped

**Decision.** B0-33's Human and merge-queue line stays at 1 s. On every group, hot or not, an entry carries at most `AGENT_INLINE_ENTRY`, 128 KiB [guess], of Agent-class inline pack bytes. An agent pack that would pass it is staged as `PackRef::Stored`, or, while placement fails, gets the agent refusal at once, as over `INLINE_MAX_HOT`. Human and MergeQueue packs do not count against it.

**Why.** In B0-33's fourth pass, run `p4r1` had 0 leader changes and still held Human and merge-queue p99 at 1.15 to 1.49 s, because those commands rode entries of 84 to 96 agent commands and about 450 KB of inline packs. In `p4r4` and `p4r5`, appends of 0.4 to 1.5 MB entries to a far follower missed `CALL_DEADLINE` and tripped the commit-error rule, which moved the leader. B0-29's entries, p99 71 KB, committed at p99 about 500 ms. Bounding the agent bytes in each entry bounds the round every class in it waits for. Staging agent packs costs nothing new, because `scripts/cost.py` already prices a Class A write per push. The budget first applied only on hot groups, and the sixth pass showed the gap: a fresh group is not hot until it has had `HOT_PUSHES` for 5 s, and in `p6r1` and `p6r2` its first entries carried 1.05 to 1.33 MB of agent packs, which in `p6r2` caused the pass's only leader change and only Human and merge-queue p99 over 1 s. Human and MergeQueue packs do not count against the budget, so it needs no hot test, and a quiet group rarely has more than one agent pack an entry anyway (operator delegation, 2026-10-30).

**Beat.** A Human and MergeQueue-only entry ahead of the agent batch, which doubles entries under the one-entry-in-flight invariant without shrinking the far-follower round. Lowering `INLINE_MAX_HOT` for agents only past `AGENT_SHARE`, which B0-33 never reaches at measured capacity. Moving the line to 1.5 s. Keeping the budget to hot groups, which leaves a fresh group's first seconds unbudgeted.

**Evidence.** `tests/m0/results/m0-wp9.md`, "Fourth pass", B0-33 runs `p4r1` to `p4r6` and B0-29 runs `p4r1` and `p4r2`, and "Proposed design edits (fourth pass)" 32. Fifth pass, `p5r3` (Human and merge-queue p99 616 ms with the budget on a settled leader). Sixth pass, `p6r1` to `p6r3`, and "Proposed design edits (sixth pass)" 38. Seventh pass, `p7r1` to `p7r3`, with the budget on every group: no entry carried more than 131,072 bytes of Agent inline packs, first entries included, and all three far-flung runs passed every B0-33 line with 0 leader changes and Human and merge-queue p99 746 to 943 ms.

**Flip.** A B0-33 rerun with the budget where Human and merge-queue p99 stays over 1 s with a stable leader, or staging p99 that pushes agent push p99 over B0-29's lines.

### Plain SSH through an Oracle Cloud Always Free VM until go-live (2026-11-02)

> "oracle has the free for forever tier ... spin up a subagent - use the oracle credentials to deploy a server that is free eligible. save the ssh key in 1password. use that server as the proxy for ssh as we can't have ssh unless on enterprise. i recognize this defeats the point of 'cloudflare' only but will make this an exception until go-live. eventually i will have the enterprise conversation with cloudflare."

**Decision.** Until go-live, `graftyard.dev` takes plain SSH on port 22 through one Oracle Cloud Always Free VM. The VM runs `cloudflared access tcp` on port 22 at the DNS-only `ssh.graftyard.dev`, one Tunnel stream per connection into the hostname the `cloudflared` option uses, so sessions end at the same SSH front, `graft-sshd`, and `ssh_outbound`. It never terminates SSH, so clients check the front's host key, and it holds no git key or credential. Its admin key is in 1Password. It is an exception to "Cloudflare products only" (`FRAME.md`, "Operator decision, 2026-11-02, plain SSH through an Oracle Cloud VM"). The Enterprise Spectrum ingress stays the target and stays designed. HTTPS stays the default, and the `cloudflared` option and B0-24a are unchanged. The VM's Failure-modes rows in `design/DESIGN.md` come from its deployment's measurements.

**Why.** It gives plain SSH on Pro with no `cloudflared` on the client, which a stock `git clone git@host:...` user lacks. Spectrum, the only Cloudflare product with a public port 22, refuses a Tunnel origin on Pro [measured, `tests/m0/results/m0-wp7.md`], and the operator has no Enterprise zone (`FRAME.md`, "Operator decision, 2026-10-31, Pro only and no GHES").

**Beat.** The `cloudflared` ProxyCommand option alone, which leaves stock SSH clients without SSH until Enterprise. Buying Enterprise now, which the operator has deferred. The 2026-10-31 SSH entry's beaten relay VM is this design, now accepted by the operator as a dated exception, not as the product's SSH path.

**Cost accepted.** A new failure domain outside Cloudflare: one VM in one Oracle availability domain, which Oracle may reclaim when idle (CPU p95, network, and, on A1 shapes, memory under 20% over 7 days [doc, https://docs.oracle.com/en-us/iaas/Content/FreeTier/freetier_topic-Always_Free_Resources.htm]), so the plain-SSH path has single-VM availability (R18). A forwarder restart cuts open sessions. It costs $0 under Always Free and is outside every cost total. The upgrade to Pay As You Go, which keeps Always Free at $0 and by Oracle's notice ends reclamation, is blocked: the account has no payment method on file, so the operator must add a card before the trial ends on 2026-12-01. A $1 actual-spend budget alert on the whole tenancy is live. The public IP stays ephemeral, because Oracle's current docs do not list reserved public IPs as Always Free (`tests/m1/results/oci-ssh.md`, "Follow-ups, 2026-11-03") [measured].

**Evidence.** The operator's quote above; `tests/m0/results/m0-wp7.md` for Spectrum's refusal on Pro and for the Tunnel hostname the VM forwards into; `tests/m1/results/oci-ssh.md`: 249 of 249 sessions in 25 minutes, p50 and p99 level with the `cloudflared` path in the same window, and a forwarder restart that cut the open sessions with about 0.32 s and no listener [measured].

**Flip.** An Enterprise zone with Spectrum, which builds the native ingress and retires the VM. Or go-live, when the exception ends whatever else holds.

### Decisions for a Pro-only build with no GHES, 2026-10-31

> "ok skip. get it done given our current pro plan. i won't have enterprise for a while, if at all. also don't have an active github enterprise either. just get it done given these limitations."

The operator said this after hearing that a Business zone would only confirm the Business cells of B0-20 and B0-21 and Spectrum's refusal on Business. "skip" means no Business zone is bought (`FRAME.md`, "Operator decision, 2026-10-31, Pro only and no GHES"). The run made the four decisions below under that answer and the delegation of 2026-10-30.

#### M1 starts with the between rows carried forward

**Decision.** Milestone 0 is closed for the purpose of starting M1. These rows are short of their Pass value and carry forward, with no gate number changed, to the reruns the Milestone 0 status table already names: B0-02 (M1-WP2 on real code), B0-07 (a rerun on M1-WP3's code), B0-11's site-C fault-coverage note (M1-WP2's 24-hour rerun), B0-29 and LF-12 (LM reruns on real code), B0-34 (a full-scale run with the warm pool), and L0-04, L0-06, and L0-11 (the LM rows that rerun them). A Fail value on any of those reruns still sends the design back. B0-24a, the `cloudflared` SSH option's 24 h rerun, gates only M6-WP2's SSH option and does not block M1; it ended between the lines on 2026-11-01 ("SSH goes through Spectrum, Tunnel, and the container outbound handler"). Rows that need a plan, an instance, or vendor accounts the operator does not have (the Business and Enterprise cells of B0-20 and B0-21, B0-24b, L0-07's vendor cells, L0-08's `ghr_` cell) stay not run with their owners named, and none of them gates M1.

**Why.** No Milestone 0 row reached its Fail value. Each row short of Pass is limited by prototype code or by platform storage stalls and distance, which only real code or a larger run can settle, and each already has a named rerun. The build plan's rule that a between result is not done would otherwise hold M1 on rows M1 itself must build the code for. The rows that need a missing plan cannot run in this account at all.

**Beat.** Holding M1 until every row passes, which waits on code M1 builds and on plans the operator may never buy. Moving the between results to Pass, which changes gate numbers without new evidence.

**Evidence.** `build/BUILD-PLAN.md`, "Milestone 0 status (2026-10-31)" and "How to use this plan"; `tests/m0/results/`, one file per package.

**Flip.** A Fail value on any carried-forward rerun, which sends the design back as a Milestone 0 fail would have. B0-24a reaching its Fail value on M6-WP2's rerun, which leaves Pro and Business HTTPS only and drops M6-WP2's SSH option, but does not reopen M1.

#### Rows that need a plan the operator lacks are owned by the first install on that plan

**Decision.** The Business and Enterprise cells of B0-20 and B0-21, and B0-24b, are recorded as not run, as M0-WP6's Plans paragraph says, and the design's claims for those plans stay [unverified] or [doc] until the first install on that plan runs the row in its own account. The edge's push cap stays parameterized by plan, with Business at 200 MB and Enterprise at 5 GB carried as [doc]. Journey 1's 2 GB push on Enterprise and B0-20's Enterprise cell stay unmeasured. M6-WP2 builds only the `cloudflared` SSH option. The Enterprise Spectrum ingress (a Spectrum app on port 22 to the Tunnel's virtual network) stays in the design and is not built until an Enterprise install exists, and B0-24b then gates it. The operator's "ssh on enterprise" decision stands: the ingress is deferred, not dropped.

**Why.** The operator has only the Pro zone `graftyard.dev` and may never have Enterprise. Building the Spectrum ingress now would ship code no row can test. The SSH front containers, `graft-sshd`, and `ssh_outbound` are the same for both ingresses, so the deferred part is the Spectrum app, its virtual-network route, and their Terraform.

**Beat.** Building both ingresses now and leaving the Spectrum one untested. Dropping native SSH on Enterprise, which would reverse an operator decision with no evidence.

**Evidence.** `build/BUILD-PLAN.md`, M0-WP6 (Plans) and M0-WP7; `tests/m0/results/m0-wp6.md` and `m0-wp7.md`; `FRAME.md`, "Operator decision, 2026-10-31, SSH ingress".

**Flip.** An Enterprise install, which builds the Spectrum ingress and runs B0-24b and B0-20's Enterprise cell in its own account. A Business install, which runs B0-20 and B0-21's Business cells and tests whether Spectrum takes a virtual-network origin there.

#### L0-11's oracle is the pinned GHES 3.22 descriptions and github.com

**Decision.** With no GHES 3.22 instance, L0-11's oracle is the pinned `ghes-3.22` OpenAPI description and the GHES 3.22 GraphQL schema for response shape (`research/clients.md` finding 49; `research/wire-census.md`, "Implications for the design"), plus github.com's API as a behavioral oracle only for operations whose `ghes-3.22` and api.github.com descriptions match, with every difference listed. This oracle is weaker than a GHES appliance [inference]: it checks shape and github.com behavior, not GHES's own behavior where it differs from github.com. The gate text is unchanged. The same reading applies to the other gates that named a GHES instance (L0-08's `ghr_` cell and BF-65's GHES archive, in the two decisions below). Gates whose oracles are unmodified clients (git, gh, Octokit contract runs from the pinned `ghes-3.22` OpenAPI, the Terraform provider, Renovate, and the rest of `design/SHARED.md`, "Surfaces, tiers, and oracles") need no GHES and are unchanged.

**Why.** The operator has no GitHub Enterprise and does have a github.com account, so a github.com oracle is runnable now. The research already suggested the pinned description as the shape oracle.

**Beat.** Leaving L0-11 waiting on an appliance the operator may never have. Using github.com for every operation, which would hide the operations whose GHES and dotcom descriptions differ.

**Evidence.** `tests/m0/results/m0-wp15.md`, L0-11 (partial: 0 of 1,011 differences against the prototype, no GHES oracle). `research/wire-census.md` finding 6 and "Implications for the design"; `research/clients.md` finding 49.

**Flip.** A GHES 3.22 instance becomes available; then rerun L0-11 against it.

#### `graft-mcp` keeps the unscoped token sealed until the next refresh, and `import ghes` is tested on a synthetic archive

**Decision.** AM2-WP1 builds A4's fallback as the behavior. At `/token`, after `token/scoped` narrows the `ghu_`, `graft-mcp` keeps the unscoped `ghu_` sealed in the `McpRefresh` beside its `ghr_`, and deletes it with `DELETE /applications/{client_id}/token` only at the next refresh, after the `ghr_` has been spent. Every call still uses the narrowed token. Revoking the client deletes the narrowed token and the held pair (journey A7 step 7). A4 stays open, and its answer no longer gates anything; L0-08's `ghr_` cell, which no Milestone 0 run could measure, becomes "a refresh succeeds with the unscoped `ghu_` held, and the held `ghu_` is deleted after it"; no number changes. Separately, `graftyard import ghes` (M7-WP6, BF-65) is tested on a synthetic `ghe-migrator` archive built from GitHub's documented format, and that test is recorded as untested against a real GHES export [inference]. Neither the design nor the research says github.com's migrations API produces the same archive, so it is not named as an oracle.

**Why.** The fallback is correct whether or not the `ghr_` survives the delete, and on Graftyard's own host it is required. `graft-mcp` redeems codes only against the install's own host, where a `ghu_` and its `ghr_` are `paired` and "revoking either revokes both" (`design/DESIGN.md`, Shape, Identity; BF-31: "the DELETE revokes"). So the immediate delete written in journey A7 would revoke the `ghr_` there and break every refresh, whatever GHES does; M0-WP15's prototype host modeled survival, which the design's own pairing rule does not give. Holding the unscoped token adds no power beyond the sealed `ghr_` that `McpRefresh` already holds, because a `ghr_` can mint an unscoped `ghu_` [inference]. If a GHES host does let the `ghr_` survive, the only loss is a later delete. No GHES instance exists to produce a real `ghe-migrator` export.

**Beat.** Deleting the unscoped `ghu_` at once, which breaks every refresh on Graftyard's host. Exempting `DELETE /applications/{client_id}/token` from the pairing rule, which would make Graftyard's revocation differ from its own BF-31. Waiting for a GHES instance for either test.

**Evidence.** `decisions/OPEN-QUESTIONS.md`, A4; `design/AGENT-LAYER.md`, journey A7 step 5, Shape (`McpRefresh`), and "Review resolutions" (AG-ACT-8); `design/DESIGN.md`, Shape, Identity (`paired`), and BF-31; `tests/m0/results/m0-wp15.md`, L0-08; `design/SHARED.md`, `rest:migrations`.

**Flip.** A GHES 3.22 instance on which the `ghr_` survives the delete, and a reason to match it, which would allow the earlier delete. A real GHES export whose shape differs from the synthetic archive, which reopens BF-65.

### Shape corrections from the Raft core build (2026-10-31)

**Decision.** `design/DESIGN.md`, Shape, now matches what `crates/raft` had to build. `decide` takes an `EntryView`, the fold at `applied` plus the events of the entry's earlier commands, and the `deps` folds read once through `fresh`, as the write-path prose and "Invariants" already said. `outcome_by_request` takes the command the edge would re-send, because `replay` is a method on it. `MemberState` gains `promised_to` and `saved_at`, which the promise rule and the `boots` rule read. A duplicate answered from its `request` row carries the current leader's latest commit proof, which covers the stored index. A new leader whose term's `Noop` reaches no quorum within 3 s [guess] returns to follower and keeps `led_term` [inference]. The member's own `COMMAND_MAX` refusal is `CallError::TooLarge`, rendered as the edge's refusal before `commit`.

**Why.** The Shape contradicted its own prose or lacked what its own rules need: a `Fresh` is the leader's committed fold, which does not include the entry's earlier commands that BF-13's case depends on; `outcome_by_request` had no command to call `replay` on; the promise and boot rules named state the row did not hold; the original acks of a duplicate are gone when it is answered; and a leader whose `Noop` cannot commit had no stated exit.

**Beat.** Keeping `decide(&Fresh)` and folding earlier commands into a copy of the committed fold, which is the same view under another name. Storing each request's acks with its row so a replay could reuse them, which adds bytes per row for a proof no caller reads beyond its existence.

**Evidence.** `tests/m1/results/m1-wp2.md`, "Where the code departs from the design's Shape", and its simulator runs: 2,000 seeds by 10,000 steps passed every safety check and Porcupine, and BF-13's case returned no refusal from an uncommitted entry [measured, simulator, not yet on the hosted code].

**Flip.** The hosted code (M1-WP2's BF-12 and BF-13 on Durable Objects) needing a different shape for any of these.

### Decisions from the M1 builds, 2026-11-01

M1-WP2 hosted the Raft core on Durable Objects, M1-WP3 built leadership and health in the crate's simulator, M1-WP4 and M1-WP5 built the pack store and the git cache shards on real buckets and containers, M1-WP6 built the git edge, M1-WP7 hosted minimal identity and placement on Durable Objects, and the operator CLI built genesis acceptance (`tests/m1/results/m1-wp2.md` to `m1-wp7.md`, and `m1-cli.md`). The lead made the decisions below under the operator's delegation of 2026-10-30. Two findings are open questions, not decisions: `confirm_slow_min` on quiet groups (F41) and every `STEP_DOWN` holding under frequent resets (F42), each stated in Shape beside its rule.

#### The leader sends AppendEntries before its own append is durable

**Decision.** The leader sends each entry to its followers before its own append is durable, and counts itself toward the quorum only once its own storage has confirmed (Raft dissertation, section 10.2.1). Every follower still confirms before it replies, and a commit is revealed only after the leader's own write is durable. A restarted leader that lost an entry it sent may send another at the same (term, index); followers match entries by digest, and the lost one never committed. The crate's persist-then-send rule changes for the leader's own `Append`, and B0-01 is re-measured on the change. Until then the Rust host's figure stands as measured.

**Why.** The design said "sends to both followers at once" and "confirmed by its own storage before its reply left" without saying whether the leader may send first. Milestone 0's prototype sent first and passed B0-01 at p99 433 to 481 ms. The Rust host let the output gate hold each round for the leader's own write, and six one-minute runs measured p99 645 to 791 ms, between pass and fail in every run, with the leader's own confirm (p50 23 to 25 ms, tail to 312 ms) added to every round under one entry in flight.

**Beat.** Keeping write-then-send and accepting that B0-01's margin is gone. Lowering the load or the gate, which changes a gate number with no new evidence.

**Evidence.** `tests/m1/results/m1-wp2.md`, "On Durable Objects", "Commit latency, B0-01 style" and "What the crate should expose" [measured]; `tests/m0/results/m0-wp2.md`, B0-01; `crates/raft/src/member/follower.rs`, which already matches entries by digest for this case.

**Flip.** The re-measure on the changed crate staying over 600 ms, which shows the leader's write was not the cause; or a lost-entry case the digest match does not cover.

**Amendment, 2026-11-13 (`m0-carried-6`).** Sending first keeps a round clear of the sending input's own write, not of an earlier one. The output gate holds a send "until all previous writes have been confirmed flushed to disk" (https://developers.cloudflare.com/durable-objects/api/storage-api/, `allowUnconfirmed`), and SQL has no `allowUnconfirmed`. So an AppendEntries issued while an earlier entry's transaction is unconfirmed waits for it. On `graft-m3d` such sends were late by about what was left of that confirm, p50 256 to 342 ms with over 150 ms left. All of them went to the far follower, whose answer arrives after the near one has appended the next entry [measured, `tests/m0/results/carried-6.md`]. The decision stands. How the host keeps sends clear of earlier writes is open (`decisions/OPEN-QUESTIONS.md`).

#### Member-call retries are the Raft core's resends

**Decision.** The host sends each member-to-member message once and never retries it. A commit-path call that fails is resent by the core at once at most `COMMIT_RETRIES` (2) times in a row and then on `tick` after each `CALL_DEADLINE`; votes are resent on `tick`; a campaign, or a new leader whose term's `Noop` reaches no quorum, gives up after CALL_DEADLINE x (1 + COMMIT_RETRIES), 3 s. `COMMIT_RETRIES` keeps its meaning: how many resends of a commit-path call before it counts as failed. An edge's re-send under the same request id is separate.

**Why.** "Every receipt and member-to-member call has a 1 s deadline and retries at most twice" read either as a transport retry or as the core's budget. A transport retry under the core's own resends would double them, and the M1 host and crate already work the second way.

**Beat.** A transport retry in the host, which would resend messages the core resends anyway and hide failures from `Health`.

**Evidence.** `tests/m1/results/m1-wp2.md`, "The host" (Transport) and "Design problems found"; `tests/m1/results/m1-wp3.md`, bug 4 (a failed call waited for `tick` until it was resent at once); `crates/raft/src/member/leader.rs` (`append_failed`) and `election.rs` (`CAMPAIGN_DEADLINE`).

**Flip.** A measured case where the core's resends miss a failure a transport retry would have recovered within one `CALL_DEADLINE`.

#### `suspect` names who a report is about, and a transfer vote promises the target

**Decision.** `suspect(follower, from: Colo, about: SiteId, term)`: a report names the member whose receipt missed, or that answered `NotLeader` naming no leader, and counts only when `about` is the leader this follower promised and has heard from in the current term; a higher term clears that leader. A granted transfer vote is also a promise to the target. A target that is drained, under `no_lead_until`, or inside its own RESET_QUARANTINE answers `TransferRefused`, and the leader abandons the transfer at once rather than at LEASE. Shape now defines `Verdict`, `TransferError`, `SuspectError`, and `Colo` as the crate has them.

**Why.** Without `about`, late reports about a transfer's old leader withheld promises from the new one: three leader changes in one run. Without the promise to the target, a report to the old leader before the target's first append reached the other follower let the old leader win that vote back and depose the target, and the group had no leader. Without `TransferRefused`, a refused transfer would hold the group, which appends nothing and serves no `fresh` read during a transfer, until LEASE ran out [inference from Shape, `transfer`].

**Beat.** Counting every report against whoever the follower follows. Breaking promises only for the transfer id, as the design had it, with no promise to the target.

**Evidence.** `tests/m1/results/m1-wp3.md`, "Bugs the tests found" 1 and 2 and "Where the code departs from the design", with the targeted cases `a_transfer_vote_promises_the_target` and `a_refused_transfer_is_abandoned_at_once_and_leaves_no_lease` [measured, simulator]; Milestone 0's prototype also passed a target (`tests/m0/src/raft/member.ts`, `suspect`).

**Flip.** The hosted code (BM-1, B0-07's rerun) showing a leaderless group or a lost promise these rules cause.

#### "Recovered" for the transfer back to `home`

**Decision.** A leader transfers to its group's `home` once `home` answered at least 5 calls in the last `STEP_DOWN` window with none failed, is outside RESET_QUARANTINE, and holds every committed entry, the leader is not tripped, and a window has passed since it started leading. After a failed transfer to `home` the next try waits 60 s. Both numbers are [guess].

**Why.** The design said "recovered and caught up" and defined neither. Asking `home` to hold the last index first held a transfer for about 58 s with no fault, because under load an entry is always in flight; the transfer itself brings `home` to the last index.

**Beat.** Waiting for `home` to hold the last index. Retrying a failed transfer at once, which can bounce a group between sites.

**Evidence.** `tests/m1/results/m1-wp3.md`, bug 6 and the `home` cases (`leadership_returns_home_after_it_recovers_and_catches_up`, `a_quarantined_home_gets_no_transfer`) [measured, simulator]; `crates/raft/src/member/health.rs` (`HOME_CALLS`, `HOME_RETRY`).

**Flip.** A hosted run where groups return to a `home` that is still failing, or stay away from a healthy one for more than a few windows.

#### The pack store and shards follow what the M1 code needed

**Decision.** A `request` row saves an unmarked pack only when its events name that pack's id, so a retried request's orphan from a failed first placement is deleted at 24 h. The edge journals each multipart upload under `R2Key::Upload` before its first part, and the sweeper aborts from the journal, because the R2 binding cannot list in-progress uploads. The `Committed` marker carries (chain, index, checksum, bytes). `IngestCursor` is keyed to a disk id the container writes at first start, and a cursor for another disk, or one behind an `Archived` truncation, means a full rehydrate. "After its last `Promoted`" means after the promoted range's `to`. Pushed packs stay stored as pushed, thin, and every ingest re-indexes with `index-pack --strict --fix-thin`; whether to store them fixed is G5.

**Why.** Each is what the code had to do for a rule the design states. A row that saves any pack under its request id kept the first placement's orphan; the binding has no list call for open uploads; the filler cannot verify a copy from (chain, index) without reading the chain; a restarted container has an empty disk under an unchanged cursor; inline packs between `to` and the `Promoted` entry are not covered by it; and git cannot keep a thin pack, so a stored `.idx` of one is unusable. Storing packs thin keeps the edge's tee, which writes the pushed bytes to the buckets while the shard validates. No result measured the cost of the re-index on real history.

**Beat.** Saving any pack whose request has a row; relying on R2's 7-day multipart expiry; a cursor with no disk id; storing fixed packs now, before G5 measures what the re-index costs.

**Evidence.** `tests/m1/results/m1-wp4.md`, BF-19 (pass on real buckets with the `request` table stubbed: 2,150 of 2,150 orphans deleted at 24 h and 0 committed packs deleted, 10 of 10 multipart orphans aborted) [measured]; `tests/m1/results/m1-wp5.md`, BF-18 (pass on Cloudflare Containers with the chain leader stubbed, 3 runs, 0 failures) [measured]; `crates/git/src/sweep.rs`, `ingest.rs`, and `host.rs`; `containers/git/server/main.go`.

**Flip.** BF-19 or BF-18 failing once the site script hosts the sweeper, `GitCacheShard`, and the real `request` table. For thin storage, G5's B0-22 rerun putting the re-index past 120 s on a real network.

#### The git edge's shape follows what M1-WP6 built

**Decision.** `CloneKey` gains `tags`, the digest of `refs/tags/` at the read, set only when the fetch asks for `include-tag`. SHARED gains `RejectReason::Atomic` for a ref refused only because another ref of an `AllOrNothing` push failed, which git shows as `atomic push failure`. The 10,000-ref cap applies to `--atomic` pushes only; a non-atomic push is bounded only by `COMMAND_MAX`. The member renders `report_status` from `&Acked` and the edge frames the bytes. A command refused inside `decide` still commits an entry with its `request` row, so the refusals that must leave no entry (the atomic cap, `COMMAND_MAX`) are made at the edge before `commit`. SHARED's `GCM_TRACE=1 git clone` oracle stays, recorded as not yet run.

**Why.** With `include-tag` the clone-cache response depends on the tag refs, so a key without them could serve a stale tag set. No reason fit a ref that would have passed in a failed atomic push, and GitHub prints `atomic push failure` for it. SHARED said both "Admission caps a push at 10,000 refs" and "Non-atomic pushes are not capped"; the code and BF-17 follow the table. `Acked` cannot cross a Durable Object binding, and only the member holds it, so the invariant holds with the member rendering. BF-13 needs a refused command's entry to commit, so "no entry" can only come from the edge.

**Beat.** A cache key without tags, which would need every tag move to invalidate the clone cache. Reusing `Stale` for the atomic case, which tells the user the wrong thing. Capping non-atomic pushes too, which GitHub does not do by default. Shipping `Acked` across the binding, which is impossible.

**Evidence.** `tests/m1/results/m1-wp6.md`, "Git clients" (29 of 29 rows on git 2.39.5, 2.51.0, and 2.56.0), BF-17, and "Design problems found" [measured]; `crates/git/src/clone.rs`, `repo.rs`, `smart.rs`, and `crates/git/tests/push.rs`.

**Flip.** A GHES 3.22 or github.com trace showing a different atomic-failure line, or a cap on non-atomic pushes; a clone-cache hit that serves a tag set its key did not digest.

#### BM-1's `git` conformance suite is SHARED's git oracles plus git's own remote-capable transport tests

**Decision.** The "`git` conformance suite" in BM-1 is SHARED's `git-http` and `git-protocol-v2` oracle rows on unmodified git 2.39 and 2.51, plus those of git's own smart-HTTP transport tests in `t/` that can be pointed at a remote host, run by a harness under `tests/m1/wp6`. The gate number is unchanged.

**Why.** No document defined the suite. Git's `t/t55xx` tests start their own Apache with `git-http-backend` as the server under test, so only the ones that can target a remote host test Graftyard without patching git's harness, which would no longer be unmodified git.

**Beat.** All of `t/t55xx` with a patched harness, which tests a modified client. SHARED's rows alone, which leave git's own transport edge cases untested.

**Evidence.** `tests/m1/results/m1-wp6.md`, "The `git conformance suite` (BM-1)".

**Flip.** A remote-capable subset too small to cover a transport behavior an unmodified client depends on, found by a client failure the suite missed.

#### Bootstrap claims a chain, and the committed config owns `home`

**Decision.** A bootstrap member, after reading its record generation from the source, claims its chain in the `MemberRegistry` of the oldest record's first site; the first member generation to claim wins, and a member of another generation that finds a claim stores nothing. A replacement on the same site, a placement retry or `ReplaceMember`, takes two config changes, `Degraded { lost }` then `Normal` with the new member. Once a group exists, its committed config's `home` is authoritative, and the record's `home` only seeds bootstrap. A `MovePlacement` that changes only `home` needs no `Joint`: it is one config change and a transfer.

**Why.** The design's rule, that a member reads the same record generation from the source, held for each of two edges holding two generations during a move, and the chain got two groups in 20 of 20 control rounds; with the claim, 100 of 100 rounds built one. `GroupConfig` counts votes and acks by site, so one config cannot name two members on one site. The transfer back to `home` read the record's `home` and pulled re-homed groups back to their old site in BF-11's first run.

**Beat.** A `Bootstrapped { chain, generation }` row committed once by the `Names` leader, which adds a commit on every chain creation. Creating a chain only from the owner's oldest record still in `moving_from`, which every edge would need to read fresh.

**Evidence.** `tests/m1/results/m1-wp7.md`, "On Durable Objects (2026-11-01)": BF-10 (1,000 of 1,000 chains with one group; 100 of 100 during a move with the claim, 0 of 20 without it) and BF-11 (1,500 of 1,500 requests found the group on the fixed code), and "Design problems found" [measured]; `workers/site/src/member.rs`; `crates/raft/src/member/health.rs` (`home` from the config).

**Flip.** A BF-10 rerun on the hosted `Names` group with two groups for any chain; a registry outage on the oldest record's first site that blocks chain creation for longer than the site's other members are down.

#### Genesis answers a second genesis with 409, and `KeyId` is the public key

**Decision.** The release endpoint answers a replay or a second genesis with 409 and `prev` set to the digest of the genesis it holds, so a retried `graftyard install` after a lost 201 sees its own digest and stops. A `KeyId` is the 32-byte Ed25519 public key. In Milestone 1, `graftyard genesis` reports the scripts it did not create, because none of them exist yet.

**Why.** A genesis has no `prev`, and the design defined 409 only for a stale one. Without this, a retry after a lost 201 cannot tell success from another genesis. `Keys` named keys by `KeyId` without defining it, while `LifeboatAdd` and `SiteLink::Https` use `Ed25519Public`.

**Beat.** A 422 `AlreadyInstalled`, which a retry could not tell apart from a different genesis without another read. A digest-style `KeyId`, which would need the key bytes stored elsewhere in `Keys`.

**Evidence.** `tests/m1/results/m1-cli.md`, "What the edge's release endpoint must accept" and "Design problems found" 2 to 4; `crates/release`.

**Flip.** A signer that needs to rotate a key without changing its id, which a digest `KeyId` would allow.

### BM-1 and the M1-WP8 fixes, 2026-11-02

M1-WP8 ran BM-1's git half on the test install with one site's member class failing every call, and fixed what the first runs found (`tests/m1/results/m1-wp8.md`). The lead made the decisions below under the operator's delegation of 2026-10-30.

#### A replayed push reports each refused ref's own reason

**Decision.** A push's `decide` writes `RepoEvent::Refused` (core tier, in the push's own entry) with each refused ref and its `RejectReason`, beside `Refs` or alone. `Push::replay` answers a duplicate from it, so each refused ref keeps its original reason. Only a `request` row written before this event existed replays an absent ref as `Stale`. SHARED's `RejectReason` also lists `NameConflict` and `AtomicTransaction`, which the code already returned.

**Why.** A `request` row holds the entry's events, and a refused ref left none, so the old rule replayed `AtomicTransaction`, `NameConflict`, `Atomic`, and `Hidden` as `Stale`, and git printed `fetch first`. That was git's t5541 #20 on a lost reply.

**Beat.** Storing the whole `Out` with the row, which duplicates the applied refs and needs a second format. Keeping `Stale`, which misreports every refused ref of a re-sent push.

**Evidence.** `tests/m1/results/m1-wp8.md`, fix 1: `a_replay_reports_each_refused_refs_own_reason` fails on the old rule, and t5541 #20 passes on the redeployed edge [measured].

**Flip.** A client that depends on a replayed refusal reading `Stale`, which none of SHARED's oracles do.

#### Calls and catch-up reads are capped by bytes

**Decision.** Each AppendEntries carries at most `APPEND_BYTES_MAX` (1.5 MB) past its first entry, as well as at most 64 entries. A member answers a shard's catch-up read of entries in pages of at most 600 kB of stored bytes, at least one entry each. Both numbers are [guess].

**Why.** A shard's catch-up read answered 239 entries, 4.6 MB with up to 1 MiB inline packs each, in 7.8 s, past the lookup's call deadline, so every attempt timed out and the shard never caught up. 64 full entries in one AppendEntries could reach 96 MB, which no member answers within `CALL_DEADLINE`, and the leader would resend it forever.

**Beat.** A longer deadline for these calls, which delays every failure the deadline exists to catch.

**Evidence.** `tests/m1/results/m1-wp8.md`, fix 4: after paging, the catch-up rehydrated 161 inline packs (3.48 MB) in 16.8 s where it had failed after 27 to 30 s; `a_lagging_follower_catches_up_in_calls_under_the_byte_cap` fails on the old code with a 4.0 MB call [measured].

**Flip.** A measured call under the caps that still misses `CALL_DEADLINE`, or catch-up time dominated by page count.

#### The site-loss bound is per request, and a total loss is left by `suspect`

**Decision.** The Summary, journey 5, and the region rows state the outage bound per request: a request to a group led on a lost site waits at most about 11.5 s plus its own commit, counted from that group's first request after the loss. Campaigns stay request-driven. A total loss of a site's member class is left by `suspect` and election; a gray region by `STEP_DOWN`, `suspect`, or a signed drain.

**Why.** Under BM-1's fault the first push took 16.8 s and later ones 5.4 to 6.3 s, while `tokens:1153`, used only near the end of each run, moved 247 to 284 s after the fault, when its first request came. A leader whose every call fails never completes a commit attempt, so its `Health` window never trips: every move in BM-1 was an election, and the one `STEP_DOWN` alarm fired on a new leader off the faulted site. The Summary's "groups led there stop reads and writes for at most about 11.5 s" read as a bound from the loss.

**Beat.** Alarm-driven campaigns, which would move idle groups at once but wake every idle group's members, against B0-15's idle-cost row and the rule that no idle group is woken.

**Evidence.** `tests/m1/results/m1-wp8.md`, "Leader moves", "Commit latency during the fault", and "Design problems found" 3 and 4 [measured].

**Flip.** A user-visible request that waits longer than about 11.5 s plus its commit after a site loss, or an idle-group move that a client needs before its first request.

#### F67: a group with a request waiting elects within 30 s, and campaigns stay request-driven

**Decision.** Liveness is a property of the core: while the same two or more voters stay up, linked, and free of gray faults, and a client request waits on the group, some request completes within T = 30 s (LEASE 9 s, the 2 x ELECTION_AFTER waivers of RESET_QUARANTINE and the drain bar, and about 1 s of campaign and round). An idle group is still never woken. Seven rules in `crates/raft` make it hold. (1) Check quorum: a ready leader with an entry, read, or transfer waiting that no quorum has answered for ELECTION_AFTER stops leading and clears `led_term`; an idle leader is never moved. (2) A follower's read-index batch unanswered for CAMPAIGN_DEADLINE (3 s) is the read-index call failing: the follower forgets the leader, campaigns if its promise lapsed, and fails the reads `NotLeader` naming none, which the edge reports with `suspect`. (3) A granted transfer vote promises the target for CAMPAIGN_DEADLINE, not ELECTION_AFTER. (4) A pre-vote is denied as `Contending` only by a voter that may lead itself and has not been refused as `Suspected`. (5) A member refused as `Suspected` keeps campaigning by pre-vote instead of answering `Busy` for 2 x SUSPECT's window. (6) A `suspect` report about the leader a follower promised counts even after the follower forgot it under (2). (7) Appends from a leader a follower withholds promises from do not count as a leader seen, so the 2 x ELECTION_AFTER waiver runs. (8) Rule 6 counts a report only about the leader the follower heard from in this term, or the one it forgot under (2); a transfer target it voted for but never heard from is not one, and a refused `Suspected` member retries its pre-vote after SUSPECT's window. (9) A leader that truncates its own-defect tail starts a new `lead_seq`, so a follower refuses rounds sent before the truncation; `id_block` also checks a lease against the blocks in the leader's uncommitted log.

**Why.** Seven paths left a group with traffic leaderless past the bound in the simulator (`tests/m0/results/f67.md`). A half-hung leader, whose appends arrived and whose answers were lost, kept its followers' promises renewed, so every `suspect` answered `Promised` and a reads-only group never elected. Follower reads shed with no report, so a group read only through followers had no trigger at all. A transfer target that could not win held both voters for ELECTION_AFTER. A quarantined campaigner refused every rival while every voter refused it. A member refused as `Suspected` stood aside for 20 s, outlasting the voter's bar on it, while the other member was `LogBehind`. A follower that forgot a half-hung leader under (2) stopped counting reports about it while still promising it. A half-hung leader's appends kept two healthy voters that quarantined each other from ever reaching the waiver. Merging into `lm1-memory` found two more on main: reports about an unheard transfer target barred the only member holding the newest entry for 17 s (liveness seed 107), and a delayed pre-truncation round deleted a committed entry (harsh seed 3013), a safety break.

**Beat.** Alarm-driven campaigns, which wake idle groups ("The site-loss bound is per request, and a total loss is left by `suspect`"). Promising only after a commit of the leader's term, which broke the promise-bar and lease scenarios. Letting a bar yield to a newer log, which the rules made unnecessary. Check quorum by progress (commits and grants) in place of answers heard, which changed no stuck seed.

**Evidence.** `tests/m0/results/f67.md` [measured, simulator]: one regression test per path that fails on the core before the fix, the 30 s bound asserted per seed in the CI sweep (`live_network_elects_within_the_bound`, which the old core fails on seed 7180 at 32.6 s), and the long sweep with Porcupine.

**Flip.** A group with a request waiting and a healthy quorum that stays leaderless past 30 s, in the sweep or hosted; or a check-quorum step-down of a leader a quorum was still answering.

#### BM-1's git half passes with one site's member class failing

**Decision.** BM-1's git half is recorded as passing. `clients.sh` passed 29 of 29 on git 2.39.5 and 2.51.0, and `t/` on 2.51.0 and 2.56.0 lost no baseline pass except t5549 #2, with site 0's member class failing every call for the rest of each run. The 24-hour linearizability half is pending until 2026-11-02T18:27Z. A gray partial-error fault and a 2.39.5 `t/` baseline are not covered and go to BM-1's next rerun.

**Why.** The one lost pass, t5549 #2, checks how many objects a negotiated push wrote, which depends on what the server advertises; the test sets a server-side `receive.hiderefs` the host does not apply, the push itself succeeded, and the same test already differed between two no-fault runs [inference]. 2.39.5's two extra failures are fixtures the hosted slot cannot reproduce (a lock file and a server-side hook).

**Beat.** Holding the git half until a gray fault runs, which BM-1's gate text does not name.

**Evidence.** `tests/m1/results/m1-wp8.md`, "BM-1 results" and "What is short of the gate" [measured].

**Flip.** BM-1's next rerun losing a baseline pass under a gray fault, or t5549 #2 failing for a cause the host controls.

**Amended, 2026-11-13 (`m0-carried-8`): `APPEND_BYTES_MAX` is 100,000 bytes.** On B0-02's far link (leader enam, far voter apac), far calls answered at p99 768 to 845 ms for 75 to 100 KB and 810 to 869 ms for 100 to 125 KB, and missed `CALL_DEADLINE` from 150 KB; calls of up to 125 KB with 500 to 550 KB of earlier pipelined calls still out answered at p99 644 ms [measured, `m0-carried-6` to `-8` traces, `tests/m0/results/carried-8.md`]. At 1.5 MB, r330-6's far voter fell 7 to 14 entries behind, its catch-up calls of 319 to 717 KB missed one after another, and with a slow near follower `CommitErrors` moved leadership to the far site (1,635 ms). A call still carries one whole entry however large, so an entry of up to 100 KB plus 128 KiB of agent packs, or one command up to `ENTRY_MAX`, still replicates (`a_lagging_follower_catches_up_in_calls_under_the_byte_cap`, entries of 400 KB). Pipelining to a voter now stops only on a lossy link (F67 rule 13's `link_lossy`, over every call in the window), not on one missed call, which had turned the pipeline off for the window and left the backlog to one call at a time (`a_voter_is_pipelined_unless_its_link_is_lossy`, `a_far_voter_behind_catches_up_in_calls_that_answer_in_time`). Flip: a far link whose calls of up to 100 KB miss `CALL_DEADLINE` at p99, or a follower that catches up slower in calls this small than the backlog grows.

### The hosted closing runs, 2026-11-02

BF-14, BF-18, and BF-19 ran on the hosted test install, and BM-1's git half reran under a gray fault (`tests/m1/results/m1-wp8.md`, "Gate rows closed on the hosted install"; `tests/m1/results/m1-wp5.md` and `tests/m1/results/m1-wp4.md`, the 2026-11-02 sections). The lead made the decisions below under the operator's delegation of 2026-10-30.

#### A promotion packs every pack its range names

**Decision.** Before a range is archived, the home shard packs the objects of every pack, inline and stored, that the range names into one `DurablePack`, so the promoted pack is self-contained, and `Archived` follows only once `Promoted` covers the range. Pushed packs stay stored thin; the rest of G5 stays open.

**Why.** Promoting only the inline packs, as the design said, failed BF-18's first hosted run: the range's stored pack was thin against earlier inline packs, a later inline pack named its objects, and rehydrate, which ingests each pack on its own with `--fix-thin`, stopped at `index-pack: fatal: did not receive expected object`. With the stored pack's objects also promoted, run 2 passed.

**Beat.** One `index-pack` over the promoted pack and the range's stored packs together, or ingest order plus `--fix-thin` against the repository, which keeps the promoted pack small but makes every rehydrate depend on ingest order across packs. The chosen rule duplicates the stored packs' objects until compaction consumes them.

**Evidence.** `tests/m1/results/m1-wp5.md`, "BF-18 on the hosted install with a real `ChainLog`", runs 1 and 2 [measured].

**Flip.** Promoted packs whose duplicated stored objects cost more storage than a rehydrate that ingests the range's packs together, measured on real history.

#### BF-14, BF-18, and BF-19 are done, with their stubs recorded

**Decision.** BF-14, BF-18, and BF-19 are recorded as passing and done for M1, each with its remaining stubs in BUILD-PLAN's "Milestone 1 status": BF-14's missed receipts were injected, not real network loss; BF-18's archive is triggered by a test route, with four smaller gaps; BF-19's 24 h ran on the sweeper's injected clock. BM-1's git half also passes the gray fault it lacked. The 24-hour linearizability half stays pending until 2026-11-02T18:27Z.

**Why.** Each row's pass line held on the real code and real groups: 0 of 2,400 commands forwarded; `git fsck` clean and 9 of 9 clones after a real `Promoted` and `Archived`; 0 of 1,000 committed packs deleted and 1,000 of 1,000 markers repaired from the real `request` rows. None of the stubs is something the gate rows name.

**Beat.** Holding them for the M2 soak, which would rerun the same paths without the injections the rows need.

**Evidence.** `tests/m1/results/m1-wp8.md`, "Gate rows closed on the hosted install, 2026-11-02 (afternoon)", BF-14 and "BM-1 gray-fault rerun"; `tests/m1/results/m1-wp5.md`, "BF-18 on the hosted install"; `tests/m1/results/m1-wp4.md`, "BF-19 on the hosted cron" [measured].

**Flip.** A real reply loss that forwards a command, a rehydrate that misses an object after a leader-policy archive, or a committed pack deleted on a real clock.

#### A gray region at low load is left by `suspect`, not `STEP_DOWN`

**Decision.** The region row and the Summary say a gray region is left by `suspect` or a signed drain, and by `STEP_DOWN` only on a group busy enough to fill its `Health` window. `commit_fail_min` is unchanged; whether quiet groups need a lower one joins F41 beside `confirm_slow_min`.

**Why.** At 30% call errors on site 0, both BM-1 gray runs moved every affected group by election after the edge's `suspect`, and no `STEP_DOWN` fired on any member. Three failed calls to each of two followers within 10 s takes about 10 appends at that rate, and `repo:196608` committed 7 entries in 5 minutes [inference from the rule and commit counts; the window was not logged].

**Beat.** Lowering `commit_fail_min` now, which brings back the follow-on changes after kills that its 3 removed (B0-06, M0-WP4), with no measurement of a busy group under the gray fault.

**Evidence.** `tests/m1/results/m1-wp8.md`, "BM-1 gray-fault rerun: site 0's member calls failing 30%" [measured for the moves and alarms].

**Flip.** A gray-fault run on a group with enough traffic to fill the window where `STEP_DOWN` still does not fire, or a quiet group that `suspect` does not move within B0-06's 20 s.

### BM-1's 24-hour half and F43, 2026-11-02

The 24-hour linearizability run `lin24` ended, F43's two stalls were traced, and M2-WP1 and M2-WP2's pure code found gaps in SHARED (`tests/m1/results/m1-wp8.md`, "The 24-hour half"; `tests/m1/results/m1-f43.md`). The lead made the decisions below under the operator's delegation of 2026-10-30.

#### BM-1's 24-hour half is not passed, and reruns with member-side ack times

**Decision.** BM-1's 24-hour half, the rerun of B0-11, is recorded as not passed. It reruns as `lin24b` on code where each member records, for every acknowledged write, its entry's commit time and the ack's send time on its own clock, and the checker asserts that every ack left at or after its entry's commit. No gate number changes.

**Why.** The gate is "24 h, zero violations", and the history cannot rule one out. `lin24` ran 2,739,691 operations on `repo:915635` with 0 unanswered, lost none of 869,257 acknowledged writes, put none of 774,418 refused CAS writes in the log, and kept three identical logs of 479,965 entries. Porcupine was Ok on all 6 keys widened 100 ms and 275 ms, and on the recorded clocks once the writes of 5 cut client chunks were restored from their request ULIDs. Two observations stay unexplained: entry 398032 needs 93 ms of ATL-LHR skew against a measured 49 ms, and 17 writes were acknowledged faster than any quorum could commit them on their own client's clock. Client-clock artifacts would explain both, and 4 of 25,031 clock samples show round trips below physics, but an early ack would too, the class of Milestone 0's 4-hour ticket-reuse bug.

**Beat.** Passing on the widened checks, which accepts the two observations on a clock argument the run cannot prove. Rerunning the same run with tighter clock sampling, which still measures the members' behavior only through client clocks.

**Evidence.** `tests/m1/results/m1-wp8.md`, "The 24-hour half, 2026-11-02: lin24 does not pass"; `tests/m1/lin/raw/lin24.report.json` [measured]. F44, since answered ("F44 answered: the fast acks were a client-clock artifact").

**Flip.** A member-side check that shows every ack sent at or after its entry's commit over 24 h, which passes BM-1's 24-hour half if Porcupine also passes; or one ack before its commit, which is a violation and sends the write path back.

**Since, 2026-11-03.** `QuorumProof` carries the leader, the commit index, the entry's `at`, the leader's commit time, and one `DurableAck { site, durable, at }` per counted site, and the host adds `sentAt` and `selfSyncedAt` to every `done` reply, in the product build. `ackcheck.ts` asserts six rules on the member's clock (`tests/m1/results/m1-wp2.md`, "Member-side ack evidence, 2026-11-03"). A planted early ack on the leader's own append was caught in 54 of 3,873 acknowledged writes in the simulator's GRAY half, also when the crate's proof hid it. `lin24b` runs on `repo:934783` from 2026-11-03T00:55:07Z to 2026-11-04T00:55:07Z after a 30-minute smoke with 0 violations on 18,225 writes [measured].

**Flip met, 2026-11-04.** `lin24b` showed every ack sent at or after its entry's commit over 24 h, 0 violations on 904,294 acknowledged writes, with Porcupine Ok and no acknowledged write lost, so BM-1's 24-hour half passes and Milestone 1 closes (`tests/m1/results/m1-wp8.md`, "The 24-hour half, 2026-11-04"; `tests/m1/lin/raw/lin24b.report.json` [measured]; "F44 answered: the fast acks were a client-clock artifact"). The write path stands. This entry stays as the record of why `lin24` alone did not pass.

#### A late member-call answer is still stepped (F43)

**Decision.** A member-to-member call that misses `CALL_DEADLINE` is reported failed at the deadline, as before, so `STEP_DOWN`'s window and the core's resend timing are unchanged. The host then awaits the answer for up to `LATE_LIMIT` (10 s) [guess] more and steps it if it comes. F43 is resolved and leaves OPEN-QUESTIONS.

**Why.** Both F43 stalls had one cause: the leader's host dropped every answer that came after 1 s, and AppendEntries carrying large inline packs often take 1 to 3 s to a far follower. In stall 1 the leader was resending entries 172 to 235, about 1.08 MB, not the 836 bytes `m1-wp8.md` assumed, and 414 of 424 answers took over 1 s (p50 1,999 ms), so it never learned the follower had matched 235. In stall 2 entry 175's 524 kB was answered in 1.76 to 1.79 s 14 times before one attempt came in under 1 s, holding the round 25 s. `Member::call_failed` already allowed a late reply to be stepped. On the hosted install the old code reproduced the slow round twice (16.6 s and 24.7 s) and the fixed code did not. In a 53-minute chaos soak with 374 pushes no round passed 5 s, no follower stayed behind a live leader, and 250 of 303 deadline misses (6.0% of calls) were answered late, the slowest in 8,802 ms.

**Beat.** A longer `CALL_DEADLINE`, which delays every failure, `suspect` and `STEP_DOWN` included. Smaller AppendEntries, which still lose every late answer to a far follower.

**Evidence.** `tests/m1/results/m1-f43.md`: the original logs, the `lag-old-*` and `lag-new-apac` reproductions, the soak, and `host::tests::an_answer_after_the_deadline_is_kept_and_the_miss_reported_first`, which fails on the old semantics [measured].

**Flip.** A late answer slower than `LATE_LIMIT` in a run (the soak's slowest, 8.8 s, leaves little margin), which raises the limit; or a stepped late answer that a measured case shows to be wrong to apply.

#### Porcupine's segmented check cuts each key on its own and keeps observed no-reply writes

**Decision.** The segmented check (`tests/m0/porcupine -seg`) that B0-11, BM-1's `lin24`, and `lin24b` rest on partitions the history by key and cuts each key on its own, from that key's own named state. A no-reply write that a later `get` saw, or a successful `cas` expected, is kept, with its return set to that first observer's return. A no-reply write nobody saw is lent to a segment only when the segment fails without it, and at most once. Cut points stay isolated operations whose result names the state; a failed `cas` is never a cut.

**Why.** The old mode had two bugs. It dropped every no-reply write, which M0 wrote "can only make the check stricter" (`tests/m0/results/m0-wp2.md`, B0-11, "The check"): true for Ok, false for Illegal, because a dropped write that a read saw makes a linearizable history look Illegal. That was the 2,000-seed simulator sweep's false Illegal. It also cut all keys of a file as one list, so a cut on one key set the start state of another key's segment, which can give a false Ok as well as a false Illegal. The gate runs fed one key per file and none had an observed no-reply write, so their verdicts stand: lin24's recorded-clock and 100 ms results and B0-11's at 0 ms and 100 ms are identical segment for segment on the fixed checker. lin24's 275 ms check was not rerun, because one key needed over 7 GB; its input takes the code path both versions share.

**Beat.** Dropping no-reply writes, which is unsound for Illegal. Checking every history whole, which needs more memory than a 24-hour key allows.

**Evidence.** `tests/m1/results/porcupine-seg.md`: 4 regression tests that fail on the old code, 3,000 random histories where segmented and whole checks agree on all 6,000 checks, 8,754 of 8,754 planted violations caught, and the reruns of lin24 and B0-11 [measured].

**Flip.** The segmented and whole-history checks disagreeing on any history.

#### `load` holds the log by bytes as well as entries (F45)

**Decision.** A member's `load` keeps at most `LOG_WINDOW` (1,000) applied entries and at most `LOG_BYTES` (8 MB [guess]) of them in memory, the newest that fit and always the last applied. `request` rows store each event in its `Fold::row_event` form, which drops inline pack bytes, and every multi-row read goes one row at a time. The value of `LOG_BYTES` stays open in F45.

**Why.** A count is not a memory bound: 1,000 entries of up to 1.5 MB can pass a 128 MB isolate, and a repository with 53 pushes of 0.7 to 1 MB inline packs failed to restore on all three members after a reset. `Push::replay` reads only refs and the sweeper only stored packs, so a `request` row needs no inline pack bytes.

**Beat.** A smaller `LOG_WINDOW` alone, which still has no byte bound for large entries. Keeping inline packs in `request` rows, which holds every recent pack twice.

**Evidence.** `tests/m1/results/m1-wp2.md`, "F45: restore holds a byte-bounded log": `the_in_memory_log_is_bounded_by_bytes` fails on the old code with 2,002,910 bytes in memory against a 300 kB budget, and `repo:196624` restored on all three members and took an 800 kB push [measured]; `tests/m1/results/m1-f43.md` for the crash.

**Flip.** A measured restore that exceeds the isolate at 8 MB, or catch-up slowed by SQLite reads below it, which sets the value.

#### SHARED's tiers, digest, and trailer as the fold code needs them

**Decision.** From M2-WP1 and M2-WP2's pure code:
- `SnapshotTrailer` carries `entry`, the digest of entry `to`.
- `fold_digest` is SHA-256 of the tier, `applied`, the term and digest of entry `applied`, and the sum mod 2^256 of SHA-256 over each row's (table, key, writing index, columns) [inference].
- Any `State` or `Draft` change touches both tiers, because `tier_of` sees the event and not the fold.
- `Resolved` keeps the core count of unresolved threads, so a `Comment` that resolves or unresolves a thread touches both tiers.
- `RepoSettings` is Core, and Metadata keeps `check_retention_days`.
- `Dismissed` touches both tiers.
- Each tier keeps its own `IdBlock` cursor.
- For one (sha, name), the higher run id wins, then the higher version, and a status's version is its `StatusId` [inference].
- A row's time is the `at` of the entry that last wrote it.

**Why.** Each is a rule the code needed that SHARED left out or stated two ways. A trailer without entry `to`'s digest leaves a restored tier unable to refuse a wrong `prev` on `to + 1`. Members compare `fold_digest`, so its bytes must be defined, and a sum is order-independent and updated per write. `tier_of` cannot tell whether an `IssueChange` targets a pull request. GHES's "Require conversation resolution before merging" gates merges on the unresolved count, and merges read only Core. Metadata pruning reads `check_retention_days`, and dismissal removes a core verdict. Both tiers assign ids after `MetaSplit`, and no tier owned the cursor.

**Beat.** Keeping the unresolved count in Metadata, which would make a merge wait on a metadata load. One shared id cursor, which neither tier could own without reading the other.

**Evidence.** The code in `crates/fold` and `crates/git` (258 workspace tests, 10 of 10 planted bugs caught); `crates/fold/src/tier.rs` and `crates/fold/src/snapshot.rs`.

**Flip.** A run-id order that is not monotonic per (sha, name) because two Actions shards report runs for one check, which needs a cross-shard tiebreak; or a measured `fold_digest` disagreement between members whose rows are equal.

**Since, 2026-11-03** (M2-WP2, `crates/git/src/tiers.rs`). A thread's first review comment and every `Deleted` also touch Core, besides `Resolved`, because `Resolved` names only the thread: the first comment opens it unresolved and maps it to its pull request, and a deletion does not say whether it named a thread. Each tier's cursor takes its own half of every leased block, Metadata the lower and Core the upper, and an event that touches Core takes Core's ids [inference: any fixed split works; this one keeps the cursors from meeting].

### Milestone 1 closes, and M2-WP4's shape, 2026-11-04

BM-1's 24-hour half passed on `lin24b`, which answered F44, and M2-WP4's pure code found gaps in SHARED and DESIGN (`tests/m1/results/m1-wp2.md`, "The 24-hour rerun's result, 2026-11-04: lin24b passes"; `tests/m1/results/m1-wp8.md`, "The 24-hour half, 2026-11-04"; commits `fb03345` to `999e5b9`).

#### F44 answered: the fast acks were a client-clock artifact

**Decision.** `lin24`'s 17 fast acks and its entry-398032 ordering are read as client-clock artifacts, not early acks, and BM-1's 24-hour half passes on `lin24b`. F44 leaves OPEN-QUESTIONS. The earlier inference that a frozen Workers clock can only lengthen a measured interval is withdrawn: a stamp can be early as well as late.

**Why.** `lin24b`'s member-side check found every one of 904,294 acknowledged writes' replies sent at or after its commit on the answering member's clock, each counted by a durable quorum of two distinct sites including the answering site. Its quorum and durable rules are causal, not timed: a follower counts only through an AppendEntries reply naming the index and the leader only through its `SelfAck`, so a stalled clock cannot pass an early ack. The fast-ack pattern recurred on 59 writes whose client interval was shorter than the site-to-site round trip in their own proof, and all 59 pass the member check; 137 writes had a client interval shorter than `commitAt − entryAt` on the leader's clock, by up to 194 ms, which a real interval cannot be. Member clocks stalled too: in 15 entries a follower's ack reached the leader 0 to 40 ms after the entry was built on the leader's clock, over a link of about 98 ms round trip [measured; the runtime's mechanism is not established]. That the same pattern explains `lin24`, whose members recorded no evidence, is [inference].

**Beat.** Keeping F44 open until `lin24`'s own history is explained, which it cannot be without member evidence. Treating the member clocks as exact, which the 15 stalled entries refute.

**Evidence.** `tests/m1/results/m1-wp2.md`, "F44: answered by the member clocks" and "Lost writes: none"; `tests/m1/lin/raw/lin24b.report.json` [measured].

**Flip.** A member-side ack-check violation in any later run, which is an early ack and sends the write path back; or a fast ack whose member evidence is missing or fails a rule.

#### M2-WP4's wire, blob, and archive rules as the code needs them

**Decision.** From M2-WP4's pure code:
- The LFS batch's `basic` transfer hrefs point at the edge, which serves upload and download itself from `R2Key::Blob` objects.
- `R2Key::ArchiveCache` is keyed by `network`, `commit`, a digest of the prefix, and `format`, not by tree.
- `LfsLockEvent` is defined, metadata tier: `Created { id, path, refspec, owner }` and `Deleted { id, actor }`.
- The install-host rule is by label: no label equal to, or ending in, `.github.com`, `.ghe.com`, or `.localhost` (and not those names themselves); `notgithub.com` is allowed.
- A legacy repository-scoped node id resolves only after the request names the repository, and cannot route by itself.
- New guesses: `BLOB_MAX` = 100 MiB; `generate_release_notes` writes the tag as the name with an empty body; `search` is a counted rate-limit window. Blob GC after `RESTORE_WINDOW` is not built.

**Why.** A presigned URL needs an R2 S3 credential on the request path and names only one bucket, while a `DurableBlob` lives in two. `git archive` writes the commit time and the prefix into the bytes, so one tree under two commits or two prefixes gives different archives. SHARED listed `LfsLock` without its type. Clients classify and normalize only those names and suffixes (`research/clients.md` #1 and #2), so the textual rule refused hosts every client treats as GHES. A legacy id carries no repository id. The pinned rate-limit overview requires `core` and `search`.

**Beat.** Presigned URLs per bucket, which put an S3 credential on the edge and lose the second copy on a read. Keying archives by tree, which serves one commit's bytes for another.

**Evidence.** `crates/git/src/blob.rs` (`archive_key` and its test), `crates/wire/src/host.rs`, `crates/api/lfs/README.md`, and `crates/api/tests/blobs.rs`: 65 captured requests replayed and 93 replies validated with 0 failures; BF-25's and BF-26's pure halves pass [measured, in the crate tests].

**Flip.** A captured client that needs a presigned or third-party href, such as a custom LFS transfer agent; a client that classifies a label-allowed host as GitHub.com; or a measured blob that needs more than `BLOB_MAX`.

### BF-20 passes after the restore fix, 2026-11-04

BF-20's 2026-11-03 failure was traced and fixed on branch `restore-fix`, and the rerun passed all three clauses (`tests/m2/results/m2-wp1.md`, "2026-11-04: restore fix and BF-20 rerun").

#### BF-20's wedge: restore held every `request` row in memory (F46)

**Decision.** A group's `request` rows live only in member SQLite. `GroupFold` takes a `RequestStore`, and the site's `RequestSql` reads the member's own `request` table one row at a time by id, with the input's unflushed rows and the retention cut on top; restore reads no request row. BF-12's semantics are unchanged: a re-send inside `REQUEST_RETENTION` gets its stored outcome. `REQUEST_RETENTION` stays 48 h [guess]. F46 is resolved and leaves OPEN-QUESTIONS.

**Why.** The trigger was restore. A probe build on `repo:196628` measured wasm memory at 35.6 MB before the `request` rows, 86.3 MB after reading 90,048 of them (8.4 MB of events), and 174.8 MB after `Member::restore`, which copies the fold, against a 128 MB isolate. An isolate out of memory is reset, and the next request restores and runs out again, so every POST that awaited anything died while synchronous GETs answered. It persisted across 4 redeploys because the `request` table survives redeploys within `REQUEST_RETENTION`. Three more defects then kept `repo:196628` down, each a panic or out-of-memory error: M1-era groups whose core tier sat at 0 behind `applied` panicked on their first apply; three metadata tables rekeyed by M2-WP2 after seeding kept their old shape under `CREATE TABLE IF NOT EXISTS`, so the snapshot alarm panicked on every scan; and the edge's lookup probe `{"q": {}}` read all 303,144 refs and ran the leader's `/read` out of memory. They spread because workers-rs 0.8.7, after any panic or wasm error, rebuilds the wasm instance for every Durable Object in the isolate, so one member's panic fails its neighbours' in-flight requests; DESIGN's chain-group instance row now says so. Fixes: `Family::catch_up` replays the stored log into a core tier behind `applied` or refuses the restore, `tier::reshape` remakes an empty table of an older shape and refuses one with rows, and the probe is `{"q": {"probe": true}}`.

**Beat.** Holding the rows in memory under a count or byte bound, as F45 does for the log, which still loads them at restore and caps the duplicates a group can answer. A shorter retention, which the 24 h orphan sweep bounds below and which the M1 `names` chains' restore check refused when tried (deploys 12 and 13).

**Evidence.** `crates/raft/tests/restore_memory.rs` fails on `1beb531` ("restore allocated 22519799 bytes for 90048 request rows on storage") and passes on the fix; wasm memory after restore was 67 to 70 MB on every `repo:196628` member, against 174.8 MB; commits `3eca218` and `1a851f8` [measured]. On storage the rows cost about 4.9 GB at a sustained 300 commands/s over 48 h (52 M rows at the measured 93 bytes of event each, before row and index cost) [inference]; that does not fit beside a 4 GB metadata tier and a 2 GB log under `CHILD_GROUP_AT`, while journey 9's 27.6 commands/s needs about 0.44 GB (`design/DESIGN.md`, Scale, "Member storage").

**Flip.** A group that sustains near 300 commands/s for hours, whose `request` rows then push member SQLite toward `CHILD_GROUP_AT` or `SQLITE_FULL`, which needs a shorter retention or a per-group byte cap; or a duplicate lookup by id measured slow enough to show in commit latency.

#### BF-20 passes, on a pooled baseline

**Decision.** BF-20 is recorded as passing all three clauses at 300 commands/s, judged against the pooled held baseline.

**Why.** The 3.99 GB tier was snapshotted twice; the snapshot under load, to 1313, refolds to the key's digest from both copies and matches all three members. The apply step's p99 was 0 ms in every phase. The leader's entry-age p99 was 838 ms during the snapshot against 840 ms for the pooled held baseline. Workers Logs held no memory or CPU limit outcome, and 423,000 of 423,000 commands succeeded. The baseline drifted across the session, 528 ms before and 840 ms after, with the snapshot held in both, so the drift is not the snapshot's; against the earlier baseline alone the to-commit phase is +59%, and pooling the two held phases, which bracket the snapshot in time, is a judgement call [inference]. Their bootstrap intervals overlap.

**Beat.** Failing on the earlier baseline alone, which charges the session's drift to the snapshot. Passing on the later one alone (-0.2% as well), which hides the drift.

**Evidence.** `tests/m2/results/m2-wp1.md`, "BF-20 rerun" and "Request limits" [measured]. The `commit_tx` aborts on site c were F47, since answered ("F47 answered: the wasm `unreachable` is an isolate out of memory").

**Flip.** A rerun with held phases interleaved with snapshot phases in which the snapshot phase is more than 10% slower than its neighbours; or a limit outcome in Workers Logs during a snapshot.

### F47, M2-WP5, and M2-WP3's pure half, 2026-11-05

F47's abort was traced and fixed on `graft-m2b`, M2-WP5 ran BF-24 on the hosted install, and M2-WP3's pure code settled seven gaps in the merge and queue shapes (`tests/m2/results/f47.md`; `tests/m2/results/m2-wp5.md`; commits `80b85ac`, `d0f09be`, `15ddbee`, and `53ac4ee`).

#### F47 answered: the wasm `unreachable` is an isolate out of memory

**Decision.** F47's abort is an isolate out of memory, not a Rust fault, and four fixes remove its causes. `Member::restore` takes the stored state by value and moves the log window and fold into the member. The audit replay reads the log one row at a time. The audit refolds at most `REPLAY_BYTES` (16 MB [guess]) of stored log in memory and reports a larger chain as skipped. `Host::kick`'s tick loop holds a `Weak<Host>` across each delay. F47 leaves OPEN-QUESTIONS, and auditing the skipped chains was F48 ("F48: the fold audit refolds from the newest snapshot").

**Why.** A probe build caught the trap at 138.1 MB of wasm memory: inside `transactionSync`, `SqlStorage::exec`'s string import failed with `RangeError: Invalid array buffer length`, wasm-bindgen rethrew it as `Critical error` and set the instance's terminated flag, and the next import shim's check executed `unreachable`, with no Rust panic. Four paths held a second large copy of a member's state: restore's clone of the log and fold (up to 118 MB of live heap), the audit replay collecting every log row (up to 131 MB), the in-memory refold even one row at a time (110 MB for `repo:65536`'s 102.6 MB of log), and a reset member's tick loop parked on a timer `ctx.abort()` never fires, whose `Rc<Host>` kept the reset member alive (1, 2, then 3 hosts at 40, 82, then 124 MB). Because workers-rs 0.8.7 rebuilds the wasm instance of every object in the isolate after such an error, each abort reset its neighbours too.

**Beat.** Catching the error in the glue, which leaves the instance terminated. A larger isolate, which Workers does not offer. Keeping the full audit refold, which takes the isolate past 128 MB on any chain with about 100 MB of log.

**Evidence.** `tests/m2/results/f47.md`: on the old code 13 lost instances in 3.24 h on `graft-m2b` under BF-20's load and an amplifier, and on `e5c6021` none to memory in 1.28 h with 71 audits and 14 resets, peak wasm memory 89 to 120 MB per site against 142.5 MB before; `restore_holds_no_second_copy_of_the_stored_log_or_fold` fails on `7a47f94` and passes on `c3e9fa1` [measured]. The one `Critical` in the final window was a separate Rust panic in `tier.rs` on a storage read refused with "Network connection lost". Since fixed in `131ee88`: a refused fold read, scan, row, or clear marks the tier lost and resets only that member with `ctx.abort()`, and a planted refused read on `repo:65537` reset that member alone while two neighbours in its isolate kept their boot ids [measured, `tests/m2/results/m2-wp5.md`, "The `tier.rs` panic"].

**Flip.** A lost instance with F47's signature on the final build; or a member whose own resident state, with no second copy, reaches the isolate limit, which needs a smaller `LOG_BYTES` or tier cache rather than these fixes.

#### `PrSync`'s first attempt runs in band, and Queues carries only the re-send

**Decision.** Operator decision, 2026-11-05, under the delegation (`FRAME.md`, "Operator decision, 2026-10-30, delegation"). After a head push the repository leader runs the `PrSync` batch's first attempt itself: the fresh read, the git cache's `/merge`, and the commit. The per-site queue carries only the re-send of a batch the committed cursor has not passed. The cursor, content-derived request ids, and `decide_sync`'s checks are unchanged. It is built (`12ec22a`, `76abf81`) and BF-24 passes on it.

**Why.** BF-24 passed on correctness in every run and failed its 10 s clause: 46 of 50 clean head pushes and 25 of 25 conflicts met 10 s, but 4 of 50 queue messages sat in Queues about 36.5 to 37.6 s before their first delivery, with no retries recorded, and the leader's 30 s re-send then covered them. Queue delivery was the largest step, p50 2.0 s and p90 5.1 s, against 1.9 s for the merge and 0.6 s for the commit, so the gate held only while Queues delivered within about 6 s [measured, `tests/m2/results/m2-wp5.md`, "Where the time goes"].

**Beat.** A shorter `REDELIVER_AFTER_MS` (5 s), which bounds the tail near 9.5 s but keeps every push's p50 behind a queue hop and doubles handler runs on slow deliveries. Keeping the queue path and accepting a p99 over 30 s.

**Evidence.** `tests/m2/results/m2-wp5.md`, "BF-24" and "2026-11-05: the in-band first attempt, and BF-24 rerun on `graft-m2b`" [measured].

**Flip.** The rerun misses 10 s at p99 with the in-band attempt, or the in-band merge holds a leader's alarm long enough to slow its commits, measured; or a measured Queues delivery bound under about 6 s at p99, which would make the queue path enough.

**Flip met? No, 2026-11-05.** On `graft-m2b`, `repo:65538`, 75 of 75 trials met 10 s on the final build (`ca0d181`): clean p50 5.50 s, p99 6.44 s; conflict p50 5.49 s, p99 5.81 s; every first attempt ran in band and none needed the queue. The leader's commits did not slow: `commitMs` p50 1,921 ms, p99 2,474 ms over 151 commits, in line with M1-WP8's steady `Push` commits of 2.1 to 2.8 s at p50. Leader transfers, a reset old leader, purged queues, and dropped first attempts were each covered, the dropped ones by the 30 s re-send at 34.7 to 38.2 s, and `tests/m2/bf24/plant.py` caught 14 of 14 planted bugs, 3 of them on the in-band path [measured, `tests/m2/results/m2-wp5.md`, "2026-11-05"]. The leader-change run found one bug, fixed in `76abf81`: a dispatcher keyed by term alone let a member reset and restored in the same term wait on its old self's first attempt, so dispatchers are keyed by (term, boot), and an attempt older than `REDELIVER_AFTER_MS` counts as lost. The in-band attempt runs in the leader's isolate for up to `FIRST_FOR` (25 s [guess]), now a row in DESIGN's `event pipeline` failure modes.

#### M2-WP3's merge and queue shapes as the code needs them

**Decision.** From M2-WP3's pure code:
- `Merged` touches both tiers, so `Merge` and `QueueMerge` entries are tagged with both, though `decide` reads only Core.
- `QueueOp.actor` is `Option<Principal>`: the edge sets it for `Enqueue` and `Dequeue`, and the `MergeQueue` consumer's `Build` and `Eject` carry none. `QueueMerge` has no actor, and each `Merged` names the enqueuing user.
- Under `HeadGreen`, `QueueMerge` also refuses when any landed entry's own queue ref has a failed or pending required conclusion.
- The consumer judges only the first built entry without a passing result, and ejects it on a failure or a timeout; every later queue ref holds its changes.
- `Merged` gains `by`, and `MergePlan` gains `up_to_date`, with `verified` and `engine`.
- The `code_owners`, `last_push`, and `signatures` rules refuse every merge until the core tier holds their data.
- Deleting a queue ref prunes the `required_check` rows of its SHA, unless an open pull request has that SHA as its head [inference].

**Why.** A follower that saw a `Merged` in a Core-only entry would refuse it, which the BF-1 plant shows. A consumer names no credential (`design/DESIGN.md`, Shape, `CredentialShard`), but `PrState::Merged` names who merged. DESIGN's `HeadGreen` read only the last entry's checks, so a landing planned before an earlier entry's failure was reported would land past it. A later entry's failure may be the earlier entry's change. `strict` needs ancestry the fold cannot see, and the git cache that ran `merge-tree` can. Without `CODEOWNERS`, the last pusher, or signature checks, passing those rules would merge what GHES refuses. Queue refs come and go with every build, so their required rows would grow the core tier.

**Beat.** A Core-only landing with a separate metadata entry, which splits a merge across two entries. A synthetic consumer principal, which would need a credential no consumer has. Ignoring the three unbuilt rules, which merges past them.

**Evidence.** `crates/git/src/merge.rs`, `queue.rs`, `queue_consumer.rs`, `tiers.rs`, and `repo.rs`; `crates/git/tests/merge_queue.rs`, 13 tests on three Raft members, pass; `tests/m2/wp3/plant.py` catches 14 of 14 planted bugs, rerun on `c7a3b3b` [measured, in the crate tests].

**Flip.** The hosted BF-21 or BF-23 shows a landing the pure tests allowed; or GHES 3.22 is shown to land a `HeadGreen` group past an earlier entry's pending check.

### M2-WP3 hosted, the site-day fixes, AM1-WP5, and M3-WP5, 2026-11-06

M2-WP3's hosted half passed, the site-day work answered F49, and the pure halves of AM1-WP5 and M3-WP5 settled gaps in the trust view, learners, `InstallSnapshot`, and the fold audit (`tests/m2/results/m2-wp3.md`; `tests/m2/results/site-day-fixes.md`; `tests/am1/results/am1-wp5.md`; `tests/m3/results/m3-wp5.md`).

#### Merge packs ride inline, and `MergeQueue` runs its first attempt in band

**Decision.** A merge's or a queue group's pack rides inline in its `MergeCmd` as `PackRef::Inline`, at most `MERGE_PACK_MAX` (900 KB), and the git cache's answer is refused above it; nothing is placed in the buckets for it. The `MergeQueue` consumer runs each batch's first attempt in the leader's isolate, on `PrSync`'s pattern, with re-sends through the site queue, its cursor moved by `MergeCmd::Cursor`, and a per-base timer from `Step::Idle { at }`.

**Why.** A merge commit's own objects are a few KB [inference], so placing them in two buckets would add a round per landing for nothing an inline pack lacks. The in-band path took `PrSync` from a p99 over 30 s to 6.44 s (BF-24), and the queue drives landings the same way.

**Beat.** Placing every merge pack as a `DurablePack`, which DESIGN's "Merge queue" text described. A queue-only consumer, which BF-24's first run showed can hold a message about 37 s.

**Evidence.** `tests/m2/results/m2-wp3.md`: BF-1 3 of 3, BF-21 100 runs, BF-23 100 of 100, BM-2's stale-`parent_tip` part 100 of 100, the per-base timer 5 of 5, 478 GraphQL and 63 REST replies valid, and 14 of 14 planted bugs [measured]; `crates/git/src/merge_cache.rs`.

**Flip.** A real group whose pack is over 900 KB, such as a squash of a large change, which needs placement; or a `MergeQueue` first attempt measured holding its leader's commits.

#### F49 closed: the 401 window was the driver's empty token

**Decision.** F49 closes with no server change. A credential read that cannot get an answer already renders 503 with `Retry-After: 5` on REST, GraphQL, and LFS, and a 401 means a request with no credential or a token no account holds. The test driver now refuses to run with an empty token. On git smart HTTP the same failed lookup renders 500, and should render 503; that follow-up is in BUILD-PLAN M2-WP4.

**Why.** All 24 requests of the run that got 401 carried an empty `Authorization` header, because `edge-drive.sh` read an empty token line and did not check it; the old driver with a missing line reproduces it. Across seven site days that reset every member, a poll once a second saw 0 of 8,750 valid-token requests get 401 and 439 of 439 unknown tokens get 401. A member answers a read only after its constructor's `restore`, a failed restore answers 500, and the lookup retries that for `LOOKUP_FOR` and returns an error rather than "no row" [inference from `workers/site/src/host.rs` and `crates/lookup/src/lib.rs`, consistent with the probes].

**Beat.** Changing the token read to answer 503 on an empty row, which would weaken the 401 that a revoked or unknown token must get at once.

**Evidence.** `tests/m2/results/site-day-fixes.md`, "The 401 window (F49)" [measured]; `ee08088`.

**Flip.** A server 401 to a valid token during or after a site day, in any later probe.

#### Trust-view details the code needed

**Decision.** From AM1-WP5's pure code, in AGENT-LAYER's trust view block:
- `Vary: Accept` is added only to opted-in replies of trust resources; a default reply is byte-identical to GHES and carries no new header.
- `is_agent` is false until AM1-WP2's `AgentApp` rows exist, so AM1-WP5 also depends on AM1-WP2 for that field. Since AM1-WP2's pure half, `is_agent` comes from `AgentMeta` and `AgentApp` rows ("AM1-WP2's agent rows as the code needs them").
- A commit's author is never trusted: its name is a claim in the git object, which no account vouches for.
- Journey A4's list of what `visible` drops now points at the Shape block's full list rather than repeating three items of it.
- A collapsed `<details>` drops its `</details>` too, which the code always did.

**Why.** `Vary: Accept` on every reply would change every default reply's bytes, which LM-1's byte diff forbids; planted bug 2 is that change, and the byte comparisons catch it. Each other point is how the code and the M0 prototype behave, and the design either said less or said it two ways.

**Beat.** `Vary: Accept` on every reply, the usual HTTP caching advice, which would break the byte diff.

**Evidence.** `tests/am1/results/am1-wp5.md`: LF-4's pure half 128 of 128, 1,209 byte comparisons with 0 differences, 18 of 18 planted bugs [measured]; `crates/api/src/trust.rs`.

**Flip.** A shared cache measured serving an opted-in reply to a default client, which would need `Vary` on more replies, or a different URL for the agent view.

#### Learners, `GroupLoad`, and `InstallSnapshot` as the code needs them

**Decision.** From M3-WP5's pure code:
- `GroupLoad` is each region's peak read rate (req/s) for the day. `place_learners` returns no region under `LEARNER_AT`; above it each region with reads is owed `ceil(rate / 1,000)` learners, 1,000 being Scale's rate per learner, handed out one at a time to the busiest region still owed one, up to `MAX_LEARNERS` [inference].
- In a group's messages, learner slot k is addressed as `SiteId(192 + k)`, above every site id, because a learner may share its site with a voter or another learner.
- `InstallSnapshot` also carries entry `through`'s term and digest, the committed config and id block at `through`, and an AppendEntries round's fields.
- Its archives reach back to the snapshot or to `REQUEST_RETENTION` before `through`, whichever is earlier, so the member rebuilds the `request` rows inside retention.

**Why.** The design named `GroupLoad` without defining it. Raft's per-peer maps need one id per member, and a site id is not one. No tier snapshot holds the config or the id block, and the next append must chain on `through`'s digest. A member without the `request` rows inside retention would decide a re-sent request whose entry lies below the snapshot a second time, which BF-12 forbids.

**Beat.** Addressing learners by (site, slot) pairs, which would change every message's envelope. Keeping `request` rows in the tier snapshots, which would grow every snapshot by up to `REQUEST_RETENTION` of requests.

**Evidence.** `tests/m3/results/m3-wp5.md`: BF-28 and BF-29 pass in memory on real raft members, 9 of 9 planted bugs, the long sweep and Porcupine Ok [measured]; `crates/types/src/placement.rs`, `crates/raft/src/member/mod.rs`.

**Flip.** B0-18's rerun showing a learner region rule that misses its read target, which changes `place_learners`; or a hosted install that cannot fit `REQUEST_RETENTION` of archives in one install.

**Flip met? No, 2026-11-07.** B0-18's rerun passed 6 of 6 on `graft-m2b` with `place_learners`' 16 regions, every learner p99 under 1 s; its failures had been call depth, not placement ("A member never calls a learner that is calling it").

#### F48: the fold audit refolds from the newest snapshot

**Decision.** The fold audit refolds a sampled chain from each tier's newest committed snapshot at or below the audited index, not from genesis. A job streams the snapshot's frames into empty scratch tables one frame at a time (`Restore`), applies the stored log tail one entry at a time with the same `apply` (`refold_segment`), and compares the `fold_digest` with every member's. A refold from one snapshot to the next must reach the next trailer's digest, so archived ranges are audited segment by segment. The first snapshot is trusted as written. F48 leaves OPEN-QUESTIONS. The site's alarm-driven cron, shared with the install loader, followed in M3-WP5's hosted half and skips nothing for log size ("M3-WP5's hosted shapes, and the fold audit at its size limit").

**Why.** Memory is one frame and one entry, not the fold, and the tail is at most `SNAPSHOT_EVERY` entries or `SNAPSHOT_BYTES` of log, because a snapshot is due by then. The refold from genesis held the fold in one isolate, and a 102.6 MB log took one past 128 MB (F47).

**Beat.** Refolding across several requests from genesis, which reads every archive on every audit. A larger isolate, which Workers does not offer.

**Evidence.** `tests/m3/results/m3-wp5.md`, "F48": for both tiers, a refold from the snapshot at 21 plus the tail matches all three voters, a refold from 21 to the next snapshot reaches its trailer digest, a dropped entry is refused, and a changed entry gives a different digest [measured, in memory].

**Flip.** A hosted audit whose snapshot-to-snapshot refold disagrees with the trailer on a chain whose members agree, which would mean the trailer or `snap_undo` is wrong rather than `apply`.

### BM-2, M3-WP5 hosted, AM1-WP1, and M3-WP1 and WP2, 2026-11-06

BM-2 failed on its fresh-leader latency clause, M3-WP5's hosted half passed BF-28 and BF-29 and failed the B0-18 rerun, and the pure halves of AM1-WP1 and of M3-WP1 and WP2 settled gaps in admission, fan-out, names, and tokens (`tests/m2/results/m2-wp6.md`; `tests/m3/results/m3-wp5.md`, "2026-11-06"; `tests/am1/results/am1-wp1.md`; `crates/identity`).

#### A merge plan runs on the leader site's shard

**Decision.** `GitCacheShard::merge` runs on the shard of the base leader's site, as `PrSync` and the `MergeQueue` consumer already did, not on the network's home shard or the base site's shard. `decide` takes the smallest free `StatusId` at or above the one the edge minted, returns it, and the 201 renders it.

**Why.** The leader site's shard is the one `PrSync` and `MergeQueue` keep caught up, so a merge there rarely waits on a catch-up, which cost 290 to 610 ms a plan once leadership moved. The edge mints a `StatusId` from its clock, and the `status` table is keyed (sha, -id), so two statuses posted in one millisecond on one commit replaced each other's row: BF-22 read back 495 of 500.

**Beat.** The network's home shard (DESIGN's `GitCacheShard` text) and the base site's shard (journey 2's), each of which needs a catch-up before most plans after a leader move. For status ids, failing the second post, which GHES does not do.

**Evidence.** `tests/m2/results/m2-wp6.md`, "Merge latency", "BF-22 reduced", and "Design sentences contradicted": the shard's own timing showed catch-up 289 to 613 ms against a 76 to 84 ms merge; `bf22e` read 500 of 500 after the fix, and `two_statuses_minted_in_one_millisecond_keep_both_rows_and_the_later_wins` fails without the free-id loop [measured].

**Flip.** A placement where the leader site's shard is measured further behind than the home shard on most plans; or F51's decision moving `StatusId` to the group's `IdBlock`, which removes the collision.

#### BM-2's fresh-leader merge p99 fails, and the gate stays

**Decision.** BM-2 is recorded as failing: journeys 2 and 3 pass 10 of 10 and the stale-`parent_tip` part 100 of 100, but the merge p99 on a freshly elected leader is 3.20 s against 2 s. The gate text is unchanged, Milestone 2 stays open, and the latency work continues on branch `bm2-latency`. The seven serial steps the run removed match "The write path" and "Commit and read paths", which the code now follows; neither section changes.

**Why.** AGENTS.md keeps a failing gate as written. What remains is cross-region round trips, with identity leaders on enam and the new repository leader on weur or apac, not a step the design orders serially.

**Beat.** Loosening the gate to the measured p99, which needs an operator decision on merge latency, as the 2026-10-28 one was for commit latency. Reading 91 samples as too few: 63 of 91 are over 2 s, which no count can turn into a pass.

**Evidence.** `tests/m2/results/m2-wp6.md`: p99 3.20 s, p50 2.28 s, n = 91 on 91 new terms; enam 2.09 s, weur 3.20 s, apac 3.16 s; the per-step split per site [measured].

**Flip.** A rerun on the same placement with p99 at most 2 s; or an operator decision that accepts the measured cross-region merge latency, as "Measured latency is accepted" did for commits. Met on 2026-11-06: the operator reworded the clause ("BM-2's latency clause measures a home-site leader, with a looser bound off home").

#### AM1-WP1's admission and fan-out shapes as the code needs them

**Decision.** From AM1-WP1's pure code, in the agent block of both DESIGN and AGENT-LAYER:
- `TestMerge` is one computed merge (`n`, `base`, `head`, `result`), recorded as SHARED's `TestMerged`, and `MergeBudget` is the budget: per network in shard memory, refilled at TEST_MERGE_BUDGET of the k shards' vCPU, with pushes and dispatch allowed one merge into deficit and GETs leaving half a second's budget.
- `fanout` takes `now` and a `Fanout` the network leader keeps, holding k and since when the load has been above or below the lines, so it stays pure.
- `DEMOTE_PER_MIN` counts the fold's committed writes of the last minute plus the leader's unresolved and refused ones, held in leader memory [inference].
- The leader refuses at `AGENT_QUEUE_MAX`, before staging, and the edge renders the refusal.
- A hot group stages every class's push over `INLINE_MAX_HOT`; while placement fails, any group refuses an agent pack over it.
- An owner's pool share has no plan field, so it is HOT_POOL - 1 [guess].
- `hot` counts pushes over the last 5 s of committed entries (`HOT_WINDOW`).

**Why.** The design's `TestMerge` enum and the code's recorded merge were two things under one name, and the budget had no type. A `fanout` that takes only loads and k cannot hold a 3 min or 1 h window without a clock. Without refused writes in the count, a demoted token whose writes are refused falls back under 60 committed writes and is classed Human again. The other points are where the design said less than the code needs.

**Beat.** Keeping the `TestMerge` enum, which nothing builds. Counting refused writes in the fold, which would need a command per refusal. A per-plan share field before plans exist.

**Evidence.** `tests/am1/results/am1-wp1.md`: B0-29 to B0-35 pass in memory, 23 of 23 planted bugs caught (including "`DEMOTE_PER_MIN` ignores the fold" and "owner share not enforced"), the long sweep and Porcupine Ok [measured]; `crates/raft/src/admit.rs`, `crates/raft/src/member/leader.rs`, `crates/git/src/shard.rs`, `crates/git/src/prsync.rs`.

**Flip.** LM-1's hosted rerun showing a demoted actor classed Human after a leader change (the refused count is leader memory), or a plan model that needs per-owner shares other than HOT_POOL - 1.

#### F50 closed: an account's login row on `account_shard`

**Decision.** `NamesEvent::AccountLogin { account, login }`, tag 12, on `account_shard(account)`, holds each account's current login. `SetAccountLogin` reads the login's own `Names` shard as a dependency and refuses unless the account holds the login there, so a write a rename overtook is refused. The edge reads logins with one read per distinct `account_shard`, all at once, at most 24 shards a request [guess]; an account past that, or with no row, renders as `ghost`. The `Audit` finisher writes a missing row after `LoginClaimed`, the commit point. F50 leaves OPEN-QUESTIONS.

**Why.** `Names` had `LoginClaimed` (login to account) but no row from an account to its login, so on the hosted edge every login but the owner's rendered from the test-only `LOGINS` variable.

**Beat.** A login field on the `Account` row, which changes Milestone 1's stored bytes. Reading the login from the account's `Identity` chain, which is one more group per account rendered.

**Evidence.** `crates/identity/src/names.rs`, `crates/identity/src/finish.rs`, `workers/edge/src/api.rs` (`LOGIN_SHARDS_MAX`), and `crates/identity/tests/m3.rs`; `tests/m2/results/m2-edge.md` for the gap [measured].

**Flip.** Replies naming more accounts than 24 shards cover in common journeys, which would need a batched login read or a cache.

#### M3-WP1 and WP2's names and token shapes as the code needs them

**Decision.** From the identity crate's pure code:
- `NamesEvent::AppSlug` carries `owner`, so the `Audit` finisher knows whose `Authz` chain to finish a claim on.
- `CLIENT_SECRETS_MAX` is 2 [guess].
- A GitHub App's `ClientId` is `Iv1.` and its App id in 16 hex digits [inference].
- An OAuth app's commit point is its `Client` route claim, as an App's is its `AppSlug`; `Authz` refuses a record its claim does not name.
- `TokensEvent::StoredWith` (tag 11) is `Stored` with its optional fields, beside `Stored`, so Milestone 1's rows keep their bytes.
- `TokenKind::Installation` exists for parsing and is never stored.
- The crash gap between `CodeUsed` and the new token's `Stored` is the design's "a failed second commit sends the client back to the start": nothing is stored, so `Audit` has nothing to finish.
- Names, Tokens, and Authz take format v2 for every command that writes an event Milestone 1 did not have; members read them after a site day onto a release that lists them, and the edge sends them only after `FormatsEnable`.

**Why.** Each is where the design said less than the code needs, and none changes a recorded behavior. Deriving the App's client id from its id makes it unique with no claim of its own. Keeping the M1 encodings means no stored entry changes.

**Beat.** Random App client ids, which need their own uniqueness claim. A new `Stored` encoding, which would change Milestone 1's stored bytes.

**Evidence.** `crates/identity/src/apps.rs`, `names.rs`, `tokens.rs`, `authz.rs`, `lib.rs` (`NAMES_FORMAT_V2`, `TOKENS_FORMAT_V2`, `AUTHZ_FORMAT_V2`), and `crates/identity/tests/m3.rs`, which pins Milestone 1's bytes and replays a used code after a leader change [measured, in memory].

**Flip.** GitHub client ids measured with a form an App's derived id collides with, or a client that needs more than two live secrets.

#### M3-WP5's hosted shapes, and the fold audit at its size limit

**Decision.** `place_learners` runs from the leader's `/learners` route until `graft-ops` exists. An `R2Key::Archive` object is `gyfarch\0`, version 1, then each stored log row in the raft codec behind a 4-byte big-endian length, so an install applies entries byte for byte. Learners write no `MemberRegistry` row. The site's fold audit refolds from snapshots, skips nothing for log size, and skips only a tier whose snapshot is over `AUDIT_SNAPSHOT_MAX` (256 MB [guess]); how to audit such a tier is F52. B0-18's rerun failing 2 of 6 is recorded in BUILD-PLAN, M3-WP5, with the investigation on branch `b0-18`.

**Why.** No `graft-ops` is deployed on the test account. The design gave archive objects no format, and JSON would change entries' bytes. A learner never votes and is found through its group's config, so the registry, which `ReplaceMember` and placement moves read for voters, does not need it. A tier's scratch copy must fit beside it in the member's 10 GB.

**Beat.** A JSON archive. Registry rows for learners, which `ReplaceMember` would then have to skip.

**Evidence.** `tests/m3/results/m3-wp5.md`, "2026-11-06": BF-29 and BF-28 pass hosted; the deep audit of `repo:65536` refolded 107.7 MB of log in 213 batches at 64.9 MB peak wasm memory, equal to all three voters; B0-18 passed 2 of 6 runs [measured]; `crates/fold/src/archive.rs`, `workers/site/src/load.rs`.

**Flip.** A `graft-ops` cron deployed, which moves the route's job to it; or a hosted audit whose snapshot-to-snapshot refold disagrees with the trailer on a chain whose members agree (F48's flip).

### BM-2's latency work and M3-WP3's pure half, 2026-11-06

`bm2-latency` cut four round trips from a fresh leader's merge and BM-2 still failed; M3-WP3's pure half passed its six gate rows in memory and settled gaps in identity, sessions, and authorization (`tests/m2/results/m2-wp6.md`, "`bm2-latency`" sections; `tests/m3/results/m3-wp3.md`).

#### BM-2 latency: repack the shards, wait for the warm catch-up, and keep the gate

**Decision.** The lead's, under the operator's delegation. Build a geometric repack of a git cache shard after catch-ups, and have the first merge of a new leadership wait for that leadership's in-flight catch-up of its site's shard; both are being built on branch `bm2-repack`. BM-2's gate text stays until that rerun, and the proposed rewording to a leader on the group's home site with the client in that region is F53. Identity placement does not change: `Names`, `Tokens`, and `Authz` keep leading where they lead. Two ideas stay out because invariants forbid them: reading `deps` by read-index from a same-site learner, and taking the edge's authentication result in place of the leader's `Tokens` read.

**Why.** The tail has two causes no design rule requires. Shards never repack, and grew from about 1,200 to about 1,950 packs over the 288-sample run, with the shard merge p50 rising from 492 to 672 ms; `git merge-tree` on 751 packs took 8.7 ms against 2.0 ms after `git repack --geometric=2`. And the new leadership's catch-up raced the first merge on the slowest shard, so 38 of 105 weur samples caught up on the merge path at 250 to 700 ms each. Moving identity leaders toward a moved repository leader would save 100 to 250 ms a step, which still leaves weur and apac p50 at about 1.7 to 2.0 s, so it does not reach a 2 s p99. A learner's read-index is not `fresh` ("Only the leader builds one", "The read path"), `decide` must see "the `deps` folds the leader read once through `fresh`", and it would still pay a round trip to the enam leader. The leader reads `Tokens`, `Names`, and `Authz` at once, so skipping `Tokens` saves nothing and weakens `decide`.

**Beat.** Rewording the gate now, before the two causes are removed. Placing identity leaders per repository leader. A same-site learner for `deps`. The edge's auth result passed to the leader.

**Evidence.** `tests/m2/results/m2-wp6.md`, "`bm2-latency`": p99 4.10 s, p50 2.11 s at n = 288 (enam 2.40 s, weur 4.15 s, apac 2.89 s), 178 of 288 over 2 s; on fresh shards, n = 109, enam p99 1.68 s, weur 3.18 s, apac 2.82 s; the four cuts took p50 from 2.86 to 1.80 s over 30 samples each [measured]. Commits `1c84ac6`, `c0a393b`, `20155a1`.

**Flip.** `bm2-repack`'s rerun showing enam-led merges over 2 s at p99 on repacked shards with no catch-up on the merge path, which would put the limit elsewhere; or F53 decided, which changes what the gate measures. F53 was decided on 2026-11-06, and the rerun put enam at p99 2.01 s at n = 66, too few samples to call; the home-site run settles it.

#### M3-WP3's identity shapes as the code needs them

**Decision.** From M3-WP3's pure code, in SHARED and DESIGN:
- `IdentityEvent::Session(SessionEvent)`, tag 5, lists web sessions, with `Record` keeping tag 4. Sign-out everywhere stays the `Account` row's `sessions_not_before`.
- `CredentialKey::Session { issued }` carries the session's issue time, encoded as 3 so `Jti` and `SshKey` keep 1 and 2. Every write refuses a session issued before `sessions_not_before`.
- `AuthzEvent::Enterprise { enterprise }`, tag 13, is the enterprise change's commit point, written only by `graft-ops`; 11 and 12 stay for `Deleted` and `Undeleted`.
- `ReadAction` is `Metadata`, `Contents`, or `Issues`, each needing `Read`. `WriteAction` is `Participate`, `Triage`, `Push`, `Maintain`, or `Administer`, needing GitHub's matching role [inference].
- `OrgPolicy` holds what `authorize` reads: `base_permission` and `sso_enforced`. Rulesets, residency, custom roles, webhooks, and secrets join it in later packages.
- `CredentialEvent` is `Added`, `Removed`, `Password`, or `SecondFactor`, holding hashes and verifiers only; `SsoEvent` is `Linked` or `Unlinked` [inference].
- Of SHARED's `Credential`, only `Stored` and `Session` are built. `Stateless` comes with M3-WP4, `Ssh` with M6-WP2, and `RunnerJwt` and `RuntimeToken` with Milestone 4.
- The transfer and the enterprise change need their own repository `Settings` command, sent only by their finisher, because the repository chain's `Settings` refuses an owner or enterprise change (`OwnerChange`). It is not built, and the hosted half needs it.
- The `SiteAdmin` bypass of org and repository permissions is not built; a site admin is so far an admin of the install account's org, checked only by `SetInstanceAuth`. It stays an open question (F54) rather than accepted, because SHARED states it as behavior and BM-3's admin journeys may need it.
- Formats: Authz 0x12.2, Identity 0x14.1 (a new family), and repository 32.6, beside Names 0x10.2 and Tokens 0x11.2. Each needs a site day onto a release that reads it, then `FormatsEnable`, before the edge sends it.

**Why.** SHARED named `CredentialEvent`, `SsoEvent`, `WriteAction`, `ReadAction`, and `OrgPolicy` without their shapes, and put sessions on the `Identity` chain with no event for them. The enterprise change's commit point is an `Authz` entry SHARED never defined. Without the issue time in the credential, the leader could not check `sessions_not_before` from the `Account` row alone. The missing `Settings` command is a real gap: the finisher's plan has a step no command can carry out.

**Beat.** Letting the ordinary `Settings` command change the owner, which would let any repository admin move a repository's `deps` to another owner. Accepting the missing `SiteAdmin` bypass without a question, which would leave SHARED's statement silently unbuilt.

**Evidence.** `tests/m3/results/m3-wp3.md`: BF-3, BF-30, BF-33, BF-34, BF-35, and BF-36 pass in memory on three real members per chain, and 13 of 13 planted bugs are caught [measured]; `crates/identity/src/access.rs`, `org.rs`, `identity.rs`, `credential.rs`, `authz.rs`; `crates/git/src/issue_ops.rs` (`OwnerChange`).

**Flip.** A GHES role boundary measured differently from `WriteAction`'s mapping; or a hosted transfer that can land `Settings` through an existing command without opening owner changes to admins.

### BM-2 reworded by the operator, `bm2-repack`, and M3-WP4's pure half, 2026-11-06

`bm2-repack` removed the shard causes of BM-2's tail, the operator reworded its latency clause, and M3-WP4's pure half passed BF-27 and BF-32 in memory (`tests/m2/results/m2-wp6.md`, "`bm2-repack`" sections; `tests/m3/results/m3-wp4.md`; `FRAME.md`, "Operator decision, 2026-11-06, BM-2 latency").

#### BM-2's latency clause measures a home-site leader, with a looser bound off home

**Decision.** The operator's (`FRAME.md`, "Operator decision, 2026-11-06, BM-2 latency"). BM-2's latency clause is now: the merge p99 on a freshly elected leader on the group's home site, with the client in that region, is at most 2 s, and on a freshly elected leader off the home site at most 3 s. Journeys 2 and 3 and the stale-`parent_tip` clause are unchanged. BUILD-PLAN M2-WP6 and DESIGN's "Milestones 1 to 7" row 2 carry the same text. BM-2's status is "reworded, pending the home-site run". F53 is closed.

**Why.**
- Nothing derived the 2 s. It came over from a non-normative arena candidate, and the only other "merge p99 at most 2 s" is B0-29's, measured with an enam leader and no git cache plan or edge identity reads.
- The design assumes the client is near the leader: "Fresh reads on a held lease cost one DO call in the leader's region, plus the client's distance to it", and "Preferred-site transfer keeps leaders near owners", which moves a leader home within one lease after any election. A freshly elected leader off home is a transient the design already repairs.
- After `bm2-repack`, the shard is no longer the limit: its merge takes 56 to 159 ms p50 and 0 of 288 merges catch up on the merge path. What remains off home is cross-region identity reads: auth and resolve about 540 ms p50 on every leader site, from the LAX edge to the `Tokens`, `Names`, and `Authz` leaders on enam, and apac's commit at 716 ms p50, one round from apac plus three dependency reads to enam.

**Beat.**
- Chasing apac with more code: what is left there is round trips to enam, which no serial step removes.
- Moving identity leaders toward a moved repository leader. It would save 100 to 250 ms a step and still leave weur and apac p50 at about 1.7 to 2.0 s, and it changes placement for every owner to serve a transient.
- Keeping one 2 s bound for every leader site.

**Evidence.** `tests/m2/results/m2-wp6.md`, "`bm2-repack`": p99 2.42 s at n = 288 under the old wording, down from 4.10 s; weur 1.96 s (n = 120), enam 2.01 s (n = 66, 1 over 2 s, 1.93 s measured from the edge), apac 2.71 s (n = 102); max 2.86 s; shard merge p50 56 to 159 ms; 0 catch-ups on the merge path. Before it: p99 4.10 s at n = 288, and on fresh shards enam-led p99 1.68 s at n = 36 [measured].

**Flip.** The home-site run measuring a p99 over 2 s with the client in the home region, which would put the limit in the design's own path rather than distance; or measured traffic showing most merges land on leaders off home for longer than one lease, which would make the off-home bound the one users see.

**Flip met? No, 2026-11-07.** On `bm2-home` the home clause measured p99 1.36 s at n = 300 (p50 0.94 s, max 1.66 s, 0 over 2 s), with every sample's leadership moved back to enam on a new term and the client a Durable Object in EWR (`cdn-cgi/trace`) calling an EWR edge; the off-home clause p99 1.83 s at n = 288 from LAX (weur 1.73 s, apac 1.83 s, 0 over 3 s). On the home site the edge's auth and resolve took 331 ms p50 and the commit 330 ms, against 515 ms and 626 ms off home, so the limit off home is distance, as the Why says. No traffic was measured on where merges land. BM-2 passes and Milestone 2 closes (`tests/m2/results/m2-wp6.md`, "`bm2-home`, 2026-11-06" [measured]; `build/BUILD-PLAN.md`, "Milestone 2 status").

#### `bm2-repack`'s shard repack and catch-up wait match the design

**Decision.** The container's `PUT /refs`, the last step of every catch-up and rehydrate, starts `git repack -d -q --geometric=2` in the background once a network holds more than 50 packs [guess], one repack per shard container at a time, under the network's write lock. The leader site's shard counts its running catch-ups per network, and the first merge plan awaits them. Both follow DESIGN as written ("one repack per shard at a time", Failure modes, git caches; merges on the leader site's shard, Shape, `GitCacheShard`), so DESIGN does not change. Test drivers keep tokens off argv and out of logs.

**Why.** Shards had never repacked, and a catch-up raced the first merge on the slowest shard; together they made most of BM-2's tail. A token reached a raw log through a traceback of a git command line that carried it.

**Beat.** Repacking on a timer, which would repack idle shards and leave a busy one long between runs. Letting the first merge catch up for itself, which doubled the work and set weur's tail.

**Evidence.** `tests/m2/results/m2-wp6.md`, "`bm2-repack`": 1,955 packs to 1 on every `wp6p` shard; `TestRefsRepacksPastRepackAt` and `a_merge_waits_for_the_catch_up_in_flight_instead_of_racing_it` fail on the old code; 21 samples waited 4 to 198 ms; journeys 2 and 3, BF-1, BF-21, and BF-23 pass on the final build; a scan of 38,052 raw files found 0 tokens [measured]. Commits `ca15db8`, `e8d47ad`, `4b6631f`, `9301e15`.

**Flip.** A shard whose background repack is measured holding its write lock long enough to show in merge or clone latency, which would need a smaller trigger or a repack outside the lock.

#### M3-WP4's stateless token shapes as the code needs them

**Decision.** From M3-WP4's pure code, in SHARED and BUILD-PLAN:
- A `ghs_` token is `ghs_`, the App id in decimal, `_`, then a compact EdDSA JWS whose payload is `iss`, `ins`, `repos`, `perms`, `jti`, `iat`, and `exp` in whole seconds, as GitHub's `ghs_APPID_JWT`. Its length varies with the claims; SHARED's "about 400 characters" becomes 338 in the BF-32 gate, and nothing assumes a length.
- `AuthzEvent::Installation` carries `row: Option<InstallationRow>` (the App, `RepoScope`, and `PermissionSet`) in place of SHARED's undefined `InstallState`; `None` is uninstalled. A narrowing or uninstall moves `not_before` to the entry's `at`, and a follower refuses it moving back.
- `authorize` for an App takes the lower of the claims' and the installation row's level; `contents` gives `Read` or `Write`, any other permission `Read` [inference].
- The old verifying key retires at the switch plus `SIGNING_KEY_CACHE`, one token lifetime, and `CLOCK_SKEW` (60 s [guess]), not at "the last token's `exp`" alone, because an isolate that cached it may sign for up to `SIGNING_KEY_CACHE` after the switch.
- `DELETE /installation/token` is accepted only from the token itself or `graft-ops`; a second delete is 204. A token request names at most 500 repositories [unverified].
- `CredentialShard::is_operator()` is no account and no key, because an installation token also names no account.
- Push format 0x20.7 carries the App actor and its claims; issue and merge commands have no stateless wire form yet and fail closed: the leader refuses them `Unauthorized(NoToken)`, which hosted rendered 403 on the label route ("M3's hosted half, 2026-11-07").

**Why.** SHARED did not define `InstallState`, the token's exact form, or how `authorize` combines the claims with the row. Retiring at the last `exp` alone would refuse tokens a cached key still signs. Reading "no account" as the operator would have made every installation token the operator; planted bug 2 is that mistake and BF-27 catches it.

**Beat.** A fixed-length token. An `InstallState` enum beside the row. Retiring the old key one lifetime after the switch.

**Evidence.** `tests/m3/results/m3-wp4.md`: BF-27 1,000 of 1,000 commands and 1,000 of 1,000 pushes refused 401; BF-32's pure half with three `universal-github-app-jwt` 2.2.2 JWTs; rotation kept kid 1 valid for 7,260 s after the switch in the test's clock; 14 of 14 planted bugs [measured]; `crates/identity/src/stateless.rs`, `authz.rs`, `access.rs`, `appauth.rs`, `credential.rs`.

**Flip.** A client measured rejecting the `ghs_APPID_` form or a token of this length; or GitHub's documented installation-token behavior differing from the `contents`-only `Write` mapping.

### Milestone 2 closes, and B0-18's call-depth fix, 2026-11-07

BM-2 passed as reworded, which closes Milestone 2, and B0-18's hosted rerun passed 6 of 6 on `graft-m2b` once the leader stopped calling learners that call it (`tests/m2/results/m2-wp6.md`, "`bm2-home`, 2026-11-06"; `tests/m3/results/m3-wp5.md`, "2026-11-06 (evening): B0-18 traced on `graft-m2b`"; `build/BUILD-PLAN.md`, "Milestone 2 status" and M3-WP5).

#### A member never calls a learner that is calling it

**Decision.** Member-to-member calls must not form loops, because the runtime nests calls at most 16 deep and charges each outgoing call to the object's newest incoming request. In the site host: a member never calls a learner that has a call open at it or called it within `QUIET` (1 s [guess]); that learner's messages are held and ride the response to its next call, ahead of the call's own replies, and the 100 ms ticker sends them as calls once the learner has been quiet for `QUIET`. A learner's `ReadIndex` that needs a round waits in its own call, up to `READ_INDEX_HOLD` (500 ms [guess]), instead of being answered by a later call from the leader. A learner call unanswered after `HEDGE` (500 ms [guess]) is sent once more, and Raft ignores the second answer. Voters are never held. `crates/raft` and its messages do not change. DESIGN states the rule in Shape ("Members reach each other through Durable Object bindings", "The read path"), Scale (learners), and Failure modes (chain groups, noisy-tenant).

**Why.** The leader's AppendEntries and late `ReadIndexRes` were calls into each learner, and each learner's next `ReadIndex` was a call back, so each exchange added 2 to the depth. At 16 the leader's own AppendEntries to its voters failed, `STEP_DOWN` tripped on `CommitErrors`, and learner reads failed `notLeader` or waited for the next leader. Milestone 0's prototype passed because its learners pulled entries inside their own read-index call; M0-WP9's change to feed learners from the leader closed the loop. After the fix, the remaining tail was one call delayed 1 to 2 s on the enam-apac path holding a learner's queued reads to `CALL_DEADLINE`, which the hedge removes.

**Beat.** Learners pulling entries again, which changes Raft's messages and gives up the leader's push. Waiting for edge reads to reset a learner's depth, which helps only learners whose reads arrive faster than the leader's calls. Lowering the read rate or the learner count, which leaves the loop and only slows the climb.

**Evidence.** `tests/m3/results/m3-wp5.md`, "2026-11-06 (evening)": 12 of 12 traced runs failed with depth failures on the leader's AppendEntries; modelled hops climbed 3, 5, 7 to 26; shallow pings to every learner made two runs pass; on `06762a2` 8 of 12 runs passed with 0 depth failures and deepest hop 4 to 6; on `e174090` 6 of 6 passed, worst learner p99 560 to 853 ms, leader 243 to 249 req/s, 7 hedged calls; the depth test fails on the old routing [measured]. `workers/site/src/callers.rs`, `host.rs` (`HEDGE`), `member.rs` (`READ_INDEX_HOLD`).

**Flip.** Cloudflare documents a depth rule that does not inherit from a finished request, or raises the limit far above 16; or a hosted run with voters calling each other at a learner's rate showing the same climb, which would need the rule for voters too; or measured hedges above a few percent of learner calls, which would make `HEDGE` a load on the leader.

#### Milestone 2 closes on reduced rechecks

**Decision.** Milestone 2 is closed. Every gate row, BF-1 and BF-20 to BF-26 and BM-2, is Done in BUILD-PLAN's "Milestone 2 status", each with the run that closed it. Rows that passed on an earlier build were rechecked on M2-WP6's final build at reduced counts or as smokes, not rerun in full, and the table says which. BF-22's prune clause and BF-25's unreadable-bucket clause ran only in their pure tests, because the install's entry clock cannot be moved and a bucket cannot be made unreadable.

**Why.** BUILD-PLAN closes a package on its gate rows, and each row met its Pass value on the hosted install or, for those two clauses, in the only form the install supports. The later builds touched the merge path, the shard, and the credential types, and the rechecks covered each change's rows.

**Beat.** Holding Milestone 2 open for full reruns of every row on the last build, which would repeat days of runs for changes the rechecks already covered. Holding it until the install has a bucket fault hook and a movable entry clock, which no Milestone 2 package builds.

**Evidence.** `build/BUILD-PLAN.md`, "Milestone 2 status"; `tests/m2/results/` [measured].

**Flip.** A later build that changes a Milestone 2 row's code path and fails that row's recheck, which reopens it; or a hosted bucket fault or clock hook, after which BF-25's and BF-22's hosted clauses run in full.

### M3's hosted half, 2026-11-07

M3-WP1 to M3-WP4 ran hosted on `graft-m1-*` with real gh, git, and `@octokit/app`; BF-32 passed and journey 4 refused every revoked token, with one `ghs_` revoke lost before the edge (`tests/m3/results/m3-hosted.md`; `build/BUILD-PLAN.md`, "M3-WP1 to M3-WP4 status").

#### `MintWrapKey`'s layout, signup's placement copy, and a stateless refusal's status

**Decision.**
- `MintWrapKey` is a JSON object `{wrap, kid, nonce, wrapped}` with hex fields: the 32-byte wrap key, the signing key's `kid`, a 12-byte nonce, and the 32-byte Ed25519 seed sealed under the wrap key with AES-256-GCM. `graft-ops` reads it, and `graftyard mint-key rotate` must write it (DESIGN, "Install operations", the rotation table) [inference: `workers/ops/src/lib.rs`].
- Signup's first follow-up copies the install account's placement record to the new owner as `NamesOp::InitPlacement` on `account_shard(owner)`, before any of the owner's chains is created (DESIGN, Shape, `PlacementRecord`; SHARED, "Multi-chain operations", Signup). `edge::after_signup` does not yet; the fix is on `m3-close`.
- Until issue and merge commands carry a stateless principal, one sent under a `ghs_` token names no account and no key, the leader refuses it `Unauthorized(NoToken)`, and the edge renders that refusal by route: 403 "Resource not accessible by integration" on the label route, as BF-32 measured, and 401 "Bad credentials" on the other issue and merge routes (`crates/api/src/rest.rs`, `label_refused`, `refused_merge`; `graphql.rs`). Both fail closed. BUILD-PLAN M3-WP4's "fail closed with 401" now says so.

**Why.** DESIGN named the secret's contents but no layout, and the rotation command and the minting Worker must agree on one. DESIGN said a new owner "copies the install account's record" without saying who writes it or when; an owner chain created before its record exists has no placement, which `setup.ts` covered by hand. The refusal's status differs by route because each route's renderer maps `Unauthorized` on its own.

**Beat.** A binary layout for `MintWrapKey`, which Wrangler's secret text cannot hold without an encoding anyway. Copying the record lazily on an owner chain's first touch, which puts a `Names` write on a request path that may run under the owner's own token. Changing every renderer to 403 now, ahead of the stateless form that removes the refusal.

**Evidence.** `tests/m3/results/m3-hosted.md`, "What was built" and "Design gaps found"; BF-32's issue-write case 403 [measured]; `workers/ops/src/lib.rs`, `crates/identity/src/edge.rs`, `crates/identity/src/names.rs` (`InitPlacement`).

**Flip.** A Secrets Store limit or a rotation tool that cannot write the JSON form; a placement design in which an owner's record is derived rather than stored; or the stateless wire form landing, after which a `ghs_` issue or merge write is decided by `authorize` and refused, if at all, 403 for a missing permission.

**Flip met, 2026-11-07, for the third bullet.** The stateless form landed in format 32.9 (`65a7b07`, `1815c70`): an App's issue write is authorized by `authorize_app` and refused 403 for a missing permission, 10 of 10 hosted, and an App's merge answers 403 at the edge. The signup bullet's code landed too (`1fce620`) [measured, `tests/m3/results/m3-close.md`].

### BM-3 passes, F51 and R20 closed, and M3's close round, 2026-11-07

BM-3 passed on the `m3-close` build, F51 was settled in code, and the lead decided R20; Milestone 3 stays open for the rows in BUILD-PLAN's "Milestone 3 status" (`tests/m3/results/m3-close.md`; `build/BUILD-PLAN.md`, M3-WP4 "M3 close round" and M3-WP6).

#### F51 closed: a status's id follows its commit's order

**Decision.** The repository group's `decide` gives a commit status the id max(the edge's minted id, the highest `StatusId` on its commit + 1), and the reply renders the id `decide` returned (`IssueOut::Id`). Ids on one commit therefore rise in commit order whatever the edges' clocks say, so for one (sha, context) the later report wins; the edge's clock orders only a commit's first status. SHARED's `RequiredCheck` text says so. F51 leaves OPEN-QUESTIONS.

**Why.** A status's version is its id, and the higher version wins. With ids from the edge's clock, a re-run reported `pending` through an edge whose clock trails got a lower id than the earlier `success`, so the check stayed green and BF-23's "a re-run blocks a merge again" failed through two edges. Taking the id from the commit's own rows is the "commit order" answer F51 named, and needs no id lease.

**Beat.** A `DbId` from the group's `IdBlock`, which orders every status by commit order too but spends ids from the lease and changes the id's meaning for the REST reply. Keeping the edge's id and fixing the collision only (`31eeadf`), which left the skew.

**Evidence.** `212aa36`; `a_later_status_from_an_edge_whose_clock_trails_still_wins` fails on the old rule, with two edges a minute apart, and the planted `status-id-from-edge-clock` bug is caught [measured in memory, `tests/m3/results/m3-close.md`, F51 and "Planted bugs"]. BF-23 passed 100 of 100 hosted through one edge (`tests/m2/results/m2-wp3.md`). The hosted run through two edges with skewed clocks was not made, because an edge cannot fake its clock.

**Flip.** A hosted status whose id falls below one already on its commit; or a client that relies on status ids being increasing timestamps across commits, which this rule no longer gives.

#### Install creates every `Tokens` shard

**Decision.** The lead's. Install creates the group of every `Tokens` shard, all 3,072 (three home families of 1,024), run in parallel; `Names` shards stay created on first use. DESIGN, "Install operations", Genesis step 5, and the chain-groups control-plane row say so. The code comes on `m3-finish`. R20 leaves OPEN-QUESTIONS.

**Why.** A sign-in code lands on a `Tokens` shard by its hash, so almost every device code met a shard with no group, and a person signing in waited for its creation: a device-code request took 13.0 s median wall time against 62 ms of CPU over 65 requests, and a new group 13 to 14 s to create. The extra groups cost little while idle: B0-15 measured 100,000 idle groups at 0 requests and 0 GB-s over 24 h (K6), and `scripts/cost.py` already counts the 3,072 token groups in its group total (`TOKEN_GROUPS`), so no Cost figure moves. `Names` shards are written by signups, renames, and App registrations, which are rarer and not a person waiting at a prompt [inference].

**Beat.** Routing codes to fewer shards, which concentrates sign-in on a few groups and changes `token_shard`'s layout. Creating shards lazily from a cron, which still leaves the first sign-ins slow. Creating every `Names` shard too, which adds 3,072 more creations at install for writes that do not wait on a person.

**Evidence.** `tests/m3/results/m3-hosted.md`, "Latency"; `tests/m3/results/m3-close.md`, "Groups created on first use" [measured]; `scripts/cost.py`, `TOKEN_GROUPS` and `w["groups"]`; K6 in `decisions/OPEN-QUESTIONS.md`.

**Flip.** Idle groups measured billing duration or storage that the cost scripts do not carry; install time with 3,072 parallel creations measured over an hour; or `Names` writes measured on a person's critical path at a rate that makes first-use creation visible.

#### `Audit`'s cursor, the bootstrap link, and BF-33 hosted

**Decision.**
- A consumer that reads one chain and writes others commits its cursor on the chain it reads, after the work, so a batch lost before its cursor runs again and re-sends only idempotent steps. `Audit` reads `Tokens` and `Names`, writes `Identity`, `Authz`, and `Names`, and keeps `AuditCursor` (formats 0x10.3 and 0x11.3) on each shard it reads, moved last or once `CURSOR_STRIDE` (1,000) entries have passed. SHARED, "Event fan-out", says so beside the [inference] for consumers that commit to the chain they read.
- `graftyard admin bootstrap` writes the sign-in link's `PutCode { kind: SignInLink }` as the operator's request itself, because `graft-ops` has no operator route (R14); the row is the one `graft-ops` would write. `POST /login/oauth/link` spends it once and answers 201 with the session id and no cookie. What the link opens in a browser is a recorded gap, owned with the other sign-in pages by F55.
- BF-33's hosted run only ever restarted: 120 of 120 kills on the exchange's `UseCode` ended in a clean restart, and its "completes" branch stands on gh's 20 of 20 hosted logins and the in-memory 100 of 100 until a kill that loses the command before commit runs hosted (BUILD-PLAN, "Milestone 3 status").

**Why.** SHARED's cursor rule assumed the consumer writes the chain it reads; `Audit` cannot commit its cursor with work on other chains. DESIGN said `graft-ops` mints the link, and no operator route to `graft-ops` exists. In every hosted kill mode the edge's re-send under the same request id found the command committed, so the run could not reach the branch where the code is unspent.

**Beat.** A cursor on each chain `Audit` writes, which needs a write to every target per batch. A `graft-ops` operator route built only for the link. Counting BF-33's "completes" branch as passed hosted.

**Evidence.** `tests/m3/results/m3-close.md`, "Audit, hosted", "`graftyard admin bootstrap`" (11 of 11), "BF-33" (120 of 120), and "Design gaps and contradictions found"; the 8 planted bugs, six on the cursor [measured].

**Flip.** A `Finisher` step that is not idempotent, which would need the cursor committed with the work; a `graft-ops` operator route, which takes the link back; or a hosted kill that loses the `UseCode` and still yields one token, which closes BF-33's branch.

### AM1-WP2's pure half, R20 and rotation hosted, and BF-30's transfer, 2026-11-07

AM1-WP2's pure half passed LF-2, B0-36, and B0-37 in memory as far as `decide` can see before tasks and leases; R20 and the hosted key rotation ran on `graft-m1`; and the lead decided how BF-30's transfer is built (`tests/am1/results/am1-wp2.md`; `tests/m3/results/m3-finish.md`; `build/BUILD-PLAN.md`, AM1-WP2 and "Milestone 3 status").

#### AM1-WP2's agent rows as the code needs them

**Decision.** In the agent block of both DESIGN and AGENT-LAYER, and AGENT-LAYER's trust view:
- `AuthzEvent::AgentApp { app, row: Option<AgentApp> }` (tag 14), where `row: None` unflags. Only the install org's admin writes it, only on `Authz(Genesis.account)`, through `PutAgentApp` and `DeleteAgentApp`.
- `agents_enabled` and `agents_workspace` are set by their own `AuthzEvent::OrgAgents { enabled, workspace }` (tag 15, `SetOrgAgents`) and read through `OrgPolicy`, never written in a `Policy` entry, so M3-WP3's `Policy` bytes hold. `code_mode` is AM2's.
- `AuthorTrust.is_agent` comes from `AgentMeta` and `AgentApp` rows; the "false until AM1-WP2" note is gone.
- `Push.changed` is carried and stored; matching it against live Exclusive leases waits for AM1-WP4's leases.
- Formats: repository 32.8 for an agent-validated push, `SetAgentPolicy`, or an issue command with agent fields; Authz 0x12.3. A push with no agent fields keeps its earlier format and bytes.

**Why.** `AgentApp(AgentApp)` had no removal form, and LF-2 and `DELETE /agents/apps/{id}` need one; an `Option` row keeps one event for both. Adding fields to `OrgPolicy`'s `Policy` record would change every M3 `Policy` entry's bytes, which the byte-identity gate forbids. The trust view now has the rows it was waiting for.

**Beat.** A separate `AgentAppRemoved` event, which adds a tag for what `None` says. New `OrgPolicy` fields behind a format bump of `Policy`, which re-encodes every org's policy.

**Evidence.** `tests/am1/results/am1-wp2.md`: LF-2 25 of 25; B0-36 100 of 100 per action across 11 actions for both App kinds, controls never refused; B0-37 10,000 hostile pushes with 0 parse failures, 0 of 17,264 forged trailers linked, 0 of 1,675 forged `Session` sources stored, 1,000 of 1,000 `refs/graftyard/` updates refused; `agent_bytes.golden` and 120 of 120 replies identical to main; 19 of 19 planted bugs [measured, in memory].

**Flip.** A hosted rerun where an unflag through `row: None` leaves a leader classing the App `Agent` after the acknowledgment; or an AM2 change to `OrgPolicy` that must re-encode `Policy` anyway, which would fold `OrgAgents` back in.

#### A transfer's own `Settings` command, and the move between placements pending

**Decision.** The lead's, for BF-30, being built on `m3-finish` on `graft-m1`:
- The transfer's `Settings` is its own repository command in repository format 32.10, with no account and no key, sent only by the transfer's finisher; a site day reads it before anything sends it. The repository chain's own `Settings` keeps refusing an owner or enterprise change (`OwnerChange`).
- `Audit` gains a cursor on the destination owner's `Authz` chain, in Authz format 0x12.4, so it finishes a transfer whose finisher died. Until it is built, SHARED's "the `Audit` consumer finishes any other chain operation" does not cover transfers, and SHARED says so.
- **Pending the builder's finding, not settled:** DESIGN moves an owner's groups "through `Joint`", but `Member::reconfigure` refuses a config that adds or drops more than one voter site (`NotOneChange`, `crates/raft/src/member/leader.rs`). The builder checks whether a real `Joint` of two disjoint placements works. If it does not, the move Workflow goes one voter site per config change and never drops the leader's site without a leadership transfer first. DESIGN and SHARED mark the move pending.
- `graft-m1` may gain a fourth site script, `graft-m1-site-d`, so a move across placements can run on the test install.

**Why.** Every repository format up to 32.9 is taken, and the finisher's commands carry no account or key, as every finisher step does. A transfer's commit point, the `Reserved` name, is on an `Authz` chain the hosted `Audit` does not scan. Three sites leave no room for a move to other sites, so BF-30's "across placements" could only change home or the placement's name.

**Beat.** Lifting `OwnerChange` from the ordinary `Settings`, which would let an admin change a repository's owner without a transfer. Raising an `Owed` item from a `Names` entry the transfer writes, which ties the finisher to a row the transfer may not have written yet. Testing the move only by re-homing on three sites, which M1-WP7 already covers.

**Evidence.** `tests/m3/results/m3-finish.md`, "Not done yet in this round" [inference, from the code named]; `crates/git/src/issue_ops.rs` (`OwnerChange`), `crates/raft/src/member/leader.rs` (`take_config`, `NotOneChange`), `crates/identity/src/audit.rs` ("A transfer is not finished here").

**Flip.** The builder finding that a disjoint `Joint` commits and keeps both majorities' leases, which settles the move as DESIGN states it; or one-site-per-change moves measured losing availability when a step drops a site, which would need `Joint` built first. For the command and the cursor: an existing command that can land the owner change without opening it to admins.

**Settled (2026-11-08).** The builder found that `reconfigure` accepts a one-site `Joint` and refuses a disjoint one, so the move goes one voter site per change; 32.10 (`RepoCmd::Transfer`) and 0x12.4 (`AuditCursor`) are built, and BF-30 passed hosted. Flip met? No: no disjoint `Joint` was built, and 0 of 2,313 route samples were lost across one-site steps ("BF-30's transfer as built", below).

#### R20 built, and kid 1 retired, on `graft-m1`

**Decision.** "Install creates every `Tokens` shard" stands as built: the edge's `POST /api/graftyard/v1/release/tokens?from&count`, accepted only under the genesis the `Release` group holds, creates up to 16 shards a request, and `graftyard install` drives 192 ranges, 8 at once, and reports failed ranges for an idempotent rerun. Hosted rotation is done: kid 2 signs and kid 1 is retired. A genuine kid-1-signed token refused `Retired` was not observed, and that gap is recorded rather than closed.

**Why.** One invocation cannot create 3,072 groups under the connection and subrequest limits, so the CLI drives ranges. The 13 s was each new member's join, `RTT_SAMPLES` sequential pings to each peer, not an election, so pre-creating removes it. No kid 1 token or key remained after the switch.

**Beat.** Creating shards from one edge invocation. Keeping the kid 1 signing key to mint a control token, which keeps a retired key alive.

**Evidence.** `tests/m3/results/m3-finish.md`: 3,072 shards in 21 minutes with one registry timeout retried; device-code request 13.0 s to 0.83 s median, max 1.3 s, token exchange about 15 s to 1.9 s, 24 of 24 tokens; joins 9.9 s and election 1.2 s on fresh shards; kid 2's secret at 19:35:36Z and 10 of 10 mints under it; kid 1 retired at 22:34:26Z (`tokens:0` index 35), the kid-1 probe `Retired` offline and 5 of 5 pushes refused, kid 2 5 of 5 accepted [measured]. Commits `5938d1b`, `eaff64b`, `74550b6`, `dc81e96`.

**Flip.** A pre-created shard measured slow on first sign-in again, which would put the cost elsewhere than the join; or a genuine kid-1-signed token accepted after its retire, which would mean `verify` checks the signature before `retired`.

### BF-30, M3's last rows, and AM1-WP3's pure half, 2026-11-08

BF-30 passed hosted on `graft-m1`, M3's assigned rows passed hosted on `graft-m2b`, and AM1-WP3's external tasks passed in memory (`tests/m3/results/m3-finish.md`; `tests/m3/results/m3-rows.md`; `tests/am1/results/am1-wp3.md`; `build/BUILD-PLAN.md`, "Milestone 3 status" and AM1-WP3).

#### BF-30's transfer as built: one voter site per change, finished in band

**Decision.**
- An owner's groups move between placements one voter site per config change: each swap is a one-site `Joint` (the new member joins at the next generation, then `Joint { old, new }`) followed by its `Normal`, leadership leaves a site before that site is dropped, and `Home` with a leadership transfer comes last. `crates/raft` is unchanged. DESIGN, "Sites and placement" and Deployment modes, and SHARED, "Multi-chain operations", say so.
- A transfer finishes in band, not in a Cloudflare Workflow. The edge commits `ReserveTransfer` under the caller's credential, starts `/finish` on the destination's `Authz` leader, and answers 202; that leader runs the move, `Settings`, `Live`, `Alias`, and `RepoOwner`, one `/move` step at a time with a `NotLeader` refusal retried at the new leader. The `Audit` alarm on the same `Authz` chain, from its `AuditCursor`, finishes any transfer left after `FINISH_AFTER` (60 s), and any org's enterprise change. DESIGN's Stack no longer lists repository transfers under Workflows.
- While `RepoOwner` still names the old owner, which by then holds only an `Alias`, `/repositories/{id}` resolves through that `Alias` as well as a `Live` name (`AuthzFold::id_name`).

**Why.** A disjoint `Joint` cannot be expressed under `reconfigure`'s `NotOneChange`, and a one-site `Joint` keeps a majority of both configs at every step. The transfer's steps are short, idempotent, and keyed by the reservation, so the destination's leader can run them where the commit point lives, and `Audit` already had to own the dead-finisher case; a Workflow would add a script and a site day for steps a member already runs. The id route looked for a `Live` name only, so it lost the repository between the last two steps.

**Beat.** Building a disjoint `Joint` in `crates/raft`, which needs a new config shape, a sweep rerun, and Porcupine. A Workflow per transfer, as DESIGN's Stack had it. Reordering `RepoOwner` before the `Alias`, which leaves a window where `RepoOwner` names an owner whose name is not yet `Live` on every read.

**Evidence.** `tests/m3/results/m3-finish.md`, BF-30: `a_joint_of_two_placements_differing_by_two_sites_is_refused`; hosted configs `0,1,2 h0` → `0,1,2,3 h0` → `0,1,3 h0` → `0,1,3 h3`; 20 of 20 transfers, 0 of 2,313 route samples lost, `deps` naming the new owner in 20 of 20; a finisher killed after `Reserved` finished by `Audit` alone in 76.2 s with 0 of 586 samples lost; the enterprise change on 2 of 2 repositories; 9 of 9 planted bugs; run a before `b18894e`, 35 of 2,715 id-route samples 404 and a `/move` step that stopped on `"NotLeader(..)"` [measured].

**Flip.** A one-site step measured losing a route or a lease, which would need a disjoint `Joint` built; a transfer whose steps outgrow one alarm's budget, such as a network with many child groups, which would move it to a Workflow; or a second route that resolves a repository by its owner's name during the window, which would need the same `Alias` fallback.

#### BF-34's compatibility choices, and M3's last row shapes

**Decision.**
- `IdentityEvent::KeyText { fingerprint, key }` (tag 6, Identity 0x14.2, op `AddKey` tag 7) holds a listed SSH key's public text; the leader refuses text whose blob does not hash to the fingerprint and a key whose `Tokens` row is not live. `IssueOp::CancelInvitation` is repository 32.11. An issue command decides at the leader's `now`, or the entry's `at` on a follower.
- Where GHES 3.22 is silent or differs from GitHub.com: `PUT .../collaborators/{u}` answers 204 for a new invitation; an org invitation is declined by `DELETE /user/memberships/orgs/{org}` [inference]; a 404 from `GET .../collaborators/{u}` carries GitHub's JSON body; a team's `node_id` uses the Organization node type; an SSH key's `created_at` is the epoch and its `id` the first 6 bytes of its fingerprint. SHARED, "Who owns which fact", lists them.
- `/user/repository_invitations` lists only the repository named by `?repo=o/r` until `Notify`'s `Inbox` (M4-WP2), and the OIDC client secret is plaintext until F57 names its wrap key.

**Why.** `GET /user/keys` renders a key's text, which the `Tokens` row does not hold. The GHES 3.22 description is the oracle, and it lists 204, not 201, for the collaborator PUT; where it names no route or body, the edge takes GitHub.com's. Nothing stores a key's creation time or id, or has a Team node type, and adding them would change M3's bytes for no client that checks them. Invitations were born expired because `decide` ran at time 0 (`db1c6be`).

**Beat.** Storing key text in the `Tokens` row, which every token check reads. Answering GitHub.com's 201 with a `repository-invitation`, which the GHES description fails. A new Team node type and stored key times now.

**Evidence.** `tests/m3/results/m3-rows.md`: BF-34 10 of 10 driver runs and 170 of 170 replies valid against GHES 3.22; BF-35 100 of 100 with no `Audit`; BF-2 9 of 9, BF-31 10 of 10, BF-33 99 of 100 completed with one token, BF-36 10 of 10; 16 of 16 planted bugs; "Design notes and contradictions" [measured].

**Flip.** A client or the Terraform provider that needs the 201 body, a real team node type, or a key's real `created_at`; a GHES route found that declines an org invitation; or the `Inbox` built, which drops `?repo`.

#### AM1-WP3's task rows as the code needs them

**Decision.** In the agent block of both DESIGN and AGENT-LAYER:
- A task's `prefix` is matched as `refs/heads/graftyard/<uuid>/*/**`, because `**` also matches zero segments, so the bare id is refused.
- Every `TaskUpdated` writes Core, an empty `PATCH` too; Metadata too when it sets `pr` or `summary`.
- Core holds `session`, `kind`, `state`, `prefix`, `requester`, `app`, `installation`, and the create and last-update times; Metadata holds the rest.
- A task reads Cancelled once the owner `Authz` read finds no installation row for its App that reaches the repository with `not_before` at or before the task's creation (the token rule), and timed out after `TASK_IDLE` with no update; its next command records the state and answers 409 with it, also from `cancel` (AGENT-LAYER, "API reference").
- `issue` and `pr` are `Option<IssueNumber>` in the task's own repository. `SessionId`'s string form is what JSON carries.
- Format: repository 32.12, `RepoEvent` 31 `TaskCreated` and 32 `TaskUpdated`.

**Why.** `decide` derives `cancelled` and `timed_out` from the core tier alone, so the fields and the last-update time it reads must be Core. "Active installation" was undefined, and a row-exists check let a reinstall revive old tasks, which an old token never could. `IssueRef` and `PrNumber` did not exist, and an external task only names its own repository. `serde_json` has no 128-bit integers.

**Beat.** Keeping the design's `**` pattern, which accepts the bare id. Reading the metadata tier on every task `decide`. Defining "active" by row existence.

**Evidence.** `tests/am1/results/am1-wp3.md`: LF-1 100 of 100 read cancelled with 0 events written by reads and 100 of 100 answered 409; linking 100 of 100; caps at 100 per requester and 2,000 per repository; 24 h idle reads `timed_out`; 120 of 120 replies identical to main; 24 of 24 planted bugs; bugs 1 and 2; "Design sentences these findings contradict or leave open" [measured, in memory].

**Flip.** A hosted LF-1 rerun where a reinstall or a token from before it reaches an old task; a Core tier measured over its budget from the extra fields; or a task reference to another repository's issue, which would need `IssueRef`.

### AM1-WP4's pure half: leases, `PathSet`, and the feed as the code needs them, 2026-11-08

AM1-WP4's in-memory half passed LF-3, LF-5, and L0-13, and the pure parts of L0-12, L0-16, LF-12, and LF-13, on real members (`tests/am1/results/am1-wp4.md`; `build/BUILD-PLAN.md`, AM1-WP4). It changed `crates/raft`; the 2,000-seed sweep passed in 348 s with 0 step-downs flagged, and Porcupine was Ok on both histories (156,742 and 2,226,521 operations).

#### Lease, `PathSet`, and feed shapes

**Decision.** In the agent block of both DESIGN and AGENT-LAYER, and in AGENT-LAYER's own Shape and prose:
- `LeaseAcquire { lease, plan: Option<IssueNumber> }`, where `plan` names the draft whose `graftyard-plan` block declared the lease; the API still renders `plan_block: bool`, true when `plan` is set.
- A plan-block lease ends, with no command, once its draft is readied, closed, or merged, because `decide` reads that PR's state through `plan`.
- `PathSet.merge_base` is `Option<Oid>`, `None` until the git cache reports one.
- `FeedCursor.group` is a `RepoId`, the 8 bytes its 17-byte text names.
- `ActivityEvent::Verdict` waits for AM4, whose `InsightVerdict` row is its only source.
- The lease refusal names the holder as `app N` (or `user N`) and the expiry in epoch milliseconds, because a push does not read `Names`, where a slug lives.
- The edge checks a feed query and answers 503 until it reads followers and learners, which serve the feed.
- Format: repository 32.14, `RepoEvent` 35 `LeaseAcquire`, 36 `LeaseRenew`, 37 `LeaseRelease`, and 38 `PathSet`.

**Why.** The editor's plan lease on a PR must be found to be replaced, and its liveness depends on the draft being open; a bool holds neither. `Push` carries no merge base and `validate` computes none. A `ChainId` with its kind does not fit the cursor's 8 bytes. Nothing writes `InsightVerdict` before AM4. The slug is on another chain, and reading it would add a dependency to every agent push. Today's edge reads only leaders, which never serve the feed.

**Beat.** A `plan_block: bool` with a separate PR index. Computing the merge base in `decide`. A wider cursor, which changes the API's opaque form. Serving the feed from leaders until learners exist, which the feed's load rule forbids.

**Evidence.** `tests/am1/results/am1-wp4.md`, "Gates", "Assumptions", and contradictions 1 to 3 and 6 to 9: LF-5 one lease from the valid block and 5 of 5 bodies unchanged, the plan lease ended by readying; LF-3 410, `wait` 25, 422 for 33 paths; L0-13 100 of 100 per bad acquire; 120 of 120 replies identical and `agent_bytes.golden` untouched; 40 of 40 planted bugs, counted from named failing tests after two plant-runner bugs were fixed. The first tags, 33 to 36 in `844ecbe`, collided with `m3-repos`' 33 `Created` and 34 `Deleted` and moved to 35 to 38 in `0477a8b`, before the merge [measured].

**Flip.** A client that needs the plan lease to outlive readying; AM4's `InsightVerdict`, which adds `Verdict`; a hosted follower and learner read path, which ends the 503; or a slug read already on the push path, which would let the text name the App.

#### A stall marker rides the next state write, and `decide` sees `ExtendLeases`

**Decision.**
- A member marks `stalled_since` in memory at a follower's first `suspect` report, a campaign start (not a transfer's), or the leader's first entry unacknowledged past CALL_DEADLINE, and stores it with its next state write. A campaign start writes at once. A crash before that write loses the marker, and the leases that stall covered lapse by its length, which leases accept as advisory. DESIGN, Shape, "Chains", says so.
- The leader applies an entry's `extend` to its scratch view before that entry's commands decide (`Scratch::extend`), so the first command after a stall sees the moved leases; a follower already applies `extend` when it applies the entry.

**Why.** A separate durable write at every `suspect` report changed the seeded harsh simulator's crash points, and acknowledged writes fell to 1,138 against its floor of 1,200; a separate `Noop` entry carrying `extend` broke two scenario tests. Without the pre-decide apply, a renew right after a 30-minute stall was refused `NotFound`, and a lease the stall covered read expired (AM1-WP4 bug 1).

**Beat.** Persisting each marker at once, which costs a write per report on the commit path and broke the simulator's floor. A `Noop` entry to carry `extend`. Deciding on the fold at `applied` and letting the extension land after.

**Evidence.** `tests/am1/results/am1-wp4.md`, contradictions 4 and 5, "Bugs the gates found" 1, and L0-12's stall: one later entry carried `ExtendLeases { by: 1,801,001 ms }`, 3 of 3 leases live after it with `expires` moved by exactly `by`, `stalled_since` cleared on all three members; the raft suite's 9 suites passed; the long sweep passed (2,000 seeds, 348 s, 0 step-downs flagged), and Porcupine was Ok on 156,742 and 2,226,521 operations [measured, in memory]. The hosted 30-minute stall with member calls blackholed has not run.

**Flip.** A hosted run, or a customer report, where a crash during a stall lapses an Exclusive lease and a second holder's push lands inside it; leases made more than advisory (an enforced mode that a lapse would break); or a durable marker write that costs nothing measurable on the commit path, which would make losing the marker needless.

### Milestone 3 closes, the read gate's 401, and AM1's hosted half, 2026-11-09

`m3-repos` built repository creation over REST and BF-34's Terraform test passed as written, which closed Milestone 3; the edge read gate ran hosted; and `am1-hosted` ran AM1's hosted half on `graft-m1`, where LM-1 does not yet pass (`tests/m3/results/m3-repos.md`; `tests/am1/results/am1-hosted.md`; `build/BUILD-PLAN.md`, "Milestone 3 status" and AM1-WP6).

#### Repository create and delete in format 32.13

**Decision.** `POST /orgs/{org}/repos`, `POST /user/repos`, and GraphQL `createRepository` commit `CreateRepo` on the owner's `Authz` chain first (the commit point), then `RepoOwner`, the `auto_init` README push under its own request id, and `InitRepo`, whose leader renders the 201. `DELETE` appends the chain's `Deleted`, then turns the name `Deleted`. Repository format 32.13 holds `InitRepo` and `DeleteRepo`, with `RepoEvent::Created { actor: Option<AccountId> }` (tag 33), whose entry time is `created_at`, and `Deleted { retain_until, actor: Option<AccountId> }` (tag 34), after which the chain refuses every push and issue command. `updated_at` and `pushed_at` render as the creation time. SHARED's `RepoEvent` and "Multi-chain operations" say so, and F59 held the missing finisher, since built ("F59 closes: `Audit` finishes repository create and delete", which also reverses the delete order).

**Why.** BF-34's acceptance test creates and deletes its own repositories. GHES's `full-repository` needs `created_at`, and no event recorded one. The other 32.x events carry `actor: Option<AccountId>`, and `Actor` would have been the only enum-shaped actor on the chain's new events. The README push and `InitRepo` shared an id, so the leader answered `InitRepo` from the push's stored row.

**Beat.** Seeding repositories for the test, which would not run it as written. Recording `updated_at` and `pushed_at` now, with no route that reads them yet. A finisher built in the same round, which needs the `Audit` owed-work model M4-WP1 extends.

**Evidence.** `tests/m3/results/m3-repos.md`: Terraform 6 of 6 runs and 12 of 12 subtests, 12 of 12 created repositories 404 afterwards; `gh repo create` 20 of 20; REST smoke 23 of 23; 14 hosted replies valid against GHES 3.22; 14 of 14 planted bugs; the `auto_init` bug before and after `cb98c9d` [measured].

**Flip.** A client that reads `updated_at` or `pushed_at` and needs them true; or an edge crash measured stranding a name before F59's finisher exists, which would move the finisher ahead of M4-WP1.

#### A refused credential is 401 before the read gate

**Decision.** A REST repository request carrying a credential the edge refuses (revoked, lapsed, or unknown) gets 401 "Bad credentials" before `authorize_read` runs, so a bad token is never served as anonymous, even on a public repository. Git already answered such a request 401. SHARED, "Identity and tokens" (`authorize_read`), says so.

**Why.** GitHub answers 401 "Bad credentials" for a token it does not accept. Treating the request as anonymous made a revoked token's read of a private repository 404 and of a public one 200, so a client could not tell a dead token from a missing repository, and a dead token kept reading public data.

**Beat.** Leaving the gate to answer for anonymous callers, which serves public repositories to a dead token. Answering 404 for every refused credential, which hides the cause from gh and Octokit.

**Evidence.** `tests/m3/results/m3-repos.md`, "Edge read gate": 12 of 12 per case, with the revoked token's REST read 404 and its git read 401 before `932cc33` [measured]. "Edge read gate, after 932cc33", edge `c23944de` on `graft-m2b`, 12 of 12 per case: a revoked token 401 "Bad credentials" on REST for the private and the public repository and 401 on git; an unknown token 401 on both; anonymous 200 on the public repository and 404 on the private one; and no driver relied on a bad token being served as anonymous (`seeded.sh`, BF-31, BF-34, BF-35, and the smoke rerun) [measured].

**Flip.** A GHES 3.22 capture that answers a bad token on a public repository as anonymous, or a client that breaks on 401 where it expected public data.

#### AM1's hosted half: what the code needed, and LM-1 pending `am1-fix`

**Decision.**
- **L0-15:** a feed poll without `contents: read` answers 404, as task and lease reads do; the row now names 401, 404, or 429.
- **External tasks:** a `graftyard_task` claim binds only a live `GraftyardRun` task, so an `External` task links pushes only by trailer and sends its session on lease calls. The agent block says so.
- **`LeaseEnforce`:** for a pusher with no task, another holder is one whose actor differs, so a vendor App with only external tasks is never bound by its own leases. The agent block says so, and hosted L0-12 holds the lease with another actor.
- **Core rows:** they are what pushes, minting, and lease checks read; a merge reads its pull row and writes `Merged`, both Metadata, so L0-16 no longer lists merges.
- **Pending `am1-fix`, not settled:** LF-12's commit p99 (A24), and a recorded stall lost when leadership moves (F60). The lead authorized a raft change for F60. Both gates stay as written. Both since settled ("`am1-fix`: the stall crosses a leader change, and the feed shares one owner read", below).

**Why.** The edge's 404 for a reader who cannot see a field is the rule every other agent read follows, and 403 would confirm what the reader may not see. `graftyard_task` is how first-party sandboxes bind tokens; an external App reports its own work and gets no prefix. Binding a vendor App's tasks to each other needs a claim external tasks cannot carry. `tier_of` puts `Merged` and the pull row in Metadata.

**Beat.** Answering 403 for the feed only. Letting `graftyard_task` name an external task, which would hand third-party tokens a task claim. Splitting a vendor App's leases by session with no claim, which no push can prove.

**Evidence.** `tests/am1/results/am1-hosted.md`, "LM-1 rows" and contradictions 1 to 6: base journeys j2 and j3 20 of 20 and journey 4 10 of 10; L0-11's diffs only j3's new issues and the read gate's anonymous 404s; L0-12's refusals 100 of 100 for the confined vendor App and 0 for the three other kinds present; L0-13 for what ran; L0-16 0 metadata reads on 3 members for 8 checks; LF-1 100 of 100; LF-2, LF-3, LF-5 pass; B0-36 100 of 100 for the confined vendor App; B0-37's push half; L0-12's stall 0 ms of extension on 20 leases after 1,814,090 ms; LF-12 leader commit p99 3,414 ms; 25 of 25 planted bugs [measured].

**Flip.** For the feed: a GHES client that branches on 403 for a visible repository. For external tasks: a vendor that needs its tasks to bind each other, which would need a claim for external tasks. For core rows: a merge built from core rows alone. For F60 and A24: `am1-fix`'s measurements settle them.

### `am1-fix`, the stored event layout, and ref-count replies, 2026-11-09

`am1-fix` fixed LM-1's two hosted failures on `graft-m1` site day 7; its run found that main's 32.13 merge moved `RepoEvent`'s stored lease variants; and another thread found `repo:65536` resetting its leader on whole-ref reads (`tests/am1/results/am1-fix.md`; `build/BUILD-PLAN.md`, AM1-WP6; `decisions/OPEN-QUESTIONS.md`, F63, since closed by `wire-fix`).

#### `am1-fix`: the stall crosses a leader change, and the feed shares one owner read

**Decision.**
- F60 closes. A follower that stores a re-send of an entry above its commit, of the leader's own term, sent CALL_DEADLINE or more after the entry's `at`, marks `stalled_since` at that `at` (`96dc541`, `crates/raft/src/member/follower.rs`), the leader's own `tick_leader` condition. The follower that wins the next term stamps `extend` from it. DESIGN, Shape, "Chains" (`ExtendLeases`), lists it as the fourth source, and AGENT-LAYER's A8 says so.
- The uncovered path is accepted: an old leader that never reaches the winner after CALL_DEADLINE leaves no re-send, so the winner's first entry carries no `extend`, the same loss the design already accepts for a marker a crash loses.
- A24 closes. Each member keeps one owner-`Authz` read in flight for its feed, shared by the polls that wait on it (`760f9c0`). LF-12's commit p99 is measured at the group's leader; the client's latency through the edge is the client path, not the commit.

**Why.** With every member call blackholed, no `suspect` report reaches a follower and no campaign can win, so the old leader held the only marker; the winner's campaign marker was from the fault's end, `by` was about 0, and its term-start noop cleared the old marker everywhere. The re-send is what every working replication path sends after a missed CALL_DEADLINE, so it carries the stall to every follower that stores it. On LF-12 the leader was not CPU-bound and held no polls: its dep read of the owner's `Authz` waited behind one read per poll from every stale learner. A gate on the client's path from the orb would fail with no polls at all, so it would not measure the feed.

**Beat.** Sending the marker in the vote or the first AppendEntries reply, which changes two message formats for the case the re-send already covers. Having a new leader take the earliest marker a quorum reports, which needs a round before the first entry. Inferring a stall from the winner's own log, which cannot tell an uncommitted last entry from an idle group's last write. Caching the owner on the learner for longer, which lets a revocation reach the feed later.

**Evidence.** `tests/am1/results/am1-fix.md`: in memory, `l0_12_a_stall_ended_by_a_leader_change_moves_every_live_lease` fails on `6042f3e` and passes with the fix; `a_follower_marks_a_stall_only_from_a_late_resend_of_an_uncommitted_entry`; the long sweep passed in 335.75 s (HARSH 0 step-downs, GRAY 1,534 `STEP_DOWN` transfers) and Porcupine was Ok on 156,742 and 2,226,521 operations. Hosted day 7, lf1-5: a 30-minute stall with every member call failing, then a leader change; term 3's first entry carried `extend { since: 1794196661447, by: 1,761,355 ms }`, all 20 leases moved by 1,761 or 1,762 s (0 on day 5). LF-12 on lf1-6: leader commit p99 after `place_learners` 474 ms against 6,481 ms before the fix, 5,000 of 5,000 polls held against 1,421; 300 polls on a stale learner had sent 300 owner reads; with no polls, client p50 994 ms and p99 2,708 ms through the edge against the leader's 196 and 513 ms. 9 of 9 planted bugs [measured].

**Flip.** A hosted stall where the old leader stays cut off from the winner, measured losing leases that matter, which would need the marker carried in the vote; a re-send marker set on an idle group (a false extension); or a feed whose shared read holds a revocation past READ_CACHE.

#### Released wire positions are pinned, and `RepoEvent` keeps leases at 35 to 38

**Decision.** The lead's: `RepoEvent` keeps its current layout, with 32.13's `Created` and `Deleted` at 33 and 34 and the lease events at 35 to 38. Every stored or member-sent enum gets explicit, pinned wire tags, with a test that fails when a variant is inserted mid-enum, so a variant's wire position never changes once released; `wire-fix` built it as `crates/git/tests/wire_tags.rs` and `wire_tags.golden`. SHARED's "Record formats and the two-release rule" and DESIGN's message rule (Shape, `Unsupported`) state the invariant. The `graft-m1` groups holding lease entries stored under `9fb5032` (j4, lf1-2, lf1-3) lost their history: they were recreated, empty, under the same ids, by clearing every member with `/retire` and creating the group on first use, because a plain reset cannot repair stored rows; LM-1's rows that used them need their seed again.

**Why.** Postcard numbers variants by position, while the digest encoding already used explicit tags, so inserting `Created` and `Deleted` ahead of the lease events in the `m3-repos` merge moved every stored lease variant by two and nothing caught it. 32.13 events are stored on `graft-m2b`, and every document already records leases at 35 to 38, so moving leases back would break `graft-m2b` and rewrite the records. The broken groups are test data on one install. In a rolling site day the same shift would make two releases misread each other's AppendEntries, so the rule has to be structural.

**Beat.** Moving the lease events back to 33 to 36, which breaks the stored 32.13 events. A decoder that tries both layouts, which hides the next shift. A review checklist instead of a failing test.

**Evidence.** `tests/am1/results/am1-fix.md`, "A finding: main's 32.13 merge renumbered the stored lease events": after site day 7, j4, lf1-2, and lf1-3 fail to restore with "Serde Deserialization Error" on every member, while the groups without lease entries restore; lease events were serde variants 33 to 36 at `9fb5032` and 35 to 38 at `6042f3e`; the digest tags were 35 to 38 at both [measured]. `tests/m3/results/wire-fix.md`: the pinning test covers 655 variants in 120 enums, its golden file generated from unchanged `f3fa02b` code, so today's bytes are unchanged; a swap fails it, an insert fails it or fails to compile on exhaustive matches; the three groups were recreated, empty [measured].

**Flip.** A real install's stored data in the old layout, which would need a one-time migration for it; or a stored encoding chosen for another reason that carries its own explicit tags, which would make the pinning test redundant.

#### No member reply grows with a repository's ref count

**Decision.** A design rule: catch-up takes ref changes from the log entries it already reads, only `rehydrate` reads a chain's whole ref set, and it reads in pages; info/refs pages too. DESIGN, Shape, "Git cache shards (derived)" (`IngestCursor`), says so. `wire-fix` built it and F63 closed ("F63 closes: ref reads are paged", below).

**Why.** A leader that serializes every ref in one JSON reply burns seconds of CPU and runs out of memory, which resets its isolate and every member in it. M2-WP1's defect (c) was the same class on a lookup probe, so a fix to one call site did not prevent the next.

**Beat.** Raising the isolate's memory headroom, which the platform does not offer. Fixing `prsync::warm`'s call alone, which leaves `/catchup` and info/refs with the same shape.

**Evidence.** Workers Logs, reported by another thread: on `repo:65536`, a site-a or site-b member that wins leadership runs `prsync::warm` from its alarm (`1c84ac6`), which calls `/catchup`, whose `core_refs` sends `/read {"q":{}}`; the leader burns 5 to 6 s of CPU and dies in `handle_alloc_error`, 41 times in 26 h from 2026-11-07 14:31Z [measured]. `tests/m2/results/m2-wp1.md`, defect (c): 303,144 refs on `repo:196628` in one `/read` [measured].

**Flip.** A page size measured too small to serve a clone's ref advertisement within its deadline, which would need a streamed reply instead of pages.

### `wire-fix` and `am1-rest`, 2026-11-09

`wire-fix` pinned every stored or member-sent enum's wire tags and fixed `repo:65536`'s whole-ref reads; `am1-rest` built in memory what LM-1's remaining hosted rows need (`tests/m3/results/wire-fix.md`; `tests/am1/results/am1-rest.md`; `build/BUILD-PLAN.md`, AM1-WP6).

#### F63 closes: ref reads are paged

**Decision.** No member reply carries more than one page of refs. A ref read answers at most `REFS_PAGE_MAX` (10,000) refs plus a `next` cursor, and `{"q":{}}` answers the whole set only while it fits one page, past which it answers `{"paged":10000}`, so no older caller can ask for everything. Routine catch-up applies the net ref changes that `/entries` now carries with each entry and never reads the ref set; only `rehydrate` pages the full set, and an entry without ref changes or a cursor below the log's first index means a rehydrate. The edge's receive-pack advertisement streams page by page with bytes identical to the whole one. The container's `/refs` skips and counts a ref whose object is missing instead of failing the set. Two defects found on the way are fixed with it: `/entries` reads one row at a time up to its page bound instead of loading 2,000 rows first, and two catch-ups of one network take turns (`CatchUps::turn`). DESIGN, Shape, "Git cache shards (derived)" (`IngestCursor`), states the rule; F64 keeps the advertisement's cost open.

**Why.** A reply that scales with the ref count runs the leader's isolate out of memory at a size real repositories reach, and that resets every member in the isolate, so the group loses its leader and the next winner repeats the read. Pages bound each reply whatever the ref count, and applying the entries' own ref changes makes catch-up's cost scale with what changed.

**Beat.** One larger reply under a higher memory limit, which the platform does not offer. A streamed member reply, which still makes the leader walk every ref on each catch-up. Reading the ref set on every catch-up but in pages, which keeps the cost scaling with the repository instead of the change.

**Evidence.** `tests/m3/results/wire-fix.md` [measured]. bf20 holds 571,872 refs; the old `{"q":{}}` reply at that size is 43,095,634 bytes. `crates/git/tests/refs_paging.rs` builds 571,872 refs and keeps every reply at most 1 MB, reassembles the set from 58 pages, and checks the streamed advertisement byte for byte; with the paged guard removed it fails. `/entries` leader CPU per page fell from about 160 ms to about 10 ms. Hosted on `graft-m2b`: before, 87 `handle_alloc_error` and 307 "exceeded its memory" on sites a and b over 50 h and no leader since 2026-11-07 14:31Z; after site day 1, 0 of each on all three sites from 10:00Z to 11:50Z, 5 cron wakes ok on each of the 3 sites, and a leader (site a, term 24) at 58 of 58 one-minute checks; `ls-remote` over v0 and v2 and `clone` work. `graft-m2b` runs `3fc2397`, whose catch-up waits for idle and then starts, not the final `6eb3433`, whose `turn` takes the slot in the same poll; `graft-m1` runs `6eb3433`.

**Flip.** A page size measured too small to serve a clone's ref advertisement within its deadline, which would need a streamed member reply instead of pages; or a catch-up whose entries' ref changes are measured to diverge from a paged rehydrate, which would make rehydrate the only path.

#### Site c rarely leads, and that matches the design's intent

**Decision.** No change. Only a group's record `home` receives `/campaign`; every site gets `suspect` reports and may start a pre-vote, and with equal logs pre-vote's tie-break denies the candidate with the higher site id, while the farthest site's vote requests arrive last. So a far site with the highest id, such as `graft-m2b`'s site c (apac), effectively never leads. DESIGN means `home` to lead and other sites to lead only after a loss, with `home` taking leadership back by transfer, so this matches. DESIGN, Shape, Leadership (`suspect`), says so.

**Why.** A leader on the far site costs every commit the longest round, so a bias against it is wanted.

**Beat.** Recording it as a defect and randomizing the tie-break, which would sometimes put the leader on the farthest site.

**Evidence.** `tests/m3/results/wire-fix.md`, "Why site c never leads" [inference, from the code and the call counts]: over 24 h on bf20, site a got 204 `/campaign` calls and sites b and c none, while `/suspect` reached a 586, b 909, and c 957 times; `election.rs` denies when `req.candidate > self.cfg.me && !newer`. No trace of one election's vote messages was taken.

**Flip.** A trace showing site c never leads even when sites a and b are both lost, or a placement whose `home` has the highest site id and is measured not to lead.

#### `am1-rest`'s design corrections

**Decision.** Four corrections from `am1-rest`, in the text that owns each:

- An `Archived` follows only once `Promoted` covers its range, so a range that names no pack gets an empty promotion (DESIGN, the `Promoted` paragraph), as `check_archived` refuses an `Archived` past the last `Promoted`.
- LF-13's fault is named: a leadership change while the follower is behind, or a log truncated past the follower, either of which makes the follower take `InstallSnapshot`. One dropped AppendEntries cannot leave a gap under one leader, which re-sends from the follower's match index (AGENT-LAYER and BUILD-PLAN, LF-13).
- `graftyard-workspace` reads a session tip by v2 `ls-refs` with a `ref-prefix`, raw, or by `git fetch` with a refspec, because git 2.56's `ls-remote <pattern>` sends no `ref-prefix` (the agent block, both files).
- LF-4 hosted reaches only issues and releases, 2 of the 8 trust resources, because the edge serves REST reads for no others; `crates/api/tests/trust.rs` covers all 8 in memory. Recorded as a gap in BUILD-PLAN, AM1-WP6.

**Why.** Each is the code's behavior, and the design text said otherwise or named a fault that cannot happen.

**Beat.** Making the code match the old text: relaxing `check_archived` for empty ranges saves one shard call and one empty pack per such range at the price of a weaker check, and a single dropped message cannot be made to leave a gap without breaking Raft's re-send.

**Evidence.** `tests/am1/results/am1-rest.md`, its contradictions 1 to 4 [measured, in memory]; `crates/raft/tests/lf13.rs`; `crates/api/tests/trust.rs`.

**Flip.** A REST read route for the other six trust resources, which closes the LF-4 gap; or a git release whose `ls-remote <pattern>` sends `ref-prefix`, which would let the controller use it.

### `am1-runs`: LM-1's hosted rows on `graft-m2b`, 2026-11-09

`am1-runs` ran AM1's remaining hosted rows on `graft-m2b`. L0-12's refusal counts, L0-13, B0-36, B0-37's push half, and LF-4 on issues and releases pass; every 500-agent load row and LF-13 fail (`tests/am1/results/am1-runs.md`; `build/BUILD-PLAN.md`, AM1-WP6; `decisions/OPEN-QUESTIONS.md`, A25, since closed by `fanout`).

#### A cursor inside an `InstallSnapshot` hole gets 410, as one older than the ring

**Decision.** A follower's feed ring that is fed an entry past a hole, as after an `InstallSnapshot`, restarts after that entry, so a cursor from before the hole gets 410 with the oldest cursor the ring can serve, the same answer as a cursor older than the ring. AGENT-LAYER, Shape, "Activity feed" (outside the agent block, so in AGENT-LAYER only), and its API table say so. Built in `dfa5410`; not yet deployed.

**Why.** The ring took each fed entry as contiguous, so after an install its floor stayed below the hole and a hold from an earlier cursor skipped every event the snapshot covered. 410 makes the client reload leases and tasks, which is the design's existing answer to a cursor it cannot serve.

**Beat.** Filling the hole from the snapshot's fold, which holds state, not the events a feed returns. Refusing the cursor as `lagging`, which a client would retry against the same hole.

**Evidence.** `tests/am1/results/am1-runs.md`, LF-13 install mode: three runs lost 6, 41, and 39 events on F across the hole [measured]. The test `a_ring_fed_past_a_hole_refuses_cursors_from_before_it` fails on the deployed code ("oldest Index(7), cursor Index(8)") and passes with `dfa5410`; 2 of 2 plants caught.

**Flip.** A feed that can serve the hole's events from somewhere else, such as the archived log, cheaply enough that a reload costs more than the read.

#### LM-1's load rows fail on the validate lock, and the agent cost lines wait for `fanout`'s rerun

**Decision.** The lead's: B0-29, B0-31 to B0-35, and L0-12's A1 replay fail as written, and their gates stay. The cause was recorded as A25 (since closed: "A25 closes", below), owned by AM1-WP1: the git shard's `/validate` holds the network's exclusive lock, so with k = 1 one network validates about 2 pushes/s against journey 9's 4.17/s. Branch `fanout` builds concurrent validates inside one shard, `fanout`, and the `NetworkLoad` report, then reruns the rows hosted. The cost figures for extra warm shards ($0 at journey 9) stay unchanged until that rerun measures them; DESIGN's Cost and Scale lines and AGENT-LAYER's accepted risk now say that k counts git CPU only. LF-13's gap fault stays unproven hosted, because even a leader change during the fault left no gap, and is retried on `fanout`.

**Why.** The failure is a missing build piece, `fanout` and in-shard concurrency, not a gate set too tight: at 100 and 250 agents the same driver passes its latency lines. Moving the cost figures before a measurement would replace one unmeasured number with another.

**Beat.** Raising k in the cost scripts now, from the lock's 2 pushes/s, which prices a fix not yet chosen. Relaxing the gates to what one locked shard holds.

**Evidence.** `tests/am1/results/am1-runs.md`, "Diagnosis" [measured]: 100 agents pushed 148 of 148 (commit p99 933 ms) and 250 pushed 379 of 379 (992 ms) with no leader change; at 500 the validate queue reached p90 17.7 s and max 58 s, and B0-29 acknowledged 5 of 2,500 agent pushes. Validates took 405 ms (100 agents) to 515 ms (250 agents) unloaded. LF-13 gap runs 2 and 3 moved the leader during the fault and showed 0 `lagging`.

**Flip.** `fanout`'s rerun passing the load rows at k = 1, which keeps the $0 line; or the rerun needing k above 1 at journey 9, which moves the cost figures through `scripts/cost_agents.py`.

### F65 follow-up: the edge reads an owner's `Authz` by read-index, 2026-11-11

Round 5 of `lm1-memory` timed each repository commit on the leader. On a 500-agent B0-29-shaped smoke with 8 learners, the leader's commit p99 was 50 s; its entries' storage confirm p99 was 202 ms and append-to-apply p99 1,000 ms, while its `Authz` dep reads had p99 17.7 s and `depsMs` p99 46.5 s. The owner's `Authz` leader took about 123 `/read`s a second, every one from the edge (`tests/am1/results/lm1-memory.md`, section 12).

**Decision.** The edge reads an owner's `Authz` fold (`authz_read`, behind `api::authz`, `ident::authz_of`, the read gate, and `install_authz`) and its repository-name rows (`resolve`, about 240 reads/s on the `Authz` leader at 500 agents once the fold reads had moved) with `graftyard_lookup::read_spread`: learners first, then followers, by read-index, under `READS_MAX` and the member caps, 503 with `Retry-After` when they shed it and never on to the leader. A repository leader's `fresh` dep read stays on the `Authz` leader. Rows place learners on the hot `Authz` group as on the repository's.

**Why.** A read-index answer holds every entry acknowledged before the read arrived, as a leader's `fresh` does, so permissions and the read gate keep their rule; the `Authz` leader keeps only writes and one dep read per repository entry.

**Beat.** An edge cache of the `Authz` fold: it would break read-your-writes for a permission change, which the read gate's 10 s cache already bounds for decisions only.

**Evidence.** `leader.split` in the smoke row, and `/read` counts per `Authz` member from Workers Logs (section 12).

**Flip.** `Authz` followers and learners that shed most reads at the row's rate, or dep reads still past 1 s on the repository leader after the move.

### Pushes stream through the edge, 2026-11-14

LM-1's B0-34 seed on `graft-m2b` failed every 60 MB push with `exceededMemory` or a hung exception on the edge: `receive_pack` read the whole body into one `Vec` before it parsed the commands, then copied it into the validate body and each bucket's put. B0-20 (`design/DESIGN.md`, Milestone 0) fails a push that needs buffering, so this is a code gap and the row stays as written.

**Decision.** The edge holds a push body only while it may still be inline: until its commands parse and more than `INLINE_MAX` of pack follows them (`graftyard_git::smart::BodyReader`). Such a body, and a gzip body, which git sends only below `postBuffer`, is handled as before, from the whole body. The rest of a longer body streams through `PackStore::place_streamed`: the tee hashes it, hands it on to validate through a one-chunk pipe, and fills two part buffers in JavaScript memory (`JsPart`), each put to every bucket still taking the pack before it is filled again. The cap counts every byte, and a body past it is answered the same `cap_refusal`. A streamed validate goes to one shard, with a 60 s wait and no trailer, and a non-422, non-429 answer is a 503 `Retry-After`. That shard is the first of `shard_order` whose bodiless `/catchup` probe answers caught up, else the last, so a streamed push fails over before its first byte and never after (amended 2026-11-14, LM-1 round 18c: sent to the first shard only, a big push through B0-34's home-shard stall would answer 503 with the other sites' shards never asked). The `DurablePack` still needs validate's index to name the SHA-256 and length the tee read.

**Why.** Two parts bound the isolate's pack bytes at 32 MiB at `PART_SIZE` whatever the push's size, and a part in a `Uint8Array` is passed to R2 as a view, so neither the wasm heap nor a copy per bucket holds it. The validate body cannot be read twice, and the trailer is the pack's last 20 bytes.

**Beat.** Buffering up to the cap and streaming above it: B0-34's 60 MB pushes are under any cap. Retrying validate on the next shard: it needs the body again, so the edge would hold it. Putting each part to two buckets and leaving the third to the filler: the filler copies whole packs, and the tee would still wait on two. Answering an early refusal (atomic refs, format, `COMMAND_MAX`, placement failing) before the rest of the body is read: the old code read the body first, so a body over the cap got the cap refusal, and it still does.

**Evidence.** `the_edge_holds_a_push_body_only_while_its_pack_may_be_inline` and `a_push_body_the_edge_does_not_stream_reads_as_when_it_was_held_whole` in `crates/git/src/smart.rs`; `a_streamed_pack_holds_two_parts_whatever_its_size`, `a_streamed_pack_is_durable_only_on_the_bytes_validate_indexed`, and `a_streamed_pack_that_breaks_off_places_nothing` in `crates/git/src/store.rs`; `tests/streampush/plant.py`; the hosted pushes on `graft-m3f` in `tests/am1/results/stream-push.md`.

**Flip.** A hosted push whose edge memory grows with the pack's size, or streamed pushes that answer 503 often enough on a warming home shard that a buffered retry across shards would serve them better.

### LM-1 round 6: shards stream push bodies and cap validates, 2026-11-12

Round 6's baseline (`tests/am1/results/lm1-memory.md`, section 13) failed B0-31, B0-32, and B0-34 in the git cache, not in the group: the first shard's Durable Object read each push body (`req.bytes()`) and copied it three or four times before the container saw it, and the container ran every validate at once.

**Decision.** The shard streams `/validate`'s body to the container (or to the pool shard) unread; the edge names the pack's trailer in the URL so the shard picks the fan-out shard without the body. The container runs at most min(16, max(4, 2 x vCPU)) validates; a human's waits for a slot, ahead of every agent's, and an agent's waits at most 2 s, with at most 8 per slot queued, then gets 429 with `Retry-After`, which the edge answers `ng <ref> agent admission full, retry after <n>s`. The container raises `RLIMIT_NOFILE` at start.

**Why.** B0-32's burst (512 validates of 256 KiB at 23:52:08Z) all failed on the first shard with "Promise will never complete" and its isolate's `exceededMemory`; B0-31's 1 MiB packs put the first shard over its 128 MB 725 times in one minute (1,713 of 2,500 agent pushes `unpacker error`); B0-34's container refused 480 agent pushes with "too many open files". A streamed body is held by the runtime, not the isolate's heap, and a slot cap bounds the files and processes one container opens.

**Beat.** A per-network in-flight cap in the Durable Object alone: it bounds memory but not the container's files when k shards share a pool container. Refusing agent pushes at the edge on a guessed rate: the container knows its own queue.

**Evidence.** `containers/git/server/gate_test.go` (human first, no slot leak on a deadline race, refusal at once past 8 per slot, a refused request reads no body); `a_validate_url_names_the_packs_trailer_and_its_class` and `a_busy_git_cache_refuses_in_the_admission_shape` in `workers/edge`; `tests/lm1mem/plant.py` round 6 plants.

**Flip.** A hosted row where agents are refused `admission full` while the container's `/load` shows idle slots, or one where 16 validates at once exceed a `standard-2` container's memory.

### F65 follow-up: a read the members shed is 503, never the leader's; learners take reads first, 2026-11-11

Round 3 of `lm1-memory` moved API tier reads and the read gate's rows to followers, but every row still failed: when both followers shed a read, `read_spread` sent it to the leader by `call_opt` and retried an `overloaded` leader up to four times, so the leader still took about a third of read invocations and stepped down on `CommitErrors` and `Confirm` (`tests/am1/results/lm1-memory.md`, section 10).

**Decision.** `read_spread` asks the group's learners first, then its followers, at most three members. If none answers a value and any sheds the read (`overloaded`, `busy`, a 5xx, or no answer within `FOLLOWER_CALL`), the edge answers 503 with `Retry-After` (1 s when shed, 5 s otherwise) and does not ask the leader. Only a read every member asked declines as not its own (`notLeader`, or a 4xx such as a group not yet created) goes to the leader. Leader-only reads (deps, refs, `ranges`, `/entries`) are unchanged. The lookup learns a config's learners with its voters, and reads a member's `/status` once per config index it has not seen. Rows place learners with the leader's `/learners` route before each row (`tests/fanout/learners.mjs`), and journey 9's reads count 6 times each against `LEARNER_READS` (`design/DESIGN.md`, Scale, learners).

**Why.** A read a follower or learner sheds is load the leader cannot take either: its thread is what moved the leader in every round-3 row. A 503 with `Retry-After` costs a client one retry; a leader step-down costs every client the election.

**Beat.** Retrying the followers after `retryAfterMs` inside the edge, which holds the request and its isolate's memory. A gate decision cache for B0-33 keyed by repository and minute: B0-33's agents share one installation token, so its `jti` cache already holds one decision per isolate, and the 6,925 gate 503s were `call_opt` exhausting 20 s on a leaderless group (term 2 to 19), not cache misses.

**Evidence.** Unit tests `a_shed_read_never_reaches_the_leader`, `spread_reads_go_to_learners_then_followers_and_skip_the_leader`, `a_config_teaches_its_learners`, `a_failed_read_is_503_with_retry_after`, and `a_learner_behind_grants_no_read_from_its_older_fold`; `tests/lm1mem/plant.py`. One learner on `graft-m2b` served 244 to 408 narrow read-index reads/s and 13 to 25 whole-tier reads/s of a 958 KB dump from one client (section 11).

**Flip.** Clients that turn the 503s into a retry storm larger than the shed load, or rows that pass only with reads on the leader.

### F65 follow-up: API tier reads and the read gate's rows go to followers by read-index, 2026-11-11

Round 2 of `lm1-memory` held memory at 500 agents, but every row failed on the leader's one thread: up to 942 ms of CPU a second in `/read`, so its storage confirms and AppendEntries replies stepped late and `STEP_DOWN` fired (`tests/am1/results/lm1-memory.md`, sections 6d and 9).

**Decision.** The edge sends a repository's API tier reads (whole-tier and merge rows) and the read gate's access and agent rows to a follower by read-index first, rotated over both followers, then to the leader (`graftyard_lookup::read_spread`). A follower serves only those queries (`family::follower_query`), and whole-tier and merge rows only once its metadata tier is loaded; for anything else it names the leader. `READS_MAX`, `HEAVY_READS`, and the dump cache hold on a follower as on the leader. A follower that has not applied the leader's index within `FOLLOWER_WAIT` (1 s) answers `overloaded`. No follower answer raises `suspect`. A command's deps, refs, `ranges`, `/entries`, and the alarms' reads stay on the leader (`design/DESIGN.md`, "Follower read").

**Why.** A read-index read is `Fresh`: the leader confirms its commit index by the same lease or round as its own reads, and the follower answers only after applying it, so the answer holds every entry acknowledged before the read arrived. The reads moved are about 90% of the leader's inbound requests on B0-29.

**Beat.** Learners, which journey 9's load does not place (`LEARNER_AT` is 500 reads/s). Excusing slow confirms in `STEP_DOWN`, a `crates/raft` change for a cause in the host. A stale mode, which needs the customer-signed flag.

**Evidence.** Unit tests and planted bugs (`tests/lm1mem/plant.py`, 29 of 29), and `a_follower_behind_grants_no_read_from_its_older_fold` in `crates/fold/tests/install.rs`. Rows: `tests/am1/results/lm1-memory.md`, section 10.

**Flip.** Followers that step down or run out of memory in turn, which would move these reads to learners.

### F65: a leader caches its packed dump, caps whole-tier reads, and sheds without `suspect`, 2026-11-11

Branch `lm1-memory` found what took repository members past 128 MB at 500 agents and changed the read path on the leader and in the lookup (`tests/am1/results/lm1-memory.md`; `design/DESIGN.md`, Scale, journey 9, item 1).

**Decision.** A repository leader builds the packed whole-tier dump once per state of its tiers (each tier's `applied` and row-hash sum, and whether the metadata tier is loaded) and serves later reads at that state from the isolate; it builds nothing for a read whose caller has gone; it serves at most `HEAVY_READS` (16) whole-tier reads at once, each counted until its answer has streamed out of the isolate in 64 KiB pieces, lets a read rebuild a stale dump at most once per `DUMP_EVERY_MS` (1 s) and answers the others `overloaded` until the next rebuild is due; it also caps reads of any kind in flight on a member at `READS_MAX` (256), and builds a granted answer once per query in the input that granted it, as bytes the reads share and stream out piece by piece; and answers the next at once with a 200 `{"busy": true, "overloaded": {"retryAfterMs": 250}}`. The lookup (`graftyard_lookup::call_opt`) retries an `overloaded` member after its delay, up to four times, never reports it with `suspect`, and then fails with an error the edge answers as 503 with `Retry-After: 1`. Every `suspect` report one isolate sends is damped to one per (chain, member, term, scope) each `SUSPECT_GAP` (500 ms).

**Why.** Memory grew with whole-tier reads in flight, about 1.4 MB each on a 1.18 MB dump (59 MB at 40, 92 MB at 60, an isolate reset at 100), and the reads were the same tiers at the same index. Missed reads became `suspect` reports, which made the leader slower and the storm larger, and a leader that granted about 730 abandoned reads in one input spent 641 s building their dumps, which left B0-35's group with no leader for 10 minutes.

**Beat.** A per-isolate memory budget, which Workers does not expose per object. Serving whole-tier reads from followers by read-index, which moves the same dumps to members that also hold the feed. Answering the cap with 503, which the lookup counts as a missed member and reports. Damping `suspect` inside the Raft core, which changes `crates/raft` and the simulator's histories for a problem the caller makes. Sharing one whole-tier read among an edge isolate's concurrent requests, built and run in the 250-agent smoke: the runtime refuses a promise resolved from another request's context, and 58,783 of 59,239 API reads answered 500.

**Evidence.** `tests/am1/results/lm1-memory.md` [measured]: wasm memory against parallel reads on `graft-m2b`, Workers Logs in-flight and CPU per route on the failed rows' leaders, and the B0-35 leader's 641 s alarm invocation with 41.9 million rows read. Unit tests and planted bugs: `tests/lm1mem/plant.py`.

**Flip.** A row whose leader still exceeds 128 MB with the cache warm and the cap holding, which would move whole-tier reads off the leader; or a `suspect` delay of `SUSPECT_GAP` measured in a leader move, which would shorten the gap.

**Status, 2026-11-11: F65 narrowed, not answered.** On `d9ee594` the 500-agent smoke held every member under 80 MB with 1,237 of 1,242 pushes ok, but B0-29's apac leader reached 262 MB, and about 600 cheap reads completed in one millisecond after a stall and then `exceededMemory`; the rows fail on leader step-downs from `Confirm` and `CommitErrors` (`build/BUILD-PLAN.md`, AM1-WP6, "After the read cap"). Next hypothesis [unverified]: one quorum round grants every waiting read and the host builds all their answers before sending any. The round now running caps every leader read in flight and streams the granted answers. The `Fresh` rule in `design/DESIGN.md`, "Commit and read paths", is reworded to admit the dump cache, whose key is read after the lease check.

### C15 answered, and M4-WP2's pure half, 2026-11-13

A hosted probe confirmed FTS5 in Durable Object SQLite under `eu` and with no jurisdiction (`decisions/OPEN-QUESTIONS.md`, C15; `tests/m4/c15/`). M4-WP2's pure half then passed BF-5, BF-67, and BF-68 in memory (`tests/m4/results/m4-wp2.md`).

#### M4-WP2's pure half: formats, tags, and three design points

**Decision.** M4-WP2 takes repository format 32.19 (`REPO_FORMAT_WATCH`) and Identity format 0x14.3, the next free ones on main (32.17 is `REPO_FORMAT_LIFECYCLE`, and 32.18 is `REPO_FORMAT_CHECKS`). Wire tags, appended: `RepoEvent` 42 `Watchers`; `IdentityEvent` 6 `Record` and 7 `CountsCursor`; `IdentityOp` 8 `SetRecord` and 9 `CountsCursor`. New enums: `UserRecord`, `WatcherUpdate`, `inbox::Reason`, and `search::Domain`. Three design points. DESIGN, Shape, "Identity", states the first two, and `crates/identity/src/identity.rs` states the third:
- A watcher row is the account's whole state for its key at `src`, never a delta.
- `Counts` keeps a `CountsCursor` on each account's `Identity` chain, and `Notify` and `Search` commit SHARED's `ConsumerCursor` on the repository chain after their work.
- `ThreadId { repo, number }` with REST id `repo << 24 | number`.

**Why.**
- A row that carries the full state at a fold index is idempotent and order-free under Queues' at-least-once delivery. The consumer needs no per-record bookkeeping, and an older batch is refused by comparing one index.
- A consumer that reads one chain and writes others moves its cursor on the chain it reads, after the work, as SHARED's "Other consumers" rule already says.
- GHES's thread id is an opaque string, so packing (repository, number) lets the edge route `/notifications/threads/{id}` with no lookup table [inference].

**Beat.**
- Sending each `UserRecord` as a delta, which a reordered redelivery would apply out of order.
- A thread-id table in the Inbox, which a rebuilt Inbox would have to reproduce identically.
- A shared cursor format, which would give `Counts` a cursor on chains it never writes.

**Evidence.** `tests/m4/results/m4-wp2.md` [measured, in memory]:
- BF-5: star and notice both visible after 1,400 ms.
- BF-67: the rebuilds kept read and done state and every `sources` event; 10,000 stars were listed live and after a refold with an equal metadata digest; a redelivery applied 0.
- BF-68: EU documents only in `eu` shards; 7 of 13 hits with `incomplete_results` while a shard was killed; 14 of 14 after the generation-2 rebuild.
- 30 of 30 plants caught. 8 REST replies valid against GHES 3.22.
- `crates/git/tests/wire_tags.golden`: 24 lines added, none removed.

**Flip.**
- For the tags: none, once released.
- For the thread id: a repository id past 2^39 or an issue number past 2^24, which the packing cannot hold.
- For the watcher rows: a hosted `Counts` lagging past BF-5's 10 s because each batch carries whole state.

### `fanout`, the read gate's installation-token cache, and M4-WP1's pure half, 2026-11-10

`fanout` removed the git shard's per-network validate lock and built `fanout`; its hosted 500-agent reruns on `graft-m2b` then failed on the edge read gate. `04487ca` caches the gate's installation-token decisions, and LM-1's load rows are being rerun. M4-WP1's pure half passed BF-57 and BF-58 in memory (`tests/am1/results/fanout.md`; `tests/m4/results/m4-wp1.md`; `build/BUILD-PLAN.md`, AM1-WP1, AM1-WP6, and M4-WP1).

#### A25 closes: a shard validates one network's pushes concurrently, and `fanout` commits k as `RepoEvent::Fanout`

**Decision.** The answer to A25 is both pieces, with concurrency inside a shard first. The git shard's `/validate` takes the network's lock shared: each validate writes only its own quarantine and reads the repository through alternates, and only the steps that write the repository (`/ingest`, ref writes, repack, init, evict) take it alone, with one mutex for setting a validated pack aside. `fanout` is built as DESIGN states it, with three differences now in the design text: a warm site's first shard reports `NetworkLoad` only during minutes in which it validates pushes, so an idle network's k steps down through `LOAD_STALE` (180 s); the leader commits k as its own `RepoEvent::Fanout { k }` entry (repository format 32.15, wire tag 39), which the edge never sends; and a push validates on the shard that rendezvous on its pack's trailer picks, which catches up before it validates, with the first shard validating the push itself if that shard cannot answer. DESIGN's Cost line "At journey 9's push every 2 minutes k is also 1, with 0 extra shards ($0 a month)" stands, by git CPU, now with the lock removed; it is not yet shown hosted at 500 agents.

**Why.** The lock, not git CPU, held one network to about 2 pushes/s, so raising k alone would have paid for pool shards to work around a serialization the container did not need. A validate's writes are private to its quarantine, so sharing the lock costs no correctness, and the race tests and `git fsck --strict` show none. With the lock gone, one `standard-2` shard validates about 9 times journey 9's offered rate, which keeps k = 1 consistent with the cost model. Picking by the pack's trailer gives every re-send the same shard.

**Beat.** Raising k to spread validates across pool shards with the lock kept, which buys shards for queueing rather than CPU and would move the $0 line. A per-ref lock, which still serializes pushes to one ref's neighbours and adds lock bookkeeping that the quarantine already makes unnecessary. Picking only among caught-up shards, which needs every first shard to track each pool shard's cursor.

**Amended 2026-11-12 (LM-1 round 7).** The shared lock is taken only after the request body is read, and the geometric repack takes no network lock (it keeps every object of the packs it rolls, and git re-reads a pack it lost), with evict and a fresh init waiting on the repack instead. Held across a streamed body or a repack, the lock let one waiting writer queue every later reader of the network behind it: hosted, the home shard's ls-refs walls rose from 2.7 to 12 s in a minute at 0.65 vCPU. The flip is unchanged.

**Evidence.** `tests/am1/results/fanout.md` [measured]. Container bench (`tests/fanout/bench.py`, 30 KB thin packs): about 16 validates/s for one network before, at any concurrency; after, 38.7/s on 1 vCPU at 4 in flight, 39.0/s at 8, and 58.2/s on 4 vCPUs at 8; the writer path (validate, ingest, `POST /refs`) stays at 13.0/s before and after. Hosted before, about 2.7/s, bounded by each validate's own catch-up, which `CatchUps::fresh` now shares. Hosted on `graft-m2b`: 100 agents at k = 1 pushed 142 of 142 with commit p99 885 ms (933 ms before); 500 agents with k forced to 4 sent validates to pool shards at p50 456 ms (first shard p50 1.1 s), against a p90 of 17.7 s before, but pushed no more, because the read gate failed first. In-band stepping was not shown: 27 of 29 `/fanout` reports in B0-34 hit a follower that answered an error, and k stayed 1; `105c228` makes a follower answer `notLeader` but is not deployed. Race tests `TestConcurrentValidatesOfOneNetwork` and `TestFinishValidateOfTheSamePackAtOnce` under `-race`; 22 of 22 plants caught.

**Flip.** A hosted 500-agent run at k = 1, with the read gate off the leader, whose validates queue past the push deadline, which would move k above 1 at journey 9 and the cost line through `scripts/cost_agents.py`; or a validate found to write outside its quarantine, which would bring back the exclusive lock.

#### The read gate caches installation-token decisions by `jti`

**Decision.** The edge read gate's per-isolate cache keys a decision by the repository and a stored token's hash, an installation token's `jti` hashed with the domain prefix `graftyard-read-gate-ghs`, or no token, for `READ_CACHE`. A web session stays uncached. An installation token's revocation or narrowing is honoured within `READ_CACHE`, as a stored token's is. DESIGN, Shape, Identity, and SHARED, `authorize_read`, say so (`04487ca`).

**Why.** Before this, only stored user tokens were cached, so every `ghs_` push and every App read cost one leader `/read` of the repository's access rows: about 260/s at journey 9's 500 agents, on a leader whose whole budget is about 305 req/s. Missed reads raised `suspect` reports, leaders stepped down, and the gate answered 503 "the repository's access rows did not answer". A token's `jti` fixes every claim the decision depends on, so it is as safe a key as a stored token's hash. The domain prefix keeps a `jti` key from colliding with a stored token's hash.

**Beat.** Serving the access rows from a follower or from the shard's catch-up at the `fresh` floor, which removes the leader read for every credential but is a larger change; it is deferred until a run shows the cache is not enough. Keying by installation id and a permissions hash, which needs the claims parsed into a key and would share a decision across tokens the uninstall path revokes one by one.

**Evidence.** `tests/am1/results/fanout.md`, "What limits the 500-agent rows now" [measured for the limiter]: 16,777 leader `/read`s and 20,199 `/suspect` reports in B0-29's 600 s, 28,105 `/suspect` in B0-34, 23 `StepDown { CommitErrors }` and 15 member memory resets in B0-34's window, and 1,897 of 2,500 agent pushes in B0-29 and 11,415 of 12,546 in B0-33 answered 503 by the gate. That the cache removes the limiter is [inference] until the rerun of LM-1's load rows reports.

**Flip.** The rerun still showing leader `/read`s near the uncached rate, or revocations of installation tokens needing to bind reads sooner than `READ_CACHE`; either would move the access rows off the leader instead.

#### M4-WP1's pure half: tags after the merge, and four design points

**Decision.** M4-WP1 takes repository format 32.16 and no Authz format, so 0x12.10 stays free. On the merge into main, `RepoEvent::Hook` and `HookCursor` take wire tags 40 and 41, after `Fanout` at 39, which was already deployed on `graft-m2b`; on their branch they had 39 and 40. `IssueOp::PutHook` and `DeleteHook` are 38 and 39. The results file's four design points stay with SHARED, which owns them, and are recorded in BUILD-PLAN's M4-WP1 status: `HookDelivery.guid` is a derived UUIDv8 and `entry` is optional for pings; `RepoEvent::Hook` carries `created: bool` and an optional actor; the `merge_group` and queue payloads are checked against dotcom's description, which GHES 3.22 lacks; and the sequencer hands out each attempt, so a Queues message carries an attempt the sequencer started.

**Why.** A deployed tag cannot move (`decisions/DECISIONS.md`, "Released wire positions are pinned, and `RepoEvent` keeps leases at 35 to 38"), and `Fanout`'s 39 was stored on `graft-m2b` before M4-WP1 merged, while the hook events were stored nowhere.

**Beat.** Keeping the branch's 39 and 40 for the hook events and moving `Fanout`, which would break `graft-m2b`'s stored entries.

**Evidence.** `tests/m4/results/m4-wp1.md` [measured, in memory]: BF-57 healthy first-attempt p99 700 ms, the slow hook's 201 deliveries in chain order, cursor lag at most 1,000 ms; BF-58 465 of 465 (event, hook) pairs delivered, 0 under a new GUID; 34 of 34 plants caught; 628 tests passed. `crates/git/tests/wire_tags.golden` on main: `RepoEvent` 39 `Fanout`, 40 `Hook`, 41 `HookCursor`; `IssueOp` 38 `PutHook`, 39 `DeleteHook`.

**Flip.** None for the tags, which are pinned once released. For the design points: a hosted `HookSequencer` that cannot hold one attempt in flight per queue from the sequencer, which would move attempt choice into the delivery Workers.

### F59 closed, agent pushes read narrowly, and B0-30 substituted, 2026-11-10

F59's finisher for repository create and delete merged in memory (`tests/m3/results/f59.md`, merged at `c31a3d6`). `lm1-rerun` showed the gate cache working and the 500-agent rows failing on repository member memory, and `ccd10b1` narrowed the agent push's row read (`tests/am1/results/lm1-rerun.md`). B0-30 runs on `graft-m1` with pushes off (`build/BUILD-PLAN.md`, AM1-WP6; `decisions/OPEN-QUESTIONS.md`, G6).

#### F59 closes: `Audit` finishes repository create and delete

**Decision.** A repository create's commit point is `CreateRepoWith` on the owner's `Authz` chain (Authz format 0x12.11), which commits the name `Live` and an `AuthzEvent::RepoCreate { key, repo, init }` in one entry; `Audit` owes from it `RepoOwner` on `Names`, then `Lifecycle::Init` (repository format 32.17), which writes the first `Settings` and `Created` once from the owner's current enterprise. The abort is the operator-only `AbortCreate`, sent when no `Created` has committed `CREATE_ABORT_AFTER` (600 s [guess]) after the commit point, and it releases the name only while it is still `Live` for that repository. The README push of an `auto_init` create is never redone. `IssueOp::InitRepo`, still 32.13, is refused `NotFound` unless the owner's `Authz` names the repository `Live`, so a late edge create after an abort cannot revive the chain. A delete is reversed from `m3-repos`: its commit point is the name's `Deleted` on the owner's `Authz`, and the chain's `Deleted` follows, which `Audit` owes through `Lifecycle::Delete` while the name, `retain_until`, and `RepoOwner` still match and the deadline has not passed. A delete has no abort path. `Audit` writes repository chains only through the transfer `Settings` (32.10) and `Lifecycle` (32.17). SHARED, "Multi-chain operations" (Repository create and delete, Finisher), its opening rule, "Event types" (`AuthzEvent::RepoCreate`, `RepoEvent::Created` and `Deleted`), and "Tombstones" say so. The formats taken release the reservations of 32.17 and 0x12.11; F59 leaves `decisions/OPEN-QUESTIONS.md`.

**Why.** The old create could strand a `Live` name with no group, and the old delete a live name on a deleted chain, because nothing owed the steps after the first commit. Putting each follow-up behind a log-borne commit point on the owner's `Authz` lets the `Audit` cursor that already finishes transfers and enterprise changes finish these too. A deleted name is the safe commit point for a delete: once it commits, every route treats the repository as gone, so only the tombstone is owed and no abort is meaningful. Changing `InitRepo`'s `decide` needs no new format, because followers verify events and never re-decide commands: `Member::on_append` calls `verify`, which applies `Scratch::check` and `Scratch::apply` to the supplied events with no command input, and `RepoScratch::check` accepts ordinary `Settings` and `Created` events (`tests/m3/results/f59.md`, Risks, citing `crates/raft/src/member/follower.rs` and `crates/raft/src/member/log.rs`). An old leader during a mixed rollout does not enforce the new guard, so full enforcement follows the site rollout.

**Beat.** Keeping the chain-first delete with a finisher for the name, which leaves a window where a deleted chain still has a live name. A delete abort that restores the name, which would resurrect a repository every route had already dropped. Redoing the README push in `Audit`, which would need the git cache's `validate` inside the finisher. A new repository format for `InitRepo`'s guard, which verification does not need.

**Evidence.** `tests/m3/results/f59.md` [measured, in memory]: 58 of 58 tests (20 identity, 38 git), including `audit_alone_finishes_every_create_cut`, `audit_alone_finishes_a_delete_cut_short_and_a_finished_delete_is_inert`, and `a_create_whose_group_never_forms_is_aborted_and_a_late_edge_init_is_not_found`; 21 of 21 planted bugs caught after two tests were added. Wire tags: `AuthzEvent` `RepoCreate` tag 17; `AuthzOp` `CreateRepoWith` 30 and `AbortCreate` 31; `audit::Owed` `Create` 5 and `Delete` 6; `LifecycleOp` and `LifecycleErr` new. Risks recorded: a create whose initialization keeps being refused holds the owner's `AuditCursor` for up to 600 s, so later owed work on that owner waits behind it; `CREATE_INIT_MAX` (8 KiB of serialized `RepoSettings` [guess]) can turn a create that 32.13 accepted into 422; and the first 0x12.11 scan must owe tombstones for names deleted before F59. No hosted proof has run.

**Flip.** A hosted crash-point run in which `Audit` fails to finish a cut create or delete, or finishes one twice; a measured cursor stall from a stuck create that delays other owed work past what owners accept, which would move creates to a per-repository queue; or a GHES client whose create body exceeds `CREATE_INIT_MAX`, which would raise or remove the cap.

#### B0-30 counts for LM-1 as substituted

**Reversed, 2026-11-11.** The operator directed that every row passes as written, with no substitutions, and B0-30 then failed on its own fail line; see "B0-30 fails at `CHILD_GROUP_AT`, and the substitution is withdrawn". This entry is kept as the record of what was decided on 2026-11-10.

**Decision.** The operator's, Ben Kraus, 2026-11-10. B0-30 counts for LM-1 as substituted: the row passes if its metadata half passes, which is member SQLite under `CHILD_GROUP_AT` with zero `SQLITE_FULL`, and archived entries readable for `RestoredTo`, over 30 simulated days of journey 9's metadata rows with git pushes off from simulated day 1.34. The push half, 30 days of pack churn, is recorded as replaced, not passed. The git cache work the push half needs becomes AM1-WP1's follow-up, "Git cache shards on a hot group with a multi-GB log", with its own gate: journey 9's pushes on a 4 GB group through leader changes and shard resets, with no push timeouts and archiving keeping up (`decisions/OPEN-QUESTIONS.md`, G7). BUILD-PLAN's LM-1 row, AM1-WP6's B0-30 row and list, and AGENT-LAYER's AM1 gate say so.

**Why.** B0-30 asks a storage question: whether a hot group's member stays under `CHILD_GROUP_AT` with no `SQLITE_FULL`. The metadata rows a day are journey 9's, and the log stays at its cap through the archive alarm, which is what the row bounds. Pushes stopped because the git cache shard could not rehydrate a multi-GB log in time, a git-cache defect that the storage row was not built to find and that a 40 h run cannot fix in place. Holding LM-1 on it would tie the storage verdict to a separate piece of work.

**Beat.** Restarting B0-30 after the shard work lands, which costs another 40 h and about $65 and delays LM-1 behind unrelated work. Counting the run as a full pass, which would hide that 30 days of pack churn on the member's free pages was never exercised hosted. Failing the row, which records a storage failure that did not happen.

**Evidence.** `tests/am1/results/b0-30.md` on branch `b0-30` [measured]: 1,340,000 seeded comment rows, 4,017,184,768 B a member; member SQLite 6.68 to 6.99 GB and 0 `SQLITE_FULL` through simulated day 1.73; archiving 443 ranges through index 1,874. "Blocked on" items 5 to 11 record why pushes stopped; blocker 12, recorded after the decision, is that archiving stopped at 15:29Z on a pack pushed before pushes stopped (`422 … did not receive expected object`), so the log passed `LOG_MAX_BYTES` and the metadata half itself is at risk of crossing `CHILD_GROUP_AT` around simulated day 13 [inference] unless the recovery attempt works (`build/BUILD-PLAN.md`, AM1-WP6, B0-30); and four site fixes on the branch (`c3a2aa9`, `117f899`, `f9a05d3`, `8753c0a`). M0 measured 30 days of pack churn on one object: 8.45 GB after an archive pass, 9.09 GB at peak (`tests/m0/results/m0-wp9.md`, B0-30).

**Flip.** AM1-WP1's follow-up shows that push churn changes metadata or log growth enough to cross `CHILD_GROUP_AT` within 30 days, which makes the push half part of B0-30's verdict again.

#### The Graftyard-run clauses move from LM-1 to AM3-WP3

**Decision.** The operator's, Ben Kraus, 2026-11-10. L0-12's lease refusal for the Graftyard-run App kind, and B0-37's fetch half (a session ref appearing to a matching `ref-prefix`), move from LM-1 to AM3-WP3's gate, which builds Graftyard-run task creation. The lead applied the same decision to the third clause that needs a live Graftyard-run task: B0-36's Graftyard-run part, its lease refusal and its no-live-task and cancelled-task fork pushes for that kind. Every other part of the three rows passed hosted (`tests/am1/results/am1-runs.md`) and stays with LM-1. BUILD-PLAN's AM3-WP3 now lists the three clauses, and AM1-WP6's "What LM-1 still needs" no longer does.

**Why.** All three clauses need a live Graftyard-run task, which only AM3-WP3 creates: today a `graftyard-workspace` token with no task is refused by `NoLiveTask` before the lease check, a cancelled-task fork push needs a task to cancel, and only `graftyard-workspace` under such a task writes a session ref. LM-1 cannot reach them on its own work, and the code paths they test are already covered in memory (`crates/git/tests/lease_gates.rs`; `containers/git/server/sessions_test.go`).

**Beat.** Holding LM-1 open until AM3-WP3, which is after M5. Building a test-only Graftyard-run task path in AM1, which would test a path AM3 will replace.

**Evidence.** `tests/am1/results/am1-runs.md` [measured]: the Graftyard-run kind refused 100 of 100 pushes and 100 of 100 REST ref writes, all by "push needs a live task", in L0-12 and in B0-36's widened run; the session-ref hiding and refusal halves passed, with listing not reachable hosted. `tests/am1/results/am1-rest.md`: session refs advertised only to a v2 `ref-prefix` in the container.

**Flip.** AM3-WP3 slips past the point where LM-1's other rows close, so LM-1 cannot certify the Graftyard-run kind at all; then a test-only task path in AM1 would be worth building.

### `lf13-m3c`: LF-13 passes on its own install, 2026-11-11

Branch `lf13-m3c` ran LF-13 on a new test install, `graft-m3c`, and passed it in both modes, 3 of 3 runs each, on `9d59283` (`tests/am1/results/lf13-m3c.md`, `tests/am1/results/install-m3c.md`; `build/BUILD-PLAN.md`, AM1-WP6; `decisions/OPEN-QUESTIONS.md`, F66).

#### LF-13's 1 s runs from the follower's stop (A26 closes)

**Decision.** The lead's, under the operator's delegation: LF-13's "Every held poll returns within 1 s" is measured from the follower's stop, the moment it learns it is behind (a gap in its AppendEntries, or an `InstallSnapshot`). The time from the fault's lift is recorded beside it but does not gate. The row's fail line keeps "any poll held or accepted while it is behind", with its one opening recorded: between the lift and the stop the follower cannot know it is behind and admits new polls, which its stop then answers.

**Why.** From the lift, the time is bounded by the leader's resend interval to a recovered follower (`CALL_DEADLINE`, 1 s; `CANNOT_FOLLOW_RETRY`, 5 s, for one marked `cannot_follow`), which is Raft's pacing, not the feed's behavior. Until its stop the follower has no evidence that it is behind, so no feed rule can answer earlier.

**Beat.** Timing from the lift, which fails the row on resend timing alone: two earlier gap runs had 0 of 999 within 1 s of the lift (1,099 and 1,177 ms) while every poll came back at F's stop. Shortening the resend to a recovered follower to pass that reading, which adds member calls on every idle group for a test's sake.

**Evidence.** `tests/am1/results/lf13-m3c.md`, batch 4 [measured]: in 6 of 6 runs all 999 held polls returned within 0 ms of F's stop, and within 99 to 799 ms of the lift; F admitted 0 to 2 of 50 probes between the lift and its stop, and none returned an event past the gap or hole. Earlier batches, "A26: within 1 s of what".

**Flip.** A client that needs bounded staleness from the fault itself, not from the follower's knowledge of it, which would move the reference to the lift and the fix to the leader's resend pacing.

#### The feed ring restarts at an install's base at once

**Decision.** The input that installs a snapshot follows again in that same input, so it restarts the feed ring at the snapshot's `through` at once, with the live lease set the installed tier holds, read in `finish_install` after the install's tables are loaded (`7972504`, `606b403`, `9d59283`). A cursor from before gets 410 before the next entry arrives, and a lease the next entry releases still reports `Released`. AGENT-LAYER's activity feed, outside the agent block, now says a member follows only while it is not loading an `InstallSnapshot` and restarts its ring at the install's base. This replaces the hole check of "A cursor inside an `InstallSnapshot` hole gets 410, as one older than the ring", which fired only at the next entry. Two more feed fixes: a stop between two waits of a held poll still answers it (`3597c6f`), and a lease acquired and released in one catch-up batch reports both (`429eede`).

**Why.** AGENT-LAYER said a member holds polls only with "no `InstallSnapshot` catch-up", but a member is following again in the very input that installs a snapshot, before its ring has any entry, so "no event is lost" rested on a check that ran later.

**Beat.** Holding polls off until the first entry after an install, which leaves the member serving nothing on an idle group. Seeding the live set in `Repo::stepped`, which runs before the install's table swap is flushed and read the old tables (batch 3: 1, 0, and 2 `Released` missing).

**Evidence.** `tests/am1/results/lf13-m3c.md` [measured]: batch 1 lost 19 and 11 events from the pre-fault cursor, batch 2 one `Released`, batch 3 up to 2; batch 4 on `9d59283` lost 0 in 3 of 3 install runs. Tests `a_ring_skipped_past_an_install_refuses_older_cursors_before_the_next_entry`, `a_stop_between_two_waits_of_a_held_poll_still_answers_it`, and `a_lease_acquired_and_released_in_one_catch_up_batch_reports_both`; 7 of 7 planted bugs caught.

**Flip.** A feed that serves an install's covered events from the archived log, as in the earlier entry's flip.

### Idle git cache shards stay stopped, 2026-11-11

**Decision.** Three changes, so no shard container runs for a repository nobody uses. `PrSync` and `MergeQueue` park a chain once `PARK_AFTER` (10) batches went out from one cursor, store the park on the member, and are unparked by a moved cursor or the member route `/prsync {"replay": true}`; the queue consumer acks a failed `PrSync` re-send instead of retrying it. A new leadership's `warm` catches up its site's shard only if the container runs, and never counts as the shard's last request. `rehydrate` loads a marked pack that a chain's tail names with the tail, in index order.

**Why.** On `graft-m2b`, with no load, 20 of the torn-down network shards were running again within an hour (`tests/am1/results/idle-shards.md`). Two loops did it. A rehydrate loaded each network's first stored pack, which is thin against an earlier inline pack, before that inline pack, so it failed every time. Each `PrSync` test merge, re-sent forever every 30 s and retried 10 times by Queues, and each new leadership's `warm`, started the shard again to fail the same way.

**Beat.** Dropping `warm`, which brings back BM-2's 270 to 540 ms catch-up on a freshly elected leader's first merge. A backoff without a bound, which still wakes the shard forever. Retrying a 422 ingest later in the same call, which would hide the ordering bug.

**Evidence.** `tests/am1/results/idle-shards.md` [measured]: the before and after idle hours, Workers Logs of the re-sends and warms, and an offline replay of 131216's 423 packs, which ingest in log order and fail with the first stored pack loaded first.

**Flip.** An idle install whose shards still start with these changes in, which would name another caller; or BM-2 failing because the shard was stopped at an election, which would warm a stopped shard only for a group with a client in the last hour.

### `merge-budget`: the test-merge budget is the merging shard's, 2026-11-11

Branch `merge-budget` built AM1-WP1's follow-up, "The test-merge budget and B0-35's instruments", and ran it on `graft-m3c` (`tests/am1/results/merge-budget.md`; `build/BUILD-PLAN.md`, AM1-WP1).

#### A27 closes: a network's `MergeBudget` lives in the shard that runs its merges, charged by measured time

**Decision.** The shard object that runs a network's test merges holds that network's `MergeBudget` in its memory (`graftyard_git::prsync::Budgets`). That shard is the network's first shard on the leader's site. The budget refills at TEST_MERGE_BUDGET of that one shard's vCPU, read from its instance type (0.25 vCPU on today's `standard-2`). Each merge it admits is charged its measured time, not the catch-up before it. Past the budget the shard answers `{"shed": true}` and runs nothing, and the `PrSync` consumer deletes the old merge ref, so `mergeable` stays null. The shard reports `spentMs`, `computed`, `shed`, `listAsked`, and `listComputes` with each merge (`x-graft-budget`) and in `/status`. The leader's `/stats` keeps them as `testMerges`, with its first attempts' asks and sheds. DESIGN's and AGENT-LAYER's agent block now say "per network in the memory of the shard that runs its merges (the leader site's first), refills at TEST_MERGE_BUDGET of its vCPU and is charged each merge's measured time". A27 leaves OPEN-QUESTIONS.

**Why.** The consumer runs in whichever isolate the leader's first attempt or the queue lands in, so a budget there was per isolate: never shared, never charged, never reported. The shard sees every merge of its network, whoever asks, and can time the merge itself. The design's "of the k shards' vCPU" priced merges that only one of the k shards ever runs. The code's `SHARD_VCPU` = 4 priced a `standard-4` while the site runs `standard-2`, which allowed 100% of the shard.

**Beat.** Keeping the budget in the consumer and moving it into a shared store, which adds a call on every merge for state the shard already sees. Charging the estimate only (50 ms), which a slow merge on a large repository (up to 338 ms at p99 on torvalds/linux) would overrun unseen. Spreading test merges over the k shards, which needs every one of them caught up to the PR's head before it merges.

**Evidence.** `tests/am1/results/merge-budget.md` [measured]. On `graft-m3c`, one repository with 50 open PRs had head pushes at 10/s and the base moved 27 times in 300 s. Its shard computed 190 test merges, shed 694, and spent 55.3 vCPU-s, which is 0.1845 vCPU against 0.25. It computed 0 merges for list fields, and 2,952 of 3,012 `mergeable` polls answered `UNKNOWN`. Tests `the_shard_budgets_each_network_once_for_all_its_callers`, `a_head_push_past_the_test_merge_budget_leaves_mergeable_null`, and `a_compute_charged_to_a_list_field_is_counted`; 18 of 18 planted bugs caught (`tests/budget/plant.py`).

**Flip.** Test merges that run on more than one shard of a network, such as merges spread over k, which would need one budget across those shards. Or a merge whose wall time misprices its CPU, because the shard runs merges side by side or waits on disk, which would charge container CPU time instead.

### K9: learners serve whole-tier reads from one shared dump, and are priced, 2026-11-12

Branch `learner-dumps` ran journey 9's API reads on `graft-m3c` with 8 learners (`tests/am1/results/learner-dumps.md`; `build/BUILD-PLAN.md`, AM1-WP6).

**Decision.** A packed whole-tier dump is built once per state of a repository's tiers as serialized bytes (`family::dump_bytes`), and every granted read at that state, from any input, answers those bytes (`host::fold_answer`). `HEAVY_READS` no longer holds on a learner; `READS_MAX` does. A learner rebuilds a stale dump at most once per `LEARNER_DUMP_EVERY_MS` (500 ms [guess]), and its read-index read whose rebuild is not due waits for it within `FOLLOWER_WAIT` and is granted again instead of answering `overloaded`. Voters keep `HEAVY_READS` and `DUMP_EVERY_MS`. `GroupLoad` counts a whole-tier read as 50 narrow ones, and `scripts/cost_agents.py` prices the learners `place_learners` adds: 0, 2, and 11 on the largest network at 50, 300, and 2,000 developers, $0, $16, and $237 a month. DESIGN's "Follower read", journey 9, Scale ("Hot group reads, learners"), and Cost say so, and K9 is closed.

**Why.** At journey 9 every REST and GraphQL repository read, `GET /repos/{o}/{r}/issues` and `pullRequests` alike, goes through `api::read` to a whole-tier `{"rows": true, "packed": true}` read. The learners shed them at `HEAVY_READS` and `DUMP_EVERY_MS` while their CPU sat at 9 to 20% and their memory at 56 to 71 MiB, so the caps bound, not the learners. `HEAVY_READS` bounded the copies of a dump in flight, and a dump shared across inputs is one copy; a learner never votes, so its reads delay no AppendEntries reply or confirm.

**Beat.** Serving the list routes from narrow reads, which needs the metadata tables and logins each route renders and is the larger change; it remains the way to cut the edge's 2 s per answer. Placing more learners under the old caps, which each shed at the same rate. Raising `HEAVY_READS` for every member, which brings back the leader memory that F65 fixed.

**Evidence.** `tests/am1/results/learner-dumps.md` [measured]. Same install, repository shape (500 open PRs, 1.06 to 1.08 MB dump), 8 learners, and 300 s B0-29 load. Before: `read:api` 13,305 answered and 38,485 shed (25.7% of answered), `read:ref` 72.7%, agent pushes 761 of 1,251, the leader moved, voters peaked at 264 and 302 MiB, and the feed answered `lagging` 8,543 times. After: `read:api` 45,194 answered and 2,552 shed (94.7%), `read:ref` 99.8%, agent pushes 1,183 of 1,234, no leader change, voters at most 128 MiB, no `lagging`. Learners used 18 to 35% CPU at 61 to 68 MiB, built 88 to 132 dumps each, and served 1,102 to 1,929 reads each after a wait. API read p99 stayed 5.0 s, edge-bound, against learner reads at p99 1.2 to 1.3 s. 12 planted bugs caught (`tests/lm1mem/plant.py`, 41 to 52).

**Flip.** A learner whose memory or CPU binds before `READS_MAX`, such as on a dump past about 5 MB, which would bring back a cap on learners. Or API routes served from narrow reads, which would drop the 50 times weight and most learners: met on 2026-11-12 by `narrow-reads` ("`narrow-reads`: REST and GraphQL reads answer only the rows their reply renders"), which counts an API read once.

### `narrow-reads`: REST and GraphQL reads answer only the rows their reply renders, 2026-11-12

Branch `narrow-reads` served journey 9's API reads from narrow member reads (`tests/am1/results/narrow-reads.md`; F65; K9's flip).

**Decision.** A repository `GET` that `graftyard_api::narrow::rest_serves` names, and every GraphQL query, reads `{"rows": {"api": spec}}`, where the spec is the route: the REST tail, query, path, and trust opt-in, or the GraphQL document, variables, and operation. The member runs the same renderer over its own fold through a recorder, and answers the rows the renderer read, the read gate's access rows, and its selections (`Directory::pick`): a list's filter, sort, and page, a connection's page and `totalCount`, a lookup by a non-key column, and which of the reply's accounts the whole tiers name, with their lowest name shards, so the edge reads the same logins. The edge renders the route over those rows with those selections. A member caches each answer per query and tier state and builds one query's at most every `NARROW_EVERY_MS` (200 ms [guess]); a follower holds a read whose build is not due within `FOLLOWER_WAIT`, as a learner holds a dump read. `GroupLoad` counts an API read once, so `scripts/cost_agents.py` places no learners for the largest network at any tier, and the agent layer is $21, $274, and $2,572 a month.

**Why.** At journey 9 every API read fetched and parsed the 1.06 MB packed dump. With learners serving them (K9), learner `/read` p99 was 1.2 to 1.3 s and the edge spent 68 to 77 ms of CPU and about 2 s of wall per answer, so API p99 was 5.0 s against B0-18's 1 s learner read gate [measured, `tests/am1/results/learner-dumps.md`]. A page of 30 issues or 10 pull requests renders a few KB.

**Beat.** Per-route narrow queries, each with its own member code that selects the rows a route reads. That is a second renderer per route, and every one could drift from the whole-tier bytes. Recording keeps one renderer, so a reply is byte-identical by construction, and the tests compare the bytes over 480 REST list cases and every captured gh document. Caching whole-tier renders on the edge, which still parses 1 MB per state and isolate. Sharing one read across edge requests, which a Workers isolate refuses (F65).

**Evidence.** `crates/api/tests/narrow.rs`: replies are byte-identical to whole-tier replies, and pages, `Link` headers, `totalCount`, and `pageInfo` are checked against expectations derived independently from the rows. Narrow answers hold only their page's or object's rows, and stale and mismatched answers are tested. `tests/nr/plant.py` catches 29 of 29 planted bugs. `crates/api/gh/validate.py` passes on 80 GraphQL and 495 REST narrow replies. Hosted on `graft-m3c` with 8 learners and 300 s of B0-29 at 500 agents [measured, `tests/am1/results/narrow-reads.md`]: `read:api` served 93.1% before and 99.8% after (503s 3,271 and 85), p50/p99 1,902/4,982 and 1,714/4,370 ms. Edge CPU per answer fell from 66 to 67 ms p50 (170 ms p99) to 11 to 13 ms p50 (76 to 102 ms p99). Learners shed 706 to 2,824 reads each before and 0 to 35 after. There was no leader change in either load window. API p99 stays over 1 s, now set by the reads before the tier read (F65).

**Flip.** A route whose selection the recorder cannot see, which would answer different bytes from the whole-tier reply. A member whose narrow builds at journey 9 cost more CPU than a shared dump did, which would bring back the dump on learners for that route. Or a GraphQL client that sends `filterBy` or login qualifiers at journey 9's rate, which still read the whole tiers.

### `m0-carried-7`: sends stay as built, and an entry stops at `ENTRY_SOFT`, 2026-11-13

#### The far follower's held sends get no host change

**Decision.** The host keeps its send order. It does not issue a turn's sends before that turn's writes (option b), and it does not pipeline a second entry to the far follower (b'). This closes C20's send half.

**Why.** All 58 far-follower sends that `m0-carried-6` found held past an earlier entry's confirm were issued in turns that only handled an `AppendRes` and wrote nothing, so moving that turn's writes after its sends changes none of them. The near follower makes the quorum, so the far hold moved commit in no measured run, and pipelining would add Raft risk for no gate gain (operator, 2026-11-13).

**Beat.** (b), sends before writes in every turn, which `m0-carried-6` proposed before the turns were counted. (b'), a second entry in flight to the far follower.

**Evidence.** `tests/m0/results/carried-7.md`, "Held sends" [measured, `tests/m0carried/held.py` over the `carried-6` gate traces].

**Flip.** A run where the far follower makes the quorum and its held sends set commit p99.

**Partly reversed, 2026-11-13 (`m0-carried-8`).** The flip was met for (b'), not for (b). In four `m0-carried-7` A/B failures the near follower went quiet for about 1 s and the far follower made the quorum. In 9 of 9 near calls of 400 ms or more, the far call for an earlier entry was still out when the late entry was appended (50 to 1,235 ms left), so the entry reached the far follower one call late, 50 to 984 ms after its append. None of the 9 was held behind an unconfirmed write, and the far follower answered promptly (its own time p50 10 to 26 ms). See "AppendEntries are pipelined to a voter". (b) stays dropped.

#### An entry stops at `ENTRY_SOFT` bytes past its first command

**Decision.** The leader fills an `Events` entry only while it stays within `ENTRY_SOFT` (60,000 encoded bytes) past its first command; `ENTRY_MAX` (1.5 MB) still bounds one command alone, and admission's capacity counts entries of `ENTRY_SOFT`. This reverses "Entry batching gets no command or byte cap" (`m0-carried-2`).

**Why.** A Durable Object member's storage confirm grows with the entry. Over 48 one-minute B0-02 runs, confirm p99 was 95 to 132 ms for entries of 20 to 80 KB, 168 ms at 80 to 100 KB, 300 ms at 100 to 130 KB and 315 ms at 130 to 200 KB. With the leader's loop behind, 5 to 15% of confirms under 80 KB took 150 ms or more, against 26 to 65% above. The two `m0-carried-6` gate runs that failed had 210 entries of about 86 commands each, against 350 to 558 entries in passing runs, per-request CPU p999 of 112 and 117 ms against 16 to 39 ms, and 26 and 35 `/commit` turns of 80 ms or more against 1 to 4. They did the same work as passing runs: the same commands, and no reads, catch-ups or dumps. The operator approved about 80 KB from that table. At 80 KB the same-window A/B passed 7 of 8 capped and 7 of 8 uncapped, and the capped failure ran its entries pinned at 81 KB. Over those 31 runs a follower's AppendEntries body stepped just past 64 KiB: 0 to 2% of bodies up to 66 KiB took 60 ms or more to arrive, against 10 to 40% at 68 to 83 KiB, while the bare-object control with no write saw none at 80 KB. So the cap keeps an entry and its envelope under 64 KiB.

**Beat.** No cap, which `m0-carried-2` chose from the near round trip alone. A command-count cap, which does not bound bytes. 80 KB, which sits on the body step. A loop-lag step-down (C20's other half), held until the cap's A/B.

**Evidence.** `tests/m0/results/carried-6.md` (confirm by entry size); `tests/m0/results/carried-7.md` (the A/B and gate) [measured]. `a_backlog_splits_into_entries_within_entry_soft_in_order` in `crates/raft/tests/scenarios.rs` fails without the cap.

**Flip.** A hosted A/B at the same rate where capped entries do not lower commit p99 or the count of heavy `/commit` turns, or confirm p99 that stops growing with entry size.

**Measured, 2026-11-13 (`m0-carried-7`).** The cap removes big-entry heavy turns: a 20 s smoke at 330 offered ran 85 commands per entry uncapped (CPU p999 98 ms, 11 `/commit` turns of 80 ms or more, commit p99 1,000 ms) against 42 capped (14 ms, 0, 276 ms). It does not pass B0-02: over two same-window A/Bs, 60 KB passed 4 of 8, 80 KB 7 of 8 and 5 of 8, and no cap 7 of 8. The failures were a lagging leader loop under the cap and near-follower calls held about 1 s while the leader was idle (`tests/m0/results/carried-7.md`). The cap stays, because it bounds the turn's CPU and stays under the body step; whether it is worth its rounds is the flip above, which the A/Bs neither met nor ruled out.

**Amended, 2026-11-13 (`m0-entry-soft-packs`): the cap counts no Agent inline pack bytes.** The flip was met on journey 9. On `graft-m2b` (500 agents, standard-4, 8 learners, 180 s, one deploy), 60 KB put B0-29's commit p99 at 1,718 ms, against 702 ms with entries filled to `ENTRY_MAX`. Commands waited for an entry at p99 1,583 ms against 507 ms, while the round fell from 202 to 151 ms and the leader ran at about 9% CPU [measured, `tests/am1/results/lm1-memory.md`, round 9]. Agent packs of 10 to 70 KB go inline, so one push filled an entry, and admission's capacity shrank with the cap. Agent inline bytes already have their own per-entry bound, `AGENT_INLINE_ENTRY` (128 KiB), so the cap now counts an entry's other bytes, and the entry's full bytes stay within `ENTRY_MAX`. Human, MergeQueue and Bot packs still count, because no other rule bounds them. Admission's capacity estimate uses the same bytes. B0-02 has no packs, so its entries are unchanged. In the simulator on journey 9's mix, commit p99 was 365 ms capped against 245 ms at `ENTRY_MAX` before the change, and 245 ms against 245 ms after it (`entry_soft_does_not_slow_journey_9`, `crates/raft/tests/admission.rs`) [measured, simulator]. An entry can now reach about 60 KB plus 128 KiB, so it can cross the 64 KiB body step again. The long sweep (`RAFT_SEEDS=2000 RAFT_STEPS=10000`) passed with longest stretches of 16.0, 24.0 and 29.8 s, and Porcupine read Ok on all three histories, 6,000 segments each; plants 9 of 9 (`tests/m0carried/plant.py`). Settled by the hosted A/B on journey 9 (LM-1, `graft-m2b` site day m21, tree `74f0e1a` = main `a7d28e5`, standard-4, 500 agents, 180 s, one deploy): commit p50/p99 381/717 ms capped as shipped against 394/761 ms at `ENTRY_MAX` (`/stats?entry=max`), queue p99 499 against 508 ms, round p99 297 against 331 ms, leader held at term 2 in both [measured, `tests/am1/results/lm1-memory.md`, round 10]. The amended cap is no worse than uncapped, so the flip line is not met. Both arms miss B0-29's 600 ms line, which LM-1 owns. Contradicted on another install: narrow-reads-3 on `graft-m3e` (B0-29, 500 agents, main `84d29c2`) measured commit p99 2,902 and 2,893 ms at 60 KB, queue p99 2.7 s, against 949 ms with `ENTRY_MAX` on all voters, queue p99 580 ms (`tests/nr3/rows/` on branch `narrow-reads-3`) [measured]. The two A/Bs disagree; LM-1's journey 9 A/B on `m0-carried-8`'s 100 KB cap decides.

**Amended, 2026-11-13 (`m0-carried-8`): `ENTRY_SOFT` is 100,000 bytes.** The flip was met at 330 offered: on main `84d29c2` with the pipeline, one 60 KB entry a round held B0-02's group to 322 to 331 commands/s at a near round p50 of 169 to 174 ms, and r330-4 (1,495 ms) and r330-1 (646 ms) queued past the line; over 32 runs on that code, every run at a utilization of 0.91 or more reached 583 ms or more [measured, `tests/m0/results/carried-8.md`]. Entries of 90 to 110 KB carried about 92 commands in a near round of p50 234 ms, 393 commands/s, a utilization of 0.78 at 308 committed. A cut takes the whole queue up to the bound, so a bound that rises only once more than one entry is queued is the same rule as a flat one; at light load an entry still carries one round's arrivals (B0-02 at 300 committed: 33 to 50 commands). Agent inline packs still do not count. This crosses the 64 KiB body step that set 60 KB, and confirm p99 at 80 to 110 KB was 168 ms against 95 to 132 ms below: B0-02's gate on this code reports CPU p999, `/commit` turns of 80 ms or more and confirm p99 by entry size per run, and heavy turns fail it on their cause (operator). B0-02's gate on this tree passed 16 of 16 (commit p99 249 to 579 ms); heavy `/commit` turns were 28 over 16 runs against 24 at 60 KB, CPU p999 23 to 49 ms, and confirm p99 108 ms at 80 to 100 KB and 131 ms at 100 to 130 KB [measured, `tests/m0/results/carried-8.md`]. Pending: LM-1's B0-29 commit A/B on journey 9 on this tree before merge, which must also settle narrow-reads-3's `graft-m3e` result (60 KB at 2.9 s against 949 ms uncapped).

### `narrow-reads-2`: API reads start with their identity and name reads at once, cached per isolate for `READ_CACHE`, 2026-11-12

Branch `narrow-reads-2` (`tests/am1/results/narrow-reads-2.md`; F65).

**Decision.** A REST `GET` reads its token and its `o/r` at once, as GraphQL already did, and a `ghs_` token reads its verifying keys and its `jti` row at once (the `jti` is in the claims before the signature is checked; a bad signature refuses the token whatever the row says). The `o/r` resolves in the owner's `Authz` fold, which the read gate reads anyway, so the name costs no read of its own once the fold is held. A REST `GET` or GraphQL query (`Ctx::read_class`) takes the token row, the keys, the `jti` row, the login row, the `Placement` row, the owner's `Authz` fold, and accounts' logins from a per-isolate cache when it holds one at most `READ_CACHE` (10 s) old from when its read started (`workers/edge/src/rcache.rs`). The cache stores values, keeps only answers that let a request through, and an older answer never replaces a newer one. A write-class request never reads it, and refreshes it. Every API request records `server-timing` stages (`auth`, `resolve`, `tier`, `authz`, `logins`, `render`, `total`) on test installs, and one in 100 logs them as `apiTiming` on every install. Journey 9's API read target is p99 at most 2 s with at least 99% served, a new Pass clause on B0-29's row, with a Fail at p99 over 5 s.

**Why.** After `narrow-reads`, API p99 at journey 9 was 4.4 s with the edge's render at 11 to 13 ms of CPU, and an idle edge spent 1.2 to 1.4 s before the tier read: the token (about 200 ms), `o/r` through the login row, the `Placement` row, and the owner's `Authz` name read in sequence (500 to 650 ms), then the tier and `Authz` reads (300 to 650 ms) [measured, `tests/am1/results/narrow-reads.md`]. Every one of those identity reads answered the same thing for every request in a 10 s window at 500 agents, and each queued on the same system groups at 200 reads/s. DESIGN had no API latency target. GitHub's API gives up on a request at 10 s and answers a GraphQL timeout with 502 or 504 (`research/wire-census.md`, item 22), and journey 9's agents poll their PR every 5 s, so an answer later than the poll interval overlaps the next one. 2 s leaves B0-18's 1 s learner read for the member and 1 s for the edge's identity, name, and render [inference].

**Invalidation.** Only revocations wait out the cache. A revoked or lapsed token: its row is kept only while not revoked, its `expires` is checked on every use, and a revoked row is never stored, so reads honour a revocation within `READ_CACHE`, as SHARED's "Token revocation" deviation and journey 4 step 4 already state; writes see it at once. A revoked `jti`: kept only as "not revoked", so the same bound. A key rotation: a token that fails to verify on cached keys reads them again. A new repository, a rename's new name, or a transfer's new owner: a name the cached fold does not hold is resolved fresh, so it serves at once; an old name serves for up to 10 s, then resolves as an `Alias` as before. A grant, membership, or installation change that opens access: a read the cached fold would refuse reads the fold again before it answers 404. One that closes access (a removed member, a narrowed installation) may serve API reads for up to 10 s, as git's read gate already does (`crate::gate`); the repository's own access rows (visibility, collaborators) are not cached, since they ride each narrow answer. A placement move: the old record may be used for 10 s; reads find the group through the members it learned (`graftyard_lookup`), as before.

**Beat.** Folding the token and name into the repository's member read, which would make one member answer for a credential it does not own and move the identity check off its chain's leader; BM-2 ruled out passing the edge's authentication to the leader. Sharing one in-flight read across requests, which a Workers isolate refuses (F65). A longer cache, which would break SHARED's 10 s revocation bound. Caching negative answers, which would hide a new token or repository for 10 s.

**Evidence.** `workers/edge/src/rcache.rs` tests (TTL from the read's start, writes never read the cache, an older answer never wins, only passing answers are kept, a revoked row is never kept); `tests/nr2/plant.py` catches 12 of 12; `tests/nr2/stale.mjs` on the new edge: a new repository and a re-added member serve at once, a removed member stops within 1.8 to 2.3 s, a revoked PAT within 77 ms (its write at once), a revoked `ghs_` within 90 ms. Hosted B0-29 on `graft-m3c`, 500 agents, 8 learners [measured, `tests/am1/results/narrow-reads-2.md`]: `read:api` 99.8% served at p50/p99 1,515/4,865 ms before, 99.9% at 655/3,152 ms after on a run with no leader change (a first after run with a leader change mid-load: 96.8% at 749/3,303 ms). The sampled stages put `auth`, `resolve`, `authz`, and `logins` at 0 ms at p50 and the tier read at 611/2,589 ms (REST) and 629/2,163 ms (GraphQL). The gate is not met: the learners' answers set the p99 now.

**Flip.** A revocation, removed grant, or rename that a client must see on API reads in under 10 s, which would bring back fresh reads for that kind of answer. Or a hosted run where the identity stages no longer show in the p99 even uncached, which would make the cache a cost with no return.

### F67 rules 12 and 13: an emptied read-index batch leaves flight, and no hand-off onto a link that loses answers, 2026-11-13

Branch `f67-r12` (`tests/m0/results/f67.md`, "Rules 12 and 13").

**Decision.** (12) A follower drops a read-index batch from flight once every read in it failed past CAMPAIGN_DEADLINE, so the next batch goes out (`check_remote`). (13) A `/transfer` or drain hand-off refuses a target whose link from the leader is lossy while another follower's is not, and a running one of those is abandoned once its target's link turns so. Lossy is the commit-error rule's measure (over `commit_errors`, at least `commit_fail_min` failed) over every commit-path call in the `STEP_DOWN` window, including the misses `call_counts` leaves out. `STEP_DOWN` keeps its own target choice and probes, and the move `home` its HOME_CALLS rule.

**Why.** m0-carried-8 found main `a7d28e5` failing `a_read_only_group_elects_off_a_half_hung_leader` on seeds 21, 31, and 92 of 0 to 119: the emptied batch stayed in flight to the half-hung leader, later reads failed as queued, which forgets no leader, and no follower campaigned for the minute [measured, simulator]. Rule 12 alone pushed live seeds 10,273 (31.8 s), 11,401 (30.0 s), and 12,761 (31.6 s) past 30 s: each is a hand-off onto a half-hung follower whose losses the leader had counted but under `commit_fail_min`, after which the target led with its answers lost or the leader served nothing for the transfer's LEASE. The hand-off already refuses a `Suspected` target (rule 10); a lossy link is the leader's own evidence of the same fault.

**Beat.** Rule 12 without 13 (three seeds past 30 s). A clean-link rule (at most `commit_errors` failed, any calls): it refused drain targets on single noise misses, and left 10,576 and 10,768 past 30 s. Guarding the move `home` too: with crashes and partitions it refused 279 of 328 home moves the HOME_CALLS rule had cleared, which would delay B0-31's re-home. Refusing with no fallback when every link is lossy: that is usually the leader's own fault, which handing off cures.

**Evidence.** `leadership.rs` `every_unanswered_read_index_batch_is_a_failed_call` and `a_leader_hands_off_to_no_follower_whose_answers_it_loses` (with its control); `liveness.rs` `a_read_only_group_elects_off_a_half_hung_leader` now also runs seeds 21, 31, and 92. Each fails without its rule. With no faults but transfers and drains, 500 seeds made 14,786 transfers and rule 13 refused none; with 2% drops it refused 256 of 14,919 [measured, simulator]. Live seeds 10,000 to 13,999: none over 30 s, worst 26.2 s (12,361). Long sweep: HARSH 16,027 ms, gray 26,832 ms, live (4,000 seeds) 26,237 ms; Porcupine Ok on all three histories.

**Flip.** A live seed past 30 s in the 4,000-seed window, or a healthy group whose `/transfer` or drain hand-off rule 13 refuses for longer than one `STEP_DOWN` window.

### `narrow-reads-3`: a read that waits for an answer's build is answered at or above its floor, with no second read-index, 2026-11-13

Branch `narrow-reads-3` (`tests/am1/results/narrow-reads-3.md`; F65).

**Decision.** Every read grant carries a floor (`raft::ReadGrant::floor`): the commit index the leader confirmed for that read, its read index on a follower or learner and its applied index on the leader. A member keeps each narrow API query's newest answer with the index it was built at and whether the metadata tier was loaded, and serves it to any read whose floor it covers. A read whose floor is newer waits for the next build, spaced by `NARROW_EVERY_MS` (now 100 ms), and is then answered at the member's state at that moment (`raft::Member::fresh_after`, same boot only), with no second read-index round. A follower or learner numbers its read-index batches from its boot (`RemoteReads::starting_at`). Reads answered in one input share an answer only at the same floor.

**Why.** After `narrow-reads-2`, the learners' narrow tier read set API p99 at journey 9 (2.2 to 2.6 s on the edge, 8 learners at 200 reads/s), and each learner counted 448 to 4,427 reads that waited for a build and 428 to 3,461 answered late in 300 s [measured, `tests/am1/results/narrow-reads-2.md`]. Each wait slept the build's spacing and then redid the read-index: a new batch behind the one in flight (50 ms floor and a leader round trip), and in that time the tiers moved again, so the answer it waited for was stale once more and the read could wait again until `FOLLOWER_WAIT`. A build of journey 9's queries over 500 PRs costs 2 to 4 ms natively [measured], so the spacing protects little and the repeated read-index cost most of the wait [inference].

**Safety.** The rule stays "no answer older than the commit index the leader confirmed for that read": an answer built at index `i` is the fold at `i`, which is a function of the committed log, so an answer at `i >= floor` holds every entry acknowledged before the read arrived. Answering later at the member's current state is also at or above the floor, since its applied index never moves back within a boot. The simulator now answers 30% of granted follower reads later through `fresh_after`; its live sweep found that an answer the leader sent a follower's previous boot (late, or held for a learner's next call) matched the new boot's first batch number and granted a read below an acknowledged write. That is a bug on `main` too, and it is now a regression test (`a_read_index_answer_from_a_previous_boot_grants_nothing`, which fails on `main` 20147db).

**Beat.** Adding learners, which costs money every month and moves the queue (operator). Sharing one in-flight read-index among waiters only: the batch already shares one, and the cost was the second round per wait, not the first. Serving the newest cached answer whatever its index, which would answer below the floor. Removing `NARROW_EVERY_MS` outright: with no spacing a learner rebuilds on every entry for every query, and the hosted run should show the build share first.

**Evidence.** `crates/raft/tests/leadership.rs` (floor and `fresh_after`; the previous-boot answer), the simulator with deferred reads (long sweep and Porcupine below), `workers/site/src/fanout.rs` (an answer below the floor is never served, a covered floor never waits, loading and loaded answers are kept apart, another query's answer never serves), `workers/site/src/host.rs` (shared answers keyed by floor); `tests/nr3/plant.py` catches 11 of 11. Hosted on `graft-m3e`, 500 agents, 8 learners, two runs with no leader change: the tier read's edge p99 fell from 2,879 ms to 1,964 and 1,669 ms, learners' `late` answers from 96 to 3,399 each to 7 to 2,472 and 17 to 263, and every narrow build took 0 ms on the isolate's clock; API p99 was 2,112 ms (99.97% served) and 3,279 ms (99.9%), the second set by cold edge isolates' identity and `Release` genesis reads, not the tier read; 0 stale answers in 449 read-your-write rounds (`tests/am1/results/narrow-reads-3.md`).

**Flip.** A hosted run where the grant stage (`readStages.grant`, `tier_grant`) rather than the build wait sets the learner p99, which would make sharing or shortening the read-index round the next change. A narrow build that costs a learner more than about 20% of its CPU at journey 9, which would raise `NARROW_EVERY_MS` again.

### The edge carries its signed genesis as a deploy-time variable, and a wrong placement sends it to the `Release` view, 2026-11-13

Branch `narrow-reads-3` (`tests/am1/results/narrow-reads-3.md`; F65). Approved by the operator with the conditions below.

**Decision.** The deploy tool writes the committed signed genesis (the file whose SHA-256 is `install.toml`'s `genesis`) into the edge script as `GRAFTYARD_GENESIS_SIGNED`, hex, and refuses any other file. An edge isolate accepts it once with `accept_genesis` against its own anchor, as it accepts a `Release` member's answer; an absent, unsigned, malformed, or other-install binding is refused for the isolate's lifetime and the isolate reads the view. A system chain call under the bound record that finds no group or fails, and a system write that may create a group, first read the view once (`graftyard_lookup::check_bound`): the same genesis keeps the binding for the isolate, another replaces it for good and the call is retried under the view's record. Site scripts are unchanged ("Install operations", Genesis, step 6).

**Why.** On a clean 500-agent run the API p99 was 3.3 s, set by cold edge isolates whose `Release` genesis, token, name, `Placement`, and `Authz` reads took 1.2 to 3.8 s before the tier read; the `Release` group's follower answered 3,566 reads at p99 1.87 s [measured, `tests/am1/results/narrow-reads-3.md`]. The genesis never changes for an install (a restore is a new install), and the edge already ships its anchor, so the bytes add no trust: a forged binding is refused by the same check.

**Beat.** Caching the view longer (the cold isolate still reads it once, which is the tail). A site-script variable too (a site day for every script, for reads sites make rarely). Trusting the binding without a fallback, which a `LeaveEvaluation` that moves a system placement would leave pointing at the old groups.

**Evidence.** `crates/lookup` tests `the_genesis_binding_is_accepted_only_against_the_anchor` (another install, another key, unsigned, a flipped byte, not hex: each refused and never retried) and `a_miss_under_the_binding_defers_to_a_different_view`; `tests/nr3/plant.py` mutants 11 to 14.

**Flip.** An entry that moves system placements often enough that isolates pay the view read on most misses, or a deploy path that cannot read the committed genesis.

### A read-index answer follows the entries the asker needs, 2026-11-13

Branch `narrow-reads-3` (`tests/am1/results/narrow-reads-3.md`, "Where a far learner's wait goes"; F65). The operator's option (a), built after the split it asked for.

**Decision.** When the leader answers a learner's read-index batch at index i (`read_resolved`) and has not sent it the entries through i, it first sends that learner AppendEntries, each within `APPEND_BYTES_MAX` and none while the learner's link is lossy (F67 rule 13): after the calls in flight, up to `PIPELINE` calls in all, as a voter's pipeline would; or, when a refusal or failure emptied that pipeline, from the first entry the learner has not acknowledged (`next`). A voter is sent every entry as it is cut, so it gets nothing extra. Two builds went before. f1ab1a61 sent nothing once the pipeline was empty: gate7 and gate8 cut three far learners' wait for entries to p99 0.4 to 1.7 s, but one apac learner a run waited p99 4.8 to 7.8 s, because its calls and their hedged copies ended out of order, an append that overtook an earlier one was refused, the refusal emptied the pipeline, and the entries waited `CALL_DEADLINE` for a resend [measured, `tests/nr3/rows/trace-gate8.json.gz`: 313 of that learner's 455 holds over 500 ms had an append arrive behind a later one]. c95216cc re-sent from `next` on every read instead: gate9 took the leader to 122 MiB, its group changed leader twice, and 73.5% of API reads were served [measured; that the re-sent entries caused the change is an inference]. 617ed5b9 sent one call per read: gate10 served 99.38% at API p99 2,717 ms with seven learners at total wait p99 1.1 to 1.9 s, but apac l5 stayed about 12 entries behind, getting one to two per 350 ms round trip, at p99 6.3 s [measured, `tests/nr3/rows/trace-gate10.json.gz`]. A learner calling the leader gets both in the same response, entries first, and steps them through the normal append path. An answer with no entries ahead of it stays valid: the asker commits through i by digest if it holds entry i, and otherwise waits to apply i as before. No commit, lease, or grant rule changes; the extra AppendEntries carries no transfer and counts toward no quorum.

**Why.** gate6 (repo:65550, 500 agents, 300 s, the reverted tree plus traces) split each far learner's wait per read. weur and apac learners had their `ReadIndexRes` at p99 476 to 639 ms, then waited p50 574 to 668 ms and p99 1.9 to 4.5 s for the entries through the read index, and applied them at p99 461 to 709 ms. In 1,236 to 1,309 of about 1,500 batches each, those entries came in a later response to the learner's own call, not with the answer [measured]. The leader sends a learner one AppendEntries at a time and holds it for the learner's next call (`workers/site/src/callers.rs`), so the entries the answer names were usually not yet sent. (ii) dominates, which the operator set as the condition for (a).

**Beat.** (c), pipelining every append to learners, was tried and reverted ("Not adopted: pipelining AppendEntries to learners"): it gave no gain and took the leader to 126 MiB, because it held up to `PIPELINE` batches per learner whenever entries arrived. This sends entries only when a read needs them and the learner was not sent them. (b), a learner-side fetch, is two round trips where this is none extra. Routing reads by measured answer time stays unbuilt: the split shows the wait is not the far link's round trip.

**Evidence.** `crates/raft/tests/scenarios.rs`, `a_learner_read_index_answer_follows_the_entries_it_needs`, fails on the tree without it and on both earlier builds; `tests/nr3/plant.py` mutants 15 to 21 (no entries, f1ab1a61's nothing after a refusal, c95216cc's re-send on every read, none while a call is out, off by one, a lossy learner, 617ed5b9's one call per read) are all caught. Rows `tests/nr3/rows/a1-b0-29-gate{6,7,8,9,10}.jsonl` with their `run-*.status`, and the split from `tests/nr3/split.mjs`.

**Flip.** The far learners' wait for entries after their answer stays over about 500 ms at p99 with this in place, or the leader's memory or egress rises toward its limits with 8 learners reading.

### Not adopted: pipelining AppendEntries to learners, 2026-11-13

Branch `narrow-reads-3` (`tests/am1/results/narrow-reads-3.md`, "Learners pipelined"; F65). The operator's choice (c), built as 682468de and reverted.

**Decision.** `send_append` keeps pipelining only to voters (`voter && !lossy`); a learner gets one AppendEntries call at a time.

**Why.** On B0-29 at 500 agents with 8 learners (gate5, repo:65549), pipelining learners left the far learners' apply wait after their `ReadIndexRes` where it was. At p99, weur went from 969 and 764 ms to 1,415 and 878 ms, and apac from 1,670 and 1,442 ms to 1,671 and 1,858 ms. Far learners still answered `late` to 1,743 to 2,870 of 7,800 to 9,400 reads, and API p99 was 2,475 ms at 99.92% served [measured]. The leader's peak memory rose to 126 MiB, against 67 to 80 MiB in every earlier final-tree run, near the 128 MB isolate [measured; the cause, pipelined batches held per learner, is an inference]. A learner that is calling the leader gets its messages in the response to its next call (`workers/site/src/callers.rs`), so the leader made 1,119 to 1,129 calls to each voter in 300 s but only 46 to 60 to each learner [measured]; the pipeline had few calls to change.

**Beat.** Kept for the record: piggybacking entries on `ReadIndexRes` stays unbuilt until a split of the apply wait shows the wait for entries dominates (operator).

**Evidence.** Rows `tests/nr3/rows/a1-b0-29-gate5.jsonl`, `run-gate5.status`, `egress-gate5.txt`, `mem-gate5.json`.

**Flip.** A learner that receives AppendEntries as calls (not in responses), with apply wait set by appends one call late.

### `narrow-reads-3`: the edge routes a learner read by measured answer time, 2026-11-14

Branch `narrow-reads-3` (`tests/am1/results/narrow-reads-3.md`; F65). The operator's routing decision after gate 11.

**Decision.** `read_spread` (`crates/lookup`) still sends one request per read, learners first, at most `SPREAD_TRIES` members. Each edge isolate keeps a moving average of each learner's answer time as it saw it (`ROUTE_WEIGHT` 0.2 [guess]; a shed, refused or unanswered read counts as `FOLLOWER_CALL`), and orders learners by it: those within `ROUTE_MARGIN` (200 ms [guess]) of the fastest come first, rotated at random among themselves, then the rest by ascending average, then the followers. A learner it has not timed counts as fastest. One read in `ROUTE_EXPLORE` (20 [guess]) ignores the averages, so a learner that recovers is timed again. Every learner still does its read-index, so the choice changes latency and never what a read sees.

**Why.** Picked at random, the far learners (apac from an enam leader) answered about a quarter of reads at p99 2.2 to 3.0 s against p99 0.8 to 1.2 s on the near ones, which set the API p99 at 2.6 s on gate 11 [measured, `tests/nr3/rows/run-gate11.status`]. A far edge pays one far round trip to a near-leader learner instead.

**Beat.** A per-region gate (operator: a substitution). The hedge, which doubled learner load and served fewer reads ("Not adopted: the edge hedging a slow learner read"). Always the single fastest learner, which would put an edge's whole load on one learner.

**Evidence.** `crates/lookup/src/lib.rs` tests: near learners share reads, the margin is inclusive at its edge, an untimed learner comes first, an exploring read may go far, the average moves by the weight with a shed counted as `FOLLOWER_CALL`, far learners follow in ascending order. Each fails on one of four mutants: always the fastest, no margin, untimed as slowest, a strict margin. The hosted per-learner read share, CPU and memory come from the gate runs on the adopted B0-29 shape.

**Flip.** One learner taking so much of the reads that it sheds or runs hot (CPU or memory near its limits), or the far learners' answers falling within `ROUTE_MARGIN` of the near ones, which makes the ordering moot.

### `narrow-reads-3`: the edge spreads a learner read over at least three learners, by a score that counts variance, 2026-11-14

Branch `narrow-reads-3` (`tests/am1/results/narrow-reads-3.md`, "The herding guard"; F65). The operator's decision after gate 15: a herding guard before site day 13, not after another miss.

**Decision.** `timed_spread_order` (`crates/lookup`) scores each learner by its moving average plus `ROUTE_DEV` (4 [guess]) moving mean deviations, both at `ROUTE_WEIGHT` (0.2). The eligible learners are the untimed ones, those within `ROUTE_MARGIN` (200 ms) of the best timed score, and, while there are fewer than `ROUTE_SPREAD` (3 [guess]), the next best scored under `ROUTE_LATE` (1 s, a member's `FOLLOWER_WAIT`). A read goes to one of them at random, and the rest follow by score. A learner this isolate has not timed for `ROUTE_STALE` (10 s [guess]) counts as untimed: eligible, and not setting the best. That replaces the one read in `ROUTE_EXPLORE` that went to any learner at random. It is still one request per read, and every learner still does its read-index.

**Why.** Gate 15 missed by 19 ms with one learner taking 50.2% of the reads, shedding 54 and answering 720 late: ordered by the average alone, the fastest learner drew the reads that then slowed it. The far learners answered at `FOLLOWER_WAIT` 425 to 1,567 times each, mostly from the random one-in-20 reads [measured, gate 15]. Replayed over gates 13 to 15's traces, the guard cut the busiest learner's share on gate 15 from 36% to 24%, late answers from 2.6% to 1.7% of reads, and the projected API p99 from 1.44 to 1.25 s; on gate 13 from 1.50 to 1.29 s; on gate 14 it was flat at 0.76 s [inference, `tests/nr3/route.py`].

**Beat.** The mean-plus-deviation score alone, which in the replay left the busiest learner at 33% on gate 15 with explore kept. A random choice between the two best-scored learners ("top two"), which in the replay put 36% on one learner on gate 14 and gave a higher p99 on gates 13 and 15 (1.64 and 1.58 s). A spread of 4, no better than 3 in the replay. Keeping `ROUTE_EXPLORE`, which sends one read in 20 to a far learner that then answers late; the stale rule times a recovered learner without it.

**Evidence.** `crates/lookup/src/lib.rs` tests: the fastest learner gets at most one in `ROUTE_SPREAD` of the first places and a learner scored at `ROUTE_LATE` is never asked while others answer in time; an untimed or stale learner is eligible and does not set the best; the score is the average plus the deviation, and a swinging learner scores worse than a steady one; the margin is inclusive at its edge. `tests/nr3/plant.py` plants six mutants of the guard (no spread, padding past `ROUTE_LATE`, no deviation, no staleness, an untimed learner as the best, spread off by one); all are caught. The replay is `tests/nr3/route.py`; the hosted read share comes from gates 16 on.

**Flip.** A gate where a learner padded in by the spread sheds or answers late often enough to raise the API p99 above what routing to the best alone gave, or where the deviation keeps a learner that recovered out for long after `ROUTE_STALE`.

### `narrow-reads-3`: a follower or learner keeps several read-index batches in flight, 2026-11-14

Branch `narrow-reads-3` (`tests/am1/results/narrow-reads-3.md`, "Gate 12 on fdf363f3"; F65). The operator's decision 3 after gate 12: find what makes a busy learner's read slow and fix it before the next gates.

**Decision.** A follower or learner keeps up to `READ_PIPELINE` (8 [guess]) read-index batches in flight (`crates/raft/src/member/install.rs`), still sending at most one per `FOLLOWER_READ_EVERY` or `LEARNER_READ_EVERY`. Each `ReadIndexRes` grants only the reads of the batch it names, in whatever order answers arrive. A batch unanswered for CALL_DEADLINE is resent on its own, and F67 rule 12 holds per batch: one whose every read failed past CAMPAIGN_DEADLINE leaves flight, and failing reads that were sent still count as a failed call. Batches stay numbered from the boot, so an earlier boot's answer grants nothing.

**Why.** Gate 12's "own apply p99 1.3 to 1.6 s" on the near learners was an artifact of the split script, which counted a read already applied when its answer came (40 to 44% of them) as waiting for the next apply. Counted right, their apply is p99 0 to 6 ms and their storage confirms p99 84 to 158 ms, so neither storage, apply CPU, nor the read share set their reads. The read-index batch did: with one in flight, a read waited out the batch ahead and then its own round trip, a grant wait of p99 0.9 to 1.6 s against round trips of p99 0.6 to 0.9 s [measured, gate 12 traces]. A replay of those round trips puts the near learners' wait at p99 0.63 to 0.77 s with 8 in flight, and the edge's API p99 at about 1.2 s against 1.8 s with 1 [inference].

**Beat.** More learners or a lower `LEARNER_READ_EVERY`, which add leader calls without removing the wait behind a slow batch. Granting every batch in flight on any answer, which grants a read below a write acknowledged after an older batch's index (the kept test fails on it). Routing on a learner's answer variance as well as its mean, a smaller lever in the same replay, left for after the gates.

**Evidence.** `each_read_index_answer_grants_only_its_own_batch` (`crates/raft/tests/leadership.rs`), with answers in order and reversed; `every_unanswered_read_index_batch_is_a_failed_call` now runs more rounds than `READ_PIPELINE`. `tests/nr3/plant.py` plants four mutants (every batch granted by one answer, the oldest batch granted by any answer, later reads joining a batch in flight, one batch in flight) and rule 12's again; all are caught. The long sweep and Porcupine, and the hosted gates on the adopted B0-29 shape, are in the results file.

**Flip.** A near learner whose grant wait stays near two round trips with batches in flight, or a leader whose `ReadIndex` load or memory rises with them: then the wait is elsewhere, in the learner's isolate or the leader's input queue.

### Not adopted: the edge hedging a slow learner read, 2026-11-13

Branch `narrow-reads-3` (`tests/am1/results/narrow-reads-3.md`, "The final tree's gate runs"; F65).

**Decision.** `read_spread` stays one member at a time, learners rotated. A hedge that asked the faster of two random learners first and the next learner after 400 ms was built, tested, and run hosted (8c54adb), then reverted.

**Why.** On the final tree (main 84d29c2 merged), 22% of tier reads spent 0.9 to 1.5 s on a first learner that answered `late` before the next began, and a replay of the sampled stages put the hedged tier p99 near 1.9 s [inference]. Hosted, with `ENTRY_MAX` on both sides to hold the commit path still, the hedge took API p99 from 2,519 to 2,306 ms but served 97.11% against 99.74%: each learner took 17,000 to 22,000 reads in 300 s against 7,900 to 12,400, and answered `late` to 1,835 to 9,968 of them against 239 to 4,621, so the extra reads turned into the misses they were meant to cover [measured]. With ENTRY_SOFT the two hedged runs served 81.9% (driver-capped behind a commit p99 of 18.6 s) and 98.69% at API p99 1,988 ms [measured].

**Beat.** Kept for the decision: no learners added (operator).

**Evidence.** Rows `tests/nr3/rows/a1-b0-29-{abmax,abmax2,gate3,gate4}.jsonl`.

**Flip.** Learners with spare read capacity at journey 9, so that a second read is not answered `late`: for example, far learners whose apply wait falls under about 300 ms at p99.

### F67 reopened: no hand-off to a suspected member, a hand-off's bar ends with it, and a 4,000-seed live window, 2026-11-13

Branch `f67-wide` (`tests/m0/results/f67.md`, "Rules 10 and 11, and the wider window").

**Decision.** (10) A leader hands off, by `STEP_DOWN`, `home`, or a drained hand-off, to no member it refuses in pre-vote as `Suspected` (`may_lead_target`). (11) A `Suspected` bar whose every report arrived while the follower held that leader's transfer id ends once the follower accepts another leader's append; a report outside a transfer keeps it a full bar. The long sweep's live half runs at least 4,000 seeds from 10,000 (`sim.rs`, `LIVE_SEEDS_MIN`), whatever `RAFT_SEEDS` says.

**Why.** Main passed the 30 s bound on live seeds 10,000 to 11,999, the window the long sweep ran, and failed it on 12,048 (33.0 s) and 12,073 (37.4 s); m0-carried-8 found four more in 12,000 to 15,999 (30.1 to 34.6 s) [measured, simulator]. In 12,073 a drained leader handed leadership back to a half-hung member it already refused as suspected, so the gray leader came back each time until check quorum moved it. With that fixed, 11,121 showed the other side: reports sent during each leader's own hand-off barred both healthy voters from each other for 31.3 s. The 2,000-seed window had hidden both, and the bound is absolute (operator: "gates pass as written").

**Beat.** A relative gate, "no worse than main" (operator: refused). Setting no bar at all during a transfer, which left a half-hung leader free to win back. Dropping an emptied read-index batch from flight, which on main pushed 10,273, 11,401, and 12,761 past 30 s; it helps only narrow-reads-3's deferred reads, and stays there pending the agreed version with m0-carried-8.

**Evidence.** `crates/raft/tests/leadership.rs` `a_leader_hands_off_to_no_member_it_suspects` and `a_hand_offs_suspect_bar_ends_when_another_leader_is_heard` (with its control); `sim.rs` `a_leader_hands_off_to_no_member_it_suspects_live` (seeds 12,048 and 12,073). Each fails without its fix. Live seeds 10,000 to 15,999 with both fixes: none over 30 s, worst 29.3 s (14,945). Long sweep (`RAFT_SEEDS=2000`, live half 4,000 seeds): HARSH 16,027 ms (11,895), gray 26,832 ms (10,676), live 27,601 ms (13,161); Porcupine Ok on all three histories (`tests/m0/results/f67.md`, "Rules 10 and 11, and the wider window").

**Flip.** A live seed past 30 s in the 4,000-seed window again, or a gray leader that keeps clearing its bar by handing off, which would need the hand-off bar to count reports by target instead.

### `m0-carried-4`: a member call streams its request body, 2026-11-12

**Decision.** A member-to-member call sends its encoded envelope through a `FixedLengthStream` of its length (`workers/site/src/host.rs`, `call_peer`), not as one `Uint8Array` body.

**Why.** B0-02's last miss on `m0-carried-3` was a 337 ms pause of the leader's AppendEntries to a healthy near follower. A bare-object control with no Raft code, on the same account and hints, sends a body and starts a SQLite write one macrotask later, as the member does. With bodies of 250 to 400 KB the body's arrival at the callee took over 150 ms (up to 1,079 ms) in 78 of 5,782 calls (1.3%). With no write it did in 12 of 5,727 (0.2%), and through a `FixedLengthStream` in 6 of 5,491 (0.1%) with the write after the stream closed and 5 of 3,569 (0.14%) with it one macrotask after the send, as the member does. Calls over 400 ms were 42, 14, 5 and 6 [measured, `tests/m0/results/carried-4.md`]. workerd checks the output gate once, when the fetch starts, and pumps the body with no gate check after that, so an array body is not held by the gate; what stalls it is the platform's, below the API [inference, workerd `22a428de`, `api/http.c++` `fetchImpl`].

**Beat.** Moving the log and state writes to KV with `allowUnconfirmed` and an explicit `sync()`, which also stalled nothing in the control but rewrites the storage layer. Holding the write until the body has left, which the platform gives no signal for. Accepting the stall as F35.

**Evidence.** `tests/m0/results/carried-4.md`; `tests/m0carried/gateprobe/probe.ts` [measured].

**Flip.** Hosted B0-02 runs whose stall windows still line up with a write during a body, which would argue for the KV path. Or a platform change that ends the stall for array bodies.

### `m0-carried-3`: F68 closed by a near round-trip bound at placement, 2026-11-12

The operator chose F68's option (a), with (b) investigated, and rejected reading B0-02 per placement (`tests/m0/results/carried-3.md`).

#### A group's home must have a voter within `NEAR_RTT`

**Decision.** `Placement::new` refuses a voter set whose home's nearest other voter measured over `NEAR_RTT` for the home's region: 100 ms for an enam or weur home and 198 ms for an apac home, with no bound for other regions (`PlacementError::NoNearFollower`, appended last). A new group's leader runs `/place` once it leads: if the home is over the bound, it re-places that nearer follower with new generations through the replacement path (`Degraded`, then `Normal`), up to `PLACE_TRIES` (8). At the cap it keeps the try with the lowest near round trip, or the member it started with when no try was nearer, and the create succeeds: the bound is a goal, not a gate (`m0-carried-4`, below). The daily latency check on the home's leader applies the same bound, and a home over it on two checks a day apart re-places its nearer follower the same way. A move's swap and `graftyard placement set` check without the bound.

**Why.** B0-02's commit p99 follows the leader's round trip to its nearer follower: 0.93 correlation over 35 runs, every run at 100 ms or less passed, and 3 of 10 over 110 ms did [measured, `tests/m0/results/carried-2.md`]. Durable Object hints place a member in a region, not a city, so a new group's near RTT varied 75 to 144 ms with the same hints. 100 ms is B0-12's median enam to weur pair and 198 ms its median enam to apac pair, the same quantile [measured, `tests/m0/results/b0-12.json`]. The B0-02 rerun on groups created this way passed 15 of 16 runs, at 249 to 540 ms. Every near follower was at 75 to 100 ms, and 5 groups were re-placed in 1 to 4 tries. The one miss, 614 ms, was a single stall on the leader about 20 s in, with the near follower at 97 ms (`tests/m0/results/carried-3.md`).

**Beat.** Reading B0-02 per placement (rejected by the operator). A tighter bound, which costs more tries for no measured gain: no run at 100 ms or less missed. One bound for every region, which an apac home could never meet against enam's 133 ms floor (B0-12's p0).

**Evidence.** `tests/m0/results/carried-3.md`; `crates/types/src/placement.rs`, `new_refuses_a_home_with_no_voter_within_near_rtt`.

**Flip.** Misses that track the near RTT inside the bound, rather than one-off leader stalls, which would argue for a lower bound (90 ms costs about 1.6 times the tries). Creates no longer fail on the bound; groups kept over it (`placedOverNear`) on more than about 1 in 100 creates of an install would argue for a bound relative to that install's pair.

**Amended by `m0-carried-4`, 2026-11-12: a create never fails on the near bound alone.** On `graft-m2b`, `repo:131277` (home enam) re-placed its weur follower 8 times at 114, 110, 108, 110, 110, 110, 111 and 111 ms, the last check read 123 ms, and the create failed; a create a minute later passed. That install's enam to weur pair sits over 100 ms, so failing would refuse creates in production. Now, after `PLACE_TRIES`, the leader keeps the nearest try (`graftyard_types::nearest_try`), or the member it started with when it was nearer than every try, stores that try's round trip in its `Rtt` rows, counts the group in the member's `placedOverNear` (`/stats`), and logs it; the daily check keeps trying on the same path and keeps a new member only if it is nearer. A bound relative to each install's measured baseline (its minimum for the pair plus a margin) was considered and not built: with the nearest kept, it would only save tries on an install like `graft-m2b`, where each create now spends all 8 (about 30 to 40 s), and it needs a per-install baseline that no component keeps yet. Evidence: `crates/types/src/placement.rs`, `nearest_try_keeps_the_best_when_every_try_is_over_the_bound`; `tests/m0/results/carried-4.md`.

#### The per-KB append cost is the request body's transfer

**Decision.** Nothing changes for the about 1 ms per KB an AppendEntries call costs. It is recorded as the binding's transfer of the request body to the follower.

**Why.** Followers now time each `/raft` call: the body's arrival (`req.bytes()`), then decode, step, and the output gate. The work measured 0 ms in every call (the isolate's clock moves only across I/O), and the body's arrival grows with its size, at a median 0.54 ms/KB against 0.76 ms/KB for the whole call over 34 run-peers, with 0.14 ms/KB left for the network and the reply [measured, `tests/m0/results/carried-3.md`, "The per-KB append cost"]. So the cost is not serialization or the SQLite write but the platform moving the body, which the code cannot make cheaper short of sending fewer bytes, such as compressing entries, which B0-02's random-free 1 KB values would flatter and real pack bytes would not.

**Beat.** Compressing AppendEntries bodies, unmeasured on real entries and a wire change. Lowering the body by a cap, which `m0-carried-2` showed does not shorten a round.

**Evidence.** `tests/m0/results/carried-3.md`, "The per-KB append cost" [measured].

**Flip.** A transport whose per-KB cost is measurably lower, or entries that compress well enough to matter at B0-02's rate.

### `m0-carried-2`: B0-02's tail, 2026-11-12

Branch `m0-carried-2` traced B0-02's misses on the Rust host (`tests/m0/results/carried-2.md`).

#### A confirm trip disqualifies a target only on a failing link

**Decision.** On a confirm trip a `STEP_DOWN` target is disqualified by the leader's calls to it only when they fail over `commit_errors` with at least `commit_fail_min` failed, the same test the commit-error rule uses for a failing link.

**Why.** B0-02's r330-9 tripped the confirm rule on a leader storage stall and held with `to: None`, so the stall stayed on the leader to the end of the run. With no drain, boots, or earlier step-down, the rate filter was the only rule that could empty the candidates, and one or two missed calls in a window of about 21 calls pass 5%. F37 added `commit_fail_min` to the trip for that reason, and the target filter had not followed. A simulator case reproduces the hold on two failed calls to each follower and passes on the fix.

**Beat.** Dropping the target filter on a confirm trip, which could move a stalled leader onto a site its calls really fail to. Removing the confirm rule at B0-02's load, which leaves a stalled leader holding the round for the stall's length (diag-7, r330-9) while B0-06 measured the rule moving such leaders in 2.3 to 5.6 s.

**Evidence.** `tests/m0/results/carried-2.md`, "The `STEP_DOWN` hold" [measured hosted for the hold; the cause is inference, the fix measured in the simulator], and `crates/raft/tests/leadership.rs`, `a_confirm_trip_ignores_fewer_than_commit_fail_min_failures_to_a_target`. The long sweep and Porcupine: see that file's "Checks".

**Flip.** A confirm trip that transfers onto a follower whose link then fails, where the rate alone would have held.

#### Entry batching gets no command or byte cap

**Decision.** An entry stays capped only by `ENTRY_MAX` and `AGENT_INLINE_ENTRY`. B0-02's large entries are not capped.

**Why.** An entry carries what queued during the previous round, so its size follows the round, and the round follows the near follower's round trip: over 19 hosted runs without a leader stall, commit p99 correlates 0.88 with the near `Rtt`, every run at 100 ms or less passed, and the 656 ms run's group had 109 ms. A cap above the queue never binds, and one below it leaves commands for later rounds, so each waits more rounds.

**Beat.** A per-entry command or byte cap, as the operator asked to check.

**Evidence.** `tests/m0/results/carried-2.md`, "What sets the tail" [measured rounds and RTTs; the fixed-point argument is inference].

**Flip.** A run where a capped entry lowers commit p99 at the same near RTT and rate.

**Reversed, 2026-11-13 (`m0-carried-7`).** Entry size also sets the storage confirm, which this entry did not measure, and two `m0-carried-6` gate runs failed on 210 entries of about 86 commands. See "An entry stops at `ENTRY_SOFT` bytes past its first command".

### `merge-budget-2`: the budget holds under concurrency, GETs compute, and list fields are counted, 2026-11-12

Branch `merge-budget-2` closed A28, A29 and A30 (`tests/am1/results/merge-budget-2.md`; `build/BUILD-PLAN.md`, AM1-WP1).

#### A28 closes: a network's merges run one at a time, charged their CPU, on the tier's instance size

**Decision.** A network's test merges on its shard run one at a time (`graftyard_git::prsync::asked`, a per-network turn in `Budgets`). Each is admitted only after the one before it was charged, and pushes and dispatch are admitted from a positive balance, not from minus the estimate. So the bucket goes below zero by at most one merge's cost. A merge the budget would not admit is shed before its catch-up; one admitted then is decided again after its catch-up and its turn. Each merge is charged the CPU of its own git processes, which the container reports as `cpuMs` (user plus system time); wall time is the fallback when the container sends none. `BudgetStats` adds `wallMs`, `lowMs` (the bucket's lowest) and `maxMs` (the largest charge), so "never overrun beyond one merge" is `lowMs >= -max(maxMs, 50)`. The shard instance follows the tier: the site reads `SHARD_INSTANCE` (`standard-2` when unset, as the 50-developer tier and every test install run), and `instance_vcpu` refuses a name that is not a Containers instance type instead of budgeting it as `standard-4`. Journey 9's sizing stands: at 2,000 developers the shard is a `standard-4`, so the budget is 1 vCPU and head-push merges use 0.21 of it. `scripts/cost_agents.py` no longer counts k shards' budget, since merges run on one shard; its figures do not change, because k = 1 at journey 9.

**Why.** LM-1 round 5's Workers Logs for `graft-m2b-site-a`, repo 131260, show the cause. First attempts timed out at 25 s and their queued re-sends ran side by side: up to 5 consumer handlers overlapped, each with a merge step of 60 to 240 s. Each merge was admitted at the 50 ms estimate while earlier merges were still uncharged. In the worst window the shard spent 31,928 ms in 107.8 s against an allowance of 26,950 ms (21:44:04 to 21:45:52). Charges were wall time, about 232 ms a merge (164,135 ms over 707), while a test merge costs 23.5 ms of CPU in a one-vCPU container bench, so wall time also over-charged merges that waited for the container's vCPUs. The design's sizing was right; the code's `SHARD_VCPU` = 4 and the site's fixed `standard-2` disagreed with each other, not with the design.

**Beat.** Charging the estimate at admission and reconciling later, which is what overran. A reservation per merge in flight (admit while the balance covers every in-flight estimate), which bounds the spend only if the estimate bounds the cost, and a 338 ms p99 merge is 7 estimates. Sizing journey 9's budget on `standard-2` (0.25 vCPU), which leaves 0.04 vCPU for GETs at 500 pollers. Raising `TEST_MERGE_BUDGET`, which takes vCPU from validates on the one shard that does both.

**Evidence.** Round 5's logs, above [measured]. `concurrent_merges_overrun_the_budget_by_at_most_one_merge_and_are_charged_their_cpu`: 10 merges of 300 ms CPU in two bursts of 5 on a 0.25 vCPU budget. Admitting at the estimate would let a burst of 5 run and leave the bucket 1.25 s short; the test sees 4 computed, 6 shed, the low at -275 ms, and 1,200 ms charged for 3,600 ms of wall time. Planted bugs in `tests/budget/plant.py`. The hosted run is in the results file.

**Flip.** A merge's CPU that the container cannot see, such as work in the shard object outside its git processes, which would charge more than `cpuMs`. Or a network whose merges queue past their usefulness behind one turn (a merge's wait past `GET_COALESCE_MS`), which would admit a second merge while one runs and accept an overrun of two.

#### A30 closes: single-PR GETs compute through the shard, and Actions dispatch has its hook

**Decision.** After it answers a GraphQL read, the edge asks the leader site's shard of the network for the merges the read's `mergeable` or `mergeStateStatus` lacked (`graftyard_api::gql::MergeAsks`, `workers/edge/src/api.rs`, `ask_merges`), in `wait_until`, so the reply never waits. A single-PR read by a writer or an App token goes to `POST /demand?cause=get` with the (base tip, head) its merge lacks. There the shard counts the GET, coalesces it per PR for `GET_COALESCE_MS`, admits it under the budget with its reserve, computes it in its turn, and commits it as a `PrSync` with no cursor and no head moves (`graftyard_git::prsync::on_demand`), whose request id is its content's. The read itself answers `UNKNOWN`, as GHES 3.22 does while its background job computes, and the next poll reads the merge. Another reader's single-PR read is counted as a list's. `crate::prsync::demand` (`workers/site/src/prsync.rs`) is the hook M4's `ActionsTrigger` calls with `MergeCause::Dispatch`: it reads the PR fresh and sends the same `/demand`. REST `GET pulls/{n}` keeps `mergeable: null` and does not ask, because its renderer, shared with webhooks, never reads the merge [inference: a REST poller sees null either way]. (Reversed by `merge-budget-3`: REST asks too.)

**Why.** DESIGN names three causes, and only the head push was wired, so a polled PR past the budget stayed null until its next push.

**Beat.** Computing in the read, which would hold a GraphQL reply for a catch-up and a merge, seconds on a cold shard. Committing the GET's merge from the edge, which would send the pack through the edge twice.

**Evidence.** `a_read_asks_for_the_merge_its_mergeable_lacks_and_a_list_only_counts`, `an_on_demand_merge_records_the_current_head_and_moves_no_cursor`, `gets_coalesce_per_pr_a_shed_get_frees_its_slot_and_list_asks_are_counted`; the hosted run's `getAsked` and `getComputes`.

**Flip.** Clients that poll REST `pulls/{n}` for `mergeable`, which would wire the same ask into `rest::get_pull` and render the recorded merge there.

#### A29 closes: list fields are counted where they render

**Decision.** A list read that renders a PR's `mergeable` with no current merge (a `pullRequests` connection or search) sends the shard one `POST /ask?cause=list&count=k` after its reply, and the shard counts k list asks without waking the container. A list never sends a compute. B0-35's clause reads `listAsked > 0` with `listComputes = 0`.

**Why.** The shard never heard of list fields, so `listAsked` was 0 by construction and the clause was shown by structure alone.

**Beat.** An edge-side counter, which no one reads with the budget, and a lead decision that the structure suffices, which leaves the instrument meaningless.

**Evidence.** As for A30, and the hosted run's `listAsked`.

**Flip.** A list read rate whose asks load the shard object, which would sample them.

### `merge-budget-3`: REST polls compute, counts outlive the shard object, and `PrSync`'s first attempt gets its own invocation, 2026-11-12

Branch `merge-budget-3` closed the five items the lead left after `merge-budget-2` (`tests/am1/results/merge-budget-3.md`).

#### REST `GET pulls/{n}` asks for its merge as GraphQL's single-PR read does

**Decision.** `rest::get_pull_asks` returns the (base tip, head) a pull request's merge lacks from the same `gql::test_merge` GraphQL's `Node` reads, and the edge sends it with `ask_merges` after the reply: a writer's or an App's poll as `/demand?cause=get`, anyone else's as a list's count. Until the merge is recorded the reply keeps GHES 3.22's `mergeable: null` and `mergeable_state: "unknown"`; once it is, `mergeable` is the result and `mergeable_state` the lower case of `mergeStateStatus` (`gql::merge_state`), and a base that moves past the record makes both null and unknown again. `rebaseable` stays null.

**Why.** Agents and CI poll REST `pulls/{n}` far more than GraphQL, so with REST silent a polled PR past the budget stayed null until its next push, and a computed merge was never shown to a REST poller. That met A30's flip.

**Beat.** Counting REST polls without computing, which leaves the most common poller null forever. Rendering the merge in the webhook payload too, which GHES 3.22 sends as null [unverified: a GHES 3.22 webhook that carries a computed `mergeable`].

**Evidence.** `a_rest_pull_read_asks_for_the_merge_graphql_asks_for_and_keeps_mergeable_null`; planted bugs in `tests/budget/plant.py`; the hosted run's `restPollAnswers` and `getAsked`.

**Flip.** A GHES 3.22 capture whose `pulls/{n}` answers a different `mergeable_state` for a computed merge, such as `blocked` where GraphQL says `BLOCKED` but REST says `clean`.

#### The counts the gate reads are stored on the shard

**Decision.** The shard stores each network's `BudgetStats` under `budget:<network>` after every ask, coalesce, and merge result, and restores them before it serves `/status`, `/ask`, `/demand`, or a budget `/merge` (`MergeBudget::restored`, `Budgets::restore`). A restore never overwrites a network the object already budgets. The bucket itself is not stored: a restarted object starts it full, since it holds one second of refill and the object resets only after about 90 s idle.

**Why.** The shard object resets about 90 s after its last request, and with it the counts, so B0-35's row read them from Workers Logs.

**Beat.** Reporting the counts to the leader before the reset, which needs a write the object cannot schedule once it is idle. Logs alone, which the lead ruled out.

**Evidence.** `a_restarted_shard_goes_on_from_its_stored_counts_and_counts_merges_in_line`. Hosted, repo 65553's counts (973 computed, 1,154 GET and 1,846 list asks) read the same 127 s after the last request and after a site day reset every object, with `vcpu` taken from the new `SHARD_INSTANCE` [measured, `tests/am1/results/merge-budget-3.md`].

**Flip.** A store per ask that loads the shard object at journey 9's GET rate, which would batch the writes per second. Or a shard object reset while it is busy, which would grant a full bucket mid-load and need the bucket stored too.

#### `PrSync`'s first attempt runs in a new invocation through the site's own service binding

**Decision.** The leader's alarm sends each `PrSync` batch's first attempt to `POST /prsync/first` on its own site script through the `SITE` service binding, which runs the handler under `wait_until` in a new invocation. Re-sends still go through the queue. Under `test-faults`, `/bench_seed {"prsyncFirstFor": ms}` cuts the first attempt's deadline, so its abandoned merges and the queue's re-sends overlap.

**Why.** Run in the leader's isolate, every call the handler made was charged to the member's newest request. Each test merge's shard read back from the member (`/read`, `/entries`), so each merge made that request one hop deeper, and a cold shard's catch-up failed its eighth `/merge` with "Subrequest depth limit exceeded" (traces 62b928c0, 77603e9e, b3ec102d). A probe Worker reproduced it: a Durable Object looping cross-region calls that call back failed at the eighth in 3 of 3 runs, and the same loop through a self service binding made 20 in 3 of 3.

**Beat.** Sending first attempts through the queue, which `2026-11-05` rejected for its latency. Reusing one stub per batch, which passed the probe but depends on how the runtime picks a request to charge.

**Evidence.** The traces and probe above, and two hosted runs on cold shards with 0 depth errors where merge-budget-2's logged 16 and 4 [measured, `tests/am1/results/merge-budget-3.md`]. A service binding call is not billed as a request [doc, Workers pricing], so cost does not change.

**Flip.** A runtime that charges a Durable Object's calls to the request that started them, which would let the handler run in the isolate again.

### `merge-budget-4`: merge waits are measured, a late older-base merge no longer replaces the current one, and the shard size is read, 2026-11-12

Branch `merge-budget-4` closed the three items the lead left after `merge-budget-3` (`tests/am1/results/merge-budget-4.md`).

#### A test merge at an older base never replaces the head's merge at the base tip

**Decision.** `PrSync::decide_sync` skips a `TestMerged` whose head already has a recorded merge when its base is not the base branch's tip. A merge of a new head records as before, even at an older tip, and a merge at the tip replaces any older one.

**Why.** `merge-budget-3`'s run 2 was thought to commit each batch twice, its abandoned first attempt and its re-send, so the duplicates slowed the leader. The leader's stored log says otherwise. No request id is at two indexes, and no (PR, head, base) was recorded twice, because content-keyed request ids and `decide_sync`'s `recorded` check already make the commit idempotent per (PR, head, base). What the log shows is a different fault. In run 1, 32 `TestMerged` events recorded a merge at an older base over the same head's merge at a newer one: a slow merge planned before a base move committed after its faster re-send. Each put `mergeable` back to null and the merge ref on a stale base until the next push.

**Beat.** Keying commits per (PR, head, base) with an ack-without-entry path, which the lead proposed for duplicates the log does not show. Ordering merges by base in the shard, which cannot see what the leader has committed.

**Evidence.** The stored logs of repos 65553 and 65554, and the hosted run's 0 such merges on repo 65559 [measured, `tests/am1/results/merge-budget-4.md`]. The container sizes are read from the Containers API: run 1's shard ran on 4 vCPU, 12 GiB, 20 GB. `an_older_base_merge_committed_late_does_not_replace_the_current_one`, and planted bugs in `tests/budget/plant.py`.

**Flip.** A base branch that moves backward, such as a force-push to an older commit, which would make the tip's merge the older one. The check compares with the tip, not ancestry, so it already holds then.

#### A28's flip, measured: merges do not wait past GET_COALESCE_MS

**Decision.** `BudgetStats` counts each admitted merge's wait, from its ask to its start, in buckets with `GET_COALESCE_MS` as an edge. GETs keep coalescing on the clock.

**Why.** With first attempts cut to 3 s, 68 of them given up and up to 44 merges in line at once, 1,993 merges waited p99 5.8 s and at most 5.8 s, a fifth of the 30 s window, so a GET's slot never expired while its merge was still queued.

**Beat.** Coalescing on the queue, so a GET joins the pending merge, which the lead proposed in case waits passed the window. They did not.

**Evidence.** `tests/am1/results/merge-budget-4.md` [measured]; `merge_waits_are_counted_in_buckets_and_read_against_the_coalescing_window`.

**Flip.** A merge wait past `GET_COALESCE_MS` (`waits_over`), such as a network whose catch-ups are slow, which would coalesce on the queue.

### B0-30 fails at `CHILD_GROUP_AT`, and the substitution is withdrawn, 2026-11-11

B0-30 ran on `graft-m1` with pushes off from simulated day 1.34 and failed on its fail line at simulated day 27.52 (`tests/am1/results/b0-30.md`; `build/BUILD-PLAN.md`, AM1-WP6, B0-30).

**Decision.** The operator's direction of 2026-11-11: every LM-1 row passes as written, with no substitutions. The 2026-11-10 decision "B0-30 counts for LM-1 as substituted" (metadata half only, push half replaced) is withdrawn, and the LM-1 gate text in BUILD-PLAN and AGENT-LAYER no longer carries it. B0-30 is rerun as written, 30 days of journey 9 with pushes on, by thread "B0-30: MetaSplit child group, archive stall, full 30-day rerun" on branch `b030-split`, which builds the metadata child group (G6) and fixes the archive stall: a promotion's `/validate` stays on the network's first shard, nothing is forwarded to a shard with no cursor for its disk, and an archive run covers a promotion (G7). Thread "hot-shards" builds the push half, AM1-WP1's follow-up "Git cache shards on a hot group with a multi-GB log", whose gate is unchanged.

**Why.** The substitution's premise was that the metadata half answers B0-30's storage question on its own. The run disproved it: archiving stalled on the git cache shard, the same defect the push half was set aside for, and the members crossed `CHILD_GROUP_AT` with nothing to take the tier. A pass on half the row would certify neither the split nor the archive under pack churn, which are what the row exists to test.

**Beat.** Counting the metadata half as passed up to the crossing, which hides that no child group exists. Moving `CHILD_GROUP_AT` up to fit the measured growth, which the 10 GB object limit leaves no room for and which the stall, not the tier, drove. Waiting for M7-WP4 in order, which leaves LM-1 open on a row that a hot group reaches in about a month.

**Evidence.** `tests/am1/results/b0-30.md` [measured]: site 0 read 8,591,708,160 B at 23:09:41Z on 2026-11-11 against 8,589,934,592 B; `SQLITE_FULL` 0 throughout; the final verify read 81 of 81 sampled archived ranges from both buckets and resolved 10 of 10 `RestoredTo` lookups; SQLite grew 0.0585 GB a simulated day while archiving kept up, projecting about 8.21 GB at day 30 [inference, linear], and 0.184 GB a day during the stall; from about 17:13Z `fanout` at k = 6 forwarded the archive promotion's validate to cold pool shards, and site a's promotion validate median went from 0.17 s at 16Z to 7,551 s at 21Z. Cost $64.90.

**Flip.** None that restores a substitution without a new operator decision. The rerun's result decides B0-30.

### Dependency reads overlap the entry in flight, 2026-11-12

**Decision.** The leader may freeze the next entry's command set while one entry is in flight, once that entry has been in flight its mean round less one mean dependency read, and ask for the batch's dependency reads then (`crates/raft/src/member/leader.rs`, `cut_early`). The batch is decided only after the entry in flight committed and applied, on that fold, with exactly the chains `deps()` names on it: a chain the applied entry newly names is read then, and a read chain it no longer names is dropped (`decide_held`). A command admitted after the read was asked waits for the entry after. Dependency ids are unique per member, not per leadership, a transfer drops the held reads, and id blocks and recorded entries still go first. `Out::deps_used` names the entry each read decided, for the host's `deps_log`. DESIGN's write path now says a revocation or suspension "binds every command whose entry's read arrived after it" instead of "every command decided after it".

**Why.** B0-29 needs leader commit p99 at most 600 ms at 500 agents, and the last hosted split had commit p99 930 to 1,163 ms with `queueMs` p99 599 to 872, `depsMs` 239 to 432, and `roundMs` 304 to 356 (leader `/stats`). With the read started only after the entry in flight committed, a command waited one round, its entry's read, and its own round. Overlapping the read with the round in flight takes min(round, read) off that path while keeping one entry in flight and `decide` on the fold plus the entry's earlier commands and the reads made once for it through `fresh`.

**Beat.** Caching dependency folds across entries, which the write path's "read once per entry through `fresh`" forbids and which would break BF-2, BF-15, and BF-27. Cutting the next batch as soon as the entry in flight is appended, which holds out every command admitted during the round and gives back the wait it saves [inference]. Deciding the batch when its reads arrive, on the fold before the entry in flight applied, which breaks "one entry in flight" (the planted bug fails four of the new tests).

**Evidence.** `crates/raft/tests/deps.rs`: seven tests, each failing on one planted bug (deciding when the reads arrive, passing stale extra reads, no re-read for a changed chain, ids reset per leadership, a post-cut command joining the early batch, a transfer keeping its reads, two entries in flight). The simulator now sends half its puts with a dependency the host answers 0 to 400 ms late, checks one entry in flight per leader and that every command an entry decided on reads was admitted before those reads were asked, and the 2,000-seed sweep and Porcupine pass on its histories [measured, simulator]. The latency gain on Cloudflare is unmeasured: the hosted B0-29 rerun decides it.

**Flip.** A hosted B0-29 rerun where commit p99 does not fall by about min(round, read) p99, or where two reads in flight per group drive the `Authz` leader's dep read p99 up by more than it saves, which would return to reading after commit; or a revocation that must bind commands admitted before it within less than one round, which the narrowed wording no longer promises.

### A metadata split runs in band on the parent leader, keeps every commit on the parent, and reads the child narrowly (2026-11-12)

This supersedes "A metadata split commits on the parent, and the parent keeps the feed" where the two differ. Branch `b030-split` builds it for B0-30's rerun (`build/BUILD-PLAN.md`, M7-WP4).

**Superseded in part, 2026-11-13.** See "A split moves only the text `decide` never reads": no `MetaHalf`, no `dep_keys` reads, and no `CHILD_LOG_MAX`.

**Decision.** The parent leader's alarm runs the split's steps, with no `MetaSplitter` Workflow: `MetaSplit` at `SPLIT_AT` of `databaseSize`, a metadata snapshot at an index p, `MetaSeed { at: p, key }` on the parent, the `RepoMeta(repo)` group created on the parent's placement record with the same `MetaSeed`, the child seeded from that snapshot, then each committed parent entry recorded on the child as an ordered `MetaHalf`, and `MetaDropped` on the parent once the child is loaded and caught up. Every command, metadata-only ones included, still commits on the parent, so no write gets 503 and nothing answers `Moved`. After `MetaDropped` a command that reads the metadata tier lists `RepoMeta(repo)` in `deps` and names the keys it reads, and the host reads those rows through the child leader's `fresh`. The child decides nothing, so it needs no core mirror. `RestoredTo`, the feed, consumer cursors, and ids stay on the parent.

**Why.** The operator vetoed a 503 for both-tier commands after a split and asked for narrow `deps` reads of `RepoMeta` instead (2026-11-12, relayed by the B0-30 lead). Committing every command on the parent keeps one commit point and one log for consumers, the feed, and `RestoredTo`, so the split changes no cursor and needs no edge routing. A child that decides nothing replays the parent's halves in order, so it holds the parent's rows after entry k with no core state of its own. Running the steps on the leader's alarm needs no new Workflow, Durable Object class, or failure domain.

**Beat.** The 503 window from `MetaSplit` to `MetaSeed` and `Moved` routing (the earlier decision), vetoed. A core mirror on the child through `MetaHalf` so the child could decide metadata-only commands, which the operator approved but which no command needs once every command commits on the parent. A `MetaSplitter` Workflow, which adds the Workflows script to B0-30's site day for steps the alarm already sequences.

**Evidence.** `design/DESIGN.md`, Shape, "Chains" (`CHILD_GROUP_AT`). Seven both-tier commands read metadata in `decide`, each by at most three keyed rows or one bounded scan, and none keyed by another metadata read (code read of `crates/git/src/issue_ops.rs` on `b030-split`). workerd's `getDatabaseSize` leaves out freelist pages, so deleting the parent's rows lowers the reading at once [doc: workerd `src/workerd/api/sql.c++`, `getDatabaseSize`; commit 7354f512c]. The tests and the hosted split on `repo:1048586` are in `tests/am1/results/b030-split.md`.

**Flip.** A read path the renderers need that cannot name its keys before the entry commits, such as an unbounded GraphQL selection, measured slow when fetched row by row from the child; or a parent whose core tier alone nears `CHILD_GROUP_AT`, which needs a second split axis rather than this one.

### A split moves only the text `decide` never reads (2026-11-13)

This supersedes "A metadata split runs in band on the parent leader, keeps every commit on the parent, and reads the child narrowly" where the two differ: no `MetaHalf`, no `dep_keys` reads, no `CHILD_LOG_MAX`. The in-band steps on the parent leader's alarm, one commit point on the parent, no 503, and no `MetaSplitter` Workflow stand. Branch `b030-split` builds it (`workers/site/src/split.rs`, `crates/git/src/split.rs`).

**Decision.** At `SPLIT_AT` of `databaseSize` the parent records `MetaSplit`, then `MetaSeed { at: p, key }` at its newest committed metadata snapshot p, creates `repo-meta:<id>` on its own placement record, and records the same `MetaSeed` there. Each child member restores that snapshot into its metadata tier on its alarm; no entry writes that tier afterwards, and the child decides nothing. Once the child leader reports its tier restored, the parent records `MetaDropped { seed: p }` and then `MetaSlim` entries, each over `SLIM_ROWS` = 512 rows, until one pass ends. A row last written at or before p loses the text no `decide` reads: issue, comment, review and release bodies, and a task's prompt and summary, replaced by `SLIMMED`. `provenance` and `path_set` rows, which no read serves, are deleted. Every table and column `decide` reads stays on the parent. Every read and reply a parent member builds fills slimmed fields from the child's row. The member reads that row from any child member whose tier is loaded, then keeps it per isolate, since it never changes. An answer that missed one is built again after the read. The child's log holds only its bootstrap, a `Noop` a term, and `MetaSeed`, so it is neither snapshotted, archived, nor truncated; the parent keeps the seed's snapshot object past retention.

**Why.** The operator chose, on 2026-11-13, the option that needs no raft change and no new `IdentityDep`, provided the parent stays under `SPLIT_AT` through day 30. The size check, from B0-30's first run on `graft-m1`, smallest member [measured, `tests/am1/raw/b030-monitor.jsonl`]: SQLite besides the log and `request` rows grew from 4.02 GB to 5.60 GB in 27 simulated days, about 0.0585 GB a day; the log held at 2.15 GB while archiving kept up, and `request` rows at about 0.27 GB. The member crossed `SPLIT_AT` (7.52 GB) near day 12. The slim pass removes the 1,340,000 seeded 1,800-byte comment bodies (about 2.4 GB) and about 21 MB a day of PR bodies and provenance written before the seed. That leaves the parent at about 5.8 + 2.15 + 0.27 − 2.65 ≈ 5.6 GB at day 30 [inference, linear], under `SPLIT_AT`, and `databaseSize` leaves out the freed pages [doc: workerd `src/workerd/api/sql.c++`, `getDatabaseSize`]. Text written after the seed stays on the parent, so the child needs no entry after its seed, which removes `MetaHalf`'s ordering, its loss and doubling cases, and the child's log growth. The operator's fallback for a child that cannot use `Promoted`, "a snapshot-and-truncate rule like the `Register` family's", is not needed. The child's log does not grow, and the `Register` family has no snapshot or truncate rule in `workers/site/src/family.rs` to copy (it is a test register).

**Beat.** `MetaHalf` with `dep_keys` reads through `fresh` (the superseded decision), which needs every command that reads metadata to list the child in `deps` and a host read per entry. An `IdentityDep` variant carrying opaque row bytes, the operator's fallback if the size check failed; it did not. Moving whole tables, which `decide` reads (issue state, comment authors and threading, review verdicts).

**Evidence.** `crates/git/tests/split.rs` (10 tests) and `workers/site/src/split.rs` tests (2). `tests/am1/b030split/plant.py`: 15 planted bugs, each caught. Hosted: `tests/am1/results/b030-split.md`.

**Flip.** A parent whose slimmed tier still nears `CHILD_GROUP_AT`, because the text written after the seed or the tables `decide` reads outgrow it. That needs a second pass with a newer seed, or the `IdentityDep` variant. Or a route that serves search over the slimmed text, which would need FTS5 rows on the child.

### A leader paces its own background writes, and a leaderless voter with such work due acts after 20 s (2026-11-14)

**Decision.** A leader's own background entries wait `PACE` (2) times as long as the entry took to commit before the next, at most `PACE_MAX` (10 s) (`workers/site/src/host.rs`, `paced`). These are the split's `MetaSlim` and the archive drain's `Promoted` and `Archived`. A voter acts as one edge `suspect` report would make it act (`Host::keep_led`) once it has seen no leader for 2 x `ELECTION_AFTER` (20 s), and its group has leader-only background work due. That work is a split step, or an archive run it led that stopped with work left in the last 10 minutes. "Seen no leader" means it neither leads nor holds a promise, so the 20 s run from the last append that renewed one: `ELECTION_AFTER` past its lapse. If it names no leader, it campaigns. If it still names one, it asks that leader for a read index. A live leader answers and nothing changes, since an idle leader sends no appends and a lapsed promise alone is no evidence. A leader that does not lead answers none, and one that does not answer in `CAMPAIGN_DEADLINE` is forgotten, and the member campaigns (F67's read-index rule). It acts again at most every `ELECTION_AFTER`. The check runs on the voter's alarm, every 5 s while the work is due. It is site code and changes nothing in `crates/raft`.

**Why.** On the smoke split of `repo:1048586`, `MetaSlim` went back to back, one every 0.45 s. Followers' `/raft` calls then ran 400 to 550 ms with 100 to 200 ms of CPU, and a leader's 20 s slim run took up to 7.5 s of CPU. There were 110 `STEP_DOWN` trips in 2.5 h, 104 of them `CommitErrors` and 5 `LoopLag` [measured, `tests/am1/results/b030-split.md`, "Smoke 1"]. Most trips found no qualifying target and held (`to: None`), so check-quorum ended the leadership. Campaigns are request-driven, and B0-30's load writes through the leader with no edge, so the group stayed leaderless until an external keeper campaigned, for up to 39 minutes. The keeper is a substitution and is out of the rerun. The design's bound for a voter that sees no leader is 20 s ("Downtime", Gray region: "A voter that has seen no leader for 20 s waives the reset rule"; Shape, `suspect`).

**Beat.** A fixed spacing such as one entry a second: it is too slow when an entry commits in 100 ms and too fast when one takes 3 s. A campaign from `tick` on every leaderless member, which Raft does and this design gave up on purpose so an idle group bills nothing (Shape, `suspect`; `workers/site/src/host.rs`, header). A raft-crate change that ends a held leadership later: the trip itself is right on a member that cannot keep up.

**Evidence.** `workers/site/src/host.rs` tests `a_leaderless_voter_with_work_due_acts_after_the_bound_only` and `paced_writes_wait_twice_their_commit_up_to_the_cap`; `workers/site/src/split.rs` test `a_split_needs_its_leader_only_while_a_step_is_due`; plants in `tests/am1/b030split/plant.py`. Hosted: smoke 2 in `tests/am1/results/b030-split.md`.

**Flip.** `STEP_DOWN` trips during a paced slim or drain at the rate seen unpaced. A voter's own campaign that deposes a leader still serving: it campaigns only once its promise has lapsed, so this would mean promises lapse under a live leader. Or a family whose leader-only work is itself request-driven, which needs no such check.

### A git cache shard catches up a page at a time, in the background, and resumes where it stopped (2026-11-12)

**Decision.** A shard's catch-up and rehydrate (`crates/git/src/catchup.rs`) read the log one `/entries` page at a time, the next page read while one is ingested, so a shard holds at most two pages. Catch-up saves its cursor after each page at the last index the page held. Rehydrate saves its place per page, with its disk id, and asks the container which packs it already holds (`/have`), so a run cut off on the same disk resumes and never starts a new repository. Stored packs stream from the bucket to the container. A pack whose bases or named objects the disk lacks answers 409, drops the cursor, and the next call rehydrates. The catch-up runs inside the shard object (`workers/site/src/shard.rs`, `Jobs`), not inside its caller: a caller waits at most its `wait=<s>` and then gets 503 `warming`, the object's alarm restarts a run a reset cut off, and a pool shard with no cursor answers a forwarded push `warming` at once while the first shard validates it. The container's `/load` leaves out derived CPU (ingest, ref writes, promotion packs, init, and the archive's own `derived=1` validate). A whole-tier `{"rows": true}` read stops at `ROWS_BYTES_MAX` (8 MiB) as well as `ROWS_MAX` rows.

**Why.** On B0-30's hot group, whose log held about 2.5 GB of inline packs above the last `Promoted`, catch-up and rehydrate read whole gaps into memory, outlasted the push's 180 s deadline and the archive alarm's `RUN_FOR`, and a cut-off rehydrate restarted from nothing, so pushes were turned off at simulated day 1.34. A cursor moved to the end of a range a new leader answered short skipped an entry for good (blocker 12). `fanout` stepped k up while cold pool shards rehydrated, which sent more pushes to more cold shards (A31). A row-only budget scanned 50,000 rows of a 4 GB tier only to refuse it, at about 302 s of CPU (blocker 5).

**Beat.** Fixed pieces of `CATCH_UP_ENTRIES` entries, which bound memory by entry count, not bytes, and restart a rehydrate's tail from the last `Promoted`. One concatenated pack per page, which `index-pack` refuses on duplicate objects. Waiting inside the caller with a longer deadline, which every caller would need and an isolate reset still loses. Counting all CPU and raising `FANOUT_UP`, which still counts work that runs on every shard whatever k is.

**Evidence.** `tests/am1/results/hot-shards.md`: in-memory tests in `crates/git/tests/catchup.rs`, `crates/api/tests/surfaces.rs` (`an_oversized_tier_read_stops_at_its_byte_budget`), and `containers/git/server/load_test.go`, with the planted bugs of `tests/am1/hotshards/plant.py`; the hosted run on `graft-m3c` there.

**Flip.** A Workers runtime that lets a Durable Object run past its last request without `waitUntil` or an alarm, which would let the catch-up stay in its caller; or a hosted run where two pages of `ENTRIES_PAGE_BYTES` with a pack in flight still pass the isolate's 128 MB.

### A push no git cache shard could check is retryable, and a hot network's shards are kept caught up (2026-11-13)

**Decision.** A shard answers a failed `/validate` 422 only for its container's own 4xx (the pack's answer, the same on every shard), and 503 for a broken catch-up, a stopped container or a lost call (`graftyard_git::validate_status`). The edge answers a push no shard checked 503 `Retry-After: 2` before any byte of a git result. A leader catches up every Full site's first shard of its network at once and every `KEEP_WARM_MS` (15 s) while it applies entries. The archive's promotion runs in a stateless invocation of the site script, not in the leader member. Repacks search no deltas and write a commit-graph.

**Why.** At `hot-shards-2`'s home-shard kill, 23 pushes in flight were refused `unpacker error`, which git reports as a failed push and a client does not retry. At `hot-shards-3`'s, the same pushes got 503, but each waited 137 s, because the other sites' shards were caught up only by pushes and sat 1,100 and 1,900 entries behind. A leader member that drove the promotion's shard calls charged each to its newest request, one the shard's own `/entries` reads had made, and failed "Subrequest depth" (merge-budget-4's trace `8c049c94`). B0-29's push p99 climbed within each run because validate's `--not --all` walks read every ref's tip from the packs, and a repack's delta search held the network's lock for seconds.

**Beat.** Retrying inside the edge on 5xx, which would hold a push past its 180 s on a shard that is rehydrating. Keeping only the leader's own site warm, the old rule, cheaper by two catch-up calls each 15 s. A once-a-day full repack to drop pack count, which does not remove the per-tip read. A pool shard check on every forward rather than a 30 s skip, which pays a call a push.

**Evidence.** `tests/am1/results/hot-shards-3.md`: A31's run (k stayed 1 through three cold rehydrates), j9g on standard-4 (push p99 4.70 s, 16 in-flight 503s, fsck clean on three shards, 0 depth errors), and the local B0-29-like runs (CPU 19.5 to 25.3 CPU-s/min down to 12.9 to 16.3; validate p99 up to 1,948 ms down to under 140 ms). Plants 38 to 46.

**Flip.** A hosted kill where in-flight pushes still wait past the edge's waits with every site warm, which would mean the retry belongs in the edge; a repository whose disk grows past `DISK_USE_PCT` from `--window=0`'s lost deltas; or a Workers runtime that charges an object's calls to the request that started them, which would let the promotion run in the member again.

### A shard's repack searches deltas, waits for no lock but fsck waits for it, and a busy member serves 1 MB local pages (2026-11-13)

**Decision.** `runRepack` runs `git repack -d --geometric=2` with git's default delta window, and the split commit-graph after it stays. `/fsck` waits for a running repack, as evict does. A member's local `/entries` page is at most `ENTRIES_BUSY_PAGE_BYTES` (1 MB) while `ENTRIES_BUSY_READS` (1) or more other reads are in flight, and up to `ENTRIES_LOCAL_PAGE_BYTES_MAX` (4 MB) on an idle member.

**Why.** `--window=0` was added while a repack held the network's lock, and its 1 to 5 s delta search queued every validate. LM-1 round 7 took the repack out of the lock, so the search no longer delays anything, and a local B0-29-like A/B on the merged container showed the same CPU and validate p99 either way. Meanwhile `--window=0` kept every pushed version of a file whole: shards held 2.07 times the pushed bytes of real history and 13 times journey 9's. With the repack out of the lock, it could delete the packs it rolled up under a running fsck, and site c's fsck answered `ok: false` just after a rehydrate's repack. A 4 MB page built beside a busy member's other reads put site b's member near its 128 MB, and it trapped in F47's shape.

**Beat.** Keeping `--window=0` with a nightly full repack, which still grows disk between nights and costs a second repack path. Taking the network's write lock for fsck, which would block every validate and fetch for the fsck's seconds. A page cap from the member's wasm memory, which only grows and so would cap every page after the first spike.

**Evidence.** `tests/am1/results/hot-shards-3.md` ("`--window=0` disk growth", "Site b's trap", the j9w fsck) and `tests/am1/results/hot-shards-4.md`. `TestRefsRepacksPastRepackAt` (deltas across packs), `TestRepackHoldsNoNetworkLock` (fsck waits, then answers ok), `only_a_local_entries_read_gets_a_larger_page`, and plants 56 to 58.

**Flip.** A hosted run where the default-window repack's CPU, which is derived and outside `NetworkLoad`, starves validates on a standard-4; a member trap at a 1 MB page under load; or 1 MB pages that make a busy shard's catch-up fall behind journey 9's push rate.

### A shard ingests a run of inline packs in one call, as one `index-pack` split on failure (2026-11-13)

**Decision.** A catch-up's and a rehydrate's inline packs go to the shard's container in one `POST /ingest-batch` per run of up to `INGEST_BATCH_BYTES` (4 MB, `crates/git/src/catchup.rs`), one answer per pack, `done` or `later`. The container concatenates the run's packs not already ingested into one pack and indexes it with one `index-pack --strict --fix-thin`. Each pack's objects are recorded under its own SHA-256 for `/promote`, found by their offsets in the joined pack. When the joined pack fails, the run is split in halves down to one pack, which is ingested as `/ingest` ingests it, so two packs that carry the same object (which `--strict` refuses in one pack) and a pack whose base is missing each end up on their own. A stored pack still streams alone from its bucket. A container whose image has no `/ingest-batch` (404) gets one `/ingest` a pack.

**Why.** After the multi-pack-index, a 2.05 GB cold rehydrate still took 20 to 22 min hosted, its tail 66 to 72 ms a pack against about 10 ms of `index-pack` locally (`tests/am1/results/hot-shards-2.md`). The difference is one shard-object-to-container call per pack plus `index-pack`'s per-run setup. One `index-pack` per batch removes both for small packs: locally, on one CPU, 9,000 packs of 20 KB took 4.9 s in 4 MB batches, from 94.3 s one call a pack.

**Beat.** A dedupe pass that rewrites a pack without its duplicate objects, which would need a pack parser in the container and changes the bytes a pack's SHA-256 names. One call carrying many packs but one `index-pack` each, which saves the round trip but not `index-pack`'s setup. Larger `/entries` pages, which raise the shard's held memory.

**Evidence.** `tests/am1/results/rehydrate-batch.md`: `containers/git/server/batch_test.go`, `crates/git/tests/catchup.rs` (`a_rehydrate_ingests_runs_of_inline_packs_a_call_at_a_time`), the planted bugs in `tests/am1/hotshards/plant.py`, the local benchmark `tests/am1/hotshards/bench/batchserve.py`, and the hosted cold rehydrates on `graft-m3f`.

**Flip.** A hosted run where the split path dominates (many batches that fail whole), or where one batch's body pushes the shard object past its memory.

### The edge serves the web app: `/_t/ui`, an unsigned session cookie behind a header, and the inlined first route (2026-11-13)

**Decision.** The edge carries the web build as a Workers static-assets binding and serves `/assets/*` immutable and the HTML shell to a browser's `GET` of any page path nothing else claims, with the first route's reads rendered in process and inlined as `__graft_seed`. The private API lives under `/_t/ui/` behind the same authentication and read gate as the repository API; git-derived routes ask the git cache shard's `/ui/*` routes, and fold-derived ones (events, threads, merge requirements) render from the rows `mergeStateStatus` reads. Web sign-in checks a PBKDF2-HMAC-SHA256 hash on the `Identity` fold, commits `SignIn { method: Password }`, and sets `__Host-gy_session=<account>.<session>`, which counts only on a request that also sends `x-graftyard-web: 1` and only while the fold lists the session and `sessions_not_before` allows it. GraphQL from the app goes as persisted documents by hash (`web/src/api/persisted.json`, embedded in the edge) (`design/SHARED.md`, "Frontend and native app", "How the edge serves it").

**Why.** `/_t` is reserved, the shell answers only `text/html` `GET`s of paths no GHES route owns, the cookie needs a header no GHES client sends, and a persisted request is one with no `query`, so no GHES 3.22 reply changes. An unsigned cookie whose 80 random bits name a session the fold must hold needs no new secret, and so no rotation row or site-day-only secret change. Rendering the seed with the same renderers as the routes keeps the inlined data identical to what a refetch returns.

**Beat.** A signed (HMAC) cookie with a new edge secret, which adds a key to rotate for no gain while the fold is read anyway. Serving the pages' reads through new `/api/v3` routes, which would be GHES paths with no oracle for their private shapes. A second `ASSETS` Worker, which would add a failure domain.

**Evidence.** `crates/identity/src/password.rs` (RFC 7914 vectors), `workers/edge/src/ui.rs` and its tests, `crates/api/src/ui.rs`; BF-38's run against `graft-m4a` (`tests/m4/results/m4-wp5.md`).

**Flip.** A session read on every cookie request that measurably raises read latency (then cache it for `READ_CACHE`, as tokens are); a GHES client that sends `x-graftyard-web`; or a shell whose seed reads push first paint past a second round trip.

### The web app is TanStack Start with ReUI and HugeIcons, started on a mocked API (2026-11-08)

**Decision.** The web app (M4-WP5) is React 19 on TanStack Start in SPA mode, with TanStack Router, Query, and DB, shadcn/ui on Base UI, ReUI's free primitives and examples from the `@reui` registry, and HugeIcons' free icons (`FRAME.md`, "Operator decision, 2026-11-08, web stack"). It lives in `web/`. It starts now, before M4's backends, against a browser mock (MSW) whose handlers and the app's client share one set of types generated from the pinned `ghes-3.22.json` at `755ee33b`, pruned to the operations the pages read. Each page region owns one `queryOptions` and one error boundary; reads retry at most twice with jitter and mutations never retry.

**Why.** The operator chose the stack. SPA mode keeps the design's rule of no Node rendering tier, because the build is static assets and a prerendered shell, which the edge can serve and later fill with the first route's data. Generating both the client's and the mock's types from the oracle the edge is already checked against keeps the mock from drifting into shapes GHES never sends, so moving a page from the mock to the edge changes the base URL, not the code.

**Beat.** Primer React, the stack GitHub ships (`research/clients.md#62`-`#64`). Waiting for M4-WP3 before any page exists. Hand-written fixture types, which nothing checks against GHES 3.22.

**Evidence.** The operator's request of 2026-11-08. TanStack Start's SPA mode documentation (https://tanstack.com/start/latest/docs/framework/react/guide/spa-mode) [doc]. ReUI's free tier covers all component examples and primitives, and its Pro blocks and icons need a license (https://reui.io/llms.txt) [doc].

**Flip.** A Start release whose SPA build needs a server at run time; a page whose first paint misses BF-38 because the shell cannot carry the inlined data; or a ReUI free component relicensed as Pro.

### The second warm site sizes its shards for the conflict predictor (2026-10-30)

**Decision.** On a network's second warm site, k is the larger of the network's fan-out k and the shards the predictor needs at the measured mean overlap under `PREDICT_CPU`. `PREDICT_PAIRS` stays 8 per push as a cap.

**Why.** With git CPU measured, A1's network needs k = 1, which left the predictor 2 vCPU. M0-WP16 measured one shard saturating at A1's push rate, with push-to-verdict p95 364 s against L0-05's 30 s, while 7 shards passed at 6.8 s. A real fleet's pushes overlap 1.80 open refs on average, so the predictor needs 2.8 vCPU, and k = 2 on the second site holds it at about $83 a month.

**Beat.** Leaving k = 1, which fails L0-05. Sizing for 8 pairs a push, which would hold 7 shards for a load the replay never produced.

**Evidence.** `tests/m0/results/m0-wp16.md`, L0-05 and L0-06; `scripts/cost_agents.py --a1`.

**Flip.** A real fleet whose mean overlap set is well above 1.80, or a predictor run at k = 2 whose L0-05 p95 passes 30 s.

### L0-12's feed clause allows FEED_BATCH plus 2 s (2026-10-30)

**Decision.** L0-12 passes when 500 long-pollers see each event within FEED_BATCH plus 2 s (3 s) at p99 outside leader kills. `FEED_BATCH` stays 1 s. Made under the operator's delegation of 2026-10-30 (`FRAME.md`).

**Why.** A poll with `paths` returns FEED_BATCH after its first match, by design, so batching takes half of a 2 s budget before any delivery time. M0-WP12 measured p99 2.2 and 2.6 s for the two agent kinds outside kills, with clock offsets under 10 ms. Halving FEED_BATCH would double the poll rate on busy scopes, the load `FEED_HELD_MAX` and learners are sized against.

**Beat.** Cutting FEED_BATCH to 500 ms. Keeping a 2 s line that fails by construction.

**Evidence.** `tests/m0/results/m0-wp12.md`, L0-12 feed.

**Flip.** Agents that act on an overlap within 3 s, where a 1 s batch costs them conflicts.

#### LM-1 round 7: a member's ring starts at its first poll, and nothing is kept for it before

**Decision.** A member keeps no entry for its activity-feed ring until a poll reads the owner's `Authz`. That poll starts a ring that never served at the member's applied entry with the leases live there (`Ring::start`, as `Ring::skip` does for an install), so a cursor from before gets 410; a ring that already serves is left as it is. `lease::rows` decodes the rows its scan returns instead of reading each again.

**Why.** Two costs of the old waiting list, both measured. Fed one by one at the first poll, each waiting entry rebuilt the live-lease set from SQL in one synchronous turn: on L0-12 (`repo:131273`, 08:00:38Z on 2026-11-12) each of the 8 learners spent 15 to 46 s of CPU on its first poll, with about 1,070 entries and 500 leases waiting, and the voters sharing their isolates then answered "Requests queued for too long" at 13 to 36% CPU, and `STEP_DOWN` moved the leader at 08:01:39. And the list held a clone of every applied entry's events, inline packs included, up to FEED_RING's 10,000 entries, on every member no poll reached: the leader and the followers. At 500 agents (`repo:131276`, 180 s) the leader's wasm memory grew from 2 to 67 MB and a follower's from 2 to 40 MB, in step with the 4.2 agent pushes a second of 90 KB [measured, `tests/am1/results/lm1-memory.md`, round 7].

**Beat.** Feeding the backlog in slices with an await between them, which keeps both costs and only spreads the CPU. Bounding the list by bytes, which still holds packs on members that never serve.

**Evidence.** `a_first_poll_starts_the_ring_at_the_applied_entry_and_a_serving_ring_keeps_its_events` (`crates/git/tests/lease_gates.rs`); plants 66 to 68 in `tests/lm1mem/plant.py`.

**Flip.** A client that must see the events of entries applied before a member's first poll, which needs that member to have read the owner's `Authz` at join.

#### LM-1 round 7: a REST merge reads only its rows, as `mergePullRequest` does

**Decision.** `PUT /pulls/{n}/merge` reads the rows its plan needs by pull request number (`{"rows": {"mergeN": n}}`, `TierRows::merge_rows` with `MergeOf::Number`): the access rows, the pull request's `issue`, `pull`, and `pull_head`, and its base's `ref`. A follower or learner serves it by read-index once its metadata tier is loaded, as it serves `mergePullRequest`'s `{"merge": id}`. A member built before `mergeN` answers it with both whole tiers.

**Why.** BM-2 cut 4 narrowed only the GraphQL merge; the REST route kept reading both whole tiers. At journey 9 that is about a 1 MB packed dump parsed at the edge on every merge, and at 500 agents (`repo:131276`, 180 s) REST merges took 1.2 to 4.4 s at the edge (p99 4.4 s, against B0-29's 2 s), while the shard's merge plan took 68 to 1,232 ms [measured, `tests/am1/results/lm1-memory.md`, round 7].

**Beat.** Leaving the REST route on the whole tiers until `narrow-reads` serves every route narrowly, which keeps B0-29's merge line failing until then.

**Evidence.** `a_merge_plans_from_its_rows_as_from_both_tiers` (`crates/api/tests/surfaces.rs`) and `a_follower_serves_only_the_designs_reads` (`workers/site/src/fanout.rs`); plants 69 to 71 in `tests/lm1mem/plant.py`.

**Flip.** A REST merge reply that renders rows beyond these, which would need them added to `merge_rows`.

#### LM-1 round 7: a merge waits for a catch-up in flight only after it misses a commit

**Decision.** `merge_then_catch_up` merges first, waits for a catch-up in flight only when the container answers that a commit is missing, merges again, and catches up itself only if that misses too. It amends the BM-2 rule that the first merge plan awaits the catch-ups in flight ("BM-2 latency: repack the shards…", and `bm2-repack`): the wait was meant for a new leadership's `warm` catch-up, which the merge still waits for when it lacks a commit.

**Why.** Under journey 9 a validate's catch-up runs on the shard nearly all the time, so every merge waited for one first. At 500 agents (`repo:131278`, 180 s, after the REST merge read only its rows) the shard's merge waited 64 to 2,151 ms of its 175 to 2,750 ms, for commits it already held (no merge caught up), and REST merge p99 was 6.2 s against B0-29's 2 s [measured, `tests/am1/results/lm1-memory.md`, round 7, the `mergeSteps` shard timings].

**Beat.** Keeping the wait and making catch-ups shorter, which leaves every merge behind whatever catch-up is running.

**Evidence.** `a_merge_waits_for_the_catch_up_in_flight_instead_of_racing_it` (`crates/git/src/merge_cache.rs`): a merge whose commits are present answers in one poll while a catch-up runs, and one that misses still waits and does not catch up a second time; plants 72 and 73 in `tests/lm1mem/plant.py`.

**Flip.** Merges that miss on their first try often enough that the extra container call shows in merge latency.

### `merge-p99`: a merge's rows wait for no dump rebuild, and a REST write resolves its name beside its token, 2026-11-13

Branch `merge-p99` (`tests/am1/results/merge-p99.md`), toward B0-29's merge line (`merge:only` p99 at most 2 s at 500 agents).

#### A merge's rows read is never spaced as a dump rebuild

**Decision.** `Repo::heavy_wait` spaces only a whole-tier dump (`{"rows": true, "packed": true}`) by `DUMP_EVERY_MS` or `LEARNER_DUMP_EVERY_MS`. A merge's rows (`{"rows": {"merge": id}}`, `{"rows": {"mergeN": n}}`), which the edge also sends with `packed`, are answered at once from `read_json` and take no rebuild slot.

**Why.** The check keyed on `packed` alone, so a merge's few rows waited for, or were shed behind, a dump rebuild it never made, and each one let through claimed the slot the next whole-tier read needed. In LM-1's m19 smokes on `graft-m2b` the REST merge's tier read was its largest stage, p90 955 ms (250 agents) and 1,030 ms (500), max 2.2 and 1.4 s, and 1,715 ms p90 and 2.8 s max on the m19 B0-29 row [measured, `mergeSteps`], while the dep-free stages (`auth`, `merge_plan`, `commit`) stayed near 50, 150 to 400, and 500 to 700 ms. The spacing is the cause by the code path; no member log isolates it [inference].

**Beat.** A shorter `LEARNER_DUMP_EVERY_MS`, which spends learner CPU on dumps to shorten a wait merges should never have had.

**Evidence.** `a_merge_rows_read_never_waits_for_a_dump_rebuild` (`workers/site/src/fanout.rs`) fails on the old check; plants in `tests/lm1mem/plant.py`.

**Flip.** A merge rows answer large enough that rebuilding it per read costs a member what a dump did.

#### A REST write reads its token and `o/r` at once

**Decision.** `rest_repo` resolves the repository name beside `authenticate` for every method, as it did for `GET` only. A write still reads both fresh (`read_class` stays false).

**Why.** A write resolved after its token, outside every timed stage: 135 to 705 ms of each m19 REST merge was untimed (`total` less `auth`, the longer of `tier` and `authz`, `merge_plan`, and `commit`) [measured]. Running it beside the token takes that serial step off every REST write [inference until the hosted split shows the gap gone].

**Beat.** Keeping the order for writes, which buys nothing: neither read needs the other's answer.

**Evidence.** The edge's per-request `server-timing` (`resolve` is now a stage of every REST request) and `a1.mjs`'s `mergeSplit`.

**Flip.** A write path that must authenticate before it may learn whether a repository exists, which `resolve` does not reveal to the caller.

#### Test timing headers are per request

**Decision.** A test install's `x-graft-trace` and `x-graft-shard-timing` live in the request's `Timing`, not in isolate-wide thread-locals.

**Why.** 54 of 58 merges in the m19 smokes and 68 of 70 on the m19 B0-29 row carried no shard timing [measured], so the shard's share of a merge was not measured. Concurrent requests in one isolate taking each other's notes fits: any request's reply cleared the isolate's note [inference from code].

**Beat.** Nothing; LM-1 round 7 moved `server-timing` into the request for the same reason (`236eca5b`).

**Evidence.** The hosted `mergeSplit` carries `shard:*` stages for each merge.

**Flip.** A note that must outlive its request, such as a shard timing reported after the reply, which would need its own channel.

#### A merge's rows are read on the leader

**Decision.** `tier_rows` sends a merge's rows (`MergeOf`) to the repository's leader by `call_opt`; whole-tier reads keep `read_spread`. It amends "F65 follow-up" and "LM-1 round 7: a REST merge reads only its rows", which sent merge rows to followers and learners.

**Why.** With the rebuild spacing gone, the merge's tier read still ran p50 224 to 300 ms and p99 1.0 to 1.4 s on `graft-m3f` at 500 agents [measured, `tests/am1/results/merge-p99.md`]: a learner or follower answers only after a read-index round to the leader, from as far as apac, and after it has applied that index. The leader answers from its lease in its own turn. The F65 move was for 200 API reads a second; merges are 10 a minute, and the merge's commit goes to the same leader right after.

**Beat.** Keeping merge rows on learners and placing one near the leader, which still waits for a read-index round and costs a learner.

**Evidence.** The hosted `mergeSplit` `tier` stage before and after.

**Flip.** Merges at a rate whose reads show in the leader's `/read` share, or a leader shedding them (`READS_MAX`), which would move them back to followers.

#### A REST merge reads ahead on its cached repository

**Decision.** `PUT pulls/{n}/merge` takes its isolate's cached login, `Placement`, and `Authz` (`resolve_cached`, entries at most `READ_CACHE` old), and while its own fresh `resolve` runs, reads its merge rows and the owner's `Authz` fresh for that repository. It keeps them only if the resolve answers the same repository, owner, and placement record (`kept_ahead`), and reads again otherwise. Every read it keeps is a fresh one started after the request arrived; only which repository to read is a guess.

**Why.** After the two fixes above, the write's fresh `resolve` was the merge's longest serial stage, p50 381 to 452 ms and p99 861 to 884 ms on `graft-m3f`, and the tier and `Authz` reads waited for it [measured, `tests/am1/results/merge-p99.md`]. Run beside it, the two smokes' merges would have run p99 1.6 and 2.0 s [inference from the same stages].

**Beat.** Reading a write's `o/r` from the cache outright, which lets a renamed or moved repository take another's merge for up to `READ_CACHE`, against SHARED's write path that reads identity fresh.

**Evidence.** `a_write_keeps_its_ahead_reads_only_for_the_repository_it_resolved` (`workers/edge/src/api.rs`); plants in `tests/lm1mem/plant.py`; the hosted `mergeSplit`'s `tier_ahead` and `authz_ahead` stages.

**Flip.** Guesses that miss often enough that the wasted reads show on the leader, or a repository renamed so often that merges pay the second read.

#### A REST merge reads the owner's `Authz` on its leader

**Decision.** For `PUT pulls/{n}/merge` (`api::rest_merge`, `Ctx::leader_reads`), the owner's `Authz` reads, both the fresh name lookup in `resolve` and the fold read ahead or in `read_repo`, go to the `Authz` group's leader by `call_opt`. Every other request keeps `read_spread`. It amends "F65 follow-up: the edge reads an owner's `Authz` by read-index" for this one route, as "A merge's rows are read on the leader" amends F65.

**Why.** With the rows on the repository's leader and read ahead, a REST merge's longest serial stages were `resolve` (p50 398 to 531 ms, p99 784 to 1,033 ms) and `authz_ahead` (p50 451 to 489 ms), both `Authz` reads by read-index through far members, while the rows read on a leader took 30 to 47 ms. The 600 s B0-29 row on `graft-m3f` missed at p99 2,517 ms [measured, `tests/am1/results/merge-p99.md`]. The 123/s that overloaded the `Authz` leader were API reads; merges add about 20 reads a minute [inference].

**Beat.** Caching the `Authz` fold for writes, which SHARED's write path rules out (the fold names who may merge), and a learner near the edge, which still waits a read-index round.

**Evidence.** `only_a_rest_merge_reads_on_leaders` (`workers/edge/src/api.rs`); plant 81 in `tests/lm1mem/plant.py`. On the 600 s B0-29 row at 500 agents on `graft-m3f`, `resolve` went from p50 530 / p99 1,033 ms to 118 / 306 ms and `authz_ahead` from p50 463 to 50 ms, and `merge:only` p99 from 2,517 to 1,571 ms, with no leader change [measured, `tests/am1/results/merge-p99.md`].

**Flip.** An `Authz` leader whose `/read` share or `READS_MAX` sheds show merge reads, or merges at a rate near the API reads that moved off it.

### The per-owner pool budget is a lease ledger in each site's first pool shard, 2026-11-13

**Decision.** Each site's first pool shard keeps a `PoolLedger` in its SQLite: per network, the pool slots it holds, its owner, and its last renewal. Before a network's leader commits a `Fanout` step, it asks both warm sites' ledgers at once (`/pool/grant {network, owner, want}`). Each ledger runs `pool_grants` over its live leases, with the asker last. The leader commits a step up to 1 plus the fewer either granted, a step down only as `fanout` asks, and keeps k when either ledger does not answer within 3 s [guess]. A lease lapses unless renewed within LOAD_STALE, by the leader's asks while k is above 1 or by the first shard's once-a-minute `/pool/renew`. While the ledger holds a lease, its object's alarm runs it every 45 s (`LEDGER_BEAT`), which drops lapsed leases and marks the time. A gap of more than 90 s (`LEDGER_GAP`) [guess] since the ledger last ran means its object was down, and that gap counts against no lease. `shards_for` takes the network's granted slots, so each pool shard serves one network.

**Why.** `pool_grants` ran only in tests. `shards_for` gave every network the same first k - 1 pool shards by rendezvous, and the leader's `Fanout` commit raised k with no grant, so B0-34's "hostile owner held to its per-owner pool budget" and "the monorepo's k reduced by another owner" would have measured missing code. A per-owner budget means nothing while two networks share their pool shards. The operator's default design fixed the ledger's place, the grant before the commit, the lease, and the outage rule. The code differs from that default in three places:
- The leader asks for the k it holds, not only for a step, so a hot network's lease stays alive.
- A grant short of the held k (a ledger that lost a race, or a newly warm site whose pool is taken) never steps k down. Otherwise a ledger, not `fanout`, would take a network's shards away.
- A ledger's own outage is not counted against leases. Otherwise a ledger outage longer than LOAD_STALE would expire every lease on its return. As first built, the ledger read any 90 s with no call as an outage, and with sparse callers every call came after such a gap. Each call shifted every lease forward, so none ever lapsed. In LM-1's m21 (`lm1-memory` round 10), two m3org networks forced to k 4 and torn down without a release held 3 slots each on every site from 11:36Z to 13:11Z. That was the owner's whole share, so every later row's network was granted k 1. Since 2026-11-13 (branch `pool-lease-expiry`), the ledger's object beats every `LEDGER_BEAT` while it holds a lease, so a gap past `LEDGER_GAP` means the object did not run, and a lease nobody renews lapses LOAD_STALE after its last renewal however rarely the ledger is called.

**Beat.**
- Grants inside the leader's `Fanout` command's `decide`, which would read the site's pool through `fresh` as a cross-group dependency and make every network's step wait on a shared group.
- A pool group or object of its own: a new failure domain for the same state.
- Rendezvous pool slots with an owner check only on k: the shards stay shared, so one owner's grant still loads another's shards.

**Evidence.** `crates/git/tests/agent_load.rs`: `b0_34_k_is_set_in_band_and_the_hostile_owner_is_held_to_its_pool_share` now runs the leader's path (`LoadReports`, `pool_ask`, two `PoolLedger`s, `granted_k`) and checks distinct slots and each owner's share on each site every minute; `a_hostile_owner_with_three_hot_networks_is_held_to_its_share`, `another_owners_step_up_never_reduces_a_networks_held_shards`, `an_unrenewed_lease_expires_and_frees_its_shards`, `a_lease_nobody_renews_lapses_however_rarely_the_ledger_is_called` (fails without the heartbeat), `an_unreachable_ledger_keeps_k`; plants in `tests/am1/pool/plant.py`; the two-owner hosted smoke in `tests/am1/results/pool-grants.md`.

**Flip.** A plan model with per-owner shares, which replaces `owner_share`. Also flips on a hosted run where a ledger round trip delays a step past `fanout`'s up window, or where a ledger's SQLite write rate on one pool shard shows in its validate latency.

### M5-WP1: the `Release` group's rules, as built (2026-11-13)

#### `decide`'s clock is the entry's own time

**Decision.** `ReleaseFold::decide(anchor, at, bytes)` takes `at`, the time the leader stamped on the entry, and a follower's `check` takes the same stamp from the entry it verifies (the member's `release_at` table). `not_after` and a drain's 24 hours are judged against it, so the rules use no clock and the CLI, the member, and the tests run one function.

**Why.** "`decide` is pure" (`design/DESIGN.md`, Shape, the `Command` trait). A wall clock read in `decide` would let a follower refuse an entry its leader accepted.

**Beat.** Reading `Date.now()` in the member, which makes `decide` impure. Carrying the time in the signed body, which the signer controls.

**Evidence.** `an_intent_past_not_after_is_refused` and `the_executor_key_replaces_only_a_lost_site_and_pins_nothing` (`crates/release/src/tests`); plants 2 and 11 in `tests/m5/plant.py`.

**Flip.** A rule that needs time the leader cannot stamp, such as a deadline that must hold while no entry commits.

#### `SignedIntent` carries no signature of its own

**Decision.** `SignedIntent` drops `release_sig`, `key`, and `sig`. The entry's signature is the intent's, and the release's signature is on its `ReleaseUploaded` record, which an intent names by id.

**Why.** A signed entry already carries its signers and binds the install and `prev`. A second signature inside the body signs fewer bytes and adds a check that can disagree with the first.

**Beat.** Keeping the three fields and checking both, which doubles the signer rules.

**Evidence.** `each_kind_takes_only_its_signers`; `design/DESIGN.md`, Shape, `SignedIntent`.

**Flip.** An intent that must be relayed without its entry, such as one carried to a second install.

#### Entries after genesis are format 0x30.2

**Decision.** A `Release` member writes the genesis as 0x30.1 and every later entry as 0x30.2, and reads both. Tags 0 (`Genesis`) and 1 (`Keys`) are unchanged; tags 2 to 23 are new, in the design's order (`crates/release/src/tests/entry_tags.golden`).

**Why.** A member built before M5-WP1 decodes only the genesis. A new format makes its refusal "unreadable format", not a wrong decode, and the existing installs' genesis bytes still re-sign byte for byte.

**Beat.** Reusing 0x30.1 for the new tags, which an older member would misread as malformed.

**Evidence.** `the_codec_round_trips_every_entry_kind`; graft-m4a's `genesis.unsigned`, signed again by this branch's `graftyard sign`, is byte-identical to its committed `genesis.signed` (digest `b40c10ed84bd…`).

**Flip.** None expected: the wire format is append-only.

#### An `EdgeRoll` undo pin names R or the release the edge runs

**Decision.** An `EdgeRoll { release: R }`'s pre-signed undos may pin only R or the release the edge currently runs, and an `EdgeRoll` whose versions drop a version an undo of the current intent names is refused `DropsUndoVersion`.

**Why.** The design says the roll may not drop an undo's version (`design/DESIGN.md`, Shape, "Release group") but not which versions an undo may name. Only these two are deployed during the roll, so a pin to any other release would point at a version no deployment holds.

**Beat.** Accepting any uploaded release as the pin, which lets an undo name a version the roll already removed.

**Evidence.** `an_edge_roll_that_drops_an_undos_version_is_refused`; plant 7 in `tests/m5/plant.py`.

**Flip.** Edge deployments that keep more than two versions (M5-WP2's gradual deploy), which would widen the set.

### M5-WP2: the cached `Release` view, pin-only pointers, and `graft-front`, as built (2026-11-14)

#### The view is a replica of the group's fold, not a copy of a member's answer

**Decision.** Each `graft-front`, edge, and site isolate holds its own `ReleaseFold`, built by replaying the group's committed entries, read by position from a member's `/view` (`{"from": n}`, entries with their entry times, plus `pending` rows), through the same `check` and `apply` a follower runs, against the isolate's own trust anchor (`crates/release/src/view.rs`, `Replica`; `crates/lookup/src/view.rs`). A page that starts past the replica's count is a gap and refused, and a member behind it adds nothing, so the view never moves backward. A refresh reads members, the last-named leader first, until one that names itself leader answers; a follower's page is applied on the way.

**Why.** "Refreshed from any member's applied state and never moved backward" (`build/BUILD-PLAN.md`, M5-WP2). A member's answer trusted as a summary would let one bad member set a pointer; replayed entries verify by their signatures and bound `prev`, so a member can only withhold. A follower applies a commit only when a later round carries it, and an idle `Release` group sends none, so a view read from a follower can lag a commit until the group's next entry.

**Beat.** A signed snapshot of the fold, which needs a signer that is not the group. Stopping at the first member that answers, leader first when one is known: BF-43's first run had isolates whose hint was empty read a follower, and requests ran unpinned for up to 59 s after the undo's 201, until the next entry. A follower `/view` that waits for a read-index round, which costs the leader a round per isolate refresh.

**Evidence.** `a_replica_replays_the_group_from_any_member_and_never_moves_backward` and `a_replica_refuses_another_installs_or_a_tampered_history` (`crates/release/src/tests/group.rs`, "The cached view (M5-WP2)"); plants 0 to 2 in `tests/m5/plant_wp2.py`; BF-43 runs 1 and 2 (`tests/m5/results/m5-wp2.md`).

**Flip.** A `Release` log too long to replay on a cold isolate inside VIEW_CALL_MS, which would need a snapshot the anchor can check.

#### When a request waits on the view

**Decision.** `Freshness::need`: a busy isolate refreshes beside its requests at POINTER_TTL - RETRY_MS (4 s); a request waits, at most VIEW_CALL_MS (1 s) from its arrival, only when the view is missing or POINTER_TTL old and the last refresh did not fail; once a refresh has failed, or while one beside requests has hung for under ABANDON_MS (4 s), no request waits, and the refresh is retried every RETRY_MS beside them.

**Why.** BF-43 needs every request pinned within POINTER_TTL of the undo, and BF-8 needs no request blocked on a blackholed group. A refresh at the TTL itself would leave requests between TTL and the refresh's answer on an old view.

**Beat.** Waiting on every refresh (BF-8's blackhole would hold every request 1 s). Never waiting (a cold isolate's first requests would carry no pin for up to a round trip after a pin, BF-43's tail).

**Evidence.** BF-43 run 2 (every request pinned by +3.2 s) and BF-8 (168,705 requests through a 601 s blackhole, p99 73 ms, none blocked), `tests/m5/results/m5-wp2.md`; `a_view_is_refreshed_before_pointer_ttl_and_no_request_waits_on_a_failing_group`; plants 9 to 11. It gives a cold isolate nothing to keep: BF-8's isolates started during the blackhole served unpinned (`decisions/OPEN-QUESTIONS.md`, R20).

**Flip.** A hosted run where an isolate's first request after an idle period waits its full second on a group that answers slowly but does answer, or an operator decision on R20 that cold isolates wait for the pins. R20 was decided for a copy outside the group instead ("A copy of the `Release` group's entries outside the group", below).

#### A pointer undo moves the script's deployed release

**Decision.** A committed `EdgePointer` (or other pointer) undo sets `deployed[script]` to (the rolled release, the pin), so the next roll of that script keeps the pin beside its new release, and `DropsUndoVersion` judges against the pin. `roll_steps` bounds an `EdgeRoll` to 4 `IntentStep`s and the 1%-then-100% rolls to 2 (`Refusal::PastLastStep`), and a `FrontRoll` with an undo is refused.

**Why.** After an undo, `graft-ops` deploys the pin at 100% (Shape, `Rollback`), so the release the next roll keeps is the pin, not the one the undo left. A step past `ROLL_STEPS` would record progress no deployment made. `graft-front` is the swap mechanism and has no pin of its own ("Downtime", `graft-front` and `graft-ops`).

**Beat.** Leaving `deployed` at the rolled release, under which BF-8's refused roll would have been accepted.

**Evidence.** BF-8's refused `EdgeRoll` hosted; `the_next_roll_of_the_pinned_script_clears_the_pin_and_keeps_the_pinned_release` and `rolls_take_only_their_roll_steps_and_a_front_roll_carries_no_undo`; plants 3 to 6. `graft_front_alarms_only_when_a_pinned_call_ran_another_version` and plants 7 and 8 cover the alarm and the override header.

**Flip.** Edge deployments that keep more than two versions.

#### An uncommitted `StaleMode`, and how members serve it

**Decision.** The edge commits a `StaleMode` for at most 10 s; on a 503 it verifies the flag against its view (`verify_stale`: the customer key alone, this install, no parent, `until` ahead) and writes it to every member's `release_pending` row, each member verifying against its own applied `Keys`, and answers 202 `{pending, of, rows}`. Isolates read the rows with the view, verify them again, and honor a flag until min(`until`, earliest arrival + STALE_MAX). A covered non-`Release` member answers `/read` from `stale()` with `"stale": {"until"}` and `/commit*` with `{"error": {"stale": {"until"}}}`. `GRAFTYARD_STALE_MAX_MS` (test-faults only) lowers STALE_MAX so a hosted run sees the bound end a flag.

**Why.** "A `StaleMode` that cannot commit is written to every reachable member's `pending` row and verified against the cached `Keys`" (`build/BUILD-PLAN.md`, M5-WP2). The earliest arrival bounds a flag replayed later to a second member. A write answered as an error, not a missed call, returns at once instead of being resent for a minute.

**Beat.** Committing the flag through another group (it is the `Release` group's own outage). Honoring from the latest arrival, which lets a replay extend it.

**Evidence.** BF-48 hosted; `an_uncommitted_stale_mode_takes_only_the_customer_key_and_a_future_until`, `an_uncommitted_flag_is_honored_until_until_and_never_past_stale_max_after_its_arrival`, `a_committed_stale_mode_is_honored_until_its_until`, `a_flag_covers_only_its_scope`; plants 12 to 20. The edge's signer precheck: `the_edge_prechecks_signers_against_its_cached_keys`, plants 21 and 22.

**Flip.** A pending flag that a member could not verify because its `Keys` lagged the edge's, which would need the edge to send the `Keys` entry with it.

#### A copy of the `Release` group's entries outside the group

**Decision.** Every `Release` member, on the alarm right after it applies entries and on restore, writes the signed entries it applied, with their entry times and positions, to a copy in the `Release` chain's own `MemberRegistry` object on its site (`registry#36`, route `/release_copy`; `workers/site/src/release.rs`, `copy`; `workers/site/src/registry.rs`). The copy only grows: a write adds positions it lacks and changes nothing past a gap. A refresh that requests wait for, on an isolate that holds no view or one idle past POINTER_TTL, also reads every site's copy, for at most `COPY_MS` (500 ms), and takes one only through `Replica::adopt`: every entry it lacks passes `check` against the isolate's own anchor or none is taken, and a copy no longer than what the isolate holds adds nothing. The waiting requests go out when the group answers or when every copy has answered, so they carry the longest copy; a cold request waits at most `COPY_MS` from arrival and an idle one at most VIEW_CALL_MS, then goes out with what is held, unpinned if nothing came. The refresh goes on beside later requests, so the group's view replaces the copy once a member answers (operator decision on R20, option 1, 2026-11-14).

**Why.** BF-8 run 2 failed "requests keep the last pointers" only on isolates that started during the blackhole: the 18 started before held the pin on 15,451 of 15,451 requests, and of the 126 started during it, 123 held no view and served 122,791 of 153,254 requests unpinned, 1,214 of them on N+2 (`tests/m5/results/m5-wp2.md`). The registry is an existing per-site class that every script with the group's bindings already reaches, and the site fault and a blackholed group do not take it down, so it is not a new failure domain. The row is the committed entries themselves rather than a summary of the pins, because pins derive from many entries (a later intent clears one), and only the signed history lets an isolate check the copy against its anchor as it checks a member's page.

**Beat.** Releasing the requests on the first copy taken, and reading copies only on an isolate that holds no view (BF-8 run 3: a follower site's copy of the idle group lagged the leader's by one entry, so 159 requests on isolates started during the blackhole went out unpinned in their first 1.4 s, and six isolates idle from BF-43 to BF-8 kept a view without the pin through the whole blackhole, 4,059 requests). Cold requests waiting for the group (option 2): breaks "no request waits on the group". Narrowing BF-8's row to isolates started before the blackhole (option 3): a substitution. A new Durable Object class or store for the copy: a new failure domain. A binding or variable carrying the last pin: a new script version per pin, which a site day must ship.

**Evidence.** `a_cold_isolate_takes_a_sites_copy_whole_and_an_older_copy_never_rolls_its_view_back` and the cold case of `a_view_is_refreshed_before_pointer_ttl_and_no_request_waits_on_a_failing_group` (`crates/release/src/tests/group.rs`); plants 2 and 3 in `tests/m5/plant_wp2.py`. BF-8 run 4, 601 s blackhole, as written (`tests/m5/results/m5-wp2.md`): 200,416 of 200,416 requests answered 200 and carried the pin, all on N, p99 56 ms, max 1,060 ms. The 37 isolates started before the blackhole pinned 33,642 of 33,642; the 240 started during it pinned 166,774 of 166,774, each minute of starts from 0 to 9 at 100%. Every one of the 277 took a site's copy (sites 0 and 2), read in 81 ms at p50 and 139 ms at most. Cold-start requests (an isolate's first 600 ms) were 28 ms at p50, 508 ms at p99, 674 ms at most, under `COPY_MS` plus the request. BF-8 run 2 for comparison: 15,451 of 15,451 pinned on isolates started before, 30,463 of 153,254 on the 126 started during. Run 3, with the first copy releasing the wait and idle isolates reading none: 159 of 151,717 unpinned on cold isolates and 4,059 on six idle ones.

**Flip.** Copy reads that miss `COPY_MS` on more than 1% of cold isolates while a registry answers (run 4's slowest was 139 ms of 500), which would raise the bound; a `Release` log too long for one registry read, which would need a signed snapshot; or a fault class that takes every site's registry with the members, which leaves the gap R20 named.

#### The run stands in for `graft-ops`'s executor

**Decision.** M5-WP3 builds `graft-ops` executing intents. Until then BF-43 and BF-8 deploy each split with `wrangler versions deploy` from the orb (the ops path) and commit the executor-signed `IntentStep` and `IntentDone` with a key the run holds. The edge versions N, N+1, and N+2 are three uploads of the same build with a distinguishing variable.

**Why.** The rows test the request path: a pointer read by `graft-front` with no API call. Who deploys the split does not change what a request sees.

**Beat.** Waiting for M5-WP3, which would leave the pointer path untested hosted until then.

**Evidence.** `tests/m5/wp2-hosted.sh`; `tests/m5/results/m5-wp2.md`.

**Flip.** M5-WP3's gate, which reruns BF-43 with `graft-ops` executing.

### M4-WP2's hosted half, as built (2026-11-13)

#### M4-WP2's consumers run in band on the leader alarm

**Decision.** `Notify` and `Search` run on each repository leader's alarm, and `Counts` on each `Identity` leader's, as `Webhooks` does (`workers/site/src/activity.rs`). Each reads its committed cursor, does one batch (at most 100 entries [guess]), and commits its cursor through its own member under a request id derived from the chain, the consumer, and the batch's end. A target that does not answer keeps the cursor, and the alarm comes back in 2 s. Nothing goes through Queues. Every consumer runs only where the site variable `GRAFTYARD_ACTIVITY` is `1`.

**Why.** BF-5 bounds a star and a notice at 10 s. `PrSync`'s queue path delivered at p50 2.0 s and p90 5.1 s with outliers near 37 s, which is why its first attempt moved in band (`PrSync`'s first attempt runs in band, above). These consumers' work is a few Durable Object writes and no git cache merge, so the leader's isolate holds none of it for long. The cursor already gives at-least-once delivery, and each target is idempotent: `Inbox::apply` ignores an entry at or below `sources`, a document write replaces the document, and `CountsCmd::decide` keeps only newer `src`.

**Beat.** A Queue message per batch, as SHARED's "Event fan-out" names: it adds the measured queue delay to every star and notice and buys nothing the cursor does not already give.

**Evidence.** `tests/m4/results/m4-wp2.md`, "Hosted" [measured once BF-5 runs hosted]; the in-band precedent `tests/m2/results/m2-wp5.md`, "2026-11-05".

**Flip.** A hosted run where the consumers' batches hold a leader's alarm long enough to slow its commits, measured, or a queue delivery bound under 2 s at p99.

#### A jurisdiction domain with no site of its jurisdiction binds its shards on its regions

**Decision.** `SearchShard::site` takes the domain's admitted sites as before. When the install has no site of an `Eu` or `Us` domain's jurisdiction at all, it binds that domain's shards on the sites of its regions (`weur` and `eeur` for `Eu`, `enam` and `wnam` for `Us`). The object is still created under the jurisdiction, through the namespace's `jurisdiction()` (`graftyard_lookup::Bindings::object`; `worker` 0.8 has none, so the JavaScript method is called).

**Why.** The jurisdiction, not the binding's script, decides where a Durable Object runs and stores (`tests/m4/c15`: an `eu` object ran in WAW from a script deployed with no placement). `graftyard sites add` refuses once an install has a genesis, so `graft-m4a` can gain no `eu` site, and BF-68's pass line is about the documents' shards only. Where the org's repository chains sit is Milestone 5's EU sites, outside BF-68 (operator, 2026-11-13, relayed by the M4-WP2 lead).

**Beat.** Deferring BF-68 to a new install with an `eu` site, which costs a site set and a genesis for a claim the shards alone carry.

**Evidence.** `crates/api/tests/activity_gates.rs`, `eu_shards_bind_on_eu_regions_when_no_site_is_eu_and_names_round_trip`; hosted, each EU shard's `/stats` reports `jurisdiction: "eu"` and its colo (`tests/m4/results/m4-wp2.md`, "Hosted").

**Flip.** A hosted EU shard that reports no `eu` jurisdiction or a colo outside the EU.

#### A shard's generation is a map on every site's `Registry`, and a lost shard is rebuilt by the consumer that meets it

**Decision.** The newest generation of each (domain, shard) lives in the `search_map` table of every site's `Registry` object `search_map`, and only moves forward. A `Search` write that finds its shard lost (`GRAFTYARD_SEARCH_LOST`, a site day's variable, or the operator's `/lose`) rebuilds the shard under the next generation from every repository of the domain and shard that the `MemberRegistry` rows list, read on each leader (`/activity {"docs": true}`). The new object answers 503 to queries until its `/built`, then the map moves, and the consumer's cursor, held meanwhile, resumes. The edge leaves a 503 or late shard out with `incomplete_results`.

**Why.** DESIGN names a generation per rebuild but no place to keep it. The `Registry` objects already exist on every site, and a map read by every query must survive the loss of any one site.

**Beat.** A chain of its own for the map, which a derived store does not need: a lost map row only makes the next rebuild reuse a generation's name, which the map's forward-only write refuses.

**Evidence.** `workers/site/src/search.rs`, `workers/site/src/activity.rs`, `workers/site/src/registry.rs`.

**Flip.** A hosted rebuild that misses a repository the registry rows list, or a query that reads a stale generation after `/built`.

### `m0-carried-8`: a pipeline to each voter, and a loop-lag step-down, 2026-11-13

#### AppendEntries are pipelined to a voter

**Decision.** A leader keeps up to `PIPELINE` (4) AppendEntries calls out to one voter. A call to a voter whose earlier calls are still out carries only the entries after the last of them, and the follower stores it right behind them. Any failure, refusal, catch-up, install, or truncation clears the voter's pipe, and the next call starts again from `next`. Learners keep one call each. This reverses the (b') half of "The far follower's held sends get no host change".

**Why.** With one call per voter, an entry appended while the far voter's previous call was running waited for that call to end. When the near voter went quiet for about 1 s, the far voter made the quorum, and commit waited one whole far call: 9 of 9 such calls in `m0-carried-7`'s four near-gap failures, 50 to 984 ms late. The far follower's own side was fast. Pipelining sends each entry at once; a model of those 9 calls puts 6 of them near 250 ms instead of 447 to 818 ms [inference, `tests/m0/results/carried-8.md`].

**Beat.** Keeping one call per voter (operator, `m0-carried-7`), on the premise that the near follower makes the quorum, which these runs broke. An unbounded pipeline, which lets a stalled far follower collect every entry's call. Reordering a turn's sends before its writes (b), which moved none of the held sends.

**Evidence.** `tests/m0/results/carried-8.md`, "Near-gap traces" [measured]. `a_far_voter_gets_each_entry_while_its_earlier_calls_are_out` and `a_voter_never_has_more_than_pipeline_calls_out` (`crates/raft/tests/scenarios.rs`) fail with no pipeline and with no bound. The long sweep counts pipelined calls and those delivered late or twice, and asserts both happen.

**Flip.** A hosted A/B in which the pipeline does not shorten commit when the near follower is quiet, or a Raft history it breaks.

#### A leader whose loop is behind steps down (`LOOP_LAG`)

**Decision.** `STEP_DOWN` also trips when two of the leader's storage confirms within 2 s took 200 ms or more and an exact multiple of 100 ms (`LOOP_LAG`, `StepDownWhy::LoopLag`). It transfers as a confirm trip does: no probe, to a candidate outside RESET_QUARANTINE whose link is not failing.

**Why.** While a Durable Object's event loop is behind, workerd clamps `Date.now()` to the overdue 100 ms tick's deadline, so a confirm that begins and ends late reads a whole number of ticks. The clamp hides the lag from the ticker and from any wall-clock measure, but not from that signature. Three B0-02 failures (r300-7, r330-8, ab07) were such a leader, with confirms of 300 to 1,000 ms that stayed under the confirm rule's six-over-500 ms. Over 82 one-minute B0-02 runs, two such confirms in 2 s matched 11 of 25 failing runs and 1 of 57 passing ones (`m0c6-ab04`, at 600 ms); three in 5 s gave the same false positive and 10 matches, four in 5 s none and 8 [measured].

**Beat.** A peer-stamped lag (follower arrival time against the leader's call end), whose worst 5 s median reached 1,290 ms in a passing run. The clamp share of the trace, which matched fewer failing runs at the same false-positive rate. Four in 5 s, which matched none of the passing runs but 3 fewer failing ones.

**Evidence.** `tests/m0/results/carried-8.md`, "LOOP_LAG" [measured]. `two_clamped_confirms_trip_loop_lag` (`crates/raft/tests/leadership.rs`); `leaders_leave_a_lagging_loop` (`crates/raft/tests/liveness.rs`), whose sick leader's confirms stay under `confirm_p99_ms`, moves 12 of 12 groups by `STEP_DOWN` in 1.4 to 3.3 s with it; without it 11 of 12 never moved in the fault's minute and one moved at 37 s. The simulator's `LoopLag` gray fault runs in the gray and live sweeps.

**Flip.** A hosted leader that trips `LoopLag` with no lag in its trace, or a run where the transfer costs more than the lag it ends.

#### The gate on the final tree, 2026-11-13

**Measured, not a decision.** On main `84d29c2` (F67 rules 10 to 13) plus the pipeline and `LOOP_LAG`, B0-02's gate passed 7 of 8 at 300 and 5 of 8 at 330 (`tests/m0/results/carried-8.md`). The near-gap shape did not recur (no near call of 400 ms or more outside r330-6) and no leader was clamped, so neither rule above is reversed. Two misses meet the `ENTRY_SOFT` flip ("capped entries do not lower commit p99") at 330: one 60 KB entry per round at a round p50 of 169 to 174 ms holds the group to 322 to 331 commands/s. One miss is a far follower whose catch-up calls of 319 to 717 KB missed `CALL_DEADLINE`, so `failed_to` turned off its pipeline and every later call carried the backlog; with a slow near follower, `CommitErrors` moved leadership to the far site. The next step is the operator's (C20).

### M4-WP1's hosted half: webhook attempts run in per-hook lanes, 2026-11-13

**Decision.** The `HookSequencer` keeps order, the delivery rows, and the cursor halves, and signs every request. It sends each attempt through the hook's lane, `hooklane:<wire>:<hook>`, an object of the same class on the store's site that keeps nothing. One lane call carries a queue's run: its head and up to `RUN_MAX` (8) deliveries queued behind it, sent in order and stopped at the first non-2xx or after `RUN_FOR_MS` (2 s). The store records each sent delivery as its queue's attempt only once it is the queue's front (`Sequencer::queued_after`, `next_in_run`), so one attempt in flight per (chain, hook) and chain order hold as before. Lanes are named and routed by `LANE_WIRE`, so a store on a new release never reaches a lane still running an old one.

**Why.** BF-57's healthy p99 stayed at 2.1 to 3.2 s through runs 8 to 12, because every POST ran inside the owner's one object behind its other work. A probe at 49 busy queues put the POST at 184 to 301 ms p50 inside one object, at 205 ms from an invocation of the site script over its own `SITE` binding, which runs beside its caller, and at 7 ms from a separate object (`graft-m4-probe-sb5`, `-sb8`, 2026-11-13). Run 13, on the `SITE` binding, had p99 9.4 s. Run 14, on per-hook lanes with one attempt per call, had p99 2.10 s, and all 124 late deliveries were behind earlier entries of their own queue. Run 16, with runs, had p99 1.11 s and none over 2 s (`tests/m4/results/m4-wp1-hosted.md`) [measured].

**Beat.** Stateless delivery Workers over the site script's own `SITE` binding, as DESIGN's Shape first read and as `prsync::in_band` does (run 13, 9.4 s). Batching the store's report writes (runs 10 to 12, no gain). Attempts through Queues (latency). One lane call per attempt (run 14, 2.10 s).

**Evidence.** `tests/m4/results/m4-wp1-hosted.md`, BF-57 runs 13 to 16. `a_run_sends_a_queues_waiting_deliveries_in_order_under_one_slot` (`crates/git/src/sequencer.rs`).

**Flip.** A run in which lanes do not shorten the healthy tail against the store's own POSTs, or a chain-order inversion that a run causes.

### M4-WP3: the Actions shard's commands, and `RunOwner`'s writes, 2026-11-14

**Decision.** The Actions family is new format family 0x15. Its first format, 0x15.1, carries DESIGN's five `ActionsEvent` variants and a sixth, `OwnerCursor { through }`, which is `RunOwner`'s cursor on the shard it reads. The commands are `ActionsCmd { actor, op }`, and `crates/git/tests/wire_tags.golden` pins their tags. A job's `needs` are released in the same entry as the completion that meets or fails them, by the shard's own `decide`, and a skip cascades in spec order. `RunOwner` writes the repository's check suite, check runs, and required checks through `RepoCmd::RunChecks`, format 32.21, which adds no `RepoEvent`. A check run's id is its job's id, a suite's id is its run's id, and the row's version follows the job's state (1 queued, 2 in progress, 3 completed), so `decide` drops a replayed or late update. A deleted repository answers `Gone`, and `RunOwner` then commits `CancelRun` for its unfinished runs. The Actions App is `AppId(DbId(2))`, in cell 0's block 0, which `Ids` never leases.

**Why.** SHARED's Workflow-run operation needs `RunOwner`'s cursor to pass a `Completed` job only after the repository took its conclusion. That cursor is consumer state on the chain it reads, as `HookCursor` and `ConsumerCursor` are (SHARED, "Other consumers"). Releasing `needs` on the shard keeps dispatch independent of the repository, as SHARED's Isolation invariant asks: a repository that is down delays only its checks. Keying a check row by job id and versioning it by state makes every `RunOwner` write idempotent with no extra table. BF-56's in-memory half kills the repository's commit path before any check write and, separately, `RunOwner` between its repository writes and its cursor. In both cases every required check had its conclusion within 60 s of its job's `Completed`, and no check-run version committed twice (`crates/actions/tests/gates.rs`) [measured].

**Beat.** Letting `RunOwner` release `needs` after the repository acknowledged, which stalls a run's next job on the repository. A new `RepoEvent` for Actions checks, when `CheckRun`, `CheckSuite`, and `RequiredCheck` already carry what is needed. Keeping the cursor in the consumer's own storage, which a site loss drops.

**Evidence.** `crates/actions/tests/gates.rs` (BF-4, BF-56, one runner at a time, the abort path). `tests/m4/wp3/plant.py`: every planted bug is caught. The 0x15 and 32.21 choices were checked against `origin/main` and every open branch on 2026-11-14; 32.20 is `b030-split`'s.

**Flip.** A run whose `RunChecksCmd` entries grow past one entry, which forces splitting a suite. Or GitHub semantics showing that a job whose need was cancelled runs anyway.

### M4-WP3: the run service's signing key, the live-log viewer path, and how the edge finds `RunnerBroker` and `LogRelay`, 2026-11-14

**Decision.** Three choices the lead settled for the hosted run service. (1) The run-service key that signs runner and job tokens (`graftyard_actions::runsvc::sign_token`) is a Secrets Store secret, `RunServiceKey`, bound to the site scripts as `RUN_SERVICE_KEY`. The sites sign; the edge verifies with the public half, a plain var `RUN_SERVICE_PUBLIC` it already has in its version. Rotation is a site day and an edge day under BF-59's rotation steps, never a request-path API call. (2) A web viewer opens the live log at `/_t/logrelay/repos/{owner}/{repo}/jobs/{job}`, so the edge checks the viewer's read access with the repository read it already does and finds the relay with no lookup. (3) `RunnerBroker` and `LogRelay` stay where DESIGN places them, on the Actions shard's leader site (Shape, "Identity"); the lead's home-site placement applied only where DESIGN said nothing, and BF-55's pass line names "a broker on the new leader site". A broker is one object per registration shard, `broker:actions:<owner>.<shard>`, which serves both scopes registered there, so a runner's token needs to name only its shard. A relay is `logrelay:<job>` on the site whose leader took `acquirejob`, and `FeedStreamUrl` names that site. A runner's and a job's tokens name their Actions shard (`ChainId::Actions`), so the edge routes every run-service call to the shard's leader site with no other lookup.

**Why.** The key: only the sites build job messages, so only they need the secret, and the edge already verifies other tokens offline with key material it holds. Secrets Store keeps the key out of script versions and out of every log. The path: the web app already reads `/_t/ui/repos/{o}/{r}/...` under the same check, and a path with only a job id would need a job-to-repository read before the access check. The placement: DESIGN's, which BF-55 measures; a relay does not follow a later leader move, because the runner keeps the socket it opened. A viewer's edge joins the relay on the site the shard's leader is on now, the same site unless the leader moved after `acquirejob`; after a move the viewer gets `no-stream` and reads the step log blobs, as after a relay kill (BF-61).

**Beat.** A key in a plain var or a Workers secret on the edge, which puts the signing half where no request needs it. A viewer path keyed by job alone. The repository's home site for both classes, which BF-55's pass line rules out. One broker per (owner, scope), DESIGN's key, which needs the scope in every runner token; a per-shard object serves the same runners.

**Evidence.** Lead decision, 2026-11-14 (thread T-01a17bda). `tests/m4/wp3/runner-protocol.md` (the runner reads `orch_id` only, so any signing key works). `crates/actions/src/runsvc.rs` tests: a token verifies only under its key and names its shard.

**Flip.** A registration shard whose `Repo`-scope runners from many repositories crowd one broker past `BROKER_HELD_MAX`: then brokers split by scope as DESIGN keys them. Or a second verifier of job tokens outside the edge, which would need the public key elsewhere.

### M4-WP3: where Actions secrets, variables, and `SealKey`s are stored, 2026-11-14

**Decision.** As SHARED places them. A repository's and its environments' secrets and variables are `RepoEvent::ActionsSecret` (tag 44) and `ActionsVariable` (tag 45), core tier, in repository format 32.23, keyed by scope (`""` or `env:<name>`) and name. The owner's `SealKey`s and its org secrets and variables are `AuthzEvent` tags 17 to 19 on the owner's `Authz` chain in Authz format 0x12.12, stored in a trailing part of the fold, so a fold with none keeps its bytes. One `SealKey` per owner serves the repository, environment, and org public-key routes. A row holds only the `crypto_box_seal` bytes the client sent and the owner's `key_id`. `RepoEvent` tag 43 is held as `Reserved43` for `b030-split`'s `Split`, so the tags after it match on every branch. An `Agent` may not write any of them.

**Why.** SHARED names the events and the chains. Keying a row by scope and name gives the GHES routes their reads with no index, and a job's reads at `acquirejob` are one scope each. A per-owner key needs no key per repository and is what SHARED says; `gh secret set` reads `key_id` and `key` and sends `key_id` back, so it cannot tell [measured: `cli/cli` `pkg/cmd/secret/set`]. Holding tag 43 keeps the tag table append-only on both branches when they merge.

**Beat.** A key per repository, as GitHub keeps [unverified], which multiplies the rotation job's work for no client-visible gain. Secrets in their own store outside the chains, which would need an outbox for their events. Taking tag 43 on this branch, which would make the two branches' tables disagree.

**Evidence.** `crates/git/tests/issues.rs` (scopes kept apart, sealed bytes stored as sent, variables' 409 rule), `crates/identity/tests/sealed.rs` (key rotation and drop rules, org rows, bytes unchanged with no Actions row). Tags and formats checked against every origin branch's `wire_tags.golden` and format constants on 2026-11-14.

**Flip.** A GHES client that compares `key_id` across repositories of one owner and expects them to differ. Or an owner whose org rows grow past what one `Authz` fold should carry, which would move them to their own chain.

### M4-WP3: who opens Actions secrets, and how both rotations run, 2026-11-14

**Decision.** `graft-ops` alone opens a `SealKey`'s private half. At `acquirejob` the edge reads the job's repository and environment rows (repository chain) and the org rows whose `SecretScope::reaches` the repository (owner's `Authz`), and sends the sealed records with the owner's wrapped keys to `graft-ops` `POST /actions/open`, which answers the plaintexts for that job message alone. The edge passes them to the shard leader in the `acquire` call, which puts them in the message's `variables` and `mask`; nothing stores or logs them. A run marked `fork` reads no secret, and still reads variables. A record whose key does not open it is left out by name. The `SealKey` rotation is an owner admin's `POST /api/graftyard/v1/{orgs,users}/{login}/actions/rotate-seal-key` on the edge: mint a key (`/actions/sealkey`), reseal every org, repository, and environment secret to it inside `graft-ops` (`/actions/reseal`, sealed bytes in and out), retire the older keys, then drop each one a fresh read shows no record names. The wrapping key's rotation is `.../rewrap-secrets` after `graft-ops` is redeployed with slot B bound and current: each `SealKey`'s private half and each hook secret is wrapped again under it (`/actions/rewrap`, `PutSealKey` with only `private` changed), and then `graft-ops` is redeployed with slot A cleared. Until `ActionsTrigger` builds fork pull request runs, the admin dispatch route's `fork` field marks a run as one.

**Why.** SHARED puts decryption behind `graft-ops` and gives job-message assembly and rotation as the only openers ("Who decrypts", "Rotation"). Opening on the edge keeps every plaintext out of the site script, its storage, and its logs; the leader only copies strings into the answer. Running both rotations as an edge route is the simplest place that already reads every chain the rotation writes, through the same commit paths a client's write takes, so each step is an ordinary acknowledged command and a stopped rotation is safe to run again.

**Beat.** Opening on the shard leader, which would put plaintexts inside a Durable Object's isolate next to its SQLite. A rotation job as a site-script consumer, which would need its own cursor over every repository of the owner for an action an admin runs rarely. A fork route built for BF-59 alone, which is M4's `ActionsTrigger` work.

**Evidence.** `workers/ops/src/lib.rs` (a PyNaCl `SealedBox` opens under the minted key and not under another or a tampered one; a reseal opens under the new key only), `crates/identity/tests/sealed.rs` (a rewrapped key replaces only `private`), `crates/actions/src/specs.rs` (a job's environment in both forms), and BF-59 on graft-m4a (`tests/m4/results/m4-wp3.md`).

**Flip.** An owner with more repositories than one edge request can reseal in its time limit, which would move the rotation into a consumer. Or `ActionsTrigger` building fork runs, which retires the `fork` field.

### M4-WP3: the workflow parser reads the runner's schema as data, 2026-11-14

**Decision.** `crates/actions/src/workflow` validates a workflow against `workflow-v1.0.json`, the schema `@actions/workflow-parser` 0.3.61 ships (MIT), with descriptions removed, through a port of its schema-driven `TemplateReader`. It does not hand-code the subset of keys the gates use. The converter, expression parser, and step ids port the npm package, and the matrix ports the runner's C# `MatrixBuilder`, which the npm package does not have. The differential test compares the port's output with the oracle's JSON for every workflow in `tests/m4/wp3/workflows/`, errors included. A multi-line string's embedded expressions are read from its value, as the C# reader does. The npm reader reads them from the raw YAML source, indentation included, to keep editor ranges, so the oracle script turns that off. YAML is `yaml-rust2` 0.10 with default features off.

**Why.** A hand-coded subset rejects or mis-reads keys the runner accepts, and real workflows use many: 43 public workflows use `container`, `services`, `defaults`, `permissions`, `timeout-minutes`, reusable calls, `schedule`, and `workflow_dispatch` inputs. With the schema as data, the allowed contexts per key, the required properties, and the error messages come from the file the runner reads. The two readers differ only where the npm one serves an editor, and the job message the runner evaluates must carry the C# reader's strings, so the port follows the C# reader there [measured: `07-scalars.yml` and `tests/m4/wp3/oracle.mjs`].

**Beat.** A hand-written parser of the gates' keys, which is smaller but fails on most of the corpus. Running `@actions/workflow-parser` itself in the site script, which adds a JavaScript runtime to a Rust Worker for one parser. `serde_yaml`, which is deprecated.

**Evidence.** `crates/actions/tests/workflows.rs`: 59 of 59 workflows match. `tests/m4/wp3/plant.py` 18 to 40: 23 of 23 planted parser, matrix, and filter bugs are caught (`tests/m4/results/m4-wp3.md`).

**Flip.** A workflow on which the runner and the npm oracle disagree in a way the gates see, which would make the C# parser the oracle. Or a schema version the runner ships that the npm package does not.

### M4-WP3: how BF-56 gets a required check, a cancel, and its two kills on a product install, 2026-11-14

**Decision.** BF-56 runs on graft-m4a through product routes and three operator-only fault routes. The rule is GHES's `PUT /repos/{o}/{r}/branches/{branch}/protection` (`crates/api/src/protection.rs`), which commits `IssueOp::SetRule` (format 32.5) on the repository chain for a repository admin, with `GET` and `DELETE` beside it; a branch's rule is the one whose pattern is the branch's name, and a `restrictions` other than `null` is refused 422. The cancel is GHES's `POST /repos/{o}/{r}/actions/runs/{run_id}/cancel`, which commits `CancelRun` on the shard for a caller with write access and answers 202 `{}`, or 409 for a completed run. The kills: a member's `/abort` resets one member (`ctx.abort()`), so killing a repository's leader leaves its group leaderless until an election; an Actions member's `/owner_fault` either holds `RunOwner` rounds for `hold_ms`, which places the commit-path kill between `RunCreated` and the round that writes the checks, or, with `kill`, resets the member after the next round's repository writes and before its shard half. The edge forwards none of the three; only a scratch Worker bound to the `Member` classes reaches them, as BF-61's relay kill did. A `RunOwner` read cut at its byte cap that touched no run commits `OwnerCursor` alone, so runner traffic cannot hold a later run's checks.

**Why.** No product route wrote `SetRule`: M2-WP3 seeded rules through the test-faults `/bench_seed`, and graft-m4a's sites are product builds. The branch protection route is `rest:branches`' T0 surface anyway, so it serves clients as well as the gate, and cancel is `rest:actions`'. A reset is the kill the in-memory gate models (a crashed leader, and a `RunOwner` that died between its two halves), and `ctx.abort()` is what a lost isolate does, so it needs no other fault machinery in the product build. Routes reached only through a binding keep clients away from them, the same line `LogRelay`'s `/abort` drew on site day 30.

**Beat.** A test-faults site day for the run, which would reset every member twice and leave the product build untested. A `Rule` seeded with `/record`, which is test-faults and skips `decide`'s admin check. Killing the repository leader with `/sever`, which is test-faults only. A sleep inside the round for the `RunOwner` kill, which an alarm's time limit could end at an unplanned place.

**Evidence.** `crates/api/tests/surfaces.rs`, `branch_protection_put_edits_the_branch_s_rule_and_delete_removes_it` (a `PUT` edits the branch's rule instead of adding a second, both `contexts` and `checks` become required, `DELETE` leaves it unprotected, a writer is refused). `crates/actions/tests/gates.rs`, both BF-56 tests, for the shape the hosted kills reproduce. BF-56 on graft-m4a (`tests/m4/results/m4-wp3.md`).

**Flip.** A ruleset route (`rest:repos` rules) that clients use instead of branch protection, which would compile to the same `Rule` rows. Or a fault the gate needs that a reset cannot make, such as a member that answers but never commits.

### M4-WP3: BF-37's merge queue setting and `ActionsTrigger` for `merge_group`, 2026-11-15

**Decision.** Three pieces. (1) The branch protection `PUT` takes an optional `merge_queue` object holding GHES's `merge_queue` ruleset rule's seven `parameters` (`crates/api/src/protection.rs`), stored in the rule's `Protection.merge_queue`; `null` removes the queue and a body without the key keeps it. (2) `ActionsTrigger` runs `merge_group` only, in band on the repository leader's alarm (`workers/site/src/actions_trigger.rs`, pure half `crates/actions/src/trigger.rs`): from its own `ConsumerCursor { ActionsTrigger }` row it lists each `GroupBuilt`'s queue refs, reads `.github/workflows` at each ref's tip from the site's git cache shard, and commits each matching workflow's `CreateRun` on the repository's Actions shard; it commits its cursor through `ConsumerCursorCmd` (format 32.19, now also for `ActionsTrigger`) after a batch that built a ref, or every 1,000 entries. (3) A `merge_group` run's `RunKey` names its queue ref after the workflow path, because one `GroupBuilt` entry builds several refs; that `CreateRun` is format 0x15.2, which only a release that keys runs so reads. go-playground/webhooks v6.4.0, the latest release, has no `merge_group` type: its unmodified `Parse`, asked for `merge_group`, checks the delivery's HMAC and then answers "unknown event merge_group" (a bad signature answers `ErrHMACVerificationFailed` first). So BF-37's receiver (`tests/m4/wp3/bf37/`) runs `github.Parse` on every delivery, takes `PullRequestPayload` for `pull_request.enqueued` and `.dequeued`, and for `merge_group` counts the library's signature check passed only on that exact answer, then decodes the body into a struct written to GitHub's documented `merge_group` shape.

**Why.** No product route set a merge queue: M2-WP3 seeded its rule through the test-faults `/record`, and graft-m4a's sites are product builds. GHES sets the queue through rulesets, a whole new surface; one optional key on the route BF-56 already uses is the smallest product path, and a GHES client never sends it. A repository's push and pull request triggers still come from the admin dispatch route, so `ActionsTrigger` builds only what BF-37 needs, on the same in-band pattern as `Notify` and `Search` (`crate::activity`), which needs no queue. The run key with the ref keeps SHARED's at-most-once rule per event: GitHub sends one `merge_group.checks_requested` per queue ref, and a key without the ref would let only the first ref's run through.

**Beat.** A rulesets route (`POST /repos/{o}/{r}/rulesets`), which is the GHES way but a new surface with its own read shapes. Seeding the rule with a test-faults site day. A per-entry run key, which creates one run for a whole group. A full `ActionsTrigger` for push and pull request in this package, which no M4-WP3 gate needs. Patching go-playground/webhooks, which would no longer be the unmodified client the row names.

**Evidence.** `crates/actions/src/trigger.rs` tests (two queue refs of one entry give two runs with distinct keys at their own tips; only a `merge_group` workflow for the base runs). `crates/git/tests/merge_queue.rs`, BF-21 (100 seeds): no `gh-readonly-queue/*` ref is left after the landings and the ejection, while 900 were seen mid-run. `crates/api/tests/surfaces.rs`, branch protection (`merge_queue` set, kept, bounded, removed). `crates/api/tests/activity_gates.rs`, the cursor test (`ActionsTrigger` moves its own cursor). BF-37 on graft-m4a (`tests/m4/results/m4-wp3.md`).

**Flip.** A rulesets route, which would compile to the same `Protection.merge_queue`. Push and pull request triggers moving into `ActionsTrigger`, which retires the dispatch route's stand-in. A go-playground/webhooks release with `merge_group`, which the harness would then call through `Parse`.

### M4-WP3: job and workflow `env` ride on each step, 2026-11-15

**Decision.** `specs::job_specs` merges the workflow's `env`, then the job's, then the step's own into each step's `environment` in the job message, a later key replacing an earlier one; an `env` that is a single expression rather than a mapping replaces the outer one whole.

**Why.** BF-37's first hosted run (`tests/m4/raw/bf37/run1/`) failed every `merge_group` run, the passing pull request's too: the runner's log of job 1507329 shows the step's header with no `env` and `test "$BF37_FAIL" = 0` exiting 1. The job message never carried job-level or workflow-level `env` (`runsvc::steps_json` rendered only the step's), and BF-59's workflow, the only earlier one with `env`, set it on the step. Folding the outer levels into each step reaches the same environment the runner builds from GitHub's job message, in the order GitHub documents (workflow, job, step, the most specific winning) [doc: "Variables" in GitHub's Actions docs], with no new message field.

**Beat.** Sending the job-level mapping as the job message's `environmentVariables`, the field GitHub's own service fills; it needs the runner's evaluation of that list against the job's contexts, which no gate exercises yet, and the per-step merge reaches the same values for literal `env`. Moving BF-37's `env` to the step only, which leaves the product gap.

**Evidence.** `crates/actions/src/specs.rs` test `workflow_and_job_env_reach_each_step_innermost_wins` (keys overlapping at all three levels; it fails on the old code). BF-37's hosted rerun ran on the day-35 build, which lacks this fix, so its workflow sets `env` on the step.

**Flip.** A job-level `env` expression that reads a context only the job's evaluation has (such as `needs` outputs) and must not be re-evaluated per step, which moves the job's `env` to `environmentVariables`.

### M4-WP3: BF-55's `Owner`-scope matching, 2026-11-15

**Decision.** `Owner`-scope runners register on `OWNER_SCOPE_SHARD` (0), as DESIGN's `actions_shard` says, through the admin route's `"scope": "owner"` for an admin of the owner. Their broker reads all 16 shards' summaries (`ActionsFold::summary`, `{"summary": true}`) and picks the oldest fitting job with `graftyard_actions::broker::pick`. Because the job's shard holds no record of the runner, the broker sends the runner's labels in a new command, `ActionsOp::MatchOwner { job, version, runner, labels }` (tag 10, format 0x15.3), which only `Operator` sends and which commits the same `Job { Matched }` event as a local match, against the job's version. The job request names the shard (`runsvc::owner_job_guid`, GUID kind `0x10 | shard`), and the edge routes `acquirejob` by it. Two rules for a leader move: a committed match whose answer was lost is handed to its runner again on a poll more than `RESEND_AFTER` (30 s) after the match, and a broker whose site no longer leads its shard answers empty with `moved`, so the edge forgets the leader and asks the broker the next lookup finds within the same poll.

**Why.** BF-55 needs an `Owner` scope, and none existed: the admin route registered only `Repo` runners, and `match` read only its own shard. A match on another shard cannot use `Job { Matched }` from the runner, because that shard's `decide` checks the runner's record, which only shard 0 has. Carrying the labels in an operator command keeps `decide` pure and needs no cross-shard `deps` read, a shape the group has never had. The resend and `moved` rules are what a killed broker site needs: without the resend, a match committed just before the kill sits `Assigned` until its 10-minute lease ends, which is BF-55's fail column (dispatch stalled over 60 s); without `moved`, polls keep reaching the old site's broker until the edge's cached leader expires.

**Beat.** A `deps` read of shard 0's runner record from the job's shard, which makes every remote match depend on shard 0's leader answering. Registering an `Owner` runner's record on all 16 shards, which multiplies registration writes 16 times and must keep 16 copies in step. One broker object per (owner, scope) as DESIGN keys it, which needs the scope in every runner token; the per-shard object already serves both scopes (DECISIONS, "M4-WP3: the run service's signing key").

**Evidence.** `crates/actions/tests/bf55.rs`: 24 `Owner` runners, a 256-job matrix over two repositories on two shards other than 0, site 0 killed after 80 jobs, where every group led, with one match's answer lost. Polls reach a broker on the new leader site, every job is matched once and acquired once, and no gap between dispatches passes 60 s. `crates/actions/src/broker.rs` test (oldest fitting job over the shards; resend at 30 s and not at 29.999 s). Plants 41 to 45 in `tests/m4/wp3/plant.py`, all caught. BF-55 on graft-m4a (`tests/m4/results/m4-wp3.md`).

**Flip.** A runner whose `acquirejob` after a delivered match takes more than `RESEND_AFTER`, which would make the resend race it; GHES's org-runner registration flow landing, which would replace the admin route's `scope` with GHES's own route to the same record.

**Amended after BF-55 run 1, 2026-11-15.** Run 1 left four jobs `Assigned` to ephemeral runners that had already exited, and its summary reads held polls for 20 s while a site was down. Three rules fix it (`graftyard_actions::broker::Taken`, `workers/site/src/broker.rs`). (1) A broker never matches an ephemeral runner it matched or saw holding a job: after a leader move the new site's broker never matched the runners already running, and each runner's last poll is still held when its job ends. (2) It matches only when every shard with a group was read in that poll: the in-memory gate, with ephemeral runners, then showed a broker on the new site matching a runner on another shard while the shard holding its lost-answer match had no leader yet, which left that job until its lease ended. (3) Each summary read is cut at `SUMMARY_WAIT` (3 s); a shard whose read failed counts as having no group only when a majority of its members answer "not joined", which a group holding jobs cannot have. Each poll also reads the registration shard before it matches, so a runner whose record was deleted while its poll was held is not matched. The simpler choice beat keeping the matched set in the group: no new command or format, and a broker that restarts also drops the polls the set guards. Evidence: plants 46 and 47 caught by `bf55.rs` and the `broker` unit test.

### M4-WP4: how hosted runners register, count against the cap, and go stale, 2026-11-15

**Decision.** (1) Every hosted runner registers at `RunnerScope::Owner` on `OWNER_SCOPE_SHARD`, a user's as an org's, with exactly one hosted label row's labels, and `decide` refuses the registration `TooMany` once the owner has `HOSTED_CAP` (20) hosted runners, counting the entry's earlier commands (`crates/actions/src/hosted.rs`, `admit`). (2) The registration is Graftyard's own `POST /api/graftyard/v1/owners/{owner}/actions/hosted/runners`, which takes the runner's public key; `graft-runners` generates the key pair and writes the JIT config itself (`runsvc::jit_config`), so the runner's private key never leaves the runner account outside its own Sandbox. (3) A hosted runner is stale when it held a job and holds none now, has held none `HOSTED_IDLE` after the broker first saw it, matches no row, or is idle beyond its row's queued jobs (newest first). The times (`Seen`) live in the broker object's storage, read before and written after each demand read. (4) A non-self delete of a hosted runner that holds a job is refused, and `graft-runners` destroys a stale runner's Sandbox only after its delete commits. (5) The App's installations come from the `HOSTED_OWNERS` variable until the install serves `GET /app/installations`.

**Why.** (1) The cap is one group's rule only if one group sees every hosted registration; a user-owned repository's `Repo` scope would spread them over 16 shards. (2) GHES's `generate-jitconfig` answers a config built from a key the server holds, and the install never holds a runner's private key (M4-WP3's runner registration takes the public key the same way). (3) Without the last rule, two overlapping demand reads near the end of a queue each start runners for the same jobs, and the spares live out `HOSTED_IDLE`; with it, BF-60 in memory left no Sandbox after the last job. (4) A demand read is stale by the time a pass acts on it: a runner listed idle can take a job before its delete, and destroying it would fail that job at its lease end.

**Beat.** `start` naming each user-owned repository's scope, SHARED's first wording, with the cap counted across 16 groups. `generate-jitconfig` with a server-held key. Staleness kept in the fold, which would add a stored field written on every match. Destroy first, then delete, SHARED's first order.

**Evidence.** `workers/runners/tests/bf60.rs` (BF-60 in memory: at most 20 registered and 20 Sandboxes, every job `Success`, none left after 10 minutes, and a stale runner matched after the read keeps its job). `tests/m4/wp4/plant.py`, 17 of 17 caught. `tests/m4/results/m4-wp4.md`.

**Flip.** An `OrgPolicy` cap field, which `admit` would read in place of `HOSTED_CAP`. A user-owned repository whose hosted jobs must run on `Repo`-scope runners for isolation, which would need the cap counted across shards. A broker restart rate that makes `HOSTED_IDLE` from first sight too slow to free the cap.

**Amended after the m4a smoke run, 2026-11-15.** (3) first kept `Seen` in the broker's memory, as its other state is derived. On graft-m4a, runner 1704299 registered at 16:40:51Z, its Sandbox failed to start (the `Reflect::apply` bug), and it never polled; demand then listed it idle against the one queued job, so every pass started nothing, and it was still not stale at 17:13:52Z with reads every 5 minutes or every 34 s. With reads 6 to 11 s apart from 17:22:37Z it went stale at 17:32:42Z, 10 minutes later, and the 17:35:50Z sweep deleted it, started runner 1704300, and the job finished `Success` at 17:36:22Z [measured, `tests/m4/results/m4-wp4.md`, "The smoke run"]. So the object left memory between reads 34 s or more apart [inference], as Cloudflare documents for an object idle about 10 s [doc, Durable Objects, "Lifecycle of a Durable Object"], and every such read saw each runner afresh: a runner whose Sandbox never starts held a cap slot and its job's start for good. `Seen` is now one JSON value in the object's storage (`workers/site/src/broker.rs`, `SEEN_KEY`), one storage read and one write per demand read. *Beat:* `first` from the registration's commit time in the fold, which needs a stored field on `RunnerRecord` and a format change; keeping the object in memory with an alarm, which costs duration and still loses `Seen` on a deploy or a move. *Evidence:* `hosted::tests::seen_ages_runners_across_stored_reads`, which fails when `first` is not stored (`tests/m4/wp4/plant.py`, plant 21, caught). *Flip:* a broker that serves demand for many owners at once, where one stored value per read becomes a write rate worth batching.

### `fresh-repo`: a new group's near placement runs after its create returns, 2026-11-13

Branch `fresh-repo` traced LM-1's two fresh-repository gaps on m23 (`tests/am1/results/fresh-repo.md`).

#### A create starts its group's near placement and does not wait for it

**Decision.** `graftyard_lookup::create` sends the new group's leader `/place` with `{"background": true}`. The leader answers `{"placing": true}` at once, or `notLeader`, and runs the same near check (`place_inner`: up to `PLACE_TRIES` re-placements, the nearest kept) under `wait_until` after it answers. It holds the daily check's `near_check` marker while it runs, so a second ask, or the daily check, does not start another placement of the same group. A create therefore returns once a member leads, and the group serves while it is placed, as it already does through a daily re-placement. `PLACE_FOR` stays 240 s, so an edge built from this branch still works with a member built before it, which ignores the field and places before it answers.

**Why.** On `graft-m2b` and `graft-m3c` the enam to weur pair measures 105 to 136 ms, over enam's 100 ms `NEAR_RTT`, so every new group spent all 8 tries. Each try's join measures 20 round trips to each peer, so a group took 60 to 95 s to place. A fresh repository can make two new groups, its `Names` shard and its own, so its create took up to 141 s [measured, `tests/am1/results/fresh-repo.md`]. LM-1's 503 "0 of 3 joined … has no RepoOwner row" was a `GET /repos` sent while its create was still placing the `Names` shard group, before `RepoOwner` committed: every member checks that row before it joins, so all three refused. Its missing `main` was `tests/am1/runs/mkrepos.mjs` taking a re-sent create's 422 plus a 200 GET as a finished create while the first create was still placing, and in one case after the client had given up on it at 180 s, so `push_init` had not run. No acknowledged write was missing from a read [measured, Workers Logs, same file]. The `m0-carried-4` amendment already made the bound "a goal, not a gate". Nothing about the create's result depends on the placement, so the create no longer needs to wait for it.

**Beat.** Answering a read of a repository with no `RepoOwner` row with 404 instead of trying to join, as the only change. It does not shorten the create, and a 140 s create also outlasts clients' timeouts, as it outlasted mkrepos'. It is still open as a second change: after this one, a `GET /repos` sent while a create runs answered 503 "0 of 3 joined" on 35 of 129 such reads, in the 10 to 30 s before `RepoOwner` commits [measured, `tests/am1/results/fresh-repo.md`]. No read after a 201 did. Fewer `RTT_SAMPLES` or `PLACE_TRIES`, which only shorten each try, and leave a create waiting on several. A bound relative to each install's baseline, which the `m0-carried-4` amendment rejected.

**Evidence.** `tests/am1/results/fresh-repo.md` [measured]. `tests/am1/fresh/fresh.mjs` creates repositories back to back, never re-sending a create. It fails on main's sites (a create over 60 s) and passes on this branch's. No cargo test can reach the change: it is the order of two Durable Object calls, and only workerd runs the site's members.

**Flip.** A group whose background placement leaves it `Degraded` or without a leader, which `fresh.mjs` checks after each run. Or commit latency on new groups during the placement window that some caller cannot take.

#### A read before `RepoOwner` commits answers 404

**Decision.** A repository member asked to join reads the `RepoOwner` row through `Names`. When the `Names` leader answers with no row, the member refuses with `{"refused": {"noRepoOwner": id}}`, distinct from a read that failed (`{"source": ..}`). `graftyard_lookup::create` names `NOT_CREATED` in its error only when a majority of the bootstrap members refused that way. The edge's repository reads that create the group on first use (`tier_rows`, `api_rows`, and the read gate's `core_rows`) answer that error 404 on REST, the GraphQL `NOT_FOUND`, and git's "Repository not found." The read gate returns `Hidden` without caching it. Every other failure stays 503, including a member that holds the row but has not joined yet.

**Why.** After the change above, a `GET /repos/{o}/{r}` sent while its create ran answered 503 "0 of 3 joined" on 35 of 129 such reads, in the 10 to 30 s before `RepoOwner` commits [measured, `tests/am1/results/fresh-repo.md`]. The repository does not exist for any caller in that window. GitHub answers 404 for a repository no one can see, and 503 tells a client to retry against an outage. A majority is required so that a single member whose `Names` read raced the commit cannot hide a repository that exists.

**Beat.** Reading `RepoOwner` on the edge before every first-use create, which adds a `Names` read to a path that only needs it when the create fails. Treating any "0 of 3 joined" as 404, which would make an outage look like "not found".

**Evidence.** `crates/lookup/src/lib.rs`, `only_a_majority_with_no_repo_owner_row_is_not_created`; the hosted rerun in `tests/am1/results/fresh-repo.md`, "Follow-up: 404 before `RepoOwner`".

**Flip.** A 404 for a repository whose `RepoOwner` row had committed, or members of an older release whose refusals keep the window at 503 after a site day.

#### `mkrepos.mjs` sends a create once

**Decision.** `tests/am1/runs/mkrepos.mjs` sends `POST /orgs/m3org/repos` once with a 400 s timeout. It no longer re-sends the request through `api`, and it no longer counts a 422 followed by a 200 GET as a create.

**Why.** That fallback started LM-1's runs on repositories whose create was still placing (131317) or had been abandoned (131320).

**Beat.** Keeping the fallback and waiting for `main`, as LM-1's `a1.mjs` does, which hides a create that never finished.

**Evidence.** `tests/am1/results/fresh-repo.md`, "Bug 2".

**Flip.** None needed. A driver that must survive a lost answer should read the repository's `main`, not its settings.

### `liveness-split-vote`: a voter that granted a rival's pre-vote waits before its own, 2026-11-14

#### A pre-vote grant holds back the granter's own campaign

**Decision.** Two rules in `crates/raft/src/member/election.rs`. Pre-vote grants and refusals are otherwise unchanged.
- A voter that grants a rival's pre-vote starts no campaign of its own for one CALL_DEADLINE (1 s) from the first grant for that asked term (`pre_granted`). A repeated pre-vote for the same term does not extend the wait.
- A voter whose own pre-vote is out when it grants a rival's drops its own (`lose_campaign`, with its usual backoff). A real campaign it already started is kept.

**Why.** `long_liveness`'s writes-fail target (every group moved within 20 s) missed seed 14011 on main (21.5 s) and seed 3052 on `m0-adaptive-append` (21.9 s). The trace of 14011: with site 0's writes failing, followers 1 and 2 lost their promises together, and the same `suspect` reports, which every edge sends to both, started both within 31 to 117 ms, three terms running (4, 5 and 6). Each granted the other's pre-vote: site 2's reached site 1 just before the report that started site 1, and a campaigning member denies only a higher id as `Contending`, so site 2 granted site 1. Both entered the real vote and each refused the other as `Voted`. Site 0 never answered in time, so each campaign ran its full CAMPAIGN_DEADLINE (3 s), lost, backed off 100 to 400 ms, and the next report started both again [measured, simulator, `tests/m0/results/split-vote.md`]. The random backoff does not separate them because the reports, not the backoff, set when they start. The first rule alone fixed 14011 and 3052 but, over 20,000 seeds, made two other seeds fail that pass on main: 15198 (3 leader changes) and 21945 (22.2 s). In both, each follower's own pre-vote was already out when it granted the other's, so the wait did not apply; the gray leader, still a follower answering pre-votes, granted both, and the two real votes split. The second rule closes that path.

**Beat.** A longer or wider random backoff: the restart comes from the reports, which arrive at both voters together, so a backoff only delays both. Denying a pre-vote while the voter has granted one, which needs a second refusal kind on the wire. Letting a lost real vote end before CAMPAIGN_DEADLINE when every answer is `Voted`, which shortens each failed term but does not stop the next split.

**Evidence.** `a_voter_that_granted_a_rivals_pre_vote_lets_it_win` and `a_voter_that_grants_a_rivals_pre_vote_drops_its_own` (`crates/raft/tests/scenarios.rs`), each failing without its rule. `tests/m0carried/plant10.py` plants seven mutants of the two rules. The writes-fail scan, the long sweep and Porcupine are in `tests/m0/results/split-vote.md`.

**Flip.** A history in which the wait delays an election past the liveness bound, for example a rival whose pre-vote passes and whose real vote never arrives, repeated each term; or a writes-fail seed over 20 s that is still a split vote.

### `liveness-slow-storage`: slow storage does not move leadership it cannot cure, 2026-11-14

#### A follower with slow storage does not depose a leader still appending, and no slow follower is a STEP_DOWN target

**Decision.** Two rules in `crates/raft`. F67's `suspect` counting and pre-vote refusals are unchanged.
- A follower whose own storage would trip `STEP_DOWN`'s confirm rule (over the window, at least `confirm_slow_min` confirms over `confirm_p99_ms`, ended or still running, and the p99 over it: `storage_slow`) starts no campaign while a leader's AppendEntries reached it within CAMPAIGN_DEADLINE (`appended_at`, `election.rs` `try_campaign`). The 2 x ELECTION_AFTER waiver does not lift it, since the leader is heard.
- A leader picks no `STEP_DOWN` target whose AppendEntries answers in the window ran over `confirm_p99_ms` past its fastest at least `commit_fail_min` times (`peer_storage_slow`, `health.rs` `check_health`): a follower answers after its own storage confirms, so its slow confirms show in its round trips. With no target left the leader holds, as before.

**Why.** LM-1's k-4 churn, which a read-only reviewer reproduced in the simulator on a7d28e5 and 84d29c2 with slow confirms on every member. On main a21ecdf, 30% of confirms at 1.05 s for a minute raised the term by 4.3 a minute on average with all three slow, and 3.8 with both followers slow, over 60 seeds each (up to 7 in one seed) [measured, simulator, `tests/m0/results/slow-storage.md`]. The traces showed two paths: a confirm-tripped leader handed off to a follower just as slow, which tripped the same way within one window; and a follower whose promises `suspect` withheld campaigned, passed pre-vote, and its real vote, sent after its own slow confirm, raised every voter's term and deposed the leader even when it lost on `LogBehind`.

**Beat.** The reviewer's three candidates. (a) skipping a target by its reply RTT over `confirm_p99_ms`, which this keeps in a softer form, as an excess over the follower's fastest answer, because the far follower's RTT alone is near that line. (b) counting a `suspect` report only if the leader's commit index stopped at the follower, which changes F67's detection of a gray leader that still commits slowly. (c) ignoring a higher-term RequestVote while a live leader's append arrived within the election timeout (Raft thesis section 4.2.3), which also blocks the healthy follower that F67 relies on to move a gray leader. The gate here is the candidate's own storage, so a healthy follower still campaigns.

**Evidence.** On 200 seeds each, the term rose by 1.3 a minute on average with all three slow and 0.3 with both followers slow, against 4.3 and 3.8 on main (60 seeds); no seed was leaderless for 20 s. A gray leader with one slow and one healthy follower is moved through the healthy one; a dead leader is replaced by a slow follower CAMPAIGN_DEADLINE after its last append; a half-hung leader with slow followers is replaced within F67's bound (p50 12.6 s, max 17.0 s over 60 seeds, as on main), because check quorum ends its leadership and so its appends. Kept tests: `slow_storage_everywhere_keeps_one_leader` and `slow_storage_on_both_followers_keeps_the_leader` (`crates/raft/tests/liveness.rs`), both failing on main; `a_slow_follower_holds_back_while_a_healthy_one_moves_a_gray_leader` and `a_slow_follower_still_replaces_a_dead_leader` (`crates/raft/tests/scenarios.rs`); `a_half_hung_leader_with_slow_followers_is_replaced` (`liveness.rs`). `tests/m0carried/plant11.py` plants the rules.

**Flip.** A gray leader that F67 moved on main and that this keeps leading past 20 s; a slow follower held back while no leader commits; or hosted evidence that a follower's answer time does not track its storage.

### One `MetaSlim` entry looks at no more than 128 KiB of rows (2026-11-14)

**Decision.** A `MetaSlim` entry slims the next rows after its cursor until it has looked at `SLIM_ROWS` (512) rows or until the row that brings the keys and columns it looked at to `SLIM_BYTES` (128 KiB), whichever comes first (`crates/git/src/split.rs`, `slim`). The next entry starts at the row after the last one it looked at, in the same table. Pacing ("A leader paces its own background writes") is unchanged.

**Why.** Smoke 2 on `repo:1048587` (seeded with about 1.8 KB comments) paced each `MetaSlim` by twice its commit, and the leader still stepped down 11 times in 9 minutes on `CommitErrors` while the slim ran, against 0 before it and after it. Pacing spaced the entries but did not shorten them: each 512-row entry rewrote about 0.9 MB of rows, and the followers' `/raft` calls in that window ran at p50 0.65 s and p90 1.8 s, 14 of 44 sampled over `CALL_DEADLINE` (1 s), against p90 0.72 s with none over it before the slim [measured, Workers Logs, `tests/am1/results/b030-split.md`, "Smoke 2"]. Missed deadlines on both followers are exactly what `STEP_DOWN`'s `CommitErrors` counts. A follower's call time grows with the rows an entry rewrites, so the bound that keeps it inside `CALL_DEADLINE` is on bytes, not on rows.

**Beat.** A larger `PACE`: the time between entries does not change how long one entry's apply holds a follower's `/raft` call. Fewer rows (`SLIM_ROWS` 128): metadata rows vary from a few hundred bytes to the 64 KiB body limit, so a row count bounds nothing. Excusing deadline misses on a follower's background apply in `STEP_DOWN`: a `crates/raft` change for a cost a site can avoid.

**Evidence.** `a_slim_entry_stops_at_its_bytes_and_the_next_takes_the_rest` (`crates/git/tests/split.rs`); plants 25 and 26 in `tests/am1/b030split/plant.py`; smoke 3 in `tests/am1/results/b030-split.md`.

**Flip.** Follower `/raft` calls over `CALL_DEADLINE` during a slim at 128 KiB entries, or a slim pass so long that the parent passes `CHILD_GROUP_AT` before it ends.

### `admission-stored-bytes`: admission prices a command by what its entry stores, 2026-11-14

**Decision.** Admission and the entry cut price a command by `Command::stored_len`, about what its events add to the entry, in place of its encoded length. By default the two are the same. A repository's issue command leaves out its `render`, the reply its leader renders from the `&Acked`, which no event stores. COMMAND_MAX and ENTRY_MAX still count the whole encoded command, so the bytes in flight stay bounded.

**Why.** On B0-33's smoke (m26c, main 2456b8f) admission priced every agent command at about 10.8 KB (agent p50 10,842 B), so capacity was ENTRY_SOFT over 10.8 KB a round, 30 to 47 commands/s against the 140 line, and agentQueued sat at AGENT_QUEUE_MAX. The leader's 505 confirm transactions stored p50 1,910 B for 7.6 commands an entry [measured, LM-1 round 15, `tests/am1/results/lm1-memory.md` on `lm1-memory`]. With the real codec over m3org's Authz fold, an issue command's render was 10,262 to 10,426 B (the owner's AuthzFold as hex, 9,974 B for a comment) and the rest 538 to 635 B; an agent push is about 405 B without its pack and has no render [measured, LM-1 round 15b].

**Beat.** Pricing an entry's events after `decide`: the cut picks commands before they decide, so it would need a second pass. Shrinking the render the edge sends: a separate change, which also cuts the AppendEntries bytes every follower receives (below).

**Evidence.** `b0_33_with_hosted_render_bytes_admits_140_agent_commands_per_second` (`crates/raft/tests/admission.rs`): B0-33's mix with a 10.3 KB render on every side command, which stores about 550 B, at round p50 225 and 315 ms, admits 252 and 250 agent commands/s with none refused, and no entry stores over ENTRY_SOFT past its first command. On main's pricing it admits 23/s with capacity 46/s. `an_issue_commands_render_is_not_priced` (`crates/git/tests/agent_bytes.rs`) holds the issue command's pricing. `tests/m0carried/plant12.py` plants the rule.

**Flip.** A hosted B0-33 run whose entries store more than ENTRY_SOFT past their first command, or whose confirm p99 rises with the entries this admits.

**Open, for a second change: dropped, 2026-11-14.** An earlier version of this paragraph said the render crosses every AppendEntries, about 80 KB of renders per B0-33 entry against 1.9 KB stored, and so bears on the far-leg p99. That is false: the leader builds each entry from the events `decide` returns (`LogEntry { body: EntryBody::Events(events) }`, `crates/raft/src/member/leader.rs`), no `RepoEvent` holds the render, and a follower receives events, not commands [code-read]. The render travels only in the edge's command to the leader, about 10 KB per issue command. Having the leader build `HostedDir.authz` from its own `Authz` dependency would save that request body but cost a 10 KB fold clone per command out of `decide`, and the reply would render from the leader's read of the fold, not the edge's, so a grant that changed between the two reads changes the reply. The operator declined it (option (a)). `ENTRY_MAX` still counts the render on the leader's cut: `decisions/OPEN-QUESTIONS.md`, F70.

### `m4-web-3`: a single-comment review leaves an APPROVED or CHANGES_REQUESTED verdict as it is (format 32.22), 2026-11-14

**Decision.** A REST review comment outside a review commits its single-comment review as `ReviewEvent::SingleComment` (tag 3) in repository format 32.22 (`REPO_FORMAT_REVIEWS`). Its fold writes the reviewer's core `review_verdict` only over none or a COMMENTED one, so every such commenter gets a review and an APPROVED or CHANGES_REQUESTED verdict stays. The edge marks the comment 32.22 once `FormatsEnable` enabled it; before that, decide writes a `Submitted` and gives a commenter who holds such a verdict no review. 32.20 and 32.21 are other branches' (`b030-split`, `m4-actions`).

**Why.** GHES gives every review comment made outside a review a COMMENTED review and keeps the reviewer's approval: `reviewDecision` and merge protection read the approval, not the comment. A `Submitted` COMMENTED replaces the verdict in the fold, so the pull request lost its approval, or kept the commenter's comment without a review (`m4-web-2`).

**Beat.** Changing how `Submitted` COMMENTED applies, which would change what every earlier entry folds to and so every chain's digest at the next refold. Skipping the review for such commenters, which leaves `pull_request_review_id: null` where GHES sets it.

**Evidence.** `an_approvers_review_comment_gets_its_review_and_keeps_the_approval` (`crates/git/tests/agent_gates.rs`) through real members, which fails when the fold replaces the verdict; hosted single-comment reviews on `m3org/web38-a` PR #3 (`tests/m4/results/m4-wp5.md`).

**Flip.** GHES 3.22 replacing an approval with a later single-comment review, or a reviews UI that needs a COMMENTED verdict beside an approval.

### `liveness-looplag-handoff`: a follower judges its own storage slow at `commit_fail_min` slow confirms, 2026-11-14

**Decision.** `storage_slow`, the campaign gate `liveness-slow-storage` added, counts a follower's own storage as slow at `commit_fail_min` (3) confirms over `confirm_p99_ms` in the window, with the p99 over it, in place of the leader's `confirm_slow_min` (6). The gate itself, `peer_storage_slow` and F67's `suspect` counting are unchanged.

**Why.** LM-1's round 15c on m26c: a fresh leader, the target of a LoopLag hand-off, lost its term about 13 s in with no StepDown logged. The simulator reproduced it with LoopLag on the leader and slow confirms on both followers: over seeds 900 to 1099, 18 went over 5 s with no member serving, worst 12.7 s. In seed 910 the hand-off worked (term 3). Then a follower with 4 of 11 confirms at 1.05 s in the window read healthy, passed pre-vote, sent its real vote after its own slow confirm, and deposed the leader on the higher term. It was refused LogBehind, and nobody led for 12.5 s. A follower confirms about once per append it accepts, so it rarely reaches the six a leader's confirm rule counts [measured, simulator, `tests/m0/results/looplag-handoff.md`].

**Beat.** Ignoring a higher-term vote while a leader is live: the operator rejected it, since it blocks the healthy follower F67 relies on. Lowering `confirm_slow_min` for leaders too: that moves STEP_DOWN's own measured line (B0-06).

**Evidence.** `a_looplag_leader_with_slow_followers_hands_off_and_holds` (`crates/raft/tests/liveness.rs`) fails on main `1f51c6f` (seeds 904 at 8.5 s and 910 at 12.5 s unserved) and passes here, worst 4.9 s. Over 200 seeds, seeds unserved for more than 5 s went from 18 to 4, and the worst from 12.7 s to 5.6 s. The slow-storage term rise over 200 seeds is 0.94 a minute with every member slow and 0.20 with both followers slow, against 1.33 and 0.30. `long_liveness` at 60 seeds moved all 60 by STEP_DOWN for calls, slow and lag. `tests/m0carried/plant11.py` gains a mutant that restores six.

**Flip.** A follower that the lower count holds back while no leader commits, or a gray leader F67 moved that this keeps leading past 20 s.

### The archive drain keeps up with twice B0-30's pushes: 8 ranges a promotion, 8 objects an archive entry, and promotion ahead first (2026-11-14)

**Decision.** A promotion takes up to `PROMOTE_RANGES` (8, was 4) archive ranges of log. One archive step puts up to `ARCHIVE_PIECES` (8, was 1) objects of `ARCHIVE_RANGE_MAX` each, one after another, with both bucket puts of each object at once, and records them all as `Archived` events in one entry. Members truncate through each in order (`Family::archived` returns every range). A promotion goes before a due archive once the log above the last `Promoted` passes `LOG_MAX_BYTES / PROMOTE_AHEAD`, unless the log is over its cap by more than `LOG_MAX_BYTES / ARCHIVE_SLACK`, when archiving goes first. A promotion reads its range in pieces of `PAUSE_EVERY` (2 MiB) and yields one timer turn between them, so its decode and hash never hold the leader's loop for the whole range. A promotion waits `PROMOTE_NEXT_WAIT_S` (10 s) on a shard that has another site's shard after it, and `PROMOTE_WAIT_S` (60 s) only on its last try (`crates/git/src/log_archive.rs`, `crates/git/src/shard.rs`, `workers/site/src/archive.rs`).

**Why.** B0-30's rerun on `repo:1048590` stopped at simulated day 1.82, when the watcher's 5% rule tripped at 2,257,289,087 B of log [measured, `tests/am1/results/b030-split.md`, "B0-30 rerun"]. The drain ran one `Promoted` of about 25 entries and then five `Archived` of about five entries each, every 13 to 15 s. That archived about 134 entries a minute against 132 to 134 applied, so it had no margin. Each step's cost is mostly its entry: a commit of about 450 ms, paced twice that again (`host::paced`). A step that carries eight objects pays for one entry where the old rule paid for eight. Archiving went before promoting ahead, so each promotion ran just ahead of the archive, and the log above it grew from 0.29 GB to 2.07 GB, the whole tail a cold shard's rehydrate reads. At the first leader change (12:49:02Z, `LoopLag` on site a), the new leader's first `/archive/promote` took 61,725 ms. Its call to site 0's shard waited 59,992 ms, the full `PROMOTE_WAIT_S`, on a catch-up that had not finished, before it tried site a's shard. That one promoted and validated in 455 and 369 ms [measured, Workers Logs]. `next=1` answered at once only while the shard was rehydrating, not while it was behind.

**Beat.** More promotions per pass at the old sizes: each still pays an entry. A larger `ARCHIVE_RANGE_MAX`: the object is built in memory beside a member's state, which a 4 MB batch took past 128 MB (`ARCHIVE_RANGE_MAX`). Faster pacing: it is what keeps followers' `/raft` inside `CALL_DEADLINE` (the pacing entry above). Answering 503 at once on any shard that is behind: a warm shard is a fraction of a second behind under load, and every promotion would then skip its own site's shard.

**Evidence.** `crates/git/tests/log_archive.rs`, `the_drain_outruns_twice_the_rerun_s_log_rate_at_its_step_costs`. On real members, it drives a scaled B0-30 log through a cost model fitted to the rerun's step times. Pushes arrive at twice the row's rate (1.03 MB/s), and the log stays within its slack. A backlog drains at 1.71 MB/s. At 4 ranges and 1 object it drains at 0.53 MB/s, the log reaches 1.57 times its cap, and the test fails. At 8 ranges and 1 object it drains at 0.64 MB/s and also fails. `a_large_promotion_reads_its_range_in_pieces_and_names_every_pack_once` checks the paused read. `a_shard_answers_warming_at_once_only_to_a_caller_with_somewhere_else_to_go` checks the wait. Plants 27 to 36 in `tests/am1/b030split/plant.py`. The cost model's split of a step into commit and puts is [inference]. The hosted resume measures it.

**Correction after the first resume (2026-11-14, 17:23Z to 17:30Z).** The first site day of this rule cut only an archive step's first piece against the cap less the slack. Once that piece took the log under the cap, `due` targeted the cap itself, and nothing past it was due. At the row's own push rate the log is over its cap by about one range a step, so every step after the backlog carried one object. Measured from 17:26:17Z to 17:29:01Z on site 0: 125 entries a minute archived and 77 promoted, against 129 applied, with the unpromoted tail rising from 57 MB to 108 MB. The first 90 s, through the 2.07 GB backlog, ran at 8 objects a step. Every piece of a step is now cut against the target its step started with. The kept test sweeps 0.5 to 2 times the row's rate. At each rate the drain must be idle at least 0.8 of `1 - r/2` of the time, and `promotedTo` must keep pace with the log. With the old cut it idles 16% at the row's rate against the 40% required, and the test fails (plant 37).

**Correction after the second resume (2026-11-14, 18:32Z to 18:40Z, simulated day 1.905 to 2.005).** The drain archived 1.31 times the applied log, against the 1.5x rule, and the log stood at 2.155 to 2.201 GB, over `LOG_MAX_BYTES` throughout. Two steps went `Idle` with the log over its cap: at each, the step's own total was about 5 MB and 8 MB under the cap, and members held only 0.3 to 10.6 MB of rows above `archivedTo` [measured, `tests/am1/results/b030-split.md`, "B0-30 rerun"]. Idle was right by the old rule, which held the slack per step: a step started only over the cap and its target fell back to the cap as soon as it was under. An idle leader then sums its log once a `SUM_EVERY_MS` (60 s), and about 40 MB arrived in that wait. The archive now has a hysteresis band per run (`log_archive::Run`). A run starts once the stored log passes the trigger, the cap less `cap / ARCHIVE_SLACK / 2`. It archives down to the floor, the cap less `cap / ARCHIVE_SLACK`, and holds that target across its steps until one archives all the way to the floor. Promoting ahead goes first only until a run passes its trigger. `ARCHIVE_SLACK` went from 16 to 8, so the half-band is 134 MB against 72 MB a minute at twice the row's log rate (113 entries a minute of about 320 KB, doubled; `DESIGN_LOG_BYTES_PER_S`, 1.2 MB/s). The site asserts that the half-band exceeds one `SUM_EVERY_MS` plus `BUSY_MS` of that rate. The kept test `the_log_stays_under_its_cap_through_the_sum_wait` starts just under the trigger and models the sum wait at 0.5, 1, 1.5 and 2 times the row's rate. At each rate the log must never pass `LOG_MAX_BYTES`, and every run that passed the trigger must reach the floor before `Idle`. Its peaks are 0.94 to 0.97 of the cap. On e9418153's rule (plant 38) the log passes the cap. Plants 39 to 41 cover the hold, the promote-ahead rule, and the end of a run (a run that ends after any archive step).

**Flip.** Hosted, the drain archives less than 1.5 times the applied log rate while it has work. A pause between pieces that lets a promotion run past `STEP_FOR`. Or a shard whose catch-up under load routinely outlasts 10 s, so every promotion falls through to another site.

### `memory-budgets` (1): a leader keeps resolved tickets by bytes and age, 2026-11-14

**Decision.** `Lead::done`, the resolved tickets a leader keeps for a late `outcome`, holds at most DONE_KEEP (4,096) of them, DONE_BYTES (8 MB) of the bytes their commands carry that no event stores (`encoded_len` less `stored_len`), and none older than OUTCOME_KEEP (15 s). `outcome` for a ticket pruned from there answers `NotLeader` naming the same leader; the edge then asks `outcome_by_request`, which answers a committed command from its `request` row through `outcome::replay`, so `Acked` is still built only inside `outcome`.

**Why.** An issue or merge command's outcome keeps a copy of its reply render, about 10 KB of the owner's Authz fold as hex plus logins, and nothing counted it. With a render shaped like m3org's, 8,192 issue commands left 60.6 MB more of a leader's live heap than the same commands without renders, flat from 4,096 on [measured, native allocator]. On B0-33 run 2 site 0's wasm heap climbed from 9.7 to 95.9 MiB in about 90 s and stayed there to its reset, while memSplit's counted buffers stayed under 10 MB [measured, LM-1 round 17, memguard]. That the renders are that heap is an inference: wasm32 values are smaller than native ones.

**Beat.** Dropping the render from the outcome: the edge renders the reply from it, so the reply bytes would change. A lower DONE_KEEP: a burst of small commands would lose outcomes an edge still asks for, while big ones would still fill the heap.

**Evidence.** `thousands_of_rendered_issue_outcomes_hold_about_done_bytes_of_the_leaders_heap` (`crates/git/tests/done_memory.rs`): 11.3 MB held by the renders here against 60.6 MB on main (its bound is DONE_BYTES times a `Value`'s 1.5 overhead). `a_late_outcome_after_its_ticket_was_pruned_is_answered_from_the_request_row` (`crates/raft/tests/scenarios.rs`) asks once past OUTCOME_KEEP and once past DONE_BYTES, and gets `NotLeader` naming the leader, then the committed index from the row; it fails with the age and byte rules removed. `tests/m0/results/memory-budgets.md`.

**Flip.** An edge asking `outcome` for a ticket more than 15 s after it resolved, or `outcome_by_request` answering `unknown` for a committed command.

### `memory-budgets` (2): a leader bounds the AppendEntries bytes alive on its calls, 2026-11-14

**Decision.** A leader counts the entries' bytes on each AppendEntries call it sent, from the send until the call's answer, an error, or the host's new `call_ended(to, round)` input, which the host sends when it stops awaiting the call (answered, failed, or LATE_LIMIT past its deadline) and when a message held for a learner leaves in that learner's own response. It sends a call only while the peer stays within PEER_OUT_MAX (2 MB), the learners together within LEARNERS_OUT_MAX (4 MB), and every peer within OUT_MAX (8 MB). A voter with no call alive always gets one. An unreported call stops counting after OUT_KEEP (12 s). Rounds never repeat within a boot, so an earlier leadership's calls stay keyed apart. No wire enum changes.

**Why.** Nothing bounded the sum. The host awaits a missed call for LATE_LIMIT (10 s) while the core resends it at once and on each tick and pipelines up to PIPELINE more. m3f's new leader sent each voter 10.7 to 11.2 MB in its first minute against an 8 MB log, and B0-33's new leaders held 58.3 MB of call bodies and 42.9 MB of messages for learners before their isolates reset [measured, LM-1 round 17 and m3fr3].

**Beat.** A shorter LATE_LIMIT: late answers are what let a far voter catch up (F43). Fewer resends: a missed call must still be retried at once for commit latency.

**Evidence.** `a_silent_follower_holds_at_most_peer_out_max_of_the_leaders_calls`: with 1 MB entries and one silent voter, at most 1.0 MB alive to it against 10.0 MB without the budget, while the other voter commits all 40. `a_new_leader_catching_up_silent_learners_stays_within_out_max`: a new leader catching up 8 learners that answer 3 s late keeps 3.0 MB alive to them against 24.0 MB without it. `a_silent_follower_costs_the_commit_rate_nothing`: 400 commands commit in the step they are admitted, with one voter silent as with none. `tests/m0/results/memory-budgets.md`.

**Flip.** A hosted run where a healthy voter or learner falls behind with its peer's bytes at PEER_OUT_MAX, or a leader that held a call back past OUT_MAX while a quorum was answering.

### `memory-budgets` (3): a member sheds reads past ANSWER_BYTES_MAX of distinct answer bytes in flight, 2026-11-14

**Decision.** Every member, leader, follower, or learner, counts the distinct bytes of read answers still streaming out of it, an answer that many reads share (`family::dump_bytes`, `shared_answer`) counted once by the address of its shared bytes. While they reach ANSWER_BYTES_MAX (16 MB), any new read is shed `overloaded`, which the edge takes to the next member without `suspect`.

**Why.** READS_MAX (256) and HEAVY_READS (16) cap reads by count, not by bytes. A new leader had 22.9 MB of answers in flight one second after it won and reset 18 s later, and B0-33's leaders 26 to 30.5 MB [measured, m3fr3 and LM-1 round 17]. Counting a shared answer per read would shed a learner streaming one 1 MB dump to 16 readers, which `learner-dumps` showed costs 74% of reads.

**Beat.** A lower READS_MAX: narrow reads of a few KB would shed long before any memory mattered. Counting per read: see above.

**Evidence.** `distinct_answer_bytes_past_the_cap_shed_and_a_shared_answer_counts_once` (`workers/site/src/member.rs`): 40 reads of one 1 MB dump count 1 MB and serve; 15 more distinct answers reach 16 MB and shed a learner's read and the leader's; the cap lifts as they finish.

**Flip.** A hosted run whose reads shed at ANSWER_BYTES_MAX with the isolate's memory well under the limit, or an isolate reset with answers under it.

### `prsync-first-once`: a `PrSync` attempt ends at `FIRST_FOR`, and a test merge shed after waiting `TURN_WAIT_MS` in line, 2026-11-14

#### An attempt ends at `FIRST_FOR` in the invocation that runs it, and the leader waits for its answer

**Decision.** Every `PrSync` attempt, first or re-sent, runs as `attempt` (`workers/site/src/prsync.rs`), which drops the handler `one` at the attempt's deadline: `FIRST_FOR` (`graftyard_git::prsync::FIRST_FOR_MS`, 25 s) after it starts, or the first attempt's `deadline` from the leader if sooner. Merges are asked one after another through `graftyard_git::prsync::test_merges`, and none is asked in the last `COMMIT_RESERVE_MS` (10 s [guess]) of the attempt, or in the last half of a test install's shorter `prsyncFirstFor`. A merge not asked by then, or not answered by then, is shed into the batch's `unmerged`, so `mergeable` reads null and the attempt still commits its head moves and cursor. The leader waits `FIRST_FOR` plus `ANSWER_MARGIN_MS` (2 s [guess]) for the answer, under `REDELIVER_AFTER_MS`, so only after the attempt has ended does `Dispatcher::finished(false)` let the next First go out. `Dispatcher` is unchanged. `prsyncFirstFor` is capped at `FIRST_FOR` and now ends the attempt too.

**Why.** LM-1 round 16d on `graft-m2b` (`tests/am1/results/lm1-memory.md` on `origin/lm1-memory` `7a70479`). On B0-29 the leader ran 155 first attempts in 0.6 to 5.1 s, then from 10:51:31Z 19 in a row timed out at 25 s over batches growing from 20 to 2,411 entries. Each timed-out attempt kept running under `wait_until` while `unsent()` made its batch due again, so a new one started every 25 s. 14 overlapping invocations in one stateless isolate, each holding a `SyncRead` of up to 500 views and its merge answers, failed together with "Worker exceeded memory limit" at 11:00:53Z, at wall times of 36 to 486 s [measured]. The attempts stopped fitting because `one` asked its merges one at a time behind the shard's line [measured: the re-sends of 894..1035 took 172.5 s for 136 merges]. Ending the attempt bounds both how long it runs and what it holds, and the leader's wait means only one runs per chain.

**Beat.** Counting a timed-out attempt as running until it ends, with the attempt left uncut: that bounds overlap, but an attempt that never finishes holds the chain until `REDELIVER_AFTER_MS` counts it lost, and the re-send then runs beside it. Cancelling it at the leader's `FIRST_FOR` only: the attempt would still ask every merge, never move the cursor under a long line, and park the chain after `PARK_AFTER`. Bounding a batch's pull-request count: the batch must cover every entry from the cursor to `through`, so a capped batch needs a cursor that can stop mid-range, a format change; bounding by time sheds the same merges and keeps the batch whole.

**Evidence.** `a_batch_behind_a_long_merge_line_ends_inside_first_for_and_sheds_the_rest` (`crates/git/tests/prsync.rs`). It runs a 40-PR batch through `test_merges` and the shard's real `budgeted`, on a virtual clock, behind 30 merges of 1 s each. The merges stop by 15 s, every PR comes out merged or shed, and the cursor command still moves. With main's loop planted back (every merge asked, every turn waited out) its merges end at 41 s and the test fails. On main itself it does not compile, since main has no deadline to pass. `tests/firstonce/plant.py` catches 8 of 8 mutants. The site's wiring runs only in a Worker: `attempt` and the leader's wait are checked by compile-time asserts on the constants, and hosted by the next B0-29 row [unverified until then].

**Flip.** A hosted row where attempts end at `FIRST_FOR` with their cursor unmoved more than `PARK_AFTER` times in a row, so the commit reserve is too short. Or one where a cut attempt's commits land after its re-send's and undo them, which the content-addressed request ids should prevent.

#### A merge that waits `TURN_WAIT_MS` in its network's line is shed, as one past the budget is

**Decision.** In `graftyard_git::prsync::asked`, a merge whose network's turn comes more than `TURN_WAIT_MS` (10 s [guess]) after it joined the line (after its catch-up) is shed without running. It is counted in `shed` and in the new `BudgetStats::late`, and a GET keeps its coalescing slot. `mergeable` is then null, as past the budget (design/DESIGN.md, Shape, agent block, `MergeBudget`). `tests/am1/rest/a1.mjs` sums `late` in B0-35's window.

**Why.** The design's "over budget sheds, `mergeable` null" means the shard spends at most `TEST_MERGE_BUDGET` of its vCPU and drops the rest at once. It does not mean a merge waits minutes for its turn. On B0-35 (m27) the shard used 0.603 vCPU of a 1.0 budget with 0 shed, yet its line held 3,593 merges, waits reached 265 s with 3,143 of 5,378 over 120 s, and `mergeable` read `UNKNOWN` to 98.3% of polls [measured, LM-1 round 16d]. The turn runs merges one at a time at their wall time, so the line, not the CPU, was the limit, and nothing shed a merge for waiting. That wait also met the flip of "A28's flip, measured" (a merge wait past `GET_COALESCE_MS`).

**Beat.** A28's flip, admitting a second merge while one runs: it accepts an overrun of two and still leaves a line with no bound when merges arrive faster than two turns clear them. Coalescing on the queue, the flip "A28's flip, measured" proposed: it saves duplicate GETs, but a push's merges are of distinct heads and still queue. Shedding at admission by line length: the length does not say how long the wait is, and a wait does.

**Evidence.** The same test: of the 30 merges in line, those that waited past 10 s were shed when their turn came, and no merge ran after waiting longer (`wait_max_ms` at most `TURN_WAIT_MS`). `tests/firstonce/plant.py` mutants 1 to 3 and 7. `TURN_WAIT_MS` is checked at compile time to stay under `FIRST_FOR_MS - COMMIT_RESERVE_MS` and `GET_COALESCE_MS`.

**Flip.** A hosted B0-35 where `late` sheds most head-push merges while the budget is far from spent. That would make `TURN_WAIT_MS` too short for the shard's merge wall time, or show that merges should run more than one at a time.

### `pool-warm`: the step up in k is reserved and warmed ahead, commits only to caught-up slots, and a lapsed slot is torn down, 2026-11-14

LM-1's B0-34 on `graft-m2b` m29 held k 1 on all five networks for 900 s: no pool shard of the 13 that exist ran a container, both warm sites' ledgers held no lease, and nothing warmed a pool shard before a grant (`tests/am1/results/lm1-memory.md`, round 18c). DESIGN, Shape, `HOT_POOL`, and Costs say idle pool shards keep hot networks warm so a step up in k needs no rehydrate, so this is building to the design, not a new cost.

**Decision.** From the first `NetworkLoad` report above `FANOUT_UP`, the network's leader asks both warm sites' ledgers for one pool slot past the k - 1 it serves (`graftyard_git::pool_reserve`) and, after every ask, sends each slot it holds a pool warm (`POST /catchup?warm=pool`), which starts a stopped pool shard and catches it up but is not a request (`Caller::PoolWarm`, `touch: false`). A step commits only when every slot it would serve from on both warm sites answered its last pool warm 200 (`graftyard_git::warm_step`); a step still cold at `FANOUT_UP_FOR` restores the hysteresis to before that report (`LoadReports::step_waited`), so the high line keeps its start and the step commits at the first report after the slot is caught up. Below the line a held reservation is given back. A slot a lease gives up, by lapsing past `LOAD_STALE` or by an ask for fewer, is handed once to the ledger object's heartbeat (`PoolLedger::beat`), which calls `/teardown` on it unless another lease took it. First shards keep their rule: a warm catch-up reaches only a running container.

**Why.** Granted slots alone would still pay one cold rehydrate at each step, and a step committed to a cold slot sends pushes to a shard that answers `warming` for the whole rehydrate, 32 minutes for B0-34's monorepo before stored-pack indexes (round 18b). A pool shard nobody starts never becomes warm, so its warm must start it; tearing it down when its lease goes keeps an idle group from billing it, which is why first-shard warms start nothing.

**Cost check.** DESIGN, Costs: "The tables price that as the pool of 7 per site running on both warm sites for a 9-hour working day, warm only for networks hot in the last hour ... The line adds $433 a month to the default at 2,000 developers and $217 to `Solo`." This runs at most the slots both ledgers grant, at most 7 per warm site and `owner_share` (6) per owner, only while the network's load is above the up line or k is above 1, plus `LOAD_STALE` (3 min) until the lease lapses and one `LEDGER_BEAT` (45 s) until the teardown. A reservation adds one slot per hot network for at most `FANOUT_UP_FOR` plus the wait for it to catch up. That stays inside the priced envelope, so `scripts/cost.py` is unchanged.

**Beat.** Warming only granted slots: the first step still waits on a rehydrate. Committing the step at `FANOUT_UP_FOR` whatever the slot's state: a cold slot answers `warming` to every push forwarded to it. Letting a pool warm `touch` its shard: `WARM_FOR` would hold it a day after its lease lapsed. Starting first shards on warm too: it would bill idle groups (`tests/am1/results/idle-shards.md`); B0-32's never-started first shard is G8.

**Evidence.** `the_step_up_is_reserved_ahead_and_never_commits_to_a_cold_slot` and `a_lapsed_lease_tears_its_slots_down_once` in `crates/git/tests/agent_load.rs`; `only_a_clients_call_starts_a_shard_or_keeps_it_warm` in `crates/git/tests/shard.rs`; plants 86 to 90 in `tests/lm1mem/plant.py`.

**Flip.** A hosted B0-34 whose pool warms keep k from stepping because rehydrate outlasts the load's high line, or pool shards that bill outside the hot hours `cost.py` prices.

### `stored-fetch`: a promotion stores its `.idx`, and a rehydrate loads it and skips the pushes it covers, 2026-11-14

LM-1's B0-34 warm on m29 (main 04f8b3a6) rehydrated repo:131296, the 5 GB linux seed, cold in 1,884 to 2,595 s a shard against DESIGN's 120 s line (fail over 600 s), and every `/catchup` before it timed out at its 120 s client limit [measured, `tests/fanout/raw/rehydrate-131296-m29.json`].

#### A promotion stores its pack's `.idx`, and a rehydrate loads it in place of `index-pack`

**Decision.** The archive's promotion reads the `.idx` that its own `/validate` wrote for the promoted pack (`GET /idx` on the same shard) and puts it under `R2Key::Idx` in every bucket the pack's placement held, create-only, before `Promoted` commits. Its custom metadata names the pack's chain and its range, `from` and `to`. The third-bucket filler copies the `.idx` with the pack. A rehydrate's marked phase streams each promoted pack to the container (`/spool`, no lock) with its `.idx` (`/spool-idx`). The container checks the `.idx` against the pack (version 2, its own SHA-1, the pack's trailer, the object count) and moves both into the repository under the network lock, with no `index-pack`; a missing or rejected `.idx` falls back to `index-pack --strict --fix-thin`. Pushed packs stay thin and as the client sent them.

**Why.** The m29 rehydrate's 257 stored packs were pushes, 2.62 GB, never promoted: `View::unpromoted` counted row bytes, and a stored pack's row is a pointer, so promote-ahead never fired (fixed in this branch's first commit). Each shard's time was 90 to 96% `index-pack --fix-thin`, 1,694 to 2,496 s at 1.0 to 1.55 MB/s, and 53 to 94 s the spool from R2. A fetch that runs in parallel alone saves at most the spool. Locally, on 4 CPUs, a 5.5 MB pack loaded through its `.idx` held the lock 1 ms against 123 ms for `index-pack` (`TestSpooledIdxHoldsLockShorter`).

**Beat.** Storing pushes fixed (G5's first question): the edge tees the pushed bytes to R2 while the shard validates, so fixed packs would mean a second upload of the shard's output per push. A parallel fetch alone: it removes only the spool's share. Running `index-pack` across 4 vCPU: about 470 s at m29's rates, still over the line.

**Evidence.** `a_rehydrate_of_promoted_packs_loads_their_stored_idx_and_skips_the_pushes_they_cover` and `a_corrupt_stored_idx_falls_back_to_index_pack` (`crates/git/tests/catchup.rs`), and `containers/git/server/stored_test.go`. Both catch-up tests fail with main's behaviour.

**Flip.** Shard-streamed promotion, where the shard puts the pack and its `.idx` itself, if hosted promotion cannot keep up with B0-31's agent push rate, or if a network's unpromoted pack bytes stay above 512 MiB for more than an hour. A hosted B0-34 cold warm whose `idx_rejected` is above zero on packs this branch promoted means the check or the copy is wrong.

#### Coverage comes from the committed `Promoted` record; the `.idx` metadata only caches it

**Decision.** A rehydrate does not load a pushed pack whose entry lies in a range that both a committed `Promoted` record (the chain's last one, or one whose entry the marked phase reads) and its pack's `.idx` metadata cover, and only when the rehydrate loads that promoted pack. After the marked phase, the tail, and the refs, the container must answer `/connected?objects=tips`: every ref's history, and the whole tree at each ref's tip, present. If it does not, the skipped packs load in index order, the check runs again, and the miss is reported as `fault`. A disk that still fails drops its cursor and place and never serves, so the next call rehydrates from nothing. This is the read side of `Compacted.consumed`; nothing writes `Compacted` yet.

**Why.** An object's custom metadata is not committed state: a wrong or stale range there would skip packs the shard needs, and objects would be missing with nothing to flag them. Without the skip, a promoted network still loads every pushed pack the promotion holds, which costs the same `index-pack` time as before.

**Beat.** Trusting the metadata alone. Writing `Compacted` now: it touches the fold and the sweeper's deletes, and the skip needs only the read.

**Evidence.** `idx_metadata_wider_than_its_promoted_record_skips_nothing_outside_the_record` and `a_planted_missing_object_in_a_covered_pack_falls_back_to_the_skipped_packs_and_passes` (`crates/git/tests/catchup.rs`), both failing with main's behaviour.

**Flip.** A hosted rehydrate that reports a `fault`: the promotion's pack missed an object of its range, which is a promotion bug to fix there.

#### Stored packs are fetched four at a time and ingested in log order

**Decision.** A catch-up or rehydrate fetches up to `FETCH_AT_ONCE` (4) stored packs at once onto the container's disk, at most `AHEAD_PACKS` (8) ahead of the ingest and, past the first four, `AHEAD_BYTES` (64 MiB) ("`fetch-ahead`" above), and ingests them strictly in log order. A catch-up does not fetch a `Promoted` entry's own pack: it applies the range's entries first, so the disk already holds every object of it.

**Why.** On B0-31 m3fr4 the apac shard's catch-up spent about 500 ms a pack in per-pack round trips outside spool and `index-pack`. A Workers invocation has at most six connections waiting for response headers [doc, Workers limits], and a `/spool` call waits for its headers until the container has the whole body, so four leave one for the in-order ingest and one for the next log page. The bounds are counted on the container's disk, where the fetched bodies wait.

**Evidence.** `a_catch_up_fetches_stored_packs_at_once_and_ingests_them_in_log_order`: 60 packs with the first one slow, 11.5 s against 34.5 s one at a time, ingested in order. `a_cold_rehydrate_of_5_gb_projects_under_120_s_and_the_serial_fetch_does_not`, at m29's site c rates (up 34.2 s, 27.9 MB/s one stream, 375 ms a pack, the `.idx` 3.5% of its pack): the seed's 257 packs in 83 s against 227 s one at a time, and 5 GB as 100 promotions of 48 MiB in 90 s against 258 s. All of these fail with main's behaviour [inference: the multi-stream R2 rate into one container is unmeasured and assumed equal to the one-stream rate].

**Flip.** B0-34's hosted cold warm on this branch over 120 s with a summed spool rate near the one-stream rate: the container's link or R2 caps the sum, and the 5 GB line needs a design change, not more fetches.

#### Promotion's measured rate is under B0-31's push rate

**Decision.** `PROMOTE_MAX` stays 48 MiB as the most pack bytes one promotion batches; since `promote-stream` it bounds no site memory and refuses no entry ("`promote-stream`" below). The unpromoted tail of a network pushed faster than promotion runs is open question G9.

**Why.** On git/git's first-parent history, 30 thin pushes of 100 commits (29.2 MB) promoted as one: `pack-objects` 4.65 s (57.9 MB out), and the validate's `index-pack` 6.50 s with its default threads (11.97 s with one), 0.0 s connectivity. So 2.62 MB/s of pushed bytes, CPU-bound, against about 4 MB/s from B0-31's agents. `pack-objects` writing files gives a pack that differs from its `--stdout` output, so the stored `.idx` is the validate's, from the bytes that are placed. A promoted pack is 1.6 to 4.3 times its thin pushes' bytes, more for shorter ranges (2.3x at 30 pushes per promotion, 4.3x at one). The push-rate sweep at 2.62 MB/s over 600 s ends 3, 0, 754, and 1,957 MB above the last promotion at 1, 2, 4, and 6 MB/s, each step at most 19.2 s [measured locally, `tests/am1/results/lm1-rows-m3f.md`, "Promotion rate"; `promotion_keeps_up_below_its_rate_and_stays_bounded_above_it`]. A growing tail costs slower rehydrates, not correctness. Shard-streamed promotion lifts the site's memory bound, not this CPU one.

**Flip.** The same as the first decision's: hosted promotion MB/s under B0-31's push rate, measured in B0-34's drain of repo:131296.

### `fetch-ahead`: four stored packs of any size fetch at once, and a promotion packs only its range's own objects by name, 2026-11-15

LM-1's B0-34 cold warm on `graft-m2b` m35 (image 36c17feb) took 154, 183, and 214 s against DESIGN's 120 s line. Spool summed 80 to 127 s for 8.53 GB of promoted packs, on a network of about 2.6 GB of pushes (`tests/am1/results/lm1-memory.md`, round 23).

**Decision.** Two changes:
- A catch-up or rehydrate starts up to `FETCH_AT_ONCE` (4) stored pack fetches ahead of the ingest whatever their size. `AHEAD_BYTES` (64 MiB) bounds only the packs past those four, within `AHEAD_PACKS` (8). The stored `.idx` streams from the bucket into the container's `/spool-idx` as the pack does (`ShardHost::put_stream`), so the shard object holds no byte of either.
- `record` lists only the objects a pushed pack brought: offsets before the sent length minus its 20-byte trailer, so not the bases `--fix-thin` appended. `/validate` saves the sent length for `adopt`. `/promote` gives `pack-objects` each object's tree-entry name. The promoted pack stays complete and holds exactly the same objects as before.

**Why.** `Ahead::pump` started another fetch only while the bytes ahead fit 64 MiB, or when nothing was ahead. Every promoted pack of repo:131296 is 106 to 207 MB, so the marked phase fetched one at a time: each shard's summed spool (79.5, 92.3, 126.6 s) is under its marked phase (100.6, 122.3, 155.4 s). The fetched bodies wait on the container's 20 GB disk, not in memory, so the byte cap protected nothing the site holds. Promoted packs are 3.15 times the pushed bytes they cover locally, against about 3.2 hosted. The causes:
- each range is packed complete: 3.24 times at 6 steps, 2.47 at 24, 1.85 at 96;
- the id list has no names: plus 17% on step 96;
- it repeats the bases `--fix-thin` appended: plus 15%.

The batched inline path (`ingestRun`) already dropped those bases by offset. Fewer and named objects also mean less `pack-objects` work per promotion, so the change helps G9's 0.49 MB/s rather than costing it [measured locally, round 23b].

**Beat.**
- **Thin promotions against earlier promotions.** A thin pack's stored `.idx` cannot be loaded (G5), so every rehydrate would re-index at 1.0 to 1.55 MB/s: about an hour for 5 GB.
- **A lower promote-ahead threshold.** It cuts the 39 to 45 s re-index of the tail, but makes more and smaller promotions, which are the slowest per pushed byte and the largest per pushed byte. G9's rate is the binding problem.
- **Longer ranges by raising `PROMOTE_MAX`.** 24 steps pack at 2.47 times against 3.24 at 6, but each promotion takes longer, at G9's same CPU rate.
- **A roll-up of promoted packs (`Compacted { consumed, produced }`)** is not built now. It reached 1.04 times, but took 385 s on 4 threads for 2.2 GB and 8.6 GB resident. It is G9's next lever.

**Evidence.**
- `large_promoted_packs_fetch_four_at_once_and_still_ingest_in_log_order` (`crates/git/tests/catchup.rs`): twelve 207 MB promoted packs reach 4 fetches at once and ingest in log order. On main it fails with 1 at once.
- `tests/fanout/fetchmem` (local workerd, R2 bodies streamed into a slowed `/spool` stand-in): peak RSS 115 MB with one 207 MB fetch and 118 MB with four, from 113 MB idle. The control that reads each pack whole reaches 532 and 1,849 MB.
- `TestRecordKeepsOnlyThePacksOwnObjects` (`containers/git/server/promote_test.go`): a thin pack's recorded objects are exactly its range's, on direct ingest, on adopt after `/validate`, and in `/ingest-batch`'s one-pack branch. On main it records 42 against 36.
- `TestPromotedPackIsCompleteAndHoldsExactlyTheRange`: `index-pack --strict` with no `--fix-thin` accepts the promoted pack in an empty repository, and its index is exactly the six ranges' objects.
- `TestPromotedPackIsSmallerThanMainsWay`: 154,117 B against main's 265,424 B (58%).
- Plants 107 to 113 in `tests/lm1mem/plant.py`, all caught.

The four-stream R2 rate into one container is unmeasured [inference: B0-34's next cold warm measures it]. Repo:131296's existing promotions stay at about 3.2 times, because only new promotions shrink.

**Flip.**
- For the fetch: B0-34's cold warm on this change with a summed spool rate near round 23's one-stream 67 to 107 MB/s. That would mean the container's link or R2 caps the sum, and the 5 GB line needs fewer bytes (G9's roll-up), not more streams.
- For the promotion: a hosted rehydrate whose `.idx` load or `/connected` reports a missing object in a promoted pack.

### `connected-after`: a rehydrate's connectivity backstop runs before its repack, 2026-11-15

LM-1's B0-34 drain on `graft-m2b` m33 (main fb4a1b66) held every shard of repo:131296 at `warming` for the whole 45 minutes. From about 04:00Z, where m31's rehydrates finished, all three containers ran at 3.3 to 3.4 vCPU with no `shard rehydrate` report to the stop (`tests/am1/results/lm1-memory.md`, round 21).

**Decision.** A rehydrate that skipped pushed packs a promotion covers sends its last ref page with `repack=0`, so the container's `/refs` starts no repack. Its backstop `/connected?repack=1` then walks every ref's history and each tip's tree, and starts the network's repack once the walk ends, passed or failed. A rehydrate that skipped nothing, and every catch-up, keep today's path: the last ref page starts the repack. `/connected` and `/fsck` keep taking the repack lock.

**Why.** `/connected` waits for any running repack, because a repack deletes the packs it rolled up as it ends, and hosted `/fsck` read false right after one (`tests/am1/results/hot-shards-3.md`). The rehydrate's own last ref page had just started a geometric repack of the whole network, 2.6 GB of linux there. So the check waited out the whole repack, and the shard's place, dropped only after the check, kept it answering `warming`. In m31 nothing was skipped (no stored `.idx` yet), so the check never ran and the shard served while the repack ran behind it.

**Beat.** Dropping the repack lock from `/connected`: a check that runs while a repack deletes packs is not shown safe. Skipping the repack after a rehydrate: every ingested pack stays its own pack, and git commands slow as packs grow (`containers/git/server/main.go`, `repackAt`). Running the check before the last ref page: the check walks the refs, so they must be written first.

**Evidence.** `TestConnectedRunsBeforeTheRehydratesRepack` (`containers/git/server/convoy_test.go`): with `runRepack` held, `/connected` answers within 5 s and only then starts the repack, after a passing and after a failing walk, and a last ref page without `repack=0` still starts it at once. On main it fails with "the last ref page started a repack though the rehydrate's check follows it". In `crates/git/tests/catchup.rs`, the skip tests assert the order `connected` then `repack` (and on a fault, `connected, repack, connected, repack`), and a rehydrate that skipped nothing asserts `repack` alone. Plants 101 to 106 in `tests/lm1mem/plant.py`.

**Flip.** A hosted rehydrate whose check reads false on a pack a concurrent repack moved: the order is not enough, and the repack must also wait for something the check holds.

### `promote-stream`: a promotion streams the shard's pack to the buckets, and an entry over `PROMOTE_MAX` promotes alone, 2026-11-15

LM-1's B0-34 drain on `graft-m2b` m31 (main dc269249) promoted entries 7..16 and 17..17 of repo:131296, then stalled for good at entry 18, a 53,125,950 B stored push over `PROMOTE_MAX` (48 MiB): `within_promote_max` refused it on every alarm, and nothing after it could promote or archive (`tests/am1/results/lm1-memory.md`, round 20).

**Decision.** The site's promotion reads the shard's `/promote` answer as a stream (the shard passes the container's body through unread) and tees it with `PackStore::place_promoted` to the placement's buckets in `PART_SIZE` multipart parts and, through a `Pipe`, to the `/validate?derived=1&via=0` on the same shard, holding the commands and at most two parts whatever the pack's size, as the edge's streamed push does (DESIGN, journey 1, step 2). Once both answered, it reads the `.idx` that validate wrote and puts it beside the pack in each bucket that holds it, with the chain and the range as `from` and `to`, as `stored-fetch` writes it. `PROMOTE_MAX` now only bounds how many pack bytes one promotion batches (about 19 s of `pack-objects` and validate at 2.62 MB/s): a range's first entry over it promotes alone, never refused. A shard whose pack breaks off mid-stream is passed over for the next Full site's, like one that failed before its first byte, because each attempt reads a fresh pack from its own shard under a fresh `PackId`.

**Why.** `PROMOTE_MAX` existed only because `promote` held the shard's whole complete pack in the site's 128 MB isolate before `place`. Streaming removes that bound at the root, so a push the edge accepted (up to its cap below 100 MB on Pro) can always be promoted and archived past.

**Beat.** Capping pushes at `PROMOTE_MAX`: it refuses pushes DESIGN allows, and a repository of one big push could never be served. Treating an over-cap pushed pack as self-contained and recording it as its own promotion: pushed packs are thin, so it is unsound. Raising the cap for a lone entry: it only moves the cliff, a 90 MB push would put a 90 MB-plus pack whole into a 128 MB isolate.

**Evidence.** `promotions_fit_the_cap_and_an_entry_over_it_promotes_alone` (`crates/git/tests/log_archive.rs`): at a 5,000 B cap every 9,000 B stored push promotes alone, 16 of 16, and the archive passes the first; on main it fails with "entry 6 names 9000 pack bytes, over the 5000 a promotion holds". `a_promotion_streams_through_two_parts_and_stores_its_idx` (`crates/git/src/store.rs`): a 112 MiB promotion, over twice `PROMOTE_MAX`, at 16 MiB parts holds at most two `MemPart`s at once, validate reads it whole, two buckets hold the same bytes in order, and the `.idx` lands with `from` and `to`; a refused validate asks for no `.idx`, and a shard with none stores the pack alone. Plants 94 to 100 in `tests/lm1mem/plant.py` and plant 4 in `tests/am1/rest/plant.py`, all caught [the hosted site memory of a streamed promotion is not yet measured; the edge's tee held under 37 MB for 60 MB to 2 GiB pushes, `tests/am1/results/stream-push.md`].

**Flip.** A hosted promotion whose site isolate passes about two parts plus its baseline, or a drain that still stops on an entry: then the tee does not stream on the site's path and the bound must be found there.

### `narrow-builds` (1): a list's selection pages from `issue_order`'s keys and reads only the page's items, 2026-11-14

Branch `narrow-builds` (`tests/am1/results/narrow-reads-3.md`, "Instrument run (b)"; F65).

**Decision.** `Repo::numbers` lists the numbers of a kind and state from `issue_order`'s keys alone. A GraphQL connection whose only filter is its states, when the states are ones the order's open flag tells apart, counts and pages from those numbers and loads only the page's items. A REST `issues` list with no `labels` sorts the `issue` rows alone, which hold `created`, `updated`, and `comments`, read in two scans of the `issue` table, and loads only the page's items. Every other filter (labels, assignee, author, mention, head, base, and CLOSED or MERGED apart) reads every item as before. Replies are unchanged byte for byte.

**Why.** On B0-29's shape with the per-invocation counter, invocations with a narrow build carried 77% of the learners' heavy CPU, at about 2,605 storage reads a build and 0.043 ms each, REST and GraphQL alike [measured, `tests/am1/results/narrow-reads-3.md`, "Instrument run (b)"]. `Repo::list` read `issue`, `pull`, and `pull_head` of every item of the kind and state, then sorted and paged: 1,556 to 1,655 storage reads for a 30-item page at 500 PRs.

**What it beat.** For the REST sorts, a fold-maintained sort index (one table per sort key, written by every update and comment) would page from about 30 index rows, against about 500 `issue` rows here. It adds three metadata tables, a format and refold, and a write per comment and update, for a list that is half of journey 9's API reads; reading the `issue` rows alone needs no schema change, takes 154 statements against 1,655, and keeps this change independent of (2). The index is the flip.

**Evidence.** `crates/api/tests/narrow.rs`: every REST sort, direction, state, and label filter over 8 page boundaries, the last partial page and empty pages included (768 cases), and GraphQL connections over 6 state sets, MERGED and CLOSED apart, at 8 page sizes and cursors, each byte-identical to the whole-tier reply against an independent oracle; `a_page_of_a_wide_repository_reads_its_page_not_every_item` (500 open PRs: at most 200 statements and 900 rows for a 30-item page; 54 to 154 statements and 592 to 799 rows here, 1,556 to 1,655 and 2,093 to 2,348 on main, where it fails). `tests/nr3/plant.py` 33 to 41 catch 9 of 9: numbers oldest first, one open flag, MERGED and CLOSED paged from the keys, `totalCount` of the page, an inclusive cursor, every item loaded, every state kept, the next link off by one, and labels paged before filtering.

**Flip.** A REST list sorted by `updated` or `comments` on a repository whose `issue` rows alone cost more than 20% of a learner's CPU at journey 9's rate: then the sort index above.

### `narrow-builds` (2): the accounts a tier names are a derived index `apply` keeps in memory, never stored, 2026-11-14

Branch `narrow-builds` (`tests/am1/results/narrow-reads-3.md`, "Instrument run (b)"; F65).

**Decision.** Each member keeps, per tier, a count of the rows naming each account (`hosted::NamedIndex`): issue authors, assignees, comment authors, reviewers, status creators, release authors, asset uploaders, lock owners, and mergers, the rows `hosted::accounts` scans. `apply` moves it through `Store::wrote` (each row's old and new value, before the write) and `Store::applied` (the entry finished), and it counts as current only when it followed every entry to the tier's `applied`. Otherwise it is built from the rows at the next narrow read: after a start, an install, a metadata load from another index, or a lost write. A narrow answer's `named` is the union of the two tiers' indexes. It is never stored. The scan stays for the edge's whole-tier read and the hook sequencer.

**How existing members get it.** Derived and never persisted, the third of the operator's options. A member built on this release builds the index from the rows it already holds on its first narrow read, whatever release wrote them, and a member restored from a snapshot taken before this change does the same. The other two options were refused by the fold rules. A stored table is a fold schema change: it enters `fold_digest` and snapshots, so it ships as a lazy refold under the two-release rule ("Schema change is a refold, never a migration"), and members on the previous release would digest differently until every site rolled. A table rebuilt from the log at boot or restore is a stored table with a refold. Nothing a reply depends on is outside the rows, so SHARED's new rule on derived indexes applies.

**Why.** A narrow build's `named` re-ran `hosted::accounts` whenever the tiers' state changed, which at B0-29's write rate was nearly every build: about 1,500 of a build's 2,605 storage reads at 500 PRs [measured, `tests/am1/results/narrow-reads-3.md`, "Instrument run (b)"].

**What it beat.** The fold table the operator named, which needs the format and refold above for an answer the rows already hold. A per-state cache of the scan, which is what `NAMED` was, misses on nearly every entry.

**Evidence.** `crates/api/tests/narrow.rs`: the index kept through every entry of `big()` equals the scan of the tiers at each entry; an account leaves with its last naming row and stays while another names it; an index that skipped an entry is not current; a member restored from snapshot frames that hold no index answers the same `named` and the same narrow answers as one that applied the log; `pulls/250` at 500 PRs reads at most 40 storage statements with the kept index, where the scan read over 1,500 rows. `tests/nr3/plant.py` 42 to 47 catch 6 of 6 (see the results file). The index costs about 40 bytes per named account per member.

**Flip.** A narrow read that needs the index before its tier is loaded, or a first-read build measured over 50 ms of a member's CPU: then store it as a fold table under the two-release rule.

### `admitted-bytes`: an open ticket keeps one copy of its command, and a deposed leadership answers its tickets as issued, 2026-11-15

**Decision.** A leader host keeps with each admitted ticket only its request id, its admission time, and what its `Committed` marker puts read after an `Acked` (`RepoCmd::marked`: a stored pack ref, or the blob a member confirmed, never an inline pack or a reply render). The command a `NeedIdBlock` re-send needs comes from the core: `Out::refused` now carries the refused command, moved out of the batch, and the host attaches it to that ticket until its entry commits. A leadership that a higher term ends answers each ticket under the term it was issued in (`Lead::term`), and seqs never repeat within a `boot`, so a term resumed in the same boot reissues none. `memSplit` reports `inFlight.admittedBytes`: the core's queued, frozen, and decided bytes (`Member::admitted_bytes`) plus what the host keeps.

**Why.** B0-33 on m30b: the leaders' heaps tracked their open tickets at about 60 to 80 KB a ticket (LM-1 round 19). Two causes. The host cloned every command into `Core::admitted` beside the core's own copy, which doubled every reply render and inline pack while the ticket was open. And `step_down` raised the term before `end_leadership` answered the tickets, so they resolved `NotLeader` under the new term; the host matched none of them, and the deposed leaders kept 581 to 1,172 tickets, each with its command and waiters, for minutes (site 1 at 1,149 and 124.7 MiB as a follower at 23:43Z). The resumed-term reuse was found by the same reading: a leader whose `Noop` reached no quorum and resumed `led_term` in the same boot issued seq 1 again, so a late `outcome` for the old ticket could answer the newer command's outcome.

**Beat.** Keeping a slim command for every ticket: the re-send needs the whole issue command, render included, so a slim copy would change the reply. A byte cap on admitted tickets with a fast `Full`: it bounds the copy, not the leak, and changes which commands are refused. Matching `NotLeader` answers by boot and seq on the host, as `crates/raft/tests/deps.rs` used to: every host would carry the workaround, and the Ticket a member answers would still not be the one it issued.

**Evidence.** `a_thousand_open_agent_commands_hold_one_copy_and_none_after_a_higher_term` (`crates/git/tests/admitted_memory.rs`): 500 issue commands with m3org-shaped renders and 500 agent pushes with 64 KB packs, all open, hold 9.9 MB of heap against 17.8 MB on main; after a higher term deposes the leader, 15.7 KB, and on main 1,000 tickets never resolve for the host. `a_leadership_deposed_by_a_higher_term_answers_its_tickets_as_issued` and `a_term_resumed_in_the_same_boot_never_reissues_a_ticket` (`crates/raft/tests/scenarios.rs`) fail on main. `tests/m0carried/plant14.py`. `tests/m0/results/admitted-bytes.md`.

**Flip.** A hosted B0-33 whose leader heap still grows with open tickets past the core's `admittedBytes` times 2, or a `NeedIdBlock` re-send that finds no command on its ticket.

### `lease-clock`: a member's clock is floored by peer and edge stamps, and lease reads take a round while it lags, 2026-11-15

**Decision.** Every time a member reads, for `LogEntry.at`, leases, expiry, and `/status`, is its member clock: the later of the isolate's `Date.now()` and the largest stamp it has seen this boot, never decreasing (`workers/site/src/clock.rs`). Every `/raft` call and reply carries the sender's member clock in `x-graft-clock`, in the product build, and so does every edge call to a member (`graftyard_lookup::post_within`). Each stamp also records a lag, the stamp minus the isolate's clock when the stamp arrived. While the largest of the last 8 lags is `DRIFT` / 2 (250 ms) or more, the host sets `raft::Member::set_clock_suspect`, and the leader grants a read that arrives then only by a round sent after its arrival, not by its lease. That one boolean, read in `start_read`, is the only raft change. `/stats` reports the lags by bucket, the largest, the window, and `leaseFallbacks`. The case left open is F75.

**Why.** L0-12 m3fr5b failed its clock line at 1,171 ms, with the leader under load reading early and the followers within about 330 ms. A probe settled the guess that a Worker's clock advances only on I/O: a Durable Object running 400 ms CPU turns read its clock 715 to 1,681 ms behind a peer's stamp, still 654 to 1,588 ms behind after an awaited storage `sync()`, `get()`, subrequest, or `setTimeout(0)`, and up to 1,349 ms before a stamp it had already received. `max(own clock, last peer stamp)` cut the error to 7 to 139 ms. The lease code reads the same clock, so the lag is a real lease-clock offset, and the clock can read before the event it is handling arrived. A read granted on such a clock can be served past its lease by up to the lag, and the margin between a lease's end and a follower's campaign is 1.5 s [inference; no stale read measured].

**Beat.** (a) The floor alone: it fixes `at`, expiry, and the offset line, but when every stamp the leader has is itself stale, the floored clock still lags and the lease check trusts it. (c) A larger `DRIFT` or a shorter `LEASE`: the probe's lag reached 1.7 s under CPU load the row's leaders saw, so `DRIFT` would need about 2 s and `LEASE` would fall to 8 s or less, which costs a lease round more often on every group, and still bounds nothing past the probe's load. Reading the time after the last I/O before `decide`, the first candidate: the probe disproved it, because an await on storage does not move the clock forward. Reporting a corrected time only on `/status`: the row would then not measure the clock that leases use.

**Evidence.** `tests/am1/results/lease-clock.md`, "The clock probe" (`tests/am1/clockprobe/`). `a_suspect_clock_grants_reads_only_by_a_round` (`crates/raft/tests/scenarios.rs`), which does not compile on main because `set_clock_suspect` does not exist there. `workers/site/src/clock.rs` tests: a stale own clock steps at the newer stamp, an older stamp never moves the clock back, and the guard trips at `DRIFT` / 2 and holds for 8 stamps. `tests/m0carried/plant15.py`.

**Flip.** A hosted lag distribution where no member's lag passes `DRIFT` / 2 under B0-31, B0-32 and L0-12's load would make the guard dead weight; a guard that sends enough reads to rounds to put API read p99 over B0-29's 2 s line would make its threshold a cost to weigh, reported, not tuned. A stale read in a saturating probe reopens the lease itself (F75).

### `lease-clock` (2): a write retries a spread read its members shed, within `LOOKUP_FOR`, 2026-11-15

**Decision.** `graftyard_lookup::read_spread_for` asks the same members again while every one asked sheds the read. It waits the largest `retryAfterMs` an `overloaded` answer named, else 250 ms, while that wait ends inside its deadline. The edge gives a write `LOOKUP_FOR` (20 s): a receive-pack, a REST call other than `GET`, or a GraphQL mutation (`Ctx::write`, `spread_read`). Any other request keeps its 503 at once.

**Why.** On m3fr5b a Human push and a REST write failed 503 "every member asked shed the read" for a `{"repo": ...}` read of a few hundred bytes on `authz:7100001`. No cap fired. The `Authz` leader stalled 7.4 s on 1 ms of CPU, so one follower missed the edge's 2 s call and the other answered `overloaded` after its 1 s `FOLLOWER_WAIT`, and `read_spread` returned the shed without the retry `call_opt` already does for leader reads [measured, Workers Logs, `tests/am1/results/lease-clock.md`, "The shed Authz read"].

**Beat.** Sending a write's shed read on to the leader: that is the overflow that stepped leaders down on every 500-agent row ("Follower read"). Exempting small `Authz` reads from `READS_MAX` or `ANSWER_BYTES_MAX`: no cap was involved. Retrying every request's shed read: a read-class request would hold its isolate for up to 20 s where GitHub's clients already retry on `Retry-After`.

**Evidence.** `a_shed_read_is_retried_inside_its_deadline_only` and `a_shed_spread_waits_what_its_members_named` (`crates/lookup/src/lib.rs`). `tests/m0carried/plant15.py`.

**Flip.** A hosted row where writes wait out `LOOKUP_FOR` on shed reads often enough to move their p99, which would put the members' capacity, not the retry, in question.

### `learner-reset`: a new member at an address, learner or voter, starts with nothing the leader held for that address, 2026-11-15

**Decision.** When a config change names a new member at an address (a voter's site, or a learner's slot address `learner_addr(slot)`), absent before or under a different `do_name`, the leader resets its per-peer state for that address (`next` to its last index, `matched` to 0, and `matched_at`, `sent`, `pipe`, `promised_at`, `acked_at`, `retries` and `refusals` cleared), and ignores answers to rounds it sent before the change to that address (`Lead::member_from`). The one-voter-change rule still compares voters by site.

**Why.** B0-33 (LM-1 round 23, `tests/am1/results/lm1-memory.md`): the reset ran for new voter sites only. `/learners` places each generation on the same slots, so a leader that had placed generation 2 kept its `matched` of about 748 for each new, empty generation-3 learner, probed it at prev 748 on every call, and was refused 1,776 times (144.9 MB) for the whole row; the learners applied nothing, `append_for_read` sent them no entries for reads at or below the stale index, and the read gate answered 503. A late answer from the old member at an address, to a call sent before the change, would have set the same stale `matched` after the reset.

**Beat.** Keying the leader's per-peer state by `do_name` instead of address: every message, `calls_out`, the host's callers, and the trace key by address, so the whole path would change for one reset. Resetting every learner on any config change: a change that keeps a learner's name would restart its catch-up.

**Evidence.** `a_second_learner_generation_on_the_same_slots_catches_up` (`crates/raft/tests/scenarios.rs`) fails on main, and with the reset but no round guard. `tests/m0carried/plant16.py`. `tests/m0/results/learner-reset.md`.

**Flip.** A hosted row in which a learner placed on a slot an earlier generation used takes longer to catch up than a learner placed on a fresh slot.

### `repack-keep`: promotions a rehydrate loads are kept, and the repack runs low and bounded, 2026-11-15

**Decision.** A pack the container ingests through its stored `.idx` (only a promotion stores one) gets a `.keep` beside it. `startRepack` counts only unkept packs against `REPACK_AT` (50). `runRepack` runs `git repack -d -q --geometric=2 --write-midx` with `pack.threads=2` and `pack.windowMemory=256m` under `nice -n 19`, so it rolls only the pushed tail and leaves a multi-pack-index over every pack, kept or not; its commit-graph runs at nice 19 too. The container answers `GET /procs` (every running git process with command, nice, CPU seconds and RSS), and the shard object's `/status?repo=` passes it on as `procs`.

**Why.** On B0-34's cold warm (m36, LM-1 round 24c), each shard of repo:131296 loaded 61 promotions, 8.6 GB, through their `.idx`, and the rehydrate's `/connected` then started the geometric repack over all of them. With no request in flight, each container went from 1.3 to 3.5 vCPU in five minutes and sat at 11.95 GiB of standard-4's 12 GiB. Site a's second instance took 8 minutes to finish; sites b and c had not finished after 23. Every catch-up, and so every upload-pack and validate waiting on one, ran behind it, answered 503 `warming`, and the row failed. Locally on 33 promotions of the same plan (6.48 GB), the unkept, unbounded repack took 325 s, 588 CPU-s and 9.8 GB of memory and rolled them into one 2.18 GB pack. Kept and bounded on the same packs plus a 12-pack tail, it took 98 s, 104 CPU-s and 5.49 GB, left all 33 promotions on disk byte for byte, and wrote a midx that verifies. Bounded but not kept, it took 757 s, 620 CPU-s and still 9.20 GB: the bounds spread the CPU, and only the keep shrinks the memory [measured, `tests/fanout/georepack.sh` and `tests/fanout/georepack-keep.sh`; branch `lm1-memory`, `tests/am1/results/lm1-memory.md`, rounds 24c and 25]. A promotion is complete and delta-searched when it is made, so rolling it again buys only the ratio a roll-up would (G9), at a whole network's cost on every cold shard.

**Beat.** Repack deferred until the shard is idle: a hot shard is never idle, and B0-34's pushes keep coming, so the tail would grow past `REPACK_AT` without bound. No repack after a rehydrate: the pushed tail is thin one-push packs, and merges slowed from 2.0 to 8.7 ms over 751 packs (`tests/m2/results/m2-wp6.md`), which the geometric roll of the tail still prevents. A cgroup CPU cap on the repack: the container runs as one cgroup with no delegation, so `nice` is the lever the image has. Raising the container to more memory: the roll-up grows with the network, and 13 GB at 8.6 GB [inference] would outgrow any size on the next network. `pack.windowMemory` alone as the memory bound: it held the unkept roll at 9.20 GB, not under it.

**Evidence.** `TestRehydratesRepackLeavesPromotedPacks` (fails on main: 53 promotions alone repack into 1 pack) and `TestRepackRunsLowAndProcsListsIt` (fails on main: no `/procs`) in `containers/git/server/repack_test.go`. `tests/m0carried/plant17.py`.

**Flip.** A hosted shard whose midx and commit-graph over the kept promotions leave merges or validates slower than after a full roll, as 1,950 packs did (`tests/m2/results/m2-wp6.md`); or a network with so many promotions that the midx write itself takes minutes. Then G9's roll-up, bounded in memory, takes their place. A cold warm whose repack still holds a shard's vCPU at nice 19 while requests wait would put a CPU cap on the container's derived work in question.

### `read-rounds`: reads at one clock reading share one read-index round, sent to the voters only, 2026-11-15

**Decision.** A leader whose lease does not cover a read (lapsed, `DRIFT` from its end, a transfer, or a suspect clock) sends one read-index round for all reads that arrive at the same member clock reading, and sends it to the voters only. The round is an AppendEntries to each other voter, forced as before; a learner's answer counts toward no quorum, so it gets none. A read at a later clock reading sends its own round, as on main, so no read waits for a round that left before it arrived and a lost round costs no more than before. DESIGN states it in Shape, "The read path".

**Why.** B0-33 on m36 (main cfd46001, LM-1 round 24b) changed term 2 to 19 with leader commit p99 4,622 ms. The F74 trace shows the voters' own calls queued behind thousands of open calls, and where those came from. On each voter's own trace, 83 to 95% of its AppendEntries (to voters, and held for learners) left in 6 to 30 single milliseconds, 250 to 560 to each of the 10 targets in one millisecond, every one a duplicate of the others: 9,010 of 9,129, 26,743 of 26,820 and 31,586 of 31,667 of the appends held for learners repeated a (learner, last entry) already held. `start_read` broadcast a forced round to every target for each read that needed one, and a Workers clock moves only on I/O, so a burst of 250 to 560 reads stepped in one turn at one time. Each held append rode a learner's next call response, and the learner answered each with its own call (`Host::call`, one per `AppendRes`, read from the code): learners sent 270,129 calls to the voters in 300 s, of which 6,144 were read-index batches and 36,580 hedged copies. The voters' calls to each other then waited: with the caller's fetches waiting for headers under 12, 44% answered within CALL_DEADLINE; at 200 to 1,000, 10%; at 1,000 or more, 4%, out-leg p50 5.8 s [measured, `tests/m0/results/read-rounds.md`].

**Beat.** Coalescing learners' read-index calls, one in flight per learner per voter (the operator's preference): read-index batches were 6,144 of the learners' 270,129 calls in the window, 2.3%, already capped at `READ_PIPELINE` and one per 50 ms. One read round out at a time, a later read waiting for its answer: it cut the same flood, but it held reads behind a slow round, and in the simulator's gray half it lengthened leaderless stretches (23 of 2,000 seeds over 20 s against 12 on main, and seed 10,756 at 32.4 s past the 30 s bound), because a per-read round also renewed promises and the lease as a retry [inference]. Raft-call priority or an outbox on the learner host: both treat the queue, not the cause, and the learner's calls follow from the leader's appends.

**Evidence.** `reads_at_one_instant_share_one_round_and_no_learner_gets_it` (`crates/raft/tests/scenarios.rs`) fails on main: 200 reads at one instant sent 200 rounds to each voter. `tests/m0carried/plant18.py`. The long sweep passed (2,000 HARSH, 2,000 GRAY, 4,000 live seeds), and Porcupine on h, h.gray and h.live returned Ok on 153,165, 2,247,416 and 5,961,247 operations (`tests/m0/results/read-rounds.md`). The same seeds' gray half with the rule: 12 seeds over 20 s, the longest 27.5 s, against main's 12 and 22.7 s.

**Flip.** A hosted B0-33 whose voters still send tens of duplicate AppendEntries per target in one millisecond, which would point to another per-read broadcast; or a row whose reads wait measurably longer for their grant than on main.

### `read-round-gap`: at most one read round per READ_ROUND_GAP while the last is unanswered, 2026-11-15

**Decision.** A leader sends a read-index round only for a read that needs one and arrived after the latest round left, and only when the slot is free: no round yet, the latest answered by a voter quorum, or `READ_ROUND_GAP` (20 ms [guess]) since it left. `check_reads` tries again after each answer, so the next round leaves on the answer. Rounds still go to the voters only, and a grant still needs a quorum ack at or after the read arrived. This replaces read-rounds' sharing per clock reading. DESIGN states it in Shape, "The read path".

**Why.** L0-12 m3fr6 on graft-m3f (build a304d3fc, with read-rounds) changed term 2 to 32 in a 10-minute replay with 10 kills, commit p99 17.3 s. The learner flood was gone, but the voters still sent read rounds in bursts: 4,504 of site 0's 6,573 appends to voters, 1,154 of site 1's 1,954 and 3,001 of site 2's 8,013 left in milliseconds with more than 5 rounds to one voter, up to 99. lease-clock floors the member clock at each newer peer stamp, so one turn's reads saw 6 to 17 new readings at p50, and each sent a round; those raises account for 1,794 of site 0's 2,579 burst rounds and 672 of site 1's 705. The two busy members' clocks were also suspect (11,543 and 10,296 lease fallbacks), so every read needed a round [measured, `tests/m0/results/read-round-gap.md`]. A bound per gap holds whatever the clock does.

**Beat.** read-rounds' rule, one round per clock reading: unbounded once the clock moves within a turn. One round in flight with no gap, the next only on its answer: it failed the gray half before (seed 10,756 at 32.4 s past the 30 s bound, `decisions/DECISIONS.md`, "`read-rounds`"), because a lost round then held reads and promise renewals until CALL_DEADLINE.

**Evidence.** `reads_send_at_most_one_round_per_gap_while_the_clock_climbs_and_no_learner_gets_it` (`crates/raft/tests/scenarios.rs`) fails on main: 200 reads on a clock climbing 1 ms each sent 200 rounds to a voter in 1 s against a cap of 50. `tests/m0carried/plant20.py`. Gray half on seeds 10,000 to 11,999: 13 seeds over 20 s and none over 30 s, the longest 27,508 ms (seed 10,698), against main's 12, none and 27,508 ms. Simulator read grant latency against main: gray p50 26 / p99 2,019 ms against 26 / 2,021; harsh p50 1,055 / p99 27,921 against 1,060 / 27,915; live p50 192 / p99 1,257 against 192 / 1,256. So the gap costs nothing measurable where it does not bind; a read that meets it with no answer has no timer of its own, so on the site host its round waits for the next answer, read or leader tick, about TICK (100 ms) at worst, not 20 ms [inference: `read_round` runs only from `start_read` and `check_reads`, and `check_reads` only from `after_ack`, `start_read` and `tick_leader`; TICK is 100 ms in `workers/site/src/host.rs`]. B0-33's hosted read-grant p99 checks it. The long sweep passed (2,000 HARSH, 2,000 GRAY, 4,000 live seeds), and Porcupine on h, h.gray and h.live returned Ok on 153,246, 2,246,286 and 5,965,959 operations (`tests/m0/results/read-round-gap.md`).

**Flip.** A hosted B0-33 whose voters still send more than one read round per voter per 20 ms; a hosted row whose reads wait measurably longer for their grant than on main; or a gray seed past the 30 s bound that traces to a read waiting out the gap.

### `fanout-dups`: the read round's slot is timed on the isolate's own clock, one round per voter per turn, 2026-11-16

**Decision.** `read_round` reads its gap and its answered-slot rule on the isolate's own clock (`Member::set_own_clock`, which the site host sets to `Date.now()` at every input), not on the floored member clock. A new round leaves only when the own clock has moved since the last one, and then only once the last is answered or `READ_ROUND_GAP` (20 ms [guess]) has passed. A Workers isolate's clock moves only between I/O turns, so this sends at most one round per voter per turn. Which reads share a round, and which round grants a read, stay on the member clock: a grant still needs a voter quorum's ack at or after the read arrived. DESIGN states it in Shape, "The read path".

**Why.** read-round-gap's flip fired. In B0-33 m39 (main a0bca583), turns with 10 or more calls to one voter carried 3,834 of site 0's 7,921 voter-to-voter calls, and 1,146 of the 1,245 `AppendQueued` rows in those turns repeated a last index the turn had already queued (164 of 174 on site 1, 101 of 106 on site 2). Those calls past a turn's sixth waited for a fetch slot: legs p50 1,000 ms for calls 1 to 6, 1,434 ms for 7 to 12 and 2,693 ms for 13 or more [measured, `tests/m0/results/fanout-dups.md`]. In the simulator, with a turn's inputs sharing one own-clock reading, the member clock jumping 20 ms or more at every fifth input, and answers from earlier turns arriving, every repeat came from `read_round`. Floor jumps of 20 ms or more expired the gap, and answers freed the slot within the turn: up to 8 rounds to a voter in one turn, 2,264 of 2,402 rounds repeating a last index already sent that turn. Heartbeats, retries, probes and catch-up sent none [measured, scratch count in `tests/m0/results/fanout-dups.md`]. Whether the floor jumps that far on the hosted voters is [inference]: the trace records the raw clock only.

**Beat.** A per-turn counter in the site host: the turn boundary is the host's fact, but the own-clock reading carries it into the member without a new message or state. The gap alone on an elapsed-time clock with answers still freeing the slot at once: answers that land in the same turn would still send a round each, at most one per answer. Coalescing at the host's outbound queue: it would hide the duplicates without stopping the member from making them.

**Evidence.** `reads_send_at_most_one_round_per_voter_per_turn_while_the_floor_jumps` (`crates/raft/tests/scenarios.rs`) fails on main with 7 rounds to each voter in the first turn, and passes with one. `tests/m0carried/plant25.py` catches 6 of 6. The long sweep passed with the same longest stalls as main (harsh 15,739, gray 27,508 and live 26,186 ms, none over 30 s), and Porcupine on h, h.gray and h.live returned Ok on 153,246, 2,246,286 and 5,965,959 operations (`tests/m0/results/fanout-dups.md`).

**Flip.** A hosted B0-33 whose voters still send more than one read round per voter in a turn; a hosted row whose read-grant p99 rises above round 29's 2.3 to 2.9 s with the turn bound as its cause; or a gray seed past the 30 s bound that traces to a read waiting for the next turn.

### `r22-sweep-fill`: the sweeper's filler streams a copy from bucket to bucket, 2026-11-15

**Decision.** The third-bucket filler (`graftyard_git::sweep`, `fill`) copies a committed pack and its stored `.idx` with `Bucket::copy_new`. On Workers the source's body is piped into a `FixedLengthStream` of the marker's length, and that stream is the create-only put's body, with the marker's SHA-256 as the put's `sha256`. No byte of the copy enters wasm memory, and R2 refuses a copy whose length or SHA-256 is not the marker's.

**Why.** Every run of site a's `*/15` cron on `graft-m2b` since 2026-11-13 ended "Promise will never complete." at wasm 150 to 195 MB, and site a's orphan sweep and fold audit had not finished since (R22). The fill read each pack whole to check and re-put it, 16 at once. Network 131296, B0-34's 5 GB monorepo, has 71 third-bucket copies to fill, 996 MB, and the first 16 are 8 to 53 MB each, 328 MB in all; the failing runs' only R2 GETs were those packs. The cursor is stored only at the end of a run, so each run started at 131292 and died on 131296.

**Beat.** (a) A lower fill concurrency: one 53 MB pack read whole is already a third of an isolate's 128 MB, and promoted packs reach 207 MB. (b) Multipart copies in 16 MiB parts through wasm, as the edge's tee does: it bounds memory but adds the part buffers, upload journals, and abort paths for a copy the runtime can make itself. (c) Storing the cursor before each network: the run would still die on 131296, one network further on.

**Evidence.** `tests/am1/results/r22-sweep-fill.md`: the failing run's trace and R2 reads, and the local workerd bed (`tests/r22/bed/run.sh`), where one sweep of 16 fills of 20 MB took 382.3 MB of wasm memory on `origin/main` and 1.3 MB with the change, with every copy byte-identical and wrong lengths and checksums refused. `sweep::tests::committed_pack_in_one_bucket_with_a_marker_is_filled_never_deleted` fails on main (218 bytes read, 0 expected). `tests/r22/plant.py`: 6 of 6.

**Flip.** A hosted cron run that streams a pack but whose put the binding refuses for its length, or a platform change that drops `FixedLengthStream` as a known-length body for R2 `put`.

### `disk-validate`: a push validates on its shard's disk as it stands, and catches up only for a missing base, 2026-11-15

**Decision.** A shard object that has caught its running container up validates a push against that disk as it stands, with no catch-up first (`graftyard_git::validate_on`, `workers/site/src/shard.rs`, `"/validate"`). The network shard forwards to the pool shard at once, and each starts a catch-up in the background. The container's `/validate?disk=1` answers 409 `missingBase` and keeps the request, its spooled pack included, when the disk lacks something the push needs: `index-pack --fix-thin`'s "unresolved delta" or `--strict`'s "did not receive expected object", the connectivity walk's "bad object", "bad tree object", "missing … object" or "Could not read", an update's old tip that is not the disk's ref, or a new commit that no disk ref reaches and the pack does not hold (`containers/git/server/main.go`, `missingBase` and `behind`). The shard then waits for a catch-up that started after the push arrived (`graftyard_git::CatchUps`), not for the run in progress and then another, and asks `/validate?held=<id>`, which checks the kept request as a validate on a caught-up disk does, so every failure there is the pack's 422. Every other failure on `disk=1`, a truncated pack or an fsck error, stays a 422. The archive's promotion, and a container the object holds no cursor for, catch up first as before.

**Why.** m3fr5c's B0-32 burst ran site a's validates at p50 5.4 s with k = 4. Each forwarded push waited out the network shard's catch-up, then the pool shard's, and `Jobs::wait` made each wait cover 1.4 to 1.9 runs. Each run grew to 4 to 9.5 s because every shard ingested every burst pack (about 564 packs and 145 MB each). The container's own work was p50 0.36 to 0.9 s [measured, Workers Logs, `tests/am1/results/lease-clock.md`, "B0-32"]. Replayed on that burst's arrivals, shards and work times with a catch-up of 412 ms plus 37 ms a pack (the fit to its 182 runs), this order gives validate p50 1.0 s, p90 6.9 s and 579 of 651 within 5 s, against main's order at p50 7.7 s, p90 14.9 s and 237 [inference, `crates/git/tests/disk_validate.rs`; base moves every 6 s is a guess].

**Safety.** A push accepted on a disk that is behind names only bases that are present there. The disk holds only committed packs: `/ingest` and catch-ups add them, a validated pack waits under `graft/validated-*` outside the alternates until its commit, and nothing prunes, since repack is `-d --geometric` with no `prune`. Its refs name only whole tips, because `applyRefs` skips a ref whose object is absent. So `index-pack --fix-thin` resolves a delta only against an object a committed pack wrote, and `rev-list --objects --not --all` stops only at a disk ref tip whose history is whole or fails on the first absent object, which is a missing base. Both answers hold on any later disk, since disks only grow. The detail reads refs, so it is taken only when each update's old tip is the disk's ref and every new commit is in the pack or reachable from a disk ref. A newer ref elsewhere can still make the merge base earlier than the leader's, which widens `changed` and `protected` and never narrows them. The leader's `decide` checks every ref's expected old value against the fold, so a stale disk can never move a ref [inference from the code; `TestValidateOnTheDiskAsItStands`].

**Beat.** Forwarding before the network shard's catch-up alone: replayed p50 3.9 s, a known fail of the 5 s line. Raising the container's gate: it turns `full` answers into slow acks and leaves p50 where it is. Waiting on the running catch-up only when it already covers the base: replayed, it holds 174 pushes against 98, at the same p50, because a run's read point is not known before it ends.

**Evidence.** `TestValidateOnTheDiskAsItStands` and `TestMissingBaseWording` (`containers/git/server/disk_test.go`); `b032s_burst_validates_in_about_a_second_on_the_disk_as_it_stands`, which fails on main's order (`crates/git/tests/disk_validate.rs`, `b032_m3fr5c.csv`); `tests/m0carried/plant19.py`.

**Flip.** A hosted B0-32 where held pushes reach a large share of the burst, so the second check's catch-up wait dominates p99 again; or any push accepted on `disk=1` whose pack does not connect on the caught-up disk, which would void the safety argument.

### `promote-unlock`: the shard writes a promoted pack with no network lock held, 2026-11-15

**Decision.** The container's `/promote` builds the promoted pack under the network's shared lock and releases it before it writes the pack to the caller (`containers/git/server/main.go`, `handlePromote`). The caller is the archive, whose tee streams the pack to the buckets and to the promotion's own validate (`workers/site/src/archive.rs`, `promote_on`). MergeQueue merges keep `MERGE_WAIT` and still answer `warming` past it.

**Why.** B0-31 m3fr6 got 11 MergeQueue merges refused with 503 `the git cache: … {"warming":true}`, and site c's (apac) shard logged 18 `/merge?method=` calls that waited the full 15 s. 15 of the 18 overlapped one of three 60 s stalls on that shard, at 21:24:49Z, 21:29:23Z and 21:31:32Z. Each began within 1.2 s of a `/promote` and lasted until the promotion's `derived=1` validate gave up at `PROMOTE_WAIT_S` (59,982, 59,993 and 59,992 ms). A catch-up that started 18 ms before the first one logged 60,371 ms for 4 packs and 82 ms of index-pack. The next ran 21.8 s for the 248 packs that queued behind it, and four `/catchup` requests waiting since 21:29:24Z answered together at 21:30:24.5Z. Outside the stalls the shard's catch-up runs were p50 1.0 s, p90 2.8 s and p99 5.9 s, and merges p50 472 ms [measured, Workers Logs on `graft-m3f-site-c`, `tests/am1/results/promote-unlock.md`]. The catch-up was not slow; it was blocked. `/promote` held the shared lock while it wrote its pack. Its reader, the archive's tee, feeds a one-chunk pipe to the derived validate, and that validate waits for a catch-up before it reads (`validate_on`, `derived`). The catch-up's `/ingest` waited for the lock alone, and Go's `RWMutex` then queued every later reader behind it: validates, merges and ls-refs. Nothing moved until the validate's 60 s timer let the pipe stop. That is a cycle across the container and the isolate [inference from the code and these timings]. `TestPromoteWritesItsPackOutsideTheLock` reproduces it: an `/ingest` and a merge wait for 20 s and more on main, and answer in about 1 s with the change.

**What it beat.** (a) Raising `MERGE_WAIT`: each stall lasted `PROMOTE_WAIT_S`, so a merge would wait 60 s and still miss B0-31's 3 s line. (b) A MergeQueue merge that waits out its catch-up instead of refusing: the numbers do not call for it. Outside the stalls merges were p50 472 ms and the shard's catch-ups p99 5.9 s, and the 3 refusals outside a stall waited behind the 21.8 s run that drained the first stall's queue [inference: with no stall, the catch-ups that hold a merge are the p99 5.9 s runs, under `MERGE_WAIT`]. (c) Letting the promotion's validate skip its catch-up: its pack is built from the same disk, but `validate_on` keeps derived work caught up first so its index is checked against every committed pack, and that is out of scope here. (d) Buffering the pack in the container before writing: the same lock order with more memory, since packs reach 207 MB.

**Evidence.** `TestPromoteWritesItsPackOutsideTheLock` (`containers/git/server/promote_lock_test.go`) fails on main and passes with the change; `TestPromotedPackIsCompleteAndHoldsExactlyTheRange` is unchanged. `tests/m0carried/plant21.py` plants the pack written under the lock, the lock released twice, and a refused promotion that keeps the lock. m3fr4's B0-31 (31 refusals) had the same 15 s merges on its apac shard, but its catch-up lines show 92.5 s and 238.8 s runs of 159 and 388 packs, the stored-fetch rate before `stored-fetch`, and no `/promote` in that window [measured]. So m3fr4's refusals had another cause, which later branches fixed, and are not evidence for this one.

**Flip.** A hosted B0-31 that still refuses MergeQueue merges `warming` with no 60 s shard stall, which would make (b) the fix; or a stall that starts at a `/promote` with this change in place, which would mean another lock in the path.

### `repack-keep-pack`: the repack names every kept pack with `--keep-pack`, 2026-11-16

**Decision.** `runRepack` passes `--keep-pack=<pack>.pack` for every pack with a `.keep` beside it, on top of `repack-keep`'s geometric roll at nice 19 with `pack.threads=2` and `pack.windowMemory=256m`. The kept promotions stay byte for byte as before; the roll's history walk now stops at them. DESIGN states it in Shape, `rehydrate`.

**Why.** On m37 (LM-1 round 26) the repack's `pack-objects` showed 9.6 to 10.9 GB RSS on every shard of repo:131296, beside up to 22 stock depth-1 upload-packs. A geometric repack runs `pack-objects --stdin-packs`, whose walk for name-hash hints stops only at packs kept in core (`--keep-pack`, or excluded with `^`), not at a `.keep` on disk (git 2.56.0, `builtin/pack-objects.c`, `stdin_packs_include_check`). So it walked from the tail's commits through every promotion's trees. Locally, on 33 kept promotions of B0-34's plan (6.5 GB) plus a tail of 21 one-step pushes and 1,200 agent packs, main's repack peaked at 2.24 GB RssAnon and 4.16 GB RssFile in 119 to 143 s; with `--keep-pack`, 1.69 GB and 1.45 GB in 68 to 87 s, the same 34 packs after, connectivity ok. The tail's pack count does not move it: with 12 tail packs instead of 1,221, main's peak was 2.23 GB and the fix's 1.69 GB [measured, `tests/fanout/repackmem.sh`, `tests/am1/results/lm1-memory.md`, round 27]. RSS from statm counts the mapped pack files, so most of the hosted 10.9 GB was promotions' pages the walk touched [inference from the RssFile split]. The 22 stock upload-packs held 0.33 to 0.46 GB private memory each, 8.85 GB summed, which is the larger private consumer beside the repack; that path is the composed-miss work's.

**Beat.** A lower `pack.windowMemory`: it bounds the delta window per thread, not the walk. Rolling fewer packs at a time (a cap on packs or objects per roll): the tail's pack count did not change the peak. A container memory cap on the repack: the container is one cgroup without delegation (`repack-keep`).

**Evidence.** `TestRehydratesRepackLeavesPromotedPacks` (`containers/git/server/repack_test.go`) fails on main: with `GIT_TRACE_PACK_ACCESS` set, the repack read 106 trees of 53 kept promotions; with the change it reads fewer than one per promotion (the test's bound). `tests/m0carried/plant17.py` plants `--keep-pack` dropped, naming the `.keep` file, and naming only the first kept pack; 14 of 14 caught.

**Flip.** A git release whose `--stdin-packs` walk honours on-disk `.keep`s (the flag becomes a no-op), or a hosted repack whose `pack-objects` private memory grows with the promotions again.

### `promote-lapse`: a due range's promotion starts a shard but does not keep a quiet one warm, 2026-11-16

**Decision.** A due range's promotion sends its shard calls (`/promote`, its `/validate`, `/idx`) with `warm=start`, read as `graftyard_git::Caller::Start`. It starts a stopped shard, as before, since the log cannot archive past the range otherwise, but records no request. When the shard's `last_request` is `WARM_FOR` old or older, or absent, each such call sets the container's inactivity timeout to `PROMOTE_LAPSE` (10 minutes [guess]) instead of the 6 h a start sets (`graftyard_git::promote_lapse`), so the container stops 10 minutes after the drain run's last promotion. A shard a client used within `WARM_FOR` keeps its 6 h timeout and renewal, and a client's later request sets 6 h again (`ShardHost::touch`, `on_alarm`).

**Why.** On `graft-m2b`'s idle check after the m39 site day (LM-1 round 34), repo:131330's due range started its shard five times on three sites. The archive alarm's due-range `/promote` had no `warm`, so it was `Caller::Client`: `serve` touched the shard, `last_request` moved to the call, and the renewal alarm then held the container `WARM_FOR`, 24 h, renewing the 6 h inactivity timeout each hour [from the code: `crates/git/src/shard.rs`, `serve` and `renewal`; `crates/git/src/host.rs`, `touch` and `on_alarm`]. Only the reaper's teardowns stopped them. A quiet group whose log reaches its archive band would bill a container for a day per promotion run.

**Beat.**
- Making a due range's promotion warm (skip a stopped shard): a stopped group with a range due would never archive, and its log would grow past the cap (`promote-warm`, Beat).
- Stopping the container when the promotion returns: the drain run's next promotion, minutes later, would start it again from nothing, a cold rehydrate per step.
- A `PROMOTE_LAPSE` on every start, client or not: a client's start must keep `WARM_FOR`.

**Evidence.** `a_due_range_promotion_starts_a_shard_and_lets_a_quiet_one_lapse` (`crates/git/tests/shard.rs`) has no `Caller::Start` or `promote_lapse` on main. `tests/promotelapse/plant.py` catches 6 of 6: `start` read as warm, `Start` touching, a lapse despite a recent client, a lapse for every caller, a never-used shard counted as recent, and a lapse as long as the inactivity timeout. The 10 minutes is [guess] until a hosted idle check shows a quiet range's shard stopping within it.

**Flip.** A hosted drain run whose promotions are more than `PROMOTE_LAPSE` apart, so each one starts and rehydrates its shard again, which would lengthen the lapse.

### `promote-warm`: promote-ahead's `/promote` is a warm call, 2026-11-16

**Decision.** The archive alarm's promote-ahead (a promotion with no range due to archive) sends its shard calls (`/promote`, the `/validate` of its pack, and `/idx`) with `warm=1`. The shard reads `warm` on any route as a warm caller (`graftyard_git::caller`; `warm=pool` still means the pool warm, on `/catchup` only), so a stopped shard answers `skipped` with `x-graft-skipped: stopped`, starts nothing, and records no request. A running shard promotes as before, but promote-ahead never keeps it warm. When every site's shard skipped, the step is idle, and a later alarm on a running shard promotes the same range. A due range's promotion, which archiving and the log cap wait on, is still a client's call and starts a shard. DESIGN states it in Shape, `Promoted`.

**Why.** On idle `graft-m2b` (LM-1 round 28) the meter rose $9.16 in six hours, $7.62 of it containers: 19 shard containers on eight repositories no run had touched since the day before. Each group that a read woke, with an unpromoted tail above `LOG_MAX_BYTES / PROMOTE_AHEAD`, ran promote-ahead. Its `/promote` was `Caller::Client`, so it started the stopped container and touched it, which `WARM_FOR` then held for 24 h. A new leadership's warm catch-ups at the same moments skipped, as "Idle git cache shards stay stopped" made them [measured in Workers Logs, `tests/am1/results/lm1-memory.md`, round 28]. Promote-ahead came later (stored-fetch, round 20) and was never put under that rule.

**Beat.** Skipping promote-ahead when the leader sees no running shard: that needs a shard call to ask, and that call races the start the same way. Marking only `/promote` warm: its `/validate` on the same shard ran as a client and would touch it. Making every promotion warm: a stopped group with a range due would never archive, and its log would grow past the cap.

**Evidence.** `promote_ahead_starts_no_stopped_shard_and_a_due_range_does` (`crates/git/tests/log_archive.rs`) and `a_warm_query_on_any_route_is_a_warm_call_and_pool_only_on_catchup` (`crates/git/tests/shard.rs`) fail on main. `tests/promotewarm/plant.py` plants promote-ahead as a client, every promotion warm, a skip as an error, `warm` read only on `/catchup`, the pool warm on every route, and a call without `warm` as warm.

**Flip.** A quiet group whose unpromoted tail stays above the bound long enough that its next cold rehydrate misses B0-34's 120 s line (G10), which would let promote-ahead start a shard for a group with a client in the last hour.

### `registry-restore`: a re-placement puts the kept member's registry row back over its tries', 2026-11-16

**Decision.** When `replace_on` (`/replace`, `/place`) finishes, it sends one `PUT /row` to the re-placed site's `MemberRegistry`: a `RowPut { row, replacing }` whose `row` names the member the group kept on that site, the old member put back with `Normal(p)` or the try that went in, at that member's own generation, and whose `replacing` names every other try. The registry applies it only when its stored row names one of those tries (`RowPut::replaces`), at any generation. A member's own write, sent as a bare `RegistryRow` as before, still never moves the row to a lower generation. DESIGN states it in Shape, `MemberRegistry`.

**Why.** Each try's `/join` writes its row before the leader checks its placement, at a generation above the kept member's, and `PUT /row` refused only a lower generation, so a try that never went in stayed the site's row. In round 29 (B0-33 on `graft-m2b`), site b's row for `repo:131342` named `#9#f1`, a try retired at 06:24:46Z, while the group's voter there was `#3#f1`. The edge's registry stage read that row about 61,000 times per site in 303 s and sent `/read` to `#9#f1`, which answered 63,387 409s (B0-02's analysis, `tests/am1/results/lm1-memory.md`, round 31).

**Beat.** Writing the kept member's row under a winning generation (the next one past the tries): the row would then name `#3#f1` at a generation the member was never built with, and readers that rebuild names or records from a row's generation (`registry_record`, `fold_audit`, `activity::rebuild`) would read a wrong one. A compare-and-set "replace if `do_name` is X" needs no new generation and checks and writes in one step on the registry object, which serves one request at a time. Writing a try's row only once its `Normal` commits: `/join` refuses a member whose site's row names a later generation, and that check runs on the try's own join, so it would move. The edge-side change (no `/suspect` and drop the row for the pass on an unjoined 409 at a registry name) is already the lookup's behavior: `classify` answers `Skip` there, which reports nothing, and `tried` drops the name for the pass. The 133 bursts B0-02 counted came from the voters missed in the same failed passes [inference from the lookup's code].

**Evidence.** `a_placement_that_kept_its_member_puts_its_row_back_over_every_try` (`crates/types/src/registry.rs`) replays round 29's tries `#4` to `#9` over `#3#f1` and fails on main, which has no restore and refuses any lower generation. `tests/registryrestore/plant.py`: 8 of 8 caught.

**Flip.** A registry row that must name a member not in the group's config, such as a joining member during a move's `Joint`, which a restore by name alone could replace; or a hosted lookup still reading a try's name after a placement under this change. Known gap: a try's own `/join` write that lands after the restore, at its higher generation, still wins over the restored row, since a bare row is compared by generation only. A retired try (test installs) sends none, but a product-build try is not retired and could; a hosted lookup reading a try's name after a placement would show it (OPEN-QUESTIONS G19).

### `feed-ahead`: a feed cursor past the member's last entry answers `lagging`, not 410, 2026-11-16

**Decision.** `graftyard_git::feed::start` returns `Start::Ahead` for a cursor of the group past the member's last applied entry and at or below the highest commit index the member knows: its own, or the largest a peer sent it this boot in an AppendEntries' `commit`, a read index, or an install's `through` (`Host::heard`, `Host::committed`). The member answers such a poll `lagging` (`Following::behind_cursor`), however recent its own last entry, so the edge's `activity` asks the next member, as for any lagging member. The edge's last resort (`lagging_ok`, every member tried lagged and the least lagged is a learner) still holds it there, reading after the cursor, which is an entry the group committed. A cursor past that commit index is 410, as are a cursor of another group, a malformed one, one below the ring's floor, and one inside an `InstallSnapshot` hole. `design/AGENT-LAYER.md`, "Activity feed", states the rule and the bound in the `FeedCursor` paragraph; the m3fr7 numbers behind it are under Why here.

**The bound.** A whole-install restore keeps the group's `RepoId` and can end its log below a cursor a client holds. Bounded only by the member's last applied entry (`41c93e72`), every member would answer that cursor `lagging`, the last resort would hold it on a learner, and once the restored log grew past it the client would get events only from there on, skipping those between the restore point and its cursor. Bounded by the commit index, the cursor names no entry any member knows committed, and gets 410. A member that heard no leader since it booted knows only its own commit, so it answers 410 to a cursor ahead of it, as before this change; that costs a reload, never an event.

**Why.** A poller held on a caught-up learner carries that learner's cursor to the next member it asks. A learner behind it answered 410, and the edge returned the 410 to the client at once, without trying another member. On L0-12 m3fr7, site c's two apac learners answered 6,176 of 20,670 polls in under 20 ms, against 632 of 21,795 on site a's four, and the driver counted 6,574 `gone` [measured, Workers Logs, `tests/am1/results/lm1-rows-m3f.md`, "m3fr7 after the run"; the logs carry no bodies, so tying the two counts is an inference]. A real client reloads its leases and tasks on a 410, and the design gives 410 only to a cursor older than the ring or inside an install hole.

**Beat.** Holding the poll on the member behind until it catches up: a member behind the cursor is behind the one that answered it, which is the lagging rule's case, and holding it there would let the slowest learner set the event latency. Answering 410 with the cursor itself as `oldest`: still a 410, which clients act on.

**Evidence.** `a_learner_behind_the_cursor_answers_lagging_not_gone` (`crates/git/tests/lease_gates.rs`) fails on main (the cursor is `Gone`) and passes with the change; the ring's other cursor tests are unchanged. `a_cursor_past_the_groups_commit_index_is_gone` (same file) and `a_cursor_past_the_groups_commit_index_is_gone_even_on_the_last_resort` (`workers/site/src/feed.rs`, a `lagging_ok` poll on a learner) fail on `41c93e72` and pass with the bound. `tests/m0carried/plant22.py`, 8 of 8. The driver half (`tests/am1/rest/a1.mjs` follows `oldest` on `gone`, and reports each member's answers as `feed.byMember`) is checked by the next L0-12.

**Flip.** A hosted L0-12 where `byMember` shows learners answering `lagging` to a cursor they hold, which would mean the cursor's index is not comparable across members; or one where the edge's routing on these `lagging` answers drives polls into 429 more than the 410s did.

### `feed-gone-route`: one member's `gone` sends the poll on; the edge answers 410 only when every member it tried did, 2026-11-16

**Decision.** `graftyard_git::feed::route` weighs a member's `gone` as an answer that holds nothing (`Answer::Gone`). The edge's `activity` asks the next member after a `gone`, as after `lagging`. Its 410 goes back only when every member it tried answered `gone` (`Route::Gone`), with the first one's `oldest`. Otherwise the members that answered decide, as before: the first `hold`; 429 at once from a full member; the least lagged learner under `lagging_ok` when every one of them lagged; else 429. The member's rule is unchanged (`feed-ahead`): a cursor past the commit index it knows is `gone`. `design/AGENT-LAYER.md`, "Activity feed", says it in the `FeedCursor` paragraph.

**Why.** On L0-12 m3fr9, with no leader change in the replay, the feed's outside-kills p99 was 25,210 ms. 858 of the row's 869 `gone` answers came from site 2's two learners, `l2` (626) and `l5` (232) [measured, `tests/am1/results/lm1-rows-m3f.md`, "m3fr9 after the run"]. Those learners did not fall behind on catch-up. Each minute of the replay, all 8 learners were within about 20 entries of the leader. But site 2's learners apply and hear commits later (apply delay behind the leader p50 1,008 and 717 ms, p99 4,074 and 1,971 ms, against p50 181 to 316 ms on site 0's) [measured, trace rows]. A poller carries a cursor from the learner that last answered it to the next one in its rotation. That cursor was past the known commit of `l2` 84.2% of the replay and of `l5` 82.5%, against 4.7 to 23.1% for the others [measured, same trace; that this explains the `gone` counts is an inference]. Each such `gone` went back to the client at once. The client reloaded from `oldest` and was sent events it had already seen, counted as whole-replay latencies. The design gives a client 410 only for an expired cursor or a snapshot hole, and these were neither: the next member held the cursor.

**Beat.** Raising FEED_RING: the cursors were not out of the ring, so a larger ring changes nothing here, and it would hide a learner that really fell behind. Holding the poll on the late learner until it hears the commit: that is `lagging`, and the edge already prefers a member that holds it now. Letting the late learner treat a cursor up to its own last appended entry as `lagging`: the restore case (`feed-ahead`, "The bound") needs the commit bound on every member, and the edge already has the other members to ask.

**Evidence.** `a_learner_that_has_not_heard_the_cursor_committed_does_not_end_the_poll` (`crates/git/tests/lease_gates.rs`) fails on main, where the first `gone` is the poll's 410, and passes with the change. The restore case is unchanged: `a_cursor_past_the_groups_commit_index_is_gone` and the `lagging_ok` test still pass, and every member answering `gone` is still a 410. `tests/m0carried/plant26.py`: 4 of 4. `tests/am1/rest/a1.mjs` routes the same way, so the next L0-12 measures the rule as the edge runs it.

**Flip.** A hosted L0-12 where `feed.byMember` shows the polls a `gone` sent on going to 429 more often than they are held, which would make returning the 410 at once the cheaper answer; or a restore test where a client's cursor past the restored log gets anything but 410.

### `push-reads`: a push reads its repository rows once, starts its reads together, and skips the probe for a known leader, 2026-11-16

**Decision.** Four changes to a push's path before and around its commit (`design/DESIGN.md`, journey 1, step 1):
1. **One rows read.** The read gate asks the repository group for `{"rows": {"access": true, "agent": true}}` on a push under 32.8. The member answers both row sets with `"agent": true` beside them (`workers/site/src/family.rs`), and `agents::push_context` reuses them (`Ctx::push_rows`). A member built before it answers access alone without the flag, and the edge then reads the agent rows separately, as before.
2. **Concurrent starts.** On git routes, `authenticate` runs beside `resolve`. On a push whose caller reads the install's `Authz` anyway (`needs_install`), that read starts once the token answers, still beside `resolve`. `install_authz` reads once per request (`Ctx::install`), so `admission` reuses `push_context`'s read.
3. **No probe for a known leader.** `commit()` skips `ensure_group`'s probe `/read` while `graftyard_lookup::knows_leader` holds. Any `NotLeader` clears the leader (`commit_hedged` calls `forget_leader`). So does a reply whose `meta.term` is below the highest this isolate saw from the group (`learn_leader`). The next commit then probes and looks the leader up again.
4. **Stage first on a group seen hot.** A group whose leader answered this isolate's push `Stage` within `HOT_WINDOW` (5 s) is hot here (`workers/edge/src/lib.rs`, `mod hot`). A push to it that has an admission class and an inline pack over `INLINE_MAX_HOT` places its pack before its first commit. Nothing else marks a group hot, and the mark lapses `HOT_WINDOW` after the last `Stage`. A cool group's push keeps its inline pack. The leader's own `hot_stage` rule is unchanged and still decides: a pack staged early is a `Stored` pack it takes as it does any other.

A test install answers each push with its stages as `server-timing`: `resolve`, `auth`, `install`, `gate`, `context`, `validate`, `admission`, `commit`, and `stage`. `Acked` is still built only inside `outcome`: the edge's changes are reads and the choice of a pack's form before `commit`.

**Why.** In B0-32 m3fr7, every burst push took 14.6 to 23.5 s.
- Before validate, two of five traced pushes spent 17.3 and 10.9 s in reads that ran one after another, many at about `FOLLOWER_WAIT` on the group's 8 learners. Before validate was 34% of the five pushes' time, and first commit to end was 36%, with 1 to 4 commits and up to 10 `/suspect` each [measured, `tests/am1/results/lm1-rows-m3f.md`, "m3fr7 after the run"].
- The path read the repository group twice (access, then agent rows) and the install's `Authz` twice (`push_context`, then `admission`). It ran the token after the resolve, and sent a create probe before every commit, the `Stage` re-commit included. The hot group answered `Stage` to every 256 KiB push after its first commit.

**Beat.**
- Sending a burst's reads to the leader: settled against, because the leader is the burst's bottleneck.
- Raising `FOLLOWER_WAIT` or `COMMIT_RECEIPT`: settled against, because it moves the wait rather than removing a read.
- A hot flag in the leader's replies: it needs a raft change and a reply field. The `Stage` answer already says it.

**Evidence.**
- `a_push_reads_its_access_and_agent_rows_at_once` (`workers/site/src/fanout.rs`) fails on main (`agent` is null) and passes with the change.
- `a_commit_probes_again_after_not_leader_or_a_stale_term` (`crates/lookup/src/lib.rs`) shows the fallback: no probe skip before a leader is known, after `NotLeader`, or after an older term's reply.
- `only_a_group_that_answered_stage_within_hot_window_stages_first` (`workers/edge/src/lib.rs`) shows that a cool group never stages first at 1 B to 1 MiB, and that the mark lapses 1 ms past `HOT_WINDOW`.
- `tests/m0carried/plant23.py` catches 10 of 10.
- The time saved is [inference] until B0-32's rerun reads the new `server-timing`.

**Flip.** A hosted B0-32 where `gate` plus `context` is still most of a push's wall, which would leave the follower read wait as the cost. Or one where pushes that stage first wait longer than a `Stage` round trip, which would mean the bucket place is slower than the leader's refusal.

### `receipt-rule`: a busy leader's slow receipt is waited on, not reported or re-sent, 2026-11-16

**Decision.** Three changes to a push's commit and validate (`design/DESIGN.md`, "Commit and read paths" and journey 1, step 2):
1. **The receipt rule** (`crates/lookup/src/lib.rs`, `receipt`). Each edge isolate records, per group and member, when that member last answered it a 200 on the commit path (`/commit` or `/outcome_by_request`) and in which term (`heard`). A `/commit` send that has no answer at `COMMIT_RECEIPT` (2 s) from a member that answered within the last `COMMIT_RECEIPT`, in the highest term the isolate has seen from the group, is busy, not gone (`busy`):
   - The send stays open up to `RECEIPT_OPEN` (8 s [guess]). Nothing is reported and nothing is sent again.
   - Each `COMMIT_RECEIPT` the edge asks the same member `/outcome_by_request` with the request id. It takes a `done`, a `NotLeader`, or a `pending` (a ticket it then waits on with `outcome`). `unknown`, any other answer, or none leaves the send open.
   - A member with no such 200, or one that answers an error status, is reported with `suspect` and sent the command again, as before.
2. **Outcome lookup without the pack** (`workers/edge/src/lib.rs`, `lookup_body`). The `/outcome_by_request` body is the same request id and the push with `pack: None`. A push's `replay` reads only its ref names, against the events its `request` row stores, so the leader answers the same without the up to 1 MiB inline pack. `crates/raft` is unchanged, and so is the member's `/outcome_by_request` route.
3. **Validate's record and `touch`.** The edge's `/validate` names the owner's placement record, as `/merge` and `/demand` do, so the network shard skips its two `Names` reads. A shard records a client's request (`last_request` and the renewal alarm) at most once per `TOUCH_EVERY` (1 min, `graftyard_git::touch_due`).

A test install also logs one line per push with an isolate token and the pushes that isolate held (`push_log`).

`Acked` is still built only inside `outcome`. Every reply the edge takes is the member's: the send's own 200, or a `done` the leader built from its `request` row through `outcome`'s `replay`. A `pending` is a ticket, not a success.

**Why.** On B0-32 m3fr8 the edge's commit stage was 5.5 s at p50 against the leader's own 427 ms [measured, `tests/am1/results/lm1-rows-m3f.md`, "m3fr8 after the run"]:
- 110 of 121 extra `/commit` sends followed a missed receipt from a leader that answered 200 to all 201 sends. 71 of those sends had handlers under 2 s, and each resend carried the 256 KiB pack again.
- The leader held up to 572 open requests.
- The misses sent 82 `suspect` waves, 27 to 32% of each follower's inbound, with no leader change. B0-02 found the same rule behind 29 to 40% of voter inbound on graft-m2b.
- Validate's 3.0 s p50 was the network shard waiting before its forward, in two `Names` reads that the record makes unneeded.

**Beat.**
- Raising `COMMIT_RECEIPT`: settled against (`push-reads`). It also delays the report on a leader that really is gone.
- Dropping `suspect` on missed receipts: the receipt is the edge's only hedge against a lost leader (BF-14, the data-center row). The rule keeps it for a member this isolate has not heard from.
- Matching a digest of the command at the leader: the replay check needs no command bytes beyond the ref names, so a digest adds a raft change for nothing.
- A hot flag in leader replies: needs a raft change (`push-reads`).

**Evidence.**
- `a_slow_200_from_a_busy_member_is_the_reply_with_no_resend_or_report` and `a_member_that_never_answered_is_still_reported` (`crates/lookup/src/lib.rs`) both fail with main's rule in the same harness, where every miss at `COMMIT_RECEIPT` is reported.
- `a_resent_request_is_answered_from_its_row_and_applied_once` (`crates/git/tests/push.rs`) shows that `outcome_by_request` answers the same refs and report-status with the pack removed.
- `a_push_asks_for_its_outcome_without_its_pack` and `a_validate_url_names_the_packs_trailer_its_class_and_the_record` (`workers/edge/src/lib.rs`) cover the edge. The second fails on main, whose URL has no record.
- `a_shard_records_its_last_request_at_most_once_a_minute` (`crates/git/tests/shard.rs`): no `touch_due` on main.
- `tests/m0carried/plant24.py` catches 14 of 14.
- The time saved is [inference] until B0-32's rerun.

**Flip.**
- A run where a leader that answered 200s stops answering and its group waits `RECEIPT_OPEN` without a campaign. That would mean the busy window lets a gray leader keep its lease.
- A rerun of B0-32 where the commit stage stays far above the leader's own commit time with no resends. That would put the wait in the legs, not the resends.

### `r24-app-workflow-job`: the install delivers `workflow_job` to the Apps installed on the owner, 2026-11-16

**Decision.** `RunOwner` appends each job's `workflow_job` steps to the repository chain as `RepoEvent::WorkflowJob` (tag 46, format 32.24) in the same `RunChecksCmd` as its check rows: `queued` when the job enters `Queued` (at `RunCreated`, or at `NeedsMet` for a job with `needs`), `in_progress` at `Acquired`, `completed` at its conclusion. It reads the steps from the shard's entries after its cursor, not from the fold, and `decide` drops a step at or below the job's metadata `workflow_job` row. The `Webhooks` consumer delivers them, and every other event it renders, to each App installed on the owner whose installation reaches the repository, whose `AppRecord` subscribes and has a `webhook`, and whose installation has read on the event's permission (`workflow_job`: `actions`). An App's webhook rides the owner's `HookSequencer` as hook `-app`, with `installation` in its body, its own secret, and target type `integration`; it gets one attempt, as on GitHub. Off until a site day sets `GRAFTYARD_APP_HOOKS` once `FormatsEnable` enabled 32.24. SHARED states it in "Event fan-out" (App deliveries) and "Multi-chain operations" (Workflow run); DESIGN in Shape, `ActionsEvent` and `HookSequencer`.

**Why.** R24: GHES sends an installed App `workflow_job`, and `graft-runners` is prompted by exactly that delivery (SHARED, "Actions", Hosted runners), but no consumer rendered it and no delivery reached an App's `webhook`, so BF-60 ran on the 5-minute sweep alone. The steps must come from the entries: a job with `needs` is `Pending` when `RunCreated` builds its check run, and becomes `Queued` through a `NeedsMet` that writes no check-run change, so a reading of the fold, or of `CheckRun` versions, never sends its `queued`, the delivery `graft-runners` starts a runner on [inference: the same gap ARC documents for jobs with `needs`].

**Beat.** (a) Rendering `workflow_job` from the `ACTIONS_APP` `CheckRun` entries: their version moves only on Pending to InProgress to Completed, so it misses a `needs` job's `queued`, and the check row has no labels, runner, run attempt, workflow, or branch, which GHES 3.22's `workflow_job` schema requires. (b) A separate App-delivery store per App owner: an App installed on many owners would need one sequencer reading every owner's chains; riding each owner's store keeps order per (chain, App) with the queues, slots, and cursor halves the hooks already have. (c) Sending App deliveries with retries: GitHub gives an App one attempt, and the hosted-runner sweep already covers a lost one.

**Evidence.** `an_installed_app_gets_each_jobs_workflow_job_steps_once_in_order` (`crates/actions/tests/gates.rs`): a run whose `test` needs `build`, with `RunOwner` killed once between its repository writes and its cursor, gives the subscribing App each job's three steps once and in order, with stable GUIDs, and nothing to an App without `actions` read or installed only on another repository; on main it does not build, as no `workflow_job` path exists. `an_apps_workflow_job_names_its_installation_and_the_app` (`crates/api/tests/webhooks.rs`). The three payloads and an App's body validate against `ghes-3.22.json` (`crates/api/gh/validate.py`). `tests/m4/wp4/plant.py` 31 to 39.

**Flip.** A hosted run where an App's `workflow_job` deliveries lag the sweep, or their volume crowds the owner's repository hooks out of `HOOK_OWNER_CONCURRENCY`, which would argue for (b); or an App client that needs retries, which would argue for (c).

### An installing parent member drops its own metadata tier, and its install names the slimmed snapshot (2026-11-16)

**Decision.** Three rules for a split parent member that falls past the leader's archive horizon (`design/DESIGN.md`, Shape, `CHILD_GROUP_AT`, Installs):
- (i-1) The parent's metadata tier is due a snapshot the moment its slim pass ends (`graftyard_git::split::slim_snapshot_due`), so the newest metadata snapshot at or below an install's `through` is the slimmed tier, not the whole one from before `MetaDropped`.
- (i-2) An install whose newer source names the same core and metadata snapshot keys, with archives that continue the running job's, extends that job to the new `through` and answers the newer round (`crate::load::next_job`). Any other source restarts it, and the restart is logged.
- (i-3) A parent member past `MetaDropped` whose install job is stored and has started, and whose `applied` is below the source's `through`, drops its own metadata tables before it loads (`drop_due`), one table a batch, and records `meta_dropped` in the job's own transaction. While the mark stands its tier is unloaded on every restore (`family::meta_loaded`), so no metadata, merge-rows or narrow `api` read is served (a follower names the leader and the edge moves on), `crate::snap` replays nothing into it, and the fold audit gets no metadata digest from it (`snap::tiers_json`), so it is listed as loading. It does not campaign, `keep_led` skips it, and it refuses a leader's `TimeoutNow` with `TransferRefused` (`Family::may_lead`). `Write::Install` clears the mark with the table swap. An abandoned job (isolate death, `BATCH_TRIES`, a restart for another source) leaves the mark, so the member stays unloaded until the next install finishes. An install the core no longer needs (`NotNeeded`, the member caught up by AppendEntries) swaps the loaded metadata tier in at `through`, clears the mark, and leaves the tier unloaded for `crate::snap::load_meta` to replay the stored log above it.

**Why.** On B0-30's rerun, site c's parent member fell past the leader's truncation at about 08:00Z on day 25.4. Its install named the pre-split metadata snapshot (3,249,617,556 B) and restarted at each of five truncations in 14 minutes. Once the group stalled, the one job left ran beside the member's own 5.70 GB and took it to 10,184,200,192 B, past `CHILD_GROUP_AT` (`tests/am1/results/b030-split.md`, "Cause 1"). With (i-1) and (i-2) alone, a member that falls behind during a slim pass, which ran over 7.5 h on that rerun, still loads the whole tier beside its own. A member past the horizon can serve nothing from its own tier that the group still needs: every read it would answer must wait for its install anyway.

**Beat.**
- An install source with no metadata snapshot (the first proposal in `b030-split.md`): the slimmed tier still holds every row and column `decide` reads (labels, pulls, check runs, issue rows without their bodies), so a member installed with an empty tier would decide wrongly once it led, and its metadata digest would differ from the other members'.
- Loading the scratch tier into the member's own tables in place: an abandoned load would leave a partial tier with a loaded meta row.
- A `crates/raft` lead bar: the site's `may_lead` gate covers the three ways a member leads (its own campaign by request or `keep_led`, and a transfer). A `led_term` resume needs an earlier lead in the current term, and a member sent an install had stepped down to follow that install's leader [inference from `Member::on_install`].

**Evidence.**
- (i-1): `the_slimmed_tier_is_snapshotted_once_its_pass_ends` and `a_snapshot_at_the_pass_end_satisfies_it` (`crates/git/tests/split.rs`); main has no such rule, so its newest snapshot after the pass stays the pre-split one.
- (i-2): `a_later_source_with_the_same_snapshots_extends_the_running_install` fails on main, which restarts on every new source; `a_source_with_other_snapshots_or_a_gap_restarts_the_install` (`workers/site/src/load.rs`).
- (i-3): `only_an_install_on_a_split_parent_past_the_horizon_drops_its_tier`, `a_member_with_its_tables_dropped_is_never_loaded`, `an_unloaded_tier_answers_no_digest_for_the_audit` (`workers/site/src/load.rs`), and `a_member_that_may_not_lead_refuses_a_transfer` (`workers/site/src/member.rs`). The read gate is `family::follower_query`'s existing rule on `loaded()`, already covered by `a_follower_serves_only_the_designs_reads`.
- `tests/b030fixes/plant.py` catches every plant listed in `tests/am1/results/b030-split.md`, "b030-fixes".
- **Disk peak [measured, local workerd].** `tests/b030fixes/dropbed.js` holds a 256 MiB log and a 512 MiB metadata table in one SQLite object (918,556,672 B by `databaseSize`), then loads a 512 MiB scratch tier. Kept, the peak is 1,531,289,600 B. Dropped first, it falls to 306,192,384 B after the drop and peaks at 918,921,216 B, its own size within 0.04%. So the peak is own − metadata + scratch, with the freed pages reused.
- **Site c's case [inference from that rule and the rerun's readings].** Own 5,696,712,704 B (`b030r2-final.jsonl.xz`). Its metadata tier was the pre-slim tier the child also holds, about 3.84 GB (the child members' 3,836,465,152 B). With (i-1), the scratch tier is the slimmed snapshot. The slim cut smoke 1's parent from 8.31 to 3.04 GB, so the slimmed tier is well under 1 GB [inference]. Expected peak: 5.70 − 3.84 + under 1 ≈ under 2.9 GB, against 10.18 GB measured. Without (i-1), if a member falls behind before the slim snapshot commits (the scratch is the 3.25 GB pre-split snapshot loaded to its 3.84 GB tier), the peak is 5.70 − 3.84 + 3.84 ≈ 5.7 GB. Both are under `CHILD_GROUP_AT`. The next B0-30 run measures it hosted.

**Flip.** A hosted install that ends with a metadata digest the other members do not share. Or a parent member under the mark that answers a metadata read or leads.

### A re-placement waits for every voter its `Degraded` config needs, and a lookup places only a group it built (2026-11-16)

**Decision.**
- (ii)(a) `replace_on`, the one path `/place`, `/replace` and the daily near check share, reads `/status` from each voter `Degraded { lost: to }` would need, other than itself, before it proposes that config. It goes on only when each has applied to within `PLACE_LAG_MAX` (64, the leader's `CATCHUP_MAX`) of the leader's commit, and otherwise answers `{"error", "behind"}` and changes nothing. A voter whose status does not answer counts as behind. Only `Routes::degrade` proposes a `Degraded` config, and it takes the `Ready` value that the check alone builds.
- `graftyard_lookup::create` starts `/place` only for a group its own `/join`s built in this call: a majority of the bootstrap members answered with `x-graft-joined-fresh`, which a member sets only on the join that built its core. A lookup that found no leader for a group that already exists campaigns as before, and places nothing.

**Why.** On B0-30's rerun, a lookup in the parent's leaderless seconds at 08:14Z ran `create` on the 25-day-old group. Its background `/place` re-placed site b, whose 123 ms to the home was over `NEAR_RTT`, by committing `Degraded { lost: 1 }` at entry 208,160. That config's quorum is sites a and c, and site c was installing, so nothing committed again (`tests/am1/results/b030-split.md`, "Cause 2"). Either rule alone would have prevented that entry.

**Beat.** (ii)(b), a leader that drops its own uncommitted `Degraded` entry after 20 s: Raft lets a leader's uncommitted config go only by a new leader overwriting it, so it is a `crates/raft` change with the long sweep and Porcupine. The operator held it unbuilt unless (a) is not enough. Checking in `Member::reconfigure` from the leader's `matched`: that is also a `crates/raft` change, and `matched` lags a follower that installs.

**Evidence.**
- `a_degraded_config_waits_for_every_voter_it_needs` (`workers/site/src/member.rs`) replays site c at 199,808 against commit 208,159.
- `only_a_group_this_create_bootstrapped_is_placed` (`crates/lookup/src/lib.rs`) shows that B0-30's all-already-joined answers place nothing, and that main's rule (any majority of 200s) would place them.
- Plants in `tests/b030fixes/plant.py`.
- What `PLACE_LAG_MAX` leaves to chance: a voter that falls behind after the check and before `Degraded` commits. That window is one config round, against the hours of an install [inference].

**Flip.** A hosted `Degraded` entry that does not commit within `change`'s 20 s with every needed voter caught up when it was proposed, which would make (ii)(b) worth its raft change.

### `app-hook-config`: `PATCH /app/hook/config` on the edge sets an App's webhook and wrapped secret, 2026-11-16

**Decision.** The edge serves GHES 3.22's `GET` and `PATCH /app/hook/config` under the App's JWT. It finds the record from the JWT's `iss` (the App id, or `ClientId::for_app`'s client id) through `NameKey::App`, verifies the JWT against the record's keys, wraps a new secret through `graft-ops` `POST /wrap` as a repository hook's secret is wrapped, and commits `AuthzOp::SetAppHook { app, webhook, secret }` (op tag 38, Authz format 0x12.13) on the App owner's `Authz` chain under the operator's credential, since the JWT is the check. `decide` sets the two fields on the record the fold holds and writes the existing `AuthzEvent::App`, so a key or permission change that commits between the edge's read and the commit survives, and a repeat writes nothing. The reply is GHES's `webhook-config`: `content_type` `json`, `insecure_ssl` `"0"`, the secret masked. A body asking for `form` or `insecure_ssl` 1 is 422, because `AppRecord` holds neither choice and App deliveries are JSON over verified TLS. Before `FormatsEnable` enabled 0x12.13 both routes answer the 404 they did before. SHARED states it in "Identity and tokens".

**Why.** R24's hosted step needs the hosted-runners App to carry a webhook and a wrapped secret that matches `graft-runners`' `WEBHOOK_SECRET`, and the install had no route that writes them: `app.mjs` sets the record through the operator's `PutApp`, which cannot wrap. GHES's own endpoint is what an App's owner uses, so any App gets it, and it reuses the one wrap path repository hooks already use, so no secret reaches a chain or a log in the clear (lead, 2026-11-16).

**Beat.** (a) A tools-only path that wraps a secret for the operator: a second wrap route outside GHES, which no App client could use (lead, 2026-11-16). (b) `PutApp` with the whole record from the edge's read: a key rotation that commits in between would be undone. (c) Storing `content_type` and `insecure_ssl` on `AppRecord`: a new stored field for choices GitHub's App settings do not offer [inference].

**Evidence.** `set_app_hook_patches_the_current_record_on_every_member` (`crates/identity/tests/wp4.rs`): on three real members, a hook set after a key rotation keeps both keys; a repeat writes no event; clearing restores the record; an unknown App is `NoApp`; a non-admin is refused; and `claimed_app` reads the App from a numeric, string, or client-id `iss`. `an_apps_hook_config_renders_ghes_webhook_config_and_a_patch_keeps_what_it_omits` (`crates/api/tests/webhooks.rs`): the reply, the masked secret, the body rules, and that the record's bytes decode to the `Wrapped`. Neither builds on main, which has no `SetAppHook`. The `GET` and `PATCH` replies validate against `ghes-3.22.json` (`crates/api/gh/validate.py`). `tests/m4/wp4/plant.py` 40 to 48.

**Flip.** An App client that sets `form` or `insecure_ssl` 1 and needs it honored, which would argue for (c); or a GHES 3.22 reply shape for the App's `webhook-config` that differs from the repository hook's `config`.

### `roll-up`: a bounded roll of a chain's oldest promoted packs, recorded as `Compacted` on the repository chain, 2026-11-16

**Status.** Approved by the operator after round 32's review, with the repository-chain form; built on branch `rollup` (G9), not deployed. Built: the fold and `verify`, the archive alarm's roll step (`log_archive::roll_plan`, `Step::Rolled`), the site's warm `/roll` through the promotion path, the container's `/roll`, the rehydrate's read of produced packs with its fallback to the consumed packs, the site variable `GRAFTYARD_ROLLUP` that gates every roll (`log_archive::rollup_enabled`: unset, the alarm sends no `/roll` and records no `Compacted`; a site day sets it to `1` only after `FormatsEnable` enabled 32.25, as R24's `GRAFTYARD_APP_HOOKS` waits for 32.24), and the deletion guard as a pure rule (`sweep::consumed_deletable`). Not built: the sweeper's deletion of consumed packs, which waits for that rule's shard answers (G9's remainder). DESIGN, Shape, `Promoted` and `rehydrate`, and Failure modes (pack buckets bad-deploy, git caches noisy-tenant) carry the rule; this entry carries the numbers.

**Decision.**
- **What rolls.** On one repository chain, the oldest run of promoted packs that no roll has consumed, in index order and consecutive (each `Promoted.from` is the last one's `to + 1`), stopping before the run's packs pass ROLL_IN_MAX bytes in, 2 GiB [guess, from the memory bound below]. A roll runs once that run reaches ROLL_IN_MAX, or once it holds ROLL_MIN_PACKS (8) packs and is older than a day [guess]. A rolled pack is never rolled again, so no roll's input grows past ROLL_IN_MAX and there is no second level. The roll is one complete pack of every object of its inputs.
- **Who writes it.** The repository group's archive alarm (`log_archive::step`), after promotion and archiving have nothing due, as a fourth step: it asks the chain's home shard, as a warm call (`promote-warm`: it starts no stopped shard), for `/roll` with the input packs' ids and checksums. The shard runs `pack-objects --stdin-packs --delta-base-offset` over exactly those packs, at nice 19, `pack.threads=2`, `pack.windowMemory=256m`, holding the repack's slot so a roll and a repack never run together. It streams the pack and its `.idx` back through the same path as a promotion (`PackStore::place_promoted`, a `/validate` on the same shard), and the leader commits `RepoEvent::Compacted { consumed, produced, from, to }` (`RepoEvent` tag 47, repository format 32.25; tag 46 and 32.24 are R24's `WorkflowJob`, M4-WP4) in an entry of its own, with a request id from the inputs' digests, so a re-sent step after a lost answer records nothing twice. The payload is SHARED's `NetworkEvent::Compacted { consumed, produced }` plus the index range the rolled promotions cover.
- **`decide`.** It accepts a `Compacted` only when every consumed pack is a committed `Promoted` pack of this chain not consumed before, the consumed ranges are consecutive and equal to `from..=to`, and `produced` is one stored pack. The fold keeps the list of (from, to, produced) rolls beside `promoted`.
- **The memory bound.** A roll's RSS is about 1.7 times its input bytes and its private memory about 0.6 times (table below). At ROLL_IN_MAX 2 GiB that is about 3.6 GB RSS, 1.3 GB private, beside a container's 12 GiB with the repack's 1.7 GB (`repack-keep-pack`) and clones, and it never runs beside the repack. A whole-network roll of 6.8 GB failed on this orb at 10.4 GB maxrss (killed), as round 23b's 8.6 GB said it would.
- **Rehydrate prefers a rolled pack.** `rehydrate_plan` drops every pack a committed `Compacted` consumed (`ChainLog::compactions` reads each chain's rolls from the `ranges` read), loads `produced` by its stored `.idx` as a promotion is loaded, and `skippable` treats a roll's `from..=to` as a promotion's range, so the pushed packs below it stay skipped. A roll whose produced pack has no `Committed` marker does not count: the shard loads its consumed packs instead, while they exist (below). Each counted roll's produced pack loads first, alone, oldest range first; one that does not load (no bucket copy matches its checksum, the container refuses it, or its bases are missing) is dropped for that run and reported (`Rehydrated::dropped_rolls`), and its consumed packs load in its place, which holds because nothing deletes them yet. A bad `.idx` alone falls back to `index-pack`, as a promotion's does. Tests: `a_roll_whose_produced_pack_does_not_load_falls_back_to_the_packs_it_consumed` fails without the fallback; `two_rolls_load_oldest_first_and_both_count`; `an_unset_rollup_variable_rolls_nothing`.
- **Deleting consumed packs.** A consumed pack and its `.idx` are deleted, then its `Committed` marker, by the sweeper only once all of: the `Compacted` is CONSUMED_KEEP (7 days) old; every shard holding the network (each warm site's first shard and its held pool slots) answers a warm `/status` with a cursor at or past the `Compacted`'s index, or holds no disk for it; and the network is not in a restore window that reads below the roll. A shard that cannot answer holds the deletion (`sweep::consumed_deletable`; the sweeper does not call it yet). Until deleted, consumed packs keep their markers, so the orphan rule never removes them early, and the filler skips them.

**Why.** On B0-34's network the cold warm reads 8.64 GB of stored packs at 104 to 132 MB/s, which with the platform's start (33 to 36 s) and the tail plus `/connected` (32 to 47 s) gives 150 to 158 s against DESIGN's 120 s (round 26). Under 120 s needs the stored packs read in about 39 to 54 s, so 4.1 to 7.1 GB at that rate (round 27b). Rolling promotions shrinks the bytes: 6-step promotions of B0-34's plan rolled 8 at a time come to 0.52 of their input, 16 at a time 0.42 (round 32). A re-promotion cannot shrink committed ranges (`check_promoted`), and more streams do not raise the total past the container's R2 rate (round 27b).

**Measured on B0-34's plan** (`tests/fanout/rollup/`, round 27's fixture: 33 complete 6-step promotions and the base, 6.80 GB; `pack-objects --stdin-packs` at nice 19 on 2 threads, `windowMemory=256m`, on 4 CPUs):

| promotions rolled | in | out | out/in | wall | CPU | maxrss | private |
|---|---|---|---|---|---|---|---|
| 2 | 0.42 GB | 0.31 GB | 0.73 | 24 s | 31 s | 0.84 GB | 0.40 GB |
| 4 | 0.80 GB | 0.51 GB | 0.64 | 40 s | 58 s | 1.47 GB | 0.64 GB |
| 8 | 1.68 GB | 0.87 GB | 0.52 | 84 s | 124 s | 2.82 GB | 1.09 GB |
| 16 | 3.52 GB | 1.48 GB | 0.42 | 176 s | 263 s | 5.72 GB | 2.11 GB |
| 34 (all) | 6.80 GB | killed | | 302 s | 481 s | 10.4 GB | 3.5 GB |
| 8 + 8, then the two rolled again | 1.62 GB | 1.50 GB | 0.92 | 99 s | 117 s | 3.71 GB | |

- The second-level row is why a rolled pack is never rolled again: rolling two 8-rolls gives 1.50 GB, about what one 16-roll gives (1.48 GB), for 99 s and 3.7 GB more.
- A roll's validate (`index-pack --threads=2` at nice 19, the 0.87 GB 8-roll) took 93 s, 162 CPU-s, 1.37 GB maxrss. So one roll of 8 costs about 3 minutes and 290 CPU-s on the home shard, against about 75 promotions' input.
- The roll kept every object: the 4-roll's index holds the same 1,540,334 objects as its inputs, and `index-pack --verify` passes.

**B0-34's cold warm with roll-up [inference from the table and round 26's phases].** repo:131296's 61 promotions (8.64 GB) rolled at ROLL_IN_MAX 2 GiB are about 8 rolls of about 8 promotions each. The two 8-rolls measured came to 0.52 and 0.41 of their input, so about 3.5 to 4.5 GB. At 104 to 132 MB/s that reads in 27 to 43 s, inside the 39 to 54 s window. Total about 100 to 127 s at 0.52, 93 to 120 s at 0.41, against 120 s. At 16 per roll (0.42, about 3.6 GB, 28 to 35 s) it would be about 94 to 118 s, but a 16-roll's 5.7 GB RSS is past the bound above. So with ROLL_IN_MAX 2 GiB the warm reaches 120 s at the R2 rate's upper half, not reliably at its lower half. Round 23b's 1.04-times-pushes figure was a whole-network `repack -a -d`; bounded rolls do not reach it. Round 34's probe (m39, standard-2) put the R2 limit per container, not per site or bucket: a second container on the same site and packs read 52.5 MB/s beside the first's 74.8, against the first's 31.2 alone in the same bins; m39's own totals (36 to 56 MB/s) were held by standard-2's one vCPU, so they do not change this estimate, which stays on standard-4's 104 to 132 MB/s: 93 to 126 s [inference].

**Beat.**
- **A whole-network roll** (`repack -a -d`): 0.32 of the input, but 8.6 to 10.4 GB resident and 385 s or more, past a 12 GiB container beside clones.
- **A geometric roll with rolled packs re-rolled:** the second level saved 8 % of its input for 99 s and 3.7 GB (table).
- **SHARED's network-chain `Compacted`:** packs, promotions and markers live on repository chains (`Promoted`, `ChainLog::last_promoted`), and no network group exists in the code.
- **Deleting consumed packs at once:** a shard mid-rehydrate, or one behind the `Compacted`, would lose packs it is about to load.

**Evidence.** `tests/am1/results/lm1-memory.md`, round 32; `tests/fanout/rollup/{rollk,level2,validate}.sh`; `tests/fanout/raw/rollup-r32.txt`.

**Flip.** A hosted B0-34 cold warm over 120 s after the roll has run, which makes AHEAD (`FETCH_AT_ONCE`) or the `/spool` early headers the next lever; a hosted roll whose RSS is past 2 times its input, which lowers ROLL_IN_MAX; or a network chain built for GC, which would move `Compacted` there with one record for both.

**The contradiction, settled.** SHARED defined `Compacted` as a `NetworkEvent` on the network chain, and DESIGN's rehydrate read it per network. The operator chose the repository-chain form (round 32's review): DESIGN wins over SHARED, the promotions, markers, and `ChainLog::last_promoted` already live on the repository chain, and no network chain exists in the code. SHARED's "Compaction never drops an object" now says the base records it on each repository chain beside `Promoted`.

**GC's pack-set guard, per chain.** SHARED's guard refuses a push that names objects it does not send, and a `RestoredTo`, when a `Compacted` committed after the pack-set index their walk read. With one `Compacted` per repository chain, the guard holds per chain, for two reasons:
- **A roll drops no object.** Its pack is `pack-objects --stdin-packs` over exactly the consumed packs, so it holds every object they held. The 4-roll kept all 1,540,334 objects, and the produced pack is validated before `Compacted` commits. A walk that read the consumed packs finds every object again in the produced one, so a roll can never make a concurrent push's walk wrong. `decide` still refuses such a push or `RestoredTo` only for a `Compacted` on its own chain, which is the chain whose packs it walked.
- **A fork's chain rolls only its own promotions.** `decide` accepts a `Compacted` only over committed `Promoted` packs of the same chain, so one chain's roll never consumes a pack another chain's walk or promotion named.

What stays open is GC itself, which will drop objects. GC must not drop an object a consumed pack held until the produced pack is loaded on every shard of the network, because a shard still behind the `Compacted` serves from the consumed packs. The sweeper's deletion rule above already waits for every shard, but GC is not built, and the rule that ties its `Compacted` to that wait is open as G9's remainder. No roll in this entry deletes an object.

**Gate risk.** B0-34's 120 s cold warm is reached only at the upper half of the R2 rate: 93 to 127 s by the inference above, against 120 s. This entry does not claim the gate. The flip below is a hosted cold warm over 120 s after the roll has run, and the next lever then is AHEAD (`FETCH_AT_ONCE`, four stored packs at once, `fetch-ahead`) or the `/spool` early headers (G9's row, `lm1-memory` round 31).
