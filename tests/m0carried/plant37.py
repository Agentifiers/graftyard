#!/usr/bin/env python3
"""clock-guard: planted bugs in the lease guard's lag (`decisions/DECISIONS.md`, "`clock-guard`"). Plants one mutant at
a time, runs the site crate's clock tests, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID
NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant37.py          # every mutant
    python3 tests/m0carried/plant37.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
CLOCK = 'workers/site/src/clock.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'clock']
LAG = '        let lag = stamp.saturating_sub(self.last.max(own).max(self.floor));\n'

MUTANTS = [
    ("the lag against this isolate's own clock: main's rule", CLOCK, LAG,
     '        let lag = stamp.saturating_sub(own);\n', TESTS),
    ('the lag ignores the readings given out', CLOCK, LAG,
     '        let lag = stamp.saturating_sub(own.max(self.floor));\n', TESTS),
    ("the lag ignores this isolate's own clock", CLOCK, LAG,
     '        let lag = stamp.saturating_sub(self.last.max(self.floor));\n', TESTS),
    ('the floor rises before the lag is taken, so no stamp ever lags', CLOCK,
     LAG + '        self.floor = self.floor.max(stamp);\n',
     '        self.floor = self.floor.max(stamp);\n' + LAG, TESTS),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        # A copy beside the file, so a run killed mid-mutant can be undone with `git checkout` or this copy.
        open(p + '.orig', 'wb').write(orig)
        open(p, 'w').write(s.replace(old, new))
        try:
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
        finally:
            open(p, 'wb').write(orig)
            os.remove(p + '.orig')
        out = r.stdout + r.stderr
        if 'error[E' in out or 'could not compile' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
