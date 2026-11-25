#!/usr/bin/env python3
"""OPEN-QUESTIONS F121: a `pull_request` run's ID token carries its base branch as `base_ref`, the short name GitHub's
tokens carry (`crates/actions/src/oidc.rs`, `claims`; `crates/actions/src/shard.rs`, `run_base`). The mutants: the
claim stays empty (main before F121), the full ref is sent, and the fold reads no base for the run. Run from the
repository root:

    python3 tests/m0carried/plant243.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

O = 'crates/actions/src/oidc.rs'
S = 'crates/actions/src/shard.rs'
CLAIM = '"base_ref": f.base_ref.map_or("", |b| b.strip_prefix("refs/heads/").unwrap_or(b)),'
TO = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'a_pull_request_and_an_environment_job_name_their_context']
TS = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'a_pull_request_run_records_its_base']
MUTANTS = [
    ("base_ref stays empty", O, CLAIM, '"base_ref": "",', TO),
    ("base_ref is the full ref", O, CLAIM, '"base_ref": f.base_ref.unwrap_or(""),', TO),
    ("the fold reads no base for the run", S, 'self.more.more.bases.get(&r).map(|b| &**b)', 'None', TS),
]

if __name__ == '__main__':
    run(MUTANTS)
