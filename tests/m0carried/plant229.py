#!/usr/bin/env python3
"""M4-WP3 actions-cache, the edge's download answer (`workers/edge/src/cache.rs`, `span`;
`decisions/DECISIONS.md`, "`actions-cache`"): a `Range` answered 200 instead of 206, an unsatisfiable range answered
with the whole entry instead of 416, and a closed range answered from its start to the entry's end. Run from the
repository root:

    python3 tests/m0carried/plant229.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

E = 'workers/edge/src/cache.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'cache']
MUTANTS = [
    ("a range is answered 200", E, 'Ok(Some((s, e))) => Some((206, s, e)),', 'Ok(Some((s, e))) => Some((200, s, e)),', T),
    ("an unsatisfiable range takes the whole entry", E,
     '        Err(_) => None,\n', '        Err(_) => Some((200, 0, size.saturating_sub(1))),\n', T),
    ("a closed range runs to the entry's end", E,
     'Ok(Some((s, e))) => Some((206, s, e)),', 'Ok(Some((s, _))) => Some((206, s, size - 1)),', T),
]

if __name__ == '__main__':
    run(MUTANTS)
