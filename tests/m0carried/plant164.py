#!/usr/bin/env python3
"""Fold aside (G37, B0-34 mt25): planted bugs in how a composed-miss chain folds beside its steps
(`containers/git/server/compose.go`, `handleCompose`, `startFold`, `swapFold`; `decisions/DECISIONS.md`, "`fold-aside`";
`tests/am1/results/b034-mt25.md`, "Why parts stop during a fold"). The plan named them plants 164 to 168; they are this
one file's mutants 0 to 4, and 5 and 6 are the lead's two additions (a restarted container drops a kept fold, and the
swap checks the chain still extends the fold's). Plants one mutant at a time, runs the tests that guard it, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Never run it under `timeout`: a
kill leaves the mutant in place. Run from the repository root:

    python3 tests/m0carried/plant164.py          # every mutant
    python3 tests/m0carried/plant164.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
CG = 'containers/git/server/compose.go'
GO = ['go', '-C', 'containers/git/server', 'test', '-count=1', '-run',
      'TestPartsKeepLandingWhileAFoldBuilds|TestARestartedContainerStepsAPartFirstAndDropsAKeptFold|TestComposedMiss$', '.']

MUTANTS = [
    ("164: the step waits for the fold it starts, so no part lands while it builds", CG,
     '\t\tstartFold(net, rp, env, next)\n',
     '\t\tstartFold(net, rp, env, next)\n\t\tv, _ := folds.Load(net)\n\t\t<-v.(*foldRun).done\n', GO),
    ("164: main's rule, the fold inline as the 20th step", CG,
     '\tbase := !have || len(h.Parts)+1 >= partsMax\n', '\tbase := !have || len(h.Parts)+1 >= 20\n', GO),
    ("165: the swap's part leaves out what the old chain holds, as if the parts after T were kept", CG,
     '\tif objs, err = missingIn(foldObjects(net), rp, objs, m.gitLow); err != nil {\n',
     '\tif objs, err = missingIn(composeObjects(net), rp, objs, m.gitLow); err != nil {\n', GO),
    ("166: the fold builds in the chain's pack directory, where dropUnnamed deletes it", CG,
     '\tdir := foldPack(net)\n', '\tdir := filepath.Join(composeObjects(net), "pack")\n', GO),
    ("167: the swap uses a fold whose tip T left the chain", CG,
     '\t\tif f.err == nil && !base && have && h.extends(f.at) && tip != f.at.tip() {\n',
     '\t\tif f.err == nil && !base && have && tip != f.at.tip() {\n', GO),
    ("168: no ceiling at partsMax", CG,
     '\tbase := !have || len(h.Parts)+1 >= partsMax\n', '\tbase := !have\n', GO),
    ("a restarted container keeps a fold its disk held", CG,
     '\t} else {\n\t\tos.RemoveAll(foldDir(net))\n\t}\n', '\t}\n', GO),
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
