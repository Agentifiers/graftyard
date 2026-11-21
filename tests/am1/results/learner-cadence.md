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

## The row: B0-33 lcA2, and the reversal

Built on `r33-learner-cadence` and merged as main `c6e123eb`; measured on graft-m2b on 2026-11-21; **reversed** by
`git revert -m 1 c6e123eb` (branch `r33-cadence-revert`) because its flip fired. DECISIONS `learner-cadence` holds the
record; A39 holds the open question.

**Setup.** Site day from `c6e123eb`, siteday exit 0; edge `d8330fb6` at 100% (rev `c6e123eba`). `reretire.sh` printed
RERETIRE OK graft-m2b. One B0-33 row, m64 shape, four learners (`enam:2000,weur:1000,apac:1000`), flag off: tag lcA2,
repository 131382, run `mwqna5p0`, load 16:53:52 to 16:58:52Z. Site 0 led at term 2 through the load window, in three
stretches cut by `terms.mjs` gaps, with five LoopLag StepDown alarms, all held. After the load, site 1 won term 3
(16:59:12Z, after a CommitErrors hold on site 1) and site 0 term 4 (16:59:36Z). A first start, tag lcA, refused
before any request because the reretire log left the tree dirty (`rows.sh`'s clean-tree check). Run cost $1.66;
graft-m2b $8.34 that UTC day.

**Verdict lines** against `sync-ab` arm A (sabA, same shape on main `c4d5a133`) [measured]:

| Line | lcA2 (200 ms) | Arm A (50 ms) | Flip line |
|---|---|---|---|
| Learner calls to the leader (`CallSent`, 4 learners) | **15.6/s** | 56.1/s | under 30: met |
| of which read-index batches / hedges | 15.0 / 0.3 | 37.2 / 2.8 | |
| Leader learner `/raft` (`Entered`) | 14.6 / 16.7 / 13.8 | 55.8 | |
| Leader confirm wall p50 / p90 / p99 ms | 48 / **612** / 2,249 and 77 / **799** / 2,258 | 52 / 379 / 2,076 | p90 under 300: **failed** |
| Leader inputs/s | 261 / 333 / 373 | 314 | |
| Leader `/outcome_by_request`/s | 40.1 / 43.2 / 92.9 | 12.6 | |
| Ticker stalls ≥ 500 ms | 69 (75.2 s) | 10 (9.1 s) | |
| Held-input batches (requests held) | 415 (71,574 of 87,929) | 464 (61,315 of 94,258) | |
| Round p50 / p99 ms (`/stats`) | 498 / 3,820 | 359 / 1,136 | |
| Cycle p50 / p99 ms (`/stats`); full cycle p50 (`leadcycle.py`) | 567 / 3,756; 707 to 735 | 420 / 1,223; 560 | |
| `queue:slot` p99 ms | 28,644 | 5,798 | |
| Human / merge commit stage p99 ms | 29,303 / 11,197 | 3,589 / 2,728 | |
| Agent commands/s cut | 70.8 | 103.2 | |
| Agent pushes ok | 1,426 of 12,382 | 5,477 of 12,547 | |
| Edge 503s | **9,948** | 525 | under 1,050: **failed** |
| Learner read wait p50 / p90 / p99 ms (queue + call) | **902 / 2,019 / 4,011** | 365 / 1,000 / 1,568 | |
| of which queue before the batch leaves, p50 / p99 | 139 / 3,275 | 6 / 764 | |
| of which the read-index call, p50 / p99 | 592 / 1,000 | 321 / 1,000 | |
| Learners' inbound `/read`/s | 300.4 | 193.2 | |

Sources: `tests/fanout/syncab.py` (inputs, confirms, stalls, held batches; three stretches of site 0's term-2
leadership, 16:53:52 to 16:55:51, 16:55:54 to 16:58:08 and 16:58:19 to 16:58:52Z); the a1 row's leader `/stats`; the
push log; `tests/m0/cuttrace/cuts.py`; `tests/fanout/leadcycle.py`; `tests/fanout/readwait.py` (new); the edge's 5xx
reasons in `rows-lcA2.status`, all "every member asked shed the read: overloaded" on task 2,813, graphql 1,950, renew
1,628, upload-pack 1,628, rest-repo-write 1,275, lease 628 and receive-pack 17. The second flip line (confirm p90
falls, round p50 does not) does not apply: confirm p90 rose.

**The flip fired.** Learner calls fell under 30 a second and the leader's confirm p90 rose instead of falling under
300 ms; every later line is worse than arm A's. The floor removed the calls it was meant to remove; their count
was not what set the confirm wall at this shape.

**Reading** [inference; one row against one row, run-to-run variance unmeasured; A39]: the floor added p50 133 ms of
queue to every learner read and the slower leader added 271 ms to its call. More reads passed `FOLLOWER_WAIT` and were
shed, the edge's shed retries raised the learners' inbound `/read` by half, and writes on longer paths asked their
outcomes on the leader three to seven times as often. So the leader's input did not fall though learner `/raft` did.
This is risk (a) above, at full strength.

**Teardown check** (`teardown-stops-runs`, shipped on the same site day) [measured]: network 131382 was torn down at
16:59:44Z; after the end sweep its shards on sites 0, 1 and 2 read `/status` running false, `alarm` null
(`tests/fanout/tornalarm.py`: TORNALARM OK), and no shard container ran. No client request answered 503 `warming`.

**Corrections to sections 1 and 2**, from the second opinion (thread T-01a1f697). The `Lag` rows and the round ranks are
rechecked here on `tsave-*.jsonl.xz` [measured]; the handler p50, the 163 to 238 ms and the autocorrelations are the
second opinion's:

- **Section 2 overstates the follower input queue.** It read the 427 and 444 ms outside the follower's handler in arm
  B as the followers' inputs queueing under the tier reads. The traces do not carry that much. The followers (sites 1
  and 2) logged no `Lag` row (kind 8) in any of sabA, sabC, sabB2 or lcA2; only the leader did (43 in A, 144 in lcA2).
  Their handler p50 was at most 53 ms. And arm A, whose followers had 1 to 2 requests open, still has 163 to 238 ms per
  leg outside the handler that the idle 94 ms leg does not explain. The between-arm contrast puts the follower read
  load at about 100 to 190 ms of the round [inference]. The rest of the leg is wire both ways, the follower's wait
  before its handler, and the leader taking the answer late, which this trace cannot split, since its cross-clock
  stamps drift by about 1 s. `b033-rebase.md`, item 3, has the Workers Logs split where it exists.
- **"Two p99 rounds plus one median" uses rounds that are not p99.** The mechanism stands (a Human waits out the entry
  in flight and then its own entry). The arithmetic does not. The `/stats` round p99s used above (1,136 / 943 / 1,067
  ms) sit at p96.2, p93.1 and p96.3 of the leader trace's entry rounds (first `AppendQueued` to first ok voter
  `AppendEnded`, 30 to 270 s), whose p50 / p90 / p99 are 292 / 766 / 2,108 (A), 472 / 819 / 1,779 (C) and 480 / 847 /
  1,507 ms (B). With those p99s the formula gives 4,508 / 4,030 / 3,494 ms, not 2,631 / 2,377 / 2,625. lcA2's `/stats`
  round p99 of 3,820 sits at p99.4 of its 323 entry rounds (p50 / p90 / p99 365 / 1,342 / 2,974). The round lag-1
  autocorrelation is −0.01, 0.04 and 0.31, so slow rounds mostly do not come in pairs, and a tail Human is one slow
  round plus ordinary parts. Which sampler the re-based line uses is `b033-rebase.md`, item 1.
- lcA2's Human p99 of 29.3 s cannot be built from rounds: it matches `queue:slot` (28.6 s), and its Humans were admitted
  16:58:45.5 to 16:58:49.5Z and answered 16:59:13 to 19Z, across the end of load and site 1's term 3 at 16:59:12Z
  [inference from the push log and the terms file].

**Raws** (`tests/fanout/raw/`): `costrow-lcA2.status`, `rows-lcA2.status`, `terms-lcA2.jsonl`, `tsave-lcA2.jsonl.xz`,
`memguard-lcA2.{err,jsonl.xz}`, `arrivals-lcA2.jsonl`, `shardgate-lcA2-b0-33-{0,1,2}.jsonl`, `siteday-lc.status`,
`reretire-graft-m2b.log`; `tests/am1/raw/a1-b0-33-mwqna5p0.jsonl`, `tests/am1/raw/runs/a1-b0-33-flcA2-b033.log`,
`fanout-lc-{edge,siteday}.log`.

## Files

- `tests/fanout/agentbacklog.py` (new): the agent lane's arrival → cut wait, rebuilt from cut rows.
- `tests/fanout/followerload.py` (new): voter call issue and end times, and each follower's open requests at the
  leader's call.
- `tests/fanout/leadcycle.py`, `tests/m0/cuttrace/cuts.py`, `tests/fanout/learnerrates.py`, `tests/fanout/slotsplit.py`
  (existing), on `tests/fanout/raw/tsave-sab{A,C,B2}.jsonl.xz` from branch `r33-sab-row`.
- `tests/fanout/readwait.py` (new, the row): each learner read's queue before its batch leaves, plus that call.
- `tests/fanout/tornalarm.py` (new, the row): each site's shard `/status` for a torn-down network.
