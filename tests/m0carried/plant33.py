#!/usr/bin/env python3
"""learners-kept: planted bugs in config changes that drop the committed learners or commit from a stale read
(`decisions/DECISIONS.md`, "`learners-kept`"). Plants one mutant at a time, runs the test that guards it, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant33.py          # every mutant
    python3 tests/m0carried/plant33.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
MEMBER = 'workers/site/src/member.rs'
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--', 'keeps_the_committed_learners']

MUTANTS = [
    ('as on main: Degraded from the first read, Normal from the try', MEMBER,
     '            np.with_learners(c.learners()).ok().map(|np| (i, *c, np))', '            Some((i, *p.clone(), np))', SITE),
    ('the try\'s placement commits with its own learners', MEMBER,
     '            np.with_learners(c.learners()).ok().map(|np| (i, *c, np))', '            Some((i, *c, np))', SITE),
    ('voters that moved during the tries are not checked', MEMBER,
     '        Some((i, raft::GroupConfig::Normal(c))) if c.voters() == p.voters() && c.home() == p.home() => {',
     '        Some((i, raft::GroupConfig::Normal(c))) => {', SITE),
    ('Normal is checked against the first read, not Degraded', MEMBER,
     'raft::GroupConfig::Normal(Box::new(np)), degraded_at).await', 'raft::GroupConfig::Normal(Box::new(np)), based_on).await', SITE),
    ('a stale change is appended', MEMBER,
     '    if at == Some(based_on) { Ok(()) }', '    if at.is_some() { Ok(()) }', SITE),
    # `move_placement` takes the committed placement's `next` now, not `Placement::new` with its learners.
    ('a move drops the learners', MEMBER,
     '    p.next(voters, p.learners().iter().cloned().collect(), home, checks).map_err(|e| e.to_string())',
     '    p.next(voters, arrayvec::ArrayVec::new(), home, checks).map_err(|e| e.to_string())', SITE),
    ('a move out of Joint keeps the old half\'s learners', MEMBER,
     '        raft::GroupConfig::Joint { new, .. } => new,\n    };\n    p.next(voters,',
     '        raft::GroupConfig::Joint { old, .. } => old,\n    };\n    p.next(voters,', SITE),
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=900)
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
