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
- The web app is React 19 on TanStack Start in SPA mode, with ReUI's free components and HugeIcons' free icons (`design/SHARED.md`, "Frontend and native app"; "Operator decision, 2026-11-08, web stack" below).
- A GPUI native app does not earn a place at launch (same section).

## Goals and quality attributes in priority order

Graftyard's goals, in order, are uptime, zero lost acknowledged pushes, and scale. The operator ranked uptime first in the original request. The 2026-10-24 decision "on the store" put downtime and scalability above build convenience, and the 2026-10-26 Cloudflare request kept "less downtime first, scale second."

The quality attributes, in priority order:

1. Availability of git reads and writes, pull request review and merge, and the REST and GraphQL APIs.
2. Durability. The system never loses an acknowledged push.
3. Wire compatibility.
4. Latency of git operations and page loads.
5. Cost and operational burden in each deployment mode.

When two attributes conflict, the higher one wins. For example, every `Tri` write pays one intercontinental round trip, which Milestone 0 measured at p50 107 to 189 ms under load and about 450 to 700 ms of commit p99 [measured], to buy RPO zero across regions (`design/DESIGN.md`, "Tradeoffs accepted").

## Constraints

### What wire compatible means

An unmodified client works after its user points it at a Graftyard host, the same way they point it at GHES today. Graftyard targets GHES 3.22. gh classifies any host that is not `github.com`, `*.ghe.com`, or `github.localhost` as GHES and derives `/api/v3/` and `/api/graphql` from it (`design/SHARED.md`, "Wire compatibility").

`research/clients.md` lists the clients that decide the question. `research/wire-census.md` and `research/actions-protocol.md` count the surfaces those clients touch. `design/SHARED.md`, "Surfaces, tiers, and oracles", gives each surface a tier and a conformance oracle, which is an unmodified real client that must pass. "Known deviations from GitHub" in the same document lists every place a client can observe a difference.

The agent layer adds no GHES path and changes no GHES 3.22 response for a client that does not opt in. Its Milestone 0 byte-diff row compares 1,000 REST and GraphQL requests with and without the layer deployed (`design/AGENT-LAYER.md`, "Build order").

### Cloudflare products only

Graftyard uses only Cloudflare products. A product enters the design only if the first table of `research/cloudflare.md` lists it with a usable status, and `design/DESIGN.md`, "Stack", copies that status. `scripts/check_design.py` enforces both. Workers KV, D1, Artifacts, and K2 are not used (`decisions/DECISIONS.md`, "KV, D1, and K2 are not used" and "Artifacts is not used").

workerd, the Workers runtime that runs outside Cloudflare, is one exception. It runs only the read-only lifeboat and evaluation installs, under the defaults in "Four Cloudflare questions still carry the run's defaults" below. The other, until go-live only, is one Oracle Cloud Always Free VM that relays plain SSH for `graftyard.dev` ("Operator decision, 2026-11-02, plain SSH through an Oracle Cloud VM" below).

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

The vendor already runs a BYOC platform for another product, with its own install model, release signing, and fleet rollouts. Graftyard reuses its release registry and signing unchanged, and `graft-ops` is the in-account executor that applies them (`design/SHARED.md`, "Release registry and signing", and `design/DESIGN.md`, "Deployment modes").

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
| 5 | Is a 25% test-merge budget acceptable? | Yes | Default kept | `TEST_MERGE_BUDGET` is 0.25. With the git CPU Milestone 0 measured, test merges cost 0.21 vCPU at a 500-agent fleet's peak and set no extra warm shards, so `mergeable` polls get answers within the budget (`tests/m0/results/m0-wp9.md`, B0-34) (`design/AGENT-LAYER.md`, "Tradeoffs accepted") |
| 6 | In managed mode, per-tenant App keys or per-tenant agents accounts? | Per-tenant App keys | Default kept | Managed starts with per-tenant App keys in one agents account per cell. Per-tenant accounts stay the upgrade path (`design/AGENT-LAYER.md`, "Alternatives considered") |
| 7 | Keep SSH through Spectrum, or serve HTTPS only? | Keep SSH, subject to Milestone 0's Spectrum-to-Tunnel row on each plan | Default kept | SSH goes through Spectrum, Cloudflare Tunnel, and the SSH front container's outbound handler. Milestone 0's SSH row runs on Pro, Business, and Enterprise (`decisions/DECISIONS.md`, "SSH goes through Spectrum, Tunnel, and the container outbound handler") |

"read all fixes. do one final pass at everything" became the final adversarial pass: three fresh reviewers, 43 findings, and 19 act-on items (`decisions/DECISIONS.md`, "Final pass"). The run reads "use for this" as "use poteto mode for this". The project is renamed Graftyard and delivered as one folder that implementing agents can build from. The Cloudflare design is the design of record, because every answer above is about it [inference].

During the run the operator added, verbatim, "rename [...] directory to simply 'graftyard'". The elided part names the old working folder, and `history/FRAME.md` has the full sentence. The workspace was renamed to `graftyard/` the same day and restructured in place. The record that implementers do not need moved into `history/`, and nothing was deleted (`history/README.md`).

**Note, 2026-10-31: answer 7's condition fired on Pro.** Milestone 0's Spectrum-to-Tunnel row (B0-24) ran on a Pro zone and failed its documented flip, "If Spectrum cannot reach a Tunnel origin on Pro or Business, SSH needs Enterprise for BYOC". The Spectrum API rejected `virtual_network_id` as "unknown field" (10012, pay-as-you-go accounts are limited to `protocol`, `dns`, and `origin_direct`), and refused even a plain SSH app on port 22 with "The requested protocol is not available" (13002) [measured, `tests/m0/results/m0-wp7.md`]. The Tunnel and container half, through `cloudflared access ssh`, served 99.886% of 2,637 sessions over 2.2 h with a container killed every 10 minutes. Applying the flip executes the operator's own condition and does not reverse answer 7, whose text above is unchanged: SSH stays, on Enterprise only for BYOC, and Pro installs serve git over HTTPS only. Business is not run and is treated like Pro until a Business zone runs the row [unverified] (`decisions/DECISIONS.md`, "SSH goes through Spectrum, Tunnel, and the container outbound handler"). The operator then refined the answer ("Operator decision, 2026-10-31, SSH ingress"): Pro installs may turn on SSH through `cloudflared access ssh`.

### Operator decision, 2026-10-28, commit latency

> "ok, i approve. continue on."

The operator approved keeping the three-site Raft base after Milestone 0 measured `Tri` commit p99 at 399 to 690 ms, over flip condition 1's 400 ms line. B0-01 now passes at a commit p99 of at most 600 ms and fails over 1 s, and flip condition 1's latency clause is 1 s. Durability outranks latency in the list above, so the base keeps RPO zero across regions (`decisions/DECISIONS.md`, "Keep `Tri` and gate commit p99 at 600 ms").

### Operator decision, 2026-10-28, latency accepted

> "just continue, latency is fine. continue the full build of the platform"

Every Milestone 0 latency gate that rests on the cross-region round now uses the measured network: 600 ms commit p99 and 1 s learner read p99 (`decisions/DECISIONS.md`, "Measured latency is accepted, and the build continues"). The build continues past Milestone 0 by `build/BUILD-PLAN.md`.

### Operator decision, 2026-10-28, B0-03 and B0-25

> "both is fine"

B0-03 passes at write and read RTO within 13 s at p99 (was 12 s) and B0-25 at 92% of pushes within three attempts (was 96%), the values Milestone 0 measured. Both fail lines are unchanged (`decisions/DECISIONS.md`, "B0-03 and B0-25 are restated to their measured values").

### Operator decision, 2026-10-29, B0-19 and pushes over the plan limit

> "let's skip b0-19. moving on." and, on the bare 413 for pushes over the plan limit, "sure, whatever you think is best"

B0-19, container starts with every account API token revoked, is skipped, so C8 stays [unknown] and warm shards stay sized to carry a day without a new start. An HTTPS push over the plan's body limit gets Cloudflare's bare `HTTP 413`, which Graftyard cannot replace, and the install docs and the 413 runbook name SSH for large pushes (`decisions/DECISIONS.md`, "Pushes over the plan's body limit get Cloudflare's 413"). Since 2026-10-31 they name SSH on Enterprise, or on Pro and Business when the install has the `cloudflared` SSH option on, and otherwise Enterprise or splitting the push ("Operator decision, 2026-10-31, SSH ingress").

### Operator decision, 2026-10-30, delegation

> "Stop waiting on me besides for the pro account. Do everything else and make the decisions. You are the pro."

The run now makes every Milestone 0 decision that does not need the Pro zone, and records each in `decisions/DECISIONS.md` with its evidence and flip condition. The operator's first use of this delegation decided four open items: the B0-07 gate counts groups active in the 50 s before a drain from the drain and other groups from their next request; B0-33's Human and merge-queue line is 1 s, as B0-01's fail line is; a PR with no recorded requester needs one more human approval; and agents may not write `refs/notes/ai` (`decisions/DECISIONS.md`, "Decisions under the operator's delegation, 2026-10-30"). Buying the Pro zone, or any paid add-on, still waits for the operator [inference].

### Operator decision, 2026-10-31, SSH ingress

> "ok. https by default. cloudflared as an option on pro for those who want it. ssh on enterprise. build to that"

This refines agent-layer answer 7 after its condition fired on Pro (the note under the answer table); answer 7's text is unchanged. HTTPS is the default git transport on every plan. On Pro, and on Business by the same plan table [unverified], an install may turn on an SSH option, off by default: a Tunnel public hostname of type `ssh` that clients reach with `cloudflared access ssh` as their SSH `ProxyCommand`. On Enterprise, SSH is native, through Spectrum on port 22 to the Tunnel's virtual network. Both ingresses end at the same SSH front containers, `graft-sshd`, and `ssh_outbound`. The run reads "on pro" to include Business, which has the same Spectrum plan entry and was not run [inference]. The option's path is the one Milestone 0 measured at 99.886% of 2,637 sessions over 2.2 h with kills [measured, `tests/m0/results/m0-wp7.md`], and its gate is B0-24a (`decisions/DECISIONS.md`, "SSH goes through Spectrum, Tunnel, and the container outbound handler").

### Operator decision, 2026-10-31, Pro only and no GHES

> "ok skip. get it done given our current pro plan. i won't have enterprise for a while, if at all. also don't have an active github enterprise either. just get it done given these limitations."

The operator had just been told that a Business zone would only confirm the Business cells of B0-20 and B0-21 and Spectrum's refusal on Business, and asked whether M1 can start. "skip" means no Business zone is bought. The build proceeds on the one Pro zone, `graftyard.dev`, and the one Milestone 0 account. There is no Business zone, no Enterprise zone, perhaps ever, and no GHES 3.22 instance. Milestone 0 is accepted as closed for starting M1: the rows short of their Pass value carry forward to the reruns the build plan already names, with no gate number changed, and a Fail value on any of them still sends the design back. Rows that need a plan the operator lacks stay not run, owned by the first install on that plan. The Enterprise Spectrum ingress stays designed and is not built until an Enterprise install exists; the SSH ingress decision above stands. Oracles that needed a GHES 3.22 appliance move to the pinned `ghes-3.22` descriptions and github.com, marked weaker [inference] (`decisions/DECISIONS.md`, "Decisions for a Pro-only build with no GHES, 2026-10-31").

### Operator decision, 2026-11-02, plain SSH through an Oracle Cloud VM

> "oracle has the free for forever tier ... spin up a subagent - use the oracle credentials to deploy a server that is free eligible. save the ssh key in 1password. use that server as the proxy for ssh as we can't have ssh unless on enterprise. i recognize this defeats the point of 'cloudflare' only but will make this an exception until go-live. eventually i will have the enterprise conversation with cloudflare."

This refines the SSH ingress decision of 2026-10-31 for the operator's own install, `graftyard.dev`. Until go-live, plain SSH on port 22 reaches the SSH front through one Oracle Cloud Always Free VM, which forwards TCP into the Tunnel public hostname that the `cloudflared` option uses, so a stock `git clone git@ssh.graftyard.dev:...` works with no `cloudflared` on the client. It is an exception to "Cloudflare products only", the only non-Cloudflare product in a serving path, and it ends at go-live. The VM's SSH key is kept in 1Password. The Enterprise Spectrum ingress stays the target: it is designed, and it is built when the operator has the Enterprise conversation with Cloudflare and an Enterprise zone exists. HTTPS stays the default, and the `cloudflared` option and its gate are unchanged. It was deployed and measured on 2026-11-03: `cloudflared access tcp` on port 22 at the DNS-only `ssh.graftyard.dev`, one Tunnel stream per connection, never terminating SSH and holding no git key or credential, with 249 of 249 sessions in 25 minutes [measured, `tests/m1/results/oci-ssh.md`] (`decisions/DECISIONS.md`, "Plain SSH through an Oracle Cloud Always Free VM until go-live (2026-11-02)"; `design/DESIGN.md`, Failure modes, `ssh relay`).

### Operator decision, 2026-11-06, BM-2 latency

The operator, Ben Kraus, chose to reword BM-2's latency clause rather than chase it with more code or move identity leaders. The clause becomes: the merge p99 on a freshly elected leader on the group's home site, with the client in that region, is at most 2 s, and on a freshly elected leader off the home site at most 3 s. BM-2's other clauses are unchanged: `gh pr` and `gh issue` journeys 2 and 3 pass, and a queue entry with a stale `parent_tip` is refused 100 times out of 100. The 2 s had been budgeted against nothing in the design, which assumes a client near the leader and moves leaders home by preferred-site transfer, and after the shard repack what remained off home was cross-region identity reads (`decisions/DECISIONS.md`, "BM-2's latency clause measures a home-site leader, with a looser bound off home"; `build/BUILD-PLAN.md`, M2-WP6).

### Operator decision, 2026-11-08, web stack

> "I want to use Tanstack Start, tanstack query, tanstack db, basically everything from tanstack. For ui components I want to use all components that are free from ReUI. And I want to use the free HugeIcons icons."

The operator also asked to start the web app (M4-WP5) now against a mocked API. This replaces Primer React. The web app is React 19 on TanStack Start, Router, Query, and DB, with TanStack Form, Table, and Virtual where a page needs them. Its components are shadcn/ui on Base UI plus ReUI's free primitives and examples, installed from the `@reui` registry with no license key. Its icons are the free HugeIcons set. Start runs in SPA mode, so the build is static assets and an HTML shell, and the design's rule of no Node server-side rendering tier holds [inference: SPA mode prerenders only the shell at build time]. Until the edge serves a page's routes, the app reads a mock that answers the pinned GHES 3.22 REST description (`decisions/DECISIONS.md`, "The web app is TanStack Start with ReUI and HugeIcons, started on a mocked API (2026-11-08)").

### Operator decision, 2026-11-16, 90% gates and a daily spend limit

> "the goal is to get within 90% of the goals for the runs. if we get to 90% of the way there, and it's a long tail of effort and testing to get the last 10% done, we note it and move on"

> "I still want to keep going and prove this out though. you are approved at $200/day spend for the entire account, starting now."

A hosted gate row is met when its measured result reaches 90% of each target: a rate or count at 90% of its goal (such as 45 of 50 jobs finishing), or a latency within 10% over its bound (such as a 1 s p99 met at 1.1 s) [inference: the operator gave the rule in words, so "within 90%" is read the same way for limits and for goals]. When a row reaches that and what is left is a long tail of work and reruns, the row is recorded as met at 90%, with its gap in `decisions/OPEN-QUESTIONS.md`, and work moves on. The rule does not relax correctness: a lost acknowledged push, a broken quorum rule, a GHES response change for a client that did not opt in, or any other invariant in `AGENTS.md` still fails its row outright. Spend is at most $200 a day across the whole Cloudflare account, counted per UTC day from 2026-11-16 20:30Z and read with `tests/fanout/acctcost.py`, after per-install meters missed most of a $2,328 bill for 2026-10-27 to 2026-11-16 (graft-m1's runs and idle containers were on no meter).

### Operator decision, 2026-11-17, $300 for 2026-11-17

> "raise to $300 total spend today"

> "GET. THIS. BUILT. I want results."

For the UTC day 2026-11-17 the account-wide spend limit is $300 instead of $200, read with `tests/fanout/acctcost.py` as before. Hosted rows run in parallel, one install each, attributed by script prefix. The 90% rule and the invariants above are unchanged. Later days return to $200 unless the operator says otherwise.

### Operator decision, 2026-11-18, a separate commit bound for rehomed groups

> "separate bound is fine"

The orchestrator asked, for B0-31 and `decisions/OPEN-QUESTIONS.md` F81, whether a repository group whose leader is rehomed onto another continent than its owner's identity groups keeps the 1.1 s commit line (which would need a dependency lease) or gets a separate bound. The operator chose the separate bound. Such a group's buckets-up commit p99 is bounded at 2 s, failing over 3 s; every group led on its owner's continent keeps its own line. The bound is the measured floor plus one round: on B0-31 m3fr16 the cross-continent `deps:all` p99 was 820 ms and the round p99 510 ms, about 1.33 s before any queueing [measured, `tests/am1/results/lm1-rows-m3f.md`, "Rows m3fr16"]. The 90% rule applies to it as to any line. The dependency lease stays the recorded fallback in `decisions/DECISIONS.md`, "F81".

### Operator decision, 2026-11-19, Pro plan

> "1. Pro is fine."

The orchestrator asked, after the cost-lever review, whether the default zone plan could be Pro instead of Business at 300 and 2,000 developers: Pro list is $25 a month against Business's $250, and Business has a 200 MB request body limit where Pro has 100 MB (`research/cloudflare.md`, Workers) and a "100% Uptime" SLA (`research/cloudflare-uptime.md`, finding 29), which Pro does not carry [inference: no Pro SLA was found]. The operator chose Pro. The default zone plan is Pro at every tier, so the default HTTPS push body limit is 100 MB, and `scripts/cost.py` prices Pro at 50, 300, and 2,000 developers. Business (200 MB) and Enterprise (up to 5 GB, and native SSH through Spectrum) remain install options for an install that needs them, priced outside the tables [inference: the operator named the default only]. A push over 100 MB on the default plan gets Cloudflare's bare 413, and the install docs and the 413 runbook name the `cloudflared` SSH option where the install has it on, and otherwise Enterprise or splitting the push, as before; an install whose pushes fall between 100 and 200 MB can move to Business instead [inference] (`decisions/DECISIONS.md`, "`pro-default`").

### Operator decision, 2026-11-19, B0-29's commit line

> "Move the target"

The orchestrator asked whether B0-29's commit p99 of 600 ms (660 at 90%) should move, since one entry in flight cannot meet it. On run 7, A-B-A row 1 and the within-network control, `/stats` commit p99 was 817, 717 and 781 ms with no admission queue. The two rounds alone, the entry in flight plus the command's own, were p99 587, 603 and 570 ms, and the freeze wait made 37–45% of the tail [measured, `tests/m0/results/b029-tail.md`]. The best freeze change, a late join, was predicted to reach 610–758 ms [inference] (`decisions/DECISIONS.md`, "B0-29 late join deferred"). The operator moved the line instead of funding round-time work. B0-29's commit p99 is at most 800 ms: two rounds at p99 plus one median round for the freeze wait, 770 ms rounded up. It still fails over 1 s. The 90% rule applies, and the row's other lines are unchanged. The 600 ms line for 100 commands/s on `Tri` (DESIGN, Milestone 0) is unchanged. It is a different row, with one round per command and no freeze.

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
