#!/usr/bin/env python3
"""F150, `secrets-wait` per branch (`crates/actions/src/shard.rs`, `ReleaseSecretsOn`, `release_secrets`;
`crates/actions/src/runsvc.rs`, `release_op`; `crates/git/src/merge.rs`, `agent_gate_pulls`;
`decisions/DECISIONS.md`, "`secrets-wait`"): a per-branch release that releases every branch's runs at the commit,
one that ignores the gated pull requests, one that takes any pull request run, one that takes any push run, one an admin
may send, the leader decoding a per-branch call as 0x15.14's, the gated pull requests read without their gate, and
0x15.15 sent as 0x15.14. Run from the repository root:

    python3 tests/m0carried/plant292.py          # every mutant
    python3 tests/m0carried/plant292.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

SHARD = 'crates/actions/src/shard.rs'
RUNSVC = 'crates/actions/src/runsvc.rs'
MERGE = 'crates/git/src/merge.rs'
GATES = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'gates', 'a_per_branch_release']
LIB = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'every_command_format_is_read']
REPLAY = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'l004_replay', 'first_300']
MUTANTS = [
    ("a per-branch release releases every branch at the commit", SHARD,
     '                Ok((release_secrets(s, *repo, head, on), ActionsOut::Done))',
     '                let _ = on;\n                Ok((release_secrets(s, *repo, head, |_| true), ActionsOut::Done))', GATES),
    ("a per-branch release takes any pull request run", SHARD,
     '                            && pulls.iter().any(|n| *r.head_ref == *graftyard_git::tiers::merge_ref(*n).display()))',
     '                            && !pulls.is_empty())', GATES),
    ("a per-branch release takes any push run", SHARD,
     '                    (r.is_push() && r.head_ref == *branch)', '                    r.is_push()', GATES),
    ("a per-branch release takes no pull request run", SHARD,
     '                        || (r.is_pull_request()\n',
     '                        || (false\n', GATES),
    ("an admin sends a per-branch release", SHARD,
     '            ActionsOp::ReleaseSecretsOn { repo, head, branch, pulls } => {\n                only(operator)?;',
     '            ActionsOp::ReleaseSecretsOn { repo, head, branch, pulls } => {\n                only(admin)?;', GATES),
    ("the leader decodes a per-branch call as 0x15.14's", RUNSVC,
     '    let Some(branch) = b["branch"].as_str() else {', '    let Some(branch) = b["branch"].as_str().filter(|_| false) else {',
     REPLAY),
    ("the gated pull requests are read without their gate", MERGE,
     '        .filter(|n| crate::tiers::agent_meta(core, *n).is_some() && gate_holds(core, *n, head).is_ok())',
     '        .filter(|n| crate::tiers::agent_meta(core, *n).is_some())', REPLAY),
    ("0x15.15 is sent as 0x15.14", SHARD,
     '            ActionsOp::ReleaseSecretsOn { .. } => ACTIONS_FORMAT_SECRETS_BRANCH,',
     '            ActionsOp::ReleaseSecretsOn { .. } => ACTIONS_FORMAT_SECRETS_WAIT,', LIB),
]

if __name__ == '__main__':
    run(MUTANTS)
