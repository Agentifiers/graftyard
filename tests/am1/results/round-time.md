# Round time under load: where m60's append time went, and why the leader ended on apac

LM-1, 2026-11-19, code-only analysis of B0-33 m60r33 (and m56b33 as the near-leader control). Nothing is built; the
two proposals at the end wait for the operator.

## 1. Append legs: the follower's queue is not where the time goes

**Hypothesis tested** (operator's, unverified): followers' input queues hold AppendEntries behind tier `/read`s, so
confirms slow, `STEP_DOWN` trips and rounds stretch.

**Method.** The trace alone cannot split an append's out leg: `CallSent` and `AnswerArrived` are the leader's isolate
clock, `CallLeft` and `Entered` the follower's, and `tests/f74/calls.py`'s skew estimate reads −2,032 to +1,775 ms on
m60. Every `/raft` call also carries `x-graft-call`, `x-graft-sent` and `x-graft-clock` (the leader's member clock), and
Workers Logs records each callee invocation's end and wall time on Cloudflare's clock. `tests/fanout/applegs.py` joins
the two on the call id. Raw extracts are in `tests/fanout/raw/applegs-m60/` (no account fields). Legs:
- **qin:** the follower's handler entry minus its invocation's start (Cloudflare). This is the follower's own turns
  ahead of the call.
- **out:** the invocation's start minus the leader's member clock at send. This is wire, plus anything that held the
  call before the follower's invocation began.
- **hand:** the follower's handler, on one clock.

| leader → follower | calls | qin p50 / p90 / p99 | out p50 / p90 / p99 | hand p50 / p90 | rtt p50 / p90 (leader clock) |
|---|---|---|---|---|---|
| m60 c → a (load, 14:40:30–14:44:17) | 205 | 26 / 115 / 265 | 298 / 996 / 1,588 | 32 / 276 | 659 / 1,815 |
| m60 c → b (load) | 346 | 23 / 139 / 238 | 329 / 908 / 1,473 | 20 / 179 | 588 / 1,691 |
| m60 a → b (idle, before load) | 101 | 1 / 3 / 5 | 68 / 74 / 105 | 0 / 0 | 108 / 115 |
| m56 a → b (load, 07:32–07:33) | 125 | 39 / 103 / 140 | 100 / 321 / 558 | 33 / 167 | 338 / 873 |
| m56 a → c (load, 07:33–07:35) | 161 | 36 / 134 / 248 | 93 / 394 / 648 | 66 / 194 | 450 / 1,093 |

- **The follower's queue is small.** It is p50 23 to 39 ms and p90 103 to 139 ms on both rows, under load.
- **The follower is not busy with reads.** Voter followers took 4 to 6 `/read` a second (learners took 109,624 of the
  row's 114,498 member `/read`s). The follower DOs used 100 to 104 ms of CPU a second, with no ticker lag. The out leg
  does not track the follower's open requests at entry (r 0.02 and 0.11) or its `/read`s in the second before
  (r −0.09 and −0.01).
- **The hypothesis does not hold on m60 or m56.** Whether Cloudflare's invocation start includes a wait at the
  object's input gate is undocumented [unverified]. A follower at 10% CPU and 0 loop lag leaves little room for one.
- **What remains is the out leg, and it follows the leader.** Calls the leader sent within 200 ms before or during
  its own ticker stall (≥ 100 ms) had out p50 592 and 484 ms, against 270 and 292 ms for the rest. On m56, whose
  leader had no such stalls, the out leg was p50 93 to 100 ms. The member clock's own skew makes out uncertain by about
  ±200 ms (m56's p10 is negative), so the split between wire and the leader's late send is [inference].

**The leader's isolate is where m60 lost time.**
- **Ticker stalls.** Leader c logged 51 stalls of 20 ms or more in 224 s as leader, summing 30.0 s (p50 423 ms, max
  1,664). b logged 14 in 45 s (3.4 s), and m60's a logged 4 in its 16 s as leader. m56's leader a logged 3 in 289 s
  (0.2 s).
- **CPU.** Leader c used 438 ms of CPU a second (Workers Logs, 14:41–14:44): `/commit` 227, `/raft` 113, `/outcome` 50.
  It ran 113 invocations of 200 ms CPU or more and 25 of 400 ms or more. m56's leader a used 513 ms a second, but with
  94 and 6.
- **Stalls inside long invocations.** 33 of c's 38 stalls of 200 ms or more lie inside invocations of 150 ms CPU or
  more, whose CPU covers at least half the stall: `/commit` 95, `/raft` 44, `/outcome` 24 of the 188 such invocations.
- **Larger entries.** c's cuts were larger: p50 0.96 MB against m56's 0.48 MB, with 58 EntryMax cuts at 1.49 MB. In
  the 3 s before a stall, a cut of 1.4 MB or more came 52% of the time, against 31% at random instants.

So m60's slow trips come from the leader's long turns, which delay sends, answers and the deferred flush, plus a
trans-Pacific wire. The likely loop [inference]: slow rounds queue more, larger entries make each turn longer, and
longer turns make the rounds slower. What fills a long turn is not measured: an isolate's clock does not advance
inside a turn, so the trace cannot time it.

### Proposal 1: measure the leader's turn, then cut per-entry work

1. **Instrument** (test-faults, site script, so it ships on a site day). A `Trace::Work` row after each input step on
   a leader. It records the input's route, entries encoded and their bytes, statements queued, and bytes rendered into
   replies. No clock is needed: it is joined to ticker stalls by order and to Workers Logs CPU by time.
   - Test: the row packs and unpacks within 2^53 (the `Cut` row's rule); a step that encodes one 1 MB entry for two
     followers reports its bytes.
2. **Candidate fix, ranked by the evidence so far:** encode each entry once on the leader. Today one AppendEntries
   encodes its entries in `append_batch`, again to count `calls_out` bytes (`leader.rs`), and the host encodes held
   learner messages again in `route` (`host.rs`). With a 1.5 MB entry, two followers and four learners, that is up to
   about 18 encodes of the entry, before the envelope's own `codec::to_bytes` per call. I have not measured whether
   this fills the long turns. Step 1 decides.
   - Test, through `Member::step`: a leader with 2 followers and 4 learners sends one entry and encodes it once. It
     fails on main's 3 per peer. Counted by a test-only hook.
   - Plant: restore a second encode in the `calls_out` byte count.
   - Raft sweep plus Porcupine on `hist.jsonl` and `.gray`, because it touches `crates/raft/src`.
   - Hosted flip: the leader's ticker stall total per minute not falling at equal cut sizes.

## 2. Why the leader went a → b → c and stayed on apac

**The target is not picked by confirm speed.** `step_down_to` (`crates/raft/src/member/health.rs`) keeps candidates
that may lead, are outside RESET_QUARANTINE, are not `peer_storage_slow`, and (on a Confirm trip) have no failing
link. It sorts them by fewest failed calls, then the most caught up. Home and distance play no part.

What happened on m60:
1. **14:39:39, a (home, enam) trips Confirm 6 of 18.** Its `sync()` was not slow: 25 syncs in the window, 2 over
   500 ms. The rule's samples run from `confirm_begin` at step to the end, so they include the deferred flush and the
   turn queue. Both b and c qualify, and b (weur) wins on catch-up. `end_leadership` sets a's `stepped_down_at`.
2. **14:40:23, b trips Confirm 6 of 16** (1 sync over 500 ms in the window). a is inside RESET_QUARANTINE of its own
   step-down, so c (apac) is the only candidate. b's `stepped_down_at` is set too.
3. **14:40:27 to 14:44:44, c leads.** Every trip finds both followers quarantined, so all 17 are holds (10
   CommitErrors, 7 Confirm).
4. **14:44:44, c transfers to a, 305 s after a's step-down.** `maybe_hand_off` moves to `home` once home is outside
   its quarantine, answered 5 calls with none failed, and the leader is not tripped. a's quarantine ended at 14:44:39.
   There was no StepDown alarm with a target from c, so this was the home hand-off, and it came as soon as the rules
   allowed.

So the far leader is what RESET_QUARANTINE does to a three-voter group after two load-driven Confirm trips. After one
step-down from home, the next step-down has one candidate (the third site) or none. A preference for home or for the
site near home cannot act: the only choice it would have is the one quarantine already removed.

**The trips were load, not storage.** A rule on `sync()` times alone (the same p99 > 500 ms and 6 slow in 10 s) trips
once on m60, on b at 14:40:00, against the 10 Confirm alarms. On m56 it trips once, against 5. On m60 the a → b and
b → c trips would not have fired, and the group would have kept its home leader, which is m56's shape (cut gap mean
546 ms against 852) [inference from replaying the rule on the trace].

### Proposal 2: judge the confirm rule on storage, not on the wait for the flush

- **Change.** The confirm a `STEP_DOWN` sample measures starts when the transaction runs (`flush`), not when the input
  stepped. The member keeps `confirm_begin` at step for the own-stall exclusion. A new input,
  `confirm_flushed(now, id)`, marks the start of the sample the rule judges. This also leaves LoopLag (loop stalls) and
  CommitErrors (late calls) to the two rules built for them.
- **Design first.** `design/DESIGN.md` Shape, the `STEP_DOWN` comment ("storage confirm p99"), and a DECISIONS entry
  that amends "`STEP_DOWN`'s confirm rule needs six slow confirmations".
  - F35's evidence is bare objects "writing and syncing", which matches `sync()` time.
  - Flips: a B0-06 gray-storage leader left in place past 20 s; a B0-33 Confirm trip whose window's `sync()` p99 is
    under 500 ms; a home leader leaving home with no slow `sync()`.
- **Not proposed:** shortening RESET_QUARANTINE for `stepped_down_at`. It is what stops two healthy sites passing a
  group back and forth while a third is sick (same DECISIONS entry, "Why"). Under load the home leader would trip
  again and bounce every 10 s. A home-first or near-home tie-break also changes nothing in a three-voter group (above).
- **Test plan.**
  - Member tests in `health.rs`, through `step`, `confirm_begin`, `confirm_flushed` and `confirm_end`:
    - a leader whose flush waits 800 ms after step but whose `sync()` takes 50 ms, six times in a window, does not
      trip. It fails on main.
    - a leader whose `sync()` takes 800 ms six times trips Confirm and transfers (the B0-06 guard).
    - a call whose deadline passes during a slow `sync()` is still excused (own-stall).
  - Plants:
    - the sample measured from step again;
    - `confirm_flushed` ignored;
    - the own-stall check moved to the flush begin.
  - Because `crates/raft/src` changes: `RAFT_HISTORY=/tmp/sw/hist.jsonl RAFT_SEEDS=2000 RAFT_STEPS=10000` sim sweep,
    then Porcupine on `hist.jsonl` and `hist.jsonl.gray`.
  - Full checks: fmt, clippy, the wasm32 clippys with and without `test-faults`, `cargo test --workspace`, Go, and
    the three `check_*.py`.
  - Hosted (when granted): one site day, then B0-33 on the m56 script. Expected: no Confirm transfers off home, and
    the cut gap mean near m56's 546 ms.

**Ranking.** Proposal 2 first. It keeps the leader on its home site, where m56 ran with 0.2 s of leader stalls and
135 agent commands a second in a minute. Proposal 1's instrument second, because it is what can show whether
per-entry work, and not only the far placement, makes a loaded leader's turns long.

## Built on `confirm-flush` (2026-11-19, code only, no hosted row)

- **Proposal 2, `confirm-storage`.** The host times each deferred flush from its transaction to `sync()`'s end and
  reports it with `Member::confirm_end_timed`. The confirm rule counts that time; LOOP_LAG, the own-stall exclusion
  and `kept` still read step-to-end. A trip's `stepDownWindow` log line now carries `confirms` (storage) and `whole`
  (step to end), so a hosted row shows both. Design: `design/DESIGN.md` Shape (`STEP_DOWN`), `decisions/DECISIONS.md`
  "`confirm-storage`". Tests in `crates/raft/tests/leadership.rs`; plants `tests/m0carried/plant53.py`.
- **Proposal 1's instrument only.** `Trace::Work` (kind 30, test-faults) per leader input step that sends, writes,
  applies or resolves: AppendEntries with entries and their entries' bytes per message, statements queued and their
  bytes, entries and events applied, resolutions. Reply bytes are not measured: the step does not render replies.
  `tests/fanout/work.py` reads it and sums the bytes of work inside each ticker stall. Encode-once is not built
  (F100).
- **Checks.** The first test (`a_long_wait_for_the_flush_over_quick_storage_does_not_trip_the_confirm_rule`) fails on
  main's step-to-end rule; plant53 caught 3 of 3. Long sweep (2,000 seeds × 10,000 steps): harsh 0 step-downs and 8,206
  holds, as main; gray 2,138 step-downs against main's 2,117, longest stuck stretch 28,501 ms (seed 11893) against main's
  24,153; live 1,791 against 1,764, 25,191 ms against 29,145, all under the 30 s bound. The main figures are from an
  earlier sweep (06:16 that day, before later merges). Seed 11893 alone, with the member fed main's `confirm_end`
  instead, also stalls 28,501 ms, so that stretch is not this change. Porcupine Ok on `hist.jsonl` (146,836 operations)
  and `hist.jsonl.gray` (2,335,345).

## Files

- `tests/fanout/applegs.py`: fetch and join for the table above.
- `tests/fanout/raw/applegs-m60/`: the five Workers Logs extracts (call id, stamps, end, wall, CPU per invocation).
