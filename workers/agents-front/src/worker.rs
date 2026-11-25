//! The Workers host for `Reader` and `plan` (`design/AGENT-LAYER.md`, Shape, "Controller", `forward`). Bindings:
//!
//! - `GRAFTYARD_INSTALL`, `GRAFTYARD_CUSTOMER_KEY`, and `GRAFTYARD_HOST`, script variables: the `AgentsAnchor` that
//!   `graftyard agents init` sets, the first two as the install's own scripts carry them (`Anchor::from_vars`).
//! - `AGENT_APP_ID`, a variable, and `AGENT_APP_KEY`, the Secrets Store binding of `graftyard-agent`'s PEM key, read
//!   once per isolate: each `GET release/entries` read carries an App JWT signed with it
//!   (`graftyard_identity::appauth::sign_app_jwt`).
//! - `CONTROLLER`, the `fetch()` service binding to `graft-controller`, and `CONTROLLER_SCRIPT`, the script name the
//!   override names.
//! - `CONTROLLER_DEFAULT`, a variable the agents `graft-ops` sets to N while a `ControllerRoll`'s split is deployed and
//!   removes after: the pin of an isolate whose view has not shown the roll (`plan`).
//! - `VERSION`, the version metadata binding, and `FRONT_TOKEN`, a Worker secret: `GET /_agents_front/status` with it
//!   as `x-front-token` answers this version, the view, and the counts. Without it the route does not exist.
//!
//! The view is read over HTTPS from the install's edge, never through a Durable Object binding: the agents account is
//! another account. A request waits for the view only on a cold or long-idle isolate whose last refresh did not fail,
//! and then for `VIEW_CALL_MS` at most; otherwise a due refresh runs beside it under `ctx.wait_until`, one at a time
//! per isolate. Each forwarded request carries `x-graftyard-agents-front`, `graft-controller` answers it with
//! `x-graftyard-controller-version`, and a pinned request answered from another version, or none, is a mismatch,
//! logged as an alarm and counted. The header is taken off the client's response unless the client sent
//! `x-graftyard-report-version`. While the view shows a `ControllerRoll`, each answer is counted by that version
//! (`Tally`) and sent to the install's `POST /api/graftyard/v1/release/errors` beside a request. No request path calls
//! the Cloudflare API, and nothing logs a body, a JWT, or a key.

use std::cell::RefCell;
use std::future::Future;
use std::time::Duration;

use futures_util::future::{Either, select};
use graftyard_identity::AppId;
use graftyard_identity::appauth::{app_key, sign_app_jwt};
use graftyard_release::agents::{AgentsAnchor, ENTRIES_PER_PAGE, parse_page};
use graftyard_release::view::{ABANDON_MS, Need, VIEW_CALL_MS, mismatch};
use graftyard_release::{Anchor, VersionId};
use graftyard_types::{DbId, Index, Timestamp};
use serde_json::json;
use worker::*;

use crate::{PAGES, Page, Reader, Tally, next_after, plan};

/// The header each forwarded request carries, and the version report `graft-controller` answers it with.
const FRONT_HEADER: &str = "x-graftyard-agents-front";
const VERSION_HEADER: &str = "x-graftyard-controller-version";
/// A client that sends this header keeps the version report in its response (test drivers).
const REPORT_HEADER: &str = "x-graftyard-report-version";
const OVERRIDE_HEADER: &str = "cloudflare-workers-version-overrides";
const STATUS_PATH: &str = "/_agents_front/status";
const ENTRIES_PATH: &str = "/api/graftyard/v1/release/entries";
const ERRORS_PATH: &str = "/api/graftyard/v1/release/errors";
/// How long an App JWT is reused: well inside the 9 minutes `sign_app_jwt` gives it [inference].
const JWT_REUSE_MS: u64 = 300_000;

#[derive(Default)]
struct Counts {
    requests: u64,
    pinned: u64,
    /// Calls whose `CONTROLLER` binding threw, answered 502.
    thrown: u64,
    mismatches: u64,
    first_mismatch: Option<u64>,
    last_mismatch: Option<u64>,
    /// Flushes of the controller's counts the edge answered 204, and the others with their last status.
    flushes: u64,
    flush_fails: u64,
    last_flush_error: Option<String>,
}

/// The `graftyard-agent` key, read from the Secrets Store once per isolate, and the last JWT signed with it.
#[derive(Default)]
struct Key {
    key: Option<rsa::RsaPrivateKey>,
    jwt: Option<(u64, String)>,
}

thread_local! {
    static READER: RefCell<Reader> = RefCell::new(Reader::default());
    static KEY: RefCell<Key> = RefCell::new(Key::default());
    static COUNTS: RefCell<Counts> = RefCell::new(Counts::default());
    static TALLY: RefCell<Tally> = RefCell::new(Tally::default());
}

fn now_ms() -> u64 {
    Date::now().as_millis()
}

/// `f`, or `None` once `d` has passed.
async fn within<T>(d: Duration, f: impl Future<Output = T>) -> Option<T> {
    match select(Box::pin(f), Box::pin(Delay::from(d))).await {
        Either::Left((v, _)) => Some(v),
        Either::Right(_) => None,
    }
}

fn var(env: &Env, k: &str) -> Option<String> {
    env.var(k).ok().map(|v| v.to_string()).filter(|v| !v.is_empty())
}

/// The anchor from the script's variables, or `None` when one is missing or malformed: then no view is read and every
/// request goes out unpinned, as a cold isolate's would.
fn anchor(env: &Env) -> Option<AgentsAnchor> {
    let a = Anchor::from_vars(&var(env, "GRAFTYARD_INSTALL")?, &var(env, "GRAFTYARD_CUSTOMER_KEY")?).ok()?;
    Some(AgentsAnchor { install: a.install, customer: a.customer, host: var(env, "GRAFTYARD_HOST")?.into() })
}

/// An App JWT of `graftyard-agent`, signed with the key from the Secrets Store, reused for `JWT_REUSE_MS`.
async fn jwt(env: &Env, now: u64) -> std::result::Result<String, String> {
    if let Some(j) = KEY.with(|k| k.borrow().jwt.clone()).filter(|(at, _)| now < at + JWT_REUSE_MS) {
        return Ok(j.1);
    }
    let app = var(env, "AGENT_APP_ID").and_then(|v| v.parse::<i64>().ok()).ok_or("AGENT_APP_ID is not set")?;
    if KEY.with(|k| k.borrow().key.is_none()) {
        let store = env.secret_store("AGENT_APP_KEY").map_err(|_| "no AGENT_APP_KEY binding")?;
        let pem = store.get().await.ok().flatten().ok_or("AGENT_APP_KEY has no value")?;
        let key = app_key(&pem)?;
        KEY.with(|k| k.borrow_mut().key = Some(key));
    }
    let j = KEY.with(|k| {
        let mut k = k.borrow_mut();
        let j = sign_app_jwt(k.key.as_ref().expect("read above"), AppId(DbId(app)), Timestamp(now));
        k.jwt = Some((now, j.clone()));
        j
    });
    Ok(j)
}

/// One page `after` the given index, for `VIEW_CALL_MS` at most. Anything but a 200 whose body parses is an error.
async fn page(host: &str, jwt: &str, after: Index) -> std::result::Result<Page, String> {
    let url = format!("https://{host}{ENTRIES_PATH}?after={}&per_page={ENTRIES_PER_PAGE}", after.0);
    let headers = Headers::new();
    let set = |k, v: &str| headers.set(k, v).map_err(|e| e.to_string());
    set("authorization", &format!("Bearer {jwt}"))?;
    set("accept", "application/json")?;
    set("user-agent", "graft-agents-front")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(headers.clone());
    let req = Request::new_with_init(&url, &init).map_err(|e| e.to_string())?;
    let read = async {
        let mut res = Fetch::Request(req).send().await.map_err(|e| e.to_string())?;
        if res.status_code() != 200 {
            return Err(format!("after={}: {}", after.0, res.status_code()));
        }
        let body = res.bytes().await.map_err(|e| e.to_string())?;
        parse_page(&body).map_err(|e| format!("after={}: {e}", after.0))
    };
    within(Duration::from_millis(VIEW_CALL_MS), read)
        .await
        .unwrap_or_else(|| Err(format!("after={}: no answer within {VIEW_CALL_MS} ms", after.0)))
}

/// The refresh that started at `start`: pages from the view's index until one is shorter than `ENTRIES_PER_PAGE`, at
/// most `PAGES`, and stopping before a page that could end past `ABANDON_MS`, so `Freshness` never replaces a refresh
/// still running and there is one at a time. A refresh that runs out of time or pages keeps what it read, then counts
/// as failed, so the next one starts after `RETRY_MS`.
async fn refresh(env: Env, anchor: AgentsAnchor, start: u64) {
    let mut pages = Vec::new();
    let read: std::result::Result<(), String> = async {
        let jwt = within(Duration::from_millis(VIEW_CALL_MS), jwt(&env, start))
            .await
            .unwrap_or_else(|| Err(format!("AGENT_APP_KEY did not answer within {VIEW_CALL_MS} ms")))?;
        let mut after = READER.with(|r| r.borrow().view().index());
        for _ in 0..PAGES {
            if now_ms() + VIEW_CALL_MS > start + ABANDON_MS {
                return Err(format!("{} pages read within {ABANDON_MS} ms, more remain", pages.len()));
            }
            let p = page(&anchor.host, &jwt, after).await?;
            let next = next_after(after, &p);
            pages.push(p);
            match next {
                Some(n) => after = n,
                None => return Ok(()),
            }
        }
        Err(format!("{PAGES} pages read, more remain"))
    }
    .await;
    READER.with(|r| {
        let mut r = r.borrow_mut();
        let paged = r.paged();
        match read {
            Ok(()) => r.apply(&anchor, Ok(pages), start, now_ms()),
            Err(e) => {
                let e = r.take(&anchor, &pages).err().unwrap_or(e);
                r.apply(&anchor, Err(e), start, now_ms());
            }
        }
        if r.paged() > paged {
            console_error!("PAGE graft-agents-front: release/entries refused, {}", r.last_error().unwrap_or_default());
        }
    });
}

/// One flush of the controller's counts to the install's edge, beside a request, within `VIEW_CALL_MS`. The edge stores
/// it only when every version it names is a `Controller` version the group uploaded (`gate::accept_flush`).
async fn flush(env: Env, host: String, counts: graftyard_release::gate::ScriptErrors) {
    let sent = async {
        let jwt = jwt(&env, now_ms()).await?;
        let headers = Headers::new();
        let set = |k, v: &str| headers.set(k, v).map_err(|e| e.to_string());
        set("authorization", &format!("Bearer {jwt}"))?;
        set("content-type", "application/json")?;
        set("user-agent", "graft-agents-front")?;
        let body = json!({ "counts": counts }).to_string();
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_headers(headers.clone()).with_body(Some(body.into()));
        let req = Request::new_with_init(&format!("https://{host}{ERRORS_PATH}"), &init).map_err(|e| e.to_string())?;
        let r = Fetch::Request(req).send().await.map_err(|e| e.to_string())?;
        match r.status_code() {
            204 => Ok(()),
            s => Err(format!("{ERRORS_PATH}: {s}")),
        }
    };
    let out = within(Duration::from_millis(VIEW_CALL_MS), sent)
        .await
        .unwrap_or_else(|| Err(format!("{ERRORS_PATH}: no answer within {VIEW_CALL_MS} ms")));
    COUNTS.with(|c| {
        let mut c = c.borrow_mut();
        match out {
            Ok(()) => c.flushes += 1,
            Err(e) => {
                c.flush_fails += 1;
                c.last_flush_error = Some(e);
            }
        }
    });
}

/// Brings the view up to `Reader::need` before the request is forwarded.
async fn ensure(env: &Env, ctx: &Context, anchor: &AgentsAnchor) {
    let now = now_ms();
    let (need, running) = READER.with(|r| {
        let r = r.borrow();
        (r.need(now), r.fresh().running.is_some())
    });
    match need {
        Need::Use => return,
        Need::Refresh => {
            READER.with(|r| r.borrow_mut().start(now, false));
            ctx.wait_until(refresh(env.clone(), anchor.clone(), now));
            return;
        }
        // Another request of this isolate is waiting for a refresh: wait for the same one.
        Need::Wait if running => {}
        Need::Wait => {
            READER.with(|r| r.borrow_mut().start(now, true));
            ctx.wait_until(refresh(env.clone(), anchor.clone(), now));
        }
    }
    let waited = async {
        while READER.with(|r| matches!(r.borrow().fresh().running, Some((_, true)))) {
            Delay::from(Duration::from_millis(10)).await;
        }
    };
    let left = (now + VIEW_CALL_MS).saturating_sub(now_ms());
    if within(Duration::from_millis(left), waited).await.is_none() {
        // The wait ran out: the refresh goes on beside later requests, and this one goes out with what is held.
        READER.with(|r| r.borrow_mut().detach());
    }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, ctx: Context) -> Result<Response> {
    if req.method() == Method::Get && req.path() == STATUS_PATH {
        return status(&env, &req);
    }
    if let Some(a) = anchor(&env) {
        ensure(&env, &ctx, &a).await;
    }
    let script = var(&env, "CONTROLLER_SCRIPT").unwrap_or_default();
    let default = var(&env, "CONTROLLER_DEFAULT").and_then(|v| v.parse::<VersionId>().ok());
    let (pin, overrides) = READER.with(|r| plan(r.borrow().view(), default, &script));
    let report = req.headers().has(REPORT_HEADER)?;
    let fwd = req.clone_mut()?;
    let _ = fwd.headers().delete(OVERRIDE_HEADER);
    fwd.headers().set(FRONT_HEADER, "1")?;
    if let Some(o) = &overrides {
        fwd.headers().set(OVERRIDE_HEADER, o)?;
    }
    let answered = env.service("CONTROLLER")?.fetch_request(fwd).await;
    COUNTS.with(|c| {
        let mut c = c.borrow_mut();
        c.requests += 1;
        c.pinned += u64::from(pin.is_some());
        c.thrown += u64::from(answered.is_err());
    });
    let ran = match &answered {
        Ok(r) => r.headers().get(VERSION_HEADER)?,
        Err(_) => None,
    };
    let status = answered.as_ref().ok().map(|r| r.status_code());
    let due =
        READER.with(|r| TALLY.with(|t| t.borrow_mut().record(r.borrow().view(), ran.as_deref(), status, now_ms())));
    if let (Some(counts), Some(a)) = (due, anchor(&env)) {
        ctx.wait_until(flush(env.clone(), a.host.to_string(), counts));
    }
    let res = match answered {
        Ok(r) => r,
        Err(_) => {
            console_error!("graft-agents-front: the CONTROLLER binding threw");
            return Ok(Response::from_json(&json!({ "message": "graft-controller did not answer" }))?.with_status(502));
        }
    };
    let bad = mismatch(pin, ran.as_deref());
    if bad {
        let now = now_ms();
        COUNTS.with(|c| {
            let mut c = c.borrow_mut();
            c.mismatches += 1;
            c.first_mismatch.get_or_insert(now);
            c.last_mismatch = Some(now);
        });
        console_error!(
            "ALARM graft-agents-front: graft-controller pinned to {} answered from {}",
            pin.map(|v| v.to_string()).unwrap_or_default(),
            ran.as_deref().unwrap_or("no version")
        );
    }
    let headers = res.headers().clone();
    if report {
        // A driver's evidence per request: the pin this request carried and whether it alarmed.
        headers
            .set("x-graftyard-agents-front-pin", &pin.map_or_else(|| "none".into(), |v: VersionId| v.to_string()))?;
        headers.set("x-graftyard-agents-front-alarm", if bad { "1" } else { "0" })?;
    } else {
        // The controller's own headers pass through as they are; only the version report is taken off.
        let _ = headers.delete(VERSION_HEADER);
    }
    let code = res.status_code();
    Ok(Response::from_body(res.body().clone())?.with_status(code).with_headers(headers))
}

fn status(env: &Env, req: &Request) -> Result<Response> {
    let token = env.secret("FRONT_TOKEN").map(|s| s.to_string()).ok();
    if token.is_none() || req.headers().get("x-front-token")? != token {
        return Response::error("Not Found", 404);
    }
    let version = env.get_binding::<WorkerVersionMetadata>("VERSION").map(|v| v.id()).ok();
    let now = now_ms();
    let view = READER.with(|r| {
        let r = r.borrow();
        let f = r.fresh();
        json!({
            "index": r.view().index().0,
            "controller": r.view().controller().map(|v| v.to_string()),
            "fetchedAgoMs": f.fetched.map(|t| now.saturating_sub(t)),
            "failing": f.failing,
            "oks": r.oks(),
            "fails": r.fails(),
            "paged": r.paged(),
            "lastError": r.last_error(),
        })
    });
    let counts = COUNTS.with(|c| {
        let c = c.borrow();
        json!({ "requests": c.requests, "pinned": c.pinned, "thrown": c.thrown, "mismatches": c.mismatches,
                "firstMismatch": c.first_mismatch, "lastMismatch": c.last_mismatch, "flushes": c.flushes,
                "flushFails": c.flush_fails, "lastFlushError": c.last_flush_error })
    });
    let default = var(env, "CONTROLLER_DEFAULT");
    Response::from_json(
        &json!({ "agentsFront": version, "now": now, "default": default, "view": view, "counts": counts }),
    )
}
