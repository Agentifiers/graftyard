#!/usr/bin/env python3
"""AM3-WP2, the controller's outbound router (`crates/git/src/outbound.rs`, `outbound`; L0-02's escape classes in
`crates/git/tests/outbound.rs`): one mutant for each rule. A Sandbox credential kept, plain HTTP allowed, an encoded
path let through, an unlisted host matched by suffix, the base's receive-pack allowed, a ref outside the task's refs
pushed, an empty command list pushed, the method override let through, status writes let through, `mcp_servers`
kept, a server tool's type taken as a client tool, any Go module served, the Sandbox's model bytes forwarded, and
the fork's token attached on the base. Run from the repository root:

    python3 tests/m0carried/plant240.py          # every mutant
    python3 tests/m0carried/plant240.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

O = 'crates/git/src/outbound.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'outbound']
MUTANTS = [
    ("the Sandbox's Authorization is forwarded", O,
     'const DROPPED: [&str; 6] = ["authorization", "cookie",', 'const DROPPED: [&str; 6] = ["x-none", "cookie",', T),
    ("plain HTTP is allowed", O, '    if !url.https {\n', '    if !url.https && false {\n', T),
    ("a percent-encoded path passes", O,
     "    if p.contains('%') || p.contains(\"//\")", "    if p.contains(\"//\")", T),
    ("a host ending in an allowed name matches it", O,
     '            host.eq_ignore_ascii_case(&url.host)\n', '            url.host.ends_with(&host.to_ascii_lowercase()[..])\n',
     T),
    ("receive-pack is allowed on the base", O, '            if !is(target) {\n', '            if !is(target) && false {\n', T),
    ("a ref outside the task's refs is pushed", O,
     '.find(|r| !refs.matches(r.as_bytes()))', '.find(|r| !r.starts_with("refs/heads/"))', T),
    ("a push with no ref is forwarded", O, '                if names.is_empty() {\n',
     '                if names.is_empty() && false {\n', T),
    ("X-HTTP-Method-Override passes", O, 'k == "x-http-method-override")', 'k == "x-none")', T),
    ("a status write passes", O,
     'let checks = |s: &str| matches!(s, "statuses" | "check-runs" | "check-suites");',
     'let checks = |s: &str| matches!(s, "check-runs" | "check-suites");', T),
    ("mcp_servers is kept", O, '    if b.contains_key("mcp_servers") {\n', '    if b.contains_key("mcp-servers") {\n', T),
    ("a tool type is matched case-insensitively", O,
     'Some(Value::String(ty)) if ty == "function" || ty == "custom" => {}',
     'Some(Value::String(ty)) if ty.eq_ignore_ascii_case("function") || ty == "custom" => {}', T),
    ("any Go module is served", O, '                if !known.iter().any(|k| **k == *module) {\n',
     '                if !known.iter().any(|k| module.starts_with(&**k)) {\n', T),
    ("the Sandbox's model bytes are forwarded", O,
     'Some(serde_json::to_vec(&body).expect("a Value"))', 'Some(req.body.to_vec())', T),
    ("the fork's token is attached on the base", O,
     '    let cred = |scheme| if is(target) { Cred::Workspace(scheme) } else { Cred::Agent(scheme) };\n',
     '    let cred = |scheme| if is(base) { Cred::Workspace(scheme) } else { Cred::Agent(scheme) };\n', T),
]

if __name__ == '__main__':
    run(MUTANTS)
