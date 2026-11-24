#!/usr/bin/env python3
"""R47, counts by the wrong version (`crates/release/src/gate.rs`, `count` and `errors_sites`;
`decisions/DECISIONS.md`, "`gate-errors`"): an answer counted under the front's version instead of the one that ran
it, a thrown call not counted as the front's 5xx, an answer with no version counted as a thrown call, and every
script's counts homed on the lowest site. plant122 to plant124 run their mutants with this file's `run`.

Plants one mutant at a time, runs the release crate's tests, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant121.py          # every mutant
    python3 tests/m0carried/plant121.py 0 2      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GATE = 'crates/release/src/gate.rs'
REL = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib']

MUTANTS = [
    ("an answer counts under the front's version, not the version that ran it", GATE,
     '        Some(_) => one(errors_key(to), ran),\n', '        Some(_) => one(errors_key(to), front),\n', REL),
    ("a thrown call is not a 5xx of the front that served it", GATE,
     'u64::from(status.is_none_or(|s| s >= 500))', 'u64::from(status.is_some_and(|s| s >= 500))', REL),
    ("an answer with no version counts as a thrown call", GATE,
     '            None => &mut e.unversioned,\n        };\n        a.answers += 1;\n        a.fivexx += fivexx;\n',
     '            None => {\n                e.threw += 1;\n                &mut e.unversioned\n            }\n        };\n'
     '        a.answers += 1;\n        a.fivexx += fivexx;\n', REL),
    ("every script's counts go to the lowest site that answers", GATE,
     '    v.rotate_left(home);\n', '    let _ = home;\n', REL),
]


def run(mutants):
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    pick = [int(a) for a in sys.argv[1:]] or range(len(mutants))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = mutants[i]
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
    run(MUTANTS)
