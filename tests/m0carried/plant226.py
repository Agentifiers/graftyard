#!/usr/bin/env python3
"""AM3-WP1, the agents account's `graft-ops` (`crates/release/src/ops.rs`, `Account`): the install's `graft-ops` running
a `ControllerRoll`, the agents one running every intent, the agents one draining sites, the agents one uploading the
site scripts of a bundle, and `release roll` never signing a `ControllerRoll`. Run from the repository root:

    python3 tests/m0carried/plant226.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

O = 'crates/release/src/ops.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-release', 'controller']
MUTANTS = [
    ("the install runs a ControllerRoll", O, 'Account::Install => !matches!(i, Intent::ControllerRoll { .. }),',
     'Account::Install => true,', T),
    ("the agents graft-ops runs every intent", O, 'Account::Agents => crate::group::agents_intent(i),',
     'Account::Agents => true,', T),
    ("the agents graft-ops drains sites", O, '    if w.account == Account::Install {\n        if let Some(a) = auto_drain',
     '    if true {\n        if let Some(a) = auto_drain', T),
    ("the agents graft-ops uploads site scripts", O,
     '        Account::Agents => Vec::new(),\n', '        Account::Agents => genesis.sites.iter().map(|s| Script::Site(s.id)).collect(),\n', T),
    ("release roll skips ControllerRoll", O, '        Intent::ControllerRoll { release: r },\n    ]);', '    ]);', T),
]

if __name__ == '__main__':
    run(MUTANTS)
