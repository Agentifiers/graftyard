#!/usr/bin/env python3
"""M6-WP3, Tokens 0x11.5 (`decisions/DECISIONS.md`, "`lifeboat-revoke`"): planted bugs in a lifeboat enroll's
revocation of the developer's earlier lifeboat tokens (`crates/identity/src/tokens.rs`). Plants one mutant at a time,
runs the test that guards it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT
COMPILE. Run from the repository root:

    python3 tests/m0carried/plant180.py          # every mutant
    python3 tests/m0carried/plant180.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
TOK = 'crates/identity/src/tokens.rs'
TOK_T = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--test', 'lifeboat']

MUTANTS = [
    ("an enroll revokes no earlier token", TOK,
     "                    .filter(|(_, a)| *a == account)\n",
     "                    .filter(|(_, a)| *a == account && false)\n", TOK_T),
    ("a LifeboatRevoked row leaves the token readable", TOK,
     "        TokensEvent::LifeboatRevoked { hash } => m.lifeboat().del(hash),\n",
     "        TokensEvent::LifeboatRevoked { .. } => {}\n", TOK_T),
    ("an enroll revokes every lifeboat token of the shard, not only the developer's", TOK,
     "                    .filter(|(_, a)| *a == account)\n",
     "                    .filter(|(_, _a)| true)\n", TOK_T),
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
        print(f'{i:2} {verdict}  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')
    sys.exit(0 if caught == len(list(pick)) else 1)


if __name__ == '__main__':
    main()
