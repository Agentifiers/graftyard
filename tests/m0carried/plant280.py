#!/usr/bin/env python3
"""F137, GHES 3.22's single-issue read and REST issue comment (`crates/api/src/rest.rs`, `get_issue`,
`plan_create_issue_comment`, `issue_comment_created`; `crates/api/src/narrow.rs`, `rest_get`;
`crates/git/src/issue_ops/call.rs`, format 32.32; `crates/git/src/issue_ops.rs`, the bot author), which answered 404
in AM2's LM-3 batch r1. The first two mutants are main's rules: neither route is served. The edge's arms and their
format gate are checked by both edgeboots and plaindiff. Run from the repository root:

    python3 tests/m0carried/plant280.py          # every mutant
    python3 tests/m0carried/plant280.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

R = 'crates/api/src/rest.rs'
N = 'crates/api/src/narrow.rs'
CALL = 'crates/git/src/issue_ops/call.rs'
OPS = 'crates/git/src/issue_ops.rs'
READ = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'narrow', 'single_object_routes']
CREATE = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces', 'rest_issue_comment_create']
FORMAT = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'issues', 'a_rest_issue_comment_rides']
MUTANTS = [
    ("the issue read is not served, as on main", N,
     '        ["issues", n] => rest::get_issue(r, n),\n', '', READ),
    ("the comment create is not planned, as on main", R,
     '    let Some(text) = body.get("body").and_then(Value::as_str) else {',
     '    let Some(text) = None::<&str> else {', CREATE),
    ("a pull request's number is 404 on the issue read", R,
     '        Some(it) => Reply::json(200, issue_json(r, &it)),',
     '        Some(it) if !it.is_pull() => Reply::json(200, issue_json(r, &it)),\n        Some(_) => Reply::not_found(),',
     READ),
    ("a comment on a missing issue is planned", R,
     '    let Some(n) = n.parse::<u32>().ok().map(IssueNumber).filter(|n| r.item(*n).is_some()) else {',
     '    let Some(n) = n.parse::<u32>().ok().map(IssueNumber) else {', CREATE),
    ("the 201 has no Location", R,
     '            Reply::json(201, v).with("location", loc)', '            let _ = loc;\n            Reply::json(201, v)',
     CREATE),
    ("the REST comment keeps 32.5, which an older member would commit and then fail to render", CALL,
     '        if self.rest_comment {', '        if self.rest_comment && false {', FORMAT),
    ("the 32.32 mark is read for a review comment", CALL,
     '        c.rest_comment = comment && IssueCall::issue_comment(&c.op);', '        c.rest_comment = comment;', FORMAT),
    ("an App's REST comment is author-less", OPS,
     '                    (Actor::App { app, .. }, CommentTarget::Issue(_)) if bot_author => Some(app.bot()),',
     '                    (Actor::App { app, .. }, CommentTarget::Issue(_)) if bot_author && false => Some(app.bot()),',
     FORMAT),
]

if __name__ == '__main__':
    run(MUTANTS)
