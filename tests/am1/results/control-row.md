# control-row: B0-33 with human-join and without commit-hold, tag crA (graft-m2b)

LM-1, 2026-11-21. One m64-shaped B0-33 row on main `3acfd02a`: `commit-hold` reverted, `human-join` kept, plus
`colo-gate` (`292c432e`, edge) and `teardown-stops-runs`. It was the lead's control after `tests/am1/results/combined-row.md`
(chA), set to split `human-join` from `commit-hold`. Tags follow `design/SHARED.md`, "Read this first".

**Short answer.** The row is valid. `human-join` fails the keep rule set before the run: Human commit p99 3,968 ms
against at most 3,589 (sabA), and a frozen batch waiting 81% of wall time against under 69%. The lead ruled it
reverted (`decisions/DECISIONS.md`, `human-join`, Status). Without `commit-hold` the leader was about halfway back from
chA towards sabA, not all the way.

## Setup

- Site day from `3acfd02a`, siteday exit 0; edge `0debc0c6` at 100% (rev `3acfd02ab`). `reretire.sh` printed RERETIRE
  OK graft-m2b (`tests/fanout/raw/reretire-graft-m2b-cr.log`).
- One B0-33 row, m64 shape, four learners (`enam:2000,weur:1000,apac:1000`), flag off: tag crA, repository 131384, run
  `mwr24b0t`, load 23:49:06 to 23:54:06Z, done line 23:56:53Z. Site 0 led at term 2 throughout, 0 leader changes.
- Harness outages 1.0 s (0.36%): valid. Run cost $2.07; graft-m2b $12.33 on 2026-11-21 (UTC); account $52.00 that day.

## Verdict lines

[measured unless tagged]

| Line | crA | chA | sabA | Rule |
|---|---|---|---|---|
| Human commit stage p99 ms (push log, n) | **3,968** (154) | 5,780 (114) | 3,589 (126) | keep if ≤ 3,589: **fail** |
| Merge commit stage p99 ms (n) | 4,197 (39) | 5,848 (48) | 2,728 (30) | |
| Humans within 1 s | 9% | 7% | 32% | |
| B0-29's sampler on this run (bycut two rounds p99 + entries.py round p50) | 3,574 + 333 = 3,907 | 3,628 + 410 = 4,038 | 3,620 + 292 = 3,912 | |
| `/stats` sampler on this run (2 × round p99 + p50) | 2 × 1,226 + 440 = 2,892 | 3,588 | 2,631 | |
| Wall time with a frozen batch waiting (humansplit) | **81%** | 95% | 69% | keep if under 69%: **fail** |
| Human p50 / p90 / p99 ms (humansplit, real arrivals, n) | 1,509 / 2,883 / 4,887 (139) | 1,806 / 2,956 / 3,322 (107) | 865 / 2,742 / 4,036 (117) | |
| Human median split: freeze wait + held + own round | 601 + 336 + 560 | 454 + 783 + 535 | 304 + 191 + 384 | |
| Humans joined a frozen batch [inference] | about 21% (1,249 admitted, 989 taken at cuts) | about 23% | 1.5% (the method's floor) | |
| Ticker stalls ≥ 500 ms (total) | 36 (29.6 s) | 81 (77.8 s) | 10 (9.1 s) | |
| Leader confirm wall p50 / p90 / p99 ms | 72 / **555** / 2,025 | 91 / 648 / 1,977 | 52 / 379 / 2,076 | |
| Leader open requests at entry p50 / p99 / max | 413 / 723 / 1,018 | 423 / 890 / 1,099 | 320 / 567 / 691 | |
| Leader inputs/s | 347.8 | 314.0 | 314.2 | |
| of which `/commit`, `/outcome`, `/raft` (learners), `/outcome_by_request`, `/read` | 150.1, 99.0, 55.9 (51.8), **22.8**, 4.7 | 131.7, 78.5, 49.4 (45.6), 37.6, 6.7 | 146.0, 75.7, 58.0 (55.8), 12.6, 3.7 | |
| `/outcome` per `/commit` | 0.66 | 0.596 | 0.518 | |
| Round p50 / p99 ms (`/stats`); entries.py | 440 / 1,226; 333 / 2,412 | 504 / 1,542; 410 / 2,338 | 359 / 1,136; 292 / 2,314 | |
| Full cycle p50 (`leadcycle.py`) | 612 | 704 | 560 | |
| `queue:slot` p99 ms | 7,740 | 8,332 | 5,798 | |
| Agent commands/s cut | about 97.8 | 81.5 | 103.2 | |
| Agent pushes ok | 3,538 of 12,398 | 1,872 of 12,493 | 5,477 of 12,547 | |
| Edge 503s | **607** | 3,842 | 525 | under 1,050: pass |
| Learner read wait p50 / p99 ms (queue + call) | 475 / 1,550 | 765 / 1,996 | 365 / 1,568 | |
| `ERR_HTTP2_STREAM_ERROR`; other `fetch failed` | 0; 2 (`UND_ERR_INFO`) | 0; 2 | 0; 3 | |
| Leader memory peak (memguard) | 97.5 MiB | 83.3 MiB | 101.5 MiB | |

Notes:
- **Edge 503s** (`rows-crA.status`): 604 "every member asked shed the read: overloaded" (graphql 221, rest-repo-write
  170, task 120, renew 76, lease 17) and 3 merge 503s on "the git cache: N {"warming":true}".
- **TORNALARM OK**: network 131384 was torn down at 23:55:06Z; after the end sweep its shards 0, 1 and 2 read `/status`
  running false, `alarm` null. The three `warming` 503s were answered in the load window, before the teardown, so no
  request answered `warming` from a torn-down shard. The end sweep found no shard container running.
- **Ticker stalls** came from T+60 s on: 0, 0, 4, 4, 2, 1, 7, 6, 9, 3 per 30 s.
- **Against the candidate lines** (lead's note): under B0-29's sampler at 90% (4,347), crA's Human 3,968 and merge
  4,197 pass, and both fail the `/stats` line (2,631). The build is being reverted, so this is not a gate pass.
- **Joined share** uses the method in `combined-row.md`: the leader's admitted Human count after the row's reset, less
  the Humans the load-window cuts took (`Trace::Cut` counts what a cut takes; joiners are added at `decide_held`).

## Reading

[inference; one row against one row, run-to-run variance unmeasured]
- Reverting `commit-hold` moved the leader about halfway back from chA towards sabA: stalls 81 → 36, edge 503s
  3,842 → 607, learner read wait p50 765 → 475 ms, confirm p90 648 → 555 ms.
- About a fifth of the Humans joined a frozen batch, yet their median freeze wait doubled against sabA (601 against
  304 ms). The replay's predicted Human p99 of 3,278 ms assumed sabA's leader speed, which neither row had.
- What remains between crA and sabA: `human-join`, `colo-gate` (an edge change), and placement variance. The lead's
  next row, on main with `human-join` reverted, is sabA's commit path plus `colo-gate`, and measures the rest.

## Raws

- `tests/fanout/raw/`: `costrow-crA.status`, `rows-crA.status`, `terms-crA.jsonl`, `tsave-crA.jsonl.xz`,
  `memguard-crA.{err,jsonl.xz}`, `arrivals-crA.jsonl`, `shardgate-crA-b0-33-{0,1,2}.jsonl`, `siteday-cr.status`,
  `reretire-graft-m2b-cr.log`.
- `tests/am1/raw/a1-b0-33-mwr24b0t.jsonl`; `tests/am1/raw/runs/a1-b0-33-fcrA-b033.log`, `fanout-cr-{edge,siteday}.log`.
  The push log stays local, as chA's and sabA's did.
- Tools, unchanged, as in `combined-row.md`.
