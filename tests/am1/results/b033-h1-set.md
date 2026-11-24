# B0-33 on dc880f1d: the h1 set on graft-m2b

The lead's three-run set on merged main dc880f1d, after the `-h1` site day from 40358386 (test-faults) and the edge
day e35640fd from dc880f1d (`tests/am1/raw/runs/fanout-h1b-edge.log`, not kept). The build carried:
- log-page (11f1a69a), LM-1's leader-memory page bound;
- agent-spill (G40), where a 429 moves an agent validate to the next Full site;
- `edge-tierrows-shed`, where a member's unparseable `rows` answer is shed, not answered "site release";
- the set readers.

Each run was a guarded fast-host B0-33 run (`tests/fanout/guardrow.sh`, `GUARD_WANT=fast`, under 28.4 ms of leader
CPU per setup commit), with `--seed-prs 1000 --advert-probe`, 5,000 agents for 300 s, RUN_STOP $10, and graft-m2b's
day under $40. They are judged on the median of the three (BUILD-PLAN B0-33).

Raws:
- `tests/fanout/raw/{rows,costrow}-h{1,2,3}A.status`;
- `{edgefull,instcpu,memguard,terms,arrivals,tsave}-h{1,2,3}A.jsonl.xz`;
- a1's rows, push logs, samples and run logs under `tests/fanout/raw/a1-h1set/`.

## Gate lines

| Line | h1A | h2A | h3A | Median | Pass |
|---|---|---|---|---|---|
| a1 run, repo | mwub8kiw, 131411 | mwude6ie, 131412 | mwufi6gv, 131413 | | |
| Host guard (ms of leader CPU a setup commit) | kept | 28.3 | 19.5 | | |
| bytediff before load | 0 of 348 differ | 0 of 348 | 0 of 348 | | |
| Agent commands/s admitted | 103.9 | 134.6 | 115.3 | 115.3 | ≥110: met |
| Human commit p99 (ms) | 1,372 | 1,958 | 1,198 | 1,372 | ≤3,912: met |
| MergeQueue (merge push) commit p99 (ms) | 1,338 | 2,501 | 1,178 | 1,338 | ≤3,912: met |
| User token admitted as Agent | not reachable | not reachable | not reachable | | see below |
| Leader changes | 0 | 0 | 0 | 0 | met |
| `suspect` reports that start an election | 0 | 0 | 0 | 0 | met |
| Refusal shape | met | met | met | | met |

- **Agent commands/s** sums a1's `byKind` codes ok/200/201/204 over `push:agent` and every `side:*` except
  `side:human`, divided by 300.
- **Commit p99** is the `commit;dur=` stage of a1's ok `human` and `merge` pushes. The p99 is
  sorted[min(n-1, int(0.99n))].
- **Refusal shape** is `tests/fanout/refusalshape.py`. Every Agent refusal was `ng` or 429 with `Retry-After`: 14,024
  (ng 4,211), 2,733 (ng 1,126) and 6,905 (ng 2,343). The Human 503s, all F65 member sheds and all with
  `Retry-After`, were 602, 124 and 355.
- **rowsUnparsed** lines were 0 on h2A and h3A (control: 575 and 996 `edgefull` lines).
- **The user-token line is not reachable on dc880f1d; the build skipped DEMOTE_PER_MIN for plain PATs.**
  `crate::admission` returned `None` for a plain user token, so the leader never saw the fast user's actor. a1's fast
  user got 198 × 200 and 300 × 503 on h1A, and 316 × 200 and 167 × 503 on h3A, as a Human writer would.
  `user-demote` (`decisions/DECISIONS.md`; `tests/am1/results/b033-user-demote.md`) fixes the build. B0-33 is met only
  once a run on it reads the fast user as Agent.

The median agent line is 115.3, so there is no 90% gap to record. h1A alone was at 94% of 110.

## The flips (design-reversal triggers, not gate lines)

| Flip | h1A | h2A | h3A |
|---|---|---|---|
| 2, over-refusal, as ruled 2026-11-24 (`tests/fanout/flip2.py`) | 0 of 31 (63 of 869 refusals) | 3 of 30 (10.0%), at the line | 3 of 31 (9.7%) |
| 2, as first written (leader wait under 2.5 s) | 31 of 31 | 29 of 30 | 31 of 31 |
| R2b push half (agent ok pushes against erDA's 8,369) | 9,295 | 12,402 | 11,108 |
| R2b drain half (cut/s while refusing against erBA's 78.7) | 95.1 | 137.9 | 111.1 |
| R1b, R1c | not met | not met | not met |

Notes on the flips:
- **Flip 2** is the lead's reading of 2026-11-24. A refusing window counts only when its refusals' budget left at the
  check exceeds the leader's `expected_wait` in that window. At the run's p50 round in place of each window's own,
  h1A read 4 of 31 (13%). It flips over 10%, so h2A's 10.0% does not flip. `decisions/DECISIONS.md`, "`edge-refusal`",
  flip 2, records the readings.
- **R2b's drain half** is read against erBA's 78.7, as the lead ruled on h1A. `tests/fanout/edgefull.py` printed
  erDA's 114.2 on these runs and read h1A and h3A FLIPPED; it now reads erBA's.

## G40, the home site

| Line | h1A | h2A | h3A |
|---|---|---|---|
| Agent validates by site (0, 1, 2) | 91.5%, 6.1%, 2.5% | 99.8%, 0.2%, 0 | 93.3%, 6.7%, 0 |
| Moved on after a failed try | 1,010 | 28 | 839 |
| Git cache busy (validate 429) in a1 | 0 | 0 | 85 |
| vCPU-minutes by site (0, 1, 2) | 48.4, 20.8, 4.2 | 44.9, 13.7, 3.3 | 51.6, 18.4, 2.7 |
| Agent pushes full, of which `bound_at_send` | 4,211, 3,225 | 1,126, 435 | 2,343, 1,192 |
| Budget left at validate's end, p50 (share under 1 s) | 1,921 ms (31.9%) | 3,429 ms (5.7%) | 2,526 ms (20.7%) |

- h1A's 1,010 moved validates got 429 "validate queue full" from a pool shard after about 2,007 ms (`via=0&pool=1`).
  The home shard passes that 429 through, so pushfull's "busy" read 0. Lever 2 (F112) is the pool gate's 2 s agent
  slot wait plus the second-site try.
- Lever 1 (LM-1), caching the agent rows with the gate decision, targets push context's own member read: about 1.9 s
  of p50 on 70% of h1A's agent pushes.
- `tests/fanout/vsplit.py` prints the validate split by site.

## Memory and cost

| Line | h1A | h2A | h3A |
|---|---|---|---|
| Peak MiB (leader; followers), memguard's stop at 120 | 78.0; 59.4, 55.3 | 84.1; 30.1, 94.3 | 78.9; 82.4, 67.5 |
| Resets in window | 0 | 0 | 0 |
| Run cost | $2.91 | $3.24 | $2.90 |

graft-m2b's UTC day stood at $15.63 after h3A.
