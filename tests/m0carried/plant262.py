#!/usr/bin/env python3
"""AM3-WP3, `AgentPolicy.ci` (`crates/actions/src/job.rs`, `JobState::Held`; `crates/actions/src/shard.rs`,
`CreateRunHeld`, `ApproveRun`, `RunHeld`; `crates/actions/src/run_owner.rs`; `crates/actions/src/rest.rs`;
`crates/actions/src/trigger.rs`, `agent_run`; `decisions/DECISIONS.md`, "`agent-ci`"): the hold not applied, a held
job matched by a runner, approval ignoring needs, a runner approving, a second approval
accepted, a held run writing checks, a held run rendered queued, its jobs listed, `agent_run` ignoring the gate or a
confine, and `mark` leaving the run unforked. Run from the repository root:

    python3 tests/m0carried/plant262.py          # every mutant
    python3 tests/m0carried/plant262.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

JOB = 'crates/actions/src/job.rs'
SHARD = 'crates/actions/src/shard.rs'
OWNER = 'crates/actions/src/run_owner.rs'
REST = 'crates/actions/src/rest.rs'
TRIG = 'crates/actions/src/trigger.rs'
GATES = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'gates', 'an_agent_run']
LIB = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', '--', 'held', 'agent_run', 'render_ghes']
MUTANTS = [
    ("the hold is not applied", SHARD,
     '                if !matches!(row.state, JobState::Queued { .. } | JobState::Pending { .. }) {',
     '                if true {', GATES),
    ("a runner takes a held job", JOB,
     '        (S::Queued { .. }, E::Matched(r)) => Some(S::Assigned { runner: *r, lease_until: lease }),',
     '        (S::Queued { .. } | S::Held { .. }, E::Matched(r)) => Some(S::Assigned { runner: *r, lease_until: lease }),',
     LIB),
    ("approval queues a job with needs", JOB,
     '        (S::Held { needs, .. }, E::Approved) if needs.is_empty() => {',
     '        (S::Held { .. }, E::Approved) => {', LIB),
    ("a runner approves", SHARD,
     '            ActionsOp::ApproveRun { run } => {\n                only(admin)?;',
     '            ActionsOp::ApproveRun { run } => {\n                only(true)?;', GATES),
    ("a second approval is accepted", SHARD,
     '                if evs.is_empty() {\n                    return Err(ActionsErr::Illegal("the run is not waiting for approval".into()));',
     '                if false {\n                    return Err(ActionsErr::Illegal("the run is not waiting for approval".into()));',
     GATES),
    ("a held run writes checks", OWNER,
     '        if fold.run_jobs(*r).iter().any(|j| matches!(j.state, JobState::Held { .. })) {',
     '        if false {', GATES),
    ("a held run renders queued", REST,
     '        RunStatus::Held => ("completed", Some("action_required")),',
     '        RunStatus::Held => ("queued", None),', LIB),
    ("a held run lists its jobs", REST,
     '    jobs.iter().filter(|j| !matches!(j.state, JobState::Held { .. }))',
     '    jobs.iter()', LIB),
    ("agent_run ignores the gate", TRIG, '    if gated || !under {', '    if !under {', LIB),
    ("agent_run ignores confines", TRIG,
     '        || prefixes.iter().any(|p| p.matches(head_ref.as_bytes()));',
     '        || prefixes.is_empty();', LIB),
    ("mark leaves the run unforked", TRIG, '            run.fork = true;', '            run.fork = false;', LIB),
]

if __name__ == '__main__':
    run(MUTANTS)
