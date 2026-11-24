#!/usr/bin/env python3
"""builder-install, option B: planted bugs in a composable miss's chain wait on a shard that holds no chain
(`crates/git/src/compose.rs`, `chain_ready`; `decisions/DECISIONS.md`, "`builder-install`"): main's rule (the 2 s bound
whatever the shard holds), the last site answering at once too, so a miss runs out of sites that would wait, and a
shard that holds a chain sending its forwarded misses on at once. Plants one
mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED,
or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant111.py          # every mutant
    python3 tests/m0carried/plant111.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
CO = 'crates/git/src/compose.rs'
SH_T = (['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard'], '.')

MUTANTS = [
    ("main's rule: a forwarded miss waits the bound whatever the shard holds", CO,
     "        (true, true) if !holds => std::time::Duration::ZERO,\n",
     "        (true, true) if !holds && false => std::time::Duration::ZERO,\n", SH_T),
    ("the last site with no chain answers at once too", CO,
     "        (true, true) if !holds => std::time::Duration::ZERO,\n",
     "        (true, _) if !holds => std::time::Duration::ZERO,\n", SH_T),
    ("a shard that holds a chain sends its forwarded misses on at once", CO,
     "        (true, true) if !holds => std::time::Duration::ZERO,\n",
     "        (true, true) if !holds || holds => std::time::Duration::ZERO,\n", SH_T),
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
