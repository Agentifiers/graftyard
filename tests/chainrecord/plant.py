#!/usr/bin/env python3
"""chain-record: planted bugs in which sites a network's composed-miss chain buckets range over
(crates/git/src/shard.rs, `chain_buckets`; decisions/DECISIONS.md, "`chain-record`"). Plants one mutant at a time, runs
the tests that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE.
Mutant 0 is main's rule. The three callers (the site script's `Compose::run` and miss path, and the edge's `chain_r2`)
run only in wasm; `chain_buckets` takes the record, so none can pass the install's sites, and the next hosted row
checks them by bucket b's or c's chain-pack reads before the kill. Run from the worktree root:

    python3 tests/chainrecord/plant.py             # every mutant
    python3 tests/chainrecord/plant.py 2           # by number
    python3 tests/chainrecord/plant.py --residue   # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
SHARD = 'crates/git/src/shard.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', '--', 'chain']
BUILD = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', '--no-run']
ENV = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '.worktrees/target-am1')),
           PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')

TAIL = '\n    if a == b { vec![a] } else { vec![a, b] }'
RULE = '    let [a, b] = warm_sites(network, record.home, &record.sites);' + TAIL
# graft-m1's install: four sites, a to d.
INSTALL = '[SiteId(0), SiteId(1), SiteId(2), SiteId(3)]'
MUTANTS = [
    ("main's rule: the install's sites, not the record's", SHARD, RULE,
     f'    let [a, b] = warm_sites(network, record.home, &{INSTALL});' + TAIL),
    ("the record's sites, but the install's when the record names fewer than it", SHARD, RULE,
     f'    let s: &[SiteId] = if record.sites.len() < 4 {{ &{INSTALL} }} else {{ &record.sites }};\n'
     '    let [a, b] = warm_sites(network, record.home, s);' + TAIL),
    ("the record's first site stands for its home", SHARD, RULE,
     '    let [a, b] = warm_sites(network, record.sites[0], &record.sites);' + TAIL),
    ("the record's home and its next site in record order, with no rendezvous", SHARD, RULE,
     '    let _ = network;\n'
     '    let (a, b) = (record.home, *record.sites.iter().find(|s| **s != record.home).unwrap_or(&record.home));' + TAIL),
]


def residue():
    out = []
    for n, (name, f, old, _) in enumerate(MUTANTS):
        c = open(os.path.join(ROOT, f)).read().count(old)
        if c != 1:
            out.append(f'{n} {name}: anchor found {c} times')
    return out


def run():
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
            r = run()
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
