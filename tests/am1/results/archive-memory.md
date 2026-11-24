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

## The open-bytes read on B0-33 clA (2026-11-24, $0) [measured]

Edge refusal's closing B0-33 run clA (graft-m2b, main `daa4dcc2` with open-bytes on its site day, repo 131414, a1
`mwuiu47v`, load 10:06:13Z for 300 s; raws `raw/memguard-clA.jsonl.xz` and `raw/a1-cl/`, branch `b033-closing`
`f1d19e10`). Memguard's 2 s samples carry `memSplit.open`.

**open.high does not explain the leader's climb.**
- Leader site 0 peaked at 89.1 MiB against memguard's 120, with no reset on any member. `open.high` peaked at
  4.20 MiB and `requestsHigh` at 498. memSplit's counted in-flight parts peaked at 2.7 MiB; `logPageHigh` 4.18 MB.
- Leader MiB against `open.high` reads r 0.99, but both are high-water marks rising together. Against `open.bytes`
  (held now) it reads 0.45.
- Of the leader's 73.6 MiB of steps of 1.5 MiB or more, 16.6 MiB came with `open.high` rising 0.5 MiB or more, and
  25.6 MiB with `requestsHigh` rising.
- `open.high` stopped rising at 10:08:48 and `requestsHigh` at 10:09:02. Memory climbed on to 10:10:34: +6.4 MiB at
  10:09:14 with 75 requests open and +4.7 MiB at 10:10:34 with 45, neither a new peak.
- Followers: site 2 peaked at 59.4 MiB (`open.high` 3.54 MiB, `requestsHigh` 35), site 1 at 31.9 (3.71 MiB, 4).

**What the climb does follow** [measured, cause inference]. In the first 34 s of load the stored log (`memSplit.log`)
filled from 1.5 to 8.0 MB, and memory rose about 6.5 MiB per log MB on the leader, 5.2 on site 2 and 1.5 on site 1.
After that the log held at about 8 MB while the archive ran, yet the leader added 35 MiB more, site 2 20 MiB (its dump
caches 2.3 to 5.2 MB) and site 1 14 MiB. So the growth is in heap memSplit does not count, and it follows entries
passing through the log and the leader's traffic (43,000 `/commit`, 20,000 `/raft`, 10,000 `/outcome`), not request
bodies held open. wasm memory never shrinks, so allocator fragmentation under that churn fits the shape; a live owner
memSplit misses fits it too. Nothing measured tells them apart: "The heap counter" below does.

## The heap counter (built, 2026-11-24, not hosted)

Branch `heap-count` from main `05ea366d`. Site script only, in the normal build as well as test-faults, so it rides the
next site day. `DECISIONS.md`, `heap-count`.

**What it counts.** `workers/site/src/heapcount.rs` wraps the system allocator as the site's `#[global_allocator]`.
Every allocation adds its size to the live count and raises the peak; every free subtracts; a reallocation moves the
count by the difference between the old and new sizes. `/stats` `memSplit` gains `heapLive` (bytes live now) and
`heapPeak` (the most live at once since the isolate started, never reset, as `wasmMemMb` is not). memguard copies
`memSplit` whole, so its samples carry both with no harness change.

**How to read it.** On a row longer than 8 minutes, plot each member's `heapLive` against `wasmMemMb` over time:
- `heapLive` tracking `wasmMemMb` up: the bytes are live, so a leak or an owner memSplit does not count. Which owner
  comes next, from the parts of memSplit that move with it.
- `heapLive` flat or falling under a rising `wasmMemMb`, with `heapPeak` well under it: the allocator holds freed pages
  it cannot reuse, so fragmentation. The fix is then the allocation pattern or the allocator, not an owner.
- `heapPeak` near `wasmMemMb` with `heapLive` far below: one moment's live peak sized the memory, and the rest is free.

**Cost.** A site isolate runs one thread, so on wasm32 the counts are plain statics: two adds and a compare per
allocation. Native bench, `--release`, 2,000 entries of two pushes and one issue command each on real members (about
24.9 million allocations): plain system allocator 2,111 to 2,245 ms over 5 runs, counted 2,169 to 2,249 ms; the
difference is inside the run-to-run spread [measured, local; the native build counts per thread, so it pays a
thread-local lookup the wasm build does not].

**Tests** (`heapcount::tests`):
- `live_bytes_follow_allocations_reallocations_and_frees`: a 3 MiB block counts 3; grown by realloc to 8 counts 8, not
  11; shrunk to 2 counts 2; a zeroed 5 MiB beside it counts 7; both freed count 0.
- `the_peak_holds_across_reads_and_rises_past_itself`: a freed 16 MiB block leaves the peak at its height through three
  reads; 20 MiB live at once raises it past 16.

## The heap reading on B0-33 ln (2026-11-24) [measured]

The lead's 15-minute row (`tests/am1/results/b033-ln.md`): lnA on graft-m2b, main `801d63ad` with heap-count on its
site day, repo 131418, a1 `mwuqsbkc`, load 900 s from 13:46:21Z, leader site 0 throughout, 0 leader changes, harness
outage 1 s (0.1%), run $6.24. Raws `raw/memguard-lnA.jsonl.xz`, `raw/leaderlogs-lnA.json.xz`, `raw/a1-ln/`. Every line
below is recorded, not gated.

**Caveat on rate.** ln's leader ran 73,369 commands at 81.5 a second and 185 inbound requests a second, against clA's 117
and 274 over its 300 s; commit p50 and p99 626 and 2,416 ms against 797 and 2,670. The plateau below is at ln's rate.
Its wasm peak matched clA's: 90.9 against 89.1 MiB.

**1. heapLive against wasmMemMb: no leak, and no fragmentation that keeps growing.** Every member's live heap stops
rising within the first minutes of load, and its wasm memory plateaus 16 to 32 MiB above the live peak.

| Member | wasmMemMb, last rise | heapPeak | heapLive in load | heapLive after load | wasm − heapPeak |
|---|---|---|---|---|---|
| site 0, leader | 14 → 90.9, 13:51:16 | 58.8 | 31 → 36 to 45, no trend after minute 3 | 39.8 | 32.1 |
| site 1 | 8 → 63.3, 13:50:25 | 47.0 | 14 to 18, flat | 17.8 | 16.3 |
| site 2 | 8.8 → 73.6, 13:55:52 | 46.3 | 14 to 18, flat | 17.8 | 27.3 |

- r(wasmMemMb, heapLive) is 0.99 on every member, from the shared first-minute ramp. Past it, heapLive is flat and
  wasm memory only steps.
- Steps of 1.5 MiB or more in wasm memory with heapPeak rising under 0.5 MiB, which is the allocator taking new pages
  while it holds free ones: 21.9 of the leader's 72.4 MiB of steps, 10.6 of site 1's 55.3, and 28.6 of site 2's 64.8.
  Site 2's last two steps (+4.8 MiB at 13:52:12, +5.3 at 13:55:52) came at heapPeak 44.4 with heapLive 17.7.
- The other way too: the leader's heapPeak rose from 55.2 to 58.8 MiB after 13:51 with no wasm growth, inside pages it
  already held.
- So wasm memory is the live peak plus 16 to 32 MiB of allocator slack, and the live peak stops rising once the load
  is steady.
- **An uncounted owner on the leader.** After load the leader holds 39.8 MiB live against each follower's 17.8. memSplit
  counts the leader's log at 7.9 MB and nothing else; each follower has the same log plus 5.9 MB of dump caches. So about
  22 MiB of live heap that only the leader holds is uncounted. It is stable, not growing: a budget item, not a leak.

**2. The climb levels off.** The leader held 90.9 MiB for the last 10 of the 15 load minutes, site 1 63.3 for the last
11, site 2 73.6 for the last 5.5. memguard's 120 was never reached, and no member reset.

**3. log-page's "not enough" flip: not flipped.** Leader peak 90.9 MiB, under 100; `logPageHigh` 4,193,555 (leader),
4,095,042 and about 4.19 MB, all at or under 4,194,304.

**4. open-bytes' bound flip: not flipped.** `open.high` 4.40 MiB on the leader (`requestsHigh` 339), 3.82 and 3.98 on
the followers, against the 30 MiB line. Only 1 of the leader's 17 steps of 2 MiB or more came with `open.high` rising
1 MiB or more.

**5. The archive turns** (`raw/leaderlogs-lnA.json.xz`: 171,573 events, 0 full chunks; memguard figures are the last
sample before each line and the highest within 6 s after it).

| Scan line (Cloudflare clock) | Stored range | Bytes scanned | Turn CPU / wall | Leader MiB, heapLive, heapPeak before → after |
|---|---|---|---|---|
| 13:45:55.338 | 2..=944 | 1.4 MB | 16 / 80 ms | 12.6, 9.8, 10.2 → 13.1, 10.1, 10.4 |
| 13:46:55.549 | 2..=1105 | 9.7 MB | 18 / 54 ms | 53.7, 32.5, 39.4 → 60.2, 29.9, 39.5 |
| 13:47:55.637 | 2..=1272 | 43.8 MB | 22 / 591 ms | 66.5, 30.0, 43.7 → 66.5, 30.3, 43.7 |
| 13:48:55.910 | 2..=1407 | 68.1 MB | 111 / 162 ms | 72.6, 34.7, 49.5 → 72.6, 33.7, 49.5 |
| 13:54:43.613 | 2..=2426 | **246.5 MB** | 61 / 872 ms | 90.9, 40.8, 58.7 → 90.9, 40.8, 58.7 |

- **No scan moved the live heap.** heapLive moved by at most 2.6 MiB across any scan (13:46:55, during the load's first
  minute), and the 246.5 MB scan moved nothing. The scan stays an aggregate: `length()` reads no content
  (`decisions/DECISIONS.md`, "`archive-scan`").
- **The 68.1 MB scan started the row's one promotion**, `Promoted { from: 17, to: 1078 }` at 13:54:43, which took 347.5 s
  (13:48:55 to 13:54:43) and committed in 296 ms. 66 `shard warm … no answer in 25 s` lines on site 2 from 13:47:03 to
  14:02:05 sit beside it. No `Archived` committed in the row, so no truncation ran: 900 s at ln's rate stores about
  250 MB, and the drain was still in its first promotion. Whether a truncated tail frees live heap is still not
  observed [inference: the 30-minute row would reach it, if the promotion takes as long].
- **archive-scan's flip: not flipped.** Scan-line turns took 16 to 111 ms CPU with no trend in the scanned bytes (1.4 to
  246.5 MB), 228 ms in all, 0.06% of the leader's load CPU, against the 1% line.

**6. heap-count's CPU flip: not flipped, but the leader's CPU per invocation rises through the row** [measured, cause
inference].
- First 300 s on the same shape: ln's leader 1.67 ms an invocation (73,681 invocations, 123.0 s) against lpB's 1.36
  (85,408, 116.1 s, main `11f1a69a`, no heap-count) and clA's rate. By route: `/commit` 1.47 against 1.24 ms, `/raft`
  1.10 against 1.24, `/outcome` 0.38 against 0.29, `/entries` 5.57 against 4.66. `/raft`, the route with the most
  allocation per byte, is cheaper on ln, and the others are within run-to-run spread for different builds, hosts and
  rates; ln's site day also carries agent-rows-cache's policy rule and the other merges since `11f1a69a`. There is no
  sign of a per-allocation cost near 1%.
- Over the 900 s the leader's CPU per invocation climbed from 1.01 ms in the first load minute to 2.95 in the last three,
  while its invocations fell from 18,739 to about 7,700 a minute: `/commit` 1.34 → 2.72 ms and `/raft` 0.99 → 2.18 in
  3-minute bins. That is work per call growing with the stored log or the open-PR set, not the counter, which costs the
  same per allocation throughout. The leader used 362.5 s of CPU over the 900 s, 40.3% of a thread (lpB: 38.7% over its 300 s). Not chased here.

**Verdicts (recorded, not gated).** heapLive plateaus on every member (no leak, no growing fragmentation; about 22 MiB of
stable uncounted live heap on the leader); the climb levels off at 90.9 MiB; log-page, open-bytes, archive-scan and
heap-count's flips all hold. No 30-minute row (the lead, 2026-11-24). Next, at $0: a native leader sim's allocation
census of the leader's uncounted live heap.

## The census of the leader's uncounted heap (2026-11-24, $0)

**Result: 19.4 MiB of the leader-only live heap is test-faults' BF-14 arrival ring, which product builds do not have.**
- **The leader-only uncounted live heap after load is 27.5 MiB, not 22.** The 22 MiB came from comparing heapLive with
  heapLive, and each follower's 5.9 MiB of dump caches is counted while the leader holds none.
- **8.1 MiB is left after the ring.** About 3 MiB of it is the site host's leader-side `Stats` sample arrays
  [inference, from their counts]. About 5 MiB is not attributed.
- **Under load, the core's resolved tickets add up to about 3.9 MB,** which OUTCOME_KEEP frees.
- **Nothing in the core grows with the stored log or the open-PR set.**

**1. The arrival ring** [measured, hosted ln and a wasm32 allocation count]. Every B0-33 row since 2026-11-18 sets
test-faults `/stats?arrival=mem` on every voter (`tests/fanout/README.md`, "B0-33 runs with `arrival=mem`"). In that
mode, each `/commit` a member receives pushes one `serde_json::Value` row `{rid, at, role, fault, answer}` onto
`Host::arrivals_mem`, which keeps up to `ARRIVALS_MEM_MAX` 40,000 rows and drops none until it is full
(`workers/site/src/member.rs`, `receipt_fault`). On B0-33 only the leader receives `/commit`s, so only the leader's
ring fills. memSplit does not count it, and archive-memory's "Why memSplit missed it" names it as a blind spot.
- **Bytes a row.** 40,000 rows built as `receipt_fault` builds them, with a 26-character request id and the counting
  allocator: 509 B a row on `wasm32-wasip1` (19.4 MiB in all), and 746 B native. It is a scratch count, not committed:
  a `serde_json::Value` object of five keys, its `BTreeMap` nodes and the key and value strings.
- **ln's shape.** The leader received 87,341 `/commit`s in the leader logs from 13:45:21Z (a1's own reset, 86,368 in the
  load window). The leader's uncounted live heap less the followers' (heapLive minus every counted memSplit part, the
  log, `open.bytes`, admitted, calls out, answers, held, dumps and memo) rose at 555 B a `/commit` until 35,250 logged
  `/commit`s (about 13:51Z), then stayed flat to the end (a two-line fit). Fitting the ring itself, 509 B a row
  filling until 40,000 with about 3,000 setup `/commit`s before the log window, leaves a flat 9.0 MiB, rms 2.2 MiB.
  After load the difference is 27.5 MiB, of which 19.4 is the full ring.
- **Why it plateaus.** The ring is full once 40,000 `/commit`s arrived, at about 13:51Z, which is where heapLive stopped
  rising on ln's leader. It is not a leak: a full ring replaces its oldest row.
- **What it changes.** Every heapLive and wasm memory figure from a B0-33 or `arrival=mem` row includes up to 19.4 MiB
  on the leader that a product build does not hold. ln's leader at 39.8 MiB live after load would be about 20.4 on a
  product build, beside each follower's 17.8 [inference: the ring's count is measured, the subtraction is not].

**2. The core's resolved tickets** [measured, a native leader sim, scratch]. Three real members through
`crates/git/tests/common` with 50 open PRs, then 1,216 commands admitted at one clock (38 batches of 32: agent pushes to
new branches, and comments carrying an 18-login render without the owner's `Authz`, as `leader-render` sends a first
send), about OUTCOME_KEEP at ln's rate. Each member's live bytes by dropping it:
- right after the window: leader 5.98 MB, each follower 2.09 MB, so 3.9 MB leader-only;
- after 20 s of ticks past OUTCOME_KEEP: OUTCOME_KEEP freed 3.58 MB, and the leader held 2.39 MB against 2.09.
- So the core's leader-only bytes are `Lead::done` (an `Acked` outcome per ticket, priced by DONE_BYTES only on its
  `unstored` bytes) plus about 0.3 MB of other `Lead` state. At ln's 81.5 commands a second, 15 s of tickets is about
  1,220, which is the window above. It is within its bounds and frees itself.
- After load the tickets are gone, so none of the 27.5 MiB is theirs.
- Not covered natively, so it holds the 8.1 MiB left after the ring:
  - The site host's leader-side `Stats` sample arrays, each under SAMPLES_MAX 200,000 `u32`s. a1's
    `/stats?reset=1` at load start clears them. At ln's counts, `queue_ms` and its two splits are 73,364 samples each,
    `commit_ms` 77,284 and `admitted_bytes` 77,286. With the call times, that is about 1.5 MB, and up to twice that in
    `Vec` capacity, so about 3 MiB [inference].
  - The leader-only `thread_local!`s: `merge_queue`, `prsync`, `hookseq`, `activity`, and `split`'s cache of at most
    CACHE_BYTES 32 MiB.
  - The test-faults trace ring is TRACE_MAX 60,000 × 48 B, 2.9 MB at most. tsave read 661,291 rows from the leader
    over the row, and 52,883 and 79,245 from the followers. So the leader's ring was full, and the followers' rings were
    nearly full or full, so at most a small part of it is leader-only.
  - About 5 MiB stays unattributed.

**3. What per-call cost tracks** (F115) [measured, native and ln's leader logs]. The native sim's per-command decide,
apply and replicate time did not move with either candidate:
- with 50 open PRs and 0 to 11,520 commands committed: 0.089 to 0.097 ms a command;
- with 50, 1,000, 1,600 and 3,200 open PRs: 0.096, 0.088, 0.090 and 0.090 ms a command, and opening a PR 0.178 to
  0.190 ms.
- On ln the leader's CPU per entry did not climb either: 133 ms an entry in the first load minute, 96 to 219 in the
  middle and 122 to 139 in the last five, at 142 to 208 entries a minute. What climbed is CPU per invocation, because
  the invocations per entry fell from 132 to 41: fewer `/commit`s per entry as agent side writes were answered 429
  (side and push ok went from 7,752 to 3,302 a minute against about 9,300 offered). `/entries` per call went from 5.4
  to 29 ms because it went from 2.8 to 56 entries a call, while its CPU per entry fell from 1.94 to 0.52 ms.
- So neither the stored log nor the open-PR set shows in the core's per-command cost natively, and the leader's per-entry
  CPU on ln held level: the per-call climb is the same work spread over fewer, larger calls [inference: the native sim
  has no SQLite and no host, so a storage-side cost of a larger log is not ruled out].

## Replay script

The page-rule replay is a few lines over the row sizes; it is not committed. It took per-index sizes from `culag`'s
stored bytes and the archive scan lines' range sums, and applied main's rule (8 rows, double while under 2 MiB, halve
over 4 MiB, up to 1,024).
