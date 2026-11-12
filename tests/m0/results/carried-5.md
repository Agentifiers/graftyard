# B0-02: the leader's confirm and the commit rule (2026-11-12)

The operator's decision on `carried-4.md`: don't rerun for a quieter window and don't accept slow-confirm days as F35.
In order: rule out `m0-carried-4`'s streamed bodies (`f33a516`) with a same-window A/B and a bare-object control;
then quantify, offline, what standard Raft's commit rule (any majority durable, the leader's copy optional; Raft
dissertation 10.2.1) would have done for every pass-1 and pass-3 miss; build that rule only if it brings every miss
under 600 ms. Gate unchanged: 8 of 8 at 300 and 8 of 8 at 330 offered. Branch `m0-carried-5`, from `main` with
`m0-carried-4` merged.

## The majority rule, offline

`tests/m0carried/majority.py` rebuilds three times per entry from each run's leader trace (test-faults kinds 10, 12
to 16): L, the end of the first storage confirm whose transaction began at or after the input that appended the entry;
N and F, the first ok AppendEntries answer from the near and far follower whose call carried it. A follower answers
only after its own confirm, so N and F are durable acks. Today's rule commits at max(L, min(N, F)); the majority rule
at the second of the three. The model's commit time matches the leader's `Applied` trace to the millisecond for 90%
of entries in every run but pass 3's r300-2 (a step-down mid-run). Commands are then replayed from their admission
times through one-entry-in-flight rounds with each round's measured durations, under each rule.

| Run | Commit p99 | Replayed, today's rule | Replayed, majority rule | Leader confirm p99 | Near ack p50 | Far ack p50 | Entries gated by the leader's confirm | Entries the majority rule commits earlier |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| p1 r300-1 | 716 | 716 | 716 | 132 | 157 | 547 | 1 of 333 | 0 |
| p1 r300-2 | 494 | 489 | 489 | 54 | 167 | 665 | 0 of 352 | 0 |
| p1 r300-3 | 738 | 575 | 575 | 300 | 157 | 607 | 1 of 354 | 0 |
| p1 r300-4 | 514 | 513 | 513 | 92 | 165 | 388 | 1 of 350 | 0 |
| p1 r300-5 | 385 | 365 | 365 | 81 | 145 | 653 | 0 of 410 | 0 |
| p1 r300-6 | 574 | 570 | 570 | 100 | 180 | 670 | 0 of 320 | 0 |
| p1 r300-7 | 588 | 584 | 584 | 133 | 180 | 532 | 0 of 315 | 0 |
| p1 r300-8 | 342 | 326 | 326 | 64 | 141 | 389 | 0 of 422 | 0 |
| p1 r330-1 | 481 | 478 | 478 | 43 | 155 | 424 | 0 of 376 | 0 |
| p1 r330-2 | 426 | 314 | 314 | 87 | 118 | 773 | 1 of 503 | 0 |
| p1 r330-3 | 469 | 469 | 469 | 79 | 150 | 552 | 0 of 385 | 0 |
| p1 r330-4 | 462 | 460 | 460 | 90 | 146 | 454 | 0 of 404 | 0 |
| p1 r330-5 | 679 | 691 | 691 | 141 | 128 | 734 | 3 of 453 | 0 |
| p1 r330-6 | 614 | 561 | 561 | 155 | 178 | 478 | 0 of 322 | 0 |
| p1 r330-7 | 583 | 570 | 570 | 103 | 141 | 518 | 1 of 409 | 0 |
| p1 r330-8 | 692 | 641 | 641 | 300 | 168 | 669 | 1 of 320 | 0 |
| p3 r300-1 | 387 | 381 | 381 | 35 | 147 | 620 | 0 of 406 | 0 |
| p3 r300-2 | 664 (1,000 on the old leader) | 1,276 | 1,276 | 744 | 275 | 787 | 7 of 123 | 0 |
| p3 r330-1 | 506 | 492 | 492 | 89 | 163 | 395 | 0 of 355 | 0 |
| p3 r330-2 | 431 | 419 | 419 | 45 | 146 | 388 | 0 of 406 | 0 |

Times are ms. Pass 3's r300-2 is read from the old leader, which committed 11,079 of its commands before it stepped
down; its replay is off because the trace misses the step-down.

**(c) does not close any miss: 0 of 7,318 entries would commit earlier.** The leader's own confirm was the last of
the quorum's acks for 16 entries in all 20 runs. Under the majority rule each of those 16 waits instead for the far
follower, whose answer came later still (far ack p50 388 to 787 ms against a leader confirm p99 of 35 to 744 ms). With
three voters and the far follower about 190 ms away, the leader's confirm can only gate a round when it outlasts the
near follower's whole round trip, and then the far follower is slower again. So on the acks as measured, building the
rule and its reset-leader fix (a reset leader that catches up or resumes as a candidate) would move nothing on this
gate, and per the operator's step 3 it is not built. The A/B below finds the measured acks are themselves held by the
leader's slow confirm, which limits this replay ("What every miss in the A/B is").

**The misses are the near follower's round trip.** The table's round tail is the near ack, not the leader's confirm.
`carried-4.md` read commit p99's 0.91 correlation with the leader's confirm p99 as cause. The trace shows the two move
together because they share a driver: in pass 1 a near call's p99 rose with the leader's confirm in the same turn
(correlation 0.13 to 0.66 per run), and in 61 of the 75 near calls over 300 ms the leader's same-turn confirm had
ended at least 100 ms before the call did, so the round waited on the call, not the confirm. Splitting the near call with the
follower's `/raft` arrival rows (`Stats::raft_in`, follower clock, aligned on each run's 2nd-percentile offset):

| Leg of a near AppendEntries, pass 1 | p50 | p99 | Correlation with call time | Correlation with the leader's same-turn confirm |
| --- | --- | --- | --- | --- |
| Request start: fetch issued to the follower's handler | 131 to 343 | 205 to 711 | 0.33 to 0.63 | 0.16 to 0.57 |
| Body arrival at the follower | 14 to 29 | 78 to 357 | 0.41 to 0.93 | -0.14 to 0.32 |
| Follower work and response leg | — | — | negative | negative |

The request leg correlates with the leader's write confirming at the same time; the body leg does not. That is what a
streamed body sent during a SQLite write would do if the stream, unlike the array, is held with it, and why step 1 runs
before anything else [inference; the A/B below shows the array is held the same way]. Comparing near-call tails across the two code versions: carried-3 (array bodies, three
runs with stats) had 0 to 5 near calls over p50 + 100 ms per run; pass 1 (streamed) had 0 to 31, median 8.

A second install points the same way (LM-1, `graft-m2b`, B0-29's smoke at 500 agents, 8 learners, k 4, 180 s; one
run each side, hours apart, so not an A/B): before `m0-carried-4` (smoke m16c, `repo:131282`, 15:17Z) the leader's
confirm was p50 30 / p99 89 ms (n 634) and its round p99 316 ms; after (site day m17 from `lm1-memory` `c756014d`,
which holds `main` `4265f3d`; smoke m17, `repo:131284`, 18:07Z) p50 37 / p99 121 ms (n 673) and round p99 507 ms.

## Same-window A/B: streamed vs array bodies

`m0-carried-5` adds a test-installs switch, `Host::array_body`: `/stats?body=array` (or `stream`) on each member sets
how its calls send their body, as `m0-carried-3` did (one `Uint8Array`) or as `f33a516` does (a `FixedLengthStream`).
`tests/m0carried/ab5.sh` ran 16 one-minute runs at 330 offered on one group, `repo:6716000` (leader enam, near
follower 91 ms, far 215 ms; placed on the 6th try), alternating stream and array run by run, 19:09 to 19:28Z, with the
bare-object control running alongside. Site day 19:08Z from `961d518` (main `4265f3d` plus the switch), `test-faults`,
image `sha256:7e0c4772…`; sites a/b/c `06e46a56`/`96993e4e`/`95559ea8`, test edge `a8e78d40`. Every run carried
306 to 308/s with 0 failed commands, and every `/stats` reported the body mode asked for.

| Run | Body | Commit p99 | Leader confirm p99 | Near call p99 |
| --- | --- | --- | --- | --- |
| ab01 | stream | **524** | 133 | 312 |
| ab02 | array | **438** | 59 | 226 |
| ab03 | stream | 911 | 281 | 275 |
| ab04 | array | 661 | 103 | 219 |
| ab05 | stream | 700 | 87 | 242 |
| ab06 | array | **388** | 80 | 214 |
| ab07 | stream | **511** | 80 | 278 |
| ab08 | array | **409** | 49 | 210 |
| ab09 | stream | **400** | 103 | 215 |
| ab10 | array | **584** | 95 | 229 |
| ab11 | stream | **467** | 103 | 256 |
| ab12 | array | **585** | 218 | 255 |
| ab13 | stream | **361** | 96 | 205 |
| ab14 | array | 697 | 99 | 261 |
| ab15 | stream | 767 | 139 | 281 |
| ab16 | array | 659 | 85 | 236 |

| Body | Pass | Commit p99 median (range) | Leader confirm p99 median | Near call p99 median | Near calls over p50 + 100 ms, median per run |
| --- | --- | --- | --- | --- | --- |
| stream | 5 of 8 | 518 (361 to 911) | 103 | 266 | 5 |
| array | 5 of 8 | 585 (388 to 697) | 90 | 228 | 3.5 |

**The stream is not the cause.** Both modes pass 5 of 8 in the same window on the same group; the array's commit p99
median is higher, its confirm and near-call tails slightly lower, all within run-to-run spread. `f33a516` stays: it
removes the large-body stall `carried-4.md` measured, and costs nothing measurable here. The c3 to p1 rise is not
explained by code; `carried-4.md`'s "platform drift" reading stands.

**The control saw no slow confirm.** `graft-m4-gateprobe` (modes `nogate`, `stream`, `nowrite`, 60 KB bodies, enam to
weur) ran 6,648 rounds alongside: `sync()` p50 21 ms, p99 34 to 36 ms, max 162 ms, none over 300; calls p99 150 to 153
ms; no body over 150 ms. Further shapes at 19:44 to 19:49Z (250 statements per transaction, 600 or 3,000 rows kept, 250
KB and 1 MB rows): `sync()` p99 47 to 109 ms, one 915 ms confirm in 1,400 rounds. The members' confirms in the same
window reached 400 to 892 ms in 10 of 16 runs.

**The majority rule again moves nothing:** `majority.py` on the 16 A/B runs, 0 of 6,709 entries commit earlier.

## What every miss in the A/B is

Every command over 600 ms in the 16 runs sits in one of 2 to 6 windows per run, each the same shape (`ab03` entry
1031, `ab04` 1347, `ab14` 5664, `ab16` 6347 and 6707, `ab15` 6139, `ab05` 1681):

1. The leader appends entry k, sends its AppendEntries in the same turn, then runs the entry's transaction one
   macrotask later (the trace's `queued`, `issued`, and `flushed` all at +0 ms).
2. That transaction's confirm takes 300 to 840 ms (the run's others: p50 about 27 ms).
3. **The AppendEntries reaches the near follower only when the confirm ends.** Aligning each `/raft` arrival on the
   follower (`raftIn`) with the leader's issue time, after removing the clock offset: for the leader's sends issued in
   the same turn as a transaction whose confirm took 150 ms or more, 37 of 40 in the A/B and 45 of 47 in `carried-4`'s
   pass 1 reached the follower no earlier than 30 ms before that confirm ended. By the same-turn confirm's length, the
   request's arrival against the run's median was:

   | Same-turn leader confirm | Sends (A/B, both followers) | Arrival later than median, p50 / p90 |
   | --- | --- | --- |
   | under 60 ms | 4,969 | -2 / 69 ms |
   | 60 to 150 ms | 126 | 53 to 112 / 198 to 227 ms |
   | 150 to 300 ms | 35 | 157 to 160 / 181 to 218 ms |
   | over 300 ms | 23 | 300 to 303 / 432 to 459 ms |

   Body transfer and the follower's own work and confirm (10 to 26 ms) stay normal. Sends issued in a later turn
   during a slow confirm are held the same way (p50 290 ms late).
4. The round ends when the held call answers, so the commit waits the confirm plus a full round trip.

So the leader's AppendEntries **does wait for its own write's confirm**, which `host.rs` (`step`) says it does not:
the sends are issued in a microtask before the deferred transaction's macrotask, and workerd checks the output gate
once, at fetch start (`carried-4.md`). The bare object issues its fetch the same way and its one 915 ms confirm left
the request on time (arrival 84 ms) [one instance]. What differs on the member is not yet shown. Candidates:

- writes outside the entry's transaction that lock the gate before a send: on test installs, BF-14's `arrival` row, a
  bare `sql.exec` after each `/commit` reply (`member.rs`, `receipt_fault`), which `carried-4.md`'s pass 2 showed
  matters (queuing it made confirms 5 times slower);
- the fetch's gate check happening later than the call on the member's path (`fetch_with_request` with a `Request`,
  through `answer`'s select), after the transaction locked it.

**This also limits the offline replay.** Its N and F are the measured acks, which already include the hold. With the
sends not held, today's rule would commit at the confirm's end (about 140 ms earlier in each window above), and the
majority rule earlier still, at the far follower's unheld ack. So (c) is not shown to help; it is not shown useless
either, and the hold comes first.

## Verdict

**B0-02: between pass and fail, not passed:** 10 of 16 at 330 in the A/B (5 of 8 with either body); 8 of 8 at 300 not
run in this window. The stream is not the cause, and the majority rule as measured moves no miss. Every miss is the
leader's AppendEntries held until its own slow write confirms. The next step is finding which write holds the gate on
the member (a test-faults switch for BF-14's `arrival` row, A/B'd the same way, and a probe that sends through the
member's exact path), then making the send independent of it.

## Checks and cost

Full checks on `961d518`: fmt, workspace clippy, wasm32 clippy for site and edge with and without `test-faults` and for
ops, `cargo test --workspace` 722 passed and 0 failed, `go test ./...` in `containers/git/server`, and the three
`scripts/check_*.py`. `crates/raft/src` unchanged. graft-m3c 19:08 to 19:50Z: $4.36
(`INSTALL=graft-m3c python3 tests/fanout/m2bcost.py`), including the control.
