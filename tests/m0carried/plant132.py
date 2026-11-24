#!/usr/bin/env python3
"""`quiet-failures`: planted bugs in which inputs keep a member's ticker up (`workers/site/src/host.rs`, `input_quiet`,
`call_failed`, `Ended`; `decisions/OPEN-QUESTIONS.md`, R55). Each mutant needs a member's bindings and a peer that
refuses its join, which only the wasm build in local workerd has, so each runs `tests/quietbed/run.sh` on a fresh
site build (about 6 minutes). Plants one mutant at a time and restores the file byte for byte. Each line prints CAUGHT,
MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant132.py          # every mutant
    python3 tests/m0carried/plant132.py 0        # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
HOST = 'workers/site/src/host.rs'
T = ['bash', 'tests/quietbed/run.sh']
FAILED = "            let _ = self.input_quiet(|m, t| m.call_failed(t, f));\n"
ENDED = "            let _ = self.0.input_quiet(|m, t| m.call_ended(t, to, round));\n"
KEEP = "        let r = self.step(f);\n        self.active_until.set(until);\n"

MUTANTS = [
    ("a failed call counts as activity again, as on main", HOST, FAILED, FAILED.replace("input_quiet", "input"), T),
    ("an ended call counts as activity again, as on main", HOST, ENDED, ENDED.replace("input_quiet", "input"), T),
    ("input_quiet keeps active_until only when the step fails", HOST, KEEP,
     "        let r = self.step(f);\n        if r.is_err() {\n            self.active_until.set(until);\n        }\n", T),
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
    print(f'caught {caught} of {len(pick)}')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
