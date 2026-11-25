#!/usr/bin/env python3
"""OPEN-QUESTIONS F127 (BF-70 r1): a job's declared `permissions:` reach the shard's `JobTokens` row on every create
path, so `id-token: write` names `GenerateIdTokenUrl` (`crates/actions/src/runsvc.rs`, `create_run_call` and
`create_run_op`; `workers/edge/src/actions.rs`, `dispatch_call`). The mutants: the dispatch drops the permissions (main
before F127), the call drops them with job tokens on, and the shard leader's decode drops them from a `pull_request`
run's `CreateRunOn`. Run from the repository root:

    python3 tests/m0carried/plant242.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

R = 'crates/actions/src/runsvc.rs'
E = 'workers/edge/src/actions.rs'
TE = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'a_dispatched_job_granted_id_token_write']
TP = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'pr_trigger', 'a_pull_request_runs_granted_job']
MUTANTS = [
    ("the dispatch sends no permissions", E,
     'Ok((runsvc::create_run_call(&run, &jobs, &permissions, tokens, None, None, None), jobs.len()))',
     'Ok((runsvc::create_run_call(&run, &jobs, &Default::default(), tokens, None, None, None), jobs.len()))', TE),
    ("the call drops the permissions with job tokens on", R,
     '    if tokens && !permissions.is_empty() {\n        b["permissions"] = json!(permissions);\n    }\n', '', TP),
    ("the decode drops a pull_request run's permissions", R,
     'ActionsOp::CreateRunOn { run, jobs, permissions, base_ref: base.into() }',
     'ActionsOp::CreateRunOn { run, jobs, permissions: Vec::new(), base_ref: base.into() }', TP),
]

if __name__ == '__main__':
    run(MUTANTS)
