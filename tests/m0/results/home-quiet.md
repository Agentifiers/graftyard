# home-quiet and lookup-keep (2026-11-19, code only)

The orchestrator granted this (T-01a17bda) after B0-35 m58 (`tests/m4/results/m4-wp4.md`, "B0-35 on a test-faults build
after site day 58"). That row passed dispatch at p99 9.1 s. Its one leader change in the load, term 4, was the transfer
back to `home`, 1.05 s after f0's RESET_QUARANTINE ended. The minute around it had dispatch p99 15.1 s, against 2.7 s
before it and 3.9 s after. The lookup-mark trace showed two walks of 6.4 s and 9.0 s, and why they were that long
(F101). No hosted spend.

## Change 1: `home-quiet` (`crates/raft`)

- **Design.** `design/DESIGN.md`, Shape, the `transfer` doc: the hand-back waits for a quiet leader and goes anyway
  10 min after it was first due. `decisions/DECISIONS.md`, "`home-quiet`".
- **Member** (`crates/raft/src/member/health.rs`, `leader.rs`).
  - `Member::quiet(queue)`: at most `queue` commands in admission's lanes (`AdmissionView::queued`), and the entry in
    flight, if any, at most `entry_soft / 2`.
  - `maybe_hand_off`: once `home` is recovered and caught up and the other existing conditions hold, `home_due` is set
    the first time and kept. The transfer starts when `quiet(HOME_QUIET_QUEUE)` holds, or `HOME_DEFER_MAX` after
    `home_due`. `home_due` is cleared only when `home` stops counting as recovered (may lead, outside
    RESET_QUARANTINE, HOME_CALLS answered, none failed).
  - `HOME_QUIET_QUEUE` = 16, `HOME_DEFER_MAX` = 600 s, both [guess], exported.
  - Drain, `STEP_DOWN` and refusal hand-offs are unchanged.
- **Tests** (`crates/raft/tests/leadership.rs`), through `step` on a three-member cluster:
  - `a_busy_leader_defers_the_transfer_home_until_quiet`: B leads away from home A, A recovers. Under 48 commands a
    step for 60 s, and separately under one 60 kB command a step for 60 s, B stays. Under light load for 15 s after,
    B goes home.
  - `the_defer_cap_sends_a_busy_leader_home`: the 48-command load for `HOME_DEFER_MAX` − 10 s, and B stays; 30 s more,
    and A leads with every committed write intact.
  - Both fail on main's rule (plant 0). The three existing `home` tests pass unchanged.

## Change 2: `lookup-keep` (`crates/lookup`)

- **Design.** `decisions/DECISIONS.md`, "`lookup-keep`". DESIGN is at its 2,200-line cap; the lookup's walk order in
  Shape (`ReplyMeta`) is unchanged by either rule, so the entry carries it.
- **Walk** (`crates/lookup/src/lib.rs`).
  - `stage`: a record's bootstrap name is left out when the held config is at that member's generation or later and
    names a different voter on its site.
  - `lookup`: per pass, the leaders followers' `NotLeader` named (not stale, not themselves) and each member's misses.
    At a pass's end with no leader, a voter named by two or more followers whose only answer was one miss is kept as
    leader at the pass's term (`keep_named_leader`), once per request; otherwise the leader is forgotten as before. The
    trace line says "leader kept".
- **Tests**, m58's term-4 walk as the case (`drive_m58_walk`: f0 `#1#f0` misses, `#8#f1` and `#1#f2` name leader 0,
  the superseded `#1#f1` answers `notLeader(leader None)` from its old config):
  - `m58s_walk_skips_the_superseded_name_and_keeps_the_leader_two_followers_name`: `#1#f1` is never posted, f0's
    second post goes as the cached leader, and f0 is cached at the end. Fails on main twice over (plants 5 and 7).
  - `a_leader_is_kept_only_on_two_followers_word_and_once_and_bootstrap_names_stay_for_an_older_config`: one
    follower's word keeps nothing; a leader that misses six times is held for at most one pass and forgotten at the
    end; a config at generation 8 under a generation-9 record keeps all six names in stage 0.

## Plants (`tests/m0carried/plant55.py`): 10 of 10 caught

| # | Mutant | Verdict |
| --- | --- | --- |
| 0 | main's rule: the transfer home goes as soon as it is due | CAUGHT |
| 1 | no cap: a busy leader never goes home | CAUGHT |
| 2 | the cap restarts every tick | CAUGHT |
| 3 | quiet ignores the queue | CAUGHT |
| 4 | quiet ignores the entry in flight | CAUGHT |
| 5 | main's walk: a superseded bootstrap name is posted | CAUGHT |
| 6 | superseded ignores the generation: a move's new members are skipped | CAUGHT |
| 7 | main's pass end: the leader is always forgotten | CAUGHT |
| 8 | one follower's word keeps a leader | CAUGHT |
| 9 | a kept leader is kept on every pass | CAUGHT |

## Long sweep

`RAFT_HISTORY=/tmp/sw/hist.jsonl RAFT_SEEDS=2000 RAFT_STEPS=10000 cargo test -q -p graftyard-raft --release --test sim
long_sweep -- --ignored --nocapture` on this branch. Main's figures are looplag-clock's branch run
(`tests/m0/results/looplag-clock.md`), whose `crates/raft` (`fc0bf684`) is byte-identical to main `3e521931`'s; the sweep
is deterministic per seed.

| phase | branch step-downs / holds | main step-downs / holds | longest stuck stretch, branch | longest stuck stretch, main |
|---|---|---|---|---|
| harsh | 0 / 8,206 | 0 / 8,206 | 14,707 ms (seed 10439) | 14,707 ms (seed 10439) |
| gray | 2,133 / 10,374 | 2,133 / 10,374 | 28,501 ms (seed 11893) | 28,501 ms (seed 11893) |
| live (4,000 seeds) | 1,782 / 10,443 | 1,782 / 10,443 | 25,191 ms (seed 13897) | 25,191 ms (seed 13897) |

The sweep passed in 1,002 s, and every phase matches main. The sim gives no phase a queue under 16 for a full step
while a group is away from home and due, with home outside RESET_QUARANTINE, often enough to move these counts
[inference]. The defer and the cap are pinned by the targeted tests above, not by the sweep.

## Porcupine

`tests/m0/porcupine`, built, `-seg 2000`, one history file per run: `hist.jsonl` Ok, 146,836 operations;
`.gray` Ok, 2,336,111; `.live` Ok, 5,289,017 (12,000 segments).

## Checks

All pass:
- `cargo fmt --check`;
- `cargo clippy --workspace --all-targets -D warnings`;
- wasm32 clippy for site and edge, each with and without `test-faults`, and for front and runners;
- `cargo test --workspace`: 1,276 passed, 0 failed;
- `node --test workers/runners/tests/*.test.mjs`: ok;
- `go test ./...` in `containers/git/server` and `tests/m0/porcupine`: ok. One run beside the Rust checks failed
  `TestCancelledCloneReleasesTheLock` ("the network's lock is still held after upload-pack's write failed"), with
  this branch changing no Go. Run alone with `-count=1`, it passed in 58 s;
- `scripts/check_design.py`, `check_names.py` and `check_links.py`: OK.
