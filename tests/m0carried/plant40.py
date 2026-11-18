#!/usr/bin/env python3
"""merge-tail: planted bugs in the answered-leader cache and the cut row's packing (`tests/m0/results/merge-tail.md`).
Plants one mutant at a time, runs the tests that should catch it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant40.py          # every mutant
    python3 tests/m0carried/plant40.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LOOKUP = 'crates/lookup/src/lib.rs'
HOST = 'workers/site/src/host.rs'
LK = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib']
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'host::tests::a_cut_row']

MUTANTS = [
    ("main's rule: a cached leader only from the config's voters", LOOKUP,
     '        .or(k.answered.filter(|t| t.site == leader))\n', '', LK),
    ('the answering member never kept', LOOKUP,
     '                    e.answered = Some(t.clone());\n', '', LK),
    ('an answered leader kept for any site, not the one the reply named', LOOKUP,
     '        .or(k.answered.filter(|t| t.site == leader))\n', '        .or(k.answered)\n', LK),
    ('the tokens back above bit 48 of `batch`', HOST,
     '    (peer, taken, left, ms)\n', '    (peer, taken | (n.tokens.max(0.0) as u64).min(0xffff) << 48, left, ms)\n', SITE),
    ('`left` at 16 bits a class, past 2^53', HOST,
     '    let left = (0..4).fold(0, |a, i| a | clamp(n.left[i], 13) << (13 * i));\n',
     '    let left = (0..4).fold(0, |a, i| a | clamp(n.left[i], 16) << (16 * i));\n', SITE),
    ('all bytes not clamped to their 21 bits', HOST,
     '    let ms = bytes(n.soft_bytes) | bytes(n.bytes) << 21 |', '    let ms = bytes(n.soft_bytes) | (n.bytes as u64) << 21 |', SITE),
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
