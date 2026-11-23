#!/usr/bin/env python3
"""edge-shed-429: planted bugs in the edge's answer to an Agent write whose row read every member shed
(`workers/edge/src/api.rs`, `shed_for`; FRAME, "Operator decision, 2026-11-23, B0-33's merge-queue and refusal-shape
lines"). Plants one mutant at a time, runs the test that guards it, and restores the file byte for byte. Each line
prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant105.py          # every mutant
    python3 tests/m0carried/plant105.py 0 3      # by number

`read_repo`'s call to `shed_reply` is not planted: it needs a request context, which only the wasm build has, so no
native test reaches it.
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
API = 'workers/edge/src/api.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'api::tests::an_agent_write_whose_rows_were_shed']
RULE = "    if write && shed && class == Some(graftyard_git::group::ClassWire::Agent) {\n"

MUTANTS = [
    ("no change: the shed stays 503 for an Agent write", API, RULE, "    if false {\n", T),
    ("every class's shed write is 429, a plain PAT's too (GHES invariant)", API, RULE, "    if write && shed {\n", T),
    ("a Bot's shed write is 429 too", API, RULE,
     "    if write && shed && class.is_some_and(|c| c != graftyard_git::group::ClassWire::Human) {\n", T),
    ("an Agent's shed read is 429 too", API, RULE,
     "    if shed && class == Some(graftyard_git::group::ClassWire::Agent) {\n", T),
    ("any Agent write's 503 is 429, not only a shed's", API, RULE,
     "    if write && r.status == 503 && class == Some(graftyard_git::group::ClassWire::Agent) {\n", T),
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
    print(f'caught {caught} of {len(pick)}')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
