# `long_liveness` writes fail: a three-term split vote (2026-11-14)

The operator's ruling on `carried-9.md`'s `long_liveness` tail: writes-fail's 20 s target missed one seed on main
(14011, 21.5 s) and one on `m0-adaptive-append` (3052, 21.9 s), so it is a failing liveness bound and gets a fix in the
core, on its own branch off main. Branch `liveness-split-vote`, off main `2b42214`.

## Verdict

**`long_liveness` writes fail now meets its 20 s target on 20,000 seeds** (3000 to 22999, simulator): healthy leader
commits p50 11.6 s, p90 13.1 s, max 18.9 s, against main's max 21.5 s at seed 14011. Two rules in pre-vote, both in
`crates/raft/src/member/election.rs`: a voter that grants a rival's pre-vote drops its own pre-vote and starts none for
one CALL_DEADLINE. The long sweep (live half 4,000 seeds at the absolute 30 s bound), Porcupine, the other three
`long_liveness` faults, the two new kept tests and the plants pass.

## The trace (seed 14011 on main)

`RAFT_TRACE=1 RAFT_TRACE_VOTES=1 RAFT_LIVE_KIND=writes RAFT_LIVE_SEEDS=12000 RAFT_LIVE_ONLY=14011 cargo test -p
graftyard-raft --release --test liveness long_liveness -- --ignored --nocapture` [measured, simulator]. Times are ms
into the run; the fault on site 0 (30% of its storage writes fail) starts about 31 s in.

| Term | Followers start (pre-vote) | Pre-votes | Real votes | Ends |
|---|---|---|---|---|
| 4 | 2 at 42,150, 1 at 42,219 | each grants the other | each answers the other `Voted` (42,446, 42,573) | both lose at CAMPAIGN_DEADLINE |
| 5 | 2 at 45,579, 1 at 45,749 | each grants the other | `Voted` both ways (46,059, 46,128) | both lose |
| 6 | 2 at 49,194, 1 at 49,311 | each grants the other | `Voted` both ways (49,595) | both lose |
| 7 | 2 and 1 at 51,991 and 52,011 | site 1 denies 2 `Contending`; 2 grants 1 | site 2 grants 1 | site 1 leads, 21.4 s after the fault start |

- Site 0, the sick leader, never answered a vote in time, so a real vote needed the other follower's grant.
- Every edge reports `suspect` to both followers, so the same report starts both within 20 to 170 ms. That is inside
  one round trip, so each one's pre-vote reaches the other before that one has started, and is granted.
- A campaigning member denies a higher id as `Contending`, but each pre-vote arrived while its peer was not yet
  campaigning, so nothing stopped either.
- Each failed real vote runs CAMPAIGN_DEADLINE (3 s), then a 100 to 400 ms backoff, and the next report starts both
  again. The backoff does not separate them: the reports set the start, not the backoff.

## The fix (`crates/raft`)

Two rules in `crates/raft/src/member/election.rs`:
1. A voter that grants a rival's pre-vote starts no campaign of its own for one CALL_DEADLINE (1 s) from its first grant
   for that asked term (`pre_granted` in `Member`, checked in `try_campaign`). A pre-vote resent for the same term does
   not extend it. The rival gets one call deadline to send its real vote and win; if it does not, the granter
   campaigns as before.
2. A voter whose own pre-vote is out when it grants a rival's drops its own (`lose_campaign`, with its 100 to 400 ms
   backoff). A real campaign it has already started is kept.

Rule 1 alone fixed 14011 (11.7 s) and 3052, but over 20,000 writes-fail seeds (3000 to 22999) it failed two seeds that
pass on main [measured, simulator]:
- 15198: 3 leader changes (main: 1).
- 21945: moved in 22.2 s (main: 11.0 s).

In both, each follower's own pre-vote was already out when it granted the other's, so rule 1's wait never applied. Site
0, the gray leader, was by then a follower answering pre-votes, and granted both. Both entered the real vote and split
it, as in 14011. With rule 2 as well, 14011 moved in 12.5 s, 15198 in 12.0 s and 21945 in 11.3 s, each with one leader
change. Recorded in DESIGN ("Failure modes", gray region) and DECISIONS ("`liveness-split-vote`").

The long sweep and the 20,000-seed writes scan below ran on the tree with both rules. The committed tree differs from it
only in a code comment.

## Results on the two-rule tree [measured, simulator]

- **`long_liveness` writes fail, 20,000 seeds** (`RAFT_LIVE_KIND=writes RAFT_LIVE_SEEDS=20000`): passes.
  - New leader off the site: p50 11.1, p90 12.6, max 18.6 s. A healthy leader commits: p50 11.6, p90 13.1, max 18.9 s.
  - On main over the same seeds: max 21.5 s (14011), one seed over 20 s; p50 11.6 s, p99 15.6 s (`carried-9.md`).
  - Rule 1 alone, same seeds: max 22.2 s, two failures (15198, 21945).
- **`long_liveness`, the other faults, 60 seeds each:** calls 30% max 9.4 s, slow confirms max 12.4 s, loop lag max
  4.8 s; 60 of 60 moved by STEP_DOWN in each.
- **Long sweep** (`RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=...`): passes. The longest stretch with a healthy
  quorum, requests waiting and none completing: HARSH 15.7 s (seed 11431), GRAY 26.1 s (seed 10539), live 26.0 s
  (seed 10778, 4,000 seeds from 10000), each under the absolute 30 s bound.
- **Porcupine** (`go run . -seg 2000`): Ok on all three histories, 156,577, 2,225,168 and 6,088,270 operations.
- **Kept tests** (`crates/raft/tests/scenarios.rs`):
  - `a_voter_that_granted_a_rivals_pre_vote_lets_it_win`: B grants C's pre-vote, then a `suspect` report reaches B;
    B stays `Busy` for one CALL_DEADLINE and C wins. A resent pre-vote does not extend the wait. Fails on main.
  - `a_voter_that_grants_a_rivals_pre_vote_drops_its_own`: B and C both start pre-votes, A (restarted, a follower)
    grants both, and A gets no real vote in time. B wins only if C dropped its pre-vote on granting B's. Fails without
    rule 2.
- **Plants** (`python3 tests/m0carried/plant10.py`): 6 of 7 caught. Plants 0 to 4 break rule 1 (no wait, an endless wait, no wait set, a zero wait, a wait that each
  resent pre-vote extends) and 5 removes rule 2; each fails a kept test. Plant 6, which drops a real campaign as well as
  a pre-vote, is missed: it needs a rival's pre-vote for term T+1 while this voter's own real vote for T is out, and
  dropping it there loses no safety (`voted_for` stays stored) and at most one backoff
- **Full checks**: pass. fmt, workspace clippy, wasm32 clippy for site and edge (with and without `test-faults`) and
  ops, `cargo test --workspace` (78 suites, 845 passed, 0 failed), `go test ./...` in `containers/git/server`, and the
  three `scripts/check_*.py`.

## Cost

No hosted work: simulator only, $0.
