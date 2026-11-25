#!/usr/bin/env python3
"""OPEN-QUESTIONS F121: a same-repository `pull_request` run's ID token carries its head branch as `head_ref`, the
short name GitHub's tokens carry, through format 0x15.7's `CreateRunFrom` and `RunHead` (`crates/actions/src/oidc.rs`,
`claims`; `crates/actions/src/shard.rs`; `crates/actions/src/runsvc.rs`, `create_run_op`;
`crates/actions/src/trigger.rs`, `pr_head`). The mutants: the claim stays empty, the full ref is sent, the fold reads
no head, `CreateRunFrom` records no `RunHead`, `CreateRunFrom` is sent as 0x15.6 (so a member without its reader is
sent it), the leader drops the head and commits `CreateRunOn`, the trigger sends a head with the flag off, and a fork's
branch rides. Run from the repository root:

    python3 tests/m0carried/plant247.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

O = 'crates/actions/src/oidc.rs'
S = 'crates/actions/src/shard.rs'
R = 'crates/actions/src/runsvc.rs'
T = 'crates/actions/src/trigger.rs'
CLAIM = '"head_ref": f.head_ref.map_or("", |h| h.strip_prefix("refs/heads/").unwrap_or(h)),'
TO = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'a_pull_request_and_an_environment_job_name_their_context']
TS = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'a_same_repository_pull_request_run_records_its_head']
TT = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'pr_trigger', 'head_ref_rides_only_with_the_flag']
MUTANTS = [
    ("head_ref stays empty", O, CLAIM, '"head_ref": "",', TO),
    ("head_ref is the full ref", O, CLAIM, '"head_ref": f.head_ref.unwrap_or(""),', TO),
    ("the fold reads no head for the run", S, 'self.more.more.more.heads.get(&r).map(|h| &**h)', 'None', TS),
    ("CreateRunFrom records no RunHead", S,
     '                    evs.push(ActionsEvent::RunHead { run: id, head_ref: head_ref.clone() });\n', '', TS),
    ("CreateRunFrom is sent as 0x15.6", S,
     'ActionsOp::CreateRunFrom { .. } => ACTIONS_FORMAT_HEAD_REF,', 'ActionsOp::CreateRunFrom { .. } => ACTIONS_FORMAT_CACHE,',
     TS),
    ("the leader drops the head", R, '(Some(base), Some(head)) if run.is_pull_request() =>',
     '(Some(base), Some(head)) if run.is_pull_request() && head.is_empty() =>', TT),
    ("the trigger sends a head with the flag off", T, 'graftyard_git::tiers::pull_head(core, n).filter(|_| on)?',
     'graftyard_git::tiers::pull_head(core, n)?', TT),
    ("a fork's branch rides", T, '(ph.head.repo == repo).then_some(ph.head.branch)', 'Some(ph.head.branch)', TT),
]

if __name__ == '__main__':
    run(MUTANTS)
