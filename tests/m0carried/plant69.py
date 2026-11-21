#!/usr/bin/env python3
"""F109: planted bugs in a slow member alarm's step line (`decisions/OPEN-QUESTIONS.md`, F109). Its four hedge mutants
went with `dep-hedge-other`'s reversal (`decisions/DECISIONS.md`, "`dep-hedge-other`"). Plants one
mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED,
or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant69.py          # every mutant
    python3 tests/m0carried/plant69.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
STEPS = 'workers/site/src/alarm_steps.rs'
HOST = 'workers/site/src/host.rs'
SITE_STEPS = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'a_slow_alarm_names_every_await']
SITE_LINE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'a_member_alarm_line_names_its_group']

MUTANTS = [
    ("an alarm of exactly ALARM_SLOW_MS logs nothing", STEPS, 'if self.total < ALARM_SLOW_MS {',
     'if self.total <= ALARM_SLOW_MS {', SITE_STEPS),
    ("the slow line leaves out the quick awaits", STEPS,
     'let steps: Vec<String> = self.steps.iter().map(|(s, ms)| format!("{s} {ms}ms")).collect();',
     'let steps: Vec<String> =\n            self.steps.iter().filter(|s| s.1 >= ALARM_STEP_LOG_MS).map(|(s, ms)| format!("{s} {ms}ms")).collect();',
     SITE_STEPS),
    ("the slow line names no group", HOST, 'format!("member {do_name} {do_id} {steps}")', 'format!("member {steps}")',
     SITE_LINE),
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
