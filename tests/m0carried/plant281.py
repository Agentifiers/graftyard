#!/usr/bin/env python3
"""AM3-WP3, the host pause (`crates/git/src/issue_ops.rs`, the `Change` and `Assign` arms of `decide_in`;
`crates/git/src/task.rs`, `issue_runs` and `write_resumed_meta`; `crates/git/src/taskbox.rs`, `Input::Base`;
`decisions/DECISIONS.md`, "`host-pause`"): a writer's edit pausing, a non-writer's edit not pausing, an App's or a
non-writer's reassignment resuming, the resume's snapshot not moving, an unmarked edit pausing, and a box that
ignores the base's pause. Run from the repository root:

    python3 tests/m0carried/plant281.py          # every mutant
    python3 tests/m0carried/plant281.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

OPS = 'crates/git/src/issue_ops.rs'
TASK = 'crates/git/src/task.rs'
BOX = 'crates/git/src/taskbox.rs'
GATES = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'task_gates', '--', 'pause', 'resume', 'unmarked']
TB = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'taskbox', 'pause']
MUTANTS = [
    ("a writer's edit pauses the task", OPS,
     '                    && perm < Some(RepoPermission::Write)\n                    && let (Some(owner), Some(actor))',
     '                    && perm <= Some(RepoPermission::Write)\n                    && let (Some(owner), Some(actor))', GATES),
    ("a non-writer's edit pauses nothing", OPS,
     '                    && perm < Some(RepoPermission::Write)\n                    && let (Some(owner), Some(actor))',
     '                    && perm < Some(RepoPermission::Read)\n                    && let (Some(owner), Some(actor))', GATES),
    ("an App's or an AgentApp user token's reassignment resumes", OPS,
     '                let human = matches!(self.actor.actor, Actor::User(_))\n'
     '                    && !self.credential_app(deps).is_some_and(|a| crate::agent::agent_app(install, a).is_some());',
     '                let human = true;', GATES),
    ("a triager's reassignment resumes", OPS,
     '                    && human\n                    && perm >= Some(RepoPermission::Write)',
     '                    && human\n                    && perm >= Some(RepoPermission::Read)', GATES),
    ("the resume keeps the create's snapshot", TASK,
     '    m.snapshot = snapshot;\n', '    let _ = snapshot;\n', GATES),
    ("an unmarked edit pauses", OPS,
     '                if pause.is_some()\n                    && matches!(change, IssueChange::Title(_) | IssueChange::Body(_))',
     '                if matches!(change, IssueChange::Title(_) | IssueChange::Body(_))', GATES),
    ("the box keeps its Sandbox when the base pauses", BOX,
     '                        self.hold = None;\n                        Ok(Act::Stop)\n                    }\n'
     '                    // Resumed on the base',
     '                        self.hold = None;\n                        Ok(Act::Keep)\n                    }\n'
     '                    // Resumed on the base', TB),
]

if __name__ == '__main__':
    run(MUTANTS)
