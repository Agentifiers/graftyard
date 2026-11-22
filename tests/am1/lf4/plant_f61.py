#!/usr/bin/env python3
"""f61-reads: planted bugs in the web app's chrome reads (`crates/api/src/rest.rs`, `crates/git/src/ls_refs.rs`;
`decisions/DECISIONS.md`, "`f61-reads`"). Plants one mutant at a time, runs the test that guards it, and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/am1/lf4/plant_f61.py          # every mutant
    python3 tests/am1/lf4/plant_f61.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
REST = 'crates/api/src/rest.rs'
LS = 'crates/git/src/ls_refs.rs'
SURF = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces', 'the_web_apps_chrome_reads']
PEEL = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--lib', 'the_tags_peel']

MUTANTS = [
    ('a branch is protected by any rule, not one that matches it', REST,
     '            let protected = !r.protections(&full).is_empty();\n',
     '            let protected = !r.rules().is_empty();\n', SURF),
    ('the protected filter is ignored', REST,
     '            if want.is_some_and(|w| w != protected) {\n', '            if want.is_some_and(|_| false) {\n', SURF),
    ('a tag lists its tag object, not its commit', REST,
     '            let sha = peeled.get(&tag).copied().unwrap_or(oid);\n', '            let sha = oid;\n', SURF),
    ('tags oldest name first', REST, '    tags.reverse();\n', '', SURF),
    ('an assignee needs no write access', REST,
     '        .filter(|(_, p)| *p >= Some(graftyard_identity::RepoPermission::Write))\n',
     '        .filter(|(_, p)| p.is_some())\n', SURF),
    ('an assignee listed once per source', REST, '    who.dedup();\n', '', SURF),
    ('a team keeps the pull permission of the plain team object', REST,
     '            v["permission"] = json!(match t.permission {\n',
     '            v["permission"] = json!(match graftyard_identity::RepoPermission::Read {\n', SURF),
    ('a run whose suite has no row is not listed', REST,
     '        if !suites.iter().any(|s| s.rec.suite == t.rec.suite) {\n', '        if false {\n', SURF),
    ('the app_id filter is ignored', REST,
     '        app.is_none_or(|a| s.rec.app.0.0 == a)\n', '        app.is_none_or(|_| true)\n', SURF),
    ('suites oldest first', REST,
     '    suites.sort_by_key(|s| std::cmp::Reverse(s.rec.suite));\n', '    suites.sort_by_key(|s| s.rec.suite);\n', SURF),
    ('a suite counts every run of the commit', REST,
     '                "latest_check_runs_count": runs.iter().filter(|r| r.rec.suite == s.suite).count(),\n',
     '                "latest_check_runs_count": runs.len(),\n', SURF),
    ('reactions of an unknown issue answer 200', REST,
     '    if !found {\n        return Reply::not_found();\n    }\n    paged_values(r, &format!("/{}/reactions"',
     '    if !found && found {\n        return Reply::not_found();\n    }\n    paged_values(r, &format!("/{}/reactions"',
     SURF),
    ('the peel request asks for every ref', LS,
     '    pkt::line(&mut b, &format!("ref-prefix {prefix}"));\n', '', PEEL),
    ('a ref outside the prefix is read as a tag', LS,
     '        let Some(short) = name.strip_prefix(prefix.as_bytes()) else { continue };\n',
     '        let short = name.strip_prefix(prefix.as_bytes()).unwrap_or(name);\n', PEEL),
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=900)
            out = r.stdout + r.stderr
        finally:
            open(p, 'wb').write(orig)
        if r.returncode == 0:
            verdict = 'MISSED'
        elif 'error[E' in out or 'could not compile' in out:
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT'
            caught += 1
        print(f'{i} {verdict}: {name}', flush=True)
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
