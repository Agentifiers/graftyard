//! The run service in memory, for an unmodified `actions/runner` (`tests/m4/wp3/run-host.sh`). One owner's Actions
//! shard is three real `graftyard_raft::Member<ActionsCmd>`s (`tests/common`), ticked every 500 ms, with the
//! leader's lease alarm; every route the runner calls commits what the hosted routes will commit, through
//! `graftyard_actions::runsvc`. Plain HTTP on 127.0.0.1, one thread per request, so a long poll holds only its own.
//!
//! `runsvc_host <port> <workflow.yml> <out dir>`: registers one runner, writes `<out>/jitconfig.txt`, creates a run
//! from the workflow's jobs on `push` to `refs/heads/main`, and serves until every job completed. `<out>/calls.jsonl`
//! logs each call, `<out>/fold.json` the final jobs, `<out>/live.jsonl` the `FeedStreamUrl` frames, and `<out>/blobs/`
//! the step logs.
//!
//! Actions OIDC (`graftyard_actions::oidc`): the host holds two RSA-2048 slots, A active, serves the issuer's
//! discovery document and both slots' JWKS under `/_services/token/.well-known/`, and mints a granted job's ID token at
//! `GET /_services/run/idtoken`. With `OIDC_ROTATE_AFTER=n` it switches to slot B after the n-th mint, as a committed
//! `Rotate` does, so a relying party sees tokens under both. Each mint is logged to `<out>/idtokens.jsonl`.
//! `MIRROR_EXTRA=owner/repo=<git dir>,...` adds public repositories to the mirror, such as an action under test.

#[path = "../tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common::Group;
use graftyard_actions::runsvc::{self, Grant, ServiceKey, Urls};
use graftyard_actions::shard::{
    ACTIONS_FORMAT, ActionsActor, ActionsCmd, ActionsErr, ActionsFold, ActionsOp, ActionsOut, RunnerRecord,
    RunnerScope, WorkflowRun,
};
use graftyard_actions::workflow;
use graftyard_actions::{JobEvent, JobId, JobState, LabelSet, RunnerId, RunnerSessionId};
use graftyard_raft::CommitError;
use graftyard_types::{AccountId, ChainId, DbId, IdBlock, Index, RepoId, Timestamp};
use rsa::pkcs8::EncodePublicKey;
use serde_json::{Value, json};

const OWNER: AccountId = AccountId(DbId(100));
const CHAIN: ChainId = ChainId::Actions(OWNER, 3);
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
    mirror: Mirror,
    /// `ActionsOidcKey`'s two slots and the active one's index.
    oidc: ([graftyard_actions::oidc::OidcKey; 2], std::sync::atomic::AtomicUsize),
    mints: std::sync::atomic::AtomicUsize,
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
        .map(|(k, v)| {
            let v = v.replace("%3A", ":").replace("%3a", ":").replace("%2F", "/").replace("%2f", "/");
            (k.to_string(), v.replace('+', " "))
        })
        .collect()
}

/// The install's repositories a `uses:` can name, as the edge's lookup holds them: by `owner/repo` lowercased, each a
/// bare git repository under `<out>/mirror/` and whether it is public. `Mirror/Hello` is public, `Mirror/Private`
/// private; both have tag `v1` and branch `main`.
struct Mirror(BTreeMap<String, (std::path::PathBuf, bool)>);

impl Mirror {
    fn build(out: &str) -> Mirror {
        let git = |dir: &std::path::Path, args: &[&str]| {
            let o = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_DATE", "2026-11-18T00:00:00Z")
                .env("GIT_COMMITTER_DATE", "2026-11-18T00:00:00Z")
                .output()
                .unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8(o.stdout).unwrap()
        };
        let mut m = BTreeMap::new();
        for (name, public) in [("Mirror/Hello", true), ("Mirror/Private", false)] {
            let dir = std::path::PathBuf::from(format!("{out}/mirror/{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let action = format!(
                "name: {name}\ninputs:\n  who:\n    default: world\nruns:\n  using: composite\n  steps:\n    - run: echo \"hello ${{{{ inputs.who }}}} from {name}\"\n      shell: bash\n"
            );
            std::fs::write(dir.join("action.yml"), action).unwrap();
            // `commit-tree`, so no hook or config changes the commit: `Mirror/Hello`'s is always
            // 24829c76e1163366bf63cb7415a07b99f433a4ff, which `tests/m4/wp3/run/uses.yml` pins.
            git(&dir, &["init", "-q", "-b", "main"]);
            git(&dir, &["add", "action.yml"]);
            let tree = git(&dir, &["write-tree"]);
            let id = ["-c", "user.name=m", "-c", "user.email=m@m"];
            let commit = git(&dir, &[&id[..], &["commit-tree", tree.trim(), "-m", "v1"]].concat());
            git(&dir, &["update-ref", "refs/heads/main", commit.trim()]);
            git(&dir, &["tag", "v1", commit.trim()]);
            m.insert(name.to_ascii_lowercase(), (dir, public));
        }
        for pair in std::env::var("MIRROR_EXTRA").unwrap_or_default().split(',').filter(|p| !p.is_empty()) {
            let (name, dir) = pair.split_once('=').expect("MIRROR_EXTRA is owner/repo=<dir>,...");
            m.insert(name.to_ascii_lowercase(), (std::path::PathBuf::from(dir), true));
        }
        Mirror(m)
    }

    /// The repository a job may read under `owner/repo`, matched case-insensitively; a private one is not found, as
    /// the edge reads every action repository anonymously.
    fn readable(&self, owner: &str, repo: &str) -> Option<&std::path::Path> {
        let (dir, public) = self.0.get(&format!("{owner}/{repo}").to_ascii_lowercase())?;
        public.then_some(dir.as_path())
    }

    fn ref_value(dir: &std::path::Path, name: &graftyard_git::refs::RefName) -> Option<graftyard_git::refs::ObjectId> {
        // Only a full ref that exists counts, as in the group's rows: `git rev-parse` alone would take a short name.
        let n = String::from_utf8_lossy(name.as_bytes()).into_owned();
        let run = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(dir).output().ok();
        if !run(&["show-ref", "--verify", "-q", &n])?.status.success() {
            return None;
        }
        let o = run(&["rev-parse", "--verify", "-q", &format!("{n}^{{commit}}")])?;
        graftyard_git::refs::ObjectId::parse_hex(String::from_utf8_lossy(&o.stdout).trim().as_bytes())
    }

    fn has_commit(dir: &std::path::Path, sha: graftyard_git::refs::ObjectId) -> bool {
        let t = std::process::Command::new("git").args(["cat-file", "-t", &sha.hex()]).current_dir(dir).output();
        t.is_ok_and(|o| o.stdout == b"commit\n")
    }
}

/// The launch service, as the edge's `actions::launch`: each action resolved in the mirror (`launch::resolve`), with
/// archive URLs this host serves itself at `/_codeload/`.
fn launch(h: &Host, b: &Value) -> (u16, Value) {
    use graftyard_actions::launch::{self, Found, Resolved};
    let Some(asked) = launch::requested(b) else { return (400, Value::Null) };
    let found: Vec<_> = asked
        .into_iter()
        .map(|a| {
            let f = match h.mirror.readable(&a.owner, &a.repo) {
                None => Found::NoRepo,
                Some(dir) => match launch::resolve(&a.version, |n| Mirror::ref_value(dir, n)) {
                    // A full SHA must be a commit in the repository, as the edge reads it from the git cache.
                    Ok(Resolved::Sha(sha)) if !Mirror::has_commit(dir, sha) => Found::NoVersion,
                    Ok(Resolved::Ref(sha) | Resolved::Sha(sha)) => {
                        let base = format!("{}/_codeload/{}/{}", h.urls.server, a.owner, a.repo);
                        let (tar_url, zip_url) = launch::archive_urls(&base, sha);
                        Found::Commit { sha, tar_url, zip_url }
                    }
                    Err(f) => f,
                },
            };
            (a, f)
        })
        .collect();
    launch::reply(&found)
}

/// `GET /_codeload/{owner}/{repo}/legacy.tar.gz/{sha}`: `git archive` with one top directory, read-gated as codeload is.
fn codeload(h: &Host, req: tiny_http::Request, rest: &str) {
    let parts: Vec<&str> = rest.splitn(4, '/').collect();
    let (dir, sha) = match parts.as_slice() {
        [o, r, "legacy.tar.gz", sha] => match h.mirror.readable(o, r) {
            Some(d) if graftyard_git::refs::ObjectId::parse_hex(sha.as_bytes()).is_some() => (d, *sha),
            _ => return respond(req, 404, json!({"message": "Not Found"})),
        },
        _ => return respond(req, 404, json!({"message": "Not Found"})),
    };
    let prefix = format!("{}-{}/", parts[1], sha);
    let out = std::process::Command::new("git")
        .args(["archive", "--format=tar.gz", &format!("--prefix={prefix}"), sha])
        .current_dir(dir)
        .output()
        .unwrap();
    if !out.status.success() {
        return respond(req, 404, json!({"message": "Not Found"}));
    }
    let r = tiny_http::Response::from_data(out.stdout)
        .with_header(tiny_http::Header::from_bytes("Content-Type", "application/x-gzip").unwrap());
    let _ = req.respond(r);
}

fn handle(h: &Host, mut req: tiny_http::Request) {
    let method = req.method().to_string();
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let grant = bearer(&req).and_then(|t| runsvc::verify_token(&t, &h.key.public(), now())).map(|(_, g)| g);
    if method == "GET"
        && let Some(rest) = path.strip_prefix("/_codeload/")
    {
        h.log(json!({"method": method, "path": path, "note": "codeload"}));
        return codeload(h, req, rest);
    }
    if method == "POST" && graftyard_actions::launch::job_of_path(&path, grant).is_some() {
        let mut body = String::new();
        let _ = req.as_reader().read_to_string(&mut body);
        let jbody: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let (status, reply) = launch(h, &jbody);
        h.log(json!({"method": method, "path": path, "grant": grant, "body": jbody, "status": status, "reply": reply}));
        return respond(req, status, reply);
    }
    // The live-log socket first: an upgrade request has no body, and reading one would wait for the client to hang up.
    if method == "GET" && path.starts_with("/_services/logrelay/") {
        h.log(json!({"method": method, "path": path, "note": "live log socket", "grant": grant}));
        return live(h, req, grant);
    }
    if method == "GET"
        && let Some(r) = oidc_get(h, &url, &path, grant)
    {
        h.log(json!({"method": method, "path": path, "grant": grant, "status": r.0}));
        return respond(req, r.0, r.1);
    }
    let rel = path.strip_prefix("/_services/").unwrap_or("");
    // actions-artifacts: binary chunks and downloads, before any body is read as text (`artifacts`).
    if rel.starts_with("run/_apis/pipelines/workflows/") || rel.starts_with("artifacts/") {
        return artifacts(h, req, &method, &url, rel, grant);
    }
    let mut body = String::new();
    let _ = req.as_reader().read_to_string(&mut body);
    let jbody: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
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

/// actions-artifacts as the edge serves it (`workers/edge/src/artifacts.rs`), on disk under `<out>/r2/` with the
/// same keys, the same isolation rule (`artifacts::authorize`, the run and repository from the fold), the same file
/// record once one job's chunks tile the file, and the stored gzip bytes answered as stored.
fn artifacts(h: &Host, mut req: tiny_http::Request, method: &str, url: &str, rel: &str, grant: Option<Grant>) {
    use graftyard_actions::artifacts::{self as art, Chunk, Container, FileRecord};
    fn header(req: &tiny_http::Request, k: &'static str) -> Option<String> {
        req.headers().iter().find(|x| x.field.equiv(k)).map(|x| x.value.as_str().to_owned())
    }
    let query = |k: &str| form(url.split_once('?').map_or("", |(_, q)| q)).remove(k);
    let r2 = |key: &str| std::path::PathBuf::from(format!("{}/r2/{key}", h.out));
    let list = |prefix: &str| -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let base = std::path::PathBuf::from(format!("{}/r2/", h.out));
        let mut stack = vec![base.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let k = p.strip_prefix(&base).unwrap().to_string_lossy().into_owned();
                    if k.starts_with(prefix) {
                        out.push((k, e.metadata().unwrap().len()));
                    }
                }
            }
        }
        out.sort();
        out
    };
    let put = |key: &str, bytes: &[u8]| {
        let p = r2(key);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    };
    let mut rec = json!({"method": method, "path": rel, "itemPath": query("itemPath"), "grant": grant});
    let Some(Grant::Job { job, .. }) = grant else { return respond(req, 401, Value::Null) };
    let (run, repo) = h.w.with(move |w| {
        let row = w.fold().job(job).cloned().expect("the job");
        (row.run, w.fold().run(row.run).expect("its run").run.repo)
    });
    let services = h.urls.services.clone();
    let (status, reply): (u16, Value) = if let Some(tail) = rel.strip_prefix("run/_apis/pipelines/workflows/") {
        let named = tail.strip_suffix("/artifacts").and_then(|r| r.parse::<i64>().ok()).unwrap_or(-1);
        let ct = Container { chain: CHAIN, repo, run: graftyard_actions::RunId(DbId(named)) };
        let mut body = String::new();
        let _ = req.as_reader().read_to_string(&mut body);
        let b: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        match (art::authorize(&ct, CHAIN, run, repo), method) {
            (Err(_), _) => (403, json!({"message": "forbidden"})),
            (Ok(()), "POST") => {
                let name = b["Name"].as_str().unwrap_or_default().to_owned();
                (201, art::artifact_json(&ct, &services, &name, -1))
            }
            (Ok(()), "PATCH") => {
                let name = query("artifactName").unwrap_or_default();
                let size = b["Size"].as_i64().unwrap_or(0);
                put(&ct.manifest_key(&name), json!({"name": name, "size": size}).to_string().as_bytes());
                (200, art::artifact_json(&ct, &services, &name, size))
            }
            (Ok(()), _) => {
                let p = ct.manifests();
                let value: Vec<Value> = list(&p)
                    .into_iter()
                    .map(|(k, _)| {
                        let m: Value = serde_json::from_slice(&std::fs::read(r2(&k)).unwrap()).unwrap();
                        art::artifact_json(&ct, &services, &k[p.len()..], m["size"].as_i64().unwrap_or(0))
                    })
                    .collect();
                (200, json!({"count": value.len(), "value": value}))
            }
        }
    } else {
        let ct = rel.strip_prefix("artifacts/").and_then(|p| Container::parse(p.split('?').next().unwrap()));
        let Some(ct) = ct.filter(|c| art::authorize(c, CHAIN, run, repo).is_ok()) else {
            h.log(json!({"method": method, "path": rel, "status": 403}));
            return respond(req, 403, json!({"message": "forbidden"}));
        };
        let path = query("itemPath").unwrap_or_default().replace("%20", " ").replace("%2B", "+");
        match method {
            "PUT" => {
                let gzip = header(&req, "Content-Encoding").is_some_and(|v| v == "gzip");
                let range = header(&req, "Content-Range").and_then(|v| art::chunk(&v));
                let mut bytes = Vec::new();
                let _ = std::io::Read::read_to_end(req.as_reader(), &mut bytes);
                let length = if gzip { header(&req, "x-tfs-filelength").and_then(|v| v.parse().ok()) } else { None };
                rec["range"] = json!(format!("{range:?}"));
                rec["gzip"] = json!(gzip);
                match range {
                    None => (400, json!({"message": "Content-Range"})),
                    Some(Chunk::Empty) => {
                        let r = FileRecord { job, upload: 0, size: 0, length: 0, gzip };
                        put(&ct.record_key(&path), &serde_json::to_vec(&r).unwrap());
                        (200, json!({}))
                    }
                    Some(Chunk::Bytes { start, end, total }) => {
                        assert_eq!(bytes.len() as u64, end - start + 1, "the chunk's body is its range");
                        let pointer =
                            std::fs::read_to_string(r2(&ct.upload_key(job, &path))).ok().and_then(|v| v.parse().ok());
                        match art::upload_for(start, pointer, rand::random::<u64>() >> 1) {
                            None => (409, json!({"message": "no upload started"})),
                            Some(upload) => {
                                if start == 0 {
                                    put(&ct.upload_key(job, &path), upload.to_string().as_bytes());
                                }
                                put(&ct.chunk_key(job, upload, &path, start), &bytes);
                                if end + 1 == total {
                                    let pre = ct.chunk_prefix(job, upload, &path);
                                    let parts: Vec<(u64, u64)> = list(&pre)
                                        .into_iter()
                                        .filter_map(|(k, n)| art::chunk_start(&pre, &k).map(|s| (s, n)))
                                        .collect();
                                    let done =
                                        art::file_done(job, upload, &parts, total, length.unwrap_or(total), gzip);
                                    if let Ok(r) = done {
                                        put(&ct.record_key(&path), &serde_json::to_vec(&r).unwrap());
                                    }
                                }
                                (200, json!({}))
                            }
                        }
                    }
                }
            }
            _ if art::check_item_path(&path).is_ok() => {
                let rk = ct.record_key(&path);
                let Ok(r) = std::fs::read(r2(&rk)).map(|b| serde_json::from_slice::<FileRecord>(&b).unwrap()) else {
                    return respond(req, 404, json!({"message": "not found"}));
                };
                let pre = ct.chunk_prefix(r.job, r.upload, &path);
                let mut body = Vec::new();
                for (k, _) in list(&pre).into_iter().filter(|(k, _)| art::chunk_start(&pre, k).is_some()) {
                    body.extend(std::fs::read(r2(&k)).unwrap());
                }
                h.log(json!({"method": method, "path": rel, "itemPath": path, "status": 200, "bytes": body.len(), "gzip": r.gzip}));
                let mut resp = tiny_http::Response::from_data(body);
                if r.gzip {
                    resp.add_header(tiny_http::Header::from_bytes("Content-Encoding", "gzip").unwrap());
                }
                let _ = req.respond(resp);
                return;
            }
            _ => {
                let name = path.clone();
                if !r2(&ct.manifest_key(&name)).exists() {
                    (404, json!({"message": "not found"}))
                } else {
                    let base = ct.record_key("");
                    let value: Vec<Value> = list(&ct.records(&name))
                        .into_iter()
                        .map(|(k, _)| {
                            let r: FileRecord = serde_json::from_slice(&std::fs::read(r2(&k)).unwrap()).unwrap();
                            art::entry_json(&ct, &services, &k[base.len()..], &r)
                        })
                        .collect();
                    (200, json!({"count": value.len(), "value": value}))
                }
            }
        }
    };
    rec["status"] = json!(status);
    h.log(rec);
    respond(req, status, reply);
}

/// The issuer's discovery document and JWKS, with no token, and a job's ID token, as the edge and the shard leader
/// answer them (`workers/edge/src/actions/oidc.rs`, `workers/site/src/actions/oidc.rs`).
fn oidc_get(h: &Host, url: &str, path: &str, grant: Option<Grant>) -> Option<(u16, Value)> {
    use graftyard_actions::oidc;
    use std::sync::atomic::Ordering::SeqCst;
    let issuer = oidc::issuer(&h.urls.server);
    match path {
        "/_services/token/.well-known/openid-configuration" => Some((200, oidc::discovery(&issuer))),
        "/_services/token/.well-known/jwks" => {
            let keys: Vec<Value> = h.oidc.0.iter().map(|k| oidc::jwk(&k.to_public_key())).collect();
            Some((200, oidc::jwks(&json!({ "keys": keys }))))
        }
        "/_services/run/idtoken" => {
            let Some(Grant::Job { job, attempt }) = grant else { return Some((401, Value::Null)) };
            let audience = form(url.split_once('?').map_or("", |(_, q)| q)).remove("audience");
            let read = h.w.with(move |w| {
                let f = w.fold();
                let row = f.job(job).cloned()?;
                let run = f.run(row.run)?.run.clone();
                let base = f.run_base(run.id).map(str::to_owned);
                Some((row, run, f.more.tokens.get(&job).cloned(), base))
            });
            let (row, run, declared, base) = read?;
            let f = oidc::Facts {
                server: &h.urls.server,
                repository: "owner/repo",
                repository_id: REPO,
                owner_id: OWNER,
                visibility: graftyard_identity::Visibility::Private,
                enterprise: None,
                run: &run,
                job: &row,
                hosted: false,
                base_ref: base.as_deref(),
            };
            let slot = h.oidc.1.load(SeqCst);
            let n = h.mints.fetch_add(1, SeqCst) + 1;
            let jti = format!("{}-{n}", job.0.0);
            let now_s = now().0 / 1000;
            let r = oidc::mint(&f, attempt, declared.as_ref(), audience.as_deref(), now_s, &jti, &h.oidc.0[slot]);
            let kid = oidc::kid(&h.oidc.0[slot].to_public_key());
            let mut log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(format!("{}/idtokens.jsonl", h.out))
                .unwrap();
            let rec = match &r {
                Ok(t) => json!({"n": n, "slot": slot, "kid": kid, "audience": audience, "token": t}),
                Err(e) => json!({"n": n, "refused": e.to_string()}),
            };
            writeln!(log, "{rec}").unwrap();
            let after: usize = std::env::var("OIDC_ROTATE_AFTER").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
            if after > 0 && n == after {
                h.oidc.1.store(1, SeqCst);
            }
            Some(match r {
                Ok(t) => (200, json!({ "value": t })),
                Err(e) => (403, json!({"source": "actions-run-service", "statusCode": 403, "message": e.to_string()})),
            })
        }
        _ => None,
    }
}

fn token(h: &Host, f: &BTreeMap<String, String>) -> (u16, Value) {
    let bad = (400, json!({"error": "invalid_client"}));
    let Some(a) = f.get("client_assertion") else { return bad };
    let Some(at) = runsvc::assertion_runner(a) else { return bad };
    let runner = at.runner;
    let key = h.w.with(move |w| w.fold().runners.get(&runner).map(|r| r.key.clone()));
    let Some(key) = key else { return bad };
    if runsvc::verify_assertion(a, at, &key, &h.urls.token(), now()).is_err() {
        return bad;
    }
    let exp = now().0 / 1000 + runsvc::RUNNER_TOKEN_TTL_S;
    let t = runsvc::sign_token(&h.key, at.chain(), Grant::Runner { runner }, exp, None);
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
            // Every runner here is ephemeral: it takes one job. The runner polls once more between `completejob`
            // and deleting its session, and a job handed to it then would be stranded.
            if w.handed.contains_key(&runner) {
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
    let declared = h.w.with(move |w| w.fold().more.tokens.get(&job).cloned());
    let id_token = graftyard_actions::oidc::id_token_granted(declared.as_ref(), run.run.fork);
    let n = now();
    let orch = runsvc::orch_id(run.run.id.0.0, job, run.run.attempt);
    let job_token = runsvc::sign_token(
        &h.key,
        CHAIN,
        Grant::Job { job, attempt: run.run.attempt },
        n.0 / 1000 + runsvc::JOB_TOKEN_TTL_S,
        Some(orch),
    );
    let a = runsvc::Acquired {
        repository: "owner/repo",
        owner_login: "owner",
        relay_site: 0,
        github_token: "ghs_test",
        job_token: &job_token,
        locked_until: Timestamp(n.0 + 600_000),
        secrets: Vec::new(),
        variables: Vec::new(),
        cache: false,
        id_token,
    };
    (200, runsvc::acquire_message(&h.urls, &run.run, &j, &a))
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
    let at = runsvc::RunnerAt { owner: OWNER, shard: 3, runner: rid };
    let jit = runsvc::jit_config(urls, at, &name, &format!("{}/owner/repo", urls.server), &key);
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
    let mut shard = Group::new(
        CHAIN,
        vec![ACTIONS_FORMAT, graftyard_actions::shard::ACTIONS_FORMAT_TOKENS],
        ActionsFold::default(),
        t,
    );
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
    let (jobs, permissions) = graftyard_actions::specs::job_specs_declared(&parsed.workflow).unwrap();
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
    // `gh-token`'s `CreateRunWith` when a job declared `permissions:`, as the site's trigger sends it.
    let op = if permissions.is_empty() {
        ActionsOp::CreateRun { run, jobs }
    } else {
        ActionsOp::CreateRunWith { run, jobs, permissions }
    };
    let created = w.cmd(ActionsActor::Operator, op);
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
        mirror: Mirror::build(&out),
        oidc: (
            [0, 1].map(|_| rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap()),
            std::sync::atomic::AtomicUsize::new(0),
        ),
        mints: std::sync::atomic::AtomicUsize::new(0),
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
