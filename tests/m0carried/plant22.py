#!/usr/bin/env python3
"""feed-ahead: planted bugs in a feed cursor past the member's last entry (`decisions/DECISIONS.md`, "`feed-ahead`").
Plants one mutant at a time, runs the test that guards it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant22.py          # every mutant
    python3 tests/m0carried/plant22.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
FEED = 'crates/git/src/feed.rs'
RUST = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'lease_gates', 'cursor']
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', 'feed::tests']
SITE_RS = 'workers/site/src/feed.rs'

MUTANTS = [
    ('a cursor ahead is gone again', FEED,
     '        Some(c) if c.group == group && c.index > newest && c.index <= committed => Start::Ahead(c.index),\n', '',
     RUST),
    ('a cursor ahead is served as inside the ring', FEED,
     'c.index <= committed => Start::Ahead(c.index)', 'c.index <= committed => Start::After(c.index)', RUST),
    ('a cursor at the newest entry is ahead', FEED,
     'c.index > newest && c.index <= committed', 'c.index >= newest && c.index <= committed', RUST),
    ('a cursor below the floor is served', FEED,
     'c.index >= oldest && c.index <= newest => Start::After', 'c.index <= newest => Start::After', RUST),
    ('a member behind the cursor still follows', FEED,
     'Following { lag: self.lag.max(FEED_BATCH + Duration::from_millis(1)), ..self }', 'self', RUST),
    # The bound (feed-ahead review): a cursor past every index the member knows committed, as after a restore.
    ('a cursor past the commit index is ahead (feed-ahead 41c93e72)', FEED,
     'c.index > newest && c.index <= committed => Start::Ahead', 'c.index > newest => Start::Ahead', RUST),
    ('the bound is one past the commit index', FEED,
     'c.index > newest && c.index <= committed => Start::Ahead', 'c.index > newest && c.index <= Index(committed.0 + 1) => Start::Ahead', RUST),
    ('the member bounds by its own applied entry alone', SITE_RS,
     'feed::start(repo, p.cursor.as_deref(), oldest, newest, committed.max(newest))',
     'feed::start(repo, p.cursor.as_deref(), oldest, newest, Index(u64::MAX))', SITE),
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
