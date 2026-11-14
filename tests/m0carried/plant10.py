#!/usr/bin/env python3
"""liveness-split-vote: planted bugs in the pre-vote grant rules (`pre_granted`, and dropping
its own pre-vote),
`tests/m0/results/split-vote.md`. Plants one mutant at a time, runs the raft tests in release, and restores the file
byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant10.py          # every mutant
    python3 tests/m0carried/plant10.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
ELECTION = 'crates/raft/src/member/election.rs'
RAFT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft']

MUTANTS = [
    ('a granted pre-vote does not hold back a campaign', ELECTION,
     ' || self.now < self.pre_granted.1 {', ' {'),
    ('the wait never ends', ELECTION, 'self.pre_granted = (req.term, after(self.now, CALL_DEADLINE));',
     'self.pre_granted = (req.term, Timestamp(u64::MAX));'),
    ('a grant sets no wait', ELECTION, 'self.pre_granted = (req.term, after(self.now, CALL_DEADLINE));', ''),
    ('a zero wait', ELECTION, 'self.pre_granted = (req.term, after(self.now, CALL_DEADLINE));',
     'self.pre_granted = (req.term, self.now);'),
    ('every grant extends the wait', ELECTION,
     '            if self.pre_granted.0 != req.term {\n                self.pre_granted = (req.term, after(self.now, CALL_DEADLINE));\n            }',
     '            self.pre_granted = (req.term, after(self.now, CALL_DEADLINE));'),
    ('a voter keeps its own pre-vote out after granting a rival', ELECTION,
     '            if self.campaign.as_ref().is_some_and(|c| c.pre()) {\n                self.lose_campaign();\n            }\n', ''),
    ('a voter drops a real campaign, not only a pre-vote', ELECTION,
     'if self.campaign.as_ref().is_some_and(|c| c.pre()) {', 'if self.campaign.is_some() {'),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            r = subprocess.run(RAFT, cwd=ROOT, capture_output=True, text=True, timeout=1800)
        finally:
            open(p, 'wb').write(orig)
        out = r.stdout + r.stderr
        if 'error[E' in out or 'could not compile' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
