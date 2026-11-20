#!/usr/bin/env python3
"""garbled-body: planted bugs in the validate body's digests (`decisions/DECISIONS.md`, "`garbled-body`"). Plants one
mutant at a time, runs the test that guards it, and restores the file byte for byte. Each line prints CAUGHT, MISSED,
or DID NOT COMPILE. Mutant 0 puts main's rule back (the container never checks a digest), so the new Go test fails on
main. Run from the repository root:

    python3 tests/m0carried/plant69.py          # every mutant
    python3 tests/m0carried/plant69.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GO = 'containers/git/server/main.go'
EDGE = 'workers/edge/src/lib.rs'
GO_T = ['go', 'test', '-count=1', '-run', 'TestValidateBodyChangedInTransit|TestValidateDetail', './...']
EDGE_T = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'a_validate_url']

MUTANTS = [
    ("main's rule: the container checks no digest", GO,
     '\t\t\tif want == "" || got == want {', '\t\t\tif true {', GO_T),
    ("a changed body is the pack's answer, 400", GO,
     '\t\t\tfail(w, 502, "validate: body changed in transit', '\t\t\tfail(w, 400, "validate: body changed in transit',
     GO_T),
    ("the pack is never checked", GO,
     '\t\tif changed("pack", sum, query.Get("packsum"), nil) {', '\t\tif false {', GO_T),
    ("the detail is not in the container's head digest", GO,
     '\t\t\theadSum.Write(head)\n\t\t\tif changed(', '\t\t\tif changed(', GO_T),
    ("the edge sends no headsum", EDGE,
     '    url.push_str(&format!("&headsum={}", sum(head)));\n', '', EDGE_T),
    ("the edge's headsum covers the tips only", EDGE,
     '    url.push_str(&format!("&headsum={}", sum(head)));', '    url.push_str(&format!("&headsum={}", sum(&head[..tipsn])));',
     EDGE_T),
    ("the edge's packsum is the head's", EDGE,
     '        url.push_str(&format!("&packsum={}", sum(pack)));', '        url.push_str(&format!("&packsum={}", sum(head)));',
     EDGE_T),
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
        cwd = os.path.join(ROOT, 'containers/git/server') if cmd is GO_T else ROOT
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=1800)
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
