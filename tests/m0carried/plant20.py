#!/usr/bin/env python3
"""read-round-gap: planted bugs in the leader's read round, at most one per READ_ROUND_GAP until answered
(`tests/m0/results/read-round-gap.md`). Plants one mutant at a time, runs the raft tests in release, and restores
the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant20.py          # every mutant
    python3 tests/m0carried/plant20.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
RAFT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft']

MUTANTS = [
    ('a round per clock reading, no gap, as read-rounds did', LEADER,
     '        let free = last.is_none_or(|t| now >= after(t, READ_ROUND_GAP) || answered.is_some_and(|a| a >= t));\n',
     '        let free = last != Some(now);\n', RAFT),
    ('the gap ignored: a new round at every later reading', LEADER,
     'now >= after(t, READ_ROUND_GAP)', 'now > t', RAFT),
    ('the read round goes to learners too', LEADER,
     '        for p in self.peers() {\n            self.send_append(p, true);\n        }\n    }\n\n    fn check_reads',
     '        for p in self.targets() {\n            self.send_append(p, true);\n        }\n    }\n\n    fn check_reads', RAFT),
    ('no next round once the latest is answered', LEADER,
     '        self.lead.as_mut().expect("leading").reads = keep;\n        self.read_round();\n',
     '        self.lead.as_mut().expect("leading").reads = keep;\n', RAFT),
    ('an answer does not free the slot: only the gap does', LEADER,
     ' || answered.is_some_and(|a| a >= t));', ');', RAFT),
    ('a read that arrived after the latest round never gets one', LEADER,
     'last.is_none_or(|t| *arrived > t)', 'last.is_none()', RAFT),
    ('a read granted by a round that left before it arrived', LEADER,
     'let round_ok = round.is_some_and(|t| t >= arrived);', 'let round_ok = round.is_some();', RAFT),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        # A copy beside the file, so a run killed mid-mutant can be undone with `git checkout` or this copy.
        open(p + '.orig', 'wb').write(orig)
        open(p, 'w').write(s.replace(old, new))
        try:
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
        finally:
            open(p, 'wb').write(orig)
            os.remove(p + '.orig')
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
