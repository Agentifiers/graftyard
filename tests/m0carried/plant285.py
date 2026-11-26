#!/usr/bin/env python3
"""F141, GHES 3.22's issue update for one field (`crates/api/src/rest.rs`, `plan_issue_edit`; `workers/edge/src/api.rs`,
its arm behind 32.33): a `PATCH .../issues/{n}` whose body names exactly one of `title` and `body` is one title or body
`Change`, which LM-3's in-repo row needs for the host pause. The first mutant is main's rule: the route is not served.
The others would split a multi-field update into commands that are not atomic, or plan the wrong change. The edge's
arm and its format gate are checked by both edgeboots and plaindiff. Run from the repository root:

    python3 tests/m0carried/plant285.py          # every mutant
    python3 tests/m0carried/plant285.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

R = 'crates/api/src/rest.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces', 'rest_issue_patch_of_one_field']
MUTANTS = [
    ("the edit is not planned, as on main", R,
     '    let o = body.as_object().filter(|o| o.len() == 1)?;',
     '    let o = body.as_object().filter(|_| false)?;', T),
    ("a two-field body plans its first field alone", R,
     '    let o = body.as_object().filter(|o| o.len() == 1)?;',
     '    let o = body.as_object()?;', T),
    ("a null body is refused instead of clearing it", R,
     '        (None, Some(Value::Null)) => IssueChange::Body("".into()),\n', '', T),
    ("a missing issue is planned", R,
     '    match n.parse::<u32>().ok().map(IssueNumber).filter(|n| r.item(*n).is_some()) {',
     '    match n.parse::<u32>().ok().map(IssueNumber) {', T),
    ("a body edit plans a title change", R,
     '        (None, Some(Value::String(b))) => IssueChange::Body(b.as_str().into()),',
     '        (None, Some(Value::String(b))) => IssueChange::Title(b.as_str().into()),', T),
]

if __name__ == '__main__':
    run(MUTANTS)
