#!/usr/bin/env python3
"""lock-split: planted bugs in the git cache shard's split of the network lock (decisions/DECISIONS.md, "`lock-split`";
containers/git/server/main.go): the additive writers run beside validates and readers, so a validate's answer rests on
moveIn's rename order, its one ref snapshot, /refs' refsLock, adopt's claim, the `.keep` before the index, the lock alone
for a /refs set that renames between file and directory, and the stock answer's hold deadline. Plants one mutant at a
time, runs the tests that gate it, and restores the file byte for byte. Each line prints CAUGHT with the failing tests,
MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/locksplit/plant.py             # every mutant
    python3 tests/locksplit/plant.py 0 3         # by number
    python3 tests/locksplit/plant.py --residue   # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GO = 'containers/git/server/main.go'
G = ['go', 'test', '-count=1', '-run',
     'TestValidateSerialEquivalence|TestSlowStockReaderReleasesTheLock|TestSpooled|TestLockWait|TestRefs', '.']

KEEP = '\t\tif err := os.WriteFile(dst+".keep", nil, 0o644); err != nil {\n\t\t\treturn err\n\t\t}\n'
VISIBLE = '\t\t\tpackVisible(rp)\n\t\t}\n'
CLAIM = ('\tclaimed := filepath.Join(rp, "graft", fmt.Sprintf("adopting-%s-%d", sum, adoptSeq.Add(1)))\n'
         '\tnow := time.Now()\n'
         '\tos.Chtimes(src, now, now) // the sweep\'s hour for the claim starts now\n'
         '\tif err := os.Rename(src, claimed); err != nil {\n\t\treturn "", 0, false, nil\n\t}\n\tsrc = claimed\n')

# The design's fifth plant, moveIn renaming the .idx first, is not here: git 2.56 adds a pack from its .idx only when
# the .pack is already there (packfile.c, add_packed_git), so an .idx alone reads as no pack, the state before the
# move, and no reader can tell the orders apart [measured with git 2.56.0: cat-file --batch-check and rev-list both
# answer missing with the .idx in and the .pack not]. decisions/DECISIONS.md, "`lock-split`", says so.
MUTANTS = [
    ('validate walks --all, not its snapshot', [
        ('strings.NewReader(in+snap.not()), "rev-list", "--objects", "--quiet", "--stdin")',
         'strings.NewReader(in), "rev-list", "--objects", "--quiet", "--stdin", "--not", "--all")'),
        ('strings.NewReader(u.New+"\\n"+snap.not()), "rev-list", "--stdin")',
         'nil, "rev-list", u.New, "--not", "--all")'),
        ('strings.NewReader(u.New+"\\n"+snap.not()), "rev-list", fmt.Sprintf("--max-count=%d", commitsMax+1), "--stdin")',
         'nil, "rev-list", fmt.Sprintf("--max-count=%d", commitsMax+1), u.New, "--not", "--all")')]),
    ('/refs without refsLock', [
        ('\trl.Lock()\n\tdefer rl.Unlock()\n\tlockWaitMs := time.Since(w0).Milliseconds()\n',
         '\t_ = rl\n\tlockWaitMs := time.Since(w0).Milliseconds()\n')]),
    ('adopt without its claim rename', [(CLAIM, '')]),
    ("the .keep written after the index's rename", [
        (KEEP + '\t\tif _, err := os.Stat(dst + ".pack"); err != nil {', '\t\tif _, err := os.Stat(dst + ".pack"); err != nil {'),
        (VISIBLE, VISIBLE + KEEP)]),
    ('a file/directory /refs set without the lock alone', [
        ('\tif dirFile(dels, ups) {\n\t\tl.RUnlock()\n\t\tl.Lock()\n\t\tunlock = l.Unlock\n\t}\n', '')]),
    ('a stock answer holds the lock to its end', [
        ('\theld := time.AfterFunc(stockHoldMax, func() { release() })\n',
         '\theld := time.AfterFunc(stockHoldMax, func() {})\n')]),
]


def residue():
    s = open(os.path.join(ROOT, GO)).read()
    bad = []
    for n, (name, reps) in enumerate(MUTANTS):
        for old, _ in reps:
            if s.count(old) != 1:
                bad.append(f'{n:2} {name}: anchor found {s.count(old)} times')
    return bad


def main():
    if sys.argv[1:2] == ['--residue']:
        bad = residue()
        print('\n'.join(bad) or f'no plant residue: {len(MUTANTS)} mutants, each anchor found once')
        sys.exit(1 if bad else 0)
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    p = os.path.join(ROOT, GO)
    for i in pick:
        name, reps = MUTANTS[i]
        orig = open(p, 'rb').read()
        s = orig.decode()
        if any(s.count(old) != 1 for old, _ in reps):
            print(f'{i:2} NOT FOUND  {name}', flush=True)
            continue
        for old, new in reps:
            s = s.replace(old, new)
        open(p, 'w').write(s)
        try:
            r = subprocess.run(G, cwd=os.path.join(ROOT, 'containers/git/server'), capture_output=True, text=True,
                               timeout=900)
        finally:
            open(p, 'wb').write(orig)
        out = r.stdout + r.stderr
        if '[build failed]' in out or 'undefined:' in out:
            print(f'{i:2} DID NOT COMPILE  {name}', flush=True)
        elif r.returncode != 0:
            caught += 1
            fails = [l.split()[2] for l in out.splitlines() if l.startswith('--- FAIL')]
            why = next((l.strip() for l in out.splitlines() if '_test.go:' in l), '')
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]  {why[:160]}', flush=True)
        else:
            print(f'{i:2} MISSED  {name}', flush=True)
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
