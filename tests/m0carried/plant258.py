#!/usr/bin/env python3
"""BF-76 head 2, check-run annotations and substitution (`crates/actions/src/annotations.rs`, `crates/actions/src/shard.rs`
`FinishAnnotated`, `crates/actions/src/run_owner.rs` `check_run`, `crates/actions/src/pin_map.rs` `redirect`,
`crates/actions/src/launch.rs` `reply`, `crates/api/src/rest.rs` `list_check_run_annotations`;
`decisions/DECISIONS.md`, "`pin-map`"). The mutants: a runner annotation's line 0 kept, a path-less annotation with an
empty path, the job-level annotations before the steps', the count cap one too high, the byte cap ignored, a finish
recording annotations for another job's token, a follower taking annotations for an unfinished job, a round whose
completed check run drops them, a pin substitution keeping the upstream SHA, the reply naming the upstream
as `resolved_name`, the reply's `version` the fork SHA (the runner would file the action under it), and the REST list
ignoring paging. Run from the repository root:

    python3 tests/m0carried/plant258.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

AN = 'crates/actions/src/annotations.rs'
SH = 'crates/actions/src/shard.rs'
RO = 'crates/actions/src/run_owner.rs'
PM = 'crates/actions/src/pin_map.rs'
LA = 'crates/actions/src/launch.rs'
RE = 'crates/api/src/rest.rs'
TA = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib']
TG = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'gates']
TS = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces']
MUTANTS = [
    ("a runner annotation's line 0 is kept", AN,
     '.unwrap_or(0).max(1)', '.unwrap_or(0)', TA),
    ("an annotation that names no path keeps an empty one", AN,
     '.unwrap_or(".github")', '.unwrap_or("")', TA),
    ("the job-level annotations come before the steps'", AN,
     '    for v in get(b, "stepResults").and_then(Value::as_array).into_iter().flatten() {\n'
     '        out.extend(get(v, "annotations").and_then(Value::as_array).into_iter().flatten().filter_map(one));\n'
     '    }\n'
     '    out.extend(get(b, "annotations").and_then(Value::as_array).into_iter().flatten().filter_map(one));\n',
     '    out.extend(get(b, "annotations").and_then(Value::as_array).into_iter().flatten().filter_map(one));\n'
     '    for v in get(b, "stepResults").and_then(Value::as_array).into_iter().flatten() {\n'
     '        out.extend(get(v, "annotations").and_then(Value::as_array).into_iter().flatten().filter_map(one));\n'
     '    }\n', TA),
    ("a job keeps 51 annotations", AN,
     'if out.len() == ANNOTATIONS_MAX ||', 'if out.len() == ANNOTATIONS_MAX + 1 ||', TA),
    ("a job's annotation text is not capped", AN,
     'bytes.saturating_add(more) > ANNOTATION_BYTES_MAX', 'bytes.saturating_add(more) > usize::MAX - 1', TA),
    ("another job's token finishes the job and leaves annotations", SH,
     '(ActionsActor::Job(me), JobEvent::Renewed | JobEvent::Finished(_)) => *me == job,',
     '(ActionsActor::Job(_), JobEvent::Renewed | JobEvent::Finished(_)) => true,', TA),
    ("a follower takes annotations for a job that did not finish", SH,
     '            ActionsEvent::JobAnnotations { job, .. } if self.job(*job).and_then(|j| j.state.completed()).is_none() => {',
     '            ActionsEvent::JobAnnotations { job, .. } if self.job(*job).is_none() => {', TA),
    ("a round's completed check run drops the job's annotations", RO,
     'fold.job_annotations(j.spec.id)', '&[]', TG),
    ("an approved pin resolves the fork at the upstream SHA", PM,
     '        Substitution::Pin { fork, fork_sha } => (fork, fork_sha.clone()),',
     '        Substitution::Pin { fork, fork_sha: _ } => (fork, a.version.clone()),', TA),
    ("the reply names the upstream as resolved_name", LA,
     '                let resolved = a.resolved.as_deref().unwrap_or(&a.name);',
     '                let resolved = &a.name;', TA),
    ("the reply's version is the fork's SHA", LA,
     '                let version = &a.key[a.name.len() + 1..];', '                let version = &a.version;', TA),
    ("the annotations list ignores paging", RE,
     '        .skip(page.skip())\n        .take(page.per_page)\n        .map(|a| {',
     '        .map(|a| {', TS),
]

if __name__ == '__main__':
    run(MUTANTS)
