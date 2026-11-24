#!/usr/bin/env python3
"""`maint-yield`: planted bugs in a git cache shard's yield of its maintenance to moved load
(`containers/git/server/maint.go`, `startRepack` and `handleRoll` in `main.go`, `graftyard_git::maint_hold` in
`crates/git/src/shard.rs`; `decisions/DECISIONS.md`, "`maint-yield`"). Plants one mutant at a time, runs the test that
guards it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Never run it under
`timeout`: a kill leaves the mutant in place. Run from the repository root:

    python3 tests/m0carried/plant89.py          # every mutant
    python3 tests/m0carried/plant89.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
MAINT = 'containers/git/server/maint.go'
MAIN = 'containers/git/server/main.go'
SHARD = 'crates/git/src/shard.rs'
GO = ['go', '-C', 'containers/git/server', 'test', '-count=1', '-run',
      'TestMaintenanceYieldsToMovedLoad|TestAHeldRollAnswers503|TestMaintDeferMaxFitsAFailoverAndThePackLine|TestAnOverdueStartWaitsForClientCPUWithinItsCap', '.']
RS = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', '--', 'only_a_call_moved_off']

MUTANTS = [
    ("a hold does not stop a running repack (mt18's repack ran through the kill)", MAINT,
     '\tif maint.cancel != nil && !maint.overdue {\n\t\tmaint.cancel()',
     '\tif maint.cancel != nil && !maint.overdue && false {\n\t\tmaint.cancel()', GO),
    ("the stop reaches only git, not its process group (the repack's children run on)", MAINT,
     'return syscall.Kill(-cmd.Process.Pid, syscall.SIGTERM)',
     'return syscall.Kill(cmd.Process.Pid, syscall.SIGTERM)', GO),
    ("a hold call does not renew the hold", MAINT,
     '\tmaint.until = now.Add(yieldHold)\n',
     '\tif !now.Before(maint.until) {\n\t\tmaint.until = now.Add(yieldHold)\n\t}\n', GO),
    ("a held shard still starts a repack", MAINT,
     '\theld := now.Before(maint.until) && deferred < maintDeferMax',
     '\theld := false && deferred < maintDeferMax', GO),
    ("the deferral bound is ignored: moved load holds maintenance forever", MAINT,
     '\theld := now.Before(maint.until) && deferred < maintDeferMax',
     '\theld := now.Before(maint.until)', GO),
    ("a run past the deferral bound is still stopped by moved load", MAINT,
     'maint.cancel, maint.overdue = cancel, deferred >= maintDeferMax',
     'maint.cancel, maint.overdue = cancel, false', GO),
    ("MAINT_DEFER_MAX back at 10 min (B0-34 mt19: 786 unkept packs while held, past 751)", MAINT,
     'var maintDeferMax = 6 * time.Minute', 'var maintDeferMax = 10 * time.Minute', GO),
    ("MAINT_DEFER_MAX shorter than a kill and a rehydrate (the repack lands inside the failover)", MAINT,
     'var maintDeferMax = 6 * time.Minute', 'var maintDeferMax = 5 * time.Minute', GO),
    ("client CPU above FANOUT_UP does not hold a start", MAINT,
     '\tbusy := cpuP99() > fanoutUp && deferred < cpuDeferMax',
     '\tbusy := false && deferred < cpuDeferMax', GO),
    ("client CPU holds a start past cpuDeferMax (plant162 covers the cap itself)", MAINT,
     '\tbusy := cpuP99() > fanoutUp && deferred < cpuDeferMax',
     '\tbusy := cpuP99() > fanoutUp', GO),
    ("a stopped repack still writes the commit-graph, holding the slot", MAIN,
     '\t\tif ctx.Err() != nil {\n\t\t\tfmt.Fprintf(os.Stderr, "repack %s: stopped for moved load\\n", net)\n\t\t\treturn\n\t\t}\n',
     '\t\tif ctx.Err() != nil {\n\t\t\tfmt.Fprintf(os.Stderr, "repack %s: stopped for moved load\\n", net)\n\t\t\ttime.Sleep(3 * time.Second)\n\t\t}\n', GO),
    ("a held roll-up builds anyway", MAIN,
     '\tctx, end, ok := maintBegin()\n\tif !ok {\n\t\tfail(w, 503, "maint-held")',
     '\tctx, end, ok := maintBegin()\n\tif !ok \u0026\u0026 false {\n\t\tfail(w, 503, "maint-held")', GO),
    ("a call the edge did not mark moved counts as moved load (G41: mt21's first tries after a hedge pass)", SHARD,
     '        && moved\n', '        && (moved || !moved)\n', RS),
    ("the home's own first try counts as moved load", SHARD,
     '        && me != home\n', '        && (me != home || me == home)\n', RS),
    ("a derived promotion counts as moved load", SHARD,
     '        && !derived\n', '        && (derived || !derived)\n', RS),
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
