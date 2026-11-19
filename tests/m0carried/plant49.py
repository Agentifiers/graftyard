#!/usr/bin/env python3
"""g11-warm: planted bugs in a planned transfer's wait for the target site's git cache shard
(`decisions/DECISIONS.md`, "`g11-warm`"). Plants one mutant at a time, runs the tests that guard it, and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant49.py          # every mutant
    python3 tests/m0carried/plant49.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GM = 'crates/identity/src/group_move.rs'
TEST = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--lib', '--', 'group_move']

MUTANTS = [
    # The orchestrator's plant: the transfer goes at once, warm or not.
    ('a transfer goes at once, whatever the shard answers', GM,
     '    if warm {\n        return (TransferGate::Go, None, waited);\n',
     '    if warm || !warm {\n        return (TransferGate::Go, None, waited);\n', TEST),
    ('a shard that never warms holds the transfer forever', GM,
     '    if waited >= bound {\n', '    if waited >= u64::MAX {\n', TEST),
    ('the deadline is counted from the latest ask, not the first', GM,
     '    let since = wait.map_or(now, |w| w.since);\n', '    let since = now;\n', TEST),
    ('a wait left by a stopped move is kept', GM,
     '    let wait = wait.filter(|w| now.0.saturating_sub(w.last.0) <= bound);\n', '    let wait = wait;\n', TEST),
    ('a warm shard after a cold deadline still holds', GM,
     '    if warm {\n        return (TransferGate::Go, None, waited);\n    }\n    if waited >= bound {\n',
     '    if warm && waited < bound {\n        return (TransferGate::Go, None, waited);\n    }\n    if waited >= bound && !warm {\n',
     TEST),
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
