#!/usr/bin/env python3
"""chain-install: planted bugs in which shards take the composed-miss chain's step after their own catch-up
(crates/git/src/compose.rs, `steps_after_catch_up`; decisions/DECISIONS.md, "`chain-install`"). Plants one mutant at a time, runs the test that gates it, and restores the file byte for byte. Each
line prints CAUGHT, MISSED, or DID NOT COMPILE. Mutant 0 is main's rule. The site script's one call to the rule (`Compose::run`) runs only in wasm and no
host test reaches it; the next hosted row checks it instead, by bucket c's chain-pack reads after a catch-up and
before the kill. Run from the worktree root:

    python3 tests/chaininstall/plant.py             # every mutant
    python3 tests/chaininstall/plant.py 2           # by number
    python3 tests/chaininstall/plant.py --residue   # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
COMPOSE = 'crates/git/src/compose.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', '--', 'a_shard_on_a_chain_site']
BUILD = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', '--no-run']
ENV = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '.worktrees/target-am1')),
           PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')

RULE = '    builder || crate::chain_buckets(network, record).contains(&site)'
MUTANTS = [
    ("main's rule, only the builder steps after a catch-up", COMPOSE, RULE,
     '    let _ = (network, record, site);\n    builder'),
    ('every shard installs after a catch-up, on a site with no chain too', COMPOSE, RULE,
     '    let _ = (network, record, site, builder);\n    true'),
    ('only the first chain bucket, the home, installs', COMPOSE, RULE,
     '    builder || crate::chain_buckets(network, record).first() == Some(&site)'),
    ('the home site stands for the chain sites', COMPOSE, RULE, '    let _ = network;\n    builder || site == record.home'),
]


def residue():
    out = []
    for n, (name, f, old, _) in enumerate(MUTANTS):
        c = open(os.path.join(ROOT, f)).read().count(old)
        if c != 1:
            out.append(f'{n} {name}: anchor found {c} times')
    return out


def run(f):
    subprocess.run(BUILD, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1800)
    return subprocess.run(T, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=120)


def main():
    if sys.argv[1:2] == ['--residue']:
        bad = residue()
        print('\n'.join(bad) or f'no plant residue: {len(MUTANTS)} anchors each found once')
        sys.exit(1 if bad else 0)
    only = {int(a) for a in sys.argv[1:]}
    caught = 0
    for i, (name, f, old, new) in enumerate(MUTANTS):
        if only and i not in only:
            continue
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i} NOT FOUND  {name}', flush=True)
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            r = run(f)
        except subprocess.TimeoutExpired:
            r = subprocess.CompletedProcess(T, 124, 'hung', '')
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
