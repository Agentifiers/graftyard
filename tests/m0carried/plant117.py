#!/usr/bin/env python3
"""M6-WP1, the spare's refill order (`crates/git/src/sweep.rs`, `refill`): the multi-copy packs before the
single-copy ones, a fill read from the lost site's bucket, and a pack the spare already holds copied again.

Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant117.py          # every mutant
    python3 tests/m0carried/plant117.py 0 1      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
SW = 'crates/git/src/sweep.rs'
SW_T = (['cargo', 'test', '-q', '-p', 'graftyard-git', '--lib', '--', 'a_refill_copies'], '.')

MUTANTS = [
    ("multi-copy packs first", SW,
     "    single.extend(rest);\n    single\n", "    let mut rest = rest;\n    rest.extend(single);\n    rest\n", SW_T),
    ("the lost bucket is a fill source", SW,
     "        let mut views: Vec<BucketView> = views.iter().filter(|v| v.site != lost).cloned().collect();\n",
     "        let mut views: Vec<BucketView> = views.to_vec();\n", SW_T),
    ("the spare's own listing is ignored, so held packs are copied again", SW,
     "        if !views.iter().any(|v| v.site == spare) {\n",
     "        views.retain(|v| v.site != spare);\n        {\n", SW_T),
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
