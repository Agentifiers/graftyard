#!/usr/bin/env python3
"""`idle-stall` (`decisions/DECISIONS.md`): a `suspect` report marks a stall only while the follower holds an entry
above its commit that is CALL_DEADLINE old (`crates/raft/src/member/election.rs`, `on_suspect`). Run from the
repository root:

    python3 tests/m0carried/plant270.py          # every mutant
    python3 tests/m0carried/plant270.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

E = 'crates/raft/src/member/election.rs'
GATE = 'if self.log.iter().any(|e| e.index > self.state.commit && self.now >= after(e.at, CALL_DEADLINE)) {'
T = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'scenarios',
     'a_suspect_report_marks_a_stall_only_while_an_entry_waits_for_commit']
L012 = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'scenarios', 'a_follower_marks_a_stall_only_from']
MUTANTS = [
    ("every report marks a stall, as before", E, GATE, 'if true {', T),
    ("no report marks a stall", E, GATE, 'if false {', T),
    ("an entry above commit marks at once, whatever its age", E, GATE,
     'if self.log.iter().any(|e| e.index > self.state.commit) {', T),
    ("a committed entry counts", E, GATE,
     'if self.log.iter().any(|e| e.index >= self.state.commit && self.now >= after(e.at, CALL_DEADLINE)) {', T),
    ("the gate reads the leader's clock window, not the entry's age", E, GATE,
     'if self.log.iter().any(|e| e.index > self.state.commit && self.now >= self.state.promise_until) {', T),
]

if __name__ == '__main__':
    run(MUTANTS)
