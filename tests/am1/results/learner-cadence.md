# learner-cadence: fewer, larger learner calls to the leader, and what holds B0-33's `queue:slot` tail ($0 design note)

LM-1, 2026-11-21. Analysis only. Nothing is built until the lead has read this. It follows
`tests/am1/results/sync-ab.md`, whose flip was positive: the leader's confirm wall tracks its input count, and the
learners' `/raft` calls are most of the fall. Every number is from that row's three arms on graft-m2b: A (learners as
m64), C (no learners) and B (no learners, `--lean`). Site 0 led at term 2 in each. The traces are
`tests/fanout/raw/tsave-sab{A,C,B2}.jsonl.xz`. Tags follow `design/SHARED.md`, "Read this first".

**Short answer.**
- Section 1: a learner read-index floor of 200 ms, with each learner's append acks carried on its read calls, should
  cut the learners' calls to the leader from 55.8/s to about 20/s. On an m64-shaped row it predicts leader inputs of
  about 280/s and a confirm p90 of 200 to 260 ms, against A's 379. That is 1.5 to 1.9×, not B's 3.75×.
- Section 2: `queue:slot`'s p99 of about 3 s in C and B is not confirm. It is the agent lane's backlog behind
  `AGENT_CMDS_ENTRY` (64 a cut), drained one cut per cycle. The cycle is about 96% round, and the round is the voters'
  AppendEntries legs outside their handlers.
- **The cadence change alone will not move B0-33's commit p99 to its gate line.** Arm C removed every learner call and
  still had `queue:slot` p99 2,983 ms and human commit p99 2,029 ms.

## Section 1: learner AppendEntries cadence

### What the learners send now (arm A, 300 s)

| Per second, 4 learners | Count | Source |
|---|---|---|
| Learner `/raft` calls to the leader (`CallSent`) | 56.1 | learner rows, all to peer 0 |
| Leader `CallArrived` from learners | 55.9 | leader rows, peer ≥ 192 |
| Read-index batches sent (`IndexSent`) | 37.2 (7.9 to 10.3 each) | learner rows |
| Hedged sends (`Hedge`) | 2.8 | learner rows |
| AppendEntries stepped on learners (`AppendStepped`) | 8.8 | learner rows |
| Leader: appends held for a learner, and taken (`AppendHeld`, `AppendTaken`) | 8.5, 8.4 | leader rows |
| Leader: held → taken | p50 0, p90 16, p99 279 ms | leader rows |
| Learners' own inputs (`Entered`, almost all edge `/read`) | 196 | learner rows; 32 to 63 `/read`/s each (`learnerrates.py`) |

So the learners' calls are their reads, not the leader's appends [measured]:
- Each learner sends a read-index batch at most every `LEARNER_READ_EVERY` (50 ms, `crates/raft/src/member/install.rs`).
  At 32 to 63 reads/s each, that floor binds, giving 8 to 10 batches a second.
- The rest of the 56 calls are hedges, re-sends and the learners' replies to appends.
- The leader never calls a learner that is calling it (`workers/site/src/callers.rs`, `QUIET`). It holds the learner's
  AppendEntries for the response to its next call, which comes p50 0 ms later.

"Fewer, larger learner `/raft` calls" therefore means a slower learner read-index cadence, not leader-side batching of
appends. The appends already ride the learners' calls.

### The change (proposed, not built)

- **`LEARNER_READ_EVERY` from 50 to 200 ms.** A learner's reads queue up to 200 ms and leave as one batch. That bounds
  read-index calls at 5 a second per learner, 20 for four, against 37.2 sent now.
- **A learner's append reply rides its next read call when one is due within the floor**, instead of leaving as its
  own call. This is the mirror of `callers.rs`. Without it, the 8.8 a second of replies stay as calls.
- `READ_PIPELINE` (8) and `QUIET` (1 s) stay. The learner count stays at m64's 4.

This changes `crates/raft/src` (the read floor) and the site's call path, so it needs the long sweep and Porcupine.
A read's grant is unchanged: it still waits for a read index from the leader and for its own apply.

### (a) What learners lose

- **Read latency, not freshness.** Every learner read is still a read-index read. It answers at or after a commit
  index the leader confirmed after the read arrived, so it is as fresh as today [doc, `design/DESIGN.md`, "The read
  path"]. What grows is the wait before the batch leaves: up to 200 ms, about 100 ms on average for a read arriving
  at random, against up to 50 ms now.
- **Lag.** A learner's applied index moves when appends reach it. They ride its calls, so with calls every 200 ms
  instead of every 50 to 100, a learner can lag the leader by up to one floor more, about 200 ms. A read never sees
  that lag as staleness: `append_for_read` puts the entries a grant names into the same response
  (`crates/raft/src/member/install.rs`, `read_resolved`, and `leader.rs`, `append_for_read`), so a read waits for its own index, not for the next call.
- **Tier reads.** Learner `/read` call times in arm A were p50 197 to 447 ms and p90 814 to 1,000 ms
  (`learnerrates.py`). Adding p50 100 ms moves more of them past FOLLOWER_WAIT (1 s), where a learner answers
  `late` and the edge tries the next member [inference].

### (b) Keeping A's 503 level, not C's

- C's 5,434 503s came from removing the learners. The edge's tier reads then landed on the two followers at 125 to
  137 `/read`/s each, against 1.4 to 1.6 in A, and they shed them as `overloaded` (`tests/fanout/followerload.py`;
  `sync-ab.md`, "Agent commands/s are confounded"). The cadence change keeps every learner in the read set, so the
  followers stay at A's read load.
- **The risk is the `late` shed in (a).** The edge's next member for a late learner read is another learner or a
  follower, so a few more late answers raise follower reads and 503s a little, not to C's level [inference].
- A lighter leader also answers read-index batches sooner (C's confirm p90 136 against A's 379), which offsets part
  of the floor [inference].
- **Guard:** the row fails this change if edge 503s exceed 2× A's 525, about 1,050.

### (c) Prediction for an m64-shaped row

- **Leader inputs:** learner calls fall from 55.8 to about 20/s (read batches at the 5/s floor, hedges and re-sends
  scaled with them). The rest of A's mix is unchanged, so about 314 − 36 = **about 280 inputs/s** [inference].
- **Confirm p90:** between A (314/s, 379 ms) and C (244/s, 136 ms), p90 changed by about 1.5% per input/s, on a log
  scale. Taking 34 fewer inputs at that rate gives about 230 ms. Each remaining learner call carries a larger
  response, and B0-02 found the hosted cost per call is mostly a per-event floor (`tests/am1/results/lm1-rows-m3f.md`, `commit_profile.rs`: learner `/raft` 1.95 ms hosted against 2 to 3 µs modelled).
  So **200 to 260 ms**, a 1.5 to 1.9× fall, and ticker stalls of 500 ms or more falling from 10 to a few [inference].
- **Flip, written now:** a row where the leader's confirm p90 does not fall under 300 ms, or where learner calls do
  not fall under 30/s, means the floor is not what drives the learners' calls.
- Reaching B's 3.75× would need learner calls near zero, which only C's no-learner shape gave.

## Section 2: why `queue:slot` p99 is about 3 s while confirm p99 is 204 ms

### `queue:slot` is the agent lane's backlog, drained 64 a cut

Leader `/stats` and the cut trace (`tests/m0/cuttrace/cuts.py`), per arm [measured]:

| | A | C | B |
|---|---|---|---|
| Cuts, of which ended `AgentCmds` (64 agent commands) | 614, 338 | 507, 325 | 511, 330 |
| Agent commands left queued after a cut, p50 / p99 | 24 / 376 | 26 / 222 | 32 / 223 |
| Agent arrival → its cut, p50 / p90 / p99 ms (`agentbacklog.py`) | 1,326 / 3,741 / 5,110 | 755 / 1,924 / 2,676 | 975 / 2,319 / 3,087 |
| Capped cuts an agent command waited through | 0 to 7 | 0 to 5 | 0 to 4 |
| `/stats` `queue:slot` p99 ms | 5,798 | 2,983 | 3,150 |
| Agent commands/s cut | 103.2 | 94.3 | 88.8 |

- The agent lane is offered 252 commands/s, and each entry takes at most `AGENT_CMDS_ENTRY` = 64 of them
  (`crates/raft/src/admit.rs`). 55 to 65% of cuts end at that cap and leave up to 223 behind.
- An agent command waits through up to four capped cuts, each one cycle, before its own. `agentbacklog.py`, rebuilt
  from the cut rows alone, gives an arrival-to-cut p99 within 2 to 12% of `/stats`' `queue:slot` p99 in every arm.
- `/stats` counts every command, and agents are 95% of them. So `queue:slot` p99 is that backlog, not a wait on
  confirm.
- `tests/fanout/slotsplit.py` (B0-29's) reads short here, B queue p99 1,463 ms. It assumes every cut takes everything
  queued, which held on B0-29 (all `Empty`) and does not on B0-33.
- **Throughput.** 64 commands a cut and one cut per cycle is the agent ceiling: 64 / 0.62 s ≈ 103/s at B's full cycle,
  against 88.8 measured with C's and B's 503s. B0-33's line of 140/s needs a full cycle of 457 ms or less at 64 a cut.

### What the entry in flight was doing (arm B)

`leadcycle.py`, full cycles (entry cut `AgentCmds`), n 327, p50 [measured]:

| Part of a 618 ms cycle | ms | Share |
|---|---|---|
| Round: first AppendQueued → first ok AppendEnded from a voter | 598 | 96% |
| Apply: that ack → leader `Applied` | 0 | 0% |
| Wait to cut | 0 | 0% |
| Tail: deps overrun, `decide`, encode → next AppendQueued | 21 (p90 156) | 3% |

Inside the round:
- **Confirm** runs beside the legs, p50 58 and p99 204 ms. It is not the critical path: the round ends at the first
  voter's ack, and confirm is shorter than either leg.
- **Fresh/deps:** `deps:all` p50 75, p99 419 ms. These are read during the entry in flight, and only their overrun
  lands in the 21 ms tail.
- **The legs:** 0 → 1 p50 461 and 0 → 2 p50 535 ms. The follower handler is p50 0 and 24 ms, so **427 and 444 ms are
  outside the handler**: wire both ways, both input queues and the follower's output gate.
  - The leader issues each call at once: AppendQueued → AppendIssued is p99 0 ms (`followerload.py`).
  - Idle, the same leg is 94 ms (`tests/am1/results/round-remainder.md`).
- **The followers were loaded in C and B, not in A.** When the leader's `/raft` call entered a follower, the follower
  had p50 37 to 54 requests open (p90 84 to 94) in C and B. In A it had 1 to 2. That is the tier reads C and B moved
  onto them, 125 to 137 a second each. In A the legs were shorter outside the handler, 258 and 323 ms, but A's leader
  stalled.
- Outside the handler is also flat across AppendEntries sizes in A and B: 1 to 40 KB p50 313 to 524 ms against over
  150 KB p50 252 to 411. So it is not bytes on the wire, as on m64 (`round-remainder.md`, section 2).

**Reading** [inference]:
- In B the queue tail is AGENT_CMDS_ENTRY × a cycle that is almost all AppendEntries legs. The legs are slow because
  the followers' inputs queue under the tier reads that no-learner arms put on them. Confirm, decide, fresh and apply
  together are under 5% of a full cycle.
- In A the legs are faster at the followers, but the leader's own lateness (10 stalls ≥ 500 ms, clock 0.6 to 1 s
  behind) stretches them.

### Commit p99 and the human line

- Human commit stage p99 was 3,589 / 2,029 / 2,290 ms (A / C / B). Humans are not in the agent backlog: a Human
  command goes in the first cut after it arrives.
- Its wait is the rest of the entry in flight plus its own entry's round: about two loaded rounds. Two p99 rounds plus
  one median round is 2 × 1,136 + 359 = 2,631 (A), 2 × 943 + 491 = 2,377 (C) and 2 × 1,067 + 491 = 2,625 (B). C and B
  sit under that, and A's stalls put it over (`FRAME.md`, "Operator decision, 2026-11-20", the re-based line).

### What the cadence change does to commit p99

- It keeps the learners, so the followers stay at A's read load and A's shorter legs. It lightens the leader, so
  fewer stalls stretch those legs. If the round falls to 300 to 360 ms p50, a full cycle is about 450 to 500 ms. That
  gives agents 64 / 0.48 ≈ 130/s, and `queue:slot` p99 about 2 s [inference].
- **It will not bring human commit p99 to 1 s.** Even with no learner calls at all (C), two p99 rounds were 1.9 s.
  The 1 s line needs the round itself near the idle 94 ms leg, or one entry in flight a round shorter.
- **The levers for `queue:slot` are elsewhere:** the round (followers' input queue, the leader's lateness), and
  AGENT_CMDS_ENTRY against the cycle. A row testing the cadence change will move confirm p90 and the stalls. It will
  move `queue:slot` and commit p99 only as far as it shortens the round.
- **B0-29** [inference]: its queue tail was frozen batches behind one large entry in flight (`b029-m3f.md`, "The queue
  tail"), all cuts `Empty`. The cadence change helps it only through a shorter round on a leader whose learners are
  busy reading. It does nothing for the large-entry wait.

## Files

- `tests/fanout/agentbacklog.py` (new): the agent lane's arrival → cut wait, rebuilt from cut rows.
- `tests/fanout/followerload.py` (new): voter call issue and end times, and each follower's open requests at the
  leader's call.
- `tests/fanout/leadcycle.py`, `tests/m0/cuttrace/cuts.py`, `tests/fanout/learnerrates.py`, `tests/fanout/slotsplit.py`
  (existing), on `tests/fanout/raw/tsave-sab{A,C,B2}.jsonl.xz` from branch `r33-sab-row`.
