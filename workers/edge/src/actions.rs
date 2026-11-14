//! The Actions run service on the edge (`design/SHARED.md`, "Actions", Runner protocol and Logs;
//! `tests/m4/wp3/runner-protocol.md`): what an unmodified `actions/runner` calls, the web app's live-log socket, and
//! the job log read. One route line in `fetch` sends `/_services/` and `/_t/logrelay/` here.
//!
//! - **`/_services/*`.** The edge verifies the runner's bearer token offline under `RUN_SERVICE_PUBLIC`, the public half
//!   of the sites' `RUN_SERVICE_KEY` (`decisions/DECISIONS.md`, "M4-WP3: the run service's signing key, the live-log
//!   viewer path, and how the edge finds `RunnerBroker` and `LogRelay`"). The token names its Actions shard, so the
//!   call goes to that shard's leader (`/runsvc`) through the lookup. The OAuth exchange has no token yet: its
//!   assertion's `clientId` names the shard (`runsvc::client_id`), and the leader checks the signature.
//! - **`GET broker/message`** is held by the shard's `RunnerBroker`, `broker:<chain>` on the leader's site, which asks
//!   the leader to commit each match. Past `BROKER_HELD_MAX` it answers 503 with `Retry-After: 15`.
//! - **Logs.** The results service's blob URLs point back here, `/_services/blob/<owner>/<shard>/<job>/<kind>-<step>`,
//!   and a PUT lands in the owner's home bucket at `actions/<job>/<kind>-<step>` while the job is `InProgress`.
//!   `GET /repos/{o}/{r}/actions/jobs/{id}/logs` reads `actions/<job>/logs-job`.
//! - **Live log.** The runner opens `FeedStreamUrl`, `/_services/logrelay/<site>/<job>`, which goes to `LOGRELAY_<site>`,
//!   the site whose leader took `acquirejob`. A viewer opens `/_t/logrelay/repos/{owner}/{repo}/jobs/{job}`: the edge
//!   checks read access as for `/_t/ui/repos/...`, then joins the relay of the site the job's shard leader is on now,
//!   which is the relay's site unless the leader moved since `acquirejob`; a relay with no runner answers `no-stream`.
//! - **Graftyard's own routes**, `/api/graftyard/v1/repos/{o}/{r}/actions/...`, for a repository admin: `runners`
//!   registers an ephemeral runner and answers its JIT config pieces, `dispatch` runs a workflow file at a ref, and
//!   `runs/{id}` and `jobs/{id}` read the shard. They stand in for `ActionsTrigger` and GHES's registration flow until
//!   those land, and change no GHES reply.
//!
//! Nothing here calls the Cloudflare API.

use graftyard_actions::runsvc::{self, Grant, RunnerAt};
use graftyard_actions::shard::{JobRow, RunnerRecord, RunnerScope, WorkflowRun};
use graftyard_actions::{JobId, JobState, LabelSet, RunId, actions_shard};
use graftyard_identity::{Actor, Principal, RepoPermission};
use graftyard_types::{AccountId, ChainId, DbId, Index, PlacementRecord, SiteId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::api::{Read, read, unavailable};

pub const SERVICES: &str = "/_services/";
pub const VIEWER: &str = "/_t/logrelay/repos/";

fn public_key(c: &Ctx<'_>) -> Option<[u8; 32]> {
    let v = c.env.var("RUN_SERVICE_PUBLIC").ok()?.to_string();
    graftyard_lookup::unhex(v.trim())?.try_into().ok()
}

fn bearer(req: &Request) -> Option<String> {
    let h = req.headers().get("authorization").ok()??;
    let (scheme, t) = h.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| t.trim().to_owned())
}

fn json_reply(status: u16, v: &Value) -> Result<Response> {
    let r = if v.is_null() { Response::empty()? } else { Response::from_json(v)? };
    Ok(r.with_status(status))
}

fn service_error(status: u16) -> Result<Response> {
    json_reply(status, &json!({ "source": "actions-run-service", "statusCode": status }))
}

/// The owner's `Placement` record, which places its Actions shards (DESIGN Shape, `PlacementRecord`), with the shard's
/// group created on first use.
async fn shard_record(c: &Ctx<'_>, chain: ChainId) -> std::result::Result<PlacementRecord, String> {
    let ChainId::Actions(owner, _) = chain else { return Err("not an Actions shard".into()) };
    let record = crate::placement(c, owner).await?.ok_or("the owner has no Placement row")?;
    graftyard_lookup::create(&c.bindings(), chain, &record, &json!({})).await?;
    Ok(record)
}

/// One `/runsvc` call on the shard's leader: its `(status, body)` and the whole answer.
async fn runsvc_call(c: &Ctx<'_>, chain: ChainId, body: &Value) -> std::result::Result<(u16, Value), String> {
    let record = shard_record(c, chain).await?;
    let (v, _) = graftyard_lookup::call(&c.bindings(), chain, &record, "/runsvc", body).await?;
    Ok((v["status"].as_u64().unwrap_or(500) as u16, v))
}

async fn answer(c: &Ctx<'_>, chain: ChainId, body: Value) -> Result<Response> {
    match runsvc_call(c, chain, &body).await {
        Ok((s, v)) => json_reply(s, &v["body"]),
        Err(e) => {
            console_error!("runsvc {chain}: {e}");
            service_error(503)
        }
    }
}

/// A shard read (`/read` `{"q": q}`): the `value`.
async fn shard_read(c: &Ctx<'_>, chain: ChainId, q: Value) -> std::result::Result<Value, String> {
    let record = shard_record(c, chain).await?;
    let (v, _) = graftyard_lookup::call(&c.bindings(), chain, &record, "/read", &json!({ "q": q })).await?;
    Ok(v["value"].clone())
}

async fn job_row(c: &Ctx<'_>, chain: ChainId, job: JobId) -> Option<JobRow> {
    serde_json::from_value(shard_read(c, chain, json!({ "job": job.0.0 })).await.ok()?).ok()
}

/// A request under `/_services/`.
pub async fn serve(c: &Ctx<'_>, mut req: Request, rest: &str) -> Result<Response> {
    let Some(public) = public_key(c) else { return service_error(503) };
    let grant = bearer(&req).and_then(|t| runsvc::verify_token(&t, &public, crate::now()));
    let method = req.method();
    // The runner's live-log socket, forwarded before any body is read.
    if let Some(tail) = rest.strip_prefix("logrelay/") {
        let Some((_, Grant::Job { job, .. })) = grant else { return service_error(401) };
        let Some((site, j)) = tail.split_once('/') else { return service_error(404) };
        let Ok(site) = site.parse::<u8>() else { return service_error(404) };
        if j != job.0.0.to_string() {
            return service_error(401);
        }
        return relay_socket(c, req, SiteId(site), job, "runner").await;
    }
    if let Some(tail) = rest.strip_prefix("blob/") {
        return blob_put(c, &mut req, method, tail).await;
    }
    let text = req.text().await.unwrap_or_default();
    let b: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    match (method, rest, grant) {
        (Method::Post, "oauth/token", _) => {
            let a = url::form_urlencoded::parse(text.as_bytes())
                .find(|(k, _)| k == "client_assertion")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            let Some(at) = runsvc::assertion_runner(&a) else {
                return json_reply(400, &json!({ "error": "invalid_client" }));
            };
            answer(c, at.chain(), json!({ "op": "token", "assertion": a })).await
        }
        (Method::Post, "broker/session", Some((chain, g @ Grant::Runner { .. }))) => {
            answer(c, chain, json!({ "op": "session", "grant": g })).await
        }
        (Method::Delete, "broker/session", Some((chain, g @ Grant::Runner { .. }))) => {
            answer(c, chain, json!({ "op": "delete_session", "grant": g })).await
        }
        (Method::Post, "broker/acknowledge", Some(_)) => json_reply(200, &json!({})),
        (Method::Get, "broker/message", Some((chain, g @ Grant::Runner { .. }))) => poll(c, chain, g).await,
        (Method::Post, "run/acquirejob", Some((chain, g @ Grant::Runner { .. }))) => {
            let repository = repository_of(c, chain, &b).await.unwrap_or_default();
            // Until `GITHUB_TOKEN` minting lands the job's token is empty: the runner masks and passes it, and the
            // gates' workflows call no API with it.
            let body = json!({ "op": "acquire", "grant": g, "jobMessageId": b["jobMessageId"],
                               "repository": repository, "github_token": "" });
            answer(c, chain, body).await
        }
        (Method::Post, "run/renewjob", Some((chain, g @ Grant::Job { .. }))) => {
            answer(c, chain, json!({ "op": "renew", "grant": g })).await
        }
        (Method::Post, "run/completejob", Some((chain, g @ Grant::Job { .. }))) => {
            answer(c, chain, json!({ "op": "complete", "grant": g, "conclusion": b["conclusion"] })).await
        }
        (Method::Post, r, Some((chain, Grant::Job { job, .. }))) if r.starts_with("results/") => {
            let method = r.rsplit('/').next().unwrap_or("");
            if method == "WorkflowStepsUpdate" {
                keep_steps(c, chain, job, &b).await?;
            }
            results(c, chain, job, method, &b)
        }
        (_, _, None) => service_error(401),
        _ => service_error(404),
    }
}

/// The job's repository as `o/r`, for `github.repository`, read before `acquirejob` commits.
async fn repository_of(c: &Ctx<'_>, chain: ChainId, b: &Value) -> Option<String> {
    let job = b["jobMessageId"].as_str().and_then(runsvc::job_of_guid)?;
    let row = job_row(c, chain, job).await?;
    let v = shard_read(c, chain, json!({ "run": row.run.0.0 })).await.ok()?;
    let run: WorkflowRun = serde_json::from_value(v["run"]["run"].clone()).ok()?;
    let repo = crate::repo_by_id(c, run.repo).await.ok()?;
    let (owner, name, _, _) = crate::api::repo_named(c, repo).await?;
    Some(format!("{owner}/{name}"))
}

/// `GET broker/message`, held by the shard's broker on the leader's site.
async fn poll(c: &Ctx<'_>, chain: ChainId, g: Grant) -> Result<Response> {
    let Ok(record) = shard_record(c, chain).await else { return service_error(503) };
    let site = leader_site(c, chain, &record).await;
    let stub = c.bindings().object("BROKER", site, &format!("broker:{chain}"), None)?;
    let mut init = RequestInit::new();
    let body = json!({ "chain": chain.to_string(), "record": record, "grant": g });
    init.with_method(Method::Post).with_body(Some(body.to_string().into()));
    let mut r = stub.fetch_with_request(Request::new_with_init("https://broker/poll", &init)?).await?;
    let v: Value = r.json().await.unwrap_or(Value::Null);
    let status = v["status"].as_u64().unwrap_or(503) as u16;
    if status == 503 {
        let r = json_reply(503, &json!({ "source": "actions-broker-listener", "statusCode": 503 }))?;
        r.headers().set("retry-after", &v["retryAfter"].as_u64().unwrap_or(15).to_string())?;
        return Ok(r);
    }
    json_reply(status, &v["body"])
}

/// The site the shard's leader is on: the lookup's cached leader, else the one a probe read finds, else the record's
/// home.
async fn leader_site(c: &Ctx<'_>, chain: ChainId, record: &PlacementRecord) -> SiteId {
    if let Some(t) = graftyard_lookup::cached_leader(chain) {
        return t.site;
    }
    let probe = json!({ "q": { "probe": true } }).to_string();
    match graftyard_lookup::call_opt(&c.bindings(), chain, record, "/read", probe.as_bytes()).await {
        Ok(Some((_, _, t))) => graftyard_lookup::cached_leader(chain).map_or(t.site, |l| l.site),
        _ => record.home,
    }
}

/// The results service: blob URLs back to this edge, and `{"ok": true}` for metadata and step updates, which the
/// runner needs or it throws [measured].
fn results(c: &Ctx<'_>, chain: ChainId, job: JobId, method: &str, b: &Value) -> Result<Response> {
    let ChainId::Actions(owner, shard) = chain else { return service_error(400) };
    let step = b["step_backend_id"].as_str().unwrap_or("job");
    let url = |kind: &str| {
        runsvc::blob_key(job, &format!("{kind}-{step}")).map(|k| {
            format!("https://{}{SERVICES}blob/{}/{shard}/{}?sig=t", c.cfg.host, owner.0.0, &k["actions/".len()..])
        })
    };
    let (field, kind) = match method {
        "GetStepLogsSignedBlobURL" | "GetJobLogsSignedBlobURL" => ("logs_url", "logs"),
        "GetJobDiagLogsSignedBlobURL" => ("diag_logs_url", "diag"),
        "GetStepSummarySignedBlobURL" => ("summary_url", "summary"),
        _ => return json_reply(200, &json!({ "ok": true })),
    };
    let Some(u) = url(kind) else { return service_error(400) };
    json_reply(
        200,
        &json!({ field: u, "blob_storage_type": "BLOB_STORAGE_TYPE_UNSPECIFIED", "soft_size_limit": 10 << 20 }),
    )
}

/// A runner's PUT of a log blob, `blob/<owner>/<shard>/<job>/<name>`, into the owner's home bucket at
/// `actions/<job>/<name>`. The runner sends no credential with it [measured], so it is taken only while the shard has
/// the job `InProgress`. A multi-block log sends `comp=appendblock` [doc], which appends.
async fn blob_put(c: &Ctx<'_>, req: &mut Request, method: Method, tail: &str) -> Result<Response> {
    if method != Method::Put {
        return service_error(405);
    }
    let parts: Vec<&str> = tail.split('/').collect();
    let [o, s, j, name] = parts.as_slice() else { return service_error(404) };
    let (Ok(o), Ok(s), Ok(j)) = (o.parse::<i64>(), s.parse::<u8>(), j.parse::<i64>()) else {
        return service_error(404);
    };
    let job = JobId(DbId(j));
    // Only the log kinds `results` hands out; `actions/<job>/steps` is the edge's own.
    if !["logs-", "diag-", "summary-"].iter().any(|k| name.starts_with(k)) {
        return service_error(404);
    }
    let Some(key) = runsvc::blob_key(job, name) else { return service_error(404) };
    let chain = ChainId::Actions(AccountId(DbId(o)), s);
    let running = job_row(c, chain, job).await.is_some_and(|r| matches!(r.state, JobState::InProgress { .. }));
    if !running {
        return service_error(404);
    }
    let Ok(record) = shard_record(c, chain).await else { return service_error(503) };
    let bytes = req.bytes().await?;
    let bucket = c.env.bucket(&format!("BUCKET_{}", record.home.0))?;
    let append = req.url()?.query_pairs().any(|(k, v)| k == "comp" && v == "appendblock");
    let body = match (append, bucket.get(&key).execute().await?) {
        (true, Some(o)) => {
            let mut prev = match o.body() {
                Some(b) => b.bytes().await?,
                None => Vec::new(),
            };
            prev.extend_from_slice(&bytes);
            prev
        }
        _ => bytes,
    };
    bucket.put(&key, body).execute().await?;
    json_reply(201, &Value::Null)
}

/// A WebSocket upgrade forwarded to `logrelay:<job>` on `site`, as `/runner/<job>` or `/viewer/<job>`.
async fn relay_socket(c: &Ctx<'_>, req: Request, site: SiteId, job: JobId, role: &str) -> Result<Response> {
    if req.headers().get("upgrade")?.as_deref().map(str::to_ascii_lowercase).as_deref() != Some("websocket") {
        return service_error(426);
    }
    let stub = match c.bindings().object("LOGRELAY", site, &format!("logrelay:{}", job.0.0), None) {
        Ok(s) => s,
        Err(_) => return service_error(404),
    };
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(req.headers().clone());
    stub.fetch_with_request(Request::new_with_init(&format!("https://logrelay/{role}/{}", job.0.0), &init)?).await
}

/// The viewer's socket, `/_t/logrelay/repos/{owner}/{repo}/jobs/{job}`: read access as for `/_t/ui/repos/...`, and the
/// job must be one of the repository's, so a job id under another repository's path gets `no-stream`.
pub async fn viewer(c: &Ctx<'_>, req: Request, rest: &str) -> Result<Response> {
    let segs: Vec<&str> = rest.split('/').collect();
    let [o, r, "jobs", j] = segs.as_slice() else { return crate::agents::respond(c, Reply::not_found()) };
    let Ok(job) = j.parse::<i64>().map(|j| JobId(DbId(j))) else {
        return crate::agents::respond(c, Reply::not_found());
    };
    c.read_class.set(true);
    let who = crate::authenticate(c, &req).await?;
    let rd = match read(c, o, r, who.as_ref()).await? {
        Ok(rd) => rd,
        Err(x) => return crate::agents::respond(c, x),
    };
    let chain = ChainId::Actions(rd.repo.owner, actions_shard(rd.repo.id));
    let mine = async {
        let row = job_row(c, chain, job).await?;
        let v = shard_read(c, chain, json!({ "run": row.run.0.0 })).await.ok()?;
        (v["run"]["run"]["repo"].as_i64() == Some(rd.repo.id.0.0)).then_some(())
    };
    // A job of another repository joins the record's home relay under a job id no runner holds there: `no-stream`.
    let site = match mine.await {
        Some(()) => leader_site(c, chain, &rd.repo.record).await,
        None => return relay_socket(c, req, rd.repo.record.home, JobId(DbId(0)), "viewer").await,
    };
    relay_socket(c, req, site, job, "viewer").await
}

/// The job's log from the owner's home bucket for `GET /repos/{o}/{r}/actions/jobs/{id}/logs`: 200 with the plain log
/// (GHES answers 302 to a blob URL, which fetch follows to the same body), or 404 for a job of another repository or
/// one with no log yet.
pub async fn job_log(c: &Ctx<'_>, rd: &Read, id: &str) -> Result<Reply> {
    let Ok(j) = id.parse::<i64>() else { return Ok(Reply::not_found()) };
    let chain = ChainId::Actions(rd.repo.owner, actions_shard(rd.repo.id));
    let Some(row) = job_row(c, chain, JobId(DbId(j))).await else { return Ok(Reply::not_found()) };
    let run = shard_read(c, chain, json!({ "run": row.run.0.0 })).await.unwrap_or(Value::Null);
    if run["run"]["run"]["repo"].as_i64() != Some(rd.repo.id.0.0) {
        return Ok(Reply::not_found());
    }
    let bucket = c.env.bucket(&format!("BUCKET_{}", rd.repo.record.home.0))?;
    let Some(o) = bucket.get(format!("actions/{j}/logs-job")).execute().await? else { return Ok(Reply::not_found()) };
    let bytes = match o.body() {
        Some(b) => b.bytes().await?,
        None => Vec::new(),
    };
    Ok(Reply::bytes(200, "text/plain; charset=utf-8", bytes))
}

// ---- Graftyard's own Actions routes

/// `/api/graftyard/v1/repos/{o}/{r}/actions/{tail}` for a repository admin, or `None` for another path.
///
/// - `POST runners` `{"labels": [..], "key": <SPKI DER, base64>, "name"?}`: registers an ephemeral `Repo`-scope runner
///   and answers `runner`, `client_id`, and the URLs, from which the caller writes the JIT config with its private key.
/// - `POST dispatch` `{"workflow": path, "ref": name}`: runs that workflow at the ref's tip, as `workflow_dispatch`.
/// - `GET runs/{id}` and `GET jobs/{id}`: the shard's rows, and the leader's site.
pub async fn admin_route(c: &Ctx<'_>, req: &mut Request, rd: &Read, who: &Principal, tail: &[&str]) -> Result<Reply> {
    use base64::Engine as _;
    let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
    let folds: Vec<&dyn graftyard_identity::AuthzRead> =
        rd.dir.authz.iter().map(|a| a as &dyn graftyard_identity::AuthzRead).collect();
    let (Actor::User(admin), Some(RepoPermission::Admin)) =
        (who.actor, graftyard_identity::permission(who, &folds, &access))
    else {
        return Ok(Reply::not_found());
    };
    let chain = ChainId::Actions(rd.repo.owner, actions_shard(rd.repo.id));
    let method = req.method();
    let b: Value = match method {
        Method::Post => serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null),
        _ => Value::Null,
    };
    Ok(match (method, tail) {
        (Method::Post, ["runners"]) => {
            let labels: Vec<&str> = b["labels"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let key = b["key"].as_str().and_then(|k| base64::engine::general_purpose::STANDARD.decode(k).ok());
            let (Ok(labels), Some(key)) = (LabelSet::parse(labels), key) else {
                return Ok(Reply::error(422, "labels and key are required", "rest"));
            };
            let rec = RunnerRecord {
                scope: RunnerScope::Repo(rd.repo.id),
                name: b["name"].as_str().unwrap_or("graftyard-runner").into(),
                labels,
                key: key.into(),
                ephemeral: true,
                hosted: false,
                version: "2.338.2".into(),
            };
            match runsvc_call(c, chain, &json!({ "op": "register", "record": rec, "admin": admin.0 })).await {
                Ok((201, v)) => {
                    let Ok(at) = serde_json::from_value::<RunnerAt>(v["body"]["runner"].clone()) else {
                        return Ok(unavailable("the shard's register answer"));
                    };
                    let urls = runsvc::Urls::new(&format!("https://{}", c.cfg.host));
                    Reply::json(
                        201,
                        json!({
                            "runner": at.runner.0.0,
                            "client_id": runsvc::client_id(at),
                            "server_url_v2": urls.broker(),
                            "authorization_url": urls.token(),
                            "github_url": format!("https://{}/{}/{}", c.cfg.host, rd.at.owner, rd.at.name),
                        }),
                    )
                }
                Ok((s, v)) => Reply::json(s, v["body"].clone()),
                Err(e) => unavailable(&e),
            }
        }
        (Method::Post, ["dispatch"]) => dispatch(c, rd, chain, &b).await?,
        (Method::Get, [kind @ ("runs" | "jobs"), id]) => {
            let Ok(id) = id.parse::<i64>() else { return Ok(Reply::not_found()) };
            let k = if *kind == "runs" { "run" } else { "job" };
            match shard_read(c, chain, json!({ k: id })).await {
                Ok(v) if v.is_null() => Reply::not_found(),
                Ok(v) => {
                    let site = graftyard_lookup::cached_leader(chain).map(|t| t.site.0);
                    Reply::json(200, json!({ "value": v, "leader": site, "shard": chain.to_string() }))
                }
                Err(e) => unavailable(&e),
            }
        }
        _ => Reply::not_found(),
    })
}

/// `POST dispatch`: the workflow file at the ref's tip, read from the git cache, parsed, and its jobs created on the
/// repository's shard under the run key (trigger chain and index, workflow), so a retried dispatch creates one run.
async fn dispatch(c: &Ctx<'_>, rd: &Read, chain: ChainId, b: &Value) -> Result<Reply> {
    let (Some(path), Some(r)) = (b["workflow"].as_str(), b["ref"].as_str()) else {
        return Ok(Reply::error(422, "workflow and ref are required", "rest"));
    };
    let host = graftyard_wire::Host::new(&c.cfg.host, false).map_err(|e| Error::RustError(format!("{e:?}")))?;
    let view = rd.view(&host);
    let Some(sha) = graftyard_api::rest::resolve(&view, r) else { return Ok(Reply::not_found()) };
    let src = match crate::ui::shard_json(c, rd, "blob", &[("commit", sha.hex()), ("path", path.to_owned())]).await {
        Ok(v) => v["text"].as_str().unwrap_or_default().to_owned(),
        Err(x) => return Ok(x),
    };
    let parsed = graftyard_actions::workflow::parse(path, &src);
    if !parsed.ok() {
        return Ok(Reply::json(422, json!({ "message": "invalid workflow", "errors": parsed.errors })));
    }
    let jobs = match graftyard_actions::specs::job_specs(&parsed.workflow) {
        Ok(j) => j,
        Err(e) => return Ok(Reply::json(422, json!({ "message": e }))),
    };
    let run = WorkflowRun {
        id: RunId(DbId(0)),
        repo: rd.repo.id,
        workflow: path.into(),
        event: "workflow_dispatch".into(),
        head_sha: sha.0,
        head_ref: format!("refs/heads/{r}").into(),
        trigger: (ChainId::Repo(rd.repo.id), Index(rd.index)),
        attempt: 1,
        concurrency: None,
        fork: false,
    };
    Ok(match runsvc_call(c, chain, &json!({ "op": "create_run", "run": run, "jobs": jobs })).await {
        Ok((s, v)) => {
            Reply::json(s, json!({ "run": v["body"]["run"], "jobs": jobs.len(), "shard": chain.to_string() }))
        }
        Err(e) => unavailable(&e),
    })
}

/// `/api/graftyard/v1/repos/{o}/{r}/actions/...`, or `None` for any other path under `/api/graftyard/v1`.
pub async fn graftyard_route(c: &Ctx<'_>, req: &mut Request, rest: &str) -> Result<Option<Response>> {
    let segs: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
    let ["repos", o, r, "actions", tail @ ..] = segs.as_slice() else { return Ok(None) };
    let who = crate::authenticate(c, req).await?;
    let Some(who) = who else {
        return crate::agents::respond(c, Reply::error(401, "Requires authentication", "rest")).map(Some);
    };
    let reply = match read(c, o, r, Some(&who)).await? {
        Ok(rd) => admin_route(c, req, &rd, &who, tail).await?,
        Err(x) => x,
    };
    crate::agents::respond(c, reply.with("content-type", "application/json; charset=utf-8")).map(Some)
}

// ---- GHES `rest:actions` reads

/// `GET runs/{id}`, `GET runs/{id}/jobs`, and `GET jobs/{id}` under `/repos/{o}/{r}/actions/`, rendered by
/// `graftyard_actions::rest` from the repository's shard rows. A run or job of another repository is 404.
pub async fn rest_read(c: &Ctx<'_>, rd: &Read, view: &graftyard_api::view::Repo<'_>, tail: &[&str]) -> Result<Reply> {
    let chain = ChainId::Actions(rd.repo.owner, actions_shard(rd.repo.id));
    let id = |s: &str| s.parse::<i64>().ok();
    let run_id = match tail {
        ["runs", r] | ["runs", r, "jobs"] => id(r),
        ["jobs", j] => match id(j) {
            Some(j) => job_row(c, chain, JobId(DbId(j))).await.map(|row| row.run.0.0),
            None => None,
        },
        _ => None,
    };
    let Some(run_id) = run_id else { return Ok(Reply::not_found()) };
    let v = match shard_read(c, chain, json!({ "run": run_id })).await {
        Ok(v) => v,
        Err(e) => return Ok(unavailable(&e)),
    };
    let Ok(run) = serde_json::from_value::<WorkflowRun>(v["run"]["run"].clone()) else { return Ok(Reply::not_found()) };
    if run.repo != rd.repo.id {
        return Ok(Reply::not_found());
    }
    let jobs: Vec<JobRow> =
        v["jobs"].as_array().into_iter().flatten().filter_map(|j| serde_json::from_value(j.clone()).ok()).collect();
    // The fold keeps no creation time; the earliest time it holds stands in for it, a queued job's `since` or a
    // completed job's `at` [inference: both are at or after the run's creation].
    let created = jobs
        .iter()
        .filter_map(|j| match &j.state {
            JobState::Queued { since, .. } => Some(*since),
            JobState::Completed { at, .. } => Some(*at),
            _ => None,
        })
        .min()
        .map(graftyard_wire::time::iso)
        .unwrap_or_else(|| graftyard_wire::time::iso(crate::now()));
    let host = graftyard_wire::Host::new(&c.cfg.host, false).map_err(|e| Error::RustError(format!("{e:?}")))?;
    let repository = graftyard_api::activity::minimal_repository(view);
    let full_name = format!("{}/{}", rd.at.owner, rd.at.name);
    let x = graftyard_actions::rest::Ctx {
        web: &host.web(),
        api: &host.api(),
        full_name: &full_name,
        repository: &repository,
    };
    let bucket = c.env.bucket(&format!("BUCKET_{}", rd.repo.record.home.0))?;
    let render = async |j: &JobRow| -> Result<Value> {
        let steps = match bucket.get(format!("actions/{}/steps", j.spec.id.0.0)).execute().await? {
            Some(o) => match o.body() {
                Some(b) => serde_json::from_slice::<Vec<Value>>(&b.bytes().await?).unwrap_or_default(),
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        let runner = j.state.runner().map(|r| (r.0.0, format!("runner-{}", r.0.0)));
        Ok(graftyard_actions::rest::job(&x, &run, j, &created, &steps, runner.as_ref().map(|(i, n)| (*i, n.as_str()))))
    };
    Ok(match tail {
        ["runs", _] => Reply::json(
            200,
            graftyard_actions::rest::workflow_run(&x, &run, &jobs, &created, &graftyard_wire::time::iso(crate::now())),
        ),
        ["runs", _, "jobs"] => {
            let mut out = Vec::new();
            for j in &jobs {
                out.push(render(j).await?);
            }
            Reply::json(200, json!({ "total_count": out.len(), "jobs": out }))
        }
        ["jobs", j] => match jobs.iter().find(|row| Some(row.spec.id.0.0) == id(j)) {
            Some(row) => Reply::json(200, render(row).await?),
            None => Reply::not_found(),
        },
        _ => Reply::not_found(),
    })
}

/// The runner's `WorkflowStepsUpdate`, merged into `actions/<job>/steps` in the owner's home bucket, which the GHES
/// `job` reads render as `steps`. Best effort: a lost update leaves the last report.
async fn keep_steps(c: &Ctx<'_>, chain: ChainId, job: JobId, update: &Value) -> Result<()> {
    let Ok(record) = shard_record(c, chain).await else { return Ok(()) };
    let bucket = c.env.bucket(&format!("BUCKET_{}", record.home.0))?;
    let key = format!("actions/{}/steps", job.0.0);
    let mut kept: Vec<Value> = match bucket.get(&key).execute().await? {
        Some(o) => match o.body() {
            Some(b) => serde_json::from_slice(&b.bytes().await?).unwrap_or_default(),
            None => Vec::new(),
        },
        None => Vec::new(),
    };
    graftyard_actions::rest::merge_steps(&mut kept, update);
    bucket.put(&key, serde_json::to_vec(&kept)?).execute().await?;
    Ok(())
}
