#!/usr/bin/env python3
"""colo-unknown's planted bugs: each must fail `an_unknown_colo_is_no_second_witness_but_still_starts_a_campaign`.
Restores each file byte for byte. Run from the worktree root: `python3 tests/colounknown/plant.py`."""
import subprocess

EL = "crates/raft/src/member/election.rs"
TY = "crates/raft/src/types.rs"
M = [("main's rule: colo 0 counts as a colo", EL, "if from != Colo::UNKNOWN {", "if true {"),
     ("an unknown report wipes the window's colos", EL, "                if from != Colo::UNKNOWN {\n                    self.reports.insert(from, self.now);\n                }",
      "                if from != Colo::UNKNOWN {\n                    self.reports.insert(from, self.now);\n                } else {\n                    self.reports.clear();\n                }"),
     ("unknown is a colo an edge can send", TY, "pub const UNKNOWN: Colo = Colo(0);", "pub const UNKNOWN: Colo = Colo(u32::MAX);"),
     ("an unknown report answers early, before the campaign", EL, "        // A stall marker: a follower's first `suspect` report.\n",
      "        if from == Colo::UNKNOWN {\n            return Verdict::Promised;\n        }\n        // A stall marker: a follower's first `suspect` report.\n")]
for name, f, old, new in M:
    s = open(f).read()
    assert s.count(old) == 1, name
    open(f, "w").write(s.replace(old, new))
    try:
        r = subprocess.run(["cargo", "test", "-q", "-p", "graftyard-raft", "--test", "leadership", "unknown_colo"],
                           capture_output=True, text=True)
    finally:
        open(f, "w").write(s)
    print("CAUGHT" if r.returncode else "MISSED", name)
