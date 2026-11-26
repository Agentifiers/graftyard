#!/usr/bin/env python3
"""`idle-stall`'s cron half (`decisions/DECISIONS.md`): the leader refolds a chain again only when it wrote its fold
since the last report (`workers/site/src/load.rs`, `refold_due`), and a voter's unloaded tier keeps no chain from
matching (`workers/site/src/fold_audit.rs`, `unloaded` and `idle`). Run from the repository root:

    python3 tests/m0carried/plant271.py          # every mutant
    python3 tests/m0carried/plant271.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

L = 'workers/site/src/load.rs'
F = 'workers/site/src/fold_audit.rs'
DUE = 'now >= at + AUDIT_EVERY_MS && v["index"].as_u64().is_none_or(|i| moved_since(Index(i)))'
UNL = '    m.get("tier").is_some() && m["loaded"] == json!(false)\n'
LT = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'moved_tests::an_unmoved_chain_is_refolded_once']
FT = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib',
      'idle_tests::an_unloaded_tier_does_not_keep_a_chain_from_matching']
MUTANTS = [
    ("a day-old report is due whatever moved, as before", L, DUE, 'now >= at + AUDIT_EVERY_MS', LT),
    ("a failed refold is never due again", L, DUE,
     'now >= at + AUDIT_EVERY_MS && v["index"].as_u64().is_some_and(|i| moved_since(Index(i)))', LT),
    ("the day's wait is dropped for a moved chain", L, DUE,
     'v["index"].as_u64().is_none_or(|i| moved_since(Index(i)))', LT),
    ("any missing tier counts as unloaded", F, UNL, '    m.get("tier").is_some()\n', FT),
    ("any missing entry counts", F, UNL, '    m["loaded"] != json!(true)\n', FT),
    ("the idle path skips a loaded tier with no digest too", F,
     '                None if v[name]["loaded"] == json!(false) => {}\n', '                None => {}\n', FT),
    ("the idle path still refolds on an unloaded tier", F,
     '                None if v[name]["loaded"] == json!(false) => {}\n', '', FT),
]

if __name__ == '__main__':
    run(MUTANTS)
