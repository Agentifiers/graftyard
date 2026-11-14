# A LoopLag hand-off with slow followers: the fresh leader's term (2026-11-14)

The operator's item from LM-1's round 15c (m26c, main `2456b8f`): after a LoopLag transfer to site 1 worked, site 1
lost its term about 13 s later with no StepDown logged. Find why and fix it. Branch `liveness-looplag-handoff`, written off
main `1f51c6f` and rebased onto main `2b1938e` (with `admission-stored-bytes` and narrow-reads-3's READ_PIPELINE).

## Verdict

**Fixed in the simulator** [measured, simulator]. A follower now judges its own storage slow at `commit_fail_min` (3)
slow confirms in the window, not the leader's six, so a slow follower no longer campaigns against the fresh leader a
LoopLag hand-off made. The new test fails on main and passes here.

## The cause (seed 910)

LoopLag(300) on the warm leader and SlowConfirm(300, 1,050) on both followers, for 60 s.
1. The LoopLag hand-off to site 1 worked: term 3.
2. About 13 s later, `suspect` reports start a campaign at site 2. Its window held 11 confirms, 4 over 500 ms
   (1,080, 1,060, 1,067 and 856 ms), so `storage_slow`, which needed six, read false and the gate let it campaign.
3. Site 2 passed pre-vote through site 0. Its real vote left about 1.07 s later, after its own slow confirm.
4. Leader site 1 adopted term 4 before its log check and stepped down with no StepDown alarm. It denied LogBehind
   (137 against 141).
5. Nobody won until site 0 at term 5, 12.4 s later.

A follower confirms about once per append it accepts, fewer than a leader confirms, so six slow ones in 10 s is rare
even at 30% slow. Hosted, the same shape is what LM-1 saw: promises lapsed at 07:01:18 with no StepDown, and site 2
led at term 4 from 07:01:21 [inference: the hosted members' confirm windows weren't sampled].

The StepDown line stamped 07:01:14.760, after the followers were at term 3, is the frozen request clock: it belongs
to a `POST /outcome` request that ended at 07:01:24.749 with wall time 9,431 ms, so site a's loop was blocked for
seconds [measured, Workers Logs on m2b].

## Test

`a_looplag_leader_with_slow_followers_hands_off_and_holds` (`crates/raft/tests/liveness.rs`), seeds 900 to 923: after
every `STEP_DOWN` that names a target, a member serves within 3 s, and no seed goes over 8 s with no member serving.

| Tree | Seeds | Named hand-offs | Over 5 s unserved | Longest unserved |
|---|---|---|---|---|
| main `1f51c6f` (kept test, 24) | 900–923 | 42 | 904 (8.5 s), 910 (12.5 s) | 12.5 s |
| this branch (kept test, 24) | 900–923 | 46 | 0 | 4.9 s |
| main, scratch scan | 900–1099 | | 18 | 12.7 s |
| this branch, scratch scan | 900–1099 | | 4 | 5.6 s |

## Other runs

- Slow-storage tests over 200 seeds: term rise 0.94 a minute with every member slow (0: 63, 1: 91, 2: 44, 3: 1, 5: 1;
  longest leaderless 16.7 s) and 0.20 with both followers slow (0: 177, 1: 9, 2: 13, 5: 1; 3.5 s), against 1.33 and
  0.30 on main.
- `long_liveness`, 60 seeds each: all 60 moved by STEP_DOWN for calls (max 9.4 s), slow (12.4 s) and lag (4.8 s);
  writes fail max 15.8 s by election.
- **Long sweep** on the rebased tree (`RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=...`): passes in 1,894 s.
  Longest stretch with a healthy quorum, requests waiting and none completing: HARSH 15.7 s (seed 11431), GRAY 25.2 s
  (seed 10665), live 23.8 s (seed 12137, 4,000 seeds from 10000), against 15.7, 25.2 and 24.7 s on main `2456b8f`
  (`tests/m0/results/stored-bytes.md`), each under the absolute 30 s bound.
- **Porcupine** (`go run . -seg 2000`): Ok on all three histories, 156,325, 2,227,139 and 5,926,324 operations.
- **Plants** (`python3 tests/m0carried/plant11.py`): 9 of 9 caught. The new mutant, which restores the leader's six,
  fails the new LoopLag test. The first clean run missed one, "storage_slow ignores running confirms": the mixed-case
  test gave B five ended slow confirms and one running, and five already reach three. B now has two ended and one
  running, with the clock run on as long as six would take, and the mutant fails it again. An earlier plant run was
  discarded: the worktree held two leftover mutants from a run stopped mid-mutant.
- **Full checks:** pass. fmt; workspace clippy; wasm32 clippy for site and edge with and without `test-faults`, and
  ops; `cargo test --workspace` 875 tests in 78 suites, none failing; `go test` in `containers/git/server`; and the
  three `scripts/check_*.py`.

## Open

LoopLag's clamped-2 threshold under B0-33's load is a separate item, to check once pricing (`admission-stored-bytes`)
removes the 30 s queues.
