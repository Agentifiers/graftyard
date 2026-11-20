#!/usr/bin/env python3
"""authz-source: planted bugs in the edge's count of where a committed issue command's reply took the owner's `Authz`
(`decisions/DECISIONS.md`, "`leader-render`"). Plants one mutant at a time, runs the test that guards it, and restores
the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant69.py
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
API = 'workers/edge/src/api.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'authz_source']

MUTANTS = [
    ("a member that names no source is not counted as the fallback", API,
     '        return Some("fallback");', '        return None;', TESTS),
    ("every named source counts as dep", API,
     '        "render" => Some("render"),\n', '        "render" => Some("dep"),\n', TESTS),
    ("a refusal or merge counts as the fallback", API,
     '    if graftyard_api::hosted::rendered_without_authz(done) {\n        return Some("fallback");',
     '    if done.pointer("/ok/out/issue/authz").is_none() {\n        return Some("fallback");', TESTS),
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
