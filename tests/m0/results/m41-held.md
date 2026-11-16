# m41-held: what holds B0-33's voter legs after fanout-dups

LM-1's B0-33 on graft-m2b, site day m41 from main 617e2b0a (with `fanout-dups`, 15f6a255), standard-4 shards,
repo:131345, a1 run mwjshq3m, load 21:48:20Z to 21:53:20Z, 5,000 agents, k 4, 8 learners. Terms 2 to 20, 13
StepDowns (11 CommitErrors, 2 Confirm), leader commit p99 14.5 s. The trace is `tests/fanout/raw/tsave-m41b33.jsonl.xz`
(tsave `EVERY=10000`, `LEARNERS=1`), from branch `m41-day` (b5041313).

```
xz -dc tests/fanout/raw/tsave-m41b33.jsonl.xz > /tmp/m41.jsonl
W="--from 2026-11-16T21:48:20Z --to 2026-11-16T21:53:20Z"
python3 tests/f74/fanout.py /tmp/m41.jsonl $W
python3 tests/f74/inbound.py /tmp/m41.jsonl $W
python3 tests/f74/calls.py /tmp/m41.jsonl $W --deadline 1000
python3 tests/f74/held.py /tmp/m41.jsonl $W
```

## 1. The six-slot fan-out no longer holds most legs

`fanout.py`, legs = total − work, by place in the caller's turn (m39 in brackets):

| place | calls | legs p50 | legs p90 |
|---|---|---|---|
| turns of six or fewer | 5,258 (3,093) | 513 ms (842) | 3,377 ms (4,357) |
| calls 1–6 of a larger turn | 671 (1,745) | 771 ms (1,000) | 4,477 ms |
| calls 7–12 | 450 (1,348) | 1,000 ms (1,434) | 5,981 ms |
| calls 13+ | 291 (2,305) | 1,043 ms (2,693) | 6,330 ms |

- Duplicates are gone: in turns with 10 or more calls to one voter, 1 of 26 `AppendQueued` rows repeated a last
  index (m39: 1,146 of 1,245) [measured].
- 78% of voter-to-voter calls now leave in turns of six or fewer, and those calls still have legs of p50 513 ms and
  p90 3.4 s. Delayed calls (out ≥ 1 s) had 22 fetches awaiting at send (p50), prompt ones 15 (m39: 93 against 22). So
  slot queueing is a residue, not the main holder [measured].
- Authz: `CallSent` counts `/raft` fetches only, so this trace cannot show whether `deps:authz` reads share a
  turn's slots. That is untested.

## 2. Storage confirms and event-loop stalls on both ends hold the legs

`held.py` matches each leg with the `Confirm` (10) and `Lag` (8) rows on both members:

- For voter-to-voter calls with legs of 1 s or more (2,508 calls, 7,683 s of legs), 71% of the leg time overlaps a
  confirm or loop stall on the caller during the out leg, or on the callee during the back leg. The same calls shifted
  by 7 s overlap 42%, so the excess is real, not chance [measured].
- Out legs sent while the caller had a confirm in flight: p50 3,076 ms (n 1,646), against 959 ms with none in flight
  (n 5,019) [measured]. The host comment in `Host::step` says a fetch issued before the deferred transaction is not
  held by the output gate. Fetches issued while an earlier transaction is unconfirmed are held, and this is what that
  looks like [inference].
- Loop stalls (the 100 ms ticker woke late) covered 11.6%, 13.8% and 7.8% of the window on sites 0, 1 and 2, with
  single stalls up to 4.7 s.
- **Confirm time follows the member's inbound rate.** Per 5 s and per voter, the median confirm p90 was 23 ms under
  100 req/s, 171 ms at 300, 981 ms at 400 and 722 ms at 700 or more. The number of confirms per 5 s fell from 29 to 2
  to 4 [measured].

The window divides at about 21:50:05Z, after the first leaderless gap:

| | voter req/s | voter confirm p99 | stalled | voter-to-voter calls missing 1 s | legs p50 / p90 |
|---|---|---|---|---|---|
| 21:48:20–21:50:20 | 142 to 329 | 238 to 1,158 ms | 3 to 9% | 873 of 4,128 (21%) | 368 / 1,582 ms |
| 21:50:20–21:53:20 | 647 to 737 | 3,056 to 5,505 ms | 8 to 22% | 2,440 of 3,437 (71%) | 1,690 / 5,899 ms |

## 3. What fills the voters: the edge's lookups and reports

Every voter's inbound comes from the edge (sender 255), averaged over the window:

| voter | `/read` per s | `/suspect` per s | `/raft` per s | `/commit` per s |
|---|---|---|---|---|
| 0 | 295 | 141 | 44 | 43 |
| 1 | 237 | 136 | 26 | 22 |
| 2 | 281 | 142 | 33 | 42 |

Each learner took a steady 82 to 87 `/read`/s throughout. On the voters, `/read` per 30 s rose from about 700 to
5,400 before the first churn to 10,000 to 21,000 from 21:50:20Z on, right after the leaderless gap at 21:49:50Z. It
stayed there while terms rose 7 to 15. `/suspect` rose from 134 to between 15,000 and 23,600 per 30 s across the three
voters at the same time. The a1 summary counts 4,802 missed receipts and 4,805 `suspect` among timed pushes [measured].

Reading [inference]: once a leader steps down, the edge's traffic moves onto the voters. Missed receipts in
`commit_hedged` send `/suspect` to every other voter, once per isolate per `SUSPECT_GAP` (500 ms), and with 5,000
agents over many isolates that comes to about 140 a second per voter. Which edge path sends the extra `/read` is not
settled. Candidates are `read_spread` falling past learners that cannot get a read index while there is no leader,
reads sent straight to the leader by `call_opt`, and `create`'s `{"probe": true}` reads. The trace records the route,
not the body. At that rate a voter's confirms run 1 to 5 s and its loop stalls for seconds. The leader's
AppendEntries then miss `CALL_DEADLINE` on both followers, which is `CommitErrors`, and the next leader inherits the
same load.

## 4. What trips the step-downs

- **CommitErrors (11):** classifying each voter-to-voter call that missed 1 s (3,313 in the window) by what held it:

  | holder | calls |
  |---|---|
  | the caller's confirm, begun after send, held the out leg; not excused by `own_stall` | 1,148 |
  | the callee's confirm or a loop stall during the back leg | 724 |
  | excused by `own_stall` (the caller's confirm began before send and was open at the deadline) | 596 |
  | the caller's loop stall | 369 |
  | none seen | 364 |
  | never entered the callee | 112 |

  So most counted misses are the members' own storage and loop stalls, on the leader's side or the follower's, not
  the link [measured for the overlap, inference for the cause]. `own_stall` excuses only a confirm that began before
  the send.
- **Confirm (2):** a at 21:48:47 (p99 721 ms) and c at 21:50:12 (p99 844 ms) against `confirm_p99_ms` 500. Both came
  while that leader's own inbound and transactions were heavy: site 0's first 30 s flushed transactions of 100 to 659
  statements every 300 to 500 ms, with confirms up to 2.4 s [measured]. The first step-down started the lookup storm.

## 5. The next fix (proposed, not built)

Cut the edge's load on voters while a group changes leader, at the source, in `crates/lookup`. The first step is
naming the `/read` source: a test-faults query tag on `Entered` (append-only, as `ROUTES` is) on the next B0-33, or
the same count in a lookup simulation. Then:

- **One leader lookup per chain per isolate in flight.** Every request that meets `NotLeader` waits on that one
  lookup instead of reaching the voters itself.
- **`/suspect` only from the first missed receipt** per chain, term and isolate in a `SUSPECT_GAP`, as `report_due`
  already keys it, but with `all=false` and `all=true` sharing one key.

Expected effect [inference]: from section 3, voter inbound falls from 650 to 740 req/s to near the pre-churn 140 to
330, where confirms held p99 of 1.2 s or less and 21% of calls missed rather than 71%. That breaks the loop
step-down → storm → misses → step-down. It does not fix the first trip: the leader's own confirm p99 at 721 ms
against 500 ms at 330 req/s. B0-33's zero-leader-change line therefore also needs the leader's per-turn transaction
size looked at (100 to 659 statements). That is the second candidate.

Test that must fail on main: a `crates/lookup` test where N concurrent requests to a chain all meet `NotLeader`. It
counts `/read` probes and `/suspect` posts per voter and asserts at most one lookup, and one report per
`SUSPECT_GAP`, per isolate. On main each request runs its own lookup.

## 6. The lookup fix, built (`lookup-share`)

Built as proposed, design first. DESIGN, Shape, `ReplyMeta` and `suspect`, and DECISIONS "`lookup-share`":

- **One lookup per chain per isolate.** A request that must ask any member but its cached leader takes the chain's
  turn. One that waited and finds a leader cached goes to it. The holder keeps the turn until its lookup returns.
- **One `suspect` per chain, `about` and term per isolate per `SUSPECT_GAP`.** The `all` flag no longer splits the
  key.
- **`ReadQuery` (trace kind 26, test-faults, append-only).** Each `/read` logs its query (`q`'s first key, codes in
  `READ_QUERIES`), whether it is a read-index read, and its `Entered` arrival number. `inbound.py` prints `/read` by
  query per member, so LM-1's next B0-33 names the source.

Regression: `one_lookup_per_chain_serves_every_request_that_met_not_leader` sends 60 requests through 2 s of
`NotLeader` naming none. With the turn off, as on main, 900 posts reached the voters while none led, against 84. With
it every request got the leader's own answer within one pass. Plants: `tests/m0carried/plant29.py`, 5 of 5.

Held for after that run, not changed: STEP_DOWN's own-stall exclusion covers only a confirm begun before the send, so
the 1,148 misses held by the caller's later confirm still count. Excusing them could hide a slow member. The leader's
transaction size is the second candidate.

## B0-33 against the 90% rule

126 agent ok/s is not reached. `push:agent` was ok 2,850 of 12,460 (9.5/s), and `side:comment` 1,513 of 5,567. The
other agent side kinds recorded no `ok` code. There were 18 leader changes against zero, a line the 90% rule does not
relax. The row is still far short.
