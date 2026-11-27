#!/usr/bin/env python3
"""F160, format 32.38: an App's release create, asset upload and review comment are authored by its bot account
(`crates/git/src/issue_ops/call.rs`, `REPO_FORMAT_APP_AUTHOR`, `IssueCall::bot_mark`, `bot_author`, the binding; and
`crates/git/src/issue_ops.rs`, `decide_in`'s `by`). Only a command committed under 32.38 names the bot: an earlier
entry keeps `null`, and a release update keeps its author. GraphQL's `Release.author`, a `User`, stays `null` for a bot
(`crates/api/src/gql.rs`). The edge's mark is three lines whose proof is the hosted row. Run from the repository root:

    python3 tests/m0carried/plant295.py          # every mutant
    python3 tests/m0carried/plant295.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

C = 'crates/git/src/issue_ops/call.rs'
D = 'crates/git/src/issue_ops.rs'
G = 'crates/api/src/gql.rs'
A = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'task_api', 'an_apps_']
MUTANTS = [
    ("32.38 does not author by the bot (F160 as found)", C,
     'REPO_FORMAT_ROUTES | REPO_FORMAT_ISSUE_COMMENT | REPO_FORMAT_APP_AUTHOR)',
     'REPO_FORMAT_ROUTES | REPO_FORMAT_ISSUE_COMMENT)', A),
    ("every App release, asset and comment names the bot, 32.38 or not", D,
     '            Actor::App { app, .. } if bot_author => Some(app.bot()),\n            _ => me,\n        };\n        let need',
     '            Actor::App { app, .. } => Some(app.bot()),\n            _ => me,\n        };\n        let need', A),
    ("a release update is marked 32.38", C,
     '            IssueOp::PutRelease { id: None, .. }\n                | IssueOp::PutAsset { .. }\n                | IssueOp::Comment',
     '            IssueOp::PutRelease { .. }\n                | IssueOp::PutAsset { .. }\n                | IssueOp::Comment', A),
    ("an upload is not marked", C,
     '            IssueOp::PutRelease { id: None, .. }\n                | IssueOp::PutAsset { .. }\n                | IssueOp::Comment',
     '            IssueOp::PutRelease { id: None, .. }\n                | IssueOp::Comment', A),
    ("a review comment is not marked", C,
     '                | IssueOp::PutAsset { .. }\n                | IssueOp::Comment { on: crate::issues::CommentTarget::Review { .. }, .. }\n',
     '                | IssueOp::PutAsset { .. }\n', A),
    ("an upload is authored by no one", D,
     'let ev = RepoEvent::ReleaseAsset { release: *release, id, asset: Some(asset), actor: by };',
     'let ev = RepoEvent::ReleaseAsset { release: *release, id, asset: Some(asset), actor: me };', A),
    ("a release create is authored by no one", D,
     '                let author = old.map_or(by, |o| o.rec.author);', '                let author = old.map_or(me, |o| o.rec.author);', A),
    ("a release update takes the bot from the original author", D,
     '                let author = old.map_or(by, |o| o.rec.author);', '                let author = by.or(old.and_then(|o| o.rec.author));',
     A),
    ("the comment's single-comment review has no reviewer", D,
     'ReviewEvent::SingleComment { id: review, n, reviewer: by, commit: at.commit }',
     'ReviewEvent::SingleComment { id: review, n, reviewer: me, commit: at.commit }', A),
    ("32.38's mark does not cross the binding", C,
     '        c.app_bot = bots && IssueCall::bot_mark(&c.op);\n', '', A),
    ("32.38 carries no single-comment review over the binding", C,
     '        c.reviews = (reviews || bots) && IssueCall::single_comment(&c.op);',
     '        c.reviews = reviews && IssueCall::single_comment(&c.op);', A),
    ("a 32.38 command is not 32.38", C,
     '        if self.app_bot {\n            return REPO_FORMAT_APP_AUTHOR;', '        if false {\n            return REPO_FORMAT_APP_AUTHOR;', A),
    ("GraphQL's `Release.author` names a bot as a `User`", G,
     '"author" => account(r, rec.author.filter(|a| graftyard_identity::AppId::of_bot(*a).is_none())),',
     '"author" => account(r, rec.author),', A),
    ("the edge reads no bot login for an upload", C,
     '            | IssueOp::PutAsset { .. } => self.bot_author(),', '            | IssueOp::PutAsset { .. } => false,', A),
]

if __name__ == '__main__':
    run(MUTANTS)
