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
