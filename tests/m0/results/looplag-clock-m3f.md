# looplag-clock on graft-m3f: B0-29 row m3fp3 (2026-11-19)

The orchestrator (T-01a17bda) granted this hosted rerun of B0-29 after `looplag-clock` merged
(`decisions/DECISIONS.md`, "A leader whose loop is behind steps down (`LOOP_LAG`)"). It compares with m3fhg
(`tests/m0/results/held-grant-m3f.md`) and m3fp2 (`tests/m0/results/confirm-storage-m3f.md`).

- **Build.** Site day m3fp3 from main `3e521931`, which carries `looplag-clock` and `disk-first-reads`. It is a
  test-faults build.
  - Sites a/b/c serve `28b8fd64` / `ae942427` / `ff9ddeae`.
  - There was no edge day. graft-m3f-edge still serves `e25b8638` (main `ab6086b4`).
  - The image `sha256:1984a852…` was reused: `containers/git` at `3e521931` builds the same digest as m3fp2's.
  - The site day took four tries:
    - Tries 1 and 2 ran wrangler 4.165.0, which `npx` fetched because the new worktree had no `tests/m0/node_modules`.
      Both failed with "Container application settings were not fully applied".
    - After `npm ci` in `tests/m0`, which installs the pinned 4.153.0, try 3 failed at site c phase 2 with an API
      timeout.
    - A manual `deploy.mjs --only 2 --skip-build` failed once on the container settings, then succeeded at
      22:58:28Z.
- **Row.** One B0-29 row on m3fhg's script (`tests/fanout/b029-m3f.sh`) with held-grant on and a fresh repo:65597.
  - a1 run `mwo5m105`.
  - Load ran 23:08:42–23:18:42Z through the egress proxy, under the harness-outage rule.
  - The leader was site 0 at the start.
  - The row service was stopped before b029.sh's colos, placement and outages steps, so those three were run by hand.
    Their output is in `rows-m3fp3.status`.
  - The first outages run read graft-m2b, because `outages.sh` defaults `RUN_PREFIX` to it. It printed
    "n/a (low rate)". The rerun with `RUN_PREFIX=graft-m3f` is the last block in the status file.
- **Stop.** $6 on graft-m3f, counted from 22:22:48Z. The install spent **$2.57** by 23:29:54Z (`acctcost.py`,
  `prefix_usd`).
- **Raw files.**
  - `tests/fanout/raw/rows-m3fp3.status`, `terms-m3fp3.jsonl`, `arrivals-m3fp3.jsonl`, `edge500guard-m3fp3.status` and
    `tsave-m3fp3.jsonl.xz`.
  - a1's `a1-b0-29-mwo5m105.jsonl` and its samples file are under the ignored `tests/am1/raw/` on this thread's orb.

The row is judged against B0-29's 800 ms commit line, 880 at 90%, with a fail over 1 s (`FRAME.md`, "Operator
decision, 2026-11-19, B0-29's commit line").

## Verdict lines

| Line | m3fp3 | Against |
|---|---|---|
| Leader setup 1-statement confirm p50 / p90 | **23 / 33** (site 0, n 586; `lsrefsab.py`). Under load: 1-statement p50 37, pooled p50 30, p99 324 | m3fhg 16 / 19, m3fp2 23 / 33 |
| Colos | PDX 556, LAX 206 | m3fhg and m3fp2: LAX only |
| Commit p99, modelled | **1,793 ms** (p50 524, p90 860, n 10,854). `bycut.py` per leader segment, pooled. Segments: s0 23:08:43–09:53 2,041; s1 23:10:00–14:56 1,210; s0 23:14:58–15:37 2,177; s1 23:15:45–18:42 1,633 | **Fail**: over the 1 s fail line |
| Commit p99, `/stats` | **4,435 ms** (p50 627, n 1,892). Covers only the final leader (s1, term 5, from 23:15:45). That is 2.8 s over that segment's model of 1,633, not the usual small offset | **Fail** |
| The same with stalls of 10 s or more removed | Unchanged. **No storage confirm of 2 s or more and no stall of 10 s or more** on any voter during load | **Fail** |
| Step-downs by reason | **3** (Workers Logs "member alarm"):<br>• 23:09:53.132 s0, `CommitErrors { calls: [(1,32,3),(2,29,3)] }` to s1. Window: slow 0 of 51, storage max 235, isolate samples up to 943. `leaderPeers` failedCalls 63 (s1: deadline 15, fetch 26; s2: deadline 22).<br>• 23:15:37.555 s0, `LoopLag { clamped: 2 }` to s1. Window: slow 0 of 34; storage ≥200 ms: 291 / 387 / 230; whole up to 438. Isolate: two distinct syncs (3400 and 3405), each clamped at 200. s0's ticker logged real lag at 23:15:30–34 (216, 466, 349, 214, 247, 553 ms).<br>• 23:18:55.148 s1, `CommitErrors` to none (a hold), after load ended | The LoopLag trip matches the rule as amended: two syncs, with real ticker lag. It is not m3fp2's false positive |
| Leader changes | **3**:<br>• 0 → 1, term 3, 23:09:54.6;<br>• 1 → 0, term 4, 23:14:57.9. No step-down alarm was found; probably the transfer home, unverified;<br>• 0 → 1, term 5, 23:15:41.8 | m3fhg 1, m3fp2 1 |
| Leader storage confirms of 2 s or more | **0** (adds 0.17 h of load with none to F99's count). Per voter during load: s0 p50 30, p99 288, max 994 (11 ≥500 ms); s1 26 / 109, max 1,743 (4 ≥500 ms); s2 4 / 209, max 392 | m3fhg one of 23,957 ms |
| `merge:only` p99 | **3,842 ms** (p50 1,441, n 67, served 83.6%). mergeSplit `tier_ahead` p99 1,384. The **11 × 409 are all `BaseModified`**, and each overlaps a 200 on the same base | m3fp2 15,836 (with its change) |
| held-grant's flip | **1 of 123** walked within 2 s of a held-and-stepped leader read: `ranges` 1 of 108, merge rows 0 of 2, q4 0 of 3, q23 0 of 10. These are site 0's reads only, and site 0 led only part of the row | **Pass** (about 95% before; m3fhg 0 of 179, m3fp2 0 of 223) |
| Round p50 / p99 | Trace (append → apply), by segment: s1 236 / 733 and 228 / 928; s0 181 / 1,537 and 273 / 1,973. `/stats` (final leader): round 242 / 715, confirm 50 / 900 | m3fhg 180 / 323, m3fp2 196 / 429 |
| Edge 500s or exceptions | **0** status-500 and 0 exceptions (`edge500guard`) | One would have stopped the row |
| Other lines | `read:api` p99 2,346 (98.83% served); `read:ref` p99 3,405 (98.9%); `push:agent` p99 6,274 (98.44%) | |
| Harness outages | 6 s (1%), **valid** | Over 5% is invalid |
| Spend | **$2.57** on graft-m3f | $6 stop |

## Site a's Member errors

Site a logged two error strings that no earlier row logged:

- 220,515 × "closure invoked recursively or after being dropped";
- 3,897 × "Cannot perform I/O on behalf of a different Durable Object".

Workers Logs, `$metadata.message` counts:

| Window | Build | closure | different DO |
|---|---|---|---|
| m3fp3, 23:00–23:30Z | `3e521931` | **220,515** (site a only) | **3,897** (site a only) |
| m3fp2, 19:30–19:50Z | `d8dcd513` | 0 | 0 |
| graft-m3f 12:00–21:24Z | m3fhg, m3fp2 | 0 | 0 |
| B0-35 m58 on graft-m4a, 21:24–22:04Z | `f50ad36b` | 0 | 0 |
| B0-34 mt7 on graft-m1, 22:20–23:00Z | `f7b08de5` | 0 | 0 |

- **Two Durable Objects.** All 220,515 come from two Member Durable Objects of site a's script version `28b8fd64`.
  - Object A serves `POST /read`, `/feed`, `GET /isolate` and `/trace`, plus alarms (112,228).
  - Object B is site 0's voter: `POST /raft`, `/entries`, `GET /status`, `POST /commit`, plus alarms (108,287).
  - No other object in the isolate logged either string.
- **Start.** Both objects logged their first error in the same millisecond, 23:08:57.536, 15 s into load. The first
  "different Durable Object" error was on A's `/feed`, 0.7 s later.
- **Duration.** The errors ran until 23:19:19, with both alarms (5,064) and fetches (215,451) failing.
- **No other symptom.** Site a logged no panic, `unreachable` or `RuntimeError` in the row.

The cause is under analysis; the next section will hold it.
