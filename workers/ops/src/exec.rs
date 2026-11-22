//! The Workers host for `graftyard_release::ops::run` (M5-WP3; `design/DESIGN.md`, "Install operations", Tools): the
//! executor half of `graft-ops`, run by the Cron Trigger. It holds no state between runs.
//!
//! - **The view.** The isolate's `graftyard_lookup::view` replica, refreshed from a `Release` member before each
//!   action over the `MEMBER_<n>` and `REGISTRY_<n>` bindings and checked against this script's trust anchor
//!   (`GRAFTYARD_INSTALL`, `GRAFTYARD_CUSTOMER_KEY`).
//! - **Commits** go to the group with `graftyard_lookup::commit_hedged`, as the edge's release endpoint sends them.
//! - **Keys.** The executor key `EXECUTOR_KEY` and the drain key `DRAIN_KEY`, Secrets Store bindings of
//!   `graftyard-executor-<slot>` and `graftyard-drain-<slot>`, each an Ed25519 seed in hex (`graftyard genesis`).
//! - **Scripts.** `DEPLOY_TOKEN` (Secrets Store, the customer's deploy token), `CF_ACCOUNT`, and `OPS_SCRIPTS`, a JSON
//!   map from each script to its name, such as `{"site/0": "graft-m4a-site-a", "edge": "graft-m4a-edge"}`. The
//!   versions and deployments API is called with that token only.
//! - **Uploads.** The `RELEASES` R2 binding is the release bucket: one prefix per release, `<label>/`, holding
//!   `release.json`, `release.sig` (64 raw bytes, Ed25519), and every file `release.json` lists. A version uploads with
//!   the release id as its `workers/tag` and `keep_bindings` for every binding type, so it carries the deployed
//!   version's bindings and changes only code [inference: a release that changes a binding is a separate operation].
//! - **Pages** are `console_error!("PAGE graft-ops ...")` lines, which the install's log alert matches.
//! - **Dwell.** `STEP_DWELL_MS` and `FIRST_DAY_MS` lower the dwells on a test install only.
//!
//! The gate is M5-WP4's: until it is built, every step's gate reads `GATE` (`pass`, `pending`, or `breach:<why>`, default
//! `pending`), so no forward step passes on its own on a product install.

use std::collections::BTreeMap;

use graftyard_lookup::{Bindings, Hooks, commit_hedged, now_ms, view};
use graftyard_release::ops::{self, Deployment, Dwell, Exec, Gate, Ran, RawBundle, Signer, Split, Submit};
use graftyard_release::{Anchor, ReleaseFold, Script, Signature, SignedEntry, VersionId};
use graftyard_types::{ChainId, ReleaseId, SiteId, SiteTable, Timestamp};
use serde_json::{Value, json};
use worker::wasm_bindgen::JsValue;
use worker::*;

const API: &str = "https://api.cloudflare.com/client/v4";
/// How long one commit waits for the group's outcome.
const COMMIT_MS: u64 = 20_000;

/// The script name key `OPS_SCRIPTS` uses for each script.
fn key(s: Script) -> String {
    match s {
        Script::Site(SiteId(n)) => format!("site/{n}"),
        s => ops::folder(s).trim_end_matches('/').to_string(),
    }
}

fn var(env: &Env, name: &str) -> std::result::Result<String, String> {
    env.var(name).map(|v| v.to_string()).map_err(|_| format!("{name} is not set"))
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    graftyard_lookup::unhex(s.trim())
}

/// RFC 3339 UTC to ms since the epoch: `2026-11-22T07:43:22.337702Z`.
fn parse_utc(s: &str) -> Option<u64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let frac: u64 = s.get(20..23).and_then(|f| f.parse().ok()).unwrap_or(0);
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let days = 365 * y + y / 4 - y / 100 + y / 400 + (153 * m + 2) / 5 + d - 719_469;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok().map(|t| t * 1000 + frac)
}

pub struct Live<'a> {
    env: &'a Env,
    sites: SiteTable,
    anchor: Anchor,
    account: String,
    token: String,
    scripts: BTreeMap<String, String>,
    executor: [u8; 32],
    drain: Option<[u8; 32]>,
}

async fn secret(env: &Env, binding: &str) -> std::result::Result<String, String> {
    env.secret_store(binding)
        .map_err(|e| format!("{binding}: {e}"))?
        .get()
        .await
        .map_err(|e| format!("{binding}: {e}"))?
        .ok_or_else(|| format!("{binding} is empty"))
}

async fn seed(env: &Env, binding: &str) -> std::result::Result<[u8; 32], String> {
    let s = secret(env, binding).await?;
    unhex(&s).and_then(|b| b.try_into().ok()).ok_or_else(|| format!("{binding} is not a 32-byte seed in hex"))
}

impl<'a> Live<'a> {
    pub async fn load(env: &'a Env) -> std::result::Result<Live<'a>, String> {
        let sites: SiteTable =
            serde_json::from_str(&var(env, "GRAFTYARD_SITES")?).map_err(|e| format!("GRAFTYARD_SITES: {e}"))?;
        let anchor = Anchor::from_vars(&var(env, "GRAFTYARD_INSTALL")?, &var(env, "GRAFTYARD_CUSTOMER_KEY")?)
            .map_err(|e| format!("anchor: {e}"))?;
        let scripts = serde_json::from_str(&var(env, "OPS_SCRIPTS")?).map_err(|e| format!("OPS_SCRIPTS: {e}"))?;
        Ok(Live {
            env,
            sites,
            anchor,
            account: var(env, "CF_ACCOUNT")?,
            token: secret(env, "DEPLOY_TOKEN").await?,
            scripts,
            executor: seed(env, "EXECUTOR_KEY").await?,
            // A drain key is needed only at a site day's drain and undrain; a missing one fails those steps alone.
            drain: seed(env, "DRAIN_KEY").await.ok(),
        })
    }

    fn bindings(&self) -> Bindings<'_> {
        Bindings { env: self.env, sites: &self.sites }
    }

    fn name(&self, s: Script) -> std::result::Result<&str, String> {
        self.scripts.get(&key(s)).map(String::as_str).ok_or_else(|| format!("OPS_SCRIPTS names no {}", key(s)))
    }

    async fn api(
        &self,
        method: Method,
        path: &str,
        body: Option<JsValue>,
        json_body: bool,
    ) -> std::result::Result<Value, String> {
        let h = Headers::new();
        h.set("authorization", &format!("Bearer {}", self.token)).map_err(|e| e.to_string())?;
        if json_body {
            h.set("content-type", "application/json").map_err(|e| e.to_string())?;
        }
        let mut init = RequestInit::new();
        init.with_method(method.clone()).with_headers(h).with_body(body);
        let url = format!("{API}/accounts/{}{path}", self.account);
        let req = Request::new_with_init(&url, &init).map_err(|e| e.to_string())?;
        let mut r = Fetch::Request(req).send().await.map_err(|e| format!("{method:?} {path}: {e}"))?;
        let status = r.status_code();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if !(200..300).contains(&status) || v["success"] == json!(false) {
            let errs = v["errors"].to_string();
            return Err(format!("{method:?} {path}: HTTP {status}: {}", errs.chars().take(300).collect::<String>()));
        }
        Ok(v)
    }
}

impl Exec for Live<'_> {
    fn now(&self) -> Timestamp {
        Timestamp(now_ms())
    }

    fn scripts(&self) -> Vec<Script> {
        let mut out = Vec::new();
        for k in self.scripts.keys() {
            let s = match k.as_str() {
                "workflows" => Script::Workflows,
                "front" => Script::Front,
                "edge" => Script::Edge,
                "ops" => Script::Ops,
                "mcp" => Script::Mcp,
                _ => continue,
            };
            out.push(s);
        }
        out
    }

    async fn view(&mut self) -> std::result::Result<ReleaseFold, String> {
        let before = view::status()["fails"].as_u64();
        view::refresh_now(&self.bindings(), &self.anchor).await;
        let st = view::status();
        if st["fails"].as_u64() != before {
            return Err(st["lastError"].as_str().unwrap_or("the refresh failed").to_string());
        }
        Ok(view::with(|r| r.fold.clone()))
    }

    async fn deployment(&mut self, s: Script) -> std::result::Result<Deployment, String> {
        let v = self.api(Method::Get, &format!("/workers/scripts/{}/deployments", self.name(s)?), None, false).await?;
        let d = &v["result"]["deployments"][0];
        let mut split = Vec::new();
        for x in d["versions"].as_array().into_iter().flatten() {
            let id: VersionId =
                x["version_id"].as_str().and_then(|i| i.parse().ok()).ok_or("a deployment version id")?;
            split.push((id, x["percentage"].as_f64().unwrap_or(0.0).round() as u8));
        }
        let created = d["created_on"].as_str().and_then(parse_utc).ok_or("a deployment with no created_on")?;
        Ok(Deployment { split, created: Timestamp(created) })
    }

    async fn gate(&mut self, _fold: &ReleaseFold) -> Gate {
        match self.env.var("GATE").map(|v| v.to_string()).as_deref() {
            Ok("pass") => Gate::Pass,
            Ok(b) if b.starts_with("breach:") => Gate::Breach(b["breach:".len()..].into()),
            _ => Gate::Pending,
        }
    }

    async fn deploy(&mut self, s: Script, split: &Split) -> std::result::Result<(), String> {
        let body = json!({
            "strategy": "percentage",
            "versions": split.iter().map(|(v, p)| json!({ "version_id": v.to_string(), "percentage": p })).collect::<Vec<_>>(),
            "annotations": { "workers/message": "graft-ops" },
        });
        let path = format!("/workers/scripts/{}/deployments", self.name(s)?);
        self.api(Method::Post, &path, Some(body.to_string().into()), true).await.map(|_| ())
    }

    async fn submit(&mut self, bytes: &[u8]) -> Submit {
        let Some(record) = view::release_record() else {
            return Submit::Failed("no Release record in the view".into());
        };
        let body = json!({ "rid": graftyard_lookup::request_id().to_string(), "cmd": graftyard_lookup::hex(bytes) });
        let b = self.bindings();
        let done = match commit_hedged(
            &b,
            ChainId::Release,
            &record,
            body.to_string().as_bytes(),
            0,
            COMMIT_MS,
            Hooks::default(),
        )
        .await
        {
            Ok(d) => d,
            Err(Ok(e)) => return Submit::Failed(format!("the Release group refused the call: {e}").into()),
            Err(Err(e)) => return Submit::Failed(e.into()),
        };
        if done.get("ok").is_some() {
            return Submit::Committed;
        }
        match done.get("refused") {
            Some(r) if r["status"].as_u64() == Some(409) => Submit::Stale,
            Some(r) => Submit::Refused {
                variant: r["err"].as_str().unwrap_or("Malformed").into(),
                message: r["message"].as_str().unwrap_or("").into(),
            },
            None => Submit::Failed(format!("no outcome: {done}").into()),
        }
    }

    fn sign(&self, signer: Signer, e: &mut SignedEntry) -> std::result::Result<(), String> {
        match signer {
            Signer::Executor => e.sign(&self.executor),
            Signer::Drain => e.sign(self.drain.as_ref().ok_or("DRAIN_KEY is not bound")?),
        };
        Ok(())
    }

    fn page(&mut self, why: &str) {
        console_error!("PAGE graft-ops: {why}");
    }

    async fn bundle(&mut self, fold: &ReleaseFold) -> std::result::Result<Option<RawBundle>, String> {
        let Ok(bucket) = self.env.bucket("RELEASES") else { return Ok(None) };
        let listed = bucket.list().delimiter("/").execute().await.map_err(|e| e.to_string())?;
        for prefix in listed.delimited_prefixes() {
            let Some(rec) = bucket.get(format!("{prefix}release.json")).execute().await.map_err(|e| e.to_string())?
            else {
                continue;
            };
            let record = rec.body().ok_or("release.json has no body")?.bytes().await.map_err(|e| e.to_string())?;
            let id: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(&record).into();
            if fold.uploads.contains_key(&id) {
                continue;
            }
            let sig = bucket.get(format!("{prefix}release.sig")).execute().await.map_err(|e| e.to_string())?;
            let sig = sig.ok_or_else(|| format!("{prefix}release.sig is missing"))?;
            let sig = sig.body().ok_or("release.sig has no body")?.bytes().await.map_err(|e| e.to_string())?;
            let sig = Signature(sig.try_into().map_err(|_| format!("{prefix}release.sig is not 64 bytes"))?);
            let mut files = Vec::new();
            let mut cursor = None;
            loop {
                let mut q = bucket.list().prefix(prefix.clone());
                if let Some(c) = cursor.take() {
                    q = q.cursor(c);
                }
                let page = q.execute().await.map_err(|e| e.to_string())?;
                for o in page.objects() {
                    let name = o.key()[prefix.len()..].to_string();
                    if name == "release.json" || name == "release.sig" {
                        continue;
                    }
                    let Some(obj) = bucket.get(o.key()).execute().await.map_err(|e| e.to_string())? else { continue };
                    let bytes = obj.body().ok_or("an object with no body")?.bytes().await.map_err(|e| e.to_string())?;
                    files.push((name, bytes));
                }
                if !page.truncated() {
                    break;
                }
                cursor = page.cursor();
            }
            return Ok(Some(RawBundle { record, sig, files }));
        }
        Ok(None)
    }

    async fn tagged(
        &mut self,
        release: ReleaseId,
        scripts: &[Script],
    ) -> std::result::Result<BTreeMap<Script, VersionId>, String> {
        let tag = graftyard_lookup::hex(&release.0);
        let mut out = BTreeMap::new();
        for s in scripts {
            let name = self.name(*s)?.to_string();
            let v =
                self.api(Method::Get, &format!("/workers/scripts/{name}/versions?per_page=10"), None, false).await?;
            // The list, newest first, carries no annotations, so each of the ten newest versions is read until one
            // carries the tag: a run uploads at most one version per script before the next reads it back.
            for item in v["result"]["items"].as_array().into_iter().flatten() {
                let Some(id) = item["id"].as_str() else { continue };
                let one = self.api(Method::Get, &format!("/workers/scripts/{name}/versions/{id}"), None, false).await?;
                if one["result"]["annotations"]["workers/tag"].as_str() == Some(tag.as_str()) {
                    out.insert(*s, id.parse().map_err(|_| format!("version id {id}"))?);
                    break;
                }
            }
        }
        Ok(out)
    }

    async fn upload(
        &mut self,
        s: Script,
        release: ReleaseId,
        modules: &[(String, Vec<u8>)],
    ) -> std::result::Result<(), String> {
        let name = self.name(s)?.to_string();
        let main = modules
            .iter()
            .map(|(n, _)| n.as_str())
            .find(|n| *n == "index.js" || *n == "entry.js" || *n == "worker.js")
            .ok_or("the script's modules have no index.js")?;
        let meta = json!({
            "main_module": main,
            "keep_bindings": ["plain_text", "secret_text", "json", "durable_object_namespace", "service", "r2_bucket",
                              "queue", "secrets_store_secret", "pipelines", "version_metadata", "assets", "workflow"],
            "keep_assets": true,
            "annotations": { "workers/tag": graftyard_lookup::hex(&release.0), "workers/message": "graft-ops upload" },
        });
        let form = web_sys::FormData::new().map_err(|e| format!("{e:?}"))?;
        form.append_with_str("metadata", &meta.to_string()).map_err(|e| format!("{e:?}"))?;
        for (n, b) in modules {
            let ct = if n.ends_with(".wasm") { "application/wasm" } else { "application/javascript+module" };
            let opts = web_sys::BlobPropertyBag::new();
            opts.set_type(ct);
            let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(b.as_slice()));
            let blob =
                web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(|e| format!("{e:?}"))?;
            form.append_with_blob_and_filename(n, &blob, n).map_err(|e| format!("{e:?}"))?;
        }
        self.api(Method::Post, &format!("/workers/scripts/{name}/versions"), Some(form.into()), false).await.map(|_| ())
    }
}

/// The dwells: the design's, or a test install's `STEP_DWELL_MS` and `FIRST_DAY_MS`.
fn dwell(env: &Env) -> Dwell {
    let ms = |n: &str| env.var(n).ok().and_then(|v| v.to_string().parse().ok());
    let d = Dwell::default();
    Dwell { step: ms("STEP_DWELL_MS").unwrap_or(d.step), first_day: ms("FIRST_DAY_MS").unwrap_or(d.first_day) }
}

/// One cron run. Each action is one log line, so Workers Logs hold the run's record.
pub async fn run(env: &Env) -> Vec<Ran> {
    let mut live = match Live::load(env).await {
        Ok(l) => l,
        Err(e) => {
            console_error!("PAGE graft-ops: cannot start: {e}");
            return vec![Ran::Error(e.into())];
        }
    };
    let log = ops::run(&mut live, dwell(env)).await;
    for r in &log {
        console_log!("graft-ops {r:?}");
    }
    log
}
