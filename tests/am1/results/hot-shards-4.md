# `hot-shards-4`: delta-searching repacks, fsck behind the repack, 1 MB pages under load, and the kill rerun (2026-11-13)

Branch `hot-shards-4`, from `hot-shards-3` `29b0d13`. Follows `tests/am1/results/hot-shards-3.md`, "Merge with main, and the follow-ups". Every hosted number is on standard-4 shard containers.

## Changes

| Open item | Change |
| --- | --- |
| `--window=0` disk growth | `runRepack` searches deltas with git's default window; the split commit-graph stays |
| site c's `fsck` `ok: false` | `/fsck` waits for a running repack (`repacking`), as evict does |
| F47-shaped trap on a 4 MB page | A local `/entries` page is at most `ENTRIES_BUSY_PAGE_BYTES` (1 MB) while `ENTRIES_BUSY_READS` (1) or more other reads are in flight on the member, and up to 4 MB on an idle one. `/stats?summary=1` counts capped pages as `busyPages` |
| Kill-rerun instruments | The shard's `/status` reports `rehydrating` and the cursor its latest catch-up reached; `tests/am1/hotshards/watch.mjs` samples them with the leader's commit |
| Merge with main `0681a70` (`ce61e42`) | rehydrate-batch's `learn_voters` and the busy-page cap both stay on the local `/entries` path. `/status` reads the cursor by the shard's per-instance `run_key`, the key main moved catch-ups to |

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

## Hosted: the kill rerun on graft-m3c (j9k, 2026-11-13)

Deploy `hs4-deploy` = `hot-shards-4` `ce61e42` (main `0681a70` merged), test-faults, image `sha256:f6d2ef408c766594b541156664de402d3db5107aa480ed3676fe2d62e34fed09`, **`SHARD_INSTANCE=standard-4`**. All of this section's numbers are on standard-4. Site day 09:08 to 09:16Z (versions `3bdee604`, `a4a6e2dc`, `df17dba3`, `60e3f3de`), then edge `b0cad2da` (`--ops graft-m3c-ops`, FORMATS with 32.18). Group m3org/hot-a (repo:65551), the 2.15 GB log. Raw files are gitignored: `tests/am1/raw/hotshards/` (`j9k.log`, `j9k-pushes.jsonl`, `watch4.log`, `faultsk.log`, `warm4.log`, `timeline.txt`) and `tests/am1/raw/hotshards3/fsck-j9k.json`.

- **Cold start:** the site day left all three shards without containers. Their three rehydrates ran at once from 09:18Z. Site a reached the head at about 09:26Z, and b and c by 09:35Z. Site b's rehydrate ran past one 602 s alarm and went on in the next.
- **Load:** j9 at 4.17 pushes/s with 16 in flight, 09:35:15 to 10:00:18Z.
- **Kill:** `/_bed/kill` of site a's shard at 09:45:16.96Z. Site a is the leader's site and the network's home.

### Push latency

| Window | Pushes | Result | p50 | p99 | Max | Over 15 s |
| --- | --- | --- | --- | --- | --- | --- |
| Whole run | 6,237 | **6,237 ok** | 2.92 s | **5.43 s** | 19.6 s | 2 |
| First 3 min | 731 | ok | 2.63 s | 6.35 s | 7.16 s | 0 |
| Warm, before the kill | 2,490 | ok | 2.72 s | 5.35 s | 7.16 s | 0 |
| 10 s before the kill | 42 | ok | 2.76 s | 3.34 s | 3.34 s | 0 |
| Kill to 60 s after | 243 | ok | 2.90 s | 5.90 s | 19.6 s | 2 |
| While site a rehydrated (09:45:17 to 09:54:11Z) | 2,227 | ok | 3.21 s | 5.55 s | 19.6 s | 2 |
| After its mark cleared | 1,520 | ok | 2.55 s | 5.12 s | 5.75 s | 0 |

- Per-minute p99 ranged from 4.2 to 6.7 s, all under 7.2 s. The two 6.7 s minutes are the first two.
- **10 pushes were in flight at the kill. All 10 were ok**, the slowest in 3.26 s.
- No push was refused.
- **The two slow pushes:** two started at 09:45:44.8 and 09:45:45.0Z and took 19.6 s and 19.0 s.
  - Each was a validate on site a's shard with `next=1`. Each waited its full 15 s and answered 503 `warming` at 09:46:05.9 and 09:46:06.0Z, and the edge then validated it on site b.
  - In the same seconds, two of the watch's `/status` calls to that object took 5.6 s each, against 58 ms otherwise.
  - So the object answered slowly while the new rehydrate started. Whether the mark was already readable when those two validates checked it is not traced [inference]. The watch first read `rehydrating: true` at 09:45:47.7Z.
  - It is two pushes of 6,237, and p99 holds.
- Compared with hot-shards-3: warm p99 is 5.35 s here, against j9g's 3.99 s and j9w's 5.01 s. The local A/B above shows the default window adds repack CPU next to validates. Whether that is the difference here is not separated [inference].

### The rehydrating mark holds until the head

The watch row (`watch4.log`, every 5 to 6 s) for site a's shard:

| Time | Leader commit | Site a | Cursor |
| --- | --- | --- | --- |
| 09:45:13.6 | 29,346 | warm, `rehydrating: false` | 29,344 |
| 09:45:19.3 | 29,369 | container gone (killed) | 29,352 (last before kill) |
| 09:45:47.7 | 29,471 | `rehydrating: true` | 29,352 |
| 09:45:47.7 to 09:54:05 | 29,471 to 31,300 | `rehydrating: true` in every row | 29,352 |
| 09:54:11.1 | 31,319 | `rehydrating: false` | 31,318 |

The mark held for the whole rehydrate and the catch-up after it, 8 min 23 s, and dropped only when the cursor was one entry short of the leader's commit, which was the entry being applied. Every later row has site a within 2 to 5 entries of the commit. In j9h, before `5f68216`, the mark dropped after the refs pages while the catch-up still had 338 entries and 38 s to go. That did not happen here.

The rehydrate of the 2.15 GB group under journey 9's push load took 8 min 50 s from the kill to the head.

### Traps and capped pages (Workers Logs, 09:16 to 10:08Z, all three sites)

| Site | `out of bounds` | `entries page capped` | `Subrequest depth` |
| --- | --- | --- | --- |
| a | **0** | 82 | 0 |
| b | **0** | 31 | 0 |
| c | **0** | 2 | 0 |

The same count query on hot-shards-3's window (04:05 to 04:08Z) finds that run's traps: 1 on site a and 1 on site b. So a 0 here is a real 0, not a query that matches nothing. The 1 MB cap held 115 pages to 1 MB, all during the cold rehydrates and site a's rehydrate after the kill.

### fsck after a cold rehydrate

All three shards rehydrated from cold at the site day, and site a rehydrated again after the kill. After the run and a catch-up (cursor 32,665 on every shard), `git fsck` answered `ok: true` with empty output on all three, with 504 refs, identical on every shard:

| Site | Objects in packs | Packs | size-pack |
| --- | --- | --- | --- |
| a | 191,700 | 22 | 263,792 KB |
| b | 191,700 | 22 | 288,316 KB |
| c | 193,020 | 42 | 296,973 KB |

### Teardown

The run script's teardown failed because the script ran without the install's environment, so I ran it by hand at 10:08Z. All three shards of repo:65551 were torn down (`{"torn_down": 3}`, all 200), and `/status` then showed `running: false` on all three. There were no pool shards: k stayed 1.

### Cost

Window 09:08 to 10:10Z:

- Workers and Durable Objects: $0.43 (`usage.py`: 7,727 Workers requests, 229,880 DO requests, 41,812 DO active seconds).
- Containers: 150 standard-4 instance-minutes and 1,303 CPU-s. At `research/prices.json` rates, that is $0.31 with vCPU billed as used, or at most $1.00 if every vCPU were busy.

This run comes to about **$0.74 to $1.43**, and hot-shards-4 to the same, since nothing else here was hosted.
