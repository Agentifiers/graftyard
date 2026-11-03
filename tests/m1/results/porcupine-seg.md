# Is Porcupine's segmented mode sound? (2026-11-03)

The question: `tests/m0/porcupine -seg` reported Illegal on the 2000-seed simulator sweep, which whole-history mode reports Ok. BM-1's `lin24b` finish, lin24, and B0-11 all rest on `-seg`, so we needed to know whether it can be wrong, and in which direction.

**Answer.** The old segmented mode had two bugs. Both are fixed in `tests/m0/porcupine/main.go`.

1. It dropped every write that got no reply. That produces false Illegal results when a later read saw such a write. The sim anomaly is this bug.
2. It cut all keys of one input file as one history. It used one key's last value as the start state for the next segment, whatever key that segment held. That can produce false Illegal results and false Ok results. Neither showed up in the gate runs, because `run.ts` `finish` and M0 both feed the checker one key per file.

lin24's verdict does not change, and neither does B0-11's. The lin24 checks that were run again give the same result, key for key and segment for segment (see "What changes for the gate runs"). lin24b's finish rebuilds the checker from the fixed source.

## The cut rule, before the fix

`segmented` (old code):

- Loads every row, then removes each write with `ok: false` (no reply). Failed reads were already removed in `load`.
- Sorts the rest of the file by call time. **All keys go into one list.**
- Walks the list, keeping `maxRet`, the largest return time so far. Operation `o` at index `i` is a cut point only if all of these hold:
  - `maxRet < o.Call`, so every earlier operation returned before `o` was called.
  - The next operation was called after `o` returned. Because the list is sorted by call, this means every later operation was called after `o` returned.
  - The segment holds at least `-seg` operations.
  - `o`'s result names the state. A `get` names its value. A `put` or a successful `cas` names its new value. A failed `cas` does not name the state, so it is skipped.
- At a cut it checks `ops[start:i+1]` with Porcupine, starting from `state`. Then it sets `state` to the state `o` names, and the next segment starts at `i+1`.

## The soundness argument, for one key's register

Take a single-key history H in which every operation has a reply. An isolated operation `o` is real-time after everything before it and before everything after it, so every linearization of H puts `o` between the two sides.

- **Ok is sound.** Say each segment linearizes from the state named at the previous cut. Then the segment linearizations, joined in order, form a linearization of H. Each one ends with its cut operation, so the next one starts from the state that operation leaves.
- **Illegal is sound.** A linearization of H splits at `o` into a linearization of the prefix that ends in the state `o` names. A `get` returning v leaves v, and a `put` or a successful `cas` of v leaves v. So if some segment cannot be linearized from the named state, H has no linearization.
- **A failed `cas` must not be a cut.** It says only that the state differs from `expect`. The old code skipped it, which is correct.

Two inputs break the argument, and the old code had both.

- **A write with no reply.** It may take effect at any time after its call, or never. With `Return = MaxInt64` it overlaps every later operation, so no cut falls after its call. The old code therefore dropped it. Its comment claimed that dropping such writes "can only make the check stricter". That is true for Ok. Ok without the write implies Ok with it, because the write can go last. It is false for Illegal. If a read saw the write's value, the history without the write is Illegal, while the real history is Ok. This is a false Illegal.
- **More than one key in one file.** A cut on key A sets `state`, and the next segment may be key B's operations. Two results follow:
  - **False Ok.** Take `put(B,b) [0,10]`, `put(A,y) [20,30]`, `get(B)=y [40,50]`. The cut after `put(A,y)` starts B's segment from `y`, so the stale read of B passes. The whole check says Illegal. With the old checker, `-seg 1` on this history printed `"result":"Ok"`.
  - **False Illegal.** Take `put(A,x)`, `put(B,y)`, `get(A)=x`. The old checker returned Illegal, though the history is linearizable.
  - Isolation is also tested across keys, so cuts become rarer. That costs memory and time but does not affect correctness.

## The root cause of the sim anomaly

The cause is the no-reply writes. It is not the shape of the sim history.

- **What the history looks like.** `/tmp/chk/hist.jsonl` (HARSH) has 2,000 seeds and 6,000 keys, named `s<seed>/k<n>`. Each seed gets its own block of client ids, so no client is shared across seeds (0 shared). Values are unique per key (0 duplicates). Every seed's clock starts near 1,000, so the seeds overlap in time. It has 160,066 rows, of which 27,735 are no-reply writes.
- **Dropping alone causes the Illegal result.** The same file with only its no-reply writes removed, checked whole by the old checker, is **Illegal**. The full file checked whole is **Ok**.
  - Reads saw a no-reply value 8,589 times, and successful `cas` operations expected one 1,497 times. That spans 3,536 keys.
  - On a 20-seed sweep, 255 of the 324 no-reply writes are never seen. Giving the 69 that are seen a return at their first observer gets every key but one to pass. That key's value 3.17 was written by a no-reply `cas` whose `expect` was the value of another no-reply write.
- **The overlapping seeds matter only because of the key bug.** On the same files the old code returned one segment. Every operation across 2,000 overlapping seeds overlaps some other operation, so no cut is ever isolated. That segment was the whole file without its no-reply writes, which is Illegal for the reason above.
- **The key bug, shown separately.** I took 20 GRAY seeds, which have no no-reply writes, and placed them one after another in time with distinct keys. The whole check is Ok. The old `-seg 2000` returned **Illegal** (10 segments), and the old `-seg 50` also returned Illegal.
- **GRAY 2000 seeds.** It has 209 no-reply writes. With them removed, the whole check is Illegal. This is the same cause.

## The fix (`tests/m0/porcupine/main.go`)

- **Partition by key first.** `segmented` runs `segmentedKey` on each key's operations, using the model's own `Partition`. Each key gets its own cuts and its own start state.
- **A no-reply write that someone saw.** If a `get` saw its value, or a successful `cas` expected it, the write took effect before that operation returned. It gets `Return` = the first such return and `ok: true`, and it takes part in cuts like an acked write. This applies only when the value has one writer on the key. Otherwise the write stays pending.
- **A no-reply write that nobody saw is pending.** First the segment is checked without pending writes. Ok there is sound, by the same argument as above.
  - If that check is Illegal, the segment is checked again with every pending write called before the segment's last return. A pending write called later cannot take effect inside the segment.
  - Illegal on the second check is sound, because giving a write more freedom can only allow more linearizations.
  - Ok on the second check is reported as Ok only if no lent write was also lent to an earlier segment. A write takes effect at most once. If one was lent twice, the segment reports Unknown, and Unknown already fails the gate.
- **Cut points do not change.** The rule is still isolated operations whose result names the state, and a failed `cas` is never a cut.
- **Output.** The summary now prints `noReplyWrites` instead of `noReplyWritesDropped`, since no write is dropped. The segment line on stderr names the key and the result, for example `k2 segment 203 (...): Illegal`. `run.ts` reads only `result` and keeps the JSON, so it needs no change.

## Experiments on known answers

`/tmp/pseg/exp.py` (scratch, kept with its JSON results in `/tmp/pseg/`), run per key, checks each true history and each planted violation three ways: whole-history Porcupine, old `-seg`, and new `-seg`. Each plant is Illegal by construction:

- **stale.** A read returns the value of a write W. A later write W2 was called after W returned, and W2 returned before the read was called.
- **lost.** An acked `put` of a new value is placed after a read's writer returned and before that read was called.
- **cas.** A `cas` is set to fail, expecting the value of a `put` that returned before the `cas` was called, when no other write can take effect from that `put`'s call to the `cas`'s return.

Plants go at spread positions through the key, and within 3 operations of the first cuts at the smallest `-seg` tested ("near cuts").

| Input | Keys | True: whole / old seg / new seg | Planted stale / lost / cas (near a cut) | Caught by whole, old seg, new seg |
| --- | --- | --- | --- | --- |
| `lin-smoke1` (copy) | 6 | Ok 6 / Ok 6 / Ok 6 | 81 (51) / 77 (47) / 34 (4) | all 192, at `-seg` 2000 and 500 |
| `lin24b-smoke` (copy) | 6 | Ok 6 / Ok 6 / Ok 6 | 84 (54) / 80 (50) / 34 (4) | all 198, at `-seg` 2000 |
| `lin24`, first hour (109,886 ops) | 6 | Ok 6 / Ok 6 / Ok 6 | 84 (54) / 80 (50) / 36 (6) | all 200, at `-seg` 2000 and 500 |
| `lin24`, first 120,000 rows | 6 | Illegal 6 / Illegal 6 / Illegal 6 | not planted | |
| Sim HARSH, 300 of 6,000 keys | 300 | Ok 300 / **Ok 123, Illegal 177** / Ok 300 | 1,363 (102) / 1,386 (134) / 43 (0) | all 2,792, at `-seg` 1, 10, and 2000 |
| Sim GRAY, 300 of 6,000 keys | 300 | Ok 300 / **Ok 296, Illegal 4** / Ok 300 | 1,845 (345) / 2,013 (513) / 1,514 (15) | all 5,372, at `-seg` 1, 10, and 2000 |

The first 120,000 rows of lin24 are Illegal in all three modes. The cut ends history in mid-flight. A row is written only when its operation returns, so a read can return a value whose write was still in flight at the cut and has no row. This is the same lost-row effect lin24 has on its recorded clocks. The first full hour has no such effect and is Ok.

Whole files, new `-seg`:

- **Sim HARSH, 2000 seeds** (156,723 ops, 6,000 keys): **Ok** at `-seg` 2000, 10, and 1. The old code gave Illegal.
- **Sim GRAY, 2000 seeds**: **Ok** at `-seg` 2000. The old code gave Illegal.
- **Sim 20-seed sweep, HARSH and GRAY**: Ok at `-seg` 2000, 10, and 1. The old HARSH result was Illegal.

`TestSegmentedAgreesWithWholeHistory` in `main_test.go` generates 3,000 random histories:

- 1 to 3 keys, with `put`, `get`, `cas` (both outcomes), no-reply writes that did or did not take effect, and quiet gaps where cuts fall.
- Half of them have one output corrupted.
- At `-seg` 1 and `-seg` 4, segmented mode matched the whole check on all 6,000 checks: 4,646 Ok and 1,354 Illegal, with no Unknown.

Totals, new segmented mode:

- **True Ok.** 624 histories that the whole check finds Ok, from smoke, lin24, and sim keys, gave 0 false Illegal. The two full 2000-seed sim files are also Ok.
- **True Illegal.** 6 of 6 cut-down lin24 keys and 1,354 of 1,354 random Illegal histories were reported Illegal.
- **Planted cases.** 8,754 of 8,754 were caught, 1,342 of them within 3 operations of a cut. The old segmented mode also caught all 8,754, because one key per file and acked-only planted writes avoid both of its bugs.

## Regression tests (`main_test.go`)

The old code fails each of these:

- **`TestSegmentedKeepsNoReplyWriteThatWasSeen`.** It covers a write with no reply that a later read saw. The whole check is Ok, the old code gave Illegal, and the new code gives Ok.
- **`TestSegmentedLendsUnseenNoReplyWrite`.** A write with no reply that nobody saw is the only explanation for a failed `cas`. The old code gave Illegal, and the new code gives Ok.
- **`TestSegmentedCutsEachKeyOnItsOwn`.** It covers both cross-key cases above, the false Illegal and the false Ok.
- **`TestSegmentedAgreesWithWholeHistory`.** The old code fails on the second random history.

`TestSegmentedLendsNoReplyWriteOnce` passes on both versions. It checks that one write cannot explain two segments.

To show these fail on the old code, I put the old `segmented` body behind the new signature in a scratch copy (`/tmp/pseg/oldpkg`). Four tests failed there. `go test ./...` in `tests/m0/porcupine` passes with the fix.

## What changes for the gate runs

- **lin24, recorded clocks.** Each key is checked with no no-reply writes (0 rows with `ok: false`). The old and new checkers give the same results: k0, k1, and k3 Ok, and k2, k4, and k5 Illegal, in the same 5 segments, with the same segment counts (225, 224, 224, 224, 223, 224).
- **lin24, widened 100 ms.** `history-prep.py` adds 5 writes. Old and new both give Ok on all 6 keys, with the same segment counts (191, 189, 193, 191, 190, 188). As one 6-key file, the new checker gives Ok, 1,142 segments. That is the sum, which shows per-key partitioning works on the multi-key file.
- **lin24, widened by the measured 275 ms. Not rerun to completion.** One key widened by 275 ms needs more than 7 GB in either checker. This orb has 16 GB, shared with the `lin24b` services. Two attempts (`/tmp` is RAM-backed) led to kernel OOM kills of my own checker processes. Those kills hit no other process (dmesg). I then stopped. The answer does not depend on the checker version: the input has no no-reply writes and is one key per file, so old and new run the same code path. The lin24 report's 275 ms results (Ok on all 6 keys) stand. Running the 275 ms check again needs a machine with more memory.
- **lin24's verdict does not change.** BM-1 is still "Not passed" for the reasons in `build/BUILD-PLAN.md`. Recorded clocks give Illegal on k2, k4, and k5, from lost client rows. Both widenings give Ok.
- **B0-11 (`cf-48h`).** I reran it the way M0 ran it: per key, `history-prep.py`, then `-seg 20000`, from `tests/m0/results/raw/raft-chaos-cf-48h.jsonl`. The raw history has 4 no-reply writes, on k2, k3, and k4, and no read or `cas` ever saw any of them.
  - On all 6 keys, the old and new checkers give Illegal at 0 ms and Ok at 100 ms. This matches `raft-chaos-cf-48h.check.log`.
  - The Illegal segments are the same in both checkers: 9, 7, 7, 5, 6, and 7, with identical op ranges. Segment counts also match.
  - B0-11's verdict does not change.
- **Earlier sim sweeps.** They were checked whole (`m1-wp2.md`, `m1-wp3.md`, `m1-wp8.md`), so they are not affected.

## lin24b

`tests/m1/lin/run.ts` `finish` builds `/tmp/lin-m0check` from `tests/m0/porcupine` with `go build` if the file is missing. It runs the binary once per key file: `-seg 2000 -timeout 20m`. I deleted `/tmp/lin-m0check`, which an earlier sweep script had built from the old source at 00:13Z. The finish service's environment can build the package. A `go build` with a cleared environment (`HOME`, `USER`, system `PATH`) succeeded. `run.ts` reads only `.result` from the JSON summary, so it needs no change. I did not touch the `lin24b` services, the `graft-m1-lin-*` workers, or `tests/m1/lin/raw/lin24b.*`.

## A design sentence this contradicts

`tests/m0/results/m0-wp2.md`, B0-11, "The check": "No-reply writes are dropped, which can only make the check stricter." The second half is wrong. Dropping a write that someone saw makes a linearizable history look Illegal. B0-11's verdict stands anyway, because none of its 4 no-reply writes was seen. `build/BUILD-PLAN.md` and `decisions/` say nothing about how `-seg` cuts, so they need no change.
