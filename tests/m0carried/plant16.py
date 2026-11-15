#!/usr/bin/env python3
"""learner-reset: planted bugs in the leader's reset of per-peer state for a new member at an address
(`tests/m0/results/learner-reset.md`). Plants one mutant at a time, runs the tests that guard it in release, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant16.py          # every mutant
    python3 tests/m0carried/plant16.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
RAFT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft', '--test', 'scenarios']

MUTANTS = [
    ('voters only, as on main', LEADER,
     '.flat_map(|p| p.voters().iter().filter(|m| voters.contains(&m.site)).chain(p.learners()))',
     '.flat_map(|p| p.voters().iter().filter(|m| voters.contains(&m.site)))', RAFT),
    ('a new address only, not a new name at an old one', LEADER,
     'now.iter().filter(|(s, n)| was.get(*s) != Some(*n))',
     'now.iter().filter(|(s, _)| !was.contains_key(*s))', RAFT),
    ('next reset, matched kept', LEADER,
     '            lead.matched.insert(p, Index(0));\n',
     '', RAFT),
    ('a late answer from the old member counts for the new one', LEADER,
     'if res.round < lead.min_round || lead.member_from.get(&from).is_some_and(|r| res.round < *r) {',
     'if res.round < lead.min_round {', RAFT),
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
