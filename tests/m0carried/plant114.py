#!/usr/bin/env python3
"""R38: planted bugs in the pooled 1% floor (`crates/release/src/ops.rs`, `next`; `crates/release/src/gate.rs`,
`window_start`; `crates/release/src/group.rs`, `Current::before`; `decisions/DECISIONS.md`, "`gate-pool`"). Plants
one mutant at a time, runs the release crate's tests, and restores the file byte for byte. Each line prints CAUGHT,
MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant114.py          # every mutant
    python3 tests/m0carried/plant114.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
OPS = 'crates/release/src/ops.rs'
GATE = 'crates/release/src/gate.rs'
GROUP = 'crates/release/src/group.rs'
REL = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib']

MUTANTS = [
    ("a starved 1% step dwells twice and pages (as before R38)", OPS,
     '                    && *pct != 1\n', '', REL),
    ("10% counts from its own start, so nothing is pooled", GATE,
     '{ c.before } else { c.since }', '{ c.since } else { c.since }', REL),
    ("every step under 100% counts from the step before it", GATE,
     'pct(k) == Some(10) && pct(k - 1) == Some(1)', 'pct(k).is_some_and(|p| p < 100)', REL),
    ("IntentStep leaves `before` at the intent's start", GROUP,
     '                c.before = c.since;\n', '', REL),
    ("a breach at 1% is ignored, as if the 1% step were skipped", OPS,
     '            if *gated && let Gate::Breach(why) = &w.gate {\n                return breach(fold, c, why);',
     '            if *gated && *pct != 1 && let Gate::Breach(why) = &w.gate {\n                return breach(fold, c, why);',
     REL),
]


def main():
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
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
