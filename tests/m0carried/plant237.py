#!/usr/bin/env python3
"""OPEN-QUESTIONS A56 (LM-3 r1): the agents view routes nothing from a prefix of the history
(`crates/release/src/agents.rs`, `AgentsReleaseView::caught_up` and `controller_pin`). The mutants: the pin ignores
`caught_up`, so a cold isolate's partial read pins an old pointer or follows the split; a full page counts as the
last, so a prefix is caught up; and a page that takes nothing marks the view a prefix, so a member behind costs a
caught-up view its pin. Run from the repository root:

    python3 tests/m0carried/plant237.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

A = 'crates/release/src/agents.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-agents-front', '--test', 'front']
MUTANTS = [
    ("the pin ignores whether the view is caught up", A,
     '    if !view.caught_up() {\n        return default;\n    }\n', '', T),
    ("a full page counts as the edge's last", A,
     'if took.is_ok() && page.len() < ENTRIES_PER_PAGE {', 'if took.is_ok() && page.len() <= ENTRIES_PER_PAGE {', T),
    ("a page that takes nothing marks the view a prefix", A,
     '        } else if self.fold.count > held {\n', '        } else {\n', T),
]

if __name__ == '__main__':
    run(MUTANTS)
