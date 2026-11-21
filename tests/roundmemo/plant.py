#!/usr/bin/env python3
"""round-memo: planted bugs in the `Notify` and `Search` batch rules (crates/git/src/inbox.rs, `notices`;
crates/git/src/search.rs, `docs_for`; decisions/DECISIONS.md, "`round-memo`"). Plants one mutant at a time, runs the
tests that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Mutants
0 and 1 are main's rule. Run from the worktree root:

    python3 tests/roundmemo/plant.py             # every mutant
    python3 tests/roundmemo/plant.py 2 3         # by number
    python3 tests/roundmemo/plant.py --residue   # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
INBOX = 'crates/git/src/inbox.rs'
SEARCH = 'crates/git/src/search.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'activity_gates']
BUILD = T + ['--no-run']
ENV = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target')),
           PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')

EACH = '''    for e in es {
        let mut by: BTreeMap<AccountId, Vec<Notice>> = BTreeMap::new();'''
MUTANTS = [
    ("main's rule: Notify reads each entry's thread again", INBOX, EACH,
     EACH.replace('    for e in es {\n', '    for e in es {\n        threads.clear();\n        subscribers = None;\n')),
    ("main's rule: Search builds a document for each entry that touches it", SEARCH,
     '        if let Some(n) = n\n            && !ns.contains(&n)\n        {', '        if let Some(n) = n {'),
    ('a thread read once is used for every thread of the batch', INBOX, 'threads.entry(t.n).or_insert_with(read)',
     'threads.entry(IssueNumber(0)).or_insert_with(read)'),
    ('a thread unsubscription is ignored', INBOX, 'th.subs.iter().any(|(x, s)| *x == a && !s)',
     'th.subs.is_empty() && false'),
    ("the repository's watchers are left out after the batch's first thread", INBOX,
     'let subscribers = subscribers.get_or_insert_with(|| watch::subscribers(meta));',
     'let subscribers = &mut subscribers.replace(Vec::new()).unwrap_or_else(|| watch::subscribers(meta));'),
    ("Search reads only the batch's first entry", SEARCH, 'for ev in es.iter().flat_map(',
     'for ev in es.iter().take(1).flat_map('),
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
            subprocess.run(BUILD, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1800)
            r = subprocess.run(T, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=300)
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
