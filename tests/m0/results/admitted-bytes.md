# admitted-bytes: what a leader and its host hold for open tickets

Branch `admitted-bytes`, built off `main` dc269249. It answers B0-33 round 19 on m30b (LM-1, lm1-memory 4ef56718,
`tests/am1/results/lm1-memory.md`, round 19): the leaders' heaps tracked their open admitted tickets at about 60 to 80 KB
a ticket, while every buffer memory-budgets bounds stayed small. The design edit is in `design/DESIGN.md`, Shape, the
write path (tickets as issued, seqs within a boot) and "Durability", the acknowledgment point (one copy of an open
ticket's command). The decision is `decisions/DECISIONS.md`, "`admitted-bytes`".

## What held the bytes

LM-1 named `Core::admitted` (`workers/site/src/host.rs`), which `commit_all_held` filled with a clone of every command.
Reading the code and round 19's memguard shows that was half of it.

1. **A second copy of every open command.** The host kept the whole command for two uses at resolution: the
   `NeedIdBlock` lease loop's re-send, and `Family::after_ok`'s `Committed` marker puts. The re-send applies only to an
   issue command refused for want of an id block. The markers read a push's `PackRef::Stored` or an issue command's
   confirmed blob; an inline pack marks nothing. So for B0-33's mix the copy was dead weight: each issue command's 10 KB
   render (about 15 KB as a `serde_json::Value`) and each inline pack held twice while its ticket was open.
2. **Tickets that never resolved for the host.** `step_down(term)` raises the term before it ends the leadership, and
   `end_leadership` built each ticket with `self.ticket(seq)`, which reads the term now. Every ticket of the lost term
   resolved `NotLeader` under the new term. The host keys `admitted` and `waiters` by the issued ticket, so it matched
   none: the commands and their waiters stayed until the object reset. `crates/raft/tests/deps.rs` already worked around
   it, matching a `NotLeader` by boot and seq at term + 1. Round 19's memguard shows it on every deposed leader:

   | member | as follower after losing the term | admitted (never falls) | wasm MiB |
   | --- | --- | --- | --- |
   | site 0, after term 5 | 23:32:51 to 23:41Z | 581 | 51.0 to 53.4 |
   | site 1, after term 6 | 23:33:10 to 23:43Z | 1,149 to 1,172 | 124.7 |
   | site 2, after term 6 | 23:32:39 to 23:33:10 | 636 | 71.5 |

3. **A ticket reused within one boot** (found while reading the same path, not seen hosted). A leader whose term's
   `Noop` reached no quorum gives up and keeps `led_term`; the next request resumes the same term in the same boot, and
   `Lead::seq` restarted at 0. Its first new ticket equals the first ticket of the leadership it resumed, so a late
   `outcome` for the old ticket could answer the newer command's outcome: B0-11's class, within one boot. Reproduced in
   `crates/raft/tests/scenarios.rs` on main (the same `Ticket { term, boot, seq: 1 }` both times).

## The fix

- `RepoCmd::marked` (`crates/git/src/repo.rs`), through `Family::marked` (`workers/site/src/family.rs`): a push with a
  stored pack, or an issue command with a confirmed blob and no render. Everything else keeps nothing. Chosen over a slim
  command for every ticket because it is the smaller change and keeps nothing for the common cases.
- The re-send's command comes from the core. `Out::refused` carries `(index, request id, command)`, the command moved out
  of the decided batch, so the core keeps no extra copy. `release` attaches it to the ticket until the ticket resolves.
  The Actions family registers its ticket before the output is released for the same reason (`workers/site/src/actions.rs`).
- `Lead::term` (`crates/raft/src/member/leader.rs`): `end_leadership` answers each ticket under the term it was issued
  in. `Member::seqs` carries the last seq across leaderships of a boot, as `rounds` already did.
- `Member::admitted_bytes` and `memSplit.inFlight.admittedBytes` (`workers/site/src/member.rs`): the encoded bytes of
  the core's queued and frozen commands, what each decided outcome in flight keeps that no event stores, and what the
  host keeps per ticket.

**Is the core's own copy bounded?** Yes, by count and by bytes for agents, and by count for the rest. The agent lane is
bounded by `AGENT_QUEUE_MAX` (1,000 commands, 64 MiB), and inline pack bytes queued by `AGENT_INLINE_ENTRY` (128 KiB,
staged past it). One batch is frozen (`Deciding`), within ENTRY_MAX, and one entry is in flight; its decided outcomes keep
only `unstored` bytes. Other lanes have no byte cap (F71, which LM-1 measured at most 493 commands and 3.97 MB in round
19). So no change to the core's bound here.

## Numbers

Native counting allocator, real `graftyard_raft::Member<RepoCmd>` groups (`crates/git/tests/admitted_memory.rs`). 500
agent issue commands with m3org-shaped renders and 500 agent pushes with 64 KB packs are admitted with no dependency
read answered, so every ticket stays open. Admission staged 997 of the 1,000 pushes, as the edge does.

| | main dc269249 | branch |
| --- | --- | --- |
| Heap held by 1,000 open tickets, core plus host | 17.8 MB | 9.9 MB (core `admittedBytes` 5.7 MB) |
| Tickets the host still holds after a higher term deposes the leader | 1,000 | 0 (heap +15.7 KB) |
| `a_leadership_deposed_by_a_higher_term_answers_its_tickets_as_issued` | fails | passes |
| `a_term_resumed_in_the_same_boot_never_reissues_a_ticket` | fails (same ticket) | passes |

The bound the test states: the core's copy within 500 × 11 KB + 500 × 1 KB + AGENT_INLINE_ENTRY of encoded bytes, and
the whole heap within 1.7 times that. Main's host copy takes it to about 3 times.

## Site 0's reset at 23:30:19 (round 19)

Workers Logs (observability API, `graft-m2b-site-a`, member object `0c37f61f…`, version ba3f8469):

- 23:30:17: memguard read 73.6 MiB and 969 admitted, then nothing answered 23:30:21 to 23:30:25. At 23:30:27 the object
  read 24.8 MiB and 0 admitted, a new wasm instance.
- The object kept logging 186 to 560 events a second through 23:30:23, with every fetch outcome `ok` and `/commit`s of
  3.0 to 3.2 s wall. 23:30:24 has no event at all; 23:30:25 resumes at 529. An alarm was canceled at 23:30:20.888 and
  another at 23:30:22.282.
- No `exceededMemory` outcome, and no message containing "memory", "exceeded", "panicked", "unreachable",
  "RuntimeError", "previous WASM", "abort", or "reset" anywhere on the script from 23:30:10 to 23:30:36.
- The `exception` outcomes are later and separate: 638 and 963 events at 23:30:38 and 23:30:43, "Internal error in
  Durable Object storage caused object to be reset".

Verdict: the reset left no outcome in the logs. It fits an isolate replaced for memory: 73.6 MiB of wasm with the JS
heap beside it, rising about 11 MiB a second before, and the next instance starts small. But that is [inference]; the
platform recorded nothing that names it. On the branch the same load holds about half the bytes per open ticket, and a
deposed leader frees them.

## The churn follow-up (F74), not fixed here

The operator asked for the root cause of member calls that miss CALL_DEADLINE under load, before any timer changes.
From B0-31 m3fr5c (m3f, lm1-rows-m3f5 db72f979, `peers-m3fr5c-b0-31.json`) and Workers Logs on graft-m3f, the second
burst at 04:33:41 to 04:33:46, leader site 2's object `2c043f02…`:

- The callees did no slow work. Every `/raft` call from site 2 that site a's and site b's members ran in 04:33:36 to
  04:33:47 (1,220 on site a, 490 on site b) had a handler wall time of 0 to 200 ms. Measured from the caller's
  `x-graft-sent` stamp to the callee's end, they took p50 2.4 s (site a) and 2.9 s (site b), up to 17.3 s.
- The answers arrived in batches. Of the leader's 222 late answers in the trace, 170 landed in an instant shared with
  three or more peers, for calls sent up to 7 s apart (04:33:46.494: voters 0 and 1 and learners 193, 194, 197, and 199
  together).
- Calls sent at 04:33:29.3 to 04:33:30.8 ran on the callees at 04:33:39 to 04:33:42. In those seconds the leader object
  had no events at 04:33:35 to 04:33:40, then `/read`s whose wall times reached 14 to 15 s.
- The StepDown that followed: `CommitErrors { calls: [(0, 6, 6), (1, 3, 3)] }`, at 04:33:43.344.

So the deadline is missed before the callee's handler starts: on the leader's side between `send` and the fetch leaving,
or in the platform's queue in front of the callee object. Both followers answer at once when the call reaches them,
which is why `own_stall` excuses nothing (it looks only at the leader's storage confirms). Not yet named: which one. The
next measurement is one burst with the caller's send time, the callee's arrival time, and the caller's event-loop lag on
the same calls. The fix design waits on that, as F74 records (`decisions/OPEN-QUESTIONS.md`).

## Checks

On this branch (crates/raft changed, so all of them):

- fmt, workspace clippy, wasm32 clippy for site, edge and ops (site and edge with and without `test-faults`): clean.
- `cargo test --workspace`: 959 passed, 0 failed. `go test` in `containers/git/server`: ok. The three
  `scripts/check_*.py`: OK.
- `tests/m0carried/plant14.py`: 4 of 4 caught (the deposed leadership answering under the new term, seen by the raft
  tests and by the host test; seqs restarting on a resumed term; the host keeping the whole command).
- Long sweep (`RAFT_SEEDS=2000 RAFT_STEPS=10000`, `long_sweep`): ok in 1,154 s. Porcupine (`-seg 2000`): `h` Ok on
  157,641 operations, `h.gray` Ok on 2,247,522, `h.live` Ok on 5,963,505.

`memSplit` change for later comparisons: `inFlight.admittedBytes` is new. It counts the core's queued, frozen and
decided bytes plus the host's kept commands; `inFlight.admitted` is still the host's ticket count.
