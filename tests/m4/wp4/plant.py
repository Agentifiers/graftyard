#!/usr/bin/env python3
"""M4-WP4's planted bugs: plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/m4/wp4/plant.py          # every mutant
    python3 tests/m4/wp4/plant.py 0 3      # by number

`$CARGO_TARGET_DIR` defaults to `target/plants`. Each run goes through `timeout -k 10 600`, and a run it ended counts as
caught (README, "Planted bugs").
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
CLIALL = ['-p', 'graftyard', '--bin', 'graftyard']
CF = 'cli/src/cf.rs'
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
L = 'workers/runners/launch.mjs'
BX = 'workers/runners/box.mjs'
NODE = ['node']  # `node --test workers/runners/tests/`, not cargo
TR = 'crates/actions/src/trigger.rs'
PR = ['-p', 'graftyard-actions', '--lib', '--test', 'pr_trigger', '--test', 'gates']
PU = 'crates/git/src/purge.rs'
PG = ['-p', 'graftyard-git', '--lib', 'purge::']
PGL = ['-p', 'graftyard-git', '--lib', '--test', 'lifecycle']
RAFT = ['-p', 'graftyard-raft', '--test', 'scenarios', '--test', 'leadership']
LK = ['-p', 'graftyard-lookup', '--lib']

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
    # ---- F80: the Sandbox's alarm starts the runner (launch.mjs) ----
    ('the delivery pass starts the process itself (main\'s rule, F80)', L,
     '  await sb.schedule(0, "launchRunner");\n  return true;', '  await sb.startProcess(cmd, opts);\n  return true;', NODE),
    ('a retried alarm starts without asking for the process', L,
     '  if (!(await sb.getProcess(st.opts.processId))) {', '  if (true) {', NODE),
    ('a second arm stores the start again', L, '  if (await sb.ctx.storage.get(KEY)) return false;\n', '', NODE),
    ('a start waiting on placement never gives up', L,
     '  if (Date.now() - st.armedAt > GIVE_UP_MS) {', '  if (false) {', NODE),
    ('a start that lands after a destroy is kept', L,
     '  if ((await sb.ctx.storage.get(KEY))?.state === "closed") return sb.destroy();\n', '', NODE),
    ('a destroy leaves the stored start pending', L,
     '  await sb.ctx.storage.put(KEY, { state: "closed" });', '', NODE),
    ('a failed placement is not tried again', L, '      await sb.schedule(RETRY_S, "launchRunner");\n', '', NODE),
    ('the JIT config stays stored after the launch', L,
     '  await sb.ctx.storage.put(KEY, { state: "launched" });', '  await sb.ctx.storage.put(KEY, { ...st, state: "launched" });', NODE),
    ('launchRunner lets a storage error throw (main\'s rule)', L,
     '  try {\n    await step(sb);\n  } catch (e) {', '  {\n    await step(sb);\n  } if (0) {', NODE),
    # B0-35's `pull_request` trigger (`pr-trigger`).
    ('a pull_request run is keyed by entry and file alone (main\'s rule)', S,
     'if self.is_merge_group() || self.is_pull_request() {', 'if self.is_merge_group() {', PR),
    ('a pull_request CreateRun is sent as format 0x15.1', S,
     '            ActionsOp::CreateRun { run, .. } if run.is_pull_request() => ACTIONS_FORMAT_PULL_REQUEST,\n', '', PR),
    ('a conflicting merge still starts a run', TR,
     '            MergeabilityResult::Conflict { .. } => PrTarget::Conflict,', '            MergeabilityResult::Conflict { .. } => PrTarget::Merge(ObjectId::ZERO),', PR),
    ('a merge of an older head counts as current', TR,
     '        Some(m) if m.head == v.head => match m.result {', '        Some(m) => match m.result {', PR),
    ('a pull_request run is on the head, not the merge ref', TR,
     '        head_ref: graftyard_git::tiers::merge_ref(n).display().into(),',
     '        head_ref: graftyard_git::tiers::head_ref(n).display().into(),', PR),
    ('the first scan replays every old entry', TR,
     '    if now.saturating_sub(e.at.0) > PR_REPLAY_MS {', '    if false {', PR),
    ('synchronize starts no run', TR,
     '(Some(a @ ("opened" | "synchronize" | "reopened")), Subject::Pull', '(Some(a @ ("opened" | "reopened")), Subject::Pull', PR),
    ('a merge at an older base tip is stale (the merged rule, which livelocks under B0-35)', TR,
     '        Some(m) if m.head == v.head => match m.result {',
     '        Some(m) if m.head == v.head && Some(m.base) == v.base_tip => match m.result {', PR),
    ('a closed pull request with no merge waits for one', TR, '        _ if v.open => PrTarget::Pending,',
     '        _ if true => PrTarget::Pending,', PR),
    ('F84: a shed event lets the cursor pass its entry', TR,
     '    let cursor = low_water(&carried, through);', '    let cursor = through;', PR),
    ('F84: a shed event is given up at once (main\'s 4-ask rule)', TR,
     '            (Tried::Again, Item::Pr(_)) if now.saturating_sub(c.at) > PR_REPLAY_MS => dropped.push(c),',
     '            (Tried::Again, Item::Pr(_)) if true => dropped.push(c),', PR),
    ('F84: a shed event is never given up', TR,
     '            (Tried::Again, Item::Pr(_)) if now.saturating_sub(c.at) > PR_REPLAY_MS => dropped.push(c),',
     '            (Tried::Again, Item::Pr(_)) if false => dropped.push(c),', PR),
    ('F84: the cursor commits the carried entry itself', TR,
     'unfinished.into_iter().map(|c| Index(c.index.0.saturating_sub(1)))', 'unfinished.into_iter().map(|c| Index(c.index.0))', PR),
    ('F84: a done event is carried too (the batch reruns)', TR,
     '            (Tried::Done, _) => {}', '            (Tried::Done, _) => carried.push(c),', PR),
    # ---- repo-purge: a deleted repository's purge (crates/git/src/purge.rs)
    ('repo-purge: a member that has not applied the tombstone is purged', PU,
     '                Status::Joined { purgeable: false, .. } => {\n                    return Err(Refused::NoTombstone { chain, member: member.clone() });\n                }\n',
     '', PG),
    ('repo-purge: an unanswered member counts as retired', PU,
     '                Status::Unanswered => return Err(Refused::Unanswered { chain, member: member.clone() }),\n',
     '                Status::Unanswered => {}\n', PG),
    ('repo-purge: early is not needed inside the retention', PU,
     '    if f.now < until && !f.early {', '    if f.now < until && f.early {', PG),
    ('repo-purge: an appended, uncommitted config is not a change', PU,
     '    placing || matches!(committed, Some(GroupConfig::Joint { .. })) || committed != Some(latest)',
     '    placing || matches!(committed, Some(GroupConfig::Joint { .. }))', PG),
    ('repo-purge: a placement running is not a change', PU,
     '    placing || matches!(committed, Some(GroupConfig::Joint { .. })) || committed != Some(latest)',
     '    matches!(committed, Some(GroupConfig::Joint { .. })) || committed != Some(latest)', PG),
    ('repo-purge: the plan keeps only the first source (the registry rows\' later generation is missed)', PU,
     '    for (s, n) in bootstrap.iter().chain(rows).chain(config).cloned() {',
     '    for (s, n) in bootstrap.iter().cloned() {', PG),
    ('repo-purge: a prefix without its trailing slash also takes network 655671', PU,
     '    let mut out: Vec<String> = by_net.iter().map(|p| format!("{p}/{n}/")).collect();',
     '    let mut out: Vec<String> = by_net.iter().map(|p| format!("{p}/{n}")).collect();', PG),
    ('repo-purge: a member behind the tombstone answers purgeable', PU,
     '    fold.child || crate::tiers::deleted(&*fold.core).is_some()', '    let _ = fold;\n    true',
     ['-p', 'graftyard-git', '--test', 'lifecycle']),
    # ---- purge-outside (R27): 91 and up ----
    ('purge-outside: an outside member is held to the tombstone too', PU,
     '                Status::Joined { .. } if !is_in => {', '                Status::Joined { .. } if !is_in && false => {', PGL),
    ('purge-outside: an in-config member is retired unchecked', PU,
     '            let is_in = newest.is_none_or(|(_, c)| in_config(c, member));', '            let is_in = newest.is_none();', PGL),
    ('purge-outside: no member of the newest config needs to answer', PU,
     '                if !of.iter().any(|i| *f.members[*i].2 == *m) {', '                if false && !of.iter().any(|i| *f.members[*i].2 == *m) {', PGL),
    ('purge-outside: a Degraded config\'s lost voter counts as in-config', PU,
     '        GroupConfig::Degraded { placement, lost } => (vec![&**placement], Some(*lost)),',
     '        GroupConfig::Degraded { placement, .. } => (vec![&**placement], None),', PGL),
    ('purge-outside: in-config members are retired first', PU,
     '    outside.extend(inside);\n    Ok(outside)', '    inside.extend(outside);\n    Ok(inside)', PGL),
    ('purge-outside: the member\'s own rule ignores I', PU,
     '        Some((at, c)) => at < i || !in_config(c, me),', '        Some((_, c)) => !in_config(c, me),', PGL),
    ('purge-outside: C is the oldest config, not the newest', PU,
     '            .max_by_key(|(x, _)| *x);', '            .min_by_key(|(x, _)| *x);', PGL),
    ('R28: a later placement drops the current generation (main\'s rule)', 'crates/types/src/placement.rs',
     '        Ok(Placement { generation: p.generation.max(self.generation), ..p })\n    }\n\n    /// The one-voter',
     '        Ok(p)\n    }\n\n    /// The one-voter', ['-p', 'graftyard-types', '--lib', 'placement::']),
    # ---- trigger-stream: 99 and up ----
    ('trigger-stream: the cursor settles past an unfinished event (only retries count)', TR,
     '            self.queued.iter().chain(self.running.values()).chain(self.retry.iter().map(|(_, c)| c)),\n            self.reached,',
     '            self.retry.iter().map(|(_, c)| c),\n            self.reached,', PR),
    ('trigger-stream: an item to be tried again does not hold the cursor', TR,
     '            self.queued.iter().chain(self.running.values()).chain(self.retry.iter().map(|(_, c)| c)),\n            self.reached,',
     '            self.queued.iter().chain(self.running.values()),\n            self.reached,', PR),
    ('trigger-stream: no cap on items at once', TR,
     '        while self.running.len() < self.cap {', '        while self.running.len() < usize::MAX {', PR),
    ('trigger-stream: an item tried again is dropped instead (main\'s 4-ask rule)', TR,
     '        self.retry.extend(again.into_iter().map(|c| (now + RETRY_MS, c)));', '        let _ = again;', PR),
    ('trigger-stream: the turn is first come, first served (main\'s line)', 'crates/git/src/prsync.rs',
     '                if self.cause == MergeCause::Dispatch {\n                    l.dispatch.push_back(t)\n                } else {\n                    l.other.push_back(t)\n                }',
     '                l.other.push_back(t);', ['-p', 'graftyard-git', '--test', 'agent_load']),
    ('trigger-stream: dispatch is never bounded, so pushes and GETs starve', 'crates/git/src/prsync.rs',
     '            let dispatch_next = !l.dispatch.is_empty() && (l.other.is_empty() || l.streak < DISPATCH_STREAK_MAX);',
     '            let dispatch_next = !l.dispatch.is_empty();', ['-p', 'graftyard-git', '--test', 'agent_load']),
    ('trigger-stream: a waiter dropped while handed the turn keeps it', 'crates/git/src/prsync.rs',
     '        if handed {\n            self.turn.release();\n        }', '        let _ = handed;',
     ['-p', 'graftyard-git', '--test', 'agent_load']),
    # ---- R29: an idle leader's commit round ----
    ('r29: an idle leader never tells a follower the last commit (main)', 'crates/raft/src/member/leader.rs',
     '                idle && (behind || waiting || untold)', '                idle && (behind || waiting)', RAFT),
    ('r29: the commit round goes at once, not after CALL_DEADLINE', 'crates/raft/src/member/leader.rs',
     '                    && lead.telling.get(p).is_none_or(|(_, _, at)| now >= after(*at, gap));',
     '                    && true;', RAFT),
    ('r29: an answered round never counts as told, so an idle leader beats forever', 'crates/raft/src/member/leader.rs',
     '                *told = (*told).max(commit.min(res.matched));', '                let _ = (told, commit);', RAFT),
    # ---- R23 RunnerBox: 109 and up ----
    ('r23: the start names no image, so the application\'s default runs', BX,
     '    c.start({ image, instance: st.instance,', '    c.start({ instance: st.instance,', NODE),
    ('r23: the runner runs under a shell that signals PID 1 (the Sandbox shape)', BX,
     """'exec /home/runner/run.sh --jitconfig "$JIT"'""", """'/home/runner/run.sh --jitconfig "$JIT"; kill -TERM 1'""",
     NODE),
    ('r23: a retried alarm starts without asking whether the container runs', BX,
     '  if (!c.running) {\n    if (!image)', '  if (true) {\n    if (!image)', NODE),
    ('r23: a start refused placement never gives up', BX,
     '  if (Date.now() - st.armedAt > GIVE_UP_MS) {', '  if (false) {', NODE),
    ('r23: a refused placement counts as the runner\'s exit, so it is never tried again', BX,
     '      (e) => (NO_INSTANCE.test(String(e?.message ?? e)) ? Promise.reject(e) : "exited"),',
     '      () => "exited",', NODE),
    ('r23: a start that lands after a destroy is kept', BX,
     '  if ((await o.ctx.storage.get(KEY))?.state === "closed") return close(o);\n', '', NODE),
    ('r23: destroy answers true for a runner this class never held', BX,
     '  if (!(await o.ctx.storage.get(KEY))) return false;\n', '', NODE),
    ('r23: every start is standard-3', BX, '    c.start({ image, instance: st.instance,',
     '    c.start({ image, instance: "standard-3",', NODE),
    ('r23: no inactivity timeout, so the runtime stops a running job when the object idles', BX,
     '  if (c.running) await c.setInactivityTimeout(JOB_MS);', '', NODE),
    ('r23: RunnerBox\'s application is created on the default policy', R,
     '            "name": name,\n            "scheduling_policy": "durable_object",',
     '            "name": name,\n            "scheduling_policy": "default",', CLI),
    ('r23: the version carries no runner image', R,
     '    containers.push(json!({ "name": spec.app(BOX.0), "class_name": BOX.0, "images": { "runner": image } }));',
     '    containers.push(json!({ "name": spec.app(BOX.0), "class_name": BOX.0 }));', CLI),
    ('r23: a runner_box upgrade still rolls the legacy applications at 100%', R,
     '            if spec.runner_box {\n                cf.deploy_versions', '            if false {\n                cf.deploy_versions',
     CLI),
    ('r23: init on a v1 account adds no RunnerBox migration', R,
     '        Some("v1") => meta["migrations"] = json!({ "old_tag": "v1", "new_tag": MIGRATION_TAG, "steps": [add_box] }),',
     '        Some("v1") => {}', CLI),
    ('r23: upgrade uploads a version to an account without RunnerBox', R,
     '                Some(t) if t == MIGRATION_TAG => {}', '                _ => {}', CLI),
    # ---- R23 image preparation (cli/src/cf.rs): 123 and up ----
    ('r23-prep: a version upload names an image it never prepared (main\'s rule)', CF,
     '        self.prepare_images(metadata)?;\n        let v = self.multipart("POST"', '        let v = self.multipart("POST"', CLIALL),
    ('r23-prep: a site script upload names an image it never prepared', CF,
     '        self.prepare_images(metadata)?;\n        self.multipart("PUT"', '        self.multipart("PUT"', CLIALL),
    ('r23-prep: a pending image counts as ready', CF,
     '                Some("ready") => return Ok(()),', '                Some("ready" | "pending") => return Ok(()),', CLIALL),
    ('r23-prep: an image never ready is polled forever', CF,
     '                Some("pending") if Instant::now() + self.prep_poll <= deadline =>',
     '                Some("pending") if true =>', CLIALL),
    ('r23-prep: an image refused by preparation is uploaded anyway', CF,
     '                Some("error") => {\n                    return Err(', '                Some("error") => {\n                    return Ok(()); #[allow(unreachable_code)] return Err(', CLIALL),
    ('r23-prep: the upload is made before the image is prepared', CF,
     '        self.prepare_images(metadata)?;\n        let v = self.multipart("POST", &format!("/workers/scripts/{script}/versions"), metadata, modules)?;',
     '        let v = self.multipart("POST", &format!("/workers/scripts/{script}/versions"), metadata, modules)?;\n        self.prepare_images(metadata)?;', CLIALL),
    # ---- repo-gone: a purged repository does not hold RunOwner's cursor ----
    ('a failed RunChecks always retries (main)', O,
     '        None if gone => ChecksAnswer::Gone,\n', '', G),
    ('a failed RunChecks on a repository its owner still names is gone', O,
     '        None => ChecksAnswer::Retry,\n    }', '        None => ChecksAnswer::Gone,\n    }', G),
    ('a Deleted name still counts as held', AZ,
     '            RepoEntry::Deleted { .. } => false,\n        })', '            RepoEntry::Deleted { .. } => true,\n        })',
     G + ['-p', 'graftyard-identity', '--test', 'audit']),
    ('an Alias does not count as held', AZ,
     '            RepoEntry::Live { repo: r, .. }\n            | RepoEntry::Alias { repo: r, .. }\n',
     '            RepoEntry::Alias { .. } => false,\n            RepoEntry::Live { repo: r, .. }\n',
     ['-p', 'graftyard-identity', '--test', 'audit']),
    ('Authz alone is proof a repository is gone', AZ,
     '        lookup_no_group && graftyard_types::no_row_anywhere(rows) && !self.holds(repo)', '        !self.holds(repo)', G),
    ('any lookup error reads as no group', 'crates/lookup/src/lib.rs',
     '    e.ends_with(&format!(": {NO_GROUP}"))', '    !e.is_empty()', ['-p', 'graftyard-lookup', '--lib', 'no_group']),
    # ---- repo-gone, the cross-placement transfer (the lead's review of 839fa6b0): 135 and up ----
    ('the lookup\'s no group under the shard owner\'s record is proof, rows unread (839fa6b0\'s rule)', AZ,
     '        lookup_no_group && graftyard_types::no_row_anywhere(rows) && !self.holds(repo)',
     '        lookup_no_group && !self.holds(repo)', G),
    ('a failed RunChecks is never sent again under the RepoOwner owner\'s record', O,
     '    repo_owner.filter(|o| no_group && *o != shard_owner)', '    repo_owner.filter(|_| false)', G),
    ('a registry read that failed counts as no row', 'crates/types/src/registry.rs',
     '            _ => RowRead::Failed,\n        }', '            _ => RowRead::NoRow,\n        }',
     ['-p', 'graftyard-types', '--lib', 'registry']),
    ('one site answering no row is enough', 'crates/types/src/registry.rs',
     '    !reads.is_empty() && reads.iter().all(|r| *r == RowRead::NoRow)', '    reads.iter().any(|r| *r == RowRead::NoRow)',
     ['-p', 'graftyard-types', '--lib', 'registry']),
    # ---- replay-wait (site day 55): a replayed request id's waiter, and the batched shard half: 139 and up ----
    ('wait_ticket waits on a ticket the leader already resolved (main)', 'workers/site/src/host.rs',
     '            if let Some(o) = resolved_now::<F>(core.m.outcome(ticket)) {\n                let _ = tx.send(o);\n                return rx;\n            }\n',
     '', SITE[:3] + ['a_resolved_ticket']),
    ('a waiter takes NotLeader as resolved', 'workers/site/src/host.rs',
     "    o.filter(|o| !matches!(o, Err(CommitError::NotLeader(_))) && !needs_block::<F>(o))",
     "    o.filter(|o| !needs_block::<F>(o))", SITE[:3] + ['a_resolved_ticket']),
    ('a waiter takes an id-block refusal as resolved', 'workers/site/src/host.rs',
     "    o.filter(|o| !matches!(o, Err(CommitError::NotLeader(_))) && !needs_block::<F>(o))",
     "    o.filter(|o| !matches!(o, Err(CommitError::NotLeader(_))))", SITE[:3] + ['a_resolved_ticket']),
    ('the shard half keys every command at index 0 (a retry replays the wrong rows)', O,
     '            graftyard_fold::canon::write(&mut key, &(through.0, i as u32));\n            (consumer_request',
     '            graftyard_fold::canon::write(&mut key, &(through.0, 0u32));\n            let _ = i;\n            (consumer_request', G),
    # ---- disk-first-reads (B0-35 m57): reads of named commits from the disk, 143 and up ----
    ('a read of named commits catches up first even on a disk that holds them (main)', 'crates/git/src/shard.rs',
     '    if on_disk {\n        let (t, missing) = read(false).await?;', '    if false {\n        let (t, missing) = read(false).await?;', ['-p', 'graftyard-git', '--test', 'disk_validate']),
    ('a demand catches up first even on a disk that holds base and head (main)', 'crates/git/src/prsync.rs',
     '    let a = crate::read_disk_first(on_disk, |caught| async move {', '    let a = crate::read_disk_first(on_disk && false, |caught| async move {', ['-p', 'graftyard-git', '--test', 'disk_validate']),
    ('a read that finds a commit missing on the disk answers without a catch-up', 'crates/git/src/shard.rs',
     '        if !missing {\n            return Ok(t);', '        if true || !missing {\n            return Ok(t);', ['-p', 'graftyard-git', '--test', 'disk_validate']),
    ('a 404 for a path at a commit on the disk reads as a missing commit', 'crates/git/src/shard.rs',
     '    status == 404 && (body.starts_with("no commit ") || body.starts_with("no repository "))', '    status == 404', ['-p', 'graftyard-git', '--test', 'disk_validate']),
    ('a rehydrating shard reads from its disk', 'crates/git/src/shard.rs',
     '    if rehydrating { ValidateOn::CaughtUp } else { validate_on(false, running, cursor) }', '    validate_on(false, running, cursor)', ['-p', 'graftyard-git', '--test', 'disk_validate']),
    ('a merge still missing a commit after the catch-up answers shed, not an error', 'crates/git/src/prsync.rs',
     '        Some(None) => Err(crate::StoreError("a commit of the merge is not in the git cache after catching up".into())),', '        Some(None) => Ok(None),', ['-p', 'graftyard-git', '--test', 'disk_validate']),
    ('the lookup-mark trace logs nothing for the holder\'s posts', 'crates/lookup/src/lib.rs',
     '            let posted = now();\n            let r = post(s, name.clone(), by).await;\n            let answer = classify(r.as_ref().map(|(s, v)| (*s, v)), matches!(step, Step::Bootstrap { .. }));\n            if held.is_some() {',
     '            let posted = now();\n            let r = post(s, name.clone(), by).await;\n            let answer = classify(r.as_ref().map(|(s, v)| (*s, v)), matches!(step, Step::Bootstrap { .. }));\n            if held.is_none() {', ['-p', 'graftyard-lookup', '--lib', 'a_traced_lookup']),
    # ---- catchup-local (B0-35 m59): a catch-up's reads on its own site's member, and the mark-wait log, 150 and up ----
    ('a catch-up read never takes its site member\'s answer, always the lookup (main\'s ranges read)', 'crates/lookup/src/lib.rs',
     '        Some((200, v)) if usable(&v) => Ok(v),', '        Some((200, v)) if false && usable(&v) => Ok(v),', LK),
    ('a local ranges read takes a shed or busy answer', 'crates/lookup/src/lib.rs',
     '    follower_answer_usable(Some((200, v))) && v.get("error").is_none() && v["value"].is_object()',
     '    v.get("error").is_none() && v["value"].is_object() || v.get("overloaded").is_some() || v.get("busy").is_some()', LK),
    ('a local ranges read takes an answer with no value', 'crates/lookup/src/lib.rs',
     '    follower_answer_usable(Some((200, v))) && v.get("error").is_none() && v["value"].is_object()',
     '    follower_answer_usable(Some((200, v))) && v.get("error").is_none()', LK),
    ('a local page that starts past the index asked for is taken', 'crates/lookup/src/lib.rs',
     '.and_then(|e| e["index"].as_u64()) == Some(from)', '.and_then(|e| e["index"].as_u64()) >= Some(from)', LK),
    ('a wait on another request\'s mark is not logged', 'crates/lookup/src/lib.rs',
     '                if !holders.is_empty() {', '                if false {', LK),
    ('a traced cached leader\'s miss is not logged', 'crates/lookup/src/lib.rs',
     '            } else if by == SentBy::Leader && answer != Answer::Reply {', '            } else if false {', LK),
    ('a follower answers a shard\'s ranges read NotLeader (main\'s rule)', F,
     '    if q.get("lsRefs").is_some() || *q == serde_json::json!({ "ranges": true }) {', '    if q.get("lsRefs").is_some() {',
     SITE[:3] + ['a_follower_serves_only']),
    ('a follower serves any read that names ranges', F,
     '    if q.get("lsRefs").is_some() || *q == serde_json::json!({ "ranges": true }) {',
     '    if q.get("lsRefs").is_some() || q.get("ranges").is_some() {', SITE[:3] + ['a_follower_serves_only']),
    # ---- lookup-miss-kind (B0-35 m60): a thrown fetch's kind, and an overloaded object waited for, 158 and up ----
    ('an overloaded object is a miss, as on main', 'crates/lookup/src/lib.rs',
     '        return if v["overloaded"] == true { Answer::ObjectOverloaded(why) } else { Answer::Missed(why) };',
     '        return Answer::Missed(why);', LK),
    ('an overloaded object is asked again at once', 'crates/lookup/src/lib.rs',
     '                    delay(Duration::from_millis(wait)).await;', '', LK),
    ('a thrown fetch\'s message is dropped', 'crates/lookup/src/lib.rs',
     '        let mut why = format!("thrown {:?}", v["thrown"].as_str().unwrap_or_default());',
     '        let mut why = String::from("no answer");', LK),
    ('an overloaded object is waited for past LOOKUP_FOR', 'crates/lookup/src/lib.rs',
     '                    if now() + wait >= end {', '                    if false {', LK),
    # ---- alarm-steps (B0-35 m60's 2,288 ms alarm): each slow alarm step names itself, 162 and up ----
    ('a step of exactly ALARM_STEP_LOG_MS is left out', 'workers/site/src/alarm_steps.rs',
     'filter(|s| s.1 >= ALARM_STEP_LOG_MS)', 'filter(|s| s.1 > ALARM_STEP_LOG_MS)', ['-p', 'graftyard-site', '--lib', 'alarm_steps::']),
    ('a step is timed from the alarm\'s start, not the step before', 'workers/site/src/alarm_steps.rs',
     '        self.at = now;\n', '', ['-p', 'graftyard-site', '--lib', 'alarm_steps::']),
    ('a quick alarm logs an empty line', 'workers/site/src/alarm_steps.rs',
     '        if slow.is_empty() {\n            return None;\n        }\n', '', ['-p', 'graftyard-site', '--lib', 'alarm_steps::']),
]

env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target', 'plants')),
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
        cmd = ['node', '--test', 'workers/runners/tests/*.test.mjs'] if args is NODE else \
            ['cargo', 'test', '-j', '3', *args, '--no-fail-fast']
        r = subprocess.run(['timeout', '-k', '10', '600', *cmd], cwd=ROOT, env=env, capture_output=True, text=True)
        out = r.stdout + r.stderr
        failed = sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.endswith('FAILED')} |
                        {l[2:].strip() for l in out.splitlines() if l.startswith('✖ ') and 'failing' not in l})
        if ('error[' in out or out.lstrip().startswith('error:') or '\nerror: ' in out) and not failed:
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 else 'MISSED'
        if r.returncode == 124:
            failed = ['(timed out)', *failed]
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig
