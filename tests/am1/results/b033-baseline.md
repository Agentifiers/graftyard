# b033-baseline: B0-33's three-run baseline set on main `06581e8e` (graft-m2b)

LM-1, 2026-11-22. The first set judged under FRAME, "Operator decision, 2026-11-22, B0-33's commit line and three-run
medians": Human and merge-queue commit p99 at most 3,912 ms (4,347 at 90%), judged on the median of three runs on
fresh placements, with no run in the fail column. Tags follow `design/SHARED.md`, "Read this first".

**Short answer. The set fails.** The median run meets the line for Humans (4,244 ms by the push log, 4,040 by B0-29's
sampler, both under 4,347). It does not meet it for merges: the median is 5,365 ms. Every run is also in B0-33's fail
column on three lines:
- under 100 agent commands/s: 76.5, 98.0 and 79.0;
- merges waiting over 2 s;
- `suspect` reports.

The admission shortfall is on the leader's side; the offered load was the same on every run [measured, below].

## Setup

- One site day from `06581e8e`, siteday exit 0. Edge `baa617cb` serves 100% (rev `06581e8e2`).
- Before each run, `reretire.sh` printed RERETIRE OK graft-m2b (`tests/fanout/raw/reretire-graft-m2b-bl{A,B,C}.log`).
- Three m64-shaped B0-33 runs, each on its own fresh repository: four learners (`enam:2000,weur:1000,apac:1000`),
  flag off, $10 run stop.
- Run 3 was first started as blC. Its `rows.sh` start sweep failed on a TLS handshake timeout from the containers
  API, before any repository or load existed. A manual sweep then found no shard running, and the run went again as
  blC2 on the reretire from 04:05Z, with no row in between.
- Harness outages were 0.7%, 0.4% and 0.4%, so all three runs are valid. The watchdog logged 0 edge 500s, exceptions
  or traps.

| Run | Repo, a1 run | Load (UTC) | Done line | Run cost |
|---|---|---|---|---|
| blA | 131386, `mwr8drgl` | 02:44:24 to 02:49:24 | 02:52Z | $1.60 |
| blB | 131387, `mwr9ur4k` | 03:25:41 to 03:30:41 | 03:33Z | $2.34 |
| blC2 | 131388, `mwrbm9cj` | 04:15:03 to 04:20:03 | 04:23Z | $1.68 |

At 04:25Z graft-m2b stood at $7.53 and the account at $9.24 for 2026-11-22, counted from 00:00Z.

## Per run

[measured]

| Line | blA | blB | blC2 | sabA (reference) |
|---|---|---|---|---|
| Leader colo (`#1#f0`, DO analytics) | IAD | **EWR** | IAD | EWR |
| Human commit stage p99, push log (n) | 7,854 (128) | 2,523 (119) | 4,244 (123) | 3,589 (126) |
| Merge commit stage p99, push log (n) | 5,365 (44) | 2,366 (36) | 9,185 (37) | 2,728 (30) |
| B0-29's sampler: bycut two rounds p99 + entries.py round p50 | 4,634 + 387 = **5,021** | 2,805 + 264 = **3,069** | 3,611 + 429 = **4,040** | 3,912 |
| Ticker stalls ≥ 500 ms, 300 s (per minute) | 55 (11.0) | **0** (0) | 89 (17.8) | 10 (2.0) |
| Leader confirm p50 / p90 / p99 ms | 76 / 600 / 2,158 | 52 / 235 / 1,628 | 89 / 486 / 2,076 | 52 / 379 / 2,076 |
| Leader inputs/s | 333.9 | 287.2 | 317.3 | 314.2 |
| Agent commands/s cut (`cuts.py`) | **76.5** | **98.0** | **79.0** | 103.2 |
| Agent queue left after a cut, p50 / p99 | 482 / 974 | 1 / 328 | 304 / 934 | |
| Leader changes (terms) | 0 | 0 | 0 | 0 |
| Voters' `/suspect` Reported rows (LAX) | 5,069 | 5 | 1,093 | 35 |
| Merges over 2 s at the commit stage | 28 of 44 | 4 of 36 | 27 of 37 | 10 of 30 |
| Edge 503s | 4,835 | 343 | 1,168 | 525 |
| `ERR_HTTP2_STREAM_ERROR`; other `fetch failed` | 0; 1 | **88**; 16 | **342**; 6 | 0; 3 |
| Wall time with a frozen batch waiting (humansplit) | 90% | 56% | 90% | 69% |
| TORNALARM; 503 `warming` to a client push | OK; 0 | OK; 0 | OK; 0 | OK; 0 |

Notes:
- On blA and blC2, `terms.mjs` could not read member 0 for one or two polls (02:48:58 to 02:49:02, 02:49:39 to
  02:49:42, 04:18:30 to 04:18:36Z), while the followers stayed at term 2. Site 0 then read leader at term 2 again. These
  were stalls the poll timed out on, not leader changes.
- Most edge 503s on blA and blC2 are "every member asked shed the read: overloaded" (the tier-row read gate), plus the
  leader's admission `full`. On blB, 333 of the 343 are the read gate and 10 are merge `warming`.
- The merge `warming` 503s (a1 counted 11 on blB's `merge:only`) are on the REST merge route, not on pushes.
- The fast user's REST writes answered 503 on 72, 14 and 16 calls. This is the same read-gate 503 the rows before this
  one had, and those rows did not count it as a refusal shape either.

## The set against the line

| | Human p99 (push log) | Merge p99 (push log) | B0-29's sampler |
|---|---|---|---|
| Runs | 7,854 / 2,523 / 4,244 | 5,365 / 2,366 / 9,185 | 5,021 / 3,069 / 4,040 |
| **Median** | **4,244** (blC2) | **5,365** (blA) | **4,040** (blC2) |
| Against 3,912 / 4,347 | over 3,912, meets 4,347 | over both: **fail** | over 3,912, meets 4,347 |

## The fail column

B0-33's fail column (`build/BUILD-PLAN.md`): "Any leader change or `suspect` report, any merge-queue command waiting
over 2 s, or under 100 agent commands/s admitted."

- **Under 100 agent commands/s: all three runs** (76.5, 98.0, 79.0).
  - The shortfall is admission, not offered load. a1 offered the same 252.2 agent-class commands/s, host-made commands
    included, on every run, and 41.67 agent pushes/s [measured].
  - On blA and blC2 the leader cut full agent batches: 64 per entry (`AGENT_CMDS_ENTRY`, p50 66 to 67 commands a cut),
    and it left a backlog of 304 to 482 commands after each cut at the median. Agents were refused by admission (5,769
    and 5,842 pushes) and by `expire_agents` (4,592 and 4,125). So entries a second bounded the agent rate, and stalls
    cut that rate.
  - On blB the leader kept up: 333 of 666 cuts found nothing to take, the backlog after a cut was 1 at the median, and
    refusals were fewer (admission 2,111, the edge's send bound 1,692, expiry 870) [measured]. 98.0 is near what a
    non-stalling leader cuts from this offer, and sabA's 103.2 sits just over the line [inference, two rows].
- **`suspect` reports: all three runs** (5,069, 5, 1,093 Reported rows, all from LAX).
  - sabA had 35, so on the reports the fail line has fired on every m64-shaped row, the reference included [measured].
- **Merges waiting over 2 s: all three runs** (28, 4, 27 at the commit stage). sabA had 10 of 30.
  - The push log's commit stage is the measure here, as in the earlier rows; no row measured the merge-queue wait
    alone.
- **Leader changes: none.**

## blB's and blC2's `ERR_HTTP2_STREAM_ERROR`

- **Route.** All are a1's `POST …/git-receive-pack`, the push itself: 87 agent and 1 Human on blB, 341 agent and 1
  Human on blC2. None is on another route.
- **When.**
  - blB: the failed pushes started 6.9 to 10.9 s after the first push (03:26:06.9 to 03:26:10.9Z) and ended in three
    bursts, 19, 42 and 27 pushes each within one 100 ms (03:26:11.2, 12.9 and 14.1Z).
  - blC2: they started 168 to 183 s into the run and ended in six bursts of 8 to 93 between 174.9 and 188.0 s
    (04:18:16 to 04:18:29Z), next to the 04:18:30Z stall.
- **What the edge saw.** In those seconds the edge's Workers Logs show receive-pack requests with outcome `ok` and
  status 200 only, all at LAX, and no 5xx or exception. The edge logs about two receive-pack POSTs per a1 attempt
  (25,276 against 12,618 on blB), so the requests cannot be matched one to one.
  - Many streams ending inside the same 100 ms points at one HTTP/2 connection between a1 and Cloudflare's front being
    reset, which takes all its in-flight streams with it. That reset is not an edge reply [inference].
  - The client runs through the orb's proxy (`NODE_USE_ENV_PROXY`). The blC2 bursts sit next to the leader stall, which
    held these pushes 8.8 s at the median. The blB bursts came in the first 15 s under no stall [measured]. The cause of
    the reset is not traced [unverified].
- **Against the 103 failed agent pushes.** blB's 103 agent `fetch failed` are these 87 plus 16 `UND_ERR_INFO`. blC2's
  347 are 341 plus 6. sabA, chA, crA and ncA had only `UND_ERR_INFO` (0 to 3).
- **Refusal shape.** These are not refusals. Nothing answered: a1 got status 0 and no body. A refusal outside the git
  `ng` / REST 429 shapes would be a server reply in another shape, and none came. a1 does not retry them: no failed
  client and ref pushed again. They count as lost to the run, 0.7% (blB) and 2.7% (blC2) of agent pushes. Admitted,
  they would lift the agent rate by at most 0.3 and 1.1 commands/s [inference from 64-command cuts carrying one
  push's commands each at most], which does not reach 100 on any run.

## Reading

[inference; three runs plus sabA]
- **Placement decides the run.** The two IAD leaders stalled 11 and 17.8 times a minute; the EWR leader did not stall
  once at the same inputs. That repeats A41: sabA and blB are the EWR placements, and both are the good runs.
- Even the good placement does not clear the fail column. blB, with no stall, had 98.0 agent commands/s, 4 slow merges
  and 5 `suspect` reports. So the set would fail on the fail column even if a median placement looked like blB. That
  is the operator's call on how the fail column reads against three-run medians; nothing here changes the line.

## Raws

- `tests/fanout/raw/` holds, for each run (blA, blB, blC2):
  - `costrow-*.status`, `rows-*.status`, `terms-*.jsonl` and `tsave-*.jsonl.xz`;
  - `memguard-*.{err,jsonl.xz}`, `arrivals-*.jsonl` and `shardgate-*-b0-33-{0,1,2}.jsonl`;
  - `costrow-blC.status` and `rows-blC.status`, the aborted start;
  - `siteday-bl.status` and `reretire-graft-m2b-bl{A,B,C}.log`.
- `tests/am1/raw/a1-b0-33-{mwr8drgl,mwr9ur4k,mwrbm9cj}.jsonl`, plus `tests/am1/raw/runs/a1-b0-33-fbl{A,B,C2}-b033.log`
  and `fanout-bl-{edge,siteday,image}.log`. The push logs stay local, as sabA's did.
- Tools, unchanged, as in `nochange-row.md`: `syncab.py`, `tests/m0/cuttrace/{bycut,entries,cuts}.py` (entries.py over
  300 s), `b033probes/humansplit.py`, `tornalarm.py` and `docpumin.py`'s query, grouped by `coloCode`.
