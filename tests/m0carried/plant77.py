#!/usr/bin/env python3
"""teardown-stops-runs: planted bugs in how a shard's `/teardown` stops its catch-ups (`graftyard_git::CatchUps`,
`tear_down`, `resume`; `tests/am1/results/sync-ab.md`, "Close-out"). Plants one mutant at a time, runs the test that
guards it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Mutant 0 is main's
rule: nothing stops a run after a teardown. The site script's wiring (`/teardown` deleting `warming`, the run loop's
check, `resume` on a caller that may start the container) runs only in wasm; the next hosted sweep checks it by each
torn-down shard's `/status` alarm. Run from the repository root:

    python3 tests/m0carried/plant77.py          # every mutant
    python3 tests/m0carried/plant77.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
SHARD = 'crates/git/src/shard.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', '--', 'a_torn_down_shard_that_held_warming']

MUTANTS = [
    ("main's rule: a teardown stops no run", SHARD, '        self.torn = true;\n', '        let _ = self;\n'),
    ("a waiting caller still starts a run after a teardown", SHARD,
     '        } else if self.torn {\n            CatchUpStep::Stopped\n', ''),
    ("a run starts on a torn-down shard when asked directly", SHARD,
     '        if self.running || self.torn {', '        if self.running {'),
    ("a client request never resumes catch-ups", SHARD, '        self.torn = false;\n', '        let _ = self;\n'),
    ("a teardown hides a run that already finished", SHARD,
     '        if self.ok >= from {\n            CatchUpStep::Done\n',
     '        if self.torn {\n            CatchUpStep::Stopped\n        } else if self.ok >= from {\n            CatchUpStep::Done\n'),
]


def main():
    # A stop ends in SystemExit, so `finally` restores the file (plant75's lesson, `sync-ab.md`, "The plant left in").
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(T, cwd=ROOT, capture_output=True, text=True, timeout=1800)
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
