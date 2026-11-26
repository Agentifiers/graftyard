//! `graftyard runners init` and `graftyard runners upgrade` (`design/DESIGN.md`, "Usage (caller's view)", Shape,
//! `graft-runners`, and "Failure modes", `actions state`): `graft-runners` in the runner account, with the token from
//! `CLOUDFLARE_RUNNER_API_TOKEN`, so the install's deploy token never reaches that account.
//!
//! Both read the bundle `wrangler deploy --dry-run --outdir <dir>` writes from `workers/runners/deploy.mjs
//! --config-only`'s config (`entry.js` and one `.wasm` module) and the runner image pinned by digest
//! (`tests/m4/wp4/image.sh`).
//!
//! - `init` writes `HostedRunnersAppKey` to the runner account's Secrets Store `graft-runners`, uploads and deploys the
//!   script with its bindings, its migrations to `v3`, and the 5-minute sweep, turns on its workers.dev route for the
//!   App's webhook, and creates `RunnerBox`'s container application on the `durable_object` policy. A rerun changes
//!   nothing that exists. On an account at `v1` or `v2` it is release M3 (`decisions/DECISIONS.md`, "`r23-m3`"): the
//!   migration deletes the Sandbox SDK classes that ran runners before `RunnerBox` (`v1` gets `RunnerBox` in the same
//!   step), then their two container applications are deleted. It refuses, and changes nothing, while either of those
//!   applications has a container that is not stopped, since that container runs a runner's job.
//! - `upgrade` deploys only a release whose `release.json` verifies under the release key and lists every bundle file's
//!   SHA-256 and the image digest. Its first run uploads the version, tagged with the release id, with the image as
//!   `RunnerBox`'s `runner` image, and sends it 10% of traffic; a run at least `CANARY` later sends it 100% [guess, the
//!   design's 10% for a day]. The 10% start their runners on the new image, and no run touches a container application.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::cf::{Cf, Module};
use crate::config::{Res, hex, read_public_key, unhex};

pub const TOKEN_VAR: &str = "CLOUDFLARE_RUNNER_API_TOKEN";
const COMPAT_DATE: &str = "2026-10-01";
const STORE: &str = "graft-runners";
const APP_KEY_SECRET: &str = "HostedRunnersAppKey";
/// SHARED's sweep, every 5 minutes [guess].
const SWEEP_CRON: &str = "*/5 * * * *";
/// `v1` added the Sandbox SDK classes, `v2` added `RunnerBox` (release M1), and `v3` deletes the Sandbox classes
/// (release M3).
const MIGRATION_TAG: &str = "v3";
/// The canary's share and how long it holds before 100% (`design/DESIGN.md`, "Failure modes", `actions state`).
const CANARY_PCT: u8 = 10;
const CANARY: u64 = 86_400;
/// The Sandbox SDK classes that ran runners before `RunnerBox`, which release M3's migration deletes along with their
/// container applications (`{script}-sandboxstd3` and `-sandboxstd4`).
const SANDBOX_CLASSES: [&str; 2] = ["SandboxStd3", "SandboxStd4"];
/// A container instance in any other state may hold a runner's job (`tests/m4/wp4/bf60.mjs` lists a stopped Sandbox as
/// `inactive`; `stopping` still runs until its process ends).
const DONE_STATES: [&str; 4] = ["inactive", "stopped", "failed", "deleted"];
/// The one runner class (`workers/runners/box.mjs`) and its binding: each start picks the instance type, and the image
/// is the version's `runner` image.
const BOX: (&str, &str) = ("RunnerBox", "RUNNER_BOX");

/// `workers/runners/deploy.mjs`'s `runners.json`.
pub struct Spec {
    script: String,
    url: String,
    app_id: i64,
    owners: Value,
    vcpu: u64,
}

impl Spec {
    pub fn read(path: &Path) -> Res<Spec> {
        let v: Value = serde_json::from_str(&fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let s = |k: &str| v[k].as_str().map(String::from).ok_or_else(|| format!("{}: no {k}", path.display()));
        Ok(Spec {
            script: s("script")?,
            url: s("graftyard_url")?,
            app_id: v["app_id"].as_i64().ok_or_else(|| format!("{}: no app_id", path.display()))?,
            owners: v["owners"].clone(),
            vcpu: v["account_vcpu"].as_u64().unwrap_or(15_000),
        })
    }

    fn app(&self, class: &str) -> String {
        format!("{}-{}", self.script, class.to_ascii_lowercase())
    }
}

pub struct Bundle {
    files: Vec<(String, &'static str, Vec<u8>)>,
}

impl Bundle {
    pub fn read(dir: &Path) -> Res<Bundle> {
        let mut files = Vec::new();
        let rd = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut names: Vec<String> = rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
        names.sort();
        for n in names {
            let ct = match n.rsplit_once('.').map(|(_, x)| x) {
                Some("wasm") => "application/wasm",
                _ if n == "entry.js" => "application/javascript+module",
                _ => continue,
            };
            let b = fs::read(dir.join(&n)).map_err(|e| format!("{n}: {e}"))?;
            files.push((n, ct, b));
        }
        if !files.iter().any(|(n, ..)| n == "entry.js") {
            return Err(format!("{}: no entry.js (wrangler deploy --dry-run --outdir writes it)", dir.display()));
        }
        Ok(Bundle { files })
    }

    fn modules(&self) -> Vec<Module<'_>> {
        self.files.iter().map(|(n, ct, b)| Module { name: n, content_type: ct, bytes: b }).collect()
    }
}

fn check_image(image: &str) -> Res<()> {
    match image.split_once("@sha256:") {
        Some((_, d)) if d.len() == 64 && unhex(d).is_some() => Ok(()),
        _ => Err(format!("--image {image:?}: name the runner image by digest (<registry>/<name>@sha256:<hex>)")),
    }
}

fn bindings(spec: &Spec, store: &str, webhook: Option<&str>) -> Vec<Value> {
    let text = |n: &str, t: String| json!({ "type": "plain_text", "name": n, "text": t });
    let mut b = vec![
        text("GRAFTYARD_URL", spec.url.clone()),
        text("HOSTED_RUNNERS_APP_ID", spec.app_id.to_string()),
        text("HOSTED_OWNERS", spec.owners.to_string()),
        text("ACCOUNT_VCPU", spec.vcpu.to_string()),
        json!({ "type": "secrets_store_secret", "name": "HOSTED_RUNNERS_APP_KEY", "store_id": store, "secret_name": APP_KEY_SECRET }),
    ];
    b.push(json!({ "type": "durable_object_namespace", "name": BOX.1, "class_name": BOX.0 }));
    if let Some(w) = webhook {
        b.push(json!({ "type": "secret_text", "name": "WEBHOOK_SECRET", "text": w }));
    }
    b
}

/// The version's metadata. `RunnerBox`'s entry carries the image map its objects start from, as the site script's
/// `GitShard` does (`cli/src/sites.rs`).
fn metadata(spec: &Spec, bindings: Vec<Value>, image: &str) -> Value {
    let containers = [json!({ "name": spec.app(BOX.0), "class_name": BOX.0, "images": { "runner": image } })];
    json!({
        "main_module": "entry.js",
        "compatibility_date": COMPAT_DATE,
        "bindings": bindings,
        "containers": containers,
        "observability": { "enabled": true },
    })
}

/// The container applications by class, created by `init` when missing. `RunnerBox`'s is keyed by its namespace and
/// takes the `durable_object` policy, which is fixed at creation, so it has no image, instance type, or
/// `max_instances`: the image comes from each version, the instance type from each start, and starts are bounded by
/// the owner's cap, `RUNNER_VCPU_STOP_PCT`, and the account's vCPU limit (`decisions/DECISIONS.md`, "`r23-runnerbox`").
fn ensure_app(cf: &Cf, spec: &Spec) -> Res<()> {
    let name = spec.app(BOX.0);
    if cf.container_apps()?.iter().any(|a| a["name"] == json!(name)) {
        println!("  container application {name}: exists");
        return Ok(());
    }
    let ns = cf
        .bindings(&spec.script)?
        .iter()
        .find(|b| b["name"] == json!(BOX.1))
        .and_then(|b| b["namespace_id"].as_str())
        .map(String::from)
        .ok_or_else(|| format!("{}: the deployed version has no {} namespace", spec.script, BOX.1))?;
    cf.create_container_app(&json!({
        "name": name,
        "scheduling_policy": "durable_object",
        "durable_objects": { "namespace_id": ns },
    }))?;
    println!("  container application {name}: created, durable_object policy");
    Ok(())
}

/// The Sandbox classes' container applications that still exist, as (name, id), after checking that none of them has
/// a container that is not stopped: such a container runs a runner's job, which deleting its class and application
/// would kill (release M3, `decisions/DECISIONS.md`, "`r23-m3`").
fn idle_sandbox_apps(cf: &Cf, spec: &Spec) -> Res<Vec<(String, String)>> {
    let mut idle = Vec::new();
    for a in cf.container_apps()? {
        let name = a["name"].as_str().unwrap_or("");
        if !SANDBOX_CLASSES.iter().any(|c| name == spec.app(c)) {
            continue;
        }
        let id = a["id"].as_str().unwrap_or("").to_string();
        let live: Vec<String> = cf
            .container_instances(&id)?
            .iter()
            .map(|i| i["status"]["state"].as_str().unwrap_or("unknown").to_string())
            .filter(|st| !DONE_STATES.contains(&st.as_str()))
            .collect();
        if !live.is_empty() {
            return Err(format!(
                "{name} has {} container(s) not stopped ({}): each may run a runner's job; run init again once they end",
                live.len(),
                live.join(", ")
            ));
        }
        idle.push((name.to_string(), id));
    }
    Ok(idle)
}

pub struct InitArgs<'a> {
    pub account: &'a str,
    pub spec: &'a Path,
    pub bundle: &'a Path,
    pub image: &'a str,
    /// The `graftyard-hosted-runners` App's private key (PEM), written to Secrets Store and nowhere else.
    pub app_key: &'a Path,
    /// The App's webhook secret, as a file.
    pub webhook_secret: &'a Path,
}

pub fn init(a: InitArgs) -> Res<()> {
    let cf = Cf::from_env_var(a.account, TOKEN_VAR)?;
    init_with(&cf, a)
}

fn init_with(cf: &Cf, a: InitArgs) -> Res<()> {
    check_image(a.image)?;
    let spec = Spec::read(a.spec)?;
    let bundle = Bundle::read(a.bundle)?;
    let read =
        |p: &Path| fs::read_to_string(p).map(|s| s.trim().to_string()).map_err(|e| format!("{}: {e}", p.display()));
    let (key, webhook) = (read(a.app_key)?, read(a.webhook_secret)?);
    if !key.contains("PRIVATE KEY-----") {
        return Err(format!("{}: not a PEM private key", a.app_key.display()));
    }
    let store = cf.store(STORE)?;
    cf.put_secret(&store, APP_KEY_SECRET, &key, "graftyard runners init: the graftyard-hosted-runners App key")?;
    println!("  {APP_KEY_SECRET}: written to Secrets Store {STORE} ({store})");
    let tag = cf.scripts()?.into_iter().find(|(s, _)| *s == spec.script).map(|(_, t)| t);
    let mut meta = metadata(&spec, bindings(&spec, &store, Some(&webhook)), a.image);
    let add_box = json!({ "new_sqlite_classes": [BOX.0] });
    let drop_sandboxes = json!({ "deleted_classes": SANDBOX_CLASSES });
    match tag.flatten().as_deref() {
        None => meta["migrations"] = json!({ "new_tag": MIGRATION_TAG, "steps": [add_box] }),
        Some("v1") => {
            meta["migrations"] =
                json!({ "old_tag": "v1", "new_tag": MIGRATION_TAG, "steps": [add_box, drop_sandboxes] })
        }
        Some("v2") => {
            meta["migrations"] = json!({ "old_tag": "v2", "new_tag": MIGRATION_TAG, "steps": [drop_sandboxes] })
        }
        Some(MIGRATION_TAG) => {}
        Some(t) => return Err(format!("{} has migration tag {t:?}, not v1, v2 or {MIGRATION_TAG:?}", spec.script)),
    }
    // Before anything changes: a Sandbox container still running refuses the whole release.
    let sandbox_apps = idle_sandbox_apps(cf, &spec)?;
    cf.put_script(&spec.script, &meta, &bundle.modules())?;
    delete_apps(cf, &sandbox_apps)?;
    cf.put_schedules(&spec.script, &[SWEEP_CRON])?;
    if !cf.workers_dev(&spec.script)? {
        cf.set_workers_dev(&spec.script, true)?;
    }
    println!(
        "  {}: deployed, sweep {SWEEP_CRON}, webhook at https://{}.{}.workers.dev",
        spec.script,
        spec.script,
        cf.subdomain()?
    );
    ensure_app(cf, &spec)
}

/// Deletes the Sandbox classes' applications, only once the migration that deletes their classes has committed.
fn delete_apps(cf: &Cf, apps: &[(String, String)]) -> Res<()> {
    for (name, id) in apps {
        cf.delete_container_app(id)?;
        println!("  container application {name}: deleted (release M3)");
    }
    Ok(())
}

pub struct UpgradeArgs<'a> {
    pub account: &'a str,
    pub spec: &'a Path,
    pub bundle: &'a Path,
    pub image: &'a str,
    /// The release's `release.json`, its Ed25519 signature (64 raw bytes or 128 hex characters), and the vendor's
    /// release public key (the genesis's `Keys.release`).
    pub record: &'a Path,
    pub sig: &'a Path,
    pub release_key: &'a Path,
}

/// The release id, after the record verified and listed every bundle file and the image.
fn verify_release(a: &UpgradeArgs, bundle: &Bundle) -> Res<String> {
    let record = fs::read(a.record).map_err(|e| format!("{}: {e}", a.record.display()))?;
    let raw = fs::read(a.sig).map_err(|e| format!("{}: {e}", a.sig.display()))?;
    let sig: [u8; 64] = match raw.len() {
        64 => raw.try_into().expect("64 bytes"),
        _ => unhex(String::from_utf8_lossy(&raw).trim())
            .and_then(|b| b.try_into().ok())
            .ok_or("the signature is not 64 bytes")?,
    };
    let key = read_public_key(a.release_key)?;
    graftyard_release::verify_record(&key, &record, &graftyard_release::Signature(sig))
        .map_err(|_| "release.json's signature does not verify under the release key".to_string())?;
    let rec: Value = serde_json::from_slice(&record).map_err(|_| "release.json is not JSON")?;
    for (name, _, b) in &bundle.files {
        let want = hex(&Sha256::digest(b));
        let listed = rec["files"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|f| f["name"] == json!(name) && f["sha256"] == json!(want));
        if !listed {
            return Err(format!("release.json lists no {name} with SHA-256 {want}"));
        }
    }
    let digest = a.image.split_once('@').map_or("", |(_, d)| d);
    if !rec["images"].as_array().into_iter().flatten().any(|i| i["digest"] == json!(digest)) {
        return Err(format!("release.json lists no image {digest}"));
    }
    Ok(hex(&Sha256::digest(&record)))
}

pub fn upgrade(a: UpgradeArgs) -> Res<()> {
    let cf = Cf::from_env_var(a.account, TOKEN_VAR)?;
    upgrade_with(&cf, a, crate::config::now().0 / 1000)
}

fn upgrade_with(cf: &Cf, a: UpgradeArgs, now_s: u64) -> Res<()> {
    check_image(a.image)?;
    let spec = Spec::read(a.spec)?;
    let bundle = Bundle::read(a.bundle)?;
    let release = verify_release(&a, &bundle)?;
    let (versions, created) = cf.deployed(&spec.script)?;
    let mut ours = None;
    for (id, pct) in &versions {
        if cf.version_tag(&spec.script, id)?.as_deref() == Some(release.as_str()) {
            ours = Some((id.clone(), *pct));
        }
    }
    let old = versions.iter().find(|(id, _)| Some(id) != ours.as_ref().map(|o| &o.0)).map(|v| v.0.clone());
    match (ours, old) {
        (Some((_, p)), _) if p >= 100.0 => {
            println!("  {}: release {release} already serves 100%", spec.script);
            Ok(())
        }
        (Some((new, _)), Some(_)) => {
            let age = now_s.saturating_sub(parse_utc(&created).ok_or(format!("deployment time {created:?}"))?);
            if age < CANARY {
                return Err(format!(
                    "release {release} is at {CANARY_PCT}% for {age} s; it goes to 100% after {CANARY} s"
                ));
            }
            cf.deploy_versions(&spec.script, &[(&new, 100)], &format!("graftyard runners upgrade {release} 100%"))?;
            println!("  {}: release {release} at 100%, starting RunnerBox on {}", spec.script, a.image);
            Ok(())
        }
        (Some(_), None) => Err("the deployment holds only this release, below 100%".into()),
        (None, None) => Err(format!("{} has no deployment: run graftyard runners init first", spec.script)),
        (None, Some(old)) => {
            // A version upload carries no migration, so release M3 (`init`) must have deleted the Sandbox classes first: a
            // version without their bindings cannot deploy beside classes that still exist.
            match cf.scripts()?.into_iter().find(|(s, _)| *s == spec.script).and_then(|(_, t)| t) {
                Some(t) if t == MIGRATION_TAG => {}
                t => {
                    return Err(format!("{} is at migration tag {t:?}: run graftyard runners init first", spec.script));
                }
            }
            let store = cf.store(STORE)?;
            let mut meta = metadata(&spec, bindings(&spec, &store, None), a.image);
            meta["keep_bindings"] = json!(["secret_text"]);
            meta["annotations"] = json!({ "workers/tag": release, "workers/message": "graftyard runners upgrade" });
            let new = cf.upload_version(&spec.script, &meta, &bundle.modules())?;
            let message = format!("graftyard runners upgrade {release} {CANARY_PCT}%");
            cf.deploy_versions(&spec.script, &[(&new, CANARY_PCT), (&old, 100 - CANARY_PCT)], &message)?;
            println!(
                "  {}: release {release} uploaded as {new}, at {CANARY_PCT}%; rerun after {CANARY} s",
                spec.script
            );
            Ok(())
        }
    }
}

/// Seconds since the epoch of an RFC 3339 UTC time such as `2026-11-15T14:00:00.123456Z`.
fn parse_utc(s: &str) -> Option<u64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let days = 365 * y + y / 4 - y / 100 + y / 400 + (153 * m + 2) / 5 + d - 719_469;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::genesis::tests::{Seen, serve};

    fn scratch() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("graftyard-runners-test-{}", hex(&crate::config::random::<6>())));
        fs::create_dir_all(d.join("bundle")).unwrap();
        fs::write(d.join("bundle/entry.js"), "export default {}").unwrap();
        fs::write(d.join("bundle/ab12-index_bg.wasm"), b"\0asm").unwrap();
        fs::write(d.join("bundle/README.md"), "not a module").unwrap();
        fs::write(
            d.join("runners.json"),
            r#"{"script":"graft-runners","graftyard_url":"https://git.example","app_id":7,"owners":{"acme":11}}"#,
        )
        .unwrap();
        fs::write(d.join("app.pem"), "-----BEGIN RSA PRIVATE KEY-----\nAAAA\n-----END RSA PRIVATE KEY-----\n").unwrap();
        fs::write(d.join("hook"), "s3cret\n").unwrap();
        d
    }

    const IMAGE: &str = "registry.cloudflare.com/f076/graft-runners-runner@sha256:1111111111111111111111111111111111111111111111111111111111111111";

    fn path(s: &Seen) -> String {
        s.line.split(' ').nth(1).unwrap_or("").split("/accounts/r").nth(1).unwrap_or("").to_string()
    }

    /// Every request the stub saw: method, path under the account, body.
    /// One request: method, path under the account, body.
    type Req = (String, String, Vec<u8>);
    type Log = Arc<Mutex<Vec<Req>>>;
    type Answer = Box<dyn FnMut(&Seen) -> (u16, String) + Send>;

    /// A runner account over the stub: `graft-runners` at migration `tag` (none: no script yet), the container
    /// applications `apps` as (name, id), and each application's instance states by id.
    fn account(
        log: Log,
        tag: Option<&'static str>,
        apps: Vec<(&'static str, &'static str, Vec<&'static str>)>,
    ) -> Answer {
        Box::new(move |s: &Seen| {
            let (m, p) = (s.line.split(' ').next().unwrap_or("").to_string(), path(s));
            log.lock().unwrap().push((m.clone(), p.clone(), s.body.clone()));
            let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
            let instances =
                apps.iter().find(|(_, id, _)| p == format!("/containers/applications/{id}/instances?per_page=1000"));
            if let Some((.., states)) = instances {
                let list: Vec<Value> = states.iter().map(|st| json!({ "status": { "state": st } })).collect();
                return ok(json!({ "instances": list }));
            }
            match (m.as_str(), p.as_str()) {
                ("GET", "/secrets_store/stores") => ok(json!([{ "id": "st1", "name": "graft-runners" }])),
                ("GET", p) if p.starts_with("/secrets_store/stores/st1/secrets") => ok(json!([])),
                ("GET", "/workers/scripts") => ok(match tag {
                    None => json!([]),
                    Some(t) => json!([{ "id": "graft-runners", "migration_tag": t }]),
                }),
                ("GET", "/workers/scripts/graft-runners/subdomain") => ok(json!({ "enabled": false })),
                ("GET", "/workers/subdomain") => ok(json!({ "subdomain": "graftyard-runners" })),
                ("GET", "/containers/applications") => {
                    ok(apps.iter().map(|(n, id, _)| json!({ "id": id, "name": n })).collect::<Vec<_>>().into())
                }
                ("POST", "/containers/image-preparations") => ok(json!({ "status": "ready" })),
                ("GET", "/workers/scripts/graft-runners/settings") => ok(json!({ "bindings": [
                    { "type": "durable_object_namespace", "name": "RUNNER_BOX", "namespace_id": "nsb" },
                ] })),
                _ => ok(json!({})),
            }
        })
    }

    fn init_args(d: &Path) -> InitArgs<'_> {
        let leak = |p: std::path::PathBuf| -> &Path { Box::leak(p.into_boxed_path()) };
        InitArgs {
            account: "r",
            spec: leak(d.join("runners.json")),
            bundle: leak(d.join("bundle")),
            image: IMAGE,
            app_key: leak(d.join("app.pem")),
            webhook_secret: leak(d.join("hook")),
        }
    }

    /// `init` over the stub: its result and every request made.
    fn run_init(
        tag: Option<&'static str>,
        apps: Vec<(&'static str, &'static str, Vec<&'static str>)>,
    ) -> (Res<()>, Vec<Req>) {
        let d = scratch();
        let log = Log::default();
        let mut answer = account(log.clone(), tag, apps);
        let (url, _rx) = serve(40, move |s| answer(s));
        let r = init_with(&Cf::with(&url, "r", "t"), init_args(&d));
        let log = log.lock().unwrap().clone();
        (r, log)
    }

    fn upload_of(log: &[Req]) -> String {
        let up = log.iter().find(|(m, p, _)| m == "PUT" && p == "/workers/scripts/graft-runners");
        up.map(|(.., b)| String::from_utf8_lossy(b).to_string()).unwrap_or_default()
    }

    const SANDBOX_APPS: [(&str, &str); 2] = [("graft-runners-sandboxstd3", "a3"), ("graft-runners-sandboxstd4", "a4")];

    /// A fresh account gets `RunnerBox` alone: its migration step, its binding, and its application on the
    /// `durable_object` policy with no image, instance type, or instance cap. No Sandbox class exists anywhere.
    #[test]
    fn init_writes_the_key_to_secrets_store_and_creates_only_the_runnerbox_application() {
        let (r, log) = run_init(None, vec![]);
        r.unwrap();
        let find = |m: &str, p: &str| log.iter().find(|(x, y, _)| x == m && y == p).map(|(.., b)| b.clone());
        let secret: Value =
            serde_json::from_slice(&find("POST", "/secrets_store/stores/st1/secrets").unwrap()).unwrap();
        assert_eq!(secret[0]["name"], json!("HostedRunnersAppKey"));
        let upload = upload_of(&log);
        assert!(upload.contains("filename=\"entry.js\"") && upload.contains("filename=\"ab12-index_bg.wasm\""));
        assert!(!upload.contains("README"));
        assert!(upload.contains(r#""migrations":{"new_tag":"v3","steps":[{"new_sqlite_classes":["RunnerBox"]}]}"#));
        assert!(upload.contains(&format!(
            r#"{{"class_name":"RunnerBox","images":{{"runner":"{IMAGE}"}},"name":"graft-runners-runnerbox"}}"#
        )));
        assert!(!upload.contains("Sandbox") && !upload.contains("SANDBOX"), "no Sandbox class or binding: {upload}");
        assert!(!upload.contains("RUNNER_BOX_ON") && !upload.contains("nodejs_compat"));
        assert!(upload.contains(r#""store_id":"st1""#));
        assert!(!upload.contains("BEGIN RSA"), "the App key goes only to Secrets Store");
        let sched: Value =
            serde_json::from_slice(&find("PUT", "/workers/scripts/graft-runners/schedules").unwrap()).unwrap();
        assert_eq!(sched, json!([{ "cron": "*/5 * * * *" }]));
        let apps: Vec<Value> = log
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/containers/applications")
            .map(|(.., b)| serde_json::from_slice(b).unwrap())
            .collect();
        assert_eq!(
            apps,
            [json!({
                "name": "graft-runners-runnerbox",
                "scheduling_policy": "durable_object",
                "durable_objects": { "namespace_id": "nsb" },
            })]
        );
    }

    /// Release M3 on the deployed account (`v2`, both Sandbox applications holding only stopped containers): one
    /// `deleted_classes` step from `v2`, and the two applications deleted only after the upload that deletes their
    /// classes. `RunnerBox`'s application exists and is left alone.
    #[test]
    fn init_on_a_v2_account_deletes_the_sandbox_classes_then_their_applications() {
        let apps = vec![
            ("graft-runners-runnerbox", "ab", vec!["stopped", "failed"]),
            (SANDBOX_APPS[0].0, SANDBOX_APPS[0].1, vec!["inactive"; 36]),
            (SANDBOX_APPS[1].0, SANDBOX_APPS[1].1, vec!["inactive"; 4]),
        ];
        let (r, log) = run_init(Some("v2"), apps);
        r.unwrap();
        assert!(upload_of(&log).contains(
            r#""migrations":{"new_tag":"v3","old_tag":"v2","steps":[{"deleted_classes":["SandboxStd3","SandboxStd4"]}]}"#
        ));
        let at = |m: &str, p: &str| log.iter().position(|(x, y, _)| x == m && y == p);
        let upload = at("PUT", "/workers/scripts/graft-runners").unwrap();
        let deleted: Vec<&str> = log.iter().filter(|(m, ..)| m == "DELETE").map(|(_, p, _)| p.as_str()).collect();
        assert_eq!(deleted, ["/containers/applications/a3", "/containers/applications/a4"]);
        assert!(at("DELETE", "/containers/applications/a3").unwrap() > upload, "applications go after their classes");
        assert!(!log.iter().any(|(m, p, _)| m == "POST" && p == "/containers/applications"), "RunnerBox's exists");
        // A rerun on the finished account changes nothing that exists.
        let (r, log) = run_init(Some("v3"), vec![("graft-runners-runnerbox", "ab", vec![])]);
        r.unwrap();
        assert!(!upload_of(&log).contains("migrations"));
        assert!(!log.iter().any(|(m, ..)| m == "DELETE"));
    }

    /// An account still at `v1` (release M1 never ran) adds `RunnerBox` and deletes the Sandbox classes in one
    /// migration.
    #[test]
    fn init_on_a_v1_account_adds_runnerbox_and_deletes_the_sandbox_classes_at_once() {
        let apps = SANDBOX_APPS.iter().map(|(n, id)| (*n, *id, vec![])).collect();
        let (r, log) = run_init(Some("v1"), apps);
        r.unwrap();
        assert!(upload_of(&log).contains(
            r#""migrations":{"new_tag":"v3","old_tag":"v1","steps":[{"new_sqlite_classes":["RunnerBox"]},{"deleted_classes":["SandboxStd3","SandboxStd4"]}]}"#
        ));
        assert_eq!(log.iter().filter(|(m, ..)| m == "DELETE").count(), 2);
        assert_eq!(log.iter().filter(|(m, p, _)| m == "POST" && p == "/containers/applications").count(), 1);
    }

    /// A Sandbox application with a container that is not stopped may run a runner's job: release M3 refuses before it
    /// writes anything, so no class, application, or secret changes.
    #[test]
    fn init_refuses_release_m3_while_a_sandbox_container_runs() {
        for live in ["running", "stopping", "provisioning"] {
            let apps = vec![
                (SANDBOX_APPS[0].0, SANDBOX_APPS[0].1, vec!["inactive", live]),
                (SANDBOX_APPS[1].0, SANDBOX_APPS[1].1, vec![]),
            ];
            let (r, log) = run_init(Some("v2"), apps);
            let err = r.unwrap_err();
            assert!(err.contains("graft-runners-sandboxstd3") && err.contains(live), "{err}");
            assert!(upload_of(&log).is_empty(), "no upload while {live}");
            assert!(!log.iter().any(|(m, ..)| m == "DELETE" || m == "PUT"), "nothing deleted while {live}");
        }
    }

    #[test]
    fn init_refuses_an_image_by_tag() {
        let d = scratch();
        let cf = Cf::with("http://127.0.0.1:9", "r", "t");
        let args = InitArgs {
            account: "r",
            spec: &d.join("runners.json"),
            bundle: &d.join("bundle"),
            image: "registry.cloudflare.com/f076/graft-runners-runner:latest",
            app_key: &d.join("app.pem"),
            webhook_secret: &d.join("hook"),
        };
        assert!(init_with(&cf, args).unwrap_err().contains("by digest"));
    }

    fn release(d: &Path, seed: &[u8; 32], files: &[(&str, &[u8])], digest: &str) {
        let files: Vec<Value> =
            files.iter().map(|(n, b)| json!({ "name": n, "sha256": hex(&Sha256::digest(b)) })).collect();
        let rec =
            json!({ "label": "r1", "files": files, "images": [{ "name": "graft-runners-runner", "digest": digest }] });
        let bytes = serde_json::to_vec(&rec).unwrap();
        fs::write(d.join("release.json"), &bytes).unwrap();
        fs::write(d.join("release.sig"), graftyard_release::sign_record(seed, &bytes).0).unwrap();
        fs::write(d.join("release.pub"), graftyard_release::public_key(seed).hex()).unwrap();
    }

    fn upgrade_args<'a>(d: &'a Path, image: &'a str) -> UpgradeArgs<'a> {
        let leak = |p: std::path::PathBuf| -> &'a Path { Box::leak(p.into_boxed_path()) };
        UpgradeArgs {
            account: "r",
            spec: leak(d.join("runners.json")),
            bundle: leak(d.join("bundle")),
            image,
            record: leak(d.join("release.json")),
            sig: leak(d.join("release.sig")),
            release_key: leak(d.join("release.pub")),
        }
    }

    const DIGEST: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";

    #[test]
    fn upgrade_refuses_a_release_that_is_unsigned_or_does_not_list_the_bundle() {
        let d = scratch();
        let cf = Cf::with("http://127.0.0.1:9", "r", "t");
        let files: [(&str, &[u8]); 2] = [("entry.js", b"export default {}"), ("ab12-index_bg.wasm", b"\0asm")];
        // Signed by another key than the one given.
        release(&d, &[3; 32], &files, DIGEST);
        fs::write(d.join("release.pub"), graftyard_release::public_key(&[4; 32]).hex()).unwrap();
        assert!(upgrade_with(&cf, upgrade_args(&d, IMAGE), 0).unwrap_err().contains("does not verify"));
        // A changed wasm module.
        release(&d, &[3; 32], &[files[0], ("ab12-index_bg.wasm", b"\0asm!")], DIGEST);
        assert!(upgrade_with(&cf, upgrade_args(&d, IMAGE), 0).unwrap_err().contains("lists no ab12-index_bg.wasm"));
        // Another image.
        release(&d, &[3; 32], &files, "sha256:2222222222222222222222222222222222222222222222222222222222222222");
        assert!(upgrade_with(&cf, upgrade_args(&d, IMAGE), 0).unwrap_err().contains("lists no image"));
    }

    /// The three upgrade runs (10%, refused before the day, 100%); every request made.
    fn canary() -> (String, Vec<Req>) {
        let d = scratch();
        release(&d, &[3; 32], &[("entry.js", b"export default {}"), ("ab12-index_bg.wasm", b"\0asm")], DIGEST);
        let rid = hex(&Sha256::digest(fs::read(d.join("release.json")).unwrap()));
        // The deployment's state across the three runs: old at 100%, then old 90% and new 10% made at T.
        let state = Arc::new(Mutex::new(0u8));
        let log = Log::default();
        let (st, lg, rid2) = (state.clone(), log.clone(), rid.clone());
        let (url, _rx) = serve(40, move |s| {
            let (m, p) = (s.line.split(' ').next().unwrap_or("").to_string(), path(s));
            lg.lock().unwrap().push((m.clone(), p.clone(), s.body.clone()));
            let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
            let deployed = match *st.lock().unwrap() {
                0 => json!([{ "version_id": "old", "percentage": 100 }]),
                _ => json!([{ "version_id": "new", "percentage": 10 }, { "version_id": "old", "percentage": 90 }]),
            };
            match (m.as_str(), p.as_str()) {
                ("GET", "/workers/scripts/graft-runners/deployments") => {
                    ok(json!({ "deployments": [{ "versions": deployed, "created_on": "2026-11-15T12:00:00.5Z" }] }))
                }
                ("GET", "/workers/scripts/graft-runners/versions/new") => {
                    ok(json!({ "annotations": { "workers/tag": rid2 } }))
                }
                ("GET", p) if p.starts_with("/workers/scripts/graft-runners/versions/") => {
                    ok(json!({ "annotations": {} }))
                }
                ("POST", "/workers/scripts/graft-runners/versions") => ok(json!({ "id": "new" })),
                ("POST", "/workers/scripts/graft-runners/deployments") => {
                    *st.lock().unwrap() += 1;
                    ok(json!({}))
                }
                ("GET", "/secrets_store/stores") => ok(json!([{ "id": "st1", "name": "graft-runners" }])),
                ("GET", "/workers/scripts") => ok(json!([{ "id": "graft-runners", "migration_tag": "v3" }])),
                ("POST", "/containers/image-preparations") => ok(json!({ "status": "ready" })),
                ("GET", "/containers/applications") => ok(json!([
                    { "id": "a3", "name": "graft-runners-sandboxstd3" }, { "id": "a4", "name": "graft-runners-sandboxstd4" }
                ])),
                _ => ok(json!({})),
            }
        });
        let cf = Cf::with(&url, "r", "t");
        let t = parse_utc("2026-11-15T12:00:00Z").unwrap();
        upgrade_with(&cf, upgrade_args(&d, IMAGE), t).unwrap();
        let err = upgrade_with(&cf, upgrade_args(&d, IMAGE), t + CANARY - 1).unwrap_err();
        assert!(err.contains("at 10%"), "{err}");
        upgrade_with(&cf, upgrade_args(&d, IMAGE), t + CANARY).unwrap();
        let log = log.lock().unwrap().clone();
        (rid, log)
    }

    /// The 10% version carries the image in `RunnerBox`'s image map, so its own starts run it; it goes to 100% only a
    /// day later, and no run touches a container application (release M3: the Sandbox applications that a 100% run
    /// used to roll are gone).
    #[test]
    fn upgrade_canaries_the_image_in_the_version_at_10_percent_and_touches_no_application() {
        let (rid, log) = canary();
        let deploys: Vec<Value> = log
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/workers/scripts/graft-runners/deployments")
            .map(|(.., b)| serde_json::from_slice::<Value>(b).unwrap()["versions"].clone())
            .collect();
        assert_eq!(
            deploys,
            vec![
                json!([{ "version_id": "new", "percentage": 10 }, { "version_id": "old", "percentage": 90 }]),
                json!([{ "version_id": "new", "percentage": 100 }]),
            ]
        );
        let upload =
            log.iter().find(|(m, p, _)| m == "POST" && p == "/workers/scripts/graft-runners/versions").unwrap();
        let upload = String::from_utf8_lossy(&upload.2);
        assert!(
            upload.contains(&format!(r#""workers/tag":"{rid}""#))
                && upload.contains(r#""keep_bindings":["secret_text"]"#)
        );
        assert!(!upload.contains("migrations"), "a version upload cannot carry a migration");
        assert!(upload.contains(&format!(
            r#"{{"class_name":"RunnerBox","images":{{"runner":"{IMAGE}"}},"name":"graft-runners-runnerbox"}}"#
        )));
        assert!(!upload.contains("Sandbox") && !upload.contains("SANDBOX") && !upload.contains("RUNNER_BOX_ON"));
        // The API refuses a version whose bindings repeat a name (code 10053, R23's M1 check on 2026-11-19).
        let spec = Spec::read(&scratch().join("runners.json")).unwrap();
        let names: Vec<String> =
            bindings(&spec, "st1", Some("w")).iter().map(|b| b["name"].as_str().unwrap().to_string()).collect();
        let unique: std::collections::BTreeSet<&String> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "binding names repeat: {names:?}");
        let at = |p: &str| log.iter().position(|(m, x, _)| m == "POST" && x == p).unwrap();
        assert!(at("/containers/image-preparations") < at("/workers/scripts/graft-runners/versions"), "prepared first");
        assert!(
            !log.iter().any(|(m, p, _)| p.starts_with("/containers/applications") && m != "GET"),
            "no application is rolled, created, or deleted"
        );
    }

    /// A version upload carries no migration, so an account whose Sandbox classes still exist (`v1`, `v2`) needs
    /// release M3's `init` first.
    #[test]
    fn upgrade_refuses_an_account_before_release_m3() {
        for tag in ["v1", "v2"] {
            let d = scratch();
            release(&d, &[3; 32], &[("entry.js", b"export default {}"), ("ab12-index_bg.wasm", b"\0asm")], DIGEST);
            let (url, _rx) = serve(5, move |s| {
                let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
                match path(s).as_str() {
                    "/workers/scripts/graft-runners/deployments" => ok(json!({ "deployments": [{
                        "versions": [{ "version_id": "old", "percentage": 100 }], "created_on": "2026-11-15T12:00:00Z"
                    }] })),
                    "/workers/scripts" => ok(json!([{ "id": "graft-runners", "migration_tag": tag }])),
                    _ => ok(json!({ "annotations": {} })),
                }
            });
            let err = upgrade_with(&Cf::with(&url, "r", "t"), upgrade_args(&d, IMAGE), 0).unwrap_err();
            assert!(err.contains("runners init first"), "{tag}: {err}");
        }
    }

    #[test]
    fn rfc3339_times_parse_to_epoch_seconds() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2026-11-15T12:00:00.123Z"), Some(1_794_744_000));
        assert_eq!(parse_utc("2024-03-01T00:00:00Z"), Some(1_709_251_200));
    }
}
