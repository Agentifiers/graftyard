# Graftyard decisions

This file records the decisions that shape what you build, and the ones you might be tempted to reverse. Each entry gives the decision, why it was made, the alternative it beat, where the evidence is, and what would reverse it.

The full log is `history/decisions.tsv`, one row per decision with its reason, evidence, and result. A row number below is that row's line number in the file, so row 112 is line 112. The reviews, walks, and judge verdicts are under `history/review/`, and the design arenas are under `history/arena/`. The root `FRAME.md` restates every operator decision, and `history/FRAME.md` is the original with the verbatim quotes used here.

If this file disagrees with `design/DESIGN.md` or `design/AGENT-LAYER.md`, the design documents win and this file is stale. Evidence tags keep the design's meaning: [inference], [guess], and [unverified] mark claims that no measurement or document settles.

## Operator decisions

The operator wrote eight things that bind the design. Each quote below is verbatim from `history/FRAME.md`. Everything after a quote is the run's reading, and the reading is marked where the operator did not say it.

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

**Cost accepted.** The team owns a Raft implementation with leases, transfer, `suspect`, pre-vote, joint consensus, learners, and `verify`. Every `Tri` write pays one intercontinental round trip, about 80 ms [guess].

**Evidence.** `design/DESIGN.md`, "Summary", "Shape", "Tradeoffs accepted", "Alternatives considered", and "Synthesis decision". `history/arena/cloudflare/SYNTHESIS.md`. `history/review/cloudflare-judge.md`. `history/review/cloudflare-foundation-attack.md`. `history/review/cloudflare-interrogate.md`. `research/cloudflare-uptime.md`. `history/decisions.tsv` rows 110 to 112 and 116.

**Flip.** Flip condition 1: if Milestone 0's consensus gate fails, with any linearizability violation in the 48-hour Porcupine run or a `Tri` commit p99 above 400 ms, candidate 1 becomes the base after its standby-loss stall is fixed. Flip condition 2: if Cloudflare states in writing, before Milestone 1, that a Durable Object's storage followers span at least two regions, candidate 1 with its `Alone` fix and `suspect` becomes the base for single-continent (`Na3`-class) and cost-sensitive installs, and `Tri` stays the base for installs that must survive a continent [inference]. Pending question 3 above asks the operator to confirm the home-grown Raft.

### One account holds three site scripts, and members talk over Durable Object bindings

**Decision.** The default install or managed cell is one Cloudflare account with three site scripts (`graft-site-a`, `graft-site-b`, `graft-site-c`). Each site script defines its site's member, git cache, and container classes and owns one pack bucket. Members reach each other through Durable Object bindings with `script_name`. Each install pre-creates one spare site per continent, already bound, so `ReplaceMember` creates no script version. A vault account and a runner account sit beside the install account. Account-separated sites are an option.

**Why.** A binding call runs inside Cloudflare's runtime and never uses the zone's public hostnames, so a partial proxy failure like December 5, 2025 should not cut replication [inference: a written answer from Cloudflare is requested before Milestone 1]. Each group has one voter per site, so a region loss, a bad deploy of one site script, or a bad site setting removes at most one voter.

**Beat.** Account-separated sites as the default, which was candidate 3's first draft. Its members replicate over public HTTPS through site Workers, so a partial proxy failure stalls commits as well as client traffic. One account for every managed user also lost, because it puts everyone inside one account-scoped fault and meets account limits.

**Cost accepted.** An account-scoped fault reaches all three sites. The vault bounds the data loss.

**Evidence.** `design/DESIGN.md`, "Shape", "Tradeoffs accepted", and "Alternatives considered". `history/arena/cloudflare/SYNTHESIS.md`, graft 1. `history/review/cloudflare-byoc-walk.md` finding 8. `research/cloudflare-uptime.md` findings 7 and 14.

**Flip.** Milestone 0 withdraws the account-separated option if signed HTTPS through a site Worker adds more than 60 ms at p50 over a cross-script binding. The default's partial-proxy claim rests on Cloudflare's written answer about the internal route of a Durable Object call.

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

**Why.** `max_instances` defaults to 20, and a warm instance per network costs about $42 to $83 a month. Two warm copies of every shard mean a site loss or a control-plane outage needs no new container start.

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

**Flip.** Artifacts earns one role, a scratch tier for best-of-N attempts that never become pull requests, only when Milestone 0 shows forks share objects with their source and Cloudflare raises the per-repository cap above the install's largest network in writing. The tier falls back to a workspace fork on any Artifacts error. GA under an SLA is then a preference, not a condition.

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

**Decision.** Spectrum on port 22 forwards TCP through Cloudflare Tunnel to a cloudflared replica inside an SSH front container. `graft-sshd` calls the container's outbound handler, which runs in the Workers runtime with the site script's bindings. The handler resolves the key fingerprint on its `Tokens` group leader and runs the same `commit` through the same bindings, so SSH never loops back through the public zone. The operator kept SSH (answer 7).

**Why.** Workers accept no inbound TCP and containers expose no public port, so Spectrum is the only Cloudflare product that serves port 22. HTTPS pushes are capped at 100 MB on Pro and 200 MB on Business, and the edge's refusal message names SSH.

**Beat.** HTTPS only, which was the other option in pending decision 7. SSH writes that loop back through the public zone, which candidate 1's outbound-handler graft replaced.

**Cost accepted.** The chain is unverified on Pro and Business. Spectrum's `ip_firewall`, `proxy_protocol`, `tls`, `origin_dns`, and `origin_port` need Enterprise, so below Enterprise sshd never sees the client's IP and SSH abuse limits count by key and token. SSH data costs $1 per GB above the plan allowance, plus an add-on fee no list price was found for [unverified].

**Evidence.** `design/DESIGN.md`, "Shape" (SSH front), "Commit and read paths" (journey 4), "Tradeoffs accepted", "Open questions and risks", and "Milestone 0". `history/FRAME.md`, answer 7. `history/review/cloudflare-interrogate.md` CF-ACT-12. `research/cloudflare-git.md` finding 26. `history/arena/cloudflare/SYNTHESIS.md`, graft 2. `history/decisions.tsv` rows 132 and 144.

**Flip.** Milestone 0's SSH row runs on Pro, Business, and Enterprise. Under 99% successful sessions fails it. If Spectrum cannot reach a Tunnel origin on Pro or Business, SSH needs Enterprise for BYOC on that plan, or is dropped there.

### Hosted runners live in a separate runner account

**Decision.** Hosted Actions runners run as Sandboxes in a runner account, in managed and in BYOC. Runners reach the Actions broker over HTTPS and need no member binding. Git cache vCPU is reserved per site through `max_instances`.

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

**Decision.** Learners serve feed polls, and followers serve them only while no learner answers. A member holds polls only while it is following, which means its last AppendEntries left no gap and it is not catching up by `InstallSnapshot`. Each member holds at most `FEED_HELD_MAX`, 1,000 [guess], and the edge tries up to two other serving members before it answers 429. The peak held count feeds `place_learners`.

**Why.** A feed poll must never run on a member that is not answering replication. "Following" by ack age was tried first and dropped, because followers on an idle group whose leader lease lapsed would stop serving the feed.

**Beat.** Defining "following" by ack age, and followers serving polls beside learners.

**Evidence.** `history/review/walk-rest-interrogate.md` WK-Y1. `history/review/walk-rest-reports.md`, WriterY. `design/AGENT-LAYER.md`, Shape (feed block).

**Flip.** None stated. The Milestone 0 held-poll row tests `FEED_HELD_MAX` (`decisions/OPEN-QUESTIONS.md`, A19).
