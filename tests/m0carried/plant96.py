#!/usr/bin/env python3
"""AM2-WP2b, the coordination reads: planted bugs in `GET /overlaps` and `GET /provenance`
(`crates/api/src/overlaps.rs`), their narrow member read (`crates/api/src/narrow.rs`), and `graft-mcp`'s
`coordination.overlaps` and `provenance.why` (`workers/mcp/src/tools.rs`). Plants one mutant at a time, runs the tests
that guard it (`crates/api/tests/coordination.rs`, `workers/mcp/tests/wp2a.rs`), and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant96.py          # every mutant
    python3 tests/m0carried/plant96.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
O = 'crates/api/src/overlaps.rs'
NARROW = 'crates/api/src/narrow.rs'
TOOLS = 'workers/mcp/src/tools.rs'
A = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'coordination']
M = ['cargo', 'test', '-q', '-p', 'graftyard-mcp', '--test', 'wp2a']

MUTANTS = [
    # Overlaps.
    ("the subject's own ref is listed as an overlap", O,
     '            if !refs.contains(&p.refname) && !overlapping(&globs, globs_of(&p)).is_empty() {\n',
     '            if !overlapping(&globs, globs_of(&p)).is_empty() {\n', A),
    ('planned paths are ignored', O,
     '    p.paths.iter().chain(&p.planned).map(|s| &**s)\n',
     '    p.paths.iter().chain(p.planned.iter().take(0)).map(|s| &**s)\n', A),
    ('a truncated directory does not cover the paths under it', O,
     '    p.paths.iter().chain(&p.planned).map(|s| &**s)\n',
     '    p.paths.iter().chain(&p.planned).map(|s| s.trim_end_matches(\'/\'))\n', A),
    ('overlaps are not capped at 100', O,
     '        items.truncate(OVERLAPS_MAX);\n', '', A),
    ("a lease is listed whether or not its paths overlap", O,
     '.is_some_and(|l| l.lease.overlaps(&g, &rf)))', '.is_some())', A),
    ('an expired lease is listed', O,
     '            Some(o) => LeaseView { core: r.core, owner: o, repo: r.at.id, at }.holds(row),\n',
     '            Some(_) => true,\n', A),
    ("a ref's open pull request is found only from the PathSet row", O,
     'r.core.prefix("pull_by_head", &[Val::Int(r.at.id.0.0), ref_key(&p.refname)])',
     'r.core.prefix("pull_by_head", &[Val::Int(-1)])', A),
    ('the verdict reads as clean before AM4-WP2', O,
     '        "verdict": Value::Null,\n', '        "verdict": "clean",\n', A),
    ('the member records nothing for an overlaps read', NARROW,
     '            crate::overlaps::overlaps_json(&r, None, graftyard_types::Timestamp(0), q);\n',
     '            let _ = q;\n', A),
    # Provenance.
    ('a squash commit does not map through Merged', O,
     'if merge_commit == *commit)', 'if merge_commit == *commit && false)', A),
    ("a commit whose branch's pull request closed is not found through its ref", O,
     '(p.head.repo == r.at.id && refs.contains(&p.head.branch)).then_some(n)',
     '(p.head.repo == r.at.id && refs.contains(&p.head.branch) && false).then_some(n)', A),
    ('a forged trailer reads as linked', O,
     '"value": view, "linked": false,', '"value": view, "linked": true,', A),
    ("a trailer's value is not capped", O,
     '            let shown: String = value.chars().take(CLAIMED_MAX).collect();\n',
     '            let shown: String = value.to_string();\n', A),
    ("a trailer's value is not trust-viewed", O,
     '"value": view, "linked": false,', '"value": shown, "linked": false,', A),
    ("blame's range end is exclusive", O,
     '            if !(s..=e).contains(&line) {\n', '            if !(s..e).contains(&line) {\n', A),
    ('the member records nothing for a provenance read', NARROW,
     '            crate::overlaps::provenance_json(&r, c, None);\n', '            let _ = c;\n', A),
    # graft-mcp.
    ("coordination.overlaps drops its query", TOOLS,
     '            c.agents("GET", &format!("{}/overlaps", c.agents_repo(args)?), &q, None, None).await\n',
     '            c.agents("GET", &format!("{}/overlaps", c.agents_repo(args)?), &[], None, None).await\n', M),
    ("provenance.why puts the ref in the path", TOOLS,
     '            c.agents("GET", &format!("{}/provenance", c.agents_repo(args)?), &q, None, None).await\n',
     '            c.agents("GET", &format!("{}/provenance/{}", c.agents_repo(args)?, q[0].1), &q[1..], None, None).await\n',
     M),
    ('provenance.why skips the grant check', TOOLS,
     '            c.agents("GET", &format!("{}/provenance", c.agents_repo(args)?), &q, None, None).await\n',
     '            c.agents("GET", &format!("/api/graftyard/v1/repos/{}/{}/provenance", s("owner").unwrap_or_default(), s("repo").unwrap_or_default()), &q, None, None).await\n',
     M),
    ('provenance.why is in a default toolset', TOOLS,
     '        name: "provenance.why",\n        set: Toolset::Provenance,\n',
     '        name: "provenance.why",\n        set: Toolset::Repos,\n', M),
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
