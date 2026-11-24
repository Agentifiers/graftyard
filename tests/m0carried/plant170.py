#!/usr/bin/env python3
"""R53: planted bugs in a drained site's calm while a running site day holds another voter to the 1% lead limit
(`crates/release/src/gate.rs`, `verdict`) and in the bound on a day's wait
on another drained site (`crates/release/src/ops.rs`, `next`; `decisions/DECISIONS.md`, "`calm-one`"). Plants one
mutant at a time, runs the release crate's tests, and restores the file byte for
byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant170.py          # every mutant
    python3 tests/m0carried/plant170.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GATE = 'crates/release/src/gate.rs'
OPS = 'crates/release/src/ops.rs'
REL = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib']

MUTANTS = [
    ("calm needs both voters even while one is lead-limited (main's rule)", GATE,
     '        if judged.len() == others.len() || judged.is_empty() {',
     '        if true || judged.len() == others.len() || judged.is_empty() {', REL),
    ("the remaining voter needs no MIN_CALLS", GATE,
     '        judged.iter().all(|o| report(*o).is_some_and(|h| h.under(UNDRAIN_BURN))) && others.iter().all(|o| quiet(*o))',
     '        judged.iter().all(|o| report(*o).is_none_or(|h| h.missed * ALLOWED_MISS_DENOM < UNDRAIN_BURN * h.calls)) && others.iter().all(|o| quiet(*o))',
     REL),
    ("calm from one voter whether or not the other is lead-limited", GATE,
     '        let judged: Vec<SiteId> = others.iter().copied().filter(|o| Some(*o) != limited).collect();',
     '        let judged: Vec<SiteId> = others.iter().copied().take(1).collect();', REL),
    ("the limited voter's own evidence of burn is ignored", GATE,
     '&& others.iter().all(|o| quiet(*o))', '&& others.iter().all(|o| quiet(*o) || true)', REL),
    ("burning drops the lead-limited voter too", GATE,
     '            let burning = all(*t, fast, &|h| h.at_least(DRAIN_BURN));',
     '            let burning = voters.iter().filter(|o| **o != *t && Some(**o) != limited).all(|o| fast.iter().any(|h| h.observer == *o && h.target == *t && h.at_least(DRAIN_BURN)));',
     REL),
    ("no page after STUCK_DAY on another drained site", OPS,
     '        if w.now >= until {\n            return Action::Page(',
     '        if w.now >= until && false {\n            return Action::Page(', REL),
    ("the wait is timed from the day's last entry only, not the later drain", OPS,
     '        let from = fold.drained.get(&d).map_or(c.since, |x| Timestamp(x.since.0.max(c.since.0)));',
     '        let from = c.since;', REL),
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
