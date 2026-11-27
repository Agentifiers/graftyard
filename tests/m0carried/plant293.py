#!/usr/bin/env python3
"""F155: an App's command whose `decide` names its bot account in a row its reply renders
(`crates/git/src/issue_ops/call.rs`, `IssueCall::names_bot`, `name_bot`, and `decide`'s `bot_author`), so the edge
reads the bot's login into the render before it commits (`workers/edge/src/api.rs`, `commit_issue_rid`). Without it
an App's milestone create rendered `creator.login` `ghost[bot]` (F137 row r1, octokit 26 of 27). The edge's call
itself reads the App's record over a binding, which no native test reaches: its proof is the hosted octokit check.
Run from the repository root:

    python3 tests/m0carried/plant293.py          # every mutant
    python3 tests/m0carried/plant293.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

C = 'crates/git/src/issue_ops/call.rs'
A = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'task_api', 'an_apps_reply_names_its_bot']
MUTANTS = [
    ("a milestone create names no bot (F155 as found)", C,
     '            | IssueOp::RecordStatus(_)\n            | IssueOp::DefineMilestone { n: None, .. } => true,',
     '            | IssueOp::RecordStatus(_) => true,', A),
    ("a milestone update names the bot too", C,
     'IssueOp::DefineMilestone { n: None, .. } => true,', 'IssueOp::DefineMilestone { .. } => true,', A),
    ("a GraphQL issue or comment names the bot", C,
     '            IssueOp::OpenIssue { .. } | IssueOp::Comment { on: crate::issues::CommentTarget::Issue(_), .. } => {\n'
     '                self.bot_author()\n            }',
     '            IssueOp::OpenIssue { .. } | IssueOp::Comment { on: crate::issues::CommentTarget::Issue(_), .. } => true,',
     A),
    ("a commit status names no bot", C,
     '            IssueOp::OpenIssueWithMilestone { .. }\n            | IssueOp::RecordStatus(_)\n',
     '            IssueOp::OpenIssueWithMilestone { .. }\n', A),
    ("the edge's login is not recorded", C,
     '            logins.insert(app.bot().0.0.to_string(), Value::String(login));', '            let _ = (logins, app, login);',
     A),
    ("the login is recorded under the App's id, not its bot's", C,
     'logins.insert(app.bot().0.0.to_string(), Value::String(login));',
     'logins.insert(app.0.0.to_string(), Value::String(login));', A),
    ("`decide` authors by another rule than `names_bot`'s", C,
     '            let bot = self.bot_author();\n', '            let bot = false;\n', A),
]

if __name__ == '__main__':
    run(MUTANTS)
