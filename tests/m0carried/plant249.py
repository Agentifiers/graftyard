#!/usr/bin/env python3
"""BF-73 (F119, F120, F122): an Actions run's actor and number, recorded beside it in format 0x15.8, reach the job
message's `github.actor` and `github.run_number` and the ID token's `actor`, `actor_id` and `run_number`; the `sub`
follows the repository's customization over the owner's (`crates/actions/src/shard.rs`, `crates/actions/src/oidc.rs`,
`crates/actions/src/runsvc.rs`, `crates/actions/src/trigger.rs`). Run from the repository root:

    python3 tests/m0carried/plant249.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

S = 'crates/actions/src/shard.rs'
O = 'crates/actions/src/oidc.rs'
R = 'crates/actions/src/runsvc.rs'
T = 'crates/actions/src/trigger.rs'
LIB = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib']
TN = LIB + ['runs_are_numbered_per_workflow_and_carry_their_actor']
TS = LIB + ['a_sub_template_follows_the_repository_setting_over_the_owners']
TO = LIB + ['a_sub_template_writes_ghes_examples']
TF = LIB + ['every_command_format_is_read']
TT = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'pr_trigger', 'actor_and_number_ride_with_the_claims_flag']
MUTANTS = [
    ("a run numbers from 1 whatever ran before", S, '(over + base) as u64 + 1', '1', TN),
    ("the counter counts every workflow of the repository", S,
     "let mine = |id: &RunId, r: &RunRow| r.run.repo == repo && &*r.run.workflow == workflow && Some(*id) != not;",
     "let mine = |id: &RunId, r: &RunRow| r.run.repo == repo && Some(*id) != not;", TN),
    ("a follower counts the run it checks", S, 'self.number_without(r.run.repo, &r.run.workflow, Some(*run))',
     'self.number_without(r.run.repo, &r.run.workflow, None)', TN),
    ("the token carries no actor", O, '        c["actor"] = json!(login);\n', '', TO),
    ("the token's run_number is always 1", O, 'c["run_number"] = json!(n.to_string());', 'c["run_number"] = json!("1");', TO),
    ("the owner's template applies without the opt-in", O, '    if r.use_default {\n        return None;\n    }\n', '', TS),
    ("the repository's keys lose to the owner's", O, 'r.keys.as_deref().or(org)', 'org.or(r.keys.as_deref())', TS),
    ("a template's repo key writes the full claim name", O, '"repo" => format!("repo:{}", esc(c["repository"].as_str().unwrap_or(""))),',
     '"repo" => format!("repository:{}", esc(c["repository"].as_str().unwrap_or(""))),', TO),
    ("a template's value keeps its colons", O, 'k => format!("{k}:{}", esc(c[k].as_str().unwrap_or(""))),',
     'k => format!("{k}:{}", c[k].as_str().unwrap_or("")),', TO),
    ("the job message keeps the owner as actor", R, '("actor", json!(a.actor.unwrap_or(a.owner_login))),',
     '("actor", json!(a.owner_login)),', TT),
    ("the job message's run_number is always 1", R, '("run_number", json!(a.run_number.unwrap_or(1).to_string())),',
     '("run_number", json!("1")),', TT),
    ("the PR event drops its actor", T, 'Some(PrEvent { n, action: a, after, actor })',
     'Some(PrEvent { n, action: a, after, actor: None })', TT),
    ("the leader ignores the call's actor", R, '    if let Some(actor) = actor {\n        let (base_ref, head_ref)',
     '    if let Some(actor) = actor.filter(|_| false) {\n        let (base_ref, head_ref)', TT),
    ("CreateRunAs is sent as 0x15.7", S,
     'ActionsOp::CreateRunAs { .. } | ActionsOp::SetSubTemplate { .. } => ACTIONS_FORMAT_CLAIMS,',
     'ActionsOp::CreateRunAs { .. } | ActionsOp::SetSubTemplate { .. } => ACTIONS_FORMAT_HEAD_REF,', TT),
    ("the release reads no 0x15.8", S, '    ACTIONS_FORMAT_HEAD_REF,\n    ACTIONS_FORMAT_CLAIMS,\n    ACTIONS_FORMAT_REUSABLE,\n];',
     '    ACTIONS_FORMAT_HEAD_REF,\n    ACTIONS_FORMAT_HEAD_REF,\n    ACTIONS_FORMAT_REUSABLE,\n];', TF),
    ("the release reads no 0x15.7, as main 46ea4200's sites", S, '    ACTIONS_FORMAT_CACHE,\n    ACTIONS_FORMAT_HEAD_REF,\n    ACTIONS_FORMAT_CLAIMS,\n    ACTIONS_FORMAT_REUSABLE,\n];',
     '    ACTIONS_FORMAT_CACHE,\n    ACTIONS_FORMAT_CLAIMS,\n    ACTIONS_FORMAT_CLAIMS,\n    ACTIONS_FORMAT_REUSABLE,\n];', TF),
]

if __name__ == '__main__':
    run(MUTANTS)
