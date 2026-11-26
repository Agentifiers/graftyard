#!/usr/bin/env python3
"""F133 (`decisions/OPEN-QUESTIONS.md`; `decisions/DECISIONS.md`, "`push-trigger`"): the base a `push` run's `paths`
filters and `github.event.commits` read from, and a force push's two-dot diff (`crates/actions/src/trigger.rs`,
`push_base`, `diff_paths`, `two_dot`; the site's `push_paths` and `push_runs` in `workers/site/src/actions_trigger.rs`
call them). Run from the repository root:

    python3 tests/m0carried/plant264.py          # every mutant
    python3 tests/m0carried/plant264.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

T = 'crates/actions/src/trigger.rs'
PT = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'pr_trigger',
      'a_push_reads_paths_from_its_base_and_a_force_push_two_dot']
MUTANTS = [
    ("a new branch has no base", T, 'default_tip.filter(|_| !p.tag() && p.r#ref != *default_branch)', 'None', PT),
    ("a new default branch reads from its own tip", T, 'default_tip.filter(|_| !p.tag() && p.r#ref != *default_branch)',
     'default_tip.filter(|_| !p.tag())', PT),
    ("an existing ref reads from the default branch", T, '        return Some(p.before);\n',
     '        return default_tip;\n', PT),
    ("a force push reads the three-dot diff", T,
     '    out.extend(to_before.keys().filter(|k| !to_after.contains_key(*k)).cloned());\n', '', PT),
    ("a path both sides changed counts by name only", T, '.filter(|(k, v)| to_before.get(*k) != Some(v))',
     '.filter(|(k, _)| !to_before.contains_key(*k))', PT),
    ("a removed file keeps its old blob", T,
     'f["sha"].as_str().filter(|_| status != Some("removed")).map(str::to_owned)',
     'f["sha"].as_str().map(str::to_owned)', PT),
    ("a rename drops its old name", T, '            out.entry(prev.to_owned()).or_insert(None);\n', '', PT),
    ("a truncated diff filters", T, '    if d["truncated"].as_bool() == Some(true) {\n        return None;\n    }\n    let mut out = BTreeMap::new();',
     '    let mut out = BTreeMap::new();', PT),
]

if __name__ == '__main__':
    run(MUTANTS)
