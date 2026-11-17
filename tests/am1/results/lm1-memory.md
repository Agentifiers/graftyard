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
- **Round 2 (`80d8837`, section 9):** every read is capped at `READS_MAX` (256), and granted answers are built once per
  query as shared bytes that stream out.
  - At 500 agents, no member logged `exceededMemory` in any row (B0-29, B0-32, B0-31, B0-34 and B0-35).
  - B0-33, at 5,000 agents, still did (651 on one member).
  - Every row still fails its gate. Commit p99 is 3.2 to 7.3 s and leaders still move, because reads saturate the
    leader's one thread.
  - B0-32 had zero leader changes and zero `suspect` reports.
  - L0-12's A1 replay was not run: the round hit its $10 cap.
- **Round 3 (follower reads, `54c45213`, section 10):** API tier reads and the read gate's rows go to followers by
  read-index. At 500 agents the leader's read load fell to about a third of read invocations, B0-29 held its first
  leader 7 minutes, B0-32 kept its leader, and no member ran out of memory in B0-29, B0-31, B0-32, B0-33 or B0-34.
  Every row still fails its gate: leaders still step down on `CommitErrors` and `Confirm`, because the edge sends a
  read both followers shed to the leader, and followers saturate at about 10,000 reads a minute. B0-35's f1 ran out of
  memory (812 `/read`), and L0-12's feed p99 was 64 s.
- **Cost:** round 1 $15.75 of the $25 cap by `tests/fanout/cost.sh` from `mem-t0`, containers $10.45 of it; round 2
  $10.23; round 3 $6.39 of its $10 cap.

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

## 6d. Round 2: every read capped, granted answers built once and streamed (`80d8837`)

The B0-29 burst (section 8) pointed at the grant: one quorum round grants every read waiting on it
(`check_reads`, `crates/raft/src/member/leader.rs`), and the host built every answer as a `serde_json::Value` in
that input, so every tree was alive until its read was sent. The probe of 600 parallel `{"q": {}}` reads on an
idle leader took it from 55.8 to 94.1 MB.

A grant holds only until the member's next input (`ReadGrant`: "only before the member's next `step`"), so the
answer cannot be built later from the fold. `80d8837` changes what is kept instead:
- `host::ReadBody::Json(Rc<[u8]>)`: the answer is serialized once, and the `Value` tree is dropped in the input.
- `host::shared_answer`: reads of the same query granted in the same input share one `Rc<[u8]>`. `/stats` reports
  this as `sharedReads`.
- `member::held_body` streams `{"value":` + the shared bytes + `,"index":…,"meta":…}` in 64 KiB pieces, copying each
  piece out only when taken.
- `READS_MAX` = 256: reads of any kind in flight on a member, counted from arrival until the last piece is taken.
  Past it, the member answers the same 200 `overloaded` that the lookup retries without `suspect`. `HEAVY_READS`
  (16) still holds for whole-tier reads on a leader. `/stats` reports `readsInFlight`.

Tests:
- `every_read_is_shed_past_reads_max_and_whole_tier_ones_past_the_heavy_cap`;
- `reads_granted_together_share_one_answer_per_query`;
- `a_streamed_answer_holds_its_read_until_sent`, rewritten for the three-part body.

`tests/lm1mem/plant.py`: 19 of 19 caught. Plants 6 to 10, 12 and 13 are new: a follower caps whole-tier reads,
narrow reads go uncapped, `READS_MAX` off by one, no sharing, sharing keyed by nothing, the shared value skipped,
and release at the first piece.

Full checks on `80d8837`, with `origin/main` `c1db5ac` merged in: fmt, every clippy, `cargo test --workspace` (674
passed), `go test`, `check_design` and `check_names` pass. `check_links` passes on a clean `git archive` (2,945
citations).

### What bounds the step-downs (evidence, before round 2's rows)

`STEP_DOWN` is `{ window_s: 10, commit_errors: 0.05, commit_fail_min: 3, confirm_p99_ms: 500,
confirm_slow_min: 6 }` (`crates/raft/src/lib.rs`):
- A leader steps down when its storage confirms in the last 10 s have a p99 over 500 ms with 6 slow;
- or when more than 5% (at least 3) of its AppendEntries to every follower miss `CALL_DEADLINE` (1 s).

Both are measured on the leader's own isolate:
- `confirm_begin`/`confirm_end` time `storage().sync()` from a `spawn_local`, so a confirm is slow when the isolate
  is busy, not only when storage is.
- A call "misses" when its reply is not stepped within 1 s, which also happens when the isolate cannot run the reply.

On B0-29's site a leader (131224 f0, Workers Logs per second, 02:53:26 to 02:53:36, just before its `Confirm`
step-down at 02:53:36):
- 187 to 774 invocations a second;
- 300 to 712 ms of CPU a second, almost all in `/read`;
- wall maxima 1 to 2.5 s.

The site c leader at 02:59:14 to 02:59:31 ran 592 ms of CPU in one second (416 in `/read`), and invocations in the
same second waited up to 3.5 s. So the confirms and the calls were late because the isolate's one thread was
saturated by reads. The raft rule is doing what it says: a leader that cannot run its own inputs in time is sick.
This is [inference] from CPU-per-second and wall, not from a trace of one confirm.

Round 2 bounds the reads in flight but not their CPU. If the step-downs remain:
- The next fix is the reads' CPU, not raft: move the edge's API reads off the leader to followers by read-index,
  which DESIGN already allows for feed polls ("Feed polls never reach the leader").
- Or make `HEAVY_READS` and `READS_MAX` adapt to the leader's own confirm p99.
- Excusing busy-isolate confirms in `STEP_DOWN` is a `crates/raft` change and would be reported first.

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

## 9. Round 2 rows (`80d8837`)

All on `graft-m2b`, sites a `fd5cedf3`, b `2134ec8b`, c `d0964d27`, and edge `0f8a0f84`, from the site day at
04:46Z. `origin/main` `c1db5ac` merged. `rows.sh m6` with K = 4, `TEARDOWN=1` and `STOP_USD=9.3`, from the window
starting at 03:35Z (`tests/fanout/raw/mem2-t0.t0`). The idle-shards thread
(T-01a1c8aa-e61f-7739-bd38-27cb59655101) held `graft-m2b` for its baseline until 04:45Z, and `graft-m2b` passed
back to it at 06:38Z.

Memory, from `durableObjectsPeriodicGroups` (peak MB per member) and Workers Logs outcomes other than `ok`:

| Row (repo) | f0 / f1 / f2 peak MB | `exceededMemory` | Other non-ok outcomes |
| --- | --- | --- | --- |
| Smoke 500, 300 s (131227) | 56 / 35 / 108 | 0 | 47 `canceled` |
| B0-29 (131228) | 73 / 203 / 167 | 0 | f1: 190 `exception` at 05:19:55, "Internal error in Durable Object storage caused object to be reset" |
| B0-32 (131229) | 98 / 34 / 30 | 0 | 28 `canceled` |
| B0-31 (131230) | 104 / 61 / 105 | 0 | 92 `canceled` |
| B0-33, 5,000 agents (131231) | 150 / 262 / 89 | f1: 651, at 06:02:03 to 06:02:05 (330 `/read`, 212 `/commit`, 87 `/suspect`) | f1 141 `exception` |
| B0-34 (131232) | 114 / 45 / 120 | 0 | 101 `canceled` |
| B0-35 (131233) | 115 / 193 / 101 | 0 | f1 34 `aborted` |

In round 1, B0-29 alone had hundreds of `exceededMemory` outcomes, and the apac leader was at 262 MB.

Rows against their gate lines, from `tests/am1/raw/a1-<row>-<run>.jsonl`:

| Row | Gate (pass / fail) | Measured | Verdict |
| --- | --- | --- | --- |
| B0-29, 600 s | Commit p99 ≤ 600 ms, merge p99 ≤ 2 s, zero `Stale` on distinct refs, leader < 600 req/s with zero feed polls / commit p99 > 1 s | Commit p99 4,699 ms (p50 2,162, n 461); leader 0 → 2, term 7; `/suspect` 12,856; agent pushes 2,426 of 2,500 ok, 51 timeouts; `read:api` 58,063 200 / 8,181 503; 96 req/s, 0 feed polls | **Fail** |
| B0-32, 180 s | Every push acked within 5 s or `ng … admission full`, merges in the next 2 entries, zero leader changes, zero `suspect` / any lost ack, leader change or `suspect` | **Zero leader changes (term 2), zero `suspect`**; commit p99 3,216 ms; agent push p99 36,660 ms; burst 112 ok, 384 refused, 4 `fetch failed`; 5 agent pushes `fetch failed` | **Fail** (pushes past 5 s; 9 `fetch failed`, unacknowledged at the client) |
| B0-31, 600 s | Commit p99 ≤ 1 s with buckets up / > 3 s | Commit p99 7,324 ms; leader 0 → 1, term 10; agent pushes 805 ok, 1,623 refused | **Fail** |
| B0-33, 300 s, 5,000 agents | Human and merge-queue p99 ≤ 1 s, ≥ 140 agent commands/s admitted, zero leader changes and `suspect` / any leader change or `suspect`, < 100 commands/s | Commit p99 21,442 ms; leader 0 → 1, term 17; `/suspect` 43,591; agent pushes refused 3,182 and timed out 1,246; f1 `exceededMemory` | **Fail** |
| B0-34, 600 s, shard kill at 300 s | k in band ≤ 8, human push p99 ≤ 3 s beyond transfer, clone TTFB p99 ≤ 1 s / TTFB p99 > 5 s, k > 8 | k stepped 1 → 2 in band (repo 131232: "k 1 at 4" → "k 2 at 652"); clone TTFB p99 26,965 ms (p50 2,196); human push p99 240 s; commit p99 3,727 ms; leader 0 → 2, term 4 | **Fail** (TTFB) |
| B0-35, 600 s, 500 open PRs | Test-merge vCPU within budget, Actions dispatch p99 ≤ 10 s, zero computes from list fields | `read:mergeable` 4,475 `MERGEABLE` and 35,036 503 of 57,957; `read:api` 2,266 200 / 37,551 503; leader 0 → 1, term 9. Test-merge vCPU, list-field computes and dispatch p99 are **not measured** (`MergeBudget::charge` is a separate work item) | **Not judged**; the reads it measured mostly answered 503 |
| L0-12 A1 replay | | Not run: `STOP at $10.23` | Not run |

### The step-downs after the read cap

They remain in every row except B0-32. What bounds them, with evidence:
- The rule is `STEP_DOWN` (section 6d): confirm p99 over 500 ms with 6 slow in 10 s, or more than 5% of
  AppendEntries to every follower missing 1 s.
- On the smoke's leader (131227 f0, Workers Logs per second, 04:59:48 to 05:00:05), before
  `StepDown { Confirm { p99_ms: 1835 } }` at 05:00:03:
  - 85 to 838 invocations a second, with 60 to 942 ms of CPU a second, almost all `/read`;
  - wall maxima of 1.3 to 3.0 s.
- The reads themselves are cheap: of 1,526 `/read`s in 04:59:48 to 04:59:52, 1,411 took 0 to 2 ms of CPU. Their
  count is the problem: 5,660 `/read`s in 5 minutes on f0 cost 16.6 s of CPU.
- `shedReads` reached 35,630 on one member in B0-29. Each shed answer is one more invocation on the leader, and the
  lookup asks again up to 4 times.
- The memory fix therefore moved the failure from `exceededMemory` to CPU: the leader's one thread spends most of
  each second on reads, so its storage confirms and AppendEntries replies are stepped late.
- [inference] This comes from CPU per second and wall time; no single confirm was traced.

Next fix (not built; no raft change made):
1. Take the edge's API reads (`read:api`, 200/s, the largest load) off the leader: serve them from followers by
   read-index, as DESIGN already does for feed polls ("Feed polls never reach the leader").
   - That splits the read CPU across three isolates.
   - It is a lookup and edge change, not a raft change. `read_index` exists (`Member::read_index`).
2. Make an `overloaded` answer cost the leader less. The lookup could wait the full `retryAfterMs` before its first
   retry and try a follower's read-index instead of the same leader. Today 4 retries on the leader multiply the
   invocations at the worst moment.
3. Only if 1 and 2 leave confirm stalls: excuse a confirm that is slow because the isolate was busy. That changes
   `crates/raft` `STEP_DOWN` and needs the sweep and Porcupine, so it would be reported first.

### Cost, round 2

`tests/fanout/cost.sh` with `T0_FILE=tests/fanout/raw/mem2-t0.t0` (03:35Z): **$10.23** at the stop, containers
included. About $1.9 of it accrued before the site day, mostly idle shard containers waking during the idle-shards
baseline. Rows: B0-29 $1.33, B0-32 $0.40, B0-31 $2.34, B0-33 $0.79, B0-34 $1.33, B0-35 $1.48. Rounds 1 and 2
together: $25.98.

## 10. Round 3: follower reads (`7bc0ed22`, deployed as `54c45213`)

### What changed

Round 2 bounded memory but every row failed on leader CPU: reads saturated the leader's one thread, so its storage
confirms and AppendEntries replies stepped late and it stepped down (section 9). Round 3 moves the edge's API tier
reads off the leader, as journey 9 item 1 and the new DESIGN rule ("*Follower read.*") now say:
- The edge's whole-tier and merge-row reads (`tier_rows`) and the read gate's access and agent rows (`core_rows`) go
  to a follower first, then the other follower, then the leader (`graftyard_lookup::read_spread`). A follower answer
  is used only if it is a value; `notLeader`, `overloaded`, `busy`, a 4xx or 5xx, or no answer within
  `FOLLOWER_CALL` (2 s) moves on, and none of them is reported with `suspect`.
- A follower serves such a read by read-index (`Member::read_index`): the leader confirms its commit index by lease or
  round, and the follower answers once it has applied that index, so the read is `Fresh`. One whose metadata tier is
  loading names the leader. One that has not applied the index within `FOLLOWER_WAIT` (1 s) answers `overloaded`,
  never an older fold.
- `READS_MAX`, `HEAVY_READS` and the dump cache hold on followers too (`member::gate`). Deps, refs, `ranges`,
  `/entries`, `prsync`, `mergequeue` and alarm reads stay on the leader (`family::follower_query`).
- `/stats` adds `followerReads`, `followerRefused` and `followerLate`.
- No change to `crates/raft/src`.

Tests: `follower_reads_skip_the_leader_and_rotate` and `only_a_followers_value_is_used` (lookup);
`every_read_is_shed_past_reads_max_and_whole_tier_ones_past_the_heavy_cap` (rewritten) and
`a_follower_serves_only_its_reads_under_the_caps_and_never_stale` (site); `a_follower_serves_only_the_designs_reads`
(fanout); `a_follower_behind_grants_no_read_from_its_older_fold` (`crates/fold/tests/install.rs`: a cut follower grants
nothing and fails at `CAMPAIGN_DEADLINE`, then sees the new write after it reconnects). `tests/lm1mem/plant.py`: 29 of
29 caught. Full checks on `7bc0ed22` and again on `6cdff1b3` (idle-shards merged): fmt, every clippy, `cargo test
--workspace` (686 passed), `go test`, `check_design`, `check_names`; `check_links` passes on a clean `git archive`
(2,978 citations).

### Deployed

| What | Commit | Version |
| --- | --- | --- |
| Site day (test-faults, NOROLL, image `…@sha256:1722…f3e9`, `containers/` unchanged since `80d8837`), 08:50 to 08:58Z | `54c45213` (`lm1-memory` with `origin/idle-shards` `f6d440a` merged) | first pass a `8a327e48`, b `729d21e8`, c `044474f8`; final a `532e021f`, b `27678409`, c `29a6ebfa` |
| Edge | `54c45213` | `1e11c400` |

The PrSync queues were purged after the site day (08:58Z) and before the smoke and the rows, as the idle-shards
thread found an old release can leave thousands of messages on them. `743ad0cc` merges `origin/main` `f3d99ed` after
the rows started; it changes only the order of two DECISIONS entries, so the deployed code is the same.

### Rows

Smoke at 500 agents (131237, 300 s) then `tests/fanout/rows.sh` (`m7` for B0-29, `m8` for the rest) with K = 4,
`TEARDOWN=1`, `STOP_USD=9.3`, cost window from `tests/fanout/raw/mem3-t0.t0` (08:50Z). Peak MB per member from
`durableObjectsPeriodicGroups`.

| Row (repo) | Gate (pass / fail) | Measured | Verdict |
| --- | --- | --- | --- |
| Smoke 500, 300 s (131237) | — | Peak 75 / 55 / 112 MB, 0 `exceededMemory`; leader moved at load start: f0 `StepDown CommitErrors` at 09:05:26 (15 s into load), then f2 held with ten `CommitErrors` holds; commit p99 5,675 ms | — |
| B0-29, 600 s (131238) | Commit p99 ≤ 600 ms, merge p99 ≤ 2 s, zero `Stale` on distinct refs, leader < 600 req/s with zero feed polls / commit p99 > 1 s, any queue merge > 10 s, any feed poll on the leader | Commit p99 3,419 ms (p50 662, n 8,167); leader 0 → 1 → 2, term 4, first step-down at 09:38:43 (7 min into load); `/suspect` 7,774 on the old leader; agent pushes 2,412 of 2,500 ok, 61 timeouts, 10 `stale`; merges p99 9,303 ms; `read:api` 69,545 200 / 4,862 503; 306 req/s; 0 feed polls on the leader; peak 145 / 68 / 231 MB, 0 `exceededMemory` on the members | **Fail** (commit p99) |
| B0-32, 180 s (131239) | Every push acked within 5 s or `ng … admission full`, merges in the next 2 entries, zero leader changes, zero `suspect` / any lost ack, other refusal shape, object reset from memory, merge > 2 s, leader change or `suspect` | **Zero leader changes (term 2), 3 `suspect`**; commit p99 2,396 ms; burst 168 ok, 332 `refused` (not the `admission full` shape), 1 within 5 s; 13 agent and 3 human pushes `fetch failed`; merge p99 41,005 ms; peak 90 / 48 / 50 MB | **Fail** (refusal shape, pushes past 5 s, merges, 3 `suspect`) |
| B0-31, 600 s (131240) | Commit p99 ≤ 1 s with buckets up / > 3 s | Commit p99 2,541 ms (p50 745, n 8,073); leader 0 → 1, term 7 (two moves, at 10:02:05 `CommitErrors` 3 of 53 and 10:10:26 `Confirm` p99 1,377 ms); agent pushes 967 ok, 1,427 refused, 96 × 503 from the read gate; peak 114 / 115 / 93 MB | **Fail** (commit p99 between the lines) |
| B0-34, 600 s, shard kill at 300 s (131241) | k in band ≤ 8, human push p99 ≤ 3 s beyond transfer, clone TTFB p99 ≤ 1 s / TTFB p99 > 5 s, k > 8 | k 1 → 2 in band ("k 1 at 4" → "k 2 at 810"); clone TTFB p99 58,554 ms (p50 1,643); human push p99 240 s; commit p99 2,138 ms; leader 0 → 1 → 0, term 5 (`Confirm` p99 1,526 ms at 10:26:07, then 15 `CommitErrors` holds); agent pushes 1,237 ok, 1,168 timeouts; peak 141 / 67 / 168 MB | **Fail** (TTFB) |
| B0-33, 300 s, 5,000 agents (131242) | Human and merge-queue p99 ≤ 1 s, ≥ 140 agent commands/s, zero leader changes and `suspect` / any leader change or `suspect`, < 100 commands/s | Term 2 → 19 in 5 minutes; `/suspect` 27,703; commit p99 15,980 ms (n 637); agent pushes 127 ok of 12,523 (5,122 + 1,803 × 503 from the read gate, 4,148 timeouts); peak 223 / 102 / 98 MB, 0 `exceededMemory` on members | **Fail** |
| L0-12 A1 replay, 600 s, leader kill every 60 s (131243) | No lease overlap over 500 ms, 500 long-pollers see each event within 3 s at p99 outside leader kills, 100 of 100 refused for the agent kinds / feed p99 over 2 s, any overlap, a refusal count off | 500 tasks and 500 leases set up (26 min of setup before the window); leader stayed f0 through 10 leader kills, term 2 → 5, with 9 `StepDown` holds (`to: None`, 8 `CommitErrors`, 1 `Confirm` p99 1,015 ms); feed p99 64,031 ms (p50 2,647, 19,472 events); commit p99 4,675 ms (p50 1,154, n 948); agent pushes 2,181 of 2,490 ok, 199 timeouts; `read:api` 28,629 200 / 21,935 503; lease renewals 540 200 / 545 503; f0 peak 275 MB with 711 `aborted` `/read` at 11:15:54, f1 66, f2 69 | **Fail** (feed p99) |
| B0-35, 600 s, 500 open PRs (131244) | Test-merge vCPU within `TEST_MERGE_BUDGET`, Actions dispatch p99 ≤ 10 s, zero computes from list fields / over budget, dispatch p99 > 30 s | 500 of 500 PRs opened; `read:mergeable` 3,876 `MERGEABLE` and 24,168 × 503 of 58,994 (30,950 skipped by the client cap); `read:api` 3,420 200 / 25,404 503; leader 0 → 1 → 2 → 1 → 0, term 2 → 8, 24 `StepDown` alarms (mostly `CommitErrors` holds on f1 at 11:33 to 11:37); commit p99 5,830 ms; agent pushes 1,464 of 2,500 ok; f1 812 `exceededMemory` on `/read` at 11:35:36 (peak 252 MB), f0 88, f2 102. Test-merge vCPU, list-field computes and dispatch p99 **not measured** (`MergeBudget::charge` is its own work item; no Actions on `graft-m2b`) | **Not judged on its gate**; what it measured failed (mergeable reads mostly 503, a member out of memory) |

Members logged no `exceededMemory` in the smoke, B0-29, B0-32, B0-31, B0-34, B0-33 or L0-12. B0-35's f1 logged 812,
all `/read`, at 11:35:36, after its peak of 252 MB; L0-12's leader f0 reached 275 MB with 711 `aborted` `/read` at
11:15:54. Site a also logged 2,341 `exceededMemory` outcomes on two git cache shard objects (`git#…` for 131240 and
131239, `/upload-pack` and `/validate`), not on any group member.

### Follower reads, as measured

On B0-29 (`/stats` at the end, 131238):

| Member | Role at the end | `/read` in | `followerReads` | `followerLate` | `shedReads` | `sharedReads` |
| --- | --- | --- | --- | --- | --- | --- |
| f0 | follower (led to 09:38:43) | 156,820 | 4,547 | 3,575 | 51,510 | 724 |
| f1 | follower | 101,677 | 22,684 | 6,658 | 72,399 | 12,709 |
| f2 | leader from 09:38:49 | 37,285 | 0 | 8 | 25,003 | 1,743 |

`followerRefused` was 0 on every member in every row: no follower was asked for a read it may not serve. B0-31 was the
same shape (f1 led with 20,587 follower reads taken before it led; f2 16,546). Before the first step-down on B0-29 the
followers took about 10,000 `/read` a minute each and the leader about 8,800 (17,653 in 09:33 to 09:35).

### What still moves the leader

Follower reads did what they were for: the leader's read CPU halved and B0-29 held its first leader for 7 minutes
instead of seconds, B0-32 held its leader, and no member ran out of memory. But leaders still step down in B0-29,
B0-31, B0-34 and B0-33, on `CommitErrors` and on `Confirm`. What bounds them, with evidence:

- **The leader still serves most reads.** On B0-29 f0 took 17,653 `/read` in 09:33 to 09:35 (31.9 s of CPU, 0 to 2 ms
  each for 90%), against 1,168 on a follower (10.2 s). A follower answer that is `overloaded` (sheds, 51,510 to 72,399
  per member) or late (`followerLate`, 3,575 to 6,658) sends the edge on to the leader, so the leader still receives
  the overflow of both followers, retried up to 4 times. Read CPU on the B0-31 leader in one 4-minute window: 73.5 s of
  the 77.7 s it spent on member routes (`/read` 35,805 invocations).
- **Per second, the leader is busy, not stalled by one input.** On B0-31's leader at the `Confirm` step-down
  (10:10:00 to 10:10:27) it ran 166 to 516 invocations a second at 200 to 950 ms of CPU a second, wall maxima 0.6 to
  2.8 s; no invocation over 300 ms of CPU. The isolate's one thread is saturated by hundreds of 1 to 2 ms reads.
- **The `CommitErrors` holds are a follower going quiet, not the leader.** After f2 took B0-29 at 09:38:49, its
  calls failed to both followers at once (`(SiteId(0), 1125, 484), (SiteId(1), 1128, 486)`), and f0's `/stats` show
  `Durable Object is overloaded. Requests queued for too long.` on its own `readIndex` calls to f1. Followers took 150
  to 600 invocations a second themselves, mostly `/read` and the 7,774 `/suspect` reports the old leader's readers
  sent. f2 also logged 450 "Internal error in Durable Object storage caused object to be reset" at 09:39 to 09:40 and
  one `/read` of 3,891 ms CPU (a dump build on a newly elected leader).
- **Follower read-index costs the leader little.** The first leaders of B0-29 and B0-31 took 2,790 and 3,189
  follower `ReadIndex` batches (about 5 a second) and answered all but 2 of each inside the call, from the lease
  (`trace`: `ResSent` in the answer), so they forced no AppendEntries round. Batching them further would not help.
- **B0-33 is the read gate.** At 5,000 agents the gate's access-row reads and agent pushes overran the members: 6,925
  pushes were answered 503 "the repository's access rows did not answer", and the leader moved 17 times. Its peak
  of 223 MB on f0 came at 10:36, with no `exceededMemory`.

So the step-downs are bounded by the leader's thread, which still takes about a third of read invocations plus every command, and by
followers that saturate in turn once they serve 10,000 reads a minute. What would remove them, in order:
1. **Let the edge stop retrying an `overloaded` follower on the leader.** Today `read_spread` falls through to
   `call_opt` on the leader after both followers shed, so overload anywhere lands on the leader. Answering the client
   503 with `Retry-After` there (or retrying the followers after `retryAfterMs`) keeps the leader for commands and
   refs. Edge change, no raft change.
2. **Give followers headroom before the leader takes their overflow.** Followers shed 51,000 to 78,000 reads a row
   at `READS_MAX` and `HEAVY_READS` while serving 10,000 a minute; learners (DESIGN: "Above 500 reads a second,
   learners serve them too") would spread the same reads wider. Placement change, no raft change.
3. **Cache the read gate's access decision per repository and minute at the edge** for B0-33's 5,000 agents, the way
   installation-token decisions are cached by `jti`.
4. Excusing busy-isolate confirms in `STEP_DOWN` stays the last resort (a raft change, reported first).

### The two loose ends from the runs

- `a1.mjs` held sockets open for up to 5 minutes after writing its row on B0-29 and B0-31, which stalled `rows.sh`.
  B0-31's driver was killed at 10:17 after its row was written (its `exit 143` in `rows-m8.status`). The driver now
  calls `process.exit(0)` after the row (`tests/am1/rest/a1.mjs`).
- `rows-m7` was stopped after B0-29 for the same stall, so B0-29 is in `rows-m7` and the other rows in `rows-m8`.

### Cost, round 3

`tests/fanout/cost.sh` with `T0_FILE=tests/fanout/raw/mem3-t0.t0` (08:50Z, site day start): **$6.39**, containers
$1.76 of it, at 11:42Z after the last teardown. Cumulative at each row's end, from the status lines: smoke
$0.36 (09:12Z), B0-29 about $1.37 (09:51Z, before B0-32), B0-32 $1.63, B0-31 $2.50, B0-34 $3.30, B0-33 $3.87, L0-12
$5.37 (with 26 minutes of setup), B0-35 $6.39. Rounds 1 to 3 together:
$32.37. Every network from 131237 to 131245 is torn down (8 instances, all 200).

## 11. Round 4: no leader fallback, learners for reads (`lm1-memory` after `6fe479e0`)

### What changed

Round 3's rows failed because a read both followers shed went on to the leader by `call_opt`, retried up to four
times, so overload anywhere landed on the leader's thread (section 10). Round 4, design first (DESIGN "*Follower
read.*", Scale "Hot group reads, learners", journey 9 item 1; AGENT-LAYER A1 step 4; DECISIONS "F65 follow-up: a read
the members shed is 503, never the leader's"):
- **Edge 503 fallback.** `graftyard_lookup::read_spread` asks at most three members (`SPREAD_TRIES`), learners first,
  then followers, never the cached leader. If none answers a value and any sheds it (`overloaded`, `busy`, a 5xx, or
  no answer within `FOLLOWER_CALL`), it fails with an error naming `OVERLOADED` and the edge answers 503 with
  `Retry-After: 1` (`api::read_failed`; 5 s for any other failed read, on the REST, LFS and git read-gate paths).
  Only a read every member asked declines as not its own (`notLeader`, or a 4xx such as a group not created yet)
  goes to the leader (`spread_verdict`). Deps, refs, `ranges` and `/entries` are unchanged.
- **Learners for reads.** `Known` keeps the config's learners with its voters; a value whose `meta.configIndex` is
  past the isolate's config makes it read that member's `/status` once (`refresh_config`). Learners already held the
  same caps: `member::gate` applies `READS_MAX`, `HEAVY_READS` and the dump cache to any index read on a non-leader,
  and `read_wait` answers `overloaded` after `FOLLOWER_WAIT` to one that has not applied the leader's index.
  `tests/fanout/learners.mjs` places them with the leader's `/learners` route; `rows.sh` runs it before `k.mjs` when
  `LEARNERS` is set.
- No change to `crates/raft/src` or the site script.

### Learners, as measured before the rows

One learner (slot 0, enam, beside the leader) added to old groups on `54c45213`, driven from the orb through the
tools edge (`/tmp/ipar.mjs`, parallel `/read` with `index: true`):

| Group | Reads | Result |
| --- | --- | --- |
| 131244 (500 PRs) | 100 × 10 narrow `access`, one client | 1,000 of 1,000 values, 244 reads/s |
| 131244 | 300 × 3 narrow | 825 values, 75 `overloaded` (`READS_MAX` 256), 408 reads/s |
| 131238 (B0-29's, 958 KB dump) | 100 × 10 narrow | 1,000 values, 276 reads/s, 0 late, 0 shed |
| 131238 | 16 × 8 whole-tier, three clients at once | 318 values, 66 `overloaded` (`HEAVY_READS`), 17 to 25 reads/s each |

The leader answered every learner batch from its lease (`trace`: `IndexTaken` then `ResSent` in the same ms, batches
50 to 100 ms apart), so a learner costs the leader at most 20 calls/s, as DESIGN says. Whole-tier reads of journey 9's
1 MB tiers are the binding cost on a learner as on a follower, which is why DESIGN now weights them.

**A new learner refuses reads until an append reaches it.** A learner's `leader` is set only by an `Append`
(`follower.rs`), so after `/learners` it answered every read `notLeader` naming no leader until the next commit. An
idle learner also lost the leader after about 25 s: its isolate restarted (a new `boot` each time, same isolate id),
and it named no leader until the next append. Under a row the group commits several times a second, so this costs
nothing there; `learners.mjs` runs before `k.mjs`, whose commit is that append, and `spread_verdict` takes a
learner's `notLeader` as declining, so it never turns a read into a 503 on its own.

### Deployed

| What | Commit | Version |
| --- | --- | --- |
| Site day (test-faults, NOROLL, image `…@sha256:1722…f3e9`, `containers/` unchanged since `80d8837`), 14:48 to 14:58Z | `48f8fae1` | a `4ebd36d0`, b `cfc50929`, c `f74b1ff0` |
| Edge | `48f8fae1` | `460e0956` |

The three PrSync queues were purged after the site day (14:59Z), before the smoke.

### Smoke at 500 agents (131245, 300 s, three learners: two enam, one weur)

`LEARNERS=enam:1500,weur:600` placed l0 and l2 on site a and l1 on site b. Read invocations by 15:05:46 (2.5 minutes
into load), from `/stats`:

| Member | `/read` in | served by read-index | shed | late |
| --- | --- | --- | --- | --- |
| f0 (leader) | 8,031 | — | 572 | 0 |
| f1, f2 | 1,016, 1,060 | 605, 556 | 647, 720 | 1, 13 |
| l0, l1, l2 | 26,707, 26,407, 27,291 | 12,101, 5,170, 11,773 | 23,092, 24,078, 23,696 | 0, 173, 0 |

- **The leader held**: term 2 throughout, 0 `/suspect` on any member, 0 leader changes. Its own `/read` was 14,807 over
  the run (the `ranges` and refs reads that stay on it), against 87,000 to 160,000 per row in rounds 1 to 3.
- It still stepped down twice by alarm, `CommitErrors` 4 of 76 and 7 of 121 calls failed to *both* followers at
  once, at 15:03:16 and 15:05:26, each to `Some(SiteId(1))` and back without a term change (`terms` all 2 at the end).
- Commit p99 3,966 ms (p50 905, n 2,690); agent pushes 1,218 of 1,241 ok (22 timeouts); human pushes 103 ok;
  feed p99 2,135 ms (11,699 events); ref reads 9,967 of 9,969 200.
- `read:api` 15,913 200 and 36,422 × 503 with `Retry-After: 1`: the members shed about 70% of 200 API reads/s of whole
  tiers. Each learner served about 40 to 80 reads/s while shedding 160 a second.
- Peak MB per member (`durableObjectsPeriodicGroups`): f0 95, followers ≤ 54, learners ≤ 55. No `exceededMemory`.

So the fallback did what it was for (no `suspect`, no term change, the leader's reads cut by 85 to 90%), but
whole-tier reads at journey 9's rate overran the five non-leaders' `HEAVY_READS`. The rows place eight learners
(`LEARNERS=enam:4000,weur:2000,apac:2000`, weighted as DESIGN now says) to see whether more members serve more.

### Rows (`rows-m9`, eight learners per row: `LEARNERS=enam:4000,weur:2000,apac:2000`)

`tests/fanout/rows.sh m9`, K = 4, `TEARDOWN=1`, `STOP_USD=11`, cost from `tests/fanout/raw/mem4-t0.t0` (14:48Z).
`place_learners` gave l0 to l7 on sites a, b, c, a, b, c, a, a (enam owed 4, weur 2, apac 2).

**B0-29, 600 s (131246).** Gate: commit p99 ≤ 600 ms, merge p99 ≤ 2 s, zero `Stale` on distinct refs, leader < 600
req/s with zero feed polls / fail commit p99 > 1 s, any queue merge > 10 s, any feed poll on the leader.
- Commit p99 5,461 ms (p50 834, n 6,793). **Fail.**
- Leader f0 held term 2 for the whole row (0 → 0); 0 `/suspect` on the group; 3 `StepDown` alarms on f0
  (15:20:20, 15:22:23, and f1's 15:22:26) were transfers that did not complete, so the term never moved. f0 took
  31,024 `/read` (the leader-only ones), 120 req/s, 0 feed polls; peak 139 MB, followers 88 and 90, learners 53 to 63,
  0 `exceededMemory`.
- Agent pushes 2,250 of 2,500 ok, 174 timeouts, 47 `fetch failed`, 27 `stale`, 2 refused; human pushes 185 ok.
- `read:api` 51,594 200 and 52,858 503 (`Retry-After: 1`); ref reads 19,443 of 19,778 200; feed p99 2,674 ms.
- Merges: 13 of 28 `merge:only` 200, 13 × 503, p99 18,971 ms.
- **The read hotspot moved to the org's `Authz` leader.** Every REST and GraphQL read reads the owner's whole `Authz`
  fold as a `dep` on its leader (`api::authz`, `call_opt`, no spread). `authz:7100001#1#f0` took 20,962 `/read` in
  15:16 to 15:20 (87/s, 33 s of CPU) and stepped down; f2 then logged six `StepDown CommitErrors` holds at 15:21:41 to
  15:25:30, the first with 753 and 950 of 1,411 calls failed. The repository's commits read that same group as a
  `deps` fold through `fresh`, so its step-downs plausibly set the repository's commit tail [inference: no per-commit
  dep timing was recorded].

**B0-32, 180 s (131247).** Gate: every push acked within 5 s or `ng … admission full`, merges in the next 2 entries,
zero leader changes, zero `suspect` / fail any lost ack, other refusal shape, object reset from memory, merge > 2 s,
leader change or `suspect`.
- **Zero leader changes (term 2 throughout), zero `/suspect`** on the group; peak MB f0 79, others ≤ 52; no reset.
- Burst: 49 ok, 446 `refused` (not the `admission full` shape), 5 `fetch failed`, 0 within 5 s (p50 26.6 s).
  **Fail** (refusal shape, pushes past 5 s, merges).
- Commit p99 2,318 ms (p50 632, n 1,820); merges `merge:only` 5 of 10 200, p99 3,266 ms.
- Four `StepDown CommitErrors` holds (f0 at 15:33:25 with 79 of 253 calls to f2 failed, f0 at 15:33:45 and 15:35:18,
  f1 at 15:33:41), none completed: the term stayed 2.

**B0-31, 600 s (131248, 1 MiB packs, leader moved to site 0 first by `transfer.mjs`: term 2 → 4).** Gate: commit p99
≤ 1 s with buckets up / fail > 3 s.
- Commit p99 2,311 ms (p50 658, n 4,421). **Fail** (between the lines).
- Leader f0 held from the transfer to the end; term 4 → 6 in the row, 29 `/suspect` (against 7,774 to 27,703 per row in
  round 3); two `StepDown CommitErrors` holds on f0 (15:39:00 with 12 of 221 calls failed to both followers, 15:42:03).
- Agent pushes 742 ok, 1,702 `refused`, 56 × 503 "every member asked shed the read" from the read gate; feed p99
  2,482 ms; `read:api` 53,010 200 / 53,172 503; peak f0 79 MB, others ≤ 52; f0 took 17,986 `/read`.

**L0-12 A1 replay, 600 s, leader kill every 60 s (131249).** Gate: no lease overlap over 500 ms, 500 long-pollers see
each event within 3 s at p99 outside leader kills, 100 of 100 refused for the agent kinds / fail feed p99 over 2 s,
any overlap, a refusal count off.
- 500 tasks and 500 leases set up (27 minutes of setup, 15:50 to 16:17Z); feed served by 8 learners and 2 followers.
- Feed p99 12,984 ms (p50 1,141, 11,398 events); polls 21,434 held, 10,167 `lagging`, 13,863 `gone`, 12,149 with no
  status. **Fail** (feed p99). Lease overlap is not reported by the driver; not judged.
- Leader 0 → 2, term 11 at the end (10 kills); 6,575 `/suspect` on the final leader; 38 `StepDown` lines across the
  three sites in 16:15 to 16:30Z. Peak MB f0 264, f1 278, f2 256, learners 70 to 111; no `exceededMemory` logged.
- Agent pushes 1,407 of 2,490 ok, 976 × 503 "every member asked shed the read" from the read gate; lease renewals 169
  200 / 1,098 503; `read:api` 27,519 200 / 83,330 503.
- The voters' peaks are back near round 1's: under leader kills the access-row reads the gate makes shed on every
  member, and each new leader rebuilds its dumps.

**B0-34, 600 s, shard kill at 300 s (131250).** Gate: k in band ≤ 8, human push p99 ≤ 3 s beyond transfer, clone TTFB
p99 ≤ 1 s / fail TTFB p99 > 5 s, k > 8.
- k 1 → 2 in band (≤ 8, pass); clone TTFB p99 158,542 ms (p50 846, 462 clones). **Fail** (TTFB).
- Leader f0 held term 2 throughout, 0 `/suspect`; commit p99 5,135 ms (p50 780, n 6,461); human push p99 121 s;
  agent pushes 2,003 of 2,500 ok, 348 timeouts; feed p99 2,514 ms; peak f0 129 MB, others ≤ 78.

**B0-33, 300 s, 5,000 agents (131251).** Gate: human and merge-queue p99 ≤ 1 s, ≥ 140 agent commands/s admitted,
zero leader changes and `suspect` / fail any leader change or `suspect`, < 100 commands/s.
- Term 2 → 8 (f0 leader again at the end), 7,539 `/suspect` on f0; commit p99 8,568 ms (p50 1,288, n 4,466).
  **Fail.**
- Agent pushes 2,020 ok of 12,533 (6.7/s), 9,752 `refused`, 454 × 503 "every member asked shed the read" from the
  read gate (against 6,925 gate 503s in round 3), the rest timeouts; fast user 341 of 494 200, p99 13,580 ms.
- Peak MB f1 258, f2 147, f0 129, learners 89 to 111.

**B0-35, 600 s, 500 open PRs (131252).** Gate: test-merge vCPU within `TEST_MERGE_BUDGET`, Actions dispatch p99 ≤ 10 s,
zero computes from list fields / fail over budget, dispatch p99 > 30 s.
- Test-merge vCPU, list-field computes and dispatch p99 **not measured** (`MergeBudget::charge` is its own work item; no
  Actions on `graft-m2b`). **Not judged on its gate.**
- What it measured failed: leader 0 → 1, term 11; 1,252 `/suspect`; commit p99 6,481 ms (n 39); agent pushes 1,481 of
  2,500 ok; `read:mergeable` 2,454 `MERGEABLE`, 7,453 `UNKNOWN`, 18,870 × 503; `read:api` 14,356 200 / 25,989 503;
  peak f1 141 MB, others ≤ 110, no `exceededMemory`.

### Verdicts, round 4

| Row | Verdict | Deciding number |
| --- | --- | --- |
| B0-29 | Fail | Commit p99 5,461 ms (gate 600 ms); 0 leader changes, 0 `suspect` |
| B0-32 | Fail | Burst 49 ok / 446 refused, 0 within 5 s; 0 leader changes, 0 `suspect` |
| B0-31 | Fail | Commit p99 2,311 ms (gate 1 s, fail 3 s) |
| L0-12 A1 | Fail | Feed p99 12,984 ms (gate 2 s); term 11 under 10 kills |
| B0-34 | Fail | Clone TTFB p99 158,542 ms (fail 5 s); k 1 → 2 in band |
| B0-33 | Fail | Term 2 → 8, 7,539 `suspect`, 2,020 of 12,533 agent pushes ok |
| B0-35 | Not judged | Its gate's instruments are not measured; 18,870 mergeable reads 503 |

### What still bounds the rows

- **Leader reads are gone; the shed moved to the members.** The repository leader took 8,931 to 31,024 `/read` per row
  (leader-only reads), against 87,000 to 160,000 before round 2. About half of the 200 API reads/s are answered 503
  with `Retry-After: 1`, because 8 learners and 2 followers together still shed half the whole-tier reads at
  `HEAVY_READS` and `DUMP_EVERY_MS` (each learner served 40 to 80 reads/s and shed about 160/s on the smoke).
- **The owner's `Authz` group is the new hot leader.** Every REST and GraphQL read reads its whole fold as a `dep`
  on its leader, and every repository entry reads it through `fresh` (`host::read_deps`). On B0-29 its leader took
  20,962 `/read` in four minutes (87/s, 33 s of CPU) and stepped down seven times; the repository's own leader held.
  Next: those dep reads by read-index on the `Authz` group's followers and learners, with the same caps.
- **Step-downs with almost no reads on the leader** remain at 5,000 agents (B0-33: 7,539 `suspect`, term 2 → 8) and
  under leader kills (L0-12), where the commit path itself is the load. Not changed here: a raft change is reported
  first.
- **B0-32's burst** is refused rather than answered `admission full`, and the driver does not record the `ng` reason;
  that refusal shape stays unexplained.

### Cost, round 4

`tests/fanout/cost.sh` with `T0_FILE=tests/fanout/raw/mem4-t0.t0` (14:48Z): **$11.19**, containers $1.93 of it, at
17:31Z after the last teardown. Cumulative at each row's end: smoke $0.57, B0-29 $1.96, B0-32 $2.38, B0-31 $3.52,
L0-12 $5.38 (27 minutes of setup), B0-34 $6.63, B0-33 $9.22, B0-35 $11.19. Rounds 1 to 4 together: $43.56. Every
network from 131245 to 131252 is torn down (smoke and rows each tore down their own; a final pass found 6 more
instances, all 200, and a second pass none).

## 12. Round 5: where the commit time goes, and `Authz` reads by read-index (`lm1-memory` after `6d1b7458`)

### What changed

- `7583378f`: the leader's `/stats` splits each commit. `queueMs` runs from a command's admission to the append of its
  entry, which covers the entry in flight, the cut, and the entry's dep reads. `depsMs` holds each `DepsWanted` read,
  whole (`all`) and by chain kind (`authz`, `names`, `tokens`), and `depFails` counts chain reads that gave no fold.
  `roundMs` (append to apply) and `confirmMs` already existed. `a1.mjs` reports all of them as `leader.split`. This
  ships in the site script, so it went out with a site day.
- `14ad1ee5`: the edge reads an owner's `Authz` fold by read-index, through `graftyard_lookup::read_spread`. That covers
  `api::authz`, `ident::authz_of`, the read gate, and `install_authz`. A read the members shed is answered 503 and
  never goes on to the leader, and the read gate keeps the `OVERLOADED` reason, so its 503 carries `Retry-After: 1`.
  A repository leader's own `fresh` dep read stays on the `Authz` leader. Design changes went in first: DESIGN
  "Follower read" and "Push", the AGENT-LAYER row for the org `Authz` leader, and a DECISIONS entry. The new test is
  `a_shed_authz_read_is_a_short_503_at_the_read_gate`, and `tests/lm1mem/plant.py` mutants 29 and 30 are caught.
  `learners.mjs` now also places learners on `authz:` chains.
- `2d06eb7d`: the edge's `o/r` lookup (`resolve`, `{"q": {"repo": name}}` on the owner's `Authz` group) also goes by
  read-index, and a failed lookup answers 503 through `read_failed`.
- No `crates/raft/src` change.

### Deployed

- Sites: `7583378f` by site day `siteday-m10`, 17:54 to 18:04Z (a, b and c, with the image unchanged). The PrSync queues
  were purged afterwards.
- Edge: `e2cfdfba` (`14ad1ee5`) at 19:14Z, then `afb025ac` (`2d06eb7d`) at 19:42Z. The edge is not a site script, so it
  needs no site day.
- Learners on `authz:7100001`: 4 (slots 0 to 3), placed at 19:17Z through the leader's `/learners` route, config 702.
  Before load, each of the two followers and each of the two learners probed returned the leader's exact bytes at
  index 702 in 190 to 580 ms.

### The measurement: B0-29's shape at 500 agents, before the fix (131253, 300 s, 8 learners)

Leader commit p50 3,970 ms and p99 50,066 ms (n 2,470). The leader moved from site 0 to site 2 (term 4).

| Part of a commit on the leader | n | p50 | p99 |
| --- | --- | --- | --- |
| Admission to append (`queueMs`) | 2,162 | 3,451 ms | 32,179 ms |
| An entry's dep reads, all chains at once | 93 | 634 ms | 46,522 ms |
| ↳ `Authz` | 182 | 238 ms | 17,713 ms |
| ↳ `Names` | 69 | 99 ms | 309 ms |
| ↳ `Tokens` | 159 | 99 ms | 326 ms |
| Append to apply (`roundMs`) | 92 | 179 ms | 1,000 ms |
| Storage confirm | 91 | 24 ms | 202 ms |

- The eight slowest whole dep reads were 46.5, 19.1, 17.7, 16.0, 13.3, 12.7, 9.8 and 6.6 s. Reads over 3 s took
  171.5 s in total, 125.0 s of it in `Authz` reads. `Names` and `Tokens` never passed 0.5 s.
- With one entry in flight per group, every command queued behind those reads.
- The owner's `Authz` leader took 140, 126, 127, 120 and 102 `/read`s a second in each minute from 18:19 to 18:23Z
  (Workers Logs, f1). All of them were the edge's permission and read-gate reads.
- After the row, the `Authz` group had no leader: three followers at term 53. One `/campaign` elected one.

So the commit p99 at 500 agents was set by the `Authz` dep read. Storage confirm and replication to a quorum were not
the bound.

### After the fix: the same smoke (131254, edge `e2cfdfba`, 4 learners on `Authz`)

| | Before | After |
| --- | --- | --- |
| Commit p50 / p99 | 3,970 / 50,066 ms | 817 / 1,739 ms |
| `Authz` dep read p99 | 17,713 ms | 366 ms |
| Admission to append, p90 | 28,323 ms | 730 ms |
| Leader | site 0 → 2, term 4 | site 0, term 2, held |
| Agent pushes ok | 682 / 1,247 | 1,238 / 1,253 |

- Append to apply now took about 365 ms per entry. That is the leader cutting six times as many entries (588 against
  92), not the bound.
- The `Authz` leader still took about 240 `/read`s a second, while each of its learners took about 52. That was the
  `o/r` lookup, which `2d06eb7d` moved before the rows.
- The `Authz` group was leaderless again after this smoke (term 55). One `/campaign` elected a leader.

### Rows (`rows-m10`, 8 learners per repository, edge `afb025ac`)

| Row | Gate / fail line | Result | Verdict |
| --- | --- | --- | --- |
| B0-29, 600 s (131255) | Commit p99 ≤ 600 ms, merge p99 ≤ 2 s, zero `Stale` on distinct refs, leader < 600 req/s with zero feed polls / commit p99 > 1 s, queue merge > 10 s, feed poll on the leader | Leader held: site 0, term 2, 0 changes, 0 `exceededMemory`, 0 feed polls on the leader. The split was lost: the leader's `/stats` read at the end of the row raised (869 ms, no message), and the isolate reset. 2,305 of 2,500 agent pushes ok (181 client timeouts at 120 s); `merge:only` 4 × 200, 25 × 503; feed p99 2,552 ms | **Not judged** on commit (not captured). Merges fail |
| B0-32, 180 s (131256) | Every push acked within 5 s or `ng … admission full`, merges in the next 2 entries, zero leader changes, zero `suspect` / other refusal shape, lost ack, merge > 2 s | Commit p50 432, p99 932 ms. Split p99: queue 694, round 304, confirm 38, deps 285 (`Authz` 234) ms. Burst: 147 ok, 352 refused, p50 23,063 ms. Term 2 → 4, leader site 0 at the start and the end; two `StepDown CommitErrors` holds on f0 (20:06:10 with 7 of 126 calls failed to each follower, 20:06:55) | **Fail** (burst refused, not `admission full`; acks past 5 s) |
| B0-31, 600 s, 1 MiB pushes (131257) | Commit p99 ≤ 1 s / > 3 s | Commit p50 650, p99 1,163 ms (n 5,157). Split p99: queue 872, round 317, confirm 72, deps 432 (`Authz` 419) ms. The leader went to site 2 by transfer, was back on site 0 at term 4 before load, and held. 1 `/suspect`. 976 agent pushes ok, 1,484 refused (reason not recorded by the driver) | **Between the lines** (round 4: 2,311 ms) |
| L0-12 A1 replay, 600 s, leader kill every 60 s (131258) | No lease overlap over 500 ms, 500 long-pollers see each event within 3 s at p99 outside leader kills / feed p99 over 2 s, any overlap | Feed p99 14,291 ms (p50 1,136; 17,473 events; 1,483 `lagging`, 9,982 with no status). Leader site 0 → 1, term 6. 2,029 of 2,490 agent pushes ok, 365 × 503 from the read gate. The last leadership's `Authz` dep reads had p50 2,246 ms (n 4) and 956 `/suspect`. Lease overlap: not measurable here, because `a1.mjs` takes only Shared leases (lines 334 and 444), and Shared leases never conflict | **Fail** (feed p99). Overlap not measured |
| B0-34, 600 s, shard kill at 300 s (131259) | k in band ≤ 8, human push p99 ≤ 3 s beyond transfer, clone TTFB p99 ≤ 1 s / TTFB p99 > 5 s, k > 8 | k 1 → 2 (pass). Clone TTFB p50 949, p99 2,253 ms over 572 clones, all 200 (round 4: 158,542 ms). Human push p99 121,020 ms (client timeouts). Commit p99 930 ms; split p99 queue 599, round 356, confirm 39, deps 239 ms. Leader site 0, term 2 throughout, 0 `/suspect` | **Fail** (human push p99). TTFB between the lines |
| B0-35, 600 s, 500 open PRs (131260) | Test-merge vCPU within `TEST_MERGE_BUDGET`, Actions dispatch p99 ≤ 10 s, zero computes from list fields / over budget, dispatch p99 > 30 s | Test merges used 0.274 vCPU of a 0.25 budget (164,135 ms spent in 600 s on site 0's shard; 707 computed, 6,043 shed); list-field computes 0 (0 asked). Dispatch p99 not instrumented. `read:mergeable` 1,949 `MERGEABLE`, 16,018 `UNKNOWN`, 39,990 × 503. Commit p50 601, p99 1,882 ms; leader site 0, term 2 throughout | **Fail** (over budget). List-field line passes |
| B0-33, 300 s, 5,000 agents | | Not run: the stop at $10 came first ($10.48 after B0-35) | Not run |

`Authz` group across the rows: leader site 0 at term 57 from B0-29 to the end. No `/campaign` was needed during the
rows.

### What the timing shows on the commit path

- **With `Authz` off the leader, no part of a commit dominates at 500 agents.** Rows without kills had commit p99 of 930
  to 1,882 ms. In each one, admission to append was the largest part (p99 599 to 1,151 ms). The dep reads came next (p99
  239 to 432 ms), then append to apply (p99 304 to 455 ms). Storage confirm stayed at p99 38 to 72 ms; round 4 saw 1.7
  to 2.1 s.
- **Every step-down logged in round 5 was `CommitErrors`; none was `Confirm`.** Per row (Workers Logs, "StepDown"):
  - B0-29: 1, on f0 at 19:49:35, and it did not complete.
  - B0-31: 1, on f1 before load.
  - B0-34: 4.
  - B0-35: 7.
  - L0-12: 22 in 21:01 to 21:08Z, across the three sites, during the kills.

  The holds listed name 3 to 7 failed calls in 41 to 126 to each follower. The terms in B0-29, B0-34 and B0-35 never
  moved.
- **Under leader kills (L0-12) the commit path is the load.** The last leadership's `Authz` dep reads took p50
  2,246 ms, against 81 to 174 ms in the other rows, and it took 956 `/suspect` reports. The split resets with each kill,
  so it covers only the last leadership. Per the brief, this is not fixed here.
- **API reads are still half shed.** In the rows, 29 to 71% of `read:api` answered 503 with `Retry-After: 1` (B0-29:
  44,992 × 200 and 70,472 × 503). These are the repository's whole-tier reads at `HEAVY_READS` and `DUMP_EVERY_MS` on 8
  learners and 2 followers.

### Checks

On `2d06eb7d`, every check passed:

- fmt;
- workspace clippy;
- wasm32 clippy for site and edge, each with and without `test-faults`, and for ops;
- `cargo test --workspace`;
- `check_design`, `check_names`;
- `check_links` on a clean copy (2,993 citations);
- `go test` in `containers/git/server`.

The first five came from `tests/am1/runs/checks.sh`; `check_links` and `go test` were run on their own.

`tests/lm1mem/plant.py` 29 and 30 were caught.

### Cost, round 5

`tests/fanout/cost.sh` with `T0_FILE=tests/fanout/raw/mem5-t0.t0` (17:56Z): **$10.56**, of it containers $2.34, at 21:54Z after the
last teardown. Cumulative at each step: first smoke $0.66, second smoke $2.02, rows start $2.18, B0-29 $3.53,
B0-32 $4.00, B0-31 $5.24, L0-12 $7.09, B0-34 $8.47, B0-35 $10.48 (the stop). Rounds 1 to 5 together: $54.12 ($43.56 + $10.56).

Teardown: each row's network, then networks 131217 to 131265 with `--pools`, which stopped 9 more instances. They
were the seven pool shards on site a, 131260, and 131228, which an earlier round had left running. Afterwards,
`ghosts.py` 21:52 to 21:56Z showed no running instance.

## 13. Round 6: the git cache's memory and files, and dep reads overlapping the round

### Baseline (`rows-m11`, sites `7583378f`, edge `afb025ac`, 2026-11-11 23:30Z to 2026-11-12 01:11Z)

| Row (repo) | Gate line | Measured | Verdict |
|---|---|---|---|
| B0-29 (131261) | Commit p99 ≤ 600 ms, merge p99 ≤ 2 s, zero `Stale`, leader < 600 req/s, zero feed polls on the leader | Mid-run commit p50 582, p99 944 ms; split p99 queue 713, round 327, deps 380 ms. 2,328 of 2,500 agent pushes ok. `merge:only` 16 × 200, 15 × 503. The leader's wasm instance was rebuilt 70 ms after the late raw `/stats` read (23:44:19.7Z), then `StepDown CommitErrors` moved it to site 1 and back | **Fail** (commit p99, merges) |
| B0-32 (131262) | Every push acked within 5 s or `ng … admission full`, zero leader changes, zero `suspect` | Burst: 87 ok, 411 `unpacker error`. The first shard took 512 validates at 23:52:08Z; all failed "Promise will never complete", with the shard isolate's `exceededMemory`. Commit p99 1,024 ms; leader held at term 2 | **Fail** (refusal shape) |
| B0-31 (131263) | Commit p99 ≤ 1 s (buckets up); placement failing: Human/MergeQueue p99 ≤ 3 s, 0 refused, agent > `INLINE_MAX_HOT` refused in `ng` | Commit p50 480, p99 818 ms. 1,713 of 2,500 agent pushes `unpacker error`: the first shard logged 725 `exceededMemory` at 00:04Z. The placement-failing half was not driven | **Fail** (half not run; agent pushes refused in the wrong shape) |
| B0-34 (131264) | k in band ≤ 8, human push p99 ≤ 3 s beyond transfer, clone TTFB p99 ≤ 1 s | k 1 throughout. 480 or more agent pushes refused "too many open files" in the container. Clone TTFB p99 120,031 ms. Human push p99 122 s. Commit p99 1,042 ms | **Fail** |
| L0-12 (131265) | No Exclusive overlap > 500 ms, member clock offsets < 500 ms, feed p99 ≤ 3 s outside kills, … | 141 probe pairs, 0 overlapping (max −3,765 ms). Clock offsets as measured from the orb were up to 4,346 ms; the orb's own clock ran about 4 s behind Cloudflare's (`date` against `cloudflare.com`'s `Date`), so that number is not the members' spread. Feed p99 11,427 ms over the whole run, with kills not excluded by the driver. 694 agent pushes 503 "every member asked shed the read". Leader site 1, term 13; 22 `StepDown`s, all `CommitErrors` | **Fail** (feed; clock line not measured as written) |

### Causes found

- **The git cache shard buffered every push body in its Durable Object** [verified from logs and code]. `/validate`
  read the whole body (`req.bytes()`), copied it with `to_vec`, prepended the tips, and built a `Uint8Array` for the
  container: four copies per validate. A burst of 512 × 256 KiB, or 1 MiB packs at 4 a second, put the first shard's
  isolate over 128 MB. This, not the group, failed B0-31 and B0-32.
- **The container ran every validate at once** [verified]. Each opens a spool file, a quarantine, pipes, and an
  `index-pack` process, so 500 in flight hit EMFILE (B0-34).
- **A raw `/stats` read rebuilt a loaded leader's wasm instance** [inference: the rebuild followed the read by 70 ms in
  B0-29 and twice before; no `exceededMemory` was logged]. It serialized megabytes of samples and traces.
- **Dep reads add a serial step to every entry** [measured]. One entry is in flight, so a command waits the round in
  flight, then its entry's dep reads (p50 118 ms under load against 28 ms idle on the same dep leaders, callee wall
  11 ms), then its own round. A simulation of the measured split gave about 800 ms p99 with the reads overlapped and
  the 600 ms gate only with near-free reads, so B0-29 needs both the overlap and less leader-side delay.

### What changed (`d8571e88`, merged with `origin/main` as `2bcb4360`)

- The shard streams `/validate`'s body to the container or pool shard unread, and picks the fan-out shard from the
  pack trailer the edge puts in the URL.
- The container runs at most min(16, max(4, 2 × vCPU)) validates: humans first, agents wait 2 s, at most 8 per slot
  queued, then 429 with `Retry-After`, which the edge answers `ng <ref> agent admission full, retry after <n>s`. It
  raises `RLIMIT_NOFILE` at start.
- Dep reads overlap the entry in flight (`crates/raft/src/member/leader.rs`): the next batch is cut once the entry in
  flight is a mean dep read from its mean round's end; `decide` runs only after that entry applies, with exactly the
  chains `deps()` names then, reading any missing.
- `/stats?summary=1` answers percentiles only; `depsMs.spawn` times the isolate's queueing before a dep read starts.
- `a1.mjs`: summaries only; L0-12's clock line from the spread between members in each sampling round and feed p99
  outside a 15 s window after each kill push; B0-31's second half with placement failing through the bed-token header
  `x-graft-place-fail`.
- Design: DESIGN, "The write path", journey 1, Scale "Entries per group", and "Git cache shards (derived)"; DECISIONS,
  "Dependency reads overlap the entry in flight" and "LM-1 round 6: shards stream push bodies and cap validates".

### Raft checks (dep-read pipelining)

- Long sweep, 2,000 seeds × 10,000 steps: exit 0 in 489 s. Harsh: 154,668 operations, 17,788 entries decided on
  reads, 6,287 early asks, 534 re-reads. Gray: 2,030,333 operations, 292,936 entries on reads, 216,744 early asks,
  13,221 re-reads.
- Porcupine: `hist.jsonl` 151,346 operations Ok; `hist.jsonl.gray` 2,030,319 operations Ok.
- Plants: decide when the reads arrive; stale extra deps; no re-read when chains change; dep ids reset per leadership;
  a post-cut command joining the early batch; a transfer keeping its reads; two entries in flight. Each was caught by
  `crates/raft/tests/deps.rs` or the simulator's checks.

### Other checks

On `d8571e88`: fmt, workspace clippy, wasm32 clippy for site and edge with and without `test-faults` and for ops,
`cargo test --workspace` (75 suites), `check_design`, `check_names`, `go test`. `tests/lm1mem/plant.py` 41 to 47
caught (4 Rust, 3 Go).

### F67 lead (leaderless after a transfer under hung calls)

L0-12's 22 `StepDown`s were all `CommitErrors`; 7 held with `to: None` (a 01:08:06; b 01:09:19 to 01:09:52, five in a
row; c 01:06:26, 01:06:46, 01:07:46Z). A hold keeps the leader (`health.rs`, `hold`), so it alone leaves no group
leaderless. Campaigns are not logged, so a transfer that ended leaderless is neither shown nor ruled out here.

### Round 6 rows (`rows-m13`, sites `11ce32c3`/`7b7c2f87`/`2446b535`, edge `3858167a`, 2026-11-12 06:32Z to 08:20Z)

| Row (repo) | Measured | Verdict |
|---|---|---|
| B0-29 (131269) | Commit p50 517, p99 1,478 ms (queue p99 1,096, round 397, deps 396). 2,291 of 2,500 agent pushes ok. `merge:only` p99 11.8 s. 17 human `stale` on shared refs. Leader held | **Fail** |
| B0-32 (131270) | Burst 197 ok, 303 `full`, 0 acked within 5 s (ok p50 27 s). Commit p99 4,833 ms | **Fail** |
| B0-31 (131271) | Buckets up: commit p99 652 ms. Human push p99 162 s up, 121 s with placement failing. 1,380 agent `full` (the `ng` shape) | **Fail** |
| B0-34 (131272) | `read:ref` p99 110 s, clone TTFB p99 2.6 s, human push p99 121 s | **Fail** |
| L0-12 (131273) | Leader 0 → 2 (term 7). 1,069 agent pushes 503 (access rows shed). Feed p99 26.5 s outside kills. Lease overlap 0. Clock spread 5,966 ms | **Fail** |
| B0-33 (131274) | Leader 0 → 1; new leader queue p50 7 s; `failedCalls` 292 | **Fail** |

## 14. Round 7: the shard's lock convoy, ls-refs' tag read, the feed's first poll, members holding entries, and merges

### Causes found

- **The git shard held its network lock across streamed bodies and repacks** [measured]. A waiting writer queued every
  ls-refs and validate behind it: home-shard ls-refs walls went from 2.7 s to 12 s within a minute at 0.65 vCPU (B0-31,
  m13). Fix `79212b59`: bodies are read before the lock, a repack takes none, and evict and fresh init wait on the
  `repacking` mutex. A local repro took ls-refs p99 from 5.3 s to 0.26 s. Tests `TestValidateBodyHoldsNoLock` and
  `TestRepackHoldsNoNetworkLock`; plants 60 to 63.
- **Every v2 upload-pack read `refs/tags/` from the leader, ls-refs included** [measured]: 27 of 40 sampled leader
  `/read`s in L0-12 came straight from the edge's upload-pack. Fix `9596128e` (`CloneKey::needs_tags`); plants 64, 65.
- **A learner's first feed poll fed its whole backlog in one turn** [measured]: each of L0-12's 8 learners spent 15 to
  46 s of CPU at 08:00:38Z (about 1,070 entries × 500 leases of SQL reads); voters sharing their isolates answered
  "Requests queued for too long" at 13 to 36% CPU, and `STEP_DOWN` moved the leader at 08:01:39.
- **Every member kept a clone of each applied entry's events, packs included, for a ring nobody polled** [measured].
  Leaders and followers are never polled, so `Feed.pending` grew to FEED_RING's 10,000 entries: at 500 agents
  (131276, 180 s, smoke m14) the leader's wasm memory went 2 → 67 MB and a follower's 2 → 40 MB, linearly with agent
  pushes, and neither shrank after load.
- **REST merges read both whole tiers** [measured]: about 1 MB parsed at the edge per merge; edge merge walls 1.2 to
  4.4 s while the shard's plan took 68 to 1,232 ms (s500 m14).
- **Every merge waited for whatever catch-up ran on the shard** [measured]: with the narrow read deployed (smoke
  m15, 131278), `x-graft-shard-timing` showed the shard's merge waiting 64 to 2,151 ms of its 175 to 2,750 ms; no merge
  caught up itself, so it already held its commits. Merge p99 6.2 s.

### What changed

| Commit | Change | Test | Plants |
|---|---|---|---|
| `79212b59` | Shard: bodies before the lock, lock-free repack | `TestValidateBodyHoldsNoLock`, `TestRepackHoldsNoNetworkLock` | 60–63 |
| `9596128e` | ls-refs reads no tags | `only_a_cacheable_include_tag_fetch_needs_the_tags` | 64, 65 |
| `2f30c61b` | `/stats?summary=1`: `wasmMemMb`, `admission`, `admitted` | — | — |
| `aed68895`, `4471a1e9` | No entry kept for the ring before a poll reads the owner; that poll starts the ring at the applied entry (`Ring::start`); `lease::rows` decodes its scan | `a_first_poll_starts_the_ring_at_the_applied_entry_and_a_serving_ring_keeps_its_events` | 66–68 |
| `302d1a14`, `3cc1d6c1` | REST merge reads `{"rows": {"mergeN": n}}` only; followers serve it | `a_merge_plans_from_its_rows_as_from_both_tiers`, `a_follower_serves_only_the_designs_reads`, `the_tier_read_asks_for_packed_rows` | 69–71 |
| `41f1860d` | A merge waits for a catch-up in flight only after it misses a commit | `a_merge_waits_for_the_catch_up_in_flight_instead_of_racing_it` | 72, 73 |

Design: DESIGN Scale item 2 and DECISIONS (A25 amendment, shard lock); AGENT-LAYER activity feed and DECISIONS (ring
start); DECISIONS (REST merge rows; merge wait after a miss, amending `bm2-repack`). `a1.mjs` writes each merge's
`server-timing` and shard timing to the row's `mergeSteps`.

### Smokes

| Smoke (repo) | Deployed | Leader / follower peak wasm MB | Commit p99 (queue) | Merge p99 | Other |
|---|---|---|---|---|---|
| m14 s250 (131275) | to `ec97e3f1` | 52 / 49, still growing after load | 689 (457) | 4.4 s | 374/374 agent ok; 10 human `stale` (shared refs) |
| m14 s500 (131276) | to `ec97e3f1` | 67 / 40, growing | 956 (599) | 4.4 s | 742/742 agent ok; API 89%; leader held |
| m15 s250 (131277) | to `3cc1d6c1` | — | — | — | Create failed: `/place` refused 8 tries, weur near follower 108–126 ms over `NEAR_RTT` 100 (fixed on `m0-carried-4` `2e4cd9f`) |
| m15 s500 (131278) | to `3cc1d6c1` | 46 / 14, flat after load | 658 (467) | 6.2 s | 741/741 agent ok; `read:ref` p99 10 s; leader held, 84 req/s, 0 feed polls |

Checks on `3cc1d6c1`: fmt, workspace clippy, wasm32 clippy (site, edge, ops; with and without `test-faults`),
`cargo test --workspace`, `check_design`, `check_names`, `go test`: all pass; `check_links` fails only on
`.worktrees/` cache paths. Raft is unchanged since round 6's sweep (ok, 339 s) and Porcupine (Ok on 151,346 and
2,029,703 operations), logged in `tests/am1/raw/runs/r6b-*.log`.

## 15. Round 8: merging hot-shards, and why identity groups lose their leaders under load

### Merge

`origin/main` 6f82993 (hot-shards) is merged as `90808bd6`, with `03b66f22` fixing two test beds for
`ShardHost::validate`'s new `derived` flag and a catch-up test that still counted the merge order before `41f1860d`.
The edge keeps round 7's streamed validate (trailer and `class=agent` in the URL, 429 as `ShardRefusal::Busy`, the
`via=0` retry after a failed fan-out) inside hot-shards' per-site loop; a 429, like a 422, ends the tries instead of
moving to the next site's shard. The container keeps round 7's admission gate and lock-free repack with hot-shards'
`derived=1` validate, which skips the queue count, and derived repack CPU. Full checks on `03b66f22`: fmt, workspace
clippy, wasm32 clippy (site, edge, ops; with and without `test-faults`), `cargo test --workspace`, `check_design`,
`check_names`, and `go test` pass; `check_links` fails only on `.worktrees/` cache paths. Not deployed yet.

### Identity-group step-downs in smoke m16c s500 (131282, 15:19:30Z to 15:23:00Z)

15 `STEP_DOWN` alarms, all `CommitErrors` with 3 to 18 failed calls per follower [measured, Workers Logs]:
`tokens:2490` f0 → f1 → f2, then f2 held 3 times; `names:152` f0 → f1 → f2, held; `names:708` f0 → f1 → f2;
`release` f0 → f1; and members of the torn-down `repo:131273`/`131274` groups.

- **Not CPU on the identity members** [measured]: `tokens:2490` f0 used 4.3 CPU-s in 4 min, f1 32 s, f2 12 s; f1's
  8,000 `/read`s cost 13.5 CPU-s.
- **Not isolate sharing, as far as it can be read** [measured, idle]: `/isolate` on 11 members (identity voters,
  `release`, `repo:131273/131274`) answered 11 distinct isolate ids. Under load is not shown.
- **Synchronized ~3 s walls with no CPU** [measured]: at 15:20:30 both `tokens:2490` followers, on sites b and c,
  ran one `/raft` of 3,146 and 3,152 ms wall at 19 and 38 ms CPU; at 15:20:42–45 five unrelated objects on site a
  ran `/raft` calls of 3.05 to 3.07 s at under 20 ms CPU each, and at 15:20:20 two site-c objects did the same
  (3.3 s). A 1 s `CALL_DEADLINE` misses on each, and three in a 10 s window trip the commit-error rule. That these
  are storage-confirm (output gate) stalls on the follower is [inference]: the walls are flat at about 3 s, carry no
  CPU, and hit many objects of one site at once. The own-stall exclusion covers only the leader's own confirms.
- **Also measured, not yet tied to a trip**: old groups' members (131267–131274, torn-down repos) ran 0.5 to 4.7 s
  CPU-bound turns at 15:19–15:20 (`repo:131274#1#f1`'s alarm: 4,730 ms CPU), the site-a fold audit cron refolds
  4 chains every 15 min (logs "database table is locked" on most runs), and other threads' repositories
  (`repo:65551`, 70 CPU-s per 5 min; `repo:33xx000`/`39xx000` groups, 20 to 30 CPU-s each per 30 min) share the
  sites.

Next: decide whether a follower's late-but-answered append should count against `STEP_DOWN` (a `crates/raft`
change: design first, then the sweep and Porcupine), then site day, smokes, and the rows.

### Round 8b: smokes m17 and a correction

Site day m17 from `c756014d` (main 4265f3d merged: `NEAR_RTT` keeps the nearest try, member calls stream bodies),
image `57ceedbb`, edge `70832204`.

- **m17 s250**: no run. The create of `fm17s250` (131283) landed, its answer was lost, and `lib.mjs`'s re-send got
  422 `name already exists`; `mkrepos.mjs` now reads the repository then (`a8d10cee`).
- **m17 s500 (131284, 180 s, 8 learners, k 4)**: leader held at term 2, 107 req/s, 0 feed polls on it; commit
  p50/p99 495/7,485 ms (queue p99 7,144, round p99 507, confirm p50/p99 37/121, `deps:tokens` p99 575); 722 of 736
  agent pushes ok (12 client fetch failures, 2 × 503 `release: not leader`); merge:only p99 6.3 s with `resolve` up to
  4.9 s of it; human push 69 ok, 2 `stale`, 5 `stale:race`; most side and read rows' p99 13 to 41 s. 3 `STEP_DOWN`s:
  `tokens:2490` f0 (3 and 6 failed of 19 and 18 calls), `names:708` f0, and `names:708` f2 held. Leader confirm p99
  121 ms against 89 ms in m16c before streamed bodies, sent to m0-carried-5 (one smoke each, not an A/B).

**Correction to section 15.** The flat 3,101 to 3,103 ms `/raft` walls at about 10 ms CPU on followers are not
stalls or holds: a follower replies at once, and the 100 ms ticker each input starts (`Host::kick`, `ACTIVE_FOR_MS`
3,000 ms, exiting on the 31st tick) keeps the invocation open after the reply, which Workers Logs counts as wall time
[inference from code, `workers/site/src/host.rs`; workerd's actor timers hold the request's drain]. So those walls do
not show why identity leaders' calls miss `CALL_DEADLINE`. `tests/fanout/idstats.mjs` now saves each identity
member's `/stats` peer calls after every smoke (each failed call's kind, size and time on the leader's side).

### Round 8c: smokes m18, and the cause of the identity step-downs

Site day m18 from `a8d10cee` (main 68166fe merged: the self `SITE` binding for PrSync), image `57ceedbb`, sites
`abddb96c`, `2b49bafb`, `3f304093`, edge `fa6fa5ec`. Both smokes held their repository leader at term 2 with 0 feed
polls on it.

| Smoke | Commit p50/p99 | Queue p99 | Confirm p99 | merge:only p99 | Agent pushes | read:api p99 |
|---|---|---|---|---|---|---|
| s250 (131285) | 427/2,625 ms | 2,340 ms | 86 ms | 6.6 s | 343 of 343 ok | 6.4 s |
| s500 (131286) | 427/950 ms | 697 ms | 146 ms | 6.9 s | 743 of 747 ok (3 × 503 `release: not leader`) | 20.2 s |

`STEP_DOWN` alarms 19:13 to 19:29Z: `release` f0 (7 of 3, twice), `names:708` f0 → f1 → f2 twice, `names:369`,
`names:152` (14 of 279 calls failed).

**Cause** [measured, `idstats` and Workers Logs; the rule it trips is in code]:

- The failed calls are the leader's AppendEntries to one follower: empty or one entry of about 112 bytes. They fail in
  bursts at one instant: `tokens:2490` at 19:17:01Z had 31 and 17 deadline misses. Each one is answered late, 1.0 to
  2.4 s after it left.
- On that follower (`tokens:2490` f1) every `/raft` and `/read` that started from 19:17:00.22 to 01.82 ended together
  at 19:17:02.16 with 0 to 2 ms CPU: one stall of the object's input queue of about 1.9 s. Why the object stalled is
  not shown.
- One stall became dozens of failed calls because a read that arrives after the lease lapsed forced a new round to
  every follower (`start_read` → `broadcast(true)`), one per read. An idle identity group's lease lapses between
  bursts, so the next burst of reads sends one AppendEntries per read to the stalled follower, each with its own
  `CALL_DEADLINE`, and `record_call` counts each round. Three in 10 s trip `STEP_DOWN`.

**Two fixes tried and withdrawn** (raft changes, both with tests and plants that caught their mutants):

- `fa30bff9`, reverted in `67e71612`: a lapsed read sends a round only to a follower with no call out, or one whose
  call is past `CALL_DEADLINE`, and an answer sends the next round for reads that arrived after it left. On the tree
  merged with f67-liveness, the harsh sweep (`sim::harsh_network_keeps_every_invariant`, 60 seeds) acked 752 writes
  against main's 1,259 (the test needs 1,200), and the long sweep stopped at seed 10909: 30,277 ms with a healthy
  quorum, requests waiting and none completing, over the 30 s bound. A same-machine A/B of the harsh sweep on main
  d7ee927 alone passed. The per-read forced rounds also stand in for retransmission after dropped calls.
- Not committed: count a deadline miss to a follower only if it was sent at least `CALL_DEADLINE` after the last miss
  counted to it (`health.rs`, `record_call`). On the f67-merged tree, `liveness::leaders_leave_a_site_whose_calls_fail`
  failed (3 of 12 seeds with 3 leader changes, limit 2) and the harsh sweep acked 1,088. **Correction:** the same tree
  without this change (`67e71612`) fails the same test on the same seeds 7, 9 and 11 with the same distribution, so
  those failures are the f67 merge's, not this change's. It has not yet been tested on a tree without f67.

The first fix is withdrawn. The second ships as `0c7cbc23` (branch `lm1-pref67`: `720bdb36` with main f10ea4b,
`fa30bff9` reverted), merged here as `aba91cbe`. `record_call` excuses a deadline miss sent within `CALL_DEADLINE` of
the last counted miss to the same follower, so one stall is one failed call. On that base [measured, simulator]:

- `sim::harsh_network_keeps_every_invariant`: 1,233 writes acked (1,212 without the change; the test needs 1,200).
- `liveness::leaders_leave_a_site_whose_calls_fail`: 12 of 12 seeds moved by `STEP_DOWN`, one leader change each, new
  leader off the sick site in 3.6 to 7.7 s, as without the change. Gray-failure detection is kept.
- Long sweep, 2,000 seeds x 10,000 steps, both halves: exit 0. Porcupine: `Ok` on `hist.jsonl` (150,903 operations)
  and `hist.jsonl.gray` (2,029,109).
- Test `leadership::one_follower_stall_counts_once_per_deadline`: 30 read rounds missed within 300 ms do not trip
  the rule; 5 misses a deadline apart do. Plants 74 (no dedupe) and 75 (dedupe forever): 2 of 2 caught.
- Design: `design/DESIGN.md`, Shape, the `STEP_DOWN` paragraph, one added sentence.

**Merges**: main f10ea4b (merge-budget-4) as `8f54b3b0`, keeping `/stats?summary` beside main's test-faults
`body=array|stream` switch; main d7ee927 (f67-liveness, a `crates/raft` change) as `77629bc7`. The f67 test
`a_group_whose_every_member_flapped_elects_on_the_first_requests` sent `suspect` to all three flapping members in
lockstep every 500 ms. Once f67 stopped flapping members denying rivals as `Contending`, all three split every vote,
because the 500 ms cadence outlasts the losers' 100 to 400 ms backoff. Reports every 100 ms elect a leader in 700 ms;
the test now uses that cadence and keeps its 1 s bound. It fails the same way with or without `fa30bff9`.

**The merged tree fails raft's checks** [measured, simulator, `77629bc7`/`67e71612`]: with main d7ee927 (f67-liveness)
merged into this branch, `sim::harsh_network_keeps_every_invariant` acks 1,102 writes (needs 1,200) and
`liveness::leaders_leave_a_site_whose_calls_fail` gives 3 leader changes on seeds 7, 9 and 11 (limit 2). Main alone and
this branch before the merge (`720bdb36`: 1,212 acked) pass both. **Correction:** `8f54b3b0` (main f10ea4b merged)
is not a green pre-f67 tree: it still holds `fa30bff9`, and its harsh sweep acked 858. Seed 107 traced: S1 leads at term 3 after a
transfer and holds the only copy of entry 160 (this branch's dep reads let the leader append while an entry is in
flight). From 43.6 s S2 campaigns, both other voters refuse S1's pre-votes as `Suspected` for 2 x SUSPECT, and S1 refuses
theirs as `LogBehind`, so no member can win until 60.4 s. Sent to f67-liveness, which owns the fix. Not deployed.

### Round 8d: the combined tree

`lm1-memory` `6ebc48c9`: main a9d1645, which carries narrow-reads-2 and f67-liveness f53b6f8, plus the identity
step-down fix `0c7cbc23`. Its tree equals `742165ca`, where the checks ran.

- **Merge of narrow-reads-2** (`8c152210`): the edge takes main's per-request `Ctx` timing, which replaces this
  branch's thread-local timing (`236eca5b`) and its test. The branch keeps a shard's 429 ending the validate tries
  and a REST merge reading only its rows. Edge and step-down plants 26, 27, 29, 30, 41 to 43, 74 and 75: 9 of 9
  caught.
- **Full checks**: fmt, workspace clippy, wasm32 clippy (site and edge, with and without `test-faults`; ops),
  `cargo test --workspace`, `check_design`, `check_names`, `check_links` and `go test` all pass (Rust 1.100).
- **Long sweep** (2,000 seeds x 10,000 steps, every half): exit 0 in 563 s. The longest stretch with a healthy quorum,
  requests waiting and none completing is 29,799 ms (seed 10921), under the 30 s bound.
- **Porcupine**: `Ok` on `hist.jsonl` (150,828 operations), `hist.jsonl.gray` (2,042,240) and `hist.jsonl.live`
  (2,601,072).

### Round 8e: site day m19 and smokes

Site day m19 from `aac3e0b9`: new shard image `sha256:73d9b2ce79af6ee6d9a23cdea6982123855a5d3b109fa428e80bf4f19d7080c3`
(this branch's container admission gate, which `15f6ed1b` lacks), site versions `049f955c`, `673e1876`, `343b1877`,
edge `b9c0fd62`. Smokes of 180 s each, 8 learners (enam 4, weur 2, apac 2), k 4 [measured]:

| Smoke | Leader | Commit p50/p99 | Queue / round / confirm p99 | merge:only p99 | Agent pushes | Push p99 agent / human | read:api p99 (n) |
|---|---|---|---|---|---|---|---|
| s250 (131287) | held at term 2, 0 feed polls | 363/982 ms | 577 / 352 / 79 ms | 3.3 s | 365 of 370 (5 `full`) | 37.8 / 38.1 s | 6.5 s |
| s500 (131288) | held at term 2, 0 feed polls, 121.8 req/s | 341/567 ms | 421 / 255 / 147 ms | 2.7 s | 735 of 736 (1 client fetch failure) | 6.6 / 8.5 s | 3.8 s (35,659; 98.4% 200, 53 × 503, 506 capped) |

- **No `STEP_DOWN` alarm on any site from 02:16 to 02:31Z** [measured, Workers Logs]. The same query over m18's
  19:16:30–19:18Z returns its three known step-downs (`release`, `names:708` f0, `names:708` f1), so it is not blind.
  In s500 the `release` leader recorded 179 and 181 deadline misses to its followers in one burst at 02:27:29Z, the
  follower-stall pattern of round 8c, and did not move. In s250 `names:152`'s leader recorded 267 the same way.
- **Leaderless stretches**: the repository leader held at term 2 through both smokes, and no identity group changed
  leader, so the longest leaderless stretch with requests waiting is 0 s in both.
- **s250's push p99 of about 38 s** is the first minute on a new image and new repository; s500, which followed on
  warm shards, was 6.6 s [inference: the shard warm-up of a just-built image, not measured].
- **API**: s500's `resolve` p99 is 3.2 s and `auth` 2.4 s, under narrow-reads-2's per-isolate caches. B0-29's API
  line (p99 at most 2 s, at least 99% served) is not met in the smoke.
- Leader confirm p99 147 ms at 500 agents, above the ~100 ms m0-carried-5 asked about.

Cost after both smokes: $31.99. Rows `rows-m19` started in the operator's order: B0-29, B0-32, B0-31, B0-33, B0-34,
B0-35, L0-12.

### Round 8f: rows m19 (standard-2 shards)

Shard instance on every m19 row: **standard-2** (`SHARD_INSTANCE` unset). The operator approved standard-4 for gate
rows on 2026-11-13 (journey 9's design size); the next site day sets it, and any B0-29 merge or push line that missed
here reruns there.

**B0-29** (131289, 500 agents, 600 s, 8 learners, k 4, 02:38:24 to 02:48:24Z) [measured]:

| Gate line | Measured | Verdict |
|---|---|---|
| Commit p99 at most 600 ms (fail over 1 s) | 796 ms (p50 444; queue p99 515, round 347, confirm 124) | miss, not a fail |
| Merge p99 at most 2 s (fail: any queue merge waiting over 10 s) | merge:only p99 4.26 s; the merge loop's p99 20.0 s | miss |
| Zero `Stale` on distinct refs | 0 agent `stale` in 2,500 agent pushes (2,499 ok, 1 client fetch failure); human pushes 7 `stale`, 19 `stale:race` on shared branches | pass |
| Leader under 600 req/s, zero feed polls | 116.2 req/s, 0 feed polls | pass |
| API read p99 at most 2 s, at least 99% served (fail over 5 s) | p99 3.60 s; 117,390 of 119,506 = 98.2% 200 (246 × 503, 1,870 capped by the driver) | miss, not a fail |
| Leader held | term 2 throughout, 4 failed calls | pass |
| Longest leaderless stretch with requests waiting | 0 s (no group changed leader; no `STEP_DOWN` on any site) | |

Push p99: agent 12.2 s, human 10.5 s.

**Commit tails follow the leader's CPU** [measured, Workers Logs, leader `repo:131289#1#f0` on site a, 10 s bins
02:39:40 to 02:48:30Z, `tests/fanout/raw/tools/cpubin.py`]:

- Pearson r = 0.83 between the leader's CPU per bin and the p90 of its `/commit` walls in that bin, and 0.84 against
  the bin's worst `/commit`. Controlling for the bin's invocation count, the partial r is 0.82, so the cause is CPU
  per bin, not request count (invocations against p90: r = 0.35).
- The worst `/commit` bins (1.0 to 1.5 s) are the top-CPU bins: 02:44:40 (3.4 s of CPU in 10 s, 34% of one core,
  `/commit` wall p90 949 ms, max 1,298), 02:46:20 (2.6 s, max 1,491), 02:48:00 (2.6 s, max 1,199). The largest single
  invocation in those bins is 196 to 218 ms of CPU.
- Leader CPU rises over the row, from 1.5 to 1.9 s per 10 s between the first and last 200 s, and commit p90 from 628
  to 683 ms.
- This matches m0-carried-6's finding that sends wait on every earlier unconfirmed write and the worst tails are when
  the leader's event loop fell behind. Which invocations make the 200 ms turns is not yet traced.

**B0-32** (131290, 500 agents, 180 s, burst of 500 × 256 KiB at 60 s with 5 merges queued, 02:49 to 02:55:20Z,
standard-2 shards) [measured]: **fail**.

| Gate line | Measured | Verdict |
|---|---|---|
| Every push acknowledged within 5 s, or 200 `ng … agent admission full, retry after <n>s` | 0 within 5 s (p50 25.4 s); 376 ok, 84 `admission full` (the allowed shape), 29 refused `unpacker error: site N's shard answered N: {"warming":true}`, 11 client timeouts at 120 s | fail: 29 in another refusal shape |
| Merge in one of the next 2 entries, none delayed over 2 s | merge:only p99 19.9 s, p50 1.7 s | fail |
| Zero leader changes | term 2 throughout on 131290 | pass |
| Zero `suspect` reports | 56 `/suspect` calls, all to identity followers (`release#1#f1` 26 and its site-c peer 26 at 02:52:04, `names:708` 2 and 2), none to the repository group | fail as written |
| Object reset from memory | none seen | pass |

Commit p50/p99 366/789 ms, 123.4 leader req/s, 4 failed calls. The row's background agent pushes: 676 of 737 ok, 17 `stale` (distinct refs, so a correctness signal to trace), 20 `admission full`, 6 `warming` refusals, 18 client failures; agent push p99 66.5 s. Longest leaderless stretch with requests waiting: 0 s;
one `STEP_DOWN` in the window is `repo:131274#1#f2`, a torn-down repository's group, at 02:49:12Z.

Cause [inference, not yet traced]: the burst's 500 validates arrive while the network's shards on standard-2 are still
catching up. A shard that answers `warming` past its wait on every placement site becomes the push's refusal
(`unpacker error`), and the rest queue behind the busy shards for 25 s. The 52 `suspect` reports at 02:52:04 are
edges that timed out reading `release` during the burst. Both need the standard-4 rerun before they are a code
finding.

**B0-31** (131291, 500 agents, 600 s, every agent push 1 MiB, placement failing from 300 s, standard-2 shards,
03:00:05 to 03:10:05Z) [measured]: **not a valid run of the row, and a fail on two lines as run.**

- **The leader was not on apac.** `transfer.mjs` moved it from site 0 to site 2 (apac) at about 02:57:30Z, term 3, and
  it was back on site 0 (enam) by 02:57:41Z, term 4, 2.5 minutes before load: site 2's member took its last
  `/commit`s at 02:57:33 to 02:57:40, and site 0's from 02:57:43 on (Workers Logs, `repo:131291#1#f0`/`#f2`). No
  `STEP_DOWN` fired in the window. Moving back matches `maybe_hand_off`: a leader whose group's `home` answered
  HOME_CALLS calls with none failed in a window transfers back to it (`crates/raft/src/member/health.rs`). So a
  transfer cannot hold a leader on apac in a group homed on enam; the row needs the group re-homed to apac.
- With the leader on enam: commit p99 584 ms (queue 458, round 226, confirm 76), 117.3 req/s, 0 feed polls, 2 failed
  calls. Longest leaderless stretch with requests waiting: 0 s (the two term changes are transfers before load).
- Buckets up: agent pushes 1,023 of 1,130 ok, 86 `admission full`, 18 client timeouts, 3 `warming` refusals; agent
  push p99 120 s (the client cap); human pushes 36 of 41 ok, p99 240 s; merge:only p99 21.6 s.
- Placement failing: human pushes 133 of 141 ok (7 `stale:race`, 1 `stale`), push p99 22.0 s; merges 32 ok, 3 × 409,
  1 × 503 "the repository's tier rows did not answer"; agent pushes 1,262 `admission full` of 1,370, 47 ok, 56
  client timeouts at 120 s.
- Against the gate: Human and MergeQueue **commit** p99 by class is not in this row's output (the driver records the
  push's whole time, not its commit), so "at most 3 s" is not measured. "Agent pushes over `INLINE_MAX_HOT` refused at
  once in the `ng` shape" fails: 1,262 were refused as `admission full`, not with the placement refusal, and 103
  were accepted or timed out. One merge was refused (503) while placement failed.

Design contradiction, quoted: B0-31 is "the same replay with the leader on apac" (`build/BUILD-PLAN.md`, B0-31), but
"a leader … whose group's `home` has recovered and caught up, transfers" back to `home`
(`crates/raft/src/member/health.rs`, `maybe_hand_off`, from DESIGN "Leadership"). On a group homed on enam, the
apac leader the row asks for lasts about 10 s.

**B0-33** (131292, 5,000 agents, 300 s, 252 agent-class commands/s offered, standard-2 shards, 03:14:11 to
03:19:11Z) [measured]: **fail**.

| Gate line | Measured | Verdict |
|---|---|---|
| Human and merge-queue commit p99 at most 1 s | leader commit p99 3,915 ms (p50 1,366; queue p99 3,490, round 1,347, deps 3,028); human pushes 11 of 155 ok; merge:only p99 167.5 s, 4 merges | fail |
| At least 140 agent commands/s admitted (fail under 100) | 50.8/s answered ok (15,242 of 49,205 agent-class requests); agent pushes 2,427 of 12,527 ok | fail |
| The user token admitted as `Agent` | fast user 259 × 200, 271 × 503 | fail (503 is not the 429 shape) |
| Every refusal in the `ng` or 429 `retry-after` shape | 2,762 `admission full` (`ng`); 3,057 × 503 "access rows did not answer … shed the read: overloaded" or "not leader"; 650 `warming`, 300 "ranges read answered null", 60 "not leader" as `unpacker error`; 33 × 500; 3,231 client timeouts | fail |
| Zero leader changes | term 2 → 16; leader served from site a, then b and c from 03:15, a again from 03:18 | fail |
| Zero `suspect` | 919 `/suspect` on the final leader | fail |

- **Leader moves** [measured, Workers Logs]: `STEP_DOWN` on `repo:131292#1#f0` at 03:14:57 by the confirm rule
  (p99 1,449 ms, 6 of 29 slow) to site 1, then 6 more by `CommitErrors` with `to: None` (holds) at 03:17:17 to
  03:20:06 on all three sites, and elections between.
- **Longest leaderless stretch with requests waiting**: 21.1 s, from 03:14:29.7Z, with no `/commit` served on any
  member (`tests/fanout/raw/tools/leadgaps.py`, a member's `/commit` of 20 ms or more counted as leader work). Next
  longest 12.0 s at 03:15:29.9Z, then 7.6 s. All under F67's 30 s bound; the first sits at the load's first minute.
- **Where it breaks**: at 5,000 agents the leader's commit queue builds to seconds in the first minute (queue p99 3.5
  s), its storage confirms go over 500 ms 6 times in 29, and the confirm rule moves it; after that, followers and the
  edge shed the access-row reads as overloaded. Which of leader CPU, the identity groups, or the shards limits it
  first is not traced. The CPU correlation for this row is not computed: the leader moved 15 times.

**B0-34** (131293, journey 9 replay, 500 agents, 600 s, the home shard killed at 300 s, k left to `fanout`,
standard-2 shards, 03:25:40 to 03:35:40Z) [measured]: **not the row as written, and a fail on the lines it ran.**

The driver runs only the first two parts of B0-34: the replay and one warm-site shard kill. The 5 GB network, the
tip moving along real history, the second network turning hot with the API unreachable, and the hostile owner are not
in `tests/fanout/rows.sh`, so "k set in band from `NetworkLoad` and the hot pool" and "the hostile owner held to its
per-owner pool budget" are not measured.

| Gate line | Measured | Verdict |
|---|---|---|
| k set in band from `NetworkLoad`, at most 8 per site | k stayed 1 (`k 1 at 7`, `k 1 at 2812`); 10 `/fanout` reports reached the leader, answered `ok` in 5 to 20 ms, none stepped | k never moved; not shown in band |
| Human push p99 at most 3 s beyond transfer | 65.2 s (p50 4.1 s); 255 ok, 28 `stale:race`, 1 `stale`, 1 `warming` refusal | fail |
| Clone time to first byte p99 at most 1 s throughout (fail over 5 s) | 3.68 s (p50 739 ms), 602 of 603 | miss, not a fail |
| No Cloudflare API call | none from a request path seen | pass |

Agent pushes 2,244 of 2,500 ok, 247 refused `unpacker error: site N's shard answered N: {"warming":true}`, 5
"Network connection lost"; agent push p99 64.0 s. Commit p99 744 ms (p50 415), leader at term 2 throughout, 107
req/s, 0 feed polls, 1 failed call; no `STEP_DOWN` in the window. Longest leaderless stretch with requests waiting: 0
s. Merge:only p99 67.8 s.

Why k stayed 1 [inference from code]: `fanout` steps k up only when the larger warm-site `cpu_p99` stays over
`FANOUT_UP` (0.75) for `FANOUT_UP_FOR` (180 s) (`crates/git/src/shard.rs`). The shard's `cpu_p99` excludes derived
work and is the p99 of one-second busy fractions over a 2-vCPU container. That the replay's one network did not hold a
standard-2 shard's client validates over 75% for 3 minutes is consistent with the 25.2 agent commands/s and 4.2
pushes/s offered. The 64 s push tails then come from the kill and the `warming` answers, not from CPU-bound shards.
The reports' loads are not logged, so this is not measured.

**B0-35** (131294, 500 agents, 600 s, 500 open PRs, 500 clients polling `mergeable` every 5 s, base moving 10 times a
minute, standard-2 shards, 03:45 to 03:55Z) [measured]:

| Gate line | Measured | Verdict |
|---|---|---|
| Test-merge vCPU within `TEST_MERGE_BUDGET` (fail over budget) | 0.146 vCPU used of 0.25 budgeted, no overrun; 1,224 computed, 24,966 shed, waits p50 10 s, p99 20 s, max 23.5 s | pass |
| Actions dispatch p99 at most 10 s (fail over 30 s) | not in this driver's output: it starts no workflow runs | not measured |
| Zero computes from list fields | 0 list computes of 158,129 list asks; 665 computes from 57,798 single-PR GETs | pass |

`mergeable` answered `UNKNOWN` to 57,798 of 58,992 polls (98.0%), `MERGEABLE` to 1,176, 503 to 18; that is the lazy
design's answer while a merge waits, so it is not a gate line, but 20 s waits behind a 0.25 vCPU budget make it what
clients see. Commit p99 657 ms (p50 360), term 2 throughout, 102.6 req/s, 0 feed polls, 11 failed calls, no
`STEP_DOWN` in the window. Longest leaderless stretch with requests waiting: 0 s. Agent pushes 2,393 of 2,499 ok
(55 `admission full`, 38 `warming` refusals), p99 78.5 s; merge:only p99 16.4 s.

The instruments this row lacked before (test-merge vCPU and list-field computes) come from merge-budget-3 and -4's
`BudgetStats`; they are measured here.

**L0-12, A1 replay** (131295, 500 agents, 600 s, a lease per agent, a leader kill every 60 s, standard-2 shards,
04:27:36 to 04:37:36Z) [measured]: **fail** on member clock offsets, and only part of the row runs.

The driver runs the A1 replay with leases and leader kills. It does not run the 30-minute blackholed stall, the idle
lease with a late overlapping Exclusive acquire, or the 100 conflicting pushes and REST writes per actor kind, so
those lines are not measured.

| Gate line | Measured | Verdict |
|---|---|---|
| No two holders hold overlapping Exclusive leases longer than `DRIFT` (500 ms) | 155 samples, 0 overlapping at all (max -6,422 ms, that is 6.4 s apart) | pass |
| Measured member clock offsets under 500 ms | spread between members 3,211 ms (119 samples, each widened by half its round trip); max offset from the orb 8,035 ms | fail as measured (see below) |
| 500 long-pollers see each event within 3 s at p99 outside leader kills (fail over 2 s) | 3,097 ms p99 outside the 10 kills (18,896 events), p50 1,199; 3,121 ms overall | miss of the 3 s line; fail by the 2 s line |
| Leases across the stall; late acquire; refusal counts per actor kind | not run | not measured |

- Leader: 10 kills, term 2 → 5 (most kills resume `led_term`); 2 `STEP_DOWN`s in the window, `repo:131295#1#f1` at
  04:33:15 by `CommitErrors` to site 2, and `#1#f0` at 04:36:58 (hold). Leaders by `/commit`: site a to 04:33,
  then c, then a from 04:36.
- **Longest leaderless stretch with requests waiting: 8.7 s**, from 04:27:39.1Z, the first seconds of load; then 3.5 s
  at 04:37:03.7Z. The kills themselves show no gap over 1.7 s.
- Agent pushes 2,418 of 2,490 ok (70 × 503 "access rows did not answer … shed the read: overloaded"), p99 17.8 s;
  human pushes 263 ok, 22 `stale:race`, 4 `stale`; merge:only p99 300.8 s.
- Lease renewals: 667 × 200, 686 × 404, 106 × 503; acquires 471 × 201, 39 × 503. The 404 renewals mean a lease the
  agent held was gone when it renewed; whether the kills or expiry under 14 s renewal latency did it is not traced.
- The clock spread: the samples are each member's `/status` `now` against the orb, widened by half the round trip;
  with round trips of seconds under load, the widening alone can exceed 500 ms, so this measures the probe as much as
  the clocks [inference]. A probe that reads all three members' clocks from one isolate would settle it.

Design contradiction, quoted: L0-12's pass line is "within FEED_BATCH plus 2 s (3 s) at p99 outside leader kills",
and its fail line is "feed p99 over 2 s" (`build/BUILD-PLAN.md`, L0-12). A p99 between 2 and 3 s passes one and
fails the other; this run's 3.10 s misses both.

### Round 8 summary (rows m19, standard-2)

| Row | Verdict | Longest leaderless stretch with requests waiting |
|---|---|---|
| B0-29 | miss, not a fail: commit p99 796 ms (≤ 600), merge:only 4.26 s (≤ 2 s), API p99 3.6 s at 98.2% (≤ 2 s, ≥ 99%); 0 stale, leader held | 0 s |
| B0-32 | fail: burst 0 of 500 within 5 s, 29 `warming` refusals, merge p99 19.9 s, 56 `suspect` on identity followers | 0 s |
| B0-31 | invalid: the leader returned to its enam `home` 10 s after the apac transfer | 0 s |
| B0-33 | fail: 50.8 agent commands/s (≥ 140), commit p99 3.9 s, term 2 → 16, 919 `suspect` | 21.1 s |
| B0-34 | partial run, fail: human push p99 65 s (≤ 3 s), TTFB p99 3.7 s (≤ 1 s), k stayed 1 | 0 s |
| B0-35 | pass on the measured lines: 0.146 of 0.25 vCPU, 0 list computes; dispatch not measured | 0 s |
| L0-12 | fail: clock spread 3.2 s (≤ 0.5 s), feed p99 3.1 s; 0 lease overlaps | 8.7 s |

F67: no leaderless stretch over 30 s with requests waiting in any row (worst 21.1 s, B0-33).
Cost through the rows: $47.90 of $150 (containers $10.59). Shards are torn down after each row.

### Round 9: site day m20 on standard-4, smokes, and the entry-cap A/B (2026-11-13)

**Deploy.** Tree `lm1-memory` `b50ec6d` (main 5293a6c: hot-shards-3, merge-p99, m0-carried-7's `ENTRY_SOFT`), sites
`2f633c70`, `f52dce4e`, `0cb8c021`; edge `ea865919`; `SHARD_INSTANCE=standard-4` on every site (set by `--vars` in
`tests/am1/runs/siteday.sh`); shard image hot-shards-4's `sha256:f6d2ef408c76…`. Learners enam 4,000, weur 2,000,
apac 2,000 (8). Every run below is B0-29's shape for 180 s.

**The L0-12 clock rule as built.** The operator's "reject samples whose round trip is over 200 ms" is read as over
200 ms above that site's fastest sample, because apac's idle round trip from this orb through the tools edge is 240
to 590 ms [measured on 131291], so a flat cap would reject every apac sample. The operator accepted this reading.

| Run | Agents | Entry limit | Commit p50 / p99 (ms) | Queue to append p50 / p99 | Round p50 | Commands per entry | Agent / human push p99 (s) | merge:only p99 | API p99, served |
|---|---|---|---|---|---|---|---|---|---|
| m20s250 (131297) | 250 | 60 KB | 356 / 1,469 | 205 / 1,332 | 153 | 2.16 | 7.7 / 5.7 | 4,051 | 2,810, 99.1% |
| m20s500 (131298) | 500 | 60 KB | 791 / 1,619 | 642 / 1,461 | 151 | 3.50 | 8.3 / 7.0 | 966 | 2,406, 100.0% |
| A/B 1 (131300) | 500 | `ENTRY_MAX` | 453 / 702 | 247 / 507 | 202 | 4.67 | 7.2 / 6.3 | 1,424 | 2,474, 100.0% |
| A/B 2 (131301) | 500 | 60 KB | 739 / 1,718 | 578 / 1,583 | 151 | 3.38 | 8.0 / 6.9 | 1,117 | 2,331, 100.0% |
| m19 s500, for reference | 500 | `ENTRY_MAX` | 341 / 567 | 194 / 421 | 142 | 3.41 | 6.6 / 8.5 | | |

The entry limit is set per member through test-faults `/stats?entry=max|<bytes>` (`tests/fanout/smoke.sh`'s fourth
field); a plain `/stats` read confirms `entrySoft` 1,500,000 on the uncapped arm. No run changed leader (term 2
throughout), no `STEP_DOWN`, and API reads were served at 99.1 to 100%.

**Cause [measured, A/B on one deploy]: `ENTRY_SOFT` (60 KB) doubles B0-29's commit p99.** With the cap, commands wait
p50 578 to 642 ms and p99 1,461 to 1,583 ms for an entry, against 247 / 507 ms uncapped; the quorum round itself is
the same or faster (151 against 202 ms p50). The leader is not busy: its CPU is about 9% of one core in every 10 s bin
of m20s500 and no turn reached 100 ms (`tests/fanout/raw/tools/m20.txt`), so this is not m19's heavy-turn shape.
Why [inference from the code and the plan]: journey 9 inlines agent packs of 10 to 70 KB (`INLINE_MAX_HOT` 64 KiB), so
a 60 KB entry carries about one push past its first command. One entry is in flight at a time, at about 5 entries a
second (round about 150 ms plus deps about 55 ms), and the 4.17 pushes a second alone fill most of them, so commands
queue behind entries. The cap's capacity term (`Admission::capacity`, `soft / cmd_bytes` commands per entry) shrinks
with it, so admission does not shed the excess either. m0-carried-7 measured on B0-02, which pushes no inline packs.

**B0-34's seed and the edge's push memory (design contradiction).** Every 60 MB push of the torvalds/linux seed ended
`exceededMemory` or "the Worker's code had hung" on `graft-m2b-edge` (6 of 6 tries, 08:31 to 08:35Z). The edge reads
a receive-pack body into one `Vec` (`read_capped`), so a push of tens of MB passes the 128 MB isolate. DESIGN,
Milestone 0, says of a 2 GB push: "Enterprise completes with Worker memory under 100 MB", and its fail line is "needs
buffering". The edge as built buffers. The seed now uses 30 MB steps (`tests/fanout/b034plan.py`), and linux's 53 MB
root commit is carried by its tree's parts first.

Cost after round 9's smokes and A/B: $54.80 of $150 (containers $14.78); every shard torn down with `--pools`.

### Round 10: site day m21 and the ENTRY_SOFT A/B on the fixed cap (2026-11-13)

**Deploy.** Tree `lm1-memory` `74f0e1a` (main a7d28e5: m0-entry-soft-packs `ff29c65`, hot-shards-4, M4-WP2's pure
half, f67-wide), sites `78a8c8f8`, `955377dc`, `2aeae0cc`; edge `bc314b3a`; `SHARD_INSTANCE=standard-4`; shard image
hot-shards-4's `sha256:f6d2ef408c76…`. A first attempt on 745c116 (main 59308e9) stopped after site a, on a wrangler
"fetch failed" for site b, and was replaced by this one. Checks on 74f0e1a: fmt, both clippies, wasm32 clippy for
site, edge and ops with and without `test-faults`, `cargo test --workspace`, `go test`, and the three design checks
(`check_links` in a clean worktree, 3,168 citations) pass. Long sweep (2,000 seeds × 10,000 steps) ok in 1,275 s;
Porcupine Ok on `hist.jsonl` (150,828 operations), `.gray` (2,044,734) and `.live` (5,253,576).

**A/B (diagnosis, not a row).** B0-29's shape, 500 agents, 180 s, learners enam 4,000, weur 2,000, apac 2,000, k 4.
The uncapped arm sets `ENTRY_MAX` on every member through test-faults `/stats?entry=max`.

| Run | Entry limit | Commit p50 / p99 (ms) | Queue p50 / p99 | Round p50 / p99 | Confirm p99 | Far follower (site 2) append p50 / p99 | Agent push p99 | merge:only p99 | API p99, served |
|---|---|---|---|---|---|---|---|---|---|
| m21cap500 (131302) | `ENTRY_SOFT`, as shipped | 381 / 717 | 205 / 499 | 168 / 297 | 56 | 262 / 635 | 6.5 s | 1,157 | 2,639, 99.97% |
| m21max500 (131303) | `ENTRY_MAX` | 394 / 761 | 213 / 508 | 169 / 331 | 90 | 226 / 577 | 6.9 s | 1,476 | 2,610, 99.96% |

The fixed cap is no worse than uncapped (717 against 761 ms p99; round 9's 60 KB cap was 1,718), so the gate rows run
capped, as shipped. Neither arm meets B0-29's 600 ms line. Leader held at term 2 in both, 0 feed polls on it, 7 and 0
`/suspect`. In both arms the queue (admission to append, which holds the entry's dep reads, p99 205 to 331 ms) is
about two thirds of the commit p99 and the round the rest; the round's p50 equals the near follower's (site 1)
AppendEntries p50, so the quorum leg is the near follower's call, not the leader's confirm (p99 56 to 90 ms).

**B0-34's seed.** m3org/b034-mono holds 5.02 GB of torvalds/linux (260 pushes of at most 30 MB, 6,075 s); push 4,
linux's 52 MB root commit carried in parts, was cut off 4 times at the edge before it went. The repos are kept.

**Rows m21 do not count: every row ran at k 1.** `k.mjs repo:N 4` answered `{"grants":[[0,[]],[2,[]]],"k":1,"want":3}`
for every row (131304 to 131309), so the forced k was capped to 1 by pool-grants' ledgers. The cause: the smokes
before them (131302, 131303) set k 4 for owner m3org, and `smoke.sh` and `rows.sh` read k after a run but never set it
back, so those networks kept their leases (`PoolLedger::ask`) after their shards were torn down. Each owner is held to
`HOT_POOL - 1` = 6 slots, and the two networks held all 6 (3 each) on every site. Asking k 1 on 131302 and 131303 freed
them, and a k 4 ask on 131304 was then granted at once (`[[0,[2,1,6]],[2,[2,1,6]]]`). `rows.sh` and `smoke.sh` now set
k 1 after each run, and `rows.sh` skips a row whose forced k was not granted.

What the k 1 rows showed is still evidence of a single shard under the row's load, recorded as such:

| Row (m21, k 1) | Leader | Commit p50 / p99 (ms) | Queue p99 | Notes |
|---|---|---|---|---|
| B0-29 (131304) | held, term 2 | 1,114 / 30,306 | 29,950 | 2,462 / 2,500 agent pushes ok, p99 37.7 s; merge:only p99 8.1 s; API p99 2.4 s, 99.4% served; leader CPU 31 to 37% of a core |
| B0-32 (131305) | held | 391 / 871 | 712 | burst 225 / 500 ok, 274 `full`; burst p99 22.7 s |
| B0-31 (131306) | apac held, term 3 | 22,151 / 30,549 | 30,354 | rehomed on apac; agent pushes p99 49 s; placement-failing half 1,265 `full` |
| B0-33 (131307) | held | 30,017 / 30,674 | 30,515 | 864 of 12,432 agent pushes ok; 1,207 `/suspect` |
| B0-35 (131308) | held | 428 / 934 | 690 | test-merge `computed` 10,191, waitMax 17.3 s on site 0 |

At k 1 the queue waits reach `AGENT_WAIT` (30 s) and agent commands are refused `Full`: one shard's validates hold
pushes back, the leader's admission fills, and every command class waits behind them. These are not the rows as
written (k 4), so none is judged.

**Design gap (pool-grants, for the parent).** A network whose shards are torn down at k > 1 keeps its pool lease
until `LOAD_STALE` of later ledger calls, and `PoolLedger::settle` shifts every lease forward past a gap of
`LEDGER_GAP`, so on a test install with a stream of short-lived networks of one owner the lease never lapses while
other networks keep calling. DESIGN's lease text says leases "lapse past LOAD_STALE unless renewed"; here they were
renewed by nobody and still held for over an hour (11:36Z to 13:11Z).

### Round 10b: rows m22 at k 4 on main 84d29c2 (2026-11-13)

**Deploy m22.** `lm1-memory` `24dec508` (main 84d29c2: F67 rules 10 to 13; not yet c0f521b). Sites `9e2b7b32`,
`2611f7ee`, `cd860099`; edge `f9d34dec` (from `565bdb01`); `SHARD_INSTANCE=standard-4`; image
`sha256:f6d2ef408c76…`. Every row got k 4 (`rows.sh` checks the grant), learners enam 4,000, weur 2,000, apac 2,000,
one fresh repository per row, run in order 13:55Z to 15:58Z. Raw data: `tests/fanout/raw/m22/` (the a1 rows and logs,
`rows-m22.status`, the site day's logs, and each churned row's `member alarm` step-downs from Workers Logs,
`alarms-*.txt`).

| Row (m22, k 4) | Repo | Leader: terms | Commit p50 / p99 (ms) | Agent pushes ok | Leader `/suspect` | Other |
|---|---|---|---|---|---|---|
| B0-29, 600 s | 131310 | held, term 2 | 512 / 1,158 | 2,497 / 2,500, p99 7.8 s | 36 | merge:only p99 2,503 ms; API p99 2,863 ms, 99.7% served |
| B0-32, 180 s | 131311 | held, term 2 | 378 / 1,265 | 641 / 757 | 18 | burst 390 / 500 ok, p99 105 s |
| B0-31, 600 s | 131312 | apac held, term 3 (rehome) | 30,044 / 30,899 | 879 / 2,500 (1,620 `full`) | 11 | round p99 401 ms; the queue sets the commit |
| B0-33, 300 s, 5,000 agents | 131313 | term 2 to 10, site 2 last | 30,003 / 30,523 | 224 / 12,498 (8,595 `full`) | 28,868 | round p99 741 ms |
| B0-35, 600 s | 131314 | term 2 to 23, site 0 last | 6,277 / 17,684 | 1,109 / 2,500 | 9,551 | 118 commit samples; API 45,485 of 117,223 served |
| L0-12, 600 s | 131315 | term 2 to 7, site 2 last | 26,153 / 31,020 | 1,692 / 2,490 (634 `full`) | 857 | 10 leader kills; feed p99 outside them 2,444 ms |

Every row fails its gate line; the commit line alone fails each (B0-29's 1,158 ms is past its 1 s fail value).

**Step-downs (Workers Logs, `member alarm: StepDown`).** In B0-33, three, all `Confirm`: 14:40:05Z site a (p99
1,245 ms, 6 slow of 20, to site 1), then site c twice (14:40:31Z p99 2,502 ms; 14:42:44Z p99 1,000 ms; to none). In
B0-35, four, all `CommitErrors` (14:49:14Z site a, 3 of 6 calls failed to each follower; 15:02:02Z site a, 42 of 48 to
site 1; 15:03:47Z site c; 15:04:20Z site b). In L0-12, seven: five `CommitErrors` and two `Confirm` on site a
(15:49:57Z p99 911 ms; 15:55:19Z p99 1,622 ms). The terms climbed far past the step-downs (B0-35 reached term 23
after four), so most terms were elections that did not seat a leader.

**Reading (with the parent's churn reviewer, 2026-11-14).** Not F67 rules 10 to 13. The climb is load-driven and older
than them: it reproduces in the simulator on both a7d28e5 and 84d29c2 when confirms are slow on every member. A
STEP_DOWN hands leadership to a follower whose confirms are just as slow, which then holds; after that,
`/suspect`-driven elections that cannot win still raise the term, because a voter adopts a candidate's term before
it checks the candidate's log. The raft side is with B0-02's thread. This branch's lever is the leader's confirm time
(`confirmSplit`, round 12). On dd046f2 (round 12) B0-33 moved its leader once, on the same `Confirm` step-down, and
the term did not climb. `tests/fanout/terms.mjs` now also logs each member's promise (`promiseUntil` less its `now`),
so a follower withholding its vote before an election shows up, and it reads each site's current voter by name: a
call by site alone reached the first generation's object after fresh-repo's background placement replaced a voter.

### Round 11: m23, the ENTRY_SOFT 100 KB A/B on m0-carried-8 (2026-11-13)

**Deploy.** Worktree `e3ba3b58`: origin/m0-carried-8 `c8c7943` (base main 84d29c2: ENTRY_SOFT 100,000 bytes, Agent
inline packs not counted; APPEND_BYTES_MAX 100 KB past an AppendEntries' first entry; AppendEntries pipelined 4 per
voter, off on a lossy link; LOOP_LAG step-down) merged with `lm1-memory` `d2320985` (main e1fee025, the pool lease
heartbeat, and this branch's queue split). Sites `172052f9`, `b0ff9cd5`, `22907958`; edge `a8257c11`;
`SHARD_INSTANCE=standard-4`; image `sha256:f6d2ef408c76…`. Before it, fmt, wasm32 test-faults clippy for the site,
`graftyard-site --lib` and `graftyard-raft` tests passed on the merged tree; m0-carried-8 ran the long sweep,
Porcupine and full checks on `c8c7943`.

**A/B (diagnosis, not a row).** B0-29's shape, 500 agents, 180 s, k 4 granted, learners enam 4,000, weur 2,000,
apac 2,000. The 60 KB arm pins `/stats?entry=60000` on every member (test-faults `set_entry_soft`).

| Arm | Commit p50 / p99 (ms) | Queue p99 | Slot p99 | Post p99 | Round p50 / p99 | Confirm p99 | Far follower append p50 / p99 | Commands per entry | Agent push p99 |
|---|---|---|---|---|---|---|---|---|---|
| As built, 100 KB (131319) | 518 / 869 | 639 | 538 | 360 | 220 / 419 | 227 | 335 / 593 | 4.84 (3,163 / 654) | 7.5 s |
| Pinned 60 KB (131321) | 437 / 912 | 708 | 687 | 260 | 178 / 332 | 77 | 232 / 447 | 3.97 (3,171 / 799) | 7.8 s |

Both arms held their leader at term 2 (10 and 26 `/suspect`), 0 feed polls on the leader, 752 / 753 and 728 / 728
agent pushes ok, merge:only p99 1,484 and 1,294 ms, API p99 2,566 and 2,760 ms at 99.99 and 99.8% served.

**Reading [measured on one deploy, one run per arm].** The 100 KB cap packs 22% more commands into each entry (4.84
against 3.97) and its p99 commit is 43 ms lower, within run-to-run spread; its p50 is 81 ms higher. Larger entries
make the round longer (p50 220 against 178 ms, p99 419 against 332) and the leader's storage confirm much longer
(p99 227 against 77 ms), and the slot wait shorter (p99 538 against 687). Neither arm meets B0-29's 600 ms line.
In both, a command's queue is mostly the wait for the entry ahead of it to commit (`queue:slot`), not its own entry's
cut and dependency reads (`queue:post`, p50 2 to 7 ms): with one entry in flight, commit p99 is about one round wait
plus one round. So the lever for 600 ms is the round's tail (the near follower's AppendEntries and the leader's
confirm), not the dependency reads.

**Fresh repositories.** Two of four creates on m23 failed in setup: 131316's create answered 503 "0 of 3 joined"
with "RepoId(DbId(131316)) has no RepoOwner row" from each member, and 131317's and 131320's `main` was missing from
ls-refs for seconds after the 201 (`a1.mjs` stopped with "the repository has no main"). `a1.mjs` now waits up to
120 s for `main` and logs the wait; 131321's appeared after 1,704 ms. Not yet traced.

Cost after round 11: $86.98 of $150 (containers $29.23); every shard torn down.

### Round 12: m24c, B0-33's leader churn traced, and the B0-29 entry A/B against ENTRY_MAX (2026-11-14)

**Deploy m24c.** `lm1-memory` `6065130` (main dd046f2: m0-carried-8's ENTRY_SOFT 100 KB, APPEND_BYTES_MAX 100 KB,
PIPELINE 4 and LOOP_LAG; F67 rules 10 to 13; the pool lease heartbeat c0f521b; the tier fold fix; fresh-repo). Sites
`7a2a620c`, `f7bb90ce`, `27504d93`; edge `66d2d952`; `SHARD_INSTANCE=standard-4`; image `sha256:f6d2ef408c76…`. Full
checks passed on `6065130`.

**B0-33's churn (smoke, repo 131323, k 4, 8 learners, `tests/fanout/terms.mjs` once a second).** One leader move in
the run, and it was the leader's own STEP_DOWN on its storage confirm: at 03:19:15Z a CommitErrors trip (3 of 5 calls
to each follower failed) held; at 03:20:45Z site 1 led at term 3; the old leader's alarm log shows a Confirm
step-down at 03:20:54Z with confirm p99 1,673 ms. 1,105 `/suspect` reports started no election, no LoopLag
step-down fired, and no transfer or drain hand-off ran, so F67 rules 10 to 13 and LOOP_LAG are not implicated
[measured]. m22's climb from term 2 to 10 on 84d29c2 does not reproduce on dd046f2. The same smoke without learners
(131322) held term 2 with one CommitErrors trip that held, commit p99 5,163 ms. B0-33 still fails on load: commit
p99 30.9 s, 1,454 of 12,441 agent pushes ok and 10,966 refused `full`, because admission sizes its capacity from the
round, which includes the 1.7 s confirm.

**B0-29 A/B (diagnosis, not a row).** 500 agents, 180 s, k 4 granted, learners enam 4,000, weur 2,000, apac 2,000,
fresh repositories. The ENTRY_MAX arm sets `/stats?entry=1500000` on every member.

| Arm | Commit p50 / p99 (ms) | Queue p99 | Slot p99 | Post p99 | Round p50 / p99 | Confirm p99 | Far follower append p50 / p99 | Commands per entry | Agent push p99 |
|---|---|---|---|---|---|---|---|---|---|
| As built, 100 KB (131324) | 347 / 575 | 417 | 343 | 240 | 149 / 256 | 102 | 245 / 459 | 3.41 (3,180 / 932) | 7.2 s |
| ENTRY_MAX (131325) | 349 / 614 | 433 | 336 | 208 | 154 / 276 | 100 | 332 / 635 | 3.41 (3,167 / 928) | 7.1 s |

Both arms held their leader at term 2 with no `/suspect` on the leader and 0 feed polls on it. Agent pushes 759 /
759 and 767 / 767 ok; merge:only p99 2,363 and 1,341 ms; API p99 2,707 and 3,188 ms (the ENTRY_MAX arm had 131
read 503s).

**Reading [measured on one deploy, one run per arm].** On a fresh repository the cap does not bind: both arms cut
the same 3.41 commands per entry, so the 100 KB cap and ENTRY_MAX give the same entries, and the 39 ms gap in commit
p99 is run-to-run spread. The as-built arm's 575 ms is the first B0-29-shaped run under the 600 ms line, against
869 ms on m23 (`c8c7943`) with 4.84 commands per entry; this deploy's rounds are shorter (p99 256 against 419 ms) and
its confirm too (p99 102 against 227). One run, so not a gate result. narrow-reads-3's graft-m3e runs (500 open PRs:
60 KB cap 2.9 s against ENTRY_MAX 949 ms) differ from these in the repository, not the tree's cap: there the cap
binds, here it does not. Whether a PR-heavy repository's non-agent commands fill ENTRY_SOFT is not yet measured
[unverified].

**Confirm split instrument (for m25).** `/stats?summary=1` now carries `confirmSplit`: for each leader confirm, the
input's step to the start of its transaction (`gate`, the deferred flush's macrotask), that start to `sync()`'s
continuation (`sync`), a zero-delay timer set beside it (`turn`, the isolate's turn queue), and `storage`
(`sync - turn` per sample), grouped by the transaction's bytes (under 100 KB, under 1 MB, larger). `a1.mjs` reports
it as `leader.confirmSplit`. Its tests fail on plants 89 to 91. Workers' clock advances only across I/O, so the
synchronous `transactionSync` itself reads as 0 ms and shows up in later inputs' `gate` and `turn`.

Cost after round 12: $106.76 of $150 (containers $45.53); `teardown.py --sweep` reported "sweep ok".

**B0-29's repository shape (adopted by the operator for every B0-29 gate run, commit and API lines alike, 2026-11-14; narrow-reads-3's idle-PR, k 1 runs do not count toward the streak).** B0-29 is one row, so both
threads gate it on one shape. The fresh-repository arms above (575 and 614 ms) and narrow-reads-3's 500-PR runs on
graft-m3e (1.9 to 2.9 s) ran different repositories, and the row text names PrSync, test-merge records and check runs,
which need open PRs:

- a fresh repository per run (`tests/am1/runs/mkrepos.mjs`), `main` at its first commit, gated about 10 to 15 minutes
  after create, once its PRs are open and learners and k are placed;
- 500 draft PRs opened inside the run (`a1.mjs b0-29 --open-prs 500`), one per agent head branch, so every agent push
  in the window is a head push with its PrSync and test merge; B0-35's mergeable pollers stay off;
- about 520 refs at the start (500 PR heads, `main`, 20 shared human refs), plus about 10 a minute from merges;
- journey 9's plan at 500 agents for 600 s, learners enam 4,000, weur 2,000, apac 2,000, k 4, standard-4 shards, no
  test-faults override. `tests/fanout/rows.sh` runs B0-29 with `--open-prs 500`.

narrow-reads-3's m3e runs opened their 500 PRs in a separate 1 s `b0-35` run, and the B0-29 agents then pushed their
own `claude/a1-<run>-i` refs, not those heads, so the PRs were idle fold rows and no head-push PrSync or test merge
ran; those runs also held k 1.

**B0-29's window (agreed with narrow-reads-3, 2026-11-14).** The row's 600 s load window, and every line judged in
it (commit, merge, `Stale`, leader load, API p99 and served share), starts only after setup has finished. In order: `mkrepos.mjs`; learners and k (`rows.sh` and `smoke.sh` place
them before `a1.mjs` starts, so the learners join a near-empty log and follow the PR opens as they commit); the 500
PR opens inside `a1.mjs` (their transient 503s, about 160 per setup on graft-m3e, are setup's and counted only in its
"open PRs" row); then a wait of up to 120 s for every learner to apply the leader's commit index, logged with the
leader's `wasmMemMb` ("learners caught up"). So no learner joins or catches up inside the window (28 to 73 MiB of
leader high-water on graft-m3e).

### Round 13: m25 on m0-adaptive-append, its B0-29 A/B, and B0-29 on the agreed 500-PR shape (2026-11-14)

**Deploy m25.** Worktree `731bbb60`: `lm1-memory` `1d7a7805` (main dd046f2 plus this branch's `confirmSplit`) merged
with origin/m0-adaptive-append `4ba495d` (main 82a0bf9: pipelining stops only on lost calls, a per-link call bound
that halves on a lost call and grows back, a pipelined call that overtakes the one ahead waits behind it, and a behind
voter gets calls until PIPELINE are out). The merge's `crates/raft` equals `4ba495d`'s, whose long sweep, Porcupine and
B0-02 gate (16 of 16) its thread ran; fmt, site wasm32 clippy with and without `test-faults`, and the site and raft
tests pass on the merge. Sites `8dbaa3fb`, `8e51a76d`, `366c1f53`; edge `3a9061ff`; `SHARD_INSTANCE=standard-4`; image
`sha256:f6d2ef408c76…`. The first site day failed at once on the worktree's missing ignored `tests/am1/raw/runs`.

All three arms: 500 agents, 180 s, k 4 granted, learners enam 4,000, weur 2,000, apac 2,000, each learner caught up
before the window.

| Arm | Commit p50 / p99 (ms) | Queue p99 (slot / post) | Round p50 / p99 | Confirm p99 | Near / far follower append p99 | Commands per entry | Agent push p99 | API p99, served |
|---|---|---|---|---|---|---|---|---|
| main dd046f2, fresh repo (m24c, 131324) | 347 / 575 | 417 (343 / 240) | 149 / 256 | 102 | 255 / 459 | 3.41 (3,180 / 932) | 7.2 s | 2,707, 99.9% |
| adaptive-append, fresh repo (m25, 131328) | 350 / 682 | 489 (377 / 245) | 152 / 286 | 51 | 291 / 701 | 3.44 (3,164 / 920) | 8.9 s | 2,684, 99.9% |
| adaptive-append, 500 open PRs (m25, 131327) | 429 / 865 | 575 (514 / 249) | 192 / 407 | 71 | 459 / 892 | 3.83 (2,875 / 750) | 13.6 s | 3,408, 99.0% |

All three held term 2 with 0 feed polls on the leader; agent pushes 759 / 759, 749 / 749 and 741 / 743 ok; merge:only
p99 2,363, 1,353 and 1,647 ms.

**adaptive-append's A/B [measured, one run per arm, on two deploys an hour apart].** On a fresh repository the tree is
not better on journey 9: commit p99 682 against 575 ms, with the same entries (3.44 against 3.41 commands per entry)
and a lower leader confirm (51 against 102 ms). The difference is the far follower's AppendEntries (p99 701 against
459 ms) and, behind it, the slot wait (377 against 343). The round's p99 still follows the near follower (286 against
256), so the far leg reaches commit only through the wait for the entry ahead. One run each is inside run-to-run
spread for p99 (round 11's two arms on one tree differed by 43 ms; rounds 10 and 12, 44 and 39); the far leg's 242 ms
is the one large move, and the 500-PR arm on the same tree shows it again (892).

**B0-29 on the agreed shape (500 open PRs) [measured, one smoke].** 500 of 500 PRs opened with no failure; the learners
caught up in 2.6 s, leader `wasmMemMb` 20.6 MiB then. Against the same tree's fresh arm, PR-heavy commands fill entries
somewhat more (3.83 against 3.44 commands per entry; 155 of 750 confirms, 21%, carried 100 KB to 1 MB, p50 143 KB,
against 131 of 920), and they do not slow confirms: `confirmSplit` storage p99 is 71 ms under 100 KB and 76 ms from 100
KB to 1 MB, against 51 and 52 on the fresh arm. The commit's extra 183 ms is the slot wait (514 against 377) behind a
longer round (p99 407 against 286) whose near and far legs both slowed (459 and 892 against 291 and 701). So on this
shape B0-29's commit line is the round's follower legs, not the leader's confirm and not entry fill.

**Confirm split's turn queue reads 0 throughout** (`gate` and `turn` p99 0 in every group of every arm). Either the
deferred flush and a zero-delay timer run in the same turn as the input on a lightly loaded leader, or Workers' clock,
which advances only across I/O, hides the wait; at B0-29's load `sync` equals storage. B0-33's smoke, whose confirms
reached p99 1.7 s, is where the split decides between the two levers.

**Driver fixes.** `a1.mjs` read the leader from one `/status` round; the first fresh arm (131326, run `mwfx2jp3`) got
none ("leader site undefined") and kept no commit split, so it was rerun as 131328. `a1.mjs` now falls back to
`leaderOf` (up to 20 s). In that first arm the learner wait reported 0 of 8 caught up after 120 s while each learner,
read by name later, stood at index 918 [unverified why].

Cost after round 13: $109.01 of $150 at 05:35Z (containers $45.79); every sweep "sweep ok".

### Round 14: B0-33's confirm split, and why the row fails (m25, 2026-11-14)

**Run.** Deploy m25 (round 13). B0-33 smoke, repo 131329 (`fm25s33-b033`), 5,000 agents, 300 s, k 4 granted, learners
enam 4,000, weur 2,000, apac 2,000, `tests/fanout/terms.mjs` once a second with each follower's promise (raw:
`tests/fanout/raw/terms-m25s33.jsonl`).

**Leader: held.** Term 2 from start to end on all three members; no `member alarm` StepDown in Workers Logs for
05:36Z to 05:47Z; both followers held a promise to the leader the whole run (it lapsed and was renewed within a
second at 05:39:10Z and in the idle tail after 05:46:51Z, never with an election). So on this tree the leader-change
and `suspect` lines hold for one run, where m22 (84d29c2) climbed to term 10 and m24c (dd046f2) moved once on a
`Confirm` step-down.

**Confirm split [measured].** The leader's confirm is short and storage-bound: confirm p99 166 ms (m24c's B0-33: 1,673
ms). Of 1,351 timed confirms, 1,348 carried under 100 KB (p50 3.6 KB, p99 46 KB): storage p99 149 ms, `gate` and
`turn` p99 0. The other 3 carried 416 to 834 KB and up to 1,560 statements: storage p99 818 ms, `gate` 154 and `turn`
28 ms. So the answer to the parent's question on this run is storage, not the turn queue, and it is driven by the few
large transactions (many statements), not by entry bytes in general; with Workers' clock advancing only across I/O,
synchronous CPU inside `transactionSync` would read as later inputs' `gate` and `turn`, and those read 0 for 99.8% of
confirms, so the isolate was not busy enough to delay them [inference from the clock rule].

**The 3 large confirms [partly measured].** They fell in the window (stats reset at 05:41:07Z), so they were not the
archive alarm's `Promoted` (index 32) and `Archived` (index 34), which Workers Logs show at 05:37:07Z and 05:37:08Z,
before it; no other archive, promote or install line appears for the group through 05:47Z, and the learners' "no
install source" lines are from 05:37Z too. So they were not Promoted, Archived or an install. What is left is a fold
write: a leader transaction of up to 1,560 statements is many entries' apply in one input (each applied request
writes its `request` row, and `store_fold` seals each tier row it changed), together with the queued appends of the
inputs a deferred flush holds, whose log rows carry up to `AGENT_INLINE_ENTRY` of Agent inline packs each. A learner
catch-up writes nothing on the leader. Which of the two carried the bytes is not measured: `/stats?summary=1` now
keeps the statement mix (verb and table, count and bytes) of a member's newest 20 transactions of 100 KB or more
(`bigTx`), and `a1.mjs` records it, so m26's B0-33 names them [the split between apply and appends is unverified].

**Why B0-33 still fails: admission capacity.** Commit p50 / p99 29,888 / 30,363 ms, all of it queue (`queue:slot`
p99 30,096 ms: commands wait AGENT_WAIT for the entry ahead) while the round is short (p50 196, p99 364 ms) and
`queue:post` p99 245 ms. The leader cut 1,351 entries of 9,844 commands in the 335 s window, 7.3 per entry and about
29 commands/s, against 252 agent-class commands/s offered: 2,328 of 12,538 agent pushes ok (10,200 `full`), about 7.8
agent pushes and 20.8 side commands/s admitted against the gate's 140. With entries of 3.6 KB at p50, ENTRY_SOFT does
not bind them, so the cut is held to about 7 commands an entry by the token bucket, `AGENT_SHARE` times the measured
capacity, where capacity is ENTRY_SOFT over the moving average of command bytes times entries per second at the
moving average of the round (`crates/raft/src/member/leader.rs`, `Admission::capacity`). The average command bytes
counts every class's soft bytes, so a few large human or bot commands lower the per-entry estimate for all
[inference; not measured on this run]. `/stats?summary=1`'s `admission` now also returns `capacity`,
`retryAfterS`, and the two moving averages the estimate uses, `cmdBytes` and `roundMs` (`AdmissionView`, a field
addition in `crates/raft/src`, no behavior change), and `admittedBytes` gives each class's admitted commands' priced
bytes (count, p50, p99, max, sum, and the sum of those of 10 KB or more). `a1.mjs` samples the leader's admission
every 10 s into the row (field `admission`). Their tests fail on plants 92 and 93.

The parent's arithmetic (2026-11-14): about 29 commands/s admitted at a 196 ms round means the bucket sized itself
near 58/s, which at ENTRY_SOFT 100 KB needs `cmdBytes` near 9 KB, while entries carried 7.3 commands at a p50 of
3.6 KB, so a mean raised by a few large commands may price every entry as byte-full [inference; m26's B0-33 runs
first to measure it].

Other lines: human pushes 109 of 189 ok, p99 29.7 s; merge:only p99 10.2 s; the fast user 470 of 495 ok (25 × 503);
read:ref p99 9.0 s; leader 457 req/s with 0 feed polls.

**Learner wait.** The pre-window wait again read 0 of 8 caught up after 120 s at commit 114 on a fresh repository, as
in round 13's first fresh arm, while the 500-PR arms read 8 of 8 within 3 s [unverified why; a learner's `/status`
before it has applied its first append may not report `applied`].

Cost after round 14: $110.68 of $150 (containers $45.90); the run's sweep "sweep ok".

**Gate-row tree.** The parent: gate rows build on main, not on m25's merge (adaptive-append is not merging yet).
`lm1-memory` now merges origin/main `34a3316f`, which brings liveness-split-vote (a21ecdf0, a raft change whose long
sweep and Porcupine its own record shows passing, `tests/m0/results/split-vote.md`). Full checks on the merge: fmt,
workspace clippy, wasm32 clippy for site, edge and ops with and without `test-faults`, `cargo test --workspace`,
`go test` and `check_design` pass; `check_names` and `check_links` fail only on `.worktrees/` and pass in a clean
worktree (2,605 files; 3,235 citations).

### Round 15: m26c on main 2456b8f, and what sets B0-33's admission capacity (2026-11-14)

**Deploy m26c.** lm1-memory a3114e0a = main 2456b8f6 (liveness-slow-storage) plus the admission instruments of
c5d38b11: sites a 75e5b130, b df53889e, c afe13337; edge 6bcce970; shards standard-4, image f6d2ef40; 06:50Z. Full
checks pass on a3114e0a (checks29; `check_names` and `check_links` pass in a clean worktree, 2,608 files and 3,235
citations). The raft tree's long sweep (sweep15) exits 0, longest healthy-quorum stall 15,735 / 25,169 / 24,655 ms;
Porcupine is Ok on all three histories (156,325, 2,230,496 and 6,087,300 operations). Plants 92 (capacity sample) and
93 (`admittedBytes` pricing) are caught.

**Instruments.** `/stats?summary=1` admission reports `capacity`, `retryAfterS`, `cmdBytes` and `roundMs`
(`AdmissionView`, no behaviour change); `admittedBytes` gives each class's count, p50, p99, max and sum of the bytes
admission priced (encoded length less an Agent inline pack); `bigTx` keeps the verb and table mix of the newest 20
transactions of 100 KB or more. `a1.mjs` samples the leader's admission every 10 s into the row's `admission`.

**Run.** B0-33 smoke, repo 131330 (`fm26s33-b033`), run mwg1tdsf, 5,000 agents, 300 s from 06:57:54Z, k 4, learners
enam 4,000, weur 2,000, apac 2,000 (0 of 8 caught up at the 120 s wait, as on m25; unexplained). Raw:
`tests/am1/raw/a1-b0-33-mwg1tdsf.jsonl`, `tests/fanout/raw/terms-m26s33.jsonl`.

| line | m26c |
|---|---|
| commit p50 / p99 | 30,012 / 30,975 ms (queue:slot p99 30,359; round p99 663; confirm p99 518) |
| agent pushes | 897 ok of 12,480 (11,525 `full`) |
| entries / commands | 504 / 3,843 (7.6 a entry) |
| fast user | 479 of 497 200, 18 503, p99 5,454 ms |
| side:human | 493 of 516 ok, 23 GraphQL 503, p99 6,280 ms |
| merge:only | 16 of 18 200, 2 409, p99 7,560 ms |
| `/suspect` on leader | 1,696; leader 280 req/s |
| leader changes | 2 (0 → 1 at term 3, 1 → 2 at term 4), leaderless at most about 1 s and about 3 s (corrected in round 15c) |

**Admission samples [measured].** Capacity fell from 78/s at start to 30–47/s and stayed there; `retryAfterS` rose to
41–66 s; `agentQueued` sat at about 1,000 (AGENT_QUEUE_MAX); `roundMs` 198–322; `cmdBytes` 8,000–10,900.

**The parent's inference is not confirmed [measured].** It was that a moving mean pushed up by a few large commands
prices every entry as byte-full. `admittedBytes`: agent n 19,490, p50 10,842, p99 11,495, max 11,507 bytes; human n
757, p50 10,772, p99 20,464. The admitted commands are uniformly about 10.8 KB, so `cmdBytes` ≈ 10 KB is the mean of
uniform commands, not of a skewed mix, and capacity = ENTRY_SOFT 100 KB / 10 KB × 1000 / round 200–320 ms ≈ 30–50/s is
what the formula gives for them. The same pricing fills each entry: `cut` stops at ENTRY_SOFT of command bytes, about
9 commands, and entries carried 7.6.

**What binds: priced bytes are not stored bytes [measured, with the cause of the 10.8 KB a hypothesis].** The leader's
505 confirm transactions (one an entry) carried p50 1,910 bytes and p99 56 KB, p50 one statement: an entry of 7.6
commands priced at about 82 KB stored about 1.9 KB. Admission prices a command by `Record::encoded_len` of the command
(`crates/raft/src/member/leader.rs`, `cmd_bytes` and `Queued::soft_bytes`), but the log row holds the entry's events
(`EntryBody::Events`), and the confirm is what storage writes. So both the entry fill and the capacity estimate are
priced about 5× above what the entry costs to store. The likely source of the 10.8 KB is `IssueCall.render`
(`crates/git/src/issue_ops/call.rs`), the opaque `Render` the leader renders the reply from: its `HostedDir`
(`crates/api/src/hosted.rs`) carries the owner's whole `AuthzFold` as hex plus `logins`, which `RepoCmd::encode`
serialises with the command (`out.extend(serde_json::to_vec(c))`) and no event stores. Most of B0-33's agent-class
commands are side commands (comments, labels, task edits, leases, about 3:1 against pushes), which are IssueCalls; that
they sit within 10.4–11.5 KB fits a constant render. Not measured: the render's share of each command, and an agent
push's priced size by itself [unverified]. The fix would be in `crates/raft` admission (price by the entry's events,
or let the command exclude bytes it does not store) or in what the edge puts in `render`; that is the B0-02 thread's
(raft) call. No fix here.

**Leader change [measured].** 07:01:14Z, Workers Logs `member alarm` from site a's leader: `StepDown { why: LoopLag {
clamped: 2 }, to: Some(SiteId(1)) }`. Site 1 led term 3, then site 2 term 4 from 07:01:21Z; this run fails the
zero-leader-change line. (The first version of this paragraph said term 3 never formed and the group was leaderless
about 17 s; that read a dead object, see round 15c.)

**The large confirms, round 14's question 2 [measured].** `bigTx` holds 3 transactions of 114–204 KB, 125–157
statements: `INSERT request` 27–35 rows (36–45 KB), `INSERT m_path_set` 22–27 rows (32–40 KB), and one `INSERT log`
row of 36–131 KB. So the large confirms are fold applies of many requests (their `request` rows and the metadata
tier's `m_path_set` rows) together with one log row, not `Promoted`, `Archived`, an install, or a learner catch-up.

**Verdict.** B0-33 fails on m26c: commit p99 30,975 ms, 897 of 12,480 agent pushes, two leader changes (round 15c). B0-33 stops here, as the parent ordered, until the admission pricing is settled.

**Cost.** $112.17 at 07:25Z (containers $46.15).

### Round 15b: what the 10.8 KB is, the hold, and the plan for the four rows (2026-11-14)

**Hold.** The parent held B0-29 and B0-35 until the admission pricing fix merges. `rows-m26r` was stopped in setup
(repo 131331 made, no a1 run, no learners). `teardown.py --sweep` at 07:38Z tore down 3 containers (131330 sites 1
and 2, 131331 site 0), all 200, then "sweep: no shard container running".

**Command bytes by kind [measured, local, real codec].** A scratch test (deleted) encoded each B0-33 / B0-29 side
command as `RepoCmd::encoded_len`, what admission prices, with the `Render` the edge builds (`commit_issue_in`,
`workers/edge/src/api.rs`) over the install's real owner fold: `authz:7100001` (m3org) read through
`/_bed/identity` `{"dep": true}`, 4,987 codec bytes (`names` 209 repos 3,778, `members` 94 at 472, `more` 719),
which `HostedDir`'s `authz_hex` sends as 9,974 bytes of hex. Actor: an installation token's claims (8 permissions).

| kind | priced total | `render` | of it `dir.authz` hex | rest of the command |
|---|---|---|---|---|
| comment (GraphQL `addComment`) | 10,973 | 10,426 | 9,974 | 551 |
| label add (REST) | 10,806 | 10,272 | 9,974 | 538 |
| task edit / stand-in (REST) | 10,888 | 10,277 | 9,974 | 615 |
| lease renew | 10,820 | 10,262 | 9,974 | 562 |
| lease acquire | 10,892 | 10,261 | 9,974 | 635 |
| agent push (40 KB inline pack) | 405 less the pack | none | none | 405 |

With 20 logins instead of 2 each IssueCall grows by 370 bytes; with no `authz` each is 834 to 1,001 bytes. So
**92% of every issue command's priced bytes is the owner's `Authz` fold in hex**, `render` is 94 to 95% of it, and the
part that reaches the log is 538 to 635 bytes; an agent push prices at about 405 bytes. This matches m26c's
`admittedBytes` (agent p50 10,842, p99 11,495: IssueCalls; the pushes are the low tail) and its confirms (p50 1,910
bytes an entry of 7.6 commands). The fold grows with the owner: `names` is one entry a repository, so every repository
m3org adds (209 now, mostly test repos) adds about 18 bytes of codec and 36 of hex to every issue command it prices.
Merges carry the same `Render` (`MergeCall.render`, `commit_merge`). Two places a fix could go, for B0-02's call: price
admission by the bytes the entry stores (`decide`'s events), or keep `render` out of `encoded_len` (it never reaches
the log); sending `authz` once per entry, or only the parts a reply reads, is the edge-side alternative.

**The four rows, priced [from measured row deltas].** Gate rows are one run each (BUILD-PLAN B0-29: "one 600 s run").
On m22 at k 4 (standard-4): B0-29 $1.90 (600 s), B0-33 $1.17, B0-35 $2.46; m26c's B0-33 with learners $1.37; m19's
B0-34 $1.98. B0-34's seeds are done and kept (`plan.txt.m3org_b034-mono.done`, the git/git networks' `.done`), so it
needs no seed. Plan on the fixed main: one site day (about $1), smokes at 250 and 500 (about $2), B0-29 on the
500-PR shape (about $3 with the opens), B0-33 (about $1.5), B0-35 (about $2.5), B0-34 last at 900 s on b034-mono
(about $3 to $5 with the warm pool and the kill), plus idle drift (about $0.35 an hour). Total about $13 to $16 over
$112.26 now (07:41Z, containers $46.21), so about $125 to $128 of the $150 cap, with room for one rerun.

### Round 15c: correction, m26c's leader change (2026-11-14)

The m3f rows thread found that `terms.mjs` can sample a dead voter object: `voterNames` takes the voter names from a
leader's committed config, so with no member leading it answers `{}`, and the sample then calls the site by site
alone, which reaches the first-generation object. That is what m26c's site-1 column shows from 07:01:04Z to 07:01:18Z:
term 2, leader 0, its promise lapsed 598 to 612 s (the `#1#f1` fresh-repo placement replaced before the run). The
live members say otherwise:

| at (Z) | site 0 | site 2 |
|---|---|---|
| 07:01:04.7 | follower, term 3, no leader, voted 1, promise 2.8 s | same |
| 07:01:05.7 | follower, term 3, leader 1, promise 9.1 s | same, 9.3 s |
| 07:01:07.7 | term 3, leader 1, promise 9.9 s (renewed) | same, 10.0 s |
| 07:01:18.7 | term 3, leader 1, promise lapsed 0.4 s | lapsed 0.03 s |
| 07:01:21.8 | term 4, leader 2 | leader, term 4 |

So the `LoopLag` transfer worked: site 1 won term 3 and renewed the followers' promises, so it led. Its promises then
lapsed, and site 2 took term 4. That is two leader changes, with no member leading for at most about 1 s at 07:01:04Z
and about 3 s from 07:01:18Z, not one change and about 17 s. Why site 1 lost term 3 is not measured (no `member
alarm` StepDown from site b in Workers Logs for 07:01:00Z to 07:01:30Z, and site 1's live `/status` was never
sampled). The only StepDown logged is site a's at 07:01:14.760Z, after the followers already followed site 1 at term
3 from 07:01:05Z; that timestamp is not explained [unverified]. Round 15's B0-33 verdict (Fail) stands.

The other traces in `tests/fanout/raw/terms-*.jsonl` (m24s33, m24s33b, m24s33c, m25s33) have no sample with a promise
lapsed over 60 s, so this misread is only in m26s33. `terms.mjs` now keeps the last voter names it saw while no
member leads.

### Round 16: m27 on main 2b1938e (admission prices `stored_len`), smokes (2026-11-14)

**Deploy m27.** lm1-memory 3dfc08d8 = main 2b1938e (B0-02's pricing fix, narrow-reads-3's READ_PIPELINE, m4-web-2)
plus this branch's instruments, whose `admittedBytes` now prices `stored_len` as admission does. Image e4257d49
(rebuilt: main's `ui.go` compare counts ship in the shard image); sites a 5b592602, b 775ad4a2, c a22f3454 (each
site deployed twice by `siteday.sh`; these are the second); standard-4; NOROLL; 10:04Z. Full checks (checks30) pass;
`check_names` and `check_links` pass in a clean worktree (2,695 files, 3,249 citations). Long sweep (sweep16/17) exit
0, longest healthy-quorum stall 15,735 / 25,169 / 23,771 ms; Porcupine Ok on all three histories (156,325, 2,230,496
and the third). Plants 92 and 93 caught.

**Smokes**, B0-29's agreed shape (fresh repo, learners enam 4,000 / weur 2,000 / apac 2,000 then k 4, 500 PRs opened
before the window), 180 s each:

| line | s250 (131332) | s500 (131333) |
|---|---|---|
| commit p50 / p99 | 351 / 968 ms | 480 / 1,671 ms |
| queue / queue:slot p99 | 625 / 596 | 1,193 / 1,151 |
| round p99 (near / far append) | 443 (503 / 727) | 746 (687 / 867) |
| confirm p99 (storage, <100 KB / 100 KB–1 MB) | 71 / 262 | 179 / 88 |
| entries / commands | 836 / 1,829 | 658 / 3,027 (4.6 an entry) |
| agent pushes | 385 / 385, p99 5.7 s | 745 / 746 (1 503 on access rows), p99 9.6 s |
| merge:only p99 | 1,358 | 1,285 |
| API reads | 34,373, 100% 200, p99 1.6 s | 34,757: 34,029 200, 66 503, 662 driver cap; p99 4.0 s |
| leader | term 2 held, 108 req/s, 0 `/suspect` | term 2 held, 123 req/s, 0 `/suspect` |
| admission capacity / retryAfterS | 218–1,078 /s, 1–2 s | 151–1,066 /s, 1–2 s |
| cmdBytes (moving mean) / agentQueued max | 740–2,431 / 4 | 716–2,015 / 8 |
| admittedBytes agent p50 / p99 | 509 / 906 | 509 / 905 |
| leader wasm | 28 → 52 MiB | 28 → 67 MiB |

The pricing fix holds: agent-class commands price at p50 509 bytes (m26c: 10,842), capacity sits at hundreds a second
(m26c: 30–47), and the agent queue never passes 8. At 500 the commit p99 (1,671 ms) is past B0-29's fail line (1 s);
it is the queue's slot wait (1,151) behind a round of 746 ms, whose far leg reads 867 ms, not admission (Retry-After
1–2 s, queue under 10) and not the confirm (179). That is round 13's finding again on this shape: the line is the
round's follower legs. B0-29 runs as the gate row (600 s) next, as the operator ordered.

Cost $114.62 at 10:41Z (containers $46.80); the smokes' sweep "sweep ok" (6 torn down).

### Round 16b: B0-29 gate row on m27 (repo 131334, run mwga2kd3, 2026-11-14)

**Run.** `rows.sh m27r`, `ROWS="b0-29"`: fresh repo `fm27r-b029`, learners enam 4,000 / weur 2,000 / apac 2,000
(8 of 8 caught up at commit 874 in 2.5 s), k 4 granted, 500 PRs opened before the window, 600 s from 10:50:43Z,
standard-4, no redeploy mid-row. Raw: `tests/am1/raw/a1-b0-29-mwga2kd3.jsonl`, `tests/fanout/raw/terms-m27r29.jsonl`,
`tests/fanout/raw/m27r29-legs.tar.xz` (`tests/fanout/legs.mjs`: every voter's and learner's raw `/stats` once a minute, the
leader's every third minute).

| B0-29 line (`build/BUILD-PLAN.md`) | m27 | verdict |
|---|---|---|
| Commit p99 at most 600 ms (fail over 1 s) | p50 360, **p99 774 ms** (n 10,825) | **miss** (between pass and fail) |
| Merge p99 at most 2 s (fail: a queue merge over 10 s) | merge:only p99 1,608 ms (72 × 200, 8 × 409 base moved) | pass |
| Zero `Stale` on distinct refs | 2,500 / 2,500 agent pushes ok (distinct refs), 0 `stale`; humans (20 shared refs) 38 `stale:race`, 1 `stale` | pass |
| Leader under 600 req/s with zero feed polls | 120.8 req/s, 0 feed polls on the leader | pass |
| API read p99 at most 2 s (fail over 5 s) | **p99 1,922 ms** (p50 471) | pass |
| API reads at least 99% served | 115,772 of 115,834 = **99.95%** (62 × 503) | pass |
| Leader changes | 0: term 2 on site 0 throughout; 0 `/suspect` | — |

So B0-29 does not pass on m27: one line, commit p99, is 174 ms over; it does not fail (under 1 s). Every other line
passes, including the API line on fixed main (round 16's s500 4.0 s was 180 s with no learner warm-up of reads;
here 1.9 s over 600 s).

**Commit split.** queue p99 484 (slot 462, post 208), round 336, confirm 58, apply 336. Near append (site 1) p99
361, far (site 2) 457. 2,895 entries carried 10,005 commands, **3.46 per entry**. Entry bytes (the leader's
confirm transactions): under 100 KB n 2,476, p50 5.8 KB, p99 96 KB; 100 KB to 1 MB n 419 (14%), p50 132 KB, p99 198
KB; storage p99 60 and 52 ms, `gate` and `turn` 0. So the commit p99 is the slot wait (a command waits for the entry
ahead, about one round) plus its own round, with the confirm small.

**Admission over 600 s [measured, 60 samples].** Capacity 255 to 1,082/s (start 1,082, 377 at 596 s, as `cmdBytes`
rose from 770 to 1,751 with human packs counting), `retryAfterS` 1 to 2 throughout, `agentQueued` at most 6, no
`pressure`. `admittedBytes`: agent n 9,227, p50 509, p99 905 (no command over 10 KB); human n 1,598, p50 301, p99
20,466 (packs). So admission does not drift into a bind over 600 s: narrow-reads-3's gate-12 queue p99 4,798 ms on
82a0bf9 is gone on this shape (here 484). Leader wasm grew 28 → 92 MiB.

**The near follower's AppendEntries leg, split [measured, `tests/fanout/legs.py`].** 1,601 matched calls to site 1
and 1,774 to site 2 from the leader trace (AppendQueued, AppendIssued, AppendEnded) against each follower's `raftIn`
rows (handler start, body ms, work ms) and trace (Stepped, Confirm):

| leg (p50 / p99 ms) | near, site 1 | far, site 2 |
|---|---|---|
| whole call (issue → end, leader clock) | 166 / 406 | 223 / 483 |
| in: issue → follower handler start (wire out + input queue; crosses clocks) | 58 / 293 | 87 / 339 |
| body read | 11 / 112 | 19 / 199 |
| step (decode, step, persist) | 0 / 0 | 0 / 0 |
| output gate (work less step) | 0 / 0 | 0 / 0 |
| follower storage confirm (`sync()`) | 0 / 0 | 0 / 0 |
| back: reply wire (crosses clocks) | 95 / 282 | 114 / 326 |
| in + back (skew cancels) | 151 / 374 | 200 / 457 |
| bytes | 28.7 KB / 176 KB | 28.3 KB / 176 KB |
| slowest 1% (mean): call, in, body, back | 489, 228, 23, 237 | 621, 226, 131, 264 |

So the near leg is the wire, not the follower. The follower's step, output gate and storage confirm read 0 ms at p99
(Workers' clock advances only across I/O, so a synchronous step reads 0: it is not long enough to delay the next I/O
[inference from the clock rule]); body read is 112 ms at p99; the two wire legs are 374 ms together at p99. Which
side of `in` is the follower's input queue is not separable without send timestamps on the follower's clock
(narrow-reads-3's `x-graft-sent` / `x-graft-left`, trace kinds 20 and 21, branch 54a700a9) [unverified split]. Round
16's s500 near p99 687 ms was 180 s with 4.6 commands an entry; here 361 ms (the whole-row `append:1`) over 600 s.

**Reads by member [measured, `inbound /read` deltas over the window].** 102,440 member reads: learners 95.5%, voters
4.5% (leader site 0 4.4%, followers 0.9% each).

| member | site | reads | share |
|---|---|---|---|
| l7 | 0 (enam) | 42,309 | 41.3% |
| l3 | 0 | 14,126 | 13.8% |
| l0 | 0 | 12,720 | 12.4% |
| l6 | 0 | 11,083 | 10.8% |
| l5 | 2 (apac) | 7,346 | 7.2% |
| f0 (leader) | 0 | 4,464 | 4.4% |
| l4 | 1 (weur) | 3,485 | 3.4% |
| l2 | 2 | 2,681 | 2.6% |
| l1 | 1 | 2,410 | 2.4% |
| f1 / f2 | 1 / 2 | 918 / 898 | 0.9% each |

The four enam learners serve 78% (one of them 41%), so the spread is uneven inside a site; the API p99 held anyway.

**Cost** $116.92 at 11:14Z (containers $47.04); the row cost about $2.29. Sweep "sweep ok" (5 torn down).

### Round 16c: B0-29's leader load, an idle baseline, and leader memory (from round 16b's data, 2026-11-14)

**Where the sites run [measured, `durableObjectsInvocationsAdaptiveGroups` `coloCode`].** repo:131334's voters:
site a (enam) **ATL**, site b (weur) **LHR**, site c (apac) **NRT**. So the near follower is London from Atlanta, a
transatlantic pair, not one region. The members' own `/rtt` rows (median of 20 sequential `/ping`s, taken at join):
a↔b 90 / 100 ms, a↔c 164 / 159 ms, b↔c 221 / 222 ms (131335's: 92 / 96, 152 / 158, 222 / 230).

**Idle baseline [measured, the leader's trace after the window].** AppendEntries the leader sent from 11:01:39 to
11:09:45Z, with no load, of 143 to 1,412 bytes: to site 1 (LHR) 102 to 122 ms (12 calls), to site 2 (NRT) 159 to
185 ms; one of 256 KB took 378 and 354 ms. Under load (the window, 2,809 calls to site 1), by entry bytes:

| bytes | n | near (LHR) p50 / p99 | far (NRT) p50 / p99 |
|---|---|---|---|
| under 4 KB | 1,268 | 144 / 332 | 202 / 396 |
| 4 to 16 KB | 60 | 160 / 316 | 203 / 428 |
| 16 to 64 KB | 697 | 156 / 339 | 214 / 385 |
| 64 to 128 KB | 560 | 178 / 412 | 239 / 553 |
| 128 KB and up | 224 | 201 / 406 | 262 / 496 |

So the near leg is about one ATL↔LHR round trip (idle 102 to 122 ms), plus about 30 to 40 ms at p50 under load
whatever the size, plus about 60 ms more for entries over 128 KB; the p99 sits about 200 ms over idle even for
entries under 4 KB. Round 16b's `in` p50 58 ms is about half the idle round trip (wire), and `back` p50 95 ms is
about 45 ms over the other half: the excess is on the reply side. Whether that is the leader's input queue (its
continuation waiting to run) or the reply's delivery is not separable without send stamps on the follower's clock
[the parent's guess; unverified]. The leader's `AppendQueued → AppendIssued` is 0 ms at p99 (2,809 calls), so the
leader does not delay the send.

**The leader isolate's load [measured].** `durableObjectsPeriodicGroups` for the leader object (aa7bf08b), per
minute in the window: CPU 11.4 to 13.0 s a minute = **19 to 22% of one core** (108 s over 10:50 to 11:01Z), active
60 s of 60, about 700k to 880k rows read and 16k written a minute. Its inbound (its own `inbound` counts, 10:52:44 to
11:01:44Z, 540 s): `/raft` 43,187 (80/s: learners' calls and their ReadIndex), `/commit` 8,926 (16.5/s), `/read`
6,131 (11.4/s), `/entries` 2,306, `/status` 1,363; about 115 requests a second, so **about 1.7 ms of CPU a request**
on average. By route (Workers Logs, which logged 17,579 of those invocations, 86 s of CPU, so a sample): `/raft`
4.3 ms each and 48% of logged CPU, `/read` **12.3 ms each and 26%**, `/commit` 3.8 ms and 15%, `/entries` 3.0 ms
and 7%. Scaled by the inbound counts, reads are about a quarter of the leader's CPU (more with the ReadIndex answers
inside `/raft`, which the logs do not split), commits about an eighth. The followers ran 10 to 11% of a core (site
b) and 8 to 10% (site c). So the leader is not CPU-saturated on average; a 20% busy isolate still queues inputs that
arrive in a burst, which is consistent with, but does not prove, a reply-side wait. **LOOP_LAG:** no `StepDown`
appears in Workers Logs for 10:50 to 11:02Z on any site, and the leader held term 2; the ticker's `Lag` trace (kind
8) was not in `legs.mjs`'s filter for this row, so per-tick lag was not sampled here [not measured]; it is in B0-35's
sampler.

**Leader memory [partly measured].** a1.mjs's admission samples read the leader's wasm memory rising 28 → 41 (60 s)
→ 67 (180 s) → 80 (300 s) → 88 (420 s) → 92 MiB (485 s, then flat to 596 s). Wasm linear memory only grows, so
this is a high-water mark, not what is live. `memSplit` was not sampled during the row; read on the same object at
11:25Z (a new isolate, `since` 11:25:28Z, wasm 16.8 MiB): log 6.1 MB (`LOG_BYTES` caps it at 8 MB), fold rows 0,
caches 0, held 0, in flight 0. So the named buckets account for at most about 8 MB of the 92 MiB; what reached the
high-water mark is not named by them [unverified: the candidates are transient read answers (12.3 ms of CPU and
about 12k rows read a second suggest large ones) and allocator growth that is never returned]. B0-35's sampler now
records `memSplit`, `logBytes` and wasm every minute, to tell a steady bucket from a transient peak.

**A memory event at the row's end [measured].** Site a's stateless script (not a member) logged "Worker exceeded
memory limit." 28 times at 11:00:53Z, all on `POST https://site/prsync/first`: the in-band first attempt of a
`PrSync` batch, which the leader runs as a new stateless invocation through its own `SITE` binding
(`workers/site/src/prsync.rs`, `in_band`). 28 at one timestamp is one isolate failing the invocations it held, ten
seconds after the window ended. No member logged one, and no row line moved. Its cause is not traced [unverified];
B0-35 (500 polled PRs, test merges each push) exercises that path harder, so its row is checked for the same event.

### Round 16d: B0-35 gate row on m27, and the `/prsync/first` memory failures (2026-11-14)

**Run.** `rows.sh m27r35`, `ROWS="b0-35"`: fresh repo `fm27r35-b035` (131335), learners enam 4,000 / weur 2,000 /
apac 2,000, k 4, 500 open PRs and 500 PR pollers every 5 s, 600 s from 11:23:33Z, standard-4, no redeploy. Raw:
`tests/am1/raw/a1-b0-35-mwgb90vc.jsonl`, `tests/fanout/raw/terms-m27r35.jsonl`, `tests/fanout/raw/m27r35b-legs.tar.xz`,
`tests/fanout/raw/prsync-131335.txt` (Workers Logs `prsync 131335` lines).

| B0-35 line (`build/BUILD-PLAN.md`) | m27 | verdict |
|---|---|---|
| Test-merge vCPU within `TEST_MERGE_BUDGET` (fail: over budget) | 0.603 vCPU used of a 1.0 vCPU budget (`TEST_MERGE_BUDGET` 0.25 of a standard-4 shard's 4 vCPU), no overrun, 0 shed | pass |
| Zero computes from list fields | 0 list computes of 102,592 list asks; 5,339 computes from 55,795 single-PR GETs | pass |
| Actions dispatch p99 at most 10 s (fail over 30 s) | not measured: the driver starts no workflow runs, and `prsync::demand` has no caller before Milestone 4 | open: waits for Milestone 4 Actions' dispatch |

So B0-35 is open, not passed: its two measured lines pass, and its dispatch line stays unmeasured, as in rounds 8
and 9, until Milestone 4 Actions' dispatch exists (parent decision, 2026-11-14). The test merges'
own wait is what clients see: of 5,378 merges, the wait histogram's mass sits in its top buckets (3,143 over 120 s,
1,411 at 60 to 120 s), max 265 s; `mergeable` answered `UNKNOWN` to 58,189 of 59,216 polls (98.3%) and `MERGEABLE`
to 949. Commit p50 / p99 369 / 685 ms (queue p99 449, round 305, confirm 70), 3,137 entries carried 15,393 commands
(4.91 an entry), term 2 throughout, 142.6 req/s, 0 feed polls on the leader, 82 `/suspect`. API reads 116,703 of
117,250 200 (99.53% of all; 509 driver cap, 38 503), p99 1,872 ms. Agent pushes 2,495 / 2,500 ok (3 client fetch
failures, 1 shard "Durable Object is overloaded"), merge:only p99 4,321 ms (3 shard-overloaded 503s). Admission:
capacity 296 to 1,189/s, `retryAfterS` at most 2, `agentQueued` at most 7. Leader wasm 28 → 78 MiB.

**Near AppendEntries split (same method as 16b).** Site 1 (LHR) n 1,465: call p50 / p99 174 / 323, in 81 / 214,
body 12 / 97, step / gate / confirm 0, back 79 / 267, in + back 160 / 309. Far (NRT) n 1,623: 237 / 547. Same shape
as B0-29: the follower adds nothing; the legs are wire plus a reply-side excess.

**Leader memory, sampled each minute [measured, `memSplit`].** wasm 65.7 MiB at 11:27Z and 78.0 MiB from 11:30Z
(flat after); log 7.9 to 8.1 MB (at `LOG_BYTES`); dump cache 1.42 → 1.73 MB; narrow cache 95 KB; held 0 to 29 KB;
in-flight call bodies 0 to 264 KB, answers 0 to 29 KB. So the named buckets hold about 10 MB at most, steady, while
the wasm high-water mark sits 68 MB above them: the peak is transient, not a bucket that grows [inference: wasm
memory only grows, and every bucket sampled flat]. Loop lag: 0 `Lag` trace entries (ticks 20 ms or more late) in
any sample of the leader's trace ring, so its 100 ms ticker was never 20 ms late in the sampled spans.

**Reads by member.** Learners 94.3% (enam l7 25.1%, l3 21.3%, l0 20.7%, l6 12.2%; apac l2 6.8%, l5 2.7%; weur l4
3.3%, l1 2.1%), leader 3.8%, followers 1.9% and 0.0%. The enam learners serve 79%, more evenly than B0-29 (top one
25% against 41%).

**The `POST /prsync/first` memory failures (the parent's item 3) [measured, cause traced].** In B0-29 (131334), the
leader's `PrSync` first attempts ran in band 155 times, 0.6 to 5.1 s each, until 10:51:06Z. From 10:51:31Z every
first attempt timed out at the leader's `FIRST_FOR` (25 s): 19 in a row, each over a larger batch (894..914, then
894..1036, 914..1148, up to 1272..3683: 20 to 2,411 entries, the PRs every entry since the cursor touched), and the
chain parked 3 times (`PARK_AFTER` 10). The queue's re-sends of the same batches were handled, slowly: 894..1035 in
172.5 s (136 merges asked, 52,580 list asks on the shard's budget), 914..1147 in 333 s (225 merges asked).

The 14 `exceededMemory` invocations (the 28 log lines are each invocation's message and its error) are those
abandoned first attempts. Workers Logs give each one's wall time: 486, 460, 435, 390, 364, 339, 314, 289, 242, 216,
191, 166, 141 and 36 s. Subtracted from 11:00:53.567Z, they started at 10:52:47, 10:53:12, 10:53:37, 10:54:23 … 10:58:32
and 11:00:17Z, the leader's first-attempt sends 25 s apart, matching its "no answer in 25000 ms" lines. So: `in_band`
(`workers/site/src/prsync.rs`) sends each batch to the site script's own `/prsync/first`, and `first_handled` runs
`one()` under `ctx.wait_until`, which the leader's `within(first_for, …)` does not cancel (by design: "A first
attempt past `first_for` is not cancelled"). After `FIRST_FOR` the leader's `Dispatcher` (`crates/git/src/prsync.rs`,
`finished(false)` → `unsent()`) is due again, and `batch()` hands out a new First over the cursor's whole range, so a
new attempt starts every 25 s while the old ones still run. Those stateless invocations landed in one isolate, which
held 14 concurrent `one()` calls, each with its `SyncRead` of every PR the batch names (`views`, up to every open
PR: 500) and the merge answers it collected (`MergeAnswer::Clean` carries the merge's pack, base64, up to
`MERGE_BYTES_PER_COMMAND` a command), until the isolate passed 128 MB and all 14 failed together at 11:00:53Z. 4 more
were `canceled` after 30, 174, 299 and 476 s. So it is not one PR or one large pack: it is overlapping attempts over
the same growing batch, with no bound on how many run at once [the per-attempt bytes are not measured; the
mechanism is measured from start times, wall times and the leader's log].

Why the first attempts stopped finishing at 10:51Z: each one runs its merges one after another on the network's
shard (`merge()` in a loop over `views`), and the shard's budget queue was by then long (`inFlightMax` 17 on that
row's shard, and B0-35's 3,593 with waits to 265 s), so one attempt of 100 or more merges cannot finish in 25 s.
In B0-35 the same pattern ran: 10 first attempts timed out from 11:24:18Z and one answered 500 "Durable Object is
overloaded" (the shard), the chain parked twice, the queue handled 834..1069 after 606 s queued; there were 9
`canceled` and no `exceededMemory`, so the overlap stayed under the limit there.

This is a product bug, not a test artefact. Candidate fixes, for the owner of `prsync.rs` to choose: count a timed-out
first attempt as still running (so `batch()` does not hand out another First while it runs), or cancel it at
`FIRST_FOR`; and bound a batch's merges (or its PR count) so one attempt fits `FIRST_FOR`. Not fixed here.

**Cost** $119.59 at 11:44Z (containers $47.29). Sweep "sweep ok".

### Round 17: B0-33 on m28 (main 4438539), two runs; the voters reset under the 5,000-agent mix (2026-11-14)

**Deploy m28.** lm1-memory 4f9380ad = main 4438539 (decode-once: no second decode of a `/raft` body; leader-clock-check)
plus round 17's instruments. Sites a 36077144, b 3ff70714, c d2743d76, edge 6c620ce7, image `sha256:79bec25d`
(built from d8c09451; `containers/` unchanged since), standard-4, test-faults. Checks: full checks33 pass (check_names
and check_links only on `.worktrees/`, clean in a fresh worktree); the long sweep and Porcupine pass on a858bfb, and
4438539 adds no `crates/raft` change. Plants 94 and 95 (`readsBy`'s kind) caught.

**Instruments.** `tests/fanout/memguard.mjs` reads every voter's `/stats?summary=1` (`wasmMemMb`, `memSplit`) every 2 s
and SIGTERMs `a1.mjs` once any voter passes 120 MiB; `a1.mjs` then ends its window and writes the row as `stopped`.
`tests/nr3/tsave.mjs` saves each voter's trace each minute, so a reset loses none of it. `/stats` `readsBy` counts each
member's `/read`s by kind, plain or read-index, and whether `Family::follower_serves` lets a follower answer it.
`tests/fanout/legs.py` splits the reply leg with main's send stamps (trace kinds 20, 21).

**Run 1, m28r33** (repo 131336, run `mwghmvck`, 14:20:24Z). Not a gate run: I stopped it at 79 s by hand after the
first leader change, which is not the stop rule (memguard never fired). Learners caught up 0 of 8 in 120 s before the
window. Leader site 0 peaked at 70.2 MiB, then its object lost its wasm instance at 14:21:02Z ("Cannot invoke closure
from previous WASM instance" ×8, "Promise will never complete." ×738, then `exception` on 589 `/commit`, 80 `/outcome`,
47 `/raft`), with no `exceeded its memory limit` event on any site; terms 2 → 5 in 36 s.

**Run 2, m28r33b** (repo 131337, run `mwgikx95`, 300 s from 14:45:08.7Z, ran in full; memguard never fired). Learners
caught up 8 of 8 in 15 s.

| B0-33 line (`build/BUILD-PLAN.md`) | m28r33b | verdict |
|---|---|---|
| Human and merge-queue commit p99 ≤ 1 s | leader commit p99 2,674 ms (n 274, p50 674); queue p99 40.5 s; `merge:only` p99 15.4 s (3 of 7 ok); `push:human` 8 ok of 186 | fail |
| ≥ 140 agent commands/s admitted (fail < 100) | 269 commands in 63 entries at the end leader's `/stats` (term 12, so only its own term); 1,645 agent pushes ok in 300 s = 5.5/s at the client | fail |
| Zero leader changes | term 2 → 12; leader site 0 183 s, site 1 25 s, site 2 9 s, none 82 s of 300 | fail |
| Zero `suspect` reports | 2,774 on site 0, 4,928 on site 1, 605 on site 2 | fail |
| Every refusal `ng` or 429 `retry-after` | 503s from the read gate ("every member asked shed the read: overloaded", "not leader", "no answer") and the git cache (`{"warming":true}`) | fail |

Admission never bound: capacity 405–1,859/s, `cmdBytes` 489–988, `retryAfterS` 1–2, agent queue at most 54; admitted
agent commands priced p50 498 B, max 898 B (the pricing fix holds). Reads: learners served 96.1% of the window's
`/read`s (76,144 of 79,224).

**Voter resets.** Seven resets in the window, every one below the 120 MiB stop line:

| reset | site | role before | wasm MiB at the last sample | Workers Logs |
|---|---|---|---|---|
| 14:47:35–47 | 0 (a) | leader | 95.9 | "previous WASM instance" ×1,087, "Promise will never complete" ×309; one `storage write failed: … isolate exceeded its memory limit` at 14:48:03 |
| 14:48:01–21 | 1 (b) | leader | 40.6 | "Durable Object's isolate exceeded its memory limit and was reset" ×805 at 14:48:30 (`POST /raft`) |
| 14:48:55–49:11 | 2 (c) | follower | 31.1 | "previous WASM instance" ×504 at 14:49:19–21 |
| 14:49:22–26 | 1 (b) | leader | 38.5 | "previous WASM instance" ×50, "exceeded its memory limit" ×597 at 14:49:44 |
| 14:49:49–51 | 0 (a) | leader | 74.4 | "exceeded its memory limit" ×274 at 14:50:01 |
| 14:49:46–51 | 1 (b) | leader | 47.3 | "exceeded" ×556 at 14:49:57 |
| 14:50:04–28 | 2 (c) | follower | 31.2 | — |

So the isolates are reset for memory while the wasm heap reads 31 to 96 MiB. `wasmMemMb` is the wasm linear memory
only; the isolate's limit counts the JS heap as well, where request and response bodies, `fetch` streams and the
promises of held requests live. What memSplit counted in the minute before each reset (bytes in Rust's own buffers):

- the in-memory log at 6.4–8.1 MB on every voter, flat once it reached its window (14:46:05 onward);
- the dump cache at 1.1–1.87 MB;
- in-flight read answers peaking at 9.2 MB (site 0 before 14:47:35), then 26–30.5 MB (sites 0 and 1 before 14:49:26–51);
- in-flight member-call bodies peaking at 5.0 MB (site 1), then 58.3 MB (site 0 before 14:49:49) and 16.9 MB (site 1);
- bytes held for learners peaking at 1.0 MB, then 42.9 MB (site 0) and 11.5 MB (site 1);
- queued statements at most 216 KB.

Site 0's first reset (14:47:35) came with only about 10 MB of these buffers and the heap flat at 95.9 MiB since
14:46:45, so the counted buffers do not explain it; the later resets came with 30 to 100 MB of answers, call bodies
and held learner messages, all of which reach the JS side as `Response` bodies too. [inference: the JS-side copies of
those bodies, which `wasmMemMb` does not see, take the isolate past its limit; not measured, since no instrument reads
the JS heap.] The growth after the first leader change is the catch-up: each new leader resends the log tail to two
voters and eight learners at once.

**Reply-leg split** (main's send stamps, matched exactly: the follower's `CallArrived` carries the leader's issue time):

| | site 1 (LHR) p50/p99 ms | site 2 (NRT) p50/p99 ms |
|---|---|---|
| idle (before the window), round trip on the wire | 93 / 119 (n 113) | 263 / 297 (n 113) |
| idle, follower's send (handler return to reply out) | 0 / 0 | 0 / 2 |
| idle, leader's input queue (reply arrival to stepped) | 0 / 1 | 0 / 3 |
| load (window until site 0's reset), leg | 207 / 526 (n 364) | 374 / 1,359 (n 341) |
| load, wire round trip (offset cancels) | 199 / 514 | 347 / 1,044 |
| load, follower's send | 0 / 121 | 0 / 427 |
| load, leader's input queue | 0 / 31 | 0 / 362 |
| load, half the asymmetry ("skew") | 678 / 1,445 | 669 / 1,445 |

The leader's input queue is 0 at p50 and at most 31 / 362 ms at p99, so the loaded excess is not the leader queueing
replies behind `/read`. The wire round trip itself doubles and more, and the asymmetry says the delay sits in the
outbound half: the follower stamps an arrival only when its own isolate runs the handler, so its input queue is in
that half. The leader's ticker logged no Lag sample (no tick 20 ms late) in the window. [inference: the excess is the
follower isolates' input queues under the 5,000-agent mix of `/suspect` (4,928 on site 1), `/entries` (3,162) and
`/read`, not the network.] Run 1's 23 and 17 matched calls agree (idle 124/159 and 250/321; under load the skew rose
to 2.7 s).

**Leader reads that could go to learners** (`readsBy`, run 2's leaders at the end): site 0 5,286 `/read`s, 4,468 of
them (84.5%) follower-servable, every one sent plain, none by read-index (`rows` dumps 2,965, `rows.access` 1,364,
`rows.agent` 139); the rest are leader-only (`probe` 486, `ranges` 321, `prsync` 11). Site 1, leader for 25 s, 8,409
`/read`s, 95.6% servable. Run 1's site 0: 11,807, 91.6% servable. The edge's read gate sends these to a voter as plain
reads; learners already carry 96% of read-index reads.

**Verdict.** B0-33 fails on m28 on every line. What binds is not admission (pricing fixed) but the voters' isolates
resetting for memory under the mix, below 96 MiB of wasm heap, which then churns the leader (term 2 → 12) and turns
commit p99 into seconds. This is the m3f thread's storm (8,465 voter resets on L0-12) reproduced on B0-33 with no
double decode.

**Raw.** `tests/am1/raw/a1-b0-33-mwghmvck.jsonl`, `a1-b0-33-mwgikx95.jsonl`; `tests/fanout/raw/memguard-m28r33{,b}.jsonl`,
`terms-m28r33{,b}.jsonl`, `tsave-m28r33{,b}.jsonl.xz`, `m28r33{,b}-legs.tar.xz`.

**Cost** $122.56 at 14:54Z (containers $47.72). Sweep "sweep ok" after both runs. B0-34 started at 14:55Z on the same
deploy with memguard and terms on `repo:131296`.

### Round 17b: B0-34 not run on m28; the 5 GB monorepo's git cache did not warm in 15 minutes (2026-11-14)

`tests/fanout/b034.sh m28` on the m28 deploy (round 17), b034-mono = repo:131296 (5.02 GB of torvalds/linux, round 10's
seed, kept). The seeds were already done (`seed … exit 0` for all five networks). `learners.mjs` and `k.mjs` hit "no
leader" at 14:59Z (the group was idle; site 0 led term 2 from 14:59:57Z), and `a1.mjs b0-34` then exited 1 at
15:01:55Z: "the repository has no main" after its 120 s wait for `refs/heads/main` in ls-refs. The group had `main`
(the leader's `/read {"refs"}` at index 292 listed `refs/heads/main` and `refs/heads/b034-parts`); what failed was the
edge's ls-refs, which the git cache answers.

The cache was cold: every row since round 16 tears its shards down, so b034-mono's started fresh. The site's warm
calls logged "shard warm 131296 site 0/1/2: no answer in 25 s" (5 of 8 warms) at 14:59–15:02Z. Then
`tests/fanout/warmmono.mjs` polled ls-refs every 75 s from 15:12:24Z for 15 minutes: 12 of 12 answers were
`503 git cache: 503 {"warming":true}`, each after 64.3–66.9 s (`tests/fanout/raw/warmmono-b034m28.jsonl`). So a shard
rehydrating the 5 GB network did not finish in at least 28 minutes (14:59:42Z to 15:27:18Z). The row as written needs
the monorepo's shards warm before its window: a warm step in `b034.sh` that waits on `/catchup` until it answers, with
its own time limit, or shards that are not torn down between this row's tries. Not done here: the budget is held back
for teardown, and B0-34 is git CPU, so it is better run on the next deploy with main a4ddca4f.

The three shard containers the warm started were torn down (`teardown.py --sweep`: torn_down 3, then "no shard
container running"). **Cost** $123.02 at 15:29Z (containers $48.07).

**B0-33 holds** until the voter memory fix is on main, then reruns as written (round 17's run 1 does not count).
**JS heap.** No instrument added: the site exposes only `wasmMemMb` (the wasm linear memory), and the Workers runtime's
`process` documents no `memoryUsage()`; Cloudflare documents only DevTools snapshots and production heap profiles.
m3f's run covers the JS side.

### Round 18: B0-35 on m29 (main a4ddca4f's prsync-first-once) (2026-11-14)

**Deploy m29.** lm1-memory 3e707c2d = main 04f8b3a6 (a4ddca4f's prsync-first-once: every `PrSync` attempt ends at
FIRST_FOR, 25 s, with a 10 s commit reserve; a test merge whose turn comes past TURN_WAIT_MS, 10 s, is shed and counted
in `late`) plus this branch's instruments and B0-34's warm step. Sites a fcfe84d5, b eae6468a, c aec148bf, edge
f32e1fab; image `sha256:ca9da2c7` (new: main changed `containers/`); standard-4, test-faults. Full checks34 pass
(check_names and check_links only on `.worktrees/`, clean in a fresh worktree); no `crates/raft` change since the last
sweep.

**Run** `rows.sh m29r35`, repo `fm29r35-b035` (131338), run `mwgm9uwu`, 600 s from 16:33:41Z, learners 8/8 caught
up in 2.7 s, k 4, 500 open PRs and 500 pollers. Memguard never fired: voter peaks 64.3 / 37.5 / 25.6 MiB.

| B0-35 line (`build/BUILD-PLAN.md`) | m29 | verdict |
|---|---|---|
| Test-merge vCPU within `TEST_MERGE_BUDGET` | 0.373 of 1.0 vCPU, no overrun | pass |
| Zero computes from list fields | 0 of 101,146 list asks; 5,045 computes from 54,628 single-PR GETs | pass |
| Actions dispatch p99 ≤ 10 s | not measured (no dispatch caller before Milestone 4) | open |

**The parent's three checks.**

- **"first handled" under 27 s: yes.** 44 `prsync 131338 … first handled` lines (all on site a, the leader's site),
  p50 15,488 ms, p99 and max 16,524 ms, 0 over 27 s. All 41 `/prsync/first` invocations in the window ended `ok`.
- **No `/prsync/first` exceededMemory: none.** No `exceeded` message on any site in the window (round 16d had 14
  invocations that exceeded memory).
- **`testMerges.late` and `mergeable`:** 3,440 merges shed `late` (all of `shed`), against 5,166 computed. The wait
  histogram tops out at 15,902 ms (round 16d: max 265 s, 3,143 over 120 s): 4,338 merges in the 10–20 s bucket, 671
  below it. `mergeable` answered `UNKNOWN` to 55,096 of 58,303 answered polls (94.5%) and `MERGEABLE` to 3,207 (5.5%);
  round 16d: 98.3% `UNKNOWN`, 949 `MERGEABLE`. So shedding late merges moved `MERGEABLE` answers up 3.4 times, while
  UNKNOWN stays the common answer: the line still has more merges than the 1.0 vCPU computes in time.

**Other numbers.** Commit p50 / p99 446 / 1,195 ms (queue p99 815, round 532, confirm 124); 2,495 entries carried 14,544
commands (5.8 an entry); term 2 throughout, site 0 leader, 139.3 req/s on it. Agent pushes 2,478 / 2,500 ok. API reads
111,614 of 113,511 200 (98.3%; 1,184 driver cap, 713 503), p99 3,744 ms. `merge:only` p99 2,116 ms (51 of 58 ok).

**Raw.** `tests/am1/raw/a1-b0-35-mwgm9uwu.jsonl`, `tests/fanout/raw/memguard-m29r35.jsonl`, `terms-m29r35.jsonl`,
`rows-m29r35.status`. **Cost** $125.69 at 16:49Z (containers $48.25). Sweep "sweep ok".

### Round 18b: B0-34 on m29, with the warm step; the cold 5 GB rehydrate fails DESIGN's 120 s line (2026-11-14)

**Warm step** (`tests/fanout/b034warm.mjs`, inside `b034.sh` before learners and k; not the row's time). repo:131296
(m3org/b034-mono), from 16:50:17Z, 45-minute limit. Raw `tests/fanout/raw/b034warm-m29.jsonl`.

- ls-refs through the edge listed `main` at **1,936 s (32.3 min)**; 24 tries before it answered 503 `{"warming":true}`.
- `/_bed/catchup?shard=S&wait=60`: every call timed out at the 120 s client limit (15 per site) until site 2's
  answered 200 at 1,936 s and site 0's at 2,003 s; site 1's never did within the warm step's 5-minute tail.
- Round 17b on m28 agrees: still warming 28 minutes after its start.

**The shards' own rehydrate reports** (Workers Logs `shard rehydrate 131296`, raw
`tests/fanout/raw/rehydrate-131296-m29.json`), for the fix's sizing (the m3f thread owns it):

| shard | done at | catch-up total | stored packs | bytes | spool (bucket read) | `index-pack` | packs by bucket |
|---|---|---|---|---|---|---|---|
| site a (`git#0`) | 17:23:48Z | 1,994 s | 257 | 2,623,972,857 | 57 s (0.22 s/pack) | 1,872 s (7.3 s/pack, 1.40 MB/s) | site 0: 256, site 1: 1 |
| site c (`git#2`) | 17:22:46Z | 1,932 s | 257 | 2,623,972,857 | 94 s (0.37 s/pack) | 1,694 s (6.6 s/pack, 1.55 MB/s) | site 2: 216, site 0: 41 |
| site b (`git#1`) | 17:34:33Z | 2,639 s | 257 | 2,623,972,857 | 53 s (0.21 s/pack) | 2,496 s (9.7 s/pack, 1.05 MB/s) | site 1: 230, site 0: 27 |

Each report: 288 entries, 257 markers, 4 inline packs (2.46 MB, held at most 2.46 MB at once); the phases before the
stored packs (`up` 32–34 s, markers, the marked phase 36–43 s in all) are small. A pack averages 10.2 MB.
`index-pack` is 90–96% of the stored-pack time; the bucket read is 2–5%.

**DESIGN check.** `design/DESIGN.md`, Milestone 0's table: "Container rehydration of a 5 GB network from R2 | At most
120 s | Over 600 s". Measured: 1,932 to 2,639 s per shard for this network (2.62 GB of stored packs, 5.02 GB pushed),
3.2 to 4.4 times the fail line. **Fail**, a finding apart from B0-34's row. [inference: a bounded parallel fetch saves
at most the spool, 53–94 s per shard; reaching 120 s needs `index-pack` in parallel or avoided, for example ingesting a
stored pack with a prebuilt index.]

**Row** (run `mwgoigqa`, 900 s from 17:31:15Z, learners 8/8 in 13.6 s, home shard killed at 300 s, the second network
at 420 s, the hostile owner's three at 600 s). Site 0 led at term 2 throughout; voter peaks 31.1 / 23.1 / 23.1 MiB (memguard never
fired); no `exceeded` message on any site. Commit p50/p99 338/1,144 ms (502 samples, 466 commands in 195 entries).

| B0-34 line (`build/BUILD-PLAN.md`) | m29 | verdict |
|---|---|---|
| k set in band from `NetworkLoad` and the hot pool, ≤ 8 per site | k 1 throughout on all five networks (`kTimeline`, `kMax` 1) | fail (k never stepped) |
| The hostile owner held to its per-owner pool budget; the monorepo's k not reduced by another owner | k 1 everywhere, so untested | not measured |
| Human push p99 ≤ 3 s beyond transfer | 4 of 36 ok; p99 189–304 s by phase; the rest 503 `warming` or client timeouts | fail |
| Clone TTFB p99 ≤ 1 s throughout (fail over 5 s) | 5 of 35 answered 200; p99 137.7–197.8 s by phase | fail |
| No Cloudflare API call from a request path | 0 files (`b034.sh`'s check) | pass |

Agent pushes 31 of 3,192 ok, most `503 git cache: no shard could check the pack ({"warming":true})`: the kill at
300 s (17:36Z) sent the home site's traffic to site b's shard, which was still rehydrating (done 17:34:33Z) or was
reached through a shard that answered `warming`. `tip:move` 3 of 11 ok. The second network's and the hostile owner's
pushes went through (33 of 41 ok), their clones mostly did not (5 of 24).

**Verdict.** B0-34 fails on m29. What binds is the git cache: every shard of this network needed 32 to 44 minutes to
rehydrate, so after the kill no warm shard was left, and k never left 1, so `fanout` added none.

**Cost** $127.06 at 17:52Z (containers $49.34). Sweep after the row: "no shard container running" (b034-mono's shards
torn down at the end, as the run ends here).

### Round 18c: B0-34 trace from m29's logs: why k stayed 1, why pushes got `warming`, and what came before the kill (2026-11-14)

No hosted spend: Workers Logs (shard objects `git#<s>#net:131296`, ids 6c3553f3 / 8cdabda8 / 617c87d6), the container
CPU dataset, two ledger reads (`/_bed/pool`), and `mwgoigqa`'s rows. Cost $127.09 at the end; sweep "no shard container
running".

**What actually happened.** From 17:31:48Z, 33 s into the window, the home shard (site 0) answered no validate 200 again.
Site 2's last 200 was in 17:32, site 1's in 17:35. After that, every validate on each site waited out its `wait` and
answered 503 `warming`: site 0's at 14.9–15.1 s (`wait=15`), site 2's at 59–61 s (`wait=60`), site 1's at 15–22 s.

| minute | site 0 validate 200 / 503 | site 1 | site 2 | container CPU-s, sites 0 / 1 / 2 (cgroup, 240 = 4 vCPU) |
|---|---|---|---|---|
| 17:31 | 10 / 0 (36 × 429 slots full) | – | 0 / 0 | 193 / 69 / 203 |
| 17:32 | 0 / 84 | 0 / 82 | 12 / 64 (29 × 429) | 240 / 81 / 228 |
| 17:33 | 0 / 293 | 0 / 289 | 0 / 47 | 250 / 80 / 226 |
| 17:34 | 0 / 23 | 6 / 57 | 0 / 254 | 198 / 104 / 202 |
| 17:35 | 0 / 168 | 10 / 17 (37 × 429) | 0 / 19 | 188 / 227 / 200 |
| 17:36 | 0 / 23 | 0 / 123 | 0 / 3 | 187 / 127 / 201 |
| 17:37–17:46 | 0 / 2,000+ | 0 / 2,000+ | 0 / 2,000+ | site 0 torn down 17:37:29, then ~70 (rehydrating); 1 and 2 at 156–209 |

The kill (`/_bed/kill`, sent at 300 s) reached site 0's shard at 17:37:22Z (`/teardown` 200 at 17:37:29, ≈374 s of the
window; the driver logged 200 at 496 s). The catch-up that had run on site 0 since about 17:32 ended only then, with
`shard catch-up 131296: Error: Network connection lost.`

**Why the catch-ups stopped: a cancelled depth-1 clone leaves the network's lock held [inference: timing on all three
sites, plus a local reproduction of the mechanism; no container stderr was captured].** B0-34's `cloneTtfb`
(`tests/am1/rest/a1.mjs`) cancels its fetch after the first byte. The container's `handleUploadPack`
(`containers/git/server/main.go`) holds the network's lock shared, copies upload-pack's stdout to the response, and then
calls `cmd.Wait()`. When the client has gone, the copy fails and returns. Nothing reads the pipe or kills the child, so
`git upload-pack` blocks on a full pipe, `Wait` never returns, and the shared lock is never released. The next catch-up's
`/ingest` then waits for the lock alone. Go's `RWMutex` makes every later `RLock` (validate, ls-refs, upload-pack) wait
behind that writer, so the catch-up never ends and every validate's `Jobs::wait` times out to `warming`. Locally, a Go
program with the same copy and wait calls (`head -c 1G /dev/zero` as the child and a writer that fails after one chunk)
stayed in `cmd.Wait` for over 10 s, with the waiting writer and a later reader both blocked. The timing on each site:

| site | first cancelled clone (shard `/upload-pack`, outcome canceled) | last validate 200 |
|---|---|---|
| 0 | started ≈17:31:41 (ended 17:33:35, 114 s wall) | 17:31:48 |
| 2 | started ≈17:32:06 (ended 17:33:37, 91.5 s) | 17:32 |
| 1 | started ≈17:35:01 (ended 17:35:53, 53 s) | 17:35 |

Round 18b's verdict line, "after the kill no warm shard was left", is wrong: no shard was serving well before the kill.

**1. Why k stayed 1.**
- **Reports.** `fanout`'s input comes only from inside a `/validate` that has caught up (`workers/site/src/shard.rs`,
  the `due` block after `caught_up()`). From 17:32 no validate caught up, so the leader (site 0, `repo:131296#1#f0`) got
  three `/fanout` calls in the whole window: 17:31:33, 17:32:06 and 17:34:34. `fanout` steps only when the larger warm
  site's `cpu_p99` stays above `FANOUT_UP` 0.75 for `FANOUT_UP_FOR` 180 s, which three reports in 3 minutes cannot show.
  The `cpu_p99` values in those reports were not logged (not measured).
- **CPU.** The containers did burn CPU: about 3.2–4.2 vCPU on sites 0 and 2 from the warm's end, and on site 1 from its
  rehydrate's end at 17:34:33. Each site's jump follows its own rehydrate's end, so it is the `/refs` → `startRepack`
  geometric repack of 257 packs plus its commit-graph [inference]. That is derived work, which `/load` leaves out by
  design (A31). So the network's `NetworkLoad` read near idle even if more reports had arrived.
- **Hot pool.** No pool shard was warm for the monorepo. None of the 13 pool objects that exist (`git#0#pool:1–7`,
  `git#1#pool:1,2,4–6`, `git#2#pool:1`) ran a container from 16:45 to 17:50. Both warm sites' ledgers hold no lease
  (`/_bed/pool?site=0` and `site=1` give `{"leases":[]}`); this network's warm sites are 0 and 1 by `warm_sites`'
  rendezvous. With k 1 and no step, `pool_ask(1, None)` is `None`, so the leader never asks a ledger. A pool shard
  catches up only when a granted push is forwarded to it with `pool=1`, and with no cursor it answers `warming`
  (`warming_now`'s `pool_cold`).
- **DESIGN versus code.** DESIGN, Shape, `HOT_POOL`: "Idle pool shards keep hot networks warm, holding the parts up to
  the tip, so a step needs no rehydrate". Costs: "Its idle shards keep hot networks warm, so a step up in k needs no
  rehydrate". No code warms a pool shard for a network before a grant: `prsync::warm` catches up only each Full site's
  first shard (`git#<s>#net:<repo>`, `promote_sites`). So a step up here would have meant a 32–44 minute rehydrate on
  each granted pool shard (round 18b's figures).

**2. Why pushes after the kill got 503 `warming`.**
- **The path.** The edge's `validate` (`workers/edge/src/lib.rs`) tries `shard_order`: `validate_sites` lists the home,
  then every other Full site in record order, and sites this isolate saw fail in the last `COLD_SHARD_MS` (30 s) go last.
  `shard_url` gives the first try `wait=15` and later tries `wait=60`, and every try but the last carries `next=1`.
  `validate_on` calls `shard_at(site)`, which is always that site's first shard `git#<site>#net:<repo>`, never a pool
  shard. So it does use another site's warm shard: the 503s on sites 1 and 2 above are those fallbacks.
- **Where the code departs.** `validate_streamed`, used for a pack over `INLINE_MAX` (1 MiB), sends the body to the
  first site of `shard_order` only, with one try (`shard_url(&url, 1, 2)`). DESIGN, Shape, `IngestCursor`, says "Pushes,
  fetches and the archive's (stateless) promotion try own site, then other Full sites; no shard checked is 503". This
  row's agent packs averaged 90 KB, so it bound only big human pushes.
- **What bound this row.** There was no serving shard to fall back to. Every Full site's shard had the stuck catch-up
  described above, so each answered `warming` after its full wait. After 17:37:29, site 0's new container was
  rehydrating, and it answered `next=1` callers at once (189 answers in 0–1 s at 17:40). DESIGN's failure-modes row for
  git caches at data-center scope says "Each network is warm on two sites, … so the other warm site already holds k
  shards". That holds only while the other site's shard can catch up. The container bug above defeats it on every site
  at once, because every site's shard serves the same cancelled clones.

**3. How much of the miss came before the kill.** `note()` is called when a request ends, so a phase is the phase it
finished in.
- **Before the kill (`@j9`, 0–300 s).** Human pushes: 8, 3 ok, p50 45.8 s, p99 189.0 s. Clone time to first byte: 10, 4
  answered 200, p50 118.3 s, p99 197.8 s (the row's highest). Agent pushes: 435, 22 ok, 210 of them `warming`.
- **Kill phase (`@kill`, 300–420 s).** Human push p99 304.1 s and clone p99 142.5 s. These requests ended after 300 s,
  but at those durations they started before it.
- **So:** the human push and clone misses are already whole before the kill. The stall began at 33 s of the window, and
  the kill was not the cause.

**For the owner.** The fix is likely in `handleUploadPack`: kill the child, or drain its stdout, once the response write
fails, and release the lock before `cmd.Wait`. It needs a test where a client cancels mid-stream and a later
ingest-then-validate still completes. The pool-warming gap and `validate_streamed`'s single try are separate departures.

### Round 19: B0-33 on m30b (main e8d69e4c, memory-budgets), stopped at 189 s by the 120 MiB guard (2026-11-14)

**Deploy m30, then m30b.** lm1-memory 0f77352b = main e8d69e4c (memory-budgets: DONE_BYTES 8 MB, OUTCOME_KEEP 15 s,
AppendEntries budgets 2 MB per peer, 4 MB learners, 8 MB total, ANSWER_BYTES_MAX 16 MB; M5-WP2; shard-cancel;
pool-warm) plus this branch's instruments. New shard image `sha256:198f296c` (main's `containers/`), standard-4,
NOROLL, test-faults. m30: sites a 5a970d51, b 6b2037c5, c a4d4c67d, edge 2e025e18. Full checks on the merge: 941
tests, 0 failed (check_names and check_links only on `.worktrees/`).

**The first try was skipped, a pool-warm bug.** Repo 131339's forced k 4 answered `stepped: false` with empty grants:
`warm_step` correctly refused the cold step, then `Grant::ask` trimmed the grants to next − 1 = 0, so no slot stayed
held, none was warmed, and the step could never commit (`fanout`'s own step up shared the path). Fix `pool_keep`
(980daa4a here; branch `pool-keep` e017fbde, merged as main dc269249): a waiting step keeps the granted − 1 slots it
would serve from. Test `a_waiting_step_keeps_its_cold_slots_until_they_warm` fails on main (k stays 1); three plants
caught. m30b with it: sites a ba3f8469, b 3b78cae8, c 0953ce1d, edge 1ca30793, same image. Hosted, repo 131340's
forced k 4 held grants [5,2,0] on sites 0 and 2, answered cold twice and stepped 1 → 4 at the third ask, 21 s after
the first.

**Run** `tests/fanout/b033.sh m30r33b`: repo `fm30r33b-b033` (131340), run `mwh1akmb`, 5,000 agents, 300 s from
23:29:02Z, learners 8/8 caught up in 11.4 s, k 4. memguard (2 s, every voter) stopped the window at 23:32:11Z, 189 s
in: site 1, leading, at 122.7 MiB.

| B0-33 line (`build/BUILD-PLAN.md`) | m30b (189 s) | verdict |
|---|---|---|
| Human and merge-queue commit p99 ≤ 1 s | leader commit p99 4,126 ms (n 409, p50 3,179); `merge:only` p99 25.1 s (6 of 6 ok); `push:human` 14 ok of 121 (102 skipped at the cap), p99 172.6 s | fail |
| ≥ 140 agent commands/s admitted (fail < 100) | 416 commands at the end leader (term 7 only); 2,560 agent pushes ok in 189 s = 13.5/s at the client | fail |
| Zero leader changes | term 2 → 7: site 0 to 23:30:19, site 1 23:30:23 to 23:32:11, then sites 2, 0, 1, 2 within a minute | fail |
| Zero `suspect` reports | 13,625 at the end leader | fail |
| Every refusal `ng` or 429 `retry-after` | 503s: read gate `overloaded` 757 and `not leader` 58; git cache `no shard could check the pack` 589; 271 client timeouts | fail |

**What memSplit showed.** The counted buffers stayed small while the wasm heap rose. At the stop, site 1's memSplit
summed about 14 MB (log 7.9 MB, dump cache 2.06 MB, read answers 4.1 MB, member calls 0, held 0, statements 0) against
122.7 MiB of heap; in the minute before, answers peaked at 4.1 MB and outgoing calls at 0.8 MB, so the memory-budgets
caps were never near binding. What tracked the heap was the leader's count of admitted tickets not yet resolved
(`inFlight.admitted`, a count, not bytes):

| leader | heap MiB | admitted (max so far) |
|---|---|---|
| site 0, 23:29:05 → 23:30:17 | 6.3 → 73.6 | 26 → 969 |
| site 1, 23:30:23 → 23:30:36 | 25.3 → 69.4 | 153 → 1,071 |
| site 1, 23:31:58 → 23:32:06 | 82 → 122.7 | 1,071 → 1,225 |
| site 2, 23:32:16 → 23:32:30 | 33.1 → 71.5 | 52 → 627 |

Each admitted ticket keeps its whole command in `Core::admitted` until it resolves (`workers/site/src/host.rs`,
`admitted: HashMap<Ticket, (RequestId, u64, Option<F::C>)>`, cloned at `member.rs` `let cmd = c.cmd.clone()`), so an
agent push's inline pack (up to `INLINE_MAX_HOT`, 64 KB; offered mean pack 90 KB) is held there as well as in the log.
1,225 tickets at up to 64 KB each is up to 78 MB, which with the counted 14 MB is the order of the heap. [inference:
the admitted map's commands are the uncounted bytes; memSplit counts tickets, not their bytes, so this is not
measured.] Commit p99 of 3 to 4 s at about 250 group commands/s offered keeps about 1,000 tickets open. This is not
F71's admission lane (agent queue at most 493 commands, 3.97 MB, `admission` samples), and not the memory-budgets
buffers. The wasm heap never shrinks: site 1 still read 124.7 MiB as a follower at 23:43Z.

Site 0's instance reset at 23:30:19 to 23:30:30 (heap 73.6 → 12.4 MiB, status unanswered for 10 s) with no
`exceeded its memory limit` event in Workers Logs (only canceled `/read`s and `/commit`s, and one `exception` each on
`/raft` and `/commit` at 23:30:38); that reset is what started the leader churn.

**Verdict.** B0-33 fails on m30b on every line. memory-budgets kept its own buffers small (answers at most 4.1 MB,
calls at most 3.4 MB), but the leader's heap still climbs with open tickets, about 60 to 80 KB per admitted ticket by
the table above. The next instrument is `inFlight.admittedBytes`, the sum of `stored_len` over `Core::admitted`;
the fix candidates are dropping the command once the ticket is appended (keeping only what the id-block re-send
needs) or capping admitted bytes with a fast `Full`.

**Raw.** `tests/am1/raw/a1-b0-33-mwh1akmb.jsonl`; `tests/fanout/raw/memguard-m30r33b.{jsonl,err}`,
`terms-m30r33b.jsonl`, `tsave-m30r33b.jsonl.xz`, `rows-m30r33b.status`; skipped try `rows-m30r33.status`. **Cost**
$129.82 at 23:45Z (containers $49.63). Sweep "no shard container running".

### Round 20: B0-34 on m31 (main dc269249, stored-fetch): the promotion drain stalls at entry 18's 53 MB pack; row not run (2026-11-15)

**Deploy m31.** lm1-memory b94d0acc = main dc269249 (stored-fetch, pool-keep, m4-actions-2, narrow-builds) plus this
branch's instruments. New shard image `sha256:b63da3e6` (main's `containers/`, with stored-fetch's `/spool`, `/idx`
and `/connected`). Sites a 4dcf32e1, b c9cc88e0, c 6044ab64, edge 3ed63d67; standard-4, NOROLL, test-faults. Full
checks on the merge: 961 tests, 0 failed, Go and every clippy pass (check_names and check_links only on `.worktrees/`;
both pass on main in a clean worktree, round 19).

**Run** `DRAIN=1 b034.sh m31` (b034drain.mjs, then a sweep, then the cold warm, then the row with memguard and terms),
from 00:27:32Z. Raw `tests/fanout/raw/b034drain-m31.jsonl`, `b034warm-m31.jsonl`, `b034-m31.status`.

repo:131296 as found: 648 entries, the last promotion `Promoted { 1..6 }` from the seed (a 257 B stored pack), 260
stored packs above it, 2,628,169,642 bytes; by bucket, site b's shard read 234 from its own bucket and 27 from site 0,
site c's 220 and 41.

**The drain.**

| step | time (from 00:27:32Z) | what |
|---|---|---|
| promote-ahead fires | 00:28:53 (81 s) | the archive asks site a's shard for `/promote`; `503 {"warming":true}` every 2 min after |
| site a's shard cold rehydrate | about 00:28:22 → 00:54:40 | no `shard rehydrate` line in Workers Logs for site a (sites b and c below) |
| first promotion | 00:54:42 (1,630 s) | `Promoted { 7..16 }` (entry 15's and 16's packs, 40.4 MB) |
| second promotion | 00:54:52 (1,640 s) | `Promoted { 17..17 }` (12.7 MB) |
| stall | 00:54:52 on | every run: `archive: entry 18 names 53125950 pack bytes, over the 50331648 a promotion holds` |

**Drain verdict: stalled, not drained.** The time to the first promotion was 1,630 s (27.2 min), almost all of it the
leader site's shard rehydrating the thin packs cold. Then the drain moved 2 of 648 entries in 10 s and stopped: entry
18 carries one 53,125,950 B stored push, over `PROMOTE_MAX` (48 MiB, 50,331,648 B), and `within_promote_max` refuses a
first entry alone over the cap, so every later run repeats the same refusal and nothing past index 17 can promote. Of
260 stored packs it is the only one over 48 MiB (the seed's 30 MB plan steps produce packs of 10 to 53 MB). So under
stored-fetch's trigger, repo:131296 cannot drain at all; the 2.62 MB/s figure was never reached, and the 45-minute
stop did not decide it. The watcher read the stall as drained (its "no move for 180 s" rule); it now exits 3 for a
stop short of the tail.

[inference, the m3f thread's call: a single entry over `PROMOTE_MAX` needs a path, either promoting it alone past the
cap, or storing a pushed pack above the cap as already self-contained, or a push-side cap at `PROMOTE_MAX`.]

**Cold rehydrates in this round (no stored `.idx` existed yet, `idx_used` 0):**

| shard | done | total | stored packs | bytes | spool | `index-pack` |
|---|---|---|---|---|---|---|
| site b (`git#1`) | 00:53:00Z | 1,507 s | 261 | 2,628,169,899 | 28.7 s | 1,458 s (5.6 s/pack, 1.80 MB/s) |
| site c (`git#2`) | 00:57:15Z | 1,762 s | 261 | 2,628,169,899 | 29.4 s | 1,709 s (6.5 s/pack, 1.54 MB/s) |

Against m29: the fetch ahead halved the spool (53 to 94 s → 29 s), but `index-pack --fix-thin` still holds the lock
for 97% of the time (`lock_ms` = `index_ms`), and the totals (1,507 and 1,762 s) fail DESIGN's 120 s line (fail over
600 s) as m29's did. stored-fetch's projected 83 s applies once the packs are promoted with stored indexes, which the
stall prevents.

**The cold warm and the row.** The warm started at 00:58:35Z on shards torn down after the stall; I stopped it at
01:07:40Z, 9 minutes in (all catch-ups at the 120 s client limit, ls-refs 503 `warming`), because with no promotion
past entry 17 it could only repeat the 25 to 30 minute cold rehydrate above, and the row as written could not start.
Row: **not run**. Sweep after the stop: "no shard container running".

**Cost** $130.32 at 01:10Z (containers $50.03).

### Round 21: B0-34 on m33 (main fb4a1b66, promote-stream): the drain's 45-minute stop fires before the first promotion; row not run (2026-11-15)

**Deploy m33.** lm1-memory 43cc5225 = main fb4a1b66 (promote-stream merged) plus this branch's instruments and one new
log line: each promotion prints its range, pack count, bytes, ms and the site isolate's wasm memory before and after
(`archive::promote_handled`); `tests/fanout/promomem.py` reads each site script's peak memory by minute. Shard image
`sha256:b63da3e6`, the same digest as m31 (promote-stream changed no `containers/` code). Site versions a bc6e0ab6,
b 38691f29, c 846992eb; edge 60b0334f; standard-4, NOROLL, test-faults.

**Run** `DRAIN=1 DRAIN_MIN=45 b034.sh m33` from 03:34:25Z. Raw `tests/fanout/raw/b034drain-m33.jsonl`,
`b034-m33.status`.

**The drain: stopped at its limit, 0 promotions.** Promotions stayed at `Promoted { 17..17 }` (from m31) for the whole
2,724 s. Promote-ahead picked the right next range, `Some((18, 18))`, the 53,125,950 B entry the old cap refused, so the
cap fix works as far as `log_archive` goes: no `over the ... a promotion holds` alarm this round. But all 24 tries, every
2 min from 03:35:21Z to 04:20:04Z, got `503 {"warming":true}` from site a's shard, then from b and c with `next=1`,
each after its 60 s `wait`. Each try's line: `1 packs, 0 B in 60,37x ms, wasm 1.8 -> 1.8 MB`. The exit 2 summary reads
`firstS: null, promotedTo: 17`.

**Not measured:** the site's memory during the streamed promotion of entry 18, and the hosted promotion MB/s G9 needs.
No promotion reached the stream. The site scripts' peak memory over the window was 27 to 36.5 MB, with wasm 1.8 MB
except one site-a `scriptThrewException` at 04:00 (wasm 170.2 MB), the same pattern as m31 at 00:30 and 00:45 (167.5 and
175.0 MB). It is not a promotion: none got past the shard. Traced (note only, not chased): each is the site
script's `scheduled` handler, the `*/15 * * * *` cron (`sweep::run`, then `fold_audit::run`), ending `exception` with
"Promise will never complete." after 234 to 255 s wall and 2.9 to 3.6 s CPU: m33 at 04:04:14Z (cron 04:00), m31 at
00:33:56Z and 00:48:53Z (crons 00:30 and 00:45). Only site a shows it in these windows.

**Why every shard stayed warming for 45 min (m31's took 25 to 29).** Container CPU by 5 minutes, each site:

| window | a | b | c |
|---|---|---|---|
| m31 00:30–00:50 | 333–370 s | 331–362 s | 337–372 s |
| m33 03:35–03:55 | 334–369 s | 332–369 s | 335–375 s |
| m33 04:05–04:15 | 674–955 s | 1,019–1,026 s | 789–1,011 s |

Through about 04:00 every shard ran exactly m31's profile: about 1.15 vCPU, the thin packs' `index-pack --fix-thin`.
Only entries 7..17 have a stored `.idx` (2 of 396 objects under `pack/131296/` in site a's bucket, with `from`/`to` 7..16
and 17..17), so 249 of the 260 pushed packs still re-indexed. Then all three jumped to about 3.3 to 3.4 vCPU and stayed
there to the stop, and no `shard rehydrate` line appeared; m31's came at that point.

The code explains the jump [inference from the code and the CPU; the container logs nothing for these phases]:
`catchup::rehydrate` writes the refs pages, and the last page (`done=true`) makes the container's `/refs` call
`startRepack`, a background `git repack -d --geometric=2` with the default delta window, here over about 265 packs and
2.6 GB of linux. Then, because the marked phase skipped the pushed packs that promotions 7..16 and 17..17 cover
(`state.skipped` non-empty, new since stored-fetch), it calls the backstop `io.connected()`. The container's
`handleConnected` takes `repacking.Lock()` and waits for the whole repack. The rehydrate's place is dropped only
after that check, so the shard answers `warming` until the repack ends. In m31 nothing was skipped (no `.idx` existed
yet), so `connected` never ran and the shard served while the repack ran behind it.

So two separate things held the drain:

1. **Cold-start order (design).** The first promotion needs a warm shard, and a cold shard of an unpromoted network
   re-indexes every thin push first: about 26 min here. A drain started on cold shards cannot reach its first
   promotion inside DESIGN's 120 s, or inside the warm step's own wait, until the network is promoted. That was known
   (round 20); this round confirms it again at the same rate.
2. **A new wait (code).** stored-fetch's backstop `/connected` waits on `repacking`, and the rehydrate's own last
   `/refs` starts that repack just before it. Every rehydrate that skipped a covered pack now adds a whole geometric
   repack of the network to its time. Here that was at least 20 min, still running at the stop.

**Cold warm and row: not run** (the drain exits 2, so b034.sh stops). Sweep after the stop: two passes, "sweep: no
shard container running".

**Cost** $131.15 at 04:25Z (containers $50.67); this round cost about $0.70.

### Round 22: B0-34 on m34 (main 1552e999, promote-stream and connected-after): 51 streamed promotions drain entries 18 to 234 at 1.65 MB/s with the site at 18.7 MiB wasm; the 90-minute stop fires short of the tail; warm and row not run (2026-11-15)

**Deploy m34.** lm1-memory 71bec610 = main 1552e999 (connected-after merged) plus this branch's instruments. Shard image
`sha256:36c17feb` (connected-after's container). Sites a e7c34df8, b 5b91624b, c bc99d3f1; edge bfed05b9; standard-4,
NOROLL, test-faults. Run `DRAIN=1 DRAIN_MIN=90 b034.sh m34` from 05:18:05Z (drain stop 90 minutes in all, no stall exit
in the first 45). Raw `tests/fanout/raw/b034drain-m34.jsonl`, `b034-m34.status`, `keepled-m34.jsonl`.

**The group idled out.** 3 minutes into the drain (05:21Z) all three members of repo:131296 were followers at term 2,
and the drain read `no leader` for 11 samples: its watcher reads members only (`/status`, `/read`, `/stats`), which a
group does not count as clients, and nothing else touched the group. The archive alarm runs only on a leader, so
promotion stopped with it. At 05:32Z one edge read woke site 0 back to leader (the term stayed 2), and
`tests/fanout/keepled.mjs` (one edge read a minute) kept it led to the end. `b034.sh` now starts keepled for the drain's length. The shard warm at the drain's start was also cut short: the
container instances still running from round 21's m33 were the same disks (site a's `git#0#net:131296` ran from
04:33:55Z on the old image `b63da3e6` until 06:55Z), so this round's first promotion went to an already rehydrated
shard, not a cold one (below).

**The drain: streamed promotion works and holds the site flat.**

- First promotion at 05:31:55Z (845 s after the start; 10 of those minutes are the idle-out above): `Some((18, 18))`,
  entry 18's 53,125,950 B push, now **promoted, 61,072,183 B in 29.9 s, wasm 1.9 → 3.5 MB**. The cap refusal of round
  20 is gone.
- **Entry 18's site memory:** the promotion runs in its own site-a invocation (`/archive/promote`). Its wasm went 1.9 to
  3.5 MB in the promotion's own log line. Over the whole drain, every promotion's after-value is at most 18.7 MB, reached
  by the fourth (21..22, 146 MB) and flat after it through a 207 MB promotion. The only site-a invocations over 18.7 MB
  wasm in the window are the six `*/15` crons, 156.6 to 195.1 MB, `scriptThrewException` (round 21's note: `sweep::run`
  and `fold_audit::run`), the same as before promote-stream and not a promotion.
- **51 promotions, entries 18..234: 211 pushed packs, 2,217,077,014 pushed bytes (the stored packs those entries name),
  7,404,855,988 promoted bytes in 4,489.6 s of promotion time: 1.65 MB/s of promoted bytes, 0.49 MB/s of pushed bytes
  covered.** By phase: promotions 2 to 19 (05:33 to 06:06Z) 1.32 MB/s, the last 10 (06:37 to 06:49Z) 1.95 MB/s. A
  promoted pack is 3.3 times the pushed bytes it covers on this history (7.40 / 2.22 GB), so the pushed-byte rate G9
  compares against B0-31's pushes is **0.49 MB/s**, against the 2.62 MB/s measured locally (`lm1-rows-m3f.md`,
  "Promotion rate"). [measured; promoted bytes and ms from each promotion's log line, pushed bytes from the log's
  entries 18..234]
- Stop: 06:48:07Z, exit 2 at the 90-minute limit, `promotedTo` 227 (234 by the last line at 06:49:03Z), against a
  target of 678 (commit 730 at the stop). Left above the last promotion: 49 stored packs, 357,944,637 B (entries 235 to
  the end). The drain did not finish; at the late rate it needs about 12 more minutes of promotion, but promote-ahead
  also stops by design once less than `LOG_MAX_BYTES / 32` (64 MiB) of packs sit above the last promotion, so "drained"
  for b034drain.mjs's rule (`to >= target - 8`) is not reachable as written either [inference from
  `log_archive::step`].

Promotion table:

| at | range | pushed packs | promoted bytes | ms | MB/s | wasm before → after |
|---|---|---|---|---|---|---|
| 05:31:55 | 18..18 | 1 | 61,072,183 | 29,901 | 2.04 | 1.9 → 3.5 |
| 05:33:05 | 19..20 | 2 | 114,260,865 | 68,134 | 1.68 | 3.5 → 13.6 |
| 05:34:41 | 21..22 | 2 | 146,427,041 | 93,553 | 1.57 | 13.6 → 18.7 |
| 05:36:24 | 23..24 | 2 | 145,385,404 | 100,470 | 1.45 | 18.7 → 18.7 |
| 05:37:46 | 25..26 | 2 | 125,374,222 | 80,276 | 1.56 | 18.7 → 18.7 |
| 05:39:40 | 27..28 | 2 | 156,846,812 | 111,074 | 1.41 | 18.7 → 18.7 |
| 05:41:50 | 29..31 | 3 | 164,651,524 | 128,124 | 1.29 | 18.7 → 18.7 |
| 05:43:31 | 32..33 | 2 | 118,368,840 | 96,970 | 1.22 | 18.7 → 18.7 |
| 05:45:03 | 34..35 | 2 | 119,869,890 | 89,429 | 1.34 | 18.7 → 18.7 |
| 05:47:01 | 36..37 | 2 | 150,739,008 | 115,482 | 1.31 | 18.7 → 18.7 |
| 05:49:24 | 38..39 | 2 | 179,812,627 | 139,841 | 1.29 | 18.7 → 18.7 |
| 05:52:13 | 40..42 | 3 | 207,310,890 | 166,943 | 1.24 | 18.7 → 18.7 |
| 05:54:13 | 43..44 | 2 | 144,931,350 | 116,590 | 1.24 | 18.7 → 18.7 |
| 05:56:46 | 45..47 | 3 | 169,168,555 | 150,044 | 1.13 | 18.7 → 18.7 |
| 05:59:01 | 48..50 | 3 | 169,163,215 | 132,168 | 1.28 | 18.7 → 18.7 |
| 06:01:34 | 51..54 | 4 | 148,915,235 | 149,675 | 0.99 | 18.7 → 18.7 |
| 06:04:10 | 55..58 | 4 | 183,507,098 | 151,893 | 1.21 | 18.7 → 18.7 |
| 06:06:06 | 59..60 | 2 | 173,507,412 | 112,902 | 1.54 | 18.7 → 18.7 |
| 06:07:36 | 61..62 | 2 | 131,261,634 | 86,662 | 1.51 | 18.7 → 18.7 |
| 06:08:59 | 63..65 | 3 | 206,394,995 | 80,703 | 2.56 | 18.7 → 18.7 |
| 06:10:10 | 66..68 | 3 | 123,412,816 | 68,305 | 1.81 | 18.7 → 18.7 |
| 06:11:25 | 69..70 | 2 | 131,756,177 | 72,392 | 1.82 | 18.7 → 18.7 |
| 06:12:40 | 71..73 | 3 | 134,903,179 | 71,304 | 1.89 | 18.7 → 18.7 |
| 06:13:59 | 74..76 | 3 | 129,021,949 | 76,217 | 1.69 | 18.7 → 18.7 |
| 06:15:09 | 77..80 | 4 | 132,400,296 | 67,054 | 1.97 | 18.7 → 18.7 |
| 06:16:23 | 81..84 | 4 | 126,245,557 | 70,848 | 1.78 | 18.7 → 18.7 |
| 06:17:38 | 85..88 | 4 | 130,918,235 | 71,200 | 1.84 | 18.7 → 18.7 |
| 06:18:48 | 89..92 | 4 | 134,861,742 | 67,074 | 2.01 | 18.7 → 18.7 |
| 06:20:04 | 93..98 | 4 | 124,651,479 | 71,803 | 1.74 | 18.7 → 18.7 |
| 06:21:10 | 99..102 | 4 | 117,557,062 | 62,400 | 1.88 | 18.7 → 18.7 |
| 06:22:08 | 103..105 | 3 | 106,688,727 | 55,325 | 1.93 | 18.7 → 18.7 |
| 06:23:20 | 106..109 | 4 | 140,104,970 | 69,003 | 2.03 | 18.7 → 18.7 |
| 06:24:29 | 110..113 | 4 | 127,409,443 | 65,786 | 1.94 | 18.7 → 18.7 |
| 06:25:59 | 114..119 | 6 | 147,945,855 | 86,027 | 1.72 | 18.7 → 18.7 |
| 06:27:13 | 120..126 | 7 | 149,478,383 | 69,501 | 2.15 | 18.7 → 18.7 |
| 06:28:21 | 127..130 | 4 | 121,493,527 | 63,846 | 1.90 | 18.7 → 18.7 |
| 06:29:40 | 131..136 | 6 | 162,401,476 | 76,002 | 2.14 | 18.7 → 18.7 |
| 06:31:05 | 137..140 | 4 | 144,031,465 | 80,507 | 1.79 | 18.7 → 18.7 |
| 06:32:35 | 141..145 | 5 | 160,209,364 | 86,311 | 1.86 | 18.7 → 18.7 |
| 06:34:15 | 146..150 | 5 | 201,093,617 | 96,270 | 2.09 | 18.7 → 18.7 |
| 06:35:26 | 151..155 | 5 | 132,610,610 | 67,225 | 1.97 | 18.7 → 18.7 |
| 06:36:48 | 156..161 | 6 | 150,785,279 | 78,029 | 1.93 | 18.7 → 18.7 |
| 06:38:04 | 162..167 | 6 | 149,693,875 | 72,603 | 2.06 | 18.7 → 18.7 |
| 06:39:22 | 168..173 | 6 | 143,249,407 | 74,005 | 1.94 | 18.7 → 18.7 |
| 06:40:47 | 174..180 | 7 | 161,669,360 | 81,318 | 1.99 | 18.7 → 18.7 |
| 06:42:08 | 181..185 | 5 | 154,673,848 | 77,268 | 2.00 | 18.7 → 18.7 |
| 06:43:39 | 186..195 | 10 | 160,012,207 | 87,728 | 1.82 | 18.7 → 18.7 |
| 06:45:00 | 196..208 | 9 | 153,484,202 | 75,007 | 2.05 | 18.7 → 18.7 |
| 06:46:31 | 209..217 | 9 | 156,988,903 | 86,504 | 1.81 | 18.7 → 18.7 |
| 06:47:46 | 218..227 | 10 | 142,278,306 | 70,289 | 2.02 | 18.7 → 18.7 |
| 06:49:03 | 228..234 | 7 | 135,855,872 | 71,625 | 1.90 | 18.7 → 18.7 |

**Cold rehydrate: not measured this round.** No `shard rehydrate 131296` report appeared, because no shard rehydrated:
site a's container from round 21 (started 04:33:55Z, image `b63da3e6`) was still running when the drain began, with its
cursor, and served every promotion; sites b and c ran at 1.0 vCPU until 05:40 to 05:45Z (round 21's leftover rehydrates)
and then idled. Round 21's sweep tore the containers down at 04:23Z, but a new leadership's `warm=1` catch-ups at 04:33:55Z
(`Member shard warm 131296 site 0..2`, after round 21's diagnostic edge read) restarted all three before this round. So whether
`/connected` now runs before the repack is not yet shown hosted; the code and its tests are on main.

**Cold warm and row: not run** (drain exit 2). Sweeps after the stop: "sweep: no shard container running" (06:55Z, and
again at 06:59Z after the next shard object restarted one on its alarm).

**Cost** $132.97 at 06:59Z (containers $52.29); this round cost about $1.61.

### Round 23: B0-34 on m34 (main 1552e999), tag m35: drained to the promote-ahead threshold, cold warm 154 to 214 s on 36c17feb, row fails because its 8 new learners never caught up (2026-11-15)

**Deploy.** Unchanged from round 22 (no site day): sites a e7c34df8, b 5b91624b, c bc99d3f1; edge bfed05b9; shard image
`sha256:36c17feb`. Run `DRAIN=1 DRAIN_MIN=90 WARM_MIN=45 IMG=sha256:36c17feb b034.sh m35` from 07:18:03Z.

**Tooling changes (this commit).**
- `b034drain.mjs`: drained is the design's own rule, the pack bytes above the last `Promoted` at or under
  `LOG_MAX_BYTES / PROMOTE_AHEAD` (64 MiB), summed from the log the way `log_archive::pack_bytes_above` does (a stored
  pack's `bytes`, an inline pack's length, at least the row's). Round 22's `to >= target - 8` could not be met.
- `netimages.py`: the running shard containers of one network with their image digests; `--none` exits 2 if any runs,
  `--image` exits 3 on another digest. `b034.sh` runs it after the teardown (no shard may run before the cold warm) and
  after the warm (every shard the warm started runs `IMG`).

**The tail drained before m35 started.** After round 22's sweep (06:59Z), the group woke at 07:03:49Z (noop entries
737 and 738; what woke it was not traced). The leadership's warm catch-ups restarted all three shards cold on 36c17feb,
which rehydrated in 243.5 s (b), 351.4 s (a), and 381.3 s (c): 80 stored packs, 58 loaded by their stored `.idx`,
239 pushed packs skipped. Promotion then resumed: `Some((263, 269))` at 07:10:25Z and `Some((270, 275))` at 07:11:48Z.
The round 22 promotions after its stop, plus those two, made 8 promotions from 06:49:03Z to 07:11:48Z: 1,134,011,885 B
in 562.4 s, 2.02 MB/s promoted. Entries 235..275 cover 39 pushed packs of 295,583,003 B. At m35's start the
unpromoted pack bytes above entry 275 were 63,871,110 B (45 packs), under 67,108,864, so the watcher read drained in
8.4 s (`b034drain-m35.jsonl`). **G9's rate from round 22 stands:** connected-after changed only the container's
`/refs` and `/connected`, not `/promote`, and the streaming runs in the site from 1552e999, so the 1.65 MB/s promoted
and 0.49 MB/s of pushed bytes measured promotion as it ships.

**Cold warm: the first hosted test of connected-after.** Teardown at 07:18:45Z: 3 torn down, "sweep: no shard container
running", `netimages.py 131296 --none`: 0 running. The warm started 07:18:47Z. Every shard it started ran
`sha256:36c17feb394f20d8619365190f266c4d48b1a0aca493f4d5833d9ace950207c9` (`netimages.py --image`: 3 running,
0 on another image).

| shard | rehydrate done | total | marked phase | tail | `/connected` and follow (refs to end) | stored packs | `idx_used` | rejected | spool (summed) | `index-pack` (summed) | lock |
|---|---|---|---|---|---|---|---|---|---|---|---|
| site b `git#1` | 07:21:37Z | 153.9 s | 100.6 s | 39.4 s | 11.9 s | 71 (8.53 GB) | 60 | 0 | 79.5 s | 38.1 s | 54.9 s |
| site c `git#2` | 07:22:07Z | 182.5 s | 122.3 s | 44.8 s | 13.1 s | 71 | 60 | 0 | 92.3 s | 43.2 s | 63.4 s |
| site a `git#0` | 07:22:37Z | 214.1 s | 155.4 s | 43.3 s | 13.2 s | 71 | 60 | 0 | 126.6 s | 42.5 s | 61.9 s |

Every shard skipped 250 pushed packs that promotions cover, and no backstop fault was found (`fault: null`). ls-refs
listed main at 163 s; the warm's summary had catch-ups done at 155.7 / 186 / 215.7 s. **Against DESIGN's 120 s line:
fail (154 to 214 s), under the 600 s fail value**, against 1,507 to 2,639 s on rounds 18b and 20.
- `/connected` no longer waits on the repack. The span from the last refs page to the rehydrate's end, which holds the
  backstop walk, was 11.9 to 13.2 s, not round 21's 20 minutes and more.
- The repack starts when `/connected`'s walk ends, by construction (`handleConnected` calls `startRepack` after
  `connectedWalk`). The container logs no repack start, so the order comes from the code. Site b's container CPU
  rose to 108 s in the minute its rehydrate ended (07:22) [inference: the repack].
- The time goes to the marked phase, 101 to 155 s. Its 71 stored packs (8.53 GB, each promotion's pack is 3.3 times
  the pushes it covers) are fetched four at a time, and 11 packs without a stored `.idx` re-index. The spool now costs
  more than `index-pack`.

**Row: fail, for a reason outside the git path.** a1 run `mwhibsq2`, 07:31:38Z to 07:46:38Z, exit 0.
- **Cause [verified in the code and on the live leader]:** `learners.mjs` placed 8 new learners at config 749, as
  generation 3 (`repo:131296#3#l0..l7`) on the addresses generation 2 used (192 to 199). The leader had led term 2 since
  before generation 2 was placed (config 295, 2026-11-14). `Member::reconfigure` resets `next`/`matched` only for the
  sites its `names()` lists, and that lists voters only (`crates/raft/src/member/leader.rs`, `reconfigure`, "A site that
  comes back names a new member: nothing the old member on that site answered may count for it"). So each new, empty
  learner kept its predecessor's `matched`, about 748.
  - `on_append_res` keeps `next >= matched.next()`, so every probe to it went out at prev 748 with entries 749 to 752.
    Leader `/stats` showed all 32 recent appends to 192 as `first 749, last 752`, 1,776 sent, 144.9 MB.
  - The learner refused each one with `last 0`. Learner `l0` showed applied 0, commit 0, config null, 16,187
    `AppendStepped` of indexes 752 to 759 in its trace, 10,089 reads, and 10,057 answered late.
  - `a1.mjs`'s wait read "learners caught up to 749: 0/8 in 121788 ms".
  - Snapshot: `tests/fanout/raw/learners-m35.json.xz`.
- **What it did to the row.** The read gate's access rows go to learners first. Each learner never applied the read
  index and answered `overloaded`, and the edge answered 503 "every member asked shed the read: overloaded". Results:
  - agent pushes ok: 23 of 1,223 (j9), 4 of 500 (kill), 2 of 365 (second), 23 of 1,501 (hostile);
  - human pushes ok: 5 of 125 at j9;
  - `read:api`: 1,299 × 200, 58,305 × 503;
  - clone TTFB at j9: p50 4.05 s, p99 72.3 s, 12 of 192 ok;
  - task creation: 0 of 100;
  - k stayed 1 on all five networks;
  - feed: 212,684 polls, all `lagging`.
- **Same failure before.** Three earlier B0-33 tries show the same "0/8 in 120 s" (fm25s33, fm26s33, fm28r33). The tries
  that caught up 8/8 (fm28r33b, fm30r33b) [inference: had a new leadership in between, which starts every peer's
  `matched` at 0].
- **Memory was not the cause.** memguard: 550 samples, voter peaks 37.8 / 19.6 / 17.6 MiB, no 120 MiB stop, no term
  change (terms.mjs: term 2 throughout).

**Verdict against B0-34's line.** Gate: k in band at most 8, hostile owner held to its pool budget, human push p99 at
most 3 s beyond transfer, clone TTFB p99 at most 1 s. Fail values: k over 8, any Cloudflare API call, the monorepo's k
reduced by another owner, or TTFB p99 over 5 s.
- **Fail:** TTFB p99 72.3 s.
- **Holds:** no Cloudflare API call (0 files in request paths); k never over 8; the monorepo's k never reduced by
  another owner (it stayed 1).
- **Undecided:** the hostile budget and k in band, because nothing stepped.

The learner defect is a Raft-core bug in `crates/raft/src` [verified].
- A scratch simulator test reproduced it, and was not committed: 2 learners placed under one leader caught up to 9. A
  second generation on the same addresses, empty, stayed at 0 against commit 15.
- The fix: reset `next`/`matched` for every peer whose member name changes, learners included. It needs a regression
  test, plants, the long sweep and Porcupine, and a rerun of the row.

**Raw.** `tests/fanout/raw/b034-m35.status`, `b034drain-m35.jsonl`, `b034warm-m35.jsonl`, `memguard-b034m35.jsonl`,
`terms-b034m35.jsonl`, `keepled-m35.jsonl`, `learners-m35.json.xz`, `a1-b0-34-m35.jsonl` (a copy of
`tests/am1/raw/a1-b0-34-mwhibsq2.jsonl`). Sweep after the row: 21 torn down, "sweep: no shard container running".

**Cost** $134.38 at 07:49Z (containers $53.17); this round cost about $1.41.

### Round 23b (local, no hosted spend): why promoted packs are 3.3 times the pushed bytes, and what a 120 s rehydrate of 5 GB needs (2026-11-15)

**The method, so the hosted shape can be reproduced.** The method runs on B0-34's own seed: `~/b034/linux.git` and
`~/b034/plan.txt`, the same pushes repo:131296 took, on the orb's 4 CPUs.
- **Pushed bytes:** one thin pack per plan step, `pack-objects --revs --thin` of each step against the one before.
- **Promoted bytes:** the shard's way, `pack-objects` of a range's object ids (`/promote` sends ids only, no path
  names), complete, six steps a range. That matches m34's hosted promotions of 2 to 10 entries.
- Scripts: `tests/fanout/promsize/{measure,m2,rollup}.sh`. Outputs: `tests/fanout/raw/promsize-*.txt`.

**The local model reproduces the hosted ratio.**
- Hosted (m34's 50 promotions after entry 18, against the pushed bytes their entries name,
  `promsize-m34-ratio.txt`): 2.84 to 4.55 times, about 3.2 at the median.
- Local (steps 8..200): 6,323,782,500 B promoted against 2,007,790,379 B pushed thin, **3.15 times**.

**Why [measured locally].** Every promotion is self-contained, so no object is stored twice. But every file a range
touches pays one whole base in that range, where a thin push stored it as a delta against an earlier range.
The factor falls as the range grows (complete `--revs` packs against the sum of the steps' thin packs):

| range | complete | thin pushes | ratio | `pack-objects` time |
|---|---|---|---|---|
| 6 steps (194..200) | 99.6 MB | 30.7 MB | 3.24 | 11 s |
| 24 steps | 286.3 MB | 115.7 MB | 2.47 | 27 s |
| 96 steps | 1,295.5 MB | 701.9 MB | 1.85 | 105 s |
| 192 steps | 3,693.8 MB | 2,007.8 MB | 1.84 | 200 s |

Three things in the shard's promotion add to it. Measured on step 96 (14.3 MB thin, 45.7 MB as a complete `--revs`
pack):
- **The id list has no path names:** 53.3 MB, plus 17%. `pack-objects` groups delta candidates by path, and an id
  alone gives it none.
- **The id list repeats the bases that `--fix-thin` appended:** 61.2 MB, plus 15%.
  - `record` lists every object of the pushed pack's `.idx`. After `--fix-thin`, that index includes the 6,621
    bases it appended from earlier pushes (14.9 MB on disk), so they go into this promotion too.
  - The batched inline path (`ingestRun`) already drops them, by offset.
  - So the operator's guess is half right: bases from earlier promotions are repeated, about 15%. Most of the rest
    is that each range is complete.
- **Delta reuse:** 24 steps rebuilt with `--no-reuse-delta` are 217.1 MB, not 286.3 MB, but take 43 s, not 27 s.

**Roll-up.** I made the 6-step promotions of steps 8..200 as their own packs and ran one `git repack -a -d` over them:
6.32 GB became **2,175,795,442 B, 1.04 times the pushed bytes**. It took 385 s of wall time on 4 threads, with
8.6 GB resident.

**Where round 23's rehydrate time went [verified in the code; the hosted numbers fit it].** The marked phase fetched
**one stored pack at a time**, not four.
- `Ahead::pump` starts another fetch only while the bytes fetched ahead stay within `AHEAD_BYTES` (64 MiB), or when
  nothing is ahead. Every promoted pack is 106 to 207 MB, so the first fetch alone passes the cap.
- The hosted numbers fit: each shard's summed spool time (79.5, 92.3, 126.6 s) is less than its marked phase (100.6,
  122.3, 155.4 s). Four overlapping fetches would sum to more than the wall time.
- So round 23 measured R2 at **one stream: 67 to 107 MB/s** (8.53 GB over the summed spool). The four-stream rate is
  still unmeasured.
- The remaining 38 to 43 s of `index-pack` is the 11 pushed packs below the first promotion, which have no `.idx`.
  The 60 packs loaded by `.idx` cost about 0.28 s each under the lock.

**What a 120 s rehydrate of 5 GB of pushes needs [inference from round 23's phases].** Fixed costs:
- start, markers and refs: about 2 s;
- `/connected` and the follow: about 13 s;
- the tail under promote-ahead's 64 MiB threshold: 39 to 45 s, about 64 MB of thin re-index at 1.0 to 1.55 MB/s.

That leaves the marked phase **about 60 s**.

| promoted shape | bytes for 5 GB pushed | rate needed in 60 s | streams at 67 to 107 MB/s each |
|---|---|---|---|
| today, 6-step ranges (3.15x) | 15.8 GB | 263 MB/s | 3 to 4, if R2 holds per stream |
| id list with paths and no `--fix-thin` bases (about 2.4x) | 12.0 GB | 200 MB/s | 2 to 3 |
| rolled up (1.04x) | 5.2 GB | 87 MB/s | 1 to 2 |

**Candidates, weighed against G9's hosted 0.49 MB/s of pushed bytes:**
1. **Let `AHEAD_BYTES` admit four large packs.** The spool waits on the container's disk, not in memory. It is a site
   constant and adds no CPU. It is the biggest lever for this network, but the four-stream R2 rate is unmeasured.
2. **Promotion sends ids with path names and leaves out the bases `--fix-thin` appended** (`record` keeps them apart
   by offset, as `ingestRun` does). This is container-only, about 25% fewer promoted bytes, and less `pack-objects`
   work per promotion, so it helps G9 rather than costing it.
3. **Roll up promoted packs as `Compacted { consumed, produced }`.** SHARED already defines that event, and the
   rehydrate already skips what it consumes, but nothing writes it yet.
   - It brings the bytes to about 1.04 times the pushed bytes and the pack count to a few.
   - Its CPU competes with promotion on the same shard: 385 s on 4 threads for 2.2 GB.
   - Its memory needs a bound (`--window-memory`, or a geometric roll-up of the newest packs only). One whole
     repack took 8.6 GB resident.
   - It needs a design entry: who triggers it, the network chain's entry, and how a rehydrate orders the packs.
4. **Thin promotions against earlier ones: rejected.** A thin pack's stored `.idx` cannot be used (G5), so every
   rehydrate would re-index at 1 to 1.5 MB/s, an hour for 5 GB.
5. **Longer ranges per promotion** (raise `PROMOTE_MAX`, today 48 MiB of pushed bytes): 3.2 to 2.5 times at 24 steps,
   and `pack-objects` per pushed byte 2.8 to 4.3 MB/s locally. Hosted, the split of a promotion's time is unmeasured.
6. **A lower promote-ahead threshold** cuts the 40 s tail, but adds more small promotions, the slowest per pushed
   byte. Not proposed.

### Round 24: B0-34 on m36 (main cfd46001, fetch-ahead), cold warm 127 to 135 s, then the row (2026-11-15)

**Deploy m36.** Main cfd46001: learner-reset, proof-voters, f74-trace, lease-clock, admitted-bytes and fetch-ahead.
- New shard image `sha256:7e8ff9c4620c7b077504508ce25db54cbc93f9c93484aefa39d4501a6d1dcb65`, built from the merge's
  `containers/`, standard-4, NOROLL, test-faults.
- Sites a `17d8d623`, b `eefb3167`, c `fe25cdd2`; the edge redeployed with current formats.
- The site day ran from a clean worktree at cfd46001. The drivers ran from this branch (b034.sh, memguard.mjs,
  terms.mjs, b034warm.mjs and netimages.py are not on main), with main's `tsave.mjs` and `tests/f74/calls.py`.
- Spend before: $134.83.

**Fix 1 alone.** Fix 2, promotions that pack only their range's own objects by name, applies to new promotions only.
repo:131296's existing promotions stay at about 3.2 times the pushes they cover, so this warm measures fix 1 alone:
four stored packs of any size fetching at once, with the `.idx` streamed.

**Cold warm.** Sequence:
- 11:01Z: teardown and sweep, "sweep: no shard container running", and `netimages.py 131296 --none`: 0 running.
- 11:02:09Z: the warm started (`b034warm-m36.jsonl`).
- `netimages.py --image sha256:7e8ff9c4` afterwards: 3 running, 0 on another image.

| shard | total | up | marked phase | tail | `/connected` and follow | stored packs | `idx_used` | spool (summed) | per stream | in total over the marked phase | streams at once (spool / marked) | `index-pack` (summed) | lock |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| site a `git#0` | 126.9 s | 32.9 s | 66.8 s | 11.5 s | 13.7 s | 70 (8.63 GB) | 61 | 188.2 s | 45.9 MB/s | 129.3 MB/s | 2.82 | 8.1 s | 35.3 s |
| site c `git#2` | 134.3 s | 35.2 s | 70.6 s | 12.1 s | 14.0 s | 70 | 61 | 163.6 s | 52.8 MB/s | 122.3 MB/s | 2.32 | 8.5 s | 41.7 s |
| site b `git#1` | 135.3 s | 35.3 s | 65.4 s | 14.6 s | 18.3 s | 70 | 61 | 140.1 s | 61.6 MB/s | 132.0 MB/s | 2.14 | 10.4 s | 56.4 s |

Each shard skipped 256 pushed packs that promotions cover, with no backstop fault and 0 `.idx` rejected. ls-refs
listed main at 137.8 s; the warm's catch-ups ended at 129.6, 137.7 and 136.9 s.

**Against DESIGN's 120 s line: fail (127 to 135 s), under the 600 s fail value**, against 154 to 214 s in round 23 on
the same packs.
- The marked phase fell from 101 to 155 s to 65 to 71 s. The summed spool is now 2.1 to 2.8 times the marked phase,
  so the fetches overlap, where in round 23 it was less than the phase.
- R2 into one container: 46 to 62 MB/s per stream with about 2 to 3 streams at once, 122 to 132 MB/s in total. Round
  23's one stream ran at 67 to 107 MB/s. So the sum is above the one-stream rate, and the `fetch-ahead` flip ("a summed
  spool rate near the one-stream rate") does not fire. Per stream it falls [inference: a per-container cap near
  130 MB/s, which more streams would not raise].
- Container start (`up`), 33 to 35 s, is now the largest fixed cost. With it, the tail (11.5 to 14.6 s, down from 39 to
  45 s: promote-ahead promoted more of it during round 23's row, 256 packs skipped against 250) and the backstop
  (13.7 to 18.3 s), 120 s leaves the marked phase about 55 s. At 130 MB/s that is about 7.2 GB, and this network
  holds 8.63 GB of promoted packs. Fix 2's 25% fewer promoted bytes on new promotions, or G9's roll-up, would bring it
  under [inference from these phases].
- Site memory during the warm: no member or shard object failed. memguard samples begin at the row, below.

**Row: fail.** a1 run `mwhq9e0l`, 11:09:50Z to 11:24:50Z, 500 agents, exit 0. The home shard was killed at 300 s;
the second network was hot from 420 s and the hostile owner's three from 600 s.
- **The learner fix holds.** `learners.mjs` placed generation 4 (`repo:131296#4#l0..l7`) at config 828. By 11:08Z
  every learner had applied 928 of the leader's commit 929, and at 11:13Z 1,330 to 1,334 against the voters' 1,330.
  a1's wait read "0/8 in 120690 ms" only because it compares `applied < commit` on a moving commit: each learner stayed
  within one entry, where round 23's learners stayed at 0.
- **Memory and terms.** memguard: 550 samples, voter peaks 34.8 / 29.5 / 29.6 MiB, no 120 MiB stop. terms.mjs:
  term 2 throughout, leader site 0. `inFlight.admitted` peaked at 8 tickets and `inFlight.admittedBytes` at 87,180 B,
  both on the leader.
- **Commit path.** Leader commit p50 257 ms, p99 529 ms (n 2,473). `read:api` 85,526 of 85,526 ok, p99 2,115 ms.
- **The git path did not serve.** Every shard ran at 3.5 to 4 vCPU from 11:08Z, which is 210 to 520 CPU-s a minute
  across the three, to the row's end. The shards answered most `upload-pack` and `validate` calls 503 after their full
  15 s or 60 s wait, or the caller gave up at 43 s. For example, site a `upload-pack`: 204 × 200 at 11:10, then 503s
  every minute to 11:24; site c `validate` 11:14: 76 × 503 and 80 cancelled.
  - The catch-ups behind them were slow: site b caught up 1,139 entries in 19.7 s at 11:16:19Z. Site a's catch-up
    ended "Network connection lost" at 11:15:06Z (the kill) and again at 11:20:50Z.
  - The results:
    - clone time to first byte at j9 (0 to 300 s): 40 of 40 aborted at 120 s;
    - human pushes at j9: 8 ok of 172 (160 skipped at the cap), p99 121 s;
    - agent pushes at j9: 132 ok of 859 (478 `full`, 245 timeouts);
    - tasks 0 of 100 ("The requester has too many open tasks": earlier rows' tasks are still open);
    - feed 14,770 polls, 2,649 `gone`.
  - **k stayed 1** on every network, as in rounds 18 and 23.
  - The cause of the shards' saturation is not isolated here [not measured]. Round 18c's inference was the geometric
    repack after each rehydrate plus validates, and these numbers fit it but do not prove it.

**Verdict against B0-34's line.** Gate: k in band at most 8, hostile owner held to its pool budget, human push p99 at
most 3 s beyond transfer, clone TTFB p99 at most 1 s. Fail values: k over 8, any Cloudflare API call, the monorepo's k
reduced by another owner, or TTFB p99 over 5 s.
- **Fail:** clone TTFB p99 121.5 s at j9 and human push p99 121 s at j9.
- **Holds:** no Cloudflare API call (0 files in request paths); k never over 8; the monorepo's k never reduced by
  another owner (it stayed 1).
- **Undecided:** the hostile budget and k in band, because nothing stepped.

**Site memory.** `promomem.py` returned no rows for 11:02 to 11:06Z, so the warm's site memory is not measured. Its
only row near the window is site a at 11:00Z, the site day's first invocation: `scriptThrewException`, wasm 177.5 MB,
version `e7c34df8`, the version the deploy replaced.

**Raw.** `tests/fanout/raw/b034-m36.status`, `b034warm-m36.jsonl`, `memguard-b034m36.jsonl`, `terms-b034m36.jsonl`,
`a1-b0-34-m36.jsonl` (a copy of `tests/am1/raw/a1-b0-34-mwhq9e0l.jsonl`), `rehydrate-131296-m36.txt` (the three
`shard rehydrate` reports). Sweep after the row: 17 torn down, then one more `git#1#net:131296` that started at
11:28:20Z after it; "sweep: no shard container running".

**Cost** $136.32 at 11:29Z (containers $53.91); site day, warm and row about $1.49.

### Round 24b: B0-33 on m36 (main cfd46001) with the F74 trace: the full 300 s ran under the memory guard; terms 2 to 19, learners' read-index calls the bulk of member calls (2026-11-15)

**Run** `tests/fanout/b033.sh m36r33` (rows.sh's b0-33 row as written: fresh repo, 8 learners, k 4, 300 s, 5,000
agents), with `tsave.mjs` at `LEARNERS=1 EVERY=10000`.
- Repo `fm36r33-b033` (131341), run `mwhr84r9`, 11:36:36Z to 11:41:36Z, exit 0.
- k 4 granted at 11:31:38Z (grants [2,1,6] on sites 0 and 2). Tasks 100 of 100.
- a1's learner wait read "0/8 in 122002 ms" against commit 114. It is the same moving-commit read as round 24's B0-34,
  where every learner was within one entry [not checked for this repo before the window].

**Memory: the guard never fired.** 313 samples, voter peaks 40.3 / 44.2 / 51.3 MiB against round 19's stop at
122.7 MiB at 189 s. `inFlight` (memSplit), the most on any sample:

| site | `admitted` | `admittedBytes` |
|---|---|---|
| 0 | 257 | 2,895,947 B |
| 1 | 61 | 690,362 B |
| 2 | 231 | 2,602,706 B |

So admitted-bytes holds an open ticket's command to about 11 KB on average (2.9 MB over 257 tickets), and the
memory storm of rounds 17 and 19 is gone.

**Leaders: churn, term 2 to 19.** terms.mjs: term 2 at 11:31:09Z, then 5 at 11:37:38Z, 6 (site 1) 11:38:00, 8, 9 (site
2), 10 (site 0), 11, 12, 13, 14 by 11:40:20Z, 15 to 17 by 11:41:30Z, and 19 at the row's end. 94 of the 939 member reads
memguard made went unanswered.

**The trace** (`calls.py tsave-m36r33.jsonl --from 11:36:36Z --to 11:41:36Z`, `calls-m36r33.{txt,tsv.xz}`):

| calls | sent | reached the callee | answered |
|---|---|---|---|
| learner → voter (read-index) | 270,129 | 42,277 | 108,421 |
| voter → voter (raft) | 12,778 | 6,542 | 5,152 |
| voter → learner (append) | 3,146 | 3,110 | 2,422 |

- **Learners' calls to the voters are 94% of all member calls:** 307 a second at p50 and up to 14,325 in one second.
  Each learner sends about 15,000 calls to site 0 and 10,000 to each of sites 1 and 2 in 300 s.
- **Voter → voter calls mostly never reached the callee** in the 10 s buckets where terms changed. For example
  11:37:00 to 11:37:40Z: 240 sent, 0 reached, while learners sent 31,472 calls to the voters. When voter calls did
  arrive (11:38:10, 2,702 sent, 1,222 reached), `work` was 0 to 2 ms at p99: the callee's handler was fast once entered.
- **The time is in the legs, not in the handler.** Voter → voter `out` p50 up to 6.3 s (2→1) and p99 up to 40 s
  (2→0), `back` up to 7.3 s, `work` p99 at most 2 ms. Learner → voter totals p50 6 to 10 s. All of this is on callers
  whose loop lag at send was under 100 ms.
- **Reading [inference from the trace, not proven]:** the learners' read-index calls queue ahead of the voters' raft
  calls at each voter, behind the six connections a Workers invocation has waiting for headers, or in workerd's
  per-object input queue. The heartbeats and votes then arrive seconds late or never, and elections follow. This is
  F74's question (`decisions/OPEN-QUESTIONS.md`), and the trace is for B0-02, which owns raft and the member calls.

**Row numbers.**
- Offered: 252 agent-class group commands a second (41.7 agent pushes a second, mean pack 90 KB).
- Agent pushes: 345 ok of 12,503. 6,668 got "every member asked shed the read: overloaded"; the rest were `not leader`,
  `no answer` and timeouts.
- Leader commit p50 2,058 ms, p99 4,622 ms (n 544, at the end leader only).
- Merges: 0 ok of 57 (30 skipped at the cap).
- Human pushes: 1 ok of 185 (153 skipped at the cap).
- `side:comment` 112 ok of 5,576.

| B0-33 line (`build/BUILD-PLAN.md`) | m36 | verdict |
|---|---|---|
| Human and merge-queue commit p99 ≤ 1 s | leader commit p99 4,622 ms; merges 0 of 27 attempted ok; human pushes 1 of 32 attempted ok | fail |
| ≥ 140 agent commands/s admitted (fail < 100) | 345 agent pushes ok in 300 s, 1.15/s at the client | fail |
| Zero leader changes and `suspect` | term 2 → 19 | fail |

**Raw.**
- `tests/fanout/raw/rows-m36r33.status`, `memguard-m36r33.{jsonl,err}`, `terms-m36r33.jsonl`.
- `tsave-m36r33.jsonl.xz` (58.8 MB raw), `calls-m36r33.txt`, `calls-m36r33.tsv.xz`.
- `a1-b0-33-m36.jsonl` (a copy of `tests/am1/raw/a1-b0-33-mwhr84r9.jsonl`).
- Sweep after the row: "sweep: no shard container running", twice.

**Cost** $137.04 at 11:46Z (containers $54.09). Round 24 in all, the site day, cold warm, B0-34's row and B0-33, cost
$2.21 from $134.83.

### Round 24c (no hosted spend): what held B0-34's shard CPU, container start-up, promotions before fix 2, the task close, and site a's cron (2026-11-15)

**Answer: the shard's own geometric repack after the cold rehydrate, which outgrew the container's 12 GiB.** It was not
clients' upload-pack. Evidence, all from data already collected (`containersUsageAdaptiveGroups`,
`containersMetricsAdaptiveGroups`, and the `GitShard` events in Workers Logs):
- **CPU with no requests.** The rehydrates ended at 11:04:33 to 11:04:41Z. From 11:05 to 11:09Z the shards had no client
  request at all, only the warm's `/catchup` polls of 0.1 to 0.6 s each. Even so each container's CPU climbed from 1.3 to
  3.5 vCPU: 770, 662 and 760 CPU-s over those five minutes on sites a, b and c. The row's traffic began at 11:10. So
  the work that held the CPU began before any upload-pack or validate.
- **Memory at the cap.** Each container's peak memory reached 11.95 GiB of standard-4's 12 GiB at 11:05Z, and stayed
  there to the row's end on sites b and c, and on site a until the kill at 11:15. Disk stayed at 9.6 GB, so nothing was
  written: the repack was still searching deltas.
- **One repack seen to its end.** Site a's new instance (after the 11:15 kill) rehydrated at 152 to 170 MB/s
  (11:15 to 11:16), then sat at 11.95 GiB again. At 11:22 to 11:24 its disk grew from 9.6 to 12.4 GB, the repack
  writing its pack. At 11:25 the disk fell to 7.6, then 3.2 GB, the rolled-up packs deleted, and memory fell to
  4.5 GiB. That same minute site a served its first upload-packs since 11:12: 186 × 200. 8.6 GB of packs rolled into
  about 3 GB is the ratio of the local repack below (6.48 to 2.18 GB).
- **Local repack, same shape.** `tests/fanout/georepack.sh` built 33 promoted packs from B0-34's plan the way the
  promotions made them (6-step complete ranges, 6.48 GB, against the 8.63 GB hosted). Then it ran the shard's own
  `git repack -d -q --geometric=2` (`containers/git/server/main.go`, `runRepack`) on 4 threads: **325 s wall, 588 CPU-s
  (1.81 vCPU), maxrss 9.8 GB**, giving 1 pack of 2.18 GB. `commit-graph write` added 15 s. Scaled to 8.63 GB that is
  about 13 GB of memory [inference], over the container's 12 GiB. So hosted, the repack ran pinned at the memory cap:
  8 minutes on site a's second instance, and more than 23 minutes, unfinished, on sites b and c.
- **Why clients got `warming`.** Every upload-pack and validate first waits for the shard object's catch-up
  (`workers/site/src/shard.rs`, `caught_up`). A catch-up `/ingest`s and puts `/refs` on a container at its memory cap
  and 4 vCPU, so it ran slowly or not at all. For example, site b caught up 1,139 entries in 19.7 s at 11:16:19Z, and
  then the next catch-up waited. Between catch-ups, everything queued behind them answered 503 `warming` after its
  15 s or 60 s wait. The bursts of 200s in the table are the minutes a catch-up ended.
- **Why k stayed 1.** `/load` leaves out derived work, and the repack is derived (A31), so `fanout` saw an idle network
  while the CPU was saturated, as round 18c inferred.
- Upload-pack is a lesser load but a real one. One depth-1 protocol-v2 upload-pack of the linux tip costs 24.8 CPU-s and
  1.10 GB RSS, sending 301 MB in 14.6 s; four at once took 102 CPU-s in 52 s (`tests/fanout/d1upload.py`, local). Each
  B0-34 clone misses, because the clone cache holds no `clone/131296/` entry in any bucket. The edge writes the tee to
  R2 only when the body is whole (`workers/edge/src/lib.rs`, `upload_pack`), and B0-34's probe cancels after the first
  byte, so the cache never fills [inference from the code]. DESIGN's composed depth-1 misses (0.2 to 0.5 CPU-s a miss)
  are not in the code (below).

**Timeline per shard** (vCPU from `containersUsageAdaptiveGroups`; peak memory and disk from
`containersMetricsAdaptiveGroups`; `GitShard` upload-pack and validate starts by result, 200 against everything else,
503, cancelled or error). Site a's instance changes at 11:15 (the kill); site b's last minute is a new instance.

| minute | a: vCPU, mem GiB, disk GB | a: upload-pack ok/not, validate ok/not | b: vCPU, mem, disk | b: up, val | c: vCPU, mem, disk | c: up, val |
|---|---|---|---|---|---|---|
| 11:03 | 2.23, 8.48, 5.5 | 0/0, 0/0 | 2.44, 7.62, 5.4 | 0/0, 0/0 | 2.11, 6.57, 4.6 | 0/0, 0/0 |
| 11:04 | 1.33, 11.74, 9.6 | 0/1, 0/0 | 1.3, 9.93, 8.5 | 1/1, 0/0 | 1.29, 10.25, 8.5 | 0/2, 0/0 |
| 11:05 | 1.44, 11.95, 9.6 | 1/0, 0/0 | 1.45, 11.95, 9.6 | 0/0, 0/0 | 1.47, 11.95, 9.6 | 0/0, 0/0 |
| 11:06 | 1.83, 11.95, 9.6 | 0/0, 0/0 | 1.84, 11.95, 9.6 | 0/0, 0/0 | 1.78, 11.95, 9.6 | 0/0, 0/0 |
| 11:07 | 2.49, 11.95, 9.6 | 0/0, 0/0 | 1.82, 11.95, 9.6 | 0/0, 0/0 | 2.37, 11.95, 9.6 | 0/0, 0/0 |
| 11:08 | 3.46, 11.95, 9.6 | 0/0, 0/0 | 2.37, 11.95, 9.6 | 0/0, 0/0 | 3.51, 11.95, 9.6 | 0/0, 0/0 |
| 11:09 | 3.61, 11.95, 9.6 | 0/0, 0/0 | 3.55, 11.95, 9.6 | 0/0, 0/0 | 3.53, 11.95, 9.6 | 0/0, 0/0 |
| 11:10 | 3.97, 11.94, 9.6 | 204/469, 9/122 | 3.75, 11.95, 9.6 | 126/60, 9/51 | 3.49, 11.95, 9.6 | 79/0, 0/18 |
| 11:11 | 4.17, 11.95, 9.6 | 0/68, 0/120 | 4.17, 11.95, 9.6 | 0/392, 0/118 | 3.99, 11.93, 9.6 | 285/0, 105/88 |
| 11:12 | 4.15, 11.95, 9.6 | 266/89, 11/188 | 4.14, 11.95, 9.6 | 0/148, 0/112 | 4.12, 11.95, 9.6 | 0/310, 0/158 |
| 11:13 | 4.16, 11.96, 9.6 | 0/327, 0/140 | 4.14, 11.96, 9.6 | 0/103, 0/146 | 3.91, 11.95, 9.6 | 150/137, 8/267 |
| 11:14 | 4.16, 11.96, 9.6 | 0/155, 0/143 | 4.16, 11.96, 9.6 | 0/246, 0/143 | 4.16, 11.95, 9.6 | 0/240, 0/156 |
| 11:15 | 2.42, 4.59, 2.0 | 0/320, 0/253 | 4.16, 11.95, 9.6 | 0/183, 0/240 | 4.19, 11.95, 9.6 | 0/215, 0/181 |
| 11:16 | 1.63, 9.46, 9.6 | 0/229, 0/215 | 3.99, 11.94, 9.8 | 125/293, 8/307 | 4.17, 11.95, 9.6 | 0/272, 0/324 |
| 11:17 | 1.25, 11.94, 9.6 | 0/286, 0/173 | 4.18, 11.94, 9.8 | 0/500, 0/309 | 4.04, 11.95, 9.6 | 395/218, 11/233 |
| 11:18 | 1.3, 11.95, 9.6 | 0/25, 0/112 | 4.18, 11.95, 9.8 | 0/253, 0/177 | 4.19, 11.95, 9.6 | 0/185, 0/149 |
| 11:19 | 2.68, 11.95, 9.6 | 0/110, 0/59 | 4.16, 11.94, 9.8 | 0/35, 0/112 | 4.19, 11.95, 9.6 | 0/61, 0/151 |
| 11:20 | 3.69, 11.95, 9.6 | 0/60, 0/165 | 4.17, 11.95, 9.8 | 0/4, 0/14 | 4.2, 11.95, 9.6 | 0/5, 0/34 |
| 11:21 | 3.59, 11.95, 9.6 | 0/12, 0/131 | 4.14, 11.95, 9.8 | 43/112, 9/474 | 4.17, 11.95, 9.6 | 0/11, 0/56 |
| 11:22 | 3.08, 11.95, 10.0 | 0/0, 0/0 | 4.18, 11.95, 9.9 | 0/2, 0/6 | 4.19, 11.95, 9.6 | 0/112, 0/390 |
| 11:23 | 3.21, 11.95, 11.4 | 0/164, 0/390 | 4.2, 11.95, 9.9 | 0/4, 0/0 | 3.94, 11.94, 9.6 | 0/0, 0/2 |
| 11:24 | 2.12, 11.95, 12.4 | 0/9, 0/12 | 3.58, 11.96, 9.9 | 0/435, 0/472 | 3.96, 11.96, 9.9 | 189/0, 12/406 |
| 11:25 | 3.36, 4.5, 7.6 | 186/8, 0/0 | 3.35, 11.96, 9.9 | 0/8, 0/1 | 4.17, 11.96, 9.9 | 0/244, 0/68 |
| 11:26 | 2.93, 4.59, 3.2 | 0/0, 3/0 | 3.34, 11.96, 9.9 | 0/0, 0/0 | 4.17, 11.96, 10.0 | 0/2, 0/0 |
| 11:27 | 0.02, 3.75, 3.2 | 0/0, 0/0 | 3.35, 11.96, 9.9 | 0/0, 0/0 | 3.44, 11.95, 10.0 | 0/0, 0/0 |
| 11:28 | 0.01, 3.72, 3.2 | 0/0, 0/0 | 1.95, 5.04, 0.2 | 0/0, 0/0 | –, 0.0, 0.0 | 0/0, 0/0 |

How many ran at once (summed wall over 60 s, `tests/fanout/raw/shardtimeline-m36.jsonl`): upload-pack 20 to 311 and
validate 20 to 334 per shard. Almost all of it was time spent waiting in the shard object for the catch-up, not git
processes.

**Missing measurement.** The container logs nothing per process, and Workers Logs holds no container stdout or stderr.
So the repack is named from its signature: CPU with no requests, memory at the cap, the disk's write-then-delete, and
the local run. It is not named from a process list. A cheap instrument for the next run: `/load` or `/status` lists
the running git children (command, CPU-s, RSS), and the shard object logs it each minute while a repack runs.

**Proposed fix (design first, not built).**
1. **The repack after a rehydrate leaves promoted packs alone.** Promoted packs are already complete and
   delta-searched. A rehydrate writes `.keep` beside each pack it loaded through a stored `.idx`. `git repack
   --geometric=2` then rolls only the pushed tail (local check: geometric rolled no kept pack) and writes the
   multi-pack index and commit-graph. Those cover the merge slowdown with many packs that `REPACK_AT` was set for.
   DESIGN's git caches row and the rehydrate text change, plus a DECISIONS entry. Flip: merges or validates slow over
   the kept packs as they did over 1,950 packs (`tests/m2/results/m2-wp6.md`).
2. **Bound the repack that remains.** `pack.threads=2`, `pack.windowMemory` (for example 256 MiB), and the repack run
   under `nice`, so derived work cannot take all 4 vCPU or the memory cap from serving. G9's roll-up, which is not
   built, needs the same bound.
3. **The clone cache fills from the shard, not from the edge's tee of a read the client may cut.** It could also be
   filled after a push, as DESIGN says ("the edge asks the network's shard to fill the clone-cache entry"). This is
   only for B0-34's TTFB row. It is a separate change, and the composed miss is the larger one.
The measurement for 1 is cheap and local: georepack with the promoted packs kept, against the same 33 packs.

**Container start-up (33 to 35 s): the platform's, not ours.**
- The image is 74.8 MB (alpine, git 2.56, the Go server).
- The entrypoint is `shard-server`: `raiseOpenFileLimit`, `sampleCPU` in a goroutine, then `ListenAndServe`. A local
  `docker run` answered `/health` in 8 ms.
- Hosted, the containers API's instance `started_at` came about 20 s after the warm's start (11:02:09 against 11:02:27
  to 11:02:30Z); the rest is boot to the first `/health`.
- So all of it is scheduling, image pull and boot on the platform's side [inference: `up` is measured from `start()` to
  the first `/health` on 200 ms polls]. Nothing in the image or entrypoint is ours to cut.

**Promotions that predate fix 2.** All 61 promoted packs with stored `.idx` on repo:131296 predate fix 2: 8,615,058,647
B, dated 2026-11-15 00:54:40Z to 07:38:33Z (2 at 00h, 15 at 05h, 41 at 06h, 3 at 07h). fetch-ahead (main cfd46001)
deployed at about 11:00Z. No promotion has run since. The list is in `tests/fanout/raw/promoted-131296-m36.json` (time,
bytes, id).
- **Estimate if they were re-promoted under fix 2** [inference]:
  - Round 23b's local fixture gave about 25% fewer promoted bytes, so about 6.46 GB.
  - At the warm's measured 122 to 132 MB/s, the marked phase is about 49 to 53 s instead of 65 to 71 s.
  - With m36's other phases unchanged (up, tail, `/connected`), totals come to about 110 / 117 / 119 s for sites a, c
    and b: just under 120 s, with no margin.
- A re-promotion is still 6.5 GB of complete packs, so the repack above (9.8 GB maxrss at 6.48 GB locally) would still
  saturate the shard. Fix 1 above is needed either way.

**Tasks closed on b034-mono.** 100 queued tasks were cancelled through `POST .../tasks/{id}/cancel` with the test
installation token (all 200), leaving 0 queued.
- The list API returned the same page for every page number. So the 3,900 "tasks" it seemed to list were 100 unique
  ones, the per-requester cap `TASKS_OPEN_PER_REQUESTER`.
- A probe create then returned 201 (`196608.m36-probe-after-close`), and the probe was cancelled.

**Corrections to round 24.**
- The 11:00Z site-a `scriptThrewException` at wasm 177.5 MB (version `e7c34df8`) was the `*/15` cron, not the site
  day's first invocation. It is now `decisions/OPEN-QUESTIONS.md` R22 (on branch `oq-r22`).
- The m36 site day's edge deploy did not complete. `fanout-m36-edge.log` stops while compiling `graftyard-lookup`, and
  `graft-m2b-edge` stayed on `bfed05b9` (deployed 05:18Z, m34). Round 24's rows ran with an edge from 1552e999, without
  cfd46001's edge changes (lease-clock's edge half). The edge had 4 `exceededMemory` on b034-mono upload-pack at
  11:24:37Z.

**Design contradiction.** DESIGN, Shape, "Git cache shards (derived)": "A depth-1 fetch that misses the clone cache, for
a commit whose tree is close to a cached tip's, is composed: the shard sends the tip's cached depth-1 pack plus a pack
of the wanted commit's objects missing from it". Nothing in `containers/git/server`, `crates/git` or the workers
composes a miss: a miss is a stock `git upload-pack`, 25 CPU-s on linux. BUILD-PLAN's B0-34 line, "with the
composed-miss rules now in the design", is consistent with that. The design states it as built behavior, though, and
B0-34's TTFB line depends on it.

**Raw.** `tests/fanout/raw/instcpu-m36.jsonl` (vCPU, memory and disk per instance and minute),
`shardtimeline-m36.jsonl`, `shardev-m36.jsonl.xz` (17,593 `GitShard` events), `promoted-131296-m36.json`. Tools:
`tests/fanout/instcpu.py`, `shardtimeline.py`, `georepack.sh`, `d1upload.py`.

**Cost** $137.15 at 13:15Z (containers $54.09); this round spent nothing hosted. $7.85 left before $145.

### Round 25 (local, no hosted spend): repack-keep, fixes 1 and 2 for B0-34's shard saturation, and the drivers (2026-11-15)

**Branch `repack-keep`** at 0c171e6d, off main 4833094d (code, tests, plants, DESIGN and DECISIONS). The drivers are on this
branch. Not deployed; it runs on the next site day, once the composed-miss branch and B0-02's read-index fix merge too.

**Fix 1: promotions a rehydrate loads are kept.** `ingestSpooled` writes `pack-<name>.keep` beside a pack it loads
through its stored `.idx`. Only a promotion stores one. `startRepack` counts only unkept packs against `repackAt` (50).
`runRepack` adds `--write-midx`, so a multi-pack index covers every pack, kept or not. Without it, 61 kept promotions
plus the tail would be 62 lookups per object.

**Fix 2: whatever repack remains runs low and bounded.** `runRepack` runs `nice -n 19 git -c pack.threads=2 -c
pack.windowMemory=256m repack -d -q --geometric=2 --write-midx`. Its commit-graph also runs at nice 19 (`gitLow`).
Alpine's busybox has `nice`, so the image needs nothing new.

**Gate: the local georepack, with promoted packs kept** (`tests/fanout/georepack-keep.sh`). Its input is round 24c's
33 promotions (6-step complete ranges of B0-34's plan, 6.48 GB) plus a 12-pack pushed tail, 6.76 GB in 45 packs.

| run | repack wall | repack CPU | maxrss | midx | commit-graph | packs after | promotions untouched |
|---|---|---|---|---|---|---|---|
| round 24c: no keep, unbounded (33 packs, no tail) | 325 s | 588 CPU-s (1.81 vCPU) | 9.8 GB | n/a | 15 s | 1 (2.18 GB) | 0 of 33 |
| **fix 1 + 2: kept, nice 19, 2 threads, 256m window** | **98 s** | **104 CPU-s (1.06 vCPU)** | **5.49 GB** | 3 s | 15 s | 34 (33 kept + 1) | **33 of 33**, midx verifies |
| fix 2 alone (no keep: as a network whose promotions predate the keep) | 757 s | 620 CPU-s (0.82 vCPU) | **9.20 GB** | 3 s | 17 s | 1 (2.23 GB) | 0 of 33 |

- Fix 1 cuts the repack's CPU 5.7× and its memory from 9.8 to 5.5 GB.
- **Fix 2 bounds CPU, not memory.** Alone, it spread the same work (620 CPU-s) over 2.3 times the wall time and kept
  9.20 GB of memory. `pack.windowMemory` bounds only the delta window, while a whole-network roll's memory is the
  object list and the bases. So fix 1 is the fix for the 12 GiB cap, and fix 2 makes whatever repack remains yield
  the CPU.
- The 5.49 GB is the tail's roll: git maps the kept packs to resolve the tail's thin deltas against them, and their
  pages count in its RSS [inference]. The container has 12 GiB, so it fits with room for clones.
- The roll still costs about 1 vCPU for about 100 s. At nice 19, that CPU is taken only when nothing else wants it.

**Can a catch-up or validate still wait behind the repack?**
- **Not on its lock.** Catch-ups (`/ingest`, `/refs`), validates and upload-packs never take `repacking`. Only evict, a
  fresh init, `/fsck` and `/connected` do, and that was already so (LM-1 round 7).
- **On its CPU, much less.** It runs at nice 19 on 2 threads. Under CFS a nice-0 task gets about 1,000 times a nice-19
  task's share of a contended CPU, so a catch-up or validate needing a vCPU takes it [inference: not measured hosted;
  `TestRepackRunsLowAndProcsListsIt` shows only that the repack runs at nice 19].
- **Still possible: memory.** A repack at about 5.5 GB plus four depth-1 linux upload-packs at 1.1 GB each fits in
  12 GiB. Two shards' worth of clone storm on top could still reach the limit, and `nice` does not bound memory.
- **Still possible: `/connected`.** A rehydrate's backstop, run a second time after a fault, waits for the repack, as
  before. Only the fault path pays this (`crates/git/src/catchup.rs`).
- **Not addressed: the cold warm's own CPU.** It is spool and ingest work, not the repack.

**Instrument: the shard object's `/status?repo=` now lists `procs`.** The container's new `GET /procs` returns every
running git process: pid, command (cut to 160 bytes), nice, CPU seconds including reaped children, and RSS in MB, most
CPU first. `ShardHost::procs` reads it, and only from a running container. Test drivers already read `/status` through
`/_bed/shard_status`, so `tests/am1/mb3/shardstatus.mjs` shows it with no change.

**Tests, failing on main.**
- `TestRehydratesRepackLeavesPromotedPacks` (`containers/git/server/repack_test.go`) loads 53 promotions through their
  `.idx` (more than `repackAt`), then a 56-push tail, then `/refs`. It requires the following:
  - the promotions alone start no repack;
  - after the tail's repack, every promotion is still on disk byte for byte;
  - the tail rolled;
  - the multi-pack index names every pack and verifies;
  - fsck is clean and main resolves.
  On main it fails at the first check: "1 packs after /refs over 53 promotions alone".
- `TestRepackRunsLowAndProcsListsIt`:
  - `/procs` finds a nice-19 git process by its pid, with its command, nice and RSS, most CPU first;
  - every git that `runRepack` starts runs at nice 19, seen by a `git` wrapper on PATH;
  - `pack.threads=2` and `pack.windowmemory=256m` reach git, seen through `GIT_TRACE2` parameters.
  On main it fails with "/procs: 404".
- `TestConnectedRunsBeforeTheRehydratesRepack`'s last step now waits out the released repack's commit-graph before
  it checks that a new one starts. Beside the new tests, that commit-graph held the lock long enough to make the check
  flaky.

**Plants, `tests/m0carried/plant17.py`: 10 of 10 caught.**
- no `.keep`;
- the threshold counting kept packs;
- the count looking for the wrong suffix;
- no `--write-midx`;
- four threads;
- no window bound;
- `gitLow` at normal priority;
- the repack through `git` and not `gitLow`;
- `/procs` reading no nice value;
- `/procs` matching `nice` and not `git`.

`--pack-kept-objects` is not a plant: under `--geometric`, git leaves kept packs byte for byte with it too (checked
locally), so it is the same program.

**Design.**
- DESIGN, Shape, `rehydrate`'s text: a promotion's pack loaded through its `.idx` is kept; the repack rolls unkept packs
  only, at nice 19 on 2 threads, with a midx over all; the 24c and 25 measurements.
- The git caches noisy-tenant row names the bound and the kept promotions.
- DESIGN stays at 2,200 lines: the rehydrate paragraph was tightened to make room.
- DECISIONS: "`repack-keep`: promotions a rehydrate loads are kept, and the repack runs low and bounded". It covers the
  decision, why, what it beat (an idle-time repack, no repack, a cgroup cap, a bigger container), the evidence, and the
  flip.

**Drivers** (this branch).
- **The task API pages with `per_page` and `after`.** `tests/fanout/closetasks.mjs OWNER/REPO [--dry]` replaces round
  24c's one-off. It lists every task in a non-final state (queued, in_progress, idle, waiting_for_user), following
  the `Link` header's `rel="next"` URL and its opaque `after` (AGENT-LAYER, "API reference"). A repeated id stops the run.
  It then cancels each task, lists again, and creates and cancels one probe. Round 24c's loop passed `page=N`, which
  the API ignores, so it read the first page every time.
  - The "3,900 tasks" were that loop's misreading, not an API bug.
  - Run `--dry` on b034-mono at 13:44Z: 0 open tasks.
- **b034.sh checks the edge before the row.** It now requires `EDGE_LOG`, the site day's edge deploy log. `tests/fanout/
  edgever.py` reads the log's last "Current Version ID" and checks that `graft-m2b-edge`'s live deployment is that
  version at 100%. It exits 3 when the log names none, as m36's did, and 4 when another version or a split serves.
  b034.sh stops before setup on either. Checked against the logs on hand:
  - m36's log: exit 3, "names no deployed version";
  - m33's log: exit 4, serving `bfed05b9`;
  - m34's log: exit 0, `bfed05b9` at 100%.

**Checks on `repack-keep` (0c171e6d):** fmt; workspace clippy; wasm32 clippy for site and edge, each with and without `test-faults`, and ops; `cargo test --workspace` 977 passed, 0 failed, 3 ignored; `go test` in `containers/git/server` ok; the three `scripts/check_*.py` OK. `crates/raft` is untouched, so no sweep or Porcupine run.

**Cost.** $137.15, unchanged: no hosted spend. The next site day (B0-34, then B0-33) has a $3 cap and waits for the
operator's go once the three branches merge.

### Round 25b (no hosted spend): the next site day's probes for repack memory and kept promotions (2026-11-15)

repack-keep is merged as main 8d79a32f. The site day waits for the composed-miss branch and B0-02's read-index fix,
so one image tests all three. The cap is $3, for B0-34's row then B0-33's.

**(a) Repack memory.** Locally, a kept repack peaked at 5.49 GB. A clone storm on top of it (1.1 GB per depth-1 linux
upload-pack) could still reach standard-4's 12 GiB. Two sources show it:
- `tests/fanout/shardprocs.mjs` (new). Every 15 s from the warm's start to the row's end, b034.sh starts it in the
  background. For each site's shard it reads `/status?repo=` through `/_bed/shard_status`, which starts no container.
  It writes `gitRssMb` (the RSS summed over every running git process), the top four processes (command, nice,
  CPU-s, RSS) and the network's pack counts.
  Output: `tests/fanout/raw/shardprocs-<tag>.jsonl`.
- `tests/fanout/instcpu.py FROM TO net:131296` afterwards: each instance's peak container memory and disk per minute
  (`containersMetricsAdaptiveGroups`), as round 24c read them.
- **The rule on the day.** A shard whose container memory reaches about 11.9 GiB, the cap m36 sat at, is recorded as a
  finding: the minute, the process holding the memory (from `top`) and its RSS, and whether the repack was among
  them. It is not chased during the row. The row's verdict stays against its gate line.

**(b) Every promotion kept.** `/procs?net=N` now also answers `packs: {count, bytes, kept, kept_bytes}` for the
network's repository. The shard object's `/status` passes it on inside `procs`.
- After the cold warm, b034.sh writes "b0-34 kept packs after the warm" with each site's `kept` and `count`.
- Expected: `kept` equals that shard's rehydrate `idx_used`. That was 61 on m36: every promotion on repo:131296,
  8.62 GB, all from before fix 2.
- A later repack leaves `kept` unchanged, because it rolls only unkept packs. So `kept` read at the row's end must
  still be 61.
- This is recorded, not a stop. A shard with fewer kept packs than `idx_used` means a promotion went in through
  `index-pack` (a stored `.idx` rejected), which the rehydrate report's `idx_rejected` also counts.

**Code.** Branch `procs-packs` off main 8d79a32f:
- `handleProcs` takes `net`. `ShardHost::procs` takes the network, and the shard object's `/status` passes it.
- `TestRehydratesRepackLeavesPromotedPacks` now checks `/procs?net=7` reports every promotion as kept, out of the
  repository's pack count.
- `tests/m0carried/plant17.py` gains mutant 10, which counts every pack as kept. It is caught: 11 of 11 now.

**Smoke, read only.** `shardprocs.mjs 131296` at 14:56Z: all three sites answered 200 with `running: false`, and the
sweep found no shard container running. So the probe starts nothing.
### Round 26: m37 site day (main 1aed2cff), cold warm 150 to 158 s, then B0-34's row; B0-33 not run at the stop (2026-11-16)

**Site day.** From a clean worktree at main `1aed2cff` (read-round-gap, composed-miss, promote-unlock, disk-validate,
r22-sweep-fill, the m3f6 instruments).
- New shard image `sha256:a6b6621a09579852953a90f949cedc4846c06ee1763c97c224e8067c4d78b269`, pushed at 23:24:33Z.
- Sites exit 0, test-faults, `SHARD_INSTANCE=standard-4`, no roll.
- Edge `b4968db8`. `edgever.py` exit 0: "graft-m2b-edge serves b4968db8 at 100% (rev 1aed2cff7)".

**Before the warm.**
- Sweep: "no shard container running". `netimages.py 131296 --none`: 0 running.
- `closetasks.mjs m3org/b034-mono`: 0 open tasks, so 0 closed; probe create 201, cancel 200.
- Fix 2 (`fetch-ahead`'s path-named promotions) does not shrink the existing promotions on repo:131296: all 61 predate
  it (round 24c). So this warm measures fix 1 and `repack-keep`, not fix 2.

**Cold warm: fail against DESIGN's 120 s line, at 150 to 158 s; under the 600 s fail value.**
- Every shard ran `sha256:a6b6621a`: `netimages.py --image`: 3 running, 0 on another image.
- ls-refs listed main at 152.4 s. Catch-ups ended at 152.4 s (site c), 158.2 s (site a) and 159.9 s (site b).

| shard | total (`catch_up`) | up | marked phase | tail | `/connected` and follow | stored packs | `idx_used` | spool (summed) | per stream | in total over the marked phase | streams at once | `index-pack` (summed) | lock |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| site c `git#2` | 150.1 s | 36.0 s | 79.4 s | 20.4 s | 12.0 s | 82 (8.64 GB) | 61 | 225.5 s | 38.3 MB/s | 108.8 MB/s | 2.84 | 7.5 s | 30.5 s |
| site a `git#0` | 156.6 s | 33.7 s | 82.8 s | 23.7 s | 14.5 s | 82 | 61 | 246.0 s | 35.1 MB/s | 104.4 MB/s | 2.97 | 9.6 s | 39.3 s |
| site b `git#1` | 157.9 s | 35.7 s | 73.0 s | 31.1 s | 16.4 s | 82 | 61 | 178.2 s | 48.5 MB/s | 118.4 MB/s | 2.44 | 10.7 s | 52.8 s |

How the columns are computed:
- "per stream" is the stored bytes over the summed spool.
- "in total" is the stored bytes over the marked phase.
- "streams at once" is the summed spool over the marked phase.
- Source: the three `shard rehydrate 131296` reports (`rehydrate-131296-m37.txt`).
- Each shard skipped 256 pushed packs that promotions cover, with 0 `.idx` rejected and no backstop fault.

Against round 24 (127 to 135 s) on the same 61 promotions:
- The marked phase grew from 65–71 s to 73–83 s.
- Totals in the R2 rate fell from 122–132 to 104–118 MB/s.
- There are 82 stored packs instead of 70: 21 pushes past the last promotion.
- The ingest path did change between the two days: `repack-keep` writes a `.keep` per stored `.idx`, and
  `ingestSpooled` gained a step. Neither is a fetch step, and the summed `index-pack` (7.5 to 10.7 s) and lock times
  are about round 24's, so the extra 8 to 18 s sits in the marked phase's R2 reads [inference: R2 at 104 to 118 MB/s
  against 122 to 132, with nothing that isolates the R2 side from the shard's].

**(b) Kept promotions.**
- After the warm, every shard had `kept 61` of 100 packs (8.62 GB kept). That equals `idx_used` (61).
- At the row's end:
  - site a: 64 kept, after its own rehydrate at 23:54Z with three newer promotions;
  - sites b and c: 61 kept.
- **The promoted packs were never rolled up.** `kept_bytes` stayed 8.62 to 8.70 GB throughout. The geometric repack
  rolled only the unkept tail; for example, site b went from 1,214 packs to 640 at 23:58:30Z.

**Composed misses (point 2).** The builder is site a's first shard.
- It ran `rev-list --objects` then `pack-objects --window=0 --threads=1` at nice 19 from 23:44:15Z. At 23:44:45Z it had
  29.7 CPU-s and 3.3 GB RSS.
- It put `compose/131296/base-88fac175….pack` (270,488,211 B) and its `.idx` (2,547,140 B) to site a's bucket at
  23:45:20Z, and to site b's at 23:45:29Z. That was **3 min 51 s before the row's first miss** at 23:49:11Z.
- Site c's bucket held no `compose/` object at any point. It is not one of the network's two warm sites, so that is
  by design (`warm_sites`).
- At the row's end each warm bucket held 25 chain objects (274,264,403 B): the base, the head, and parts.
- A second base step ran on site a at 23:59:30 to 00:00:00Z (29.6 CPU-s at nice 19, 3.3 GB). The row went past the
  `future` list's end and moved main to another commit.
- Workers Logs had no `shard compose` error line on any site, 23:41 to 00:20Z.

**Clone-cache misses in the first-byte probe (point 1).**
- 51 misses: **36 composed and 15 stock**.
- The stock reasons:
  - "stock: chain behind HEAD": 9 (23:57:37 to 00:02:22Z, after main moved);
  - "stock: not a ref tip": 6 (00:01:37 to 00:03:37Z, at the hostile phase; the want was a main the driver last saw
    that a later move had replaced).
- Time to first byte on a miss:
  - composed: p50 8.5 s, p99 68.7 s, min 1.3 s;
  - stock: p50 63.6 s, p99 209.7 s.
  - The first 12 misses were all at the warm tip `88fac175` with 0 to 18 s between them. They were all composed, at
    1.4 to 12.0 s.
- **R2 entry within a minute of the cancel: 50 of 51.**
  - Every entry was in site a's bucket, as `clone/131296/<key>/0`. `cloneKeyHex` matches
    `CloneKey::digest`, checked against the Rust function on one want.
  - The 12 first misses share one key, `98859749…`. It was in R2 by 23:49:39 to 23:49:53Z, 22 to 29 s after each
    cancel.
  - The one miss not found within 60 s was a stock "not a ref tip" at 00:03:37Z (key `4f433315…`). Its entry landed
    at 00:06:21Z, 2 min 44 s after the cancel.
- **New finding: every clone-cache hit answered 500.** 414 probes after the key was filled answered 500 with no
  `x-graft-clone-cache` header: 249 at j9, 94 at the kill, 71 at the second phase.
  - The edge's events for them read "level error, outcome ok, status 500, about 0.4 to 1 s wall, 5 to 11 ms CPU", with
    no exception text.
  - The first came at 23:49:50Z, 10 s after the entry landed. They stopped when main moved off the key, and came back
    for each new key.
  - The edge's hit path (`workers/edge/src/lib.rs`, the clone-cache branch, from `917667242`) reads the whole object
    with `.bytes()` and returns `Response::from_bytes`. Each entry is 263 to 271 MB. So the cause is most likely the
    edge isolate's 128 MB limit on a buffered 270 MB body [inference; the event has no message to confirm it].
  - One check to settle it, not run on this budget: one hit with `wrangler tail` open on graft-m2b-edge.
  - DESIGN's clone-cache rows assume hits stream ("Clone cache reads": 40 MB entries at 316 ms read p99). A
    cached depth-1 answer of this network is 6.6 times that size.

**(a) Repack memory.** The memory finding is container memory at the 12 GiB cap, not the repack:
- Container memory reached 11.88 to 11.95 GiB on every shard in 2 to 3 minutes each (23:50 to 23:52Z, 23:58 to
  00:01Z).
- In those minutes `shardprocs` showed the repack at nice 19, plus up to 22 stock `--shallow-file pack-objects` at
  nice 0. Site c at 00:00:00Z: 29.3 GB summed RSS over 22 processes, most of it shared pages counted per process.
- The repack's own `pack-objects` peaked at 9.6 to 10.9 GB RSS (site a 23:58:45Z: 11.0 GB, 120 CPU-s; site b
  23:53:15Z: 10.9 GB, 166 CPU-s).
- So **`repack-keep`'s bound is not enough beside stock upload-packs**. The repack's `pack-objects` alone held
  10.9 GB of the 12 GiB.
  - Round 25's local gate measured 5.49 GB maxrss for the same repack.
  - Two candidate explanations, neither checked: the tail here is 1,214 packs, not 12, or `pack.windowMemory` does not
    bound `pack-objects`' object table [not measured which].
- Site a's shard restarted at 23:54:32Z and rehydrated again. Sites a and b restarted again at 00:14:47 and 00:16:08Z,
  after the row, while `shardprocs` was still polling `/status`.
- Container vCPU peaked at 3.19 to 4.16 per shard (`instcpu-m37.jsonl`).

**B0-34's row.** 500 agents, 900 s from 23:49:11Z, home shard killed at 304.6 s.
- Learners: 8 of 8 caught up to 3,135 in 11.4 s. Leader wasm 25.8 MiB at the start.
- **Memory and terms.**
  - memguard: voter peaks 44.9 / 39.8 / 35.6 MiB, with no 120 MiB stop.
  - `inFlight.admitted` peaked at 21 and `inFlight.admittedBytes` at 248,174 B.
  - terms.mjs: term 2 throughout, leader site 0. One `StepDown { why: CommitErrors { calls: [(1, 14, 3), (2, 14, 3)] },
    to: Some(1) }` was logged at 23:56:31Z on site a, but no term change followed.
- **Commit path.** Leader commit p50 365 ms, p99 1,025 ms (n 7,812). `read:api` 71,770 of 71,920 ok (7 × 503), p99
  12.7 s.
- **Pushes.**
  - human at j9: 68 ok, 77 skipped at the cap, 7 `stale:race`, p99 31.6 s;
  - human at kill and second: p99 46.6 s and 25.6 s;
  - agent at j9: 947 ok, 266 `full`, 4 × "Durable Object is overloaded" 503, p99 23.5 s.
- **Tip moves: a driver bug.** All 60 tip moves at j9 and kill were refused as non-fast-forward. The repository keeps
  main between rounds, and `a1.mjs` started `future` at index 0 every round, so it pushed history main already had
  past.
  - At the second phase 3 of 11 went through, when the index caught up. At the hostile phase all 11 went through, and
    the builder's second base step and the "chain behind HEAD" misses followed.
  - Fixed on this branch: the index starts after main's place in `future`, and the start is logged as a row.
- **k.** repo:131296 stepped to **k 2** (`kMax` 2; "k 2 at 5789" after the row). The other networks stayed at 1.
- **Hostile phase.** Clone TTFB p99 120 to 133 s on each hostile network, with half the probes aborted at 120 s.
  Hostile pushes `skipped: cap` 24 to 32 of 39 to 47.

**Verdict against B0-34's line.** Gate: k in band at most 8, hostile owner held to its pool budget, human push p99 at
most 3 s beyond transfer, clone TTFB p99 at most 1 s. Fail values: k over 8, any Cloudflare API call, the monorepo's k
reduced by another owner, or TTFB p99 over 5 s.
- **Fail.**
  - Clone TTFB p99 8.7 s at j9 (500s included; misses alone 12.0 s), 3.3 s at the kill, 14.7 s at the second phase
    and 217.6 s at the hostile phase.
  - Human push p99 31.6 s at j9.
  - The hits that should carry the TTFB line answer 500.
- **Holds.** No Cloudflare API call (0 files in request paths). k never over 8. The monorepo's k never reduced by
  another owner: 1, then 2.
- **Undecided.** The hostile budget: the hostile networks stayed at k 1, so no ledger refusal was tested.

**B0-33: not run.** The meter read $139.48 at 00:01Z mid-row and $140.19 at 00:09Z after the teardown, past the
$140.15 stop. So B0-02's hosted read-round check has no new trace yet.
- `tests/f74/rounds.py` (new) computes that check from a tsave file. For each voter it counts F74's `CallSent`
  fetches to each other voter per 20 ms bucket, both as fetches and as distinct stamps (turns). It also measures the
  learner read grant, `IndexTaken` to `ResSent` on the leader's clock.
- Round 24's B0-33 trace (`tsave-m36r33`, before read-round-gap) as the baseline:
  - up to 427 to 562 fetches and 3 to 4 turns in one 20 ms bucket to one voter;
  - 14 to 48% of buckets over one fetch;
  - grant p99 3.1 to 4.9 s.

**Raw.** All under `tests/fanout/raw/`:
- the site day's status: `siteday-m37.status`;
- B0-34's driver files: `b034-m37.status`, `b034warm-m37.jsonl`, `memguard-b034m37.jsonl`, `terms-b034m37.jsonl`,
  `a1-b0-34-m37.jsonl` (with `b034.misses` and `b034.missSummary`);
- the probes: `shardprocs-m37.jsonl.xz`, `instcpu-m37.jsonl`;
- the rehydrate reports and R2 listings: `rehydrate-131296-m37.txt`, `compose-131296-m37.jsonl`,
  `clone-131296-m37.jsonl` (the row's end).

Sweeps after the row tore down 26 containers, including net 131304. Two later sweeps after site a's and site b's
post-row restarts both read "sweep: no shard container running".

**Cost.** $140.26 at 00:22Z, containers $54.74. The site day, warm and row cost $2.55, against the $3 cap.

### Round 27 (local, no hosted spend): what held the repack's 10.9 GB, the `--keep-pack` fix, and the cold warm under 120 s (2026-11-16)

**The premise needs one correction first.** Round 26 read "all three shards sat at 11.9 GiB of 12 GiB and restarted"
as the repack and the upload-packs filling memory. The container metrics say otherwise:
- At 23:45 to 23:48Z, before the row, every shard had 0 git processes (`shardprocs`) and a container memory metric of
  9.15 to 9.7 GiB, with 9.6 GB on disk. In m36 the metric read 11.95 GiB with the same 9.6 GB disk. The metric
  follows disk use, so page cache or a memory-backed disk is counted [inference; no swap, and Cloudflare's docs
  say only that an instance out of memory is OOM-killed and restarted].
- Every exit 137 came at teardown or sweep times, not in the row: site c at 00:07:27Z (row end), b at 00:15:59Z,
  a at 00:18:13Z (after the sweeps). Site 0's 23:54:30Z reset is a1's scripted shard kill (t0 + 304.6 s =
  23:54:15.9Z).
- So no restart in the row is shown to be an OOM. `/procs` RSS is statm resident, which counts mapped pack files, so
  the 9.6 to 10.9 GB repack RSS was mostly file-backed pages.

**Repro.** `tests/fanout/repackmem.sh` (new): 33 kept 6-step promotions of B0-34's plan (6.5 GB, with `.keep`s, as a
rehydrate leaves them), then a tail of 21 one-step pushes and 1,200 agent pushes in a1.mjs's shape, one pack each:
1,254 packs. It samples every git process's RssAnon and RssFile from /proc each second. One run at a time (flock);
two overlapping runs in round 26's notes corrupted the fixture, and their failures are discarded.

| case | largest RssAnon | its RssFile | wall | after |
|---|---|---|---|---|
| main's repack, 1,221-pack tail | 2.22 to 2.24 GB | 4.16 GB | 119 to 143 s | 34 packs, 33 kept, connectivity ok |
| main's repack, 12-pack tail | 2.23 GB | 4.06 GB | 130 to 132 s | ok |
| `--keep-pack` per kept pack, 1,221-pack tail | 1.69 to 1.70 GB | 1.45 GB | 68 to 87 s | ok |
| `--keep-pack`, 12-pack tail | 1.69 GB | 1.35 GB | 68 to 74 s | ok |
| `--keep-pack`, `pack.windowMemory=64m` | 1.66 GB | 1.45 GB | 82 s | ok |
| 22 stock depth-1 upload-packs alone | 0.33 to 0.46 GB each, 8.85 GB summed over 44 processes | 29 GB summed (shared) | 284 s | — |
| 22 upload-packs beside the `--keep-pack` repack | 9.13 GB summed over 47 | 29.4 GB summed | 423 s | ok |

(The TAIL=12 runs first set every agent ref without the agent packs, so connectivity failed there; the fixture now
sets main alone for TAIL=12 and the rerun above is clean.)

**Telling the three apart.**
- **Pack count: not it.** A 12-pack tail and a 1,221-pack tail give the same peak, 2.23 against 2.24 GB on main and
  1.69 on both with the fix.
- **The object walk into kept packs: the repack's share.** A geometric repack runs `pack-objects --stdin-packs`. Its
  walk for name-hash hints stops only at packs kept in core (`--keep-pack`, or `^`-excluded), not at a `.keep` on
  disk (git 2.56.0 source, `stdin_packs_include_check` checks `KEPT_PACK_IN_CORE`). So it walks from the tail's
  commits through every promotion's trees.
  - On git/git (`/tmp/kw.sh`, 1 kept base and 33 steps): 205,989 kept-pack accesses against 76,560 with
    `--keep-pack`, and pack-objects' peak RssAnon fell from 224 to 111 MB. Same output, 4,437 objects written.
  - The Go test's 53 kept promotions: 106 tree reads on main, under one per promotion with the fix.
  - At B0-34's scale the fix drops the repack's private memory 0.55 GB (24%), its mapped file pages 2.9× (4.16 to
    1.45 GB), and its wall time about 45%.
- **The rest of the repack's private 1.7 GB is neither the window nor the tail.** A 64 MB window gives 1.66 GB and a
  12-pack tail 1.69 GB. Not attributed further [not measured]; it does not grow with the tail.
- **Concurrent upload-packs are the larger private consumer.** 22 stock depth-1 upload-packs held 8.85 GB private,
  about 0.4 GB each. That path (stock misses beside the composed ones) is the composed-miss thread's.

**The honest bound.** Repack private memory is about 1.7 GB at this network, with about 1.45 GB of mapped kept
pages, and does not grow with the tail. Each stock depth-1 upload-pack adds about 0.4 GB private. 22 concurrent stock
misses are about 9 GB, so the 12 GiB container holds the repack plus roughly 20 to 25 stock misses before the disk's
counted pages, and fewer if the metric's 9.6 GiB of disk counts against the same cap [inference]. So the bound worth
stating is on concurrent stock upload-packs, not on packs per roll: no cap on packs or objects per roll is needed,
since the tail's count did not move the peak.

**Fix (branch `repack-mem`).** `runRepack` passes `--keep-pack=<pack>.pack` for every kept pack
(`containers/git/server/main.go`). Design first: DESIGN, Shape, `rehydrate`; DECISIONS, "`repack-keep-pack`".
- `TestRehydratesRepackLeavesPromotedPacks` now traces pack access during the rehydrate's repack and fails on main:
  "the repack read 106 trees and blobs of 53 kept promotions".
- `tests/m0carried/plant17.py`: three new plants (no `--keep-pack`; naming the `.keep` file instead of the pack;
  only the first kept pack named). Its older plants were updated to the new argument list, and plant 6 (`gitLow` at
  normal priority) was updated to match after composed-miss added a second `gitLow`. 14 of 14 caught.

**Point 4: the cold warm (150 to 158 s against 120 s), and what would bring it under.**
- Phases on m37: up 33.7 to 36.0 s, marked phase 73 to 83 s for 8.64 GB of stored packs at 104 to 118 MB/s in total
  (35 to 49 MB/s a stream, 2.4 to 3.0 streams at once of AHEAD's 4), tail 20 to 31 s, `/connected` and follow 12 to
  16 s.
- **Why fix 2 doesn't reach the 61 old promotions:** it changes how `/promote` builds a pack, and all 61 on
  repo:131296 were promoted before it (round 24c). A committed `Promoted` is never rebuilt, so they stay at about
  3.2 times the pushes they cover.
- **Re-promoting 131296 under fix 2:** about 25% fewer bytes, 8.64 to about 6.46 GB. At 104 to 118 MB/s the marked
  phase falls to about 55 to 62 s, and the total to about 125 to 140 s on m37's other phases. **Not under 120 s alone.**
- **More streams:** the warm ran 2.4 to 3.0 of 4 streams at once at 35 to 49 MB/s each. If each stream keeps its rate
  [guess], 8 streams at about 6 at once would carry about 230 MB/s: 8.64 GB in about 38 s, total about 100 to 120 s.
  Site memory stays bounded per stream (`fetch-ahead` streams to the container), so the cost is R2 reads at once.
  Marginal on its own.
- **Both:** 6.46 GB at about 230 MB/s is about 28 s, total about 95 to 110 s [inference].
- **G9's roll-up at 1.04 times the pushes:** about 2.2 GB, about 20 s at 110 MB/s, total about 90 to 100 s. It costs
  385 s on 4 threads and 8.6 GB resident per roll-up (round 23b), so it needs the memory bound G9 names.
- **The fixed part:** up 33 to 36 s is the platform's start (round 24c), and the tail plus `/connected` is 32 to 47 s,
  which only promotion keeping up (G9) shrinks. With the platform's 34 s, under 120 s needs the stored packs read in
  about 39 to 54 s, so 4 streams at today's 104 to 118 MB/s reach it only below about 4.3 to 5.9 GB of stored packs.
- **Proposal, not built:** re-promote 131296 under fix 2, and raise AHEAD from 4 to 8 streams with a local site-memory
  check at 8 × 207 MB as fix 1's test did at 4. Together about 95 to 110 s. The roll-up stays G9's next lever.

### Round 27b (local, no hosted spend): point 4's site-memory check at 8 × 207 MB, and why neither lever is built (2026-11-16)

The operator's go (after merging `repack-mem` as main 1d90863a): re-promote repo:131296 under fix 2 and raise AHEAD
from 4 to 8, once a local site-memory check at 8 × 207 MB passes as fix 1's did at 4. The check passes. Three other
findings stop both levers short of building, and correct round 27's projection.

**Site memory at 8 × 207 MB: bounded.** `tests/fanout/fetchmem/run.sh` with `SEED=8` (README, "Eight at once"):

| Fetch | Packs at once | Peak workerd RSS | Wall |
|---|---|---|---|
| idle after restart | | 113 MB | |
| streamed | 1 / 4 / 8 × 207 MB | 115 / 118 / 122 MB | 3.46 / 3.48 / 3.46 s |
| streamed, again | 1 / 4 / 8 | 122 / 122 / 123 MB | 3.50 / 3.46 / 3.46 s |
| buffered (control) | 1 / 8 × 207 MB | 537 / 3,961 MB | 3.80 / 6.57 s |

Eight streamed packs hold about what four do (122 against 118 MB), so memory does not stop eight.

**1. Eight `/spool`s at once break the six-connection limit.**
- A Workers invocation has at most six connections waiting for response headers. A seventh is queued until one of
  them gets its headers [doc, https://developers.cloudflare.com/workers/platform/limits/, "Simultaneous open
  connections"].
- The container's `/spool` writes its headers only after the whole body has landed: `handleSpool` calls `jsonOut`
  after `spool` (`containers/git/server/main.go`). So each fetch counts toward the six for its whole length.
- `FETCH_AT_ONCE` is 4 so that the in-order ingest and the next log page keep a connection each. The bed asserts
  `FETCH_AT_ONCE + 2 <= 6` at compile time, and raising it to 8 fails that assertion (`crates/git/tests/catchup.rs`).
  At 8, the ingest and the page would queue behind fetches, and the ingest order is what the marked phase waits on.
- Local workerd does not enforce the six: eight such calls ran at once here (sink peak 8, headers at 1,008 to
  1,010 ms each, scratch check). So the memory table above shows nothing about connections.
- A fix exists but changes the protocol: `/spool` answers 200 at once and writes its JSON at the end of the body. Then
  a fetch no longer counts toward the six. Locally, headers came at 2 to 33 ms instead of 1,003 to 1,010 ms, with the
  bodies unchanged (scratch check). A checksum mismatch would then be read from the body, not the status, so this is
  a container and site change, design first, and a site day.

**2. More streams would likely not raise the total.** Round 27's "8 streams ≈ 230 MB/s" assumed each stream keeps its
rate [guess]. The three warms say the total is close to a ceiling, not to the stream count:
- round 23 (m35), one stream at a time on large packs: 67 to 107 MB/s;
- round 24 (m36), 2.1 to 2.8 streams at once: 122 to 132 MB/s in total, 46 to 62 MB/s each;
- round 26 (m37), 2.4 to 3.0 at once: 104 to 118 MB/s in total, 35 to 49 MB/s each.
- Round 24 already read this as a per-container cap near 130 MB/s [inference]. A Cloudflare engineer on the docs
  issue (cloudflare/cloudflare-docs#23626, October 2025) gave 1 Mbit per milli-vCPU, which is 4 Gbps for standard-4,
  "but I'm not positive" about a lower per-instance cap [unverified]. About 130 MB/s is about 1 Gbps.
- The bed with repo:131296's 61 promoted sizes at 45 MB/s a stream keeps 3.6 to 3.8 fetches in flight on average at
  `FETCH_AT_ONCE` 4 (scratch run of `catchup.rs`'s bed). Hosted it was 2.4 to 3.0, so the four slots are not full now.
  The streams are waiting on the link, not on the cap of four [inference].
- So round 27's 95 to 110 s for "re-promote plus 8 streams" is withdrawn. At about 130 MB/s the marked phase is set by
  the stored bytes alone.

**3. Re-promotion of repo:131296 cannot be done as a data job.**
- A committed `Promoted` range is fixed. `verify` accepts a new `Promoted` only from the last range's `to + 1`
  (`crates/git/src/repo.rs`, `check_promoted`), so entries 1 to 234 cannot be promoted again.
- A rehydrate loads every stored pack with a marker except those a `Compacted` consumed, and nothing writes
  `Compacted` yet (`crates/git/src/catchup.rs`, `skippable`: "the read side of `Compacted.consumed`, which nothing
  writes yet"). So a second, smaller pack for a covered range would load beside the old one and add bytes.
- Deleting the 61 old promoted packs or their markers would not help: pushed packs are skipped only when the promotion
  covering them is loaded (`skippable`). So the warm would load the 256 covered pushed packs instead, thin, through
  `index-pack`, as rounds 18b and 20 did (1,507 to 2,639 s).
- What does work: a writer for `Compacted { consumed, produced }`, which is G9's roll-up mechanism, not a one-off. Or
  a fresh network seeded and drained under main. That costs the seed plus a drain of about 2.2 GB at 0.49 MB/s of
  pushed bytes (round 22: 75 minutes, about $1.6), so about $2 to $3 before the row, over the $4 day with B0-34 and
  B0-33 [inference from round 22's cost].

**What would bring the cold warm under 120 s.** With up at 33 to 36 s (the platform's) and the tail plus
`/connected` at 32 to 47 s, the stored packs must be read in about 39 to 54 s. At 104 to 132 MB/s that is about 4.1
to 7.1 GB of stored packs. repo:131296 holds 8.64 GB.
- Roll-up (G9): about 2.2 GB, about 17 to 21 s at that rate. Total about 85 to 105 s [inference]. It needs the
  `Compacted` writer and the memory bound G9 names (385 s on 4 threads, 8.6 GB resident, round 23b).
- Re-promote under fix 2, if it could be done: about 6.46 GB, 49 to 62 s. Total about 115 to 145 s. Not reliably under.
- Eight streams with the early-headers `/spool`: little or nothing if the link is the cap; unmeasured.

**Decision needed (operator).** Nothing built, AHEAD unchanged, so DESIGN and DECISIONS are unchanged. Options:
(a) build G9's roll-up with the `Compacted` writer, design first, as the one lever that reaches 120 s on this
network; (b) measure the per-container R2 ceiling first on the next site day (one cold warm with `FETCH_AT_ONCE` 4,
reading each stream's bytes and times, already in the rehydrate report), and decide after; (c) accept 150 to 158 s
on this network as a G9 finding and run the site day without warm work.

### Budget change (2026-11-16, about 01:58Z)

The user raised LM-1's stop from $145 to $155, relayed by the operator thread at about 01:58Z. The $155 is only for
B0-33:
- The next m2b site day is unchanged: B0-34 then B0-33, once the repack fix and the composed-miss fix are merged, with
  a $4 cap and a stop at $144.50 on the meter.
- If B0-33 does not run that day, or runs and fails with a cause that is named and fixed, one more site day for B0-33
  alone may run up to $155, stopping at $154.50. The cause and the fix go to the operator before that day.
- The $155 does not cover B0-34 reruns or other rows; those need the operator's approval first.

### Round 28 (no site day): the meter passed the stop while idle; promote-ahead kept old repositories' shards running (2026-11-16)

The operator's go for the m2b day on main 5e1227d9 (B0-34 then B0-33, $4 cap, stop at $144.50) did not run. Before
the site day the meter read **$149.42** (`cost.sh` at about 06:00Z), against $140.26 at round 26's end (00:22Z), with
nothing hosted run since. Containers billed **$7.62** of that, by `containers.py` from 00:22Z.

**What ran.** `instcpu.py` from 00:22Z: 19 shard containers running at about 06:00Z, every one on a repository no run
had touched since November 15:

| started | network | shards |
|---|---|---|
| 00:18–00:21Z | 131304 (`fm21-b029`) | 3 |
| 00:35–00:36Z | 65536 (m4a's) | 3 |
| 00:49–00:52Z | 131306 (`fm21-b031`) | 3 |
| 01:19Z | 131307 (`fm21-b033`) | 1 |
| 02:19–02:20Z | 131312 (`fm22-b031`) | 2 |
| 02:51Z | 131315 (`fm22-l012`) | 1 |
| 05:04–05:05Z | 131323 (`fm24s33c-b033`) | 3 |
| 05:50–05:52Z | 131329 (`fm25s33-b033`) | 3 |

**What started them [measured in Workers Logs, mechanism read from the code].** Every one starts the same way. On
131329: at 05:50:15Z a member of the group (woken by a read) logs `shard warm 131329 site 0/1/2`. Each warm's
`/catchup?warm=1` answers in 11 to 22 ms, so it started nothing (`Caller::Warm` on a stopped shard is `Skip`). Then the
leader's archive alarm sends promote-ahead's `/promote?wait=60&next=1` to site a's shard at 05:50:16Z. `/promote` is
`Caller::Client` (`workers/site/src/shard.rs`: only `/catchup` with `warm` is a warm caller), so it starts the
container and records a request (`Serve::Run { touch: true }`), which keeps it warm `WARM_FOR`, 24 h. The shard
rehydrated (`shard rehydrate 131329` at 05:51:48Z), and later warms then found it running. 131304, 131306, 131323 and
65536 show the same `/promote` 503 after 30 to 60 s, then the rehydrate. These repositories hold an unpromoted tail
above `LOG_MAX_BYTES / PROMOTE_AHEAD`, so each wake of their group runs promote-ahead and starts a shard for a day.
The idle-shards fixes (`tests/am1/results/idle-shards.md`) made `warm` start nothing, but promote-ahead's `/promote`
came later (stored-fetch, round 20) and was never put under that rule [inference: no idle-hours check ran after it].

**Done.** `teardown.py --sweep` at about 06:03Z tore down all 19 (`no shard container running` after). Reapers
(`tests/fanout/reaper.sh`, orb services `reaper-a` over 131290–131345 and `reaper-b` over 65536) tear down every 60 s;
none had to by 06:10Z.

**Not run:** the site day, B0-34, B0-33. The instruments for them are on branch `r2-ceiling` (below), not merged.

**Instruments ready (branch `r2-ceiling`, off main 5e1227d9).**
- `StoredMs.spools`: each stored pack's `[start, end, bytes, site]` in the rehydrate report, at most 512, so the
  per-container R2 ceiling comes from the report: streams busy over time, MB/s per stream and in total
  (`crates/git/src/catchup.rs`, `crates/git/src/host.rs`). `tests/fanout/spools.py` reads it from `b034warm`.
- The second-container probe: `/_bed/catchup_pool?site=&pool=&repo=` on the test edge (`/catchup?warm=pool` on a hot
  pool shard, which starts its container), run by `b034warm.mjs` with `POOL_PROBE=2:7` beside the cold warm and torn
  down by `b034.sh` after. About $0.04 (one standard-4 for about 5 minutes).
- Per miss: `tests/fanout/misses.py` joins each a1 probe row to the miss-log branch's `shard miss <net> <key>` line by
  the CloneKey digest and splits its first byte into container, shard transit, and edge plus network; `b034.sh` runs it
  after the row.
- `shardprocs.mjs` records `uploads` (stock upload-packs at once) and `uploadRssMb` per shard, for comparison with
  `instcpu.py`'s container memory at about 0.4 GB each.
- G9's row records the `/spool` early-headers option, to build only if the ceiling allows more streams.
- Full checks: fmt, workspace clippy, wasm32 clippy for site and edge (with and without test-faults) and ops,
  `cargo test --workspace` 1,009 passed and 0 failed, `go test`, `check_design`.

### Round 29: B0-33 alone on the deployed m37 sites and edge (no site day), under the $155 approval: fail, terms 2 to 15 (2026-11-16)

**Run** `tests/fanout/b033-r29.sh r29` as one orb service, reapers a and b running throughout.
- Edge check before the row: graft-m2b-edge serves b4968db8, m37's (deployments API). Sites a 2cf0b9f7, b 25a747eb,
  c 9c45528d, so read-round-gap (B0-02) is deployed.
- Meter $149.69 before the row, expected +$1.5 against the $155 stop; read every 60 s, never near $154.50.
- Repo `fr29-b033` (131342), run `mwivq5ms`, 8 learners, k 4 granted at 06:25:24Z, tasks 100 of 100.
- Load 06:30:42Z to about 06:35:45Z (300 s, 5,000 agents), a1 exit 0. a1's learner wait read "0/8 in 122402 ms"
  against commit 116, the same moving-commit read as rounds 24 and 24b.
- Teardown: the row's 6 shards torn down (200 each), then "sweep: no shard container running".

**Memory: held.** 469 memguard samples (42 member reads unanswered), voter peaks 50.0 / 48.9 / 53.4 MiB.
`inFlight` at most `admitted` 681 / 623 / 479 and `admittedBytes` 3.38 / 4.23 / 3.37 MB.

**Leaders: churn, term 2 to 15.** terms.mjs: term 2 until 06:31:05Z, then 3 (site 2) 06:31:15, 4 (site 1), 5 (site 0)
06:31:49, and so on to 15 (site 0) at 06:35:39Z, which held to the end. Step-downs in Workers Logs (`member alarm:
StepDown`, `stepdowns-r29.txt`), 12 in all:

| reason | site a | site b | site c |
|---|---|---|---|
| `CommitErrors` | 2 | 7 | 1 |
| `Confirm` (p99 1,633 and 1,314 ms) | 2 (06:36:42, 06:36:54Z, after the load) | 0 | 0 |

The `CommitErrors` windows name one follower with most calls failed, for example site a at 06:33:35Z
`(SiteId(1), 137, 119)` and site b at 06:34:38Z `(SiteId(2), 61, 58)`. Terms rose about three per step-down, so
followers' elections without a step-down made the rest.

**B0-02's hosted check (`tests/f74/rounds.py`, 06:30:42Z to 06:35:45Z, `rounds-r29.json`).**

| voter → voter | fetches | max per 20 ms | turns max | buckets over 1 |
|---|---|---|---|---|
| 0 → 1 / 0 → 2 | 2,229 / 2,502 | 84 / 92 | 3 / 2 | 44% / 55% |
| 1 → 0 / 1 → 2 | 1,855 / 1,971 | 50 / 76 | 3 / 3 | 45% / 48% |
| 2 → 0 / 2 → 1 | 1,285 / 1,145 | 40 / 39 | 2 / 3 | 32% / 34% |

- Read grant (learner `IndexTaken` to `ResSent`, on the serving leader): p99 2,313 / 2,853 / 2,413 ms on sites 0 / 1 /
  2, max 4,742 ms; p50 0 to 25 ms.
- Against round 24b's baseline (before read-round-gap): max per bucket 427 to 562 down to 39 to 92, turns 3 to 4
  down to 2 to 3, grant p99 3.1 to 4.9 s down to 2.3 to 2.9 s. Buckets over one fetch went from 14 to 48% to 32 to
  55%. `rounds.py` counts every `/raft` fetch, appends and votes included, so a bucket over 1 is a candidate breach,
  not a proven one; turns over 1 per bucket stayed at 11 to 30 per pair in the window.

**The trace (`calls.py`, same window, `calls-r29.{txt,tsv.xz}`).**

| calls | sent | reached the callee | answered |
|---|---|---|---|
| learner → voter (read-index) | 17,642 | 15,145 | 14,820 |
| voter → voter (raft) | 10,987 | 9,604 | 9,476 |
| voter → learner (append) | 3,038 | 3,026 | 2,894 |

- Learners' calls are down from 270,129 (round 24b) to 17,642, 15 times fewer, and are 56% of member calls, not 94%.
  At most 349 member calls in one second (p50 94), against up to 14,325.
- The voter legs are still slow. Voter → voter `out` p50 1.7 to 3.2 s and p99 8.9 to 23.7 s; `work` p50 0 ms and p99
  at most 2.2 s. 24,537 calls missed the 1,000 ms deadline or went unanswered; 3,868 never reached the callee. Callers'
  loop lag at send was under 100 ms for 27,401 of 31,667 calls.
- The leader at the end (term 15, site 0) counted 503 requests a second inbound: `/read` 114,236, `/suspect` 46,384,
  `/commit` 26,332, `/raft` 12,914, `/outcome_by_request` 5,294, `/campaign` 3,558 [measured, `leader.inbound`; the
  counts are since that member's start, not the window alone].
- **Reading [inference, not proven]:** read-round-gap removed the learner fetch storm, but the voters' raft calls
  still spend seconds in the legs, not the handler, so heartbeats miss the deadline and `CommitErrors` steps leaders
  down. What is left ahead of them is the client load on the voters (edge `/read`s, `/commit`s, and `/suspect`
  reports from the elections themselves). This is F74's question and B0-02's, and the trace went to B0-02.

**Row numbers.**
- Offered: 252.2 agent-class group commands a second with host-made (41.7 agent pushes a second, mean pack 90 KB).
- Agent-class requests ok over 300 s: 3,679, 12.3 a second at the client (pushes 2,265 of 12,316; stand-ins 420;
  comments 325; lease renews 236; task edits 249; leases 118; draft PRs 66; labels 0).
- Agent pushes' refusals: 4,799 "access rows did not answer", 2,578 "created, but no member leads", 904 `full` (the
  `ng` shape), 517 git cache 503s, 387 `no outcome ... missed`.
- Leader commit p50 4,274 ms, p99 15,106 ms (n 6,688). Merges 0 ok of 49 (30 skipped at the cap). Human pushes 8 ok
  of 193 (163 skipped at the cap); GraphQL human writes 62 of 470; the fast user 36 of 476 × 200.
- Entries 84, commands 2,801 on the end leader.

| B0-33 line (`build/BUILD-PLAN.md`) | r29 (m37) | verdict |
|---|---|---|
| Human and merge-queue commit p99 ≤ 1 s | leader commit p99 15,106 ms; merges 0 of 19 attempted; human pushes 8 of 30 attempted | fail |
| ≥ 140 agent commands/s admitted (fail < 100) | 12.3 agent-class ok a second at the client | fail |
| User token admitted as `Agent` | 36 of 476 fast-user requests 200, 440 × 503 | not shown |
| Every refusal in the `ng` or 429 shape | 503s and 500s dominate | fail |
| Zero leader changes and `suspect` | term 2 → 15; 12 step-downs; `/suspect` 46,384 on the end leader | fail |

**Raw.** Under `tests/fanout/raw/`: `rows-r29.status`, `r29-r29.status`, `memguard-r29.jsonl`, `terms-r29.jsonl`,
`tsave-r29.jsonl.xz` (12.4 MB raw), `calls-r29.txt`, `calls-r29.tsv.xz`, `rounds-r29.json`, `stepdowns-r29.txt`, and
`a1-b0-33-r29.jsonl` (a copy of `tests/am1/raw/a1-b0-33-mwivq5ms.jsonl`). The driver is `tests/fanout/b033-r29.sh`.

**Cost.** $151.09 at 06:40Z after the sweep, from $149.69 before: the row cost $1.40, against $1.5 expected and the
$155 stop. $3.91 of the $155 approval is left.

### Round 30 (local, no hosted spend): `promote-warm`, promote-ahead starts no stopped shard (2026-11-16)

Round 28's cause: the archive alarm's promote-ahead sent `/promote` as `Caller::Client`. Each old group woken with an
unpromoted tail above the bound started its stopped shard and kept it warm 24 h. Branch `promote-warm`, off main
b79a2b26, fixes it at the caller, design first (DESIGN, Shape, `Promoted`; DECISIONS, "`promote-warm`").
- `log_archive::step` passes `warm` to `ArchiveHost::promote` for promote-ahead (no range due), and treats a promotion
  that every shard skipped as `Step::Idle`, so the same range promotes on a later alarm once a shard runs. A due
  range's promotion is still a client's call and starts a shard, since archiving waits on it.
- The site's promotion (`workers/site/src/archive.rs`) adds `warm=1` to every shard call of a warm promotion: `/promote`,
  its `/validate` and `/idx`. A shard's `skipped` answer now carries `x-graft-skipped: stopped`, which `shard_call` reads
  as `SKIPPED`, so the skip is not mistaken for `/promote`'s 200 pack.
- The shard reads `warm` on any route as a warm caller (`graftyard_git::caller`): `serve(Warm, false)` is `Skip`, and
  `serve(Warm, true)` runs without a touch. `warm=pool` stays the pool warm, on `/catchup` only.

**Tests and plants.**
- `promote_ahead_starts_no_stopped_shard_and_a_due_range_does` (`crates/git/tests/log_archive.rs`): on a stopped shard,
  promote-ahead asks warm, records nothing and starts nothing; once the shard runs, the same range promotes from entry
  1. A due range over a small cap asks as a client and starts the shard. It fails on main, where `promote` takes no
  `warm`.
- `a_warm_query_on_any_route_is_a_warm_call_and_pool_only_on_catchup` (`crates/git/tests/shard.rs`) fails on main, where
  only `/catchup` read `warm`.
- `tests/promotewarm/plant.py`: 6 of 6 caught (promote-ahead as a client, every promotion warm, a skip as an error,
  `warm` only on `/catchup`, the pool warm on every route, a call without `warm` as warm).

**Checks.** fmt, workspace clippy, wasm32 clippy for site and edge (with and without test-faults) and ops, `cargo test
--workspace` 1,011 passed and 0 failed, `go test`, and the three `scripts/check_*.py` OK; DESIGN at 2,200 lines.

**Not yet:** deployed. It ships with the next m2b site day; reapers a and b stay on until then.

### Round 31 (local, no hosted spend): `registry-restore`, a re-placement's tries no longer leave the site's registry row (2026-11-16)

B0-02's analysis of round 29: during `repo:131342`'s create placement, `replace_on` joined tries `#4#f1` to `#9#f1` on
site b. Each `/join` wrote its registry row before the placement check, and `PUT /row` refused only a lower generation,
so `#9#f1`, which never went in, stayed the row over the voter `#3#f1`. The edge's registry stage read the row about
61,000 times per site in 303 s and got 63,387 `/read` 409s from `#9#f1`.

Branch `registry-restore`, off main 112ee41e, design first (DESIGN, Shape, `MemberRegistry`; DECISIONS,
"`registry-restore`").
- `replace_on` records every try it joins. When it ends, by `Normal(p)` with the old member or `Normal(np)` with a
  try, it sends `RowPut { row: kept, replacing: other tries }` to that site's registry (`restore_row`, best effort,
  logged on failure).
- `PUT /row` takes a bare `RegistryRow` (a member's own write, as before: never a lower generation) or a `RowPut`, which
  replaces the stored row only when it names one of the tries (`RowPut::replaces`).
- Compare-and-set by name, not a winning generation: the kept member keeps its own generation, which the readers that
  rebuild records from a row (`registry_record`, `fold_audit`, `activity::rebuild`) rely on.
- Edge: no change. A 409 at a registry-step name is already `Skip` (no `/suspect`, `classify`), and `tried` drops the
  name for the rest of the pass.
- Mixed versions: a site script before this change answers a `RowPut` with an error (its `PUT /row` reads a bare row),
  which `restore_row` logs; the row stays as on main until both sites run it.

**Test and plants.** `a_placement_that_kept_its_member_puts_its_row_back_over_every_try` (`crates/types/src/registry.rs`)
fails on main. `tests/registryrestore/plant.py` 8 of 8 caught: a restore over any row, a restore under the generation
rule (main's), an own write moving the row back, a restore creating a row, the kept member among those replaced, an
empty restore sent, the sites lost, and the newest try's generation on the restored row.

**Checks.** fmt, workspace clippy, wasm32 clippy for site and edge (with and without test-faults) and ops, `cargo test
--workspace` 1,013 passed and 0 failed, `go test`, and the three `scripts/check_*.py` OK; DESIGN at 2,200 lines.

**Not yet:** deployed. It ships with the next m2b site day.

### Round 32 (local, no hosted spend): G9's roll-up, design only, with its memory bound measured (2026-11-16)

The operator asked for G9's roll-up as a design for review, with no code: a `Compacted` writer, the memory bound,
rehydrate preferring a rolled pack, and deleting consumed packs only after every shard has moved past them. The
design is in DECISIONS, "`roll-up` (proposed, not built)", and in short form in DESIGN, Shape, `rehydrate`.

**Measured: bounded rolls of B0-34's promotions** (`tests/fanout/rollup/rollk.sh`, `level2.sh`, `validate.sh` on round
27's fixture `~/geo-T`: the base pack plus 33 complete 6-step promotions of B0-34's plan, 6.80 GB, each kept). Each roll
is `pack-objects --stdin-packs --delta-base-offset` of exactly the first K packs, at nice 19, `pack.threads=2`,
`pack.windowMemory=256m`, on the orb's 4 CPUs. Raw output is in `tests/fanout/raw/rollup-r32.txt`.

| K | in | out | out/in | wall | CPU | maxrss | peak RssAnon |
|---|---|---|---|---|---|---|---|
| 2 | 422 MB | 309 MB | 0.73 | 23.6 s | 31.0 s | 838 MB | 398 MB |
| 4 | 801 MB | 514 MB | 0.64 | 39.8 s | 57.9 s | 1,467 MB | 637 MB |
| 8 | 1,680 MB | 870 MB | 0.52 | 83.7 s | 123.5 s | 2,816 MB | 1,093 MB |
| 16 | 3,516 MB | 1,482 MB | 0.42 | 175.9 s | 263.2 s | 5,721 MB | 2,111 MB |
| 34 (all) | 6,802 MB | none, killed (rc 128) | | 301.6 s | 480.9 s | 10,450 MB | 3,535 MB |

- **Memory scales with the input:** maxrss about 1.6 to 1.75 times the bytes rolled and private memory about 0.5 to 0.6
  times. So the bound is a cap on input bytes per roll. ROLL_IN_MAX 2 GiB keeps a roll near 3.6 GB RSS.
- **The whole-network roll does not fit.** All 34 reached 10.4 GB maxrss and was killed, on the 15 GB orb with its
  work directory on a tmpfs. A rerun on disk was stopped after it stalled the orb's executor; no output was kept.
- **Second level:** two 8-rolls (1,680 MB to 870 MB and 1,836 MB to 750 MB, 92 and 118 s, 2.8 and 4.2 GB maxrss), then
  those two rolled together: 1,620 MB to 1,496 MB, 0.92, in 99 s, 117 CPU-s, 3.7 GB. A direct 16-roll gave 1,482 MB.
  So re-rolling buys about 8 %, and the design never rolls a rolled pack.
- **Validate of a roll:** `index-pack --threads=2` at nice 19 of the 870 MB 8-roll: 92.9 s, 162 CPU-s, 1.37 GB maxrss.
- **Objects kept:** the 4-roll's `.idx` lists the same 1,540,334 objects as its four inputs (`comm` of `show-index`:
  0 differ), and `index-pack --verify` passes.

**B0-34's cold warm with roll-up [inference].** repo:131296's 61 promotions (8.64 GB) in rolls of about 2 GiB in (about
8 promotions each) at 0.41 to 0.52 come to about 3.5 to 4.5 GB. At round 26's 104 to 132 MB/s that is 27 to 43 s of
stored reads, inside round 27b's 39 to 54 s window. With up at 33.7 to 36.0 s, the tail at 20.4 to 31.1 s and
`/connected` at 12.0 to 16.4 s, the total is about 100 to 127 s at 0.52, or 93 to 120 s at 0.41, against 120 s. So the
roll-up reaches 120 s at the upper half of the R2 rate, not reliably at the lower. Round 23b's 1.04-times-pushes figure
was a whole-network `repack -a -d`, which bounded rolls do not reach.

**Cost of the rolls themselves:** about 8 rolls of about 3 minutes and 290 CPU-s each (roll plus validate) on the home
shard, at nice 19 and in the repack's slot. That is about 40 CPU-minutes for the network's history once, then one roll
per 2 GiB of new promotions.

**Docs on this branch (`rollup-design`, off main 87c2cd23):**
- DESIGN, Shape, `rehydrate`: rolled packs load by `.idx` like promotions, plus the roll-up's short form. DESIGN stays at
  2,200 lines.
- DECISIONS: "`roll-up` (proposed, not built)", with the contradiction against SHARED's network-chain `Compacted`.
- DECISIONS, "`registry-restore`", Flip: the late-join gap.
- OPEN-QUESTIONS G19 (opened as G12, renumbered after b030-split): `move_step`'s `Swap` and the late join, with B0-30's `repo:1048590#9#f1`.

### Budget change (2026-11-16, about 13:40Z)

The user raised LM-1's stop from $155 to **$160** (about 13:40Z, relayed by the parent thread). Scope: one `graft-m2b`
site day, then B0-34, then B0-33, with a $160 stop on the meter and costok-style checks before each phase and between
rows. Nothing else.

### Round 33 (no site day): the meter read $178.42 before the day; promote-ahead started 34 idle shards outside the reapers' ranges (2026-11-16)

**Not run: the site day, B0-34 and B0-33.** Before any step, `cost.sh` (T0 `mem7-t0`) read **$178.42** at about
14:05Z, against $151.09 at round 29's end (06:40Z) and the new $160 stop. Nothing hosted ran in between.

**Where it went** (`m2bcost.py`, 06:40Z to 14:20Z): **$27.66**, of which containers **$19.36**
(`containers.py`), 14,376 container CPU-s, 1,443 Worker requests and 871,899 DO requests. By window: 06:40–10:00Z
$8.07 ($3.92 containers), 10:00–12:00Z $10.07 ($7.31), 12:00–14:10Z $9.20 ($7.82), so containers billed about
$3.6 an hour from 10:00Z.

**What ran.** At 14:15Z, 35 shard containers were running (site a 18, b 11, c 6), on networks 131226 to 131272, 131289
and 131330, started from 07:21Z on. Reapers `reaper-a` (131290–131341) and `reaper-b` (65536) cover none of
131226–131289, so nothing tore those down. 131330 was in reaper-a's range and was torn down again each minute
(`reaper.log`, 13:59–14:09Z).

**What started them [measured on 131258, mechanism as round 28's].** `shardtimeline.py 131258` at 09:20–09:24Z: each
site's `/catchup` answers 200 (the warm, which starts nothing), then site a's `/promote` (503 at 09:22Z) and site b's
(09:23Z). That is promote-ahead's `/promote` as a client call, which starts a stopped container and touches it for
`WARM_FOR`. The fix (`promote-warm`, main 112ee41e) is merged but not deployed: the deployed sites are still m37's, and
it ships on this site day.

**Done.** `teardown.py --sweep` at about 14:16Z tore down 33 (all 200), then "sweep: no shard container running".
`reaper-b` now covers 65536, 131072–131289 and 131342–131400 as well as before, so every older network is reaped
until the site day deploys `promote-warm`. The next 60 s window billed $0.00.

### Budget change (2026-11-16, about 14:30Z)

The user raised LM-1's stop from $160 to **$185** (about 14:30Z, relayed by the parent thread), after round 33's
meter read $178.77 at 14:16Z with nothing running. Same scope as the $160 approval: one `graft-m2b` site day from main
a0bca583 or later, then the 30-minute idle-start check with the reapers on, then B0-34, then B0-33 with the inbound
trace, with a $185 stop on the meter and costok-style checks before each phase and between rows. B0-33 is skipped if
the meter is near $185 before it starts. Nothing else.

### Round 34: m39 site day, the idle-start check, B0-34, B0-33 with the inbound trace (2026-11-16)

**Site day.** From a clean worktree at main `a0bca583`: promote-warm, registry-restore, push-reads, feed-ahead,
receipt-rule, the test-faults inbound trace, and b030-split's site code.
- Shard image `sha256:a476cbbf63971df7c0dae756c65dfc0bd5f35b55c18f6353cc3263d38a2352a7`, pushed 14:31:55Z.
- Sites, test-faults, exit 0 at 14:43:05Z: a `167fa92e`, b `96c1850e`, c `65016766`. `--roll-shards` with no shard
  running.
- Edge `f14188bc` (formats as m37's: none changed since). `edgever.py`: "graft-m2b-edge serves f14188bc at 100% (rev
  a0bca583a)".
- **`SHARD_INSTANCE` was not set** (`fanout-m39-siteday.log`: "SHARD_INSTANCE=unset"). `run-siteday.sh` never sets
  it, while m37's day ran with `SHARD_INSTANCE=standard-4` by hand. So every shard on m39 is `standard-2` (the
  50-developer default, `workers/site/src/shard.rs`, `DEFAULT_INSTANCE`): about 1 vCPU and 6 GiB. That is my
  mistake. It changes B0-34's cold warm and stock-pack numbers below, and it is why the git CPU sat at 1.05 vCPU.
  `run-siteday.sh` now defaults `SHARD_INSTANCE=standard-4`.

**Idle-start check (30 minutes, reapers on), `idlewatch.py`: 5 starts, all repo:131330, none after 14:47Z.**
- 14:44:07 to 14:46:27Z, `git#{0,1,2}#net:131330` started; the reapers tore each down within a minute.
- Workers Logs: site a's leader of repo:131330 ran an archive step at 14:43 and 14:45Z: "promotion Some((8, 406)):
  283 packs" over `/promote?wait=10&next=1` and `/promote?wait=60` **without `warm=1`**: a due range (`Next::Promote`),
  which starts a shard by design ("a due range starts one, since the log cannot archive past it otherwise",
  `crate::log_archive::step_in`). Both failed (503 `warming`, then "Network connection lost" at the reaper's
  teardown). At 14:53 and 14:54Z the same leader's promote-ahead calls carried `warm=1` and answered 200 skipped
  in 0 to 16 ms, starting nothing.
- So promote-warm holds: promote-ahead started no shard. What started 131330 is a range due to archive on an old
  repository, which only a reaper hides: each reaper teardown cut the promotion, so the range stays due and the next
  alarm starts the shards again. No start after 14:47Z: its leader's `SUM_EVERY_MS` retry found the shards gone and
  the reaper-cut steps back off a minute each.
- **Reapers stopped at 15:15Z** after the check: reaper-b, and reaper-a's 131290–131341 range. reaper-a now covers
  only 131330 (`tests/fanout/reaper.sh 131330 131330`), until that repository's due range has promoted once without a
  teardown, or is decided otherwise.

**B0-34 (driver `tests/fanout/costrow.sh`, ROW=b034, POOL_PROBE=2:7), a1 run `mwjex2b9`, 15:28:39Z to 15:43:39Z.**
- First try stopped at setup at 15:15:59Z (the day38 worktree lacked the untracked `tests/m0/.m0-token`, linked
  then); no spend.

Cold warm (all three shards rehydrated, standard-2): **250.9 to 332 s** to ls-refs-capable, main at 292.3 s.
- From the three `shard rehydrate 131296` reports (`rehydrate-131296-m39.txt`): 253 stored packs (8.87 GB, 70 through
  their `.idx`), 471 pushed packs skipped, 0 `.idx` rejected, no backstop fault. Phases: up 31.9 to 34.1 s, marked
  phase 110 to 167 s, tail 56 to 70 s, catch-up 46 to 56 s.
- 253 stored packs where m37 had 82: 171 more pushes past the last promotion since m37, none promoted yet.

Per-container R2 ceiling (`spools.py` over the reports, `spools-m39.txt`):

| container | bytes | span | total | streams busy mean / most | peak 5 s bin | per stream, bytes-weighted |
|---|---|---|---|---|---|---|
| site a `git#0` | 8.87 GB | 230.1 s | 38.6 MB/s | 2.15 / 4 | 101.3 MB/s | 17.9 MB/s |
| site b `git#1` | 8.87 GB | 157.7 s | 56.3 MB/s | 1.25 / 4 | 163.7 MB/s | 45.1 MB/s |
| site c `git#2` | 8.87 GB | 186.0 s | 47.7 MB/s | 1.45 / 4 | 144.4 MB/s | 32.8 MB/s |
| site c `pool:7` (probe) | 8.87 GB | 246.8 s | 35.9 MB/s | 1.63 / 4 | 106.7 MB/s | 22.0 MB/s |

- **Second-container probe** (site c, pool slot 7, same packs at once): in the 22 bins both were busy, `git#2` 74.8
  plus `pool:7` 52.5 = **127.3 MB/s, against `git#2` alone 31.2 MB/s** in those bins. The sum is well above either
  alone, so the ceiling here is per container (its link or its CPU), not per site or per bucket [measured, one
  probe]. Probe cost: one standard-2 for about 5 minutes, under $0.02.
- The peak bins (101 to 164 MB/s) match m37's 104 to 118 MB/s summed, but on standard-2 the bins fall to 30 to 60
  MB/s once ingest starts, with the container at 1.0 to 1.05 vCPU every minute (`instcpu-m39.jsonl`): the per-container
  limit on m39 is CPU, not R2 [inference: vCPU pinned at the instance's 1 while the rate fell].
- Container memory 5.9 to 6.0 GiB from the first minute (the standard-2 cap); all three later ended `failed` exit 137.

Row, a1 `mwjex2b9` (500 agents, 900 s; kill at 300 s, second network at 420 s, hostile at 600 s):
- Site 0 led at term 2 throughout; leader commit p50 273 ms, p99 633 ms (n 2,721).
- k: repo:131296 2 throughout (`kMax` 2); the second network and the hostile three 1. k never over 8.
- **Clone-cache hits stream, no 500s**: 34 hits, all 200 (kill 4, second 17, hostile 13), p50 0.7 to 4.2 s, p99 4.4
  to 9.1 s.
- Misses: 461 probes, every one joined to its `shard miss` line (`misses.py`: 461 joined, 0 without a line, 2 lines
  without a probe). Answers: 458 composed, 1 "stock: no chain", 2 cancelled with no answer. Composed misses' first
  byte p50 4.4 s, p99 91.5 s. Split: to the shard p50 2.2 s, p99 9.7 s; container (`headers_ms` less the step before)
  p50 1.3 s, p99 86.1 s; shard transit p50 0 ms, p99 26 ms; edge plus network p99 21.5 s. The p99 sits in the
  container on standard-2 (`misses-m39.txt`, raw `misslines-m39.jsonl`).
- **Stock upload-packs per shard against memory** (`shardprocs-m39.jsonl`): site a 74 at once at 15:29:28Z (159 MB
  summed RSS, 4.69 GB of git RSS with the commit-graph write's 2.9 GB); site b 54 at 15:36:58Z (114 MB); site c at
  most 1 (a 1.3 to 2.9 GB `pack-objects` at 15:43 to 15:45Z). The burst counts are `upload-pack --stateless-rpc`
  processes of the row's clones and ref reads, about 2 MB each, with their `pack-objects` not yet started; the row
  made one stock miss ("stock: no chain"), so the 0.4 GB each measured for stock depth-1 misses did not come up.
  Container memory sat at the standard-2 cap (5.9 to 6.0 GiB) throughout.

| B0-34 line (`build/BUILD-PLAN.md`) | m39 | verdict |
|---|---|---|
| k set in band from `NetworkLoad` and the hot pool, ≤ 8 per site | monorepo k 2; others 1; `kMax` 2 | pass on ≤ 8; k stepped once |
| The hostile owner held to its pool budget; the monorepo's k not reduced by another owner | hostile k 1; monorepo 2 throughout | holds |
| Human push p99 ≤ 3 s beyond transfer | p99 75.8 s (j9), 32.2 s (kill), 22.2 s (second), 92.1 s (hostile) | fail |
| Clone TTFB p99 ≤ 1 s throughout (fail over 5 s) | p99 30.0 s (j9), 6.4 s (kill), 16.9 s (second), 93.8 s (hostile) | fail |
| No Cloudflare API call from a request path | 0 files | pass |

Cold warm against DESIGN's 120 s line: **fail at 250.9 to 332 s** on standard-2 (m37: 150 to 158 s on standard-4).
Not comparable to m37 on the git side, because of the instance type.

**Roll-up cold-warm estimate with this ceiling.** The ceiling is per container, so the roll-up's estimate keeps standard-4's 104 to 132 MB/s (rounds 24 and 26): 61 promotions (8.64 GB) rolled at 0.41 to 0.52 are 3.5 to 4.5 GB, a marked phase of 27 to 43 s, and a total of 93 to 126 s with m37's other phases (up 33.7 to 36.0 s, the rest 32 to 47 s) [inference]. Since m37, 171 more pushes sit above the last promotion (253 stored packs against 82), so promote-ahead's work comes first on this network.

**B0-33 (ROW=b033, tsave EVERY=10000, LEARNERS=1), a1 run `mwjghnzg`, repo:131343, 16:06:40Z to 16:17:18Z.**
- First try: rows.sh refused to start ("commit the driver changes first": my uncommitted `reaper.sh`), leaving
  b033.sh waiting. Stopped at 16:02Z, the drivers committed on the day branch (`7fbb4f64`), restarted 16:03:14Z.
  No spend in the failed try.
- Terms 2 to 9: site 0 at 2, then 2 at 3, 0 at 4, 1 at 5, 0 at 7, 1 at 8, 0 at 9 from 16:13:02Z, with leaderless
  gaps of 2 to 5 s (`terms-m39b33.jsonl`).
- Leader commit p50 5.8 s, p99 29.7 s (n 5,349); queue:slot p99 26.0 s; deps:all p99 13.9 s.
- Agent-class ok 2,784 in 300 s (9.3 a second). Agent pushes 1,368 of 12,469: `full` 3,102; "access rows did not
  answer (not leader)" 2,889; "(no config)" 2,679; "created, but no member leads" 1,179; "no outcome ... missed" 302.
- Voter peaks 64 / 28 / 42 MiB.
- The trace `tsave-m39b33.jsonl` (32 MB, 406 lines; `.xz` 2.4 MB) went to B0-02
  (T-01a1ccdd-804f-769d-b81a-5f937e3309df) for `inbound.py`.

| B0-33 line (`build/BUILD-PLAN.md`) | m39 | verdict |
|---|---|---|
| Human and merge-queue commit p99 ≤ 1 s | leader commit p99 29.7 s; merges 2 of 58 ok; human pushes 9 of 194 | fail |
| ≥ 140 agent commands/s admitted (fail < 100) | 9.3 agent-class ok a second | fail |
| User token admitted as `Agent` | fast user 61 of 517 × 200, 456 × 503 | not shown |
| Every refusal in the `ng` or 429 shape | 503s and 500s dominate | fail |
| Zero leader changes and `suspect` | term 2 → 9 | fail |

**Cost.** The meter (`cost.sh`, T0 `mem7-t0`): $178.80 before the site day (14:31Z); $178.88 before B0-34
(15:16Z); $180.22 after B0-34 and its sweep (15:54Z, the row $1.34); $181.22 after B0-33 and its sweep (16:19Z, the
row $1.00). The day cost $2.42 of the $185 stop; $3.78 of the approval is left. Every row ended with
"sweep: no shard container running".

**Raw.** Under `tests/fanout/raw/` on the day branch: `siteday-m39.status`, `idlewatch-m39.jsonl`,
`costrow-m39.status`, `b034-m39.status`, `b034warm-m39.jsonl`, `rehydrate-131296-m39.txt`, `spools-m39.txt`,
`instcpu-m39.jsonl`, `shardprocs-m39.jsonl.xz`, `misses-m39.txt`, `misslines-m39.jsonl`, `a1-b0-34-m39.log`,
`costrow-m39b33.status`, `rows-m39b33.status`, `memguard-m39b33.jsonl.xz`, `terms-m39b33.jsonl`,
`tsave-m39b33.jsonl.xz`, `a1-b0-34-m39.jsonl` and `a1-b0-33-m39.jsonl` (copies of `tests/am1/raw/a1-b0-34-mwjex2b9.jsonl` and `a1-b0-33-mwjghnzg.jsonl`). Day branch `m39-day` (`6276c837`), with the drivers `costrow.sh` and `idlewatch.py`.

**From graft-m3f (T-01a1d805, run m3fr9, row B0-32 on repo 65575, k 4, 8 learners; main a0bca583 plus a test-faults instrument).** Site 0 held term 2 through setup and the first 3 minutes of load, then 17 elections in 5 minutes (term 2 at 16:13:09Z to 19 at 16:18:29Z; wins 6 / 7 / 4 by site), with 10 "member alarm: StepDown" lines (7 CommitErrors, 3 Confirm) and 7 term changes after the leader went quiet or reset. The leader's peers ended at 1,085 and 1,289 deadline failures against about 5,700 ok each; queue p99 9.0 s, append p99 about 2 s, deps:authz p99 7.3 s. The same shape as m39's B0-33 (terms 2 to 9 once load arrived): the leader steps down on CommitErrors when its appends miss their deadlines under load, rather than losing a vote [inference; the m3f raw files are in that thread's orb, not pushed].

### Round 35: m40 site day (main 77c9fa23, standard-4), the idle check, B0-34; B0-33 stopped by the hosted stop (2026-11-16)

**Site day m40.** From main `77c9fa23` with `SHARD_INSTANCE=standard-4` (`run-siteday.sh`'s new default). Image
`sha256:a476cbbf…` (unchanged), sites deployed with exit 0 at 18:31Z (site c `dadfc447`), edge `23610374`. Idle check
18:57:52 to 19:28Z with the reapers on: 0 starts (`idlewatch-m40.jsonl`). An orb drop had cut an earlier idle window
(`try1-idlewatch-m40.jsonl`), which does not count.

**B0-34, a1 `mwjnulqj`, 19:39:18 to 19:54:18Z.**
- Cold warm 209.3 s to every shard ls-refs-capable (main at 110.6 s; catch-ups 110.3 / 135.8 / 209.3 s), 118 stored
  packs, 8.95 GB, no fault and no dropped roll.
- Per-container R2 ceiling (`spools.py`): site 0 51.7 MB/s, site 1 90.7, site 2 117.9, probe `pool:7` 109.8; both busy
  238.6 MB/s, so the limit is per container [measured].
- Clone TTFB p99 by phase (j9 / kill / second / hostile): 49.5 / 22.3 / 31.3 / 348 s. Human push p99 76.7 / 27.8 /
  41.7 / 350 s. Leader commit p99 810 ms, term 2 throughout, k 2.
- Misses: 334 of 340 probes joined to their line (`misses-m40.txt`); composed 313, container p99 5.0 s; to-shard p99
  13.4 s.
- Site c's git RSS peaked at 37 GB with 8 stock upload-packs at 19:52Z (`shardprocs-m40.jsonl`).
- Verdict: fail on TTFB and human push; k ≤ 8, hostile owner and no-API hold. Row cost $1.69 (cost.sh).

**B0-33 m40** started at 20:11Z (repo:131344) and was stopped about 3 minutes in by the operator's hosted stop
(20:13Z, the account bill). Not judged. `teardown.py --sweep` tore down 8 shards; 0 running at 20:14:45Z.

### Idle Durable Object rows on graft-m2b, and the retire (2026-11-16, no deploy, no spend)

The operator's question: which m2b objects write rows while idle (about $0.61 in an hour). From
`durableObjectsPeriodicGroups` by namespace and object (`tests/fanout/idlerows.py`):
- m2b rows written per hour: 17Z 19,082 and 18Z 21,105 idle; 19Z 583,653 (B0-34's tail and B0-33's start, the $0.58
  hour). Idle m2b is about $0.02 an hour [measured].
- The account's idle rows were graft-m1, not m2b: 11.1 M rows at 17Z (acctcost $11.15 for that hour), 98% from one
  group, repo:1048591 (`#1#f0` 4.14 M, `#1#f2` 4.02 M, `#1#f1` 2.95 M). Over 2026-10-30 to 11-16 m1 wrote 420 M rows and
  m2b 174 M. Reported to the operator; m1 is not this thread's to touch.
- m2b's idle writers, 16:45 to 18:30Z, 4.1 to 6.2 k rows per 15 minutes:
  - `names:*` members, about 2.8 k per 15 minutes, woken by the `*/15` site cron: `sweep::run_at` reads
    `repo_owner` and `owner_record` on Names chains for every network each run, and the fold audit wakes more. Each
    woken member then re-arms a 2 s alarm for `DRAIN_IDLE_MS` (60 s) after each request it served
    (`workers/site/src/host.rs`, `alarm` and `arm`; one row per `setAlarm`).
  - The finished B0-33 groups 131322, 131323, 131329 and 131330, 1.3 to 3.2 k per 15 minutes: the cron's `/promote`
    and `/catchup` with `warm=1` on each start new leaderships ("shard warm" lines from `prsync.rs`).
- Retired at 20:27Z with the test route `/retire`, as the install's own teardown (`tests/fanout/idleretire.mjs`): every
  member of the finished lm1 test networks 131322 to 131344 that wrote rows that day, 215 members (198 joined, 17 already
  not), then `/status`: 215 of 215 "not joined" (`tests/fanout/raw/idle-retire-m2b.log`). Not retired: B0-34's
  networks (131296 with its 5 GB seed, 327700, 9043968 to 9043970), `names:*` and the system chains.
- Loop writers to fix (operator's rule 4, not built): the cron sweep's Names reads across every network every 15
  minutes (cache each network's home in the sweeper); the 2 s member alarm re-arm after each served request; and the
  cron's `warm=1` promote and catch-up on finished groups.

### Budget change (2026-11-16, about 20:30Z)

The operator replaced every per-thread cap: $200 a day for the whole Cloudflare account, per UTC day from 20:30Z,
read with `tests/fanout/acctcost.py` (the only meter; per-install meters are for diagnosis). One hosted run at a time in
a slot the operator grants, with an estimate and a stop sent first; every run ends with a sweep, `/retire` of every
finished test group, and an acctcost reading 30 minutes later showing the install idle (under $0.10 an hour). A row is
met at 90% of each target (`FRAME.md`, "Operator decision, 2026-11-16, 90% gates and a daily spend limit"); zero targets
stay exact. The m41 slot's stops: $120 of account spend since 20:30Z, or $25 of the run's own.

**90% re-judge from results already in hand (no spend).** L0-12's A1 replay is met at 90% on m3fr6 (graft-m3f), its
clock line judged on the spread between members (56 to 512 ms), by the operator's decision; the 972 ms offset from
Cloudflare's clock is a finding under F3 (branch `lm1-90`). Still short: B0-29 (best gate run m27, commit p99 774 ms
against 660), B0-31 (m3fr8, buckets-up commit p99 2,138 ms against 1,100), B0-32 (0 of 500 within 5.5 s), B0-33,
B0-34, and B0-35 (dispatch unmeasured, not a long tail).

### Round 36: the m41 slot, site day from main 617e2b0a with roll-up on, the idle check, B0-33, B0-34 (2026-11-16)

The operator's slot, granted after the 20:30Z budget change. Account meter `acctcost.py` since 20:30Z: $0.10 before
the site day, $0.20 after it, $0.34 after the idle check, $2.27 after B0-33 and its sweep, $4.51 after B0-34 and its
sweep (22:30Z). The slot's own delta since 20:52Z: $4.41. The stops (account $120 since 20:30Z, run $25) were never near.

**Site day m41.** From main `617e2b0a` (promote-lapse, rollup, fanout-dups, feed-gone-route), `SHARD_INSTANCE=standard-4`
and, new, `GRAFTYARD_ROLLUP=1` on every site (`siteday.sh`'s `ROLLUP=1`), with 32.25 added to the edge's
`FORMATS_ENABLED` (`run-siteday.sh`'s `EDGE_EXTRA`). Image `sha256:0f0c5ee1…` (containers changed since a476cbbf: the
container's `/roll`). Sites, test-faults, exit 0: a `54c4fc8f`, b `a83bbd2b`, c `dfda5a3f`; edge `8c67d1ff`, served at
100% (`edgever.py`). 20:52 to 21:07Z.

**Idle check, 21:08:51 to 21:38:51Z, reapers a and b off: 0 shard starts on graft-m2b** (`idlewatch-m41.jsonl`). What
it shows: nothing started a shard while idle. What it does not show: promote-lapse stopping a shard a promotion
started, because no promotion came due in the window. That proof still needs a promotion-started shard on a quiet group
to stop within about 10 minutes (`PROMOTE_LAPSE`, `crates/git/src/shard.rs`). Reapers back on at 21:41Z for every
range except the row's networks.

**B0-33, repo:131345, a1 `mwjshq3m`, 5,000 agents, k 4, 8 learners, inbound trace on (tsave EVERY=10000), load
21:48:20 to 21:53:20Z: fail.**

| B0-33 line | m41 | m39 (before fanout-dups) | verdict at 90% |
|---|---|---|---|
| Human and merge-queue commit p99 ≤ 1 s | leader commit p99 14.5 s (p50 2.0 s, n 5,977); merges 2 of 42; human pushes 12 of 195 | 29.7 s | fail |
| ≥ 140 agent commands/s admitted (≥ 126 at 90%; fail < 100) | 26.8 agent-class ok a second at the client | 9.3 | fail |
| Zero leader changes and `suspect` | term 2 → 20 | 2 → 9 | fail |

- Step-downs (Workers Logs `member alarm: StepDown`): 13, 11 `CommitErrors` and 2 `Confirm` (site a 21:48:47 p99
  721 ms; site c 21:50:12 p99 844 ms). The leader logged 1,532 failed calls; to site 1, 427 deadline and 98 fetch of
  2,029 sent, 394 late.
- Leader split p99: queue 14.9 s, round 13.3 s, confirm 4.5 s, apply 11.7 s, deps:authz 9.3 s.
- Read rounds per voter (`tests/f74/rounds.py`, voters only; appends and votes counted, so upper bounds): fetches per
  voter per 20 ms max 21, p99 10, p50 1 over 5,215 windows (m39: max 57, p99 26 over 1,451); fetching turns per voter
  per 20 ms max 3, p99 2 (m39 the same).
- Read grant on the leader (IndexTaken to ResSent): site 0 p99 2,540 ms (n 1,533), site 1 4,078 ms (n 473), site 2
  1,713 ms (n 868).
- Voter peaks 60 / 73 / 81 MiB, 0 `exceededMemory`. No lost acknowledgment seen.
- Reading: fanout-dups cut duplicate fetches about 2.5 times at p99, and the agent-class rate rose 2.9 times, but the
  `CommitErrors` churn remains [measured]; the trace went to B0-02 (T-01a1ccdd).
- Cost $1.94 (acctcost 21:39 to 21:56Z).

**B0-34, repo:131296, a1 `mwjt5tks`, standard-4, pool probe 2:7, load 22:08:19 to 22:23:19Z: fail.**
- Cold warm 197.2 s to every shard ls-refs-capable (main at 167 s; catch-ups 181.1 / 197.2 / 155.4 s). 203 stored packs,
  9.09 GB, 75 through their `.idx`, 0 rejected, no fault, `dropped_rolls` [] (no roll has been recorded on this network
  yet). Rehydrate phases: up 0.4 to 34 s, markers 12.7 to 45 s, marked phase 99 to 114 s, tail 134 to 172 s.
- Per-container R2 ceiling (`spools-m41.txt`): site 0 63.3 MB/s, site 1 72.4, site 2 100.3, probe `pool:7` 107.8 MB/s
  in total; peak 5 s bins 130 to 258 MB/s. In the 14 bins both were busy, site 2 119.7 plus the probe 129.7 = 249.4 MB/s
  against site 2 alone 35.6: per container again [measured].
- k: the monorepo stepped in band to `kMax` 3; the second network and the hostile three stayed at 1.
- Clone-cache hits stream, no 500s: 55 hits, all 200 (j9 12, kill 14, second 4, hostile 25).
- Misses: 432 probes, all composed; `misses.py` joined 425 to their line (7 probes without one, 13 lines without a
  probe); answers 424 composed and 1 × 503. Composed first byte p50 2.5 s, p99 25.9 s; to the shard p99 11.1 s;
  container p50 190 ms, p99 3.1 s; transit p99 20 ms; edge plus network p99 22.5 s (`misses-m41.txt`).
- Stock upload-packs per shard: at most 2 at once (site b, 6 MB) and 1 (site a); git RSS peaked 10.0 / 8.9 / 5.6 GB on
  sites a / b / c during the warm, not the row (`shardprocs-m41.jsonl`).
- Leader commit p99 932 ms (p50 352, n 5,386), term 2 throughout.

| B0-34 line | m41 (j9 / kill / second / hostile) | verdict at 90% |
|---|---|---|
| k in band, ≤ 8 per site | `kMax` 3 on the monorepo, 1 elsewhere | pass |
| Hostile owner held to its pool budget; the monorepo's k not reduced | hostile 1; monorepo 2 to 3 | holds |
| Human push p99 ≤ 3 s beyond transfer (≤ 3.3 s) | 48.5 / 34.9 / 48.9 / 32.9 s | fail |
| Clone TTFB p99 ≤ 1 s (≤ 1.1 s; fail over 5 s) | 31.3 / 12.7 / 45.4 / 23.6 s | fail |
| No Cloudflare API call from a request path | 0 files | pass |

Cold warm against DESIGN's 120 s line: 197.2 s, fail (m40 209.3 s; m37 150 to 158 s). The roll-up has not rolled this
network, so the warm read the consumed packs as before; the roll measurement waits for its own slot. Cost $2.17
(acctcost 21:57 to 22:31Z).

**Close-out.** Sweep after each row ("no shard container running"); B0-33's group retired at 22:32Z, all 20 members that
wrote rows "not joined" after (`idle-retire-m41.log`); B0-34's networks kept, as its seeds are reused. Reapers back on
for every range, B0-34's included.

**Idle after the slot (30 minutes).** acctcost 22:31 to 23:02Z: $0.21 for the whole account, of which graft-m2b $0.04
(sites a $0.02, c $0.01, b $0.01), about $0.08 an hour for m2b and under the $0.10 line; graft-m3f $0.11 is another
thread's. m2b rows written: 5,674 in 22:00 to 23:00Z (the slot's tail included), 703 in 23:00 to 23:02Z, mostly
`names:*` (names:708 and names:824 members 211 to 283 rows each, the cron's loop writers above). `idlewatch.py` 22:32:31
to 23:04:31Z: 0 shard starts. No range came due on B0-34's network, so promote-lapse was not exercised; its proof
still waits for a promotion-started shard on a quiet group. Account since 20:30Z: $4.77 at 23:02Z. Slot closed.

### `sweep-quiet`: the cron sweep's `Names` reads (2026-11-17, code only, no spend)

The operator's rule 4 named the `names:*` rows the `*/15` sweep writes as a loop writer. Measured on idle `graft-m2b`
before any change, with no deploy (`tests/fanout/idlerows.py` and a per-minute query of `durableObjectsPeriodicGroups`):
- 23:05 to 23:30Z: 2,614 rows written, all by `Member` objects named `names:*` (487 objects), apart from 3 `Registry`
  rows (the sweep and fold audit cursors) and 1 `GitShard` row. Per minute, the member rows land only in 23:15 to 23:19
  and 23:30 to 23:34 (513 to 814 a minute, then 25 to 27), the four minutes after each cron run.
- One member, `names:391#1#f0`: 5 rows written and about 3,780 read at 23:16 and at 23:18, then at 23:31 and 23:33,
  two wakes after each cron run. `names:708`'s leader stays active 60 s a minute for 4 minutes after each run, the
  drain alarm's `DRAIN_IDLE_MS`.
- Every site's cron run spends 255 subrequests (`workersInvocationsAdaptive`, 12:00 to 23:00Z on 11-16), and site a's
  has ended "Too many API requests by single Worker invocation" on every run since 20:43Z (Workers Logs), so its
  sweep has not run since. Sites b and c log `sweep {` with 0 networks swept and 0 unresolved, 198 to 243 s a run,
  nearly all of it the home reads.

Cause [read from the code, matched to the timing above]: `sweep::run_at` reads `repo_owner` and `owner_record` through
the `Names` leader for every network in the buckets, every run, on every site, before it sweeps any.

Fix, branch `sweep-quiet` (DESIGN, the sweeper paragraph; DECISIONS, `sweep-quiet`): each site keeps every network's
home in its `sweep` cursor object for HOME_FOR_MS (24 h) and reads at most HOMES_PER_RUN (64) new or day-old homes a
run. Two tests in `workers/site/src/sweep.rs`; `tests/sweepquiet/plant.py` catches 7 of 7 plants, the first of them
main's read-every-run. Expected after a site day [inference]: one run reads every home once (64 a run, so about three
runs on m2b's 180 networks), then about 180 home reads a day per site instead of about 34,000, and the `names:*` rows
fall from about 2.6 k a run to near zero outside a client request. Not deployed: it ships with the next site day.

sweep-quiet also fixes a broken sweeper, not only idle cost: site a's cron sweep has failed on every run since 20:43Z on
2026-11-16 at the subrequest limit, so no network homed on site a has been swept since (orphan packs, missing
third-bucket copies and markers, and multipart aborts). With 64 home reads a run at most (128 `Names` calls) it should
stay inside the limit [inference]; the site day checks it. Merged to main at `68d39927`; it ships with the B0-33 slot's
site day, which checks it hosted: `names:*` rows per cron run once the cache fills, against about 2.6 k, and site a's
`sweep {` lines with no subrequest failure.

### Round 37: the m42 slot, site day from main 68d39927 (lookup-share, sweep-quiet, near-check, audit-locked), B0-33 (2026-11-17)

The operator granted the slot at about 03:15Z while B0-30's close-out ran on graft-m1, so the run's stop counted only
graft-m2b's scripts: `acctcost.py`'s new `PREFIX` and `costok.sh`/`costrow.sh`'s `RUN_PREFIX`. Stops: $25 on m2b from
03:20:06Z, or $170 for the account's UTC day. Estimate sent before the day: about $3.5 (m41's slot $4.15 by prefix).

**Site day m42.** From `20b87b14` (main `68d39927` plus this file's note), m41's image `sha256:0f0c5ee1` (containers
unchanged since `617e2b0a`), standard-4, `GRAFTYARD_ROLLUP=1`, 32.25 in the edge's `FORMATS_ENABLED`. Sites, test-faults,
exit 0: a `002c035d`, b `389742cf`, c `705e3523`; edge `e2efd770` served at 100% (`edgever.py`). 03:20 to 03:33Z.

**B0-33, repo:131346, a1 `mwk57m2z`, 5,000 agents, k 4, 8 learners, inbound trace on, load 03:43:57 to 03:48:57Z:
void for the lookup check (operator, about 04:10Z).** lookup-share broke the edge under load, as m3f found on its own
install: the errors below. The operator is reverting lookup-share on main; the lookup comparison waits for B0-02's
redesign. The numbers stay as a record of what the group did while the edge failed, not as a verdict.

| B0-33 line | m42 (void) | m41 | verdict at 90% (not judged: void) |
|---|---|---|---|
| Human and merge-queue commit p99 ≤ 1 s | leader commit p99 2,515 ms (p50 486, n 12,580); merges 6 of 54; human pushes 41 of 192 | 14,521 ms; 2 of 42; 12 of 195 | fail |
| ≥ 140 agent commands/s admitted (≥ 126 at 90%; fail < 100) | 29.2 agent-class ok a second at the client | 26.8 | fail |
| Zero leader changes and `suspect` | term 2 throughout, 0 step-downs (Workers Logs); /suspect 0 to 0.2 a second on the voters | term 2 → 20, 13 step-downs | leader changes met; `suspect` not zero |

- Voter routes during load (`Trace::Entered`, tsave): /read 5.3, 1.5, 3.4 a second and /suspect 0, 0.2, 0.2 a second
  on sites 0, 1, 2; m41 after its first change 321, 258, 306 and 155, 149, 156. No leader change came, so the
  "after the first change" comparison has no window; these are the whole load.
- `inbound.py`'s `/read` source: on the voters `ranges:lead` (leader 3.9 a second) and `other:idx`; on learners 6 to
  12 a second, `other:idx` and `ranges:idx`. Voter-to-voter /raft calls 17,591, 16,921 of them out under 200 ms with
  nothing entered ahead; 102 missed 1,000 ms.
- Read grant on the leader p99 228 ms (n 10,391; m41 2,540); fetches per voter per 20 ms max 6, p99 3, p50 1 over
  14,935 windows (m41 21, 10, 1); fetching turns max 3, p99 2.
- Leader split p99: queue 4,351 ms (slot 4,102), round 535, confirm 263, apply 535.
- 0 exceededMemory; leader wasm 4 to 16 MB.

**Where the clients' time went: the edge.** Agent-class requests answered at p50 20,028 ms, `LOOKUP_FOR` exactly
(m41's p50s were 4.4 to 25 s), and the edge logged in every 10 s window of the load: `tokens:N: no member answered`
(2,414 for tokens:0, 684 for tokens:932), "closure invoked recursively or after being dropped" 329, "Cannot perform
I/O on behalf of a different request" 219, the runtime's "hung" cancel 121, "Cannot invoke closure from previous WASM
instance" 21, "Promise will never complete." 20. m41's load window holds none of them, and neither does m42's setup
before load. The tokens leaders answered their reads in tens of ms in the same minutes (`durableObjectsInvocations`).
Hypothesis [inference, not traced]: lookup-share's per-isolate `Turn` (`crates/lookup/src/lib.rs`, `take_turn`) parks
requests on another request's lookup; a holder the runtime cancels or drops wakes wakers of requests whose contexts
are gone, and the waiters sit out the whole `LOOKUP_FOR`. Sent to B0-02 (lookup-share's thread) with the trace.

**sweep-quiet, hosted (`tests/fanout/sweepquiet.py`).** Before the day, six slots 02:00 to 03:30Z: 2,565 to 2,847
`names:*` rows a slot from 484 to 485 objects, and site a's run failed "Too many API requests" every time. After it:
- 03:30 slot (b on the new script, a and c not yet): b `homesRead 64`, 96 s; a failed once more on the old script.
- 03:45 slot: a, b, c `ok`, each `homesRead 64`, 72 to 110 s. **Site a's sweep completed**, its first since 20:43Z on
  11-16.
- 04:00 slot: a `homesRead 64`, swept 20 networks, 135 s; b 55 (its cache full); c 64.
- `names:*` rows: 3,319, 2,910 and 1,671 in those slots, with the cache still filling and the B0-33 load's own name
  reads in the 03:45 slot. The steady state is in the idle reading below.

**Not measured this slot.** promote-lapse: no range came due on a quiet group. The roll: 131296 has no rollable run
(8 promoted packs or a day-old run), so no `Compacted`.

**B0-33 rerun on the revert (operator, about 04:15Z): edge-only redeploy from main `0de37416`** (lookup-share
reverted; sites stay on m42's day, the edge is not a site script). Edge `2004a578`, served at 100% (`edgever.py`,
rev `f18eecd8`, the day branch with `0de37416` merged), same formats. A baseline row plus sweep-quiet, not a lookup
check. Repo:131347, a1 `mwk6t1a9`, 5,000 agents, k 4, 8 learners, inbound trace on, load 04:26:53 to 04:31:53Z: fail.

| B0-33 line | m42 rerun (revert) | m42 (lookup-share, void) | m41 | verdict at 90% |
|---|---|---|---|---|
| Human and merge-queue commit p99 ≤ 1 s | leader commit p99 11,859 ms (p50 2,985, n 2,875); merges 1 of 54; human pushes 7 of 200 | 2,515 ms | 14,521 ms | fail |
| ≥ 140 agent commands/s (≥ 126 at 90%) | 22.0 agent-class ok a second | 29.2 | 26.8 | fail |
| Zero leader changes and `suspect` | term 2 → 19, 14 step-downs (13 `CommitErrors`, 1 `LoopLag`); first at 04:27:18Z, 25 s into load | term 2, 0 | term 2 → 20, 13 | fail |

- Voter routes over the load (`Trace::Entered`): /read 368, 463, 348 a second and /suspect 157, 111, 128 a second on
  sites 0, 1, 2 (m41 after its first change: 321, 258, 306 and 155, 149, 156). The m41 storm is back with lookup-share
  gone. Corrected by B0-02's trace reading (below): lookup-share did not calm the group; its 20 s hung requests cut the
  offered load, and the group stayed quiet only because little load reached it.
- Read grant on the leader p99 2,486 / 1,980 / 3,117 ms by leading site (m41 2,540 / 4,078 / 1,713); fetches per voter
  per 20 ms max 19, p99 9, p50 1 over 3,948 windows (m41 21, 10, 1).
- Leader split p99: queue 13.6 s, round 7.9 s, confirm 4.3 s, apply 8.5 s.
- Agent pushes p50 33.8 s, p99 123.9 s; `503 the repository's access rows did not answer` 1,052, `created, but no member
  leads` 912.
- **Edge errors** (`$metadata.level` error, request lines left out; `edgeerr-*.txt`): none of the lookup-share kinds in
  the rerun's load window, as in m41's; 27 of 30 ten-second windows were at the query's 100-event cap with request
  lines only. In m42's lookup-share window: `tokens:N: no member answered` 636, "closure invoked recursively or after
  being dropped" 329, "Cannot perform I/O on behalf of a different request" 219, the runtime's hung cancel 121, "failed
  to read array buffer from request" 98, "Cannot invoke closure from previous WASM instance" 21, "Promise will never
  complete." 20. Every count is a lower bound: the query returns at most 100 events a window. Read alone,
  `no member answered` gave 3,098 events (2,414 for tokens:0, 684 for tokens:932, 3 for names:708).

**sweep-quiet, settled.** 04:30 slot: a, b, c each `homesRead 1` (the one new network, 131347), walls 78, 42 and 40 s;
site a swept 20 networks. `names:*` rows 499 from 17 objects in that slot, which held B0-33's load; before the day
2,565 to 2,847 from 484 to 485. The idle reading below gives the steady state.

**Idle after the rerun (04:41:15 to 05:11:15Z).** graft-m2b by prefix $0.02 for the 30 minutes (about $0.04 an hour,
under the $0.10 line); the account's $15.20 in the window is graft-m1's B0-30 close-out. `idlewatch.py` 04:41 to 05:12Z:
0 shard starts. **sweep-quiet's steady state: 0 `names:*` rows in the 04:45 and 05:00 slots** (0 objects), against
2,565 to 2,847 a slot before the day; every site's run read 0 homes, walls 36 to 39 s on b and c and 119 to 209 s on
site a, which sweeps 20 networks a run, all `ok`.

**B0-33 on lookup-poll (operator, about 05:05Z; slot stop raised to $40 by graft-m2b script from 03:20:06Z).** Edge-only
deploy from main `67ba9343` (B0-02's lookup-poll: one plain-data mark per chain that waiters poll on their own timers),
edge `1b03108d`, served at 100% (`edgever.py`, rev `6a59ce12`, the day branch with `67ba9343` merged). Sites stay on
m42's day. `tests/fanout/edgeguard.sh` watched the edge every 30 s for lookup-share's lines, to stop the row and put
back `0de37416`'s edge. Repo:131348, a1 `mwk8r5wk`, 5,000 agents, k 4, 8 learners, inbound trace on, load 05:21:07 to
05:26:07Z: fail.

| | lookup-poll | baseline (revert) | lookup-share (void) | m41 |
|---|---|---|---|---|
| Agent-class ok a second | 18.6 | 22.0 | 29.2 | 26.8 |
| Agent p50 near 20 s from the lookup | no (503s after retries) | no | yes, 20,028 ms | no |
| lookup-share error lines (cross-request I/O, dropped closure, previous instance) | 0 (guard read 0 every 30 s; load-window census none) | 0 | 569 and more | 0 |
| Terms; step-downs | 2 → 22; 9 (all `CommitErrors`) | 2 → 19; 14 | 2; 0 | 2 → 20; 13 |
| First leader change | 05:21:33Z, 26 s in | 04:27:18Z, 25 s in | none | 27 s in |
| Voter /read a second after it | 293, 463, 213 | 392, 501, 375 | (no change) | 321, 258, 306 |
| Voter /suspect a second after it | 140, 243, 125 | 172, 121, 139 | (no change) | 155, 149, 156 |
| Read grant p99 by leading site | 2,699 / 1,800 / 3,338 ms | 2,486 / 1,980 / 3,117 | 228 | 2,540 / 4,078 / 1,713 |
| Leader commit p99 | 17,701 ms | 11,859 | 2,515 | 14,521 |
| Fetches per voter per 20 ms max / p99 | 20 / 7 | 19 / 9 | 6 / 3 | 21 / 10 |

- The edge is correct on lookup-poll: no lookup-share line in setup or load. The agent routes' p50 of 17.6 to 22.7 s are
  503s after member retries (`access rows did not answer` 2,091, `created, but no member leads` 1,661), as on the
  baseline, not the `LOOKUP_FOR` wait.
- The group's storm is as on the baseline: after the first leader change each voter takes 213 to 463 `/read` and 125
  to 243 `/suspect` a second. `inbound.py` names the voters' reads as lead reads: on voters 0 and 1, `other:lead` 153 and
  256 a second, `ranges:lead` 79 and 118, `probe:lead` 58 and 84; learners take 110 to 121 idx reads a second each.
  lookup-poll's mark is per isolate, so this is consistent with many edge isolates each running a lookup once their
  cached leader is wrong [inference, not traced]; sent to B0-02 with the trace. The tag names are wrong; see B0-02's
  reading below.

**B0-02's reading of the lookup-poll trace (`tsave-m42p33`, analysis only).**
1. **The `ReadQuery` tag names the wrong reads.** `read_query_code` takes the query's first key, but this build's
   serde_json has no `preserve_order`, so the keys come back sorted. `ranges` is `core_rows_q`'s
   `{"ranges", "rows": {access/agent/actions}}`, the push gate's and agents' rows. `other` holds `tier_rows`'
   `{"packed", "ranges", "rows"}` (sorted, it starts with `packed`), refs pages and `{"q": {}}`. `probe` is right: the
   probe of `lookup::create` (`ensure_group`, and `leader_site` in actions). So every `ranges` and `other` label in m42
   and m42p means core rows and packed or refs reads, not range reads.
2. **Why lead reads jump after the first change.** `read_spread` sends core and tier reads to the leader only when all
   three members it asks answer NotLeader. A learner answers that once its read-index call to the leader fails, because
   the failure makes it forget the leader. Learner `IndexEnded` before 05:21:33Z: 320 ok, 469 late, 495 failed; after:
   867 ok, 4,957 late, 6,101 failed; even under a leader stable for 3 s or more, 661 ok, 1,450 late, 1,739 failed.
   Voter idx reads fell from about 100 to 190 a second to about 0 after 05:22:27Z, and lead reads took their place. That
   is a loop: learners lose the leader, reads fall to the leader, the leader slows, more learner calls fail. Its
   trigger is the leader's load, the same load that trips `CommitErrors`.
3. **Lead reads by leadership state**, a second across the voters: 974 while the leader changes (164 s; other 533,
   ranges 269, probe 172); 673 with no leader (30 s); with a leader stable 5 s or more (68 s), 508 at the leader plus 230
   at non-leaders. The 230 are edge requests still walking voters, from many isolates or from caches cleared again by
   `commit_hedged`'s `forget_leader` after NotLeader or a missed receipt; members stamp every edge request `from=255`, so
   this trace cannot count isolates or split the two.
4. **`probe:lead` runs 100 to 390 a second after the change.** The edge's `commit()` calls `ensure_group`, which probes,
   whenever the isolate holds no leader, as it does after every NotLeader; while no leader is found, `create` also posts
   `/campaign` to the home voter in its loop, up to 370 in 20 s to site 0 while sites 1 and 2 led. That is the agents'
   "created, but no member leads" 503.
5. **The m42 contradiction.** m42's quiet group (term 2, /read 1.5 to 5.3 a second) was not lookup-share fixing the
   storm: its hung requests (p50 20 s) throttled the offered load. The revert (2 → 19, 14 step-downs) and lookup-poll
   (2 → 22, 9) churn alike; lookup-poll restores the edge and leaves the group side as it was.

Fix candidates (not designed; the operator decides): (a) a learner whose read-index call failed sheds a spread read
(retry) instead of answering NotLeader, so a lost leader does not move the whole read class onto the leader; (b) skip
`ensure_group`'s probe and campaign loop when the chain's config is known (the group exists); (c) fix the query tag
(append `packed` and `refs` to `READ_QUERIES`, or tag the sorted key set); (d) count edge isolates (an isolate id on
test-faults requests) before redesigning the mark.
- Close-out: sweep "no shard container running"; 131348's 11 members retired (11 of 11 not joined).

**Final idle reading and slot close.**
- Idle window 05:34:19–06:04:19Z after the lookup-poll row, reapers on except the retired row network: graft-m2b
  $0.02 by script (about $0.04 an hour; account $4.82 in the window, all graft-m1's B0-30 close-out). idlewatch-m42p
  saw 0 shard starts in 31 minutes (`raw/idlewatch-m42p.jsonl`).
- sweep-quiet after the third row (`sweepquiet.py`, 05:15–06:05Z): names:* rows 55 (05:30), 169 (05:45) and 24
  (06:00) a slot, up from the 0 of the 04:45 and 05:00 slots, and all sites ok. Site a's sweep wall grew to 352 s
  (05:30, swept 12, planned 708) and 620 s (05:45, swept 20, planned 156); its 06:00 run had not reported by
  06:06Z. Open finding [measured, cause not traced]: the rows likely come from the row's new and retired group refilling
  the cache, but a 620 s wall is two thirds of the 15-minute cron period, so site a's sweep can overlap the next run.
  Check at the next slot whether the wall falls back to 119–209 s.
- Promote-lapse and the roll measurement were not exercised (no promotion came due; 131296 has no rollable run).
- Slot closed at 06:06Z. Meter (`acctcost.py`): graft-m2b $4.43 since the site day at 03:20:06Z (stop $40); account
  $77.78 for the UTC day at 06:06Z (stop $170).

### Site a's sweep wall (2026-11-17, code only, no spend)

Asked for by the operator: site a's `*/15` sweep took 620 s at 05:45Z. Read from logs already in Workers Logs and R2
analytics, with no new hosted work. The raw pulls are by trace id of each cron invocation.

**What dominates.**
- Site a's runs are serial over its networks (`graftyard_git::sweep_all`), and each unmarked pack's `request` row is
  asked through `lookup::call`, which spends LOOKUP_FOR (20 s) on a chain no member answers.
- The 05:45 run (trace 16e21fac, 647.8 s wall, 13.9 s CPU, ended `ok`):
  - 05:45:22 cursor read; 05:45:58–05:46:09 network 131296's lookups (372 member reads, 10 s).
  - 05:46–05:49: network 131296's 64 fills from site a's bucket, 979 MB of GETs (r2OperationsAdaptiveGroups:
    308, 288, 202 and 65 MB a minute), no member reads for 267 s; 56 of them failed "Network connection lost".
  - 05:50:30–05:55:22: networks 131297–131315, lookup bursts per group of 1 to 119 s, 28 member objects. The longest
    were two member objects with 2,843 reads over 119 s and 2,399 over 22 s; by their order in the run they are
    131307 and 131313, the finished B0-33 groups whose 8,494 and 8,395 packs alarmed on 11-16 [inference: the trace
    names objects, not networks]. 7,013 member reads in all, all 200, wall p50 16 ms.
- The 05:30 run (352 s) was the same shape: 407 reads, a 68 s gap for 131296's fills, the 547 markers.
- **The 06:00 and 06:15 runs were cancelled at 899.5 s and 899.6 s**, the Cron Trigger's 15 minutes, with no `sweep {`
  report. Trace 4719b3f9 (06:00): 3,980 member `/read`s answered **409 "member not joined"** and 3,384
  `registry/row` reads for `repo:131322` (1,782) and `repo:131323` (1,602), at a steady 270 to 370 a minute for the
  whole 14 minutes. Those are the B0-33 groups retired on 11-16 (`/retire` deletes the members' storage); their packs
  stay in the buckets, so each pack's lookup walks bootstrap members (409), the registry rows and back until LOOKUP_FOR.
  At 16 lookups at a time and 20 s each, about 48 packs a minute; 131322 alarmed 1,236 packs and 131323 9,216 on 11-16,
  so no run gets past them [inference from those counts; the 06:00 trace shows the steady rate, not a pack count].
- A cancelled run stores no cursor (`workers/site/src/sweep.rs` writes it at the end), so every later run starts at
  the same network and dies there. Site b's and c's runs stayed at 65 to 99 s; they home none of these networks.
- Before the retirements, 11-16 00:00–11:45Z, every site-a run took 266 to 455 s: one or two networks a run, cut by
  the 600-action budget, with the B0-33 groups' 1 to 10 k alarms each. The wall was already half the period.

So the 620 s was not the sweep's normal cost: it was one 4.5-minute fill burst plus serial lookups on large finished
groups, and the next two runs show the failure it was heading for, a dead chain that wedges the cron.

**Bound, on branch `sweep-wall`** (design first: `design/DESIGN.md`, Shape, the sweeper paragraph;
`decisions/DECISIONS.md`, `sweep-wall`; `decisions/OPEN-QUESTIONS.md` R25):
- Once a chain's lookup fails, the run asks it no more: later packs of that chain are answered as failed lookups,
  kept and alarmed `LookupFailed`. The asks already in flight (at most 16) still run, so a dead chain costs about one
  LOOKUP_FOR of wall time.
- SWEEP_WALL, 8 minutes [guess] of the run's own clock: past it the run asks no chain and starts no network, and
  returns with the next network as its cursor, so the caller stores it and the fold audit runs. A network in flight
  at the wall still runs the actions it was answered (131296's fills took about 270 s, inside the 7 minutes left).
- The report gains `dead` and `walled`.
- Tests: `a_chain_whose_lookup_failed_is_asked_once_a_run` (30 orphans on a dead chain at 20 s each: one ask, 30
  alarms, the next network still swept) and `a_run_stops_starting_networks_at_its_wall` (3-minute lookups: networks
  1–3 swept, the second lookup of 3 refused at 9 minutes, cursor 4, the next run sweeps 4; and a lookup at exactly the
  wall refused). Both fail on main.
- Plants (`tests/sweepwall/plant.py`), 8 of 8 caught: main's ask-every-pack with no wall; a failed chain asked again;
  a failure not remembered; rows asked past the wall; networks started past the wall; `>` for `>=`; the walled run
  dropping its cursor; the deadline from 0 rather than the run's start.

Open (R25): a retired test group's packs stay in the buckets, so each run still asks each retired chain once and
alarms its packs. Whether the teardown should delete the network's prefixes, or the sweeper treat a chain whose
every member answers "not joined" as having no group, is for the operator.

### Retired test networks purged (2026-11-17, R25 settled; the operator's one granted hosted action)

The operator settled R25: the test teardown deletes a retired network's bucket objects, and the sweeper does not treat
an all-"not joined" chain as having no group, because a live group mid-placement or briefly unreachable can look the
same. Built on branch `retire-purge`:
- `tests/fanout/purge.py NET...`: per network, it asks `/status` of `repo:NET`'s voter on each site, under the registry
  row's name and the first generation's, and refuses the network unless every answer is "not joined" (a failed status
  refuses too). It then lists and deletes every key under the network's prefixes (`pack/`, `committed/pack/`,
  `upload/`, `compose/`, `clone/`, `archive-cache/`, `blob/`, `committed/blob/` by network; `archive/`, `snapshot/`,
  `inline-idx/` by `repo:` and `repo-meta:` chain) in each site bucket through R2's S3 API, 1,000 per call, and lists
  them again for the after count.
- `tests/fanout/idleretire.mjs` runs it once every member it retired answers "not joined" (`NO_PURGE=1` skips).
- `tests/fanout/test_purge.py`: 3 tests (the gate; every kind of the network deleted and none of 13132, 1313220,
  131323's; a dry run deletes nothing). `tests/fanout/plant_purge.py`: 8 of 8 caught.
- The slot rules in `tests/fanout/README.md` gain the purge as close-out step 3.

**The cleanup pass**, 08:23:41–08:56:34Z on graft-m2b, over 131297–131348 (`tests/fanout/raw/purge-m2b.log`):
**20 networks purged, 202,101 objects deleted, every bucket 0 after; 32 refused, nothing of them deleted.**

| Network | a before → after | b before → after | c before → after |
|---|---|---|---|
| 131322 | 6,549 → 0 | 6,555 → 0 | 6,554 → 0 |
| 131323 | 5,988 → 0 | 5,984 → 0 | 5,963 → 0 |
| 131324 | 153 → 0 | 155 → 0 | 154 → 0 |
| 131325 | 157 → 0 | 159 → 0 | 158 → 0 |
| 131326 | 169 → 0 | 173 → 0 | 172 → 0 |
| 131327 | 183 → 0 | 185 → 0 | 184 → 0 |
| 131328 | 187 → 0 | 189 → 0 | 188 → 0 |
| 131329 | 8,025 → 0 | 8,009 → 0 | 7,969 → 0 |
| 131330 | 5,258 → 0 | 5,258 → 0 | 5,232 → 0 |
| 131335 | 962 → 0 | 961 → 0 | 961 → 0 |
| 131337 | 2,247 → 0 | 2,251 → 0 | 2,250 → 0 |
| 131338 | 1,270 → 0 | 1,274 → 0 | 1,270 → 0 |
| 131340 | 4,405 → 0 | 4,407 → 0 | 4,406 → 0 |
| 131341 | 710 → 0 | 712 → 0 | 710 → 0 |
| 131342 | 6,325 → 0 | 6,324 → 0 | 6,325 → 0 |
| 131343 | 3,685 → 0 | 3,453 → 0 | 2,509 → 0 |
| 131344 | 7 → 0 | 9 → 0 | 3 → 0 |
| 131345 | 8,515 → 0 | 7,454 → 0 | 6,153 → 0 |
| 131347 | 8,719 → 0 | 7,972 → 0 | 5,508 → 0 |
| 131348 | 7,915 → 0 | 7,241 → 0 | 5,278 → 0 |
| **20 networks** | **71,429 → 0** | **68,725 → 0** | **61,947 → 0** |

Refused (some member joined, so not retired; nothing deleted):
- 131297–131321 and 131331–131334, 131339: never retired. They are earlier LM-1 rows' groups (B0-32 131305, B0-33
  131307 and 131313, B0-31 131306 and 131312, and so on) whose members are still followers at term 2 to 5. The
  operator's list said "already-retired B0-33 networks 131297–131348"; only 131322 to 131348 were retired, and of those,
  not every one.
- 131336 and 131346: retired on 11-16 and at m42 with "11 of 11 not joined", but site 1's current voter (a later
  generation than the retire list named) still answers follower. The gate found the partial retirement that the old
  log's count missed.

So the sweeper still meets 32 live finished groups' packs in 131297 to 131346. They answer, so each costs normal
lookups, not LOOKUP_FOR; 131307 and 131313 alarm about 8.4 k packs each a run. Retiring and purging them is a
teardown the operator can grant; it is not done.

Meter (`acctcost.py`): graft-m2b $0.01 for 08:23:39–08:57:12Z; the account $1.86 in that window, and $108.34 for the
UTC day at 08:57Z (graft-m1's B0-30 close-out is the rest). R2 deletes are free operations; the lists are class A.

### The 32 live finished groups retired and purged (2026-11-17, operator-granted teardown)

The operator granted retiring and purging the 32 groups the first pass refused: 131297–131321, 131331–131334, 131336,
131339 and 131346.

**Reuse check.** None is reused. Every one is a finished run's repository in `$M3_STATE/am1-runs-repos.txt`
(`fm20s250` = 131297 through `fm42b33-b033` = 131346). 131316 is the m23 create that failed in setup. Rows name
fresh repositories `f$TAG-<row>`, created by `tests/am1/runs/mkrepos.mjs` (`POST /orgs/m3org/repos`) under new ids, so
the next B0-33 creates its own. No script names any of these ids.

**Retire by current names** (`tests/fanout/retirenets.mjs`, new). The old retire lists named the members a run had when
it was listed. 131336 and 131346 kept a later-generation voter joined (`repo:131336#8#f1` on the list, a newer one
on site 1). Per network the script:
1. collects each site's registry-row voter, each site's first-generation voter, and every voter and learner of the
   newest committed config any of them answers;
2. sends `/retire` to each one still joined;
3. checks `/status` again.

All 32 ended with "all not joined", from 4 to 13 names each (`tests/fanout/raw/retirenets-m2b.log`). On 131346 one
member was still joined, and it was retired.

**Purge**, `purge.py` with its gate unchanged (`tests/fanout/raw/purge-m2b.log`), 09:03–09:45Z:
**32 networks purged, 93,107 objects deleted, every bucket 0 after.** One transient refusal: 131303 at 09:37:48Z,
when a registry read timed out in the TLS handshake. The gate treats a failed read as not retired. It passed on the
retry at 09:45Z. 131316–131318 held no objects. The retire run's first service was stopped at 09:08Z (see the meter
below) after retiring 131298–131309; those 12 were purged in a separate pass.

| Network | a before → after | b before → after | c before → after |
|---|---|---|---|
| 131297 | 125 → 0 | 127 → 0 | 126 → 0 |
| 131298 | 661 → 0 | 663 → 0 | 662 → 0 |
| 131299 | 3 → 0 | 5 → 0 | 4 → 0 |
| 131300 | 195 → 0 | 197 → 0 | 196 → 0 |
| 131301 | 627 → 0 | 629 → 0 | 628 → 0 |
| 131302 | 185 → 0 | 187 → 0 | 186 → 0 |
| 131303 | 201 → 0 | 203 → 0 | 202 → 0 |
| 131304 | 3,295 → 0 | 3,296 → 0 | 3,296 → 0 |
| 131305 | 680 → 0 | 682 → 0 | 681 → 0 |
| 131306 | 2,540 → 0 | 2,542 → 0 | 2,534 → 0 |
| 131307 | 4,580 → 0 | 4,583 → 0 | 4,554 → 0 |
| 131308 | 707 → 0 | 711 → 0 | 709 → 0 |
| 131309 | 3 → 0 | 7 → 0 | 6 → 0 |
| 131310 | 811 → 0 | 813 → 0 | 810 → 0 |
| 131311 | 1,109 → 0 | 1,111 → 0 | 1,110 → 0 |
| 131312 | 2,191 → 0 | 2,190 → 0 | 2,181 → 0 |
| 131313 | 3,291 → 0 | 3,291 → 0 | 3,287 → 0 |
| 131314 | 1,014 → 0 | 1,016 → 0 | 1,015 → 0 |
| 131315 | 4,216 → 0 | 4,220 → 0 | 4,216 → 0 |
| 131316 | 0 → 0 | 0 → 0 | 0 → 0 |
| 131317 | 0 → 0 | 0 → 0 | 0 → 0 |
| 131318 | 0 → 0 | 0 → 0 | 0 → 0 |
| 131319 | 221 → 0 | 223 → 0 | 222 → 0 |
| 131320 | 3 → 0 | 5 → 0 | 4 → 0 |
| 131321 | 225 → 0 | 227 → 0 | 226 → 0 |
| 131331 | 3 → 0 | 5 → 0 | 4 → 0 |
| 131332 | 103 → 0 | 105 → 0 | 104 → 0 |
| 131333 | 323 → 0 | 325 → 0 | 324 → 0 |
| 131334 | 833 → 0 | 835 → 0 | 834 → 0 |
| 131336 | 841 → 0 | 843 → 0 | 842 → 0 |
| 131339 | 3 → 0 | 5 → 0 | 4 → 0 |
| 131346 | 2,038 → 0 | 2,042 → 0 | 2,025 → 0 |
| **32 networks** | **31,027 → 0** | **31,088 → 0** | **30,992 → 0** |

With the first pass, 131297–131348 are retired and purged: 52 networks, 295,208 objects.

**The meter, and a finding.**
- graft-m2b cost $1.22 for 09:02–09:46Z. $1.15 of it is one object: `repo:65536#1#f1` on site b wrote 1,144,665
  rows in the minute 09:06Z. The retire work itself wrote a few hundred rows a minute.
- That minute is site a's `*/15` fold audit (trace 06fae83a). Its sample reached `repo:65536`, an old 31.6 MB network
  that no test touches. The leader refolded the chain into scratch tables from its 31,608,994-byte core snapshot
  (`fold audit refold`, 16 batches, 31.6 MB, index 7,335), and site b's voter wrote 1.14M rows for it.
- The audit is daily per chain by design (`AUDIT_EVERY_MS`, 24 h, `workers/site/src/load.rs`), so this is not a loop.
  But one refold of that chain costs about $1.15 a day, and the same audit hit `SQLITE_LOCKED` on 327699, 327700 and
  65537. I stopped the retire run at $1.16 to find the source before going on, because it was not the retire work.
- Open finding for the operator: a refold's scratch tables write one row per fold row of the chain's snapshot, so
  the audit's cost grows with the chain. Options: cap the audited chain's snapshot size, sample hot chains only, or
  retire old test networks like 65536 to 65540 as well.

**Idle reading**, 09:45:57–10:15:57Z, 30 minutes after the last purge (`PREFIX=graft-m2b acctcost.py`):
- graft-m2b cost **$0.01** (about $0.02 an hour). The account cost $0.04 in the window, mostly graft-m4a. The UTC day
  stood at $109.74 at 10:16Z.
- Site a's sweep, on the site script still deployed (from 68d39927, without sweep-wall), now **completes in 70 s and
  87 s** (09:45 and 10:00 runs, 20 networks swept, 0 planned). Before the purges its 06:00 and 06:15 runs were
  cancelled at 900 s. Removing the retired groups' packs alone unwedged it.
- `names:*` rows: 0 in both slots. Sites b and c took 28 to 33 s.

### 65536–65540 and 131072–131073 left in place: every one is a fixture (2026-11-17, no spend)

The operator granted retiring and purging these seven networks on graft-m2b, on one condition: first check that no
row script reads any of them as a fixture. **Every one is a fixture, so I retired and purged none of them.**

| Network | Name | Read by |
|---|---|---|
| 131072 | `m3org/j4` | `$M3_STATE/m3.json` `.repo`, loaded by `tests/am1/hosted/lib.mjs` as `state.repo`. Used in `tests/am1/hosted/feed.mjs` (L0-15 and LF-3; the foreign cursor is `state.repo + 1`, which is 131073), `stall.mjs`, `smoke.mjs`, and `tests/m3/rows/bf2.mjs` and `bf31.mjs` |
| 131073 | `m3org/j4b` | `tests/m3/rows/bf31.mjs` (`state.repo2`, the repository outside the token's scope), and the cursor above |
| 65536 | `m2b/bf20` | `~/.graft-m2b/repos.json`. Read by `tests/m2/wp3/hosted.py`, `wp6/p99.py`, `tests/m1/wp8/bf14.ts`, `f43-*.sh`, `tests/m1/integ/bf12.ts`, and `tests/m3/wirefix/*-bf20.sh` |
| 65537 | `m2b/smoke` | the same `repos.json`; `tests/m3/wp5/b0-18.ts` (`--repo 65537`); `tests/m2/sitedays/shards.sh` (`smoke:65537`) |
| 65538 | `m2b/bf24` | the same `repos.json`; `tests/m2/bf24/bf24.py` (default `BF24_REPO=bf24`) |
| 65539 | `m2b/bf29` | the same `repos.json`; `tests/m3/wp5/bf29.sh`; `b0-18.ts --repo 65539` (the B0-18 rerun in BUILD-PLAN M3-WP5) |
| 65540 | `m2b/bf29r` | the same `repos.json`; BF-29 on a fresh repository (`tests/m3/results/m3-wp5.md`) |

All seven still have a joined group: followers at term 2 to 5 on every site, read by current registry name.

Two findings went to `decisions/OPEN-QUESTIONS.md`:
- **K10**, the daily refold's cost. The 09:06Z refold of 65536 wrote 1,144,665 rows, about 2 rows per row of the
  core tier's 571,889. The audit's own caps (4 chains per site per run, once a day per chain) bound it at 1,152
  refolds a day.
- **F82**. Site a's refolds keep reporting `SQLITE_LOCKED` after `audit-locked`: 39 chains in a row from 04:47 to
  10:17Z. The three reports read in full carry `at` times from 2026-11-16 00:20–00:35Z, before the site day, so
  these may be stale reports of jobs that will not run again until each is a day old. That is not yet confirmed.

The cheaper path the operator named, retiring 65536 so its next audit never runs, would break those fixtures. Re-seeding
65536 smaller, or leaving its daily refold in place, is for the operator.

The operator kept 65536 and its daily refold (about $1.15 a day; K10's only measurement), and asked for F82 to be
settled from a later free reading.

### F82 settled: the SQLITE_LOCKED reports were stale

A reading from Workers Logs and DO analytics at 10:41Z on 2026-11-17, with no hosted action:

- From the m42 site day (03:20Z) to 10:41Z, all three `graft-m2b` sites log 0 `SQLITE_LOCKED` lines.
- Site a logged 43 "fold audit refold" completions, all without error. The DO analytics name 41 of them. 37 are on
  chains whose stale reports said `SQLITE_LOCKED`: 18 of `repo:327680` to `327700`, 65538, 65539, 9043968 to 9043970,
  131072, 131107, and 131111 to 131122. The other 4 are fresh groups: 131331 to 131333 and 131339.
- Site a's 10:32Z audit report still shows `SQLITE_LOCKED` on 131119 to 131122. Those error rows carry `at` 2026-11-16
  01:52 to 02:07Z, and the same four chains refolded cleanly at 10:02 and 10:17Z. The report keeps the old row until
  its new one lands.
- The only "fold audit failed" lines after the site day are three in site a's cron at 03:28 and 03:43Z, all "Too
  many API requests by single Worker invocation". That is the sweeper's subrequest limit that sweep-quiet and
  sweep-wall address, not the refold.

So `audit-locked` holds, and F82 is marked settled in `decisions/OPEN-QUESTIONS.md`.
