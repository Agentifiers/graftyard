#!/usr/bin/env python3
"""chain-wait: planted bugs in a composable miss's wait for its shard's chain (decisions/DECISIONS.md, "`chain-wait`";
crates/git/src/compose.rs, `chain_ready`). Plants one mutant at a time, runs the test that gates it, and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/chainwait/plant.py
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
RS = 'crates/git/src/compose.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', 'a_forwarded_miss']

WAIT = 'let wait = if follows { left.min(CHAIN_WAIT_FORWARDED) } else { left };'
MUTANTS = [
    # main's behaviour: every site waits the whole deadline.
    ('every site waits the whole deadline', WAIT, 'let wait = left;'),
    # the last site loses its deadline, so kill's case would send a miss nowhere.
    ('the last site waits only the bound', WAIT, 'let wait = left.min(CHAIN_WAIT_FORWARDED);'),
    # the bound overruns what is left of the upload-pack's deadline.
    ('the bound ignores the deadline', WAIT, 'let wait = if follows { CHAIN_WAIT_FORWARDED } else { left };'),
    # the rule inverted: the last site gets the bound and a forwarded miss the deadline.
    ('the rule inverted', WAIT, 'let wait = if !follows { left.min(CHAIN_WAIT_FORWARDED) } else { left };'),
    # a timeout read as ready, so the miss goes on to compose on a shard with no chain.
    ('a timeout counts as ready', 'futures_util::future::Either::Right(_) => false,',
     'futures_util::future::Either::Right(_) => true,'),
    # a ready chain read as late, so every miss answers warming.
    ('a ready chain counts as late', 'futures_util::future::Either::Left(_) => true,',
     'futures_util::future::Either::Left(_) => false,'),
    # a 20 s bound, COMPOSE_WAIT's value, which waits out the home's whole 15 s as main did.
    ('the bound at 20 s', 'std::time::Duration::from_secs(2);', 'std::time::Duration::from_secs(20);'),
]


def main():
    path = os.path.join(ROOT, RS)
    orig = open(path).read()
    bad = 0
    try:
        for name, old, new in MUTANTS:
            if orig.count(old) != 1:
                print(f'ANCHOR {name}: found {orig.count(old)} times'); bad += 1; continue
            open(path, 'w').write(orig.replace(old, new))
            r = subprocess.run(T, cwd=ROOT, capture_output=True, text=True)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                print(f'MISSED   {name}'); bad += 1
            elif 'error[' in out or 'could not compile' in out:
                print(f'DID NOT COMPILE {name}'); bad += 1
            else:
                print(f'CAUGHT   {name}')
    finally:
        open(path, 'w').write(orig)
    sys.exit(1 if bad else 0)


if __name__ == '__main__':
    main()
