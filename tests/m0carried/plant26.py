#!/usr/bin/env python3
"""feed-gone-route: planted bugs in the edge's routing of a member's `gone` (`decisions/DECISIONS.md`,
"`feed-gone-route`"). Plants one mutant at a time, runs the test that guards it, and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant26.py          # every mutant
    python3 tests/m0carried/plant26.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
FEED = 'crates/git/src/feed.rs'
RUST = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'lease_gates', 'does_not_end_the_poll']

MUTANTS = [
    ('the first gone is the poll\'s, as on main', FEED,
     '            _ => {}\n        }\n    }\n    let admitted',
     '            Answer::Gone => return Route::Gone(i),\n            _ => {}\n        }\n    }\n    let admitted', RUST),
    ('every member gone answers 429, not 410', FEED,
     '        return if answers.is_empty() { retry } else { Route::Gone(0) };', '        return retry;', RUST),
    ('any gone among the answers is the poll\'s', FEED,
     '    if admitted.is_empty() {', '    if admitted.len() < answers.len() {', RUST),
    ('a gone counts against the last resort', FEED,
     '    let all_lagging = admitted.iter().all(|(_, a, _, _)| *a == Admit::Lagging);',
     '    let all_lagging = admitted.len() == answers.len() && admitted.iter().all(|(_, a, _, _)| *a == Admit::Lagging);',
     RUST),
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
