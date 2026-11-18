#!/usr/bin/env python3
"""b034-reads: planted bugs in B0-34's read-path fixes: (b) an /ingest's index-pack in a quarantine under the network's
lock shared (`indexIngest`, containers/git/server/main.go), and (a) a v2 ls-refs answered at the edge from the group's
refs (`graftyard_git::ls_refs`). Plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/b034reads/plant.py          # every mutant
    python3 tests/b034reads/plant.py 0 3      # by number
    python3 tests/b034reads/plant.py --residue  # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GO = 'containers/git/server/main.go'
G = ['go', 'go', 'test', '-count=1', '-run', 'TestIngest|TestRecordKeepsOnlyThePacksOwnObjects|TestConcurrent|TestThinPack|TestSpooled', '.']

MUTANTS = [
    ("main's rule: index-pack under the lock alone", GO,
     '\tl.RLock()\n\tq, name, err := indexQuarantined(rp, file)\n\tl.RUnlock()\n',
     '\tl.Lock()\n\tq, name, err := indexQuarantined(rp, file)\n\tl.Unlock()\n', G),
    ("a thin pack's missing base answered 422, not 409", GO,
     '\tl.RUnlock()\n\tif err != nil {\n\t\tif unresolved(err) {\n\t\t\treturn 0, 0, 409, err',
     '\tl.RUnlock()\n\tif err != nil {\n\t\tif false {\n\t\t\treturn 0, 0, 409, err', G),
    ('the pack never moved in', GO,
     '\tif err := moveIn(q, rp, name); err != nil {\n\t\treturn 0, 0, 500, err\n\t}\n\tif err := record(', '\tif err := record(', G),
    ('no record of the pack\'s objects', GO,
     '\tif err := record(rp, sum, name, filepath.Join(rp, "objects"), st.Size()); err != nil {\n\t\treturn 0, 0, 500, err\n\t}\n\tmidxAfterIngest(net, rp)\n\treturn time.Since(t0)',
     '\t_ = st\n\tmidxAfterIngest(net, rp)\n\treturn time.Since(t0)', G),
    ('no multi-pack-index count after an ingest', GO,
     '\tmidxAfterIngest(net, rp)\n\treturn time.Since(t0).Milliseconds(), 0, 0, nil',
     '\treturn time.Since(t0).Milliseconds(), 0, 0, nil', G),
    ('the quarantine left behind', GO, '\tdefer os.RemoveAll(q)\n\tl.Lock()\n', '\tl.Lock()\n', G),
]

ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')


def run(t):
    cmd, cwd = (t[1:], os.path.join(ROOT, 'containers/git/server')) if t[0] == 'go' else (['cargo', 'test', '-q'] + t, ROOT)
    r = subprocess.run(cmd, cwd=cwd, env=ENV, capture_output=True, text=True, timeout=1800)
    out = r.stdout + r.stderr
    return r.returncode, out, 'error[E' in out or ('could not compile' in out and 'test failed' not in out) or '[build failed]' in out


def residue():
    bad = []
    for n, (name, path, old, _, _) in enumerate(MUTANTS):
        c = open(os.path.join(ROOT, path)).read().count(old)
        if c != 1:
            bad.append(f'{n:2} {name} ({path}): anchor found {c} times')
    return bad


def main():
    if sys.argv[1:2] == ['--residue']:
        bad = residue()
        print('\n'.join(bad) or f'no plant residue: {len(MUTANTS)} anchors each found once')
        sys.exit(1 if bad else 0)
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, t = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}', flush=True)
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            code, out, broke = run(t)
        finally:
            open(p, 'wb').write(orig)
        if broke:
            print(f'{i:2} DID NOT COMPILE  {name}', flush=True)
        elif code != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')] + [l.split()[2] for l in out.splitlines() if l.startswith('--- FAIL')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]', flush=True)
        else:
            print(f'{i:2} MISSED  {name}', flush=True)
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
