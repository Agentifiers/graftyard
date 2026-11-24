#!/usr/bin/env python3
"""AM3-WP1, `graft-controller`'s webhook intake (`workers/controller/src/lib.rs`): a signature compare that accepts a
prefix, a repeated delivery id that is not dropped, a `Seen` that never evicts, and a refused delivery remembered so it
hides a later valid one. Run from the repository root:

    python3 tests/m0carried/plant225.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

L = 'workers/controller/src/lib.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-controller']
MUTANTS = [
    ("the MAC compare accepts a prefix", L, '    got.len() == want.len() && got.bytes()', '    got.bytes()', T),
    ("a repeated delivery is accepted", L, '        if self.ids.contains(id) {\n            return false;',
     '        if self.ids.contains(id) {\n            return true;', T),
    ("Seen never evicts", L, '        if self.order.len() == SEEN_MAX\n', '        if self.order.len() == usize::MAX\n', T),
    ("a refused delivery is remembered", L,
     '    if !verify(secret, body, headers("x-hub-signature-256").as_deref()) {\n',
     '    let _ = headers("x-github-delivery").map(|d| seen.insert(&d));\n'
     '    if !verify(secret, body, headers("x-hub-signature-256").as_deref()) {\n', T),
]

if __name__ == '__main__':
    run(MUTANTS)
