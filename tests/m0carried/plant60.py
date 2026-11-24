#!/usr/bin/env python3
"""step-agree: planted bugs in the hold that keeps a hot leader through its own `STEP_DOWN` trip
(`decisions/DECISIONS.md`, "`step-agree`"; B0-35 m60's term 3). Plants one mutant at a time, runs the tests that guard
it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Mutant 0 puts main's
behaviour back, so the hold test fails on main. Run from the repository root:

    python3 tests/m0carried/plant60.py          # every mutant
    python3 tests/m0carried/plant60.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
HEALTH = 'crates/raft/src/member/health.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'leadership', '--test', 'liveness']
# step-busy (later) widened `hot` to `hot || (errors_trip && busy)`; these mutants plant step-agree's half of it.
GUARD = '        if (self.hot() || (errors_trip && self.busy())) && !self.site_sick {\n'

MUTANTS = [
    ("main's rule: every trip moves the leader, hot or not", HEALTH, GUARD, '        if false && errors_trip {\n', TESTS),
    ("the other sites' verdict is ignored", HEALTH, GUARD,
     '        if self.hot() || (errors_trip && self.busy()) {\n', TESTS),
    ("every group holds, hot or not", HEALTH, GUARD,
     '        if (true || (errors_trip && self.busy())) && !self.site_sick {\n', TESTS),
    ("the hold is for a quiet group, not a hot one", HEALTH, GUARD,
     '        if (!self.hot() || (errors_trip && self.busy())) && !self.site_sick {\n', TESTS),
    ("the host's verdict is never stored", HEALTH, '        self.site_sick = sick;\n', '        let _ = sick;\n', TESTS),
    ("a held trip is a transfer to nobody, with no hold window", HEALTH,
     '            self.hold(why);\n            return;\n        }\n        let failing',
     '            self.out.alarms.push(Alarm::StepDown { why, to: None });\n            return;\n        }\n        let failing',
     TESTS),
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
