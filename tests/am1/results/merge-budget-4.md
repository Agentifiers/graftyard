# `merge-budget-4`: merge waits measured, a late stale merge fixed, and the shard size read (2026-11-12)

Branch `merge-budget-4`, from `origin/main` `4265f3d` (merge-budget-3 merged), with `origin/nr-deploy` `2d63c4e` merged so narrow-reads' deployed code stays up. It closes the three items the lead left after `merge-budget-3` (`tests/am1/results/merge-budget-3.md`). Hosted on `graft-m3c`; nothing ran on `graft-m1-*` or `graft-m2b-*`.

## Summary

- **A28's flip, measured: not met.** With first attempts cut to 3 s and up to 44 merges in line at once, 1,993 merges waited p50 ≤ 2.5 s, p99 5.8 s, max 5.8 s from ask to start. None passed `GET_COALESCE_MS` (30 s). The coalescing window stays as it is.
- **Duplicate `PrSync` commits: the inference was wrong.** The leader's stored logs of merge-budget-3's runs hold no request id at two indexes and no (PR, head, base) recorded twice. Commits were already idempotent per (PR, head, base), so the ack-without-entry change is not needed. The logs did show a different bug: in run 1, 32 merges at an older base were recorded over the same head's merge at a newer base. That is fixed, and the hosted run recorded none.
- **Run 2's 7.6 s commit p99 came from a stall, not duplicates.** It traces to one 10.1 s gap between leader entries, during which followers' `/raft` calls took about 3.1 s each. This run's commit p99 under the same fault was 1.09 s.
- **The container size is read.** The Containers API keeps each instance's configuration. merge-budget-3's run 1 shard ran on 4 vCPU, 12 GiB, 20 GB, which is `standard-4`.
- **Plants:** 8 new, all caught.
- **Cost:** $0.35 for the hosted window.

## 1. Merge waits against `GET_COALESCE_MS`

`BudgetStats` gains `waitHist` and `waitMaxMs`. `asked` records each admitted merge's wait, from its ask reaching the shard to its start, which covers the catch-up, the turn and the second look at the budget. The buckets' upper edges are `WAIT_EDGES_MS`: 10 ms to 120 s, with `GET_COALESCE_MS` as one edge, so the count of waits past it is exact. `wait_ms(q)` answers a quantile as its bucket's upper edge, or the largest wait in the last bucket, and `waits_over(ms)` counts waits past an edge. The counts are stored with the rest (merge-budget-3), and `a1.mjs` reports the window's `wait`.

The hosted run reproduced merge-budget-3's run-2 shape on `standard-2` (0.25 vCPU):

```
bash tests/am1/mb4/hosted.sh mb4-w mb4r1 50 10 240 --rest-polls 0.3 --first-for 3000   # repo:65559, 20:06:22Z, 240 s
```

| Measure | merge-budget-4 | merge-budget-3 run 2, for comparison |
| --- | --- | --- |
| First attempts given up at 3 s | 68 | 47 |
| Most merges in line at once | 44 | 69 |
| Merges computed, shed | 1,993, 3,698 | 1,783, 648 |
| **Merge wait p50, p99, max** | **≤ 2.5 s, 5.8 s, 5.8 s** | not measured; `/merge` wall p99 11.4 s, max 11.9 s |
| **Waits past 30 s** | **0** | 0 by the wall-time bound |
| Wait buckets (to 25 ms, 50, 100, 250, 500, 1 s, 2.5 s, 5 s, 10 s) | 0, 51, 21, 49, 135, 423, 1,060, 228, 26 | |
| Lowest bucket, largest merge | −32.0 ms, 43.4 ms (within one merge) | −24.8 ms, 37.8 ms |
| vCPU used of 0.25 | 0.261 | 0.191 |
| GET asks, computes | 1,683, 69 | 1,303, 150 |
| List asks, computes | 3,237, 0 | 2,184, 0 |
| Leader commit p50, p99 | 439 ms, 1,090 ms | 644 ms, 7,578 ms |

The longest wait was a fifth of `GET_COALESCE_MS`, so a GET coalesced on the clock never let a second GET for the same PR queue behind the first. A28's flip ("a merge's wait past `GET_COALESCE_MS`") is not met. Queue-based coalescing is not needed.

The vCPU used, 0.261, is above 0.25 by 0.011. It is the spend over the 240 s window, and merges admitted near the window's end were charged in it. The bucket's own bound held: the lowest was 32 ms below zero against a largest merge of 43 ms.

## 2. Duplicate `PrSync` commits

### What the leader's entries show

I read each run's committed log from its leader (`/log`, test-faults) and decoded every `TestMerged`'s base from its inline pack (the merge commit's first parent):

| | merge-budget-3 run 1 (repo 65553) | merge-budget-3 run 2 (repo 65554) | merge-budget-4 (repo 65559) |
| --- | --- | --- | --- |
| Entries | 1,131 | 689 | 970 |
| Request ids at more than one index | 0 | 0 | 0 |
| `TestMerged` at an exact (PR, head, base) already recorded | 0 | 0 | 0 |
| Same head, newer base (a base move) | 112 | 76 | 3 |
| **Same head, older base than the recorded one** | **32** | 0 | **0** |

So the attempt and its re-send never committed the same merge twice. The content-keyed request id answers a repeat from its request row, and `decide_sync`'s `recorded` check skips a merge already recorded. Making the commit idempotent per (PR, head, base) was already done, and an ack-without-entry path adds nothing.

### Where run 2's 7.6 s commit p99 came from

The largest gap between run 2's leader entries was 10,149 ms (entries 443 to 444, 17:49:38.973Z to 17:49:49.122Z). In that gap the followers' `/raft` calls on sites b and c each took 3.0 to 3.1 s of wall time at about 10 ms of CPU, and the leader's own `/raft` took 3,126 ms at 365 ms of CPU. The next entry carried 70 requests (27 of them `PrSync`) in 586 KB. Commands queued during the gap set the p99. I did not find what stalled the followers; it is not duplicate commits. Under the same fault, this run's commit p99 was 1.09 s.

### The bug the entries did show

In merge-budget-3's run 1, 32 `TestMerged` events recorded a merge at an older base over the same head's merge at a newer one. A merge planned before a base move, such as a first attempt that outlived its deadline, committed after the faster re-send's merge at the new tip. Each event turned `mergeable` back to null and moved `refs/pull/N/merge` to the stale base until the next push. The data structure allowed it: `decide_sync` checked only "same head and not already this exact pair".

The fix, in `PrSync::decide_sync`: once the head has a recorded merge, a merge records only if its base is the base branch's tip. A new head's first merge still records at any base, as before, because a stale merge is better than none.

```rust
let older = have.as_ref().is_some_and(|r| r.head == m.head) && s.get(&base) != Some(m.base);
if head != m.head || recorded || older { continue; }
```

The hosted run recorded 0 such events.

## 3. The container's instance size

The Containers API (`accounts/{id}/containers/applications/{app}/instances`) lists each instance with the `configuration` it ran on, including stopped ones. `tests/am1/mb4/instances.py` prints them.

| Instance | Run | `SHARD_INSTANCE` | Configuration read | Cloudflare's instance type [doc, Containers limits] |
| --- | --- | --- | --- | --- |
| `git#0#net:65553` | merge-budget-3 run 1 | `standard-4` | 4 vCPU, 12,288 MiB, 20,000 MB | `standard-4` (4 vCPU, 12 GiB, 20 GB) |
| `git#0#net:65554` | merge-budget-3 run 2 | `standard-2` | 1 vCPU, 6,144 MiB, 12,000 MB | `standard-2` (1 vCPU, 6 GiB, 12 GB) |
| `git#0#net:65559` | merge-budget-4 | `standard-2` | 1 vCPU, 6,144 MiB, 12,000 MB | `standard-2` |

So merge-budget-3's 1 vCPU budget ran on a 4 vCPU container [measured], not on inference. The readings are in `tests/am1/mb4/instances-2026-11-12.jsonl`. I did not add a container-side probe: the API answers without a new image.

## Tests and plants

- `crates/git/tests/prsync.rs`, `an_older_base_merge_committed_late_does_not_replace_the_current_one`: the re-send's merge at the new tip records; the late merge at the old tip records nothing; the merge ref and `mergeable` stay at the tip's. A new head's first merge at an older tip still records, and the tip's merge then replaces it. It fails without the fix.
- `crates/git/tests/agent_load.rs`:
  - the concurrency test asserts one wait per merge, the first of a burst waiting 0, the longest a whole number of 900 ms turns, and none past `GET_COALESCE_MS`;
  - `merge_waits_are_counted_in_buckets_and_read_against_the_coalescing_window`: known waits give exact quantiles; a wait exactly at 30 s is not past it; an older shard's stats read as no waits; a 3 s catch-up is counted in the wait;
  - the restore test checks the waits survive a restore.
- `a1.mjs --selftest`: the window's wait histogram is the delta on a kept shard and the whole on a restarted one.
- **Plants: 8 new, all caught** (`tests/budget/plant.py`, 53 in all):
  - a merge's wait never counted;
  - the wait timed from after the catch-up (missed at first; the catch-up case in the test now catches it);
  - a wait on an edge lands in the bucket above;
  - waits past 30 s counted from the edge's own bucket;
  - the driver drops the histogram;
  - an older base's merge replaces the current one again;
  - any recorded merge of the head blocks a new one, even at the tip;
  - a new head's first merge blocked when the base moved.

Full checks pass: fmt; workspace clippy; wasm32 clippy for site and edge, with and without `test-faults`, and for ops; `cargo test --workspace`; `go test` in `containers/git/server`; the three `scripts/check_*.py`. No wire enum changed. `BudgetStats` reads older JSON with defaults.

## Hosted

- **Site day** 19:54Z to 20:02Z, from `a3742c7`: main `4265f3d`, `origin/nr-deploy` `2d63c4e` and this branch, with test-faults, image `sha256:7e0c4772…` and `SHARD_INSTANCE=standard-2`. Sites a/b/c are `eb036b6b` / `0eea976c` / `40a5c992`. `graft-m3c-edge` is `e21663a0`, with `--ops graft-m3c-ops` and FORMATS including 32.18.
- Coordinated after narrow-reads and m0-carried-5, with f67-liveness next.
- **Teardown:** 11 shard containers, including hot-shards' leftover pools 1 to 7 and `net:65551`/`65558`. A second pass found none.

### Not caused by this change

- 17 "subrequest depth" errors at 20:07:37Z to 20:08:01Z on sites a, b and c (trace `8c049c94`). They came from a cold catch-up of hot-shards' `repo:65551`, left from its run: `/entries` called once per entry from 4859 to 15553, with `/promote` and the archive drain. The `/raft` calls on b and c failed in the same trace. That is the same ratchet merge-budget-3 fixed for `PrSync`, in another caller; I reported it to hot-shards. None was on this run's repository.
- 2 GraphQL and 1 REST poll answered HTTP 503, against about 1,000 in merge-budget-3's runs; narrow-reads' read change is in this build.

## Design changes and contradictions

1. **Agent block**, kept identical in both files: `TestMerge`'s comment adds "never over the head's merge at the base tip".
2. **`decisions/DECISIONS.md`**, "`merge-budget-4`": a test merge at an older base never replaces the head's merge at the base tip, with Why (the log, not duplicates), Beat (the lead's per-(PR, head, base) ack, which the log did not need), Evidence and Flip.
3. **Contradiction, settled:** merge-budget-3 inferred that run 2's commit p99 came from duplicate commits. The log shows none. The p99 came from one 10 s follower stall, and the real fault was a stale merge recorded late.
4. **Contradiction, settled:** merge-budget-3 inferred the `standard-4` container from the code. The Containers API confirms 4 vCPU.
5. **A28's flip:** measured and not met. The decision stands.

## Cost

$0.35 for 19:53Z to 20:14Z (`INSTALL=graft-m3c python3 tests/fanout/m2bcost.py 2026-11-12T19:53:00Z 2026-11-12T20:14:00Z`): 6,674 Workers requests, 131,561 DO requests and 682 container CPU-s, $0.17 of it containers. The cap was $15. No cost constant moved.
