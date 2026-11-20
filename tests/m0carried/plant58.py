#!/usr/bin/env python3
"""git-read-class: planted bugs in the edge's git request class, `resolve`'s chained reads, and the git read gate's
fresh re-read (`decisions/DECISIONS.md`, "`git-read-class`"). Plants one mutant at a time, runs the tests that guard
it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Mutants 0, 3 and 6 put
main's behaviour back, so each test fails on main. Run from the repository root:

    python3 tests/m0carried/plant58.py          # every mutant
    python3 tests/m0carried/plant58.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LIB = 'workers/edge/src/lib.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'a_fetch_is_read_class',
         'resolve_starts_its_three_reads', 'a_gate_closed_on_a_cached_authz']

MUTANTS = [
    ("main's class: a fetch is not read-class", LIB,
     '        | Route::Rpc { service: Service::UploadPack, .. } => (true, false),',
     '        | Route::Rpc { service: Service::UploadPack, .. } => (false, false),', TESTS),
    ("a push is read-class too", LIB,
     '        | Route::Rpc { service: Service::ReceivePack, .. } => (false, true),',
     '        | Route::Rpc { service: Service::ReceivePack, .. } => (true, true),', TESTS),
    ("a fetch is a write, so its shed reads retry", LIB,
     '        | Route::Rpc { service: Service::UploadPack, .. } => (true, false),',
     '        | Route::Rpc { service: Service::UploadPack, .. } => (true, true),', TESTS),
    ("main's resolve: the reads run in turn whatever is held", LIB,
     '    let Some((ga, gr)) = guess else {',
     '    let Some((ga, gr)) = guess.filter(|_| false) else {', TESTS),
    ("the guessed record is kept when the account moved", LIB,
     '    let r = if a == ga { r } else { second(a).await };',
     '    let r = r;', TESTS),
    ("the guessed name is kept when the record moved", LIB,
     '    if a == ga && r == gr { t } else { third(a, r).await }',
     '    if a == ga { t } else { third(a, r).await }', TESTS),
    ("main's gate: a closed gate on a cached Authz is final", LIB,
     '        (Some(_), Ok(d)) if d != graftyard_identity::Decision::Allow => Ok((decide(fresh().await)?, at)),\n',
     '', TESTS),
    ("an open gate on a cached Authz ages from now", LIB,
     '        (held, first) => Ok((first?, held.unwrap_or(at))),',
     '        (_, first) => Ok((first?, at)),', TESTS),
    ("a failed gate read is read again", LIB,
     '        (Some(_), Ok(d)) if d != graftyard_identity::Decision::Allow =>',
     '        (Some(_), Err(_)) => Ok((decide(fresh().await)?, at)),\n        (Some(_), Ok(d)) if d != graftyard_identity::Decision::Allow =>', TESTS),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out:
                verdict = 'DID NOT COMPILE'
            else:
                verdict = 'CAUGHT'
                caught += 1
        finally:
            open(p, 'wb').write(orig)
        print(f'{i} {name}: {verdict}', flush=True)
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
