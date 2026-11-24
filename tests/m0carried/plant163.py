#!/usr/bin/env python3
"""The rehydrate's repack wait (G37, B0-34 mt24): planted bugs in who starts a git cache shard's repack at the end of a
rehydrate that skipped covered packs (`crates/git/src/catchup.rs`, `rehydrate`, its last catch-up and backstop;
`containers/git/server/main.go`, `handleRefs` and `connectedWalk`; `tests/am1/results/b034-mt24.md`, "The home's
rehydrate"): main's rule, where the last catch-up's `POST /refs` starts the repack and the backstop's walk waits it
out; the flag read inverted or dropped on the page; the steps' `walk` lap lost; the container ignoring `repack=0` on
a POST; and `repack_wait_ms` timed after the wait. Plants one mutant at a time, runs the tests that guard it, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Never run it under `timeout`: a
kill leaves the mutant in place. Run from the repository root:

    python3 tests/m0carried/plant163.py          # every mutant
    python3 tests/m0carried/plant163.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
CATCHUP = 'crates/git/src/catchup.rs'
MAIN = 'containers/git/server/main.go'
RUST = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'catchup']
GO = ['go', '-C', 'containers/git/server', 'test', '-count=1', '-run',
      'TestACatchUpPostLeavesTheRepackToTheBackstopsWalk|TestConnectedRunsBeforeTheRehydratesRepack', '.']

MUTANTS = [
    ("main's rule: the last catch-up's ref changes start the repack ahead of the backstop's walk", CATCHUP,
     '    let repack = state.skipped.is_empty();\n', '    let repack = true;\n', RUST),
    ("the flag inverted: the repack is left to a check only when no check follows", CATCHUP,
     '    let repack = state.skipped.is_empty();\n', '    let repack = !state.skipped.is_empty();\n', RUST),
    ("the page drops the flag and always lets its changes start the repack", CATCHUP,
     '            io.apply_refs(&changes, None, *repack).await?;\n',
     '            io.apply_refs(&changes, None, true).await?;\n', RUST),
    ("the catch-up's lap is lost, so the walk's time reads as the catch-up's", CATCHUP,
     '    lap(&mut ms, "catch_up");\n    r.skipped = state.skipped.len();\n', '    r.skipped = state.skipped.len();\n', RUST),
    ("the container ignores repack=0 on a POST", MAIN,
     '\t\tif q.Get("repack") != "0" {\n\t\t\trepack = startRepack(net, rp)',
     '\t\tif put && q.Get("repack") == "0" {\n\t\t} else {\n\t\t\trepack = startRepack(net, rp)', GO),
    ("repack_wait_ms timed after the wait for the repack", MAIN,
     '\tw0 := time.Now()\n\trepacking.Lock()\n\tdefer repacking.Unlock()\n\tt0 := time.Now()\n',
     '\trepacking.Lock()\n\tdefer repacking.Unlock()\n\tt0 := time.Now()\n\tw0 := t0\n', GO),
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, errors='replace', timeout=1800, stdin=subprocess.DEVNULL)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out or '[build failed]' in out or 'vet:' in out:
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
