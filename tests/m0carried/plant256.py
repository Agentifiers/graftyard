#!/usr/bin/env python3
"""BF-72 `reusable`, the expansion and the contexts (`crates/actions/src/reusable.rs`; `decisions/DECISIONS.md`,
"`reusable`"). The mutants: a called job that does not wait for its caller's `needs`, a dependent of the call that waits
for none of its jobs, a branch picked over a tag of the same name, inputs that keep their YAML strings untyped, a
declared default dropped, `inherit` that passes nothing, an explicit mapping that passes every secret, a nested call
handed its grandparent's secrets, the call's output read from its last job instead of `on.workflow_call.outputs`, a
matrix call whose first leg's output wins or whose legs are read in matrix order, a number input left a string, a fifth
level, a 21st file, a loop, and a nested job wider than its caller.
Run from the repository root:

    python3 tests/m0carried/plant256.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

R = 'crates/actions/src/reusable.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'reusable']
MUTANTS = [
    ("a called job does not wait for its caller's needs", R,
     '            if needs.is_empty() {\n                needs = fr.needs.to_vec();\n            }\n', '', T),
    ("a dependent of the call waits for none of its jobs", R, '                at[i].extend(added?);', '                added?;', T),
    ("a branch wins over a tag of the same name", R, '["refs/tags/", "refs/heads/"].iter()', '["refs/heads/", "refs/tags/"].iter()', T),
    ("inputs are not typed", R, '            out.insert(name, typed(eval(&v, &ctx), ty));', '            out.insert(name, eval(&v, &ctx));', T),
    ("a declared default is dropped", R,
     '            if let Some(v) = d.get("default") {\n                out.insert(k.clone(), v.clone());\n            }\n', '', T),
    ("inherit passes nothing", R, '        if c.inherit {\n            return parent;\n        }', '        if c.inherit {\n            return Vec::new();\n        }', T),
    ("an explicit mapping passes every secret", R, '        let map = c.secrets.as_deref()', '        if c.secrets.is_some() {\n            return parent;\n        }\n        let map = c.secrets.as_deref()', T),
    ("a nested call is handed its grandparent's secrets", R,
     'let parent = c.parent.map_or_else(|| self.top.to_vec(), |p| self.secrets(p));', 'let parent = self.top.to_vec();', T),
    ("the call's output is its last job's", R, '                for (_, f) in frames {\n                    merge(&mut outputs, self.call_outputs(f));\n                }',
     '                for p in &ps {\n                    let got = self.ids.get(*p).and_then(|id| self.f.job_outputs(*id)).into_iter().flatten();\n                    merge(&mut outputs, got.map(|(k, v)| (k.to_string(), Value::from(&**v))).collect());\n                }\n                let _ = frames;', T),
    ("a matrix call's first set output wins", R, '        if v.as_str() != Some("") && !v.is_null() {\n            into.insert(k, v);',
     '        if v.as_str() != Some("") && !v.is_null() && !into.contains_key(&k) {\n            into.insert(k, v);', T),
    ("a matrix call's legs are read in matrix order, not completion order", R, '                frames.sort();\n', '', T),
    ("a number input given a string stays a string", 'crates/actions/src/reusable.rs',
     '            o.insert("type".into(), Value::from("number"));', '', T),
    ("a fifth level is allowed", R, '            if depth >= LEVELS_MAX {\n                return Err', '            if depth > LEVELS_MAX {\n                return Err', T),
    ("a 21st file is allowed", R, 'if called.files.len() == UNIQUE_MAX {', 'if called.files.len() > UNIQUE_MAX {', T),
    ("a loop is not refused", R, '            if ancestry.contains(&co.key()) {', '            if ancestry.contains(&co.key()) && depth > LEVELS_MAX {', T),
    ("a nested job may be wider than its caller", R, '                && !d.within(cap)', '                && d.within(cap) && false', T),
]

if __name__ == '__main__':
    run(MUTANTS)
