#!/usr/bin/env python3
"""agent-cmds-entry: planted bugs in the leader's cap of Agent commands per entry (`decisions/DECISIONS.md`,
"`agent-cmds-entry`"; B0-32 m3fgt1's entries of 191 and 160 agent commands). Plants one mutant at a time, runs the
tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Mutant 0
puts main's behaviour back, so the new test fails on main. Run from the repository root:

    python3 tests/m0carried/plant62.py          # every mutant
    python3 tests/m0carried/plant62.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
ADMIT = 'crates/raft/src/admit.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'admission']
SHARE = '        while taken[AdmitClass::Agent.lane()] < AGENT_CMDS_ENTRY as u32\n            && ad.tokens >= 1.0'
FLOOR = 'n < floor.min(AGENT_CMDS_ENTRY)'
END = '            (None, Some(_)) if taken[AdmitClass::Agent.lane()] >= AGENT_CMDS_ENTRY as u32 => CutEnd::AgentCmds,\n'

MUTANTS = [
    ("main's rule: no cap on agent commands", LEADER, SHARE, '        while ad.tokens >= 1.0', TESTS),
    ("the cap counts only the share loop, the floor loop adds more", LEADER, SHARE,
     '        while taken[AdmitClass::Agent.lane()] < (AGENT_CMDS_ENTRY + n) as u32\n            && ad.tokens >= 1.0',
     TESTS),
    ("off by one: 65 per entry", LEADER, SHARE,
     '        while taken[AdmitClass::Agent.lane()] <= AGENT_CMDS_ENTRY as u32\n            && ad.tokens >= 1.0', TESTS),
    ("the floor loop ignores the cap", LEADER, FLOOR, 'n < floor', TESTS),
    ("a cut the cap ended is not recorded", LEADER, END, '', TESTS),
    ("the cap is 128", ADMIT, 'pub const AGENT_CMDS_ENTRY: usize = 64;', 'pub const AGENT_CMDS_ENTRY: usize = 128;', TESTS),
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
