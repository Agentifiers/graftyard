#!/usr/bin/env python3
"""`restore-gate` (`decisions/DECISIONS.md`): the `Release` group refuses a `RestorePlacement` while its site is lost and
not yet replaced, and a `Resume` of a halted `RestorePlacement` while its site is drained (`crates/release/src/group.rs`,
`check_intent` and the `Resume` check). m6aL2's restore committed 42 s after its `SiteLost`, before graft-ops drained
the site. Run from the repository root:

    python3 tests/m0carried/plant265.py          # every mutant
    python3 tests/m0carried/plant265.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

G = 'crates/release/src/group.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib',
     'tests::group::replace::']
MUTANTS = [
    ("a restore of a lost site is accepted before its drain, as on main", G,
     'if self.lost.contains(&site.0) && !self.replaced.contains_key(&site.0) {',
     'if false && self.lost.contains(&site.0) && !self.replaced.contains_key(&site.0) {', T),
    ("a lost site that was replaced is still refused", G,
     'if self.lost.contains(&site.0) && !self.replaced.contains_key(&site.0) {',
     'if self.lost.contains(&site.0) {', T),
    ("a halted restore resumes while its site is drained", G,
     '                    && self.drained.contains_key(&site.0)\n                {\n                    return Err(refuse("a RestorePlacement resumes',
     '                    && !self.drained.contains_key(&site.0)\n                {\n                    return Err(refuse("a RestorePlacement resumes', T),
]

if __name__ == '__main__':
    run(MUTANTS)
