#!/usr/bin/env python3
"""G26: planted bugs in the shard's HEAD-from-default_branch fix: the container's `/refs` `head=` (`handleRefs`,
containers/git/server/main.go), the catch-up that names it (`crate::catchup`), and the member read that answers it
(`workers/site/src/family.rs`). Plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/g26/plant.py            # every mutant
    python3 tests/g26/plant.py 0 3        # by number
    python3 tests/g26/plant.py --residue  # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GO = 'containers/git/server/main.go'
CU = 'crates/git/src/catchup.rs'
FAM = 'workers/site/src/family.rs'
G = ['go', 'go', 'test', '-count=1', '-run', 'TestRefs', '.']
C = ['-p', 'graftyard-git', '--test', 'catchup']
S = ['-p', 'graftyard-site', '--lib', 'the_ranges_read']

MUTANTS = [
    ("main's rule: head= ignored", GO,
     '\t\thead := q.Get("head")\n\t\tif head == "" && put {', '\t\thead := ""\n\t\tif put {', G),
    ('head= ignored when it names a branch not created yet', GO,
     '\t\tsetHead(rp, head)\n\t\t// repack=0',
     '\t\tif _, err := git(rp, []string{derivedEnv}, nil, "rev-parse", "-q", "--verify", head); err == nil {\n\t\t\tsetHead(rp, head)\n\t\t}\n\t\t// repack=0', G),
    ('a change set never names HEAD', GO,
     '\tif !put && len(order) == 0 {\n\t\tsetHead(rp, q.Get("head"))', '\tif !put && len(order) == 0 {\n\t\t_ = q', G),
    ('the rehydrate passes no head', CU,
     'state.skipped.is_empty(), head_ref.as_deref()).await?;', 'state.skipped.is_empty(), None).await?;', C),
    ('a changed default never reaches a warm disk', CU,
     '    for (chain, head) in heads.into_iter().filter(|_| !behind) {\n',
     '    for (chain, head) in heads.into_iter().filter(|_| false) {\n', C),
    ('HEAD named on every catch-up', CU,
     '        let Some(h) = head.filter(|h| cursor.head_of(chain) != Some(h.as_str())) else { continue };\n',
     '        let Some(h) = head else { continue };\n', C),
    ('the cursor forgets the head a rehydrate set', CU,
     '        cursor.set_head(*chain, head.clone());\n', '', C),
    ('the head parameter not encoded', CU,
     '        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {', '        if true {', C),
    ('the member answers no default branch', FAM,
     '            "default_branch": default_branch,\n', '            "default_branch": Value::Null,\n', S),
]

ENV = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target', 'plants')), PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')


def run(t):
    cmd, cwd = (t[1:], os.path.join(ROOT, 'containers/git/server')) if t[0] == 'go' else (['cargo', 'test', '-q'] + t, ROOT)
    # A hang counts as caught (README, "Planted bugs").
    r = subprocess.run(['timeout', '-k', '10', '600', *cmd], cwd=cwd, env=ENV, capture_output=True, text=True)
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
            if code == 124:
                fails = ['(timed out)', *fails]
            print(f'{n:2} CAUGHT  {name}  {" ".join(fails[:3])}')
        else:
            print(f'{n:2} MISSED  {name}')
    print(f'{caught} of {len(list(picks))} caught')


if __name__ == '__main__':
    main()
