# combined-row: B0-33 with commit-hold and human-join, tag chA (graft-m2b)

LM-1, 2026-11-21. One m64-shaped B0-33 row on main `68f1ba32`, which carries the learner-cadence revert,
`teardown-stops-runs`, `commit-hold` (COMMIT_HOLD 1.5 s, COMMIT_RECEIPT 2.5 s) and `human-join`. Run under the standing
queue at the lead's go. Tags follow `design/SHARED.md`, "Read this first".

**Short answer.** The row is valid and fails. Every line is worse than sync-ab arm A (sabA, same shape on `c4d5a133`),
and both flips fired: `commit-hold`'s (asks per `/commit` 0.596, leader inputs 314.0/s) and `human-join`'s (Human p99
and the frozen-batch wait both rose). The lead's ruling: revert `commit-hold`; keep `human-join` for one control row
that splits the two (`decisions/DECISIONS.md`, `commit-hold`, Status).

## Setup

- Site day from `68f1ba32`, siteday exit 0; edge `be0ff841` at 100% (rev `68f1ba32e`). `reretire.sh` printed RERETIRE
  OK graft-m2b (log `tests/fanout/raw/reretire-graft-m2b-ch.log`).
- One B0-33 row, m64 shape, four learners (`enam:2000,weur:1000,apac:1000`), flag off: tag chA, repository 131383, run
  `mwqyi49p`, load 22:07:57 to 22:12:57Z, done line 22:15:31Z. Site 0 led at term 2 throughout, 0 leader changes.
- Harness outages 1.0 s (0.35%): valid. Run cost $1.73; graft-m2b $10.18 that UTC day (it was $8.44 before the site
  day); account $47.18.

## Verdict lines

Against sabA [measured]:

| Line | chA | sabA | Gate or flip line |
|---|---|---|---|
| Human commit stage p99 ms (push log, n) | **5,780** (114) | 3,589 (126) | 3,912 (B0-29's sampler; 4,347 at 90%) or 2,631 (`/stats`): **fail** both |
| Merge commit stage p99 ms (n) | **5,848** (48) | 2,728 (30) | same: **fail** |
| Humans within 1 s | 7% | 32% | |
| B0-29's sampler on this run: bycut two rounds p99 + entries.py round p50 | 3,628 + 410 = 4,038 | 3,620 + 292 = 3,912 | |
| `/stats` sampler on this run: 2 × round p99 + p50 | 2 × 1,542 + 504 = 3,588 | 2 × 1,136 + 359 = 2,631 | |
| All-command `/stats` commit p50 / p99 ms | 4,400 / 9,118 | 2,229 / 6,387 | |
| Human p50 / p90 / p99 ms (humansplit, real arrivals) | 1,806 / 2,956 / 3,322 (107) | 865 / 2,742 / 4,036 (117) | |
| Human median split: freeze wait + held + own round | 454 + 783 + 535 | 304 + 191 + 384 | |
| Wall time with a frozen batch waiting | **95%** | 69% | `human-join` flip: **fired** |
| Humans joined a frozen batch | about 21 to 23% [inference] | (1.5%, the method's floor) | |
| `/outcome` per `/commit` | **0.596** (78.5 / 131.7) | 0.518 | ≤ 0.33: **fired** |
| Leader inputs/s | **314.0** | 314.2 | ≤ 286 (−9%): **fired** |
| of which `/commit`, `/outcome`, `/raft` (learners), `/outcome_by_request`, `/read` | 131.7, 78.5, 49.4 (45.6), **37.6**, 6.7 | 146.0, 75.7, 58.0 (55.8), 12.6, 3.7 | |
| Leader confirm wall p50 / p90 / p99 ms | 91 / **648** / 1,977 | 52 / 379 / 2,076 | |
| One-statement confirm p50 under load | 117 | 56 | |
| Round p50 / p99 ms (`/stats`); entries.py | 504 / 1,542; 410 / 2,338 | 359 / 1,136; 292 / 2,314 | |
| Full cycle p50 (`leadcycle.py`) | 704 | 560 | |
| Voter legs p50, outside the handler (site 1; site 2) | 372, 354; 505, 436 | 289, 258; 414, 323 | |
| Ticker stalls ≥ 500 ms | **81** (77.8 s) | 10 (9.1 s) | |
| Held-input batches (requests held) | 552 (77,092 of 94,199) | 464 (61,315 of 94,258) | |
| Leader open requests at entry p50 / p99 / max | 423 / 890 / 1,099 | 320 / 567 / 691 | |
| `queue:slot` p99 ms | **8,332** | 5,798 | |
| Agent commands/s cut | **81.5** | 103.2 | |
| Agent pushes ok | 1,872 of 12,493 | 5,477 of 12,547 | |
| Edge 503s | **3,842** | 525 | under 1,050: **fail** |
| Learner read wait p50 / p90 / p99 ms (queue + call) | 765 / 1,399 / 1,996 | 365 / 1,000 / 1,568 | |
| of which the read-index call p50 | 629 | 321 | |
| `ERR_HTTP2_STREAM_ERROR` | **0** | 3 claimed in `commit-hold`'s flip; the push log has 0 (3 `UND_ERR_INFO`) | above sabA's: not fired |
| Other `fetch failed` | 2 (`UND_ERR_INFO`) | 3 (`UND_ERR_INFO`) | |
| Leader memory peak (memguard) | 83.3 MiB | 101.5 MiB | |

Notes on the lines:
- **Edge 503s** were all "every member asked shed the read: overloaded": task 1,083, graphql 975, rest-repo-write 708,
  renew 527, upload-pack 337, lease 204, receive-pack 6, info-refs 2 (`rows-chA.status`).
- **`ERR_HTTP2_STREAM_ERROR` on sabA.** `commit-hold`'s flip line says "sabA's 3". sabA's push log has three
  `fetch failed` pushes, all `UND_ERR_INFO`, and no `ERR_HTTP2_STREAM_ERROR`; sync-ab.md's table says the same. The
  line's intent (none above sabA's) holds either way: chA has 0.
- **Joined share** [inference]: the code logs no join count. The leader admitted 1,077 Human commands
  (`admittedBytes.human.n`, after the row's reset at load start); the cuts in the load window took 829 (`Trace::Cut`'s
  Human count counts what a cut takes, not joiners, which are added at `decide_held`). The 248 not counted at any cut
  are 23% of the admitted; 21% with the widest window. The same count on sabA, which had no join, leaves 17 of 1,132
  (1.5%) uncounted, so that is the method's floor.
- **Against B0-02's replay** (Human p99 3,278 predicted on sabA's trace): 5,780 measured. The replay held the leader
  at sabA's speed; this row's leader was slower (below).
- **TORNALARM OK**: network 131383 was torn down at 22:13:51Z; after the end sweep shards 0, 1 and 2 read `/status`
  running false, `alarm` null. No client request answered 503 `warming` (push log: 0). The end sweep found no shard
  container running; the four pool shards it tore down answered 200.

## Why the hold covered fewer `/commit`s

`tests/fanout/holdmodel.py` on chA's trace [measured for W, inference for the shift]:

| | chA | sabA |
|---|---|---|
| Trace wait W (arrival → its entry applied) p50 | 849 ms | 579 ms |
| W past 1 s / 1.5 s / 2 s | 42.7% / **26.0%** / 13.4% | 15.4% / **7.7%** / 4.7% |
| Shift fitted at the 1.5 s hold to the measured 0.596 | 861 ms | (441 at 1 s, the model's fit) |
| Model's asks at 1.5 s with the 1 s shift (361 ms) | 0.38 | 0.14 (predicted) |

The model predicted the hold on sabA's W. On this row W was longer from the start, so more `/commit`s outlived 1.5 s
than outlived 1 s on sabA, and the leader answered later than the model's shift on top. The asks did not fall, and
`/outcome_by_request` tripled (12.6 → 37.6/s), lcA2's amplification signature (OPEN-QUESTIONS A39).

## Reading

[inference; one row against one row, run-to-run variance unmeasured] The leader ran slower from the first 30 s: 5 to
13 ticker stalls ≥ 500 ms in every 30 s window, the confirm p50 and the one-statement confirm under load near double,
and 32% more requests open at entry. Learners' read-index calls slowed with it (p50 629 against 321 ms), the learners
shed reads, and the edge's retries plus `/outcome_by_request` held the leader's inputs at 314/s though `/commit` fell
10%. That is the loop lcA2 showed with a different trigger.

The row cannot say what slowed the leader. Candidates: `commit-hold`'s longer-held `/commit` streams (each holds a
request open 50% longer), `human-join`'s extra decide work and joiner reads, or placement variance. The lead's control
row, `human-join` without `commit-hold`, splits the first two.

## The next row, ruled before it runs

From merged main with `commit-hold` reverted; same shape, learners kept, flag off, $10 stop. Keep `human-join` if Human
commit-stage p99 is at or below sabA's 3,589 and the wall time with a frozen batch waiting is below 69%; otherwise
`human-join` is reverted too. If it fails on the same leader slowdown, stop and bring the analysis before a third row.

## Raws

- `tests/fanout/raw/`: `costrow-chA.status`, `rows-chA.status`, `terms-chA.jsonl`, `tsave-chA.jsonl.xz`,
  `memguard-chA.{err,jsonl.xz}`, `arrivals-chA.jsonl`, `shardgate-chA-b0-33-{0,1,2}.jsonl`, `siteday-ch.status`,
  `reretire-graft-m2b-ch.log`.
- `tests/am1/raw/a1-b0-33-mwqyi49p.jsonl`; `tests/am1/raw/runs/a1-b0-33-fchA-b033.log`,
  `fanout-ch-{edge,siteday}.log`. The push log (`a1-pushes-b0-33-mwqyi49p.jsonl`, 7.5 MB) stays local, as sabA's did.
- Tools, unchanged: `tests/fanout/syncab.py`, `tests/m0/cuttrace/{bycut,entries,cuts}.py` (entries.py over 300 s),
  `tests/fanout/b033probes/{humansplit,probe22}.py`, `tests/fanout/leadcycle.py`, `tests/fanout/holdmodel.py`,
  `tests/fanout/readwait.py`, `tests/fanout/tornalarm.py`.
