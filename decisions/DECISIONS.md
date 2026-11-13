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

**Evidence.** `design/DESIGN.md`, "Stack" and "Failure modes" (`git caches`). `history/review/cloudflare-interrogate.md` CF-ACT-9.

**Flip.** None stated.

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

**Flip.** None stated. FTS5 inside Durable Object SQLite is not yet confirmed (`decisions/OPEN-QUESTIONS.md`, C15).

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

**Cost accepted.** The cap and the region check are Graftyard code, not platform limits, so a bug there could exceed the cap or misplace a container, with the account's 1,500 vCPU and the 70% alarm as the only backstop. The policy is in beta. Every container process has root's capabilities, so file permissions inside the git image protect nothing. The design never relied on them, and each container starts with `enableInternet: false`.

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

#### BM-1's git half passes with one site's member class failing

**Decision.** BM-1's git half is recorded as passing. `clients.sh` passed 29 of 29 on git 2.39.5 and 2.51.0, and `t/` on 2.51.0 and 2.56.0 lost no baseline pass except t5549 #2, with site 0's member class failing every call for the rest of each run. The 24-hour linearizability half is pending until 2026-11-02T18:27Z. A gray partial-error fault and a 2.39.5 `t/` baseline are not covered and go to BM-1's next rerun.

**Why.** The one lost pass, t5549 #2, checks how many objects a negotiated push wrote, which depends on what the server advertises; the test sets a server-side `receive.hiderefs` the host does not apply, the push itself succeeded, and the same test already differed between two no-fault runs [inference]. 2.39.5's two extra failures are fixtures the hosted slot cannot reproduce (a lock file and a server-side hook).

**Beat.** Holding the git half until a gray fault runs, which BM-1's gate text does not name.

**Evidence.** `tests/m1/results/m1-wp8.md`, "BM-1 results" and "What is short of the gate" [measured].

**Flip.** BM-1's next rerun losing a baseline pass under a gray fault, or t5549 #2 failing for a cause the host controls.

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

### `fanout`, the read gate's installation-token cache, and M4-WP1's pure half, 2026-11-10

`fanout` removed the git shard's per-network validate lock and built `fanout`; its hosted 500-agent reruns on `graft-m2b` then failed on the edge read gate. `04487ca` caches the gate's installation-token decisions, and LM-1's load rows are being rerun. M4-WP1's pure half passed BF-57 and BF-58 in memory (`tests/am1/results/fanout.md`; `tests/m4/results/m4-wp1.md`; `build/BUILD-PLAN.md`, AM1-WP1, AM1-WP6, and M4-WP1).

#### A25 closes: a shard validates one network's pushes concurrently, and `fanout` commits k as `RepoEvent::Fanout`

**Decision.** The answer to A25 is both pieces, with concurrency inside a shard first. The git shard's `/validate` takes the network's lock shared: each validate writes only its own quarantine and reads the repository through alternates, and only the steps that write the repository (`/ingest`, ref writes, repack, init, evict) take it alone, with one mutex for setting a validated pack aside. `fanout` is built as DESIGN states it, with three differences now in the design text: a warm site's first shard reports `NetworkLoad` only during minutes in which it validates pushes, so an idle network's k steps down through `LOAD_STALE` (180 s); the leader commits k as its own `RepoEvent::Fanout { k }` entry (repository format 32.15, wire tag 39), which the edge never sends; and a push validates on the shard that rendezvous on its pack's trailer picks, which catches up before it validates, with the first shard validating the push itself if that shard cannot answer. DESIGN's Cost line "At journey 9's push every 2 minutes k is also 1, with 0 extra shards ($0 a month)" stands, by git CPU, now with the lock removed; it is not yet shown hosted at 500 agents.

**Why.** The lock, not git CPU, held one network to about 2 pushes/s, so raising k alone would have paid for pool shards to work around a serialization the container did not need. A validate's writes are private to its quarantine, so sharing the lock costs no correctness, and the race tests and `git fsck --strict` show none. With the lock gone, one `standard-2` shard validates about 9 times journey 9's offered rate, which keeps k = 1 consistent with the cost model. Picking by the pack's trailer gives every re-send the same shard.

**Beat.** Raising k to spread validates across pool shards with the lock kept, which buys shards for queueing rather than CPU and would move the $0 line. A per-ref lock, which still serializes pushes to one ref's neighbours and adds lock bookkeeping that the quarantine already makes unnecessary. Picking only among caught-up shards, which needs every first shard to track each pool shard's cursor.

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

### `narrow-reads-2`: API reads start with their identity and name reads at once, cached per isolate for `READ_CACHE`, 2026-11-12

Branch `narrow-reads-2` (`tests/am1/results/narrow-reads-2.md`; F65).

**Decision.** A REST `GET` reads its token and its `o/r` at once, as GraphQL already did, and a `ghs_` token reads its verifying keys and its `jti` row at once (the `jti` is in the claims before the signature is checked; a bad signature refuses the token whatever the row says). The `o/r` resolves in the owner's `Authz` fold, which the read gate reads anyway, so the name costs no read of its own once the fold is held. A REST `GET` or GraphQL query (`Ctx::read_class`) takes the token row, the keys, the `jti` row, the login row, the `Placement` row, the owner's `Authz` fold, and accounts' logins from a per-isolate cache when it holds one at most `READ_CACHE` (10 s) old from when its read started (`workers/edge/src/rcache.rs`). The cache stores values, keeps only answers that let a request through, and an older answer never replaces a newer one. A write-class request never reads it, and refreshes it. Every API request records `server-timing` stages (`auth`, `resolve`, `tier`, `authz`, `logins`, `render`, `total`) on test installs, and one in 100 logs them as `apiTiming` on every install. Journey 9's API read target is p99 at most 2 s with at least 99% served, a new Pass clause on B0-29's row, with a Fail at p99 over 5 s.

**Why.** After `narrow-reads`, API p99 at journey 9 was 4.4 s with the edge's render at 11 to 13 ms of CPU, and an idle edge spent 1.2 to 1.4 s before the tier read: the token (about 200 ms), `o/r` through the login row, the `Placement` row, and the owner's `Authz` name read in sequence (500 to 650 ms), then the tier and `Authz` reads (300 to 650 ms) [measured, `tests/am1/results/narrow-reads.md`]. Every one of those identity reads answered the same thing for every request in a 10 s window at 500 agents, and each queued on the same system groups at 200 reads/s. DESIGN had no API latency target. GitHub's API gives up on a request at 10 s and answers a GraphQL timeout with 502 or 504 (`research/wire-census.md`, item 22), and journey 9's agents poll their PR every 5 s, so an answer later than the poll interval overlaps the next one. 2 s leaves B0-18's 1 s learner read for the member and 1 s for the edge's identity, name, and render [inference].

**Invalidation.** Only revocations wait out the cache. A revoked or lapsed token: its row is kept only while not revoked, its `expires` is checked on every use, and a revoked row is never stored, so reads honour a revocation within `READ_CACHE`, as SHARED's "Token revocation" deviation and journey 4 step 4 already state; writes see it at once. A revoked `jti`: kept only as "not revoked", so the same bound. A key rotation: a token that fails to verify on cached keys reads them again. A new repository, a rename's new name, or a transfer's new owner: a name the cached fold does not hold is resolved fresh, so it serves at once; an old name serves for up to 10 s, then resolves as an `Alias` as before. A grant, membership, or installation change that opens access: a read the cached fold would refuse reads the fold again before it answers 404. One that closes access (a removed member, a narrowed installation) may serve API reads for up to 10 s, as git's read gate already does (`crate::gate`); the repository's own access rows (visibility, collaborators) are not cached, since they ride each narrow answer. A placement move: the old record may be used for 10 s; reads find the group through the members it learned (`graftyard_lookup`), as before.

**Beat.** Folding the token and name into the repository's member read, which would make one member answer for a credential it does not own and move the identity check off its chain's leader; BM-2 ruled out passing the edge's authentication to the leader. Sharing one in-flight read across requests, which a Workers isolate refuses (F65). A longer cache, which would break SHARED's 10 s revocation bound. Caching negative answers, which would hide a new token or repository for 10 s.

**Evidence.** `workers/edge/src/rcache.rs` tests (TTL from the read's start, writes never read the cache, an older answer never wins, only passing answers are kept, a revoked row is never kept); `tests/nr2/plant.py` catches 12 of 12; `tests/nr2/stale.mjs` on the new edge: a new repository and a re-added member serve at once, a removed member stops within 1.8 to 2.3 s, a revoked PAT within 77 ms (its write at once), a revoked `ghs_` within 90 ms. Hosted B0-29 on `graft-m3c`, 500 agents, 8 learners [measured, `tests/am1/results/narrow-reads-2.md`]: `read:api` 99.8% served at p50/p99 1,515/4,865 ms before, 99.9% at 655/3,152 ms after on a run with no leader change (a first after run with a leader change mid-load: 96.8% at 749/3,303 ms). The sampled stages put `auth`, `resolve`, `authz`, and `logins` at 0 ms at p50 and the tier read at 611/2,589 ms (REST) and 629/2,163 ms (GraphQL). The gate is not met: the learners' answers set the p99 now.

**Flip.** A revocation, removed grant, or rename that a client must see on API reads in under 10 s, which would bring back fresh reads for that kind of answer. Or a hosted run where the identity stages no longer show in the p99 even uncached, which would make the cache a cost with no return.

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

### B0-30 fails at `CHILD_GROUP_AT`, and the substitution is withdrawn, 2026-11-11

B0-30 ran on `graft-m1` with pushes off from simulated day 1.34 and failed on its fail line at simulated day 27.52 (`tests/am1/results/b0-30.md`; `build/BUILD-PLAN.md`, AM1-WP6, B0-30).

**Decision.** The operator's direction of 2026-11-11: every LM-1 row passes as written, with no substitutions. The 2026-11-10 decision "B0-30 counts for LM-1 as substituted" (metadata half only, push half replaced) is withdrawn, and the LM-1 gate text in BUILD-PLAN and AGENT-LAYER no longer carries it. B0-30 is rerun as written, 30 days of journey 9 with pushes on, by thread "B0-30: MetaSplit child group, archive stall, full 30-day rerun" on branch `b030-split`, which builds the metadata child group (G6) and fixes the archive stall: a promotion's `/validate` stays on the network's first shard, nothing is forwarded to a shard with no cursor for its disk, and an archive run covers a promotion (G7). Thread "hot-shards" builds the push half, AM1-WP1's follow-up "Git cache shards on a hot group with a multi-GB log", whose gate is unchanged.

**Why.** The substitution's premise was that the metadata half answers B0-30's storage question on its own. The run disproved it: archiving stalled on the git cache shard, the same defect the push half was set aside for, and the members crossed `CHILD_GROUP_AT` with nothing to take the tier. A pass on half the row would certify neither the split nor the archive under pack churn, which are what the row exists to test.

**Beat.** Counting the metadata half as passed up to the crossing, which hides that no child group exists. Moving `CHILD_GROUP_AT` up to fit the measured growth, which the 10 GB object limit leaves no room for and which the stall, not the tier, drove. Waiting for M7-WP4 in order, which leaves LM-1 open on a row that a hot group reaches in about a month.

**Evidence.** `tests/am1/results/b0-30.md` [measured]: site 0 read 8,591,708,160 B at 23:09:41Z on 2026-11-11 against 8,589,934,592 B; `SQLITE_FULL` 0 throughout; the final verify read 81 of 81 sampled archived ranges from both buckets and resolved 10 of 10 `RestoredTo` lookups; SQLite grew 0.0585 GB a simulated day while archiving kept up, projecting about 8.21 GB at day 30 [inference, linear], and 0.184 GB a day during the stall; from about 17:13Z `fanout` at k = 6 forwarded the archive promotion's validate to cold pool shards, and site a's promotion validate median went from 0.17 s at 16Z to 7,551 s at 21Z. Cost $64.90.

**Flip.** None that restores a substitution without a new operator decision. The rerun's result decides B0-30.

### A git cache shard catches up a page at a time, in the background, and resumes where it stopped (2026-11-12)

**Decision.** A shard's catch-up and rehydrate (`crates/git/src/catchup.rs`) read the log one `/entries` page at a time, the next page read while one is ingested, so a shard holds at most two pages. Catch-up saves its cursor after each page at the last index the page held. Rehydrate saves its place per page, with its disk id, and asks the container which packs it already holds (`/have`), so a run cut off on the same disk resumes and never starts a new repository. Stored packs stream from the bucket to the container. A pack whose bases or named objects the disk lacks answers 409, drops the cursor, and the next call rehydrates. The catch-up runs inside the shard object (`workers/site/src/shard.rs`, `Jobs`), not inside its caller: a caller waits at most its `wait=<s>` and then gets 503 `warming`, the object's alarm restarts a run a reset cut off, and a pool shard with no cursor answers a forwarded push `warming` at once while the first shard validates it. The container's `/load` leaves out derived CPU (ingest, ref writes, promotion packs, init, and the archive's own `derived=1` validate). A whole-tier `{"rows": true}` read stops at `ROWS_BYTES_MAX` (8 MiB) as well as `ROWS_MAX` rows.

**Why.** On B0-30's hot group, whose log held about 2.5 GB of inline packs above the last `Promoted`, catch-up and rehydrate read whole gaps into memory, outlasted the push's 180 s deadline and the archive alarm's `RUN_FOR`, and a cut-off rehydrate restarted from nothing, so pushes were turned off at simulated day 1.34. A cursor moved to the end of a range a new leader answered short skipped an entry for good (blocker 12). `fanout` stepped k up while cold pool shards rehydrated, which sent more pushes to more cold shards (A31). A row-only budget scanned 50,000 rows of a 4 GB tier only to refuse it, at about 302 s of CPU (blocker 5).

**Beat.** Fixed pieces of `CATCH_UP_ENTRIES` entries, which bound memory by entry count, not bytes, and restart a rehydrate's tail from the last `Promoted`. One concatenated pack per page, which `index-pack` refuses on duplicate objects. Waiting inside the caller with a longer deadline, which every caller would need and an isolate reset still loses. Counting all CPU and raising `FANOUT_UP`, which still counts work that runs on every shard whatever k is.

**Evidence.** `tests/am1/results/hot-shards.md`: in-memory tests in `crates/git/tests/catchup.rs`, `crates/api/tests/surfaces.rs` (`an_oversized_tier_read_stops_at_its_byte_budget`), and `containers/git/server/load_test.go`, with the planted bugs of `tests/am1/hotshards/plant.py`; the hosted run on `graft-m3c` there.

**Flip.** A Workers runtime that lets a Durable Object run past its last request without `waitUntil` or an alarm, which would let the catch-up stay in its caller; or a hosted run where two pages of `ENTRIES_PAGE_BYTES` with a pack in flight still pass the isolate's 128 MB.

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
