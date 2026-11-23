# seal-bytes: multi-row seal statements for B0-33, and entry bytes for B0-32 (design, not built)

LM-1, 2026-11-23. Asked by the lead after `read-memo`'s set (`b033-readmemo.md`): a $0 design, no code until the lead
rules, with B0-32's pricing (`lm1-rows-m3f.md`, "Statement cut priced for B0-32"). Plant 101 is kept for it. Tags follow
`design/SHARED.md`, "Read this first".

**Short answer.**
- **The seal batch is not worth building for B0-33's gate.** On mrB's and msA's leaders it removes 31 and 33% of the
  fold transactions' statements, but only 2.1 to 3.4% of joined leader CPU, and B0-33's failing line (agent
  throughput, 107.3 and 106.8/s against 126) is not limited by leader CPU [inference, below].
- **BF-12 survives batching** with `RETURNING rid`: the duplicates are the rids the statement did not return [verified
  in local workerd]. If built, plant 101 is the check that misreads a partial batch.
- **Per call or per row:** in local workerd 62 to 65% of a one-row statement's cost is per call [measured]. Hosted, the split
  is [guess]; one probe on the next site day settles it and the log row's per-transaction against per-byte cost.
- **B0-32's lever is entry bytes, and no leader-side change found here reaches m3fsf2's ×0.63.** Encoding each entry
  once on the leader is 6 to 10% of its CPU [inference], line about 284 to 289 of 500 on m3fsf2 and 262 on m3fg3.
  Only fewer bytes per entry reach the per-byte storage cost, and that is `AGENT_INLINE_ENTRY`, in the agent block.

## 1. The seal batch, scoped to B0-33

**What it is.** `Tier::seal` (`workers/site/src/tier.rs`) writes one `INSERT OR REPLACE` per buffered row, and
`SqlStore`'s `Write::Apply` (`workers/site/src/host.rs`) one `REQUEST_INSERT` per request. The batch groups the rows of
one (verb, table) into multi-row statements of at most `floor(100 / params)` rows: 25 `request` rows (4 parameters),
33 `c_ref` rows (3), 25 `m_provenance` rows (4). Statement text stays far under 100 KB, and each row's blob is a
parameter [doc, `developers.cloudflare.com/durable-objects/platform/limits/`: 100 bound parameters a query, 100 KB a
statement, 8 MB a row]. Order inside a transaction does not matter for distinct keys, and `seal`'s map already holds
one value per key. FTS statements (`fts`) stay as they are.

**What it removes, B0-33** [measured, the leader's `bigTx` in the a1 files; fold transactions are those with more
than two statements]:

| | mrB (24.5 ms, ORD) | msA (22.3 ms, EWR) |
|---|---|---|
| Fold transactions sampled; log-only | 11; 9 | 10; 10 |
| Statements in them (mean) | 2,168 (197) | 1,548 (155) |
| `request` rows | 579 | 371 |
| Removed by batching `request`, `c_ref`, `m_provenance` | 670 (31%) | 515 (33%) |
| Removed by batching every (verb, table) | 1,777 (82%) | 1,232 (80%) |
| Flush fit (`heavyturns.py` flush slices) | 4.5 ms + 0.149 ms a statement | 3.2 + 0.127 |
| Flush CPU, joined CPU | 2,599 of 17,072 ms | 1,848 of 14,625 |

**Priced** [inference]. The flushed statements in the window are 12,720 (mrB, 159 flushes at a mean of 80) and almost
all of them sit in fold transactions. 31% of them is about 3,900 statements. At the marginal 0.149 ms that is 580 ms,
**3.4% of joined CPU**, if every statement's marginal cost is per call; at the local per-call share (about 62 to 65%, section 3) it
is 365 ms, **2.1%**. msA gives 2.9% at its own fit. Batching every table: 5.6 to 9.1%. The apply turn builds fewer `Stmt`s too, but its fit is per
event and per KB (0.159 ms a statement queued, collinear with events), so I count nothing there.

**What it would do to B0-33's lines** [inference]:
- **Agent throughput: nothing.** mrB's leader ran 0.38 vCPU; agent pushes end `full` at the edge's send bound (3,058
  and 3,334) and in admission (1,140 and 1,144), and the memo's 42% CPU cut per event did not raise the agent line
  (107.3 against blB's 118.3 commands cut) (`b033-readmemo.md`, "Leader CPU").
- **Human and merge commit stages:** under 3.4% of leader CPU; within noise of the two kept runs' spread (2,603 and
  2,196 ms p99).
- **Verdict: not worth building for B0-33.** Its value is cost (fewer statements billed nothing extra: rows written are
  the same) and later headroom on a leader that is CPU-bound, which B0-33's leaders are not.

## 2. BF-12's dup check under batched request rows

Today (`workers/site/src/txn.rs`): `REQUEST_INSERT` is `ON CONFLICT (rid) DO NOTHING`; on test installs
`commit_tx` reads the insert's `rows_written`, and 0 means the request id was already on disk, so it writes a `dup`
row in the same transaction. `GroupFold::apply` refuses the double apply first, in memory (`RequestStore::put`).

A multi-row insert breaks that test two ways [verified in local workerd 2026-11-23, wrangler 4.153, `/tmp` scratch]:
- `rows_written` counts index rows too: three new rows wrote 9 (table and `request_at`), and a batch of one new and two
  present rows wrote 3, not 1. So "0 written" never fires for a batch with any new row.
- A batch can be partly duplicate: [b, d, c] against [a, b, c] inserted only d.

**The design.** On test installs the batched statement ends `ON CONFLICT (rid) DO NOTHING RETURNING rid`, and the
duplicates are the batch's rids that did not come back. Measured: `INSERT … (a),(b),(c) … RETURNING rid` returned
[a, b, c], and then [b, d, c] returned [d]. `commit_tx` writes one `dup` row (rid, idx) for each, in the same
transaction, as now. Production keeps the statement without `RETURNING`; the stored rows are the same. A batch never
holds one rid twice: an entry's `requests` are distinct, and `seal` batches one entry's input.

**Plant 101** (if built): the dup check keys on the batch's `rows_written == 0`, the current single-row test, which
misses a partial duplicate. The test: a batch of three with only the middle rid on disk must write exactly one `dup`
row naming it, and the planted code writes none. A second plant worth the same test: treating returned rids as the
duplicates (inverted), which writes two wrong rows.

## 3. Hosted cost per call against per row

**Local** [measured, workerd via `wrangler dev`, one Durable Object, 50 transactions of 60 rows into each of
`request`, `c_ref`, `m_prov`, three runs each]:

| Rows a statement | Statements | ms for 9,000 rows |
|---|---|---|
| 1 | 9,000 | 116 to 125 |
| 8 | 1,200 | 59 to 61 |
| 25 | 450 | 45 to 48 |

That is about 0.0087 ms a call and 0.0047 ms a row: **62 to 65% of a one-row statement is per call** locally (the 8-row runs sit a little above the two-term fit).

**Hosted: [guess].** Hosted per-statement costs are 7 to 50 times the bed's (`b033-applyprice.md`), and nothing hosted
separates call from row. **Probe to settle it**, on the next site day of my next code branch, test-faults only, $0
beyond that site day: a member route that runs, in one request, (a) n transactions of k rows as k one-row statements,
(b) the same as one k-row statement, and (c) n one-row `log`-shaped transactions of 4, 32, 128 and 256 KB, each on
its own scratch table and dropped after. Its CPU comes from Workers Logs per invocation. (c) also settles B0-32's
open question in section 4: whether the slow hosts' one-row `log` flush (16 to 23 ms) is per transaction or per byte.
Locally (c) is per byte and small: 0.015, 0.04, 0.155 and 0.305 ms a transaction at 4, 32, 128 and 256 KB [measured].
Hosted, mrB's one-statement flushes cost about 5.0 ms and m3fsf2's 22.5 ms at about 130 KB (`stmt4.py` scratch, flush
slices with one statement against slices with no work), 30 to 150 times local.

## 4. B0-32: entry bytes per entry and apply cost per KB

**What B0-32's burst entries cost** [measured, `lm1-rows-m3f.md` and `heavyturns.py`, m3fsf2 (52.5 ms) and m3fsf1
(27.3 ms)]:
- Deferred transaction 41% of joined CPU on m3fsf2 (617 of 1,495 ms), 21% on m3fsf1. Eight of its twelve transactions
  are a single `log` row of about 128 KB, at 22.5 ms each on m3fsf2 against 3.0 on m3fsf1.
- Apply 27% (409 ms), fit 12.8 ms + 0.445 ms a KB queued on m3fsf2 (2.0 + 0.560 on m3fsf1), r 0.84 against KB.
- Every byte of an entry is paid several times on the leader [verified in code]:
  1. `push_entry` (`crates/raft/src/member/log.rs`) runs `encode_entry` and SHA-256 for the digest and length.
  2. `SqlStore`'s `Write::Append` (`workers/site/src/host.rs`) runs `raft::digest(e)` again (a second canonical encode
     and SHA-256) and `codec::to_bytes(e)` (postcard) for the row.
  3. `send` counts held bytes with `raft::encode_entry(x).len()` per entry of every `Append`, and `call` encodes the
     whole envelope with postcard, once per peer: 2 followers and 8 learners on B0-32, so up to 20 full encodes of a
     128 KB entry.
  4. The `log` row's 128 KB into SQLite, and the shards' `/entries` pages.
  The bed prices 2 at 2.0 ms an entry at 128 KB, and one postcard envelope at 0.67 ms a peer [measured,
  `tests/leaderprof`, `lp-render.out`].

**Lever A, encode once (mine: `host.rs`, `crates/raft/src/member`).** Keep the digest and length `push_entry`
computed (the member already holds `digests` and `sizes`) and use them in `Write::Append` and in `send`'s byte count;
encode the entry's postcard bytes once and splice them into each peer's envelope. It removes 1 and 3 above except one
postcard encode: about 7 to 10 ms an entry in the bed at 128 KB and 10 peers [inference from the bed's per-unit costs]. It
touches `crates/raft/src` only to expose `size_at`, so the long sweep and the Porcupine check run with it.
- **Priced on m3fsf2** [inference]: the encodes sit in the apply and send slices, whose byte-linked part is at most
  0.445 ms × 73 KB ≈ 32 ms an entry (9 entries, about 290 ms). Removing a third to a half of it is 10 to 16 ms an
  entry, 90 to 145 ms of 1,495 joined, 6 to 10%: x ≈ 0.9 to 0.94, **line about 284 to 289** (`b032share.py`'s marks:
  ×0.9 → 289). It does not touch the 41% deferred transaction. **m3fg3: 262**, flat to ×0.6. On B0-33's fast
  hosts the same cut is a smaller share of each byte's cost.
- **Not enough for 450.** m3fsf2 needs ×0.63.

**Lever B, fewer bytes per entry (not mine: the agent block).** Only fewer bytes reach the `log` row's storage cost
and every encode above together. Those bytes are agent inline packs, bounded per entry by `AGENT_INLINE_ENTRY`
(128 KiB, `design/DESIGN.md` and `design/AGENT-LAYER.md`, the agent block), and a pack past it is staged
(`agent-stage-first`). If the deferred transaction (41% of m3fsf2's joined CPU) and the byte part of apply (about 19%) scale
with bytes, ×0.63 needs about 62% fewer bytes an entry, an `AGENT_INLINE_ENTRY` near 50 KB [inference]. But
every pack past it pays the bucket place on its push path, inside B0-32's 5 s line, and that cost is unmeasured on a
burst. The owner is B0-32's thread, which landed `agent-stage-first` and `AGENT_CMDS_ENTRY`; any change edits both
documents' agent block identically.

**Correction from B0-32** [inference from its cut rows, 2026-11-23]. The bytes are not the burst pushes': those are
256 KiB and stage first (485 of 500 on m3fsf2 and 496 of 500 on m3fg3 carry `stagefirst`). Entries' inline bytes come
from journey 9's background agent pushes of 10 to 70 KB. Entries are also smaller than bigTx suggests: the burst
window's cut rows (`tests/m0/cuttrace/cuts.py`) average 72 KB an entry on m3fsf2 and 95 KB on m3fg3, and bigTx's ~128 KB
is its ≥100 KB sample. So 62% fewer bytes needs an inline cap near 16 KiB (m3fsf2) or 24 KiB (m3fg3), not 50 KB:
clipping at 50 KiB removes only 32% and 38%. Probe (c) still gates lever B.

**The caveat on both.** Over the burst seconds the leader averaged 0.42 vCPU (m3fsf2) and 0.31 (m3fg3), so its commit
stage is lumpy turns and stalls, not a full CPU; CPU-proportional x is optimistic (B0-32's note, which I read the same
way). The flush fit cannot split per transaction from per byte on B0-32's raws, because nearly every one-row `log`
flush is about 128 KB; section 3's probe (c) does.

## 5. Flips

- **Seal batch, build it:** a guarded B0-33 or B0-32 row whose leader runs at 0.8 vCPU or more over its load window
  with the deferred transaction at 30% or more of joined CPU, and probe (a)/(b) showing hosted per-call at 50% or more.
  Then the batch is worth 3 to 8% of a CPU-bound leader.
- **Seal batch, never:** probe (b) costing as much hosted as (a) (no per-call cost).
- **Lever A, build it:** cleared by the lead with the next code branch; it is worth it if probe (c) shows the `log`
  row per byte and the bed confirms 7 to 10 ms an entry. **Drop it:** the bed's split shows the encodes under 3 ms an entry
  at 128 KB and 10 peers.
- **Lever B, for its owner:** probe (c) per byte, and a B0-32 burst's stage place p50 under 1 s.

## 6. The next code branch, if the lead clears it

The lead's ruling after msA: one $0 instrument rides the next code branch, not its own. Each leader alarm step
(`Repo::on_alarm`'s split, load, prsync, merge_queue, hookseq, activity, actions_trigger, snap; the archive's spawned
run) logs one console line as it starts: the step's name and the stored log bytes it is about to read (the dispatcher's
range summed from the member's `sizes`, the snapshot's batch, the archive's walk), only for a step with stored reads to
do, so an idle 50 ms alarm logs nothing. A reset then shows the step it was in and how much it had to scan, which tests
the rescan hypothesis in `b033-readmemo.md` ("msA's resets"). Probe (c) and lever A, if cleared, ride the same site day.
