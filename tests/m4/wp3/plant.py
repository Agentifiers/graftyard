#!/usr/bin/env python3
"""M4-WP3's planted bugs: plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/m4/wp3/plant.py          # every mutant
    python3 tests/m4/wp3/plant.py 0 3      # by number

`$CARGO_TARGET_DIR` defaults to `../target-m4a`.
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
J = 'crates/actions/src/job.rs'
S = 'crates/actions/src/shard.rs'
O = 'crates/actions/src/run_owner.rs'
C = 'crates/git/src/run_checks.rs'
ACT = ['-p', 'graftyard-actions', '--test', 'gates', '--lib']

# (name, file, old, new, test args)
MUTANTS = [
    # ---- the job state machine ----
    ('a lease never expires a running job', J,
     '        (S::InProgress { lease_until, .. }, E::LeaseExpired) if now >= *lease_until => Some(done(Conclusion::Failure)),',
     '', ACT),
    ('an expired lease from InProgress requeues the job', J,
     '        (S::InProgress { lease_until, .. }, E::LeaseExpired) if now >= *lease_until => Some(done(Conclusion::Failure)),',
     '        (S::InProgress { lease_until, .. }, E::LeaseExpired) if now >= *lease_until => Some(S::Queued { labels: labels.clone(), since: now }),',
     ACT),
    ('every renew rewrites the lease', J,
     '            if lease_until.0 < now.0 + LEASE_REWRITE_UNDER.as_millis() as u64 {',
     '            if true {', ACT),
    ('a renew extends the lease by idle time past JOB_LEASE', J,
     '    let lease = after(now, JOB_LEASE);', '    let lease = after(now, JOB_LEASE * 2);', ACT),
    ('a completed job takes a cancel', J,
     '        (S::Completed { .. }, _) => None,', '', ACT),
    # ---- the shard ----
    ('a match commits against a stale version', S,
     '        if j.version != version {\n            return Err(ActionsErr::Stale(j.version));\n        }', '', ACT),
    ('a match ignores labels', S,
     '            && !s.runner(*r).is_some_and(|rec| j.spec.labels.fits(&rec.labels))',
     '            && s.runner(*r).is_none()', ACT),
    ('a second run for one trigger', S,
     '        if s.has_key(&run.key()) {\n            return Err(ActionsErr::Duplicate);\n        }', '', ACT),
    ('a stall moves a lease by more than its length', S,
     '            if *lease_until >= x.since =>\n        {\n            lease_until.0 += by\n        }',
     '            if *lease_until >= x.since =>\n        {\n            lease_until.0 += by * 2\n        }', ACT),
    ('a stall moves no lease', S,
     '            if *lease_until >= x.since =>\n        {\n            lease_until.0 += by\n        }',
     '            if *lease_until >= x.since =>\n        {\n            lease_until.0 += 0 * by\n        }', ACT),
    ('a stall revives a lease that had ended before it', S,
     '            if *lease_until >= x.since =>', '            if true =>', ACT),
    ('a failed need still releases its dependents', S,
     '            let event = if cs.iter().all(|c| *c == Conclusion::Success) {',
     '            let event = if cs.iter().all(|c| *c != Conclusion::Cancelled) {', ACT),
    ('a skip does not release its own dependents', S,
     '                done.insert(j.spec.id, Conclusion::Skipped);\n                JobEvent::NeedsFailed',
     '                JobEvent::NeedsFailed', ACT),
    ('the RunOwner cursor moves back', S,
     '                if *through <= s.owner_cursor() {', '                if *through == s.owner_cursor() {', ACT),
    # ---- RunOwner and the repository's writes ----
    ('RunOwner never cancels a gone repository\'s run', O,
     '                then.push(ActionsOp::CancelRun { run: *r });\n', '', ACT),
    ('a replayed check run moves its row back', C,
     '            if get_rec::<Timed<CheckRunUpdate>>(meta, "check_run", &key).is_some_and(|t| t.rec.version >= u.version) {',
     '            if get_rec::<Timed<CheckRunUpdate>>(meta, "check_run", &key).is_some_and(|t| t.rec.version > u.version) {',
     ACT),
    ('a completed check writes no conclusion', C,
     '                    conclusion: if done { u.conclusion } else { None },', '                    conclusion: None,',
     ACT),
    ('every job\'s check row is version 1', O,
     '        CheckStatus::Completed => 3,\n        CheckStatus::InProgress => 2,',
     '        CheckStatus::Completed => 1,\n        CheckStatus::InProgress => 1,', ACT),
]

env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '..', 'target-m4a')),
           CARGO_INCREMENTAL='0')
only = sys.argv[1:] and set(int(x) for x in sys.argv[1:])
for n, (name, path, old, new, args) in enumerate(MUTANTS):
    if only and n not in only:
        continue
    f = os.path.join(ROOT, path)
    orig = open(f, 'rb').read()
    src = orig.decode()
    assert src.count(old) == 1, (name, 'anchor not unique', src.count(old))
    open(f, 'w').write(src.replace(old, new, 1))
    try:
        r = subprocess.run(['cargo', 'test', '-j', '3', *args, '--no-fail-fast'], cwd=ROOT, env=env,
                           capture_output=True, text=True)
        out = r.stdout + r.stderr
        failed = sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.endswith('FAILED')})
        if ('error[' in out or out.lstrip().startswith('error:') or '\nerror: ' in out) and not failed:
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 else 'MISSED'
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig
