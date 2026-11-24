#!/usr/bin/env python3
"""`warm-order` (B0-34 mt22's every-clone collapse): planted bugs in the order a validate or a read tries a network's
sites (`workers/edge/src/lib.rs`, `validate_sites`; `decisions/DECISIONS.md`, "`warm-order`"): main's record order,
which put mt22's reads after the kill of site a on site b with no chain, the warm site sorted last, the warm site picked
by rendezvous with the record's first site as home, and the warm site put ahead of the home. Plants one mutant at a
time, runs the test that guards it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT
COMPILE. Run from the repository root:

    python3 tests/m0carried/plant161.py          # every mutant
    python3 tests/m0carried/plant161.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
ED = 'workers/edge/src/lib.rs'
ED_T = (['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'a_validate_falls_back_from_the_home_shard'], '.')
SORT = "    others.sort_by_key(|s| *s != warm);\n"
WARM = "    let [_, warm] = graftyard_git::warm_sites(network, record.home, &record.sites);\n"
CHAIN = "    std::iter::once(record.home).chain(others).collect()\n}\n\n/// After a shard"

MUTANTS = [
    ("main's rule: the other Full sites in record order", ED, SORT, "", ED_T),
    ("the warm site tried last", ED, SORT, "    others.sort_by_key(|s| *s == warm);\n", ED_T),
    ("the warm site picked with the record's first site as home", ED, WARM,
     "    let [_, warm] = graftyard_git::warm_sites(network, record.sites[0], &record.sites);\n", ED_T),
    ("the warm site tried before the home", ED, CHAIN,
     "    others.insert(0, record.home);\n    others.sort_by_key(|s| *s != warm);\n    others\n}\n\n/// After a shard",
     ED_T),
]

def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, (cmd, cwd) = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=os.path.join(ROOT, cwd), capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out or '[build failed]' in out:
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
