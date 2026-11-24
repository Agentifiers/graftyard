#!/usr/bin/env python3
"""heap-count: planted bugs in the site's counting allocator (`workers/site/src/heapcount.rs`; `decisions/DECISIONS.md`,
"`heap-count`"). Live heap bytes follow every allocation, reallocation and free, and the peak holds across reads.
Plants one mutant at a time, runs the tests, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID
NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant152.py          # every mutant
    python3 tests/m0carried/plant152.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
T = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'heapcount']
H = 'workers/site/src/heapcount.rs'

MUTANTS = [
    ('dealloc does not subtract', H,
     '        with(|c| c.shrink(l.size()));\n        unsafe { System.dealloc(p, l) }',
     '        unsafe { System.dealloc(p, l) }'),
    ('realloc counts the new size as a new allocation', H,
     '            with(|c| c.moved(l.size(), new));', '            with(|c| c.grow(new));'),
    ('realloc is not counted', H,
     '            with(|c| c.moved(l.size(), new));', ''),
    ('a shrinking realloc is not subtracted', H,
     'if new >= old { self.grow(new - old) } else { self.shrink(old - new) }',
     'if new >= old { self.grow(new - old) }'),
    ('a read resets the peak', H,
     '    with(|c| (c.live.get(), c.peak.get())).unwrap_or((0, 0))',
     '    with(|c| (c.live.get(), c.peak.replace(c.live.get()))).unwrap_or((0, 0))'),
    ('the peak is not raised past itself', H,
     '        self.peak.set(self.peak.get().max(live));',
     '        if self.peak.get() == 0 {\n            self.peak.set(live);\n        }'),
    ('a zeroed allocation is not counted', H,
     '        let p = unsafe { System.alloc_zeroed(l) };\n        if !p.is_null() {\n            with(|c| c.grow(l.size()));\n        }',
     '        let p = unsafe { System.alloc_zeroed(l) };'),
]

ENV = dict(os.environ, CARGO_INCREMENTAL='0')


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, F, old, new = MUTANTS[i]
        p = os.path.join(ROOT, F)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        # As tests/prefix/plant.py: the original stays a hard link and the mutant goes in by rename, so a full disk
        # never leaves the source empty.
        keep, tmp = p + '.plant-orig', p + '.plant-new'
        os.link(p, keep)
        try:
            with open(tmp, 'w') as t:
                t.write(s.replace(old, new))
            os.replace(tmp, p)
            r = subprocess.run(T, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=3600)
        finally:
            if os.path.exists(tmp):
                os.remove(tmp)
            os.replace(keep, p)
            if open(p, 'rb').read() != orig:
                sys.exit(f'{F} did not restore: run `git diff -- {F}` before anything else')
        out = r.stdout + r.stderr
        if 'error[E' in out or 'could not compile' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            print(f'{i:2} CAUGHT  {name}: {next((l for l in out.splitlines() if "panicked" in l), "")[:160]}')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')
    sys.exit(0 if caught == len(list(pick)) else 1)


if __name__ == '__main__':
    main()
