#!/usr/bin/env python3
"""BF-76's r1 fixes (`workers/edge/src/actions/pinmap.rs`, `policy_from` and `annotations_from`;
`crates/identity/src/authz.rs`, `pin_map_change`; `decisions/DECISIONS.md`, "`pin-map`"). The first mutant is main's
early return: with tags off and no approval the current map is not read, so an unapproved pin gets no Graftyard
annotation (check run 1972519). Then: Graftyard's annotation after the runner's, a tag counted as a pin, and the
withdraw written as JSON `null` (main's shape), read back as keep, or the binary forms changed. Run from the repository
root:

    python3 tests/m0carried/plant274.py          # every mutant
    python3 tests/m0carried/plant274.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

PM = 'workers/edge/src/actions/pinmap.rs'
AZ = 'crates/identity/src/authz.rs'
AN = 'crates/actions/src/annotations.rs'
TE = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'pinmap']
TI = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--test', 'agents', '--', 'actions_substitution']
MUTANTS = [
    ("the annotation path skips the current map with tags off and nothing approved, as on main", PM,
     '        let p = policy_from(r, owner, true).await;\n', '        let p = policy_from(r, owner, false).await;\n', TE),
    ("the policy read never reads the current map for an org with no policy", PM,
     '    if !p.tags && p.pin_map.is_none() && !known {\n', '    if !p.tags && p.pin_map.is_none() {\n', TE),
    ("Graftyard's annotation goes after the runner's", PM,
     '    an::job_annotations(first, runner)\n}\n\n// ---- routes', '    an::job_annotations(runner, first)\n}\n\n// ---- routes', TE),
    ("a known pin's annotation names the upstream SHA where the fork's belongs", PM,
     '                first.push(an::pin_map_annotation(&name, &version, &fork, &fork_sha, web, &org));\n',
     '                first.push(an::pin_map_annotation(&name, &version, &fork, &version, web, &org));\n', TE),
    ("the withdraw is written as JSON null, as on main", AZ,
     '            Some(None) if s.is_human_readable() => s.serialize_str(WITHDRAW),\n', '', TI),
    ("\"withdraw\" reads back as keep", AZ,
     '            Some(serde_json::Value::String(w)) if w == WITHDRAW => Ok(Some(None)),\n',
     '            Some(serde_json::Value::String(w)) if w == WITHDRAW => Ok(None),\n', TI),
    ("postcard writes the withdraw as a string too, changing stored entries' bytes", AZ,
     '            Some(None) if s.is_human_readable() => s.serialize_str(WITHDRAW),\n',
     '            Some(None) => s.serialize_str(WITHDRAW),\n', TI),
    ("a field left out of the JSON is refused instead of kept", AZ,
     '        #[serde(default, with = "pin_map_change")]\n', '        #[serde(with = "pin_map_change")]\n', TI),
]

if __name__ == '__main__':
    run(MUTANTS)
