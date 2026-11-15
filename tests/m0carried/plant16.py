#!/usr/bin/env python3
"""disk-validate: planted bugs in a push's validate on the shard's disk as it stands (`decisions/DECISIONS.md`,
"`disk-validate`"). Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant16.py          # every mutant
    python3 tests/m0carried/plant16.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GO = 'containers/git/server/main.go'
SHARD = 'crates/git/src/shard.rs'
GOT = ['go', 'test', '-count=1', '-run', 'TestValidateOnTheDiskAsItStands|TestMissingBaseWording|TestValidateDetail',
       './...']
RUST = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'disk_validate']

MUTANTS = [
    ('a truncated pack is a missing base', GO,
     '"missing blob object", "missing tree object", "missing commit object", "Could not read"} {',
     '"missing blob object", "missing tree object", "missing commit object", "Could not read", "EOF"} {', GOT),
    ('an unresolved delta is the pack\'s refusal', GO,
     'for _, m := range []string{"unresolved delta", "did not receive expected object",',
     'for _, m := range []string{"did not receive expected object",', GOT),
    ('a disk=1 failure is never held', GO,
     'if disk && (err == errBehind || missingBase(err)) {', 'if false && disk {', GOT),
    ('every failure is held, disk=1 or not', GO,
     'if disk && (err == errBehind || missingBase(err)) {', 'if err == errBehind || missingBase(err) {', GOT),
    ('a stale old tip is not behind', GO,
     'if c := strings.TrimSpace(string(cur)); c != u.Old && !(c == "" && zero(u.Old)) {',
     'if c := strings.TrimSpace(string(cur)); false && c != u.Old {', GOT),
    ('a new commit outside the pack and the refs is not behind', GO,
     '\t\t\tif !have[c] {\n\t\t\t\treturn true, nil', '\t\t\tif false && !have[c] {\n\t\t\t\treturn true, nil', GOT),
    ('the pack-and-refs check runs before index-pack wrote the pack', GO,
     '\t\tif disk {\n\t\t\thave := map[string]bool{}', '\t\tif false {\n\t\t\thave := map[string]bool{}', GOT),
    ('a held request is kept after its retry', GO,
     '\t\thd := heldDir(heldID)\n\t\tdefer os.RemoveAll(hd)', '\t\thd := heldDir(heldID)', GOT),
    ('a held retry is checked on disk=1 again', GO,
     '\tdisk := query.Get("disk") == "1"\n', '\tdisk := query.Get("disk") == "1" || query.Get("held") != ""\n', GOT),
    ('a caller takes the run in progress as its own', SHARD,
     '        self.started + 1\n', '        self.started.max(1)\n', RUST),
    ('a failed run counts as caught up', SHARD,
     '        if ok {\n            self.ok = self.started;', '        if ok || true {\n            self.ok = self.started;', RUST),
    ('a promotion validates on the disk as it stands', SHARD,
     'if derived || !running || !cursor { ValidateOn::CaughtUp }', 'if !running || !cursor { ValidateOn::CaughtUp }', RUST),
    ('every push catches up first again', SHARD,
     'if derived || !running || !cursor { ValidateOn::CaughtUp } else { ValidateOn::Disk }',
     'if derived || !running || !cursor || true { ValidateOn::CaughtUp } else { ValidateOn::Disk }', RUST),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        cwd = os.path.join(ROOT, 'containers/git/server') if cmd is GOT else ROOT
        try:
            r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=1800)
        finally:
            open(p, 'wb').write(orig)
        out = r.stdout + r.stderr
        if 'error[E' in out or 'could not compile' in out or '[build failed]' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            fails = [l.split()[1] if l.startswith('---- ') else l.split()[2] for l in out.splitlines()
                     if l.startswith('---- ') or l.startswith('--- FAIL')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
