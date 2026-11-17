#!/usr/bin/env python3
"""late-suspect's planted bugs: each must fail one of `an_overloaded_leader_still_appending_keeps_its_promises`,
`a_gray_leader_whose_appends_come_late_still_loses_its_lease` or `two_colos_within_the_window_stop_promises_to_a_renewing_leader`.
Restores each file byte for byte. Run from the worktree root: `python3 tests/latesuspect/plant.py`."""
import subprocess

EL = "crates/raft/src/member/election.rs"
LATE = "let late = self.now >= after(self.appended_at, CALL_DEADLINE);"
M = [("main's rule: every report counts", EL, LATE, "let late = true;"),
     ("late only past LEASE, so a gray leader's 1.5 s gaps never count", EL, LATE,
      "let late = self.now >= after(self.appended_at, crate::LEASE);"),
     ("late judged from the last promise renewal, which withholding stops", EL, LATE,
      "let late = self.now >= after(self.last_leader_seen, CALL_DEADLINE);"),
     ("a timely report clears the window's colos", EL, "                && late\n            {",
      "            {\n                if !late {\n                    self.reports.clear();\n                }"),
     ("reports never count", EL, LATE, "let late = false;")]
for name, f, old, new in M:
    s = open(f).read()
    assert s.count(old) == 1, name
    open(f, "w").write(s.replace(old, new))
    try:
        r = subprocess.run(["cargo", "test", "-q", "-p", "graftyard-raft", "--test", "leadership", "--", "colos_within",
                            "overloaded_leader", "gray_leader_whose", "unknown_colo"], capture_output=True, text=True)
    finally:
        open(f, "w").write(s)
    failed = [l.split()[0] for l in r.stdout.splitlines() if l.endswith("FAILED") and "---" in l]
    print("CAUGHT" if r.returncode else "MISSED", name, failed)
