#!/usr/bin/env python3
"""K11, the live-row check: planted bugs in its digest, its yield rules and its bound (`decisions/DECISIONS.md`,
"`live-check`"). Plants one mutant at a time, runs the test that guards it, and restores the file byte for byte. Each
line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant47.py          # every mutant
    python3 tests/m0carried/plant47.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
SNAP = 'crates/fold/src/snapshot.rs'
LIVE = 'workers/site/src/live.rs'
FOLDT = ['cargo', 'test', '-q', '-p', 'graftyard-fold', '--test', 'tiers', '--', 'caught_by_the_live_check']
SITET = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'live::tests']

MUTANTS = [
    ('the check compares the trailer with itself, not with the sum apply kept', SNAP,
     '                want: self.want(store),\n', '                want: tr.fold_digest,\n', FOLDT),
    ('a check whose tier an install replaced still gives a verdict', SNAP,
     '        try_meta(store).is_ok_and(|m| m.snap == Some(self.cursor.at))\n', '        true\n', FOLDT),
    ('the check reads live rows only, without snap_undo, so apply during the scan moves it', SNAP,
     '            out.extend(copies);\n', '            let _ = copies;\n', FOLDT),
    ('the undo bound abandons at the cap, not past it', SNAP,
     '    undo > LIVE_UNDO_MAX\n', '    undo >= LIVE_UNDO_MAX\n', FOLDT),
    ('a check starts while the leader holds queued commands', LIVE,
     '        && !b.leads_queued\n', '', SITET),
    ('a check starts while a snapshot job runs', LIVE,
     '        && !b.snapshot\n', '', SITET),
    ('a check starts at every visit, not once a week', LIVE,
     '        && b.last_ms.is_none_or(|l| now >= l + LIVE_EVERY_MS)\n', '', SITET),
    ('the cron never reads a mismatching tier', LIVE,
     '.filter(|t| t["matched"] == json!(false))', '.filter(|_| false)', SITET),
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
