# Confirm StepDown, the read gate's shed retry, and the lease-render 500 (2026-11-18)

Code-only work from B0-33's m50 and m51 runs (`tests/am1/results/c-design.md`, "The B0-33 measurement, m50" and "B0-33
with 4 learners, m51"), on branch `health-gate-lease` from main 3e548e95. No hosted run, $0. Items, in the
orchestrator's order:

1. The Confirm `STEP_DOWN`: why site a logged `Confirm { p99_ms: 1093, slow: 6, of: 21 }` while the trace showed no
   confirm over 273 ms in the last 10 s. **Answered: the trace window was misplaced, by the orb's clock, not the
   rule's. No code change.**
2. The read gate's shed-retry loop. **Fixed:** backoff with jitter, and a write gives up after `SHED_RETRY_FOR` (4 s)
   instead of LOOKUP_FOR (20 s).
3. The lease-render 500. **Fixed:** 404.
4. The learner read-index design. Below, as a design, before any code.

## 1. The Confirm StepDown

### The rule saw what the trace shows

The rule (`crates/raft/src/member/health.rs`, `tripped`) keeps one sample per input that wrote. `Host::step` calls
`Member::confirm_begin` for such an input, and the `sync()` that covers it calls `confirm_end` for each one it covers
(`workers/site/src/host.rs`, `flush`). Both stamps come from the member clock (`Host::now`, `clock.rs`), and the window
is `STEP_DOWN.window_s`, 10 s, pruned on the same clock. The trace's `Confirm` row is one per `sync()`, on raw
`now_ms()`.

My m51 report compared the alarm with the 10 s before 11:17:35.4Z. That time comes from `terms-m51b33.jsonl`, which the
harness stamps with the orb's clock. The orb runs 7.3 to 7.5 s behind Cloudflare's: the newest trace row in each tsave
fetch is up to 7,463 ms (m51) and 7,333 ms (m50) after the orb's `at` for that fetch, and a `Date:` header from
cloudflare.com read 7 s ahead of the orb's `date` today. On Cloudflare's clock the change came at about 11:17:42.9Z,
just after the alarm at 11:17:42.318Z (Workers Logs). So the 10 s that mattered were 11:17:32.3 to 42.3Z on the trace's
clock, not 11:17:25 to 35.

In that window, matching each input that wrote (`Stepped` rows) to the `sync()` that covered it (the first `Confirm`
whose start is at or after it):

| alarm (Workers Logs) | samples in the 10 s before | over 500 ms | p99 | slowest samples (ms) |
|---|---|---|---|---|
| m51 site a, 11:17:42.318Z | 16 | 5 | 3,658 | 667, 1,443, 1,443, 3,658, 3,658 |
| m50 site a, 10:18:04.218Z | 15 | 6 | 3,000 | 1,201, 2,186, 2,607, 3,000 (and 727, 887) |
| m50 site a, 10:18:27.998Z | 19 | 6 | 1,193 | 778, 819, 903, 1,000, 1,018, 1,193 |

The logged alarms read `of: 21`, `slow: 6`, `p99_ms: 1093` (m51), and `of: 15`, `slow: 6`, `p99_ms: 1096` and `of:
18`, `slow: 6`, `p99_ms: 827` (m50). The sample and slow counts agree within a few and within one. The p99s do not
agree: the rule's p99 is the sample at index `len * 99 / 100`, which for 15 to 21 samples is the largest, and the
reconstruction's largest is 3,658, 3,000 and 1,193 ms. The member clock and the window's edges can move one sample in
or out, and one input's confirm ends with its own `sync()` even when the trace pairs it with a later one, but I have not
shown which sample each logged p99 is [unverified]. What the reconstruction does show is five or six inputs over 500 ms
in each window, from three or four syncs of 0.7 to 3.7 s.

One clue to the p99s: m51's alarm at 11:17:42.318Z is the exact end of the 3,454 and 3,658 ms syncs (trace,
11:17:42.318Z). The rule tripped on that `confirm_end`, yet its largest sample was 1,093 ms, so on the member clock that
sync took at most 1,093 ms where the raw clock read 3,658. The member clock is raised to peers' stamps (`clock.rs`), and
a busy isolate's `Date.now()` stands still between I/O polls (clock-guard, B0-02), so the raw stamp at the sync's start
was likely 2.5 s stale [inference]. If so, the trace's raw `Confirm` durations overstate the slow syncs, and the rule's
member-clock samples are the better measure. Either way the rule counted six inputs over 500 ms.

So the rule measured the confirms, over the right window: 1.4 to 3.7 s `sync()`s, three or four of them in 10 s. The
trace shows them too (`raw/tsave-m51b33.jsonl.xz`, member 0's `Confirm` rows at 11:17:38.005Z (1,443 ms) and 11:17:42.318Z
(3,454 and 3,658 ms)).

### Why those syncs were slow

- **Not load in the isolate.** During each sync of 1 s or more the leader's handler kept entering requests, at 29 to
  470 a second (`Entered` rows), and its ticker ran on time through m50's 1,971 and 1,000 ms syncs and five of m51's
  member-1 syncs. In others the ticker was 0.9 to 2.8 s late in the same span, so some overlap an event-loop stall too.
- **Followers have them idle.** m51's member 2, a follower taking 1 to 3 requests a second, had syncs of 2.4, 2.9, 3.4
  and 4.4 s.
- **The same picture as F35.** M0-WP4 found bare objects with no Raft code stalling over 500 ms about every 5 s, up
  to 4 in 10 s (`tests/m0/results/m0-wp4.md`, "F35"), and set `confirm_slow_min` to 6 for it. What m50 and m51 add is the
  size: the slow ones here run 1.4 to 3.7 s, and each covers several inputs.

### Is the rule wrong for a loaded leader?

Counted per input, one slow `sync()` covering several inputs counts several times. Sliding the rule's condition every
500 ms over each member's leadership (`/tmp` replay of the trace; numbers below), counted three ways:

| member while leading | per input (the rule now) | per distinct confirm end | per sync |
|---|---|---|---|
| m51 member 0 (50 s) | 0 half-seconds | 0 | 0 |
| m51 member 1 (232 s) | 8 (first 11:18:21Z) | 0 | 2 |
| m50 member 0 (37 s) | 22 (first 10:18:19Z) | 1 | 19 |
| m50 member 1 (253 s) | 44 (first 10:18:47Z) | 19 | 0 |

The replay does not reproduce m51 member 0's trip at all (the trace misses inputs the rule counted, above), so it
cannot be the evidence for changing the rule. Per sync and per distinct end disagree on m50. Counting per sync would
not have stopped m50's first change (19 of 37 leader-seconds still hold), and a per-end count would have kept member 0
on syncs of 2 to 3.7 s.

The stall is the platform's, and the rule did what DESIGN says: a leader whose storage confirms take 1.4 to 3.7 s
three or four times in 10 s hands off. What is new is that the new leader's own storage was no better: after each
change the new leader had slow syncs at 6.6 (m51) and 9.4 (m50) a minute against the old one's 0.3 and 0. Each change
traded one slow store for another. That is the "Flip" of `STEP_DOWN`'s confirm rule (`decisions/DECISIONS.md`, "six slow
confirmations") read the other way: the rule assumes a healthier member exists.

**Recommendation, not code.** The rule needs no fix for a measurement fault, because there is none. Whether a loaded
leader should hand off on platform stalls that follow the leadership is a design question with simulator evidence
behind the current setting (`slow_storage_everywhere_keeps_one_leader`, `slow_storage_on_both_followers_keeps_the_leader`):
it belongs to the learner-load work below. m51 cut the leader's slow syncs from 17 a minute (m50, member 0 before its
change) to 4, by halving the learner load. B0-33 can be measured cleanly once the leader's load no longer makes
multi-second syncs, which is the learner design's target. If the operator wants a rule change first, the candidate is
`peer_storage_slow` judging a target by its own confirms, not its answer times, so a confirm trip holds when the target's
storage is as slow. That is a `crates/raft` change and needs its own design.

## 2. The read gate's shed-retry loop

**Before.** `graftyard_lookup::read_spread_for` asked again after every shed pass, waiting the members' `retryAfterMs`
(250 ms; `OVERLOAD_RETRY_MS`), for as long as the edge allowed: LOOKUP_FOR (20 s) for a write (`spread_read`). A pass is
up to three members, each waiting up to FOLLOWER_WAIT (1 s) before it sheds, so a held write made 7 to 16 passes (up to
80 if they shed at once), every one reaching the learners. On m51 after the change the learners took 136 to 514 reads
a second each, 85 to 90% whole-tier `packed` reads, against 15 to 54 before, and 4,200 REST and GraphQL writes answered
503 after 19 to 24 s (p50 22.0 s).

**After.**
- `shed_wait` (`crates/lookup/src/lib.rs`): the wait after the nth shed answer in a row is OVERLOAD_RETRY_MS doubled
  per shed up to `SHED_BACKOFF_MAX_MS` (2 s), with a random part of up to half taken off so writes shed together do not
  come back together, and never under the `retryAfterMs` the members named.
- `SHED_RETRY_FOR` (4 s [guess]) replaces LOOKUP_FOR as a write's retry budget (`workers/edge/src/lib.rs`,
  `shed_retry_for`). A bounded Agent push keeps the smaller of it and its AGENT_PUSH_BOUND. Past it a write answers
  503 with `Retry-After: 1` (`api::read_failed`, unchanged), or the `ng` shape for an Agent push (`gate_ng`, unchanged).
  A read-class request still answers 503 at once.

With every pass shed, a held write now makes 2 to 5 passes, not 7 to 80, and has its answer by about 4 s plus one pass
(at most 7 s with three 1 s FOLLOWER_WAITs), not 20 to 24 s. That is the "Flip" of `lease-clock` (2), recorded in
`decisions/DECISIONS.md` as met.

**Kept.** The 7.4 s `Authz` stall that `lease-clock` (2) answered: a write now gets 503 with `Retry-After: 1` after about
4 to 7 s where it got through at 7 s before. That is the trade. The members, not the retry, set the capacity, and a
client that honours `Retry-After` comes back after the stall with its own retry, which the edge does not multiply.

**Tests** (each fails on main):
- `a_read_the_members_keep_shedding_backs_off_and_gives_up` (`crates/lookup/src/lib.rs`): doubling waits to 2 s, up to
  half taken off but never under the members' 250 ms, at most 12 tries in LOOKUP_FOR (main: 80), and at the edge's deadline
  with 3 s passes the error comes within SHED_RETRY_FOR plus one pass, after at most 3 tries. On main `retry_shed` takes
  no jitter argument, and with it restored to fixed waits the test fails on the waits.
- `a_shed_read_is_retried_inside_its_deadline_only` (updated): retried inside the deadline only, and the members'
  larger `retryAfterMs` wins over the backoff.
- `a_write_asks_a_shed_read_again_for_seconds_not_lookup_for` (`workers/edge/src/lib.rs`): a write's retry budget is
  SHED_RETRY_FOR, at most a quarter of LOOKUP_FOR, a bounded push no more than its bound, a read-class request zero.
  Main has no `shed_retry_for` and gives a write LOOKUP_FOR.
- Plants: `tests/m0carried/plant43.py`.

## 3. The lease-render 500

`leases::render_ok` (`crates/api/src/leases.rs`) renders a committed lease command's reply over tiers that have applied
the whole entry. A renew followed by a release of the same lease in one entry found no row and answered 500 `no lease
row`. On m50 all 157 renew 500s were renews with a release (204) of the same lease in flight. It now answers 404, as
`refused` does for `LeaseErr::NotFound` and as 2,881 other renews of a released lease did.

**Test:** `a_renew_whose_lease_a_release_in_its_entry_removed_answers_404` (`crates/api/tests/lease_api.rs`): acquire,
then renew and release in one entry; the renew renders 404 and the release 204. With the old 500 restored it fails
(`left: 500, right: 404`). Plant: `tests/m0carried/plant43.py`.

## 4. The learner read-index design (for review, no code)

See "Learner load" in the report to the orchestrator and the section below, which is the design.

### What the learners' calls to the leader are, measured

m51, per learner, before the change (60 s from load start) and after it (11:18:30 to 11:21:30Z), trace rows on the
learners (`raw/tsave-m51b33.jsonl.xz`):

| per learner, per second | before | after |
|---|---|---|
| `/raft` calls to the leader (`CallSent`) | 11.6 to 13.1 | 12.1 to 18.4 |
| of them read-index batches (`IndexSent`) | 7.0 to 10.7 | 2.9 to 8.7 |
| hedges (`Hedge`) | 0.1 to 2.5 | 1.3 to 3.9 (0 answered) |
| entries applied (`Applied`) | 1.8 | 2.2 to 2.4 |
| inbound `/read` from the edge | 15 to 54 | 136 to 514 |

So a learner's calls are mostly read-index batches, the rest acks and hedges. After the change the batches per second
went down while the reads went up tenfold: each batch carried more reads, and the read-index round itself got slower
(p50 925 to 1,000 ms in the 50 s after the change, against 47 to 335 before). The learners shed because the round was
slow, not because they asked too often.

### Were the ~500 packed reads a second the gate's retries or a client need?

Mostly the gate's retries [inference]. The clients' offered mix did not change at 11:17:35Z, and every write the gate
held asked again after each shed pass for up to 20 s. 4,200 writes held about 22 s each over the last 4 minutes is about
385 held at any time, each making a pass every 1 to 2 s of up to three reads, so about 400 to 1,000 reads a second;
the learners took about 1,470 a second after the change against about 140 before. The before-rate (15 to 54 a learner)
is the clients' need. Item 2 removes the multiplier; whether the rest is the reads' own growth is not separated here.

### Design: one read-index round per learner per leader round

**Goal.** Fewer learner calls to the leader per read, and no read-index round for a read whose answer the learner
already has the index for.

1. **Answer reads from the held append.** A learner whose call carried a held append from the leader (`AppendTaken`)
   could carry the leader's commit index as of the moment the leader built the response, under the leader's lease. A
   read that arrived at the learner before that call left could be served at that commit once applied, with no
   `ReadIndex` of its own: it is the same proof a `ReadIndexRes` gives, if and only if the commit is read when the
   response is built (after the call arrived), not when the held append was cut, which can be earlier than the read.
   The leader would stamp the response with that commit, its term, and that it held its lease then. This removes the read-index batch for every read that rides
   an append. On m51 that is the 2.2 to 2.4 appends a second per learner, each covering the reads queued since the last.
2. **Hold the batch for the next append.** With (1), a learner with queued reads sends its `ReadIndex` only when no
   append has reached it in `LEARNER_READ_EVERY` (50 ms). Under load an append arrives every 400 to 500 ms per learner
   (m51), so this does not cut batches; the cut comes from (1) letting the next append grant them.

**Safety.** (1) is the read-index argument with the call's response as the answer: a commit the leader reads while
holding its lease, after the learner's call arrived, covers every write acknowledged before the call left, so every read
that arrived at the learner before the call left. A commit cut into a held append before the call arrived does not. It needs the leader's
lease, not just its commit: an append from a deposed leader must grant nothing, the same check `on_read_index_res`
makes through `ReadIndexRes`. Porcupine and the long sweep must cover it, with a gray leader that loses its lease
between sending an append and its arrival.

**What it touches.** `crates/raft/src/member/install.rs` (`RemoteReads`, `check_remote`), the append message's
format (a lease mark, append-only on the wire), and the sim. No site-script change beyond the member build.

**What it does not cover.** The packed whole-tier reads themselves: each costs the learner a dump of about 150 KB, and
item 2, not this design, removes the retry multiplier. If the clients' rate of whole-tier reads grows, the next lever
is DESIGN "Follower read"'s narrow reads, not the read-index path.

**What I would measure first.** The share of learner reads an append could have granted, from m51's trace: reads
queued at a learner when an `AppendTaken` arrived from a leader holding its lease. That number sets the expected cut;
it needs an `AppendTaken`-to-read join the existing trace rows support.

## Checks

All on the branch, from the repository root:
- `cargo fmt --all -- --check`: ok.
- `cargo clippy --workspace --all-targets -D warnings`: exit 0 (one `unused_dependencies` warning, also on main).
- wasm32 clippy for site and edge, with and without `test-faults`, and for ops (no `test-faults` feature): exit 0.
- `cargo test --workspace`: ok.
- `go test ./...` in `containers/git/server`: ok (44.8 s).
- `scripts/check_design.py`, `check_names.py`, `check_links.py`: OK.
- `tests/m0carried/plant43.py`: 8 of 8 caught.
- `crates/raft` is unchanged, so no long sweep and no Porcupine.

Hosted rows: none (code-only, $0).
