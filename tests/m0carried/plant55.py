#!/usr/bin/env python3
"""dump-floor: planted bugs in which cached whole-tier dump answers a read (`decisions/DECISIONS.md`, "`dump-floor`";
B0-33 m61's rebuild sheds, `tests/am1/results/b033-m61-why.md`). Plants one mutant at a time, runs the tests that guard
it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository
root:

    python3 tests/m0carried/plant55.py          # every mutant
    python3 tests/m0carried/plant55.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
FAMILY = 'workers/site/src/family.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'fanout::tests::a_learner_dump_read',
         'fanout::tests::a_dump_is_built', 'fanout::tests::a_cached_dump', 'fanout::tests::dump_rebuilds']
COVERS = '        self.loaded == now.loaded && self.child == now.child && self.applied >= floor\n'

MUTANTS = [
    ("main's rule: a dump serves only the tiers' current state", FAMILY, COVERS,
     '        self.loaded == now.loaded && self.child == now.child && self.applied == now.applied && floor > 0\n', TESTS),
    ('a dump below the floor is served', FAMILY, COVERS,
     '        self.loaded == now.loaded && self.child == now.child && self.applied + 1 >= floor\n', TESTS),
    ("a loading member takes a loaded member's dump", FAMILY, COVERS,
     '        self.child == now.child && self.applied >= floor\n', TESTS),
    ("the metadata child takes its parent's dump", FAMILY, COVERS,
     '        self.loaded == now.loaded && self.applied >= floor\n', TESTS),
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
