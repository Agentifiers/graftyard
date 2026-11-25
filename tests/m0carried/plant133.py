#!/usr/bin/env python3
"""`move-unmoved`: planted bugs in how a Workflows group ends when no leader answers it, and how the coordinator
reports it (`workers/workflows/move.mjs`; `decisions/DECISIONS.md`, "`move-unmoved`"; OPEN-QUESTIONS R62). Each runs
`node --test workers/workflows/tests/move.test.mjs`. Plants one mutant at a time and restores the file byte for byte.
Each line prints CAUGHT or MISSED. Run from the repository root:

    python3 tests/m0carried/plant133.py          # every mutant
    python3 tests/m0carried/plant133.py 0 1      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
WF = 'workers/workflows/move.mjs'
WF_T = ['node', '--test', 'workers/workflows/tests/move.test.mjs']

MUTANTS = [
    ("a group no leader answered still errors the coordinator", WF,
     "  if (led) throw new Error(`${p.chain}: not done after ${ASKS_MAX} asks`);\n",
     "  throw new Error(`${p.chain}: not done after ${ASKS_MAX} asks`);\n"),
    ("a group a leader answered ends unmoved instead of erroring", WF,
     "    if (r.step) led = true;\n",
     ""),
    ("an unmoved group pages nothing", WF,
     "    console.error(`PAGE graft-workflows: ${p.chain} unmoved: ${unmoved}`);\n",
     ""),
    ("the coordinator drops its unmoved list", WF,
     "    unmoved.push(...r.unmoved);\n",
     ""),
    ("a restore group whose voters name no leader asks none to campaign", WF,
     "    if (l === null || l === undefined || (lost !== null && l === lost)) return { campaign: true };\n",
     "    if (lost !== null && (l === null || l === undefined || l === lost)) return { campaign: true };\n"),
    ("the coordinator reads an errored group as unmoved", WF,
     "        if (s.status === \"errored\" || s.status === \"terminated\") {\n",
     "        if (s.status === \"errored\") { left.push({ chain: chains[i].chain, why: \"errored\" }); continue; }\n"
     "        if (s.status === \"terminated\") {\n"),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(WF_T, cwd=ROOT, capture_output=True, text=True, timeout=600)
            verdict = 'MISSED' if r.returncode == 0 else 'CAUGHT'
            caught += verdict == 'CAUGHT'
        finally:
            open(p, 'wb').write(orig)
        print(f'{i} {name}: {verdict}', flush=True)
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
