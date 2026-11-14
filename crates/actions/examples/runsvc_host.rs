//! The run service in memory, for an unmodified `actions/runner` (`tests/m4/wp3/run-host.sh`). One owner's Actions
//! shard is three real `graftyard_raft::Member<ActionsCmd>`s (`tests/common`), ticked every 500 ms, with the
//! leader's lease alarm; every route the runner calls commits what the hosted routes will commit, through
//! `graftyard_actions::runsvc`. Plain HTTP on 127.0.0.1, one thread per request, so a long poll holds only its own.
//!
//! `runsvc_host <port> <workflow.yml> <out dir>`: registers one runner, writes `<out>/jitconfig.txt`, creates a run
//! from the workflow's jobs on `push` to `refs/heads/main`, and serves until every job completed. `<out>/calls.jsonl`
//! logs each call, `<out>/fold.json` the final jobs, `<out>/live.jsonl` the `FeedStreamUrl` frames, and `<out>/blobs/`
//! the step logs.

#[path = "../tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common::Group;
use graftyard_actions::runsvc::{self, Grant, JobMessage, ServiceKey, Urls};
use graftyard_actions::shard::{
    ACTIONS_FORMAT, ActionsActor, ActionsCmd, ActionsErr, ActionsFold, ActionsOp, ActionsOut, JobSpec, RunnerRecord,
    RunnerScope, WorkflowRun,
};
use graftyard_actions::workflow::{self, matrix};
use graftyard_actions::{JobEvent, JobId, JobState, LabelSet, RunnerId, RunnerSessionId};
use graftyard_raft::CommitError;
use graftyard_types::{AccountId, ChainId, DbId, IdBlock, Index, RepoId, Timestamp};
use rsa::pkcs8::EncodePublicKey;
use serde_json::{Value, json};
use smallvec::SmallVec;

const OWNER: AccountId = AccountId(DbId(100));
const REPO: RepoId = RepoId(DbId(7));
/// A poll is held this long [inference: under the runner's 100 s HTTP timeout, measured].
const POLL_HOLD: Duration = Duration::from_secs(50);

fn now() -> Timestamp {
    Timestamp(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64)
}

struct World {
    shard: Group<ActionsCmd>,
    next: u128,
    /// The job each runner was handed, for its cancellation message.
    handed: BTreeMap<RunnerId, JobId>,
    cancelled_sent: BTreeMap<JobId, bool>,
    messages: u64,
    registered: u32,
    /// Set by an ephemeral runner's `DELETE session`: register the next one once its record is gone.
    replace: bool,
}

impl World {
    fn cmd(&mut self, actor: ActionsActor, op: ActionsOp) -> Option<Result<ActionsOut, ActionsErr>> {
        self.next += 1;
        match self.shard.commit(now(), self.next << 32, ActionsCmd { actor, op })? {
            Ok(a) => Some(Ok(a.out)),
            Err(CommitError::Refused { err, .. }) => Some(Err(err)),
            Err(_) => None,
        }
    }
    fn fold(&self) -> &ActionsFold {
        self.shard.fold()
    }
}

type Work = Box<dyn FnOnce(&mut World) + Send>;

/// The members are not `Send`: they live on one thread, which runs each request's closure and the 500 ms tick.
struct Members(Mutex<mpsc::Sender<Work>>);

impl Members {
    fn with<R: Send + 'static>(&self, f: impl FnOnce(&mut World) -> R + Send + 'static) -> R {
        let (tx, rx) = mpsc::channel();
        self.0.lock().unwrap().send(Box::new(move |w| _ = tx.send(f(w)))).unwrap();
        rx.recv().unwrap()
    }
}

struct Host {
    w: Members,
    urls: Urls,
    key: ServiceKey,
    out: String,
    log: Arc<Mutex<std::fs::File>>,
    t0: Instant,
}

impl Host {
    fn log(&self, rec: Value) {
        let mut rec = rec;
        rec["dt"] = json!((self.t0.elapsed().as_millis() as f64) / 1000.0);
        let mut f = self.log.lock().unwrap();
        writeln!(f, "{rec}").unwrap();
    }
}

fn respond(req: tiny_http::Request, status: u16, body: Value) {
    let data = if body.is_null() { Vec::new() } else { body.to_string().into_bytes() };
    let mut r = tiny_http::Response::from_data(data).with_status_code(status);
    if !body.is_null() {
        r.add_header(tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap());
    }
    let _ = req.respond(r);
}

fn bearer(req: &tiny_http::Request) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(String::from))
}

fn form(body: &str) -> BTreeMap<String, String> {
    body.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.replace("%3A", ":").replace("%3a", ":").replace('+', " ")))
        .collect()
}

fn handle(h: &Host, mut req: tiny_http::Request) {
    let method = req.method().to_string();
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let grant = bearer(&req).and_then(|t| runsvc::verify_token(&t, &h.key.public(), now()));
    // The live-log socket first: an upgrade request has no body, and reading one would wait for the client to hang up.
    if method == "GET" && path.starts_with("/_services/logrelay/") {
        h.log(json!({"method": method, "path": path, "note": "live log socket", "grant": grant}));
        return live(h, req, grant);
    }
    let mut body = String::new();
    let _ = req.as_reader().read_to_string(&mut body);
    let jbody: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let rel = path.strip_prefix("/_services/").unwrap_or("");
    let mut rec = json!({"method": method, "path": path, "grant": grant});
    if !jbody.is_null() && !rel.starts_with("blob/") {
        rec["body"] = jbody.clone();
    }
    let (status, reply) = match (method.as_str(), rel) {
        ("POST", "oauth/token") => token(h, &form(&body)),
        ("POST", "broker/session") => match grant {
            Some(Grant::Runner { runner }) => {
                let session = RunnerSessionId(now().0 as u128);
                let r =
                    h.w.with(move |w| w.cmd(ActionsActor::Runner(runner), ActionsOp::OpenSession { runner, session }));
                match r {
                    Some(Ok(_)) => {
                        (200, json!({"sessionId": runsvc::job_guid(5, session.0 as i64), "brokerSession": true}))
                    }
                    other => (403, json!({"source": "actions-broker-listener", "errorMessage": format!("{other:?}")})),
                }
            }
            _ => (401, Value::Null),
        },
        ("GET", "broker/message") => match grant {
            Some(Grant::Runner { runner }) => poll(h, runner),
            _ => (401, Value::Null),
        },
        ("POST", "broker/acknowledge") => (200, json!({})),
        ("DELETE", "broker/session") => match grant {
            Some(Grant::Runner { runner }) => {
                // An ephemeral runner deletes its config after its job: its record goes, and the next is registered.
                h.w.with(move |w| {
                    w.cmd(ActionsActor::Runner(runner), ActionsOp::CloseSession { runner });
                    w.cmd(ActionsActor::Runner(runner), ActionsOp::DeleteRunner(runner));
                    w.replace = true;
                });
                (200, Value::Null)
            }
            _ => (401, Value::Null),
        },
        ("POST", "run/acquirejob") => match grant {
            Some(Grant::Runner { runner }) => acquire(h, runner, &jbody),
            _ => (401, Value::Null),
        },
        ("POST", "run/renewjob") => match grant {
            Some(Grant::Job { job, .. }) => job_event(h, job, JobEvent::Renewed),
            _ => (401, Value::Null),
        },
        ("POST", "run/completejob") => match grant {
            Some(Grant::Job { job, .. }) => {
                let c = jbody.get("conclusion").and_then(Value::as_str).and_then(runsvc::conclusion);
                match c {
                    Some(c) => job_event(h, job, JobEvent::Finished(c)),
                    None => (400, json!({"source": "actions-run-service", "statusCode": 400})),
                }
            }
            _ => (401, Value::Null),
        },
        ("POST", r) if r.starts_with("results/twirp/") => match grant {
            Some(Grant::Job { job, .. }) => results(h, job, r.rsplit('/').next().unwrap_or(""), &jbody),
            _ => (401, Value::Null),
        },
        ("PUT", r) if r.starts_with("blob/") => {
            let p = format!("{}/blobs/{}", h.out, r.trim_start_matches("blob/").replace('/', "_"));
            std::fs::create_dir_all(format!("{}/blobs", h.out)).unwrap();
            std::fs::OpenOptions::new().create(true).append(true).open(&p).unwrap().write_all(body.as_bytes()).unwrap();
            (201, Value::Null)
        }
        _ => (404, json!({"message": "not found"})),
    };
    rec["status"] = json!(status);
    if rel != "broker/message" || !reply.is_null() {
        rec["reply"] = if rel == "run/acquirejob" { json!("job message") } else { reply.clone() };
    }
    h.log(rec);
    if rel == "run/acquirejob" && status == 200 {
        std::fs::write(format!("{}/acquirejob.json", h.out), serde_json::to_string_pretty(&reply).unwrap()).unwrap();
    }
    respond(req, status, reply);
}

fn token(h: &Host, f: &BTreeMap<String, String>) -> (u16, Value) {
    let bad = (400, json!({"error": "invalid_client"}));
    let Some(a) = f.get("client_assertion") else { return bad };
    let Some(runner) = runsvc::assertion_runner(a) else { return bad };
    let key = h.w.with(move |w| w.fold().runners.get(&runner).map(|r| r.key.clone()));
    let Some(key) = key else { return bad };
    if runsvc::verify_assertion(a, runner, &key, &h.urls.token(), now()).is_err() {
        return bad;
    }
    let exp = now().0 / 1000 + runsvc::RUNNER_TOKEN_TTL_S;
    let t = runsvc::sign_token(&h.key, Grant::Runner { runner }, exp, None);
    (200, json!({"access_token": t, "token_type": "bearer", "expires_in": runsvc::RUNNER_TOKEN_TTL_S}))
}

/// The broker's long poll: a cancellation for the job this runner holds, or a committed `Matched` on the oldest
/// queued job whose labels it takes, or an empty 200 after `POLL_HOLD`.
fn poll(h: &Host, runner: RunnerId) -> (u16, Value) {
    let until = Instant::now() + POLL_HOLD;
    while Instant::now() < until {
        let urls = h.urls.clone();
        let got = h.w.with(move |w| {
            if let Some(job) = w.handed.get(&runner).copied() {
                let cancelled = matches!(
                    w.fold().job(job).map(|j| &j.state),
                    Some(JobState::Completed { conclusion: graftyard_actions::Conclusion::Cancelled, .. })
                );
                if cancelled && !w.cancelled_sent.get(&job).copied().unwrap_or(false) {
                    w.cancelled_sent.insert(job, true);
                    w.messages += 1;
                    return Some((200, runsvc::job_cancellation(w.messages, job)));
                }
            }
            // A runner takes one job at a time: none while the job it was handed is unfinished.
            if w.handed.get(&runner).is_some_and(|j| w.fold().job(*j).is_some_and(|j| j.state.completed().is_none())) {
                return None;
            }
            let Some(rec) = w.fold().runners.get(&runner).cloned() else {
                return Some((400, json!({"source": "actions-broker-listener", "errorKind": "RunnerSessionInvalid"})));
            };
            let pick =
                w.fold().queued().into_iter().find(|(_, l, _, _)| l.fits(&rec.labels)).map(|(j, _, _, v)| (j, v));
            let (job, version) = pick?;
            let r =
                w.cmd(ActionsActor::Runner(runner), ActionsOp::Job { job, version, event: JobEvent::Matched(runner) });
            if !matches!(r, Some(Ok(_))) {
                return None;
            }
            w.handed.insert(runner, job);
            w.messages += 1;
            Some((200, runsvc::job_request(&urls, w.messages, job)))
        });
        if let Some(r) = got {
            return r;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    (200, Value::Null)
}

fn acquire(h: &Host, runner: RunnerId, b: &Value) -> (u16, Value) {
    let refuse = |code: u16| (code, json!({"source": "actions-run-service", "statusCode": code}));
    let Some(job) = b.get("jobMessageId").and_then(Value::as_str).and_then(runsvc::job_of_guid) else {
        return refuse(404);
    };
    let got = h.w.with(move |w| {
        let j = w.fold().job(job).cloned()?;
        let r =
            w.cmd(ActionsActor::Runner(runner), ActionsOp::Job { job, version: j.version, event: JobEvent::Acquired });
        matches!(r, Some(Ok(_))).then(|| (w.fold().run(j.run).cloned().expect("the job's run"), j))
    });
    let Some((run, j)) = got else { return refuse(409) };
    let n = now();
    let orch = runsvc::orch_id(run.run.id.0.0, job, run.run.attempt);
    let job_token = runsvc::sign_token(
        &h.key,
        Grant::Job { job, attempt: run.run.attempt },
        n.0 / 1000 + runsvc::JOB_TOKEN_TTL_S,
        Some(orch),
    );
    let matrix: Vec<(String, Value)> =
        j.spec.matrix.iter().map(|(k, v)| (k.to_string(), serde_json::from_str(v).unwrap_or(Value::Null))).collect();
    let github = runsvc::ctx_dict([
        ("repository", json!("owner/repo")),
        ("repository_owner", json!("owner")),
        ("event_name", json!(&*run.run.event)),
        ("ref", json!(&*run.run.head_ref)),
        ("sha", json!(hex(&run.run.head_sha))),
        ("server_url", json!(h.urls.server)),
        ("api_url", json!(format!("{}/api/v3", h.urls.server))),
        ("workflow", json!(&*run.run.workflow)),
        ("run_id", json!(run.run.id.0.0.to_string())),
        ("run_number", json!("1")),
        ("run_attempt", json!(run.run.attempt.to_string())),
        ("job", json!(&*j.spec.name)),
        ("actor", json!("owner")),
    ]);
    let m = JobMessage {
        job,
        attempt: run.run.attempt,
        run_id: run.run.id.0.0,
        job_name: &j.spec.name,
        display_name: &j.spec.name,
        github,
        matrix: if matrix.is_empty() {
            Value::Null
        } else {
            runsvc::ctx_dict(matrix.iter().map(|(k, v)| (k.as_str(), runsvc::context_of(v))))
        },
        strategy: Value::Null,
        steps: serde_json::from_str(&j.spec.steps).unwrap(),
        secrets: Vec::new(),
        variables: Vec::new(),
        github_token: "ghs_test",
        job_token: &job_token,
        locked_until: &graftyard_wire::time::iso(Timestamp(n.0 + 600_000)),
    };
    (200, runsvc::job_message(&h.urls, &m))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn job_event(h: &Host, job: JobId, event: JobEvent) -> (u16, Value) {
    let ev = event.clone();
    let r = h.w.with(move |w| {
        let j = w.fold().job(job).cloned()?;
        w.cmd(ActionsActor::Job(job), ActionsOp::Job { job, version: j.version, event: ev })
    });
    match (r, event) {
        (Some(Ok(ActionsOut::Job(JobState::InProgress { lease_until, .. }))), JobEvent::Renewed) => {
            (200, json!({"lockedUntil": graftyard_wire::time::iso(lease_until)}))
        }
        (Some(Ok(_)), _) => (200, json!({})),
        (Some(Err(ActionsErr::Illegal(_))), _) => (404, json!({"source": "actions-run-service", "statusCode": 404})),
        _ => (503, json!({"source": "actions-run-service", "statusCode": 503})),
    }
}

fn results(h: &Host, job: JobId, method: &str, b: &Value) -> (u16, Value) {
    let step = b.get("step_backend_id").and_then(Value::as_str).unwrap_or("job");
    let blob = |kind: &str| format!("{}blob/{}-{}-{}?sig=t", h.urls.services, job.0.0, kind, step);
    match method {
        "GetStepLogsSignedBlobURL" | "GetJobLogsSignedBlobURL" => (
            200,
            json!({"logs_url": blob("logs"), "blob_storage_type": "BLOB_STORAGE_TYPE_UNSPECIFIED", "soft_size_limit": 10 << 20}),
        ),
        "GetJobDiagLogsSignedBlobURL" => {
            (200, json!({"diag_logs_url": blob("diag"), "blob_storage_type": "BLOB_STORAGE_TYPE_UNSPECIFIED"}))
        }
        "GetStepSummarySignedBlobURL" => {
            (200, json!({"summary_url": blob("summary"), "blob_storage_type": "BLOB_STORAGE_TYPE_UNSPECIFIED"}))
        }
        _ => (200, json!({"ok": true})),
    }
}

/// The `FeedStreamUrl` socket: the runner's frames, one JSON object per message, appended to `live.jsonl`.
fn live(h: &Host, req: tiny_http::Request, grant: Option<Grant>) {
    if !matches!(grant, Some(Grant::Job { .. })) {
        return respond(req, 401, Value::Null);
    }
    let key = req
        .headers()
        .iter()
        .find(|x| x.field.equiv("Sec-WebSocket-Key"))
        .map(|x| x.value.as_str().to_string())
        .unwrap_or_default();
    let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
    let resp = tiny_http::Response::empty(101)
        .with_header(tiny_http::Header::from_bytes("Upgrade", "websocket").unwrap())
        .with_header(tiny_http::Header::from_bytes("Connection", "Upgrade").unwrap())
        .with_header(tiny_http::Header::from_bytes("Sec-WebSocket-Accept", accept).unwrap());
    let stream = req.upgrade("websocket", resp);
    let mut ws = tungstenite::WebSocket::from_raw_socket(stream, tungstenite::protocol::Role::Server, None);
    let path = format!("{}/live.jsonl", h.out);
    while let Ok(msg) = ws.read() {
        if let tungstenite::Message::Text(t) = msg {
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).unwrap();
            writeln!(f, "{{\"dt\":{},\"frame\":{}}}", h.t0.elapsed().as_millis(), t).unwrap();
        }
    }
}

/// A new ephemeral runner: a key pair, its record, and `<out>/jitconfig.txt` with the private half.
fn register(w: &mut World, urls: &Urls, out: &str) -> RunnerId {
    let key = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
    let der = key.to_public_key().to_public_key_der().unwrap().as_bytes().to_vec();
    w.registered += 1;
    let name = format!("graftyard-host-runner-{}", w.registered);
    let rec = RunnerRecord {
        scope: RunnerScope::Repo(REPO),
        name: name.as_str().into(),
        labels: LabelSet::parse(["self-hosted", "linux", "x64"]).unwrap(),
        key: der.into(),
        ephemeral: true,
        hosted: false,
        version: "2.338.2".into(),
    };
    let Some(Ok(ActionsOut::Runner(rid))) = w.cmd(ActionsActor::Admin(OWNER), ActionsOp::PutRunner(rec)) else {
        panic!("runner not registered")
    };
    let jit = runsvc::jit_config(urls, rid, &name, &format!("{}/owner/repo", urls.server), &key);
    std::fs::write(format!("{out}/jitconfig.txt.tmp"), jit).unwrap();
    std::fs::rename(format!("{out}/jitconfig.txt.tmp"), format!("{out}/jitconfig.txt")).unwrap();
    rid
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (port, wf, out) = (args[1].clone(), &args[2], args[3].clone());
    std::fs::create_dir_all(&out).unwrap();
    let urls = Urls::new(&format!("http://127.0.0.1:{port}"));
    let t = now();
    let mut shard = Group::new(ChainId::Actions(OWNER, 3), vec![ACTIONS_FORMAT], ActionsFold::default(), t);
    shard.id_block(t, IdBlock::whole(0, 9).unwrap());
    let mut w = World {
        shard,
        next: 1,
        handed: BTreeMap::new(),
        cancelled_sent: BTreeMap::new(),
        messages: 0,
        registered: 0,
        replace: false,
    };

    let rid = register(&mut w, &urls, &out);

    // The run: every job of the workflow, each matrix configuration its own job.
    let src = std::fs::read_to_string(wf).unwrap();
    let parsed = workflow::parse(wf, &src);
    assert!(parsed.ok(), "{:?}", parsed.errors);
    let mut jobs = Vec::new();
    for j in &parsed.workflow.jobs {
        let labels: Vec<String> = match j.runs_on.as_ref().map(|n| &n.t) {
            Some(workflow::template::T::Str(s)) => vec![s.clone()],
            Some(workflow::template::T::Seq(v)) => v.iter().filter_map(|n| n.str().map(String::from)).collect(),
            _ => vec!["self-hosted".into()],
        };
        let steps = runsvc::steps_json(&j.steps).to_string();
        let strategy = matrix::expand(j.strategy.as_ref(), j.name.str()).unwrap();
        let configs = strategy.map(|s| s.configurations).unwrap_or_default();
        let one = |name: String, m: SmallVec<[(Box<str>, Box<str>); 4]>| JobSpec {
            id: JobId(DbId(0)),
            name: name.into(),
            labels: LabelSet::parse(&labels).unwrap(),
            needs: SmallVec::new(),
            matrix: m,
            environment: None,
            steps: steps.clone().into(),
        };
        if configs.is_empty() || configs.iter().all(|c| c.matrix == matrix::Data::Null) {
            jobs.push(one(j.id.clone(), SmallVec::new()));
        }
        for c in configs.iter().filter(|c| c.matrix != matrix::Data::Null) {
            let matrix::Data::Map(pairs) = &c.matrix else { continue };
            let m = pairs.iter().map(|(k, v)| (k.as_str().into(), v.json().to_string().into())).collect();
            jobs.push(one(c.name.clone().unwrap_or_else(|| j.id.clone()), m));
        }
    }
    let run = WorkflowRun {
        id: graftyard_actions::RunId(DbId(0)),
        repo: REPO,
        workflow: wf.as_str().into(),
        event: "push".into(),
        head_sha: [0x11; 20],
        head_ref: "refs/heads/main".into(),
        trigger: (ChainId::Repo(REPO), Index(1)),
        attempt: 1,
        concurrency: None,
        fork: false,
    };
    let created = w.cmd(ActionsActor::Operator, ActionsOp::CreateRun { run, jobs });
    assert!(matches!(created, Some(Ok(ActionsOut::Run(_)))), "{created:?}");
    println!("runner {} registered; jit config in {out}/jitconfig.txt; listening on {}", rid.0.0, urls.server);

    let log = Arc::new(Mutex::new(std::fs::File::create(format!("{out}/calls.jsonl")).unwrap()));
    let (tx, rx) = mpsc::channel::<Work>();
    let host = Arc::new(Host {
        w: Members(Mutex::new(tx)),
        urls,
        key: ServiceKey(rand::random()),
        out: out.clone(),
        log: log.clone(),
        t0: Instant::now(),
    });
    let h0 = host.clone();
    std::thread::spawn(move || {
        let server = tiny_http::Server::http(format!("127.0.0.1:{port}")).unwrap();
        for req in server.incoming_requests() {
            let h = h0.clone();
            std::thread::spawn(move || handle(&h, req));
        }
    });
    // The members live on this thread: each request's closure, and every 500 ms a tick and the leader's lease alarm.
    let (lg, o) = (log.clone(), out.clone());
    {
        let mut w = w;
        let mut last = Instant::now();
        loop {
            if let Ok(f) = rx.recv_timeout(Duration::from_millis(100)) {
                f(&mut w);
            }
            if last.elapsed() < Duration::from_millis(500) {
                continue;
            }
            last = Instant::now();
            w.shard.tick(now());
            if w.replace && !w.fold().queued().is_empty() {
                w.replace = false;
                register(&mut w, &host.urls, &o);
            }
            if w.fold().next_deadline().is_some_and(|d| d <= now()) {
                let r = w.cmd(ActionsActor::Operator, ActionsOp::ExpireLeases);
                writeln!(lg.lock().unwrap(), "{}", json!({"alarm": "ExpireLeases", "result": format!("{r:?}")}))
                    .unwrap();
            }
            let f = w.fold();
            if !f.jobs.is_empty() && f.jobs.values().all(|j| j.state.completed().is_some()) {
                let jobs: Vec<Value> = f
                    .jobs
                    .iter()
                    .map(|(id, j)| json!({"job": id.0.0, "name": &*j.spec.name, "state": format!("{:?}", j.state), "version": j.version}))
                    .collect();
                std::fs::write(format!("{o}/fold.json"), serde_json::to_string_pretty(&jobs).unwrap()).unwrap();
                println!("all jobs completed: {}", serde_json::to_string(&jobs).unwrap());
                std::thread::sleep(Duration::from_secs(5));
                std::process::exit(0);
            }
        }
    }
}
