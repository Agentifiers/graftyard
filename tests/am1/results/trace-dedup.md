# Trace dedup: equal rows inside one save are distinct events (2026-11-24, $0)

LM-1, at the lead's request after `readindex-batch` (`tests/am1/results/readindex-batch.md`, "A correction to my
earlier IndexTaken figures"). No hosted work.

## The bug

`tests/nr3/tsave.mjs` saves each member's `/trace?since=` rows. Readers merged saves with a set of row tuples, so
two rows equal in every column were kept once. Inside one save, equal rows are two events:
- a learner's read-index batch number is `boot & !0xFFFF_FFFF` plus a counter, above 2^53, so the save's JSON
  rounds two asks in one millisecond to the same number (`IndexSent` 1, `IndexTaken` 5, `ResSent` 6, `Hedge` 11);
- `suspect` reports (`Reported` 28) carry colo, about, term and verdict, and repeat exactly under load;
- Work rows (30), `Lag` (3), `CallSent`/`CallEnded` (10, 15) and `Entered` (20, 21) repeat when one turn writes the
  same values twice.

`heavyturns.py`'s `trace()` also parsed each column through `float` before `int`, which rounds again.

## The fix

`tests/fanout/tracerows.py` (new) merges saves: a row is kept as many times as the save that holds it most. tsave's
saves never overlap (each asks `since` the last `at` it kept), and an end-of-run dump repeats them, which adds
nothing. Columns are parsed as Python ints, with no float step. The rounding the capture already did to batch
numbers above 2^53 is in the raws and cannot be undone, so readers count rows, not distinct batch ids. Running the
module runs its self-test, which covers both an equal pair inside one save (kept twice) and a dump that repeats a
save (kept once).

Converted readers (every reader that reads kind 1, 5, 6, 11 or 28, plus the rest of those whose recorded figures
moved): `heavyturns.py`, `learnershare/share.py`, `learnerrates.py`, `readnewest.py`, `readwait.py`, `reported.py`,
`campaigns.py`, `latesuspect.py`, `votershed/prep.py`, `work.py`, `hostwork.py`, `grown.py`, `b029join.py`,
`leadcycle.py`, `stallline.py`, `txmix.py`, `aba/stor.py`, `b033probes/host/common.py` and `susp2.py`,
`tests/f74/calls.py`, `held.py`, `rounds.py`, `tests/m0/cuttrace/attr.py`, `bycut.py`, `heldgrant.py`, `latejoin.py`,
`shares.py`, `tierahead.py`, `tests/nr3/gates.py` and `legs.py`.

## Proof on the snapshot that showed the gap

Learner `IndexTaken` rows (kind 5, `r[2]` ≥ 192) on the leader, inside the load window from t0 + 90 s, against the
asks counted from `RaftCall` (kind 33, whose call id is under 2^53 and never rounds):

| Run, snapshot | `RaftCall` learner asks | old set dedup | `tracerows.merge` |
|---|---|---|---|
| mrB, snapshot 39 | 1,052 | 547 | **1,052** |
| m3fsf2, snapshot 4 | 670 | 289 | **670** |
| m3fg3, snapshot 4 | 402 | 231 | **402** |
| mrB, every save merged | 9,769 | 4,898 | **9,770** |

The one row over on mrB's whole run is an ask whose `RaftCall` row fell in a gap between two saves [measured].

Row loss by kind under the old dedup, the worst run for each kind with 200 rows or more: `Lag` 3 75% (m41b33),
`IndexTaken` 5 78% (m36r33), `ResSent` 6 84% (m41b33), `Hedge` 11 69%, `CallSent` 10 51%, `CallEnded` 15 54%, 17 92%
(m36r33), `Reported` 28 81% (m45b33), Work 30 48% (pfA), `Entered` 20 and 21 64% and 47% (m28r33b). Kinds 4, 12 to 14,
16, 18, 19, 23 to 27, 29, 31, 33 and 34 lost under 1%.

## Figures that moved by more than 5%, corrected in place

Each correction carries "Corrected 2026-11-24 (`tests/am1/results/trace-dedup.md`)" and the figure it replaced.

| File | Figure | Was | Now |
|---|---|---|---|
| `readindex-batch.md` | m3fg3 read-index fit, ms a call | 0.17 + 0.408 | −0.33 + 1.087 (0.76 ms a call) |
| `readindex-batch.md` | m3fg3 options F / B / F+B / C / P, ms an entry | (smaller) | 10.2 (7.8%) / 9.3 (7.1%) / 19.5 (14.9%) / 7.2 (5.5%) / 21.8 (16.6%); range "9.5 to 14.9%" |
| `learner-relay.md` | m3fg3 learner bill row, ms an entry | (smaller) | 21.8 / 9.3 / 8.3 / 2.2, 41.5 (32%) |
| `seal-bytes-design.md` | m3fg3 flush fit and slices | (old fit) | −0.33 + 1.087 + 0.0082 a KB, 4 slices, 14.0 / 6.5, 16.2 / 299, 19.8, −5.8 / −3.3 |
| `lm1-memory.md`, `DECISIONS.md` (`late-suspect`) | m44 `suspect` reports | 31,509 (LAX 18,238, NRT 12,144, VIE 422, UNKNOWN 705) | 160,306 (LAX 98,623, NRT 59,481, VIE 1,217, UNKNOWN 985) |
| `lm1-memory.md` | m45 reports; follower `promised` / leader `leader` | 24,676; 24,461 / 215 (LAX 24,077, UNKNOWN 599) | 128,828; 123,938 / 4,890 (LAX 127,946, UNKNOWN 882) |
| `lm1-memory.md`, `c-design.md` | m47 reports | 41,356 (LAX 22,163, DFW 15,605, SJC 3,041, UNKNOWN 547) | 125,609 (LAX 72,641, DFW 42,784, SJC 9,548, UNKNOWN 636) |
| `lm1-memory.md` | m47 `/suspect`: repeats within 1 s; share left to step; entered that stepped | 63 to 75%; 29 to 42%; 26 to 37% | 37 to 72%; 28 to 63%; **every one** |
| `voter-shed-design.md` | m47 `/suspect` stepped with the cache, a second; removed | 8.4 / 6.1 / 18.3; 282 (54%) / 135 (40%) / 269 (51%) | 40.3 / 28.1 / 54.5; 250 (48%) / 113 (33%) / 233 (44%) |
| `voter-shed-design.md` | the cache alone answers | 63 to 73% | 37 to 72% |
| `m4-wp4.md`, `DECISIONS.md` (`read-newest`), `DESIGN.md` | m61 learner read-index calls; replay | 46,250; 36,467 (21% fewer); copies 21%, `HEDGE` 15.3% | 50,461; 40,678 (19.4% fewer); 18.9%, 14.0% |
| `learner-share.md` | m51 read-index batches pre / post change | 45.1 / 23.9 a second | 48.3 / 38.4 |
| `b033-m52.md` | learner read-index call time, m51 / m52, p50 and p90 | 174 / 923, 430 / 1,308; 958 / 1,443, 1,000 / 1,824 | 167 / 690, 484 / 1,378; 1,000 / 1,472, 1,000 / 1,960 |
| `b033-m61.md` | `work.py` leader steps; entry bytes; inside / outside stalls | 3,994; 460 MB; 25 steps, 1,112 B/ms / 1,904 | 4,647; 512 MB; 44 steps, 1,280 B/ms / 2,079 |
| `b033-m62.md` | `work.py` inside / outside stalls, B/ms | 1,287 / 1,385 | 1,620 / 1,547 |
| `nochange-row.md` | `hostwork.py` entry bytes per input, five rows | 3,070, 3,269, 908, 1,031, 1,492 | 3,420, 3,506, 1,129, 1,272, 1,731 |
| `lm1-rows-m3f.md` | `heavyturns.py` "no traced work" / "cuts or sends": m3fhs1, m3fg3, m3fsf2 | 28 / 8%, 25 / 10%, 22 / 5% | 24 / 11%, 18 / 12%, 18 / 8% |

Raws regenerated: `campaigns-m44b33.txt`, `latesuspect-m44b33.txt` and `-m47b33.txt` (m47's two-voter withholding 184
to 187 s, lapsing terms 1 to 2 of 18; m44's t13 first withholding 6.7 to 5.9 s; neither is quoted in a results file),
`learnershare-m51b33.txt`, `reported-m44b33.txt`, `-m45b33.txt`, `-m47b33.txt`, `flushslice.out`, `learnercost.out`,
`readcalls.out`.

Moved by 5% or less, not edited: B0-33's `heavyturns.py` shares on mrB, msA, pgA, pfA, blA, blB, blC2 and qlA to qlC
(no class moved by more than 2 points; apply and the deferred transaction did not move); `b029join.py` on m3fb29, r2,
r3 and b9m4a3r2 (apply per event 2.53 and 3.58 ms unchanged; "no traced work" 29 to 27% and "cuts or sends" 8 to 9% on
b9m4a3r2); `grown.py` m63 (confirm counts 183 to 190); `f74/rounds.py` on m41 to m44 and r29 (unchanged).

**One reading changed.** On m47, every `/suspect` that entered a voter reached `verdict`. The note that "only 26 to 37%
… reached `verdict`; the rest never stepped, whether dropped by their callers or lost to `ready_or`" was the merge's
loss, not the members'. The voter-shed design's case is smaller but stands: its `/read` shed is unchanged, and the
`/suspect` cache still answers 37 to 72% of stepped reports.

**One reading moved within its noise.** m62's stall steps now carry 5% more bytes a ms than steps outside (it read 7%
fewer). No step held a 1.4 MB message on m61 or m62, so encode-once is still not indicated on either.

## Verdicts

No B0-29 or B0-33 verdict moves [measured].
- **B0-33's `suspect` line.** Under the ruling that counts only `started` and `won`, the count is unchanged on every
  run (m44 61, m45 0, m47 49 `started`; mrB 0). mrB's 8 refused LAX reports were already counted per row: the old set
  gave 6 in its window, the recorded 8 came from the end-of-run dump. msA 0 either way.
- **B0-33's other lines** (commit p99, agent rate, leader changes, refusal shapes) come from a1, push logs and term
  samples, not from the trace merge.
- **B0-29's lines** read `apply ms an event` and commit p99, which did not move; `b029join.py`'s shares moved by at
  most 2 points.
- **The readindex-batch and learner-relay rulings stand.** m3fg3's option savings rose (F+B 14.9% of leader CPU an
  entry, P 16.6%), still under ×0.63; neither flip condition is met on mrB or m3fsf2.

## Not converted

These readers still merge with a set but read none of kinds 1, 5, 6, 11 or 28. Each one that reads a lossy kind
would undercount on the runs named above if rerun as it stands: kind 21 (`b033probes/host/b29.py`, `cutgap.py`,
`grid.py`, `pend.py`, `percmd.py`, `second.py`, `wk.py`, `probe2/3/5/9/10/12/13/18.py`, `callclock.py`; lossy only on
m28r33, m28r33b and m30r33b); kind 15 (`holdmodel.py`, `leadround.py`, `m0/clockguard/turns.py`, `m0/cuttrace/cmdwait.py`,
`cutmodel.py`, `entries.py`, `floor.py`, `freezesim.py`, `skew.py`, `tailcut.py`; 8 to 17% on m36 to m47 and r29);
kind 10 (`probe7.py`, `turns.py`); kind 20 (`leaderwl.py`, `replay.py`, `cutmodel.py`, `skew.py`; m28 and m30 only);
kind 30 (`probe4.py`, `probe19.py`). Their figures were not rerun. A reader that gets reused should move to
`tracerows.load` first.
