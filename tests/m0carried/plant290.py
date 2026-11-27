#!/usr/bin/env python3
"""Milestones head 2, format 32.37: GraphQL's `Issue.milestone`, `Repository.milestones(states:, first:)` and
`milestone(number:)` (`crates/api/src/gql.rs`, `Milestone` and `milestones`), served only once the edge's directory
says 32.37 is enabled (`crates/api/src/hosted.rs`, `HostedDir::milestones`); and an App's milestone write by either
`issues` or `pull_requests` (`crates/git/src/issue_ops.rs`, `app_needs`), created by its bot. Run from the repository
root:

    python3 tests/m0carried/plant290.py          # every mutant
    python3 tests/m0carried/plant290.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

Q = 'crates/api/src/gql.rs'
H = 'crates/api/src/hosted.rs'
I = 'crates/git/src/issue_ops.rs'
A = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces',
     'graphql_milestones_read_what_rest_wrote_and_a_pull_requests_app_writes_them']
MUTANTS = [
    ("an issue's milestone is `null` again", Q,
     '            "milestone" => Some(milestone(r, graftyard_git::tiers::issue_milestone(r.meta, n))),',
     '            "milestone" => Some(Out::Null),', A),
    ("`states` is ignored, so a closed milestone is listed as open", Q,
     '        .filter(|(_, m)| states.is_empty() || states.contains(&if m.open { "OPEN" } else { "CLOSED" }))',
     '        .filter(|(_, _)| true)', A),
    ("`after` is ignored, so every page is the first", Q,
     '    let start = a.get("after").and_then(Value::as_str).and_then(parse_cursor).map_or(0, |c| c as usize);\n    let n = first_or_last(a);\n    let page: Vec<(u32, MilestoneRow)>',
     '    let start = 0;\n    let n = first_or_last(a);\n    let page: Vec<(u32, MilestoneRow)>', A),
    ("a full page says no page follows", Q,
     '    let more = page.len() > n;\n    let nodes: Vec<Box<dyn Obj + \'r>> =\n        page.into_iter().take(n).map(|(n, row)| Box::new(Milestone',
     '    let more = false;\n    let nodes: Vec<Box<dyn Obj + \'r>> =\n        page.into_iter().take(n).map(|(n, row)| Box::new(Milestone', A),
    ("an `orderBy` is answered by number", Q,
     '        ["orderBy", "query", "last", "before"].into_iter()', '        ["query", "last", "before"].into_iter()', A),
    ("`milestone(number:)` reads the milestone one lower", Q,
     '"milestone" => milestone(r, a.get("number").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok())),',
     '"milestone" => milestone(r, a.get("number").and_then(Value::as_u64).and_then(|n| u32::try_from(n - 1).ok())),', A),
    ("the milestone fields are served before 32.37", Q,
     '            "milestones" | "milestone" if !r.dir.milestones() => return None,\n', '', A),
    ("the edge's directory never says 32.37 is on", H,
     '    fn milestones(&self) -> bool {\n        self.milestones\n    }',
     '    fn milestones(&self) -> bool {\n        false\n    }', A),
    ("a `pull_requests` App's milestone write is refused", I,
     '            IssueOp::DefineMilestone { .. } | IssueOp::DeleteMilestone { .. } => {\n                smallvec![w("issues"), w("pull_requests")]',
     '            IssueOp::DefineMilestone { .. } | IssueOp::DeleteMilestone { .. } => {\n                smallvec![w("issues")]', A),
    ("any App writes a milestone", I,
     '            IssueOp::DefineMilestone { .. } | IssueOp::DeleteMilestone { .. } => {\n                smallvec![w("issues"), w("pull_requests")]',
     '            IssueOp::DefineMilestone { .. } | IssueOp::DeleteMilestone { .. } => {\n                smallvec![w("issues"), w("pull_requests"), w("contents")]', A),
    ("an App's milestone has no creator", I,
     '                    Actor::App { app, .. } => Some(app.bot()),\n                    _ => me,\n                };\n                let ev = MilestoneEvent::Defined {',
     '                    Actor::App { .. } => None,\n                    _ => me,\n                };\n                let ev = MilestoneEvent::Defined {', A),
]

if __name__ == '__main__':
    run(MUTANTS)
