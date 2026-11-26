#!/usr/bin/env python3
"""`restore-gate-from` (`decisions/DECISIONS.md`): the `Release` group's refusal of a `RestorePlacement` while its site is
lost and not yet replaced holds only for entries stamped at or after `RESTORE_GATE_FROM` (`crates/release/src/group.rs`,
`check_intent`), so m6aL2's restore, committed between its `SiteLost` and the drain, still replays in every replica.
Run from the repository root:

    python3 tests/m0carried/plant272.py          # every mutant
    python3 tests/m0carried/plant272.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

G = 'crates/release/src/group.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', 'tests::group::replace::']
ARM = 'if at >= RESTORE_GATE_FROM && self.lost.contains(&site.0) && !self.replaced.contains_key(&site.0) {'
MUTANTS = [
    ("the refusal acts on committed history, as 265 first shipped", G, ARM,
     'if self.lost.contains(&site.0) && !self.replaced.contains_key(&site.0) {', T),
    ("an entry stamped exactly at the activation point is not yet refused", G, ARM,
     'if at > RESTORE_GATE_FROM && self.lost.contains(&site.0) && !self.replaced.contains_key(&site.0) {', T),
    ("the activation point also lifts the drained refusal before it", G,
     '                if self.drained.contains_key(&site.0) {\n                    return Err(refuse("RestorePlacement is refused while its site is drained"));',
     '                if at >= RESTORE_GATE_FROM && self.drained.contains_key(&site.0) {\n                    return Err(refuse("RestorePlacement is refused while its site is drained"));', T),
]

if __name__ == '__main__':
    run(MUTANTS)
