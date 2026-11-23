#!/usr/bin/env python3
"""`cold-write` cw3: planted bugs in the install guess's comparison (`workers/edge/src/lib.rs`, `kept_guess`;
`decisions/DECISIONS.md`, "`cold-write`", cw3). A guess is kept only when it equals the real `Placement` row on every
field a read uses, `at` alone differing. Plants one mutant at a time, runs the test that guards it, and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Never run it under `timeout`: a kill leaves the
mutant in place. Run from the repository root:

    python3 tests/m0carried/plant100.py          # every mutant
    python3 tests/m0carried/plant100.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
EDGE = 'workers/edge/src/lib.rs'
ET = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'the_install_guess', 'rows_read_beside',
      'chained_reads']

MUTANTS = [
    ("a guess that differs only in generation is kept", EDGE,
     '            && *generation == b.generation\n', '            && (*generation == b.generation || true)\n', ET),
    ("a guess that differs only in moving_from is kept (None against Some)", EDGE,
     '                (None, None) => true,\n                (Some(x), Some(y)) => same(x, y),\n                _ => false,',
     '                (None, None) => true,\n                (Some(x), Some(y)) => same(x, y),\n                _ => true,', ET),
    ("a moving_from that differs in its members is kept", EDGE,
     '                (Some(x), Some(y)) => same(x, y),', '                (Some(_), Some(_)) => true,', ET),
    ("a guess that differs only in home is kept", EDGE,
     '            && *home == b.home\n', '\n', ET),
    ("a guess that differs only in sites is kept", EDGE,
     '            && *sites == b.sites\n', '\n', ET),
    ("the stamp still counts (the guess is dropped on every cold isolate, as before cw3)", EDGE,
     '        Some(g) if same(g, &fresh) => g.clone(),', '        Some(g) if *g == fresh => g.clone(),', ET),
    ("a kept guess answers the real row, so link reads the fold again", EDGE,
     '        Some(g) if same(g, &fresh) => g.clone(),', '        Some(g) if same(g, &fresh) => fresh,', ET),
    ("the install read compares the raw answer again (kept_guess not on the path)", EDGE,
     '        async move { Ok(placement.await?.map(|r| kept_guess(guess.as_ref(), r))) }',
     '        async move { let _ = &guess; placement.await }', ET),
]


def main():
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, errors='replace', timeout=1800,
                               stdin=subprocess.DEVNULL)
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
