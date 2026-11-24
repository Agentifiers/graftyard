#!/usr/bin/env python3
"""AM3-WP1, `graftyard agents init` (`cli/src/agents.rs`): `graftyard-agent` granted `contents: write`,
`graftyard-workspace` granted only `contents: read`, the `AgentApp` rows written as `Mcp` instead of `GraftyardRun`, the
agents executor's seed written to `agents.json` in place of its public key, and a rerun not refused. Run from the
repository root:

    python3 tests/m0carried/plant223.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

C = 'cli/src/agents.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard', '--bin', 'graftyard', 'agents::']
MUTANTS = [
    ("graftyard-agent writes contents", C, '            ("contents", Access::Read),\n',
     '            ("contents", Access::Write),\n', T),
    ("graftyard-workspace only reads contents", C,
     'permissions: &[("contents", Access::Write), ("metadata", Access::Read)],',
     'permissions: &[("contents", Access::Read), ("metadata", Access::Read)],', T),
    ("the AgentApp rows are Mcp", C, 'kind: AgentKind::GraftyardRun, confine: None };',
     'kind: AgentKind::Mcp, confine: None };', T),
    ("the executor seed is written to agents.json", C, 'agents_executor: graftyard_release::public_key(&seed).hex(),',
     'agents_executor: hex(&seed),', T),
    ("a rerun is not refused", C, '    if state.exists() {\n        return Err(format!(',
     '    if false {\n        return Err(format!(', T),
]

if __name__ == '__main__':
    run(MUTANTS)
