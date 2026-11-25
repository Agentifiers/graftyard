#!/usr/bin/env python3
"""sshfront-image: planted bugs in the SSH front's restart on its version's image (`workers/site/ssh.mjs`, `up`) and in
graft-ops' `SshHostKeys` front probe (`workers/ops/src/lib.rs`, `ssh_fronts_ready`; `decisions/DECISIONS.md`,
"`sshfront-image`"). Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Never run it under `timeout`: a kill leaves the mutant in place.
Run from the repository root:

    python3 tests/m0carried/plant214.py          # every mutant
    python3 tests/m0carried/plant214.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
SSH = 'workers/site/ssh.mjs'
OPS = 'workers/ops/src/lib.rs'
NODE = ['node', '--test', 'workers/site/tests/ssh.test.mjs']
PROBE = ['cargo', 'test', '-q', '-p', 'graftyard-ops', '--lib', '--', 'ssh_fronts_ready']

MUTANTS = [
    ("the front never restarts on another image", SSH,
     '    if (ran === image) return;',
     '    return;', NODE),
    ("the front restarts on every session, even under the same image", SSH,
     '    if (ran === image) return;',
     '    if (false) return;', NODE),
    ("a running front with no recorded image keeps running", SSH,
     '    if (ran === image) return;',
     '    if (ran === image || ran === undefined) return;', NODE),
    ("the probe accepts a running old graftyard front", OPS,
     '            .and_then(|b| b.trim().strip_prefix("SSH-2.0-graftyard_"))',
     '            .map(|b| b.trim().strip_prefix("SSH-2.0-graftyard_").unwrap_or("2"))', PROBE),
    ("the probe accepts generation 1", OPS,
     'pub const SSHD_VIEW_GENERATION: u32 = 2;',
     'pub const SSHD_VIEW_GENERATION: u32 = 1;', PROBE),
    ("the probe passes a running front with no banner", OPS,
     '        if !generation.is_some_and(|n| n >= SSHD_VIEW_GENERATION) {',
     '        if f.banner.is_some() && !generation.is_some_and(|n| n >= SSHD_VIEW_GENERATION) {', PROBE),
    ("the probe judges a stopped front's banner", OPS,
     '        if !f.running {\n            continue;\n        }\n',
     '', PROBE),
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
