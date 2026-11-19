#!/usr/bin/env python3
"""edge-read-batch: planted bugs in the edge's write-class read batches (`decisions/DECISIONS.md`, "`edge-read-batch`").
Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant48.py          # every mutant
    python3 tests/m0carried/plant48.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
RC = 'workers/edge/src/rcache.rs'
TEST = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--', 'rcache']

MUTANTS = [
    # The orchestrator's plant: a waiter is handed the answer of the read that was in flight when it arrived.
    ('a waiter takes the in-flight read\'s answer', RC,
     '    fn next_id(&self) -> u64 {\n        self.seq + 1\n',
     '    fn next_id(&self) -> u64 {\n        self.seq\n', TEST),
    ('any finished read is taken, whenever it started', RC,
     '            && *id >= need\n', '            && *id >= need.min(1)\n', TEST),
    ('no batching: every write-class call reads alone', RC,
     '            Some((_, at)) if !stale(at) => Step::Wait,\n', '            Some((_, at)) if at == u64::MAX => Step::Wait,\n',
     TEST),
    ('a stuck read is waited on forever', RC,
     '            Some((_, at)) if !stale(at) => Step::Wait,\n', '            Some(_) => Step::Wait,\n', TEST),
    ('a dropped read keeps its waiters waiting', RC,
     '            f.running = None;\n        }\n    }\n\n    /// Read `id` on `k` answered',
     '            let _ = f;\n        }\n    }\n\n    /// Read `id` on `k` answered', TEST),
    ('an error is not shared: waiters read again', RC,
     '    running.end(shared);\n', '    if shared.is_ok() {\n        running.end(shared);\n    }\n', TEST),
    ('a batched read stores nothing for read-class requests', RC,
     '    if let Ok(Some(v)) = &answer {\n        put(cache, k, at, v.clone());\n    }\n    Batched',
     '    let _ = at;\n    Batched', TEST),
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
