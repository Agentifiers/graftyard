# read-rounds: what starved B0-33's voter calls, and one read round per clock reading

Branch `read-rounds`, built off `main` aa4a9d9c. It answers the operator's F74 hand-off: confirm the starvation
reading from LM-1's m36 trace, find the cause, and fix it design-first. The design edit is in `design/DESIGN.md`,
Shape, "The read path"; the decision is `decisions/DECISIONS.md`, "`read-rounds`".

## The trace

LM-1 round 24b (lm1-memory e94fbc08, `tests/am1/results/lm1-memory.md`): B0-33 on graft-m2b m36, main cfd46001, repo
131341, 5,000 agents, 300 s from 11:36:36Z, k 4, 8 learners. Terms 2 to 19, leader commit p99 4,622 ms. Files:
`tests/fanout/raw/tsave-m36r33.jsonl.xz` and `calls-m36r33.tsv.xz` on that branch. Every number below is from those
two files, joined by call id. Workers clocks advance only on I/O, so stamps from one isolate turn share a value; the
analysis compares stamps on one member's clock, or across members only where the trace covers both ends.

Coverage: `tsave` keeps a ring of 60,000 rows per member, so the voters' traces cover 266, 372 and 465 s of the run in
pieces, and the learners' 258 to 279 s of the 300. A call counts as "reached" only when its callee's trace covers
the 11 s after it was sent. Unfiltered, LM-1's "240 sent, 0 reached" at 11:37:00 to 11:40Z is a coverage gap: no voter
trace covers those 40 s.

## Starvation: confirmed, on both ends

Voter-to-voter calls by the caller's own fetches waiting for headers when it sent the call (`awaiting`, kind 23):

| caller awaiting | calls | answered within CALL_DEADLINE | out leg p50, ms (callee traced) | caller's open calls to voters p50 |
|---|---|---|---|---|
| under 12 | 396 | 44% | n 52 | 6 |
| 12 to 50 | 420 | 8% | 66 | 25 |
| 50 to 200 | 411 | 15% | 2,537 | 233 |
| 200 to 1,000 | 7,282 | 10% | 4,913 | 929 |
| 1,000 or more | 4,269 | 4% | 5,818 | 1,407 |

Caller event-loop lag was under 100 ms for 278,422 of 286,053 calls, and callee handler work was at most 2 ms at p99
on voter-to-voter calls. So the time is in the legs, and it grows with the fetches already open on the caller. The
callee side is the same story: learners' calls in flight into each voter reached 3,791 handlers at once on site 1 and
2,182 on site 2.

The caller's open calls are mostly to the other voters, not to learners (the last column, and 83 to 152 open to
learners at p50). So the learner reading in LM-1's inference is half the picture: what holds the voters' calls is the
voters' own outgoing calls as much as the learners' incoming ones.

## Where the calls come from

**Not read-index.** In the window learners sent 270,129 calls to the voters; 6,144 (2.3%) were read-index batches
(kind 1) and 36,580 hedged copies (kind 11). Read-index is already capped at `READ_PIPELINE` and one batch per 50 ms.
Coalescing read-index calls, the operator's preferred fix, would remove at most 2.3% of the calls.

**Duplicate AppendEntries in single milliseconds.** On each voter's own trace, AppendEntries queued to a voter (kind
12) and held for a learner (kind 18):

| site | appends | in the 6 to 30 milliseconds with 20 or more to one peer | distinct (ms, peer) |
|---|---|---|---|
| 0 | 12,196 | 10,120 (83%) | 1,633 |
| 1 | 33,923 | 32,300 (95%) | 786 |
| 2 | 36,612 | 33,000 (90%) | 1,307 |

The largest single milliseconds: site 1 at 1794742694509 sent 324 to each of its 10 targets; site 2 at 1794742750362,
562 to each of 9; site 0 at 1794742896773, 427 to each of 10. Of the appends held for learners, 9,010 of 9,129,
26,743 of 26,820 and 31,586 of 31,667 repeated a (learner, last entry) already held. Each such millisecond also has 5
to 10 `IndexTaken` rows: learners' read-index batches arriving in the same turn.

**The cause in code.** `start_read` (`crates/raft/src/member/leader.rs`) calls `broadcast(true)`, a forced round to
every target, for each read whose lease does not cover it. A batch of reads stepped in one isolate turn arrives at one
clock reading, so it sent one forced round per read. Each round to a voter is a call, and each to a learner is held
for the learner's next call and rides its response; the learner steps every one and answers each with its own call
(`Host::call`, one per `AppendRes`), which is where 270,129 learner calls come from. The voters' calls to each other
then sit behind hundreds of the voter's own duplicate rounds and its learners' answers.

The simulator shows the same rule: on main, 200 reads at one instant with every read needing a round send 200
AppendEntries to each of the two voters and each of 8 learners.

## The fix

`start_read` now calls `read_round`, which sends one round per clock reading, to the voters only:

- A round that left at or after a read arrived counts for it (`check_reads` grants on `acked_at` at or after the
  read's arrival), so every read at the same reading shares one round. A read at a later reading sends its own round,
  as on main, so no read waits for a round that left before it arrived.
- A learner's answer counts toward no quorum or lease, so the round goes to `peers()`, not `targets()`. Learners still
  get every entry as it is cut, `tick`'s heartbeats, and `append_for_read`'s entries ahead of a read-index answer.
- `crates/raft` only. No wire message, trace kind, or host path changes.

From the trace: with one round per (millisecond, target), the 82,731 appends become at most 3,726 (4.5%), and the
learners' held appends at most one per millisecond [inference: assumes the same read arrivals].

## An approach that failed the sweep

The first build kept one read round out at a time: a read that arrived after it left waited for its answer, then the
next round left. It removed the flood too, but the long sweep failed on its gray half: seed 10,756 had a healthy quorum
with requests waiting and none completing for 32,386 ms, past `LIVE_BOUND_MS` (30 s). Main passes that seed. Over the
2,000 gray seeds the longest stretch per seed moved by more than 5 s in 673 of them, and 23 seeds went over 20 s
against main's 12: a per-read round also renews follower promises and the lease, so holding it lengthened the
pre-vote and promise stalls the gray faults cause [inference from the vote traces: `Promised`, `LogBehind`,
`Quarantine` and `Suspected` denials through the stretch]. The per-instant rule keeps every round main sent at a new
clock reading; on the same 2,000 gray seeds it has 12 seeds over 20 s, the longest 27,508 ms (seed 10,698), and the
mean stretch 8,491 ms against main's 8,517.

## Tests and plants

- `reads_at_one_instant_share_one_round_and_no_learner_gets_it` (`crates/raft/tests/scenarios.rs`): 200 reads at one
  instant send one round to each voter and none to any learner; a read a millisecond later sends its own; the shared
  round grants all 200 and not the later read, whose own round grants it. On main it fails: "200 reads at one instant
  sent 200 rounds to SiteId(1)".
- `tests/m0carried/plant18.py`, five mutants: the main rule (a round per read to every target), the round to learners
  too, no sharing (one round per read to the voters), sharing with every later read, and no round on an expired lease.

`python3 tests/m0carried/plant18.py`, on this branch, release:

```
 0 CAUGHT  a round per read, to every target, as on main  [reads_at_one_instant_share_one_round_and_no_learner_gets_it]
 1 CAUGHT  the read round goes to learners too  [reads_at_one_instant_share_one_round_and_no_learner_gets_it]
 2 CAUGHT  no round shared: one per read, to the voters  [reads_at_one_instant_share_one_round_and_no_learner_gets_it]
 3 CAUGHT  a round shared with every later read  [one_follower_stall_counts_once_per_deadline]
 4 CAUGHT  no round for a read on an expired lease  [one_follower_stall_counts_once_per_deadline]
5 of 5 caught
```

## Sweep and Porcupine

`RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=... cargo test -p graftyard-raft --release --test sim long_sweep --
--ignored`, passed in 1,344 s:

| half | seeds | ops | acked | longest healthy-quorum stall (ms, seed) |
|---|---|---|---|---|
| HARSH | 2,000 | 157,027 | 48,232 | 15,485 (11,844) |
| GRAY | 2,000 | 2,247,416 | 962,580 | 27,508 (10,698) |
| live | 4,000 | 6,619,093 | 1,293,618 | 26,186 (11,208) |

Porcupine (`tests/m0/porcupine`, `-seg 2000`): h Ok, 153,165 operations; h.gray Ok, 2,247,416; h.live Ok, 5,961,247.

The sweep with the first build (one round out at a time) failed at seed 10,756, as above.

## Not done here

- No site day. The hosted check is LM-1's B0-33 rerun with the F74 trace on: it should show the voters' AppendEntries
  at most one per target per millisecond and their calls to each other answered within CALL_DEADLINE.
- The learner host still sends one call per `AppendRes` and hedges each. With the flood gone that is one call per
  append the leader cuts, which B0-18 ran at; it changes only if the rerun still shows learner calls in the thousands.
