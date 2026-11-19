#!/usr/bin/env python3
"""b034-next: planted bugs in B0-34 mt3's two fixes: a repository with no `Settings` row reads `refs/heads/main` as its
default branch in one place both HEADs use (`graftyard_git::tiers::default_branch`; the member's `ranges` read and the
edge's ls-refs read), and an upload-pack write's deadline that releases the network's lock behind a caller that
stopped reading (`flushOnWrite`, containers/git/server/compose.go). Plants one mutant at a time, runs the tests that
gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the
worktree root:

    python3 tests/b034next/plant.py            # every mutant
    python3 tests/b034next/plant.py 0 3        # by number
    python3 tests/b034next/plant.py --residue  # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
TI = 'crates/git/src/tiers.rs'
LS = 'crates/git/src/ls_refs.rs'
FAM = 'workers/site/src/family.rs'
CG = 'containers/git/server/compose.go'
S = ['-p', 'graftyard-site', '--lib', 'the_ranges_read']
G = ['go', 'go', 'test', '-count=1', '-run', 'TestStalledCloneReleasesTheLock|TestCancelledCloneReleasesTheLock', '.']

MUTANTS = [
    ("main's rule: no row, no default branch", FAM,
     '        let default_branch = git::tiers::default_branch(&*fold.core).display();',
     '        let default_branch = git::tiers::get_rec::<git::issues::RepoSettings>(&*fold.core, "settings", &[])\n            .map(|s| s.default_branch.display());', S),
    ('the default is not main', TI,
     'RefName::parse(b"refs/heads/main").expect("a valid ref"), |s| s.default_branch)',
     'RefName::parse(b"refs/heads/master").expect("a valid ref"), |s| s.default_branch)', S),
    ('the row ignored: always main', TI,
     'RefName::parse(b"refs/heads/main").expect("a valid ref"), |s| s.default_branch)',
     'RefName::parse(b"refs/heads/main").expect("a valid ref"), |_| RefName::parse(b"refs/heads/main").unwrap())', S),
    ("the edge's ls-refs keeps its own rule: no HEAD without a row", LS,
     '        .then(|| crate::tiers::default_branch(&*fold.core))\n',
     '        .then(|| crate::tiers::get_rec::<crate::issues::RepoSettings>(&*fold.core, "settings", &[]).map(|s| s.default_branch))\n        .flatten()\n', S),
    # An ignored write or flush error is not planted: past the deadline the connection stays failed, so a later write
    # or the end of a finite pack still ends upload-pack and frees the lock; the deadline is what the test gates.
    ('no write deadline', CG,
     '\t_ = rc.SetWriteDeadline(time.Now().Add(clientWriteTimeout))\n', '\t_ = rc\n', G),
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
        print('\n'.join(bad) or 'every anchor found once')
        sys.exit(1 if bad else 0)
    picks = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for n in picks:
        name, path, old, new, t = MUTANTS[n]
        p = os.path.join(ROOT, path)
        orig = open(p, 'rb').read()
        s = orig.decode()
        assert s.count(old) == 1, f'{n} {name}: anchor found {s.count(old)} times'
        try:
            open(p, 'w').write(s.replace(old, new))
            code, out, broken = run(t)
        finally:
            open(p, 'wb').write(orig)
        if broken:
            print(f'{n:2} DID NOT COMPILE  {name}')
        elif code != 0:
            caught += 1
            fails = [l.split()[2] if l.startswith('--- FAIL') else l.split()[1] for l in out.splitlines() if l.startswith('--- FAIL') or l.startswith('test ') and l.endswith('FAILED')]
            print(f'{n:2} CAUGHT  {name}  {" ".join(fails[:3])}')
        else:
            print(f'{n:2} MISSED  {name}')
    print(f'{caught} of {len(list(picks))} caught')


if __name__ == '__main__':
    main()
