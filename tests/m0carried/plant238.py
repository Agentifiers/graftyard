#!/usr/bin/env python3
"""`lifeboat-clone` (`decisions/DECISIONS.md`): planted bugs in the lifeboat git copy's readiness rule
(`lifeboat/src/git.rs`, `ready`), the networks a pass catches up (`GitCopy::due`), the puller's drop of a held roll's
consumed packs (`lifeboat/src/pass.rs`, `rolled_off`), and the puller's check of a repository after `graft-git`
restarts (`workers/lifeboat/src/git.rs`, `turn`). Plants one mutant at a time, runs the tests that guard it, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant238.py          # every native mutant
    python3 tests/m0carried/plant238.py 0 2      # by number

The restart check is caught only by `tests/lifeboatbed/run.sh` (check 8; it builds the site, the edge, the lifeboat
and the static git, about 15 minutes): `python3 tests/m0carried/plant238.py 7` runs it, and `BED=1` with no numbers
runs every mutant.

Run on branch `lifeboat-clone` (2026-11-25): 0 to 6 caught 7 of 7 natively, and 7 was caught by lifeboatbed.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

GIT = 'lifeboat/src/git.rs'
PASS = 'lifeboat/src/pass.rs'
WGIT = 'workers/lifeboat/src/git.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-lifeboat']
BED = ['bash', 'tests/lifeboatbed/run.sh']

MUTANTS = [
    ("a repository serves while a rebuild is in progress", GIT,
     "    if f.rehydrating {\n        return wait(\"the git copy is rebuilding this repository\");\n    }\n",
     "", T),
    ("a cursor written on another disk is trusted", GIT,
     "    if f.cursor_disk.as_ref() != Some(disk) {\n",
     "    if f.cursor_disk.is_none() {\n", T),
    ("a repository serves after a restart before it is checked", GIT,
     "        Some(Net { served: Some((at, when)), checked: Some(c), .. }) if c == boot => {\n",
     "        Some(Net { served: Some((at, when)), .. }) => {\n", T),
    ("a halted copy is told to retry", GIT,
     "    if f.halted {\n        return Ready::Halted;\n    }\n",
     "    if f.halted {\n        return wait(\"halted\");\n    }\n", T),
    ("a network is not caught up again after a restart", GIT,
     "                    && (g.checked.as_deref() != Some(boot) || g.served.is_none_or(|(s, _)| s < *at))\n",
     "                    && g.served.is_none_or(|(s, _)| s < *at)\n", T),
    # Not planted: `roll_markers` at `r.from` instead of `r.to` is equivalent, because `catch_up` orders and loads a
    # roll's produced pack by its record's range, never by its marker's index.
    ("a roll's consumed packs stay owed once its produced pack is held", PASS,
     "        st.owed.retain(|o| !consumed.contains(&o.id));\n",
     "", T),
    ("a roll's consumed packs are dropped before its produced pack is held", PASS,
     "    let held: Vec<PackId> = st.rolls.keys().filter(|p| !st.owed.iter().any(|o| o.id == **p)).copied().collect();\n",
     "    let held: Vec<PackId> = st.rolls.keys().copied().collect();\n", T),
    # Not planted: the edge's 503 with no Retry-After for graft-git's 404. The puller answers not ready before the
    # edge calls graft-git, so that branch covers only a rebuild starting between the two calls, which no bed hits.
    # Not planted: a lifeboat member that truncates. It binds no bucket, so `truncate` finds 0 copies and truncates
    # nothing either way; the guard only spares the error BF-53 logged, and check 8 commits no `Archived`.
    ("a restart check that errs keeps the cursor of a lost repository", WGIT,
     "                && let Err(why) = io.connected(false).await.unwrap_or_else(|e| Err(e.0))\n",
     "                && let Err(why) = io.connected(false).await?\n", BED),
]

if __name__ == '__main__':
    # With no numbers, every native mutant; the bed's only with BED=1.
    if len(sys.argv) == 1 and not os.environ.get('BED'):
        sys.argv += [str(i) for i in range(len(MUTANTS) - 1)]
    run(MUTANTS)
