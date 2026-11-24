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

## 7. Probe readings, run pb (graft-m2b, 2026-11-23)

**Setup.** Test-faults site day -pb from main 327ac2bb, with row B's shard image (sha256:db305d1e…): site-a 460cbcac,
site-b f5a3c62b, site-c 769d3000, edge b659229d at 100%. A fresh repository, 131405, had its leader on site 0, object
`repo:131405#1#f0` in ATL. `tests/fanout/proberow.sh` ran three reps of every arm, 2 s apart (19:14:44 to 19:15:46Z).
Each arm was one `/bench_seed {"sqlProbe": ...}` invocation of n = 50 transactions, and its CPU is that invocation's
Workers Logs `cpuTimeMs`. That covers the request, the scratch table's create and drop, and building the rows.
Raws: `tests/fanout/raw/probe-pb.jsonl`, `probe-pb-wl.json`, `probe-pb.fit`, `proberow-pb.status`. The 24
invocations join the 24 asks in order (`sqlprobe.py`'s `join`). Workers Logs stamped each one 2.1 to 3.5 s after the
client's `at`, so a time window paired some asks with their neighbour's CPU.

**Arms (a) and (b): per call against per row** [measured]. CPU per invocation, three reps:

| Arm | Rows a transaction | Statements | Rows | CPU ms | Median |
|---|---|---|---|---|---|
| (a) one-row statements | 60 | 3,000 | 3,000 | 48, 52, 53 | 52 |
| (b) one statement (33 + 27 rows) | 60 | 100 | 3,000 | 21, 18, 31 | 21 |
| (a) one-row statements | 8 | 400 | 400 | 16, 7, 26 | 16 |
| (b) one statement | 8 | 50 | 400 | 3, 3, 9 | 3 |

- Fit over the 12 invocations: 6.4 ms an invocation, plus **0.0099 ms a call** and **0.0052 ms a row**.
- **Hosted per-call share: 66%** of a one-row statement. Local was 62 to 65%, and local per call was 0.0087 ms. So
  inside one transaction on this host, a statement costs what it costs in the bed. It is not 7 to 50 times the bed.

**Arm (c): per transaction against per byte** [measured]. One `log`-shaped row a transaction:

| Row | CPU ms, 50 transactions | Median | ms a transaction |
|---|---|---|---|
| 4 KB | 5, 3, 4 | 4 | 0.08 |
| 32 KB | 4, 5, 4 | 4 | 0.08 |
| 128 KB | 19, 24, 29 | 24 | 0.48 |
| 256 KB | 57, 71, 55 | 57 | 1.14 |

- Fit: **−0.02 ms a transaction** (zero) and **0.0047 ms a KB**. The cost is all per byte. A 128 KB row costs about
  0.6 ms of CPU to write, building and copying the blob included.
- Not measured: the per-transaction cost of a flush in its own event. The probe's 50 transactions share one invocation,
  so their durability wait is one output gate, and that wait is not CPU anyway.

**Readings against the flips (section 5)** [inference from the readings above].
- **Seal batch.** "Never" does not fire, because (b) costs well under (a). The per-call condition of "build it" holds
  at 66%. But a call costs 0.0099 ms, so B0-33's cut of about 65 statements an entry (31 to 33% of blB's 206) saves
  about 0.65 ms an entry. At 61 events an entry and mrB's 0.92 ms an event, that is about 1.2% of the leader's entry
  CPU, under section 2's 2.1 to 3.4%. The vCPU and deferred-share conditions still decide. Keep it unbuilt.
- **The hosted one-row `log` flush is not SQLite's write.** The flush slices cost 5.0 ms on mrB and 16 to 23 ms on
  B0-32's slow hosts at about 128 KB (sections 3 and 4), against 0.6 ms for the row's own write here. The other 4 to
  22 ms is other work in the flush slice. Lever A's encodes and digests are candidates for it. This host's per-call cost
  matches local, so it is probably a fast host. A slow host might double these readings, which still leaves the write
  at most a sixth of a slow-host flush.
- **Lever A.** Its first condition is met: (c) reads per byte, with nothing per transaction. Its second, the bed
  confirming 7 to 10 ms an entry at 128 KB and 10 peers, is still open and costs $0.
- **Lever B (B0-32).** The storage part of an entry's bytes is small: 72 to 95 KB is 0.33 to 0.44 ms of SQLite write.
  A 62% byte cut saves about 0.25 ms of storage CPU an entry. Whatever else lever B buys comes through the same encodes
  lever A removes, without changing agent semantics.

**The instrument, hosted.** On the fresh leader the only step lines were the archive walk's, once a minute:
`repo:131405#1#f0 alarm step archive scans stored 1..=2: 157 bytes` (19:14:22Z), then `1..=6: 1295 bytes` and
`2..=8: 1569 bytes`. No other step read the stored log, as expected for a group whose entries are all in memory.

**Cost.** graft-m2b's day went from $16.22 at the handover to $16.37 after the site day and $16.43 after the row: $0.21
in all, against the $3 stop. The sweep reported no shard container running.

## 8. Lever A's bed split, and what fills the hosted flush slice (2026-11-23)

The lead cleared this split at $0 after section 7 (no hosted run). There are two questions. What is a one-statement
flush slice's other 4 to 22 ms made of? And does encoding each entry once save 7 to 10 ms an entry at 128 KB and 10
peers?

**What a one-statement flush slice holds** [measured, `tests/fanout/flushslice.py` on the committed traces, output in
`tests/fanout/raw/flushslice.out`]. Each `/raft` slice's CPU also pays for the learner calls it answers. Their answers
are mostly held AppendEntries (`Trace::RaftCall`). The script fits CPU on calls and answer KB over each row's plain
slices (no apply, flush, send or confirm), and uses that fit to predict each flush slice's answers:

| Row | Plain-slice fit | Flush slices | CPU mean, p50 | Calls, answer KB | Answers predict | Left, mean and p50 |
|---|---|---|---|---|---|---|
| mrB | 0.12 + 0.190 a call + 0.0072 a KB | 68 | 5.3, 2.0 | 5.8, 235 | 2.9 | 2.4, 0.2 |
| m3fsf2 (52.5) | 0.15 + 0.289 + 0.0075 | 8 | 22.9, 11.5 | 15.9, 336 | 7.3 | 15.6, 5.3 |
| m3fsf1 (27.3) | 0.05 + 0.234 + 0.0047 | 5 | 3.2, 1.0 | 8.8, 183 | 3.0 | 0.2, −0.5 |
| m3fg3 (55.8) | −0.33 + 1.087 + 0.0082 | 4 | 14.0, 6.5 | 16.2, 299 | 19.8 | −5.8, −3.3 |

Corrected 2026-11-24 (`tests/am1/results/trace-dedup.md`): the trace merge no longer drops equal rows inside one save; m3fg3's fit moved from 0.17 + 0.408 to −0.33 + 1.087 (one 46 ms slice with 3 calls carries it; capped at 20 ms of CPU it reads 0.16 + 0.293, 0.46 ms a call). Before it, m3fg3 read 0.17 + 0.408 + 0.0081, 3 slices, 16.3 and 6.0, 15.0 and 262, 8.4, and 7.9 and 0.0. mrB's 68 flush slices are 67 (mean 5.4, left 2.5 and 0.3) and m3fsf1's 5 are 4 (mean 3.8, 10.2 calls of 177 KB, left 0.5 and −0.4).

- **The "flush slice" is mostly a learner-answer slice.** At the median, the calls the slice answered explain all of
  its CPU on every row (left p50 between −0.5 and 5.3 ms). The means are pulled up by a few slices: mrB's 128 ms
  outlier, and m3fsf2's 77 and 47 ms slices, two of which carried a storage confirm.
- **Answer bytes cost 0.0047 to 0.0081 ms a KB hosted**, plus 0.19 to 0.41 ms a call. A postcard envelope encode is
  0.0036 ms a KB in the bed (below), so an answer costs about one encode of what it carries at 1.3 times the bed
  (m3fsf1) to 2.0 (mrB) and 2.1 to 2.3 (m3fsf2, m3fg3), plus the request.
- **So section 7's "other 4 to 22 ms" is learners, not the flush.** Sections 3 and 4 priced the one-row flush at
  16 to 23 ms on slow hosts because these slices were taken as flush-only. They are not: each answered 9 to 16 learner
  calls carrying 180 to 800 KB of held entries.

**Copies of each entry leaving the leader** [measured, same traces, per entry replicated in the window]:
- mrB: 2.1 call copies (the two voters) of 168 KB and 4.3 learner copies (4 learners), 574 KB held in answers.
- m3fsf2: 1.9 call copies of 107 KB and 7.8 learner copies (8 learners), 684 KB in answers.
- m3fg3: 2.4 call copies of 140 KB and 9.6 learner copies, 1,012 KB in answers.
- That is 6.4 to 12 copies an entry, about what "10 peers" assumed, and each copy is one envelope encode.

**The bed's encodes** [measured, local workerd, `tests/leaderprof/encodes.sh` (`/encodes` in the bed), five asks at
each size, 30 reps a stage, medians, ms]. The entry is B0-33's 60-command mix, with one inline pack of the given size
and every other push stored. `raw_*` and `clone` read 0.0 to 0.03 at every size.

| Inline KB | Canonical KB | Envelope KB | `raft::digest` | of it SHA-256 | `log` row | Envelope encode | Envelope decode | Pack as base64, encode / decode |
|---|---|---|---|---|---|---|---|---|
| 4 | 11 | 11 | 0.13 | 0.13 | 0.07 | 0.07 | 0.07 | 0.03 / 0.00 |
| 32 | 39 | 48 | 0.37 | 0.37 | 0.20 | 0.20 | 0.23 | 0.17 / 0.17 |
| 72 | 79 | 102 | 0.77 | 0.73 | 0.37 | 0.37 | 0.47 | 0.33 / 0.40 |
| 95 | 102 | 132 | 0.97 | 0.97 | 0.47 | 0.47 | 0.60 | 0.43 / 0.53 |
| 128 | 135 | 176 | 1.30 | 1.23 | 0.60 | 0.63 | 0.77 | 0.57 / 0.73 |
| 256 | 263 | 347 | 2.47 | 2.47 | 1.23 | 1.23 | 1.60 | 1.20 / 1.50 |

- **The digest is SHA-256.** `encode_entry` itself is 0.03 ms or less at every size, so the length checks
  (`append_batch`, the catch-up and `Host::send`'s held bytes) cost nothing, and so does cloning an entry.
- **A postcard encode, the `log` row or an envelope, is almost all base64.** `PackRef::Inline` serializes as base64
  (`pack::b64`). At 128 KB the pack's base64 is 0.57 of the envelope's 0.63 ms. The same bytes as postcard bytes
  encode and decode in 0.03 ms or less.

**Lever A as designed: confirmed in range, but only at the top.** It keeps `push_entry`'s digest for
`Write::Append`, saving one SHA-256, and encodes the envelope once for every copy. At 128 KB that is
1.30 + (copies − 1) × 0.63 ms: **4.7 ms at 6.4 copies (mrB's count), 7.0 at 10, and 8.2 at 12** [measured unit costs,
counted copies]. At B0-32's real entry sizes (section 4's correction), it is 4.0 ms an entry on m3fsf2 (72 KB, 9.7
copies) and 6.1 on m3fg3 (95 KB, 12 copies). So 7 to 10 ms holds at 128 KB with 10 or more copies, and the condition
reads **met at the bed's terms, under them at B0-32's measured sizes**.

**Hosted, this work costs 1.3 to 2.3 times the bed**, by the answer-byte fit above, not the 7 to 50 times SQLite
calls cost. Scaled so, lever A is about 8 ms an entry on m3fsf2 (4.0 × 2.1) and 14 on m3fg3 (6.1 × 2.3). Their
leaders' CPU on all routes over the burst window is 2,550 ms and 1,835 ms. Over the entries those leaders replicated
in it (about 20 and 14, by the AppendEntries rows' index span), that is 128 and 131 ms an entry, and lever A is 6 to
11% of it. Over the 9 and 6 entries `heavyturns.py` counts as applied in joined slices, it is 3 to 5%. So x ≈ 0.89
to 0.97: m3fsf2's line about 284 to 290 (section 4's marks: ×0.9 → 289), against ×0.63 for 450, and m3fg3 flat (it
needs ×0.2). B0-33's mrB entries carry 8 to 10 KB inline, so lever A is under 1 ms an entry there.

**A cheaper and larger lever in the same place: base64 out of the wire and the row** [inference from the table].
Every copy of an inline pack, in the `log` row, every envelope and every `/entries` page, is base64 inside postcard.
Encoding is 90% of an envelope's cost, and decoding the same again on each receiver. Bytes would make every encode
and decode of the pack about free. They would also cut the canonical encode's hash input by nothing (SHA-256 runs
over the raw pack either way), and every copy's size by 25%.
- At 128 KB and 10 copies, alone, it removes 10 × 0.57 (envelopes) + 0.57 (row) ≈ 6.3 ms of the leader's encodes in
  the bed, about 13 ms on a slow host. It also removes 0.73 ms of decode on every receiver, and a quarter of the bytes
  held and sent.
- With lever A in, the leader encodes each entry once, so base64 removal adds only that one encode's and the row's
  base64 (about 1.1 ms at 128 KB). Together they take the bed's 9.5 ms of encodes at 128 KB and 10 copies (two
  digests, the row, ten envelopes) to about 2: one SHA-256 and the envelopes' frames. Its own value after lever A is
  the receivers' decode and the bytes.
- **But it is a wire change.** `PackRef::Inline`'s serialized form is in the stored `log` row and in every
  member-to-member envelope, so it changes `CODEC_VERSION` or `PackRef` (`crates/git/tests/wire_tags.golden` makes
  variants append-only), and old rows must still read. It also needs a site day, and a rolling deploy mixes codecs
  between members.
- I'd scope it only for the receivers and the bytes. Both together, at B0-32's sizes and the hosted scale, take
  about 10 to 16 ms an entry off the leader: x ≈ 0.88 to 0.92 by the replicated count, m3fsf2's line about 287 to
  290, still short of ×0.63.

**Second condition, my reading.** It holds only as "7 to 10 ms at 128 KB and 10 peers" in the bed; it is 4 to 6 ms
at B0-32's measured entries (8 to 14 hosted). Either way it is 3 to 11% of a slow leader's per-entry CPU, so it can't
lift B0-32 alone, as section 4 said. Its value is that it is pure CPU with no behavior change (`decide`, the log, the wire and every
reply byte unchanged), and it compounds with base64 removal. It is a lead's call whether 3 to 11% is worth a raft
change, the long sweep and Porcupine.

**Design sketch, if built** (nothing built). Raft keeps the digest and size it already computes per entry.
1. `crates/raft`: `Write::Append` carries `(LogEntry, Digest)` (or the member exposes `digest_of(i)`, which it
   already has), and `SqlStore::write` uses it instead of `raft::digest(e)`. This is one SHA-256 less per entry on
   every member, not just the leader.
2. `workers/site/src/host.rs` `Host::call`: memoize the postcard bytes of an `Append`'s entries by (index, digest) for
   the current send burst. Encode `[Envelope]` as postcard's frame around the cached entry bytes: postcard encodes a
   `Vec` as a varint length then its items in order, so the envelope's bytes are the fields before `entries`, the
   cached entry encodings, then the fields after (`commit`, `transfer`, `lead_seq`): byte-identical to
   `codec::to_bytes(&[env])`.
3. `workers/site/src/member.rs`: a learner's answer (`codec::to_bytes(&out)`) goes through the same splice for its held
   `Append`s.
4. **Tests.** The spliced bytes equal `codec::to_bytes` for envelopes of 0, 1 and several entries, with inline and
   stored packs, decoded by the receiver's path. A plant drops the length prefix, or reuses a cached encoding for an
   entry whose digest differs after a truncate. The long sweep and Porcupine run, since `crates/raft/src` changes.
5. No wire, log-row, reply or design-doc change; DECISIONS gets an entry.
