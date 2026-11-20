#!/usr/bin/env python3
"""chain-hold: planted bugs in how the edge's composed answer checks and streams its chain packs (decisions/DECISIONS.md,
"`chain-hold`"; crates/git/src/compose.rs, `stream_in_turn` and `check_in_time`). Plants one mutant at a time, runs the
test that gates it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from
the worktree root:

    python3 tests/chainhold/plant.py             # every mutant
    python3 tests/chainhold/plant.py --residue   # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
RS = 'crates/git/src/compose.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', 'a_composed_answer_streams']
ENV = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '.worktrees/target-am1')),
           PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')

LOOP = """    for p in packs {
        let b = open(p).await?;
        send(b).await?;
    }
"""
MUTANTS = [
    # main's rule: every pack's body opened before the first is sent, and held to its turn.
    ("main's rule: every body opened, then sent", LOOP,
     """    let mut opened = Vec::new();
    for p in packs {
        opened.push(open(p).await?);
    }
    for b in opened {
        send(b).await?;
    }
"""),
    # the next pack opened while this one is still being sent: two bodies held.
    ('the next body opened ahead', LOOP,
     """    let mut next = None;
    for p in packs {
        let b = open(p).await?;
        if let Some(prev) = next.replace(b) {
            send(prev).await?;
        }
    }
    if let Some(b) = next {
        send(b).await?;
    }
"""),
    # no bound on the checks: a check that never answers holds the clone.
    ('checks with no bound', 'Ok(match within(CHAIN_CHECK_WITHIN, first, sleep).await {',
     'let _ = &sleep;\n    Ok(match Some(first.await) {'),
    # a late check declined to the shard, which waits on the same chain, instead of refused.
    ('a late check declined, not refused', '        None => ChainCheck::Late,', '        None => ChainCheck::Declined,'),
    # a bound past the clone line.
    ('the bound at 10 s', 'pub const CHAIN_CHECK_WITHIN: std::time::Duration = std::time::Duration::from_secs(3);',
     'pub const CHAIN_CHECK_WITHIN: std::time::Duration = std::time::Duration::from_secs(10);'),
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
