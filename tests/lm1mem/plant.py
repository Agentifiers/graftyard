#!/usr/bin/env python3
"""lm1-memory: planted bugs in this branch's new code (the lookup's overload answer and `suspect` damping, the member's
heavy-read cap with streamed answers, and the packed-dump cache). Plants one mutant at a time, runs the
tests that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run
from the worktree root:

    python3 tests/lm1mem/plant.py          # every mutant
    python3 tests/lm1mem/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LK = 'crates/lookup/src/lib.rs'
MEM = 'workers/site/src/member.rs'
FAM = 'workers/site/src/family.rs'
HOST = 'workers/site/src/host.rs'
API = 'workers/edge/src/api.rs'
EDGE_LIB = 'workers/edge/src/lib.rs'
RAFT_INSTALL = 'crates/raft/src/member/install.rs'
LOOKUP = ['cargo', '-p', 'graftyard-lookup', '--lib', 'tests']
SITE = ['cargo', '-p', 'graftyard-site', '--lib', '']
EDGE = ['cargo', '-p', 'graftyard-edge', '--lib', 'a_failed_read']
EDGE_GATE = ['cargo', '-p', 'graftyard-edge', '--lib', 'a_shed_authz']
FOLD = ['cargo', '-p', 'graftyard-fold', '--test', 'install', 'a_learner_behind']

MUTANTS = [
    ('lookup: an overloaded answer is taken as a missed member (suspect feeds the storm)', LK,
     '        return Answer::Overloaded(o.get("retryAfterMs").and_then(Value::as_u64).unwrap_or(250));',
     '        let _ = o;\n        return Answer::Missed("overloaded".into());', LOOKUP),
    ('lookup: an overloaded answer is returned to the caller as the read', LK,
     '    if let Some(o) = v.get("overloaded") {', '    if let Some(o) = v.get("overloaded").filter(|_| false) {', LOOKUP),
    ('lookup: a 5xx is no longer reported', LK,
     'if status >= 500 { Answer::Missed(status.to_string()) }', 'if status >= 600 { Answer::Missed(status.to_string()) }',
     LOOKUP),
    ('lookup: the overload reply drops `busy`, so an older lookup returns it as the read', LK,
     'json!({ "busy": true, "overloaded": { "retryAfterMs": retry_after_ms } })',
     'json!({ "overloaded": { "retryAfterMs": retry_after_ms } })', LOOKUP),
    ('damping: keyed without the term, so a new term\'s report is dropped', LK,
     '        match r.get(&(chain, about, term, all)) {', '        match r.get(&(chain, about, 0, all)) {', LOOKUP),
    ('damping: the gap never ends', LK, '            Some(at) if now < *at + gap => false,',
     '            Some(_) => false,', LOOKUP),
    ('cap: a plain read on a follower is shed instead of answering NotLeader', MEM,
     '(heavy && (leading || index) && heavies >= HEAVY_READS)', '(heavy && heavies >= HEAVY_READS)', SITE),
    ('cap: narrow reads are never capped (heavy only, as before)', MEM, '    if reads >= READS_MAX || (heavy',
     '    if (reads >= READS_MAX && heavy) || (heavy', SITE),
    ('cap: READS_MAX is off by one', MEM, '    if reads >= READS_MAX ||', '    if reads > READS_MAX ||', SITE),
    ('follower: HEAVY_READS does not hold on a follower', MEM, '(heavy && (leading || index) && heavies',
     '(heavy && leading && heavies', SITE),
    ('follower: a follower serves a read it may not (no NotLeader gate)', MEM,
     '    if index && !leading && !servable {', '    if index && !leading && !servable && false {', SITE),
    ('follower: the NotLeader gate runs after the caps, so a capped follower is retried, not left', MEM,
     '    if index && !leading && !servable {\n        return Gate::NotLeader;\n    }\n    if reads >= READS_MAX',
     '    if reads >= READS_MAX', SITE),
    ('follower: a late read-index read answers busy like the leader (lookup skips it, fine) - wait too long', MEM,
     '        (FOLLOWER_WAIT, graftyard_lookup::overloaded_reply(OVERLOAD_RETRY_MS))',
     '        (raft::CALL_DEADLINE * 3, graftyard_lookup::overloaded_reply(OVERLOAD_RETRY_MS))', SITE),
    ('follower: a late read-index read answers a stale value', MEM,
     '        (FOLLOWER_WAIT, graftyard_lookup::overloaded_reply(OVERLOAD_RETRY_MS))',
     '        (FOLLOWER_WAIT, json!({ "value": null, "index": 0 }))', SITE),
    ('follower: refs pages go to followers', FAM, '        None => false,\n    }\n}',
     '        None => q.get("refs").is_some(),\n    }\n}', SITE),
    ('follower: whole-tier rows are served while the metadata tier loads', FAM,
     '        Some(Value::Bool(true)) => meta_loaded,', '        Some(Value::Bool(true)) => true,', SITE),
    ('lookup: the cached leader is asked as a follower first', LK, ".filter(|(s, _)| Some(*s) != leader).collect()",
     ".filter(|(s, _)| Some(*s) != leader || true).collect()", LOOKUP),
    ('lookup: a follower that names the leader is taken as the answer', LK,
     '&& r.is_some_and(|(_, v)| v.pointer("/error/notLeader").is_none()',
     '&& r.is_some_and(|(_, v)| v.pointer("/error/notLeaderX").is_none()', LOOKUP),
    ('lookup: no rotation, every caller hits the same follower', LK, '            out.rotate_left(n);', '            let _ = n;', LOOKUP),
    ('round 4: a read the members shed falls back to the leader', LK,
     'if answers.iter().all(declined) { Spread::Leader } else { Spread::Shed }',
     'if answers.iter().all(declined) { Spread::Leader } else { Spread::Leader }', LOOKUP),
    ('round 4: a member that did not answer counts as declining, so the read goes to the leader', LK,
     '        None => false,\n    };\n    if answers', '        None => true,\n    };\n    if answers', LOOKUP),
    ('round 4: a 5xx counts as declining', LK, '        Some((s, _)) => (400..500).contains(s),',
     '        Some((s, _)) => (400..600).contains(s),', LOOKUP),
    ('round 4: learners are not asked', LK, '    let mut out = rotated(learners);',
     '    let mut out: Vec<Target> = Vec::new();\n    let _ = rotated(learners);', LOOKUP),
    ('round 4: followers before learners', LK,
     '    let mut out = rotated(learners);\n    out.extend(rotated(voters.into_iter().filter(|(s, _)| Some(*s) != leader).collect()));',
     '    let mut out = rotated(voters.into_iter().filter(|(s, _)| Some(*s) != leader).collect());\n    out.extend(rotated(learners));',
     LOOKUP),
    ('round 4: every learner is asked, however many', LK, '    out.truncate(SPREAD_TRIES);\n', '', LOOKUP),
    ('round 4: a config\'s learners are not learned', LK, '            e.learners = learners;',
     '            e.learners = Vec::new();\n            let _: &Vec<(SiteId, String)> = &learners;', LOOKUP),
    ('round 4: a shed read is 503 without Retry-After', API,
     '    Reply::error(503, &format!("{why} ({e})"), "rest").with("retry-after", retry)',
     '    let _ = retry;\n    Reply::error(503, &format!("{why} ({e})"), "rest")', EDGE),
    ('round 4: a shed read waits the 5 s of a failure, not 1 s', API,
     'if e.contains(graftyard_lookup::OVERLOADED) { "1" } else { "5" }', '{ "5" }', EDGE),
    ('round 4: a learner behind grants a read from its older fold', RAFT_INSTALL,
     '        for id in failed {\n            self.out.resolutions.push(Resolution::Read { id, result: Err(nl.clone()) });',
     '        for id in failed {\n            let g = grant(self);\n            self.out.resolutions.push(Resolution::Read { id, result: Ok(g) });', FOLD),
    ('round 5: the read gate drops a shed Authz read\'s reason (Retry-After 5, not 1)', EDGE_LIB,
     '        Err(e) => Err(format!("the owner\'s Authz chain did not answer: {e}")),',
     '        Err(_) => Err("the owner\'s Authz chain did not answer".into()),', EDGE_GATE),
    ('round 5: a shed Authz read is taken as a missing group', EDGE_LIB,
     '        Err(e) => Err(format!("the owner\'s Authz chain did not answer: {e}")),',
     '        Err(_) => Ok(graftyard_identity::AuthzFold::default()),', EDGE_GATE),
    ('shared: every read builds its own answer', HOST, '    if let Some(b) = built.get(&key) {',
     '    if let Some(b) = built.get(&key).filter(|_| false) {', SITE),
    ('shared: answers keyed by nothing, so another query gets the first answer', HOST,
     '    let key = q.to_string();', '    let key = String::new();\n    let _ = q;', SITE),
    ('cap: narrow row reads count as heavy', MEM, '    q.get("rows").is_some_and(Value::is_boolean)',
     '    q.get("rows").is_some()', SITE),
    ('stream: the shared value is skipped', MEM, '                1 => &shared,', '                1 => &[],', SITE),
    ('stream: the read is released at the first piece', MEM, '        if sent >= total {', '        if sent > 0 {', SITE),
    ('cap: a dropped read stays counted', MEM, '        self.0.set(self.0.get().saturating_sub(1));',
     '        let _ = self.0;', SITE),
    ('rebuilds: a valid cached dump still waits for the next slot', FAM,
     '    if DUMPS.with(|d| d.borrow().get(&repo).is_some_and(|(k, _)| *k == key)) {\n        return None;\n    }',
     '    let _ = key;', SITE),
    ('rebuilds: a let-through rebuild is not recorded, so every stale read rebuilds', FAM,
     '                r.insert(repo, now);\n                None', '                None', SITE),
    ('cache: a hit is served without comparing its key, so an applied entry is not in the next read', FAM,
     '.filter(|(at, _)| at == k)', '.filter(|(at, _)| at == k || true)', SITE),
    ('cache: the key is a constant', FAM, '    (c.applied.0, c.sum, fold.meta.loaded(), m.applied.0, m.sum)',
     '    let _ = (c, m);\n    (0, [0; 32], false, 0, [0; 32])', SITE),
]

ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0',
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', '/home/user/workspace/repo/.worktrees/target-rows'))


def run(t):
    if t[0] == 'node':
        r = subprocess.run(t, cwd=ROOT, capture_output=True, text=True, timeout=120)
        out = r.stdout + r.stderr
        return r.returncode, out, 'SyntaxError' in out
    if t[0] == 'go':
        r = subprocess.run(['go', 'test', '-count=1', '-run', t[1], '.'], cwd=os.path.join(ROOT, 'containers/git/server'),
                           capture_output=True, text=True, timeout=600)
        out = r.stdout + r.stderr
        return r.returncode, out, 'build failed' in out or 'syntax error' in out or 'undefined:' in out
    _, _, pkg, kind, name, *rest = t
    if kind == '--lib':
        args = ['cargo', 'test', '-q', '-p', pkg, '--lib', name]
    else:
        args = ['cargo', 'test', '-q', '-p', pkg, '--test', name] + [f for f in rest if f]
    r = subprocess.run(args, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1200)
    out = r.stdout + r.stderr
    return r.returncode, out, 'error[E' in out or 'could not compile' in out


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, t = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            code, out, broke = run(t)
        finally:
            open(p, 'wb').write(orig)
        if broke:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif code != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ') or l.startswith('--- FAIL')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
