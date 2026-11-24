#!/usr/bin/env python3
"""AM3-WP1, the controller's task model (`crates/git/src/controller.rs`): the fork named after the base's owner, the
workspace org's login limit ignored, each budget limit off by one or dropped, a non-writer's edit not pausing, any
reassignment resuming a pause, the snapshot not moving, a final task stepping, and `Cancelled` or `Draft` sending the
wrong base state. Run from the repository root:

    python3 tests/m0carried/plant221.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

C = 'crates/git/src/controller.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'controller']
MUTANTS = [
    ("the fork is the base owner's repository", C,
     'Login::parse(&format!("{}-agents", base.owner.as_str()))', 'Login::parse(base.owner.as_str())', T),
    ("an over-long workspace org login is used anyway", C,
     '.map_err(|_| NoWorkspaceOrg)?', '.unwrap_or_else(|_| base.owner.clone())', T),
    ("a task at its token limit fails", C, 'used.task_tokens > self.task_tokens', 'used.task_tokens >= self.task_tokens', T),
    ("the org's monthly budget is not checked", C,
     'used.org_month_tokens > self.org_month_tokens', 'false && used.org_month_tokens > self.org_month_tokens', T),
    ("the wall is reached only past it", C, 'used.elapsed >= self.wall', 'used.elapsed > self.wall', T),
    ("a non-writer's edit does not pause", C,
     '(S::Queued | S::Running { .. }, TaskEvent::IssueEdited { by_writer: false }) => paused(),', '', T),
    ("any reassignment resumes a paused task", C,
     '(S::Paused { .. }, TaskEvent::Reassigned { by_writer: true, at }) => {',
     '(S::Paused { .. }, TaskEvent::Reassigned { at, .. }) => {', T),
    ("a reassignment does not move the snapshot", C, '                self.snapshot = Some(at);\n', '                let _ = at;\n', T),
    ("a final task takes steps", C, '        if self.state.is_final() {\n            return Err(StepErr::Final);',
     '        if false {\n            return Err(StepErr::Final);', T),
    ("Cancelled is sent with PATCH", C, '            TaskState::Cancelled => return None,',
     '            TaskState::Cancelled => TaskApiState::Cancelled,', T),
    ("a draft PR sends in_progress", C, '            TaskState::Draft { .. } | TaskState::Done => TaskApiState::Completed,',
     '            TaskState::Draft { .. } => TaskApiState::InProgress,\n            TaskState::Done => TaskApiState::Completed,', T),
]

if __name__ == '__main__':
    run(MUTANTS)
