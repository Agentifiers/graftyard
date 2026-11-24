#!/usr/bin/env python3
"""M6-WP1, the format check on the spare (`crates/release/src/ops.rs`, `auto_replace`): a `ReplaceMember` signed onto
a spare whose `SiteRelease` cannot read an enabled format, by skipping the check and by checking only the first
enabled format.

Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant116.py          # every mutant
    python3 tests/m0carried/plant116.py 0 1      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
OPS = 'crates/release/src/ops.rs'
REL_T = (['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', '--', 'tests::group::replace'], '.')

MUTANTS = [
    ("no format check on the spare", OPS,
     "    if let Some(f) = fold.enabled.iter().find(|f| !reads.is_some_and(|r| r.contains(f))) {\n",
     "    if let Some(f) = fold.enabled.iter().find(|f| false && !reads.is_some_and(|r| r.contains(f))) {\n", REL_T),
    ("only the first enabled format is checked", OPS,
     "    if let Some(f) = fold.enabled.iter().find(|f| !reads.is_some_and(|r| r.contains(f))) {\n",
     "    if let Some(f) = fold.enabled.iter().take(1).find(|f| !reads.is_some_and(|r| r.contains(f))) {\n", REL_T),
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
