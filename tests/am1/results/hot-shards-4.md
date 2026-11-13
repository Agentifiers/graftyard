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

## Local A/B: default window against `--window=0`

`/tmp/b029prof/driver.py`: one container server pinned to one CPU, 300 s each, journey 9's push mix (about 250 validates and 2,000 upload-packs a minute, 500 PR branches seeded). `a0` is the server with `--window=0` (`67db2c3`), `a4` is this branch's (`35c398c`). Both runs did 25 repacks.

| | `--window=0` | default window |
| --- | --- | --- |
| repack CPU over 300 s | 1.6 s | 11.2 s |
| repack wall, total / p50 / max | 2.3 s / 57 ms / 424 ms | 15.8 s / 280 ms / 4,378 ms |
| validate p99, minutes 1 to 5 (ms) | 83, 83, 110, 114, 105 | 222, 194, 242, 310, 583 |
| ingest p99, minute 5 | 48 ms | 709 ms |
| pack bytes at the end | 116.1 MB | 114.3 MB |

The repack no longer holds the network lock, but on one CPU it still competes with validates for CPU: minute 5's 583 ms p99 is the minute of the 4.4 s repack. Journey 9's pushed blobs are random and do not delta, so here the window saves only 1.5 % of pack bytes. Real history is where it saves disk (`hot-shards-3.md`: 1.24× against 2.07× of pushed bytes). A standard-4 container has four vCPUs, so the hosted run below shows whether this cost shows up in push p99.
