# fanout: concurrent validate in one shard, `fanout`, and the 500-agent reruns on graft-m2b (2026-11-10)

Branch `fanout`, cut from main `aa9e097`. Everything hosted ran on **graft-m2b**. Nothing touched `graft-m1-*`, no
load went to bf20 (`repo:65536`), and B0-30 was not run.

**Summary.**
- **Validate concurrency.** One network validates up to 39/s on 1 vCPU and 58/s on 4 at 8 concurrent pushes,
  against about 16/s before (section 1). Shared catch-ups take the per-validate leader round trip off the shard.
- **`fanout`.** Built: `RepoEvent::Fanout` (tag 39, repository format 32.15), `/load`, `LoadReports`, rendezvous
  `push_validator`, and pool shards. Hosted, a forced k = 4 spread validates over pool shards (p50 456 ms). The
  in-band step from `NetworkLoad` was not shown: a follower's `/fanout` answer was an error. That is fixed in
  `105c228` but not deployed.
- **500-agent rows.** Every rerun fails: B0-29, B0-31, B0-32, B0-33, B0-34, B0-35, and L0-12's A1 replay. The limiter
  moved from the validate lock to the edge's read gate: one leader `/read` per git request for installation tokens,
  then `/suspect` storms, member resets and 503s (section 3, "What limits"). At 100 agents every push lands, with
  commit p99 885 ms.
- **B0-35's setup** now opens 500 of 500 PRs. Its 10 of 500 in this package's first try came from a driver parser
  bug, not the gate (`994266c`). am1-runs' 10 of 500 very likely had the same cause.
- **LF-13** reached real install faults in 4 of 6 runs and a real gap in 1 of 6. Both halves fail their line
  (section 3, LF-13).
- **Checks.** 22 of 22 plants caught. Full checks pass (606 tests). Hosted cost $4.46 against the $15 stop.

## 1. Concurrent validate inside one shard

**The change** (`containers/git/server/main.go`, `07d91af`):

- `handleValidate` takes the network's lock **shared** (`RLock`) instead of alone.
- What a validate writes:
  - its own `graft/incoming-*` quarantine (`os.MkdirTemp`, unique per call);
  - the pack spooled into it;
  - `index-pack --strict --fix-thin` with `GIT_OBJECT_DIRECTORY` set to the quarantine.
- What a validate only reads, through alternates:
  - the repository's objects and refs, for the connectivity check (`rev-list --not --all`) and `details()`.
- The one step two validates share is setting the quarantine aside as `graft/validated-<sha256>`, which
  `RemoveAll`s and renames. Two pushes of byte-identical packs race on it, so it takes a per-network mutex
  (`finishValidate`).
- Nothing else changed:
  - The writers still take the lock alone: `/ingest` (adopt and `index-pack` into `objects/pack`), `PUT`/`POST
    /refs`, the geometric repack `/refs` starts, `/init`, and `/evict`. So a validate never reads a pack a repack is
    deleting, and an adopt never moves a `validated-*` directory a validate is writing.
  - `sweepValidated` removes only directories older than an hour (`PACK_COMMIT_WINDOW`), so a running validate's
    young quarantine is safe.

**Correctness** (`containers/git/server/concurrent_test.go`, run with `-race`):

- `TestConcurrentValidatesOfOneNetwork` runs 36 validates at once on one network: 12 branches, each pushed 3 times
  with byte-identical thin packs, all touching one shared blob. Beside them runs a loop of `/ingest` of every
  validated pack so far plus `PUT /refs`. The repository holds `repackAt + 2` packs, so each `PUT /refs` starts a
  geometric repack.
- Afterwards the test checks:
  - every validated pack ingests and is recorded;
  - every tip is reachable (`rev-list --objects`);
  - `git fsck --strict` is clean;
  - no `incoming-*` is left behind.
- `TestFinishValidateOfTheSamePackAtOnce`: 200 rounds of 8 same-sum renames behind a barrier.
- `TestLoadReportsCPUAndValidatesRunning`: `/load` counts a validate waiting for the lock and drops it when it ends.

**Validates per second for one network in the container**:

- Setup: `tests/fanout/bench.py`, 20 s per point, the image built from `containers/git`. Each push is a ~30 KB thin
  pack, a new branch off a 200-commit `main`, posted to `/validate` with its tip and a detail line, as the site's
  shard sends it.
- "before" is `aa9e097`'s image and "after" is this branch's. Raw data: `tests/fanout/raw/bench.jsonl`.

| CPUs (docker `--cpus`) | Concurrent pushes | Before: validates/s (p50 / p99 ms) | After: validates/s (p50 / p99 ms) |
| --- | --- | --- | --- |
| 1 (`standard-2` is 1 vCPU) | 1 | 17.1 (58 / 71) | 17.1 (57 / 79) |
| 1 | 2 | 16.6 (121 / 144) | 21.9 (91 / 116) |
| 1 | 4 | 16.2 (247 / 302) | **38.7** (99 / 177) |
| 1 | 8 | 14.3 (591 / 653) | **39.0** (200 / 393) |
| 4 | 1 | 14.9 (67 / 88) | 17.0 (58 / 71) |
| 4 | 2 | 16.5 (121 / 164) | 21.1 (93 / 144) |
| 4 | 4 | 15.1 (255 / 323) | 34.1 (110 / 204) |
| 4 | 8 | 15.7 (496 / 661) | **58.2** (124 / 243) |

- Before, one network validates about 16/s whatever the concurrency: the lock serializes it.
- After, it reaches 39/s on one vCPU (2.4×) and 58/s on four (3.7×). On one vCPU, a validate is about half git CPU and
  half waiting on disk and process start, so two to four in flight fill the CPU.
- In `push` mode each validate is followed by its `/ingest` and `POST /refs`, which take the lock alone. That
  sequence runs at 13.0/s before and 13.0/s after at 1 concurrent push, and 9.4/s against 9.8/s at 8. The commit
  path's writers still serialize, as they must.
- Hosted, before this change, one network validated about 2.7 pushes/s. The site logs of the am1-runs 250- and
  500-agent runs (`v250`, `v500`) show 410 and 430 validates in about 150 s each, with wall p50 429 and 1,892 ms. That
  is far below the container's 16/s, so the hosted bottleneck was not the container alone (see section 2, shared
  catch-ups).

## 2. `fanout`

**Shared catch-ups first** (`b8fc23e`, `crates/git/src/merge_cache.rs`, `CatchUps::fresh`).

- The hosted 2.7/s was the shard object, not the container. Each `/validate` ran its own catch-up in turn
  (`CatchUps::turn`), each catch-up reads the group's leader, so one network's validates queued behind one another's
  leader round trips.
- `fresh()` returns once a catch-up that started after the request arrived has succeeded. Requests that arrive while
  one runs share the next one, so a burst of n validates costs two catch-ups, not n. Validates, clones, and fetches
  use it; `/catchup`, `/promote`, `/merge`, and `/archive` keep their own turn.
- The `x-graft-rehydrated` header had no reader and was dropped; `x-graft-cursor` now comes from the per-network
  index the latest shared catch-up reached.
- Catch-up gates and cursors are keyed by shard object and network (`0a15b2a`), so a pool shard sharing an isolate
  with a first shard catches up its own container.

**`fanout`** (`545cc55`). The pure `fanout` and `NetworkLoad` were already in `crates/git/src/shard.rs`; this adds what
runs them.

- **Container** `GET /load`: `cpu_p99`, the 99th percentile of the last 60 one-second samples of the cgroup's
  `cpu.stat`, as a share of the container's vCPUs; `queue_depth`, validates waiting for the lock or running.
- **Record.** `RepoEvent::Fanout { k }`, appended as wire tag 39 (line added to `crates/git/tests/wire_tags.golden`),
  under **repository format 32.15** (`REPO_FORMAT_FANOUT`). It writes the core row `fanout`; `RepoFold::fanout_k` is 1
  before any `Fanout`. The site reads 32.15; the edge's `FORMATS_ENABLED` is unchanged, since only the leader writes
  it (by its own entry, not through the edge).
- **Pure pieces** (`crates/git/src/shard.rs`):
  - `LoadReports`: the leader's latest report per warm site and the hysteresis. A report older than `LOAD_STALE`
    (180 s) counts as idle, and a site that is not warm is ignored. `report` answers a k only when it differs from
    the committed one.
  - `combine`: a site's report is its most loaded shard's CPU and the summed queue.
  - `push_validator`: the shard among `shards_for(network, site, count, k)` that rendezvous on the push id picks.
  - `object_name`: the first shard keeps `git#<site>#net:<repo>`, so no network moves when fan-out ships; pool shards
    are `git#<site>#pool:<n>`, `n` in `1..1 + HOT_POOL`, shared between networks.
- **Site** (`workers/site/src/fanout.rs`, `workers/site/src/shard.rs`):
  - The first shard reads k from every catch-up's `ranges` read (no extra read).
  - Once a minute, on its own validates, it reads its k shards' `/load`, combines them, and posts the result to the
    group leader's `/fanout`. A network with no pushes reports nothing.
  - The leader folds the report and, when k steps, records `Fanout { k }` as its own entry and waits for it.
  - The first shard forwards each `/validate` (`&via=0` and the placement record) to the shard `push_validator`
    picks, by the pack's trailer (its own SHA-1, the same on every re-send). The pool shard catches up, then
    validates. A pool shard never forwards; if it can't answer, the first shard validates the push itself (logged
    `fanout: <pool>: <why>; validating on the first shard`).
  - Test-faults `/bench_seed {"fanout": k}` commits k at once, so a row can name its pool shards before load.
- No API call or cron: the report rides the shard's own requests and the commit is the leader's entry.
- `crates/raft` is unchanged (`git diff aa9e097 -- crates/raft` is empty), so the long sweep and Porcupine were not
  rerun.

**Tests.** `crates/git/tests/agent_load.rs`:
`the_leader_folds_minute_reports_into_one_k_for_both_warm_sites`, `pushes_spread_over_the_k_shards_by_push_id`,
`a_site_reports_its_most_loaded_shard_and_the_summed_queue`; `crates/git/tests/push.rs`:
`a_fanout_entry_sets_the_committed_k`; `workers/site/src/fanout.rs`: `the_ranges_read_answers_the_committed_k`;
`merge_cache::tests::fresh_shares_one_catch_up_among_the_requests_that_wait_for_it`; the three Go tests above.

**Plants** (`tests/fanout/plant.py`, `tests/fanout/raw/plants.log`): **22 of 22 caught** in the final run (04:26Z). Five in the Go container
(unserialized same-pack rename, a sweep of a young quarantine, ingest taking the shared lock, and two `queue_depth`
miscounts), three in `CatchUps::fresh`, three in `LoadReports`, one in `combine`, two in `push_validator` and
`object_name`, three in the fold and the wire tag (k 0 before any `Fanout`, no row, tag 40, inserted before
`PathSet`), three in the site (the ranges read leaves out k; the release does not read 32.15; a follower answers a
`/fanout` report without the `notLeader` shape), and one in the A1 driver (push results matched by text, plant 21). In the first run,
plant 0 was missed by `TestConcurrentValidatesOfOneNetwork` alone; `TestFinishValidateOfTheSamePackAtOnce` was added
and catches it.

**Full checks** (`tests/am1/runs/checks.sh`, `tests/fanout/raw/checks.log`): fmt, workspace clippy, wasm32 clippy for
site and edge with and without `test-faults` and for ops, `cargo test --workspace` (**606 passed**, 0 failed, at `fbec385`), the
three `scripts/check_*.py`, and `go test ./...` in `containers/git/server`: all exit 0. The three Go tests above also passed under `-race` when they were written.

## 3. Hosted on graft-m2b

### Deployed

- **Site day** on graft-m2b from `e46da45`, which holds `dfa5410` (the feed ring restarts after a hole, so an older
  cursor gets `gone`) and `89cf205` (GraphQL label mutations route by `labelableId`). Image
  `registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:fd537ab42f4cd6c02770e0640af0f4a896dbab4f509525a7295086e447b0dab7`.
  - Sites a/b/c: `81dc79fd-9b7e-4a67-b2b2-4fd7bdc91bdb`, `11009d5b-f131-41e1-96ce-2165d427fa37`,
    `00a81536-cb06-4fc1-83a2-60c360ed51f1`. The first pass of the same day was `7a7fe4ec` / `c2b9c0da` / `813682a5`.
  - Roll: 115 instances. 54 answered `kill 200, catchup 200`. 10 got catchup 500, from ingest 422 "did not receive
    expected object" on old diag and a1 repositories 131170–131184 (the prsync 422 am1-runs saw). The rest were on
    stale or other sites and answered `fail(7)`.
- **Edge** `b9f4291e-1f42-45af-8bfb-c4003d394442`, from `e46da45`, with the same `FORMATS_ENABLED` as before
  (32.15 is written only by the leader's own entry, so it isn't in the edge's list) and `--ops graft-m2b-ops`. Ops
  unchanged (`c53e4459`). Logs: `tests/am1/raw/runs/fanout-siteday.log`, `fanout-edge.log`.
- The drivers changed after the deploy (`3bad6f9`, `87b1a46`, `8acd520`, `bb7dfa7`); no site or edge code did.

### Like-for-like against am1-runs (reads and pollers off)

| Run | Agents | k | Agent pushes ok | Commit p50/p99 | Validate wall (site logs) |
| --- | --- | --- | --- | --- | --- |
| am1-runs `diag-2` (before) | 100 | 1 | 148 of 148 | 460/933 ms | mean 405 ms |
| `fdiag2` (131191) | 100 | 1 | **142 of 142** | **360/885 ms** | p50/p90 276/384 ms |
| am1-runs `a1-smoke` (before) | 500 | 1 | 541 of 1,241 | — | p90 17.7 s, max 58 s |
| `fdiag5` (131192) | 500 | 4 from about 7 s in | 453 of 1,253 (671 × 503) | 559/751 ms (n 19) | first shard p50 1.1 s; via pool p50 456 ms |

At 100 agents commit time fell about 100 ms. At 500 the validates spread and stay short (pool p50 456 ms, against
a p90 of 17.7 s before), but no more pushes get through. The limiter has moved off the container.

### The load rows (`tests/fanout/rows.sh`, one fresh repository per row)

Each row ran `tests/am1/rest/a1.mjs` at journey 9's 500 agents (B0-33 at 5,000), with reads and 500 pollers on. Before
each row except B0-34, k was set to 4 through the leader's test-faults `/bench_seed {"fanout": 4}`, so the pool
shards were named before load. B0-34 left k to `fanout`. Driver rows: `tests/am1/raw/a1-<row>-<run>.jsonl`; logs:
`tests/am1/raw/runs/a1-<row>-f<tag>-<row>.log`. `tests/am1/raw/` is gitignored (`tests/am1/.gitignore`), so these
stay on the orb, as in am1-runs; the numbers here are copied from them. "503 gate" is the edge's "the repository's access rows did not answer"
(`workers/edge/src/lib.rs` and `api.rs`, the read gate's `access_rows`).

| Row | Run (repo) | Pass line | Measured | Verdict |
| --- | --- | --- | --- | --- |
| B0-29 | `mwa0i4ps` (131193), 600 s, k 4 | "Commit p99 at most 600 ms, merge p99 at most 2 s, zero `Stale` on distinct refs, leader under 600 req/s with zero feed polls" | Agent pushes **105 of 2,500 ok**: 1,897 × 503 gate, 440 client timeouts (120 s), 2 `stale`. Commit p50/p99 30,092/30,092 ms (n 9). Merge p99 120 s. Leader 0 → 2, term 19; 499 failed member calls. Leader inbound in 600 s: `/read` 16,777, `/suspect` 20,199. 0 feed polls on the leader. Feed events p99 29.7 s. API reads 2,361 × 200, 9,660 × 503 | **FAIL** (commit p99 over 1 s) |
| B0-32 | `mwa1m4wt` (131194), 180 s, k 4 | "Every push acknowledged within 5 s or answered in an HTTP 200 with `ng <ref> agent admission full, retry after <n>s` … zero leader changes, and zero `suspect` reports" | Burst: **0 of 500 within 5 s**; 323 × 503 gate, 132 × 500 "created, but no member leads", 43 timeouts, 2 errors, none in the `ng` shape. Agent pushes 169 of 762 ok. Leader 0 → 2 (term 8); `/suspect` 18,651 on the leader | **FAIL** (other refusal shape, leader change, `suspect`) |
| B0-34 | `mwa265zb` (131195), 600 s, home shard killed at 300 s (200), k from `fanout` | "k set in band from `NetworkLoad` … human push p99 at most 3 s beyond transfer, and clone time to first byte p99 at most 1 s throughout" | **k stayed 1**. Of the first shards' `/fanout` reports in the window, 27 ended in an exception on the member (Workers Logs) and 2 were ok. Clone TTFB p50/p99 **23.2/172.4 s** (109 × 200, 226 × 503). Human pushes 0 of 37 attempted ok (30 × 503 gate). Agent pushes 6 of 2,501 ok. Commit p50/p99 304/361 ms (n 90). Leader 0 → 1, term 13. 0 Cloudflare API calls (none on any request path) | **FAIL** (TTFB over 5 s; k not set in band) |
| B0-31 | `mwa2sahl` (131196), 600 s, leader moved to site 2 (apac) first, k 4 | "Commit p99 at most 1 s with buckets up" | Agent pushes **133 of 2,500 ok**: 1,777 × 503 gate, 341 timeouts, 194 refused. Commit p50/p99 2,225/2,508 ms (n 12). The apac leader didn't hold: the final leader was site 0, term 26. The placement-failing half was not reached | **FAIL** |
| B0-33 | `mwa3al9y` (131197), 5,000 agents, 300 s, k 4 | "Human and merge-queue commit p99 at most 1 s, at least 140 agent commands/s admitted, the user token admitted as `Agent`, every refusal in the git `ng` or REST 429 `retry-after` shape, zero leader changes" | Agent pushes **62 of 12,546 ok** (11,415 × 503 gate). Agent side commands about 2.5/s admitted (taskEdit 129, comment 147, lease 79, standIn 194 ok in 300 s). Fast user 21 of 489 × 200. Commit p50/p99 1,423/3,415 ms (n 608). Leader held (site 0, term 15) with 2,934 failed member calls | **FAIL** (admitted rate, refusal shape) |
| L0-12, A1 replay with 500 pollers | `mwa3mbsl` (131198), 600 s, 500 tasks and 500 Shared leases held, a leader kill every 60 s, k 4 | "500 long-pollers see each event within FEED_BATCH plus 2 s (3 s) at p99 outside leader kills" | Feed: **0 events delivered** to the pollers in 600 s, from 211,151 polls (144,136 no status, 40,783 `gone`, 13,488 `notServing`, 6,087 `lagging`, 6,657 held). Kill pushes: 10 sent, 0 ok (9 × 503 gate, 1 timeout), so no kill landed; the leader still moved 0 → 2 (term 18). Agent pushes 41 of 2,490 ok. Lease renews 6 of 1,495 × 200 (1,485 × 503) | **FAIL** (no events seen; the kills did not land) |

**A driver bug in these counts (`994266c`).** `pushResult` in `a1.mjs` found `ok` by the regex
`(^|[^a-z])ok refs/`. The status line's pkt-line length goes just before it, so a length ending in a hex letter
(`002cok refs/…`) failed the match, and an accepted push was counted `error`. Which refs it hit depends only on the
ref name's length:
- **B0-29, B0-31, B0-32, B0-34, L0-12**: the agents' own refs (`claude/a1-<run>-<i>`, i < 500) are all unaffected.
  The 503, 500 and timeout counts above are HTTP statuses and are unaffected too. B0-32's burst refs (`…-<i>-b`)
  are 490 of 500 affected, but none of the burst's 500 pushes was an HTTP 200 (the burst row above), so its count
  stands.
- **B0-33**: 4,000 of its 5,000 agent refs (i ≥ 1,000) are affected. Its 269 `error` results are HTTP 200 replies
  that may have been accepted, so its agent pushes ok are **62 to 331 of 12,546**, not 62. Every other B0-33 count
  above stands, and the row fails either way.
- **B0-35's setup**, in am1-runs and in this package's first B0-35 (`fr2-b035`, 131199): 490 of 500 PR refs
  (`…-pr<n>`, n ≥ 10) are affected. The setup retried each "error" 5 times; the retries got `stale`, because the
  first push had landed. So it opened 10 PRs and stopped (`open PRs 10/500 {"push error":490,"push stale":1960}`),
  while the chain held 450 of the PR branches. am1-runs' "Setup opened **10 of 500** PRs" ran the same parser on
  the same ref shape, so it very likely has the same cause rather than the gate. Its raw log isn't in this worktree,
  so I didn't recheck it.
- The fix reads each status line by its pkt-line length (side-band framed or not). `--selftest` now checks a
  `002cok` line and a side-band reply, and plant 21 (the old regex) is caught.

### B0-35 (500 open PRs)

Pass line: "Test-merge vCPU within `TEST_MERGE_BUDGET`, Actions dispatch p99 at most 10 s, and zero computes from list
fields".

| Run (repo) | Setup | Measured | Verdict |
| --- | --- | --- | --- |
| `mwa4unw0`, `fr2-b035` (131199), 03:30Z | 100 tasks; **10 of 500 PRs** opened (`{"push error":490,"push stale":1960}`), the parser bug above | Stopped at 03:52Z before its load window. Not a valid run | invalid |
| `mwa6q48c`, `fr3-b035` (131200), 04:23Z, k 4, with `994266c` | 100 tasks; **500 of 500 PRs opened**, no failures, in 7.5 min (500 PR branches at about 130 a minute, then 500 `createPullRequest`) | 600 s at journey 9 plus 500 `mergeable` pollers at one poll per 5 s. `mergeable` polls: **10 `MERGEABLE`, 10,162 × 503**, 47,961 skipped by the client cap. Agent pushes **0 of 2,500 ok** (2,466 × 503 gate, 34 timeouts). API reads 17 × 200 of 10,361 sent. Side commands 0 ok (every one 503). The chain recorded 40 test merges in the window (`testMerged`, all clean). Leader inbound `/read` 12,168, `/suspect` 20,653; leader site 1 at the start, site 0 at the end (term 18). Actions dispatch not measurable: no Actions on m2b | **FAIL** (the setup is now valid; the load fails at the gate's 503s) |

The setup half the task asked for is fixed: 500 PRs open. The row's own measures (test-merge vCPU against
`TEST_MERGE_BUDGET`, and computes from list fields) aren't instrumented in `a1.mjs`. Neither could be read here,
since 99.9% of the polls got 503 before reaching a fold. The 40 recorded test merges in 600 s are far below the 10
base moves a minute × 500 PRs that a compute-everything design would do. But with almost every poll refused, that
proves nothing.

### What limits the 500-agent rows now

The validate lock is no longer the limiter. At 100 agents every push lands (`fdiag2`). At 500, with k = 4, the
validates that do run take about 0.5–1 s. But the network's group can't serve its reads, and every git request
waits on those reads first. The chain, with numbers from the runs above:

1. **Each git request reads the group's leader before any byte.** Since `1c47d28` the edge's read gate
   (`workers/edge/src/lib.rs`, `read_gate`) reads the repository's access rows (`api::access_rows`, `/read` on the
   group's leader through `graftyard_lookup`) and the owner's `Authz`, for every receive-pack, upload-pack and REST
   read. Its 10 s cache covers only stored user tokens (`gate::key`). The agents push under m3-ci installation
   tokens, which are never cached, so every agent push and every API read by an App costs one leader `/read`.
2. **At journey 9's rates that is about 260 leader reads/s**: 4.17 pushes/s, each as info/refs plus receive-pack,
   plus 200 API reads/s and 33 ref reads/s. Measured leader inbound `/read` was 16,777 in B0-29 and 23,335 in B0-34,
   with no leader holding long enough to count the rest. The gate was deployed in am1-runs too (`0172f55` holds
   `1c47d28`), where the validate queue hid it. am1-runs' B0-29 already showed the same 503 count shape (1,772 × 503).
   Removing the validate bottleneck exposed it.
3. **A read the leader doesn't answer within `CALL` (3.5 s) counts as missed**, and `graftyard_lookup` reports a
   missed member with `/suspect` to the other voters (`crates/lookup/src/lib.rs`, the "Members that missed" pass).
   Measured `/suspect` on the leader: 20,199 (B0-29), 18,651 (B0-32), 28,105 (B0-34).
4. **Members then reset and step down.** Site logs in the B0-34 window: 23 `StepDown { CommitErrors }` and 15
   "Durable Object's isolate exceeded its memory limit and was reset". Leaders moved in B0-29, B0-32, B0-34 and
   L0-12. A read during an election answers `not leader` or `no answer`, which the edge returns as **503 "the
   repository's access rows did not answer"**: 1,897 of 2,500 agent pushes in B0-29, 11,415 of 12,546 in B0-33.

So the gate's design is correct (a 503, never a 404, when the rows don't answer), but at agent scale its per-request
leader read is the new limiter. Per the task ("report exactly what broke and don't work around it in the gate"),
the gate is unchanged. What would remove it, for the next package, not built here:
- serve `access_rows` from a follower or the shard's catch-up, at the same `fresh` floor;
- or cache installation-token decisions too, keyed by installation id and its permissions hash, for `READ_CACHE`.

This is my inference from the counts above. I didn't run an isolating experiment (for example the same row with
the gate's cache extended to installation tokens), so the share of the 503s due to the gate's reads, against the
member memory resets that also happen under 500 agents' commits, is unmeasured.

**`fanout` itself, hosted:**
- k = 4 forced by `/bench_seed` held in every row (`k.mjs` read back 4 after each row), and validates went to pool
  shards: in `fdiag5`'s site-a logs (the sample `logs.py` returns, at most 100 events a window), 121 validates came
  `via=0` to a pool shard, p50 456 ms, and 160 ran on the first shard, p50 1.1 s.
- In-band stepping was not shown. In B0-34, 27 of the 29 `/fanout` reports in the window raised an exception on
  the member that got them (a follower answered `Err("not leader")`, a 500 that lookup counts as a missed member),
  and k stayed 1. `105c228` / `320b4a1` make a follower answer `notLeader`, which lookup follows; that is tested
  in memory (plant 20) but **not deployed**, since it needs another site day.

### LF-13 (`tests/fanout/lf13.sh`, `tests/am1/rest/lf13.mjs`)

Pass line (`build/BUILD-PLAN.md`, AM1-WP4): "Every held poll returns within 1 s with its cursor and no events, new
polls are refused until it catches up, and no event is lost". Each run holds 1,000 polls on a follower F (paths no
write touches, so only F stopping or the 25 s `FEED_WAIT_MAX` returns them), faults F, and lifts the fault. Columns:
held polls stopped by F (`stopped`), returned before the lift, returned within 1 s of the lift, p50/max ms after the
lift; new polls answered `lagging` of 50 probes (every 200 ms after the lift); lease events committed; F's answer
from the pre-fault cursor and the events missing on F against the third voter. Logs: `tests/am1/raw/runs/lf13-round{1,2,3}/`.

Three rounds. Round 1 ran before this package's driver fixes; round 2 with `8acd520` / `bb7dfa7`; round 3 with
`5232265` (gap: transfer once O holds an entry F lacks, since round 2 never saw the leader 3 ahead) and `fbec385`
(F's events compared once F serves again, up to 60 s).

| Run | Fault reached? | Stopped | Before lift | Within 1 s | After lift p50/max ms | `lagging` probes | Events: F / ref / missing on F |
| --- | --- | --- | --- | --- | --- | --- | --- |
| r2 gap 1 | no (transfer refused: leader moved first) | 757 | 1,000 | 0 | — | 0 | 2 / 1 / 0 |
| r2 gap 2 | no gap (`gapSeen` null) | 0 | 0 | 197 | 1,576 / 2,604 | 0 | 2 / 1 / 0 |
| r2 gap 3 | no gap | 0 | 0 | 0 | 4,787 / 5,788 | 0 | 2 / 2 / 0 |
| r2 install 1 | no (not truncated past F in 18 s) | 0 | 1,000 | 0 | — | 0 | 2 / 0 / 0 |
| r2 install 2 | **yes**, truncated to 635, F at 633, at 21.3 s | **509** | 0 | 388 | 1,334 / 1,496 | **47** | F `lagging` / 14 / 14 |
| r2 install 3 | **yes**, 762 past F at 757, 22.1 s | 0 | 207 | 572 | 788 / 1,558 | **45** | F `lagging` / 7 / 7 |
| r3 gap 1 | F's polls `notServing` (leader changed before the hold) | 0 | 1,000 | 0 | — | 0 | 0 / 0 / 0 |
| r3 gap 2 | **F stood still, O one ahead, moved** (F 110, O 111, then leader 113) | 0 | 0 | 0 | 3,458 / 4,566 | 0 | 3 / 4 / **1** |
| r3 gap 3 | no gap (O level with F) | 0 | 0 | 0 | 3,418 / 4,433 | 0 | 2 / 2 / 0 |
| r3 install 1 | no (not truncated in 18 s) | 0 | 1,000 | 0 | — | 0 | 2 / 1 / 0 |
| r3 install 2 | **yes**, truncated to 938, F at 936, 20.5 s | 0 | 494 | 424 | 492 / 1,080 | **46** | F `gone` (33 s after lift) / 17 / 17 |
| r3 install 3 | **yes**, 1,058 past F at 1,055, 21.7 s | **526** | 80 | 394 | 1,093 / 1,236 | **47** | F `gone` (35.6 s) / 0 / 0 |

**Verdict: FAIL**, on both halves, with real faults now reached:

- **Install** reached its fault in 4 of 6 runs. In each, new polls were refused (45–47 of 50 probes `lagging`) and
  every held poll came back with its cursor and no events. But F stopped its held polls in only 2 of the 4
  (`stopped` 509 and 526); in the other two the polls returned at their own 25 s wait. Only 388–572 of 1,000
  returned within 1 s of the lift, and some returned before it. The held polls start before the fault, so "within
  1 s" is measured from the lift, while F stops them when the install starts. The line doesn't say which. No event
  was lost: after an install F answers the pre-fault cursor `gone` (the ring rule `dfa5410` deployed), so its
  client restarts from the oldest it can serve. In round 2 F was still `lagging` 3 s after the lift; in round 3 it
  served again 33–36 s after the lift. The "missing" counts in those rows are that `gone`, not a loss.
- **Gap** was reached once in 6 runs (r3 gap 2): F stood still, O held one entry F lacked, and the transfer moved
  the lead to O. Even then there were **0 `lagging` probes** and **0 stopped polls**. F answered `hold` from the
  pre-fault cursor and had **one lease event fewer than the third voter** (3 against 4). A later compare found the
  three members' stored logs identical through index 117 (same digests). So the entries reached F, but F's
  feed ring served one event fewer than the other member's. I didn't isolate why. My inference: F was never marked
  `gap` (its first answer to the new leader was ok, so `stepped` saw no refused `AppendRes`). The new leader sent
  the gap entries in one round F could take, because F held their predecessor, and the leader's `next[F]` is its
  own `last + 1`. The miss is then something in the ring itself, a lease whose acquire and release both fell in one
  applied batch. That is an open bug candidate, not a measured cause.
- A driver flaw limits the probe column. The run holds exactly `FEED_HELD_MAX` (1,000) polls on F, so until they
  return, a new poll is answered `full` before any `lagging` check. In r3 gap 2 the first 8 probes (to 2.2 s
  after the lift) were `full`, then `hold`. A `lagging` window shorter than the held polls' return can't show. The
  next run should hold 999 or fewer.
- In the other gap runs F caught up from the same leader or O was level, so no gap formed. That is the raft rule
  `crates/raft/tests/lf13.rs` states, `rounds_lost_to_the_same_leader_leave_no_gap`. In 1 s steps the site fault
  can't be timed finely enough to place a refused round hosted.

## 4. Cost

`tests/fanout/cost.sh` (Workers Analytics since the site day, at overage prices; containers not counted): **$4.46**
at 04:41Z for every hosted run of this package: the site day, edge, smokes, the six load rows, two B0-35 tries,
`fdiag2`/`fdiag5`, and three LF-13 rounds. Against the $15 stop, `rows.sh` stops a new row at $12. The container
instances aren't in Analytics. A worst-case bound at `research/prices.json`'s container rates: 15 `standard-2`
instances (a first shard and 4 pool shards on each of 3 sites), 1 vCPU busy the whole time, 6 GiB and 12 GB each,
for 4 h. That is $4.32 vCPU + $3.24 memory + $0.18 disk = **$7.74**, so **$12.20 at most** in total, under the $15
stop [inference; the real instance count and busy time weren't read].

## 5. What LM-1 still lacks, besides B0-30

1. **A read path that holds journey 9 on one group.** The edge's read gate reads the group's leader once per git
   request and per App API read, uncached for installation tokens. That is about 260 leader reads/s at 500 agents,
   and in every 500-agent row it is the first thing to fail (section 3). The design's own budget for the leader is
   about 305 req/s in total. B0-29, B0-31 to B0-35 and L0-12's A1 replay can't pass until `access_rows` is served
   off the leader (a follower or the shard's catch-up at the `fresh` floor) or cached for installation tokens.
   Neither is built here, per the instruction not to work around the gate.
2. **`fanout` stepping k in band, shown hosted.** That needs `105c228` deployed (a site day), then B0-34 again.
3. **L0-12's feed and kills** under load: 0 events delivered and 0 of 10 kill pushes landed, both behind (1).
4. **B0-35's instruments**: test-merge vCPU against `TEST_MERGE_BUDGET`, and a count of computes from list fields.
   `a1.mjs` measures neither, and m2b has no Actions for the dispatch half.
5. **LF-13**:
   - an install fault whose held polls F stops in every run (2 of 4 here);
   - a gap fault that leaves F marked behind (0 `lagging` in the one real gap);
   - the one-event difference in r3 gap 2's F ring, explained or fixed;
   - and a driver that holds fewer than `FEED_HELD_MAX` polls, so a `lagging` answer isn't masked by `full`.
6. **B0-33** at 5,000 agents: its agent-push count is only bounded (62 to 331 of 12,546) because of the parser bug.
   A rerun after (1) settles it.

## 6. Design contradictions (quoted; no design file edited)

- `design/DESIGN.md`, journey 9, step 1: "its leader about 305 req/s". `design/AGENT-LAYER.md`, journey A1 step 4,
  counts 277 reads plus 27.6 commands. Neither counts the edge read gate's own leader `/read` per git request and
  App read since `1c47d28`, about 260/s more at journey 9's rates by this package's count. Measured: 12–23 k leader
  `/read`s per 600 s row, with the leader unable to hold.
- `design/DESIGN.md`, "Commit and read paths" (the `fanout` comment): "Each warm site's first shard of the network
  reports `NetworkLoad` each minute". Built: the first shard reports only while it validates pushes. A network with
  no pushes reports nothing, so its k steps down only through `LOAD_STALE`.
- `design/DESIGN.md`: "Pushes validate on any caught-up shard of the k, chosen by rendezvous on the push id". Built:
  the pick is by rendezvous whether or not the shard is caught up. The chosen pool shard catches up, then validates,
  and only if it can't answer does the first shard validate the push itself.
- `design/DESIGN.md`, journey 9, step 2: "`fanout` keeps k = 1 `standard-4` shard per warm site, at 67% of its 4
  vCPU", and "At journey 9's push every 2 minutes k is also 1, with 0 extra shards ($0 a month)". Hosted with k = 1,
  am1-runs' 500-agent validates queued to a p90 of 17.7 s. With the lock removed and k = 4, the limiter moved to
  the read gate (section 3). So k = 1 at journey 9 is not yet shown to hold.
- `build/BUILD-PLAN.md`, LF-13: "Every held poll returns within 1 s with its cursor and no events". The line doesn't
  say within 1 s of what. A held poll waits at most `FEED_WAIT_MAX` (25 s, `design/AGENT-LAYER.md`), and a fault that
  forces an `InstallSnapshot` here took 20–22 s to truncate past F. Held polls placed before the fault can time out
  first, so "within 1 s" of the lift or of F's stop separates runs that otherwise behave the same.

## 7. Commits on `fanout` (from `aa9e097`; not pushed, not merged)

`07d91af`, `b8fc23e`, `545cc55`, `0a15b2a`, `38da814`, `4f119f8`, `e46da45`, `4426d6d`, `3bad6f9`, `87b1a46`,
`8acd520`, `bb7dfa7`, `105c228`, `320b4a1`, `41d4e3b`, `994266c`, `5232265`, `fbec385`, and this results commit.
