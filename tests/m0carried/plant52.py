#!/usr/bin/env python3
"""receive-503: planted bugs in how a push's commit that gave no outcome is answered (B0-33 m59r33's four receive-pack
500s, `tests/am1/results/b033-m59.md`). Plants one mutant at a time, runs the test that guards it, and restores the file
byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant52.py          # every mutant
    python3 tests/m0carried/plant52.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
EDGE = 'workers/edge/src/lib.rs'
EDGET = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'a_commit_with_no_outcome']

MUTANTS = [
    ("main's 500: no outcome is an edge error", EDGE,
     '        Err(Err(e)) => Err(CommitFail::NoOutcome(e)),\n',
     '        Err(Err(e)) => return Err(Error::RustError(e)),\n', EDGET),
    ("an unreadable reply read as no outcome", EDGE,
     '        Ok(done) => serde_json::from_value::<PushOutcome>(done).map_err(|_| CommitFail::Wire(unreadable_reply())),\n',
     '        Ok(done) => serde_json::from_value::<PushOutcome>(done).map_err(|e| CommitFail::NoOutcome(e.to_string())),\n',
     EDGET),
    ("a member error tagged in /_bed/push's outcome", EDGE,
     '#[serde(untagged)]\npub(crate) enum CommitFail {\n',
     'pub(crate) enum CommitFail {\n', EDGET),
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
