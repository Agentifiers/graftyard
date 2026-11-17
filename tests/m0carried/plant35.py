#!/usr/bin/env python3
"""dep-hedge: planted bugs in the hedged dependency read (`decisions/DECISIONS.md`, "`dep-hedge`"). Plants one mutant
at a time, runs the test that guards it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID
NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant35.py          # every mutant
    python3 tests/m0carried/plant35.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LOOKUP = 'crates/lookup/src/lib.rs'
TEST = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--', 'a_dependency_read_with_no_answer']

MUTANTS = [
    ('as on main: no second request', LOOKUP,
     '    hedge(read(), delay(DEP_HEDGE), &read).await', '    (read().await, Hedged::First)', TEST),
    ('the second request is sent at once', LOOKUP,
     '    hedge(read(), delay(DEP_HEDGE), &read).await', '    hedge(read(), delay(Duration::ZERO), &read).await', TEST),
    ('the first\'s error ends the read while the second may answer', LOOKUP,
     '        Either::Left((Err(e), second)) => match second.await {\n            Ok(v) => (Ok(v), Hedged::Won),\n            Err(_) => (Err(e), Hedged::Asked),\n        },',
     '        Either::Left((Err(e), _)) => (Err(e), Hedged::Asked),', TEST),
    ('the second\'s error ends the read while the first may answer', LOOKUP,
     '        Either::Right((Err(_), first)) => (first.await, Hedged::Asked),',
     '        Either::Right((Err(e), _)) => (Err(e), Hedged::Won),', TEST),
    ('the hedge waits for both answers', LOOKUP,
     '        Either::Right((Ok(v), _)) => (Ok(v), Hedged::Won),',
     '        Either::Right((Ok(v), first)) => {\n            let _ = first.await;\n            (Ok(v), Hedged::Won)\n        }', TEST),
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=900)
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
