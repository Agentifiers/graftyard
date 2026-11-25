#!/usr/bin/env python3
"""OPEN-QUESTIONS F130: `graftyard oidc` (`cli/src/oidc.rs`). The configuration rule turns Browser Integrity Check off
for the issuer's path on the install's host only, keeps every other rule in the zone's entrypoint, and is written once;
the check fails on BIC's 403; a workers.dev host, whose BIC cannot be turned off, is refused by the rule and named by the
check. The mutants: the rule covers the whole host, the customer's rules are dropped, a second run writes again, the
check sends no Python agent, a workers.dev host is not refused, and a suffix match takes any host ending "workers.dev". Run from the repository root:

    python3 tests/m0carried/plant244.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

O = 'cli/src/oidc.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard', '--bin', 'graftyard', 'oidc::tests']
MUTANTS = [
    ("the rule covers the whole host", O,
     '"(http.host eq \\"{host}\\" and starts_with(http.request.uri.path, \\"{ISSUER_PATH}/\\"))"',
     '"(http.host eq \\"{host}\\")"', T),
    ("the customer's rules are dropped", O,
     'let mut out: Vec<Value> = rules.iter().map(writable).collect();',
     'let mut out: Vec<Value> = rules.iter().filter(|r| r["ref"].as_str() == Some(RULE_REF)).map(writable).collect();', T),
    ("a rule as written is written again", O,
     'Some(i) if same(&rules[i], &want) =>', 'Some(i) if false && same(&rules[i], &want) =>', T),
    ("the check sends no Python agent", O,
     'ureq::get(&url).set("user-agent", CHECK_AGENT).call()', 'ureq::get(&url).call()', T),
    ("a workers.dev host is not refused", O,
     '    if on_workers_dev(host) {\n        return Err(format!("refused for {host}: {WORKERS_DEV}"));\n    }\n', '', T),
    ("any host ending workers.dev counts", O,
     'h == "workers.dev" || h.ends_with(".workers.dev")', 'h.ends_with("workers.dev")', T),
]

if __name__ == '__main__':
    run(MUTANTS)
