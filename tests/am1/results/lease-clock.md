# lease-clock: the member clock floor, the read-lease guard, and a write's shed read

Branch `lease-clock`, off main `6a9857f2` (admitted-bytes). It follows the m3fr5b and m3fr5c rows (`tests/am1/results/lm1-rows-m3f.md`, "Rows m3fr5b"), which failed L0-12's clock line, B0-31 and B0-32. Decision record: `decisions/DECISIONS.md`, "`lease-clock`". New open question: F75.

## The clock probe

L0-12 m3fr5b read the leader under load up to 1.2 s early against Cloudflare's clock, while followers read within about 330 ms. The guess was that a Worker's `Date.now()` advances only on I/O. A throwaway Worker tested it (`tests/am1/clockprobe/index.js`, deployed as `graft-clockprobe` on the m3f account and deleted after). One Durable Object runs a loop: wait, then spin the CPU for `work` ms. A second request to the same object reads its own clock, calls a peer object that answers its `Date.now()`, and compares them. Each row is the peer's stamp minus the caller's clock when it sent the call, which is the caller's lag plus one way.

| Load on the object | Caller's clock behind the peer's stamp |
| --- | --- |
| Idle | about 5 ms |
| Loop of 150 ms CPU a turn | 245 to 392 ms |
| Loop of 400 ms CPU a turn | 715 to 1,681 ms |
| Same, read again right after an awaited storage `sync()`, `get()`, a subrequest, or `setTimeout(0)` | still 654 to 1,588 ms |
| Read right after receiving the reply that carries the stamp | up to 1,349 ms before that stamp |
| `max(own clock, last peer stamp)` | 7 to 139 ms |

[measured, 2026-11-15, one object pair on the m3f account.] The Workers documentation and workerd's source agree: `Date.now()` returns the time of the last I/O event the runtime polled, and a busy isolate polls late, so the clock can read before the event it is handling arrived. workerd's `precise_timers` flag is experimental and not usable on the platform [doc].

So the parent's first fix candidate, reading time after the last I/O before `decide`, does not work: an await on storage does not bring the clock forward. The probe rules it out.

## Is the read lease exposed?

The code is consistent with itself. `start_read` records `arrived = self.now` in the read's own step. `check_reads` grants on `now` from the same step or a later one. A round grant needs `quorum_since(acked_at) >= arrived`. `lease_end` is the quorum's `promised_at + LEASE`.

The argument that a stale clock is harmless assumed the clock never reads before the arrival of the event being handled. The probe shows it can: a busy isolate's clock read up to 1.35 s before a stamp already received. With a stale `now` at the lease check, a read can be granted on a lease that has already ended in real time, by up to the lag. The margin between a lease's end and a follower's campaign is 1.5 s (`ELECTION_AFTER` 10 s against `LEASE` 9 s less `DRIFT` 0.5 s). A lag of 1.5 s or more on the leader, with a new leader elected and a write acknowledged in that window, would let the old leader serve a read that misses the write. This is [inference]: no stale read was measured, and no test pins it.

## What the branch changes

**The member clock** (`workers/site/src/clock.rs`, `Host::now`, `Host::stamped`):
- Every `/raft` call and every `/raft` reply carries the sender's member clock in `x-graft-clock`, in the product build, and every edge call to a member through `graftyard_lookup::post_within` carries the edge's clock.
- A member's floor is the largest stamp it has seen this boot. Every input steps at `max(now_ms(), floor)`, never below an earlier reading (monotonic).
- `/status` `now`, `?clock=1`'s `clockBefore` and `clockAfter`, `/sever`'s `now`, and `/elections`' `now` report the same clock. So L0-12's clock line now measures the clock that leases and `decide` read.
- A stamp from a sender whose clock runs ahead raises the floor by at most that sender's drift, which `DRIFT` already budgets.

**The guard** (`Clock::suspect`, `raft::Member::set_clock_suspect`):
- Each stamp's lag is the stamp minus this isolate's clock at its arrival. While the largest of the last 8 lags (`LAG_WINDOW`) is `DRIFT / 2` (250 ms) or more, the clock is suspect.
- While suspect, a read that arrives on the leader is not granted on the lease. It waits for a round sent after its arrival, which rests on the followers' acks. The raft change is that one boolean, read in `start_read` only.
- A read that arrived on a live lease with a trusted clock is linearized at its arrival. A later grant on that lease is still safe, so `check_reads` does not reread the flag.
- `/stats` reports `clock`: the lag of every stamp seen since the last reset, as counts in power-of-two buckets (`lagBuckets[0]` a lag of 0, `[i]` below 2^i ms), `lagMax`, the window the guard reads now, `suspect`, and `leaseFallbacks`, the reads a leader sent to a round because of it.

**What stays open (F75).** If every member is saturated and an edge stamp is itself old by the time the read is handled, no fresh stamp reaches the leader. A read can wait in a busy isolate's queue after its edge stamp was taken, so its handler's floor is no later than that stamp, and the guard measures lag only against stamps that arrived. The case needs all three members and the edge stamps behind together by more than the 1.5 s margin [inference]. The measures that would show it: guard fallbacks per minute, the lag distribution per member, and any stale read in a probe.

## The shed Authz read (m3fr5b, 03:02Z)

The Human PAT write that failed 503 "every member asked shed the read" was not shed by any cap. All line numbers are as of `6d6b2efd`.
- **The read.** `resolve`'s `{"q":{"repo":"fm3fr5b-l012"}}` on `authz:7100001`, the org m3org. The query goes through `authz_get` to `read_spread`, which adds `"index": true`, and is sent to the two followers; the cached config had no learners. A git request or a POST is not read-class, so `resolve` skips the cache. The answer is `AuthzFold::read_json`, a few hundred bytes. It takes one `READS_MAX` slot and counts toward `ANSWER_BYTES_MAX`, and not toward `HEAVY_READS`. The members took 22 to 53 `/read`s in that minute, so no cap was near.
- **What happened.** The Authz leader `df792e6a` on site a stalled from 03:02:05.5 to 03:02:12.890Z. One alarm ran 7,362 ms of wall time on 1 ms of CPU, and 15 `/raft` calls into it, started from 05.974 to 11.699, all ended at 12.890. Neither follower could get a read index. Follower `ef42` on site b did not answer within the edge's `FOLLOWER_CALL` (2 s). Follower `7d31` on site c answered `overloaded` after its `FOLLOWER_WAIT` (1 s). `spread_verdict` counts both as shed. [measured, Workers Logs; the member ids are inferred from the traffic.] That every response left at one instant fits an output gate held by a slow storage confirm [inference].
- **Three 503s.** A receive-pack at 05.608 to 08.183 (the admin's scratch push, which `rows.mjs` ignores), a REST write at 08.217 to 09.699, and a GET `info/refs` at 10.214 to 12.522. The last is the push the driver counted as rejected. Requests succeeded again from 12.649.
- **Nothing retried it.** `read_spread` returned the shed at once and dropped the members' `retryAfterMs`. Only `call_opt` retries `overloaded`, on leader reads.

**The fix.** `graftyard_lookup::read_spread_for` asks again while every member asked sheds the read. It waits the largest `retryAfterMs` an `overloaded` answer named, else 250 ms. It keeps asking while that wait ends inside its deadline, then returns the shed as before. The edge marks a request a write (`Ctx::write`): a receive-pack `info/refs` or RPC, a REST call other than `GET`, or a GraphQL mutation. A write's spread reads (`resolve`, `authz_read`, `core_rows`, `tier_rows`, `api_rows`) take `LOOKUP_FOR` (20 s), as `call_opt` does. Any other request answers 503 at once, as before. The 7.4 s stall would have been covered.

## The hung push (m3fr5b, about 02:09Z)

The push was k = 82, not the 100th: counting that kind's REST writes from its first at 01:53:23Z, the write that ended at 02:09:25.507 is the 83rd. The driver's git worktree (`/tmp/am1-git-F4iNwg/wt`) has a reflog commit `l012 0-p82` at 02:09:22Z and the next, `l012 0-83`, at 02:14:22Z, exactly the driver's 300 s timeout apart.

The edge saw nothing of it:
- There were no edge events from 02:09:25.6 to 02:14:25.7, and no outcome other than ok from 02:09:25 to 02:15:00.
- No request ran past 30 s of wall time from 02:09 to 04:00.
- No service logged an event naming `fm3fr5b-l012` in the gap. Logging worked: the tools edge logged at 02:11:40.
- Not even git's first unauthenticated `info/refs`, which always gets a 401, arrived.

The credential helper runs only after a 401, and `GIT_TERMINAL_PROMPT=0` is set. The driver waited its 300 s and moved on.

**Verdict: client side.** The git process or its connection stalled before any request reached the edge [measured absence; whether git or TLS stalled is not recorded, because `rows.mjs` keeps no stderr]. The rows doc's "100th push" and "the next kind" are wrong, and the next row write-up corrects them.

## B0-32: why site a's validates ran p50 5.4 s

Local analysis of m3fr5c's Workers Logs, 04:01:00 to 04:02:30Z. No hosted spend. The split below comes from matching timings, not request ids.

**What k granted.** k was 4: slots [3, 5, 4] on sites 0 and 1, which is 3 pool shards plus the network shard. All four shards on site a took validates. The edge calls only the network shard, which forwards to a pool shard (`workers/site/src/shard.rs`, `push_validator`). 696 of the network shard's 925 calls carried `via=0&pool=1`, which is 75%, as rendezvous over 4 shards gives. Sites b and c got none: the edge leaves the home site only on a 503 or a failed call, and a pool forward stays on the network shard's site. Site b's pool shards were cold anyway (`catchup?warm=pool` 503 at 04:01:11 and 04:02:37). Pool-keep did not hold the lease through the idle gap from 03:56:35 to 04:00:00, so the pool rehydrated during warm-up; that cost warm-up, not the burst.

| Shard | Calls | 200s, p50 / p90 / max | 429s |
| --- | --- | --- | --- |
| network shard | 925, 696 forwarded | 899, 3.9 / 11.6 / 19.4 s | 26 |
| pool:6 | 243 | 238, 1.06 / 3.1 / 5.5 s | 5 |
| pool:5 | 254 | 237, 0.88 / 4.5 / 7.4 s | 17 |
| pool:4 | 199 | 199, 5.1 / 9.9 / 12.1 s | 0 |

**Where the time went.** A forwarded push waits through two catch-ups. The network shard's `/validate` waits for `caught_up()` before it decides to forward, and the pool shard runs its own `caught_up()` before it validates. `Jobs::wait` waits for the run in progress and then a whole new one, so each wait covers 1.4 to 1.9 runs.

| Path | n | Network-shard wall p50 | Wait on its catch-up (mean) | Wait on the pool's (mean) | Container work after catch-up (mean) |
| --- | --- | --- | --- | --- | --- |
| forwarded to pool:6 | 202 | 6.1 s | 2.9 s | 1.4 s | 0.43 s |
| forwarded to pool:5 | 213 | 5.0 s | 2.8 s | 2.0 s | 0.53 s |
| forwarded to pool:4 | 170 | 9.8 s | 2.9 s | 4.3 s | 0.64 s |
| validated on the network shard | 306 | 1.5 s | 1.6 s | none | 1.15 s |

- **Every shard ingested every burst pack**, about 564 packs and 145 MB each, so k = 4 did the ingest work four times. Runs grew to 4 to 9.5 s during the burst. Ingest costs about 40 ms a pack, about 23 ms of it in `index-pack` under the exclusive lock.
- **The stored `.idx` was never used** (`idx_used` 0 in all 286 catch-up lines). That is expected: catch-up passes an empty `with_idx` (`crates/git/src/catchup.rs`), and only promoted packs have a stored `.idx`.
- **The parallel fetch ran**: spooling overlapped ingest.
- **The 429s are waiters released together** when a catch-up ends. The gate admits 8 and queues 64. When pool:5's run ended at 04:01:22.361, 89 validates were released and 17 got 429, which is 89 − (8 + 64).
- **Not the limit**: the container's own work (p50 0.36 to 0.9 s), the spread across shards, and the leader (commit p99 1.46 s).

**Fix candidate: forward before catching up on the network shard.**
- In `GitShard::fetch`'s `/validate` arm, move the forward block (`pool_slots`, `push_validator`, `pool_forward`) ahead of `caught_up().await`.
- Catch up only on the paths where the network shard validates the push itself: `via` set, `derived`, no slots, the pick is shard 0, or the pool shard is cold.
- Validation does not change, because the pool shard still catches up before it validates. A failed forward still answers `x-graft-fanout-failed`, and the edge resends with `via=0`, which catches up.
- Replayed on this run's timings with the network-shard wait removed from forwarded calls [inference]: burst arrivals p50 6.9 → 3.9 s, p90 13.4 → 7.9 s, and 231 → 417 of 588 within 5 s.

That alone probably does not pass B0-32's 5 s line, because each pool shard still ingests every pack one run at a time. The next step is to validate against the shard's current disk and catch up only when the push's base is missing. With no catch-up wait the same model bounds p50 at 1.0 s and p90 at 3.1 s. The code does not support it yet: the container's `/validate` answers 422 for any `index-pack` failure, so it would need to report a missing base separately, as `unresolved()` already does for ingest. The detail's merge-base and `ff` also read refs that may be stale on a disk that has not caught up. This branch does not change `shard.rs`.

## Checks

On this branch (crates/raft changed, so all of them):

- fmt, workspace clippy, wasm32 clippy for site, edge and ops (site and edge with and without `test-faults`): clean.
- `cargo test --workspace`: 969 passed, 0 failed. `go test` in `containers/git/server`: ok. The three
  `scripts/check_*.py`: OK.
- `tests/m0carried/plant15.py`: 9 of 9 caught (the guard ignored at a read's arrival; the guard tripping only past
  `DRIFT` / 2; the guard reading only the newest lag; an older stamp lowering the floor; a non-monotonic clock; the
  floor never applied; a shed read never retried; one retried past its deadline; the retry ignoring the members' wait).
  A plant of the guard ignored in `check_reads` was not caught, because that check was redundant: a read is judged at
  its arrival, so the check was removed rather than tested.
- Long sweep (`RAFT_SEEDS=2000 RAFT_STEPS=10000`, `long_sweep`): ok in 988 s. Porcupine (`-seg 2000`): `h` Ok on
  157,641 operations, `h.gray` Ok on 2,247,522, `h.live` Ok on 5,963,505.
- `a_suspect_clock_grants_reads_only_by_a_round` does not compile on main (no `set_clock_suspect`); the clock and
  retry tests are new with the code they test.
