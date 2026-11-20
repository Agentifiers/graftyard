#!/usr/bin/env python3
"""chain-open: planted bugs in how the edge's composed answer opens its chain packs (decisions/DECISIONS.md,
"`chain-open`"; crates/git/src/compose.rs, `open_in_order`). Plants one mutant at a time, runs the test that gates it,
and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/chainopen/plant.py             # every mutant
    python3 tests/chainopen/plant.py --residue   # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
RS = 'crates/git/src/compose.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', 'a_composed_answer_opens']
ENV = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '.worktrees/target-am1')),
           PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')

BUF = 'futures_util::stream::iter(opens).buffered(CHAIN_OPEN_AT_ONCE)'
MUTANTS = [
    # main's rule: each pack opened after the one before it.
    ("main's rule: one pack at a time", BUF, 'futures_util::stream::iter(opens).buffered(1)'),
    # packs handed on as they open, not in the manifest's order.
    ('packs out of order', BUF, 'futures_util::stream::iter(opens).buffer_unordered(CHAIN_OPEN_AT_ONCE)'),
    # every pack opened at once, past the six connections an invocation has.
    ('every pack at once', BUF, 'futures_util::stream::iter(opens).buffered(usize::MAX)'),
    # a missing pack skipped instead of declining the answer.
    ('a missing pack skipped', '            None => return Ok(None),\n', '            None => {}\n'),
]


def residue():
    s = open(os.path.join(ROOT, RS)).read()
    return [f'{n} {name}: anchor found {s.count(old)} times' for n, (name, old, _) in enumerate(MUTANTS)
            if s.count(old) != 1]


def main():
    if sys.argv[1:2] == ['--residue']:
        bad = residue()
        print('\n'.join(bad) or f'no plant residue: {len(MUTANTS)} anchors each found once')
        sys.exit(1 if bad else 0)
    p = os.path.join(ROOT, RS)
    caught = 0
    for i, (name, old, new) in enumerate(MUTANTS):
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i} NOT FOUND  {name}', flush=True)
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            r = subprocess.run(T, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1800)
        finally:
            open(p, 'wb').write(orig)
        out = r.stdout + r.stderr
        if 'error[E' in out or ('could not compile' in out and 'test failed' not in out):
            print(f'{i} DID NOT COMPILE  {name}', flush=True)
        elif r.returncode != 0:
            caught += 1
            print(f'{i} CAUGHT  {name}', flush=True)
        else:
            print(f'{i} MISSED  {name}', flush=True)
    print(f'{caught} of {len(MUTANTS)} caught')


if __name__ == '__main__':
    main()
