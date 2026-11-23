#!/usr/bin/env python3
"""read-memo: planted bugs in a fold tier's read memo (`workers/site/src/tier.rs`, `decisions/DECISIONS.md`,
"`read-memo`"). Plants one mutant at a time, runs `tests/leaderprof/memo.sh` (the member's own tier code in local
workerd), and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the
repository root:

    python3 tests/m0carried/plant97.py          # every mutant
    python3 tests/m0carried/plant97.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
TIER = 'workers/site/src/tier.rs'
MEMO = ['bash', 'tests/leaderprof/memo.sh']

MUTANTS = [
    ('`flushed` keeps the memo (a snapshot writer\'s own statements are then hidden)', TIER,
     '        i.sealed.clear();\n        *i.memo.borrow_mut() = Memo::fresh();\n',
     '        i.sealed.clear();\n'),
    ('`clear` keeps the memo', TIER,
     '        *inner.memo.borrow_mut() = Memo::fresh();\n        inner.open.retain',
     '        inner.open.retain'),
    ('a refused read is kept', TIER,
     '        self.loaded && self.lost.borrow().is_none()\n',
     '        self.loaded\n'),
    ('the memo is read before the buffered writes', TIER,
     '        match inner.pending(table, key) {\n            Some(v) => v,\n            None => inner.sql_get(table, key),\n        }\n',
     '        if let Some(v) = inner.memo().gets.get(&(table.to_owned(), key.to_vec())).cloned() {\n'
     '            return v;\n        }\n'
     '        match inner.pending(table, key) {\n            Some(v) => v,\n            None => inner.sql_get(table, key),\n        }\n'),
    ('a transaction run through another handle keeps this one\'s memo', TIER,
     '        if m.generation != crate::txn::generation() {\n',
     '        if false && m.generation != crate::txn::generation() {\n'),
    ('the memo keys a prefix read by its table alone', TIER,
     '        let k = (table.to_owned(), prefix.to_vec());\n        if self.memo_ok()\n',
     '        let k = (table.to_owned(), Vec::new());\n        if self.memo_ok()\n'),
]

ENV = dict(os.environ, CARGO_INCREMENTAL='0')


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        # As tests/prefix/plant.py: the original stays a hard link and the mutant goes in by rename, so a full disk
        # never leaves the source empty.
        keep, tmp = p + '.plant-orig', p + '.plant-new'
        os.link(p, keep)
        try:
            with open(tmp, 'w') as t:
                t.write(s.replace(old, new))
            os.replace(tmp, p)
            r = subprocess.run(MEMO, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=3600)
        finally:
            if os.path.exists(tmp):
                os.remove(tmp)
            os.replace(keep, p)
            if open(p, 'rb').read() != orig:
                sys.exit(f'{f} did not restore: run `git diff -- {f}` before anything else')
        out = r.stdout + r.stderr
        if r.returncode == 2 or 'error[E' in out or 'could not compile' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            print(f'{i:2} CAUGHT  {name}: {next((l for l in out.splitlines() if "false" in l or "mismatch" in l), "")[:160]}')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')
    sys.exit(0 if caught == len(list(pick)) else 1)


if __name__ == '__main__':
    main()
