#!/usr/bin/env python3
"""lookup-share: planted bugs in the edge's shared leader lookup and its `suspect` damping
(`tests/m0/results/m41-held.md`, "The lookup fix"). Plants one mutant at a time, runs the lookup crate's tests, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant29.py          # every mutant
    python3 tests/m0carried/plant29.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LOOKUP = 'crates/lookup/src/lib.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-lookup']

MUTANTS = [
    ('no turn: every request runs its own lookup, as on main', LOOKUP,
     '            if turn.is_none() && cached.as_ref().is_none_or(|l| l.do_name != name) {',
     '            if false && turn.is_none() && cached.as_ref().is_none_or(|l| l.do_name != name) {', TESTS),
    ('the turn is given up at the end of each pass', LOOKUP,
     '        forget_leader(chain);\n        delay(Duration::from_millis(250)).await;',
     '        forget_leader(chain);\n        turn = None;\n        delay(Duration::from_millis(250)).await;', TESTS),
    ('a waiter that finds the leader cached still looks it up itself', LOOKUP,
     '                if let Some(l) = cached_leader(chain).filter(|l| !tried.contains(&l.do_name)) {',
     '                if let Some(l) = cached_leader(chain).filter(|_| false) {', TESTS),
    ('waiters are never woken when the turn is dropped', LOOKUP,
     '        for w in LOOKING.with(|l| l.borrow_mut().remove(&self.0)).unwrap_or_default() {\n            w.wake();',
     '        for w in LOOKING.with(|l| l.borrow_mut().remove(&self.0)).unwrap_or_default() {\n            drop(w);', TESTS),
    ('suspect damping off: every report goes out', LOOKUP,
     '            Some(at) if now < *at + gap => false,', '            Some(_) => true,', TESTS),
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
