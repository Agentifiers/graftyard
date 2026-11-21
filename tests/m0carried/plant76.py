#!/usr/bin/env python3
"""learner-cadence: planted bugs in a learner's read-index floor (`raft::LEARNER_READ_EVERY`, 200 ms) and in the append
replies that ride its read calls (`workers/site/src/callers.rs`, `Acks`). Plants one mutant at a time, runs the test
that guards it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Mutant 0 is
main's floor of 50 ms. The site script's wiring (`Host::send` asking `Acks`, the ticker's `due`, `has_waiters`) runs
only in wasm; the next hosted row checks it by the learners' calls a second and the leader's `CallArrived` from them.
Run from the repository root:

    python3 tests/m0carried/plant76.py          # every mutant
    python3 tests/m0carried/plant76.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
INSTALL = 'crates/raft/src/member/install.rs'
CALLERS = 'workers/site/src/callers.rs'
RAFT = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'scenarios', '--',
        'a_learners_reads_within_the_floor_leave_as_one_batch_and_stay_linearizable']
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--',
        'a_learners_append_replies_ride_its_next_read_call_to_their_leader']

MUTANTS = [
    ("main's floor: a learner batches reads every 50 ms", INSTALL, RAFT,
     'pub const LEARNER_READ_EVERY: Duration = Duration::from_millis(200);',
     'pub const LEARNER_READ_EVERY: Duration = Duration::from_millis(50);'),
    ("a learner sends at a follower's floor", INSTALL, RAFT,
     'let every = if self.is_learner() { LEARNER_READ_EVERY } else { FOLLOWER_READ_EVERY };',
     'let every = FOLLOWER_READ_EVERY;'),
    ("an answer also grants the reads still queued for the next batch", INSTALL, RAFT,
     'let granted = earlier.into_iter().flat_map(|b| b.2).chain(reads);',
     'let granted = earlier.into_iter().flat_map(|b| b.2).chain(reads).chain(std::mem::take(&mut self.remote.queued));'),
    ("a learner's append reply leaves at once", CALLERS, SITE,
     '                self.by.entry(to).or_insert((now, Vec::new())).1.push(e);\n                Vec::new()\n',
     '                vec![e]\n'),
    ("a read call carries no held reply", CALLERS, SITE,
     '                let mut es = self.by.remove(&to).map(|(_, v)| v).unwrap_or_default();',
     '                let mut es = Vec::new();'),
    ("held replies leave alone without waiting the floor", CALLERS, SITE,
     'filter(|(_, (at, _))| now >= at + self.hold)', 'filter(|(_, (at, _))| now >= *at)'),
    ("a read call carries another member's replies", CALLERS, SITE,
     '                let mut es = self.by.remove(&to).map(|(_, v)| v).unwrap_or_default();',
     '                let mut es = self.by.pop_first().map(|(_, (_, v))| v).unwrap_or_default();'),
    ("the read goes ahead of its replies, so the call is judged as a reply", CALLERS, SITE,
     '                es.push(e);\n                es\n', '                es.insert(0, e);\n                es\n'),
]


def main():
    # A stop ends in SystemExit, so `finally` restores the file (plant75's lesson, `sync-ab.md`, "The plant left in").
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, t, old, new = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(t, cwd=ROOT, capture_output=True, text=True, timeout=1800)
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
