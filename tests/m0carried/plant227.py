#!/usr/bin/env python3
"""AM3-WP1, A53's default pin (DECISIONS "`controller-default`", `crates/release/src/agents.rs`): a cold
`graft-agents-front` isolate that ignores `CONTROLLER_DEFAULT` and follows the split to N+1 (LF-7 r1's failure), a
verified view of the roll that still pins N so the canary never runs, a default kept at 100%, a default that is the
split's smaller share, and a settings change that drops the old value only. Run from the repository root:

    python3 tests/m0carried/plant227.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

A = 'crates/release/src/agents.rs'
F = ['cargo', 'test', '-q', '-p', 'graftyard-agents-front']
R = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', 'agents']
MUTANTS = [
    ("a cold isolate ignores the default", A, '    if canary { None } else { default }\n',
     '    let _ = canary;\n    let _ = default;\n    None\n', F),
    ("a verified view of the roll pins the default", A, '    if canary { None } else { default }\n',
     '    let _ = canary;\n    default\n', F),
    ("the default stays at 100%", A, '    if split.iter().any(|(_, p)| *p == 100) {\n',
     '    if split.is_empty() {\n', R),
    ("the default is the smaller share", A, '    split.iter().max_by_key(|(_, p)| *p).map(|(v, _)| *v)\n',
     '    split.iter().min_by_key(|(_, p)| *p).map(|(v, _)| *v)\n', R),
    ("a settings change keeps the old default too", A, '        .filter(|b| b["name"] != CONTROLLER_DEFAULT)\n',
     '        .filter(|_| true)\n', R),
]

if __name__ == '__main__':
    run(MUTANTS)
