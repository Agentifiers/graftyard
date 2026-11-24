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

## Hosted check (planned, graft-m2b)

One test-faults site day from this branch once merged, then one B0-33-shaped row at a $10 stop. Read:

- leader memory from memguard;
- `memSplit.logPageHigh` (expect ≤ 4,194,304);
- the alarm-step scan lines past 49 MB;
- whether any climb of more than 20 MiB lines up with a scan or a long `/entries`.

## Replay script

The page-rule replay is a few lines over the row sizes; it is not committed. It took per-index sizes from `culag`'s
stored bytes and the archive scan lines' range sums, and applied main's rule (8 rows, double while under 2 MiB, halve
over 4 MiB, up to 1,024).
