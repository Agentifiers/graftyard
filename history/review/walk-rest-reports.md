# Third walk: writer reports (X1, X2, Y)

## WriterX1

All seven items, WK-X1 to WK-X7, are applied. The three checks pass, and `scripts/__pycache__` is removed.

## Per item

**WK-X1 (W2): broker and live-log relay.** Added in `design/DESIGN.md` Shape, Identity.
- `RunnerBroker { owner, scope }` and `LogRelay { job }` are site-script classes that hold derived state only.
- **Placement:** each object sits on its shard's leader site, which the edge reads from the cached `ReplyMeta`. A broker uses its registration shard. A relay uses the job's shard at `acquirejob`, and `FeedStreamUrl` names it. When the leader moves, a fresh object starts on the new site and reloads from the shards.
- **Cap:** `BROKER_HELD_MAX` is 2,000 held polls [guess]. Past it, the broker answers 503 with `Retry-After: 15`.
- **What the runner does then:** it polls again after a random 15 to 30 s, and its session stays valid [unverified: `BrokerMessageListener` backoff].
- **Relay:** it hibernates its WebSockets and keeps the last 1,000 lines [guess].
- `design/SHARED.md` "Actions", Logs, now describes the relay and the fallback to step log blobs.

**WK-X2 (W3): run, job, runner, and session events.**
- In DESIGN, `ActionsEvent` is now an enum:
  - `RunCreated { run, jobs: Vec<JobSpec> }`
  - `RunStatus`
  - `Job { job, version, event }`
  - `Runner { id, record }`
  - `Session { runner, session }`
- `actions_shard(run)` is `hash(run.repo) % 16`.
- In SHARED "Actions runs and jobs" I added `JobSpec` (labels, needs, matrix row, environment), `RunnerScope { Owner, Repo }`, `RunnerRecord`, `RunnerSessionId`, and `JobEvent::QueueExpired`.
- I dropped the old `Creating` run. One `RunCreated` event now carries the run and all its jobs.
- **How one broker reads all 16 shards:** it loads each shard's queued summary when it starts. It then applies hints that each shard's leader sends after a commit that moves a job into or out of `Queued`. Each hint carries a count, and the broker reloads a shard if a count is skipped or after 30 s of silence [guess]. A job is handed out only after `Matched` commits on its shard.

**WK-X3 (W4): workflow run.** Added a "Workflow run" bullet to SHARED "Multi-chain operations".
- `RunCreated` is the commit point, and `RunOwner` is the finisher.
- Every repository entry is keyed by (job, attempt). `completejob` commits on the Actions shard first, then appends the repository entries itself, and `RunOwner` writes any it missed.
- `Cancel` is the abort path.
- **No `RequiredCheck` is left without a conclusion:** every job reaches `Completed`, and `RunOwner`'s cursor passes a completion only once the repository chain acknowledges or refuses its conclusion entries.
- I updated the Completion invariant and the "Other consumers" text to match.

**WK-X4 (W6): webhook delivery.** Added to SHARED "Event fan-out".
- `HookCursor { read, replay_from }`: `read` advances once the `HookDelivery` rows are recorded, never on an attempt.
- Order holds per (chain, hook): one queue each, with one attempt in flight.
- `HOOK_OWNER_CONCURRENCY` is 50 [guess]. Hooks that recently failed share at most half the slots.
- A slow hook therefore delays only its own queues.

**WK-X5 (W7): secrets.** Added a "Secrets and variables" paragraph to SHARED "Actions".
- Stored formats: `SealKey` (X25519, with `key_id`), `Wrapped`, `ActionsName`, `SecretScope` (repository, environment, or org), `SecretRecord`, and `VariableRecord`.
- The two components that decrypt are job-message assembly and webhook signing.
- **Rotation:** a new key every 90 days [guess], with old secrets resealed to it. The wrapping key rotates through its two slots.
- In DESIGN, `SecretName` gains `SecretsWrapKey`, which only `graft-ops` reads, and `HostedRunnersAppKey`.

**WK-X6 (W8): hook record and delivery store.**
- SHARED `RepoEvent` gains `Hook { id, record: Option<HookRecord>, actor }`, plus `ActionsSecret` and `ActionsVariable`.
- `HookRecord` holds url, events, content_type, active, insecure_ssl, the wrapped secret, and the retry opt-in.
- The site-local delivery store is a `HookSequencer` object per hook owner, on the owner's home site. It also backs ping and the deliveries endpoints.
- A store lost with its site is rebuilt by replaying from `replay_from`.

**WK-X7 (W11): hosted-runner Worker.**
- SHARED "Hosted runners" covers the demand endpoint, `GET /api/graftyard/v1/owners/{owner}/actions/hosted/demand` (`Broker::demand` and `HostedDemand`), and the `graftyard-hosted-runners` App credential.
- A `workflow_job` webhook prompts a demand read, and a 5-minute sweep catches lost webhooks.
- It also covers `generate-jitconfig`, Sandbox start and destroy, a label table with every value [guess], and a per-owner cap of 20 [guess] that `decide` enforces.
- DESIGN names `graft-runners`, gives the Sandbox instance types, and adds `RUNNER_VCPU_STOP_PCT` of 70% [guess].
- WriterX2 applied my exact text to the Workers, Durable Objects, Sandbox SDK, Cron Triggers, and Secrets Store rows of the Stack table.

**Failure modes.** I rewrote all 8 `event pipeline` rows and all 8 `actions state` rows so each one names the broker, the relay, `HookSequencer`, and `graft-runners`. I also updated the component list sentence. No new component was needed, so the Deployment modes rows did not change.

## Decisions beyond the item text
- Runs are sharded by repository, not by run id. `actions_shard(run)` hashes `run.repo`. This makes `/actions/runs` a single-shard query, and a run id is found through its repository.
- The sequencer is one object per hook owner, with a queue per (chain, hook) inside it, rather than one object per hook. That is what lets it enforce the per-owner cap. A retried delivery holds its queue so that order still holds.
- Hosted runners use a first-party App and the existing `generate-jitconfig` endpoint, so no new credential type was needed. A user's `Owner` scope holds only hosted runners.
- A job queued for 24 hours completes with `Failure` [unverified].

## Gate rows
| Milestone | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| 4 | Hold 2,500 runner polls on one broker scope (BROKER_HELD_MAX + 500) with jobs queued | Polls past the cap get 503; those runners re-poll within 30 s with their sessions valid; job start p99 at most 35 s for them | Any `RunnerSessionInvalid`, a runner that exits, or a job unstarted after 60 s |
| 4 | Kill the leader site of an `Owner` scope's registration shard during a 256-job matrix | Polls reach a broker on the new leader site within 15 s; every job runs once; zero jobs assigned twice | A job assigned twice, or dispatch stalled over 60 s |
| 4 | Kill the repository's commit path (and, separately, `RunOwner`) between `RunCreated` and the `RequiredCheck` writes, then finish or cancel every job | Every `RequiredCheck` on an open PR head has a conclusion within 60 s of its job's `Completed` | Any `RequiredCheck` with `conclusion: None` after its job completed |
| 4 | One hook timing out at 10 s beside 49 healthy hooks of the same owner, 10 events/s | Healthy hooks' first-attempt p99 at most 2 s; the slow hook gets its events in chain order; Webhooks cursor lag at most 5 s | Healthy p99 over 10 s, an out-of-order delivery to one hook, or cursor lag that grows with the slow hook |
| 4 | Kill the site holding a hook owner's `HookSequencer` with deliveries pending | Every event in the range reaches every hook at least once, with unchanged GUIDs | Any event never delivered |
| 4 | `gh secret set` for a repository, an environment, and an org, then a job reading all three; then rotate the `SealKey` and the wrapping key | The job sees all three values before and after both rotations, and a fork-PR job sees none | A wrong value, a value shown to a fork PR, or a plaintext secret found in any entry or log |
| 4 | 50 `ubuntu-latest` jobs queued for one owner with the cap at 20, while two `graft-runners` sweeps overlap | At most 20 hosted runners at once; every job finishes; no Sandbox is left 10 minutes after the last job | More than 20 at once, or a leaked Sandbox |
| 4 | View a running job's live log in the web app, then kill its `LogRelay` | Lines appear within 2 s; after the kill, the viewer falls back to step log blobs and the final log is complete | Missing lines in the final log |

## Open questions
1. Do the runner's broker listener's backoff and session behavior on 503 match the [unverified] claim? The first gate row settles it.
2. One owner's `HookSequencer` is one object under the 1,000 req/s soft limit. Does a very large org need it split by hook hash?
3. Does the runner reconnect its `FeedStreamUrl` WebSocket when it breaks, or drop live output?
4. Does GHES end a job queued for 24 hours, and with which conclusion?
5. Sandbox CPU architecture, and whether arm or larger hosted labels can be offered.
6. Does a `workers.dev` route work as `graft-runners`' webhook URL, or does the runner account need a zone?

## Final line counts and checks
`design/DESIGN.md` is 1,899 lines (cap now 2,200), `design/SHARED.md` is 960, and `design/AGENT-LAYER.md` is 997.

```
design/DESIGN.md: OK
design/AGENT-LAYER.md: OK
agent block: OK
cost figures: OK
check_names: OK, 34 files
check_links: OK, 1360 citations
```

## WriterX2

I applied all six items, WK-X8 to WK-X13. Each is in the region it was assigned to, plus a few small edits outside those regions, listed under "Edits outside my owned regions". All three checks pass. `design/DESIGN.md` is at 1,899 of its 1,900-line cap, so one more line is all that fits there.

## Per item

**WK-X8 (W1, child groups).** Added in `design/DESIGN.md` Shape, "Chains":
- `ChainId::RepoMeta(RepoId)`.
- `EntryBody::Split(SplitEntry)`, with variants `MetaSplit { child, at }`, `MetaSeed { at, snapshot }`, `MetaHalf { parent, prev, events }` and `MetaDropped`.
- A doc block on `CHILD_GROUP_AT` that covers each part of the item:
  - **Trigger.** The leader reads `ctx.storage.sql.databaseSize` after each snapshot. I checked that this API exists on the Cloudflare storage-api page, which is cited.
  - **Commit point.** `MetaSplit` is a core entry on the parent, and `at` is its own index.
  - **Routing.** From `at`, the parent refuses metadata-only commands and reads with `Moved { child }`, and the edge reroutes them.
  - **Seeding.** `Audit` starts a `MetaSplitter` Workflow. The child commits `MetaSeed` from the metadata snapshot at `at`. The parent then drops its metadata tables after `MetaDropped`. Metadata writes return 503 between `at` and `MetaSeed`.
  - **Commands on both tiers.** The whole entry commits on the parent first. The named finisher `meta_finish` appends the metadata half to the child as an ordered `MetaHalf`. The edge runs it before answering, and `Audit` runs it after a minute.
  - **Consumer ordering.** Consumers skip `MetaHalf`. A child cursor does not pass `MetaHalf { parent: k }` until the parent cursor has passed k.
  - **Repair.** `RestoredTo` runs on the parent only. `RepoReset` resets each chain to its own index at time t. For a t before the split, the child replays from the parent's archived entries.
  - **Feed.** The parent keeps its log, indexes and feed ring, so a split causes no 410.
- Export name `repo-meta:<id>`.
- The owner-move Workflow now moves child groups too.

**WK-X9 (W9, watcher index and `Inbox`).** In Shape, "Identity", the `Counts` consumer now writes a derived watcher index (`watcher` and `thread_sub` rows) to the repository's metadata tier. `/stargazers`, `/subscribers` and `Notify` read it. For `Inbox`, I added a daily snapshot with a `sources` table and a rebuild rule: Identity thread state, then a replay of the chains in `sources`, then the account's watched and subscribed repositories from `INBOX_WINDOW` (2 days [guess]) before the snapshot. `Inbox` is now keyed by `{ account, site }`. I also updated the `rest:activity` row and the "Derived state" row of the RPO table.

**WK-X10 (W10, search store).** Added `SearchShard { domain, shard, generation }` and `Domain`: FTS5 tables, 16 shards per residency domain by repository hash [guess]. A shard sits on the domain's first Full site under the domain's DO jurisdiction, and a lost shard is rebuilt from metadata tiers found through `MemberRegistry`. Updated the `rest:search` row (5 s deadline [guess], then `incomplete_results`) and the Residency paragraph.

**WK-X11 (W12, Solo to Tri).** Added `Genesis.evaluation: bool` and `Intent::LeaveEvaluation { sites, placements, default }`, which only the customer key may sign. Its commit adds the new sites, swaps the system-chain records and clears the flag. `graft-ops` then moves each group through `Joint` in this order: `Release`, `Ids`, `Names`, `Tokens`, then each `Solo` owner one at a time [guess]. I also updated the signer rule, the Keys bullet, the Evaluation bullet in "Deployment modes", and the placement-record and routing comments.

**WK-X12 (W13, imports).** Added `EntryBody::Imported(ImportBatch)`, plus `ImportBatch` and `ImportItem` (original time, author, legacy id, and `merged`). The `rest:migrations` row now covers:
- which consumers skip imported entries;
- the import-only command set, gated by an `importing` flag in `Settings`;
- issue numbers kept, with the counter moved past the largest;
- a merged PR recorded with no `Merged` event and no tree-diff check;
- where the `legacy_id` table lives.

**WK-X13 (W15, rate counters).** Added `RateCounter`, `RateKey` (user, installation, `GITHUB_TOKEN` repository, minted agent token, IP) and `RATE_FLUSH = 20` [guess]. Each key is one Durable Object defined by `graft-site-a`. The error bound is in the SHARED "Public service concerns" bullet and the `rest:rate-limit` row: `x-ratelimit-remaining` can overstate by at most 20 requests, or one second of requests, for each other isolate serving that key.

**Stack and failure rows.**
- Stack: changed the "Used for" cells of the Durable Objects and Workflows rows. I added WriterX1's text to the Workers, Durable Objects and Sandbox SDK rows at its request.
- One new failure component, `derived stores` (`Inbox`, `SearchShard`, `RateCounter`, watcher index), with a row for each of the 8 failure classes, a Deployment modes row, and an entry in the component list.

## Decisions beyond the item text
- **Watcher index storage.** It lives in the repository's metadata tier, written by `Counts`, instead of a new object class. That makes it chained state that a refold rebuilds.
- **Issue numbers.** `IssueOpened` touches both tiers after a split, so the parent keeps the issue-number counter.
- **Snapshot window.** Metadata writes return 503 between the split and the child's seed snapshot. The new split gate row below measures this window.
- **Search sharding.** Search is per residency domain and sharded, not per owner, so a cross-owner query fans out to one domain's shards instead of every owner's.
- **Rate-limit failure.** Rate limiting fails open per isolate when its counter object cannot be reached.
- **Edits outside my owned regions.** Small edits to unowned text, needed by my items:
  - the `Release`-block signer rule, the Keys bullet and the Evaluation bullet;
  - the export chain list;
  - the `rest:activity` row;
  - the RPO "Derived state" row.
- **Coordination with WriterY.** WriterY agreed to change the AGENT-LAYER feed 410 sentence to my text. It also agreed to make its verdict entry a core-tier entry on the parent repository chain.

## Gate rows (Milestone | Measurement | Pass | Fail)
| Milestone | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| 7 | Monorepo member reaches `CHILD_GROUP_AT` under 300 commands/s with comments, PR opens, merges and releases with tags | Zero `SQLITE_FULL`. Metadata 503 window at most 10 min [guess]. Every both-tier entry has its `MetaHalf` within 60 s, with zero lost or doubled. A comment on #n is never delivered before #n opened, 100 of 100. Pre-split feed cursors continue with no 410 | Any `SQLITE_FULL`, a lost or doubled half, an out-of-order delivery, or a 410 |
| 7 | `RepoReset` to a t before the split and to a t after it, on a split repository | Both tiers equal a refold at t | Any tier differs |
| 7 | `LeaveEvaluation` on a Cloudflare `Solo` install under write load. Separately, a vendor-signed `LeaveEvaluation` | System groups and owners end on `Tri` with `moving_from` cleared, zero lost acknowledged writes, `Placement::solo` refused afterwards. The vendor-signed one is refused | Any lost write, a group left on `Solo`, or the vendor-signed one accepted |
| 7 | GHES archive import with hooks, a workflow, watchers, merged PRs and `#N` links | Zero webhook deliveries, runs or notifications. Numbers, `created_at` and authors match. Merged PRs show merged. `legacy_id` resolves every id | Any delivery, run or notification, or any mismatch |
| 7 | 20 isolates hit one key at the limit, then the counter object is killed | Overshoot of `remaining` and of enforcement within 20 x isolates. Refund at most one window | Above the bound |
| 4 | Delete an `Inbox` and rebuild it. Also 10,000 stars, then a refold | Read and done state kept, nothing from `sources` lost. `/stargazers` lists all 10,000 | Any lost read state or star |
| 4 | EU-resident org's search documents. Kill one `SearchShard` | Documents only in the `eu`-jurisdiction shards. `incomplete_results` until the rebuild, then full results | A document outside EU, or results wrong after the rebuild |

## Open questions
- Is a metadata-write 503 window of minutes during a split acceptable? Pre-seeding the child before the split would shrink it, but that departs from the item text ("child starts from the snapshot at `at`").
- `Placement::solo` needs the install-wide `evaluation` flag, yet Usage shows `placement set eval-org solo` on an install that otherwise looks normal. This predates my work: is per-owner `Solo` allowed on an install without the flag?
- FTS5 inside Durable Object SQLite is cited only for D1 in `research/cloudflare.md`. The existing Stack row already assumes it. Milestone 0 should confirm it.

## Final line counts and checks
- `design/DESIGN.md` 1,899 (cap 1,900). `design/AGENT-LAYER.md` 997 (cap 1,000). `design/SHARED.md` 952.
- `check_design.py`: DESIGN OK, AGENT-LAYER OK, agent block OK, cost figures OK.
- `check_names.py`: OK, 34 files.
- `check_links.py`: OK, 1,360 citations.
- `scripts/__pycache__` removed.

## WriterY

I applied WK-Y1, WK-Y2 and WK-Y3. All three checks pass, and the agent block is identical in `design/DESIGN.md` and `design/AGENT-LAYER.md`. No cost constant changed, so I did not rerun the cost scripts, and the README and AWS totals stay as they are. `design/AGENT-LAYER.md` is now at 997 of its 1,000 lines, which leaves Writer Z almost no room.

## WK-Y1 (W5): held feed polls
All edits are in `design/AGENT-LAYER.md`.
- **Shape, feed block.** Learners serve feed polls. Followers serve them only while no learner answers. The leader and witnesses never do. A member holds polls only while it is following, which means its last AppendEntries left no gap and it is not catching up by `InstallSnapshot`. A member that stops following answers its held polls at once with their cursor and no events, and refuses new ones.
- **The cap.** I added `FEED_HELD_MAX = 1_000` per member [guess]. A member at the cap refuses the poll, and the edge tries up to two other serving members. If all of them refuse, the client gets 429 with `Retry-After` set to `FEED_BATCH_ALL`.
- **Learner placement.** Each member reports its peak held count with its read rate. `place_learners` adds learners until the peak fits in half of `FEED_HELD_MAX` per learner [guess], up to `MAX_LEARNERS`.
- **Other sections updated:** Summary (A1 line), "Tasks and the activity feed", A1 steps 4 and 11, the Scale feed row (250 held per follower, 4x headroom, 10 learners at 5,000 agents), the API reference 429, three `edge and fold additions` failure rows, and the closing paragraph of Failure modes.
- **Cursor text.** WriterX2 asked me to change the 410 sentence for child groups, and I applied that text.

## WK-Y2 (W14): insights verdict entry
- **Shape, overlaps block.**
  - `InsightJob` now has one producer, the `Insights` consumer's cursor. It sends one job per entry that writes or deletes a `PathSet` row, or opens a PR on its ref. Nothing comes from `apply`.
  - The new `InsightVerdict` struct is a core row per ref. It stays on `Repo(repo)` after a metadata split, which WriterX2 asked for. `graftyard-insights` commits it through `POST .../insights`.
  - `decide` refuses a `head` that is no longer the ref's tip, and writes nothing when the result equals the stored row. Otherwise one entry holds the row plus, as metadata, the changed check runs and the label changes.
  - The "changed" check, the feed's `Verdict` event, the PR and agents pages, and `graftyard_overlaps` read only this row.
  - Clusters are connected components of overlapping `PathSet` rows, named by the lowest PR number. A label is removed when its cluster dissolves.
- **Agent block, both files.** `AgentRepoEvent` gains `InsightVerdict` (Core, with its check runs and labels as Metadata).
- **Other sections updated:** the `Verdict` feed variant now states the severity order, the API reference has the `/insights` endpoint, the "First-party Apps" table, the GHES compatibility table, and the `insights consumer` failure rows.

## WK-Y3: Code Mode opt-in
- **Agent block, both files.** `OrgPolicy` gains `code_mode: bool` (default false). I reflowed the existing lines, so the block did not grow.
- **`CodeMode` struct.** `enabled_orgs` is gone. `/codemode` serves a token only when every repository's owner is an org with `code_mode` on. User-owned repositories never get it.
- **API reference.** `org_agents` gains `code_mode`. `GET /orgs/{org}/agents` also accepts the `Graftyard MCP` client credentials, which `graft-mcp` uses to read the setting.
- **Other sections updated:** Summary, the GHES compatibility table, Deployment modes, and step 5 of "Enable agents".

## Decisions beyond the item text
- One entry carries the verdict row, its check runs and its labels. That makes the "changed" check survive a crash and a redelivered job. After a split, X2's parent-first `meta_finish` rule covers the two tiers.
- The host sets `pr`. The consumer computes `cluster`.
- Closed PRs keep their labels.
- `graft-mcp` caches the `code_mode` setting for 60 s [guess].
- I first defined "following" by ack age, then dropped it. On an idle group whose leader lease lapses, followers would have stopped serving the feed.

## Gate rows
| Milestone | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| Milestone 0 (layer) | A1 replay on one `Tri` group with no learners, plus 5,000 held feed polls with `paths`, then again after `place_learners` runs | Commit p99 at most 300 ms throughout, zero `overloaded` replies to AppendEntries, at most 1,000 polls held per member with the rest answered 429 with `Retry-After`, zero polls on the leader, and zero on followers after placement | Commit p99 over 1 s, any `overloaded` replication reply, any member above 1,000 held, or any poll on the leader |
| Milestone 0 (layer) | 1,000 polls held on a follower, then its AppendEntries dropped for 5 s so the next one leaves a gap, then an `InstallSnapshot` catch-up | Every held poll returns within 1 s with its cursor and no events, new polls are refused until it catches up, and no event is lost | Any poll held or accepted while it is behind, or any event skipped |
| AM4 | A5 replay with every `InsightJob` delivered twice, plus a stale job after a newer push | One `InsightVerdict` entry per changed result, none for a repeat, 409 for the stale head, one check run per changed check, and every feed `verdict` event at an `InsightVerdict` index | Any duplicate entry or check run, or a verdict for a moved head |
| AM4 | 1,000 agent pushes across a leader change | Exactly one `InsightJob` per `PathSet` entry, all from the `Insights` cursor, none from `apply` | Any second job for one entry |
| AM4 | PRs 4, 7 and 9, where 4 overlaps 7, 7 overlaps 9, and 4 does not overlap 9. Close 4, then close 7 | `overlap:4` on all three, then `overlap:7` on 7 and 9, then no `overlap:` label on 9. Closed PRs keep their labels | Any other name, or a label on an open PR that is in no cluster |
| AM4, after base Milestone 7 | Split the metadata tier of a repository that has live `InsightVerdict` rows | Feed cursors keep working, verdict rows and `Verdict` events stay on `Repo(r)`, and the check runs and labels reach `RepoMeta(r)` | Any lost verdict, event, check run or label |
| AM4 | `/codemode` for an org repository with `code_mode` off, then on, then off; once for a user-owned repository; a `PATCH` of `code_mode` by a non-admin | Refused while off and for the user repository, served while on, refused within 60 s of turning it off, and the non-admin `PATCH` refused | Any Code Mode call served while off or on a user repository |

## Open questions
1. Does a Durable Object have an undocumented cap on requests it holds open, and does it answer `overloaded` to AppendEntries while holding 1,000 polls? `FEED_HELD_MAX` is a guess until the first Milestone 0 row above runs.
2. `place_learners` runs once a day. Until it runs, polls beyond the two followers' 2,000 get 429. Should held-poll placement run more often? That is the base's "Sites and placement" section, which I don't own.
3. `GroupLoad` is used in `design/DESIGN.md` but never defined. It needs a held-poll field.
4. If a human removes an `overlap:` label, the consumer puts it back only on the next change.
5. `CHECK_WRITES_PER_PUSH` in `scripts/cost_agents.py` counts every check run as its own command. One `InsightVerdict` command now carries both, so A1's 2.08/s is an upper bound. I left the script unchanged.

## For Writer Z
- `build/BUILD-PLAN.md` AM4-WP2 still says `PrSync`'s apply enqueues `InsightJob`. It needs the cursor producer, `InsightVerdict`, and the `/insights` endpoint.
- The AM1 feed build list in `build/BUILD-PLAN.md` lacks `FEED_HELD_MAX`.
- AM4-WP3 in `build/BUILD-PLAN.md` lacks `OrgPolicy.code_mode`.
- In `design/AGENT-LAYER.md`, the paragraph that starts "A stronger form of the router" (one line plus its blank line) repeats the Alternatives bullet. Cutting it would give your "Third walk" entry two more lines.

## Line counts and checks
- `design/AGENT-LAYER.md`: 997 lines (cap 1,000).
- `design/DESIGN.md`: 1,899 lines (cap 1,900). My share there is one line.
- `check_design.py`: DESIGN.md OK, AGENT-LAYER.md OK, agent block OK, cost figures OK.
- `check_names.py`: OK, 34 files.
- `check_links.py`: OK, 1,360 citations.
- I removed `scripts/__pycache__`.