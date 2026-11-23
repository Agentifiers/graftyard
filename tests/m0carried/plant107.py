#!/usr/bin/env python3
"""catchup-lag, lever A: planted bugs in the room budget a run carries across its pieces
(`crates/git/src/catchup.rs`, `Run`; `decisions/DECISIONS.md`, "`catchup-lag`"). Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant107.py          # every mutant
    python3 tests/m0carried/plant107.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
CU = 'crates/git/src/catchup.rs'
GO = 'containers/git/server/main.go'
CU_T = (['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'catchup'], '.')
GO_T = (['go', 'test', '-count=1', '-run', 'IngestSpooledBatch|IngestBatch', '.'], 'containers/git/server')

MUTANTS = [
    ("the budget resets each piece (catchup-pieces as merged)", CU,
     "    let mut t = Tally { room: carried.room, ..Tally::default() };\n",
     "    let mut t = Tally { room: 0, ..Tally::default() };\n", CU_T),
    ("the run hands on no budget", CU,
     "Some(to.clone()), room: t.room",
     "Some(to.clone()), room: 0", CU_T),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, (cmd, cwd) = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=os.path.join(ROOT, cwd), capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out or '[build failed]' in out:
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
