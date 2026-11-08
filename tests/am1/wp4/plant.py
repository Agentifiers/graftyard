#!/usr/bin/env python3
"""AM1-WP4's planted bugs: plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/am1/wp4/plant.py          # every mutant
    python3 tests/am1/wp4/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
L = 'crates/git/src/lease.rs'
F = 'crates/git/src/feed.rs'
R = 'crates/git/src/repo.rs'
G = 'crates/identity/src/glob.rs'
T = 'crates/git/src/task.rs'
C = 'crates/git/src/issue_ops/call.rs'
A = 'crates/api/src/leases.rs'
M = 'crates/raft/src/member/leader.rs'
# One cargo invocation per package list: `--lib` may appear once, and applies to every `-p` named.
GIT = ['-p', 'graftyard-git', '--test', 'lease_gates', '--test', 'agent_bytes', '--lib']
API = ['-p', 'graftyard-api', '--test', 'lease_api', '--lib']
BOTH = ['-p', 'graftyard-git', '-p', 'graftyard-api', '--test', 'lease_gates', '--test', 'agent_bytes', '--test',
        'lease_api', '--lib']

# (name, file, old, new, test args)
MUTANTS = [
    ('holds ignores expiry', L,
     '        if self.at >= l.expires || !self.can_push(l.holder.actor, l.granted) {',
     '        if !self.can_push(l.holder.actor, l.granted) {', BOTH),
    ('holds ignores the holder losing push access', L,
     '        if self.at >= l.expires || !self.can_push(l.holder.actor, l.granted) {',
     '        if self.at >= l.expires {', GIT),
    ('a final task does not release its leases in holds', L,
     '            && crate::task::effective(&t, self.owner, self.repo, self.at).is_final()',
     '            && crate::task::effective(&t, self.owner, self.repo, self.at).is_final() && false', GIT),
    ('a plan lease outlives its draft', L,
     "crate::tiers::pull_head(self.core, n).is_some_and(|p| matches!(p.state, PrState::Open { draft: true }))",
     "crate::tiers::pull_head(self.core, n).is_some_and(|_| true)", GIT),
    ('a reinstall revives an installation\'s leases (not_before ignored)', L,
     '                        && since >= state.not_before', '                        && since.0 + 1 > 0', GIT),
    ('Exclusive never reads Shared (EXCLUSIVE_MAX ignored)', L,
     'if self.mode == LeaseMode::Exclusive && at.0.saturating_sub(self.granted.0) < ms(EXCLUSIVE_MAX) {',
     'if self.mode == LeaseMode::Exclusive {', GIT),
    ('a holder conflicts with its own session', L,
     '            Some(s) => l.holder.session.as_ref() != Some(s),', '            Some(_) => true,', GIT),
    ('the Exclusive index misses leases under the glob', L,
     '                    Some(Val::Text(t)) if t.starts_with(&under) => ids.extend(id_of(k.get(1))),',
     '                    Some(Val::Text(t)) if t.starts_with(&under) => {}', GIT),
    ('Exclusive accepts a wildcard first segment', L,
     "                    first.is_empty() || first.contains('*')", "                    first.is_empty()", GIT),
    ('a task may hold two Exclusive leases', L,
     '                    return Err(LeaseErr::OnePerTask(own.lease.id));', '                    let _ = own;', GIT),
    ('an App token may acquire with no session', L,
     '            if matches!(p.actor, Actor::App { .. }) && session.is_none() {',
     '            if matches!(p.actor, Actor::App { .. }) && session.is_none() && false {', GIT),
    ('Exclusive without a live task', L,
     '                if !live {\n                    return Err(LeaseErr::NoLiveTask);',
     '                if !live && false {\n                    return Err(LeaseErr::NoLiveTask);', GIT),
    ('Shared conflicts like Exclusive (overlap check on every acquire)', L,
     '            if n.mode == LeaseMode::Exclusive {\n                let globs',
     '            if true {\n                let globs', BOTH),
    ('anyone may release (admin rule dropped)', L,
     '            if r.lease.holder.actor != actor && !ctx.admin {',
     '            if r.lease.holder.actor != actor && false {', GIT),
    ('a plan block from an account without push access makes a lease', L,
     '    if !v.can_push(actor, v.at) {\n        return out;\n    }', '', GIT),
    ('PLAN_LEASES_PER_REPO not enforced', L,
     '|| plans >= usize::from(PLAN_LEASES_PER_REPO)', '|| plans > usize::from(PLAN_LEASES_PER_REPO) * 2', GIT),
    ('a repeated key in a plan block is accepted', L,
     '        if k.is_empty() || seen.contains(&k) {', '        if k.is_empty() {', GIT),
    ('the 4 KiB plan-block cap off', L,
     '    if lines.iter().map(|l| l.len() + 1).sum::<usize>() > PLAN_BLOCK_MAX {',
     '    if lines.iter().map(|l| l.len() + 1).sum::<usize>() > PLAN_BLOCK_MAX * 4 {', GIT),
    ('a later block is read after an invalid first one', L,
     '            return Some(plan_lines(&lines[i + 1..end]));',
     '            if let Some(b) = plan_lines(&lines[i + 1..end]) {\n                return Some(Some(b));\n            }', GIT),
    ('an invalid block releases the plan lease', L,
     '        Some(None) => return out,', '        Some(None) => None,', GIT),
    ('prune never drops expired rows', L,
     ".take_while(|(k, _)| matches!(k.first(), Some(Val::Int(e)) if *e as u64 <= at.0))",
     ".take_while(|(k, _)| matches!(k.first(), Some(Val::Int(e)) if *e < 0 && *e as u64 <= at.0))", GIT),
    ('ExtendLeases moves nothing', L,
     '        .map(|r| (r.lease.id, Timestamp(r.lease.expires.0 + ms(by))))',
     '        .map(|r| (r.lease.id, r.lease.expires))', GIT),
    ('the leader does not apply extend before its commands decide', M,
     '            if let Some(x) = extend {\n                s.extend(x);\n            }', '            let _ = &extend;',
     ['-p', 'graftyard-git', '--test', 'lease_gates']),
    ('Advisory refuses too (policy ignored)', R,
     '.filter(|v| crate::tiers::agent_policy(v.core).leases == crate::agent::LeaseEnforce::AgentsExclusive);',
     '.filter(|_| true);', GIT),
    ('a lease refusal refuses the whole 32.14 push, not the ref', R,
     '                Some(AgentRule::Leased) => self.format() == REPO_FORMAT_LEASES,',
     '                Some(AgentRule::Leased) => false,', GIT),
    ('a cancel does not release its task\'s leases in the same command', T,
     '    (!ids.is_empty()).then_some(crate::repo::RepoEvent::LeaseRelease { ids, by })', '    let _ = (ids, by);\n    None',
     GIT),
    ('glob overlap ignores a trailing ** after a literal', G,
     '        (Some(&"**"), _) => overlap_segs(&a[1..], b) || (!b.is_empty() && overlap_segs(a, &b[1..])),',
     '        (Some(&"**"), _) => overlap_segs(&a[1..], b),', ['-p', 'graftyard-identity', *GIT]),
    ('a lease command on an unloaded metadata tier is refused (L0-16)', C,
     '            None if self.op.is_lease() =>', '            None if false =>', GIT),
    ('wait is not clamped', F,
     '                q.wait = Duration::from_secs(s).min(FEED_WAIT_MAX);',
     '                q.wait = Duration::from_secs(s);', BOTH),
    ('33 paths are accepted', F,
     '    if q.paths.len() > FEED_PATHS_MAX {', '    if q.paths.len() > FEED_PATHS_MAX + 1 {', BOTH),
    ('a foreign cursor is served', F,
     '        Some(c) if c.group == group && c.index >= oldest && c.index <= newest => Start::After(c.index),',
     '        Some(c) if c.index >= oldest && c.index <= newest => Start::After(c.index),', BOTH),
    ('a cursor decodes a non-canonical spelling', F,
     '        (c.encode() == s).then_some(c)', '        Some(c)', BOTH),
    ('the leader serves feed polls', F,
     '            FeedRole::Leader | FeedRole::Witness => false,', '            FeedRole::Leader => true,\n            FeedRole::Witness => false,', BOTH),
    ('followers serve while a learner answers', F,
     '            FeedRole::Follower => !learner_answers,', '            FeedRole::Follower => true,', BOTH),
    ('FEED_HELD_MAX not enforced', F,
     '        if self.held >= FEED_HELD_MAX {', '        if self.held >= FEED_HELD_MAX * 2 {', BOTH),
    ('a lagging member holds polls', F,
     '        if !f.following() {\n            return Admit::Lagging;\n        }', '', BOTH),
    ('a release reports Expired, not Released', F,
     '        let change = if released.contains(id) || task_final { LeaseChange::Released } else { LeaseChange::Expired };',
     '        let change = if task_final { LeaseChange::Released } else { LeaseChange::Expired };', GIT),
    ('the lease list ignores contents: read', A,
     '    if !contents {\n        return Reply::not_found();\n    }\n    let (mut paths', '    let (mut paths', API),
    ('the task GET lists released leases', A,
     '    Value::Array(v.session_live(s).iter().map(|row| lease_json(r, row, at)).collect())',
     '    Value::Array(graftyard_git::lease::session_rows(r.core, s).iter().map(|row| lease_json(r, row, at)).collect())',
     API),
    ('intent is raw text, not a TrustView', A,
     '        "intent": intent,', '        "intent": &*l.intent,', API),
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
        if ('error[' in out or out.lstrip().startswith('error:') or '\nerror: ' in out) and not failed:
            # A compile error, or a cargo usage error: never counted as caught.
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 else 'MISSED'
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig
