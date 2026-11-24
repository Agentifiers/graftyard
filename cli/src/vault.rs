//! `graftyard vault init` and `graftyard vault token` (`design/DESIGN.md`, "Install operations", Vault; Shape,
//! "Lifeboat feed", `ExportClaims`; `build/BUILD-PLAN.md`, M5-WP5).
//!
//! `init` deploys the vault Worker in its own account, under `CLOUDFLARE_VAULT_API_TOKEN`, creates its bucket if
//! needed, and preserves existing object-lock rules while locking the export prefixes for 35 days. It mints an
//! install-scoped export token with the customer key and uploads it only as the Worker's `EXPORT_TOKEN` secret.
//! `upgrade` uploads a later release's vault Worker as a new version configured as the deployed one, tagged with the
//! release id, and deploys it at 100%; a release already deployed is left alone. Both take a release folder
//! (`release.json`, `release.sig`, and the files it lists) and deploy only its `vault/` modules, after
//! `graftyard_release::ops::check_vault` verified `release.json` under the release key and found every `vault/` file
//! listed with its SHA-256, so a tampered or unsigned bundle never reaches the vault account (`design/DESIGN.md`,
//! "Install operations", Vault; OPEN-QUESTIONS R39 (b)).
//! `token` signs an export token offline with the customer key, for the install in install.toml, and writes it to a
//! file of mode 0600. Neither command prints the token or the key; they print only the token's `jti`, scope, holder,
//! and expiry (and, for `token`, the file it wrote). The token revokes itself with
//! `DELETE /api/graftyard/v1/export/token`.

use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;

use graftyard_identity::export_token::{self, EXPORT_TOKEN_MAX, ExportClaims, HOLDER_MAX};
use graftyard_release::{InstallId, Scope, public_key};
use graftyard_types::{Timestamp, Ulid};
use serde_json::{Value, json};

use graftyard_release::ops::{self, VaultArtifact};

use crate::cf::{Cf, Module};
use crate::config::{InstallToml, Res, hex, now, random, read_private_key, read_public_key, unhex};

const DAY_MS: u64 = 24 * 3600 * 1000;
pub const TOKEN_VAR: &str = "CLOUDFLARE_VAULT_API_TOKEN";
pub const LOCK_DAYS: u64 = 35;
pub const LOCKED: [&str; 4] = ["feed/", "entries/", "snapshot/", "pack/"];
const COMPAT_DATE: &str = "2026-10-01";
const PULL_CRON: &str = "* * * * *";

/// What `vault token` takes besides the install file.
pub struct TokenArgs<'a> {
    pub key: &'a Path,
    pub scope: &'a str,
    pub holder: &'a str,
    pub days: u64,
    pub out: &'a Path,
}

/// A release folder: `release.json`, `release.sig` (64 raw bytes or 128 hex characters, Ed25519), and every file
/// `release.json` lists, by its listed name relative to the folder; and the vendor's release public key (the genesis's
/// `Keys.release`).
pub struct Release<'a> {
    pub dir: &'a Path,
    pub release_key: &'a Path,
}

/// The release's vault Worker, once `check_vault` passed on the folder's `release.json`, signature, and `vault/` files.
fn verified(r: &Release) -> Res<VaultArtifact> {
    let read = |n: &str| std::fs::read(r.dir.join(n)).map_err(|e| format!("{}: {e}", r.dir.join(n).display()));
    let record = read("release.json")?;
    let raw = read("release.sig")?;
    let sig: [u8; 64] = match raw.len() {
        64 => raw.try_into().expect("64 bytes"),
        _ => unhex(String::from_utf8_lossy(&raw).trim())
            .and_then(|b| b.try_into().ok())
            .ok_or("release.sig is not 64 bytes")?,
    };
    let key = read_public_key(r.release_key)?;
    let vault = r.dir.join(ops::VAULT_FOLDER);
    let rd = std::fs::read_dir(&vault).map_err(|e| format!("{}: {e}", vault.display()))?;
    let mut files = Vec::new();
    for e in rd {
        let e = e.map_err(|e| e.to_string())?;
        let name = e.file_name().into_string().map_err(|_| "a vault/ file name is not UTF-8")?;
        if e.file_type().map_err(|e| e.to_string())?.is_file() {
            files.push((format!("{}{name}", ops::VAULT_FOLDER), read(&format!("{}{name}", ops::VAULT_FOLDER))?));
        }
    }
    ops::check_vault(&key, &record, graftyard_release::Signature(sig), &files)
        .map_err(|e| format!("release {}: refused ({}: {e})", r.dir.display(), e.variant()))
}

fn modules(a: &VaultArtifact) -> Res<Vec<Module<'_>>> {
    a.modules
        .iter()
        .map(|(n, b)| {
            let content_type = match n.rsplit_once('.').map(|(_, x)| x) {
                Some("wasm") => "application/wasm",
                Some("js" | "mjs") => "application/javascript+module",
                _ => return Err(format!("vault/{n}: not a .js or .wasm module")),
            };
            Ok(Module { name: n, content_type, bytes: b })
        })
        .collect()
}

/// What `vault init` takes besides the install file.
pub struct InitArgs<'a> {
    pub account: &'a str,
    pub key: &'a Path,
    pub release: Release<'a>,
    pub bucket: &'a str,
    pub region: &'a str,
    pub script: &'a str,
    pub edge: Option<&'a str>,
    pub days: u64,
}

/// The 35-day lock on every export prefix. `state/` stays unlocked because the pull cursor overwrites it.
fn lock_rules() -> Vec<Value> {
    LOCKED
        .iter()
        .map(|p| {
            json!({
                "id": format!("vault-{}-{LOCK_DAYS}d", p.trim_end_matches('/')),
                "enabled": true,
                "prefix": p,
                "condition": { "type": "Age", "maxAgeSeconds": LOCK_DAYS * 86_400 },
            })
        })
        .collect()
}

pub fn init(config: &Path, a: &InitArgs) -> Res<()> {
    let cf = Cf::from_env_var(a.account, TOKEN_VAR)?;
    init_with(&cf, config, a, now())
}

fn init_with(cf: &Cf, config: &Path, a: &InitArgs, at: Timestamp) -> Res<()> {
    let install = InstallToml::load(config)?;
    let anchor = install.anchor()?;
    let seed = read_private_key(a.key)?;
    let c = claims(anchor.install, Scope::Install, "vault", a.days, at, Ulid::new(at, random()))?;
    let text = export_token::sign(&c, &seed);
    let customer = public_key(&seed);
    if customer != anchor.customer {
        return Err(format!(
            "{} is not install.toml's customer key: the edge would refuse the vault's token",
            a.key.display()
        ));
    }
    match export_token::verify(&text, anchor.install, &anchor.customer, at) {
        Ok(v) if v == c => {}
        Ok(_) => return Err("the signed token verifies to other claims".into()),
        Err(e) => return Err(format!("the signed token does not verify: {e}")),
    }
    let artifact = verified(&a.release)?;
    let release = hex(&artifact.release.0);

    match cf.bucket_location(a.bucket, None)? {
        Some(location) => println!("  {}: exists in {location}", a.bucket),
        None => {
            let location = cf.create_bucket(a.bucket, a.region, None)?;
            println!("  {}: created in {location}", a.bucket);
        }
    }

    let ours = lock_rules();
    let mut rules: Vec<Value> =
        cf.bucket_lock(a.bucket)?.into_iter().filter(|rule| !ours.iter().any(|our| our["id"] == rule["id"])).collect();
    rules.extend(ours);
    cf.put_bucket_lock(a.bucket, &rules)?;
    println!("  {}: lock rules {} ({LOCK_DAYS} days on {})", a.bucket, rules.len(), LOCKED.join(", "));

    let edge = a.edge.map(String::from).unwrap_or_else(|| format!("https://{}", install.zone));
    let meta = json!({
        "main_module": "entry.js",
        "compatibility_date": COMPAT_DATE,
        "bindings": [
            { "type": "r2_bucket", "name": "VAULT", "bucket_name": a.bucket },
            { "type": "plain_text", "name": "GRAFTYARD_URL", "text": edge.trim_end_matches('/') },
            { "type": "secret_text", "name": "EXPORT_TOKEN", "text": text },
        ],
        "observability": { "enabled": true },
        "annotations": { "workers/tag": release, "workers/message": "graftyard vault init" },
    });
    cf.put_script(a.script, &meta, &modules(&artifact)?)?;
    cf.put_schedules(a.script, &[PULL_CRON])?;
    println!(
        "  {}: release {release} deployed, pull cron {PULL_CRON}, export token jti {} scope install holder {:?} exp {} \
         ({} days)",
        a.script,
        c.jti,
        &*c.holder,
        c.exp.0 / 1000,
        a.days
    );
    Ok(())
}

/// What `vault upgrade` takes.
pub struct UpgradeArgs<'a> {
    pub account: &'a str,
    pub script: &'a str,
    pub release: Release<'a>,
}

pub fn upgrade(a: &UpgradeArgs) -> Res<()> {
    let cf = Cf::from_env_var(a.account, TOKEN_VAR)?;
    upgrade_with(&cf, a)
}

/// The release the vault script serves at 100%, by its versions' `workers/tag`: `None` when no version does.
pub(crate) fn serving(cf: &Cf, script: &str) -> Res<Option<String>> {
    let (versions, _) = cf.deployed(script)?;
    for (id, pct) in &versions {
        if *pct >= 100.0 {
            return cf.version_tag(script, id);
        }
    }
    Ok(None)
}

/// Verifies first, so a refused release makes no API call; then skips a release the deployment already serves at
/// 100%, and otherwise uploads it as a version that keeps the deployed bindings (`VAULT`, `GRAFTYARD_URL`, and the
/// `EXPORT_TOKEN` secret, which no install token could set) and deploys it at 100%. The bindings and runtime settings
/// are read from the deployed version and sent (`ops::upload_metadata`), since `keep_bindings` would take them from the
/// newest upload, deployed or not; only the secret is kept (R57, DECISIONS "`deployed-bindings`").
pub(crate) fn upgrade_with(cf: &Cf, a: &UpgradeArgs) -> Res<()> {
    let artifact = verified(&a.release)?;
    let release = hex(&artifact.release.0);
    let (versions, _) = cf.deployed(a.script)?;
    if versions.is_empty() {
        return Err(format!("{} has no deployment: run graftyard vault init first", a.script));
    }
    if serving(cf, a.script)?.as_deref() == Some(release.as_str()) {
        println!("  {}: release {release} already serves 100%", a.script);
        return Ok(());
    }
    let (live, _) = versions.iter().max_by(|x, y| x.1.total_cmp(&y.1)).ok_or("a deployment with no versions")?;
    let mut meta = ops::upload_metadata(&cf.version(a.script, live)?, "entry.js", &release);
    meta["annotations"]["workers/message"] = json!("graftyard vault upgrade");
    let new = cf.upload_version(a.script, &meta, &modules(&artifact)?)?;
    cf.deploy_versions(a.script, &[(&new, 100)], &format!("graftyard vault upgrade {release}"))?;
    println!("  {}: release {release} uploaded as {new} and deployed at 100%", a.script);
    Ok(())
}

/// `install`, `owner:<id>`, or `repo:<id>`, as the token spells it.
fn parse_scope(s: &str) -> Res<Scope> {
    export_token::parse_scope(s).ok_or_else(|| format!("--scope {s:?}: expected install, owner:<id>, or repo:<id>"))
}

/// The claims of a token issued at `at` (whole seconds, as the token carries them), checked against the token's
/// bounds: a holder of 1 to `HOLDER_MAX` bytes, and 1 day up to `EXPORT_TOKEN_MAX`.
fn claims(install: InstallId, scope: Scope, holder: &str, days: u64, at: Timestamp, jti: Ulid) -> Res<ExportClaims> {
    let max_days = EXPORT_TOKEN_MAX.as_millis() as u64 / DAY_MS;
    if days == 0 || days > max_days {
        return Err(format!("--days {days}: an export token lives 1 to {max_days} days"));
    }
    if holder.is_empty() || holder.len() > HOLDER_MAX {
        return Err(format!("--holder: 1 to {HOLDER_MAX} bytes"));
    }
    let iat = Timestamp(at.0 / 1000 * 1000);
    Ok(ExportClaims { install, scope, holder: holder.into(), jti, iat, exp: Timestamp(iat.0 + days * DAY_MS) })
}

/// Writes `text` to `path`, created or truncated, readable by its owner only.
fn write_token(path: &Path, text: &str) -> Res<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // `mode` applies only to a file it creates: an existing file loses any wider mode before the token goes in.
    f.set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(format!("{text}\n").as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn token(config: &Path, a: &TokenArgs) -> Res<()> {
    let anchor = InstallToml::load(config)?.anchor()?;
    let scope = parse_scope(a.scope)?;
    let seed = read_private_key(a.key)?;
    let at = now();
    let c = claims(anchor.install, scope, a.holder, a.days, at, Ulid::new(at, random()))?;
    let text = export_token::sign(&c, &seed);
    let customer = public_key(&seed);
    match export_token::verify(&text, anchor.install, &customer, at) {
        Ok(v) if v == c => {}
        Ok(_) => return Err("the signed token verifies to other claims".into()),
        Err(e) => return Err(format!("the signed token does not verify: {e}")),
    }
    if customer != anchor.customer {
        eprintln!(
            "note: {} is not install.toml's customer key; the edge takes the token only if a later Keys entry made it \
             the customer key",
            a.key.display()
        );
    }
    write_token(a.out, &text)?;
    println!(
        "export token jti {} scope {} holder {:?} exp {} ({} days): wrote {} (mode 0600)",
        c.jti,
        export_token::scope_text(c.scope),
        &*c.holder,
        c.exp.0 / 1000,
        a.days,
        a.out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::config::{hex, mint_install};
    use crate::genesis::tests::{Seen, serve};

    const SEED: [u8; 32] = [9; 32];

    fn setup(name: &str) -> (std::path::PathBuf, InstallId) {
        let dir = std::env::temp_dir().join(format!("graftyard-vault-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let install = mint_install();
        let t = InstallToml {
            install: install.to_string(),
            account: "be2a".into(),
            zone: "git.acme.example".into(),
            customer_key: public_key(&SEED).hex(),
            evaluation: false,
            genesis: None,
            sites: Vec::new(),
            rtt: Vec::new(),
            default_placement: None,
        };
        t.save(&dir.join("install.toml")).unwrap();
        std::fs::write(dir.join("customer.key"), hex(&SEED) + "\n").unwrap();
        (dir, install)
    }

    const RELEASE_SEED: [u8; 32] = [3; 32];
    const VAULT_FILES: [(&str, &[u8]); 2] =
        [("vault/entry.js", b"export default {}"), ("vault/ab12-index_bg.wasm", b"\0asm")];

    /// A release folder signed by `seed`: `release.json` lists `listed`, and the folder holds `present`, plus a file
    /// outside `vault/` that the vault never reads. The release key file names `RELEASE_SEED`'s key.
    fn release_dir(
        dir: &Path,
        seed: &[u8; 32],
        listed: &[(&str, &[u8])],
        present: &[(&str, &[u8])],
    ) -> std::path::PathBuf {
        use sha2::{Digest as _, Sha256};
        let r = dir.join("release");
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(r.join("vault")).unwrap();
        std::fs::create_dir_all(r.join("edge")).unwrap();
        std::fs::write(r.join("edge/index.js"), "edge").unwrap();
        let mut files: Vec<Value> =
            listed.iter().map(|(n, b)| json!({ "name": n, "sha256": hex(&Sha256::digest(b)) })).collect();
        files.push(json!({ "name": "edge/index.js", "sha256": hex(&Sha256::digest(b"edge")) }));
        let rec = serde_json::to_vec(&json!({ "label": "v1", "files": files })).unwrap();
        for (n, b) in present {
            std::fs::write(r.join(n), b).unwrap();
        }
        std::fs::write(r.join("release.json"), &rec).unwrap();
        std::fs::write(r.join("release.sig"), graftyard_release::sign_record(seed, &rec).0).unwrap();
        std::fs::write(dir.join("release.pub"), public_key(&RELEASE_SEED).hex()).unwrap();
        r
    }

    fn good_release(dir: &Path) -> std::path::PathBuf {
        release_dir(dir, &RELEASE_SEED, &VAULT_FILES, &VAULT_FILES)
    }

    fn release_id(r: &Path) -> String {
        use sha2::{Digest as _, Sha256};
        hex(&Sha256::digest(std::fs::read(r.join("release.json")).unwrap()))
    }

    fn init_args<'a>(
        dir: &'a Path,
        key: &'a Path,
        release: &'a Path,
        edge: Option<&'a str>,
        days: u64,
    ) -> InitArgs<'a> {
        let release_key: &'a Path = Box::leak(dir.join("release.pub").into_boxed_path());
        InitArgs {
            account: "v",
            key,
            release: Release { dir: release, release_key },
            bucket: "graft-vault",
            region: "enam",
            script: "graft-vault",
            edge,
            days,
        }
    }

    fn path(s: &Seen) -> String {
        s.line.split(' ').nth(1).unwrap_or("").split("/accounts/v").nth(1).unwrap_or("").to_string()
    }

    /// The JSON metadata part of a script upload, without logging the upload or its secret.
    fn metadata(upload: &[u8]) -> Value {
        let text = std::str::from_utf8(upload).unwrap();
        let marker = "Content-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n";
        let json = text.split_once(marker).unwrap().1.split("\r\n--").next().unwrap();
        serde_json::from_str(json).unwrap()
    }

    type Log = Arc<Mutex<Vec<(String, String, Vec<u8>)>>>;

    /// The deployed vault version's bindings, as the versions API reads them: the secret without its value.
    fn old_bindings() -> Value {
        json!([
            { "type": "r2_bucket", "name": "VAULT", "bucket_name": "graft-vault-a" },
            { "type": "plain_text", "name": "GRAFTYARD_URL", "text": "https://edge.example" },
            { "type": "secret_text", "name": "EXPORT_TOKEN" },
        ])
    }

    const AT: Timestamp = Timestamp(1_800_000_000_123);

    #[test]
    fn init_creates_and_locks_a_fresh_vault_and_deploys_a_verified_export_token() {
        let (dir, install) = setup("init-fresh");
        let release = good_release(&dir);
        let key = dir.join("customer.key");
        let config = dir.join("install.toml");
        let log = Log::default();
        let answer_log = log.clone();
        let (url, _rx) = serve(6, move |s| {
            let (m, p) = (s.line.split(' ').next().unwrap_or("").to_string(), path(s));
            answer_log.lock().unwrap().push((m.clone(), p.clone(), s.body.clone()));
            let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
            match (m.as_str(), p.as_str()) {
                ("GET", "/r2/buckets/graft-vault") => (
                    404,
                    json!({ "success": false, "errors": [{ "code": 10000, "message": "not found" }] }).to_string(),
                ),
                ("POST", "/r2/buckets") => ok(json!({ "location": "enam" })),
                ("GET", "/r2/buckets/graft-vault/lock") => ok(json!({ "rules": [] })),
                ("PUT", "/r2/buckets/graft-vault/lock")
                | ("PUT", "/workers/scripts/graft-vault")
                | ("PUT", "/workers/scripts/graft-vault/schedules") => ok(json!({})),
                _ => panic!("unexpected request: {m} {p}"),
            }
        });
        init_with(
            &Cf::with(&url, "v", "t"),
            &config,
            &init_args(&dir, &key, &release, Some("https://edge.example///"), 30),
            AT,
        )
        .unwrap();

        let log = log.lock().unwrap();
        let find = |m: &str, p: &str| log.iter().find(|(x, y, _)| x == m && y == p).map(|(.., b)| b);
        let bucket: Value = serde_json::from_slice(find("POST", "/r2/buckets").unwrap()).unwrap();
        assert_eq!(bucket, json!({ "name": "graft-vault", "locationHint": "enam" }));
        let lock: Value = serde_json::from_slice(find("PUT", "/r2/buckets/graft-vault/lock").unwrap()).unwrap();
        let rules = lock["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 4);
        let prefixes: Vec<&str> = rules.iter().map(|r| r["prefix"].as_str().unwrap()).collect();
        assert_eq!(prefixes, LOCKED);
        for rule in rules {
            assert_eq!(rule["condition"], json!({ "type": "Age", "maxAgeSeconds": 3_024_000 }));
            assert_eq!(rule["enabled"], json!(true));
        }
        assert!(!rules.iter().any(|r| r["prefix"] == json!("state/")));

        let upload = find("PUT", "/workers/scripts/graft-vault").unwrap();
        let meta = metadata(upload);
        assert_eq!(meta["main_module"], json!("entry.js"));
        assert_eq!(meta["compatibility_date"], json!(COMPAT_DATE));
        assert_eq!(meta["observability"], json!({ "enabled": true }));
        assert_eq!(meta["annotations"]["workers/tag"], json!(release_id(&release)));
        // Only the release's vault/ modules go up, never another folder's.
        let text = String::from_utf8_lossy(upload);
        assert!(text.contains("filename=\"entry.js\"") && text.contains("filename=\"ab12-index_bg.wasm\""));
        assert!(!text.contains("index.js"));
        let bindings = meta["bindings"].as_array().unwrap();
        let url = bindings.iter().find(|b| b["name"] == json!("GRAFTYARD_URL")).unwrap();
        assert_eq!(url["text"], json!("https://edge.example"));
        let export = bindings.iter().find(|b| b["name"] == json!("EXPORT_TOKEN")).unwrap();
        assert_eq!(export["type"], json!("secret_text"));
        let verified = export_token::verify(export["text"].as_str().unwrap(), install, &public_key(&SEED), AT).unwrap();
        assert_eq!(verified.scope, Scope::Install);
        assert_eq!(&*verified.holder, "vault");
        assert_eq!(verified.install, install);
        assert_eq!(verified.exp.0 - verified.iat.0, 30 * DAY_MS);
        let schedules: Value =
            serde_json::from_slice(find("PUT", "/workers/scripts/graft-vault/schedules").unwrap()).unwrap();
        assert_eq!(schedules, json!([{ "cron": PULL_CRON }]));
        drop(log);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn init_rerun_preserves_foreign_lock_rules_and_restores_our_full_retention() {
        let (dir, _) = setup("init-rerun");
        let release = good_release(&dir);
        let key = dir.join("customer.key");
        let config = dir.join("install.toml");
        let keep = json!({
            "id": "keep-me",
            "enabled": true,
            "prefix": "other/",
            "condition": { "type": "Age", "maxAgeSeconds": 123 },
        });
        let old_pack = json!({
            "id": "vault-pack-35d",
            "enabled": true,
            "prefix": "pack/",
            "condition": { "type": "Age", "maxAgeSeconds": 86_400 },
        });
        let log = Log::default();
        let answer_log = log.clone();
        let answer_rules = json!([keep.clone(), old_pack]);
        let (url, _rx) = serve(5, move |s| {
            let (m, p) = (s.line.split(' ').next().unwrap_or("").to_string(), path(s));
            answer_log.lock().unwrap().push((m.clone(), p.clone(), s.body.clone()));
            let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
            match (m.as_str(), p.as_str()) {
                ("GET", "/r2/buckets/graft-vault") => ok(json!({ "location": "enam" })),
                ("GET", "/r2/buckets/graft-vault/lock") => ok(json!({ "rules": answer_rules })),
                ("PUT", "/r2/buckets/graft-vault/lock")
                | ("PUT", "/workers/scripts/graft-vault")
                | ("PUT", "/workers/scripts/graft-vault/schedules") => ok(json!({})),
                _ => panic!("unexpected request: {m} {p}"),
            }
        });
        init_with(&Cf::with(&url, "v", "t"), &config, &init_args(&dir, &key, &release, None, 365), AT).unwrap();

        let log = log.lock().unwrap();
        assert!(!log.iter().any(|(m, p, _)| m == "POST" && p == "/r2/buckets"));
        let lock = log.iter().find(|(m, p, _)| m == "PUT" && p == "/r2/buckets/graft-vault/lock").unwrap();
        let lock: Value = serde_json::from_slice(&lock.2).unwrap();
        let rules = lock["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 5);
        assert!(rules.contains(&keep));
        let pack = rules.iter().find(|r| r["id"] == json!("vault-pack-35d")).unwrap();
        assert_eq!(pack["condition"]["maxAgeSeconds"], json!(3_024_000));
        assert!(!rules.iter().any(|r| r["condition"]["maxAgeSeconds"] == json!(86_400)));
        let upload = log.iter().find(|(m, p, _)| m == "PUT" && p == "/workers/scripts/graft-vault").unwrap();
        let meta = metadata(&upload.2);
        let url = meta["bindings"].as_array().unwrap().iter().find(|b| b["name"] == json!("GRAFTYARD_URL")).unwrap();
        assert_eq!(url["text"], json!("https://git.acme.example"));
        drop(log);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn init_refuses_a_key_other_than_the_install_customer_key_before_any_api_call() {
        let (dir, _) = setup("init-wrong-key");
        let release = good_release(&dir);
        let key = dir.join("other.key");
        std::fs::write(&key, hex(&[8; 32]) + "\n").unwrap();
        let config = dir.join("install.toml");
        let calls = Arc::new(AtomicUsize::new(0));
        let answer_calls = calls.clone();
        let (url, _rx) = serve(1, move |_| {
            answer_calls.fetch_add(1, Ordering::SeqCst);
            panic!("a wrong customer key reached the Cloudflare API")
        });
        let err =
            init_with(&Cf::with(&url, "v", "t"), &config, &init_args(&dir, &key, &release, None, 365), AT).unwrap_err();
        assert!(err.contains("is not install.toml's customer key: the edge would refuse the vault's token"), "{err}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_written_token_verifies_with_the_asked_scope_holder_and_expiry() {
        let (dir, install) = setup("ok");
        let out = dir.join("export.token");
        // An existing, world-readable file is truncated and narrowed to 0600.
        std::fs::write(&out, "old contents that are longer than nothing").unwrap();
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o644)).unwrap();
        let before = now();
        let args =
            TokenArgs { key: &dir.join("customer.key"), scope: "owner:42", holder: "acme-org", days: 30, out: &out };
        token(&dir.join("install.toml"), &args).unwrap();
        assert_eq!(std::fs::metadata(&out).unwrap().permissions().mode() & 0o777, 0o600);
        let text = std::fs::read_to_string(&out).unwrap();
        let c = export_token::verify(text.trim_end(), install, &public_key(&SEED), now()).unwrap();
        assert_eq!(c.scope, Scope::Owner(graftyard_types::AccountId(graftyard_types::DbId(42))));
        assert_eq!(&*c.holder, "acme-org");
        assert_eq!(c.exp.0 - c.iat.0, 30 * DAY_MS);
        assert!(c.iat.0 + 1000 > before.0 && c.iat <= now());
        assert_eq!(c.jti.time().0 / 1000, c.iat.0 / 1000, "the jti is minted at issue");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_401_days_a_bad_scope_and_a_bad_holder_and_writes_nothing() {
        let (dir, _) = setup("refuse");
        let out = dir.join("export.token");
        let key = dir.join("customer.key");
        let config = dir.join("install.toml");
        let run = |scope: &str, holder: &str, days: u64| {
            token(&config, &TokenArgs { key: &key, scope, holder, days, out: &out })
        };
        assert!(run("install", "vault", 401).unwrap_err().contains("--days 401"));
        assert!(run("install", "vault", 0).is_err());
        assert!(run("org:1", "vault", 365).unwrap_err().contains("--scope"));
        assert!(run("repo:", "vault", 365).is_err());
        assert!(run("install", "", 365).is_err());
        assert!(run("install", &"x".repeat(HOLDER_MAX + 1), 365).is_err());
        assert!(!out.exists());
        run("repo:7", "vault", 400).unwrap();
        assert!(out.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A release whose `release.json` does not verify, or whose `vault/` files it does not list exactly, is refused by
    /// `init` and `upgrade` alike before any Cloudflare call.
    #[test]
    fn init_and_upgrade_refuse_a_tampered_or_unsigned_release_before_any_api_call() {
        let (dir, _) = setup("tampered");
        let key = dir.join("customer.key");
        let config = dir.join("install.toml");
        let entry: (&str, &[u8]) = ("vault/entry.js", b"export default {}");
        let wasm: (&str, &[u8]) = ("vault/ab12-index_bg.wasm", b"\0asm");
        type Files<'a> = &'a [(&'a str, &'a [u8])];
        let changed: [(&str, &[u8]); 2] = [entry, (wasm.0, b"\0asm!")];
        let cases: [(&str, [u8; 32], Files, Files, &str); 5] = [
            ("signed by another key", [4; 32], &VAULT_FILES, &VAULT_FILES, "does not verify"),
            ("a changed module", RELEASE_SEED, &VAULT_FILES, &changed, "lists no vault/ab12-index_bg.wasm"),
            (
                "a module the record does not list",
                RELEASE_SEED,
                &[entry],
                &VAULT_FILES,
                "lists no vault/ab12-index_bg.wasm",
            ),
            ("a listed module missing", RELEASE_SEED, &VAULT_FILES, &[entry], "holds no vault/ab12-index_bg.wasm"),
            ("no entry.js", RELEASE_SEED, &[wasm], &[wasm], "no vault/entry.js"),
        ];
        for (what, seed, listed, present, want) in cases {
            let rel = release_dir(&dir, &seed, listed, present);
            let (url, _rx) = serve(1, |s| panic!("a refused release reached the Cloudflare API: {}", s.line));
            let cf = Cf::with(&url, "v", "t");
            let err = init_with(&cf, &config, &init_args(&dir, &key, &rel, None, 30), AT).unwrap_err();
            assert!(err.contains(want) && err.contains("BadRecord"), "init, {what}: {err}");
            let up = UpgradeArgs {
                account: "v",
                script: "graft-vault",
                release: Release { dir: &rel, release_key: &dir.join("release.pub") },
            };
            let err = upgrade_with(&cf, &up).unwrap_err();
            assert!(err.contains(want), "upgrade, {what}: {err}");
        }
        // The signature is over release.json's bytes: one changed byte in the record is refused too.
        let rel = good_release(&dir);
        let mut rec = std::fs::read(rel.join("release.json")).unwrap();
        let n = rec.len() - 2;
        rec[n] ^= 1;
        std::fs::write(rel.join("release.json"), rec).unwrap();
        let (url, _rx) = serve(1, |s| panic!("a refused release reached the Cloudflare API: {}", s.line));
        let err =
            init_with(&Cf::with(&url, "v", "t"), &config, &init_args(&dir, &key, &rel, None, 30), AT).unwrap_err();
        assert!(err.contains("does not verify"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `upgrade` uploads the verified release as a version configured as the deployed one, tagged with the release id,
    /// and deploys it at 100%; run again once it serves 100%, it changes nothing. The deployed version's bindings and
    /// runtime are sent and only the secret is kept, so a newer upload that was never deployed sets none of them (R57).
    #[test]
    fn upgrade_deploys_a_verified_release_keeping_the_bindings_then_leaves_it() {
        let (dir, _) = setup("upgrade");
        let rel = good_release(&dir);
        let rid = release_id(&rel);
        let state = Arc::new(Mutex::new(false));
        let log = Log::default();
        let (st, lg, rid2) = (state.clone(), log.clone(), rid.clone());
        let (url, _rx) = serve(9, move |s| {
            let (m, p) = (s.line.split(' ').next().unwrap_or("").to_string(), path(s));
            lg.lock().unwrap().push((m.clone(), p.clone(), s.body.clone()));
            let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
            let deployed = if *st.lock().unwrap() { "new" } else { "old" };
            match (m.as_str(), p.as_str()) {
                ("GET", "/workers/scripts/graft-vault/deployments") => ok(json!({ "deployments": [{
                    "versions": [{ "version_id": deployed, "percentage": 100 }], "created_on": "2026-11-23T00:00:00Z" }] })),
                ("GET", "/workers/scripts/graft-vault/versions/new") => {
                    ok(json!({ "annotations": { "workers/tag": rid2 } }))
                }
                ("GET", "/workers/scripts/graft-vault/versions/old") => ok(json!({
                    "annotations": { "workers/tag": "aa" },
                    "resources": { "bindings": old_bindings(), "script_runtime": { "compatibility_date": "2026-09-01" } },
                })),
                ("POST", "/workers/scripts/graft-vault/versions") => ok(json!({ "id": "new" })),
                ("POST", "/workers/scripts/graft-vault/deployments") => {
                    *st.lock().unwrap() = true;
                    ok(json!({}))
                }
                _ => panic!("unexpected request: {m} {p}"),
            }
        });
        let cf = Cf::with(&url, "v", "t");
        let up = UpgradeArgs {
            account: "v",
            script: "graft-vault",
            release: Release { dir: &rel, release_key: &dir.join("release.pub") },
        };
        upgrade_with(&cf, &up).unwrap();
        upgrade_with(&cf, &up).unwrap();
        let log = log.lock().unwrap();
        let uploads: Vec<_> =
            log.iter().filter(|(m, p, _)| m == "POST" && p == "/workers/scripts/graft-vault/versions").collect();
        assert_eq!(uploads.len(), 1, "the second run uploads nothing");
        let meta = metadata(&uploads[0].2);
        assert_eq!(meta["annotations"]["workers/tag"], json!(rid));
        assert_eq!(meta["keep_bindings"], json!(["secret_text"]), "only the export token is kept");
        let sent = old_bindings().as_array().unwrap()[..2].to_vec();
        assert_eq!(meta["bindings"], json!(sent), "the deployed version's bucket and URL, and no secret");
        assert_eq!(meta["compatibility_date"], json!("2026-09-01"), "the deployed version's runtime");
        let deploys: Vec<_> =
            log.iter().filter(|(m, p, _)| m == "POST" && p == "/workers/scripts/graft-vault/deployments").collect();
        assert_eq!(deploys.len(), 1);
        let d: Value = serde_json::from_slice(&deploys[0].2).unwrap();
        assert_eq!(d["versions"], json!([{ "version_id": "new", "percentage": 100 }]));
        drop(log);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
