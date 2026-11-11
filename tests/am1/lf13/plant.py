#!/usr/bin/env python3
"""lf13-m3c: planted bugs in this branch's two feed fixes, on `tests/am1/rest/plant.py`'s runner. Plants one mutant at a
time, runs the test that gates it, and restores the file byte for byte. Run from the worktree root:

    python3 tests/am1/lf13/plant.py          # every mutant
    python3 tests/am1/lf13/plant.py 1        # by number
"""
import importlib.util, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location('rest_plant', os.path.join(HERE, '..', 'rest', 'plant.py'))
rest = importlib.util.module_from_spec(spec)
# Load the runner without running its own mutants: its loop runs only the numbers on argv, so give it one that matches none.
argv, sys.argv = sys.argv, [sys.argv[0], '-1']
spec.loader.exec_module(rest)
sys.argv = argv

SITE_FEED = 'workers/site/src/feed.rs'
RING = 'crates/git/src/feed.rs'
SITE = ['cargo', '-p', 'graftyard-site', '--lib', 'feed']
GATES = ['cargo', '-p', 'graftyard-git', '--test', 'lease_gates', 'catch_up']

MUTANTS = [
    ('site feed: a held poll waits again without checking for a stop since its admission', SITE_FEED,
     '        if self.stops != since {\n            return None;\n        }\n',
     '        let _ = since;\n', SITE),
    ('site feed: a stop counts only when it drained a waiter', SITE_FEED,
     '        for w in self.waiters.drain(..) {\n            let _ = w.send(stop);\n        }\n        if stop {\n            self.stops += 1;\n',
     '        let woke = !self.waiters.is_empty();\n        for w in self.waiters.drain(..) {\n            let _ = w.send(stop);\n        }\n        if stop {\n            self.stops += u64::from(woke);\n',
     SITE),
    ('ring: a lease ended later in the same input reads as ended at its acquire', RING,
     '|l: &Lease| crate::lease::lease_row(core, l.id).is_none() && !released.contains(&l.id) && at < l.expires;',
     '|l: &Lease| false && crate::lease::lease_row(core, l.id).is_none();', GATES),
    ('ring: a lease acquired in a batch is not kept live past its acquire', RING,
     '                && ended_later(lease)\n            {\n                now.insert(lease.id, lease.clone());',
     '                && ended_later(lease)\n            {\n                let _ = lease;', GATES),
]

only = sys.argv[1:] and set(int(x) for x in sys.argv[1:])
for n, (name, path, old, new, args) in enumerate(MUTANTS):
    if not only or n in only:
        rest.plant(n, name, path, old, new, args)
