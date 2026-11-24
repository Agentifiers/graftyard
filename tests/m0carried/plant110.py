#!/usr/bin/env python3
"""builder-install, option A: planted bugs in the builder's chain step after a restart (`crates/git/src/compose.rs`,
`builder_step`; `decisions/DECISIONS.md`, "`builder-install`"): main's rule (step only, so a new container builds a
base), an install over a chain the container already holds, and a failed install that leaves no step. Plants one
mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED,
or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant110.py          # every mutant
    python3 tests/m0carried/plant110.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
CO = 'crates/git/src/compose.rs'
SH_T = (['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard'], '.')

MUTANTS = [
    ("main's rule: the builder only steps, so a new container builds a base", CO,
     "    if io.held().await?.is_none()\n        && let Ok(Some(head)) = io.bucket_head().await\n",
     "    if false\n        && let Ok(Some(head)) = io.bucket_head().await\n", SH_T),
    ("the bucket head is installed over the chain the container holds", CO,
     "    if io.held().await?.is_none()\n",
     "    if io.held().await.is_ok()\n", SH_T),
    ("a failed install ends the step with no chain", CO,
     "        let _ = io.install(&head).await;\n",
     "        if !io.install(&head).await? {\n            return Ok(None);\n        }\n", SH_T),
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
