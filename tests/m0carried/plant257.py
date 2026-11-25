#!/usr/bin/env python3
"""AM3-WP1 S4, LM-3's kill row (`crates/git/src/taskbox.rs`, `refused`; `workers/controller/src/taskbox.rs`, the
test-faults `/_late` route that replays a killed Sandbox's report and outbound request through it): a late report or
request answered as if taken, a late refusal not logged, a late refusal logged without its Sandbox, and the refusal
dropped from `admit` so late traffic passes. Run from the repository root:

    python3 tests/m0carried/plant257.py          # every mutant
    python3 tests/m0carried/plant257.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

B = 'crates/git/src/taskbox.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'taskbox']
MUTANTS = [
    ("a late refusal answers 200", B, '        Report::Late { from } => (\n            403,', '        Report::Late { from } => (\n            200,', T),
    ("a late refusal is not logged", B, '                b.late\n            )),\n        ),\n',
     '                b.late\n            ))\n            .filter(|_| false),\n        ),\n', T),
    ("a late refusal's line names no Sandbox", B, '                from.0,\n', '                "",\n', T),
    ("the late refusal is dropped: an old Sandbox's traffic passes", B,
     '        if self.hold.is_some() || self.current() != Some(from) {\n            self.late += 1;\n            return Err(Report::Late { from: from.clone() });\n        }\n',
     '', T),
]

if __name__ == '__main__':
    run(MUTANTS)
