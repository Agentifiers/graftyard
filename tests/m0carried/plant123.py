#!/usr/bin/env python3
"""R47, N's rate ignored (`crates/release/src/gate.rs`, `judge_errors` and `judge_roll`): N+1 judged against 0.1%
alone, N always read from the step's window (so a pinned script's 0% N at 100% is no baseline), and N read as
another version's answers. Run from the repository root:

    python3 tests/m0carried/plant123.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import REL, run  # noqa: E402

GATE = 'crates/release/src/gate.rs'
MUTANTS = [
    ("N's own 5xx rate is not the baseline", GATE,
     'let base = (n.fivexx as f64 / n.answers.max(1) as f64 +', 'let base = (0.0 * n.fivexx as f64 +', REL),
    ("N is read from the step's window even when it served nothing there", GATE,
     '        Some(n) if window.of(n).answers < STEP_MIN_ANSWERS => whole.of(n),\n', '', REL),
    ("N+1's answers are read as N's", GATE,
     '        Some(n) => window.of(n),\n        None => Answers::default(),\n    };\n    judge_errors(window.of(n1),',
     '        Some(_) => window.of(n1),\n        None => Answers::default(),\n    };\n    judge_errors(window.of(n1),',
     REL),
]

if __name__ == '__main__':
    run(MUTANTS)
