#!/usr/bin/env python3
"""looplag-clock: planted bugs in what `LOOP_LAG` counts (`decisions/DECISIONS.md`, "A leader whose loop is behind steps
down (`LOOP_LAG`)", amended 2026-11-20; B0-29 m3fp2's trip on member-clock samples,
`tests/m0/results/confirm-storage-m3f.md`). Plants one mutant at a time, runs the tests that guard it, and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant54.py          # every mutant
    python3 tests/m0carried/plant54.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
HEALTH = 'crates/raft/src/member/health.rs'
HOST = 'workers/site/src/host.rs'
LEAD = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'leadership', '--', 'loop_lag', 'two_clamped']
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'step_down_window']

MUTANTS = [
    ("main's rule: the member clock's step-to-end time again", HEALTH,
     '            let isolate = times.and_then(|t| t.isolate_ms).unwrap_or(ms);\n',
     '            let isolate = ms;\n', LEAD),
    ("each confirm one sample, not each sync", HEALTH,
     '            .map(Ended::sample)\n',
     '            .map(|c| (false, c.id.0))\n', LEAD),
    ("the isolate reading ignored for whole ticks: any 200 ms or more counts", HEALTH,
     '            .filter(|c| c.at >= since && c.isolate >= at_least && c.isolate % 100 == 0)\n',
     '            .filter(|c| c.at >= since && c.isolate >= at_least)\n', LEAD),
    ("the host logs the member clock's time as the isolate's", HOST,
     '        w.isolate.iter().map(|(t, ms, s)| [t.0 as i64, i64::from(*ms), s.map_or(-1, |s| s as i64)]).collect();\n',
     '        w.whole.iter().map(|(t, ms)| [t.0 as i64, i64::from(*ms), -1]).collect();\n', SITE),
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
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
