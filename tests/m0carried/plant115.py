#!/usr/bin/env python3
"""M6-WP1, the recovery rule (`crates/release/src/group.rs`, `RestorePlacement`'s check;
`crates/release/src/ops.rs`, `auto_drain` and `auto_replace`): a `RestorePlacement` accepted while its site is
drained, a lost or replaced site undrained on an automatic drain's 15-minute calm instead of `RECOVER_AFTER`
recovered, and a `ZERO_ACKS` replace that signs on the held drain alone, without both other sites reading it silent.

Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant115.py          # every mutant
    python3 tests/m0carried/plant115.py 0 1      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GROUP = 'crates/release/src/group.rs'
OPS = 'crates/release/src/ops.rs'
REL_T = (['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', '--', 'tests::group::replace'], '.')

MUTANTS = [
    ("RestorePlacement is accepted while its site is drained", GROUP,
     "                if self.drained.contains_key(&site.0) {\n                    return Err(refuse(\"RestorePlacement is refused",
     "                if self.drained.contains_key(&site.0) && false {\n                    return Err(refuse(\"RestorePlacement is refused",
     REL_T),
    ("a lost or replaced site undrains on calm, not recovered", OPS,
     "        !v.burning && if loss { v.recovered } else { v.calm }\n",
     "        !v.burning && if loss { v.calm } else { v.calm }\n", REL_T),
    ("ZERO_ACKS of drain alone signs a replace, silent or not", OPS,
     "            d.since.0.saturating_add(crate::ZERO_ACKS) <= w.now.0 && w.health.get(s).is_some_and(|v| v.silent)\n",
     "            d.since.0.saturating_add(crate::ZERO_ACKS) <= w.now.0 && w.health.get(s).is_none_or(|v| v.silent || true)\n",
     REL_T),
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
