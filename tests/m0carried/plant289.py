#!/usr/bin/env python3
"""Milestones, format 32.37 (`crates/git/src/issue_ops.rs`, `DefineMilestone`, `DeleteMilestone`, `SetMilestone`,
`EditWithMilestone`, `OpenIssueWithMilestone`; `crates/git/src/tiers.rs`, the `milestone`, `milestone_last`,
`issue_milestone` and `milestone_issue` rows; `crates/git/src/issue_ops/call.rs`, the format; `crates/api/src/rest.rs`,
`milestone_json`, `issue_milestone_json`, the routes' planners and `split_milestone`). The edge's arms are checked by
both edgeboots and plaindiff, whose bed leaves 32.37 off and shows every milestone route and an issue body's
`milestone` answer as main does. Run from the repository root:

    python3 tests/m0carried/plant289.py          # every mutant
    python3 tests/m0carried/plant289.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

I = 'crates/git/src/issue_ops.rs'
T = 'crates/git/src/tiers.rs'
C = 'crates/git/src/issue_ops/call.rs'
R = 'crates/api/src/rest.rs'
G = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'issues', 'milestones_number_count_and_clear_in_one_entry']
A = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces', 'rest_milestones_render_as_ghes_does']
MUTANTS = [
    ("closing or reopening an issue leaves its milestone's counts", T,
     '                    if let Some(m) = issue_milestone(w, n) {\n                        w.delete("milestone_issue", &milestone_issue_key(m, was, n));',
     '                    if let Some(m) = None::<u32> {\n                        w.delete("milestone_issue", &milestone_issue_key(m, was, n));', G),
    ("a deleted milestone stays its issues' milestone", T,
     '                            w.delete("issue_milestone", &[int(*i)]);\n', '', G),
    ("a deleted milestone's number is handed out again", I,
     '                    None => (crate::tiers::milestone_last(v.meta()) + 1, v.next_id(Tier::Metadata)?),',
     '                    None => (crate::tiers::milestones(v.meta()).len() as u32 + 1, v.next_id(Tier::Metadata)?),', G),
    ("a title another milestone has in another case is free", I,
     '                if all.iter().any(|(m, r)| r.title.to_lowercase() == lower && Some(*m) != *n) {',
     '                if all.iter().any(|(m, r)| *r.title == **title && Some(*m) != *n) {', G),
    ("a reader sets an issue's milestone", I,
     '                // GHES applies a milestone for triage or more and ignores the field from anyone else.\n                if perm < Some(RepoPermission::Triage) {',
     '                // GHES applies a milestone for triage or more and ignores the field from anyone else.\n                if perm < Some(RepoPermission::Read) {', G),
    ("a missing milestone is dropped, so the edit with it commits", I,
     '                if milestone.is_some_and(|m| crate::tiers::milestone(v.meta(), m).is_none()) {\n                    return Err(IssueErr::Invalid("milestone"));',
     '                if milestone.is_some_and(|m| crate::tiers::milestone(v.meta(), m).is_none()) {\n                    return Ok((SmallVec::new(), IssueOut::Done));', G),
    ("a reader's create sets the milestone", I,
     '                if perm >= Some(RepoPermission::Triage) {\n                    if crate::tiers::milestone(v.meta(), *milestone).is_none() {',
     '                if perm >= Some(RepoPermission::Read) {\n                    if crate::tiers::milestone(v.meta(), *milestone).is_none() {', G),
    ("an edit of a closed milestone restamps `closed`", T,
     '                        (Some(MilestoneRow { closed: Some(c), .. }), false) => Some(*c),',
     '                        (Some(MilestoneRow { closed: Some(_), .. }), false) => Some(at),', G),
    ("a milestone command keeps 32.5, which a member without its event cannot decode", C,
     '        if self.op.is_milestone() {\n            return REPO_FORMAT_MILESTONES;',
     '        if false {\n            return REPO_FORMAT_MILESTONES;', G),
    ("an issue's milestone renders `null`", R,
     '        "milestone": issue_milestone_json(r, it.n),', '        "milestone": Value::Null,', A),
    ("a milestone's open and closed counts swap", R,
     '        "open_issues": open,\n        "closed_issues": closed,',
     '        "open_issues": closed,\n        "closed_issues": open,', A),
    ("the list shows every state by default", R,
     '    let state = param(query, "state").unwrap_or("open");', '    let state = param(query, "state").unwrap_or("all");', A),
    ("a milestone with no due date sorts first", R,
     '        _ => all.sort_by_key(|(n, m)| (m.due_on.is_none(), m.due_on, *n)),',
     '        _ => all.sort_by_key(|(n, m)| (m.due_on.is_some(), m.due_on, *n)),', A),
    ("an issue body's unknown milestone number is accepted", R,
     '    match n.and_then(|n| u32::try_from(n).ok()).filter(|n| graftyard_git::tiers::milestone(r.meta, *n).is_some()) {',
     '    match n.and_then(|n| u32::try_from(n).ok()) {', A),
    ("a partial PATCH clears the description it leaves out", R,
     '        None => was.and_then(|w| w.description.clone()),', '        None => None,', A),
    ("a milestone's creator is not named, so the edge renders them `ghost`", 'crates/api/src/hosted.rs',
     '        "milestone" => rec::<graftyard_git::tiers::MilestoneRow>(cols).creator,\n', '', A),
]

if __name__ == '__main__':
    run(MUTANTS)
