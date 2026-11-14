# Slow storage on more than the leader: term churn (2026-11-14)

The operator's item from a read-only review of LM-1's k-4 leader churn (B0-33): with slow confirms on every member,
the simulator showed about 60 leader terms and a term rise of 80 to 90 over 10 seeds, on a7d28e5 and 84d29c2. Fix the
churn in the core, test first, without weakening F67's gray-leader move. Branch `liveness-slow-storage`, off main
`a21ecdf` (which includes `liveness-split-vote`).

## Verdict

**Slow storage no longer turns into term churn** [measured, simulator]. With 30% of confirms at 1.05 s for a minute,
the term rose by 1.3 a minute on average with every member slow and 0.3 with both followers slow, over 200 seeds each,
against 4.3 and 3.8 on main `a21ecdf` over 60. No seed went 20 s without a serving member. Two rules in `crates/raft`:
a follower whose own storage would trip STEP_DOWN's confirm rule does not campaign while a leader appended to it in
the last CAMPAIGN_DEADLINE, and a confirm-tripped leader hands off to no follower whose answers run as slow.

F67's `suspect` counting and pre-vote refusals are unchanged. A gray leader with one slow and one healthy follower is
still moved through the healthy one. A dead leader is still replaced by a slow follower 3 s after its last append, and
a half-hung one within F67's bound (p50 12.6 s, max 17.0 s over 60 seeds, as on main). The long sweep, Porcupine, the
20,000-seed writes-fail scan, `long_liveness`, plants and full checks pass. This does not make B0-33 pass: its 1.7 s
confirm is LM-1's.

## The test (`crates/raft/tests/liveness.rs`)

`slow_storage(seed, followers_only)` warms a group up for 30 s at about 10 acked writes a second, then for 60 s slows
30% of storage confirms to 1.05 s (`FaultKind::SlowConfirm(300, 1_050)`) on every member, or on both followers of the
warm leader. It counts the term rise over the minute and the longest stretch with no member serving, and checks the
history for linearizability.

`check_slow_storage` asserts, over its seeds: a mean term rise of at most 2 a minute with every member slow and 1 with
both followers slow (the leader is then the healthiest member), at most 5 in any one seed, and no stretch over the 20 s
gray target with no member serving. The kept tests run 24 seeds each; `RAFT_SLOW_SEEDS=n` runs more. Per seed, 2 a
minute was the operator's line; 11 of 200 seeds rose by 3 to 5 with every member slow, and 7 of 200 by 3 or 4 with
the followers slow, so the per-seed bound is 5 and the mean carries the line.

| Tree | Mix | Seeds | Term rise per minute: mean | Distribution | Leaderless max |
|---|---|---|---|---|---|
| main `a21ecdf` | every member slow | 60 | 4.27 | 2: 2, 3: 11, 4: 19, 5: 25, 6: 3 | 13.7 s |
| main `a21ecdf` | both followers slow | 60 | 3.83 | 2: 3, 3: 18, 4: 27, 5: 10, 6: 2 | 12.1 s |
| this branch | every member slow | 200 | 1.33 | 0: 38, 1: 74, 2: 77, 3: 6, 4: 4, 5: 1 | 14.5 s |
| this branch | both followers slow | 200 | 0.30 | 0: 170, 1: 8, 2: 15, 3: 6, 4: 1 | 10.6 s |

[measured, simulator.] Both kept tests fail on main (mean 4.21 and 4.08 over their 24 seeds).

## The traces (main)

- **A hand-off to a member just as slow.** A leader whose confirms trip `STEP_DOWN`'s confirm rule hands off to a
  follower whose link is clean. That follower's storage is as slow, so it trips within one window and hands off again.
  Seed 605: 0 to 1 at +6.1 s, 1 to 2 at +15.7 s, then a hold at +24.8 s once quarantine left no target.
- **A campaign that cannot win still deposes.** `suspect` reports make followers withhold promises from the leader,
  which keeps appending. A follower campaigns, passes pre-vote, and sends its real vote after its own slow confirm. Its
  higher term reaches the leader and the other follower, which step down to it before the log check refuses it
  (`LogBehind`). Seed 700: four terms in a minute with the leader on site 0 all along.

## The fix (`crates/raft`)

1. **A slow follower does not campaign against a leader still appending** (`election.rs` `try_campaign`). A member
   whose own storage would trip the confirm rule (`storage_slow`: over the window, at least `confirm_slow_min` confirms
   over `confirm_p99_ms`, ended or still running, and the p99 over it) answers `MayNotLead` while a leader's
   AppendEntries reached it within CAMPAIGN_DEADLINE (`appended_at`, set on every accepted append, withheld or not).
   The 2 x ELECTION_AFTER waiver does not lift it.
2. **A confirm-tripped leader hands off to no slow follower** (`health.rs` `check_health`). `peer_storage_slow`: at
   least `commit_fail_min` of the follower's AppendEntries answers in the window ran over `confirm_p99_ms` past its
   fastest. A follower answers after its own storage confirms, so its round trip carries its confirm.

Rule 1 alone gave a mean of 1.78 (every member) and 0.67 (followers) over 60 seeds; with rule 2 counting
`confirm_slow_min` (6) slow answers, 1.63 and 0.50; counting `commit_fail_min` (3), 1.33 and 0.30. Rule 2 alone left
main's numbers (3.88 on followers).

## The two checks the operator asked for

- **Mixed case: a gray leader with one slow and one healthy follower**
  (`a_slow_follower_holds_back_while_a_healthy_one_moves_a_gray_leader`, `scenarios.rs`). Two colos report the leader
  A to both followers, which withhold their promises, while A keeps appending. B's confirms are slow. Once the
  promises lapse, B answers a report `MayNotLead` and C starts a campaign and wins. With rule 1 removed, B starts and
  the test fails. So F67's move of a gray leader still runs through the healthy follower.
- **What bounds the hold when the leader is dead.** The gate holds only while appends arrive: CAMPAIGN_DEADLINE (3 s)
  after the last accepted append, a slow follower campaigns as any other. `a_slow_follower_still_replaces_a_dead_leader`
  (`scenarios.rs`): A crashes, both followers are slow, and B starts and wins once its promise lapses. A half-hung
  leader, whose appends still arrive with every answer lost, keeps the gate shut until F67's check quorum ends its
  leadership after ELECTION_AFTER, then CAMPAIGN_DEADLINE more:
  `a_half_hung_leader_with_slow_followers_is_replaced` (`liveness.rs`), over 60 seeds, led off the site at p50
  12.6 s, max 17.0 s, the same as main, within the 30 s bound.

## Results on the final tree [measured, simulator]

- **Slow-storage tests:** pass, 24 seeds each (mean 1.12 everywhere, 0.33 followers). 200 seeds each: in the table.
- **`long_liveness`, 60 seeds each:** calls 30% max 9.4 s, slow confirms max 12.4 s, loop lag max 4.8 s, all 60 moved by
  STEP_DOWN; writes fail over 20,000 seeds (3000 to 22999) p50 11.6 s, p90 13.1 s, max 18.9 s, the same as main.
- **Long sweep** (`RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=...`): passes. Longest stretch with a healthy quorum,
  requests waiting and none completing: HARSH 15.7 s (seed 11431), GRAY 25.2 s (seed 10665), live 24.7 s (seed 11833,
  4,000 seeds from 10000), each under the absolute 30 s bound.
- **Porcupine** (`go run . -seg 2000`): Ok on all three histories, 156,325, 2,230,496 and 6,087,300 operations.
- **Plants** (`python3 tests/m0carried/plant11.py`): 8 of 8 caught on the final tree.
  The first run missed three (one slow confirm counted as slow storage, running confirms ignored, no hand-off filter).
  The mixed-case test now has C take one slow confirm and B's sixth still running, and the followers-slow test's mean
  bound is 1.0 (1.21 without the filter); each of the three then fails its test.
- **Full checks**: pass. fmt, workspace clippy, wasm32 clippy for site and edge (with and without `test-faults`) and
  ops, `cargo test --workspace` (78 suites, 856 passed, 0 failed), `go test ./...` in `containers/git/server`, and the
  three `scripts/check_*.py`.

## Cost

No hosted work: simulator only, $0.
