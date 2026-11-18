#!/usr/bin/env python3
"""write-cuts: planted bugs in BF-12's double-apply evidence (`workers/site/src/txn.rs`) and the B0-33 harness's
`arrival=mem` (`tests/fanout/rows.sh`, `arrival.mjs`). Plants one mutant at a time, runs the check that guards it, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/writecuts/plant.py          # every mutant
    python3 tests/writecuts/plant.py 0 2      # by number

The bed mutants build and run local workerd (`tests/writecuts/bed/run.sh`, about a minute each).
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
TXN = 'workers/site/src/txn.rs'
ARR = 'tests/fanout/arrival.mjs'
UNIT = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'txn::']
BED = ['bash', 'tests/writecuts/bed/run.sh']
# arrival.mjs's own check, without an install: a voter answering `sql` must fail it.
ARRT = ['node', '--input-type=module', '-e', '''
import { readFileSync } from "node:fs";
const src = readFileSync("tests/fanout/arrival.mjs", "utf8");
const m = src.match(/if \\((.*)\\) ok = false;/);
if (!m) process.exit(3);
const bad = new Function("got", "mode", `return ${m[1]};`);
process.exit(bad("sql", "mem") && !bad("mem", "mem") && bad(null, "mem") ? 0 : 1);
''']

MUTANTS = [
    ('the dup write drops (a double apply leaves no evidence)', TXN,
     '                sql.exec(dq, da).map_err(fail)?;\n', '                let _ = (dq, da);\n', BED),
    ('every request insert writes a dup row (as the old per-request statement wrote one per conflict, now always)', TXN,
     '(q == REQUEST_INSERT && written == 0)', '(q == REQUEST_INSERT)', UNIT),
    ('a written row counts as a double apply when it wrote only its index entry', TXN,
     '(q == REQUEST_INSERT && written == 0)', '(q == REQUEST_INSERT && written <= 1)', UNIT),
    ('the dup row names the request but not the index', TXN,
     'args.iter().take(2).cloned().collect()', 'args.iter().take(1).cloned().chain([SqlStorageValue::Integer(0)]).collect()',
     UNIT),
    ('arrival.mjs passes a voter that answered sql', ARR, '  if (got !== mode) ok = false;\n',
     '  if (got === null) ok = false;\n', ARRT),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out:
                verdict = 'DID NOT COMPILE'
            else:
                verdict = 'CAUGHT'
                caught += 1
        finally:
            open(p, 'wb').write(orig)
        print(f'{i} {name}: {verdict}', flush=True)
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
