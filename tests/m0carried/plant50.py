#!/usr/bin/env python3
"""shed-retry-2s: planted bugs in a write's shed-retry budget (`decisions/DECISIONS.md`, "`shed-retry-2s`"). Plants one
mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED,
or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant50.py          # every mutant
    python3 tests/m0carried/plant50.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LOOKUP = 'crates/lookup/src/lib.rs'
EDGE = 'workers/edge/src/lib.rs'
LOOKT = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib', '--', 'shed']
EDGET = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'a_write_asks']

BUDGET = 'pub const SHED_RETRY_FOR: Duration = Duration::from_secs(2);\n'
CAP = '    let cap = OVERLOAD_RETRY_MS.saturating_mul(1 << n.min(16)).min(SHED_BACKOFF_MAX_MS);\n'
STOP = '                if now() + ms >= until {\n'

MUTANTS = [
    ('a write keeps 4 s, as on main', LOOKUP, BUDGET, 'pub const SHED_RETRY_FOR: Duration = Duration::from_secs(4);\n', LOOKT),
    ('a write keeps 3 s', LOOKUP, BUDGET, 'pub const SHED_RETRY_FOR: Duration = Duration::from_millis(3_000);\n', LOOKT),
    ('no backoff inside the shorter budget', LOOKUP, CAP, '    let cap = OVERLOAD_RETRY_MS;\n', LOOKT),
    ('a wait that ends past the deadline is still taken', LOOKUP, STOP, '                if now() >= until {\n', LOOKT),
    ('a bounded push ignores the shorter budget', EDGE,
     '        (true, Some(left)) => left.min(graftyard_lookup::SHED_RETRY_FOR),\n',
     '        (true, Some(left)) => left.min(graftyard_lookup::LOOKUP_FOR),\n', EDGET),
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
