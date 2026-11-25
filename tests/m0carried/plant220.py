#!/usr/bin/env python3
"""AM3-WP1, LF-8 and the agents view's rules (`crates/release/src/agents.rs`, `AgentsReleaseView::extend`; the
group's `decide`, which the view replays): another install's entry taken, a `Keys` taken under any key, an entry taken
out of index order, the view replayed over entries it holds (moving it to a lower index), a refused refresh leaving a
half-taken view, and the controller override set with no pointer. Run from the repository root:

    python3 tests/m0carried/plant220.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import REL, run  # noqa: E402

AG = 'crates/release/src/agents.rs'
GROUP = 'crates/release/src/group.rs'
LIB = 'crates/release/src/lib.rs'
MUTANTS = [
    ("LF-8: another install's entry is taken", GROUP,
     '        if b.install != genesis.install {\n            return Err(Refusal::WrongInstall',
     '        if false && b.install != genesis.install {\n            return Err(Refusal::WrongInstall', REL),
    ("LF-8: another install's genesis is taken by a cold view", LIB,
     '    if b.install != anchor.install {\n        return Err(Refusal::WrongInstall { want: anchor.install, got: b.install });\n    }\n    if g.install',
     '    if g.install', REL),
    ("LF-8: a Keys is taken under any key of the latest Keys", GROUP,
     '                    if !want.contains(key) {', '                    if role(k, key).is_none() {', REL),
    ("LF-8: an entry out of index order is taken", AG,
     '            if index.0 != self.fold.count + 1 {', '            if false {', REL),
    ("an entry the view holds is replayed over it, moving it to a lower index", AG,
     '            if index.0 <= held {\n', '            if index.0 <= held && false {\n', REL),
    ("a refresh whose entry is refused keeps nothing it held", AG,
     '            self.fold.check(&a, *at, &ev)?;',
     '            if let Err(e) = self.fold.check(&a, *at, &ev) {\n                *self = AgentsReleaseView::default();\n'
     '                return Err(e);\n            }', REL),
    ("the controller override is set with no pointer", AG,
     "    view.controller().map(|v| version_override(controller_script, v))",
     "    Some(version_override(controller_script, view.controller().unwrap_or(VersionId([0; 16]))))", REL),
]

if __name__ == '__main__':
    run(MUTANTS)
