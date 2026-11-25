#!/usr/bin/env python3
"""BF-78 (`on: push`, Actions format 0x15.12; `decisions/DECISIONS.md`, "`push-trigger`"): which pushes start runs
and what each run carries (`crates/actions/src/trigger.rs`, `push_events`, `push_run`, `skips_ci`;
`crates/actions/src/shard.rs`, `WorkflowRun::key`, `CreateRunPush`, the format; `crates/actions/src/runsvc.rs`,
`create_run_op` and `acquire_message`; `crates/api/src/webhooks.rs`, `actions_push`). Run from the repository root:

    python3 tests/m0carried/plant255.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

T = 'crates/actions/src/trigger.rs'
S = 'crates/actions/src/shard.rs'
R = 'crates/actions/src/runsvc.rs'
W = 'crates/api/src/webhooks.rs'
PT = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'pr_trigger']
ONE = PT + ['one_entry_of_pushes_starts_a_run_per_matching_ref_at_its_commit']
EDGES = PT + ['push_events_skip_deletes_queue_refs_and_more_than_three_tags']
SKIP = PT + ['a_head_commit_with_a_skip_marker_skips_push_runs']
LIB = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'a_push_run_records_its_event_and_keys_by_its_ref']
API = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'webhooks', 'an_actions_push_payload_carries']
PLAN = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'reusable', 'a_planned_push_run_keeps_its_plan']
MUTANTS = [
    ("a branch delete starts a run", T,
     'out.retain(|p| p.after != ObjectId::ZERO && p.after != p.before', 'out.retain(|p| p.after != p.before', EDGES),
    ("more than three tags still start tag runs", T,
     '&& !(p.tag() && tags > PUSH_TAGS_MAX));', '&& !(p.tag() && tags > PUSH_TAGS_MAX + 1));', EDGES),
    ("a queue ref starts a push run", T,
     'if !named || n.starts_with(b"refs/heads/gh-readonly-queue/") {', 'if !named {', EDGES),
    ("a merged pull request is no push", T,
     '            RepoEvent::Merged { by, .. } => Some(by.0),\n', '', EDGES),
    ("a ref moved twice in one entry keeps its first after", T,
     'Some(p) => (p.after, p.pusher) = (after, pusher.or(p.pusher)),', 'Some(p) => p.pusher = pusher.or(p.pusher),', EDGES),
    ("the run is at the old tip", T, '        head_sha: p.after.0,\n        head_ref: r#ref.into(),',
     '        head_sha: p.before.0,\n        head_ref: r#ref.into(),', ONE),
    ("a push run's paths are ignored", T,
     'triggers(ev, &Trigger::Push { r#ref: &r#ref, paths })', 'triggers(ev, &Trigger::Push { r#ref: &r#ref, paths: None })',
     ONE),
    ("a push run's key does not name its ref", S,
     'if self.is_merge_group() || self.is_pull_request() || self.is_push() {',
     'if self.is_merge_group() || self.is_pull_request() {', ONE),
    ("a push is sent as 0x15.8", S, 'op if op.created().is_some_and(WorkflowRun::is_push) => ACTIONS_FORMAT_PUSH,',
     'op if op.created().is_some_and(WorkflowRun::is_push) => ACTIONS_FORMAT_CLAIMS,', ONE),
    ("the pusher is not recorded", S,
     '                    if let Some((actor, login)) = actor.clone() {\n                        evs.push(ActionsEvent::RunActor { run: id, actor, login });\n                    }\n                    evs.push(ActionsEvent::RunNumber { run: id, number: s.next_number(run.repo, &run.workflow) });\n                    evs.push(ActionsEvent::RunEvent',
     '                    evs.push(ActionsEvent::RunNumber { run: id, number: s.next_number(run.repo, &run.workflow) });\n                    evs.push(ActionsEvent::RunEvent', ONE),
    ("a follower takes a payload for a run that is not a push", S,
     'if !self.run(*run).is_some_and(|r| r.run.is_push()) => {', 'if self.run(*run).is_none() => {', LIB),
    ("the leader drops the push payload", R, "if let Some(event) = b[\"event\"].as_str().filter(|_| run.is_push()) {",
     "if let Some(event) = b[\"event\"].as_str().filter(|_| false) {", ONE),
    ("the job message carries no github.event", R, "        d.push(json!({\"k\": \"event\", \"v\": context_of(&ev)}));\n", '', ONE),
    ("forced is always false", W, '    m.insert("forced".into(), json!(forced));\n', '', API),
    ("the trailer skips from any line", T,
     'matches!(last.trim(), "skip-checks:true" | "skip-checks: true")',
     'message.lines().any(|l| matches!(l.trim(), "skip-checks:true" | "skip-checks: true"))', SKIP),
    ("a planned push run drops its plan", S,
     '                    if let Some(plan) = plan {\n                        evs.push(ActionsEvent::RunPlanned { run: id, plan: plan.clone() });\n                    }\n                }',
     '                }', PLAN),
    ("a push call with a plan is CreateRunPlanned, which drops the event", R,
     "        let plan = if b[\"plan\"].is_null() { None } else { Some(plan()?) };",
     "        if !b[\"plan\"].is_null() { return Ok((key, ActionsOp::CreateRunPlanned { run, jobs, permissions, plan: plan()?, base_ref: None, head_ref: None, actor })); }\n        let plan = None;", PLAN),
]

if __name__ == '__main__':
    run(MUTANTS)
