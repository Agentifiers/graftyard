#!/usr/bin/env python3
"""dep-hedge-adaptive: planted bugs in each chain's hedge delay (`decisions/DECISIONS.md`, "`dep-hedge-adaptive`"). Plants one mutant
at a time, runs the test that guards it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID
NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant42.py          # every mutant
    python3 tests/m0carried/plant42.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LOOKUP = 'crates/lookup/src/lib.rs'
TEST = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--', 'dependency_read']

MUTANTS = [
    ('as on main: every chain waits DEP_HEDGE', LOOKUP,
     '    let got = hedge(read(), delay(dep_hedge_after(chain)), &read).await;',
     '    let got = hedge(read(), delay(DEP_HEDGE), &read).await;', TEST),
    ('a fixed 150 ms for every chain', LOOKUP,
     '        let Some(times) = m.get(&chain).filter(|t| t.len() >= DEP_HEDGE_MIN) else { return DEP_HEDGE };',
     '        let Some(times) = m.get(&chain).filter(|t| t.len() >= DEP_HEDGE_MIN) else { return DEP_HEDGE };\n        let _ = times;\n        return Duration::from_millis(150);', TEST),
    ('one window for every chain', LOOKUP,
     '        let times = m.entry(chain).or_default();',
     '        let times = m.entry(ChainId::Ids).or_default();\n        let _ = chain;', TEST),
    ('dep_read records no read times', LOOKUP,
     '        dep_read_took(chain, now().saturating_sub(began));', '        let _ = (began, &now);', TEST),
    ('no floor', LOOKUP,
     '.clamp(DEP_HEDGE_FLOOR, DEP_HEDGE)', '.min(DEP_HEDGE)', TEST),
    ('no cap', LOOKUP,
     '.clamp(DEP_HEDGE_FLOOR, DEP_HEDGE)', '.max(DEP_HEDGE_FLOOR)', TEST),
    ('the window never drops its oldest', LOOKUP,
     '        if times.len() >= DEP_HEDGE_SAMPLES {\n            times.remove(0);\n        }\n', '', TEST),
    ('the median, not the p90', LOOKUP,
     'sorted[sorted.len() * 9 / 10]', 'sorted[sorted.len() / 2]', TEST),
    ('hedges before the chain is timed', LOOKUP,
     '.filter(|t| t.len() >= DEP_HEDGE_MIN)', '.filter(|t| !t.is_empty())', TEST),
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=900)
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
