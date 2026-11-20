# LM-1 rows on graft-m3f: B0-31, B0-32, L0-12

Three Milestone 0 gate rows, split off from LM-1 ([thread](https://ampcode.com/threads/T-01a1c694-6112-72b4-83ca-dc9337a9646e), `tests/am1/results/lm1-memory.md`) to run hosted on a second install, `graft-m3f` (`tests/am1/results/install-m3f.md`). Each row is judged exactly as `build/BUILD-PLAN.md` writes it, on one run, with no substitution. LM-1 keeps B0-29, B0-33, B0-35 and B0-34 on `graft-m2b`. This thread: [LM-1 rows on graft-m3f](https://ampcode.com/threads/T-01a1d805-b0ec-7624-a63c-99341738c925). Branch `lm1-rows-m3f`.

> **Annotation, 2026-11-18 (cut-trace).** Queue figures from builds with lease-clock (`0b5769b7`, 2026-11-15) and
> without cut-trace mix two clocks and overstate the wait. Checked with `git merge-base --is-ancestor` on each run's
> site-day build (rewritten SHAs): m3fr6 to m3fr17 carry it and are annotated; m3fr1 to m3fr5c (`31636ec1`,
> `dc269249` and earlier) predate it, so their queue figures are on one clock. The m3fr11 analysis of B0-31's
> buckets-up tail below quotes m3fr11's figures and carries the same caveat.

## Build

`lm1-rows-m3f` is main `7c989fb` plus these changes:

- **LM-1's row drivers**, from `origin/lm1-memory` `ff27a11`: `rows.sh`, `smoke.sh`, `a1.mjs` with `clock.mjs`, `stall.mjs`, `transfer.mjs`, `learners.mjs`, `teardown.py --sweep`, `terms.mjs`, and `siteday.sh`. The branch's one product change, the queue split in `workers/site/src/host.rs`, is instrumentation and was not taken.
- **`ROWS_ENV`** names the install's environment for `rows.sh`, `smoke.sh` and `a1.sh` (default `tests/m3/rows/env.sh`, graft-m2b's). `tests/fanout/rows-m3f.sh` runs the rows with `tests/am1/lf13/env-m3f.sh`, which sets `INSTALL=graft-m3f`, so the teardowns and the sweep touch only graft-m3f's shards.
- **The admission trace.** `Member::admission` also reports the two moving averages that `Admission::capacity` is sized from, `round_ms` and `cmd_bytes`. The member's `/status` carries the leader's admission (capacity, roundMs, cmdBytes, queued, agentQueued, agentBytes, hot, pressure). `tests/fanout/admit.mjs` polls it every second beside a row (`TRACE=1`), next to `terms.mjs`'s leader trace.
- **L0-12's script gaps.** On m22, the replay's 540 tasks filled `TASKS_OPEN_PER_REQUESTER` (100) for every requester, m3-ci's own included. The stall's task creates then got 422 "The requester has too many open tasks". With no task id, every lease acquire read "An App token must name its session", and the refusal part could not create its holder task. `a1.mjs` now cancels the replay's tasks after the row line, which also releases their leases. `stall.mjs` now stops on a refused task create instead of reporting exit 0.

Shard image `sha256:de39c828…`, built from main's `containers/git` (`b4a7510`: the `/ui` routes, which `f6d2ef40` predates), on `standard-4`.

## Site day

**m3fr2's (second) site day:** 2026-11-14, 10:04Z to 10:18:30Z, from `lm1-rows-m3f` `d438f437`: main `1d331994` merged, with `repo-write-mark` (`RepoFold::write_mark`, `MERGE_WAIT`) and `admission-stored-bytes` (admission prices `stored_len`; an issue command's render is not priced), plus `retryAfterS` in `/status` admission and LM-1's `terms.mjs` (`bdc7f8b`, live voters). New shard image `sha256:4ab9de37…` from main's `containers/git` at `2fd459ce`, standard-4. Sites `cc66a386` / `1c62907b` / `54e6bc4d` (phase 1 `79d61116` / `9daba21f` / `ec875942`), edge `817c4adc`. Checked: `GET /repos/m3org/j4` 200, `repo:65536` leader on site 0 answering `admission.retryAfterS`.

**m3fr1's site day:**

2026-11-14, 05:02Z to 05:17Z, `tests/fanout/siteday-m3f.sh`, logged in `install-m3f.md` on main. Sites `4440f04d` / `664f2927` / `dfd53b45` (phase 1 `9f3d53c2` / `03930d80` / `fea147c4`), edge `a197b7fd`, built from `lm1-rows-m3f` `7e5b63f`. After the deploy, `GET /repos/m3org/j4` answered 200 and `repo:65536`'s leader answered the new `/status` admission fields. The install's own Apps (`tests/am1/rest/apps.mjs`, for L0-12's Graftyard-run and MCP kinds) were registered at 05:59Z: `graftyard-workspace` 196608, `graftyard-agent` 262144, `graftyard-mcp` 327680, installations 7190011 to 7190013 on `m3org`.

## Instruments for m3fr4 (after m3fr3; test-faults only)

**Counted-bytes high-water** (`Host::mark_mem`, `workers/site/src/host.rs`). The counted bytes are the call bodies in flight, the read answers streaming, the learner holds and the log: `memSplit`'s `calls`, `answers`, `held` and `log`. The host takes their sum each time one grows: a member call's body is counted, a read answer is counted, a message is held for a learner, or an input that wrote is stepped. It keeps the largest sum, with its parts, its time and the isolate's wasm memory then, as `Stats::mem_high`. `/stats?summary=1` answers it as `memHigh`. `&high=1` resets it after the read, so each of `vmem.mjs`'s 2 s samples shows the peak since the one before, including seconds in which the member answered no `/stats`. Past 16 MiB the host also logs `mem high {..}` each 8 MiB higher, so Workers Logs keep the peak of an isolate whose reset loses its `/stats`. **Stop rule:** `vmem.mjs` writes `tests/fanout/raw/.memstop-<TAG>` once a voter's `memHigh.total` passes 120 MiB (`MEM_STOP_MIB`); `rows.sh` then stops the row in progress and runs no later row.

**Driver clock offset** (`tests/am1/rest/clock.mjs`, `dateBounds` and `driverOffset`). Each HEAD to the edge, sent at orb time s and answered at e, carries Cloudflare's `Date`, the second D in which the edge answered. Cloudflare's clock minus the orb's then lies in [D − e, D + 1000 − s]. Forty probes at random gaps (37 to 137 ms), over many phases of the second, are intersected to bound the offset to about one round trip (40 probes at 15:13Z: [1,787, 1,829] ms, offset 1,808 ms). `a1.mjs` measures it before the window and after it. L0-12's `maxAbsOffsetMs` is now each member's offset less the mean of the two, that is each member against Cloudflare's clock. `maxAbsOffsetRawMs` keeps the old reading against the orb, and `driver` both bounds. `clockSpread` (the members against each other) is unchanged.

## Branch `stored-fetch`: stored `.idx`, promoted-range skip, parallel stored fetch (2026-11-14, no site day yet)

`stored-fetch` is main `e8d69e4c` (with LM-1's `shard-cancel` and `pool-warm`, and `memory-budgets`) plus four commits: promote-ahead counts pack bytes; the container's `/spool`, `/spool-idx`, `/ingest&spooled=1`, `/idx` and `/connected`; `a1.mjs` writes `testMerges` for every plan; and the Rust catch-up, rehydrate and promotion described here. The decisions and their flips are in `decisions/DECISIONS.md`, "`stored-fetch`"; G5 is answered and G9 opened in `decisions/OPEN-QUESTIONS.md`. Nothing here ran hosted yet.

### Why B0-34's cold rehydrate took half an hour

LM-1's m29 reports for repo:131296 (`tests/fanout/raw/rehydrate-131296-m29.json`, from `lm1-memory`) [measured]:

| Site | Up | Spool | `index-pack` | Rest | Total | Packs from |
|---|---|---|---|---|---|---|
| a | 32.3 s | 57.4 s | 1,872 s | 23 s | 1,994 s | site 0 256, site 1 1 |
| b | 32.1 s | 53.4 s | 2,496 s | 46 s | 2,638 s | site 0 27, site 1 230 |
| c | 34.2 s | 94.0 s | 1,694 s | 96 s | 1,932 s | site 0 41, site 2 216 |

All 257 stored packs (2,623,972,857 bytes, about 10.2 MB each) were pushes, none promoted, so every one ran `index-pack --fix-thin` at 1.0 to 1.55 MB/s. They were never promoted because `View::unpromoted` summed row bytes, and a stored pack's row is a short pointer, so promote-ahead never passed `LOG_MAX_BYTES / PROMOTE_AHEAD`. Sites a, b and c are site ids 0, 1 and 2, so each read its own bucket first (`own_first` is in 04f8b3a6). Site b's 27 and site c's 41 packs from site 0 came from there because their own buckets lacked them: a placement returns once two regions hold the pack, and the third-bucket filler copies the rest on a later sweep [code, `crates/git/src/store.rs` `place`, `crates/git/src/sweep.rs` `committed`]. A promoted pack goes through the same `place`, so it too reaches the third bucket only through the filler; this branch makes the filler copy the pack's `.idx` with it, and the rehydrate takes a missing `.idx` from another bucket.

### Promotion rate (local, 4 CPUs, git 2.56)

Scratch `/tmp/promo/measure.py` on git/git's first-parent history: 30 thin pushes of 100 commits each, ingested as a shard does, then promoted as `handlePromote` does [measured]:

| Step | Time | Rate |
|---|---|---|
| 30 thin pushes, `index-pack --fix-thin` | 17.52 s for 29.2 MB | 1.67 MB/s |
| Promotion `pack-objects --stdout` (72,577 objects) | 4.65 s, 57.9 MB out | 12.5 MB/s of output |
| `pack-objects` writing `.pack` and `.idx` | 4.80 s, idx 2.03 MB | pack differs from `--stdout`'s |
| Validate `index-pack`, default threads | 6.50 s | 8.9 MB/s of the promoted pack |
| Validate `index-pack --threads=1` / 2 / 4 | 11.97 / 7.96 / 6.81 s | 4.8 / 7.3 / 8.5 MB/s |

So a promotion costs 11.15 s for 29.2 MB of pushed bytes: **2.62 MB/s**, CPU-bound, under B0-31's agents' ~4 MB/s. The validate already runs `index-pack` threaded. Because `pack-objects` writing files produces different bytes from its `--stdout`, the stored `.idx` is the validate's (`GET /idx`), built from the bytes that are placed. A promoted pack is larger than the thin pushes it covers: 1.6 to 4.3 times, more for short ranges (git/git, `/tmp/promo/ranges.py`: 2.3x at 30 pushes per promotion, 2.7x at 5, 4.3x at 1; 1.6x for 1,000-commit pushes).

The push-rate sweep (`promotion_keeps_up_below_its_rate_and_stays_bounded_above_it`, 600 s, 1 MiB stored pushes, at 2.62 MB/s): 1 MB/s ends 3 MB above the last promotion, 2 MB/s 0 MB, 4 MB/s 754 MB, 6 MB/s 1,957 MB; the longest step is 19.2 s. At B0-31's rate the tail grows about 1.4 MB/s. That costs slower rehydrates, not correctness. It is G9; per the parent's call the branch does not widen into shard-streamed promotion, which lifts the memory bound, not this CPU bound.

### Stored-idx rehydrate, projected

`a_cold_rehydrate_of_5_gb_projects_under_120_s_and_the_serial_fetch_does_not` (virtual clock, site c's measured rates: up 34.2 s, one stream 27.9 MB/s, 375 ms a pack outside spool and index, `.idx` 3.5% of its pack) [inference from measured rates]:

| Network | Fetch ahead (4 at once) | One at a time | `index-pack` as on m29 |
|---|---|---|---|
| repo:131296 as 257 promoted packs | 83 s | 227 s | ~1,730 s |
| 5 GB as 100 promotions of 48 MiB | 90 s | 258 s | — |

The unmeasured variable is R2's throughput with four streams into one container; the projection assumes each keeps the one-stream rate. B0-34's cold warm on this branch measures it (`StoredMs.spool_ms`, `bytes`, `idx_used`, `lock_ms` in the `shard rehydrate` log line).

### B0-32 m3fr4: where the 503s came from (Workers Logs, read-only)

- The pool shards (pool:4, :5, :6) were warm at 16:25:57, 40 s before the burst. Their second wave of `warming` (16:27:06 to 16:28:13) was catch-up behind the hot log, not a cold start: each pool shard ingested all 186 burst packs (pool:4's catch-ups grew to 244 entries, 186 packs, 47 MB, 30.1 s).
- Each 503 push waited out up to four shards: home `wait=15` plus the pool forward's own 15 s, the `via=0` resend's 15 s, site b's 60 s, site c's 60 s, p50 163 s (114 pushes). The edge's comment that three Full sites fit in 180 s counts 135 s; the fan-out adds two 15 s waits it does not count. Trimming them makes the 503 come sooner, not succeed.
- Site a's validates ran p50 4.5 to 9.8 s in the burst, mostly waiting on the network's catch-up (one at a time, every shard ingests every pack).
- **Site c's network shard never started** (G8, routed to LM-1's pool-warm): all 361 validates and 405 upload-packs answered 503; its `/catchup?warm=1` calls answered in 2 to 4 ms through 16:27:22 because a warm call does not start a stopped container, and its catch-ups from 16:27:37 ended only at the teardown with "The container has not been started".
- LM-1's stuck upload-pack lock (round 18c) was absent: no cancelled clone or fetch preceded the first stuck validate on either row's shards, and B0-31's apac shard ran no upload-pack or validate at all.

This branch's catch-up change helps B0-32 and B0-31 the same way: stored packs fetched four at a time ahead of the in-order ingest, and a promoted range's pack not fetched again by a catch-up that already applied the range.

## Re-judged at 90% (2026-11-16, no new spend)

The operator's rule (`FRAME.md`, "Operator decision, 2026-11-16, 90% gates and a daily spend limit"): a count or rate is met at 90% of its goal, a latency at most 10% over its bound, and an invariant never at 90%. Each row is judged on one run, every line of that run, from the results below.

| Row | Line | Best run on it | 90% reading | Verdict |
|---|---|---|---|---|
| B0-31 | Commit p99 at most 1 s with buckets up | m3fr2 1,118 ms (split by the 300 s reset); m3fr4 1,333; m3fr8 2,138; m3fr9 9,600 | Bound 1.1 s. m3fr2 is 18 ms (1.6%) past it | **Short by 18 ms** on its best run |
| B0-31 | Placement failing: Human and MergeQueue p99 at most 3 s, 0 refused | m3fr8 and m3fr9: leader p99 1,588 and 1,638 ms, 0 Human and 0 MergeQueue refused | Met outright | Pass (m3fr2 failed it: 29 of 29 merges `warming`) |
| B0-31 | Agent pushes over INLINE_MAX_HOT refused at once in `ng` | m3fr9 1,258 of 1,258; m3fr8 1,245 of 1,262 (98.7%) | Met | Pass |
| B0-31 | Row | No one run meets all three: m3fr2 misses the merge line and is 1.6% over on commit; m3fr8 and m3fr9 are 2,138 and 9,600 ms on commit | | **Fail** |
| B0-32 | Every push acknowledged within 5 s or `ng … admission full` | 0 of 500 within 5 s on every run; best acknowledged p50 13.5 s (m3fr8), p99 22.2 s | 450 needed | **Short by 450 of 500** |
| B0-32 | Queued agent bytes under AGENT_QUEUE_MAX | max 532 KB (m3fr9) | Met | Pass |
| B0-32 | Merge in one of the next 2 entries | Not judged from entries on any run; m3fr4's slowest merge 2.4 s end to end against the 2 s fail line | | Not shown |
| B0-32 | Zero leader changes, zero `suspect` | m3fr8: 0 changes, 47 `suspect`; m3fr4: 0 and 0; m3fr9: 17 elections, 8,564 `suspect` | A zero count has no 90% reading | Pass only on m3fr4, whose burst line is 0 of 500 |
| B0-32 | Row | | | **Fail** |
| L0-12 | Lease overlap at most DRIFT (500 ms) | m3fr6 and m3fr9: 0 over 0 ms | Invariant | Pass |
| L0-12 | Member clock offsets under 500 ms | m3fr9: members 102 to 339 ms apart, 297 ms against Cloudflare's clock; m3fr6: 972 ms against Cloudflare (members 56 to 512 ms apart) | Bound 550 ms; m3fr6 is 422 ms over it. The overlap line above is the lease invariant; this line measures DRIFT's assumption (F3) | m3fr9 pass, m3fr6 **fail** |
| L0-12 | Every lease live after the stall, `expires` moved | m3fr6: 20 of 20 | | Pass (m3fr6); not run on m3fr9 |
| L0-12 | Late Exclusive acquire succeeds | m3fr6 and m3fr7: 201 | | Pass |
| L0-12 | Feed p99 at most 3 s outside kills | m3fr6 2,954 ms; m3fr9 25,210 ms | Bound 3.3 s | Pass on m3fr6, **short by 21.9 s** on m3fr9 |
| L0-12 | Refusals: 100 of 100 for each Agent kind, 0 for the other three | m3fr6 exact for all five kinds | Exact | Pass (m3fr6); not run on m3fr9 |
| L0-12 | Row | m3fr6 passes every line but the clock (972 ms); m3fr9 passes the clock but not the feed, and ran no stall or refusals | | **Fail** |

**What each row needs.**
- **B0-31:** one run with buckets-up commit p99 at most 1.1 s, and the merge and refusal lines as m3fr8 and m3fr9 had them. m3fr9's 9.6 s tail is on the leader and is not split (`a1.mjs` now keeps `splitByHalf.up`).
- **B0-32:** 450 of 500 burst pushes within 5 s. Every run's commit wait is 5 to 46 s; held for B0-02's duplicate-append fix.
- **L0-12:** one whole run with feed p99 at most 3.3 s and the member clock under 500 ms. The feed fix (`learner-lag`, `feed-gone-route`) and the writes' retry (`agents-write`) are built and not yet run hosted.

## Site day m3fg12: G11's rerun on `g11-home` (2026-11-19, graft-m3f, main `f466a39e`)

**Slot.** Granted by the orchestrator after `g11-home` merged: one site day from main `f466a39e` (`g11-home`, with LM-1's edge-only tier-read-batch reverted), then one seeded repository nearer m3fr7's size and one planned rehome, under the harness-outage rule, with a $3 stop on m3f counted from the site day's start. Raw files: `tests/fanout/raw/g11-m3fg12.{status,jsonl}`, `terms-m3fg12-g11.jsonl`, `g11-seed-m3fg12.log`, `arrivals-m3fg12.jsonl`, and the plan `g11plan-m3fg12.txt`. Driver: `tests/fanout/g11.sh` (`PRS=40`), run through the egress proxy. **Verdict: G11 passes** (orchestrator, 2026-11-19).

- **Site day:** 10:35:02Z to 10:48:27Z, `site exit 0`, `edge exit 0`. Image `sha256:15bc0e2d…`, built from `f466a39e` because `containers/git` changed since `1d815c92` (b034-lockwait).
- **Setup:** repo 65594 (`fm3fg12-g11`), seeded from a bare git/git clone in 250 pushes spaced evenly along the first-parent history, 0.25 GB in 1,128 s, largest push 5.7 MB. 40 PRs opened. Site 0 led at term 2, and site 0's shard was warm. Site c's shard was torn down at 11:11:52.8Z.
- **The move:**
  - The first `/move` asked at 11:11:53Z. 17 asks from site 0 answered `Home { home: SiteId(2) }` with `{"warming": true, "site": 2}` in about 0.3 s each, over 68 s. The last held ask read `waitedMs` 59,208. Home and leader stayed on site 0 at term 2 throughout.
  - Site c's rehydrate ended at 11:12:59.6Z: `catch_up` 66,172 ms, 183 entries, 25 stored packs (131 MB) and 8 packs, index 14.1 s, `tail` 65.0 s [measured, Workers Logs, graft-m3f-site-c]. m3fr7's was 69.0 s for 268 MB in 257 packs, so this is like-for-like.
  - The next ask, at 11:13:02.4Z, found the shard warm and committed `Home` (configIndex 406), with no `cold` label.
  - The trace shows site 2 leading at term 3 at 11:13:02.98Z, under 1 s after the commit, with no `Transfer` asked. The ask at 11:13:05.9Z answered `Done`.
  - The move took 73.2 s from the first ask to `Done` (m3fg11's cold move: 4.2 s).
- **Merges:** 33 of 33 merged.
  - During the 68 s hold, the old leader served 12 of 12, at p50 1.08 s and max 1.28 s.
  - In the minute after the hand-off, **0 of 11 answered `warming`**, all 200, at p50 1.76 s and max 2.45 s (m3fr7: 5 of 5 `warming`; m3fg11: 1 of 9). The new home's shard answered with `waited;dur=0`, and with `catchup` 240 to 380 ms on every other merge.
- **R29:** after 30 s with no write, all three voters were at commit 423 and applied 423, with site 2 leading at term 3. 0 leader changes besides the planned one.
- **Colos:** client LAX at the start and the end.
- **Harness outages:** `{"outages": "n/a (low rate)", "edgePerS": 0.58, "valid": true}`, by rule 5 of `tests/fanout/README.md`, "Harness outages on gate rows". Every merge and `/move` got an answer.
- **The margin:** the shard went warm at 59.2 s waited, 0.8 s inside WARM_BEFORE_TRANSFER, on a network of this size. That is the first condition of `g11-warm`'s flip, seen once. The orchestrator kept the bound at 60 s until more rehomes judge it.
- **Cost:** graft-m3f $0.16 by `PREFIX` from 10:35:02Z to 11:26Z (stop $3), and $0.00 from 11:16Z on. The account was $22.92 for the UTC day at 11:26Z.
- **Close-out:**
  - `retirenets.mjs 65594`: 4 names, all not joined.
  - `purge.py`: 297 objects (a 108, b 84, c 105). The second pass deleted 1 more on site a, which a sweep run had written, and listed 0.
  - `joined.mjs 65594`: none joined.
  - Two end sweeps at 11:26Z: no shard container running.

## Site day m3fg11: G11's hosted rehome (2026-11-19, graft-m3f, main `1d815c92`)

**Slot.** Granted by the orchestrator after `g11-warm` merged: one site day from main `1d815c92` (`g11-warm`, R29, why-logs, a1-wall), then one seeded repository and one planned rehome, under the harness-outage rule, with a $3 stop on m3f counted from the site day's start. Raw files: `tests/fanout/raw/g11-m3fg11.{status,jsonl}`, `terms-m3fg11-g11.jsonl`, `g11-seed-m3fg11.log`, `arrivals-m3fg11.jsonl`. Driver: `tests/fanout/g11.sh` and `g11.mjs`, run through the egress proxy.

- **Site day:** 08:49:33Z to 09:03:22Z, `site exit 0`, `edge exit 0`. Image `sha256:171857b3…`, built from `1d815c92` because `containers/git` changed since `7b576074`.
- **Setup:** repo 65593 (`fm3fg11-g11`), seeded from a bare git/git clone in 12 pushes, 0.25 GB in 167 s (`b034plan.py` at 30 MB steps; the plan is `tests/fanout/raw/g11plan-m3fg11.txt`). 24 PRs opened, each at the next of the plan's first-parent commits. Site 0 led at term 2, and site 0's shard was warm (`/catchup` 200).
- **The move:** site c's shard was torn down at 09:09:57.4Z, so the target was cold. Merges ran one every 6 s from 30 s before the move.
  - The first `/move` (on site 0, 09:09:58.3Z, 588 ms) committed `Home`, home 2.
  - The trace shows site 2 leading at term 3 at 09:10:00.3Z, about 1.5 s later, with no `Transfer` asked. The raft core's `maybe_hand_off` moved leadership to the config's new home by itself.
  - The second `/move` (on site 2, 09:10:01.6Z) answered `Done`. The move took 4.2 s from the first ask to `Done`.
- **G11's gate did not run:** 0 held asks, no `waitedMs`, no `cold` label. `move_step`'s `Transfer` arm was never reached. The move was cold in fact, without the label. m3fr16's rehome went the same way: `Home` then `Done` on site 2, warmed after by `shardwarm.mjs`.
- **MergeQueue `warming` in the minute after the hand-off: 1 of 9** (m3fr7: 5 of 5).
  - PR 7 (09:10:02.6Z) answered 503 `the git cache: 503 {"warming":true}` after 15.6 s, then merged on its retry in 11.2 s, shard `waited;dur=8630`.
  - The other 8 merged in 1.1 to 2.4 s.
  - Site c's cold rehydrate logged `catch_up` 20,065 ms for 87 entries, 0.46 MB inline and no packs (m3fr7: 69.0 s for 268 MB in 257 packs). This repository's packs are far fewer, so the two are not like-for-like on the window's length [measured, Workers Logs, graft-m3f-site-c, 09:10:28Z].
- **R29:** after 30 s with no write, all three voters were at commit 116 and applied 116, with site 2 leading at term 3. 0 leader changes besides the planned one (`terms-m3fg11-g11.jsonl`).
- **Colos:** client LAX at the start and the end.
- **Harness outages:** `arrivals.py` marked 54% of the seconds from 09:07:29 to 09:12:05Z as edge 0 with sites serving (`"valid": false`). The edge averaged 0.53 requests a second over that span and 0.18 in the merge window. That is the row's own one-request-every-6-s gap, not a dropped host: every merge and `/move` got an answer, with no `fetch failed`. The orchestrator's rule for such rows is now rule 5 of `tests/fanout/README.md`, "Harness outages on gate rows". No line reads differently with or without the windows.
- **Cost:** graft-m3f $0.05 by `PREFIX` from 08:49:33Z (stop $3). The account was $18.92 for the UTC day at 08:48Z.
- **Close-out:**
  - `retirenets.mjs 65593`: 3 names, all not joined.
  - `purge.py`: 115 objects (a 39, b 36, c 40), second pass 0.
  - `joined.mjs 65593`: none joined.
  - Two sweeps at 09:30Z: no shard container running.
  - Idle reading from 09:31Z: $0.00 at 09:47Z.

**Verdict and the fix.** The wait has to come before the config that triggers the hand-off, not before `Transfer`. Branch `g11-home` moves it there (`decisions/DECISIONS.md`, "`g11-warm`").

**The fix, `g11-home` (code-only, approved by the orchestrator).**
- **The gate moved.** `move_step` waits before any planned step that makes another site the next leader (`group_move::warm_site`): a `Home` step's new home, a `Swap` that drops the home (`swap_home`), and a `Transfer`'s target. A held step changes nothing and answers `{"warming": true, "waitedMs", "site"}`. A step past the bound carries `"cold": true`.
- **Other ways the core hands off, checked in the code.** `maybe_hand_off` hands off to `home()`, read from the latest config in the log (`log_config`), and otherwise only on a drain or `no_lead`, which are not planned moves. The only product paths that write a config are `/move` (`Home`, `Swap`, `Leave`), the re-placement and `/learners`. The re-placement and `/learners` keep `p.home()`. `Leave` keeps the `Joint`'s new home, which the `Swap` before it already waited for. `/reconfigure` is a test route with no product caller.
- **The `Transfer` gate stays.** It covers a move where the core did not hand off, for example a home whose calls failed in the window, or a `Joint`'s leader on the dropped site. An expired wait is kept, so a `Transfer` after a cold `Home` goes cold at once instead of waiting a second 60 s.
- **The driver** now reads `move_step`'s `{step, ok}` shape, which m3fg11's 200-character cut lost, and times `heldS` and the hand-off.
- **Tests:** four new on real members (`crates/identity/tests/group_move.rs`), one changed unit test, and `plant49.py` 8 of 8.

## B0-32 status after m3fst1 (2026-11-20, analysis only, no hosted spend)

Where B0-32 stands on main `b8e555df`, line by line, and which open question owns each loss that is left. The figures come from m3fst1's committed raw files. Two new files are committed with this section: `lm1-rows-m3f-data/m3fst1-raw/merge-pushes-b0-32-mwp3ub25.jsonl` (the row's 30 merge pushes from the client's push log) and `tests/fanout/raw/shardev-m3fst1-b0-32.jsonl.xz` (graft-m3f's `GitShard` events for repo 65602, 15:03:28Z to 15:04:15Z, from Workers Logs).

### Fixed: these lines pass on m3fst1 and stayed passed

| Line | Since | What fixed it |
|---|---|---|
| Zero `suspect` | m3fst1 (m3fac1 had 69) | `burst-receipts` (ask before reporting), `b032-shed`, `agent-cmds-entry`; A34's flush waits ruled out |
| Zero leader changes | m3fe2 | `step-agree` and the receipt rules |
| No other refusal shape; queued agent bytes under `AGENT_QUEUE_MAX`; no object reset from memory | m3fr13 to m3fe2 | the read cap, `b032-shed`, `AGENT_PUSH_BOUND` |

### Left: within 5 s (332 of 500) and merges (5 of 30 over 2 s)

**The 168 `ok` pushes over 5 s lost it in the commit stage** [measured, `server-timing`]:
- Their commit stage took p50 3,383 ms, against 2,410 ms for the 153 `ok` pushes within 5 s. Their other stages are ordinary: front 692, gate 291, validate 597.
- For each of the 168, the commit stage's time past 1 s alone exceeds its overage, which is p50 1,483 ms and at most 2,409 ms. With every commit stage capped at 1,500 ms, 130 of the 168 come in under 5 s.
- 118 of them committed twice (the leader's `Stage` answer, then the staged command). The `hot` stage-first rule causes none of these (Row m3fac1), and `agent-stage-first`, which removed them, cost more than it saved.
- **Owner:** F106 for the slow confirms, which the platform owns, and M4-WP4's `soft-grow` (branch `soft-grow`) for the queue behind them. **Across rows** [measured, each row's `tsave`, leader confirms of 500 ms or more in the 15 s from the burst]: m3fe2 0, m3fwi2 0, m3fwi3 0, m3fb32 1, m3fr17 2, m3fgt1 2, m3fac1 2, m3fst1 4. The three rows with none reached 87 to 99% within 5 s; the rows with them reached 27 to 66%. Other changes landed between those rows, so this is a correlation, not a controlled result.

**The 179 `full` pushes lost their budget before the send** [measured]:
- 155 of 179 spent 1.5 s or more on their core-rows read (in `gate` if the isolate's gate cache missed, else in `context`). Their read p50 was 1,858 ms, against 607 ms for `ok` pushes. That is 112 in `gate` and most of the other 67 in `context` (p50 2,002 ms).
- These are the late read-index grants behind the same slow confirms (A35, settled: no gate change).
- **Owner:** F106 and A35. Nothing is left to build at the gate.

**The merge pushes: two causes, and one of them is not the leader** [measured, the client's push log and the shard events]:

| Merge push (s from the burst) | Total | Largest stage | Cause |
|---|---|---|---|
| −71.5 (warm-up) | 2,438 ms | front 1,611 | A cold edge isolate on the row's first merge, before the burst |
| −2.0 | 2,460 ms | commit 1,046 | The leader's queue in the burst's first second |
| 4.1 | 3,976 ms | commit 2,844 | The leader's queue behind the 1,000 ms and 1,449 ms confirms (F106) |
| 6.1 | 2,703 ms | commit 2,260 | Same |
| 11.3 | **15,215 ms** | validate 14,116 | **Site a's git shard catch-up** |

- **The 15.2 s push is its own cause.** Site a's network shard and pool shards each caught up the burst's staged packs in one long run: one 11-entry catch-up ingested 294 packs (74.5 MB, index 4.2 s, spool 3.8 s) in 15.8 s and ended at 25.4 s. The others ran 8.2 to 11.7 s and also ended at 25.3 to 25.5 s.
- Validates and merges that arrived during these runs waited for them: 53 first-shard validates of 5 s or more, started 9.6 to 21.4 s and ended 24.7 to 27.6 s. They were 65 `full` agent pushes, 1 human and this merge push. Their `full` comes from `AGENT_PUSH_BOUND`, not the leader.
- The same run held `PUT …/merge`: one answered 503 `warming` after 15,008 ms and one 200 after 14,582 ms with `waited` 14,421.
- Three catch-up attempts on site a failed first, at 9.1, 9.3 and 13.5 s, each with `get: We encountered an internal error. Please try again. (10001)` from a bucket read. Whether those failures lengthened the run is an inference: the 294-pack run's first spool began at 9.75 s.
- None of this touches the leader: its storage confirms from 7 to 40 s were all under 300 ms, and it had no ticker stall and no input gap of 400 ms or more.
- **It recurs.** The merge's shard `waited`: m3fwi3 11,247 ms, m3fwi2 about the same (12,279 ms total), m3fac1 2,381, m3fgt1 1,093 (plus `merge_plan` 6,257), m3fst1 14,421; m3fe2 passed. Merge pushes with validate over 5 s: m3fwi3 2, m3fgt1 2, m3fst1 1.
- **Owner:** new `decisions/OPEN-QUESTIONS.md` G34. Neither F106 nor `soft-grow` touches it. Branch `catchup-pieces` builds the fix: catch-ups in pieces of at most 32 packs or 8 MiB, with held validates and `PUT …/merge` trying again between pieces (`decisions/DECISIONS.md`, "`catchup-pieces`"); no hosted row has run it yet.
- **Other `merge:only` answers:** 3,064 ms with commit 2,810 is the leader's queue (F106). 5,890 ms on the client was 632 ms on the edge, so the time was client-side.

**Summary of owners:**
- F106 (platform storage confirms) plus `soft-grow` own the commit-stage loss, the `full` budget loss and 3 of the 5 merge pushes.
- A35 records that the gate has nothing left to change.
- G34 owns the shard catch-up stall: 1 merge push, 2 `merge:only` answers and 65 `full` pushes.
- The cold-isolate merge push in warm-up has no owner; it fell before the burst.

## Row m3fpc1: B0-32 on main `a3430143` (`catchup-pieces`, `soft-grow`, `chain-hold`, `command-rows`) (2026-11-20, graft-m3f, `ROWS="b0-32"`)

**Slot.** One site day and edge day from main `a3430143` with `--ssh`, then one B0-32 row, stop $8 on graft-m3f from the site day's start (19:09:03Z). It is the first B0-32 row on `soft-grow`, `chain-hold`, `command-rows` and `catchup-pieces`.

**Committed raw files:** `tests/fanout/raw/rows-m3fpc1.status`, `{admit,terms,vmem}-m3fpc1-b0-32.jsonl`, `tsave-m3fpc1-b0-32.jsonl.xz`, `arrivals-m3fpc1.jsonl`, `shardgate-m3fpc1-b0-32-*.jsonl`, `shardev-m3fpc1-b0-32.jsonl.xz` (`GitShard` events for repo 65603, 19:30:00Z to 19:31:40Z), and `lm1-rows-m3f-data/m3fpc1-raw/` (the client's summary and its 32 merge pushes). `tests/fanout/pieces.py` reads the shard events into runs and pieces.

- **Site day and edge day** m3fpc, 19:09:03Z to 19:23:23Z, both exit 0, one run (one-shot launcher). Images are the m3fst ones, since no container code changed.
- **Row:** repo 65603 (run `mwpde1fv`), 8 learners, k 4, leader site 0 term 2. The burst reached the leader at about 19:30:56Z on the server clock; done 19:33:34Z. Harness outage 4 s (2.4%), valid.
- **Spend:** $4.92 from 19:09:03Z to 19:35:28Z (graft-m3f prefix $0.70). Group torn down 19:33:39Z; SWEEP2 OK. `ssh.graftyard.dev` gave slot A's three fingerprints at 19:52Z.

| Line | Measured | Verdict |
|---|---|---|
| Every burst push acknowledged within 5 s or `ng … admission full` | **251 of 500** (92 `ok` within 5 s, 159 `full`); m3fst1 332 | Fail (450 needed) |
| No other refusal shape | **3** refused `unpacker error: validate: bad update line` with garbage bytes, container 422 at 2.8 to 3.2 s; seen once before, on m3fr13 | Fail |
| Queued agent bytes under `AGENT_QUEUE_MAX` | peak 128,488 B | Pass |
| Every merge in one of the next 2 entries, none over 2 s | **4 of 32** merge pushes over 2 s (3,160, 2,402, 3,525, 4,972 ms); max down from 15,215 ms. `merge:only` 26 × 200, 3 × 409, **no 503**; p99 10,492 ms | Fail |
| Zero leader changes | term 2, leader site 0 throughout | Pass |
| Zero `suspect` | **0**, no misses | Pass |

**Late `ok` and `full`, against m3fst1** [measured, `server-timing`]:
- 246 of 338 `ok` pushes took over 5 s. In 243 of them, the commit stage alone exceeds the overage (all sends: p50 3,132 ms, against 3,383 on m3fst1's late ones). None had validate over 2 s.
- Committed twice: 204 of 338 (60%), against 43%. Stage answer p50 637 ms, second commit p50 2,512 ms. Of the 204, 30 came in within 5 s; of the 134 committed once, 62 did.
- `full`: 116 after a commit and 43 at `bound` (m3fst1: 177 and 2). 90 of 159 spent 1.5 s or more reading core rows (gate p50 2,411 ms), the late read-index grants A35 and F106 own.
- Leader commit p50/p99 438/1,781 ms (483/2,448); round p50 158 ms (185). Leader confirms of 500 ms or more in the burst's first 15 s: **2** (1,033 ms on 33 statements and 5.6 KB; 794 ms on 1 statement and 148 KB), both labelled `storage`, against 4 on m3fst1. No ticker stall of 200 ms or more.

**`catchup-pieces`** [measured, `shardev-m3fpc1-b0-32.jsonl.xz`, `tests/fanout/pieces.py`]:
- Each burst entry carried 49 to 61 staged packs (12.6 to 17.4 MB), over the 32-pack cap, so one piece was one entry. On site a a piece took about 1.8 to 2.3 s.
- Site a's four objects caught the burst up in two runs each, of 2 to 3 and 5 to 6 pieces: 340 to 346 packs (90 to 92 MB) in **14.9 s end to end**, against 15.8 s for m3fst1's 294 packs. Site b took 368 packs in 20.7 s (56 ms a pack, against 45 ms on m3fst1's b, +24%). Site c took 31.6 s (86 ms a pack), with no comparable run on m3fst1.
- First-shard validates of 5 s or more: **25, against 53**, max 9.8 s. Of the 46 site-a validates of 2 s or more that answered 200, 30 ended between pieces: after the piece that applied entry 409 (18.1 to 18.6 s from the burst) and before their run's last piece (19.0 to 21.9 s). One ended earlier, 15 later.
- The two slow `PUT …/merge` took 8,637 and 9,584 ms (`waited` 472 and 212, `catchup` 7,930 and 8,947). They ended at 18.2 and 19.4 s, also right after that piece.
- So the turns between pieces reached the waiters. What they waited for was in the burst's last large entry, and entries 404 to 409 (330 packs) applied 8 to 18 s after the burst. The remaining wait is the catch-up's speed, not the turns.
- 429 `Busy`: 0 on the edge's calls. The held re-asks are shard-to-container calls, which Workers Logs does not show.

**`soft-grow`: not exercised.** No cut grew: the largest took 45,411 soft bytes, 879 of 881 cuts ended `Empty` and 2 `AgentCmds` (64 agent commands each). The burst is Agent class, and the queue's depth counts only Human and MergeQueue bytes. So its flip cannot be judged on this row.

## Row m3fst1: B0-32 on main `f9b9f50e` (`sync-trace`) (2026-11-20, graft-m3f, `ROWS="b0-32"`)

**Slot.** The orchestrator granted one site day and edge day from main `f9b9f50e` with `--ssh`, then one B0-32 row, stop $8 on graft-m3f from the site day's start (14:42:42Z). The measures: whether a leader confirm over 1 s recurs and what `Trace::Sync` labels it (`decisions/OPEN-QUESTIONS.md`, F106), and the cap's cut ends again (`agent-cmds-entry`).

**Committed raw files:** `tests/fanout/raw/rows-m3fst1.status`, `{admit,terms,vmem}-m3fst1-b0-32.jsonl`, `tsave-m3fst1-b0-32.jsonl.xz`, `arrivals-m3fst1.jsonl`, `shardgate-m3fst1-b0-32-*.jsonl`, and `lm1-rows-m3f-data/m3fst1-raw/a1-b0-32-mwp3ub25.jsonl`.

- **Site day and edge day** m3fst, 14:42:42Z to 14:56:51Z, both exit 0, one run (one-shot launcher). Sites a/b/c `396cc0ba` / `68fd6150` / `a12767e8`, edge `8e393245`.
- **Row:** repo 65602 (run `mwp3ub25`), 8 learners, k 4, leader site 0 term 2, shards warm. First request 15:02:32Z; the burst reached the leader at 15:03:44Z to 15:03:49Z on the server clock (14.3 s ahead of the orb's). Done 15:06:19Z.
- **Harness outage:** from the first request, 3 s, share 0.0181. Valid.
- **Spend:** $2.84 for the whole account from 14:42:42Z to 15:08:10Z, $0.67 of it on graft-m3f (stop $8).
- **Close-out:** group torn down; second sweep OK. `ssh.graftyard.dev` gave slot A's three fingerprints at 14:57Z and 15:11Z.

### Result: fail, valid [measured]

| Gate line | m3fst1 | m3fac1 |
|---|---|---|
| Every push acknowledged within 5 s, or `full` | **Fail: 332 of 500 (66.4%)**: 153 of 321 `ok` within 5 s, plus 179 `full`; 0 unanswered. `ok` edge total p50 5,161 ms, p99 7,355 ms | 311 of 500 |
| No other refusal shape | Pass | Pass |
| Queued agent bytes under `AGENT_QUEUE_MAX` | Pass: peak 197,509 B | Pass |
| Every merge in one of the next 2 entries, none over 2 s | **Fail:** 5 of 30 merge pushes over 2 s on the edge clock (max 15,215 ms); `merge:only` 21 × 200, 3 × 409, 1 × 503 `warming` | Fail |
| Zero leader changes | Pass: term 2, site 0 | Pass |
| Zero `suspect` | **Pass: 0** from the edge; no push missed a receipt. The voters' 3 `Reported` rows fell at 14:58:43Z, in group setup | Fail: 69 |
| No object reset from memory | Pass: wasm high 37.4 MB | Pass |

- **`full`:** 177 refused by the leader after a send whose budget ran out in its queue, 2 at the edge (`bound`). Their read gate took p50 1,682 ms, so they reached the commit send with p50 1,535 ms of AGENT_PUSH_BOUND left (the `ok` pushes, 3,033 ms). 112 of the 120 pushes in the gate's slow mode ended `full`, all from send position 200 on.
- **Stages, `ok` pushes, p50 / p99 ms:** front 696 / 1,040; auth 120 / 340; resolve 446 / 946; gate when it read rows 610 / 1,748 (108 pushes); first commit 2,178 / 3,544. Committed twice: 138 of 321 (43%); `stagefirst` 228 of 500. Leader commit (`/stats`, whole row) 483 / 2,448; round p50 185.
- **Gate slow mode:** 120 pushes at 1.5 s or more. Their slowest `repo:65602` pass: 69 follower tries answered `overloaded` at 900 to 1,300 ms (`FOLLOWER_WAIT`), and 32 more answered `ok` after 1.3 s.
- **A34:** of 1,254 `/commit` and ask arrivals at the leader in the burst (1,126 and 128; counted with `a34.py`'s corrected route codes), none met a pending deferred flush; no flush ran 500 ms late; no push missed a receipt.

### Confirms of 1 s or more, with `syncs.py`'s label [measured, `Trace::Sync`]

Leader (site 0): 1,251 confirms in the row, wall p50 51 ms, p99 243 ms, max 1,449 ms. Followers: max 270 and 227 ms.

| At (s from 15:03:44Z) | Wall | Statements | Bytes | Wakes inside | Requests inside | Lag start / end | Label |
|---|---|---|---|---|---|---|---|
| 5.48 | 1,000 ms | 221 | 75,532 | 10 | 101 | 0 / 0 | `storage` |
| 6.93 | 1,449 ms | 67 | 8,143 | 14 | 60 | 0 / 0 | `storage` |

- In the burst also: 900 ms (34 statements, 15.9 KB; 9 wakes, 334 requests) and two of 579 ms (1 statement of 191 KB, and 211 of 80.7 KB). All `storage`.
- One confirm outside the burst reached 1,039 ms (1 statement, 175,513 B), also `storage`.
- **So a confirm over 1 s recurred, and this time the isolate stayed free:** the ticker woke and requests entered throughout, and the loop lag was 0 at both ends. The time was the storage write itself. Its size does not predict it: 8 KB took 1.4 s where 191 KB took 0.6 s. m3fac1's 2,436 ms, in which nothing ran, is not repeated; whether that one was `gate held` cannot be told from its trace.
- No ticker stall of 200 ms or more in the burst (m3fac1: 306 and 269 ms).

### Cut ends and the burst's entries [measured, cut trace]

- **Whole row:** 770 cuts, 766 `Empty`, **4 `AgentCmds`**, none `Soft`, `EntryMax` or `AgentTokens`.
- **Burst entries** (s from 15:03:44Z):

| Entry | Cut at | Commands | Soft bytes | Cut to append | Round |
|---|---|---|---|---|---|
| 389 | 0.84 | 59 | 44,309 | 900 ms | 100 ms |
| 390 | 1.75 (`AgentCmds`) | 66 | 47,614 | 90 ms | 1,216 ms |
| 391 | 1.84 (`AgentCmds`) | 65 | 27,187 | 1,216 ms | 169 ms |
| 392 | 3.13 | 44 | 18,009 | 100 ms | 859 ms |
| 393 | 3.69 (`AgentCmds`) | 67 | 47,827 | 392 ms | 394 ms |
| 396 | 4.09 (`AgentCmds`) then 5.48 | 33 | 20,784 | 1,691 ms | 281 ms |

- Entry rounds, whole burst, by size: 1 to 4 commands p50 187 ms; 5 to 15, 226 ms; 16 to 50, 281 ms; 51 to 120, 394 ms (max 1,216 ms). The long rounds and appends sit on the slow confirms above.
- **Agent queue:** at most 68 left after a cut (at 1.75), 7 after the next cut 0.09 s later. Arrivals kept coming until about 5 s, so the queue rose again to 42 and 40 after the cuts at 3.69 and 4.09, and was empty after the cut at 5.48: 3.7 s after the first capped cut, behind the 1,000 ms confirm that ended at 5.48. `/stats` never sampled more than 4 agent commands queued.

### `FOLLOWER_WAIT` attributed: the grant was late, not the apply (code-only follow-up, branch `follower-fast`) [measured from `tsave-m3fst1-b0-32.jsonl.xz` and the a1 file; one inference marked]

The operator asked how far behind the followers that answered `overloaded` at about 1 s were, and whether an immediate answer, or going at once to the leader or another member, would have kept the late pushes' budget. The script that joins each member's read-index reads (`Trace::ReadQuery` with `ms` 1) to its next answered `ReadIndex` call (`IndexSent`/`IndexEnded`/`ResStepped`) and to its applied index (`Applied`) was scratch work and is not committed. The figures below are its output.

- **Read-index reads in the first 30 s of the burst:** 7,982 on the ten non-leader members (374 on the two followers, 7,608 on the eight learners). 791 of them were served 1 s or more after they entered, or not at all (92 on followers, 699 on learners). 751 of those 791 entered between 0.59 and 6.71 s, the same span as the leader's four slow confirms (900 ms ending at 1.64, 579 ms at 3.81, 1,000 ms at 5.48, 1,449 ms at 6.93) and its input gaps of 300 ms or more (1.94 to 2.61, 3.25 to 3.60, 4.73 to 5.23, 5.48 to 6.95).
- **How the 791 split:** waiting for a call slot took p50 54 ms; the read-index round trip took p50 1,152 ms (p90 1,510); applying after the grant took p50 0 ms (0 for 511 of them, p90 271). 498 of their calls ran past `CALL_DEADLINE`.
- **Followers:** at its grant, each of the 92 late follower reads was **0 entries behind**. The round trip was p50 1,187 ms. So the "how far behind" answer is zero: the follower already had the index and waited for the leader to answer.
- **The leader answered at once and the answer reached the follower late.** The leader logged `IndexTaken` and `ResSent` for each call in the same millisecond. Answers then came back in bunches: site b's calls sent from 1.08 to 1.22 s were all answered at 1.51 s, and those sent from 1.51 to 2.16 s at 2.95 s; site c's matched within 25 ms. The bunches end with the leader's 900 ms confirm and its 668 ms gap with no rows at all. That the leader's output gate held the answers until its storage confirms ended is an inference: the two isolates' clocks could not be aligned closely enough to place each answer's departure.
- **The other members were held by the same leader at the same time.** A read arriving at 1.9 s would have been served on the ten members at 2.87 to 3.29 s. One arriving at 5.5 s, at 6.78 to 10.9 s. For 85 of the 92 late follower reads, the other follower, asked 130 ms after the first, would also have taken 1 s or more.

**The fix as asked, replayed.** A 35% sample of the reads was replayed against each member's measured serve times. A member answers `overloaded` after the bound W; the edge then tries the members of the same kind in random order, with `retry_shed`'s pause after every three tries:

| W (follower bound) | Reads 1 s or more | Reads 1.5 s or more | p90 | Member tries per read |
|---|---|---|---|---|
| 1,000 ms (main) | 274 | 193 | 995 ms | 1.11 |
| 600 ms | 405 | 288 | 1,523 ms | 1.41 |
| 400 ms | 544 | 423 | 1,890 ms | 1.93 |
| 250 ms | 739 | 564 | 2,779 ms | 3.05 |

A shorter bound makes the gate slower and multiplies the member reads, because each member it moves to is behind the same late grant. A variant that answers early only while the member's oldest pending read is past a threshold (250 to 600 ms) was worse still: 614 to 915 reads of 1 s or more. Asking a second member at the same time after 150 ms, while the first one's answer is still pending, cut the reads of 1 s or more from 270 to 200 and p90 from 972 to 851 ms: a 26% cut for one extra read per hedged try.

**Budget.** The 112 slow-gate pushes that ended `full` reached the commit send with p50 1,656 ms of `AGENT_PUSH_BOUND` left. Suppose each of their `overloaded` tries had come at 150 ms and the next member had still answered when it actually did: they would have had p50 2,117 ms. At this row's `ok` rate per 250 ms of budget, that is about 51 of 112 kept. That is an upper bound the replay rules out: asked 850 ms sooner, the next member is that much earlier in the same stall and waits that much longer.

**Verdict:** the gate's slow mode on m3fst1 is the leader's late read-index grants during its slow storage confirms (F106), seen through every member at once. It is not a follower that holds a read it cannot serve. The fix as asked (answer at once past a short bound) does not pass this replay, so no gate change was built. Recorded in `decisions/OPEN-QUESTIONS.md`, A35.

## Row m3fac1: B0-32 on main `eaac204b` (`agent-cmds-entry`) (2026-11-20, graft-m3f, `ROWS="b0-32"`)

**Slot.** The orchestrator granted one site day and edge day from main `eaac204b` with `--ssh`, then one B0-32 row, stop $8 on graft-m3f from the site day's start (12:38:08Z). The measure is `agent-cmds-entry` alone: at most 64 Agent commands per entry (`decisions/DECISIONS.md`, "`agent-cmds-entry`").

**Committed raw files:**
- `tests/fanout/raw/rows-m3fac1.status`
- `{admit,terms,vmem}-m3fac1-b0-32.jsonl`
- `tsave-m3fac1-b0-32.jsonl.xz`
- `arrivals-m3fac1.jsonl`
- `shardgate-m3fac1-b0-32-*.jsonl`
- `lm1-rows-m3f-data/m3fac1-raw/a1-b0-32-mwozv8cb.jsonl`, which holds every burst push's `server-timing` and `x-graft-spread`

- **Site day and edge day** m3fac from `eaac204b`, each exit 0. The launcher ran as an orb service, which restarted it each time it exited, so the day ran three times before it was stopped (edge versions at 12:51:55Z, 12:57:04Z and 13:02:37Z); no group existed then. Final: sites a/b/c `232c5d18` / `71d80615` / `b609bf53`, edge `a06a4309`, each at 100%. Images reused; `--ssh` on. The launcher now ends in `sleep infinity`, as `rows.sh` does.
- **Row:** repo 65601 (run `mwozv8cb`), 8 learners, k 4, leader site 0 term 2, shards warm. The first request was at 13:11:14Z; the burst reached the leader at 13:12:25Z to 13:12:28Z on the server clock, 14.1 s ahead of the orb's (`tsave` clock row). The row was done at 13:15:01Z.
- **Harness outage:** from the first request, 3 s, share 0.0181. Valid.
- **Spend:** $2.22 for the whole account from 12:38:08Z to 13:16:39Z, $0.67 of it on graft-m3f (stop $8). The account was at $24.53 for the UTC day at 13:06Z.
- **Close-out:** group torn down; second sweep OK.
- **SSH:** `ssh-keyscan ssh.graftyard.dev` at 13:05Z and 13:17Z gave slot A's three fingerprints: Ed25519 `SHA256:TMEOnX…`, ECDSA `SHA256:XEJXBQ…`, RSA `SHA256:qgOnPP…`.

### Result: fail, valid [measured]

Edge clock (`server-timing` `total`); `full` counts as meeting the line.

| Gate line | m3fac1 | m3fgt1 | m3fwi3 |
|---|---|---|---|
| Every push acknowledged within 5 s, or `ng … agent admission full` | **Fail: 311 of 500 (62.2%)**: 25 of 214 `ok` within 5 s, plus 286 `full`; 0 unanswered. `ok` edge total p50 7,272 ms, p99 8,329 ms | 135 of 500 (27.0%) | 442 of 500 (88.4%) |
| No other refusal shape | Pass | Pass | Pass |
| Queued agent bytes under `AGENT_QUEUE_MAX` | Pass: peak 166,121 B, 180 queued at 13:12:13.8Z (server clock) | Pass: 108,508 B | Pass: 112,715 B |
| Every merge in one of the next 2 entries, none over 2 s | **Fail:** 3 of 27 merge pushes over 2 s on the edge clock (max 5,896 ms); `merge:only` 22 × 200, 1 × 409 | Fail: 2 of 22 | Fail: 1 of 23 |
| Zero leader changes | Pass: term 2, site 0 | Pass | Pass |
| Zero `suspect` | **Fail: 69 `suspect`** from 69 burst pushes, one each; the voters logged 81 `Reported` rows (LAX, 80 promised) | Fail: 169 | Pass: 0 |
| No object reset from memory | Pass: wasm high 34.9 MB | Pass | Pass |

**The 286 `full` answers.** None is a leader refusal at admission time; they are AGENT_PUSH_BOUND (5 s from the push's arrival) running out:
- 75 refused at the edge (`bound`) before any send: 69 at the commit send, 6 at the read gate. All 75 had a read gate in the slow mode.
- 211 refused by the leader after a send whose `budget_ms` ran out in the queue (`expire_agents`): their commit stage p50 1,899 ms, p99 3,746 ms; budget at send p50 2,505 ms. 140 of them had staged once (`AdmitError::Stage`, `stage` p50 539 ms) and sent again.
- `full` by send order: 13, 17, 17, 20, 28, 38, 34, 40, 41, 38 per block of 50. Late arrivals had less budget left.

**Burst `ok` pushes, p50 / p99 ms, edge clock:**

| | m3fac1 | m3fgt1 | m3fwi3 |
|---|---|---|---|
| Committed twice | **77 of 214 (36.0%)**; flip line 5% | 0 of 490 | 206 of 489 |
| `stagefirst` | 137 of 214 (213 of all 500) | 490 of 490 | 283 of 489 |
| front | 688 / 977 | 920 / 1,278 | 695 / 1,149 |
| auth | 121 / 263 | 135 / 353 | 128 / 300 |
| resolve | 435 / 893 | 636 / 1,174 | 438 / 995 |
| gate, when it read rows | 288 / 1,353 (106 pushes) | 600 / 854 | 720 / 2,152 |
| gate's slow mode (1.5 s or more), all 500 pushes | **134**, every one `full` | 0 | 57 |
| first commit | 4,157 / 5,668 | 6,651 / 7,977 | 973 / 1,712 |
| leader commit (`/stats`, whole row) | 394 / 3,549 | 377 / 5,490 | 498 / 1,345 |
| leader round p50 (whole row) | 159 | 146 | 204 |

`stagefirst` came back because main reverted `agent-stage-first` but the edge's `hot` rule still stages first on a group whose leader asked it to stage within HOT_WINDOW. One-commit and two-commit `ok` pushes took about the same (p50 6,976 and 7,666 ms): the second commit is not the cost here.

### Where the double commits came from: not the `hot` stage-first rule [measured from the a1 `server-timing` marks; one inference marked]

Added 2026-11-20 at the orchestrator's request, from the committed `a1-b0-32-mwozv8cb.jsonl`, with no new spend. Each burst push falls in one of four paths:

| Path | `ok` | `full` | Commits |
|---|---|---|---|
| `stagefirst`: the edge's `hot` rule staged the pack before the first commit | 137 | 76 | one |
| Inline commit answered `Stage` by the leader, then staged and sent again | 77 | 140 | two |
| Inline, never staged | 0 | 13 | one |
| No commit sent (`bound` at the gate or at the send) | 0 | 57 | none |

- **All 77 `ok` pushes that committed twice took the leader's `Stage` answer**, and none was `stagefirst`. A push the `hot` rule stages first never commits twice. So the rule causes 0 of the 36%. It removed the second commit from 213 pushes that would otherwise have been sent inline into the same `Stage` answer.
- Counting every push that sent a commit, 217 of 443 (49%) committed twice; 140 of those ended `full`.
- The two paths interleave through the burst: first sends at p50 34.6 s (`stagefirst`) and 34.9 s (`Stage` answer) from the client's start, in every half-second. The `hot` rule is per edge isolate (`hot::STAGED`, a `thread_local`), so an isolate stages first only once one of its own pushes got `Stage` within HOT_WINDOW. With 500 connections spread over many isolates, each isolate's first hot push or pushes still commit twice [inference: the trace does not name the isolate].
- One-commit and two-commit `ok` pushes took about the same edge total (p50 6,976 and 7,666 ms), so on this row the second commit is not where the time went.

### `agent-cmds-entry`'s flip [measured from the leader's cut and turn traces, `tsave-m3fac1-b0-32.jsonl.xz`]

Times are seconds from 13:12:25.5Z on the server clock.

**Cut ends over the whole row:** 889 cuts, 886 `Empty`, **3 `AgentCmds`**, none `Soft`, `EntryMax` or `AgentTokens`. The burst's three cuts of 64 agent commands ended `AgentCmds`.

**The burst's entries:**

| Entry | Cut at | Commands (agent) | Soft bytes | Cut to append | Round | Ticker stalls inside |
|---|---|---|---|---|---|---|
| 421 | 0.99 | 68 (64) | 13,430 | 446 ms | 100 ms | none |
| 422 | 1.54 | 67 (64) | 73,549 | 554 ms | 100 ms | none |
| 423 | 1.54 | 64 (64) | 48,376 | 100 ms | 1,003 ms | none |
| 424 | 2.76 | 39 (37) | 26,624 | 1,103 ms | 496 ms | 306 ms |
| 425 | 3.05 | 14 (11) | 42,948 | 2,817 ms | 614 ms | none |

Entry and round times come from `entries2.py`'s join of cut to append to apply. By size, over the 15 s from 1.5: entries of 1 to 4 commands had round p50 213 ms, 5 to 15 213 ms, 16 to 50 300 ms, 51 to 120 496 ms (max 1,003 ms).

**Ticker stalls:** 306 ms (ending 2.95) and 269 ms (3.51). No stall over 1 s. That half of the flip does not fire.

**The agent queue's drain:** the queue peaked at 195 agent commands left after the cuts at 1.54, and was 0 after the cut at 2.76: 1.2 s from the peak, and 2.2 s from the first 64-command cut at 0.99. `/stats` read 180 queued at 13:12:13.8Z and 27 at 13:12:16.5Z on its own sample clock. Under 3 s, so that half of the flip does not fire either. **`agent-cmds-entry`'s flip did not fire.**

**What held the burst instead: one 2,436 ms storage confirm.** Entry 424's transaction (211 statements, 163,535 B, deferred at 2.65, flushed at 2.95) called `sync()` at 3.14. That confirm took 2,436 ms and ended at 5.58. From 3.51 to 5.58 the leader logged no row at all: no `Entered`, no `/raft`, no ticker wake. Entry 425's append waited behind it (cut to append 2,817 ms), and resolutions went 68 in 3.0 to 3.5, then 1 by 5.58, then 42.
- Its followers had applied entry 423 by 5.44 to 5.54, and their own confirms took 2 to 13 ms. The stall was the leader's storage, not its peers.
- No other leader confirm in the row went over 1,000 ms; the whole-row p50 was 44 ms and p99 138 ms (1,429 confirms). m3fwi2 and m3fwi3's largest were 319 and 192 ms; m3fgt1's 1,799 ms was entry 446's 600-statement flush.
- Site a's member logs for 13:12:24Z to 13:12:36Z (Workers Logs, on the server clock): `/commit` wall p50 861 ms, p90 3,320 ms, p99 3,826 ms; `/outcome_by_request` p50 329 ms, p99 900 ms; `/raft` p99 615 ms. Followers b and c logged `/raft` walls of about 3.1 s at p90: the calls the leader held.
- Why one `sync()` of 211 statements took 2.4 s while the row's others took under 140 ms is not attributed. `decisions/OPEN-QUESTIONS.md`, F106.

### The read gate's slow mode: `FOLLOWER_WAIT`, behind the leader's stall [measured from `x-graft-spread`]

- 134 pushes had a gate of 1.5 s or more, every one of them `full`. In each, the slowest pass was the gate's own `repo:65601` read; 128 of 134 used a second or third member (69 three tries, 59 two), and 6 were shed.
- Their member tries: 209 followers answered `overloaded` at about 1 s (203 in 900 to 1,300 ms, 6 over), then the next member answered `ok`. That is the `FOLLOWER_WAIT` mechanism m3fgt1 could not test.
- Followers wait for the leader's index, and from 3.51 to 5.58 the leader took no input, so the followers' `overloaded` answers are F106's stall seen through the gate [inference]. No gate fix is built; the cause is upstream.

### A34: the asks and misses against flush waits [measured from the same trace]

**Joined at the leader.** Each `/commit` and `/outcome_by_request` `Entered` row is matched to the deferred flush pending when its answer could leave (`settled()`): arrival plus `COMMIT_HOLD` for `/commit`, arrival for an ask (`tests/fanout/a34.py`, which reads `Trace::Flushed`'s deferral span).

| | m3fac1 | m3fgt1 |
|---|---|---|
| `/commit` arrivals in the burst | 1,105: 1,103 no flush pending, 2 under 500 ms | 1,040: 923 none, 9 under 500 ms, 32 at 500 to 1,000 ms, 76 at 1 to 2 s |
| `/outcome_by_request` arrivals | 367: all none | 682: 424 none, 168 under 500 ms, 15 at 500 to 1,000 ms, 75 at 1 to 2 s |
| Deferred flushes 500 ms late or more | none | 2 (1,515 and 1,730 ms) |

Corrected 2026-11-20: the first version of this table read route codes 2 and 5 (`/read` and `/outcome`) as `/commit` and `/outcome_by_request`. Route codes are `ROUTES`' index + 1 (3 and 6), fixed in `tests/fanout/a34.py`. The conclusion stands: m3fac1's arrivals met almost no flush waits, and m3fgt1's met real ones.

**At the edge** (`server-timing` marks):

| Marks | Pushes | Result |
|---|---|---|
| `send ask` | 146 | 98 `ok`, 48 `full` |
| `send stage send` | 93 | 92 `full`, 1 `ok` |
| `send stage send ask` | 89 | 41 `ok`, 48 `full` |
| `bound` | 75 | `full` |
| `send stage send ask missed suspect send` | 35 | `ok` |
| `send ask missed suspect send` | 29 | `ok` |
| `send` | 28 | 23 `full`, 5 `ok` |
| `send ask missed suspect send ask` | 5 | `ok` |

- 304 pushes asked; 69 missed a receipt, each once, each after an ask; none after a send alone.
- **No miss follows a flush wait.** The deferred flushes ran on time throughout (none 500 ms late), while the 2,436 ms `sync()` happened after its flush, which `settled()` does not wait for. The asks that missed met a leader that took no input at all from 3.51 to 5.58, so answering `admitted`/`pending` without `settled()` (A34) would not have answered them either [inference from the trace: the leader logged no `Entered` in that span].
- So A34 is not confirmed by this row, and the misses point to F106's stall.

## Row m3fgt1: B0-32 on main `19b4385f` (`agent-stage-first`) with gate tracing (2026-11-20, graft-m3f, `ROWS="b0-32"`)

**Slot.** The orchestrator granted one site day and edge day from `19b4385f` with `--ssh`, then one B0-32 row, stop $8 on graft-m3f from the site day's start (10:43:34Z). The day ran from branch `gate-trace` (`fe3f8c64`): `19b4385f` plus test-faults-only tracing. Each push answers `x-graft-spread`, which lists every spread read pass with its member tries, and `server-timing` gains `gate_rows` and `gate_authz`. The measures are `agent-stage-first` and the `FOLLOWER_WAIT` hypothesis for the read gate's slow mode.

**Committed raw files:**
- `tests/fanout/raw/rows-m3fgt1.status`
- `{admit,terms,vmem}-m3fgt1-b0-32.jsonl`
- `tsave-m3fgt1-b0-32.jsonl.xz`
- `arrivals-m3fgt1.jsonl`
- `shardgate-m3fgt1-b0-32-*.jsonl`
- `lm1-rows-m3f-data/m3fgt1-raw/a1-b0-32-mwovbevd.jsonl`, which holds every burst push's `server-timing` and `x-graft-spread`

- **Site day and edge day** m3fgt, 10:43:35Z to 10:56:55Z, both exit 0 on the first try. Images were reused (`containers` unchanged since `b655c323`). `--ssh` was on.
- **Row:** repo 65600 (run `mwovbevd`), 8 learners, k 4, leader site 0 term 2, shards warm. The run started 11:03:29Z; the burst was 11:04:29Z to about 11:04:31Z on the client clock. The row was done at 11:07:28Z.
- **Harness outage:** the window opens at the first request, 11:03:42Z, under `outage-window`. 6 s of outage, share 0.0361. Valid.
- **Spend:** $0.89 for the whole account from 10:43:34Z to 11:09Z, $0.69 of it on graft-m3f (stop $8).
- **Close-out:** group torn down; second sweep OK.
- **SSH:** `ssh-keyscan ssh.graftyard.dev` at 10:57Z and 11:15Z gave slot A's three fingerprints: Ed25519 `SHA256:TMEOnX…`, ECDSA `SHA256:XEJXBQ…`, RSA `SHA256:qgOnPP…`.

### Result: fail, valid; `agent-stage-first`'s flip fired [measured]

Edge clock (`server-timing` `total`); `full` counts as meeting the line.

| Gate line | m3fgt1 | m3fwi3 |
|---|---|---|
| Every push acknowledged within 5 s, or `ng … agent admission full` | **Fail: 135 of 500 (27.0%)**: 125 of 490 `ok` within 5 s, plus 10 `full`; 0 unanswered. Edge total p50 8,713 ms, p99 10,539 ms | 442 of 500 (88.4%); p50 4,092 ms, p99 5,684 ms |
| No other refusal shape | Pass | Pass |
| Queued agent bytes under `AGENT_QUEUE_MAX` | Pass: peak 108,508 B. The leader's queue peaked at 307 commands at 11:04:42Z | Pass: 112,715 B, 74 queued |
| Every merge in one of the next 2 entries, none over 2 s | **Fail:** 2 of 22 over 2 s, max 6,803 ms; 1 × 409 | Fail: 1 of 23 |
| Zero leader changes | Pass: term 2, site 0 | Pass |
| Zero `suspect` | **Fail: 169 `suspect`** from 115 burst pushes that missed a receipt | Pass: 0 |
| No object reset from memory | Pass | Pass |

**Burst `ok` pushes, p50 / p99 ms, edge clock:**

| | m3fgt1 | m3fwi3 |
|---|---|---|
| Committed twice | **0 of 490** | 206 of 489 |
| `stagefirst` | 490 of 490 | 283 of 489 |
| front | 920 / 1,278 | 695 / 1,149 |
| auth | 135 / 353 | 128 / 300 |
| resolve | 636 / 1,174 | 438 / 995 |
| gate, when it read rows | 600 / 854 (270 pushes); max 1,347 | 720 / 2,152 |
| gate's slow mode (1.5 s or more) | **0** | 57 |
| first commit | 6,651 / 7,977 | 973 / 1,712 |
| leader commit (`/stats`, whole row) | 377 / 5,490 | 498 / 1,345 |
| leader round p50 (whole row) | 146 | 204 |

- `agent-stage-first` removed the second commit round as built: 0 pushes committed twice. But pushes that staged first took edge p50 8,713 ms, against 4,582 ms for m3fwi3's two-commit pushes. That meets its flip (`decisions/DECISIONS.md`, "`agent-stage-first`", "Flip fired"), and it is reverted on branch `gate-trace`.
- **Commit time by send order:** the first 50 burst pushes had commit p50 1.3 s; every later block of 50 had 6.7 to 7.3 s.
- **Commit time by the edge's marks:** 266 pushes had one send and one ask (commit p50 6,355 ms); 60 had no ask (976 ms); 115 missed a receipt (61 with one miss, 54 with two).

### `FOLLOWER_WAIT` and the read gate's slow mode: not confirmed on this row [measured]

- **Passes:** 1,569 spread passes on burst pushes. 1,548 used their first member (followers 1,311, learners 254).
- **Moved on:** 21 passes moved to a second member, every one after learner site 2 answered `overloaded` at 1,115 to 1,258 ms. That is the `FOLLOWER_WAIT` mechanism. 4 of the 21 were gate reads; 17 were the push's own agent-rows read.
- **Shed:** 4 passes were shed after three tries, at about 3.3 s.
- **Slow mode:** the gate never reached 1.5 s (max 1,347 ms), so this row cannot attribute m3fwi3's slow mode. No gate fix was built.

### Where the burst's commit time went [measured from the leader's cut and turn traces, `tsave-m3fgt1-b0-32.jsonl.xz`]

**Clocks.** Trace times are the server's clock, 13.8 s ahead of the orb's (`tsave` clock row). The burst's commits reached the leader at about 11:04:54Z on the server clock.

**Arrival at the leader:**
- **m3fgt1:** 406 `/commit` and 322 `/outcome_by_request` reached the leader in one half-second, beside 538 `/raft` (`Trace::Entered`).
- **m3fwi3:** the refused first commits and the bucket places spread the same burst over about 3 s, and the leader's queue peaked at 74.

**Entries and rounds.** Each entry is one leader cut (`Trace::Cut`), joined to its append (`Stepped`) and apply (`Applied`).

| Commands per entry | m3fgt1: entries, round p50 (max) | m3fwi3: entries, round p50 (max) |
|---|---|---|
| 1 to 4 | 581, 143 ms (1,177) | 349, 193 ms (583) |
| 5 to 15 | 249, 152 ms (1,305) | 301, 214 ms (731) |
| 16 to 50 | 1, 1,549 ms | 7, 248 ms (329) |
| 51 to 120 | 2, 259 ms | 6, 471 ms (578) |
| over 120 | 2, 2,553 and 2,844 ms | 0 |

**The burst's entries** (server clock from 11:04:54Z):

| Entry | Commands | Soft bytes | Cut to append | Round | Ticker stalls inside |
|---|---|---|---|---|---|
| 444 | 74 | 71,570 | 214 ms | 259 ms | none |
| 445 | 35 | 14,227 | 1,343 ms | 1,549 ms | 2,492 ms |
| 446 | 191 | 99,892 | 2,842 ms | 2,553 ms | 1,731 ms |
| 447 | 160 | 66,268 | 5,395 ms | 2,844 ms | 822 ms |

- **Cut ends:** the 191-command cut ended `Soft`, at 99,892 of `ENTRY_SOFT`, with 157 left and 248 agent tokens unspent. The 160-command cut ended `Empty`. No burst cut ended `AgentTokens`, so the token bucket never bound.
- **The agent queue drained faster than admission capacity allows.** It went from 307 queued to 0 within one 1 s sample, because `cut_early` froze batches of 191 and 160 behind entry 445. The wait was in frozen batches behind slow entries, not in the lane. `/stats` `queue` (admission to append) p99 was 3,113 ms.
- **Resolutions answered per second** (`Trace::Work`): 146 in the first second, none for 2 s, 191, none for 2 s, 160, then 45 and 47. The burst resolved in three lumps over about 9 s. Admission capacity read 750 to 875 commands/s during the burst; the burst drained at about 55/s.
- **Ticker stalls:** three, of 2,492, 1,731 and 822 ms, fell inside the three slow burst entries. m3fwi3 had none in its burst.
- **Work in those turns:**
  - Entry 446's apply was one turn of 600 statements (430,767 B), 374 events and 191 resolutions.
  - Its deferred transaction ran 1,730 ms after it was deferred; entry 445's ran 1,515 ms after.
  - Entries 445 to 447 were each encoded to 7 to 10 peers per step, 1.48 to 2.07 MB per step.
  - Workers Logs show leader-site `/raft` invocations of 557 and 615 ms CPU at 11:04:58Z to 11:04:59Z.
  - Which of these stalls the turn is [inference]: M4-WP4's B0-35 attribution of the same leader's `/raft` CPU is the owner of that question.

**Why 430 pushes asked and 115 missed a receipt:**
- **Asks.** The member's `/commit` answers after `COMMIT_HOLD` (1 s) with `admitted`, but only once `ready_or` sees `settled()`. In the burst's seconds, site A's member `/commit` wall was p50 3,709 ms and p99 4,551 ms (Workers Logs, sampled, 11:04:51Z to 11:05:00Z). The deferred flush and the stalls held the hold's answer past the edge's `COMMIT_RECEIPT` (2 s). So most pushes asked `/outcome_by_request`, which answered `pending` (wall p90 603 ms in the sample).
- **The pushes' receipt marks, in order** (`server-timing`):

  | Marks | Pushes |
  |---|---|
  | `send ask` | 266 |
  | `send` | 60 |
  | `send ask missed suspect send ask missed suspect send` | 54 |
  | `send ask ask` | 49 |
  | `send ask ask missed suspect send` | 39 |
  | `send ask missed suspect send ask` | 22 |

  Every miss followed an ask, and none followed a send alone.
- **Misses.** An ask has `CALL_DEADLINE` (1 s). One that met a stall had no answer in time, and an isolate whose last 200 from the leader was more than 2 s old is not `busy`, so the edge reported `suspect` and sent again. A queued, admitted command therefore read as a dead leader. DESIGN's write path says queue time counts against no receipt; that holds for the queue itself, but not for a hold answer that a turn stall delays [inference from the code path and the logs; per-push last-heard times are not traced].

## Rows m3fwi2 and m3fwi3: B0-32 on main `b655c323` (identity recs 2a, 2b, 2c) (2026-11-20, graft-m3f, `ROWS="b0-32"`)

**Slot.** Granted by the orchestrator after `write-identity` merged: one site day and edge day from main `b655c323` with `--ssh`, then one B0-32 row, stop $8 on graft-m3f from the site day's start (08:08:24Z), through the egress proxy under the harness-outage rule. The measure is the three review recs together: `git-read-class` (2a, git fetches read-class; 2b, `resolve`'s reads started at once), and `write-identity` (2c, a decided write takes its token or `jti` row from the edge's cache). Committed raw files: `tests/fanout/raw/rows-m3fwi{2,3}.status`, `{terms,admit,vmem}-m3fwi{2,3}-b0-32.jsonl`, `tsave-m3fwi{2,3}-b0-32.jsonl.xz`, `arrivals-m3fwi{2,3}.jsonl`, `shardgate-m3fwi{2,3}-b0-32-*.jsonl`. The a1 files (`a1-b0-32-mwopyfzd.jsonl`, `a1-b0-32-mwoqek4r.jsonl`, their `a1-pushes-` and `a1-samples-`) are under the ignored `tests/am1/raw/` on this thread's orb.

- **Site day and edge day** m3fwi, 08:08:24Z to 08:23:16Z, both exit 0 on the first try: sites a/b/c `76098911` / `1551dcbb` / `ddc3e0b7` (phase 1 `925df47a` / `743752d9` / `9b1de312`), edge `448bc161`, each at 100%. Shard image `sha256:076a16ee…` and sshfront `sha256:fc6eda91…` reused (`containers` unchanged since `f413f2ac`). The edge's `--ssh` public JSON was read back from the live `meta/ssh` (slot A active).
- **Sweep fix.** `teardown.py --sweep` counted the three always-on `-sshfront` containers (`ssh#0..2`) as shards and failed, which would stop `rows.sh` at its start. The sweep and teardown now skip a site's `-sshfront` application (`df0f5253`).
- **First try, m3fwi:** `mkrepos.mjs` read the tools token from the main checkout and created nothing ("no repo"); no group, no spend. The launcher now sets `TOOLS_TOKEN_FILE`.
- **m3fwi2:** repo 65598 (run `mwopyfzd`), 8 learners, k 4, leader site 0 term 2, shards warm, burst 08:34:43–51Z. Invalid by the rule as first applied (share 0.0611 from the announced start), so rerun; **valid** once the window opens at the first request (share 0.0241, below).
- **m3fwi3:** repo 65599 (run `mwoqek4r`), the same setup, burst 08:47:16–25Z. Invalid as first applied (0.0667); **valid** from the first request (0.024).
- **Spend:** $1.39 on graft-m3f by `PREFIX` from 08:08:24Z to 09:01Z (stop $8); the account $12.20 for the UTC day at 09:01Z.
- **Close-out:** both groups retired, 12 of 12 names each; purged 5,671 objects, second pass 0, none refused; none joined of 2; sweeps with no shard container running (09:00Z).
- **SSH after the row:** `ssh-keyscan ssh.graftyard.dev` at 09:01:13Z gave slot A's three fingerprints (Ed25519 `SHA256:TMEOnX…`, ECDSA `SHA256:XEJXBQ…`, RSA `SHA256:qgOnPP…`), and `meta/ssh` answers `active: A`.

### The outage windows fall mostly before the client's first request; re-judged valid (operator, 2026-11-20) [measured]

`outages.sh` opens the window at a1's announced start (08:46:02Z and 08:33:29Z). a1's first sample started 13 and 14 s later (08:46:15Z, 08:33:43Z), after its setup. Of m3fwi3's 12 edge-0 seconds, 8 fell before the first request; of m3fwi2's 11, 7. Those are the client's idle seconds, which rule 5 already says do not mean a dropped host. The other 4 in each row are in the burst's upload, when one orb sends 125 MiB on 500 connections (edge 0 to 8 invocations a second while the sites serve), and the second at the first request. Counted from the first request, each row has about 4 to 5 outage seconds of 167, a share near 0.03, which would be valid. **Re-judged (operator, 2026-11-20; `decisions/DECISIONS.md`, "`outage-window`").** The window opens at the client's first request. Recomputed with `arrivals.py`'s own `outages` and `summary` over the committed arrivals: m3fwi3 from 08:46:15Z, windows 08:46:15–16Z, 08:47:19–20Z and 08:47:23–25Z, 4 s, share 0.024; m3fwi2 from 08:33:43Z, windows 08:33:43–44Z and 08:34:48–51Z, 4 s, share 0.0241. **Both rows are valid, and both fail.** Every burst line below reads the same either way, because the gate judges each burst push by its own edge `total`. `outages.sh` now computes this window itself.

### Result: fail on both runs, both valid [measured]

Edge clock (`server-timing` `total`). `full` counts as meeting the line (the `ng` answer). The client clock includes one orb uploading 500 × 256 KiB through the egress proxy: 0 of 500 within 5 s on it in both runs (client p50 11.2 s and 10.5 s).

| Gate line | m3fwi3 | m3fwi2 | m3fe2 |
|---|---|---|---|
| Every push acknowledged within 5 s, or `ng … agent admission full` | **Fail: 442 of 500 (88.4%)**, 431 of 489 `ok` within 5 s plus 11 `full`; 0 unanswered; edge total p50 4,092, p99 5,684, max 5,859 ms | **Fail: 436 of 500 (87.2%)**, 411 of 475 `ok` plus 25 `full`; 0 unanswered; p50 4,034, p99 5,736 | 496 of 500, met at 90% |
| No other refusal shape | Pass | Pass | Pass |
| Queued agent bytes under `AGENT_QUEUE_MAX` | Pass: peak 112,715 B, 10 queued | Pass: 110,487 B | Pass |
| Every merge in one of the next 2 entries, none over 2 s | **Fail:** 1 of 23 at 12,077 ms (`merge_plan` 11,480: the shard's `waited` 11,247), the rest under 2 s; 4 × 409 | **Fail:** 4 of 27 over 2 s (max 12,279, the same shard wait) | Pass |
| Zero leader changes | Pass: term 2, site 0 | Pass | Pass |
| Zero `suspect` | Pass: 0 from the edge; the voters' 2 `Reported` rows fell at 08:41:14Z, in group setup before the row (as m3fe2's) | Pass: same, 08:28:10Z | Pass |
| No object reset from memory | Pass: no voter's wasm dropped, high 35.1 MB | Pass: 33.3 MB | Pass |

Leader commit p50 / p99 498 / 1,345 ms (m3fwi2 454 / 1,207), against m3fe2's 332 / 856.

### The front, with recs 2a, 2b and 2c

Burst pushes' `server-timing` (`ok` only), p50 / p99 in ms. The front is max(auth + install, resolve).

| Stage | m3fwi3 | m3fwi2 | m3fe2 |
|---|---|---|---|
| auth | 128 / 300 | 200 / 400 | 120 / 280 |
| install | 557 / 888 | 649 / 940 | 600 / 994 |
| resolve | **438** / 995 | 545 / 1,589 | 551 / 949 |
| front | **695** / 1,149 | 894 / 1,589 | 767 / 1,180 |
| wall before the first send | 2,010 / 3,756 | 2,000 / 3,834 | 1,885 / 3,434 |
| after the first send | 1,652 / 2,755 | 1,828 / 2,764 | 1,338 / 2,539 |
| total | 4,092 / 5,684 | 4,034 / 5,736 | 3,541 / 4,863 |

- **auth did not move, as built** [inference from the code]. Every burst push is the vendor App's `ghs_` token. `write-identity` caches the `jti` row for a decided write, but the verifying keys stay fresh for every write (only the edge checks a key's retirement), so `ident::stateless` still reads TokenShard(0) per push. None of 489 pushes had auth at or under 20 ms. A stored-token push would take the held row; this row has none.
- **resolve (2b)** came down on m3fwi3 (p50 438 against 551) and not on m3fwi2 (545). The front followed it: 695 and 894 against 767. Two runs do not separate rec 2b from run-to-run spread.
- **The tail is after the front.** Of m3fwi3's 58 `ok` pushes over 5 s, 39 waited on a read gate that read its rows (gate p50 720 ms when it read, p99 2,152), and 36 ran two commits (staged, then committed). Commit p50 rose to 498 ms from 332, so "after the first send" rose by about 300 ms. That, not the front, sets the 58.
- **rcread** p50 2 (m3fe2 5): fewer fresh reads per push, from the read-class and held entries.

### Where m3fwi's slow pushes lost their time (code-only follow-up, branch `tail-attr`)

Each lever is judged against the `ok` burst pushes over 5 s on the edge clock (`server-timing` `total`), from the a1 files' `burst timing` steps. A push is rescued when its total less the lever's saving is at or under 5 s. "ms over" is the sum of each slow push's time past 5 s.

| Lever | m3fwi3 (58 slow of 489 `ok`, 16,508 ms over) | m3fwi2 (64 slow of 475, 22,652 ms over) |
|---|---|---|
| Read gate's slow mode (gate 1,500 ms or more) cut to 720 ms | 35 rescued, 13,415 ms removed (57 pushes in the mode) | 54 rescued, 21,587 ms |
| No second commit round (stage first) | 33 rescued, 13,123 ms (206 pushes committed twice) | 51 rescued, 20,231 ms |
| Leader commit at m3fe2's speed (scaled estimate) | 48 rescued | 44 rescued |
| Gate and second round together | 40 rescued; 18 slow pushes had neither | 52 had both; every slow push read its gate |

- **The read gate is bimodal:** one mode at 250 to 1,000 ms, and one at 1,500 to 2,300 ms. The slow mode is cold isolates (`rcread` 7). That it is a learner read waiting `FOLLOWER_WAIT` (1 s) and then trying the next member is a hypothesis; followers' `/read` p99 was about 1 s.
- **The second round is deterministic.** 191 of the 206 two-commit pushes also read their gate, and stage-first pushes almost all had a cached gate. So a cold isolate pays both. The leader's `agent_stage` rule answers `Stage` to every Agent inline pack past `AGENT_INLINE_ENTRY` (128 KiB) on any group, and the edge staged first only after it had seen a `Stage`. Per push, a two-commit push paid p50 287 ms for the refused commit, 543 ms to place, and 1,385 ms for the second commit. A push that staged first paid p50 1,351 ms for its one commit. The round cost p50 900 to 1,164 ms.
- **Leader admission** (hot samples): round p50 218 ms against m3fe2's 133, and queued p99 74 against 10. About 210 of the burst's commits carried a 256 KiB inline pack the leader then refused.
- **Fix built:** `agent-stage-first` (`decisions/DECISIONS.md`). An Agent push with an inline pack over `AGENT_INLINE_ENTRY` stages first on any group. It removes the second round with no new read, and takes the refused 256 KiB bodies off the leader. Its effect on the burst, and on leader commit p50, is [inference] until a B0-32 rerun. The gate's slow mode is the next lever. It needs its cause measured first.

## G11 built: a planned rehome warms the target's shard first (2026-11-19, code-only, branch `g11-warm`)

Approved by the orchestrator after B0-31 was met on m3fr16, which left no other B0-31 item in BUILD-PLAN. `decisions/DECISIONS.md`, "`g11-warm`", has the decision and its flip.

- **What changed:** before `move_step`'s `Transfer`, a repository group's leader asks the target site's first shard of the network for `/catchup` (`next=1`, 5 s wait, 10 s call). While it answers anything but 200, the step answers `{"warming": true, "waitedMs": ..}` and does not transfer. After `WARM_BEFORE_TRANSFER` (60 s [guess]) it transfers anyway, answers `"cold": true`, and logs `transfer to site N cold after N ms`. The gate is the pure `graftyard_identity::group_move::transfer_gate`.
- **Callers:** the audit consumer's move asks again after 5 s, without counting the held asks against its 40 steps. `tests/am1/runs/transfer.mjs` already asks again every 3 s until `Done`, for up to 300 s, so the wait fits inside it.
- **Tests:** three in `group_move`: holds while warming then goes warm; goes cold at the deadline (and warm if warm by then); a wait left by a stopped move starts again. `tests/m0carried/plant49.py` catches 5 of 5, the first being a transfer that goes at once.
- **Not changed:** `tests/fanout/rows.sh` keeps `shardwarm.mjs` for B0-31 until a hosted rehome shows the move warmed the shard itself.
- **Hosted check, not run:** one rehome on graft-m3f with a site day from this branch. It should read the step's `waitedMs` and `cold`, and MergeQueue merges in the minute after the hand-off against m3fr7's 5 of 5 `warming`.

## Rows m3fe2: B0-32 on main `8c794619` (`edge-read-batch`) (2026-11-19, graft-m3f, `ROWS="b0-32"`)

**Slot.** Granted by the orchestrator after m3fb32: one edge day from main `8c794619`, then one B0-32 rerun, inside the $10 stop on graft-m3f counted from 00:38:18Z. The run went through the egress proxy under the harness-outage rule (`tests/fanout/README.md`, "Harness outages on gate rows"). m3fe1, the first try on the same edge, was stopped in placement for a restart and has no result (`install-m3f.md`). Committed raw files: `tests/fanout/raw/rows-m3fe2.status`, `tests/fanout/raw/{terms,admit,vmem}-m3fe2-b0-32.jsonl`, `tests/fanout/raw/tsave-m3fe2-b0-32.jsonl.xz`, `tests/fanout/raw/arrivals-m3fe2.jsonl`. The a1 files (`a1-b0-32-mwn5807e.jsonl`, `a1-pushes-b0-32-mwn5807e.jsonl`, `a1-samples-b0-32-mwn5807e.jsonl.gz`) are under the ignored `tests/am1/raw/` on this thread's orb.

- **Edge day** m3fe1, 03:52Z to 03:53:49Z, `edge exit 0`: edge `a69d83c8` at 100% (rev `fb89ba98`, main `8c794619` plus a log commit). No site day: `edge-read-batch` changes only the edge, so the sites stayed on m3fb32's build (main `76c3e131`).
- **Setup:** repo 65592 (run `mwn5807e`), 8 learners, k 4 at index 8, leader site 0 at term 2. The burst left at 06:08:46.6Z on Cloudflare's clock, after a 60 s warm-up.
- **Colos:** client DFW. The edge served the burst window's receive-packs in LAX (271), DFW (219) and SJC (125). m3fb32's were DFW 272 and LAX 151; m3fr17's were ATL, LAX, EWR and DFW.
- **Harness outages:** 06:07:36–42Z and 06:07:44–47Z, 9 s, share 0.05, valid. Both fell before the burst, so every burst line reads the same with the outages removed.
- **Spend:** $1.48 on graft-m3f by `PREFIX` from 00:38:18Z to 06:46Z (stop $10); the account was $16.64 for the UTC day.

### Result: met at 90%, provisionally (orchestrator, 2026-11-19) [measured]

Judged on the edge clock (`server-timing` `total`), as from m3fb32 on. **Client clock includes orb connection setup:** the client clock includes opening 500 new HTTP/1.1 connections from one orb, which a production fleet keeping its connections alive does not pay. On the client clock 3 of 498 `ok` came within 5 s (p50 7,686 ms, p99 10,209 ms).

| Gate line | m3fe2 (measured = outages removed) | m3fb32 | m3fr17 |
|---|---|---|---|
| Every push acknowledged within 5 s, or `ng … agent admission full` | **496 of 500 (99.2%), met at 90%.** 498 `ok`, 0 `full`, 2 with no answer (below). Edge total p50 3,541 ms, p99 4,863 ms; the 2 `ok` over 5 s took 5,056 and 5,105 ms | 80 of 310 `ok`, 190 `full` | 167 of 474 `ok`, 20 `full`, 6 lost before the edge |
| No other refusal shape | Pass: 0 burst 503s | Pass | Pass |
| Queued agent bytes under `AGENT_QUEUE_MAX` (64 MiB) | Pass: admit trace peak 132,877 B, 22 queued | Pass | Pass |
| Every merge in one of the next 2 entries, none delayed over 2 s | Pass: 24 of 24 `merge:only` 200, edge max 1,603 ms (client p99 1,668) | Fail (edge max 6,920 ms) | 19 × 200, 1 × 409, 1 × 503 |
| Zero leader changes | Pass: term 2, leader site 0 throughout | Pass | 1 change |
| Zero `suspect` | Pass: 0 from the edge (`pushTiming`). The 3 voter `Reported` rows at 06:01:52Z (colo LAX) fell during group setup at term 1, before any load; the orchestrator ruled them not the row's | 34 edge, 32 + 32 `tsave` | 5 edge, 18 + 18 |
| No object reset from memory | Pass: one boot per voter, wasm high 34.5 MB | Pass | Pass |

Leader commit p50 332 ms, p99 856 ms, against m3fb32's 568 and 1,660.

### The front, with `edge-read-batch`

Burst pushes' `server-timing` (498 `ok`), p50 / p99 in ms. The front is max(auth + install, resolve).

| Stage | m3fe2 | m3fb32 | m3fr17 |
|---|---|---|---|
| auth | **120** / 280 | 591 / 754 | 170 / 2,578 |
| install | 600 / 994 | 905 / 1,204 | **416** / 959 |
| resolve | **551** / 949 | 1,390 / 2,051 | 594 / 2,432 |
| front | **767** / 1,180 | 1,563 / 2,051 | 775 / 3,338 |
| gate + context + validate, then the front: wall before the first send | 1,885 / 3,434 | 2,933 / 4,264 | 2,114 / 5,289 |
| after the first send | 1,338 / 2,539 | 3,000 / 5,726 | 3,120 / 4,882 |
| total | 3,541 / 4,863 | 6,431 / 7,898 | 5,538 / 7,443 |

- **Batch counters:** `rcread` 5 / 7, `rcshare` 1 / 7, `rcwait` 660 / 1,940 ms (max 2,160). 367 of 498 pushes waited in a batch.
- The front met the 775 ms target and `rcwait` p99 stayed under the 4 s flip line, so `edge-read-batch` holds (DECISIONS, "`edge-read-batch`").
- **The gap:** install p50 is 600 ms against m3fr17's 416. It does not come down with burst order (quintile medians 509, 593, 632, 628, 619 ms), and 74 pushes had install over 500 ms with `rcwait` under 100 ms, so not all of it is batch waiting.

### The two pushes with no answer: never reached the edge [measured]

k 188 and 189 (client ids `mwn5807e-486` and `-487`, refs `refs/heads/claude/a1-mwn5807e-188-b` and `-189-b`, new tips `7986bc09…` and `78b8b9d2…`) left the client at 06:08:48.20Z on the burst's HTTP/1.1 pool. Both ended in the client's own timeout at 06:15:31Z after 403 s, with status 0 and no request id.

- **The edge never ran them.** From 06:08:40 to 06:16:00Z Workers Logs count 1,126 receive-pack invocations, all outcome `ok` and status 200, the longest 12,122 ms of wall time. That is exactly the number of client pushes in the window that got a request id (1,126 of 1,129; the third without one failed in 10 ms with `fetch failed` at 06:10:36Z). No receive-pack ran after 06:16Z. Across every edge invocation from 06:08:40 to 06:20Z, 1 was `canceled`, and it was a `/_bed/shard_status` call.
- **No log line on any script.** No edge, site a, b or c log line names either ref or either tip (06:08 to 06:30Z). The edge's `push_log`, written on every receive-pack return in a test-faults build, has no line for either client id.
- **Edge arrivals in that second:** the edge took 12 invocations at 06:08:48Z, 3 of them receive-packs, and the sites kept serving. By the rule's definition this is not a harness outage, so the two count against the line as measured, and the line is judged with them as failures.
- **So no invariant broke:** no 2xx left without quorum, and no committed push was reported failed. A push the edge never ran could not reach a leader, so neither ref moved. The group was retired before this trace, so its refs were not read back directly; the evidence is the invocation count and the missing log lines.
- **Which deadline let it run:** none on the edge, which never held them. On the client, `GIT_TIMEOUT_MS` (200 s) is `https.request`'s socket idle timeout, not a wall-clock deadline. Why it fired at 403 s and not 200 s is not settled [unverified].
- **Where they were lost:** between the client's HTTP/1.1 pool and the edge, which is the orb's egress proxy or the pool itself [inference]. m3fr17's 6 "lost before the edge" are the same class.

Recorded in `decisions/OPEN-QUESTIONS.md`, A32.

### Follow-up 1: the burst client's wall-clock deadline (code-only, branch `a1-wall`)

`a1.mjs`'s burst pushes now go through `tests/am1/rest/h1.mjs`'s `postH1`. It ends a request at `GIT_TIMEOUT_MS` (200 s) on a timer that starts at the send, so a request lost before the edge ends at 200 s and is counted, not left open for 403 s.

- **Tagging:** after the burst, each push with no answer (status 0) is looked up in the edge's Workers Logs by its `x-graft-client-push` id (`edgeInvocations`), after `LOGS_LAG_MS` (60 s [guess]). The burst row's `unanswered` counts them as `no edge invocation`, `edge ran it`, or `edge lookup failed`, and each `unacked` entry carries its tag. A lookup that fails three times is never counted as `no edge invocation`.
- **Test** (`node --test tests/am1/rest/h1.test.mjs`): a local server answers 200 headers, then sends a byte every 50 ms and never ends. The deadline must end it at 300 ms. With `postH1` put back to the old idle `timeout`, the test hangs to its 5 s limit and fails. The other tests check that an answer arrives intact, the lookup's query and its failure paths, and the three tags.
- **Live check:** run over m3fe2's window, the lookup tags `mwn5807e-318` `edge ran it`, and `-486` and `-487` `no edge invocation`. That matches the trace above.
- `rows.sh` now writes its "start" line with `~` in place of `$HOME`, so status files carry no home path.

### Follow-up 2: the install gap traced (m3fe2 logs, no hosted spend) [measured]

The question: install p50 600 ms against m3fr17's 416, and 74 pushes with install over 500 ms and `rcwait` under 100 ms. `install` times `install_authz` (`workers/edge/src/lib.rs`): the install account's `Placement` row through `rcache::PLACEMENTS`, then its `Authz` fold through `rcache::AUTHZ`, each one `read_spread` to the learners and followers of a system group.

- **The 74 are cold isolates, not slow reads.** All 74 ran in an isolate with no push before the burst, and 71 were that isolate's first push of the run. Each ran all 7 write-class reads itself (`rcread` 7 in 71 of 74, `rcshare` 0). The burst ran on 116 isolates, and only 2 burst pushes landed in an isolate that had served the warm-up.
- **Member time is the same on both runs.** Sampled `/read` wall time at the objects the install read reaches, burst window, median / p90 in ms:

  | Member | m3fe2 | m3fr17 |
  |---|---|---|
  | `authz:1#1#f1` | 110 / 293 | 107 / 107 (sample capped at 1,000) |
  | `authz:1#1#f2` | 181 / 247 | 191 / 255 |
  | `authz:1#1#f0` (leader) | 23 / 112 | 9 / 246 |

  The `names` shard of `Placement(install)` (shard 24) and its p90 read 5 / 50 on m3fr17. It did not come back in m3fe2's sample: Workers Logs timed out on the wider query.
- **Where the 500 to 600 ms goes in a cold isolate:** two reads in a row (`Placement`, then `Authz`). Each is a `read_spread` to the group's followers, f1 (weur) and f2 (apac) for a leader at f0 (enam), the only member with a median near 10 ms. Each costs its member's read-index round to the enam leader plus the hop: about 110 to 250 ms at the member alone. That puts two reads at about 400 to 500 ms before decode [inference from the member times above; the edge does not time the two reads apart].
- **Decode is not it:** the push's whole edge CPU was p50 27 to 50 ms by colo (DFW 27, SJC 50, LAX 75), against the 600 ms install wall.
- **Batching adds the rest in warm isolates.** By `rcwait`, install p50 was 504 ms at under 100 ms of wait, 522 at 100 to 500, 620 at 500 to 1,000, and 709 over 1 s. A push that shared the next read waits up to one read in flight plus its own (DECISIONS, "`edge-read-batch`"), and that wait lands in `install`. Pushes with no wait in an isolate that had already read (18) had install p50 377 ms, at m3fr17's level.
- **So the gap is cold isolates plus batch waits. No `Authz` member read is slower than on m3fr17,** and the `names` shard was not compared. In m3fe2, 496 of 498 burst pushes ran in an isolate that had served no warm-up push. m3fr17's install p10 was 222 ms against m3fe2's 379, which fits more of its pushes finding a warm isolate [inference: m3fr17's isolates were not read, because its push_log query timed out].

**No fix proposed.** The front still met its 775 ms target. Making install wait less in a cold isolate would mean starting the `Placement` and `Authz` reads together, which needs the `Placement` record to address the `Authz` group, or reading the `Authz` group's learners first. The first changes the read path's order and the second changes `read_spread`'s order for every chain. Neither pays for 184 ms at the median while the line passes. If a later B0-32 fails on the front, the step is to time `placement` and `authz_read` apart in `server-timing` (`instpl`, `instaz`) and decide from that.

## Rows m3fb32: B0-32 on main `76c3e131` (2026-11-19, graft-m3f, `ROWS="b0-32"`)

**Slot.** Granted by the orchestrator after m3flc: one site day from main `76c3e131` or later (`tier-off-leader`, F91, both write cuts), then B0-32. Stops: graft-m3f at $10 from the site day's start, or the account at $170. Committed raw files: `tests/fanout/raw/rows-m3fb32.status`, `tests/fanout/raw/{terms,admit,vmem}-m3fb32-b0-32.jsonl`, `tests/fanout/raw/tsave-m3fb32-b0-32.jsonl.xz`. The a1 files (`a1-b0-32-mwmura9l.jsonl`, `a1-pushes-b0-32-mwmura9l.jsonl`, `runs/{siteday-m3f-m3fb32,a1-b0-32-fm3fb32-b032}.log`) are under the ignored `tests/am1/raw/` on this thread's orb, as on earlier rounds.

- **Site day** 00:38:18Z to 00:50:59Z, `site exit 0`, `edge exit 0`. Image `sha256:1e6cc1d0…` reused (`containers/` unchanged since `7b576074`), standard-4, test-faults. Sites `ce49f4f5` / `81350e53` / `dfa4c0f7`, edge `c65f55bf` at 100% (edgever, rev `76c3e131`).
- **Setup:** repo 65590, config 11 with 8 learners, k 4 at index 12, leader site 0 at term 2. The burst left at 01:15:42Z, after a 61 s warm-up.
- **Client colo:** DFW from `/cdn-cgi/trace` at 00:38Z and 01:36Z. The edge's Workers Logs put the burst's receive-packs in DFW (272) and LAX (151); m3fr17's were ATL 267, LAX 175, EWR 129 and DFW 28.
- **Spend:** $0.65 on graft-m3f by `PREFIX` from 00:38:18Z to 01:37Z (stop $10); the account was $8.86 for the UTC day.

### Result: fail [measured]

**Judged on the edge clock (orchestrator, 2026-11-19).** The client clock includes the orb's connection setup: about 4 s before the edge's clock starts, spent opening 500 new HTTP/1.1 connections from one orb. A production fleet keeps its connections alive, so that time is not the product's. From this run on, the 5 s line is the edge's `server-timing` `total`. The client figures are kept for comparison with m3fr8, m3fr11 and m3fr17.

| Gate line | m3fb32 | m3fr17 |
|---|---|---|
| Every push acknowledged within 5 s, or `ng … agent admission full` | **Fail.** 310 `ok`, 190 `full`, 0 lost. On the edge clock 80 of 310 `ok` within 5 s; edge total p50 6,431 ms, p99 7,898 ms. Client clock: 0 of 500, `ok` from 7.4 s, p50 10.4 s, p99 13.5 s | 474 `ok`, 20 `full`, 6 lost before the edge. Edge clock: 167 of 474 within 5 s, p50 5.5 s |
| No other refusal shape | Pass: 0 burst 503s. Other traffic: 113 `read:api` 503s, 6 on ref reads | 0 on the burst |
| Queued agent bytes under `AGENT_QUEUE_MAX` (64 MiB) | Pass: peak 111,706 B, 5 queued in `/stats`; the leader's own admit trace peaked at 138 queued, 216,345 B | Pass |
| Every merge in one of the next 2 entries, none delayed over 2 s | **Fail.** 22 of 22 `merge:only` returned 200, but 3 took over 2 s on the client clock and 2 on the edge clock (edge max 6,920 ms: `merge_plan` 4,468, `shard:waited` 2,823). Not judged from entries | 19 × 200, 1 × 409, 1 × 503 warming |
| Zero leader changes | Pass: term 2 throughout, leader site 0 (20 `terms` samples) | 1 change, term 2 → 3 |
| Zero `suspect` | **Fail.** 34 from the edge (`pushTiming`), 32 at each of sites 1 and 2 (`tsave` `Reported`) | 5 edge, 18 + 18 `tsave` |
| No object reset from memory | Pass: one boot per voter in `vmem`, wasm high 34.7 MB | Pass |

Leader commit p50 568 ms, p99 1,660 ms (n 3,757), against m3fr17's 459 and 3,514. Queue p99 1,000 ms, round p99 444 ms, confirm p99 249 ms.

Against earlier runs: m3fr11 had 499 `ok`, 1 within 5 s by the client, and 103 `suspect`. m3fr8 had 498 `ok`, 0 within 5 s, and a client p50 of 13.5 s. This run is better on leader commit p99 and on leader stability. It is worse on the two numbers the line counts: `full` went from 20 to 190, and edge-clock `ok` within 5 s from 167 to 80.

### Where the edge's 6.4 s goes

Each push's `server-timing` stages, burst pushes only, in ms. auth plus install run alongside resolve, so the wall before the first send is max(auth + install, resolve) + gate + context + validate.

| Stage | m3fb32 p50 / p99 | m3fr17 p50 / p99 |
|---|---|---|
| auth | **591** / 754 | 170 / 2,578 |
| install | **905** / 1,204 | 416 / 959 |
| resolve | **1,390** / 2,051 | 594 / 2,432 |
| gate | 0 / 2,292 | 0 / 1,998 |
| context | 330 / 1,163 | 366 / 2,135 |
| validate | 768 / 1,495 | 578 / 1,333 |
| front: max(auth + install, resolve) | **1,563** / 2,051 | 775 / 3,338 |
| wall before the first send | **2,933** / 4,264 | 2,114 / 5,289 |
| after the first send (commit, stage, ask) | 3,000 / 5,726 | 3,120 / 4,882 |
| total | 6,431 / 7,898 | 5,538 / 7,443 |

- **What grew:** the front. auth went from 170 to 591 ms at p50, install from 416 to 905, and resolve from 594 to 1,390. The other stages moved less than 200 ms, and the time after the first send stayed flat (3.0 s against 3.1 s).
- **Not cold caches:** in burst order, the first 100 pushes had auth p50 142 and resolve p50 551 ms. Pushes 100 to 500 sat at auth 585 to 652 and resolve 1,407 to 1,488. m3fr17's quintiles stayed at 113 to 210 and 504 to 674.
- **Not the members:** wall time of `/read` on the system groups these stages read was the same on both runs (Workers Logs, sampled):

  | Member | m3fb32 median / p99 | m3fr17 median / p99 |
  |---|---|---|
  | `names:152#1#f0` | 3 / 119 | 4 / 131 |
  | `tokens:0#1#f0` | 3 / 238 | similar ids in m3fr17 read 3 to 4 / 246 |
  | `authz:7100001` f1 / f2 | 104 / 309 and 259 / 379 | 100 / 229 and 253 / 406 |

  Member-side `/read` p90 fell from 488 to 242 ms, and `/commit` median from 1,293 to 714 ms.
- **Where the time went instead:** each of these stages is a fresh read from an edge isolate (`authenticate`, `install_authz`, `resolve`: `account_of`, `placement`, `authz_get`). The member answers in single-digit milliseconds, so the extra 0.4 to 0.8 s per stage is spent in the edge isolate before or after the call. The edge's CPU says where: the burst ran in two colos, not four. m3fr17 spread its 599 receive-packs over ATL, LAX, EWR and DFW. m3fb32 put 272 in DFW and 151 in LAX, with LAX at a median 80 ms CPU per push and 18.0 s of CPU in the 25 s window. Fewer edge isolates served the same 500 pushes, and every push in an isolate waits behind the others' CPU (sub-requests are I/O, the stages' JSON and token checks are not) [inference: Workers Logs gives CPU per colo, not per isolate; the stage split and the flat member times are measured].
- **Leader-side:** nothing in this window ties the grown stages to `tier-off-leader`. Those reads (`names`, `tokens`, `authz`) are not whole-tier dumps or `ls-refs`, so `leader_never_serves` is false for them and their path is unchanged (`crates/lookup/src/lib.rs`, `leader_never_serves`).

### Why `full` grew from 20 to 190

- **The gate that answered:** the leader's own `AGENT_PUSH_BOUND`, after the command was admitted. 188 of the 190 `full` had a last commit stage of 600 ms or more: queued, then answered `Full` when their `until` passed (`expire_agents`, `crates/raft/src/member/leader.rs`). Only 2 were refused at admission, and 0 at the edge's `bound` (`bound_at_send`). 127 of them carried an `ask`: the send stayed open past `COMMIT_RECEIPT` (2 s), the edge asked `/outcome_by_request`, and the answer was the `Full` outcome. 93 had staged first (Stage, then a second send).
- **Why they ran out of budget:** a push's budget at its send is 5 s minus the edge's front. With the front up 0.8 s, the budget left at the last send fell to a p50 of 1,089 ms for the `full` pushes (54 had 0 left) and 2,219 ms for the `ok` ones. By how much was left at the first send:

  | Budget left at the first send | m3fb32 ok / full | m3fr17 ok / full |
  |---|---|---|
  | under 1 s | 0 / 26 | 6 / 7 |
  | 1 to 2 s | 69 / **131** | 68 / 7 |
  | 2 to 3 s | 157 / 33 | 180 / 6 |
  | 3 to 5 s | 84 / 0 | 220 / 0 |

  Every push with 3 s or more left committed. Below 2 s, two thirds were refused.
- **Admission capacity did not shrink to cause it.** The leader cut every batch `Empty`, taking every queued command, on every cut from 01:15:50 to 01:16:15Z (57 of 57 Cut rows, `CutEnd::Empty`, 0 left in any class). No cut stopped on Soft, EntryMax or AgentTokens. The admit trace's capacity sat at 452 to 770 commands/s during the burst (m3fr17: 663 to 912), with cmd_bytes 444 to 1,246.
- **What held the queued commands instead:** the burst reached the leader late and all at once, and then the leader stalled once. All times are on Cloudflare's clock, which a1's push log and `tsave` share; the orb's clock ran 9.3 s behind it.
  - **01:15:52.7Z:** the edge took the burst, 237 agent pushes in 2 s (`a1-pushes`).
  - **01:15:57.5 to 01:16:00.5Z:** the commands reached the leader, about 3 s later, which is the front above. Cuts grew from 7 to 27 commands, then to 94 and 160 at 01:16:00.2 and 01:16:00.5Z, with 591 requests open at entry.
  - **01:16:01.2 to 01:16:04.5Z, no cut for 3.3 s:** entry 322, the batch after the 160-command entry, took **2,618 ms** in its storage confirm (`Trace::Confirm`, 01:16:03.959Z), and the next flush carried 66 statements. It was the run's only confirm over 1 s. m3fr17's burst had two, 1,901 and 2,032 ms.
  - Pushes that arrived with 1 to 2 s left and queued behind that confirm passed their `until` there.
- **Neither `tier-off-leader` nor the write cuts slowed how fast admission frees room.**
  - Reads at the leader went down: site a's sampled `/read` count was 208 in the 20 s window, against m3fr17's 263, and the followers took 6 and 150. notLeader answers were not counted separately.
  - Round p99 was 444 ms, against m3fr17's 485.
  - The write cuts lowered `/commit` handler time, median 1,293 to 714 ms.
  - What changed was arrival: the pushes reached `commit` 0.8 s later at the median and more bunched. The one storage stall then landed on pushes with no budget left.
- **The 34 `suspect`:** from `missed` receipts on sends that stayed open past `COMMIT_RECEIPT` with no 200 to the ask, from 01:16:00 to 01:16:04Z: the 591 open requests, then the 2.6 s confirm. Same mechanism as m3fr11's 103, a third as many.

### Proposed fix (code-only, for review)

Two separate gaps; the first explains most of the line.

1. **The edge's front costs 1.6 s at the median under the burst, for reads whose answers do not change within a burst.** The token, the install `Authz` and the owner's `Names`/`Placement`/`Authz` rows are the same for all 500 pushes. A push is write-class, so `rcache::through` reads fresh every time (`workers/edge/src/rcache.rs`), and each read is a full lookup and JSON decode in a shared, CPU-bound isolate. Proposal: **batched fresh reads per key per isolate for write-class requests**, the way group commit batches commands.
   - A write-class `through` call with no read of its key in flight starts one.
   - Calls that arrive while that read is in flight do not take its answer, because it started before they arrived. They wait for the next read, which starts once the in-flight one ends, and they all share it.
   - That keeps "a write reads fresh": every caller's answer comes from a read that started after it arrived. Reads per key per isolate drop from one per push to at most two in flight.

   Then measure the front on the next B0-32. The target is m3fr17's 775 ms or lower; by the table above, that moves 131 + 33 `full` pushes into the 2 to 3 s bucket, which committed.

   Test: of N write-class calls arriving during one read, none gets that read's answer, all get the next read's, and exactly two reads run. A read-class call still uses a cached entry within `READ_CACHE`. Nothing in crates/raft.

   This rests on an inference: the cost is isolate CPU and queueing per read, not the member. The flat member times and the two-colo spread support it. The stage split after the change will show whether it was right.
2. **One storage confirm of 2.6 s after a 160-command entry.** `admission_check` admitted these pushes because `expected_wait` (the queue at the agent rate plus one average round, about 280 ms) fit their budget. The 2.6 s confirm then took the budget, and 186 of 188 refusals came from `expire_agents`, after a 2 s receipt wait and an ask. This is not something to fix in admission's arithmetic, which cannot foresee one slow sync. The question is why entry 322's confirm was 2,618 ms when the run's confirm p99 was 249. m3fr17 had the same shape: two confirms around 2 s after its largest cut. That belongs with LM-1's leader-tail plan (`tests/am1/results/lm1-memory.md`; slow syncs follow the leader's own write volume, F92). I have not touched crates/raft or the host's transaction. Gap 1 comes first because it decides how many pushes reach the leader with budget to spare.

## Site day m3frs: the audit's hosted checks and K12 (2026-11-18, graft-m3f, main `7b576074`)

Site day and edge day from main `7b576074` (`refold-stream`, `audit-moved`, `b034-tails`), 17:54:52Z to 18:08:53Z, `site exit 0`, `edge exit 0` (`tests/am1/raw/runs/siteday-m3f-m3frs.log`). New shard image `sha256:1e6cc1d0…`, standard-4. Sites `ef2045c3` / `1cc81f49` / `53fd05b7` (phase 1 `aa15cfd8` / `b88c2482` / `f3850881`), edge `18a707d8`. One seeded repository, `m3org/k12-g1wc3` = `repo:65588` (leader on site 0, voters `#1#f0..2`), made by REST create plus 4 labels, then one `POST /labels` (a `DefineLabel` entry) at the start of each quiet minute. Per-object rows come from `durableObjectsPeriodicGroups` by object name and minute.

The orb's proxy answered 502 for every `*.workers.dev` host this session, and its resolver often failed on them. The drivers ran with the edge's address pinned (172.67.202.67, from public DNS) in Node, Python and curl. That was local plumbing; nothing on the install changed.

### 1. audit-moved: idle and deleted counts

- **Scheduled crons, site a (site 0).** 18:18, 18:32, 18:48, 19:03, 19:18 and 19:33Z each read `homed 53, idle 0, deleted 0, alarms 0`. Every chain sampled was a retired test chain answering 409 or `no group` (`repo:65536` to `65547`), as the idle cleanup left them. Sites b and c home no chain. So the scheduled crons could not show `idle` or `deleted`: the cursor walks 53 rows 4 at a time and did not reach 65588 in this window.
- **The same pass, run on demand for one chain** (`/bench_seed {"audit": "repo:65588"}`, the cron's own `fold_audit::run` from site 0, test-faults), [measured]:
  - 18:25:15Z, forced: starts the refold (through 18).
  - 18:25:35Z: `alarm false`, core and metadata each `compared 3, mismatches []`, `missing []`, so the chain is marked matched at 18.
  - 18:25:38Z: `{"chain": "repo:65588", "idle": true, "index": 18}`, report `idle 1`, no refold started, no lookup.
  - After `DELETE /repos/m3org/k12-g1wc3` (204) at 18:49:55Z: 18:50:02Z and 18:50:04Z each `{"deleted": true}`, report `deleted 1`, no refold.
- **The PREFIX meter over two crons.** 19:05 to 19:35Z (crons 19:18 and 19:33Z), after the close-out: graft-m3f **$0.00** by `PREFIX`; 139 rows written in total, all `Registry` and cron bookkeeping (`rowsmin.py`: 69 and 70 per 15 minutes, none on any `repo:` object).

### 2. One streamed refold's `rowsWritten` against `pages` + `saved`

Two forced refolds of `repo:65588`, each alone in its minute on the leader `#1#f0` [measured]:

| Refold | Index | Report: pages, saved, batches, entries, ms | Leader rows written in the minute | Pages + saved |
| --- | --- | --- | --- | --- |
| 18:25:12Z | 18 | 6, 11, 2, 10, 1,230 | 60 | 17 |
| 18:43:15Z | 27 | 6, 14, 2, 19, 881 | 52 | 20 |

So `rowsWritten` is 3.5 and 2.6 times `pages + saved`, which is over `refold-stream`'s flip line (twice). What the extra rows are, from the code path (`workers/site/src/load.rs`) [inference]:
- Each staged page and saved change is written once and deleted once at the end (`finish_stmts`, `AUDIT_CLEAR`): 2 × 17 = 34 and 2 × 20 = 40. `cost.py` already prices that (`2 * pages + 2 * FOLD_ROWS * tail`).
- The rest is fixed per audit: `load_job` written at start, after the prepare, and once per batch, then deleted (about 5); `load_report` (1); and the cron's `audit_seen` mark (1). `audit_page` and `audit_overlay` are `WITHOUT ROWID`, so each of their puts is one row. That leaves 60 − 34 = 26 and 52 − 40 = 12 rows. The 18:25 minute also held that run's `noop` entries 19 and 20, a new term's, each with its `log` and `raft_state` rows.

The flip line compared rows with `pages + saved`, but each of those is billed twice by design. Against `2 × (pages + saved)` the two refolds wrote 1.76 and 1.30 times as much, with the excess a fixed 12 to 26 rows per audit. That changes no cost line: at 1,824 to 10,506 refolds a month, 26 more rows each is $0.02 to $0.27. Recorded in `decisions/DECISIONS.md`, "`refold-stream`".

### 3. K12: one `apply`'s `rowsWritten`

Four `DefineLabel` entries, each alone in its minute (18:23, 18:24, 18:28, 18:42Z: entries 17, 18, 23, 27):

| Member | Rows written per entry | Statements in its transaction |
| --- | --- | --- |
| Leader `#1#f0` | 19, 19, 19, 19 | 1 confirm of 1 statement, 143 B, plus the apply's |
| Followers `#1#f1`, `#1#f2` | 13 to 15 when the commit arrives in the same minute (18:24: 15 and 15) | |

What one entry writes, counted two ways:
- **Fold rows.** In memory, a `DefineLabel` entry puts 6 fold rows: core `_digest` and `_fold`, metadata `_digest`, `_fold`, `ids` and `label` (a counting store over `RepoFold`, scratch test, deleted). That matches `FOLD_ROWS` 6.
- **Other rows per entry**, from `SqlStore::write` and the host (`workers/site/src/host.rs`): the `log` row, `raft_state`, the `request` row, `DELETE FROM request WHERE at < cut` (0 rows here), and the test-faults `dup` probe and `arrival` row (test installs only). So 3 product rows per entry, plus 2 test-only ones.

How the platform counts them [doc]: workerd's row counter (`patches/sqlite/0001-row-counts-plain.patch` in cloudflare/workerd) adds one row written for each `OP_Insert` and each `OP_IdxInsert` on a non-ephemeral cursor, and for each `OP_Delete` that carries the change-count flag. Cloudflare's D1 pricing says the same of indexes: "two rows written: one to the table itself, and one to the index". Every fold tier table, `_fold` and `_digest` included, has a `PRIMARY KEY` over untyped `k0..` columns and no `WITHOUT ROWID` (`workers/site/src/tier.rs`, `create`). Such a key is not a rowid alias, so SQLite keeps an automatic index beside the rowid table, and each put inserts into both.

Accounting for the leader's 19 [inference, from the patch and the schemas; it fits every clean minute]:
- 6 fold puts × 2 = 12.
- `log` (`idx INTEGER PRIMARY KEY`, a rowid alias) 1, and `request` (`rid TEXT PRIMARY KEY`, with an index) 2.
- `raft_state` 1 or 2: the append and the commit are separate inputs on the leader.
- The test-faults `arrival` row, whose `AUTOINCREMENT` also writes `sqlite_sequence`: 2 on the leader only, and test installs only.

So a product member writes about 17 rows for this entry, 16 on a follower whose `raft_state` write lands once. I did not settle whether `INSERT OR REPLACE`'s implicit delete of a replaced row counts. The patch counts `OP_Delete` only with the change-count flag, and the measured 19 fits it not counting.

**Answer:** yes. A tier row put costs 2 rows written, and one `apply` of this 6-fold-row entry wrote about 17 product rows, against `cost.py`'s 1 + `FOLD_ROWS` = 7 [measured: 19 on the leader with 2 test-only rows; inference: the split]. The "Durable Objects SQLite rows written" line is therefore about 2.4 times what `cost.py` prices. At 17 rows an entry (`FOLD_ROWS` 16), the line is $0, $66 and $726 a month at 50, 300 and 2,000 developers, against $0, $0 and $270, and the totals are $235, $1,282 and $9,832. Those figures come from `scripts/cost.py --detail` with `FOLD_ROWS` set by hand and not committed. `cost.py` is not changed here: K12 asked for the measurement, and the orchestrator decides the pricing and whether the tier tables move to `WITHOUT ROWID` as a new table set and refold (`decisions/OPEN-QUESTIONS.md`, K12).

### Close-out

- `retirenets.mjs 65588` (18:58Z): 3 names, 3 joined, all not joined. `purge.py 65588`: purged 1 network, 14 objects, second pass 0, none refused (18:59:47Z; the first try through `retirenets.mjs` refused on the orb's proxy 502, and the rerun had the address pinned).
- `node tests/fanout/joined.mjs 65536-65588`: none joined of 53. `joined.mjs 0` reads `repo:0 ---` for the latency probe's chain, which this session did not touch.
- `INSTALL=graft-m3f teardown.py --sweep`: the first sweep tore down `git#0#net:65588` (code `000` through the proxy, so `SWEEP FAILED`, exit 2), and the pinned rerun answered 200 and `sweep: no shard container running`, exit 0, and again at 19:03Z.
- Cost: graft-m3f $0.09 by `PREFIX` from 17:52Z to 19:03Z ($0.08 of it site a's shard container), $2.83 for the day; the account $28.26 for the day at 19:39Z.

## Site day m3flc: `live-check` and `audit-cover` hosted (2026-11-18/19, graft-m3f, main `203e86fa`)

Site day and edge day from main `203e86fa` after its CI passed, 23:29:38Z to 23:42:56Z, `site exit 0`, `edge exit 0` (`tests/am1/raw/runs/siteday-m3f-m3flc.log`). Image `sha256:1e6cc1d0…` reused (`containers` unchanged since `7b576074`), standard-4; edge version `d4d96835`. One seeded repository, `m3org/k12-rq87q` = `repo:65589` (REST create plus 4 labels at 23:45Z, leader on site 0). Drivers ran with the edge's address pinned locally, as on m3frs.

### audit-cover: **passes**

Site a's 23:48Z cron [measured, Workers Logs `fold audit` report]: `homed 54, retired 53, idle 0, deleted 0, alarms 0`. The run looked at all 53 retired chains (`repo:65536` to `65588`, each `retired`, no sample spent, no lookup) and reached the live `repo:65589` in the same run, which started its refold (index 17, 2 batches, 15 entries). Sites b and c home no chain. On main's rule the 4 samples would have gone to 4 retired chains, as on m3frs.

### live-check: **passes**

- Site a's 00:03Z cron: `homed 54, retired 53, liveStarted 3, liveAlarms 0`. `repo:65589` refold-compared at index 17, core and metadata `compared 3, mismatches []`, `missing []`, `live {"started": 3, "alarms": []}` [measured].
- Each voter's `live_last` after its scan (`/fold {"live": true}`, 00:05Z to 00:06Z) [measured]:

| Voter | Status | Core: to, rows, want = got | Metadata: to, rows, want = got | Batches | undoMax |
| --- | --- | --- | --- | --- | --- |
| `#1#f0` (site 0, leader) | done | 19, 7, `5cd89316…` matched | 19, 6, `f74b0e06…` matched | 2 | 0 |
| `#1#f1` (site 1) | done (at 00:06:08Z; running at the first read) | 10, 5, `cef0d0d1…` matched | 10, 5, `940504ba…` matched | 2 | 0 |
| `#1#f2` (site 2) | done | 17, 7, `8fe1c3e1…` matched | 17, 6, `601d9037…` matched | 2 | 0 |

- Site 2's core `want` at 17 equals the refold's core digest at index 17 (`8fe1c3e1…`), so the live rehash, the kept sum and the refold agree there.
- **Rows** [measured, `durableObjectsPeriodicGroups` by object]: each voter's check minute wrote 0 to 25 rows and read 1,194 to 2,292 (00:03Z and 00:06Z), against about 1,160 to 1,190 read in a quiet minute on the same members; a check of this 13-row chain writes only its `live_run`, mark and `live_last` rows.
- **Meter:** graft-m3f $0.05 by `PREFIX` from 23:29Z ($0.04 site a's shard container). The account read $34.46 for 2026-11-18 and $0.10 for 2026-11-19 at 00:11Z.

### Close-out

- `retirenets.mjs 65589` (00:07Z): 4 names, 4 joined, all not joined. `purge.py 65589`: purged 1 network, none refused, second pass 0 (00:10:22Z).
- `joined.mjs 65536-65589`: none joined of 54. `repo:0` is untouched.
- `INSTALL=graft-m3f teardown.py --sweep`: tore down `git#0#net:65589` (200), then `no shard container running`, exit 0, and again 45 s later (`torn_down 0`).
- **Idle reading**, 00:12 to 00:32Z (crons 00:18 and 00:33Z): graft-m3f **$0.00** by `PREFIX`; all Member objects wrote 134 rows. Each cron read `homed 54, retired 54`, no refold, no lookup.

## `live-check` built (K11, code-only, branch `live-check`, 2026-11-18)

Built as approved (`decisions/DECISIONS.md`, "`live-check`"): the fold audit's cron sends each voter it visits `/fold {"live": true}`; a voter whose last check is 7 days old, and that is quiet (not leading with commands queued, no snapshot, refold, install or slim running, metadata tier loaded), runs a snapshot job with no upload on its alarm, both tiers in turn, and compares each trailer's rehash with the sum `apply` kept (`workers/site/src/live.rs`, `SnapshotJob::live_step`). Past 50,000 `snap_undo` copies it ends as `abandoned`; a real snapshot that comes due takes the tier (`yielded`); an install ends it as `gone`.

- **Bound:** at 1,200 rows/s, `repo:65536`'s 571,889-row core tier takes 7.9 min and BF-20's 1.33 M-row tier 18.5 min.
- **Test that fails on main:** `a_row_changed_in_storage_after_apply_is_caught_by_the_live_check_alone` (`crates/fold/tests/tiers.rs`): `fold_digest`, `digest_at` and a refold all agree with the overwritten tier; the live check names it.
- **Plants:** `tests/m0carried/plant47.py`, 8 of 8 caught.
- **Cost:** $0.76, $4.31, $35.52 a month ($7, $41, $339 at the 65 B floor); totals $236, $1,284, $9,855.
- **Hosted half (next m3f site day, with `audit-cover`'s check):** a cron report with `liveStarted` > 0, one voter's `live_last` after the scan (`/fold {"live": true}` again), and its rows read; and `retired` > 0 with a live chain audited in the same run.

## Follow-ups to site day m3frs (code-only, branch `audit-cover`, 2026-11-18)

**audit-moved's cron coverage.** graft-m3f's registry holds a row for each of the 53 chains `repo:65536` to `65588`, all homed on site 0, and none is joined (`joined.mjs 65536-65588`); `repo:0` has rows on sites 0 and 2 [measured, tools edge `/registry`]. The test route `/retire` deletes a member's storage and resets it, standing for a lost member whose row must stay for its replacement, and `tests/fanout/purge.py` deletes bucket objects only. The product purge does delete each site's row (`workers/edge/src/purge.rs`, `unregister`). So the cron's coverage was lost in the cursor: every row it looked at took one of its 4 samples, and a site of mostly retired rows (or, in production, rows between a delete and its purge) never reached a live chain. Fixed in `fold_audit::walk`: a `retired` (not joined on any site its rows name, asked directly), `deleted` or `skipped` row costs a look and no sample, up to `LOOK_MAX` 64 looks a run; the report counts `retired` (`decisions/DECISIONS.md`, "`audit-cover`"). Test `a_run_over_mostly_retired_rows_still_audits_the_live_ones`, which fails with every look counted as a sample. Hosted check at the next m3f site day: one cron report with `retired` > 0 and an `idle` or refolded live chain in the same run.

**K12 re-priced.** `scripts/cost.py` now has `ENTRY_ROWS` 17 for apply (each fold put 2, `log` 1, `request` 2, `raft_state` 1 to 2) and `FOLD_PUTS` 6 for the audit's `WITHOUT ROWID` overlay, plus `AUDIT_FIXED_ROWS` 26 per audit. Rows written: $0, $66, $726 a month; totals $235, $1,280, $9,819; agent layer $21, $330, $2,947. The audit line is $0.01, $2.26, $9.29. `WITHOUT ROWID` tier tables deferred (`decisions/DECISIONS.md`, "`k12-rows`"); K12 closed.

## Idle cleanup and the fold audit's cost (2026-11-18, graft-m3f, no deploy)

**What billed.** With no container running, graft-m3f billed $0.15 from 03:30 to 04:00Z and $0.37 from 04:00 to 04:33Z, nearly all Durable Object rows written, led by `graft-m3f-site-a`. The writer was site a's `*/15` fold audit (`workers/site/src/fold_audit.rs`): it homes 52 repository chains (sites b and c home none), samples 4 a run after its `fold_audit` cursor, and the leader of each starts a refold when its last report is a day old (`AUDIT_EVERY_MS`). Workers Logs' `fold audit` reports name the chains of each burst: 03:32Z `repo:65561` to `65564`, 03:48Z `65565` to `65568`, 04:03Z `65569` to `65572`, 04:17Z `65573` to `65576`. From 03:30 to 04:20Z those 16 chains wrote 501,211 rows, about $0.50 [measured, `durableObjectsPeriodicGroups` by object].

**Why they were joined.** `repo:65536` to `65576` came from runs of 2026-11-13 to 11-16: the j4 seed, lf13, ld-before/after, web38, B0-29's rows and m3fr1 to m3fr9's rows. Each run's "torn down" in `install-m3f.md` meant shard containers. `65577` to `65587` had already been retired.

**Retired and purged.** `retirenets.mjs 65537-65576` retired every member of the 40 chains from 04:44 to 05:05Z, each "12 names, 12 were joined, all not joined" (some log lines say STILL JOINED because `/status` timed out; a direct `/status` read "not joined" on 65537, 65547 and 65550). `purge.py` then purged the 40 networks: 148,186 objects, second pass 62, none refused (05:48:44Z). `repo:65536`, m3org/j4, is used only by `tests/m3/hosted` and `tests/am1/hosted/feed.mjs`, which default to graft-m1's edge, so it is not a standing m3f fixture: retired at 05:05Z, purged at 05:40Z, 17 objects. `repo:0`, the latency probe, stays. Logs: `tests/fanout/raw/retirenets-m3f-idle.log` and `.out`, `tests/fanout/raw/purge-m3f-idle.log`.

**Close-out.** `node tests/fanout/joined.mjs 65536-65587`: none joined of 52. `teardown.py --sweep`: no shard container running. Idle 05:30 to 06:00Z, spanning the 05:45 and 06:00Z crons: graft-m3f $0.00 by `PREFIX` ($0.01 from 04:40 to 05:40Z); `idlerows.py` lists only the `Registry` objects' `sweep`, `fold_audit` and `faults` rows. The audit now answers 409 or "no group" for each retired chain at no refold cost. The registry rows stay, so `homed` still reads 52; `joined.mjs` is the check (`tests/fanout/README.md`, close-out step 4).

**Why every member wrote 9,000 to 10,000 rows.** The refold runs on the leader alone: `audit_route` (`workers/site/src/load.rs`) answers `notLeader` to `start` on any other member. In `repo:65568`'s 03:47 to 03:48Z burst the leader `#1#f0` wrote 14 rows and its 2 followers and 8 learners 9,053 to 9,064 each. Those are each member's expiry of the run's `request` rows, not audit work: a member applying an entry deletes every `request` row older than REQUEST_RETENTION (48 h) before it (`DELETE FROM request WHERE at < cut`, `workers/site/src/host.rs`; `GroupFold::apply`, `crates/raft/src/fold.rs`), and a delete bills as a row written. On an idle chain the first entry past 48 h is the no-op of a term that the audit's own lookup starts (`graftyard_lookup::call` campaigns a leaderless group). The followers apply it at the next visit, when the next append carries its commit. For 65568, from B0-31's m3fr6 at 2026-11-15 21:17Z: the leader wrote 33 rows at the 11-17 21:18Z visit, 9,054 at 00:48Z, and the 10 others 9,053 to 9,064 each at 03:48Z [inference from the per-object rows and the visit times; the members are retired, so the `request` row count was not read]. So the expiry is a one-time cost per run's requests, paid on every member, and a product chain pays it too. What the audit adds every day on an idle chain is the election: a new term's no-op on every member (3 to 43 rows each a visit), and a commit index that climbs with no write (`65548` 2,922 to 2,942 over 11-17).

**Rows per refold (leader only).** `repo:65563` 14,846 (2.6 MB read), `65569` 18,622, `65570` 34,229 (9.5 MB, 1,287 entries), `65566` 34,943 (8.2 MB, 2,442 entries), `65571` 118,523 (106.5 MB, 4,696 entries): 14 to 27 rows per entry refolded, $0.015 to $0.12 per chain per refold. A chain whose report was under a day old wrote 14. `scripts/cost.py` has no audit line (`decisions/OPEN-QUESTIONS.md`, K10).

**The mismatch alarms.** 41 "fold audit mismatch" lines on site a since 2026-11-17 10:18Z, every one on the core tier, with all three voters agreeing with each other against the refold. Chains that matched before now mismatch from the same snapshot: 65565 matched at 1034 and mismatched at 1098, 65567 at 3077 and 3769, 65548 at 2922 and 2942. Chains refolded only from entries before 11-17 still matched (65569, 65570, 65571). So the refold's replay of entries appended since about 11-17 differs from the members' live apply [inference]. `graft-m1` site a has 10 such lines since 11-14, `graft-m2b` site a 1. Recorded as F88; not fixed.

### Design: the audit refolds only a chain that moved (code-only, for review before any build)

**Change.** In `audit_route` (`workers/site/src/load.rs`), a leader starts a refold only when the chain has committed an entry with events since the last report's `index`, or with `force`. Otherwise the report is kept as it is, and `fold_audit::audit_chain` compares each voter's `digest_at(index)` against that report's digests at the same index, as it does now for a fresh report. Second, a chain whose fold is tombstoned (`purge::purgeable`, already in each member's `/status`) gets no refold and no comparison until its purge; the audit lists it as `deleted`. Third, the audit's reads do not start an election: `audit_chain` reads the leader through `graftyard_lookup::call` only after its own site's member answers with a known leader, and a leaderless chain whose last report is current is listed `idle`, not campaigned.

- "Moved" is the last committed index with an `Events` body, which each tier's meta does not keep today. The leader keeps it beside `load_report` in the member's own storage (one integer, written in the same transaction as the entry it follows). A no-op or a config entry does not move it, so the audit's own elections and a learner change do not refold.
- The voters' digests are compared at the report's index while it is within `DIGESTS_KEEP` (20,000) of their `applied`. Past that, `digest_at` answers none and the member is listed as missing, so an idle chain whose log moved 20,000 no-op entries would be refolded anyway [inference: an idle chain adds about 1 to 4 entries a day].

**What it keeps from F48.** F48's purpose is that `apply` is checked against a second, independent replay of the log, and that a member's stored tier rows are checked against the digest `apply` kept. An idle chain's entries were already replayed at its last refold, and none came since, so a new refold replays the same entries from the same snapshot and can only find what the last one found. What a refold of an idle chain does catch, and the digest comparison does not, is a member's tier rows changing in storage after `apply` (a bad write or storage corruption), because `digest_at` reads the digest `apply` kept, not the rows. But today's refold does not catch it either: it rebuilds into the leader's scratch tables from the snapshot and the log, never reading any member's live tier rows, and compares against each voter's kept digest. So skipping it loses no detection the audit has today [inference from `apply_one`, `finish_audit` and `tiers_json`]. Catching storage corruption needs a separate check, `refold_digest` over a member's live rows (`crates/fold/src/tier.rs`), which `refold_page` already runs on demand; I propose it weekly per member as a follow-up, not in this change.

**Cost.** An idle chain then costs one `/fold {"audit": {}}` read on its leader and one `/fold {"at": n}` read per voter a day: no row written. A tombstoned chain costs nothing until its purge, instead of a daily refold for 90 days. The first refold after a write still costs the 14 to 27 rows per entry above; the daily bound of 4 chains per site per run stays.

**Test that fails on main.** In `load.rs`'s tests, a member with a report at index n and no events since: `audit_route({"start": true})` a day later answers `started: false` and keeps the report; after one more `Events` entry it answers `started: true`; after a no-op entry only, `started: false`. On main the first case starts a job. And in `fold_audit.rs`, a tombstoned chain gives `deleted` and no `/fold` call to its leader.

**Decision record.** A DECISIONS entry "`audit-moved`" with decision / why / beat (a refold every day per chain, F48's cadence; sampling rows instead) / evidence (this section and K10) / flip: a storage-corruption case found on an idle chain by the follow-up live-row check that a daily refold would have found first, or a refold of an unmoved chain that disagrees with its last report.

## `refold-stream` built as A′ (2026-11-18, code-only, branch `refold-stream`)

Built as amended after review: (A) is dropped and A′ was approved (`decisions/DECISIONS.md`, "`refold-stream`").
- **Why not (A).** (A) answered a page not yet in memory as "absent" and re-applied the entry once the page arrived. Fold code may assert on a row it is entitled to expect: `CommentEvent::Resolved` reads its thread (present) and then the unresolved count (absent on a miss), and `count_unresolved` panics. On wasm a panic resets every member in the isolate.
- **A′.**
  - The snapshot is checked against its trailer by `Restore` and staged by `Pager` into 256 KB pages, one row each in `audit_page ... WITHOUT ROWID`.
  - Entries apply through `SnapView`: the batch's changes, then `audit_overlay` (earlier batches' changes), then the page, through 8 cached pages. Every read is synchronous.
  - Changes are saved with the batch's cursor. The audit's end deletes both tables in its report's transaction. A restore with no audit running deletes them, and so do a failed or abandoned audit.
  - Staging is refused past half the member's room below `SPLIT_AT`; that replaces `AUDIT_SNAPSHOT_MAX`.
  - Install is unchanged. The audit's scratch path is deleted, and so are `TierLoad.skip`, `AUDIT_SNAPSHOT_MAX` and `AUDIT_ROWS_PER_ROW`.
- **What A′ replaces in (A)'s conditions.** Condition 1 (miss and re-apply): there are no misses. A batch's changes reach `audit_overlay` only in the batch transaction, after every entry in it applied. Condition 2 (128-frame cap): nothing to cap, so no `incomplete` count. At 256 KB pages the most pages one entry of the fixtures read is 4 per tier (73 at 300 B pages). Conditions 3 to 5 stand.
- **Proof.**
  - The corpus folds all 902 rows through staged pages to the golden digests.
  - At every index, the staged refold and a restore give the same `fold_digest`, `refold_digest` and rows: on `repo:65536`'s shape scaled down, the split parent from four snapshots, and the 552-comment slim, with a cache of one page.
  - `key_bytes` order is property-tested against `Key` order over integers across the sign boundary, empty and prefix strings and blobs, the escape byte, and composite keys. The same SQL is tested in SQLite (`crates/fold/tests/stream_sqlite.rs`).
  - The staged-row clean-up is tested at the end and on restore.
  - `tests/m0carried/plant44.py`: 9 of 9 caught.
- **Cost** (`scripts/cost.py`, rows written and read, R2 Class B, duration): the line is $0.01, $1.48 and $9.03 a month (was $421, $2,517 and $3,110), and $0.10, $8.02 and $17.10 at the 65 B floor. Default totals are $235, $1,211 and $9,362; `Solo` $132, $822 and $5,558; agents add $21, $278 and $2,599. Cadence stays daily.
- **K12 opened.** Do the live tier tables (composite key, no `WITHOUT ROWID`) write 2 rows per put? To be settled by measurement.
- **Hosted, at the next m3f site day from a main with this merged.**
  - Read the cron report's `idle` and `deleted` counts and the m3f PREFIX meter over two crons (audit-moved).
  - Read one refold's `rowsWritten` against its report's `pages` and `saved` (refold-stream).
  - Read `rowsWritten` for one `apply` on a seeded repository (K12).

## Design: the fold audit's refold writes no scratch rows (2026-11-18, code-only, for review before any build)

**Why.** At `ROW_KB` 0.62 the refold line is $421, $2,517 and $3,110 a month, 64% and 68% of the bill at 50 and 300 developers (K10). The writes come from restoring each tier's snapshot into scratch tables: 2 rows written per tier row. That is likely because each table has a composite `PRIMARY KEY` without `WITHOUT ROWID`, so SQLite writes the row and its autoindex [inference, from `tier.rs` `create` and Lumenize's DO write-cost measurements (https://lumenize.com/blog/do-sqlite-write-costs) [doc, secondary]].

**The crux: `apply` reads.** Every write reads the old row first (`Writer::before_change`, to keep the row-hash sum and the `snap_undo` copy). Every entry also scans an expiry head: `lease_expiry` on core, `check_expiry` on metadata (`on_entry`). Some events read other rows (`ref`, `review_verdict`, `queue`, `split`, ...). So a refold has to answer `get`, `scan` and `written` over the tier as of each entry.

**Three options, priced.**
- **(A) Read-through over a frame index, plus an overlay of the tail's changes.** Recommended; detailed below. Priced per month (cost.py's workload, `AUDIT_MOVED_SHARE` 0.3, the overlay persisted): $0.27, $1.58 and $9.68 at 50, 300 and 2,000 developers, against $421, $2,517 and $3,110 today. At the 65 B floor: $0.48, $2.74 and $11.11, against $4,372, $24,010 and $29,660. The line falls to 0.10 to 0.19% of the total.
- **(B) Stream only the tiers or kinds that never read during apply.** None qualify. Every entry runs `on_entry`'s expiry scan in both tiers, and every write reads its old row for the sum. (B) would cover 0% of entries, so I rejected it. Priced: no change from today.
- **(C) Keep scratch tables but avoid billed writes.** Not available. workerd's authorizer refuses `CREATE TEMP TABLE` ("TODO(someday): Allow temporary tables", `src/workerd/util/sqlite.c++`, `SQLITE_CREATE_TEMP_TABLE`) [doc, source]. `ATTACH` is refused too. Cloudflare's docs bill every row written, including index rows and virtual tables (SQLite-backed Durable Object Storage, SQL API) [doc]. The only unbilled writes are SQLite's own ephemeral tables inside a `SELECT` (workerd commit 42265416, "don't count rows written to temporary (ephemeral) tables") [doc]. What remains is making the scratch tables `WITHOUT ROWID`, about half the rows: $210, $1,258 and $1,555, which is 47%, 51% and 14% of the total. Still far above 10%, so I rejected it as the fix.

### (A) in detail

1. **Verify the snapshot as a stream.**
   - Read each tier's snapshot object in 1 MiB range reads (as today), check frames and trailer with `FrameCheck` (as `Restore` does), and write no row.
   - While reading, build a **frame index**: for each `Rows` frame, its `(table, first key, last key)` and its byte range in the object.
   - A frame holds at most `ROWS_PER_READ` (500) rows, so 88 MiB of frames is about 2,800 index entries of about 100 B: 280 KB, kept in the job row.
2. **Apply the tail through a new `Store`: `SnapView`.** It lives in `crates/fold` beside `MemStore`; `crates/raft` is untouched.
   - `get(t, k)`: the overlay first, then the one frame whose range holds `k`. That frame comes from a bounded in-memory cache of decoded frames, or from the snapshot object on a miss.
   - `scan(t, from, n)`: a merge of the overlay's keys and frame rows from `from`, overlay winning and overlay deletes hiding frame rows.
   - `put`/`delete`: into the overlay.
   - `apply` itself is unchanged: the same `Writer`, `before_change`, sum and `snap_undo` rules. So the digest is today's by construction: the snapshot's sum (checked against its trailer) plus each entry's subtract-old/add-new, which is exactly what scratch tables give.
3. **Reads are synchronous; R2 is not.** `Store` cannot await, so `SnapView` answers a miss on an unloaded frame by recording it and returning nothing.
   - The job applies each entry to a copy of the overlay. If any miss was recorded, it drops the copy, fetches those frames, and applies the entry again.
   - Each retry loads at least one new frame and an entry touches a bounded set of keys, so this ends.
   - A miss is never mistaken for a real absence, because only a run with no misses is kept.
   - The frames an entry needs are pinned in the cache until the entry applies, so eviction cannot undo a fetch and loop. An entry whose pinned frames would pass the cache cap (more than about 128 frames of 32 KB) fails the audit with its index and frame count. It is reported and retried at the next due run, not looped. Today's events touch a handful of keys per entry, and an expiry scan reads 64 or 256 rows, so this is a guard, not a path [inference].
4. **Overlay and memory bound.**
   - The overlay is the tail's changed rows only. It is persisted at each batch boundary to one member table, `audit_overlay(tier, tab, key, written, v) WITHOUT ROWID`, so a reset object resumes, as the job's cursor does today. It is read back on a cache miss.
   - Memory holds: the frame cache, capped at 4 MiB [guess]; one batch's entries, at most `BATCH_BYTES` 1 MiB as today; the frame index, ≤ 0.3 MB; and the in-memory overlay pages, capped at 4 MiB, beyond which they spill to the table.
   - About 10 MiB in all, against today's measured refold peak of 64.9 MB with the isolate at 56.6 MB before it (`tests/m3/results/m3-wp5.md`, F48). It also stays below memguard's 120 MiB stop line and the 31 to 96 MiB of wasm memory at which LM-1 saw isolates reset (`tests/am1/results/lm1-memory.md`, round 17).
   - So the bound is structural: a tail that rewrites a million rows writes about a million overlay rows (priced), never a million rows in memory.
5. **What it writes.** Only overlay rows (≤ the tail's changed rows, about `FOLD_ROWS` 6 per entry), one job row per batch, and the report. For `repo:65571`'s 4,696-entry refold, that is about 28k overlay rows instead of 118,523. For a chain with a short tail (the common case, about 9 to 63 entries a visit in cost.py's workload), it is tens of rows instead of 2 per snapshot row.
6. **`deep` and `AUDIT_SNAPSHOT_MAX`.**
   - A deep audit streams the oldest snapshot and checks each later trailer the same way: at the later snapshot's index, `fold_digest` of `SnapView` must equal that trailer's.
   - `AUDIT_SNAPSHOT_MAX` exists because a scratch copy would double member storage. With no copy, it goes, and BF-20's 3.99 GB tier (2.4 GB snapshot) becomes auditable: about 2,400 range reads ($0.0009) and about 9 minutes of batches at m3-wp5's 4.7 MB/s.

**Deletion.** The audit's use of scratch tables goes: `Kind::Audit`'s branch in `batch`, `finish_audit`'s scratch digest, the audit side of `reset_scratch`, the `skip` field and `AUDIT_SNAPSHOT_MAX`.
- The install path keeps `Restore` into scratch tables, because an install's rows become the member's real tiers (`Write::Install` swaps them in). Only the audit stops writing them.
- So the old refold path is deleted, not kept beside the new one. The shared code that stays (archive reads, entry decode, the job cursor) is the install's.

**Proof it gives today's digest.**
1. **Golden corpus** (`crates/git/tests/fold_corpus.rs`, 48 kinds, 902 rows): each fixture also folds through `SnapView` over a snapshot of the empty or seeded tier, and must give the recorded digest.
2. **Refold-vs-stream equality** in `crates/git/tests`. At every index of each fixture's tail, `fold_digest` and `refold_digest` agree between the two paths (`Restore` into a `MemStore` plus `apply`, against `SnapView` plus `apply`), and so do the rows a full scan returns:
   - **`repo:65536`'s shape:** a core tier of about 570k ref, lease and check rows at a scaled-down count, with a tail that renews and expires leases, moves refs and promotes.
   - **A split parent:** `split.rs`'s `Parent` through seed, mid-pass and end.
   - **A slimmed chain:** the 552-comment `SLIM_BYTES` pass.
3. **The retry rule:** a frame cache of size 1 forces a miss on every read, and the digest must not change.
4. **Plants** (`tests/m0carried/plant41.py`), each to be caught:
   - The overlay's delete not hiding a frame row.
   - A frame lookup off by one frame.
   - `scan` returning frame rows past the overlay's.
   - A miss kept as an absence.
5. **Hosted, at the next site day:** an m3f site day with this build refolds a moved chain, and its report matches all three voters, as today's does. `rowsWritten` per refold is read from Workers Logs.

**Re-price.** `scripts/cost.py`'s "Fold audit refolds" line is recomputed from rows written (overlay plus job rows), rows read, R2 Class B range reads, and member duration at m3-wp5's 4.7 MB/s:

| | 50 | 300 | 2,000 |
| --- | --- | --- | --- |
| Line today (620 B rows) | $421 | $2,517 | $3,110 |
| Line streamed (620 B) | $0.27 | $1.58 | $9.68 |
| Line streamed (65 B floor) | $0.48 | $2.74 | $11.11 |
| Default total streamed (620 B) | $235 | $1,211 | $9,363 |

**Cadence.** Weekly instead of daily for moved chains would save cents ($0.24, $1.45 and $9.51 weekly, against the daily figures above), and the streamed line is at most 0.19% of the total. That is far below the 10% trigger, so the cadence stays daily.

**Side finding, not in this change.** The member's own tier tables have the same shape (composite key, no `WITHOUT ROWID`), so `apply`'s live writes may also cost 2 rows per row [inference]. A `rowsWritten` reading on one hosted `apply` would settle it. A schema change is a new table set and a refold (SHARED, "Schema change is a refold, never a migration"), so it gets its own OPEN-QUESTIONS row, not this change.

## `audit-moved` built (2026-11-18, code-only, branch `audit-moved`)

Built as designed in "Design: the audit refolds only a chain that moved" below, with the orchestrator's gate: only a refold that every voter answered and matched opens the idle path, and a chain whose last refold alarmed refolds until one matches (`decisions/DECISIONS.md`, "`audit-moved`").

- **Where the match lives.** On the cron's own site member, not the leader: `audit_seen` (index, core and metadata digests, `matched`), written by `fold_audit` through `/fold {"audit": {"seen": ..}}` after each refold comparison. The leader is reached only through a lookup, which campaigns a leaderless group; the own member is read first on every visit anyway.
- **Moved.** Each voter answers `/fold {"at": n, "since": n}` with `moved` from its in-memory log (`crate::load::moved`): an entry above `n` with events, an id block, or a lease extension; or `n` below its log base, which it cannot see past. A no-op or config entry does not move a chain, so the audit's own past elections do not refold it.
- **Idle.** All voters answered, none moved, and each holds both digests at `n`: compared with the match; equal is `idle`, unequal alarms (`console_error`, "idle") and clears `matched`. Anything else falls through to the leader path, unchanged.
- **Deleted.** A tombstoned chain (`git::purge::purgeable`, answered by the own member) is listed `deleted`; `audit_route` starts no refold for it without `force`.
- **Cost.** `scripts/cost.py` "Fold audit refolds": $30, $390 and $482 a month at 50, 300 and 2,000 developers; base totals now $261, $1,579 and $9,669 (were $231, $1,189 and $9,187, the audit unpriced). Copied into `design/DESIGN.md` Cost, `design/AGENT-LAYER.md` Cost and `alternatives/AWS.md`; `research/costs.md` regenerated.
- **Tests.** `only_an_entry_that_writes_the_fold_moves_a_chain` (load.rs) and `an_unmoved_chain_is_compared_at_its_last_match_and_not_refolded` (fold_audit.rs). Both rules are new, so both fail to compile on main. Not run hosted: the first site day carrying it should show `idle` chains in the cron's report and a $0.00 PREFIX meter on an idle install.
- **Follow-up.** K11 (weekly live-row check) opened in OPEN-QUESTIONS.
- **Row size (2026-11-18 14:01Z).** M4-WP3 read `graft-m4a`'s j4 (`repo:65536`), read-only, $0.00 by PREFIX: (dbBytes - logBytes) / rows is 619 to 622 B on `#1#f0` and `#1#f2` and 740 B on the lost voter `#2#f1` (6,171 metadata rows, 32 core rows; refold equal to the kept digest on all three); snapshot frames 65 B per metadata row, 129 B per core row. `ROW_KB` 0.62: the refold line is $421, $2,517 and $3,110, totals $656, $3,726 and $12,463 (K10).

## F88 fixed: a committed entry folds by its own format (2026-11-18, code-only, branch `f88-format-fold`)

- **Fix.** `Tiered::write` takes the entry's format. `Promoted` writes `promoted_log` only in a 32.25 entry, and the archive step records a promotion in 32.25 only with `GRAFTYARD_ROLLUP` on (`decisions/DECISIONS.md`, "F88: a committed entry folds by its own format"). The reproduction test passes; it failed on main.
- **Guard.** `crates/git/tests/fold_corpus.rs`: one fixture per `RepoEvent` kind, 48 kinds, folded at every format version each can be committed at (902 rows) against `fold_corpus.golden`. Versions 1 to 24 are pinned on `0d212879` and version 25 on `e2bddc92`. On `e2bddc92` the corpus differs from `0d212879` in exactly `Promoted` 1 to 24 (core); on this branch it matches both. `tests/m0carried/plant40.py`: 4 of 4 caught, among them roll-up's unconditional write.
- **graft-m1's earlier alarms (step 3).** The corpus run on the main commit before each graft-m1 site-script upload from 2026-11-13 19:56Z to 11-16 17:29Z (`a21723a7`, `ca66ccc6`, `dc954421`, `83758b28`, `79eb701a`, `605f25ca`, `6782f8f5`, `7ac6d880`, `b3a5b3ce`, `c75f2138`) gives the same digests as `0d212879` for every kind each release decodes: 0 of 844 to 901 rows differ. So no release before roll-up changed what an existing event folds to, as far as the corpus's single-entry fixtures reach. m1's two early alarming chains are metadata-tier mismatches, not core: `repo:1048587` (B0-30 smoke 2's split parent, from 11-14 18:33Z, refolded from a 256 MB metadata snapshot at 6 over 289 entries) and `repo:196628` (BF-20's 3.99 GB tier, from 11-16 09:47Z, only sites 0 and 2 compared differing). Both are split or slim chains, where the metadata tier's rows are pruned by `MetaSlim` by `written` index, which a restore restamps (`crates/git/src/split.rs`, `slim`; `workers/site/src/load.rs`) [inference, untested]. graft-m1's alarms from 11-17 10:46Z to 11-18 09:00Z are on 35 chains, 34 of them on the core tier (first `repo:1048578` at 89), and follow its 11-17 00:13Z site day from `59ee6035`, which carries roll-up: F88 itself.
- **Leftover divergence (step 4).** Chains whose members applied 32.1 `Promoted` on a roll-up release and that are still joined alarm once more after this lands, and are reseeded or retired by their owners, not backfilled: graft-m1's 34 core-alarming chains above, graft-m4a's 589824 to 589827 (alarming from 11-17 18:01Z) and its kept `repo:65567`, and on graft-m2b its one alarming chain, its 12 kept chains and B0-34's networks (also on m1). Their owners have the per-install lists from the orchestrator. graft-m3f has none joined (repo:0 aside).

## F88 traced: roll-up changed the `Promoted` fold in place (2026-11-18, code-only, no spend)

- **Cause.** Roll-up (merge `e2bddc92`, 2026-11-16 18:29Z) made `RepoEvent::Promoted` also write a `promoted_log` core row (`crates/git/src/tiers.rs`, `write_core`). Members applied their promotions on the older release; the audit's refold replays them on the new one and adds the row, so the core digests split.
- **Bound.** All `graft-m3f` audits matched before 11-17 09:47Z. The first alarm was `repo:65568` at 1343. All 26 distinct alarming audits after m3fr11's site day (11-17 03:30Z) had a `Promoted` entry in the refolded tail (Workers Logs `archive Promoted`, mapped to objects by id).
- **Which side.** Neither apply is wrong for its own release. Live state is what the older release wrote; the refold is what the newer one writes. This breaks `design/SHARED.md`'s dedicated-release rule for core-tier schema changes, not `decide`'s purity.
- **Reproduction.** `crates/git/tests/encoding.rs`, `a_committed_promoted_entry_folds_to_the_digest_it_had_when_it_committed`: one `Promoted{from 1, to 17}` entry folds to `4ceb4070…77cc` on `0d212879` and `d5c31b8e…d51f` on main. It passes on the old release and fails on main; it lands with the fix.
- **Risk.** Chains that promoted before the upgrade hold no `promoted_log` rows for those promotions, so roll-up's `rollable` and `check_compacted` miss them. Roll-up is off on m3f and m4a; `graft-m2b` has `GRAFTYARD_ROLLUP=1` from m43 [inference].
- **Not traced.** `graft-m1`'s alarms began 11-14, before roll-up.

## B0-32 design proposal: `b032-shed` (approved 2026-11-18 and built on branch `b032-shed`; DECISIONS "`b032-shed`")

Two changes, one row. Part A stops a stalled leader's read-index timeouts from turning into leader reads. Part B bounds an agent push at 5 s from its arrival at the edge, so a push that cannot commit in time answers B0-32's `ng` shape.

### Part A: a member that forgot its leader sheds the read and hands the edge the report

**Today** (`tests/am1/results/lm1-rows-m3f.md`, "Why the burst's reads reached the leader"):
- `check_remote` (`crates/raft/src/member/install.rs`) fails a batch's reads after `CAMPAIGN_DEADLINE`, sets `forgot`, and clears `leader` (F67 rule 12).
- After that, `queue_remote` answers every new read `NotLeader` naming no leader, at once, until an append names a leader again.
- The site's `/read` renders that as `{"error": {"notLeader": ..}}`. `spread_verdict` (`crates/lookup`) reads "every member declined" as `Spread::Leader`, and `call_opt` walks the voters with plain reads and `suspect` reports.

**Change:**
1. `crates/raft`: one read-only accessor, `Member::forgot() -> Option<SiteId>`, the leader this member forgot by rule 12. `check_remote`, `queue_remote`, `forgot`'s set and clear points, and every timer stay as they are. Rule 12's own campaign (`try_campaign` on a lapsed promise) is unchanged.
2. `workers/site/src/member.rs` `read`: for an `index` read that fails `NotLeader` naming no leader while `forgot()` is `Some(l)` (both the step's `Err` and the resolution's), answer `overloaded_reply(OVERLOAD_RETRY_MS)` with `"forgot": {"site": l, "term": t}`. A genuine `NotLeader` is unchanged: one naming a leader, or one with no `forgot` (a `ReadIndexRes` saying "not leader" clears `leader` without setting `forgot`).
3. `crates/lookup` `spread_once`: on `Spread::Shed`, if any answer carried `forgot`, send one `suspect` about that member at that term (`suspect`, damped by `SUSPECT_GAP` as now, `all = false`). The read is never sent to the leader. `spread_verdict`'s rules are unchanged; a shed answer was already `Shed`.

**Why liveness holds** [inference; the sim and long sweep are the check]: the reports that elect off a half-hung leader today come from `call_opt`'s walk after the decline. Under A they come from the shed answer instead, at the same damping, about the same member and term, and without the leader read. `a_read_only_group_elects_off_a_half_hung_leader` is the case.

### Part B: an agent push is answered within `AGENT_PUSH_BOUND` (5 s) of its arrival at the edge

**Today:** the only time limit on an agent command is `AGENT_WAIT` (30 s, `expire_agents`), counted from its admission at the leader. On m3fr15 the server spent p50 8.3 s before `commit` (gate p90 10.0 s, context p90 13.4 s). A push read retried a shed read for up to `LOOKUP_FOR` (20 s, `read_spread_for`).

**Change**, with the agent block edited identically in `design/DESIGN.md` and `design/AGENT-LAYER.md`:
1. **`AGENT_PUSH_BOUND` = 5 s** [doc, `build/BUILD-PLAN.md` B0-32's pass line], counted from the edge's `Timing::start`. It applies to a push the edge may class agent without a read (an App token or `X-Graftyard-Agent-Session`, the `needs_install` test), and the leader applies it to a push `admission` classed Agent.
2. **Edge, before the commit send:** each wait that is not the commit (`read_spread_for`'s shed retries, the validate, the stage) ends at the bound, not at `LOOKUP_FOR`. Past it, the push answers `ng <ref> agent admission full, retry after <n>s`. `n` is the largest `retryAfterMs` seen, rounded up, at least 1. Nothing was sent to the group, so nothing can commit later. A human push keeps today's waits.
3. **The commit send:** `AdmitWire` gains `budget_ms`, the bound left at the send. It's relative, so the edge's and the member's clocks never meet.
   - `crates/raft` `Admit::budget: Option<Duration>`.
   - `admission_check` answers `Full` at once when the expected wait (queued agent commands over `AGENT_SHARE` × capacity, plus one round) is past the budget.
   - `expire_agents` answers `Full` to a queued agent command once its budget passes, as it does at `AGENT_WAIT`.
   - Only a command still queued, in no entry, is expired, so no acknowledgment can be lost: the same rule as `AGENT_WAIT` today.
4. **Not bounded:** a command already in an entry. It waits for its outcome as now. On m3fr15 the entries' rounds ran 2.5 to 4.1 s while the leader stalled, so such a push can still finish past 5 s [risk; the rerun measures it].

### Tests that fail on main

- `crates/raft` (`leadership.rs`): a learner whose read-index batch the leader never answers reports `forgot() == Some(leader)` after `CAMPAIGN_DEADLINE`, and a new read answers `NotLeader` with no leader. The control: one cleared by a `ReadIndexRes { index: None }` reports `forgot() == None`. Fails to compile on main, which has no accessor.
- `crates/raft` (`admission.rs`): with the agent lane full, an agent command whose budget is under the expected wait is refused `Full` at once; another, queued with budget 2 s, is answered `Full` at 2 s, not at `AGENT_WAIT`; a Human command beside them is never refused. Fails on main, which has no budget.
- `workers/site` (`member.rs` pure test of the answer mapping): `NotLeader` with no leader and `forgot` → `overloaded` with `forgot`; with a leader named, or no `forgot` → `notLeader`.
- `crates/lookup`: a spread whose every answer is `overloaded` with `forgot` returns `Shed` and sends one `suspect` about that site, with no post to the leader; a spread of plain `notLeader` still goes to the leader.
- `workers/edge`: a push whose bound passed before the commit send renders `ng <ref> agent admission full, retry after <n>s`; a human push in the same state does not.
- `tests/m0carried/plant36.py`: mutants for each rule (shed without the report; report without the shed; budget ignored at admission; budget ignored in `expire_agents`; a queued command expired once it is in an entry). Every mutant must be caught.
- Simulator: the harness's `spread_read` (`crates/raft/tests/common/sim.rs`) follows Part A: on a declined read from a member with `forgot`, it sheds and reports instead of taking the leader's path. Then the long sweep (`RAFT_SEEDS=2000 RAFT_STEPS=10000 … long_sweep`) and Porcupine on its three histories, plus `long_liveness` on 4,000 live seeds.
  - Note: LM-1 reports seed 10353 already stalls 31.8 s on main (late-suspect with read-beat). A run of this branch is judged against main's own sweep, seed by seed.

### Built (branch `b032-shed`, on main `67db458f`)

- **A, the read shed:**
  - `raft::Member::forgot()` (read-only; `check_remote` is unchanged).
  - The site's `not_leader_reply` sheds a read-index read on a member that forgot its leader, with `forgot: {site, term}`.
  - Lookup's `spread_once` sends one damped `suspect` about it and never reads on the leader.
  - The simulator's `spread_read` does the same.
- **B, the bound:**
  - `AGENT_PUSH_BOUND` (5 s) is in both agent blocks.
  - On the edge, `Ctx::bound` is set from the push's arrival for an App, a token with an App, or a session header. Shed-read retries stop there.
  - `bound_at_send` refuses an Agent push with nothing left, and `gate_ng` answers a shed gate read at the bound with `ng` instead of 503. Both use B0-32's shape, `ng <ref> agent admission full, retry after 1s`.
  - `AdmitWire::budget_ms` → `Admit::budget`. `admission_check` refuses when the expected wait passes the budget, and `expire_agents` expires at each command's `until` from the lanes only.
- **Still 503:** a gate failure other than a shed read (no leader, not created), and a push the edge can't class Agent without the gate read (DECISIONS flip line). The rerun counts these against the bound line.
- **Checks:**
  - The full checks on the merged head: see the report.
  - plant38: 6 of 6 caught.
  - Long sweep: harsh 16.8 s, gray 24.2 s, live 29.1 s (seed 10681), all under 30 s. `progress-suspect` alone gives 16.8 / 24.2 / 25.8 s.
  - Seeds 10000 to 10681 alone run identically on both builds (worst 23.9 s at 10312), and 10681 alone runs 2.7 s on both. Not traced: what the full run carries into it.
  - Porcupine Ok on all three histories.

### The rerun (m3fr17, once merged)

- A site day on graft-m3f from the merged main, then edgever, then B0-32 alone.
- About $2. Stop at $5 by PREFIX, or when the account passes $180 for the day.
- Report:
  - the burst's within-5 s count;
  - `ng` by shape (commit send, gate, leader `Full` at admission, leader expiry), from server-timing `bound` and the client codes;
  - 503s by cause;
  - pushes past 5 s whose command was already in an entry: their `commit` stage started before 5 s and ended after it;
  - `/suspect` and leader changes, from `terms.mjs`;
  - plain `/read`s at the voters while the learners shed, from `tsave`.

### What the B0-32 rerun then checks

- The pass line as written.
- From the trace: no plain `/read` at the voters while learners shed; `/suspect` at most one per isolate per `SUSPECT_GAP`.
- Every push past 5 s before its send answers `ng`.
- The count of pushes over 5 s whose command was already in an entry (Part B, item 4).
- About $2 with a site day, which the site-script change needs.

## Rows m3fr17 (2026-11-18, graft-m3f, `ROWS="b0-32"`, main `6dcf061a`, `b032-shed`)

> **Annotation, 2026-11-18 (cut-trace).** This run's build (main `6dcf061a`) carries lease-clock (`0b5769b7`), so its
> `/stats` `queue`, `queue:slot` and `queue:post` figures mix two clocks and overstate the wait. Commit p99 and the
> verdicts were on one clock and stand (`tests/m0/results/cut-trace.md`).

**Slot.** B0-32 rerun on `b032-shed`, granted at about $2. Stops: graft-m3f past $5 by `PREFIX` from the site day's start, or the account past $180. Raw files: `tests/am1/raw/a1-b0-32-mwljfpch.jsonl`, `tests/am1/raw/runs/{siteday-m3f-m3fr17,a1-b0-32-fm3fr17-b032}.log`, `tests/fanout/raw/rows-m3fr17.status`, `tests/fanout/raw/{terms,admit}-m3fr17-b0-32.jsonl`, `tests/fanout/raw/tsave-m3fr17-b0-32.jsonl.xz`, `idle-retire-m3fr17.log`.

- **Site day** 02:41 to 02:54Z, image `sha256:0f0c5ee1…` reused. Edge `f6acd65d` at 100% (edgever, rev `11cb53c0`).
- **Setup:** repo 65587, config 5 with 8 learners, k 4. a1's `feed members` read **8 learners, 2 followers**, and the learners caught up 8/8 in 15.9 s. Every member read `5/8` from 03:04:21Z to the trace's end: **no config change after `/learners`**.
- **Client protocol:** the burst ran on HTTP/1.1 (node:https Agent, keepAlive, maxSockets = burst pushes). The rest of the driver ran on HTTP/2.

### Result: fail, 0 of 500 within 5 s by the client [measured]

| Line | m3fr17 |
| --- | --- |
| Burst outcome | 474 `ok`, 20 `full` (`ng … agent admission full, retry after`), 4 connect `ETIMEDOUT` and 2 client timeouts, none of which reached the edge |
| Within 5 s, client clock | **0** of 500. `ok` p50 9.7 s, p99 13.2 s, minimum 5.6 s; `full` answered at 8.3 to 12.7 s |
| Within 5 s, edge clock (server-timing `total`) | 167 of 474 `ok`. Server total p50 5.5 s, p99 7.4 s. The client spends about 4 s more than the edge, all before the edge's clock starts (500 new HTTP/1.1 connections from the orb) [inference] |
| `ng` by shape | 20 `full`. 5 are the edge's bound (`bound;dur=0` after validate, before any send). 15 got the leader's answer after a commit send: 9 on a resend past 5 s, 4 in an entry before 5 s, 2 with the first send past 5 s. No gate `ng` |
| 503s | 0 on burst pushes. Other traffic: 41 on `read:api`, 4 on ref reads, and 1 merge `503 the git cache … warming` |
| Pushes past 5 s (edge clock) already in an entry | 327 pushes ran past 5 s. In 295 of them (291 `ok`, 4 `full`), the last commit send began before 5 s, which is the case the bound leaves open. 12 `ok` first sent past 5 s, so the bound did not refuse them (see below) |
| `/suspect` and leader changes | 5 `/suspect` from the edge (`pushTiming`), 18 each at sites 1 and 2 by `tsave`. One leader change, term 2 → 3, site 0 → 1, at 03:10:32 to 03:10:33Z. Admission was not stalled then (queued 0 to 9, capacity 308 to 1,253) |
| Plain `/read`s at the voters | Spread reads (`rows`, `core`, `packed`) went to the learners. The voters' plain `/read`s were the leader's own `ranges` (about 10 to 18 a second, leader-only by design). No `notLeader` fall-through to voter reads like m3fr15's 213 to 484 a second |
| Leader | Commit p50 459 ms, p99 3,514 ms; queue p99 3,765 ms; round p99 485 ms; confirm p99 183 ms; apply p99 657 ms; `deps:all` p99 787 ms. Admission queue at most 9 |

### What it shows

- **The shed works.** The learners kept the spread reads, and no voter took the reads that m3fr15 sent there. The leader did not stall: apply p99 657 ms against m3fr15's 3.2 s gaps, and confirm p99 183 ms against 1.0 to 1.5 s.
- **The time now goes before admission.** On the edge clock, a push spends about 3 to 4 s in `auth`, `resolve`, `install`, `context` and `validate` (each p99 1.6 to 1.9 s) before it reaches the leader. The leader then commits it in about 1 s. Admission saw at most 9 queued commands and refused only 15.
- **The bound refused only 5.** `bound_at_send` refuses a push only when nothing is left of its 5 s. A push that reaches send at 4.5 s still goes, and its commit takes about 1 s. So 291 `ok` pushes ran past 5 s with their command already in an entry, which the DECISIONS flip line names. 12 `ok` pushes had their first send past 5 s. Why the bound let those through is not traced: the edge's bound may start later than the server-timing `total`.
- **Not traced:** why the leader changed at 03:10:32Z without a queue, and why the 12 sends past 5 s went out.
- **Close-out:** SWEEP2 OK at 03:18:25Z. 65587 retired 11 of 11 at 03:20:34Z; the purge removed 3,132 objects at 03:25:23Z, and the second pass found 0. The sweep at 03:25:49Z found no shard container running.
- **Cost:** graft-m3f **$0.73** by `PREFIX` from 02:41:29 to 03:25:51Z (site day, row, sweeps, retire and purge). The account's UTC day was $6.53 at 03:25Z. The idle reading was not taken: the repository-wide pause came first.

## Rows m3fr16 (2026-11-17/18, graft-m3f, `ROWS="b0-31"`, branch `m3fr16`, main `7430a479`)

> **Annotation, 2026-11-18 (cut-trace).** This run's build (main `7430a479`) carries lease-clock (`0b5769b7`), so its
> `/stats` `queue`, `queue:slot` and `queue:post` figures mix two clocks and overstate the wait. Commit p99 and the
> verdicts were on one clock and stand (`tests/m0/results/cut-trace.md`).

**Slot.** B0-31 rerun on `dep-hedge` (main `7430a479`, with M4-WP4's F84), granted at about $3. Stops: graft-m3f past $6 by `PREFIX`, or the account past $280 (on 2026-11-18, past $180).
- **Site day** 23:35 to 23:48Z (`siteday-m3f-m3fr16.log`), image `sha256:0f0c5ee1…` reused. Sites `ab8fd5a9` / `51f0273b` / `c1969c00` (phase 1 `4a15c906` / `6a53b48b` / `6f52f304`). Edge `23813b68` served at 100% by edgever.
- **Setup:** repo 65586 rehomed on site 2 (term 2 → 3 by the transfer, before the load), apac shard warm in 4.4 s, config 10 with 8 learners, k 4. a1's `feed members` read 8 learners and 2 followers. Its catch-up check read "0/8 in 120 s" (commit 117), but the learners stood at 1,249 to 1,254 applied at 00:00Z and served 1,000 to 2,000 read-index reads a minute each through the load (`tsave-m3fr16-b0-31.jsonl`), so the check's `/status` reads failed rather than the learners lagging [inference; not traced].
- **Cost:** graft-m3f **$1.81** by `PREFIX` from 23:35 to 00:13:34Z (site day, row, sweeps, retire and purge); account $3.92 over the same window. The account's UTC day 2026-11-17 closed at $180.94; 2026-11-18 was $1.27 at 00:13Z.

### B0-31 (repo 65586, rehomed on site 2, run `mwlcj6zb`): fail on the buckets-up line, by 734 ms

| Line | Measured | At 90% |
|---|---|---|
| Commit p99 at most 1 s with buckets up | leader **1,834 ms** (n 5,364, p50 773, max 3,055) | Bound 1.1 s: **short by 734 ms** |
| Placement failing: Human and MergeQueue p99 at most 3 s, 0 refused | leader commit p99 2,559 ms (n 4,318); `push:human@fail` 89 ok, 0 refused; `merge:only@fail` 30 × 200 | Pass |
| Agent pushes over INLINE_MAX_HOT refused at once in `ng` | `push:agent@fail` 1,262 of 1,262 `full`, p50 1.1 s | Pass |

`splitByHalf.up`, ms, p50 / p99 (m3fr12 in brackets):

| | m3fr16 | m3fr12 |
|---|---|---|
| cycle (append to append) | 253 / 619 (n 1,091) | 246 / 562 |
| held at each append | 1 / 1 (n 1,092) | 1 / 2 |
| round | 233 / 510 | 230 / 360 |
| deps:all | 358 / **820** | 373 / 800 |
| deps:authz | 317 / **745** (n 2,152) | 725 p99 |
| deps:tokens | 187 / 524 | |
| deps:names | 166 / 470 | |
| queue:slot | 654 / **2,233** | 636 / 2,486 |
| entries in the up half | 1,091, 5,036 queued commands (4.6 each) | 1,132 (3.4 each) |

- **`dep-hedge` asked 14 times and won 2** in the whole row (`leader.depHedges`), against about 3,700 dependency reads in the up half. Its threshold is 1 s, and `deps:authz` p99 was 745 ms: the tail is reads of 0.5 to 0.9 s, not reads held past 1 s. So the hedge did what it is built for, and the line did not move.
- **No opening stall.** The admission trace (`admit-m3fr16-b0-31.jsonl`, per second) held the queue at 0 to 16 through the up half, with its largest value 16 at 23:59:42Z and 14 at 23:57:53Z, and `roundMs` 201 to 303. Nothing like m3fr12's 4 s freeze recurred, so this run's p99 is the steady tail.
- **Leader held:** site 2 at term 3 from before the load to after it, `failedCalls` 126 (deadline, to the two followers), no `/suspect` in `pushTiming`.
- **What sets the 1.8 s** [inference from the split]: a command waits for the next entry (`queue:slot` p50 654, p99 2,233 ms), and that entry pays its batch's dependency reads (p99 820) and a round (p99 510). 820 + 510 = 1,330 ms before any queueing, already over 1.1 s, so no admission or hedge change gets the p99 under the bound while each entry reads its owner's `Authz` across the continent. That is F81's case: the remaining gap is the `deps:authz` distance plus a round, not held requests.
- Other rows' numbers: `push:agent@up` 1,238 of 1,238 ok (p50 4.0 s, p99 6.7 s), `push:human@up` p99 51.8 s (94 ok, 4 stale), `merge:only@up` 32 × 200 and 4 × 409. Push stages: validate p99 4.4 s, commit p99 2.4 s.

**Close-out:**
- SWEEP2 OK at 00:10:18Z. The sweep at 00:13:34Z found no shard container running.
- At 00:11:38Z, 14 of 14 members of 65586 were "not joined" (`lm1-rows-m3f-data/m3fr16-retire.json`; `#2#l7` was listed on sites 1 and 2 as well as site 0, and those two were already not joined).
- Purge at 00:13:12Z: 6,334 objects (a 2,427, b 1,725, c 2,182, all to 0); the second pass found 0.
- Idle reading from 00:13:30Z, scheduled for 00:45Z.

## Rows m3fr15 (2026-11-17, graft-m3f, `ROWS="b0-32"`, branch `b032-h1`, main `368498ad`)

> **Annotation, 2026-11-18 (cut-trace).** This run's build (sites from main `0105297a`) carries lease-clock (`0b5769b7`), so its
> `/stats` `queue`, `queue:slot` and `queue:post` figures mix two clocks and overstate the wait. Commit p99 and the
> verdicts were on one clock and stand (`tests/m0/results/cut-trace.md`).

**Slot.** B0-32's burst rerun with a1's burst client on HTTP/1.1, granted at about $1 with no site day. The sites and edge stay on m3fr14's build from main `0105297a`. Stops: graft-m3f past $5 by `PREFIX`, or the account past $280.
- **Client.** The 500 burst pushes go over `node:https` with their own Agent (keepAlive, `maxSockets` = 500). The rest of a1 keeps `fetch` (undici, HTTP/2), so its other numbers compare with m3fr13 and m3fr14. The burst row records `protocol: "http/1.1 (node:https Agent, keepAlive, maxSockets = burst pushes)"`.
- **Cost:** graft-m3f **$0.66** by `PREFIX` from 21:26:09Z to 21:58Z, covering the row, sweeps, retire and purge (`rows-m3fr15.status` cost line: $0.99 for the account over the row). The account's UTC day was $175.14 at 21:25Z and $176.38 at 21:58Z.
- **Edge errors** (`graft-m3f-edge`, 21:30 to 21:39:10Z): "Cannot perform I/O on behalf" 0, "closure invoked recursively" 0, "code had hung" 0.
- **Close-out:**
  - SWEEP2 OK at 21:40:58Z.
  - At 21:50Z, 19 of 19 members of 65585 were retired (`lm1-rows-m3f-data/m3fr15-retire.json`): `#1#f0` to `#1#f2`, the re-placement voters `#2#f1` to `#9#f1`, and `#7#l0` to `#7#l7`.
  - idleretire's own purge was cut off by a 300 s `timeout` I wrapped around it. I reran purge.py alone at 21:57Z. It found 0 objects on both passes, a 0, b 0, c 0, so the run left no packs in the buckets.
  - The sweep at 21:58Z found nothing running.
- **Idle reading** 21:57 to 22:27Z, after the purge: graft-m3f **$0.00** by `PREFIX`. The sweep at 23:05Z found nothing running. The account's UTC day was $177.77 at 23:05Z. The scheduled reading did not run during a long turn, so it was taken at 23:05Z over the same window.
- **m3fr14's idle reading** was cut short at 21:26Z by this slot: $0.05 for 21:15 to 21:26Z, and the sweep found nothing running.

### Setup: learners held, a1's catch-up check was stale

- Config **11** with 8 learners committed at 21:29:07Z. Every member read `11/8` from then through 21:39:14Z, so no config change committed after `/learners` (`terms-m3fr15-b0-32.jsonl`).
- a1's `feed members` read **8 learners, 2 followers**.
- a1 logged "learners caught up to 116: 0/8 in 122 s". A later `/status` read showed all 8 learners at the leader's commit (156 to 161), so a1's check, not the learners, was stale [measured]. It cost 122 s of the warm-up and did not change the row.

### B0-32 (repo 65585, run `mwl7grmz`): fail. The burst is now a product reading, and the leader changed twice

| Line | Measured | At 90% |
|---|---|---|
| Every burst push acknowledged within 5 s or `ng … admission full` | **500 of 500 ok, 0 within 5 s.** Client p50 18,944 ms, p90 27,055, p99 28,952, max 29,212 | **Fail** (needs 450) |
| Queued agent bytes under AGENT_QUEUE_MAX | peak 68,757 B, 11 queued | Pass |
| Merges in one of the next 2 entries | not judged from entries; `merge:only` 16 of 19 ok (3 × 409 "Base branch was modified"), p50 2.7 s, p99 10.9 s | Not shown |
| Zero leader changes and zero `suspect` | **terms 2 → 3 → 4**; `pushTiming` 1,274 `suspect` | **Fail** |

**The transport is fixed.** All 500 burst pushes reached the edge and were answered (m3fr14: 0 of 500). Each ended `ok`, which shows the burst was accepted.

**Where the burst's time went** [measured, the burst pushes' own `Server-Timing`], p50 / p90 / p99 ms:

| Stage | p50 / p90 / p99 |
|---|---|
| auth | 453 / 4,089 / 5,734 |
| install | 486 / 1,182 / 2,399 |
| resolve | 985 / 4,037 / 6,564 |
| gate | 0 / 9,990 / 16,066 |
| context | 832 / 13,386 / 15,505 |
| validate | 484 / 708 / 1,041 |
| commit | 4,573 / 9,917 / 10,770 |
| total (server) | 13,638 / 21,758 / 22,672 |
| client less server total | 5,154 / 6,215 / 6,564 |

- **The commit wait alone is 4.6 s at p50**, so the 5 s line can't hold before anything else is added. 146 of the 500 pushes got at least one `busy` answer, up to 17 each.
- **About 5 s of every push is outside the Worker's timed total.** That is upload and queueing before the handler's first stamp, or after its last. It is the same for every push (p50 5.2 s, p99 6.6 s), so it looks like the 128 MiB of 256 KiB bodies arriving over 500 sockets in under 5 s [inference]. It is not measured per hop.
- **The row's other pushes** (`pushTiming`, all 1,305 timed): auth 90 / 5,287; resolve 562 / 5,513; gate 0 / 15,058; context 321 / 14,816; validate 328 / 15,600; commit 1,114 / 8,988; total 4,855 / 22,545. Receipts: commits 1,818, send 4,542, busy 1,323, ask 287, missed 1,274, suspect 1,274.
- **Leader** (site 0 at the end, term 4): commitMs p50 684, p99 4,936; queue:slot p99 5,666; round p50 270, p99 767; confirm p50 89, p99 422; cycle p50 311, p99 1,284.
- **Other traffic:** agent pushes 747 ok, 16 `fetch failed`; read:ref had 244 × 503 and 58 × 500.

### The two leader changes (repo 65585)

From `terms-m3fr15-b0-32.jsonl`, plus `vmem-m3fr15-b0-32.jsonl` (`/elections`) and `tsave-m3fr15-b0-32.jsonl` (the `Reported`, `Confirm`, `Applied` and `AppendEnded` traces; 23 MB, kept on the LM-1 rows orb, not committed, as on earlier rounds), and the sites' Workers Logs [measured]:

1. **The leader stalled under the burst.** The burst fired at 21:36:16.7Z.
   - On site 0 (`#1#f0`), the applied index stood still for 1.0 to 3.2 s at a time: 21:36:36.3 (1.0 s), 37.3 (1.2 s), 38.4 (**3.2 s**), 42.5 (2.4 s) and 44.9 (3.2 s).
   - Its ticker woke up to 659 ms late (21:36:38.96).
   - The AppendEntries to the followers ended in 2.5 to 4.1 s at 21:36:41.6.
   - Storage confirms for entry 309 took 1.0 to 1.5 s at 21:36:44.0 (run p50 89 ms).
   - Its alarm ran 1.2 to 1.6 s wall at 0 ms CPU (21:36:37.4, 39.6 and 45.6), and a burst of `/raft` calls was held for 0.4 to 2.2 s.
   - An `/entries` page logged "capped at 1000000 B: 192 reads in flight" at 21:36:37.1.
2. **The edge reported the leader.**
   - From 21:36:32.9Z, each follower took 957 `/suspect` reports about site 0: 412 at term 0, 400 at term 2 and 99 about site 2.
   - Each follower answered `promised` while its promise to the leader stood.
3. **Term 3.** At 21:36:44.62Z both followers' promises had lapsed, and the next report started a pre-vote on each (`Reported` verdict `started`; `Elect::Asked` with started 1).
   - Site 1 (`#6#f1`) won term 3 at 21:36:45.0 and committed its first entry (310) at 45.18.
   - Leaderless: none; the old leader kept serving until 45.1.
4. **Term 4.** At 21:37:04.18Z site 0 became a candidate without a pre-vote and won at 04.37, first commit 365 at 04.58. That is the leader's hand-off back to the group's `home`, the transfer `crates/raft` starts in `health.rs` once home answers `HOME_CALLS` calls with none failed [inference from the vote shape: a direct vote, not a pre-vote, from home]. It is by design, but the row's zero-change line counts it.

This is the k 4 churn LM-1 and its reviewer are diagnosing. The trace was sent to LM-1, as the brief asks. I did not change `crates/raft`.

### m3fr14's two `/suspect` (21:03:29 to 34Z): product latency, not the transport

Both came from one push (isolate `6aec1d13…`, push `01M7NN4NQTMWHSSNNAHEN7QCA0`), whose commit took 8,902 ms [measured]:
- **The receipts.** Its 2 missed receipts went to the followers `repo:65584#4#f1` and `#1#f2`, so it sent 2 `suspect`.
- **The leader stalled.** `repo:65584#1#f0` (`c7bf3474`) logged no event from 21:03:26.4 to 34.35Z.
  - Its alarm ran 8,048 ms wall at 20 ms CPU, then every held request completed at 34.368.
  - The storage confirm (`sync()`) for entry 1075 took **8,509 ms**. The run's 1,456 confirms were p50 57 ms and p99 202 ms, and the next highest was 354 ms.
- **The learners agree.** `#5#l3` and `#5#l7` show `/raft` walls of about 9.3 s ending at 35.07.
- **Not the transport.** m3fr14's transport event was 21:01:11 to 49Z, which does not overlap.

So both runs' `/suspect` come from leader stalls: an 8.5 s storage confirm on m3fr14, and 1 to 3 s apply gaps on m3fr15.

## B0-32 after m3fr15: why nothing was refused, and what stalled the leader (code-only)

From m3fr15's raw files (`admit-`, `tsave-`, `vmem-m3fr15-b0-32.jsonl`, a1's `burst timing`) and site a's Workers Logs for the leader `repo:65585#1#f0` [measured, except where tagged].

**Admission never had a reason to refuse.**
- **Every limit it checks was far off.** `admission_check` (`crates/raft/src/member/leader.rs`) refuses an agent command only at `AGENT_QUEUE_MAX` (1,000 commands or 64 MiB queued), while placement fails, or after `AGENT_WAIT` (30 s, `expire_agents`). Through the burst the leader's agent queue peaked at 136 commands and 342 KB (21:36:33 and 41Z), and `pressure` stayed false.
- **The 128 MiB never reached the leader.** The group was hot, so each 256 KiB pack was over `INLINE_MAX_HOT` (64 KiB), and the edge staged it in the buckets before its first commit (`hot::stage_first`, `stage` p50 523 ms). The leader queued `PackRef::Stored` commands of a few KB each. So staging does come before the admission check, and no byte limit counts the burst.
- **Most of each push's time came before admission.** The server time before `commit` was p50 8.3 s and p90 18.3 s (auth p90 4.1 s, resolve 4.0, gate 10.0, context 13.4). Then `commit` was p50 4.6 s, and the leader's `queue:slot` p99 5.7 s. Only 6 of the 500 had a server total of 5 s or less. So a bound on the leader's queue alone can't turn these into `ng` within 5 s.

**What stalled the leader: not CPU, memory or storage alone.**
- **CPU:** at most 605 ms a second in 21:36:20 to 37:04Z.
- **Memory:** wasm 23 to 32 MB, in-flight held at most 622 KB.
- **Storage:** confirms mostly under 500 ms. One reached 1,502 ms, at 21:36:44.
- **Inbound load:** the leader took 807 requests at 21:36:30, and 675 and 708 a second at 36:36 and 36:37, with up to 680 open in its handler at once. Their walls ran long at about 0 ms CPU.
- **Reads sent to the leader:** many of these were `/read`s sent to the leader, not as read-index reads. That was the push gate's `core` rows at 86 to 149 a second, and whole-tier `packed` dumps at 39 to 56 a second, with `/outcome_by_request` at up to 130 a second. The edge is meant to send those to learners and followers (`design/DESIGN.md`, "Follower read").
- **Inference:** the burst's own edge reads fell back to the leader, and hundreds of open requests held its handlers waiting, not computing. Why they reached the leader (no learners known to fresh edge isolates, or every member asked declining) is not yet traced.

### Why the burst's reads reached the leader (code-only, m3fr15's `tsave-m3fr15-b0-32.jsonl`)

Decoded from the members' inbound trace: `Entered` (25) for the route, `ReadQuery` (26) for the query and its `index` flag, `SentBy` (27) for the edge's lookup step, and the learners' read-index rows (1 `IndexSent`, 2 `IndexEnded`, 5 `IndexTaken` on the leader) [measured unless tagged].

| Second (21:36:xx) | Learners' `/read`s in | Their read-index calls ended ok / late / failed | Leader (site 0) `IndexTaken` | Plain `/read`s at the voters | `/suspect` at sites 1 + 2 |
|---|---|---|---|---|---|
| 28 | 29 | 25 / 0 / 0 | 27 | 16 | 0 |
| 30 | 324 | 65 / 0 / 2 | 126 | 24 | 0 |
| 31 | 305 | 35 / 16 / 35 | 1 | 3 | 0 |
| 32 | 386 | 5 / 16 / 82 | 2 | 2 | 8 |
| 33 | 620 | 0 / 79 / 92 | 0 | 4 | 66 |
| 34 | 730 | 4 / 40 / 80 | 2 | 33 | 130 |
| 35 | 1,331 | 1 / 0 / 2 | 2 | 213 | 144 |
| 36 | 1,523 | 2 / 0 / 17 | 19 | 484 | 409 |
| 37 | 699 | 0 / 0 / 8 | 30 | 459 | 454 |

- **The learners took the burst's reads first, as designed.** Through 21:36:34 almost every push-gate `core` read went to a learner by read-index (`SentBy` Spread, `index` 1), and the voters saw at most 33 plain reads a second.
- **Then the leader stopped answering the learners' read-index calls.** At 21:36:30 the leader had 446 requests open (423 `/raft`, 246 `/commit`), and from 21:36:31 its `IndexTaken` fell from 126 a second to 0 to 2. Its AppendEntries calls ran up to 1.66 s (21:36:32). The learners' calls ended late or failed.
- **A learner whose batch went unanswered for `CAMPAIGN_DEADLINE` (3 s) forgets the leader** (`check_remote`, `crates/raft/src/member/install.rs`, F67 rule 12). Its reads then answer `notLeader` naming no leader, at once. That is why the learners' inbound rate doubled at 21:36:35 to 36 (1,331 and 1,523 a second) while they served almost nothing.
- **The edge sends a read that every member declined with `notLeader` to the leader** (`spread_verdict`, `crates/lookup`: `Spread::Leader`, then `call_opt`, which walks for the leader and reports `suspect`). So from 21:36:35 the push gate's `core` rows, the `rows` and `packed` reads went plain to the voters: 213, 484 and 459 a second, `SentBy` Leader or Walk, 141 to 149 edge isolates each. The `/suspect` reports rose with them, 409 to 454 a second at sites 1 and 2.
- **So the reads did not miss the learners.** The edge knew all 8 (`SentBy` Spread through 21:36:34). They reached the leader because a learner that cannot get a read index answers `notLeader`, and the edge reads that as "not mine to serve", not as "shed". That turns the leader's stall into more load on the leader, and the reports into the term changes at 21:36:45Z.
- **What started the stall** is the burst itself on the leader: 246 `/commit` and 423 `/raft` open at 21:36:30, then AppendEntries of 1.0 to 1.7 s. The learners' read-index traffic (up to 126 a second) added to it. Not separated: how much of the leader's wait was its own storage and how much its input queue [inference].

## Rows m3fr14 (2026-11-17, graft-m3f, `ROWS="b0-32"`, branch `m3fr14`, main `0105297a`)

**Slot.** B0-32 rerun on `learners-kept`, after a site day from main `0105297a`. The orchestrator granted it at about $2. Stops: graft-m3f past $5 by `PREFIX`, or the account past $280.
- **Site day.** It took three attempts (`tests/am1/results/install-m3f.md`, m3fr14 row):
  - The first deployed site a, then site b's PUT got a 504.
  - The second failed on a transient `r2/buckets` API error before it deployed anything.
  - The third deployed all three sites, 20:51:42 to 20:51:58Z: `3ea01a61`, `aaf974e0` and `836e2ba9`.
  - The edge `58f2b859` (first attempt) served at 100% by edgever.
- **Cost:** graft-m3f **$1.15** from 20:23:10Z to 21:15Z: site day, row, sweeps, retire and purge. The account's UTC day was $165.61 at 20:22Z and $174.66 at 21:15Z.
- **Edge errors** (`graft-m3f-edge`, 20:55 to 21:04Z): "Cannot perform I/O on behalf" 0, "closure invoked recursively" 0, "code had hung" 0.
- **Close-out:**
  - The sweeps at 20:53, 21:06 and 21:15Z found nothing running.
  - At 21:13Z, 14 of 14 members of 65584 were retired: `#1#f0` to `#1#f2`, `#2#f1` to `#4#f1`, and `#5#l0` to `#5#l7` (`lm1-rows-m3f-data/m3fr14-retire.json`).
  - 955 objects were purged (a 329, b 330, c 296, all to 0). The second pass found 0.

### `learners-kept` on the hosted install: the learners stayed

From `tests/fanout/raw/rows-m3fr14.status` and `terms-m3fr14-b0-32.jsonl` [measured]. The terms trace now records each member's committed config index and its learner count:
- **The placement finished before `/learners`.** The create's background `/place` committed config 5 (`Degraded`) at 20:54:42Z and config 7 (`Normal`) at 20:54:43Z. Meanwhile `learners.mjs` logged "placing since …: waiting" 10 times (the driver belt). Then it committed config **9** with 8 learners at 20:54:52Z.
- **No config change after `/learners`.** Every member read `9/8` from 20:55:01Z through the trace's last sample at 21:04:49Z, past the row's end at 21:04:34Z.
- **a1 confirms it.** a1's `feed members` read **8 learners, 2 followers**, and "learners caught up to 116: 8/8 in 11.9 s".
- **What this shows and doesn't.** The belt did its job, so the race the product fix closes didn't occur on this run. The product fix itself is shown by its tests and `plant33.py`, not by this run.

### B0-32 (repo 65584, run `mwl68qny`, leader site 0): not measured. The orb's connections to the edge failed for the whole burst

| Line | Measured | At 90% |
|---|---|---|
| Every burst push acknowledged within 5 s or `ng … admission full` | **500 of 500 ended in the client with no HTTP answer**: 230 `ERR_HTTP2_STREAM_ERROR`, 156 `ECONNRESET`, 114 `UND_ERR_SOCKET` | **Not measured** (see below) |
| Queued agent bytes under AGENT_QUEUE_MAX | peak 169,453 B, 16 queued | Pass |
| Merges in one of the next 2 entries | not judged from entries; `merge:only` 22 of 24 ok, p50 1.9 s, p99 23.6 s | Not shown |
| Zero leader changes and zero `suspect` | term 2 throughout, leader site 0; **2 `/suspect` about `repo:65584`** at 21:03:29 to 34Z, from two receipts a1 counted `missed` (`pushTiming`: ask 2, missed 2, suspect 2), each sent to both followers | **Fail** (the line's target is 0) |

**Why the burst line isn't a reading** [measured]:
- **The client sent the burst on time.** a1 sent the 500 burst pushes (256 KiB each, 128 MiB in all) from 21:01:11.357Z to 21:01:15.832Z.
- **Every one failed in the client, below HTTP.** The failures came in waves: 114 sockets closed at 21:01:15, 70 connections reset at 21:01:16, 86 more at 21:01:22, and 225 HTTP/2 streams reset from 21:01:44 to 49Z.
- **Almost none reached the Worker.** Over 21:01:10 to 21:02:00Z the edge's Workers Logs hold 231 `git-receive-pack` requests, all answered 200 with no exception. Only 13 were over 200 KB, and only 7 were the burst's size (262,746 to 262,751 bytes, at 21:01:20 to 30Z). Each of those 7 was served in 2.2 to 4.0 s, yet the client recorded them as failed too.
- **The orb's other traffic failed at the same time.** Over the run, agent pushes ended `fetch failed` or `terminated` 88 times (m3fr13: 8), and ref reads ended with status 0 1,503 times (m3fr13: 119).
- **The cause is unknown** [inference]. The fault is in the transport between the orb's Node client (undici, HTTP/2 to the edge) and Cloudflare's front end: almost none of the requests reached the Worker. The driver's burst path hasn't changed since m3fr13 (`git diff 1d3c897b 0105297a -- tests/am1/rest/a1.mjs` touches only B0-35's `--workflow`).

**The product side, from the 1,042 non-burst pushes the edge timed** [measured, `pushTiming`], p50 / p99 in ms (m3fr13 in brackets):

| Stage | p50 / p99 |
|---|---|
| auth | 80 / 825 (158 / 2,706) |
| resolve | 521 / 1,302 (615 / 3,710) |
| gate | 288 / 1,124 (0 / 5,979) |
| context | 0 / 971 (324 / 7,443) |
| validate | 237 / 3,345 (384 / 1,656) |
| commit | 733 / 1,286 (750 / 2,709) |
| total | 2,159 / 5,477 (3,126 / 12,321) |

- **Leader:** commit p50 558 ms and p99 1,106 ms over 4,663 commands. The cycle was p50 238 ms and p99 589 ms, and `held` was p99 1.
- **Receipts:** 1,147 commits on 1,149 sends, 0 busy, 2 asks, 2 missed, 2 `suspect`.
- **Context:** with 8 learners serving the feed, `context` p99 fell from 7.4 s to 1.0 s, and the timed total p99 from 12.3 s to 5.5 s.
- **Other `/suspect`** in the window, not about this repository [measured, the sites' Workers Logs]:
  - one per voter of `authz:1` at 20:55:29Z, during setup;
  - one per voter of the retired `repo:65562` to `65565` at 21:02:30 to 44Z, from one edge isolate.

## Rows m3fr13 (2026-11-17, graft-m3f, `ROWS="b0-32"`, branch `m3fr13`, main `1d3c897b`)

**Slot.** B0-32 was rerun alone, with no site day, because main's `crates`, `workers` and `containers` are unchanged since `fec1d01d` (`git diff --quiet fec1d01d 1d3c897b`). edgever showed `graft-m3f-edge` serving `2c42bfcc` at 100%. The stops were graft-m3f past $5 by `PREFIX`, or the account past $280.
- **Cost:** graft-m3f **$0.34** from 17:50:44Z to 18:02:02Z (row and sweeps), and **$0.40** through the retire and purge at 19:05Z. The account's UTC day read $149.14 at 17:50Z and $158.16 at 19:05Z.
- **Edge errors** (`graft-m3f-edge`, 17:52 to 18:01Z): "Cannot perform I/O on behalf", "closure invoked recursively" and "code had hung" were **0, 0 and 0**.
- **Close-out:** both sweeps found nothing running (17:51Z, 18:01Z and 18:02Z). At 19:03Z, 13 of 13 members of 65583 were retired, and 4,011 objects were purged (a 1,347, b 1,349, c 1,315, all to 0). The final sweep at 19:05Z found nothing running. Idle reading 19:05 to 19:35Z (`PREFIX=graft-m3f acctcost`): graft-m3f **$0.00**, idle. The scheduled reading at 19:38Z did not run; it was taken at 19:58Z over the same window, and the sweep at 19:58Z found no shard container running. The account's UTC day read $162.23 at 19:58Z.

### B0-32 (repo 65583, run `mwkzph1o`, leader site 0): fail, and again not the row as written (no learners)

| Line | Measured | At 90% |
|---|---|---|
| Every burst push acknowledged within 5 s or `ng … admission full` | 498 of 500 ok, 1 HTTP/2 stream error, 1 refused "unpacker error: validate: bad update line"; **0 within 5 s**; client p50 9.6 s, p99 17.0 s | 450 needed: **short by 450** |
| Queued agent bytes under AGENT_QUEUE_MAX | peak 40,175 B, 7 queued | Pass |
| Merges in one of the next 2 entries | not judged from entries; `merge:only` p99 7.65 s (n 15) | Not shown |
| Zero leader changes and zero `suspect` | term 2 throughout, leader site 0; **2 `/suspect` about `repo:65583`** at 17:57:27Z, about 15 s into the burst, each from a different edge isolate (step `Other`), sent to all three voters | **Fail** (the line's target is 0) |

**Server-timing** on the 1,305 timed pushes, in ms, p50 / p99 (m3fr12 in brackets):

| Stage | p50 / p99 |
|---|---|
| auth | 158 / 2,706 (1,147 / 5,747) |
| resolve | 615 / 3,710 |
| gate | 0 / 5,979 (0 / 17,517) |
| context | 324 / 7,443 (1,841 / 20,719) |
| validate | 384 / 1,656 (596 / 1,281) |
| commit | 750 / 2,709 (2,683 / 4,027) |
| total | 3,126 / 12,321 |

The receipt counters were 1,668 commits on 1,668 sends, 0 busy, 9 asks, 0 missed, and 0 receipt `suspect`. The leader's `deps:all` p99 was 479 ms.

**The learners were dropped again, after `learners.mjs` succeeded** [measured, `tests/fanout/raw/rows-m3fr13.status` and site a's Workers Logs]:
- 17:51:59Z: `/learners` committed config 5 with 8 learners. The fixed driver logged it and went on.
- About 17:51:49.6Z to 17:52:10.956Z: the create's background `/place` ran 21,329 ms ("placement of repo:65583", `old` `repo:65583#1#f1`, over 146 ms against the 100 ms bound). It committed `Degraded` at 6 and `Normal` at 7 with a new f1.
- a1's `feed members` step read **0 learners, 2 followers**. The followers ended at 1,456 and 705 feed events, with "lagging" and "gone" answers.

**Why: any voter re-placement drops every learner** [measured from the code, `workers/site/src/member.rs`]. `replace_steps` builds `Degraded { placement: p, .. }` from the placement it read when it started. `try_member` builds the new `Normal` with `Placement::new(voters, ArrayVec::new(), ..)`, so it has no learners. Raft's one-change check (`NotOneChange`) counts voters only, so a change that removes all learners passes. The driver fix on `b032-isolate` cannot cover this order, because the placement starts before `/learners` and commits after it. In production, the daily near check would likely drop a hot group's learners the same way, until the next daily `place_learners` [inference]. Proposed fix (not built): a re-placement carries the learners of the config committed when it changes config, or the change refuses when the committed config index moved since its read.

## B0-31's opening stall (m3fr12, from the raw files and Workers Logs, no hosted spend)

**Clock note.** The orb's clock is not synced. It reads about 4.9 s behind Cloudflare (three `/cdn-cgi/trace` reads at 18:51Z: −5.08, −4.89 and −4.71 s). The admission trace and a1's `buckets-up` cut use the orb's clock. Member traces and Workers Logs use Cloudflare's. So the stall the admission trace put at 16:21:32.7 to 36.7Z is **16:21:36.87 to 42.10Z** in member time.

**What the leader waited on: one dependency read of the actor's `Authz` group** [measured for the times; inference for the link between them]. The trace is the leader `repo:65582#1#f2`'s own rows (`lm1-rows-m3f-data/stall-m3fr12-b0-31.json`).
- At 36.871 the leader applied entry 153, and both followers' appends ended at 36.871 and 36.944. No entry was in flight.
- From 36.871 to 42.099 it queued no AppendEntries and stepped no input that wrote. Edge `/commit`s kept arriving, and the handler's open count rose from 9 to 93. Nothing else held it:
  - `ready` held, and there was no transfer and no term change.
  - Admission's capacity was 650 to 780 a second.
  - No entry was in flight, so `pump` reached `decide_held`, which returns while the held batch's `DepsWanted` is unanswered.
- The leader's isolate read its dependencies about every 230 ms through 37.69. Those were `authz:7100001`, `authz:1`, `tokens:11`, `tokens:482`, `names:24` and `names:369` (object names from `durableObjectsPeriodicGroups`).
- Its read to `authz:7100001#1#f0` (site a, enam), stamped 36.473, did not start at that object until **41.084, 4.6 s later**. That object served it in 8 ms.
- A `/raft` call from the same group's site-b member, stamped 35.641, started there at 41.075, 5.4 s later. Calls from its site-c member arrived on time.
- The lookup's `CALL` (3.5 s) gave up at about 39.97. At 39.973 the read went to `#1#f1` on site b, which answered in 1 ms. At 41.923 it went to f0 again, which answered in 9 ms. Entry 154 was appended at **42.099**, 16 ms after that answer arrived.
- The earlier 1.7 s gap (entry 131 at 30.012 to 132 at 31.708) matches f0 being held then too. f0's alarm ran 1,827 ms, ending at 31.449, and the 34 calls queued behind it all ended at 31.480, with walls of up to 1.75 s.

**What it is.** It is a liveness cost, not a broken rule. `decide` must have the batch's dependency folds read through `fresh`, and read-ahead keeps it so. While the held batch's read is unanswered, nothing behind it can be decided. So one dependency read that waits 4 to 5 s stalls the whole group, as at 16:21:37 to 42. Why calls to `authz:7100001#1#f0` waited 4.6 to 5.4 s before starting is **not known**. The object's own handlers were fast, and calls from its site-c member were not delayed. A narrower lever, not built: ask the dependency read again sooner than `CALL` (for example at about 1 s, against `deps:authz` p99 672 ms). That would cut a stall like this to about 1 s, but it would not lower the floor.

**B0-31 at 90% is not reachable without F81** [measured for the split; inference for the floor]:
- With the stall cut out (edge push log from 16:21:44Z, Cloudflare time, to the half's end), the edge commit p99 is **1,444 ms** (n 1,172). The 90% line is 1.1 s.
- Only 4 apply gaps in the half exceed 600 ms (the two stalls, then 779 and 625 ms). So the steady p99 is the floor itself, not more stalls.
- The floor is a batch's `deps:all` (p99 795 ms, driven by `deps:authz` p99 672 ms to the enam identity groups) plus a round (p99 360 ms), plus the wait for the batch. That is already over 1.1 s before any queueing.
- Removing the stall gives about 1,444 ms. Getting to ≤ 1.1 s needs the `Authz` read off the cross-continent path (F81), or the read hedged and shortened as well.

## F83: the 4.6 s wait at `authz:7100001#1#f0` (m3fr12, Workers Logs, no hosted spend)

**Question** (`decisions/OPEN-QUESTIONS.md`, F83). B0-31's opening stall was one held `Authz` dependency read. Did `authz:7100001#1#f0` (site a, Durable Object `df792e6a`) delay its inbound calls because of its own 1.8 s alarm, or because of a call backlog?

**Neither** [measured; the Workers Logs of all three sites for 16:21:28 to 16:21:46Z, Cloudflare time]:
- **No backlog.** In the 4.6 s the held read waited (sent 36.473, started 41.084), the same object started **215 other calls**. Their wait from send to start was p50 95 ms, p99 457 ms and max 468 ms. No handler ran over 148 ms.
- **No long alarm.** The object's alarms in that window ran 69 ms (37.684) and 57 ms (39.677). The 1,827 ms alarm ended at 31.449, five seconds earlier. It did hold the 34 calls queued behind it (walls up to 1.75 s), and that explains the earlier 1.7 s gap at entries 131 and 132, not this one.
- **Exactly two calls waited.** They were the held read (4,603 ms before start) and site b's `/raft` call 309 (sent 35.641, started 41.075, 5,433 ms). They came from two different isolates: the repository leader on site c, and `authz:7100001`'s follower on site b. They were released 9 ms apart.
- **The sender was not slow.** Site b's next calls 310 to 315, sent 73 to 282 ms after 309, started 75 to 91 ms after they were sent. 135 of site b's later calls started at the object before 309 did.
- **No other site was affected.** In 16:21:36 to 42.5Z, sites b and c held none of their 2,276 member calls past 1.5 s. Site a held only these two of its 2,428.

**What it is** [inference]. The object kept serving every other call within half a second. Two single requests from two senders were held about 5 s each and released together. Their headers carry nothing unusual (`x-graft-hop` 3, as their neighbours). So the delay is below the handler: in delivery of those two requests to the object, not in the object's work. No request-level runtime trace is available to settle it. The two other cases in the kept logs fit the same shape: 1.5 to 5 s holds on single requests while their objects served the rest (16:11:14 to 16:11:21Z, `8b099d95`, `5741543e`, `39d622bc` and `d970720c`).

**What it means for the lever.** The held request is still pending in the runtime while the caller waits, so the faster re-ask the orchestrator set aside would not have queued behind a busy object. It would have opened a second request: the lookup's own retry after `CALL` at 39.973 went to `#1#f1` on site b and was answered in 1 ms. So a dependency read re-asked at about 1 s (against `deps:authz` p99 672 ms) would have cut this stall from 5.2 s to about 1 s. The re-ask stays the orchestrator's call; this only removes the reason it was deferred.

## Rows m3fr12 (2026-11-17, graft-m3f slot, `ROWS="b0-32 b0-31"`, branch `m3fr12`)

> **Annotation, 2026-11-18 (cut-trace).** This run's build (main `fec1d01d`) carries lease-clock (`0b5769b7`), so its
> `/stats` `queue`, `queue:slot` and `queue:post` figures mix two clocks and overstate the wait. Commit p99 and the
> verdicts were on one clock and stand (`tests/m0/results/cut-trace.md`).

**Slot.** The orchestrator moved the slot from 00:00Z Nov 18 to 15:47Z Nov 17. The operator then raised the day's limit to $300 (FRAME.md at `19b5ae77`). Stops: graft-m3f past $10 by `PREFIX`, or the account's UTC day past $280. The earlier $170 line and the $125 check before B0-31 were dropped. LM-1's B0-33 and M4-WP4 ran at the same time on other installs, so only the `PREFIX` figure is this run's.
- **Estimate:** about $5. **Actual:** graft-m3f **$2.19** from 15:47:50Z to 16:55:31Z, covering the site day, both rows and the close-out. The account's UTC day was $111.01 at 15:47Z and $132.47 at 16:55Z.

**Site day:**
- From main `fec1d01d`, 15:47:50Z to 16:02:34Z, image `sha256:0f0c5ee1…` reused (`containers/` unchanged since `68d39927`), standard-4, test-faults.
- Sites exit 0 (versions `65cfbee4`, `ec32e344`, `f7624166`). Edge exit 0: `graft-m3f-edge` serves `2c42bfcc` at 100%, rev `fec1d01d` (`edgever.py`).
- The rows' first start refused ("rows.sh: commit the driver changes first") because `install-m3f.md` was uncommitted; it was committed and the service restarted at 16:04Z.

**Edge errors** (Workers Logs, `graft-m3f-edge`, `/tmp/wlcount.py` in 120 s bins, none capped): "Cannot perform I/O on behalf of a different request", "closure invoked recursively" and "code had hung" were **0, 0 and 0** in B0-32's window (16:06 to 16:14Z) and in B0-31's (16:16:30 to 16:32:30Z). An unfiltered query in the same minutes returns events, so the zeros are real.

### B0-32 (repo 65581, run `mwkvwnmp`, leader site 0): fail on the within-5 s line

| Line | Measured | At 90% |
|---|---|---|
| Every burst push acknowledged within 5 s or `ng … admission full` | 494 of 500 ok, 5 HTTP/2 stream errors, 1 × 503 "every member asked shed the read: overloaded"; **0 within 5 s** (fastest 5.1 s, 144 within 10 s); client p50 14.0 s, p99 28.9 s | 450 needed: **short by 450** |
| Queued agent bytes under AGENT_QUEUE_MAX | peak 136,810 B, 18 queued | Pass |
| Merges in one of the next 2 entries | not judged from entries; `merge_plan` p99 8.4 s (n 16) | Not shown |
| Zero leader changes and zero `suspect` | term 2 throughout, leader site 0; **0 `/suspect`** | **Pass** |

**The receipt rule** (the row's counters over every push): 1,714 commits on 1,714 sends; **0 sends kept open as busy, 22 outcome asks ran, 0 receipts missed, 0 `/suspect`** (m3fr11: 0, 441, 103 and 103). 244 pushes staged first and 0 ran the create probe.

**Server-timing on the 494 acknowledged burst pushes** (ms, p50 / p99; m3fr11 in brackets):

| Stage | p50 / p99 |
|---|---|
| auth | 1,147 / 5,747 (635 / 766) |
| resolve | 1,256 / 3,546 (1,335 / 1,693) |
| install | 389 / 694 |
| gate | 0 / 17,517 (0 / 878) |
| context | 1,841 / 20,719 (374 / 1,208) |
| validate | 596 / 1,281 (802 / **15,759**) |
| commit | 2,683 / 4,027 (3,481 / 8,384) |
| stage | 626 / 1,028 (n 261) |
| total | 11,527 / 25,735 (8,427 / 18,968) |

The client saw 3.1 s more than the server's total at p50 (5.1 s at p99). Only 22 burst pushes finished within 5 s even at the server.

**The per-isolate push log** (1,093 pushes from the burst on, 16:10:55 to 16:13:45Z, all status 200; `lm1-rows-m3f-data/pushlog-m3fr12-b0-32.json`). The edge logs the line as structured fields, so the query filters on the `isolate` key, not on the message text.
- **Isolates:** 124, with 4 pushes each at p50 and 42 at most.
- **`open`:** p50 2, p90 10, max 24.
- **By `open`** (edge total p50 / p99, context p50, auth p50, in ms):

| `open` | n | total | context p50 | auth p50 |
|---|---|---|---|---|
| 1 | 474 | 2,763 / 17,553 | 0 | 92 |
| 2 | 190 | 3,908 / 25,954 | 501 | 136 |
| 3 | 74 | 5,925 / 74,932 | 593 | 446 |
| 4 to 7 | 190 | 10,672 / 25,318 | 5,976 | 1,042 |
| 8 or more | 165 | 13,258 / 25,735 | 6,579 | 2,729 |

- **Reading** (withdrawn: the row ran with no learners, and the correlation is with start time, not `open`; see "m3fr12 after the run" below) [measured, no churn this time]: per-isolate load now tracks the time. A push alone on its isolate takes 2.8 s at p50; one sharing it with four or more takes 10.7 to 13.3 s, and the growth is in `context` and `auth`, the edge's own reads before the leader. m3fr8's inference that the gaps between a push's reads are its isolate's event loop is **supported** on this non-churning row (m3fr9's churning row did not support it).
- **Stage first:** 241 stage-first pushes in the log; 237 came after their isolate had logged a staged push and 4 before. The per-isolate stage-first inference holds.

**Verdict:** the receipt rule and the pool send-back did what they were built for: no `/suspect`, and validate's 15.8 s tail is gone. The within-5 s line is no closer, but this run is not comparable: it ran with no learners, so its `context` and `gate` tail is the two followers overloaded (see "m3fr12 after the run" below).

### m3fr12 after the run: B0-32 ran with no learners, so its reads overloaded the two followers (raw files and Workers Logs, no hosted spend; branch `b032-isolate`)

**The cause** [measured]:
- **No learners.** a1's own `feed members` step reads **0 learners, 2 followers** for B0-32 (`tests/am1/raw/a1-b0-32-mwkvwnmp.jsonl`). m3fr8 to m3fr11's B0-32 and every B0-31 read 8.
- **The race.** The rows' status line for `learners.mjs` ends `config undefined`. Its `/learners` call ran from 16:05:32.7 to 16:05:39.6Z (site a's Workers Logs, wall 6,899 ms). It joined 8 learners, but its config change came from the config it read first. Meanwhile, at 16:05:37.9Z, the create's background placement committed a new f1 (`placement of repo:65581: {"configIndex":6, … "over":{"boundMs":100,"ms":110,"site":1}}`). The change committed nothing, and `learners.mjs` exits 0 on any answer without `error`, so `rows.sh` ran the row anyway. The error text was not logged.
- **So every spread read went to f1 (site b) and f2 (site c).** Their client `/read` rate (10 s bins) was 125 to 145 a second each before the burst, 260 to 345 a second during it, and back to 125 to 140 after. During the burst, 18 to 35% of site c's answers came back in 15 ms or less (gate sheds past READS_MAX 256), against 0 to 3% before and after. Site b let 276 reads run out the edge's FOLLOWER_CALL (cancelled at about 2.08 s).

**How that sets the edge stages** [measured for the stages, inference for the pass count]:
- Each push makes exactly one read of the repository's core rows: in `gate` when its isolate's gate cache misses (168 of the 542 pushes started in the burst's 10 s), else in `context` (374). No push did both.
- Total equals max(auth + install, resolve) plus the later stages, to within 32 ms at p99. Edge CPU per receive-pack was 41 ms at p50 and 154 ms at p99.
- A push's read is write-class (`read_spread_for` with LOOKUP_FOR). Each pass asks f1 and f2. A pass that both shed or time out waits the named retry (250 ms) and runs again, so a read of 6 to 16 s is two to five passes. Reads finishing in the same 100 ms across isolates (74 reads on 38 isolates at 25.6 s, 40 on 29 at 21.2 s) are those passes ending together.
- **Not per-isolate queueing.** Grouped by when the read started, pushes alone on their isolate waited as long as crowded ones: 1.6 s against 6.7 s in second 18, 8.6 s against 7.1 s in second 19, and 11.3 s against 10.3 s in second 21. The by-`open` table above reflects start time, because crowded isolates are the burst's first second. No lock, cap or single lookup in flight orders a push's reads by its isolate; `read_spread` holds no per-chain mark. The m3fr12 reading that per-isolate load is "supported" is withdrawn.

**What this means for the row:**
- m3fr12's B0-32 is **not comparable** with m3fr8 to m3fr11, because it ran without the learners those runs had. Its `context` and `gate` tail (p99 20.7 and 17.5 s) is this run's missing learners, not a product regression.
- Its receipt result stands: 0 `/suspect`, 0 missed, 22 outcome asks. It only got easier with followers this slow, so the next run should confirm it.
- m3fr11, with learners, had `context` p99 1.2 s and `gate` p99 0.9 s. Its within-5 s gap was commit (p50 3.5 s) plus auth, install and resolve (about 2.9 s together).

**Driver fix (branch `b032-isolate`, test scripts only):**
- `learners.mjs` asks again on any error, or on an answer with no `configIndex`, within its 120 s, logs the outcome first, and exits 1 unless a config committed.
- `rows.sh` skips a row whose learners did not place (`LEARNERS NOT PLACED, row skipped`), as it skips one without k 4.

**A product note, no fix proposed:** with learners gone, a 500-push burst roughly doubled the followers' read rate, and the write-class retry turned each shed into a full extra pass on the same two members. The design sends these reads to learners first ("Follower read"), so this is the configuration the row is meant to avoid, not the one it measures.

### B0-31 (repo 65582, rehomed on site 2, warm in 2.2 s, run `mwkwap5z`): fail on the buckets-up line

| Line | Measured | At 90% |
|---|---|---|
| Commit p99 at most 1 s with buckets up | leader **2,673 ms** (n 5,345, p50 790) | Bound 1.1 s: **short by 1,573 ms** |
| Placement failing: Human and MergeQueue p99 at most 3 s, 0 refused | leader commit p99 1,390 ms (n 4,237); `push:human@fail` 86 ok, 1 `stale:race`, 0 refused; `merge:only@fail` 23 × 200, 8 × 409, 0 × 503 | Pass |
| Agent pushes over INLINE_MAX_HOT refused at once in `ng` | `push:agent@fail` 1,252 of 1,252 `full`, p50 1.1 s | Pass |

**read-ahead's flip** (`splitByHalf.up`, ms, p50 / p99; m3fr11 in brackets):

| | m3fr12 | m3fr11 |
|---|---|---|
| cycle (append to append) | **246 / 562** (n 1,114) | 413 per entry (726 entries in 300 s) |
| held at each append | 1 / 2 | (not recorded) |
| round | 230 / 360 | 198 / 373 |
| deps:all | 373 / 800 | 368 / 685 |
| queue:slot | 636 / 2,486 | 531 / 1,701 |
| queue:post | 0 / 297 | |
| entries in the up half | 1,132 (3,888 commands, 3.4 each) | 726 (7.2 each) |

- **The flip does not hold.** The cycle is about one round plus decide (246 against a 230 ms round), not one read plus decide (373 + 45). `read-ahead` did what it was built for [measured].
- **`held` reads 1 at p50, not 2.** The count is taken at each append, after the front batch has left the held queue, so 1 there means two batches were in the pipeline (one in flight, one held). It reached 2 at p99. The planned "held p50 2" was the wrong reading of this instrument.
- **The commit tail got worse anyway.** p99 rose from 1,439 to 2,673 ms while p50 stayed about 790 ms. `queue:slot` p99 rose from 1.7 to 2.5 s, and the shorter cycle carries half the commands per entry.
- **Where the tail sits** [inference, from the per-second admission trace, `tests/fanout/raw/admit-m3fr12-b0-31.jsonl`]: at the load's start (16:21:30Z) the leader held 72 queued commands, 66 of them agent. At 3.4 commands per 246 ms entry that is about 5 s to drain. After that the queue stayed at 2 to 14. The p99 falls within the first 20 s's backlog, about 1% of the half's 5,345 commits. Not checked: the commit times by second, which a1 does not keep per sample.
- `push:agent@up` was 1,208 of 1,249 ok, 5 `full`, and **36 × 503 "no shard could check the pack (site N's shard: Durable Object is overloaded)"**, p50 4.4 s, p99 10.5 s.
- The leader held on site 2 at term 3 before and after; the learners caught up 8 of 8 in 9.7 s.

**Verdict:** B0-31 fails on the buckets-up line alone. read-ahead shortened the cycle as designed, but the p99 is set by the opening backlog, not the cycle.

**After the run: the per-second check of the backlog inference** (no hosted spend; push log `lm1-rows-m3f-data/pushlog-m3fr12-b0-31.json`, 1,230 buckets-up pushes with a commit stage; admission trace `tests/fanout/raw/admit-m3fr12-b0-31.jsonl`):

| Buckets-up pushes started | Edge `commit` p99 |
|---|---|
| all 300 s | 2,151 ms |
| from 20 s on | 1,446 ms |
| from 30 s on | 1,444 ms |

- **The opening seconds carry the spike.** The slowest 1% (2,151 ms or more) all started 7 to 19 s into the load. Every later 20 s bin had p99 1.1 to 1.8 s.
- **It was a commit stall, not admission sizing** [measured]. From 16:21:32.7 to 16:21:36.7Z the leader's round average held at exactly 236 ms (it moves only when an entry applies) while its queue grew 12 → 31 → 46 → 53 → 72. At 16:21:37.7Z the queue was 1. So the leader applied no entry for about 4 s, then everything queued committed together. Capacity stayed 650 to 780 commands a second, far above the load, and `pressure` stayed false. Admission did not refuse or hold anything back.
- **What stalled it is not known.** In the same seconds, all three sites' shards for 65582 were rehydrating or catching up, site c's shard warmed in 4.9 s, and `prsync` logged its first run handled in 6,616 ms (Workers Logs) [inference that these are related; not checked]. The terms trace shows no leader change.
- **Without the stall, the line still fails, and the floor is the dependency read** [inference from the split]. From 20 s on, the edge commit p99 is 1,446 ms, about m3fr11's 1,439. A command's commit includes its batch's `deps:all` read (p99 800 ms) and a round (p99 360 ms), 1,160 ms together, already over the 1.1 s bound before any queueing. read-ahead raised the leader's throughput (cycle 413 → 246 ms) but not a command's latency, which still pays read plus round plus the wait for its batch.
- So **an admission-sizing fix would not move this line**. The levers are the opening stall (cause first) and the cross-continent `deps:authz` read (p99 725 ms, about 2 per entry). The `Authz` read was deferred until fold size is measured, and its placement is a product question (OPEN-QUESTIONS F81).

**Close-out:**
- SWEEP2 OK at 16:34:22Z, and the sweep at 16:55Z found no shard container running.
- Both groups retired: 23 of 23 members "not joined" (`lm1-rows-m3f-data/idle-retire-m3fr12.log`).
- Purge: 10,587 objects deleted in the first pass, which left 88 in site c's bucket for 65582. A second pass deleted 504 more (objects were still landing after the first listing), and a third found 0 in every bucket for both networks.
- graft-m3f was handed to B0-02 as free at 16:56Z; B0-02 ran B0-29 on graft-m2b instead and does not hold graft-m3f.
- Idle reading 16:56 to 17:26Z (`PREFIX=graft-m3f acctcost`): graft-m3f **$0.00**, idle; still $0.00 through 17:47Z, and the sweep at 17:47:36Z found no shard container running. (The scheduled reading at 17:27Z did not run; it was taken at 17:47Z over the same window.)

## Rows m3fr10 and m3fr11 (2026-11-17, graft-m3f slot, `ROWS="b0-32 b0-31"`, run stop $10 on graft-m3f, account stop $170)

> **Annotation, 2026-11-18 (cut-trace).** This run's build (main `68d39927`) carries lease-clock (`0b5769b7`), so its
> `/stats` `queue`, `queue:slot` and `queue:post` figures mix two clocks and overstate the wait. Commit p99 and the
> verdicts were on one clock and stand (`tests/m0/results/cut-trace.md`).

**Slot.** The operator gave graft-m3f a slot on 2026-11-17, with the 90% rule (`FRAME.md`) and the account-wide meter. The run's own figure is acctcost with `PREFIX=graft-m3f` (this branch), because other installs ran in the same window.
- **Estimate:** about $3.10, priced on m3fr9's windows: site day $0.01, B0-32 $1.34, B0-31 $1.68.
- **Actual:** $4.42 on graft-m3f from 03:25:24Z to 05:03:48Z. That covers the site day, m3fr10's two rows, the edge-only day and m3fr11's two rows. The account's UTC day was $60.42 at 05:03Z.

**Site day m3fr10:**
- From main 68d39927 (plus the meter change), with image `sha256:0f0c5ee1…` reused, standard-4 and test-faults.
- Sites deployed with exit 0. The edge `0a4d5f30` served at 100% (`edgever.py`), 03:25:24Z to 03:38:44Z.

### m3fr10: B0-32 void, B0-31 affected, both by `lookup-share`

The edge built from 68d39927 carries `lookup-share` (0a89bdb2). Under each row's load it logged three errors, counted in 20 s steps [measured, Workers Logs, graft-m3f-edge `0a4d5f30`]:

| Window | "Cannot perform I/O on behalf of a different request" | "closure invoked recursively or after being dropped" | canceled as hung (20 s wall, 503) |
|---|---|---|---|
| B0-32, 03:54 to 03:58Z | 1,062 | 730 | 455 |
| B0-31, 04:01 to 04:18Z | 2,740 | 1,278 | 856 |
| before 03:54Z, and m3fr9's whole window | 0 | 0 | 0 |

graft-m2b's edge shows the same errors during LM-1's slot, 03:40 to 03:50Z.

**Likely mechanism** [inference, sent to B0-02]: `Turn::drop` wakes the wakers in the isolate-wide `LOOKING` map. Those wakers belong to other concurrent requests, which may already have ended.

**What it did to the rows:**
- **B0-32** (repo:65577) is void. The driver stopped at 03:54Z, before the burst, when the App token mint (`ghs`) answered 503.
- **B0-31** (repo:65578) is affected, and not judged. Buckets-up leader commit p99 read 1,548 ms (n 4,556).

**Fix:** the operator reverted `lookup-share` on main (0de37416). graft-m3f's edge was redeployed from it alone, edge-only day m3fr11e, no site day: edge `349951e4` at 100%, 04:21:51Z. Both m3fr10 groups were retired (30 of 30 members "not joined" after), and SWEEP2 OK.

### m3fr11 (edge `349951e4`, sites from m3fr10): 0 edge errors in either window

| Window | I/O | closure | hung |
|---|---|---|---|
| B0-32, 04:23:30 to 04:35Z | 0 | 0 | 0 |
| B0-31, 04:35 to 04:56Z | 0 | 0 | 0 |

### B0-32 (repo 65579, run `mwk6v6np`): fail

| Line | Measured | At 90% |
|---|---|---|
| Every burst push acknowledged within 5 s or `ng … admission full` | 499 of 500 ok, 1 HTTP/2 stream error; **1 within 5 s**; client p50 11.6 s, p99 21.5 s | 450 needed: **short by 449** |
| Queued agent bytes under AGENT_QUEUE_MAX | peak 123,353 B, 7 queued | Pass |
| Merges in one of the next 2 entries | not judged from entries; `merge_plan` p99 6.1 s, `shard:waited` p99 5.2 s (n 25) | Not shown |
| Zero leader changes and zero `suspect` | term 2 throughout, leader site 0; **103 `/suspect`** from missed receipts | **Fail** (invariant-shaped zero) |

**Server-timing on the 499 acknowledged burst pushes** (ms, p50 / p99):

| Stage | p50 / p99 (ms) |
|---|---|
| auth | 635 / 766 |
| install | 971 / 1,200 |
| resolve | 1,335 / 1,693 |
| gate | 0 / 878 |
| context | 374 / 1,208 |
| validate | 802 / 15,759 |
| admission | 0 / 0 |
| commit | 3,481 / 8,384 |
| stage | 544 / 823 (n 247) |
| total | 8,427 / 18,968 |

**The row's counters over every push:**
- 1,697 commits on 1,800 sends.
- 0 sends were held open as busy, and 441 outcome asks ran.
- 103 receipts were missed, and each was reported with `/suspect`.
- 258 pushes staged first, and 2 ran the create probe (1,350 timed).

**Reading:**
- The tail is validate (p99 15.8 s) and commit (p50 3.5 s).
- The receipt rule never saw a busy answer. Its outcome asks ran, but 103 receipts still missed and were each reported. So the `/suspect` line fails on the receipt timer, not on a dead member.
- Leader commit p50 393 ms and p99 4,335 ms; queue p99 6.8 s.
- Agent pushes outside the burst: 734 of 749 ok, p50 1.9 s.

### B0-31 (repo 65580, rehomed on site 2, warm in 3.4 s, run `mwk7aypv`): fail on the buckets-up line

| Line | Measured | At 90% |
|---|---|---|
| Commit p99 at most 1 s with buckets up | leader 1,439 ms (n 5,556, p50 792) | Bound 1.1 s: **short by 339 ms** |
| Placement failing: Human and MergeQueue p99 at most 3 s, 0 refused | leader commit p99 1,274 ms (n 4,433); `push:human@fail` 146 ok, 11 stale, 0 refused; `merge:only@fail` 36 × 200, 9 × 409, 0 × 503 | Pass |
| Agent pushes over INLINE_MAX_HOT refused at once in `ng` | `push:agent@fail` 1,248 of 1,248 `full`, p50 1.1 s | Pass |

**Leader and timing:**
- The leader held on site 2 at term 3, before and after.
- With buckets up, `push:agent@up` was 1,249 of 1,252 ok, at p50 3.6 s and p99 5.9 s.
- The leader's queue p99 was 2.0 s, and `queue:slot` p99 1.7 s.

**Verdict:** B0-31 fails on the buckets-up line alone, 30% over its 1.1 s bound. Its best run is still m3fr2 at 1,118 ms.

**Close-out:**
- Both m3fr11 groups were retired: 22 of 22 members "not joined" after (`tests/fanout/raw/idle-retire-m3fr11.log`).
- The sweep at 05:03:48Z found no shard container running.
- Idle reading, 05:04 to 05:34Z (`PREFIX=graft-m3f acctcost`): graft-m3f **$0.00**, idle. The sweep at 05:41Z again found no shard container running. The account's top writers in that window were graft-m1's sites ($10.0 of $11.62).

### m3fr11 after the run: B0-32's validate tail and its 103 `/suspect` (Workers Logs and the r11 raw files, no hosted spend; branch `b032-trace`)

**The validate p99 is ten pushes on two pool shards** [measured, site a's `GitShard` events 04:31:20 to 04:32:10Z]:
- 489 of 499 acknowledged burst pushes validated in at most 1,317 ms. The other 10 took 15,636 to 16,028 ms, and nothing lies between.
- **Where the 10 waited.**
  - 9 were forwarded to `git#0#pool:4` (`d442dd98`) and 1 to `pool:2` (`91056c0f`). They arrived at 04:31:43.5 to :44.5, the burst's peak second, when the network shard took 273 validates in 0.5 s.
  - Each pool shard was then in a catch-up of the burst's own packs: `pool:4` from 04:31:43.2, 141 packs, 11.2 s; `pool:2` from :43.1, then 282 packs, 13.2 s.
  - A push that needs a catch-up waits for the run after the running one (`wait_from`). So these waited out the edge's `wait=15` and answered 503 `warming` at 15.2 to 15.7 s. The 503 was not `pool_cold`: no 503 came at once.
- **The first shard then validated each** with `via=0` in 131 to 244 ms, and logged "pool:4 is warming" 9 times and "pool:2" once.
- **The network shard's own burst validates** ended within 1.1 s, except one at 6.4 s. The pool shards' others ended at p50 0.4 to 0.9 s.
- **Not settled from the logs:** whether each of the 10 was a 409 `missingBase` on the disk as it stands, or a push that caught up first. The container logs no 409s. Either path waits in `wait_from`, and the fix covers both.

**The 103 `/suspect` were deadlines on a slow leader, not a gone one** [measured, the edge's per-push log line (1,063 pushes, 95 isolates) and the leader's `Member` events]:
- **When and where.** All 71 pushes with a miss started at 04:31:41.4 to :45.7, the burst's first 5 s, from 13 isolates. 103 misses in all; 11 of the pushes missed three times. Term 2 held throughout, with leader site 0 (`dbde0f0f`).
- **Why `busy` was false.** At each missed deadline, the isolate's last 200 from the leader, from a push of its own that had ended, was 2,083 ms old at p10, 2,524 at p50 and 3,941 at most. It was never within `COMMIT_RECEIPT`. Asks that did run (`busy` true) had a last 200 at p50 1,608 ms. Sends answered in time had one at 1,391 ms.
- **Why no 200 came in time.**
  - Leader `/commit` handlers that started at 04:31:43 to :45 ran p50 3,114 ms and up to 4,645 ms: 353 of 521 over 2 s, against a 1 s `COMMIT_HOLD`.
  - `/outcome_by_request` on the same leader in the same 10 s: p50 164 ms, p90 608, at most 625.
  - The leader's `queue:slot` p99 was 4.5 s. Its CPU was 0.2 to 0.7 s a second, so the queue is entries waiting their turn, not a busy isolate.
- **What the misses cost.** Pushes with a miss committed in p50 5,066 ms, against 5,097 ms without. The resends did not slow commits. They cost one more 256 KiB send each and 103 `suspect` waves to the followers, which is what fails the row's zero-`suspect` line.
- **The staged first commit** (the small command after a `Stage`) took p50 4,872 ms. The inline 256 KiB first commit that drew `Stage` took 853 ms. The slow part is the queue the staged commands join, not the pack's bytes.

**B0-31's buckets-up tail and B0-32's commit tail share their place, not shown their cause** [measured, `leader.split`]:
- Both are in the leader's queue slot, with the round about flat:
  - B0-31 buckets-up: commit p99 1,439 ms, `queue:slot` p99 1,701, round p99 373.
  - B0-32: leader commit p99 4,335, `queue:slot` p99 4,486, round p99 280.
- B0-32's slot wait comes from 500 pushes in one second, each two commands. B0-31's comes from 1 MiB agent packs at a steady rate, with `queue:post` p99 470 ms against B0-32's 2,265.
- **Same place, not shown to be the same cause.** Neither fix here shortens the slot. The receipt fix removes B0-32's resends and suspects, and the pool send-back removes its validate tail. B0-31's 339 ms over its 1.1 s bound is left as it was.

**Proposal, built on `b032-trace`** (`decisions/DECISIONS.md`, "`burst-receipts`"):
1. **Ask before reporting.** At each `COMMIT_RECEIPT`, the edge asks `/outcome_by_request` within `CALL_DEADLINE` whether or not the member is `busy`, and reports only a member that did not answer it with a 200 and is not `busy` either. In m3fr11's trace that keeps all 103 sends open: no `suspect`, no resend.
2. **Pool shards send back.** A pool shard sends back, with 409, a forwarded push that needs a catch-up, and the first shard validates it. In the trace that is the 10 pushes at about 0.2 s instead of 15.6 s, so validate p99 about 1.3 s.
3. **Expected B0-32 effect** [inference]. The zero-`suspect` line should pass if the leader holds. The within-5 s line still fails: commit p50 3.5 s and auth, install and resolve about 2.9 s together already total over 5 s. That needs the leader's queue (LM-1 and B0-02's work) and the edge's read stages.

### B0-31's buckets-up commit tail: the apac leader's cycle is its dependency reads, not its round (raw files of 8 B0-31 runs, no hosted spend; branch `b031-slot`)

**The cycle** [measured, `leader.splitByHalf.up` of `a1-b0-31-mwk7aypv`, m3fr11]:
- 726 entries in the 300 s half is one entry per 413 ms, carrying 7.2 commands each.
- Round p50 198 ms (p99 373). The entry's dependency reads (`deps:all`, every chain at once) p50 368 ms (p99 685).
- `cut_early` asks the next batch's reads once the entry in flight has been out its mean round less one mean read. That wait is 0 here, since the read is longer than the round. The batch is then decided when its reads land, about 368 ms after the append. That plus about 45 ms to decide and append is the 413 ms cycle.
- So the round is hidden and the read is not. `queue:slot` p50 531 ms is about 1.3 cycles, and p99 1,701 about 4 cycles.

**Why the reads are long** [inference, from the record and the read split]:
- The repository group is rehomed onto apac (site 2) for the row. The owner's identity groups (Authz, Tokens, Names) follow the owner's record, `Tri`, home site 0 (enam), so each read crosses the Pacific.
- `deps:tokens` and `deps:names` p50 174 ms, about one round trip. `deps:authz` p50 293 ms, with about 2 Authz reads per entry (1,437 for 721 entries), so `deps:all` takes the slower of two.
- B0-32's leader on enam reads the same groups at `deps:all` p50 101 ms.
- Not checked: which site led each identity group during the row. The terms trace records only the repository group.

**Across the B0-31 runs that held their leader**, commit p99 follows the read (each `deps:all` p50, commit p99):
- `mwg9hh9a`: 356 ms, 1,118 ms
- `mwfyd102`: 357 ms, 1,165 ms
- `mwk7aypv`: 368 ms, 1,439 ms
- `mwglf3oa`: 461 ms, 1,333 ms
- `mwk61jkm`: 488 ms, 1,548 ms
- `mwj4z7uv`: 485 ms, 2,138 ms

Rounds stayed between 194 and 254 ms p50 in all of them.

**So B0-31 and B0-32 do not share a cause.** B0-32's slot wait is queue depth behind a one-second burst. B0-31's is a cycle held to the dependency read.

**Where a fix would go** (not built):
1. **Overlap more than one batch's reads** (`crates/raft/src/member/leader.rs`, `cut_early` and the one held batch). Cut and ask the next batch's reads while one batch is held, so the cycle tends to the round plus decide (about 245 ms) instead of the read plus decide (413 ms). Each read is still asked after its commands were admitted, and one entry stays in flight. This is `crates/raft`, which this thread does not edit.
2. **A cheaper Authz read** on the identity leader (`workers/site/src/host.rs`, `{"q": {"dep": true}}` returns the whole fold, hex-encoded). It removes at most the roughly 120 ms Authz takes over Tokens, and only if fold size is the reason, which is unmeasured.
3. **Placement:** the row rehomes only the repository on purpose, so this is a product question: should a group whose leader moves read its dependencies from groups led on another continent?

## The daily near check re-placed healthy groups (2026-11-16, Workers Logs, no hosted spend; branch `near-check`)

**What the logs show** [measured, Workers Logs, "daily near check re-placed" on every site of each install, 00:00Z to 23:59Z]:
- The daily near check re-placed 117 groups on six installs: graft-m2b 72, graft-m1 12, graft-m3e 12, graft-m4a 9, graft-m3f 7, graft-m3c 5. That is 81 distinct repositories.
- Every one was on site a, an enam home, re-placing its weur follower (site 1) against a 100 ms bound.
- The readings were 101 to 148 ms: p50 112, p90 131, max 148. 26 were over 125 ms, 12 over 130, and none over 150.
- 115 of the 117 ended 0 to 8 minutes after a 15-minute cron run (minute mod 15: 0 to 8).

**One case, traced.** graft-m3f repo:65549 was re-placed at 19:04:37Z on a 109 ms reading (`configIndex` 3739, 9.3 s).
- In 18:55 to 19:04Z, neither edge (`graft-m3f-edge`, `graft-m3f-tools-edge`) logged a single request.
- In 19:00 to 19:04Z, site a's members took 59 `POST /fold` and 37 `POST /read` calls, the cron's fold audit and its lookups, plus the check's own `/ping`s.

**Why** [code]:
- `Host::served` is called on every leader `/read`, `/record` and `/commit`, and on a dozen family routes. The cron's calls are among them.
- `served` keeps a leader's alarm running for DRAIN_IDLE_MS, and every alarm runs `recheck`.
- `recheck` measures once its rows are a day old, and re-places on the second reading over `NEAR_RTT` a day after the first.
- Placement aims for 100 ms on an install whose enam to weur pair sits around 105 to 136 ms (`tests/am1/results/fresh-repo.md`). So a group that has been served once is re-placed about every other day, by the cron alone.
- Each re-placement joins up to 8 new members, and each replays the log. It keeps the leader alarm at 2 s for minutes. These DO rows are written on idle groups.

**Built (branch `near-check`; `decisions/DECISIONS.md`, "`near-check`"):**
- The check starts only within 60 s of a call an edge made for a client (`CLIENT_CALL`).
- It moves a member only on two readings a day apart over `NEAR_RECHECK`: 150 ms for enam and weur, 297 ms for apac. That is half again over `NEAR_RTT`, past every reading above and past B0-12's p99 pairs (135 and 262 ms).
- Under that rule, none of the 117 would have moved, since none read over 150 ms.

**The fold audit's 404s, same day:**
- graft-m3f site c's only homed chain is `repo:0`, the CLI latency probe's (`repo:0#1794539306#f0`). It answered 404 on every 15-minute run, as did site a once a cycle.
- graft-m1 site a's 404s are test-register chains (repo:3229002 to 3229005 at 22:04Z). A register member has no `/fold` route.
- Each 404 cost about 21 `/fold` calls, because the lookup asked again for LOOKUP_FOR.
- The cursor does move on, as B0-30 corrected in `tests/am1/results/b030-split.md`.
- The audit now drops `repo:0` and skips a member that answers 404 after one call. B0-30's thread was told and is not editing `fold_audit.rs`.

## Rows m3fr9 (2026-11-16, TAG `m3fr9`, `ROWS="l0-12 b0-32 b0-31"`, `STOP_USD=77`)

> **Annotation, 2026-11-18 (cut-trace).** This run's build (`lm1-rows-m3f10` on main `a0bca583`) carries lease-clock (`0b5769b7`), so its
> `/stats` `queue`, `queue:slot` and `queue:post` figures mix two clocks and overstate the wait. Commit p99 and the
> verdicts were on one clock and stand (`tests/m0/results/cut-trace.md`).

**Build and site day.**
- **Build:** branch `lm1-rows-m3f10` `48c67a34`. It is main `a0bca583` (`receipt-rule`, `a1-phase-sent`, `m3fr8-commit-trace`) plus one test-faults instrument. The instrument adds one `server-timing` entry per `/commit` send (`send`), per receipt missed while the member was `busy` (`busy`), per `/outcome_by_request` ask while a send was open (`ask`), per other missed receipt (`missed`), and per `suspect` report (`suspect`). `a1.mjs` sums them (`pushTiming`).
- **Image:** `sha256:83e217ba…`, reused. `containers/git` is unchanged since `09a945cd`.
- **Site day:** 14:45:16Z to 14:57:25Z, site and edge both exit 0. Sites `f72bbf56` / `e58afba4` / `0b752f19`, edge `87f2a7df`. An earlier 14:35Z start from main was stopped while it compiled, to add the instrument. Nothing was deployed by it.
- **Checks:** `m3org/j4` answered 200 to admin and member. `repo:65536` is led by site 0 at term 4. Each voter's lagMax was 142 / 0 / 0 ms, with 0 lease fallbacks.

**Teardown.**
- The start sweep found no shard container.
- Each row's shards were torn down after it: 65574 at 16:02:05Z, 65575 at 16:29:54Z, 65576 at 16:51:02Z.
- The end sweep, `SWEEP2`, and a third sweep at 16:55Z all reported "no shard container running".

**Spend (graft-m3f, account Graftyard, `env-m3f.sh`).** $62.76 at the start. $65.35 at B0-32's start, $66.66 at B0-31's start, and **$68.31** at the end. The stop was 77.
- L0-12 cost $2.59 (it stopped early).
- B0-32 cost $1.31.
- B0-31 cost $1.65.

### L0-12 (repo 65574, run `mwje8g89`): fail; stall and refusals did not run

The group reached k 4 at 15:03:31Z. Creating the 540 tasks took until 15:22:00Z, and the learners caught up (8 of 8) at 15:38:36Z. The replay ran 600 s from 15:38:41Z: 500 agents with a lease each, 500 feed long-pollers, and a leader kill every 60 s.

| Gate line | Measured | Verdict |
|---|---|---|
| No two holders overlap Exclusive leases for longer than `DRIFT` (500 ms) | 172 samples, 0 over 0 ms, max −1,112 ms; the bound with the clock spread is −773 ms | Pass (replay part) |
| Measured member clock offsets under 500 ms | 117 kept samples of 120: the members' clocks were 102 to 339 ms apart (`lowerMs` / `upperMs`) | Pass on the replay. The rest-state samples before and after the stall were not taken, because the stall did not run |
| Every lease live before the stall live after it, `expires` moved by the stall | Not run | **Not run** |
| The late Exclusive acquire succeeds | Not run | **Not run** |
| 500 long-pollers see each event within 3 s at p99 outside leader kills | `feed.outsideKills`: 19,760 events, 10 kills, p50 1,106 ms, **p99 25,210 ms** | **Fail** |
| 100 of 100 refused for each Agent kind, 0 of 100 for the other three | Not run | **Not run** |

**Why the stall and refusals did not run.**
- `costok` passed both phases, at $65.23 and $65.28.
- Right after the replay, the edge answered 503 for the stall's first task (15:53:30Z): "the repository's tier rows did not answer; retry (every member asked shed the read: overloaded)". `stall.mjs` exits on a refused task create, as fixed in rows m22.
- `rows.mjs` then got the same 503 on `PUT …/agents/policy` `agents_exclusive`, so the policy stayed `advisory` (read back 200 at 16:01Z). With an advisory policy, no refusal kind measures anything, so I stopped `rows.mjs` at 16:01:59Z after its holder lease (201).
- At 16:01Z the leader's `/stats` showed 5 busy pages and site 2 showed 46. Its counted memory high-water was 48.9 MB wasm on site 0, with 6.7 MB of log.
- Under the parent's ruling (no partial L0-12 rerun), the stall and refusals wait for the next whole run.

**Feed: where the slow 1% came from.**
- The 198 slowest events outside kills came from site 2's learners (130) and site 0's (68).
- 154 of them carry `{"429": 1, "gone": 1}`: the poller was turned away with 429, then answered `gone` and reloaded from `oldest`. The other 44 carry no miss and took 25.8 to 37.7 s, which is a whole hold with no answer.
- By minute from the replay's start: 53, 14, 40, 9, 37, 0, 0, 16, 13, 16. So it is not only the first minute.
- `gone` answers totalled 869 over the row. 858 of them came from site 2's learners `l2` (626) and `l5` (232), against 4 or fewer on every other learner. The feed-ahead change (`lagging` for a cursor past `newest`) left those 858.
- **Reading** [inference]: site 2's two learners fell behind further than `FEED_RING`, and their cursors aged out. The fix would be in learner catch-up or ring size, not in the cursor rule. The per-learner `lagging` counts (2,649 and 3,809 on `l5` and `l2`, against 127 to 928 elsewhere) agree. Corrected after the run: the trace shows they did not fall behind. See "m3fr9 after the run" below.

**Replay, other lines.** Leader commit p50 567 ms, p99 813 ms (n 37, the end read after the last kill). Agent pushes: 2,491 of 2,491 ok, p50 2,330 ms, p99 11,896 ms. B0-29's API read: p99 3,347 ms, with 3,108 × 503 and 2,847 skipped at the driver's cap, of 118,788.

**Comparison with m3fr6's L0-12** (the replay windows, 600 s each):

| | m3fr6 (18:17:14Z to 18:27:14Z) | m3fr9 (15:38:41Z to 15:48:41Z) |
|---|---|---|
| Term range | 2 to 32, churn from the learners' catch-up on | **2 to 2**: site 0 led throughout, with no election |
| Step-downs (Workers Logs `member alarm: StepDown`) | 16: 13 CommitErrors (site a 6, b 6, c 1), 3 Confirm (site a) | **0** on any site, 15:03Z to 15:54Z |
| lagMax / lease fallbacks per voter | 5,687 ms / 11,543; 4,300 / 10,296; 163 / 0 | **3,899 ms / 746**; 190 / 0; 384 / 0 (max over the window, `vmem` `clock`) |
| Read rounds per voter per 20 ms (`tests/fanout/rounds.py`, `--gap 20`) | max 99 / 61 / 84 (the first `rounds.py`, uncommitted) | site 0 (the only leader): 9,905 windows, max 18, p99 7, 200 windows over 5. Sites 1 and 2 never led |
| Kills | 10 kill pushes, 9 ok and 1 × 503; times not kept | 10 of 10 ok (round trip 2.9 to 4.7 s). Each reset site 0, which came back as leader at term 2 within the kill window. Sent 15:39:39Z, 15:40:40Z, 15:41:39Z, 15:42:40Z, 15:43:39Z, 15:44:40Z, 15:45:39Z, 15:46:40Z, 15:47:39Z, 15:48:40Z. The 9th recorded no reset |
| Feed `outsideKills` p99 | 2,954 ms (pass) | 25,210 ms (fail) |
| Leader commit p50 / p99 | 2,258 / 17,312 ms | 567 / 813 ms (end read) |

- Site 0's 746 lease fallbacks and 3.9 s lagMax are the leader's. Each kill reset it, so its clock restarts behind.
- With no churn, the feed's tail moved to the learners' cursor ageing. m3fr6's pass came with constant elections that kept the feed answered by followers [inference].

### m3fr9 after the run: why site 2's learners answered `gone`, and the read the stall's task was shed on (analysis from the raw files and Workers Logs, no hosted spend)

**The learners did not fall behind on catch-up.** The reading above is wrong.
- The trace rows (`tsave-m3fr9-l0-12.jsonl`, each learner's `Applied` and `AppendStepped` rows) put all 8 learners within about 20 entries of the leader at every minute of the replay. Learners reset only at 15:04:12Z and after the replay, so no ring restarted under it.
- Site 2's learners (`l2` and `l5`, the same apac site as m3fr7's two learners, on a new repository) apply each entry later. Their apply delay behind the leader was p50 1,008 and 717 ms, p99 4,074 and 1,971 ms. Site 0's learners were at p50 181 to 316 ms. Of the entries applied in the replay, `l2` applied 426 of 844 more than FEED_BATCH after the leader, and `l5` 209 of 1,010. Site 0's learners applied about 32 to 36 that late.
- **Mechanism** [the code path is read from the code; tying it to the counts is an inference]. `a1` and the edge route a poller across the learners in rotation, so a cursor one learner served goes next to another. `feed::start` answers `gone` to a cursor past the commit index the member knows (`feed-ahead`'s bound). The next learner's newest entry was past `l2`'s known commit 84.2% of the replay, and past `l5`'s 82.5%. For the other six learners the figure was 4.7 to 23.1%.
- The edge, and `a1` mirroring it, returned the first member's `gone` at once. The poller reloaded from `oldest` and was sent events again that it had seen, which the driver timed as up to the whole replay. This also fits B0-32 (139 and 367 `gone`) and B0-31's 0.
- **Fix** (branch `learner-lag`, `decisions/DECISIONS.md`, "`feed-gone-route`"): one member's `gone` sends the poll on, and the edge answers 410 only when every member it tried did. The restore bound is unchanged. `a1.mjs` routes the same way. FEED_RING is not touched.

**The read the stall's task was shed on.**
- At 15:53:30Z, `POST …/tasks` (wall 7.3 s) answered 503 "the repository's tier rows did not answer; retry (repo:65574: every member asked shed the read: overloaded)". The `PUT …/agents/policy` after it got the same.
- The read is `api::tier_rows`: the whole-tier dump `{"rows": true, "ranges": true, "packed": true}`, sent through `spread_read` to the learners and then the followers [measured, Workers Logs].
- `agents::serve` (`workers/edge/src/agents.rs`), which serves policy, tasks and leases, never sets `Ctx::write`. So `spread_read` runs these writes with a retry of 0, as a read-class request. DESIGN, "Follower read", says a REST call other than `GET` asks the same members again within LOOKUP_FOR (20 s). The REST and GraphQL paths set it (`rest_repo`, the GraphQL mutation); the agents routes do not [read from the code].
- The group was idle: the leader had 0 reads in flight, and the learners had applied 3,564 to 3,613 against the leader's 3,613. The leader's appends to learners were sparse after the replay: it applied entry 3,613 at 15:53:08Z and 3,614 at 15:53:59Z.
- Why every member asked shed this one dump is not settled [unverified]. Two causes fit the code: a voter that had not finished rebuilding its dump (`waits_for_rebuild`), or a learner that had not applied the read index within FOLLOWER_WAIT.
- **Built (branch `agents-write`):** `agents::serve` sets `c.write` for a non-`GET`, as `rest_repo` does. A shed tier read then waits within LOOKUP_FOR instead of failing the write.

**Why every member shed it, from the request's trace** (`933ed636`, Workers Logs, every event of the 7.3 s task create; no bodies are logged, so what each member answered is read from the code and the timings).
- The tier read went to three objects, one after another: `1396e389` (site a, voter `#1#f0` and the leader) from 15:53:28.26 to 28.95, `f6be03d0` (site b) from 30.03 to 30.43, and `865d3a37` (site c, voter `#1#f2`) from 30.43 to 31.64. The edge answered 503 at 32.06. Every `/read` returned HTTP 200, so each was a shed body (`overloaded`), not a cap or an error status.
- **No learner was asked**, and the leader was. That is `timed_spread_order` with no config known to the edge isolate: no learners, no known leader, and the voters from `record.bootstrap_members`, the first generation's names [read from the code].
- **One of the three is a removed voter** [inference]. Site b's voter was replaced at config 3 (`repo:65574#2#f1`, `d61a05a0`, which `vmem` polls by name). `f6be03d0` takes only the drivers' unnamed `/status` calls, about 100 against 139 to 141 on the current voters. That is the first generation's `#1#f1`, which `statuses` reaches by site alone. What a removed member answers a read-index read is not settled.
- **The leader's read took a quorum round:** `/raft` to site b's and site c's voters on the same trace, ending 28.41 and 28.51, inside its 685 ms. Its lease was not usable (7 lease fallbacks, `lagMax` 666 ms in `vmem`). A voter sheds a whole-tier dump whose rebuild is not due (`waits_for_rebuild` is false for a voter's dump), and answers `overloaded` past FOLLOWER_WAIT. Which one each of the three hit is **not known**.
- Site c's 1,210 ms is past FOLLOWER_WAIT, which fits a late read index. The leader's 685 ms and site b's 399 ms do not.
- **What would settle it:** a test-faults log line on each shed that names the branch (`gate` cap, rebuild not due, FOLLOWER_WAIT, removed member), and the read's `q` kind. Not built.
- **A second finding:** a cold edge isolate reads a group as if it had no learners, asks the leader by read-index, and can ask a removed voter. It learns the config only from a usable answer's `meta` or a `notLeader`, so an isolate whose three members all shed learns nothing [read from the code]. Not built; it needs a design decision on how a cold isolate learns a group's learners before its first spread read.
- **Driver change, built:** `rows.sh` now waits up to 120 s, before the stall, for the replay's requests to drain (`tests/fanout/drain.mjs`: no held feed poll, no read in flight, nothing in admission, on every member). There is no retry: `stall.mjs` and `rows.mjs` record a refused task, policy or holder as a result and go on.

### B0-32 (repo 65575, run `mwjgfcsw`): fail, with leader churn

The group reached k 4 at 16:04:52Z, with the learners caught up 0 of 8 after 122 s. The load started at 16:10:20Z. The burst warm-up saw `hot` and warm after 347 s, and the burst fired at 16:16:08Z.

| Gate line | Result |
|---|---|
| Every push acknowledged within 5 s, or answered `ng … agent admission full` | **Fail**. 391 ok, **0 within 5 s**: p50 46.3 s, p99 91.3 s, min 8.4 s. The other 109: 73 × 503 "access rows did not answer (not leader)", 28 × 502 (Cloudflare HTML), 4 × client `fetch failed` (`ERR_HTTP2_STREAM_ERROR`, about 4.6 s, acknowledgment unknown), 3 × 500 "no outcome: missed", 1 × 500 "no outcome: not leader" |
| Queued agent bytes under `AGENT_QUEUE_MAX` (64 MiB) | Pass: max 532,391 B (the `admit.mjs` trace) |
| Every merge in one of the next 2 entries | Not judged from entries. `merge:only`: 23 × 200, 3 × 409, **4 × 503 `warming`** (the git cache), 1 × 503 tier rows; p50 4.3 s, p99 28.9 s |
| Zero leader changes | **Fail**: 17 elections, term 2 to 19, 16:13:09Z to 16:18:29Z |
| Zero `suspect` reports | **Fail**: 7,135 inbound `/suspect` on the final leader; the edge reported 8,564 over the row |

**The churn.**
- Site 0 held term 2 until 16:13:09Z, three minutes into the warm-up.
- Then 17 elections in 5 minutes 20 s, won by site 0 six times, site 1 seven, and site 2 four. A new leader often answered `?` (reset or unreachable) for 1 to 6 s.
- Site 0 held term 19 from 16:18:29Z to the end.
- Workers Logs have 10 `StepDown`s: 7 `CommitErrors` (site a 2, b 4, c 1) and 3 `Confirm` (site a: 16:12:35, 16:13:54, 16:16:31Z). The other 7 term changes followed a leader reset or silence, not a step-down.
- Final leader peers: 1,085 and 1,289 `deadline` failures to sites 1 and 2, against about 5,700 ok each.
- Leader split: queue p50 3.1 s, p99 9.0 s; append p99 about 2 s; confirm p99 1.7 s; `deps:authz` p99 7.3 s.
- The trace went to LM-1 ([thread](https://ampcode.com/threads/T-01a1c694-6112-72b4-83ca-dc9337a9646e)). `crates/raft` is not touched here.

**The receipt rule, counted** [measured, `server-timing` of all 2,409 timed pushes, and the edge's per-push log, 1,727 pushes in Workers Logs]:

| Count | Row total |
|---|---|
| `/commit` sends | 15,584 over 3,668 commits |
| Missed receipts kept open as `busy` (no resend at once, no report) | 2,488 |
| `/outcome_by_request` asks while a send was open | 1,156 |
| Other missed receipts (reported) | 8,561 |
| `suspect` reports | 8,564 |

By minute, from the edge log:

| Minute | Pushes | Sends | Busy | Asks | Missed | Suspect |
|---|---|---|---|---|---|---|
| 16:10 | 178 | 196 | 0 | 0 | 0 | 0 |
| 16:11 | 272 | 368 | 0 | 22 | 29 | 29 |
| 16:12 | 237 | 380 | 0 | 32 | 54 | 54 |
| 16:13 | 263 | 522 | 0 | 37 | 125 | 125 |
| 16:14 | 252 | 571 | 0 | 94 | 149 | 149 |
| 16:15 | 338 | 1,029 | 29 | 15 | 478 | 478 |
| 16:16 | 325 | 3,863 | 1,126 | 396 | 2,003 | 2,005 |
| 16:17 | 345 | 7,569 | 1,224 | 466 | 5,283 | 5,285 |
| 16:18 | 215 | 1,281 | 189 | 93 | 525 | 525 |

- **What it shows:**
  - Before the churn (16:10 to 16:12Z, term 2), the rule worked as designed. 0 receipts were missed in the first minute. In the next two minutes, 83 were missed and reported, with 54 asks.
  - 0 `busy` keep-opens came before 16:15Z. Every early miss came from a member that this isolate had not heard from within COMMIT_RECEIPT. That case is still reported by design.
  - From 16:13Z the leader changed every 3 to 30 s. A member that is no longer the current term's leader is not `busy`, so its miss is reported. Each report is a `suspect` wave plus a resend, and the burst's 8,000 suspects came in the churn's two worst minutes.
- **Reading** [inference]: in this run the suspects follow the churn, and they don't cause it. The first `Confirm` step-down (16:12:35Z) came when the edge had sent only 54 suspects in that minute. But each suspect wave to the voters adds to their inbound, and the 7,135 inbound `/suspect` on the final leader exceeds every other route except `/raft` and `/read`. So the rule's effect is not isolated here. It needs a B0-32 that holds its leader, or a churn fix first.

**Per-push stages** (`server-timing`, all 2,409 timed pushes, ms):

| Stage | n | p50 | p99 |
|---|---|---|---|
| auth | 2,409 | 83 | 952 |
| resolve | 2,409 | 524 | 2,204 |
| install | 2,409 | 371 | 990 |
| gate | 2,409 | 990 | 24,376 |
| context | 2,409 | 0 | 24,681 |
| validate | 2,409 | 270 | 15,697 |
| admission | 2,409 | 0 | 0 |
| probe | 463 | 10,935 | 53,341 |
| commit | 3,668 | 2,553 | 56,257 |
| stage | 1,259 | 536 | 853 |
| edge total | 2,409 | 11,241 | 86,627 |

- **Create probe:** 463 of 3,668 commits probed, and 1,946 of 2,409 pushes skipped it. The probes are the slow ones (p50 10.9 s): a probe runs after `NotLeader` or a stale term drops the isolate's leader, which the churn did every few seconds.
- **Stage first:** 63 pushes staged before their first commit, against m3fr8's 253. 1,259 commits staged after a `Stage` reply.
- **`gate` and `context` at p99 24 s** are the read gate's member reads during elections: "access rows did not answer (not leader)".

**The per-isolate push log** (842 pushes from the burst on, from Workers Logs; the edge's `{push, isolate, open, status, timing}` line):
- **Isolates:** 99 served the burst, with 6 pushes each at p50 and 39 at most.
- **`open`** (pushes the isolate held when one started, itself included): p50 3, p90 11, max 26.
- **Edge total by `open`:** 33.8 s at open 1, 36.4 s at 2, 33.9 s at 3, 44.2 s at 4, and 37.7 s at 8 or more. Load per isolate adds little against the churn's commit wait. So the reading that the gaps between a push's reads were its isolate's event loop is **not supported** here [measured, on a churning row].
- **Stage first and the isolate's own `Stage`:** of the 54 stage-first pushes in the log, 9 came before their isolate had logged any staged push, and 45 after. This mostly confirms the m3fr8 inference: an isolate stages first only after its own first `Stage` reply. The 9 exceptions are isolates whose first `Stage` reply came on a push that ended after them [inference: the log line is written at the end of a push].

### B0-31 (repo 65576, rehomed on site 2, run `mwjhg50v`): fail on the buckets-up line

The rehome was on site 2 (`transfer.mjs`). Site c's shard answered warm 3.5 s later, before the merges (`shardwarm.mjs`). The group reached k 4 at 16:33:28Z, and the learners caught up 0 of 8 after 121 s. The load ran 600 s from 16:40:05Z, with placement failing from 300 s.

| Gate line | Result |
|---|---|
| Commit p99 at most 1 s with buckets up | **Fail**: 9,600 ms (p50 1,054, max 11,401, n 5,366). m3fr8: 2,138 ms |
| Placement failing: Human and MergeQueue commit p99 at most 3 s, 0 of them refused | Pass: leader commit p99 1,638 ms (n 4,267). `push:human@fail` 161 ok, 9 stale, 0 refused. `merge:only@fail` 29 × 200, 2 × 409, 0 × 503 |
| Agent pushes over INLINE_MAX_HOT refused at once in the `ng` shape | **Pass**: 1,258 of 1,258 `full` (p50 1,125 ms, p99 3,258 ms), 0 committed. a1-phase-sent labels each push by the phase it was sent in, and m3fr8's 17 are gone |

- **The apac leader held:** term 3 from the rehome (16:32:26Z) to the end, with 24 failed calls and 0 `StepDown`s in Workers Logs.
- **MergeQueue `warming`:** 0 × 503 on either half. `merge:only@up` 28 × 200, 11 × 409; `merge:only@fail` 29 × 200, 2 × 409. The two `warming` lines in Workers Logs (16:40:15Z) are pool shards `git#0#pool:4` and `:7` before the first pushes, which validated on the home shard.
- **The buckets-up p99 tail:** `push:agent@up` p99 15.3 s, `push:human@up` p99 14.4 s, and `merge@up` p99 36.9 s. The leader's buckets-up commit tail (9.6 s) is new against m3fr8's 2.1 s with the same build of `crates/raft`.
  - The receipt rule shows nothing here: over the row, 1,664 sends for 1,641 commits, 0 `busy`, 4 asks, 23 missed and 23 `suspect`.
  - The tail is on the leader. The `upStats` split was not kept for the buckets-up half, because the reset read holds the commit percentiles only. So its stage, queue or confirm, is not known from this run.
- **Agent push stages with buckets up:** resolve p50 504 ms, validate 1,518, commit 1,383.
- **B0-29's API read:** p99 5,420 ms buckets up (500 × 503), 1,434 ms placement failing.

## Rows m3fr8 (2026-11-16, TAG `m3fr8`, `ROWS="b0-32 b0-31"`, `STOP_USD=66`)

**Build and site day.**
- **Build:** branch `lm1-rows-m3f9` `0038ab2e` is main `09a945cd` plus two test-faults `server-timing` marks: `probe` when a commit ran `ensure_group`'s create probe, and `stagefirst` when a push staged before its first commit. `a1.mjs` records every push's `server-timing` (`pushTiming`, and `burst timing` per burst push).
- **What changed since m3fr7's `1aed2cff`:** `repack-keep-pack`, `composed-hits`, `feed-ahead`, `registry-restore`, `promote-warm`, `push-reads`, and lm1-rows-m3f8's drivers. These include B0-31's wait for site c's shard to warm.
- **Image:** new, built from `09a945cd`'s `containers/git`, `sha256:83e217ba…`.
- **Site day:** 10:14:33Z to 10:27:43Z. Sites `18e3db57`, `e2bfd834` and `6929abd0`, edge `1b4d607d`, both exit 0.
- **Checks:** `m3org/j4` answered 200 to admin and member. `repo:65536` is led by site 0 at term 4. Each voter's lagMax was 101, 0 and 0 ms, with 0 lease fallbacks.
- **L0-12** waits for a cap decision.

**Teardown.**
- The start sweep found no shard container.
- Each row's shards were torn down after it: 65572 at 10:39:40Z, 65573 at 10:59:54Z.
- The end sweep and `SWEEP2` both reported "no shard container running", and a third sweep at 11:05Z found none.

**Spend (graft-m3f, account Graftyard).**
- $59.93 at B0-32's start, $60.61 at B0-31's start, and $62.23 at 11:05Z.
- The site day was $0.01, B0-32 $0.68 and B0-31 $1.62; containers were $26.89 of the window.

### B0-32 (repo 65572, run `mwj4jl6k`): fail

| Gate line | Result |
|---|---|
| Every push acknowledged within 5 s, or answered `ng … agent admission full` | **Fail**: 498 ok, 0 within 5 s; `push:burst` p50 13,493 ms, p99 22,211 ms (m3fr7: 22,352 / 27,516). Two pushes got the client's `fetch failed` (`ERR_HTTP2_STREAM_ERROR`) at 3.26 s with no reply, so their acknowledgment is unknown |
| Queued agent bytes under `AGENT_QUEUE_MAX` (64 MiB) | Pass: max 174,979 B |
| Every merge in one of the next 2 entries | Not judged from entries. `merge:only` 18 × 200, 3 × 409, p50 896 ms, p99 11,079 ms; the edge's merge commit stage p99 1,181 ms |
| Zero leader changes | Pass: term 2 throughout, leader site 0 |
| Zero `suspect` reports | **Fail**: 47 inbound `/suspect` on the leader (m3fr7: 17) |

- **Leader:** commit p50 427 ms, p99 2,718 ms (n 4,026). Failed calls: 346, all `deadline` (102 to site 1, 180 to site 2).
- **B0-29's API read:** p99 3,776 ms, with 988 × 503.

**Where a burst push's time went** [measured, `server-timing` of 498 burst pushes, ms; each push's stages summed over its commits]:

| Stage | p50 | p99 |
|---|---|---|
| resolve | 1,487 | 1,799 |
| auth | 473 | 567 |
| install | 829 | 1,358 |
| gate | 255 | 1,847 |
| context | 0 | 1,811 |
| validate | 2,967 | 4,513 |
| admission | 0 | 0 |
| probe (34 pushes) | 1,109 | 5,215 |
| commit | 5,468 | 14,080 |
| stage (258 pushes) | 549 | 812 |
| edge total | 10,936 | 19,738 |

- **Before validate:** about 2 s at p50, against m3fr7's traced 1.2 to 17.3 s. `resolve` (1.5 s) is now the largest read. It runs beside `auth` and `install`, so the three are not additive.
- **Commit is the cost now:** 5.5 s at p50 for the edge's commit stage, against the leader's own commit p50 of 427 ms. Most of the edge's commit wait is outside the leader's measured commit [inference: the 47 `/suspect` and the 346 `deadline` failed calls point at receipts missing COMMIT_RECEIPT].
- **The create probe:** 464 of 498 burst pushes skipped it, and 34 ran it. Over the whole row, 43 of 1,733 commits probed.
- **Stage first:** 240 burst pushes staged before their first commit and committed once. 258 committed inline first, got `Stage`, and committed twice. Over the row, 253 pushes staged first.

### B0-31 (repo 65573, rehomed on site 2, run `mwj4z7uv`): fail

| Gate line | Result |
|---|---|
| Commit p99 at most 1 s with buckets up | **Fail**, between the lines: 2,138 ms (p50 998, max 3,938, n 5,421); m3fr7 1,767 ms |
| Placement failing: Human and MergeQueue commit p99 at most 3 s, 0 of them refused | Pass: leader commit p99 1,588 ms; Human 160 ok, 13 stale, 0 refused; `merge:only@fail` 27 × 200, 5 × 409, 0 × 503 |
| Agent pushes over INLINE_MAX_HOT refused at once in the `ng` shape | **Fail, by 17**: 1,245 `full` (`ng … agent admission full`, p50 1,051 ms); 17 of 1,262 committed |

- **The apac leader held:** term 3 from the rehome to the end, with 14 failed calls.
- **The shard was warm before the merges:** site c's shard answered warm 4.9 s after the rehome (`shardwarm.mjs`). There were 0 MergeQueue `warming` 503s: `merge:only@up` 31 × 200 and 9 × 409; `merge:only@fail` 27 × 200 and 5 × 409.
- **B0-29's API read:** p99 1,263 ms buckets up, 1,104 ms placement failing.
- **Agent push stages with buckets up:** resolve p50 518 ms, validate 1,682, commit 1,264. No commit probed and none staged first, because 1 MiB packs never go inline.
- **The 17 agent commits counted under placement failing:** most likely pushes sent with buckets up that ended after the switch [inference]. `a1.mjs` labels a push `@fail` by its end time (`half()` runs after the push), and sends `x-graft-place-fail` only on pushes that start after `placeFailAt`. With pushes at p50 4.1 s, the agents in flight at the switch (about 4.17/s × 4 s, so 17) fit the count. The driver keeps no per-push send time, so the row cannot prove it. To make the line judgeable, label each push by its start phase. As run, the line fails by 17.

### m3fr8 after the run: B0-32's commit wait (analysis from Workers Logs and the r8 raw files, nothing built, no hosted spend)

**Sources.** 80 burst pushes sampled from the edge's 769 `git-receive-pack` events and traced end to end in Workers Logs by trace id (1,643 events). All came from colo LAX. Also the burst's `server-timing` for 498 pushes (`tests/am1/raw/a1-b0-32-mwj4jl6k.jsonl`, `burst timing`), the members' trace (`tests/fanout/raw/tsave-m3fr8-b0-32.jsonl`), and `tests/f74/inbound.py` over 10:36:49Z to 10:37:30Z. A Workers Logs `timestamp` is the event's end, so a handler's start is `timestamp - wallTimeMs`. A handler's wall is its time inside the callee, not the round trip.

**Where the edge's 5.5 s commit stage goes, against the leader's 427 ms** [measured, 80 traces, from the first `/commit` handler's start to the edge's end]:

| Part | Mean per push | p50 |
|---|---|---|
| The final send's leader handler | 786 ms | 648 ms |
| Earlier sends' leader handlers, whose 200 the edge did not take | 2,091 ms | 1,959 ms |
| Between sends, with no leader handler running: the 2 s receipt timer, `/suspect`, the 200 ms pause, and out legs | 2,286 ms | 2,138 ms |
| After the final handler: the back leg and the edge's own finish | 785 ms | 911 ms |
| Whole window | 5,949 ms | 6,013 ms |

- **Sends.** There were 201 `/commit` sends for 80 pushes (2.5 a push). Every one reached the leader (DO `6864b3e1`) and answered 200. Of the 121 sends that were not a push's last:
  - 39 had a leader handler of 2 s or more (p50 2,673 ms): the edge's COMMIT_RECEIPT expired while the leader still held the command.
  - 71 had a handler under 2 s (p50 965 ms), but the edge sent again anyway. The 200 came back after the receipt deadline, so the out leg plus the back leg took the rest. Of the 45 sends followed by a `/suspect`, 31 had already ended in the leader's handler when the `/suspect` arrived.
  - 11 were the Stage round trip: a quick handler (p50 169 ms) that answered `Stage`, then the bucket place, then the second commit.
- **The leader was congested by the resends, not slow at committing.** In the burst's window the leader took 2,075 `/commit` requests. Requests open in its handler, all routes, peaked at 572 (10.0 to 12.0 s after the burst started), p50 84. A resend under the same request id joins the same ticket, but it is another request on the leader's input, and it carries the 256 KiB inline pack again. Leader commit p50 427 ms, the leader's own measure, never sees the out leg or the queue ahead of the handler.
- **`/suspect` is the same mechanism as B0-02's** [verified match]. On a missed receipt, `commit_hedged` (`crates/lookup/src/lib.rs`) calls `suspect(all=false)`, which reports the leader to the other two voters. Over the first 31 s of the burst, `/suspect` was 842 of site 1's 2,643 inbound requests (31.9%) and 842 of site 2's 3,127 (26.9%), but only 47 of the leader's 6,945 (0.7%). There were none before or after the burst. 35 of the 80 pushes sent some. Every verdict left the leader in place: term 2 throughout, and zero leader changes. B0-02's 29 to 40% came from the same rule while a leader was healthy. Here it is the same rule with the leader healthy and congested.

**Why 258 pushes committed twice instead of staging first** [measured from `server-timing`; the per-isolate part is an inference]:
- **HOT_WINDOW did not lapse.** Each push's stage-first decision comes at the edge after resolve, auth, install, gate and context. Those decisions fell 2.0 s to 7.0 s after the first burst send, 486 of 498 of them between 3.5 s and 6.5 s, so the whole burst decided inside one HOT_WINDOW (5 s).
- **The hot signal came too late.** Times are from the client's first burst send. The first `Stage` reply anywhere reached the edge at 3.1 s, 3 had arrived by 3.5 s, 9 by 4.5 s, and 33 by 5.5 s. The re-committed pushes decided at p50 4.7 s, and the stage-first ones at p50 5.4 s. Per 0.5 s of decision time:

| Decision at | Committed twice | Staged first | `Stage` replies already back |
|---|---|---|---|
| before 3.5 s | 5 | 0 | 0 |
| 3.5 s | 29 | 20 | 3 |
| 4.0 s | 69 | 34 | 4 |
| 4.5 s | 79 | 31 | 9 |
| 5.0 s | 47 | 54 | 21 |
| 5.5 s | 21 | 73 | 33 |
| 6.0 s on | 8 | 28 | 46 to 56 |

- **So these were pushes decided in an isolate before that isolate's own first `Stage` reply** [inference: `hot` is a per-isolate map, and Workers Logs do not name the isolate]. This is not a single first push per isolate: the reply is late because the first commit, which answers `Stage` at admission, takes p50 3,448 ms to come back over the same congested commit path.
- **Staging first did not save much.** Stage-first pushes took 10.9 s at the edge (p50) and committed once in 5,278 ms. Re-committed pushes took 12.2 s: 3,448 ms commit, 549 ms stage, and 2,008 ms for the second commit. The commit path is the cost either way.

**What resolve's 1.5 s is** [measured, the 12 traces matched one to one to their `server-timing` and the 80 traces' reads]:
- **The reads, in order:**
  1. The `Names` login row on site a, a handler of 1 ms.
  2. The `Names` placement row on site a, 1 ms.
  3. The owner's `Authz` read by `spread_read`, with `index: true`, to a learner on site b (handler p50 173 to 183 ms) or site c (370 to 406 ms). Each such handler is a read-index round to the `Authz` leader on site a.
  4. Sometimes a second member, with a handler of 430 to 490 ms.
- **The learners are picked at random.** The `Authz` reads split almost evenly: site b's `ef42c0db` 57 and `f1471310` 41, site c's `7d31a775` 57 and `10d8b866` 36. An isolate with no answer time for a learner counts it as near (`timed_spread_order`), so fresh isolates send as many reads to apac as to weur [inference].
- **Reads were in flight for only 41% of resolve** (p50, p10 31%, p90 49%). The rest is the edge between reads, p50 140 ms per gap. Over the burst the colo's edge CPU summed 5.1 s per 2 s, with up to 529 pushes in flight. If few isolates held them, the gaps are their event loops queueing [hypothesis: Workers Logs do not name the isolate].
- **Resolve is not on the critical path by much.** It runs beside auth plus install, p50 473 + 829 ms, so it adds about 0.2 s.

**Whether validate's 3.0 s is the network shard or a pool forward** [measured, 80 traces]:
- **The network shard on site a** (DO `af5b4f7a`) received all 80 validates. It forwarded 56 of them to a pool shard (`via=0&pool=1`) and validated 24 itself (wall p50 2,186 ms).
- **The pool shard is cheap.** Its wall is p50 370 ms (p99 891 ms).
- **The wait is in the network shard, before the forward.** For the 56 forwarded validates, the forward started p50 3,038 ms after the network shard's handler began, at CPU p50 0 ms. In that time the network shard read the repository's owner and then the owner's record, both from `Names` on site a (`500d9d51`, then `54fddaa8`), with handlers of 1 ms:

| Step | p50 | p90 |
|---|---|---|
| Handler start to the first read | 294 ms | 613 ms |
| First read end to second read start | 1,185 ms | 3,024 ms |
| Second read end to the forward | 281 ms | 1,976 ms |

- **The edge does not pass `record` on validate.** The shard's code already takes the caller's record to "save the two `Names` reads" (`workers/site/src/shard.rs`), and the edge's merge and demand calls pass it. `validate_url` (`workers/edge/src/lib.rs`) does not.
- **The gaps are waiting in one Durable Object while about 434 pushes arrived in 1 s** [inference]. One candidate is the output gate holding each outgoing call behind the per-push `touch` storage writes (`last_request` and the alarm). The logs cannot tell the candidates apart.

**Proposal** (design first, not built; each needs its design text before code):
1. **Commit receipt (`commit_hedged`, DESIGN "Commit and read paths").** The operator decision, through B0-02, is that cutting `/suspect` needs a design of this rule. This is the design.
   - On a missed receipt to a member this isolate heard a 200 from in the current term within the last COMMIT_RECEIPT, do not report it. That member is busy, not dead. Report only on a miss with no such reply.
   - Do not resend the command, inline pack included. Keep the first send open, and from COMMIT_RECEIPT on ask `/outcome_by_request` for the request id, which joins the ticket without a second command on the leader's input.
   - In this trace, that would remove the 110 missed-receipt resends of the 121 non-final sends and the 82 `/suspect` waves, since every isolate kept hearing 200s from the leader, and most of the 2.3 s between sends [inference]. The final handler and the back leg, about 1.6 s, remain.
   - It changes no constant: COMMIT_RECEIPT and FOLLOWER_WAIT stay. It touches no `crates/raft` code, and `Acked` stays built only inside `outcome`.
   - Open point for the design: `/outcome_by_request` takes the command for its replay. A body without the inline pack needs a decision on the replay check.
2. **Validate:** pass `record` on the edge's `/validate`, as the merge and demand calls do. That removes both `Names` reads and their waits from every push, and validate should drop toward the pool shard's 370 ms [inference]. Separately, write `touch`'s `last_request` at most once a minute, since the renewal alarm is an hour out, so the burst writes once instead of 434 times. Each change is a small edge or site change with a regression test.
3. **Stage-first:** rerun after 1 before changing it. The late hot signal is the slow first commit, and 1 shortens it. If it is still late, an isolate can stage when it already holds several pushes over INLINE_MAX_HOT in flight to one group. That is a local signal, with no raft flag, which the `push-reads` entry rejected. Expected gain at most about 1.3 s a push (10.9 s against 12.2 s).
4. **Resolve:** treat a learner with no answer time as far, or seed its score from the site's measured round trip, so a fresh isolate reads `Authz` from weur (about 180 ms) before apac (about 380 ms). Expected gain about 0.2 s, because resolve runs beside auth and install. Lowest priority.
5. **Instrument for the rerun:** a per-isolate token in one edge log line per push, to test the isolate-queueing hypothesis and the per-isolate reading of stage-first.

## Rows m3fr7 (2026-11-15/16, TAG `m3fr7`, `ROWS="b0-32 b0-31 l0-12"`, `STOP_USD=66`)

> **Annotation, 2026-11-18 (cut-trace).** This run's build (main `1aed2cff`) carries lease-clock (`0b5769b7`), so its
> `/stats` `queue`, `queue:slot` and `queue:post` figures mix two clocks and overstate the wait. Commit p99 and the
> verdicts were on one clock and stand (`tests/m0/results/cut-trace.md`).

**Build and site day.** Main `1aed2cff`, which ran with these merges since m3fr6's `a304d3fc`: lm1-rows-m3f6's drivers and test-faults instruments, `read-round-gap` (`0646faa2`), `composed-miss`, and `promote-unlock`. New shard image `sha256:29c3b917…`, built from that commit's `containers/git`. Site day 23:21:26Z to 23:35:02Z, logged in `install-m3f.md`. Rows started 23:41Z at $48.91 (graft-m3f, account Graftyard). From 02:23Z a cost watch metered every 15 minutes with `env-m3f.sh` and would have stopped L0-12's refusals after the current kind at $66. It never fired.

**What was lost.** The orb restarted at about 04:20Z, in L0-12's last refusal kind, and cleared `/tmp`. That took the run worktree, its raw files (`a1-*.jsonl`, `terms`/`admit`/`vmem`/`tsave-m3fr7-*`, the status file, the stall and refusals logs) and the uncommitted driver changes (`rows.sh` `ROW_USD`, `tests/fanout/rounds.py`). The figures below were read from those files before the restart (they are quoted from this thread), or from Workers Logs, the repository's refs and the meter after it. A line marked *lost* could not be judged.

**Teardown.** The row's own teardown and `rows.sh`'s final sweep never ran. At 04:26Z `teardown.py --sweep` found 7 shard containers running: `net:65571` on all three sites (L0-12's), `net:65568` on all three (an earlier row's), and `net:65570` on site c (B0-31's, restarted after its row's teardown). It tore down all 7 and reported none left, and a second sweep also found none [measured].

**Spend (graft-m3f, account Graftyard):** $48.91 at the start, $51.01 after B0-32, $52.74 after B0-31, $56.43 after L0-12's replay, $57.09 at the start of the refusals, $59.33 at 04:26Z. The rows cost $10.42.

### B0-32 (repo 65569, run `mwihdvym`): fail

| Gate line | Result |
|---|---|
| Every push acknowledged within 5 s, or answered `ng … agent admission full` | **Fail**: 500 of 500 ok, 0 within 5 s; `push:burst` p50 22,352 ms, p99 27,516 ms |
| Queued agent bytes under `AGENT_QUEUE_MAX` (64 MiB) | Pass: max 155,696 B |
| Every merge in one of the next 2 entries | *Lost* (not evaluated before the restart). Merges: 62 × 200, 12 × 409, 1 × 503 `warming` |
| Zero leader changes | Pass: every voter at term 2 before and after |
| Zero `suspect` reports | **Fail**: 17 inbound `/suspect` on the leader |

Leader commit p99 3,942 ms, queue p99 4,393 ms, 96.7 req/s. B0-29's API read p99 (`read:api`): 3,038 ms, with 1,149 × 503.

**Where the 22 s goes** (five burst pushes traced end to end by Workers Logs `traceId`) [measured]:

1. **Reads before validate, 1.2 to 17.3 s.** The read gate's and identity reads go one member at a time, and many take about 1.0 s each. That matches a follower waiting out FOLLOWER_WAIT for its read index, answering `overloaded`, and the edge asking the next member. Corrected after the run: this is the largest share in two of the five (`a63615bb`, `b4b732be`), not four. See "B0-32: the reads on a hot push's path" below for the split.
2. **Validate, 0.4 to 7.0 s.** On the network shard (site a) it took p50 2.6 s, max 4.5 s; on a site b pool shard, up to 7.0 s.
3. **The first `/commit`, 0.2 to 0.9 s.**
4. **Then, in every push after the group turned hot:**
   - a gap of 1.5 to 2.5 s with no member call;
   - a `{"probe": true}` `/read` on the leader;
   - a second `/commit`.
5. **When a commit's wall passes COMMIT_RECEIPT (2 s):** `/suspect` to both followers, then the same commit again.

**The "stage, then re-commit" reading is confirmed** from the code path and the trace shape. Workers Logs carry no reply bodies, so the reply itself is not seen [inference]:
- **Only Stage makes a second commit.** The edge's `commit()` runs `ensure_group` (one probe `/read`) before `commit_hedged`. It runs a second time only after `CallErrorWire::Stage` (`workers/edge/src/lib.rs`, `receive_pack`). A receipt resend inside `commit_hedged` sends `/suspect` and no probe.
- **The traces show that shape.** The traces have the probe `/read` between `/commit` 1 and 2 with no `/suspect`, and the gap fits the R2 place.
- **The group was hot when the gap appeared.** The admission sampler shows `hot` false until 23:55:03.9Z and true for the whole burst. 256 KiB is over INLINE_MAX_HOT (64 KiB), so the leader's `hot_stage` answers `Stage` (`crates/raft/src/member/leader.rs`). A push before the burst (23:54:52Z, not hot) has one `/commit`.
- **The row expects staging** ("the row tests `INLINE_MAX_HOT`, which stages each 256 KiB pack"). Its cost here is one leader round trip, the R2 place, a probe read, and a second commit.
- **Corrected after the run: there is no second validate.** Each push's validate shows as two `/validate` spans because the network shard (site b `71715191`) forwards the pack to a pool shard (`via=0&pool=1`, `2690500a` or `fa0cdb9a`) and returns that shard's answer. The pairs are 410 / 3,184, 737 / 697 and 7,033 / 6,442 ms (network / pool). The pool shard's Workers wall runs past the network shard's return: on `b4b732be` the network shard returned at 11,296 ms and the pool shard's wall ended at 14,392 ms, after the first `/commit` at 12,290 ms. That tail is work after the response, not a validate between the commits [measured spans; that it is post-response work is an inference].

**The 17 `/suspect` are missed receipts, not a failing leader.**
- **Commit walls:** sampled leader `/commit` walls in 23:55:00 to 23:55:40Z were n 519, p50 570, p90 2,020, p99 3,094 ms, with 54 over 2 s.
- **Pushes affected:** 43 of 596 burst pushes had a commit over 2 s.
- **Who reported:** each missed receipt drew one `suspect` round, gapped per isolate by SUSPECT_GAP.
- **The leader held:** it kept term 2 throughout.

**What a fix has to move, in order:**
1. The reads before validate under a burst.
2. Validate on the network shard.
3. The Stage round trip. A hot group could answer Stage in the first commit's place, so the edge stages before its first send.
4. The receipt deadline against a commit p99 of 3.1 s.

Nothing is built.

### B0-31 (repo 65570, rehomed on site 2, run `mwiidjbq`): fail

| Gate line | Result |
|---|---|
| Commit p99 at most 1 s with buckets up | **Fail**: 1,767 ms (max 2,567, p50 998, n 5,426); m3fr6 2,895 ms |
| Placement failing: Human and MergeQueue commit p99 at most 3 s, 0 of them refused | Pass: leader commit p99 1,824 ms; Human 136 ok, 15 stale, 0 refused; `merge:only@fail` 29 × 200, 4 × 409, 0 × 503 |
| Agent pushes over INLINE_MAX_HOT refused at once in the `ng` shape | *Lost* (`push:agent@fail` codes not read before the restart) |

The apac leader held at term 3, with 1 failed call. B0-29's API read p99: 1,423 ms (buckets up), 1,549 ms (placement failing).

**The parent's checks:**
- **MergeQueue `warming` 503s: 5, not 0.** All were in `merge:only@up`.
- **No 60 s stall after a `/promote`.** 12 promotes took 195 to 4,668 ms, and their `derived=1` validates 5.3 to 11.7 s. The one 61.9 s `site/archive/promote` (00:29:32Z) came after the row ended (00:27:47Z) [measured, Workers Logs].

**The 5 `warming` 503s came from a cold home shard** [measured, Workers Logs on site c's GitShard]:
- **When they ended:** 00:17:18Z to 00:17:56Z, each at 15.0 s (MERGE_WAIT). That is 28 to 66 s into the 600 s run, which started at 00:16:50Z, about 30 s after `transfer.mjs` rehomed the group.
- **What the shard was doing:** the apac shard was rehydrating from cold the whole time. Its log at 00:18:13Z shows up 34,182 ms, tail 59,732 ms, catch_up 69,047 ms, 268 MB in 257 packs, index 6.2 s.
- **Catch-ups in the window:** 74.0, 58.5, 43.2 and 28.1 s, and "shard warm 65570 site 2: no answer in 25 s" four times.
- **After it warmed:** catch-ups took 0.3 to 5.7 s, with 0 `warming` in the remaining ~510 s of buckets up and 0 with placement failing.

**Proposal: warm the shard in the row setup; this is not a product bug.**
- **Why not a product bug.** DESIGN's `IngestCursor` text makes `warming` the retryable answer a caller gets while a shard catches up, and MERGE_WAIT bounds it. Raising MERGE_WAIT, or having merges wait out the catch-up, were both rejected.
- **Why the row should wait.** B0-31 measures a steady apac leader, and the rehome's cold start is not what it gates. `rows.sh` should wait after `transfer.mjs` until site c's shard `/status` shows the rehydrate done, with its cursor at the leader's commit. That adds about 70 s of setup.
- **What stays open** [inference]: a planned rehome of a repository this size gives about 70 s of merge 503s on the new home. Warming the target site's shard before a transfer would remove that. This is a candidate OPEN-QUESTIONS row.

### L0-12 (repo 65571, run `mwij41ij`): fail

| Gate line | Result |
|---|---|
| No overlapping Exclusive leases over DRIFT; member clock offsets under 500 ms | *Lost* |
| Every lease live before the stall live after it, `expires` moved by the stall | *Lost*; the stall ran its 30 minutes (exit 0, 02:03:59Z), with the leader on site 0 at term 6 and no member `stalledSince` through minute 21 |
| The late Exclusive acquire succeeds | Pass: 201, 0 conflicts (02:03:59Z) |
| 500 long-pollers see each event within 3 s at p99 outside leader kills | *Lost*. Corrected after the run: the 25,235 ms first written here (p50 3,876; 42,090 `hold`, 6,574 `gone`) is `byKind.feed`, each poll's wall time including its 25 s hold, not an event's latency. The gate's figure is `feed.outsideKills.p99`, which was in the lost raw file. On m22 the same two figures were 25,453 ms (poll wall) and 2,444 ms (outside kills) (`tests/fanout/raw/m22/a1-l0-12-mwf42xv9.jsonl`). See "m3fr7 after the run" below |
| 100 of 100 refused for each of the two `ActorKind::Agent` kinds, 0 of 100 for the other three | 5 kinds complete, all as the gate wants. The Graftyard MCP kind stopped at 55 of 100 when the orb restarted |

Refusals by kind. The counts come from the edge's `POST …/git/refs` statuses in Workers Logs, in kind order, and from the refs that exist on the repository afterwards (`git ls-remote`) [measured]:

| Kind | REST ref writes | Pushes |
|---|---|---|
| confined vendor App | 100 × 422 (02:04 to 02:31Z), "leased Exclusive by user 7100002" | 100 of 100 refused, naming the lease (driver summary) |
| flagged App without `confine` | 100 × 201 | 100 refs exist, so 0 refused |
| unflagged App | 100 × 201 | 100 accepted (driver summary) |
| human PAT | 100 × 201 | 100 refs exist |
| Graftyard-run App (`graftyard-workspace`) | 100 × 422 (03:33 to 04:00Z) | no refs, so 100 of 100 refused |
| Graftyard MCP | 55 × 201, then stopped | 55 refs exist, so 0 of 55 refused; **45 not run** |

`am1-runs` saw `graftyard-workspace` refused by `NoLiveTask`, not by the lease. Workers Logs carry no bodies, so which refusal it was here is not known.

**Replay** (600 s from 01:07:16Z, leader site 0 at term 2, 10 kills, one every 60 s):
- **Leader commit:** p50 1,543, p99 6,918 ms; 1,192 failed calls.
- **B0-29's API read p99:** 3,637 ms (7,155 × 503 of 116,793).
- **Agent pushes:** 2,491 of 2,491 ok, p99 25,783 ms.

**Against m3fr6:**

| | m3fr6 | m3fr7 |
|---|---|---|
| Term range | 2 to 32 | 2 to 6 |
| Leader changes across the 10 kills | (m3fr6 section) | 3 (by kills 5, 9, 10) |
| StepDowns 01:05 to 01:25Z | (m3fr6's record in its section) | site a 6 (Confirm 5, CommitErrors 1, all one member `ef19dfa6`); site b 0; site c 2 (CommitErrors, member `d4897b3f`) |
| lagMax and lease fallbacks per member | 4.3 to 5.7 s on sites 0 and 1, 163 ms on site 2 | *lost* (`vmem` raw); live `/stats` at 04:30Z read 0 everywhere, so the members had restarted |
| Read rounds per voter per 20 ms (max / p99 / windows over 5) | 99/61/84, 55/42/18, 213/45/270 | *lost* (`tsave` raw) |

Kill log (the leader seen before and after each kill, and the sites reset):

| Kill | Before | After | Reset |
|---|---|---|---|
| 1 | 0, term 2 | 0, term 2 | none |
| 2 | 0, term 2 | 0, term 2 | 0 |
| 3 | 0, term 2 | 0, term 2 | 0 |
| 4 | 0, term 2 | 0, term 2 | 0 |
| 5 | 1, term 3 | 1, term 3 | 1 |
| 6 | 1, term 3 | 1, term 3 | 1 |
| 7 | 1, term 3 | 1, term 3 | 1 |
| 8 | 1, term 3 | 1, term 3 | 1 |
| 9 | 0, term 4 | 0, term 4 | none |
| 10 | none answered | 2, term 5 | none |

The terms rose with the read-round gap in place, by 4 over the run against 30 on m3fr6. Most kills that reset a member did not move the leader.

### m3fr7 after the run: L0-12's feed and B0-32's reads (analysis from code and Workers Logs, nothing built, no hosted spend)

**L0-12's feed line is unproven on m3fr7, not failed.**
- **What the 25.2 s was.** a1.mjs records each poll under `byKind.feed` as `Date.now() - s`, and that wall includes the 25 s hold. The gate needs `feed.outsideKills.p99`: each event's time from its push's ack to a poller seeing it, with events near a kill left out. That figure was in the raw file `/tmp` lost.
- **Earlier runs.** On m22 the two figures were 25,453 ms (poll wall) and 2,444 ms (outside kills). On m3fr6 the outside-kills p99 was 2,954 ms, under the fail line of 3 s that `d91d45cf` set, so m3fr6's feed line passed (corrected above).

**What Workers Logs show of the feed**, from `member/feed` walls over the replay, 01:07:16Z to 01:17:16Z [measured]:

| Learners | Polls | Under 20 ms | 20 ms to 1.5 s | 1.5 to 24 s | 24 s or more |
|---|---|---|---|---|---|
| site a, 4 learners | 21,795 | 632 (2.9%) | 3,263 | 16,806 | 1,094 (5.0%) |
| site b, 2 learners | 8,688 | 173 (2.0%) | 985 | 6,981 | 549 (6.3%) |
| site c, 2 learners | 20,670 | **6,176 (29.9%)** | 7,630 | 6,808 | 56 (0.3%) |

- **No event is held back on sites a and b** [inference]. A held poll ends at its first matching event plus FEED_BATCH, or at 25 s. Each poller follows one of 50 path groups, and 2,491 pushes in 600 s is about one event per group every 12 s. That predicts about 12% of polls with no event in 25 s, and sites a and b had 5 to 6%. Their polls ran 1.5 to 24 s because they waited for the next event.
- **Mass ends follow the kills.** Seconds in which 100 or more polls ended came 11 to 14 s after a kill (01:09:28, 01:10:28 and 01:12:25Z on every site). That is inside KILL_WINDOW_MS (15 s), so those events are already outside the gate's p99.
- **Site c answered 30% of its polls at once.** The driver counted 6,574 `gone` answers. The fast polls on all sites add up to 6,981, and 6,176 of them are on site c's two apac learners. The logs carry no bodies, so tying the two counts together is an inference.

**Two defects that would make a `gone` loop, both found in the code:**
1. **The member answers 410 for a cursor ahead of it.** `graftyard_git::feed::start` returns `Gone` unless `oldest <= cursor <= newest`. A cursor past the member's own `newest` (a member behind, not an expired cursor) also gets 410. The edge's `activity` returns that 410 to the client at once, without trying another member. The design gives 410 only "for a cursor older than the ring or inside an `InstallSnapshot` hole" (`design/AGENT-LAYER.md`), so a real client would needlessly reload its leases and tasks. How it happens: an apac learner that lags answers `lagging`, and the poller is held on a nearer learner. The next poll carries that learner's newer cursor back to the apac learner, which is first in the poller's order, and gets 410 until it catches up.
2. **The driver re-polls at once with the same cursor.** A `gone` answer carries `oldest`, not `cursor`, and a1.mjs keeps its old cursor (`if (v.cursor) cursor = v.cursor`). So it asks the same lagging learner again at once, until that learner passes the cursor. Every event in that span arrives late, and inside the gate's sample.

**Proposal** (design first, small, nothing built):
- **Product.** A cursor ahead of the member's `newest` answers `lagging` with its `lagMs`, which the edge already routes to the next member. 410 stays for a cursor below the ring's floor and for another group's cursor. This is one change in `feed::start` plus its unit test (a cursor at `newest + 1` is not `Gone`), and AGENT-LAYER's 410 sentence already says this.
- **Driver.** a1.mjs follows `oldest` on `gone`, as a client follows `oldest_cursor`. It also counts `gone` and `lagging` per member in the summary, so the next run shows whether the cycle is gone.
- **Then rerun L0-12 whole**, as decided. The lost lines also need it: the clock offsets, lease drift and stall, and the MCP kind's last 45.
- **No change to the hold path.** On this evidence nothing there holds events.

**B0-32: the reads on a hot push's path, in order** (`workers/edge/src/lib.rs` `git` and `receive_pack`) [code]:

| Step | Reads | Where | Run |
|---|---|---|---|
| 1. `resolve` | `account_of` (Names), then `placement` (Names), then `{"q":{"repo"}}` on the owner's `Authz` | Names by `call_opt` on the leader; `Authz` by `spread_read` (learners, then followers, retried while shed for LOOKUP_FOR) | serial; a write skips `rcache` |
| 2. `authenticate` (`ghs_`) | Tokens(0) keys dep and the `jti` row | leaders | joined; serial after 1 |
| 3. `read_gate` | `core_rows(access)` on the repository group, `authz_read(dep)` on `Authz` | `spread_read` | joined; skipped on a gate-cache hit by `jti` |
| 4. `push_context` | `install_authz` (install `placement` + install `Authz` dep), then `core_rows(agent)` | Names leader, then `spread_read` | serial |
| 5. validate | network shard, forwarded to a pool shard | | |
| 6. `admission` | `install_authz` again | as 4 | serial |
| 7. `commit` | `ensure_group`'s probe `/read`, then `commit_hedged` | repository leader by `call_opt` | serial |
| 8. on `Stage` | `place_teed`, then 7 again | R2, then leader | serial |

**Where the five traced pushes spent their time** [measured, Workers Logs spans, ms]:

| Push | Total | Before validate | Validate (network) | Validate to first commit | First commit to end | Commits | `/suspect` |
|---|---|---|---|---|---|---|---|
| `11f70160` | 20,223 | 1,787 | 2,459 | 2,959 | 13,018 | 3 | 7 |
| `2f7072a9` | 23,523 | 1,674 | 7,033 | 9,234 | 5,582 | 2 | 2 |
| `a63615bb` | 20,926 | 17,260 | 737 | 1,370 | 1,559 | 1 | 0 |
| `b4b732be` | 18,010 | 10,886 | 410 | 994 | 5,720 | 2 | 0 |
| `f7ba529b` | 14,644 | 1,217 | 2,649 | 1,224 | 9,554 | 4 | 10 |
| Share of the sum | | 34% | 14% | 16% | 36% | | |

- **Which members the slow reads hit.** The reads of about 1 s before validate went to the repository group's 8 learners. Those are the 8 members, besides the three voters, that logged "no install source at Index(1)" at 23:44:18 to 20Z, when the row placed them, 4 / 2 / 2 on sites a / b / c as `LEARNERS` asks. So the 1 s reads are step 3's access rows and step 4's agent rows, not `Authz` (this corrects my earlier guess).
- **How slow those learners were** in the burst (23:55 to 23:58Z): `/read` p50 283 to 651 ms and p99 1.0 to 2.3 s. The repository's followers on the same sites answered at p50 13 and 48 ms.
- **Idle edge time.** In the two slow pushes, no member call was open for 8.9 s and 5.3 s before validate. That fits the waits between shed retries (`retry_shed`) and `call_opt`'s 250 ms pass sleep [inference].
- **Repeated passes before a commit.** Before a commit, and in `a63615bb` before validate, the traces repeat one pattern every 1.1 to 1.5 s: fast reads on the repository followers, three `registry/row`s, then the leader's `/read` at 0.1 to 0.95 s. That is a `call_opt` pass that found no leader to answer, repeated. That it is `ensure_group`'s probe, or `core_rows`' own `ensure_group` after a spread that answered `Leader`, is an inference, since no body is logged.
- **The commit tail is as large as the reads.** After the first commit, a hot group answers `Stage`. The edge places the pack, probes again, and commits again. A commit wall over COMMIT_RECEIPT draws `/suspect` and a resend: 2 to 4 commits and up to 10 `/suspect`s in three of the five pushes.

**Which reads can run at once or go to the leader:**
- **At once.** Step 2 needs no repository, so it can run beside step 1, as REST already does. `install_authz` needs nothing from the request, so it can start with step 1, and step 6 can reuse step 4's result (one Names read and one `Authz` read fewer per push). Steps 3 and 4 both read the repository group, so they can be one read: `{"rows": {"access": true, "agent": true}}`.
- **To the leader.** For a push, that one rows read could go to the repository leader, as a REST merge's rows already do, since its commit goes there next. I do not propose that for a 500-push burst: about 33 reads/s added to a leader whose own `/read` ran 0.1 to 0.95 s, and DESIGN records the leader stepping down while it took follower overflow. The cut is to ask fewer times, not to move the reads.
- **The probe.** `commit()` runs `ensure_group` before every commit, even when the isolate has already reached the group's leader. A group the isolate has had a leader answer from (`KNOWN` holds its leader) needs no probe.

**Proposed fix, in order, design first** (DESIGN "Follower read" and the write path, plus a DECISIONS entry):
1. **One rows read per push.** Merge access and agent into one learner read, start `install_authz` and `authenticate` at request start, and reuse `install_authz` in `admission`. That removes 3 to 4 serial reads per push; in `a63615bb` and `b4b732be` the serial learner reads alone were 7.1 s and 2.9 s.
2. **Skip `ensure_group`'s probe** when the isolate already knows the group's leader, so the re-commit after `Stage` costs a commit and no probe or pass loop.
3. **Stage before the first commit on a group the isolate saw answer `Stage`** within the last N seconds. That saves one leader round trip and the re-commit in every hot push.
4. **Test installs log per-stage push timing** (`resolve`, `auth`, `gate`, `context`, `validate`, `admission`, `commit`, `stage`), as API requests carry `server-timing`. Then the next run times each step instead of inferring it from spans.
- **Not changed:** COMMIT_RECEIPT and FOLLOWER_WAIT.
- **The test** replays the five traced pushes' read and commit spans through the new order and must cut the p50 on main. The real check is B0-32's gate on the next site day.

## Rows m3fr6 (2026-11-15, TAG `m3fr6`, `ROWS="l0-12 b0-31 b0-32"`, `STOP_USD=46`)

> **Annotation, 2026-11-18 (cut-trace).** This run's build (`lm1-rows-m3f6` `520020fe` on main `a304d3fc`) carries lease-clock (`0b5769b7`), so its
> `/stats` `queue`, `queue:slot` and `queue:post` figures mix two clocks and overstate the wait. Commit p99 and the
> verdicts were on one clock and stand (`tests/m0/results/cut-trace.md`).

**Build and site day.** `lm1-rows-m3f6` `520020fe`: main `a304d3fc` with this branch's drivers and test-faults instruments, including two read-only raft `AdmissionView` fields (`round_ms`, `cmd_bytes`) and `vmem.mjs`'s per-voter `/stats` `clock`. The build ran with these main merges: `read-rounds` (`def9d6c1`), `repack-keep` (`8d79a32f`), `r22-sweep-fill` (`450bcae4`), `disk-validate` (`e7139c2e`) and `lease-clock` (`04577c42`). New shard image `sha256:b78eacca…`, built from main's `containers/git` at `a304d3fc` (`/validate?disk=1`, `held=`), on standard-4. Site day 17:17:57Z to 17:32:27Z: sites `0f1ce3ee` / `116b517b` / `f603b1b4` (phase 1 `0375142b` / `f4e758b6` / `d5d1056f`), edge `65a6583a`. Each step exited 0. After it, `j4` answered 200, `repo:65536` was led on site 0 at term 4, and every voter's `/stats` `clock` read lag 0 and `leaseFallbacks` 0. R22 second look: site a's `*/15` cron logged `sweep {` at 17:33:42Z and `fold audit {` at 17:35:01Z, with no "never complete".

Rows ran with `TRACE=1`, `LEARNERS=enam:4000,weur:2000,apac:2000` and `TEARDOWN=1`, from 17:36Z. **B0-32 did not run:** after B0-31, rows.sh's stop read graft-m3f's spend at $47.99, over `STOP_USD` 46 (see Cost). The end-of-run sweep found `git#2#net:65568` (B0-31's apac network shard) still running at 21:37:43Z and printed SWEEP FAILED. The second sweep tore it down and printed "no shard container running" (SWEEP2 OK, 21:38:11Z). After the service stopped, `teardown.py --sweep` ran twice more, at 21:39Z and 21:41Z, and both printed "no shard container running".

### L0-12: fail

Repository 65567 (`fm3fr6-l012`), k 4 from 17:38:49Z. The 8 learners had caught up 0 of 8 after 122 s. Replay run `mwi4cbu9`, 18:17:14Z to 18:27:14Z: 500 agents with a lease each, 500 feed long-pollers, and a leader kill every 60 s (10 kill pushes: 9 ok, 1 × 503). Stall 18:31:43Z to 19:01:47Z (`stalledMs` 1,801,162), the late acquire at 19:12:36Z, then refusals 19:12:52Z to 21:13:42Z, seven kinds.

| Gate line | Measured | Verdict |
|---|---|---|
| No two holders overlap Exclusive leases for longer than `DRIFT` (500 ms) | 111 samples, 0 over 0 ms, max −11,330 ms; with the clock spread, the bound is −10,818 ms | Pass |
| Measured member clock offsets under 500 ms | During the replay, against Cloudflare's clock (driver offset 5,335 ms): **972 ms** on the 76 samples kept by the round-trip filter, and 6,289 ms raw over all 97. The members' clocks were 56 to 512 ms apart (`lowerMs` / `upperMs`). Before and after the stall, at rest: spread at most 324 and 228 ms | **Fail** |
| Every lease live before the stall is live after it, `expires` moved by the recorded stall | 20 of 20 live, `expires` moved 1,772,000 to 1,773,000 ms for a 1,801 s stall | Pass [inference: the 28 to 29 s gap is the lease renewals' own step, not checked] |
| The late Exclusive acquire succeeds | Idle 60 s lease 201, then the overlapping Exclusive acquire 201 with 0 conflicts | Pass |
| Feed: each event within 3 s at p99 outside leader kills; fail over 3 s | 19,286 events outside kills: p50 1,178 ms, **p99 2,954 ms** (all 20,818: p99 4,360, max 16,876) | Pass. Corrected after the run: this row first read the fail line as "over 2 s", but `d91d45cf` (2026-11-13) set it to "feed p99 over 3 s", so there is no zone between the lines |
| 100 of 100 refused for each `ActorKind::Agent` kind, 0 of 100 for the other three | Table below | Pass |

| Kind | Gate | Pushes | REST ref writes |
|---|---|---|---|
| Confined vendor App | 100 refused | 100 refused (`leased Exclusive by user 7100002`) | 100 × 422 refused |
| Flagged App without `confine` | 0 refused | 100 accepted | 100 accepted |
| Human PAT | 0 refused | 100 accepted | 100 accepted |
| Graftyard-run App (`graftyard-workspace`) | 100 refused | 100 refused (`push needs a live task`) | 100 × 422 refused |
| `Graftyard MCP` | 0 refused | 100 accepted | 100 accepted |

An unflagged App also ran, though it is not one of the row's five kinds: 100 and 100 accepted. The refusal counts are exact for the first time on this install (m3fr5b was off by one on two kinds).

**Leader churn (the comparison point for B0-02's minimum round gap).** Term **2 to 32** over 18:16:09Z to 18:26:16Z (`tests/fanout/raw/terms-m3fr6-l0-12.jsonl`); term 33 after the stall. Churn began before the first kill, during the learners' catch-up. **16 `StepDown`s** in Workers Logs (`member alarm: StepDown`), 18:16:31Z to 18:25:37Z: 13 `CommitErrors` (site a 6, site b 6, site c 1) and 3 `Confirm` (site a, 18:20:48Z to 18:21:07Z, for example p99 998 ms with 7 of 9 slow). Only 3 leader aborts from the kills show in Workers Logs, all on site c (18:18:22Z, 18:19:32Z, 18:26:04Z); the driver kept no kill times (fixed on this branch, `ab45d58e`). Elections: wins per site 11 / 8 / 9, campaigns lost 6 / 6 / 29. Leader commit p50/p99 2,258 / 17,312 ms, queue p99 15.6 s, 8,574 failed calls. In the F74 call trace (`tests/f74/calls.py`, deadline 1 s), 46,794 of 111,415 `/raft` calls missed or went unanswered; the largest leg was out (sent to entered) in 22,959 of them and the callee's work in 2,217. B0-02's analysis: forced read rounds still come in bursts, because `read-rounds` shares a round per clock reading and `lease-clock`'s floor moves the clock on each peer stamp [B0-02's reading of this trace]. The trace and files went to the B0-02 thread.

**Clock lag and lease fallbacks per member** (`/stats` `clock` at 18:40Z; buckets 0, then under 2^i ms):

| Site | lagMax | leaseFallbacks | Stamps lagging 256 ms or more |
|---|---|---|---|
| 0 | 5,687 ms | 11,543 | 110,725 of 137,735 |
| 1 | 4,300 ms | 10,296 | 52,329 of 95,674 |
| 2 | 163 ms | 0 | 0 of 803 |

**API reads (B0-29's line, the lease fallbacks' cost).** `read:api` p99 22,450 ms: 46,259 × 200, 21,487 × 503 and 47,798 skipped at the driver's cap. B0-29's line is p99 at most 2 s with 99% served, and fails over 5 s. Under this churn the fallback's own share cannot be separated from the elections'.

### B0-31: fail (between the lines)

Repository 65568 (`fm3fr6-b031`). It rehomed on site 2 (apac), which led at term 3, and k reached 4 at 21:17:18Z. The 8 learners had caught up 0 of 8 after 120 s. Run `mwic5as1`: 500 agents and 1 MiB packs for 600 s from 21:23:39Z, with every bucket's placement failing from 300 s.

| Gate line | Measured | Verdict |
|---|---|---|
| Commit p99 at most 1 s with buckets up | Leader stats, buckets-up half: 5,365 commits, p50 1,009 ms, **p99 2,895 ms** | **Fail**, between the lines (under the 3 s fail line) |
| While placement fails, Human and MergeQueue commit p99 at most 3 s | Leader stats, failing half (all commands): 4,237, p50 996, p99 1,650 ms. End to end at the client: Human push p99 11.4 s (157), Human side commands p99 3.5 s (454), `merge:only` p99 17.0 s (32) | Pass on the leader's commit line [the leader's split is not per class; inference that Human and MergeQueue sit within it] |
| …and 0 Human or MergeQueue refused | Human pushes: 146 ok, 11 stale (race with the agents' pushes to the same ref, not a refusal), 0 refused. Merges: 16 × 200, 5 × 409 `Base branch was modified`, **11 × 503** `the git cache: … {"warming":true}` | **Fail** (MergeQueue: 11 refused) |
| Agent pushes over `INLINE_MAX_HOT` refused at once in the `ng` shape | 1,245 `full` in the `ng` shape (p50 1.9 s, p99 5.0 s), 15 ok, 1 client fetch failure, 0 × 503 | Pass on shape [the 15 ok went through before admission filled] |

**The apac leader held** the whole row: term 2 to 3 (the rehoming), 0 changes during load, 0 `StepDown` lines in Workers Logs from 21:15Z to 21:35Z, 16 failed calls, 0 feed polls on the leader. Against m3fr5c (term 3 to 16, commit p99 5.9 s, Human push p99 35.4 s, 121 × 503 shed on agents): no churn, commit p99 2.9 s, Human push p99 11.4 s, 0 agent 503s. With buckets up, agent pushes all committed (1,240 ok) but p99 72.5 s.

**disk-validate on this row.** Workers Logs on the three sites from 21:17Z to 21:34:30Z have no `missingBase`, `held=` or `disk=1` lines; the shard logs no line on a held push, so this says nothing about how often a held retry ran.

**Clock lag and lease fallbacks per member** (`/stats` `clock` at 21:35:41Z):

| Site | lagMax | leaseFallbacks |
|---|---|---|
| 0 | 280 ms | 0 |
| 1 | 224 ms | 0 |
| 2 (leader) | 727 ms | 107 |

**API reads (B0-29's line).** `read:api` 107,273 × 200, 0 × 503: p99 1,906 ms with buckets up and 1,193 ms with placement failing, both under B0-29's 2 s line, with 107 lease fallbacks on the leader.

### B0-32: not run

rows.sh stopped before it at $47.99 (above), as `STOP_USD` 46 asks. disk-validate's burst result is still owed.

## Rows m3fr5b (2026-11-15, TAG `m3fr5b`, `ROWS="l0-12 b0-31 b0-32"`)

**Build and site day.** `lm1-rows-m3f5`: main `dc269249` (stored-fetch, memory-budgets, shard-cancel, pool-warm) with this branch's drivers and test-faults instruments (`9ef481d7`), then `k.mjs` from `lm1-memory` `980daa4a` (`f40f6bf7`), which asks for the forced k again until pool-warm's slots are caught up. Full checks on `9ef481d7` in a clean worktree: fmt, clippy (workspace and wasm), cargo test, Go, design, names and links all passed. New shard image `sha256:c8d3a831…`, built from that tree's `containers/git` (the same as main's), on standard-4. Site day 2026-11-14 23:57:55Z to 2026-11-15 00:14:44Z: sites `329d440e` / `3de20354` / `aaa83b74`, then `f2c01d3c` / `29c4c3cf` / `c0e61a1d`, edge `8dd6b30b`. Each exited 0. After it, `j4` answered 200 and `repo:65536`'s leader answered with `memHigh`.

The first attempt (TAG `m3fr5`) skipped L0-12 with "k 4 not granted" (k 1, want 3: the slots were leased but not warm). It was swept: 10 torn down, none left running. The `k.mjs` change above fixed it. Rows ran with `TRACE=1`, `LEARNERS=enam:4000,weur:2000,apac:2000`, the 120 MiB memstop, `STOP_USD` 48 and the driver clock offset. B0-31 was skipped in this run (below) and rerun as m3fr5c. After the run: `SWEEP2 OK` at 04:06:06Z, and `teardown.py --sweep` at 04:15Z printed "no shard container running".

### L0-12: fail

Repository 65563 (`fm3fr5b-l012`), run `mwh395gx`. The replay ran 00:56:56Z to 01:06:56Z: 500 agents with a lease each, 500 feed long-pollers, and a leader kill every 60 s. Ten kills took the term from 2 to 18: 16 terms for 10 kills. Terms 9 to 15 came within 2.5 minutes (01:03:15Z to 01:05:51Z), and the leader moved between sites 0, 1 and 2. In terms 3, 6, 7 and 16, the trace saw no leader at all (`tests/fanout/raw/terms-m3fr5b-l0-12.jsonl`). The stall followed, then the idle and late leases, then the refusals (01:53Z to 03:52Z, six kinds). The 8 learners had caught up 0 of 8 after 121.7 s before the window.

| Gate line | Measured | Verdict |
|---|---|---|
| No two holders overlap Exclusive leases for longer than `DRIFT` (500 ms) | 100 samples, 0 over 0 ms, max −2,549 ms; with the clock spread, the bound is −1,215 ms | Pass |
| Measured member clock offsets under 500 ms | Against Cloudflare's clock (driver offset 3,045 ms: 3,028 before the window, 3,062 after): **1,171 ms** on the 73 samples kept by the round-trip filter, and 3,934 ms over all 87. In kept rounds the members' clocks are at least **975 ms** apart (`lowerMs`) and at most 1,334 ms (`upperMs`). The outliers are site 0 as leader in rounds 2 and 4, reading 1,874 and 1,977 ms against followers' 3,003 to 3,036 ms, on round trips of 189 to 205 ms | **Fail** |
| Every lease live before the stall is live after it, `expires` moved by the recorded stall | 20 of 20 live, `expires` moved about 1,790 s (the recorded stall) | Pass |
| The late Exclusive acquire succeeds | Idle 60 s lease 201, then the overlapping Exclusive acquire 201 with 0 conflicts | Pass |
| Feed: each event within 3 s at p99 outside leader kills; fail over 2 s | 7,914 events outside kills: p50 1,092 ms, **p99 4,245 ms** (all 11,878: p99 4,940, max 27,034) | **Fail** |
| 100 of 100 refused for each `ActorKind::Agent` kind, 0 of 100 for the other three | Table below | **Fail** (two kinds off by one) |

| Kind | Gate | Pushes | REST ref writes |
|---|---|---|---|
| Confined vendor App | 100 refused | **99** refused (`leased Exclusive by user 7100002`), 1 `rejected` | 100 × 422 refused |
| Flagged App without `confine` | 0 refused | 100 accepted | 100 accepted |
| Human PAT | 0 refused | **99** accepted, 1 `rejected` | **99** accepted, 1 × 503 |
| Graftyard-run App (`graftyard-workspace`) | 100 refused | 100 refused (`push needs a live task`) | 100 × 422 refused |
| `Graftyard MCP` | 0 refused | 100 accepted | 100 accepted |

An unflagged App also ran, though it is not one of the row's five kinds: 100 and 100 accepted.

**The two counts that are off.** Neither was the lease rule answering wrongly. Both requests got no usable answer at all.

- **The confined App's 100th push** hung for git's 300 s timeout. The edge logged no request between its REST write at 02:09:25Z and the next kind's first `info/refs` at 02:14:30Z, so the push never reached a logged answer. No `refs/heads/claude/l012-mwh6jpl1-*` exists, so nothing was accepted [measured, Workers Logs on `graft-m3f-edge`]. Where it stalled (the client, or a request the edge never finished) is not known.
- **The human PAT's lost push and REST write**, at 03:02:08Z and 03:02:09Z: both got 503 `the repository lookup failed; retry (authz:7100001: every member asked shed the read: overloaded)` with `Retry-After`. This is the round 4 shed answer (`workers/edge/src/api.rs`, `read_failed`). Neither git nor `rows.mjs` retries a 503. The sites' logs for that minute show no shed or busy lines, so why every member of the owner's `Authz` group shed at once is not known [unverified].

**Clock.** The orb's clock is about 3.0 s behind Cloudflare's. After subtracting that, followers read within about 330 ms of Cloudflare, and the leader under load reads up to 1.2 s early. One hypothesis: a Worker's `Date.now()` advances only on I/O, so a busy leader's `/status` reports a stale time [guess; not tested]. If so, the line measures timer coarsening, not clock drift. Settling it needs either a design edit to F3 that says how the offset is measured, or a measurement that rules it in or out.

**Memory: the first hosted check of the memory fixes under this load.** `memHigh` peaked at 16.8 MiB on site 0, 16.1 on site 1 and 11.4 on site 2, with wasm memory at most 69 MB. The 120 MiB memstop never fired, and there was no reset storm (m3fr3 had one on the leader). The leader's commit p99 was 10.7 s (p50 3.9 s).

**Edge memory.** At 01:53:12Z, `graft-m3f-edge` logged `Worker exceeded memory limit.` on the refusal setup's admin clone (`git-upload-pack`, 5.8 s), and that clone still returned 200.

### B0-31: skipped (driver), rerun as m3fr5c

Repository 65564. `transfer.mjs` sent `/move` 12 times, 3 s apart, and each answer was `Degraded`. Placement was still replacing the new group's site-1 voter: it was 104 to 135 ms from home site 0, over `NEAR_RTT` (100 ms). Over 84 s it tried f1 generations 2 to 7 and then kept the nearest. `group_move::next_step` returns `Degraded` for a `Degraded` config. The group settled at a `Normal` config, generation 5, after the driver had given up. `transfer.mjs` now retries until `Done` or 300 s.

### B0-31 m3fr5c: fail (rerun on the same site day)

TAG `m3fr5c`, `ROWS="b0-31"`, with the same environment as m3fr5b and the fixed `transfer.mjs`. Repository 65566. The group rehomed on site 2 (apac), which then led at term 3, and k reached 4 at 04:18:16Z. The 8 learners had caught up 0 of 8 after 120.7 s. Run `mwhbqtfw`: 500 agents and 1 MiB packs for 600 s from 04:24:15Z, with every bucket's placement failing from 300 s. After the row, rows.sh tore down 3 shards and its sweep printed "no shard container running" (04:41:33Z). The service was stopped and swept again at 04:45Z: none left running.

| Gate line | Measured | Verdict |
|---|---|---|
| Commit p99 at most 1 s with buckets up | Leader stats at 300 s: 249 commits, p50 1,605 ms, **p99 5,878 ms** | **Fail** |
| While placement fails, Human and MergeQueue commit p99 at most 3 s | The leader's own stats cover only 9 commands, because it changed 7 times in this half and each new leader's stats start empty. End to end at the client: Human push p99 **35.4 s** (138), Human side commands p99 28.0 s, `merge:only` p99 2.6 s | **Fail** |
| …and 0 Human or MergeQueue refused | Human pushes: 121 ok, 14 stale, **3 × 503** `the repository's access rows did not answer; retry (… every member asked shed the read: overloaded)`. One merge got 503 `Durable Object is overloaded` | **Fail** |
| Agent pushes over `INLINE_MAX_HOT` refused at once in the `ng` shape | 1,114 `full` in the `ng` shape (p50 1.9 s), **121 × 503** shed, and 17 ok | **Fail** |

**Leader churn.** The term rose from 3 to 16 during the row: 4 to 9 from 04:25:46Z (the buckets-up half) and 10 to 16 from 04:32:26Z (the failing half). The leader moved around sites 0, 1 and 2 and ended on 2. The apac leader's calls to its peers failed 3,732 times, almost all `deadline`, with 497 and 487 late answers from the two voters. The trace (`tests/fanout/raw/terms-m3fr5c-b0-31.jsonl`) went to LM-1. In the buckets-up half, agent pushes also got 503s: 119 shed, 14 `no answer`, 4 `not leader`, and 3 `no shard could check the pack (… isolate exceeded its memory limit and was reset)` from a git shard's object. `memHigh` stayed low on the voters: 6.9 / 8.1 / 6.4 MiB. Admission capacity ran 127 / 585 / 1,539 (min / p50 / max), with at most 84 agent commands queued.

Against m22 (commit p99 30.9 s, Human push p99 26.4 s): commit p99 is now 5.9 s, but the leader no longer holds apac, and Human pushes are no faster.

### B0-32: fail

Repository 65565, run `mwhayuzy`. The load started 04:00:01Z. Learners caught up 8 of 8 in 15.9 s, k was 4 by 03:56:32Z, and the warm-up saw `hot` and warm after 62 s. The last 30 agent pushes before the burst acked in 2.6 to 4.7 s. The term stayed at 2 throughout, with site 0 leading.

| Gate line | Measured | Verdict |
|---|---|---|
| Every push acked within 5 s or answered HTTP 200 `ng <ref> agent admission full, retry after <n>s` | **0 of 500 within 5 s**: 475 ok (p50 14.6 s, p99 25.0 s), 25 `full` in the `ng` shape, 0 with no answer or another shape | **Fail** (5 s) |
| Queued agent bytes under `AGENT_QUEUE_MAX` | Peak 106 KB, 11 agent commands | Pass |
| Every merge in one of the next 2 entries, none over 2 s | `merge:only` 24: 20 × 200, 4 × 409; client p50 939 ms, **p99 2,582 ms**. Edge `total` p99 2,556 ms (`tier_ahead` p99 1,659, `commit` p99 1,015) | **Fail** (over 2 s) |
| Zero leader changes | Term 2, leader site 0 throughout | Pass |
| Zero `suspect` reports | 0 inbound `/suspect` at the row's leader. 12 `POST /member/suspect` reached three other objects (4 per site) at 04:00:06Z to 04:00:07Z, as the load started and before the burst. They were not attributed to a group | Pass at the leader (as m3fr4 was judged); the 12 elsewhere need attributing |
| No object reset from memory | 0 `exceeded`; `memHigh` 9.1 / 8.1 / 8.2 MiB | Pass |

**Against m3fr4.** The 104 × 503 `no shard could check the pack (… warming)` are gone (0 now): every burst push got an `ok` or a `full`. The time went to site a's validates instead. In the burst's first minute, 1,000 validates ran on site a at p50 5.4 s, p90 11.2 s and max 19.4 s, including 44 × 429 from the container's gate. Sites b and c got none. The edge's `git-receive-pack` ran at p50 10.3 s, p90 16.4 s and max 22.5 s. The leader was not the limit: commit p99 1,460 ms (queue p99 601, round p99 330), at 148 req/s. Admission capacity ran 256 / 800 / 1,899 (min / p50 / max) with `cmdBytes` 426 / 886 / 2,063, and `pressure` never set.

## Rows m3fr4 (2026-11-14, TAG `m3fr4`, `ROWS="b0-31 b0-32"`)

On the 15:45Z site day from `lm1-rows-m3f` `31636ec1` (main `04f8b3a6`: `retired-voter`, `prsync-first-once`, `m4-web-5`, `bucket-own-first`, `decode-once`, `leader-clock-check`, plus m3fr4's instruments). Shard image `sha256:746aee8e…`, standard-4. Sites `fff2002a` / `8e4c2001` / `278cfa22`, edge `464357d5`. Each row on a fresh repository with learners enam 4,000, weur 2,000, apac 2,000, k 4 granted, and its shards torn down after it. `vmem.mjs` read `memHigh` every 2 s and `admit.mjs` read admission every second. The 120 MiB stop rule never fired: the highest counted high-water on any voter was 9.1 MiB. No isolate on any site ended `exceededMemory` from 16:04Z to 16:31Z (Workers Logs outcomes: `ok`, `canceled`, `aborted` only). Sweep after: `sweep: no shard container running`. Raw files: `tests/am1/raw/a1-b0-31-mwglf3oa.jsonl`, `a1-b0-32-mwgm2dkk.jsonl`, `tests/fanout/raw/{vmem,admit,terms,tsave}-m3fr4-<row>.jsonl` (git-ignored).

### B0-31: fail

Repository 65559, run `mwglf3oa`, 16:06:00Z to 16:16:00Z, 500 agents, every agent push 1 MiB, placement failing from 300 s. The group was rehomed on apac, and the apac leader (site 2) held all 600 s at term 3. The leader logged 1 failed call (a `deadline` to site 1) and 0 inbound `/suspect`.

| Gate line | Measured | Verdict |
|---|---|---|
| Commit p99 at most 1 s with buckets up | **1,333 ms** (p50 982, n 5,370). Split: queue p99 1,060, round p99 377, confirm p99 69 ms | **Fail** (between 1 s and the 3 s fail line) |
| While placement fails: Human and MergeQueue commit p99 at most 3 s, 0 of them refused | Leader commit p99 1,322 ms (p50 974, n 4,248, all classes). Human pushes: 143 ok, 15 `stale:race`, 1 `stale`, 0 refused. **MergeQueue: 0 of 29 merges committed**; all 29 answered 503, and `merge:only` gave 31 × 503 `the git cache: {"warming":true}` | **Fail** (MergeQueue refused) |
| Agent pushes over `INLINE_MAX_HOT` refused at once in the `ng` shape | 1,251 `full` (`ng … agent admission full`), push p50 1,970 ms; 22 ok (started before 300 s) | Pass on the refused shape |
| Fail line | MergeQueue merges refused while placement fails | **Fail** |

Buckets up: 1,227 of 1,227 agent pushes ok (p50 6.6 s, p99 12.6 s). Merges: 1 × 200, 1 × 409 and 21 × 503 `warming`. Feed p99 1,789 ms.

**Why the merges were refused: the apac shard's catch-up still falls behind, even reading its own bucket.** Bucket-own-first works: every stored pack the apac shard `git#2#net:65559` (`229b7170`) read came from site 2's bucket (`from_site {"2": 388}`). Its catch-ups still grew, though (`shard catch-up stored` reports, ms):

| Ended | Entries | Packs | Bytes | `total_ms` | Spool | Index |
|---|---|---|---|---|---|---|
| 16:06:29Z | 9 | 27 | 28 MB | 13,975 | 1,257 | 888 |
| 16:07:10Z | 28 | 65 | 68 MB | 40,060 | 4,241 | 2,146 |
| 16:08:42Z | 81 | 159 | 167 MB | 91,680 | 6,678 | 4,906 |
| 16:12:41Z | 186 | 388 | 407 MB | 237,252 | 31,256 | 11,987 |

The next catch-up ran from 16:12:41Z until teardown (16:16:36Z, "The container has not been started"). Every queue `/merge` on that shard waited it out to `MERGE_WAIT` (15 s) and answered `warming`. **The per-step timing settles it: the cost is per pack, outside the body read and index-pack.** In the 388-pack catch-up, spool (the container reading the body from R2) took 13% and index took 5%. The other 194 s, about 500 ms a pack, is outside both: by inference, each R2 GET's time to its first byte and the shard's call into the container, one pack at a time (`ingest_stored`, `crates/git/src/host.rs`, times the whole `first_copy`). The log brings about 4 staged packs a second, so a serial catch-up at about 610 ms a pack can never catch up. This is the branch the parent's scope named for this case: fetch still dominates with own-first, so fetch stored packs a few at a time, bounded and applied in order. Not built yet.

### B0-32: fail

Repository 65560, run `mwgm2dkk`. The load started 16:24:44Z. The warm-up saw `hot` and warm at 113 s; the last 30 agent pushes before the burst acked in 2.8 to 4.6 s. The burst fired at 16:26:37Z. Term 2 throughout, with site 0 leading; the leader logged 1 failed call (a `deadline` to site 2) and **0 inbound `/suspect`**.

| Gate line | Measured | Verdict |
|---|---|---|
| Every push acked within 5 s or answered HTTP 200 `ng <ref> agent admission full, retry after <n>s` | **0 of 500 within 5 s.** 385 ok (p50 21.0 s), 10 `full` in the `ng` shape, **104 × 503 `no shard could check the pack (… {"warming":true}); retry`** (answered after 160 to 170 s), and 1 client `fetch failed` (`ERR_HTTP2_STREAM_ERROR`, 4.7 s) | **Fail** (another refusal shape) |
| Queued agent bytes under `AGENT_QUEUE_MAX` | Peak 335 KB and 129 agent commands, against (1,000, 64 MiB) | Pass |
| Every merge in one of the next 2 entries, none over 2 s | `merge:only` 20: 15 × 200, 5 × 409, client p50 1,057 ms, p99 2,412 ms. Edge `total` p99 2,163 ms (commit stage p99 725, shard merge p99 517, no catch-up wait) | **Fail** on the slowest merge (over 2 s end to end) |
| Zero leader changes, zero `suspect` | 0 leader changes, 0 `/suspect` at the leader | Pass |
| No object reset from memory | 0 `exceededMemory`; `memHigh` max 9.1 MiB | Pass |

**Where the burst went: validate on site a.** Site a's shards answered validates during the burst at p50 4.5 to 9.8 s, p90 15 to 20 s and max 41.6 s. The network shard `0e7e1f40` took 1,204 of them, and the pool shards `cc2e0173`, `b09f271a` and `d442dd98` took 199 to 240 each. `d442dd98` was still rehydrating until 16:25:42Z (container `up` 32 s, rehydrate 48 s). From 16:27:06Z the edge logged `fanout: git#0#pool:4 is warming; pushes validate on the first shard for a while`. The catch-ups on site a's shards are small (1 to 40 entries, under 4 s), so they are not what is slow. The 104 pushes refused `warming` were the ones whose validate found no shard to check them: the network shard was saturated and a pool shard was warming. Admission did not hold them back: `pressure` was never true and capacity stayed at 277 or more.

**Retired-voter's first hosted check:** 0 `/suspect` at the leader, against 72 on m3fr2, and 1 failed call, against 1,032.

### Admission, memory and PrSync, both rows

| Row | Capacity min / p50 / max | `cmdBytes` min / p50 / max | `retryAfterS` | `hot` samples | `pressure` | Agent queued max |
|---|---|---|---|---|---|---|
| B0-31 (1,031 samples) | 189 / 442 / 885 | 459 / 931 / 2,025 | 1 to 2 | 230 | never | 19 |
| B0-32 (693) | 277 / 1,269 / 1,732 | 440 / 643 / 2,081 | 1 to 2 | 186 | never | 129 (335 KB) |

`memHigh` peak per voter (MiB; wasm memory peak in brackets): B0-31 site 0 8.1 (26.7), site 1 6.6 (19.1), site 2 leader 7.7 (62.0). B0-32 site 0 leader 9.1 (52.6), site 1 8.0 (33.3), site 2 8.8 (30.1). The log is most of each peak (3.8 to 8.0 MB); the largest in-flight call bodies were 1.5 MB.

`prsync-first-once`, first hosted check:

- **"first handled":** site c's voter (B0-31's apac leader) 39, 16:06Z to 16:16Z, p50 15,972 ms, max 16,609 ms. Each one spent about 14.96 s in the merge step (`ms[2]`) waiting on the warming apac shard, then shed (438 shed, 38 asked). Site a's voter (B0-32) 58, p50 1,500 ms, max 15,372 ms (the slow ones at 16:27Z, while site a's pool shards warmed). Every attempt ended under `FIRST_FOR_MS` (25 s), and none timed out.
- **`/prsync/first` `exceededMemory`:** none (no `exceededMemory` outcome on any site).
- **`testMerges.late`:** not recorded. `a1.mjs` writes `testMerges` only for plans with `openPrs`, and B0-31 and B0-32 have none. The `first handled` reports carry `asked`, `unasked`, `shed` and `budget` (null on these rows), not `late`.

**Next:** B0-31's MergeQueue line needs the apac shard to apply stored packs faster than 4 a second: a bounded parallel fetch in `ingest_stored`'s caller, applied in log order. B0-32's line is validate capacity on the leader's site during the burst: the 503 `warming` shape comes from a pool shard that is still warming while the network shard is saturated. Neither row stormed.

## Rows m3fr3 (2026-11-14, TAG `m3fr3`, `ROWS="l0-12"`, `L012_REPLAY_ONLY=1`)

On the 13:33Z site day from `lm1-rows-m3f` `1fc80ec1` (main `44385391`: `bucket-own-first`, `narrow-reads-3`'s herding guard, `decode-once`, `leader-clock-check`), image `sha256:c7650ed2…`. The `retired-voter` fix (main `fef67dee`) was **not deployed** on this site day; m3fr4's site day carries it. L0-12's replay ran alone as the parent's memory-storm reproducer, a diagnostic run, not a gate run. Beside it: every voter's `/stats?summary=1` every 2 s (`vmem.mjs`, wasm high-water and `memSplit`), its `/elections` rows every 20 s, each voter's `/trace` every minute (`tsave.mjs`), `terms.mjs` and `admit.mjs`. Memory resets, `/snapshot_commit` calls and installs per member and minute come from Workers Logs (`tests/fanout/wlnames.py`). Raw: `tests/am1/raw/a1-l0-12-mwggu4r3.jsonl`, `tests/fanout/raw/{vmem,tsave,terms,admit}-m3fr3-l0-12.jsonl`.

Repository 65558, run `mwggu4r3`, 14:28:33Z to 14:38:33Z, 500 agents, 10 leader kills. Sweep after: SWEEP2 OK, no graft-m3f shard container running (Containers API, 53 instances, 0 running). Cost $19.95 of $40 at 14:44Z.

### The storm reproduced, and starts on the leader

Memory resets (`exceededMemory` outcomes) per voter, by second:

| Voter | Reset outcomes by second (count) | Outcomes / "memory limit" log lines |
|---|---|---|
| site a `repo:65558#1#f0` | 14:33:20 (79), 14:34:08–09 (331), 14:35:42–54 (751), 14:36:38–44 (886) | 2,047 / 4,272 |
| site b `repo:65558#1#f1` | 14:36:19 (999), 14:36:27 (618), 14:36:51–37:06 (2,241) | 3,858 / 4,329 |
| site c `repo:65558#1#f2` | none | 0 / 1 |

- **No snapshot path.** No `/snapshot_commit`, no `install {`, `install not started` or `abandoned` line at any member of 65558 in 14:27 to 14:45Z. m3fr2's storm (11:38 to 11:52Z) had none either. The narrow-reads-3 lead does not hold on graft-m3f.
- **The first reset is site a's, and site a was the leader** (term 5, from 14:33:03; the leadership had moved site a → b → c → a by then). Site b's first reset, at 14:36:19, also came while it led (term 8, from 14:36:00). Site c led from 14:37:29 to the end and never reset. So far every reset whose voter's role is known in the second before it is a voter that had just become leader.

### What the counted bytes show in the minute before each first reset

`memSplit` counts: fold rows, the fold caches (dumps, narrow, named), log bytes, learner holds (`held`), and in-flight call bodies, answer bodies and statements.

- **Site a's first reset, 14:33:20.** Site a was a follower until 14:33:03 at wasm 47.8 MiB (its high-water since leading at 14:29), log 8.1 MB, dumps 1.1 MB, everything in flight 0. It won term 5 at 14:33:03; no `/stats` sample answered from 14:33:02 to 14:33:21 (19 s gap), so the last counted bytes before the reset are a follower's: about 9.2 MB counted against a 47.8 MiB heap. The calls it took in the 25 s before (Workers Logs): 3,958 `/read`, 446 `/raft`, 328 `/commit`.
- **Site b's first reset, 14:36:19.** At 14:36:01, one second after it won term 8: wasm 30.2 MiB, **answers in flight 22.9 MB**, calls 2.7 MB, log 8.0 MB, admitted 47. The next sample answered at 14:36:25, after the reset. In the 24 s before: 1,756 `/read`, 890 `/raft`, 92 `/commit`.
- **Site a's later reset, 14:36:38.** It won term 9 at 14:36:27: at 14:36:25 **calls in flight 29.0 MB**, at 14:36:27 calls 26.0 MB and answers 20.3 MB (46 MB counted, wasm 26.8 MiB), at 14:36:32 answers 20.4 MB; reset at 14:36:38. In the 23 s before: 2,485 `/read`, 1,357 `/raft`.
- **Site b, 14:36:45** (between its resets): wasm 78.9 MiB with **learner holds 38.7 MB**, the biggest number of the run.
- **Site c as leader, 14:37:29 on (no reset).** Wasm 42.8, then 55.0 MiB; calls in flight at most 1.2 MB, holds at most 0.6 MB. Its `/trace`: in its first minute it queued 11.2 MB of AppendEntries to site a and 10.7 MB to site b (83 calls each), and held 67 to 82 AppendEntries per learner.

**Reading.** At each reset after the first, the counted bytes are a new leader's resend of the log tail: call bodies 26 to 29 MB, answers 20 to 23 MB, learner holds up to 38.7 MB, all within a few seconds of its election, against an 8 MB log. That matches LM-1's B0-33 on main `4438539` (round 17: call bodies up to 58.3 MB, holds up to 42.9 MB). The first reset is not explained by counted bytes: the last sample before it was a follower's 9.2 MB counted against 47.8 MiB of wasm, and the 19 s before the reset after the election has no sample. **What `memSplit` does not cover:** the JS heap (request and response objects, the `fetch` bodies queued in the runtime, the streamed answers' chunks once handed to the runtime), the decoded copies of incoming `/raft` bodies while they are stepped, and wasm the allocator holds but no counter names (fragmentation; the heap only grows). The sample gap is itself the shape of the problem: the new leader stops answering `/stats` in the seconds the bytes would show.

### Elections and terms

Term 2 to 12 over the window on the live voters (10 kills): 14:32:49 to 14:33:03 had 13 `CampaignEnded` rows for term 4 on sites a and c with no leader for 14 s, and 14:36:00 to 14:36:27 had 9 for term 8. Site a led term 7 from 14:33:45 through its resets at 14:34:08 and 14:35:42, and term 9 from 14:36:27 until its reset at 14:36:38. One more election at 14:40:44Z (term 13, site a), after the window, during the row's teardown reads.

### Other lines (diagnostic; this run is not a gate run)

- **Feed:** p50 1,349 ms, p99 8,379 ms (outside kills p99 8,595 ms), max 21.3 s. The slow 1% outside kills were served by site c, the last leader.
- **Clock:** `maxAbsOffsetMs` 1,674 ms, and every voter reads 1.5 to 1.7 s ahead of the driver at its median (site a 1,505, site b 1,576, site c 1,606 ms). The orb's own clock reads 1.5 to 2.0 s behind the edge's `Date` header (three probes, ±500 ms). So most of the offset is the **driver's clock**, not a member's; the run's spread between members in one round is up to 2.3 s, from site a's reset leader at −677 ms. `clock.mjs` should subtract a driver offset measured against the edge before the gate line is judged.
- **Leases:** 126 overlap samples, none above 0 ms (max −914 ms).
- **Pushes:** agent 1,960 ok of 2,491; 422 got 503 "every member asked shed the read: overloaded", 74 "not leader", 11 500 "created, but no member leads".

## Rows m3fr2 (2026-11-14, TAG `m3fr2`, `ROWS="b0-31 b0-32 l0-12"`)

On the 10:04Z site day (`d438f437`: `repo-write-mark` and `admission-stored-bytes`). Each row on a fresh repository with learners enam 4,000, weur 2,000, apac 2,000, k 4 granted, the row's shards torn down after it. `admit.mjs` read capacity, `cmdBytes` and `retryAfterS` every second. Raw files: `tests/am1/raw/a1-<row>-<run>.jsonl`, `tests/fanout/raw/{terms,admit}-m3fr2-<row>.jsonl`, `tests/fanout/raw/rows-m3fr2.status` (git-ignored).

### B0-31: fail

Repository 65555, run `mwg9hh9a`, 10:33:16Z to 10:43:16Z, 500 agents, every agent push 1 MiB, placement failing from 300 s. `/move` homed the group on apac. The apac leader held all 600 s at term 3 (`terms.mjs` on the live voters), with 1 failed call and 6 `/suspect` reports at the leader.

| Gate line | Measured | Verdict |
|---|---|---|
| Commit p99 at most 1 s with buckets up | Leader `/stats` reset at 300 s: buckets-up half p99 **1,118 ms** (p50 777, n 5,210); queue p99 910, round p99 311, confirm p99 59 ms | **Fail** (between 1 s and the 3 s fail line) |
| While placement fails: Human and MergeQueue commit p99 at most 3 s, 0 of them refused | Leader commit p99 1,121 ms over the half (all classes). Human pushes 158 ok, 10 `stale:race`, 0 refused, end to end p99 20.1 s. **MergeQueue: 0 of 29 merges committed**; every one answered 503 `the git cache: {"warming":true}` after `MERGE_WAIT` (15 s) | **Fail** (MergeQueue refused) |
| Agent pushes over `INLINE_MAX_HOT` refused at once in the `ng` shape | 1,277 of 1,302 `full` (`ng … agent admission full`), push p50 1.8 s; 25 accepted (started before 300 s) | Pass on the refused shape |
| Fail line | MergeQueue merges refused while placement fails | **Fail** |

Buckets up: 1,199 of 1,199 agent pushes ok (p99 11.0 s), and 1 of 22 merges committed (21 × 503 `warming`). Feed p99 2,444 ms.

**Hot and staged now.** `hot` was true in 239 of 300 buckets-up samples (it reads committed pushes over HOT_WINDOW, so it falls once placement fails and agent pushes are refused: 5 of 299). Every agent pack was staged (1 MiB, over `INLINE_MAX_HOT`).

**Admission (`admit.mjs`, leader apac throughout):**

| Half | Capacity, min / p50 / max | `cmdBytes` p50 / max | Agent queued p50 / max | `retryAfterS` | Round p50 / max (ms) | `pressure` |
|---|---|---|---|---|---|---|
| Buckets up (300 samples) | 270 / 654 / 1,043 | 768 / 1,892 | 2 / 18 | 2 throughout | 198 / 255 | never |
| Placement failing (299) | 192 / 367 / 846 | 1,356 / 2,563 | 2 / 11 | 2 throughout | 198 / 234 | never |

Capacity stayed 7 to 40 times the 27.6 commands/s offered. The commit p99 is queue time (910 ms p99) on a 200 ms round, not admission.

**The apac shard still falls minutes behind, and `MERGE_WAIT` now turns that into refused merges.** Merges go to the leader site's shard, `git#2#net:65555` (`workers/edge/src/api.rs`, `merge_plan`). That shard's `warm` catch-ups (Workers Logs, site c, object `c89e4269`):

- 10:28:20Z, before the window: rehydrate of 36 entries in 36 s, 34 of it `up` (container start).
- 10:30 to 10:33: warm catch-ups of 0.2 s each (the log was quiet).
- From 10:34:09Z: 34 s, then 76 s and 91 s (10:35:22Z), then five more that never ended: each ran 504 to 577 s and was cut at teardown (10:43:46Z, "The container has not been started").
- Every one of the queue's `/merge` calls on that shard waited on the catch-up and was answered 503 `warming` at `MERGE_WAIT`; `merge_plan` on the one merge that committed spent 12.1 s in the catch-up.
- `shard warm 65555 site 2: no answer in 25 s` every 15 s from 10:41Z to the end.

What its catch-up reads, from R2's per-bucket operations (GraphQL `r2OperationsAdaptiveGroups`, 10:33 to 10:44): agent packs were put to all three buckets (about 500, 300 and 450 puts a minute to a, b, c), but **every GET was from bucket a (enam)**, 1,000 to 1,400 a minute while the load ran. Buckets b and c had no GETs. `ShardIo::ingest_stored` (`crates/git/src/host.rs`) tries `self.buckets` in order, and `buckets()` (`workers/site/src/shard.rs`) lists every site's bucket by site id, so every shard reads site 0's bucket first. The apac shard therefore pulls each 1 MiB staged pack from enam, one pack at a time (`Load::Stored` is one call each in `ingest_all`), at about 4 packs/s of log. Hypothesis, not yet timed per step: the catch-up is bound by that cross-region fetch, not by unpack or index (site a's shards validate the same packs at p50 1.1 to 2.6 s). The fix this points to: `ingest_stored` should try the shard's own site's bucket first, then the others; and stored packs could be fetched a few at a time. Not made yet; the per-step timing (`ms` in the catch-up report) of a catch-up that finishes would settle it.

### B0-32: invalid as run (driver bug), and fail on its numbers

Repository 65556, run `mwga3x8e`. The load started 10:48:07Z; the warm-up waited from 60 s for `hot` and the last 30 agent pushes acked within 5 s.

**The warm-up's load stopped at 180 s.** The driver's load loops run while `Date.now() < end`, and `end` was `t0 + 180 s`; the warm-up moved `end` only after it ended. So the load ran 10:48:07Z to 10:51:07Z, `hot` was true from about 10:48:30Z to 10:51Z (170 of 180 samples), and then the group sat idle while the warm-up polled for 900 s. Warm never held in the load's 120 s of warm-up: the last 30 agent pushes took 3.2 to 7.4 s. The burst fired at 11:04:08Z on a cold, idle group (`hot: false, warm: false`), which is not the row as written. Fixed in `a1.mjs` (below): `end` now moves past the warm-up's longest wait when the warm-up starts.

Its numbers, for the record (they do not count as the row):

| Gate line | Measured |
|---|---|
| Every push acked within 5 s or `ng … agent admission full` | 0 of 500 within 5 s. 233 ok (p50 25.0 s), 26 `full` in the `ng` shape, 241 × 503 `no shard could check the pack (… {"warming":true})`. Every push has an answer: none lost, client log has `sentAt` and result for each |
| Queued agent bytes under `AGENT_QUEUE_MAX` | Peak 13.9 KB, 34 agent commands |
| Merges in one of the next 2 entries, none over 2 s | merge:only p99 1,783 ms (24 × 200, 2 × 409), shard catch-up 0 ms |
| Zero leader changes, zero `suspect` | Term 2 throughout, site 0 led; but **72 `/suspect` reports and 1,032 failed calls at the leader** |

Admission during the burst (166 samples): capacity 323 / 947 / 1,056, `cmdBytes` p50 407 max 1,900, agent queued max 34, `retryAfterS` 1 to 2, `hot` in 12 samples, never `pressure`.

Before the burst, while the load ran, site a's validates took p50 1.1 s and p90 1.9 s (100 sampled, 43 forwarded to pool shards), and agent pushes acked at p50 4.1 s. So even hot, agent pushes on this group were not under 5 s end to end; whether the burst meets 5 s on a warm group is the rerun's question.

**Where the pre-burst push time goes** (45 acked agent pushes, 3 every 10 s from 10:48:30Z to 10:50:50Z, joined by trace id in Workers Logs: edge `git-receive-pack`, shard validate, member `/commit`; ms, p50 / p90):

| Step | p50 | p90 |
|---|---|---|
| Edge wall (client p50 4,138) | 3,927 | 5,126 |
| Before validate: about 8 member `/read`s per push, 2 cross-site (site c p50 253 ms, site b about 100 ms), and about 465 ms with no object call (body upload, edge work) | 1,135 | 1,492 |
| Validate on site a's network shard (`wait=15&next=1`, all 45 answered 200 first try, none forwarded to a pool shard) | 1,486 | 2,312 |
| Validate end to `/commit` | 116 | 312 |
| `/commit` at the leader, all sends | 466 | 1,300 |
| Last `/commit` end to the edge's end | 618 | 743 |

Validate is the largest part (38%). Staging is off the path for inline packs; 5 of 45 got `Stage` and paid about 770 ms of placement plus a second `/commit` of about 614 ms. The last row's 0.6 s also shows on merges, inside the edge's `commit` stage; not explained. The edge sends no `server-timing` for `receive-pack`, so the rerun cannot read this split from the client.

**The 72 `/suspect` at the leader** came from edge isolates inside push requests, just before `/commit` (46 of 49 traces), 35 during the load and 37 in the burst's first 3 s; none changed a term (term 2 throughout). Each follower got 96, which fits 72 `all` reports (a `NotLeader` naming no leader) plus 24 about site 0. In the 7 traces read, the edge had just read site b's retired first-generation voter `repo:65556#1#f1`, still live and answering follower at term 2 with no leader and config index 3 (the current `#2#f1` is at 9). Inference, not proved: lookups fall back to the record's generation-1 bootstrap names and the stale voter's `NotLeader` with no leader starts the `all` reports.

**The 1,032 failed calls** are the leader's outgoing AppendEntries and pings that errored or missed CALL_DEADLINE. The two followers handled every `/raft` under 1 s. 128 calls of 1 s or more went to learners (feed learners and two others), about 12%. The rest never reached a callee or queued before it; the per-peer split was not kept (`a1.mjs` reads only `/stats?summary=1`, and the leader has since restarted). The rerun needs the leader's full `peers` before the row ends.

### L0-12 m3fr2: diagnostic only, stall and refusals cut off

Repository 65557, run `mwgayihm`, replay 11:39:41Z to 11:49:41Z. The stall started next; the `rows-m3f-r2` service was stopped at about 12:15Z, in the stall's 20th minute of 30, so **the stall and refusal parts have no result**. The replay's numbers:

| Line | Measured |
|---|---|
| Clock | `lowerMs` 704, `upperMs` 1,150. The low reading follows the leader role, not a site: whichever member leads reads 350 to 800 ms below both followers (raw samples, each leader change moves it) |
| Feed p99 outside kills | 6,291 ms over 12,636 events. All 128 slowest were held by apac (site 2) learners after 1 to 8 `lagging` answers: the driver pinned pollers to members and did not route as the edge does |
| Overlap | 133 samples, none over 0 ms |
| Kills | 10 kills, 9 ok, 1 answered 503 `overloaded`; term 2 → 10 |
| StepDowns | 5 × `CommitErrors` holds from site 2's leader (11:43:42Z to 11:44:41Z, 3 to 6 failed calls to sites 0 and 1 each), 2 from site 0's leader (11:47:41Z, 11:47:51Z); no `Confirm`. Trace sent to LM-1 |

Leaderless spans per kill (live-voter terms trace `tests/fanout/raw/terms-m3fr2-l0-12.jsonl`; leader lost → new leader site and term):

| Lost | Led | Gap |
|---|---|---|
| 11:43:00Z | 11:43:07Z, site 1, t3 | 7 s |
| 11:43:08Z | 11:43:26Z, site 2, t4 | 18 s |
| 11:45:52Z | 11:45:56Z, site 0, t5 | 4 s |
| 11:46:19Z | 11:46:26Z, site 1, t6 | 7 s |
| 11:46:35Z | 11:46:49Z, site 0, t7 | 15 s (rounded from 14) |
| 11:47:03Z | 11:47:14Z, site 1, t8 | 11 s |
| 11:47:35Z | 11:47:35Z, site 0, t9 | 0 s |
| 11:49:29Z | 11:49:54Z, site 1, t10 | 26 s |

BUILD-PLAN's L0-12 gives no 15 s kill window; the 15 s `KILL_WINDOW_MS` is `a1.mjs`'s own. `ELECTION_AFTER` is 10 s (`crates/raft/src/lib.rs`), so a gap of 11 to 26 s is an election that ran past one timeout. The `/suspect` bursts start 11:43:24Z, 11:45:57Z, 11:46:51Z, 11:47:15Z, 11:47:37Z, 11:49:29Z and 11:49:55Z. Pre-vote, vote and first-commit times per kill are in no log on this build.

**What the leaderless spans were: voter isolates resetting on memory, not slow elections.** Workers Logs counts of "Durable Object's isolate exceeded its memory limit and was reset" over the replay, per 10 s: site a's voter `7b9da729` 8,465 events (11:42:50, 11:43:00, 11:46:10 to :20, 11:46:50 to 11:47:00, 11:49:10, 11:49:40), site b's voter `9a5c6a7e` 4,722 (11:43:00 to :30, 11:46:20 to :30, 11:47:20, 11:49:30, 11:49:50), site c's none. The injected kills (`graftyard member fault`) were at 11:40:48, 11:42:48 and 11:49:04 on site a and 11:45:46 on site c. Against the gaps:

| Gap | What the trace and logs show |
|---|---|
| 11:43:00 → :07 (7 s) | Site 0 (leader) killed at 11:42:48, then memory resets on site a; site 1 campaigned at 11:43:02 and led at :07 |
| 11:43:08 → :26 (18 s) | Site 1, the new leader, reset on memory from 11:43:00 to :30. Site 2 named site 1 its leader to 11:43:23; its promise to site 1 held until it lapsed, then site 2 campaigned at 11:43:25 and led at :26 |
| 11:46:35 → :49 (14 s) | Site 1 leading, reset on memory at 11:46:20 to :30; site 0 campaigned at 11:46:48 |
| 11:47:03 → :14 (11 s) | Site 0 leading, reset on memory at 11:47:00 |
| 11:49:29 → :54 (26 s) | **Not leaderless for 26 s.** Site 0 was killed at 11:49:04. At 11:49:29 site 2 already followed site 1 at term 10, and site 0 named site 1 at :47. Site 1 (`9a5c6a7e`) was elected but its isolate reset on memory at 11:49:36 to :38 and again at :50, so its `/status` did not answer until 11:49:54. The terms trace marks a leader only when the leader's own `/status` says so |

So past two ELECTION_AFTERs is a leader that won but whose isolate kept resetting, and the 18 s gap is a follower's promise to a leader that reset outlasting the reset. Every gap past 10 s has a memory reset on the leader's isolate inside it. The same build's m3fr2 B0-31 and B0-32 logged no memory resets; m3fr1's L0-12 logged 2,695 on site a. This is the same symptom as gate 16's memory storm, on a build (`d438f437`, main `1d331994`) that has neither narrow-reads-3 nor its `/raft` send timestamps (`54a700a9` is not in it), so the `/raft` double decode cannot be its whole cause. What allocates is not known from these logs.

The election rows (`leader-clock-check` `6a21d71a`, `GET /elections`) record pre-vote sends, vote answers with deny reasons, role changes, campaign ends and each term's first leader commit, so the rerun will time each phase from the members' own clocks.

**Teardown leak after m3fr2.** With the stall's `/_bed/fault?site=255` still armed when the service stopped, every member call failed, `/_bed/kill` could not read the repository's record (`release: release#9#f1: 409`, 500 after 20 s), and five shards stayed running: `git#2#net:65555` (from 10:43Z), `git#2#net:65556` (from 11:06Z), and `git#{0,1,2}#net:65557` (from 11:07Z to 11:43Z), until 12:28Z. `rows.sh` printed "torn down" because `teardown.py` exited 0 on 500s. Fixed in `teardown.py`: `--sweep` clears the site fault first, and a teardown with any non-200 exits 1. `/_bed/fault?off=1`, then `--sweep`: 5 × 200, none running.

**Built since (for the L0-12 rerun):**

- Clock check and election rows, branch `leader-clock-check` (`7bbf68dc`, `6a21d71a`) off main `7200c96`, full checks pass (878 tests): `/status?clock=1` adds `clockBefore` and `clockAfter` around one awaited macrotask and `leaseClock`; `Stats::lease_clock` records, per leader input that appended an entry, `at` → after the step and `at` → next macrotask (`/stats?summary=1` `leaseClock`). `a1.mjs` reads `?clock=1` and keeps the three readings in each raw clock sample.
- Feed driver `f3dd757e`: polls as the edge's `activity` routes them (learners first, then non-leader voters, up to 3; a lagging member's poll moves on; `lagging_ok` on the least lagged learner when all lag).

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

### L0-12 m3fr1: fail

Repository 65554 (`fm3fr1-l012`). Replay run `mwfza2tr`, 06:18:59Z to 06:28:59Z: 500 agents with a lease each, 500 feed long-pollers, leader killed every 60 s. Then the 30-minute stall (`stall.mjs`), then the refusals (`rows.mjs l012 100`, `AgentsExclusive`, holder m3-member's Exclusive lease on `l012/<tag>/**`).

| Gate line | Measured | Verdict |
|---|---|---|
| No overlapping Exclusive holders over `DRIFT` (500 ms) | 156 overlap samples, 0 over 0 ms | Pass |
| Measured member clock offsets under 500 ms | Spread `lowerMs` 1,260 (at least one round had two members at least 1.26 s apart); `maxAbsOffsetMs` 3,321 is against the orb's clock, not the gate quantity. Site 1's samples read a dead object (below) | **Fail** |
| Every lease live before the stall is live after, `expires` moved by the stall | 20 of 20 live, moved 1,803 to 1,804 s for a 1,801 s stall | Pass |
| The late Exclusive acquire succeeds | 201, 0 conflicts (07:15:36Z) | Pass |
| Feed: each event within 3 s at p99 outside leader kills | p99 25,622 ms, p50 1,147 ms (15,886 events, 10 kills) | **Fail** |
| Refusals: 100 of 100 for the two `ActorKind::Agent` kinds, 0 of 100 for the other three | Table below | Pass (five kinds; `Graftyard MCP` run alone, below) |

| Kind | Gate | Pushes | REST ref writes |
|---|---|---|---|
| Confined vendor App | 100 refused | 100 refused (`leased Exclusive by user 7100002`) | 100 × 422 refused |
| Flagged App without `confine` | 0 refused | 100 accepted | 100 accepted |
| Human PAT | 0 refused | 100 accepted | 100 accepted |
| Graftyard-run App (`graftyard-workspace`) | 100 refused | 100 refused (`push needs a live task`) | 100 × 422 refused |
| `Graftyard MCP` (run alone, 09:36Z to 09:57Z) | 0 refused | 100 accepted | 100 accepted |

The run also tried an unflagged App, which is not one of the row's five kinds: 96 accepted, and 4 REST writes got 422 `Object does not exist` (their scratch commits had not landed yet [guess]).

`Graftyard MCP` did not run inside the row: `rows.sh`'s 7,200 s timeout ended the refusal phase at 09:15:36Z, after five kinds of about 21 minutes each. It was run alone on the same repository and holder setup (`ROWS_L012_ONLY`). The first lone run (09:20Z) is void: its admin clone failed unseen, so all 100 REST writes read 422 `"sha" wasn't supplied` and all 100 pushes were rejected with no refusal text. `rows.mjs` now stops on a failed clone (`4fc6e736`). The kind ran again from 09:35Z with the same holder setup (a new holder task and Exclusive lease, `01M7CPKX…`): 100 of 100 pushes and 100 of 100 REST writes accepted. Its shards were torn down and a sweep of graft-m3f found none running (09:57:50Z).

L0-12 fails on the clock and feed lines alone. Their causes are below.

### 4. L0-12 m3fr1: the clock and feed fails, what the kept data shows

**Every site-1 probe read a dead object.** Site 1's voter of repo:65554 is `repo:65554#6#f1` (a replacement, generation 6). `lib.mjs` `statuses()` and a1.mjs's clock sampler called site 1 by site alone, which reaches the first generation's `#1#f1`. That object answered at term 2, last 3, and commit 3 before, during, and after the row (checked live at 07:46Z: `#6#f1` follower at term 17, `#1#f1` follower at term 2). So site 1's column in `terms.mjs` and `admit.mjs` and site 1's clock samples are not the voter's. Fixed in `d90d0ab`: `statuses()` and the clock sampler now call each site's current voter from the committed config.

**Leader churn.** The kept trace is `lm1-rows-m3f-data/terms-m3fr1-l0-12.jsonl`; site 1's column is the dead object. Sites 0 and 2 show the following:

- Through the first 5 kills (06:18:59 to 06:25:11) the term held at 2. Each kill gave a 1 s gap, and site 0 led again.
- From 06:25:11 to 06:28:59 the term climbed from 2 to 15, with 4 kills in that window. Leadership went 0 → 1 → 0 → 1. Leaderless spans ran 4 to 48 s; 06:26:31 to 06:27:19 had no member reporting leader.
- Over the 600 s, site 0 led 471 s, site 1 110 s, and none 19 s.

All 10 kill pushes answered ok. The trace went to LM-1. This build is before main `2456b8f` (liveness-slow-storage).

**Clock (`maxAbsOffsetMs` 3,321, `lowerMs` 1,260).**

- `maxAbsOffsetMs` is the offset from this orb's clock, not between members, so it is not the gate quantity. Measured live at 07:46Z, the orb ran 778 to 832 ms behind all three members, which agreed within 54 ms.
- The gate quantity is the member spread. `lowerMs` 1,260 means that in at least one round, two members' kept intervals were at least 1.26 s apart. That fails the under 500 ms line.
- Which member and which round are not recoverable: the row kept only the summary, and site 1's samples came from the dead `#1#f1`.
- Hypothesis, not checked: a member answering `/status` mid-churn or mid-reset reports a `now` far from the others. The raw samples (round, site, offset, rtt, term, role) are now kept in the row (`d90d0ab`) to settle it.

**Feed (p99 25.6 s outside kills).**

- 16,100 of 49,954 polls (32%) answered `lagging`, and 33,854 `hold`.
- The driver's kill window is 15 s after each kill push. The churn ran 230 s, so most of it counts as "outside kills". Feed events in a leaderless span wait for the next leader's commit.
- Hypothesis: the p99 is the churn, not the feed path. It is not checked, because the row kept no per-event times or members.
- The row now keeps the slowest 1% of events outside kills, with the member that served each and the poller's misses before the hold (`d90d0ab`).

**Next:** rerun L0-12 on main `5c0f06f` with these instruments. The churn may be gone on the liveness build; if the clock spread or the feed p99 still fails, the raw data names the member.

## Cost

**$68.31** at 16:55Z on 2026-11-16, graft-m3f only, after the 14:45Z site day and rows m3fr9 (L0-12 $2.59, stopped after the replay; B0-32 $1.31; B0-31 $1.65), with no shard container running. Started at $62.76, stop at 77.

**Cap raised to $80** (2026-11-16, about 13:40Z, approved by the user through the parent thread), with `STOP_USD=77` for rows.sh: graft-m3f was at $62.23 after m3fr8, and a whole L0-12 (about $12) plus a site day did not fit under 66. The rows that follow the `receipt-rule` merge are L0-12 whole, then B0-32, then B0-31 if `costok` leaves room.

**Cap raised to $70** (2026-11-15, by the parent thread), with `STOP_USD=66` for rows.sh: graft-m3f was at $48.06 after m3fr6, past the earlier $50 cap's room for another site day and three rows. **The meter mix-up, recorded:** `tests/fanout/m2bcost.py` priced graft-m2b unless `INSTALL` named another install, and during m3fr6 the meter was run in a shell without `tests/am1/lf13/env-m3f.sh` sourced. It read graft-m2b ($33.41 to $33.52), and those figures went to the parent as graft-m3f's; rows.sh's own lines, which source it, were right ($45.75 and $47.99). From now on every metering command sources `env-m3f.sh`, and the meter's line ends with the install and the Cloudflare account it priced (`"install": "graft-m3f", "account": …`, branch `promote-unlock`).

**$48.06** at 21:39Z on 2026-11-15, graft-m3f only, after the 17:18Z site day, the new image and rows m3fr6 (`source tests/am1/lf13/env-m3f.sh; T0_FILE=tests/am1/raw/m3f-t0.t0 bash tests/fanout/cost.sh`, containers $21.45), with no shard container running. Rows m3fr6 and their site day cost $14.50 (from $33.56 at 17:36:51Z). **Correction:** `tests/fanout/m2bcost.py` prices graft-m2b unless `INSTALL` names another install, and between these readings the meter was run without `env-m3f.sh` sourced, which read graft-m2b ($33.57 at 21:39Z, containers $8.79; `INSTALL=graft-m2b` reads the same). The m3fr6 progress figures sent to the parent thread ($33.41 to $33.52) were graft-m2b's. The earlier figures below came from rows-m3f.sh's own `cost` lines or a sourced shell, and are graft-m3f's [inference from the 17:36Z line, which matches: $33.56, containers $13.23].

**$32.90** at 04:45Z on 2026-11-15, after the 23:58Z site day, rows m3fr5b and the B0-31 rerun m3fr5c (`T0_FILE=tests/am1/raw/m3f-t0.t0 bash tests/fanout/cost.sh`, graft-m3f only, containers $13.23), with no shard container running. Earlier: **$26.38** at 02:29Z on 2026-11-15.

**$22.45** at 16:33Z on 2026-11-14, after the 15:45Z site day and rows m3fr4, with no shard container running. Earlier: **$17.14** at 13:07Z on 2026-11-14 (after m3fr2 and its 5 leaked shards; none running since 12:28Z). Earlier: metered with `tests/fanout/cost.sh` from `tests/am1/raw/m3f-t0.t0` (graft-m3f only): **$10.39** at 09:59Z on 2026-11-14, covering the 05:02Z site day, rows m3fr1 (B0-31, B0-32, L0-12 with its stall and refusals), the lone `Graftyard MCP` runs, and the image `sha256:4ab9de37…` push. Cap $40.
