#!/usr/bin/env python3
"""liveness-slow-storage: planted bugs in the slow-storage campaign gate (`storage_slow`, `appended_at`) and the
hand-off filter (`peer_storage_slow`), `tests/m0/results/slow-storage.md`. Plants one mutant at a time, runs the raft
tests in release, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the
repository root:

    python3 tests/m0carried/plant11.py          # every mutant
    python3 tests/m0carried/plant11.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
ELECTION = 'crates/raft/src/member/election.rs'
FOLLOWER = 'crates/raft/src/member/follower.rs'
HEALTH = 'crates/raft/src/member/health.rs'
RAFT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft']

MUTANTS = [
    ('no campaign gate', ELECTION,
     'if self.now < after(self.appended_at, CAMPAIGN_DEADLINE) && self.storage_slow() {', 'if false {'),
    ('the gate ignores the leader (holds a slow member forever)', ELECTION,
     'if self.now < after(self.appended_at, CAMPAIGN_DEADLINE) && self.storage_slow() {',
     'if self.storage_slow() {'),
    ('the gate ignores the storage (holds every member)', ELECTION,
     'if self.now < after(self.appended_at, CAMPAIGN_DEADLINE) && self.storage_slow() {',
     'if self.now < after(self.appended_at, CAMPAIGN_DEADLINE) {'),
    ('appended_at never set', FOLLOWER, '        self.appended_at = self.now;\n', ''),
    ('storage_slow counts only one slow confirm', HEALTH,
     'p99 > u64::from(STEP_DOWN.confirm_p99_ms) && slow >= usize::from(STEP_DOWN.confirm_slow_min)', 'slow >= 1'),
    ('storage_slow ignores running confirms', HEALTH, '.filter(|(_, e)| e.is_none_or(|e| e >= since))',
     '.filter(|(_, e)| e.is_some_and(|e| e >= since))'),
    ('no hand-off filter', HEALTH, ' && !self.peer_storage_slow(*s))', ')'),
    ('the hand-off filter refuses every follower', HEALTH,
     '        slow >= usize::from(STEP_DOWN.commit_fail_min)\n', '        slow >= 0\n'),
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
