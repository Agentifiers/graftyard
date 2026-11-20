#!/usr/bin/env python3
"""chain-warm: planted bugs in how the edge checks a manifest's chain packs across the network's chain buckets
(decisions/DECISIONS.md, "`chain-warm`"; crates/git/src/compose.rs, `check_in_time`; crates/git/src/shard.rs,
`chain_buckets`). Plants one mutant at a time, runs the test that gates it, and restores the file byte for byte. Each
line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/chainwarm/plant.py             # every mutant
    python3 tests/chainwarm/plant.py --residue   # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
COMPOSE = 'crates/git/src/compose.rs'
SHARD = 'crates/git/src/shard.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', 'chain']
ENV = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '.worktrees/target-am1')),
           PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')

LOOP = """        for b in buckets {
            if let Some(v) = open_in_order(opens(b)).await? {
                return Ok(Some((b, v)));
            }
        }
        Ok(None)"""
MUTANTS = [
    # main's rule: the home bucket alone.
    ("main's rule: the home bucket alone", COMPOSE, 'for b in buckets {', 'for b in buckets.into_iter().take(1) {'),
    # the first bucket's decline taken as the answer.
    ('the first decline ends the checks', COMPOSE, LOOP,
     """        if let Some(b) = buckets.into_iter().next() {
            return Ok(open_in_order(opens(b)).await?.map(|v| (b, v)));
        }
        Ok(None)"""),
    # each bucket bounded on its own: a late home decline leaves the other a whole bound past the clone line.
    # no bound over the second bucket: a home that declines late leaves the other past the clone line.
    ('the bound on the home bucket only', COMPOSE,
     'Ok(match within(CHAIN_CHECK_WITHIN, first, sleep).await {',
     'let _ = &sleep;\n    Ok(match Some(first.await) {'),
    # the chain buckets as the home's alone, as the edge's clone cache was.
    ('chain buckets: home only', SHARD, 'if a == b { vec![a] } else { vec![a, b] }', 'let _ = b;\n    vec![a]'),
    # the other warm bucket first.
    ('chain buckets: other first', SHARD, 'if a == b { vec![a] } else { vec![a, b] }', 'if a == b { vec![a] } else { vec![b, a] }'),
]


def residue():
    out = []
    for n, (name, f, old, _) in enumerate(MUTANTS):
        c = open(os.path.join(ROOT, f)).read().count(old)
        if c != 1:
            out.append(f'{n} {name}: anchor found {c} times')
    return out


def main():
    if sys.argv[1:2] == ['--residue']:
        bad = residue()
        print('\n'.join(bad) or f'no plant residue: {len(MUTANTS)} anchors each found once')
        sys.exit(1 if bad else 0)
    caught = 0
    for i, (name, f, old, new) in enumerate(MUTANTS):
        p = os.path.join(ROOT, f)
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
