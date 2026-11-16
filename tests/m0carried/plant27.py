#!/usr/bin/env python3
"""agents-write: planted bugs in which agent-route requests count as writes (`decisions/DECISIONS.md`,
"`agents-write`"). Plants one mutant at a time, runs the test that guards it, and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant27.py          # every mutant
    python3 tests/m0carried/plant27.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
AGENTS = 'workers/edge/src/agents.rs'
EDGET = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'the_agent_routes']

MUTANTS = [
    ('no agent route is a write, as on main', AGENTS, '    *method != Method::Get\n', '    false\n', EDGET),
    ('only POST is a write', AGENTS, '    *method != Method::Get\n', '    *method == Method::Post\n', EDGET),
    ('every request is a write, GETs included', AGENTS, '    *method != Method::Get\n', '    true\n', EDGET),
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
