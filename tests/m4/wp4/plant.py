#!/usr/bin/env python3
"""M4-WP4's planted bugs: plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/m4/wp4/plant.py          # every mutant
    python3 tests/m4/wp4/plant.py 0 3      # by number

`$CARGO_TARGET_DIR` defaults to `../target-m4a`.
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
H = 'crates/actions/src/hosted.rs'
S = 'crates/actions/src/shard.rs'
C = 'workers/runners/src/control.rs'
B = 'crates/actions/src/broker.rs'
R = 'cli/src/runners.rs'
T = ['-p', 'graftyard-actions', '-p', 'graftyard-runners', '--lib', '--test', 'bf60']
CLI = ['-p', 'graftyard', 'runners::']
G = ['-p', 'graftyard-actions', '--lib', '--test', 'gates']

# (name, file, old, new, test args)
MUTANTS = [
    # ---- decide: the cap and the hosted record ----
    ('decide never checks a hosted registration', S,
     '                if r.hosted {\n                    crate::hosted::admit(s, r)?;', '                if false {\n                    crate::hosted::admit(s, r)?;', T),
    ('no cap on hosted registrations', H,
     '    if s.runners().filter(|(_, x)| x.hosted).count() >= usize::from(HOSTED_CAP) {', '    if false {', T),
    ('the cap lets one more through', H,
     '    if s.runners().filter(|(_, x)| x.hosted).count() >= usize::from(HOSTED_CAP) {',
     '    if s.runners().filter(|(_, x)| x.hosted).count() > usize::from(HOSTED_CAP) {', T),
    ('a hosted runner may be repository-scoped', H,
     '    if r.scope != RunnerScope::Owner || !r.ephemeral', '    if !r.ephemeral', T),
    ('a busy hosted runner may be deleted', H,
     "    if *actor != ActionsActor::Runner(id) && s.jobs().any(|(_, j)| j.state.runner() == Some(id)) {",
     '    if false {', T),
    # ---- Broker::demand ----
    ('idle runners are not counted against the queue', H,
     '        let n = u16::try_from(q as usize - idle.len()).unwrap_or(u16::MAX).min(room);',
     '        let n = u16::try_from(q as usize).unwrap_or(u16::MAX).min(room);', T),
    ('start ignores the cap\'s room', H, '        room -= n;', '        room -= 0;', T),
    ('start ignores runners already registered', H,
     '    let mut room = cap.saturating_sub(u16::try_from(hosted.len()).unwrap_or(u16::MAX));', '    let mut room = cap;', T),
    ('idle runners beyond the queue are never stale', H,
     '        while idle.len() > q as usize {\n            stale.push(idle.pop().expect("more idle than queued").1);\n        }\n        let n = u16::try_from(q as usize - idle.len())',
     '        let n = u16::try_from((q as usize).saturating_sub(idle.len()))', T),
    ('a runner whose job completed is not stale', H,
     '            Some(row) if !seen.held.contains(r) && now.0', '            Some(row) if now.0', T),
    ('an idle runner is never stale by age', H,
     ' && now.0 < first.0 + HOSTED_IDLE.as_millis() as u64 =>', ' =>', T),
    ('a busy runner is listed stale', H,
     '        if held.contains(r) {\n            continue;\n        }', '', T),
    # ---- BF-55's broker rule, for hosted runners ----
    ('a broker matches a hosted runner again after its job', B,
     '            Pick::Match { .. } if !complete || ephemeral && self.0.contains(&runner) => Pick::Idle,',
     '            Pick::Match { .. } if !complete => Pick::Idle,', T),
    # ---- graft-runners: one pass ----
    ('a refused delete still destroys the Sandbox', C,
     '                Ok(false) => report.kept_busy += 1,',
     '                Ok(false) => {\n                    let _ = h.destroy(*r, Instance::Standard3).await;\n                    report.kept_busy += 1\n                }', T),
    ('a stale runner\'s Sandbox is destroyed without deleting its registration', C,
     '            match h.delete(owner, *r).await {', '            match Ok::<bool, String>(true) {', T),
    ('the vCPU stop is ignored', C, '                if used + instance.vcpu() > stop {', '                if false {', T),
    ('registered runners are not counted toward the vCPU stop', C,
     '    let mut used: u32 = demands.iter().flat_map(|(_, d)| d.runners.iter()).map(|(_, l)| vcpu_of(l)).sum();',
     '    let mut used: u32 = 0;', T),
    ('every label runs on standard-3', C,
     '        if row.vcpu > 2 { Instance::Standard4 } else { Instance::Standard3 }',
     '        let _ = row;\n        Instance::Standard3', T),
    # ---- graftyard runners upgrade ----
    ('upgrade deploys a release whose signature does not verify', R,
     '        .map_err(|_| "release.json\'s signature does not verify under the release key".to_string())?;',
     '        .ok();', CLI),
    ('upgrade goes to 100% before the canary day', R, '            if age < CANARY {', '            if false {', CLI),
    ('upgrade rolls the image with the 10% canary', R,
     '            let new = cf.upload_version(&spec.script, &meta, &bundle.modules())?;',
     '            let new = cf.upload_version(&spec.script, &meta, &bundle.modules())?;\n'
     '            for a3 in cf.container_apps()? {\n'
     '                cf.roll_container_app(a3["id"].as_str().unwrap_or(""), &json!({ "image": a.image }))?;\n'
     '            }', CLI),
    # ---- the broker's stored Seen ----
    ('the broker\'s stored Seen drops first sight', H,
     '    first: BTreeMap<RunnerId, Timestamp>,', '    #[serde(skip)]\n    first: BTreeMap<RunnerId, Timestamp>,', T),
    # ---- F76: one resend rule for both scopes ----
    ('a Repo-scope poll never resends a lost match (main\'s rule)', B,
     '        taken.pick(&[(shard, &self.summary())], true, runner, labels, ephemeral, now)',
     '        match taken.pick(&[(shard, &self.summary())], true, runner, labels, ephemeral, now) {\n'
     '            Pick::Resend { .. } => Pick::Busy,\n            p => p,\n        }', G),
    ('a Repo-scope poll matches a runner that holds a job a second job', B,
     '        taken.pick(&[(shard, &self.summary())], true, runner, labels, ephemeral, now)',
     '        let mut s = self.summary();\n        s.held.clear();\n        taken.pick(&[(shard, &s)], true, runner, labels, ephemeral, now)', G),
    ('a summary forgets when a job was matched', B,
     '                    Some((*runner, *id, Some(Timestamp(lease_until.0.saturating_sub(JOB_LEASE.as_millis() as u64)))))',
     '                    Some((*runner, *id, None))', G + ['--test', 'bf55']),
]

env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '..', 'target-m4a')),
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
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 else 'MISSED'
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig
