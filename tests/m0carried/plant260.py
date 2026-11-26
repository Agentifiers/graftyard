#!/usr/bin/env python3
"""LM-3, OPEN-QUESTIONS A59 (`crates/git/src/taskbox.rs`, `TaskBox::unreported` and `TaskBox::answered`; the host's
`report_base` in `workers/controller/src/taskbox.rs`): the base's task never told the box's state, a 5xx taken as
answered, a 4xx sent again forever, and a cancel sent as a state. Run from the repository root:

    python3 tests/m0carried/plant260.py          # every mutant
    python3 tests/m0carried/plant260.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

B = 'crates/git/src/taskbox.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'taskbox']
MUTANTS = [
    ("the box owes the base nothing", B, '        self.task.state.api().filter(|s| self.base != Some(*s))',
     '        self.task.state.api().filter(|_| false)', T),
    ("a 5xx is taken as answered", B, '        if status < 500 {\n            self.base = Some(state);',
     '        if status < 600 {\n            self.base = Some(state);', T),
    ("a 4xx is sent again", B, '        if status < 500 {\n            self.base = Some(state);',
     '        if status < 300 {\n            self.base = Some(state);', T),
    ("a new box owes the base its queued state", B, 'hold: None, base: Some(TaskApiState::Queued) }',
     'hold: None, base: None }', T),
]

if __name__ == '__main__':
    run(MUTANTS)
