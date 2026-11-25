#!/usr/bin/env python3
"""BF-72 `reusable`, the shard, the wire and the token (format 0x15.9; `crates/actions/src/shard.rs`,
`crates/actions/src/runsvc.rs`, `crates/actions/src/oidc.rs`, `crates/actions/src/reusable.rs`). The mutants:
`CreateRunPlanned` sent as 0x15.7 (so a member without its reader is sent it), a planned run that records no plan,
`FinishWith` that records no outputs, the fold that reads no outputs, a secret output kept from `completejob`, the job
message without `jobOutputs`, a called job's `job_workflow_ref` naming the caller, `job_workflow_sha` the run's commit,
the token's facts without the job's call, a planned run that records no actor or no number (BF-73's rows), the leader
dropping a planned call's actor, a release whose members do not read 0x15.9, a `./` call read at the default branch instead of the caller's commit, and the leader
taking a planned call as a plain run. Run from the repository root:

    python3 tests/m0carried/plant252.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

S = 'crates/actions/src/shard.rs'
R = 'crates/actions/src/runsvc.rs'
O = 'crates/actions/src/oidc.rs'
U = 'crates/actions/src/reusable.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'reusable']
MUTANTS = [
    ("CreateRunPlanned is sent as 0x15.7", S,
     'ActionsOp::CreateRunPlanned { .. } | ActionsOp::FinishWith { .. } => ACTIONS_FORMAT_REUSABLE,',
     'ActionsOp::CreateRunPlanned { .. } | ActionsOp::FinishWith { .. } => ACTIONS_FORMAT_HEAD_REF,', T),
    ("a planned run records no plan", S, '                    evs.push(ActionsEvent::RunNumber { run: id, number: s.next_number(run.repo, &run.workflow) });\n                    evs.push(ActionsEvent::RunPlanned { run: id, plan: plan.clone() });\n',
     '                    evs.push(ActionsEvent::RunNumber { run: id, number: s.next_number(run.repo, &run.workflow) });\n', T),
    ("FinishWith records no outputs", S, '                    evs.push(ActionsEvent::JobOutputs { job: *job, outputs: outputs.clone() });\n', '', T),
    ("the fold reads no outputs", S, 'self.more.more.more.more.more.outputs.get(&j).map(Vec::as_slice)', 'None', T),
    ("a secret output is kept", R, '.filter(|(_, v)| get(v, "isSecret") != Some(Value::Bool(true)))', '', T),
    ("the job message carries no jobOutputs", R, '        msg["jobOutputs"] = o;', '        let _ = o;',
     ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'a_job_message_carries_a_planned_jobs_contexts']),
    ("job_workflow_ref names the caller", O,
     '"job_workflow_ref": f.job_workflow.map_or_else(|| workflow_ref.clone(), |c| c.workflow_ref(f.repository)),',
     '"job_workflow_ref": workflow_ref.clone(),', T),
    ("job_workflow_sha is the run's commit", O,
     '"job_workflow_sha": f.job_workflow.map_or_else(|| sha.clone(), |c| c.sha.iter().map(|b| format!("{b:02x}")).collect()),',
     '"job_workflow_sha": sha.clone(),', T),
    ("the token's facts carry no call", S, '            called: crate::reusable::job_frame(self, j).cloned(),', '            called: None,', T),
    ("a planned run records no actor", S,
     '                    if let Some((actor, login)) = actor.clone() {\n                        evs.push(ActionsEvent::RunActor { run: id, actor, login });\n                    }\n                    evs.push(ActionsEvent::RunNumber { run: id, number: s.next_number(run.repo, &run.workflow) });\n                    evs.push(ActionsEvent::RunPlanned',
     '                    let _ = actor;\n                    evs.push(ActionsEvent::RunNumber { run: id, number: s.next_number(run.repo, &run.workflow) });\n                    evs.push(ActionsEvent::RunPlanned', T),
    ("a planned run is not numbered", S,
     '                    evs.push(ActionsEvent::RunNumber { run: id, number: s.next_number(run.repo, &run.workflow) });\n                    evs.push(ActionsEvent::RunPlanned',
     '                    evs.push(ActionsEvent::RunPlanned', T),
    ("the leader drops a planned call's actor", R,
     'ActionsOp::CreateRunPlanned { run, jobs, permissions, plan, base_ref, head_ref, actor }',
     'ActionsOp::CreateRunPlanned { run, jobs, permissions, plan, base_ref, head_ref, actor: { let _ = actor; None } }', T),
    ("a ./ call is read at the default branch", U,
     '        let version = Version::At { r#ref: from.r#ref.clone(), sha: from.sha };',
     '        let version = Version::Named("main".into());', T),
    ("the leader takes a planned call as a plain run", R, '    if !b["plan"].is_null() {', '    if false {', T),
    ("the release reads no 0x15.9", S, '    ACTIONS_FORMAT_CLAIMS,\n    ACTIONS_FORMAT_REUSABLE,\n    ACTIONS_FORMAT_PUSH,\n];',
     '    ACTIONS_FORMAT_CLAIMS,\n    ACTIONS_FORMAT_CLAIMS,\n    ACTIONS_FORMAT_PUSH,\n];',
     ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'every_command_format_is_read']),
]

if __name__ == '__main__':
    run(MUTANTS)
