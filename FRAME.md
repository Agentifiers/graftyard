# Graftyard frame

This file states what Graftyard is, what the operator asked for, the goals and constraints every design choice answers to, and every decision the operator made. Read it before `design/DESIGN.md`. The original frame, with the run plan and every quote in context, is `history/FRAME.md`.

## What Graftyard is

Graftyard is a code host that unmodified GitHub clients treat as GitHub Enterprise Server (GHES) 3.22. It runs only on Cloudflare products, and it ships in three modes: managed, bring-your-own-cloud (BYOC), and self-hosted.

Graftyard was formerly called Tributary, a placeholder codename.

Every repository, fork network, owner, token shard, and name shard is a chain of entries. Each chain is a Raft group of three SQLite-backed Durable Objects on three sites in three regions. A push is acknowledged only after its entry is on two sites in two regions and its pack bytes are in R2 buckets on two of those sites (`design/DESIGN.md`, "Summary"). AI coding agents use Graftyard as GHES clients, and an agent layer adds leases, an activity feed, a task API, and host-enforced agent rules without changing what other clients see (`design/AGENT-LAYER.md`, "Summary").

## The request and the scope

The operator first asked for an AWS-native GitHub alternative with three deployment modes: self-hosted, BYOC, and a hosted managed service. The operator ranks uptime first and wants excellence everywhere else. Wire compatibility with GitHub matters. Rust was the likely service language, React the likely web frontend, and GPUI, the Rust UI framework that Zed uses, a candidate for a native app.

The design run produced that AWS design first. On 2026-10-26 the operator asked for a version built only on Cloudflare products, and later answers made the Cloudflare design the design of record (`decisions/DECISIONS.md`, "Cloudflare is the design of record"). The AWS design is now a summarized alternative in `alternatives/AWS.md`.

The last request was to clean the notes into "a single comprehensive folder of notes, cleaned, ready for an implementation by other agents." This folder is that deliverable. It is a design. Nobody has written product code, and every cost and capacity number is a guess until Milestone 0 measures it (`design/DESIGN.md`, "Build order from zero").

The stack choices that came out of the request are these:

- Service code is Rust compiled to WebAssembly through workers-rs [inference] (`design/DESIGN.md`, "Usage (caller's view)").
- The web app is React 19 with Primer React (`design/SHARED.md`, "Frontend and native app").
- A GPUI native app does not earn a place at launch (same section).

## Goals and quality attributes in priority order

Graftyard's goals, in order, are uptime, zero lost acknowledged pushes, and scale. The operator ranked uptime first in the original request. The 2026-10-24 decision "on the store" put downtime and scalability above build convenience, and the 2026-10-26 Cloudflare request kept "less downtime first, scale second."

The quality attributes, in priority order:

1. Availability of git reads and writes, pull request review and merge, and the REST and GraphQL APIs.
2. Durability. The system never loses an acknowledged push.
3. Wire compatibility.
4. Latency of git operations and page loads.
5. Cost and operational burden in each deployment mode.

When two attributes conflict, the higher one wins. For example, every `Tri` write pays one intercontinental round trip, about 80 ms [guess], to buy RPO zero across regions (`design/DESIGN.md`, "Tradeoffs accepted").

## Constraints

### What wire compatible means

An unmodified client works after its user points it at a Graftyard host, the same way they point it at GHES today. Graftyard targets GHES 3.22. gh classifies any host that is not `github.com`, `*.ghe.com`, or `github.localhost` as GHES and derives `/api/v3/` and `/api/graphql` from it (`design/SHARED.md`, "Wire compatibility").

`research/clients.md` lists the clients that decide the question. `research/wire-census.md` and `research/actions-protocol.md` count the surfaces those clients touch. `design/SHARED.md`, "Surfaces, tiers, and oracles", gives each surface a tier and a conformance oracle, which is an unmodified real client that must pass. "Known deviations from GitHub" in the same document lists every place a client can observe a difference.

The agent layer adds no GHES path and changes no GHES 3.22 response for a client that does not opt in. Its Milestone 0 byte-diff row compares 1,000 REST and GraphQL requests with and without the layer deployed (`design/AGENT-LAYER.md`, "Build order").

### Cloudflare products only

Graftyard uses only Cloudflare products. A product enters the design only if the first table of `research/cloudflare.md` lists it with a usable status, and `design/DESIGN.md`, "Stack", copies that status. `scripts/check_design.py` enforces both. Workers KV, D1, Artifacts, and K2 are not used (`decisions/DECISIONS.md`, "KV, D1, and K2 are not used" and "Artifacts is not used").

workerd, the Workers runtime that runs outside Cloudflare, is the one exception. It runs only the read-only lifeboat and evaluation installs, under the defaults in "Four Cloudflare questions still carry the run's defaults" below.

### Three deployment modes, one signed artifact

One signed artifact is used in all three modes. It holds `graft-front`, the edge, the three site script bundles, `graft-ops`, the container images, web assets, record formats and fold schemas, the operator CLI, and the lifeboat bundle (`design/DESIGN.md`, "Deployment modes"). That section's table states what varies by mode for each stateful component.

| Mode | Who operates it | Where it runs | Namespace |
| --- | --- | --- | --- |
| Managed | The vendor | A front account plus one Cloudflare account per cell, all owned by the vendor | One global, public, github.com-style namespace shared by every user and org |
| BYOC | The vendor and the customer, through signed intents that `graft-ops` executes inside the customer's account. The vendor holds no Workers permission | The customer's Cloudflare account | The install's own namespace, as on GHES |
| Self-hosted | The customer | The customer's own Cloudflare account, running the same artifact with no vendor token (`Keys { vendor: None }`) | The install's own namespace, as on GHES |

Self-hosted means the customer runs the same artifact in its own Cloudflare account. It does not mean production on the customer's own hardware.

## Operator decisions

Each section below quotes the operator verbatim from `history/FRAME.md`, typos included. The paragraph after each quote says how the design applies it. Readings the operator did not state are marked [inference]. `decisions/DECISIONS.md`, "Operator decisions", gives the evidence rows for each.

### Operator decision, 2026-10-24

> "yes it needs to have org and repo name uniquess as we will support public github style stuff"

The managed service supports public, github.com-style use. User and org logins are globally unique in one namespace, and repository names are unique per owner. The run also inferred, without the operator saying so, public repositories with anonymous read, forks and pull requests across owners, stars, follows, and search across everything a caller can see. BYOC and self-hosted installs keep their own namespace, as each GHES instance does. In the design, a managed front account holds the global `Names` and `Tokens` groups on its own three sites, and owners live in cells behind it. Logins are claimed on `Names` groups placed on `Tri`, so a continent loss never stalls a claim or loses a name (`design/DESIGN.md`, "Shape", managed cells and "Not needed here"). `design/SHARED.md`, "Names", defines the login rules.

### Operator decision, 2026-10-24, later

> "you can use dynamodb global tables if that helps worldwide"

This answered an AWS-design question: the namespace's global tier lived only in North American Regions, because Aurora DSQL multi-Region clusters cannot span continents. DynamoDB global tables have a multi-Region strong consistency form that spans exactly three Regions and can cross continents (`research/uptime.md#29`, `#30`, and `research/dynamodb.md`). The decision applies only to the AWS alternative in `alternatives/AWS.md`. The Cloudflare design uses no AWS service.

### Operator decision, 2026-10-24, last

> "also do a pass at the design and ensure byoc self installs would work too."

The run reads "byoc self installs" as both BYOC installs and self-hosted installs, because either reading needs the same install machinery. A design passes only if every stateful component states its form in each mode, and a self-hoster and a vendor SRE can walk install, upgrade, recovery, and exit against the text. `design/DESIGN.md`, "Deployment modes", has a row per stateful component for each mode, and `scripts/check_design.py` checks that every component has one. The Cloudflare design was walked as a BYOC install, a managed cell, and a self-hosted install (`history/review/cloudflare-byoc-walk.md`, `history/review/cloudflare-selfhost-walk.md`, `history/review/cloudflare-rewalk.md`).

The vendor already runs a BYOC platform for another product. `research/byoc-grounding.md` describes its install model, release signing, and fleet rollouts. Graftyard reuses its release registry and signing unchanged, and `graft-ops` is the in-account executor that applies them (`design/SHARED.md`, "Release registry and signing", and `design/DESIGN.md`, "Deployment modes").

### Operator decision, 2026-10-24, on the store

> "im not trying to port but rather build an entire new, better scalable github that has less downtime"

No foundation is fixed because an earlier round chose it. Downtime and scalability outrank build convenience when foundations are compared, so keeping invariants inside database transactions counts only as far as it lowers outage risk. The round-4 arena and the Cloudflare arena both weighted downtime 3 in their rubrics (`history/review/cloudflare-rubric.md`). That weight decided the Cloudflare base: three-site Raft groups won over Cloudflare arena candidate 1, one Durable Object per aggregate with a Paired standby (`decisions/DECISIONS.md`, "Three-site Raft groups of SQLite Durable Objects are the commit point").

### Operator decision, 2026-10-26, Cloudflare

> "note all these in the docs. then, use again and come up with what this would look like entirely cloudflare native. only cloudflare technologies. they have stuff like cloudflare basin, etc."

The run read this as two asks. First, record the cost findings in the docs. They are in `research/costs.md` and `design/DESIGN.md`, "Cost", where the default install costs $264, $1,299, and $9,417 a month at 50, 300, and 2,000 developers [guess constants until Milestone 0]. Second, design a separate version that uses only Cloudflare products, with the same goals: GHES wire compatibility, less downtime first, scale second, and managed, BYOC, and self-hosted modes as far as Cloudflare allows them. At the time the run read "what this would look like" as a comparison, not a replacement [inference]. Later answers made the Cloudflare design the design of record. Cloudflare Basin was checked in `research/cloudflare.md`, found real, and is used for analytics only (`design/DESIGN.md`, "Stack").

### Operator decision, 2026-10-26, agents

> "read this. https://blog.cloudflare.com/next-git-platform-on-cloudflare why not artifacts? how can we take it a step further and make sure this is agent-friendly?"

The run read this as two asks. First, re-examine the rejection of Cloudflare Artifacts against current facts. `research/artifacts-recheck.md` did, and Artifacts still plays no role (`design/AGENT-LAYER.md`, "Artifacts' role"). Second, design an agent layer that makes AI coding agents first-class users without changing any GHES 3.22 response for clients that do not opt in. "Agent-friendly" adds users and does not relax wire compatibility [inference]. The layer is `design/AGENT-LAYER.md`, and the base changes that agent load forces are in `design/DESIGN.md`, "Shape", agent block. The run then asked seven questions about the layer and adopted a default for each. The next section lists them with the operator's answers.

### Operator decision, 2026-10-26, agents answered and graftyard

> "1 - offered is fine 2 - yes, include leases and tasks in v1 3 - sounds good as you decided 4 - default is fine 5 - sounds good 6 - sounds good 7 - ssh through spectrum read all fixes. do one final pass at everything. use for this, then clean up all notes so it is a single comprehensive folder of notes, cleaned, ready for an implementation by other agents. we are going to call this 'graftyard'."

Answers 1, 3, 4, 5, 6, and 7 keep the run's default. Answer 2 overrides it. Other documents cite a row as "agent-layer answer N" or "answer N".

| # | Question | Run default | Answer | Effect in the design |
| --- | --- | --- | --- | --- |
| 1 | Should Graftyard run its own agents (a controller, an agents account, Sandboxes, and model keys), or only host third-party agents? | Offered, off until an org enables it | Default kept | The org flag `agents_enabled` is off by default, and `POST .../tasks` for a Graftyard-run task returns 422 while it is off (`design/AGENT-LAYER.md`, "Tasks and the activity feed"). Model tokens dominate its cost: $182,400 a month at 2,000 developers at a guessed $2 a task, against a $9,417 base (`scripts/cost_agents.py`) |
| 2 | Are leases, a live who-is-working-on-what feed, and a task API must-haves for the first release? | No | Overridden: yes | Leases, the activity feed, and the task API ship in the agent layer's first release. The run reads "leases" to include the read side, the feed [inference]. They are grafted onto agent candidate A as chain state on the repository's group, not a rebase on candidate C (`decisions/DECISIONS.md`, "Leases and the task API are grafted onto A, not a rebase on C") |
| 3 | Should agent pushes get CI without a human click by default? | No: `ci: Approve`, as on GitHub, with `AutoReadOnly` opt-in per repository | Default kept | A 500-agent fleet needs about 75 approvals a minute under the default, so a Graftyard-run agent relies on the test command its controller runs in the Sandbox before each push (`design/AGENT-LAYER.md`, "Journeys", A1) |
| 4 | Should metadata child groups (base Milestone 7) move earlier so a 500-agent fleet can run on one repository at agent-layer launch? | No | Default kept | Fleets of that size on one repository wait for child groups at Milestone 7 (`design/AGENT-LAYER.md`, "Open questions and risks") |
| 5 | Is a 25% test-merge budget acceptable? | Yes | Default kept | `TEST_MERGE_BUDGET` is 0.25. It sets 7 shards per warm site at a 500-agent fleet's peak, about $990 a month if held, and most `mergeable` polls return null under load (`design/AGENT-LAYER.md`, "Tradeoffs accepted") |
| 6 | In managed mode, per-tenant App keys or per-tenant agents accounts? | Per-tenant App keys | Default kept | Managed starts with per-tenant App keys in one agents account per cell. Per-tenant accounts stay the upgrade path (`design/AGENT-LAYER.md`, "Alternatives considered") |
| 7 | Keep SSH through Spectrum, or serve HTTPS only? | Keep SSH, subject to Milestone 0's Spectrum-to-Tunnel row on each plan | Default kept | SSH goes through Spectrum, Cloudflare Tunnel, and the SSH front container's outbound handler. Milestone 0's SSH row runs on Pro, Business, and Enterprise (`decisions/DECISIONS.md`, "SSH goes through Spectrum, Tunnel, and the container outbound handler") |

"read all fixes. do one final pass at everything" became the final adversarial pass: three fresh reviewers, 43 findings, and 19 act-on items (`decisions/DECISIONS.md`, "Final pass"). The run reads "use for this" as "use poteto mode for this". The project is renamed Graftyard and delivered as one folder that implementing agents can build from. The Cloudflare design is the design of record, because every answer above is about it [inference].

During the run the operator added, verbatim, "rename [...] directory to simply 'graftyard'". The elided part names the old working folder, and `history/FRAME.md` has the full sentence. The workspace was renamed to `graftyard/` the same day and restructured in place. The record that implementers do not need moved into `history/`, and nothing was deleted (`history/README.md`).

## Four Cloudflare questions still carry the run's defaults

The run asked these on 2026-10-26 and adopted a default for each. The operator's numbered answers match the agent-layer list in content, so these four stay unanswered [inference]. Each default is the run's choice, not the operator's. `decisions/DECISIONS.md` and `design/DESIGN.md`, "Open questions and risks", track them.

1. May Graftyard run workerd plus Docker on non-Cloudflare hardware for (a) a read-only lifeboat, (b) evaluation, (c) production? Default: yes, yes, no. Self-hosted then means the customer's own Cloudflare account, and the lifeboat is the only off-Cloudflare component in production (`design/DESIGN.md`, "Deployment modes").
2. May managed users get a lifeboat on vendor hardware for Cloudflare-wide outages? Default: no, so managed has no service during one (`design/DESIGN.md`, "Tradeoffs accepted").
3. Is a home-grown Raft over Durable Objects acceptable as the commit point? Default: yes, gated by Milestone 0's 48-hour linearizability run, with Cloudflare arena candidate 1 as the fallback (`design/DESIGN.md`, "Synthesis decision").
4. Should BYOC require the Enterprise plan? Default: no. Pro carries 100 MB HTTPS pushes and Business 200 MB, and pushes over 200 MB need Enterprise.

## Out of scope

These are outside what Graftyard builds. Each one names where the design says so.

- The AWS design. It is a summarized alternative, and `alternatives/AWS.md` names the conditions under which it would be the better choice.
- Production on workerd or any non-Cloudflare hardware. workerd runs only the lifeboat and evaluation installs, unless the operator answers question 1 above otherwise (`design/DESIGN.md`, "Deployment modes").
- A writable lifeboat (`design/DESIGN.md`, "Alternatives considered"), and any lifeboat for managed mode under the current default (`design/DESIGN.md`, "Deployment modes").
- Workers KV, D1, Artifacts, K2, and any product that `research/cloudflare.md` does not list, such as AI Gateway (`design/AGENT-LAYER.md`, "Alternatives considered").
- A GPUI native app at launch (`design/SHARED.md`, "Frontend and native app").
- Any agent surface under `/api/v3/agents/*`, and any new webhook event for agents that GHES clients would see (`design/AGENT-LAYER.md`, "Alternatives considered").
- Ref lanes for agent prefixes, a coordination Durable Object per repository, and a WebSocket feed (`design/AGENT-LAYER.md`, "Alternatives considered").
- Surfaces whose tier is Out in `design/SHARED.md`, "Surfaces, tiers, and oracles".
- Pricing for an Enterprise contract, Actions runner compute, and the Sandbox compute and model tokens of Graftyard-run agents. The cost tables state these exclusions (`design/DESIGN.md`, "Cost").
