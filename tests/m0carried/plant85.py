#!/usr/bin/env python3
"""`validate-hedge`: planted bugs in a human push's validate hedge (`workers/edge/src/lib.rs` `hedged` and
`validate_hedge`; `decisions/DECISIONS.md`, "`validate-hedge`"). Plants one mutant at a time, runs the test that guards
it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Never run it under
`timeout`: a kill leaves the mutant in place. Run from the repository root:

    python3 tests/m0carried/plant85.py          # every mutant
    python3 tests/m0carried/plant85.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LIB = 'workers/edge/src/lib.rs'
ED = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'a_human_validate']

MUTANTS = [
    ("no hedge: a human validate waits on one shard, as mt16's straddler did", LIB,
     '    (!agent).then_some(VALIDATE_HEDGE)', '    (agent && !agent).then_some(VALIDATE_HEDGE)', ED),
    ("an agent's validate is hedged too", LIB,
     '    (!agent).then_some(VALIDATE_HEDGE)', '    (agent || !agent).then_some(VALIDATE_HEDGE)', ED),
    ("the last try is passed by a hedge (G30, `chain-wait`)", LIB,
     '            timer = hedge.filter(|_| out.started < n)', '            timer = hedge.filter(|_| out.started <= n)', ED),
    ("a passed shard is not marked cold", LIB,
     '                passed(i);\n                out.hedged = true;', '                let _ = i;\n                out.hedged = true;', ED),
    ("a hedge abandons the slow try (a deadline, not a hedge)", LIB,
     '                out.hedged = true;\n                due = n - out.started;',
     '                out.hedged = true;\n                running.clear();\n                due = n - out.started;', ED),
    ("the hedge starts only the next site, not every one left (mt17's two slow shards in turn)", LIB,
     '                due = n - out.started;', '                due = 1;', ED),
    ("a 422 from the hedged shard does not decide", LIB,
     '            Some((i, r @ (Ok(_) | Err((422 | 429, _))))) => {', '            Some((i, r @ (Ok(_) | Err((429, _))))) => {', ED),
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800, stdin=subprocess.DEVNULL)
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
