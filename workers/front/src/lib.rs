//! `graft-front` (`design/DESIGN.md`, Stack, Workers; Shape, "Release group", `Pointers` and `POINTER_TTL`; "Commit
//! and read paths", journey 7): the stable Worker every request enters. It forwards each request to the edge over the
//! `EDGE` `fetch()` service binding. While its cached `Release` view pins the edge (`Pointers.edge` is `Some`, which
//! only a pre-signed `EdgePointer` undo sets), it adds the `Cloudflare-Workers-Version-Overrides` header naming the
//! pinned version; otherwise the deployment's percentage split picks the version. No path here calls the Cloudflare
//! API.
//!
//! - **The view.** One `graftyard_lookup::view` replica per isolate, read from any `Release` member over the
//!   `MEMBER_<n>` and `REGISTRY_<n>` bindings and checked against this script's own trust anchor. A request waits for
//!   it only on a cold or long-idle isolate, and never once a refresh has failed.
//! - **The version report.** Each forwarded request carries `x-graftyard-front`, and the edge answers it with the
//!   version that ran (`x-graftyard-edge-version`, from its version metadata binding). A pinned request whose edge
//!   reports another version, or none, is a mismatch: an override is ignored silently when its version is not in the
//!   current deployment. Each mismatch is logged as an alarm (`console_error`) and counted. The header is taken off
//!   the client's response unless the client asked for it with `x-graftyard-report-version`, so no client response
//!   changes.
//! - **Status.** `GET /_front/status` with the `FRONT_TOKEN` secret as `x-front-token` answers the view and the
//!   counters. Without the secret the route does not exist.

use std::cell::RefCell;

use graftyard_lookup::{Bindings, view};
use graftyard_release::view::{mismatch, version_override};
use graftyard_release::{Anchor, Script};
use graftyard_types::SiteTable;
use serde_json::json;
use worker::*;

/// The request header that asks the edge for its version.
const FRONT_HEADER: &str = "x-graftyard-front";
/// The edge's version report.
const EDGE_VERSION_HEADER: &str = "x-graftyard-edge-version";
/// A client that sends this header keeps the edge's version report in its response (test drivers).
const REPORT_HEADER: &str = "x-graftyard-report-version";
const OVERRIDE_HEADER: &str = "cloudflare-workers-version-overrides";

#[derive(Default)]
struct Counts {
    requests: u64,
    pinned: u64,
    mismatches: u64,
    first_mismatch: Option<u64>,
    last_mismatch: Option<serde_json::Value>,
}

thread_local! {
    static COUNTS: RefCell<Counts> = RefCell::new(Counts::default());
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
    let pin = view::pin(Script::Edge);
    let report = req.headers().has(REPORT_HEADER)?;
    let fwd = req.clone_mut()?;
    fwd.headers().set(FRONT_HEADER, "1")?;
    let _ = fwd.headers().delete(OVERRIDE_HEADER);
    if let Some(v) = pin {
        fwd.headers().set(OVERRIDE_HEADER, &version_override(&env.var("EDGE_SCRIPT")?.to_string(), v))?;
    }
    let res = env.service("EDGE")?.fetch_request(fwd).await?;
    let ran = res.headers().get(EDGE_VERSION_HEADER)?;
    let bad = mismatch(pin, ran.as_deref());
    COUNTS.with(|c| {
        let mut c = c.borrow_mut();
        c.requests += 1;
        c.pinned += u64::from(pin.is_some());
        if bad {
            let now = graftyard_lookup::now_ms();
            c.mismatches += 1;
            c.first_mismatch.get_or_insert(now);
            c.last_mismatch = Some(json!({ "at": now, "pin": pin.map(|v| v.to_string()), "ran": ran }));
        }
    });
    if bad {
        console_error!(
            "ALARM graft-front: edge pinned to {} answered from {}",
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
    // The edge's own headers otherwise pass through as they are; only the version report is taken off.
    let headers = res.headers().clone();
    let _ = headers.delete(EDGE_VERSION_HEADER);
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
        json!({ "requests": c.requests, "pinned": c.pinned, "mismatches": c.mismatches,
                "firstMismatch": c.first_mismatch, "lastMismatch": c.last_mismatch })
    });
    Response::from_json(&json!({
        "front": version,
        "now": graftyard_lookup::now_ms(),
        "pin": view::pin(Script::Edge).map(|v| v.to_string()),
        "view": view::status(),
        "counts": counts,
    }))
}
