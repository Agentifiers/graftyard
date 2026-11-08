#!/usr/bin/env python3
"""AM1's hosted half: planted bugs in the code this round added or changed (the edge's agent push path, REST ref
writes, the git cache detail's Rust side, the serving member's ring, `place_learners`' feed half, and the GraphQL body
edit). Plants one mutant at a time, runs the tests that gate it, and restores the file byte for byte. Each line prints
CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/am1/hosted/plant.py          # every mutant
    python3 tests/am1/hosted/plant.py 0 3      # by number

The Go side of `/validate`'s detail is planted by `go test` mutants at the end (GO).
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
E = 'workers/edge/src/agents.rs'
A = 'crates/git/src/agent.rs'
F = 'crates/git/src/feed.rs'
G = 'crates/api/src/graphql.rs'
EDGE = ['-p', 'graftyard-edge', '--lib', 'agents']
TASK = ['-p', 'graftyard-git', '--test', 'task_gates']
LEASE = ['-p', 'graftyard-git', '--test', 'lease_gates']
API = ['-p', 'graftyard-api', '--test', 'surfaces']

# (name, file, old, new, test args)
MUTANTS = [
    ('every push asks for detail (agentish always true)', E,
     '    if !agent_format {\n        return false;\n    }\n    if matches!(who.actor, graftyard_identity::Actor::App { .. }) {\n        return true;\n    }',
     '    if !agent_format {\n        return false;\n    }\n    if true {\n        return true;\n    }', EDGE),
    ('a user push under a confine prefix asks no detail', E,
     '        u.name.starts_with(graftyard_git::task::RUN_PREFIX) || prefixes.iter().any(|p| p.matches(u.name.as_bytes()))\n    })\n}\n\n/// The `validate`',
     '        u.name.starts_with(graftyard_git::task::RUN_PREFIX)\n    })\n}\n\n/// The `validate`', EDGE),
    ('detail asked before 32.8 is enabled', E,
     '    if !agent_format {\n        return false;\n    }', '', EDGE),
    ('the token claim is not among the sessions linked', E,
     '    own.extend(graftyard_git::task::claim(who).cloned());', '', EDGE),
    ('changed kept for a user push outside every agent prefix', E,
     '    push.changed = v.changed.into_iter().filter(|x| under(&x.refname)).collect();',
     '    push.changed = v.changed;', EDGE),
    ('protected dropped for an agent update', E,
     '    push.protected = v.protected.into_iter().filter(|x| under(&x.refname)).collect();',
     '    push.protected = Vec::new();', EDGE),
    ('leases set before 32.14', E, '    push.leases = leases;', '    push.leases = true;', EDGE),
    ('a non-fast-forward PATCH accepted', E,
     '    u.expect.is_none() || force || detail.first().is_some_and(|d| d.ff)',
     '    u.expect.is_none() || force || !detail.is_empty()', EDGE),
    ('force ignored on a REST ref write', E,
     '    u.expect.is_none() || force || detail.first().is_some_and(|d| d.ff)',
     '    u.expect.is_none() || detail.first().is_some_and(|d| d.ff)', EDGE),
    ('detail lines sent for a default push', A,
     '    if !agentish {\n        return String::new();\n    }', '', TASK),
    ('detail lines name the old tip as the new', A,
     '            Some(format!("{} {old} {}\\n", new.hex(), u.name.display()))',
     '            Some(format!("{old} {} {}\\n", new.hex(), u.name.display()))', TASK),
    ('validated_of drops protected paths', A,
     '        v.protected.extend(protected_of(&r, &paths, extra));', '', TASK),
    ('validated_of ignores the commits (no provenance)', A,
     '        v.provenance.extend(provenance_of(&r, &commits, &notes));', '', TASK),
    ('the ring serves events at or before the cursor', F,
     "        self.events.iter().filter(|(i, _, e)| *i > after && e.matches(filter)).cloned().collect()",
     "        self.events.iter().filter(|(i, _, e)| *i >= after && e.matches(filter)).cloned().collect()", LEASE),
    ('the ring ignores the paths filter', F,
     "        self.events.iter().filter(|(i, _, e)| *i > after && e.matches(filter)).cloned().collect()",
     "        self.events.iter().filter(|(i, _, _)| *i > after).cloned().collect()", LEASE),
    ('learners_for_feed ignores refusals', F,
     '    let total: u64 = reports.iter().map(|(peak, refused)| u64::from(*peak) + u64::from(*refused)).sum();',
     '    let total: u64 = reports.iter().map(|(peak, _)| u64::from(*peak)).sum();', LEASE),
    ('updatePullRequest takes both body and title', G,
     '                _ => return Err(unprocessable(field, "exactly one of body and title is served per request")),',
     '                (Some(b), Some(_)) => graftyard_git::issues::IssueChange::Body(b.into()),\n                _ => return Err(unprocessable(field, "exactly one of body and title is served per request")),',
     API),
    ('updatePullRequest ignores an unserved field (state)', G,
     '                [\"assigneeIds\", \"baseRefName\", \"labelIds\", \"maintainerCanModify\", \"milestoneId\", \"projectIds\", \"state\"]',
     '                [\"assigneeIds\", \"baseRefName\", \"labelIds\", \"maintainerCanModify\", \"milestoneId\", \"projectIds\"]',
     API),
    ('a core-only read refused while the metadata tier loads (L0-16)', 'workers/edge/src/api.rs',
     '        None => core_only.then_some(empty),', '        None => None,', EDGE),
    ('every read served without the metadata tier', 'workers/edge/src/api.rs',
     '        None => core_only.then_some(empty),', '        None => Some(empty),', EDGE),
]

# The git cache shard's detail (Go): (name, old, new), each run with `go test ./...` in containers/git/server.
GOFILE = 'containers/git/server/main.go'
GO = [
    ('commits reachable from existing refs reported as new',
     '"rev-list", fmt.Sprintf("--max-count=%d", commitsMax+1), u.New, "--not", "--all")',
     '"rev-list", fmt.Sprintf("--max-count=%d", commitsMax+1), u.New)'),
    ('ff always true',
     '\t\t\td.FF = err == nil', '\t\t\td.FF = true || err == nil'),
    ('only the old tip diffed (merge-base paths lost)',
     '\t\t\t\tbases = append(bases, d.MergeBase)', '\t\t\t\t_ = bases'),
    ('a delete gets detail', '\t\tif zero(u.New) {\n\t\t\tcontinue\n\t\t}\n\t\td := refDetail', '\t\td := refDetail'),
    ('an update line without refs/ accepted',
     ' || !strings.HasPrefix(f[2], "refs/") {', ' {'),
]


def cargo(args):
    env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
               CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '..', 'target-am1')),
               CARGO_INCREMENTAL='0')
    return subprocess.run(['cargo', 'test', '-j', '3', *args, '--no-fail-fast'] if args[-1] != 'agents' else
                          ['cargo', 'test', '-j', '3', *args[:-1], '--no-fail-fast', args[-1]],
                          cwd=ROOT, env=env, capture_output=True, text=True)


def plant(n, name, path, old, new, run):
    f = os.path.join(ROOT, path)
    orig = open(f, 'rb').read()
    src = orig.decode()
    assert src.count(old) == 1, (name, 'anchor not unique', src.count(old))
    open(f, 'w').write(src.replace(old, new, 1))
    try:
        r = run()
        out = r.stdout + r.stderr
        failed = sorted({l.split()[1] for l in out.splitlines()
                         if (l.startswith('test ') and l.endswith('FAILED')) or l.startswith('--- FAIL')})
        if ('error[' in out or out.lstrip().startswith('error:') or '\nerror: ' in out
                or 'build failed' in out) and not failed:
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 else 'MISSED'
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig


only = sys.argv[1:] and set(int(x) for x in sys.argv[1:])
for n, (name, path, old, new, args) in enumerate(MUTANTS):
    if not only or n in only:
        plant(n, name, path, old, new, lambda: cargo(args))
for k, (name, old, new) in enumerate(GO):
    n = len(MUTANTS) + k
    if not only or n in only:
        plant(n, f'go: {name}', GOFILE, old, new,
              lambda: subprocess.run(['go', 'test', './...'], cwd=os.path.join(ROOT, 'containers/git/server'),
                                     capture_output=True, text=True))
