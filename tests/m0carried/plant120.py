#!/usr/bin/env python3
"""open-bytes: planted bugs in the open-request byte counter (`workers/site/src/openbytes.rs`, `/stats` `memSplit`
`open`; `tests/am1/results/archive-memory.md`, "The open-bytes counter"). A request holds its body and its answer's
bytes until it ends by any path, and the peaks move only at a reset, to what is held then. Plants one mutant at a time,
runs the module's tests, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run
from the repository root:

    python3 tests/m0carried/plant120.py          # every mutant
    python3 tests/m0carried/plant120.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
F = 'workers/site/src/openbytes.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'openbytes']

HELD = ('    let mut open = OpenBytes::enter(of, body);\n    let r = f.await;\n    if let Ok(v) = &r {\n'
        '        open.answer(answer_len(v));\n    }\n    r\n')

MUTANTS = [
    ('an ended request keeps its bytes',
     '        self.of.bytes.set(self.of.bytes.get().saturating_sub(self.bytes));\n', ''),
    ('an ended request stays open',
     '        self.of.open.set(self.of.open.get().saturating_sub(1));\n', ''),
    ('a handler that errs leaks its count',
     HELD, HELD.replace('    r\n', '    if r.is_err() {\n        std::mem::forget(open);\n    }\n    r\n')),
    ('the count is released only when the handler returns, so a dropped future leaks it',
     HELD,
     '    let mut open = std::mem::ManuallyDrop::new(OpenBytes::enter(of, body));\n    let r = f.await;\n'
     '    if let Ok(v) = &r {\n        open.answer(answer_len(v));\n    }\n'
     '    unsafe { std::mem::ManuallyDrop::drop(&mut open) };\n    r\n'),
    ('a sample resets the byte peak',
     '            high: self.high.get(),\n', '            high: self.high.replace(self.bytes.get()),\n'),
    ('a sample resets the open peak',
     '            open_high: self.open_high.get(),\n', '            open_high: self.open_high.replace(self.open.get()),\n'),
    ('a reset sets the byte peak to 0, below what is held',
     '        self.high.set(self.bytes.get());\n', '        self.high.set(0);\n'),
    ('a reset leaves the peaks alone',
     '        self.high.set(self.bytes.get());\n        self.open_high.set(self.open.get());\n', ''),
    ('an answer does not raise the byte peak',
     '        self.of.bytes.set(self.of.bytes.get() + n);\n        self.of.mark();\n',
     '        self.of.bytes.set(self.of.bytes.get() + n);\n'),
    ('a new request does not raise the open peak',
     '        this.open_high.set(this.open_high.get().max(this.open.get()));\n', ''),
    ('the answer is not counted',
     '        open.answer(answer_len(v));\n', '        let _ = answer_len(v);\n'),
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
