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
     '        for p in self.peers() {\n            if !self.read_beat(p) {',
     '        for p in self.targets() {\n            if !self.read_beat(p) {', RAFT),
    # read-round-gap (plant20) replaced the round shared per clock reading with `free`: a waiting read joins the round
    # out until it is answered or READ_ROUND_GAP passes. Main's rule, a round per read, is a round whenever one waits.
    ('no round shared: one per read, to the voters', LEADER,
     '        if !waiting || !free {', '        if !waiting {', RAFT),
    # Distinct from plant20's "a read that arrived after the latest round never gets one": here the first round is
    # the only one, so every later read waits on it.
    ('a round shared with every later read', LEADER,
     'last.is_none_or(|(t, o)| own > o && (own', 'last.is_none_or(|(t, o)| false && (own', RAFT),
    # Equivalent since read-round-gap: `check_reads`, called right after, ends in `read_round` too. So the read is
    # planted as one that never asks for a round, which is what the arrival's call guarded.
    ('no round for a read on an expired lease', LEADER,
     '        lead.reads.push((id, now, !lease));', '        lead.reads.push((id, now, false));', RAFT),
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
