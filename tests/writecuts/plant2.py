#!/usr/bin/env python3
"""write-cuts cut (2): planted bugs in the `ExtendLeases` floor (`crates/raft/src/member/leader.rs`, `take_extend`, and
the marker rule in `crates/raft/src/member/log.rs`, `set_commit`). Plants one mutant at a time, runs the tests that
guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the
repository root:

    python3 tests/writecuts/plant2.py          # every mutant
    python3 tests/writecuts/plant2.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
LOG = 'crates/raft/src/member/log.rs'
FLOOR = '(by >= crate::EXTEND_MIN).then_some(ExtendLeases { since, by })'
TESTS = [
    ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'scenarios', '--', 'extend_min', 'quick_campaign'],
    ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'lease_gates', '--', 'extend_min'],
    ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'gates', '--', 'extend_min'],
]

MUTANTS = [
    ('`>` for `>=`: a stall of exactly EXTEND_MIN stamps nothing', LEADER, FLOOR,
     '(by > crate::EXTEND_MIN).then_some(ExtendLeases { since, by })'),
    ('the floor judges the gap since the last entry, not the stall from `since`', LEADER, FLOOR,
     '(std::time::Duration::from_millis(self.now.0.saturating_sub(last.0)) >= crate::EXTEND_MIN)'
     '.then_some(ExtendLeases { since, by })'),
    ('the marker is kept after a dropped stamp, so the next extend covers the dropped stall and the idle time after it',
     LOG, '&& (e.extend.is_some() || e.at > m)', '&& e.extend.is_some()'),
    ('no floor (main)', LEADER, FLOOR, '(!by.is_zero()).then_some(ExtendLeases { since, by })'),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        failed, out = [], ''
        try:
            open(p, 'w').write(text.replace(old, new))
            for cmd in TESTS:
                r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
                out += r.stdout + r.stderr
                if r.returncode != 0:
                    failed.append(cmd[4])
        finally:
            open(p, 'wb').write(orig)
        if 'error[E' in out or 'could not compile' in out:
            verdict = 'DID NOT COMPILE'
        elif failed:
            verdict = f'CAUGHT ({", ".join(failed)})'
            caught += 1
        else:
            verdict = 'MISSED'
        print(f'{i} {name}: {verdict}', flush=True)
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
