# Graftyard build plan

This plan splits the build order in `design/DESIGN.md` ("Build order from zero") and `design/AGENT-LAYER.md` ("Build order") into numbered work packages. It adds no measurement of its own. Every number in a gate comes from the design. Where this plan places work that the design does not place, the text says so and tags it [inference].

## How to use this plan

- Start with Milestone 0. It runs about six weeks on one throwaway account, before any product git code, and any fail value sends the design back (`design/DESIGN.md`, "Build order from zero", "Milestone 0"). Durations are [guess]es for a team of six.
- A work package is done only when every gate row assigned to it meets its Pass value. A row's Fail value in Milestone 0 sends the design back. The design does not say what a result between the Pass and Fail values means. Treat such a result as not done and raise it [inference].
- Do not change a number in a gate. To change one, change the design first.
- Each work package names the Shape types and functions it builds and the section that specifies them. Read that section before you write code. `design/SHARED.md` holds the wire surfaces, their tiers, and their oracles. Where `design/SHARED.md` and `design/DESIGN.md` disagree, `design/DESIGN.md` wins.
- A work package with no gate row is a prerequisite. Its acceptance names the part of a later gate it must carry.
- The design lists some rows under Milestone 0 that need code from later milestones. For example, the byte diff row needs REST. AM1's gate says "the base's agent rows rerun on real code", so this plan reads the Milestone 0 runs of agent rows as runs on prototype code [inference]. Each such row belongs to a Milestone 0 work package. The later gate that reruns it names it by id.
- The milestones run in this order: M0, M1, M2, M3, AM1 (built with M2 and M3), M4, M5, AM2 (after M5), AM3 (after M5), M6, AM4 (after M6), M7. This is where the design puts the agent milestones ("AM1, with base 2 and 3", "AM2, after base 5", "AM3, after base 5", "AM4, after base 6").
- Row ids are this plan's, numbered in table order. B0-nn is the base Milestone 0 table. BM-n is the base "Milestones 1 to 7" table. BF-n is the base table of rows added after the final pass: BF-1 to BF-9 from the final pass, BF-10 to BF-53 from the implementability walk, and BF-54 onward from the third walk. L0-nn is the layer "Milestone 0 rows" table. LM-n is the layer "Milestones" table. LF-n is the layer table of rows added after the final pass: LF-1 to LF-11 from the implementability walk, and LF-12 onward from the third walk. An LF row whose milestone is 0 belongs to a Milestone 0 work package. The coverage table at the end lists every row once.
- The tables of milestones (BM and LM) have a Gate column and no Pass or Fail column. For those rows, Pass is the gate text, and Fail is any result that does not meet it.
- `decisions/OPEN-QUESTIONS.md` lists the open questions, risks, and unverified claims that each work package depends on. Do not assume an answer to any of them.

## Component map

Each deployable or module, the Shape section that specifies it, and the work package that first builds it for real.

| Component | What it is | Specified in | First built |
| --- | --- | --- | --- |
| `graft-front` | Stable Worker in front of everything. It forwards each request to the edge, and `mcp.<host>` to `graft-mcp`, through `fetch()`-based service bindings. It sets a version override only while its cached `Pointers` pin the script, which only a pre-signed undo does. It alarms when the edge reports another version. | `design/DESIGN.md`, Stack (Workers) and Shape ("Release group", `Pointers`, `POINTER_TTL`); "Commit and read paths" | M5-WP2 |
| The edge | Worker for HTTPS git, REST, GraphQL, and web assets. It authenticates, reads tokens and `Authz` on their leaders, calls group members through bindings by site class with the compiled hint table, and mints installation tokens through `graft-ops`. | `design/DESIGN.md`, Stack (Workers), Shape ("Sites, not services, are the failure unit", "Identity"); `design/SHARED.md`, "Wire compatibility" | M1-WP6 (git), M2-WP4 (REST and GraphQL) |
| Site scripts (`graft-site-a`, `graft-site-b`, `graft-site-c`, plus spare and residency sites) | One Worker script per site, pinned to one region, with no public route. Each defines the site's member class, its git cache shard class, its container classes, and its `MemberRegistry` shards, and owns one pack bucket. Site crons run the pack sweeper, GC, and fold audit. | `design/DESIGN.md`, Shape ("Sites, not services, are the failure unit", "Sites and placement", "Release group"); Stack (Durable Objects, Cron Triggers) | M1-WP1 |
| Member class (in each site script) | Group members and learners: Raft log, lease, both fold tiers in SQLite with FTS5, and consumer cursors. | `design/DESIGN.md`, Shape ("Chains", "Leadership", "The write path", "The read path"); `design/SHARED.md`, "Folds" | M1-WP2 |
| Git cache shard class (in each site script) | `GitCacheShard { site, shard }`, one container-backed Durable Object per (site, shard), with `validate`, `upload_pack`, `merge`, and later `predict`. Derived state only. | `design/DESIGN.md`, Shape ("Git cache shards (derived)"); `design/AGENT-LAYER.md`, Shape ("Overlaps and prediction") | M1-WP5 |
| Git container image | Upstream git (index-pack, upload-pack, merge-tree, repack) run by git cache shards under the `durable_object` scheduling policy, started by each shard object with `ctx.container.start()`. | `design/DESIGN.md`, Stack (Containers), Shape ("Git cache shards (derived)") | M1-WP5 |
| `RunnerBroker` and `LogRelay` classes (in each site script) | Derived objects on their Actions shard's leader site. `RunnerBroker { owner, scope }` holds runner long polls up to `BROKER_HELD_MAX`, matches queued jobs by label from shard summaries and counted hints, and serves hosted demand. `LogRelay { job }` takes the runner's `FeedStreamUrl` WebSocket and sends each line to the web viewers. | `design/DESIGN.md`, Stack (Durable Objects), Shape ("Identity"); `design/SHARED.md`, "Actions runs and jobs" and "Actions" (Logs) | M4-WP3 |
| `HookSequencer` class (in each site script) | One object per hook owner on the owner's home site. It is the webhook delivery store (`HookDelivery` rows), with one queue per (chain, hook), one attempt in flight per queue, and at most `HOOK_OWNER_CONCURRENCY` attempts per owner. | `design/DESIGN.md`, Shape ("Identity"); `design/SHARED.md`, "Event fan-out" | M4-WP1 |
| `Inbox` class (in each site script) | One derived object per account, keyed by `{ account, site }` and filled by `Notify`. It serves notifications, feeds, `/issues`, and `/user/repos`, writes a daily snapshot with a `sources` table, and rebuilds from that snapshot plus `INBOX_WINDOW` of watched repositories. | `design/DESIGN.md`, Shape ("Identity") | M4-WP2 |
| `SearchShard` class (in each site script) | FTS5 tables in one object per (residency domain, shard), 16 shards per domain [guess], on the domain's first Full site under the domain's DO jurisdiction, written by the `Search` consumer. | `design/DESIGN.md`, Shape ("Identity"); `design/SHARED.md`, `rest:search` and "Public service concerns" (Residency) | M4-WP2 |
| `RateCounter` class (in `graft-site-a`) | One object per `RateKey` that holds the window total behind `x-ratelimit-*` where GHES rate limits are on. Each isolate adds its count every `RATE_FLUSH` requests or 1 s. | `design/DESIGN.md`, Shape ("Identity"); `design/SHARED.md`, "Public service concerns" | M7-WP3 [inference: the design gates it in Milestone 7] |
| Workflows script | The script that runs the Workflows: `ReplaceMember`, `RestorePlacement`, owner placement moves, imports, repository transfers, network compaction, `MetaSplitter` for child groups, and the `LeaveEvaluation` group moves. It binds every site's member class and bucket, and it rolls on its own `SiteDay { Workflows }`. | `design/DESIGN.md`, Stack (Workflows), Shape ("Members reach each other through Durable Object bindings", "Sites and placement", "Chains" (`CHILD_GROUP_AT`), `DayTarget`) | M1-WP7 (owner placement moves) [inference], M5-WP4 (its site day), M6-WP1 (`ReplaceMember`), M7-WP4 (`MetaSplitter`), M7-WP5 (`LeaveEvaluation`) |
| Queue consumers | The `Consumer` variants (`Webhooks`, `ActionsTrigger`, `RunOwner`, `PrSync`, `MergeQueue`, `Notify`, `Search`, `Audit`, `Counts`, `Insights`), fed from leader cursors through Queues, plus the consumer that writes the change feed. The design puts the insights consumer in the site scripts. The location of the others is not stated [inference: the same site scripts]. | `design/SHARED.md`, "Event fan-out"; `design/DESIGN.md`, Shape ("Merge queue", "Lifeboat feed", "Reused with the additions below", "Dominant access patterns") | M2-WP5 (cursors, Queues, `PrSync`), M2-WP3 (`MergeQueue`), M4-WP1 (the rest), M5-WP5 (change feed) |
| `graft-ops` | Stateless Worker with no member classes. It executes signed intents, uploads releases from the release bucket with the customer's deploy token, owns every cron that calls the Cloudflare API and every rotatable secret, mints installation tokens, drains a site with its drain-only key, and commits `ReleaseUploaded`, `IntentStep`, `IntentDone`, `SiteRelease`, and `ReplaceMember` with its executor key. | `design/DESIGN.md`, Stack (Workers, Secrets Store), Shape ("Release group", "Identity"), "Install operations" | M3-WP4 (minting), M5-WP3 (executor) |
| SSH front | Container class and image running `graft-sshd` and cloudflared replicas. Two ingresses reach it through Tunnel: Spectrum on port 22 on Enterprise, and on Pro and Business, when the install turns the SSH option on, a Tunnel public hostname of type `ssh` that clients reach with `cloudflared access ssh`. Its outbound handler `ssh_outbound` runs in the Workers runtime with the site script's bindings and hands the containers their host keys from Secrets Store. | `design/DESIGN.md`, Shape ("SSH front"), journey 4, Stack (Spectrum, Cloudflare Tunnel) | M6-WP2 |
| Runner image and hosted runners | Unmodified `actions/runner`, one job per Sandbox, in a separate runner account. | `design/SHARED.md`, "Actions" (Hosted runners); `design/DESIGN.md`, Stack (Sandbox SDK) | M4-WP4 |
| `graft-runners` | Worker in the runner account that holds `HostedRunnersAppKey`. It reads hosted demand from the `Owner`-scope broker as the `graftyard-hosted-runners` App, calls `generate-jitconfig`, starts and destroys one Sandbox per runner, and starts none past `RUNNER_VCPU_STOP_PCT`. | `design/DESIGN.md`, Stack (Workers, Sandbox SDK, Cron Triggers), Shape ("Identity"); `design/SHARED.md`, "Actions" (Hosted runners) | M4-WP4 |
| Vault Worker | Worker in the vault account. It pulls the change feed, entries, snapshots, and packs through the edge's export endpoints with an install-scoped export token signed by the customer key, and writes them to R2 under 35-day bucket locks. `graftyard vault upgrade` updates it. | `design/DESIGN.md`, Shape ("Lifeboat feed"), "Install operations" (Vault), "Failure modes" (`vault`) | M5-WP5 |
| Lifeboat | Read-only workerd bundle with a pinned workerd version on customer hosts. It rebuilds from the change feed and the export endpoints, serves clones and reads, stores files through `graft-blob`, and publishes `LifeboatStatus`. | `design/DESIGN.md`, Shape ("Lifeboat feed"), "Install operations" (Lifeboat), "Deployment modes" | M6-WP3 |
| CLI (`graftyard`) | Operator binary: `init`, `sites add`, `sites add-spare`, `genesis`, `sign`, `install`, `placement set`, `vault init`, `vault upgrade`, `runners init`, `runners upgrade`, `admin bootstrap`, `release roll`, `lifeboat init`, `lifeboat enroll`, `import ghes`, `actions import`, `apps init`, and `agents init`. | `design/DESIGN.md`, "Usage (caller's view)", "Install operations", "Failure modes" (`actions state`); `design/SHARED.md`, "Surfaces, tiers, and oracles" (`rest:migrations`) and "Actions"; `design/AGENT-LAYER.md`, "Deployment modes" and journey A8 | Across milestones; each work package names its commands |
| `graft-mcp` | Worker at `mcp.<host>` in the install account, routed through `graft-front`. It is an MCP server and a small authorization server, and it calls only the edge, over a service binding. | `design/AGENT-LAYER.md`, Shape ("graft-mcp"), journey A7 | AM2 |
| Controller | Optional `agents` account with `graft-agents-front`, `graft-controller`, and its own `graft-ops`: a GitHub App backend reached by webhooks over HTTPS, Workflows per task, Sandboxes, and the outbound router. `graft-agents-front` pins controller traffic by `Pointers.controller`. | `design/AGENT-LAYER.md`, Shape ("Controller"), journeys A3 and A8 | AM3 |
| Insights consumer | Consumer in the site scripts, fed only by its own cursor. It runs `predict` on the shard and commits one `InsightVerdict` row per ref, with its `graftyard-insights` check runs and labels, through `POST .../insights` on the edge. | `design/AGENT-LAYER.md`, Shape ("Overlaps and prediction"), "API reference" | AM4 |
| Web app | React 19 with Primer React, served by the edge from the HTML shell, reading the public GraphQL and REST endpoints. | `design/SHARED.md`, "Frontend and native app" | M4-WP5 ([inference], as `design/DESIGN.md`, "Milestones 1 to 7", tags it) |

### Proposed repository layout [inference]

The design names the deployables but no repository layout, so everything in this subsection is a proposal. The design builds `graft-front`, the edge, the three site script bundles, and `graft-ops` from Rust to WebAssembly ("Deployment modes"), through workers-rs [inference, as the design itself tags it in "Usage (caller's view)"]. It does not state a language for `graft-mcp` or the controller.

```
crates/types        Shape types shared by every script (design/DESIGN.md Shape, design/SHARED.md)
crates/raft         log, lease, transfer, suspect, pre-vote, verify, learners
crates/fold         apply, tier_of, fold tables, fold_audit
crates/wire         SHARED's cross-cutting conventions, one layer every handler uses
crates/git          PackStore, GitCacheShard client side, receive-pack, report_status
workers/front       graft-front
workers/edge        the edge
workers/site        one source tree, built as the three site script bundles
workers/ops         graft-ops
workers/workflows   the Workflows script
workers/vault       the vault Worker
workers/runners     graft-runners
workers/mcp         graft-mcp
containers/git      git container image
containers/ssh      graft-sshd and cloudflared
containers/runner   runner image
lifeboat/           workerd config, pinned version, graft-blob
cli/                graftyard
agents/controller   controller App backend and its outbound router
tests/m0/           throwaway Milestone 0 harness, never shipped
```

## Milestone 0: measure the platform and the protocol

About six weeks on one throwaway account with three site scripts, before any git code. Every package runs in that one account, with no second account and no trial account or zone (`decisions/DECISIONS.md`, "Milestone 0 runs in one account"). Any fail value sends the design back (`design/DESIGN.md`, "Milestone 0"). The layer adds its rows to the same milestone (`design/AGENT-LAYER.md`, "Milestone 0 rows").

### Milestone 0 status (2026-10-31)

Every package ran on prototype code in the one Milestone 0 account, except M0-WP7, which needs a zone on Pro or above. Results are in `tests/m0/results/`, one file per package, and each fix the runs found went into the design first (`decisions/DECISIONS.md`, "Milestone 0"). Rows still short of their Pass value, each with its owner:

| Row | State | What closes it |
| --- | --- | --- |
| B0-11 | Passes: 0 violations on 5,742,736 operations and 0 lost acknowledged writes in 48 hours, with a 100 ms clock allowance across client colos that the run did not measure; fault coverage on site C is thin (F36) | M1-WP2's 24-hour rerun with per-isolate fault counters and recorded client clock offsets |
| B0-02 | Between pass and fail: 159 to 280 commands/s at p99 750 to 1,288 ms against 300/s at 600 ms | M1-WP2 on real code |
| B0-07 | 179 of 182 groups within 20 s as written, 181 of 182 under the 50 s form; one group at 20.2 s unexplained | A rerun on M1-WP3's code |
| B0-20, B0-21 | Ran on the Pro zone `graftyard.dev`: the 100 MiB cap holds for sized and chunked bodies, and git over it sees a bare 413, as the 413 decision accepts. Business and Enterprise not run | A zone on each of those plans |
| B0-24a | The Pro and Business `cloudflared` option: 99.886% over 2.2 h with kills on Pro | 24 h rerun on graftyard.dev (running) |
| B0-24b | Enterprise Spectrum: not run (Spectrum refuses a Tunnel origin on Pro) | Needs an Enterprise zone |
| B0-29 | Passes in 5 of 8 runs; misses at 602 and 625 ms track placement (first pass, `r2` and the `sd3r1` variant) and 809 ms tracks storage stalls on the leader (fifth pass, `p5r1`) | LM rerun on real code |
| B0-34 | Partial pass at reduced scale, with the composed-miss rules now in the design | A full-scale run with the warm pool |
| L0-06, L0-11, L0-04 | Partial: predictor without other git work; byte diff without a GHES oracle; agent CI without an Actions engine | LM rows that rerun them on real code |
| L0-07 | Vendor cells not run: they need vendor accounts and a public host | The operator's vendor accounts |
| L0-08 | The `ghr_` cell needs a GHES 3.22 instance (A4) | A GHES 3.22 instance |
| LF-12 | Between pass and fail on the final rules: 1 pass, 3 between, 0 fail in 4 runs; misses are storage stalls and distance | LM rerun on real code |

### M0-WP1. Test bed, bindings, and placement facts

**Build.** One throwaway account with prototype site scripts `graft-site-a`, `graft-site-b`, and `graft-site-c` that bind each other's member classes and buckets with `script_name`, in cycles (`design/DESIGN.md`, Shape, "Members reach each other through Durable Object bindings"). No other account and no trial zone: M0-WP5 emulates the account-separated hop inside the account, and M0-WP6 and M0-WP7 run on zones added to it. A fault injector for member calls and storage writes, a Porcupine history recorder, and load clients pinned to colos, built as their own script, `graft-m0-tools`, so changing them never creates a site-script version (`tests/m0/README.md`; self-test in `tests/m0/results/tools-selftest.json`) [inference: the rows below need these tools]. Prototype `Placement::new` with `MIN_PAIR_RTT` and per-member `Rtt` rows kept outside `GroupConfig`, measured at join and at most once a day on a day the group served a request (Shape, "Sites and placement"), and the `graft-ops` bucket `location` check (Shape, the placement paragraph before "Sites and placement"). Beside B0-12, measure intra-region round trips, because "Open questions and risks" says Milestone 0 measures both.

**Depends on.** None.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| B0-12 | Member placement: 1,000 members created with each site class's hint, median of 20 binding round trips to each other voter | At least 95% of voter pairs at 30 ms or more and consistent with the hinted regions | Under 80% |
| B0-13 | Bucket placement: 20 buckets per hint, `location` from the get-bucket API against a latency probe | 100% agree | The API reports the hint, not the placement |
| B0-15 | Idle cost: 100,000 idle groups for 24 hours across a UTC midnight, with the daily `Rtt` recheck shortened to hourly, beside 1,000 control groups that serve one request an hour | Zero duration billed and zero `Rtt` checks for idle groups, while the control groups show both | Any duration billed, or any `Rtt` check, for idle groups, or a control group that shows neither |
| B0-16 | Account-owned deploy token with Workers Scripts Edit and Containers Edit runs a deploy with a container image | Deploys | Fails, which moves the deploy token to a user token held by the customer |
| B0-38 | Shard containers under the `durable_object` policy: 30 starts from objects created with each site's hint, then a new site-script version with a changed image map while three run | Every container in its site's container regions, and no running container restarts or changes image | A container outside its site's regions that the start check misses, or any restart on deploy |

**Flip condition.** B0-16's Fail value moves the deploy token to a user token held by the customer (`design/DESIGN.md`, "Deployment modes", "What each mode requires of the customer"). B0-38's Fail value means the shard object cannot keep its container on its site, which reopens the scheduling-policy decision (`decisions/DECISIONS.md`, "Git cache shards run under the `durable_object` scheduling policy").

### M0-WP2. Raft prototype on Durable Objects

**Build.** A member Durable Object prototype with `LogEntry`, `EntryBody::Noop`, `MemberState`, `GroupConfig` (`Normal` and `Joint`), pre-vote, `led_term`, `RESET_QUARANTINE`, `commit`, `outcome`, `Acked`, `verify`, `fresh`, `transfer`, and `suspect`, with `LEASE`, `ELECTION_AFTER`, `DRIFT`, `CALL_DEADLINE`, and `COMMIT_RETRIES` (`design/DESIGN.md`, Shape: "Chains", "Leadership", "The write path", "The read path"). Group commit with one entry in flight. Hedged 1 s deadlines on receipts and member calls ("Commit and read paths").

**Depends on.** M0-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| B0-01 | Commit p99 on `Tri`, leader enam, 1 KB commands, 100 commands/s, member calls through cross-script DO bindings | At most 600 ms | Over 1 s |
| B0-02 | Group commit on one group, one entry in flight | At least 300 commands/s at commit p99 at most 600 ms | Under 100 commands/s |
| B0-03 | Write and read RTO, with one site script's member class failing every call (simulated region loss) under 100 writes/s, run once with every client in one colo and once with calls to that class blackholed (hung, not failing) | New leader committing and serving `fresh` within 13 s at p99 in both runs | Over 60 s in either run |
| B0-11 | Linearizability: 48 hours of Porcupine-checked histories while member calls to one site are dropped at random, members are reset, and leadership transfers, some abandoned mid-way | Zero violations, zero lost acknowledged entries | Any one |
| B0-25 | Partial DO failure: inject 22% errors into member calls and into storage writes, which reset objects, and count whole pushes | At least 92% of pushes succeed within three attempts | Under 84% |
| B0-26 | Reset storm: every member on all three sites reset within 60 s under 100 writes/s, with `suspect` reports from two colos | Every group commits within 12 s of the last reset | Any group over 12 s |

**Flip condition.** Flip condition 1 (`design/DESIGN.md`, "Synthesis decision"): any linearizability violation in the 48-hour run, or `Tri` commit p99 above 1 s, makes candidate 1 the base, after its standby-loss stall is fixed. B0-01 and B0-02 were at most 150 ms and the latency clause 400 ms until the operator restated them on 2026-10-28 from the measured network (`decisions/DECISIONS.md`, "Keep `Tri` and gate commit p99 at 600 ms"). `alternatives/AWS.md`, "When the AWS design would be the better choice", condition 1, adds that an operator who needs RPO zero across regions after that flip has it in AWS Paired [inference]. B0-02 is also the 300 commands/s figure the A1 verdict rests on (`design/AGENT-LAYER.md`, "Open questions and risks").

### M0-WP3. Gray failure and learners

**Build.** `Health`, `STEP_DOWN`, the two-colo `SUSPECT` rule, and `SiteHealth` (`design/DESIGN.md`, Shape, "Leadership"). Learners: `MemberKind::Learner`, `LEARNER_AT`, `MAX_LEARNERS`, and read-index with one request in flight and at most one every 50 ms (Shape, "Sites and placement" and "The read path").

**Depends on.** M0-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| B0-06 | Gray site, with 30% errors injected on one site's member calls, then separately on its storage writes | Every group led there moves within 20 s and changes leader at most twice in an hour | Any group slower, or more than two changes |
| B0-18 | Learners: 16 learners serving 16,000 reads/s on one group | Leader under 700 req/s, learner read p99 at most 1 s | Leader returns `overloaded` |

### M0-WP4. Site days, verify refusals, and drain

**Build.** A prototype `Release` group with `SiteMode`, `Mode`, the drain-only key in `Keys.drain`, `SignedIntent.undo`, and `Intent::SiteDay` (`design/DESIGN.md`, Shape, "Release group"). A prototype `graft-ops` that runs a site day (drain, 1%, 10%, 50%, 100%, undrain) with the apply gate and the pre-signed rollback ("Install operations", "Site day"). `MemberState.no_lead_until` and `cannot_follow`, and the refusal rules of `verify` (Shape, "The write path"). Automatic drain from `SiteHealth` with the drain-only key ("Commit and read paths", journey 5, "Automatic drain"; journey 7).

**Depends on.** M0-WP2, M0-WP3.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| B0-04 | Site deploy under 100 writes/s, with groups led from every site and clients on the leaders' continents: drain, deploy at 1%, 10%, 50%, 100%, undrain | Zero failed writes, commit p99 at most 1 s for clients on the leader's continent | Any failed write |
| B0-05 | Follower regression: a release whose N+1 followers throw while applying one event kind | The apply gate stops it at 1% and `graft-ops` runs the pre-signed rollback, with no failed write and no human signature | It reaches 10%, or any write fails |
| B0-07 | Gray site C during B's site day at 10%, with automatic drain on | Callers on A and B report C, `graft-ops` undrains B under the 1% lead limit and drains C. Every group led on C that served a request or committed an entry in the 50 s before the drain moves within 20 s of it, and every other group led on C moves within 20 s of its next request | Two sites drained at once, or any group on C slower |
| B0-08 | `verify` refusal by an older follower: release N+1 on site A writes a shape N cannot read | Every group A led moves to an undrained follower holding its last index; other writes keep committing; A stays non-leader | Any group without a leader for more than one lease |
| B0-09 | Rolling site refuses: N+1 followers on B refuse valid N entries at 10% of a site day | No leader steps down, B is marked unable to follow, an alarm fires, and the site day halts and rolls back | Any group without a leader for more than one lease |
| B0-10 | Two-site residual: A and B on N+1 commit a shape C on N cannot read | Each group keeps its leader and commits on A and B, C is marked unable to follow, and the site day halts | Any group without a leader for more than one lease |
| B0-27 | Drain guard: two drain-key `SiteMode { Drain }` entries for different sites submitted straight to the `Release` group, then a test group whose voters are all drained by customer-key entries | The second drain-key entry is refused and pages, and the all-drained group elects a leader within 12 s | Two sites drained by the drain key, or the all-drained group leaderless over 60 s |

### M0-WP5. Account-separated link

**Build.** `SiteLink::Https` with Ed25519-signed member requests through the site Worker (`design/DESIGN.md`, Shape, "Account-separated sites (option)" and "Sites and placement"). In the one Milestone 0 account, each site Worker gets a public hostname, and callers set `global_fetch_strictly_public` so a fetch to it enters through the front door as a request from another account would. The hop leaves the account's runtime the same way a cross-account call does [inference: no page says a cross-account hop takes another path]; `graft-m0-tools` `/hop` measured 7 to 10 ms for a warm hop inside one colo.

**Depends on.** M0-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| B0-17 | Account-separated option: member RPC over signed HTTPS through the site Worker, against a cross-script binding | p50 overhead at most 20 ms | Over 60 ms, which withdraws the option |

**Flip condition.** B0-17's Fail value withdraws the account-separated option. M7-WP2 is then not built.

### M0-WP6. Git on Containers and R2

**Build.** Prototypes of `PackStore::place` (two buckets, `If-None-Match: *`, `index-pack --strict` on the same bytes) and `DurablePack` (`design/DESIGN.md`, Shape, "Packs"). A `GitCacheShard` container class under the `durable_object` scheduling policy, with the start cap, the start-time region check, and the 6 h inactivity timeout renewed by alarm for 24 h (Shape, "Git cache shards (derived)"). The edge's tee of a push body to buckets and a container, with byte counting and a refusal at its own cap through a `report-status` that names Enterprise or splitting the push below Enterprise, and SSH on Enterprise (journey 1, step 2). The clone cache with `CloneKey` and `R2Key::CloneCache` (Shape, "Packs").

**Depends on.** M0-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| B0-14 | Daily working set of git cache shards per tier, in bytes, on a synthetic replay | Recorded; `cost.py` shard counts updated | None. The result sets the shard count |
| B0-19 | Container starts with the control plane out of reach: 100 new instances started with every account API token revoked | 100 of 100 start | Any fails; the control-plane row stays `contained` either way and warm shards stay sized for a day |
| B0-20 | Push size: 2 GB push through the edge on Enterprise, teed to two buckets and a container; on Pro and Business, a push over the edge cap | Enterprise completes with Worker memory under 100 MB; Pro and Business print the edge's `report-status` naming Enterprise or splitting the push | Fails, needs buffering, or shows a bare 413 |
| B0-21 | Chunked push on Pro, Business, and Enterprise | Recorded either way | None. The result sets each plan's stated cap |
| B0-22 | Container rehydration of a 5 GB network from R2 | At most 120 s | Over 600 s |
| B0-23 | Clone cache: 33 CI clones/s of a monorepo whose tip moves 10 times a minute | Hit rate at least 95%, time to first byte p99 at most 500 ms | Under 80% |

**Plans.** B0-20 and B0-21 run on zones added to the Milestone 0 account, on the plans those zones carry. A plan the account does not have is recorded as not run, and the design's claims for that plan stay [unverified] until an install on that plan runs the row in its own account.

**Flip condition.** B0-14 sets `SHARDS` and the shard counts in `scripts/cost.py`. B0-21 sets each plan's stated push cap. If B0-19 fails, the control-plane row stays `contained` and warm shards stay sized to carry a day. B0-20 and B0-21 bear on `alternatives/AWS.md` condition 4 (pushes above 200 MB without Enterprise).

### M0-WP7. SSH chain

**Build.** Tunnel and cloudflared in three containers, with `graft-sshd` calling `ssh_outbound` for writes, reached by each ingress: a Tunnel public hostname of type `ssh` through `cloudflared access ssh` for the Pro and Business option, and Spectrum on port 22 to the Tunnel's virtual network for Enterprise (`design/DESIGN.md`, Shape, "SSH front"; journey 4, step 3; `FRAME.md`, "Operator decision, 2026-10-31, SSH ingress"). Run on the zones added to the Milestone 0 account; a plan it does not have is recorded as not run, as in M0-WP6.

**Depends on.** M0-WP2, M0-WP6.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| B0-24a | The Pro and Business SSH option: clients through `cloudflared access ssh` as their `ProxyCommand` to a Tunnel public hostname of type `ssh`, then cloudflared in three containers, with writes through the outbound handler and one container killed every 10 minutes for a day. On Pro, 2,634 of 2,637 sessions (99.886%) over 2.2 h with 13 kills [measured, `tests/m0/results/m0-wp7.md`]; a 24 h rerun on graftyard.dev is running | At least 99.9% of sessions succeed | Under 99%, which leaves Pro and Business HTTPS only |
| B0-24b | Enterprise native SSH: Spectrum on port 22 to the Tunnel's virtual network, then the same containers, handler, and kills for a day. Not run: it needs an Enterprise zone. On Pro the Spectrum API refused a Tunnel origin (10012, 13002) [measured, same] | At least 99.9% of sessions succeed | Under 99%, which drops native SSH on Enterprise |

**Flip condition.** Spectrum refusing a Tunnel origin on Pro already fired: native SSH needs Enterprise, and Pro and Business get the `cloudflared` option (`decisions/DECISIONS.md`, "SSH goes through Spectrum, Tunnel, and the container outbound handler"). A failing B0-24a leaves Pro and Business HTTPS only; a failing B0-24b drops native SSH on Enterprise. Spectrum on Pro or Business accepting a virtual-network origin would bring native SSH there. `alternatives/AWS.md` condition 5 names an SSH failure as a reason to prefer AWS. M6-WP2 builds each ingress only where its row passes.

### M0-WP8. Restore window

**Build.** The `ref_log` fold table as GC roots, `RESTORE_WINDOW`, `R2Key::Snapshot`, `R2Key::Archive`, `Archived`, and truncation only through a committed range (`design/DESIGN.md`, Shape, "Packs"). `RestoredTo` (`design/SHARED.md`, "Repository repair, tombstones, and retention").

**Depends on.** M0-WP2, M0-WP6.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| B0-28 | Restore window: delete a branch and force-push another on day 1 with GC and archiving running, then restore both to day 0 on day 30 (clock-skewed replay) | Both refs return to their day-0 targets, every object they reach is readable, and archives are named by committed `Archived` records in two buckets | The restore is refused, any object is missing, or a follower truncates past a range with no committed `Archived` |

### M0-WP9. Agent load on the base

**Build.** Prototypes of admission (`AdmitClass` lanes, `AGENT_SHARE`, `AGENT_FLOOR`, `DEMOTE_PER_MIN`, `AGENT_WAIT`, `AGENT_QUEUE_MAX`), the hot inline cap (`INLINE_MAX_HOT`, `AGENT_INLINE_ENTRY`, `HOT_PUSHES`), `LOG_MAX_BYTES` archiving, fan-out (`NetworkLoad`, `fanout`, `FANOUT_MAX`, `HOT_POOL`, the per-owner pool budget), and lazy test merges (`TestMerge`, `TEST_MERGE_BUDGET`) (`design/DESIGN.md`, Shape, "Agent load and agent safety" and "Git cache shards (derived)"; journey 9). Feed polls answered on followers and learners (`design/AGENT-LAYER.md`, Shape, "Activity feed").

**Depends on.** M0-WP2, M0-WP6.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| B0-29 | Agent fleet replay (journey 9) on one `Tri` group, leader enam. 500 agents push distinct refs every 2 min (40 KB mean, 5% at 1 MiB), 27.6 commands/s with PrSync, test-merge records, check runs, task updates, and lease renewals, 45 feed polls/s on followers and learners, 10 merges/min, 33 ref reads/s, 200 API reads/s. Commit p99 on this row is set by storage stalls on the leader: of the seven fourth-to-seventh-pass runs, the five with 0 to 4 failed AppendEntries in 600 s passed at 378 to 531 ms, and the two with 10 and 12 missed at 798 and 809 ms [measured, `tests/m0/results/m0-wp9.md`, fourth to seventh passes]. The line stays one 600 s run at 600 ms, and M1-WP2 reruns it on real code | Commit p99 at most 600 ms, merge p99 at most 2 s, zero `Stale` on distinct refs, leader under 600 req/s with zero feed polls | Commit p99 over 1 s, any queue merge waiting over 10 s, or any feed poll on the leader |
| B0-30 | Hot-group storage. 30 days of the journey 9 replay on a group seeded with a 4 GB metadata tier | Member SQLite under `CHILD_GROUP_AT` with zero `SQLITE_FULL`, and archived entries readable for `RestoredTo` | Any `SQLITE_FULL`, or member SQLite over `CHILD_GROUP_AT` without a child group taking the tier |
| B0-31 | Same replay with the leader on apac and every agent push at 1 MiB, `INLINE_MAX_HOT` on, then with every bucket's placement failing, under the design's `STEP_DOWN` with no variant. The placement-failing half gates Human and MergeQueue latency, not the leader's site: the apac leader may move once on the Human packs that the outage rule keeps inline. On the final Milestone 0 code both halves passed: buckets up, the apac leader held 600 s with commit p99 598 ms and every agent push staged; placement failing, Human and MergeQueue p99 1.11 s with 0 of 287 refused, 2,500 of 2,500 agent pushes refused at once in the `ng` shape, and one `STEP_DOWN` from apac at 8.9 s (6 and 3 failed calls) [measured, `tests/m0/results/m0-wp9.md`, B0-31, eighth pass] | Commit p99 at most 1 s with buckets up. While placement fails, Human and MergeQueue commit p99 at most 3 s and 0 of them refused, and agent pushes over `INLINE_MAX_HOT` refused at once in the `ng` shape | Over 3 s, or any Human or MergeQueue push up to 1 MiB refused while placement fails |
| B0-32 | Burst of 500 agent pushes of 256 KiB each in 1 s with merges queued, admission on. The burst rides a hot group, so the row tests `INLINE_MAX_HOT`, which stages each 256 KiB pack before `AGENT_INLINE_ENTRY` is reached; a burst on a quiet group is covered by the budget's B0-33 evidence (fresh groups' first entries capped at 128 KiB, seventh pass). On the final Milestone 0 code 505 of 505 burst pushes committed within 5 s (client p99 1.40 s, stage p99 1.19 s), the burst's merges took 129 to 227 ms, each in the next entry (up to 2.30 s at a 256 KiB cap), queued agent bytes peaked at 119 KB, and there were 0 leader changes and 0 `suspect` [measured, `tests/m0/results/m0-wp9.md`, B0-32, eighth pass] | Every push acknowledged within 5 s or answered in an HTTP 200 with `ng <ref> agent admission full, retry after <n>s`, queued agent bytes under `AGENT_QUEUE_MAX`, every merge in one of the next 2 entries, zero leader changes, and zero `suspect` reports | Any lost acknowledgment, any other refusal shape, an object reset from memory, a merge delayed over 2 s, any leader change, or any `suspect` report |
| B0-33 | 5,000 agents on one group, 252 agent-class commands/s offered with caused commands classed, plus one user token writing 100 commands a minute, admission on. With `outcome` folded into `commit`, `AGENT_INLINE_ENTRY` on every group, and `hot` from the log, three far-flung runs (ATL leader, weur and apac followers, fresh groups) held their leader with 0 `suspect`, Human and MergeQueue p99 746 to 943 ms, 253 agent commands/s admitted, and the leader at about 500 req/s [measured, `tests/m0/results/m0-wp9.md`, B0-33, seventh pass]. With `commit_fail_min` added, a fourth run passed again at Human and MergeQueue p99 674 ms, 250 agent commands/s, and 0 leader changes; no window reached the 5% rate [measured, same, eighth pass]. Before the fold the leader served 750 to 950 req/s and 2 of 3 runs lost it by election (fifth pass). The zero-leader-change claim is measured for this mix, with a margin of 57 to 326 ms under 1 s, least in the first 30 s | Human and merge-queue commit p99 at most 1 s, at least 140 agent commands/s admitted, the user token admitted as `Agent`, every refusal in the git `ng` or REST 429 `retry-after` shape, zero leader changes, and zero `suspect` reports | Any leader change or `suspect` report, any merge-queue command waiting over 2 s, or under 100 agent commands/s admitted, which reopens ref lanes with a parent `Abort { txn }` record (`design/AGENT-LAYER.md`) |
| B0-34 | Git CPU for one 5 GB network under the journey 9 replay, with the tip moving at 10 merges a minute along real history, then one warm site killed, then a second network that turns hot with the API unreachable, then a hostile owner making three of its own networks hot beside it | k set in band from `NetworkLoad` and the hot pool, at most 8 per site, the hostile owner held to its per-owner pool budget, human push p99 at most 3 s beyond transfer, and clone time to first byte p99 at most 1 s throughout | k over 8, any Cloudflare API call, the monorepo's k reduced by another owner, or time to first byte p99 over 5 s |
| B0-35 | Lazy test merges, with the base moving 10 times a minute under 500 open PRs and 500 clients polling their PR every 5 s | Test-merge vCPU within `TEST_MERGE_BUDGET`, Actions dispatch p99 at most 10 s, and zero computes from list fields | Over budget, or dispatch p99 over 30 s |

**Flip condition.** B0-33's Fail value reopens ref lanes with a parent `Abort { txn }` record (`design/AGENT-LAYER.md`, "Alternatives considered", ref lanes). B0-29 and B0-34 decide the A1 verdict and the guessed git CPU constants (`design/AGENT-LAYER.md`, "Open questions and risks").

### M0-WP10. Agent refusals and provenance on the base

**Build.** Prototypes of `ActorKind`, `AgentActor`, the refusal list for `ActorKind::Agent` in `decide`, the `AgentsExclusive` check, `validate`'s `ProtectedHits`, `ChangedPaths`, and `Provenance` outputs, `Source`, `TrailerKey`, and the advertisement rules of `SESSION_REFS` (`design/DESIGN.md`, Shape, "Agent load and agent safety").

**Depends on.** M0-WP2, M0-WP6.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| B0-36 | Agent refusals, run once with a Graftyard-run token and once with a confined vendor App's token, used without the controller. Each tries approve, merge, ready-for-review, a bypass, dispatch, workflow re-run, a release, a label outside the allowlist, a `graftyard/` status, a push outside its prefix, a push with no live task and a fork push after its task is cancelled (Graftyard-run token only; a confined App is bound by `confine`), a REST ref write into another task's Exclusive scope under `AgentsExclusive`, and a change under the protected list, over git, REST, and GraphQL where GHES 3.22 has a surface for the action. The Graftyard-run token runs once with its shipped permissions and once on an installation widened to every permission, and a refusal from either counts. It also pushes an outsider commit that changes a workflow under its ref, pushes after an approval and then merges, and posts `graftyard/agent-gate`. Then a flagged App without `confine`, `Graftyard MCP`, and a user token with `X-Graftyard-Agent-Session` push into the same Exclusive scope, and a user token pushes a commit whose only trailer is `Co-authored-by` | 100 of 100 refused for each, for both agent kinds. The other three are never refused by the lease, and the `Co-authored-by` push stays Human | Any accepted, any of the other three refused by a lease, or the `Co-authored-by` push classed Agent |
| B0-37 | Provenance and hidden refs. 10,000 pushes with malformed trailers and notes, forged `Graftyard-Session` trailers, and writer pushes to `refs/graftyard/`, then v0, v1, v2, and `--mirror` fetches | Zero pushes fail on parsing, every forged trailer stays `Claimed`, every `refs/graftyard/` update not by `graftyard-workspace` is refused, and session refs appear only to a v2 `ref-prefix` that starts with it | Any push fails on parsing, a forged trailer links, or a session ref is advertised otherwise |

### M0-WP11. Platform questions answered by research and measurement

**Build.** Answer four questions from Cloudflare's documents, postmortems, and measurements in the Milestone 0 account, not from a written answer (`decisions/DECISIONS.md`, "Platform questions are answered by measurement"): (a) the internal route of a Durable Object call through a binding, (b) whether a Durable Object's storage followers span regions (`design/DESIGN.md`, "Build order from zero", the paragraph after the Milestone 0 table), (c) where a Durable Object restarts after a whole-region failure and whether a recovering old leader ever runs beside a new instance of itself, and (d) whether forks share objects with their source in Artifacts (`design/AGENT-LAYER.md`, "Artifacts' role", flip condition, recheck question 2). The probes are `QuorumProbe` and `RouteProbe` in `graft-m0-tools` and `tests/m0/run/wp11-quorum.ts`.

**Depends on.** None.

**Acceptance.** No gate row. Each answer is recorded with its evidence tag in `decisions/OPEN-QUESTIONS.md` and `design/DESIGN.md`, "Open questions and risks", before M1 starts. Done on 2026-10-27 (`tests/m0/results/wp11.md`): (a) a binding call carries no front-line proxy headers [measured], with the transport between colos undocumented; (b) no, the confirming quorum stays in the object's region [measured]; (c) a replacement starts only after 3 of 5 followers fence the old instance, which then cannot write, and after a whole-region loss the object waits for its region [doc, inference]; (d) no, each fork reports the source's full size [measured].

**Flip condition.** Flip condition 2 (`design/DESIGN.md`, "Synthesis decision"), which (b) did not meet: if the storage followers span at least two regions, candidate 1 with its `Alone` fix and `suspect` becomes the base for single-continent (`Na3`-class) and cost-sensitive installs, and `Tri` stays the base for installs that must survive a continent [inference, as the design tags it]. The Artifacts flip condition (`design/AGENT-LAYER.md`, "Artifacts' role") gives Artifacts a scratch tier for best-of-N attempts only when forks share objects and the per-repository cap rises above the install's largest network; (d) did not meet it.

### M0-WP12. Leases, the activity feed, and the core tier (layer rows)

**Build.** Prototypes of `Lease`, `LeaseHolder`, `ActorId`, `LeaseScope`, `PathGlob`, `RefPattern`, `LeaseMode`, `LeaseEnforce`, the lease constants, and the `LeaseAcquire`, `LeaseRenew`, and `LeaseRelease` events (`design/AGENT-LAYER.md`, Shape, first block). `ExtendLeases` on `LogEntry.extend` (`design/DESIGN.md`, Shape, "Chains"). The activity feed: `ActivityEvent`, `FeedCursor`, `FEED_WAIT_MAX`, `FEED_BATCH`, `FEED_BATCH_ALL`, `FEED_POLLS`, `FEED_HELD_MAX`, and `FEED_RING` (`design/AGENT-LAYER.md`, Shape, "Activity feed"). Only a following member holds polls: learners first, and followers only while no learner answers. Following means no gap, no `InstallSnapshot` catch-up, and an entry applied within `FEED_BATCH` of the leader's `at` [guess]. A member that stops following, or lags further, answers its held polls at once. A full member refuses, and the edge answers 429 with `Retry-After` at that first full reply; a member that is not serving sends it to up to two others, and if every one tried refused as lagging, to the least lagged only if that is a learner, else 429 with `Retry-After` [measured, `tests/m0/results/m0-wp12.md`, L0-12 `t6`]. Each member reports its peak held count for `place_learners`. The core tier rows that pushes, merges, minting, and lease checks read (`design/SHARED.md`, "Folds").

**Depends on.** M0-WP3, M0-WP9, M0-WP10.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| L0-01 | Gray enam (30% errors) during the A1 replay | Base and fork groups move within 20 s, 99% of agent pushes succeed within three attempts | Any group slower, or under 97% |
| L0-12 | Leases and the feed. The A1 replay with a lease per agent and leader kills, then a 30-minute stall with every member running and member calls blackholed, then an idle 60 s lease followed 10 minutes later by an overlapping Exclusive acquire, then 100 conflicting pushes and 100 conflicting REST ref writes into another task's Exclusive scope under `AgentsExclusive`, run once per actor kind: Graftyard-run App, confined vendor App, flagged App without `confine`, `Graftyard MCP`, and a human user token | No two holders hold overlapping Exclusive leases for longer than `DRIFT` (500 ms), and measured member clock offsets stay under 500 ms. Every lease live before the stall is live after it, with `expires` moved by the recorded stall. The late Exclusive acquire succeeds. 500 long-pollers see each event within FEED_BATCH plus 2 s (3 s) at p99 outside leader kills. 100 of 100 refused for each of the two `ActorKind::Agent` kinds, 0 of 100 for the other three | Any overlap over 500 ms, any lease expired by the stall, the late acquire refused, feed p99 over 2 s, or a refusal count off for any kind |
| L0-13 | Lease limits. Exclusive acquires of `**`, `*`, and `*/x`, one with no live task, and a second for one task; an App-token acquire with no session; 600 plan blocks from accounts without push access, then 600 from writers; a holder whose push access is removed; an App uninstalled while it holds leases and tasks; an Exclusive lease renewed for 5 h | 100 of 100 refused for each bad acquire. Zero plan leases from non-writers and at most 500 from writers. The removed holder's lease refuses nothing at the next `decide`. Uninstall releases every lease and cancels every task. The lease reads as Shared after 4 h | Any accepted, any plan lease from a non-writer, or an Exclusive lease past 4 h |
| L0-16 | Core tier alone. Pushes, merges, minting, and lease checks on the A1 replay with the metadata tier unloaded | Every check passes or refuses with zero metadata loads | Any metadata load on those paths |
| LF-12 | A1 replay on one `Tri` group with no learners, plus 5,000 held feed polls with `paths`, then again after `place_learners` runs. On the final rules, with the own-stall exclusion and the all-lagging `Retry-After` of `FEED_BATCH_ALL`, 4 runs gave 1 pass and 3 between the lines, with no fail, no step-down, and every other line passing in every phase. The misses trace to Durable Object storage confirm stalls on the leader or the near follower and to the far follower's round trip, as for B0-02 and B0-29 [measured, `tests/m0/results/m0-wp12.md`, LF-12, tenth pass]. The gate is one run, as B0-29's is | Commit p99 at most 600 ms once `place_learners` has run and at most 1 s before it, zero `overloaded` replies to AppendEntries, at most 1,000 polls held per member with the rest answered 429 with `Retry-After`, zero polls on the leader, and zero on followers after placement | Commit p99 over 1 s once `place_learners` has run or over 2 s before it, any `overloaded` replication reply, any member above 1,000 held, a refused poll without 429 and `Retry-After`, any poll on the leader, or any poll on a follower after placement |
| LF-13 | 1,000 polls held on a follower, then its AppendEntries dropped for 5 s so the next one leaves a gap, then an `InstallSnapshot` catch-up | Every held poll returns within 1 s with its cursor and no events, new polls are refused until it catches up, and no event is lost | Any poll held or accepted while it is behind, or any event skipped |

L0-12 also measures the clock-drift assumption behind `DRIFT` (`decisions/OPEN-QUESTIONS.md`, F3). LF-12 settles whether a member that holds 1,000 polls still answers AppendEntries (`decisions/OPEN-QUESTIONS.md`, A19).

### M0-WP13. Token ceilings, tasks, the agent gate, and the trust view (layer rows)

**Build.** Prototypes of `AgentTask`, `TaskKind`, `TaskApiState`, the task creation rules, the `graftyard_task` token request field, `AgentGate` in `Merge::decide`, and the `WriteScope` check on approve and merge (`design/DESIGN.md`, Shape, "Agent load and agent safety"; `design/AGENT-LAYER.md`, Shape, first block and "graft-mcp"). The trust view: `TrustView`, `Hidden`, `AuthorTrust`, and `AGENT_MEDIA_TYPE` (`design/AGENT-LAYER.md`, Shape, "Trust view").

**Depends on.** M0-WP10.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| L0-03 | Token ceilings with raw tokens, bypassing the router. A workspace token pushes to the base, to another task's prefix, and to `.github/workflows/`, a token minted with no live `AgentTask` pushes, a fork-mode token pushes to its fork after its task is cancelled, and the base token merges | 100 of 100 refused for each | Any accepted |
| L0-09 | Trust view. 200 injection payloads (HTML comments, zero-width, bidi, link titles, collapsed blocks) in bodies, comments, commit messages, and check-run text, plus a non-writer edit after assignment | 100% absent from `visible` and counted, `body` byte-identical, and the edited task paused | Any payload in `visible`, or the task runs on the edit |
| L0-14 | Task rules. With `agents_enabled` off, then on: Graftyard-run creates from an installation token, an `AgentApp`'s user token, an MCP token, a demoted user, and a read-only user; a controller create from an issue assigned to another bot; 101 open tasks for one requester; `PATCH` of a cancelled task to `in_progress`; a token request naming another App's task or a repository outside the token's list; a body that sets `prefix` | 422 for every create while off. When on, 100 of 100 refused for each bad case, the prefix always derived from `session`, and every create and cancel at the controller within 5 s, at no other App's webhook | Any accepted, or the delivery reaches any other App |
| L0-15 | Gate and read permissions. An MCP client with `WriteApproveMerge` approves an agent PR. Code Mode calls approve and merge through `codemode.request()` without that scope. A vendor's named requester approves its PR. Anonymous, `metadata: read`-only, and over-limit feed polls, and `/log` and prompt reads with `metadata: read` only | `AgentGate` stays unsatisfied in all three cases, and Code Mode is refused 100 of 100. Feed polls get 401, 403, or 429, the leader sees zero feed requests, and `/log` and prompts return 403 | Any gate satisfied, any Code Mode approve or merge, or any read served |

### M0-WP14. Sandbox containment and agent CI (layer rows)

**Build.** A controller prototype with the outbound router `outbound` and its `Allowed` arms (`GraftyardHost`, `ModelInference`, `Registry`), running a harness in a Sandbox (`design/AGENT-LAYER.md`, Shape, "Controller"). `AgentPolicy` with `CiMode::AutoReadOnly` and fork-PR semantics for agent pushes (Shape, "Agent actors in the layer"). Beside the rows, measure model tokens per harness, because "Open questions and risks" says Milestone 0 should.

**Depends on.** M0-WP9, M0-WP13.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| L0-02 | Sandbox containment, 100 attempts each to reach an unlisted host, read a token from env, files, or `/proc`, push to another repository on the host with a sandbox-supplied token, create a gist, call GraphQL, write a status or check run, send a model request with `mcp_servers` or a server-side tool, and fetch an unknown Go module path | 0 succeed | Any succeeds |
| L0-04 | Agent CI under `ci: AutoReadOnly` on the A1 replay | Agent-push workflows start within 30 s with a read-only token and no secrets, and jobs that reference secrets wait | Any secret reaches an agent-push job before `AgentGate` holds for its head |

### M0-WP15. MCP and unmodified agent clients (layer rows)

**Build.** A `graft-mcp` prototype with `Consent`, `McpGrant`, `McpToken`, `McpRefresh`, `WriteScope`, and `call` (`design/AGENT-LAYER.md`, Shape, "graft-mcp"; journey A7). The manifest flow and narrowed installation tokens that third-party agents use (journey A2). A request recorder that diffs responses with and without the agent layer deployed [inference: the byte diff row needs one].

**Depends on.** M0-WP13.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| L0-07 | A2 oracles. Claude Code GHES manifest flow, Cursor's GHES app, Codex's app template, local `github-mcp-server` default toolsets | Each installs, gets webhooks, opens a PR, reads `graftyard/overlap`, writes a check, and revokes, with zero Graftyard-specific client code. A push 1 s after revoke is refused 100 of 100 | Any needs a patch, or a revoked token pushes |
| L0-08 | MCP. Three 2026-07-28 clients and one 2025-11-25 client, CIMD, PKCE, consent, 30 calls each | 100% succeed, tool overhead p50 at most 50 ms, 100 of 100 wrong-audience tokens refused, a refreshed token keeps its scope 100 of 100, a replayed refresh revokes the grant, a framed or skipped consent issues no code, and the `ghr_` survives deleting the unscoped `ghu_` | Any wrong-audience token accepted, any refresh wider than its grant, or any code without consent |
| L0-11 | Byte diff. 1,000 REST and GraphQL requests from unmodified clients, with and without the agent layer deployed, and the same requests against a GHES 3.22 oracle | With and without the layer: identical except request ids and times. Against the oracle: every difference listed, and none in a field a client reads | Any other difference with and without the layer, or an unlisted oracle difference |

L0-08 settles whether the `ghr_` refresh token survives the delete (`decisions/OPEN-QUESTIONS.md`, A4).

### M0-WP16. Prediction and provenance (layer rows)

**Build.** A prototype of `GitCacheShard::predict` with `Verdict`, `PathSet`, `PREDICT_PAIRS`, and `PREDICT_CPU` (`design/AGENT-LAYER.md`, Shape, "Overlaps and prediction"). Provenance rows through a vault restore (`design/DESIGN.md`, "Install operations", Vault and Whole-install restore).

**Depends on.** M0-WP8, M0-WP9, M0-WP10.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| L0-05 | Prediction on 1,000 replayed branch pairs of a real monorepo | 100% of textual conflicts on current tips found, at most 5% false conflicts, push to verdict p95 at most 30 s, overlap check on the head SHA before a PR exists | Any missed conflict on current tips, or p95 over 120 s |
| L0-06 | Predictor under the A1 replay, then with its site leading | Second-site shard CPU at most 50%, and zero predictions while that site leads | Over 60%, or any prediction while it leads |
| L0-10 | Provenance. 10,000 replayed agent commits, then a vault restore | 100% resolve to a session or PR before and after | Any lost |

## Milestone 1: groups and git

Built (`design/DESIGN.md`, "Milestones 1 to 7"): Raft groups with `verify`, transfer, `suspect`, pre-vote, and `led_term` over cross-script bindings, placement records with `bootstrap()` and `MemberRegistry` rows, request ids, `PackStore` with `Promoted` and `Committed` markers, git cache shards, receive-pack and upload-pack, the clone cache, and minimal identity: the `Ids` group, `Names` login and repository-name claims, an owner's `Authz` record with admin grants and `RepoName`, and stored-token authentication. T0 surfaces `git-http` and `git-protocol-v2` [inference]. No REST.

### M1-WP1. Site scripts and placement

**Build.** `Site`, `SiteId`, `Region`, `Jurisdiction`, `SiteLink::Binding`, `Member` with `slot` and `generation` and names of the form `{chain}#{generation}#{kind}{slot}`, `MemberKind::Full` and `MemberKind::Witness`, `Placement` built only by `Placement::new` or `Placement::from_record`, `PlacementName`, `PlacementRecord`, `SystemChain`, `MIN_PAIR_RTT`, per-member `Rtt` rows, and the hint table compiled into callers (`design/DESIGN.md`, Shape, "Sites, not services, are the failure unit" and "Sites and placement"). The group lookup rule: the cached `GroupConfig`, then the bootstrap names built from the chain's `PlacementRecord`, then the chain's row in each record site's `MemberRegistry`, then `moving_from`. `MemberRegistry` and `RegistryRow`, written by a member before it stores its first entry (Shape, "Release group"). Each site script binds the member classes and buckets of every active, spare, and residency site with `script_name` (Shape, "Members reach each other through Durable Object bindings"). Placement failures retry with a new member generation ("Install operations", Placement failures). CLI: `graftyard init`, `graftyard sites add`, `graftyard sites add-spare`, `graftyard placement set`.

**Depends on.** M0 (every work package).

**Acceptance.** No gate row of its own. It carries the M1 gate (BM-1, owned by M1-WP8) with one site's member class failing mid-run, and the placement records that BF-10 and BF-11 (M1-WP7) route through.

### M1-WP2. Raft group core

**Build.** `ChainId`, `Term`, `Index`, `Digest`, `LogEntry`, `EntryBody`, `GroupConfig` (`Normal`, `Joint`), `MemberState`, `LEASE`, `ELECTION_AFTER`, `DRIFT`, `CALL_DEADLINE`, `COMMIT_RETRIES`, `RESET_QUARANTINE`, pre-vote, `commit`, `outcome`, `outcome_by_request`, `Committed`, `Admitted`, `COMMIT_HOLD`, `Ticket`, the `Command` trait, `Acked` with `QuorumProof`, `verify`, `fresh`, and `ReadView` (`design/DESIGN.md`, Shape, "Chains", "The write path", "The read path"). Bootstrap: a group's first entry is `Config(Normal(from_record(record)))` at term 1, index 1, and a member with no committed config serves no read and grants no vote. `ReplyMeta`, and `NotLeader` with the committed config (Shape, "Sites and placement" and "The write path"). `RequestId`, `LogEntry.requests` as `(RequestId, Range<u32>)`, the core-tier `request` table kept for `REQUEST_RETENTION`, `replay` of stored events for a duplicate, and tickets, refusals included, that resolve only when their entry commits. `CredentialShard`, `CredentialKey`, `credential()`, and `deps` reading `RepoSettings` from the fold. `COMMAND_MAX`, `Unsupported`, and `CallError` (Shape, "The write path"; `design/SHARED.md`, "Record formats and the two-release rule"). The invariants under "Invariants" hold: one value per index, one entry in flight per group, one writer per fold.

**Depends on.** M1-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-12 | Leader killed after quorum and before the reply, 10,000 times. The edge re-sends with the same `RequestId` | Original outcomes returned, zero double applies | Any double apply or changed outcome |
| BF-13 | Leader lost before commit, on entries with a `Stale` refusal caused by an earlier command in the same entry | Zero refusals returned from uncommitted entries (Porcupine) | Any refusal returned from an uncommitted entry |

It also carries BM-1's "Linearizability rerun at 24 hours" (the rerun of B0-11).

### M1-WP3. Leadership and health

**Build.** `transfer`, `suspect`, `SUSPECT`, `Health`, `STEP_DOWN`, `led_term` resumption, and transfer back to a group's `home` after it recovers (`design/DESIGN.md`, Shape, "Leadership"; "Commit and read paths", journey 5). A hedge on a missed receipt is a `suspect` report, never a command forwarded to another member ("Usage (caller's view)", call sites).

**Depends on.** M1-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-14 | Receipts that miss `CALL_DEADLINE` | Zero commands forwarded to a second member | Any command forwarded |

It also carries BM-1's run "with one site's member class failing mid-run".

### M1-WP4. Pack store and R2 keys

**Build.** `PackId`, `PackRef` (`Inline` up to 1 MiB, `Stored`), `DurablePack` built only by `PackStore::place`, `R2Key` (`Pack`, `Idx`, `Snapshot`, `Archive`, `CloneCache`, `Committed`), `Archived`, and `Promoted`, which packs an archived range's inline packs into one `DurablePack` before `Archived` commits. `PACK_COMMIT_WINDOW`, and the create-only `Committed` marker the leader puts after `Acked` for each stored pack. The site-script sweeper cron repairs a missing marker from the chain's `request` table, deletes an unmarked object older than 24 h, fills the third bucket from the markers, and aborts orphan multipart uploads (`design/DESIGN.md`, Shape, "Packs"; Stack, Cron Triggers; journey 1).

**Depends on.** M1-WP1, M1-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-19 | 1,000 failed placements, plus 1,000 commits whose marker write is killed | Failed ones deleted after 24 h, zero committed ones deleted, missing markers repaired | Any committed pack deleted |

It also carries BM-1's `git push --atomic` with one site failing.

### M1-WP5. Git cache shards and the git container image

**Build.** `GitCacheShard`, `shards_for`, `SHARDS` as set by B0-14, `HOT_POOL` counted in the start cap of `count + HOT_POOL` per site, rendezvous hashing of (network, shard), the LRU of bare repositories with reftable, the start-time region check, the `git` and `git_prev` image map, and the 6 h inactivity timeout renewed by alarm until 24 h after the last request (`design/DESIGN.md`, Shape, "Git cache shards (derived)"). `GitCacheShard::validate` and `GitCacheShard::upload_pack`. `IngestCursor` per (shard, network), and `rehydrate` from every stored pack with a marker plus the packs each member chain names after its last `Promoted` (Shape, "Packs" and "Git cache shards (derived)"). The git container image with upstream git.

**Depends on.** M1-WP4.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-18 | Archive a range of inline packs, kill all shards, rehydrate. A second shard behind its `IngestCursor` | `git fsck` clean, every ref clones, second shard serves the newer pushes | Any missing object |

It also carries BM-1's `git clone` and `fetch`.

### M1-WP6. The edge for git over HTTPS and the clone cache

**Build.** The edge's receive-pack: stage, validate, place, commit, and report, with `Push` (its `actor` a `Principal`), `RefUpdate`, `Atomicity`, and `report_status` taking `&Acked` (`design/DESIGN.md`, "Usage (caller's view)", call sites; `design/SHARED.md`, "Refs and pushes"). The edge mints one `RequestId` per client request, encodes each command, and refuses one over `COMMAND_MAX`. A push that would go over the cap stores its pack, and a push whose ref updates alone exceed it is refused whole with an `ng` line on every ref (SHARED, "Refs and pushes"). The edge renders `Unsupported` as 503 with `Retry-After` (Shape, "The write path"). Upload-pack through the shard, with the output teed to the client and R2. The clone cache keyed by `CloneKey` (Shape, "Packs"; "Dominant access patterns"). The plan cap refusal that names Enterprise or splitting the push below Enterprise, and SSH on Enterprise (journey 1). The `git-http` and `git-protocol-v2` surfaces at T0 (`design/SHARED.md`, "Surfaces, tiers, and oracles"). The 10,000-ref atomic push limit and its oracle (`design/SHARED.md`, "Known deviations from GitHub").

**Depends on.** M1-WP2, M1-WP3, M1-WP4, M1-WP5, M1-WP7.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-16 | Edge on N+1 with a new message format that is not enabled, plus one forced unknown variant | Nothing sent early. Unknown variant gives 503 with `Retry-After` and no state change | Any other rendering or any state change |
| BF-17 | Atomic push of 10,000 refs with a 1 MiB pack. A push whose updates exceed `COMMAND_MAX` | First: pack stored and push commits. Second: refused whole, no entry | Partial push, or an entry over 1.5 MB |

It also carries all of BM-1's git commands.

### M1-WP7. Minimal identity and owner placement records

**Build.** The minimal identity that real code needs, with no test-only resolver (`decisions/DECISIONS.md`, "Milestone 1 builds minimal identity"). The `Ids` group with `DbId`, its cell, block, and offset layout, and `EntryBody::IdBlock` (`design/DESIGN.md`, Shape, "Chains"). `NameShard`, `name_shard`, `account_shard`, and the `NameKey` and `NamesEvent` variants for login claims, `Account` rows, owner `Placement` records, and `RepoOwner` (Shape, "Identity" and "Sites and placement"). `Login`, `LoginKey`, `RepoName`, `RepoNameKey`, `RESERVED`, and `RepoEntry` (`design/SHARED.md`, "Names"). An owner's `Authz` record with admin grants and `RepoName` entries (SHARED, "Event types"). Stored-token authentication: `TokenShard`, `token_shard`, `TokensEvent::Stored` and `Revoked`, `OpaqueToken`, `Credential`, `Principal`, `READ_CACHE`, the per-isolate cache, `IDENTITY_BUDGET_PER_ISOLATE`, and the leader's read of each command's `CredentialShard` once per entry (Shape, "Identity" and "The write path"; SHARED, "Identity and tokens"). Owner placement moves through the Workflows script, which bumps the record's `generation`, keeps `moving_from`, and moves each group through `Joint` (Shape, "Sites and placement"). The system chains' records live in the signed `Genesis`, and the `Release` group is M5-WP1, so this work package builds genesis acceptance: `graftyard genesis`, `graftyard sign`, and `graftyard install --genesis` into a `Release` group that accepts only a `Genesis` matching the trust anchor [inference] (`decisions/OPEN-QUESTIONS.md`, R13). How the gate's accounts, repositories, and tokens are created before REST and sign-in exist is open (`decisions/OPEN-QUESTIONS.md`, R14).

**Depends on.** M1-WP1, M1-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-10 | 1,000 chain creations with two edges at once, plus 100 during an owner record change | One group per chain. Members without a committed config serve no read and grant no vote | Any chain with two groups, or any read or vote served without a config |
| BF-11 | Edges with cold caches after a member retry, `ReplaceMember`, and an owner move | 100% of requests find the group | Any request that finds no group |
| BF-15 | Revoke a stored token, and separately suspend an account, each with 1,000 of its commands queued | 100% of commands decided afterwards get 401 | Any accepted |

It also carries BM-1's authenticated pushes [inference].

### M1-WP8. Milestone 1 gate

**Build.** Nothing new. Run the gate on M1-WP1 to M1-WP7.

**Depends on.** M1-WP1, M1-WP2, M1-WP3, M1-WP4, M1-WP5, M1-WP6, M1-WP7.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| BM-1 | The `git` conformance suite and `git clone` / `fetch` / `push --atomic` from unmodified git 2.4x pass with one site's member class failing mid-run. Linearizability rerun at 24 hours |

## Milestone 2: folds, issues, pull requests, and merge

Built (`design/DESIGN.md`, "Milestones 1 to 7"): both fold tiers in member SQLite with follower snapshots, the issue, pull request, review, check-run, and merge-queue model from `design/SHARED.md`, "Issues and pull requests", with `parent_tip` and `QueueMerge` checked in `decide`, leader cursors and Queues with the `PrSync` and `MergeQueue` consumers, `TestMerged`, and the blob store for LFS objects and release assets, plus the T0 repository surfaces [inference].

### M2-WP1. Folds in member SQLite

**Build.** `Tier`, `tier_of`, `apply`, and `fold_audit` (`design/SHARED.md`, "Folds"). Both tiers in each member's SQLite, with FTS5 (`design/DESIGN.md`, Stack, Durable Objects; Shape, "Reused with the additions below"). Snapshots per tier under `R2Key::Snapshot`, with one snapshot per tier at or below every index inside `RESTORE_WINDOW`. One Full follower writes each snapshot, chosen by `to / SNAPSHOT_EVERY` modulo the Full followers, every `SNAPSHOT_EVERY` entries or `SNAPSHOT_BYTES` of log and at least daily, at one index without pausing `apply` through `snap_undo`, as `SnapshotFrame` parts ending in a `SnapshotTrailer` in two buckets (Shape, "Packs"). The site-script fold audit cron (Stack, Cron Triggers).

**Depends on.** M1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-20 | Snapshot of a 4 GB [guess] metadata tier under 300 commands/s | Apply p99 within 10% [guess]. Snapshot digest equals a refold at `to`. No request-limit errors | Apply p99 more than 10% slower, a digest mismatch, or any request-limit error |

It also carries BM-2's journey 3 lists.

### M2-WP2. Issues, pull requests, reviews, and checks

**Build.** The model in `design/SHARED.md`, "Issues and pull requests": `IssueNumber`, `IssueKind`, `HeadSpec`, `PrState`, `Mergeability`, `MergeMethod`, `record_check`, and `record_status`, with the stored formats `IssueCapture`, `IssueChanged`, `CommentEvent`, `ReviewEvent`, `LabelEvent`, `BranchRule`, `RepoSettings`, and `CheckRunUpdate`. The `RepoEvent` variants they write, including `CheckRun`, `CheckSuite`, and `Status` in the metadata tier, pruned after `RepoSettings.check_retention_days`, and `RequiredCheck` in the core tier with an optional conclusion (`design/SHARED.md`, "Event types"). `GitCacheShard::merge` with the tree-diff check (`design/DESIGN.md`, Shape, "Git cache shards (derived)"). The `rest:issues`, `rest:pulls`, `rest:checks`, and `rest:commits` surfaces and the GraphQL resolvers that `gh pr` and `gh issue` use (`design/SHARED.md`, "Surfaces, tiers, and oracles").

**Depends on.** M2-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-22 | 10,000 non-required check runs and statuses on one repository, then entry time advanced past `check_retention_days` | `gh pr checks` and `GET /commits/{sha}/statuses` read them; core tier bytes unchanged; rows pruned on every member with equal `fold_digest` | A non-required row in the core tier, or members disagreeing |

It also carries BM-2's journeys 2 and 3.

### M2-WP3. Merge and the merge queue

**Build.** `Merge`, `MergePlan`, `check_protection`, and `MergeError` (`design/SHARED.md`, "Issues and pull requests"). `MergeQueueEntry` with its queue ref, the `parent_tip` rule, `QueueEvent` (`Enqueued`, `GroupBuilt`, `Ejected`, `Landed`), `EjectReason`, `QueueOp`, and `QueueMerge`, whose `decide` lands a whole group or nothing, so queue tips only fast-forward. The `MergeQueue` consumer builds groups, creates the `gh-readonly-queue/*` refs, reads `RequiredCheck` on them, ejects failures, and submits `QueueMerge` (`design/DESIGN.md`, Shape, "Merge queue"; "Invariants"; `design/SHARED.md`, "Event fan-out").

**Depends on.** M2-WP1, M2-WP2, M2-WP5.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-1 | One entry holding a 5-PR merge-queue group plus a `PrSync` batch | It commits on all three sites | Any follower refuses it |
| BF-21 | A group of 5 PRs under `HeadGreen`, the 3rd failing its required check on its queue ref, 100 runs, with the base leader killed mid-landing in 10 | The base moves only by `QueueMerge` entries that land a whole group; the failing PR is ejected and the other 4 land after a rebuild; every base move is a fast-forward | Any partial group, any non-fast-forward base move, or a queued PR lost |
| BF-23 | A required check passes, then is re-requested before the merge | The merge is refused until the new run concludes, 100 of 100 | A merge on the stale success |

### M2-WP4. The wire layer, the blob store, and the other T0 repository surfaces

**Build.** The cross-cutting conventions in one shared wire layer (`design/SHARED.md`, "Cross-cutting conventions"), the hostnames and endpoints rules ("Hostnames and endpoints"), and client and object ids ("Client ids and object ids"). The GraphQL schema generated from GHES 3.22 SDL. The blob store: `R2Key::Blob { repo, oid }` placed in two buckets as `DurableBlob`, `BlobRef`, the `Release`, `ReleaseAsset`, and `LfsObject` events, and derived `R2Key::ArchiveCache` objects beside the clone cache (`design/DESIGN.md`, Shape, "Packs"; `design/SHARED.md`, "Event types"). The T0 surfaces that `design/DESIGN.md`, "Milestones 1 to 7", places in Milestone 2 [inference]: `meta-endpoint`, `rest:meta`, `rest:rate-limit`, `rest:repos`, `rest:branches`, `rest:git`, `graphql`, `rest:releases`, `uploads-release-assets`, `codeload-archives`, `raw-content`, and `lfs` over HTTPS. Its SSH part is M6-WP2.

**Depends on.** M2-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-25 | `git lfs push` and `gh release upload`, then one site bucket made unreadable | `verify` and the 201 come only after two buckets hold the blob; fetch and download succeed 100 of 100 | Any acknowledged blob unreadable |
| BF-26 | All `ArchiveCache` objects deleted, then `gh release download --archive=tar.gz` | The bytes match the earlier download | Any difference or error |

Each surface's oracle in `design/SHARED.md`, "Surfaces, tiers, and oracles", passes [inference: the design names the oracles but no gate row for each]. It also carries BM-2's `gh` runs.

### M2-WP5. Cursors, Queues, and the `PrSync` consumer

**Build.** Leader cursors and alarms, one queue per site, consumer Workers, and `Consumer` (`design/SHARED.md`, "Event fan-out"; `design/DESIGN.md`, "Dominant access patterns"; Scale, fan-out). `PrCapture`, and the `PrSync` consumer that moves `refs/pull/N/head`, has the git cache compute the test merge, and moves `refs/pull/N/merge` by `TestMerged` (`design/SHARED.md`, "Issues and pull requests" and "Known deviations from GitHub"). The budget on test merges is AM1-WP1.

**Depends on.** M2-WP1, M2-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-24 | A head push, then `git ls-remote origin refs/pull/N/merge`; a conflicting push | The `TestMerged` commit within 10 s with a matching `mergeable`; on a conflict the ref is deleted and `mergeable: false` | A stale ref |

It also carries the `PrSync` batch in BF-1 (M2-WP3).

### M2-WP6. Milestone 2 gate

**Build.** Nothing new.

**Depends on.** M2-WP1, M2-WP2, M2-WP3, M2-WP4, M2-WP5.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| BM-2 | `gh pr` and `gh issue` journeys 2 and 3 pass, the merge p99 on a freshly elected leader is at most 2 s, and a queue entry with a stale `parent_tip` is refused 100 times out of 100 |

## Milestone 3: identity and authorization

Built (`design/DESIGN.md`, "Milestones 1 to 7"): the rest of `Tokens`, `Names`, `Authz`, and `Identity`: fine-grained and paired tokens, OAuth web and device flows with short-lived codes, Apps and OAuth apps, stateless installation tokens signed in `graft-ops` with `jti` revocation, SSO through `InstanceAuth`, collaborators and invitations, transfers, `InstallSnapshot`, and learners for hot groups, plus the T0 identity surfaces [inference]. Milestone 1 built the minimal identity (M1-WP7).

### M3-WP1. App and OAuth app records and their names

**Build.** `ClientId`, `SecretHash`, `AppRecord`, `AppKey`, and `OAuthAppRecord` on the owner's `Authz` chain, as `AuthzEvent::App` and `AuthzEvent::OAuthApp` (`design/SHARED.md`, "Identity and tokens"). The `NameKey` variants `AppSlug`, `Client`, `App`, and `Installation`, and the `NamesEvent` variants `AppSlug`, `AppRegistered`, `ClientRegistered`, `InstallationRegistered`, `Shortcode`, `EmailClaimed`, `FormerName`, and `Retired`, which hold only routes to an owner. Registration claims `AppSlug` first, the commit point, and `Audit` finishes a registration nobody finished (`design/DESIGN.md`, Shape, "Identity").

**Depends on.** M2.

**Acceptance.** No gate row of its own. It carries the App half of BF-32 (M3-WP4) and BM-3's journey 4 name reads.

### M3-WP2. Token kinds and short-lived codes

**Build.** Every `TokenKind`, `fingerprint_shard`, `jti_shard`, and the rest of `TokensEvent`: the optional `Stored` fields (`app`, `client`, `permissions`, `resource_owner`, `repos`, and `paired`), the `ghr_` redrawn onto its `ghu_`'s shard, `SshKey`, `SshKeyRemoved`, `JtiRevoked`, and `VerifyingKey`, and the `Code`, `CodeApproved`, and `CodeUsed` rows with `CodeKind`, which lapse at `expires` (`design/DESIGN.md`, Shape, "Identity"; `design/SHARED.md`, "Identity and tokens"). The `tokens` surface at T0.

**Depends on.** M3-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-2 | Revoke a token while 100 of its pushes wait in admission | None is acknowledged at an index after the revocation's | Any one is |
| BF-31 | A `github_pat_` scoped to one repository with `contents: read`; a `ghu_` and `ghr_` refresh; `DELETE /applications/{client_id}/token` | The PAT is refused outside its scope; revoking either paired token refuses the other; the DELETE revokes | Any use after revocation |

### M3-WP3. Authz, Identity, and sign-in

**Build.** `AuthzEvent` and `IdentityEvent` on their chains, `authorize`, `authorize_write`, and `Decision` (`design/SHARED.md`, "Event types" and "Identity and tokens"). The `Account` row with `suspended` and `sessions_not_before`, read on every write (`design/DESIGN.md`, Shape, "Identity"). The edge's `Identity` write right after a new credential's `Tokens` commit, with `Audit` finishing only what nobody finished. OAuth web and device flows with the `FirstPartyClient` seeds. `InstanceAuth` on the install's own `Authz` chain, written only by a `SiteAdmin`. Outside collaborators as `Collaborator` entries and org members and teams as `Grant` entries, never both, and `InvitationEvent` with accept and decline. Repository transfer and an org's enterprise change (SHARED, "Multi-chain operations"), with the owner placement move of M1-WP7. The T0 surfaces that `design/DESIGN.md`, "Milestones 1 to 7", places in Milestone 3 [inference]: `rest:orgs`, `rest:teams`, `rest:users`, `rest:collaborators`, `rest:deploy-keys` over HTTPS, `oauth-web-flow`, and `oauth-device-flow`. The SSH half of `rest:deploy-keys` is M6-WP2. CLI: `graftyard admin bootstrap` ("Install operations", First admin).

**Depends on.** M3-WP1, M3-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-3 | Suspend a user, then push with their PAT; separately, add one SSH key from two edges at once | Every push refused; the key gets one owner | Any push accepted, or two owners |
| BF-30 | Transfer across enterprises and placements, plus an org changing enterprise | Both routes reach the group throughout. `deps` names the new owner after `Settings` | Any lost route, or a stale owner after `Settings` |
| BF-33 | `gh auth login` device flow with the code shard's leader killed mid-flow, 100 runs; replay of a used code; a code older than 900 s | Each login completes or restarts cleanly; replay and the expired code fail 100 of 100 | A token from a reused or expired code |
| BF-34 | `TestAccGithubRepositoryCollaborator`; an org invitation accepted and one declined; a `Collaborator` write for an org member | Accepting grants in the same entry; declining grants nothing; the member write is refused | Any overlap or lost grant |
| BF-35 | A new SSH key and PAT, listed at once with no `Audit` consumer deployed | `gh ssh-key list` and `GET /user/keys` show them 100 of 100 | Any listing that misses them |
| BF-36 | A site admin switches `InstanceAuth` from Builtin to OIDC | The next sign-in uses OIDC; a non-admin write of it is refused | Any other result |

The SSH push of the suspended user is BF-52 (M6-WP2).

### M3-WP4. Stateless installation tokens in `graft-ops`

**Build.** `InstallationClaims` and `ghs_` tokens as compact JWS with `kid` (`design/SHARED.md`, "Identity and tokens"). Minting in `graft-ops` with a signing key wrapped by a Secrets Store key and cached for at most `SIGNING_KEY_CACHE`, `TokensEvent::VerifyingKey` on `TokenShard(0)`, and `DELETE /installation/token` committing `TokensEvent::JtiRevoked` to `jti_shard` (`design/DESIGN.md`, Shape, "Identity"; journey 4). App JWTs verified against `AppRecord.keys`. The `rest:apps` and `github-apps` surfaces at T0. The edge mints through a `fetch()`-based binding to `graft-ops`. The `OpsPointer` override on that binding arrives with M5-WP2 [inference].

**Depends on.** M3-WP1, M3-WP2, M3-WP3.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-27 | Revoke a `jti` with 1,000 commands under its installation token queued | 100% of commands decided afterwards get 401 | Any accepted |
| BF-32 | `@octokit/app` installation-token flow with a newly registered App; OAuth web flow with a registered OAuth app and a wrong secret | The JWT verifies against `AppRecord.keys`; the wrong secret gets 401; a dump of every `Identity` chain shows no client secret | Any other result |

It also carries BM-3's revoked stateless installation token.

### M3-WP5. Learners and snapshot install

**Build.** `MemberKind::Learner` with its own name slot, `LEARNER_AT`, `MAX_LEARNERS`, `place_learners` as a daily `graft-ops` cron over Analytics Engine counters, and learner read-index (`design/DESIGN.md`, Shape, "Sites and placement" and "The read path"; "Dominant access patterns", hot group read). `InstallSnapshot` for a member whose next entry the leader has archived, which new learners and `ReplaceMember` members also use (Shape, "Packs").

**Depends on.** M1-WP2, M2-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-28 | A learner on its voter's site, and two learners on one site | Distinct objects | A name collision |
| BF-29 | Follower offline past the archive boundary, then rejoins | Catches up by `InstallSnapshot` with matching fold digests | Stuck or divergent |

It also carries BM-3's "Learners rerun the Milestone 0 learner gate" (the rerun of B0-18).

### M3-WP6. Milestone 3 gate

**Build.** Nothing new.

**Depends on.** M3-WP1, M3-WP2, M3-WP3, M3-WP4, M3-WP5.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| BM-3 | Journey 4 passes over HTTPS, with revoked tokens, a revoked stateless installation token included, refused on the next write 100 times out of 100. Learners rerun the Milestone 0 learner gate |

## AM1: base agent changes, leases, and tasks (built with M2 and M3)

Built (`design/AGENT-LAYER.md`, "Milestones"): base agent changes, `AgentApp`, `AgentMeta`, `AgentTask`, `AgentGate`, `PathSet`, the trust view, leases, the activity feed, external tasks, and the task read endpoints.

### AM1-WP1. Base agent load changes

**Build.** Admission inside group commit: `AdmitClass` lanes, `AGENT_SHARE`, `AGENT_FLOOR`, `DEMOTE_PER_MIN`, `AGENT_WAIT`, `AGENT_QUEUE_MAX`, and the git `ng` and REST 429 refusal shapes. `INLINE_MAX_HOT`, `AGENT_INLINE_ENTRY`, and `HOT_PUSHES`. `LOG_MAX_BYTES` archiving to `R2Key::Archive` in two buckets. `NetworkLoad`, `fanout`, `FANOUT_MAX`, and the per-owner share of `HOT_POOL`. `TestMerge` and `TEST_MERGE_BUDGET`, with `mergeable` null past the budget. `R2Key::InlineIdx`, and hot clone-cache entries in both warm sites' buckets (`design/DESIGN.md`, Shape, "Agent load and agent safety", "Packs", "Git cache shards (derived)"; journey 9).

**Depends on.** M1, M2-WP3.

**Acceptance.** No gate row of its own. It carries LM-1's rerun of B0-29 to B0-35 on real code.

### AM1-WP2. Agent actors and refusals in `decide`

**Build.** `ActorKind`, `AgentActor`, `AgentApp`, `AgentKind`, `AgentMeta`, `AgentPolicy`, `CiMode`, `PROTECTED_PATHS`, `ProtectedHits`, `ChangedPaths`, `PATHS_MAX`, `Provenance`, `Source`, `TrailerKey`, `SESSION_REFS`, `AgentGate` in `Merge::decide`, `AuthzEvent::AgentApp`, and `OrgPolicy.agents_enabled` and `agents_workspace` (`design/DESIGN.md`, Shape, "Agent load and agent safety"; `design/AGENT-LAYER.md`, Shape, "Agent actors in the layer"). `AgentApp` rows on `Authz(Genesis.account)`, which every App command lists in `deps()`, and the settings endpoints for `AgentApp`, `AgentPolicy`, `agents_enabled`, and the workspace mode (`design/AGENT-LAYER.md`, "API reference"). `Push` gains `provenance`, `protected`, and `changed` (`design/SHARED.md`, "Refs and pushes").

**Depends on.** AM1-WP1, M3-WP3, M3-WP4.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-2 | Unflag an `AgentApp` (`DELETE /agents/apps/{id}`), then push under its old `confine` prefix | The next `decide` after the acknowledgment no longer classes it `ActorKind::Agent` | A decision after the acknowledgment uses the old row |

It also carries LM-1's rerun of B0-36 and B0-37 on real code.

### AM1-WP3. External tasks and the task read endpoints

**Build.** `AgentTask`, `TaskKind`, `TaskApiState`, `SessionId`, `TASKS_OPEN_PER_REQUESTER`, `TASKS_OPEN_PER_REPO`, `TASK_IDLE`, and the `TaskCreated` and `TaskUpdated` events (`design/DESIGN.md`, Shape, "Agent load and agent safety"; `design/AGENT-LAYER.md`, Shape, "Agent-layer entries"). The task read endpoints and `External` task creation under `/api/graftyard/v1/repos/{owner}/{repo}/tasks`, with the bodies and errors in `design/AGENT-LAYER.md`, "API reference". The `graftyard_task` token request field and the `task` claim, valid on a token of the task's App or `graftyard-workspace` for the task's repository or its workspace fork (`design/SHARED.md`, "Identity and tokens"). An uninstalled App's tasks read as cancelled through the same check that releases its leases, with no sweep. Graftyard-run creation is AM3-WP3.

**Depends on.** AM1-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-1 | Uninstall an App with 100 open tasks across 10 repositories | Every task reads `cancelled` at the next read with no sweep command, and its next command records it | Any task reads live |

It also carries LM-1's "base journeys pass unchanged".

### AM1-WP4. Leases, `PathSet`, and the activity feed

**Build.** `Lease`, `LeaseHolder`, `ActorId`, `LeaseScope`, `PathGlob` and `RefPattern` with their glob dialect and overlap rule, `LeaseMode`, `LeaseEnforce`, `LEASE_TTL`, `EXCLUSIVE_MAX`, `LEASES_PER_HOLDER`, `LEASES_PER_APP`, `PLAN_LEASES_PER_REPO`, `LEASES_PER_REPO`, and the lease events (`design/AGENT-LAYER.md`, Shape, first block and "Leases"). `graftyard-plan` blocks parsed by the grammar in `design/AGENT-LAYER.md`, "API reference". `ExtendLeases` on `LogEntry.extend` (`design/DESIGN.md`, Shape, "Chains"). `PathSet` (Shape, "Overlaps and prediction"). The activity feed with `ActivityEvent`, `PrAction`, `LeaseChange`, `FeedCursor` and its encoding, `wait` in whole seconds clamped at 25, `paths` as a repeated parameter, and the `FEED_*` constants, `FEED_HELD_MAX` included. Learners serve it, followers serve it only while no learner answers, and only a following member holds polls (Shape, "Activity feed"). Each member's peak held count feeds `place_learners` (M3-WP5).

**Depends on.** AM1-WP3.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-3 | Feed cursor from another group, `wait=40`, 33 `paths` | 410 with `oldest_cursor`, `wait` clamped to 25, 422 for the 33 paths | Any other status, or `wait` above 25 |
| LF-5 | `graftyard-plan` blocks: a valid one, a repeated key, a bad glob, a second block, and a block over 4 KiB | One lease from the valid block. None from the others, and the body unchanged | A lease from an invalid block, no lease from the valid one, or a changed body |

It also carries LM-1's rerun of L0-12, L0-13, and L0-16, and reruns LF-12 and LF-13 on real code [inference].

### AM1-WP5. The trust view

**Build.** `AGENT_MEDIA_TYPE`, `TrustView`, `Hidden`, `AuthorTrust`, `HIDDEN_CHARS`, and the resources that get `graftyard_trust`, rendered by the edge beside `body` and `body_html`, which stay byte-identical to GHES (`design/AGENT-LAYER.md`, Shape, "Trust view"; journey A4).

**Depends on.** M2-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-4 | Trust view: one payload per `HIDDEN_CHARS` range in each `graftyard_trust` resource | 100% absent from `visible` and counted | Any present |

It also carries LM-1's byte diff (the rerun of L0-11).

### AM1-WP6. AM1 gate

**Build.** Nothing new.

**Depends on.** AM1-WP1, AM1-WP2, AM1-WP3, AM1-WP4, AM1-WP5, M3-WP6.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| LM-1 | Base journeys pass unchanged, the base's agent rows rerun on real code, the base's 30-day hot-group storage row passes on a group seeded with the 4 GB tier, the leases and feed, lease limits, gate and read permissions, and core-tier rows pass, and the byte diff passes. Fleets of A1's size on one repository are enabled only after metadata child groups (base Milestone 7) |

The base's agent rows are B0-29 to B0-37 and the stateless revocation in BM-3 (`design/AGENT-LAYER.md`, "Milestone 0 rows"). The 30-day hot-group storage row is B0-30. The layer rows named are L0-12, L0-13, L0-15, L0-16, and L0-11. Fleets of A1's size on one repository stay off until M7-WP4 passes.

## Milestone 4: events, webhooks, and Actions

Built (`design/DESIGN.md`, "Milestones 1 to 7"): webhooks and the other Queues consumers with per-owner `HookSequencer` delivery stores and hook records, user records, the watcher index, the `Inbox`, and `SearchShard` search, the Actions engine with run, job, runner, and session events, `RunnerBroker`, `LogRelay`, and sealed secrets and variables, Sandboxes runners in the runner account started by `graft-runners`, and the web app, plus the T0 Actions and webhook surfaces [inference]. Cursors and Queues are M2-WP5.

### M4-WP1. Webhooks and the remaining consumers

**Build.** `SUBSCRIPTIONS`, `HookDelivery` with one automatic attempt and the `retry` opt-in, `HookCursor`, whose `read` passes an entry only once its `HookDelivery` rows are recorded, `HookRecord` on `RepoEvent::Hook`, `HOOK_OWNER_CONCURRENCY`, and the `Webhooks`, `Audit`, and `Search` consumers on the M2-WP5 pipeline (`design/SHARED.md`, "Event types" and "Event fan-out"; `design/DESIGN.md`, "Dominant access patterns", Webhook). `HookSequencer`, one per hook owner on its home site, as the delivery store: one queue per (chain, hook) with one attempt in flight, failing hooks held to half the owner's slots, ping, the deliveries endpoints, manual redelivery, and a rebuild from `replay_from` after a site loss (`design/DESIGN.md`, Shape, "Identity"; "Failure modes", `event pipeline`). Webhook signing unwraps `HookRecord.secret` through `graft-ops` (`design/SHARED.md`, "Actions", Secrets and variables). The `webhooks` surface at T0, including `merge_group`, `pull_request.enqueued`, and `pull_request.dequeued` from `QueueEvent`. Audit entries stream through Basin Pipelines (`design/DESIGN.md`, Stack). The first-party-only task delivery to the controller rides this pipeline and is AM3-WP3.

**Depends on.** M2-WP5, M3.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-57 | One hook timing out at 10 s beside 49 healthy hooks of the same owner, 10 events/s | Healthy hooks' first-attempt p99 at most 2 s; the slow hook gets its events in chain order; Webhooks cursor lag at most 5 s | Healthy p99 over 10 s, an out-of-order delivery to one hook, or cursor lag that grows with the slow hook |
| BF-58 | Kill the site holding a hook owner's `HookSequencer` with deliveries pending | Every event in the range reaches every hook at least once, with unchanged GUIDs | Any event never delivered, or a redelivery under a new GUID |

It also carries BM-4's job events and the deliveries in BF-37 (M4-WP3).

### M4-WP2. User records, the watcher index, the `Inbox`, and search

**Build.** `UserRecord` as `IdentityEvent::Record`, the `Counts` consumer and the watcher index it writes to each repository's metadata tier (`watcher` and `thread_sub` rows), which `/stargazers`, `/subscribers`, and `Notify` read, the `Notify` consumer, and `Inbox { account, site }`, one derived object per account, with its daily snapshot, its `sources` table, and the rebuild rule with `INBOX_WINDOW` (`design/DESIGN.md`, Shape, "Identity"). `SearchShard` and `Domain`: FTS5 tables in 16 shards per residency domain, on the domain's first Full site under its DO jurisdiction, rebuilt under the next `generation` from the metadata tiers that `MemberRegistry` lists, with `incomplete_results` past the `rest:search` deadline (Shape, "Identity"; `design/SHARED.md`, `rest:search` and "Public service concerns", Residency). The `rest:activity` surface at T1 (`design/SHARED.md`, "Surfaces, tiers, and oracles").

**Depends on.** M4-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-5 | Star a repository and comment on a watched issue, then delete and rebuild the watcher's `Inbox` | `stargazers_count` and `/notifications` show both within 10 s, and the rebuilt `Inbox` keeps read state | Lag over 60 s, or read state lost |
| BF-67 | Delete an `Inbox` and rebuild it. Also 10,000 stars, then a refold | Read and done state kept, nothing from `sources` lost. `/stargazers` lists all 10,000 | Any lost read or done state, a lost event from a `sources` chain, or a missing star |
| BF-68 | EU-resident org's search documents. Kill one `SearchShard` | Documents only in the `eu`-jurisdiction shards. `incomplete_results` until the rebuild, then full results | A document outside EU, a partial result without `incomplete_results`, or results wrong after the rebuild |

BF-68 assumes FTS5 inside Durable Object SQLite, which Milestone 0 has not confirmed (`decisions/OPEN-QUESTIONS.md`, C15).

### M4-WP3. The Actions engine

**Build.** `JobState`, `JobEvent` with `QueueExpired`, `transition`, `RunStatus`, `derive_run_status`, `WorkflowRun`, `JobSpec`, `RunnerScope`, `RunnerRecord`, `RunnerSessionId`, `Broker`, and label matching from each shard's queued summary and counted hints (`design/SHARED.md`, "Actions runs and jobs"). The "Workflow run" operation: `RunCreated` is the commit point, `RunOwner` the finisher keyed by (job, attempt), and `Cancel` the abort path (`design/SHARED.md`, "Multi-chain operations"). The workflow engine and the Rust port of `WorkflowParser` and `MatrixBuilder`, with `on: merge_group` runs on queue refs (`design/SHARED.md`, "Actions"). `ChainId::Actions` shards, `ActionsEvent` (`RunCreated`, `RunStatus`, `Job`, `Runner`, and `Session`), `actions_shard`, job leases extended only by `ExtendLeases`, and the leader's alarm that commits `LeaseExpired` or `QueueExpired` (`design/DESIGN.md`, Shape, "Identity"). `RunnerBroker { owner, scope }` with `BROKER_HELD_MAX` and the 503 past it, and `LogRelay { job }`, which `FeedStreamUrl` names, both on their shard's leader site (Shape, "Identity"; `design/SHARED.md`, "Actions", Logs). Secrets and variables: `SealKey`, `Wrapped`, `ActionsName`, `SecretScope`, `SecretRecord`, `VariableRecord`, `RepoEvent::ActionsSecret` and `ActionsVariable`, the org rows on the owner's `Authz` chain, job-message assembly at `acquirejob` through `graft-ops` with `SecretsWrapKey`, and the key rotations (`design/SHARED.md`, "Actions", Secrets and variables; `design/DESIGN.md`, Shape, `SecretName`). The T0 surfaces `rest:actions`, `actions-runner-protocol`, `actions-workflow-syntax`, `actions-cache`, `actions-artifacts`, `actions-oidc`, and `actions-logs-results` [inference, as `design/DESIGN.md`, "Milestones 1 to 7", places them]. CLI: `graftyard actions import`.

**Depends on.** M4-WP1, M3-WP4.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-4 | Kill a runner mid-job, then leave its Actions group idle | The job fails within 10 minutes of its last `renewjob` | It is still running at 11 minutes |
| BF-37 | `on: merge_group` workflow, and go-playground/webhooks parsing `merge_group`, `pull_request.enqueued`, and `pull_request.dequeued` | A run on each built queue ref; every queue ref deleted within 5 min of landing or ejection [guess bound] | A missing delivery, or a queue ref left over |
| BF-54 | Hold 2,500 runner polls on one broker scope (`BROKER_HELD_MAX` plus 500) with jobs queued | Polls past the cap get 503; those runners re-poll within 30 s with their sessions valid; job start p99 at most 35 s for them | Any `RunnerSessionInvalid`, a runner that exits, or a job unstarted after 60 s |
| BF-55 | Kill the leader site of an `Owner` scope's registration shard during a 256-job matrix | Polls reach a broker on the new leader site within 15 s; every job runs once; zero jobs assigned twice | A job assigned twice, or dispatch stalled over 60 s |
| BF-56 | Kill the repository's commit path (and, separately, `RunOwner`) between `RunCreated` and the `RequiredCheck` writes, then finish or cancel every job | Every `RequiredCheck` on an open PR head has a conclusion within 60 s of its job's `Completed` | Any `RequiredCheck` with `conclusion: None` after its job completed |
| BF-59 | `gh secret set` for a repository, an environment, and an org, then a job reading all three; then rotate the `SealKey` and the wrapping key | The job sees all three values before and after both rotations, and a fork-PR job sees none | A wrong value, a value shown to a fork PR, or a plaintext secret found in any entry or log |

BF-54 settles the runner's backoff on 503 (`decisions/OPEN-QUESTIONS.md`, F26).

### M4-WP4. Hosted runners on Sandboxes

**Build.** `graft-runners`, a Worker in the runner account that holds `HostedRunnersAppKey` and reaches the install only through the front (`design/DESIGN.md`, Shape, "Identity"; Stack, Workers, Sandbox SDK, and Cron Triggers). As the `graftyard-hosted-runners` App it reads `GET /api/graftyard/v1/owners/{owner}/actions/hosted/demand` (`Broker::demand`, `HostedDemand`) on each `workflow_job` webhook and on a 5-minute sweep. For each `start` entry it calls `generate-jitconfig` and starts one Sandbox per runner (`standard-3`, or `standard-4` for a 4-core label), destroys it on exit or `stale`, and starts none past `RUNNER_VCPU_STOP_PCT`. The hosted label table and the per-owner cap that `decide` enforces on hosted registrations (`design/SHARED.md`, "Actions", Hosted runners). The runner image with unmodified `actions/runner`, one job per Sandbox. CLI: `graftyard runners init` and `graftyard runners upgrade` ("Failure modes", `actions state`).

**Depends on.** M4-WP3, M4-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-60 | 50 `ubuntu-latest` jobs queued for one owner with the cap at 20, while two `graft-runners` sweeps overlap | At most 20 hosted runners at once; every job finishes; no Sandbox is left 10 minutes after the last job | More than 20 at once, a job that never finishes, or a leaked Sandbox |

It also carries BM-4's matrix run.

### M4-WP5. The web app [inference placement]

**Build.** The React 19 and Primer React web app, reading the public GraphQL and REST endpoints with persisted queries plus the private `/_t/ui/*` API for diffs, file trees, and blame. The edge serves the HTML shell with the first route's query result inlined, and the static assets with immutable cache headers. Each page region is its own error boundary with its own query, and the client retries only idempotent reads within a budget (`design/SHARED.md`, "Frontend and native app"). The live log view reads the job's `LogRelay` WebSocket and falls back to step log blobs when the relay has no runner stream (`design/SHARED.md`, "Actions", Logs). `design/DESIGN.md`, "Milestones 1 to 7", places it in Milestone 4 [inference], because its pages read issues and pull requests (M2), sign-in (M3), and live Actions logs (M4-WP3).

**Depends on.** M2-WP4, M3-WP3, M4-WP3.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-38 | Web app ("Frontend and native app" in `design/SHARED.md`) under Playwright [inference]: sign in, open a repository, a pull request's diff, checks, and timeline, and a running job's live log (the `actions-logs-results` oracle), then force the checks region's query to fail | Every page renders, the first route's data is inlined in the HTML shell with zero API requests before first paint, the log streams live, and with the checks query failing only the checks region shows an error | Any page fails, the first route waits on a second round trip, or the checks failure breaks another region |
| BF-61 | View a running job's live log in the web app, then kill its `LogRelay` | Lines appear within 2 s; after the kill, the viewer falls back to step log blobs and the final log is complete | A line later than 2 s, no fallback after the kill, or missing lines in the final log |

### M4-WP6. Milestone 4 gate

**Build.** Nothing new.

**Depends on.** M4-WP1, M4-WP2, M4-WP3, M4-WP4, M4-WP5.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| BM-4 | An unmodified `actions/runner` runs a matrix, and a site is failed mid-run with no lost job state |

## Milestone 5: release control and site days

Built (`design/DESIGN.md`, "Milestones 1 to 7"): the `Release` group with genesis against the trust anchor, `Keys` with the executor key, `ReleaseUploaded`, `IntentStep`, `IntentDone`, `SiteRelease`, `SiteMode`, pin-only `Pointers` set by undos, `graft-front`, `graft-ops` with the release bucket, `POST /api/graftyard/v1/release/entries`, `FrontRoll` and `OpsRoll`, `verify`-gated formats, gradual edge deploys, gated site days with pre-signed `Rollback`s, the Workflows script's day included, `SiteHealth` drain and undrain, `StaleMode`, the revocation and rotation runbooks, the change feed and export endpoints, and the vault with `graftyard vault upgrade`.

### M5-WP1. The `Release` group, genesis, and keys

**Build.** `ChainId::Release` on `Tri`, `Bound` with `prev` as the digest of the last committed signed entry (not counting `Noop` and `Config`), every `ReleaseEntry` variant, `Keys` with generations and the `drain`, `executor`, and `agents_executor` keys, `SiteMode`, `Mode`, `IntentId`, `SignedIntent` with its pre-signed `undo`, every `Intent` variant, `IntentState`, and the signer rules. The group's refusals: unknown keys, wrong generations, intents past `not_after`, a release, pointer, `Rollback`, or `Abort` that cannot read an enabled format, an intent naming a release with no `ReleaseUploaded`, a second current intent except `ReplaceMember`, a drain-key `SiteMode { Drain }` while another site is drained, and an `EdgeRoll` that drops an undo's version (`design/DESIGN.md`, Shape, "Release group"). `Genesis`, `InstallId`, `ReleaseId`, `VersionId`, `Format`, `FormatSet`, `ImageRef`, `Scope`, and `ReleaseUploaded` with `Script`. A genesis is accepted only when it matches the `GRAFTYARD_INSTALL` and `GRAFTYARD_CUSTOMER_KEY` trust anchor, and M1-WP7 built that acceptance. `POST /api/graftyard/v1/release/entries` with its 201, 409, and 422 bodies. `StaleMode`, `STALE_MAX`, and `stale` (Shape, "The read path"). CLI: `graftyard init`, `graftyard genesis`, `graftyard sign`, and `graftyard install` ("Install operations", Genesis and Keys).

**Depends on.** M3, M1-WP7.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-6 | Replay every historical `Release` entry, pre-signed undos and `McpPointer` and `ControllerPointer` entries included, after `Keys { vendor: None }` | 100 of 100 refused | Any accepted |
| BF-40 | Submit a second intent while one is current, and an intent naming a release with no `ReleaseUploaded` | Both refused | Either committed |
| BF-41 | Two genesis entries sent to an empty `Release` group: one matching the anchor, one signed by another key | Only the matching one commits | The other commits |
| BF-42 | Sign an entry offline while `graft-ops` commits an `IntentStep` | 409 with the current digest; the re-signed entry commits | A stale `prev` commits |
| BF-45 | A forged `ReleaseUploaded`, `ReplaceMember`, or pointer signed by the executor key, for a site not drained 24 h | Only a correctly signed `ReleaseUploaded` commits | A pin, or a `ReplaceMember` of an undrained site |

### M5-WP2. `graft-front`, pin-only pointers, and gradual edge deploys

**Build.** `graft-front`, `Pointers` (each `Option<VersionId>`, `None` following the deployment split and `Some` set only by a pre-signed `EdgePointer`, `OpsPointer`, `McpPointer`, or `ControllerPointer` undo, cleared by the next intent that rolls the same script), `POINTER_TTL`, and the whole `Release` view cached per isolate, members included, refreshed from any member's applied state and never moved backward. A `StaleMode` that cannot commit is written to every reachable member's `pending` row and verified against the cached `Keys`. `fetch()`-based service bindings with version overrides, N and N+1 in each edge deployment, the edge's version report and the mismatch alarm, `Intent::EdgeRoll`, and `Intent::FrontRoll` (`design/DESIGN.md`, Shape, "Release group" and "The read path"; "Commit and read paths", journey 7).

**Depends on.** M5-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-8 | Blackhole every `Release` member for 10 minutes, then set `EdgePointer` to a version outside the deployment | Requests keep the last pointers; the mismatch alarms within `POINTER_TTL` and the `EdgeRoll` that dropped it was refused | Any request blocked on the group, or a silent mismatch |
| BF-43 | `EdgePointer` undo under 1% `EdgeRoll` with the API blocked, then the next `EdgeRoll` | Every request pins to N within `POINTER_TTL`; the next `EdgeRoll` sets `None` | Any request off N while pinned, or the pin survives |
| BF-48 | Blackhole the `Release` group, then send a customer-signed `StaleMode` | Scoped groups serve `stale()` within `POINTER_TTL`; the flag expires at `until` | No stale reads, or a flag past `STALE_MAX` honored |

### M5-WP3. The `graft-ops` executor and record formats

**Build.** `graft-ops` as the executor of signed intents: uploads only from the release bucket after the release signature verifies, then commits `ReleaseUploaded` with the executor key and resolves each intent's `ReleaseId` to `VersionId`s from it. It commits `IntentStep` after each step passes its gate, `SiteRelease`, and `IntentDone`, so a restarted `graft-ops` reruns only the first unrecorded step. The customer's deploy token and read token in Secrets Store, every cron that calls the Cloudflare API, `Intent::OpsRoll`, `Rollback { target, release }`, and `Abort { intent, target }` (`design/DESIGN.md`, Stack; Shape, "Release group"; "Install operations", Tools, Keys, and Release roll). `FormatsEnable` with the release-signing key plus the customer key, and formats gated by `verify` (`design/SHARED.md`, "Record formats and the two-release rule"; `design/DESIGN.md`, Shape, "The write path"). CLI: `graftyard release roll`, which signs one intent at a time.

**Depends on.** M5-WP1, M5-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-7 | After `FormatsEnable`, submit the pre-signed `Rollback` to N and `Abort { intent, target: N }` | Both refused, and both page | Either applied |

### M5-WP4. Site days and automatic drain

**Build.** `Intent::SiteDay` with `DayTarget` (a site, the Workflows script, whose day skips drain and undrain, or `Containers(site)`, the undo of a `ContainerDay`) and `Intent::ContainerDay`, whose steps restart shards onto the new image in rendezvous order and whose rollback returns them to `git_prev` with no API call. Site days: drain, deploy at 1%, 10%, 50%, then 100%, and undrain, with the steps of `ROLL_STEPS` and the gate and dwell constants of "Install operations", Site day. Each step and the 1% day gate on the N+1 members' apply lag, apply and verify exceptions, ack rate, and `STEP_DOWN` trips. A new site leads 1% of its home groups for its first day. Spare and residency sites take a site day per release behind a canary group. A breach runs the pre-signed `Rollback` with no human signature ("Install operations", Site day; "Commit and read paths", journey 7). `SiteHealth` objects on each observer site, automatic drain by `graft-ops` with its drain-only key, and undrain by the same key under `UNDRAIN_BURN` for `UNDRAIN_AFTER` (journey 5, "Automatic drain"; Shape, `SiteMode`).

**Depends on.** M5-WP3, M1-WP3.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-39 | Kill `graft-ops` at each step of a `SiteDay` and an `EdgeRoll`, 100 times | It resumes after the last `IntentStep`; no step skipped; one `IntentDone` | A skipped or doubled step |

It also carries BM-5's unreadable format "commits nothing and moves A's groups".

### M5-WP5. The change feed, the export endpoints, and the vault

**Build.** The change feed: the Queue consumer that writes `ChangeBatch` as `ExportObject`s with entry bytes to `R2Key::Export` in the export bucket, `ExportCursor`, and the four export endpoints under `/api/graftyard/v1/export` with their chain-id encoding and errors, served from followers, learners, or archives and never the leader. `ExportClaims` export tokens, revoked by `jti` through `DELETE /api/graftyard/v1/export/token` (`design/DESIGN.md`, Shape, "Lifeboat feed"). `graftyard vault init` and the vault Worker, which pulls the feed, entries, snapshots, and packs under an install-scoped export token signed by the customer key into R2 under 35-day bucket locks, deletes a pack no sooner than `RESTORE_WINDOW` after the install's GC, and exports every `ChainId`. `graftyard vault upgrade` updates the vault Worker under the vault account's token ("Install operations", Vault; "Deployment modes"; "Failure modes", `vault`).

**Depends on.** M2-WP5, M5-WP1, M5-WP4.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-47 | `release roll` end to end, including `SiteDay { Workflows }` and `graftyard vault upgrade` | Order as in Release roll; vault upgraded with no install token | Any other order, or an install token used |
| BF-49 | Vault pulls every export endpoint; then the export `jti` is revoked | All four endpoints served; refused within `READ_CACHE` after revocation | Any served after `READ_CACHE` |

The vault's restore is gated by BM-7 (M7-WP8).

### M5-WP6. Revocation and rotation runbooks

**Build.** The revocation runbook: revoke the vendor's release-bucket token, `Halt { intent }`, `Keys` with `vendor: None`, then `Resume { intent, undo }` with undos re-signed by the customer, or `Abort { intent, target }`. Undos are checked against the `Keys` current at the parent intent's index. The rotation runbook as its table: two Secrets Store slots per `SecretName`, `Rotate { secret, slot }` signed by an intent key, and new values written by the customer or a site admin with their own Wrangler login, plus `TokensEvent::VerifyingKey` for the signing key ("Install operations", Revocation runbook and Rotation runbook).

**Depends on.** M5-WP1, M5-WP3, M5-WP4.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-44 | A vendor-signed undo not in the parent's `undo` list, and a `Resume` after `Keys { vendor: None }` without customer re-signed undos | Both refused | Either accepted |
| BF-46 | `Rotate` of each `SecretName` with live traffic | No new script version; zero failed calls; old value unused after the switch | A new version, or failures |

It also carries BM-5's vendor-token revocation mid-roll.

### M5-WP7. Milestone 5 gate

**Build.** Nothing new.

**Depends on.** M5-WP1, M5-WP2, M5-WP3, M5-WP4, M5-WP5, M5-WP6.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| BM-5 | A deliberately unreadable format released to `graft-site-a` commits nothing and moves A's groups. With the vendor token revoked mid-roll, `Halt`, `Keys { vendor: None }`, then `Abort { intent, target: N }` returns every site and pinned script to N, and a vendor-signed intent submitted after `Keys { vendor: None }` is refused. A bad `graft-ops` at 1% swaps back by `OpsPointer` with the API blocked |

## AM2: `graft-mcp` (after M5)

Built (`design/AGENT-LAYER.md`, "Milestones"): `graftyard apps init` in the install account, `graft-mcp` with consent, grants, and narrowed refresh, `graft-front` routing shipped as a `FrontRoll`, `McpPointer`, and `McpRoll`. AM2 follows base 5, because `FrontRoll`, `McpRoll`, and `McpPointer` are `Release` group entries that Milestone 5 builds.

### AM2-WP1. `graftyard apps init` and the `graft-mcp` authorization server

**Build.** `graftyard apps init` in the install account, which registers `Graftyard MCP` and `graftyard-insights` and stores their keys and the MCP client secret in the install's Secrets Store (`design/AGENT-LAYER.md`, Shape, "First-party Apps"; "Deployment modes", "Enable agents", step 1). `Consent`, `McpGrant`, `McpToken`, `McpRefresh`, and `WriteScope` (`design/AGENT-LAYER.md`, Shape, "graft-mcp"). The flow in journey A7: RFC 9728 and RFC 8414 metadata, PKCE S256, CIMD fetches with their limits, consent before the GHES redirect, the `__Host-` cookie bound to `state`, `token/scoped` narrowing with the additive `graftyard_grant` and `graftyard_write` fields, deletion of the unscoped `ghu_`, the RFC 9207 `iss` parameter, and refusal of an older `generation` that revokes the grant. The grant endpoints under `/api/graftyard/v1/mcp/grants`, authenticated by the `Graftyard MCP` App's client credentials ("API reference"). `IdentityEvent::McpGrant` and the settings page at `https://mcp.<host>/settings`. The sealing key in Secrets Store.

**Depends on.** M3-WP3, M3-WP4, AM1-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-6 | `token/scoped` from `Graftyard MCP` without `graftyard_grant`, with another user's grant, and with `graftyard_write` above the grant. A stale `generation` on `POST /mcp/grants/{id}/generation`. Grant calls with wrong client credentials | 422 for each bad token request. 409 and the grant revoked for the stale generation. 401 for wrong credentials, 100 of 100 | Any accepted |

It also carries LM-2's MCP row (the rerun of L0-08).

### AM2-WP2. `graft-mcp` tools

**Build.** `call`, `Toolset`, `ToolError` with `retry_after_s` and rate fields, the `coordination` and `provenance` tools, and the trust view on every text field (`design/AGENT-LAYER.md`, Shape, "graft-mcp"; journey A7, step 7). Writes run as the `Graftyard MCP` `AgentApp` with `requester = sub`.

**Depends on.** AM2-WP1, AM1-WP4, AM1-WP5.

**Acceptance.** No gate row of its own. It carries LM-2's MCP row.

### AM2-WP3. `graft-mcp` release wiring

**Build.** `graft-front` routes `mcp.<host>` and reads `Pointers.mcp` with `Pointers.edge`. While either pins its script, it sets the override in one header, which `graft-mcp` forwards on its edge calls, and a `None` pointer sets no override. This ships as a `FrontRoll`. `McpRoll` runs at 1% for a day, then 100% (`design/AGENT-LAYER.md`, Shape, "graft-mcp", release control comment; `design/DESIGN.md`, Shape, "Release group").

**Depends on.** AM2-WP1, M5-WP1, M5-WP2, M5-WP3.

**Acceptance.** No gate row of its own. It carries LM-2's swap-back.

### AM2-WP4. AM2 gate

**Build.** Nothing new.

**Depends on.** AM2-WP1, AM2-WP2, AM2-WP3.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| LM-2 | MCP row passes, and a bad `graft-mcp` at 1% swaps back with the API blocked |

The MCP row is L0-08.

## AM3: the controller and Graftyard-run agents (after M5)

Built (`design/AGENT-LAYER.md`, "Milestones"): the controller, `graftyard agents init`, `graft-agents-front`, its `graft-ops` with `ControllerRoll`, `ControllerPointer`, and `Keys.agents_executor`, the `GET /api/graftyard/v1/release/entries` reads, the workspace endpoint, workspace forks, in-repo mode, the outbound router, budgets, session logs, session pages, `agents_enabled`, the first-party task delivery, and Graftyard-run task creation through the task API. AM3 follows base 5 for the same reason as AM2, and base 4 for the event pipeline.

### AM3-WP1. The agents account, `graft-agents-front`, and the controller

**Build.** `Task`, `TaskState`, `TaskBudget`, and `Workspace` (`design/AGENT-LAYER.md`, Shape, "Controller"). The controller is a GitHub App backend reached by webhooks over HTTPS through `graft-agents-front`, which sets a version override on `graft-controller` only while `Pointers.controller` is `Some`. `AgentsAnchor` (install id, customer key, and host, set as script variables) and `AgentsReleaseView`, built from `GET /api/graftyard/v1/release/entries?after=` under a `graftyard-agent` App JWT, checked against the anchor, cached for `POINTER_TTL`, never moved to a lower index, and kept when a refresh fails. The controller runs Workflows per task and Sandboxes, and heartbeats each Sandbox, because Sandboxes stop after 6 hours without activity ("Open questions and risks"). Its own `graft-ops` executes `ControllerRoll`, commits its own `ReleaseUploaded` for `AgentsFront` and `Controller`, and signs with `Keys.agents_executor`. CLI: `graftyard agents init`, which registers only `graftyard-agent` and `graftyard-workspace`, stores their keys in the agents account's Secrets Store, creates the agents scripts with the install's trust anchor, and prints the agents executor public key for a customer-signed `Keys` (`design/AGENT-LAYER.md`, "Deployment modes", "Enable agents"; journey A8).

**Depends on.** M4, M5-WP1, M5-WP2, M5-WP3.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-7 | Bad `graft-controller` at 1% with the deployments API blocked. Submit the pre-signed `ControllerPointer` undo, then blackhole `GET release/entries` for 10 minutes | Every `graft-agents-front` isolate pins N within `POINTER_TTL` of its next request. Webhooks keep flowing on the last view | Any webhook reaches N+1 after the pin, or blocks on the read |
| LF-8 | `release/entries` with an entry signed for another install id, a `Keys` entry not signed by the anchored customer key, and an entry out of index order | All refused by the agents view, 100 of 100 | Any accepted |

It also carries LM-3's "a Sandbox killed mid-task resumes" and "a bad controller at 1% swaps back".

### AM3-WP2. The outbound router

**Build.** `outbound` and `Allowed` (`GraftyardHost`, `ModelInference`, `Registry`): default deny, `Authorization`, `Cookie`, and URL userinfo overwritten with the task's token, receive-pack refused outside the task's prefix, and status and check-run writes refused (`design/AGENT-LAYER.md`, Shape, "Controller"; journey A4).

**Depends on.** AM3-WP1.

**Acceptance.** No gate row of its own. It carries LM-3's rerun of L0-02 and L0-03.

### AM3-WP3. Graftyard-run tasks and workspaces

**Build.** `POST .../tasks` for `GraftyardRun` tasks, which returns 422 while `OrgPolicy.agents_enabled` is off, with the creation rules and the controller's exception for issues assigned to its bot (`design/DESIGN.md`, Shape, "Agent load and agent safety"; `design/AGENT-LAYER.md`, Shape, "Tasks and the activity feed"). The first-party-only task delivery on the event pipeline, with the event name, payload, and signing secret in `design/AGENT-LAYER.md`, "API reference". Workspace forks (`Workspace::Fork`, `<org>-agents/<repo>`), created through `POST /api/graftyard/v1/repos/{owner}/{repo}/workspace`, which only `graftyard-workspace` may call, and in-repo mode. Only `graftyard-workspace` pushes, and `graftyard-agent` keeps `contents: read`. The base chain sits in `Push::deps()` for fork-mode tokens. `AgentPolicy.ci` with `Approve` by default and `AutoReadOnly` opt-in.

**Depends on.** AM1-WP3, M4-WP1, AM3-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-9 | Token ceilings by kind: a `graftyard-agent` token pushes to the base and the fork. A `graftyard-workspace` token pushes to the base in fork mode. A `graftyard_task` request from a third App, or for a repository that is neither the base nor its fork | 100 of 100 refused for each | Any accepted |

It also carries LM-3's "A3 end to end in both modes" and the rerun of L0-14 and L0-04.

### AM3-WP4. Session logs and session pages

**Build.** `SessionLog`, `Step`, `StepKind`, and `Harness`, pushed by `graftyard-workspace` after secret scanning and redaction as stored blobs under `SESSION_REFS`, and served as JSON (`design/AGENT-LAYER.md`, Shape, "Controller"). In fork mode the session URL names the base repository, and the page serves the log from the fork. Session pages and `/log` and prompt reads with their permissions (L0-15). Retention as a retention command plus GC ("Tradeoffs accepted").

**Depends on.** AM3-WP1, AM1-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-10 | Fork-mode session URL in a trailer, read by a base reader with no access to the fork | The page and JSON serve the log from the fork | 404, or the URL names the fork |

It also carries LM-3's "the provenance row passes" (the rerun of L0-10).

### AM3-WP5. AM3 gate

**Build.** Nothing new.

**Depends on.** AM3-WP1, AM3-WP2, AM3-WP3, AM3-WP4.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| LM-3 | A3 end to end in both modes, prompt-only tasks included, the task rules row, containment, ceiling, and agent CI rows rerun, a Sandbox killed mid-task resumes, a bad controller at 1% swaps back, the provenance row passes |

The rows rerun are L0-14, L0-02, L0-03, and L0-04. The provenance row is L0-10.

## Milestone 6: region loss, SSH, and the lifeboat

Built (`design/DESIGN.md`, "Milestones 1 to 7"): `ReplaceMember` signed by the executor key and `RestorePlacement`, both finding groups through `MemberRegistry`, `Degraded`, lifeboat tail and pack pulls over the Milestone 5 export endpoints, `graft-blob`, lifeboat registration, `lifeboat:read` enroll, the workerd bundle, and SSH through the outbound handler with host keys from Secrets Store if Milestone 0 passed, plus `git-ssh` and the SSH parts of `lfs` and `rest:deploy-keys` [inference].

### M6-WP1. Region loss and re-protection

**Build.** `ReplaceMember { lost, spare }` as a `Release` intent signed only by the executor key and accepted after a customer-signed `SiteLost`, or after `ZERO_ACKS` with zero acks in both other sites' `SiteHealth`. `graft-ops` then starts one Workflow instance per group, enumerated from the surviving sites' `MemberRegistry` rows that name the lost site. Each instance adds a member on the same continent's spare through a `Joint` config, under a new member generation and by `InstallSnapshot`, copies single-copy packs first, then each network once, and refuses a spare that cannot read every enabled format. `RestorePlacement { site }` takes an intent key, is refused while the site is drained, and runs once the site counts as recovered. Transfer back to `home`, and `deleteAll()` on removed members after 35 days. `GroupConfig::Degraded` for the `*2w` placements (`design/DESIGN.md`, "Commit and read paths", journey 5; Shape, "Chains" and "Release group"; "Install operations", Site loss; "Failure modes", `chain groups`).

**Depends on.** M5, M3-WP5.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-50 | Lose site A with every API token revoked; drain it 24 h; then recover it | Executor-signed `ReplaceMember` moves every group with a voter on A, using B's and C's registries; `RestorePlacement` refused until undrained | A group left out, or a restore while drained |

It also carries BM-6's drill.

### M6-WP2. The SSH front

**Build.** Both ingresses to one SSH front: the Tunnel public hostname of type `ssh` for the Pro and Business option, off by default and turned on per install, with the client setup in the install docs (`Host git.example.com` with `ProxyCommand cloudflared access ssh --hostname %h`), gated by B0-24a; and Spectrum on port 22 to the Tunnel's virtual network for Enterprise, gated by B0-24b. Spectrum refuses a Tunnel origin on Pro [measured, `tests/m0/results/m0-wp7.md`]. The edge's over-cap refusal and the 413 runbook name SSH where the install has it on. Start the container with `chown root:root /` or a `graft-sshd` key lookup without OpenSSH's ownership check, and change the front's code only by stopping its container (`design/DESIGN.md`, Shape, "SSH front"). The SSH front container class and image with `graft-sshd` and cloudflared replicas, `ssh_outbound` with the virtual hosts it serves, and Spectrum, the Tunnel, its virtual network, and its public hostname through Terraform (`design/DESIGN.md`, Shape, "SSH front"; journey 4, step 3; "Install operations", Tools). `SshHostKeys` in two Secrets Store slots, handed to the containers by the outbound handler, rotated by `Rotate`, and published at `/api/graftyard/v1/meta/ssh`, so `/api/v3/meta` keeps its GHES shape ("Install operations", Rotation runbook). By either ingress sshd never sees the client's IP, so SSH abuse limits count by key and token in the identity groups ("Open questions and risks"). The `git-ssh` surface and its oracle, and the SSH parts of the `lfs` and `rest:deploy-keys` oracles (`design/SHARED.md`, "Surfaces, tiers, and oracles").

**Depends on.** M3-WP2, M3-WP3, M1-WP6, M0-WP7, M5-WP6.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-51 | Rotate SSH host keys | `UpdateHostKeys` clients learn the new set; `/api/graftyard/v1/meta/ssh` lists both slots for the overlap; same fingerprints on every replica | A mismatch, or a host-key error for an updated client |
| BF-52 | Suspend a user, then push over SSH with their key; separately, revoke an SSH key with 1,000 of its commands queued | Every push from the suspended user refused; 100% of commands decided after the revocation get 401 | Any accepted |

B0-24a gates the `cloudflared` option on Pro and Business, and B0-24b native SSH on Enterprise.

### M6-WP3. The lifeboat

**Build.** The workerd bundle with a pinned workerd version, rebuilt on a fresh directory at every version change, with `uniqueKey` and the key modifier backed up. `graft-blob` (temp file, fsync, link, directory fsync before any 200, checksum reads, weekly scrub). `LifeboatStatus`, `/api/v3/meta` and the `X-Graftyard-Lifeboat-Lag` header, tail and pack pulls through the M5-WP5 export endpoints signed by the host's key and served from followers when the R2 feed is stale, and a halt on any undecodable entry. `LifeboatAdd`, `LifeboatDecoder` sent through `POST /api/graftyard/v1/release/entries` at least daily, and `LifeboatRemove`, and `formats:enable` waiting for every registered lifeboat. The `lifeboat:read` token with `TokensEvent::LifeboatRead`, minted by `POST /api/graftyard/v1/lifeboat/tokens` and refused by the main edge. ACME DNS-01 through a validation zone in a separate account. CLI: `graftyard lifeboat init` and `graftyard lifeboat enroll` (`design/DESIGN.md`, Shape, "Lifeboat feed"; "Install operations", Lifeboat; "Deployment modes").

**Depends on.** M5-WP3, M5-WP5.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-53 | Lifeboat enroll, a daily `LifeboatDecoder`, and tail and pack pulls with the R2 feed stopped | Token refused by the main edge; no 7-day alarm; tail and packs fetched from followers | Any leader read, or a main-edge accept |

It also carries BM-6's "enrolled `gh pr list` works against the lifeboat with the edge blocked".

### M6-WP4. Milestone 6 gate

**Build.** Nothing new.

**Depends on.** M6-WP1, M6-WP2, M6-WP3.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| BM-6 | After two release rolls and one `FormatsEnable`, in a journey 5 drill, one site's member and container classes fail for a day, and `ReplaceMember` runs with every account API token revoked. It passes only if no script version is created, the spare reads every enabled format, every acknowledged write is present, groups return home, and enrolled `gh pr list` works against the lifeboat with the edge blocked |

## AM4: insights, the predictor, and Code Mode (after M6)

Built (`design/AGENT-LAYER.md`, "Milestones"): the insights consumer fed only by its cursor, `InsightVerdict` rows committed through `POST .../insights`, `graftyard-insights` check runs and labels, the agents page, the predictor, and Code Mode behind `OrgPolicy.code_mode`.

### AM4-WP1. The predictor

**Build.** `GitCacheShard::predict` with `git merge-tree --write-tree --name-only` against up to `PREDICT_PAIRS` overlapping refs, on the network's second warm site, under `PREDICT_CPU`, paused while that site leads. `Verdict`, read from `PathSet` rows (`design/AGENT-LAYER.md`, Shape, "Overlaps and prediction").

**Depends on.** AM1-WP4, M6.

**Acceptance.** No gate row of its own. It carries LM-4's rerun of L0-05 and L0-06.

### AM4-WP2. The insights consumer and the agents page

**Build.** `Consumer::Insights`, the only producer of `InsightJob`. Its cursor sends one job per entry that writes or deletes a `PathSet` row or opens a PR on its ref, and `apply` enqueues nothing. `InsightVerdict`, one core row per ref, which `graftyard-insights` commits through `POST .../insights` (`design/AGENT-LAYER.md`, "API reference"). `decide` refuses a `head` that is no longer the ref's tip and writes nothing for an unchanged result. Otherwise one entry holds the row plus, as metadata, the changed `InsightCheck` runs (`neutral`, on the head SHA) and the label changes. The feed's `Verdict` event, the PR and agents pages, and `graftyard_overlaps` read only that row. Clusters are connected components of overlapping `PathSet` rows, named by the lowest PR number, and a label is removed when its cluster dissolves. `graftyard-insights` uses the key that `graftyard apps init` stored in the install account (AM2-WP1), so a self-hoster without a controller gets insights. The labels `agent:<app>` and `overlap:<cluster>`. The agents page (`design/AGENT-LAYER.md`, Shape, "Overlaps and prediction" and "First-party Apps"; journeys A5 and A6).

**Depends on.** AM4-WP1, M4-WP1, AM2-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-11 | `graftyard apps init` on an install with no agents account, then the MCP and A5 insights flows | MCP and overlap check runs work. No `Graftyard MCP` or `graftyard-insights` secret exists in any agents account | Either flow needs the agents account |
| LF-14 | A5 replay with every `InsightJob` delivered twice, plus a stale job after a newer push | One `InsightVerdict` entry per changed result, none for a repeat, 409 for the stale head, one check run per changed check, and every feed `verdict` event at an `InsightVerdict` index | Any duplicate entry or check run, a verdict for a moved head, or a feed `verdict` event at another index |
| LF-15 | 1,000 agent pushes across a leader change | Exactly one `InsightJob` per `PathSet` entry, all from the `Insights` cursor, none from `apply` | Any second job for one entry, an entry with no job, or a job from `apply` |
| LF-16 | PRs 4, 7 and 9, where 4 overlaps 7, 7 overlaps 9, and 4 does not overlap 9. Close 4, then close 7 | `overlap:4` on all three, then `overlap:7` on 7 and 9, then no `overlap:` label on 9. Closed PRs keep their labels | Any other name, or a label on an open PR that is in no cluster |

It also carries LM-4's "A5 and A6 pass on a seeded repository".

### AM4-WP3. Code Mode behind `OrgPolicy.code_mode`

**Build.** `CodeMode` on the opt-in route `/codemode`. `search()` and `execute()` run model code in a Dynamic Worker whose only capability is `codemode.request()`, bound to `upstream` and passed through the same tool deny-list as `call` (`design/AGENT-LAYER.md`, Shape, "graft-mcp"). It serves a token only while every repository's owner is an org with `OrgPolicy.code_mode` on, which `graft-mcp` reads through `GET /orgs/{org}/agents` with the `Graftyard MCP` client credentials and caches for 60 s. User-owned repositories never get it. `org_agents.code_mode` and its admin-only `PATCH` (`design/AGENT-LAYER.md`, "API reference"; "Deployment modes", "Enable agents", step 5).

**Depends on.** AM2-WP2, AM1-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-18 | `/codemode` for an org repository with `code_mode` off, then on, then off; once for a user-owned repository; a `PATCH` of `code_mode` by a non-admin | Refused while off and for the user repository, served while on, refused within 60 s of turning it off, and the non-admin `PATCH` refused | Any Code Mode call served while off, on a user repository, or 60 s after turning it off, or a non-admin `PATCH` accepted |

LM-4 enables it only if it serves 2026-07-28.

**Flip condition.** Code Mode stays off unless it serves MCP spec 2026-07-28 (LM-4). Whether `openApiMcpServer()` does is open (`decisions/OPEN-QUESTIONS.md`, A7).

### AM4-WP4. AM4 gate

**Build.** Nothing new.

**Depends on.** AM4-WP1, AM4-WP2, AM4-WP3.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| LM-4 | Both predictor rows pass, A5 and A6 pass on a seeded repository, and Code Mode is enabled only if it serves 2026-07-28 |

The predictor rows are L0-05 and L0-06.

## Milestone 7: breadth and modes

Built: BYOC and self-hosted through Terraform and Wrangler, account-separated sites, managed front and cell accounts, `RateCounter` rate limits, `Solo` and workerd for evaluation with the `LeaveEvaluation` move to `Tri`, child groups for oversized metadata (`RepoMeta` chains seeded by the `MetaSplitter` Workflow), and import from GHES as `Imported` entries with the plan preflight.

### M7-WP1. BYOC and self-hosted installs

**Build.** Terraform for the zone, DNS, buckets, queues, Spectrum, Tunnel, and WAF. Wrangler for the first creation of `graft-front`, the edge, the site scripts, and `graft-ops`, after which only `graft-ops` creates versions. The install, vault, and runner accounts, and `Keys { vendor: None }` for self-hosted ("Install operations", Tools; "Deployment modes", "What each mode requires of the customer").

**Depends on.** M6.

**Acceptance.** No gate row of its own. It carries BM-7's self-hosted install.

### M7-WP2. Account-separated sites

**Build.** Only if B0-17 passed. `SiteLink::Https` in production, four accounts, ring keys in `Keys.ring`, and the rule that a new ring key enters `Keys` before its site switches to it (`design/DESIGN.md`, Shape, "Account-separated sites (option)"; "Install operations", Rotation runbook). CLI: `graftyard sites add --link https`.

**Depends on.** M7-WP1, M0-WP5.

**Acceptance.** No gate row of its own. B0-17 decides whether it is built.

### M7-WP3. Managed front and cell accounts

**Build.** A front account with the zone, `graft-front`, the edge, and the global `Names` and `Tokens` groups on its own three sites. The front edge resolves an owner's cell from `Names`, or from the cell bits of a `DbId`, and calls the cell's edge over Ed25519-signed HTTPS, and it reads each cell's pointers the same way (`design/DESIGN.md`, Shape, "Managed cells" and "Chains", `DbId`). The Data Localization Suite for EU cells. The managed public service rules: anonymous access, rate limits, abuse, takedowns, and public content behind a CDN (`design/SHARED.md`, "Public service concerns"). The rate limits count in `RateCounter` objects, one per `RateKey`, which isolates update every `RATE_FLUSH` requests or 1 s. An isolate that cannot reach its counter counts locally against the full limit (`design/DESIGN.md`, Shape, "Identity"; `design/SHARED.md`, `rest:rate-limit`). BYOC and self-hosted installs that turn GHES rate limits on use the same class ("Deployment modes", `derived stores`).

**Depends on.** M7-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-66 | 20 isolates hit one key at the limit, then the counter object is killed | `x-ratelimit-remaining` and enforcement overshoot by at most 20 requests times the number of isolates. The kill refunds at most one window | An overshoot above that bound, or more than one window refunded |

It also carries the managed-cell half of BF-9.

### M7-WP4. Child groups for oversized metadata

**Build.** `ChainId::RepoMeta` and `EntryBody::Split` with `MetaSplit`, `MetaSeed`, `MetaHalf`, and `MetaDropped`. The leader reads `ctx.storage.sql.databaseSize` after each snapshot and commits `MetaSplit { child, at }` at `CHILD_GROUP_AT`. From `at` the parent answers metadata-only work with `Moved { child }`, and the edge routes it to the child. `Audit` starts the `MetaSplitter` Workflow, which has the child commit `MetaSeed` from the metadata snapshot at `at`, then `MetaDropped` on the parent. Metadata writes get 503 until `MetaSeed`. `meta_finish` appends each both-tier entry's metadata half to the child, run by the edge before it answers and by `Audit` after a minute. Consumers skip `MetaHalf`, and a child cursor waits for the parent cursor. `RestoredTo` runs on the parent only, `RepoReset` resets each chain at its own index, the parent keeps the feed, the export name is `repo-meta:<id>`, and owner moves carry child groups (`design/DESIGN.md`, Shape, "Chains" and "Sites and placement"; Stack, Workflows). `InsightVerdict` rows stay on `Repo(repo)` (`design/AGENT-LAYER.md`, Shape, overlaps block).

**Depends on.** M2-WP1, M7-WP3, AM4-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-9 | A monorepo member reaching `CHILD_GROUP_AT`, and `octokit.paginate` over `/repositories/{id}` links on a managed cell | The metadata tier moves to a child group with zero `SQLITE_FULL`; page 2 is served by the right cell | Any `SQLITE_FULL`, or a page routed to the wrong cell |
| BF-62 | Monorepo member reaches `CHILD_GROUP_AT` under 300 commands/s with comments, PR opens, merges and releases with tags | Zero `SQLITE_FULL`. Metadata 503 window at most 10 min [guess]. Every both-tier entry has its `MetaHalf` within 60 s, with zero lost or doubled. A comment on #n is never delivered before #n opened, 100 of 100. Pre-split feed cursors continue with no 410 | Any `SQLITE_FULL`, a 503 window over 10 min, a lost or doubled half, an out-of-order delivery, or a 410 |
| BF-63 | `RepoReset` to a t before the split and to a t after it, on a split repository | Both tiers equal a refold at t | Any tier differs |
| LF-17 | Split the metadata tier of a repository that has live `InsightVerdict` rows | Feed cursors keep working, verdict rows and `Verdict` events stay on `Repo(r)`, and the check runs and labels reach `RepoMeta(r)` | Any lost verdict, event, check run or label |

When BF-9 and BF-62 pass, fleets of A1's size on one repository may be enabled (LM-1) [inference: BF-62 is the split under load that LM-1 waits for]. LF-17 gates AM4 but needs child groups, so it runs here.

### M7-WP5. `Solo` and workerd for evaluation

**Build.** `Placement::solo` with the install's signed `evaluation` flag in `Genesis.evaluation`, and workerd on customer hardware for evaluation only, under the same flag ("Deployment modes"). `Intent::LeaveEvaluation { sites, placements, default }` moves a Cloudflare `Solo` install to `Tri`. Only the customer key signs it, and `decide` refuses it unless the flag is set. Its commit adds the new sites to the `Release` view, swaps each system chain's record, and clears the flag, so `Placement::solo` refuses from then. `graft-ops` then moves `Release`, `Ids`, `Names`, `Tokens`, and each `Solo` owner through `Joint`, one `IntentStep` each (`design/DESIGN.md`, Shape, "Sites and placement" and "Release group"; "Deployment modes", Evaluation).

**Depends on.** M7-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-64 | `LeaveEvaluation` on a Cloudflare `Solo` install under write load. Separately, a vendor-signed `LeaveEvaluation` | System groups and owners end on `Tri` with `moving_from` cleared, zero lost acknowledged writes, `Placement::solo` refused afterwards. The vendor-signed one is refused | Any lost write, a group left on `Solo`, or the vendor-signed one accepted |

It also carries BM-7's "A `Solo` group moves to `Tri` under load with no failed write".

### M7-WP6. Import from GHES with the plan preflight

**Build.** `graftyard import ghes <archive>` for a `ghe-migrator` archive, as `design/SHARED.md` describes under `rest:migrations` ("Surfaces, tiers, and oracles"). Every import entry is `EntryBody::Imported(ImportBatch)` with `ImportItem`s that carry the original time, author, legacy id, and `merged`. `Webhooks`, `ActionsTrigger`, `Notify`, `RunOwner`, `PrSync`, `MergeQueue`, and `Insights` skip them. The import-only command set runs behind the `importing` flag in `Settings`, keeps issue numbers and moves the counter past the largest, records a merged pull request with no `Merged` event and no tree-diff check, and fills the `legacy_id` table (`design/DESIGN.md`, Shape, "Chains"; `design/SHARED.md`, `rest:migrations`). The install preflight checks the plan against the largest imported pack (`design/DESIGN.md`, "Review resolutions", CF-ACT-12).

**Depends on.** M7-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-65 | GHES archive import with hooks, a workflow, watchers, merged PRs and `#N` links | Zero webhook deliveries, runs or notifications. Numbers, `created_at` and authors match. Merged PRs show merged. `legacy_id` resolves every id | Any delivery, run or notification, or any mismatch |

Its oracle is also the `rest:migrations` export and import round trip in `design/SHARED.md`.

### M7-WP7. Whole-install restore

**Build.** The five-step restore from the vault or from a lifeboat export into new accounts: a named cutoff and a customer-signed genesis, members from the newest snapshot at or before the cutoff plus the log tail, packs copied and checked, revocation-class entries re-applied with the signing key rotated, and writes opened only when every index matches ("Install operations", Whole-install restore).

**Depends on.** M5-WP5, M6-WP3, M7-WP1.

**Acceptance.** No gate row of its own. It carries BM-7's two restores.

### M7-WP8. Milestone 7 gate

**Build.** Nothing new.

**Depends on.** M7-WP1, M7-WP3, M7-WP4, M7-WP5, M7-WP6, M7-WP7, and M7-WP2 if B0-17 passed.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| BM-7 | A self-hosted install with no vendor key passes milestones 1 to 6 gates and enables a new format. A vault restore to a chosen cutoff and a lifeboat-export restore into new accounts each lose no entry up to their cutoff, and afterward a developer's PAT and SSH key, a user-owned repository, and an org name all work, while a PAT revoked after the cutoff fails. A `Solo` group moves to `Tri` under load with no failed write |

## Work packages per milestone

| Milestone | Work packages | Count |
| --- | --- | --- |
| M0 | M0-WP1 to M0-WP16 | 16 |
| M1 | M1-WP1 to M1-WP8 | 8 |
| M2 | M2-WP1 to M2-WP6 | 6 |
| M3 | M3-WP1 to M3-WP6 | 6 |
| AM1 | AM1-WP1 to AM1-WP6 | 6 |
| M4 | M4-WP1 to M4-WP6 | 6 |
| M5 | M5-WP1 to M5-WP7 | 7 |
| AM2 | AM2-WP1 to AM2-WP4 | 4 |
| AM3 | AM3-WP1 to AM3-WP5 | 5 |
| M6 | M6-WP1 to M6-WP4 | 4 |
| AM4 | AM4-WP1 to AM4-WP4 | 4 |
| M7 | M7-WP1 to M7-WP8 | 8 |
| Total | | 80 |

## Coverage table

Every gate row in `design/DESIGN.md`, "Build order from zero", and `design/AGENT-LAYER.md`, "Build order", appears here once, with the one work package that owns it. Reruns are named in the owning milestone's gate work package and are not counted again. There are 151 rows: 38 B0, 7 BM, 68 BF, 16 L0, 4 LM, and 18 LF.

| Row | Source table | Measurement (abbreviated) | Work package |
| --- | --- | --- | --- |
| B0-01 | DESIGN, Milestone 0 | Commit p99 on `Tri`, 100 commands/s | M0-WP2 |
| B0-02 | DESIGN, Milestone 0 | Group commit, one entry in flight | M0-WP2 |
| B0-03 | DESIGN, Milestone 0 | Write and read RTO with one member class failing or blackholed | M0-WP2 |
| B0-04 | DESIGN, Milestone 0 | Site deploy under 100 writes/s | M0-WP4 |
| B0-05 | DESIGN, Milestone 0 | Follower regression stopped by the apply gate | M0-WP4 |
| B0-06 | DESIGN, Milestone 0 | Gray site, 30% errors | M0-WP3 |
| B0-07 | DESIGN, Milestone 0 | Gray site C during B's site day | M0-WP4 |
| B0-08 | DESIGN, Milestone 0 | `verify` refusal by an older follower | M0-WP4 |
| B0-09 | DESIGN, Milestone 0 | Rolling site refuses | M0-WP4 |
| B0-10 | DESIGN, Milestone 0 | Two-site residual | M0-WP4 |
| B0-11 | DESIGN, Milestone 0 | Linearizability, 48 hours | M0-WP2 |
| B0-12 | DESIGN, Milestone 0 | Member placement by round trips | M0-WP1 |
| B0-13 | DESIGN, Milestone 0 | Bucket placement against `location` | M0-WP1 |
| B0-14 | DESIGN, Milestone 0 | Daily working set of git cache shards | M0-WP6 |
| B0-15 | DESIGN, Milestone 0 | Idle cost of 100,000 groups over 24 hours, zero `Rtt` checks, against hourly controls | M0-WP1 |
| B0-16 | DESIGN, Milestone 0 | Account-owned deploy token deploys a container | M0-WP1 |
| B0-17 | DESIGN, Milestone 0 | Account-separated member RPC overhead | M0-WP5 |
| B0-18 | DESIGN, Milestone 0 | 16 learners, 16,000 reads/s | M0-WP3 |
| B0-19 | DESIGN, Milestone 0 | Container starts with the control plane out of reach | M0-WP6 |
| B0-20 | DESIGN, Milestone 0 | Push size by plan | M0-WP6 |
| B0-21 | DESIGN, Milestone 0 | Chunked push by plan | M0-WP6 |
| B0-22 | DESIGN, Milestone 0 | Container rehydration of 5 GB | M0-WP6 |
| B0-23 | DESIGN, Milestone 0 | Clone cache, 33 CI clones/s | M0-WP6 |
| B0-24a | DESIGN, Milestone 0 | SSH option through `cloudflared access ssh`, Tunnel, cloudflared (Pro and Business) | M0-WP7 |
| B0-24b | DESIGN, Milestone 0 | Native SSH through Spectrum, Tunnel, cloudflared (Enterprise) | M0-WP7 |
| B0-25 | DESIGN, Milestone 0 | Partial DO failure, 22% errors | M0-WP2 |
| B0-26 | DESIGN, Milestone 0 | Reset storm on all three sites | M0-WP2 |
| B0-27 | DESIGN, Milestone 0 | Drain guard | M0-WP4 |
| B0-28 | DESIGN, Milestone 0 | Restore window, day 1 to day 30 | M0-WP8 |
| B0-29 | DESIGN, Milestone 0 | Agent fleet replay (journey 9) | M0-WP9 |
| B0-30 | DESIGN, Milestone 0 | Hot-group storage, 30 days, 4 GB tier | M0-WP9 |
| B0-31 | DESIGN, Milestone 0 | Replay on apac with `INLINE_MAX_HOT` and placement failing | M0-WP9 |
| B0-32 | DESIGN, Milestone 0 | Burst of 500 agent pushes | M0-WP9 |
| B0-33 | DESIGN, Milestone 0 | 5,000 agents on one group | M0-WP9 |
| B0-34 | DESIGN, Milestone 0 | Git CPU and fan-out with a site killed and a hostile owner | M0-WP9 |
| B0-35 | DESIGN, Milestone 0 | Lazy test merges under polling | M0-WP9 |
| B0-36 | DESIGN, Milestone 0 | Agent refusals with raw tokens | M0-WP10 |
| B0-37 | DESIGN, Milestone 0 | Provenance parsing and hidden refs | M0-WP10 |
| B0-38 | DESIGN, Milestone 0 | Shard containers start in their site's regions and survive a deploy | M0-WP1 |
| BM-1 | DESIGN, Milestones 1 to 7 | git conformance with a site failing, linearizability at 24 hours | M1-WP8 |
| BM-2 | DESIGN, Milestones 1 to 7 | `gh pr` and `gh issue`, merge p99, stale `parent_tip` | M2-WP6 |
| BM-3 | DESIGN, Milestones 1 to 7 | Journey 4 revocations, learner rerun | M3-WP6 |
| BM-4 | DESIGN, Milestones 1 to 7 | `actions/runner` matrix with a site failed | M4-WP6 |
| BM-5 | DESIGN, Milestones 1 to 7 | Unreadable format, revocation mid-roll, `graft-ops` swap-back | M5-WP7 |
| BM-6 | DESIGN, Milestones 1 to 7 | Journey 5 drill, `ReplaceMember` with tokens revoked, lifeboat | M6-WP4 |
| BM-7 | DESIGN, Milestones 1 to 7 | Self-hosted install, restores, `Solo` to `Tri` | M7-WP8 |
| BF-1 | DESIGN, after the final pass (M2) | 5-PR merge-queue group plus a `PrSync` batch | M2-WP3 |
| BF-2 | DESIGN, after the final pass (M3) | Revoke a token with 100 pushes in admission | M3-WP2 |
| BF-3 | DESIGN, after the final pass (M3) | Suspended user's PAT push; one key from two edges | M3-WP3 |
| BF-4 | DESIGN, after the final pass (M4) | Runner killed mid-job | M4-WP3 |
| BF-5 | DESIGN, after the final pass (M4) | Stars, notifications, and a rebuilt `Inbox` | M4-WP2 |
| BF-6 | DESIGN, after the final pass (M5) | Replay of every `Release` entry after `Keys { vendor: None }` | M5-WP1 |
| BF-7 | DESIGN, after the final pass (M5) | `Rollback` to N and `Abort { intent, target: N }` after `FormatsEnable` | M5-WP3 |
| BF-8 | DESIGN, after the final pass (M5) | `Release` blackholed, `EdgePointer` outside the deployment | M5-WP2 |
| BF-9 | DESIGN, after the final pass (M7) | Member at `CHILD_GROUP_AT`, paginate on a managed cell | M7-WP4 |
| BF-10 | DESIGN, after the final pass (M1) | Chain creation from two edges, bootstrap config | M1-WP7 |
| BF-11 | DESIGN, after the final pass (M1) | Cold-cache group lookup after moves | M1-WP7 |
| BF-12 | DESIGN, after the final pass (M1) | Leader killed before reply, re-send by `RequestId` | M1-WP2 |
| BF-13 | DESIGN, after the final pass (M1) | No refusal from an uncommitted entry | M1-WP2 |
| BF-14 | DESIGN, after the final pass (M1) | No command forwarded on a missed receipt | M1-WP3 |
| BF-15 | DESIGN, after the final pass (M1) | Stored-token revocation and suspension with 1,000 queued | M1-WP7 |
| BF-16 | DESIGN, after the final pass (M1) | Two-release rule for messages, `Unsupported` as 503 | M1-WP6 |
| BF-17 | DESIGN, after the final pass (M1) | 10,000-ref atomic push and `COMMAND_MAX` | M1-WP6 |
| BF-18 | DESIGN, after the final pass (M1) | Rehydrate after archived inline packs, `IngestCursor` | M1-WP5 |
| BF-19 | DESIGN, after the final pass (M1) | Orphan sweep and marker repair | M1-WP4 |
| BF-20 | DESIGN, after the final pass (M2) | Snapshot of a 4 GB tier under load | M2-WP1 |
| BF-21 | DESIGN, after the final pass (M2) | 5-PR `HeadGreen` group with a failing PR and a leader kill | M2-WP3 |
| BF-22 | DESIGN, after the final pass (M2) | Non-required checks in the metadata tier, pruned | M2-WP2 |
| BF-23 | DESIGN, after the final pass (M2) | Re-requested required check blocks merge | M2-WP3 |
| BF-24 | DESIGN, after the final pass (M2) | `TestMerged` and `refs/pull/N/merge` | M2-WP5 |
| BF-25 | DESIGN, after the final pass (M2) | LFS and release blobs in two buckets | M2-WP4 |
| BF-26 | DESIGN, after the final pass (M2) | `ArchiveCache` rebuilt on a miss | M2-WP4 |
| BF-27 | DESIGN, after the final pass (M3) | `jti` revocation with 1,000 queued | M3-WP4 |
| BF-28 | DESIGN, after the final pass (M3) | Distinct learner member names | M3-WP5 |
| BF-29 | DESIGN, after the final pass (M3) | Rejoin by `InstallSnapshot` | M3-WP5 |
| BF-30 | DESIGN, after the final pass (M3) | Transfer across enterprises and placements | M3-WP3 |
| BF-31 | DESIGN, after the final pass (M3) | Scoped PAT, paired `ghu_` and `ghr_`, token DELETE | M3-WP2 |
| BF-32 | DESIGN, after the final pass (M3) | App JWT and OAuth app secret, no secret in `Identity` | M3-WP4 |
| BF-33 | DESIGN, after the final pass (M3) | Device flow with a leader kill, code replay and expiry | M3-WP3 |
| BF-34 | DESIGN, after the final pass (M3) | Collaborators and invitations | M3-WP3 |
| BF-35 | DESIGN, after the final pass (M3) | New SSH key and PAT listed with no `Audit` | M3-WP3 |
| BF-36 | DESIGN, after the final pass (M3) | `InstanceAuth` switch to OIDC | M3-WP3 |
| BF-37 | DESIGN, after the final pass (M4) | `merge_group` workflow and queue webhooks | M4-WP3 |
| BF-38 | DESIGN, after the final pass (M4) | Web app pages and error boundaries | M4-WP5 |
| BF-39 | DESIGN, after the final pass (M5) | `graft-ops` killed at each intent step | M5-WP4 |
| BF-40 | DESIGN, after the final pass (M5) | Second intent and missing `ReleaseUploaded` refused | M5-WP1 |
| BF-41 | DESIGN, after the final pass (M5) | Genesis against the trust anchor | M5-WP1 |
| BF-42 | DESIGN, after the final pass (M5) | Stale `prev` answered 409 | M5-WP1 |
| BF-43 | DESIGN, after the final pass (M5) | `EdgePointer` pin and clear | M5-WP2 |
| BF-44 | DESIGN, after the final pass (M5) | Undo validity and `Resume` after vendor revocation | M5-WP6 |
| BF-45 | DESIGN, after the final pass (M5) | Executor-key forgeries | M5-WP1 |
| BF-46 | DESIGN, after the final pass (M5) | `Rotate` of each `SecretName` | M5-WP6 |
| BF-47 | DESIGN, after the final pass (M5) | `release roll` end to end with `vault upgrade` | M5-WP5 |
| BF-48 | DESIGN, after the final pass (M5) | Uncommitted `StaleMode` with `Release` blackholed | M5-WP2 |
| BF-49 | DESIGN, after the final pass (M5) | Vault export endpoints and `jti` revocation | M5-WP5 |
| BF-50 | DESIGN, after the final pass (M6) | Site loss by executor-signed `ReplaceMember` | M6-WP1 |
| BF-51 | DESIGN, after the final pass (M6) | SSH host key rotation | M6-WP2 |
| BF-52 | DESIGN, after the final pass (M6) | Suspended user over SSH; SSH key revocation with 1,000 queued | M6-WP2 |
| BF-53 | DESIGN, after the final pass (M6) | Lifeboat enroll, decoder, and follower pulls | M6-WP3 |
| BF-54 | DESIGN, after the final pass (M4) | 2,500 polls on one broker past `BROKER_HELD_MAX` | M4-WP3 |
| BF-55 | DESIGN, after the final pass (M4) | Registration shard's leader site killed during a 256-job matrix | M4-WP3 |
| BF-56 | DESIGN, after the final pass (M4) | Every `RequiredCheck` concluded after commit-path and `RunOwner` kills | M4-WP3 |
| BF-57 | DESIGN, after the final pass (M4) | One slow hook beside 49 healthy hooks | M4-WP1 |
| BF-58 | DESIGN, after the final pass (M4) | `HookSequencer` site killed with deliveries pending | M4-WP1 |
| BF-59 | DESIGN, after the final pass (M4) | Repository, environment, and org secrets across both key rotations | M4-WP3 |
| BF-60 | DESIGN, after the final pass (M4) | Hosted-runner cap with overlapping `graft-runners` sweeps | M4-WP4 |
| BF-61 | DESIGN, after the final pass (M4) | Live log through `LogRelay`, then the relay killed | M4-WP5 |
| BF-62 | DESIGN, after the final pass (M7) | Metadata split under 300 commands/s | M7-WP4 |
| BF-63 | DESIGN, after the final pass (M7) | `RepoReset` before and after a split | M7-WP4 |
| BF-64 | DESIGN, after the final pass (M7) | `LeaveEvaluation` under load, and a vendor-signed one | M7-WP5 |
| BF-65 | DESIGN, after the final pass (M7) | GHES archive import fires nothing and keeps numbers | M7-WP6 |
| BF-66 | DESIGN, after the final pass (M7) | `RateCounter` error bound with 20 isolates and a killed counter | M7-WP3 |
| BF-67 | DESIGN, after the final pass (M4) | `Inbox` rebuild from `sources`, 10,000 stars and a refold | M4-WP2 |
| BF-68 | DESIGN, after the final pass (M4) | EU search residency and a killed `SearchShard` | M4-WP2 |
| L0-01 | AGENT-LAYER, Milestone 0 rows | Gray enam during the A1 replay | M0-WP12 |
| L0-02 | AGENT-LAYER, Milestone 0 rows | Sandbox containment | M0-WP14 |
| L0-03 | AGENT-LAYER, Milestone 0 rows | Token ceilings with raw tokens | M0-WP13 |
| L0-04 | AGENT-LAYER, Milestone 0 rows | Agent CI under `AutoReadOnly` | M0-WP14 |
| L0-05 | AGENT-LAYER, Milestone 0 rows | Prediction on 1,000 branch pairs | M0-WP16 |
| L0-06 | AGENT-LAYER, Milestone 0 rows | Predictor CPU under A1 and while its site leads | M0-WP16 |
| L0-07 | AGENT-LAYER, Milestone 0 rows | A2 oracles | M0-WP15 |
| L0-08 | AGENT-LAYER, Milestone 0 rows | MCP clients, consent, refresh | M0-WP15 |
| L0-09 | AGENT-LAYER, Milestone 0 rows | Trust view, 200 payloads | M0-WP13 |
| L0-10 | AGENT-LAYER, Milestone 0 rows | Provenance through a vault restore | M0-WP16 |
| L0-11 | AGENT-LAYER, Milestone 0 rows | Byte diff with and without the layer | M0-WP15 |
| L0-12 | AGENT-LAYER, Milestone 0 rows | Leases and the feed | M0-WP12 |
| L0-13 | AGENT-LAYER, Milestone 0 rows | Lease limits | M0-WP12 |
| L0-14 | AGENT-LAYER, Milestone 0 rows | Task rules | M0-WP13 |
| L0-15 | AGENT-LAYER, Milestone 0 rows | Gate and read permissions | M0-WP13 |
| L0-16 | AGENT-LAYER, Milestone 0 rows | Core tier alone | M0-WP12 |
| LM-1 | AGENT-LAYER, Milestones | AM1 gate | AM1-WP6 |
| LM-2 | AGENT-LAYER, Milestones | AM2 gate | AM2-WP4 |
| LM-3 | AGENT-LAYER, Milestones | AM3 gate | AM3-WP5 |
| LM-4 | AGENT-LAYER, Milestones | AM4 gate | AM4-WP4 |
| LF-1 | AGENT-LAYER, after the final pass (AM1) | Uninstall cancels tasks with no sweep | AM1-WP3 |
| LF-2 | AGENT-LAYER, after the final pass (AM1) | Unflagged `AgentApp` | AM1-WP2 |
| LF-3 | AGENT-LAYER, after the final pass (AM1) | Feed cursor, `wait`, and `paths` limits | AM1-WP4 |
| LF-4 | AGENT-LAYER, after the final pass (AM1) | `HIDDEN_CHARS` in every `graftyard_trust` resource | AM1-WP5 |
| LF-5 | AGENT-LAYER, after the final pass (AM1) | `graftyard-plan` block grammar | AM1-WP4 |
| LF-6 | AGENT-LAYER, after the final pass (AM2) | `token/scoped` grants and generations | AM2-WP1 |
| LF-7 | AGENT-LAYER, after the final pass (AM3) | `ControllerPointer` pin with `release/entries` blackholed | AM3-WP1 |
| LF-8 | AGENT-LAYER, after the final pass (AM3) | Forged `release/entries` refused | AM3-WP1 |
| LF-9 | AGENT-LAYER, after the final pass (AM3) | Token ceilings by App and repository | AM3-WP3 |
| LF-10 | AGENT-LAYER, after the final pass (AM3) | Fork-mode session URL | AM3-WP4 |
| LF-11 | AGENT-LAYER, after the final pass (AM4) | `graftyard apps init` without an agents account | AM4-WP2 |
| LF-12 | AGENT-LAYER, after the final pass (M0) | 5,000 held feed polls on followers, then after `place_learners` | M0-WP12 |
| LF-13 | AGENT-LAYER, after the final pass (M0) | Held polls on a follower that stops following | M0-WP12 |
| LF-14 | AGENT-LAYER, after the final pass (AM4) | Duplicate and stale `InsightJob`s | AM4-WP2 |
| LF-15 | AGENT-LAYER, after the final pass (AM4) | One `InsightJob` per `PathSet` entry across a leader change | AM4-WP2 |
| LF-16 | AGENT-LAYER, after the final pass (AM4) | Cluster names and labels as PRs close | AM4-WP2 |
| LF-17 | AGENT-LAYER, after the final pass (AM4, after base 7) | Metadata split with live `InsightVerdict` rows | M7-WP4 |
| LF-18 | AGENT-LAYER, after the final pass (AM4) | `/codemode` under `OrgPolicy.code_mode` | AM4-WP3 |
