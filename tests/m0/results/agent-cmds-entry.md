# agent-cmds-entry (2026-11-20, code only)

The orchestrator (T-01a17bda) picked option 1 of the m3fgt1 attribution (`tests/am1/results/lm1-rows-m3f.md`, "Row
m3fgt1"): cap the Agent commands of one entry at `AGENT_CMDS_ENTRY` = 64. It is the only change, so the next B0-32 row
measures it alone. Option 2 (answer `admitted` and `pending` without `settled()`) is `decisions/OPEN-QUESTIONS.md`, A34.
No hosted spend.

## Change (`crates/raft`)

- **Design.** `design/DESIGN.md`, Shape, agent block (the constant, in both `DESIGN.md` and `AGENT-LAYER.md`) and the
  noisy-tenant row for chain groups; `decisions/DECISIONS.md`, "`agent-cmds-entry`".
- **Leader** (`crates/raft/src/member/leader.rs`, `cut`).
  - The `AGENT_FLOOR` loop takes at most `min(floor, AGENT_CMDS_ENTRY)`.
  - The `AGENT_SHARE` loop stops once the batch holds AGENT_CMDS_ENTRY Agent commands, the floor loop's included.
  - A cut it ended records `CutEnd::AgentCmds` (4, appended; `tests/m0/cuttrace/cuts.py` reads it).
  - Human, MergeQueue, Bot, the token bucket, `ENTRY_SOFT` and `ENTRY_MAX` are unchanged.
- **Constant** (`crates/raft/src/admit.rs`): `AGENT_CMDS_ENTRY: usize = 64` [guess between m3fwi3's entries of 31 to
  106 commands, at most 578 ms, and m3fgt1's of 160 and 191, 2.6 and 2.8 s].

## Test

`crates/raft/tests/admission.rs`, `a_burst_of_agent_commands_is_cut_at_agent_cmds_entry_per_entry`: m3fgt1's shape,
307 agent pushes with no inline pack and one Human push queued within 5 ms, at a fixed capacity of 2,000 commands/s so
the token bucket does not bind (it never did on m3fgt1).

| | Agent commands per cut | Result |
|---|---|---|
| Branch | `[1, 64, 64, 64, 64, 50]` | pass: a cut ends `AgentCmds`, the Human push goes in the first cut after it arrives, 308 of 308 commit |
| Main | `[1, 306]` | fail |

## Plants

`tests/m0carried/plant62.py`, 6 of 6 caught:

| # | Mutant | Verdict |
| --- | --- | --- |
| 0 | main's rule: no cap on agent commands | CAUGHT |
| 1 | the cap counts only the share loop, the floor loop adds more | CAUGHT |
| 2 | off by one: 65 per entry | CAUGHT |
| 3 | the floor loop ignores the cap | CAUGHT |
| 4 | a cut the cap ended is not recorded | CAUGHT |
| 5 | the cap is 128 | CAUGHT |

Mutant 5 was missed at first, because the test compared against the constant. It now asserts 64 and the exact cuts.

## Long sweep

`RAFT_HISTORY=… RAFT_SEEDS=2000 RAFT_STEPS=10000 cargo test -q -p graftyard-raft --release --test sim long_sweep --
--ignored --nocapture`, on this branch and on main `9a6db022` (a worktree, same command):

| phase | branch step-downs / holds | main step-downs / holds | longest stuck stretch, branch | longest stuck stretch, main |
|---|---|---|---|---|
| harsh | 0 / 8,206 | 0 / 8,206 | 14,707 ms (seed 10439) | 14,707 ms (seed 10439) |
| gray | 2,133 / 10,374 | 2,133 / 10,374 | 28,501 ms (seed 11893) | 28,501 ms (seed 11893) |
| live (4,000 seeds) | 1,782 / 10,443 | 1,782 / 10,443 | 25,191 ms (seed 13897) | 25,191 ms (seed 13897) |

Both passed (branch 1,289 s, main 1,214 s), and every phase matches main. The sim submits no Agent-class commands, so
the cap never binds there [inference]; the rule is pinned by the targeted test and plants above, not by the sweep.

## Porcupine

`tests/m0/porcupine`, built, `-seg 2000`, each exit 0, on both branch and main histories, which match:
`hist.jsonl` Ok, 146,836 operations; `.gray` Ok, 2,336,111; `.live` Ok, 5,289,017 (12,000 segments).

## What M4-WP4 found on B0-35 m61 (T-01a1dd0e)

f0's cut trace on m61 (main `9d6fdb71`): 1,296 entries during the load.
- Commands per entry: p50 7, p90 20, p99 48, max 80. Only 1 entry had more than 64.
- Bytes per entry: p50 75.7 KB, p99 116 KB, max 147 KB. 969 cuts ended `Soft` and 327 ended `Empty`.
- Every command was classed Human in the cut columns, so this Agent-only cap does not bind on B0-35.
- f0's 34 `/raft` turns of 100 ms or more of CPU had at most 24 commands per entry in their span. m61's heavy turns do not track entry size, so its 10.9 s queue p99 is not this cause.

M4-WP4's `read-newest` touches `crates/raft/src/member/install.rs`, `crates/raft/tests/leadership.rs` and
`workers/site/src/host.rs`, none of which this change touches.

## Checks

All exit 0, each checked by its exit code: `cargo fmt --all --check`; workspace clippy `-D warnings`; wasm32 clippy for
site and edge, each with and without `test-faults`, and for ops, front and runners; `cargo test --workspace` (1,304
passed, 0 failed); `go test` in `containers/git/server` and `tests/m0/porcupine`; the three `scripts/check_*.py`; and
`node --test tests/am1/rest/h1.test.mjs`.
