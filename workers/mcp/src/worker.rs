//! The Workers host for `flow::serve` (`design/AGENT-LAYER.md`, Shape, "graft-mcp"; Deployment modes). Bindings:
//!
//! - `EDGE`, the service binding to the edge, the only way `graft-mcp` reaches the install. `graft-front` sets the
//!   edge's version override header on the request it routes here, and every edge call forwards it, so an undo that
//!   pins the edge pins `graft-mcp`'s calls too.
//! - `MCP_CLIENT_SECRET` and `MCP_SEAL_KEY`, Secrets Store bindings of the secrets `graftyard apps init` stored in the
//!   install's store (`MCP_CLIENT_SECRET_A` and `MCP_SEAL_KEY_A`, `cli/src/apps.rs`), and `MCP_SEAL_KEY_OLD`,
//!   optional, bound to the slot a rotation retired, which still opens what it sealed.
//! - Variables `MCP_CLIENT_ID` (`Graftyard MCP`'s client id), `MCP_ORIGIN` (`https://mcp.<host>`), `HOST_ORIGIN`
//!   (`https://<host>`), and `DENY_ZONES` (comma-separated: the install's own zones, which a CIMD URL may not name).
//!
//! The CIMD cache and the per-IP `/authorize` count live in the isolate, as caches: losing them costs a fetch or lets
//! a few more calls through, and the WAF rule on `mcp.<host>` is the install-wide limit.

use std::cell::RefCell;
use std::collections::BTreeMap;

use futures_util::StreamExt as _;
use worker::*;

use crate::flow::{self, Req, Resp};
use crate::{AUTHORIZE_PER_MIN, CIMD_MAX_BYTES, CIMD_TIMEOUT_MS, CimdDoc, Fetched, Sealer};

const OVERRIDE_HEADER: &str = "cloudflare-workers-version-overrides";

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
    async fn edge(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Option<Vec<u8>>,
    ) -> std::result::Result<(u16, Vec<u8>), String> {
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
            Ok::<_, Error>((res.status_code(), res.bytes().await?))
        };
        go.await.map_err(|e| e.to_string())
    }
}

async fn live(env: Env, req: &Request) -> Result<Live> {
    let key = |s: Option<String>| s.as_deref().and_then(Sealer::key_from_hex);
    let mut keys: Vec<[u8; 32]> = key(secret(&env, "MCP_SEAL_KEY").await?).into_iter().collect();
    keys.extend(key(secret(&env, "MCP_SEAL_KEY_OLD").await?));
    let sealer = Sealer::new(keys).ok_or_else(|| Error::RustError("MCP_SEAL_KEY is not 64 hex characters".into()))?;
    let secret = secret(&env, "MCP_CLIENT_SECRET").await?.ok_or_else(|| Error::RustError("no client secret".into()))?;
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
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let host = match live(env, &req).await {
        Ok(h) => h,
        // A missing or bad secret stops new sessions; nothing else is served without it (Failure modes, bad-config).
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
    let Resp { status, headers, body } = flow::serve(&host, &r).await;
    let h = Headers::new();
    for (k, v) in &headers {
        h.append(k, v)?;
    }
    Ok(Response::from_bytes(body)?.with_status(status).with_headers(h))
}
