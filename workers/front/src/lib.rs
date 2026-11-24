//! `graft-front` (`design/DESIGN.md`, Stack, Workers; Shape, "Release group", `Pointers` and `POINTER_TTL`; "Commit
//! and read paths", journey 7): the stable Worker every request enters. It forwards each request over a `fetch()`
//! service binding: `MCP` (`graft-mcp`) for the host `MCP_HOST` names (`mcp.<host>`), and `EDGE` for every other host
//! (`graftyard_release::view::front_route`). While its cached `Release` view pins the edge or `graft-mcp`
//! (`Pointers.edge` or `Pointers.mcp` is `Some`, which only a pre-signed `EdgePointer` or `McpPointer` undo sets), it
//! sets one `Cloudflare-Workers-Version-Overrides` header naming every pinned version
//! (`graftyard_release::view::overrides`); a `None` pointer adds nothing, so the deployment's percentage split picks the
//! version. `graft-mcp` forwards that header on its edge calls, so a pinned edge stays pinned behind it
//! (`design/AGENT-LAYER.md`, Shape, "graft-mcp", release control). No path here calls the Cloudflare API.
//!
//! - **The view.** One `graftyard_lookup::view` replica per isolate, read from any `Release` member over the
//!   `MEMBER_<n>` and `REGISTRY_<n>` bindings and checked against this script's own trust anchor. A request waits for
//!   it only on a cold or long-idle isolate, and never once a refresh has failed.
//! - **The version report.** Each forwarded request carries `x-graftyard-front`, and the script that ran answers it
//!   with its version (`x-graftyard-edge-version` from the edge, `x-graftyard-mcp-version` from `graft-mcp`, each from
//!   its version metadata binding). A request whose script is pinned and reports another version, or none, is a
//!   mismatch: an override is ignored silently when its version is not in the
//!   current deployment. Each mismatch is logged as an alarm (`console_error`) and counted. The header is taken off
//!   the client's response unless the client asked for it with `x-graftyard-report-version`, so no client response
//!   changes.
//! - **`GATE_ERRORS`'s input.** Each answer is counted by the version it reported, under the script that answered and
//!   under this front's own version (`graftyard_release::gate::count`): answers and 5xx, calls whose binding threw,
//!   and answers with no version, which alarm. At most every `FLUSH_MS`, beside a request, the isolate sends what it
//!   counted to the `errors` object of each script's home site (`gate::errors_sites`), over the `REGISTRY_<n>` bindings
//!   it reads the view with, and to the next site only when the home does not answer. `graft-ops` sums every site's
//!   object to judge an edge, front, or MCP roll (OPEN-QUESTIONS R47).
//! - **Status.** `GET /_front/status` with the `FRONT_TOKEN` secret as `x-front-token` answers the view and the
//!   counters. Without the secret the route does not exist.

use std::cell::RefCell;
use std::collections::BTreeMap;

use graftyard_lookup::{Bindings, view};
use graftyard_release::gate::{self, ScriptErrors};
use graftyard_release::view::{forward, mismatch};
use graftyard_release::{Anchor, Script};
use graftyard_types::SiteTable;
use serde_json::json;
use worker::*;

/// The request header that asks the edge for its version.
const FRONT_HEADER: &str = "x-graftyard-front";
/// The edge's and `graft-mcp`'s version reports.
const EDGE_VERSION_HEADER: &str = "x-graftyard-edge-version";
const MCP_VERSION_HEADER: &str = "x-graftyard-mcp-version";
/// A client that sends this header keeps the edge's version report in its response (test drivers).
const REPORT_HEADER: &str = "x-graftyard-report-version";
const OVERRIDE_HEADER: &str = "cloudflare-workers-version-overrides";

#[derive(Default)]
struct Counts {
    requests: u64,
    pinned: u64,
    /// Requests routed to `graft-mcp`.
    mcp: u64,
    mismatches: u64,
    first_mismatch: Option<u64>,
    last_mismatch: Option<serde_json::Value>,
}

/// How often an isolate sends its `GATE_ERRORS` counts [guess], as the site members send their gate counters
/// (`workers/site/src/gatecount.rs`, `FLUSH_MS`).
const FLUSH_MS: u64 = 30_000;
/// How long one flush to one site waits before it tries the next.
const FLUSH_CALL: std::time::Duration = std::time::Duration::from_millis(2_000);

/// What this isolate counted for `GATE_ERRORS` since its last flush, by `gate::errors_key`.
#[derive(Default)]
struct Pending {
    errors: BTreeMap<String, ScriptErrors>,
    flushed: u64,
    version: Option<Option<String>>,
}

thread_local! {
    static COUNTS: RefCell<Counts> = RefCell::new(Counts::default());
    static PENDING: RefCell<Pending> = RefCell::new(Pending::default());
}

/// This front's own version (the `VERSION` binding), read once.
fn own_version(env: &Env) -> Option<String> {
    PENDING.with(|p| {
        p.borrow_mut()
            .version
            .get_or_insert_with(|| env.get_binding::<WorkerVersionMetadata>("VERSION").ok().map(|v| v.id()))
            .clone()
    })
}

/// Counts one forwarded request, and once `FLUSH_MS` has passed since the last flush, sends what was counted beside
/// the request.
fn count(env: &Env, ctx: &Context, sites: &SiteTable, to: Script, ran: Option<&str>, status: Option<u16>) {
    let front = own_version(env);
    let now = graftyard_lookup::now_ms();
    let taken = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        gate::count(&mut p.errors, to, ran, front.as_deref(), status);
        if now < p.flushed + FLUSH_MS {
            return None;
        }
        p.flushed = now;
        Some(std::mem::take(&mut p.errors))
    });
    let Some(errors) = taken else { return };
    let (env, sites) = (env.clone(), sites.clone());
    ctx.wait_until(async move {
        let voters: Vec<_> = sites.0.iter().filter(|s| !s.spare).map(|s| s.id).collect();
        let b = Bindings { env: &env, sites: &sites };
        for (key, counts) in errors.into_iter().filter(|(_, c)| !c.is_empty()) {
            let body = json!({ "key": key, "counts": counts }).to_string();
            for site in gate::errors_sites(&key, &voters) {
                if put_errors(&b, site, &body).await {
                    break;
                }
            }
        }
    });
}

/// One flush to one site's `errors` object: whether it answered 200 within `FLUSH_CALL`.
async fn put_errors(b: &Bindings<'_>, site: graftyard_types::SiteId, body: &str) -> bool {
    let Ok(stub) = b.registry(site, "errors") else { return false };
    let mut init = RequestInit::new();
    init.with_method(Method::Put).with_body(Some(body.into()));
    let Ok(req) = Request::new_with_init("https://registry/errors", &init) else { return false };
    matches!(graftyard_lookup::within(FLUSH_CALL, stub.fetch_with_request(req)).await, Some(Ok(r)) if r.status_code() == 200)
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, ctx: Context) -> Result<Response> {
    let sites: SiteTable = serde_json::from_str(&env.var("GRAFTYARD_SITES")?.to_string())?;
    let anchor = match (env.var("GRAFTYARD_INSTALL"), env.var("GRAFTYARD_CUSTOMER_KEY")) {
        (Ok(i), Ok(k)) => Anchor::from_vars(&i.to_string(), &k.to_string()).ok(),
        _ => None,
    };
    if let Some(a) = &anchor {
        view::ensure(&Bindings { env: &env, sites: &sites }, a, Some(&ctx)).await;
    }
    if req.path() == "/_front/status" {
        return status(&env, &req);
    }
    // `graft-mcp` sits behind this front only when the deployment names its host and script (`deploy.mjs --mcp`).
    let var = |k: &str| env.var(k).ok().map(|v| v.to_string()).filter(|v| !v.is_empty());
    let mcp = var("MCP_HOST").zip(var("MCP_SCRIPT"));
    let plan = forward(
        req.url()?.host_str().unwrap_or_default(),
        &env.var("EDGE_SCRIPT")?.to_string(),
        mcp.as_ref().map(|(h, s)| (h.as_str(), s.as_str())),
        view::pin,
    );
    let (to, pin) = (plan.to, plan.pin);
    let (binding, version_header) = match to {
        Script::Mcp => ("MCP", MCP_VERSION_HEADER),
        _ => ("EDGE", EDGE_VERSION_HEADER),
    };
    let report = req.headers().has(REPORT_HEADER)?;
    let fwd = req.clone_mut()?;
    fwd.headers().set(FRONT_HEADER, "1")?;
    let _ = fwd.headers().delete(OVERRIDE_HEADER);
    if let Some(o) = &plan.overrides {
        fwd.headers().set(OVERRIDE_HEADER, o)?;
    }
    let res = match env.service(binding)?.fetch_request(fwd).await {
        Ok(r) => r,
        Err(e) => {
            count(&env, &ctx, &sites, to, None, None);
            return Err(e);
        }
    };
    let ran = res.headers().get(version_header)?;
    count(&env, &ctx, &sites, to, ran.as_deref(), Some(res.status_code()));
    let bad = mismatch(pin, ran.as_deref());
    if ran.is_none() && !bad {
        console_error!(
            "ALARM graft-front: {} answered {} with no version",
            if to == Script::Mcp { "graft-mcp" } else { "edge" },
            res.status_code()
        );
    }
    COUNTS.with(|c| {
        let mut c = c.borrow_mut();
        c.requests += 1;
        c.pinned += u64::from(pin.is_some());
        c.mcp += u64::from(to == Script::Mcp);
        if bad {
            let now = graftyard_lookup::now_ms();
            c.mismatches += 1;
            c.first_mismatch.get_or_insert(now);
            c.last_mismatch =
                Some(json!({ "at": now, "script": binding, "pin": pin.map(|v| v.to_string()), "ran": ran }));
        }
    });
    if bad {
        console_error!(
            "ALARM graft-front: {} pinned to {} answered from {}",
            if to == Script::Mcp { "graft-mcp" } else { "edge" },
            pin.map(|v| v.to_string()).unwrap_or_default(),
            ran.as_deref().unwrap_or("no version")
        );
    }
    if report {
        // A driver's evidence per request: the pin this request carried, whether it alarmed, and this isolate's view.
        let headers = res.headers().clone();
        headers.set("x-graftyard-front-pin", &pin.map(|v| v.to_string()).unwrap_or_else(|| "none".into()))?;
        headers.set("x-graftyard-front-alarm", if bad { "1" } else { "0" })?;
        headers.set("x-graftyard-front-view", &view::brief())?;
        let status = res.status_code();
        return Ok(Response::from_body(res.body().clone())?.with_status(status).with_headers(headers));
    }
    // The script's own headers otherwise pass through as they are; only the version report is taken off.
    let headers = res.headers().clone();
    let _ = headers.delete(version_header);
    let status = res.status_code();
    Ok(Response::from_body(res.body().clone())?.with_status(status).with_headers(headers))
}

fn status(env: &Env, req: &Request) -> Result<Response> {
    let token = env.secret("FRONT_TOKEN").map(|s| s.to_string()).ok();
    if token.is_none() || req.headers().get("x-front-token")? != token {
        return Response::error("Not Found", 404);
    }
    let version = env.get_binding::<WorkerVersionMetadata>("VERSION").map(|v| v.id()).ok();
    let counts = COUNTS.with(|c| {
        let c = c.borrow();
        json!({ "requests": c.requests, "pinned": c.pinned, "mcp": c.mcp, "mismatches": c.mismatches,
                "firstMismatch": c.first_mismatch, "lastMismatch": c.last_mismatch })
    });
    let pending = PENDING.with(|p| serde_json::to_value(&p.borrow().errors).unwrap_or_default());
    Response::from_json(&json!({
        "front": version,
        "now": graftyard_lookup::now_ms(),
        "pin": view::pin(Script::Edge).map(|v| v.to_string()),
        "mcpPin": view::pin(Script::Mcp).map(|v| v.to_string()),
        "view": view::status(),
        "counts": counts,
        "errorsPending": pending,
    }))
}
