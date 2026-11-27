#!/usr/bin/env python3
"""F156, a cancel before or during a task's first start is final (`crates/git/src/taskbox.rs`, `TaskBox::queued`,
`start_due`, `after_start`, `unstarted`): a cancel that came first is dropped, a cancelled box still owes its start, a
container a start began as the cancel landed keeps running, and a first start that keeps failing leaves the task
queued. Run from the repository root:

    python3 tests/m0carried/plant294.py          # every mutant
    python3 tests/m0carried/plant294.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

BOX = 'crates/git/src/taskbox.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'taskbox', 'first_start']
MUTANTS = [
    ("a cancel that came first is dropped", BOX,
     '        if cancelled_first {\n            b.cancel();\n        }',
     '        let _ = cancelled_first;', TESTS),
    ("a cancelled box still owes its first start", BOX,
     '        self.task.state == TaskState::Queued && self.generation == 0',
     '        self.generation == 0', TESTS),
    ("a container a start began as the cancel landed keeps running", BOX,
     '        if running && self.task.state.is_final() { Act::Stop } else { Act::Keep }',
     '        let _ = running;\n        Act::Keep', TESTS),
    ("a first start that keeps failing leaves the task queued", BOX,
     '        if self.start_tries >= START_TRIES {',
     '        if self.start_tries >= START_TRIES && false {', TESTS),
]

if __name__ == '__main__':
    run(MUTANTS)
