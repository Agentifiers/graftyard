#!/usr/bin/env python3
"""lease-clock: planted bugs in the member clock's floor, the read-lease guard, and the edge's retry of a shed spread
read (`decisions/DECISIONS.md`, "`lease-clock`"). Plants one mutant at a time, runs the tests that guard it in release,
and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant15.py          # every mutant
    python3 tests/m0carried/plant15.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
CLOCK = 'workers/site/src/clock.rs'
LOOKUP = 'crates/lookup/src/lib.rs'
RAFT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft', '--test', 'scenarios']
SITE = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-site', '--lib', 'clock']
LOOK = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-lookup', '--lib']

MUTANTS = [
    ('the guard is ignored when a read starts', LEADER,
     '            && !self.clock_suspect\n            && now < Timestamp(lead.lease_end',
     '            && now < Timestamp(lead.lease_end', RAFT),
    ('the guard trips only past DRIFT / 2', CLOCK,
     'self.lag() >= LAG_SUSPECT_MS', 'self.lag() > LAG_SUSPECT_MS', SITE),
    ('the guard reads only the newest lag', CLOCK,
     'self.lags.iter().copied().max().unwrap_or(0)', 'self.lags.back().copied().unwrap_or(0)', SITE),
    ('a stamp sets the floor, older or not', CLOCK,
     'self.floor = self.floor.max(stamp);', 'self.floor = stamp;', SITE),
    ('the clock is not monotonic', CLOCK,
     'self.last = self.last.max(own).max(self.floor);', 'self.last = own.max(self.floor);', SITE),
    ('the floor is never applied', CLOCK,
     'self.last = self.last.max(own).max(self.floor);', 'self.last = self.last.max(own);', SITE),
    ('a shed read is never retried', LOOKUP,
     'Err((_, ms)) if now() + ms < until => wait(ms).await,', 'Err((_, ms)) if false => wait(ms).await,', LOOK),
    ('a shed read is retried past its deadline', LOOKUP,
     'Err((_, ms)) if now() + ms < until => wait(ms).await,', 'Err((_, ms)) if now() < until => wait(ms).await,', LOOK),
    ('the retry ignores the members\' wait', LOOKUP,
     'Spread::Shed(wait.max().unwrap_or(OVERLOAD_RETRY_MS).clamp(50, 2_000))', 'Spread::Shed(OVERLOAD_RETRY_MS)', LOOK),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
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
