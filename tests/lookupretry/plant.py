#!/usr/bin/env python3
"""lookup-retry's planted bugs: each must fail one of `a_leader_that_misses_once_is_asked_again_and_no_follower_is_walked`,
`a_leader_that_is_gone_is_walked_away_from_after_its_second_miss`, `suspect_reports_are_damped_per_member_and_term`, or
the simulator's `a_gray_leader_moves_within_the_target_at_the_suspect_gap`. Restores each file byte for byte. Run from
the worktree root: `python3 tests/lookupretry/plant.py` (`--lookup` runs only the lookup plants)."""
import subprocess
import sys

LK = "crates/lookup/src/lib.rs"
LT = ["cargo", "test", "-q", "-p", "graftyard-lookup", "--", "leader_that", "suspect_reports_are_damped"]
SIM = ["cargo", "test", "-q", "-p", "graftyard-raft", "--release", "--test", "liveness", "--", "at_the_suspect_gap"]
M = [("main's rule: a leader miss walks at once", LK, "            if by == SentBy::Leader\n                && !leader_missed",
      "            if false\n                && !leader_missed", LT),
     ("the retry never ends: a gone leader is asked forever", LK, "                leader_missed = true;\n",
      "", LT),
     ("a retried miss is not reported", LK,
      "                if matches!(answer, Answer::Missed(_)) {\n                    suspect(",
      "                if false {\n                    suspect(", LT),
     ("a `busy` is reported as a miss", LK,
      "                if matches!(answer, Answer::Missed(_)) {\n                    suspect(",
      "                if true {\n                    suspect(", LT),
     ("the gap left at 500 ms", LK, "pub const SUSPECT_GAP: Duration = Duration::from_secs(1);",
      "pub const SUSPECT_GAP: Duration = Duration::from_millis(500);", LT),
     ("a gap past SUSPECT's window: a second colo never lands inside it", "crates/raft/tests/liveness.rs",
      "const GAP_MS: u64 = 1_000;", "const GAP_MS: u64 = 15_000;", SIM)]
for name, f, old, new, cmd in M[:5] if "--lookup" in sys.argv else M:
    s = open(f).read()
    assert s.count(old) == 1, name
    open(f, "w").write(s.replace(old, new))
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=900)
        code = r.returncode
    except subprocess.TimeoutExpired:
        code = "timeout"
    finally:
        open(f, "w").write(s)
    print("CAUGHT" if code else "MISSED", name, code, flush=True)
