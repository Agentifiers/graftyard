//! The Workers host of the controller's `TaskBox` (`graftyard_git::taskbox`; `design/AGENT-LAYER.md`, Shape,
//! "Controller"; DECISIONS "`controller-taskbox`"): one Durable Object per Graftyard-run task, named by the task's id,
//! on this script's one `durable_object`-policy container application. It keeps the box in storage under `box`, runs
//! the task's one Sandbox in `ctx.container`, and keeps one alarm every `HEARTBEAT`. Bindings beyond `worker.rs`'s:
//!
//! - `TASKBOX`, this class; `ctx.exports.Outbound`, the Sandbox's outbound router (`entry.mjs`).
//! - `AGENT_APP_ID` and `WORKSPACE_APP_ID`, variables, and `AGENT_APP_KEY` and `WORKSPACE_APP_KEY`, the Secrets Store
//!   bindings of the two Apps' PEM keys; `MODEL_KEY`, the Secrets Store binding of the model key.
//! - `AGENTS_FRONT`, the `fetch()` service binding to `graft-agents-front` with `props` `{"pointer": true}`, which only
//!   this binding can set: the alarm reads the `ControllerPointer` of its view there (`GET /_agents_front/pointer`).
//!
//! What the object does:
//!
//! - **Start.** `POST /start` (from this script's own code, never the Internet) with the task, its outbound route,
//!   the harness's command, and the two installations. The container starts with no Internet, this version's
//!   `sandbox` image, and the harness as its entrypoint, with `GRAFTYARD_SANDBOX` and `GRAFTYARD_START` (the commit
//!   to start from, empty for the base); its HTTP and HTTPS go to `Outbound` with `{ task, sandbox }` as props.
//! - **Outbound.** `TaskBox::admit` refuses a Sandbox the task no longer holds (counted in `late`, logged).
//!   `graftyard_git::outbound` refuses or rewrites the request; the host attaches the credential, sends it with
//!   `redirect: "manual"`, and for a receive-pack reads the host's report (`acked_pushes`), so `last_push` moves only
//!   on the host's `ok`. The Sandbox's own reports (usage, finished, failed) go to `REPORT_HOST`, which the router
//!   answers here and never forwards.
//! - **Tokens.** A `graftyard-agent` token for the base and a `graftyard-workspace` token for the push target, each
//!   from `POST /api/v3/app/installations/{id}/access_tokens` on the install with `repositories` and `graftyard_task`
//!   naming the task, under an App JWT (`graftyard_identity::appauth`), cached in the object until
//!   `TOKEN_SLACK_MS` before it expires.
//! - **Alarm.** The container's state, this object's version (`VERSION`), and the pointer go to `TaskBox::on`, and the
//!   host does what it answers.
//!
//! No request path calls the Cloudflare API, and nothing logs a token, a key, or a body.

use std::cell::RefCell;
use std::time::Duration;

use base64::Engine as _;
use futures_util::future::{Either, select};
use graftyard_git::controller::{Delivery, Resume, SandboxId, TASK_WALL, Task, TaskBudget, TaskEvent};
use graftyard_git::outbound::{Allowed, Cred, OutReq, Route, Scheme, outbound};
use graftyard_git::taskbox::{self as tb, Act, HEARTBEAT, Input, Pointer, Report, acked_pushes};
use graftyard_identity::appauth::{app_key, sign_app_jwt};
use graftyard_identity::{AppId, SessionId, WorkspaceMode};
use graftyard_types::{DbId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use worker::js_sys::{self, Array, Function, Object, Promise, Reflect};
use worker::wasm_bindgen::JsValue;
use worker::wasm_bindgen_futures::JsFuture;
use worker::*;

/// The header `Outbound` sets from its props: `{ task, sandbox }`.
const PROPS_HEADER: &str = "x-graftyard-outbound";
/// The host the Sandbox reports its own events to; the router answers it and never forwards it.
const REPORT_HOST: &str = "controller.graftyard.internal";
/// The container's inactivity timeout, renewed every alarm: the platform's maximum, 6 h.
const INACTIVITY_MS: f64 = 6.0 * 3_600_000.0;
/// How long the alarm waits for `graft-agents-front`'s pointer [guess]; past it the pointer is `Unread`.
const POINTER_READ_MS: u64 = 2_000;
/// A cached token is reminted this long before it expires.
const TOKEN_SLACK_MS: u64 = 300_000;
/// A task's model budget until the controller's settings hold the org's [guess]; the wall is the design's 60 minutes.
const BUDGET: TaskBudget = TaskBudget { task_tokens: 2_000_000, org_month_tokens: 200_000_000, wall: TASK_WALL };

/// A signed `graftyard_task` delivery (`worker.rs`, after `intake` accepted it): `created` starts the task's box, and
/// `cancelled` stops it. The box is the object named by the task's id, so a repeated `created` finds its box and
/// changes nothing (409), and two deliveries never start two Sandboxes. Bindings: `GRAFTYARD_HOST`, the install's
/// host; `HARNESS`, the harness's command as a JSON array; `ROUTE_OTHERS`, the route's model and registry arms as JSON
/// (`graftyard_git::outbound::Allowed`), `[]` for a harness that calls no model. In-repo mode only (AM3-WP3's
/// minimum): the host refuses a fork-mode create, so the workspace is the base.
pub async fn deliver(env: &Env, payload: &Value) -> Result<Response> {
    let Some((action, task)) = Task::of_delivery(payload, WorkspaceMode::InRepo, BUDGET) else {
        return reply(422, json!({ "message": "not a Graftyard-run task delivery" }));
    };
    let stub = env.durable_object("TASKBOX")?.get_by_name(&task.id.text())?;
    if action == Delivery::Cancelled {
        return stub.fetch_with_str("https://taskbox/cancel").await;
    }
    let host = env.var("GRAFTYARD_HOST")?.to_string();
    let command: Vec<String> = serde_json::from_str(&env.var("HARNESS")?.to_string()).map_err(err)?;
    let others: Vec<Allowed> = match env.var("ROUTE_OTHERS") {
        Ok(v) => serde_json::from_str(&v.to_string()).map_err(err)?,
        Err(_) => Vec::new(),
    };
    let route = Route::of(&task, &host, others).ok_or_else(|| err("no route for the task"))?;
    let repo = task.base.name.as_str().to_owned();
    let agent = payload["installation"]["id"].as_i64().ok_or_else(|| err("the delivery names no installation"))?;
    let workspace = workspace_installation(env, &host, &task).await?;
    let start = Start {
        task,
        route,
        command,
        agent: Inst { installation: agent, repo: repo.clone() },
        workspace: Inst { installation: workspace, repo },
    };
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(serde_json::to_string(&start)?.into()));
    stub.fetch_with_request(Request::new_with_init("https://taskbox/start", &init)?).await
}

/// A signed `issues.assigned` delivery naming `graftyard-agent`'s bot (`worker.rs`): the controller's one create,
/// `POST /api/graftyard/v1/repos/{o}/{r}/tasks` with the issue's number under a `graftyard-agent` installation token
/// minted without `graftyard_task` (AGENT-LAYER, "Tasks and the activity feed"). The payload is a trigger only: the
/// host's `decide` reads the issue's latest `Assigned` entry and takes its actor as the requester, or refuses. The
/// answer is the create's.
pub async fn assigned(env: &Env, payload: &Value) -> Result<Response> {
    let (Some(installation), Some(full), Some(n)) = (
        payload["installation"]["id"].as_i64(),
        payload["repository"]["full_name"].as_str(),
        payload["issue"]["number"].as_u64(),
    ) else {
        return reply(422, json!({ "message": "not an issue assignment delivery" }));
    };
    let Some((_, repo)) = full.split_once('/') else {
        return reply(422, json!({ "message": "not an issue assignment delivery" }));
    };
    let host = env.var("GRAFTYARD_HOST")?.to_string();
    let jwt = app_jwt(env, "AGENT").await?;
    let h = Headers::new();
    h.set("authorization", &format!("Bearer {jwt}"))?;
    h.set("accept", "application/vnd.github+json")?;
    h.set("content-type", "application/json")?;
    h.set("user-agent", "graft-controller")?;
    let mut init = RequestInit::new();
    let body = json!({ "repositories": [repo] });
    init.with_method(Method::Post).with_headers(h.clone()).with_body(Some(body.to_string().into()));
    let url = format!("https://{host}/api/v3/app/installations/{installation}/access_tokens");
    let mut r = Fetch::Request(Request::new_with_init(&url, &init)?).send().await?;
    let v: Value = r.json().await.unwrap_or(Value::Null);
    let Some(token) = v["token"].as_str().filter(|_| r.status_code() == 201) else {
        return Err(err(format!("AGENT access_tokens: {} {}", r.status_code(), v["message"])));
    };
    h.set("authorization", &format!("token {token}"))?;
    let mut init = RequestInit::new();
    let body = json!({ "issue_number": n });
    init.with_method(Method::Post).with_headers(h).with_body(Some(body.to_string().into()));
    let url = format!("https://{host}/api/graftyard/v1/repos/{full}/tasks");
    let mut r = Fetch::Request(Request::new_with_init(&url, &init)?).send().await?;
    let code = r.status_code();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    console_log!("graft-controller: create from {full}#{n}: {code}");
    reply(code, v)
}

/// `graftyard-workspace`'s installation on the task's push target, the base in-repo: `GET
/// /repos/{owner}/{repo}/installation` under its App JWT.
async fn workspace_installation(env: &Env, host: &str, task: &Task) -> Result<i64> {
    let jwt = app_jwt(env, "WORKSPACE").await?;
    let h = Headers::new();
    h.set("authorization", &format!("Bearer {jwt}"))?;
    h.set("accept", "application/vnd.github+json")?;
    h.set("user-agent", "graft-controller")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(h);
    let (o, n) = (task.base.owner.as_str(), task.base.name.as_str());
    let url = format!("https://{host}/api/v3/repos/{o}/{n}/installation");
    let mut r = Fetch::Request(Request::new_with_init(&url, &init)?).send().await?;
    let v: Value = r.json().await.unwrap_or(Value::Null);
    match (r.status_code(), v["id"].as_i64()) {
        (200, Some(id)) => Ok(id),
        (code, _) => Err(err(format!("graftyard-workspace installation on {o}/{n}: {code} {}", v["message"]))),
    }
}

/// An App JWT of `app` (`AGENT` or `WORKSPACE`) from its id variable and its Secrets Store key.
async fn app_jwt(env: &Env, app: &str) -> Result<String> {
    let id: i64 = env.var(&format!("{app}_APP_ID"))?.to_string().parse().map_err(err)?;
    let pem = env.secret_store(&format!("{app}_APP_KEY"))?.get().await?.ok_or_else(|| err("no App key"))?;
    Ok(sign_app_jwt(&app_key(&pem).map_err(err)?, AppId(DbId(id)), Timestamp(now_ms())))
}

/// One App's installation for the task, and the repository its tokens name.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Inst {
    pub installation: i64,
    pub repo: String,
}

/// What `POST /start` takes and the object stores beside the box.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Start {
    pub task: Task,
    pub route: Route,
    pub command: Vec<String>,
    pub agent: Inst,
    pub workspace: Inst,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    b: tb::TaskBox,
    start: Start,
}

#[derive(Deserialize)]
struct Props {
    sandbox: SandboxId,
}

#[durable_object]
pub struct TaskBox {
    state: State,
    env: Env,
    /// (credential, token, expires ms): the installation tokens this object minted.
    tokens: RefCell<Vec<(Cred, String, u64)>>,
    /// Receive-packs forwarded and not yet answered, which `/status` reports: LM-3's kill row records from it whether a
    /// kill landed during a push.
    pushing: std::cell::Cell<u32>,
}

fn now_ms() -> u64 {
    Date::now().as_millis()
}

fn version(env: &Env) -> String {
    env.get_binding::<WorkerVersionMetadata>("VERSION").map(|v| v.id()).unwrap_or_default()
}

fn reply(status: u16, body: Value) -> Result<Response> {
    Ok(Response::from_json(&body)?.with_status(status))
}

fn err(e: impl std::fmt::Debug) -> Error {
    Error::RustError(format!("{e:?}"))
}

/// Calls `obj[method](...args)` and awaits its result, a promise or not.
async fn call(obj: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue> {
    let f: Function = Reflect::get(obj, &method.into())?.into();
    let out = Reflect::apply(&f, obj, &args.iter().collect::<Array>())?;
    Ok(JsFuture::from(Promise::resolve(&out)).await?)
}

fn obj(pairs: &[(&str, JsValue)]) -> Result<JsValue> {
    let o = Object::new();
    for (k, v) in pairs {
        Reflect::set(&o, &(*k).into(), v)?;
    }
    Ok(o.into())
}

/// The install's host: the route's `GraftyardHost` arm.
fn host(route: &Route) -> Option<&str> {
    route.arms.iter().find_map(|a| match a {
        Allowed::GraftyardHost { host, .. } => Some(&**host),
        _ => None,
    })
}

/// Logs a refusal's line (`tb::refused`) and answers its status.
fn refuse(b: &tb::TaskBox, r: &Report, message: &str) -> Result<Response> {
    let (status, line) = tb::refused(b, r);
    if let Some(l) = line {
        console_log!("{l}");
    }
    reply(status, json!({ "message": message }))
}

impl TaskBox {
    async fn load(&self) -> Result<Option<Stored>> {
        self.state.storage().get("box").await
    }

    async fn save(&self, s: &Stored) -> Result<()> {
        self.state.storage().put("box", s).await
    }

    fn container(&self) -> Result<Container> {
        self.state.container().ok_or_else(|| err("this object has no container"))
    }

    /// Does what the box answered.
    async fn act(&self, act: &Act, s: &Stored) -> Result<()> {
        let c = self.container()?;
        match act {
            Act::Keep => {}
            Act::Renew => {
                call(c.as_ref(), "setInactivityTimeout", &[INACTIVITY_MS.into()]).await?;
            }
            Act::Stop => {
                if c.running() {
                    c.destroy(None).await?;
                }
            }
            Act::Start { sandbox, from } => {
                if c.running() {
                    c.destroy(None).await?;
                }
                self.launch(&c, s, sandbox, *from).await?;
            }
        }
        Ok(())
    }

    /// Starts the Sandbox: no Internet, this version's `sandbox` image, the harness with where to start from, and every
    /// HTTP and HTTPS request to `Outbound` under this Sandbox's props.
    async fn launch(&self, c: &Container, s: &Stored, sandbox: &SandboxId, from: Resume) -> Result<()> {
        let image = Reflect::get(&Reflect::get(c.as_ref(), &"images".into())?, &"sandbox".into())?;
        if image.is_undefined() {
            return Err(err("no \"sandbox\" image in this version's container"));
        }
        let start = match from {
            Resume::Base => String::new(),
            Resume::Push(h) => h.hex(),
        };
        let env = obj(&[("GRAFTYARD_SANDBOX", (*sandbox.0).into()), ("GRAFTYARD_START", start.into())])?;
        let entrypoint: Array = s.start.command.iter().map(|a| JsValue::from_str(a)).collect();
        let opts = obj(&[
            ("image", image),
            ("instance", "standard-2".into()),
            ("enableInternet", false.into()),
            ("env", env),
            ("entrypoint", entrypoint.into()),
        ])?;
        call(c.as_ref(), "start", &[opts]).await?;
        let exports = Reflect::get(self.state.as_raw(), &"exports".into())?;
        let props = obj(&[("task", s.b.task.id.text().into()), ("sandbox", (*sandbox.0).into())])?;
        let router = call(&exports, "Outbound", &[obj(&[("props", props)])?]).await?;
        call(c.as_ref(), "interceptOutboundHttps", &["*".into(), router.clone()]).await?;
        call(c.as_ref(), "interceptAllOutboundHttp", &[router]).await?;
        call(c.as_ref(), "setInactivityTimeout", &[INACTIVITY_MS.into()]).await?;
        console_log!("graft-controller: task {} started {} from {:?}", s.b.task.id.text(), sandbox.0, from);
        Ok(())
    }

    /// `graft-agents-front`'s `ControllerPointer`, `Unread` unless a caught-up view answered in time.
    async fn pointer(&self) -> Pointer {
        let read = async {
            let mut r =
                self.env.service("AGENTS_FRONT")?.fetch("https://agents-front/_agents_front/pointer", None).await?;
            if r.status_code() != 200 {
                return Err(err(r.status_code()));
            }
            r.json::<Value>().await
        };
        let v = match select(Box::pin(read), Box::pin(Delay::from(Duration::from_millis(POINTER_READ_MS)))).await {
            Either::Left((Ok(v), _)) => v,
            _ => return Pointer::Unread,
        };
        match (v["caughtUp"].as_bool(), v["controller"].as_str()) {
            (Some(true), Some(p)) => Pointer::To(p.into()),
            (Some(true), None) => Pointer::Clear,
            _ => Pointer::Unread,
        }
    }

    async fn heartbeat(&self) -> Result<()> {
        // The pointer read awaits a fetch, which lets other events into the object, so the box is loaded after it:
        // loaded first, a push acked during the read would be lost when the alarm saved.
        let pointer = self.pointer().await;
        let Some(mut s) = self.load().await? else { return Ok(()) };
        let running = self.container()?.running();
        let input = Input::Alarm { running, version: version(&self.env), pointer };
        let act = s.b.on(input, now_ms()).map_err(err)?;
        if !matches!(act, Act::Keep | Act::Renew) {
            console_log!("graft-controller: task {} alarm: {:?}, hold {:?}", s.b.task.id.text(), act, s.b.hold);
        }
        self.save(&s).await?;
        self.act(&act, &s).await?;
        self.report_base().await?;
        // A final box keeps its alarm until the base answered its last state.
        let unreported = self.load().await?.is_some_and(|s| s.b.unreported().is_some());
        if !s.b.task.state.is_final() || unreported {
            self.state.storage().set_alarm(HEARTBEAT).await?;
        }
        Ok(())
    }

    /// A request the Sandbox made, from `Outbound`.
    async fn outbound(&self, mut req: Request, props: Props) -> Result<Response> {
        let Some(mut s) = self.load().await? else { return reply(403, json!({ "message": "no task" })) };
        let admitted = s.b.admit(&props.sandbox, now_ms());
        self.save(&s).await?;
        if let Err(r) = admitted {
            return refuse(&s.b, &r, "refused: this Sandbox is not the task's");
        }
        if req.url()?.host_str() == Some(REPORT_HOST) {
            return self.report(req, props.sandbox).await;
        }
        let headers: Vec<(String, String)> =
            req.headers().entries().filter(|(k, _)| k != PROPS_HEADER).map(|(k, v)| (k.to_lowercase(), v)).collect();
        let method = req.method().to_string();
        let url = req.url()?.to_string();
        let body = req.bytes().await.unwrap_or_default();
        let fwd = match outbound(OutReq { method: &method, url: &url, headers: &headers, body: &body }, &s.start.route)
        {
            Ok(f) => f,
            Err(r) => {
                console_log!("graft-controller: task {} refused {}: {}", s.b.task.id.text(), r.rule, r.text);
                let mut resp = Response::ok(format!("refused: {}\n", r.text))?.with_status(r.status);
                resp.headers_mut().set("x-graftyard-refused", r.rule)?;
                return Ok(resp);
            }
        };
        let h = Headers::new();
        for (k, v) in &fwd.headers {
            h.set(k, v)?;
        }
        if let Some(auth) = self.credential(fwd.cred, &s).await? {
            h.set("authorization", &auth)?;
        }
        let sent = fwd.body.unwrap_or(body);
        let mut init = RequestInit::new();
        init.with_method(Method::from(method.clone())).with_headers(h).with_redirect(RequestRedirect::Manual);
        if !matches!(method.as_str(), "GET" | "HEAD") {
            init.with_body(Some(js_sys::Uint8Array::from(sent.as_slice()).into()));
        }
        let push = method == "POST" && fwd.url.ends_with("/git-receive-pack");
        self.pushing.set(self.pushing.get() + u32::from(push));
        let answered = Fetch::Request(Request::new_with_init(&fwd.url, &init)?).send().await;
        self.pushing.set(self.pushing.get() - u32::from(push));
        let mut resp = answered?;
        if !push {
            return Ok(resp);
        }
        // A push: the host's report moves `last_push` on its `ok`, and the Sandbox gets the same bytes.
        let (status, out_headers) = (resp.status_code(), resp.headers().clone());
        let bytes = resp.bytes().await?;
        if let Some((_, head)) = acked_pushes(&sent, &bytes, &s.start.route.refs).pop() {
            let mut s = self.load().await?.ok_or_else(|| err("the box went away"))?;
            let pushed = Input::Report { from: props.sandbox, event: TaskEvent::Pushed { head } };
            if let Err(r) = s.b.on(pushed, now_ms()) {
                tb::refused(&s.b, &r).1.into_iter().for_each(|l| console_log!("{l}"));
            }
            self.save(&s).await?;
        }
        Ok(Response::from_bytes(bytes)?.with_status(status).with_headers(out_headers))
    }

    /// `POST https://controller.graftyard.internal/report` from the Sandbox: a JSON `TaskEvent` of its own, only
    /// `Used`, `Finished`, or `Failed`. A push is never reported this way: it counts only on the host's `ok`.
    async fn report(&self, mut req: Request, from: SandboxId) -> Result<Response> {
        let event: TaskEvent = match req.json().await {
            Ok(e @ (TaskEvent::Used(_) | TaskEvent::Finished | TaskEvent::Failed { .. })) => e,
            _ => return reply(422, json!({ "message": "a report is Used, Finished, or Failed" })),
        };
        let mut s = self.load().await?.ok_or_else(|| err("the box went away"))?;
        let out = s.b.on(Input::Report { from, event }, now_ms());
        self.save(&s).await?;
        match out {
            Ok(act) => {
                self.act(&act, &s).await?;
                self.report_base().await?;
                reply(200, json!({ "state": s.b.task.state }))
            }
            Err(r) => refuse(&s.b, &r, &format!("{r:?}")),
        }
    }

    /// The `Authorization` value for `cred`, minting the task's installation token when none is cached.
    async fn credential(&self, cred: Cred, s: &Stored) -> Result<Option<String>> {
        let (scheme, app, inst) = match cred {
            Cred::None => return Ok(None),
            Cred::ModelKey => {
                let key = self.env.secret_store("MODEL_KEY")?.get().await?.ok_or_else(|| err("MODEL_KEY is empty"))?;
                return Ok(Some(format!("Bearer {key}")));
            }
            Cred::Agent(sc) => (sc, "AGENT", &s.start.agent),
            Cred::Workspace(sc) => (sc, "WORKSPACE", &s.start.workspace),
        };
        let now = now_ms();
        let cached =
            self.tokens.borrow().iter().find(|t| t.0 == cred && t.2 > now + TOKEN_SLACK_MS).map(|t| t.1.clone());
        let token = match cached {
            Some(t) => t,
            None => {
                let (t, exp) = self.mint(app, inst, &s.b.task.id, &s.start.route).await?;
                let mut tokens = self.tokens.borrow_mut();
                tokens.retain(|t| t.0 != cred);
                tokens.push((cred, t.clone(), exp));
                t
            }
        };
        Ok(Some(match scheme {
            Scheme::Bearer => format!("Bearer {token}"),
            Scheme::Basic => {
                format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}")))
            }
        }))
    }

    /// Sends the base's task the state the box reached, if the base has not answered it (`tb::TaskBox::unreported`,
    /// OPEN-QUESTIONS A59): `PATCH /api/graftyard/v1/repos/{owner}/{repo}/tasks/{id}` under `graftyard-agent`'s token,
    /// the task's App. The box is loaded again after the request, as `heartbeat` does, so a report that came in while
    /// it waited is kept; a failed request is left for the next alarm.
    async fn report_base(&self) -> Result<()> {
        let Some(s) = self.load().await? else { return Ok(()) };
        let Some(state) = s.b.unreported() else { return Ok(()) };
        let host = host(&s.start.route).ok_or_else(|| err("the route names no host"))?.to_owned();
        let (o, n) = (s.b.task.base.owner.as_str(), s.b.task.base.name.as_str());
        let url = format!("https://{host}/api/graftyard/v1/repos/{o}/{n}/tasks/{}", s.b.task.id.text());
        let auth = self.credential(Cred::Agent(Scheme::Bearer), &s).await?.ok_or_else(|| err("no agent token"))?;
        let h = Headers::new();
        h.set("authorization", &auth)?;
        h.set("accept", "application/vnd.github+json")?;
        h.set("content-type", "application/json")?;
        h.set("user-agent", "graft-controller")?;
        let mut init = RequestInit::new();
        init.with_method(Method::Patch)
            .with_headers(h)
            .with_body(Some(json!({ "state": state.as_str() }).to_string().into()));
        let status = match Fetch::Request(Request::new_with_init(&url, &init)?).send().await {
            Ok(r) => r.status_code(),
            Err(e) => {
                console_log!("graft-controller: task {} PATCH {}: {e}", s.b.task.id.text(), state.as_str());
                return Ok(());
            }
        };
        console_log!("graft-controller: task {} PATCH {}: {status}", s.b.task.id.text(), state.as_str());
        let Some(mut s) = self.load().await? else { return Ok(()) };
        s.b.answered(state, status);
        self.save(&s).await
    }

    /// One installation token of `app` (`AGENT` or `WORKSPACE`) for `inst.repo`, claimed for the task.
    async fn mint(&self, app: &str, inst: &Inst, task: &SessionId, route: &Route) -> Result<(String, u64)> {
        let jwt = app_jwt(&self.env, app).await?;
        let host = host(route).ok_or_else(|| err("the route names no host"))?;
        let h = Headers::new();
        h.set("authorization", &format!("Bearer {jwt}"))?;
        h.set("accept", "application/vnd.github+json")?;
        h.set("content-type", "application/json")?;
        h.set("user-agent", "graft-controller")?;
        let body = json!({ "repositories": [inst.repo], "graftyard_task": task.text() });
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_headers(h).with_body(Some(body.to_string().into()));
        let url = format!("https://{host}/api/v3/app/installations/{}/access_tokens", inst.installation);
        let mut r = Fetch::Request(Request::new_with_init(&url, &init)?).send().await?;
        let v: Value = r.json().await.unwrap_or(Value::Null);
        match (r.status_code(), v["token"].as_str()) {
            (201, Some(t)) => Ok((t.to_string(), now_ms() + 3_600_000)),
            (code, _) => Err(err(format!("{app} access_tokens: {code} {}", v["message"]))),
        }
    }
}

impl DurableObject for TaskBox {
    fn new(state: State, env: Env) -> Self {
        TaskBox { state, env, tokens: RefCell::default(), pushing: Default::default() }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if let Some(p) = req.headers().get(PROPS_HEADER)? {
            let props: Props = serde_json::from_str(&p).map_err(err)?;
            return self.outbound(req, props).await;
        }
        match (req.method(), req.path().as_str()) {
            (Method::Post, "/start") => {
                let start: Start = req.json().await?;
                if self.load().await?.is_some() {
                    return reply(409, json!({ "message": "this task has a box" }));
                }
                let mut b = tb::TaskBox::new(start.task.clone());
                let act = b.start(&version(&self.env), now_ms()).map_err(err)?;
                let s = Stored { b, start };
                self.save(&s).await?;
                self.act(&act, &s).await?;
                self.state.storage().set_alarm(HEARTBEAT).await?;
                self.report_base().await?;
                reply(201, json!({ "state": s.b.task.state }))
            }
            (Method::Get, "/cancel") => {
                let Some(mut s) = self.load().await? else { return reply(404, json!({ "message": "no box" })) };
                let act = s.b.cancel();
                self.save(&s).await?;
                self.act(&act, &s).await?;
                console_log!("graft-controller: task {} cancelled", s.b.task.id.text());
                reply(200, json!({ "state": s.b.task.state }))
            }
            (Method::Get, "/status") => {
                let running = self.container().map(|c| c.running()).unwrap_or(false);
                let b = self.load().await?.map(|s| s.b);
                let pushing = self.pushing.get();
                reply(200, json!({ "box": b, "running": running, "version": version(&self.env), "pushing": pushing }))
            }
            // LM-3's kill row, test installs only: destroy the Sandbox as a crash would, leaving the box to find it
            // stopped at its next alarm.
            #[cfg(feature = "test-faults")]
            (Method::Post, "/_kill") => {
                let c = self.container()?;
                let was = c.running();
                if was {
                    c.destroy(None).await?;
                }
                let b = self.load().await?.map(|s| s.b);
                reply(200, json!({ "killed": was, "pushing": self.pushing.get(), "at": now_ms(), "box": b }))
            }
            // LM-3's kill row, test installs only: replay a report and an outbound request as the Sandbox `sandbox`
            // would send them, through the same `outbound` path `Outbound` hands the box, so a killed Sandbox's late
            // traffic is refused, counted, and logged by the product code.
            #[cfg(feature = "test-faults")]
            (Method::Post, "/_late") => {
                let body: Value = req.json().await?;
                let sandbox = SandboxId(body["sandbox"].as_str().ok_or_else(|| err("no sandbox"))?.into());
                let mut out = Vec::new();
                for (method, url, payload) in [
                    ("POST", format!("https://{REPORT_HOST}/report"), Some(json!("Finished").to_string())),
                    ("GET", body["url"].as_str().unwrap_or("https://example.invalid/").to_string(), None),
                ] {
                    let mut init = RequestInit::new();
                    init.with_method(Method::from(method.to_string()));
                    if let Some(p) = payload {
                        init.with_body(Some(p.into()));
                    }
                    let r =
                        self.outbound(Request::new_with_init(&url, &init)?, Props { sandbox: sandbox.clone() }).await?;
                    out.push(json!({ "url": url, "status": r.status_code() }));
                }
                let b = self.load().await?.map(|s| s.b);
                reply(200, json!({ "replayed": out, "box": b }))
            }
            _ => reply(404, json!({ "message": "Not Found" })),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        if let Err(e) = self.heartbeat().await {
            console_error!("graft-controller: alarm: {e}");
            self.state.storage().set_alarm(HEARTBEAT).await?;
        }
        Response::ok("ok")
    }
}
