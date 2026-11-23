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
- `tests/RESULTS.md` lists every run that counts toward a gate row so far, with its faults, checks, and results file. The status tables below give each row's state.
- Test drivers never print or log a credential: no token, key, or secret on a command line, in an environment dump, a URL, a crash trace, or a raw log. Before a results file is written, check its raw logs for credentials and remove any found. A token reached tool output twice in Milestone 2, once through `ps` and once in a crash trace written to a raw log. A token file is never read with a command that prints its contents, such as `cat`, `head`, or `cut`; pass its path to the driver or into git's environment instead. `wire-fix`'s session printed the first 20 characters of three `graft-m2b` user tokens into its tool output, and those tokens are being rotated on `am1-runs`.

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
| Workflows script | The script that runs the Workflows: `ReplaceMember`, `RestorePlacement`, owner placement moves, imports, network compaction, and the `LeaveEvaluation` group moves. It binds every site's member class and bucket, and it rolls on its own `SiteDay { Workflows }`. | `design/DESIGN.md`, Stack (Workflows), Shape ("Members reach each other through Durable Object bindings", "Sites and placement", "Chains" (`CHILD_GROUP_AT`), `DayTarget`) | M1-WP7 (owner placement moves) [inference], M5-WP4 (its site day), M6-WP1 (`ReplaceMember`), M7-WP5 (`LeaveEvaluation`) |
| Queue consumers | The `Consumer` variants (`Webhooks`, `ActionsTrigger`, `RunOwner`, `PrSync`, `MergeQueue`, `Notify`, `Search`, `Audit`, `Counts`, `Insights`), fed from leader cursors through Queues, plus the consumer that writes the change feed. The design puts the insights consumer in the site scripts. The location of the others is not stated [inference: the same site scripts]. | `design/SHARED.md`, "Event fan-out"; `design/DESIGN.md`, Shape ("Merge queue", "Lifeboat feed", "Reused with the additions below", "Dominant access patterns") | M2-WP5 (cursors, Queues, `PrSync`), M2-WP3 (`MergeQueue`), M4-WP1 (the rest), M5-WP5 (change feed) |
| `graft-ops` | Stateless Worker with no member classes. It executes signed intents, uploads releases from the release bucket with the customer's deploy token, owns every cron that calls the Cloudflare API and every rotatable secret, mints installation tokens, drains a site with its drain-only key, and commits `ReleaseUploaded`, `IntentStep`, `IntentDone`, `SiteRelease`, and `ReplaceMember` with its executor key. | `design/DESIGN.md`, Stack (Workers, Secrets Store), Shape ("Release group", "Identity"), "Install operations" | M3-WP4 (minting), M5-WP3 (executor) |
| SSH front | The edge's `connect()` handler, reached by a Spectrum TCP app on port 22 with the edge as Worker origin (`graftyard ssh spectrum`); an `SshFront` Durable Object class on each site script (one per site, `ssh#<site>`) that starts its container (image `sshfront`, `basic`, no internet) and pipes the socket to port 22; `graft-sshd` in the container (Go, no OpenSSH, git, or cloudflared); and `SshOutbound`, the site script's `WorkerEntrypoint` for the `keys.graft` and `git.graft` intercepts, which serves host keys from Secrets Store and forwards git to the edge over the `EDGE` binding with `{"ssh": true}` props. Off by default per install. | `design/DESIGN.md`, Shape ("SSH front"), journey 4, Stack (Spectrum) | M6-WP2 |
| Runner image and hosted runners | Unmodified `actions/runner`, one job per Sandbox, in a separate runner account. | `design/SHARED.md`, "Actions" (Hosted runners); `design/DESIGN.md`, Stack (Sandbox SDK) | M4-WP4 |
| `graft-runners` | Worker in the runner account that holds `HostedRunnersAppKey`. It reads hosted demand from the `Owner`-scope broker as the `graftyard-hosted-runners` App, calls `generate-jitconfig`, starts and destroys one Sandbox per runner, and starts none past `RUNNER_VCPU_STOP_PCT`. | `design/DESIGN.md`, Stack (Workers, Sandbox SDK, Cron Triggers), Shape ("Identity"); `design/SHARED.md`, "Actions" (Hosted runners) | M4-WP4 |
| Vault Worker | Worker in the vault account. It pulls the change feed, entries, snapshots, and packs through the edge's export endpoints with an install-scoped export token signed by the customer key, and writes them to R2 under 35-day bucket locks. `graftyard vault upgrade` updates it. | `design/DESIGN.md`, Shape ("Lifeboat feed"), "Install operations" (Vault), "Failure modes" (`vault`) | M5-WP5 |
| Lifeboat | Read-only workerd bundle with a pinned workerd version on customer hosts. It rebuilds from the change feed and the export endpoints, serves clones and reads, stores files through `graft-blob`, and publishes `LifeboatStatus`. | `design/DESIGN.md`, Shape ("Lifeboat feed"), "Install operations" (Lifeboat), "Deployment modes" | M6-WP3 |
| CLI (`graftyard`) | Operator binary: `init`, `sites add`, `sites add-spare`, `genesis`, `sign`, `install`, `placement set`, `vault init`, `vault upgrade`, `runners init`, `runners upgrade`, `admin bootstrap`, `release roll`, `lifeboat init`, `lifeboat enroll`, `import ghes`, `actions import`, `apps init`, `agents init`, and `ssh spectrum`. | `design/DESIGN.md`, "Usage (caller's view)", "Install operations", "Failure modes" (`actions state`); `design/SHARED.md`, "Surfaces, tiers, and oracles" (`rest:migrations`) and "Actions"; `design/AGENT-LAYER.md`, "Deployment modes" and journey A8 | Across milestones; each work package names its commands |
| `graft-mcp` | Worker at `mcp.<host>` in the install account, routed through `graft-front`. It is an MCP server and a small authorization server, and it calls only the edge, over a service binding. | `design/AGENT-LAYER.md`, Shape ("graft-mcp"), journey A7 | AM2 |
| Controller | Optional `agents` account with `graft-agents-front`, `graft-controller`, and its own `graft-ops`: a GitHub App backend reached by webhooks over HTTPS, Workflows per task, Sandboxes, and the outbound router. `graft-agents-front` pins controller traffic by `Pointers.controller`. | `design/AGENT-LAYER.md`, Shape ("Controller"), journeys A3 and A8 | AM3 |
| Insights consumer | Consumer in the site scripts, fed only by its own cursor. It runs `predict` on the shard and commits one `InsightVerdict` row per ref, with its `graftyard-insights` check runs and labels, through `POST .../insights` on the edge. | `design/AGENT-LAYER.md`, Shape ("Overlaps and prediction"), "API reference" | AM4 |
| Web app | React 19 on TanStack Start in SPA mode with ReUI's free components and HugeIcons' free icons, served by the edge from the HTML shell, reading the public GraphQL and REST endpoints. Source in `web/`. | `design/SHARED.md`, "Frontend and native app" | M4-WP5 ([inference], as `design/DESIGN.md`, "Milestones 1 to 7", tags it) |

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
containers/ssh      graft-sshd (image sshfront)
containers/runner   runner image
lifeboat/           workerd config, pinned version, graft-blob
cli/                graftyard
agents/controller   controller App backend and its outbound router
tests/m0/           throwaway Milestone 0 harness, never shipped
```

## Milestone 0: measure the platform and the protocol

About six weeks on one throwaway account with three site scripts, before any git code. Every package runs in that one account, with no second account and no trial account or zone (`decisions/DECISIONS.md`, "Milestone 0 runs in one account"). Any fail value sends the design back (`design/DESIGN.md`, "Milestone 0"). The layer adds its rows to the same milestone (`design/AGENT-LAYER.md`, "Milestone 0 rows").

### Milestone 0 status (2026-10-31)

Every package ran on prototype code in the one Milestone 0 account, M0-WP6 and M0-WP7 on its one Pro zone, `graftyard.dev`. Results are in `tests/m0/results/`, one file per package, and each fix the runs found went into the design first (`decisions/DECISIONS.md`, "Milestone 0"). No row hit its Fail value. Milestone 0 is closed for starting M1 (`FRAME.md`, "Operator decision, 2026-10-31, Pro only and no GHES"; `decisions/DECISIONS.md`, "Decisions for a Pro-only build with no GHES, 2026-10-31", "M1 starts with the between rows carried forward"): the rows below carry forward to the owner named, with no gate number changed, and a Fail value on any of those reruns still sends the design back. Rows that need a plan, an instance, or accounts the operator does not have stay not run, owned by whoever first has them, and gate no M1 work. Rows still short of their Pass value, each with its owner:

| Row | State | What closes it |
| --- | --- | --- |
| B0-11 | Passes: 0 violations on 5,742,736 operations and 0 lost acknowledged writes in 48 hours, with a 100 ms clock allowance across client colos that the run did not measure; fault coverage on site C is thin (F36). Rechecked with the fixed segmented checker, the verdict is unchanged at 0 ms and 100 ms: none of its 4 no-reply writes was ever observed, so dropping them, as M0's check did, changed nothing here, though it is not "only stricter" in general (`tests/m1/results/porcupine-seg.md`). Its 24-hour rerun on the M1 code, `lin24`, had fault counters in all 88 isolates and recorded clocks, and did not pass, because an early ack was not ruled out; the second rerun, `lin24b`, passed with member-side ack evidence, which answered that as a client-clock artifact ("Milestone 1 status", BM-1; `decisions/DECISIONS.md`, "F44 answered: the fast acks were a client-clock artifact") | Done (`lin24b`) |
| B0-01 | Passes on the Rust host (`m0-carried`, `graft-m3c`, 2026-11-11): 6 of 6 one-minute runs at 100 commands/s, commit p99 244 to 377 ms, 0 failed; M1-WP2's host had passed 2 of 6 (`tests/m0/results/carried.md`) | Done |
| B0-02 | **Passed** on `m0-carried-8` (`graft-m3d`, standard-4 shards, 2026-11-13, `tests/m0/results/carried-8.md`): 8 of 8 at 300 and 8 of 8 at 330, commit p99 249 to 579 ms, on main `84d29c2` (F67 rules 10 to 13) plus an AppendEntries pipeline (`PIPELINE` 4, stopped only on a lossy link), the `LOOP_LAG` step-down, `ENTRY_SOFT` at 100 KB and `APPEND_BYTES_MAX` at 100 KB. Heavy turns stayed at the 60 KB tree's level (28 `/commit` turns of 80 ms or more over 16 runs against 24; CPU p999 23 to 49 ms), and confirm p99 was 108 ms at 80 to 100 KB and 131 ms at 100 to 130 KB. Long sweep (live half 4,000 seeds at an absolute 30 s), Porcupine, `long_liveness`, plants 15 of 16 and the full checks pass. Before the two size changes the same code passed 12 of 16. Merged to main (25ea4252) after LM-1's journey 9 A/B (`tests/am1/results/lm1-memory.md`, round 11). Open, outside the gate: one run's `LOOP_LAG` trip moved the leader to weur, where the apac voter's link was lossy at these call sizes | Done |
| B0-07 | Passes on the Rust host (`m0-carried`, `graft-m3c`, 2026-11-12), with `test-faults` stand-ins for `graft-ops`'s drain and `SiteHealth`, which the hosted code does not have yet: 3 days, C detected 3.6 to 3.7 s after the fault, 235 of 235 groups led on C moved within 20 s (max 17.2 s), 0 failed writes, never two sites drained (`tests/m0/results/carried.md`). M0's one group at 20.2 s stays unexplained for the prototype | Done for the member path; M5-WP4 reruns it with the real `graft-ops` drain |
| B0-20, B0-21 | Ran on the Pro zone `graftyard.dev`: the 100 MiB cap holds for sized and chunked bodies, and git over it sees a bare 413, as the 413 decision accepts. Business and Enterprise not run; their caps (200 MB, 5 GB) stay [doc] | The first install on that plan |
| B0-24a | History: the retired `cloudflared` option ended between pass and fail, 99.587% of 28,347 sessions over 24 h with 143 kills; 85 of 117 failures within 2 min of a kill, the rest Tunnel-edge drops (background near 99.89%); 0 acknowledged pushes lost. Its 99.9% gate carries to the Spectrum ingress with kills (not yet run). Gates only M6-WP2, not M1 | M6-WP2 on the Spectrum ingress |
| B0-24b | Dropped 2026-11-19: Spectrum to a Tunnel virtual network is no longer the Enterprise ingress; the Spectrum Worker app serves every plan (B0-24c, B0-24d [measured]) | Nothing |
| B0-29 | Passes in 5 of 8 runs; misses at 602 and 625 ms track placement (first pass, `r2` and the `sd3r1` variant) and 809 ms tracks storage stalls on the leader (fifth pass, `p5r1`) | LM rerun on real code |
| B0-34 | Partial pass at reduced scale, with the composed-miss rules now in the design | A full-scale run with the warm pool |
| L0-06, L0-11, L0-04 | Partial: predictor without other git work; byte diff without a GHES oracle, which becomes the pinned `ghes-3.22` descriptions and github.com [inference]; agent CI without an Actions engine | LM rows that rerun them on real code (LM-4, LM-1, LM-3) |
| L0-07 | Vendor cells not run: they need vendor accounts and a public host; the documented-behavior drivers are the evidence (Codex's documented permissions fail the check cells). Does not block M1; no LM gate names it | The operator's vendor accounts |
| L0-08 | The `ghr_` cell needs a GHES 3.22 instance, and none exists. AM2-WP1 builds A4's fallback, so the cell becomes the held-token refresh and A4 gates nothing | LM-2's rerun of L0-08 on real code (AM2-WP1) |
| LF-12 | Passes on real code after `am1-fix` (lf1-6, AM1-WP6 status): leader commit p99 474 ms placed and 710 ms before, 5,000 of 5,000 polls held on 11 learners and 0 on voters, and 0 failed AppendEntries calls of any kind in 165 `/stats` samples, so zero `overloaded` (`tests/m0/results/carried.md`, LF-12). On the prototype it was 1 pass and 3 between in 4 runs | Done |

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

**Build (as run, 2026-10-30 to 31; the path has since retired).** Tunnel and cloudflared in three containers, with `graft-sshd` calling `ssh_outbound` for writes, reached by each ingress: a Tunnel public hostname of type `ssh` through `cloudflared access ssh` for the Pro and Business option, and Spectrum on port 22 to the Tunnel's virtual network for Enterprise (`design/DESIGN.md`, Shape, "SSH front"; journey 4, step 3; `FRAME.md`, "Operator decision, 2026-10-31, SSH ingress"). Run on the zones added to the Milestone 0 account; a plan it does not have is recorded as not run, as in M0-WP6, and B0-24b waits for the first Enterprise install, which runs it in its own account.

**Depends on.** M0-WP2, M0-WP6.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| B0-24a | The Pro and Business SSH option: clients through `cloudflared access ssh` as their `ProxyCommand` to a Tunnel public hostname of type `ssh`, then cloudflared in three containers, with writes through the outbound handler and one container killed every 10 minutes for a day. On Pro, 2,634 of 2,637 sessions (99.886%) over 2.2 h with 13 kills, then 28,230 of 28,347 (99.587%) over 24.0 h with 143 kills: between the lines. 85 of the 117 failures came within 2 min of a kill, and the other 32 were Tunnel-edge drops (stream closed before the banner, no banner within `ConnectTimeout`, 90 s timeouts), a background near 99.89%. Failures are dominated by sessions in flight on the killed replica, so a 10-minute kill period puts a floor of about one failure per kill under the rate. 9,418 pushes acknowledged, all 9,418 in the group log [measured, `tests/m0/results/m0-wp7.md`, B0-24a; `tests/m0/results/raw/wp7/b0-24a-r4/summary.json`]. Carried to M6-WP2's rerun on real code | At least 99.9% of sessions succeed | Under 99%, which leaves Pro and Business HTTPS only |
| B0-24b | Dropped 2026-11-19. Enterprise native SSH: Spectrum on port 22 to the Tunnel's virtual network, then the same containers, handler, and kills for a day. Never run: it needed an Enterprise zone. On Pro the Spectrum API refused a Tunnel origin (10012, 13002) [measured, same] | At least 99.9% of sessions succeed | Under 99%, which drops native SSH on Enterprise |

**Flip condition.** Spectrum refusing a Tunnel origin on Pro fired on 2026-10-31 and gave the `cloudflared` option and the Enterprise Tunnel ingress. That split was reversed on 2026-11-19 on new evidence: B0-24c, a Spectrum TCP app on port 22 with a Worker origin on the Pro zone, whose `connect()` held 600 s byte-exact [measured, `tests/m0/results/m0-wp7.md`, B0-24c], and B0-24d, the Durable Object and container hops [measured, `tests/m6/results/m6-wp2.md`] (`decisions/DECISIONS.md`, "SSH goes through a Spectrum Worker application to the SSH front (2026-11-19)"). B0-24a stays as the record of the retired option, and its gate carries to the Spectrum ingress in M6-WP2; B0-24b is dropped. A failing gate on the Spectrum ingress leaves SSH off, with HTTPS the default. `alternatives/AWS.md` condition 5 names an SSH failure as a reason to prefer AWS.

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
| B0-29 | Agent fleet replay (journey 9) on one `Tri` group, leader enam. 500 agents push distinct refs every 2 min (40 KB mean, 5% at 1 MiB), 27.6 commands/s with PrSync, test-merge records, check runs, task updates, and lease renewals, 45 feed polls/s on followers and learners, 10 merges/min, 33 ref reads/s, 200 API reads/s. Commit p99 on this row is set by storage stalls on the leader: of the seven fourth-to-seventh-pass runs, the five with 0 to 4 failed AppendEntries in 600 s passed at 378 to 531 ms, and the two with 10 and 12 missed at 798 and 809 ms [measured, `tests/m0/results/m0-wp9.md`, fourth to seventh passes]. The line is one 600 s run, at 800 ms since the operator moved it on 2026-11-19, and M1-WP2 reruns it on real code | Commit p99 at most 800 ms (`FRAME.md`, "Operator decision, 2026-11-19, B0-29's commit line"), judged as the median of three runs on fresh placements ("Operator decision, 2026-11-20"), merge p99 at most 2 s, zero `Stale` on distinct refs, leader under 600 req/s with zero feed polls, API read p99 at most 2 s with at least 99% served ("Follower read") | Commit p99 over 1 s, any queue merge waiting over 10 s, any feed poll on the leader, or API read p99 over 5 s |
| B0-30 | Hot-group storage. 30 days of the journey 9 replay on a group seeded with a 4 GB metadata tier | Member SQLite under `CHILD_GROUP_AT` with zero `SQLITE_FULL`, and archived entries readable for `RestoredTo` | Any `SQLITE_FULL`, or member SQLite over `CHILD_GROUP_AT` without a child group taking the tier |
| B0-31 | Same replay with the group homed on apac (its committed config's `home` set to apac by `/move` before the row starts, so the hand-off back to `home` keeps the leader there) and every agent push at 1 MiB, `INLINE_MAX_HOT` on, then with every bucket's placement failing, under the design's `STEP_DOWN` with no variant. The placement-failing half gates Human and MergeQueue latency, not the leader's site: the apac leader may move once on the Human packs that the outage rule keeps inline. On the final Milestone 0 code both halves passed: buckets up, the apac leader held 600 s with commit p99 598 ms and every agent push staged; placement failing, Human and MergeQueue p99 1.11 s with 0 of 287 refused, 2,500 of 2,500 agent pushes refused at once in the `ng` shape, and one `STEP_DOWN` from apac at 8.9 s (6 and 3 failed calls) [measured, `tests/m0/results/m0-wp9.md`, B0-31, eighth pass] | Commit p99 at most 2 s with buckets up, the bound for a group led on another continent than its owner's identity groups (`FRAME.md`, "Operator decision, 2026-11-18, a separate commit bound for rehomed groups"). While placement fails, Human and MergeQueue commit p99 at most 3 s and 0 of them refused, and agent pushes over `INLINE_MAX_HOT` refused at once in the `ng` shape | Over 3 s, or any Human or MergeQueue push up to 1 MiB refused while placement fails |
| B0-32 | Burst of 500 agent pushes of 256 KiB each in 1 s with merges queued, admission on. The burst rides a hot group, so the row tests `INLINE_MAX_HOT`, which stages each 256 KiB pack before `AGENT_INLINE_ENTRY` is reached; a burst on a quiet group is covered by the budget's B0-33 evidence (fresh groups' first entries capped at 128 KiB, seventh pass). On the final Milestone 0 code 505 of 505 burst pushes committed within 5 s (client p99 1.40 s, stage p99 1.19 s), the burst's merges took 129 to 227 ms, each in the next entry (up to 2.30 s at a 256 KiB cap), queued agent bytes peaked at 119 KB, and there were 0 leader changes and 0 `suspect` [measured, `tests/m0/results/m0-wp9.md`, B0-32, eighth pass] | Every push acknowledged within 5 s or answered in an HTTP 200 with `ng <ref> agent admission full, retry after <n>s`, queued agent bytes under `AGENT_QUEUE_MAX`, every merge in one of the next 2 entries, zero leader changes, and zero `suspect` reports | Any lost acknowledgment, any other refusal shape, an object reset from memory, a merge delayed over 2 s, any leader change, or any `suspect` report |
| B0-33 | 5,000 agents on one group, 252 agent-class commands/s offered with caused commands classed, plus one user token writing 100 commands a minute, admission on. With `outcome` folded into `commit`, `AGENT_INLINE_ENTRY` on every group, and `hot` from the log, three far-flung runs (ATL leader, weur and apac followers, fresh groups) held their leader with 0 `suspect`, Human and MergeQueue p99 746 to 943 ms, 253 agent commands/s admitted, and the leader at about 500 req/s [measured, `tests/m0/results/m0-wp9.md`, B0-33, seventh pass]. With `commit_fail_min` added, a fourth run passed again at Human and MergeQueue p99 674 ms, 250 agent commands/s, and 0 leader changes; no window reached the 5% rate [measured, same, eighth pass]. Before the fold the leader served 750 to 950 req/s and 2 of 3 runs lost it by election (fifth pass). The zero-leader-change claim is measured for this mix, with a margin of 57 to 326 ms under 1 s, least in the first 30 s | Human and merge-queue commit p99 at most 3,912 ms, two loaded rounds at p99 plus one median round on sabA (`FRAME.md`, "Operator decision, 2026-11-22, B0-33's commit line and three-run medians"), judged as the median of three runs on fresh placements, at least 140 agent commands/s admitted, the user token admitted as `Agent`, every refusal in the git `ng` or REST 429 `retry-after` shape, zero leader changes, and zero `suspect` reports | Any leader change or `suspect` report, any merge-queue command waiting over 2 s, or under 100 agent commands/s admitted, which reopens ref lanes with a parent `Abort { txn }` record (`design/AGENT-LAYER.md`) |
| B0-34 | Git CPU for one 5 GB network under the journey 9 replay, with the tip moving at 10 merges a minute along real history, then one warm site killed, then a second network that turns hot with the API unreachable, then a hostile owner making three of its own networks hot beside it | k set in band from `NetworkLoad` and the hot pool, at most 8 per site, the hostile owner held to its per-owner pool budget, human push p99 at most 3 s beyond transfer, clone-cache hits' time to first byte p99 at most 1 s in steady state, and every clone's time to first byte at most 5 s through the kill and cold phases (`FRAME.md`, "Operator decision, 2026-11-20, leader-side render, three gate lines, and load clients on Cloudflare") | k over 8, any Cloudflare API call, the monorepo's k reduced by another owner, or time to first byte p99 over 5 s |
| B0-35 | Lazy test merges, with the base moving 10 times a minute under 500 open PRs and 500 clients polling their PR every 5 s | Test-merge vCPU within `TEST_MERGE_BUDGET`, Actions dispatch p99 at most 10 s, and zero computes from list fields | Over budget, or dispatch p99 over 30 s |

**Flip condition.** B0-33's Fail value reopens ref lanes with a parent `Abort { txn }` record (`design/AGENT-LAYER.md`, "Alternatives considered", ref lanes). B0-29 and B0-34 decide the A1 verdict and the guessed git CPU constants (`design/AGENT-LAYER.md`, "Open questions and risks").

**B0-34's every-clone line and the 90% rule (2026-11-20).** "Every clone's time to first byte at most 5 s through the kill and cold phases" is a count line, so the 90% rule (`FRAME.md`, "Operator decision, 2026-11-16, 90% gates and a daily spend limit"; `AGENTS.md`, "How to pick work", item 5) reads it as at least 90% of the clone probes, hits and misses together, at most 5 s, in each of the kill and second phases on the client clock. The hits line is a p99 and is judged at 1 s. `tests/fanout/clonelines.py` writes both lines into the verdict, as measured and with harness outages removed, and prints the share against 100% beside it. From the first Load34 row on, the clients run inside Cloudflare (`decisions/DECISIONS.md`, "`load34`").

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
| L0-12 | Leases and the feed. The A1 replay with a lease per agent and leader kills, then a 30-minute stall with every member running and member calls blackholed, then an idle 60 s lease followed 10 minutes later by an overlapping Exclusive acquire, then 100 conflicting pushes and 100 conflicting REST ref writes into another task's Exclusive scope under `AgentsExclusive`, run once per actor kind: Graftyard-run App, confined vendor App, flagged App without `confine`, `Graftyard MCP`, and a human user token | No two holders hold overlapping Exclusive leases for longer than `DRIFT` (500 ms), and measured member clock offsets stay under 500 ms. Every lease live before the stall is live after it, with `expires` moved by the recorded stall. The late Exclusive acquire succeeds. 500 long-pollers see each event within FEED_BATCH plus 2 s (3 s) at p99 outside leader kills. 100 of 100 refused for each of the two `ActorKind::Agent` kinds, 0 of 100 for the other three | Any overlap over 500 ms, any lease expired by the stall, the late acquire refused, feed p99 over 3 s, or a refusal count off for any kind |
| L0-13 | Lease limits. Exclusive acquires of `**`, `*`, and `*/x`, one with no live task, and a second for one task; an App-token acquire with no session; 600 plan blocks from accounts without push access, then 600 from writers; a holder whose push access is removed; an App uninstalled while it holds leases and tasks; an Exclusive lease renewed for 5 h | 100 of 100 refused for each bad acquire. Zero plan leases from non-writers and at most 500 from writers. The removed holder's lease refuses nothing at the next `decide`. Uninstall releases every lease and cancels every task. The lease reads as Shared after 4 h | Any accepted, any plan lease from a non-writer, or an Exclusive lease past 4 h |
| L0-16 | Core tier alone. Pushes, minting, and lease checks on the A1 replay with the metadata tier unloaded (a merge reads its pull row and writes `Merged`, both Metadata, so it waits for the tier) | Every check passes or refuses with zero metadata loads | Any metadata load on those paths |
| LF-12 | A1 replay on one `Tri` group with no learners, plus 5,000 held feed polls with `paths`, then again after `place_learners` runs. On the final rules, with the own-stall exclusion and the all-lagging `Retry-After` of `FEED_BATCH_ALL`, 4 runs gave 1 pass and 3 between the lines, with no fail, no step-down, and every other line passing in every phase. The misses trace to Durable Object storage confirm stalls on the leader or the near follower and to the far follower's round trip, as for B0-02 and B0-29 [measured, `tests/m0/results/m0-wp12.md`, LF-12, tenth pass]. The gate is one run, as B0-29's is | Commit p99, measured at the group's leader (admission to resolution), at most 600 ms once `place_learners` has run and at most 1 s before it, zero `overloaded` replies to AppendEntries, at most 1,000 polls held per member with the rest answered 429 with `Retry-After`, zero polls on the leader, and zero on followers after placement | Commit p99 over 1 s once `place_learners` has run or over 2 s before it, any `overloaded` replication reply, any member above 1,000 held, a refused poll without 429 and `Retry-After`, any poll on the leader, or any poll on a follower after placement |
| LF-13 | 1,000 polls held on a follower, then its member calls failed while commits run and leadership moves to the other follower, so the new leader's first round leaves a gap, then its log truncated past the follower for an `InstallSnapshot` catch-up (one leader re-sends from the follower's match, so dropped rounds alone leave none, `crates/raft/tests/lf13.rs`; hosted, the follower's calls are severed until it stands behind the other follower, then leadership moves there, `tests/am1/results/lf13-m3c.md`) | Every held poll returns within 1 s of the follower's stop, when it learns it is behind, with its cursor and no events, new polls are refused until it catches up, and no event is lost; the time from the fault's lift is reported too (A26, `decisions/DECISIONS.md`, "LF-13's 1 s runs from the follower's stop") | Any poll held or accepted while it is behind, or any event skipped. Between the lift and its stop the follower cannot know it is behind: it admitted 0 to 2 of 50 probes there, and its stop answered them with nothing skipped [measured, `graft-m3c`] |

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
| L0-15 | Gate and read permissions. An MCP client with `WriteApproveMerge` approves an agent PR. Code Mode calls approve and merge through `codemode.request()` without that scope. A vendor's named requester approves its PR. Anonymous, `metadata: read`-only, and over-limit feed polls, and `/log` and prompt reads with `metadata: read` only | `AgentGate` stays unsatisfied in all three cases, and Code Mode is refused 100 of 100. Feed polls get 401, 404 (no `contents: read`, as task and lease reads answer), or 429, the leader sees zero feed requests, and `/log` and prompts return 403 | Any gate satisfied, any Code Mode approve or merge, or any read served |

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
| L0-08 | MCP. Three 2026-07-28 clients and one 2025-11-25 client, CIMD, PKCE, consent, 30 calls each | 100% succeed, tool overhead p50 at most 50 ms, 100 of 100 wrong-audience tokens refused, a refreshed token keeps its scope 100 of 100, a replayed refresh revokes the grant, a framed or skipped consent issues no code, and a refresh succeeds with the unscoped `ghu_` held, which is deleted after it | Any wrong-audience token accepted, any refresh wider than its grant, or any code without consent |
| L0-11 | Byte diff. 1,000 REST and GraphQL requests from unmodified clients, with and without the agent layer deployed, and the same requests against a GHES 3.22 oracle: with no GHES instance, the pinned `ghes-3.22` OpenAPI and GraphQL schema for shape, and github.com for behavior on operations whose descriptions match, weaker than an appliance [inference] | With and without the layer: identical except request ids and times. Against the oracle: every difference listed, and none in a field a client reads | Any other difference with and without the layer, or an unlisted oracle difference |

L0-08's refresh cell checks the held-token behavior, which is correct whether or not the `ghr_` survives the delete, so A4 no longer gates it (`decisions/OPEN-QUESTIONS.md`, A4; `decisions/DECISIONS.md`, "Decisions for a Pro-only build with no GHES, 2026-10-31").

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

### Milestone 1 status (2026-11-04)

Milestone 1 is closed: every gate row below is Done, the last being BM-1's 24 h half on `lin24b` (2026-11-04).

Every row ran on the test install `graft-m1-site-a/b/c` in the Milestone 0 account, or, where the table says so, in the crate's simulator or with a stub. Results are in `tests/m1/results/`, one file per work package, and each design fix they found went into the design first (`decisions/DECISIONS.md`, "Decisions from the M1 builds, 2026-11-01", "BM-1 and the M1-WP8 fixes, 2026-11-02", "The hosted closing runs, 2026-11-02", and "BM-1's 24-hour half and F43, 2026-11-02").

| Row | State | What closes it |
| --- | --- | --- |
| BF-10 | Pass on Durable Objects: 1,000 of 1,000 chains with one group from two edges at once, and 100 of 100 during an owner move with the bootstrap claim (`m1-wp7.md`) | Done |
| BF-11 | Pass on Durable Objects: 1,500 of 1,500 cold-cache requests found the group after a member retry, a `ReplaceMember` stand-in, and an owner move (`m1-wp7.md`). `ReplaceMember` without its Workflow, spare, or `Joint` | Done for M1; the full `ReplaceMember` is M6-WP1 |
| BF-12 | Pass on Durable Objects: 10,000 of 10,000 kills kept their outcome on the register, and 1,000 of 1,000 on `Push` through the git edge (`m1-wp2.md`, `m1-wp6.md`) | Done |
| BF-13 | Pass on Durable Objects: 0 refusals from uncommitted entries in 400 rounds, plus 120 with early AppendEntries; Porcupine `Ok` (`m1-wp2.md`) | Done |
| BF-14 | Pass on the hosted edge: 2,400 commands across `Push` and identity (`PutAccount`) commands each missed its first receipt, half held 2.6 s and half dropped, and 0 were forwarded to a second member; also 0 in the simulator (`m1-wp8.md`, "Gate rows closed"; `m1-wp3.md`). The misses were injected by the `test-faults` build, not real network loss of a reply | Done |
| BF-15 | Pass on Durable Objects: 1,000 of 1,000 queued commands refused 401 after a revocation, and after a suspension (`m1-wp7.md`) | Done |
| BF-16 | Pass on the real groups (`m1-wp6.md`) | Done |
| BF-17 | Pass on the real groups, 12 of 12 rows (`m1-wp6.md`) | Done |
| BF-18 | Pass with a real `ChainLog` on a real repository group (run 2): `git fsck` clean on both shards, 9 of 9 clones matched, and the second shard served the newer pushes. Run 1 failed on a thin stored pack in the promoted range, now a design rule (Shape, "Packs"). Stubs: a test route triggers the archive, a key in one bucket is not re-put, `last_promoted`'s index is an upper bound, `consumed` is empty, and `RepoFold::check` requires every archived range to be promoted (`m1-wp5.md`, "BF-18 on the hosted install") | Done |
| BF-19 | Pass on the hosted cron with the real `request` table: 0 of 1,000 committed packs deleted, 1,000 of 1,000 missing markers repaired, and 1,000 of 1,000 failed placements deleted, none before 24 h. Stub: the 24 h ran on the sweeper's injected clock, not a day of waiting (`m1-wp4.md`, "BF-19 on the hosted cron") | Done |
| BM-1, git half | Pass with site 0's member class failing every call for the rest of each run: `clients.sh` 29 of 29 on git 2.39.5 and 2.51.0; `t/` on 2.51.0 and 2.56.0 lost no baseline pass except t5549 #2, which sets a server-side `receive.hiderefs` the host does not apply [inference] (`m1-wp8.md`). It also passes a gray fault, site 0's calls failing 30%: `clients.sh` 29 of 29 on 2.39.5 and 2.51.0, every leader move an election and no `STEP_DOWN` on any member (`m1-wp8.md`, "BM-1 gray-fault rerun"). Not covered: `t/` under the gray fault, other rates or sites, and a no-fault `t/` baseline on 2.39.5 (compared with 2.51.0 instead) | Done; the 2.39.5 baseline goes with BM-1's next rerun |
| BM-1, 24 h linearizability | Pass on `lin24b` (`tests/m1/lin/raw/lin24b.report.json`; `m1-wp8.md`, "The 24-hour half, 2026-11-04"; `m1-wp2.md`, "The 24-hour rerun's result, 2026-11-04"): `repo:934783`, 2026-11-03T00:55:07Z to 2026-11-04T00:55:07Z, 2,850,523 operations, 0 without a reply, p50 314 ms, p99 952 ms, under 4,176 resets (2,795 of a leader), 2,752 transfers (855 abandoned), and fault counters in 97 isolates. Porcupine Ok on all 6 keys widened 100 ms and 153 ms (the measured 305 ms ATL-SIN bound, halved); on the recorded clocks Illegal on k0 to k4 in 20 segments, 18 reads or pending writes of client rows lost in 15 of 12,535 cut chunks (Ok with them restored) and 2 cross-colo ties needing 1 ms and 4 ms of skew inside the measured bounds, none same-colo. 0 of 904,294 acknowledged writes lost, three identical logs of 513,232 entries, 0 of 805,201 refused CAS writes in the log, the member-side ack check 0 violations on 904,294 writes, and B0-11's same-clock early-ack test 0. That answers F44 as a client-clock artifact (`decisions/DECISIONS.md`, "F44 answered: the fast acks were a client-clock artifact"). Before it, `lin24` did not pass: on `repo:915635`, 24 h to 2026-11-02T18:27Z: 2,739,691 operations, 0 without a reply, under 5,547 resets, 3,743 transfers (1,131 abandoned), and faults in 88 isolates. Porcupine Ok on all 6 keys widened 100 ms and 275 ms (the measured 550 ms LHR-SIN bound, halved); on the recorded clocks Illegal on k2, k4, and k5, all 5 segments reads of writes whose client rows were lost in 5 of 12,518 cut chunks, and Ok with them restored. 0 of 869,257 acknowledged writes lost, 0 of 774,418 refused CAS writes in the log, three identical logs of 479,965 entries, and B0-11's same-clock early-ack test 0. An early ack was not ruled out: entry 398032 needed 93 ms of ATL-LHR skew against a measured 49 ms, and 17 writes were acknowledged faster than any quorum could commit them on their own client's clock (`m1-wp8.md`, "The 24-hour half, 2026-11-02"; `tests/m1/lin/raw/lin24.report.json`). `lin24b`'s 30-minute smoke had passed: 57,137 operations, Porcupine Ok on the recorded clocks and both widenings, and the ack check Ok on 18,225 of 18,225 writes (`m1-wp8.md`, "The 24-hour half, 2026-11-03"). Every history is checked per key with Porcupine's segmented mode, which now cuts each key on its own and keeps a no-reply write that a later operation observed; the old mode dropped every no-reply write, which can make a linearizable history look Illegal, and cut all keys of a file as one list. Rerun on the fixed checker, lin24's recorded-clock and 100 ms verdicts are unchanged; the 275 ms check needed over 7 GB for one key and was not rerun, and stands because its input has no no-reply writes and one key per file, the path both versions share (`tests/m1/results/porcupine-seg.md`). `lin24b`'s finish builds the fixed checker. `lin24b`'s pass rule was zero Porcupine violations, no acknowledged entry lost, and zero violations of the member-side ack check's six rules (`tests/m1/results/m1-wp2.md`, "Member-side ack evidence, 2026-11-03") | Done |

Milestone 0's carried rows (B0-01, B0-02, B0-07, B0-11's note) keep their owners in "Milestone 0 status"; B0-11's rerun is BM-1's 24 h row above, now passed. B0-01 on the Rust host with early AppendEntries passed 600 ms in 2 of 6 runs and sat between the lines in 4, level with the prototype's same-hour controls (`m1-wp2.md`, "B0-01 on the new host"). On the current host (`m0-carried`, 2026-11-11 and -12) B0-01 passed 6 of 6, B0-02 6 of 8 at 308 commands/s, and B0-07 235 of 235 groups within 20 s on test-faults stand-ins for the drain (`tests/m0/results/carried.md`). B0-02's latest gate (`m0-carried-6`, `graft-m3d`, 2026-11-13) passed 7 of 8 at 300 and 6 of 8 at 330 (`tests/m0/results/carried-6.md`).

### M1-WP1. Site scripts and placement

**Build.** `Site`, `SiteId`, `Region`, `Jurisdiction`, `SiteLink::Binding`, `Member` with `slot` and `generation` and names of the form `{chain}#{generation}#{kind}{slot}`, `MemberKind::Full` and `MemberKind::Witness`, `Placement` built only by `Placement::new` or `Placement::from_record`, `PlacementName`, `PlacementRecord`, `SystemChain`, `MIN_PAIR_RTT`, per-member `Rtt` rows, and the hint table compiled into callers (`design/DESIGN.md`, Shape, "Sites, not services, are the failure unit" and "Sites and placement"). The group lookup rule: the cached `GroupConfig`, then the bootstrap names built from the chain's `PlacementRecord`, then the chain's row in each record site's `MemberRegistry`, then `moving_from`. `MemberRegistry` and `RegistryRow`, written by a member before it stores its first entry (Shape, "Release group"). Each site script binds the member classes and buckets of every active, spare, and residency site with `script_name` (Shape, "Members reach each other through Durable Object bindings"). Placement failures retry with a new member generation ("Install operations", Placement failures). CLI: `graftyard init`, `graftyard sites add`, `graftyard sites add-spare`, `graftyard placement set`.

**Depends on.** M0, closed for starting M1 with its between rows carried forward to their named reruns (`decisions/DECISIONS.md`, "Decisions for a Pro-only build with no GHES, 2026-10-31", "M1 starts with the between rows carried forward").

**Acceptance.** No gate row of its own. It carries the M1 gate (BM-1, owned by M1-WP8) with one site's member class failing mid-run, and the placement records that BF-10 and BF-11 (M1-WP7) route through.

### M1-WP2. Raft group core

**Build.** `ChainId`, `Term`, `Index`, `Digest`, `LogEntry`, `EntryBody`, `GroupConfig` (`Normal`, `Joint`), `MemberState`, `LEASE`, `ELECTION_AFTER`, `DRIFT`, `CALL_DEADLINE`, `COMMIT_RETRIES`, `RESET_QUARANTINE`, pre-vote, `commit`, `outcome`, `outcome_by_request`, `Committed`, `Admitted`, `COMMIT_HOLD`, `Ticket`, the `Command` trait, `Acked` with `QuorumProof`, `verify`, `fresh`, and `ReadView` (`design/DESIGN.md`, Shape, "Chains", "The write path", "The read path"). Bootstrap: a group's first entry is `Config(Normal(from_record(record)))` at term 1, index 1, and a member with no committed config serves no read and grants no vote. `ReplyMeta`, and `NotLeader` with the committed config (Shape, "Sites and placement" and "The write path"). `RequestId`, `LogEntry.requests` as `(RequestId, Range<u32>)`, the core-tier `request` table kept for `REQUEST_RETENTION`, `replay` of stored events for a duplicate, and tickets, refusals included, that resolve only when their entry commits. `CredentialShard`, `CredentialKey`, `credential()`, and `deps` reading `RepoSettings` from the fold. `COMMAND_MAX`, `Unsupported`, and `CallError` with the member's `TooLarge`, `EntryView` for `decide`, and `outcome_by_request` taking the command (Shape, "The write path"; `design/SHARED.md`, "Record formats and the two-release rule"). The leader sends AppendEntries before its own append is durable and counts itself toward the quorum only once its own storage confirms. The host sends each member message once, and resends come from the core's `tick`, `COMMIT_RETRIES`, and the 3 s campaign deadline ("Commit and read paths"). A call that misses `CALL_DEADLINE` is reported failed then, and its answer is still awaited for `LATE_LIMIT` (10 s) [guess] and stepped, because a host that dropped late answers never learned that a far follower had taken a 1.08 MB catch-up (`tests/m1/results/m1-f43.md`). `load` holds the in-memory log by bytes as well as by `LOG_WINDOW` entries, because a member of a repository with 53 inline packs of 0.7 to 1 MB ran out of its 128 MB isolate in `load` after a reset: `LOG_BYTES` (8 MB) [guess], `request` rows in `Fold::row_event` form without inline pack bytes, and every multi-row read one row at a time (`tests/m1/results/m1-wp2.md`, "F45"; F45 for the value). `QuorumProof` carries the leader, the commit index, the entry's `at`, the leader's commit time, and a `DurableAck` per counted site, and the host adds `sentAt` and `selfSyncedAt` to every `done` reply, in the product build ("Member-side ack evidence"). A host that held each round for its own write measured B0-01-style p99 645 to 791 ms (`tests/m1/results/m1-wp2.md`, "On Durable Objects"). Each AppendEntries carries at most `APPEND_BYTES_MAX` (1.5 MB) past its first entry, and at most 64 entries [guess], because a call of up to 64 full entries is never answered within `CALL_DEADLINE` and the leader would resend it forever (`tests/m1/results/m1-wp8.md`, fix 4). The invariants under "Invariants" hold: one value per index, one entry in flight per group, one writer per fold.

**Depends on.** M1-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-12 | Leader killed after quorum and before the reply, 10,000 times. The edge re-sends with the same `RequestId` | Original outcomes returned, zero double applies | Any double apply or changed outcome |
| BF-13 | Leader lost before commit, on entries with a `Stale` refusal caused by an earlier command in the same entry | Zero refusals returned from uncommitted entries (Porcupine) | Any refusal returned from an uncommitted entry |

It also carries BM-1's "Linearizability rerun at 24 hours" (the rerun of B0-11).

### M1-WP3. Leadership and health

**Build.** `transfer`, `suspect`, `SUSPECT`, `Health`, `STEP_DOWN`, `led_term` resumption, and transfer back to a group's `home` after it recovers (`design/DESIGN.md`, Shape, "Leadership"; "Commit and read paths", journey 5). `suspect` takes `about`, the member a report is about; a granted transfer vote also promises the target; `TransferRefused` abandons a transfer at once; "recovered" is 5 calls to `home` in a window with none failed, outside quarantine, with 60 s before a retry [guess]; and `Verdict`, `TransferError`, `SuspectError`, and `Colo` are as Shape defines them (`tests/m1/results/m1-wp3.md`). A hedge on a missed receipt is a `suspect` report, never a command forwarded to another member ("Usage (caller's view)", call sites).

**Depends on.** M1-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-14 | Receipts that miss `CALL_DEADLINE` | Zero commands forwarded to a second member | Any command forwarded |

It also carries BM-1's run "with one site's member class failing mid-run".

### M1-WP4. Pack store and R2 keys

**Build.** `PackId`, `PackRef` (`Inline` up to 1 MiB, `Stored`), `DurablePack` built only by `PackStore::place`, `R2Key` (`Pack`, `Idx`, `Snapshot`, `Archive`, `CloneCache`, `Committed` with checksum and size, and `Upload`, the journal of an in-flight multipart upload, because the R2 binding cannot list open uploads), `Archived`, and `Promoted`, which packs the objects of every pack, inline or stored, an archived range names into one self-contained `DurablePack` before `Archived` commits (`tests/m1/results/m1-wp5.md`, BF-18 run 1). `PACK_COMMIT_WINDOW`, and the create-only `Committed` marker the leader puts after `Acked` for each stored pack. The site-script sweeper cron repairs a missing marker from the chain's `request` row only when that row names the pack's id, deletes an unmarked object older than 24 h, fills the third bucket from the markers, and aborts orphan multipart uploads from the `Upload` journal (`design/DESIGN.md`, Shape, "Packs"; Stack, Cron Triggers; journey 1; `tests/m1/results/m1-wp4.md`). The sweeper's per-run subrequest budget counts its `request` reads, one member call per unmarked pack, beside its listings and actions [guess, to tune], because BF-19's hosted run made about 2,000 such reads a run in 70 to 95 s that `CronBudget::SITE_CRON` did not count (`tests/m1/results/m1-wp4.md`, "BF-19 on the hosted cron").

**Depends on.** M1-WP1, M1-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-19 | 1,000 failed placements, plus 1,000 commits whose marker write is killed | Failed ones deleted after 24 h, zero committed ones deleted, missing markers repaired | Any committed pack deleted |

It also carries BM-1's `git push --atomic` with one site failing.

### M1-WP5. Git cache shards and the git container image

**Build.** `GitCacheShard`, `shards_for`, `SHARDS` as set by B0-14, `HOT_POOL` counted in the start cap of `count + HOT_POOL` per site, rendezvous hashing of (network, shard), the LRU of bare repositories with reftable, the start-time region check, the `git` and `git_prev` image map, and the 6 h inactivity timeout renewed by alarm until 24 h after the last request (`design/DESIGN.md`, Shape, "Git cache shards (derived)"). `GitCacheShard::validate` and `GitCacheShard::upload_pack`. `IngestCursor` per (shard, network), keyed to the disk id the container writes, and `rehydrate` from every stored pack with a marker plus the packs each member chain names after its last `Promoted` range's `to`, with a full rehydrate for a cursor on another disk or behind an `Archived` truncation (Shape, "Packs" and "Git cache shards (derived)"). Every ingest runs `index-pack --strict --fix-thin`, because pushed packs are stored thin (G5). A member answers a shard's catch-up read of entries in pages of at most 600 kB of stored bytes, at least one entry each [guess], because a 4.6 MB answer took 7.8 s, past the lookup's call deadline, and the shard never caught up (`tests/m1/results/m1-wp8.md`, fix 4). The git container image with upstream git.

**Depends on.** M1-WP4.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-18 | Archive a range of inline packs, kill all shards, rehydrate. A second shard behind its `IngestCursor` | `git fsck` clean, every ref clones, second shard serves the newer pushes | Any missing object |

It also carries BM-1's `git clone` and `fetch`. B0-22's git time had prebuilt indexes and no thin packs; G5's rerun on this image, with a real network of thin pushed packs, times the copy and the index apart.

### M1-WP6. The edge for git over HTTPS and the clone cache

**Build.** The edge's receive-pack: stage, validate, place, commit, and report, with `Push` (its `actor` a `Principal`), `RefUpdate`, `Atomicity`, and `report_status` taking `&Acked` (`design/DESIGN.md`, "Usage (caller's view)", call sites; `design/SHARED.md`, "Refs and pushes"). The edge mints one `RequestId` per client request, encodes each command, and refuses one over `COMMAND_MAX`. A push that would go over the cap stores its pack, and a push whose ref updates alone exceed it is refused whole with an `ng` line on every ref (SHARED, "Refs and pushes"). The edge renders `Unsupported` as 503 with `Retry-After` (Shape, "The write path"). Upload-pack through the shard, with the output teed to the client and R2. The clone cache keyed by `CloneKey` (Shape, "Packs"; "Dominant access patterns"). The plan cap refusal that names Enterprise or splitting the push below Enterprise, and SSH on Enterprise (journey 1). The `git-http` and `git-protocol-v2` surfaces at T0 (`design/SHARED.md`, "Surfaces, tiers, and oracles"). The 10,000-ref atomic push limit and its oracle (`design/SHARED.md`, "Known deviations from GitHub"), for `--atomic` pushes only. From the M1-WP6 build (`tests/m1/results/m1-wp6.md`): `CloneKey` carries `tags`, the digest of `refs/tags/`, when the fetch asks for `include-tag`; `RejectReason::Atomic` marks the refs an atomic push refused only because another failed (`atomic push failure`); the member renders `report_status` from `&Acked` and the edge frames the bytes, because `Acked` cannot cross a binding; and the refusals that must leave no entry (the atomic cap, `COMMAND_MAX`) are the edge's, before `commit`, because a refusal inside `decide` still commits an entry with its `request` row. SHARED's `GCM_TRACE=1 git clone` oracle row stays and is not yet run: Git Credential Manager was not in the orb.

**Depends on.** M1-WP2, M1-WP3, M1-WP4, M1-WP5, M1-WP7.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-16 | Edge on N+1 with a new message format that is not enabled, plus one forced unknown variant | Nothing sent early. Unknown variant gives 503 with `Retry-After` and no state change | Any other rendering or any state change |
| BF-17 | Atomic push of 10,000 refs with a 1 MiB pack. A push whose updates exceed `COMMAND_MAX` | First: pack stored and push commits. Second: refused whole, no entry | Partial push, or an entry over 1.5 MB |

It also carries all of BM-1's git commands.

**Composed misses and the shard's fill (2026-11-15)** (`tests/m0/results/composed-miss.md`; branch `composed-miss`). Built: the composed clone-cache miss (`containers/git/server/compose.go`, `graftyard_git::compose`, `ShardHost::ready_chain`, `workers/site/src/shard.rs`, `Compose`) and the clone-cache fill from the shard, which no longer depends on the client reading to the end (`decisions/DECISIONS.md`, "Clone-cache misses are composed from the cached tip", "As built", and "The clone cache fills from the shard"). On a linux fixture a composed miss sent its first pack bytes at 0.011 to 0.018 s, against 5.0 s and 30 CPU-s for stock. B0-34's hosted check is LM-1's next run.

### M1-WP7. Minimal identity and owner placement records

**Build.** The minimal identity that real code needs, with no test-only resolver (`decisions/DECISIONS.md`, "Milestone 1 builds minimal identity"). The `Ids` group with `DbId`, its cell, block, and offset layout, and `EntryBody::IdBlock` (`design/DESIGN.md`, Shape, "Chains"). `NameShard`, `name_shard`, `account_shard`, and the `NameKey` and `NamesEvent` variants for login claims, `Account` rows, owner `Placement` records, and `RepoOwner` (Shape, "Identity" and "Sites and placement"). `Login`, `LoginKey`, `RepoName`, `RepoNameKey`, `RESERVED`, and `RepoEntry` (`design/SHARED.md`, "Names"). An owner's `Authz` record with admin grants and `RepoName` entries (SHARED, "Event types"). Stored-token authentication: `TokenShard`, `token_shard`, `TokensEvent::Stored` and `Revoked`, `OpaqueToken`, `Credential`, `Principal`, `READ_CACHE`, the per-isolate cache, `IDENTITY_BUDGET_PER_ISOLATE`, and the leader's read of each command's `CredentialShard` once per entry (Shape, "Identity" and "The write path"; SHARED, "Identity and tokens"). Owner placement moves through the Workflows script, which bumps the record's `generation`, keeps `moving_from`, and moves each group through `Joint` (Shape, "Sites and placement"). The system chains' records live in the signed `Genesis`, and the `Release` group is M5-WP1, so this work package builds genesis acceptance: `graftyard genesis`, `graftyard sign`, and `graftyard install --genesis` into a `Release` group that accepts only a `Genesis` matching the trust anchor [inference] (`decisions/OPEN-QUESTIONS.md`, R13). How the gate's accounts, repositories, and tokens are created before REST and sign-in exist is open (`decisions/OPEN-QUESTIONS.md`, R14). In Milestone 1, `graftyard genesis` makes the keys and the unsigned entry and reports that it did not create `graft-front`, the edge, `graft-ops`, or the Workflows script, which do not exist yet (`tests/m1/results/m1-cli.md`). From the build on Durable Objects (`tests/m1/results/m1-wp7.md`): a bootstrap member claims its chain on the registry of the oldest record's first site, and only the first generation to claim bootstraps; a replacement on the same site goes through `Degraded { lost }` then `Normal`; the transfer back to `home` reads the committed config's `home`, not the record's; and a `MovePlacement` that changes only `home` is one config change and a transfer, with no `Joint`. The test routes (`/retire`, `hold_deps_ms`, `no_claim`, operator-op routes) are removed or gated off before a real install.

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
| BM-1 | The `git` conformance suite and `git clone` / `fetch` / `push --atomic` from unmodified git 2.4x pass with one site's member class failing mid-run. Linearizability rerun at 24 hours. The `git` conformance suite is SHARED's `git-http` and `git-protocol-v2` oracle rows on unmodified git 2.39 and 2.51, plus git's own smart-HTTP transport tests from `t/` that can be pointed at a remote host, run by `tests/m1/wp6/conformance/run.sh` (`decisions/DECISIONS.md`, "Decisions from the M1 builds, 2026-11-01") |

## Milestone 2: folds, issues, pull requests, and merge

Built (`design/DESIGN.md`, "Milestones 1 to 7"): both fold tiers in member SQLite with follower snapshots, the issue, pull request, review, check-run, and merge-queue model from `design/SHARED.md`, "Issues and pull requests", with `parent_tip` and `QueueMerge` checked in `decide`, leader cursors and Queues with the `PrSync` and `MergeQueue` consumers, `TestMerged`, and the blob store for LFS objects and release assets, plus the T0 repository surfaces [inference].

### Milestone 2 status (2026-11-07)

Milestone 2 is closed: every gate row below is Done, the last being BM-2 as reworded, on `bm2-home` (2026-11-06 to 2026-11-07).

Every hosted row ran on the test install `graft-m1-site-a/b/c` and `graft-m1-edge`, or on `graft-m2b` where the table says so (`graft-m1` gained a fourth site script, `graft-m1-site-d` in wnam, on 2026-11-08 for BF-30, and `graft-m2b` gained `graft-m2b-ops`, Worker `c53e4459` with the Secrets Store secret `graft-m2b-mint-wrap-key`, on 2026-11-09, when `graft-m2b-edge` became `c4eb6915` with formats 32.8, 32.12, 32.14, and 18.3 added (AM1-WP6), then `b9f4291e` with the same formats after `fanout`'s site day on 2026-11-10 (AM1-WP1); Milestone 2 ran before both), with real gh 2.102.1, git, and git-lfs 3.8.0 clients. LF-13 ran on a third test install, `graft-m3c` (`tests/am1/results/install-m3c.md`), built 2026-11-11 in account `<cf-account-id>`: install `01M742HEA2SBCYP1VW26624KMW`, sites `graft-m3c-site-a` (enam), `-b` (weur), and `-c` (apac), edge `graft-m3c-edge` with the agent formats and the tools edge `graft-m3c-tools-edge`, and no Secrets Store or `graft-m3c-ops`, because genesis hit `maximum_stores_exceeded` (the account's one store is `graft-m2b`'s), so installation tokens do not mint there. Results are in `tests/m2/results/`, one file per work package, and each design fix they found went into the design first (`decisions/DECISIONS.md`, the groups from "BF-20 passes after the restore fix, 2026-11-04" to "BM-2 reworded by the operator, `bm2-repack`, and M3-WP4's pure half, 2026-11-06"). M2-WP6's final build rechecked the earlier rows at reduced counts, not as full reruns ("Per-row recheck on the final build", under M2-WP6).

| Row | State | What closes it |
| --- | --- | --- |
| BF-20 | Pass at 300 commands/s on the 3.99 GB metadata tier of `repo:196628`: digest equal to a refold from both R2 copies and on all three members, apply p99 0 ms, no limit outcome, 423,000 of 423,000 commands (`m2-wp1.md`, "2026-11-04: restore fix and BF-20 rerun"). Rechecked on M2-WP6's final build only as a smoke: digests agree at two indexes after two site days, and 3 pushes; the 300/s campaign was not rerun | Done |
| BF-22 | Pass in the pure half at the row's 10,000 check runs and statuses on two members, prune included (M2-WP2). Hosted at 500 statuses (`bf22e`, `m2-wp6.md`, "BF-22 reduced"): 500 of 500 read through `gh pr checks` and REST, core rows unchanged but for `consumer_cursor`, both tiers' digests equal on every member, after two fixes. The prune clause ran only in the pure test, because the install's entry clock cannot be moved | Done for M2; a hosted prune needs a movable entry clock |
| BF-1 | Pass hosted, 3 of 3 (`m2-wp3.md`); rechecked 2 of 2 on M2-WP6's final build (`bf1s`) and on `bm2-repack`'s | Done |
| BF-21 | Pass hosted, 100 runs with the leader killed in 10 (`m2-wp3.md`); rechecked 10 of 10 with 5 kills on M2-WP6's final build (`bf21t`), on `bm2-latency`'s, and on `bm2-repack`'s | Done |
| BF-23 | Pass hosted, 100 of 100 (`m2-wp3.md`); rechecked 20 of 20 on M2-WP6's builds, and 10 of 10 on `bm2-latency`'s and on `bm2-repack`'s | Done |
| BF-24 | Pass on `graft-m2b`, 75 of 75 within 10 s with the in-band first attempt, clean p99 6.44 s, conflict p99 5.81 s, with leader-change, reset, and purge runs (`m2-wp5.md`, "2026-11-05"); rechecked 20 of 20 on M2-WP6's builds (clean 0.83 to 2.10 s, conflicts 1.03 to 1.71 s) and once after the edge-wiring site day (6.3 s, `m2-edge.md`) | Done |
| BF-25 | Pass with real git-lfs and gh release: every `verify` and asset 201 commits only after the member confirms two buckets in two regions, with each blob's marker in all three buckets (`m2-edge.md`); rechecked as a smoke on M2-WP6's build. The unreadable-bucket clause ran only in the pure test, 60 of 60 asset and LFS reads with each bucket in turn unreadable, plus one corrupt copy (`crates/api/tests/blobs.rs`), fewer than the row's 100, because the install has no hook to make a bucket unreadable (`m2-wp6.md`, "BF-25 and BF-26 smoke") | Done for M2; a hosted unreadable-bucket run at 100 reads needs a bucket fault hook |
| BF-26 | Pass in the pure half with real `git archive` tar.gz and zip, and hosted: all 6 `archive-cache/` objects deleted, then `gh release download --archive=tar.gz` gave the same bytes and rebuilt the object (`m2-wp6.md`, "BF-25 and BF-26 smoke") | Done |
| BM-2 | Pass as reworded on `bm2-home`: home clause p99 1.36 s at n = 300 with an EWR client and edge, 0 over 2 s; off-home clause p99 1.83 s at n = 288 from LAX, 0 over 3 s; every sample on a new term. Journeys 2 and 3 passed 10 of 10 on `bm2-repack`'s build and stale `parent_tip` 100 of 100 on `bm2f`, neither rerun on `bm2-home`'s (M2-WP6, "BM-2 status") | Done |

F51, which asked for the `StatusId` source before M2 closed, was settled in code after it (`212aa36`; `decisions/DECISIONS.md`, "F51 closed: a status's id follows its commit's order"). BF-23 passed with statuses from one edge; a hosted run through two edges with skewed clocks cannot be made, because an edge cannot fake its clock.

Repository creation over REST (`rest:repos`, including `POST /orgs/{org}/repos`), since built on `m3-repos` ("Milestone 3 status"), was not built in Milestone 2: M2-WP4's "Built so far" lists `rest:repos` under "Not yet", and no M2 gate row needed it, because the runs created repositories with `CreateRepo` on the owner's `Authz` chain, as the seed does [inference, from `tests/m3/results/m3-rows.md`, "Terraform"]. Milestone 2 stays closed on its gate rows; BF-34's Terraform test needed the route, and `m3-repos` built it in repository format 32.13 (2026-11-09).

### M2-WP1. Folds in member SQLite

**Build.** `Tier`, `tier_of`, `apply`, and `fold_audit` (`design/SHARED.md`, "Folds"). Both tiers in each member's SQLite, with FTS5 (`design/DESIGN.md`, Stack, Durable Objects; Shape, "Reused with the additions below"). Snapshots per tier under `R2Key::Snapshot`, with one snapshot per tier at or below every index inside `RESTORE_WINDOW`. One Full follower writes each snapshot, chosen by `to / SNAPSHOT_EVERY` modulo the Full followers, every `SNAPSHOT_EVERY` entries or `SNAPSHOT_BYTES` of log and at least daily, at one index without pausing `apply` through `snap_undo`, as `SnapshotFrame` parts ending in a `SnapshotTrailer` in two buckets (Shape, "Packs"). The site-script fold audit cron (Stack, Cron Triggers).

**Depends on.** M1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-20 | Snapshot of a 4 GB [guess] metadata tier under 300 commands/s | Apply p99 within 10% [guess]. Snapshot digest equals a refold at `to`. No request-limit errors | Apply p99 more than 10% slower, a digest mismatch, or any request-limit error |

It also carries BM-2's journey 3 lists.

**Built so far (2026-11-03).** The pure code, in `crates/fold`: `apply` per tier on a store trait, `fold_digest` as the sum of row hashes (`design/SHARED.md`, "Folds"), snapshots with `snap_undo` ending in a `SnapshotTrailer` that carries entry `to`'s digest, and pruning by entry time. The hosted half (`tests/m2/results/m2-wp1.md`, "What was built"): the Raft hook `Fold::apply_entry` that hands a fold the whole committed entry, with `Fold::scratch_at` for `decide` and `verify` and `Fold::tiers` for recorded entries, and no `Command` change; refs as the core tier's `ref` table, so `RepoFold` holds only the two tier stores and `decide` reads what `apply` writes; the SQLite store, `TierSql`, with DDL generated from each tier's schema and two derived FTS5 tables outside `fold_digest`; the lazy metadata load, applied from the stored log by alarm; alarm-driven snapshot hosting through `snap_undo` to two buckets, resumable across resets; and the site's fold audit cron, run so far only on an empty log. M3-WP5's hosted half (2026-11-06) then built `InstallSnapshot` hosting, `R2Key::Archive` objects through a test route, and the snapshot-based fold audit cron, which skips nothing for log size, only tiers over `AUDIT_SNAPSHOT_MAX` (`design/SHARED.md`, "Folds", `fold_audit`; M3-WP5 status below).

**BF-20 status (2026-11-04): passes on all three clauses at 300 commands/s** (`tests/m2/results/m2-wp1.md`, "2026-11-04: restore fix and BF-20 rerun"). The 3.99 GB metadata tier of `repo:196628` (1.33 M rows) was snapshotted twice in full, at `to` = 853 and 1313, each in 290 batches. The snapshot taken under load, to 1313, refolds to the key's digest from both R2 copies, and that digest matches `/fold` on all three members. The apply step's p99 was 0 ms (under 1 ms) in every phase. The leader's entry age p99 was 838 ms during the snapshot against 840 ms for the pooled held baseline, -0.2%. The baseline drifted across the session (528 ms before, 840 ms after), so against the earlier baseline alone the snapshot phase is +59%; pooling both held phases, which bracket the snapshot in time, is a judgement call [inference], and the bootstrap intervals overlap (717 to 1,007 and 743 to 1,278 ms). Workers Logs show no memory or CPU limit outcome on any site, and 423,000 of 423,000 commands succeeded across 47 runs with site 0 leading each. The 2026-11-03 failure was restore loading every `request` row into memory, plus three defects that kept one group down (`decisions/DECISIONS.md`, "BF-20's wedge: restore held every `request` row in memory"). Its 8 wasm `unreachable` aborts in `host::commit_tx` on site c's member, no command lost, were F47: an isolate out of memory, fixed in `c3e9fa1`, `6e89c6e`, and `e5c6021` (`tests/m2/results/f47.md`; `decisions/DECISIONS.md`, "F47 answered: the wasm `unreachable` is an isolate out of memory"). Those fixes left the fold audit skipping any chain with over 16 MB of log; M3-WP5's refold from the newest snapshot answers that in design, and its site cron is M2-WP1 work (`decisions/DECISIONS.md`, "F48: the fold audit refolds from the newest snapshot"). Not gated: follower entry-age p99 was 21 s at baseline and 31 to 50 s during the snapshot, set by about one AppendEntries per run that missed `CALL_DEADLINE` (F41). The snapshot's batches averaged 3.8 s, worst 11.4 s, against the 2 s `BATCH_MS` [guess] (F15). The first run's "300/s in 1 of 3 runs" did not recur: all 47 runs here carried it, with a campaign before each run and no isolate resets [inference].

### M2-WP2. Issues, pull requests, reviews, and checks

**Build.** The model in `design/SHARED.md`, "Issues and pull requests": `IssueNumber`, `IssueKind`, `HeadSpec`, `PrState`, `Mergeability`, `MergeMethod`, `record_check`, and `record_status`, with the stored formats `IssueCapture`, `IssueChanged`, `CommentEvent`, `ReviewEvent`, `LabelEvent`, `BranchRule`, `RepoSettings`, and `CheckRunUpdate`. The `RepoEvent` variants they write, including `CheckRun`, `CheckSuite`, and `Status` in the metadata tier, pruned after `RepoSettings.check_retention_days`, and `RequiredCheck` in the core tier with an optional conclusion (`design/SHARED.md`, "Event types"). `GitCacheShard::merge` with the tree-diff check (`design/DESIGN.md`, Shape, "Git cache shards (derived)"). The `rest:issues`, `rest:pulls`, `rest:checks`, and `rest:commits` surfaces and the GraphQL resolvers that `gh pr` and `gh issue` use (`design/SHARED.md`, "Surfaces, tiers, and oracles").

**Depends on.** M2-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-22 | 10,000 non-required check runs and statuses on one repository, then entry time advanced past `check_retention_days` | `gh pr checks` and `GET /commits/{sha}/statuses` read them; core tier bytes unchanged; rows pruned on every member with equal `fold_digest` | A non-required row in the core tier, or members disagreeing |

It also carries BM-2's journeys 2 and 3.

**Built so far (2026-11-03).** The pure model in `crates/git`: issues, pull requests, reviews, and checks with `decide`, `apply`, and `tier_of` over two tiers, sharing M2-WP1's tests and planted bugs. The core count of unresolved review threads, which a thread's first review comment, `Resolved`, and `Deleted` write (`design/SHARED.md`, "Issues and pull requests"). One `DbId` cursor per tier: Metadata assigns the lower half of each leased block and Core the upper, and an event that touches Core takes Core's ids (`crates/git/src/tiers.rs`, `tier_block`). A new `crates/api` with pure handlers for the 29 GraphQL documents gh 2.102.1 sends for `gh pr` and `gh issue`, resolved over 213 fields, and the one REST route gh calls, `GET /repos/{o}/{r}/commits/{ref}/statuses`. Replies are validated against the pinned GHES 3.22 OpenAPI description and GraphQL schema, 7 of 7 planted bad replies were caught, and the real gh drove the handlers through a TLS proxy (`crates/api/gh/README.md` for the capture and the oracle files). BF-22's pure half passes: two members apply 10,000 non-required check runs and statuses, gh's `PullRequestStatusChecks` and the statuses route read them all, the core tier is unchanged, and after `check_retention_days` the rows are pruned on both with equal `fold_digest`. Not yet: executing a merge (`GitCacheShard::merge`) is M2-WP3, the mention index, and BF-22's hosted run.

**Wired on Cloudflare (2026-11-05)** (`tests/m2/results/m2-edge.md`). The edge routes REST, GraphQL, uploads, LFS, and codeload through `graftyard_wire::Host::endpoint` to `crates/api`; it reads a repository's two tiers from its leader with one `{"q": {"rows": true}}` read, and commits each `IssueCall` through the member, which renders the reply from `&Acked`. M2-WP5's `RepoCmd { Push, Sync }` and this work's issue command are one `RepoCmd { Push, Sync, Issue }`, and `IssueCall` is repository format 32.5, because M2-WP3 took 32.4; the family reads 32.1, 32.3, and 32.5, and the test edge enables 32.5 with `FORMATS_ENABLED` as a stand-in for the `Release` view. Real gh 2.102.1 passes `gh issue list/create/view/comment` and `gh pr list/create --draft/view` against `graft-m1-edge` (`gh pr checks` exits 1 with gh's empty-rollup message, every request 200), and 26 GraphQL and 12 REST replies of the final run validate against the pinned schemas with 0 failures. Found only on Cloudflare and fixed: every hosted write failed with 503 because `AuthzFold.grants`, keyed by `(RepoId, UserId)`, cannot cross as JSON, so `HostedDir` carries the fold in the raft codec (`eba06c7`). Still stand-ins: logins other than the owner's render from the test-only `LOGINS` variable, because `Names` has no row from an account to its login (`decisions/OPEN-QUESTIONS.md`, F50); and the edge reads a repository's whole tiers into memory, capped at `ROWS_MAX` (50,000 rows [guess]), past which a read answers 503, because a paged read would mix indexes. DESIGN's list path is one indexed SQLite query on the leader ("Dominant access patterns"), so the hosted reads move to leader-side queries before any repository nears the cap.

### M2-WP3. Merge and the merge queue

**Build.** `Merge`, `MergePlan`, `check_protection`, and `MergeError` (`design/SHARED.md`, "Issues and pull requests"). `MergeQueueEntry` with its queue ref, the `parent_tip` rule, `QueueEvent` (`Enqueued`, `GroupBuilt`, `Ejected`, `Landed`), `EjectReason`, `QueueOp`, and `QueueMerge`, whose `decide` lands a whole group or nothing, so queue tips only fast-forward. The `MergeQueue` consumer builds groups, creates the `gh-readonly-queue/*` refs, reads `RequiredCheck` on them, ejects failures, and submits `QueueMerge` (`design/DESIGN.md`, Shape, "Merge queue"; "Invariants"; `design/SHARED.md`, "Event fan-out").

**Depends on.** M2-WP1, M2-WP2, M2-WP5.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-1 | One entry holding a 5-PR merge-queue group plus a `PrSync` batch | It commits on all three sites | Any follower refuses it |
| BF-21 | A group of 5 PRs under `HeadGreen`, the 3rd failing its required check on its queue ref, 100 runs, with the base leader killed mid-landing in 10 | The base moves only by `QueueMerge` entries that land a whole group; the failing PR is ejected and the other 4 land after a rebuild; every base move is a fast-forward | Any partial group, any non-fast-forward base move, or a queued PR lost |
| BF-23 | A required check passes, then is re-requested before the merge | The merge is refused until the new run concludes, 100 of 100 | A merge on the stale success |

**Built so far (2026-11-05), the pure half.** In `crates/git`: `Merge`, `MergePlan`, `check_protection`, and `MergeError`, deciding on the core tier alone with the git cache's `MergePlan` as the command's input (`merge.rs`); `MergeQueueEntry` with its queue ref, `QueueEvent`, `EjectReason`, `QueueOp`, and `QueueMerge` (`queue.rs`), with `Merged` and `Queue` as event tags 23 and 24 and the commands in repository format 32.4; and the `MergeQueue` consumer as a pure `step` over a fresh `QueueView` of one base, planning `CacheAsk`s for the git cache and the commands to submit (`queue_consumer.rs`). `crates/api`'s `mergePullRequest` builds and commits `Merge`. `crates/git/tests/merge_queue.rs` runs BF-1, BF-21 (100 seeds, the leader killed mid-landing in 10), BF-23 (100 of 100), and BM-2's stale-`parent_tip` refusal (100 of 100) on three real Raft members whose followers `verify` every entry, with a fake git cache and fake CI; all 13 tests pass, and `tests/m2/wp3/plant.py` catches 14 of 14 planted bugs (rerun on `c7a3b3b`). The seven shape gaps it found are settled in DESIGN, Shape, "Merge queue", and SHARED (`decisions/DECISIONS.md`, "M2-WP3's merge and queue shapes as the code needs them"). The hosted half followed (below).

**M2-WP3 status (2026-11-06): the hosted half passes** (`tests/m2/results/m2-wp3.md`), on `graft-m1-*` with gh 2.102.1 and git. Built: `MergeCmd` hosted as `RepoCmd::Merge` (format 32.4, enabled on the test edge with `FORMATS_ENABLED`); the container's `/merge` with `method` serving `GitCacheShard::merge` for merge, squash, and rebase chains, one pack per group; the site's `MergeQueue` consumer on `PrSync`'s pattern, with its first attempt in band, re-sends through the site queue, its cursor moved by `MergeCmd::Cursor`, and a per-base timer from `Step::Idle { at }`; and the edge's `mergePullRequest`, REST `PUT /pulls/{n}/merge`, `enqueuePullRequest`, `enablePullRequestAutoMerge` on a queued base, `dequeuePullRequest`, and `POST /statuses/{sha}`. Results: BF-1 3 of 3 (each landing of five `Merged` and `Landed` and a five-head `PrSync` `Refs` in one entry, committed on all three members with one digest); BF-21 100 runs, `main` moving only by whole `QueueMerge` fast-forwards, the 3rd ejected `CheckFailed` and the other four merged, all 10 leader kills (6 after quorum, 4 before commit) ending whole; BF-23 100 of 100, `gh pr merge` and `mergePullRequest` both refused on the re-requested check and `main` never moving; BM-2's stale-`parent_tip` part 100 of 100 on the leader (300 commands refused `StaleChain`); the per-base timer 5 of 5 (`GroupBuilt` 63.1 to 63.2 s after the first `Enqueued` under `min_wait_minutes` 1); real `gh pr merge --merge`, `--squash`, and `--rebase` exit 0 with the expected parents and trees, and REST merge answers 200, 409 on a stale `sha`, 403 to a reader, and 405 to a second merge; 478 GraphQL and 63 REST replies validate with 0 failures; `tests/m2/wp3/plant.py` catches 14 of 14 planted bugs. Merge and group packs ride inline in the command, at most `MERGE_PACK_MAX` (900 KB), and a larger one is refused, rather than being placed [inference: a merge's own objects are a few KB] (DESIGN, Shape, "Merge queue"). Still open: no client route sets a branch rule or the repository settings, so the gates recorded them with the test-only `/bench_seed {"record": ..}` (GHES's branch-protection and rulesets routes are M3 or later); a status's id came from the edge's clock (F51, since closed: `decisions/DECISIONS.md`, "F51 closed: a status's id follows its commit's order"); `dequeuePullRequest` and the other queue payloads render `mergeQueueEntry` as null, because no `MergeQueueEntry` node type is served; `mergePullRequest`'s conflict text reuses `NotUpToDate`'s wording [unverified: GitHub's text]; and the `CODEOWNERS`, last-push, and signature data whose rules refuse every merge.

### M2-WP4. The wire layer, the blob store, and the other T0 repository surfaces

**Build.** The cross-cutting conventions in one shared wire layer (`design/SHARED.md`, "Cross-cutting conventions"), the hostnames and endpoints rules ("Hostnames and endpoints"), and client and object ids ("Client ids and object ids"). The GraphQL schema generated from GHES 3.22 SDL. The blob store: `R2Key::Blob { repo, oid }` placed in two buckets as `DurableBlob`, `BlobRef`, the `Release`, `ReleaseAsset`, and `LfsObject` events, and derived `R2Key::ArchiveCache` objects beside the clone cache (`design/DESIGN.md`, Shape, "Packs"; `design/SHARED.md`, "Event types"). The T0 surfaces that `design/DESIGN.md`, "Milestones 1 to 7", places in Milestone 2 [inference]: `meta-endpoint`, `rest:meta`, `rest:rate-limit`, `rest:repos`, `rest:branches`, `rest:git`, `graphql`, `rest:releases`, `uploads-release-assets`, `codeload-archives`, `raw-content`, and `lfs` over HTTPS. Its SSH part is M6-WP2.

**Depends on.** M2-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-25 | `git lfs push` and `gh release upload`, then one site bucket made unreadable | `verify` and the 201 come only after two buckets hold the blob; fetch and download succeed 100 of 100 | Any acknowledged blob unreadable |
| BF-26 | All `ArchiveCache` objects deleted, then `gh release download --archive=tar.gz` | The bytes match the earlier download | Any difference or error |

Each surface's oracle in `design/SHARED.md`, "Surfaces, tiers, and oracles", passes [inference: the design names the oracles but no gate row for each]. It also carries BM-2's `gh` runs.

**Built so far (2026-11-04).** `crates/wire`, the shared wire layer: headers, pagination, errors, host validation and the surface router, client ids, `databaseId`, and node ids with the legacy base64 form (two planted bugs caught). The GraphQL schema generated from the pinned GHES 3.22 SDL, 1,626 types (two planted executor bugs caught). The blob store in `crates/git`: `DurableBlob`, sealed so it is built only once two buckets hold the blob, and the `Release`, `ReleaseAsset`, `LfsObject`, and `LfsLock` events, tags 17 to 20. The LFS batch, transfer, verify, and locks routes, with the transfer served by the edge rather than presigned URLs (`design/SHARED.md`, `lfs`), releases and assets, and archives through `ArchiveCache`, keyed by the commit and a hash of the prefix, because `git archive` writes the commit time and the prefix into the bytes, so one tree under two commits or two prefixes gives different archives (`crates/git/src/blob.rs`, `archive_key`; `design/DESIGN.md`, Shape, "Packs"). 65 requests captured from git 2.56.0, git-lfs 3.8.0, and gh 2.102.1 are replayed through the handlers, and 93 replies validated against the pinned REST, GraphQL, and git-lfs schemas with 0 failures (`crates/api/lfs/README.md`, `crates/api/tests/blobs.rs`). The pure halves of BF-25 and BF-26 pass: with two buckets refusing puts there is no upload 201 and no `verify` 200, and with one bucket unreadable or one copy corrupt 60 of 60 asset and LFS reads succeed; real `git archive` tar.gz and zip bytes match after every `ArchiveCache` object is deleted. New guesses are in SHARED: `BLOB_MAX` = 100 MiB, `generate_release_notes` writing the tag as the name with an empty body, and `search` as a counted rate-limit window. Not yet: `rest:repos`, `rest:branches`, `rest:git` beyond ref delete, and `raw-content`, for which no client was captured; blob GC after `RESTORE_WINDOW`; and enabling through `FormatsEnable` the format that carries event tags 17 to 20, which is `IssueCall`'s 32.5, before any member writes them (`design/SHARED.md`, the two-release rule); the test install stands in with the edge's `FORMATS_ENABLED`. The code still follows two of this run's SHARED fixes: `crates/wire`'s host check refuses `notgithub.com` by suffix, where SHARED now refuses by label, and `archive_key` packs the commit and prefix hash into the old `tree` field.

**Wired on Cloudflare (2026-11-05)** (`tests/m2/results/m2-edge.md`). After a site day on `graft-m1-site-*`, real git-lfs 3.8.0 (push, clone, pull, lock, locks, unlock) and gh 2.102.1 (`release create/upload/view/download`, `download --archive=tar.gz`) pass against `graft-m1-edge`, with pulled and downloaded bytes equal to what was pushed; the final run's 26 GraphQL, 12 REST, and 5 LFS replies, 43 in all, validate against the pinned GHES schemas and git-lfs 3.8.0's with 0 failures. Each LFS `verify` and asset upload commits only after the member's `BlobStore::confirm`, and each blob has its `Committed` marker in all three buckets. Codeload runs `git archive` on the shard's `/archive`. Two bugs were found only on Cloudflare and fixed: every `verify` answered 404 and every upload 503, because the member compared the edge's `BlobRef` with its confirmed one including the site lists, so a blob now matches by oid and size and the sites the member found decide (`6685801`); and `mark_blob` marked the edge's `BlobRef`, so a `verify` marked no bucket and an upload two, and it now marks the member's confirmed `DurableBlob` in every bucket that holds it (`82f04c0`). The site day did not roll the shard containers to the new image, which DESIGN expects, since a new site-script version restarts no running container and images move only on a `ContainerDay` (Shape, "Git cache shards"); `ContainerDay` is M5-WP4, and Wrangler's `--containers-rollout` cannot stand in, because containers under the `durable_object` policy take no part in application rollouts (https://developers.cloudflare.com/containers/configuration/rollouts/). Until then test installs roll warm shards with `workers/site/deploy.mjs --roll-shards <edge url>`, which lists each site's shard instances through the Containers API and kills and catches up every one whose image is not the deployed one through the test edge; two site days rolled 6 of 6 shards each way (`tests/m2/results/site-day-fixes.md`, `319a4cb`). A shard that restarts on its own between days could start on a new image before its `ContainerDay`; one old-code site day on `graft-m2b` replaced warm containers for no known reason [unverified] (same file). The 401 "Bad credentials" in one client run after the site day was the driver sending no token, not the server: across seven site days 0 of 8,750 valid-token requests got 401, unknown tokens got 401 439 of 439, and a member that cannot answer is retried and rendered 503 with `Retry-After: 5` on REST, GraphQL, and LFS (same file, "The 401 window"; F49, now closed in `decisions/DECISIONS.md`, "F49 closed: the 401 window was the driver's empty token"). Follow-up: on git smart HTTP that same failed token lookup renders 500, which git does not retry; it should be 503 with `Retry-After`, as the API paths answer.

### M2-WP5. Cursors, Queues, and the `PrSync` consumer

**Build.** Leader cursors and alarms, one queue per site, consumer Workers, and `Consumer` (`design/SHARED.md`, "Event fan-out"; `design/DESIGN.md`, "Dominant access patterns"; Scale, fan-out). `PrCapture`, and the `PrSync` consumer that moves `refs/pull/N/head`, has the git cache compute the test merge, and moves `refs/pull/N/merge` by `TestMerged` (`design/SHARED.md`, "Issues and pull requests" and "Known deviations from GitHub"). The budget on test merges is AM1-WP1.

**Depends on.** M2-WP1, M2-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-24 | A head push, then `git ls-remote origin refs/pull/N/merge`; a conflicting push | The `TestMerged` commit within 10 s with a matching `mergeable`; on a conflict the ref is deleted and `mergeable: false` | A stale ref |

It also carries the `PrSync` batch in BF-1 (M2-WP3).

**Built so far (2026-11-04)** (`tests/m2/results/m2-wp5.md`). The container's `/merge` (`git merge-tree --write-tree`, a deterministic `commit-tree`, and DESIGN's tree-diff check, in a quarantine object directory); `RepoEvent::TestMerged` (tag 21) and `RepoEvent::ConsumerCursor` (tag 22), both core rows; the leader's memory-only `Dispatcher`, keyed by (member, term) and always restarting at the committed cursor; `PrSync::decide_sync`, which keeps only what still holds on the fold, so a batch delivered twice or out of order changes nothing; content-derived request ids; the site's per-site queue and `#[event(queue)]` consumer, which acks a message only after its cursor-moving command commits; GraphQL `mergeable` and `mergeStateStatus` from the recorded test merge; and the queue message carrying the chain's `PlacementRecord`. 10 tests on three Raft members pass, and `tests/m2/bf24/plant.py` catches every planted bug.

**BF-24 status (2026-11-05): passes**, with the leader's in-band first attempt (`tests/m2/results/m2-wp5.md`, "2026-11-05: the in-band first attempt, and BF-24 rerun on `graft-m2b`"). On `graft-m2b`, `repo:65538`, 75 of 75 trials met 10 s on the final build: clean p99 6.44 s, conflict p99 5.81 s, every merge commit's parents (base tip, head), `mergeable` right each time, and every conflict's ref deleted. The `lc` run covered transfers, a reset old leader, purged queues, and dropped first attempts (the dropped ones synced by the 30 s re-send at 34.7 to 38.2 s), and the `purge` run a second re-send at 60 s; `audit.py` found one `TestMerged` per head over 194 heads, and `tests/m2/bf24/plant.py` catches 14 of 14 planted bugs. The leader's commits held at p50 1.92 s and p99 2.47 s, so no flip of the decision was met (`decisions/DECISIONS.md`, "`PrSync`'s first attempt runs in band, and Queues carries only the re-send"). That run found and fixed one bug (`76abf81`): dispatchers are now keyed by (term, boot), and an in-band attempt older than `REDELIVER_AFTER_MS` counts as lost. The same branch fixed the `tier.rs` panic F47's run saw: a refused fold read resets only that member (`131ee88`). The `/merge` is now about 70% of the server time. The first run, 2026-11-04, on the queue path alone, failed the 10 s clause and passed on correctness: on `repo:196629` with an enam leader, 46 of 50 clean head pushes and 25 of 25 conflicting ones met 10 s (p50 5.57 s and 5.91 s), and p99 was over 30 s: 4 queue messages sat in Queues 36.5 to 37.6 s before their first delivery, with no retry recorded, and the leader's 30 s re-send covered them. Every `mergeable` read matched its committed test merge (130 of 131 reads, the other `UNKNOWN` before a late `TestMerged`), every conflict deleted the ref, and the log audit found exactly one `TestMerged` per head over 131 heads, across 13 leader transfers, 3 restores, and 6 purged batches. That led to the in-band first attempt above. Design gaps the report found: the `PrSync` consumer and its git-cache dependency had no Failure-modes rows (now in DESIGN's `event pipeline` rows); SHARED named no cursor for consumers other than `Webhooks` (now `RepoEvent::ConsumerCursor`); the repository family reads format 32.3 without a `FormatsEnable`, because only the install's own site scripts write it, and SHARED's two-release rule does not say how it applies to a command that site scripts both write and read; and `refs/pull/N/head` waits on the test merge, because it moves in the same command as the `TestMerged`, against SHARED's 10 s oracle for the head ref (met in the rerun, since only the merge now precedes it), and whether to move it first in its own command is not decided. Its constants (`REDELIVER_AFTER_MS` 30 s, `MERGE_BYTES_PER_COMMAND` 900,000, the queue settings) are [guess]. No edge route opens a pull request yet; BF-24 used a test-only `/bench_seed`.

### M2-WP6. Milestone 2 gate

**Build.** Nothing new.

**Depends on.** M2-WP1, M2-WP2, M2-WP3, M2-WP4, M2-WP5.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| BM-2 | `gh pr` and `gh issue` journeys 2 and 3 pass, the merge p99 on a freshly elected leader on the group's home site, with the client in that region, is at most 2 s, and on a freshly elected leader off the home site at most 3 s, and a queue entry with a stale `parent_tip` is refused 100 times out of 100 |

**BM-2 status (2026-11-07): passes as reworded** (`tests/m2/results/m2-wp6.md`, "`bm2-home`, 2026-11-06"). The operator reworded the latency clause on 2026-11-06 (`FRAME.md`, "Operator decision, 2026-11-06, BM-2 latency"; `decisions/DECISIONS.md`, "BM-2's latency clause measures a home-site leader, with a looser bound off home"). Home clause: each sample moved leadership back to enam, the group's home, on a new term, and a test-only client Durable Object created with `locationHint: "enam"` sent gh's own `PullRequestMerge` document and timed it in its region; `cdn-cgi/trace` put the client in EWR and `x-graft-colo` the edge in EWR on all 300 samples. p99 1.36 s, p50 0.94 s, n = 300, 0 over 2 s, where a pass allows 3. Off-home clause: leadership moved to weur or apac on a new term and the LAX orb timed the mutation, edge LAX: p99 1.83 s, p50 1.48 s, n = 288, 0 over 3 s, where a pass allows 2 (weur 1.73 s, apac 1.83 s, n = 144 each). Every one of the 600 samples was on its own new term and merged. The 12 whole `gh pr merge` runs off home (p50 3.48 s, max 4.23 s, 10 over 3 s) include gh's start-up and its reads before the mutation, so they are excluded from the clause, as in every earlier BM-2 run; their 98 GraphQL and 13 REST replies, with the smoke's, validate against GHES 3.22 with 0 failures. A scan of 45,066 raw files for 17 secrets found 0 hits. Journeys 2 and 3 passed 10 of 10 on the `bm2-repack` build and stale `parent_tip` 100 of 100 on `bm2f`, and the queue code is unchanged since; neither was rerun on this build, whose merge-path change is M3-WP4's credential types. The runs below were measured under the old wording ("the merge p99 on a freshly elected leader is at most 2 s"), on `graft-m1-*` with gh 2.102.1 and git, a LAX client and edge, and the identity groups (`Names`, `Tokens`, `Authz`) leading on enam (`tests/m2/results/m2-wp6.md`).

| Clause | Result |
| --- | --- |
| Journey 2 (`gh pr` review, required check, merge, view, list) | Pass, 10 of 10 (`j2b`): the merge is refused until the review and the check commit, then lands with parents (base, head), and `gh pr view` and `gh pr list --state merged` both show it |
| Journey 3 (`gh label`, `gh issue create -l`, `gh issue list`, REST issues) | Pass, 10 of 10 (`j3b`) |
| Merge p99 on a freshly elected leader at most 2 s (old wording) | After `bm2-repack`: p99 2.42 s, p50 1.52 s, n = 288 on 300 new terms (`final`); per new leader site p99 enam 2.01 s (n = 66), weur 1.96 s (n = 120), apac 2.71 s (n = 102); 8 of 288 over 2 s. Before it, after `bm2-latency`: p99 4.10 s, p50 2.11 s, p90 2.93 s, n = 288 on 288 new terms (`final`); per new leader site p99 enam 2.40 s, weur 4.15 s, apac 2.89 s; 178 of 288 over 2 s, where a pass at this n allows 2. The first run, before the cuts: p99 3.20 s, n = 91 |
| Stale `parent_tip` refused 100 of 100 | Pass, 100 of 100 on the final build (`bm2f`); not rerun on `bm2-repack`, which does not touch the queue's `decide` |

183 GraphQL and 10 REST replies of `j2b`, and 60 and 30 of `j3b`, validate against GHES 3.22 with 0 failures. On that first run, what remained was cross-region round trips, not serial steps, so a weur or apac leader paid two to four more long round trips than an enam one; `bm2-latency` followed (below).

The run fixed seven serial steps, each with a test that fails on the old code, and none changed the design: the lookup reads `MemberRegistry` rows only after the cached config and bootstrap names miss; the leader reads an entry's `deps` at once, as "The write path" says; the shard's catch-up reads the first index and refs at once; the shard merges first and catches up only when the container answers 409 or 404 (with the container started first, `576badf`, `4c3727e`); `commit_hedged` sends first to the cached leader, as "Commit and read paths" says; and the edge reads tiers and `Authz` at once, resolves once, authenticates beside the resolve, and passes the owner's record to the shard. "The write path" and "Commit and read paths" were followed late, not changed. The merge plan runs on the shard of the leader's site, which `PrSync` and `MergeQueue` keep caught up (DESIGN, journey 2). Two more fixes came from BF-22's hosted run: REST under `/repositories/{id}`, the form every `link` header names (`57d1411`), and a `StatusId` collision rule, where `decide` takes the smallest free id at or above the edge's (`31eeadf`; F51, since closed by `212aa36`). Not built: journey 2's fork (Milestone 2's repositories are roots), `PATCH /repos/{o}/{r}/labels/{name}` (`gh label create --force` exits 1), and the merge mutation's payload, whose `pullRequest` renders null.

**`bm2-latency` (2026-11-06)** (`tests/m2/results/m2-wp6.md`, "`bm2-latency`" sections, commits `1c84ac6`, `c0a393b`, `20155a1`). Four cuts, each measured on 30 samples:
1. The leader reads its owner's `Authz` dependency under the record its member joined with, so no `fresh` read of the owner's `Placement` row on `Names` comes first; the current record is read only if the joined one finds no group (`the_owners_authz_is_read_under_the_joined_record`).
2. A new leadership catches up its own site's shard of the network on its first alarm (`prsync::warm`), so the first merge plan finds both commits there: shard catch-ups on the merge path fell from 29 of 29 to 0 of 30.
3. The edge reads a merge's logins while the git cache plans it, and reads the owner's login and `Authz` at once.
4. `mergePullRequest` reads only its rows, at most six (`{"rows": {"merge": id}}`), instead of both whole tiers, 571 KB of JSON on `wp6p` (`a_merge_plans_from_its_rows_as_from_both_tiers`); and a merge by node id starts those reads beside the owner's name.

p50 fell from 2.86 s (baseline, n = 30) to 1.80 s after cut 4 (n = 30, 6 over 2 s). BF-1 2 of 2, BF-21 10 of 10 with 5 kills, BF-23 10 of 10, and 15 GraphQL and 2 REST replies with 0 failures on the final build. Two ideas were ruled out by invariants: read-index from a same-site learner for `deps`, because "Only the leader builds" `fresh` (DESIGN, "The read path") and `decide` must see "the `deps` folds the leader read once through `fresh`" (Invariants), and the learner still pays one round trip to the enam dependency leader plus 10 to 50 ms of batching; and the edge passing its auth result to the leader, because `decide` must read `Tokens` itself through `fresh`, and the three dependency reads run at once, so skipping one saves nothing.

**Where the 2 s comes from.** Nothing in DESIGN or DECISIONS derives it, places the client, or says whether the client-to-edge leg counts. It came over from a non-normative arena candidate (`history/arena/cloudflare/candidate-3/DESIGN.md`). The only other "merge p99 at most 2 s" is B0-29's pass value, measured with an enam leader and no git cache plan or edge identity reads. The design assumes a client near the leader and the owner: "Every request enters at the Cloudflare data center nearest the client" ("Commit and read paths"), "Fresh reads on a held lease cost one DO call in the leader's region, plus the client's distance to it" and "Preferred-site transfer keeps leaders near owners" (Scale, latency list), and "one read of the entry's `Authz` dependency on a leader in the owner's home region [guess]" (Scale, "Entries per group"). BM-2's freshly elected leader is off home in two of every three samples.

**The tail has two more causes.** Shards never repack (`gc.auto 0`, and nothing runs `repack`): each shard grew from about 1,200 to about 1,950 packs over the run, its merge p50 rose from 492 to 672 ms, and in the orb `git merge-tree` on a 751-pack repository took 8.7 ms against 2.0 ms after `git repack --geometric=2`. And the new leadership's `warm` catch-up races the first merge on the slowest shard: 38 of 105 weur samples caught up on the merge path, at 250 to 700 ms each, against 3 and 4 elsewhere. The rest is cross-region round trips: every identity leader is on enam, and the leader's dependency reads take 21 to 47 ms p50 from enam, 118 to 132 ms from weur, and 226 to 265 ms from apac. On fresh shards (n = 109) enam-led p99 was 1.68 s, weur 3.18 s, apac 2.82 s.

**Decision (the lead's).** Build a geometric repack after catch-ups and have the first merge wait for the new leadership's in-flight catch-up, on branch `bm2-repack`; hold any rewording of the gate until that rerun; do not change identity placement (`decisions/DECISIONS.md`, "BM-2 latency: repack the shards, wait for the warm catch-up, and keep the gate"). The rewording (F53) was then the operator's call, and F53 is closed (above).

**`bm2-repack` (2026-11-06)** (`tests/m2/results/m2-wp6.md`, "`bm2-repack`" sections; commits `ca15db8`, `e8d47ad`, `4b6631f`, `9301e15`). Both changes match the design (DESIGN, Shape, `GitCacheShard`; Failure modes, git caches, "one repack per shard at a time"). The container's `PUT /refs`, the last step of every catch-up and rehydrate, starts `git repack -d -q --geometric=2` in the background when the network has more than 50 packs [guess], one repack per shard container at a time, under the network's write lock (`TestRefsRepacksPastRepackAt`); every `wp6p` shard went from 1,955 packs to 1. The leader site's shard counts its running catch-ups per network, and `/merge` awaits them before the first merge plan (`a_merge_waits_for_the_catch_up_in_flight_instead_of_racing_it`). Under the old wording, p99 fell from 4.10 s to 2.42 s at n = 288, and over 2 s from 178 to 8. The shard's own merge p50 is 56 to 159 ms by site (from 492 to 1,004 ms), 21 samples waited 4 to 198 ms for a catch-up, and 0 of 288 caught up on the merge path (from 45). What remains is cross-region identity reads: auth and resolve about 540 ms p50 on every leader site (the LAX edge reading `Tokens`, `Names`, and `Authz` on enam), apac's commit 716 ms p50 (one round from apac plus three dependency reads to enam), and an apac edge-to-shard tail in 2 of 6 apac samples over 2 s. On the final build journeys 2 and 3 pass 10 of 10, BF-1 2 of 2, BF-21 10 of 10 with 5 leader kills, and BF-23 10 of 10, with every reply valid against GHES 3.22. No driver passes a token on a command line any more (git takes the header from `GIT_CONFIG_COUNT`, curl from `-H @<(printf …)`, and the Python drivers redact every log line); a scan of 38,052 raw files for every install, tools, bed, and Cloudflare token found 0 hits, after the earlier hits were redacted in place.

Per-row recheck on the final build (sites from site day `sd10` with `--roll-shards`, edge `3a2e4f02`; rows marked sd8 ran before the two shard-merge fixes, which touch only the merge path):

| Row | Recheck | Result |
| --- | --- | --- |
| BF-1 | 2 queue runs (`bf1s`) | Pass 2 of 2: each landing's 5 `Merged`, `Landed`, and one 15-update `Refs` in one entry on all 3 members with one digest |
| BF-20 | Smoke: digests at 1720 and 1738 on `repo:196628` after two site days, and 3 pushes | Smoke passes: all three members agree; the first pushes after two idle days took 7.1 and 14.4 s (cold tier load); the 300/s campaign was not rerun |
| BF-21 | 10 runs, 5 leader kills (`bf21t`, sd10) | Pass 10 of 10, each kill ending whole in 93 to 120 s; the first try (`bf21r`, 9 of 10) stalled 9.5 min on a stopped container, fixed by `576badf` and `4c3727e` |
| BF-22 | 500 statuses, 8 posters (`bf22e`, sd8) | Pass after two fixes: 500 of 500 through `gh pr checks` and 5 REST pages, core rows unchanged but for `consumer_cursor`, both tiers' digests equal on all members; the prune clause stays with the pure test |
| BF-23 | 10 + 10 (`bf23r` sd7, `bf23s` sd10) | Pass 20 of 20 (merge 8, squash 6, rebase 6) |
| BF-24 | 10 + 10, with conflicts and a transfer (sd7, sd10) | Pass 20 of 20 within 30 s: clean 0.83 to 2.10 s, conflicts 1.03 to 1.71 s |
| BF-25 | `git lfs` and `gh release` smoke | LFS and asset bytes match, both blobs in all 3 buckets; the unreadable-bucket clause has no hosted hook |
| BF-26 | All 6 `archive-cache/` objects deleted, then `gh release download --archive=tar.gz` | Pass: same bytes, object rebuilt |
| BM-2 stale `parent_tip` | 100 (`bm2f`, sd10) | Pass 100 of 100 |

## Milestone 3: identity and authorization

Built (`design/DESIGN.md`, "Milestones 1 to 7"): the rest of `Tokens`, `Names`, `Authz`, and `Identity`: fine-grained and paired tokens, OAuth web and device flows with short-lived codes, Apps and OAuth apps, stateless installation tokens signed in `graft-ops` with `jti` revocation, SSO through `InstanceAuth`, collaborators and invitations, transfers, `InstallSnapshot`, and learners for hot groups, plus the T0 identity surfaces [inference]. Milestone 1 built the minimal identity (M1-WP7).

### M3-WP1. App and OAuth app records and their names

**Build.** `ClientId`, `SecretHash`, `AppRecord`, `AppKey`, and `OAuthAppRecord` on the owner's `Authz` chain, as `AuthzEvent::App` and `AuthzEvent::OAuthApp` (`design/SHARED.md`, "Identity and tokens"). The `NameKey` variants `AppSlug`, `Client`, `App`, and `Installation`, and the `NamesEvent` variants `AppSlug`, `AppRegistered`, `ClientRegistered`, `InstallationRegistered`, `Shortcode`, `EmailClaimed`, `FormerName`, and `Retired`, which hold only routes to an owner. Registration claims `AppSlug` first, the commit point, and `Audit` finishes a registration nobody finished (`design/DESIGN.md`, Shape, "Identity").

**Depends on.** M2.

**Acceptance.** No gate row of its own. It carries the App half of BF-32 (M3-WP4) and BM-3's journey 4 name reads.

**Built so far (2026-11-06), the pure half, with M3-WP2** (`crates/identity`, commits `3041df8` and `36bb7f9`; `crates/identity/tests/m3.rs`). `AuthzEvent::App` (tag 8) and `OAuthApp` (tag 9). `PutApp` and `PutOAuthApp` read the claim's `Names` shard and refuse unless the slug's `AppSlug` names the App id and owner, or the client id's route names the owner, so the claim is the commit point. `NamesEvent::AppSlug` carries `owner`, so the `Audit` finisher knows whose `Authz` chain to finish a claim on. A GitHub App's `ClientId` is `Iv1.` and its App id in 16 hex digits [inference], and `CLIENT_SECRETS_MAX` is 2. The `AppSlug`, `Client`, `App`, `Installation`, `Shortcode`, and `Email` keys and their events. F50's `NamesEvent::AccountLogin { account, login }` (tag 12) on `account_shard`, written by `SetAccountLogin`, whose leader reads the login's own shard and refuses unless the account holds the login there; the edge reads other accounts' logins with one read per `account_shard`, at most 24 a request [guess], and keeps the test-only `LOGINS` only as a fallback for accounts with no row (`decisions/DECISIONS.md`, "F50 closed: an account's login row on `account_shard`"). The `Audit` finisher plans for an unfinished registration and a missing login row (`finish.rs`). Milestone 1's fold blobs and event bytes are unchanged. Formats: Names, Tokens, and Authz v2 (`0x10.2`, `0x11.2`, `0x12.2`) for every command that writes an event Milestone 1 did not have, which the site reads now; a member needs a site day onto a release that lists them, and the edge sends them only after `FormatsEnable`. Not yet: hosting beyond the login read, and BF-32.

**Hosted (2026-11-07).** F50's login rows render with `LOGINS` empty (787 of 787 the same), and BF-32's App half passes; see "M3-WP1 to M3-WP4 status" under M3-WP4.

### M3-WP2. Token kinds and short-lived codes

**Build.** Every `TokenKind`, `fingerprint_shard`, `jti_shard`, and the rest of `TokensEvent`: the optional `Stored` fields (`app`, `client`, `permissions`, `resource_owner`, `repos`, and `paired`), the `ghr_` redrawn onto its `ghu_`'s shard, `SshKey`, `SshKeyRemoved`, `JtiRevoked`, and `VerifyingKey`, and the `Code`, `CodeApproved`, and `CodeUsed` rows with `CodeKind`, which lapse at `expires` (`design/DESIGN.md`, Shape, "Identity"; `design/SHARED.md`, "Identity and tokens"). The `tokens` surface at T0.

**Depends on.** M3-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-2 | Revoke a token while 100 of its pushes wait in admission | None is acknowledged at an index after the revocation's | Any one is |
| BF-31 | A `github_pat_` scoped to one repository with `contents: read`; a `ghu_` and `ghr_` refresh; `DELETE /applications/{client_id}/token` | The PAT is refused outside its scope; revoking either paired token refuses the other; the DELETE revokes | Any use after revocation |

**Built so far (2026-11-06), the pure half** (`crates/identity/src/tokens.rs`, with M3-WP1 above). Every `TokenKind` with its fields, SSH keys, verifying keys, `jti` revocations, lifeboat reads, and device, authorization, and sign-in codes, routed by `fingerprint_shard` and `jti_shard`. `TokenKind::Installation` exists for parsing and is never stored. A `Stored` with optional fields is written as `TokensEvent::StoredWith` (tag 11), so Milestone 1's `Stored` rows keep their bytes. `UseCode` spends a code once, and a replay under a new request id after a leader change is refused. An edge that dies between `CodeUsed` and the new token's `Stored` leaves a spent code and no token; the client starts again (`design/SHARED.md`, "Identity and tokens", OAuth). Not yet: BF-2, BF-31, and the `tokens` surface hosted.

**Hosted (2026-11-07).** Device codes, `UseCode`, `StoreWith`, `JtiRevoked`, and `VerifyingKey` run on the hosted `Tokens` shards; BF-2 and BF-31 have not run hosted; see "M3-WP1 to M3-WP4 status" under M3-WP4.

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

**Built so far (2026-11-06), the pure half** (`tests/m3/results/m3-wp3.md`). In `crates/identity`: `authorize`, `authorize_write`, and `Decision` over the owner's `Authz` and the repository's access rows, with `ReadAction`, `WriteAction`, and `OrgPolicy` as `design/SHARED.md` now gives them; pushes, issue commands, and merges go through it. `Authz` teams, team grants, `OrgPolicy`, org invitations with a 7-day TTL, `InstanceAuth` written only by an admin of the install's own org, `AuthzEvent::Enterprise` (tag 13) written only by `graft-ops`, and the transfer's `Reserved`, `Live`, and `Alias` steps. Web sessions as `Credential::Session`, whose `CredentialKey::Session { issued }` every write checks against the `Account` row's `sessions_not_before`. The `Identity` chain, a new family: `Created`, `Credential`, `Sso`, `Audit`, and `Session` (tag 5), with sign-in against `InstanceAuth`, a credential listed only once its `Tokens` row is live, and the `Audit` finisher `finish::credential`. On the repository chain, `RepoEvent::Collaborator` (tag 25) and `Invitation` (26), with an org member refused as a collaborator; a user command's `deps` name the owner and enterprise from `RepoSettings`. The transfer finisher plan (`finish::transfer`: move, `Settings`, `Live`, `Alias`, `RepoOwner`) and the edge's follow-up commands (`crate::edge`). BF-3, BF-30, BF-33, BF-34, BF-35, and BF-36 pass in memory on three real members per chain: 200 of 200 pushes refused after a suspension; 200 of 200 keys with one owner; every route resolving through a transfer across and within placements; 100 of 100 device-flow logins completing under leader kills, 25 restarting cleanly, and replay and expiry refused 100 of 100; invitations and collaborators never overlapping; 100 of 100 key and PAT pairs listed at once; and only a site admin setting `InstanceAuth`. 13 of 13 planted bugs caught (`tests/m3/wp3/plant.sh`). Formats: Authz 0x12.2, Identity 0x14.1 (new), and repository 32.6, beside M3-WP1 and WP2's Names 0x10.2 and Tokens 0x11.2; a member reads them only after a site day onto a release that lists them, and the edge sends none until `FormatsEnable` (or the test edge's `FORMATS_ENABLED`) enables them. Deferred: `Credential::Stateless` (M3-WP4), `Ssh` (M6-WP2), and `RunnerJwt` and `RuntimeToken` (Milestone 4); the `SiteAdmin` bypass (`decisions/OPEN-QUESTIONS.md`, F54); and a transfer `Settings` command, because the repository chain's `Settings` refuses any owner or enterprise change (`OwnerChange`), so the finisher's `Settings` step has no command to send yet (`decisions/DECISIONS.md`, "M3-WP3's identity shapes as the code needs them").

The hosted half needs: that site day and `FormatsEnable` for 0x10.2, 0x11.2, 0x12.2, 0x14.1, and 32.6, with the edge sending no M3 command before it; the transfer `Settings` command; the edge's routes for signup, sign-in, rename, `rest:orgs`, `rest:teams`, `rest:users`, `rest:collaborators`, `rest:deploy-keys` over HTTPS, and the OAuth web and device flows, each sending `crate::edge`'s steps; the `AccountLogin` backfill for accounts seeded before F50, then a run that renders every login with `LOGINS` empty; the `Audit` consumer submitting `finish::credential` and `finish::transfer`, with the move Workflow driving the transfer's group move through `Joint`; `graftyard admin bootstrap`; and BF-34's `TestAccGithubRepositoryCollaborator`.

**Hosted (2026-11-07).** The `Identity` family, org membership, and the OAuth device and web flows run hosted with real gh, behind test routes for the browser pages; the `Audit` consumer, `admin bootstrap`, the remaining REST surfaces, and the transfer are not built, and `after_signup` lacks the placement copy; see "M3-WP1 to M3-WP4 status" under M3-WP4.

**Read gate (2026-11-08), hosted 2026-11-09.** Before this, no edge route called `authorize` for a read: anyone could clone any repository, REST reads ran with no viewer, and LFS transfer ignored the caller. Now `authorize_read` (`crates/identity/src/access.rs`) gates every repository route at the edge: git info/refs, upload-pack, and receive-pack, REST, GraphQL by node id, LFS batch and transfer, uploads, codeload, and a user's invitation. The edge reads the repository's settings and collaborator rows with `{"q": {"rows": {"access": true}}}` and the owner's `Authz` fold in parallel, and caches the decision per repository and stored token for `READ_CACHE` (`workers/edge/src/lib.rs`, `read_gate` and `gate`). An anonymous git request for a repository that is not public gets 401, a signed-in caller who cannot read gets 404, and a missing owner fold answers 503, never 404. A REST repository route that gets a credential the edge refuses (revoked, expired, or unknown) answers 401 "Bad credentials" before the gate runs, as GitHub does, so a bad token on a public repository is refused rather than served as anonymous (`932cc33`). Hosted on `graft-m2b` after a site day from `3160687` (`tests/m3/results/m3-repos.md`, "Edge read gate"): 12 of 12 per case, anonymous clone of a private repository 401 with `WWW-Authenticate: Basic realm="GitHub"` and of a public one served, a non-reader's clone and `gh api repos/o/r` 404, a collaborator 200 and a clone, a non-reader's LFS batch, codeload, and tarball 404, a revoked token's git read 401, and one leader read of the access query returning only the settings and collaborator rows; the owner, writer, and reader drivers, BF-34, BF-35, and Terraform's `TestAccGithubRepositoryCollaborator` passed behind it. The revoked token's REST read answered 404 there, before `932cc33`. On `graft-m1` (site day 6, `am1-hosted`) AM1's agent routes passed behind the gate (`tests/am1/results/am1-hosted.md`). GHES's 401 body for an anonymous clone is F58.

### M3-WP4. Stateless installation tokens in `graft-ops`

**Build.** `InstallationClaims` and `ghs_` tokens as compact JWS with `kid` (`design/SHARED.md`, "Identity and tokens"). Minting in `graft-ops` with a signing key wrapped by a Secrets Store key and cached for at most `SIGNING_KEY_CACHE`, `TokensEvent::VerifyingKey` on `TokenShard(0)`, and `DELETE /installation/token` committing `TokensEvent::JtiRevoked` to `jti_shard` (`design/DESIGN.md`, Shape, "Identity"; journey 4). App JWTs verified against `AppRecord.keys`. The `rest:apps` and `github-apps` surfaces at T0. The edge mints through a `fetch()`-based binding to `graft-ops`. The `OpsPointer` override on that binding arrives with M5-WP2 [inference].

**Depends on.** M3-WP1, M3-WP2, M3-WP3.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-27 | Revoke a `jti` with 1,000 commands under its installation token queued | 100% of commands decided afterwards get 401 | Any accepted |
| BF-32 | `@octokit/app` installation-token flow with a newly registered App; OAuth web flow with a registered OAuth app and a wrong secret | The JWT verifies against `AppRecord.keys`; the wrong secret gets 401; a dump of every `Identity` chain shows no client secret | Any other result |

It also carries BM-3's revoked stateless installation token.

**Built so far (2026-11-06), the pure half** (`tests/m3/results/m3-wp4.md`). In `crates/identity`: `ghs_` tokens as `ghs_`, the App id, `_`, and a compact EdDSA JWS with `kid` (`stateless.rs`), of variable length, 338 characters in the BF-32 gate, every one matching Renovate's `ghs_[A-Za-z0-9\.\-_]{36,}`; the edge's offline `parse` and `verify` against the `kid`'s `VerifyingKey` row, refusing a retired key, a lifetime over 1 h, and an `iat` more than `CLOCK_SKEW` (60 s [guess]) ahead; `Minter::mint`, asking a `KeySource` for the unwrapped key at most once per `SIGNING_KEY_CACHE`; `Credential::Stateless`, `Actor::App`, and `CredentialKey::Jti`, with the leader reading `jti_shard` once per entry and refusing on a held `JtiRevoked` row, and `CredentialShard::is_operator()` (no account and no key) replacing every "no account means the operator" check; `DELETE /installation/token` as `RevokeJti` under the token itself; `AuthzEvent::Installation { id, row, not_before }` (tag 5) with `InstallationRow`, committing after its `Names` route and within the App's permissions, a narrowing or uninstall moving `not_before`; `authorize` for an App, intersecting the claims with the installation's current row; App JWTs verified RS256 against `AppRecord.keys` and client secrets compared by SHA-256 (`appauth.rs`); the token request's narrowing, at most `TOKEN_REPOS_MAX` (500 [unverified]) repositories; and `rotation_step`, publishing the new `kid`, switching after one token lifetime, and retiring the old `kid` at the switch plus `SIGNING_KEY_CACHE`, one lifetime, and `CLOCK_SKEW`. In `crates/git`, push format 0x20.7 carries an App actor and its claims. In memory, on three real members per chain: BF-27 passes (1,000 of 1,000 queued `Authz` commands, and 1,000 of 1,000 queued pushes, refused 401 after the revocation); BF-32's pure half passes (three JWTs from `universal-github-app-jwt` 2.2.2, the signer `@octokit/app` uses, verify against the record's key, five bad ones are refused, the token narrows to what was granted, a wrong OAuth secret fails, and no `Identity` chain holds a client secret or its hash); rotation keeps both keys valid through the overlap and refuses the old `kid` after `retired`; and installations stay within their App and fail every earlier token on a narrowing or uninstall. 14 of 14 planted bugs caught (`tests/m3/wp4/plant.sh`). New dependency: `rsa` 0.9 for public-key verification only, so RUSTSEC-2023-0071 does not apply.

The hosted half needs: a site day onto a release that reads 0x20.7 beside 0x10.2, 0x11.2, 0x12.2, 0x14.1, and 32.6, then `FormatsEnable`, with no `ghs_` push and no `Install` sent before it; a check that no deployed `Authz` fold holds a trailing part before 0x12.2 is enabled, because `AuthzMore.installations` is a new positional field and a part written by an earlier M3 build would not decode [inference]; `graft-ops` with a `KeySource` unwrapping the signing key with `MintWrapKey`, the `fetch()` route the edge's binding calls with a `Grant`, and `graftyard mint-key rotate` driving `rotation_step`; the edge's `POST /app/installations/{id}/access_tokens`, `DELETE /installation/token` (204), `ghs_` authentication against a `TokenShard(0)` read within `READ_CACHE`, and the installation flow (`RegisterInstallation`, then `Install`); a stateless-principal wire form for issue and merge commands, which today carry no account and no key, so the leader refuses them `Unauthorized(NoToken)` and they fail closed (the reply's status is the route's: see the hosted status below); and BF-32 over HTTPS with `@octokit/app` against the deployed edge, with BM-3's revoked stateless token in journey 4.

**M3-WP1 to M3-WP4 status (2026-11-07): the hosted half on `graft-m1-*`** (`tests/m3/results/m3-hosted.md`). One round took the four packages' pure halves onto the install; nothing ran on `graft-m2b-*`.
- **Site day and formats.** A site day with `--roll-shards` onto a release that reads every M3 format: `Identity` as a new family under the owner's placement, Names 0x10.2, Tokens 0x11.2, Authz 0x12.2, Identity 0x14.1, push 0x20.7, and repository 32.6 and 32.7. Before 0x12.2 was enabled, no deployed `Authz` fold held a trailing part, so `AuthzMore.installations` decodes. The edge enables them with `FORMATS_ENABLED`, standing in for `FormatsEnable`, and sends a `ghs_` push only once 32.7 is enabled. A clone and a push worked after the day.
- **`graft-ops`.** `graft-m1-ops` (`workers/ops`) mints `ghs_` tokens through `POST /mint` on the edge's `OPS` service binding, with a real Secrets Store `KeySource`: it reads the `MintWrapKey` secret only when `Minter::needs_key` says the cache needs a key, and unwraps it in the layout DESIGN now gives ("Install operations", the rotation table). Kid 1's `PutVerifyingKey` was committed once by hand (`mintkey.rs`).
- **Edge routes.** `ghs_` authentication against `TokenShard(0)`'s `VerifyingKey` with a fresh `jti_shard` read; `POST /app/installations/{id}/access_tokens` and `DELETE /installation/token`; `POST /login/device/code` and `POST /login/oauth/access_token` running `UseCode`, `StoreWith`, then `AddCredential`; `DELETE /applications/{client}/token`; `GET /user`, `GET /users/{u}`, org memberships, and GraphQL `viewer`. `GET|POST /login/oauth/authorize` and `POST /login/oauth/device/approve` are PAT-authenticated test routes standing in for the browser pages (M4-WP5).
- **F50 hosted.** The `AccountLogin` backfill claimed the missing logins, wrote the rows, and planned nothing on a second run. With `LOGINS` empty, the edge rendered the same 787 logins across 9 repositories as with it set, with no difference.
- **BF-32: pass, 12 of 12, twice,** with `@octokit/app` 16.1.4 against the deployed edge: a 344-character `ghs_` token matching Renovate's regex, a read with it, narrowing, 422 for a permission the App or the installation lacks, 401 for a JWT under another key, 204 on revoke and 401 after it, and the OAuth web flow with 401 on a wrong secret and 200 on the right one. Its issue-write case fails closed with 403 (below).
- **Device and web flow.** BF-33's replay cases pass 10 of 10 per case, twice: `authorization_pending` before approval, one `gho_` after it, no token on a second exchange or a made-up code. Real gh 2.102.1 `auth login` over the device flow logged in 20 of 20 in two sets of 10, and `auth login --web` 1 of 1. BF-33's leader-kill case ran only in memory.
- **Replies.** 10 GraphQL and 14 REST replies validate against GHES 3.22 with 0 failures.
- **Journey 4 over HTTPS, 100 rounds** with stock git 2.56.0 against `m3org/j4`: 100 of 100 PAT pushes accepted and 100 of 100 refused 401 after revoke with the ref not moved; 100 of 100 `ghs_` pushes accepted and 99 of 99 refused after revoke. Round 97's `DELETE /installation/token` got no reply in 120 s, and Workers Observability shows no request at the edge in that window, so that token was never revoked and its next push was rightly accepted [inference: Observability logs every invocation]; the cause is unknown. Member removal did not run in the 100-round run (the script was edited while bash read it); it ran on its own right after, removal 204 and the push refused 403, and in 5 more full rounds that passed every step. Across the three runs 209 of 209 revoked tokens were refused on the next write. **BM-3's clause 1 is not yet closed**, because one of the 100 `ghs_` revokes never ran; a rerun is on `m3-close`.
- **Tokens and logs.** No token on argv or in a URL; the raw output and the account's Workers Observability for 6 h hold no token, code, or key.
- **Latency.** A device-code request takes a median 13 s and a token exchange about 15 s, because each commit on a code's `Tokens` shard, one of 3,072, first creates that shard's group (R20, since closed: `decisions/DECISIONS.md`, "Install creates every `Tokens` shard"). gh polls every 5 s for 900 s, so logins complete.
- **Fails closed with 403, not 401** (since replaced by the stateless form, below). An issue or merge command under a `ghs_` token carries no account and no key, so the leader refuses it `Unauthorized(NoToken)`; the label route renders that as 403 "Resource not accessible by integration" [measured, BF-32], while other routes render the same refusal as 401 "Bad credentials" (`crates/api/src/rest.rs`, `label_refused` and `refused_merge`; `graphql.rs`). The stateless issue and merge wire form is being built on `m3-close`.
- **Signup placement.** `edge::after_signup` writes the login row, the `Account` row, and the `Identity` chain's `Create`, but not the owner's `Placement` copy that DESIGN requires before any owner chain (Shape, "Sites and placement", `PlacementRecord`); `setup.ts` wrote `InitPlacement` for every seeded account by hand. Fixed on `m3-close` (`1fce620`, below).

Not built: `graftyard admin bootstrap` and `graft-ops`' sign-in link; `graftyard mint-key rotate` on the hosted install (rotation is tested in memory); the App installation flow at the edge (`RegisterInstallation` and `Install` were committed as the operator); REST for teams, collaborators, invitations, and SSH keys; `ghu_` user-to-server tokens; a stateless wire form for issue and merge commands; the `Audit` consumer for the hosted chains, so BF-3, BF-30, BF-33's leader kill, BF-34, BF-35, and BF-36 ran in memory only; the transfer `Settings` command over HTTPS; and the browser pages for the device and authorize flows. BF-2 and BF-31 have not run hosted.

**M3 close round (2026-11-07)** (`tests/m3/results/m3-close.md`; branch `m3-close`, merged at `655fee3`). Site day 3 onto `420ae12`'s site code with `GRAFTYARD_AUDIT=1`; edge `8a824825` with repository format 32.9. Built:
- **Stateless issue commands** (`65a7b07`, `1815c70`): under a `ghs_` token an issue command is format 32.9 and carries the verified `InstallationClaims`, and `decide` authorizes it with `authorize_app` against the installation's current row. Hosted 10 of 10 each: an App with `issues: write` creates a label (201), and one with `contents` only gets 403 "Resource not accessible by integration" (`decisions/OPEN-QUESTIONS.md`, F56, for GHES's own status). A merge by an App answers 403 at the edge, because `Merged` names a `UserId`.
- **The hosted `Audit` consumer** (`776eaa4`, `023116e`): `AuditCursor` on `Names` and `Tokens` shards (formats 0x10.3 and 0x11.3), one batch in band from the leader's alarm, finishing credentials, App registrations, and login rows at least `FINISH_AFTER` (60 s) old. Hosted, a PAT stored with no `AddCredential` was listed on its owner's `Identity` chain by the next batch. It found two Milestone 1 seed accounts with no `Placement` or `Identity` chain, retrying every `RETRY_MS` (30 s [guess]) with no limit, until `m1-identity.sh` wrote them. `finish::transfer` is not submitted.
- **F51** (`212aa36`), and **`after_signup`** copying the install account's `Placement` record as `InitPlacement` (`1fce620`); no edge signup route exists, so seeds still write it by hand.
- **`graftyard admin bootstrap`** (`0479288`): 11 of 11 links spent once at `POST /login/oauth/link` (201, `m3-admin`, method `Link`), a second use and a one-character change 401. It answers a session id and sets no cookie (DESIGN, "Install operations", First admin).
- **`graftyard mint-key rotate | step`** (`0479288`): kid 2 is published (`PutVerifyingKey` at index 24 on `tokens:0`), and mints still sign with kid 1. The switch and kid 1's retire followed ("Milestone 3 status", done since).
- **Hosted rows as far as the routes reach:** BF-3 (200 of 200 pushes refused after a suspension, one key from two requests 100 of 100 with one owner); BF-33's leader kill (below); BF-34's org half; BF-30's enterprise half; BF-35 on the `Identity` fold (100 of 100); BF-36 on the `Identity` command the edge would send after OIDC. 8 of 8 planted bugs caught (`tests/m3/close/plant.sh`).
- **BF-33 hosted only ever restarted:** 120 of 120 runs with the code shard's leader killed on the exchange's `UseCode` (`after_quorum`, `before_commit_sent`, `before_commit_early`) ended `expired_token`, a clean restart, with no token from a spent code. In every run the killed command committed and the edge's re-send found its outcome, so the "completes" branch never happened hosted; its evidence stays gh's 20 of 20 logins and the in-memory 100 of 100.

### M3-WP5. Learners and snapshot install

**Build.** `MemberKind::Learner` with its own name slot, `LEARNER_AT`, `MAX_LEARNERS`, `place_learners` as a daily `graft-ops` cron over Analytics Engine counters, and learner read-index (`design/DESIGN.md`, Shape, "Sites and placement" and "The read path"; "Dominant access patterns", hot group read). `InstallSnapshot` for a member whose next entry the leader has archived, which new learners and `ReplaceMember` members also use (Shape, "Packs").

**Depends on.** M1-WP2, M2-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-28 | A learner on its voter's site, and two learners on one site | Distinct objects | A name collision |
| BF-29 | Follower offline past the archive boundary, then rejoins | Catches up by `InstallSnapshot` with matching fold digests | Stuck or divergent |

It also carries BM-3's "Learners rerun the Milestone 0 learner gate" (the rerun of B0-18).

**Built so far (2026-11-06), the pure half** (`tests/m3/results/m3-wp5.md`). `crates/types`: `LEARNER_AT`, `MAX_LEARNERS`, `GroupLoad` (peak reads per region), `place_learners`, learner addresses `SiteId(192 + slot)`, and `Placement::new`'s refusal of a repeated slot or a second member on one object. `crates/raft`: learners replicate but never vote, lease, commit, or campaign; read-index for followers (10 ms batches) and learners (one batch in flight, a 50 ms floor); and `InstallSnapshot` from `Member::set_install`, after which the member acks nothing until `Member::install` has applied `through`. `crates/fold`: streaming `Restore` and `refold_segment`. BF-28 and BF-29 pass in memory on real raft members in `crates/fold/tests/install.rs`: six members, six objects, six addresses, learners reading the leader's latest write and committing nothing; and a follower down from index 7 rejoins by one install past an archive boundary, with both tiers' `fold_digest`, request rows, and config equal to the leader's. 9 of 9 planted bugs caught; the long sweep (2,000 seeds) and Porcupine (HARSH 156,723 and GRAY 2,226,280 operations) are Ok, though the simulator runs no learners or installs. Hosting followed (below).

**M3-WP5 status (2026-11-07): the hosted half passes on `graft-m2b`: BF-28, BF-29, and the B0-18 rerun** (`tests/m3/results/m3-wp5.md`, "2026-11-06: the hosted half" and "2026-11-06 (evening)"). Built: one loader in `workers/site/src/load.rs`, shared by `InstallSnapshot` and the fold audit, which runs one batch per alarm (at most 2 s, 1 MB read, or 20,000 rows and entries), writes into scratch tables, commits each batch with its cursor, and abandons a batch that began three times without committing, so a batch that kills its isolate cannot keep resetting its neighbours (F47); the leader names an install source after it truncates, and a member that installed names one from its `log_base` (`222fee6`); learners joined like voters, with no `MemberRegistry` or RTT rows, at `SiteId(192 + slot)` mapped to `(site, do_name)` through the config; `place_learners` from the leader's `/learners` route, because `graft-ops` is not deployed, so the cron's Analytics Engine read is not built; and the site's fold audit cron, which refolds from snapshots (DESIGN, "Packs", `R2Key::Archive`'s `gyfarch\0` format; `design/SHARED.md`, "Folds"). Results:
- **BF-29: pass.** On `repo:65539`, site 2's member held down from index 7 while 30 real pushes committed and 1..=40 archived; it installed in 1 batch of 2,954 ms (both snapshots, 40 entries, 38 `request` rows kept) and caught up, and at index 59 all three members had equal core and metadata `fold_digest` and equal `request` rows.
- **BF-28: pass.** `place_learners` gave 16 regions; 16 learners joined at addresses 192 to 207, with 19 distinct objects and 19 boot ids, every learner a follower, 15 caught up by `InstallSnapshot`.
- **F48 hosted.** A deep audit refolded `repo:65536`'s core tier from its snapshot at 2 through 5,788 entries (107.7 MB of log) in 213 batches, peak wasm memory 64.9 MB, equal to all three voters and through the snapshot at 5,735's trailer. Its 2.4 GB metadata tier was skipped as over `AUDIT_SNAPSHOT_MAX` (`decisions/OPEN-QUESTIONS.md`, F52). Two fixes came from it: 1 MB batches abandoned after three deaths (`aa5e0f4`), and log pages read to their end (`d6e350d`).
- **B0-18 rerun: passes 6 of 6 on `graft-m2b` after a call-depth fix** (`tests/m3/results/m3-wp5.md`, "2026-11-06 (evening): B0-18 traced on `graft-m2b`"). Each run: `b0-18.ts --repo 65539`, 16 learners x 1,000 read-index reads/s for 30 s under 10 pushes/s, all 19 members reset onto `e174090`. The leader ran at 243 to 249 req/s against 700, the worst learner p99 was 560 to 853 ms against 1 s, with 0 failed reads, 0 depth failures, and no `STEP_DOWN`; 7 calls were hedged, all from one apac learner. The earlier failures, 2 of 6 and then 12 of 12 traced runs, were the leader stepping down: learners called the leader with `ReadIndex` while the leader called learners with AppendEntries and late `ReadIndexRes`, and the runtime charges an object's outgoing calls to the newest request it received, so each exchange added 2 to the call depth until the leader's AppendEntries to its voters failed "Subrequest depth limit exceeded" and tripped `STEP_DOWN`. Sending every learner outside shallow pings made the same runs pass, which confirmed it. The fix, in the site host only (`crates/raft` unchanged): a member never calls a learner that has a call open at it or called within `QUIET` (1 s [guess]), and holds that learner's messages for the response to its next call (`workers/site/src/callers.rs`, `06762a2`); a learner's `ReadIndex` that needs a round waits in its own call up to `READ_INDEX_HOLD` (500 ms [guess]); and a learner call unanswered after `HEDGE` (500 ms [guess]) is sent once more (`e174090`), for the single call delayed on the enam-apac path that held the last two failing runs' reads to `CALL_DEADLINE`. The fix alone passed 8 of 12 runs; with the hedge, 6 of 6. The test `a_learner_that_calls_the_leader_is_never_called_back_so_depth_stays_bounded` models the depth rule: the old routing nests past 16, the new stays at 3. The design carries the rule (DESIGN, Shape, "Members reach each other"; "The read path"; Scale, learners; Failure modes, chain groups, noisy-tenant). BM-3's "Learners rerun the Milestone 0 learner gate" is met on this build. Rechecked on it: BF-28 passed in all 30 runs of the round, and BF-29 passed again on a fresh repository, `m2b/bf29r` (`repo:65540`): one install of 1,249 ms, then equal digests on all three members at index 53.
- **Damaged test repositories.** `repo:65537` and `repo:65539` on `graft-m2b` are unreadable over git: every read answers 500 from the shard's `PUT /refs`, because B0-18's driver pushed refs naming made-up object ids through `/_bed/push`, and the shard image since `ca15db8` refuses them when it rebuilds refs on catch-up. It is a driver fault, not a member or raft regression. Use fresh repositories for git-path gates on `graft-m2b` until the `b018/*` refs are removed or `b0-18.ts` pushes real objects.
- **A site day left busy members on the old version.** At 22:58Z the three voters kept the previous build until each was sent `/reset`, so every deploy that round was followed by `/reset` on all 19 members (`decisions/OPEN-QUESTIONS.md`, R19).

Not yet: `place_learners`'s `graft-ops` cron, a product archiver (the test route builds the archive object in one isolate's memory; a product one needs a multipart put), and an audit for tiers over `AUDIT_SNAPSHOT_MAX`.

### M3-WP6. Milestone 3 gate

**Build.** Nothing new.

**Depends on.** M3-WP1, M3-WP2, M3-WP3, M3-WP4, M3-WP5.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| BM-3 | Journey 4 passes over HTTPS, with revoked tokens, a revoked stateless installation token included, refused on the next write 100 times out of 100. Learners rerun the Milestone 0 learner gate |

**BM-3 status (2026-11-07): passes** (`tests/m3/results/m3-close.md`, "BM-3" and "The learner half of BM-3"). Journey 4 ran 100 rounds over HTTPS on edge `8a824825` and the day-3 sites, 18:19 to 19:19Z, with stock git against `m3org/j4`: 100 of 100 PAT pushes accepted, revoked with 204, and refused 401 on the next push with the ref not moved; 100 of 100 `ghs_` pushes accepted, 100 of 100 `DELETE /installation/token` answered 204 with none re-sent (0.46 s median, 0.94 s max), and 100 of 100 refused 401 with the ref not moved. At round 50, inside the run, `m3-admin` removed `m3-member` from `m3org` (204), the member's live PAT was refused 403 with the ref not moved, and the re-add answered 200. The earlier run's round-97 `DELETE` never reached the edge (Workers Analytics shows no requests for that minute), so it was a client or network loss, not a revoked token accepted; the driver now records each revoke's timings and re-sends a `000` once. The learner half counts B0-18's 6 of 6 on `graft-m2b` (M3-WP5); it was not rerun on `graft-m1`, and counting it is a judgement from the commit history since `e174090`, in which a learner's code changed only to host the `Identity` family and to read issue formats that B0-18's load never sends [inference, `git log e174090..`].

### Milestone 3 status (2026-11-09)

Milestone 3 is closed: every gate row below is Done, the last being BF-34's `TestAccGithubRepositoryCollaborator` on `m3-repos` (2026-11-09, `tests/m3/results/m3-repos.md`). BF-30 ran on `graft-m1` (`m3-finish`, merged at `b767933`); the `m3-rows` rows ran on `graft-m2b` after four site days, the first onto main's build `e2c67b5` and the last onto `db1c6be` (`m3-rows.md`, "What was deployed on graft-m2b"); `m3-repos` ran on `graft-m2b` after a site day from `3160687`. The two test installs: `graft-m1` has four site scripts since 2026-11-08, `graft-m1-site-a` to `graft-m1-site-c` and `graft-m1-site-d` in wnam (added for BF-30's move across placements), with `graft-m1-edge`, the tools edge `graft-m1-wp7-edge`, and `graft-m1-ops`; `graft-m2b` has three, `graft-m2b-site-a` to `-c`, with `graft-m2b-edge` and `graft-m2b-ops`. Three rows rest partly on runs outside the hosted install, and the table says which: BF-27's 1,000-queued count ran only in memory, BF-32's no-secret dump ran only in memory, and BM-3's learner half counts B0-18's `graft-m2b` run as a judgement.

| Row | State | What closes it |
| --- | --- | --- |
| BF-2 | Pass on `graft-m2b`, 9 of 9 runs: 600 of 600 pushes held behind a revocation refused 401, 0 acknowledged at or after the revocation's index; the 3 controls acked 300 from reads below it (`m3-rows.md`, "BF-2") | Done |
| BF-31 | Pass on `graft-m2b`, 10 of 10: a scoped `github_pat_` refused outside its scope; revoking either of a `ghu_`/`ghr_` pair refuses the other; `DELETE /applications/{client_id}/token` revokes; 0 uses after revocation (`m3-rows.md`, "BF-31") | Done |
| BF-3 | Pass hosted on `graft-m1`: 200 of 200 pushes refused after a suspension, one key from two requests 100 of 100 with one owner (`m3-close.md`, "BF-3"); the SSH push is BF-52 (M6-WP2) | Done |
| BF-30 | Pass on `graft-m1`, 20 of 20 transfers and 0 of 2,313 route samples lost: across enterprises, across placements (`Tri` [0,1,2] to `Na3` [0,3,1] home 3, with the new `graft-m1-site-d` in wnam), both at once, home only, and back; `deps` named the new owner after `Settings` in 20 of 20; a finisher killed after `Reserved` finished by `Audit` alone in 76.2 s with 0 of 586 samples lost; an org's enterprise change gave 2 of 2 repositories their `Settings`; 9 of 9 planted bugs (`m3-finish.md`, "BF-30") | Done |
| BF-33 | Pass on `graft-m2b`: with the new token's `Tokens` leader killed after `UseCode` committed and before the reply, 99 of 100 completed with exactly one token and 1 restarted cleanly; replay and codes older than 900 s refused 100 of 100 each, kept from an earlier run on the same install (`m3-rows.md`, "BF-33") | Done |
| BF-34 | Pass on `graft-m2b`: the driver 10 of 10 with 170 of 170 replies valid against GHES 3.22 (`m3-rows.md`, "BF-34"), and Terraform's `TestAccGithubRepositoryCollaborator` as written 6 of 6 runs, 12 of 12 subtests (terraform-provider-github `1b9c5c1`, Terraform 1.17.1), every one of the 12 repositories it created answering 404 afterwards; its third subtest skips without `GH_TEST_COLLABORATOR`, as written (`m3-repos.md`, "Terraform") | Done |
| BF-35 | Pass on `graft-m2b`, 100 of 100 through `gh ssh-key list` and `GET /user/keys` with no `Audit` consumer (`m3-rows.md`, "BF-35") | Done |
| BF-36 | Pass on `graft-m2b`, 10 of 10: a real OIDC code flow with PKCE against a test IdP, a Password sign-in `WrongMethod` after the switch, a non-admin's write 403 (`m3-rows.md`, "BF-36") | Done |
| BF-27 | Pass in memory at the row's count, 1,000 of 1,000 refused (`m3-wp4.md`); hosted only through BM-3's 100 of 100 `ghs_` refused after revoke, not with 1,000 queued | Done, on that evidence |
| BF-32 | Pass hosted, 12 of 12 twice with `@octokit/app` 16.1.4 (`m3-hosted.md`); the no-secret dump of every `Identity` chain ran in memory | Done |
| BF-28, BF-29 | Pass hosted on `graft-m2b` (M3-WP5 status) | Done |
| BM-3 | Pass, journey 4 100 rounds on edge `8a824825`, with the learner half counting B0-18's 6 of 6 as a judgement (M3-WP6, "BM-3 status") | Done |

`m3-rows` also took 16 of 16 planted bugs (`tests/m3/rows/plant.sh`) and formats Identity 0x14.2 (`KeyText`, event tag 6; `AddKey`, op tag 7) and repository 32.11 (`CancelInvitation`), and fixed issue commands deciding at `Timestamp(0)`, which gave every hosted repository invitation an `expires` a week after the epoch (`db1c6be`). BF-30 took repository 32.10 (`RepoCmd::Transfer`) and Authz 0x12.4 (`AuditCursor`, event tag 16, op tag 29), and fixed `/repositories/{id}` answering 404 in the window where the old owner holds only an `Alias` (35 of 2,715 samples in run a) and a `/move` step that lost leadership not being retried (`b18894e`). Design records: `decisions/DECISIONS.md`, "BF-30, M3's last rows, and AM1-WP3's pure half, 2026-11-08"; F57 for the plaintext OIDC client secret.

`m3-repos` built `rest:repos` creation for BF-34 (`workers/edge/src/repos.rs`; `crates/api` and `crates/git`): `POST /orgs/{org}/repos`, `POST /user/repos`, GraphQL `createRepository`, and `GET`, `PATCH`, and `DELETE /repos/{o}/{r}`, in repository format 32.13 (`IssueOp::InitRepo` and `DeleteRepo`; `RepoEvent::Created`, tag 33, and `Deleted`, tag 34; a deleted chain refuses every push and issue command). `gh repo create` passed 20 of 20, the REST smoke 23 of 23, and 14 hosted replies were valid against GHES 3.22 with 0 failed; 14 of 14 planted bugs were caught (`tests/m3/repos/plant.sh`). One bug was fixed (`cb98c9d`): an `auto_init` create's README push shared the create's request id, so the leader answered the following `InitRepo` from the push's stored row and the settings commit never ran. Nothing finished a create or delete that an edge crash cut short until F59 (`tests/m3/results/f59.md`, merged at `c31a3d6`; in memory only): a create commits `CreateRepoWith` (Authz 0x12.11) and a delete commits the name's `Deleted` first, and `Audit` finishes both through `Lifecycle` (repository 32.17) or aborts a create with no `Created` after `CREATE_ABORT_AFTER` (600 s [guess]); 58 of 58 tests and 21 of 21 planted bugs (`tests/m3/f59/plant.sh`). A hosted proof needs a site day with `GRAFTYARD_AUDIT=1` and 18.11 and 32.17 enabled, an edge with `--test-faults` and the same formats, a driver for the four crash points (`name`, `repo_owner`, `readme`, `delete_name`), and a pre-F59 deleted name seeded before `Audit`'s first scan (`decisions/DECISIONS.md`, "F59 closes"). The edge read gate's hosted run is under M3-WP3 ("Read gate"); its rerun after `932cc33` is on branch `m3-gate2`, on `graft-m2b`.

Formats: `m3-repos` took repository 32.13 and no Authz format, so 0x12.7 stays unused; AM1-WP4 took 32.14 and no Authz format, so 0x12.8 stays unused; `am1-hosted` took no format. Later: `fanout` took repository 32.15 (`RepoEvent::Fanout`, tag 39) and no Authz format; M4-WP1 took 32.16 and no Authz format. F59 took repository 32.17 (`Lifecycle`) and Authz 0x12.11 (`CreateRepoWith`, `AbortCreate`, `RepoCreate`), which releases those reservations. `lf4-rest-3` took repository 32.18 (`IssueOp::CreateCheck`, tag 40: a REST check-run create whose run id `decide` takes from the metadata tier's `IdBlock` cursor) and no Authz format. M4-WP5's `m4-web-3` took repository 32.22 (`ReviewEvent::SingleComment`, tag 3: a review comment's single-comment review that leaves an APPROVED or CHANGES_REQUESTED verdict as it is), after `b030-split`'s 32.20 and `m4-actions`' 32.21. M4-WP3's `m4-actions-3` took repository 32.23 (`ActionsSecret`, `ActionsVariable`, tags 44 and 45; 43 is `b030-split`'s `Split`). R24's `r24-app-workflow-job` took repository 32.24 (`RepoEvent::WorkflowJob`, tag 46: `RunOwner`'s `workflow_job` steps, which the `Webhooks` consumer delivers to Apps installed on the owner), and G9's `roll-up` took 32.25 (`RepoEvent::Compacted`, tag 47) and no Authz format. `app-hook-config` took Authz 0x12.13 (`AuthzOp::SetAppHook`, op tag 38: `PATCH /app/hook/config`'s patch of an App's webhook and wrapped secret) and no repository format. M4-WP3's `gh-token` took Authz 0x12.14 (`AuthzOp::Install` of the Actions App, no new op tag) and Actions 0x15.5 (`JobTokens`, `TokenIssued`, `CreateRunWith`, `AcquireToken`) and no repository format. Still reserved: Authz 0x12.9 (AM1) and Authz 0x12.10 (M4-WP1, unused by its pure half).

Also done on `graft-m1` (`tests/m3/results/m3-finish.md`):
- **R20: install creates every `Tokens` shard** (`5938d1b`, `eaff64b`). The edge's `POST /api/graftyard/v1/release/tokens?from&count`, under the signed genesis, creates up to 16 shards per request, and `graftyard install` creates all 3,072 in 192 ranges, 8 requests at once, reporting failed ranges for a rerun. Hosted on edge `9339fb5b`, the 3,072 shards were created in 21 minutes, with one shard's registry claim timing out and its range retried (14 s); a second sweep found every group. Over 24 device flows the device-code request fell from 13.0 s to 0.83 s median (max 1.3 s), the token exchange from about 15 s to 1.9 s, and 24 of 24 tokens were issued. The 13 s was not elections: each new member pings every peer `RTT_SAMPLES` (20) times on join, 9.9 s of joins, then 1.2 s to elect [measured].
- **Hosted `mint-key rotate`** (`74550b6`, `dc81e96`). Kid 2's wrap key went into the Secrets Store at 19:35:36Z, and 10 of 10 mints after it signed with kid 2. Kid 1 retired at 22:34:26Z (`tokens:0` index 35). A token whose header names kid 1 now fails `Retired`, and over git 5 of 5 such pushes were refused while kid 2 pushed 5 of 5. Gap: no genuine kid-1-signed token was left to try (the control had expired, and the kid 1 signing key is not kept), so the probe was a kid 2 token with its header rewritten to kid 1; `verify` checks `retired` before the signature, so a real one should fail the same way [inference].

## AM1: base agent changes, leases, and tasks (built with M2 and M3)

Built (`design/AGENT-LAYER.md`, "Milestones"): base agent changes, `AgentApp`, `AgentMeta`, `AgentTask`, `AgentGate`, `PathSet`, the trust view, leases, the activity feed, external tasks, and the task read endpoints.

### AM1-WP1. Base agent load changes

**Build.** Admission inside group commit: `AdmitClass` lanes, `AGENT_SHARE`, `AGENT_FLOOR`, `DEMOTE_PER_MIN`, `AGENT_WAIT`, `AGENT_QUEUE_MAX`, and the git `ng` and REST 429 refusal shapes. `INLINE_MAX_HOT`, `AGENT_INLINE_ENTRY`, and `HOT_PUSHES`. `LOG_MAX_BYTES` archiving to `R2Key::Archive` in two buckets. `NetworkLoad`, `fanout`, `FANOUT_MAX`, and the per-owner share of `HOT_POOL`. `TestMerge` and `TEST_MERGE_BUDGET`, with `mergeable` null past the budget. `R2Key::InlineIdx`, and hot clone-cache entries in both warm sites' buckets (`design/DESIGN.md`, Shape, "Agent load and agent safety", "Packs", "Git cache shards (derived)"; journey 9).

**Depends on.** M1, M2-WP3.

**Acceptance.** No gate row of its own. It carries LM-1's rerun of B0-29 to B0-35 on real code.

**Built so far (2026-11-06), the pure half** (`tests/am1/results/am1-wp1.md`). `crates/raft/src/admit.rs` and the leader: `AdmitClass` lanes with per-actor fair queuing, the `AGENT_SHARE` bucket and `AGENT_FLOOR`, `DEMOTE_PER_MIN` counted from the group fold's last minute plus the leader's unresolved and refused writes, `AGENT_WAIT`, `AGENT_QUEUE_MAX` refused by the leader before staging, `INLINE_MAX_HOT` with `hot` read from the log over a 5 s window, `AGENT_INLINE_ENTRY`, and the outage refusal; `Member::commit` is unchanged (Human, no actor) and `commit_as` is the new entry point. The git `ng` and REST 429 refusal shapes, carried as `PushOutcome::Full`. In `crates/git`: `NetworkLoad`, `Fanout`, and a pure `fanout` from the larger of two warm sites' loads; `pool_grants`, holding each owner to a share and keeping what a network holds first; `clone_cache_buckets`; `MergeBudget` for `TEST_MERGE_BUDGET`, with a shed head's `mergeable` null. In `crates/fold`: `LOG_MAX_BYTES`, `LOG_KEEP`, and `archive_due`. In memory, B0-29 to B0-35 pass: B0-29, B0-31, B0-32, and B0-33 on three real members with admission over a modeled network (one-way 40 to 110 ms plus 0.3 ms per KB, storage confirming at once), B0-30 on M3-WP5's install bed with a scaled cap, and B0-34 and B0-35 on the pure functions; 23 of 23 planted bugs caught; the 2,000-seed sweep and Porcupine Ok (156,742 and 2,226,521 operations). The latencies measure admission's queueing and entry sizes on that model, not Cloudflare, so LM-1's hosted rerun decides the gates. The design now says what the code needed (DESIGN, Shape, agent block; `decisions/DECISIONS.md`, "AM1-WP1's admission and fan-out shapes as the code needs them"). Not yet: host wiring (the edge does not call `commit_as`, nothing classes commands until AM1-WP2, and `Admit` does not cross the member route), the leader's archive alarm that calls `archive_due`, the network leader's per-minute `fanout` commit and the shard's `NetworkLoad` report, clone-cache writes to the second warm bucket, and the test-merge budget beyond k = 1 per isolate in the site consumer.

**Pool grants (2026-11-13)** (`tests/am1/results/pool-grants.md`; branch `pool-grants`). The per-owner share of `HOT_POOL` is now on every path: each site's first pool shard keeps a `PoolLedger`, the network leader asks both warm sites' ledgers before it commits a `Fanout` step and commits 1 plus the fewer granted, leases lapse past `LOAD_STALE`, an unreachable ledger keeps k, and `shards_for` serves each network its own granted pool slots (`decisions/DECISIONS.md`, "The per-owner pool budget is a lease ledger in each site's first pool shard"). B0-34's hostile-owner lines now measure this code; LM-1 scripts the full row.
**`fanout` (2026-11-10)** (`tests/am1/results/fanout.md`; branch `fanout`, merged at `562e26b`). Built: concurrent validates of one network inside a shard (`containers/git/server/main.go`, `handleValidate` takes the network's lock shared; only ingest, ref writes, repack, init, and evict take it alone), shared catch-ups (`CatchUps::fresh`), the container's `GET /load`, `LoadReports` and `combine`, rendezvous `push_validator`, pool shards `git#<site>#pool:<n>`, the first shard's once-a-minute report to the leader's `/fanout`, and `RepoEvent::Fanout { k }` (repository format 32.15, wire tag 39) committed as the leader's own entry. Container bench: one network validates 39/s on 1 vCPU and 58/s on 4 at 8 in flight, against about 16/s before; hosted it was about 2.7/s before. Hosted on `graft-m2b`, k forced to 4 sent validates to pool shards at p50 456 ms. Not yet shown hosted: k stepping in band, because a follower answered `/fanout` with an error (27 of 29 reports in B0-34); `105c228` fixes that and waits for a site day. 22 of 22 planted bugs caught; `crates/raft` unchanged (`decisions/DECISIONS.md`, "A25 closes").

**Follow-up: git cache shards on a hot group with a multi-GB log** (operator decision, 2026-11-10, the substitution part withdrawn on 2026-11-11; `decisions/DECISIONS.md`, "B0-30 fails at `CHILD_GROUP_AT`, and the substitution is withdrawn"; `decisions/OPEN-QUESTIONS.md`, G7; thread "hot-shards"). Make the git cache shard's rehydrate and catch-up work on a hot group whose log holds gigabytes of inline packs above the last `Promoted`, so that pushes, `/promote`, and archiving keep up through leader changes and shard resets. Evidence: `tests/am1/results/b0-30.md` on branch `b0-30`, "Blocked on", items 5 to 11: every whole-tier `{"rows": true}` read on the 4 GB tier cost about 302 s of CPU and reset the leader (5); the first archive's id-block check read every log row above it (6); the archive alarm drained about 0.2 MB/s, through a warm shard only (7); a shard far behind read its whole gap into memory (8); a shard with no cursor read the whole tail above `Promoted`, about 2.5 GB (9); a rehydrate in pieces outlasted the push's 180 s deadline and the alarm's `RUN_FOR` (10); a cut-off rehydrate restarted from nothing (11); and, with pushes off, archiving stopped at 15:29Z because every archive run's ingest fails `422 git index-pack: … did not receive expected object` on a pack pushed before pushes stopped, so the log passed `LOG_MAX_BYTES` (2.277 GB) and member SQLite grows 0.193 GB a simulated day toward `CHILD_GROUP_AT` (12; one recovery attempt running). Four site fixes are on `b0-30`, not yet on main: the id-block check reads only rows under 4 KB (`c3a2aa9`), catch-up in pieces of `CATCH_UP_ENTRIES` (32) saving the cursor after each (`117f899`), rehydrate in pieces with packs ingested in `INGEST_BATCH_BYTES` batches (`f9a05d3`), and a cut-off rehydrate resuming on the same disk (`8753c0a`). Item 5's agent-push half is `ccd10b1` on main. Gate: journey 9's pushes on a group seeded with a 4 GB metadata tier, through leader changes and git cache shard resets, with no push timeouts and archiving holding the log at `LOG_MAX_BYTES`. It is not a coverage-table row; it is the push half B0-30's rerun with pushes on needs (`b030-split`).

**Follow-up: the test-merge budget and B0-35's instruments** (`tests/am1/results/lm1-memory.md`, section 8; `decisions/OPEN-QUESTIONS.md`, A27). `workers/site/src/prsync.rs` admits each head-push merge through `MergeBudget::admit` but never calls `MergeBudget::charge`, and keeps the budget in an isolate `thread_local`, so `TEST_MERGE_BUDGET` is not enforced and B0-35's test-merge vCPU cannot be measured. Build: charge each computed merge's measured time, keep it per network in shard memory, as `design/DESIGN.md`, Shape, `MergeBudget` states, report spent and shed merges and a count of computes from list fields in a new `/stats` field on the consumer, then a site day. Its gate is B0-35's own line. Built on branch `merge-budget` (2026-11-11, `tests/am1/results/merge-budget.md`; `decisions/DECISIONS.md`, "A27 closes"). The shard that runs a network's merges holds its `MergeBudget` and charges each merge's measured time. Past the budget it answers `shed`, so `mergeable` is null. It reports `spentMs`, `computed`, `shed`, and list-field asks and computes with each merge and in `/status`, and the leader's `/stats` keeps them as `testMerges`. `tests/am1/rest/a1.mjs` B0-35 pushes the PRs' heads and writes `testMerges` per site. 18 of 18 planted bugs were caught. On `graft-m3c`, after a site day, 50 PRs with head pushes at 10/s spent 0.1845 vCPU of a 0.25 budget, with 190 merges computed, 694 shed, and 0 computed for list fields. B0-35 itself still waits on the 500-agent bar and on Actions for its dispatch half. Only head pushes ask for a merge: dispatch and single-PR GET computes are not wired. Branch `merge-budget-2` (2026-11-12, `tests/am1/results/merge-budget-2.md`; `decisions/DECISIONS.md`, "A28 closes", "A30 closes", "A29 closes") runs a network's merges one at a time, each admitted after the one before was charged its CPU (`cpuMs` from the container), so the spend passes the budget by at most one merge. It sizes the shard by the site's `SHARD_INSTANCE`, computes single-PR GraphQL GETs through the shard's `/demand`, gives M4's `ActionsTrigger` `crate::prsync::demand` to call, and counts list fields on the shard. Branch `merge-budget-3` (2026-11-12, `tests/am1/results/merge-budget-3.md`; `decisions/DECISIONS.md`, "`merge-budget-3`") has REST `GET pulls/{n}` ask for its merge as GraphQL does and render it once computed, stores the shard's counts so they outlive the object's reset, adds `inFlightMax` to show concurrent merges wait their turn, and runs `PrSync`'s first attempt in a new invocation through the site's own service binding, which ends "Subrequest depth limit exceeded" on a cold shard's catch-up. On `graft-m3c`, a `standard-4` shard (1 vCPU budget) used 0.111 vCPU for 50 PRs pushed every 30 s, with 0 shed, and with first attempts cut to 3 s so re-sends overlapped, 69 merges in line at once left the bucket at -24.8 ms against a 37.8 ms merge; 0 depth errors on either cold start. Branch `merge-budget-4` (`tests/am1/results/merge-budget-4.md`) measures each merge's wait (p99 5.8 s with 44 in line, under the 30 s coalescing window), keeps a late older-base merge from replacing the head's merge at the base tip (32 in merge-budget-3's run 1, 0 after), and reads the `standard-4` shard's 4 vCPU from the Containers API.

**Cost risk: idle shard wake-ups** (`tests/am1/results/lm1-memory.md`, section 4; `decisions/OPEN-QUESTIONS.md`, K8). Torn-down network shards on `graft-m2b` ran again with no load, about $3/h for the set (`tests/am1/results/lm1-rerun2.md`). Three causes: `PrSync` redeliveries that retry a failing merge without end (the queue's `max_retries: 10, retry_delay: 2`, plus the dispatcher's re-send after `REDELIVER_AFTER_MS` for as long as it leads; 189 `shard/merge` calls in 30 idle minutes on site a); `warm()` posting `shard/catchup` on every new leadership; and the `*/15` cron's fold audit restoring old members, which starts the second [inference]. Branch `idle-shards` (thread T-01a1c8aa-e61f-7739-bd38-27cb59655101) is fixing them, along the results file's suggested fixes: a bounded number of re-sends for a failing batch, `warm()` only for a leadership with `PrSync` work past its cursor, and a teardown that covers every `net:` instance.

### AM1-WP2. Agent actors and refusals in `decide`

**Build.** `ActorKind`, `AgentActor`, `AgentApp`, `AgentKind`, `AgentMeta`, `AgentPolicy`, `CiMode`, `PROTECTED_PATHS`, `ProtectedHits`, `ChangedPaths`, `PATHS_MAX`, `Provenance`, `Source`, `TrailerKey`, `SESSION_REFS`, `AgentGate` in `Merge::decide`, `AuthzEvent::AgentApp`, and `OrgPolicy.agents_enabled` and `agents_workspace` (`design/DESIGN.md`, Shape, "Agent load and agent safety"; `design/AGENT-LAYER.md`, Shape, "Agent actors in the layer"). `AgentApp` rows on `Authz(Genesis.account)`, which every App command lists in `deps()`, and the settings endpoints for `AgentApp`, `AgentPolicy`, `agents_enabled`, and the workspace mode (`design/AGENT-LAYER.md`, "API reference"). `Push` gains `provenance`, `protected`, and `changed` (`design/SHARED.md`, "Refs and pushes").

**Depends on.** AM1-WP1, M3-WP3, M3-WP4.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-2 | Unflag an `AgentApp` (`DELETE /agents/apps/{id}`), then push under its old `confine` prefix | The next `decide` after the acknowledgment no longer classes it `ActorKind::Agent` | A decision after the acknowledgment uses the old row |

It also carries LM-1's rerun of B0-36 and B0-37 on real code.

**Built so far (2026-11-07), the pure half** (`tests/am1/results/am1-wp2.md`; commits from `b76ef28` to `dff8db1`, merged at `53231a6`). In `crates/identity`: `AgentApp`, `AgentKind`, `WorkspaceMode`, `RefPattern`, and `PathGlob`; `AuthzEvent::AgentApp { app, row: Option<AgentApp> }`, whose `None` unflags, and `AuthzEvent::OrgAgents { enabled, workspace }`, read through `OrgPolicy` but never written in a `Policy` entry, so M3-WP3's bytes hold; ops `PutAgentApp`, `DeleteAgentApp`, and `SetOrgAgents`, written only by the install org's admin on `Authz(Genesis.account)`. In `crates/git/src/agent.rs`: the agent types, `validate`'s trailer parser and `provenance_of`, `changed_of`, and `protected_of` ported from M0-WP10, linking, classing (`actor_kind` in `decide`, `admit_class` for admission), and the refusals in the design's order. `Push` gains `provenance`, `protected`, and `changed`, carried only in push format 32.8 when one is non-empty; repository events `Provenance`, `AgentMeta`, `AgentGate`, and `AgentPolicy`; `AgentGate` checked by `Merge::decide` and the queue's `Enqueue`; and agent refusals in issue and merge `decide`, rendered 403 or `FORBIDDEN` with the rule text. The edge classes commands and passes a non-`Human` class in `AdmitWire`, and the leader classes again from its own fresh install read; the trust view's `is_agent` comes from `AgentMeta` and `AgentApp` rows; the settings endpoints are under `/api/graftyard/v1`; the site reads 32.8 and 0x12.3. Formats and tags: repository 32.8, Authz 0x12.3, `AuthzEvent` 14 `AgentApp` and 15 `OrgAgents`, `AuthzOp` 26 to 28, `RepoEvent` 27 to 30, `RejectReason` 7 `Agent`. In memory, on three real members of the install's `Authz` chain and three of one repository (`crates/git/tests/agent_gates.rs`):
- **LF-2: pass**, 25 of 25 first decisions after the unflag's acknowledgment classed the App `Bot` and accepted the push; a stale install read keeps the old class, as the control shows.
- **B0-36: pass for what `decide` sees before AM1-WP3 and WP4**, 100 of 100 refused per action across 11 actions, for both a confined vendor App and a Graftyard-run App, with protected paths, ruleset bypass, the label allowlist, and draft-only covered; an unflagged App, an unconfined App, and user tokens are never refused by an agent rule.
- **B0-37: pass for the parser, linking, and `decide`**: 10,000 hostile pushes, 0 parse failures; 17,264 forged `Graftyard-Session` trailers, 0 linked; 1,675 `Session` sources forged on the wire, 0 stored; 1,000 of 1,000 `refs/graftyard/` updates refused.
- **`AgentGate`** counts human writers on the current head, a head move voids it, and `extra_human_approvals` adds one; a default PR merges with no gate, and `Co-authored-by` alone never classes.
- **Byte identity:** `agent_bytes.golden`, pinned on main before any agent code, passes, and 120 of 120 API replies are identical to main's.
- 19 of 19 planted bugs caught (`tests/am1/wp2/plant.py`). Three bugs the gates found, fixed at the cause: `Push::tiers` was always Core, so followers refused a provenance-carrying entry; `session_from_url` panicked on a value cut inside a multi-byte character; and an agent refusal came after an App permission check, so it lost its rule text.

Waiting for AM1-WP3 and WP4 and the hosted rerun: B0-36's task, dispatch, and lease rows, and B0-37's fetch half (session refs over v0, v1, v2, and `--mirror`). Until tasks exist a Graftyard-run App's push is always refused `NoLiveTask`, every agent PR's gate needs one extra approval, and `changed` is carried and stored but nothing matches it.

### AM1-WP3. External tasks and the task read endpoints

**Build.** `AgentTask`, `TaskKind`, `TaskApiState`, `SessionId`, `TASKS_OPEN_PER_REQUESTER`, `TASKS_OPEN_PER_REPO`, `TASK_IDLE`, and the `TaskCreated` and `TaskUpdated` events (`design/DESIGN.md`, Shape, "Agent load and agent safety"; `design/AGENT-LAYER.md`, Shape, "Agent-layer entries"). The task read endpoints and `External` task creation under `/api/graftyard/v1/repos/{owner}/{repo}/tasks`, with the bodies and errors in `design/AGENT-LAYER.md`, "API reference". The `graftyard_task` token request field and the `task` claim, valid on a token of the task's App or `graftyard-workspace` for the task's repository or its workspace fork (`design/SHARED.md`, "Identity and tokens"). An uninstalled App's tasks read as cancelled through the same check that releases its leases, with no sweep. Graftyard-run creation is AM3-WP3.

**Depends on.** AM1-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-1 | Uninstall an App with 100 open tasks across 10 repositories | Every task reads `cancelled` at the next read with no sweep command, and its next command records it | Any task reads live |

It also carries LM-1's "base journeys pass unchanged".

**Built so far (2026-11-08), the pure half** (`tests/am1/results/am1-wp3.md`; branch `am1-wp3`, merged at `b767933`). In `crates/git/src/task.rs`: `TaskKind`, `TaskApiState`, `TaskActor`, and `AgentTask`; the derived states `effective` and `installed`; the caps; `run_refs`, `live_run_task`, `own_sessions`, and `token_task`; and `decide` for external `CreateTask`, `UpdateTask`, and `CancelTask`, where a task's next command records a derived final state and answers 409 with it. `Push::decide` binds a claimed token to its live `GraftyardRun` task under the prefix `refs/heads/graftyard/<uuid>/*/**`. `SessionId` moved to `graftyard_identity::agents` and carries its string form in JSON; `InstallationClaims.task` and `Grant.task` carry the claim. The read and create routes are in `crates/api/src/tasks.rs` under `/api/graftyard/v1/repos/{o}/{r}/tasks`, and the edge serves them only once 32.12 is enabled. Format: repository 32.12, `RepoEvent` 31 `TaskCreated` and 32 `TaskUpdated`. In memory, on real members:
- **LF-1: pass.** After an uninstall, 100 of 100 tasks across 10 repositories read `cancelled` at the next read with 0 events written by reads, and 100 of 100 next commands recorded it and answered 409; the control App's tasks stayed `queued`.
- **Linking:** 100 of 100 claimed pushes accepted with their trailers linked as `Source::Session`, and every bad case (no claim, outside the prefix, another task's prefix or the bare id, an unknown task) refused 100 of 100.
- **Caps and idle:** 100 open per requester and 2,000 per repository, then 422; a task idle 24 h reads `timed_out` with no sweep.
- **Byte identity:** 120 of 120 API replies identical to main's, and `agent_bytes.golden` unchanged.
- 24 of 24 planted bugs caught (`tests/am1/wp3/plant.py`). Two bugs the gates found, fixed at the cause: `SessionId::Run(u128)` serialized as a 128-bit integer that `serde_json` cannot carry, and an uninstall then reinstall revived old tasks until `installed` needed `task.created >= not_before`.

The design now says what the code needed (DESIGN and AGENT-LAYER, Shape, agent block; `decisions/DECISIONS.md`, "AM1-WP3's task rows as the code needs them"). The hosted half needs:
1. A site day onto a release that reads 32.12, then `FormatsEnable` for 32.12; until then the task API and `graftyard_task` answer 503 and a claimed push is refused `Unsupported`.
2. `validate` in the git cache returning `Provenance`, and the edge's `receive_pack` linking it with `agent::link`, `task::app_sessions`, and the token's claim before the commit.
3. A `graftyard-workspace` App with a `GraftyardRun` row, and AM3-WP3's controller creating Graftyard-run tasks; until then only external tasks exist hosted.
4. The hosted LF-1 rerun: 10 real repositories, 100 external tasks from a test `AgentApp`, the uninstall through the hosted `DELETE`, then reads and next commands through the edge.
5. A check that a claimed token minted by `graft-ops` verifies on the edge with its `task`, and that a 32.12 push under it reaches `decide`.
6. B0-37's fetch half: session refs advertised only to a v2 `ref-prefix`.
7. Leases (AM1-WP4): a cancel or finish releasing the task's leases in the same command, `leases` in the single GET, and LF-1's lease half.

### AM1-WP4. Leases, `PathSet`, and the activity feed

**Build.** `Lease`, `LeaseHolder`, `ActorId`, `LeaseScope`, `PathGlob` and `RefPattern` with their glob dialect and overlap rule, `LeaseMode`, `LeaseEnforce`, `LEASE_TTL`, `EXCLUSIVE_MAX`, `LEASES_PER_HOLDER`, `LEASES_PER_APP`, `PLAN_LEASES_PER_REPO`, `LEASES_PER_REPO`, and the lease events (`design/AGENT-LAYER.md`, Shape, first block and "Leases"). `graftyard-plan` blocks parsed by the grammar in `design/AGENT-LAYER.md`, "API reference". `ExtendLeases` on `LogEntry.extend` (`design/DESIGN.md`, Shape, "Chains"). `PathSet` (Shape, "Overlaps and prediction"). The activity feed with `ActivityEvent`, `PrAction`, `LeaseChange`, `FeedCursor` and its encoding, `wait` in whole seconds clamped at 25, `paths` as a repeated parameter, and the `FEED_*` constants, `FEED_HELD_MAX` included. Learners serve it, followers serve it only while no learner answers, and only a following member holds polls (Shape, "Activity feed"). Each member's peak held count feeds `place_learners` (M3-WP5).

**Depends on.** AM1-WP3.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-3 | Feed cursor from another group, `wait=40`, 33 `paths` | 410 with `oldest_cursor`, `wait` clamped to 25, 422 for the 33 paths | Any other status, or `wait` above 25 |
| LF-5 | `graftyard-plan` blocks: a valid one, a repeated key, a bad glob, a second block, and a block over 4 KiB | One lease from the valid block. None from the others, and the body unchanged | A lease from an invalid block, no lease from the valid one, or a changed body |

It also carries LM-1's rerun of L0-12, L0-13, and L0-16, and reruns LF-12 and LF-13 on real code [inference].

**Built so far (2026-11-08), the pure half** (`tests/am1/results/am1-wp4.md`; branch `am1-wp4`, merged at `9fb5032`). In `crates/identity/src/glob.rs`: `glob_overlap`, `literal_prefix`, and `PathGlob::overlaps`. In `crates/git/src/lease.rs`: `LeaseId` (a ULID from the entry's `at` and `lease_seq`), `ActorId`, `LeaseHolder`, `LeaseScope`, `LeaseMode`, `Lease`, the caps and TTL range, the core lease tables with Exclusive globs indexed by literal prefix, `LeaseView::holds`, `decide` for `AcquireLease`, `RenewLease`, and `ReleaseLease`, `graftyard-plan` parsing with the editor's plan lease replaced or released in the body write's own command, and `marks` for the edge. `Push::decide` matches `changed` against live Exclusive leases under `AgentsExclusive` for `ActorKind::Agent` only, per ref in 32.14; a 32.14 push writes `PathSet`; a task's cancel, finish, or derived final record releases its leases in the same command. `crates/git/src/feed.rs`: the `FEED_*` constants, `ActivityEvent` without `Verdict`, the 17-byte `FeedCursor`, `parse_query`, `start`, `events`, and the serving model (`Serving`, `route`). `crates/api/src/leases.rs`: the lease and activity routes under `/api/graftyard/v1`. In `crates/raft`: `stalled_since` marked at a `suspect`, a campaign start, or the leader's unacknowledged entry and stored with the member's next state write, and the leader applying an entry's `extend` before its commands decide (`Scratch::extend`); the raft suite passes, the 2,000-seed sweep passed in 348 s with 0 step-downs flagged, and Porcupine was Ok on 156,742 and 2,226,521 operations. Format: repository 32.14, `RepoEvent` 35 `LeaseAcquire`, 36 `LeaseRenew`, 37 `LeaseRelease`, and 38 `PathSet`; the first draft took 33 to 36, which collided with `m3-repos`' 33 and 34, and moved before the merge (`0477a8b`). In memory, on real members:
- **LF-3: pass.** A foreign cursor 410 with `oldest_cursor`, `wait` clamped to 25, 422 for 33 `paths`, 32 accepted.
- **LF-5: pass.** One Shared lease from the valid block, none from the four invalid ones, 5 of 5 bodies stored byte for byte; readying the draft ends the plan lease with no command.
- **L0-13: pass.** 100 of 100 refused for each of 9 bad acquires with 0 written; 0 plan leases from non-writers and 500 from writers; a demoted holder's lease refuses nothing; uninstall takes 7 leases to 0 and cancels 5 of 5 tasks; Exclusive reads Shared after 4 h.
- **L0-12, pure parts: pass.** Under `AgentsExclusive`, 100 of 100 pushes and REST writes refused for both agent kinds and 0 for the other three; a late Exclusive acquire succeeds; a 30-minute stall moves 3 of 3 leases by exactly the recorded `by`.
- **L0-16: pass.** 5 of 5 lease decisions identical on the core tier alone, with 0 metadata reads.
- **LF-12 and LF-13, pure models: pass.** 5,000 polls held at most 1,000 per member, 0 on the leader, 0 on followers once learners answer; a lagging follower answers 1,000 of 1,000 held polls at once and loses no event.
- **Byte identity:** 120 of 120 replies identical to main's, and `agent_bytes.golden` untouched.
- 40 of 40 planted bugs caught (`tests/am1/wp4/plant.py`). The plant runner had two bugs, a doubled `--lib` that counted a cargo usage error as a catch and a second argument bug that left mutant 26 uncompiled; both are fixed, and every count is from named failing tests. Two code bugs the gates found, fixed at the cause: the first command after a stall decided before the extension, and a lease command waited for the metadata tier.

The design now says what the code needed (DESIGN, Shape, "Chains" and the agent block; AGENT-LAYER, Shape and "Leases"; `decisions/DECISIONS.md`, "AM1-WP4's pure half: leases, `PathSet`, and the feed as the code needs them, 2026-11-08"). The hosted half needs:
1. A site day onto a release that reads 32.14, then `FormatsEnable` for 32.14; until then the lease endpoints answer 503, no command is marked 32.14, and no push writes `PathSet` or is refused `Leased`.
2. The edge sending `changed` and `validate`'s other outputs on agent pushes and REST ref writes, and setting `Push.leases` once 32.14 is enabled.
3. Feed serving on followers and learners from their applied log, held polls with `Serving`, the edge's `route`, `FEED_POLLS` per token and IP, and each member's peak, refusals, and read rate reported to `place_learners`; until then the edge answers 503 after checking the query.
4. `stalled_since` wired into the site's `suspect` path and the leader's tick, stored with the member's state row, and the hosted 30-minute stall with member calls blackholed and real clock offsets (L0-12's `DRIFT` line).
5. The hosted runs: L0-12 on the A1 replay with leader kills and 500 long-pollers, LF-12's 5,000 polls and commit p99, LF-13 with a real gap and `InstallSnapshot`, and L0-16 with the metadata tier unloaded.
6. Plan blocks on the PR body paths: no REST `PATCH /pulls/{n}` or GraphQL `updatePullRequest` builds a body change yet; a GraphQL `createPullRequest` draft already marks.
7. MCP's `coordination.*` tools (AM2) and the controller's Shared lease at task start (AM3).

### AM1-WP5. The trust view

**Build.** `AGENT_MEDIA_TYPE`, `TrustView`, `Hidden`, `AuthorTrust`, `HIDDEN_CHARS`, and the resources that get `graftyard_trust`, rendered by the edge beside `body` and `body_html`, which stay byte-identical to GHES (`design/AGENT-LAYER.md`, Shape, "Trust view"; journey A4).

**Depends on.** M2-WP2, and AM1-WP2 for `AuthorTrust.is_agent`, which AM1-WP2's pure half now sets from `AgentMeta` and `AgentApp` rows.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-4 | Trust view: one payload per `HIDDEN_CHARS` range in each `graftyard_trust` resource | 100% absent from `visible` and counted | Any present |

It also carries LM-1's byte diff (the rerun of L0-11).

**Built so far (2026-11-06), the pure half** (`tests/am1/results/am1-wp5.md`). `crates/api/src/trust.rs`: `AGENT_MEDIA_TYPE`, `TrustView`, `Hidden`, `AuthorTrust`, `HIDDEN_CHARS`, the trust resources as REST path patterns from the pinned GHES 3.22 description, `visible` ported from the M0-WP13 prototype (all 200 L0-09 payloads give the prototype's output exactly), and `layer`, which returns a reply unchanged unless one `Accept` range equals the media type; the edge's `rest_repo` calls it. LF-4's pure half passes: one payload per range for each of 16 ranges in each of 8 resource kinds, 128 of 128 absent from `visible` and counted, raw text unchanged. LM-1's byte diff, pure half: 93 replies of the gh, git-lfs, and release suites, framed as the edge frames them, compared with and without the layer for 13 `Accept` values, 1,209 comparisons with 0 differences, and all 93 reply files identical to `main`'s; 18 of 18 planted bugs caught. Today the hosted edge serves only releases among the trust resources, and the view is unchanged in GraphQL by design. `is_agent` and `first_contribution` are always false (no `AgentApp` rows, no first-contribution row). Not yet: the hosted LF-4 run with real payloads over the edge, and the MCP and controller callers of `TrustView::of`.

### AM1-WP6. AM1 gate

**Build.** Nothing new.

**Depends on.** AM1-WP1, AM1-WP2, AM1-WP3, AM1-WP4, AM1-WP5, M3-WP6.

**Acceptance.**

| Row | Gate (Pass) |
| --- | --- |
| LM-1 | Base journeys pass unchanged, the base's agent rows rerun on real code, the base's 30-day hot-group storage row passes on a group seeded with the 4 GB tier, the leases and feed, lease limits, gate and read permissions, and core-tier rows pass, and the byte diff passes. Fleets of A1's size on one repository are enabled only after metadata child groups (base Milestone 7) |

The base's agent rows are B0-29 to B0-37 and the stateless revocation in BM-3 (`design/AGENT-LAYER.md`, "Milestone 0 rows"). The 30-day hot-group storage row is B0-30. The layer rows named are L0-12, L0-13, L0-15, L0-16, and L0-11. B0-30 passes as written, 30 days of journey 9 with pushes on: the operator's 2026-11-10 substitution (its metadata half only, its push half replaced) is withdrawn by the operator's direction of 2026-11-11 that every row passes as written (`decisions/DECISIONS.md`, "B0-30 fails at `CHILD_GROUP_AT`, and the substitution is withdrawn"). The operator's decision of 2026-11-10 that moves the Graftyard-run clauses (`decisions/DECISIONS.md`, "The Graftyard-run clauses move from LM-1 to AM3-WP3") still stands: L0-12's lease refusal for the Graftyard-run kind, B0-36's Graftyard-run part (its lease refusal and its no-live-task and cancelled-task fork pushes), and B0-37's fetch half (a session ref appearing to a matching `ref-prefix`) are gated by AM3-WP3 instead. Fleets of A1's size on one repository stay off until M7-WP4 passes.

**Shared isolates, before a verdict trusts a site's tail (2026-11-20).** A hosted AM1 row's verdict reads `python3 tests/fanout/isolates.py tests/fanout/raw/tsave-<tag>.jsonl` (`b029.sh` and `b033.sh` append it to the status file as "shared isolates") before it trusts a site's or a leader's tail. A member of the group that shared an isolate with another (a learner beside its own voter) shares that isolate's CPU and memory, and on builds before `feed-per-member` its activity feed too: on B0-29 m3fp3 learner 195 and site 0's voter shared one, and site a logged 220,515 "closure invoked recursively or after being dropped" (`tests/m0/results/looplag-clock-m3f.md`; `decisions/DECISIONS.md`, "`feed-per-member`"). A verdict names any shared isolate and its members, and reads that site's segments as confounded.

**LM-1 status (2026-11-11): not closed; its 500-agent load rows fail on `graft-m2b` and are being rerun, and LF-13 passes on `graft-m3c`** (`tests/am1/results/am1-hosted.md`, branch `am1-hosted`, merged into main before `6042f3e`; `tests/am1/results/am1-runs.md`, branch `am1-runs`, merged as `aa9e097`). AM1's hosted half ran on `graft-m1` (sites a to d, `graft-m1-edge`, `graft-m1-ops`) through site days 2 to 7; `am1-fix` added day 7. No record format was taken, so 32.15 and 0x12.9 stay free; `crates/raft` was not changed.

| Day | Commit | Sites a / b / c / d | Edge | Ops | Notes |
| --- | --- | --- | --- | --- | --- |
| 2 | `40708cb` | `be79725f` / `aed638b3` / `232474cc` / `150bab9c` | `19e72fe6` | `1e9eb629` (the `task` claim) | New shard image `sha256:5db19b4e…`; `--roll-shards` stopped on a catch-up timeout and `tests/am1/hosted/roll.mjs` finished the roll |
| 3 | `a4dd646` | `c2f4c5e4` / `bde58f9a` / `6fe455bf` / `69beff51` | `98de7bf4` | | no roll |
| 4 | `f925d40` | `ed0bff49` / `8686876c` / `11ff63a5` / `f47c2827` | `b330c05c` | | no roll |
| 5 | `c836d14` | `b3d42554` / `7bed34dc` / `e57c7482` / `0b7bcdf4` | `d3a8981f` | | no roll; L0-16's two fixes |
| 6 | `04e4f22` | `6553b6dc` / `751b0701` / `98221437` / `002551dc` | `ce2e1ac3` | | no roll; main through `5d18dda` merged, with the read gate |
| 7 | `c7384ec` | `b5bfd297` / `a0ee9724` / `da589789` / `9199c4e4` | `382356ed` | unchanged | no roll; `am1-fix`'s two fixes (`tests/am1/results/am1-fix.md`) |

`am1-runs` ran the rest on `graft-m2b` (sites a to c) on 2026-11-09, on `0172f55`'s code: one site day (`612c5f4`, which differs only under `tests/`) with shard image `:am1runs` (`sha256:fb90615a…`, 70 shard instances rolled), then the edge with the agent formats. `graft-m2b-edge` is now `c4eb6915`, with 32.8, 32.12, 32.14, and 18.3 added to `FORMATS_ENABLED`, the same agent formats `graft-m1-edge` enables, and `--ops graft-m2b-ops`. `graft-m2b-ops` is new: Worker `c53e4459`, its `MintWrapKey` in the Secrets Store secret `graft-m2b-mint-wrap-key` (created from a 0600 key file, since deleted), and kid 1's `PutVerifyingKey` on `tokens:0` at index 14. `apps.mjs` registered `graftyard-workspace` (App 1441792), `graftyard-agent` (1507328), and `graftyard-mcp` (1572864) on `authz:1`, installed on m3org; each mints 201. The three leaked `graft-m2b` user tokens were revoked (204, then 401 over REST and git) and replaced. Hosted runs cost about $7.0 at overage rates, an account-wide upper bound, against the plan's $5.44. `dfa5410` (the feed ring's hole) and `89cf205` (GraphQL label routing) are on main but not deployed.

Built hosted: `validate`'s per-update detail from the git cache, with provenance and `changed` on agent pushes and REST ref writes (`POST`/`PATCH .../git/refs`, GHES's fast-forward rule) and `agent::link`; the `task` claim in tokens minted by `graft-ops`; the feed served by followers and learners, with each member's ring, held polls under FEED_POLLS and FEED_HELD_MAX, the edge's `route`, and `/learners {"feed": true}` placing learners from peaks and refusals; `stalledSince` in the member Durable Object; and GraphQL `updatePullRequest` for plan-block body edits. Fixes the runs found: a `RefCell` borrow that panicked `/feed`; an idle follower that read as lagging; a pending ring with no bound; lease commands that answered 503 while the metadata tier loaded; and L0-16's harness counting the alarm's replay as path reads. 25 of 25 planted bugs caught (`tests/am1/hosted/plant.py`).

| Row | Hosted result |
| --- | --- |
| Base journeys | Pass: j2 and j3 20 of 20 each; journey 4 PATs and `ghs_` 10 of 10 refused after revoke |
| L0-11 (byte diff) | Pass: day 2 0 of 2,048 replies differ; the day 5 and day 6 differences are only j3's new issues and the read gate's anonymous 404s |
| L0-12, refusals | Pass for all six kinds (`am1-runs`): the confined vendor App 100 of 100 pushes and 100 of 100 REST ref writes refused naming the lease; the Graftyard-run App (`graftyard-workspace`) 100 of 100 of each refused, but by `NoLiveTask` ("push needs a live task") before the lease check, because the edge cannot yet create a Graftyard-run task (AM3-WP3); 0 of 100 for a flagged App without `confine`, an unflagged App, a human PAT, and `Graftyard MCP` |
| L0-12, stall | **Pass after `am1-fix`** (day 7, lf1-5): a 30-minute stall with every member call failing, then a leader change; term 3's first entry carried `extend { by: 1,761,355 ms }`, all 20 leases moved by 1,761 or 1,762 s, exactly one entry carried `extend`, and clock spread was at most 65 ms. Before the fix (day 5) a new leader wrote no `extend` and leases moved 0 ms. The fix (`96dc541`): a follower marks the stall from the leader's late re-send of an uncommitted entry; the long sweep passed (335.75 s) and Porcupine was Ok on both histories. Uncovered: an old leader that never reaches the winner, which loses the stall as a lost marker does |
| L0-12, A1 replay | **Fail** (`am1-runs`, 500 tasks and 500 Exclusive leases, a leader kill every 60 s): 0 events reached the 500 pollers in 600 s, of 149,265 polls (74,432 with no status, 52,602 `gone`, 12,099 `notServing`, 6,277 `lagging`, 3,855 held); 57 of 2,491 agent pushes ok; the kills did not land (1 of 10 kill pushes ok, term unchanged); 5 of 1,451 lease renewals answered 200 |
| L0-13 | Pass: 6 bad acquires 100 of 100 (`am1-hosted`); then (`am1-runs`) 600 + 600 plan blocks gave 0 plan leases from non-writers and 500 from writers (63 holders, at most 8 each); a holder removed from the org refused nothing at once (2 of 2 pushes refused before, 2 of 2 accepted after); the 5 h renew read `shared` at every read after 240 min |
| L0-15, feed half | Pass with the row's 404 (contradiction 1, now in the row); the `AgentGate`, Code Mode, `/log`, and prompt halves need MCP and the controller, and LM-2, LM-4, and LM-3 carry them (`FRAME.md`, "Operator decision, 2026-11-22, B0-33 measured before a design flip, and a $50 research budget") |
| L0-16 | Pass: 0 metadata reads on 3 members for 8 checks; a merge waits for the tier by design (the row no longer lists merges) |
| LF-1, LF-2, LF-3, LF-5 | Pass |
| LF-12 | **Pass after `am1-fix`** (day 7, lf1-6): leader commit p99 474 ms after `place_learners` (6,481 ms before the fix) and 710 ms before it, with 5,000 of 5,000 polls held on 11 learners and 0 on voters. The cause was a member call: each poll on a stale learner sent its own owner-`Authz` read to that chain's leader, which delayed the repository leader's dep read; the fix (`760f9c0`) keeps one owner read in flight per member. Through the edge from the orb, client p99 is about 2.7 s even with no polls: that is the client path, not the commit, which the row measures at the leader |
| LF-13 | **Pass on `graft-m3c`** (`lf13-m3c`, site day 4, `9d59283`; `tests/am1/results/lf13-m3c.md`), both modes, 3 of 3 runs each, 999 polls held. Gap: a real gap in 3 of 3; 999 of 999 held polls returned with their cursor and no events within 1 s of the lift (99, 306, 799 ms) and of F's stop (0 ms); 1 of 50 probes `lagging` per run, because a gap ends at F's next accepted round; 0 events missing. Install: `InstallSnapshot` in 3 of 3; 999 of 999 within 1 s of the lift (788, 83, 437 ms) and of the stop (0 ms); probes `lagging` 48, 50, and 49 of 50; 0 missing, and the pre-fault cursor answered `gone`. Between the lift and F's stop, F admitted 0 to 2 of 50 probes, which its stop then answered with nothing skipped. Fixes: `3597c6f` (a stop counter, so a stop between two waits still answers a held poll; and the test-faults `/sever` route), `429eede` (a lease acquired and released in one catch-up batch reports both), and `7972504`, `606b403`, `9d59283` (the ring restarts at the install's base at once, with the installed tier's live leases); 7 of 7 planted bugs caught. Cost $2.55. Earlier, on `graft-m2b` (`am1-runs` to `lm1-rerun2`): no gap formed, and held polls ran out their own 25 s |
| LF-4 | **Pass, all eight resources** (`lf4-rest`, `graft-m3c`; `tests/am1/results/lf4-rest.md`): issues, pull requests, issue comments, reviews, review comments, commits, check runs, and releases 16 of 16 each, every hidden range absent from `visible`, `invisible_chars` 2, raw text unchanged, the untrusted writer's objects `trusted: false`, no `graftyard_trust` without the media type. The six new reads are REST GETs on the edge (F61), validated against the pinned description locally (96 REST replies) and hosted (16), 0 failed; 16 of 16 planted bugs caught. Rerun on `lf4-rest-2` (`tests/am1/results/lf4-rest-2.md`) with review comments and check runs written through the new REST POSTs instead of a test-only commit: 8 of 8 resources 16 of 16 again, every write 201. Rerun on `lf4-rest-3` (`tests/am1/results/lf4-rest-3.md`) with the check-run create's id taken from the `IdBlock` cursor in `decide` (repository 32.18): 16 concurrent creates 16 × 201 with 16 distinct ids from one block, 8 of 8 resources 16 of 16. Earlier (`am1-runs`): issues and releases only |
| B0-29 | **Fail** (`am1-runs`, 500 agents): commit p50/p99 1,205/9,316 ms (n 321); 5 of 2,500 agent pushes ok (1,772 × 503, 722 client timeouts); 1 of 99 merges; leader moved 0 → 2 with 2,305 failed member calls; 0 feed polls reached the leader |
| B0-31 | **Met on m3fr16 (2026-11-18, `graft-m3f`, run `mwlcj6zb`, main `7430a479`; `tests/am1/results/lm1-rows-m3f.md`, "Rows m3fr16"), under the operator's separate bound for rehomed groups (`FRAME.md`, "Operator decision, 2026-11-18, a separate commit bound for rehomed groups")**: buckets-up commit p99 1,834 ms (p50 773, n 5,364) against 2 s, the apac leader held at term 3 with 0 `/suspect`; placement failing, 89 Human pushes ok and 0 refused, 30 merges × 200, and 1,262 of 1,262 agent pushes over `INLINE_MAX_HOT` refused at once as `full`. `dep-hedge` asked 14 and won 2; the floor is the cross-continent `deps:all` p99 820 ms plus round p99 510 ms. Before: 4 of 2,501 agent pushes ok and the apac leader did not hold (`am1-runs`). **Re-confirmed on m67r31 (2026-11-21, `graft-m4a`, run `mwq34twn`, main `b67d6d22`; `tests/m4/results/m4-wp4.md`, "B0-31 on graft-m4a after test-faults site day 67")**, after the m64r31 to m66r31 rows on later mains missed it: buckets-up commit p99 1,595 ms and placement-failing 1,115 ms, both under 2 s, with 0 leader changes. The fix was `round-memo` (`decisions/DECISIONS.md`), which stopped the leader's in-band `Notify` and `Search` rounds from rereading the busy issue's comments for every entry. The 1 s line is the long-tail gap under the 90% rule (`decisions/OPEN-QUESTIONS.md`, F110). **Done.** Its setup's cold-shard follow-up, G11, passed on m3fg12: the planned rehome now warms the apac shard itself (`decisions/DECISIONS.md`, "`g11-warm`") |
| B0-32 | **Fail**: 0 of 500 burst pushes within 5 s, 346 × HTTP 503 and 154 timeouts, neither in the `ng` shape; 0 leader changes; commit p50/p99 714/1,199 ms (n 84); 72 of 755 agent pushes ok |
| B0-33 | **Fail** (5,000 agents, 300 s): about 0.6 agent commands/s admitted against 140; 9 of 12,454 agent pushes ok (12,194 × 503); the fast user 6 of 508 × 200 and 502 × 503; humans 4 of 487; commit p99 1,086 ms (n 53); 0 leader changes; 3,195 failed member calls |
| B0-34 | **mt18 (2026-11-22, `graft-m1`, run `mws7hr33`; `tests/am1/results/b034-mt18.md`): kill line fails a third time, and every-clone fails.** The widened hedge worked: both hedged straddlers decided on site c at about 1.29 s. But kill-phase human push p99 was 4,025 ms (75%), and the two straddlers over 3 s lost their time in front, gate and commit, not validate. Every clone ≤ 5 s: kill 46.8%, second 14.6%, because after the kill site b's shard served every miss while a long geometric repack and then a commit-graph write ran (container median up to 12.6 s). Hits p99 718 ms in steady state (217 hits; 1,555 ms over all 223). k in band (`kMax` 4), monorepo 1 → 2 → 3 never reduced, hostile owner 5 slots against `owner_share` 6 (met, as mt17); 0 Cloudflare API calls. Rows stop here until the lead picks an option. **Ruling after mt18 (2026-11-22):** (A) background maintenance yields to failover (`maint-yield`, built and tested in memory, not hosted), shipped through a site day; (C) the kill line is judged on edge time beyond transfer with every straddler included, client time beside it, which on mt18 is 3,735 ms edge and 3,813 ms client (still failing); (B) the orb-side straddlers stay as they are, since a cold client pushing as a site dies is a real push; (D) **the kill line waits on LM-1's commit-path work**, which owns the commit share of the kill-phase residual (0.7 to 1.3 s on mt18's straddlers). The cold isolate's front and gate on a write is F112. No hosted row until `maint-yield` has merged and LM-1's commit-path change has landed; then one row, a site day carrying `maint-yield`, then the full row on new networks. **mt17 (2026-11-22, `graft-m1`, run `mws0dzh5`; `tests/am1/results/b034-mt17.md`): kill line fails again, now on two slow shards in turn.** The 1 s validate hedge (`validate-hedge`) ran: the push started at the kill call hedged from the dead home to site b, which was slow too, then to c, validate 2,294 ms, client 3,486 ms (86%); widened since to ask every site left at the first hedge. Hits p99 688 ms in steady state (230 j9 hits from 1.1 s on; 1,474 ms over all 235). Every clone ≤ 5 s through kill and second 100%; k in band (`kMax` 4); 0 Cloudflare API calls. **Hostile budget met**: the hostile owner asked for 9 pool slots and held 5, and the monorepo's k was never reduced; `owner_share` (6) could not bind with 5 free, and `agent_load.rs`, `a_hostile_owner_with_three_hot_networks_is_held_to_its_share` holds it. Human push p99 3,033 / 3,128 / 2,573 / 2,963 ms (j9 / kill / second / hostile). **mt16 (2026-11-22, `graft-m1`, run `mwrlshin`; `tests/am1/results/b034-mt16.md`): hits line met in steady state, human push line fails on kill.** Clone-cache hits p99 1,263 ms measured over all 193 j9 hits, 713 ms over the 188 in steady state (j9 from 1.1 s after t0); the four cold-start hits are j9's first four, within 103 ms of t0 on a new monorepo and a new edge (1,160, 1,355, 1,263, 1,215 ms). Every clone ≤ 5 s: kill and second 100 %. k in band, `kMax` 3, never reduced; 0 Cloudflare API calls. **Human push p99 not met on kill**: 2,785 / 3,454 / 2,132 / 2,301 ms (j9 / kill / second / hostile); kill's 3,454 ms is 87% of the target, one push that straddled the shard kill and waited about 2 s in `validate` on the killed shard (its own work 36 ms). **Hostile pool budget not exercised**: the hostile networks stayed at k 1 and never asked to step up, so the next full B0-34 row needs a hostile load that steps its networks past 1. First run (`am1-runs`): fail, clone time to first byte p99 86.7 s |
| B0-35 | **Fail**, not a valid run of its line: setup opened 10 of 500 PRs; `mergeable` polls 187 `MERGEABLE` and 9,386 × 503; 63 of 2,500 agent pushes ok; commit p99 13,213 ms; dispatch not measurable (no Actions on `graft-m2b`) |
| B0-30 | **Met at 90% on the third short run (`b030r6`, 2026-11-17; `tests/am1/results/b030-split.md`, "B0-30 third short run (b030r6)")**, after failing on its fail line (`tests/am1/results/b0-30.md`; `graft-m1`, branch `b0-30`, merged into main; pushes off from simulated day 1.34). Member SQLite crossed `CHILD_GROUP_AT` (8,589,934,592 B) at 23:09:41Z on 2026-11-11, simulated day 27.52: site 0 read 8,591,708,160 B, and sites 1 and 2 crossed within 30 s. No child group took the tier, because `MetaSplit` and `RepoMeta` are types only (G6). The rest held: `SQLITE_FULL` 0 on every member throughout, and the final verify read 81 of 81 sampled archived ranges from both buckets and resolved 10 of 10 `RestoredTo` lookups. Rates: while archiving kept up (simulated days 7.08 to 22.27), SQLite grew 0.0585 GB a simulated day, which projects to about 8.21 GB at day 30, under the line [inference, linear]; during the stall it grew 0.184 GB a day. Cause of the stall: from about 17:13Z `fanout` (k = 6, with no entry found that raised it; A31) forwarded the archive promotion's `/validate` to cold pool shards, each of which read the multi-GB log tail before validating, so archive runs exceeded `RUN_FOR` (120 s); site a's promotion validate median went from 0.17 s at 16Z to 7,551 s at 21Z, and site b archived its last range at 20:17:56Z (G7). Blocker 12's cursor skip came back on site c, and the group was left with no leader three more times (F67). Cost $64.90. The earlier substitution is withdrawn ("What LM-1 still needs"). The second run (`b030r2`, `tests/am1/results/b030-split.md`) also fails: the parent stopped committing at simulated day 25.83 on a `Degraded` config that waited for an installing voter, and that member reached 10,184,200,192 B. **Re-judged at 90% (FRAME, 2026-11-16): still fails** on every run, short by day 25.83 of 27, 1.186 × `CHILD_GROUP_AT` against 1.1, and a parent that stopped committing (`b030-split.md`, "B0-30 re-judged at 90%"). **Procedure for the gate run (operator decision, 2026-11-16, option A; `b030-split.md`, "Proposal: a shorter B0-30 run from the split onward"):** a fresh group's metadata tier is seeded to 5.5 GB instead of 4 GB, so the parent reaches `SPLIT_AT` about 0.8 simulated days into the load, and journey 9 runs with pushes on to 8 simulated days after `MetaSplit`. The seed replaces the stretch before the split, which two full runs already showed: the parent grew 0.0585 GB a simulated day with archiving holding the log at `LOG_MAX_BYTES`, over days 7.08 to 22.27 on the first run (`tests/am1/results/b0-30.md`) and to day 25.8 on the second. A 5.5 GB tier is larger than the row's 4 GB, so the child and any reloaded tier are larger, not smaller. The run also forces what the row only allows: after `MetaDropped` it severs site c's parent member until it falls past the leader's archive horizon, so it must install under load, and while it installs it calls `/place` on the leader. That is stronger than the row requires, which never forces a member to lag. Judged at 90% (FRAME, 2026-11-16) on each line: every member under 1.1 × `CHILD_GROUP_AT`, zero `SQLITE_FULL`, `RestoredTo` readable for sampled archived entries, and a child group holding the tier. These count as fail lines too: site c's install restarting on a new source with the same snapshots; site c answering a metadata read or leading while its tier is dropped; a `Config` entry appended by that `/place`; and the leader's commit index standing still for 60 s while a leader exists **Second short run (`b030r5`, 2026-11-17, `b030-split.md`, "Verdict (b030r5)"):** with the `drop_due` fix for G20 (`install && applied < through`), site c, severed at `MetaSplit` below `MetaDropped`, dropped its 74 metadata tables before its load (06:48:23Z, applied 4,968), and the gate line passes at 90%: site c's peak 7.60 GB and every member under 9.45 GB, `SQLITE_FULL` 0, 41 of 41 archived ranges and 10 of 10 `RestoredTo`, both split entries committed, and `/place` refused with no config entry. **Open, not met (operator, 2026-11-17):** c's install did not finish inside the read-only cap, so 7.60 GB is a lower bound on its peak, not the peak; and the line on restarts with the same snapshots is undetermined, probably hit; under load the install restarted at every leader snapshot and never finished (G21, after r4), a correctness gap the 90% rule does not cover. r6 waits for the G21 fix. Spend $53.06 by graft-m1 script, idle $0.00 for the 30 minutes after. **Third short run (`b030r6`, 2026-11-17, main `fec1d01d` with G20, G21 and G22): met at 90%.** Site c, severed at `MetaSplit` and again at applied 5,446 below `MetaDropped`, dropped its metadata tier 6 s after the sever lifted (18:27:16Z) and installed under load, extending its job 45 times with 0 restarts; the install ended at 20:27:25Z, 2 h 0 min after the lift, replaying 231.6 entries a minute against the leader's 101. Site c's peak through the install's end was 7,743,791,104 B against 9.45 GB (1.1 × `CHILD_GROUP_AT`), every member stayed under it, `SQLITE_FULL` 0, 41 of 41 sampled archived ranges and 10 of 10 `RestoredTo` read back, `MetaSplit` and `MetaDropped` committed, `/place` during the install was refused with no config entry, and neither create held a `Degraded` config past its chosen try (G22: 0.5 s and 0.23 s). Harness-caused, not counted: a 46 s leaderless window at 17:36:56Z and a 63 s stall alert from the run's own `/log` reads, now byte-capped (G24). Gaps recorded: G23 (install source right after an install) and G25 (orb-to-colo resets; 3 of 107 unanswered pushes never reached the edge, none acknowledged and lost). Spend $49.63 by graft-m1 script, idle $0.00 for the 30 minutes after |
| B0-36 | Pass (`am1-runs`) on every surface the deployed edge routes, 100 per action, for the confined vendor App (m3-ci) and the Graftyard-run App (`graftyard-agent`) with its shipped permissions and on an installation widened to every permission; ready-for-review and REST `issues/{n}/labels` included. `Graftyard MCP`, the control, met no agent refusal. The Graftyard-run kind's git-surface refusals came from `NoLiveTask`; its lease refusal and task pushes moved to AM3-WP3 (operator, 2026-11-10). GraphQL `addLabelsToLabelable`, rerun on `graft-m3c` (`lf4-rest`): 100 of 100 refused by the allowlist rule for m3-ci and for `graftyard-agent` shipped and widened, 0 of 100 for `Graftyard MCP`, a label inside the allowlist accepted 5 of 5 for every kind; the same again on `lf4-rest-2`, with m3-ci flagged through `PUT /api/graftyard/v1/agents/apps/{id}` by the m3 admin, now a site admin of `graft-m3c` |
| B0-37 | Push half pass at 100 per kind (about 1,000 pushes, not 10,000): 0 parse failures, every forged trailer `Claimed`, 100 of 100 `refs/graftyard/` writes refused. Fetch half partial: session refs stay hidden from v0, v2, and `clone --mirror`, but no session ref exists to list, because only `graftyard-workspace` under a live Graftyard-run task writes one (AM3-WP3); `containers/git/server/sessions_test.go` covers the listing |

The design took the hosted half's findings (`decisions/DECISIONS.md`, "AM1's hosted half: what the code needed, and LM-1 pending `am1-fix`", and "`am1-fix`: the stall crosses a leader change, and the feed shares one owner read"). `am1-fix` took 9 of 9 planted bugs (`tests/am1/fix/plant.py`) and no record format.

**Why the load rows fail (2026-11-09, `am1-runs`, "Diagnosis").** The git shard's `/validate` holds the network's exclusive lock (`containers/git/server/main.go`, `handleValidate`), so with k = 1 one network's pushes validate one at a time, 405 to 515 ms each unloaded: about 2 pushes/s, against journey 9's 4.17/s. With the same driver, 100 agents pushed 148 of 148 (commit p99 933 ms) and 250 pushed 379 of 379 (992 ms), with no leader change; at 500 the validate queue diverged (p90 17.7 s, max 58 s), edge calls timed out, followers' `/raft` calls took about 3.1 s, and leaders stepped down. `fanout` and the shard's `NetworkLoad` report are still "Not yet" under AM1-WP1, and DESIGN's k = 1 at journey 9 counts git CPU, not this lock (`decisions/OPEN-QUESTIONS.md`, A25). Branch `fanout` builds concurrent validates inside one shard, `fanout`, and the `NetworkLoad` report, then reruns the load rows hosted; the cost figures stay as they are until that rerun measures them.

**After `fanout` (2026-11-10)** (`tests/am1/results/fanout.md`, section 3). The validate lock is gone (AM1-WP1, "`fanout`"), and at 100 agents every push landed with commit p99 885 ms. Every 500-agent rerun still failed, on the next limiter: the edge read gate read the repository's access rows on the group's leader for every git request and App read under an installation token, which it did not cache, about 260 leader `/read`s a second. Unanswered reads became `suspect` reports, leaders stepped down, and the gate answered 503 "the repository's access rows did not answer". The reruns, with k forced to 4 except B0-34:

| Row | Rerun on `graft-m2b` |
| --- | --- |
| B0-29 | Fail: 105 of 2,500 agent pushes ok (1,897 × 503 from the gate, 440 timeouts); commit p99 30,092 ms (n 9); merge p99 120 s; leader 0 → 2; 16,777 leader `/read`s and 20,199 `/suspect` in 600 s |
| B0-31 | Fail: 133 of 2,500 ok (1,777 × 503); commit p99 2,508 ms (n 12); the apac leader did not hold, and the placement-failing half was not reached |
| B0-32 | Fail: 0 of 500 burst pushes within 5 s (323 × 503, 132 × 500 "no member leads", 43 timeouts), none in the `ng` shape; leader 0 → 2 |
| B0-33 | Fail (5,000 agents): about 2.5 agent commands/s admitted against 140; 62 to 331 of 12,546 agent pushes ok (a driver parser bug, `994266c`, bounds the count); commit p99 3,415 ms; no leader change |
| B0-34 | Fail: k stayed 1 (a follower's `/fanout` error); clone time to first byte p99 172.4 s; 0 of 37 human pushes ok; commit p99 361 ms |
| B0-35 | Fail: the setup now opens 500 of 500 PRs (am1-runs' 10 of 500 was very likely the same parser bug); `mergeable` polls 10 `MERGEABLE` and 10,162 × 503; 0 of 2,500 agent pushes ok |
| L0-12, A1 replay | Fail: 0 events reached the 500 pollers in 600 s of 211,151 polls; 0 of 10 kill pushes landed |
| LF-13 | Fail: install mode reached its fault in 4 of 6 runs, refused new polls (45 to 47 of 50 `lagging`), and lost no event (the ring answers `gone`, `dfa5410` deployed), but F stopped its held polls in only 2 of 4; gap mode reached a real gap in 1 of 6, with 0 `lagging` and one lease event fewer on F's ring than on the third voter, an open bug candidate. The row's "within 1 s" does not say of what: the lift or F's stop |

`04487ca` (merged at `1e25da8`) caches installation-token decisions in the gate by `jti` for `READ_CACHE` (`decisions/DECISIONS.md`, "The read gate caches installation-token decisions by `jti`"); that it removes the limiter is [inference] until the rerun. LM-1's load rows are being rerun on `graft-m2b` with it. Hosted cost about $4.46 in Workers Analytics, $12.20 at most with containers [inference].

**After the gate cache (2026-11-10)** (`tests/am1/results/lm1-rerun.md`; branch `lm1-rerun`, merged at `b2476d0`). A site day and edge `b94893bd` from `1e25da8` on `graft-m2b`, carrying `04487ca`, `105c228`, and `dfa5410`. The gate cache worked: a 100-agent smoke landed 101 of 101 pushes and 4,869 of 4,869 API reads with 7 `/suspect` reports and member peaks of 32 to 74 MiB. Every 500-agent row then failed on repository member memory: members peaked at 250 to 331 MiB against the 128 MB isolate, with thousands of `exceededMemory` outcomes, resets, `StepDown { CommitErrors }`, `/suspect` storms, and leader churn, during which the gate's `access_rows` read found no leader and answered 503 [measured]. The likely cause is the whole-tier `{"rows": true}` read, 0.5 to 1.5 MB on the leader (1.46 MB and 1.5 s measured on 131205), that every agent push (`push_context`) and every REST and GraphQL read made: 60 parallel whole-tier reads failed 50 of 60 on one leader, while 60 `{"rows": {"access": true}}` reads all succeeded [inference for the cause]. `ccd10b1` (merged at `d8bed7c`) makes an agent push read only `{"rows": {"agent": true}}`, the `agent_policy` row and the `agent_task` rows; REST and GraphQL still read whole tiers. A rerun on `graft-m2b` is measuring both.

| Row | `lm1-rerun` on `graft-m2b` |
| --- | --- |
| B0-29 | Fail: 274 of 2,500 agent pushes ok (1,425 × 503 from the gate, 224 × 500, 567 timeouts); commit p50/p99 1,063/1,505 ms (n 144); merges 2 of 3, p99 76 s; API reads 4,143 × 200, 9,576 × 503; leader held; `exceededMemory` 11,117 / 4,478 / 2,814 on sites a/b/c. With API reads off (diagnostic, 300 s): 600 of 1,249 pushes ok and clean for 2 minutes, then members ran out of memory |
| B0-31 | Fail: 48 of 2,500 ok (1,741 × 503); commit p99 2,359 ms (n 93); the placement-failing half was not reached |
| B0-32 | Fail: 0 of 500 burst pushes within 5 s (456 × 503, 44 timeouts), none in the `ng` shape; leader 0 → 2; 40,180 leader `/read`s and 51,362 `/suspect` |
| B0-33 | Fail (5,000 agents): 101 of 12,484 agent pushes ok (10,633 × 503); about 2.1 side commands/s admitted against 140; 0 of 29 human pushes; leader 0 → 1; members 331 / 262 / 250 MiB |
| B0-34 | Invalid: stopped at about 07:15Z on a false cost reading (account-wide rows that were B0-30's); 103 of 2,577 receive-packs answered 200; k stayed 1, though `/fanout` reports now answer ok (16 of 17 in B0-29) |
| B0-35 | Not run: it would have started at $11.42 against the $12 row stop |
| L0-12, A1 replay | Fail: 0 events timed at the pollers of 146,705 polls; 1 of 10 kill pushes landed; 137 of 2,490 agent pushes ok; members 283 / 280 / 265 MiB |
| LF-13 | Fail, with `HELD=999`. Install reached its fault in 2 of 3 runs: every held poll came back with its cursor and no events, 48 and 47 of 50 new polls answered `lagging`, and no event was lost (F answered `gone`), but only 630 and 479 of 999 returned within 1 s of the lift (the slowest at 1,003 and 1,004 ms), and F itself stopped only 193 and 0 of them; the rest returned before the lift at their own 25 s wait. Gap left no gap in 3 of 3 (O level with F each time) (A26) |

Cost of the round: $12.41 for `graft-m2b` alone, containers included ($9.22 of it), from `tests/fanout/m2bcost.py` at overage prices; LF-13 alone $0.82.

**After the narrowed reads (2026-11-10)** (`tests/am1/results/lm1-rerun2.md`; branch `lm1-rerun2`, merged at `f51cc73`). Three site days and edge `d579a6b0` on `graft-m2b` from `d8bed7c`, which carries `ccd10b1`. Two read fixes went in: `eb8eb0b` answers a whole-tier read as one packed JSON string, so neither side builds a `serde_json::Value` tree (a 740 KB answer had been a 13 MB tree, peaking at 24 MB per read [measured, local]), and `a271fb3` leaves `path_set` and `provenance` out of it, which no view reads from the tiers; together they took the read from 1.6 MB to about 150 KB at 500 agents. 6 of 6 plants caught; 642 tests passed. Members still exceeded 128 MB, so the remaining cause is [unverified] (`decisions/OPEN-QUESTIONS.md`, F65).

| Row | `lm1-rerun2` on `graft-m2b` |
| --- | --- |
| Smoke, 250 agents, API reads off (`ccd10b1` only) | 491 of 624 pushes ok; leader 0 → 2; members 280 / 113 / 105 MiB; 607 `exceededMemory` on site c |
| Smoke, 500 agents, API reads off | `ccd10b1` only: 300 of 1,232 ok, 6,924 `exceededMemory` on site c. With `eb8eb0b`: 812 of 1,254 ok, still a leader change and 1,621 `exceededMemory` on site b |
| B0-29, twice | Fail. With `eb8eb0b`: 118 of 2,501 agent pushes ok (1,690 × 503 from the gate), commit p99 37,954 ms, members 273 / 247 / 201 MiB. With `a271fb3`: 107 of 2,501 ok (1,770 × 503), commit p50/p99 1,757/3,967 ms, members 267 / 265 / 263 MiB though a whole-tier read was about 150 KB; 87,000 leader `/read`s and 108,000 `/suspect` |
| B0-35 | Fail: the setup opened 500 of 500 PRs, then from about 17:08:30Z every edge request answered 503. The group was left with no leader: at 17:21Z, after the load stopped, all three voters were followers at term 4 with no leader, after 150,399 `/suspect` reports and 78,842 `/read`s to one of them, and one test-only `/campaign` to each elected site 0 within 5 s. Member peak 279 MiB. Diagnosed by `lm1-memory`: the term-2 leader f0 was stuck in one alarm invocation for 641 s wall (622 s CPU, 41.9 M rows read), building dumps for about 730 reads whose callers had gone (fixed in `f125a37`), while f1 kept hitting `exceededMemory`; elections were not at fault and `crates/raft` is unchanged. Test-merge vCPU and list-field computes are still not instrumented (AM1-WP1, "Follow-up: the test-merge budget and B0-35's instruments") |
| B0-32 | Fail: 12 of 500 burst pushes ok, none in the `ng` shape, p50 27 s; leader 0 → 2; 63,043 `/suspect` on the leader; member peak 318 MiB |
| B0-31 | Fail: 126 of 2,501 ok; commit p99 9,754 ms; the term churned from 3 to 28 in the run, with one voter at a time not answering `/status`; 128,734 `/suspect` on the leader; member peak 265 MiB |
| B0-33, B0-34, L0-12 A1 replay | Not run: the round's $13.40 no-new-row threshold was reached after B0-31 |
| LF-13 | Fail on `graft-m2b` (gap left no gap in 3 of 3; install's held polls returned up to 1,228 and 1,741 ms after the lift), its last run there. LF-13 has moved to its own install, `graft-m3c` (thread T-01a1c666-2b83-7522-8afd-45a508a3cd74) |

Cost of the round: $13.91 for `graft-m2b`, containers $10.77. The memory work continues on branch `lm1-memory` on `graft-m2b`.

**After the read cap (2026-11-11)** (`tests/am1/results/lm1-memory.md`; branch `lm1-memory`, merged at `dcf360f`). Memory grew with whole-tier reads in flight on the leader, not with held polls or reply size alone: on a 1.18 MB dump, 10.5 MB at 1 read in flight, 59 MB at 40, 92 MB at 60, and 100 reset the isolate. Missed reads became `suspect` reports. The fixes (`f125a37`, `1263bfd`, `a0efa82`, `d9ee594`): a packed-dump cache on the member keyed by both tiers' `applied` index and row-hash sum, no dump for a canceled read, `HEAVY_READS` (16) answered `busy`/`overloaded` with `retryAfterMs`, which the lookup retries and never counts as missed or reports with `suspect`, `suspect` damped per (chain, member, term, scope) every 500 ms, streamed answers, and a stale dump rebuilt at most once a second. Edge coalescing of identical reads was built and removed, because Workers refuses a promise shared across requests (`decisions/DECISIONS.md`, "F65: a leader caches its packed dump"). F65 is narrowed, not answered: memory is no longer the 500-agent smoke's failure, but leaders still step down on `Confirm` (storage confirm p99 1.7 to 2.1 s) and on `CommitErrors` at about 300 req/s, and about 600 cheap reads completed in one millisecond after a stall and then `exceededMemory`. The next hypothesis [unverified]: one quorum round grants every waiting read and the host builds all their answers before sending any (600 parallel `{"q":{}}` reads took an idle leader from 55.8 to 94.1 MB). The second round on `lm1-memory` tested that. Round 2 (`80d8837`) capped every read in flight (`READS_MAX`) and built granted answers once per query as streamed bytes: at 500 agents no member logged `exceededMemory` in B0-29, B0-31, B0-32, B0-34 or B0-35, and B0-32 had no leader change or `suspect`; every row still fails its gate on commit p99 (3.2 to 7.3 s) and clone TTFB, because reads saturate the leader's thread and its confirms and AppendEntries replies step late. Next: API reads by read-index on followers. L0-12's A1 replay was not run (cost cap). Round 3 (`54c45213`, "Follower read" in DESIGN) sent those reads to followers: B0-29 held its first leader 7 minutes and B0-32 held its leader, but every row, L0-12 and B0-35 included, still fails its gate, because a read both followers shed falls back to the leader and followers saturate at about 10,000 reads a minute; B0-35's f1 ran out of memory once. Next: answer 503 with `Retry-After` when both followers shed, learners for reads, and an edge cache of the read gate's access decision (`tests/am1/results/lm1-memory.md`, section 10). Round 4 (`48f8fae1`) answered a read every member asked sheds with 503 and `Retry-After`, never the leader's, and placed 8 learners per row that take those reads first: B0-29 and B0-34 held one leader at term 2 with 0 `suspect` and B0-32 had no leader change, but every row still fails its gate (commit p99 2.3 to 8.6 s, clone TTFB p99 158 s, feed p99 13 s on L0-12), members shed about half the API reads, and the busy leader is now the owner's `Authz` group, which every REST and GraphQL read and every repository entry's `deps` read on its leader (`tests/am1/results/lm1-memory.md`, section 11). Next: `Authz` dep reads by read-index on its followers and learners.

| Row | `lm1-memory` on `graft-m2b`, `d9ee594` |
| --- | --- |
| Smoke, 500 agents, 300 s | Every member under 80 MB (peaks 69 / 35 / 78 MB); 1,237 of 1,242 agent pushes ok (5 refused); commit p50/p99 793/2,956 ms at the leader; leader 0 → 2, on `Confirm` then `CommitErrors` |
| B0-29 | Fail: commit p99 3,867 ms against 600 ms (p50 1,000, n 2,077); leader 0 → 1; 1,089 of 2,500 agent pushes ok (666 `stale`, 406 timeouts, 205 `missed`); the apac member at 262 MB |
| B0-32 | Fail: a leader change, 71 `suspect`, commit p99 4,834 ms; 48 of the burst ok, 452 refused |
| B0-31 | Stopped early, before its row line |
| B0-33, B0-34, B0-35, L0-12 A1 replay | Not run: the 500-agent bar did not hold at 600 s, and the rows would have passed the cost cap with nothing new to test |

Cost of the round: $15.75 for `graft-m2b`, containers $10.45, by `tests/fanout/cost.sh`.

**Round 3, follower reads (2026-11-11)** (`tests/am1/results/lm1-memory.md`, section 10). Site day and edge `1e11c400` on `54c45213`, at 500 agents unless the row says otherwise.

| Row | Gate (pass / fail) | Round 3 on `graft-m2b`, `54c45213` |
| --- | --- | --- |
| Smoke, 500 agents, 300 s | None | Peaks 75 / 55 / 112 MB, 0 `exceededMemory`; the leader stepped down on `CommitErrors` 15 s into load; commit p99 5,675 ms |
| B0-29 | Commit p99 ≤ 600 ms, merge p99 ≤ 2 s, zero `Stale` on distinct refs, leader < 600 req/s with zero feed polls / commit p99 > 1 s, a queue merge > 10 s, a feed poll on the leader | Fail: commit p99 3,419 ms (p50 662, n 8,167); leader 0 → 1 → 2, the first step-down 7 minutes into load; 7,774 `/suspect`; 2,412 of 2,500 agent pushes ok; merge p99 9,303 ms; 306 req/s and 0 feed polls on the leader |
| B0-32 | Every push acked within 5 s or `ng … admission full`, merges in the next 2 entries, zero leader changes and `suspect` | Fail: zero leader changes but 3 `suspect`; burst 168 ok and 332 refused, not in the `admission full` shape, 1 within 5 s; merge p99 41,005 ms |
| B0-31 | Commit p99 ≤ 1 s with buckets up / > 3 s | Fail, between the lines: commit p99 2,541 ms; leader 0 → 1 in two moves (`CommitErrors`, then `Confirm` p99 1,377 ms) |
| B0-34 | k in band ≤ 8, human push p99 ≤ 3 s beyond transfer, clone TTFB p99 ≤ 1 s / TTFB p99 > 5 s, k > 8 | Fail: clone TTFB p99 58,554 ms (p50 1,643); k 1 → 2 in band; human push p99 240 s |
| B0-33, 5,000 agents | Human and merge-queue p99 ≤ 1 s, ≥ 140 agent commands/s, zero leader changes and `suspect` / a leader change or `suspect`, < 100 commands/s | Fail: term 2 → 19 in 5 minutes, 27,703 `/suspect`; commit p99 15,980 ms; 127 of 12,523 agent pushes ok, 6,925 answered 503 by the read gate |
| L0-12, A1 replay with a leader kill every 60 s | Feed p99 ≤ 2 s outside kills, no lease overlap over 500 ms, refusal counts right | Fail: feed p99 64,031 ms (p50 2,647); f0 led through 10 kills (term 2 → 5); f0 peaked at 275 MB with 711 aborted `/read`s |
| B0-35, 500 open PRs | Test-merge vCPU within `TEST_MERGE_BUDGET`, dispatch p99 ≤ 10 s, zero list-field computes | Not judged: the budget was not instrumented and `graft-m2b` has no Actions; `mergeable` 3,876 `MERGEABLE` and 24,168 × 503; f1 logged 812 `exceededMemory` |

Cost of round 3: $6.39, containers $1.76.

**Round 4, learners first and no leader fallback (2026-11-11)** (`tests/am1/results/lm1-memory.md`, section 11; `decisions/DECISIONS.md`, "F65 follow-up: a read the members shed is 503, never the leader's"). Site day and edge `460e0956` on `48f8fae1`, 8 learners per row (enam 4, weur 2, apac 2), the same gate lines as round 3.

| Row | Round 4 on `graft-m2b`, `48f8fae1` |
| --- | --- |
| Smoke, 500 agents, 300 s, 3 learners | Leader held term 2 with 0 `/suspect`; commit p99 3,966 ms; 1,218 of 1,241 agent pushes ok; `read:api` 15,913 × 200 and 36,422 × 503 with `Retry-After: 1`; leader peak 95 MB, no `exceededMemory` |
| B0-29 | Fail: commit p99 5,461 ms (p50 834, n 6,793) against 600 ms; 0 leader changes and 0 `/suspect`; 2,250 of 2,500 agent pushes ok; merges 13 of 28 answered, p99 18,971 ms; `read:api` 51,594 × 200 and 52,858 × 503; the org's `Authz` leader took 87 `/read`/s and stepped down |
| B0-32 | Fail: 0 leader changes and 0 `/suspect`, but the burst had 49 ok and 446 refused, not in the `admission full` shape, 0 within 5 s (p50 26.6 s) |
| B0-31 | Fail, between the lines: commit p99 2,311 ms (p50 658) against 1 s, under the 3 s fail line; 29 `/suspect` |
| L0-12, A1 replay with a leader kill every 60 s | Fail: feed p99 12,984 ms (p50 1,141) against 2 s; term 11 after 10 kills; voters peaked at 256 to 278 MB; lease overlap not reported by the driver |
| B0-34 | Fail: clone TTFB p99 158,542 ms (p50 846) against 1 s; k 1 → 2 in band; leader held term 2 with 0 `/suspect`; human push p99 121 s |
| B0-33, 5,000 agents | Fail: term 2 → 8, 7,539 `/suspect`; commit p99 8,568 ms; 2,020 of 12,533 agent pushes ok |
| B0-35, 500 open PRs | Not judged: the budget's instruments (`merge-budget`) were not in this build and `graft-m2b` has no Actions; 18,870 `mergeable` reads answered 503 |

Cost of round 4: $11.19, containers $1.93; rounds 1 to 4 together $43.56.

**Round 5, `Authz` reads by read-index and the commit split (2026-11-11)** (`tests/am1/results/lm1-memory.md`, section 12; `decisions/DECISIONS.md`, "F65 follow-up: the edge reads an owner's `Authz` by read-index"). Site day on `7583378f`, which adds the leader's commit split to `/stats`, then edge `e2cfdfba` and `afb025ac`, which read an owner's `Authz` fold and its `o/r` rows by read-index; 8 learners per repository and 4 on the org's `Authz` group; the same gate lines as round 3. On a 500-agent B0-29-shaped smoke before the edge change, the leader's commit p99 was 50,066 ms, with `Authz` dep reads at p99 17,713 ms; after it, commit p99 was 1,739 ms with `Authz` dep reads at p99 366 ms, and the leader held. Storage confirm stayed at p99 38 to 202 ms.

| Row | Round 5 on `graft-m2b`, sites `7583378f`, edge `afb025ac` |
| --- | --- |
| B0-29 | Not judged on commit: the leader held (site 0, term 2, 0 changes, 0 feed polls), but its `/stats` split was lost when the isolate reset. Merges fail: 4 × 200 and 25 × 503. 2,305 of 2,500 agent pushes ok |
| B0-32 | Fail: burst 147 ok and 352 refused, not in the `admission full` shape (p50 23,063 ms); commit p99 932 ms; two `CommitErrors` holds, term 2 → 4 |
| B0-31 | Between the lines: commit p99 1,163 ms against 1 s, under the 3 s fail line (round 4: 2,311 ms); the leader held |
| L0-12, A1 replay with a leader kill every 60 s | Fail: feed p99 14,291 ms against 2 s; leader 0 → 1, term 6; the last leadership's `Authz` dep reads p50 2,246 ms and 956 `/suspect`. Lease overlap is not measurable: the driver takes only Shared leases, which never conflict |
| B0-34 | Fail on human push p99, 121 s (client timeouts). Clone TTFB p99 2,253 ms is between the lines (round 4: 158,542 ms); k 1 → 2 in band; the leader held |
| B0-35 | Judged, and fails over budget: test merges used 0.274 vCPU against 0.25 (707 computed, 6,043 shed). List-field computes 0, which passes. Dispatch p99 not instrumented (no Actions). `mergeable` 1,949 `MERGEABLE`, 16,018 `UNKNOWN`, and 39,990 × 503 |
| B0-33, 5,000 agents | Not run: the round's $10 stop came first |

Every step-down in round 5 was `CommitErrors`, none `Confirm`. In the rows, 29 to 71% of `read:api` still answered 503, shed by 8 learners and 2 followers at `HEAVY_READS`. After both smokes the org's `Authz` group was left with no leader (three followers at terms 53 and 55) until a test-only `/campaign` (`decisions/OPEN-QUESTIONS.md`, F67).

Cost of round 5: $10.56, containers $2.34; rounds 1 to 5 together $54.12.

**B0-29 rerun on main `19b5ae77` (2026-11-17, `graft-m2b`, `tests/am1/results/b029-m2b.md`).** Run `mwkyk9zg`, repository 131351, with 8 learners, k 4 forced, 500 open PRs and 600 s. Probe-known, query-tag, read-ahead, burst-receipts and learn_leader are in the build. **Fail on commit p99**: 1,682 ms against 600 ms (p50 513, n 10,931), past the 1 s fail line and past the 660 ms the 90% rule allows. The tail is `queue:slot`, the wait for the one entry in flight (p99 4,308 ms); the quorum `round` p99 is 514 ms. Merge p99 is 4,398 ms (`merge:only`, 67 of 73 answered 200, none over 10 s) and API read p99 is 2,661 ms with 99.58% served, so both are between the lines. These pass: 0 `Stale` (2,499 of 2,500 agent pushes ok), the leader at 176.7 req/s with 0 feed polls, and term 2 held with 0 leader changes. This is a regression from m27's 774 ms (2026-11-14) on the same shape. Every stage of the cycle is slower (round p99 336 → 514 ms, confirm 58 → 231, `deps:all` 222 → 715), entries fell from 2,895 to 2,268, and the leader took 1.7 times the `/raft` calls and 565 `/outcome` asks against 7. The cause is not traced; the candidates are listed in the results file. An earlier run that day, `mwkxemsq`, is void: its learners joined but their config change never committed, and the driver now skips such a row.

**B0-29 on read-beat (2026-11-17, `graft-m2b`, `tests/am1/results/b029-m2b.md`, "Run 3").** Run `mwlbozdl`, repository 131354, site day m46 from main `56918444` (read-beat, no F85), the same row shape. **Fail on commit p99**: 1,569 ms against 600 ms (p50 476, n 10,809). Read-beat did what it claimed: AppendEntries carrying entries fell from 6.35 and 7.81 per entry to 1.00 and 1.06, and entry bytes to the followers from 1.25 to 1.75 MB/s down to 0.19 to 0.21 MB/s, m27's level. But entries cut rose only from 3.8/s to 4.3/s (m27 4.8/s), and `queue:slot` p99 went from 4,308 to 3,172 ms (m27 462). The leader's `/stats.clock` shows 97.4% of stamps lagging 256 ms or more and 5,061 lease fallbacks, so it was suspect for most of the load [measured]. Merge p99 3,649 ms and API read p99 2,050 ms (99.86% served) are between the lines; 0 Stale; 2,499 of 2,500 agent pushes ok; leader held term 2 at 165.7 req/s with 0 feed polls.

**B0-29 on F85 plus clock-guard (2026-11-18, `graft-m2b`, `tests/am1/results/b029-m2b.md`, "Run 4").** Run `mwlhgt8y`, repository 131356, site day m48 from main `9ffb285d`, the same row shape. **Fail on commit p99**: 1,122 ms against 600 ms (p50 433, n 10,796; run 3 1,569). Clock-guard and F85 did what they claimed: `leaseFallbacks` 0 (run 3: 5,061), `lagMax` 227 ms, 0 read rounds a second to the followers (26.5 before), and 0 learner calls held 500 ms (22,808 before). The tail is still the single entry slot: `queue:slot` p99 2,476 ms at 4.3 entries a second. Merge p99 4,490 ms and API read p99 3,214 ms (100% served; the edge's own `api:total` p99 1,418 ms) are between the lines; 0 Stale; 2,500 of 2,500 agent pushes ok; leader held term 2 at 151.9 req/s with 0 feed polls.

**B0-29's queue split on one clock (2026-11-18, code-only, `tests/m0/results/cut-trace.md`).** The `queue:slot` figures in the three B0-29 paragraphs above mix two clocks and overstate the wait: on run 4's trace the queue p99 is 934 ms on one clock against 2,532 ms as `/stats` measured it. Commit p99 and the verdicts stand. Branch `cut-trace` puts the split on one clock and adds a per-cut trace row (kind 29, test-faults) for the next B0-29. With one entry in flight, p99 is about two rounds, so 600 ms needs a round p99 near 250 ms (run 4: 349) [inference].

**B0-29 run 5 with per-cut rows (2026-11-18, `graft-m2b`, `tests/am1/results/b029-m2b.md`, "Run 5").** Run `mwlu6gge`, repository 131357, site day m49 from main `94fdfb48` (cut-trace and b032-shed among the changes since run 4), the same row shape. **Fail on merge p99**: `merge:only` p99 12,516 ms, past the 10 s fail line. Commit p99 **669 ms** (p50 355, n 10,696; run 4 1,122), between the lines and 9 ms past the 660 ms that 90% allows. API read p99 2,777 ms (between the lines, 100% served less one 503); 0 Stale; 152.1 req/s and 0 feed polls on the leader. All 3,057 cuts ended `Empty`: no admission limit binds. Round p99 255 ms reached the two-round floor's 250 ms and commit p99 still did not reach 600 ms; the rest is the freeze wait and the post-apply dependency read (`tests/m0/results/cut-trace.md`, "Run 5").

**B0-29 run 6 with merge-tail and dep-hedge-adaptive (2026-11-18, `graft-m2b`, `tests/am1/results/b029-m2b.md`, "Run 6").** Run `mwm4d2ho`, repository 131360, site day m51 from main `3e548e95` (17 merges since run 5, among them b034-reads), the same row shape. **Fail on commit p99 and API reads served**: commit p99 7,720 ms (p50 2,205), API reads 86.7% answered 200 (12,384 × 503). Merge p99 8,553 ms, between the lines (0 × 503, 14 × 409). 1 agent push `stale` of 2,500, cause unverified (42 client `fetch failed`). The leader served 21,329 `ls-refs` refs pages, new with b034-reads; requests open in it rose from p50 7 to 36 and its round p99 from 272 to 2,353 ms (F91, open). merge-tail's resolve fix held (push resolve p99 6,039 → 815 ms, no names:708 exception, no lost merge answer). The adaptive hedge asked 416 of 3,151 reads (13%), most at the 1 s cap, because every chain's p90 passed 1 s.

**B0-29 on `graft-m3f` with looplag-clock (2026-11-19, `tests/m0/results/looplag-clock-m3f.md`).** Row m3fp3, repository 65597, run `mwo5m105`, site day from main `3e521931` (edge from `ab6086b4`), m3fhg's script and load. **Fail on commit p99**: modelled 1,793 ms over the row (per leader segment 1,210–2,198), `/stats` 4,435 (site 0's terms 2 and 4); no stall of 10 s or more. 3 leader changes: a `CommitErrors` trip at 23:09:53 with 63 failed calls, the transfer home (unverified), and a `LoopLag` trip at 23:15:37 on two syncs with real ticker lag, which the amended rule meant to keep. The leader's setup baseline matched m3fp2's (1-statement confirm p50 / p90 23 / 33). held-grant's flip passes (1 of 123). Site a's two Member objects, site 0's voter among them, logged 220,515 × "closure invoked recursively or after being dropped" from 23:08:57.536 to the end; they came from learner 195 and site 0's voter sharing one isolate, where the repository-keyed `FEEDS` carries one member's feed wakeups into the other's polls (in the code since `7613c04a`; graft-m2b's 11-13 burst had the same signature on `24dec508`). Not a regression in `d8dcd513..3e521931`; a fix keyed by member is proposed.

**B0-29 on `graft-m3f` with confirm-storage (2026-11-19, `tests/m0/results/confirm-storage-m3f.md`).** Row m3fp2, repository 65596, run `mwnxzqr1`, site day from main `d8dcd513` (edge from `ab6086b4`), m3fhg's script and load. **Fail on commit p99**: `/stats` 1,160 ms (p50 480), with no stall of 10 s or more. The leader's storage was slower from setup on (confirm p99 273 against m3fhg's 80–107; 1-statement p50 23 against 16 before load) with the same flush sizes, entries and load, and it reached the rounds through the output gate. Mapped onto m3fhg's confirms the model gives 964, and 866 with m3fhg's deps reads too. No Confirm trip, so no verdict on `confirm-storage`. The one leader change was a `LoopLag` trip on two member-clock samples of 500 ms that read 532 and 533 ms on the isolate clock, with no lag row: it meets `LOOP_LAG`'s flip (`decisions/DECISIONS.md`). held-grant's flip passes again (0 of 223). Edge 500s 0; outages 0.8%, valid.

**B0-29 on `graft-m3f` with held-grant (2026-11-19, `tests/m0/results/held-grant-m3f.md`).** Row m3fhg, repository 65595, run `mwnmao4d`, site and edge day from main `ab6086b4`, run 7's shape, judged against the 800 ms line (`FRAME.md`, 2026-11-19). **Fail on commit p99**: `/stats` 1,325 ms (p50 406), over the 1 s fail line. A 24 s storage confirm on the site 0 leader (14:12:11–14:12:35) held every send at the output gate, with 0 event-loop lag and 0 failed calls before it, and the leader moved 0 → 1 → 0. Modelled p99 670 ms before the stall, 1,105 after it, and 859 without it. Round p99 outside it was 322–323 ms, against run 7's 316. held-grant's flip passes: 0 of 179 held-and-stepped leader reads walked, against about 95% before. Edge 500s 0; outages 1.3%, valid. What a leader should do about such a stall is open (`decisions/OPEN-QUESTIONS.md`, F99).

**B0-29 on `graft-m3f`, three runs on today's merges: fails narrowly on the median of three (2026-11-21, `tests/am1/results/b029-m3f.md`).** Rows m3fb29 (repo 65604, site day from main `a1e431a6`: `soft-grow`, `command-rows`, `leader-render`, `catchup-pieces`, `step-busy`, `dep-hedge-other`), then m3fb2r2 and m3fb2r3 (repos 65606 and 65607, site day from main `54638269`), run 7's shape, each valid (outage share 0.0017). **Median commit p99 1,102 ms** of 1,102, 731 and 1,272 (p50 450, 357 and 462), over the 1 s fail line, so B0-29 fails narrowly. On the median, merge p99 1,844 ms, 0 `Stale` failures on distinct refs, leader 151 req/s with 0 feed polls, and API read p99 1,628 to 1,730 ms all pass; 0 leader changes in all three runs. Run 2 missed merge on its own at 2,404 ms (n 79), over the 2.2 s 90% line and under the 10 s fail; runs 1 and 3 passed it at 1,844 and 1,592. The commit tail is queueing, not storage: queue:slot p99 704, 429 and 746 ms. On run 1's trace, every command in the slot tail waited behind one or two frozen batches, mostly on the entry in flight (63%), not on deps (deps:all p99 447 ms). LM-1's leader-cycle change, which covers this row and B0-33, comes before any further B0-29 row.

**B0-29's frozen-batch wait is mostly not the leader's apply CPU (2026-11-23, $0, `tests/am1/results/b029-m3f.md`, "The frozen-batch wait and the leader's CPU").** The B0-33 join (`tests/fanout/heavyturns.py`) run on the three runs' traces and leader Workers Logs (`tests/fanout/b029join.py`) finds the same mix at a tenth the size. The leader used 0.19 to 0.23 vCPU. Apply and the deferred transaction are 57 to 60% of its joined CPU, but an entry carries 5 to 6 events, so a send turn costs p50 11 to 14 ms. The ticker logged no lag of 20 ms or more in any run [measured]. Taking every send turn's CPU off every cycle a command waits through moves p99 by 46 to 79 ms at most, so the median run would go from 1,102 to about 1,020 ms on `/stats` [inference, replay]. LM-1's apply cut is part of B0-29's fix, not all of it. The larger part is the window between the early freeze and the in-flight entry's apply, which costs a whole extra cycle to 60% of commands. The proposal, not built, re-asks a frozen `Empty` batch's dependency read each time it answers until the entry before it applies, and moves commands admitted since the cut into the batch. Read-after-admission is kept. Replayed, it brings the median to about 958 ms, and to about 869 ms with LM-1's cut, within the 880 ms that 90% allows, while m3fb2r3 (placement) stays over the fail line. B0-29's next rows run on graft-m4a (`tests/fanout/b029-m4a.sh`) once the lead clears them: first LM-1's cut alone, then this change.

**B0-33 on site day m53, both write cuts and F91 (2026-11-18, `graft-m2b`, `tests/am1/results/b033-m53.md`).** Repository 131364, main `adfb3aeb`, `arrival=mem`, 4 learners. **Fail on every line**: leader transactions p99 476 statements (cut 2 held), leader confirm p99 2,000 ms, 21 leader changes (terms 2 to 23), 42.9 agent commands/s. The changes came from SUSPECT's two-colo withhold, which fired for the first time on a hosted B0-33 row: reports from LAX and DFW, against LAX only on m45 and m50 to m52. **Leader-change counts on B0-33 rows before m53 are not comparable with it**: those rows never exercised the withhold, and m52's leader was late for its followers in 203 follower-seconds. From m53 every B0-33 and B0-29 row records its reports by colo (`tests/fanout/README.md`, "Client colos"). The open question is the leader's 1 to 3 s tail, not the rule (`decisions/DECISIONS.md`, "`late-suspect`").

**B0-35 with Actions dispatch on `graft-m4a` (2026-11-17, `tests/m4/results/m4-wp4.md`, "B0-35 with Actions dispatch on graft-m4a").** Run `mwl642gm`, repository 65567, main `c2232435` (`pr-trigger` and `b035-base-tip`) after site days 50 and 51, 500 open PRs, 600 s, 8 learners. **Fails on dispatch p99: 680 s** from push acknowledgment to run (p50 355 s; site log, entry to `RunCreated`, p99 725 s), against 10 s, met at 11 s, failed over 30 s. List-field computes 0 (passes). Test merges about 0.069 vCPU of 1 (1,727 computed, 484 shed late). The trigger did what the design says: 1,195 runs at `refs/pull/N/merge`, 203 superseded events run at the current merge, 0 conflicts, 0 events left `unmerged` (8.6% on a first try, all asked again by a rerun round; F84). The bound is the repository group under the 500-agent load: 234 of 2,490 agent pushes acknowledged, `demand`'s reads and commits mostly "no member answered", and the backlog's runs landed after the load stopped. Setup's 446 `opened` events, before the load, took p99 39 s. B0-35 moves with LM-1's commit work. Cost $2.41 on graft-m4a with both site days, then $0.30 idle in 30 minutes.

**B0-35 rerun on `graft-m4a` after site day 54 (2026-11-19, `tests/m4/results/m4-wp4.md`, "B0-35 rerun on graft-m4a after site day 54").** Run `mwmusr0e`, repository 65568, main `67483ab9` (`76c3e131`), 500 open PRs, 600 s, 8 learners. **Fails on dispatch p99: 120 s** (p50 45 s) from push acknowledgment to run, 2,471 of 2,471 paired, against 10 s, met at 11 s, failed over 30 s. Test merges 0.54 vCPU of 1 (passes; 5,294 computed, 1,372 shed late). List-field computes 0 of 50,516 asks (passes). Agent pushes 2,471 of 2,500 acknowledged. The tail is `ActionsTrigger`'s rounds: an entry waits p50 26 s and p99 97 s for the round in flight to finish its slowest event, then p50 15 s inside its own. The next step is code-only: stream events instead of rounds, and serve dispatch merges first in the shard's turn. Six leader changes, terms 3 to 5 in a `/suspect` burst at 01:20 to 01:22Z. Cost $4.39 on graft-m4a with the site day. **Built code-only on branch `dispatch-tail` (2026-11-19; `decisions/DECISIONS.md`, "`trigger-stream`"):** `ActionsTrigger` runs as a stream of at most `IN_FLIGHT_MAX` items, with `settle`'s cursor rule; the merge turn serves dispatch first, at most `DISPATCH_STREAK_MAX` in a row while another merge waits; each dispatch log line splits its time into wait, demand, reads, and create. Plants 99 to 105 caught. B0-35's rerun waits on a site day for it and on LM-1's B0-33 work on the shed reads behind m54's `suspect` burst.

**B0-35 on graft-m4a after test-faults site day 58 (2026-11-19, `tests/m4/results/m4-wp4.md`, "B0-35 on a test-faults build after site day 58").** Main `f50ad36b` with `disk-first-reads`, 500 open PRs, 600 s. **Passes dispatch: p99 9.1 s** from push acknowledgment to run (p50 3.0 s, 2,498 of 2,498 paired; site p99 7.5 s), against 10 s; m57 was 184.5 s. Test merges 0.62 vCPU of 1 (passes), list-field computes 0 (passes), 0 `warming` 503s, no `exceededMemory`. One leader change in the load, term 4, was the transfer back to `home`; the minute around it had dispatch p99 15.1 s. Built code-only on branch `r29-home-quiet`: that transfer now waits for a quiet leader up to 10 min (`decisions/DECISIONS.md`, "`home-quiet`"), and the lookup skips a superseded bootstrap name and keeps a leader two followers name (`lookup-keep`).

**B0-35 on graft-m4a after test-faults site day 59 (2026-11-20, `tests/m4/results/m4-wp4.md`, "B0-35 on a test-faults build after site day 59").** Main `857c4f02` with `home-quiet` and `lookup-keep`. **Fails dispatch at p99 18.6 s** (p50 3.3 s, 2,486 of 2,486 paired), inside the 30 s fail line; through minute 8 p99 was 2.4 to 3.7 s. The tail is the last 30 s of the load, when the home leader raised a `LoopLag` and two `CommitErrors` alarms, all held, with no leader change. Test merges 0.59 vCPU of 1, list-field computes 0, 0 `warming` 503s, leader commit p99 2,829 ms. `lookup-keep`'s walks took 0.9 to 1.9 s (m58: 6.4 and 9.0 s). Cost $4.94.

**B0-35 on graft-m4a after test-faults site day 60 (2026-11-20, `tests/m4/results/m4-wp4.md`, "B0-35 on a test-faults build after site day 60").** Main `96a91d58` with `catchup-local`. **Fails dispatch at p99 26.2 s** (p50 3.0 s, 2,485 of 2,485 paired), inside the 30 s fail line; to 05:23:20Z the site's p99 was 5.4 s. The tail is the load's last 30 s on site a: the isolate's posts to its cached leader f0 threw within milliseconds (125), a chain of lookup-mark walks followed, and f0's `CommitErrors` trip moved the leader (term 3); term 4 was the transfer home after the load. Test merges 0.60 vCPU of 1, list-field computes 0, 15 `warming` 503s, leader commit p99 1,690 ms, 0 isolate errors, no shared isolate. Cost $5.43.

**B0-35 on graft-m4a after test-faults site day 61 (2026-11-20, `tests/m4/results/m4-wp4.md`, "B0-35 on a test-faults build after site day 61").** Main `9d6fdb71` with `step-agree` and `alarm-steps`. **Fails dispatch at p99 90.2 s** (p50 10.3 s, 2,285 of 2,285 paired), past the 30 s fail line. Leader commit p99 10.9 s, almost all of it queueing at about 0.6 vCPU on f0, half of it learner read-index calls; 21% of those were `HEDGE` copies and resends (`read-newest`). Term 3 was a transfer after `step-agree`'s hold lapsed, term 4 the transfer home. Cost $4.68.

**B0-35 on graft-m4a after test-faults site day 62 (2026-11-20, `tests/m4/results/m4-wp4.md`, "B0-35 on a test-faults build after site day 62").** Main `99f614ee` with `read-newest`. **Fails dispatch at p99 38.6 s** (p50 5.5 s, 2,415 of 2,423 paired; m61 90.2 s), past the 30 s fail line. Leader commit p99 18.0 s, queueing behind one byte-bound entry a round at 0.49 vCPU mean on f0; `HEDGE` copies fell from 15.3% to 4.1% of learner read-index calls. 0 leader changes: three step-agree holds, no step-down. Cost $4.41. The end-of-load `/commit` storm at f0 was site a's git shard re-posting its on-demand test merges (`commit_as`, step `Leader`), not edge receipt re-sends. Next: `soft-grow` (entries fill to 150 KB while more than two entries' worth of Human bytes waits); the m62 replay puts the queue p99 at 5.6 s against 18.0 s.

**B0-35 on graft-m4a after test-faults site day 63: passed on the median of three runs (2026-11-20, `tests/m4/results/m4-wp4.md`, "B0-35 on a test-faults build after site day 63").** Main `63e82533` with `soft-grow`, one site day, rows m63, m64 and m65 with no redeploy, judged as FRAME's operator decision of 2026-11-20 sets: three runs, the median run meets the line, and no run passes its fail line. **Actions dispatch p99, driver: 6.6, 9.9 and 7.2 s, median 7.2 s** against 10 s (fail 30 s). Test-merge vCPU on the leader's shard 0.58, 0.58 and 0.61 of its 1 vCPU budget, and 0 computes from list fields in each. Leader commit p99 1.5, 2.7 and 1.1 s; f0 mean 0.31 to 0.48 vCPU; 0 leader changes in any load window. Both m64 and m65 completed a step-down transfer and the transfer home during the driver's post-load run cancellation, outside the gate window. `soft-grow`'s flip is not tripped across the three rows (pooled grown confirm p99 434 ms against m62's flat 966 ms; grown `sync` p99 lower), so it stays as built; m64's 8 grown far-voter calls past `CALL_DEADLINE` would reopen it if a later row repeats them. The `/commit` storm did not recur. Cost $4.03, $5.08 and $5.31 on graft-m4a. Also on this day: the M2 canary promoted `graft-runners` `13193b05` to 100% after a 50% burst put 3 of 8 jobs on RunnerBox, all Success.

**B0-31 on graft-m4a after test-faults site day 64 (2026-11-20, `tests/m4/results/m4-wp4.md`, "B0-31 on graft-m4a after test-faults site day 64").** Main `c60051c9`, the first B0-31 on `soft-grow`, `command-rows` and `catchup-pieces`. Repo 65580 rehomed on apac, run `mwph4tu0`, driven by `tests/m4/wp4/b031.sh`. **Between the lines on its buckets-up half:** commit p99 2,590 ms against 1 s and the rehomed group's 2 s, under the 3 s fail line. Round 5 measured 1,163 ms; m3fr16, the row's met run, 1,834 ms. The placement-failing half was not measured, because the product edge ignores `x-graft-place-fail`. The lead grants a test-faults edge day plus a B0-31 row once LM-1's step-down change merges. One leader change: a `CommitErrors` trip at 3 failed calls of 26 per follower, on a group still committing, recorded as LM-1's case. The first half's `queue:slot` tail (p99 2,109 ms against round p99 684) is the install `Authz` read (`authz:1`), which every App command's entry makes. In 6 entries it stalled over 1 s, behind its leader's 3.8 s and 2.5 s alarms. Two changes are proposed and not built (`decisions/OPEN-QUESTIONS.md`, F109). Spend $2.05 on graft-m4a, $0 runner.

**B0-31 on graft-m4a after test-faults edge day 29 and site day 65 (2026-11-21, `tests/m4/results/m4-wp4.md`, "B0-31 on graft-m4a after test-faults edge day 29 and site day 65").** Main `81f241e7`, with `step-busy` and `dep-hedge-other`; the first B0-31 on graft-m4a with both halves measured. Repo 65581 rehomed on apac, run `mwpn8ro9`. **Between the lines on both halves:** buckets-up commit p99 1,947 ms, under the rehomed group's 2 s (m64r31 2,590), and placement-failing 2,185 ms, both against 1 s and under the 3 s fail line. 0 leader changes, one `LoopLag` hold, no `CommitErrors` trip, 0 `TransferAbandoned`, 0 `suspect`. The hedge to another member won 7 of 354; with B0-29 m3fb29's 0 of 844 its flip is met, and the records recommend going back to the same-leader hedge. The largest dep read is the install `Authz` read (`authz:1`, p50 565, p99 1,210 ms), 277 ms over the owner's read at the median for one more `Names` hop. Keeping the install account's record on the member is proposed: modelled commit p99 about 1.6 s, not under 1 s. Spend $1.75 on graft-m4a, $0 runner.

**B0-31 on graft-m4a after test-faults site day 66 (2026-11-21, `tests/m4/results/m4-wp4.md`, "B0-31 on graft-m4a after test-faults site day 66").** Main `8c59344e`, the first B0-31 on `install-record`, with `dep-hedge-other` reversed. Repo 65582 rehomed on apac, run `mwprrhgo`. **The buckets-up half meets the rehomed group's 2 s at 1,408 ms (m65r31 1,947); the placement-failing half does not, at 2,396 ms (m65r31 2,185). Both are between the 1 s line and the 3 s fail line, so B0-31 is not yet recorded as meeting the rehomed bound** (lead, 2026-11-21). The install `Authz` read now matches the owner's (same-entry difference +2 ms at p50, against +277 on m65r31), so `install-record`'s flip did not trip. The same-leader hedge won 49 of 423. 0 leader changes, 0 step-down and 0 `TransferAbandoned` lines. The 19 failed leader calls are all calls past CALL_DEADLINE. The failing half's tail is not placement: no slot or retry is spent on a refused push. It grows with the row's age, as m65r31's did, through the leader's AppendEntries leaving late (forward-leg excess p99 493 → 1,515 ms over the row) and its CPU per request tripling. Three long-call windows set most of it, one of them the cause of 212 edge `suspect` reports. Proposed, not built: hedge the leader's voter AppendEntries as learners' are. Spend $1.82 on graft-m4a, $0 runner.

**B0-31 on graft-m4a after test-faults site day 67 (2026-11-21, `tests/m4/results/m4-wp4.md`, "B0-31 on graft-m4a after test-faults site day 67").** Main `b67d6d22`, the first B0-31 on `round-memo`, run under the standing row queue. Repo 65583 rehomed on apac, run `mwq34twn`. **Both halves meet the rehomed group's 2 s: buckets up 1,595 ms, placement failing 1,115 ms (m66r31 1,408 and 2,396), with 0 leader changes, so B0-31 is re-confirmed as meeting the rehomed bound** (lead, 2026-11-21). The leader's rows read per minute plateaued at 306k to 403k against m66r31's climb to 2.09M, and its CPU stayed flat at 10.0 to 11.3 CPU-s a minute. `install-record`'s same-entry gap was 0 ms at p50 in both halves. `reretire.sh graft-m4a.txt` printed RERETIRE OK after the site day. Spend $1.84 on graft-m4a, $0.00 runner.

**L0-12's A1 replay: met at 90% on m3fr6 (2026-11-16, under FRAME's 90% rule).** Repository 65567 on `graft-m3f`, run `mwi4cbu9`, `lm1-rows-m3f6` `520020fe` on main `a304d3fc` (`tests/am1/results/lm1-rows-m3f.md`, "Rows m3fr6", L0-12). The lines, from that run: 0 Exclusive overlaps over `DRIFT` (111 samples, max −11,330 ms); 20 of 20 leases live after a 1,801 s stall, `expires` moved 1,772 to 1,773 s; the late Exclusive acquire 201 with 0 conflicts; feed p99 outside leader kills 2,954 ms against 3 s; refusals 100 of 100 for both `ActorKind::Agent` kinds and 0 of 100 for the other three. The clock line is judged on the spread between members, the quantity `DRIFT` bounds, since leases and the overlap check read member clocks only (`design/DESIGN.md`, Shape, `DRIFT`, and "Leases read the member clock"), and the row's fail column has no clock line: 56 to 512 ms in the replay (`lowerMs` / `upperMs`), at most 324 and 228 ms at rest, within 550 ms (500 ms plus 10%) [measured]. Operator's decision, 2026-11-16. No invariant failed. Gap: the members' 972 ms offset from Cloudflare's clock in the replay, recorded under F3 (`decisions/OPEN-QUESTIONS.md`). No rerun is planned for this row. Re-judged the same day under the rule, still short: B0-29 (best gate run m27, commit p99 774 ms against 660), B0-31 (m3fr8, buckets-up commit p99 2,138 ms against 1,100), and B0-32 to B0-35 far short; zero targets (leader changes, `suspect`) stay exact.

What LM-1 still needs before it closes (2026-11-11, after `lm1-memory`'s round 5). `am1-rest` built the pieces in memory (`tests/am1/results/am1-rest.md`, 26 of 26 planted bugs; no record format, so 32.15 and 0x12.9 stay free), and `am1-runs` ran them hosted (`tests/am1/results/am1-runs.md`, 3 of 3 planted bugs). Passed and closed: L0-12's refusal counts, L0-13, B0-36, B0-37's push half, LF-13 (on `graft-m3c`, `tests/am1/results/lf13-m3c.md`), and, on `lf4-rest` (`tests/am1/results/lf4-rest.md`, 2026-11-12), LF-4 on all eight resources and B0-36's GraphQL label surface. Left:
- Leaders now hold under the read load: learners, then followers, take API tier reads, the read gate's rows, and the owner's `Authz` fold and `o/r` rows by read-index, and a read they shed answers 503 with `Retry-After` and never reaches the leader. `Authz` is no longer the bound: with it off the leader, no part of a commit dominates (round 5). The validate lock (`fanout`) and the gate's installation-token reads (`04487ca`) were fixed before. Left on branch `lm1-memory` (F65), round 6 next:
  - Groups left with no leader until a test-only `/campaign`, which production does not have (F67). Seven causes fixed in `crates/raft` on `f67-liveness` with the 30 s liveness bound in the sweep (`tests/m0/results/f67.md`); the term-2 case after B0-35's setup is still open.
  - `CommitErrors` step-downs and slow dep reads under L0-12's leader kills (`Authz` dep reads p50 2,246 ms on the last leadership), and B0-33 at 5,000 agents, not run in round 5.
  - L0-12's lease overlap, which the driver cannot show with Shared leases only; round 6 adds Exclusive leases.
  - B0-32's burst: refused rather than answered `admission full`, and acknowledged past 5 s.
  - B0-34's human push p99 (121 s).
  - **B0-35: passed on 2026-11-20** on the median of three runs on graft-m4a (m63, m64 and m65: dispatch p99 median 7.2 s against 10 s, test merges within budget, 0 list-field computes; "B0-35 on graft-m4a after test-faults site day 63" above). History of this item: Actions for B0-35's dispatch half. Its budget, instruments, overrun fix (A28), GET merges (A30) and list-field count (A29) are built (A27 to A30 closed; AM1-WP1, "Follow-up: the test-merge budget and B0-35's instruments"). The `pull_request` trigger B0-35's dispatch line needs is built code-only on branch `b035-dispatch` (2026-11-17; `decisions/DECISIONS.md`, "`pr-trigger`"): `ActionsTrigger` starts a run at each event's current test merge, behind `GRAFTYARD_PR_TRIGGER`, and `tests/am1/rest/a1.mjs --workflow` measures dispatch from each head push's acknowledgment to its run on graft-m4a. Run on graft-m4a after site days 50 and 51 (2026-11-17): dispatch p99 680 s, a fail; see "B0-35 with Actions dispatch on `graft-m4a`" below.
  - Whole-tier API reads on learners: fixed on branch `learner-dumps` (K9 closed), 94.7% of `read:api` answered at 500 agents on `graft-m3c` against 25.7% before, at p99 5.0 s; the 500-agent rows rerun it on `graft-m2b` (`tests/am1/results/learner-dumps.md`). Branch `narrow-reads` answers those reads from the rows each reply renders instead of the 1 MB dump (`tests/am1/results/narrow-reads.md`).
- Nothing that needs a live Graftyard-run task: L0-12's lease refusal for that kind, B0-37's session-ref listing, and B0-36's lease refusal and no-live-task and cancelled-task fork pushes for that kind are AM3-WP3's, by the operator's decision of 2026-11-10; every other part of the three rows passed hosted.
- B0-30 as written, with pushes on for the full 30 days: owned by thread "B0-30: MetaSplit child group, archive stall, full 30-day rerun" on branch `b030-split`, which builds the metadata child group (G6), fixes the archive stall (a promotion's validate stays on the first shard; no forwarding to a shard with no cursor for its disk; an archive run covers a promotion; G7), and reruns the full 30 days. Thread "hot-shards" builds the push half (AM1-WP1, "Follow-up: git cache shards on a hot group with a multi-GB log"). The operator's direction of 2026-11-11 is that every row passes as written, with no substitutions, so the 2026-11-10 substitution (metadata half only, push half replaced) is withdrawn.
- **B0-31, B0-32 and L0-12 re-judged at 90% (2026-11-16, `FRAME.md`, "Operator decision, 2026-11-16, 90% gates and a daily spend limit"; no new spend; `tests/am1/results/lm1-rows-m3f.md`, "Re-judged at 90%").** None passes, so no row status changes and every one still needs a rerun. B0-31: placement-failing half and refusal shape pass, but buckets-up commit p99 is 1,118 ms at best (m3fr2), 1.6% past the 1.1 s allowance, and 9,600 ms on m3fr9. B0-32: 0 of 500 burst pushes within 5 s on every run (391 to 500 acknowledged at p50 13.5 to 46 s), against 450 needed; leader changes and `suspect` are zero-count lines with no 90% reading, and m3fr9 had 17 elections and 8,564 `suspect`. L0-12: feed p99 2,954 ms passed outright on m3fr6, but that run's member clock line read 972 ms against a 550 ms allowance; m3fr9's clock passed (297 ms against Cloudflare, members 102 to 339 ms apart) but its feed p99 was 25,210 ms, and its stall and refusals did not run. No single run passes every line at 90%.
- The j4, lf1-2, and lf1-3 groups on `graft-m1` were recreated, empty, under the same ids (`tests/m3/results/wire-fix.md`); any `graft-m1` rerun of rows that used them needs their seed data again.

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

**Built so far (2026-11-10), the pure half** (`tests/m4/results/m4-wp1.md`; branch `m4-wp1`, merged at `98bbdfc`). In `crates/git`: `HookRecord`, `HookCursor`, the `hook` and `hook_cursor` core rows, `IssueOp::PutHook` and `DeleteHook` (admin only; an `Agent` refused; at most 20 hooks a repository), `HookCursorCmd`, `EVENTS` and `SUBSCRIPTIONS`, `delivery_guid` (a UUIDv8 over restore generation, chain, index, slot, and hook), and the `HookSequencer`'s rows and logic in `sequencer.rs`: one queue per (chain, hook), one attempt in flight per queue, round robin under `HOOK_OWNER_CONCURRENCY` (50) with failing hooks at most 25, one automatic attempt, the `retry` opt-in, and `replay_from`. The consumer's pure half (`plan`, `consumer_start`, `cursor_to_commit`), payloads checked against GHES 3.22's `x-webhooks` (and dotcom's description for `merge_group` and the queue events), GHES's request headers and `X-Hub-Signature-256`, the REST hook routes, and `graft-ops` `POST /wrap` (not deployed). On three real members per repository: **BF-57 passes** (healthy first-attempt p99 700 ms over 9,849 deliveries, the slow hook's 201 deliveries in chain order, cursor lag at most 1,000 ms, at most 50 in flight); **BF-58 passes** (after the sequencer's site is killed, 465 of 465 (event, hook) pairs delivered, 117 more than once, all under unchanged GUIDs). 34 of 34 planted bugs caught; 628 tests passed. Formats: repository 32.16 (`REPO_FORMAT_HOOKS`); no Authz format, so 0x12.10 stays free. Wire tags on main: `RepoEvent` 40 `Hook` and 41 `HookCursor`, after `fanout`'s 39 `Fanout`, which was deployed on `graft-m2b` first (on the branch they were 39 and 40); `IssueOp` 38 `PutHook` and 39 `DeleteHook`; new enums `HookContentType` and `sequencer::Outcome` (`decisions/DECISIONS.md`, "M4-WP1's pure half"). Contradictions with SHARED, for its owner to settle when the hosted half lands: `HookDelivery.guid` is a derived UUIDv8 and `entry` is optional for pings; `RepoEvent::Hook` carries `created: bool` and `actor: Option<AccountId>`; `merge_group` and the queue payloads have no GHES 3.22 oracle; and the sequencer, not the delivery Workers, picks each attempt. Not built: the `Audit` and `Search` consumers, Basin Pipelines, the `HookSequencer` Durable Object class, delivery Workers and `graft-ops`'s unwrap route, the Secrets Store bindings for `SecretsWrapKey`, the edge's sequencer routes (503 until then), and the site day enabling 32.16. Then a hosted BF-57 and BF-58.

**Built so far (2026-11-14), the hosted half** (`tests/m4/results/m4-wp1-hosted.md`; branch `m4-actions`, on `graft-m4a`, site days 3 to 25 in `tests/am1/results/install-m4a.md`). The `HookSequencer` class on each site script (one store per hook owner on its `home` site, falling back to the record's next site and retrying home every 30 s), the leader-side `Webhooks` consumer on the M2-WP5 pipeline committing `HookCursor`, the edge's hook and delivery routes with `retry`, and delivery through per-hook lanes: keyless `hooklane:<wire>:<hook>` objects of the same class, each POSTing its queue's run in order (`decisions/DECISIONS.md`, "M4-WP1's hosted half: webhook attempts run in per-hook lanes"). **BF-57 passes** (run 16: healthy first-attempt p99 1.07 s over 9,800 deliveries, 0 chain-order inversions on all 50 hooks by GUID, cursor lag at most 4.8 s). **BF-58 passes** (run 2, site a killed through two production site days: 12,325 of 12,325 (event, hook) pairs delivered, 0 under two GUIDs; site b's store served 0.19 s after site a's store went down; first redelivery 0.95 s after the restore). The `Audit` consumer sends each scanned identity entry's rows to a Basin Pipelines stream before it moves its cursor, and a new SSH key's row reached the R2 sink in 144 s. Search is M4-WP2's. A running Durable Object keeps its old version until it restarts, measured at up to 5.5 minutes after a site day, so a kill is timed from the object's own first call on the new version (`tests/m4/wp1/bf58-kill.sh dead`).

### M4-WP2. User records, the watcher index, the `Inbox`, and search

**Build.** `UserRecord` as `IdentityEvent::Record`, the `Counts` consumer and the watcher index it writes to each repository's metadata tier (`watcher` and `thread_sub` rows), which `/stargazers`, `/subscribers`, and `Notify` read, the `Notify` consumer, and `Inbox { account, site }`, one derived object per account, with its daily snapshot, its `sources` table, and the rebuild rule with `INBOX_WINDOW` (`design/DESIGN.md`, Shape, "Identity"). `SearchShard` and `Domain`: FTS5 tables in 16 shards per residency domain, on the domain's first Full site under its DO jurisdiction, rebuilt under the next `generation` from the metadata tiers that `MemberRegistry` lists, with `incomplete_results` past the `rest:search` deadline (Shape, "Identity"; `design/SHARED.md`, `rest:search` and "Public service concerns", Residency). The `rest:activity` surface at T1 (`design/SHARED.md`, "Surfaces, tiers, and oracles").

**Depends on.** M4-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-5 | Star a repository and comment on a watched issue, then delete and rebuild the watcher's `Inbox` | `stargazers_count` and `/notifications` show both within 10 s, and the rebuilt `Inbox` keeps read state | Lag over 60 s, or read state lost |
| BF-67 | Delete an `Inbox` and rebuild it. Also 10,000 stars, then a refold | Read and done state kept, nothing from `sources` lost. `/stargazers` lists all 10,000 | Any lost read or done state, a lost event from a `sources` chain, or a missing star |
| BF-68 | EU-resident org's search documents. Kill one `SearchShard` | Documents only in the `eu`-jurisdiction shards. `incomplete_results` until the rebuild, then full results | A document outside EU, a partial result without `incomplete_results`, or results wrong after the rebuild |

BF-68 assumes FTS5 inside Durable Object SQLite, which a hosted probe confirmed on 2026-11-13, under the `eu` jurisdiction too (`decisions/OPEN-QUESTIONS.md`, C15; `tests/m4/c15/`).

**Built so far (2026-11-13), the pure half** (`tests/m4/results/m4-wp2.md`; branch `m4-wp2`). In `crates/identity`:
- `UserRecord` as `IdentityEvent::Record`, through `IdentityOp::SetRecord`, which only the account sends and which emits nothing on a repeat.
- The `Counts` cursor, operator only and forward only.
- `ThreadId` and the user's rows in the fold's trailing part (format 0x14.3).

In `crates/git`:
- The watcher index (`watcher`, `stargazer`, `subscriber`, `thread_sub`, `watch_count`), written by `CountsCmd` from whole-state updates that a row never takes at an older `src`, and `counts_plan`.
- `ConsumerCursorCmd` for `Notify` and `Search` (format 32.19).
- `notices`, `Inbox` with `sources`, the daily snapshot, and `rebuild_from` with `INBOX_WINDOW`.
- `Domain`, `SearchShard` placement, the FTS5 `DDL`, documents, the query parser and SQL, and `merge` with `incomplete`.

In `crates/api`: the `rest:activity` and `GET /search/issues` renderers; 8 replies are valid against GHES 3.22.

On real members and real SQLite FTS5:
- **BF-5 passes**: star and notice both after 1,400 ms; the rebuilt Inbox keeps read state.
- **BF-67 passes**: both rebuilds keep read and done state and every `sources` event; 10,000 stars are listed live and after a refold.
- **BF-68 passes**: EU documents only in `eu` shards; `incomplete_results` with a shard killed; full results after the generation-2 rebuild.

30 of 30 planted bugs caught. Wire tags: `RepoEvent` 42 `Watchers`; `IdentityEvent` 6 and 7; `IdentityOp` 8 and 9 (`decisions/DECISIONS.md`, "M4-WP2's pure half").

Not built: the `Inbox` and `SearchShard` site classes and their migration, the consumers' Queue wiring, the edge routes behind `FORMATS_ENABLED` 32.19 and 0x14.3, and the site day. Then a hosted BF-5, BF-67, and BF-68, estimated at about $8.

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

**Built so far (2026-11-18), slice A: the launch service.** `system.github.launch_endpoint` in the job message and the edge's `runnerresolve` route, which resolves each `uses:` on the install alone (`crates/actions/src/launch.rs`; `decisions/DECISIONS.md`, "M4-WP3 slice A: the launch service"). Before it, an unmodified runner failed every job with a `uses:` step. In memory the runner now runs actions by tag, branch and full SHA, and gets a 422 at once for an abbreviated SHA, a missing version, or a repository that is missing or private (`tests/m4/results/m4-wp3-launch.md`). It has no gate row and no hosted run yet. Private actions are F90. Slice B, `GITHUB_TOKEN` minting, is built on branch `gh-token` (2026-11-18): each job gets an installation token of the install-owned Actions App on its owner, installed at the first `acquirejob` with no new dep for `decide`, for its repository alone, with GHES's permissions and fork rule, living the job limit plus a lease (24 h at most), and denied by `jti` when the job finishes, is cancelled, its repository is deleted, or its lease expires (`decisions/DECISIONS.md`, "`gh-token`"; `tests/m4/results/m4-wp3.md`, slice B). Formats Actions 0x15.5 and Authz 0x12.14. It needs a readers site day, `FormatsEnable`, then `GRAFTYARD_JOB_TOKENS=1`; no hosted run yet.

### M4-WP4. Hosted runners on Sandboxes

**Build.** `graft-runners`, a Worker in the runner account that holds `HostedRunnersAppKey` and reaches the install only through the front (`design/DESIGN.md`, Shape, "Identity"; Stack, Workers, Sandbox SDK, and Cron Triggers). As the `graftyard-hosted-runners` App it reads `GET /api/graftyard/v1/owners/{owner}/actions/hosted/demand` (`Broker::demand`, `HostedDemand`) on each `workflow_job` webhook and on a 5-minute sweep. For each `start` entry it calls `generate-jitconfig` and starts one Sandbox per runner (`standard-3`, or `standard-4` for a 4-core label), destroys it on exit or `stale`, and starts none past `RUNNER_VCPU_STOP_PCT`. The hosted label table and the per-owner cap that `decide` enforces on hosted registrations (`design/SHARED.md`, "Actions", Hosted runners). The runner image with unmodified `actions/runner`, one job per Sandbox. CLI: `graftyard runners init` and `graftyard runners upgrade` ("Failure modes", `actions state`).

**Depends on.** M4-WP3, M4-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-60 | 50 `ubuntu-latest` jobs queued for one owner with the cap at 20, while two `graft-runners` sweeps overlap | At most 20 hosted runners at once; every job finishes; no Sandbox is left 10 minutes after the last job | More than 20 at once, a job that never finishes, or a leaked Sandbox |

It also carries BM-4's matrix run.

**Hosted rows (2026-11-15, graft-m4a and the runner account, $12.68 of the $25 stop)** (`tests/m4/results/m4-wp4.md`):

- **BF-60 passes**: run 1966182, on the sweep alone.
- **BM-4 passes**: run 1900546, with site a down mid-run.
- **BF-54 fails on job start.** The 503s, re-polls, and sessions pass, which settles F26. But a `Repo`-scope match whose answer was lost waits out its lease (`decisions/OPEN-QUESTIONS.md`, F76), so the row stays open until F76's fix and a rerun.
- **BF-54 run 2 (2026-11-16, site day 46, main `cab0b332`, $15.67 of the $25 stop) still fails on job start, for a new reason.** F76's fix holds: no lost match and no broker `exceededMemory`. But acquires fail. The edge's secrets read before `acquirejob` answered 503 74 times, and one acquired job never ran. So job start p99 is 39.0 s and one job started at 62 s (`decisions/OPEN-QUESTIONS.md`, F77).
- **BF-54 passes on run 3 (2026-11-16, site day 47 and edge day 21, main `1d90863a`, $18.32 of the $25 stop).** With F77's fix, all 512 jobs succeeded, job start p99 was 16.6 s after the run committed (gate 35 s), max 19.2 s, and every `acquirejob` answered 200 with 0 `job_config` failures (run 2: 74). Sessions and re-polls pass as before (`tests/m4/results/m4-wp4.md`, "BF-54 run 3"; `decisions/OPEN-QUESTIONS.md`, F77).
- Queue latency on the sweep alone is about 10 minutes at p50, which is R24's gap.
- **R24's delivery run (2026-11-16, site day 48 and edge day 22, main `77c9fa23`) breaks an invariant, so BF-60 is reopened.** Run 1966233 had 24 of 24 runners started by `workflow_job` deliveries and 0 by sweeps, and queue-to-start p50 of 261 s (sweep alone: 588 s). But 49 of 50 jobs succeeded. A stale delete removed runner 1704433 and destroyed its Sandbox 3 s after it acquired job 1966235, which then failed at its lease end. The delete is checked only against `OWNER_SCOPE_SHARD`'s jobs, and the run's repository is on shard 5 (`decisions/OPEN-QUESTIONS.md`, F78). The 90% rule (`FRAME.md`, "Operator decision, 2026-11-16, 90% gates and a daily spend limit") does not apply: a runner deleted while it holds a job fails the row outright. Run 1966182's pass used the same repository and shard, so the guard was already wrong there and did not fire. BF-60 stays open until F78's fix and one rerun. The fix is on main (`f78-hosted-delete`, `decisions/DECISIONS.md`).
- **BF-60 is met at 90% on its rerun after F78 (2026-11-17, site day 49 and edge day 23, main `0de37416`).** Run 1966284: at most 20 registrations and 19 Sandboxes at once, every job finished, nothing left 10 minutes after the last job, 30 runners started by deliveries and 0 by sweeps, queue-to-start p50 216 s. No runner was deleted while it held a job: the 3 stale deletes came after their runners' jobs completed. 49 of 50 jobs succeeded (98%, above the 90% line). Job 1966290 failed because its acquire answered an empty repository name, a separate bug recorded as F79 (`decisions/OPEN-QUESTIONS.md`). Cost $0.58 by `tests/fanout/acctcost.py` (graft-m4a $0.24, runner account $0.34), against a $2.50 estimate.
- **BF-60 passes outright on F80's cold-burst check (2026-11-17, `graft-runners` from main `a73871ce`).** Run 1966335, after the Sandbox class sat idle 8 hours: 50 of 50 jobs succeeded, at most 20 registrations and 20 Sandboxes at once, nothing left 10 minutes after the last job, and no runner deleted while it held a job. No delivery pass was cut at 30 s, and every registered runner opened a session. Queue-to-start p50 186 s. Cost $0.59 by `tests/fanout/acctcost.py` (graft-m4a $0.25, runner account $0.34) (`tests/m4/results/m4-wp4.md`, "F80 cold-burst check on the alarm start").
- **Re-judged at 90% (2026-11-16).** BM-4 and BF-54 (run 3) met every target outright, so the rule changes neither. Run 2's 39.0 s p99 was 11.4% over its 35 s bound, past the 10% allowance, and its job at 62 s crossed the 60 s fail line, so run 2 would not have passed at 90% either. Run 3 supersedes it. BF-60 is the one open row, and it is open on an invariant, not on a count.
- **Repository purge route, hosted check (2026-11-18, site day 52 and edge day 25, main `703a9642`): stopped.** The gate refused 27 of the 35 leftover chains on graft-m4a with `NoTombstone`, on a superseded site-1 voter the route always asks. That run deleted and purged nothing. Then the 8 clean live repositories were deleted and purged with both passes: each is not joined and lists 0 objects. After `purge-outside` (R27; site day 53 and edge day 26, main `94fdfb48`), the other 27 were purged the same way, with no refusal past a lost reply or timing. Only `repo:0` and j4 stay joined (`tests/m4/results/m4-wp4.md`, "Repository purge route, hosted check on graft-m4a"; `decisions/OPEN-QUESTIONS.md`, R27 and R29).

**Built so far (2026-11-15)**. The install side: the hosted label table, `decide`'s per-owner cap on hosted registrations and its refusal to delete a hosted runner that holds a job (`crates/actions/src/hosted.rs`), `Broker::demand` on the owner's `RunnerBroker` under BF-55's every-shard rule, and the edge's `GET .../hosted/demand`, `POST .../hosted/runners`, and `DELETE .../hosted/runners/{id}`. `graft-runners` in `workers/runners`: one pass (`control.rs`), the Workers host (`worker.rs`, `entry.mjs`), a deploy script, and the runner image (`containers/runner/Dockerfile`, built in the orb; `tests/m4/wp4/image.sh` pushes it). CLI: `graftyard runners init` and `graftyard runners upgrade` (`cli/src/runners.rs`). BF-60's in-memory half passes on three real members with a fake Sandbox host, with runners polling through BF-55's `Taken`, and `tests/m4/wp4/plant.py` catches every plant. The runner account exists, and the operator approved the estimate (about $12 to $19, with a $25 stop).

### M4-WP5. The web app [inference placement]

**Build.** The React 19 web app on TanStack Start in SPA mode, with ReUI's free components and HugeIcons' free icons (`FRAME.md`, "Operator decision, 2026-11-08, web stack"), reading the public GraphQL and REST endpoints with persisted queries plus the private `/_t/ui/*` API for diffs, file trees, and blame. The edge serves the HTML shell with the first route's query result inlined, and the static assets with immutable cache headers. Each page region is its own error boundary with its own query, and the client retries only idempotent reads within a budget (`design/SHARED.md`, "Frontend and native app"). The live log view reads the job's `LogRelay` WebSocket and falls back to step log blobs when the relay has no runner stream (`design/SHARED.md`, "Actions", Logs). `design/DESIGN.md`, "Milestones 1 to 7", places it in Milestone 4 [inference], because its pages read issues and pull requests (M2), sign-in (M3), and live Actions logs (M4-WP3).

**Depends on.** M2-WP4, M3-WP3, M4-WP3.

**Started early on a mock (2026-11-08).** The pages that need no M4 backend start now in `web/`, against a browser mock that answers the pinned GHES 3.22 REST description with the same generated types the client uses (`decisions/DECISIONS.md`, "The web app is TanStack Start with ReUI and HugeIcons, started on a mocked API (2026-11-08)"). The mock carries no gate row: BF-38 and BF-61 count only runs against a hosted edge. By the end of that day the mock covered every GHES surface the app has a page for, plus the `/api/graftyard/v1` agent API, each area of it owning its pages and mock handlers (`web/README.md`, "Layout"). Most routes those pages read were not served by the edge then; the chrome's last unserved reads (branches, tags, labels, assignees, teams, check suites, reactions) are served since `f61-reads` (`decisions/OPEN-QUESTIONS.md`, F61, closed), the pages also read private `/_t/ui/*` routes the design does not name (F62), and the live log view runs against a mocked `LogRelay` socket until M4-WP3.

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

**Passed (2026-11-13)** (`tests/m5/results/m5-wp1.md`). BF-6 refused 154 of 154 in a replay test. BF-41 passed on a throwaway product install: the members themselves refused another key's genesis. BF-40, BF-42, and BF-45 passed on graft-m4a's `Release` group after a site day. 17 of 17 plants were caught. The edge checks only decoding and genesis until M5-WP2's cached view, and `decide` runs every other rule.

### M5-WP2. `graft-front`, pin-only pointers, and gradual edge deploys

**Build.** `graft-front`, `Pointers` (each `Option<VersionId>`, `None` following the deployment split and `Some` set only by a pre-signed `EdgePointer`, `OpsPointer`, `McpPointer`, or `ControllerPointer` undo, cleared by the next intent that rolls the same script), `POINTER_TTL`, and the whole `Release` view cached per isolate, members included, refreshed from any member's applied state and never moved backward. A `StaleMode` that cannot commit is written to every reachable member's `pending` row and verified against the cached `Keys`. `fetch()`-based service bindings with version overrides, N and N+1 in each edge deployment, the edge's version report and the mismatch alarm, `Intent::EdgeRoll`, and `Intent::FrontRoll` (`design/DESIGN.md`, Shape, "Release group" and "The read path"; "Commit and read paths", journey 7).

**Depends on.** M5-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-8 | Blackhole every `Release` member for 10 minutes, then set `EdgePointer` to a version outside the deployment | Requests keep the last pointers; the mismatch alarms within `POINTER_TTL` and the `EdgeRoll` that dropped it was refused | Any request blocked on the group, or a silent mismatch |
| BF-43 | `EdgePointer` undo under 1% `EdgeRoll` with the API blocked, then the next `EdgeRoll` | Every request pins to N within `POINTER_TTL`; the next `EdgeRoll` sets `None` | Any request off N while pinned, or the pin survives |
| BF-48 | Blackhole the `Release` group, then send a customer-signed `StaleMode` | Scoped groups serve `stale()` within `POINTER_TTL`; the flag expires at `until` | No stale reads, or a flag past `STALE_MAX` honored |

**Passed (2026-11-14)** (`tests/m5/results/m5-wp2.md`). BF-43 and BF-48 passed on graft-m3c after site day 3. BF-8 failed in run 2 on isolates started during the blackhole, which held no pointers (`decisions/OPEN-QUESTIONS.md`, R20). The operator chose a copy of the group's entries outside the group, and BF-8 run 4 passed after site day 4: 200,416 of 200,416 requests pinned through a 601 s blackhole, the refused `EdgeRoll`, and an alarm 331 ms after the pointer left the deployment, none silent. BF-43 also passed again on the final code. 25 of 25 plants were caught.

### M5-WP3. The `graft-ops` executor and record formats

**Build.** `graft-ops` as the executor of signed intents: uploads only from the release bucket after the release signature verifies, then commits `ReleaseUploaded` with the executor key and resolves each intent's `ReleaseId` to `VersionId`s from it. It commits `IntentStep` after each step passes its gate, `SiteRelease`, and `IntentDone`, so a restarted `graft-ops` reruns only the first unrecorded step. The customer's deploy token and read token in Secrets Store, every cron that calls the Cloudflare API, `Intent::OpsRoll`, `Rollback { target, release }`, and `Abort { intent, target }` (`design/DESIGN.md`, Stack; Shape, "Release group"; "Install operations", Tools, Keys, and Release roll). `FormatsEnable` with the release-signing key plus the customer key, and formats gated by `verify` (`design/SHARED.md`, "Record formats and the two-release rule"; `design/DESIGN.md`, Shape, "The write path"). CLI: `graftyard release roll`, which signs one intent at a time.

**Depends on.** M5-WP1, M5-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-7 | After `FormatsEnable`, submit the pre-signed `Rollback` to N and `Abort { intent, target: N }` | Both refused, and both page | Either applied |

**Pure half built (2026-11-22), hosted half pending.** `graftyard_release::ops` decides each `graft-ops` action from the `Release` fold and the deployments; `FormatsEnable` now waits for every site's and lifeboat's report; `graftyard release roll --entries` signs the next intent; the edge writes a `PAGE release` line on a paging refusal. BF-7 passes in memory, and plant82 catches 19 of 19 (`decisions/DECISIONS.md`, "M5-WP3: `graft-ops` executes intents, as built"). The Worker host is built (2026-11-22, not deployed): `ops::run` drives `next` on the 5-minute cron, and `workers/ops/src/exec.rs` reaches the group, the deployments and versions API with the deploy token, the executor and drain keys, and the release bucket; its gate is the `GATE` variable until M5-WP4. On graft-m4a's `Release` group, 0 of 8 committed entries are `FormatsEnable` (2026-11-22). **BF-7 passed hosted (2026-11-22)** on graft-m4a with `graft-ops` executing every roll step: after `FormatsEnable 65000.1` the customer's `Abort` to N and `graft-ops`'s pre-signed `Rollback` to N were both refused `UnreadableFormat` and both paged, with no split moved; 65000.1 stays enabled on that group (`tests/m5/results/m5-wp3.md`). **BF-43 passed rerun with `graft-ops` executing (2026-11-22):** with the API blocked, `graft-ops` committed the pre-signed `EdgePointer` within 5.6 s of the breach run, requests pinned to N 1.2 s later, and 0 of 99,936 were off N or unpinned from +5 s to the next `EdgeRoll`, which cleared the pin in 0.35 s. That needed `dce09ec2`: a gated breach no longer waits on a deployment read.

### M5-WP4. Site days and automatic drain

**Build.** `Intent::SiteDay` with `DayTarget` (a site, the Workflows script, whose day skips drain and undrain, or `Containers(site)`, the undo of a `ContainerDay`) and `Intent::ContainerDay`, whose steps restart shards onto the new image in rendezvous order and whose rollback returns them to `git_prev` with no API call. Site days: drain, deploy at 1%, 10%, 50%, then 100%, and undrain, with the steps of `ROLL_STEPS` and the gate and dwell constants of "Install operations", Site day. Each step and the 1% day gate on the N+1 members' apply lag, apply and verify exceptions, ack rate, and `STEP_DOWN` trips. A new site leads 1% of its home groups for its first day. Spare and residency sites take a site day per release behind a canary group. A breach runs the pre-signed `Rollback` with no human signature ("Install operations", Site day; "Commit and read paths", journey 7). `SiteHealth` objects on each observer site, automatic drain by `graft-ops` with its drain-only key, and undrain by the same key under `UNDRAIN_BURN` for `UNDRAIN_AFTER` (journey 5, "Automatic drain"; Shape, `SiteMode`).

**Depends on.** M5-WP3, M1-WP3.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-39 | Kill `graft-ops` at each step of a `SiteDay` and an `EdgeRoll`, 100 times | It resumes after the last `IntentStep`; no step skipped; one `IntentDone` | A skipped or doubled step |

It also carries BM-5's unreadable format "commits nothing and moves A's groups".

**Pure half built (2026-11-22), hosted half pending.** `graftyard_release::gate` judges each step on the N+1 and N members and calls a site burning or calm from both other sites' `SiteHealth`. `graft-ops` drains and undrains on it with the drain key, preempts a site day under the 1% lead limit, undrains a day stuck for `STUCK_DAY`, pages on a step starved past twice `STEP_DWELL`, and runs `ContainerDay` through the view with no API call. BF-39 passes in memory: 3 seeds with `graft-ops` killed in every run, each step committed once and in order, one `IntentDone` per intent. plant87 catches 29 of 29 (`decisions/DECISIONS.md`, "M5-WP4: site days and automatic drain, as built"). BM-5's in-memory half is `an_older_refuser_makes_the_leader_truncate_and_hand_off` (`crates/raft/tests/leadership.rs`). Left for the hosted half: the members' gate counters, the `SiteHealth` object and its reads in `graft-ops`, and the host steps of `may_not_lead` and `shard_day`; then BF-39 hosted on graft-m4a.

**Hosted half's code built (2026-11-22), hosted rows pending.** Members count each version's gate inputs and `SiteHealth` into the site's `gate` and `site_health` registry objects, `graft-ops` judges a site day on them and drains on `verdict`, members step `may_not_lead` from the view, and shards start the image `shard_day` names (`decisions/DECISIONS.md`, "The hosted half's inputs are counted in the site's registry objects"). plant87 catches 37 of 37. BF-39's hosted row runs a `test-faults` `graft-ops` (`--executor` with `test_faults`): `OPS_KILL` kills each run after 0 to n of its calls, and `OPS_STEP_MIN_ENTRIES` lowers the starvation floor, because a 1% step on graft-m4a cannot reach 1,000 entries (OPEN-QUESTIONS R38). Its drain leg uses B0-07's gray fault (`SiteFault.hang_ms`) on the real `SiteHealth`. **BF-39 passed hosted (2026-11-23)** on graft-m4a: `graft-ops` was killed 134 times, 119 of them inside a `SiteDay`, an `EdgeRoll` and a second `SiteDay` whose real gate breached and ran the pre-signed `Rollback`. Each step committed once and in order, and each intent had one `IntentDone`. The gray fault drained c by the drain key 3 min 39 s after it started. The automatic undrain was not reached, because steady load keeps c over `UNDRAIN_BURN`. The row refutes `GATE_APPLY_LAG`'s 500 ms cap and `DRAIN_BURN`'s margin, both [guess] values (OPEN-QUESTIONS R40, R41; `tests/m5/results/m5-wp4.md`). **Gates fixed in code (2026-11-23), not deployed:** `GATE_APPLY_LAG` is N+1's share over N's p99 with a 5 s backstop (`decisions/DECISIONS.md`, "`gate-relative`", closing R40), and the drain budget is 1 in 100 at 5 x, calm under 2 x over 15 minutes, `MIN_CALLS` 300 ("`drain-budget`"; R41 stays open). plant87 catches 49 of 49. Next: a check row on graft-m4a with a real N beside N+1 at 1, 10 and 50%.

### M5-WP5. The change feed, the export endpoints, and the vault

**Build.** The change feed: the Queue consumer that writes `ChangeBatch` as `ExportObject`s with entry bytes to `R2Key::Export` in the export bucket, `ExportCursor`, and the four export endpoints under `/api/graftyard/v1/export` with their chain-id encoding and errors, served from followers, learners, or archives and never the leader. `ExportClaims` export tokens, revoked by `jti` through `DELETE /api/graftyard/v1/export/token` (`design/DESIGN.md`, Shape, "Lifeboat feed"). `graftyard vault init` and the vault Worker, which pulls the feed, entries, snapshots, and packs under an install-scoped export token signed by the customer key into R2 under 35-day bucket locks, deletes a pack no sooner than `RESTORE_WINDOW` after the install's GC, and exports every `ChainId`. `graftyard vault upgrade` updates the vault Worker under the vault account's token ("Install operations", Vault; "Deployment modes"; "Failure modes", `vault`).

**Depends on.** M2-WP5, M5-WP1, M5-WP4.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-47 | `release roll` end to end, including `SiteDay { Workflows }` and `graftyard vault upgrade` | Order as in Release roll; vault upgraded with no install token | Any other order, or an install token used |
| BF-49 | Vault pulls every export endpoint; then the export `jti` is revoked | All four endpoints served; refused within `READ_CACHE` after revocation | Any served after `READ_CACHE` |

The vault's restore is gated by BM-7 (M7-WP8).

**Vault Worker and `graftyard vault init` built (2026-11-23), code only, not deployed.** `workers/vault` pulls `export/batches` into its own bucket byte for byte under `feed/`, fills chains first carried past index 1 from the entries endpoint (or a snapshot once archived), copies every stored pack and snapshot an entry names after checking it against the entry, and deletes a consumed or purged pack only `RESTORE_WINDOW` after the install first answered 404; its one rewritten object, `state/vault`, is unlocked and written compare-and-swap. `graftyard vault init` locks `feed/`, `entries/`, `snapshot/` and `pack/` for 35 days, deploys the Worker with an install-scoped token as its secret, and sets the minute cron (`decisions/DECISIONS.md`, "`vault-worker`"). In local workerd (`tests/vaultbed/run.sh`) the vault's own build pulled exportbed's real feed through the edge's own build: every `export/` object byte for byte, a 9 MiB pack in two parts, a bad pack refused, and nothing after `DELETE export/token` and READ_CACHE. **R39 (b) built in code (2026-11-23):** `vault init` and `vault upgrade` deploy only the `vault/` modules of a release whose `release.json` verifies under `Keys.release`, and a create-only put refused by 412 or by a lock's 409 is judged by HEAD (`decisions/DECISIONS.md`, "`vault-release`"). **Hosted vault row vh1 passed (2026-11-23), on the runner account as a stand-in vault account:** `vault init` on the real API, 461 of 461 feed objects byte for byte, a stored pack and a snapshot copied, 409 (10069) on a create-only put under the lock, a pass rerun from an older `state/vault` storing nothing new and failing nothing, `vault upgrade` keeping the bindings, and the teardown leaving nothing; $0.08. **BF-47's local half built (2026-11-23):** `release roll` runs `vault upgrade` after `McpRoll` under the vault account's token, never an install token, or prints it owed (`decisions/DECISIONS.md`, "`vault-roll`"); BF-47's hosted row remains. OPEN-QUESTIONS R39 (`tests/m5/results/m5-wp5-vault.md`).

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

**Build.** `graftyard apps init` in the install account, which registers `Graftyard MCP` and `graftyard-insights` and stores their keys and the MCP client secret in the install's Secrets Store (`design/AGENT-LAYER.md`, Shape, "First-party Apps"; "Deployment modes", "Enable agents", step 1). `Consent`, `McpGrant`, `McpToken`, `McpRefresh`, and `WriteScope` (`design/AGENT-LAYER.md`, Shape, "graft-mcp"). The flow in journey A7: RFC 9728 and RFC 8414 metadata, PKCE S256, CIMD fetches with their limits, consent before the GHES redirect, the `__Host-` cookie bound to `state`, `token/scoped` narrowing with the additive `graftyard_grant` and `graftyard_write` fields, the unscoped `ghu_` held sealed and deleted only after the next refresh (A4's fallback, built as the behavior), the RFC 9207 `iss` parameter, and refusal of an older `generation` that revokes the grant. The grant endpoints under `/api/graftyard/v1/mcp/grants`, authenticated by the `Graftyard MCP` App's client credentials ("API reference"). `IdentityEvent::McpGrant` and the settings page at `https://mcp.<host>/settings`. The sealing key in Secrets Store.

**Depends on.** M3-WP3, M3-WP4, AM1-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| LF-6 | `token/scoped` from `Graftyard MCP` without `graftyard_grant`, with another user's grant, and with `graftyard_write` above the grant. A stale `generation` on `POST /mcp/grants/{id}/generation`. Grant calls with wrong client credentials | 422 for each bad token request. 409 and the grant revoked for the stale generation. 401 for wrong credentials, 100 of 100 | Any accepted |

It also carries LM-2's MCP row (the rerun of L0-08).

**Built so far (2026-11-22), the pure half** (`tests/am2/results/am2-wp1.md`; branch `am2-wp1`). `graftyard apps init`
(`cli/src/apps.rs`); the grant rows in Identity format 0x14.4 and scoped `ghu_` fields in Tokens format 0x11.4; the
grant endpoints and `token/scoped` for `Graftyard MCP` (`crates/api/src/mcp.rs`, routed by `workers/edge/src/mcp.rs`);
and `graft-mcp` as `workers/mcp`, with the metadata, CIMD, PKCE, consent, callback, token, settings, and the MCP door.
- **LF-6: pass in memory.** 422 for each bad `token/scoped` request, 409 with the grant and its tokens revoked for a
  stale generation, and 401 for wrong client credentials, 100 of 100 each. The 200 and 422 replies validate against
  GHES 3.22.
- **L0-08's local checks: pass.** 100 of 100 wrong-audience tokens refused, a refreshed token keeps its scope over 100
  refreshes, a replayed refresh revokes the grant, framed, cross-site, or skipped consent issues no code, and each
  refresh succeeds with the unscoped `ghu_` held, which is deleted after it.
- 20 of 20 planted bugs caught (`tests/m0carried/plant86.py`).

**Hosted, auth-only day m3fa2 (2026-11-23, graft-m3f).** LF-6 passes hosted, 100 of 100 each. L0-08's auth cells
pass: wrong audience, refresh scope, replay, consent, and the held `ghu_`. A43: none of the four SDK clients retries a
code after a lost `/token` answer; each goes back through consent. The day fixed graft-mcp's CIMD fetch
(`global_fetch_strictly_public`) and its edge calls (`User-Agent`). L0-08's 30-call cell and the tool-overhead p50
wait for WP2a (`tests/am2/results/am2-wp1.md`, "Hosted, auth-only day m3fa2").

The hosted half needs:
1. A site day onto a release that reads 0x14.4 and 0x11.4, then `FormatsEnable` for both. Until then the grant
   endpoints and `token/scoped` answer 503. Done on graft-m3f for m3fa2, with FORMATS only.
2. `graftyard apps init` on a test install, and `graft-mcp` deployed with its bindings.
3. AM2-WP3's routing of `mcp.<host>`.
4. L0-08 hosted, with the four SDK clients.

### AM2-WP2. `graft-mcp` tools

**Build.** `call`, `Toolset`, `ToolError` with `retry_after_s` and rate fields, the `coordination` and `provenance` tools, and the trust view on every text field (`design/AGENT-LAYER.md`, Shape, "graft-mcp"; journey A7, step 7). Writes run as the `Graftyard MCP` `AgentApp` with `requester = sub`.

**Depends on.** AM2-WP1, AM1-WP4, AM1-WP5.

**Acceptance.** No gate row of its own. It carries LM-2's MCP row.

**Split (the lead's ruling, 2026-11-23).** WP2 ships in two parts, WP2a first.
- **WP2a: what L0-08 needs.** The MCP door over Streamable HTTP in both lanes: the 2026-07-28 stateless lane with
  `server/discover` and per-request `_meta`, and the 2025-11-25 `initialize` lane the TypeScript v1 SDK needs. Also
  `call` with the static toolsets, `ToolError` with `retry_after_s` and the rate fields, the trust view, writes on the
  scoped token, and the lease and feed tools. A second hosted day for L0-08's 30-call cell and the tool-overhead p50
  runs once WP2a merges.
- **WP2b: the tools that need new edge reads.** `coordination.overlaps` and `provenance.why` with their edge reads. The
  edge read's design goes to the lead before any edge change, and is checked with B0-34's work on the edge's
  cold-isolate write path (F112).

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

It also carries three clauses moved from LM-1 by the operator's decision of 2026-11-10 (`decisions/DECISIONS.md`, "The Graftyard-run clauses move from LM-1 to AM3-WP3"), each run hosted once Graftyard-run tasks can be created:

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| L0-12, Graftyard-run kind | 100 pushes and 100 REST ref writes by a `graftyard-workspace` token with a live Graftyard-run task, into another holder's Exclusive scope under `AgentsExclusive` | 100 of 100 of each refused naming the lease, not `NoLiveTask` | Any accepted, or any refused before the lease check |
| B0-36, Graftyard-run kind | A `graftyard-workspace` token with a live Graftyard-run task: a REST ref write into another task's Exclusive scope under `AgentsExclusive`, a push with no live task, and a fork push after its task is cancelled, over git and REST, with shipped and widened permissions | 100 of 100 refused for each, the ref write naming the lease | Any accepted, or the ref write refused before the lease check |
| B0-37, fetch half | A session ref pushed by `graftyard-workspace` under a live Graftyard-run task, then v0, v1, v2, and `--mirror` fetches and a v2 `ls-refs` | The session ref appears only to a v2 `ref-prefix` that starts with it | It appears to any other listing, or not to the matching `ref-prefix` |

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

Built (`design/DESIGN.md`, "Milestones 1 to 7"): `ReplaceMember` signed by the executor key and `RestorePlacement`, both finding groups through `MemberRegistry`, `Degraded`, lifeboat tail and pack pulls over the Milestone 5 export endpoints, `graft-blob`, lifeboat registration, `lifeboat:read` enroll, the workerd bundle, and native SSH through a Spectrum Worker app, the edge's `connect()`, `SshFront`, `graft-sshd`, and `SshOutbound`, with host keys from Secrets Store, plus `git-ssh` and the SSH parts of `lfs` and `rest:deploy-keys` [inference].

### M6-WP1. Region loss and re-protection

**Build.** `ReplaceMember { lost, spare }` as a `Release` intent signed only by the executor key and accepted after a customer-signed `SiteLost`, or after `ZERO_ACKS` with zero acks in both other sites' `SiteHealth`. `graft-ops` then starts one Workflow instance per group, enumerated from the surviving sites' `MemberRegistry` rows that name the lost site. Each instance adds a member on the same continent's spare through a `Joint` config, under a new member generation and by `InstallSnapshot`, copies single-copy packs first, then each network once, and refuses a spare that cannot read every enabled format. `RestorePlacement { site }` takes an intent key, is refused while the site is drained, and runs once the site counts as recovered. Transfer back to `home`, and `deleteAll()` on removed members after 35 days. `GroupConfig::Degraded` for the `*2w` placements (`design/DESIGN.md`, "Commit and read paths", journey 5; Shape, "Chains" and "Release group"; "Install operations", Site loss; "Failure modes", `chain groups`).

**Depends on.** M5, M3-WP5.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-50 | Lose site A with every API token revoked; drain it 24 h; then recover it | Executor-signed `ReplaceMember` moves every group with a voter on A, using B's and C's registries; `RestorePlacement` refused until undrained | A group left out, or a restore while drained |

It also carries BM-6's drill.

### M6-WP2. The SSH front

**Build.** Native git over SSH, off by default per install, so a stock `git clone git@<host>:owner/repo.git` and `git push` work on a Pro zone with no `cloudflared` (`FRAME.md`, "Operator decision, 2026-11-19, native SSH on Pro"; `decisions/DECISIONS.md`, "SSH goes through a Spectrum Worker application to the SSH front (2026-11-19)"; `design/DESIGN.md`, Shape, "SSH front"; journey 4, step 3):
- **Ingress.** A Spectrum TCP app on port 22 with `traffic_type: "worker"` whose origin is the edge script, created and deleted by the operator CLI: `graftyard ssh spectrum --zone <zone id> --hostname <name> --edge <edge script>`, idempotent, `--delete` removes it, with a token that has zone Spectrum edit. Pro refuses the `tls` field (10012), so the CLI never sends it [measured, `tests/m0/results/m0-wp7.md`, B0-24c].
- **The edge's `connect()` handler**, on with `SSH_OPTION=on` (the edge deploy takes `--ssh`). It picks the `SshFront` object `ssh#<site>` and tries sites in turn until a front answers with its SSH banner, failing over only before any client byte is forwarded.
- **`SshFront`**, a new Durable Object class on each site script, one object per site. It starts its container (image `sshfront`, `basic`, `durable_object` policy, no internet), registers outbound intercepts for `keys.graft` and `git.graft`, pipes the socket to the container's port 22 (`ctx.container.getTcpPort(22)`), and starts a stopped container again on the next session. It holds no alarm and no pending `monitor()`, so its container stops within `IDLE_MS` (1 h) of the last session (`decisions/DECISIONS.md`, "An SSH front holds nothing past its session"). The site deploy takes `--ssh <store id>/<secret prefix>`, which binds the host-key slots `SSH_HOST_KEY_A` and `SSH_HOST_KEY_B` from Secrets Store, and `--edge <edge script>`, which binds `EDGE` with `props` `{"ssh": true}`.
- **`SshOutbound`**, a `WorkerEntrypoint` of the site script (DESIGN's `ssh_outbound`): `keys.graft` answers both host-key slots; `git.graft` forwards to the edge over `EDGE`.
- **`graft-sshd`** in `containers/ssh` (Go on golang.org/x/crypto/ssh; no OpenSSH, git binary, or cloudflared). It fetches both host-key slots from `keys.graft` at start, serves the active slot's keys in key exchange, announces both with `hostkeys-00@openssh.com`, and answers `hostkeys-prove-00@openssh.com`. It authenticates a key by `GET /_graftyard/ssh/key?fp=<hex>`, which the edge honours only with the `EDGE` props and answers from the key's `Tokens` row on `fingerprint_shard` through `fresh`; a deploy key is refused as a user. It runs `git-upload-pack` (protocol v2 only; a v0 fetch gets an error naming `GIT_PROTOCOL=version=2`) and `git-receive-pack` (v0) as the edge's smart-HTTP requests with the fingerprint in `x-graftyard-ssh-key`, so SSH takes the same read gate, staging, validate, place, commit, and `report-status`. It keeps B0-24a's findings that still apply: a 30 s login grace, client keepalives every 15 s with 4 misses, and retries through the edge for at least a group failover.
- **The write path.** `Credential::Ssh(fingerprint)` in repository format 32.26 (`REPO_FORMAT_SSH`), with `CredentialKey::SshKey { shard, fingerprint }`; the leader re-reads the key's row and the `Account` row once per entry. Sent only after `FormatsEnable` enables 32.26.
- **Host keys.** One Ed25519, one ECDSA P-256, and one RSA 3072 key per slot, two slots in Secrets Store (`SshHostKeys`), generated and stored by the script in `deploy/ssh-front/`, rotated by `Rotate`. The edge serves both slots' public keys and fingerprints at `/api/graftyard/v1/meta/ssh` from a variable the deploy writes; `/api/v3/meta` keeps its GHES shape ("Install operations", Rotation runbook).
- **Docs and refusals.** The edge's over-cap refusal and the 413 runbook keep HTTPS as the default and name SSH where the install has it on. The install docs give the stock remote, suggest `ConnectTimeout 30`, and say a failed session, a push included, is safe to retry. The edge's `connect()` sees the client's address but does not pass it to `graft-sshd` yet, so SSH abuse limits count by key and token ("Open questions and risks").
- **Not built.** Protocol v0 fetch and deploy keys over SSH. The `cloudflared` option, the Tunnel, and the Enterprise Spectrum-to-Tunnel ingress retire. The Oracle Cloud VM in `deploy/oci-ssh/` retired on 2026-11-20, when `ssh.graftyard.dev` moved to the Spectrum app on `graft-m3f` (`tests/m6/results/m6-wp2.md`, "Cutover"). Deploys of that install without `--ssh` refuse while the app points at it (`workers/site/sshguard.mjs`); `--ssh-off` turns SSH off on purpose.
- The `git-ssh` surface and its oracle, and the SSH parts of the `lfs` and `rest:deploy-keys` oracles (`design/SHARED.md`, "Surfaces, tiers, and oracles").

**Depends on.** M3-WP2, M3-WP3, M1-WP6, M0-WP7, M5-WP6.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-51 | Rotate SSH host keys | `UpdateHostKeys` clients learn the new set; `/api/graftyard/v1/meta/ssh` lists both slots for the overlap; same fingerprints on every replica | A mismatch, or a host-key error for an updated client |
| BF-52 | Suspend a user, then push over SSH with their key; separately, revoke an SSH key with 1,000 of its commands queued | Every push from the suspended user refused; 100% of commands decided after the revocation get 401 | Any accepted |
| B0-24d | The Spectrum Worker ingress end to end on a Pro zone (`graft-m3f`): a stock `git clone git@<host>:owner/repo.git`, a `git push`, a key refused after its removal, and a session held 10 minutes. Hops measured 2026-11-20: 1 MiB byte-exact in 18 ms after open, 4 MiB in 839 to 991 ms, open 20 to 137 ms warm, about 2 s cold [measured, `tests/m6/results/m6-wp2.md`]. End to end on 2026-11-20 through `sshtest.graftyard.dev`: clone rc 0 in 5.6 s, push rc 0 in 9.5 s with main read back over HTTPS, clone and push refused "Permission denied (publickey)" after the key's removal, and a session held 633 s with 11 of 11 `ls-refs` answered [measured, `tests/m6/results/m6-wp2.md`]. **Pass** | All four pass | Any fails |
| B0-24a on the Spectrum ingress | Three fronts, one killed every 10 minutes for a day, sessions through Spectrum and the edge's `connect()`, with writes; not yet run | At least 99.9% of sessions succeed, and no acknowledged push lost | Under 99%, or a lost acknowledged push, which leaves SSH off |

BF-51 and BF-52 are built as unit tests: `crates/git/tests/push.rs`, the `bf52_*` tests, and `graft-sshd`'s Go tests for the host-key announce and prove. Hosted on `graft-m3f` (2026-11-20): BF-51's client half passes (an `UpdateHostKeys=yes` client went from 1 to 6 known host keys, matching `/api/graftyard/v1/meta/ssh`), and BF-52's suspension half passes (20 of 20 SSH pushes refused, no ref moved) [measured, `tests/m6/results/m6-wp2.md`]. BF-52's revocation half with 1,000 queued commands is the unit test only.

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

Built: BYOC and self-hosted through Terraform and Wrangler, account-separated sites, managed front and cell accounts, `RateCounter` rate limits, `Solo` and workerd for evaluation with the `LeaveEvaluation` move to `Tri`, child groups for oversized metadata (`RepoMeta` chains the parent leader seeds in band), and import from GHES as `Imported` entries with the plan preflight.

### M7-WP1. BYOC and self-hosted installs

**Build.** Terraform for the zone, DNS, buckets, queues, and WAF; the Spectrum app comes from `graftyard ssh spectrum` (M6-WP2). Wrangler for the first creation of `graft-front`, the edge, the site scripts, and `graft-ops`, after which only `graft-ops` creates versions. The install, vault, and runner accounts, and `Keys { vendor: None }` for self-hosted ("Install operations", Tools; "Deployment modes", "What each mode requires of the customer").

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

**Started early (2026-11-11).** B0-30 failed because no child group took the tier at `CHILD_GROUP_AT` (AM1-WP6), so branch `b030-split` builds `RepoMeta` hosting and the split for B0-30's rerun (`decisions/OPEN-QUESTIONS.md`, G6). This package's own gate rows are unchanged.

**Build.** `ChainId::RepoMeta` hosted as a repository chain, and `RepoEvent::Split` with `MetaSplit`, `MetaSeed`, `MetaDropped`, and `MetaSlim`. The parent leader's alarm runs every step in band, with no Workflow: at `SPLIT_AT` of `ctx.storage.sql.databaseSize` it records `MetaSplit`, records `MetaSeed` at its newest committed metadata snapshot, creates `RepoMeta(repo)` on the parent's placement record and records `MetaSeed` there, and once the child's members have restored that snapshot records `MetaDropped` and then `MetaSlim` entries that remove from the parent the text no `decide` reads, for rows written at or before the seed. Every command still commits on the parent with no new `deps`, so no write gets 503 and nothing routes to the child; reads and replies fill the slimmed text from the child's rows (`decisions/DECISIONS.md`, "A split moves only the text `decide` never reads"). The parent keeps the feed, the consumers' cursors, `RestoredTo`, and `RepoReset`'s index; the export name is `repo-meta:<id>`, and owner moves carry child groups (`design/DESIGN.md`, Shape, "Chains" and "Sites and placement"). `InsightVerdict` rows stay on `Repo(repo)` (`design/AGENT-LAYER.md`, Shape, overlaps block).

**Depends on.** M2-WP1, M7-WP3, AM4-WP2.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-9 | A monorepo member reaching `CHILD_GROUP_AT`, and `octokit.paginate` over `/repositories/{id}` links on a managed cell | The metadata tier moves to a child group with zero `SQLITE_FULL`; page 2 is served by the right cell | Any `SQLITE_FULL`, or a page routed to the wrong cell |
| BF-62 | Monorepo member reaches `CHILD_GROUP_AT` under 300 commands/s with comments, PR opens, merges and releases with tags | Zero `SQLITE_FULL`. No metadata write gets 503. Every both-tier entry has its `MetaHalf` within 60 s, with zero lost or doubled. A comment on #n is never delivered before #n opened, 100 of 100. Pre-split feed cursors continue with no 410 | Any `SQLITE_FULL`, any metadata 503, a lost or doubled half, an out-of-order delivery, or a 410 |
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

**Build.** `graftyard import ghes <archive>` for a `ghe-migrator` archive, as `design/SHARED.md` describes under `rest:migrations` ("Surfaces, tiers, and oracles"). With no GHES instance, BF-65 runs on a synthetic archive built from GitHub's documented `ghe-migrator` format, untested against a real GHES export [inference] (`decisions/DECISIONS.md`, "Decisions for a Pro-only build with no GHES, 2026-10-31"). Every import entry is `EntryBody::Imported(ImportBatch)` with `ImportItem`s that carry the original time, author, legacy id, and `merged`. `Webhooks`, `ActionsTrigger`, `Notify`, `RunOwner`, `PrSync`, `MergeQueue`, and `Insights` skip them. The import-only command set runs behind the `importing` flag in `Settings`, keeps issue numbers and moves the counter past the largest, records a merged pull request with no `Merged` event and no tree-diff check, and fills the `legacy_id` table (`design/DESIGN.md`, Shape, "Chains"; `design/SHARED.md`, `rest:migrations`). The install preflight checks the plan against the largest imported pack (`design/DESIGN.md`, "Review resolutions", CF-ACT-12).

**Depends on.** M7-WP1.

**Acceptance.**

| Row | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| BF-65 | GHES archive import with hooks, a workflow, watchers, merged PRs and `#N` links, on a synthetic `ghe-migrator` archive built from the documented format while no GHES instance exists [inference] | Zero webhook deliveries, runs or notifications. Numbers, `created_at` and authors match. Merged PRs show merged. `legacy_id` resolves every id | Any delivery, run or notification, or any mismatch |

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
| B0-24a | DESIGN, Milestone 0 | SSH option through `cloudflared access ssh`, Tunnel, cloudflared (retired); its gate carries to the Spectrum ingress | M0-WP7, M6-WP2 |
| B0-24b | DESIGN, Milestone 0 | Native SSH through Spectrum, Tunnel, cloudflared (Enterprise; dropped 2026-11-19) | M0-WP7 |
| B0-24d | DESIGN, Milestone 0 | Spectrum Worker ingress end to end on Pro | M6-WP2 |
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
