#!/usr/bin/env python3
"""fanout: planted bugs in this branch's new code (concurrent validate and /load in the git shard container, shared
catch-ups, the Fanout event and its fold row, the leader's report fold, the validator choice, the site's ranges read,
and the A1 driver's push-result parser). Plants one mutant at a time, runs the tests that gate it, and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/fanout/plant.py          # every mutant
    python3 tests/fanout/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GO = 'containers/git/server/main.go'
MC = 'crates/git/src/merge_cache.rs'
SH = 'crates/git/src/shard.rs'
REPO = 'crates/git/src/repo.rs'
TIERS = 'crates/git/src/tiers.rs'
FAM = 'workers/site/src/family.rs'

GOT = ['go', 'TestConcurrent|TestLoad|TestFinishValidate']
CATCH = ['cargo', '-p', 'graftyard-git', '--lib', 'merge_cache::tests::fresh']
LOAD = ['cargo', '-p', 'graftyard-git', '--test', 'agent_load', '']
PUSH = ['cargo', '-p', 'graftyard-git', '--test', 'push', 'fanout']
WIRE = ['cargo', '-p', 'graftyard-git', '--test', 'wire_tags', '']
SITE = ['cargo', '-p', 'graftyard-site', '--lib', 'fanout::tests']
EDGE = ['cargo', '-p', 'graftyard-edge', '--lib', 'api::tests']
SURF = ['cargo', '-p', 'graftyard-api', '--test', 'surfaces', 'a_hosted_mutation']
SURF2 = ['cargo', '-p', 'graftyard-api', '--test', 'surfaces', '']
A1 = 'tests/am1/rest/a1.mjs'
WIRE = 'tests/am1/rest/wire.mjs'
SELF = ['node', A1, '--selftest']

MUTANTS = [
    ('go: two validates of the same pack rename onto one directory unserialized', GO,
     '\tmu.Lock()\n\tdefer mu.Unlock()\n\tdst := filepath.Join(rp, "graft", "validated-"+sum)',
     '\t_ = mu\n\tdst := filepath.Join(rp, "graft", "validated-"+sum)', GOT),
    ('go: the sweep deletes a running validate\'s young quarantine', GO,
     'time.Since(st.ModTime()) > time.Hour', 'time.Since(st.ModTime()) >= 0', GOT),
    ('go: validate holds the shared lock but ingest takes it shared too', GO,
     '\tl := lock(net)\n\tl.Lock()\n\tif name, ok, err := adopt(rp, want); ok {',
     '\tl := lock(net)\n\tl.RLock()\n\tif name, ok, err := adopt(rp, want); ok {', GOT),
    ('go: a validate waiting for the lock is not counted in queue_depth', GO,
     '\tvalidating.Add(1) // waiting or running: the queue\n\tdefer validating.Add(-1)\n\tl := lock(net)\n\tl.RLock()\n\tdefer l.RUnlock()\n',
     '\tl := lock(net)\n\tl.RLock()\n\tdefer l.RUnlock()\n\tvalidating.Add(1)\n\tdefer validating.Add(-1)\n', GOT),
    ('go: a finished validate stays in queue_depth', GO,
     '\tvalidating.Add(1) // waiting or running: the queue\n\tdefer validating.Add(-1)\n',
     '\tvalidating.Add(1) // waiting or running: the queue\n', GOT),
    ('catch-up: a request is served by a catch-up that started before it arrived', MC,
     '        if self.1.ok.get() > arrived {', '        if self.1.ok.get() >= arrived {', CATCH),
    ('catch-up: a success is not recorded, so every request runs its own', MC,
     '        self.1.ok.set(n);\n        Ok(())', '        let _ = n;\n        Ok(())', CATCH),
    ('catch-up: shared catch-ups no longer take turns', MC,
     '        let _running = self.turn().await;\n        if self.1.ok.get() > arrived {',
     '        let _running = self.start();\n        if self.1.ok.get() > arrived {', CATCH),
    ('reports: a stale report keeps counting', SH,
     '.find(|(x, at, _)| *x == s && now.0 < at.0 + LOAD_STALE.as_millis() as u64)',
     '.find(|(x, _, _)| *x == s)', LOAD),
    ('reports: a site that is not warm is counted', SH,
     '        if !warm.contains(&site) {\n            return None;\n        }\n', '', LOAD),
    ('reports: an unchanged k is answered as a step', SH,
     '        (next != k.clamp(1, FANOUT_MAX)).then_some(next)', '        Some(next)', LOAD),
    ('combine: the CPUs are summed', SH,
     'cpu_p99: loads.iter().map(|l| l.cpu_p99).fold(0.0, f32::max),', 'cpu_p99: loads.iter().map(|l| l.cpu_p99).sum(),', LOAD),
    ('validator: every push to the first shard', SH,
     '    validator(push, &ks).unwrap_or(ks[0])', '    let _ = push;\n    ks[0]', LOAD),
    ('object name: the first shard takes a pool name', SH,
     '    if s.shard < count {\n        format!("git#{}#net:{}"', '    if s.shard <= count {\n        format!("git#{}#net:{}"', LOAD),
    ('fold: k before any Fanout is 0', REPO,
     'get_rec::<u8>(&*self.core, "fanout", &[]).unwrap_or(1)', 'get_rec::<u8>(&*self.core, "fanout", &[]).unwrap_or(0)', PUSH),
    ('fold: Fanout writes no row', TIERS,
     '            RepoEvent::Fanout { k } => put_rec(w, "fanout", Vec::new(), k),', '            RepoEvent::Fanout { .. } => {}', PUSH),
    ('wire: Fanout takes tag 40', REPO,
     'RepoEvent::Fanout { k } => return tagged(out, 39, k),', 'RepoEvent::Fanout { k } => return tagged(out, 40, k),', PUSH),
    ('wire: Fanout inserted before PathSet', REPO,
     '    /// Metadata, in the push\'s entry: one row per ref, deleted when the ref\'s pull request closes.\n    PathSet(crate::agent::PathSet),\n    // Format 32.15 (`REPO_FORMAT_FANOUT`): tag 39.\n    /// Core. The k both warm sites hold for the network (`crate::shard::fanout`), committed by the network group\'s\n    /// leader in an entry of its own with no request, only when k changes. Its core `fanout` row is what a shard\'s\n    /// `ranges` read answers.\n    Fanout {\n        k: u8,\n    },\n',
     '    // Format 32.15 (`REPO_FORMAT_FANOUT`): tag 39.\n    Fanout {\n        k: u8,\n    },\n    /// Metadata, in the push\'s entry: one row per ref, deleted when the ref\'s pull request closes.\n    PathSet(crate::agent::PathSet),\n', WIRE),
    ('site: the ranges read leaves out k', FAM,
     '                "fanout": fold.fanout_k(),\n', '', SITE),
    ('site: this release does not read 32.15', FAM,
     '            git::repo::REPO_FORMAT_FANOUT,\n        ]', '        ]', SITE),
    ('site: a follower answers a report without the notLeader shape', 'workers/site/src/fanout.rs',
     'json!({ "error": crate::host::call_error_json(&graftyard_raft::CallError::NotLeader(nl)) })',
     'json!({ "error": "not leader", "term": nl.term.0 })', SITE),
    # lm1-rerun2: whole-tier reads packed into one JSON string.
    ('site: a packed rows read still answers the Value tree', FAM,
     '            if q.get("packed").and_then(Value::as_bool) == Some(true) {\n                return rows.packed();\n            }\n',
     '', SITE),
    ('edge: the tier read stops asking for packed rows', 'workers/edge/src/api.rs',
     'json!({ "q": { "rows": rows, "ranges": true, "packed": true } })',
     'json!({ "q": { "rows": rows, "ranges": true } })', EDGE),
    ('api: a packed answer is read as the Value tree it is not', 'crates/api/src/hosted.rs',
     '            Value::String(s) => serde_json::from_str(s),\n', '', SURF),
    ('api: packing drops the metadata tier', 'crates/api/src/hosted.rs',
     '        Value::String(serde_json::to_string(self).unwrap_or_default())',
     '        Value::String(serde_json::to_string(&TierRows { meta: None, ..self.clone() }).unwrap_or_default())', SURF),
    ('api: the tier read leaves out a table a view reads (comment)', 'crates/api/src/hosted.rs',
     'pub const UNSERVED: &[&str] = &["path_set", "provenance"];',
     'pub const UNSERVED: &[&str] = &["path_set", "provenance", "comment"];', SURF2),
    ('api: the tier read serves path_set again', 'crates/api/src/hosted.rs',
     'pub const UNSERVED: &[&str] = &["path_set", "provenance"];', 'pub const UNSERVED: &[&str] = &["provenance"];',
     SURF2),
    ('driver: a push result is matched by its text, so `002cok refs/...` reads as an error', WIRE,
     '  return lines.some((l) => l.startsWith("ok refs/")) ? "ok" : "error";',
     '  return /(^|[^a-z])ok refs\\//.test(text) ? "ok" : "error";', SELF),
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
