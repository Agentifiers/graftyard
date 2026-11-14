#!/usr/bin/env python3
"""AM1-WP1's planted bugs: plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/am1/wp1/plant.py          # every mutant
    python3 tests/am1/wp1/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
L = 'crates/raft/src/member/leader.rs'
A = 'crates/raft/src/admit.rs'
F = 'crates/raft/src/fold.rs'
W = 'crates/wire/src/reply.rs'
S = 'crates/git/src/smart.rs'
P = 'crates/git/src/prsync.rs'
H = 'crates/git/src/shard.rs'
FS = 'crates/fold/src/snapshot.rs'
RAFT = ['-p', 'graftyard-raft', '--release', '--test', 'admission']
GIT = ['-p', 'graftyard-git', '--test', 'agent_load', '--test', 'prsync', '--lib']
WIRE = ['-p', 'graftyard-wire', '--lib']
FOLD = ['-p', 'graftyard-fold', '--test', 'install']
R = 'crates/git/src/repo.rs'
MARKS = ['-p', 'graftyard-git', '--test', 'agent_gates']

# (name, file, old, new, test args)
MUTANTS = [
    ('agent lane starves humans: Agent cut before Human and MergeQueue', L,
     '        for class in [AdmitClass::Human, AdmitClass::MergeQueue] {\n            while fits(ad.lanes[class.lane()].front(), &batch, bytes) {',
     '        while ad.tokens >= 1.0 && fits(ad.agent().front(), &batch, bytes) {\n            let q = ad.take(AdmitClass::Agent).expect("front");\n            (bytes, ad.tokens) = (bytes + q.bytes, ad.tokens - 1.0);\n            batch.push(q);\n        }\n        for class in [AdmitClass::Human, AdmitClass::MergeQueue] {\n            while fits(ad.lanes[class.lane()].front(), &batch, bytes) {',
     RAFT),
    ('AGENT_FLOOR not honored: no floor before Bot', L,
     '        let floor = (AGENT_FLOOR * cap * since / 1000.0).floor() as usize;',
     '        let floor = 0 * (AGENT_FLOOR * cap * since / 1000.0).floor() as usize;', RAFT),
    ('AGENT_SHARE not enforced: the bucket never empties', L,
     '            (bytes, ad.tokens) = (bytes + q.bytes, ad.tokens - 1.0);',
     '            (bytes, ad.tokens) = (bytes + q.bytes, ad.tokens);', RAFT),
    ('DEMOTE_PER_MIN in leader memory only: the fold count ignored', L,
     '        let committed = self.fold.actor_writes(actor, from, Timestamp(self.now.0 + 1));',
     '        let committed = 0 * self.fold.actor_writes(actor, from, Timestamp(self.now.0 + 1));', RAFT),
    ('DEMOTE_PER_MIN off by default: the fold never marks writes', F,
     '                if let Some(m) = F::write_mark(ev) {',
     '                if let Some(m) = F::write_mark(ev).filter(|_| false) {', RAFT),
    ('outage rule off: agent pushes over INLINE_MAX_HOT queue while placement fails', L,
     '        if agent && !a.placement_ok && a.inline > INLINE_MAX_HOT {\n            return Err(full());\n        }',
     '', RAFT),
    ('outage rule refuses Human pushes too', L,
     '        if agent && !a.placement_ok && a.inline > INLINE_MAX_HOT {',
     '        if !a.placement_ok && a.inline > INLINE_MAX_HOT {', RAFT),
    ('AGENT_QUEUE_MAX not enforced', L,
     '        if agent && (ad.agent().len() >= AGENT_QUEUE_MAX.0 as usize ||',
     '        if agent && (ad.agent().len() >= usize::MAX ||', RAFT),
    ('AGENT_WAIT never expires agents', L,
     '        let old = ad.lanes[AdmitClass::Agent.lane()].take_old(|q| q.at < cut);',
     '        let old = ad.lanes[AdmitClass::Agent.lane()].take_old(|q| q.at < cut && false);', RAFT),
    ('hot from leader memory: a pushes window that is never filled', A,
     '        if m.push {\n            self.pushes.push_back(at);\n        }', '', RAFT),
    ('AGENT_INLINE_ENTRY not applied: only pressure stages agent packs', L,
     '(self.pressure(cap) || ad.agent_inline + a.inline > AGENT_INLINE_ENTRY)', '(self.pressure(cap))', RAFT),
    ('429 without retry-after', W,
     '        .with("retry-after", retry_after_s.to_string())', '', WIRE),
    ('429 with the primary rate limit message', W,
     '"You have exceeded a secondary rate limit. Please wait a few minutes before you try again."',
     '"API rate limit exceeded."', WIRE),
    ('ng without the retry hint', S,
     'reason: format!("agent admission full, retry after {retry_after_s}s"),',
     'reason: "agent admission full".into(),', ['-p', 'graftyard-git', '--lib']),
    ('mergeable computed past the budget: pushes always admitted', P,
     '            MergeCause::HeadPush | MergeCause::Dispatch => self.ms > -est_ms,',
     '            MergeCause::HeadPush | MergeCause::Dispatch => true,', GIT),
    ('GETs ignore the budget', P,
     '                fresh && self.ms - est_ms >= GET_RESERVE * Self::rate(self.k)',
     '                fresh', GIT),
    ('list fields compute', P,
     '            MergeCause::List => false,', '            MergeCause::List => self.ms >= est_ms,', GIT),
    ('GETs not coalesced per PR', P,
     '                let fresh = self.got.get(&n).is_none_or(|t| now >= t + GET_COALESCE_MS);',
     '                let fresh = true;', GIT),
    ('fanout past FANOUT_MAX', H,
     '        want.max(k + 1).min(FANOUT_MAX)', '        want.max(k + 1)', GIT),
    ('fanout from one site, not the larger load', H,
     '    let cpu = loads[0].cpu_p99.max(loads[1].cpu_p99);', '    let cpu = loads[0].cpu_p99;', GIT),
    ('owner share not enforced', H,
     '        let more = u16::from(a.3.saturating_sub(out[i])).min(share(a.1).saturating_sub(*used)).min(left);',
     '        let more = u16::from(a.3.saturating_sub(out[i])).min(left);', GIT),
    ('another owner can reduce a held network', H,
     '    let mut out: Vec<u8> = asks.iter().map(|a| a.2.min(a.3)).collect();',
     '    let mut out: Vec<u8> = asks.iter().map(|_| 0).collect();', GIT),
    ('LOG_MAX_BYTES ignored: only the 30-day rule archives', FS,
     '    let mut over = log.iter().map(|e| e.2).sum::<u64>().saturating_sub(max_bytes);',
     '    let mut over = 0 * log.iter().map(|e| e.2).sum::<u64>().saturating_sub(max_bytes);', FOLD),
    ('RepoFold marks no writes: hot and DEMOTE_PER_MIN never fire on a repository', R,
     '        actor.map(|a| WriteMark { actor: Some(a), push: false })\n    }',
     '        let _ = actor;\n        None\n    }', MARKS),
    ('RepoFold drops the push mark: a Refs counts as a write, not a push', R,
     '                return Some(WriteMark { actor: by(actor), push });',
     '                return Some(WriteMark { actor: by(actor), push: push && false });', MARKS),
    ('RepoFold marks every write a push', R,
     '        actor.map(|a| WriteMark { actor: Some(a), push: false })',
     '        actor.map(|a| WriteMark { actor: Some(a), push: true })', MARKS),
]

env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '..', 'target-par')),
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
