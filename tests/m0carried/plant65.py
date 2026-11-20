#!/usr/bin/env python3
"""soft-grow: planted bugs in the leader's deep-queue soft bound (`decisions/DECISIONS.md`, "`soft-grow`"; B0-35 m62's
byte-bound cut). Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each
line prints CAUGHT, MISSED, or DID NOT COMPILE. The new test fails on main itself (checked by running it against main's
leader.rs). Run from the repository root:

    python3 tests/m0carried/plant65.py          # every mutant
    python3 tests/m0carried/plant65.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
LIB = 'crates/raft/src/lib.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'admission', '--test', 'scenarios', '--',
         'a_deep_human_queue', 'each_cut_records', 'a_backlog_splits', 'b0_33', 'entry_soft_does_not_slow']
RULE = '        let soft = if queued > DEEP_QUEUE * soft { soft.max(ENTRY_SOFT_DEEP) } else { soft };'
QUEUED = '        let queued: usize = first_lanes.iter().flat_map(|c| ad.lanes[c.lane()].iter()).map(|q| q.soft_bytes).sum();'

MUTANTS = [
    ("the bound never grows", LEADER, RULE, '        let soft = soft;', TESTS),
    ("the bound grows on any queue", LEADER, RULE,
     '        let soft = if queued > 0 { soft.max(ENTRY_SOFT_DEEP) } else { soft };', TESTS),
    ("the bound grows to ENTRY_MAX", LEADER, RULE,
     '        let soft = if queued > DEEP_QUEUE * soft { ENTRY_MAX } else { soft };', TESTS),
    ("the bound never drops back once grown", LEADER, RULE,
     '        let soft = if queued > DEEP_QUEUE * soft || ad.round_ms > 0.0 { soft.max(ENTRY_SOFT_DEEP) } else { soft };',
     TESTS),
    ("the queue counts commands, not bytes", LEADER, QUEUED,
     '        let queued: usize = first_lanes.iter().map(|c| ad.lanes[c.lane()].len()).sum();', TESTS),
    ("the deep test reads one entry, not DEEP_QUEUE", LIB,
     'pub const DEEP_QUEUE: usize = 2;', 'pub const DEEP_QUEUE: usize = 8;', TESTS),
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
