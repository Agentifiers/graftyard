#!/usr/bin/env python3
"""b034-tails: planted bugs in B0-34's tail fixes (`decisions/DECISIONS.md`, "`b034-tails`"): over-the-cap misses
retried after the chain's step, which builds a base on a far chain (`compose.go`, `handleCompose`); one deadline per
upload-pack (`graftyard_git::compose::compose_then_catch_up`); stock fetches capped at stockMax in the container and
queued in arrival order in the shard object (`stock_in_line`); and `/ingest`'s `midx_ms`. Plants one mutant at a
time, runs the tests that gate it, and restores the file byte for byte. Run from the worktree root:

    python3 tests/b034tails/plant.py          # every mutant
    python3 tests/b034tails/plant.py 0 3      # by number
    python3 tests/b034tails/plant.py --residue  # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GO = 'containers/git/server/main.go'
CG = 'containers/git/server/compose.go'
RS = 'crates/git/src/compose.rs'
R = ['-p', 'graftyard-git', '--test', 'shard', '--', 'a_miss_on', 'a_stuck', 'stock_waits']
G = ['go', 'go', 'test', '-count=1', '-run', 'TestStockFetchesPastStockMax|TestComposeStepOnAFarChain|TestIngestReportsTheMidx|TestComposedMiss$', '.']

MUTANTS = [
    ("main's rule: over the cap is not retried", RS,
     '    if !(over || why == Some(NOT_YET) && running) {', '    if !(why == Some(NOT_YET) && running) {', R),
    ('over the cap retried without the chain step', RS,
     '        if over {\n            step().await;\n        }\n', '', R),
    ('the step before the running catch-up ends', RS,
     '        if running {\n            in_flight().await;\n        }\n        if over {\n            step().await;\n        }\n',
     '        if over {\n            step().await;\n        }\n        if running {\n            in_flight().await;\n        }\n', R),
    ("main's rule: no deadline on the retry", RS,
     '        futures_util::future::Either::Right(_) => AfterCompose::Late,\n    }\n}',
     '        futures_util::future::Either::Right(_) => futures_util::future::pending().await,\n    }\n}', R),
    ('a later stock request jumps the line', RS,
     '            if line.borrow().is_head(t)\n', '            if true\n', R),
    ('stock asks at once though others wait', RS,
     '        if !line.borrow().busy()\n            && let Some(r) = call().await', '        if let Some(r) = call().await', R),
    ('a late stock request keeps its place', RS,
     '            if let Some(t) = self.1 {\n                self.0.borrow_mut().leave(t);\n            }',
     '            let _ = &self.1;', R),
    ("main's rule: no stock cap in the container", GO,
     '\t\tif stockRunning.Add(1) > stockMax {', '\t\tif stockRunning.Add(1) > 1<<30 {', G),
    ('a stock slot never freed', GO, '\t\tdefer stockRunning.Add(-1)\n', '', G),
    ('ls-refs takes a stock slot', GO,
     '\tif !advertise && !lsRefs(body) {', '\tif !advertise {', G),
    ("main's rule: a far chain's step is a part", CG,
     '\t\tif len(objs) > composeDiffMax {\n\t\t\tbase = true', '\t\tif false {\n\t\t\tbase = true', G),
    ('every step a base', CG,
     '\t\t} else {\n\t\t\tparts = objs\n\t\t}', '\t\t} else {\n\t\t\tparts = objs\n\t\t\tbase = true\n\t\t}', G),
    ('midx_ms dropped', GO,
     '\treturn max(time.Since(t0).Milliseconds(), 1)', '\treturn 0 * max(time.Since(t0).Milliseconds(), 1)', G),
]

ENV = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '.worktrees/target-am1')), PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')


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
