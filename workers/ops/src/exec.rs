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
//! - **BF-39 (`test-faults` builds only).** `OPS_KILL=<n>` kills each run after a uniform 0 to n of its view reads,
//!   deploys, and commits: that call and every later one fail unsent, as `crates/release`'s in-memory BF-39 kills its
//!   fake, so a run can die between a deploy and its `IntentStep`. `OPS_STEP_MIN_ENTRIES=<n>` passes a step that
//!   `judge_site` found `Starved` with at least n entries, for a test install whose 1% step carries too little traffic
//!   (OPEN-QUESTIONS R38).
//!
//! - **Gates and drain (M5-WP4).** A site day's step reads the rolling site's `gate` object and asks
//!   `graftyard_release::gate::judge_site`; every other intent's gate is pending until its inputs are counted. Each
//!   run reads every voter site's `site_health` object and asks `gate::verdict`. A test install's `GATE` variable
//!   (`pass`, `pending`, or `breach:<why>`) overrides the gate; unset, the gate is read.
//! - **`Rotate` (M5-WP6).** Each rotatable secret this script holds has two Secrets Store bindings, `<NAME>_0` and
//!   `<NAME>_1` (`slot_binding`), and the `Release` view names the active one (`ReleaseFold::slot`), so a switch is a
//!   committed `IntentStep` and no new script version. Slot 0 of `DEPLOY_TOKEN` and `MINT_WRAP_KEY` falls back to the
//!   unslotted binding an install before M5-WP6 has. The probes call nothing on a request path
//!   (`decisions/DECISIONS.md`, "`rotate-probe`"): `DeployToken` lists the install's scripts, `ReadToken` reads one
//!   site bucket's `location`, and `MintWrapKey`'s steps commit and retire its `VerifyingKey` rows on `TokenShard(0)`.

use std::collections::BTreeMap;

use graftyard_lookup::{Bindings, Hooks, commit_hedged, now_ms, view, within};
use graftyard_release::ops::{self, Deployment, Dwell, Exec, Gate, Ran, RawBundle, Signer, Split, Submit};
use graftyard_release::{Anchor, ReleaseFold, RotateStep, Script, SecretName, Signature, SignedEntry, VersionId, gate};
use graftyard_types::{ChainId, ReleaseId, SiteId, SiteTable, Timestamp, TokenShard};
use serde_json::{Value, json};
use worker::wasm_bindgen::JsValue;
use worker::*;

const API: &str = "https://api.cloudflare.com/client/v4";
/// How long one commit waits for the group's outcome.
const COMMIT_MS: u64 = 20_000;
/// Refreshes a run tries before it gives up on the view, each VIEW_CALL_MS at most.
const VIEW_TRIES: usize = 4;
/// How long one gate or health read of a `Registry` object waits [guess]: a run reads at most 2 gate and 6 health.
const VIEW_CALL: std::time::Duration = std::time::Duration::from_millis(2_000);

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
    /// The `DeployToken` slot `token` came from.
    token_slot: u8,
    scripts: BTreeMap<String, String>,
    executor: [u8; 32],
    drain: Option<[u8; 32]>,
    /// `OPS_KILL`'s draw for this run: calls left before the kill.
    #[cfg(feature = "test-faults")]
    kill_after: Option<usize>,
}

async fn secret(env: &Env, binding: &str) -> std::result::Result<String, String> {
    env.secret_store(binding)
        .map_err(|e| format!("{binding}: {e}"))?
        .get()
        .await
        .map_err(|e| format!("{binding}: {e}"))?
        .ok_or_else(|| format!("{binding} is empty"))
}

/// The Secrets Store binding of `secret`'s `slot` (`ReleaseFold::slot`). Slot 0 of a secret an install before M5-WP6
/// bound unslotted falls back to that binding.
pub fn slot_binding(secret: SecretName, slot: u8) -> std::result::Result<String, String> {
    let base = match secret {
        SecretName::DeployToken => "DEPLOY_TOKEN",
        SecretName::ReadToken => "READ_TOKEN",
        SecretName::MintWrapKey => "MINT_WRAP_KEY",
        s => return Err(format!("graft-ops binds no slot of {s:?}")),
    };
    Ok(format!("{base}_{slot}"))
}

/// `secret`'s value in `slot`, from its slotted binding, or the unslotted one for slot 0.
pub async fn slot_value(env: &Env, name: SecretName, slot: u8) -> std::result::Result<String, String> {
    let name = slot_binding(name, slot)?;
    match secret_opt(env, &name).await? {
        Some(v) => Ok(v),
        None if slot == 0 => secret(env, name.trim_end_matches("_0")).await,
        None => Err(format!("{name} is not bound or is empty")),
    }
}

/// A binding's value, `None` when the binding is missing or empty.
async fn secret_opt(env: &Env, binding: &str) -> std::result::Result<Option<String>, String> {
    let Ok(s) = env.secret_store(binding) else { return Ok(None) };
    s.get().await.map_err(|e| format!("{binding}: {e}"))
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
        // The deploy token's active slot, from the view this isolate holds; a cold isolate reads slot 0 until its
        // first refresh, which a run makes before any API call, and `api` reads the slot again then.
        let deploy_slot = view::with(|r| r.fold.slot(SecretName::DeployToken));
        Ok(Live {
            env,
            sites,
            anchor,
            account: var(env, "CF_ACCOUNT")?,
            token: slot_value(env, SecretName::DeployToken, deploy_slot).await?,
            token_slot: deploy_slot,
            scripts,
            executor: seed(env, "EXECUTOR_KEY").await?,
            // A drain key is needed only at a site day's drain and undrain; a missing one fails those steps alone.
            drain: seed(env, "DRAIN_KEY").await.ok(),
            #[cfg(feature = "test-faults")]
            kill_after: env.var("OPS_KILL").ok().and_then(|v| v.to_string().parse::<usize>().ok()).map(|n| {
                let k = (js_sys::Math::random() * (n + 1) as f64) as usize;
                console_log!("graft-ops OPS_KILL: this run dies after {k} calls");
                k
            }),
        })
    }

    /// Whether `OPS_KILL` has killed this run before `call`; on a product build, never.
    fn killed(&mut self, _call: &str) -> bool {
        #[cfg(feature = "test-faults")]
        match &mut self.kill_after {
            Some(0) => {
                console_log!("graft-ops OPS_KILL: killed before {_call}");
                return true;
            }
            Some(n) => *n -= 1,
            None => {}
        }
        false
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
        self.api_as(&self.token, method, path, body, json_body).await
    }

    /// The deploy token of the slot the view names now: after a `Rotate`'s switch commits, the next call uses it.
    async fn follow_deploy_slot(&mut self) -> std::result::Result<(), String> {
        let slot = view::with(|r| r.fold.slot(SecretName::DeployToken));
        if slot != self.token_slot {
            self.token = slot_value(self.env, SecretName::DeployToken, slot).await?;
            self.token_slot = slot;
        }
        Ok(())
    }

    async fn api_as(
        &self,
        token: &str,
        method: Method,
        path: &str,
        body: Option<JsValue>,
        json_body: bool,
    ) -> std::result::Result<Value, String> {
        let h = Headers::new();
        h.set("authorization", &format!("Bearer {token}")).map_err(|e| e.to_string())?;
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
        // A cron run starts on a cold isolate with no view, so its first refresh reads a registry and then a member
        // in turn, from wherever the cron runs, and can pass refresh_now's VIEW_CALL_MS: graft-m4a's first two runs
        // stopped at 1,014 and 1,008 ms. A try keeps the pages it applied and leaves the stubs warm, so it tries again.
        if self.killed("view") {
            return Err("OPS_KILL".into());
        }
        let mut err = String::new();
        for _ in 0..VIEW_TRIES {
            let before = view::status()["fails"].as_u64();
            view::refresh_now(&self.bindings(), &self.anchor).await;
            let st = view::status();
            if st["fails"].as_u64() == before {
                self.follow_deploy_slot().await?;
                return Ok(view::with(|r| r.fold.clone()));
            }
            err = st["lastError"].as_str().unwrap_or("the refresh failed").to_string();
        }
        Err(err)
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

    /// A site day's gate from the site's `gate` object (`graftyard_release::gate::judge_site`), N+1's version against
    /// N's over the step, from the current intent's last entry, or N's from the intent's start once N runs nowhere. Every other intent's gate input (5xx rates for
    /// `GATE_ERRORS`, a `ContainerDay`'s shards) is not counted yet, so it is pending and its step waits. A test install's
    /// `GATE` (`pass`, `pending`, or `breach:<why>`) overrides the reading.
    async fn gate(&mut self, fold: &ReleaseFold) -> Gate {
        match self.env.var("GATE").map(|v| v.to_string()).as_deref() {
            Ok("pass") => return Gate::Pass,
            Ok("pending") => return Gate::Pending,
            Ok(b) if b.starts_with("breach:") => return Gate::Breach(b["breach:".len()..].into()),
            _ => {}
        }
        let (Some((site, n1, n)), Some(c)) = (gate::day_versions(fold), fold.current.as_deref()) else {
            return Gate::Pending;
        };
        let (since, c_start) = (c.since.0, c.intent.issued_at.0);
        let stub = match self.bindings().registry(site, "gate") {
            Ok(s) => s,
            Err(_) => return Gate::Pending,
        };
        let read = |v: graftyard_release::VersionId, since: u64| {
            let (stub, path) = (&stub, format!("https://registry/gate?version={v}&since={since}"));
            async move {
                let mut r = within(VIEW_CALL, stub.fetch_with_str(&path)).await?.ok()?;
                (r.status_code() == 200).then_some(())?;
                r.json::<gate::Counters>().await.ok()
            }
        };
        let Some(c1) = read(n1, since).await else { return Gate::Pending };
        // N's baseline is N beside N+1 in the same window. At 100% and on the first day few or no members run N, so it
        // is N's counters from the intent's start: the 1, 10 and 50% steps, where N last ran on this site (DECISIONS
        // "`gate-relative`", R44).
        let c0 = match n {
            Some(n) => match read(n, since).await {
                Some(c) if gate::n_from_start(&c) => match read(n, c_start).await {
                    Some(c) => c,
                    None => return Gate::Pending,
                },
                Some(c) => c,
                None => return Gate::Pending,
            },
            None => gate::Counters::default(),
        };
        let g = gate::judge_site(&c1.side(), &c0.side());
        #[cfg(feature = "test-faults")]
        let g = {
            let floor = self.env.var("OPS_STEP_MIN_ENTRIES").ok().and_then(|v| v.to_string().parse::<u64>().ok());
            if matches!(g, Gate::Starved(n) if floor.is_some_and(|f| n >= f)) { Gate::Pass } else { g }
        };
        console_log!("graft-ops gate site {}: N+1 {:?} N {:?} -> {g:?}", site.0, c1.side(), c0.side());
        g
    }

    /// Each voter site's verdict (`graftyard_release::gate::verdict`) from the other voter sites' `site_health`
    /// objects, over `DRAIN_WINDOW` and `UNDRAIN_AFTER`. An observer that does not answer reports nothing, so its
    /// targets get no verdict: a drain needs both other sites.
    async fn health(&mut self, fold: &ReleaseFold) -> BTreeMap<u8, gate::Verdict> {
        let Some(g) = fold.genesis.as_ref() else { return BTreeMap::new() };
        let voters: Vec<SiteId> = g.sites.iter().filter(|s| !s.spare).map(|s| s.id).collect();
        let now = now_ms();
        let (mut fast, mut slow) = (Vec::new(), Vec::new());
        for o in &voters {
            for (window, out) in [(gate::DRAIN_WINDOW, &mut fast), (gate::UNDRAIN_AFTER, &mut slow)] {
                let path = format!("https://registry/site_health?since={}", now.saturating_sub(window));
                let Ok(stub) = self.bindings().registry(*o, "site_health") else { continue };
                let Some(Ok(mut r)) = within(VIEW_CALL, stub.fetch_with_str(&path)).await else { continue };
                let Ok(rows) = r.json::<Vec<(u8, u64, u64)>>().await else { continue };
                out.extend(rows.into_iter().map(|(t, calls, missed)| gate::SiteHealth {
                    observer: *o,
                    target: SiteId(t),
                    calls,
                    missed,
                }));
            }
        }
        let v = gate::verdict(&voters, &fast, &slow);
        if v.values().any(|x| x.burning) {
            console_log!("graft-ops health: {v:?} from {fast:?}");
        }
        v
    }

    async fn deploy(&mut self, s: Script, split: &Split) -> std::result::Result<(), String> {
        if self.killed("deploy") {
            return Err("OPS_KILL".into());
        }
        let body = json!({
            "strategy": "percentage",
            "versions": split.iter().map(|(v, p)| json!({ "version_id": v.to_string(), "percentage": p })).collect::<Vec<_>>(),
            "annotations": { "workers/message": "graft-ops" },
        });
        let path = format!("/workers/scripts/{}/deployments", self.name(s)?);
        self.api(Method::Post, &path, Some(body.to_string().into()), true).await.map(|_| ())
    }

    async fn submit(&mut self, bytes: &[u8]) -> Submit {
        if self.killed("commit") {
            return Submit::Failed("OPS_KILL".into());
        }
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
        // A version upload keeps no runtime settings from the version before it: it falls back to the 2021-11-02
        // compatibility date, the default CPU limit, and no containers. So the upload carries the running version's
        // date, flags, limits, and container map, as wrangler's `--containers-rollout=none` carries the containers.
        let d = self.deployment(s).await?;
        let (live, _) = d.split.iter().max_by_key(|(_, p)| *p).ok_or("a deployment with no versions")?;
        let one = self.api(Method::Get, &format!("/workers/scripts/{name}/versions/{live}"), None, false).await?;
        let rt = &one["result"]["resources"]["script_runtime"];
        let mut meta = json!({
            "main_module": main,
            "keep_bindings": ["plain_text", "secret_text", "json", "durable_object_namespace", "service", "r2_bucket",
                              "queue", "secrets_store_secret", "pipelines", "version_metadata", "assets", "workflow"],
            "keep_assets": true,
            "annotations": { "workers/tag": graftyard_lookup::hex(&release.0), "workers/message": "graft-ops upload" },
        });
        for k in ["compatibility_date", "compatibility_flags", "limits", "containers"] {
            if !rt[k].is_null() {
                meta[k] = rt[k].clone();
            }
        }
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

    /// One host step of a `Rotate` (`decisions/DECISIONS.md`, "`rotate-probe`"). Every call is to the Cloudflare API or
    /// a `Tokens` leader from this cron, never on a request path, and each is safe to repeat.
    async fn rotate(
        &mut self,
        fold: &ReleaseFold,
        secret: SecretName,
        slot: u8,
        step: RotateStep,
    ) -> std::result::Result<(), String> {
        if self.killed("rotate") {
            return Err("OPS_KILL".into());
        }
        let new = slot_value(self.env, secret, slot).await?;
        match (secret, step) {
            // The deploy token's own scope: the call `graft-ops` makes before every deploy.
            (SecretName::DeployToken, RotateStep::Probe) => {
                self.api_as(&new, Method::Get, "/workers/scripts", None, false).await.map(|_| ())
            }
            // Workers R2 Storage Read, the read token's scope, on the first site's bucket.
            (SecretName::ReadToken, RotateStep::Probe) => {
                let site = fold.genesis.as_ref().and_then(|g| g.sites.first()).ok_or("no site in the view")?;
                let v = self.api_as(&new, Method::Get, &format!("/r2/buckets/{}", site.bucket), None, false).await?;
                v["result"]["location"].as_str().map(|_| ()).ok_or_else(|| "no bucket location in the answer".into())
            }
            (SecretName::MintWrapKey, RotateStep::Publish | RotateStep::Retire) => {
                // The old key is the other slot's: at `Retire` the switch has already made `slot` the active one.
                let old = match step {
                    RotateStep::Retire => slot_value(self.env, secret, 1 - slot).await?,
                    _ => String::new(),
                };
                let switched = fold.slots.get(&secret).map_or(Timestamp(0), |(_, t)| *t);
                let cmd = crate::rotate_command(step, &new, &old, switched).map_err(|e| e.to_string())?;
                let Some(cmd) = cmd else { return Ok(()) };
                let chain = ChainId::Tokens(TokenShard(0));
                let record = graftyard_lookup::system_record(&self.bindings(), &self.anchor, chain).await?;
                let cmd = serde_json::to_value(&cmd).map_err(|e| e.to_string())?;
                let done = graftyard_lookup::commit(&self.bindings(), chain, &record, cmd).await?;
                if done.get("ok").is_some() { Ok(()) } else { Err(format!("tokens:0 refused the key row: {done}")) }
            }
            (s, st) => Err(format!("no host step {st:?} for {s:?}")),
        }
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
