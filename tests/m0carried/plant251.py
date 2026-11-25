#!/usr/bin/env python3
"""AM3-WP3's minimum, step S3 of LM-3's kill row (`decisions/DECISIONS.md`, "`run-create`"): a Graftyard-run create
taken from an installation token, an `AgentApp`'s user token, or a reader; with `agents_enabled` off; in fork mode;
without `graftyard-workspace` installed; with neither issue nor prompt; or under a session the request did not mint, or
not a version-8 UUID. And the `graftyard_task` delivery sent to every App that
subscribes, sent for an external task, not sent on cancel, or parsed by the controller from a task already moving.
Run from the repository root:

    python3 tests/m0carried/plant251.py          # every mutant
    python3 tests/m0carried/plant251.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

T = 'crates/git/src/task.rs'
I = 'crates/git/src/issue_ops.rs'
H = 'crates/git/src/hooks.rs'
S = 'crates/git/src/sequencer.rs'
C = 'crates/git/src/controller.rs'
GATES = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'task_gates']
API = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'task_api']
MUTANTS = [
    ("any principal with write creates a run task", T,
     '            if !ctx.human_writer || ctx.perm < Some(RepoPermission::Write) {\n',
     '            if ctx.perm < Some(RepoPermission::Write) {\n', GATES),
    ("a reader creates a run task", T,
     '            if !ctx.human_writer || ctx.perm < Some(RepoPermission::Write) {\n',
     '            if !ctx.human_writer {\n', GATES),
    ("an AgentApp's user-to-server token counts as a human", I,
     '                let human = app.is_none() && own == Decision::Allow && !agent_token;\n',
     '                let human = app.is_none() && own == Decision::Allow;\n', GATES),
    ("agents_enabled off still creates", T,
     '            if !policy.agents_enabled {\n                return Err(TaskErr::AgentsOff);\n',
     '            if false {\n                return Err(TaskErr::AgentsOff);\n', GATES),
    ("fork mode creates an in-repo task", T,
     '            if policy.agents_workspace != graftyard_identity::WorkspaceMode::InRepo {\n',
     '            if false {\n', GATES),
    ("a run task without graftyard-workspace installed", T,
     '    installed(run_app(WORKSPACE_SLUG)?)?;\n', '', GATES),
    ("neither an issue nor a prompt creates", T,
     '            if n.issue.is_none() && n.prompt.is_none() {\n                return Err(TaskErr::Invalid("prompt"));\n',
     '            if false {\n                return Err(TaskErr::Invalid("prompt"));\n', GATES),
    ("the session is not the request's", T,
     '            let session = run_session(ctx.request);\n',
     '            let session = run_session(graftyard_types::RequestId(graftyard_types::Ulid(7)));\n', GATES),
    ("the session id is not a version-8 UUID", T,
     '    let u = (u & !(0xf << 76)) | (0x8 << 76);\n', '', GATES),
    ("the delivery goes to every App that subscribes", S,
     '            for a in apps.iter().filter(|a| a.hook == t) {\n', '            for a in apps.iter() {\n', GATES),
    ("an external task's create carries the delivery", H,
     '            RepoEvent::TaskCreated(t) if t.kind == crate::task::TaskKind::GraftyardRun => {\n',
     '            RepoEvent::TaskCreated(t) => {\n', GATES),
    ("a cancel carries no delivery", H,
     '                    out.push(task_webhook(session, t.app, "cancelled", user_of(actor)));\n', '', GATES),
    ("the controller starts a task that already left queued", C,
     '            "created" if t["state"] == "queued" => Delivery::Created,\n',
     '            "created" => Delivery::Created,\n', API),
]

if __name__ == '__main__':
    run(MUTANTS)
