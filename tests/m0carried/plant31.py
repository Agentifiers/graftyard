#!/usr/bin/env python3
"""probe-known: planted bugs in `create`'s skip for a known group, its generation check, and a behind follower's
answer (`decisions/DECISIONS.md`, "`probe-known`"). Plants one mutant at a time, runs the lookup crate's tests, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant31.py          # every mutant
    python3 tests/m0carried/plant31.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LOOKUP = 'crates/lookup/src/lib.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-lookup']

MUTANTS = [
    ('any cached config counts as known, whatever its generation', LOOKUP,
     '!e.voters.is_empty() && e.generation >= record.generation', '!e.voters.is_empty()', TESTS),
    ('a config is never known: the probe runs on every create', LOOKUP,
     'KNOWN.with(|k| k.borrow().get(&chain).is_some_and(|e| !e.voters.is_empty() && e.generation >= record.generation))',
     'false', TESTS),
    ('learn keeps no generation: every config reads as generation 0', LOOKUP,
     '            e.generation = generation;\n', '', TESTS),
    ('a behind follower clears the leader again', LOOKUP,
     '    if !behind {\n        learn_leader(chain, v);', '    if !behind || true {\n        learn_leader(chain, v);', TESTS),
    ('a leaderless pass is no longer reported to every voter', LOOKUP,
     '            suspect(t.clone(), term, true).await;', '            suspect(t.clone(), term, false).await;', TESTS),
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
