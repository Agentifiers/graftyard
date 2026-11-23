#!/usr/bin/env python3
"""alarm-step scan lines: planted bugs in the stored-range and line helpers of `workers/site/src/alarm_steps.rs`
(`stored_range`, `scan_line`; `tests/am1/results/seal-bytes-design.md`, section 6). Each leader alarm step that is about
to read stored log rows logs one line, so a reset mid-step shows the step and its scan. Plants one mutant at a time,
runs the module's tests, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run
from the repository root:

    python3 tests/m0carried/plant103.py          # every mutant
    python3 tests/m0carried/plant103.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
F = 'workers/site/src/alarm_steps.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'alarm_steps']

MUTANTS = [
    ('the memory window is ignored: every in-memory step logs a line',
     '    let hi = base.map_or(to, |base| to.min(base));\n',
     '    let hi = to;\n'),
    ('the base entry counts as in memory (off by one at the window)',
     '    let hi = base.map_or(to, |base| to.min(base));\n',
     '    let hi = base.map_or(to, |base| to.min(base.saturating_sub(1)));\n'),
    ('a range that ends at its start still logs',
     '    (hi > from).then_some((from, hi))\n',
     '    (hi >= from).then_some((from, hi))\n'),
    ('a step with no window (stored only) logs nothing',
     '    let hi = base.map_or(to, |base| to.min(base));\n',
     '    let hi = base.map_or(from, |base| to.min(base));\n'),
    ('the line names the exclusive start',
     '    format!("{name} alarm step {step} scans stored {}..={to}: {bytes} bytes", from + 1)\n',
     '    format!("{name} alarm step {step} scans stored {}..={to}: {bytes} bytes", from)\n'),
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
