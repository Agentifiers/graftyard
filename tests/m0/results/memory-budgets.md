# memory-budgets: three bounds on a voter's memory under load, elections, and resets

Branch `memory-budgets`, built off `main` a4ddca4f and rebased onto c1508aaa; the rebase changed nothing in `crates/raft` or `workers/site`. It holds the operator's fixes 1 to 3 from the raft memory audit, one commit each, with the design edit first in each. Fix 4 (admission lanes), the `confirms` leak, and the host's `reissued` are open questions F71 to F73 (`decisions/OPEN-QUESTIONS.md`).

## Why

Voters on hosted installs reset with "isolate exceeded its memory limit" under heavy load. In those resets, memSplit's counted buffers showed less than the heap held [measured, LM-1 round 17, lm1-memory 014b55b0; m3fr3, lm1-rows-m3f 66b86100]:

- B0-33 on main 4438539: site 0's wasm heap sat at 95.9 MiB for a minute before the first reset. New leaders then held 58.3 MB of call bodies, 26 to 30.5 MB of read answers in flight, and 42.9 MB of messages held for learners.
- m3fr3 L0-12:
  - Site b had 22.9 MB of read answers in flight one second after it won term 8, and reset 18 s later.
  - Site a in term 9 held 26.0 MB of calls plus 20.3 MB of answers.
  - Site c sent each voter 10.7 to 11.2 MB in its first minute as leader, against an 8 MB log.

The audit found three things with no byte bound:

- `Lead::done` holds each resolved outcome with its reply render.
- The bytes of AppendEntries alive on a leader's calls. The host awaits a missed call LATE_LIMIT (10 s) more while the core resends it.
- Read answers in flight, which READS_MAX and HEAVY_READS cap only by count.

## Fix 1: `done` by bytes and age

`Lead::done` keeps at most DONE_KEEP (4,096) resolved tickets, at most DONE_BYTES (8 MB) of the bytes their commands carry that no event stores, and none older than OUTCOME_KEEP (15 s). For a ticket pruned from there, `outcome` answers `NotLeader` naming the same leader. The edge then asks `outcome_by_request`, which answers the committed command from its `request` row through `outcome::replay`, so `Acked` is still built only inside `outcome`.

| | main a4ddca4f | branch |
| --- | --- | --- |
| Heap held by renders of 8,192 issue commands, m3org-shaped render, native counting allocator (`crates/git/tests/done_memory.rs`) | 60.6 MB | 11.3 MB (bound: DONE_BYTES × 1.5) |
| Late `outcome` after the prune (`a_late_outcome_after_its_ticket_was_pruned_is_answered_from_the_request_row`) | kept, so never pruned | `NotLeader` naming A, then `outcome_by_request` gives Done at the row's index; fails with the age and byte rules removed |

## Fix 2: an outgoing byte budget

The leader counts the canonical bytes of the entries on each AppendEntries call, from the send until one of these ends it:

- the call's answer, which any role takes;
- an error report through `call_failed`;
- the host's new `call_ended(to, round)` input.

A missed deadline does not end a call, because the host still awaits it. The host reports `call_ended` from a drop guard when its call task ends by any path: an in-time answer, a late answer, an error, or LATE_LIMIT given up. It also reports it when an AppendEntries held for a learner, or built while stepping that learner's call, leaves in the learner's own response.

The leader sends a call only while all three bounds hold:

- the peer is within PEER_OUT_MAX (2 MB);
- all learners together are within LEARNERS_OUT_MAX (4 MB);
- every peer together is within OUT_MAX (8 MB).

A voter with no call alive always gets one. A call the host never reports stops counting after OUT_KEEP (12 s). Rounds never repeat within a boot, so an earlier leadership's calls stay keyed apart. No wire enum changes. `/stats` memSplit gains `inFlight.callsOut`.

The table below measures with the same harness on both trees. The host model reports a miss at CALL_DEADLINE and ends a call at its answer or at LATE_LIMIT; the bytes are measured from the envelopes, not from the leader's count. On main, `call_ended` is a no-op.

| Test | main a4ddca4f | branch |
| --- | --- | --- |
| `a_silent_follower_holds_at_most_peer_out_max_of_the_leaders_calls`: 40 entries of 1 MB, C silent; most bytes alive to C | 10.0 MB | 1.0 MB |
| same: B commits all 40 | yes | yes |
| same, calls to C fail late with an error at 2 s: calls sent to C while silent | | 18, against 3 with `call_ended` a no-op |
| same: C catches up after it answers again | | within 5 steps of 500 ms |
| `a_new_leader_catching_up_silent_learners_stays_within_out_max`: new leader B, 8 empty learners answering 3 s late; bytes alive to learners / to one learner | 24.0 MB / 3.0 MB | 3.0 MB / 1.0 MB |
| same: B commits through C | yes | yes |
| `a_silent_follower_costs_the_commit_rate_nothing`: 400 commands of 2 KB, one per 50 ms; commands not committed in their own step, and the final commit index | 0, equal with and without C silent | 0, equal with and without C silent |

The learner budget binds in this test because each learner gets one catch-up call of about 1 MB.

## Fix 3: answer bytes in flight

Every member counts the distinct bytes of read answers still streaming (`host::Answers`). An answer that many reads share counts once, keyed by the address of its shared bytes. While they reach ANSWER_BYTES_MAX (16 MB), a new read is shed `overloaded`, as READS_MAX sheds. **memSplit `inFlight.answers` changed meaning.** It now reports distinct answer bytes, a shared answer once; on main and every earlier run it summed each read's copy of a shared answer. Runs where many reads share dumps (learners, or `shared_answer` hits on a leader) read lower from this branch on for the same load. Compare against older runs only on reads that share nothing, or add `sharedReads` back in.

`distinct_answer_bytes_past_the_cap_shed_and_a_shared_answer_counts_once` (`workers/site/src/member.rs`):

- 40 reads of one 1 MB dump count 1 MB, and a learner's read is served.
- 15 more distinct 1 MB answers reach 16 MB, and both a learner's read and the leader's are shed.
- The cap lifts as answers finish.

On main no byte cap exists, so nothing is shed below READS_MAX.

## Plants

`tests/m0carried/plant13.py`: all 9 mutants caught.

| # | Mutant | Caught by |
| --- | --- | --- |
| 0 | `done` keeps tickets past OUTCOME_KEEP | `a_late_outcome_after_its_ticket_was_pruned_is_answered_from_the_request_row` |
| 1 | `done` ignores DONE_BYTES | `thousands_of_rendered_issue_outcomes_hold_about_done_bytes_of_the_leaders_heap` |
| 2 | every call is allowed | both budget tests |
| 3 | learners share no cap | `a_new_leader_catching_up_silent_learners_stays_within_out_max` |
| 4 | a missed deadline ends the call | both budget tests |
| 5 | an answer does not end its call | `a_lagging_follower_catches_up_in_calls_under_the_byte_cap`, `a_silent_follower_holds_...` |
| 6 | `call_ended` ends nothing | `a_silent_follower_holds_...` (late-error case). Missed on the first pass, when OUT_KEEP alone freed the budget; the test now counts the retries |
| 7 | a shared answer counts per read | `distinct_answer_bytes_...` |
| 8 | the gate ignores answer bytes | `distinct_answer_bytes_...` |

## Long sweep and Porcupine

**Long sweep** on fix 3's tree (`RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=...`, release): passed in 1,220 s. The longest stretch with a healthy quorum, requests waiting, and none completing:

| Mix | Stretch | Seed |
| --- | --- | --- |
| HARSH | 18.8 s | 11447 |
| GRAY | 25.2 s | 10665 |
| LIVE | 23.8 s | 12137 |

`looplag-handoff` recorded 15.7 s for HARSH and 25.2 s for GRAY.

The later test-only change to `a_silent_follower_holds_...` touched no crates/raft source.

**Porcupine** (`go run . -seg 2000`): Ok on all three histories, with 157,364, 2,228,424, and 5,925,586 operations.

## Rebase onto c1508aaa

Main's DESIGN was already at the 2,200-line cap, so this branch's design text moved into existing paragraphs with no new lines:

- `done`: the "Acknowledgment point" invariant.
- The byte budget and the host's inputs: Shape, "Members reach each other through Durable Object bindings".
- `ANSWER_BYTES_MAX`: the same sentences as before, in Scale and "Follower read".

The constants PEER_OUT_MAX, LEARNERS_OUT_MAX, OUT_MAX and OUT_KEEP are named in that prose rather than as `pub const` lines.

## Open, not built

- F71, fix 4 (lane bytes). A replay of B0-33's mix through a 20 s stall queued 17 MB at 100 commands/s and 68 MB at 300/s. At 1,000/s against about 613/s of capacity, the lanes reached 358 MB [measured, simulator, in the audit]. This waits for the hosted result of fixes 1 to 3, because a byte cap changes which commands are refused.
- F72, unended `confirms` after a failed `sync()`. Pruning one changes `own_stall`'s excuse, so `STEP_DOWN` changes too. That is more than a line or two.
- F73, the host's `reissued`. An edge may still ask by the old ticket, so dropping entries needs an age rule and a test of that ask. That is more than a line or two.

## Next, by others

m3f reruns L0-12 with memHigh, and LM-1 reruns B0-33 as written, once the operator merges.
