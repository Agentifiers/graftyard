#!/usr/bin/env python3
"""b034-keep: planted bugs in the spooled promotion's `.keep` order (`ingestSpooled`, containers/git/server/main.go): the
`.keep` must exist before the index's rename makes the pack visible to the lock-free geometric repack. Plants one mutant
at a time, runs the test that gates it, and restores the file byte for byte. Each line prints CAUGHT with the failing
tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/b034keep/plant.py             # every mutant
    python3 tests/b034keep/plant.py --residue   # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GO = 'containers/git/server/main.go'
G = ['go', 'test', '-count=1', '-run', 'TestSpooled', '.']
KEEP = '\t\tif err := os.WriteFile(dst+".keep", nil, 0o644); err != nil {\n\t\t\treturn err\n\t\t}\n'
VISIBLE = '\t\t\tpackVisible(rp)\n\t\t}\n'

MUTANTS = [
    # main's order before b034-keep: the `.keep` written after the pack and its index are in.
    ("the .keep written after the index's rename", GO, KEEP + '\t\tif _, err := os.Stat(dst + ".pack"); err != nil {',
     '\t\tif _, err := os.Stat(dst + ".pack"); err != nil {', VISIBLE, VISIBLE + KEEP),
    ('no .keep at all', GO, KEEP + '\t\tif _, err := os.Stat(dst + ".pack"); err != nil {',
     '\t\tif _, err := os.Stat(dst + ".pack"); err != nil {', None, None),
]


def residue():
    s = open(os.path.join(ROOT, GO)).read()
    bad = []
    for n, (name, _, old, _, old2, _) in enumerate(MUTANTS):
        for a in (old, old2):
            if a is not None and s.count(a) != 1:
                bad.append(f'{n:2} {name}: anchor found {s.count(a)} times')
    return bad


def main():
    if sys.argv[1:2] == ['--residue']:
        bad = residue()
        print('\n'.join(bad) or f'no plant residue: {len(MUTANTS)} mutants, each anchor found once')
        sys.exit(1 if bad else 0)
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, old2, new2 = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1 or (old2 is not None and s.count(old2) != 1):
            print(f'{i:2} NOT FOUND  {name}', flush=True)
            continue
        s = s.replace(old, new)
        if old2 is not None:
            s = s.replace(old2, new2)
        open(p, 'w').write(s)
        try:
            r = subprocess.run(G, cwd=os.path.join(ROOT, 'containers/git/server'), capture_output=True, text=True, timeout=600)
        finally:
            open(p, 'wb').write(orig)
        out = r.stdout + r.stderr
        if '[build failed]' in out or 'undefined:' in out:
            print(f'{i:2} DID NOT COMPILE  {name}', flush=True)
        elif r.returncode != 0:
            caught += 1
            fails = [l.split()[2] for l in out.splitlines() if l.startswith('--- FAIL')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]', flush=True)
        else:
            print(f'{i:2} MISSED  {name}', flush=True)
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
