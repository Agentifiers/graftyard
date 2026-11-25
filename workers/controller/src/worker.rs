//! The Workers host for `intake` (`design/AGENT-LAYER.md`, Shape, "Controller"; journey A3 step 2). Bindings:
//!
//! - `WEBHOOK_SECRET`, the Secrets Store binding of the `graftyard-agent` webhook secret `graftyard agents init` stored
//!   in the agents account's store. Read once per isolate and kept there.
//! - `VERSION`, the version metadata binding: a request from `graft-agents-front` (`x-graftyard-agents-front`) gets the
//!   version that ran as `x-graftyard-controller-version` on every response, which `graft-agents-front` checks against
//!   its `ControllerPointer` pin.
//! - `STATUS_TOKEN`, a Worker secret: `GET /_controller/status` with it as `x-controller-token` answers this version,
//!   the isolate's counts, and its last `RECENT_MAX` accepted deliveries. Without it the route does not exist.
//!
//! Every `POST` is a webhook: 202 `{"accepted": event}`, 200 `{"duplicate": true}`, or the refusal's status. A
//! `graftyard_task` delivery answers as its task's `TaskBox` does (`taskbox::deliver`). No
//! request path calls the Cloudflare API, and nothing logs a body or a secret.

use std::cell::RefCell;
use std::collections::VecDeque;

use serde_json::json;
use worker::*;

use crate::{Intake, Seen, intake};

/// The header `graft-agents-front` sends on each request it forwards, and the version report it reads back.
const FRONT_HEADER: &str = "x-graftyard-agents-front";
const VERSION_HEADER: &str = "x-graftyard-controller-version";
const STATUS_PATH: &str = "/_controller/status";
/// How many accepted deliveries the status route lists.
const RECENT_MAX: usize = 20;

#[derive(Default)]
struct Counts {
    accepted: u64,
    duplicate: u64,
    refused: u64,
    /// (event, action, delivery), newest last.
    recent: VecDeque<(String, Option<String>, String)>,
}

thread_local! {
    static SECRET: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
    static SEEN: RefCell<Seen> = RefCell::new(Seen::default());
    static COUNTS: RefCell<Counts> = RefCell::new(Counts::default());
}

/// The webhook secret, from the isolate's cache or the Secrets Store. `None` when the binding is missing or empty.
async fn secret(env: &Env) -> Option<Vec<u8>> {
    if let Some(s) = SECRET.with(|s| s.borrow().clone()) {
        return Some(s);
    }
    let s = env.secret_store("WEBHOOK_SECRET").ok()?.get().await.ok()??.into_bytes();
    if s.is_empty() {
        return None;
    }
    SECRET.with(|c| *c.borrow_mut() = Some(s.clone()));
    Some(s)
}

fn reply(status: u16, body: serde_json::Value) -> Result<Response> {
    Ok(Response::from_json(&body)?.with_status(status))
}

fn status(env: &Env, req: &Request, version: Option<String>) -> Result<Response> {
    let token = env.secret("STATUS_TOKEN").map(|s| s.to_string()).ok();
    if token.is_none() || req.headers().get("x-controller-token")? != token {
        return reply(404, json!({ "message": "Not Found" }));
    }
    let body = COUNTS.with(|c| {
        let c = c.borrow();
        let recent: Vec<_> = c
            .recent
            .iter()
            .map(|(event, action, delivery)| json!({ "event": event, "action": action, "delivery": delivery }))
            .collect();
        json!({ "version": version, "accepted": c.accepted, "duplicate": c.duplicate, "refused": c.refused,
                "recent": recent })
    });
    reply(200, body)
}

async fn webhook(mut req: Request, env: &Env) -> Result<Response> {
    let Some(secret) = secret(env).await else {
        // A missing secret refuses every delivery; GitHub's automatic retries bring them back once it is bound.
        return reply(503, json!({ "message": "graft-controller is not configured" }));
    };
    let body = req.bytes().await?;
    let h = req.headers();
    let headers = |name: &str| h.get(name).ok().flatten();
    if cfg!(feature = "test-bad") && crate::verify(&secret, &body, headers("x-hub-signature-256").as_deref()) {
        return reply(500, json!({ "message": "test-bad controller" }));
    }
    let out = SEEN.with(|s| intake(&secret, &headers, &body, &mut s.borrow_mut()));
    COUNTS.with(|c| {
        let mut c = c.borrow_mut();
        match out {
            Intake::Accepted { ref event, ref action, ref delivery } => {
                c.accepted += 1;
                if c.recent.len() == RECENT_MAX {
                    c.recent.pop_front();
                }
                c.recent.push_back((event.clone(), action.clone(), delivery.clone()));
            }
            Intake::Duplicate => c.duplicate += 1,
            Intake::Refused(..) => c.refused += 1,
        }
    });
    match out {
        // The first-party delivery starts or stops the task's `TaskBox`; it reaches no one else (`intake` checked it
        // was signed with `graftyard-agent`'s secret).
        Intake::Accepted { event, .. } if event == graftyard_git::hooks::TASK_EVENT => {
            let payload: serde_json::Value = serde_json::from_slice(&body)?;
            crate::taskbox::deliver(env, &payload).await
        }
        Intake::Accepted { event, .. } => reply(202, json!({ "accepted": event })),
        Intake::Duplicate => reply(200, json!({ "duplicate": true })),
        Intake::Refused(code, why) => reply(code, json!({ "message": why })),
    }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let front = req.headers().has(FRONT_HEADER)?;
    let version = env.get_binding::<WorkerVersionMetadata>("VERSION").map(|v| v.id()).ok();
    #[cfg(feature = "test-faults")]
    if let Some(r) = box_fault(&env, &req).await? {
        return Ok(r);
    }
    let mut resp = match req.method() {
        Method::Get if req.path() == STATUS_PATH => status(&env, &req, version.clone())?,
        Method::Post => webhook(req, &env).await?,
        _ => reply(405, json!({ "message": "Method Not Allowed" }))?,
    };
    // The version report `graft-agents-front` asked for: it checks it against its `ControllerPointer` pin.
    if front && let Some(v) = &version {
        resp.headers_mut().set(VERSION_HEADER, v)?;
    }
    Ok(resp)
}

/// LM-3's kill row, test installs only: `/_controller/box/{task}/{status|_kill|_late}` with `x-controller-token` goes
/// to that task's `TaskBox` (`taskbox.rs`), so the row can read a box, destroy its Sandbox, and replay a killed
/// Sandbox's traffic. `None` for any other path or without the token.
#[cfg(feature = "test-faults")]
async fn box_fault(env: &Env, req: &Request) -> Result<Option<Response>> {
    let path = req.path();
    let Some(rest) = path.strip_prefix("/_controller/box/") else { return Ok(None) };
    let token = env.secret("STATUS_TOKEN").map(|s| s.to_string()).ok();
    if token.is_none() || req.headers().get("x-controller-token")? != token {
        return Ok(None);
    }
    let Some((task, what)) = rest.split_once('/') else { return Ok(None) };
    let stub = env.durable_object("TASKBOX")?.get_by_name(task)?;
    let mut init = RequestInit::new();
    init.with_method(req.method());
    if req.method() == Method::Post {
        init.with_body(Some(req.clone()?.text().await?.into()));
    }
    let url = match what {
        "status" => "https://taskbox/status",
        "_kill" => "https://taskbox/_kill",
        "_late" => "https://taskbox/_late",
        _ => return Ok(None),
    };
    Ok(Some(stub.fetch_with_request(Request::new_with_init(url, &init)?).await?))
}
