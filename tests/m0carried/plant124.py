#!/usr/bin/env python3
"""R47, a breach under the floor not undoing (`crates/release/src/gate.rs`, `judge_errors`, `judge_roll` and
`roll_versions`): a breach judged only from STEP_MIN_ANSWERS, so 5 of 6 answers at 1% wait out the dwell; thrown
binding calls left out of N+1's 5xx; answers with no version header breaching as N+1's; and an `McpRoll` left
pending as before R47. Run from the repository root:

    python3 tests/m0carried/plant124.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import REL, run  # noqa: E402

GATE = 'crates/release/src/gate.rs'
MUTANTS = [
    ("a breach is judged only once N+1 has STEP_MIN_ANSWERS", GATE,
     '    if past_chance(a, e, base) {', '    if a >= STEP_MIN_ANSWERS && past_chance(a, e, base) {', REL),
    ("a thrown binding call is not one of N+1's 5xx", GATE,
     '    judge_errors(window.of(n1), window.threw, n_side)', '    judge_errors(window.of(n1), 0, n_side)', REL),
    ("an answer with no version header counts against N+1", GATE,
     '    judge_errors(window.of(n1), window.threw, n_side)',
     '    judge_errors(window.of(n1), window.threw + window.unversioned.fivexx, n_side)', REL),
    ("an McpRoll's gate stays pending", GATE,
     '    let (Intent::EdgeRoll { .. } | Intent::FrontRoll { .. } | Intent::McpRoll { .. } | Intent::ControllerRoll { .. }) =',
     '    let (Intent::EdgeRoll { .. } | Intent::FrontRoll { .. } | Intent::ControllerRoll { .. }) =', REL),
]

if __name__ == '__main__':
    run(MUTANTS)
