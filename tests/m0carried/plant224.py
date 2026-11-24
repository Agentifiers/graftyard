#!/usr/bin/env python3
"""AM3-WP1, `graft-agents-front`'s reader (`workers/agents-front/src/lib.rs`): a failed refresh that is not counted as
failed (so the next request waits on a failing install), a refused page counted as an ok refresh, the ok time taken at
the refresh's end instead of its start, and a refresh that reads past a short page or re-reads a page behind it. Run from
the repository root:

    python3 tests/m0carried/plant224.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

L = 'workers/agents-front/src/lib.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-agents-front']
MUTANTS = [
    ("a failed refresh is not marked failed", L, '                self.fresh.failed(now);\n',
     '                self.fresh.ok(start);\n', T),
    ("a refused page counts as ok", L,
     '.map_err(|e| format!("{}: {e}", e.variant()))',
     '.or_else(|_| Ok::<(), String>(()))', T),
    ("ok is stamped at the end", L, '                self.fresh.ok(start);\n',
     '                self.fresh.ok(now);\n', T),
    ("the next page is read behind the view", L, '(page.len() >= ENTRIES_PER_PAGE && last > after)',
     '(page.len() >= ENTRIES_PER_PAGE)', T),
]

if __name__ == '__main__':
    run(MUTANTS)
