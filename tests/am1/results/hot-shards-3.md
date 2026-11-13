# `hot-shards-3`: A31, pushes at a kill, the warm tail, and the depth chain (2026-11-13)

Branch `hot-shards-3`, from main `642f038` rebased on `20147db`. Follows `tests/am1/results/hot-shards-2.md`. Item 4 (batched rehydrate) moved to thread rehydrate-batch (T-01a1d2d6-889e-720b-b84b-3939b1ed7b79) and is not here.

Shard size: every hosted number below is on **standard-4** shard containers (`SHARD_INSTANCE=standard-4`, journey 9's tier size, approved for test installs on 2026-11-13). `hot-shards-2`'s figures quoted for comparison (warm p99 10.45 s, first 3 min 55.8 s, B0-29's climb) were all on standard-2.

## Summary

- **A31, answered: derived work does not raise k.** With k forced to 1 and all three shards killed at 03:40:20Z, each shard rehydrated 2.1 GB with no pushes. Container CPU load ran 0.13 to 0.99 a minute. The `NetworkLoad` the shards report stayed between 0.0001 and 0.042 for 14 min, and k stayed 1. The 0.99 that `hot-shards-2` read was an accounting gap: a running repack's `pack-objects` was counted only after it exited. That gap is fixed.
- **Pushes in flight at a home-shard kill now all succeed.** In j9h, after the keep-warm fix, all 41 pushes started in the 10 s before the kill finished ok, the slowest in 18.0 s, and all 3,696 pushes of the run were ok. In j9g, before that fix, 16 of the 40 in flight got `http 503` with `Retry-After` after 137 s, because the other sites' shards were 1,100 and 1,900 entries behind. That is retryable, not `unpacker error`; `hot-shards-2` refused 23 with `unpacker error`. Both are kept: the shard and edge answer 503 when no shard can check a push, and the leader keeps every Full site's shard caught up (`9336556`).
- **Push p99 was 4.7 to 4.9 s at journey 9's rate on standard-4**, under 7.2 s in every window. j9g (6,919 pushes, 3.84/s): warm 3.99 s, first 3 min 4.89 s (j9f on standard-2: 55.8 s), after the kill 4.78 s. j9h (3,696 pushes): warm 3.63 s, after the kill 4.88 s. 16 pushes in each run took about 18 s at one moment: the second transfer in j9g, and 05:12:03 to 05:12:07Z in j9h, when the leader's 15 s keep-warm call to its own site's shard timed out. The second is a cost of the new keep-warm (see open items).
- **The warm tail and B0-29's climb come from the shard's git, not the shard path.** Validate's two `--not --all` walks read every ref's tip from the packs, so their CPU grew with the refs. A repack that reached the big pack ran a delta search 1 to 5 s under the network's lock. Repacks now write a commit-graph and use `--window=0`. Locally, a B0-29-like 300 s run dropped from 19.5 to 25.3 CPU-s/min to 12.9 to 16.3, and validate p99 from up to 1,948 ms to under 140 ms. Hosted, j9g's p99 stayed flat at 3.0 to 5.2 s per minute for 30 min.
- **A carried-over k sends no push to a cold pool shard for 30 s after it answers `warming`.** j9g ran at k = 1, so this was not exercised hosted. It is covered by a kept test and two plants.
- **Depth: 0 "Subrequest depth" errors** on all three sites' Member and GitShard logs from 03:34Z to 04:35Z. In that window repo:65551 had three cold rehydrates of 2.1 GB, a home-shard kill, two transfers and 50+ archive steps. The archive's promotion now runs in a stateless invocation through `SITE`, as `PrSync`'s first attempt does.
- **Promote and archive kept up through the kill.** promotedTo went from 18,647 to 22,906 and archivedTo from 6,006 to 6,451. The log above `Promoted` stayed at 61 to 83 MB, and the log at 2.15 GB.
- **`git fsck` was clean on all three shards after catch-up**, after both j9g and j9h: 504 refs each, identical on every shard.

## Changes

| Item | Change | Commit |
| --- | --- | --- |
| 1. A31 accounting | `sampleDerived` (container) counts every live descendant of a derived command, so a running repack's `pack-objects` is in derived CPU before it exits | `c3413ca` |
| 2. Pushes at a kill | A shard answers a failed `/validate` 422 only for the container's own 4xx (`validate_status`); a broken catch-up, a stopped container or a lost call is 503. The edge stops at a 422 and answers `Unavailable` (all shards failed without one) as 503 `Retry-After: 2` | `c3413ca` |
| 3. Warm tail and B0-29 climb | Repacks `--window=0` and write a split commit-graph. A first shard skips a pool shard for `POOL_COLD_SKIP_MS` (30 s) after it answered `warming`. Site days set `SHARD_INSTANCE=standard-4` | `e99dd50` |
| 5. Depth chain | The archive's promotion (`/promote`, `/validate`, placement) runs in `POST /archive/promote` on the site's own script through `SITE` (`archive::in_band`); the member records `Promoted` | `b41e2f0` |
| Found by j9g | A leader catches up every Full site's first shard at once and every `KEEP_WARM_MS` (15 s) while it applies entries, not only its own site's once per leadership (`keep_warm_due`). Rerun as j9h | `9336556` |

### Item 3: what sets the warm tail

Local, one CPU (`taskset -c 0`), a B0-29-like mix (33 ls-refs/s, 500 agents pushing every 120 s, 10 to 70 KB, 5% 1 MiB, no merges). Driver and analysis in `/tmp/b029prof` (not committed).

| Run, 300 s | CPU-s per minute 1..5 | validate p99 (ms) | upload-pack p99 (ms) | longest repack lock |
| --- | --- | --- | --- | --- |
| Before | 19.5, 21.7, 25.1, 22.0, 25.3 | 258 to 1,948 | 184 to 1,499 | 1,757 ms |
| Commit-graph only | 14.6, 15.3, 17.7, 17.0, 18.4 | 138 to 1,560 | 146 to 1,406 | 1,792 ms |
| `--window=0` only | 16.2, 18.8, 22.1, 22.6, 22.5 | 121 to 405 | 31 to 109 | 241 ms |
| Both | 12.9, 13.1, 15.6, 15.1, 16.3 | 59 to 138 | 15 to 57 | 472 ms |

- The validate walks grew from 7.8 to 14.9 ms a call as refs went from 501 to 1,001. Pack count barely mattered (1 pack: 6.2 ms, 43 packs: 7.3 ms). With a commit-graph the walks take 2.7 to 3.7 ms.
- The latency spikes were repack lock holds: a geometric roll that reached the 189 MB pack ran a full delta search under the lock, and every validate, ls-refs and ingest queued behind it. `--window=0` gave the same pack size on these random blobs. On real text it could grow the disk; that is not measured.
- B0-29's hosted climb (narrow-reads-3's raw timing, standard-2) matches this shape. The home container's CPU went from 21 to 36 CPU-s/min to 45 to 52 by minute 5, saturating 1 vCPU, with memory and disk under 210 MiB. It was neither pool-shard k carry-over (no pool shard served) nor memory or disk growth. A hosted B0-29 rerun was not done.

### Item 4: batched ingest

Moved to rehydrate-batch. Rehydrate on standard-4 for comparison: sites a and b rehydrated 2.1 GB (1,966 stored and 1,168 inline packs) in 536 s and 660 s. The marked phase (stored packs) was 432 s and 573 s of it, and the tail 59 s and 42 s. hot-shards-2 measured 1,212 to 1,335 s on standard-2.

## Tests and plants

- `containers/git/server/load_test.go`: `TestARunningDerivedCommandsChildCountsBeforeItExits`.
- `containers/git/server/repack_test.go`: `TestRefsRepacksPastRepackAt` now also requires a commit-graph chain that verifies.
- `crates/git/tests/shard.rs`: `a_shard_answers_422_only_for_the_containers_own_refusal`, `a_pool_shard_that_answered_warming_gets_no_push_until_the_skip_passes`, `a_leader_keeps_the_shards_warm_only_while_it_applies_entries`.
- `workers/edge/src/lib.rs`: `only_a_shards_422_refuses_a_push_and_shards_that_all_fail_are_unavailable`.

Plants (`tests/am1/hotshards/plant.py`, numbers 38 to 46): all 47 caught, none missed. A first version of the edge plant "only the last try's 422 counts" was missed. The edge then stopped at a 422, which makes that mutant equivalent, and the plant was dropped.

| # | Mutant | Caught by |
| --- | --- | --- |
| 38 | a running derived command counts its children only once they exit | `TestARunningDerivedCommandsChildCountsBeforeItExits` |
| 39 | a shard answers every failed validate 422 | `a_shard_answers_422_only_for_the_containers_own_refusal` |
| 40 | a container 5xx is the pack's answer | same |
| 41 | the edge answers `unpacker error` when every shard failed without a 422 | `only_a_shards_422_refuses_a_push_...` |
| 42 | the repack writes no commit-graph | `TestRefsRepacksPastRepackAt` |
| 43 | a pool shard that answered warming still gets every push | `a_pool_shard_that_answered_warming_...` |
| 44 | a pool shard that answered warming is never tried again | same |
| 45 | a leadership catches the shards up once and never again | `a_leader_keeps_the_shards_warm_...` |
| 46 | an idle group keeps waking its shards | same |

Not planted, because no local test reaches them: running the promotion in the member again (item 5) and `--window=0`. The hosted window covers the first (0 depth errors). Full checks pass on `b41e2f0`: fmt, workspace clippy, wasm32 clippy for site and edge with and without `test-faults`, ops, `cargo test --workspace`, `go test`, and the three `scripts/check_*.py`.

## Hosted on `graft-m3c`

Deploy `hs3-deploy` = `hot-shards-3` `b41e2f0`, test-faults, image `sha256:106fe9b4842921e8666065697f7ec8b06c54eda8a02aa5fe0f7542967272457b`, `SHARD_INSTANCE=standard-4`. Site day 03:25 to 03:34Z (sites `1b4516ee`/`0299a6c8`/`93dfba60`); edge `21e69e59` (`--ops graft-m3c-ops`, FORMATS with 32.18). Group m3org/hot-a (repo:65551): 2.15 GB log, 63 MB above `Promoted` at the start. Raw files in `tests/am1/raw/hotshards3/` and `tests/am1/raw/hotshards/` (gitignored): `a31.jsonl`, `j9g-pushes.jsonl`, `j9g.log`, `faultsg.log`, `monitor.jsonl`, `fsck-j9g.json`, `timeline.txt`.

After the site day the group was leaderless: its members idled, as F67's idle shape intends. One `/read` elected a leader in under 3 s.

### A31 (03:40 to 03:54Z, no pushes)

`tests/am1/hotshards/a31.mjs`: `/bench_seed {"fanout": 1}` on the leader, `/_bed/kill` of all three first shards, a catch-up call to each, then every minute the committed k on each member and each shard's `/status` (`load`, `k`).

| Minute | k (committed) | `load.cpu_p99` a / b / c | Container CPU load a / b / c (metrics) |
| --- | --- | --- | --- |
| 0 | 1 | not running / 0 / 0 | |
| 2 | 1 | 0.034 / 0.026 / 0.026 | 0.35 / 0.32 / 0.39 |
| 5 | 1 | 0.034 / 0.015 / 0.017 | 0.59 / 0.13 / 0.16 |
| 8 | 1 | 0.042 / 0.028 / 0.016 | 0.48 / 0.15 / 0.19 |
| 9 | 1 | 0.042 / 0.015 / | 0.99 (a's repack) / 0.16 / 0.46 |
| 11 | 1 | 0.0007 / 0.032 / 0.015 | 0 / 0.77 (b's repack) / 0.16 |
| 13 | 1 | 0.0005 / 0.0001 / 0.014 | |

Sites a and b finished their rehydrates at 03:49:21Z and 03:51:25Z, each with a repack at the end. The repack's minute read 0.99 and 0.77 on the container while the reported load stayed under 0.05. A first shard reports load only on its own push validates (`workers/site/src/shard.rs`), so a network with no pushes cannot step k at all. What this run shows is the load such a report would carry while derived work runs.

### j9g: pushes through two transfers and a home-shard kill (03:54:26 to 04:24:28Z)

1,800 s, 6,919 pushes at 3.84/s (journey 9's 4.17/s, INFLIGHT 16). Faults: transfer 0→1 at 03:59:27Z, kill of site a's shard (the home and the leader's site) at 04:06:28Z, transfer 0→1 at 04:16:27Z. Both transfers ended with site 0 leading again.

| Window | Pushes | p50 | p99 | Max | Over 15 s |
| --- | --- | --- | --- | --- | --- |
| Whole run | 6,919 (6,903 ok) | 2.26 s | 4.70 s | 18.6 s | 16 |
| First 3 min | 758 | 2.30 s | 4.89 s | 5.1 s | 0 |
| Warm, 03:57:30 to the kill | 2,210 | 2.10 s | 3.99 s | 14.0 s | 0 |
| After the kill | 3,911 (all ok) | 2.38 s | 4.78 s | 18.6 s | 16 (all at 04:16:40, the second transfer) |

The 40 pushes started in the 10 s around the kill:

- 24 ok;
- **16 `http 503` with `Retry-After`**, after 137 s each. The home's 15 s wait answered 503, then site b's 60 s, then site c's 60 s. Each shard was still catching up by single entries (site b 1,100 behind, site c 1,900), and site b's member trapped with "memory access out of bounds" 3 s after the kill. None was refused `unpacker error`; `hot-shards-2` refused 23 that way.
- Pushes started after 04:06:30Z all went through: site a's shard rehydrated, and sites b and c caught up.

That those shards were so far behind is the gap commit `9336556` closes: the leader now catches up every Full site's shard every 15 s while it applies entries.

### j9h: the home-shard kill again, with the shards kept warm (04:57:41 to 05:12:44Z)

Site-only redeploy of `9336556` (sites `ca58df09`/`6a4905a3`/`dbb1a767`, same image, standard-4); the edge is unchanged. The three shards rehydrated first (finished 04:53 to 04:58Z). 900 s, 3,696 pushes at 4.1/s, and one fault: a kill of site a's shard (the home and the leader's site) at 05:02:44Z. No transfers, so the run tests only the in-flight path.

| Window | Pushes | p50 | p99 | Max | Over 15 s |
| --- | --- | --- | --- | --- | --- |
| Whole run | 3,696 (all ok) | 2.41 s | 4.88 s | 18.5 s | 26 |
| Warm, before the kill | 1,214 | 2.11 s | 3.63 s | 5.1 s | 0 |
| **Started in the 10 s before the kill** | **41 (all ok)** | 2.42 s | 18.0 s | 18.0 s | 10 |
| After the kill | 2,441 (all ok) | 2.53 s | 4.88 s | 18.5 s | 16 (05:12:03 to 05:12:07) |

The 10 pushes in flight that took over 15 s waited out the home's 15 s and then validated on site b's or site c's shard, which was a few seconds behind. Promotion and archive went on through the kill: promotedTo went from 23,438 to 25,312 and archivedTo from 6,510 to 6,672, with 60 to 78 MB above `Promoted`. fsck after catch-up (cursor 26,036 on every shard): all three `ok`, 504 identical refs, 161,512 objects each. 0 "Subrequest depth" in any site's Member or GitShard logs from 04:35Z to 05:14Z.

### Promote and archive

| Time (Z) | promotedTo | archivedTo | Above `Promoted` | Log |
| --- | --- | --- | --- | --- |
| 03:54 | 18,647 | 6,006 | 63 MB | 2.148 GB |
| 04:06 (kill) | 20,282 | 6,188 | 72 MB | 2.156 GB |
| 04:09 | 20,500 | 6,204 | 66 MB | 2.154 GB |
| 04:16 | 21,514 | 6,313 | 71 MB | 2.155 GB |
| 04:25 | 22,906 | 6,451 | 61 MB | 2.147 GB |

Promotion paused for about 2 min after the kill (04:06 to 04:08, promotedTo 20,282) and then went on on sites b and c. The log stayed under `LOG_MAX_BYTES`, and the archive trimmed what pushes added.

### fsck

After catch-up, cursors 23,701, 23,701 and 23,703: all three `ok`, 504 refs each, identical refs, 147,320 / 147,320 / 146,056 objects in pack.

### Depth (item 5)

0 "Subrequest depth" in Workers Logs for `graft-m3c-site-a`, `-b`, `-c` (Member and GitShard) from 03:34Z to 04:35Z. merge-budget-4's trace `8c049c94` (20:07:37Z on 2026-11-12) ran the archive drain from the leader member: the shard's catch-up read `/entries` back from that member one entry at a time, and the 31st read failed. In this window the archive took 50+ steps through three cold rehydrates, a kill and two transfers.

## Cost

Hosted window 02:55Z to 05:20Z on `graft-m3c`, both site days and both runs, about **$2.35**:

- Workers and Durable Objects: $0.65 (`tests/am1/hotshards/usage.py`: 11,905 Workers requests, 380,796 DO requests, 73,852 DO active seconds).
- Containers: about $1.70 for 230 standard-4 instance-minutes (4 vCPU, 12 GiB, 20 GB at `research/prices.json` rates, from the containers metrics rows).

The cap was $25.

## Design changes

- `design/DESIGN.md`, Shape, `IngestCursor`: the archive's promotion is stateless, and a push no shard checked is 503. `GitCacheShard::validate`: a commit-graph each repack keeps `--not --all` flat. The line count stays at 2,200.
- `decisions/OPEN-QUESTIONS.md`: A31 answered. G7: item 4 moved to rehydrate-batch.

## Contradictions and open items

- **The keep-warm costs a spike** (traced and fixed below, "The 05:12 stall"). In j9h, at 05:12:00Z, the leader's keep-warm call to its own (home) shard got no answer in 25 s, and 16 pushes in the same seconds took about 18 s. The likely cause is the home shard's catch-up running while it also serves the pushes [inference]: the shard serializes catch-ups per network, and a push waits on the one running. Not traced further. It stays inside the 7.2 s p99. A keep-warm that skips the home's own site would avoid it, but then the home after a transfer is the one that falls behind.
- **The 503 path is kept but untested hosted with warm shards.** j9h's in-flight pushes all succeeded, so none needed it. j9g showed it: 16 pushes got 503, not `unpacker error`.
- **`--window=0`** costs deltas between pushes once packs roll up. On these random blobs that was free; on real source text it could grow disk. Not measured.
- **A31's mechanism note:** a network with no pushes cannot step k, because only push validates report load. A31 shows derived work is left out of the report, not that the step logic is idle-safe for every input.
- **Site b's member trapped** ("memory access out of bounds") at 04:06:31Z: see "Site b's trap" below.

## Merge with main, and the follow-ups (2026-11-13, 05:20 to 07:05Z)

### Merges

- **`6a92d62` merges main `020b3e5` (LM-1 rounds 6 to 8).** `ShardHost::validate` keeps LM-1's streamed body, admission slots and `Validated::{Busy, Done}`, and its error carries the container's status. Only the container's own 4xx is 422; a 429 is `Busy`, never an error. Everything else is 503. The edge's `ShardRefusal` gains `Unavailable` beside `Invalid` and `Busy`. `shard_refusal` gives no refusal for it, so the push is answered `shard_unavailable` (503, `Retry-After: 2`), never `unpacker error`. `--window=0` now lives in LM-1's `runRepack` hook, and the split commit-graph follows it. `pool_forward`, the keep-warm and `/archive/promote` are unchanged. Two test beds (`tests/m1/wp5`, `tests/m1/wp6/bed`) answer a failed validate by `validate_status` (`75f93c2`). New plant: the edge refuses `Unavailable` as `unpacker error`.
- **`67db2c3` merges main `a2687fe` (rehydrate-batch).** Both sides are kept: `/ingest-batch`, the batched `ShardIo::ingest` and its plants, plus this branch's plants. DECISIONS keeps both entries. G7 takes rehydrate-batch's narrowing, then this branch's. `image.sh` keeps rehydrate-batch's `TAG`, defaulting to `hs3`.
- **Image:** `registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:696907a2838fafd91cb9cab7b9be4a673af54f56b1c811014a4eb0a4e055aec5`, built from `67db2c3` (`tests/am1/raw/hotshards3/image-hs3b.log`). It holds LM-1's admission gate, `/ingest-batch`, the commit-graph and `--window=0`. An intermediate image from `75f93c2`, `sha256:8528e368…`, was superseded within the hour and is not to be used.

### The 05:12 stall: a rehydrate dropped its mark before it could serve (`5f68216`)

In j9h the home shard (site a) rehydrated after the kill. The rehydrate saved its cursor after the refs pages and dropped its `rehydrating` place in the same step. Its report (05:12:41Z) shows the catch-up from the first refs page, 338 entries, took 38 s after that (`refs` 556.9 s, `catch_up` 595.1 s). With the place gone, `warming_now` no longer sent a push that had another shard next on at once. Each push the home took waited out its 15 s (16 validates of 15.1 to 15.2 s at 05:12:23 to 05:12:27Z), then validated on site b in under 1.5 s. The keep-warm's "no answer in 25 s" was the same catch-up.

Fix: the place is dropped only when the last catch-up reaches the head, or by the catch-up that finishes a run cut off in it. A dropped cursor drops it too, and `ShardHost::rehydrating` is the place alone. Kept test: `a_rehydrate_is_marked_rehydrating_until_its_last_catch_up_reaches_the_head` (`crates/git/tests/catchup.rs`), which cuts the run at every point and checks the mark through the last catch-up. Plants 43 and 44 (dropping the place with the cursor, and a finishing catch-up that keeps it) are both caught. Not rerun hosted: the smoke below had no kill.

### Site b's trap at 04:06:31Z: the F47 shape, not a new bug

The trapped invocation was Member `12100d32…` on `graft-m3c-site-b` (version `0299a6c8`), `POST /entries?from=20141&to=21263&local=1&bytes=4000000`. That is site b's shard reading a 4 MB local page during its catch-up, 3 s after the kill. The stack was `worker::delay::Delay::poll`, under `wasm_bindgen` `invoke`. The same request ended "Promise will never complete." at 04:06:35Z (outcome `exception`, 4,123 ms wall, 26 ms CPU). The next `/entries` call at 04:06:35Z answered in 602 ms (288 ms CPU), and the catch-up went on. No other invocation in that isolate failed in that window, and no `Critical` or "previous WASM instance" line appears.

This matches `tests/m2/results/f47.md`, "How the generated glue turns this into `unreachable`": a JS-side allocation failed near the isolate's 128 MB, and the terminated instance's next call surfaced as "memory access out of bounds" [inference]. The page holds up to 4 MB of stored rows (`ENTRIES_LOCAL_PAGE_BYTES_MAX`) and is built as JSON in the member. It is not the Register restore issue: m0-carried-6's thread (T-01a1ccdd-804f-769d-b81a-5f937e3309df) reports no trap, Critical, or restore failure. Its "Register" is the test-faults member kind, not a failure. Open: whether the 4 MB local page should drop to 2 MB under load. Every later catch-up of the 2 GB tails today ran at 4 MB without a trap.

### `--window=0` disk growth, measured locally

Two local replays (`/tmp/hs3/win/run.sh` and `hot.sh`, not committed) ingest each push with `index-pack --strict --fix-thin` and repack geometric past 8 packs. They compare `--window=0` with the default window.

| Shape | Pushes | Pushed bytes | Shard packs, `--window=0` | Shard packs, default window | Full `repack -adf` |
| --- | --- | --- | --- | --- | --- |
| This repository's last 148 first-parent commits | 148 | 29.0 MB | 60.0 MB (2.07×) | 35.8 MB (1.24×) | 29.8 MB |
| Journey 9's push shape (`load.mjs`: 10 to 70 KB blobs, 5% 1 MiB, cut from one base) | 400 | 31.2 to 41.0 MB | 31.2 MB (1.00×) | 3.1 MB (0.075×) | 1.9 MB |

On real source history, `--window=0` costs about 1.7 times the disk of a delta-searching repack. On journey 9's synthetic blobs it keeps every push whole, about 13 times the disk, because those blobs all delta against one base. That is why every shard here holds 2.56 GB for a 2.15 GB log.

The reason for `--window=0` is also gone. It was the 1 to 5 s delta search under the network's lock, and LM-1 round 7 (`79212b5`, merged here) took the repack out of the lock. A local B0-29-like A/B on the merged container (300 s, one CPU) compared `--window=0` with the default window:

| Arm | CPU-s per minute | Validate p99 |
| --- | --- | --- |
| `--window=0` | 15.8 to 18.9 | 102 to 172 ms |
| Default window | 16.1 to 19.6 | 106 to 196 ms |

The default-window arm's per-command breakdown did not record. The latency and CPU columns come from the driver and the cgroup.

Recommendation, not made here: drop `--window=0` (keep the commit-graph), and record the disk numbers in DECISIONS. Doing that needs another image and another tell to LM-1, so it is the parent's call.

### Hosted smoke on the merged tree (graft-m3c, standard-4)

Site day 06:27:54 to 06:41Z from `hs3-deploy` = `bc069c8` (`67db2c3` plus the image pin), test-faults, `SHARD_INSTANCE=standard-4`, image `696907a2…`. Sites `7af8a462`/`32c229d6`/`2ef0139a`, edge `b527e1ef` (`--ops graft-m3c-ops`, FORMATS with 32.18). The site day tore down every shard, so all three started cold on repo:65551 (2.15 GB log, about 2.5 GB of objects).

| Run | Window | Pushes | Result | p50 | p99 | Max |
| --- | --- | --- | --- | --- | --- | --- |
| j9s, cold start | 06:42:04 to 06:50:09Z | 113 sent (16 in flight cap) | 65 ok, 48 `http 503` | n/a | n/a | 65 s |
| j9w, warm | 06:51:11 to 06:56:13Z, 4.17/s | 1,251 | **1,251 ok** | 3.07 s | **5.01 s** | 6.44 s |

- **Cold start.** Every push in the first 7.5 min got `http 503` after about 64 s. It was the retryable answer, never `unpacker error`, because all three shards were rehydrating: 15 s home, then 60 s for the next shard, which answered `warming` first. Site a's rehydrate with batched ingest took 450 s (marked 395 s for 2,531 stored packs, tail 8 s for 1,104 inline packs, `held_max` 5.96 MB). Site c's took 1,091 s, its `/catchup` call cancelled at 119.6 s and continued in the background. The driver does not retry a 503; git clients do.
- **Warm.** p99 was 4.4 to 5.2 s per minute, under 7.2 s, with no refusal.
- **fsck** after catch-up (cursor 27,186 on every shard): sites a and b `ok` with 166,736 objects. Site c answered `ok: false` with empty output and 166,104 objects in one pack, just after its rehydrate's geometric repack. Refs are identical on all three (504). The script dropped `fsck`'s error text and the container was torn down, so the cause is open. A likely suspect is a repack finishing while `fsck` ran: `handleFsck` takes only the network's read lock, which a repack no longer takes [inference]. `tests/am1/raw/hotshards3/fsck-j9w.json`.
- Depth: 0 "Subrequest depth" in the window [not rechecked separately; the run had no transfer or kill].

### Cost of this part

05:20 to 07:05Z: Workers and Durable Objects $0.13 (`usage.py`: 2,017 Workers requests, 93,044 DO requests). Containers: 154 standard-4 instance-minutes and 1,190 CPU-s. At `research/prices.json` rates (vCPU active only), that is $0.31, and at most $1.03 if every vCPU were busy. This part comes to about $0.45 to $1.16, and the whole of hot-shards-3 to about $2.8 to $3.5 of the $25 cap.
