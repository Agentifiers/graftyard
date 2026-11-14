# LM-1 rows on graft-m3f: B0-31, B0-32, L0-12

Three Milestone 0 gate rows, split off from LM-1 ([thread](https://ampcode.com/threads/T-01a1c694-6112-72b4-83ca-dc9337a9646e), `tests/am1/results/lm1-memory.md`) to run hosted on a second install, `graft-m3f` (`tests/am1/results/install-m3f.md`). Each row is judged exactly as `build/BUILD-PLAN.md` writes it, on one run, with no substitution. LM-1 keeps B0-29, B0-33, B0-35 and B0-34 on `graft-m2b`. This thread: [LM-1 rows on graft-m3f](https://ampcode.com/threads/T-01a1d805-b0ec-7624-a63c-99341738c925). Branch `lm1-rows-m3f`.

## Build

`lm1-rows-m3f` is main `7c989fb` plus these changes:

- **LM-1's row drivers**, from `origin/lm1-memory` `ff27a11`: `rows.sh`, `smoke.sh`, `a1.mjs` with `clock.mjs`, `stall.mjs`, `transfer.mjs`, `learners.mjs`, `teardown.py --sweep`, `terms.mjs`, and `siteday.sh`. The branch's one product change, the queue split in `workers/site/src/host.rs`, is instrumentation and was not taken.
- **`ROWS_ENV`** names the install's environment for `rows.sh`, `smoke.sh` and `a1.sh` (default `tests/m3/rows/env.sh`, graft-m2b's). `tests/fanout/rows-m3f.sh` runs the rows with `tests/am1/lf13/env-m3f.sh`, which sets `INSTALL=graft-m3f`, so the teardowns and the sweep touch only graft-m3f's shards.
- **The admission trace.** `Member::admission` also reports the two moving averages that `Admission::capacity` is sized from, `round_ms` and `cmd_bytes`. The member's `/status` carries the leader's admission (capacity, roundMs, cmdBytes, queued, agentQueued, agentBytes, hot, pressure). `tests/fanout/admit.mjs` polls it every second beside a row (`TRACE=1`), next to `terms.mjs`'s leader trace.
- **L0-12's script gaps.** On m22, the replay's 540 tasks filled `TASKS_OPEN_PER_REQUESTER` (100) for every requester, m3-ci's own included. The stall's task creates then got 422 "The requester has too many open tasks". With no task id, every lease acquire read "An App token must name its session", and the refusal part could not create its holder task. `a1.mjs` now cancels the replay's tasks after the row line, which also releases their leases. `stall.mjs` now stops on a refused task create instead of reporting exit 0.

Shard image `sha256:de39c828…`, built from main's `containers/git` (`b4a7510`: the `/ui` routes, which `f6d2ef40` predates), on `standard-4`.

## Site day

2026-11-14, 05:02Z to 05:17Z, `tests/fanout/siteday-m3f.sh`, logged in `install-m3f.md` on main. Sites `4440f04d` / `664f2927` / `dfd53b45` (phase 1 `9f3d53c2` / `03930d80` / `fea147c4`), edge `a197b7fd`, built from `lm1-rows-m3f` `7e5b63f`. After the deploy, `GET /repos/m3org/j4` answered 200 and `repo:65536`'s leader answered the new `/status` admission fields. The install's own Apps (`tests/am1/rest/apps.mjs`, for L0-12's Graftyard-run and MCP kinds) were registered at 05:59Z: `graftyard-workspace` 196608, `graftyard-agent` 262144, `graftyard-mcp` 327680, installations 7190011 to 7190013 on `m3org`.

## Rows m3fr1 (2026-11-14, `tests/fanout/rows-m3f.sh`, TAG `m3fr1`)

Every row ran on a fresh repository with learners enam 4,000, weur 2,000, apac 2,000 (8 learners), k 4 granted on both warm sites, and the row's shards torn down after it. Raw files: `tests/am1/raw/a1-<row>-<run>.jsonl`, `tests/fanout/raw/{terms,admit}-m3fr1-<row>.jsonl`, and `tests/fanout/raw/rows-m3fr1.status`. All of them are git-ignored.

### B0-31: fail

Repository 65552, run `mwfyd102`, 05:19:58Z to 05:29:58Z, 500 agents, every agent push 1 MiB, placement failing from 300 s. `/move` homed the group on apac (`rehomed on 2`). The apac leader held for the whole row at term 3, with 0 `/suspect` reports, 3 failed calls, and 87.5 req/s on the leader.

| Gate line | Measured | Verdict |
|---|---|---|
| Commit p99 at most 1 s with buckets up | 1,165 ms at the leader (p50 797, n 9,278). The leader's `/stats` covers both halves, so the buckets-up half alone is not split out. Queue p99 930, round p99 297, confirm p99 22 ms | Not shown to pass: 1,165 ms is over 1 s and under 3 s, and the buckets-up half is not measured apart |
| While placement fails: Human and MergeQueue commit p99 at most 3 s, 0 of them refused | Human pushes 140 ok, 13 `stale:race`, 5 `stale`, 0 refused. Push p99 21.0 s end to end, most of it validation on the shards (below). The per-class commit time is not recorded. The 1 merge sent in this half got no answer for 542 s (client status 0), so it did not commit within 3 s | **Fail** (MergeQueue) |
| Agent pushes over `INLINE_MAX_HOT` refused at once in the `ng` shape | 1,242 of 1,261 refused `full` (`ng … agent admission full`), push p50 2.4 s. The 19 that were accepted [inference] were pushes that started before 300 s and finished after it: the driver labels a push with its half when it ends | Pass on the refused shape |
| Fail line: over 3 s, or any Human or MergeQueue push up to 1 MiB refused | The merge waited 542 s | **Fail** |

Buckets up, 1,238 of 1,239 agent pushes committed (the other was a client fetch failure), agent push p99 10.6 s. Feed p99 1,517 ms.

**The admission trace answers LM-1's hypothesis: refuted on this build.** `tests/fanout/admit.mjs` read the leader's `capacity`, `cmdBytes` and `roundMs` every second:

| Half | Capacity, min / p50 / max (commands/s) | `cmd_bytes` p50 / max | Agent commands queued, p50 / max | Round p50 / max (ms) | `pressure`, `hot` |
|---|---|---|---|---|---|
| Buckets up (299 samples) | 150 / 247 / 369 | 1,804 / 3,089 | 3 / 26 | 221 / 250 | never, never |
| Placement failing (300 samples) | 130 / 178 / 252 | 2,575 / 3,498 | 2 / 17 | 216 / 239 | never, never |

Capacity fell by about a quarter while placement failed, because the 20 KB human packs it keeps inline raise `cmd_bytes`, but it stayed 5 to 9 times the 27.6 commands/s offered. No agent command waited near `AGENT_WAIT` (commit p99 1.2 s, not 30 s). The hypothesis named 1 MiB human packs, but `a1.mjs` sends 20 KB human packs in every row. Its 1 MiB packs are agent packs, which admission stages (buckets up) or refuses (placement failing) and never takes inline. m22's 30 s queue therefore did not come from admission capacity on this build. m22 ran on main `84d29c2`, before m0-carried-8 (`25ea425`), which stopped counting agent inline packs against `ENTRY_SOFT`. What failed m22 is not established from these data.

**Where B0-31's time went now: the git cache shards.**

- **Merges hung in the shard.** The queue's `/merge` calls on the network's shards ran until they were canceled at the edge's 180 s (11 of the 19 `/merge` calls Workers Logs shows for the row), and test merges took 170 to 570 s. The merge sent while placement failed is one of them. The merges that did answer waited 32 s in the shard (`shard:waited;dur=31999`).
- **Human validates waited on cold pool shards.** Shard `validate` wall time while placement failed was p50 905 ms and p99 29.3 s (270 calls). The slow ones were forwards to pool shards (`pool=1`) that were warming, and the edge's 15 s home wait (`VALIDATE_WAIT_HOME_S`).

### B0-32: fail

Repository 65553, run `mwfyyh4b`, 05:35:53Z to 05:38:53Z. The burst was 500 agent pushes of 256 KiB in 1 s, at 60 s. The leader held at term 2 on site 0, with 0 `/suspect` reports and 6 failed calls.

| Gate line | Measured | Verdict |
|---|---|---|
| Every push acknowledged within 5 s, or answered 200 `ng … agent admission full` | 494 of 500 acknowledged and 0 within 5 s, burst push p50 15.5 s. The other 6 timed out at the client's 120 s, with no answer. No push was refused | **Fail** |
| Queued agent bytes under `AGENT_QUEUE_MAX` | Peak 155 KB (`admit.mjs`) against 64 MiB, and at most 52 agent commands queued | Pass |
| Every merge in one of the next 2 entries, none delayed over 2 s | merge:only p99 1,547 ms (8 merges, 5 × 200, 3 × 409 `Base branch was modified`) | Pass on time. Entries per merge are not recorded |
| Zero leader changes, zero `suspect` | Term 2 throughout (`terms.mjs`), 0 `/suspect` reports at the leader | Pass |
| Fail line: any lost acknowledgment or any other refusal shape | 6 burst pushes without an answer in 120 s | **Fail** |

The commit path was not the bottleneck: leader commit p99 1,462 ms (queue p99 1,101 ms, round p99 549 ms). The time was in validation. 207 of the 274 burst validates logged in the burst's first minute ran on the network's first shard (`git#0#net:65553`), at wall time p50 15.0 s and p99 26.8 s. That p50 is the edge's 15 s home wait. The 67 forwarded to pool shards took p50 3.2 s. Both pool shards answered `warming` early in the burst (`fanout: git#0#pool:N is warming`), so `POOL_COLD_SKIP_MS` kept pushes on the first shard for 30 s. m22's 106 × 503 `warming` refusals are gone: 8 agent pushes outside the burst got one. Feed p99 4,050 ms.

### Product gap found: `RepoFold` marks no writes for admission

`Fold::write_mark` defaults to `None` (`crates/raft/src/fold.rs`), and the repository fold (`crates/git/src/repo.rs`, `impl Fold for RepoFold`) does not override it. Only the test `Kv` fold does. As a result, on every repository group:

- `hot` never turns true: the trace read `hot: false` in every sample of B0-31 and B0-32, at about 4 pushes/s against `HOT_PUSHES` 2.
- The hot-group staging rule (`INLINE_MAX_HOT` with `hot` from the log) never fires.
- DEMOTE_PER_MIN never demotes anyone.

B0-31's and B0-32's packs were staged anyway, by `AGENT_INLINE_ENTRY` (128 KiB). So this gap did not decide either row, but B0-32 is written to test `INLINE_MAX_HOT` on a hot group. It is left for the operator's decision, not fixed here.

**Operator decision (2026-11-14):** B0-31 and B0-32 are invalid as run, whatever their numbers, because neither tested a hot group. Fixed on branch `repo-write-mark` (below), to rerun on a fresh site day once it is on main.

## Follow-ups after m3fr1 (branch `repo-write-mark`)

### 1. `RepoFold::write_mark`

Commit `a4b9499`. There is one mark per write command, on the event the command is for. `push: true` is set only for a `Refs` that moved a client ref; PrSync's `refs/pull/` moves are host work. Task API events (`TaskCreated`, `TaskUpdated`) and events with no actor mark nothing. Companion events in the same entry (`Provenance`, `PathSet`, `AgentMeta`, `AgentGate`, `RequiredCheck`, and `Settings` beside `Created`) do not mark again. Each `RepoEvent` variant was checked against DEMOTE_PER_MIN's text in DESIGN's agent block.

Tests on three real members through `RepoFold` (`crates/git/tests/agent_gates.rs`):

- 10 pushes in `HOT_WINDOW` leave the group cold, and 11 turn it hot. 11 issues never do.
- The 60th write in a minute (30 pushes and 29 issues, then the probe) demotes a Human-classed user token to Agent. Another user stays Human.

Three new plants in `tests/am1/wp1/plant.py` (23 to 25) were all caught: no marks, no push mark, and every mark a push.

### 2. B0-31's 542 s merge

Where it waited: in `merge_then_catch_up`, on the shard's catch-up, with no deadline.

- The merge-queue `/merge` calls on site c's shard (`git#2#net:65552`, the apac leader's site) missed a commit and waited for the catch-up in flight.
- That shard's `warm` catch-ups behind the hot log ran 172 to 581 s (Workers Logs `POST /catchup?warm=1`, 05:23:26 and 05:30:29). The 05:30:29 one ended with "The container has not been started" at the row's teardown.
- So `/merge` held until the edge canceled it at 180 s, and the client's request never got an answer.
- It was not a bucket PUT and not a stage retry: the merges that did answer took 80 to 250 ms in the shard (`shard:merge;dur=81`).
- The same hang hit the buckets-up half too (merge `@up` 183 to 547 s), so it does not depend on placement.

The fix is commit `8acb4c5`: the whole merge-then-catch-up is bounded by `MERGE_WAIT` (15 s [guess], the edge's `VALIDATE_WAIT_HOME_S`). Past it:

- The shard answers 503 `warming`.
- The edge answers the merge 503 with `retry-after: 5`.
- The merge-queue consumer retries its ask.
- The catch-up goes on in the background.

A merge that succeeds is never cut. New test: `a_merge_behind_a_catch_up_that_never_ends_answers_warming_at_its_deadline`.

The slow catch-ups themselves are a second cause: the apac shard took minutes to apply a 4 push/s log tail of 1 MiB agent packs. The bound stops them from hanging merges, but merges on that shard are refused while it is that far behind.

The driver now splits commit p99 by half (`lm1-rows-m3f` `5a72c7e`): it reads the leader's `/stats?summary=1&reset=1` at `placeFailAt`, so `leader.commitMsByHalf` gives `{up, fail}`.

### 3. B0-32: cold pool shards and the 6 lost acknowledgments

**Were the pool shards cold only because the group was new?** Yes. Repository 65553 was created at 05:31:09Z. The first push went at 05:35:53Z, and the burst at 05:36:53Z. The pool shards (`git#0#pool:3/5/6`) first rehydrated 65553 at 05:36:03Z, 10 s into the row, and answered `warming` again at 05:37:29Z in the burst. A group that had been taking pushes before the window would have had warm pool shards. This run does not show shards going cold on a group already under load. Driving warm-up load on the group before the burst fits the row as written.

**The 6 pushes with no answer.** One of the six is traced end to end in Workers Logs (trace `90d35ddb`, started 05:36:53, the burst's first second):

1. 44 s passed in the edge before its first member read, at 05:37:36. Workers Logs records nothing for that interval. It is probably the edge taking in 500 concurrent 256 KiB bodies, but that is not shown.
2. Site a's first shard answered 503 `warming` at its 15 s wait (05:37:52).
3. Site b's shard answered 503 `warming` at its 60 s wait (05:38:53).
4. The edge request was then canceled at 120 s (05:38:53.26), which is the driver's `AbortSignal.timeout(120_000)` on the client, before site c's 60 s try could answer.

No member held that push. It never reached `/commit`. The other five have no edge event in Workers Logs (the query finds one canceled burst push), so they are not traced. Their client-side shape is the same: `TimeoutError` at 120 s. The edge's validate chain (15 + 60 + 60 s) is longer than the driver's 120 s timeout. The driver now waits 200 s (`5a72c7e`), so each push ends in the edge's answer: a 200, or a 503 `no shard could check the pack`.

Either answer still fails B0-32's gate (acknowledged within 5 s, or `ng … admission full`). The real failure is validation on cold shards: first-shard validates at the 15 s home wait while the pool shards rehydrated. That is what the warm-up and `write_mark` (`INLINE_MAX_HOT` staging on a hot group) should change.

## Cost
