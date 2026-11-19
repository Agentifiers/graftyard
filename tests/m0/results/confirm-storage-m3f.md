# confirm-storage on graft-m3f: B0-29 row m3fp2 (2026-11-19)

This is the hosted check of `confirm-storage` (`decisions/DECISIONS.md`, "`confirm-storage`"), granted by the
orchestrator (T-01a17bda). It compares with m3fhg (`tests/m0/results/held-grant-m3f.md`).

- **Build.** A site day, m3fp2, from main `d8dcd513`, which carries `confirm-storage`, `lock-split` and Trace::Work.
  - Sites a/b/c serve `7e673088` / `9c9381ea` / `3c903ade`.
  - There was no edge day. graft-m3f-edge still serves `e25b8638` from m3fhg (main `ab6086b4`).
  - The image `sha256:1984a852…` was built from `d8dcd513`, because `containers/git` changed for lock-split
    (`65a679c5`).
  - The site-day log's "edge exit 0" line is printed even for a site-only day.
- **Row.** One B0-29 row on m3fhg's script (`tests/fanout/b029-m3f.sh`) with held-grant on and a fresh repo:65596.
  - a1 run `mwnxzqr1`.
  - Load ran 19:35:18.8–19:45:18.8Z through the egress proxy, under the harness-outage rule.
  - The leader was site 0 until 19:44:04.
- **Stop.** $6 on graft-m3f, counted from 19:10:37Z. The install spent **$2.46** (`acctcost.py`, `prefix_usd`, read
  at 20:33Z). The account total for the day was $43.56 at the row's end.
- **Raw files.**
  - `tests/fanout/raw/rows-m3fp2.status`, `terms-m3fp2.jsonl`, `arrivals-m3fp2.jsonl`, `edge500guard-m3fp2.status`
    and `tsave-m3fp2.jsonl.xz`.
  - a1's `a1-b0-29-mwnxzqr1.jsonl` and its samples file are under the ignored `tests/am1/raw/` on this thread's orb.

The row is judged against B0-29's 800 ms commit line, 880 at 90%, with a fail over 1 s (`FRAME.md`, "Operator
decision, 2026-11-19, B0-29's commit line").

## Verdict lines

| Line | m3fp2 | Against |
|---|---|---|
| `/stats` commit p99 | **1,160 ms** (p50 480, n 9,524). `bycut.py` models 1,155 for 19:35:19–19:44:00, so the /stats offset is about +5 ms here (+70 on run 7) | **Fail**: over the 1 s fail line |
| The same with stalls of 10 s or more removed | Unchanged: there was **no stall of 10 s or more** on any voter. The largest leader confirm was 885 ms | **Fail** |
| Per-minute modelled p99 | 1,264, 1,294, 966, 1,060, 874, 1,205, 755, 1,290, 1,117 | 1 of 9 minutes under 800 |
| Step-downs by reason | **1**: `LoopLag { clamped: 2 }` to site 1 at 19:44:03.680 on site a. **No Confirm trip**. The `stepDownWindow` had `confirms` slow 0 of 66; the two tripping samples are 500 / 500 in both `confirms` (storage) and `whole` | So no verdict on confirm-storage's change. Section 2 is about the LoopLag trip |
| Leader changes | **1**: 0 → 1 at term 3, about 19:44:04 | B0-29 has no leader-change line |
| Leader storage confirms of 2 s or more | **0** (adds 0.17 h of load with none to F99's count) | m3fhg: one of 23,957 ms |
| `merge:only` p99 | 15,836 ms (n 90, served 85.6%). Outside 19:44:03–19:44:30: p99 3,407. Both 503s are git cache warming after the change. The **11 × 409 are all `BaseModified`**, and each overlaps a merge on the same base that answered 200 | m3fhg 19,352 |
| held-grant's flip | **0 of 223** `ranges` and **0 of 3** merge rows walked after a held-and-stepped leader read | **Pass** (m3fhg 0 of 179) |
| Round p50 / p99 | Trace (append → apply): **196 / 429**. `/stats`: round 194 / 438, confirm 43 / 225 | m3fhg 180 / 323 before its stall, 182 / 322 after; run 7 167 / 316 |
| Edge 500s or exceptions | **0** status-500 and 0 exceptions. 1,448 × 503 at 19:44, in the change | One would have stopped the row |
| Harness outages | 5 s (0.8%), **valid** | Over 5% is invalid |
| Spend | **$2.46** on graft-m3f | $6 stop |

Load matched m3fhg's: 161 requests/s at site 0, with open requests p99 28. Colos were LAX only.

## 1. Why site 0's storage ran about 3× m3fhg's

### Measured

Leader (site 0) confirms over 19:35:19–19:44:00 (`Confirm`, kind 10), split by the statements of the deferred flush
that began each one (`Flushed`, kind 16), against m3fhg's 14:07:30–14:11:50 and run 7:

| | confirms | p50 | p90 | p99 | ≥ 200 ms | ≥ 500 ms | 1-statement flushes p50 / p99 | flushes of more than 20 statements p50 / p99 |
|---|---|---|---|---|---|---|---|---|
| m3fp2 | 3,111 | 49 | 60 | **273** | 46 | 11 | 41 / 259 | 50 / 433 |
| m3fhg before | 1,695 | 40 | 48 | 80 | 0 | 0 | 29 / 80 | 42 / 76 |
| m3fhg after | 1,787 | 41 | 50 | 107 | 2 | 1 | 30 / 96 | 43 / 162 |
| run 7 | 4,304 | 43 | 50 | 101 | 3 | 0 | 35 / 108 | 46 / 89 |

- **Not the flush's size.**
  - The slow tail is in every flush class, 1-statement included: 24 of the 46 confirms of 200 ms or more were
    1-statement flushes.
  - Flush statements were p50 1, p99 77 (max 225), against m3fhg's 1 / 63 (202) and run 7's 1 / 69 (224).
  - a1's `confirmSplit` agrees. Entries under 100 KB had storage p50 43 / p99 235 against m3fhg's 32 / 70 and run
    7's 36 / 90. Entries of 100 KB–1 MB had 41 / 225 against 33 / 175 and 36 / 82.
  - Gate and turn were 0 / 0 in every run, so the time is all `sync()`.
- **Not the entries.**
  - Entry bytes per AppendEntries were p50 36,655, p90 131,993, p99 177,763 against m3fhg's 33,296 / 121,327 /
    175,229 and run 7's 28,129 / 116,464 / 174,868.
  - There were 115 entries over 150 KB against run 7's 117.
- **The object was slower from before load.**
  - In setup (19:28:30–19:35:00, no load), site 0's 1-statement confirms were p50 **23** (p90 33) and its unpaired
    confirms p50 50, against m3fhg's 16 (p90 19) and 39.
  - Under load they rose to 31 / 52 against 21 / 44.
  - So the new network's leader object was already about 40% slower at the median before any load. That is the
    placement baseline that `lsrefsab.py` exists to read.
  - The status file's baseline line names site 1, the leader at the end, so it does not apply. It is recomputed here
    for site 0.
- **The followers' storage was normal.** Site 1's confirms were p50 14 / p99 56 and site 2's 10 / 22, against
  m3fhg's 13 / 28 and 7 / 57.
- **Colo or placement.**
  - Workers Logs carry no colo or placement for Durable Object invocations (`$workers` holds only the script name,
    version and URL).
  - The voters' `Reported` rows read LAX for the whole row, as on m3fhg.
  - So where site 0's object lived is not shown.
- **The instrument does not explain it.**
  - Trace rows on site 0 ran at 772/s against 722–755/s on m3fhg and run 7. Work rows are 28/s of that, about 4%
    more rows.
  - Site 0 logged **0** ticker-lag rows (kind 8, 20 ms or more) in the whole load before the trip, as on m3fhg (0).
    A trace that cost measurable CPU per step would show up there first.
  - The Work row's own cost is one loop over the step's outputs and one over the applied entries (`host.rs`,
    `work_row`'s caller), and only on test-faults.
  - m61r33 on graft-m2b carried the same Trace::Work build on B0-33 (`tests/am1/results/b033-m61.md`).
  - What the trace cannot show is time inside `sync()`, which is where the difference is. Gate and turn were 0.

### Conclusion

The leader's storage was slower from setup onward on a new object, with the same flush shapes, entry sizes and
load, and with followers at normal speed. Nothing in `d8dcd513`'s diff from `ab6086b4` changes what a site writes
per step:
- `confirm-storage` adds a timer and one `u32` per confirm;
- `replay-wait`, `repo-gone` and `lock-split` change actions, the registry and the git container.

This is the platform's storage for that object [inference: placement is not observable, but every input to the
flush matches and the setup baseline was already slow].

## 2. The 19:44:03 LoopLag trip

### The turn

From site 0's trace, 19:44:03.148–03.680 [measured]:
- At 03.148 site 0 appended entry 2792 (21,541 B), sent it, and deferred one flush.
- Inputs then stepped at 03.168, .170, .202, .218, .289 and .320, each sending one AppendEntries of 16–100 KB to a
  learner, then a resolution at 03.404.
- Then site 0's isolate clock stops for 276 ms. No trace row on site 0 is stamped between 03.404 and 03.680 except
  `Entered`, `IndexTaken` and `ResSent` for incoming `/read`s.
- The next turn, at 03.680, is the big one:
  - it ends two confirms of **532 and 533 ms** (both begun at 03.148);
  - its Work row sends **10 AppendEntries carrying 394,579 bytes of entries**, with 1 statement queued (37 B), 0
    entries applied and 0 resolutions;
  - it appends at index 2792 again to both followers (catch-up of 21,551 B).
- No ticker-lag row was logged on site 0 at any point in 19:44:00–04.5.
- The 10 AppendEntries are learner catch-up: 4 learners, each owed entries 2790–2792.

### The rule

`decisions/DECISIONS.md`, "A leader whose loop is behind steps down (`LOOP_LAG`)": "`STEP_DOWN` also trips when two
of the leader's storage confirms within 2 s took 200 ms or more and an exact multiple of 100 ms". Its why: "While a
Durable Object's event loop is behind, workerd clamps `Date.now()` to the overdue 100 ms tick's deadline, so a
confirm that begins and ends late reads a whole number of ticks."

### Why this trip is not the signature

- **The two samples are not a clamp.**
  - On the isolate's clock the two confirms read 532 and 533 ms, not whole hundreds.
  - They read 500 / 500 only on the member clock. Since `lease-clock` (`0b5769b7`, 2026-11-15), that clock is
    `max(isolate clock, largest peer or edge stamp)` (`workers/site/src/clock.rs`), and `confirm_begin` and `input`
    step at it.
  - Under this load the member clock ran ahead of site 0's isolate clock by p50 461 ms (68 to 1,132), from
    learners' and the edge's stamps. Measured: the window's 163 member-clock confirm ends against the trace's.
  - So a confirm's member-clock length is its isolate length plus the change in that lead between its begin and
    its end. At 03.148 → 03.680 the lead went from −925 to −453 ms (end offsets), and both confirms began and ended
    at the same instants. Two confirms sharing one sync give two samples of the same number.
  - Over the row's whole leadership, the isolate-clock confirms of 200 ms or more had one whole hundred, a single
    200 at 19:40:30. No pair falls within 2 s.
  - The rule as written was calibrated in `m0-carried-8` (2026-11-13), before the member clock existed. Read on the
    isolate clock, it would not have tripped here [measured, replay of site 0's 3,129 confirm rows].
- **A stall did happen, but the clamp did not show it.**
  - The 276 ms gap with only reads logged, then one turn holding 10 sends, fits a busy isolate [inference].
  - The ticker logged no lag, so the stall was under 20 ms on the ticker's own reading, or the ticker's timer was
    itself clamped.
  - The step-down cost more than the stall: a 503 burst (1,448 at the edge in 19:44), merge:only's two 503s, and
    the 19:44 minute's commit p99 of 1,117.

So the trip **matches the rule as written** (two member-clock samples ≥ 200 ms and whole hundreds within 2 s) and
**meets its flip**: "A hosted leader that trips `LoopLag` with no lag in its trace, or a run where the transfer costs
more than the lag it ends." There was no lag row, and the transfer cost more than 276 ms.

The fault is the clock the rule reads, not the threshold. Two confirms ending in one sync are one sample, and the
member clock rounds by the peers' stamps, not by ticks.

### F100

- F100 asks whether leader CPU on large cuts (1.4 MB or more) makes long turns.
- On this row, 0 steps sent 1.4 MB or more (max 1,229,360 B over 10 messages, p99 609,679).
- The trip's turn sent 395 KB, so the trip is not F100's case.
- The row had 0 ticker stalls of 100 ms or more on site 0, so it gives F100 no stall to join (`work.py`).
- The 276 ms gap before the 395 KB turn is one data point of a gap before a large learner catch-up turn. It is not
  shown to be caused by it, because the gap comes before the turn's own row [inference].

## 3. Commit without the change, with m3fhg's storage

`bycut.py` split for 19:35:19–19:44:00 against m3fhg's 14:07:30–14:11:30:

| | m3fp2 p50 / p99 | m3fhg p50 / p99 |
|---|---|---|
| modelled commit | 469 / **1,155** | 393 / 674 |
| cutwait (to its freeze) | 117 / 515 | 114 / 336 |
| held (entry in flight) | 85 / 353 | 32 / 231 |
| post (deps, decide) | 3 / 366 | 31 / 229 |
| own round | 201 / 466 | 184 / 340 |
| two rounds (ahead + own) | 401 / 902 | 370 / 590 |

How the leader's storage enters a round [measured]:
- The leader's own copy is never the last ack. In 2,001 entries its confirm after append (p50 43, p99 203) never
  ended after the near follower's ack.
- It enters through the output gate. An AppendEntries leaves only after every earlier write has confirmed
  (`decisions/DECISIONS.md`, `m0-carried-6`).
- Per call to site 1, by the leader confirm still running when the call was sent:

| leader confirm left at send | n | call p50 / p90 |
|---|---|---|
| under 50 ms | 1,295 | 178 / 239 |
| 50–200 ms | 685 | 223 / 297 |
| 200 ms or more | 23 | 556 / 929 |

  m3fhg had 971 / 30 / 0 calls in those bands, at the same 179 and 222 ms p50.
- Without the leader's confirm, the near round was the same: near ack minus leader confirm, p50 143–179 against
  m3fhg's 146–164.

Counterfactual: map site 0's confirm still running at each append, rank for rank, onto m3fhg's distribution.
Shorten that entry's round, and the `held` of the command behind it, by the difference [model]:
- **Round:** p50 / p99 184 / 343, against m3fhg's 180 / 323.
- **Commit:** p50 / p90 / p99 **444 / 622 / 964**.
- **Also mapping `post` onto m3fhg's** (deps reads ran slower too: `deps:all` p99 489 against 333 and 280):
  **432 / 606 / 866**.
- m3fhg itself, before its stall: 393 / 537 / 674.

So matching the storage takes about 190 ms off the p99 and leaves it at 964, still over the line. About another 100
comes from slower deps reads (Authz p99 401 against 260 and 245). The rest, 866 against 674, is in cutwait and the
two-round floor. The per-30 s windows show it: the 4 windows whose leader confirm p99 was under 110 ms had commit
p99 755. The 10 windows at 110–300 ms had 1,100, and the 4 above 300 ms had 1,315.

**Answer.** On this build, B0-29's commit p99 is storage-bound on this row: the leader's confirms decide the tail
through the output gate. With m3fhg's storage the model lands at 866–964 against m3fhg's 674. The rest of that gap is
deps reads and cutwait, measured slower here and not explained. With matched storage the row is between the lines,
not a code-bound fail [model; the mapping assumes each send waits on the confirm in progress at its append].

## Ranked code change

1. **`LOOP_LAG` judges the isolate clock, and counts one sample per sync.** The rule's evidence is workerd's clamp
   of `Date.now()`, but since `lease-clock` the member reads its confirms on the member clock, which peer stamps
   move in jumps. Today's only step-down on a valid B0-29 row met the rule's own flip (no lag row; the transfer cost
   more than it ended).
   - **Fix:** have the host report the isolate-clock length with each confirm, as it now reports storage, and have
     `LOOP_LAG` count whole hundreds on that. Confirms that end on one sync count once.
   - This needs a design edit (DESIGN, Shape, `STEP_DOWN`; DECISIONS, `LOOP_LAG`'s amendment) and `crates/raft`
     with the long sweep and Porcupine.
   - It would have removed this row's leader change. It would not have fixed this row's commit p99: the 19:44
     minute was 1,117, but the minutes before it were already 874–1,294.

For the commit line itself, no code change is supported. The row's tail is the leader object's slower `sync()` from
setup onward, through the output gate, on a build whose write path matches m3fhg's. **A rerun on a fresh network is
the right next step for B0-29's commit line.** Record the setup 1-statement baseline for the actual leader, and read
the row against its baseline.

## Close-out

- `retirenets.mjs 65596`: 11 names, all not joined.
- `purge.py` twice: 2,576 objects, then 0.
- `joined.mjs 65596`: "none joined of 1".
- `teardown.py --sweep` twice: no shard container running.
- Services `siteday-m3fp2` and `row-m3fp2` are stopped.

Scripts: `tests/m0/cuttrace/bycut.py` and `heldgrant.py`, `tests/fanout/work.py` and `lsrefsab.py`. The per-confirm
joins, the gate bands and the counterfactual ran as one-off scripts over `tsave-m3fp2.jsonl.xz` and
`tsave-m3fhg.jsonl.xz`. Their method is above.
