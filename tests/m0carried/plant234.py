#!/usr/bin/env python3
"""AM3-WP1, LM-3's "a Sandbox killed mid-task resumes", the restart rule (`crates/git/src/controller.rs`,
`TaskEvent::Lost` and `Task::resume`): a restart that drops the last push so the new Sandbox starts from the base, a
restart that keeps the lost Sandbox, a late report about a Sandbox the task no longer holds taken anyway, and a resume
that starts from the base after a push. Run from the repository root:

    python3 tests/m0carried/plant234.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

C = 'crates/git/src/controller.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'controller']
MUTANTS = [
    ("a restart drops the last push", C,
     '                S::Running { sandbox: new, last_push: *last_push }\n',
     '                let _ = last_push;\n                S::Running { sandbox: new, last_push: None }\n', T),
    ("a restart keeps the lost Sandbox", C,
     '                S::Running { sandbox: new, last_push: *last_push }\n',
     '                let _ = new;\n                S::Running { sandbox: sandbox.clone(), last_push: *last_push }\n', T),
    ("a late report about another Sandbox restarts the task", C,
     '                if *sandbox == lost && new != lost =>\n', '                if new != lost =>\n', T),
    ("a resume after a push starts from the base", C,
     '            TaskState::Running { last_push: Some(h), .. } => Some(Resume::Push(*h)),\n',
     '            TaskState::Running { last_push: Some(_), .. } => Some(Resume::Base),\n', T),
]

if __name__ == '__main__':
    run(MUTANTS)
