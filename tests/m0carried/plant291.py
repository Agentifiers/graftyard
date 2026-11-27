#!/usr/bin/env python3
"""F149, format 32.37: gh's milestone writes through GraphQL (`crates/api/src/graphql.rs`, `plan`'s `milestoneId`,
`createIssue`, `updatePullRequest` and `updateIssue`, `milestone_of`, `route`). Each plans the 32.37 command REST's
issue body plans (`crate::rest::with_milestone`), so `decide` adds nothing. The edge's `updateIssue` route gate is
checked by plaindiff, whose bed leaves 32.37 off. Run from the repository root:

    python3 tests/m0carried/plant291.py          # every mutant
    python3 tests/m0carried/plant291.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

G = 'crates/api/src/graphql.rs'
A = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces', 'gh_milestone_writes_commit_the_32_37_commands']
MUTANTS = [
    ("a milestone node id is read as its number", G,
     '    graftyard_git::tiers::milestones(r.meta).into_iter().find(|(_, m)| m.id.0 == id.id()).map(|(n, _)| n)',
     '    u32::try_from(id.id()).ok().filter(|n| graftyard_git::tiers::milestone(r.meta, *n).is_some())', A),
    ("another repository's milestone id is accepted", G,
     'n.ty == NodeType::Milestone && n.repo() == Some(r.at.id))?;', 'n.ty == NodeType::Milestone)?;', A),
    ("`milestoneId: null` leaves the milestone", G,
     '            Some(Value::Null) => Some(None),\n', '            Some(Value::Null) => None,\n', A),
    ("`milestoneId` is served before 32.37", G,
     '    let served = |k: &str| k == "milestoneId" && r.dir.milestones();',
     '    let served = |k: &str| k == "milestoneId";', A),
    ("`createIssue` drops its milestone", G,
     '            let op = match milestone()? {\n                Some(m) => crate::rest::with_milestone(op, m),\n                None => op,\n            };',
     '            let op = match milestone()? {\n                Some(_) => op,\n                None => op,\n            };', A),
    ("an edit with a milestone drops its title and body", G,
     '                Some(m) => crate::rest::with_milestone(IssueOp::Edit { n, changes, labels: None, assignees: None }, m),',
     '                Some(m) => IssueOp::SetMilestone { n, milestone: m },', A),
    ("`updateIssue` with a title and body is refused", G,
     '                None if !pull && !changes.is_empty() => IssueOp::Edit { n, changes, labels: None, assignees: None },\n', '', A),
    ("`updateIssue` names a pull request", G,
     '                .filter(|n| r.item(*n).is_some_and(|it| it.is_pull() == pull))',
     '                .filter(|n| r.item(*n).is_some())', A),
    ("`updateIssue` is served before 32.37", G,
     '        "updatePullRequest" | "updateIssue" if field == "updatePullRequest" || r.dir.milestones() => {',
     '        "updatePullRequest" | "updateIssue" => {', A),
    ("`updateIssue` routes nowhere", G,
     '            "dequeuePullRequest" | "updateIssue" => {', '            "dequeuePullRequest" => {', A),
    ("`updateIssue`'s payload has no issue", G,
     '            ("issue", IssueOut::Done) if self.root.p.field == "updateIssue" => node(self.root.p.subject?),\n', '', A),
]

if __name__ == '__main__':
    run(MUTANTS)
