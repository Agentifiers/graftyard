#!/usr/bin/env python3
"""stats-bytes: planted bugs in memSplit's `stats`, the heap the host's `Stats` sample arrays hold
(`workers/site/src/host.rs`, `Stats::sample_bytes`; `tests/am1/results/archive-memory.md`, "The census of the leader's
uncounted heap"). The count is each array's capacity times its element size, matches what the counting allocator sees
while the arrays fill and after `reset`, and leaves no array out.
Plants one mutant at a time, runs the tests, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID
NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant154.py          # every mutant
    python3 tests/m0carried/plant154.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
T = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--features', 'test-faults', '--', 'stats_bytes']
H = 'workers/site/src/host.rs'

MUTANTS = [
    ('length, not capacity', H,
     '        fn v<T>(x: &Vec<T>) -> usize {\n            x.capacity() * size_of::<T>()',
     '        fn v<T>(x: &Vec<T>) -> usize {\n            x.len() * size_of::<T>()'),
    ('elements, not bytes', H,
     '        fn v<T>(x: &Vec<T>) -> usize {\n            x.capacity() * size_of::<T>()',
     '        fn v<T>(x: &Vec<T>) -> usize {\n            x.capacity()'),
    ('every element priced as a u32', H,
     '        fn v<T>(x: &Vec<T>) -> usize {\n            x.capacity() * size_of::<T>()',
     '        fn v<T>(x: &Vec<T>) -> usize {\n            x.capacity() * 4'),
    ('the maps\' arrays left out', H,
     '        let mut samples = maps\n', '        let mut samples = 0 * maps\n'),
    ('the call times left out', H,
     '            + self.calls.values().map(|c| v(&c.ms)).sum::<usize>();', '            ;'),
    ('the queue splits left out', H,
     '            + v(&self.queue_slot_ms)\n            + v(&self.queue_post_ms)\n', ''),
    ('the read stages left out', H,
     '            + v(&self.read_stages)\n', ''),
    ('the trace ring left out', H,
     '            (q(&self.trace), q(&self.elections))', '            (0, q(&self.elections))'),
    ('the trace ring by length', H,
     '        fn q<T>(x: &VecDeque<T>) -> usize {\n            x.capacity() * size_of::<T>()',
     '        fn q<T>(x: &VecDeque<T>) -> usize {\n            x.len() * size_of::<T>()'),
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
