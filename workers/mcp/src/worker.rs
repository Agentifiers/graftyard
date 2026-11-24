//! The Workers host for `flow::serve` (`design/AGENT-LAYER.md`, Shape, "graft-mcp"; Deployment modes). Bindings:
//!
//! - `EDGE`, the service binding to the edge, the only way `graft-mcp` reaches the install. `graft-front` sets one
//!   version override header for the edge and `graft-mcp` on the request it routes here, and every edge call forwards
//!   it, so an undo that pins the edge pins `graft-mcp`'s calls too.
//! - `VERSION`, the version metadata binding: a request from `graft-front` (`x-graftyard-front`) gets the version that
//!   ran as `x-graftyard-mcp-version`, which `graft-front` checks against `Pointers.mcp`.
//! - `MCP_CLIENT_SECRET_0` and `MCP_CLIENT_SECRET_1`, Secrets Store bindings of the client secret's two slots
//!   (`MCP_CLIENT_SECRET_A`, which `graftyard apps init` stored in the install's store, `cli/src/apps.rs`, and
//!   `MCP_CLIENT_SECRET_B`, bound once a rotation has stored it). Each request reads the slot of
//!   `SecretName::McpClientSecret` that its cached `Release` view holds (`crate::client_secret_bindings`), so a
//!   `Rotate { McpClientSecret, slot }` switch committed on the `Release` group changes the secret with no new script
//!   version (OPEN-QUESTIONS R45). Slot 0 falls back to `MCP_CLIENT_SECRET`, the one binding of a version deployed
//!   before the slots.
//! - `MCP_SEAL_KEY`, the Secrets Store binding of `MCP_SEAL_KEY_A`, and `MCP_SEAL_KEY_OLD`, optional, bound to the slot
//!   a seal-key rotation retired, which still opens what it sealed.
//! - The view: `MEMBER_<n>` and `REGISTRY_<n>` per site and the variables `GRAFTYARD_SITES`, `GRAFTYARD_INSTALL`, and
//!   `GRAFTYARD_CUSTOMER_KEY`, as `graft-front` binds them. One `graftyard_lookup::view` replica per isolate, read from
//!   any `Release` member and checked against the trust anchor; no request path calls the Cloudflare API. While those
//!   variables are set and the view holds no entries yet, the script answers 503, since a guessed slot could pick a
//!   secret the App no longer lists. Without them the client secret is slot 0.
//! - Variables `MCP_CLIENT_ID` (`Graftyard MCP`'s client id), `MCP_ORIGIN` (`https://mcp.<host>`), `HOST_ORIGIN`
//!   (`https://<host>`), and `DENY_ZONES` (comma-separated: the install's own zones, which a CIMD URL may not name).
//!
//! The CIMD cache and the per-IP `/authorize` count live in the isolate, as caches: losing them costs a fetch or lets
//! a few more calls through, and the WAF rule on `mcp.<host>` is the install-wide limit.

use std::cell::RefCell;
use std::collections::BTreeMap;

use futures_util::StreamExt as _;
use graftyard_lookup::{Bindings, view};
use graftyard_release::{Anchor, SecretName};
use graftyard_types::SiteTable;
use worker::*;

use crate::flow::{self, Req, Resp};
use crate::{AUTHORIZE_PER_MIN, CIMD_MAX_BYTES, CIMD_TIMEOUT_MS, CimdDoc, Fetched, Sealer};

const OVERRIDE_HEADER: &str = "cloudflare-workers-version-overrides";
/// The header `graft-front` sends on each request it forwards, and the version report it reads back.
const FRONT_HEADER: &str = "x-graftyard-front";
const MCP_VERSION_HEADER: &str = "x-graftyard-mcp-version";

thread_local! {
    static CIMD: RefCell<BTreeMap<String, (CimdDoc, u64)>> = const { RefCell::new(BTreeMap::new()) };
    static HITS: RefCell<BTreeMap<String, (u32, u64)>> = const { RefCell::new(BTreeMap::new()) };
}

struct Live {
    env: Env,
    origin: String,
    host: String,
    client_id: String,
    secret: String,
    sealer: Sealer,
    deny: Vec<String>,
    pin: Option<String>,
}

fn var(env: &Env, name: &str) -> Result<String> {
    Ok(env.var(name)?.to_string())
}

async fn secret(env: &Env, name: &str) -> Result<Option<String>> {
    match env.secret_store(name) {
        Ok(s) => s.get().await,
        Err(_) => Ok(None),
    }
}

impl flow::Env for Live {
    fn now(&self) -> u64 {
        Date::now().as_millis()
    }
    fn origin(&self) -> &str {
        &self.origin
    }
    fn host_origin(&self) -> &str {
        &self.host
    }
    fn client(&self) -> (&str, &str) {
        (&self.client_id, &self.secret)
    }
    fn sealer(&self) -> &Sealer {
        &self.sealer
    }
    fn deny(&self) -> &[String] {
        &self.deny
    }
    fn random(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        let crypto: web_sys::Crypto = js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).unwrap().into();
        crypto.get_random_values_with_u8_array(&mut b).unwrap();
        b
    }
    fn allow_authorize(&self, ip: &str) -> bool {
        let now = self.now();
        HITS.with(|h| {
            let mut h = h.borrow_mut();
            h.retain(|_, (_, t)| now.saturating_sub(*t) < 60_000);
            let e = h.entry(ip.to_owned()).or_insert((0, now));
            e.0 += 1;
            e.0 <= AUTHORIZE_PER_MIN
        })
    }
    fn cimd_cached(&self, url: &str) -> Option<CimdDoc> {
        let now = self.now();
        CIMD.with(|c| c.borrow().get(url).filter(|(_, until)| *until > now).map(|(d, _)| d.clone()))
    }
    fn cimd_keep(&self, url: &str, doc: &CimdDoc, secs: u64) {
        let until = self.now() + secs * 1000;
        CIMD.with(|c| {
            let mut c = c.borrow_mut();
            if c.len() >= 1_000 {
                c.clear();
            }
            c.insert(url.to_owned(), (doc.clone(), until));
        });
    }
    async fn fetch(&self, url: &url::Url) -> std::result::Result<Fetched, String> {
        let go = async {
            let headers = Headers::new();
            headers.set("accept", "application/json")?;
            let mut init = RequestInit::new();
            init.with_method(Method::Get).with_headers(headers).with_redirect(RequestRedirect::Manual);
            let req = Request::new_with_init(url.as_str(), &init)?;
            let signal = AbortSignal::from(web_sys::AbortSignal::timeout_with_u32(CIMD_TIMEOUT_MS));
            let mut res = Fetch::Request(req).send_with_signal(&signal).await?;
            let status = res.status_code();
            let cache_control = res.headers().get("cache-control")?;
            let mut body = Vec::new();
            if status == 200 {
                let mut s = res.stream()?;
                while let Some(chunk) = s.next().await {
                    body.extend(chunk?);
                    if body.len() > CIMD_MAX_BYTES {
                        break;
                    }
                }
            }
            Ok::<_, Error>(Fetched { status, cache_control, body })
        };
        go.await.map_err(|e| e.to_string())
    }
    async fn edge_answer(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Option<Vec<u8>>,
    ) -> std::result::Result<crate::tools::Answer, String> {
        let go = async {
            // The edge refuses a request with no User-Agent, as GHES does (m3fa2: every edge call answered 403).
            let h = Headers::new();
            h.set("user-agent", "graft-mcp")?;
            for (k, v) in headers {
                h.set(k, v)?;
            }
            if let Some(p) = &self.pin {
                h.set(OVERRIDE_HEADER, p)?;
            }
            let mut init = RequestInit::new();
            init.with_method(Method::from(method.to_owned())).with_headers(h);
            if let Some(b) = body {
                init.with_body(Some(js_sys::Uint8Array::from(b.as_slice()).into()));
            }
            let req = Request::new_with_init(&format!("{}{path}", self.host), &init)?;
            let mut res = self.env.service("EDGE")?.fetch_request(req).await?;
            let headers = res.headers().entries().collect();
            Ok::<_, Error>(crate::tools::Answer { status: res.status_code(), headers, body: res.bytes().await? })
        };
        go.await.map_err(|e| e.to_string())
    }
}

/// The client secret in the slot this isolate's `Release` view names (`crate::client_secret_bindings`).
async fn client_secret(env: &Env, ctx: &Context) -> Result<String> {
    let vars = (env.var("GRAFTYARD_SITES"), env.var("GRAFTYARD_INSTALL"), env.var("GRAFTYARD_CUSTOMER_KEY"));
    let (members, slot) = match vars {
        (Ok(sites), Ok(install), Ok(customer)) => {
            let sites: SiteTable = serde_json::from_str(&sites.to_string())?;
            let anchor = Anchor::from_vars(&install.to_string(), &customer.to_string())
                .map_err(|e| Error::RustError(e.to_string()))?;
            view::ensure(&Bindings { env, sites: &sites }, &anchor, Some(ctx)).await;
            (true, view::with(|r| (r.count() > 0).then(|| r.fold.slot(SecretName::McpClientSecret))))
        }
        _ => (false, None),
    };
    let names = crate::client_secret_bindings(members, slot).map_err(|e| Error::RustError(e.into()))?;
    for name in names {
        if let Some(s) = secret(env, name).await? {
            return Ok(s);
        }
    }
    Err(Error::RustError(format!("no client secret in {}", names.join(" or "))))
}

async fn live(env: Env, req: &Request, ctx: &Context) -> Result<Live> {
    let key = |s: Option<String>| s.as_deref().and_then(Sealer::key_from_hex);
    let mut keys: Vec<[u8; 32]> = key(secret(&env, "MCP_SEAL_KEY").await?).into_iter().collect();
    keys.extend(key(secret(&env, "MCP_SEAL_KEY_OLD").await?));
    let sealer = Sealer::new(keys).ok_or_else(|| Error::RustError("MCP_SEAL_KEY is not 64 hex characters".into()))?;
    let secret = client_secret(&env, ctx).await?;
    Ok(Live {
        origin: var(&env, "MCP_ORIGIN")?,
        host: var(&env, "HOST_ORIGIN")?,
        client_id: var(&env, "MCP_CLIENT_ID")?,
        deny: var(&env, "DENY_ZONES").unwrap_or_default().split(',').map(|s| s.trim().to_owned()).collect(),
        pin: req.headers().get(OVERRIDE_HEADER)?,
        secret,
        sealer,
        env,
    })
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, ctx: Context) -> Result<Response> {
    let host = match live(env, &req, &ctx).await {
        Ok(h) => h,
        // A missing or bad secret stops new sessions; nothing else is served without it (Failure modes, bad-config).
        // So does a bound `Release` view with no entries yet.
        Err(e) => return Response::error(format!("graft-mcp is not configured: {e}"), 503),
    };
    let url = req.url()?;
    let r = Req {
        method: req.method().to_string(),
        path: url.path().to_owned(),
        query: url.query().unwrap_or_default().to_owned(),
        headers: req.headers().entries().collect(),
        body: if req.method() == Method::Get { Vec::new() } else { req.bytes().await.unwrap_or_default() },
    };
    let front = r.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(FRONT_HEADER));
    let Resp { status, headers, body } = flow::serve(&host, &r).await;
    let h = Headers::new();
    for (k, v) in &headers {
        h.append(k, v)?;
    }
    // The version report `graft-front` asked for: it alarms when `Pointers.mcp` pins another version, and takes the
    // header off before the client sees it.
    if front && let Ok(v) = host.env.get_binding::<WorkerVersionMetadata>("VERSION") {
        h.set(MCP_VERSION_HEADER, &v.id())?;
    }
    Ok(Response::from_bytes(body)?.with_status(status).with_headers(h))
}
