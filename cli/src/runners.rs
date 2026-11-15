//! `graftyard runners init` and `graftyard runners upgrade` (`design/DESIGN.md`, "Usage (caller's view)", Shape,
//! `graft-runners`, and "Failure modes", `actions state`): `graft-runners` in the runner account, with the token from
//! `CLOUDFLARE_RUNNER_API_TOKEN`, so the install's deploy token never reaches that account.
//!
//! Both read the bundle `wrangler deploy --dry-run --outdir <dir>` writes from `workers/runners/deploy.mjs
//! --config-only`'s config (`entry.js` and one `.wasm` module) and the runner image pinned by digest
//! (`tests/m4/wp4/image.sh`).
//!
//! - `init` writes `HostedRunnersAppKey` to the runner account's Secrets Store `graft-runners`, uploads and deploys the
//!   script with its bindings, its `v1` migration, and the 5-minute sweep, turns on its workers.dev route for the App's
//!   webhook, and creates the `standard-3` and `standard-4` container applications. A rerun changes nothing that exists.
//! - `upgrade` deploys only a release whose `release.json` verifies under the release key and lists every bundle file's
//!   SHA-256 and the image digest. Its first run uploads the version, tagged with the release id, and sends it 10% of
//!   traffic; a run at least `CANARY` later sends it 100% and rolls both container applications to the image [guess, the
//!   design's 10% for a day]. The 10% half runs the old image, because a container application has one image.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::cf::{Cf, Module};
use crate::config::{Res, hex, read_public_key, unhex};

pub const TOKEN_VAR: &str = "CLOUDFLARE_RUNNER_API_TOKEN";
const COMPAT_DATE: &str = "2026-10-01";
/// The Sandbox SDK's bundle imports `node:path/posix`, and its templates set this flag.
const COMPAT_FLAGS: [&str; 1] = ["nodejs_compat"];
const STORE: &str = "graft-runners";
const APP_KEY_SECRET: &str = "HostedRunnersAppKey";
/// SHARED's sweep, every 5 minutes [guess].
const SWEEP_CRON: &str = "*/5 * * * *";
const MIGRATION_TAG: &str = "v1";
/// The canary's share and how long it holds before 100% (`design/DESIGN.md`, "Failure modes", `actions state`).
const CANARY_PCT: u8 = 10;
const CANARY: u64 = 86_400;
/// One container class per instance type (`workers/runners/entry.mjs`), with the binding the Worker calls.
const CLASSES: [(&str, &str, &str); 2] =
    [("SandboxStd3", "SANDBOX_STD3", "standard-3"), ("SandboxStd4", "SANDBOX_STD4", "standard-4")];

/// `workers/runners/deploy.mjs`'s `runners.json`.
pub struct Spec {
    script: String,
    url: String,
    app_id: i64,
    owners: Value,
    vcpu: u64,
    max: [u64; 2],
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
            max: [v["max_instances"]["std3"].as_u64().unwrap_or(20), v["max_instances"]["std4"].as_u64().unwrap_or(5)],
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
    b.extend(CLASSES.iter().map(|(c, n, _)| json!({ "type": "durable_object_namespace", "name": n, "class_name": c })));
    if let Some(w) = webhook {
        b.push(json!({ "type": "secret_text", "name": "WEBHOOK_SECRET", "text": w }));
    }
    b
}

fn metadata(spec: &Spec, bindings: Vec<Value>) -> Value {
    json!({
        "main_module": "entry.js",
        "compatibility_date": COMPAT_DATE,
        "compatibility_flags": COMPAT_FLAGS,
        "bindings": bindings,
        "containers": CLASSES.iter().map(|(c, ..)| json!({ "name": spec.app(c), "class_name": c })).collect::<Vec<_>>(),
        "observability": { "enabled": true },
    })
}

/// The container applications by class, each with its id and image, created by `init` when missing.
fn ensure_apps(cf: &Cf, spec: &Spec, image: &str) -> Res<()> {
    let apps = cf.container_apps()?;
    let bound = cf.bindings(&spec.script)?;
    for (i, (class, binding, instance)) in CLASSES.iter().enumerate() {
        let name = spec.app(class);
        if apps.iter().any(|a| a["name"] == json!(name)) {
            println!("  container application {name}: exists");
            continue;
        }
        let ns = bound
            .iter()
            .find(|b| b["name"] == json!(binding))
            .and_then(|b| b["namespace_id"].as_str())
            .ok_or_else(|| format!("{}: the deployed version has no {binding} namespace", spec.script))?;
        cf.create_container_app(&json!({
            "name": name,
            "scheduling_policy": "default",
            "configuration": { "image": image, "instance_type": instance },
            "instances": 0,
            "max_instances": spec.max[i],
            "durable_objects": { "namespace_id": ns },
        }))?;
        println!("  container application {name}: created, {instance}, at most {} instances", spec.max[i]);
    }
    Ok(())
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
    let mut meta = metadata(&spec, bindings(&spec, &store, Some(&webhook)));
    match tag.flatten().as_deref() {
        None => {
            meta["migrations"] =
                json!({ "new_tag": MIGRATION_TAG, "steps": [{ "new_sqlite_classes": CLASSES.map(|c| c.0) }] })
        }
        Some(MIGRATION_TAG) => {}
        Some(t) => return Err(format!("{} has migration tag {t:?}, not {MIGRATION_TAG:?}", spec.script)),
    }
    cf.put_script(&spec.script, &meta, &bundle.modules())?;
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
    ensure_apps(cf, &spec, a.image)
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
            let apps = cf.container_apps()?;
            for (class, _, instance) in CLASSES {
                let app = apps
                    .iter()
                    .find(|x| x["name"] == json!(spec.app(class)))
                    .ok_or(format!("no application {}", spec.app(class)))?;
                cf.roll_container_app(
                    app["id"].as_str().unwrap_or(""),
                    &json!({ "image": a.image, "instance_type": instance }),
                )?;
            }
            cf.deploy_versions(&spec.script, &[(&new, 100)], &format!("graftyard runners upgrade {release} 100%"))?;
            println!("  {}: release {release} at 100%, container applications rolled to {}", spec.script, a.image);
            Ok(())
        }
        (Some(_), None) => Err("the deployment holds only this release, below 100%".into()),
        (None, None) => Err(format!("{} has no deployment: run graftyard runners init first", spec.script)),
        (None, Some(old)) => {
            let store = cf.store(STORE)?;
            let mut meta = metadata(&spec, bindings(&spec, &store, None));
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

    /// A runner account over the stub: no script yet, then the namespaces the upload made.
    /// Every request the stub saw: method, path under the account, body.
    type Log = Arc<Mutex<Vec<(String, String, Vec<u8>)>>>;
    type Answer = Box<dyn FnMut(&Seen) -> (u16, String) + Send>;

    fn account(log: Log) -> Answer {
        Box::new(move |s: &Seen| {
            let (m, p) = (s.line.split(' ').next().unwrap_or("").to_string(), path(s));
            log.lock().unwrap().push((m.clone(), p.clone(), s.body.clone()));
            let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
            match (m.as_str(), p.as_str()) {
                ("GET", "/secrets_store/stores") => ok(json!([{ "id": "st1", "name": "graft-runners" }])),
                ("GET", p) if p.starts_with("/secrets_store/stores/st1/secrets") => ok(json!([])),
                ("GET", "/workers/scripts") => ok(json!([])),
                ("GET", "/workers/scripts/graft-runners/subdomain") => ok(json!({ "enabled": false })),
                ("GET", "/workers/subdomain") => ok(json!({ "subdomain": "graftyard-runners" })),
                ("GET", "/containers/applications") => ok(json!([])),
                ("GET", "/workers/scripts/graft-runners/settings") => ok(json!({ "bindings": [
                    { "type": "durable_object_namespace", "name": "SANDBOX_STD3", "namespace_id": "ns3" },
                    { "type": "durable_object_namespace", "name": "SANDBOX_STD4", "namespace_id": "ns4" },
                ] })),
                _ => ok(json!({})),
            }
        })
    }

    #[test]
    fn init_writes_the_key_to_secrets_store_and_creates_both_instance_types() {
        let d = scratch();
        let log = Log::default();
        let mut answer = account(log.clone());
        let (url, _rx) = serve(13, move |s| answer(s));
        let cf = Cf::with(&url, "r", "t");
        let args = InitArgs {
            account: "r",
            spec: &d.join("runners.json"),
            bundle: &d.join("bundle"),
            image: IMAGE,
            app_key: &d.join("app.pem"),
            webhook_secret: &d.join("hook"),
        };
        init_with(&cf, args).unwrap();
        let log = log.lock().unwrap();
        let find = |m: &str, p: &str| log.iter().find(|(x, y, _)| x == m && y == p).map(|(.., b)| b.clone());
        let secret: Value =
            serde_json::from_slice(&find("POST", "/secrets_store/stores/st1/secrets").unwrap()).unwrap();
        assert_eq!(secret[0]["name"], json!("HostedRunnersAppKey"));
        let upload = String::from_utf8_lossy(&find("PUT", "/workers/scripts/graft-runners").unwrap()).to_string();
        // Only the bundle's modules, the migration for both Sandbox classes, and the key bound from the store.
        assert!(upload.contains("filename=\"entry.js\"") && upload.contains("filename=\"ab12-index_bg.wasm\""));
        assert!(!upload.contains("README"));
        assert!(upload.contains(r#""new_sqlite_classes":["SandboxStd3","SandboxStd4"]"#));
        assert!(upload.contains(r#""store_id":"st1""#) && upload.contains(r#""nodejs_compat""#));
        assert!(!upload.contains("BEGIN RSA"), "the App key goes only to Secrets Store");
        let sched: Value =
            serde_json::from_slice(&find("PUT", "/workers/scripts/graft-runners/schedules").unwrap()).unwrap();
        assert_eq!(sched, json!([{ "cron": "*/5 * * * *" }]));
        let apps: Vec<Value> = log
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/containers/applications")
            .map(|(.., b)| serde_json::from_slice(b).unwrap())
            .collect();
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0]["configuration"], json!({ "image": IMAGE, "instance_type": "standard-3" }));
        assert_eq!(
            (apps[0]["max_instances"].clone(), apps[0]["durable_objects"]["namespace_id"].clone()),
            (json!(20), json!("ns3"))
        );
        assert_eq!(apps[1]["configuration"]["instance_type"], json!("standard-4"));
        assert_eq!(apps[1]["durable_objects"]["namespace_id"], json!("ns4"));
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

    #[test]
    fn upgrade_canaries_at_10_percent_then_goes_to_100_only_a_day_later() {
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
        let log = log.lock().unwrap();
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
        let rolls: Vec<&str> = log.iter().filter(|(m, ..)| m == "PATCH").map(|(_, p, _)| p.as_str()).collect();
        assert_eq!(
            rolls,
            ["/containers/applications/a3", "/containers/applications/a4"],
            "the image rolls only at 100%"
        );
    }

    #[test]
    fn rfc3339_times_parse_to_epoch_seconds() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2026-11-15T12:00:00.123Z"), Some(1_794_744_000));
        assert_eq!(parse_utc("2024-03-01T00:00:00Z"), Some(1_709_251_200));
    }
}
