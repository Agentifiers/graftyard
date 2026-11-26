#!/usr/bin/env python3
"""BF-76 head 3, v4 artifacts through Twirp and the block shim (`crates/actions/src/twirp.rs`, `crates/actions/src/runsvc.rs`
`sign_job_token` and `job_message`, `crates/actions/src/artifacts.rs` `of_artifact`; `decisions/DECISIONS.md`,
"`actions-artifacts`", the v4 amendment). The mutants: the scope naming the job before the run, a call naming another
run admitted, a list keeping another run's rows, a list oldest first, a `digest` left out, retention truncated instead of
rounded, a block id's `+` read back as a space kept, a block list committed in staging order, a listed block that was
never staged committed, a delete that misses the staged blocks, a job token with no `scp`, and a job message with no
`ResultsServiceUrl`. Run from the repository root:

    python3 tests/m0carried/plant269.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

TW = 'crates/actions/src/twirp.rs'
RS = 'crates/actions/src/runsvc.rs'
AR = 'crates/actions/src/artifacts.rs'
TA = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib']
MUTANTS = [
    ("the scope names the job before the run", TW,
     '    (job_guid(2, run.0.0), job_guid(1, job.0.0))', '    (job_guid(1, job.0.0), job_guid(2, run.0.0))', TA),
    ("a call naming another run is admitted", TW,
     'body["workflow_run_backend_id"].as_str() == Some(backend_ids(run, JobId(DbId(0))).0.as_str())',
     'body["workflow_run_backend_id"].as_str().is_some_and(|r| r.starts_with("00000000-0000-4002-"))', TA),
    ("the list keeps another run's rows", TW,
     'r.run == run && !r.expired(now_ms)', '!r.expired(now_ms)', TA),
    ("the list is oldest first", TW,
     '    out.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));\n    out',
     '    out.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));\n    out', TA),
    ("the list leaves out the digest", TW,
     '        v["digest"] = json!(d);', '        let _ = d;', TA),
    ("retention is truncated, not rounded", TW,
     '    Some(((ahead + DAY_MS / 2) / DAY_MS).min(RETENTION_DAYS_MAX) as i64)',
     '    Some((ahead / DAY_MS).min(RETENTION_DAYS_MAX) as i64)', TA),
    ("a block id's + decoded as a space is kept", TW,
     "    let id = id.replace(' ', \"+\");", '    let id = id.to_owned();', TA),
    ("a block list commits in staging order", TW,
     '    for id in list {', '    for id in staged.iter().filter_map(|(p, _)| list.iter().find(|i| block_part(i).as_deref() == Some(p))) {', TA),
    ("a listed block that was never staged commits", TW,
     '            .map(|(_, n)| *n)\n            .ok_or_else(|| format!("the block {id} was not staged by this upload"))?;',
     '            .map(|(_, n)| *n)\n            .unwrap_or(0);', TA),
    ("a delete misses the artifact's staged blocks", AR,
     '        Some(("i" | "v", rest)) => named(rest),', '        Some(("i", rest)) => named(rest),', TA),
    ("a job token carries no scp", RS,
     'scp: Some(crate::twirp::results_scope(run, job)),', 'scp: None,', TA),
    ("a v4 job message names no ResultsServiceUrl", RS,
     '        data.insert("ResultsServiceUrl".into(), json!(urls.results_service()));', '', TA),
]

if __name__ == '__main__':
    run(MUTANTS)
