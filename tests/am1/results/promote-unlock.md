# promote-unlock: why B0-31's MergeQueue merges answered `warming`

B0-31 m3fr6 (2026-11-15, `graft-m3f`, repository 65568, run `mwic5as1`, apac leader on site c) refused 11 MergeQueue merges with 503 `the git cache: … {"warming":true}`; m3fr4 refused 31. The decision is in `decisions/DECISIONS.md`, "`promote-unlock`".

## What the apac shard logged

Workers Logs on `graft-m3f-site-c`, entrypoint `GitShard`, object `dfed94d6…` (the network's first shard on site c), 21:23Z to 21:34:30Z [measured]:

- **MergeQueue merges** (`/merge?method=`): 60 calls. 18 waited the full 15 s and answered 503; 42 answered 200. The merge queue's answers at the client were 16 × 200, 5 × 409 and 11 × 503; the shard's 18 include the queue's own retries.
- **Three 60 s stalls**, all starting at a `/promote`:

| `/promote` arrived | Its `derived=1` validate | Catch-up running | Merges that waited out 15 s |
|---|---|---|---|
| 21:24:47.826Z (answered in 1,088 ms) | 21:24:48.975Z, 503 after 59,982 ms | from 21:24:48.957Z: 60,371 ms for 4 packs, index-pack 82 ms; then 21,795 ms for the 248 packs queued behind it (114 entries, 260 MB) | 7 (21:25:13Z to 21:26:00Z) |
| 21:29:22.495Z (410 ms) | 21:29:22.958Z, 503 after 59,993 ms | four `/catchup` requests from 21:29:24Z all answered at 21:30:24.535Z | 6 (21:29:26Z to 21:29:58Z) |
| 21:31:31.340Z (420 ms) | 21:31:31.814Z, 503 after 59,992 ms | four `/catchup` requests from 21:31:42Z all answered at 21:32:33.439Z | 5 (21:31:34Z to 21:32:10Z) |

15 of the 18 refused merges overlap a stall. The other 3 started between 21:25:54Z and 21:26:01Z and waited on the 21.8 s run that drained the first stall's queue.

- **Outside the stalls:** 105 catch-up runs on this object, p50 1,041 ms, p90 2,844 ms, p99 5,878 ms (the 21.8 s and 60.4 s runs are the stalls). Merges not overlapping a stall: n 41, p50 472 ms, p90 2,143 ms.

So the shard's catch-up was not slow under the 1 MiB agent pushes. It was blocked for 60 s three times, and each block began at a promotion.

## The cycle

1. `/promote` (`containers/git/server/main.go`, `handlePromote`) takes the network's lock shared, runs `pack-objects`, and writes the pack to its caller while still holding the lock.
2. The caller is the archive's promotion (`workers/site/src/archive.rs`, `promote_on`). It tees the pack to the buckets and to the shard's own `/validate?derived=1` through a one-chunk pipe (`crates/git/src/store.rs`, `Pipe`).
3. That validate catches up first, because a promotion is derived work (`graftyard_git::validate_on`; `workers/site/src/shard.rs`, `"/validate"`). It reads none of the pack until the catch-up ends.
4. The catch-up's `/ingest` takes the network's lock alone (`indexIngest`). It waits for `/promote`'s shared lock.
5. Once the pipe and the sockets fill, `/promote` blocks on its write. Go's `RWMutex` makes every later shared lock (validates, merges, ls-refs) wait behind the waiting writer.
6. Nothing moves until the validate's `PROMOTE_WAIT_S` (60 s) ends with 503. The tee's pipe then stops, `/promote` finishes its write and releases the lock, the 4-pack ingest completes, and the queued catch-up runs.

Steps 1 to 5 are read from the code; the timings above fit them [inference].

## The fix

`handlePromote` releases the lock once `pack-objects` has returned, before it writes the pack (`promote-unlock`). Merges keep `MERGE_WAIT` (15 s). The numbers do not call for a MergeQueue merge that waits out its catch-up: with no stall, the catch-ups that hold a merge are the p99 5.9 s runs, under `MERGE_WAIT` [inference].

**Regression test:** `TestPromoteWritesItsPackOutsideTheLock` (`containers/git/server/promote_lock_test.go`). It promotes an 8 MB pack, reads 4 KB of it and stops, then asks an `/ingest` and a merge. On main both waited past the test's 20 s limit ("an /ingest or a merge waited on a promotion whose reader had stopped reading"). With the change they answered and the test passed in 0.9 to 1.1 s, three runs out of three. It also checks that a promotion refused 409 leaves no lock behind, and that the stalled reader then gets the rest of the pack.

**Plants:** `python3 tests/m0carried/plant21.py`, 3 of 3 caught: the pack written under the shared lock, the lock released twice, and a refused promotion that keeps the lock.

## m3fr4 is not this

m3fr4's B0-31 (2026-11-14, 16:06Z to 16:16Z, repository 65559) also had 15 s merges on the apac shard (object `229b7170…`), 50 of them answering 503. But there was no `/promote` and no `derived` validate in that window. Its catch-ups were 14.4 s, 40.6 s, 92.5 s and 238.8 s for 27, 65, 159 and 388 packs: about 0.6 s a pack, before `stored-fetch` parallelised the fetch. That cause was fixed by later branches. Today's runs ingest the same packs in about 40 ms each.

## Not measured

- Whether B0-31's MergeQueue p99 now meets 3 s. That needs the hosted rerun.
- Whether another derived call (`/idx` after the tee, or a rehydrate's `/connected`) holds the lock while waiting on a catch-up. Read from the code, `/idx` reads a file and returns, and `/connected` runs only in a rehydrate, which no caller of it waits on.
