#!/usr/bin/env python3
"""confirm-storage: planted bugs in which time `STEP_DOWN`'s confirm rule judges (`decisions/DECISIONS.md`,
"`confirm-storage`"; B0-33 m60's Confirm trips on quick syncs, `tests/am1/results/round-time.md`). Plants one mutant at
a time, runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT
COMPILE. Run from the repository root:

    python3 tests/m0carried/plant53.py          # every mutant
    python3 tests/m0carried/plant53.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
HEALTH = 'crates/raft/src/member/health.rs'
LEAD = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'leadership', '--', 'a_long_wait_for_the_flush',
        'slow_storage_still', 'a_quick_storage_confirm', 'six_slow_confirms']

MUTANTS = [
    ("main's rule: the sample measured from the step again", HEALTH,
     '        let mut ms: Vec<u32> = lead.health.confirms.iter().map(|c| c.storage).collect();\n',
     '        let mut ms: Vec<u32> = lead.health.confirms.iter().map(|c| c.whole).collect();\n', LEAD),
    ("the host's storage time ignored", HEALTH,
     '            let storage = times.map_or(ms, |t| t.storage_ms.min(ms));\n',
     '            let storage = times.map_or(ms, |_| ms);\n', LEAD),
    ("the own-stall check moved to the flush's begin", HEALTH,
     '            *end = Some(now);\n            let ms = now.0.saturating_sub(began.0) as u32;\n',
     '            *end = Some(now);\n'
     '            if let Some(t) = times {\n'
     '                *began = Timestamp(now.0.saturating_sub(u64::from(t.storage_ms)));\n'
     '            }\n'
     '            let ms = now.0.saturating_sub(began.0) as u32;\n', LEAD),
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
