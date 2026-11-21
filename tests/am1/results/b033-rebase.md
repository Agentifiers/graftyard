# b033-rebase: FRAME's B0-33 line, outcome asks against COMMIT_HOLD, the leg split, and the Human split ($0, 2026-11-21)

LM-1, analysis only: nothing built, no hosted row, FRAME not edited. It answers the lead's four items after the
second opinion on B0-33 (thread T-01a1f697). The runs, all B0-33 on `graft-m2b` (load windows on Cloudflare's clock):

| Tag | a1 run | Repo | Build | Load from | Shape |
|---|---|---|---|---|---|
| m64r33 | `mwpnhiva` | 131378 | a1e431a65 (step-busy, command-rows) | 00:11:46Z | four learners |
| sabA | `mwqbz2c4` | 131379 | main `c4d5a133` | 11:37:06Z | four learners, flag off |
| sabC | `mwqcdvtq` | 131380 | same | 11:48:41Z | no learners, flag off |
| sabB2 | `mwqd4xt9` | 131381 | same | 12:09:54Z | no learners, lean flag on |
| lcA2 | `mwqna5p0` | 131382 | main `c6e123eb` (learner-cadence, reversed) | 16:53:52Z | four learners, flag off |

## 1. FRAME's B0-33 re-base

### (a) The sampler and window behind B0-29's 770 ms

FRAME, "Operator decision, 2026-11-19, B0-29's commit line": "two rounds at p99 plus one median round for the freeze
wait, 770 ms rounded up". The commit that recorded it (`88ae0051`) and `tests/m0/results/b029-tail.md` give the parts:

- **603 ms, "two rounds at p99"**: `tests/m0/cuttrace/bycut.py`'s per-command line "two rounds (ahead + own) p99" on
  A-B-A row 1 (`tsave-m2bb29j`, 00:26:30 to 00:31:30Z, 2026-11-19), the largest of run 7 / row 1 / control (587 / 603 /
  570). It is per `/commit`: for each command, the round of the entry in flight when its batch froze plus its own
  entry's round, each the leader's first append (kind 15) to its apply (kind 4), one clock, then p99 over commands.
  Rerun here: `bycut.py tsave-m2bb29j.jsonl.xz 2026-11-19T00:26:30Z 2026-11-19T00:31:30Z` prints "two rounds (ahead +
  own): p50 319 p99 603" [measured].
- **167 ms, "one median round"**: `tests/m0/cuttrace/entries.py`'s entry round p50 on run 7 (`tsave-m2bb29h`, 600 s
  from 18:24:33Z, 2026-11-18). Rerun: "round: n 2745 p10 124 p50 167 p90 219 p99 316" [measured].
- `entries.py` fixes its window at 600 s (B0-29's row). The B0-33 figures below use the same code over each
  run's 300 s load (m64r33: site 0's 144 s term).
- So B0-29's line is **trace samplers** (append → apply on the leader trace), not `/stats`. Its "two rounds at p99" is
  the p99 of each command's pair sum, not 2 × the round p99. On B0-29 the two readings were close (2 × 316 = 632 against
  603) because rounds were tight (p99 / p50 = 1.9). On B0-33 they are not (p99 / p50 = 4 to 8), so the choice decides
  the number.
- `/stats` rounds on the same runs were p50 160 to 166 / p99 305 to 325, so on B0-29 any sampler gave 770 to 820.

### (b) The first valid B0-33 run under FRAME's wording

FRAME, "Operator decision, 2026-11-20, leader-side render, three gate lines, and load clients on Cloudflare": "measured
on the first valid B0-33 run after the whole-tier read leaves REST writes and the self-inflicted step-downs are fixed".

- Whole-tier read off REST writes: `command-rows` (merged `a7ae728b`, 2026-11-20 18:24Z). m63r33 was its first row.
- Self-inflicted step-downs: `step-busy` (merged `b5d9c23e`, 2026-11-20 23:03Z).
- **m64r33** (`mwpnhiva`) is the first run on a build with both (site day from `a1e431a6`, a descendant of both). Its
  harness outages were 0.7%, so its results file calls it valid, but it had **one leader change** (site 0 term 2 → site
  1 term 3 at 00:14:13Z, a transfer whose start logged no line) and its file says "This is not the re-base run".
- **sabA** (`mwqbz2c4`) is the first run after both with **0 leader changes**, the same shape (m64's four learners), and
  flag off. Its build adds only the test-faults lean switch, which is off and byte-identical in the product build.
- Which one FRAME means turns on whether "valid" includes "no leader change". The m64 file read it that way, so sabA is
  my answer [inference]; both are computed below.
- sabC and sabB2 are not B0-33's shape (no learners; B adds the lean hold); lcA2 ran the reversed cadence change.

### (c) The line, by sampler

| Run | B0-29's sampler: bycut two rounds p99 + entries.py round p50 | Trace 2 × round p99 + p50 (entries.py) | `/stats` 2 × p99 + p50 |
|---|---|---|---|
| **sabA** | 3,620 + 292 = **3,912** | 2 × 2,314 + 292 = 4,920 | 2 × 1,136 + 359 = 2,631 |
| m64r33 (site 0's term only, 00:11:46 to 00:14:10) | 3,731 + 229 = 3,960 | 2 × 2,331 + 229 = 4,891 | 2 × 1,944 + 274 = 4,162 (site 1's `/stats`) |
| sabC | 2,343 + 456 = 2,799 | 2 × 1,738 + 456 = 3,932 | 2 × 943 + 491 = 2,377 |
| sabB2 | 2,082 + 479 = 2,561 | 2 × 1,366 + 479 = 3,211 | 2 × 1,067 + 491 = 2,625 |
| lcA2 | 7,313 + 365 = 7,678 | 2 × 3,863 + 365 = 8,091 | 2 × 3,820 + 498 = 8,138 |

- **On sabA, B0-29's sampler gives 3,912 ms** [measured]. The `/stats` reading gives 2,631 and the trace 2 × p99 + p50
  gives 4,920. The second opinion's 4,509 used probe20's entry rounds over 30 to 270 s (p99 2,108); `entries.py` over
  the whole 300 s gives p99 2,314. The window moves the 2 × p99 form by 400 ms and the pair-sum form by much less.
- `/stats` round p99 sits at p96.2 of sabA's trace entry rounds (p93.1 on C, p96.3 on B2), so the `/stats` line is
  really "two p96 rounds plus a median".
- The pair-sum p99 is below 2 × round p99 on every arm here because slow rounds rarely come in pairs (lag-1
  autocorrelation −0.01 to 0.31, the second opinion's figure).

### (d) The four runs against it

Measured Human commit-stage p99 (push log `server-timing` `commit;dur`) and merge commit-stage p99:

| Run | Human p99 | Merge p99 | Against 3,912 (B0-29's sampler, sabA) | Against 2,631 (`/stats`) |
|---|---|---|---|---|
| sabA | 3,589 (n 136) | 2,728 (n 30) | pass (92%) | fail |
| sabC | 2,031 (n 132) | 2,358 (n 24) | pass | pass |
| sabB2 | 2,290 (n 100) | 2,129 (n 43) | pass | pass |
| lcA2 | 29,303 (n 120) | 11,197 (n 38) | fail | fail |

- With the 90% rule (AGENTS.md, "How to pick work", item 5) the `/stats` line's 90% is 2,923, which sabA's 3,589 still
  fails.
- A line set on sabA and judged on sabA passes by construction under either trace sampler. The informative judgments
  are the other arms. Both B0-33-shaped runs on main (sabA and m64r33) give 3.9 to 4.0 s under B0-29's sampler.

## 2. Outcome asks against COMMIT_HOLD

How it works today: the leader holds each `/commit` up to `COMMIT_HOLD` (1 s, `crates/raft/src/lib.rs`) and then
answers `admitted` with its ticket. The edge then asks `/outcome` with `wait_ms` 10,000 (`crates/lookup/src/lib.rs`,
`outcome_through`). Separately, while a send is open with no reply, the edge asks `/outcome_by_request` every
`COMMIT_RECEIPT` (2 s) up to `RECEIPT_OPEN` (8 s). So an `/outcome` ask is one per `/commit` that outlives the hold, and
an obr ask is one per 2 s a send stays unanswered.

### The commit wait against 1 s [measured]

| | sabA | lcA2 |
|---|---|---|
| Leader `/commit` inputs/s | 146 | 145 |
| `/outcome` per `/commit` | 0.518 | 0.571 |
| `/outcome_by_request` per `/commit` | 0.086 | 0.372 |
| Leader inputs/s, all routes | 314 | 315 |
| Trace wait (arrival → its entry applied, bycut), p50 / p90 / p99 ms | 577 / 1,265 / 3,300 | 762 / 3,914 / 7,390 |
| of which over 1 s | 15.2% | 41.9% |
| Push commit stage (edge, agents + humans), p50 / p90 / p99 ms | 1,025 / 3,546 / 4,896 | 700 / 3,913 / 11,250 |
| of which over 1 / 1.5 / 2 / 3 s | 51 / 37 / 33 / 19% | 43 / 33 / 28 / 21% |

- The trace's wait ends when the entry applies on the leader. Only 15% of sabA's waits pass 1 s there, yet 52% of its
  `/commit`s were answered `admitted`. The resolution the held `/commit` waits for comes later than `Applied`. Fitting
  a constant shift so the trace reproduces the measured ask rate gives 442 ms (sabA) and 407 ms (lcA2), consistent
  across runs [inference]. Before the edge sees it, the answer also waits on the leader's input queue (round-remainder,
  site 1's input depth).
- The second opinion's backlog split reproduces (`b033probes/probe22.py`): sabA's /outcome asks per /commit are 0.14
  in its lowest-backlog third, 0.74 and 0.84 in the others. lcA2 is 0.84 to 1.06 in every third, with obr 30 to 79/s.

### Predicted at holds of 1.5, 2 and 3 s

`tests/fanout/holdmodel.py`, from each /commit's trace wait plus the fitted shift, with all other inputs held equal
[inference: the model moves only the /outcome asks; it holds the obr asks, the round and the backlog fixed]:

| Hold | sabA: /outcome per /commit | sabA: leader inputs/s | sabA: held /commit at the leader, mean / peak | lcA2: /outcome per /commit | lcA2: leader inputs/s | lcA2: held mean / peak |
|---|---|---|---|---|---|---|
| 1.0 s (today) | 0.52 (fit) | 314 | 128 / 455 | 0.57 (fit) | 315 | 122 / 837 |
| 1.5 s | 0.14 | 259 (−18%) | 149 / 535 | 0.40 | 290 (−8%) | 157 / 837 |
| 2.0 s | 0.07 | 249 (−21%) | 157 / 589 | 0.31 | 278 (−12%) | 183 / 972 |
| 3.0 s | 0.03 | 243 (−23%) | 164 / 589 | 0.23 | 265 (−16%) | 223 / 977 |

- A held `/commit` is one open edge→leader request, the same as the `/outcome` long-poll that replaces it past the hold.
  So the leader's open requests barely change: in sabA, held `/commit` plus held `/outcome` stays about 166 (128 + 38
  today, 164 + 3 at 3 s). The hold moves which call is open, and removes the extra input that the switch costs.
- obr asks are separate (12.6/s in sabA, 54/s in lcA2). The hold does not touch them. They come from sends that stay
  unanswered past 2 s, and a longer hold makes more sends cross 2 s. At a 2 s or 3 s hold, more held `/commit`s would
  pass `COMMIT_RECEIPT` and draw obr asks unless that deadline moves with the hold. `COMMIT_RECEIPT` is documented as
  "COMMIT_HOLD plus CALL_DEADLINE", so it would move to 2.5 / 3 / 4 s [inference from the code].

### Against A38's 6 s failures

- A38: 203 `ERR_HTTP2_STREAM_ERROR` pushes in sabB2, held p50 5.9 s (p10 4.1, p90 8.0), 111 in one second. That is
  the lean hold of up to 6 s on the client's shared HTTP/2 connections [inference, A38].
- The client side is the relevant count: an agent push's HTTP stream stays open for its whole edge time, whatever the
  edge does with the leader. Client pushes open at once peaked at 550 (sabA), 622 (sabC), 732 (sabB2) and 451 (lcA2),
  and push wall p50 was 4.2 to 4.9 s in all three sab arms. sabA and sabC had streams as long and as many as sabB2's,
  with 3 and 1 `fetch failed`. So **the open-stream count alone did not reset the client's connections** [measured,
  push logs]. What differs in B2 is the edge→leader hold of up to 6 s, with the edge's own request to the leader open
  that long.
- So A38's risk sits on the edge→leader leg, measured by commands held at the leader past about 4 s: none at holds up to
  3 s, since the hold caps them, against 544 (sabA) and 913 (lcA2) push commit stages of 4 s or more under a 6 s cap
  [measured stages; the cap is the model].

### Which hold

- **1.5 s** cuts sabA's /outcome asks from 0.52 to 0.14 per /commit and the leader's inputs by about 18%, the shape
  the second opinion found at no backlog. No command is held at the leader past 1.5 s, a quarter of A38's 6 s.
  `COMMIT_RECEIPT` moves to 2.5 s with it. On lcA2's shape (deep backlog) it cuts only 8%.
- **2 s** adds 3 points (21%). Past 2 s the gain is small (3 s: 23%) and every held call is longer.
- Recommendation [inference, one row each]: **1.5 s**, if any hold. It is not a lever on confirm p90 by itself. On
  sync-ab, removing all /outcome inputs (C → B) did not move the round p50 (472 → 480). B's confirm p90 fell from 136 to
  101 ms with the obr cut and the lean hold together. A 17 to 21% input cut is about a third of arm C's learner cut.
- **The risk is not measured.** No row has run a 1.5 or 2 s hold. A38's own test is the same flip line: count `fetch
  failed` against arm B's 203.

## 3. legsplit on sabC and sabB2 (Workers Logs retained)

Workers Logs still held both arms (and sabA) at 18:03Z. `tests/fanout/applegs.py fetch` pulled each follower's `/raft`
invocations from leader member 0 for 30 to 270 s of load: 1,751 to 2,058 per follower. Extracts are in
`tests/fanout/raw/applegs-sab/`, with no account fields. Then `applegs.py join` and `tests/fanout/legsplit.py` (CPU
column empty: `docpusec.py` was not run) [measured]:

| Arm, leader → follower | Calls | rtt p50 | Outside p50 / p90 | Follower qin p50 / p90 | Follower handler p50 / p90 | Leader clock stale p50 |
|---|---|---|---|---|---|---|
| A, 0 → 1 | 565 | 293 | **254** / 790 | 15 / 66 | 8 / 34 | 1,187 |
| A, 0 → 2 | 610 | 411 | **316** / 857 | 11 / 66 | 53 / 168 | 1,233 |
| C, 0 → 1 | 460 | 469 | **371** / 774 | 33 / 181 | 12 / 99 | 969 |
| C, 0 → 2 | 487 | 481 | **353** / 695 | 34 / 160 | 27 / 177 | 968 |
| B2, 0 → 1 | 460 | 461 | **383** / 767 | 23 / 126 | 1 / 69 | 745 |
| B2, 0 → 2 | 500 | 519 | **408** / 778 | 23 / 136 | 27 / 167 | 777 |
| m64 site 0 → 1 / 0 → 2 (round-remainder) | 265 / 270 | 233 / 329 | 198 / 261 | 4 / 3 | 17 / 27 | 626 to 634 |
| idle (m64) | 181 | 94 | 94 / 106 | 0 | 0 | 0 |

- **The follower's input wait is small in every arm** [measured]. qin (Cloudflare's invocation start to the handler's
  `Entered`) is p50 11 to 34 ms. C and B2 sit 10 to 20 ms above A, with p90 126 to 181 against 66. The follower
  handler is p50 1 to 53 ms. Together these are under 70 ms of a 460 to 520 ms rtt.
- **Outside the follower** (wire both ways, the leader's late send, its late read of the answer) is 254 to 408 ms at
  p50, against the idle 94. That excess, 160 to 314 ms, is the largest part of the leg in every arm.
- C and B2's outside is 60 to 130 ms above A's, though their leaders took fewer inputs (244 and 167/s against 314).
  Their AppendEntries bodies are larger (p50 145 to 173 KB against 59 KB in A), but in each arm outside is flat across
  body sizes (C's over-150 KB band p50 330 to 348 against 379 to 392 for 1 to 40 KB). So it is not bytes.
- By staleness: calls sent while the leader's clock was 1 s or more behind took 418 to 496 ms outside, against 313 to
  400 ms under 1 s, in every arm. A's leader was the stalest (p50 1.2 s, 10 stalls ≥ 500 ms), yet it had the shortest
  outside.
- **This sizes the second opinion's lever 1** (the followers' input queue): qin plus handler is p50 23 to 64 ms a leg
  in every arm, and C and B2 add at most 22 ms over A, not 100 to 190 ms. The C/B2 round excess over A is outside the follower's invocation. It is wire
  or leader-side, not the follower's queue. That supports correction 1 in `learner-cadence.md` and goes further: in
  C and B2 the follower read load shows up mainly in rtt outside the follower, which this join cannot attribute
  further.
- What remains [unverified]: whether Cloudflare's invocation start includes time at the object's input gate
  (applegs.py's docstring). If it does not, a follower's gate wait would hide in "outside".

## 4. The second opinion's probes, and the Human split

The probes are copied to `tests/fanout/b033probes/` (from the second opinion's `.amp/in/artifacts/b033-probes/`,
unchanged except `humansplit.py`). `humansplit.py` takes `--pushes PUSHES`: each Human's admission is its push's `end`
less its `commit` server-timing stage, on Cloudflare's clock, so the arrival is real rather than estimated from the
trace.

`humansplit.py TSAVE START 0 300 --pushes tests/am1/raw/a1-pushes-b0-33-RUN.jsonl` [measured, with the split
modelled]:

| Run | Humans | p50 / p90 / p99 ms | Within 1 s | Riding a 64-agent entry | Wall with a frozen batch waiting | Entry rounds p50 / p90 / p99 |
|---|---|---|---|---|---|---|
| sabA | 117 | 865 / 2,742 / 4,036 | 59% | 62% | 69% | 286 / 787 / 2,314 |
| sabC | 118 | 971 / 1,662 / 2,716 | 58% | 69% | 37% | 470 / 863 / 1,813 |
| sabB2 | 83 | 905 / 1,593 / 2,188 | 57% | 80% | 36% | 482 / 852 / 1,507 |
| lcA2 | 104 | 1,589 / 4,521 / 9,341 | 31% | 81% | 98% | 353 / 1,398 / 3,728 |

- On the three sab arms the modelled p99s sit −4 to +34% from the measured commit-stage p99s (3,589 / 2,031 /
  2,290); lcA2's is 29,303. lcA2's
  measured p99 is the end-of-load stall below, which the 0 to 300 s window cuts off.
- sabA's top 3% are freeze wait 1,525 + held 965 + own round 1,372. On sabC and sabB2 the own round is most of the
  tail. That matches "freeze wait plus the entry in flight plus its own round" in the second opinion.
- What-ifs (the probe's own): no rows reach a Human p99 of 1 s without every part at ×0.33 or less. A late join helps
  sabA most (p99 3,563 against 4,036).

### lcA2's two checks

- **Humans were cut in the Human lane** [measured, `tests/m0/cuttrace/cuts.py` over 300 s]: 999 Human commands taken,
  0 left after any cut (p99 0, max 0). The 29 s Humans waited on entries, not in the queue behind agents.
- **The largest cut-to-cut gaps against the terms file** (load-window cuts on site 0, from 15 s in):

  | Gap | From | Leader `Lag` ≥ 500 ms inside | terms-lcA2 polls near it |
  |---|---|---|---|
  | 8,479 ms | 16:58:46.061Z | 608 (16:58:46.0), 1,751 (16:58:47.9) | none from 16:58:25.8 to 16:59:05.8 |
  | 5,431 | 16:57:57.758 | 1,816 (16:57:57.4) | 16:58:08.4 site 0 '?', 16:58:09.4 site 0 leader term 2 |
  | 3,935 | 16:54:48.465 | 1,368, 500, 1,110 | none |
  | 3,849 | 16:55:42.236 | 1,174, 1,349, 1,924 | 16:55:45.8 leader term 2; 16:55:51.8 site 0 '?' |

  - The three '?' polls (16:55:51.8, 16:58:08.4, 16:58:13.5) are site 0 not answering `terms.mjs` within the poll
    while both followers still read term 2 with no change. They are a stalled leader, not a leaderless group. The
    terms file has nine polls inside the load window, with gaps up to 133 s, so it cannot place the gaps by itself.
  - The largest gap (8.5 s from 16:58:46) is the one under the 29 s Humans. Site 0 cut its last batch at 16:58:54.8,
    memguard stopped the row at 16:58:52.1 with site 0 at 124.5 MiB (limit 120), the terms file shows site 0 as a
    term-3 follower by 16:59:05.8, and site 1 cut its first batch at 16:59:04.4 and led term 3 from 16:59:12.
  - The Humans admitted 16:58:45.5 to 16:58:49.5 were answered 16:59:13 to 19, on site 1's term 3.
  - So lcA2's Human p99 is the end-of-load stall on site 0, then a leader change, not round stretch [measured
    timestamps; that the memory stop and the stall are one event is an inference]. The second opinion's point stands:
    it matches `queue:slot` p99 (28.6 s) and is not built from rounds.

## Answers in short

1. B0-29's 770 = bycut's per-command two-round p99 (603, row 1) + entries.py's round p50 (167, run 7), both from the
   leader trace. On the first valid run (sabA; m64r33 if a leader change is allowed) that sampler gives **3,912 ms**
   (m64r33 3,960). `/stats` gives 2,631 and the trace 2 × p99 + p50 gives 4,920. sabA, sabC and sabB2 pass 3,912; lcA2
   fails. Against 2,631 only sabC and sabB2 pass.
2. 52 to 57% of /commits outlive today's 1 s hold at the leader's answer (15 to 42% by the trace's apply, plus about
   0.4 s to the resolution). A 1.5 s hold predicts 0.14 /outcome asks per /commit and −18% leader inputs on sabA's
   shape (−8% on lcA2's), with no command held at the leader past 1.5 s. Recommended if any: 1.5 s, with
   `COMMIT_RECEIPT` at 2.5 s, unmeasured.
3. legsplit ran (logs retained): the follower's own wait is p50 11 to 34 ms and its handler 1 to 53. Outside the
   follower is 254 to 408 ms, against 94 idle. The followers' queue and handler are p50 23 to 64 ms a leg; C and B2 add at most 22 ms over A.
4. Probes copied and humansplit rerun with real arrivals. lcA2's Humans were cut in their lane, and its 29 s p99 is the
   end-of-load stall and site 1's term 3, not rounds.

## Files

- `tests/fanout/b033probes/`: the second opinion's probes; `humansplit.py` with `--pushes`.
- `tests/fanout/holdmodel.py`: item 2's hold model.
- `tests/fanout/raw/applegs-sab/`: Workers Logs extracts, `{A,C,B2}-{b,c}-from-0.json`.
- Local, not committed, as for earlier rows: `tests/am1/raw/a1-pushes-b0-33-{mwpnhiva,mwqbz2c4,mwqcdvtq,mwqd4xt9,mwqna5p0}.jsonl`.
