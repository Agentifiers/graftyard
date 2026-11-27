#!/usr/bin/env python3
"""F137's label routes and the route docs of `What::Deleted` (`crates/api/src/rest.rs`, `get_label`,
`plan_update_label`, `plan_delete_label`, `label_updated`, `label_update_refused`, `check_repo_assignee`;
`crates/api/src/hosted.rs`, `Render::doc` and `What::LabelUpdated`; `crates/wire/src/reply.rs`, `with_doc`;
`crates/git/src/issue_ops/call.rs`, format 32.36). The edge's arms are checked by both edgeboots and plaindiff, whose
bed shows a refused delete's `documentation_url` move and nothing else. Run from the repository root:

    python3 tests/m0carried/plant288.py          # every mutant
    python3 tests/m0carried/plant288.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

R = 'crates/api/src/rest.rs'
H = 'crates/api/src/hosted.rs'
W = 'crates/wire/src/reply.rs'
C = 'crates/git/src/issue_ops/call.rs'
A = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces', 'rest_label_routes_and_route_docs_render_as_ghes_does']
G = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'issues', 'a_rest_label_update_is_32_36_and_keeps_its_issues']
MUTANTS = [
    ("a label by name is matched case-sensitively", R,
     '    r.labels().into_iter().find(|(_, l)| l.name.0.eq_ignore_ascii_case(&name))',
     '    r.labels().into_iter().find(|(_, l)| *l.name.0 == *name)', A),
    ("a label's name is not percent-decoded", R,
     '    let name = unpercent(name);\n    r.labels()', '    let name = name.to_owned();\n    r.labels()', A),
    ("a body that leaves out the color resets it", R,
     '        None => row.color,', '        None => [0xed, 0xed, 0xed],', A),
    ("an empty description is kept, not cleared", R,
     '        Some(Value::String(d)) => (!d.is_empty()).then(|| d.as_str().into()),',
     '        Some(Value::String(d)) => Some(d.as_str().into()),', A),
    ("a body that leaves out the description clears it", R,
     '        None => row.description.clone(),', '        None => None,', A),
    ("the update answers 201, as a create does", R,
     '        Some((id, l)) => Reply::json(200, label_json(after, id, &l)),\n        None => Reply::not_found(),\n    }\n}\n\n/// A refused label update',
     '        Some((id, l)) => Reply::json(201, label_json(after, id, &l)),\n        None => Reply::not_found(),\n    }\n}\n\n/// A refused label update', A),
    ("a reader can be assigned to the repository's issues", R,
     '    if perm >= Some(graftyard_identity::RepoPermission::Write) { Reply::empty(204) } else { Reply::not_found() }',
     '    if perm.is_some() { Reply::empty(204) } else { Reply::not_found() }', A),
    ("a refused delete names the releases page, as on main", H,
     '            Some(doc) => crate::release::refused(e).with_doc(doc),',
     '            Some(_) => crate::release::refused(e),', A),
    ("a 404's documentation moves too", W,
     '            && *u != root\n', '', A),
    ("a label update keeps 32.5, which a member without `LabelUpdated` renders", C,
     '        if self.rest_label {\n            return REPO_FORMAT_LABEL_UPDATE;',
     '        if false {\n            return REPO_FORMAT_LABEL_UPDATE;', G),
    ("32.36 marks a label create too", C,
     '        matches!(op, IssueOp::DefineLabel { id: Some(_), .. })',
     '        matches!(op, IssueOp::DefineLabel { .. })', G),
]

if __name__ == '__main__':
    run(MUTANTS)
