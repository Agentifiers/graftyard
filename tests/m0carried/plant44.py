#!/usr/bin/env python3
"""refold-stream: planted bugs in the fold audit's staged refold (`decisions/DECISIONS.md`, "`refold-stream`"): the key
encoding the page lookup orders by, the view's merge of its changes over the staged pages, and the deletion of an
audit's staged rows. Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant44.py          # every mutant
    python3 tests/m0carried/plant44.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
STREAM = 'crates/fold/src/stream.rs'
LOAD = 'workers/site/src/load.rs'
FOLD = ['cargo', 'test', '-q', '--no-fail-fast', '-p', 'graftyard-fold', '--lib', '--test', 'stream_sqlite']
GIT = ['cargo', 'test', '-q', '--no-fail-fast', '-p', 'graftyard-git', '--test', 'fold_corpus', '--test', 'refold_stream']
SITE = ['cargo', 'test', '-q', '--no-fail-fast', '-p', 'graftyard-site', '--lib', 'tests::']

MUTANTS = [
    ('an integer keyed by its bytes with no sign flip, so negatives sort after positives', STREAM,
     'out.extend_from_slice(&((*i as u64) ^ (1 << 63)).to_be_bytes());',
     'out.extend_from_slice(&(*i as u64).to_be_bytes());', FOLD),
    ('text and blobs not escaped, so a value that prefixes another can sort after it', STREAM,
     '        out.push(c);\n        if c == 0 {\n            out.push(0xff);\n        }',
     '        out.push(c);', FOLD),
    ("a key looked up one page late: a page's last key is found in the next page", STREAM,
     '        let kb = key_bytes(key);\n        let (p, rows) = self.page_from(table, &kb)?;',
     '        let kb = key_bytes(key);\n        let (p, rows) = self.page_from(table, &after(&kb))?;', FOLD),
    ("a deleted row that does not hide the staged page's copy in a scan", STREAM,
     '            None => merged.remove(&k),\n        };\n    }\n    merged.into_iter().take(limit).collect()',
     '            None => None,\n        };\n    }\n    merged.into_iter().take(limit).collect()', FOLD),
    ("a scan that reads only `limit` staged rows past the changes' deletes, so a deleted row's place is left empty",
     STREAM, 'lower(from, limit.saturating_add(deletes))', 'lower(from, limit.max(deletes))', FOLD),
    ('a saved change that loses to the staged page it replaced', STREAM,
     '        match self.base.saved(table, key) {\n            Some(v) => v,\n            None => self.page_get(table, key),\n        }',
     '        self.page_get(table, key).or_else(|| self.base.saved(table, key).flatten())', FOLD),
    ('staged rows of a restore that a frame lost, taken as the whole snapshot (no trailer check)', STREAM,
     '    r.finish_rows(&mut pager)?;', '    let _ = &mut r;', GIT),
    ("an audit's end that keeps its staged pages", LOAD,
     '    stmts.extend(fold::AUDIT_CLEAR.iter().map(|q| (*q, vec![])));\n', '', SITE),
    ('a restore that keeps staged pages with no audit running', LOAD,
     '    job.is_some_and(|j| !j.install())\n', '    job.is_none_or(|j| !j.install())\n', SITE),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
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
            failed = sorted({l.split()[1] for l in out.splitlines() if l.startswith('---- ') and l.endswith(' stdout ----')})
            print(f'{i:2} CAUGHT  {name}  [{", ".join(failed[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
