#!/usr/bin/env python3
"""BM-6's container half of BM-1's site fault (`workers/site/src/shard.rs`, `GitShard::fetch`): planted bugs in which
a `GitShard` on the faulted site keeps serving, or fails when it should not. The check runs in the wasm build only, so
each mutant runs `tests/shardfaultbed/run.sh` on a fresh site build (about 6 minutes). Plants one mutant at a time and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant245.py          # every mutant
    python3 tests/m0carried/plant245.py 0        # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
import plant132  # noqa: E402

S = 'workers/site/src/shard.rs'
T = ['bash', 'tests/shardfaultbed/run.sh']
CHECK = "        if crate::faults::site_fault(&self.env, None, false).await.is_some() {\n"

MUTANTS = [
    ("the shard ignores the site fault, as on main", S, CHECK,
     "        if crate::faults::site_fault(&self.env, None, false).await.is_some() && false {\n", T),
    ("the shard fails on a group-only plan too (consensus true)", S, CHECK,
     "        if crate::faults::site_fault(&self.env, None, true).await.is_some() {\n", T),
    ("the shard fails on every plan that names any site", S, CHECK,
     "        if crate::faults::site_fault(&self.env, None, true).await.is_some() || true {\n", T),
]

if __name__ == '__main__':
    plant132.MUTANTS = MUTANTS
    plant132.main()
