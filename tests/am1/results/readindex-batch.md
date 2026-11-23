# Batching read-index calls at the leader: priced at $0 (2026-11-23)

The lead asked for this after `learner-relay.md` found that read-index calls are 43 to 58% of a leader's learner bill,
at 0.31 to 0.58 ms a call. The question: would the leader save that by confirming R once per heartbeat round for every
waiting learner and follower read, not once per call? Does `read-newest` or `learner-hold` already do this, and why
is the per-call cost still there? For each option, which invariant it touches and what it saves on mrB, m3fsf2 and
m3fg3. Nothing is built and no hosted row ran. Tags follow `design/SHARED.md`, "Read this first".

## 1. The confirm is already batched

[measured from the code at main `06e9f300`]
- A learner's or follower's batch reaches the leader as one `ReadIndex` and goes into the same queue as the leader's
  own reads: `on_read_index` (`crates/raft/src/member/install.rs`) calls `start_read`
  (`crates/raft/src/member/leader.rs`).
- `start_read` queues the read, then calls `read_round`. A round goes out only when some waiting read arrived after
  the last round left, and only once that round is answered or READ_ROUND_GAP (20 ms) has passed since it left.
  Under the lease (`lease_end − DRIFT`, no transfer, no `clock_suspect`) no round goes out at all.
- `check_reads` grants every waiting read from one quorum answer (`quorum_since(acked_at) ≥ arrived`), whoever asked.
- So "confirm R once per round for every waiting read" holds today. The voters see at most one round per answer or
  per 20 ms, whatever the number of asks. On mrB the leader took 1.4 voter `/raft` calls a second in the load window
  (`tests/fanout/raw/readcalls.out`); confirm rounds are not where the read-index bill goes.

## 2. Why the per-call cost is still there

The per-call cost is the hosted request, not the confirm:
- Each learner ask is its own Durable Object `/raft` request: decode, `Host::step`, the output gate, an answer encode
  and the Workers request floor. Each call carries exactly one envelope (`Trace::RaftCall` asks per call is at most 1
  on all three rows).
- The plain-slice fit prices that at 0.31 ms a call on mrB, 0.44 on m3fsf2 and 0.58 on m3fg3, apart from the
  per-KB answer cost [measured, `tests/fanout/learnercost.py`]. B0-02 measured 1.95 ms a learner `/raft` call hosted
  against 2 to 3 µs of modelled raft logic (`tests/am1/results/lm1-rows-m3f.md`, "Sizing LM-1's input cut on B0-32").
- What sets the call count is each learner's own send floor: `send_batch` sends a batch at most every
  LEARNER_READ_EVERY (50 ms), with READ_PIPELINE (8) in flight, plus `HEDGE` copies and CALL_DEADLINE resends of the
  newest batch. Batching on the leader's side cannot lower that count, because the calls have already arrived.

What the two levers the lead named do:
- **`read-newest`** (`on_read_index_res`, `tick_remote`): a later answer grants every earlier batch, and only the
  newest batch is resent or hedged. It removed duplicate calls (2,446 resends and 7,089 hedges on B0-35 m61), not the
  base rate of one ask per learner per floor.
- **`learner-hold`** (`workers/site/src/member.rs`, `raft()`, READ_INDEX_HOLD 500 ms; `callers.rs`): a learner's call
  is held until its answer leaves in any response, so the answer rides the same request. It cut held time (29,547 of
  92,092 calls held past an answer on B0-29), not the number of calls.
- **Append replies** still leave as calls of their own. Ack riding was the second half of `learner-cadence`, reversed
  with its 200 ms floor (A39); `DECISIONS.md` notes that it "may come back as its own change with its own row".

## 3. The call mix at the leader

[measured, `tests/fanout/readcalls.py`, output in `tests/fanout/raw/readcalls.out`; load windows are mrB's 210 s and
B0-32's 6 s bursts, so the B0-32 rows rest on 402 to 670 asks]

| Row (entries/s; leader CPU an entry) | Learners | Learner calls/s | Asks/s (per learner) | Append-reply calls/s | Read-index ms an entry | Append-reply ms an entry | Follower asks/s |
|---|---|---|---|---|---|---|---|
| mrB, B0-33 (2.8; 136 ms) | 4 | 60.9 | 46.5 (11.6) | 14.4 | 5.1 | 1.6 | 1.4 |
| m3fsf2, B0-32 52.5 (3.3; 128 ms) | 8 | 146.0 | 111.7 (14.0) | 34.3 | 14.7 | 4.5 | 30.5 |
| m3fg3, B0-32 55.8 (2.3; 131 ms) | 8 | 95.7 | 67.0 (8.4) | 28.7 | 16.5 | 7.1 | 14.5 |

Learners average 8 to 14 asks a second against the floor's 20, so the floor binds in bursts, not all the time.

**A correction to my earlier IndexTaken figures.** Earlier scratch counts gave the leader's `IndexTaken` (kind 5) at
23.3, 48.2 and 38.5 a second, under the asks counted from `RaftCall`. That gap is an artefact of the extract, not of
the member: a batch number is `boot & !0xFFFF_FFFF` plus a counter, above 2^53, so the trace's JSON rounds it, and
`heavyturns.py`'s `trace()` dedup merges different asks that share a millisecond and a rounded number. Within a single
snapshot, before dedup, the leader's taken rows equal `RaftCall`'s asks exactly: 1,052, 670 and 402. Trace kinds that
carry a learner batch number (IndexSent 1, ResSent 6, Hedge 11, IndexTaken 5) are under-counted the same way by any
script that uses that dedup; `RaftCall` rows carry the caller's call id (under 2^53) and are not.

## 4. The options

Savings are the per-call fit (`a + b`) times the calls removed, per entry. Bytes are unchanged in every option, so
the per-KB part is not counted. Each 1 s bucket keeps at most the option's cap.

| Option | What changes | mrB | m3fsf2 | m3fg3 | Invariant it touches |
|---|---|---|---|---|---|
| **Leader-side batching** (the question as asked) | Nothing: the confirm is already one round for all waiting reads (section 1) | 0 | 0 | 0 | none |
| **F, one ask per region** | A region's learners send one batch per 50 ms between them, through a coordinator or one learner that asks for the rest | 0.5 ms (0.4%) | 7.7 ms (6.0%) | 7.8 ms (5.9%) | `read-newest`: a forwarded answer must grant only batches sent before it, per learner. A new hop adds a learner-to-learner call |
| **B, ack riding at 50 ms** | A learner's append replies ride its next ask at the existing 50 ms floor | up to 1.6 (1.2%) | up to 4.5 (3.5%) | up to 7.1 (5.4%) | none of the four; `Acked` is untouched because learners never count |
| **F and B** | Both | 2.1 (1.6%) | 12.2 (9.5%) | 14.9 (11.3%) | as F |
| **C, 100 ms learner floor** | LEARNER_READ_EVERY 50 → 100 ms | 1.3 (1.0%) | 5.7 (4.4%) | 5.5 (4.2%) | none, but `learner-cadence` (200 ms) was reversed on read wait |
| **Follower floor 10 → 50 ms** | FOLLOWER_READ_EVERY | 0 | 0.7 (0.5%) | 0.1 (0.1%) | none |
| **P, pushed grants** | The leader pushes (R, its clock) in answers it already sends; a learner grants a read only once a push stamped at least DRIFT after the read arrived reaches it | up to 5.1 (3.8%) | up to 14.7 (11.5%) | up to 16.5 (12.6%) | read-index linearizability: R would rest on the lease and the clock bound, not on a confirm sent after the read arrived |

Notes on each:
- **F** saves little on mrB, whose one learner per region (two in enam) already asks under 20 a second. On B0-32 the
  four enam learners carry most of it, which is the enam learner count that B0-32's owner now holds.
  A region coordinator is a new call path between learners, so it must respect the call-depth climb and `QUIET`
  (`design/DESIGN.md`, "Scale", "Hot group reads, learners"). Lag: a forwarded ask waits for the coordinator's next
  50 ms slot, at most 50 ms plus one in-region hop [inference].
- **B** is the cheapest and the only option with no read-path risk. It needs a hold of at most 50 ms on each reply
  (the floor), and a reply with no ask to ride leaves alone on the next tick, as `learner-cadence` built it. Replies feed
  the leader's view of a learner's progress (`append_for_read`, `held` entries), so a held reply delays only that.
  The "up to" is because a learner with no ask pending still sends its reply alone [inference].
- **C** is the cadence lever again at half the floor. At 200 ms, `learner-cadence` cut calls from 56.1 to 15.6 a second
  but read wait p99 rose from 1.6 to 4.0 s and edge 503s rose 19 times (A39). 100 ms saves 1 to 4% and no row measured
  its read wait.
- **P** is the only option that removes the read-index calls themselves, and it buys that with the read path's safety
  argument. A learner cannot tell from a pushed R that the leader confirmed it after the read arrived, so it has to
  wait out the clock bound: about DRIFT (500 ms) plus the push interval on every learner read, against today's read
  wait p50 365 ms and p99 1,568 ms. It also needs a learner call open to carry the pushes, because a member never calls
  a learner (`raft()`'s doc), so learners would still send one held call each per hold. And it touches `fresh`,
  `read-newest` ("an answer grants only batches sent before it"), the lease, and `clock_suspect`, so the long sweep and
  Porcupine would have to show it. I don't recommend it [inference].

## 5. Reading

- The leader's confirm is already batched; the bill is per request, and only fewer learner requests lower it.
- B, then F, are the two levers that keep the read path as it is. Together they save 1.6% on mrB and 9.5 to 11.3% on
  B0-32's slow leaders, which is the same order as lever A (`encode-once`) and relay (`learner-relay`), and no more.
  Neither reaches the ×0.63 commit scale m3fsf2 needs for 450.
- All three levers (A, relay, F and B) share one cost: `crates/raft/src` or the site's call path, so the long sweep,
  Porcupine and a site day. If any is built, B alone is the smallest change for its share: one hold of append replies
  at the floor that already exists, which `learner-cadence` built and tested once.
- The enam learner count, now B0-32's call, moves more than any of these on B0-32: half the learner bill is the
  enam learners (`learner-relay.md`, section 5).

**Proposed flip for a `readindex-batch` entry, if the lead wants one:** build B (ack riding at the 50 ms floor) when a
leader's append-reply calls pass 5% of its CPU an entry on a row that misses its line on leader CPU; build F only if
a region's learners together ask over 40 times a second on such a row.
