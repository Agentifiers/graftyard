#!/usr/bin/env python3
"""`maint-yield`'s overdue start under client CPU (G41 line 3, B0-34 mt23): planted bugs in how long client CPU above
`FANOUT_UP` may delay a git cache shard's maintenance start (`containers/git/server/maint.go`, `maintBegin`,
`cpuDeferMax`; `decisions/DECISIONS.md`, "`maint-yield`", "The overdue start waits for client CPU"): main's cap of
`maintDeferMax` plus one `yieldHold`, no cap at all, a cap read off the wrong clock, CPU at the line counted as over
it, and a constant under mt23's measured wait. Plants one mutant at a time, runs the tests that guard it, and restores
the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Never run it under `timeout`: a kill
leaves the mutant in place. Run from the repository root:

    python3 tests/m0carried/plant162.py          # every mutant
    python3 tests/m0carried/plant162.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
MAINT = 'containers/git/server/maint.go'
GO = ['go', '-C', 'containers/git/server', 'test', '-count=1', '-run',
      'TestMaintenanceYieldsToMovedLoad|TestAnOverdueStartWaitsForClientCPUWithinItsCap', '.']
BUSY = '\tbusy := cpuP99() > fanoutUp && deferred < cpuDeferMax\n'

MUTANTS = [
    ("main's rule: client CPU delays an overdue start only one yieldHold past maintDeferMax (mt23's c)", MAINT, BUSY,
     '\tbusy := cpuP99() > fanoutUp && deferred < maintDeferMax+yieldHold\n', GO),
    ("client CPU delays a start without end", MAINT, BUSY, '\tbusy := cpuP99() > fanoutUp\n', GO),
    ("client CPU never delays an overdue start", MAINT, BUSY,
     '\tbusy := cpuP99() > fanoutUp && deferred < maintDeferMax\n', GO),
    ("CPU under the line still delays the overdue start", MAINT, BUSY,
     '\tbusy := cpuP99() > fanoutUp/2 && deferred < cpuDeferMax\n', GO),
    ("the cap is off by one yieldHold (granted one hold early)", MAINT, BUSY,
     '\tbusy := cpuP99() > fanoutUp && deferred < cpuDeferMax-yieldHold\n', GO),
    ("cpuDeferMax under mt23's measured 617 s wait", MAINT, 'var cpuDeferMax = 12 * time.Minute',
     'var cpuDeferMax = 7 * time.Minute', GO),
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
