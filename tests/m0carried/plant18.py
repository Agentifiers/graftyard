#!/usr/bin/env python3
"""read-rounds: planted bugs in the leader's shared read round (`tests/m0/results/read-rounds.md`). Plants one
mutant at a time, runs the raft tests in release, and restores the file byte for byte. Each line prints CAUGHT,
MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant18.py          # every mutant
    python3 tests/m0carried/plant18.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
RAFT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft']

MUTANTS = [
    ('a round per read, to every target, as on main', LEADER,
     '        if !lease {\n            self.read_round();\n        }\n',
     '        if !lease {\n            self.broadcast(true);\n        }\n', RAFT),
    ('the read round goes to learners too', LEADER,
     '        for p in self.peers() {\n            self.send_append(p, true);\n        }\n    }\n\n    fn check_reads',
     '        for p in self.targets() {\n            self.send_append(p, true);\n        }\n    }\n\n    fn check_reads', RAFT),
    ('no round shared: one per read, to the voters', LEADER,
     '        if lead.read_round == Some(now) {\n            return;\n        }\n',
     '', RAFT),
    ('a round shared with every later read', LEADER,
     '        if lead.read_round == Some(now) {',
     '        if lead.read_round.is_some() {', RAFT),
    ('no round for a read on an expired lease', LEADER,
     '        if !lease {\n            self.read_round();\n        }\n',
     '', RAFT),
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
