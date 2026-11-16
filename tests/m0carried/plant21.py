#!/usr/bin/env python3
"""promote-unlock: planted bugs in /promote's network lock (`decisions/DECISIONS.md`, "`promote-unlock`"). Plants one
mutant at a time, runs the test that guards it, and restores the file byte for byte. Each line prints CAUGHT, MISSED,
or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant21.py          # every mutant
    python3 tests/m0carried/plant21.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GO = 'containers/git/server/main.go'
GOT = ['go', 'test', '-count=1', '-timeout', '90s', '-run', 'TestPromoteWritesItsPackOutsideTheLock|TestPromotedPackIsComplete', './...']

MUTANTS = [
    ('the pack is written under the shared lock', GO,
     '"--delta-base-offset")\n\tl.RUnlock()\n\tlocked = false\n', '"--delta-base-offset")\n', GOT),
    ('the lock is released twice', GO,
     '\tl.RUnlock()\n\tlocked = false\n\tif err != nil {', '\tl.RUnlock()\n\tif err != nil {', GOT),
    ('a refused promotion keeps the lock', GO,
     '\tdefer func() {\n\t\tif locked {\n\t\t\tl.RUnlock()\n\t\t}\n\t}()\n',
     '\tdefer func() {\n\t\tif locked && false {\n\t\t\tl.RUnlock()\n\t\t}\n\t}()\n', GOT),
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
            cwd = os.path.join(ROOT, 'containers/git/server')
            r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=600)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'build failed' in out or 'setup failed' in out:
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
