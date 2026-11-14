//! `actions:<owner>.<shard>` members: an owner's Actions shard (`design/DESIGN.md`, Shape, `ActionsEvent`;
//! `design/SHARED.md`, "Actions runs and jobs"). The fold and commands are `graftyard_actions::shard`.
//!
//! - **Stored** as one blob, as `Authz` is: a shard's rows are small and short-lived (`tests/m4/results/
//!   m4-wp3-design.md`, The Actions chain).
//! - **The lease alarm.** After the leader applies entries, and on each of its alarms, it commits `ExpireLeases` once the
//!   fold's earliest lease end or queue deadline has passed, then arms for the next one. The request id is the chain and
//!   the deadline, so an alarm run twice commits once. This is BF-4's half on the site.
//! - **Reads** (`/read` `q`): `{"job": id}` (the row), `{"run": id}` (the run and its jobs), `{"runner": id}` (the record,
//!   with its key, for the OAuth exchange), `{"queued": true}` (the broker's summary), and otherwise the counts.
//! - **The run service** (`/runsvc`, from the edge, on the leader): the edge verified the caller's token, so the body
//!   names the grant. The leader commits the runner's or job's command, and signs tokens with `RUN_SERVICE_KEY`, the
//!   Secrets Store binding only the sites hold (`decisions/DECISIONS.md`, "M4-WP3: the run service's signing key").
//!   `acquire` opens the job's `LogRelay` on this site, which `FeedStreamUrl` names; `complete` ends it.

use std::rc::Rc;

use graftyard_actions::runsvc::{self, Grant, RunnerAt, ServiceKey, Urls};
use graftyard_actions::shard::{
    ActionsActor, ActionsCmd, ActionsErr, ActionsEvent, ActionsFold, ActionsOp, ActionsOut,
};
use graftyard_actions::{JobEvent, JobId, JobState, RunId, RunnerId, RunnerSessionId, shard as act};
use graftyard_raft::{self as raft, Format, Role, codec};
use graftyard_types::{ChainId, DbId, RequestId, Timestamp, Ulid};
use serde_json::{Value, json};
use worker::SqlStorage;

use crate::family::{Family, Source, Stmt, load_blob, refusal, store_blob};
use crate::host::Host;
use crate::member::Join;

pub struct Actions;

/// How soon after applying entries the leader checks the deadlines: the earliest is at least a lease or a queue limit
/// away from any entry that set it, so a second is ample.
const CHECK_AFTER_MS: i64 = 1_000;

fn id_of(q: &Value, k: &str) -> Option<DbId> {
    q.get(k).and_then(Value::as_i64).map(DbId)
}

/// A request id the tag `0x4158` ("AX") in the top 16 bits, then 112 bits of SHA-256 of `parts`.
fn rid(parts: &str) -> RequestId {
    use sha2::Digest as _;
    let h = sha2::Sha256::new().chain_update(parts).finalize();
    let low = u128::from_be_bytes(h[..16].try_into().expect("16 bytes")) & ((1u128 << 112) - 1);
    RequestId(Ulid((0x4158u128 << 112) | low))
}

impl Family for Actions {
    type Ev = ActionsEvent;
    type Fold = ActionsFold;
    type C = ActionsCmd;

    fn reads() -> Vec<Format> {
        vec![act::ACTIONS_FORMAT]
    }
    fn fold_alarm_ms(leading: bool) -> Option<i64> {
        leading.then_some(CHECK_AFTER_MS)
    }
    async fn on_alarm(host: &Rc<Host<Self>>) {
        expire(host).await;
    }
    fn route_paths() -> &'static [&'static str] {
        &["/runsvc"]
    }
    async fn route(host: &Rc<Host<Self>>, path: &str, body: Value) -> Option<worker::Result<Value>> {
        match path {
            "/runsvc" => Some(runsvc_route(host, body).await),
            _ => None,
        }
    }
    fn new_fold(_sql: &SqlStorage, _join: &Join) -> ActionsFold {
        ActionsFold::default()
    }
    fn store_fold(fold: &ActionsFold, _: &[&raft::LogEntry<ActionsEvent>], stmts: &mut Vec<Stmt>) {
        store_blob(fold, stmts);
    }
    fn load_fold(sql: &SqlStorage, join: &Join) -> worker::Result<ActionsFold> {
        Ok(load_blob(sql)?.unwrap_or_else(|| Self::new_fold(sql, join)))
    }
    fn out_json(_fold: &ActionsFold, a: &raft::Acked<ActionsOut>) -> Value {
        serde_json::to_value(&a.out).unwrap_or(Value::Null)
    }
    fn err_json(e: &ActionsErr) -> Value {
        match e {
            ActionsErr::Forbidden => refusal(e, 403),
            ActionsErr::NotFound => refusal(e, 404),
            ActionsErr::Stale(_) | ActionsErr::Duplicate => refusal(e, 409),
            _ => refusal(e, 422),
        }
    }
    fn read_json(f: &ActionsFold, q: &Value) -> Value {
        if let Some(j) = id_of(q, "job") {
            return serde_json::to_value(f.job(JobId(j))).unwrap_or(Value::Null);
        }
        if let Some(r) = id_of(q, "run") {
            let jobs: Vec<Value> =
                f.run_jobs(RunId(r)).into_iter().map(|j| serde_json::to_value(j).unwrap_or(Value::Null)).collect();
            return json!({ "run": f.run(RunId(r)), "jobs": jobs });
        }
        if let Some(r) = id_of(q, "runner") {
            let r = RunnerId(r);
            return json!({ "runner": f.runners.get(&r), "session": f.sessions.get(&r) });
        }
        if q.get("queued").and_then(Value::as_bool) == Some(true) {
            let queued: Vec<Value> = f
                .queued()
                .into_iter()
                .map(|(job, labels, since, version)| json!({ "job": job, "labels": labels, "since": since, "version": version }))
                .collect();
            return json!({ "queued": queued, "moves": f.queue_moves });
        }
        json!({
            "runs": f.runs.len(),
            "jobs": f.jobs.len(),
            "runners": f.runners.len(),
            "sessions": f.sessions.len(),
            "moves": f.queue_moves,
            "ownerCursor": f.owner_cursor.0,
            "nextDeadline": f.next_deadline(),
        })
    }
    fn fold_bytes(f: &ActionsFold) -> Vec<u8> {
        codec::to_bytes(f)
    }
    fn dep(chain: ChainId, _bytes: &[u8]) -> Result<(), String> {
        Err(format!("an Actions shard reads no dependency ({chain})"))
    }
    fn needs_block(e: &ActionsErr) -> bool {
        matches!(e, ActionsErr::NeedIdBlock)
    }
    fn source(join: &Join) -> Source {
        match join.chain {
            ChainId::Actions(o, _) => Source::Owner(o),
            _ => Source::Given,
        }
    }
}

thread_local! {
    /// The queue-move count each shard last hinted its broker at.
    static HINTED: std::cell::RefCell<std::collections::HashMap<ChainId, u64>> = Default::default();
}

/// The leader's alarm: a hint to this site's broker when the queue moved since the last one (`crate::broker::hint`),
/// then `ExpireLeases` once the earliest deadline has passed, and an alarm at the next one.
async fn expire(h: &Rc<Host<Actions>>) {
    let now = crate::now_ms();
    let found = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref().filter(|c| c.m.role() == Role::Leader) else { return };
        let f = &c.m.group_fold().fold;
        (c.join.chain, c.join.me.site.0, f.queue_moves, f.next_deadline())
    };
    let (chain, site, moves, due) = found;
    if HINTED.with(|m| m.borrow_mut().insert(chain, moves) != Some(moves)) {
        crate::broker::hint(&h.env, site, chain).await;
    }
    let Some(Timestamp(due)) = due else { return };
    if due > now {
        h.arm_in((due - now) as i64);
        return;
    }
    let cmd = ActionsCmd { actor: ActionsActor::Operator, op: ActionsOp::ExpireLeases };
    let rid = rid(&format!("{chain}/expire/{due}"));
    let admitted = match h.input(|m, t| m.commit(t, rid, cmd)) {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => return worker::console_error!("{chain} ExpireLeases: {}", crate::host::call_error_json(&e)),
        Err(e) => return worker::console_error!("{chain} ExpireLeases: {e}"),
    };
    h.served();
    let w = h.wait_ticket(admitted.ticket, None);
    if !matches!(crate::host::within(std::time::Duration::from_secs(10), w).await, Some(Ok(Ok(_)))) {
        h.arm_in(5_000);
    }
}

// ---- the run service, on the leader

/// The run-service key: `RUN_SERVICE_KEY`, a Secrets Store secret of 64 hex digits, read once per isolate.
async fn service_key(env: &worker::Env) -> Result<ServiceKey, String> {
    thread_local! {
        static KEY: std::cell::RefCell<Option<[u8; 32]>> = const { std::cell::RefCell::new(None) };
    }
    if let Some(k) = KEY.with(|k| *k.borrow()) {
        return Ok(ServiceKey(k));
    }
    let text = env
        .secret_store("RUN_SERVICE_KEY")
        .map_err(|e| e.to_string())?
        .get()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("RUN_SERVICE_KEY is not set")?;
    let b = crate::host::unhex(text.trim()).ok_or("RUN_SERVICE_KEY is not hex")?;
    let k: [u8; 32] = b.try_into().map_err(|_| "RUN_SERVICE_KEY is not 32 bytes")?;
    KEY.with(|c| *c.borrow_mut() = Some(k));
    Ok(ServiceKey(k))
}

fn urls(env: &worker::Env) -> Urls {
    Urls::new(&format!("https://{}", crate::hookseq::api_host(env)))
}

/// One command from this leader, waiting for its outcome: `Ok(Ok(out))`, `Ok(Err(refusal))`, or `Err` for a member
/// that is not the leader or did not commit in 10 s, which the edge answers 503.
async fn commit(
    h: &Rc<Host<Actions>>,
    rid: RequestId,
    actor: ActionsActor,
    op: ActionsOp,
) -> Result<Result<ActionsOut, ActionsErr>, String> {
    let cmd = ActionsCmd { actor, op };
    let admitted = h.input(|m, t| m.commit(t, rid, cmd)).map_err(|e| e.to_string())?;
    let a = admitted.map_err(|e| crate::host::call_error_json(&e).to_string())?;
    h.served();
    let t = a.ticket;
    let end = crate::now_ms() + 10_000;
    let mut rx = h.wait_ticket(t, None);
    loop {
        let left = std::time::Duration::from_millis(end.saturating_sub(crate::now_ms()).max(1));
        match crate::host::within(left, &mut rx).await {
            Some(Ok(Ok(a))) => return Ok(Ok(a.out.clone())),
            Some(Ok(Err(raft::CommitError::Refused { err, .. }))) => return Ok(Err(err)),
            Some(Ok(Err(e))) => return Err(format!("{e:?}")),
            // A command refused for an id block was re-sent under a new ticket by the lease loop.
            Some(Err(_)) if h.current(t) != t => rx = h.wait_ticket(h.current(t), None),
            Some(Err(_)) => return Err("member reset".into()),
            None => return Err("did not commit in 10 s".into()),
        }
    }
}

/// A request id for one run-service call: the time, then 80 random bits.
fn fresh_rid() -> RequestId {
    let mut b = [0u8; 10];
    for x in &mut b {
        *x = (worker::js_sys::Math::random() * 256.0) as u8;
    }
    RequestId(Ulid::new(Timestamp(crate::now_ms()), b))
}

/// The shard and this site when this member leads, else the `notLeader` answer `graftyard_lookup` follows to the leader.
fn leader_site(h: &Rc<Host<Actions>>) -> Result<(ChainId, u8), Value> {
    let core = h.core.borrow();
    let Some(c) = core.as_ref() else { return Err(json!({ "error": { "notLeader": {} } })) };
    if c.m.role() != Role::Leader {
        return Err(json!({ "error": { "notLeader": crate::host::nl_json(&c.m.not_leader()) } }));
    }
    Ok((c.join.chain, c.join.me.site.0))
}

fn refused(status: u16, e: impl std::fmt::Debug) -> Value {
    json!({ "status": status, "body": { "source": "actions-run-service", "statusCode": status, "message": format!("{e:?}") } })
}

/// `/runsvc` `{"op", "grant"?, ...}`: one run-service call the edge authenticated. Answers `{"status", "body"}`, or
/// `notLeader` with the config, so the caller's lookup follows the leader.
async fn runsvc_route(h: &Rc<Host<Actions>>, b: Value) -> worker::Result<Value> {
    let (chain, site) = match leader_site(h) {
        Ok(x) => x,
        Err(nl) => return Ok(nl),
    };
    let ChainId::Actions(owner, shard) = chain else { return Ok(refused(400, "not an Actions shard")) };
    let grant: Option<Grant> = serde_json::from_value(b["grant"].clone()).ok();
    let key = match service_key(&h.env).await {
        Ok(k) => k,
        Err(e) => return Ok(refused(503, e)),
    };
    let urls = urls(&h.env);
    let now = crate::now_ms();
    let op = b["op"].as_str().unwrap_or("");
    let fold_job = |j: JobId| {
        let core = h.core.borrow();
        core.as_ref().and_then(|c| {
            let f = &c.m.group_fold().fold;
            let row = f.job(j)?.clone();
            let run = f.run(row.run)?.run.clone();
            Some((row, run))
        })
    };
    Ok(match (op, grant) {
        // A runner registered by an admin through the edge: `{"record"}`, the runner's public key in it. The id comes
        // from the group's block, so the JIT config is written by the edge after.
        ("register", None) => {
            let rec: act::RunnerRecord =
                serde_json::from_value(b["record"].clone()).map_err(|e| worker::Error::RustError(e.to_string()))?;
            let admin =
                serde_json::from_value(b["admin"].clone()).map_err(|e| worker::Error::RustError(e.to_string()))?;
            match commit(h, fresh_rid(), ActionsActor::Admin(admin), ActionsOp::PutRunner(rec)).await {
                Ok(Ok(ActionsOut::Runner(id))) => {
                    json!({ "status": 201, "body": { "runner": RunnerAt { owner, shard, runner: id } } })
                }
                Ok(r) => refused(422, r),
                Err(e) => refused(503, e),
            }
        }
        // The OAuth exchange: the assertion checked against the runner's record, then a runner token.
        ("token", None) => {
            let a = b["assertion"].as_str().unwrap_or("");
            let Some(at) = runsvc::assertion_runner(a).filter(|at| at.chain() == chain) else {
                return Ok(json!({ "status": 400, "body": { "error": "invalid_client" } }));
            };
            let key_der = h
                .core
                .borrow()
                .as_ref()
                .and_then(|c| c.m.group_fold().fold.runners.get(&at.runner).map(|r| r.key.clone()));
            let ok =
                key_der.is_some_and(|k| runsvc::verify_assertion(a, at, &k, &urls.token(), Timestamp(now)).is_ok());
            if !ok {
                return Ok(json!({ "status": 400, "body": { "error": "invalid_client" } }));
            }
            let exp = now / 1000 + runsvc::RUNNER_TOKEN_TTL_S;
            let t = runsvc::sign_token(&key, chain, Grant::Runner { runner: at.runner }, exp, None);
            json!({ "status": 200, "body": { "access_token": t, "token_type": "bearer", "expires_in": runsvc::RUNNER_TOKEN_TTL_S } })
        }
        ("session", Some(Grant::Runner { runner })) => {
            let session = RunnerSessionId(u128::from(now) << 16 | u128::from(runner.0.0 as u16));
            match commit(h, fresh_rid(), ActionsActor::Runner(runner), ActionsOp::OpenSession { runner, session }).await
            {
                Ok(Ok(_)) => {
                    json!({ "status": 200, "body": { "sessionId": runsvc::job_guid(5, (session.0 & 0x1f_ffff_ffff_ffff) as i64), "brokerSession": true } })
                }
                Ok(Err(e)) => {
                    json!({ "status": 400, "body": { "source": "actions-broker-listener", "errorKind": "RunnerSessionInvalid", "message": format!("{e:?}") } })
                }
                Err(e) => refused(503, e),
            }
        }
        // An ephemeral runner's last call: its session and record go.
        ("delete_session", Some(Grant::Runner { runner })) => {
            let _ = commit(h, fresh_rid(), ActionsActor::Runner(runner), ActionsOp::CloseSession { runner }).await;
            match commit(h, fresh_rid(), ActionsActor::Runner(runner), ActionsOp::DeleteRunner(runner)).await {
                Ok(_) => json!({ "status": 200, "body": null }),
                Err(e) => refused(503, e),
            }
        }
        // The broker's match: the oldest queued job this runner's labels take, committed against its version. A runner
        // holding an unfinished job gets that job's cancellation, once it was cancelled, or nothing.
        ("match", Some(Grant::Runner { runner })) => {
            let (pick, held) = {
                let core = h.core.borrow();
                let Some(c) = core.as_ref() else { return Ok(json!({ "error": { "notLeader": {} } })) };
                let f = &c.m.group_fold().fold;
                let Some(rec) = f.runners.get(&runner) else {
                    return Ok(
                        json!({ "status": 400, "body": { "source": "actions-broker-listener", "errorKind": "RunnerSessionInvalid" } }),
                    );
                };
                let held = f.jobs.iter().find(|(_, j)| j.state.runner() == Some(runner)).map(|(id, _)| *id);
                let pick = f.queued().into_iter().find(|(_, l, _, _)| l.fits(&rec.labels)).map(|(j, _, _, v)| (j, v));
                (pick, held)
            };
            if held.is_some() {
                return Ok(json!({ "status": 200, "body": null }));
            }
            let Some((job, version)) = pick else { return Ok(json!({ "status": 200, "body": null })) };
            let op = ActionsOp::Job { job, version, event: JobEvent::Matched(runner) };
            match commit(h, fresh_rid(), ActionsActor::Runner(runner), op).await {
                Ok(Ok(_)) => json!({ "status": 200, "body": runsvc::job_request(&urls, now, job), "job": job }),
                Ok(Err(_)) => json!({ "status": 200, "body": null }),
                Err(e) => refused(503, e),
            }
        }
        ("cancelled", Some(Grant::Runner { runner })) => {
            // A job this runner was matched to and that ended `Cancelled` while it ran: the broker sends it once.
            let job: Option<JobId> = serde_json::from_value(b["job"].clone()).ok();
            let c = job.and_then(fold_job).is_some_and(|(j, _)| {
                matches!(j.state, JobState::Completed { conclusion: graftyard_actions::Conclusion::Cancelled, .. })
            });
            json!({ "status": 200, "body": c.then(|| runsvc::job_cancellation(now, job.expect("job"))) , "runner": runner })
        }
        ("acquire", Some(Grant::Runner { runner })) => {
            let Some(job) = b["jobMessageId"].as_str().and_then(runsvc::job_of_guid) else {
                return Ok(refused(404, "no job"));
            };
            let Some((row, _)) = fold_job(job) else { return Ok(refused(404, "no job")) };
            let op = ActionsOp::Job { job, version: row.version, event: JobEvent::Acquired };
            match commit(h, fresh_rid(), ActionsActor::Runner(runner), op).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Ok(refused(409, e)),
                Err(e) => return Ok(refused(503, e)),
            }
            let Some((row, run)) = fold_job(job) else { return Ok(refused(404, "no job")) };
            let orch = runsvc::orch_id(run.id.0.0, job, run.attempt);
            let job_token = runsvc::sign_token(
                &key,
                chain,
                Grant::Job { job, attempt: run.attempt },
                now / 1000 + runsvc::JOB_TOKEN_TTL_S,
                Some(orch),
            );
            let steps = serde_json::from_str::<Value>(&row.spec.steps)
                .ok()
                .and_then(|v| v.as_array().map(Vec::len))
                .unwrap_or(0);
            if let Err(e) = relay(&h.env, site, job, &format!("/open/{}?steps={steps}", job.0.0), &json!({})).await {
                worker::console_error!("{chain} logrelay open {}: {e}", job.0.0);
            }
            let names = b["repository"].as_str().unwrap_or("");
            let a = runsvc::Acquired {
                repository: names,
                owner_login: names.split('/').next().unwrap_or(""),
                relay_site: site,
                github_token: b["github_token"].as_str().unwrap_or(""),
                job_token: &job_token,
                locked_until: Timestamp(now + graftyard_actions::JOB_LEASE.as_millis() as u64),
                secrets: Vec::new(),
                variables: Vec::new(),
            };
            json!({ "status": 200, "body": runsvc::acquire_message(&urls, &run, &row, &a), "repo": run.repo })
        }
        ("renew", Some(Grant::Job { job, .. })) => {
            let Some((row, _)) = fold_job(job) else { return Ok(refused(404, "no job")) };
            let op = ActionsOp::Job { job, version: row.version, event: JobEvent::Renewed };
            match commit(h, fresh_rid(), ActionsActor::Job(job), op).await {
                Ok(Ok(ActionsOut::Job(JobState::InProgress { lease_until, .. }))) => {
                    json!({ "status": 200, "body": { "lockedUntil": graftyard_wire::time::iso(lease_until) } })
                }
                Ok(r) => refused(404, r),
                Err(e) => refused(503, e),
            }
        }
        ("complete", Some(Grant::Job { job, .. })) => {
            let Some(c) = b["conclusion"].as_str().and_then(runsvc::conclusion) else {
                return Ok(refused(400, "conclusion"));
            };
            let Some((row, _)) = fold_job(job) else { return Ok(refused(404, "no job")) };
            if row.state.completed().is_none() {
                let op = ActionsOp::Job { job, version: row.version, event: JobEvent::Finished(c) };
                match commit(h, fresh_rid(), ActionsActor::Job(job), op).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => return Ok(refused(404, e)),
                    Err(e) => return Ok(refused(503, e)),
                }
            }
            let word = b["conclusion"].as_str().unwrap_or("failure");
            if let Err(e) = relay(&h.env, site, job, &format!("/end/{}", job.0.0), &json!({ "conclusion": word })).await
            {
                worker::console_error!("{chain} logrelay end {}: {e}", job.0.0);
            }
            json!({ "status": 200, "body": {} })
        }
        // `CreateRun` from the trigger (the edge's dispatch until `ActionsTrigger` runs on the repository leader).
        ("create_run", None) => {
            let run: act::WorkflowRun =
                serde_json::from_value(b["run"].clone()).map_err(|e| worker::Error::RustError(e.to_string()))?;
            let jobs: Vec<act::JobSpec> =
                serde_json::from_value(b["jobs"].clone()).map_err(|e| worker::Error::RustError(e.to_string()))?;
            let mut k = Vec::new();
            graftyard_fold::canon::write(&mut k, &run.key());
            let rid = act::consumer_request(chain, &k);
            match commit(h, rid, ActionsActor::Operator, ActionsOp::CreateRun { run, jobs }).await {
                Ok(Ok(ActionsOut::Run(r))) => json!({ "status": 201, "body": { "run": r } }),
                Ok(r) => refused(422, r),
                Err(e) => refused(503, e),
            }
        }
        _ => refused(401, "no grant for this call"),
    })
}

/// A call to the job's `LogRelay` on `site`, this site.
async fn relay(env: &worker::Env, site: u8, job: JobId, path: &str, body: &Value) -> Result<(), String> {
    let stub = env
        .durable_object(&format!("LOGRELAY_{site}"))
        .and_then(|ns| ns.get_by_name(&format!("logrelay:{}", job.0.0)))
        .map_err(|e| e.to_string())?;
    let mut init = worker::RequestInit::new();
    init.with_method(worker::Method::Post).with_body(Some(body.to_string().into()));
    let req = worker::Request::new_with_init(&format!("https://logrelay{path}"), &init).map_err(|e| e.to_string())?;
    let r = stub.fetch_with_request(req).await.map_err(|e| e.to_string())?;
    if r.status_code() == 200 { Ok(()) } else { Err(format!("{}", r.status_code())) }
}
