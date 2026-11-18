#!/usr/bin/env python3
"""b034-fix: planted bugs in B0-34's two fixes, a pool warm answered from the slot's own state (`catchup_answer`) and a
composable miss on a just-moved tip composed again after the running catch-up (`compose_then_catch_up`). Plants one
mutant at a time, runs the tests that gate it, and restores the file byte for byte. Each line prints CAUGHT with the
failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/b034fix/plant.py          # every mutant
    python3 tests/b034fix/plant.py 0 3      # by number
    python3 tests/b034fix/plant.py --residue  # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
SHARD = 'crates/git/src/shard.rs'
COMPOSE = 'crates/git/src/compose.rs'
T = ['-p', 'graftyard-git', '--test', 'shard', '--test', 'agent_load']

MUTANTS = [
    ("main's rule: a pool warm waits for a catch-up", SHARD,
     '        Caller::PoolWarm if running && cursor && !rehydrating => CatchupAnswer::Warm,\n        Caller::PoolWarm => CatchupAnswer::Cold,\n',
     '', T),
    ('a cursor mid-rehydrate counts as warm', SHARD,
     'Caller::PoolWarm if running && cursor && !rehydrating =>', 'Caller::PoolWarm if running && cursor =>', T),
    ('a stopped container counts as warm', SHARD,
     'Caller::PoolWarm if running && cursor && !rehydrating =>', 'Caller::PoolWarm if cursor && !rehydrating =>', T),
    ('no cursor needed', SHARD,
     'Caller::PoolWarm if running && cursor && !rehydrating =>', 'Caller::PoolWarm if running && !rehydrating =>', T),
    ('every caller is answered at once', SHARD,
     '        _ => CatchupAnswer::WaitForCatchUp,', '        _ => CatchupAnswer::Warm,', T),
    ("main's rule: a 409 always waits for a later catch-up", COMPOSE,
     '    if why != Some(NOT_YET) || !running {', '    if true {', T),
    ('any reason is composed again', COMPOSE,
     '    if why != Some(NOT_YET) || !running {', '    if !running {', T),
    ('composed again with no catch-up running', COMPOSE,
     '    if why != Some(NOT_YET) || !running {', '    if why != Some(NOT_YET) {', T),
    ('composed again before the running catch-up ends', COMPOSE,
     '    in_flight().await;\n    match compose().await {', '    match compose().await {', T),
]

ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')


def run(t):
    r = subprocess.run(['cargo', 'test', '-q'] + t, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1800)
    out = r.stdout + r.stderr
    return r.returncode, out, 'error[E' in out or 'could not compile' in out and 'test failed' not in out


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
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]', flush=True)
        else:
            print(f'{i:2} MISSED  {name}', flush=True)
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
