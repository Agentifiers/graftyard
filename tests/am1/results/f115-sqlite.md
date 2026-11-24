# F115's storage side: a leader's SQLite cost against its stored log (2026-11-24, $0) [measured, local]

F115 asks whether a repository leader's work per call grows with its stored log or its open-PR set
(`decisions/OPEN-QUESTIONS.md`, F115). The census ruled out the core: decide, apply and replicate cost the same at 0 and
11,520 commands committed and at 50 and 3,200 open PRs (`tests/am1/results/archive-memory.md`, "The census of the
leader's uncounted heap"). The native sim has no SQLite, so this measures the storage half in local workerd, at the
lead's 1k, 10k and 40k entries.

**Result: flat.** The leader's SQLite statements cost the same within a few µs at 1,000 and 40,000 stored entries.
Over the bed's whole calls, reading a stored entry or a request row adds nothing measurable above a call that reads no
SQLite at all. A few µs a statement is about three orders of magnitude below ln's per-call climb (`/commit` 1.34 to
2.72 ms, `/entries` 5.4 to 29 ms). So F115 narrows to fewer, larger calls.

## Bed

`tests/f115bed/run.sh`: the site's own build (test-faults) as three site scripts in local workerd, bound as quietbed
binds them, with real local Durable Object SQLite. One register group (`repo:900401`) grows its log through `/commit`,
one put of a 4,096-character value per entry. At each stop it times from outside, in three rounds:
- `grow`: 300 more puts, each committed on three members, in ms per entry. That covers the leader's append, the apply
  on each member, the `request` row, and the retention delete.
- `entries`: 300 leader `/entries` reads of one stored entry each, spread over the whole log, in ms per read.
- `request`: 300 `/outcome_by_request` lookups of committed request rows, spread over all of them, in ms per lookup.
- `status`: 300 `/status` calls. It reads no SQLite, so it is the per-call cost the other routes share.

`tests/f115bed/sqltime.py` then timed the statements themselves on a copy of the leader's own SQLite file, cut to each
stop. It used 5,000 runs each, Python's sqlite3, and `synchronous=OFF`, so the figures exclude this machine's fsync.

The run ended with 40,302 entries, 1.8 GB of local state and 0 wasm traps. Every call in every round answered (300 of
300).

## Whole calls, ms per call (three rounds)

| Stored entries | grow (per entry) | entries | request | status (no SQLite) |
|---|---|---|---|---|
| 1,002 | 118.8, 123.6, 121.9 | 17.1, 18.2, 19.4 | 17.8, 19.1, 17.7 | 19.1, 18.6, 18.0 |
| 10,402 | 107.1, 109.1, 108.0 | 15.8, 16.1, 16.1 | 15.7, 16.4, 15.8 | 16.0, 16.2, 15.8 |
| 40,302 | 155.4, 155.0, 155.4 | 22.5, 23.7, 22.7 | 22.2, 22.6, 22.8 | 21.8, 21.9, 22.0 |

- **Reads.** `entries` and `request` track `status` to within 1 ms at every stop. A stored-entry or request-row read
  adds nothing above the per-call cost.
- **The rise at 40k.** All four routes rose by about 20 to 30% from 10k to 40k, and `status`, which reads no SQLite, rose
  as much as the reads. So it is the bed's per-call overhead.
  - Over the same span the bed's growth fell from about 640 to 365 entries a minute.
  - Local workerd's own trace store grew to 486 MB beside the members' files [inference: a local cost, not storage].
- **Writes.** `grow` per entry over `status` per call was 6.5, 6.8 and 7.1 times at the three stops.
  - A put is one `/commit` call plus the leader's two AppendEntries and their applies, so that ratio is mostly calls.
  - The small rise from 1k to 40k is within what the bed can separate.
  - The statement timings below put the apply transaction itself flat.

## Statements, µs each, on the leader's own file

| Stored entries (file) | `log` row by idx | 8-row length page | `request` row by rid | apply transaction |
|---|---|---|---|---|
| 1,000 (12.5 MB) | 5.5 | 6.9 | 6.5 | 43.0 |
| 10,000 (95.9 MB) | 7.0 | 9.5 | 8.7 | 40.8 |
| 40,000 (373.9 MB) | 7.8 | 11.0 | 10.3 | 47.1 |

- The apply transaction is as `host::SqlStore` writes it: the retention delete, the log insert, the request insert and
  the raft state.
- From 1k to 40k the point reads gain 2 to 4 µs, which is B-tree depth. The apply transaction does not move.

**At ln's row size.** ln's stored rows averaged about 100 KB (246.5 MB over 2,425 entries). A synthetic file of 100 KB
rows with 27 request rows per entry (ln's 73,369 commands in 2,684 entries) gave:

| Stored entries (file) | `log` row by idx | 8-row length page | `request` row by rid | apply transaction |
|---|---|---|---|---|
| 300 (35 MB) | 72.5 | 7.4 | 6.4 | 322.0 |
| 2,400 (285 MB) | 90.9 | 13.3 | 8.1 | 316.5 |

- A whole-row read costs about its bytes.
- The 2,400-entry figures are at ln's end size, and the apply transaction does not move.

## What it does not cover

- **Local, not hosted.** Hosted Durable Object SQLite runs on another host and another storage layer.
  - A hosted per-entry cost that grows with the stored log is not ruled out.
  - F115's flip, the commit p99 line on a row longer than 300 s, stays the hosted check.
- **One shape.** The bed's register entries are small (4 KB values) and one command per entry. The statement timings
  cover ln's row size, but not ln's mix or its read memo.
