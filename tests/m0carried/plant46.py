#!/usr/bin/env python3
"""F91, the edge's ls-refs on learners and followers: planted bugs in its read-your-writes and one-state rules
(`decisions/DECISIONS.md`, "F91: the edge's `ls-refs` reads its refs from learners and followers"). Plants one mutant
at a time, runs the test that guards it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID
NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant46.py          # every mutant
    python3 tests/m0carried/plant46.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
RAFT = 'crates/raft/src/member/install.rs'
LS = 'crates/git/src/ls_refs.rs'
FAM = 'workers/site/src/family.rs'
LST = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'ls_refs']
FANT = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'a_follower_serves_only']

MUTANTS = [
    ('a follower answers below its floor, at its own applied index', RAFT,
     '            if i <= applied {\n', '            if true {\n', LST),
    ('HEAD and its branch from two states are answered', LS,
     '                if line.is_some_and(|o| Some(o) != h.oid.as_ref()) {\n',
     '                if false && line.is_some_and(|o| Some(o) != h.oid.as_ref()) {\n', LST),
    ('the member leaves HEAD out of its read', LS,
     '        .map(|b| HeadRead {', '        .filter(|_| false)\n        .map(|b| HeadRead {', LST),
    ('a follower declines the ls-refs read, so it never leaves the leader', FAM,
     '    if q.get("lsRefs").is_some() || *q == serde_json::json!({ "ranges": true }) {\n',
     '    if *q == serde_json::json!({ "ranges": true }) {\n', FANT),
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
