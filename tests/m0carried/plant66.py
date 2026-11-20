#!/usr/bin/env python3
"""command-rows and leader-render: planted bugs in the command read (`decisions/DECISIONS.md`, "`command-rows`") and in
the leader's fill of an issue command's owner `Authz` (`leader-render`). Plants one mutant at a time, runs the tests
that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Mutant 0 puts
main's behaviour back (a task write reads the whole tiers), so the new edge test fails on main. Run from the repository
root:

    python3 tests/m0carried/plant66.py          # every mutant
    python3 tests/m0carried/plant66.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
EDGE = 'workers/edge/src/api.rs'
SITE = 'workers/site/src/family.rs'
HOSTED = 'crates/api/src/hosted.rs'
NARROW = 'crates/api/src/narrow.rs'
CALL = 'crates/git/src/issue_ops/call.rs'
API = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'task_api']
EDGE_T = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'task_and_lease', 'only_row_free',
          'the_first_send']
SITE_T = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'a_command_read']

MUTANTS = [
    ("main's rule: a task write reads the whole tiers", EDGE,
     '    if *method == Method::Get {\n        return None;\n    }\n    let session = match route {',
     '    if *method != Method::Delete {\n        return None;\n    }\n    let session = match route {', EDGE_T),
    ("the command answer streams the metadata tier", HOSTED,
     'TierRows { index, core: c, meta: meta.map(|_| Vec::new()), complete: true, picks }',
     'TierRows { index, core: c, meta: meta.map(|m| TierRows::dump(m, &graftyard_git::tiers::METADATA, '
     '&mut Budget::rows(100_000)).0), complete: true, picks }', API),
    ("every task's row, not the named one", HOSTED,
     '            if let Some(cols) = core.get("agent_task", &key) {\n'
     '                c.push(("agent_task".to_owned(), key.clone(), cols));\n            }\n',
     '            c.extend(TierRows::agent_rows(core).into_iter().filter(|(t, _, _)| t == "agent_task"));\n', API),
    ("the task's leases are not read, so lease::marks differs", HOSTED,
     '                core.prefix("lease_by_session", &key)', '                core.prefix("lease_by_session", &[])', API),
    ("the task's requester is not asked for", HOSTED,
     '                asked.extend(t.requester.map(|u| u.0));', '                let _ = t;', API),
    ("the picks name every named account, asked or not", NARROW,
     '    let known: Vec<i64> = asked.intersection(&named).map(|a| a.0.0).collect();',
     '    let known: Vec<i64> = named.iter().map(|a| a.0.0).collect();', API),
    ("the member never fills the render's Authz", HOSTED,
     '    render.dir.authz = Some(dep?.clone());\n    Some("dep")',
     '    let _ = dep?;\n    Some("dep")', API),
    ("decide never puts the dependency in its output", CALL,
     '        let authz = self.render_lacks_authz().then(|| owner_authz(s.fold(), deps)).flatten();',
     '        let authz = false.then(|| owner_authz(s.fold(), deps)).flatten();', API),
    ("decide takes another chain's Authz", CALL,
     '(ChainId::Authz(o), IdentityDep::Authz(f)) if *o == owner && f.owner == Some(owner) => Some(f),',
     '(ChainId::Authz(o), IdentityDep::Authz(f)) if *o != owner => Some(f),', API),
    ("the render's Authz wins only when the dep has none", HOSTED,
     '    if render.dir.authz.is_some() {\n        return Some("render");\n    }',
     '    if render.dir.authz.is_some() && dep.is_none() {\n        return Some("render");\n    }', API),
    ("the first send drops the whole render", EDGE,
     '        dir["authz"] = Value::Null;', '        *dir = Value::Null;', EDGE_T),
    ("an older member's answer is not asked again", HOSTED,
     '    done.pointer("/ok/out/issue").is_some() && done.pointer("/ok/out/issue/authz").is_none()',
     '    done.pointer("/ok/out/issue/authz").is_some() && done.pointer("/ok/out/issue").is_none()', EDGE_T),
    ("an issue's labels write reads command rows", EDGE,
     '(Method::Post, ["check-runs"] | ["statuses", _] | ["labels"]))',
     '(Method::Post, ["check-runs"] | ["statuses", _] | ["labels"] | ["issues", _, "labels"]))', EDGE_T),
    ("a loading follower serves a read that needs the tier", SITE,
     '        Some(r) if r.get("command").and_then(|c| c.get("loaded")) == Some(&Value::Bool(true)) => meta_loaded,\n',
     '', SITE_T),
    ("the member's answer has no marker", SITE,
     '        v["command"] = json!(true);\n        return v;', '        return v;', SITE_T),
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
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
