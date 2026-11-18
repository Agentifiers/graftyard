#!/usr/bin/env python3
"""shed-backoff and lease-gone: planted bugs in the read gate's shed retry and the lease reply
(`decisions/DECISIONS.md`, "`shed-backoff`" and "`lease-gone`"). Plants one mutant at a time, runs the test that guards
it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository
root:

    python3 tests/m0carried/plant43.py          # every mutant
    python3 tests/m0carried/plant43.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LOOKUP = 'crates/lookup/src/lib.rs'
EDGE = 'workers/edge/src/lib.rs'
LEASES = 'crates/api/src/leases.rs'
LOOKT = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib', '--', 'shed']
EDGET = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'a_write_asks']
LEASET = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'lease_api', '--', 'a_renew_whose_lease']

CAP = '    let cap = OVERLOAD_RETRY_MS.saturating_mul(1 << n.min(16)).min(SHED_BACKOFF_MAX_MS);\n'
JIT = '    (cap / 2 + (r.clamp(0.0, 1.0) * (cap / 2) as f64) as u64).max(named)\n'

MUTANTS = [
    ('no backoff: every wait is OVERLOAD_RETRY_MS, as on main', LOOKUP, CAP, '    let cap = OVERLOAD_RETRY_MS;\n', LOOKT),
    ('no cap on the backoff', LOOKUP, CAP, '    let cap = OVERLOAD_RETRY_MS.saturating_mul(1 << n.min(16));\n', LOOKT),
    ('the members\' retryAfterMs ignored', LOOKUP, JIT, '    cap / 2 + (r.clamp(0.0, 1.0) * (cap / 2) as f64) as u64\n', LOOKT),
    ('no jitter: the whole wait every time', LOOKUP, JIT, '    cap.max(named)\n', LOOKT),
    ('a wait that ends past the deadline is still taken', LOOKUP, '                if now() + ms >= until {\n',
     '                if now() >= until {\n', LOOKT),
    ('a write keeps LOOKUP_FOR, as on main', EDGE, '        (true, None) => graftyard_lookup::SHED_RETRY_FOR,\n',
     '        (true, None) => graftyard_lookup::LOOKUP_FOR,\n', EDGET),
    ('a bounded push ignores its bound', EDGE, '        (true, Some(left)) => left.min(graftyard_lookup::SHED_RETRY_FOR),\n',
     '        (true, Some(_)) => graftyard_lookup::SHED_RETRY_FOR,\n', EDGET),
    ('a renew whose lease is gone answers 500, as on main', LEASES,
     '            let Some(row) = lease_row(r.core, *id) else { return Reply::not_found() };\n',
     '            let Some(row) = lease_row(r.core, *id) else { return Reply::error(500, "no lease row", DOC) };\n', LEASET),
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
