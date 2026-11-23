#!/usr/bin/env python3
"""catchup-lag, lever C: planted bugs in the batched stored ingest (`containers/git/server/main.go`,
`/ingest-spooled-batch` and `ingestRun`; `crates/git/src/catchup.rs`, `ingest_all`; `decisions/DECISIONS.md`,
"`catchup-lag`"): a batch that records one pack's objects under another's sum, a refused call that moved packs,
and a batch that waits on a running fetch. Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant109.py          # every mutant
    python3 tests/m0carried/plant109.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
CU = 'crates/git/src/catchup.rs'
GO = 'containers/git/server/main.go'
CU_T = (['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'catchup'], '.')
GO_T = (['go', 'test', '-count=1', '-run', 'IngestSpooledBatch|IngestBatch', '.'], 'containers/git/server')

MUTANTS = [
    ("a run records each pack's objects under the next pack's sum", GO,
     "\t\tif err := writeObjs(rp, packs[i].sum, objs[k]); err != nil {\n",
     "\t\tif err := writeObjs(rp, packs[run[(k+1)%len(run)]].sum, objs[k]); err != nil {\n", GO_T),
    ("an adopted pack is recorded under the call's last sum", GO,
     "\t\t\t\t\terr = record(rp, p.sum, name, filepath.Join(rp, \"objects\"), sent)\n\t\t\t\t}\n\t\t\t\tif err != nil {\n\t\t\t\t\treturn fmt.Errorf(\"pack %d: %v\", i, err)\n",
     "\t\t\t\t\terr = record(rp, packs[len(packs)-1].sum, name, filepath.Join(rp, \"objects\"), sent)\n\t\t\t\t}\n\t\t\t\tif err != nil {\n\t\t\t\t\treturn fmt.Errorf(\"pack %d: %v\", i, err)\n", GO_T),
    ("a call refused for a pack not staged moves the packs before it first", GO,
     "\t\tif here != nil && aside != nil && staged != nil {\n",
     "\t\tif false && here != nil && aside != nil && staged != nil {\n", GO_T),
    ("a batch waits on a stored pack whose fetch is still running", CU,
     "                                    match ahead.take_ready().await {\n                                        None => break,\n",
     "                                    match Some(ahead.take().await) {\n                                        None => break,\n", CU_T),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, (cmd, cwd) = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=os.path.join(ROOT, cwd), capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out or '[build failed]' in out:
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
