# The leader's archive-step memory on B0-33 (g3A, g4B): a log page sized by the page before

Branch `leader-logpage`, from main dac2348e. What held the scanned rows in wasm memory, the fix, and its checks. All
work here cost $0: the raws are g3A's and g4B's (`tests/fanout/raw/leaderlogs-g3A.json.xz`, `leaderlogs-g4B.json.xz`,
`memguard-g3A.jsonl`, `memguard-g4B.jsonl`, `culag-g3A.json.xz`, `culag-g4B.json.xz`; write-up
`tests/am1/results/b033-g39.md`), and the bed is local workerd.

## Cause

Every walk of the stored log that goes through `crate::logread::log_after` sized each page by the bytes of the page
before it. Pages start at 8 rows and double up to 1,024 while a page reads under 2 MiB. A leader's log under B0-33 is
about 1,000 short entries (index 1 to about 1,000, about 1.5 KB each) and then entries with inline packs of 80 to 300
KB. A walk from the start reads pages of 8, 16, ... 512 short rows, each well under 2 MiB, then asks for 512 or 1,024
rows of the long part in one query and collects all of them (`c.raw().collect::<Vec<Row>>()`) before it hands back the
first. The halving rule acts only on the page after that one.

That page is the memory. Replaying `log_after`'s rule over each run's row sizes (from `culag` and the scan lines'
range sums; script below) gives:

| Walk | First fetch of the long part | Rows | Bytes in one page |
|---|---|---|---|
| g3A `/entries?from=16&to=1454&local=1&bytes=4000000` (23:44:44.100Z, 215 ms CPU) | idx 1032 to 1494 | 463 | about 54 MB |
| g4B `/entries?from=16&to=1747&local=1&bytes=4000000` (00:56:48.348Z, 181 ms CPU, the invocation in flight at the reset) | idx 1032 to 1543 | 512 | about 66 MB |
| g4B `/entries?from=472&to=1747&local=1` (00:56:50.380Z, 152 ms CPU, the new instance) | idx 976 to 1487 | 512 | about 62 MB |
| `plan()` (promote-ahead) from a promoted index near 500 | 512 rows from there | 512 | 57 to 65 MB |

`entries_json`'s own `page_bytes` cap (1 MB busy, 4 MB idle) stops what it answers, not what `log_after` already
fetched: the cap is checked row by row on a page already collected.

The bed reproduces it (`tests/auditlock/bed/run.sh`, `/pages`: 1,000 rows of 1,500 B and then 200 of 300,000 B, one
walk in a later request) [measured, local workerd]:

| `logread.rs` | Rows back | wasm memory growth over the walk | Bed |
|---|---|---|---|
| main dac2348e (`LOGREAD=` main's file) | 1,200 of 1,200 | **52 MiB** | FAIL |
| this branch | 1,200 of 1,200 | **4 MiB** | PASS |

### How the hosted climbs line up

- **g4B, 59.8 → 114.8 MiB.** memguard's clock runs 2.35 s behind Cloudflare's on g4B (the skew that best matches its
  `/stats?arrival=mem` calls in the leader logs: 64 of 174). On that clock the climb falls between 00:55:16.79 and
  00:55:18.80Z, and the 68.4 MB archive scan line is at 00:55:18.258Z, inside it. That line's invocation is a `GET
  /status` of 293 ms CPU, the largest in the window, which is the alarm's turn: `note_scan`, then `archive::on_alarm`'s
  `drain` → `step_in` → `view` and `due`, run in the invocation that delivered the alarm. No `/entries` of more than 10
  rows ran in the window. So this climb is the archive step's own walk [inference: the logs place the turn, not which
  of its walks]. `view`'s walk reads only `length(row)` and rows under 4 KB, at most 0.8 MB a page; `due` stops at its
  first row on a log under the cap. That leaves `plan()`, promote-ahead's walk: the unpromoted tail (68.4 MB) is past
  64 MiB (`LOG_MAX_BYTES / PROMOTE_AHEAD`), so `step_in` takes `plan` from the last `Promoted`, and from a promoted
  index anywhere from 300 to 700 the first long page is 20 to 65 MB [inference: g4B's promoted index is not in the
  raws; no `promotion` line from `promote_handled` appears, as its `in_band` call would come after the walk].
- **g4B's reset (00:56:48.299Z).** It is the instance with the 66 MB `/entries?from=16` page in flight (that invocation
  ended `exception` after 181 ms CPU) on top of 114.8 MiB [inference: no log line names the limit].
- **g4B's new instance, 12.5 → 102.4 MiB in 2 s.** In one alarm turn, merge_queue (82 MB range), prsync and archive
  (91.5 MB) noted scans, and `/entries?from=472` fetched a 62 MB page. merge_queue's own read is one row at a time
  (`stored_entry_sized`, under `SCAN_BYTES` 16 MiB per alarm), so its 82 MB line is its range, not its memory.
- **g3A, 74.1 → 106.6 MiB.** On memguard's clock (0.2 s behind) the climb falls between 23:44:40.12 and 23:44:42.12Z.
  The 49 MB scan line (23:44:35.632Z) is before it, and the `/entries?from=16` walk (23:44:44.100Z, start time; its
  page is fetched in the first tens of ms) is 2 s after it. Neither is inside the bracket by the clocks alone; both
  walks have the same page shape [unverified which].
- **g3A, 106.6 → 124.8 MiB** (23:45:29 to 23:45:34Z) holds the 71 MB scan's turn (a `/raft` of 240 ms CPU at
  23:45:35.692Z) on the edge of the bracket.

### What `note_scan` adds

Nothing in wasm. It runs `SELECT COALESCE(SUM(length(row)),0)` over the step's range and gets back one integer;
`length()` of a blob does not read its content. Its cost is that aggregate's SQLite time [verified, code].

### Why memSplit missed it

memSplit counts what lives between requests: the fold's buffered rows, memo and caches, the in-memory log window, held
calls, and in-flight call and statement bytes. A `log_after` page lives inside one invocation, so no sample sees it
except by chance, and nothing counted its peak. Its other blind spots, for the record: `/entries` answers being built,
test-faults' `arrivals_mem` (capped at 40,000 rows), the trace ring, and response bodies.

## Fix (one change, at the source)

`log_after` now sizes each page from its rows' lengths before it fetches them. Each page first runs `SELECT idx,
length(row) ... LIMIT page` (lengths only), takes the longest prefix whose stored bytes stay within `PAGE_BYTES` (4
MiB), at least one row (`page_through`), and then fetches `cols` of exactly that range, read to its end as before. A row
over 4 MiB is a page alone; `ENTRY_MAX` is 1.5 MB, so it does not occur. The row count still doubles to 1,024 while
pages are under 2 MiB. Every caller keeps its code and gets the bound:

- the archive's `view`, `rows` (so `due` and `plan`), and promote-ahead;
- `/entries` (`entries_json`), the shard's catch-up and rehydrate read;
- `stored_range` (a follower's AppendEntries catch-up), `log_rows` (the change feed), and test-faults `log_json`.

One alarm step read the log outside `log_after`: `snap::load_meta` collected up to `META_BATCH` (500) whole rows in
one `rows()` call. It now walks `log_after` to the same end.

A fresh instance can run several walks in one turn. With every walk bounded by one page (4 MiB plus its decode), the
worst turn is a few of them, tens of MB at most, against about 60 MB per walk before.

**What it does not change.** What is due, what is promoted, and how far the log may grow (`LOG_MAX_BYTES`,
`archive_due`, promote-ahead's 64 MiB) are unchanged: the walks return the same rows in the same order. The cost is one
more small query per page.

**memSplit's new counter.** `logPageHigh`: the largest page any `log_after` walk fetched in this isolate, in stored row
bytes. On this branch it should never pass 4,194,304 except for a single row. On main it would have read about 55 to 66
MB on g3A and g4B.

## Checks

- `workers/site/src/logread.rs` tests, `a_page_holds_its_budget_not_its_row_count` (1,000 rows of 1,500 B, then 300 KB
  rows: the page ends at row 1,008 with 3,900,000 B) and `a_long_first_row_is_a_page_alone`.
- Plants (`tests/m0carried/plant112.py`), 4 of 4 caught: the budget ignored; a page that ends exactly at the budget cut
  one row short; a first row over the budget taken as no page; the row that passes the budget taken too.
- Walk-loop plants against the bed (`LOGREAD=`, `ARMS=fix`), both FAIL as they should:
  - `more` set only from a full count, so a page cut by the budget ends the walk: `rows=1024 bytes=8700000`;
  - `after` not advanced: `rows=8 bytes=12000`.
- The bed itself: main's `logread.rs` grows 52 MiB and fails; this branch grows 4 MiB and passes, and the
  SQLITE_LOCKED arms are unchanged.
- Full checks on this branch:
  - `cargo fmt --all -- --check`;
  - workspace clippy `--all-targets -D warnings` (one `unused dependency worker` lint on `workers/ops/Cargo.toml`, a file
    this branch does not touch);
  - wasm32 clippy for site, edge and ops, with and without `test-faults`;
  - `cargo test --workspace`, exit 0;
  - the three `scripts/check_*.py`;
  - `tests/exportbed/run.sh` (snap.rs and `log_rows` feed the export path): PASS, one `export/` object, 3 idle
    rounds, 0 wasm traps.
  No edge or container code changed, and the edge does not link `graftyard-site`, so edgeboot and `go test` do not apply.

## Hosted check: lpB on graft-m2b, 2026-11-24 [measured]

**Result: the fix holds hosted.** Under B0-33's load the leader peaked at 81.6 MiB against memguard's 120 (g3A 124.8,
g4B 114.8 then a reset). `memSplit.logPageHigh` stayed at or under 4 MiB on every member. No archive scan, up to
81.3 MB, moved the leader's memory by more than 7 MiB, and no wasm instance reset.

**What ran.**
- Site day -lp from main `11f1a69a` with test-faults. Sites a `4106c848`, b `eb468b3f`, c `f01f850f`; edge
  `d6e527d3` at 100%. Shard image `sha256:85914041…`, the same as -g3, since containers are unchanged.
- `tests/fanout/guardrow.sh`, TAG=lp, GUARD_WANT=fast, RUN_STOP $10, DAY_STOP $20.
  - The first try stopped before load on the bytediff gate. All 36 differing replies differed only in
    `x-graft-colo` (DFW against the -pb baseline's LAX). `bytediff.py` now masks that value as it masks `etag`'s, and a
    planted body change still differs. That fix is `58181608` on branch `lp-hosted`.
- lpA ended before load. At start, a1's installation-token call (`ghs`) answered 503, so the guard never got a verdict
  and the run failed closed. It cost $0.05 and ran on repo 131409.
- **lpB** was kept on repo 131410 (a1 `mwu7dw9c`). hostguard read 26.0 ms of leader CPU a setup commit against the
  28.4 line. Load ran 300 s from 04:43:24.149Z, with 5,000 agents, leader site 0 at term 2. Harness outage 1 s (0.3%),
  so the run is valid. It cost $2.71. graft-m2b's day went from $3.31 before load to $6.07 after the sweep, and the
  sweep found no shard container running.

**Memory** (`raw/memguard-lpB.jsonl`, site 0, the leader throughout):

| | lpB | g4B (main `dac2348e`) | g3A (main `5bed46af`) |
|---|---|---|---|
| Leader peak | **81.6 MiB** at 04:48:39Z | 114.8, then a reset | 124.8, stopped by memguard |
| `logPageHigh`, max over the load, sites 0 / 1 / 2 | **4,191,755 / 4,173,122 / 4,183,360** bytes (limit 4,194,304) | not counted | not counted |
| Followers' peak | 78.4 / 55.1 MiB | | |
| Wasm resets ("Promise will never complete", "previous WASM") | **0** | 1, 41 calls lost | |

The leader's peak memSplit at 81.6 MiB: log 7.96 MB, caches 0, memo 0, nothing in flight. The memory climbs steadily
with the load, not in steps. Every sample-to-sample step is under 7 MiB: 15.7 to 21.3, 61.3 to 67.9, 67.9 to 74.9 and
74.9 to 81.6. mrB and msA, on older code, peaked at 82 to 96 MiB the same way.

**The archive turns against memory** (`raw/leaderlogs-lpB.json.xz`). Memguard figures are the last sample within 6 s
before each scan line and the highest within 6 s after it, on memguard's clock:

| Scan line (Cloudflare clock) | Stored range | Bytes scanned | Leader MiB before / after |
|---|---|---|---|
| 04:42:47.412 | 2..=971 | 1.4 MB | 12.6 / 12.6 |
| 04:43:50.546 | 2..=1156 | 8.3 MB | 38.7 / 42.2 |
| 04:44:50.702 | 2..=1281 | 18.4 MB | 58.8 / 58.8 |
| 04:45:50.744 | 2..=1454 | 48.8 MB | 67.9 / 67.9 (the 61.3 to 67.9 step is at 04:45:50.403 on memguard's clock) |
| 04:46:51.348 | 2..=1628 | **81.3 MB** | 74.9 / 74.9 |

On g3A, a 49 MB scan came with 74 to 107 MiB; on g4B, a 68.4 MB scan with 59.8 to 114.8. Here an 81.3 MB scan moved
nothing. The scans themselves still grow with the log, about 10 to 30 MB a minute. That is CPU, not memory, and is the
B0-29 merge-front note's subject, not this fix's.

The leader never climbed past 100 MiB, so the per-invocation split the lead asked for in that case was not needed.

**Open: a longer run.** The leader's memory still climbs steadily, 7 MiB at load start to 81.6 MiB at 04:48:39Z over
the 300 s, with no archive-sized jump. memSplit's counted parts are flat (log about 8 MB; caches, memo and in-flight
bytes 0 at the peak), so the growth is in memory memSplit does not name. At about 15 MiB a minute, the leader would
reach memguard's 120 MiB around minute 8 of a load like this one, and the 128 MB isolate limit soon after, if the climb
continues. This row cannot say whether it levels off. A run longer than 300 s, or the per-invocation split taken over
this row's leader logs, would settle it [inference].

**The per-invocation read of lpB's leader logs (2026-11-24, $0).** `wasmMemMb` is `memory_size`, which only grows, so the
climb is a series of new high-water marks, not memory still held. memSplit counts what a member keeps; a request's own
buffers (its body, the JSON tree built from it, its answer) are counted nowhere. Joined with the leader's invocations
from `raw/leaderlogs-lpB.json.xz` (start plus wall):
- **To 61.3 MiB, the climb tracks how many requests the leader had open at once** [measured]. Across the 391 memguard
  samples, leader MiB against the running peak of open invocations reads r 0.90. Open requests went from 16 before
  load to 589 at 04:43:44Z, then 691 and 753 at 04:44:58Z, mostly `/commit` and `/outcome`. Each step of 2 MiB or more
  in that stretch has 352 to 753 requests open in the 3 s before it.
- **The last 20 MiB came at lower concurrency** [measured]. They came in three steps: 61.3 to 67.9 at 04:45:50, 67.9 to
  74.9 at 04:46:41, and 74.9 to 81.6 at 04:48:39. Those windows had 189, 231 and 3 requests open, with single turns of
  116 to 137 ms CPU (`/raft`, `/commit`, `/read`), or with `/entries?bytes=4000000` pages (from=1403..1790 and
  1426..1790) just before the last step. A freshly fragmented heap needing a new 4 MB page would grow the memory this
  way [inference; nothing logs the allocator].
- **So a longer run does not climb at 15 MiB a minute by itself.** Memory rises when a new peak of open requests or a
  new large buffer arrives, not with time. Whether a 600 s B0-33 load would reach 120 MiB depends on its peak open
  count, which edge refusal's bound limits [inference].
- **The counter memSplit lacks:** the request bytes the member holds open, meaning the sum of open request and answer
  body lengths, with its peak since the last `/stats` reset, beside the open count's peak. It would show whether a new
  high-water mark is concurrency or one large buffer. Now built: "The open-bytes counter" below.

**B0-33's lines on lpB, for the record** [measured, `raw/rows-lpB.status`; read under the 4040b083 rulings]:
- human commit stage p99 2,641 ms (n 137, 19 over 2 s);
- merge commit stage p99 2,406 ms (n 31, 4 over 2 s);
- agent-class ok 104.3 a second, against 110 (99 at 90%);
- leader changes 0;
- `suspect` that started an election 0 (8 refused reports, LAX 6 and SJC 2);
- refusal shape: 386 Human 503s, all `F65 member shed` with Retry-After;
- edge refusal's R2b flip read FLIPPED on both halves: agent ok pushes 7,336 against erDA's 8,369, and agent cut/s
  while refusing 101.2 against 114.2.

The B0-33 verdict and R2b belong to edge refusal's set.

## The open-bytes counter (built, 2026-11-24, not hosted)

Branch `open-bytes` from main `6d1e47d5`. Site script only, so it rides the next site day; no hosted run is scheduled
for it (the lead). `DECISIONS.md`, `open-bytes`.

**What it counts.** `workers/site/src/openbytes.rs`. Every request a joined member handles (`Member::fetch`, around
`Routes::fetch`) takes a hold that counts one open request and its body's `content-length`. When the handler returns
`Ok`, the hold adds the answer's body length while the handler still owns it. The hold drops by any path: answered,
errored, or the handler's future dropped when the caller went away. `/stats` `memSplit` gains:

```
"open": { "bytes": <held now>, "high": <peak bytes>, "requests": <open now>, "requestsHigh": <peak open> }
```

Both peaks hold across plain samples and restart from what is held now at `/stats?reset=1`, the same reset as the
other `/stats` counters. memguard copies `memSplit` whole from `/stats?summary=1`, so its samples carry `open` with no
change to `tests/fanout/memguard.mjs`.

**What it does not count.**
- An answer that streams after its handler returns (a read's `held_body`): `inFlight.answers` counts it, once per
  shared answer, and the counter counts that answer as 0 so the two never add the same bytes.
- A request body with no `content-length` counts as 0. Member-to-member calls send a `FixedLengthStream`, which
  carries one [inference: not read on a hosted member yet], and the edge's `/commit` bodies are arrays.
- The JSON tree or the decoded command a handler builds from its body. The body length is its lower bound; if
  `open.high` stays far under the next wasm step, those trees or the allocator are the suspects.
- Alarm turns and `/join` before a member joined: neither goes through `Routes::fetch`'s wrap.

**Tests** (`openbytes::tests`):
- `a_request_releases_its_bytes_however_it_ends`: an answered request holds body plus answer at once (peak 6,000)
  and releases both; an erred request releases; a pending handler holds 3,000 and one open request until its future
  is dropped, then releases.
- `the_peaks_hold_until_a_reset_and_restart_from_what_is_held`: two open requests peak at 16,000 bytes and 2 open;
  three plain samples after one ends keep both peaks; a reset restarts them at the 4,000 still held, not 0; the peak
  outlives the request that set it.

**Plants** (`tests/m0carried/plant120.py`): 11 of 11 caught. An ended request keeps its bytes; an ended request stays
open; an erring handler leaks its hold; a dropped future leaks its hold; a sample resets the byte peak; a sample
resets the open peak; a reset zeroes the byte peak; a reset leaves the peaks alone; an answer does not raise the peak;
a new request does not raise the open peak; the answer is not counted.

**How to read it on the next row.** Beside each memguard sample's leader `wasmMemMb`: when a step of 2 MiB or more
comes with `open.high` rising by a similar amount, the step is request buffers at that concurrency (lpB's first 61
MiB, by the invocation read). When `open.high` stays flat across a step, the memory is a single large buffer the
counter does not see, or allocator growth, and `logPageHigh` and the turn's own logs come next.

## Replay script

The page-rule replay is a few lines over the row sizes; it is not committed. It took per-index sizes from `culag`'s
stored bytes and the archive scan lines' range sums, and applied main's rule (8 rows, double while under 2 MiB, halve
over 4 MiB, up to 1,024).
