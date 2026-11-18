# clock-guard: why B0-29's leader looked 256 ms late, and the lag the lease guard reads now

Branch `clock-guard`, off main `ac4b2ceb`, code-only, design first. The decision record is `decisions/DECISIONS.md`, "`clock-guard`". Asked for by the operator after B0-29 on read-beat (`tests/am1/results/b029-m2b.md`, "Run 3"). That run's leader `/stats.clock` put 97.4% of stamps at 256 ms or more of lag and counted 5,061 lease fallbacks, so every read paid a round.

## The question

What blocks the leader isolate's turn under B0-29's load so that its clock lags 256 ms or more? The operator's candidates were long synchronous `decide` or fold work, JSON encoding of large replies, SQLite calls, the alarm handler, or the guard's own window measuring the wrong thing. The answer is the last one. Nothing in the leader's turn blocks for long, and the guard measured `Date.now()`, which no lease reads.

## Nothing blocks the turn

Sources: run `mwlbozdl` (repo 131354, load 23:35:12.7–23:45:12.7Z), its tsave trace `tests/fanout/raw/tsave-m2bb29c.jsonl.xz`, and the account's `durableObjectsPeriodicGroups` for `repo:131354#1#f0` (`tests/m0/clockguard/docpu.py`, `tests/m0/clockguard/turns.py`) [measured].

| Reading on the leader | Value |
| --- | --- |
| CPU time per minute of load | 19.8 to 26.3 s: 33 to 44% of one core; 247 s over 13 minutes |
| 100 ms ticker woke 20 ms or more late (`Trace::Lag`) | 9 times in 600 s; p50 248, max 670 ms |
| event-loop lag at each `/raft` handler (`CallLeft`) | 0 at p99, max 670 ms, over 89,823 calls |
| event-loop lag at each `/raft` send (`CallSent`) | 0 at p99, max 670 ms, over 37,028 calls |
| synchronous step and persist (`Stepped`) | 0 ms at the max, over 3,940 inputs that wrote |
| deferred transaction (`Flushed`) | 0 ms at p99, max 273 ms |
| storage confirm (`Confirm`) | p50 52, p99 192 ms (an awaited `sync()`, not a blocked turn) |
| requests open in the handler (`Entered`) | p50 38, p99 186 |

A turn that blocked for 256 ms or more would show up in the ticker and in the loop lag at the next handler. Both are 0 at p99. The leader is busy, at about 40% of a core, but it is not saturated. So the lag is not CPU, and admission has nothing to shed for it.

## Where the 256 ms comes from

`Date.now()` in a Worker is the time of the last I/O poll (`tests/am1/results/lease-clock.md`, "The clock probe"). It stands still between polls, even across awaited storage calls. On the trace, the raw stamp gap (stamp minus the receiver's `Date.now()`) runs both ways at once:

- At the leader, from its followers: p50 590 and 537 ms ahead.
- At the followers, from the leader: p50 643 and 714 ms behind.
- Learners on site 0 see the leader ahead or behind by about 100 to 300 ms, and learners on sites 1 and 2 see it 500 to 650 ms behind.

Real clocks cannot all run ahead of each other. Each isolate's `Date.now()` reads early by a different and varying amount, and the gap between two of them says nothing about either one's real time. Before load (two minutes before) the same pairs sat at 20 to 220 ms.

## The guard measured the wrong clock

Since `lease-clock`, every lease decision reads the member clock, `max(Date.now(), largest stamp seen, last reading)` (`workers/site/src/clock.rs`, `Clock::now`): `start_read`'s arrival, `quorum_since`, and `lease_end` through `promised_at`. But `Clock::stamp` recorded each lag as the stamp minus `Date.now()` alone. That made the guard fire on exactly the staleness the floor had already removed.

The fix measures a stamp against the member clock just before it arrived. A stamp that is ahead of that clock still means the clock a lease reads is behind a sender. A stamp the floor already covers means nothing new.

`tests/m0/clockguard/replay.py` rebuilds every traced member's member clock over the `/raft` call graph. Each message's stamp is then its sender's member clock, as `x-graft-clock` is. It replays the leader's guard both ways. Edge stamps are not traced, so they are left out [measured, replay]:

| Run | Leader stamps in load | Old lag p50 / p99 / max | Old: suspect | New lag p50 / p99 / max | New: suspect |
| --- | --- | --- | --- | --- | --- |
| `mwlbozdl` (read-beat) | 126,769 | 419 / 2,337 / 3,444 | 99.6% | 0 / 86 / 440 | 0.06% |
| `mwkyk9zg` (run 2) | 129,186 | 404 / 3,075 / 3,520 | 98.3% | 0 / 80 / 518 | 0.11% |

The old rule's replay matches the hosted `/stats.clock` on `mwlbozdl`: 91.6% of stamps at 256 ms or more in the replay, against 97.4% in the buckets, which also count edge stamps.

## Safety

The lease stays correct for the same reason as before: the floor. A stale `Date.now()` under a fresh floor puts the lease on the floor's clock, which `lease-clock` already relies on for `LogEntry.at`, expiry and `/status`. The guard still catches what the floor cannot fix: a stamp ahead of the member clock, meaning this member and every sender it heard from read late. That is F75's case, all members saturated together, and it stays open [inference]. No raft code, wire format or trace kind changes. `set_clock_suspect` and its rule (8 stamps, `DRIFT` / 2) are unchanged.

## Expected on the next B0-29

Lease reads come back for about 99% of arrivals, so the read rounds (about 30 calls a second to each follower) mostly stop. `leaseFallbacks` drops from 5,061 to tens, and `clock.lagBuckets` sits under 128 ms. What that does for commit p99 is not measured. The rounds cost the followers and the leader's confirm, but the entry slot was the tail. The next run carries F85 too, so it cannot split the two.

## Verification

- **Test:** `clock::tests::a_stamp_the_member_clock_already_covers_shows_no_lag` fails on main ("a stale own clock under a fresh floor counted as lag") and passes with the change. The other three clock tests pass unchanged.
- **Plants:** `tests/m0carried/plant37.py`, 4 of 4 caught:
  - the lag against the isolate's own clock (main's rule);
  - the lag ignoring the readings given out;
  - the lag ignoring the isolate's own clock;
  - the floor raised before the lag is taken.
- **Full checks:** see the commit message.
