#!/usr/bin/env python3
"""held-grant: planted bugs in how a leader answers a read whose output waited for a deferred transaction
(`decisions/DECISIONS.md`, "`held-grant`"). Plants one mutant at a time, runs the test that guards it, and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant49.py          # every mutant
    python3 tests/m0carried/plant49.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
HOST = 'workers/site/src/host.rs'
LEADER = 'crates/raft/src/member/leader.rs'
TEST = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'a_read_released_after_later_inputs']

MUTANTS = [
    ("main's rule: the released view is the grant's own input only", HOST,
     '    m.fresh_after(g)\n}\n', '    m.fresh(g)\n}\n', TEST),
    ('the release resolves a read through `fresh` directly', HOST,
     '                                released_view(&core.m, &g).map(|f| {\n',
     '                                core.m.fresh(&g).map(|f| {\n', TEST),
    ('a grant from before a restart still answers', LEADER,
     '        if grant.boot != self.boot {\n            return Err(self.not_leader());\n        }\n'
     '        Ok(Fresh { fold: &self.fold.fold, index: self.fold.applied() })\n',
     '        Ok(Fresh { fold: &self.fold.fold, index: self.fold.applied() })\n', TEST),
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
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
