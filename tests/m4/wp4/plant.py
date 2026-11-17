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
J = 'crates/actions/src/job.rs'
F = 'workers/site/src/family.rs'
A = 'crates/api/src/hosted.rs'
SITE = ['-p', 'graftyard-site', '--lib', 'fanout::']
O = 'crates/actions/src/run_owner.rs'
Q = 'crates/git/src/sequencer.rs'
K = 'crates/git/src/run_checks.rs'
W = 'crates/api/src/webhooks.rs'
HK = 'crates/git/src/hooks.rs'
R24 = G + ['-p', 'graftyard-api', '--test', 'webhooks']
AZ = 'crates/identity/src/authz.rs'
AA = 'crates/identity/src/appauth.rs'
HC = ['-p', 'graftyard-identity', '--test', 'wp4', '-p', 'graftyard-api', '--test', 'webhooks', '-p', 'graftyard-git',
      '--test', 'wire_tags']
EA = 'workers/edge/src/actions.rs'
EDGE = ['-p', 'graftyard-edge', '--lib', 'actions::']

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
     '            Some(row) if !finished(r) && now.0', '            Some(row) if now.0', T),
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
    # ---- F77: the acquire path ----
    ('a holder\'s repeated acquire is refused again (main\'s rule)', J,
     '        (S::InProgress { .. }, E::Acquired) => Some(s.clone()),\n', '', G),
    ('a repeated acquire re-arms the lease', J,
     '        (S::InProgress { .. }, E::Acquired) => Some(s.clone()),',
     '        (S::InProgress { runner, .. }, E::Acquired) => Some(S::InProgress { runner: *runner, lease_until: lease }),', G),
    ('any runner may acquire an InProgress job', S,
     '            (ActionsActor::Runner(r), JobEvent::Acquired) => j.state.runner() == Some(*r),',
     '            (ActionsActor::Runner(_), JobEvent::Acquired) => true,', G),
    ('the actions rows are a whole-tier dump', F,
     '        Some(r) if r.get("actions").is_some() => Some(graftyard_api::hosted::TierRows::actions_rows(&fold.core)),\n', '', SITE),
    ('the actions rows leave out the variables', A,
     '        for t in ["actions_secret", "actions_variable"] {', '        for t in ["actions_secret"] {', SITE),
    ('a follower sends the actions rows to the leader', F,
     ' || r.get("actions").is_some(),', ',', SITE),
    # ---- R24: `workflow_job` to an App ----
    ('a job with needs is never queued (the fold read, not the entries)', O,
     '                    (_, JobEvent::NeedsMet | JobEvent::LeaseExpired) => JobAction::Queued,',
     '                    (_, JobEvent::LeaseExpired) => JobAction::Queued,', R24),
    ('a replayed step is written again', K,
     '            if held.as_ref().is_some_and(|h| h.action >= u.action) {', '            if false {', R24),
    ('the completed step forgets its runner', K,
     '                    runner: u.runner.or(h.runner),', '                    runner: u.runner,', R24),
    ('an App without actions read gets workflow_job', Q,
     '            let read = a.permissions.0.get(need).is_some_and(|x| *x >= graftyard_identity::Access::Read);',
     '            let read = true;', R24),
    ('an installation on other repositories reaches this one', Q,
     '            graftyard_identity::RepoScope::Selected(r) => r.contains(&repo),',
     '            graftyard_identity::RepoScope::Selected(_) => true,', R24),
    ('the App\'s rows share the hook\'s GUID', Q,
     '                out.push(row(a.hook, body));', '                out.push(NewDelivery { hook: a.hook, ..row(HookId(graftyard_types::DbId(1)), body) });', R24),
    ('an App\'s request names the stored hook id', W,
     '        ("x-github-hook-id", d.hook.as_app().map_or(d.hook.0.0, |a| a.0.0).to_string()),',
     '        ("x-github-hook-id", d.hook.0.0.to_string()),', R24),
    ('an App\'s body has no installation', W,
     '        m.insert("installation".into(), json!({ "id": id, "node_id": node }));', '        let _ = (id, node);', R24),
    ('workflow_job renders as queued for every step', HK,
     'out.push(w("workflow_job", Some(u.action.name()), Subject::WorkflowJob(u.clone())))',
     'out.push(w("workflow_job", Some("queued"), Subject::WorkflowJob(u.clone())))', R24),
    # ---- `app-hook-config`: `PATCH /app/hook/config` ----
    ('SetAppHook writes the record the edge read, not the fold\'s', AZ,
     '                let record = AppRecord { webhook: webhook.clone(), webhook_secret: secret.clone(), ..have.clone() };',
     '                let mut record = have.clone();\n                record.keys.truncate(1);\n                record.webhook = webhook.clone();\n                record.webhook_secret = secret.clone();', HC),
    ('SetAppHook drops the secret', AZ,
     'webhook_secret: secret.clone(), ..have.clone() };', 'webhook_secret: None, ..have.clone() };', HC),
    ('a repeated SetAppHook writes an event', AZ,
     '                if record == *have {\n                    return Ok((SmallVec::new(), AuthzOut::Done));\n                }\n                Ok((smallvec![AuthzEvent::App { app: *app',
     '                Ok((smallvec![AuthzEvent::App { app: *app', HC),
    ('SetAppHook rides an older Authz format', AZ,
     '            AuthzOp::SetAppHook { .. } => AUTHZ_FORMAT_V13,', '            AuthzOp::SetAppHook { .. } => AUTHZ_FORMAT_V12,', HC),
    ('the hook config shows the secret', W, '        v["secret"] = json!("********");', '        v["secret"] = json!(format!("{:?}", rec.webhook_secret));', HC),
    ('a PATCH without url clears the webhook', W, '        None => cur.webhook.clone(),', '        None => None,', HC),
    ('a PATCH accepts insecure_ssl 1', W, '        Some(Value::String(s)) if s == "0" => {}', '        Some(Value::String(_)) => {}', HC),
    ('an empty secret is kept, not cleared', W,
     '        Some(Value::String(s)) if s.is_empty() => Some(None),\n        Some(Value::String(s)) => Some(Some(s.clone())),\n        Some(Value::Null) => Some(None),\n        Some(_) => return Err(bad("secret")),',
     '        Some(Value::String(s)) if s.is_empty() => None,\n        Some(Value::String(s)) => Some(Some(s.clone())),\n        Some(Value::Null) => Some(None),\n        Some(_) => return Err(bad("secret")),', HC),
    ('the client-id iss names no App', AA, "            Err(_) => i64::from_str_radix(t.strip_prefix(\"Iv1.\").filter(|h| h.len() == 16)?, 16).ok()?,",
     '            Err(_) => return None,', HC),
    # ---- F78: the stale delete sees every shard, and Seen trusts only marks older than its read ----
    ('the delete reads no shard first', H,
     '    if let Some(shard) = busy(&h.read(shards).await?) {\n        return Ok(Deleted::Busy { shard });\n    }\n',
     '', T),
    ('the delete never reads after it commits', H,
     '    Ok(busy(&h.read(shards).await?).map_or(Deleted::Gone, |shard| Deleted::Busy { shard }))',
     '    Ok(Deleted::Gone)', T),
    ('the delete reads the registration shard only', H,
     '    let mut s: BTreeSet<u8> = repos.into_iter().map(actions_shard).collect();',
     '    let mut s: BTreeSet<u8> = repos.into_iter().map(|_| OWNER_SCOPE_SHARD).collect();', T),
    ('Seen counts a mark a later read left', H,
     '    let finished = |r: &RunnerId| seen.held_at.get(r).is_some_and(|t| *t < began);',
     '    let finished = |r: &RunnerId| seen.held_at.contains_key(r);', T),
    ('Seen moves a mark to the newest read', H,
     '            seen.held_at.entry(*r).or_insert(now);', '            seen.held_at.insert(*r, now);', T),
    ('the owner\'s deleted repositories are not read', H,
     '            E::Reserved { repo, .. } | E::Live { repo, .. } | E::Alias { repo, .. } | E::Deleted { repo, .. } => *repo,',
     '            E::Reserved { repo, .. } | E::Live { repo, .. } | E::Alias { repo, .. } => *repo,\n            E::Deleted { .. } => RepoId(graftyard_types::DbId(0)),', T),
    # ---- F79: acquirejob never sends a job without its repository's name or secrets ----
    ('a failed repository read acquires with an empty name (main\'s rule, F79)', EA,
     '    let (name, repo, authz) = r.named(repo).await.map_err(|e| failed(format!("repository {}: {e}", run.repo.0.0)))?;',
     '    let Ok((name, repo, authz)) = r.named(repo).await else { return Ok((String::new(), Vec::new(), Vec::new())) };',
     EDGE),
    ('a failed owner read acquires with an empty name (main\'s rule)', EA,
     '    let repo = r.repo(&run).await.map_err(|e| failed(format!("repository {}: owner: {e}", run.repo.0.0)))?;',
     '    let Ok(repo) = r.repo(&run).await else { return Ok((String::new(), Vec::new(), Vec::new())) };', EDGE),
    ('a failed run read acquires with an empty name (main\'s rule)', EA,
     '    let run = r.run(row.run).await.map_err(|e| failed(format!("run {}: {e}", row.run.0.0)))?;',
     '    let Ok(run) = r.run(row.run).await else { return Ok((String::new(), Vec::new(), Vec::new())) };', EDGE),
    ('a failed secrets read acquires without the secrets', EA,
     '        r.config(&run, &row, &repo, &authz).await.map_err(|e| failed(format!("secrets: {e}")))?;',
     '        r.config(&run, &row, &repo, &authz).await.unwrap_or_default();', EDGE),
    ('a job the shard does not hold is retried as a failed read', EA,
     '    let row = row.ok_or_else(|| gone(format!("job {}: no such job", job.0.0)))?;',
     '    let row = row.ok_or_else(|| failed(format!("job {}: no such job", job.0.0)))?;', EDGE),
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
