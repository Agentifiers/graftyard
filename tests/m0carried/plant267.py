#!/usr/bin/env python3
"""`cold-view-rows` (`decisions/DECISIONS.md`): a cold `Release` view asks every site's registry row's member, the
newest generation first, before any one record's bootstrap names (`crates/lookup/src/view.rs`, `cold_targets`).
m6aL2's graft-ops read only site 0's stale generation-4 row and never reached the live voters while site 0 was down.
Run from the repository root:

    python3 tests/m0carried/plant267.py          # every mutant
    python3 tests/m0carried/plant267.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

V = 'crates/lookup/src/view.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib', 'a_cold_view_asks_every_sites_row']
MUTANTS = [
    ("the rows' members are not asked, only one record's names, as on main", V,
     '    for (site, r) in &rows {\n        add(Target { site: *site, do_name: r.do_name.to_string() });\n    }\n', '', T),
    ("the oldest generation goes first", V,
     '    rows.sort_by_key(|(_, r)| std::cmp::Reverse(r.generation));\n',
     '    rows.sort_by_key(|(_, r)| r.generation);\n', T),
    ("the fallback names come from the oldest row", V,
     'record.cloned().or_else(|| rows.first().and_then(|(_, r)| r.record()))',
     'record.cloned().or_else(|| rows.last().and_then(|(_, r)| r.record()))', T),
]

if __name__ == '__main__':
    run(MUTANTS)
