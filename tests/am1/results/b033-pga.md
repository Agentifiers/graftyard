# b033-pga: B0-33 at full load with 1,000 open draft PRs, and `exact-prefix`'s flip

LM-1, 2026-11-23. The second full-load B0-33 row after `exact-prefix`, under the standing queue and the $50 research
budget, cleared by the lead after pfA (`tests/am1/results/b033-pfa.md`). It differs from pfA in two ways:
- **Open PRs:** a1 seeds 1,000 open draft PRs before load (`--seed-prs 1000`), so `exact-prefix`'s flip is tested.
- **Placement guard:** a leader placed in ATL or IAD before load is re-placed on a fresh repository, at most twice.

Tags follow `design/SHARED.md`, "Read this first".

**Short answer.**
- **`exact-prefix`'s first flip is not met** [measured].
  - With 1,000 to 1,310 draft PRs open, the leader's `/commit` CPU a call stayed at pfA's and blB's level, 1.36 to
    1.48 ms over the first 120 s.
  - It did not climb with the count. Its one rise came during the leader's stall and drop, not with the PRs.
  - `decisions/DECISIONS.md`, `exact-prefix`, Evidence, now cites this row.
- **The row fails B0-33's fail column on every line, as pfA did** [measured].
  - The guard read the leader in ORD, which is not in ATL or IAD, so it did not re-place.
  - A41 already lists ORD (crA) among the slower hosts, and this run behaved like one:
    - leader excess p50 536 ms against blB's 373;
    - 6.04 ms of leader CPU per agent command cut against the baseline's 4.5 to 5.5;
    - 2.75 ms of CPU per event applied against blB's 1.59.
- **What fills the leader's heavy turns is the same as on pfA, blB and qlC** [measured, `heavyturns.py`]. Applying
  committed entries is 50% of the joined CPU and the deferred transaction 25%.
- **So the limit is per-event apply CPU plus off-CPU time on a slow host**, the same cause as pfA. The lead has ruled no
  third hosted B0-33 row until that has an answer. The next step is the $0 local apply question.

## The row

| Item | pgA |
|---|---|
| Build | no site day: site `a25edb28` (pfA's site day); edge `e922c0b1` serves 100% (rev `a25edb288`) |
| Wrapper | checkout `2209b2bcb`, the same tree as merged main `3f1f1ccb` for every file the row runs |
| Repo, a1 run | 131393, `mwsmtf48`; 5,000 agents; learners `enam:2000,weur:1000,apac:1000`; flag off; `--seed-prs 1000` |
| Seed | 1,000 of 1,000 draft PRs opened, 02:16:12 to 02:20:14Z, 0 failures |
| Load (UTC) | 02:20:20.1 to 02:25:20Z; done line 02:28:08Z |
| Run cost | $1.90 (`costrow-pgA.status`), setup, seed and sweep included |
| Placement | leader f0 ORD (seen 02:14:48Z, before load), near voter f1 CDG, far voter f2 SIN; 0 re-places |

- **The site lacks two later changes.** Main has two site-script changes since `a25edb28`:
  - maint-yield (`workers/site/src/shard.rs`, `maint_hold`);
  - AM2-WP1 (`workers/site/src/family.rs`, `TOKENS_FORMAT_V4` and `IDENTITY_FORMAT_V4`, and Identity's `out_json`).
  The lead kept the site at `a25edb28` so the row compares with pfA. Neither change is on the repository leader's
  path.
- **Before and after the row.** `reretire.sh` printed RERETIRE OK with 266 networks before it, and the sweep after it
  exited 0 with no shard container left. TORNALARM OK.

[measured]

| Line | pgA (ORD) | pfA (ATL) | blB (EWR, baseline) |
|---|---|---|---|
| Leader clock behind the near voter, p10/p50/p90 ms (`stale.py`) | 115 / 1,459 / 3,319 | 1,147 / 2,695 / 4,177 | |
| Cycle on the near voter's clock, mean (p50) ms (`second.py`) | 865 (707) | 910 (755) | 514 (491) |
| Leader excess p50 / p90 ms | 536 / 1,284 | 643 / 1,530 | 373 / 661 |
| Of the excess, leader on CPU / off CPU ms (`cycsplit.py`, 90 to 229 s) | 315 / 294 | 286 / 510 | 273 / 128 |
| Entries/s; commands an entry (`cycsplit.py`) | 1.12; 48 | 0.94 to 1.10; 58 | 1.95 |
| Agent commands/s cut (armsum) | 60.0 | 47.8 | 118.3 |
| Agent-class commands answered ok/s (a1) | 55.1 | 39.5 | |
| Human commit stage p99, push log (n) | 11,018 (85) | 9,394 (79) | 2,523 (119) |
| Merge commit stage p99, push log (n) | 4,794 (34) | 7,413 (27) | 2,366 (36) |
| B0-29's sampler: bycut two rounds p99, entries.py round p50 | 5,276, 368 | 5,637, 571 | |
| Merges over 2 s at the commit stage | 21 of 34 | 22 of 27 | 4 of 36 |
| Ticker stalls ≥ 500 ms (leader clock) | 28 + 10 (two leaderships) | 87 | 0 |
| Voters' Reported rows in the load window | 1,706 (878 site 1, 827 site 2, 1 site 0) | 4,718 | 5 |
| Leader changes | site 0 lost and retook term 2 at 02:24:09 and 02:24:14; site 1 took term 3 at 02:25:48, after load | 0 term changes | 0 |
| Edge 503s | 5,205 | 6,754 | |
| `ERR_HTTP2_STREAM_ERROR` on agent pushes | 2,251 | 1,371 | 88 |

- **The 503s** [measured, `rows-pgA.status`]:
  - 2,760 were "every member asked shed the read: overloaded", on GraphQL (1,088), task (927) and REST writes (745).
  - 660 were `full` retry-afters on task writes.
  - The other 1,785 had other reasons, spread across renew (922 503s in all), lease (299), GraphQL and REST calls.
    They are not split further here.
- **Agent pushes:** 2,740 ok, 7,805 refused `full`, and 2,479 lost to `fetch failed`, which is A42's h2 shape.
- **The leader's two leaderships.** Site 0 led term 2 from 02:13:34. Its inputs fell from about 300 a second to 25 to
  57 at 150 to 195 s into load (`curve.py`), and it lost the lead at 02:24:09 (229 s), then took it back at 02:24:14.
  After the load window it lost it again at 02:25:47, and site 1 took term 3. The verdict windows use 90 to 229 s, the
  first leadership's steady part.

### CPU per command

[measured, `tests/leaderprof/hosted.py` and `cycsplit.py`, 90 s to the end of site 0's first leadership]

| Run | Commands cut/s | Leader CPU ms/s | Workers Logs, ms per command | cycsplit (ring count), ms per command |
|---|---|---|---|---|
| pgA (ORD) | 60.0 | 362 | 6.04 | 6.58 (53.4 cut/s) |
| pfA (ATL) | 47.8 | 275 | 5.75 | 4.96 (53.9 cut/s) |
| blB (EWR) | 118.3 | 533 | 4.50 | |
| qlC (EWR, quarter) | 41.8 | 206 | 4.94 | |

- The two pgA figures differ in window and denominator. hosted.py takes 90 s to the end of the trace over agent
  commands cut, and cycsplit takes 90 to 229 s over every command cut.
- **Per route** (hosted.py; the billing caveat below applies): `/commit` 1.82 ms a call (2.69 ms per command), learner
  `/raft` 2.16 (0.98), `/entries` 5.33 (0.79), `/outcome_by_request` 1.28, `/outcome` 0.49, `/read` 4.17.

### `/commit` CPU per call against open draft PRs

a1's `openPrsAt` on pgA, seeded 1,000 included: 1,000 at 0 s, 1,099 at 60 s, 1,193 at 120 s, 1,214 at 180 s, 1,263 at
240 s, 1,310 at 300 s [measured].

| 30 s bucket from load start | 0 | 30 | 60 | 90 | 120 | 150 | 180 | 210 | 240 | 270 | 300 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| open draft PRs at the bucket's start | 1,000 | 1,014 | 1,099 | 1,147 | 1,193 | 1,208 | 1,214 | 1,231 | 1,263 | 1,309 | 1,310 |
| pgA `/commit` ms a call | 1.47 | 1.48 | 1.36 | 1.36 | 2.05 | 3.17 | 2.86 | 1.98 | 1.66 | 1.45 | 1.78 |
| pgA leader CPU ms/s | 142 | 395 | 419 | 371 | 468 | 230 | 252 | 478 | 425 | 313 | 108 |
| pfA `/commit` ms a call (39 open) | 1.65 | 1.16 | 1.25 | 1.11 | 1.50 | 1.62 | 1.75 | 1.54 | 2.71 | 9.20 | |
| blB `/commit` ms a call | 1.08 | 1.40 | 1.30 | 1.47 | 1.53 | 1.50 | 1.47 | 2.33 | 2.24 | 1.90 | |

- **Flat with the count** [measured]. Over 0 to 120 s, 1,000 to 1,193 open PRs, `/commit` held at 1.36 to 1.48 ms a
  call. That is pfA's level with 39 open and blB's with far fewer than 256. Before `exact-prefix`, the local bed put
  one `pulls_from` at 1.31 ms with 991 open, against 0.11 with 91, and four calls a push
  (`tests/am1/results/b033-profile.md`). A cost of that size would show here.
- **The rise is the stall** [inference]. 150 to 210 s is the leader's stall and drop: inputs fell to 25 to 57 a second,
  and leader CPU fell to 230 to 252 ms/s. workerd bills a turn to the newest request (`b033-pfa.md`, "Who is billed"),
  so with fewer `/commit` calls the same apply turns were billed over fewer of them. At 240 to 300 s, with 1,263 to
  1,310 open, it was back to 1.45 to 1.78 ms.
- **Verdict.** `exact-prefix`'s first flip ("`/commit` CPU per call still rises through the run as it did (1.1 to 2.3
  ms) with open pull requests counted rising") is not met. `decisions/DECISIONS.md`, `exact-prefix`, Evidence, now
  cites this row.

## What fills the leader's heavy turns

The same join as pfA (`tests/fanout/heavyturns.py`, `b033-pfa.md`, "The join") [measured, pgA, 90 to 229 s].
- Every leader `/raft` call was joined exactly to Workers Logs on (caller, `x-graft-call`): 3,882 slices.
- The 63 slices of 20 ms or more carry 80% of the joined CPU.

| Slice holds | pgA: share of CPU, mean ms | pfA | blB | qlC |
|---|---|---|---|---|
| Applying committed entries | 50%, 153.4 | 48%, 155.5 | 44%, 104.6 | 43%, 16.3 |
| Deferred transaction (writes, no apply) | 25%, 45.5 | 29%, 65.7 | 27%, 38.7 | 22%, 7.3 |
| No traced work | 17%, 0.4 | 18%, 0.4 | 16%, 0.3 | 15%, 0.3 |
| Cuts or sends only | 5%, 2.2 | 2%, 1.8 | 3%, 0.8 | 9%, 1.0 |
| Storage confirm only | 4%, 8.6 | 3%, 10.1 | 10%, 14.0 | 11%, 3.6 |

- **Apply, per entry:** 56 events, 165 statements and 84 KB queued, 46 resolutions, 153.4 ms. That is **2.75 ms an
  event**, between blB's 1.59 (EWR) and pfA's 3.17 (ATL).
- **The deferred transaction:** the 40 slices that ran one held 86 statements, at 27.9 ms, or 0.33 ms a statement
  (blB 0.20, pfA 0.69).
- **What apply-slice CPU tracks:** KB queued at r 0.80 here, events at r 0.40. On pfA, statements queued tracked best
  (r 0.57) and KB not at all (r 0.01). So no one size measure predicts a turn on every run.

Two caveats on this join, from the lead's review of pfA. They apply to pfA's table too.
- **"No traced work" is an upper bound** on a route's own handling. It holds whatever work in the slice wrote no traced
  row, as well as the request's own decode, step and answer.
- **The slices are only the turns that follow a `/raft` call.** Treating them as representative of the leader's other
  turns, after `/commit`, `/outcome` and the ticker, is an assumption. Learner calls arrive 32 to 59 a second through
  the run, so they sample it evenly in time, but a turn's content may depend on what started it [unverified].

## What it changes

[inference]
- **`exact-prefix` holds at 1,000+ open PRs.** The open-PR read no longer adds CPU per command as PRs accumulate.
- **Per-command CPU follows the host.** pgA's 6.04 ms per command and 2.75 ms per event both sit between EWR's and
  ATL's. The guard by colo let ORD through, though A41 already lists ORD among the slower hosts. A guard for a later
  row has to judge the host by what it measures in setup, not by its colo name (the lead's ruling, 2026-11-23).
- **Two fails in a row on one cause.** pfA and pgA are both limited by per-event apply CPU plus off-CPU time on a slow
  leader host. The next step is the $0 question of why hosted apply costs 1.25 to 3.17 ms an event, against the bed's
  0.11 ms a command.

## The host guard

The lead's ruling after pgA: judge the leader's host by what it measures during setup, not by its colo name, with a
line derived from blB and qlC, written down before the next row. This section is that line. It costs $0: it reads
Workers Logs that every row already writes.

**The probe.**
- a1's setup creates 100 tasks, one `/commit` each, at the same pace on every run: 44 to 48 in the first 60 s.
- The leader object (`#1#f0`) does the same work for each, so its Workers Logs CPU on every route, over the first
  60 s from its first `/commit`, divided by the `/commit` calls in that window, prices the host on a fixed workload.
- It needs no clock but Cloudflare's, no trace, and nothing from the load. `tests/fanout/hostguard.py` reads it with
  two Workers Logs calculations.

**What it reads on the twelve B0-33 runs** [measured, `hostguard.py` replayed on each run's setup]:

| Run | Leader colo | ms of leader CPU a setup commit | Load: ms an event applied (`heavyturns.py`) | Load: leader excess p50 ms (`second.py`) |
|---|---|---|---|---|
| qlC | EWR | 21.8 | 1.25 | |
| qlB | MIA | 21.9 | 1.15 | |
| qlA | MIA | 23.0 | 1.23 | |
| sabA | EWR | 23.7 | | 360 |
| blB | EWR | 23.7 | 1.59 | 373 |
| crA | ORD | 26.2 | | 427 |
| blA | IAD | 31.0 | 2.43 | 480 |
| chA | IAD | 31.2 | | 446 to 480 (A41) |
| blC2 | IAD | 31.5 | 3.50 | 446 |
| ncA | ATL | 39.7 | | 703 |
| pgA | ORD | 43.2 | 2.75 | 536 |
| pfA | ATL | 46.1 | 3.17 | 643 |

- **The runs fall into two groups with a gap between them**: 21.8 to 26.2 ms and 31.0 to 46.1 ms. The setup reading
  orders the runs the way their loaded apply cost does, wherever both were measured. Every A41 slow-host run is in the
  upper group.
- **The colo name does not decide it.** Two ORD leaders read 26.2 (crA) and 43.2 (pgA). The same colo can be either
  host [measured]. That is why the colo guard let pgA through.
- **The 60 s window is stable.** On each run the reading at 30, 60 and 90 s agreed within about 3 ms (for example
  pfA 44.1, 45.6, 44.6 and blB 24.6, 23.6, 23.7), so the verdict is ready about 60 s into setup, minutes before load.

**The line** [inference: a ratio chosen from these runs, not a measured threshold]:
- **Re-place when the setup reading is over 28.4 ms a commit.** That is 1.2 times the higher baseline reading, blB's
  23.7 (qlC's is 21.8).
- It keeps sabA, crA, blB, qlA, qlB and qlC, and re-places chA, blA, blC2, ncA, pfA and pgA.
- crA sits under the line at 26.2, with load excess 427 ms against EWR's 360 to 373. The line keeps a host about 15%
  slower than EWR and refuses one 30% slower. A tighter line would refuse crA as well.
- The ticker-stall half of the ruling is not used. In every run's last minute before load the leader had no ticker wake
  of 200 ms or more: the host gap shows only under load, and stalls are a leader-clock reading (A41's correction).
  The setup's CPU per commit separates the hosts without them.

**In the wrapper.** `tests/fanout/guardrow.sh` is pgA's set wrapper with the colo test replaced by `hostguard.py`.
- a1 runs with `--hold-file`: after the learner gate it waits, before load, until the guard writes the file.
- Every 20 s the wrapper asks the guard. Exit 2 (not judged yet) keeps asking while a1 holds.
- Exit 0 writes `go` and the load starts. Exit 3 writes `stop`: a1 exits 4 before load, and the wrapper re-places
  on a fresh repository, at most twice.
- **It fails closed.** With no verdict, a1 exits 4 before load after 300 s, and that counts as a re-place.
- If the third placement is also re-placed, it stops the set and the sweep runs.
- It logs each verdict with its number, and the row's results must quote it.

**Fix, 2026-11-23, after the lead's live replay.** The first version found the first `/commit` the leader object ever
served. On pgA that was one of the repository's own two creation commits at 02:13:28, a minute before the task
creates began at 02:14:22. So its 60 s window held 6 commits, and the run read "unknown". The table above came from a
replay that started its search later, which is why it did not show this. The fix:
- The guard now anchors on the first of 5 commits each within 5 s of the next, which is the task phase's pace (1.2 to
  1.6 s apart on every run).
- It judges a window only once it ended 30 s ago, so Workers Logs' ingestion lag cannot read short.
- Live, from the rows log's start time and with no end time, it reproduces the table on pgA (43.2 ms, 45 commits,
  re-place), pfA (46.1, 45, re-place) and blB (23.7, 46, keep) [measured].
- Replayed on pgA with an end time from 02:14:30 to 02:15:30 it says unknown; from 02:15:55 it says re-place.
- On every run of the table the verdict is ready 21 to 41 s before a1 logged its load start (pgA, with its 1,000 seeded
  PRs, 264 s). With the hold, a late verdict delays load rather than letting it start.

The setup extracts behind the table are `tests/fanout/raw/wl-setup/<tag>-setup.json.xz`: each run's leader object
from a1's log start to load start, from `wlobj.py fetch`.

## Raws and tools

- `tests/fanout/raw/` holds pgA's files:
  - `costrow-pgA.status`, `rows-pgA.status`, `terms-pgA.jsonl` and `tsave-pgA.jsonl.xz`;
  - `memguard-pgA.{err,jsonl.xz}`, `arrivals-pgA.jsonl` and `shardgate-pgA-b0-33-{0,1,2}.jsonl`;
  - `reretire-graft-m2b-pgA.log`;
  - `wl-pg/pgA-f{0,1,2}.json.xz`: the three voters' Workers Logs, 02:19:20 to 02:25:30Z, from `wlobj.py`.
- **The a1 raws stay local, as before** (`tests/am1/raw/` is git-ignored): `a1-b0-33-mwsmtf48.jsonl` with its
  `openPrsAt` and `seededPrs`, its push log, and `runs/a1-b0-33-fpgA-b033.log`.
- `tests/fanout/b033probes/host/common.py` gains pgA, `tests/leaderprof/hosted.py` gains pgA's agent commands cut, and
  `tests/fanout/heavyturns.py` gains the `wl-pg` directory.
