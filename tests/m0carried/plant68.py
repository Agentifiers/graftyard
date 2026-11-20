#!/usr/bin/env python3
"""step-busy: planted bugs in the hold that keeps a loaded leader through its own `CommitErrors` trip
(`decisions/DECISIONS.md`, "`step-busy`"; B0-33 m63r33's 19:06 leader change). Plants one mutant at a time, runs the
tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Mutant 0
puts main's behaviour back, so the hold test fails on main. Run from the repository root:

    python3 tests/m0carried/plant68.py          # every mutant
    python3 tests/m0carried/plant68.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
HEALTH = 'crates/raft/src/member/health.rs'
LEADER = 'crates/raft/src/member/leader.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'leadership', '--test', 'liveness']
GUARD = '        if (self.hot() || (errors_trip && self.busy())) && !self.site_sick {\n'

MUTANTS = [
    ("main's rule: only a hot group holds", HEALTH, GUARD, '        if self.hot() && !self.site_sick {\n', TESTS),
    ("busy holds without its CommitErrors scope, on every trip", HEALTH, GUARD,
     '        if (self.hot() || self.busy()) && !self.site_sick {\n', TESTS),
    ("busy ignores the backlog, so a quiet gray leader holds", HEALTH,
     '        committed >= HOLD_ENTRIES && !self.quiet(HOME_QUIET_QUEUE)',
     '        committed >= HOLD_ENTRIES', TESTS),
    ("commits are never noted", LEADER, '                self.note_commit(entries);\n', '', TESTS),
    ("the window's commits are never pruned, so old commits keep a stuck leader busy", HEALTH,
     '        while h.commits.front().is_some_and(|c| c.0 < since) {\n            h.commits.pop_front();\n        }\n',
     '', TESTS),
    ("the other sites' verdict is ignored for a busy leader", HEALTH, GUARD,
     '        if self.hot() && !self.site_sick || errors_trip && self.busy() {\n', TESTS),
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
