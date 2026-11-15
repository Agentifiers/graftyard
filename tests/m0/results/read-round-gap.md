# read-round-gap: L0-12's burst rounds, and at most one read round per READ_ROUND_GAP

Branch `read-round-gap`, built off `main` ca747283. It answers the operator's hand-off after read-rounds: L0-12 m3fr6
on graft-m3f still churned, so find why the leader still sent bursts of rounds, fix it design-first, and say what made
the busy members' clocks lag. The design edit is in `design/DESIGN.md`, Shape, "The read path"; the decision is
`decisions/DECISIONS.md`, "`read-round-gap`".

## The trace

L0-12 m3fr6 on graft-m3f, build lm1-rows-m3f6 520020fe (main a304d3fc, which has read-rounds, plus the test-faults
instruments), k 4, repo:65567, A1 replay 18:17 to 18:27Z with `--kill-every 60`. The term went 2 to 32; commit p99
17.3 s; failedCalls 8,574. Per-member /stats: site 0 lagMax 5,687 ms and 11,543 lease fallbacks, site 1 4,300 ms and
10,296, site 2 163 ms and none. Files from the m3f thread (T-01a1d805): `m3fr6-l0-12-tsave-voters.jsonl`,
`-tsave-learners.jsonl`, `-calls.tsv`, `-elections.jsonl`, `-stepdowns.txt`, `-terms.jsonl`. Every number below is from
those files. Workers clocks advance only on I/O, so stamps from one isolate turn share a value; "one millisecond"
below means one clock reading on that member, which can be one turn of seconds.

## The learner flood is gone; the voters still get bursts

With read-rounds no learner gets a read round: at most 3 held appends per learner per millisecond. Rounds to the voters
(kind 12, AppendEntries queued to a voter) still came in bursts:

| site | appends to voters | in ms with more than 5 rounds to one voter | burst ms | rounds per voter in a burst ms, p50 / max |
|---|---|---|---|---|
| 0 | 6,573 | 4,504 | 134 | 13 / 99 |
| 1 | 1,954 | 1,154 | 47 | 11 / 61 |
| 2 | 8,013 | 3,001 | 147 | 9 / 84 |

The bursts repeat the same last entry: these are read rounds, not new entries.

## Why read-rounds did not hold them to one per reading

read-rounds shares one round per member clock reading. lease-clock (`decisions/DECISIONS.md`, "`lease-clock`") floors
the member clock at the newest peer stamp seen, so each call arriving with a newer stamp moves the clock inside one
turn, and the next read in the turn sees a new reading and sends its own round. Peer stamps that raised the clock in a
burst millisecond, p50: 15 on site 0, 17 on site 1, 6 on site 2, against 1 in milliseconds with one round. Those
raises account for 1,794 of site 0's 2,579 burst rounds, 672 of site 1's 705 and 1,050 of site 2's 1,991 [measured].
The same members ran on a suspect clock (the lease fallbacks above), so every read needed a round.

## The fix

A read round leaves only when a read that needs one arrived after the latest round left, and the slot is free: no
round yet, the latest answered by a quorum (`quorum_since(acked_at)` at or after it left), or READ_ROUND_GAP (20 ms
[guess]) since it left. `check_reads` calls `read_round` at its end, so the next round leaves on the answer. Rounds
still go to `peers()` only. A grant still needs a round whose quorum acked at or after the read arrived, so a read never
shares a round that left before it. With the gap, rounds per voter are at most one per answer or per 20 ms, 50 a second
when unanswered, whatever the clock does.

`crates/raft` only. No wire message, trace kind, or host path changes.

## Gate 1: the gray half, every seed under 30 s

Same scratch harness as read-rounds (the longest healthy-quorum stall per seed), seeds 10,000 to 11,999:

| build | seeds over 20 s | seeds over 30 s | longest (ms, seed) |
|---|---|---|---|
| main ca747283 (read-rounds) | 12 | 0 | 27,508 (10,698) |
| gap 20 ms | 13 | 0 | 27,508 (10,698) |

The gap does not lengthen the gray stalls: the same longest seed, and the mean longest stretch 8,572 ms against main's
8,491. 398 of the 2,000 seeds moved by more than 5 s either way, and the seeds over 20 s change places (6 of main's
12 fall under, 7 others rise over), as any change to message timing reshuffles the gray faults. The harsh half has no
seed over 20 s on either build (longest 15,739 ms with the gap, 15,485 on main), and the live half 9 against 8, none
over 30 s, the longest 26,186 ms (seed 11,208) on both. At 20 ms the gap passes; it was not tuned.

## Gate 2: read grant latency in the simulator

Every answered get's latency, from request to answer, over the same seeds:

| half | build | reads | p50 ms | p90 ms | p99 ms |
|---|---|---|---|---|---|
| gray | main | 897,048 | 26 | 513 | 2,021 |
| gray | gap 20 ms | 897,136 | 26 | 512 | 2,019 |
| harsh | main | 57,304 | 1,060 | 15,942 | 27,915 |
| harsh | gap 20 ms | 57,353 | 1,055 | 15,880 | 27,921 |
| live | main | 4,274,707 | 192 | 841 | 1,256 |
| live | gap 20 ms | 4,280,328 | 192 | 842 | 1,257 |

Within 6 ms at every percentile, on every half. The simulator's leaders rarely meet a burst of reads inside one
gap, so this says the gap costs nothing measurable where it does not bind, not what it costs a read that does wait:
at most one gap, 20 ms, before its round leaves. The hosted number is LM-1's B0-33 rerun.

## Gate 4: why sites 0 and 1's stamps lagged by seconds

The operator's hypothesis: peer stamps rode calls stuck 2 to 3 s in the out leg, so the suspect guard fed the flood
that delayed them. The trace says otherwise.

- The stamps that ran ahead mostly rode learner calls. On site 0, 21,892 of 22,711 arrivals with a stamp at least
  250 ms ahead of the receiver's clock came from learners; on site 1, 9,898 of 10,981.
- Slow calls carried stale stamps, not leading ones. Arrivals whose out leg took 2 s or more had a stamp lead of
  −559 ms (site 0) and −3,363 ms (site 1) at p50: behind the receiver's clock. Arrivals whose out leg took under 200 ms
  led by 3,011 and 2,817 ms. A stamp delayed in transit is older, not newer, so queueing cannot make it run ahead.
- So the receiver's own clock froze. Site 0's 1 s ticker woke 1 s or more late 31 times (91 s in all, the worst
  11,242 ms); site 1's 23 times (67 s, worst 8,322 ms); site 2's never. 149 and 97 milliseconds on sites 0 and 1 took
  50 or more arrivals at once, and the newest stamp in those was ahead by 2,680 and 1,973 ms at p50. Storage confirms
  there ran p99 515 ms (max 4,186) on site 0, and flushes reached 3.7 s.

So lagMax measures the busy isolate's `Date.now()` standing still between I/O turns, not stamps held in the out leg.
The guard does feed the bursts: a suspect clock forces every read to a round, and each newer stamp gave read-rounds a
new reading. The gap removes that feedback, since the round count no longer depends on the clock. What stalls the two
isolates for seconds is not answered here; it is F74's next question (`decisions/OPEN-QUESTIONS.md`, F74).

## Tests and plants

- `reads_send_at_most_one_round_per_gap_while_the_clock_climbs_and_no_learner_gets_it`
  (`crates/raft/tests/scenarios.rs`, replacing read-rounds' one-instant test), on a suspect clock: over 1 s, 200
  reads, each at a clock reading 1 ms higher (a floored clock climbing inside one turn), with no round answered. It
  asserts at most 52 rounds to each voter (the cap of 50 plus 2) and none to a learner while reads step; every read
  granted once the rounds are answered; then, 1 ms after an answered round and inside the gap, a new read sends one
  round at once and is not granted until that round is answered. On main it fails: "200 rounds to SiteId(1) in 1 s for
  200 reads on a climbing clock, against a cap of 50".
- `tests/m0carried/plant20.py`, seven mutants. `python3 tests/m0carried/plant20.py`, on this branch, release:

```
 0 CAUGHT  a round per clock reading, no gap, as read-rounds did  [reads_send_at_most_one_round_per_gap_while_the_clock_climbs_and_no_learner_gets_it]
 1 CAUGHT  the gap ignored: a new round at every later reading  [reads_send_at_most_one_round_per_gap_while_the_clock_climbs_and_no_learner_gets_it]
 2 CAUGHT  the read round goes to learners too  [reads_send_at_most_one_round_per_gap_while_the_clock_climbs_and_no_learner_gets_it]
 3 CAUGHT  no next round once the latest is answered  [reads_send_at_most_one_round_per_gap_while_the_clock_climbs_and_no_learner_gets_it]
 4 CAUGHT  an answer does not free the slot: only the gap does  [reads_send_at_most_one_round_per_gap_while_the_clock_climbs_and_no_learner_gets_it]
 5 CAUGHT  a read that arrived after the latest round never gets one  [one_follower_stall_counts_once_per_deadline]
 6 CAUGHT  a read granted by a round that left before it arrived  [a_suspect_clock_grants_reads_only_by_a_round, reads_need_the_lease_or_a_round, reads_send_at_most_one_round_per_gap_while_the_clock_climbs_and_no_learner_gets_it]
7 of 7 caught
```

## Sweep and Porcupine

`RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=... cargo test -p graftyard-raft --release --test sim long_sweep --
--ignored`, passed in 1,279 s:

| half | seeds | ops | acked | longest healthy-quorum stall (ms, seed) |
|---|---|---|---|---|
| HARSH | 2,000 | 157,152 | 48,122 | 15,739 (10,160) |
| GRAY | 2,000 | 2,246,288 | 961,687 | 27,508 (10,698) |
| live | 4,000 | 6,620,774 | 1,292,713 | 26,186 (11,208) |

Porcupine (`tests/m0/porcupine`, `-seg 2000`): h Ok, 153,246 operations; h.gray Ok, 2,246,286; h.live Ok, 5,965,959.

## Not done here

- No site day. The hosted check is LM-1's B0-33 rerun on graft-m2b with the F74 trace on, together with composed
  misses: it should show at most one read round per voter per 20 ms and the voters' calls to each other answered
  within CALL_DEADLINE.
- The isolate stalls on sites 0 and 1 (F74's next question).
