#!/usr/bin/env python3
"""OPEN-QUESTIONS F129 (BF-70 r2): an Actions ID token's `nbf` sits `ID_TOKEN_NBF_SKEW_S` (600 s, GitHub's offset) before
its `iat`, so a relying party whose clock lags the edge's takes a fresh token (`crates/actions/src/oidc.rs`, `claims`).
The mutants: `nbf` equals `iat` (main before F129), and a backdate of the token's lifetime instead of GitHub's offset.
Run from the repository root:

    python3 tests/m0carried/plant241.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

O = 'crates/actions/src/oidc.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'a_push_token_carries_ghes_claims_as_strings']
MUTANTS = [
    ("nbf equals iat", O, '"nbf": now_s.saturating_sub(ID_TOKEN_NBF_SKEW_S),', '"nbf": now_s,', T),
    ("the backdate is the token's lifetime", O, '"nbf": now_s.saturating_sub(ID_TOKEN_NBF_SKEW_S),',
     '"nbf": now_s.saturating_sub(ID_TOKEN_TTL_S),', T),
]

if __name__ == '__main__':
    run(MUTANTS)
