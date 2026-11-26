#!/usr/bin/env python3
"""F137's REST issue writes (`crates/git/src/issue_ops.rs`, `IssueOp::Edit`; `crates/git/src/issue_ops/call.rs`,
format 32.35; `crates/api/src/rest.rs`, `plan_issue_update`, `plan_issue_lock`, `plan_edit_issue_comment`,
`plan_delete_issue_comment`, `check_assignee`; `crates/api/src/hosted.rs`, the three new `What`s). The edge's arms
are checked by both edgeboots and plaindiff. Run from the repository root:

    python3 tests/m0carried/plant287.py          # every mutant
    python3 tests/m0carried/plant287.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

O = 'crates/git/src/issue_ops.rs'
C = 'crates/git/src/issue_ops/call.rs'
R = 'crates/api/src/rest.rs'
H = 'crates/api/src/hosted.rs'
G = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'issues', 'a_multi_field_issue_edit_commits_whole_or_not_at_all']
A = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces', 'rest_issue_edits_commit_and_render_as_ghes_does']
MUTANTS = [
    ("a refused title or body part leaves the rest of the edit to commit", O,
     '                    part(IssueOp::Change { n: *n, change: change.clone() })?;',
     '                    let _ = part(IssueOp::Change { n: *n, change: change.clone() });', G),
    ("a reader's label set is applied, so their edit is refused whole", O,
     '                    && perm >= Some(RepoPermission::Triage)', '                    && perm >= Some(RepoPermission::Read)',
     G),
    ("an `Edit` keeps 32.33's format, which a 32.33 member reads without the arm", C,
     '        if self.rest_edit || matches!(self.op, IssueOp::Edit { .. }) {', '        if self.rest_edit {', G),
    ("a 32.35 comment edit or lock reads back unmarked", C,
     '        c.rest_edit = edit && IssueCall::edit_mark(&c.op);', '        c.rest_edit = false;', G),
    ("32.35 marks any `Change`, so a title edit has two forms", C,
     '            IssueOp::EditComment { .. } | IssueOp::Change { change: crate::issues::IssueChange::Locked(_), .. }',
     '            IssueOp::EditComment { .. } | IssueOp::Change { .. }', G),
    ("a single title is an `Edit`, so it needs 32.35 where it never did", R,
     '        ([IssueChange::Title(_) | IssueChange::Body(_)], None, None) => {',
     '        ([IssueChange::Body(_)], None, None) => {', A),
    ("a `state` already in force is a change", R, '            if open != it.row.open {', '            if true {', A),
    ("a lock with no reason unlocks", R, '        (true, None | Some(Value::Null)) => Some(LockReason::Unspecified),',
     '        (true, None | Some(Value::Null)) => None,', A),
    ("a comment edit answers 201, as a create does", R,
     '    match issue_comment_by_id(after, id) {\n        Some(v) => Reply::json(200, v),',
     '    match issue_comment_by_id(after, id) {\n        Some(v) => Reply::json(201, v),', A),
    ("a comment delete takes any id, so a deleted comment is deleted again", R,
     '    match id.parse::<i64>().ok().filter(|id| issue_comment_by_id(r, *id).is_some()) {',
     '    match id.parse::<i64>().ok() {', A),
    ("a refused lock renders a release's `tag_name` errors", H,
     '        What::IssueLocked => WireReply::of(&crate::rest::issue_lock_refused(e)),',
     '        What::IssueLocked => WireReply::of(&crate::release::refused(e)),', A),
    ("a reader can be assigned", R, '    if found && perm >= Some(graftyard_identity::RepoPermission::Write) {',
     '    if found && perm.is_some() {', A),
]

if __name__ == '__main__':
    run(MUTANTS)
