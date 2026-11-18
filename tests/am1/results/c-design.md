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
| (c1b) a per-isolate cap on `/commit` and `/outcome` waiters held open on the leader, answering `admitted` early | the commit family's 80 to 100% of the non-`/raft` open | **the largest measured lever**: out leg p50 ~0.3 s at < 64 open against 1 to 4 s at 300+ | low to medium: edge already handles `admitted`; client-visible commit latency moves to `/outcome` polls | **1** |
| (c2) leader sends clear of the output gate | at most the gate's share, 14 to 37% of the round-trip excess | **medium, bounded by the gate share**; sends issued in the apply's release still wait out that release | high: storage rewrite (KV `allowUnconfirmed`), reopens `m0-carried-4`'s Beat, and the flip isn't met | 2 |

Ranking by expected out-leg change per unit of risk puts (c1b) first, (c2) second and (c1a) third.

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
