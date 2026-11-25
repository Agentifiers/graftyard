#!/usr/bin/env python3
"""`feed-retired` (`decisions/DECISIONS.md`): planted bugs in the purge's wait for the change feed and its 1 h cap
(`crates/git/src/export.rs`, `purge_wait`), the round's rule for dropping a retired chain's report (`forgets`), the
source read's not-joined answer (`crates/lookup/src/lib.rs`, `log_rows`), and the registry's report drop
(`workers/site/src/export_feed.rs`, `REPORT_DROP`). Plants one mutant at a time, runs the tests that guard it, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant230.py          # every native mutant
    python3 tests/m0carried/plant230.py 0 2      # by number

The round's drop and the lookup's not-joined answer are caught only by `tests/exportbed/run.sh` (it builds the site,
about 11 minutes): `python3 tests/m0carried/plant230.py 6 7` runs them, and `BED=1` with no numbers runs every mutant.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

EXP = 'crates/git/src/export.rs'
LOOK = 'crates/lookup/src/lib.rs'
FEED = 'workers/site/src/export_feed.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'export']
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'export_feed']
BED = ['bash', 'tests/exportbed/run.sh']

MUTANTS = [
    ("an install with no feed waits for one", EXP,
     "    let Some(at) = cursor else { return PurgeWait::Go };\n",
     "    let at = cursor.unwrap_or(Index(0));\n", T),
    ("a purge goes while the feed is still behind", EXP,
     "    if at >= commit {\n        return PurgeWait::Go;\n    }\n    let waited",
     "    if at.0 + 2 >= commit.0 {\n        return PurgeWait::Go;\n    }\n    let waited", T),
    ("a stalled feed holds a purge for good", EXP,
     "    if waited >= PURGE_FEED_WAIT_MS {\n",
     "    if waited >= PURGE_FEED_WAIT_MS && cursor.is_none() {\n", T),
    ("the cap's unexported range starts at the cursor, not after it", EXP,
     "        return PurgeWait::Late { from: Index(at.0 + 1), to: commit };\n",
     "        return PurgeWait::Late { from: at, to: commit };\n", T),
    ("a round drops a report on any miss with the retire proof", EXP,
     "    unread == Unread::Unjoined && retired\n",
     "    retired\n", T),
    ("the report drop deletes nothing", FEED,
     'pub(crate) const REPORT_DROP: &str = "DELETE FROM export_report WHERE chain = ?";\n',
     'pub(crate) const REPORT_DROP: &str = "DELETE FROM export_report WHERE chain = ? AND 0";\n', SITE),
    ("a round never drops a retired chain's report", FEED,
     "        if export::forgets(export::Unread::Unjoined, retired) {\n",
     "        if export::forgets(export::Unread::Missed, retired) {\n", BED),
    ("the source read counts no 409 as not joined", LOOK,
     "                409 => Some(Err(true)),\n",
     "                409 => Some(Err(false)),\n", BED),
]

if __name__ == '__main__':
    # With no numbers, every native mutant; the bed's only with BED=1.
    if len(sys.argv) == 1 and not os.environ.get('BED'):
        sys.argv += [str(i) for i in range(len(MUTANTS) - 2)]
    run(MUTANTS)
