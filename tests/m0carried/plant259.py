#!/usr/bin/env python3
"""BF-77's token half, `app-bot` (`crates/git/src/issue_ops.rs` `app_needs` and `RecordStatus`,
`crates/git/src/issue_ops/call.rs` 32.29, `crates/api/src/wire.rs` `bot_user`, `crates/api/src/rest.rs`
`plan_create_issue` and `issue_created`; `decisions/DECISIONS.md`, "`app-bot`"). The mutants: an App's status needing
no `statuses` permission, an App's status taking any creator, the bot account the App's id rather than its negation,
an App's status left in 32.9, the bot rendered as a user, the bot's `id` its negative account id, a REST issue by an App
left author-less, an issue create's labels dropped, an unknown label passed over, and the 201 without `Location`. The edge's routes are checked by
`tests/bf77bed/run.sh`, which this file does not run. Run from the repository root:

    python3 tests/m0carried/plant259.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

IO = 'crates/git/src/issue_ops.rs'
CA = 'crates/git/src/issue_ops/call.rs'
AP = 'crates/identity/src/apps.rs'
WI = 'crates/api/src/wire.rs'
RE = 'crates/api/src/rest.rs'
TI = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'issues', 'an_installation_token_sets']
TS = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces', 'rest_issue_create']
MUTANTS = [
    ("an App's status needs no `statuses` permission", IO,
     'IssueOp::RecordStatus(_) if matches!(self.actor.actor, Actor::App { .. }) => smallvec![w("statuses")],',
     'IssueOp::RecordStatus(_) if matches!(self.actor.actor, Actor::App { .. }) => smallvec![w("contents")],', TI),
    ("an App's status takes any creator", IO,
     '                if creator != Some(st.creator) {', '                if creator.is_none() {', TI),
    ("the bot account is the App's id, not its negation", AP,
     '        graftyard_types::AccountId(DbId(-self.0.0))', '        graftyard_types::AccountId(self.0)', TI),
    ("an App's status stays in 32.9", CA,
     '            || (matches!(self.op, IssueOp::RecordStatus(_)) && matches!(self.actor.actor, Actor::App { .. }))\n',
     '            || false\n', TI),
    ("the bot renders as a user", WI, '"type": "Bot", "site_admin": false,', '"type": "User", "site_admin": false,', TS),
    ("the bot's id is its negative account id", WI,
     '"login": login, "id": app.0.0, "node_id": NodeId::top(NodeType::Bot, app.0.0).encode(),',
     '"login": login, "id": id.0.0, "node_id": NodeId::top(NodeType::Bot, app.0.0).encode(),', TS),
    ("a REST issue by an App has no author", IO,
     '                    Actor::App { app, .. } if bot_author => Some(app.bot()),',
     '                    Actor::App { app, .. } if bot_author && app.0.0 < 0 => Some(app.bot()),', TI),
    ("an issue create drops its labels", RE,
     '            if !labels.contains(&id) {\n                labels.push(id);', '            if labels.contains(&id) {\n                labels.push(id);', TS),
    ("an unknown label is passed over", RE,
     '            let id = id.ok_or_else(|| invalid("labels", "invalid"))?;\n',
     '            let Some(id) = id else { continue };\n', TS),
    ("the 201 has no Location", RE,
     '            Reply::json(201, issue_json(after, &it)).with("location", loc)',
     '            Reply::json(201, issue_json(after, &it)).with("x-location", loc)', TS),
]

if __name__ == '__main__':
    run(MUTANTS)
