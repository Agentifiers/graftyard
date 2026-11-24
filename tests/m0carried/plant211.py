#!/usr/bin/env python3
"""BM-5 clause 3: planted bugs in `MINT_ERROR_RATIO` (`crates/release/src/gate.rs`, `count_mint` and `MintWatch`;
`decisions/DECISIONS.md`, "`mint-ratio`"). Plants one mutant at a time, runs the release crate's swap-back test, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant211.py          # every mutant
    python3 tests/m0carried/plant211.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GATE = 'crates/release/src/gate.rs'
REL = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', 'bm5_a_bad_graft_ops']

MUTANTS = [
    ("a 4xx counts as a mint error", GATE,
     'let fivexx = u64::from(status.is_none_or(|s| s >= 500));\n    let a = match (status, ran) {',
     'let fivexx = u64::from(status.is_none_or(|s| s >= 400));\n    let a = match (status, ran) {', REL),
    ("a thrown mint is not counted", GATE,
     '            mints.threw += 1;\n            return;', '            return;', REL),
    ("N's errors count too: every version's mints are summed", GATE,
     '        let n1 = self.mints.of(n1);',
     '        let _ = n1;\n        let n1 = self.mints.versions.values().fold(Answers::default(), |mut s, x| {\n            s.add(x);\n            s\n        });',
     REL),
    ("no resend gap: every mint past the line submits again", GATE,
     '        if self.sent_ms.is_some_and(|t| now_ms < t + MINT_RESEND_MS) {',
     '        if self.sent_ms.is_some_and(|_| false) {', REL),
    ("the counts never start over at a new intent", GATE,
     '            *self = MintWatch { intent: Some(c.intent.id), ..MintWatch::default() };',
     '            self.intent = Some(c.intent.id);', REL),
    ("no chance bound: any N+1 rate over 1% swaps back", GATE,
     '    a as f64 * (term(p, base) + term(1.0 - p, 1.0 - base)) >= GATE_ERRORS_NATS',
     '    let _ = term;\n    true', REL),
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
