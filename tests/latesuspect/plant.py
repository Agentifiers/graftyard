#!/usr/bin/env python3
"""late-suspect's and progress-suspect's planted bugs: each must fail one of
`an_overloaded_leader_still_appending_keeps_its_promises`, `a_gray_leader_whose_appends_come_late_still_loses_its_lease`,
`two_colos_within_the_window_stop_promises_to_a_renewing_leader`, `a_commit_index_move_is_progress` (leadership), or the simulator's
`a_half_hung_leader_s_empty_rounds_do_not_hold_its_followers`. Restores each file byte for byte. Run from the worktree
root: `python3 tests/latesuspect/plant.py`."""
import subprocess

EL = "crates/raft/src/member/election.rs"
FO = "crates/raft/src/member/follower.rs"
LATE = "let late = self.now >= after(self.progressed_at, CALL_DEADLINE);"
LEAD = ["cargo", "test", "-q", "-p", "graftyard-raft", "--test", "leadership", "--", "colos_within", "overloaded_leader",
        "gray_leader_whose", "unknown_colo", "commit_index_move"]
SIM = ["cargo", "test", "-q", "-p", "graftyard-raft", "--release", "--test", "sim", "--", "empty_rounds_do_not_hold"]
M = [("main's rule: every report counts", EL, LATE, "let late = true;", LEAD),
     ("late only past LEASE, so a gray leader's 1.5 s gaps never count", EL, LATE,
      "let late = self.now >= after(self.progressed_at, crate::LEASE);", LEAD),
     ("late judged from the last promise renewal, which withholding stops", EL, LATE,
      "let late = self.now >= after(self.last_leader_seen, CALL_DEADLINE);", LEAD),
     ("a timely report clears the window's colos", EL, "                && late\n            {",
      "            {\n                if !late {\n                    self.reports.clear();\n                }", LEAD),
     ("reports never count", EL, LATE, "let late = false;", LEAD),
     ("late-suspect's rule: any accepted append is progress", EL, LATE,
      "let late = self.now >= after(self.appended_at, CALL_DEADLINE);", SIM),
     ("an empty append counts as progress", FO, "        if stored || self.state.commit > commit0 {",
      "        if true {", SIM),
     ("a stored entry alone is progress, a commit is not", FO, "        if stored || self.state.commit > commit0 {",
      "        if stored {", LEAD)]
for name, f, old, new, cmd in M:
    s = open(f).read()
    assert s.count(old) == 1, name
    open(f, "w").write(s.replace(old, new))
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=1200)
        code = r.returncode
    finally:
        open(f, "w").write(s)
    print("CAUGHT" if code else "MISSED", name, code, flush=True)
