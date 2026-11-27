#!/usr/bin/env python3
"""F142, `secrets-wait` (`crates/actions/src/job.rs`, `JobState::SecretsWait`; `crates/actions/src/shard.rs`,
`CreateRunWaiting`, `ReleaseSecrets`, `ActionsEvent::SecretsWait`, `ActionsFold::released`;
`crates/actions/src/secrets.rs`; `crates/actions/src/rest.rs`; `crates/actions/src/trigger.rs`, `gate_events`;
`decisions/DECISIONS.md`, "`secrets-wait`"): no job waits, a waiting job taken by a runner, a release for any head,
a release for another repository, a release by an admin, the release not recorded, a fork run's unreleased job
reading secrets, the scanner missing the bracket form or the whole map, the run's token counted as a secret, a failed
need not skipping a waiting job, a waiting job with met needs left pending after its release, a waiting job rendered
queued, and one approval's entry released twice. Run from the repository root:

    python3 tests/m0carried/plant286.py          # every mutant
    python3 tests/m0carried/plant286.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

JOB = 'crates/actions/src/job.rs'
SHARD = 'crates/actions/src/shard.rs'
SEC = 'crates/actions/src/secrets.rs'
REST = 'crates/actions/src/rest.rs'
TRIG = 'crates/actions/src/trigger.rs'
GATES = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'gates', 'an_agent_runs_secret_jobs']
LIB = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', '--', 'secrets', 'render_ghes']
PRT = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'pr_trigger', 'gate_events']
MUTANTS = [
    ("no job waits", SHARD,
     '                    if !jobs.is_empty() {\n                        evs.push(ActionsEvent::SecretsWait',
     '                    if false {\n                        evs.push(ActionsEvent::SecretsWait', GATES),
    ("a runner takes a waiting job", JOB,
     '        (S::Queued { .. }, E::Matched(r)) => Some(S::Assigned { runner: *r, lease_until: lease }),',
     '        (S::Queued { .. } | S::SecretsWait { .. }, E::Matched(r)) => {\n'
     '            Some(S::Assigned { runner: *r, lease_until: lease })\n        }', LIB + ['a_secrets_waiting']),
    ("a release for any head releases", SHARD,
     '        if h != head || s.run(j.run).is_none_or(|r| r.run.repo != repo || !on(&r.run)) {',
     '        if s.run(j.run).is_none_or(|r| r.run.repo != repo || !on(&r.run)) {', GATES),
    ("a release for another repository releases", SHARD,
     '        if h != head || s.run(j.run).is_none_or(|r| r.run.repo != repo || !on(&r.run)) {',
     '        if h != head || s.run(j.run).is_none_or(|r| !on(&r.run)) {', GATES),
    ("an admin releases", SHARD,
     '            ActionsOp::ReleaseSecrets { repo, head } => {\n                only(operator)?;',
     '            ActionsOp::ReleaseSecrets { repo, head } => {\n                only(admin)?;', GATES),
    ("the release is not recorded", SHARD,
     '                if *event == JobEvent::GateHeld {\n                    t.put_released(*job);',
     '                if false {\n                    t.put_released(*job);', GATES),
    ("a fork run's unreleased job reads secrets", SEC, '    !run.fork || released', '    !run.fork || !released', GATES),
    ("the scanner misses secrets['X']", SEC,
     "            Some(b'[') => {", "            Some(b'[') if false => {", LIB),
    ("the scanner misses the whole map", SEC,
     "            Some(b')' | b'}' | b',' | b':') => return true,", "            Some(b'}') => return true,", LIB),
    ("the run's token counts as a secret", SEC,
     '                if e > s && &t[s..e] != "github_token" {', '                if e > s {', LIB),
    ("a failed need does not skip a waiting job", JOB,
     '        (S::Pending { .. } | S::SecretsWait { .. }, E::NeedsFailed) => Some(done(Conclusion::Skipped)),',
     '        (S::Pending { .. }, E::NeedsFailed) => Some(done(Conclusion::Skipped)),', GATES),
    ("a waiting job whose needs succeeded stays pending after its release", SHARD,
     '        if !needs.is_empty() && needs.iter().all(met) {', '        if false && needs.iter().all(met) {', GATES),
    ("a waiting job renders queued", REST,
     '        JobState::Pending { .. } | JobState::Held { .. } | JobState::SecretsWait { .. } => ("waiting", None),\n'
     '        JobState::Queued { .. } | JobState::Assigned { .. } => ("queued", None),',
     '        JobState::Pending { .. } | JobState::Held { .. } => ("waiting", None),\n'
     '        JobState::Queued { .. } | JobState::Assigned { .. } | JobState::SecretsWait { .. } => ("queued", None),',
     LIB),
    ("one approval's entry releases twice", TRIG,
     '            && !out.iter().any(|x| x.n == g.pr && x.head == g.head)', '', PRT),
]

if __name__ == '__main__':
    run(MUTANTS)
