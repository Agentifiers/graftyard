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

## Rows m3fr6 (2026-11-15, TAG `m3fr6`, `ROWS="l0-12 b0-31 b0-32"`, `STOP_USD=46`)

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
| Feed: each event within 3 s at p99 outside leader kills; fail over 2 s | 19,286 events outside kills: p50 1,178 ms, **p99 2,954 ms** (all 20,818: p99 4,360, max 16,876) | Between the lines: under 3 s, over the 2 s fail line |
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

**Cap raised to $70** (2026-11-15, by the parent thread), with `STOP_USD=66` for rows.sh: graft-m3f was at $48.06 after m3fr6, past the earlier $50 cap's room for another site day and three rows. **The meter mix-up, recorded:** `tests/fanout/m2bcost.py` priced graft-m2b unless `INSTALL` named another install, and during m3fr6 the meter was run in a shell without `tests/am1/lf13/env-m3f.sh` sourced. It read graft-m2b ($33.41 to $33.52), and those figures went to the parent as graft-m3f's; rows.sh's own lines, which source it, were right ($45.75 and $47.99). From now on every metering command sources `env-m3f.sh`, and the meter's line ends with the install and the Cloudflare account it priced (`"install": "graft-m3f", "account": …`, branch `promote-unlock`).

**$48.06** at 21:39Z on 2026-11-15, graft-m3f only, after the 17:18Z site day, the new image and rows m3fr6 (`source tests/am1/lf13/env-m3f.sh; T0_FILE=tests/am1/raw/m3f-t0.t0 bash tests/fanout/cost.sh`, containers $21.45), with no shard container running. Rows m3fr6 and their site day cost $14.50 (from $33.56 at 17:36:51Z). **Correction:** `tests/fanout/m2bcost.py` prices graft-m2b unless `INSTALL` names another install, and between these readings the meter was run without `env-m3f.sh` sourced, which read graft-m2b ($33.57 at 21:39Z, containers $8.79; `INSTALL=graft-m2b` reads the same). The m3fr6 progress figures sent to the parent thread ($33.41 to $33.52) were graft-m2b's. The earlier figures below came from rows-m3f.sh's own `cost` lines or a sourced shell, and are graft-m3f's [inference from the 17:36Z line, which matches: $33.56, containers $13.23].

**$32.90** at 04:45Z on 2026-11-15, after the 23:58Z site day, rows m3fr5b and the B0-31 rerun m3fr5c (`T0_FILE=tests/am1/raw/m3f-t0.t0 bash tests/fanout/cost.sh`, graft-m3f only, containers $13.23), with no shard container running. Earlier: **$26.38** at 02:29Z on 2026-11-15.

**$22.45** at 16:33Z on 2026-11-14, after the 15:45Z site day and rows m3fr4, with no shard container running. Earlier: **$17.14** at 13:07Z on 2026-11-14 (after m3fr2 and its 5 leaked shards; none running since 12:28Z). Earlier: metered with `tests/fanout/cost.sh` from `tests/am1/raw/m3f-t0.t0` (graft-m3f only): **$10.39** at 09:59Z on 2026-11-14, covering the 05:02Z site day, rows m3fr1 (B0-31, B0-32, L0-12 with its stall and refusals), the lone `Graftyard MCP` runs, and the image `sha256:4ab9de37…` push. Cap $40.
