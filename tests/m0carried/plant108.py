#!/usr/bin/env python3
"""catchup-lag, lever B: planted bugs in the unapplied entries a run carries across its pieces
(`crates/git/src/catchup.rs`, `Run` and `follow`; `decisions/DECISIONS.md`, "`catchup-lag`"): a tail kept past the
cursor or applied twice, a run that outlives its end, and no tail at all. Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant108.py          # every mutant
    python3 tests/m0carried/plant108.py 0 2      # by number
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
    ("a tail is used wherever it starts, past the cursor too", CU,
     "        carry.take().filter(|c| c.first().is_some_and(|e| e.index == from)).map(Ok);\n",
     "        carry.take().map(Ok);\n", CU_T),
    ("a tail that starts before the cursor is used, so its first entries apply twice", CU,
     "        carry.take().filter(|c| c.first().is_some_and(|e| e.index == from)).map(Ok);\n",
     "        carry.take().filter(|c| c.first().is_some_and(|e| e.index <= from)).map(Ok);\n", CU_T),
    ("no tail is kept: each piece reads its page again (catchup-pieces as merged)", CU,
     "            if capped {\n                *carry = Some(rest);\n            }\n",
     "            let _ = rest;\n", CU_T),
    ("the run outlives its last piece: the next run reads no heads", CU,
     "    if behind {\n        *run = Run {",
     "    if true {\n        *run = Run {", CU_T),
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
