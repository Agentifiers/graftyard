# B0-33 closing row clA on graft-m2b: met

The lead's closing row for B0-33. It ran after the h1 set (`tests/am1/results/b033-h1-set.md`), whose user-token line
was not reachable on `dc880f1d`, because that build skipped DEMOTE_PER_MIN for plain PATs.

**Build, from main `daa4dcc2`:**
- `user-demote`: the edge sends every authenticated user write with its actor, and `/stats` counts demotions.
- `user-demote-note`.
- `slot-bound`: an agent validate's slot wait is bounded by its push's bound.
- The h1 set's readers.

**Deploys:**
- Shard image `sha256:cd54d680…`.
- Site day `-cl` (test-faults): sites a, b and c f15f63c5, 7d7cd10c and 0da34177.
- Edge day: graft-m2b-edge 8d688aad at 100%.
- The deploy logs are not kept, because they name the account and the workers subdomain. The versions above are the
  ones they report.

`agent-rows-cache` (lever 1) is not in this build. It needs its own site day, so it rides a later B0-33 run.

**Run:** one guarded fast-host run (`tests/fanout/guardrow.sh`, `GUARD_WANT=fast`), the h1 set's load (5,000 agents,
300 s, `--seed-prs 1000 --advert-probe`), RUN_STOP $10.
- The guard kept repo 131414 at 25.7 ms of leader CPU a setup commit (line 28.4).
- a1 run `mwuiu47v`, load from 10:06:13Z, done 10:20:19Z.
- a1 sent its Human load round-robin over five user PATs (`tests/am1/results/b033-user-demote.md`).

**Raws:**
- `tests/fanout/raw/{rows,costrow}-clA.status`;
- `{edgefull,instcpu,memguard,terms,arrivals,tsave}-clA.jsonl.xz`, and `memguard-clA.err`;
- a1's row, push log, samples, run log and the bytediff under `tests/fanout/raw/a1-cl/`.

## Verdict: B0-33 met

The lead's rule (2026-11-24): B0-33 met is judged on the h1 set's median (h1A to h3A) for every line those runs could
read, plus clA for the user-token line and as a fourth run meeting every line.

| Line | Pass | h1 set median | clA |
|---|---|---|---|
| Agent commands/s admitted | ≥110 | 115.3 | 113.4 (34,021 ok over 300 s) |
| Human commit p99 (ms) | ≤3,912 | 1,372 | 1,977 (n 129) |
| MergeQueue (merge push) commit p99 (ms) | ≤3,912 | 1,338 | 1,674 (n 54) |
| User token admitted as `Agent` | yes | not reachable on `dc880f1d` | **met**: the fast user (7100002) demoted on 265 writes |
| Leader changes | 0 | 0, 0, 0 | 0 (term 2 throughout) |
| `suspect` reports that start an election | 0 | 0, 0, 0 | 0 (LAX: leader 1, promised 69, busy 1) |
| Refusal shape | met | met, met, met | met |

clA's guards and checks:
- **Human guard** (`tests/fanout/demoted.py`): no Human token demoted. The leader's `demoted` named one actor, the fast
  user, and none of the five Human writers (7100003, 7100100 to 7100103).
- **Refusal shape:** 9,319 Agent refusals, all `ng` or 429 with `Retry-After` (ng 4,774). There were 254 Human 503s,
  all F65 member sheds on graphql, and every one carried `Retry-After`.
- **bytediff before load on the new edge:** 348 replies compared, 0 differ. One read was unavailable, a
  `GET /repos/m3org/b034-mono` 503, which counts as load and is excused. rowsUnparsed lines: 0, with 2,168 `edgefull`
  lines as the control.
- **Harness outages:** 1 s, a share of 0.0034, so the run is valid.

The fast user's own answers were 383 × 200 and 115 × 503. Demotion changes the class its writes queue under, not its
replies: its 503s are the F65 member sheds every graphql writer met.

## Levers

| Line | h1A | h2A | h3A | clA |
|---|---|---|---|---|
| Lever 2 (`slot-bound`): moved validates that ended ok | 29 of 1,010 | 7 of 28 | 362 of 839 | 337 of 1,919 |
| A moved validate's own time, p50 (p90), ms | 4,917 (6,606) | | 2,732 (3,460) | 1,194 (3,927) |
| Lever 1 baseline: agent push context p50 (p90), ms | 940 (3,438) | 423 (1,229) | 603 (2,078) | 462 (3,004) |
| Before validate (front, gate and context) p50, ms | 2,366 | 1,104 | 1,540 | 1,830 |
| Agent pushes ok; full; full at `bound_at_send` | 9,295; 4,211; 3,225 | 12,402; 1,126; 435 | 11,108; 2,343; 1,192 | 8,741; 4,774; 2,045 |
| Git cache busy (validate 429 seen by a1) | 0 | 0 | 85 | 284 |
| Agent validates by site (0, 1, 2) | 91.5%, 6.1%, 2.5% | 99.8%, 0.2%, 0 | 93.3%, 6.7%, 0 | 86.2%, 13.0%, 0.9% |
| vCPU-minutes by site (0, 1, 2) | 48.4, 20.8, 4.2 | 44.9, 13.7, 3.3 | 51.6, 18.4, 2.7 | 57.1, 28.2, 6.3 |

`slot-bound`'s settle reading is in `decisions/DECISIONS.md`, "`slot-bound`".

## Flips

- **Flip 2 (`tests/fanout/flip2.py`, the lead's per-window reading): 5 of 33 refusing windows (15%), 481 of 2,042
  refusals.** That is over 10%, and the first time on a fully passing run. It is a trigger, not a gate line, and it is
  recorded as `decisions/OPEN-QUESTIONS.md` A50 (`edge-refusal-bound`). On the first reading it was 33 of 33. The edge's
  refused writes had `budgetMs` p50 215 ms and advert `waitMs` p50 466 ms.
- **A50's reading 3, its settle reading (the lead's ruling of 2026-11-24, after this run): 1 of 31 judged windows (3%),
  not met.** Each refusing window's advert p50 against the commit p50 of agent writes sent in that window. 2 windows
  were excluded for fewer than 10 sent: the 60 s adverts at 10:06:00Z and 10:06:10Z, with 3 and 4 sent. The one over is
  10:09:00Z, 2,392 against 1,735 ms over 104 sent. The advert/commit median is 0.50. Sent writes are a survivor sample:
  only those whose budget passed the advert. Reading 2, budget against the refusing advert's own wait, is 0 of 2,042
  by construction. The advert was 1.31 times the cut reading at the median, so the 5 flips above came from the cut
  reading under-stating the wait [inference].
- **R2b:** push half 8,741 against erDA's 8,369, and drain half 107.7 against erBA's 78.7. Neither is flipped.
- **Flip 3:** 2 refusing windows with the leader's agent lane empty.
- **Flip 4:** 124 fail-opens against 2,040 refused, under half.
- **R1b and R1c:** not met.

## Memory and cost

- Peaks: leader 89.1 MiB, followers 31.9 and 59.4 MiB, against memguard's stop at 120. 0 resets in the window.
- Run cost $2.98. graft-m2b's UTC day was $18.64 after it, including the image, the site day and the edge day.
