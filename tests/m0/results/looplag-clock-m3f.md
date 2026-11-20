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
| Commit p99, `/stats` | **4,435 ms** (p50 627, n 1,892). This is **site 0's** `/stats`: a1 reads the starting leader's at the end (`statsFrom: end`, `before: 0`), so it covers site 0's terms 2 and 4, not the final leader. `bycut.py` models site 0's two terms at p99 **2,176** (n 1,941), so the gap is about 2.3 s, not the usual small offset. Site 0's `queueMs` (admission → entry) was p99 2,177, max 4,770, against the model's cutwait plus held of about 1 s at p99. The model puts every command in the next cut, so it misses commands a cut left queued [inference]. Site 1's term 5 alone models 1,633 (2,220 in its first 19 s, 1,430 from 23:16) | **Fail** |
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
- **Duration.** The errors ran without a break until 23:19:19 (5 s counts), on alarms (5,064) and fetches (215,451).
  An earlier count saw bursts every 119 s; that came from 120 s query windows capped at 2,000 events.
- **Requests still answered.** Site a's events in the load window ended `ok` 385,189 times, `exception` 3,897 times
  (exactly the `/feed` different-object errors) and `canceled` 65 times. So the closure errors are thrown in callbacks
  that run after or beside a request, not in the handler that answers it.
- **No other symptom.** Site a logged no panic, `unreachable` or `RuntimeError` in the row.

### Cause: a learner and its voter shared an isolate, and `FEEDS` is keyed by repository

**Measured.**
- Two members of repo:65597 ran in one isolate. a1's `/isolate` probes (`tsave-m3fp3.jsonl`) gave the same isolate id
  `202bf0b1eb80` for **learner 195 and site 0's voter**.
  - The two erroring objects are those two. Their `GET /isolate` log lines line up with the probes: `610f1c…` at
    23:00:20.208 for site 0, and `ab0012…` at 23:00:58.926 for learner 195.
  - `610f1c…` is the voter that logged both step-downs.
- No other tsave with `/isolate` rows has a voter and a learner in one isolate: m3fhg, m3fp2, m2bb29n, m56b33, m57b33
  and m59r33–m61r33. So m3fp3 is the first traced row where this happened.
- graft-m2b-site-a's burst of the same string on 2026-11-13 (94,336 on Member `/read`, plus 4,498 different-object
  errors, build `24dec508`, reported by M4-WP4) also came from exactly two Member objects (`c69e65…` 47,563 and
  `23c976…` 46,773). The same objects logged on `/feed`. That build predates both `disk-first-reads` and
  `looplag-clock`.
- B0-35 m58 on graft-m4a (`f50ad36b`, which has disk-first and no looplag-clock) logged 0 of either string on every
  site and the edge (M4-WP4's count, checked against a known string).

**Read from the code** (`3e521931`):
- `crate::feed::FEEDS` (`workers/site/src/feed.rs`) is an isolate-wide `BTreeMap<RepoId, Rc<RefCell<Feed>>>`. So
  every member of a repository in one isolate shares one `Feed`.
- A `Feed` holds `waiters: Vec<oneshot::Sender<bool>>`, plus the `refreshing` senders, for held `/feed` polls.
- Every member's input runs `Repo::stepped` (`workers/site/src/family.rs`), which ends in `f.fed(now)`. That drains
  every waiter with `send`.
- So the voter's `/raft`, `/entries`, `/commit`, alarm and confirm steps wake the learner's held polls from the
  voter's object.
- Each woken poll loops in `feed::poll`. It drops one `within(..)` `Delay`, whose drop calls `clearTimeout`, then
  sets the next one and touches its response. All of this runs under the waking object's context.
- `lookup-poll` (`decisions/DECISIONS.md`) recorded the same failure across requests. This is the same failure across
  objects:
  - "Cannot perform I/O on behalf of a different Durable Object" on the learner's `/feed`;
  - "closure invoked recursively or after being dropped" from timers whose closures the other object dropped.
- `FEEDS` dates from `7613c04a` (2026-11-08). None of `d8dcd513..3e521931` touches it.
  - looplag-clock's new state, the `syncs` Cell and the 4-tuple `unconfirmed`, sits in the per-object `Host`.
  - disk-first's `MARK_TRACED` is a plain `HashSet<ChainId>`.

**Inference.**
- Errors appear only when the platform places a learner beside a voter of its own repository in one isolate, while
  feed polls are held. That would explain why no row from 11-14 to 11-18 hit it, and why it is not tied to a build.
- The shared `Feed` also mixes the two members' `gap`, `installing`, `applied_at` and ring. A voter's state can make
  the learner's polls lag or stop, and the reverse. That is a correctness bug in the feed, beside the cross-object I/O.
- The error stream had no gap from 23:08:57.536 to 23:19:19, 5 s counts. The 119 s bursts reported earlier were an
  artifact of 120 s query windows capped at 2,000.

### Against the commit tail

- The errors ran through the whole row, so no single trip lines up with them.
- Both of site 0's leader segments were the slowest:
  - modelled p99 2,041 and 2,198;
  - round p99 1,537 and 1,973;
  - `failedCalls` 63 at the 23:09:53 `CommitErrors` trip.
- Site 1 (site b, 0 errors) led with p99 1,210 and 1,633. That leaves site 1 still over the 1 s fail line. So the
  errors are not the whole commit failure.
- That the voter's outbound calls and timers failed because they ran under the learner's context fits
  "fetch 26, deadline 37" to its peers, but it is not shown [inference].

### Proposed fix (not built)

- Key the feed's state by member, not by repository.
  - `FEEDS` becomes `BTreeMap<Box<str>, Rc<RefCell<Feed>>>`, keyed by `join.me.do_name`.
  - Its four callers have that name at hand: `Repo::stepped`, `load::finish_install`, `feed::route`, and `/status`.
  - A member's `fed` then wakes only its own polls, and each member keeps its own ring and `Following`.
- **Test, off-wasm, fails on main.** Two members of one repository, A a learner holding a poll and B a voter: B's
  `stepped` with an applied entry must not complete A's waiter, and A's `Following` must not take B's `gap`.
- **Check in workerd.** `wrangler dev` runs every object in one isolate, so a `tests/lookupbed`-style bed can
  reproduce the cross-object errors: a learner and a voter of one repository, held polls, and voter steps.
  Expected: errors on main, 0 with the fix.
- Extend `no_isolate_state_holds_a_future_or_waker` to `workers/site`. Fail on isolate-wide state that holds a
  `Sender`, `Waker` or future and is not keyed per object.
