#!/usr/bin/env python3
"""AM3-WP1, A55's equivocation check (DECISIONS "`agents-equivocation`", `crates/release/src/agents.rs`, `extend`). The
mutants: a view that skips other bytes at a held index as before (LF-8 r1's silent ok refresh), a view that refuses an
equal replay, a view that records no digest for an entry it applied, an equivocation that does not page, and a front
that does not count a paging refusal. Run from the repository root:

    python3 tests/m0carried/plant228.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

A = 'crates/release/src/agents.rs'
L = 'crates/release/src/lib.rs'
W = 'workers/agents-front/src/lib.rs'
F = ['cargo', 'test', '-q', '-p', 'graftyard-agents-front']
R = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', 'agents']
MUTANTS = [
    ("other bytes at a held index are skipped", A,
     '                return Err(Refusal::Equivocation { index: index.0 });\n',
     '                continue;\n', R),
    ("an equal replay is refused", A,
     '                if i.and_then(|i| self.applied.get(i)) == Some(&Accepted::digest_of(bytes)) {\n',
     '                if i.and_then(|i| self.applied.get(i)) == Some(&Accepted::digest_of(bytes)) && false {\n', R),
    ("an applied entry records no digest", A,
     '            self.applied.push(ev.digest());\n', '', R),
    ("an equivocation does not page", L,
     '    matches!(variant, "UnreadableFormat" | "DrainGuard" | "Equivocation")\n',
     '    matches!(variant, "UnreadableFormat" | "DrainGuard")\n', R),
    ("the front does not count a paging refusal", W,
     '                *paged += u64::from(e.pages());\n', '', F),
]

if __name__ == '__main__':
    run(MUTANTS)
