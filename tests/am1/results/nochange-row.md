# nochange-row: B0-33 on sabA's commit path, tag ncA (graft-m2b)

LM-1, 2026-11-22. The lead's no-change control after `tests/am1/results/control-row.md` (crA): one m64-shaped B0-33
row on main `d6811ee8`, with `commit-hold` and `human-join` both reverted, so the member's commit path is sync-ab arm
A's (sabA, `c4d5a133`). It still carries `colo-gate` (`292c432e`), `teardown-stops-runs` and `chain-record`. Tags
follow `design/SHARED.md`, "Read this first".

**Short answer.** The row is valid and crA-like by the rule set before it ran, so the lead's stop applies and no
further row ran. Site 0's leader had 69 ticker stalls of 500 ms or more in its first 216 s, a confirm p90 of 826 ms
and edge 503s of 5,041, then transferred leadership to site 2 (StepDown). With sabA's commit path back, the slowdown
seen on chA and crA is not explained by `commit-hold` or `human-join` [measured from the diff; one row].

## Setup

- Site day from `d6811ee8`, siteday exit 0; edge `02c5c08c` at 100% (rev `d6811ee82`). `reretire.sh` printed RERETIRE
  OK graft-m2b (`tests/fanout/raw/reretire-graft-m2b-nc.log`).
- One B0-33 row, m64 shape, four learners (`enam:2000,weur:1000,apac:1000`), flag off: tag ncA, repository 131385, run
  `mwr5sti9`, load 01:32:10 to 01:37:10Z, done line 01:40:22Z. Harness outages 0%: valid.
- Site 0 led at term 2 and logged a `Transfer` row (cause 1, StepDown) at 01:35:39.5Z. Site 2 led at term 3 from
  01:35:46Z to the end.
- Run cost $1.68; graft-m2b $1.79 and the account $3.16 on 2026-11-22 (UTC, from 00:00Z).

## The rule, set before the run

Sound if Human p99 is within 10% of sabA's 3,589 (≤ 3,948), ticker stalls are 20 or fewer, and edge 503s are under
1,050. crA-like if stalls are 30 or more or confirm p90 is 500 ms or more: stop and bring the analysis.

## Verdict lines

[measured unless tagged]. Where the row splits by leadership, site 0 (216 s) is given first and site 2 (83 s) second.

| Line | ncA | crA | sabA |
|---|---|---|---|
| Human commit stage p99 ms (n) | **9,331** (115) | 3,968 (154) | 3,589 (126) |
| of which ended before / after the transfer | 12,891 (64) / 9,331 (51) | | |
| Merge commit stage p99 ms (n) | 8,451 (29) | 4,197 (39) | 2,728 (30) |
| Humans within 1 s | 5% | 9% | 32% |
| B0-29's sampler (bycut two rounds p99 + entries.py round p50; leaderships mixed) | 5,308 + 466 = 5,774 | 3,907 | 3,912 |
| `/stats` sampler (site 2's term only) | 2 × 2,007 + 585 = 4,599 | 2,892 | 2,631 |
| `/stats` commit p99; `queue:slot` p99 (site 2, the backlog it inherited) | 32,200; 30,717 | 8,425; 7,740 | 6,387; 5,798 |
| Human p50 / p99 (humansplit, 64 Humans in the window) | 1,967 / 14,043 | 1,509 / 4,887 | 865 / 4,036 |
| Human median split: freeze wait + held + own round | 596 + 690 + 574 | 601 + 336 + 560 | 304 + 191 + 384 |
| Wall time with a frozen batch waiting | 64% | 81% | 69% |
| Ticker stalls ≥ 500 ms | **69 (68.9 s) in 216 s**; 0 in 83 s | 36 | 10 |
| Leader confirm wall p50 / p90 / p99 ms | 100 / **826** / 2,000; 18 / 79 / 518 | 72 / 555 / 2,025 | 52 / 379 / 2,076 |
| Leader open requests at entry p50 / p99 / max (site 0) | 910 / 1,710 / 2,223 | 413 / 723 / 1,018 | 320 / 567 / 691 |
| Leader inputs/s | 306.1; 355.2 | 347.8 | 314.2 |
| `/outcome_by_request`/s | **44.3**; 1.4 | 22.8 | 12.6 |
| Edge 503s | **5,041** | 607 | 525 |
| Learner read wait p50 / p99 ms (until the transfer) | 903 / 3,513 | 475 / 1,550 | 365 / 1,568 |
| Voters' `/suspect` Reported rows at LAX (whole trace) | 5,426 | 56 | 35 |
| Agent pushes ok | 3,658 of 12,505 | 3,538 of 12,398 | 5,477 of 12,547 |
| `ERR_HTTP2_STREAM_ERROR`; other `fetch failed` | 0; 3 (`UND_ERR_INFO`) | 0; 2 | 0; 3 |
| Leader memory peak | 85.1 MiB (site 0); 71.8 (site 2) | 97.5 | 101.5 |

Notes:
- **Edge 503s** (`rows-ncA.status`): 2,671 "the member answered {"full":...}" (the leader's admission), 2,302 "every
  member asked shed the read: overloaded" (58 of them upload-pack), and 3 merge `warming`.
- **Ticker stalls** on site 0 per 30 s: 8, 1, 1, 16, 14, 22, 7; site 2 had none.
- **TORNALARM OK**: network 131385 was torn down at 01:38:29Z; after the end sweep its shards 0, 1 and 2 read `/status`
  running false, `alarm` null. The `warming` 503s came in the load window, before the teardown. No shard container ran
  after the sweep.

## What changed since sabA

`git diff c4d5a133 d6811ee8` [measured]:
- `crates/raft`: unchanged.
- `crates/lookup`: tests and `COMMIT_RECEIPT`'s derivation only; it is 2,000 ms as in sabA.
- `workers/site`: `teardown-stops-runs` (shard) and `RECEIPT_DELAY_MS`'s derivation (2.6 s as before).
- `workers/edge`, `crates/git`: `colo-gate` (`colo.rs`, the shared read gate's Allow decisions; header sets in
  `compose` and `host`) and `chain-record`.

## Reading

[inference; one row each, run-to-run variance unmeasured]
- `commit-hold` and `human-join` do not explain ncA, which ran neither. crA, which ran `human-join`, was better than
  ncA, so the order of the three rows does not follow the commit-path code.
- The candidates left are variance (placement, the colo's load, the isolates sharing LAX) and the edge's `colo-gate`,
  which could change what the edge sends to members. m64r33, before `colo-gate`, had 24 stalls in 144 s of site 0's
  term against sabA's 10 in 300 s, so pre-`colo-gate` builds already varied about twofold.
- ncA's 5,426 voter `/suspect` reports at LAX, against 35 to 56 on sabA and crA, show the edge suspected members far
  more on this run; the cause is not traced [unverified].
- The defensible reading: B0-33's m64 shape on graft-m2b sits near a cliff, and sabA's 10 stalls may be the good tail.
  Settling it takes a row on `c4d5a133` itself, or `colo-gate` off through its test-faults share-off arm, or rows sized
  for variance (FRAME's three-run rule). That is the lead's call; nothing runs meanwhile.

## Slower host or more work? ($0, the lead's four questions)

From the five rows' traces (`tests/fanout/hostwork.py`, new) and Cloudflare's Durable Object analytics for each
leader object `repo:N#1#f0` (`durableObjectsInvocationsAdaptiveGroups` and `durableObjectsPeriodicGroups`, by
`coloCode`, read-only; `tests/fanout/docpumin.py` for CPU a minute). Windows: each row's load on site 0's leadership
(m64r33 00:11:46 to 00:14:10, ncA 01:32:10 to 01:35:46) [measured unless tagged].

Entry bytes corrected 2026-11-24 (`tests/am1/results/trace-dedup.md`; they read 3,070, 3,269, 908, 1,031 and 1,492 before the trace merge kept equal rows inside one save). The reading below holds.

| Row (build) | Leader colo | Inputs/s | Stalls ≥ 500 ms a minute (stalled s a minute) | Confirm p90 | Turn work per input: entry bytes, statements | Leader CPU-s a loaded minute | CPU per request p50 / p90 µs |
|---|---|---|---|---|---|---|---|
| sabA (`c4d5a133`) | **EWR** | 314.2 | **2.0** (1.8) | 379 | 3,420, 1.26 | 33.5 to 40.3 | **298 to 411** / 1,075 to 2,265 |
| m64r33 (`a1e431a6`, before `colo-gate`) | **IAD** | 224.7 | 10.0 (10.7) | 318 | 3,506, 0.80 | 22.2 to 24.8 | 483 to 486 / 1,599 to 2,080 |
| chA (`68f1ba32`) | **IAD** | 314.0 | 16.2 (15.6) | 648 | 1,129, 0.93 | 35.0 to 40.7 | 392 to 516 / 1,260 to 1,600 |
| crA (`3acfd02a`) | **ORD** | 347.0 | 7.2 (5.9) | 555 | 1,272, 1.03 | 32.2 to 41.3 | 326 to 379 / 994 to 1,695 |
| ncA (`d6811ee8`) | **ATL** | 306.1 | **19.2** (19.1) | 826 | 1,731, 0.68 | 36.1 to 38.5 | **520 to 589** / 1,709 to 2,292 |

1. **Inputs against stalls.** ncA's leader took 306 inputs a second against sabA's 314, with less turn work per input
   (entry bytes encoded 1,731 against 3,420, statements 0.68 against 1.26), and the same CPU-seconds a minute (36 to
   39 against 34 to 40). Yet it stalled 19.2 times a minute against 2.0. chA had sabA's input rate exactly (314.0) and
   stalled 16.2 a minute. The same or less work ran slower.
2. **The leader's colo.** The leader object `#1#f0` (site a, enam) ran in EWR (sabA), IAD (m64r33, chA), ORD (crA) and
   ATL (ncA). sabA, the one good row, is the only EWR placement. CPU per request at p50 is lowest on sabA and crA (298
   to 411 µs) and highest on ncA (520 to 589), but the rank does not hold all the way: m64r33 (485 µs, IAD) stalled less
   than chA (392 to 516 µs, IAD). The leader's total CPU a loaded minute is the same on every full row (32 to 41
   CPU-seconds, 0.55 to 0.69 vCPU), so the stalls are not extra CPU: they are time the isolate had work and did not run
   it, which points at the host the object shares (scheduling or contention) [inference].
3. **What the edge sends members.** Edge (peer 255) requests a second, by route:

   | Row | Edge → leader | of which `/commit`, `/outcome`, `/outcome_by_request`, `/read` | Edge `/read` to all members | Edge `/suspect` |
   |---|---|---|---|---|
   | sabA | 256.2 | 146.0, 75.7, 12.6, 3.7 | 199.9 | 0 |
   | m64r33 | 185.0 | 88.7, 47.2, 35.2, 6.8 | 184.5 | 5.6 |
   | chA | 264.6 | 131.7, 78.5, 37.6, 6.7 | 254.7 | 0 |
   | crA | 291.2 | 149.8, 98.6, 22.8, 4.7 | 214.8 | 0 |
   | ncA | 263.5 | 130.0, 75.7, 44.3, 5.5 | 258.4 | 25.2 |

   `colo-gate` changes the edge's read gate. The edge's reads to the leader stayed at 3.7 to 6.8 a second on every row,
   before and after it. The edge's `/read`s to all members rose about 25 to 30% on chA and ncA, almost all to the
   learners, along with `/outcome_by_request`. That is the shed-retry pattern lcA2 showed after a slow leader
   (OPEN-QUESTIONS A39), and crA, on the same edge build, rose only 7%. Nothing here shows `colo-gate` adding leader
   work [inference].
4. **ncA's `/suspect` reports.** Of the voters' Reported rows in site 0's term, all but 33 are about member 0, the
   leader. They follow the stalls; they do not precede them. Per 30 s, stalls then reports: 8 then 6, 1 then 4, 1 then 0,
   16 then 58, 14 then 1,230, 22 then 744, 7 then 579. After site 2 took over, 23 were about site 2 and 11 about site 0.
   chA has the same shape (reports about site 0 rising after its stall bursts), and crA had 54 in all. The reports are
   the edge's lookups timing out on a stalled leader, the effect, not the cause.

**Verdict: variance, most likely the leader's host; `colo-gate` is not supported** [inference; five rows, four
colos]. The work per leader is flat or lower across the rows, the leader's total CPU a minute is the same, the edge's
traffic to the leader did not change with `colo-gate`, and the `/suspect` storm follows the stalls. What differs is
where the leader object ran and how often its isolate went unscheduled. A Durable Object's colo is chosen by the
platform at creation; the harness cannot pin it. Not excluded: host speed is inferred from stalls with equal CPU, not
measured as a machine, and IAD alone gave 10.0 and 16.2 stalls a minute, so placement is not the whole spread.

## Raws

- `tests/fanout/raw/`: `costrow-ncA.status`, `rows-ncA.status`, `terms-ncA.jsonl`, `tsave-ncA.jsonl.xz`,
  `memguard-ncA.{err,jsonl.xz}`, `arrivals-ncA.jsonl`, `shardgate-ncA-b0-33-{0,1,2}.jsonl`, `siteday-nc.status`,
  `reretire-graft-m2b-nc.log`.
- `tests/am1/raw/a1-b0-33-mwr5sti9.jsonl`; `tests/am1/raw/runs/a1-b0-33-fncA-b033.log`, `fanout-nc-{edge,siteday}.log`.
  The push log stays local.
- `tests/fanout/hostwork.py` (new): per leadership, inputs by sender and route, edge requests to every member, stalls,
  confirm, turn work per input, and voters' `/suspect` reports by member per 30 s.
- Tools, unchanged, as in `combined-row.md`; `syncab.py` run per leadership (site 0 01:32:10 to 01:35:46Z, site 2
  01:35:47 to 01:37:10Z).
