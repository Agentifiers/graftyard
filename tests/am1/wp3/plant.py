#!/usr/bin/env python3
"""AM1-WP3's planted bugs: plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/am1/wp3/plant.py          # every mutant
    python3 tests/am1/wp3/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
T = 'crates/git/src/task.rs'
R = 'crates/git/src/repo.rs'
I = 'crates/git/src/issue_ops.rs'
A = 'crates/api/src/tasks.rs'
S = 'crates/identity/src/agents.rs'
GIT = ['-p', 'graftyard-git', '--test', 'task_gates', '--test', 'agent_gates', '--test', 'agent_bytes', '--lib']
API = ['-p', 'graftyard-api', '--test', 'task_api', '--lib']
BOTH = GIT + ['-p', 'graftyard-api', '--test', 'task_api']

# (name, file, old, new, test args)
MUTANTS = [
    ('effective ignores the installation: an uninstalled App\'s tasks read live', T,
     '    } else if !installed(t, owner, repo) {', '    } else if false && !installed(t, owner, repo) {', BOTH),
    ('a reinstall revives the old tasks (not_before not checked)', T,
     '            && t.created >= state.not_before', '            && t.created.0 + 1 > 0', BOTH),
    ('TASK_IDLE never fires', T,
     '    } else if at.0.saturating_sub(t.updated.0) >= TASK_IDLE.as_millis() as u64 {',
     '    } else if at.0.saturating_sub(t.updated.0) >= u64::MAX {', BOTH),
    ('a final state does not stay final in effective', T,
     '    if t.state.is_final() {\n        t.state\n    } else if', '    if false {\n        t.state\n    } else if', BOTH),
    ('the requester cap off', T,
     '            if mine >= usize::from(TASKS_OPEN_PER_REQUESTER) {',
     '            if mine > usize::from(TASKS_OPEN_PER_REQUESTER) {', GIT),
    ('the repository cap off', T,
     '            if all >= usize::from(TASKS_OPEN_PER_REPO) {', '            if all > usize::from(TASKS_OPEN_PER_REPO) {',
     GIT),
    ('a final task leaves the open index only on cancel (caps count completed tasks)', T,
     '    if t.state.is_final() {\n        w.delete("agent_task_open", &task_key(s));',
     '    if t.state == TaskApiState::Cancelled {\n        w.delete("agent_task_open", &task_key(s));', GIT),
    ('PATCH may set cancelled or timed_out', T,
     '            if matches!(state, Some(TaskApiState::Cancelled | TaskApiState::TimedOut)) {',
     '            if matches!(state, Some(TaskApiState::Idle)) && false {', GIT),
    ('a command on a stored final task changes it', T,
     '    if t.state.is_final() {\n        return Some(Err(TaskErr::Final(t.state)));\n    }',
     '    if false {\n        return Some(Err(TaskErr::Final(t.state)));\n    }', BOTH),
    ('a derived final state is not recorded by the next command', T,
     '        return Some(Ok((smallvec::smallvec![updated(id, now, actor)], out)));',
     '        return Some(Ok((smallvec::smallvec![], out)));', BOTH),
    ('any writer may cancel any task', T,
     '                TaskActor::User(u) => t.requester == Some(u) || ctx.perm >= Some(RepoPermission::Admin),',
     '                TaskActor::User(_) => ctx.perm >= Some(RepoPermission::Write),', GIT),
    ('an App that is not an AgentApp creates external tasks', T,
     '            if crate::agent::agent_app(ctx.install, app).is_none() {',
     '            if crate::agent::agent_app(ctx.install, app).is_none() && false {', GIT),
    ('a duplicate session is created again', T,
     '            if task_core(core, &session).is_some() {', '            if task_core(core, &session).is_some() && false {',
     GIT),
    ('own_sessions takes any session named on the wire', T,
     '        let mine = Some(&id) == claimed || task_core(core, &id).is_some_and(|t| t.app == app);',
     '        let mine = true;', GIT),
    ('a claim binds another App\'s Graftyard-run task', T,
     '    let mine = t.app == app || is_workspace(install, app);', '    let mine = true;', GIT),
    ('a run task\'s refs match the bare id (run_refs is the stored ** pattern)', T,
     'SessionId::Run(_) => RefPattern::parse(&format!("{RUN_PREFIX}{}/*/**", s.text())).ok(),',
     'SessionId::Run(_) => RefPattern::parse(&format!("{RUN_PREFIX}{}/**", s.text())).ok(),', GIT),
    ('a paused task reads live for pushes and minting', T,
     '        !self.is_final() && self != TaskApiState::WaitingForUser', '        !self.is_final()', GIT),
    ('token_task lets a token cover another repository', T,
     '    if ids.iter().any(|r| *r != repo) {', '    if ids.iter().all(|r| *r != repo) {', GIT),
    ('the workspace may delete its session ref', R,
     '                if u.name == *own && u.new.is_some() && s.get(&u.name) == u.expect {',
     '                if u.name == *own && s.get(&u.name) == u.expect {', GIT),
    ('AgentMeta drops the task\'s requester (the gate no longer excludes them)', I,
     '                        Some((s, t)) if t.app == app => (Some(s.clone()), t.requester),',
     '                        Some((s, t)) if t.app == app => (Some(s.clone()), None),', GIT),
    ('AgentMeta takes a session of another App\'s task', I,
     '                        Some((s, t)) if t.app == app => (Some(s.clone()), t.requester),',
     '                        Some((s, t)) => (Some(s.clone()), t.requester),', GIT),
    ('the API renders the stored state, not the derived one', A,
     '    let state = effective(core, owner, r.at.id, at);', '    let state = core.state;', API),
    ('the prompt is shown without contents: read', A,
     '    if reader.contents {\n        v["prompt"]', '    if reader.metadata {\n        v["prompt"]', API),
    ('a session is a number in JSON (a UUID cannot cross the issue-command wire)', S,
     '        if s.is_human_readable() {\n            return s.serialize_str(&self.text());\n        }',
     '        if false {\n            return s.serialize_str(&self.text());\n        }', GIT + ['-p', 'graftyard-identity']),
]

env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '..', 'target-am1')),
           CARGO_INCREMENTAL='0')
only = sys.argv[1:] and set(int(x) for x in sys.argv[1:])
for n, (name, path, old, new, args) in enumerate(MUTANTS):
    if only and n not in only:
        continue
    f = os.path.join(ROOT, path)
    orig = open(f, 'rb').read()
    src = orig.decode()
    assert src.count(old) == 1, (name, 'anchor not unique', src.count(old))
    open(f, 'w').write(src.replace(old, new, 1))
    try:
        r = subprocess.run(['cargo', 'test', '-j', '3', *args, '--no-fail-fast'], cwd=ROOT, env=env,
                           capture_output=True, text=True)
        out = r.stdout + r.stderr
        failed = sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.endswith('FAILED')})
        if 'error[' in out and not failed:
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 else 'MISSED'
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig
