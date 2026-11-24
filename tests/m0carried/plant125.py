#!/usr/bin/env python3
"""R47's 1% gap (`crates/release/src/ops.rs`, `steps` and `next`; `crates/release/src/gate.rs`, `judge_roll`): a front
or MCP roll's 100% step back to no dwell, its N+1 judged against N's empty step window instead of N's counts from the
intent's start, and a starved 100% step committing instead of dwelling. Run from the repository root:

    python3 tests/m0carried/plant125.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import REL, run  # noqa: E402

OPS = 'crates/release/src/ops.rs'
GATE = 'crates/release/src/gate.rs'
MUTANTS = [
    ("a front or MCP roll's 100% step takes no dwell", OPS,
     'if *p < 100 { dwell.first_day } else { dwell.step }', 'if *p < 100 { dwell.first_day } else { 0 }', REL),
    ("N is judged on its empty step window, not from the intent's start", GATE,
     '        Some(n) if window.of(n).answers < STEP_MIN_ANSWERS => whole.of(n),\n', '', REL),
    ("a starved 100% step commits instead of dwelling", OPS,
     '                    && *pct != 1\n', '                    && *pct != 1\n                    && *pct != 100\n', REL),
]

if __name__ == '__main__':
    run(MUTANTS)
