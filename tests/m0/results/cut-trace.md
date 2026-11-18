# cut-trace: B0-29's commit tail by cut, and the queue split on one clock (2026-11-18)

Code only, no hosted run. The trace is B0-29 run 4 (`tests/am1/results/b029-m2b.md`, "Run 4"; a1 `mwlhgt8y`, repository
131356, `tests/fanout/raw/tsave-m2bb29d.jsonl.xz`, load from 1794968230152 ms, 02:17:10Z). Branch `cut-trace` from main
436095f7. The operator's ask: what ended each cut, which limit binds, and each candidate's share of the slot wait.

## What the trace shows

### The `queue:slot` figure mixes two clocks

A command's admission time is the isolate's own clock (`now_ms()`, in `member.rs` `commit_all_held`, the lease loop in
`host.rs` and `actions.rs`). `Host::note` took the queue's other end, and the last apply that splits it, from the
member clock (`Host::now`), which lease-clock (`0b5769b7`, 2026-11-15) floors by peer and edge stamps. Under load that
clock leads the isolate's own, so `queue`, `queue:slot` and `queue:post` grew by the lead. `commitMs` was always on
the isolate's own clock at both ends (`release`).

| run 4, leader, 10,882 commands | p50 | p90 | p99 |
|---|---|---|---|
| queue as `/stats` measured it (member clock − own clock) | 602 | 844 | 2,532 |
| queue on one clock (own clock − own clock) | 350 | 518 | 934 |
| member clock lead at `/commit` arrivals | 261 | 434 | 1,495 |

The lead is a lower bound: it is rebuilt from the peer stamps the leader received (`CallArrived`, `AnswerArrived`),
and edge stamps are not traced (`tests/m0/cuttrace/skew.py`). Over every leader input it is p50 248, p90 407, p99
1,238, max 2,817 ms (`cutmodel.py`, 79,005 inputs).

So a command waits for about one to two entries, not ten: the queue p99 is 934 ms on a cycle p50 of 216 ms.

### No admission cap binds

- 3.8 commands per entry (9,940 over 2,604 entries).
- a1's admission samples, every 10 s: mean queued 2.3, max 8. Capacity read 424 to 986 commands a second, so the agent
  bucket refilled at 212 to 493 a second against 16.6 offered. `pressure` was never on; `hot` was on in 59 of 60.
- Bytes: 442 of 2,558 entries carried over 90 KB on the wire. That includes agent inline packs, which ENTRY_SOFT does
  not count, so it does not show a cut at ENTRY_SOFT.
- So the likely end of nearly every cut is an empty queue [inference: the trace has no per-cut row, which is why this
  branch adds one].

### Where the commit time goes

Per entry (`entries.py`): cycle p50 / p99 216 / 461 ms, round 178 / 349, the idle gap from an apply to the next append
p50 4, p90 138, p99 269. 79.7% of the time an entry is in its round.

A model of which entry each `/commit` rides (`cutmodel.py`, `shares.py`): under read-ahead the next batch freezes at
the entry in flight's append plus W. At W 80 ms it gives commit p50 / p99 442 / 970 against the measured 433 / 1,122.

| part of a command's commit (W 80 ms) | share of the mean | share of the tail (over p99) | p99 |
|---|---|---|---|
| its own entry's round | 42% | 54% | 440 |
| frozen behind the entry in flight | 22% | 21% | 325 |
| waiting for the freeze | 25% | 16% | 355 |
| apply to the next append (deps, decide) | 11% | 9% | 284 |

The operator's two candidates:
- **The commit-to-next-cut gap:** 11% of the mean, 9% of the tail.
- **`append:2`'s far leg** (634 ms against m27's 457): about 0%. The near voter (site 1) made the quorum on 2,587 of
  2,590 entries, and the far voter made none of the 15 tail commands' own quorums.

### The floor with one entry in flight [inference]

A command that arrives just after a cut waits for the entry in flight and then rides the next one, so its commit is
about two rounds plus the gap. With one entry in flight, an invariant that stays (`AGENTS.md`), B0-29's p99 is then
about two round p99s: 600 ms needs a round p99 near 250 ms, against 349 ms in run 4. This may become an operator
question about the target. Change (c), on rounds, is held until a run has per-cut rows.

## What this branch changes

**(a) One clock for the queue split.** `Host::note` takes both clocks (`Clocks`). The queue, `queue:slot`,
`queue:post` and the last apply that splits them read the isolate's own clock, the one the admission time and
`commitMs` read, so queue plus the entry's round adds up to commitMs. Cycle, round and apply times stay on the member
clock, where both of their ends are.

Every reader of a command's `at` and of the append time was checked: `Host::note` (queue and split), `commitMs` in
`release`, the test-faults kills and `fault_for` (request id only), the refused path, and the `/stats` admitted count.
The append time feeds `last_append_at` (`cycle_ms`), `appended` (round), the apply time and `last_apply_at`. All are
`/stats` only; none feeds an admission or expiry decision.

**(b) A per-cut trace row.** `Lead::cut` returns a `raft::CutNote` with its batch: commands taken and left per class,
the batch's soft and total bytes, the agent tokens left, the frozen batches held, whether an entry was in flight, and
`CutEnd`, what ended it: `Empty` 0, `Soft` 1 (ENTRY_SOFT, or the host's soft bytes), `EntryMax` 2, `AgentTokens` 3.
The codes are append-only. `cut_fresh` pushes each non-empty cut to `Out::cuts`, which nothing in the core reads. The
site host writes one `Trace::Cut` (kind 29, checked free on main, `m0-adaptive-append` and `narrow-reads-3`) per note,
under test-faults only; `cut_row` gives the bit layout. Behaviour does not change: the cut takes the same commands in
the same order.

Not recorded in `cut`: a skipped early cut (read-ahead full, a batch with no dependency, a read not yet measured).
Those return before `cut` runs, so no row means no cut was made.

## Tests and plants

- `host::tests::the_queue_split_reads_the_clock_its_admission_read`: a member clock 1,000 ms ahead of the isolate's
  changes no queue figure. On main's rule it fails (queue 1,420 against 420).
- `host::tests::a_cut_row_packs_its_note_where_the_trace_reader_looks`: the bit layout, with saturation.
- `admission::each_cut_records_what_ended_it`: three commands a second apart each cut alone with `Empty`; twenty
  10 KiB Human pushes behind one entry cut 9 at `Soft` with 11 left; at 2 commands a second each cut of six queued
  agent commands takes one and ends `AgentTokens`, the last `Empty`.
- `tests/m0carried/plant39.py`: 8 of 8 caught. Three plant main's member-clock rule into the queue, the split's end
  and the last apply; five plant wrong end codes, swapped byte limits, a Human queue ignored, `left` read as `taken`,
  and Human commands left uncounted.

## Run 5: the per-cut rows on graft-m2b (2026-11-18)

B0-29 run 5 (`tests/am1/results/b029-m2b.md`, "Run 5"; a1 `mwlu6gge`, repository 131357, site day m49 from main
`94fdfb48`, load from 1794990053053 ms, 08:20:53Z; `tests/fanout/raw/tsave-m2bb29e.jsonl.xz`). Commit p99 669 ms
(p50 355, n 10,696), against 1,122 in run 4. The build also carries `b032-shed` and other merges since run 4, so the
fall is not cut-trace's: cut-trace changes no behaviour.

### What ended each cut [measured]

`tests/m0/cuttrace/cuts.py` over the leader's kind 29 rows in the load window:

| | |
| --- | --- |
| cuts | 3,057 (2,553 early, with an entry in flight; 13 with one batch held) |
| end codes | **`Empty` 3,057**; `Soft` 0, `EntryMax` 0, `AgentTokens` 0 |
| commands a cut, p50 / p99 / max | 3 / 9 / 14, all agent class |
| commands left queued after a cut | 0 on every cut |
| agent tokens left after a cut, p1 / p50 | 211 / 398 |
| soft bytes a cut, p50 / p99 / max | 1,527 / 23,525 / 44,731, against ENTRY_SOFT 100,000 |

So no admission limit ends a cut: every cut takes the whole queue, with tokens and bytes to spare. This settles
the [inference] above. A cap would move nothing on B0-29.

One reader fix: a row's `batch` passes through a JS number, and with the tokens above bit 48 it is past 2^53, so its
lowest bits round to the double's step (8 to 32 at these values). `cuts.py` now reads the double the site wrote. The
rounding reaches only the Human count (bits 0 to 11), marked `human~`; the agent, merge-queue and bot counts, the
tokens, `left` (in `index`, under 2^53) and the bytes are exact. Every row's Human count reads 0, which matches B0-29's
mix, but a later run with Human commands needs the tokens moved out of `batch` (a follow-up, not done here).

### The queue split on one clock, against commitMs [measured]

| `/stats`, leader | p50 | p99 |
| --- | --- | --- |
| commitMs | 355 | 669 |
| queue | 202 | 518 |
| queue:slot | 162 | 403 |
| queue:post | 12 | 273 |
| round | 147 | 272 |
| confirm | 42 | 105 |
| cycle | 182 | 430 |

Queue p50 202 plus round p50 147 is 349, against commitMs p50 355: the two now add up on one clock, as (a) intended.
At p99 the parts do not sum (518 + 272 = 790 against 669), as quantiles do not. The trace's own one-clock replay
(`skew.py`) gives queue p50 / p99 286 / 611 under its second-append model; the member clock lead at `/commit`
arrivals fell to p50 75, p99 417 (run 4: 261, 1,495), so the old mixed figure would have read 354 / 795.

### Does the two-round floor hold?

Per entry (`entries.py`): round p50 / p99 151 / 255, cycle 175 / 438, the gap from an apply to the next append p50 7,
p99 280. The shares model (`shares.py`, W 80 ms) gives commit p50 / p99 355 / 652, against the measured 355 / 669:

| part of a command's commit (W 80 ms) | share of the mean | share of the tail | p99 |
| --- | --- | --- | --- |
| its own entry's round | 41% | 20% | 255 |
| frozen behind the entry in flight | 18% | 7% | 175 |
| waiting for the freeze | 29% | 47% | 349 |
| apply to the next append (deps, decide) | 12% | 25% | 268 |

- **The floor's arithmetic holds:** 2 × round p99 is 510 ms. A command arriving just after a cut waits for the
  entry in flight and then rides the next one; the two rounds it spans sum to p50 294, p99 463 ms (`floor.py`
  replay, `tests/m0/cuttrace/floor.py`). The commit those arrivals see under that model is p50 / p99 439 / 743, a
  model that rides the second append; the measured commitMs is 355 / 669.
- **The tail is no longer the rounds.** Round p99 is 255 ms, at the floor's "near 250 ms", and commit p99 is still
  669. What sits above the two rounds is the wait for the next freeze (47% of the tail, p99 349) and the apply to
  next append gap (25%, p99 268, the dependency read and `decide`). Commit minus the two rounds it spans is p50 135,
  p99 478 ms.
- So the floor is necessary, not sufficient [inference]: a round p99 near 250 ms was reached and 600 ms was not. The
  remaining 69 ms over the line, and 9 ms over the 90% line, sit in the freeze timing (`cut_early`'s wait of a mean
  round less a mean read) and the dependency read after the apply, not in a cap or the round. Change (c), on rounds,
  would not close it; the operator's question about the target now reads: does B0-29 need the freeze or the
  post-apply gap moved, or is 669 ms a long tail under the 90% rule's spirit (it is 9 ms past its letter).
- The far voter made 2 of 3,080 quorums and none of the 42 tail commands' own (`shares.py`).

## Full checks, the long sweep and Porcupine

- Full checks on `cut-trace`: fmt, workspace clippy, wasm32 clippy for site, edge and ops (site and edge with and
  without `test-faults`), `cargo test --workspace` 1,151 passed and 0 failed, `go test` in `containers/git/server` ok,
  and the three `scripts/check_*.py` ok.
- Long sweep (`RAFT_SEEDS=2000 RAFT_STEPS=10000`, release, `RAFT_HISTORY`), 1,179 s: passed. Longest healthy-quorum
  stall: harsh 16,824 ms (seed 10356), gray 24,153 ms (11076), live 29,145 ms (10681). Main 436095f7, swept at the
  same time on the same orb: the same three, and every counter line of the output is identical. So the change moves
  no behaviour the sweep sees, as an instrument should. The live 29.1 s is main's own (F87).
- Porcupine (`tests/m0/porcupine`, `-seg 2000`): h Ok, 147,412 operations; h.gray Ok, 2,331,603; h.live Ok,
  5,317,887. The same counts as main's.

## Files

`tests/m0/cuttrace/`: `entries.py` (per entry: cycle, round, gap, bytes), `skew.py` (queue on one clock against
`/stats`'s, the member clock lead), `cmdwait.py` (entries each command waited), `cutmodel.py` (the freeze model across
W), `shares.py` (the shares table and quorum makers). Each takes the trace file and the load start in ms:

    python3 tests/m0/cuttrace/shares.py tests/fanout/raw/tsave-m2bb29d.jsonl.xz 1794968230152 80
