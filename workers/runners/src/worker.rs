//! The Workers host for `control::pass` (`design/DESIGN.md`, Shape, `graft-runners`; Stack, Workers, Sandbox SDK, Cron
//! Triggers). It reaches the install only through the front, as the `graftyard-hosted-runners` App:
//!
//! - **App JWT.** RS256 under `HOSTED_RUNNERS_APP_KEY` (the Secrets Store binding of `HostedRunnersAppKey`), `iss` the
//!   App id `HOSTED_RUNNERS_APP_ID`, then one installation token per owner a pass touches, from the front's
//!   `POST /api/v3/app/installations/{id}/access_tokens`. `HOSTED_OWNERS` maps each owner's login to the App's
//!   installation on it [inference: the install serves no `GET /app/installations` yet, so the list is configured].
//! - **Runner keys.** WebCrypto makes each runner's RSA key pair; the public half goes to the install's register route
//!   and the private half only into the `--jitconfig` this Worker writes (`runsvc::jit_config`), so no runner key
//!   leaves the runner account except inside its own Sandbox.
//! - **Sandboxes.** One per runner, named `runner-<id>`, on `SANDBOX_STD3` or `SANDBOX_STD4` (the Sandbox SDK's class,
//!   one container class per instance type, `entry.mjs`), called over Durable Object RPC the way `getSandbox` does.
//!   The process runs `run.sh --jitconfig` with `keepAlive`, then signals the Sandbox's PID 1, so a runner's exit stops
//!   its container [inference: unverified on Sandboxes until the first hosted run].
//! - **Triggers.** `fetch` takes the App's `workflow_job` webhook (`X-Hub-Signature-256` under `WEBHOOK_SECRET`) and
//!   runs a pass for that owner after answering 202; `scheduled` runs one over every owner on the 5-minute cron.

use std::cell::RefCell;
use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64S, URL_SAFE_NO_PAD as B64};
use graftyard_actions::RunnerId;
use graftyard_actions::hosted::{HostedDemand, HostedLabel};
use graftyard_actions::runsvc;
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use rsa::RsaPrivateKey;
use serde_json::{Value, json};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::wasm_bindgen_futures::JsFuture;
use worker::*;

use crate::control::{self, Host, Instance, Registered};

/// Where the runner image unpacks `actions/runner` (`containers/runner/Dockerfile`).
const RUNNER_DIR: &str = "/home/runner";

struct Live {
    env: Env,
    /// The install's front, `https://<host>`.
    base: String,
    app_id: i64,
    key: RsaPrivateKey,
    owners: Vec<(String, i64)>,
    vcpu: u32,
    tokens: RefCell<BTreeMap<String, String>>,
}

fn now_s() -> u64 {
    Date::now().as_millis() / 1000
}

fn var(env: &Env, name: &str) -> std::result::Result<String, String> {
    env.var(name).map(|v| v.to_string()).map_err(|_| format!("{name} is not set"))
}

/// A PEM RSA private key, PKCS#1 (GitHub's App key form) or PKCS#8.
fn app_key(pem: &str) -> std::result::Result<RsaPrivateKey, String> {
    use rsa::pkcs1::DecodeRsaPrivateKey as _;
    use rsa::pkcs8::DecodePrivateKey as _;
    let body: String = pem.lines().map(str::trim).filter(|l| !l.starts_with("-----")).collect();
    let der = B64S.decode(body).map_err(|e| format!("HOSTED_RUNNERS_APP_KEY: {e}"))?;
    RsaPrivateKey::from_pkcs1_der(&der)
        .or_else(|_| RsaPrivateKey::from_pkcs8_der(&der))
        .map_err(|e| format!("HOSTED_RUNNERS_APP_KEY: {e}"))
}

/// Calls `obj[method](...args)` and awaits its result, a promise or not.
async fn call_js(obj: &JsValue, method: &str, args: &[JsValue]) -> std::result::Result<JsValue, String> {
    let f: Function = Reflect::get(obj, &method.into())
        .and_then(|f| f.dyn_into())
        .map_err(|_| format!("{method} is not a function"))?;
    let out = f.apply(obj, &args.iter().collect::<Array>()).map_err(|e| format!("{method}: {e:?}"))?;
    JsFuture::from(Promise::resolve(&out)).await.map_err(|e| format!("{method}: {e:?}"))
}

fn obj(pairs: &[(&str, JsValue)]) -> JsValue {
    let o = Object::new();
    for (k, v) in pairs {
        let _ = Reflect::set(&o, &(*k).into(), v);
    }
    o.into()
}

/// A fresh RSA-2048 key pair from WebCrypto: (PKCS#8 private, SPKI public), DER.
async fn keypair() -> std::result::Result<(Vec<u8>, Vec<u8>), String> {
    let crypto = Reflect::get(&js_sys::global(), &"crypto".into()).map_err(|_| "no crypto")?;
    let subtle = Reflect::get(&crypto, &"subtle".into()).map_err(|_| "no crypto.subtle")?;
    let alg = obj(&[
        ("name", "RSASSA-PKCS1-v1_5".into()),
        ("modulusLength", 2048.into()),
        ("publicExponent", Uint8Array::from(&[1u8, 0, 1][..]).into()),
        ("hash", "SHA-256".into()),
    ]);
    let usages: Array = ["sign", "verify"].iter().map(|s| JsValue::from_str(s)).collect();
    let pair = call_js(&subtle, "generateKey", &[alg, true.into(), usages.into()]).await?;
    let mut out = Vec::new();
    for (format, half) in [("pkcs8", "privateKey"), ("spki", "publicKey")] {
        let k = Reflect::get(&pair, &half.into()).map_err(|_| half)?;
        let buf = call_js(&subtle, "exportKey", &[format.into(), k]).await?;
        out.push(Uint8Array::new(&buf).to_vec());
    }
    let public = out.pop().expect("two halves");
    Ok((out.pop().expect("two halves"), public))
}

fn hex_rand() -> String {
    let crypto = Reflect::get(&js_sys::global(), &"crypto".into()).expect("crypto");
    let b = Uint8Array::new_with_length(4);
    let f: Function = Reflect::get(&crypto, &"getRandomValues".into()).expect("getRandomValues").unchecked_into();
    let _ = f.call1(&crypto, &b);
    b.to_vec().iter().map(|x| format!("{x:02x}")).collect()
}

impl Live {
    async fn load(env: Env, only: Option<&str>) -> std::result::Result<Live, String> {
        let pem = env
            .secret_store("HOSTED_RUNNERS_APP_KEY")
            .map_err(|e| e.to_string())?
            .get()
            .await
            .map_err(|e| e.to_string())?
            .ok_or("HOSTED_RUNNERS_APP_KEY is empty")?;
        let owners: BTreeMap<String, i64> =
            serde_json::from_str(&var(&env, "HOSTED_OWNERS")?).map_err(|e| format!("HOSTED_OWNERS: {e}"))?;
        Ok(Live {
            base: var(&env, "GRAFTYARD_URL")?.trim_end_matches('/').to_string(),
            app_id: var(&env, "HOSTED_RUNNERS_APP_ID")?.parse().map_err(|_| "HOSTED_RUNNERS_APP_ID")?,
            key: app_key(&pem)?,
            owners: owners.into_iter().filter(|(o, _)| only.is_none_or(|x| x.eq_ignore_ascii_case(o))).collect(),
            vcpu: var(&env, "ACCOUNT_VCPU").ok().and_then(|v| v.parse().ok()).unwrap_or(1_500),
            tokens: RefCell::default(),
            env,
        })
    }

    /// The App JWT: RS256, a 10-minute window starting a minute back for clock skew [doc: GitHub's App JWT rules].
    fn app_jwt(&self) -> String {
        use rsa::pkcs1v15::SigningKey;
        use rsa::signature::{SignatureEncoding as _, Signer as _};
        let now = now_s();
        let head = B64.encode(json!({ "alg": "RS256", "typ": "JWT" }).to_string());
        let claims = B64.encode(json!({ "iat": now - 60, "exp": now + 540, "iss": self.app_id }).to_string());
        let signed = format!("{head}.{claims}");
        let sig = SigningKey::<rsa::sha2::Sha256>::new(self.key.clone()).sign(signed.as_bytes()).to_vec();
        format!("{signed}.{}", B64.encode(sig))
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        auth: &str,
        body: Option<Value>,
    ) -> std::result::Result<(u16, Value), String> {
        let h = Headers::new();
        for (k, v) in [
            ("authorization", format!("Bearer {auth}").as_str()),
            ("accept", "application/vnd.github+json"),
            ("user-agent", "graft-runners"),
            ("content-type", "application/json"),
        ] {
            h.set(k, v).map_err(|e| e.to_string())?;
        }
        let mut init = RequestInit::new();
        init.with_method(method).with_headers(h);
        if let Some(b) = body {
            init.with_body(Some(b.to_string().into()));
        }
        let req = Request::new_with_init(&format!("{}{path}", self.base), &init).map_err(|e| e.to_string())?;
        let mut r = Fetch::Request(req).send().await.map_err(|e| e.to_string())?;
        let text = r.text().await.unwrap_or_default();
        Ok((r.status_code(), serde_json::from_str(&text).unwrap_or(Value::Null)))
    }

    /// The App's installation token on `owner`, narrowed to self-hosted-runner write; one per pass.
    async fn token(&self, owner: &str) -> std::result::Result<String, String> {
        if let Some(t) = self.tokens.borrow().get(owner) {
            return Ok(t.clone());
        }
        let id = self.owners.iter().find(|(o, _)| o == owner).ok_or("not in HOSTED_OWNERS")?.1;
        let body = json!({ "permissions": { "organization_self_hosted_runners": "write" } });
        let path = format!("/api/v3/app/installations/{id}/access_tokens");
        let (s, v) = self.call(Method::Post, &path, &self.app_jwt(), Some(body)).await?;
        let t = v["token"].as_str().filter(|_| s == 201).ok_or(format!("access_tokens: {s} {v}"))?.to_string();
        self.tokens.borrow_mut().insert(owner.to_string(), t.clone());
        Ok(t)
    }

    fn hosted(owner: &str) -> String {
        format!("/api/graftyard/v1/owners/{owner}/actions/hosted")
    }

    fn sandbox(&self, instance: Instance, runner: RunnerId) -> std::result::Result<(JsValue, String), String> {
        let binding = match instance {
            Instance::Standard3 => "SANDBOX_STD3",
            Instance::Standard4 => "SANDBOX_STD4",
        };
        let name = format!("runner-{}", runner.0.0);
        let ns = self.env.durable_object(binding).map_err(|e| e.to_string())?;
        let stub = ns.get_by_name(&name).map_err(|e| e.to_string())?;
        Ok((stub.into_rpc::<JsValue>(), name))
    }
}

impl Host for Live {
    fn owners(&self) -> Vec<String> {
        self.owners.iter().map(|(o, _)| o.clone()).collect()
    }

    fn account_vcpu(&self) -> u32 {
        self.vcpu
    }

    async fn demand(&self, owner: &str) -> std::result::Result<HostedDemand, String> {
        let t = self.token(owner).await?;
        match self.call(Method::Get, &format!("{}/demand", Live::hosted(owner)), &t, None).await? {
            (200, v) => serde_json::from_value(v).map_err(|e| e.to_string()),
            (s, v) => Err(format!("{s} {v}")),
        }
    }

    async fn register(&self, owner: &str, row: &'static HostedLabel) -> Registered {
        let go = async {
            let t = self.token(owner).await?;
            let (private, public) = keypair().await?;
            let name = format!("graftyard-hosted-{}", hex_rand());
            let body = json!({ "label": row.name(), "key": B64S.encode(public), "name": name });
            let (s, v) = self.call(Method::Post, &format!("{}/runners", Live::hosted(owner)), &t, Some(body)).await?;
            if s == 409 {
                return Ok(Registered::Full);
            }
            let at = v["client_id"].as_str().and_then(runsvc::runner_of_client).filter(|_| s == 201);
            let (Some(at), Some(broker), Some(scope)) = (at, v["server_url_v2"].as_str(), v["github_url"].as_str())
            else {
                return Err(format!("{s} {v}"));
            };
            use rsa::pkcs8::DecodePrivateKey as _;
            let key = RsaPrivateKey::from_pkcs8_der(&private).map_err(|e| e.to_string())?;
            let urls = runsvc::Urls::new(broker.trim_end_matches("_services/broker/"));
            Ok(Registered::Runner { id: at.runner, jit: runsvc::jit_config(&urls, at, &name, scope, &key) })
        };
        go.await.unwrap_or_else(Registered::Failed)
    }

    async fn delete(&self, owner: &str, runner: RunnerId) -> std::result::Result<bool, String> {
        let t = self.token(owner).await?;
        let path = format!("{}/runners/{}", Live::hosted(owner), runner.0.0);
        match self.call(Method::Delete, &path, &t, None).await? {
            (204 | 404, _) => Ok(true),
            (422, _) => Ok(false),
            (s, v) => Err(format!("{s} {v}")),
        }
    }

    async fn start(
        &self,
        _: &str,
        runner: RunnerId,
        instance: Instance,
        jit: String,
    ) -> std::result::Result<(), String> {
        let (stub, name) = self.sandbox(instance, runner)?;
        call_js(&stub, "setSandboxName", &[name.into()]).await?;
        call_js(&stub, "setKeepAlive", &[true.into()]).await?;
        let cmd = format!("{RUNNER_DIR}/run.sh --jitconfig \"$JIT\"; kill -TERM 1");
        let opts =
            obj(&[("env", obj(&[("JIT", jit.into())])), ("cwd", RUNNER_DIR.into()), ("processId", "runner".into())]);
        call_js(&stub, "startProcess", &[cmd.into(), opts]).await.map(|_| ())
    }

    async fn destroy(&self, runner: RunnerId, instance: Instance) -> std::result::Result<(), String> {
        let (stub, _) = self.sandbox(instance, runner)?;
        call_js(&stub, "destroy", &[]).await.map(|_| ())
    }
}

async fn run(env: Env, only: Option<String>) {
    let live = match Live::load(env, only.as_deref()).await {
        Ok(l) => l,
        Err(e) => return console_error!("graft-runners: {e}"),
    };
    let r = control::pass(&live).await;
    console_log!(
        "graft-runners pass: started {} destroyed {} kept_busy {} full {} vcpu_stopped {}",
        r.started,
        r.destroyed,
        r.kept_busy,
        r.full,
        r.vcpu_stopped
    );
    for e in &r.errors {
        console_error!("graft-runners: {e}");
    }
}

/// `X-Hub-Signature-256` over `body` under `secret`, compared in constant time.
fn signed(secret: &[u8], body: &[u8], header: &str) -> bool {
    let mac = graftyard_identity::sealed::hmac_sha256(secret, body);
    let want: String = mac.iter().map(|b| format!("{b:02x}")).collect();
    let got = header.strip_prefix("sha256=").unwrap_or("");
    got.len() == want.len() && got.bytes().zip(want.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, ctx: Context) -> Result<Response> {
    if req.method() != Method::Post {
        return Response::error("not found", 404);
    }
    let body = req.bytes().await?;
    let secret = env.secret("WEBHOOK_SECRET")?.to_string();
    let sig = req.headers().get("x-hub-signature-256")?.unwrap_or_default();
    if !signed(secret.as_bytes(), &body, &sig) {
        return Response::error("bad signature", 401);
    }
    if req.headers().get("x-github-event")?.as_deref() != Some("workflow_job") {
        return Ok(Response::empty()?.with_status(204));
    }
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let Some(owner) = v["repository"]["owner"]["login"].as_str().map(str::to_owned) else {
        return Response::error("no repository owner", 422);
    };
    ctx.wait_until(run(env, Some(owner)));
    Ok(Response::empty()?.with_status(202))
}

#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    run(env, None).await;
}
