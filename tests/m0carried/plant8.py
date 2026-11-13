#!/usr/bin/env python3
"""m0-carried-8: planted bugs in the AppendEntries pipeline (`PIPELINE`), and the
loop-lag step-down (`LOOP_LAG`), `tests/m0/results/carried-8.md`. Plants one mutant at a time, runs the raft tests
in release, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the
repository root:

    python3 tests/m0carried/plant8.py          # every mutant
    python3 tests/m0carried/plant8.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
HEALTH = 'crates/raft/src/member/health.rs'
INSTALL = 'crates/raft/src/member/install.rs'
LIB = 'crates/raft/src/lib.rs'
RAFT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft']

MUTANTS = [
    ('no pipeline (PIPELINE = 1)', LEADER, 'pub const PIPELINE: usize = 4;', 'pub const PIPELINE: usize = 1;'),
    ('the bound off by one', LEADER, '&& pipe.len() < PIPELINE', '&& pipe.len() <= PIPELINE'),
    ('no bound at all', LEADER, '&& pipe.len() < PIPELINE', '&& pipe.len() < usize::MAX'),
    ('pipelining to a voter with a failed call in the window', LEADER,
     '                        && !lead.health.failed_to(peer)\n', ''),
    ('a pipelined call starts at next, not after the calls out', LEADER,
     '                    Some(through)\n', '                    Some(Index(next.0 - 1))\n'),
    ('a pipelined call starts one past the calls out', LEADER,
     '                    Some(through)\n', '                    Some(through.next())\n'),
    ('LOOP_LAG off', LIB, 'pub const LOOP_LAG: (u32, Duration, u32) = (2,',
     'pub const LOOP_LAG: (u32, Duration, u32) = (1000,'),
    ('LOOP_LAG trips on one clamped confirm', LIB, 'pub const LOOP_LAG: (u32, Duration, u32) = (2,',
     'pub const LOOP_LAG: (u32, Duration, u32) = (1,'),
    ('LOOP_LAG counts confirms that are not whole ticks', HEALTH,
     'c.1 >= at_least && c.1 % 100 == 0', 'c.1 >= at_least'),
    ('LOOP_LAG counts 100 ms ticks', HEALTH, 'c.1 >= at_least && c.1 % 100 == 0', 'c.1 >= 100 && c.1 % 100 == 0'),
    ('LOOP_LAG counts the whole STEP_DOWN window, not 2 s', HEALTH,
     'self.now.0.saturating_sub(within.as_millis() as u64)', 'self.now.0.saturating_sub((within * 5).as_millis() as u64)'),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            r = subprocess.run(RAFT, cwd=ROOT, capture_output=True, text=True, timeout=1800)
        finally:
            open(p, 'wb').write(orig)
        out = r.stdout + r.stderr
        if 'error[E' in out or 'could not compile' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
