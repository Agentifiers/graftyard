#!/usr/bin/env python3
"""AM3-WP3, F20's assignment changes and the controller's create (`crates/git/src/task.rs`, `decide`'s
Graftyard-run create; `crates/git/src/issue_ops.rs`, `IssueOp::Assign`; `crates/git/src/tiers.rs`, `assigned_by`;
`decisions/DECISIONS.md`, "`assign`"): the controller's create allowed without a user's assignment, a task-claimed
token allowed, any App's bot assignable, a non-writer assignable, a
caller without triage changing assignees, a removed bot's row kept, and the App's own add naming a user. Run from the
repository root:

    python3 tests/m0carried/plant263.py          # every mutant
    python3 tests/m0carried/plant263.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

TASK = 'crates/git/src/task.rs'
OPS = 'crates/git/src/issue_ops.rs'
TIERS = 'crates/git/src/tiers.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'task_gates', 'the_controller']
MUTANTS = [
    ("the controller creates without a user's assignment", TASK,
     '                        Some(u) if controller => u,\n                        _ => return Err(TaskErr::NotWriter),',
     '                        Some(u) if controller => u,\n                        _ if controller => graftyard_identity::UserId(app.bot()),\n'
     '                        _ => return Err(TaskErr::NotWriter),', T),
    ("a task-claimed token creates", TASK,
     '            let controller = ctx.run == Some((app, installation)) && claim(p).is_none() && n.issue.is_some();',
     '            let controller = ctx.run == Some((app, installation)) && n.issue.is_some();', T),
    ("any App's bot is assignable", OPS, '                    Some(_) => Some(*a) == bot,',
     '                    Some(_) => true,', T),
    ("a non-writer is assignable", OPS,
     '                    None => permission(&user_principal(*a), v, &folds) >= Some(RepoPermission::Write),',
     '                    None => permission(&user_principal(*a), v, &folds) >= Some(RepoPermission::Read),', T),
    ("a caller without triage changes assignees", OPS,
     '                if perm < Some(RepoPermission::Triage) {\n                    return Ok((SmallVec::new(), IssueOut::Done));',
     '                if perm < Some(RepoPermission::Read) {\n                    return Ok((SmallVec::new(), IssueOut::Done));', T),
    ("a removed assignee's row is kept", TIERS, '                    w.delete("issue_assignee", &[n_key(*n), int(a.0.0)]);',
     '                    let _ = (n, a);', T),
    ("an App's own add of its bot names a user", TIERS,
     '    by.filter(|b| graftyard_identity::AppId::of_bot(*b).is_none()).map(graftyard_identity::UserId)',
     '    by.map(|b| graftyard_identity::UserId(graftyard_types::AccountId(graftyard_types::DbId(b.0.0.abs()))))', T),
]

if __name__ == '__main__':
    run(MUTANTS)
