#!/usr/bin/env python3
"""learner-hold (F85): planted bugs in how a leader ends a learner call held for its batch's read-index answer
(`decisions/DECISIONS.md`, "`learner-hold`"). Plants one mutant at a time, runs the site crate's callers tests, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant35.py          # every mutant
    python3 tests/m0carried/plant35.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
CALLERS = 'workers/site/src/callers.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'callers']

MUTANTS = [
    ('take records no answers as left: main\'s hold for its own response', CALLERS,
     '        self.left(from, &answers);\n', '        let _ = answers;\n', TESTS),
    ('left wakes no waiting call', CALLERS,
     '            c.left.pop_front();\n        }\n        for w in c.wake.drain(..) {\n            let _ = w.send(());\n        }',
     '            c.left.pop_front();\n        }', TESTS),
    ('answered ignores answers that left', CALLERS,
     'c.held.iter().any(is_answer) || c.left.contains(&batch)', 'c.held.iter().any(is_answer)', TESTS),
    ('any answer that left ends every hold', CALLERS,
     'c.held.iter().any(is_answer) || c.left.contains(&batch)', 'c.held.iter().any(is_answer) || !c.left.is_empty()', TESTS),
    ('left is never trimmed', CALLERS,
     '        while c.left.len() > LEFT_KEPT {', '        while c.left.len() > usize::MAX - 1 {', TESTS),
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
