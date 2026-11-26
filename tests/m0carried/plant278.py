#!/usr/bin/env python3
"""F138, a task's requester and creator logins (`crates/api/src/hosted.rs`, `NAMING` and `row_account`): the task
row left out of the accounts the tiers name, so a requester no other row names renders as `ghost`. A task's user
creator is its requester, so no mutant drops it separately. Run from the repository root:

    python3 tests/m0carried/plant278.py          # every mutant
    python3 tests/m0carried/plant278.py 0        # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

HOSTED = 'crates/api/src/hosted.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'task_api', 'command']
MUTANTS = [
    ("the task's requester is not a named account (main's rule)", HOSTED,
     '        "agent_task" => rec::<graftyard_git::task::TaskCore>(cols).requester.map(|u| u.0),',
     '        "agent_task" => None,', T),
]

if __name__ == '__main__':
    run(MUTANTS)
