#!/usr/bin/env python3
"""repack-keep: planted bugs in the shard's post-rehydrate repack (`containers/git/server/main.go`: the `.keep` a
promotion's stored-`.idx` ingest writes, `unkeptPacks`, `runRepack`'s bounds, `gitLow`, and `/procs`),
`tests/am1/results/lm1-memory.md`, rounds 25 and 27 (`--keep-pack`). Plants one mutant at a time, runs the shard server's Go tests, and
restores the file byte for byte. Not planted: `--pack-kept-objects` on the geometric repack, which git ignores with
`--geometric` (the kept packs stayed byte for byte under it), so it is the same program. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant17.py          # every mutant
    python3 tests/m0carried/plant17.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
MAIN = 'containers/git/server/main.go'
GO = ['go', 'test', '-count=1', '-run',
      'TestRehydratesRepackLeavesPromotedPacks|TestRepackRunsLowAndProcsListsIt|TestRefsRepacksPastRepackAt|'
      'TestIngestWritesAMultiPackIndexEveryMidxEveryPacks|TestSpooled|TestConnected|TestRepackHoldsNoNetworkLock',
      './...']

MUTANTS = [
    ('a promotion loaded through its .idx is not kept', MAIN,
     '\t\tif err := os.WriteFile(dst+".keep", nil, 0o644); err != nil {\n\t\t\treturn err\n\t\t}\n', ''),
    ('the threshold counts kept promotions', MAIN,
     '\tif unkeptPacks(rp) <= repackAt || !repacking.TryLock() {',
     '\tif packCount(rp) <= repackAt || !repacking.TryLock() {'),
    ('a .keep is ignored by the count (counts every pack)', MAIN,
     '\t\tif _, err := os.Stat(strings.TrimSuffix(f, ".pack") + ".keep"); err != nil {',
     '\t\tif _, err := os.Stat(strings.TrimSuffix(f, ".pack") + ".kept"); err != nil {'),
    ('no multi-pack-index after the repack', MAIN,
     '"repack", "-d", "-q", "--geometric=2", "--write-midx"}',
     '"repack", "-d", "-q", "--geometric=2"}'),
    ('the repack on all four threads', MAIN,
     '"-c", "pack.threads=2", "-c", "pack.windowMemory=256m", "repack"',
     '"-c", "pack.threads=4", "-c", "pack.windowMemory=256m", "repack"'),
    ('no bound on the delta window', MAIN,
     '"-c", "pack.threads=2", "-c", "pack.windowMemory=256m", "repack"',
     '"-c", "pack.threads=2", "repack"'),
    ('gitLow at normal priority', MAIN,
     'args ...string) ([]byte, error) {\n\treturn runGit(exec.Command("nice", append([]string{"-n", "19", "git"}, args...)...), nil,',
     'args ...string) ([]byte, error) {\n\treturn runGit(exec.Command("git", args...), nil,'),
    ('the repack through git, not gitLow', MAIN,
     '\t_, err := gitLow(rp, []string{derivedEnv}, args...)',
     '\t_, err := git(rp, []string{derivedEnv}, nil, args...)'),
    ('/procs reads no nice value', MAIN,
     '\t\t\t\tfmt.Sscan(f[16], &p.Nice) // field 19', '\t\t\t\t_ = f[16]'),
    ('/procs misses git run through nice (matches the parent only)', MAIN,
     '\t\tif filepath.Base(args[0]) != "git" && !strings.HasPrefix(filepath.Base(args[0]), "git-") {',
     '\t\tif filepath.Base(args[0]) != "nice" {'),
    ('/procs?net= counts every pack as kept', MAIN,
     '\t\t\t\tif _, err := os.Stat(strings.TrimSuffix(f, ".pack") + ".keep"); err == nil {\n\t\t\t\t\tkept++',
     '\t\t\t\t{\n\t\t\t\t\tkept++'),
    # Round 27: the kept packs named in core, so the roll's hint walk stops at them.
    ('no --keep-pack for the kept promotions', MAIN,
     '\t\targs = append(args, "--keep-pack="+strings.TrimSuffix(filepath.Base(k), ".keep")+".pack")\n', '\t\t_ = k\n'),
    ('--keep-pack names the .keep file, not the pack', MAIN,
     '"--keep-pack="+strings.TrimSuffix(filepath.Base(k), ".keep")+".pack"', '"--keep-pack="+filepath.Base(k)'),
    ('only the first kept pack named', MAIN,
     '\tfor _, k := range keeps {\n', '\tfor _, k := range keeps[:min(1, len(keeps))] {\n'),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i]
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
            r = subprocess.run(GO, cwd=os.path.join(ROOT, 'containers/git/server'), capture_output=True, text=True,
                               timeout=1800)
        finally:
            open(p, 'wb').write(orig)
            os.remove(p + '.orig')
        out = r.stdout + r.stderr
        if '[build failed]' in out or 'declared and not used' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            fails = [l.split()[2] for l in out.splitlines() if l.startswith('--- FAIL')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
