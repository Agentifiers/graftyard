# (c): the leader's AppendEntries under B0-33 load, a design for review

Branch `c-design`. This is a design only. It changes no code in `crates/raft`, the storage path, or `workers/site`.

The orchestrator (T-01a17bda) chose (c) after point 2 of the voter-shed review ruled out the shed
(`voter-shed-design.md`, "Point 2"). It asked for two levers, weighed separately, each with a number:
- (c1) fewer commit waiters open at once;
- (c2) sends clear of the output gate.

It also asked which recorded rejection each lever reopens, and whether today's evidence meets that entry's flip.

All numbers below come from m47's trace: `raw/tsave-m47b33.jsonl.xz`, B0-33 at 5,000 agents, load 01:00:18 to
01:05:18Z. Each member is counted only while it led, from `terms-m47b33.jsonl`. The scripts are
`tests/fanout/votershed/p3*.py`, run on `/tmp/shed/rows.pkl` from `prep.py`.

"Round trip" means the skew-free round trip of a leader-to-follower `/raft` call: `CallSent` to `AnswerArrived`, on
the leader's clock, less the follower's own `Entered` to `CallLeft`. Over 1,748 answered calls the follower's part
was 0 ms at p50.

## Summary

| lever | what it changes | expected out-leg change | risk | rank |
|---|---|---|---|---|
| (c1a) answer `/outcome_by_request` from a follower or learner's `request` row | moves 9 to 12k of the leader's 27.6k `/outcome_by_request` (the walk's) off it | small: these held 13 to 20% of entries at ≥ 64 open but little of the open count (they don't wait) | low: read-only, edge plus member, no Raft change | 3 |
| (c1b) a per-isolate cap on `/commit` and `/outcome` waiters held open on the leader, answering `admitted` early | the commit family's 80 to 100% of the non-`/raft` open | **withdrawn on review**: the release it targets is 9% of stall time, and its polls add 267 to 2,780 req/s at m47's tickets (see "(c1b) under review") | reopens F39's fold | — |
| (c2) leader sends clear of the output gate | at most the gate's share, 14 to 37% of the round-trip excess | **medium, bounded by the gate share**; sends issued in the apply's release still wait out that release | high: storage rewrite (KV `allowUnconfirmed`), reopens `m0-carried-4`'s Beat, and the flip isn't met | 2 |

Ranking by expected out-leg change per unit of risk first put (c1b) first, (c2) second and (c1a) third. The review below withdraws (c1b), shelves (c2) in DECISIONS, and parks (c1a) in OPEN-QUESTIONS.

(c2)'s ceiling is the share of round-trip excess that the measured gate time covers. That ceiling is lower than
what the commit waiters hold, and (c2) is a storage rewrite that today's evidence doesn't justify. Details follow.

## What holds the leader's sends (new evidence)

### 1. The JavaScript-side gate time explains less than half of the excess

`Confirm` rows time `sync()` from the call to its callback. A send issued while one was pending is "gate closed".

| leader open at send | n | round trip p50 / p90 | share of excess over 300 ms covered by the gate time left at send |
|---|---|---|---|
| < 64 | 220 | 285 / 2,791 ms | 14% |
| 64 to 299 | 567 | 390 / 4,378 ms | 32% |
| 300+ | 961 | 990 / 6,586 ms | 37% |

There is a second check, on sends issued while a confirm was pending. 97% of them entered the follower after that
confirm's callback ran, and p50 1.8 to 3.2 s after it, at 64 open or more (`p3a.py`). The callback is itself a turn
on the leader. So the gate does release the send, but the send then still waits about as long as the callback did to
be run. The gate's real share is somewhere between 14–37% and the share of time those turns waited.

### 2. The leader's isolate stalls, and the stalls start where commit waiters are released

The member's 100 ms ticker records a `Lag` row when it wakes late. It shows stretches in which the isolate ran no
turn for the ticker. Workers' clock moves only on I/O, so a stretch reads as one long gap between stamps. Its own
rows show at most a few distinct clock values (p50 3 to 7 inside a stall of 300 ms or more, `p3l.py`).
- **Ticker stalls of 100 ms or more:** 23.4 s on member 0 over 87 s of leading, 5.1 s on member 1 over 88 s, and
  5.6 s on member 2 over 44 s (`p3e.py`).
- **How they overlap the round trips:** 72% of calls at 300+ open overlap a ticker stall, and stalls cover 54% of
  those calls' excess over 300 ms. At 64 to 299 open, 29% of calls overlap one, covering 11%. Calls without a stall
  run p50 269 to 294 ms; calls with one run p50 577 to 1,553 ms.
- **What fills the stall:** inside the stalls, edge `/commit`, `/outcome_by_request` and `/read` entries arrive at 3
  to 7 times the leader's average rates. `/raft` answers (`CallLeft`) arrive at 3 to 8 times (`p3l.py`). The isolate
  is taking requests in, not answering.
- **What comes just before a stall:** a stall of 300 ms or more starts within 100 ms after an `Applied` row (the
  release that resolves an entry's waiters) or a confirm callback in 47% of cases on member 0, 40% on member 1 and
  25% on member 2. At random leader times the same holds 6 to 9% of the time (`p3i.py`). By time, a third of all
  stall time on the three leaders (33% of 34.0 s) starts there (`p3k.py`).
- **The release itself holds the isolate.** After an `Applied` row, the next stamped row comes p50 3 ms later when
  that apply resolved fewer than 20 `/commit` waiters. It comes 34 ms later at 20 to 149, and 100 ms (p90 335 ms)
  later at 150 or more. Gaps between rows are otherwise p50 5 ms (`p3m.py`). Applies resolved p50 73 and p90 493
  `/commit` waiters on member 0, up to 1,888.
- **The sends in that release go out late.** AppendEntries sent in the same release as an `Applied` row:

| `/commit` waiters that release resolved | n | out leg p50 / p90 (skew-corrected) | release hold p50 / p90 | leader open p50 |
|---|---|---|---|---|
| < 20 | 45 | 2,378 / 4,152 ms | 50 / 226 ms | 131 |
| 20 to 149 | 79 | 1,344 / 3,508 ms | 27 / 109 ms | 301 |
| 150+ | 144 | 4,139 / 6,597 ms | 100 / 272 ms | 751 |

  (`p3p.py`.) Measured over the round trip instead, these were p50 185, 279 and 1,335 ms (`p3o.py`).

[inference] The leader's resolution loop sends every resolved waiter's `oneshot` in one turn
(`host.rs` `release`, `Resolution::Ticket`). Each woken `/commit` handler then runs `self.done(&o).await` in turn:
`reply_json`, the outcome's render, and `settled()`. Each `/commit` reply that leaves is one more message the platform
pushes through the output gate. This is the queue the AppendEntries joins. The trace doesn't stamp when a woken handler
runs, so this is an inference from the stalls' start points and the release's hold. It isn't measured per handler.

### 3. The commit waiters are the open count

- **The rates.** The leader took `/commit` at 120, 40 and 102 per second (members 0, 1 and 2), `/outcome` at 22, 4
  and 28 per second, and `/outcome_by_request` at 130, 68 and 123 per second (`p2*`, here `p3*`).
- **What each one costs in open count.**
  - `/commit` waits up to `COMMIT_HOLD` (1 s), so it holds at most 120, 40 and 102 open.
  - `/outcome` waits up to `wait_ms` (10 s), so it holds at most 217, 43 and 283.
  - Each also waits `settled()` and its turn, which pushes the measured commit-family residence to 1.0 to 3.3 s
    (point 2).
  - `/outcome_by_request` has no wait of its own. It still enters at 3 to 7 times the average rate inside stalls.
- **Who sends it.** By `SentBy`:
  - `/commit` is the edge's send (16,156) or a walk (2,255).
  - `/outcome_by_request` is the receipt's `ask` (12,191), the walk for the leader (9,002), or a direct post to
    the cached leader (1,477).
  - `/outcome` comes from `commit_hedged`'s `admitted` path (3,137).
- **Tickets.** The leader's open tickets (`memguard`, `inFlight.admitted`) ran 140 to 1,465 during load. The round
  trip rises with them: p50 257 ms at 100 to 299 tickets, 679 ms at 300 to 599, 795 ms at 600 or more (`p3n.py`).
  The correlation is weaker (0.32) than with the open count, because the 2 s samples are coarse.

### 4. Storage isn't the bottleneck

- The leader's `confirmSplit` on m47: storage p50 36 to 41 ms, turn p50 0. The p99s of 3.2 to 4.9 s line up with
  the stalls above.
- Confirm duration doesn't grow with bytes: p50 36 to 40 ms at 100 to 500 KB with the leader quiet (`p3d.py`). It
  grows with the open count.
- Log bytes to one follower ran 18 to 158 KB/s.

## (c1) Fewer commit waiters open at once

### (c1a) `/outcome_by_request` and `/outcome` from a follower or learner

**What it would take.**
- `outcome_by_request` today is leader-only (`raft::Member::outcome_by_request`). A row lookup through
  `fold.request(id)` is the same on any member at or past the row's index.
- A follower or learner could answer `done` from its applied `request` row. It needs a commit proof for `replay`.
  It could use the leader's latest proof it holds (`AppendEntries.commit` plus the entry's proof), or answer with a
  read-index floor, as feed polls do.
- `unknown` on a follower means "not applied here yet", not "not admitted". The edge would have to treat that answer
  as "ask the leader", not as `unknown`. `pending` exists only on the leader, so a follower can't answer it.

**What it moves.**
- On m47 the walk's `/outcome_by_request` was 9,002 and the cached-leader posts 1,477. Those are the share a
  follower could take: about 38% of the leader's `/outcome_by_request`, and 12% of its entries at 64 open or more.
- The receipt's `ask` (12,191) must reach the leader, or its 200 no longer means "the leader answers"
  (`receipt`'s `busy` rule).
- `/outcome_by_request` holds little open time of its own: point 2's fit gave it 0 to 6.2 s on member 0, and it was
  not separable from `/commit`.

**Expected out-leg change:** small [inference]. It removes entries, not waiters.

**Risk:** low. It's a read-only path, but it adds a new answer shape the edge must handle, and it touches T-01a1d805's
`crates/lookup` walk.

**Client latency:** none on the commit itself. A walk answered by a follower saves one hop.

### (c1b) Cap the commit waiters a leader holds open

**What it would take.**
- The edge already handles a `/commit` that answers `admitted` before COMMIT_HOLD: it follows up with `/outcome`,
  and on `pending` it sends `/commit` again under the same request id, which joins the ticket (`commit_hedged`,
  `None => why = "same"`). Today that loop has no pause.
- The change is a per-member count of `/commit` and `/outcome` handlers waiting on a ticket. Past `WAITERS_MAX`
  [guess], a new `/commit` answers `admitted` as soon as it is admitted, with no hold, and a new `/outcome`
  answers `pending` at once, carrying a `retry_ms`.
- The lever is the hold time, not admission. Every command is still admitted and decided as today.
  `Acked` is untouched, since `pending` and `admitted` are not 2xx outcomes for the client (`receipt` takes
  `pending` as `admitted`).

**What it moves.**
- The commit family's 80 to 100% of non-`/raft` open (point 2).
- The waiters a release wakes: p50 73 and p90 493 per apply on member 0.
- A cap of, say, 128 waiters [guess] would bound each release's woken handlers to 128. On m47 that bounds the
  release hold, which was 100 ms (p90 272) at 150+ resolved and 27 ms at 20 to 149.
- The round trip at < 64 open was p50 285 ms; at 300+ it was 990 ms. Sends in a release that resolved 20 to 149
  waiters went out p50 1,344 ms against 4,139 ms at 150+.

**Expected out-leg change:** the largest available.
- [inference] If the open count stays under a few hundred, the out leg follows the m47 table: p50 under 1 s, against
  3 to 4 s at 400+.
- This is a hypothesis to measure, not a measured result. The only data are m47's own bands. The first run should
  compare the out leg at the same offered load, with the cap on and off.

**The cost the client sees.**
- An agent push answered `admitted` waits on `/outcome` polls. Each poll is one more call to the leader, which
  is the same load moved elsewhere.
- To avoid that, the `/outcome` past the cap is itself answered `pending` with a `retry_ms`, and the edge sleeps
  before polling. Client commit latency then rises by at most one `retry_ms` per poll.
- B0-33's commit p99 on m47 was 22.5 s (`commitMs` p50 5.2 s), so a 250 ms `retry_ms` [guess] is noise on that row.
- On B0-29 the leader's open p50 was 9 and its max 145, so a cap at 128 or more barely acts there.

**Risk.**
- Low to medium: an edge and member change, with no `crates/raft` change.
- The new rule must not turn a pending command into an error, or into an `ng` outside B0-32's bound shapes. `pending`
  isn't an outcome, so a push still gets its real outcome by polling.
- Test: a leader past the cap still answers every waiter's eventual outcome, and a `/raft` answer arrives ahead of
  the waiters' release. It must fail on main by the release-hold measure.

## (c2) Sends clear of the output gate

**What it would take.**
- SQL has no `allowUnconfirmed`. The output gate holds every outgoing message, AppendEntries included, until all
  earlier writes confirm (`m0-carried-6`'s amendment).
- In workerd `8f9bacf`, `ActorSqlite::onWrite` locks the output gate for any write without `allowUnconfirmed`. The
  SQL API (`api/sql.c++`) never passes `allowUnconfirmed`. KV `put` on a SQLite-backed object does pass it
  (`api/actor-state.c++` 488). [doc] The SQLite storage page lists `allowUnconfirmed` for KV `put` and `delete` on
  SQLite-backed objects.
- So the only way for a send to clear the gate is to make every write that precedes it a KV `put` with
  `allowUnconfirmed`, followed by an explicit `sync()` before anything that rests on durability. That covers the log,
  the Raft state and the tier rows (`stmt_mix`: `INSERT log`, `raft_state`, `request`, `m_*`, `c_*`).
- That is the storage rewrite `m0-carried-4`'s Beat rejected. It also needs a second `Store`, a new restore path, and
  a migration of every existing member's SQL rows.

**Safety argument.**
- A follower's ack must leave only after its write is durable. Its `/raft` reply would wait on an explicit
  `sync()` in place of the gate. A leader counts itself only after its own confirm, as today.
- `proof_for`'s `lead.matched` is correct only while each ack waits for its write. "Not adopted: a learner's writes
  clear of its own output gate" found exactly this dependency.
- Every reply path (`ready_or`, `raft`, `outcome`) would need an explicit `sync().await` in place of `settled()`'s
  implicit gate. A missed one acks an unconfirmed write. Testing can only catch that with a fault that loses an
  unconfirmed write, which the sim can model but the hosted platform can't inject.

**Blast radius.** Every member write and every reply path. It reopens:
- `m0-carried-4` Beat (KV path). Flip: "Hosted B0-02 runs whose stall windows still line up with a write during a
  body". **Not met.** The stall here is the isolate's turn queue and its gate, not a body in flight during a write.
  m47's `confirmSplit` storage p50 was 36 to 41 ms.
- `m0-carried-6` amendment (C20). This lever is C20's open question. C20 has no flip; it asks how.
- "Not adopted: a learner's writes clear of its own output gate". Flip: "a measured learner-gate share above 30% of
  the gap". That entry is about learners. Here the leader's gate share is 14 to 37% of the round-trip excess, near
  that bar for the leader's sends, but no flip is written for the leader.

**Expected out-leg change.**
- At most the gate's share, 14 to 37% of the excess. The sends in an apply's release would still wait out that
  release's turn, and those are the slowest (4.1 s p50 at 150+ resolved).
- [inference] Some of the "gate" time is the confirm callback waiting for its turn behind the same waiters, so
  (c1b) shrinks (c2)'s share as well.

**Risk:** high. It's a storage-layer rewrite, its evidence doesn't meet its flip, and the safety argument rests on
every reply path awaiting `sync()`.

## Recommendation

1. **Build (c1b)** behind a test-faults switch, and run B0-33 with it on and off on the same day. Before any code I
   send the cap rule, `WAITERS_MAX` and `retry_ms` [guess], and the test that fails on main. It touches
   `workers/site` (the waiter count and `admitted`/`pending` answers) and `crates/lookup` `commit_hedged` (honouring
   `retry_ms`). The lookup part is outside T-01a1d805's `spread_once` and `spread_verdict`.
2. **Shelve (c2)**, with its flip: a run where (c1b) holds the leader's open count under its cap and the gate's
   share of the round-trip excess is still above 30%.
3. **Hold (c1a)** as a follow-up for load on the walk, not for the out leg.

Before (c1b) is measured, the trace needs one test-faults row: the woken waiter's handler start (`/commit` and
`/outcome`, when `within` returns). The (c1b) run then measures the release queue directly instead of inferring it.
That needs a site day, so it ships with (c1b).

## (c1b) under review: the three questions, and a correction

The orchestrator approved (c1b)'s direction and asked three things before code: the poll load a cap adds against the
fold's 750 req/s history, bounded-batch release as the cheaper alternative, and the request-id path that keeps a
pending-to-resend loop from committing twice. Answering them meant testing the claim (c1b) rests on: that a release
waking hundreds of `/commit` waiters is what stalls the leader. **That test fails.** The release is a minor share of
the stalls, so (c1b) should not be built on this evidence. Scripts: `votershed/p4*.py`.

### The correction: what the stalls are made of

- **The release is 9% of stall time, not a third.** Of 34.0 s of leader ticker stalls of 100 ms or more, 3.2 s falls
  inside the gap right after an `Applied` row (`p4g.py`). Only 24 of 72 applies were followed by a gap of 100 ms or
  more. The earlier "a third of stall time starts within 100 ms after an apply or confirm" (`p3k.py`) counted a window
  around the start, which dense traffic fills at random.
- **The turn that starts a stall rarely holds an apply.** The last stamped clock value before a stall's start carried
  an `Applied` row in 2 of 17 stalls of 300 ms or more (12%). Of all frozen-stamp bursts of 100 or more rows while
  leading, 9% do (`p4e.py`). That is the base rate, so the release is not over-represented.
- **Stalls are made of inbound requests.** 38,210 requests entered during the 36 stalls, about 1,000 to 1,124 a
  second. A stall's length tracks how many entered in it: Spearman 0.84. By route: `/read` 0.72, `/commit` 0.66,
  `/outcome_by_request` 0.65, `/raft` 0.64, `/suspect` 0.39 (`p4c.py`). Their mix is close to the leader's average:
  `/read` 41% against 50%, `/outcome_by_request` 23% against 15%, `/commit` 18% against 12%, `/suspect` 7% against
  14%.
- **Stalls grow with the request rate.** On member 0 a second's stall share was 8% at under 300 requests entered,
  25% at 300 to 599, and 37% and 34% at 600 to 899 and 900 or more (`p4f.py`).
- **The leader handled 563 to 846 req/s while leading** (members 1, 0 and 2), 6 to 14 requests per `/commit`
  (`p4d.py`). The largest streams were the walk's `/read` (201 to 336/s), `/commit` sends (14 to 120/s), `/suspect`
  (66 to 131/s) and the receipt's `/outcome_by_request` asks (9 to 88/s). That is the 750 to 950 req/s range in which
  the fifth pass lost 2 of 3 runs to elections.
- **CPU.** The trace records no CPU time per request. LM-1's earlier note measured member 0 at 0.28 to 0.40 CPU s per
  s at 357 to 666 req/s. The ticker stalls say turns were queued, not why. [inference] Per-request isolate overhead
  at about 1,000 inbound a second fills the isolate's turn queue whatever the route. That fits the stall length
  tracking total requests better than any one route.

So this design's earlier "what holds the leader's sends, 2." overstated the release. The commit family still holds the
open count (point 2), but it doesn't hold it by the release: it holds it by waiting, and in waiting it costs nothing.
What fills the turn queue is the leader's inbound request rate.

### 1. The fold and the poll load a cap adds

Leader open tickets on m47 ran p50 395, p90 823, max 1,465 (`memguard`, 72 samples; `p4e.py`). B0-33's sixth pass
held up to 576 calls at once. A cap at C waiters turns the rest into polls at one per waiter per `retry_ms`:

| cap | waiters | added polls/s at retry 250 ms | at 1 s | at 2 s |
|---|---|---|---|---|
| 128 | 576 (sixth pass max) | 1,792 | 448 | 224 |
| 128 | 395 (m47 p50) | 1,068 | 267 | 134 |
| 128 | 823 (m47 p90) | 2,780 | 695 | 348 |
| 128 | 1,888 (m47 max per apply) | 7,040 | 1,760 | 880 |
| 512 | 823 | 1,244 | 311 | 156 |
| 512 | 1,888 | 5,504 | 1,376 | 688 |

The leader already serves 563 to 846 req/s at m47. The fold's history is 750 to 950 req/s with 2 of 3 runs lost.
No retry under 2 s keeps the added load "well under" that line at m47's p90, and polls at 2 s add up to a full second
to every commit past the cap. Moving the polls off the leader means (c1a), a follower answering `done` from its
`request` row. That only answers after the entry applies there, so a poll before then must still reach the leader for
`pending`, or the edge waits blind.

**Flip check.** F39's settled answer is that holding `commit` cut the leader from 750 to 950 to 459 to 498 req/s. Its
open item is `COMMIT_HOLD`'s value, and B0-33's line is "`mitigated` until the leader's request rate falls". A cap
past which commits poll is a partial unfold. With the stalls tracking inbound rate, it moves the leader back toward
the rate that lost the fifth pass. It reopens F39's evidence rather than staying clear of it. The cap does not
survive question 1.

### 2. Bounded-batch release

Releasing a resolution's waiters in batches of B, yielding a macrotask between batches, bounds one release turn.
On m47 it would act on at most the 9% of stall time that follows an apply. It also delays the last waiters' replies
by one turn per batch, and the AppendEntries in that release only by what the batches ahead of it take. That is
cheaper than the cap: no added requests, no edge change, no fold question. It still targets the minor part, so the
expected out-leg change is small, about the 9% bound [inference]. **Batches beat the cap; neither is worth building
on m47's evidence.**

### 3. Idempotence (for the record)

The resend path cannot commit a command twice, whatever the leader answers first:
- `Leader::admit` looks up `by_request` and joins an open ticket under the same `RequestId`: `joins` → the same `seq`.
- A command whose id already has a `request` row skips admission and is resolved from its row:
  `fold.request(id).is_some()`, then `drop_replays` builds `Acked` through `outcome::replay` from the stored events.
- So a resend after `admitted` or `pending` joins or replays, and `Acked` is built only inside `outcome` and
  `replay`.
- One gap is recorded, not new: `by_request` lives in the leader's memory, so across a leader change an un-applied
  command is admitted again under the new term. That is the existing `NotLeader` → resend rule, decided at most once
  because only one entry can carry the id's row (`drop_replays` at cut time).

### What I recommend instead

Don't build (c1b). The lever the stall evidence points at is the leader's inbound rate per commit: 6 to 14 requests
per `/commit` on m47, against the fifth pass's 750 to 950 req/s history. The streams that are not the command itself:
- the walk's `/read`, 201 to 336/s on the leader;
- `/suspect`, 66 to 131/s;
- the receipt's `/outcome_by_request` asks, 9 to 88/s.

Each has a design owner already:
- the walk and its `/read` belong to `crates/lookup`'s `call_opt`, B0-32's shed and T-01a1d805;
- `/suspect` damping belongs to the late-suspect and progress-suspect rules.

The next step is a measurement, not code: on the next B0-33 run, read the leader's req/s by `SentBy` step beside the
out leg. That needs no site day, since the rows exist. Then pick the stream to cut.

## The B0-33 measurement, m50 (2026-11-18)

Granted by the orchestrator after (c1b) was dropped: B0-33's 5,000-agent row on graft-m2b from its current deploy (site
day m49, main 94fdfb48; edge 10a49946 at 100%), with no site day and no code change. The task was to read the leader's
req/s by lookup step beside the out leg and its stall share, and to name the inbound stream to cut first.

**Run.** `costrow.sh` row b033, tag m50b33: repo `fm50b33-b033` (131358), a1 `mwlyot52`, 5,000 agents, 300 s of load
from 10:17:37.7Z, leader site 0 at term 2, k 4, 8 learners. Memguard peaks 67.2, 90.3 and 46.1 MiB, with no 120 MiB
stop.

| B0-33 line | m50 | m47 | verdict at 90% |
|---|---|---|---|
| Human and merge-queue commit p99 ≤ 1 s | leader commit p99 7,264 ms (p50 2,981, n 5,164); merges 19 ok of 28 attempted, p99 20.1 s; human pushes 82 ok of 90 attempted (95 skipped at the cap), p99 30.8 s | 22,536 ms; merges 1 of 20; human pushes 10 of 24 | fail |
| ≥ 140 agent commands/s (≥ 126 at 90%; fail < 100) | **90.7 agent-class ok a second** (27,209 over 300 s: stand-ins 9,120, comments 5,428, task edits 4,732, lease renews 4,095, leases 1,949, agent pushes 1,290, draft PRs 595) | 17.2 | fail |
| User token admitted as `Agent` | fast user 513 of 513 × 200 | 37 of 482 | pass |
| Every refusal in the `ng` or 429 shape | agent pushes: 11,216 `full` (the `ng` shape) and 0 other refusals; other requests 943 × 5xx of 47,574 (2.0%) | 503s and 500s dominate | fail on the other requests (2.0%) |
| Zero leader changes and zero `suspect` | **1 change** (term 2 → 3 at 10:18:24Z, 46 s into load); 1,631 `suspect` reports stepped at the voters (one colo, 1,618; all `promised` or `leader`) | 19 changes; 41,356 | fail |

**Better than m47 on every line, and on the same code.** This deploy is main 94fdfb48. It has progress-suspect, lookup-retry
and clock-guard, which m47 (branch lookup-retry 4e5c2afe) did not have. Agent pushes were refused cleanly at admission
(`full`) instead of failing on 503s. With one term change in place of 19, the walk's `/read` and `/suspect` almost
disappeared from the leader.

**The one term change.** In the 10 s before it, leader 0's storage confirms ran 593 to 1,193 ms (11 of 17 over
100 ms). Its ticker stalled up to 1,341 ms (10:18:25Z), and the followers saw append gaps of 1.0 to 2.0 s
(10:18:04 to 10:18:24Z). Reports came from one colo, and every verdict stepped was `promised` or `leader`: no report
started a campaign. So the change came from a lapsed promise after appends stopped. That matches the `STEP_DOWN` loop-lag or
progress rules, not edge reports [inference: the trace has no `Elect` rows for this change].

### The leader's inbound by stream (`votershed/bystep.py`, `raw/bystep-m50b33.txt`)

283 leader-seconds in the load window (member 0 for 40 s, then member 1 for 243 s), 496 to 510 req/s while leading.

| stream (route:step) | req/s | share | share in the 103 seconds with 30%+ stall |
|---|---|---|---|
| `/commit` send | 169.9 | 34.1% | 27.5% |
| `/raft` from learners | 134.7 to 148.6 | 28.2% (all `/raft`) | 29.0% |
| `/outcome` (after `admitted`) | 116.2 | 23.3% | 22.6% |
| `/outcome_by_request` ask | 59.1 | 11.9% | **19.3%** |
| `/read`: walk, leader, spread | 6.9 | 1.4% | 0.9% |
| `/suspect` | 0.6 | 0.1% | 0.0% |
| voter `/raft` | 2.4 to 12.1 | (in `/raft`) | |

**Stall share against req/s** (the share of each leader-second the ticker sat 100 ms or more late):

| requests entered that second | seconds | stall share | out leg p50 / p90 |
|---|---|---|---|
| < 300 | 70 | 13% | 1,444 / 2,892 ms |
| 300 to 599 | 129 | 31% | 1,241 / 2,847 ms |
| 600 to 899 | 61 | 45% | 1,948 / 3,144 ms |
| 900 to 1,199 | 8 | 48% | 1,857 / 2,543 ms |

Total leader stall time was 87.6 s of 283 (31%), against m47's 34.0 s of 219 (16%), and at lower req/s. The out leg
(`raw/outleg-m50b33.txt`) was p50 1,291 ms at 500 to 999 open and 1,507 ms at 1,000+. The skew-free round trip at 300+
open was p50 421 ms, p90 1,308 ms (m47: 990 / 6,586 ms). Of its excess over 300 ms, the output gate left at send took
25%, loop lag at send 20%, and the rest 56% (`p2p.py`).

**Which stream leads the stalls** (`votershed/bylag.py`, `raw/bylag-m50b33.txt`). Spearman of a stream's count in a
second against stall ms k seconds later. A stream that causes stalls should correlate at k = 0 or k > 0; one the
stalls cause should correlate at k < 0.

| stream | member 0 (40 s): k −1 / 0 / +1 | member 1 (243 s): k −1 / 0 / +1 |
|---|---|---|
| `/outcome_by_request` ask | 0.47 / **0.81** / 0.50 | 0.50 / **0.73** / 0.69 |
| `/raft` from learners | −0.14 / 0.34 / **0.60** | 0.02 / 0.29 / **0.46** |
| `/commit` send | −0.29 / 0.15 / **0.56** | −0.28 / −0.05 / 0.19 |
| `/outcome` | 0.18 / 0.36 / 0.38 | −0.02 / 0.12 / 0.17 |

- **The receipt's ask is a symptom, not a cause.** The edge asks `/outcome_by_request` only when a `/commit` send has
  had no reply for COMMIT_RECEIPT (2 s). Its count peaks with the stall (0.73 to 0.81 at k = 0), and it is already
  high a second before (0.47 to 0.50). That is what a stall-driven retry looks like. Cutting it would cut a
  consequence.
- **Learner `/raft` and `/commit` sends lead the stalls by a second** (0.46 to 0.60 at k = +1). The release-after-apply
  share is also up: 23% of stall time falls in the gap after an `Applied` row (`p4g.py`; m47 9%), and 61% of stalls
  contain one (m47 44%).
- **Learner `/raft`.** Each of the 8 learners called the leader 15.5 to 20.2 times a second (134.7 to 148.6/s in all;
  `raw/learners-m50b33.txt`). Over all learners these carried 34.5 to 40.7 read-index requests/s, and 10.7 to 12.4
  held appends taken a second.

### Which stream to cut first

**Learner `/raft` on the leader, 135 to 149 req/s (28%).**
- It is the largest stream that is not client work, and it leads stalls by a second (0.46 to 0.60).
- It is the one stream the design can cut without touching the commit path. Two candidate ways: fewer learner calls
  per append (each held call returns on the next append), or fewer read-index calls per learner. DESIGN "Hot group
  reads, learners" budgets "at most 20 read-index calls/s" per learner (50 ms floor, `READ_PIPELINE` 8 [guess]), and
  m50's learners ran at 15.5 to 20.2 calls/s each. Which one to cut needs a design, not this run.
- Cutting the walk's `/read` (m47's largest at 201 to 336/s) or `/suspect` (66 to 131/s) is no longer first. With one
  term change they ran 6.9 and 0.6 req/s here, so progress-suspect and lookup-retry already removed them.
- `/outcome` (116/s, after a `/commit` past COMMIT_HOLD answers `admitted`) and the receipt's ask (59/s) are the
  commit path's own retries: 1.0 `/outcome` and 0.35 ask per `/commit`. They are the next candidate. They follow the
  stalls, but they add to the next one. This is (c1b)'s territory, which the poll-load table already closed for a cap.

[inference] The m50 evidence for learners is a correlation in time, not a cut. A run with 4 learners in place of 8
(same load, a test-faults config, no code change) would show whether the leader's stall share falls with the learner
call rate.

### Close-out

- `retirenets.mjs 131358`: 12 names, all "not joined" (10:29:14Z). Two-pass purge: a 5,228, b 4,862, c 4,364
  (14,454 objects); second pass 0 on every site (`raw/purge-m50.log`).
- `joined.mjs` over 65536–65545, 131072–131360, 327680–327710 and 9043968–9043975: 12 still joined, exactly the
  kept fixtures (65536–65540, 131072, 131073, 131296, 327700, 9043968–9043970); 131358 not joined
  (`raw/joined-m50.txt`).
- `costrow` swept shards after the row: "no shard container running".
- **Idle reading** 10:33:35 to 11:05:35Z, spanning the 10:45 and 11:00 crons: graft-m2b $0.00 by PREFIX, and
  `idlewatch.py` saw 0 shard containers start in 32 minutes (`raw/idlewatch-m50.jsonl`). The account spent $1.46 in
  that window, all on other installs.
- **Cost.** graft-m2b by prefix from 10:09:51Z to 11:06Z: $3.71 of the $10 stop, all of it by 10:28Z. The account
  day was $15.40 at 10:28Z.

Raw files in `tests/fanout/raw/`: `costrow-m50b33.status`, `rows-m50b33.status`, `a1-b0-33-m50.jsonl`,
`terms-m50b33.jsonl`, `memguard-m50b33.jsonl.xz`, `tsave-m50b33.jsonl.xz`, `bystep-m50b33.txt`, `bylag-m50b33.txt`,
`outleg-m50b33.txt`, `learners-m50b33.txt`, `retirenets-m50.out`, `purge-m50.log`, `joined-m50.txt`,
`idlewatch-m50.jsonl`.
