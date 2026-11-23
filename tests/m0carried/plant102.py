#!/usr/bin/env python3
"""AM2-WP3, `graft-mcp` release wiring: planted bugs in `graft-front`'s plan for a request
(`crates/release/src/view.rs`: `overrides`, `front_route`, `forward`, `mismatch`), which routes `mcp.<host>` to
`graft-mcp`, sets one `Cloudflare-Workers-Version-Overrides` header for both pointers, and checks the answer against
its own script's pin. Plants one mutant at a time, runs the tests that guard it (`crates/release/src/tests/group.rs`),
and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant102.py          # every mutant
    python3 tests/m0carried/plant102.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
V = 'crates/release/src/view.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', '--', 'graft_front', 'override']

MUTANTS = [
    # overrides
    ('a second pinned script replaces the first in the header', V,
     '    (!members.is_empty()).then(|| members.join(", "))\n',
     '    members.last().cloned()\n', T),
    ('an unpinned script gets an empty override', V,
     '    let members: Vec<String> = pins.iter().filter_map(|(s, p)| p.map(|v| version_override(s, v))).collect();\n',
     '    let members: Vec<String> = pins.iter().map(|(s, p)| p.map_or(format!("{s}=\\"\\""), |v| version_override(s, v))).collect();\n', T),
    ('no pin sends an empty header rather than none', V,
     '    (!members.is_empty()).then(|| members.join(", "))\n',
     '    Some(members.join(", "))\n', T),
    ('the members are joined without the dictionary separator', V,
     '    (!members.is_empty()).then(|| members.join(", "))\n',
     '    (!members.is_empty()).then(|| members.join(" "))\n', T),
    # front_route
    ('a host that ends in the MCP host goes to graft-mcp', V,
     '        Some(m) if !m.is_empty() && name(host) == name(m) => Script::Mcp,\n',
     '        Some(m) if !m.is_empty() && name(host).ends_with(&name(m)) => Script::Mcp,\n', T),
    ('a host that starts with the MCP host goes to graft-mcp', V,
     '        Some(m) if !m.is_empty() && name(host) == name(m) => Script::Mcp,\n',
     '        Some(m) if !m.is_empty() && name(host).starts_with(&name(m)) => Script::Mcp,\n', T),
    ('the host is compared with case', V,
     "    let name = |h: &str| h.split(':').next().unwrap_or_default().trim_end_matches('.').to_ascii_lowercase();\n",
     "    let name = |h: &str| h.split(':').next().unwrap_or_default().trim_end_matches('.').to_owned();\n", T),
    ('a port on the host misroutes it', V,
     "    let name = |h: &str| h.split(':').next().unwrap_or_default().trim_end_matches('.').to_ascii_lowercase();\n",
     "    let name = |h: &str| h.trim_end_matches('.').to_ascii_lowercase();\n", T),
    ('an empty MCP host matches an empty host', V,
     '        Some(m) if !m.is_empty() && name(host) == name(m) => Script::Mcp,\n',
     '        Some(m) if name(host) == name(m) => Script::Mcp,\n', T),
    ('every request goes to graft-mcp once it is deployed', V,
     '        Some(m) if !m.is_empty() && name(host) == name(m) => Script::Mcp,\n',
     '        Some(m) if !m.is_empty() => Script::Mcp,\n', T),
    # forward
    ("an MCP request is checked against the edge's pin", V,
     '        pin: if to == Script::Mcp { mcp_pin } else { edge },\n',
     '        pin: edge,\n', T),
    ("an API request is checked against graft-mcp's pin", V,
     '        pin: if to == Script::Mcp { mcp_pin } else { edge },\n',
     '        pin: if to == Script::Mcp { mcp_pin } else { mcp_pin.or(edge) },\n', T),
    ('an MCP request drops the edge pin from its header', V,
     '        overrides: overrides(&[(edge_script, edge), (mcp_script, mcp_pin)]),\n',
     '        overrides: if to == Script::Mcp { overrides(&[(mcp_script, mcp_pin)]) } else { overrides(&[(edge_script, edge), (mcp_script, mcp_pin)]) },\n', T),
    ('an API request drops the graft-mcp pin from its header', V,
     '        overrides: overrides(&[(edge_script, edge), (mcp_script, mcp_pin)]),\n',
     '        overrides: if to == Script::Mcp { overrides(&[(edge_script, edge), (mcp_script, mcp_pin)]) } else { overrides(&[(edge_script, edge)]) },\n', T),
    ('a front without graft-mcp still reads Pointers.mcp', V,
     '        None => ("", None),\n',
     '        None => ("graft-mcp", pin(Script::Mcp)),\n', T),
    ('the MCP pin is keyed on the edge script', V,
     '        Some((_, s)) => (s, pin(Script::Mcp)),\n',
     '        Some((_, _)) => (edge_script, pin(Script::Mcp)),\n', T),
    # mismatch
    ('a pinned answer with no version report is not an alarm', V,
     '        Some(v) => ran.and_then(|r| r.parse::<VersionId>().ok()) != Some(v),\n',
     '        Some(v) => ran.is_some_and(|r| r.parse::<VersionId>().ok() != Some(v)),\n', T),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out:
                verdict = 'DID NOT COMPILE'
            else:
                verdict = 'CAUGHT'
                caught += 1
        finally:
            open(p, 'wb').write(orig)
        print(f'{i} {name}: {verdict}', flush=True)
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
