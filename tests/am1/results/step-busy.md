# step-busy (2026-11-20, code only)

The orchestrator (T-01a17bda) asked for the step-down fix after B0-33 m63r33: step-agree should hold a `CommitErrors`
trip on a group that keeps committing, whatever `hot` reads, keyed on entries committed in the trip's window. FRAME's
re-base waits on it. No hosted spend.

## The case, from m63r33's traces

`tests/fanout/raw/tsave-m63r33.jsonl.xz` (site a's leader trace) and `raw/a1-pushes-b0-33-mwpchsge.jsonl`:

- Site a held `CommitErrors [(1,22,3),(2,22,3)]` at 19:05:41, then moved on `[(1,18,3),(2,18,3)]` to site b at
  19:06:18 (term 3 at 19:06:22.651Z).
- **The group was committing.** Site a applied 13 entries in the 10 s before 19:06:18 (9 in the last 5 s). Over its
  whole leadership no 10 s window held fewer than 8 applied entries (p10 10, median 14); site b's leadership was the
  same (min 7, p10 10).
- **It had a backlog.** Every cut from 19:05:56 to 19:06:18 had an entry in flight and ended `AgentCmds` or
  `AgentTokens`, with 247 to 455 agent commands left after it.
- **No traced call failed.** In each trip's window site a's `AppendEnded` rows show 42 and 34 AppendEntries, none
  failed, and 10 and 8 answered after 1 s or more. The rule counted late answers to calls that missed CALL_DEADLINE
  [inference: `Trace::AppendEnded` marks a call failed only when its fetch errs].
- **`hot` read it cold.** Acknowledged pushes (a1's own clock, 5 s windows) ran 3 to 6 from 19:05:53 to 19:06:07 and
  again from 19:06:19, between bursts of up to 147: pushes commit in bursts between AgentCmds cuts, and task, lease and
  check commands are not pushes. The fold's push count at 19:06:18 is not traced, so that `hot` was false then is
  [inference]; the hold at 19:05:41 came at the end of a 146-push burst, and the move at 19:06:18 at the end of the
  next one.

## The change

- **Design.** `design/DESIGN.md`, "Gray region": a `CommitErrors` trip also holds, hot or not, on a leader that
  committed at least `HOLD_ENTRIES` (5) entries in the window and is not quiet. `decisions/DECISIONS.md`,
  "`step-busy`", with its flip.
- **Member** (`crates/raft/src/member`):
  - `leader.rs`, `advance_commit`: each commit notes how many entries it committed (`note_commit`).
  - `health.rs`: `LeaderHealth::commits` keeps them for the `STEP_DOWN` window (`prune`); `busy` is at least
    `HOLD_ENTRIES` committed in it and not `quiet(HOME_QUIET_QUEUE)`; `check_health` holds when `hot() || (errors_trip
    && busy())`, unless `site_sick`. `HOLD_ENTRIES` is exported.
- The Confirm and LoopLag trips are unchanged: they measure the leader's own storage and loop, which commits do not
  clear.

## Tests

`crates/raft/tests/leadership.rs`, "step-busy", through `step` on a three-member cluster. `drive_agents` offers Agent
commands that are not pushes (so `hot` stays false) at 80 a second against a fixed capacity of 100, so the agent bucket
drains 50 a second and a backlog stays queued.

| Test | Shape | Main | Branch |
| --- | --- | --- | --- |
| `a_loaded_leader_that_still_commits_holds_through_its_own_commit_errors_trip` | m63's: every sixth call to each follower fails | moves: `CommitErrors { calls: [(1, 72, 4), (2, 72, 4)] }, Some(SiteId(1))` | holds, `to: None`; leads and commits more than 100 entries; backlog over 100 |
| same, every third / fourth call failing | | moves (57/3, 64/4) | holds |
| `a_loaded_leader_that_stops_committing_is_not_kept` | then every answer 3 s late: 5 entries in about 20 s, a quorum still heard | | holds once, then probes and transfers |
| `a_loaded_leader_on_a_site_the_others_call_sick_still_moves` | `set_site_sick(true)` | | transfers |
| `a_loaded_leader_whose_storage_trips_still_moves` | six 750 ms storage confirms under the same load | | Confirm trip transfers |

Step-agree's four tests pass unchanged. `cargo test -p graftyard-raft`: all suites pass.

Liveness, `leaders_leave_a_site_whose_calls_fail` (30% call failures), release, branch:
- 16 clients (10.2 acked writes/s): 12 of 12 moved by STEP_DOWN, new leader p50 5.8 s, max 9.6 s; 1 change each.
- 32 clients (18.3 acked writes/s): 12 of 12 by STEP_DOWN, p50 5.6 s, max 9.8 s; 1 change each.
- A 64-client run was killed by the orb's out-of-memory killer (9.5 GB resident, beside another thread's workspace
  tests), not by an assertion; it is not reported.

## Plants

`tests/m0carried/plant68.py`, 6 of 6 caught (rerun on the rebased head, exit 0):

| # | Mutant | Verdict |
| --- | --- | --- |
| 0 | main's rule: only a hot group holds | CAUGHT |
| 1 | busy holds without its CommitErrors scope, on every trip | CAUGHT |
| 2 | busy ignores the backlog, so a quiet gray leader holds | CAUGHT |
| 3 | commits are never noted | CAUGHT |
| 4 | the window's commits are never pruned, so old commits keep a stuck leader busy | CAUGHT |
| 5 | the other sites' verdict is ignored for a busy leader | CAUGHT |

Mutants 1 and 4 were missed at first. Mutant 1 needed `a_loaded_leader_whose_storage_trips_still_moves`; its first
form drained the backlog during the slow confirms, so the leader read quiet. Mutant 4 needed the stop test to keep a
quorum heard (late answers, not lost ones): with every answer lost, check-quorum ended the leadership whatever the hold
said.

## Long sweep and Porcupine

`RAFT_HISTORY=/tmp/sw/hist.jsonl RAFT_SEEDS=2000 RAFT_STEPS=10000 cargo test -q -p graftyard-raft --release --test sim
-- --ignored --nocapture` on the branch: exit 0, 1,388 s; again on the rebased head, exit 0, 1,047 s, every count below the same.

| phase | step-downs / holds | longest stuck stretch |
| --- | --- | --- |
| harsh | 0 / 8,206 | 14,707 ms (seed 10439) |
| gray | 2,133 / 10,374 | 28,501 ms (seed 11893) |
| live (4,000 seeds) | 1,747 / 10,251 | 27,362 ms (seed 12553) |

These match main's last recorded sweep counter for counter (`tests/m4/results/m4-wp4.md`, the `read-newest` branch now
on main: harsh 0 / 8,206, gray 2,133 / 10,374, live 1,747 / 10,251, 27,362 ms). The sweep's gray faults are
site-wide, and its groups carry no backlog of more than HOME_QUIET_QUEUE commands through a `CommitErrors` trip, so
step-busy never holds there [inference]. The rule is pinned by the targeted tests, not by the sweep.

Porcupine (`tests/m0/porcupine`, built, `-seg 2000`), each exit 0: `hist.jsonl` Ok, 146,836 operations; `.gray` Ok,
2,336,111; `.live` Ok, 5,476,993 (12,000 segments). The same on the rebased head's histories.

## Checks

Each by its exit code. First on the branch from main `479aff21`, then again after its rebase onto main `c60051c9`
(f108-stepdown-log, whose own `plant67.py` moved this branch's plants to `plant68.py`), with the same results except
the test count:
- `cargo fmt --all -- --check` 0; `cargo clippy --workspace --all-targets -- -D warnings` 0;
- wasm32 clippy `-D warnings`: site 0 and 0 with `test-faults`; edge 0 and 0; ops 0, and 101 with `test-faults`, which
  ops does not have (expected);
- `cargo test --workspace --no-fail-fast` 0: 1,324 passed, 0 failed (1,327 on the rebased head);
- `go test ./...` in `containers/git/server` 0;
- `scripts/check_design.py` 0, `check_names.py` 0, `check_links.py` 0 (with `.worktrees` moved aside).

## Change 2, the Human and MergeQueue lane: not built, because main already has it

The orchestrator allowed only the second form: each cut takes the queued Human and MergeQueue commands first, within
the entry's byte bounds, then fills Agent up to AGENT_CMDS_ENTRY. That is `cut` on main
(`crates/raft/src/member/leader.rs`): its first loop drains `first_lanes = [Human, MergeQueue]` while `fits`, then
AGENT_FLOOR, Bot, and Agent to AGENT_CMDS_ENTRY. `soft-grow` sits in the same lines. The guard test exists too:
`a_burst_of_agent_commands_is_cut_at_agent_cmds_entry_per_entry` (`crates/raft/tests/admission.rs`) asserts the Human
push queued with 307 agent pushes goes in the first cut after it arrives.

m63r33's cut trace agrees [measured, `tests/fanout/raw/tsave-m63r33.jsonl.xz`, kind 29]:
- 402 cuts; Human taken 437 (site a) and 684 (site b), at most 15 a cut; Human or MergeQueue left queued after a cut:
  **0 in every one of the 402 cuts**. No Human command ever waited for a later cut.
- So Human's 4.2 s commit p50 is not a wait behind full agent entries in the queue. It is the wait for the cuts ahead
  of it to commit, one entry in flight:
  - Of 343 cuts that took a Human or MergeQueue command, 110 had no frozen batch ahead, 207 had one and 26 had two.
    Each frozen batch ahead held a full 64-command agent batch (p50 64, max 128 across two).
  - Cut to own apply, by batches frozen ahead (cut matched to its entry by index): 0 ahead p50 1,000 ms (p99 4,061 /
    2,568 by site); 1 ahead p50 1,806 / 1,671 (p99 3,876 / 4,322); 2 ahead p50 2,472 (p99 7,478).
  - The entry's own round (append to apply): p50 300 to 642 ms, p99 2.4 to 2.9 s. The gap between cuts was p50 500 to
    510 ms, p90 1.5 to 1.8 s.
- a1's Human pushes: commit stage p50 2,300, p99 5,397 ms (n 119); merges p50 2,664, p99 9,964 (n 41).

What would shorten that is a Human command joining a batch already frozen (B0-29's option (b), "B0-29 late join
deferred", which breaks read-after-admission unless it reads its own dependency), or fewer frozen batches ahead
(`read_ahead`), or shorter rounds. Each is a different decision from the one asked for, so none is built here. The
in-memory load I drafted to prove change 2 confirmed this: on main, with 120 agent side commands a second against a
fixed capacity of 165 and 4 Human pushes a second, Human p99 was 665 ms at a 330 ms round and 1,845 ms at a 925 ms
round, about two rounds, with every cut ending `AgentTokens` or `AgentCmds`.

The authz-source counter on `commit_issue_rid` rode change 2's branch, so it is not built either; it is a small
edge-only change and can go on whichever branch the orchestrator names.
