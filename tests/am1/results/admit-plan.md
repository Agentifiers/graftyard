# Admission and the 503s after m54, and the StepDown replay: plan before code (2026-11-19)

The orchestrator's two asks after B0-33 m54 (`b033-m54.md`): make admission and queueing the next item, plan first,
and look at STEP_DOWN's confirm thresholds, as analysis only. No hosted run; nothing deployed; $0.

## A correction to m54's rank 1

`b033-m54.md` ranked "commands queued at site b's leader" first, from a1's `commitMs` p99 5,865 and `queue:slot` p99
4,023. **Those numbers are site a's, not site b's.** a1 reads `/stats` from the leader it found at the start
(`tests/am1/rest/a1.mjs`, `leader` and `endStats`). That was site a, which led 39 s of load and then again after
01:23:24Z. Site b, which led 184 s with 56% of the 503s, has no `commitMs` in the row. Its cut trace (`Cut` 29,
`tests/m0/cuttrace/cuts.py`'s decoding, 10 s buckets) shows:

| site b, 01:19:43–01:21:20 | site b, 01:21:20–01:22:47 |
|---|---|
| 1.25 cuts/s; ends AgentTokens 82, EntryMax 26, Empty 10, Soft 3 | 1.39 cuts/s; **ends Empty 105**, AgentTokens 13, EntryMax 2, Soft 1 |
| agent commands left after a cut: p50 199, p90 335 | **left p50 0, p90 8** |
| commands taken 104–167/s | 60–86/s |
| bytes per cut p50 1.15 MB, p90 1.50 MB | p50 586 KB, p90 1.11 MB |
| edge 503s 6.2/s | edge 503s **59.4/s** |

So while the 503s rose tenfold, site b's leader had taken every queued command at every cut, with agent tokens left
(45 to 63) [measured]. **Leader admission did not make those 503s.** In the first phase it was token-bound (AGENT_SHARE),
with 200 to 335 agent commands waiting. At 104 to 167 taken a second that is about 1.5 to 2.6 s of queue, inside both
the REST commit's 60 s (`commit_hedged`'s `for_ms`) and AGENT_WAIT's 30 s, so it answered late, not 503.

## Where the 503s come from instead (hypothesis, to settle by logging)

- **The wall floor is SHED_RETRY_FOR.** Every write class's 503s start at 4.1 to 4.4 s (p10) in both phases: task,
  GraphQL, renew, label and lease. Upload-pack, a read with no retry, starts at 2.6 to 2.9 s. A write asks its
  shed tier read again for SHED_RETRY_FOR (4 s) and then answers 503 (`spread_read`, `shed_retry_for`)
  [measured walls; the match is an inference].
- **a1's only 503 bodies are that reason:** `push:human` "the repository's access rows did not answer; retry (repo:N:
  every member asked shed the read: overloaded)" (4), and `merge` (1). a1 keeps no REST bodies.
- **What the writes read:** task, label, renew, lease and GraphQL mutations each read the tiers first (`api::read`,
  `read_core`), through learners then followers. The learners' `/read` rose from 323–363/s to 433–530/s as the 503s
  rose, while clients offered about 118/s of these kinds. Their whole-tier `packed` reads rose from 201–228/s to
  341–364/s [measured, `Entered` 25 and `ReadQuery` 26]. Writes asking again for 4 s, and a1 retrying their 503s,
  multiply the reads [inference].
- **Which gate shed them is not known.**
  - Not READS_MAX: learners' open requests at entry were p90 149–188, and only 1–7% at 200 or more.
  - Not apply lag: learners applied 0–2 entries behind the leader.
  - Learner read-index calls ran p50 499 ms, 27% at 1 s or more. But their failed share fell in the high-503 phase (122–192 per 20 s against 215–512 earlier), so FOLLOWER_WAIT expiry alone does not fit.
  - Remaining: ANSWER_BYTES_MAX (16 MB of distinct answers, with about 150 KB per dump and 100–190 open) and the dump rebuild spacing. **Unmeasured.**

## Push admission: the other limiter

`push:agent` answered `full` 8,569 of 12,506 times (68%). That's AGENT_PUSH_BOUND's `ng … agent admission full`,
B0-32's shape, not a 503. Only 3,911 agent pushes landed, 13/s of 41.7/s offered. This is admission, and it costs
the ≥140 commands/s line directly (79.1 ok/s). Its split by leader and by refusal point is not in the row: refused at
the edge before send (`bound_at_send`), at admission (expected wait past the budget), or by `expire_agents` while
queued.

## Entry size and F92's syncs: is a cap worth it?

- **Bytes don't predict sync on m54.** On site a's `confirmSplit`, entries under 100 KB had storage p99 1,651 ms (n 21) against 1,209 ms for 100 KB to 1 MB (n 47).
- **Small syncs after a big flush.** 1-statement confirms that began within 500 ms of a flush of 250 or more statements were slow (500 ms or more) 7 of 70 times. Other 1-statement confirms were slow 32 of 739 times. By run (after a big flush, otherwise):
  - m50: 2/23 against 9/134
  - m51: 0/11 against 5/306
  - m52: 3/22 against 13/91
  - m53: 0/3 against 2/62
  - m54: 2/11 against 3/146

  A weak association (10% against 4.3%, n 70) [measured, the trace's raw clock].
- **m3f's 160-command entry** followed by a 2,618 ms confirm (B0-32, and m3fr17) fits the same shape, but one sample per row can't separate it from F35's background stalls [inference].
- **A byte or command cap** would make more entries, and each costs a round (p50 424 ms) with one in flight. So it lowers throughput unless cycles shorten, against weak evidence that size sets the sync. **Ranked last; not proposed.**

## Ranked candidates

1. **The write's tier read, shed by every member asked** (56% of m54's 503s by phase, with the leader idle). Fixes
   to weigh once the reason is known:
   - (a) writes read only the rows their plan needs (the narrow `rows.access` and `core` reads), not whole tiers;
   - (b) learners' shed gate, if it is ANSWER_BYTES_MAX or the dump rebuild;
   - (c) a shorter write retry, or none, since retries add reads.
2. **Push admission at AGENT_PUSH_BOUND** (68% of agent pushes `full`): which refusal point, on which leader, and
   whether the AGENT_SHARE bucket or the 5 s bound sets it.
3. **Admission queueing on a token-bound leader** (site b's first phase, 200 to 335 waiting). This is a latency cost,
   not a 503 source, at m54's load.
4. **An entry byte or command cap for F92.** Weak evidence; not proposed.

## Measurement plan (code-only; in the order to build)

1. **Edge: log every 5xx with its reason.** One line per REST, GraphQL and git 5xx: status, route kind and the
   reply's message, cut to 200 characters. That's at `api::serve` after `route`, and at the two git 503 builders in
   `workers/edge/src/lib.rs`. Messages are server text (chain ids, member names), so there's no token. Edge only, on
   an edge day. A `fivexxwhy.py` counts them by reason.
2. **Members: split `shedReads` by gate** (READS_MAX, ANSWER_BYTES_MAX, HEAVY_READS, FOLLOWER_WAIT expiry, rebuild
   not due) in `/stats`. Site code, on a site day.
3. **a1: read every leader's stats.** At each leader change in `terms`, read the outgoing leader's
   `/stats?summary=1`, so each term's `commitMs`, queue and cut split is its own. Also add the push-admission
   refusal point to `pushTiming` (edge `bound_at_send` against the leader's `Full`). Harness and edge.
4. Then one B0-33 row on the next m2b site day, then rank again with the reasons.

## STEP_DOWN's confirm rule against candidate rules (analysis only)

`tests/fanout/stepdown.py` over m50–m54's logged trips (`raw/stepdown-alarms-m50-m54.txt`, Workers Logs `member
alarm: StepDown`); output in `raw/stepdown-m50-m54.txt`.

**The trace cannot replay the rule.** The rule times each input's confirm on the member clock, from its step to its
`sync()`. The member clock is raised by every stamp, edge requests included, and those aren't traced. Rebuilt from
`Stepped`, `Confirm` and the peer stamps the trace holds (which ran p50 568 and p99 3,206 ms ahead of site b's raw
clock), none of m54's 11 logged Confirm windows reached the logged slow count: 0 to 5 slow against the logged 6 to 8.
So I judged the share rules from each logged trip's own `slow` of `of`, which is exact for whether the trip happens.
A dropped trip starts no hold, so a later check could still trip, and kept counts are a lower bound. **The longer
windows (20 s, 30 s) are not measured**: they need the rule's window samples, which no row logs.

| m50–m54 | Confirm trips kept | Confirm transfers kept | dropped trips with a raw stall in the 10 s before |
|---|---|---|---|
| now (≥ 6 slow) | 41 / 41 | 4 / 4 | 0 |
| share25 (≥ max(6, 25%)) | 39 / 41 | 4 / 4 | 0 |
| share33 (≥ max(6, 33%)) | 30 / 41 | 2 / 4 (drops m51's and m52's) | 5 |
| share40 (≥ max(6, 40%)) | 27 / 41 | 1 / 4 (keeps only m54's) | 8 |

- **Missed stalls.** There were 34 raw stall episodes while leading in the load windows: a raw confirm of 2 s or
  more, or a tick 1 s late. Only 18 had a Confirm trip from 1 s before to 10 s after under today's rule. The raw
  clock overstates slow syncs on a lagging isolate, so "stall" is a stand-in.
- **Hold under write load (F92).** Of the 4 Confirm transfers, 3 came with more than 100 flushed statements a second
  in the 10 s before: m50 420, m52 266, m54 192. Under hold-busy they hold. m51's, at 67/s, transfers.
  - m54's other transfer was a CommitErrors trip (390/s; 3 of 21 and 3 of 26 calls failed, all deadline misses),
    which hold-busy does not cover.
  - So hold-busy would have kept m54's leader through both of its step-down changes only together with a
    CommitErrors rule of its own [inference].
- **Reading.** A share rule cuts holds more than transfers: share33 removes 11 of 41 trips but also drops 5 trips near
  a real stall, and it keeps m54's transfer. Hold-busy targets the transfers themselves: 3 of 4 confirm transfers,
  and none of the holds change. Neither is proposed until the window samples are logged. That needs a test-faults
  accessor for the health window in `crates/raft`, so the long sweep and Porcupine apply even to it.
