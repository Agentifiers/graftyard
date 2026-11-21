#!/usr/bin/env python3
"""transfer-why: planted bugs in the transfer-start alarm (`decisions/OPEN-QUESTIONS.md`, F108, and
`tests/am1/results/transfer-why.md`). Plants one mutant at a time, runs the tests that guard it, and restores the file
byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant72.py          # every mutant
    python3 tests/m0carried/plant72.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
TRANSFER = 'crates/raft/src/member/transfer.rs'
HEALTH = 'crates/raft/src/member/health.rs'
LEADER = 'crates/raft/src/member/leader.rs'
HOST = 'workers/site/src/host.rs'
RAFT = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'leadership', '--',
        'transfer_hands_leadership', 'a_refused_transfer', 'commit_errors_trip_and_the_probe',
        'leadership_returns_home', 'a_drained_site_hands_off', 'an_older_refuser']
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'a_member_alarm_line_names_its_group']
PUSH = '        self.out.alarms.push(Alarm::TransferStarted { to, why });\n'

MUTANTS = [
    ("a started transfer logs nothing", TRANSFER, PUSH, '', RAFT),
    ("the alarm names the leader, not the target", TRANSFER, 'Alarm::TransferStarted { to, why }',
     'Alarm::TransferStarted { to: self.cfg.me, why }', RAFT),
    ("the alarm is pushed before the checks, so a refused transfer logs a start", TRANSFER,
     '        let step_down = why == TransferWhy::StepDown;\n',
     '        let step_down = why == TransferWhy::StepDown;\n        self.out.alarms.push(Alarm::TransferStarted { to, why });\n',
     RAFT),
    ("/transfer names another cause", TRANSFER, '        let r = self.start_transfer(to, TransferWhy::Asked, true);',
     '        let r = self.start_transfer(to, TransferWhy::Home, true);', RAFT),
    ("a step-down names another cause", HEALTH, '        match self.start_transfer(to, TransferWhy::StepDown, false) {',
     '        match self.start_transfer(to, TransferWhy::Asked, false) {', RAFT),
    ("the move home names another cause", HEALTH, '            let _ = self.start_transfer(home, TransferWhy::Home, false);',
     '            let _ = self.start_transfer(home, TransferWhy::Asked, false);', RAFT),
    ("a drained hand-off is logged as no_lead", HEALTH,
     '            self.hand_off(if self.no_lead() { TransferWhy::NoLead } else { TransferWhy::Drained });',
     '            self.hand_off(TransferWhy::NoLead);', RAFT),
    ("a refusal's hand-off is logged as drained", LEADER, '            self.hand_off(TransferWhy::NoLead);\n            return;',
     '            self.hand_off(TransferWhy::Drained);\n            return;', RAFT),
    ("the trace row's cause codes shift", HOST, '        Asked => 5,', '        Asked => 4,', SITE),
    ("the line names no group", HOST, '    format!("member alarm {do_name} {do_id}: {a:?}")',
     '    format!("member alarm: {a:?}")', SITE),
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
