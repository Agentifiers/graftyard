#!/usr/bin/env python3
"""read-newest: planted bugs in a follower or learner's read-index batches (`decisions/DECISIONS.md`, "`read-newest`";
B0-35 m61's duplicate learner calls). Plants one mutant at a time, runs the tests that guard it, and restores the file
byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. The new test fails on main itself
(checked by restoring main's install.rs). Run from the repository root:

    python3 tests/m0carried/plant63.py          # every mutant
    python3 tests/m0carried/plant63.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
INSTALL = 'crates/raft/src/member/install.rs'
HOST = 'workers/site/src/host.rs'
RAFT = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'leadership', 'read_index']
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'tests::a_learner_hedges']
NEWEST = '        let resend = self.remote.inflight.iter_mut().max_by_key(|b| b.0)'

MUTANTS = [
    ("the oldest batch is resent, not the newest", INSTALL,
     NEWEST + '.filter(|b| now >= after(b.1, CALL_DEADLINE));',
     '        let resend = self.remote.inflight.iter_mut().min_by_key(|b| b.0).filter(|b| now >= after(b.1, CALL_DEADLINE));',
     RAFT),
    ("a later answer grants no earlier batch", INSTALL,
     '.into_iter().partition(|b| b.0 < batch);', '.into_iter().partition(|b| b.0 < batch && false);', RAFT),
    ("an answer grants later batches too", INSTALL,
     '.into_iter().partition(|b| b.0 < batch);', '.into_iter().partition(|b| b.0 != batch);', RAFT),
    ("the resend ignores CALL_DEADLINE", INSTALL,
     '.filter(|b| now >= after(b.1, CALL_DEADLINE));', '.filter(|b| now >= b.1);', RAFT),
    ("every read-index call is hedged", HOST,
     '    read.is_none_or(|b| b >= newest)', '    read.is_none_or(|b| b >= newest) || read.is_some()', SITE),
    ("no read-index call is hedged", HOST,
     '    read.is_none_or(|b| b >= newest)', '    read.is_none_or(|b| b > newest)', SITE),
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
