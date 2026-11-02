# M1-WP4: pack store and R2 keys

Run 2026-11-01 in the Milestone 0 account. The code is `crates/git` (`graftyard-git`). It holds `PackId`, `PackRef`, `DurablePack`, `R2Key`, `Archived`, `Promoted`, `PackStore::place`, the `Committed` marker, and the sweeper. The decisions are pure, and the R2 side sits behind the `Bucket` trait. With the `worker` feature, `host::R2` implements `Bucket` over a Workers R2 binding. No site script hosts any of this yet. The row runs on its own throwaway script, `graft-m1-wp4` (`tests/m1/wp4`), against three real buckets: `graft-m1-wp4-a` (ENAM), `-b` (WEUR), and `-c` (APAC).

## What is built

- `R2Key` and its text form (`crates/git/src/key.rs`). Pack, index, marker, and upload-journal keys start with their network: `pack/<net>/<id>.pack`, `pack/<net>/<id>.idx`, `committed/pack/<net>/<id>`, `upload/<net>/<id>`. The sweeper and the filler therefore list one network with one prefix per kind and never list a chain. Each key has exactly one spelling, and a parse rejects every other spelling. Indices are zero-padded to 20 digits so that ranges sort by index.
- `PackStore::place` (`crates/git/src/store.rs`). It refuses bytes whose SHA-256 or size differs from the `PackIndex` that `validate` returned. A pack within `inline_max` goes inline. A larger pack gets a create-only put (`If-None-Match: *`, with R2 checking the SHA-256) to every bucket at once, and `place` returns as soon as the copies span two regions (`durable_sites`). The rule counts regions, not buckets. A key that already exists counts only when R2 reports the same SHA-256, so a retry of the same bytes counts and a collision does not. Each object carries `chain` and `request` custom metadata. `Copies::Solo` needs one bucket.
- `mark_committed` puts the create-only marker after `Acked`, in each bucket that the pack's `sites` names. The marker body is (chain, index, checksum, bytes), and it is copied into the marker's custom metadata, so a listing carries every body. The checksum and size are additions to the design's "(chain, index)". The filler needs them to verify a copy without reading the chain.
- `sweep::decide` (pure) and `sweep::sweep` (`crates/git/src/sweep.rs`). The rules are in the module comment. In short:
  - A pack with a marker in any bucket is never deleted. A holder without a marker gets one, and a bucket that lacks the pack is filled from a copy that matches the marker's size and SHA-256, and then marked.
  - An unmarked pack whose chain's `request` row names this pack id gets its marker, at any age.
  - An unmarked pack with no row, or with a row that names other packs, is deleted only when two conditions hold. It must be at least 24 h old, measured from both the youngest copy's upload time and its `PackId` time. It must also be younger than `REQUEST_RETENTION` (48 h) by its `PackId` time. Past that age it is kept and alarmed.
  - An unmarked pack is kept and alarmed when its metadata is unreadable or its leader is unavailable.
  - A marker whose pack no bucket holds is kept and alarmed (`LostPack`). A copy of a committed pack that is the wrong size or has the wrong hash is alarmed and never spread (`BadCopy`).
  - An upload journal older than 24 h has its multipart upload aborted, and then the journal is deleted.
  - Actions run in phases: markers, then fills, then aborts, then deletes. A run that dies partway has therefore marked everything it planned before deleting anything.
- The `request` table lives on the chain's Raft group, which no script hosts yet. The sweeper reads it through the `RequestSource` trait. The bed implements that trait with `Requests`, a stub Durable Object that holds (RequestId → index, packs) rows in SQLite. That stub is the only stand-in in this row.
- `host::start_upload` journals a multipart upload's id in the same bucket before the first part is uploaded. The Workers R2 binding cannot list in-progress multipart uploads (the R2 Workers API reference has `createMultipartUpload` and `resumeMultipartUpload` but no list call). Without the journal, the sweeper could not find an orphan to abort. R2 itself aborts uploads after 7 days by default (R2 docs, "Object lifecycles").

## BF-19 on real buckets

The bed (`tests/m1/wp4/src/lib.rs`, a `Bed` object driven by its own alarm) seeds one run in batches of 50 + 50:

- **1,000 failed placements.** `PackStore::place` with buckets B and C refusing every pack put. Only A takes the pack, the two-region rule fails, and `place` returns `TooFew`. The one copy stays behind, as it would after a real failed placement.
- **1,000 committed packs whose marker write is killed.** `place` succeeds, and the bed writes the `request` row to the stub. That write is the commit. Then `mark_committed` runs with every marker put refused, as when a leader dies after `Acked` and before its marker puts. For one in three of these packs (334 in r10), bucket C also refused the pack put. Those packs are durable in two buckets, and the filler has to copy them to C from the markers.
- **5 orphan multipart uploads** on bucket A. Each one is journaled, given one 5 MiB part, and abandoned.

Every pack is 1 MiB + 1 byte, the smallest size that is stored rather than inline. All 2,000 placements behaved as planned, with 0 surprises.

**How time was shortened.** The sweeper takes `now` as an argument, and that argument is the only fake. Each sweep's clock is a base plus a skew. The base is the real time, the seed's first placement time, or the end of its last placement. The skew is set per call (`POST /sweep?run=R&base=first|last|now&skew_m=M`). Everything the sweeper compares `now` against is real: R2's own `uploaded` time for each object, and the `PackId`'s ULID time, which was minted at placement.

This is sound for three reasons. First, the decision reads time only through `now`, object `uploaded`, and `PackId` time, and `decide` has no other clock. Second, R2 does not change an object's state with age within these windows. The only age-driven R2 behavior is the 7-day multipart expiry, which the shortened run never reaches. Third, a run that waited 24 h would differ only by advancing `now`. The one thing a skewed clock cannot show is a real day of sweeper runs interleaved with new pushes, which the unit tests cover instead (`failed_placement_is_deleted_after_a_day_and_not_before`, `young_unmarked_pack_with_no_row_is_kept`).

The commands for run r10 were `tests/m1/wp4` `worker-build --release`, then `wrangler deploy`, then `POST /start?run=r10&n=1000`, and then `WP4_URL=… WP4_TOKEN=… tests/m1/wp4/bf19.sh r10`, which makes the calls below in order. r11 is a replicate that differs only in its seed. The full output is in `tests/m1/results/raw/bf19-r10.log` and `bf19-r11.log`.

| Sweep (r10) | Clock | Actions run | Kept young | Failures | After: per bucket A / B / C |
| --- | --- | --- | --- | --- | --- |
| before | | | | | packs with a marker 0 / 0 / 0; committed packs 1,000 / 1,000 / 666; failed packs 1,000 / 0 / 0; open uploads 5 |
| 1 | real time | 2,799 marker puts from the `request` rows | 1,050 | 0 | (as below, before the fill) |
| 2 | real time | 351 fills (334 + 17, see below) | 1,050 | 0 | committed packs 1,000 / 1,000 / 1,000, each marked; failed packs still 1,000 |
| 3 | last placement + 23 h | none | 1,050 | 0 | unchanged: nothing is deleted under 24 h |
| 4 | first placement + 24 h 1 min | 5 aborts, 87 deletes | 963 | 0 | failed packs 913 left: only those placed in the seed's first minute are 24 h old; open uploads 0 |
| 5 | last placement + 24 h | 963 deletes | 0 | 0 | failed packs 0; committed packs 1,000 / 1,000 / 1,000 with 1,000 markers in each bucket |
| 6 | last placement + 72 h | none | 0 | 0 | unchanged: 0 unmarked pack keys in any bucket, 1,050 packs and 1,050 markers each |

**The extra batch.** The network holds 1,050 packs and not 1,000 because one alarm invocation of the seed placed its batch of 50 + 50 and wrote the 50 `request` rows, then ended before it saved its progress. The runtime retried the alarm, which ran the batch again with new ids. The marker count shows this: 2,799 = 666 × 3 + 334 × 2 for the tracked 1,000, plus 33 × 3 + 17 × 2 for the extra 50, of which 17 were in two buckets (hence 351 fills). The extra packs are a real case of the row, namely an edge or leader that dies after the commit and is retried. Their 50 committed packs have rows, so they were marked, filled, and kept like the others. Their 50 failed twins were deleted at 24 h (87 + 963 = 1,050 deletes). The "committed packs" and "failed packs" columns count only the 1,000 tracked ids of each class. r11 had two such retried batches (1,100 packs).

Replicate r11, with the same steps: 2,932 marker puts, 368 fills, nothing before 24 h, then 5 aborts and 92 + 1,008 deletes. It ended with 0 unmarked pack keys, 1,100 packs and 1,100 markers per bucket, 0 failed packs, and 0 open uploads. Every action in both runs succeeded (`failed: 0`).

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| BF-19 | r10 and r11 together, counting the retried batches: 2,150 of 2,150 failed placements deleted at 24 h by the youngest copy and none before. 0 of 2,150 committed packs deleted. 2,150 of 2,150 missing markers repaired from the `request` rows, in every bucket holding the pack, and the 719 two-bucket packs filled to the third bucket (351 + 368). 10 of 10 orphan multipart uploads aborted. Time shortened by an injected `now` only | Failed ones deleted after 24 h, zero committed ones deleted, missing markers repaired; fail on any committed pack deleted | **Pass**, with the `request` table stubbed. It needs a rerun once the site script hosts the sweeper cron and the Raft group's real `request` table |

## Adversarial unit tests and planted bugs

`cargo test -p graftyard-git` covers the following cases, and each one also runs on `MemBucket`, which has a settable clock and injected put failures:

- two buckets in one region (not durable)
- a key that exists with other bytes
- bytes that differ by one bit from the validated ones
- a retry of the same bytes
- a marker with the pack in only one bucket (filled, never deleted)
- a pack younger than 24 h with no marker, where the youngest copy sets the age
- a `PackId` minted after the upload time
- a marker for a missing object, and a wrong-sized copy as the only source
- a `request` row of a retried request that names a different pack
- a down leader, missing metadata, and past `REQUEST_RETENTION` (all kept and alarmed)
- multipart orphans
- stray keys
- a budget cut that must spend on markers before deletes

Planted bugs, one at a time, each run against the full test suite (`/tmp/mutate.py` during the run, not kept). Of 25 planted bugs, 25 were caught. Two needed work first. The first run caught 23. One planted bug did not compile and was rewritten, and then it was caught. The other was missed ("renew to 24 h after the first request rather than the last", in the M1-WP5 alarm logic). That miss led to a new assertion for a late alarm in `alarm_renews_hourly_until_a_day_after_the_last_request`, and the bug was caught on the rerun. The M1-WP4 planted bugs:

| Planted bug | Caught by |
| --- | --- |
| two buckets in one region count as two copies | `rule_needs_two_regions_not_two_buckets` |
| an existing key with other bytes counts | `place_counts_a_retry_of_the_same_bytes_and_not_a_collision` |
| no check that placed bytes are the validated ones | `place_inlines_small_packs_and_refuses_unvalidated_bytes` |
| orphan age 23 h | `failed_placement_is_deleted_after_a_day_and_not_before`, `young_unmarked_pack_with_no_row_is_kept` |
| age from the oldest copy | `young_unmarked_pack_with_no_row_is_kept` |
| `PackId` time ignored | `pack_id_time_counts_as_well_as_upload_time` |
| any row for the request saves the pack | `a_row_for_a_retried_request_that_names_another_pack_does_not_save_this_one` |
| unavailable leader treated as no row | `past_retention_or_unknown_metadata_or_a_down_leader_keeps_the_object` |
| deletes past `REQUEST_RETENTION` | same |
| deletes an unmarked pack with no metadata | same |
| fills from a copy of the wrong size | `marker_for_a_missing_object_alarms_and_keeps_the_marker` |
| no third-bucket fill | four sweep tests |
| aborts uploads at any age | `orphan_multipart_uploads_are_aborted_after_a_day` |
| deletes before markers | three sweep tests |
| deletes the marker of a lost pack | `marker_for_a_missing_object_alarms_and_keeps_the_marker` |
| network prefix without its trailing slash (net 7 lists net 77) | `listing_prefixes_cover_exactly_their_kind` |

## Not built here

- The site-script cron that calls `sweep` per network. It needs workers/site to host it, plus the list of the site's networks, which comes from the registry or the groups.
- The edge's multipart tee with 16 MiB parts (M1-WP6). `host::start_upload` is the piece it calls.
- `Promoted`'s re-put of its range's markers. The BF-18 bed does this (`tests/m1/wp5`), but the leader-side call belongs to the Raft host.
- `R2Key::Blob` placement (`DurableBlob`), which is M2-WP4.

## BF-19 on the hosted cron with the real `request` table (2026-11-02)

BF-19: "1,000 failed placements, plus 1,000 commits whose marker write is killed. Pass: failed ones deleted after 24 h, zero committed ones deleted, missing markers repaired." The earlier run stubbed the `request` table. This one runs everything on the test install `graft-m1-site-a/b/c` (`test-faults` build) and `graft-m1-edge`, on repository `wp6/bf19a` (network 196622, home site 0). Driver: `tests/m1/wp8/bf19.ts --run r1 --repo 196622 --n 1000 --par 8`.

- **1,000 failed placements.** Each is a stored pack of 1 MiB + 1 byte, placed by the edge's `PackStore::place_teed` through `/_bed/push` with `fail_place`. Every bucket except the record home's (site 0) refuses the put. All 1,000 failed with `held by 1 buckets in 1 regions`, and the edge did not commit them. One copy stayed in bucket 0, with custom metadata naming the chain and a request id that has no `request` row.
- **1,000 commits whose marker write is killed.** Each is a stored pack placed in two regions (272 in buckets {0,1}, 717 in {0,2}, 11 in {1,2}), then a `Push` naming it, committed through the edge's `commit_hedged` on the repository's real Raft group. The test-faults flag `no_marker` makes the deciding leader skip its `Committed` marker puts after `Acked`, as a leader killed between them would. The `request` rows are the ones `GroupFold::apply` wrote. The 1,000 commands committed in 500 entries.
- **The sweeper is the site script's own cron body,** `sweep::run_at(env, now)`, which `scheduled` calls as `run_at(env, now_ms())`. It runs `crates/git`'s `sweep_all` with `CronBudget::SITE_CRON` and the stored `next`, keeps the networks whose home site is this site, and reads `request` rows through `/read {"q":{"request":id}}` on the chain's leader. It was called through Registry's test-faults `POST /sweep_run?now=MS` (the edge's `/_bed/sweep?site=0&now=MS`) with the clock set ahead. **The clock is the only fake.** The deployed `*/15` cron trigger stayed on during the run; its effect was not separated, and it can only have done the same actions earlier.
- **Counts come from the buckets.** A HEAD of every seeded pack and its marker in all three buckets (`/_bed/heads`) after each sweep step. Seeding ran 12:04:47 to 12:20:45Z.

| Sweeper clock | Runs (600-action budget each) | Actions | Failed placements held | Committed packs held, and in ≥ 2 buckets | Committed packs with a marker in every holder | Committed deleted |
| --- | --- | --- | --- | --- | --- | --- |
| before | — | — | 1,000 | 1,000, 1,000 | 0 | 0 |
| real time | 5 | 3,000: 2,998 marker puts and 2 third-bucket fills | 1,000 | 1,000, 1,000 | **1,000** | 0 |
| real time, again (a second invocation) | 1 | none | 1,000 | 1,000, 1,000 | 1,000 | 0 |
| last seed + 23 h | 1 | none | 1,000 | 1,000, 1,000 | 1,000 | 0 |
| first failed + 24 h 1 min | 1 | 66 deletes | 935 | 1,000, 1,000 | 1,000 | 0 |
| last seed + 24 h 1 min | 2 | 935 deletes (600 + 335) | **0** | 1,000, 1,000 | 1,000 | **0** |
| last seed + 72 h | 1 | none | 0 | 1,000, 1,000 | 1,000 | 0 |

- At "first + 24 h 1 min", 65 of the run's failed placements went (1,000 to 935 held), and the next step deleted the other 935. The run's file has 66 rows within 60 s of the first. The 66th delete is most likely the smoke test's failed placement, seeded on the same network before the run and outside the run's file, which the census does not count. The 66th row in the file was seeded 59,968 ms after the first, 32 ms inside the boundary, and is probably the one kept until the next step. `at` is the orb's clock at the reply, while the sweeper ages a pack by Cloudflare's clock from its upload and `PackId` times, so a 32 ms margin does not separate them. Which row was kept was not checked. No failed placement was deleted before its 24 h had passed.
- Final count: 0 of 1,000 held. No failed placement is left, and none of their copies ever got a marker.
- Every one of the 1,000 committed packs kept every copy at every step, and got its markers from its `request` row: the `request` row's index and pack checksum, in every bucket holding the pack. No sweep failed (`failed: 0`) and no network was left unresolved (`unresolved: 0`).
- The sweep logs, the report and the seed rows are in `tests/m1/wp8/raw/r1.bf19-sweep-a.log` (the first invocation, cut after its first real-time step because its census was too slow; the census then ran 5 calls at once), `r1.bf19-sweep-b.log` (the full run), `r1.bf19-report.json`, and `r1.bf19.jsonl`.

| Row | Result | Verdict |
| --- | --- | --- |
| BF-19 (hosted cron, real `request` table) | 1,000 of 1,000 failed placements deleted, all after 24 h and none before. 0 of 1,000 committed packs deleted. 1,000 of 1,000 missing markers repaired from the real `request` rows. Time advanced by the injected sweeper clock only | **Pass** |

What is still stubbed or not covered:

- **The 24 h is the sweeper's injected clock,** passed to the cron body through a test-faults route, not a day of waiting.
- **A placement that does not commit in M1 has no `request` row,** so the "row for a retried request that names another pack" case (unit test `a_row_for_a_retried_request_that_names_another_pack_does_not_save_this_one`) did not arise on the hosted install.
- **Each cron run reads one `request` row per unmarked pack through the leader.** That is 2,000 reads in about 70 to 95 s per run here. At 600 actions a run, the 2,998 marker puts took five runs, about 15 minutes of 15-minute cron ticks. The budget's subrequest estimate (`m1-wp5.md`, "The sweeper's cron entry") does not count these reads. Each read is one member call from the cron invocation, so a network with many unmarked young packs can exceed the 1,000-subrequest limit before the action budget does. Not reached here: no run failed.
