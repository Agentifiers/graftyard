# B0-33 host probes

Read-only probes over the committed traces `tests/fanout/raw/tsave-<tag>.jsonl.xz`. Python 3, standard library only. Written for the second-opinion review of B0-33 on origin/main `1e65e9c8` (2026-11-22). They are scratch probes: no tests, some hardcoded run lists.

## Running them

Run from the repository root, so `tests/fanout/raw` resolves, or set `B033_RAW` to the directory that holds the traces:

```
python3 .amp/in/b033-host-probes/second.py
python3 .amp/in/b033-host-probes/curve.py blB blA
```

Every script but `b29.py` imports `common.py` from its own directory. A script that takes tags reads them from the command line; `second.py`, `arms.py`, `grid.py` and `wk.py` have their run lists written in the file.

To use a new row, add its tag to `RUNS` in `common.py`: the load's start (UTC) and how many seconds site 0 led. Add its colo to `COLO`. `second.py` also needs a line in its own tuple list.

The trace must hold rows from before the load starts. The idle wire round trip and the clock offsets come from the `/raft` calls of that idle period (28 to 46 calls in the existing traces). Without them the scripts fail on `None`.

## Which clock to trust

Under B0-33's load the leader's `Date.now()` runs 1 to 3.5 s behind real time and wanders (`stale.py`, `drift.py`). So a duration between two leader stamps, such as a confirm's wall or a ticker stall, is not a duration. The probes rest on three things instead:

- **The near voter's clock.** Voters handle a call in 0 to 7 ms at p50 on the full rows (25 to 42 ms on m64r33), so their stamps are fresh.
- **Ring order.** Trace rows are pushed in execution order, whatever their stamps say.
- **The idle wire.** Round trips before the load, when the leader's clock is still fresh.

"Leader excess" in every script is the same quantity: the next entry reaching the near voter's handler, minus that voter's answer to the previous entry leaving, minus the idle wire round trip. It is the time the leader took between a voter's answer and the next entry, measured on the voter's clock alone.

## The scripts

- `common.py`: the loader. `RUNS`, `COLO`, `load(tag, sites, pad)` (one site's rows in the window, duplicates dropped) and `pc` (percentile). Judges no clock.
- `stale.py`: how far the leader's clock is behind, per run: voter handler entry minus leader send, less the idle value of the same difference. Also inputs/s, entries/s and agent commands cut/s. Run it first on any new row; it says whether the leader's stamps may be used at all. Voter clock against leader clock.
- `drift.py`: the same comparison in 30 s buckets, way out and way back. A negative "back" is the leader stamping an answer before the voter sent it. Both clocks, to show they disagree.
- `second.py`: leader excess, cycle and entries/s per run, for first leaders and for the second leader after a transfer (ncA site 2, m64r33 site 1). Voter clock; idle wire.
- `q2b.py`: the Q2 arithmetic: entries/s × agent commands per entry, ramp (0 to 90 s) against steady, share of capped cuts, cycle after a capped cut. Leader stamps, used for counts over long windows and for cut-to-cut gaps. Trust the means, not the p50 of single gaps.
- `curve.py`: per 15 s bucket: leader inputs/s, entries/s, commands cut/s, leader excess p50/p90, stalls. Excess on the voter clock; counts are bucketed by leader stamps, which is fine at 15 s. The stall count is a leader-clock reading.
- `percmd.py`: leader excess by the commands in the next entry (Cut rows summed in ring order between two appends), a linear fit, and the fit again in two input-rate bands (under and over 250/s). Voter clock and ring order. 73 to 97% of steady entries are capped at 64 on the full rows, so the fit leans on the ramp.
- `leanfit.py`: sabB2 and sabC only. "pre" is voter entry minus leader send; "post" is leader answer-stepped minus voter answer-left; both less offset and one-way wire, by commands in the entry. Needs a fresh leader clock, which only these two arms have. Read only pre + post at the median: the two parts alone swing ±0.6 s and their fits have opposite signs on the two arms. Not controlled for input rate, so small entries are also the ramp's entries.
- `lean.py`: the same two arms, one line per run: way out, voter handling, way back, turn, own confirm. Same caveat. Its sabA line shows what a stale leader clock does to these columns.
- `arms.py`: sync-ab's three arms (sabA, sabC, sabB2): inputs/s, cycle, leader excess, entries/s, commands per entry. Voter clock.
- `grid.py`: median leader excess by leader inputs/s (ring count over the voter-clock cycle) and by the next entry's KB, pooled by colo. Voter clock and ring order.
- `turn.py`: both voters mapped into one frame by their idle offsets. Turnaround after the near ack and after both acks, and how far the two voters disagree on when an entry left (7 to 134 ms at p50, the check that voter clocks agree). Voter clocks; assumes offsets stay constant over a run.
- `cutgap.py`: whether the next batch was cut early (Cut `peer` & 64) or at commit, and the leader excess of each kind. Voter clock and ring order. Its Cut → AppendQueued column is leader stamps and is not reliable.
- `order.py`: whether each voter AppendEntries' fetch (`CallSent`) was pushed before the entry's own transaction ran (`Flushed`). Ring order only.
- `pend.py`: whether the previous entry's transaction was still unconfirmed when the next entry's fetch was issued (its `Sync` row comes later in the ring), and the leader excess of each class. Ring order and voter clock. It checks the previous entry's transaction only, not other small transactions.
- `wakes.py`: per run, entry confirms by the 100 ms ticker wakes that ran inside them. N wakes mean at least (N − 1) × 100 ms of real time. The wake count needs no clock; the "stamped wall" columns are the leader's.
- `wk.py`: by wakes inside an entry's own confirm: whether the near voter's answer was stepped first (ring order), and the leader excess before and after that entry (voter clock). Both rise together with the wakes, which reads as a common cause, not a gate. The weakest of the three confirm checks.
- `susp2.py`: seconds with a `Reported` row about the leader, and whether a leader confirm of 1 s or more or a stall of 500 ms or more overlaps the 4 s before. Leader stamps for confirms and stalls, so it shows association with a late loop, not a storage duration.
- `b29.py`: B0-29's traces (m3fb29, m3fb2r2, m3fb2r3): cycle, leader excess by commands and by entry KB, leader clock behind. Stands alone. Voter clock; with no idle window in those traces, the wire is the 5th percentile round trip.

## What is not here

- Nothing reads refs. A `Cut` row carries commands per `AdmitClass` only (12 bits each in `batch`, in `AdmitClass::ALL` order), so a count of commands per ref or lane needs another source.
- The "5 to 6 ms of leader CPU per admitted command" was arithmetic, not a script: the leader's CPU-seconds per loaded minute from `tests/am1/results/nochange-row.md`, over 60, over the steady agent commands/s from `q2b.py`.
