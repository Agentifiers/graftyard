#!/usr/bin/env python3
"""arrival-bytes: planted bugs in memSplit's `arrivals`, BF-14's in-memory arrival ring and its bytes
(`workers/site/src/host.rs`, `Arrivals`; `tests/am1/results/archive-memory.md`, "The census of the leader's uncounted
heap"). The count follows the ring's length, stops at its cap, and falls to 0 when `arrival=sql` clears it.
Plants one mutant at a time, runs the tests, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID
NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant153.py          # every mutant
    python3 tests/m0carried/plant153.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
T = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'arrival_bytes']
H = 'workers/site/src/host.rs'

MUTANTS = [
    ('the bytes count no rows', H,
     '        (n, n * ARRIVAL_ROW_BYTES)', '        (n, ARRIVAL_ROW_BYTES)'),
    ('the bytes count the cap, not the rows held', H,
     '        (n, n * ARRIVAL_ROW_BYTES)', '        (n, self.0.as_ref().map_or(0, |_| ARRIVALS_MEM_MAX) * ARRIVAL_ROW_BYTES)'),
    ('a full ring keeps growing', H,
     '        if mem.len() >= ARRIVALS_MEM_MAX {\n            mem.pop_front();\n        }\n', ''),
    ('a full ring drops its newest row', H,
     '            mem.pop_front();\n        }\n        mem.push_back(row);', '            mem.pop_back();\n        }\n        mem.push_back(row);'),
    ('arrival=sql leaves the ring', H,
     '            (false, _) => self.0 = None,', '            (false, _) => {}'),
    ('a second arrival=mem empties the ring', H,
     '            (true, false) => self.0 = Some(VecDeque::new()),', '            (true, _) => self.0 = Some(VecDeque::new()),'),
    ('sql mode keeps the row in memory', H,
     '        let Some(mem) = self.0.as_mut() else { return Some(row) };',
     '        let mem = self.0.get_or_insert_with(VecDeque::new);'),
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
