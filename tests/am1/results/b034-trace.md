# `b034-trace`: why k stayed low, the follower's /fanout, and clone TTFB (2026-11-18)

Branch `b034-trace`, from main `7430a479`. Code-only. The split with LM-1 (T-01a1c694-6112-72b4-83ca-dc9337a9646e): this branch owns k and fan-out, the composable branch of the shard's `/upload-pack`, and the a1 driver. LM-1 owns `caught_up`, `Jobs`, `indexIngest`, and the non-composable ls-refs path, as (a) the edge answers a v2 ls-refs from the leader's refs and (b) `index-pack` runs outside the network's exclusive lock.

## Summary

- **The operator's figures (k = 1, clone TTFB p99 172 s, 0 of 37 human pushes) are from the Nov 10 run.** The latest run is LM-1's m41 on `graft-m2b` (`tests/fanout/raw/m41/`, `b034-m41.json`, `misses-m41.txt`), and this trace reads it.
- **k did rise in m41, but only to 3, and only at 888 s of the 900 s row.** The pool slots finished rehydrating at 22:13:32 to 22:14:49Z. Of 59 pool warms after 22:15, 34 answered 503 (`warming`), because a pool warm waited for a catch-up within its 2 s, and catch-ups under load took 1 to 14 s. `warm_step` steps only when every slot answers warm, so one slow catch-up held k.
- **The follower's /fanout was not failing in m41.** All 55 `/fanout` calls answered 200. The second and hostile networks ran at 0.79 vCPU or less, so k = 1 is correct for them.
- **143 of 441 clone-cache misses were 409 NOT_YET** (the tip had just moved). Every one was composed in the end, but each waited for the running catch-up and then one more: first byte after the 409 p50 3.1 s, p90 10.1 s, p99 13.1 s. In 80 of them, a catch-up ended before theirs did.
- **The a1 driver inflated its own latencies.** `tip:move` ran `git` synchronously, which blocked the event loop for the length of each push. With the blocked windows removed, client clone TTFB moves from p50 2.6 s, p90 12.7 s, p99 25.9 s to about 1.3 s, 5.5 s, 9.7 s [inference: an estimate from the row's own timestamps, not a rerun].
- **Three fixes, each with a test that fails on main.** 9 of 9 plants are caught. The full checks pass.

## Trace

| Question | Evidence (m41, repo:131296, 22:08 to 22:23Z) | Finding |
| --- | --- | --- |
| Why k stayed at 1 for most of the row | Pool slots rehydrated by 22:14:49Z. 34 of 59 warms after 22:15 answered 503. `warm_step` needs every slot warm. k reached 3 at 888 s | The pool warm's rule, "caught up within 2 s", fails under load for a reason unrelated to whether the slot can serve: a slot behind the head still serves, and a forwarded push it lacks a base for returns to the first shard (`on_catch_up`) |
| The follower's /fanout | 55 of 55 `/fanout` answered 200 | No error in m41. The Nov 10 error does not reproduce on main |
| k on the second and hostile networks | ≤ 0.79 vCPU each | k = 1 is correct |
| Clone TTFB on a miss | 143 of 441 misses were 409 NOT_YET; all composed later. After the 409: p50 3.1 s, p90 10.1 s, p99 13.1 s | A NOT_YET miss waits for a catch-up that starts after it, though the running one would usually bring its want |
| Client-side numbers | `tip:move`'s synchronous `git` blocked a1's event loop; a 3 s git call blocks it 3,018 ms sync and 32 ms async (local check) | m41's client latencies are inflated. Rows must report their own loop lag |
| Catch-up length | Catch-up `lock_ms` up to 13.5 s for packs of 0.1 to 2.3 MB. Site a's shard at 11.1 to 11.9 GiB of 12 GiB | LM-1's (b). Memory pressure is a hypothesis, not checked |

## Fixes

1. **A pool warm is answered from the slot's own state** (`graftyard_git::catchup_answer`, `CatchupAnswer`; `workers/site/src/shard.rs`, `/catchup`). `Caller::PoolWarm` starts a catch-up in the background and answers at once: 200 when the container runs, holds a cursor for its own disk, and no rehydrate of the network is under way; `warming` otherwise. Every other caller still waits for a catch-up. `warm_step` reads the answer as before. Rejected: a longer warm `wait` (it still ties k to catch-up length) and "a cursor" alone (a rehydrate saves its cursor before its tail).
2. **A composable miss is composed again after the running catch-up** (`graftyard_git::compose_then_catch_up`, `AfterCompose`, `NOT_YET`; the composable branch of `/upload-pack`). After a 409 whose reason is `NOT_YET` while a catch-up runs, the shard waits for that catch-up (`jobs.idle()`) and asks `compose=only` again. On 200 it serves composed and fills; otherwise it falls through to `caught_up()` as before. Any other 409 reason, or no catch-up running, behaves as on main. Best case on m41's misses [inference]: p50 2.4 s, p90 9.0 s, p99 12.5 s after the 409, a modest gain bounded by catch-up length.
3. **The a1 driver no longer blocks its loop** (`Git.runAsync` in `tests/am1/hosted/lib.mjs`, `tip:move` in `tests/am1/rest/a1.mjs`). Each row records `loopDelay {p50, p99, max}` from `monitorEventLoopDelay`. `tests/fanout/loopstall.py ROWS [MISSES]` measures the stall in past rows.

Design: `design/DESIGN.md`, Shape, `HOT_POOL` (the warm rule) and the composed-miss paragraph. Decisions: `decisions/DECISIONS.md`, "`b034-trace`: a pool warm is answered from the slot's own state…", with the three sub-decisions and their flips. The agent block is unchanged.

## Tests and plants

| Test | Fails on main because |
| --- | --- |
| `crates/git/tests/shard.rs`, `a_pool_warm_is_answered_at_once_and_only_a_finished_rehydrate_is_warm` | main waits for a catch-up on a pool warm |
| `crates/git/tests/shard.rs`, `a_miss_on_a_just_moved_tip_is_composed_after_the_running_catch_up` | main always waits for a later catch-up after a 409 |
| `crates/git/tests/agent_load.rs`, `a_rehydrated_slot_steps_k_under_load_whatever_its_catch_up_takes` | under main's rule k stays 1 for the whole simulated run |

`python3 tests/b034fix/plant.py`: 9 of 9 caught; `--residue` OK. Plants 0 and 5 are main's two rules. The others: a cursor mid-rehydrate counts as warm, a stopped container counts as warm, no cursor needed, every caller answered at once, any 409 reason composed again, composed again with no catch-up running, and composed again before the running catch-up ends.

Full checks: fmt, workspace clippy, wasm32 clippy for site and edge (with and without `test-faults`), ops, front and runners, `cargo test --workspace`, `go test` in `containers/git/server`, the three `scripts/check_*.py`, and the plant residue checks.

## LM-1's half, proposed and not built (T-01a1c694-6112-72b4-83ca-dc9337a9646e)

Recorded here so the proposal is whole. LM-1 owns both items and will build them on its own branch.

- **(a) The edge answers a v2 `ls-refs` from the group's ref read** (`read_refs`, `workers/edge/src/lib.rs`, the path receive-pack's advertisement already uses), with stock's prefix filtering, `symrefs`, `peel` and framing. Only `fetch` goes to a shard. Trace: on m41, 14 of 23 and 14 of 25 shard ls-refs on repos 327700 and 9043968 took over 5 s, p50 23 to 26 s, waiting on the rehydrate (22:15 to 22:19Z) and on `caught_up()` behind ingests. Tests: a byte-diff against stock v2 ls-refs (branches, annotated and light tags, HEAD symref, `ref-prefix`), an edge test where the shard never answers, and plants. It checks the leader's read load first (about 33 ls-refs a second against m45's about 340 `/read` a second) and needs a design line under "Commit and read paths" before code.
- **(b) `indexIngest` runs `index-pack` under `RLock` into a quarantine directory** (as `/validate` does), and takes `Lock` only to move the pack, `record` it, and write the midx. Trace: `lock_ms` 5.5 to 13.5 s for 0.1 to 2.3 MB packs, with every reader queued behind it. Tests: a Go test where a reader gets in during a slow index step, plus plants. Caveat (LM-1's): if memory pressure makes `index-pack` slow, (b) frees the readers but does not shorten the ingest, so catch-up p99 and fix 2's bound may stay high. Nobody has measured that yet.

Neither item touches `caught_up` or `Jobs`, and neither does this branch: fix 2 only calls the existing `Jobs::idle()` and reads `RUNS`. The two halves edit different code.

## A run of this half alone on `graft-m1`

LM-1's (a) and (b) may come later. This half runs on its own.

**It can prove:**
- k steps soon after the pool rehydrate, not at the end of the row.
- The true client numbers, because each row reports `loopDelay`.
- The change in TTFB on the 409 path, from the shard's miss lines.
- `/fanout` health through the row.

**It can't prove:**
- Human push p99 ≤ 3 s beyond transfer. The push's `tip()` is a v2 ls-refs that still waits on the shard's `caught_up()` (LM-1's (a)).
- Short catch-ups under load (LM-1's (b)). Catch-up length bounds fix 2's gain.
- Clone TTFB p99 ≤ 1 s. It will likely still fail while catch-ups take up to 14 s.
- The cold-warm 120 s line, which the rehydrate's length decides.

So the run would show whether this half moves k and the 409 path. It would not meet B0-34 alone. A combined run with (a) and (b) is the one that can test the human push and TTFB lines. It needs the same steps and the same estimate, because neither item adds hosted work beyond a site day.

**Estimate on `graft-m1`, about $6 to $10 and 4 to 5 hours:**

| Step | Cost |
| --- | --- |
| Site day from the branch | about $1 |
| Seeds: the 5 GB monorepo (about 1.7 h of pushes on m2b) and four git/git networks. graft-m1 has no B0-34 seeds, and this orb needs the bare clones of linux.git and git.git first | about $1 to $2 |
| Drain (round 22's figure) | about $1.6 |
| Warm plus the 900 s row (m41's figure) | about $2.2 |
| Sweep and the 30-minute idle reading | under $1 |

**Stop to hold me to:**
- graft-m1 past $12 from the site day;
- or the account's UTC day past $180;
- and start only while the account is under $170.

At 00:16Z on 2026-11-18 the account was at $1.28 for the day and graft-m1 at $0.02.
