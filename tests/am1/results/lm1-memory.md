# LM-1 memory on graft-m2b: what took members past 128 MB, and the read fixes (2026-11-10 to 11)

Branch `lm1-memory`, from `f51cc73` with `origin/main` `422bc02` merged (B0-30's shard fixes and the short-range
guard). Only `graft-m2b` was used. `graft-m1-*` and `graft-m3c-*` were not touched. Nothing is merged.

## Summary

- **Cause, measured:** whole-tier reads (`{"q": {"rows": true, "packed": true}}`) in flight on a repository's leader.
  Memory grew with the number of these reads running at once; held polls and per-request leaks did not grow it.
  Every lookup that missed its 3.5 s call deadline sent a `suspect` report. A slow leader therefore drew more reads
  and more reports, until the edge's gate answered 503.
- **The leaderless group (B0-35, 131215):** the leader's isolate was blocked for 641 s inside one alarm invocation.
  The third voter kept running out of memory, so the one healthy voter rarely had a peer to form a quorum with.
- **Fixes, in `f125a37`, `1263bfd`, `a0efa82` and `d9ee594`:**
  - a per-tier-state cache of the packed dump on the member;
  - no dump built for a read whose caller has gone;
  - `HEAVY_READS` (16) whole-tier reads at once, with an immediate 200 `overloaded` answer past that;
  - the lookup retries an `overloaded` answer without a `suspect` report or counting the member missed;
  - `suspect` reports damped per (chain, member, term, scope) every 500 ms.
- **Edge coalescing was built and removed.** Workers refuses a promise resolved in another request's context, so
  in the first 250-agent smoke 58,783 of 59,239 API reads answered 500 (section 5).
- **Rows: not passing.** On `d9ee594`, the 300 s 500-agent smoke held memory under 80 MB, but B0-29's 600 s run took
  the apac leader to 262 MB with an `exceededMemory` burst, and B0-29 and B0-32 both failed on commit p99 and
  leader changes. B0-31 was stopped early; B0-33, B0-34, B0-35 and the L0-12 A1 replay were not run (section 8).
- **What still fails:** leader moves from `StepDown Confirm` (storage confirm p99 1.7 to 2.1 s) and `CommitErrors`
  under about 300 req/s on the leader, and a second memory spike in which about 600 cheap reads complete in the same
  millisecond after a stall, then `exceededMemory` (section 8, hypothesis).
- **Cost:** $15.75 of the $25 cap by `tests/fanout/cost.sh` from `mem-t0`, containers $10.45 of it.

## 1. Cause

### Memory against reads in flight, one leader (`/tmp/par.mjs`, `/isolate`'s `wasmMemMb`)

Repository 131215 has 500 PRs, a packed dump of 1,176,150 B, and up to 7.8 s of CPU per read under load.

| Whole-tier reads in parallel | Leader wasm memory |
| --- | --- |
| 1 | 10.5 MB |
| 40 | 59 MB |
| 60 | 92 MB |
| 100 | isolate reset, all 100 failed |

On 131217 (dump about 230 KB), 10 to 200 parallel reads left memory flat at 49 MB. Throughput was capped:

| Load on 131217's leader | Result |
| --- | --- |
| Closed loop, 10 in flight | 28 reads/s |
| Closed loop, 30 in flight | 39 reads/s |
| Closed loop, 60 in flight | 46 reads/s |
| Closed loop, 100 in flight | the leader stepped down |
| Open loop, 80 reads/s | 813 in flight, p99 16 s, `StepDown CommitErrors` from CPU starvation |

Ruled out:
- held polls: 900 held `/feed` polls on a follower left its memory unchanged;
- per-request leaks: none seen;
- the earlier fixes (validate lock, read gate token cache, narrowed agent read, packed and trimmed dump): kept.

### On the failed rows (Workers Logs, `wallTimeMs` summed per route)

- **Reads in flight on the leaders:** `/read` averaged 40 in flight on B0-35's leader and 69.5 on B0-31's, with wall
  times up to 56 s.
- **`exceededMemory`:** came in bursts. Site c's arrived with 0 ms wall time, so the isolate was already over its
  limit when they arrived.
- **The `suspect` storm:** whenever a read missed `CALL` (3.5 s) or got a 5xx, `call_opt` reported the member with
  `suspect`, once per pass for every edge isolate. One leader received 87k `/read` and 108k `/suspect` in a run.
- **`busy`:** the member's own `busy` answer came only after `CALL_DEADLINE` × 3. That was too late, because the
  caller had already counted the member missed at 3.5 s.

## 2. The leaderless group (B0-35, 131215, 17:08 to 17:19Z on 2026-11-10)

f0 was not a wedged follower. It was the term-2 leader:
- It answered `/commit` and `/outcome_by_request` in 17:07, and only a leader answers those.
- It logged `prsync 131215 524..533 first handled` at 17:08:03, and only a leader runs the prsync alarm.

The block:
- From 17:07:17, f0's isolate stalled. At 17:08:06.474, 999 events ended in the same millisecond: 609 canceled
  `/read` (wall up to 49 s), 113 ok `/read`, 275 `/suspect`, and 2 `/raft`.
- One `alarm` invocation then started at about 17:08:15. It ended `ok` at 17:18:56.8 with wallTimeMs 641,631.
- That period has no other invocation over 60 s, and Workers Logs shows no f0 events at all from 17:09 to 17:17.
- f0's periodic metrics for minute 17:07 show cpuTime 622,240 ms and 41,928,052 rows read. That is about 57k rows
  per `/read` over 731 reads, or one tier dump for each read granted.

The mechanism:
- The read waiters (`core.reads`) outlived their canceled callers.
- The host built a dump for each granted read in `Resolution::Read` and sent it into a dropped receiver.
- One input that granted hundreds of reads at once (`check_reads`) blocked the isolate for minutes.
- Fixed in `f125a37` (`host.rs`, `Resolution::Read`): a read whose receiver is canceled builds nothing.

The other two voters:
- f1 ended its alarms in `exceededWallTime` at 17:09:14, 17:10:53, 17:14:04, and 17:18:43. Storage operations
  timed out (`object to be reset`), there were about 200 `fold storage lost` aborts, and `exceededMemory`
  outcomes came each minute.
- f2 got `/raft` only while f1 was briefly alive: 17:09:1x, 17:10:53, 17:11:07–16, and 17:14:0x–2x.
- f2 answered `/suspect` and `/read` within 0 to 2 ms, which is a verdict and not a quorum round. Its last append
  from f0 was at 17:08:06, so its promise lapsed by 17:08:16.
- f2 led a later term before teardown (`k.mjs` at 17:18:38 saw `leader 2`). Each leadership in the window died with
  f1.

Conclusion:
- Elections could not complete because no second voter answered for more than a few seconds.
- The election code was not the cause, so nothing in `crates/raft` changed.
- [unverified] The exact term changes inside the window, because no verdict bodies are logged.

Later, at 20:00Z with the group quiet, two `/suspect` reports elected a leader within 2.5 s.

## 3. Fixes

| Commit | What | Where |
| --- | --- | --- |
| `f125a37` | Packed dump cached per isolate under `DumpKey`, both tiers' `applied` and row-hash `sum` plus the metadata tier's `loaded`, read after the lease check, at most 8 repositories, cleared on fold storage loss | `workers/site/src/family.rs` |
| `f125a37` | No dump for a read whose caller has gone | `workers/site/src/host.rs`, `Resolution::Read` |
| `f125a37` | `HEAVY_READS` = 16 whole-tier reads at once on a leader; past it, `{"busy": true, "overloaded": {"retryAfterMs": 250}}` at once; `shedReads` and `heavyReads` in `/stats` | `workers/site/src/member.rs`, `read` |
| `f125a37` | `classify`: `overloaded` is retried on the same member after its delay, at most 4 times, never `missed`, never `suspect`; then `Err` naming `OVERLOADED`, which the edge answers 503 `Retry-After: 1`. `busy` is kept in the reply so an older lookup also skips it | `crates/lookup/src/lib.rs` |
| `f125a37` | `suspect` damped: one report per (chain, member, term, all-voters) per isolate per `SUSPECT_GAP` (500 ms) | `crates/lookup/src/lib.rs`, `suspect` |
| `1263bfd` | Edge coalescing removed (section 5) | `workers/edge/src/api.rs` |
| `a0efa82` | A whole-tier answer streams in 64 KiB pieces and holds its `HEAVY_READS` place until sent (section 6) | `workers/site/src/member.rs`, `held_body` |
| `d9ee594` | A stale dump is rebuilt at most once per `DUMP_EVERY_MS` (1 s) per repository; other granted reads answer `overloaded` with the ms until the next rebuild (section 6b) | `workers/site/src/family.rs`, `dump_wait`; `host.rs` |

No file under `crates/raft/src` changed, so the long sweep and the Porcupine checker were not needed.

The design moved first in each commit: `design/DESIGN.md`, Scale, journey 9 item 1; `design/AGENT-LAYER.md`,
Scale item 4; `decisions/DECISIONS.md`, "F65: a leader caches its packed dump"; the F65 row; and the BUILD-PLAN
AM1-WP6 status.

### Tests and plants

New tests:
- `crates/lookup`: `an_overloaded_answer_is_retried_and_never_missed` (the parent's "busy, retry, never suspect"
  requirement) and `suspect_reports_are_damped_per_member_and_term`;
- `workers/site` `member::tests`: `only_whole_tier_reads_on_a_leader_are_shed_past_the_cap`;
- `workers/site` `fanout::tests`: `a_cached_dump_never_outlives_its_tiers` and
  `dump_rebuilds_are_spaced_but_cache_hits_are_not`;
- `workers/site` `member::tests`: `a_streamed_answer_holds_its_read_until_sent`.

`tests/lm1mem/plant.py` caught 11 of 11 planted bugs once coalescing was removed. The 3 coalescing plants were also
caught before that removal.
- On `d9ee594`, `python3 tests/lm1mem/plant.py`: 14 of 14 caught, each by the test named in its line.

Full checks (`tests/am1/runs/checks.sh`) on `f125a37`:
- fmt, workspace clippy, and wasm32 clippy for site, edge, and ops, with and without `test-faults`: pass;
- `cargo test --workspace`: 674 passed;
- `go test`: pass;
- `check_names` and `check_links`: pass;
- `check_design`: passed after one fix (DESIGN.md had gone over its 2,200-line cap).

Full checks on `d9ee594`: fmt, workspace clippy, wasm32 clippy for site, edge and ops (with and without
`test-faults`), `cargo test --workspace` (673 passed), `go test`, `check_design` and `check_names` pass.
`check_links` fails in the working tree only because it walks the untracked `.worktrees/` build directory (82
lines, all under `.worktrees/`); on a clean `git archive` of `d9ee594` plus this file it passes, 2,944 citations.

## 4. What woke the idle shard containers

Teardowns did not stick: 56 shards torn down at about 19:3xZ were running again by 20:41Z, 34 of them.
`tests/fanout/reaper.sh`, a teardown every 60 s, then tore down 2 to 10 shards a minute with no load running.

What woke them, in order of weight:
1. **`PrSync` queue redeliveries, the largest source.**
   - Site a's `GitShard` received 189 `shard/merge` calls in 30 idle minutes (23:3x to 00:0xZ), all for 131211
     and 131216, B0-32's and B0-30-era repositories.
   - Each one ended `exception` after up to 131 s.
   - The queue consumer retries a failed batch (`max_retries: 10, retry_delay: 2`), and the leader's dispatcher
     sends unfinished work again after `REDELIVER_AFTER_MS` (30 s) for as long as it leads.
   - So a batch whose test merge cannot be computed keeps waking that network's shard.
2. **A new leadership warms its shard.**
   - Any wake of an old repository's group can start a new leadership, which creates a new `Dispatcher`, which
     calls `warm()`, which posts `shard/catchup`. The cron's fold audit, or a lookup's `suspect`, can wake the group.
   - At 20:00–20:04Z, site a's shards got two `shard/catchup` per repository for 131198–131214.
   - Many warms failed with 422 "did not receive expected object".
3. **The `*/15` cron's fold audit** (`fold_audit::run`, SAMPLE 4) reads old repository members (`/fold`, about 85
   per 30 minutes on site b) and so restores them. [inference] That is what starts item 2 for groups with no other
   traffic.

Other running instances:
- Networks 65536–65540 and 131072–131073, older installs' repositories, were running outside every teardown range
  `rows.sh` uses.

Suggested fix, not made here:
- the dispatcher should stop re-sending a batch after a bounded number of failed attempts;
- `warm()` should run only for a leadership that has PrSync work past its cursor;
- `teardown.py` should cover every `net:` instance.

## 5. Edge coalescing, built and removed

- **What it did:** `tier_rows` shared one `/read` among identical requests that arrived while another was in flight,
  and never the call already in flight, which keeps `fresh`.
- **Unit tests:** passed, on manually polled futures.
- **Hosted, it failed:** the 250-agent smoke on edge `db324659` answered 58,783 of 59,239 API reads with 500. The
  edge logged "A promise was resolved or rejected from a different request context than the one it was created in".
- **Why:** Workers refuses I/O objects and promises shared across requests.
- **Removed in `1263bfd`** (edge `86ce1cc6`). The member's dump cache serves the repeat reads instead: a cached read
  of 131215's 1.18 MB dump took 230 to 760 ms, against 960 to 1,330 ms uncached, with memory flat.

## 6. The answers' bodies (found in the 500-agent smoke)

In the 500-agent smoke on `1263bfd`, the leader moved from site a to site c (apac) at 00:45Z. Its memory went
from 64 MB to 178 MB, then 258 MB at 00:47, and it logged 102 `exceededMemory` outcomes at 00:48:20, 157 of them
on `/read`. The cap was holding, with `shedReads` 308 / 1,251 / 270 on f0 / f1 / f2, so no more than 16 answers
were being built at once.

What the cap did not count:
- Built answers still being sent. A 513 KB `{"value": "<packed>", ...}` body stays in the isolate until it has gone
  to the calling edge on site a, a trans-Pacific transfer.
- At 100 to 200 answers/s, many such bodies are in flight at once.
- The site a leader, near the callers, held 64 to 75 MB under the same load.

The fix, `a0efa82`:
- The body streams in 64 KiB pieces.
- The read keeps its place under `HEAVY_READS` until the last piece is taken or the stream is dropped (`held_body`).
- Test: `a_streamed_answer_holds_its_read_until_sent`. Plant 9: release at the first piece, which the test caught.

## 6b. Rebuilds (found in the 500-agent smoke on `a0efa82`)

In the 500-agent smoke on `a0efa82` (repo 131222, 01:47 to 01:52Z), the cap held, but the cache rarely hit: the tiers
changed with nearly every agent push, so each granted read rebuilt the dump. Workers Logs, per member, 01:45 to 01:56Z
(`/tmp/objcpu2.py`, `$workers.event.request.url`):

| Member | Route | Count | CPU sum | CPU max | Wall max |
| --- | --- | --- | --- | --- | --- |
| f0 (site a, leader until 01:49) | `/read` | 22,637 | 56.1 s | 285 ms | 2.9 s |
| f2 (site c, leader from 01:49) | `/read` | 4,812 | 23.1 s | 784 ms | 18.9 s |
| f2 | `/raft` | 389 | 10.5 s | 1,199 ms | 2.7 s |

The 1.2 s `/raft` inputs on f2 are reads granted by a quorum round and dumped inside that input. Memory per minute
(`durableObjectsPeriodicGroups`): f0 61 to 88 MB while leading, f2 96 MB at 01:50 and 212 MB at 01:51, with the
"fold storage lost" resets. The leader moved on `StepDown CommitErrors`.

The fix: a stale dump is rebuilt at most once per `DUMP_EVERY_MS` (1 s) per repository (`family::dump_wait`). It is checked
where the dump would be built, after the read's grant, so one granted batch rebuilds once and the rest answer
`overloaded` with the ms until the next rebuild is due. A read whose cached dump is at the tiers' state is served at
once. Test: `dump_rebuilds_are_spaced_but_cache_hits_are_not`. Plants 10 and 11.

### The 500-agent smoke on `d9ee594` (repo 131223, 02:37 to 02:42Z, 300 s)

| | `a0efa82` (131222) | `d9ee594` (131223) |
| --- | --- | --- |
| Peak member memory, per minute | f2 212 MB, with fold-storage-lost resets | f0 69 MB, f2 78 MB, f1 35 MB |
| Agent pushes ok | 1,063 of 1,256 | 1,237 of 1,242 (5 refused) |
| Commit p50 / p99 at the leader | 1,062 / 4,583 ms | 793 / 2,956 ms |
| `read:api` 200 / 503 | 29,094 / 2,852 | 43,872 / 471 |
| `/suspect` at the final leader | 10,151 | 1,768 |
| Leader | 0 → 2 | 0 → 2, term 4 |

`shedReads` at the end: f0 13,934, f1 1,327, f2 8,015. The leader still moved: f0 stepped down on `Confirm`
(storage confirm p99 2,070 ms, 6 slow of 18) at 02:38:49, then f1 on `CommitErrors` to f2 at 02:38:56. f2 logged
seven more `CommitErrors` step-downs with `to: None` between 02:39 and 02:41 and stayed leader. Memory is no longer the
failure. Commit latency and leader moves still are, through storage confirm stalls and the far site's commit errors.

## 6c. Design contradictions

- `design/DESIGN.md`, "Commit and read paths" (the `Fresh` comment): "Every `Fresh` read touches SQLite after its
  lease check, with no in-isolate fold cache, so a replaced instance cannot answer."
  - F65's dump cache is an in-isolate cache of fold rows.
  - It still reads both tiers' meta rows from SQLite after the lease check, and serves the cached dump only when the
    key matches, so a replaced instance still cannot answer. The rule's purpose holds; its words do not.
  - DESIGN.md is at its 2,200-line cap, so journey 9 item 1 records the exception and this line was left as is. An
    operator decision or a line elsewhere is needed to reword it.
- `DUMP_EVERY_MS` answers `overloaded` to a granted read whose dump is stale and whose rebuild is not due. That is
  shedding by age, not by load. The lookup then retries up to 4 times, 250 ms to 1 s apart. The design's "a read
  answers the state at its grant" still holds for every read that is answered.

## 7. Deployed

| What | Commit | Version |
| --- | --- | --- |
| Site day (test-faults, NOROLL, image `…@sha256:1722…f3e9`, `containers/` unchanged) | `f125a37` | two passes per the site-day script: a `b0d1cbc6` / `d0eb15ca`, b `e839a6cc` / `4a6645d6`, c `d3da69d3` / `852e65e7` (final) |
| Edge, with coalescing | `f125a37` | `db324659` |
| Edge, without coalescing | `1263bfd` | `86ce1cc6` |
| Site day | `a0efa82` | a `60a0a1bb`, b `c1a18453`, c `ffca4916`; edge `c8e9a566` |
| Site day (same image, NOROLL, test-faults) | `d9ee594` | first pass a `07c6b773`, b `957c6e4f`, c `a9bb4bd0`; final a `bd093c68`, b `9748eff1`, c `2abb792a`; edge `c1f5f7d0` |

Hosted check after the site day, on 131215's leader (1.18 MB dump):

| Whole-tier reads in parallel | Result |
| --- | --- |
| 1 | memory flat at 14.6 MB |
| 40 | all 40 ok |
| 100 | all 100 ok, max 2.5 s |
| 200 | 12 ok, 172 `overloaded` (`shedReads` 172), 16 `notLeader`; heavy reads in flight back to 0 after |

## 8. Rows

All on `d9ee594` (section 7), K = 4, `TEARDOWN=1`, `tests/fanout/rows.sh m5`. Logs:
`tests/am1/raw/runs/a1-<row>-fm5-<row>.log`.

| Row | Gate line | Measured | Verdict |
| --- | --- | --- | --- |
| B0-29 (131224, 600 s) | Commit p99 at most 600 ms, merge p99 at most 2 s, zero `Stale` on distinct refs, leader under 600 req/s with zero feed polls | Commit p99 3,867 ms (p50 1,000, n 2,077); leader 0 → 1 at term 7; `/suspect` 27,586 and `/campaign` 47 at the final leader; agent pushes 1,089 ok of 2,500, 666 `stale`, 406 timeouts, 205 `missed`; `read:api` 32,567 200 / 10,536 503; leader 195 req/s, 0 feed polls; apac member f2 262 MB at 02:54 | **Fail** (commit p99 over 1 s) |
| B0-32 (131225, 180 s) | Every push acknowledged within 5 s or answered `ng … agent admission full`, every merge in one of the next 2 entries, zero leader changes, zero `suspect` | Leader 0 → 1; `/suspect` 71; commit p99 4,834 ms; burst 48 ok, 452 refused; 31 agent pushes `fetch failed` | **Fail** (leader change, `suspect` reports, lost acknowledgments) |
| B0-31 (131226) | Commit p99 at most 1 s with buckets up | Stopped at about 03:17Z, before its row line | Not run |
| B0-33, B0-34, B0-35, L0-12 A1 replay | | | Not run: the 500-agent bar did not hold at 600 s, and the rows' cost would have passed the cap with no change to test |

B0-35's test-merge vCPU and list-field computes stay **not measured**. `MergeBudget::spent_ms` exists, but
`workers/site/src/prsync.rs` never calls `charge` and keeps its budget in an isolate `thread_local`, so reporting it
needs a new `/stats` field on the consumer, plus a site day. B0-35 itself was not run.

### What failed on B0-29 (Workers Logs, `durableObjectsPeriodicGroups`)

| Minute | f0 (a) MB / CPU s | f1 (b) | f2 (c) |
| --- | --- | --- | --- |
| 02:52 | 61 / 26.8 (leader) | 29 / 5.8 | 32 / 5.2 |
| 02:53 | 44 / 2.3 | 56 / 31.2 (leader) | 42 / 7.6 |
| 02:54 | 53 / 8.3 | 49 / 8.1 | 262 / 27.6 (leader) |
| 02:56 | 50 / 7.8 | 50 / 8.1 | 182 / 41.0 |
| 02:59 | 69 / 5.5 | 110 / 30.3 (leader) | 122 / 20.5 |

- Step-downs, from each member's `/stats` alarms:
  - f0 `Confirm { p99_ms: 1718, slow: 6, of: 19 }` to f1;
  - f1 `Confirm { p99_ms: 1951 }` to f2;
  - f2 five `CommitErrors` (for example 624 calls, 408 failed, to site 0) and then to f1;
  - f1 ten `CommitErrors` with `to: None` (up to 912 calls, 352 failed).
- f2 `exceededMemory`, 02:53 to 02:58: 112 `/commit` and 166 `/read`, all at 02:55:33.
  - At 02:55:32.713, 600 `/read` invocations ended in the same millisecond. Their wall p50 was 2,387 ms, and CPU over
    876 reads was 1,367 ms in total.
  - So these were not dump builds. The isolate stalled about 2.5 s, then completed the reads together, and then ran
    out of memory.
- **Hypothesis, not verified:** a quorum round grants every read waiting on it in one input (`check_reads` in
  `crates/raft/src/member/leader.rs`). The host then builds every answer `Value` in one loop
  (`Resolution::Read`, `workers/site/src/host.rs`) before any is sent.
  - With hundreds of refs pages (47 KB serialized) or access reads waiting, all their bodies are alive at once.
  - Probe: 600 parallel `{"q": {}}` reads on an idle leader raised its memory from 55.8 to 94.1 MB.
  - Evidence still missing: the query of each of the 600 reads, which Workers Logs does not record.
- Every probe burst of 1,000 or more narrow reads on the idle 131224 moved its leader (term 8 to 10). Read volume
  alone can now cost a leader its lease through `CommitErrors`, independent of memory.
