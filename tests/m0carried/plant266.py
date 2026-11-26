#!/usr/bin/env python3
"""`warm-lapse` (`decisions/DECISIONS.md`; OPEN-QUESTIONS R67): a move's warm ask of the target's network shard is a
start, not a client's request (`workers/site/src/member.rs`, `warm_url`), so a shard no client reads stops
`PROMOTE_LAPSE` after the move rather than a day on. Run from the repository root:

    python3 tests/m0carried/plant266.py          # every mutant
    python3 tests/m0carried/plant266.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

M = 'workers/site/src/member.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib',
     'a_moves_warm_ask_starts_the_shard_but_keeps_it_no_longer_than_promote_lapse']
URL = '"https://shard/catchup?warm=start&repo={}&record={record}&wait={WARM_ASK_WAIT_S}&next=1"'
MUTANTS = [
    ("the warm ask is a client's request, as on main", M, URL,
     '"https://shard/catchup?repo={}&record={record}&wait={WARM_ASK_WAIT_S}&next=1"', T),
    ("the warm ask is a keep-warm call, which never starts a stopped shard", M, URL,
     '"https://shard/catchup?warm=1&repo={}&record={record}&wait={WARM_ASK_WAIT_S}&next=1"', T),
    ("the warm ask waits out a rehydrate instead of trying the next shard", M, URL,
     '"https://shard/catchup?warm=start&repo={}&record={record}&wait={WARM_ASK_WAIT_S}"', T),
]

if __name__ == '__main__':
    run(MUTANTS)
