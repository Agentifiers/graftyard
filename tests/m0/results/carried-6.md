# B0-02: the write that holds the leader's send (2026-11-13)

The operator's decision on `carried-5.md`: B0-02 measures the production write path, so no write that exists only on
test installs may sit on the send path while the gate runs. A/B BF-14's `arrival` row first. If it holds the send,
run the gate with it in memory; if not, find the production write that does. The gate is unchanged: 8 of 8 one-minute
runs at 300 offered and 8 of 8 at 330, commit p99 at most 600 ms. Branch `m0-carried-6`; install `graft-m3d`, used by
this thread alone from 23:57Z; sites built from `dc7eb7e` (main `85ca32b` merged), `test-faults`.

## Verdict

**B0-02: not passed.** The gate ran once (02:05 to 02:33Z, `arrival=mem`, each run on a new group). It passed **7 of 8
at 300** and **6 of 8 at 330**. One run failed outright: r300-7 at 1,226 ms and 249/s. The arrival row is not the
cause. What holds a send is a write the leader made earlier and has not yet confirmed. That is the platform's
documented behavior, and the design had stated the opposite. The two runs that sank the gate (and the A/B's four worst)
show a different, stronger signature: the leader's event loop running behind. Neither is fixed, and the fix is a
design question (below).

| Run | Commit/s | Commit p99 | Leader confirm p99 | Round p50 | Clamp share | Verdict |
| --- | --- | --- | --- | --- | --- | --- |
| r300-1 | 280 | 373 | 76 | 142 | 0.02 | Pass |
| r300-2 | 280 | 266 | 49 | 124 | 0.02 | Pass |
| r300-3 | 280 | 299 | 124 | 125 | 0.02 | Pass |
| r300-4 | 280 | 263 | 68 | 122 | 0.02 | Pass |
| r300-5 | 280 | 522 | 68 | 165 | 0.02 | Pass |
| r300-6 | 279 | 234 | 33 | 108 | 0.02 | Pass |
| r300-7 | 249 | **1,226** | 875 | 300 | **0.53** | Fail |
| r300-8 | 280 | 279 | 45 | 124 | 0.02 | Pass |
| r330-1 | 308 | 311 | 40 | 138 | 0.02 | Pass |
| r330-2 | 308 | **628** | 157 | 164 | 0.02 | Between |
| r330-3 | 308 | 500 | 66 | 164 | 0.02 | Pass |
| r330-4 | 308 | 504 | 135 | 154 | 0.02 | Pass |
| r330-5 | 308 | 239 | 43 | 109 | 0.02 | Pass |
| r330-6 | 308 | 494 | 78 | 155 | 0.02 | Pass |
| r330-7 | 308 | 504 | 74 | 165 | 0.02 | Pass |
| r330-8 | 276 | **1,000** | 827 | 300 | **0.46** | Between |

0 failed commands in all 16. Raw: `raw-carried-6/m0c6g-*` (bench rows and gzipped `/stats`).

## The arrival-row A/B

Group `repo:6867000` (leader enam, near follower 100 ms, far 188 ms), 16 one-minute runs at 330 offered, alternating
`/stats?arrival=sql` and `arrival=mem` (`Host::arrivals_mem`, `2b582de`; `/arrivals` serves either), 00:09 to 00:25Z,
with the bare-object control alongside:

| Arrival | Passed | Commit p99 (median) | Confirms of 150 ms or more |
| --- | --- | --- | --- |
| `sql` | 4 of 8 (861, 933, 900, 543, 454, 900, 553, 499) | 707 | 123 of 2,768 |
| `mem` | 6 of 8 (730, 600, 677, 460, 386, 577, 542, 467) | 559.5 | 55 of 2,875 |

The first six runs (both modes) were slow, and the later ones were quiet, so the A/B is confounded by time. The row
does make confirms slower: it is a bare `exec` after every `/commit` reply, its own implicit transaction. But sends
whose same-turn confirm took 150 to 300 ms were late by 112 to 455 ms (`held.py`) **in both modes**. So the row is not
what holds them. The gate then ran with `arrival=mem`, as the operator asked. Alignment evidence stays: `/arrivals`
serves the in-memory rows, and `held.py` and `clamp.py` run on the gate runs themselves.

## Which write holds a send

A production write, the previous entry's transaction. workerd's `ActorSqlite::onWrite` (`src/workerd/io/actor-sqlite.c++`,
main as of 2026-11-12) locks the output gate when an implicit transaction takes its first must-confirm write, and the
lock lasts until the **previous** commit (`lastCommit`) confirms. `fetchImpl` (`src/workerd/api/http.c++`) waits once,
at fetch start, for every lock taken before it. The Durable Objects storage docs say the same thing: the gate "will pause
outgoing network messages from the Durable Object until all previous writes have been confirmed flushed to disk"
(https://developers.cloudflare.com/durable-objects/api/storage-api/, `allowUnconfirmed`). SQL `exec` has no
`allowUnconfirmed` option.

So `step`'s ordering (send in a microtask, transaction one macrotask later) keeps a send clear of **its own** input's
transaction, but not of a transaction an **earlier** input flushed that has not confirmed yet. The traces show it.
`clamp.py` bins every send with entries by how much of an earlier transaction's confirm was left when the send was
issued. Its same-turn confirm was under 60 ms, and lateness is measured against a local median after removing each
call's clock offset from its own two legs:

| Earlier transaction's confirm left at issue | A/B, 16 runs: sends, late p50 / p90 | Gate, 16 runs: sends, late p50 / p90 |
| --- | --- | --- |
| none | 3,984: -6 / 58 ms | 3,544: -3 / 32 ms |
| 1 to 50 ms | 440: 8 / 73 ms | 448: 17 / 54 ms |
| 50 to 150 ms | 89: 112 / 266 ms | 32: 73 / 330 ms |
| over 150 ms | 10: 342 / 512 ms | 10: 256 / 493 ms |

Each such send is late by about what was left of the earlier confirm. Every one of the 58 sends in the quiet runs
(A/B 06 to 16 and the gate's runs other than r300-7) is the same input. The leader steps an `AppendRes` from the
**far** follower while the near follower's answer, stepped a turn earlier, has already appended and flushed the next
entry, and that transaction has not confirmed. The far follower's next AppendEntries then waits for it. Near-follower
sends never hit this (0 of 58), because the near answer commits the entry and the next append happens in the same
turn as that send. So this hold delays the far follower only. In a quiet run it does not move commit, because the near
follower makes the quorum. It moves commit only when the near call is slow too.

The `arrival` row is one more earlier write of this kind, about 330 a second. That is why `arrival=sql` raised confirm
times without changing how late the held sends were.

## The stronger signature: the leader's event loop is behind

r300-7 and r330-8, and A/B ab01, ab02, ab03 and ab05 (the six worst runs, commit p99 730 to 1,226 ms, 0 of 6 passing),
share a signature the other 26 runs (23 of 26 passing, worst 900 ms) do not have. Of the leader's trace events, 10 to
53% fall on **one millisecond phase**, against 2% when the loop keeps up. Their slow confirms are exact multiples of
100 ms (53 of 120 in r300-7, 63 of 106 in r330-8, 0 in the quiet runs). Calls end at the same millisecond a confirm
ends. And the 100 ms ticker's `Lag` trace never fires.

That is what workerd's clock does when the event loop is behind. `TimerChannel::now` (`src/workerd/io/io-channels.h`)
clamps time to the earliest timeout deadline of the active scope, "including fired timers whose callbacks have not
completed". The member's 100 ms ticker (`Host::kick`) is always armed. So while the isolate is behind, every
`Date.now()` reads that overdue tick's deadline: confirms, call ends and sends all stamp the same phase, and the
ticker computes its lag as 0. Inference: in those windows the isolate is not running the leader's turns. That would
make the held sends, the slow confirms and the late replies one event, seen three ways.

What keeps the loop behind is not shown. It is not the leader's own CPU: per command it is 0.14 to 0.72 ms in the quiet
gate runs against 1.28 in r300-7 and 0.81 in r330-8, which is 23 and 16 s of CPU in a minute (Cloudflare analytics,
`durableObjectsPeriodicGroups`). Two candidates fit: the CPU of a shared isolate or machine (both bad runs placed the
leader in ORD), and the work of whatever the loop runs late. Neighbouring objects in the A/B's colo (ATL) showed no CPU
spike in the slow seconds (51 against 54 ms/s, `ab03`), and the precreate traffic was steady through quiet and slow
runs alike. Placement varies by group: the gate's 16 groups put the enam leader in IAD, ATL, EWR, MIA and ORD.

Run r330-2 (628 ms) is the other shape. It has no clamp. One confirm took 738 ms (entry 242, 114 KB), and the far
follower's next call waited for it (issued at +1 ms, answered at +985 ms). Entries over 120 KB had a confirm of 150 ms
or more 45% of the time, against 0.7 to 1% for 40 to 70 KB entries (all 32 runs).

## merge-budget-4's 10.1 s gap

The cause found here does not explain it. In that gap the **followers'** `/raft` calls took 3.0 to 3.1 s wall time at
about 10 ms of CPU (`tests/am1/results/merge-budget-4.md`). A follower's reply waits only for the follower's own
writes, and the leader's held send cannot lengthen a call already running on the follower. The clamp signature would
fit a follower whose isolate is behind, but that run's traces were not read here, so this is not shown.

## Design correction

`design/DESIGN.md` (Shape, the write path; Scale, "Commit on `Tri`") and `decisions/DECISIONS.md` ("The leader sends
AppendEntries before its own append is durable") now say that sending first keeps a round clear of the leader's own
write only. A send issued while an **earlier** write is unconfirmed waits for it, with the doc above cited. The
`host.rs` comment in `step` says the same. No code change. The two ways to remove the hold are design questions, so
they are not built (see "For the operator").

## LM-1's data points

LM-1 (`graft-m2b`, production-shaped repos) reported leader confirm p99 86 and 146 ms at 250 and 500 agents (m18), and
79 and 147 ms (m19). Commit p99 at 500 agents was 950 ms (m18) and 567 ms (m19), and commit-queue p99 was 697 and
421 ms. Their commit tails follow queueing, not the leader's confirm. That is consistent with this file: a slow confirm
moves commit only when it holds the near follower's send or the loop is behind.

## For the operator

1. **Decide how the leader's sends get clear of earlier writes.** Both options change the host's write ordering, not
   `crates/raft/src`:
   - (a) A send that finds an unconfirmed earlier transaction waits in the host and goes out at that confirm's end
     (what the gate does today, made explicit). It changes nothing measured.
   - (b) The leader issues every AppendEntries before **any** write of the turn that may follow, and defers the
     flush of a turn that only stepped an `AppendRes` until the round's sends are out. This removes the far-follower
     hold, but not the near one in a clamped window.
2. **Decide whether the clamp windows count against B0-02.** They are platform behavior on some placements (ORD in
   both gate failures). Two ways forward: a placement rule that measures the leader's own loop lag, through the clamp
   or a `scheduler.wait` probe, and moves a group off a lagging host; or an F-number for it, as B0-29's storage stalls
   have. Rerunning for a quieter window is not on offer, per the earlier decision.

## Checks and cost

Full checks on `dc7eb7e`: fmt; workspace clippy; wasm32 clippy for site and edge, with and without `test-faults`, and for
ops; `cargo test --workspace` 751 passed and 0 failed; `go test ./...` in `containers/git/server`; and the three
`scripts/check_*.py`. This file, `clamp.py`, and the DESIGN, DECISIONS, BUILD-PLAN and comment edits add no code a
check covers. `crates/raft/src` is unchanged, so no long sweep or Porcupine. `graft-m3d` from 23:57Z to 02:35Z cost
$7.55 (`INSTALL=graft-m3d python3 tests/fanout/m2bcost.py`), including the A/B, the gate, the site day and LF-13's
precreate rerun.
