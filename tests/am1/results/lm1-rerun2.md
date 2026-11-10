# LM-1 rerun 2 on graft-m2b: narrow agent reads, packed and trimmed tier reads (2026-11-10)

This round starts from `origin/main` `d8bed7c`, which includes `ccd10b1`: an agent push reads only
`{"rows": {"agent": true}}`, its `agent_policy` and `agent_task` rows, not the whole tier. Branch `lm1-rerun2`, cut
from `d8bed7c`. Only graft-m2b was used; graft-m1 was not touched (B0-30 runs there). Nothing is pushed or merged.

## Summary

- **Step 2 (API reads off): the narrow agent read was not enough on its own.** 250 agents: 491 of 624 pushes ok,
  with a leader change and 607 `exceededMemory` on site c. 500 agents: 300 of 1,232 ok, with a leader change and
  6,924 `exceededMemory` on site c. So the remaining whole-tier reads were still the cause, even with API reads off.
- **Where the memory went (measured, local).** A 740 KB whole-tier answer built as a `serde_json::Value` is a 13 MB
  tree in wasm32 and peaks at 24 MB per read (`/tmp/ar/vsize`, an allocator harness on the real `TierRows::dump`
  output). Ten of them in flight exceed a 128 MB isolate.
- **Fix 1, `eb8eb0b`: whole-tier reads answer one packed JSON string.** The member serializes the rows once into a
  string, and the edge parses that string straight into `TierRows`, so no `Value` tree is built on either side.
  After it, 500 agents with API reads off landed 812 of 1,254 pushes, against 300 of 1,232 before. It still moved
  the leader, and site b logged 1,621 `exceededMemory`.
- **Fix 2, `a271fb3`: the whole-tier read leaves out `path_set` and `provenance`.** No view reads either table from
  the tiers, and at 500 agents they were 1.12 MB of a 1.6 MB read.
- **Every row run still fails.** B0-29 ran after each fix and failed both times: 118 and 107 of 2,501 agent pushes
  ok, nearly all the rest the gate's 503 "access rows did not answer". Members still peaked at 263–267 MiB, even
  though a whole-tier read of the fresh repository is now about 150 KB. **The remaining memory limiter isn't
  identified** (section 4). B0-31 to B0-35 and the L0-12 A1 replay weren't run: there wasn't budget left, and with
  B0-29 failing on the same 503 they couldn't pass.
- **LF-13 (`HELD` 1000, 3 runs per mode) still fails both modes.** Gap left no gap in 3 of 3 runs. Install reached
  its fault in 2 of 3 runs, but held polls still returned after the 1 s limit: max 1,228 ms and 1,741 ms.
- **Cost: $11.19** with `m2bcost.py` from the r2 site day (13:28Z) to 16:49Z; containers were $9.26.

## 1. Deployed

All builds are from branch `lm1-rerun2`. Every site day reused the pushed image
`registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:172216ad0cbdf8779b0c20a698a0b5040a3de4295581408698ea3b157a8df3e9`,
because `containers/` hasn't changed since `e46da45`. Each day ran with `NOROLL=1`: shards are derived caches, and
each run uses a fresh repository whose shards start on the image (`tests/fanout/run-siteday.sh`, `IMG`/`NOROLL`).
The edge always ran with the same `FORMATS_ENABLED`
(`32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3`) and
`--ops graft-m2b-ops`.

| Day | Commit | Sites a / b / c (final) | Edge | Logs |
| --- | --- | --- | --- | --- |
| r2 | `d8bed7c` | `2902e126-57df-41dd-8af3-751dc5e4c17b` / `87d38e16-101b-4068-b730-d1936652c9d8` / `51b45855-e880-4316-bf12-da6c6c740195` (first pass `1ba6c4d7` / `d79548b8` / `50bf5b27`) | `caf1496c-9f2e-4de7-b354-270ee39bbace` | `tests/am1/raw/runs/fanout-r2-*.log` |
| r3 | `eb8eb0b` (fix 1) | `aa3464a5-6a56-40b6-b33c-3815fd10c89b` / `64fdf22d-7959-44db-a1f8-ef17d99e2fb7` / `015a56ba-1b16-4092-9185-0004bec3343c` (first pass `85a12778` / `0a325e72` / `279b1cdc`) | `1957be83-1926-4940-b7df-6a091c12b01d` | `fanout-r3-*.log` |
| r4 | `a271fb3` (fix 2) | `a888d8f0-8dc3-4a42-bab9-b3d0a1fad403` / `55f373d3-cb24-4ee7-aca8-14a3573af828` / `31ba3098-7fcd-4cc1-9e4e-96daa9b677ad` (first pass `9414737b` / `cf0dea6e` / `5a919126`) | `21fd29fb-6a4c-433a-bbbd-d5ab4970e4eb` | `fanout-r4-*.log` |
| edge only | `f0efb8c` (test edge `/_bed/kill_pool`, for the teardown) | unchanged (r4) | **`d579a6b0-01f0-4681-9523-ee1e33218994`** (current) | `fanout-r5-edge.log` |

The last edge deploy changes only a `test-faults` route, so it needed no site day.

## 2. Step 2: smokes with API reads off (`tests/fanout/smoke.sh`)

Each smoke uses a fresh repository with k 4 and runs for 300 s on `tests/am1/rest/a1.mjs`, with `--api-reads 0`.
Memory is the max `memoryUsageBytes` per member (`tests/fanout/memstats.py`). `exceededMemory` is a Workers Logs
outcome count per site script (`tests/fanout/count.py`, filtered by outcome and grouped by one field; see "Telemetry"
below). `/read` and `/suspect` are the leader's inbound counts from its `/stats` at the run's end.

| Smoke | Repo, run | Agent pushes ok | 503 gate | Leader | Member peak MiB (a/b/c) | `exceededMemory` | Leader `/read` | Leader `/suspect` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 250, reads off (r2, `d8bed7c`) | 131210, `mwaqjyfy` | **491 of 624** (78.7%) | 105 | 0 → 2 | 280 / 113 / 105 | site c 607 | 24,844 | 13,076 (about 13 k on each of f0 and f1) |
| 500, reads off (r2) | 131211, `mwar5e2c` | **300 of 1,232** (24.4%) | 771 | 0 → 2 | 290 / 208 / 56 | site c 6,924 | 28,880 | 35,640 |
| 500, reads off (r3, fix 1) | 131212, `mwaube7y` | **812 of 1,254** (64.8%); merges 18 ok; feed 7,960 events, p99 2.3 s | 386 | 0 → 1 at about 15:29Z | 268 / 132 / 85 | site b 1,621 (`/read` 1,522, `/feed` 77) | 920 (since the new leader) | 494 (since the new leader) |

Reading the table:
- The narrow agent read (`ccd10b1`) didn't hold 500, or even 250, with API reads off. The gate's 503s fell into
  the member resets, as in `lm1-rerun.md` section 3.
- Byte-sized evidence (measured on the members, with the `/tmp/ar/rsize2.mjs` and `/tmp/ar/par.mjs` probes): at 500
  agents the edge's whole-tier `{"rows": true}` read (`api::tier_rows`, used by every REST and GraphQL resolver and
  by the A1 driver's draft-PR, label, comment and human paths) was 740 KB, then 1.6 MB as `PathSet` rows
  accumulated.
- The local allocator harness on a captured 740 KB dump showed that `serde_json::Value` turns it into a 13 MB tree,
  with a peak of 24 MB per read. A member isolate is 128 MB, so a handful of these reads in flight is enough to
  exceed it. Fix 1 targets this.

## 3. Step 3: API reads on, and the fixes

With API reads off, 500 agents still failed. I ran B0-29 with reads on anyway, to see the step-3 picture under the
fix that was already built:

| Run | Repo, run | Agent pushes ok | Commit p50/p99 ms | API reads | Member peak MiB | `exceededMemory` | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |
| B0-29, fix 1 (r3) | 131213, `mwav2l4y`, 600 s, k 4 | **118 of 2,501** (1,690 × 503 gate, 488 timeouts, 198 × 500) | 13,106 / 37,954 (n 612) | 3,246 × 200, 9,092 × 503 | 273 / 247 / 201 | a 1,603, b 1,251 | **FAIL** |
| B0-29, fix 2 (r4) | 131214, `mwawb2ld`, 600 s, k 4 | **107 of 2,501** (1,770 × 503 gate, 540 timeouts, 78 × 500) | 1,757 / 3,967 (n 251) | 2,022 × 200, 9,532 × 503 | 267 / 265 / 263 | a 958, b 1,429, c 1,403 (site c mostly `/read`) | **FAIL** |

### Fix 1: packed tier reads (`eb8eb0b`; plants `3858058`)

- `TierRows::packed` (`crates/api/src/hosted.rs`) writes the dumped rows once as one JSON string, and
  `TierRows::from_value` reads either the packed string or the old object.
- A member's `read_json` answers `{"q": {"rows": true, "packed": true}}` with that string.
- The edge's `tier_rows_query` asks for `packed`, and parses the string straight into `TierRows`. A `Value` tree of
  every row no longer exists on either side.
- **No format change.** The packed answer is a read reply, not a stored record, and the edge reads both shapes, so
  an edge can run ahead of a site day.
- Tests:
  - `crates/api/tests/surfaces.rs` `a_hosted_mutation_renders_on_the_member_as_in_process` checks that the packed
    round trip renders the same as in process.
  - Site: `fanout::tests::a_packed_rows_read_answers_one_string_of_the_same_rows`.
  - Edge: `api::tests::the_tier_read_asks_for_packed_rows`.
- Plants 21–24 in `tests/fanout/plant.py`: **4 of 4 caught** (`tests/fanout/raw/plants-r2.log`).

### Fix 2: the whole-tier read leaves out `path_set` and `provenance` (`a271fb3`)

- Measured on 131212 at 500 agents: `path_set` was 1.12 MB of a 1.6 MB whole-tier read (500 rows of about 2.2 KB
  each). No REST, GraphQL or agent view reads `path_set` or `provenance` from the dumped tiers. The commands that
  need them read them on the member, through the fold.
- `TierRows::dump` skips `UNSERVED = ["path_set", "provenance"]`.
- Test `crates/api/tests/surfaces.rs` `the_tier_read_leaves_out_path_sets_and_provenance_only` captures every query
  the resolvers make, then checks that each one answers the same from the dumped rows as from the member's own
  tiers, and that only those two tables are left out.
- Plants 25–26: **2 of 2 caught**.

### Why it still fails: the limiter left after fix 2 (section 4)

On 131214 (B0-29 after fix 2), a packed whole-tier read is about 150 KB, a tenth of the 1.6 MB before fix 2, but
members still peaked at 263–267 MiB. So whole-tier dump size no longer explains the memory.

## 4. The remaining limiter (not identified)

**Measured, on 131214 (`mwawb2ld`):**
- Leader inbound by route over the run: `/read` 87 k, `/suspect` 108 k, `/feed` 58 k. These are from
  `tests/fanout/edgekinds.py` and the leader's `/stats` before it was reset; the driver's end-of-run `/stats` was
  taken after a restart, so it reads `/read` 1,219.
- Site c's `exceededMemory` events came in one burst at 16:23:11, each with `wallTimeMs` 0. A request rejected with
  0 ms of wall time means the isolate was already over its limit when that request arrived.
- `StepDown { CommitErrors }` at 16:22:19 (site c) and 16:22:37 (site a). After that, the gate's `access_rows` read
  found no leader during the elections and answered 503.
- In the 500-agent smoke after fix 1, the 15:27Z `/read` load on site a came from the Names, Tokens and Authz
  members, not the repository's (`tests/fanout/byobject.py`). Those small groups also take load from every gate
  and token check.

**Hypotheses (unverified, for the next round):**
1. **Concurrency rather than size.** `graftyard_lookup` fans a read to every candidate member. Each member then also
   serves 500 held feed polls and the `/suspect` storm. Many `/read`s in flight at once, each holding its rows,
   decoded queries and reply buffers, can add up past 128 MB even when each read is 150 KB. The test is
   per-request memory against concurrent requests in flight, on one member, with `/tmp/ar/par.mjs` at
   increasing parallelism after fix 2.
2. **Retained state per request or per poll.** A cache or a held-poll structure that grows with the number of
   pollers or reads and is not released. The test is a heap snapshot of a member under the A1 load, or a member
   read of its own `performance.memory`-style counters if exposed.
3. **The `/suspect` feedback loop.** Once one member resets, `/suspect` reports arrive at 23–34 k a minute, and each
   one costs the other members work and memory. That turns one reset into a cascade. A rate limit on `/suspect` per
   reporter would cut the cascade, but it would not stop the first reset.

**What I'd do next (not built; larger than this task's "small, read-path" scope):**
1. Measure members' memory against reads in flight, with fix 2 deployed, using `par.mjs` at 10, 30 and 60 in
   parallel, to tell (1) apart from (2).
2. If it's (1), cap in-flight `/read`s per member, answering 503 with `retry-after`, and send REST and GraphQL tier
   reads to one member, not to every candidate.
3. Collapse `/suspect` reports per reporter per second.

## 5. The rows

The rows use `tests/fanout/rows.sh` with `TEARDOWN=1`: a fresh repository per row, k = 4 through `/bench_seed`,
driving `tests/am1/rest/a1.mjs` at journey 9's 500 agents.

| Row | Pass line | Measured | Verdict |
| --- | --- | --- | --- |
| B0-29 | "Commit p99 at most 600 ms, merge p99 at most 2 s, zero `Stale` on distinct refs, leader under 600 req/s with zero feed polls" | After fix 2 (131214): agent pushes 107 of 2,501 ok. Commit p50/p99 1,757 / 3,967 ms. 5 `stale`. Leader stayed at site 0 but reset (term 22). After fix 1 (131213): 118 of 2,501 ok, commit p99 37,954 ms | **FAIL** |
| B0-31 | "Commit p99 at most 1 s with buckets up" | Not run | **not run** |
| B0-32 | "Every push acknowledged within 5 s or answered in an HTTP 200 with `ng <ref> agent admission full, retry after <n>s` … zero leader changes, and zero `suspect` reports" | Not run | **not run** |
| B0-33 | "Human and merge-queue commit p99 at most 1 s, at least 140 agent commands/s admitted …, zero leader changes" | Not run | **not run** |
| B0-34 | "k set in band from `NetworkLoad` … human push p99 at most 3 s beyond transfer, and clone TTFB p99 at most 1 s throughout" | Not run | **not run** |
| B0-35 | "Test-merge vCPU within `TEST_MERGE_BUDGET`, Actions dispatch p99 at most 10 s, and zero computes from list fields" | Not run | **not run** |
| L0-12, A1 replay, 500 pollers | "500 long-pollers see each event within FEED_BATCH plus 2 s (3 s) at p99 outside leader kills" | Not run | **not run** |
| LF-13 gap / install | Section 6 | Section 6 | **FAIL / FAIL** |

**Why B0-31 to B0-35 and L0-12 weren't run:**
- All of them run the same journey-9 load as B0-29, which failed after both fixes on the same gate 503s during member
  resets. They couldn't pass until the limiter in section 4 is removed.
- When B0-29 r4 ended, the round stood at about $10. Each 500-agent row cost about $1–2.5 with its containers in this and the last round [inference from the running totals], so the
  remaining rows would have reached the $15 cap without telling us anything new.

**Deviations from the order asked:**
- "B0-35 before LF-13" wasn't followed. B0-35 wasn't run at all. The `rows.sh` service ran LF-13 straight after
  B0-29, because `NO_LF13` was unset. LF-13 doesn't depend on the agent load, so its result stands on its own.

## 6. LF-13 (both modes, 3 runs each; `HELD` 1000)

`tests/fanout/lf13.sh` driving `tests/am1/rest/lf13.mjs`, from the `rows.sh` service. Logs are in
`tests/am1/raw/runs/flf13-<mode>-<n>.log`, with steps in `tests/am1/raw/lf13-<mode>.jsonl`. **This run used
`HELD=1000`, not the 999 of the last round**, so the follower's 1,000 held polls reach `FEED_HELD_MAX`, and new
probes can answer `full` before the fault rather than `lagging`. The 1,000 held polls themselves are what the row
judges.

Pass line (`build/BUILD-PLAN.md`, AM1-WP4): "Every held poll returns within 1 s with its cursor and no events, new
polls are refused until it catches up, and no event is lost".

| Run | Fault reached? | Stopped by F | Before lift | Within 1 s after | After-lift p50/p99/max ms | `lagging` of 50 probes | Events (F / ref / missing on F) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| gap 1 | no gap: F and O both at 150 when F stopped; fault 33.5 s | 0 | 1,000 | 0 | — | 0 | 2 / 1 / 0 |
| gap 2 | no gap (156/156) | 0 | 0 | 0 | 2,527 / 3,357 / 3,367 | 0 | 2 / 1 / 0 |
| gap 3 | no gap (162/162) | 0 | 0 | 0 | 3,587 / 4,490 / 4,516 | 0 | 2 / 2 / 0 |
| install 1 | no: not truncated past F during the fault (`truncated: null`) | 0 | 1,000 | 0 | — | 0 | 1 / 1 / 0 |
| install 2 | **yes**: truncated to 1,508, F at 1,505, at 22.5 s | 0 | 548 | 447 | 338 / 1,193 / **1,228** | **46** | 1 / 0 / 0 (F served 82 s after the lift) |
| install 3 | **yes**: truncated to 1,617, F at 1,615, at 20.9 s | 376 | 0 | 396 | 1,156 / 1,740 / **1,741** | **46** | F `gone` / 15 / 15 (see below) |

- All 6,000 held polls came back with their cursor and no events.
- **Gap: FAIL.** As in every earlier round but one, no run left a gap. Each time F stopped, the other follower was
  level with it, so the transfer elected a leader whose first round matched F. The task's "a fault that really
  leaves a gap" is still not placed hosted. The fault is a site-level member fault, at
  `SITE_FAULT_TTL_MS` resolution, and the transfer happens after F is already level. I didn't build the
  `test-faults` hook that would make the leader skip F's next `AppendEntries`.
- **Install: FAIL.**
  - In both runs that reached the fault, new polls were refused (46 of 50 `lagging`).
  - But held polls weren't all returned within 1 s: 447 and 396 within 1 s, max 1,228 and 1,741 ms. In run 2, 548
    returned before the lift. In run 3, 376 were stopped by F, which is the first time F stopping its polls was seen
    hosted, but the remaining 624 still came back 1–1.7 s after.
  - Install 3's "missing on F 15" is F answering `gone` from the pre-fault cursor, after an install to a log
    truncated past it. That is the ring rule (`lf13.mjs` records it that way), not a lost event. The reference
    member, which still held the range, listed 15 lease events.

## 7. Cost

From `tests/fanout/cost.sh` with `T0_FILE=tests/fanout/raw/siteday-r2.t0` (13:28:18Z), which runs
`tests/fanout/m2bcost.py` over graft-m2b only, containers included, at `research/prices.json` overage prices.

- **$11.19 at 16:49Z**, of which **containers $9.26**. Workers: 563,853 requests, 4.78 M CPU ms. DO: 4.10 M
  requests, 738,901 rows written, 197,721 active s. Under the $15 cap.
- **Containers dominate, and they keep billing with no load.** A torn-down network shard is started again by
  background work, such as the members' prsync warm catch-ups and archive alarms. Each one then bills its 6 GiB and
  12 GB until it is torn down again, about $3/h for the set.

## 8. Teardown

- Every running network shard was torn down repeatedly: `tests/fanout/raw/teardown-r2-1.log` to `-9.log`, 13, 11
  and 14 shards in the last three passes, all 200.
- **Pool shards `git#0#pool:1-7`: torn down, 7 of 7 (200).** No row needs them any more. `teardown.py` couldn't
  reach them, because `/_bed/kill` names only `git#S#net:R`. So `f0efb8c` adds a test-only `/_bed/kill_pool?site=&pool=`
  on the edge and `teardown.py --pools`, and edge `d579a6b0` carries it. The container API then listed all seven as
  `stopped`.
- A few network shards (131149, 131150, 131210–131212) started again within minutes of the teardown, as section 7
  describes. The final pass at about 16:55Z (`teardown-r2-10.log`) tore down 7 of them, all 200, and no pool shard
  was running. Shards that background work restarts after that keep billing until the next teardown, or until
  `WARM_FOR` (24 h) passes with no requests.

## 9. Telemetry pitfalls (verified)

- `tests/fanout/count.py` with two groupBys silently drops most rows. Filter by `$workers.outcome` and group by one
  field.
- Workers Logs and the Analytics datasets lag by minutes, so a count taken at a run's end undercounts.
- The leader's `/stats` resets when the member resets, so its end-of-run inbound counts cover only the time since
  the last reset (131214's 1,219 `/read` against 87 k from the logs).

## 10. Design contradictions (quoted; no design file edited)

- `design/DESIGN.md`, journey 9, step 1: "its leader about 305 req/s". This counts requests, not their size or how
  many are in flight at once. Measured this round: the per-push read was cut to the agent rows (`ccd10b1`), and
  whole-tier reads were cut from 1.6 MB to about 150 KB (fixes 1 and 2), yet a 128 MB member isolate still exceeded
  its memory at 500 agents. The design's sizing of the repository leader doesn't account for the member's memory
  under journey 9's concurrent reads and 500 held feed polls.
- The contradictions in `tests/am1/results/fanout.md` section 6 and `lm1-rerun.md` section 7 still stand:
  - in-band `NetworkLoad` only while validating;
  - the rendezvous pick not limited to caught-up shards;
  - k = 1 at journey 9 not shown;
  - LF-13's "within 1 s" of what.

## 11. What LM-1 still lacks, besides B0-30

1. Repository members that hold journey 9 in 128 MB. Fixes 1 and 2 removed the size of whole-tier reads; the
   remaining cause is in section 4. Until it's fixed, B0-29, B0-31 to B0-35 and the L0-12 A1 replay fail at the
   gate's 503 during member resets.
2. B0-34's `fanout` stepping k in band, shown hosted.
3. B0-35, with test-merge vCPU and list-field computes instrumented.
4. LF-13:
   - for gap, a fault that places a gap exactly;
   - for install, F stopping all its held polls when the install begins.

## 12. Commits on `lm1-rerun2` (from `d8bed7c`; not pushed, not merged)

- `75c353e`: `run-siteday.sh` `IMG` (reuse a pushed image) and `NOROLL`; `smoke.sh` (memory smokes with API reads set).
- `eb8eb0b`: whole-tier reads answer one packed JSON string, not a `Value` tree (fix 1).
- `3858058`: plants 21–24 for the packed tier read, 4 of 4 caught.
- `320d46d`: edge drops the import `TierRows::from_value` made unused.
- `a271fb3`: the whole-tier read leaves out `path_set` and `provenance` (fix 2), with plants 25–26.
- `f0efb8c`: test edge `/_bed/kill_pool`; `teardown.py --pools`.
- This results commit.

Full checks at `f0efb8c` (`tests/am1/runs/checks.sh`, log `tests/fanout/raw/checks.log`): see section 13.

## 13. Checks

At `f0efb8c`, every check passes (`tests/am1/runs/checks.sh`, log copied to `tests/fanout/raw/checks.log`):
- `cargo fmt --all --check`;
- workspace clippy;
- wasm32 clippy for the site and the edge, each with and without `test-faults`, and for ops;
- `cargo test --workspace`: **642 passed, 0 failed**, which is the 641 at `320d46d` plus fix 2's test;
- the three `scripts/check_*.py`;
- `go test ./...` in `containers/git/server`, cached, since `containers/` is unchanged.

Plants: 21–24 caught 4 of 4 and 25–26 caught 2 of 2, so 6 of 6 for this round's new code. `f0efb8c` adds only a
`test-faults` route for the teardown, and no plant was written for it. The teardown itself shows it works: 7 of 7
pool shards answered 200 and then listed as `stopped`.

## 14. After the scope change: B0-35 and the 500-agent rows, no LF-13 (17:00–17:42Z)

**Scope change (operator, after section 13):**
- LF-13 moved to its own thread and a new install, `graft-m3c`. It is no longer run on graft-m2b, and section 6 is
  the last graft-m2b LF-13 record.
- Two clauses moved from LM-1 to AM3-WP3: L0-12's lease refusal for the Graftyard-run kind, and B0-37's session-ref
  listing. Neither is counted below as an LM-1 failure.
- The freed budget went to B0-35 and the 500-agent rows: `ROWS="b0-35 b0-32 b0-31 b0-33 l0-12 b0-34"`,
  `NO_LF13=1`, `TEARDOWN=1`.
- `STOP_USD=13.4` (`4ccef7c`, a new `rows.sh` knob) means no new row starts once the round has cost $13.40. That
  leaves room for one row's overrun under $15.

**Deployed:** unchanged from section 1. Sites are r4 (`a271fb3`), and the edge is `d579a6b0-01f0-4681-9523-ee1e33218994`
(`f0efb8c`).

| Row | Run (repo) | Pass line | Measured | Verdict |
| --- | --- | --- | --- | --- |
| B0-35 | `mwaxscha` (131215), 500 agents, 600 s, k 4 | "Test-merge vCPU within `TEST_MERGE_BUDGET`, Actions dispatch p99 at most 10 s, and zero computes from list fields" | **Setup opened 500 of 500 PRs** (plus 100 of 100 tasks) in about 4.5 min. Then, from about 17:08:30Z to the end, **every edge request answered 503**: receive-pack 10 × 200 of 2,464, upload-pack 0 of 9,747, GraphQL 20 × 200 of 17,156, REST v3 0 of 6,070, graftyard v1 0 of 3,252 (`edgekinds.py`, 17:07:53–17:17:53Z). Member peaks 279 (f1) / 56 / 51 MiB; site b 2,380 `exceededMemory`. The driver crashed when the run ended (leader `/stats` had no body), so it wrote no summary row; `a0cf57a` fixes that. Test-merge vCPU and list-field computes are still not instrumented, and m2b has no Actions to dispatch | **FAIL** |
| B0-32 | `mwayfpmp` (131216), 180 s, k 4 | "Every push acknowledged within 5 s or answered in an HTTP 200 with `ng <ref> agent admission full, retry after <n>s` … zero leader changes, and zero `suspect` reports" | Burst **12 of 500 ok**: 410 × 503 gate, 56 timeouts, 21 × 500, 1 refused, none in the `ng` shape; p50 27 s. Agent pushes 333 of 757 ok. Leader 0 → 2. Leader inbound `/read` 46,430, `/suspect` 63,043, `/campaign` 497. Member peak 318 MiB (f2); site c 2,314 `exceededMemory` | **FAIL** |
| B0-31 | `mwayqfnk` (131217), 600 s, leader moved to site 2 first, k 4 | "Commit p99 at most 1 s with buckets up" | Agent pushes **126 of 2,501 ok**: 1,582 × 503 gate, 469 refused, 258 timeouts, 66 × 500. Leader commit p50/p99 4,226 / 9,754 ms (n 244). Term went from 3 to 28 during the run. Leader inbound `/read` 103,445, `/suspect` 128,734. Member peaks 265 / 97 / 81 MiB; `exceededMemory` a 218, b 812 | **FAIL** |
| B0-33, L0-12 A1 replay, B0-34 | — | — | **Not run.** `rows.sh` stopped at $13.80 (> `STOP_USD` 13.4) after B0-31 | **not run** |

**New observation: a group left with no leader (B0-35, 131215).**
- From about 17:08:30Z the group answered every edge call with 503. At 17:21Z, after the load had stopped, all
  three voters were still followers at term 4 with `leader: null`; two had voted for site 2.
- Over 17:09–17:17Z, f2 received 150,399 `/suspect` reports and 78,842 `/read`s, yet no election finished.
- One test-only `/campaign` to each voter at 17:22Z elected site 0 at term 5 within 5 s.
- So during this run the request-driven campaign path (lookup `suspect`, then `try_campaign`) didn't elect a leader
  under load, but a quiet `/campaign` did at once.
- Why isn't verified. It may be f2 sitting over its memory limit, so its own campaign or vote traffic failed, or a
  campaign left `Busy`. B0-31's `tests/fanout/raw/lead-r5.log` (`tests/fanout/leadwatch.sh`, 15 s samples) shows
  the opposite failure: elections that do finish, but churn from term 9 to 28 in about 10 minutes, with one voter at
  a time not answering `/status`.

**Reading the rows together.** All three rows that ran fail the way B0-29 did in section 3:
- one member climbs to 265–318 MiB and logs thousands of `exceededMemory`;
- then the `/suspect` storm (63–129 k on the leader), elections, and the gate's 503s.
- The limiter is still the unexplained member memory of section 4. Fixes 1 and 2 didn't remove it.

**Cost.** $13.91 by `m2bcost.py` at 17:42Z (containers $10.77), from the r2 site day. Under the $15 cap. This
section's rows added $2.66: B0-35 $0.91, B0-32 $0.52, B0-31 $1.12, plus the teardown.

**Teardown.** The final pass at 17:41Z (`tests/fanout/raw/teardown-r2-11.log`) tore down 52 running shards, all
200. That includes `git#0#pool:1,3-7`, which this section's k = 4 rows had started again. No pool shard is needed by
any row now.

**What LM-1 still lacks, besides B0-30** (updated; this replaces section 11):
1. Repository members that hold journey 9 in 128 MB (section 4). B0-29, B0-31, B0-32 and B0-35 fail on it, and
   B0-33, B0-34 and L0-12's A1 replay weren't run.
2. Leader election under that load. B0-35's group stayed leaderless for about 10 minutes until a manual
   `/campaign`. This is a new item, not diagnosed.
3. B0-34's `fanout` stepping k in band, shown hosted.
4. B0-35's instruments: test-merge vCPU against `TEST_MERGE_BUDGET`, list-field computes, and Actions dispatch,
   which needs an install with Actions.
5. LF-13 now belongs to `graft-m3c`. L0-12's Graftyard-run lease refusal and B0-37's session-ref listing belong to
   AM3-WP3.

**Commits added:**
- `4ccef7c`: `rows.sh` `STOP_USD`.
- `a0cf57a`: `a1.mjs` keeps its row when the leader's `/stats` has no body.
- This commit: section 14, `tests/fanout/lead.mjs` and `leadwatch.sh`, and the raw status files.
