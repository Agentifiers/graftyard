#!/usr/bin/env python3
"""OPEN-QUESTIONS A57 (LM-3 r2): the controller gate's reads (`crates/release/src/gate.rs`, `read_sites`, `sum_sites`,
`controller_gate`, `GATE_READ_MS`). The mutants: the sites asked in turn, so a read takes one call per site; a site
that did not answer counted as one that did; a missed read judged as zero counts instead of `Pending`; a missed read's
line that does not name it; and the cron's read held to a request's `VIEW_CALL_MS`. Run from the repository root:

    python3 tests/m0carried/plant239.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

G = 'crates/release/src/gate.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', 'a57_']
MUTANTS = [
    ("the sites are asked in turn", G,
     '    sum_sites(futures_util::future::join_all(sites.iter().map(|s| read(*s))).await)\n',
     '    let mut out = Vec::new();\n    for s in sites {\n        out.push(read(*s).await);\n    }\n    sum_sites(out)\n', T),
    ("a site that did not answer is counted", G,
     '    for e in answers.into_iter().flatten() {\n        sum.add(&e);\n        sites += 1;\n    }\n',
     '    for e in answers {\n        sum.add(&e.unwrap_or_default());\n        sites += 1;\n    }\n', T),
    ("a missed read is judged as no counts", G,
     '        (Err(e), _) | (_, Err(e)) => {\n            return (Gate::Pending, format!("graft-agents-ops gate controller: pending, release/errors {e}"));\n        }\n',
     '        (w, a) => (w.unwrap_or_default(), a.unwrap_or_default()),\n', T),
    ("a missed read's line does not name it", G,
     'format!("graft-agents-ops gate controller: pending, release/errors {e}")',
     '{ let _ = e; "graft-agents-ops gate controller: pending".to_string() }', T),
    ("the cron's read is held to VIEW_CALL_MS", G,
     'pub const GATE_READ_MS: u64 = 5_000;', 'pub const GATE_READ_MS: u64 = 1_000;', T),
]

if __name__ == '__main__':
    run(MUTANTS)
