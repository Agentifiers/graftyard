#!/usr/bin/env python3
"""F132 (`decisions/OPEN-QUESTIONS.md`; `decisions/DECISIONS.md`, "`reusable`"): a push or pull request run's call
into another repository, read as the edge's dispatch reads it (`crates/actions/src/reusable.rs`, `called_file` and
`pin_other`, which `ActionsTrigger`'s and the dispatch's `CallReads` both call). The first mutant is main's trigger:
it refused every call into another repository. Run from the repository root:

    python3 tests/m0carried/plant268.py          # every mutant
    python3 tests/m0carried/plant268.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

R = 'crates/actions/src/reusable.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'reusable',
     'a_called_file_in_another_repository_is_pinned_as_the_dispatch_pins_it']
# Dropped: "a full SHA is looked up as a ref name". Another repository's call is always `Version::Named`, and
# `resolve_version` reads a hex name as a SHA first, so the mutant is equivalent.
MUTANTS = [
    ("a call into another repository is refused, as main's trigger did", R,
     '            let (o, n) = name.split_once(\'/\').ok_or("owner/repo")?;\n',
     '            return Err("a call into another repository is not taken from a push or pull request yet".into());\n'
     '            #[allow(unreachable_code)]\n            let (o, n) = name.split_once(\'/\').ok_or("owner/repo")?;\n', T),
    ("a private repository is read", R,
     '    if graftyard_identity::authorize_read(None, None, &[], &access) != graftyard_identity::Decision::Allow {',
     '    if false && graftyard_identity::authorize_read(None, None, &[], &access) != graftyard_identity::Decision::Allow {',
     T),
    ("the called file is read in the run's repository", R, '            (Some(&other), pinned)', '            (None, pinned)', T),
    ("an unknown version pins the default branch", R,
     '    let (r, sha) = resolve_version(v, at).ok_or_else(|| format!("{name}@{v}: no such version"))?;',
     '    let (r, sha) = resolve_version(v, at).or_else(|| resolve_version("main", at)).ok_or_else(|| format!("{name}@{v}: no such version"))?;',
     T),
]

if __name__ == '__main__':
    run(MUTANTS)
