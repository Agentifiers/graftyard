#!/usr/bin/env python3
"""F88's golden-digest corpus and the `MetaSlim` pin: planted changes to what a committed event folds to (`decisions/DECISIONS.md`,
"F88: a committed entry folds by its own format"). Plants one mutant at a time, runs the corpus and the in-memory
reproduction, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the
repository root:

    python3 tests/m0carried/plant40.py          # every mutant
    python3 tests/m0carried/plant40.py 0        # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
TIERS = 'crates/git/src/tiers.rs'
CORPUS = ['cargo', 'test', '-q', '--no-fail-fast', '-p', 'graftyard-git', '--test', 'fold_corpus', '--test', 'encoding']
SPLIT = ['cargo', 'test', '-q', '--no-fail-fast', '-p', 'graftyard-git', '--test', 'split', 'a_slim_pass_folds']

MUTANTS = [
    ("roll-up's unconditional promoted_log write (e2bddc92)", TIERS,
     '                if format >= REPO_FORMAT_ROLLUP {\n'
     '                    put_rec(w, "promoted_log", vec![Val::Int(p.from.0 as i64)], p);\n'
     '                }\n',
     '                let _ = (format, REPO_FORMAT_ROLLUP);\n'
     '                put_rec(w, "promoted_log", vec![Val::Int(p.from.0 as i64)], p);\n'),
    ('a 32.25 Promoted that no longer logs its promotion', TIERS,
     '                if format >= REPO_FORMAT_ROLLUP {\n',
     '                if format > REPO_FORMAT_ROLLUP {\n'),
    ('a closed pull request that keeps no final head (an old event, a new write)', TIERS,
     '            RepoEvent::PrClosed { n, final_head } => close_core(w, *n, PrState::Closed { at, final_head: *final_head }),',
     '            RepoEvent::PrClosed { n, .. } => close_core(w, *n, PrState::Closed { at, final_head: ObjectId([0; 20]) }),'),
    ('a lease renewal that rounds its expiry up to the second (a write the release changes)', TIERS,
     '            RepoEvent::LeaseRenew { id, expires } => crate::lease::write_renew(w, *id, *expires),',
     '            RepoEvent::LeaseRenew { id, expires } => crate::lease::write_renew(w, *id, Timestamp(expires.0.div_ceil(1_000) * 1_000 + 1_000)),'),
    ("a MetaSlim back on the 512-row rule before SLIM_BYTES (a0e4d7bc's in-place change, reversed)",
     'crates/git/src/split.rs',
     'pub const SLIM_BYTES: usize = 128 << 10;', 'pub const SLIM_BYTES: usize = usize::MAX;', SPLIT),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i][:4]
        cmd = MUTANTS[i][4] if len(MUTANTS[i]) > 4 else CORPUS
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
        finally:
            open(p, 'wb').write(orig)
            os.remove(p + '.orig')
        out = r.stdout + r.stderr
        if 'error[E' in out or 'could not compile' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            moved = sorted({l.split(':')[0].strip() for l in out.splitlines() if ': the fold moved' in l})
            print(f'{i:2} CAUGHT  {name}  [{", ".join(moved[:4])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
