#!/usr/bin/env python3
"""near-check: planted bugs in the daily near check and the fold audit's chain filter (`decisions/DECISIONS.md`,
"`near-check`"). Plants one mutant at a time, runs the test that guards it, and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant28.py          # every mutant
    python3 tests/m0carried/plant28.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
MEMBER = 'workers/site/src/member.rs'
TYPES = 'crates/types/src/placement.rs'
AUDIT = 'workers/site/src/fold_audit.rs'
NEAR = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'the_daily_near_check']
SKIP = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'the_audit_skips_the_latency_probe_chain']

MUTANTS = [
    ('any served call starts the check, as on main', MEMBER,
     '    now.saturating_sub(client_at) < crate::host::DRAIN_IDLE_MS\n        && (rows', '    (rows', NEAR),
    ('a client call exactly DRAIN_IDLE_MS ago still counts', MEMBER,
     'now.saturating_sub(client_at) < crate::host::DRAIN_IDLE_MS', 'now.saturating_sub(client_at) <= crate::host::DRAIN_IDLE_MS', NEAR),
    ('the daily check reads against NEAR_RTT, as on main', MEMBER,
     '        near: graftyard_types::NEAR_RECHECK,', '        near: graftyard_types::NEAR_RTT,', NEAR),
    ('a 25% margin (125 ms)', TYPES,
     '(Region::Enam, 150), (Region::Weur, 150)', '(Region::Enam, 125), (Region::Weur, 125)', NEAR),
    ('an apac home bounded at 300', TYPES, '(Region::Apac, 297)]', '(Region::Apac, 300)]', NEAR),
    ('a note re-places only after more than a day', MEMBER,
     '(true, Some(t)) if now.saturating_sub(t) >= DAY_MS => NearStep::Replace',
     '(true, Some(t)) if now.saturating_sub(t) > DAY_MS => NearStep::Replace', NEAR),
    ('the audit samples repo:0', AUDIT, 'matches!(chain, ChainId::Repo(r) if r != PROBE)', 'matches!(chain, ChainId::Repo(_))', SKIP),
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=900)
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
