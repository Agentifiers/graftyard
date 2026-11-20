#!/usr/bin/env python3
"""F108: planted bugs in the member-alarm log line and the abandoned-transfer alarm (`decisions/OPEN-QUESTIONS.md`,
F108). Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant67.py          # every mutant
    python3 tests/m0carried/plant67.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
TRANSFER = 'crates/raft/src/member/transfer.rs'
HOST = 'workers/site/src/host.rs'
RAFT = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'leadership', '--', 'abandon']
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'a_member_alarm_line_names_its_group']
PUSH = '        self.out.alarms.push(Alarm::TransferAbandoned { to: t.to, step_down: t.step_down, why });\n'

MUTANTS = [
    ("an abandoned transfer logs nothing", TRANSFER, PUSH, '', RAFT),
    ("the LEASE path names the wrong cause", TRANSFER, '            self.abandon_transfer(AbandonWhy::Lease);',
     '            self.abandon_transfer(AbandonWhy::Refused);', RAFT),
    ("a refusal names the wrong cause", TRANSFER, '        self.abandon_transfer(AbandonWhy::Refused);',
     '        self.abandon_transfer(AbandonWhy::Lease);', RAFT),
    ("the lossy-link path is folded into the LEASE's", TRANSFER,
     '            self.abandon_transfer(AbandonWhy::LinkRefused);', '            self.abandon_transfer(AbandonWhy::Lease);',
     RAFT),
    ("the alarm reports the leader, not the target", TRANSFER, 'TransferAbandoned { to: t.to,',
     'TransferAbandoned { to: self.cfg.me,', RAFT),
    ("the line names no group", HOST, '    format!("member alarm {do_name} {do_id}: {a:?}")',
     '    format!("member alarm: {a:?}")', SITE),
    ("the line names the group but not its object id", HOST, '    format!("member alarm {do_name} {do_id}: {a:?}")',
     '    format!("member alarm {do_name}: {a:?}")', SITE),
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
