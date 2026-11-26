#!/usr/bin/env python3
"""F137's issue reads from the fold (`crates/api/src/rest.rs`, `list_issue_labels`, `list_issue_events`,
`list_repo_issue_events`, `get_issue_event`, `list_timeline`, `list_repo_issue_comments`, `get_issue_comment`;
`crates/api/src/narrow.rs`, `rest_get`), each of which answered 404 on main. The first two mutants are main's rules:
the route is not served, or `issues/{n}` takes the repository-wide tail. The edge's arms are checked by both edgeboots
and plaindiff. Run from the repository root:

    python3 tests/m0carried/plant282.py          # every mutant
    python3 tests/m0carried/plant282.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

R = 'crates/api/src/rest.rs'
N = 'crates/api/src/narrow.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'narrow', 'issue_reads_serve_what_the_fold_records']
MUTANTS = [
    ("the repository's comments are not served, as on main", N,
     '        ["issues", "comments"] => rest::list_repo_issue_comments(r, query),\n', '', T),
    ("`issues/{n}` takes `issues/events`, as an arm after it would", N,
     '        ["issues", "events"] => rest::list_repo_issue_events(r, query),\n', '', T),
    ("the repository's comments include review comments", R,
     '                let keep = matches!(row.on, CommentTarget::Issue(_)) && since.is_none_or(|s| row.updated >= s);',
     '                let keep = since.is_none_or(|s| row.updated >= s);', T),
    ("`since` reads the creation time", R,
     '                let keep = matches!(row.on, CommentTarget::Issue(_)) && since.is_none_or(|s| row.updated >= s);',
     '                let keep = matches!(row.on, CommentTarget::Issue(_)) && since.is_none_or(|s| row.created >= s);',
     T),
    ("a `sort` with no `direction` is ascending", R,
     '        (Some("created" | "updated"), None | Some("desc")) => false,',
     '        (Some("created" | "updated"), Some("desc")) => false,\n        (Some("created" | "updated"), None) => true,',
     T),
    ("the comment list's Link drops `sort`", R,
     '        .filter(|(k, _)| matches!(*k, "sort" | "direction" | "since"))',
     '        .filter(|(k, _)| matches!(*k, "direction" | "since"))', T),
    ("the timeline is in id order, not time order", R,
     '    all.sort_by_key(|(at, id, _)| (*at, *id));', '    all.sort_by_key(|(_, id, _)| *id);', T),
    ("the timeline's review state is upper case, as `pulls/{n}/reviews` has it", R,
     '        v["state"] = json!(review_state(&row).to_lowercase());', '        v["state"] = json!(review_state(&row));', T),
    ("the repository's events are oldest first", R,
     '        all.sort_unstable_by(|a, b| b.cmp(a));', '        all.sort_unstable();', T),
    ("an event's id is the item's number, which `issues/events/{id}` cannot find", R,
     '        "id": id, "node_id": scoped(NodeType::MergedEvent, r, id),',
     '        "id": it.n.0, "node_id": scoped(NodeType::MergedEvent, r, id),', T),
    ("an item's labels list another item's", R,
     '    let all = r.labels_of(it.n).iter().map(|(id, l)| label_json(r, *id, l)).collect();',
     '    let all = r.labels_of(IssueNumber(1)).iter().map(|(id, l)| label_json(r, *id, l)).collect();', T),
]

if __name__ == '__main__':
    run(MUTANTS)
