#!/usr/bin/env python3
"""R57: planted bugs in the bindings `graft-ops` uploads a version with (`crates/release/src/ops.rs`, `upload_source` and
`upload_metadata`; `decisions/DECISIONS.md`, "`deployed-bindings`"). Plants one mutant at a time, runs the release
crate's R57 test, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the
repository root:

    python3 tests/m0carried/plant212.py          # every mutant
    python3 tests/m0carried/plant212.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
OPS = 'crates/release/src/ops.rs'
REL = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', 'r57_']

MUTANTS = [
    ("the source is the deployment's first version, N+1 at 1% here", OPS,
     '    split.iter().max_by_key(|(_, p)| *p).map(|(v, _)| *v)',
     '    split.first().map(|(v, _)| *v)', REL),
    ("main's behaviour: every binding type kept from the newest upload, none sent", OPS,
     '        "bindings": bindings,\n        "keep_bindings": KEPT_BINDINGS,',
     '        "bindings": Vec::<Value>::new(),\n        "keep_bindings": ["plain_text", "secret_text", "r2_bucket"],\n        "_unused": bindings,',
     REL),
    ("a secret's binding is sent with no value", OPS,
     '        .filter(|b| !KEPT_BINDINGS.contains(&b["type"].as_str().unwrap_or("")))\n',
     '', REL),
    ("the runtime settings are not copied", OPS,
     '    for k in ["compatibility_date", "compatibility_flags", "limits", "containers"] {\n        if !rt[k].is_null() {\n            meta[k] = rt[k].clone();',
     '    for k in ["compatibility_date", "compatibility_flags", "limits", "containers"] {\n        if rt[k].is_null() {\n            meta[k] = rt[k].clone();',
     REL),
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
