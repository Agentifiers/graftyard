#!/usr/bin/env python3
"""F136, `GET /repos/{owner}/{repo}/pulls` (`crates/api/src/rest.rs`, `list_pulls`; `crates/api/src/narrow.rs`,
`rest_get`): GHES 3.22's list of pull requests, which answered 404 in BF-76 r1. The first mutant is main's rule: the
route is not served. Run from the repository root:

    python3 tests/m0carried/plant276.py          # every mutant
    python3 tests/m0carried/plant276.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

R = 'crates/api/src/rest.rs'
N = 'crates/api/src/narrow.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'narrow', 'rest_pull_lists']
MUTANTS = [
    ("the list is not served, as on main", N,
     '        ["pulls"] => rest::list_pulls(r, query),\n', '', T),
    ("every sort defaults to desc, as GET /issues does", R,
     '        _ => sort != "created",\n', '        _ => false,\n', T),
    ("issues are listed instead of pull requests", R,
     'r.numbers(graftyard_git::issues::IssueKind::PullRequest, state)',
     'r.numbers(graftyard_git::issues::IssueKind::Issue, state)', T),
    ("base is ignored", R,
     '                if base.as_ref().is_some_and(|b| *b != short_branch(&pull.base)) {',
     '                if false && base.as_ref().is_some_and(|b| *b != short_branch(&pull.base)) {', T),
    ("a head with no owner filters by branch", R,
     "        let (o, b) = h.split_once(':')?;", "        let (o, b) = h.split_once(':').unwrap_or((\"o\", &h));", T),
    ("popularity sorts by update", R,
     '            "popularity" => all.sort_by_key(|(n, row)| (row.comments, *n)),',
     '            "popularity" => all.sort_by_key(|(n, row)| (row.updated, *n)),', T),
    ("each item is the full pull-request object", R,
     '        .map(|it| crate::webhooks::pull_request_simple(r, &it))',
     '        .map(|it| crate::webhooks::pull_request(r, &it))', T),
    ("the Link header drops the filters", R,
     'r.at.id.0.0, "/pulls", &keep, page,', 'r.at.id.0.0, "/pulls", &[], page,', T),
]

if __name__ == '__main__':
    run(MUTANTS)
