# fanout: concurrent validate in one shard, `fanout`, and the 500-agent reruns on graft-m2b (2026-11-10)

Branch `fanout`, cut from main `aa9e097`. Everything hosted ran on **graft-m2b**. Nothing touched `graft-m1-*`, no
load went to bf20 (`repo:65536`), and B0-30 was not run.

<!-- SUMMARY -->

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

**Plants** (`tests/fanout/plant.py`, `tests/fanout/raw/plants.log`): **20 of 20 caught**. Five in the Go container
(unserialized same-pack rename, a sweep of a young quarantine, ingest taking the shared lock, and two `queue_depth`
miscounts), three in `CatchUps::fresh`, three in `LoadReports`, one in `combine`, two in `push_validator` and
`object_name`, three in the fold and the wire tag (k 0 before any `Fanout`, no row, tag 40, inserted before
`PathSet`), and two in the site (the ranges read leaves out k; the release does not read 32.15). In the first run,
plant 0 was missed by `TestConcurrentValidatesOfOneNetwork` alone; `TestFinishValidateOfTheSamePackAtOnce` was added
and catches it.

**Full checks** (`tests/am1/runs/checks.sh`, `tests/fanout/raw/checks.log`): fmt, workspace clippy, wasm32 clippy for
site and edge with and without `test-faults` and for ops, `cargo test --workspace` (**605 passed**, 0 failed), the
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
`tests/am1/raw/runs/a1-<row>-f<tag>-<row>.log`. "503 gate" is the edge's "the repository's access rows did not answer"
(`workers/edge/src/lib.rs` and `api.rs`, the read gate's `access_rows`).

| Row | Run (repo) | Pass line | Measured | Verdict |
| --- | --- | --- | --- | --- |
| B0-29 | `mwa0i4ps` (131193), 600 s, k 4 | "Commit p99 at most 600 ms, merge p99 at most 2 s, zero `Stale` on distinct refs, leader under 600 req/s with zero feed polls" | Agent pushes **105 of 2,500 ok**: 1,897 × 503 gate, 440 client timeouts (120 s), 2 `stale`. Commit p50/p99 30,092/30,092 ms (n 9). Merge p99 120 s. Leader 0 → 2, term 19; 499 failed member calls. Leader inbound in 600 s: `/read` 16,777, `/suspect` 20,199. 0 feed polls on the leader. Feed events p99 29.7 s. API reads 2,361 × 200, 9,660 × 503 | **FAIL** (commit p99 over 1 s) |
| B0-32 | `mwa1m4wt` (131194), 180 s, k 4 | "Every push acknowledged within 5 s or answered in an HTTP 200 with `ng <ref> agent admission full, retry after <n>s` … zero leader changes, and zero `suspect` reports" | Burst: **0 of 500 within 5 s**; 323 × 503 gate, 132 × 500 "created, but no member leads", 43 timeouts, 2 errors, none in the `ng` shape. Agent pushes 169 of 762 ok. Leader 0 → 2 (term 8); `/suspect` 18,651 on the leader | **FAIL** (other refusal shape, leader change, `suspect`) |
| B0-34 | `mwa265zb` (131195), 600 s, home shard killed at 300 s (200), k from `fanout` | "k set in band from `NetworkLoad` … human push p99 at most 3 s beyond transfer, and clone time to first byte p99 at most 1 s throughout" | **k stayed 1**. Of the first shards' `/fanout` reports in the window, 27 ended in an exception on the member (Workers Logs) and 2 were ok. Clone TTFB p50/p99 **23.2/172.4 s** (109 × 200, 226 × 503). Human pushes 0 of 37 attempted ok (30 × 503 gate). Agent pushes 6 of 2,501 ok. Commit p50/p99 304/361 ms (n 90). Leader 0 → 1, term 13. 0 Cloudflare API calls (none on any request path) | **FAIL** (TTFB over 5 s; k not set in band) |
| B0-31 | `mwa2sahl` (131196), 600 s, leader moved to site 2 (apac) first, k 4 | "Commit p99 at most 1 s with buckets up" | Agent pushes **133 of 2,500 ok**: 1,777 × 503 gate, 341 timeouts, 194 refused. Commit p50/p99 2,225/2,508 ms (n 12). The apac leader didn't hold: the final leader was site 0, term 26. The placement-failing half was not reached | **FAIL** |
| B0-33 | `mwa3al9y` (131197), 5,000 agents, 300 s, k 4 | "Human and merge-queue commit p99 at most 1 s, at least 140 agent commands/s admitted, the user token admitted as `Agent`, every refusal in the git `ng` or REST 429 `retry-after` shape, zero leader changes" | Agent pushes **62 of 12,546 ok** (11,415 × 503 gate). Agent side commands about 2.5/s admitted (taskEdit 129, comment 147, lease 79, standIn 194 ok in 300 s). Fast user 21 of 489 × 200. Commit p50/p99 1,423/3,415 ms (n 608). Leader held (site 0, term 15) with 2,934 failed member calls | **FAIL** (admitted rate, refusal shape) |
| L0-12, A1 replay with 500 pollers | `mwa3mbsl` (131198), 600 s, 500 tasks and 500 Shared leases held, a leader kill every 60 s, k 4 | "500 long-pollers see each event within FEED_BATCH plus 2 s (3 s) at p99 outside leader kills" | Feed: **0 events delivered** to the pollers in 600 s, from 211,151 polls (144,136 no status, 40,783 `gone`, 13,488 `notServing`, 6,087 `lagging`, 6,657 held). Kill pushes: 10 sent, 0 ok (9 × 503 gate, 1 timeout), so no kill landed; the leader still moved 0 → 2 (term 18). Agent pushes 41 of 2,490 ok. Lease renews 6 of 1,495 × 200 (1,485 × 503) | **FAIL** (no events seen; the kills did not land) |

<!-- ROWS2 -->
