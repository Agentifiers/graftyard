# learner-hold (F85): a learner's call stops waiting once its answer has left in any response

The operator asked for F85 after read-beat, code-only and design first (`tests/m0/results/read-beat.md`, "deps:tokens and deps:names p99 at exactly 500 ms"). The source is B0-29 `mwkyk9zg`'s leader trace (`tests/fanout/raw/tsave-m2bb29b.jsonl.xz`, load window 1794936481594 + 600 s, leader site 0).

## What the holds wait for

A learner's `/raft` call that carries a `ReadIndex` the leader cannot answer at once stays open up to READ_INDEX_HOLD (500 ms) for the batch's `ReadIndexRes` (`workers/site/src/member.rs`, `raft_steps`). The call ends early only if the answer is in its own reply or held for the learner (`Callers::holds`). A learner keeps up to `READ_PIPELINE` batches and several calls open at once, and any call's response takes everything held for it (`Callers::take`). So a call whose answer another call's response took, or whose batch was answered before the call arrived, waits the whole 500 ms.

From the trace [measured]:
- 92,092 learner calls reached the leader. 29,660 left 499 ms or more after their handler's entry stamp, 27,723 of them exactly 500 ms.
- In 29,547 of those 29,660, the batch's `ReadIndexRes` had already left in a response to the same learner before the call arrived. These are re-sent copies of answered batches: HEDGE copies after 500 ms, and `tick_remote` re-asks.
- The learners hedged 18,619 calls, all answered. A call held 500 ms is the one a learner hedges at HEDGE (500 ms), so the holds and the hedges feed each other [inference].
- The hold timers put 17% of the leader's trace rows on an instant exactly 500 ms after a read-index arrived. That fits the 500 ms p99 of `deps:tokens` and `deps:names` (read-beat.md) [inference].

## The change

The leader remembers, per learner, the read-index batches whose answers left in any response to that learner (`Callers::left`, the newest LEFT_KEPT of them). Both `take` (answers held and taken by a call) and a call's own reply record them, and each record wakes every call of that learner that is waiting. A held call ends once its batch is answered by either route (`Callers::answered`).
- **Correctness.** Nothing changes in what a learner receives. Every `ReadIndexRes` still reaches the learner in some response; the held call only stops waiting for one it will not carry. Raft already ignores a second answer to a batch, and batch numbers are unique per learner boot, so a remembered batch never matches a later boot's.
- **Depth.** The leader still never calls a learner that is calling it (`Callers::route` is unchanged), so B0-18's call-depth bound holds.
- **Scope.** `workers/site/src/callers.rs` and `Routes::raft` and `raft_steps` in `workers/site/src/member.rs` only. No raft change, no wire change.

What it beat:
- **A shorter READ_INDEX_HOLD:** every call that waited for nothing would still wait, only less, and a far learner's real round (230 ms measured) could miss it.
- **No hold at all:** a learner's answer would then ride its next call, up to 50 ms or more later, and a far learner's read waited one to two calls more before the hold existed (B0-18).
- **A longer HEDGE:** the learner would hedge less, but each held call would still sit 500 ms.

Replay [inference, from the trace]: under the new rule, 29,586 of the 29,660 full holds end on arrival (p50 and p90 0 ms), and 74 still run to 500 ms. That frees about 15,000 s of held call time over the 600 s. Whether the hedges fall with it is for the hosted run.

## Results

- **Test:** `callers::tests::a_held_call_ends_when_its_answer_leaves_in_another_call` (`workers/site/src/callers.rs`). It fails on main, where only `holds` is checked ("an answer another call's response took left the call held"), and passes with the change. It also checks:
  - that the waiting call is woken when the answer leaves;
  - that another batch, or another learner's answer, ends no hold;
  - that only the newest LEFT_KEPT batches are remembered.
- **Plants:** `tests/m0carried/plant36.py`, 5 of 5 caught:
  - `take` records nothing (main's rule);
  - `left` wakes no call;
  - `answered` ignores answers that left;
  - any answer that left ends every hold;
  - the memory is never trimmed.
- **B0-18's depth test:** `a_learner_that_calls_the_leader_is_never_called_back_so_depth_stays_bounded` passes unchanged.
