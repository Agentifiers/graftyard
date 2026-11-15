# learner-reset: a new member at an address starts clean, learners included

Branch `learner-reset`, built off `main` 85c8068d. It fixes the bug LM-1 found on B0-33 (lm1-memory 20a26878,
`tests/am1/results/lm1-memory.md`, round 23). The design edit is in `design/DESIGN.md`, "Placement failures".

## The bug

`take_config` (`crates/raft/src/member/leader.rs`) reset the leader's per-peer state (`next`, `matched`,
`matched_at`, `sent`, `pipe`, `promised_at`, `acked_at`, `retries`, `refusals`) only for a site that was absent from
the old config's voters. Learners were never in that set. `/learners` places each new generation on slots 0 to 7 again,
so at the same addresses (192 to 199) under new names. A leader that had placed generation 2 kept generation 2's
`matched`, about 748, for each new and empty generation-3 learner. `on_append_res` never moves `next` below
`matched.next()`, so every probe left at prev 748 with entries 749 to 752. Each learner refused with last 0 and applied
nothing: 1,776 calls, 144.9 MB, for the whole row. The same "0/8 in 120 s" shows in fm25s33, fm26s33 and fm28r33.

## The fix

- `take_config` now finds every address whose member is new: a voter site or a learner slot address that was absent
  before or that names a different `do_name`. It resets the same nine maps for each. A learner removed and placed again
  under a new name counts as new.
- A second hole, found while writing the test: a call sent to the old member at an address can still be answered after
  the change. Its `matched` would then count for the new member, because the reset happens once, at the change. The
  leader now records the first round it sends to each new member (`Lead::member_from`). `on_append_res` ignores an
  answer to an earlier round from that address, as it already ignores rounds below `min_round`.
- The voter one-change rule (`NotOneChange`) is unchanged. It still compares voter names by site.

## What else a learner's stale `matched` fed

From every read of `Lead::matched` in `crates/raft/src`:

- **Learner read grants: yes.** `append_for_read` sends a learner the entries through a read index only while
  `matched(peer) < index`. With the stale `matched` at about 748, a read granted at or below it got no entries ahead
  of its answer. The empty learner then waited `FOLLOWER_WAIT` and shed the read `overloaded`. That is the read gate's
  503s, alongside the refused probes.
- **Catch-up from storage and installs: yes.** `next` never fell below `matched.next()`, so the leader never sent the
  learner a catch-up read or an `InstallSnapshot`, which needs `prev` below the archive boundary. A learner behind the
  in-memory window could not have recovered either.
- **The outgoing byte budget: yes, bounded.** The refused calls counted against `LEARNERS_OUT_MAX` (4 MB) while alive,
  so the waste was capped per moment. It went on for the whole row and took budget from the other learners.
- **Commit proofs: evidence only, not the commit.** `advance_commit` and `after_ack` put every address with `matched`
  ≥ n in `acked`, learners included. `quorum::commit` counts only voters, so a learner never made a commit. But
  `proof_for` lists every acked address in `QuorumProof::acks`. A scratch run listed both learners there beside the
  two voters. With a stale `matched`, an `Acked`'s evidence named a learner as durable through an entry it did not
  hold. With the fix, a new learner appears there only after it acks. Learners in the proof are not new: [inference]
  harmless to the ack checks, which require at least a quorum of distinct sites including the answering one. They are
  flagged here for the operator, not changed.
- **No effect:** transfer and hand-off targets, `STEP_DOWN`'s picks, and the move home all filter to voters.
  Compaction (`log::compact`) reads only the applied index and the window. The archive works from applied entries.
  `place_learners` picks regions from load alone. `/learners` and `tests/fanout/learners.mjs` have no caught-up test,
  so nothing there read the stale value.
- **Left as is:** `STEP_DOWN`'s window (`LeaderHealth::links`) keeps the old member's call results for up to 10 s. A
  new learner whose old slot failed often in the last window could read as lossy until then, and `append_for_read`
  skips a lossy learner. It clears itself within one window.

## Tests and plants

- `a_second_learner_generation_on_the_same_slots_catches_up` (`crates/raft/tests/scenarios.rs`). It starts from LM-1's
  scratch repro. Two learners catch up. A second generation is then placed on the same slots while calls to the first
  are still out, and the old members answer those calls after the change. Then the slots are emptied and a third
  generation is placed. Each generation must hold the leader's commit index.
  - On main it fails: "the second generation caught up: {192: 0, 193: 0} of 15".
  - With the reset but without the round guard it fails the same way, at 16.
- `tests/m0carried/plant15.py`, four mutants:
  1. the reset for voters only, as on main;
  2. a new address only, not a new name at an old one;
  3. `next` reset but `matched` kept;
  4. a late answer from the old member counted for the new one.

## Checks

`crates/raft` changed, so all of them ran:

- fmt, workspace clippy, and wasm32 clippy for site, edge and ops (site and edge with and without `test-faults`):
  clean.
- `cargo test --workspace`: 965 passed, 0 failed. `go test` in `containers/git/server`: ok. The three
  `scripts/check_*.py`: OK.
- `tests/m0carried/plant15.py`: 4 of 4 caught. Mutant 3 is also caught by
  `a_member_is_replaced_on_its_own_site_in_two_config_changes`.
- Long sweep (`RAFT_SEEDS=2000 RAFT_STEPS=10000`, `long_sweep`): ok in 1,142 s.
  - Longest stretches with a healthy quorum and nothing completing: 15.7 s (harsh, seed 11431), 22.7 s (gray, 11201)
    and 23.8 s (live, 11169).
  - Porcupine (`-seg 2000`): `h` Ok on 157,641 operations, `h.gray` Ok on 2,247,522, and `h.live` Ok on 5,963,505.
