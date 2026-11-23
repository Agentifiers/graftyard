#!/usr/bin/env python3
"""agent-stage-first: planted bugs in the edge's choice to stage an Agent push's pack before its first commit
(`decisions/DECISIONS.md`, "`agent-stage-first`"; B0-32 m3fwi3's second commit round). Plants one mutant at a time,
runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE.
Mutant 0 puts main's behaviour back, so the new test fails on main. Run from the repository root:

    python3 tests/m0carried/plant61.py          # every mutant
    python3 tests/m0carried/plant61.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
EDGE = 'workers/edge/src/lib.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'hot_tests']
RULE = '        (agent && inline > AGENT_INLINE_ENTRY)\n'

MUTANTS = [
    ("main's rule: only a group seen hot stages first", EDGE, RULE, '        false\n', TESTS),
    ("every class past the budget stages first", EDGE, RULE, '        (inline > AGENT_INLINE_ENTRY)\n', TESTS),
    ("a pack at the budget stages first", EDGE, RULE, '        (agent && inline >= AGENT_INLINE_ENTRY)\n', TESTS),
    ("the agent rule uses the hot threshold", EDGE, RULE, '        (agent && inline > INLINE_MAX_HOT)\n', TESTS),
    ("an agent push stages first only on a hot group", EDGE,
     '        (agent && inline > AGENT_INLINE_ENTRY)\n            || inline > INLINE_MAX_HOT\n                && STAGED',
     '        (agent || inline > INLINE_MAX_HOT)\n            && inline > INLINE_MAX_HOT\n                && STAGED',
     TESTS),
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
