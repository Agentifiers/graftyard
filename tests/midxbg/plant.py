#!/usr/bin/env python3
"""midx-background: planted bugs in the background multi-pack-index write (decisions/DECISIONS.md, "`midx-background`";
containers/git/server/main.go, `midxAfterIngest`). Plants one mutant at a time, runs the Go tests that gate it, and
restores the file byte for byte. The last mutants plant the catch-up's page timing (`crates/git/src/catchup.rs`,
`PageMs`), which the same branch adds so the next row can tell which part of a long catch-up ran long. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/midxbg/plant.py             # every mutant
    python3 tests/midxbg/plant.py 0 3         # by number
    python3 tests/midxbg/plant.py --residue   # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GO = 'containers/git/server/main.go'
T = ['go', 'test', '-count=1', '-run',
     'TestAMidx|TestASecondMidx|TestReadersSee|TestRepackAndFreshInitWait|TestIngestReportsTheMidx|'
     'TestIngestWritesAMultiPackIndex|TestValidateSerialEquivalence', '.']

CU = 'crates/git/src/catchup.rs'
RT = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'catchup', 'a_catch_up_times']
ENV = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '.worktrees/target-am1')),
           PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')

GOROUTINE = '\tm.n = 0\n\tgo func() {\n\t\tdefer m.mu.Unlock()'
GO_MUTANTS = [
    # main's rule: the write runs in the ingest's writer section.
    ("main's rule: the write in the writer's section", GOROUTINE, '\tm.n = 0\n\tfunc() {\n\t\tdefer m.mu.Unlock()'),
    # a trigger beside a running write waits for it, then writes again: queued, not skipped.
    ('a second trigger queued behind the write', '\tif m.n < midxEvery || !m.mu.TryLock() {\n\t\treturn\n\t}',
     '\tif m.n < midxEvery {\n\t\treturn\n\t}\n\tm.mu.Lock()'),
    # a skipped trigger resets the count, so the next write waits another midxEvery ingests.
    ('a skipped trigger resets the count', '\tm.n++\n\tif m.n < midxEvery || !m.mu.TryLock() {',
     '\tm.n++\n\tif m.n >= midxEvery && m.mu.TryLock() {\n\t\tm.mu.Unlock()\n\t} else if m.n >= midxEvery {\n\t\tm.n = 0\n\t}\n'
     '\tif m.n < midxEvery || !m.mu.TryLock() {'),
    # the write removes the old index before it writes the new one: a reader sees neither.
    ('the old index removed before the write',
     '\t\tif _, err := git(rp, []string{derivedEnv}, nil, "multi-pack-index", "write"); err != nil {',
     '\t\tos.Remove(filepath.Join(rp, "objects", "pack", "multi-pack-index"))\n'
     '\t\tif _, err := git(rp, []string{derivedEnv}, nil, "multi-pack-index", "write"); err != nil {'),
    # evict deletes the repository under a running write.
    ('evict ignores a running write', '\tif m := midxOf(rp); !m.mu.TryLock() {', '\tif m := midxOf(rp); false && !m.mu.TryLock() {'),
    # a fresh init deletes the repository under a running write.
    ('a fresh init ignores a running write', '\t\tm := midxOf(rp)\n\t\tm.mu.Lock()\n\t\tos.RemoveAll(rp)\n\t\tos.RemoveAll(composeDir(net))\n\t\tm.mu.Unlock()',
     '\t\tos.RemoveAll(rp)\n\t\tos.RemoveAll(composeDir(net))'),
    # the repack runs beside a write, which then fails on git's lock or names a pack the repack deleted.
    ('the repack ignores a running write', '\t\tm := midxOf(rp)\n\t\tm.mu.Lock()\n\t\tdefer m.mu.Unlock()\n\t\tif _, err := os.Stat(rp); err != nil {',
     '\t\tif _, err := os.Stat(rp); err != nil {'),
    # midx_ms reported on every answer, not once.
    ('midx_ms never cleared', 'func midxMs(rp string) int64 { return midxOf(rp).ms.Swap(0) }',
     'func midxMs(rp string) int64 { return midxOf(rp).ms.Load() }'),
    # /ingest-batch drops midx_ms again, as mt7's did.
    ('/ingest-batch reports no midx_ms', '"lock_wait_ms": lockWaitMs, "midx_ms": midxMs(rp)}', '"lock_wait_ms": lockWaitMs}'),
]
MUTANTS = [(n, GO, o, w) for n, o, w in GO_MUTANTS] + [
    # catch_up's page timing (`PageMs`): a part counted in another, or not at all.
    ("main's rule: /refs counted with the ingest", CU, '            io.apply_refs(&changes, None, *repack).await?;\n            lap(&mut t.ms.refs);',
     '            io.apply_refs(&changes, None, *repack).await?;\n            lap(&mut t.ms.ingest);'),
    ('make_room untimed', CU, '    t.room -= bytes;\n    lap(&mut t.ms.room);', '    t.room -= bytes;\n    lap(&mut 0);'),
]


def run_go():
    r = subprocess.run(T, cwd=os.path.join(ROOT, 'containers/git/server'), capture_output=True, text=True, timeout=1800)
    out = r.stdout + r.stderr
    return r.returncode, out, '[build failed]' in out or 'syntax error' in out


def run_rust():
    r = subprocess.run(RT, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1800)
    out = r.stdout + r.stderr
    return r.returncode, out, 'error[E' in out or ('could not compile' in out and 'test failed' not in out)


def residue():
    bad = []
    for n, (name, f, old, _) in enumerate(MUTANTS):
        c = open(os.path.join(ROOT, f)).read().count(old)
        if c != 1:
            bad.append(f'{n:2} {name}: anchor found {c} times')
    return bad


def main():
    if sys.argv[1:2] == ['--residue']:
        bad = residue()
        print('\n'.join(bad) or f'no plant residue: {len(MUTANTS)} anchors each found once')
        sys.exit(1 if bad else 0)
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}', flush=True)
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            code, out, broke = run_go() if f == GO else run_rust()
        finally:
            open(p, 'wb').write(orig)
        if broke:
            print(f'{i:2} DID NOT COMPILE  {name}', flush=True)
        elif code != 0:
            caught += 1
            fails = [l.split()[2] for l in out.splitlines() if l.startswith('--- FAIL')] + \
                [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]', flush=True)
        else:
            print(f'{i:2} MISSED  {name}', flush=True)
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
