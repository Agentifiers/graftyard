# Graftyard

Graftyard is a code host that unmodified GitHub clients treat as GitHub Enterprise Server (GHES) 3.22, built only on Cloudflare products. Every repository is a Raft group of three SQLite-backed Durable Objects on three sites in three regions, and a push is acknowledged only after two sites in two regions hold it. It ships as one signed artifact in three modes, managed, BYOC, and self-hosted, and AI coding agents use it as GHES clients with an agent layer on top.

## Status

The design is the specification that implementing agents build from, and the build is under way (`build/BUILD-PLAN.md`, the "Milestone 0 status" to "Milestone 3 status" tables). Milestone 0 is closed, with its between rows carried to named reruns. Milestone 1 is closed: BM-1's 24-hour linearizability row, which `lin24` did not pass, passed on its rerun `lin24b`. Milestone 2 is closed: its gate rows pass on the hosted install with real gh, git, git-lfs, and release clients, except two fault clauses the install cannot inject, which pass in memory (`tests/m2/results/`), and BM-2, as the operator reworded it, passed with a merge p99 of 1.36 s for a home-site leader and 1.83 s off home. M3-WP5's hosted half passes: BF-28, BF-29, and the B0-18 rerun, 6 of 6 after a call-depth fix; M3-WP1 to M3-WP4 (Apps, names, tokens, authorization, sign-in, and stateless installation tokens) run hosted with real gh, git, and `@octokit/app`, and BM-3, Milestone 3's gate, passes: 200 of 200 revoked tokens refused on the next write, 100 of them stateless. Milestone 3 is closed: every gate row is Done (BF-27's 1,000-queued count and BF-32's no-secret dump in memory), the last being Terraform's `TestAccGithubRepositoryCollaborator` as written, 6 of 6 runs, once `m3-repos` built repository creation over REST (BUILD-PLAN, "Milestone 3 status"). AM1's pure halves pass in memory, and its hosted half ran on the test install, but its gate LM-1 does not pass yet: its two hosted failures (a stall lost across a leader change, and commit p99 in held-poll bursts) are fixed by `am1-fix`, and its hosted runs on `graft-m2b` pass the refusal, lease-limit, provenance, and trust-view rows but fail LF-13 and the 500-agent load rows: `fanout` removed the git shard's validate lock, the next limiter was the read gate's uncached installation-token reads, which `04487ca` caches, and the load rows are being rerun (`tests/m3/results/`, `tests/am1/results/`, `crates/identity`). Numbers that no run has measured stay tagged [guess], [inference], or [unverified].

## What has been tested

Milestones 0 and 1 ran on real Cloudflare and in the Raft simulator. `tests/RESULTS.md` lists every run with its faults, checks, and source, and defines each number below.

- Porcupine checked 12,304,207 operations for linearizability over 104.6 hours of chaos on Cloudflare, under 16,258 member resets and 10,680 leadership transfer attempts [measured]. The 4-hour run was Illegal and found a real bug, since fixed. The 48-hour run passes with a 100 ms clock allowance. The 24-hour M1 rerun, `lin24`, did not pass because it could not rule out an early ack. Its rerun `lin24b` passes, with every acknowledged write checked against its member's commit time.
- 0 of 3,588,424 acknowledged writes were lost in the three runs whose members' logs were audited [measured].
- 12,000 leader kills between quorum and reply each returned the original outcome (BF-12) [measured].
- 31,482 SSH git sessions ran, most of them under container kills, and 31,362 succeeded. The 24-hour run's 99.587% is between the pass and fail lines [measured].
- With a site's members failing, git's own `t/` HTTP tests lost no baseline pass except t5549 #2. `clients.sh` passed 29 of 29 rows in all 12 runs on git 2.39.5, 2.51.0, and 2.56.0, 6 of them under a site fault [measured].
- The simulator's long sweep ran 40,191,804 steps. Porcupine found both halves' histories linearizable, and all 1,000,856 acknowledged writes carried valid ack evidence [measured].
- The workspace has 259 Rust tests and 14 Go tests, and its suites caught 84 of 85 planted bugs [measured].

## Reading order

Read these in order. Each line says why.

1. `FRAME.md`: what was asked, the goals in priority order, the constraints, and every operator decision with its verbatim quote.
2. `design/DESIGN.md`: the design of record. Sites, Raft groups, the commit and read paths, failure modes, deployment modes, cost, and the build order from Milestone 0.
3. `design/SHARED.md`: the platform-independent specification the base reuses. Wire compatibility, event types, tokens, issues and pull requests, folds, Actions, and retention.
4. `design/AGENT-LAYER.md`: the agent layer. Agent Apps, the MCP server, the controller, leases, the activity feed, and the task API.
5. `build/BUILD-PLAN.md`: the work packages, their order, and the gate that decides when each is done.
   `tests/RESULTS.md` lists every test run so far, with its faults, checks, and source, and defines each total in "What has been tested".
6. `decisions/DECISIONS.md`: why each major choice was made, what it beat, and what would reverse it. Read it before you change a foundation.
7. `decisions/OPEN-QUESTIONS.md`: what is still unanswered, and which measurement or answer settles each question.
8. `alternatives/AWS.md`: the AWS design that lost, priced beside this one, with the conditions under which it would win.
9. `research/`: the evidence behind the design's citations. Read a file when a claim you depend on cites it.
10. `history/`: the verbatim record of the design run under the project's old name. Implementers should not need it, and nothing in it is a requirement.

## Folder map

```
README.md                  this index
AGENTS.md                  instructions for agents implementing Graftyard
FRAME.md                   request, goals, constraints, operator decisions
design/
  DESIGN.md                base design of record (Cloudflare)
  SHARED.md                platform-independent specification the base reuses
  AGENT-LAYER.md           the agent layer
build/
  BUILD-PLAN.md            work packages and gates, Milestone 0 first
decisions/
  DECISIONS.md             decision record
  OPEN-QUESTIONS.md        open questions and what settles each
alternatives/
  AWS.md                   the AWS alternative, summarized
research/
  wire-census.md           GitHub wire surfaces and cross-cutting conventions
  clients.md               how each client targets GHES
  actions-protocol.md      what an unmodified runner and toolkit need from a server
  agents.md                what AI coding agents need from a git host
  cloudflare.md            Cloudflare products: status, limits, prices, sources
  cloudflare-uptime.md     Cloudflare incidents, SLAs, and failure domains
  cloudflare-git.md        a git host on Cloudflare only: limits and prior art
  artifacts-recheck.md     Cloudflare Artifacts rechecked on 2026-10-26
  byoc-grounding.md        what the vendor's existing BYOC platform provides
  uptime.md                AWS mechanisms and outages, GitHub incidents
  git-storage.md           how large git hosts store repositories
  dynamodb.md              DynamoDB facts (AWS alternative only)
  pricing.md               AWS and GitHub list prices
  costs.md                 generated cost tables; never edit by hand
  prices.json              price inputs for every cost script
  surfaces.json            wire-surface counts written by census.py
scripts/
  check_design.py          structure, shared agent block, and cost figures of both design documents
  check_names.py           the old project name is absent outside history/
  check_links.py           every cited repository path exists
  cost.py                  monthly Cloudflare cost of the base
  cost_agents.py           added monthly cost of the agent layer
  cost_model.py            AWS cost model; holds the shared workload (TIERS, ACTIVITY)
  costs_report.py          regenerates research/costs.md
  census.py                counts GitHub wire surfaces into research/surfaces.json
  results_lin.py           prints a tests/RESULTS.md row from a lin run's report
history/                   the verbatim design record; see history/README.md
```

## Glossary

Each row names the section that defines the term. "Shape" means `design/DESIGN.md`, "Shape", unless another document is named. Code-font names are Rust types or constants in that section.

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

## Checks and cost scripts

Run every script from the folder root with `python3`. Each exits 0 on success.

| Command | What it checks or computes | What a pass proves |
| --- | --- | --- |
| `python3 scripts/check_design.py` | Base and layer headings and line caps, the Stack table against `research/cloudflare.md`, failure-mode and deployment rows per component, costs at three sizes, and a Milestone 0. It also checks that the agent block in "Shape" is byte-identical in `design/DESIGN.md` and `design/AGENT-LAYER.md`, and that the Cost sections print what the cost scripts print | Structure and consistency only, not design quality |
| `python3 scripts/check_names.py` | Every text file outside `history/` for the old project name | Only the one allowed line in `FRAME.md` names it |
| `python3 scripts/check_links.py` | Every repository path cited outside `history/` | Every cited path exists |
| `python3 scripts/cost.py` | Monthly Cloudflare list cost of the base at 50, 300, and 2,000 developers for each variant, shard counts, and the egress split. `--detail large design` prints line items | The arithmetic over guessed constants. Milestone 0 replaces the guesses |
| `python3 scripts/cost_agents.py` | The agent layer's added monthly cost on top of `cost.py`. `--a1` adds the per-object rates of journey A1, the 500-agent fleet on one monorepo | As above |
| `python3 scripts/cost_model.py` | The AWS cost model. It also holds `TIERS` and `ACTIVITY`, the workload every cost script uses | As above |
| `python3 scripts/costs_report.py` | Rewrites `research/costs.md` from the cost scripts | The cost reference matches the scripts |
| `python3 scripts/census.py` | Counts GitHub wire surfaces and writes `research/surfaces.json`. The first run downloads and pins sources, and later runs are offline | The surface counts in `research/wire-census.md` can be regenerated |

After any design edit, run `check_design.py`, `check_names.py`, and `check_links.py`.

## Headline costs

Monthly Cloudflare list prices for one install of the default shape (one account, three sites, `Tri`), from `python3 scripts/cost.py` and `python3 scripts/cost_agents.py`. They match `research/costs.md` and `design/DESIGN.md`, "Cost". Every per-operation constant is a [guess] until Milestone 0.

| Developers | Base | Agent layer added | Base plus agents | Added share |
| --- | --- | --- | --- | --- |
| 50 | $231 | $21 | $252 | 9% |
| 300 | $1,189 | $274 | $1,463 | 23% |
| 2,000 | $9,187 | $2,572 | $11,759 | 28% |

The totals exclude an Enterprise contract, which managed cells and pushes over 200 MB need and which has no list price. They also exclude Actions runner compute. Sandbox compute and model tokens for Graftyard-run agents sit outside the agent column: $2,827 and $182,400 a month at 2,000 developers, at a guessed $2 of tokens a task (`design/DESIGN.md`, "Cost"). Container memory for warm git cache shards is the largest line at every size.

## How this folder was produced

A multi-agent design run wrote this folder from 2026-10-23 to 2026-10-27. Read-only research slices wrote `research/`. Four arena rounds produced the AWS design, then a Cloudflare arena and an agent-layer arena each pitted three candidates against each other, scored by a lead and a cross-judge, and folded the strongest ideas from the losers into the winner. Adversarial reviews, install walks for BYOC, self-hosted, and managed, and a leases-and-tasks review followed, and each act-on finding was resolved in the design. A final adversarial pass by security, correctness, and consistency reviewers raised 43 findings and 19 act-on items, all applied (`decisions/DECISIONS.md`, "Final pass"). A move-only script then restructured the workspace into this layout. Three implementability walks by fresh agents, acting as implementers, then found 55 gaps across every milestone, and all were closed in the design (`decisions/DECISIONS.md`, "Implementability walk" and "Third implementability walk"). `history/README.md` explains the record, and `history/arena/` and `history/review/` hold every arena and review.
