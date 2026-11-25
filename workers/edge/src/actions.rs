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
//! - **`cache/*`** is actions-cache's cache v1 API and its downloads (`crate::cache`).
//! - **Live log.** The runner opens `FeedStreamUrl`, `/_services/logrelay/<site>/<job>`, which goes to `LOGRELAY_<site>`,
//!   the site whose leader took `acquirejob`. A viewer opens `/_t/logrelay/repos/{owner}/{repo}/jobs/{job}`: the edge
//!   checks read access as for `/_t/ui/repos/...`, then joins the relay of the site the job's shard leader is on now,
//!   which is the relay's site unless the leader moved since `acquirejob`; a relay with no runner answers `no-stream`.
//! - **Graftyard's own routes**, `/api/graftyard/v1/repos/{o}/{r}/actions/...`, for a repository admin: `runners`
//!   registers an ephemeral runner and answers its JIT config pieces, `dispatch` runs a workflow file at a ref, and
//!   `runs`, `runs/{id}`, and `jobs/{id}` read the shard. They stand in for push and pull request triggers and GHES's
//!   registration flow until those land (`ActionsTrigger` runs `merge_group` on the sites), and change no GHES reply.
//!
//! Nothing here calls the Cloudflare API.

use graftyard_actions::runsvc::{self, Grant, RunnerAt};
use graftyard_actions::shard::{JobRow, RunnerRecord, RunnerScope, WorkflowRun};
use graftyard_actions::{JobId, JobState, LabelSet, RunId, actions_shard};
use graftyard_identity::{Actor, AuthzFold, AuthzRead, Principal, RepoPermission};
use graftyard_types::{AccountId, ChainId, DbId, Index, PlacementRecord, SiteId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::api::{Read, read, unavailable};

mod oidc;

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
pub async fn shard_record(c: &Ctx<'_>, chain: ChainId) -> std::result::Result<PlacementRecord, String> {
    let ChainId::Actions(owner, _) = chain else { return Err("not an Actions shard".into()) };
    let record = crate::placement(c, owner).await?.ok_or("the owner has no Placement row")?;
    graftyard_lookup::create(&c.bindings(), chain, &record, &json!({})).await?;
    Ok(record)
}

/// One `/runsvc` call on the shard's leader: its `(status, body)` and the whole answer.
pub async fn runsvc_call(c: &Ctx<'_>, chain: ChainId, body: &Value) -> std::result::Result<(u16, Value), String> {
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
pub(crate) async fn shard_read(c: &Ctx<'_>, chain: ChainId, q: Value) -> std::result::Result<Value, String> {
    let record = shard_record(c, chain).await?;
    let (v, _) = graftyard_lookup::call(&c.bindings(), chain, &record, "/read", &json!({ "q": q })).await?;
    Ok(v["value"].clone())
}

pub(crate) async fn job_row(c: &Ctx<'_>, chain: ChainId, job: JobId) -> Option<JobRow> {
    serde_json::from_value(shard_read(c, chain, json!({ "job": job.0.0 })).await.ok()?).ok()
}

/// A request under `/_services/`.
pub async fn serve(c: &Ctx<'_>, mut req: Request, rest: &str) -> Result<Response> {
    // The OIDC issuer's discovery document and JWKS, which relying parties fetch with no token (`oidc`).
    if req.method() == Method::Get
        && let Some(r) = oidc::well_known(c, rest).await?
    {
        return Ok(r);
    }
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
    // actions-cache (`crate::cache`), whose chunk uploads and downloads stream and are never read here.
    if let Some(tail) = rest.strip_prefix("cache/") {
        return crate::cache::serve(c, req, tail, grant, &public).await;
    }
    // actions-artifacts (`crate::artifacts`), whose chunk uploads and downloads stream and are never read here.
    if crate::artifacts::claims(rest) {
        return crate::artifacts::serve(c, req, rest, grant).await;
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
            // An `Owner`-scope runner's request names the job's shard, which is not the one its token names (BF-55).
            let chain = match (chain, b["jobMessageId"].as_str().and_then(runsvc::owner_job_of_guid)) {
                (ChainId::Actions(owner, _), Some((shard, _))) => ChainId::Actions(owner, shard),
                _ => chain,
            };
            // A job that would run without its repository's name or the secrets it names is not started: any read
            // before the commit that fails answers 503, which the runner retries, and the job stays `Assigned`
            // until the broker resends it or its lease returns it to the queue (SHARED, "Who decrypts"; F79).
            let (repository, secrets, variables) = match acquire_config(&Acquire { c, chain }, &b).await {
                Ok(x) => x,
                Err(Refused::Gone(e)) => {
                    console_error!("acquirejob {chain}: {e}");
                    return service_error(404);
                }
                Err(Refused::Failed(e)) => {
                    console_error!("acquirejob {chain}: {e}");
                    return service_error(503);
                }
            };
            // The job's `GITHUB_TOKEN` once 0x15.5 and 0x12.14 are enabled (`gh-token`), else empty: the runner masks
            // and passes it. A token that did not mint answers 503, as any other read before the commit does.
            let github_token = match job_token(c, chain, &b).await {
                Ok(t) => t,
                Err(e) => {
                    console_error!("acquirejob {chain}: GITHUB_TOKEN: {e}");
                    return service_error(503);
                }
            };
            let body = json!({ "op": "acquire", "grant": g, "jobMessageId": b["jobMessageId"],
                               "repository": repository, "github_token": github_token,
                               "secrets": secrets, "variables": variables });
            answer(c, chain, body).await
        }
        (Method::Get, "run/idtoken", Some((chain, g @ Grant::Job { .. }))) => oidc::idtoken(c, &req, chain, g).await,
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

/// The job a request is the launch service's call for (`graftyard_actions::launch::job_of_path`): a `POST` to
/// `/actions/build/{plan}/jobs/{job}/runnerresolve/actions` carrying that job's token. `None` for every other request,
/// which then routes as it did before the launch service existed.
pub fn launch_job(c: &Ctx<'_>, req: &Request, path: &str) -> Option<JobId> {
    if req.method() != Method::Post || !path.starts_with("/actions/build/") {
        return None;
    }
    let grant = bearer(req).and_then(|t| runsvc::verify_token(&t, &public_key(c)?, crate::now())).map(|(_, g)| g);
    graftyard_actions::launch::job_of_path(path, grant)
}

/// The launch service (`design/SHARED.md`, "Actions", Runner protocol): each `uses:` the runner names, resolved in
/// the install's own repositories as an anonymous reader would see them (`crate::api::launch_resolve`), all at once.
pub async fn launch(c: &Ctx<'_>, mut req: Request) -> Result<Response> {
    use graftyard_actions::launch;
    let b: Value = serde_json::from_str(&req.text().await.unwrap_or_default()).unwrap_or(Value::Null);
    let Some(asked) = launch::requested(&b) else { return service_error(400) };
    let Ok(host) = graftyard_wire::Host::new(&c.cfg.host, false) else { return service_error(503) };
    let found = futures_util::future::join_all(asked.iter().map(|a| crate::api::launch_resolve(c, &host, a))).await;
    let (status, v) = launch::reply(&asked.into_iter().zip(found).collect::<Vec<_>>());
    if status != 200 {
        console_error!("launch {status}: {v}");
    }
    json_reply(status, &v)
}

/// The reads `acquirejob` makes before it commits, in order: the job row and its run on the job's shard, the
/// repository's owner, then the owner's login and `Authz` (which name the repository), then the job's secrets and
/// variables. The edge's `Acquire` and the test's fake.
#[allow(async_fn_in_trait)]
trait AcquireReads {
    type Repo;
    /// `None` when the shard holds no such job, or no such run.
    async fn job(&self, job: JobId) -> std::result::Result<Option<JobRow>, String>;
    async fn run(&self, run: RunId) -> std::result::Result<Option<WorkflowRun>, String>;
    async fn repo(&self, run: &WorkflowRun) -> std::result::Result<Self::Repo, String>;
    async fn named(&self, repo: Self::Repo) -> std::result::Result<(String, Self::Repo, AuthzFold), String>;
    async fn config(
        &self,
        run: &WorkflowRun,
        job: &JobRow,
        repo: &Self::Repo,
        authz: &AuthzFold,
    ) -> std::result::Result<(Pairs, Pairs), String>;
}

/// Why an acquire is not sent to the shard: the job or its run is not there (404, which the runner drops, as the
/// shard answers it), or a read failed (503, which the runner retries), naming the read.
#[derive(Debug, PartialEq, Eq)]
enum Refused {
    Gone(String),
    Failed(String),
}

/// The job's `github.repository` (`o/r`), secrets, and variables, or why not. Never an empty name: a job that ran
/// without one failed at "Prepare workflow directory" (F79, run 1966284).
async fn acquire_config<R: AcquireReads>(r: &R, b: &Value) -> std::result::Result<(String, Pairs, Pairs), Refused> {
    let gone = |e: String| Refused::Gone(e);
    let failed = |e: String| Refused::Failed(e);
    let id = b["jobMessageId"].as_str().ok_or_else(|| gone("no jobMessageId".into()))?;
    let job = runsvc::job_of_guid(id)
        .or_else(|| runsvc::owner_job_of_guid(id).map(|(_, j)| j))
        .ok_or_else(|| gone("no job".into()))?;
    let row = r.job(job).await.map_err(|e| failed(format!("job {}: {e}", job.0.0)))?;
    let row = row.ok_or_else(|| gone(format!("job {}: no such job", job.0.0)))?;
    let run = r.run(row.run).await.map_err(|e| failed(format!("run {}: {e}", row.run.0.0)))?;
    let run = run.ok_or_else(|| gone(format!("run {}: no such run", row.run.0.0)))?;
    let repo = r.repo(&run).await.map_err(|e| failed(format!("repository {}: owner: {e}", run.repo.0.0)))?;
    let (name, repo, authz) = r.named(repo).await.map_err(|e| failed(format!("repository {}: {e}", run.repo.0.0)))?;
    let (secrets, variables) =
        r.config(&run, &row, &repo, &authz).await.map_err(|e| failed(format!("secrets: {e}")))?;
    Ok((name, secrets, variables))
}

/// The job's `GITHUB_TOKEN` (`graftyard_actions::token`), or `""` before `FormatsEnable` enabled 0x15.5 and 0x12.14.
/// The owner's Actions installation is committed first if its `Authz` lacks it, with no dep and under the operator's
/// credential, so the first acquire on an owner installs it and every later one only reads (`decisions/DECISIONS.md`,
/// "`gh-token`"). Then `graft-ops` signs the job's grant: its repository, its permissions, and its own `jti`.
async fn job_token(c: &Ctx<'_>, chain: ChainId, b: &Value) -> std::result::Result<String, String> {
    use graftyard_identity::stateless::actions_installation;
    if !c.cfg.enabled.contains(&graftyard_actions::shard::ACTIONS_FORMAT_TOKENS)
        || !c.cfg.enabled.contains(&graftyard_identity::AUTHZ_FORMAT_V14)
    {
        return Ok(String::new());
    }
    let ChainId::Actions(owner, _) = chain else { return Err("not an Actions shard".into()) };
    let id = b["jobMessageId"].as_str().unwrap_or("");
    let job = runsvc::job_of_guid(id).or_else(|| runsvc::owner_job_of_guid(id).map(|(_, j)| j)).ok_or("no job")?;
    let v = shard_read(c, chain, json!({ "jobToken": job.0.0 })).await?;
    let run: WorkflowRun = serde_json::from_value(v["run"].clone()).map_err(|e| format!("run: {e}"))?;
    let declared: Option<graftyard_identity::PermissionSet> =
        serde_json::from_value(v["permissions"].clone()).map_err(|e| format!("permissions: {e}"))?;
    let record = crate::placement(c, owner).await?.ok_or("the owner has no placement")?;
    let authz = crate::authz_read(c, owner, &record).await?.ok_or("the owner has no Authz")?;
    let inst = actions_installation(owner);
    if authz.installation(inst).and_then(|i| i.row.as_ref()) != Some(&graftyard_identity::actions_row()) {
        let op =
            graftyard_identity::AuthzOp::Install { id: inst, row: graftyard_identity::actions_row(), app_owner: owner };
        let cmd = json!({ "cred": graftyard_types::CredentialShard::NONE, "op": op });
        let done = crate::ident::commit_owner(c, ChainId::Authz(owner), owner, cmd)
            .await
            .map_err(|r| format!("the Actions installation did not commit ({})", r.status))?;
        if done.get("ok").is_none() {
            return Err(format!("the Actions installation did not commit: {done}"));
        }
    }
    let grant = graftyard_actions::token::job_grant(&run, owner, job, declared.as_ref());
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(serde_json::to_string(&grant).map_err(|e| e.to_string())?.into()));
    let mut res = c.ops("https://graft-ops/mint", init).await.map_err(|e| e.to_string())?;
    if res.status_code() != 200 {
        return Err(format!("graft-ops did not mint ({})", res.status_code()));
    }
    let v: Value = res.json().await.map_err(|e| e.to_string())?;
    v["token"].as_str().map(str::to_owned).ok_or_else(|| "graft-ops answered no token".into())
}

struct Acquire<'a, 'b> {
    c: &'a Ctx<'b>,
    chain: ChainId,
}

impl AcquireReads for Acquire<'_, '_> {
    type Repo = crate::Repo;
    async fn job(&self, job: JobId) -> std::result::Result<Option<JobRow>, String> {
        let v = shard_read(self.c, self.chain, json!({ "job": job.0.0 })).await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }
    async fn run(&self, run: RunId) -> std::result::Result<Option<WorkflowRun>, String> {
        let v = shard_read(self.c, self.chain, json!({ "run": run.0.0 })).await?;
        serde_json::from_value(v["run"]["run"].clone()).map_err(|e| e.to_string())
    }
    async fn repo(&self, run: &WorkflowRun) -> std::result::Result<crate::Repo, String> {
        crate::repo_by_id(self.c, run.repo).await
    }
    async fn named(&self, repo: crate::Repo) -> std::result::Result<(String, crate::Repo, AuthzFold), String> {
        let (owner, name, repo, authz) = crate::api::repo_named_why(self.c, repo).await?;
        Ok((format!("{owner}/{name}"), repo, authz))
    }
    async fn config(
        &self,
        run: &WorkflowRun,
        job: &JobRow,
        repo: &crate::Repo,
        authz: &AuthzFold,
    ) -> std::result::Result<(Pairs, Pairs), String> {
        job_config(self.c, run, job, repo, authz).await
    }
}

type Pairs = Vec<(String, String)>;

/// The secrets and variables `job` reads (`design/SHARED.md`, "Actions", Who decrypts): its repository's, its
/// environment's, and the org rows whose visibility reaches its repository, the repository's winning a name over the
/// org's and the environment's over both, as on GitHub [doc: "Using secrets in GitHub Actions", precedence]. A
/// fork-PR run reads no secret; it still reads variables, which GitHub passes to forks [doc]. Secrets are opened by
/// `graft-ops` (`POST /actions/open`), so only the job message ever holds a plaintext, and nothing here logs one.
/// Nothing is read before 32.23 and 0x12.12 are enabled.
async fn job_config(
    c: &Ctx<'_>,
    run: &WorkflowRun,
    job: &JobRow,
    repo: &crate::Repo,
    authz: &AuthzFold,
) -> std::result::Result<(Pairs, Pairs), String> {
    use graftyard_identity::sealed::{EnvName, SecretScope};
    if !c.cfg.enabled.contains(&graftyard_git::secrets::REPO_FORMAT_SECRETS)
        || !c.cfg.enabled.contains(&graftyard_identity::AUTHZ_FORMAT_V12)
    {
        return Ok((Vec::new(), Vec::new()));
    }
    // The narrow `actions` rows, not the whole tiers: a voter sheds a whole-tier read whose tiers changed within
    // DUMP_EVERY_MS of its last dump rebuild, and 74 of BF-54 run 2's 586 acquires failed on that read while the jobs'
    // check runs committed to the repository (`decisions/OPEN-QUESTIONS.md`, F77). A failed read still fails the acquire.
    let core = match crate::api::core_rows(c, repo, "actions").await.map_err(|e| e.to_string())? {
        Ok(core) => core,
        Err(e) => return Err(format!("the repository's rows did not answer: {e}")),
    };
    let private = graftyard_git::tiers::get_rec::<graftyard_git::issues::RepoSettings>(&core, "settings", &[])
        .is_none_or(|s| s.visibility != graftyard_identity::Visibility::Public);
    let env = job.spec.environment.as_deref().map(|e| EnvName(e.into()));
    let a = authz.actions();
    // Lowest precedence first, so a later insert replaces an earlier one of the same name.
    let mut sealed = std::collections::BTreeMap::new();
    let mut vars = std::collections::BTreeMap::new();
    for (n, (scope, r)) in &a.secrets {
        if scope.reaches(None, run.repo, private, env.as_ref()) {
            sealed.insert(n.as_str().to_owned(), (r.sealed.clone(), r.key_id.clone()));
        }
    }
    for (n, (scope, r)) in &a.variables {
        if scope.reaches(None, run.repo, private, env.as_ref()) {
            vars.insert(n.as_str().to_owned(), r.value.to_string());
        }
    }
    let mut scopes = vec![SecretScope::Repo];
    scopes.extend(env.clone().map(SecretScope::Env));
    for s in &scopes {
        for (n, r) in graftyard_git::secrets::secrets(&core, s) {
            sealed.insert(n.as_str().to_owned(), (r.sealed, r.key_id));
        }
        for (n, r) in graftyard_git::secrets::variables(&core, s) {
            vars.insert(n.as_str().to_owned(), r.value.into());
        }
    }
    let vars: Pairs = vars.into_iter().collect();
    if run.fork || sealed.is_empty() {
        return Ok((Vec::new(), vars));
    }
    let req = json!({
        "keys": a.seal_keys.values().collect::<Vec<_>>(),
        "secrets": sealed.into_iter().map(|(n, (s, k))| (n, s, k)).collect::<Vec<_>>(),
    });
    let ops = c.env.service("OPS").map_err(|_| "no graft-ops binding")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(req.to_string().into()));
    let mut res = ops.fetch("https://graft-ops/actions/open", Some(init)).await.map_err(|e| e.to_string())?;
    if res.status_code() != 200 {
        return Err(format!("graft-ops answered {}", res.status_code()));
    }
    #[derive(serde::Deserialize)]
    struct Opened {
        secrets: Pairs,
        failed: Vec<String>,
    }
    let o: Opened = res.json().await.map_err(|e| e.to_string())?;
    if !o.failed.is_empty() {
        // Names only: which secrets did not open under any of the owner's keys.
        console_error!("acquirejob: secrets that did not open: {:?}", o.failed);
    }
    Ok((o.secrets, vars))
}

/// `GET broker/message`, held by the shard's broker on the leader's site. A broker that finds its site no longer
/// leads (`moved`), or that does not answer (its site is gone), makes the edge forget the leader and ask the broker on
/// the site a fresh lookup finds, within the same poll (BF-55).
async fn poll(c: &Ctx<'_>, chain: ChainId, g: Grant) -> Result<Response> {
    let Ok(record) = shard_record(c, chain).await else { return service_error(503) };
    let mut v = Value::Null;
    for _ in 0..3 {
        let site = leader_site(c, chain, &record).await;
        let stub = c.bindings().object("BROKER", site, &format!("broker:{chain}"), None)?;
        let mut init = RequestInit::new();
        let body = json!({ "chain": chain.to_string(), "record": record, "grant": g });
        init.with_method(Method::Post).with_body(Some(body.to_string().into()));
        let answered = match stub.fetch_with_request(Request::new_with_init("https://broker/poll", &init)?).await {
            Ok(mut r) if r.status_code() == 200 => r.json::<Value>().await.ok(),
            _ => None,
        };
        match answered {
            Some(a) if a.get("moved").is_none() => {
                v = a;
                break;
            }
            // A moved broker's answer is an empty poll: kept, so a last moved answer reaches the runner as one.
            Some(a) => {
                v = a;
                graftyard_lookup::forget_leader(chain);
            }
            None => graftyard_lookup::forget_leader(chain),
        }
    }
    // No broker answered (its site is gone and no new leader is elected yet): an empty poll, which the runner repeats
    // at once [measured], not a 503, whose 15 to 30 s back-off would hold it past a new leader (BF-55).
    if v.is_null() {
        return json_reply(200, &Value::Null);
    }
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
pub async fn leader_site(c: &Ctx<'_>, chain: ChainId, record: &PlacementRecord) -> SiteId {
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
/// (GHES answers 302 to a blob URL, which fetch follows to the same body), the step logs so far for a job still
/// running, or 404 for a job of another repository or one with no log yet.
pub async fn job_log(c: &Ctx<'_>, rd: &Read, id: &str) -> Result<Reply> {
    let Ok(j) = id.parse::<i64>() else { return Ok(Reply::not_found()) };
    let chain = ChainId::Actions(rd.repo.owner, actions_shard(rd.repo.id));
    let Some(row) = job_row(c, chain, JobId(DbId(j))).await else { return Ok(Reply::not_found()) };
    let run = shard_read(c, chain, json!({ "run": row.run.0.0 })).await.unwrap_or(Value::Null);
    if run["run"]["run"]["repo"].as_i64() != Some(rd.repo.id.0.0) {
        return Ok(Reply::not_found());
    }
    let bucket = c.env.bucket(&format!("BUCKET_{}", rd.repo.record.home.0))?;
    let read = async |key: String| -> Result<Option<Vec<u8>>> {
        Ok(match bucket.get(key).execute().await? {
            Some(o) => Some(match o.body() {
                Some(b) => b.bytes().await?,
                None => Vec::new(),
            }),
            None => None,
        })
    };
    if let Some(bytes) = read(format!("actions/{j}/logs-job")).await? {
        return Ok(Reply::bytes(200, "text/plain; charset=utf-8", bytes));
    }
    // A job without its whole log yet: GHES serves the log so far, so here it is the step logs uploaded so far, in
    // upload order, which is step order [inference: the runner uploads each step's log as the step ends], so a step
    // still running has no lines here yet.
    let mut steps: Vec<(i64, String)> = Vec::new();
    let mut cursor = None;
    loop {
        let mut q = bucket.list().prefix(format!("actions/{j}/logs-"));
        if let Some(cur) = cursor.take() {
            q = q.cursor(cur);
        }
        let page = q.execute().await?;
        steps.extend(page.objects().iter().map(|o| (o.uploaded().as_millis() as i64, o.key())));
        match page.cursor().filter(|_| page.truncated()) {
            Some(cur) => cursor = Some(cur),
            None => break,
        }
    }
    if steps.is_empty() {
        return Ok(Reply::not_found());
    }
    steps.sort();
    let mut out = Vec::new();
    for (_, key) in steps {
        out.extend(read(key).await?.unwrap_or_default());
    }
    Ok(Reply::bytes(200, "text/plain; charset=utf-8", out))
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
            // `"scope": "owner"` registers an `Owner`-scope runner on the owner's shard 0, for an admin of the owner
            // (BF-55); it takes jobs from every repository of the owner.
            let owner_scope = b["scope"].as_str() == Some("owner");
            if owner_scope && !rd.dir.authz.iter().any(|a| a.owner == Some(rd.repo.owner) && a.is_admin(admin)) {
                return Ok(Reply::not_found());
            }
            let (scope, chain) = if owner_scope {
                (RunnerScope::Owner, ChainId::Actions(rd.repo.owner, graftyard_actions::OWNER_SCOPE_SHARD))
            } else {
                (RunnerScope::Repo(rd.repo.id), chain)
            };
            let rec = RunnerRecord {
                scope,
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
                            "github_url": if owner_scope {
                                format!("https://{}/{}", c.cfg.host, rd.at.owner)
                            } else {
                                format!("https://{}/{}/{}", c.cfg.host, rd.at.owner, rd.at.name)
                            },
                        }),
                    )
                }
                Ok((s, v)) => Reply::json(s, v["body"].clone()),
                Err(e) => unavailable(&e),
            }
        }
        (Method::Post, ["dispatch"]) => dispatch(c, rd, chain, &b).await?,
        // Every run of the repository with its event and ref, newest first (BF-37 finds `merge_group` runs by ref).
        (Method::Get, ["runs"]) => match shard_read(c, chain, json!({ "repoRuns": rd.repo.id.0.0 })).await {
            Ok(v) => Reply::json(200, v),
            Err(e) => unavailable(&e),
        },
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
    let tokens = c.cfg.enabled.contains(&graftyard_actions::shard::ACTIONS_FORMAT_TOKENS)
        && c.cfg.enabled.contains(&graftyard_identity::AUTHZ_FORMAT_V14);
    // A run marked as a fork pull request's, whose jobs read no secret (BF-59): no fork route exists yet, so the
    // admin's dispatch marks it [inference: the refusal is `acquirejob`'s, which reads only `run.fork`].
    let fork = b["fork"].as_bool().unwrap_or(false);
    let (call, jobs) = match dispatch_call(rd.repo.id, Index(rd.index), path, &src, r, sha.0, fork, tokens) {
        Ok(x) => x,
        Err(reply) => return Ok(reply),
    };
    Ok(match runsvc_call(c, chain, &call).await {
        Ok((s, v)) => Reply::json(s, json!({ "run": v["body"]["run"], "jobs": jobs, "shard": chain.to_string() })),
        Err(e) => unavailable(&e),
    })
}

/// The dispatch's `/runsvc` `create_run` call for workflow `path` (source `src`) at `sha` on branch `r`, and its job
/// count, or the 422 a bad workflow answers. Once job tokens are on (`tokens`), the jobs' declared permissions ride
/// along as the site's trigger sends them (`runsvc::create_run_call`), so a dispatched job's `GITHUB_TOKEN` and
/// `id-token: write` follow its `permissions:` (F127).
#[allow(clippy::too_many_arguments)]
fn dispatch_call(
    repo: graftyard_types::RepoId,
    index: Index,
    path: &str,
    src: &str,
    r: &str,
    sha: [u8; 20],
    fork: bool,
    tokens: bool,
) -> std::result::Result<(Value, usize), Reply> {
    let parsed = graftyard_actions::workflow::parse(path, src);
    if !parsed.ok() {
        return Err(Reply::json(422, json!({ "message": "invalid workflow", "errors": parsed.errors })));
    }
    let (jobs, permissions) = graftyard_actions::specs::job_specs_declared(&parsed.workflow)
        .map_err(|e| Reply::json(422, json!({ "message": e })))?;
    let run = WorkflowRun {
        id: RunId(DbId(0)),
        repo,
        workflow: path.into(),
        event: "workflow_dispatch".into(),
        head_sha: sha,
        head_ref: format!("refs/heads/{r}").into(),
        trigger: (ChainId::Repo(repo), index),
        attempt: 1,
        concurrency: None,
        fork,
    };
    Ok((runsvc::create_run_call(&run, &jobs, &permissions, tokens, None, None), jobs.len()))
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

/// `POST /repos/{o}/{r}/actions/runs/{run_id}/cancel`: 202 `{}` once `CancelRun` committed on the shard, for a caller
/// with write access, as on GHES [doc: GHES 3.22 REST, "Cancel a workflow run"]; a run of another repository is 404.
/// A run already completed answers 409, GitHub's "Cannot cancel a workflow run that is completed."
pub async fn cancel_run(c: &Ctx<'_>, rd: &Read, who: &Principal, run: &str) -> Result<Reply> {
    let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
    let folds: Vec<&dyn graftyard_identity::AuthzRead> =
        rd.dir.authz.iter().map(|a| a as &dyn graftyard_identity::AuthzRead).collect();
    let Actor::User(me) = who.actor else {
        return Ok(Reply::error(403, "Resource not accessible by integration", "rest"));
    };
    if graftyard_identity::permission(who, &folds, &access) < Some(RepoPermission::Write) {
        return Ok(Reply::not_found());
    }
    let Ok(run) = run.parse::<i64>() else { return Ok(Reply::not_found()) };
    let chain = ChainId::Actions(rd.repo.owner, actions_shard(rd.repo.id));
    let v = match shard_read(c, chain, json!({ "run": run })).await {
        Ok(v) => v,
        Err(e) => return Ok(unavailable(&e)),
    };
    let Ok(row) = serde_json::from_value::<graftyard_actions::shard::RunRow>(v["run"].clone()) else {
        return Ok(Reply::not_found());
    };
    if row.run.repo != rd.repo.id {
        return Ok(Reply::not_found());
    }
    if matches!(row.status, graftyard_actions::RunStatus::Completed(_)) {
        return Ok(Reply::error(409, "Cannot cancel a workflow run that is completed.", "rest/actions/workflow-runs"));
    }
    Ok(match runsvc_call(c, chain, &json!({ "op": "cancel", "run": run, "admin": me.0 })).await {
        Ok((s, v)) => Reply::json(s, v["body"].clone()),
        Err(e) => unavailable(&e),
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use graftyard_actions::JobState;
    use graftyard_actions::shard::JobSpec;
    use graftyard_identity::{AuthzFold, NameEpoch, RepoEntry};
    use graftyard_types::{AccountId, ChainId, DbId, Index, RepoId, Timestamp};
    use serde_json::json;

    use super::{AcquireReads, JobId, JobRow, Pairs, Refused, RunId, WorkflowRun, acquire_config, runsvc};

    /// Which read fails, if any; whether the shard holds the job; every read the acquire made, in order.
    struct Fake {
        fail: Option<&'static str>,
        gone: bool,
        made: Cell<u8>,
    }

    impl Fake {
        fn read(&self, what: &'static str) -> Result<(), String> {
            self.made.set(self.made.get() + 1);
            if self.fail == Some(what) { Err(format!("{what} did not answer")) } else { Ok(()) }
        }
    }

    const REPO: RepoId = RepoId(DbId(65562));

    impl AcquireReads for Fake {
        type Repo = RepoId;
        async fn job(&self, job: JobId) -> Result<Option<JobRow>, String> {
            self.read("job")?;
            if self.gone {
                return Ok(None);
            }
            let spec = JobSpec {
                id: job,
                name: "build".into(),
                labels: Default::default(),
                needs: Default::default(),
                matrix: Default::default(),
                environment: None,
                steps: "[]".into(),
            };
            let state = JobState::Assigned { runner: graftyard_actions::RunnerId(DbId(9)), lease_until: Timestamp(1) };
            Ok(Some(JobRow { run: RunId(DbId(7)), spec, state, version: 1 }))
        }
        async fn run(&self, run: RunId) -> Result<Option<WorkflowRun>, String> {
            self.read("run")?;
            Ok(Some(WorkflowRun {
                id: run,
                repo: REPO,
                workflow: ".github/workflows/bf60.yml".into(),
                event: "workflow_dispatch".into(),
                head_sha: [0; 20],
                head_ref: "refs/heads/main".into(),
                trigger: (ChainId::Repo(REPO), Index(1)),
                attempt: 1,
                concurrency: None,
                fork: false,
            }))
        }
        async fn repo(&self, run: &WorkflowRun) -> Result<RepoId, String> {
            self.read("owner")?;
            Ok(run.repo)
        }
        async fn named(&self, repo: RepoId) -> Result<(String, RepoId, AuthzFold), String> {
            self.read("authz")?;
            let mut a = AuthzFold::new(AccountId(DbId(3)));
            let key = graftyard_identity::RepoName::parse("wp4-bf60").unwrap().key();
            a.names.insert(key, RepoEntry::Live { repo, epoch: NameEpoch(1) });
            let name = a.id_name(repo).ok_or("no name")?.as_str().to_owned();
            Ok((format!("m3org/{name}"), repo, a))
        }
        async fn config(
            &self,
            _: &WorkflowRun,
            _: &JobRow,
            _: &RepoId,
            _: &AuthzFold,
        ) -> Result<(Pairs, Pairs), String> {
            self.read("secrets")?;
            Ok((vec![("TOKEN".into(), "s".into())], vec![]))
        }
    }

    fn acquire(fail: Option<&'static str>, gone: bool) -> (Result<(String, Pairs, Pairs), Refused>, u8) {
        use futures_util::FutureExt;
        let f = Fake { fail, gone, made: Cell::new(0) };
        let b = json!({ "jobMessageId": runsvc::owner_job_guid(5, JobId(DbId(1966290))) });
        let out = acquire_config(&f, &b).now_or_never().expect("ready");
        (out, f.made.get())
    }

    /// F79 (run 1966284): a failed read before `acquirejob` commits gave the runner a job message with an empty
    /// `github.repository` and no secrets, and the runner failed the job. Each read that fails now refuses the acquire
    /// as `Failed` (503, retried), naming the read, and the reads after it are not made; only when all answer is there
    /// a name and the secrets. A job the shard does not hold is `Gone` (404, which the runner drops), as the shard
    /// itself answers.
    #[test]
    fn an_acquire_whose_repository_read_fails_is_refused_never_sent_without_a_name_or_secrets() {
        let (ok, made) = acquire(None, false);
        assert_eq!(ok, Ok(("m3org/wp4-bf60".into(), vec![("TOKEN".into(), "s".into())], vec![])));
        assert_eq!(made, 5);
        for (i, read) in ["job", "run", "owner", "authz", "secrets"].into_iter().enumerate() {
            let (out, made) = acquire(Some(read), false);
            let Err(Refused::Failed(e)) = out else { panic!("{read}: {out:?}") };
            assert!(e.contains(&format!("{read} did not answer")), "{read}: {e}");
            assert_eq!(usize::from(made), i + 1, "{read}: no read after the one that failed");
        }
        let (out, made) = acquire(None, true);
        assert!(matches!(out, Err(Refused::Gone(_))), "{out:?}");
        assert_eq!(made, 1);
        use futures_util::FutureExt;
        let f = Fake { fail: None, gone: false, made: Cell::new(0) };
        let none = acquire_config(&f, &json!({})).now_or_never().expect("ready");
        assert!(matches!(none, Err(Refused::Gone(_))), "a request naming no job");
    }

    /// F127: a dispatched workflow whose job declares `id-token: write` gets `GenerateIdTokenUrl` once job tokens are
    /// on. The dispatch's call goes through the site's decode (`create_run_op`) and the shard's `decide` and fold, and
    /// the job message reads the grant from the fold as the shard leader does. On main the dispatch sent no
    /// permissions, so the run was `CreateRun`, no `JobTokens` row existed, and the URL was missing.
    #[test]
    fn a_dispatched_job_granted_id_token_write_gets_the_token_url_once_job_tokens_are_on() {
        use graftyard_actions::shard::{ActionsActor, ActionsCmd, ActionsFold, ActionsOp};
        use graftyard_raft::{Command as _, EntryBody, Fold as _, LogEntry, Scratch as _, TierSet};
        use graftyard_types::IdBlock;

        const SRC: &str = "on: workflow_dispatch\npermissions:\n  contents: read\n  id-token: write\njobs:\n  \
                           sts:\n    runs-on: self-hosted\n    steps:\n      - run: \"true\"\n  denied:\n    \
                           runs-on: self-hosted\n    permissions:\n      contents: read\n    steps:\n      - run: \
                           \"true\"\n";
        let urls = runsvc::Urls::new("https://graft.example");
        let granted = |tokens: bool, fork: bool| -> Vec<(String, bool)> {
            let (call, n) =
                super::dispatch_call(REPO, Index(4), ".github/workflows/bf70.yml", SRC, "main", [7; 20], fork, tokens)
                    .unwrap();
            assert_eq!(n, 2);
            let (_, op) = runsvc::create_run_op(&call).unwrap();
            assert_eq!(matches!(op, ActionsOp::CreateRunWith { .. }), tokens, "{op:?}");
            let mut f = ActionsFold::default();
            f.apply_id_block(IdBlock::whole(0, 9).unwrap());
            let cmd = ActionsCmd { actor: ActionsActor::Operator, op };
            let (evs, _) = cmd.decide(&f.scratch_at(Timestamp(5), TierSet::CORE), &[]).unwrap();
            let mut sc = f.scratch_at(Timestamp(5), TierSet::CORE);
            for ev in &evs {
                sc.check(ev).unwrap();
                sc.apply(ev);
            }
            f.apply_entry(&LogEntry {
                term: graftyard_raft::Term(1),
                index: Index(1),
                prev: graftyard_raft::PREV_ZERO,
                format: graftyard_actions::shard::ACTIONS_FORMAT_TOKENS,
                tiers: TierSet::CORE,
                at: Timestamp(5),
                requests: Default::default(),
                extend: None,
                body: EntryBody::Events(evs.into_vec()),
            });
            let run = f.runs.values().next().unwrap().run.clone();
            let mut out = Vec::new();
            for row in f.jobs.values() {
                let a = runsvc::Acquired {
                    repository: "m3org/bf70",
                    owner_login: "m3org",
                    relay_site: 0,
                    github_token: "",
                    job_token: "jt",
                    locked_until: Timestamp(9),
                    secrets: vec![],
                    variables: vec![],
                    cache: false,
                    // As the shard leader reads it (`workers/site/src/actions.rs`, `acquire`).
                    id_token: graftyard_actions::oidc::id_token_granted(f.more.tokens.get(&row.spec.id), run.fork),
                };
                let m = runsvc::acquire_message(&urls, &run, row, &a);
                let data = &m["resources"]["endpoints"][0]["data"];
                out.push((row.spec.name.to_string(), data.get("GenerateIdTokenUrl").is_some()));
            }
            out.sort();
            out
        };
        assert_eq!(granted(true, false), [("denied".into(), false), ("sts".into(), true)]);
        assert_eq!(
            granted(false, false),
            [("denied".into(), false), ("sts".into(), false)],
            "job tokens off: as before"
        );
        assert_eq!(granted(true, true), [("denied".into(), false), ("sts".into(), false)], "a fork's run: never");
    }
}
