# `hot-shards-4`: delta-searching repacks, fsck behind the repack, 1 MB pages under load, and the kill rerun (2026-11-13)

Branch `hot-shards-4`, from `hot-shards-3` `29b0d13`. Follows `tests/am1/results/hot-shards-3.md`, "Merge with main, and the follow-ups". Every hosted number is on standard-4 shard containers.

## Changes

| Open item | Change |
| --- | --- |
| `--window=0` disk growth | `runRepack` searches deltas with git's default window; the split commit-graph stays |
| site c's `fsck` `ok: false` | `/fsck` waits for a running repack (`repacking`), as evict does |
| F47-shaped trap on a 4 MB page | A local `/entries` page is at most `ENTRIES_BUSY_PAGE_BYTES` (1 MB) while `ENTRIES_BUSY_READS` (1) or more other reads are in flight on the member, and up to 4 MB on an idle one. `/stats?summary=1` counts capped pages as `busyPages` |

## Tests and plants

- `containers/git/server/repack_test.go`, `TestRefsRepacksPastRepackAt`: 60 versions of a 4 KB file, one line changed each, in one pack each; after the repack at least 30 objects are deltas.
- `containers/git/server/convoy_test.go`, `TestRepackHoldsNoNetworkLock`: a `/fsck` sent while a repack is held does not answer until it ends, then answers ok.
- `workers/site/src/host.rs`, `only_a_local_entries_read_gets_a_larger_page`: one read in flight caps the page at 1 MB, and an idle member serves 4 MB.
- Plants 56 to 58 (`tests/am1/hotshards/plant.py`): a repack with `--window=0`, fsck alongside a repack, a busy member serving 4 MB pages.
