#!/usr/bin/env python3
"""log-page: planted bugs in `log_after`'s page bound (`workers/site/src/logread.rs`, `page_through`;
`tests/am1/results/archive-memory.md`). A page takes rows while their stored bytes stay within `PAGE_BYTES`, and at
least one. Plants one mutant at a time, runs the module's tests, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. The walk's own loop (its `more` and `after`) is planted against the workerd bed,
`tests/auditlock/bed/run.sh` with `LOGREAD=`, as that write-up records. Run from the repository root:

    python3 tests/m0carried/plant112.py          # every mutant
    python3 tests/m0carried/plant112.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
F = 'workers/site/src/logread.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'logread']

MUTANTS = [
    ('the budget is ignored: a page takes every row its count allows',
     '        if out.is_some() && bytes > budget {\n',
     '        if out.is_some() && bytes > u64::MAX {\n'),
    ('a page that would end exactly at the budget stops one row short',
     '        if out.is_some() && bytes > budget {\n',
     '        if out.is_some() && bytes >= budget {\n'),
    ('a first row over the budget is no page, so the walk stops at it',
     '        if out.is_some() && bytes > budget {\n',
     '        if bytes > budget {\n'),
    ('the row that passes the budget is taken too',
     '            break;\n        }\n        out = Some((idx, bytes));\n',
     '            out = Some((idx, bytes));\n            break;\n        }\n        out = Some((idx, bytes));\n'),
]

ENV = dict(os.environ, CARGO_INCREMENTAL='0')


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, old, new = MUTANTS[i]
        p = os.path.join(ROOT, F)
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
            r = subprocess.run(T, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=3600)
        finally:
            if os.path.exists(tmp):
                os.remove(tmp)
            os.replace(keep, p)
            if open(p, 'rb').read() != orig:
                sys.exit(f'{F} did not restore: run `git diff -- {F}` before anything else')
        out = r.stdout + r.stderr
        if 'error[E' in out or 'could not compile' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            print(f'{i:2} CAUGHT  {name}: {next((l for l in out.splitlines() if "panicked" in l), "")[:160]}')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')
    sys.exit(0 if caught == len(list(pick)) else 1)


if __name__ == '__main__':
    main()
