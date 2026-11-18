# Graftyard glossary

The terms the design and build plan use. Each row names the section that defines the term. "Shape" means `design/DESIGN.md`, "Shape", unless another document is named. Code-font names are Rust types or constants in that section.

| Term | Meaning | Defined in |
| --- | --- | --- |
| chain | One gap-free log of entries for one aggregate: a repository, a fork network, an owner's `Authz` or `Identity`, a token or name shard, an org's Actions state, `Ids`, or `Release`. | `design/SHARED.md`, "Read this first"; Shape (`ChainId`) |
| site | One Worker script whose Durable Objects, containers, and pack bucket are pinned to one region, and the unit of failure. | `design/DESIGN.md`, "Summary"; Shape |
| site script | The Worker script (`graft-site-a`, `graft-site-b`, `graft-site-c`) that defines a site's member, git cache, and container classes and owns its pack bucket, with no public route. | Shape |
| spare site | A site pre-created on each continent with no warm containers and only a canary group, bound from install so `ReplaceMember` onto it creates no script version. | Shape |
| group | The Raft group that holds one chain: one voting member on each of three sites, plus up to 16 learners. | `design/DESIGN.md`, "Summary"; Shape |
| member | One SQLite-backed Durable Object in a group, holding the Raft log, lease state, folds, and consumer cursors. | Shape (`Member`); `design/DESIGN.md`, "Stack" |
| learner | A member that stores content, never votes, and serves read-index reads, added when a group passes 500 reads a second (`LEARNER_AT`). | Shape (`MemberKind`) |
| witness | A voting member that stores only (term, index, digest) and never content, used by residency placements. | Shape (`MemberKind`) |
| entry | One `LogEntry` in a group's log, keyed by index and chained to the previous entry's digest, and every acknowledged change is one entry. | Shape; `design/SHARED.md`, "Chain events and commands" |
| group commit | The leader puts every command that arrives during a round into the next entry, up to 1.5 MB, so one entry in flight carries many commands. | Shape (the write path) |
| `decide` | The pure method on a `Command` that turns the fold, plus the earlier commands of the same entry, into events or a refusal. | Shape (the write path) |
| `verify` | The check a follower runs on entry k+1 after applying k, which refuses an entry that does not decode, does not chain, or fails its replayed `expect`s. | Shape (the write path) |
| `Acked` | The acknowledgment proof built only inside `outcome` from the acks the leader counted, which every 2xx renderer takes by reference. | Shape (the write path) |
| fold | A SQLite database derived from one chain's entries that serves every list, filter, and sort. | `design/SHARED.md`, "Folds" |
| core tier | The fold tier that git, merges, and authorization read: refs, rules, collaborators, pull request heads, verdicts, required checks, settings, and the agent rows `decide` checks. | `design/SHARED.md`, "Folds" |
| metadata tier | The fold tier for issues, comments, review bodies, labels, and the timeline, loaded lazily, which no core request waits for. | `design/SHARED.md`, "Folds" |
| `fresh` | A read that includes every entry acknowledged before the request arrived, built only by the leader after its term's `Noop` applies. | Shape (the read path) |
| read-index | The round that bounds a read: a leader near its lease end runs one to a follower, and a follower or learner asks the leader for its commit index and waits until applied. | Shape (the read path) |
| leader lease | `LEASE`, 9 s, during which a leader answers `fresh` reads alone, because followers promise no vote to a rival until it lapses. | Shape (`LEASE`, `MemberState`) |
| `DRIFT` | 500 ms subtracted from the leader's own lease end for clock drift [guess], which is also the skew bound on agent leases. | Shape |
| transfer | Raft leadership transfer: the leader stops appending, brings the target to its last index, and sends `TimeoutNow`. | Shape (Leadership) |
| `suspect` | A request-driven report from the edge to a follower when a leader misses `CALL_DEADLINE`, and reports from two colos in 10 s stop the follower's promises. | Shape (Leadership) |
| drain | `SiteMode { Drain }`, under which a site's members hand off leadership within one lease and keep voting and storing. | Shape (`Release` group) |
| site day | The only way a new site-script version ships: drain the site, deploy at 1%, 10%, 50%, then 100%, then undrain, one site a day. | `design/DESIGN.md`, "Install operations" |
| `Release` group | One group per install, cell, and front account, placed on `Tri`, that holds signed entries: genesis, `Keys`, intents, site modes, pointers, and restores. | Shape (`Release` group) |
| intent | A signed `Release` entry that tells `graft-ops` to run one operation, such as `SiteDay` or `EdgeRoll`, with a pre-signed undo and a `not_after` expiry. | Shape (`SignedIntent`) |
| pointer | A pin in the `Release` group's `Pointers` fold for the edge, `graft-ops`, `graft-mcp`, or the controller. `None` lets the deployment split route calls. Only a pre-signed undo (`EdgePointer`, `OpsPointer`, `McpPointer`, or `ControllerPointer`) pins a script to a version, which is swap-back with no Cloudflare API call, and the next intent that rolls that script clears it. | Shape (`Release` group, `Pointers`); `design/DESIGN.md`, "Stack" |
| `graft-front` | The stable Worker every request enters. It forwards each request to the edge on the deployment's split, or to the version an undo pinned in `Pointers`. | `design/DESIGN.md`, "Stack" and "Commit and read paths" |
| edge | The Worker that serves HTTPS git, REST, GraphQL, and web assets and calls group members through Durable Object bindings. | `design/DESIGN.md`, "Stack" |
| `graft-ops` | The stateless in-account Worker that executes signed intents, owns the crons that call the Cloudflare API, and mints installation tokens. | `design/DESIGN.md`, "Install operations" |
| git cache shard | `GitCacheShard { site, shard }`, a container-backed Durable Object that runs upstream git on bare repositories rebuilt from R2 and holds nothing canonical. | Shape (git cache shards) |
| rendezvous | Rendezvous hashing of (network, shard) within a site, which maps networks to shards so the shard count never changes and no other network moves. | Shape (git cache shards) |
| hot pool | `HOT_POOL`, 7 instances per site counted in the site's shard start cap, from which `fanout` adds shards for a hot network. | Shape (git cache shards) |
| clone cache | R2 objects keyed by `CloneKey` that hold protocol v2 fetch responses, which are the same bytes for a fixed ref state, served from R2 through the edge on a hit. | Shape (Packs, dominant access patterns) |
| `DurablePack` | A pack that `PackStore::place` wrote to two sites' buckets in two regions after `index-pack --strict` passed, and the only pack an entry may name. | Shape (Packs) |
| inline pack | `PackRef::Inline`, a pack of up to 1 MiB (256 KiB on a hot group while placement succeeds) carried inside the entry. | Shape (Packs) |
| vault | A separate account that none of the install's tokens can reach, which pulls the change feed and packs into R2 under 35-day bucket locks, required in every mode. | `design/DESIGN.md`, "Install operations" |
| lifeboat | A read-only copy on two workerd hosts on the customer's network that serves clones and reads to enrolled developers during a Cloudflare-wide outage. | `design/DESIGN.md`, "Install operations" |
| `Tri` | The default placement, with Full voters on enam, weur, and apac, which survives a region, a continent, and a site deploy. | Shape (`PlacementName`) |
| `Solo` | A one-member placement for evaluation only, behind the signed `evaluation` flag, with no region-loss guarantee and no availability SLO. | Shape (`PlacementName`) |
| cell | In managed mode, one Cloudflare account with three site scripts that holds the groups of the owners placed in it. | Shape (managed cells); `design/DESIGN.md`, "Deployment modes" |
| front account | The managed account that holds the zone, `graft-front`, the edge, and the global `Names` and `Tokens` groups, and calls each owner's cell over signed HTTPS. | Shape (managed cells) |
| Milestone 0 | About six weeks of measurements on one throwaway account before any git code, where any fail value sends the design back. | `design/DESIGN.md`, "Build order from zero" |
| admission class | `AdmitClass` (Human, MergeQueue, Bot, Agent), which the leader assigns to each command in group commit, with Agent capped at `AGENT_SHARE`. | Shape (agent block) |
| `ActorKind::Agent` | The actor kind of Graftyard's own agent Apps and of admin-flagged Apps with `confine` set, whose approvals, merges, and out-of-prefix pushes `decide` refuses. | Shape (agent block) |
| `AgentTask` | The task row on a repository's group, public under `/api/graftyard/v1/repos/{owner}/{repo}/tasks` with GitHub's agent task states. | Shape (agent block); `design/AGENT-LAYER.md`, "Shape" |
| lease | A Shared or Exclusive `Lease` by one holder on paths or refs of one repository, kept as core-tier rows on its group and live while an entry's `at` is before `expires`. Not the leader lease. | Shape (agent block); `design/AGENT-LAYER.md`, "Shape" |
| activity feed | A long-polled view of a group's log under `/api/graftyard/v1/repos/{owner}/{repo}/activity`, served by learners first, by followers only while no learner answers, never by the leader, and at most `FEED_HELD_MAX` held polls per member. | `design/AGENT-LAYER.md`, "Shape" |
| `AgentGate` | A head-bound approval by a human writer, which `Merge` requires for a pull request with `AgentMeta`. | Shape (agent block) |

