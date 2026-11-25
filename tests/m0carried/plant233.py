#!/usr/bin/env python3
"""AM3-WP1, LM-3's controller gate (OPEN-QUESTIONS A52, R47; `crates/release/src/gate.rs`, `accept_flush`,
`roll_versions`, `count_answer`; `crates/release/src/ops.rs`, `steps`; `workers/agents-front/src/lib.rs`, `Tally`;
`workers/edge/src/release.rs`, `errors`). The mutants: a `ControllerRoll` left pending, a flush naming a version no
release uploaded stored anyway, a flush over `FLUSH_CALLS_MAX` stored anyway, more 5xx than answers stored, a
`ControllerRoll`'s 100% step not held, a front that counts with no roll in its view, a front that flushes only by
time so a burst passes the cap, and the edge route storing a flush with no `graftyard-agent` App JWT (caught by the
edgeboot, which builds the edge). Run from the repository root:

    python3 tests/m0carried/plant233.py          # every mutant
    python3 tests/m0carried/plant233.py 0 1      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

G = 'crates/release/src/gate.rs'
O = 'crates/release/src/ops.rs'
W = 'workers/agents-front/src/lib.rs'
E = 'workers/edge/src/release.rs'
R = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', 'lm3_']
F = ['cargo', 'test', '-q', '-p', 'graftyard-agents-front']
BOOT = ['bash', 'tests/edgeboot/run.sh']
MUTANTS = [
    ("a ControllerRoll's gate stays pending", G,
     '| Intent::McpRoll { .. } | Intent::ControllerRoll { .. }) =\n        c.intent.intent',
     '| Intent::McpRoll { .. }) =\n        c.intent.intent', R),
    ("a flush naming a version no release uploaded is stored", G,
     '            return Err(FlushRefused::UnknownVersion(v.clone()));\n', '', R),
    ("a flush over FLUSH_CALLS_MAX is stored", G,
     '    if n > FLUSH_CALLS_MAX {\n', '    if n > FLUSH_CALLS_MAX * 2 {\n', R),
    ("a flush with more 5xx than answers is stored", G,
     '.any(|a| a.fivexx > a.answers) {', '.any(|a| a.fivexx > a.answers + 1) {', R),
    ("a ControllerRoll's 100% step is not held", O,
     '(Intent::FrontRoll { .. } | Intent::McpRoll { .. } | Intent::ControllerRoll { .. }, Some(ps))',
     '(Intent::FrontRoll { .. } | Intent::McpRoll { .. }, Some(ps))', R),
    ("the front counts with no ControllerRoll in its view", W,
     '        if !gate::controller_judged(view.fold()) {\n', '        if false && !gate::controller_judged(view.fold()) {\n', F),
    ("the front flushes only by time, so a burst passes the cap", W,
     '        if now < self.flushed + FLUSH_MS && gate::calls(&self.pending) < FLUSH_CALLS_MAX {\n',
     '        if now < self.flushed + FLUSH_MS {\n', F),
    ("the edge stores a flush with no graftyard-agent App JWT", E,
     '    if let Err(r) = crate::ident::graftyard_agent(c, req).await {\n        return crate::agents::respond(c, r);\n    }\n    let key = gate::errors_key(Script::Controller);',
     '    let key = gate::errors_key(Script::Controller);', BOOT),
]

if __name__ == '__main__':
    run(MUTANTS)
