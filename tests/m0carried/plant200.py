#!/usr/bin/env python3
"""M6-WP3, `lifeboat-turns` (`decisions/DECISIONS.md`): planted bugs in the lifeboat puller's turns and budgets
(`lifeboat/src/puller.rs`, `Puller::pulls`; `lifeboat/src/pass.rs`, `pass` and `in_turns`) and the test-faults feed
hold (`workers/site/src/registry.rs`, `/export_round`). Plants one mutant at a time, runs the tests that guard it, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant200.py          # every mutant
    python3 tests/m0carried/plant200.py 0 2      # by number

The feed hold's mutant is caught only by `tests/exportbed/run.sh` (it builds the site, about 11 minutes), not by a
native test: `python3 tests/m0carried/plant200.py 8` runs it, and `HOLD=1` with no numbers runs every mutant.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

PULL = 'lifeboat/src/puller.rs'
PASS = 'lifeboat/src/pass.rs'
REG = 'workers/site/src/registry.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-lifeboat']
BED = ['bash', 'tests/exportbed/run.sh']

MUTANTS = [
    ("stale pulls start at the first chain in key order every pass", PULL,
     "            due.into_iter().partition(|c| self.turn.is_some_and(|t| *c > t));\n",
     "            due.into_iter().partition(|_| true);\n", T),
    ("the probe budget is the data budget", PASS,
     "        if r.pages >= PAGES_PER_PASS || r.probes >= PROBES_PER_PASS {\n",
     "        if r.pages >= PAGES_PER_PASS || r.probes >= PAGES_PER_PASS {\n", T),
    ("a cycle in which a chain did not answer counts as current", PASS,
     "                if !st.puller.cycle_dirty && st.puller.behind.is_empty() && st.puller.halted.is_empty() {\n",
     "                if st.puller.behind.is_empty() && st.puller.halted.is_empty() {\n", T),
    ("a stale pass that knows chains is current at its own end", PASS,
     "    if settled && (if r.stale { last.is_none() } else { r.feed_error.is_none() }) {\n",
     "    if settled && (r.stale || r.feed_error.is_none()) {\n", T),
    ("owed packs are tried in key order, not by network", PASS,
     "    let (later, earlier): (Vec<_>, Vec<_>) = turns.into_iter().partition(|(t, _)| after.is_some_and(|a| *t > a));\n"
     "    let mut queue: VecDeque<VecDeque<Owed>> = later.into_iter().chain(earlier).map(|(_, q)| q).collect();\n"
     "    let mut out = Vec::new();\n",
     "    let _ = after;\n    return owed.iter().take(n).copied().collect();\n"
     "    #[allow(unreachable_code)]\n"
     "    let mut queue: VecDeque<VecDeque<Owed>> = turns.into_values().collect();\n"
     "    let mut out = Vec::new();\n", T),
    ("a gone network's owed packs are kept", PASS,
     "                    st.owed.retain(|o| o.network != p.network);\n",
     "                    st.owed.retain(|_| true);\n", T),
    ("a pack unserved on a live chain is not counted toward the alarm", PASS,
     "                    st.unserved.entry(p.id).or_insert(now);\n",
     "                    let _ = now;\n", T),
    ("the unserved alarm fires at once", PASS,
     "    st.unserved.values().filter(|t| now.saturating_sub(**t) >= UNSERVED_ALARM_MS).count()\n",
     "    st.unserved.values().filter(|_| now > 0).count()\n", T),
    ("a held feed still claims rounds", REG,
     "                if export_held(&sql)? {\n",
     "                if false && export_held(&sql)? {\n", BED),
]

if __name__ == '__main__':
    # With no numbers, every native mutant; the feed hold's only with HOLD=1.
    if len(sys.argv) == 1 and not os.environ.get('HOLD'):
        sys.argv += [str(i) for i in range(len(MUTANTS) - 1)]
    run(MUTANTS)
