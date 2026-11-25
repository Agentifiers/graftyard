#!/usr/bin/env python3
"""The lifeboat's weekly scrub and workerd pin (`decisions/DECISIONS.md`, "`lifeboat-bundle`", "The weekly scrub and
the workerd pin"): planted bugs in when the puller scrubs (`lifeboat/src/pass.rs`, `scrub_due`), how it reads
graft-blob's answer (`scrubbed`), and the start script's and the bundle's version checks (`cli/src/lifeboat.rs`,
`start_script` and `pin_state`). Plants one mutant at a time, runs the tests that guard it, and restores the file byte
for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant232.py          # every mutant
    python3 tests/m0carried/plant232.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

PASS = 'lifeboat/src/pass.rs'
CLI = 'cli/src/lifeboat.rs'
LB = ['cargo', 'test', '-q', '-p', 'graftyard-lifeboat', '--test', 'pass']
CT = ['cargo', 'test', '-q', '-p', 'graftyard', '--bin', 'graftyard', 'lifeboat']

MUTANTS = [
    ("a scrub that did not answer is tried on every pass", PASS,
     "    now.saturating_sub(st.scrub_at) >= SCRUB_EVERY_MS && now.saturating_sub(st.scrub_tried) >= SCRUB_RETRY_MS\n",
     "    now.saturating_sub(st.scrub_at) >= SCRUB_EVERY_MS\n", LB),
    ("the scrub runs an hour after one that answered, not a week", PASS,
     "    now.saturating_sub(st.scrub_at) >= SCRUB_EVERY_MS && now.saturating_sub(st.scrub_tried) >= SCRUB_RETRY_MS\n",
     "    now.saturating_sub(st.scrub_tried) >= SCRUB_RETRY_MS\n", LB),
    ("a key graft-blob could not check is not reported", PASS,
     '            "restored" => r.restored += 1,\n            _ => {\n',
     '            "restored" => r.restored += 1,\n            "error" => {}\n            _ => {\n', LB),
    ("a lost pack names no copy to pull it from", PASS,
     "                    .map(|n| ChainId::Repo(RepoId(DbId(n))));\n",
     "                    .map(|n: i64| ChainId::Repo(RepoId(DbId(n)))).filter(|_| false);\n", LB),
    ("start.sh runs any workerd", CLI,
     'if [ "$have" != "$pin" ]; then\n',
     'if [ -z "$have" ]; then\n', CT),
    ("start.sh opens another version's state", CLI,
     'if [ "$held" != "$pin" ]; then\n',
     'if [ -z "$held" ]; then\n', CT),
    ("the bundle takes another version's state as its own", CLI,
     "        Ok(had) if had.trim() == version => Ok(()),\n",
     "        Ok(_) => Ok(()),\n", CT),
]

if __name__ == '__main__':
    run(MUTANTS)
