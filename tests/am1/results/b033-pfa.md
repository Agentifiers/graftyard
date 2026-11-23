# b033-pfa: B0-33 at full load after `exact-prefix`, and what fills the leader's heavy turns

LM-1, 2026-11-23. One full-load B0-33 row on graft-m2b from main `a25edb28` (`exact-prefix` and `raft-split` merged),
under the standing queue and the $50 research budget, and a $0 join of the leader's Workers Logs to its trace on pfA,
blB and qlC. Tags follow `design/SHARED.md`, "Read this first".

**Short answer.**
- **pfA landed its leader on an ATL host and does not price `exact-prefix`** [measured]. ATL is A41's slow-host class,
  the same as ncA. The run was off-CPU-bound: 510 of 795 ms of leader excess a cycle was off CPU. It cut 47.8 agent
  commands/s against blB's 118.3, and its open draft PRs never passed 39, so the flip never got tested. The row fails
  B0-33's fail column on every line, as blA and blC2 did on IAD.
- **About 1% of the leader's invocations carry 65 to 88% of its CPU on every route, and that CPU is not theirs**
  [measured, with the attribution rule from workerd's source, [doc]].
  - workerd bills an actor's CPU to the request delivered most recently, until the next one is delivered.
  - So a 100 to 160 ms "learner `/raft`" or "`/commit`" invocation is billed for whatever turn ran while it was the
    newest request.
  - The heavy turns are **applying a committed entry: 44 to 48% of leader CPU, at 1.25 to 3.17 ms an event applied**.
    Next is **the deferred transaction (the batched SQL write), 22 to 29%, at 0.20 to 0.69 ms a statement**, then
    storage confirms at 3 to 11%.
  - The per-call work of the requests themselves (decode, step, answer) is the "no traced work" class: 15 to 18% of
    CPU, at 0.3 to 0.4 ms mean a call.
- **So the cut that pays is in apply and the transaction, not in any route.** Per-route CPU, the bed-vs-hosted
  comparison (`b033-profile.md`) and raftcalls.py's per-stage fit all read the billed request, not the work.

## The row

| Item | pfA |
|---|---|
| Build | site day from `a25edb28` (siteday exit 0); edge `e922c0b1` serves 100% (rev `a25edb288`) |
| Repo, a1 run | 131392, `mwsk8jzg`; 5,000 agents; learners `enam:2000,weur:1000,apac:1000`; flag off |
| Load (UTC) | 01:04:05.5 to 01:09:05.5; done line 01:11:51Z |
| Run cost | $1.10 (`costrow-pfA.status`); graft-m2b $1.11 for 2026-11-23 from 00:00Z |
| Placement | leader f0 ATL, near voter f1 AMS, far voter f2 HKG (DO analytics, `docolo.py`) |

Before the row, `reretire.sh` printed RERETIRE OK, with 266 networks. The sweep after it exited 0, and the watchdog
logged 0 edge 500s, exceptions or traps.

[measured]

| Line | pfA | blB (EWR, baseline) |
|---|---|---|
| Leader clock behind the near voter, p10/p50/p90 ms (`stale.py`) | 1,147 / 2,695 / 4,177 | |
| Cycle on the near voter's clock, mean (p50) ms (`second.py`) | 910 (755) | 491 p50 |
| Leader excess p50 / p90 ms | 643 / 1,530 | |
| Of the excess, leader on CPU / off CPU ms (`cycsplit.py`) | 286 / 510 | |
| Entries/s; commands an entry | 0.94 to 1.10; 58 | |
| Agent commands/s cut (armsum) | 47.8 | 118.3 |
| Agent-class commands answered ok/s (a1) | 39.5 | |
| Human commit stage p99, push log (n) | 9,394 (79) | 2,523 (119) |
| Merge commit stage p99, push log (n) | 7,413 (27) | 2,366 (36) |
| B0-29's sampler: bycut two rounds p99, entries.py round p50 | 5,637, 571 | |
| Merges over 2 s at the commit stage | 22 of 27 | 4 of 36 |
| Ticker stalls ≥ 500 ms (leader clock) | 87 | 0 |
| Voters' Reported rows in the load window | 4,718 (2,310 site 1, 2,408 site 2) | 5 |
| Leader changes | 0 term changes; site 0 lost and retook term 2 at 01:09:01 and 01:09:29 | 0 |
| Edge 503s | 6,754 | |
| `ERR_HTTP2_STREAM_ERROR` on agent pushes | 1,371, 01:08:07 to 01:09:16 | 88 |

- **The 503s** were "every member asked shed the read: overloaded" on task, GraphQL and REST writes. There were also
  999 `full` retry-afters on task writes [measured, `fivexxwhy.py`].
- **The h2 errors** are A42's shape. They all came in the run's last 70 s, after the leader's queue had grown. 1,370
  were agent pushes and 1 a merge.
- **Agent pushes:** 10,511 refused `full`, 190 ok. a1's draft-PR opener found no branch it could use on 721 of 866
  tries, because it opens only on a branch first pushed 10 s ago.

### CPU per command

[measured, `tests/leaderprof/hosted.py` and `cycsplit.py`, 90 s to the end of site 0's leadership]

| Run | Commands cut/s | Leader CPU ms/s | Workers Logs, ms per command | cycsplit (ring count), ms per command |
|---|---|---|---|---|
| pfA (ATL) | 47.8 | 275 | 5.75 | 4.96 (53.9 cut/s) |
| blB (EWR) | 118.3 | 533 | 4.50 | |
| qlC (EWR, quarter) | 41.8 | 206 | 4.94 | |

- The two pfA figures differ in the denominator: armsum counts agent commands cut, and cycsplit counts every command
  cut. The CPU is the same.
- Per command cut, pfA sits inside the baseline's 4.5 to 5.5 ms. This row cannot show whether `exact-prefix` moved
  it: its open-PR count stayed at 39, where the batched read never filled.

### `/commit` CPU per call against open draft PRs

a1's `openPrsAt` on pfA: 0 at 0 s, 18 at 30 s, 39 from 45 s to 300 s [measured].

| 30 s bucket from load start | 0 | 30 | 60 | 90 | 120 | 150 | 180 | 210 | 240 | 270 |
|---|---|---|---|---|---|---|---|---|---|---|
| pfA `/commit` ms a call | 1.65 | 1.16 | 1.25 | 1.11 | 1.50 | 1.62 | 1.75 | 1.54 | 2.71 | 9.20 |
| blB `/commit` ms a call | 1.08 | 1.40 | 1.30 | 1.47 | 1.53 | 1.50 | 1.47 | 2.33 | 2.24 | 1.90 |

pfA's last two buckets are the drain after the leader dropped (01:09:01), with few calls left. Before that it held 1.1
to 1.75 ms with 39 PRs open. That is no evidence either way on the flip, which needs about 256 open PRs or more.

## What fills the leader's heavy turns

### Who is billed

- **The rule** [doc]. workerd's `IoContext::IncomingRequest::delivered` (`src/workerd/io/io-context.c++`) reports
  "accumulated CPU time" to the old front request and makes the new request the front. An actor's CPU is therefore
  billed to the request delivered most recently, until the next delivery.
- **What that means here** [inference from the rule]. A request's `cpuTimeMs` in Workers Logs is the CPU of every turn
  that ran from its delivery to the next delivery. That includes applying entries, the deferred transaction, the
  ticker and held-output release, whichever request they belong to.
- **What it looks like** [measured, pfA, blB, qlC, 90 s to the end]:
  - Invocations of 20 ms or more are 0.94% (pfA), 1.75% (blB) and 1.60% (qlC) of the leader's invocations. They carry
    72%, 80% and 31% of its CPU.
  - Every route has them, at the same means: 99 to 159 ms on pfA, 33 to 123 ms on blB, 29 to 48 ms on qlC.
  - raftcalls.py's kind-33 rows show heavy learner calls the same as light ones in every stage on pfA. Body was 26
    against 24 B, asks 0.8 each, held 0.1 each, answer 226 against 2,550 B. Its linear fit is therefore singular
    without the constant envelope count, and meaningless with it (intercept 16.9 ms, negative stage coefficients).

### The join

- **How it works** (`tests/fanout/heavyturns.py`) [measured]. A test install's handler writes `Entered` (kind 25) as its
  first act. So in the trace's ring order, the rows between one `/raft` call's `Entered` and the next `Entered` are the
  work its billed CPU paid for. No clock is read; the leader's clock ran 1.4 s behind Workers Logs' at p50 on pfA.
- **The sample.** Every leader `/raft` call joined exactly to Workers Logs on (caller, `x-graft-call`): 4,586 on pfA,
  12,807 on blB and 11,108 on qlC. Learner calls are about 32 to 59 a second, so these slices sample the leader's turns
  evenly through the run.

| Slice holds | pfA: share of CPU, mean ms | blB | qlC |
|---|---|---|---|
| Applying committed entries | 48%, 155.5 | 44%, 104.6 | 43%, 16.3 |
| Deferred transaction (writes, no apply) | 29%, 65.7 | 27%, 38.7 | 22%, 7.3 |
| No traced work (the request's own handling) | 18%, 0.4 | 16%, 0.3 | 15%, 0.3 |
| Storage confirm only | 3%, 10.1 | 10%, 14.0 | 11%, 3.6 |
| Cuts or sends only | 2%, 1.8 | 3%, 0.8 | 9%, 1.0 |

- **Apply, per entry** [measured]:
  - pfA: 49 events, 175 statements and 18 KB queued, 54 resolutions, 155.5 ms. That is 3.17 ms an event.
  - blB: 66 events, 217 statements and 95 KB, 58 resolutions, 104.6 ms. That is 1.59 ms an event.
  - qlC: 13 events, 49 statements and 55 KB, 10 resolutions, 16.3 ms. That is 1.25 ms an event.
- **Turn length tracks the entry's size,** loosely. Across apply slices, CPU against events applied gives r 0.50, 0.52
  and 0.35, and against statements queued r 0.57, 0.45 and 0.30. That is not a fixed cost a turn: a turn of 13 events
  costs 16 ms, and one of 49 to 66 costs 105 to 155. The per-event cost is twice as high on the ATL host as on EWR.
- **The deferred transaction** [measured]. Slices that ran one (a Flushed row) held 71 (pfA), 128 (blB) and 21 (qlC)
  statements, at 48.9, 25.1 and 4.5 ms: 0.69, 0.20 and 0.22 ms a statement. Some write-only slices have no Flushed row
  in them. Those cost more (100, 80 and 15 ms), which fits a transaction that ran inside another input's turn.
- **What this leaves for the request routes:** about 0.3 to 0.4 ms of CPU a call, in line with the local bed's learner
  call (`b033-profile.md`).

### What it changes

[inference]
- **Where a cut pays.** At full load on a good host, applying entries and the deferred transaction are about 70% of
  the leader's CPU. That is about 3.1 of blB's 4.5 ms per command, and it scales with events applied. A cut there pays
  at every load. A cut to learner `/raft` or `/outcome` handling pays at most the "no traced work" share, 15 to 18%.
- **The bed.** The bed prices apply and the transaction directly (`fold_apply`, `sql_tx`, `log_row`). Its shortfall
  against hosted (20 to 46%) is the measurement's attribution, not missing stages. Hosted apply at 1.25 to 3.17 ms an
  event is 10 to 30 times the bed's (0.11 ms a command at N=60), so the next local question is what apply does hosted
  that the bed's does not [unverified].
- **The slow host.** An event costs twice the CPU on ATL as on EWR at the same entry shapes. A41 is about off-CPU
  time; this is the on-CPU half of the same host gap.

## Raws and tools

- `tests/fanout/raw/` holds pfA's files:
  - `costrow-pfA.status`, `rows-pfA.status`, `terms-pfA.jsonl` and `tsave-pfA.jsonl.xz`;
  - `memguard-pfA.{err,jsonl.xz}`, `arrivals-pfA.jsonl` and `shardgate-pfA-b0-33-{0,1,2}.jsonl`;
  - `siteday-pf.status` (account redacted) and `reretire-graft-m2b-pfA.log`;
  - `wl-pf/pfA-f{0,1,2}.json.xz`: the three voters' Workers Logs, 01:03:00 to 01:09:10Z, from `wlobj.py`.
- The a1 raws stay local, as before (`tests/am1/raw/` is git-ignored): `a1-b0-33-mwsk8jzg.jsonl`, its push log, and
  `runs/a1-b0-33-fpfA-b033.log`.
- `tests/fanout/heavyturns.py` (new) is the join above. `tests/fanout/b033probes/host/common.py` gains pfA, and
  `tests/leaderprof/hosted.py` gains pfA's agent commands cut.
- `tests/am1/rest/a1.mjs` gains `--seed-prs N`, for the next row, which tests `exact-prefix`'s flip:
  - it opens N draft PRs before the window, on branches no agent pushes (`claude/a1-<run>-<i>s`);
  - those branches sort among the agents' own, so each agent push's `pull_by_head` read starts among them;
  - the agents' branches and their push shape are unchanged, and `openPrsAt` counts the seeded PRs as `seededPrs`.
