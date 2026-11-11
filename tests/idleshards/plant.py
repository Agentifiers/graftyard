#!/usr/bin/env python3
"""idle-shards: planted bugs in this branch's new code (the dispatcher's park, the rehydrate's order, and the warm
call that starts no shard). Plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/idleshards/plant.py          # every mutant
    python3 tests/idleshards/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
PR = 'crates/git/src/prsync.rs'
ING = 'crates/git/src/ingest.rs'
SH = 'crates/git/src/shard.rs'
PRSYNC = ['-p', 'graftyard-git', '--test', 'prsync']
INGEST = ['-p', 'graftyard-git', '--lib', 'ingest']
SHARD = ['-p', 'graftyard-git', '--test', 'shard']

MUTANTS = [
    ('park: never parks', PR, '        if self.tries >= PARK_AFTER {\n            return None;',
     '        if self.tries >= u32::MAX {\n            return None;', PRSYNC),
    ('park: the count never resets when the cursor moves', PR,
     '            self.tried_at = cursor;\n            self.tries = 0;', '            self.tried_at = cursor;', PRSYNC),
    ('park: off by one, PARK_AFTER + 1 batches go out', PR, '        if self.tries >= PARK_AFTER {',
     '        if self.tries > PARK_AFTER {', PRSYNC),
    ('park: a parked chain keeps re-arming the alarm', PR,
     '        !self.parked(cursor) && (self.sent > cursor', '        (self.sent > cursor', PRSYNC),
    ('park: a restored park is ignored', PR, '        self.tries = PARK_AFTER;\n    }', '        let _ = PARK_AFTER;\n    }',
     PRSYNC),
    ('rehydrate: the marked phase loads every marked pack first, as before', ING,
     '        markers.iter().filter(|(id, m)| !consumed.contains(id) && !in_tail(id, m)).collect();',
     '        markers.iter().filter(|(id, m)| !consumed.contains(id) && !in_tail(id, m) || true).collect();', INGEST),
    ('rehydrate: the promoted pack is left to the tail too', ING,
     '                && p.as_ref().and_then(|p| p.pack.stored_id()) != Some(*id)', '', INGEST),
    ('rehydrate: the tail starts at the promoted range\'s first index', ING,
     '                && m.index >= tail_start(p.as_ref())', '                && m.index >= p.as_ref().map_or(Index(1), |p| p.from)',
     INGEST),
    ('warm: a warm call starts a stopped shard', SH, '        Caller::Warm => Serve::Skip,',
     '        Caller::Warm => Serve::Run { touch: false },', SHARD),
    ('warm: a warm call keeps the shard warm', SH, '        Caller::Warm if running => Serve::Run { touch: false },',
     '        Caller::Warm if running => Serve::Run { touch: true },', SHARD),
]

ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')


def run(t):
    r = subprocess.run(['cargo', 'test', '-q'] + t, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1200)
    out = r.stdout + r.stderr
    return r.returncode, out, 'error[E' in out or 'could not compile' in out


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, t = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            code, out, broke = run(t)
        finally:
            open(p, 'wb').write(orig)
        if broke:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif code != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
