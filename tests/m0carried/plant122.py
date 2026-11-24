#!/usr/bin/env python3
"""R47, the floor dropped (`crates/release/src/gate.rs`, `judge_errors`; `crates/release/src/ops.rs`, `next`): a
quiet N+1 passed instead of starved, the floor raised to every step so a healthy 1% step never commits, and a starved
1% step that dwells twice and pages as later steps do (as before R38). Run from the repository root:

    python3 tests/m0carried/plant122.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import REL, run  # noqa: E402

GATE = 'crates/release/src/gate.rs'
OPS = 'crates/release/src/ops.rs'
MUTANTS = [
    ("under STEP_MIN_ANSWERS a clean N+1 passes", GATE,
     '    if a < STEP_MIN_ANSWERS {\n        return Gate::Starved(a);\n', '    if a < 1 {\n        return Gate::Starved(a);\n',
     REL),
    ("a starved 1% step dwells twice and pages", OPS, '                    && *pct != 1\n', '', REL),
]

if __name__ == '__main__':
    run(MUTANTS)
