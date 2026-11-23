//! `graftyard release entry`: builds one unsigned `Release` entry from JSON, bound to `--prev` (the digest of the
//! group's last signed entry, which a 409 names), for `graftyard sign` and `graftyard release post`
//! (`design/DESIGN.md`, Shape, "Release group"; "Install operations", Keys). M5-WP3's `release roll` and `graft-ops`
//! build the same entries from a release bundle; this is the operator's hand tool, which BF-41, BF-42, and BF-45
//! drive. A `Keys` spec starts from `--keys`, the latest committed signed genesis or `Keys` entry.
//!
//! The JSON names one entry kind with its fields, in the design's names: `{"Keys": {"executor": "<hex>"}}` (any field
//! left out keeps the current key, and `generation` is the current plus one), `{"SiteMode": {"site": 1, "mode":
//! "Drain"}}`, `{"ReleaseUploaded": {"record": "<path>", "record_sig": "<hex>", "versions": [["Edge", "<uuid>"]],
//! "reads": ["32.1"]}}`, `{"IntentStep": {"intent": "<ulid>", "step": 0}}`, `{"IntentDone": {"intent": "<ulid>"}}`,
//! `{"Halt": {"intent": "<ulid>"}}`, `{"Intent": {"id": "<ulid>", "ttl_s": 3600, "intent": {"EdgeRoll": {"release":
//! "<hex>"}}, "undo": [{"EdgePointer": {"pin": "<hex>"}}], "undo_key": "<private key path>"}}`, and
//! `{"Undo": {"intent": "<ulid>", "entry": {"EdgePointer": {"pin": "<hex>"}}}}`, which binds a pre-signed undo to its
//! parent for the `sign` that pre-signed it, and `{"StaleMode": {"scope": {"Repo": 7}, "until_s": 600}}`. An intent may
//! be `{"Rotate": {"secret": "DeployToken", "slot": 1}}` (M5-WP6).

use std::fs;
use std::path::Path;

use graftyard_release::{
    Bound, DayTarget, Format, FormatSet, ImageRef, Intent, IntentId, KeyId, Mode, ReleaseEntry, ReleaseUploaded, Scope,
    Script, SecretName, Signature, SignedEntry, SignedIntent, SiteMode, presign_undo,
};
use graftyard_types::{AccountId, ChainId, DbId, Digest, ReleaseId, RepoId, SiteId, Timestamp, Ulid};
use serde_json::Value;

use crate::config::{InstallToml, Res, now, parse_public_key, random, read_private_key, unhex};
use crate::genesis::{Answer, post_entry};

/// Every `SecretName`, so `Rotate` parses one by its name.
const SECRET_NAMES: [SecretName; 12] = [
    SecretName::DeployToken,
    SecretName::ReadToken,
    SecretName::TunnelToken,
    SecretName::MintWrapKey,
    SecretName::SshHostKeys,
    SecretName::McpAppKey,
    SecretName::McpClientSecret,
    SecretName::InsightsAppKey,
    SecretName::AgentsAppKey,
    SecretName::WorkspaceAppKey,
    SecretName::SecretsWrapKey,
    SecretName::HostedRunnersAppKey,
];

fn field<'a>(v: &'a Value, k: &str) -> Res<&'a Value> {
    v.get(k).ok_or_else(|| format!("missing field {k:?} in {v}"))
}

fn str_field<'a>(v: &'a Value, k: &str) -> Res<&'a str> {
    field(v, k)?.as_str().ok_or_else(|| format!("{k} is not a string"))
}

fn h32(s: &str) -> Res<[u8; 32]> {
    Digest::parse_hex(s).map(|d| d.0).ok_or_else(|| format!("{s:?} is not 64 lower-case hex digits"))
}

fn release_id(v: &Value, k: &str) -> Res<ReleaseId> {
    h32(str_field(v, k)?).map(ReleaseId)
}

fn site(v: &Value, k: &str) -> Res<SiteId> {
    field(v, k)?.as_u64().filter(|s| *s < 64).map(|s| SiteId(s as u8)).ok_or_else(|| format!("{k} is not a site id"))
}

fn intent_id(v: &Value, k: &str) -> Res<IntentId> {
    let s = str_field(v, k)?;
    s.parse::<Ulid>().map(IntentId).map_err(|e| e.to_string())
}

fn format(s: &str) -> Res<Format> {
    let (f, v) = s.split_once('.').ok_or_else(|| format!("format {s:?} is not family.version"))?;
    Ok(Format { family: f.parse().map_err(|_| s.to_string())?, version: v.parse().map_err(|_| s.to_string())? })
}

fn script(s: &str) -> Res<Script> {
    Ok(match s {
        "Workflows" => Script::Workflows,
        "Front" => Script::Front,
        "Edge" => Script::Edge,
        "Ops" => Script::Ops,
        "Mcp" => Script::Mcp,
        "AgentsFront" => Script::AgentsFront,
        "Controller" => Script::Controller,
        s => match s.strip_prefix("Site") {
            Some(n) => Script::Site(SiteId(n.parse().map_err(|_| format!("script {s:?}"))?)),
            None => return Err(format!("script {s:?}")),
        },
    })
}

fn one(v: &Value) -> Res<(&str, &Value)> {
    let o = v.as_object().filter(|o| o.len() == 1).ok_or_else(|| format!("{v} names more or less than one kind"))?;
    let (k, b) = o.iter().next().expect("one");
    Ok((k.as_str(), b))
}

fn day_target(v: &Value) -> Res<DayTarget> {
    if v.as_str() == Some("Workflows") {
        return Ok(DayTarget::Workflows);
    }
    let (k, b) = one(v)?;
    let s = b.as_u64().filter(|s| *s < 64).map(|s| SiteId(s as u8)).ok_or("a day target names a site id")?;
    match k {
        "Site" => Ok(DayTarget::Site(s)),
        "Containers" => Ok(DayTarget::Containers(s)),
        _ => Err(format!("day target {k:?}")),
    }
}

fn intent(v: &Value) -> Res<Intent> {
    let (k, b) = one(v)?;
    Ok(match k {
        "SiteDay" => Intent::SiteDay { target: day_target(field(b, "target")?)?, release: release_id(b, "release")? },
        "ContainerDay" => Intent::ContainerDay { site: site(b, "site")?, release: release_id(b, "release")? },
        "EdgeRoll" => Intent::EdgeRoll { release: release_id(b, "release")? },
        "FrontRoll" => Intent::FrontRoll { release: release_id(b, "release")? },
        "OpsRoll" => Intent::OpsRoll { release: release_id(b, "release")? },
        "McpRoll" => Intent::McpRoll { release: release_id(b, "release")? },
        "ReplaceMember" => Intent::ReplaceMember { lost: site(b, "lost")?, spare: site(b, "spare")? },
        "RestorePlacement" => Intent::RestorePlacement { site: site(b, "site")? },
        // `{"Rotate": {"secret": "DeployToken", "slot": 1}}` (M5-WP6): the secret by its `SecretName`.
        "Rotate" => {
            let name = str_field(b, "secret")?;
            let secret = SECRET_NAMES
                .iter()
                .find(|s| format!("{s:?}") == name)
                .copied()
                .ok_or_else(|| format!("secret {name:?} is not a SecretName"))?;
            Intent::Rotate {
                secret,
                slot: field(b, "slot")?.as_u64().filter(|s| *s <= 1).ok_or("slot is 0 or 1")? as u8,
            }
        }
        _ => return Err(format!("intent {k:?} is not built in this tool")),
    })
}

/// A forward or undo body, without signatures.
fn body(v: &Value, t: &InstallToml, keys: &Value) -> Res<ReleaseEntry> {
    let (k, b) = one(v)?;
    Ok(match k {
        "Keys" => {
            let cur = |n: &str| keys.get(n).and_then(Value::as_str).map(str::to_string);
            let key = |n: &str| -> Res<Option<KeyId>> {
                match b.get(n) {
                    Some(Value::Null) => Ok(None),
                    Some(Value::String(s)) => Ok(Some(parse_public_key(s)?)),
                    Some(_) => Err(format!("{n}: a hex key or null")),
                    None => cur(n).map(|s| parse_public_key(&s)).transpose(),
                }
            };
            let need = |n: &str| key(n)?.ok_or_else(|| format!("Keys.{n} cannot be null"));
            let generation = keys["generation"].as_u64().ok_or("a Keys spec needs --keys")? + 1;
            let mut ring = smallvec::SmallVec::new();
            for r in keys["ring"].as_array().into_iter().flatten() {
                ring.push((SiteId(r[0].as_u64().unwrap_or(0) as u8), parse_public_key(r[1].as_str().unwrap_or(""))?));
            }
            ReleaseEntry::Keys(Box::new(graftyard_release::Keys {
                generation,
                customer: need("customer")?,
                vendor: key("vendor")?,
                release: need("release")?,
                drain: need("drain")?,
                executor: need("executor")?,
                agents_executor: key("agents_executor")?,
                ring,
            }))
        }
        "SiteMode" => ReleaseEntry::SiteMode(SiteMode {
            site: site(b, "site")?,
            mode: match str_field(b, "mode")? {
                "Drain" => Mode::Drain,
                "Normal" => Mode::Normal,
                m => return Err(format!("mode {m:?}")),
            },
        }),
        "ReleaseUploaded" => {
            let record = fs::read(str_field(b, "record")?).map_err(|e| e.to_string())?;
            let release = ReleaseId(<sha2::Sha256 as sha2::Digest>::digest(&record).into());
            let sig: [u8; 64] = unhex(str_field(b, "record_sig")?)
                .and_then(|v| v.try_into().ok())
                .ok_or("record_sig is 128 hex digits")?;
            let versions = field(b, "versions")?
                .as_array()
                .ok_or("versions is a list")?
                .iter()
                .map(|p| {
                    let v: graftyard_release::VersionId =
                        p[1].as_str().unwrap_or("").parse().map_err(|_| format!("version {}", p[1]))?;
                    Ok((script(p[0].as_str().unwrap_or(""))?, v))
                })
                .collect::<Res<_>>()?;
            let reads = FormatSet::new(
                field(b, "reads")?
                    .as_array()
                    .ok_or("reads is a list")?
                    .iter()
                    .map(|f| format(f.as_str().unwrap_or("")))
                    .collect::<Res<Vec<_>>>()?,
            );
            let images = b
                .get("images")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|i| {
                    Ok(ImageRef { name: str_field(i, "name")?.into(), digest: Digest(h32(str_field(i, "digest")?)?) })
                })
                .collect::<Res<_>>()?;
            ReleaseEntry::ReleaseUploaded(Box::new(ReleaseUploaded {
                release,
                record: record.into(),
                record_sig: Signature(sig),
                versions,
                images,
                reads,
            }))
        }
        "IntentStep" => ReleaseEntry::IntentStep {
            intent: intent_id(b, "intent")?,
            step: field(b, "step")?.as_u64().ok_or("step")? as u8,
        },
        "IntentDone" => ReleaseEntry::IntentDone { intent: intent_id(b, "intent")? },
        "SiteRelease" => ReleaseEntry::SiteRelease { site: site(b, "site")?, release: release_id(b, "release")? },
        "Rollback" => {
            ReleaseEntry::Rollback { target: day_target(field(b, "target")?)?, release: release_id(b, "release")? }
        }
        "EdgePointer" => ReleaseEntry::EdgePointer { pin: release_id(b, "pin")? },
        "OpsPointer" => ReleaseEntry::OpsPointer { pin: release_id(b, "pin")? },
        "McpPointer" => ReleaseEntry::McpPointer { pin: release_id(b, "pin")? },
        "SiteLost" => ReleaseEntry::SiteLost { site: site(b, "site")? },
        // `{"StaleMode": {"scope": "Install" | {"Owner": id} | {"Repo": id}, "until_s": seconds from now}}`, or an
        // absolute `"until"` in epoch ms.
        "StaleMode" => {
            let sc = field(b, "scope")?;
            let scope = match sc.as_str() {
                Some("Install") => Scope::Install,
                _ => {
                    let (k, id) = one(sc)?;
                    let id = DbId(id.as_i64().ok_or("a scope names an id")?);
                    match k {
                        "Owner" => Scope::Owner(AccountId(id)),
                        "Repo" => Scope::Repo(RepoId(id)),
                        _ => return Err(format!("scope {k:?}")),
                    }
                }
            };
            let until = match (b.get("until").and_then(Value::as_u64), b.get("until_s").and_then(Value::as_u64)) {
                (Some(ms), _) => Timestamp(ms),
                (None, Some(s)) => Timestamp(now().0 + s * 1000),
                (None, None) => return Err("a StaleMode needs until (epoch ms) or until_s".into()),
            };
            ReleaseEntry::StaleMode { scope, until }
        }
        "Halt" => ReleaseEntry::Halt { intent: intent_id(b, "intent")? },
        "Abort" => ReleaseEntry::Abort { intent: intent_id(b, "intent")?, target: release_id(b, "target")? },
        "Intent" => {
            let id = match b.get("id").and_then(Value::as_str) {
                Some(s) => IntentId(s.parse().map_err(|e: graftyard_types::ParseUlidError| e.to_string())?),
                None => IntentId(Ulid::new(now(), random())),
            };
            let issued_at = now();
            let ttl = b.get("ttl_s").and_then(Value::as_u64).unwrap_or(3_600);
            let undo_key = b.get("undo_key").and_then(Value::as_str);
            let mut undo = Vec::new();
            for u in b.get("undo").and_then(Value::as_array).into_iter().flatten() {
                let u = body(u, t, keys)?;
                let seed = read_private_key(Path::new(undo_key.ok_or("an intent with undos needs undo_key")?))?;
                let sig = presign_undo(t.anchor()?.install, id, &u, &seed);
                undo.push((u, sig));
            }
            ReleaseEntry::Intent(Box::new(SignedIntent {
                id,
                issued_at,
                not_after: Timestamp(issued_at.0 + ttl * 1000),
                intent: intent(field(b, "intent")?)?,
                undo,
            }))
        }
        _ => return Err(format!("entry kind {k:?} is not built in this tool")),
    })
}

/// The latest `Keys` from a signed genesis or `Keys` entry file, as the JSON the `Keys` spec merges over.
fn keys_json(path: &Path) -> Res<Value> {
    let e = SignedEntry::decode(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let k = match e.body {
        ReleaseEntry::Genesis(g) => g.keys,
        ReleaseEntry::Keys(k) => *k,
        b => return Err(format!("{}: a {}, not a genesis or Keys", path.display(), b.kind())),
    };
    Ok(serde_json::json!({
        "generation": k.generation,
        "customer": k.customer.hex(),
        "vendor": k.vendor.map(|x| x.hex()),
        "release": k.release.hex(),
        "drain": k.drain.hex(),
        "executor": k.executor.hex(),
        "agents_executor": k.agents_executor.map(|x| x.hex()),
        "ring": k.ring.iter().map(|(s, x)| serde_json::json!([s.0, x.hex()])).collect::<Vec<_>>(),
    }))
}

/// Builds an unsigned entry from `spec`, bound to `prev` (or, for `{"Undo": ...}`, to its intent).
pub fn entry(config: &Path, spec: &Path, out: &Path, prev: Option<&str>, keys: Option<&Path>) -> Res<()> {
    let t = InstallToml::load(config)?;
    let v: Value = serde_json::from_slice(&fs::read(spec).map_err(|e| format!("{}: {e}", spec.display()))?)
        .map_err(|e| format!("{}: {e}", spec.display()))?;
    let keys = keys.map(keys_json).transpose()?.unwrap_or(Value::Null);
    let install = t.anchor()?.install;
    let (bound, body) = if let Some(u) = v.get("Undo") {
        let parent = intent_id(u, "intent")?;
        (
            Bound { install, group: ChainId::Release, prev: None, parent: Some(parent) },
            body(field(u, "entry")?, &t, &keys)?,
        )
    } else {
        let prev = prev.ok_or("--prev: the digest of the group's last signed entry")?;
        (Bound { install, group: ChainId::Release, prev: Some(Digest(h32(prev)?)), parent: None }, body(&v, &t, &keys)?)
    };
    let e = SignedEntry::unsigned(bound, body);
    fs::write(out, e.encode()).map_err(|e| format!("{}: {e}", out.display()))?;
    println!(
        "wrote unsigned {} to {}, prev {}{}",
        e.body.kind(),
        out.display(),
        e.bound.prev.map(|d| d.hex()).unwrap_or_else(|| "none".into()),
        match &e.body {
            ReleaseEntry::Intent(i) => format!(", intent {}", i.id.0),
            _ => String::new(),
        }
    );
    Ok(())
}

/// Posts a signed entry's bytes and prints the edge's answer as one JSON line.
pub fn post(config: &Path, input: &Path, edge: Option<&str>) -> Res<()> {
    let t = InstallToml::load(config)?;
    let bytes = fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let edge = edge.map(String::from).unwrap_or_else(|| format!("https://{}", t.zone));
    let a = post_entry(&edge, &bytes)?;
    let line = match &a {
        Answer::Created { index, digest } => serde_json::json!({ "status": 201, "index": index, "digest": digest }),
        Answer::Stale { prev, index } => serde_json::json!({ "status": 409, "prev": prev, "index": index }),
        Answer::Refused { status, message, refusal } => {
            serde_json::json!({ "status": status, "refusal": refusal, "message": message })
        }
    };
    println!("{line}");
    Ok(())
}

/// The group's committed entries from a file of `/view` answers (`{"count", "entries": [[hex, at], ...]}`, one answer
/// or a list of them, from position 0), replayed against this install's anchor: what `release roll` decides on. Every
/// entry passes the group's own `check`, so a file another install wrote, or one with an entry changed, is refused.
pub fn replay(t: &InstallToml, path: &Path) -> Res<graftyard_release::view::Replica> {
    let v: Value = serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let pages = match &v {
        Value::Array(a) => a.clone(),
        _ => vec![v],
    };
    let mut entries = Vec::new();
    for p in &pages {
        for e in p["entries"].as_array().into_iter().flatten() {
            let bytes = e[0].as_str().and_then(unhex).ok_or("an entry is [hex, at]")?;
            entries.push((bytes, Timestamp(e[1].as_u64().ok_or("an entry is [hex, at]")?)));
        }
    }
    let mut r = graftyard_release::view::Replica::default();
    r.extend(&t.anchor()?, 0, &entries).map_err(|e| format!("{}: {}: {e}", path.display(), e.variant()))?;
    if r.fold.genesis.is_none() {
        return Err(format!("{}: no genesis", path.display()));
    }
    Ok(r)
}

/// `graftyard release roll R`: from the group's entries (`--entries`), signs the first intent of the roll order that R
/// has not finished, with its pre-signed undos, by `key`, bound to the last digest, and posts it, or writes it to
/// `out`. One intent per run: the operator, or the vendor's release service, runs it again after each `IntentDone`.
#[allow(clippy::too_many_arguments)]
pub fn roll(
    config: &Path,
    release: &str,
    entries: &Path,
    key: &Path,
    ttl_s: u64,
    out: Option<&Path>,
    edge: Option<&str>,
    vault: Option<&VaultStep>,
) -> Res<()> {
    let token = std::env::var(crate::vault::TOKEN_VAR).ok();
    let install = std::env::var("CLOUDFLARE_API_TOKEN").ok();
    let api = std::env::var("GRAFTYARD_CF_API").unwrap_or_else(|_| "https://api.cloudflare.com/client/v4".into());
    let creds = Creds { vault: token.as_deref(), install: install.as_deref(), api: &api };
    roll_in(config, release, entries, key, ttl_s, out, edge, vault, &creds)
}

/// The tokens `release roll`'s vault step sees: the vault account's, the install's (only to refuse it), and the API.
struct Creds<'a> {
    vault: Option<&'a str>,
    install: Option<&'a str>,
    api: &'a str,
}

#[allow(clippy::too_many_arguments)]
fn roll_in(
    config: &Path,
    release: &str,
    entries: &Path,
    key: &Path,
    ttl_s: u64,
    out: Option<&Path>,
    edge: Option<&str>,
    vault: Option<&VaultStep>,
    creds: &Creds,
) -> Res<()> {
    use graftyard_release::ops::{Roll, next_roll, sign_intent, vault_due};
    let t = InstallToml::load(config)?;
    let r = replay(&t, entries)?;
    let rid = ReleaseId(h32(release)?);
    if vault_due(&r.fold, rid) {
        println!("{}", vault_step(&t, rid, vault, creds)?);
    }
    let (intent, undo) = match next_roll(&r.fold, rid) {
        Roll::Done => {
            println!("{{\"roll\": \"done\", \"release\": \"{release}\"}}");
            return Ok(());
        }
        Roll::Wait(why) => return Err(format!("release roll waits: {why}")),
        Roll::Next { intent, undo } => (intent, undo),
    };
    let seed = read_private_key(key)?;
    let id = IntentId(Ulid::new(now(), random()));
    let e = sign_intent(&r.fold, intent, &undo, &seed, id, now(), ttl_s * 1000);
    let kind = match &e.body {
        ReleaseEntry::Intent(i) => format!("{:?}", i.intent),
        _ => unreachable!("sign_intent signs an intent"),
    };
    eprintln!("release roll: intent {} {kind}, {} undo(s)", id.0, undo.len());
    if let Some(out) = out {
        fs::write(out, e.encode()).map_err(|e| format!("{}: {e}", out.display()))?;
        println!("{}", serde_json::json!({ "intent": id.0.to_string(), "wrote": out.display().to_string() }));
        return Ok(());
    }
    let edge = edge.map(String::from).unwrap_or_else(|| format!("https://{}", t.zone));
    let line = match post_entry(&edge, &e.encode())? {
        Answer::Created { index, digest } => serde_json::json!({ "status": 201, "index": index, "digest": digest }),
        Answer::Stale { prev, index } => serde_json::json!({ "status": 409, "prev": prev, "index": index }),
        Answer::Refused { status, message, refusal } => {
            serde_json::json!({ "status": status, "refusal": refusal, "message": message })
        }
    };
    println!("{}", serde_json::json!({ "intent": id.0.to_string(), "answer": line }));
    Ok(())
}

/// `release roll`'s vault step: the vault account, its script, and R's release folder with the release key.
pub struct VaultStep<'a> {
    pub account: &'a str,
    pub script: &'a str,
    pub release: &'a Path,
    pub release_key: &'a Path,
}

/// `release roll R`'s vault step, once every code intent through `McpRoll` is done for R ("Install operations", Release
/// roll, step 4). It is no group intent, because graft-ops cannot reach the vault account (`decisions/DECISIONS.md`,
/// "`vault-roll`"). Under the vault account's own token (`token`, never an install token, BF-47's fail line) it reads
/// the release the vault script serves, and runs `vault upgrade` with R's release folder when that is not R. With no
/// `--vault-account` or no token, it reports the step owed and the roll goes on. Answers the line `release roll`
/// prints.
fn vault_step(t: &InstallToml, r: ReleaseId, vault: Option<&VaultStep>, creds: &Creds) -> Res<Value> {
    let rid = r.0.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let owed = |why: &str| serde_json::json!({ "vault": "owed", "release": rid, "why": why });
    let Some(v) = vault else { return Ok(owed("no --vault-account: run graftyard vault upgrade --release R")) };
    let Some(token) = creds.vault.filter(|t| !t.is_empty()) else {
        return Ok(owed(&format!("{} is not set", crate::vault::TOKEN_VAR)));
    };
    if v.account == t.account {
        return Err(format!(
            "--vault-account is the install's account {}: the vault lives in its own account",
            t.account
        ));
    }
    if creds.install == Some(token) {
        return Err(format!(
            "{} is the install's token: the vault upgrades only under its own",
            crate::vault::TOKEN_VAR
        ));
    }
    let cf = crate::cf::Cf::with(creds.api, v.account, token);
    if crate::vault::serving(&cf, v.script)?.as_deref() == Some(rid.as_str()) {
        return Ok(serde_json::json!({ "vault": "at", "release": rid }));
    }
    let record = fs::read(v.release.join("release.json")).map_err(|e| format!("{}: {e}", v.release.display()))?;
    use sha2::Digest as _;
    if sha2::Sha256::digest(&record).as_slice() != r.0 {
        return Err(format!("{} is not release {rid}'s folder", v.release.display()));
    }
    let release = crate::vault::Release { dir: v.release, release_key: v.release_key };
    crate::vault::upgrade_with(&cf, &crate::vault::UpgradeArgs { account: v.account, script: v.script, release })?;
    Ok(serde_json::json!({ "vault": "upgraded", "release": rid }))
}

#[cfg(test)]
mod tests {
    use graftyard_release::{Format, FormatSet, ReleaseFold, sign_record};
    use serde_json::json;
    use sha2::Digest as _;

    use super::*;
    use crate::genesis::sign;
    use crate::genesis::tests::{install_toml, key_file, make_genesis, scratch, serve};

    const CUSTOMER: [u8; 32] = [7; 32];

    /// A `/view` answer of `entries` with their times, as a member gives it.
    fn view(entries: &[(Vec<u8>, Timestamp)]) -> Value {
        let e: Vec<Value> = entries.iter().map(|(b, at)| json!([crate::config::hex(b), at.0])).collect();
        json!({ "count": e.len(), "entries": e, "pending": [] })
    }

    /// `release roll` reads the group's entries, signs the first unfinished intent with its undo to the release the
    /// site runs, binds it to the last digest, and posts it; the edge's group accepts it. A file with a changed entry
    /// is refused before anything is signed.
    #[test]
    fn release_roll_signs_the_next_intent_on_the_last_digest_and_posts_it() {
        let dir = scratch();
        let config = install_toml(&dir, &CUSTOMER);
        let unsigned = make_genesis(&dir, &config, "genesis.unsigned");
        let ckey = key_file(&dir, "customer.key", &CUSTOMER, false);
        let signed = dir.join("genesis.signed");
        sign(&unsigned, &signed, &ckey, None).unwrap();
        let t = InstallToml::load(&config).unwrap();
        let anchor = t.anchor().unwrap();
        let mut fold = ReleaseFold::default();
        let mut log: Vec<(Vec<u8>, Timestamp)> = Vec::new();
        let mut at = Timestamp(1_790_000_000_000);
        let mut commit = |fold: &mut ReleaseFold, b: Vec<u8>, log: &mut Vec<_>| {
            at = Timestamp(at.0 + 60_000);
            let ev = fold.decide(&anchor, at, &b).unwrap();
            fold.apply(&ev, at);
            log.push((b, at));
        };
        commit(&mut fold, fs::read(&signed).unwrap(), &mut log);
        // Release R's upload, signed by the executor key `graftyard genesis` made; offline keys are lost, so a
        // `Keys` names a known executor first.
        let exec = [5u8; 32];
        let mut k = fold.keys.clone().unwrap();
        k.generation += 1;
        k.executor = graftyard_release::public_key(&exec);
        let bound =
            |f: &ReleaseFold| Bound { install: anchor.install, group: ChainId::Release, prev: f.last, parent: None };
        let mut e = SignedEntry::unsigned(bound(&fold), ReleaseEntry::Keys(Box::new(k)));
        e.sign(&CUSTOMER);
        commit(&mut fold, e.encode(), &mut log);
        let reads = FormatSet::new([Format { family: 32, version: 1 }]);
        let rec = format!(
            r#"{{"files":[{{"name":"formats.txt","sha256":"{}"}}]}}"#,
            crate::config::hex(&sha2::Sha256::digest(reads.file().as_bytes()))
        )
        .into_bytes();
        let rid = ReleaseId(sha2::Sha256::digest(&rec).into());
        let up = ReleaseUploaded {
            release: rid,
            record_sig: sign_record(&[3; 32], &rec),
            record: rec.into(),
            versions: (0..4).map(|s| (Script::Site(SiteId(s)), graftyard_release::VersionId([s + 1; 16]))).collect(),
            images: smallvec::SmallVec::new(),
            reads,
        };
        let mut e = SignedEntry::unsigned(bound(&fold), ReleaseEntry::ReleaseUploaded(Box::new(up)));
        e.sign(&exec);
        commit(&mut fold, e.encode(), &mut log);
        let entries = dir.join("view.json");
        fs::write(&entries, serde_json::to_vec(&view(&log)).unwrap()).unwrap();

        let last = fold.last;
        let (url, seen) = serve(1, move |s| {
            let e = SignedEntry::decode(&s.body).unwrap();
            assert_eq!(e.bound.prev, last, "bound to the group's last digest");
            match fold.decide(&anchor, Timestamp(at.0 + 60_000), &s.body) {
                Ok(ev) => (201, json!({ "index": 9, "digest": ev.digest().hex() }).to_string()),
                Err(e) => (422, json!({ "message": e.to_string(), "refusal": e.variant() }).to_string()),
            }
        });
        roll(
            &config,
            &rid.0.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            &entries,
            &ckey,
            3_600,
            None,
            Some(&url),
            None,
        )
        .unwrap();
        let s = seen.recv().unwrap();
        let e = SignedEntry::decode(&s.body).unwrap();
        let ReleaseEntry::Intent(i) = e.body else { panic!("not an intent") };
        assert_eq!(i.intent, graftyard_release::Intent::SiteDay { target: DayTarget::Site(SiteId(0)), release: rid });
        assert!(i.undo.is_empty(), "no site reports a release yet, so there is no N to roll back to");

        // One entry changed in the file: the replay refuses it, and nothing is signed.
        let mut bad = log.clone();
        bad[2].0[40] ^= 1;
        fs::write(&entries, serde_json::to_vec(&view(&bad)).unwrap()).unwrap();
        let r = roll(&config, &"00".repeat(32), &entries, &ckey, 3_600, Some(&dir.join("x")), None, None);
        assert!(r.is_err(), "{r:?}");
        assert!(!dir.join("x").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// BF-47's vault step through `release roll`: once R's code intents are done, the roll reads the vault script's
    /// release under the vault account's token and upgrades it to R's verified `vault/`, keeping its bindings; with no
    /// vault account or no token it prints the step owed and signs nothing new; it refuses the install's token or
    /// account, and never calls the API for them.
    #[test]
    fn release_roll_upgrades_the_vault_under_its_own_token_once_the_code_intents_are_done() {
        use std::sync::{Arc, Mutex};
        let dir = scratch();
        let config = install_toml(&dir, &CUSTOMER);
        let unsigned = make_genesis(&dir, &config, "genesis.unsigned");
        let ckey = key_file(&dir, "customer.key", &CUSTOMER, false);
        let rkey = key_file(&dir, "release.pub", &[3; 32], true);
        let signed = dir.join("genesis.signed");
        sign(&unsigned, &signed, &ckey, None).unwrap();
        let t = InstallToml::load(&config).unwrap();
        let anchor = t.anchor().unwrap();
        let mut fold = ReleaseFold::default();
        let mut log: Vec<(Vec<u8>, Timestamp)> = Vec::new();
        let mut at = Timestamp(1_790_000_000_000);
        let mut commit = |fold: &mut ReleaseFold, b: Vec<u8>, log: &mut Vec<_>| {
            at = Timestamp(at.0 + 60_000);
            let ev = fold.decide(&anchor, at, &b).unwrap();
            fold.apply(&ev, at);
            log.push((b, at));
        };
        commit(&mut fold, fs::read(&signed).unwrap(), &mut log);
        let exec = [5u8; 32];
        let mut k = fold.keys.clone().unwrap();
        k.generation += 1;
        k.executor = graftyard_release::public_key(&exec);
        let bound =
            |f: &ReleaseFold| Bound { install: anchor.install, group: ChainId::Release, prev: f.last, parent: None };
        let mut e = SignedEntry::unsigned(bound(&fold), ReleaseEntry::Keys(Box::new(k)));
        e.sign(&CUSTOMER);
        commit(&mut fold, e.encode(), &mut log);
        // R's folder: formats.txt and the vault Worker, release.json signed by the genesis's release key. R names no
        // script graft-ops deploys, so no code intent is owed and the vault step is due at once.
        let reads = FormatSet::new([Format { family: 32, version: 1 }]);
        let rel = dir.join("r");
        fs::create_dir_all(rel.join("vault")).unwrap();
        let formats = reads.file();
        let files: [(&str, &[u8]); 3] = [
            ("formats.txt", formats.as_bytes()),
            ("vault/entry.js", b"export default {}"),
            ("vault/ab-index_bg.wasm", b"\0asm"),
        ];
        let mut listed = Vec::new();
        for (n, b) in files {
            fs::write(rel.join(n), b).unwrap();
            listed.push(json!({ "name": n, "sha256": crate::config::hex(&sha2::Sha256::digest(b)) }));
        }
        let rec = serde_json::to_vec(&json!({ "label": "rv", "files": listed })).unwrap();
        fs::write(rel.join("release.json"), &rec).unwrap();
        fs::write(rel.join("release.sig"), sign_record(&[3; 32], &rec).0).unwrap();
        let rid = ReleaseId(sha2::Sha256::digest(&rec).into());
        let rhex = crate::config::hex(&rid.0);
        let up = ReleaseUploaded {
            release: rid,
            record_sig: sign_record(&[3; 32], &rec),
            record: rec.clone().into(),
            versions: [(Script::Front, graftyard_release::VersionId([9; 16]))].into_iter().collect(),
            images: smallvec::SmallVec::new(),
            reads,
        };
        let mut e = SignedEntry::unsigned(bound(&fold), ReleaseEntry::ReleaseUploaded(Box::new(up)));
        e.sign(&exec);
        commit(&mut fold, e.encode(), &mut log);
        // FrontRoll is owed first: the vault step is not due, and nothing reaches the vault account.
        let entries = dir.join("view.json");
        fs::write(&entries, serde_json::to_vec(&view(&log)).unwrap()).unwrap();
        let none = Creds { vault: Some("vault-token"), install: Some("install-token"), api: "http://127.0.0.1:9" };
        let vs = VaultStep { account: "vaultacct", script: "graft-vault", release: &rel, release_key: &rkey };
        let out1 = dir.join("i1");
        roll_in(&config, &rhex, &entries, &ckey, 3_600, Some(&out1), None, Some(&vs), &none).unwrap();
        let e1 = SignedEntry::decode(&fs::read(&out1).unwrap()).unwrap();
        assert!(
            matches!(&e1.body, ReleaseEntry::Intent(i) if matches!(i.intent, graftyard_release::Intent::FrontRoll { .. }))
        );
        // FrontRoll done.
        commit(&mut fold, e1.encode(), &mut log);
        let ReleaseEntry::Intent(i) = &e1.body else { unreachable!() };
        for step in 0..2 {
            let mut e = SignedEntry::unsigned(bound(&fold), ReleaseEntry::IntentStep { intent: i.id, step });
            e.sign(&exec);
            commit(&mut fold, e.encode(), &mut log);
        }
        let mut e = SignedEntry::unsigned(bound(&fold), ReleaseEntry::IntentDone { intent: i.id });
        e.sign(&exec);
        commit(&mut fold, e.encode(), &mut log);
        fs::write(&entries, serde_json::to_vec(&view(&log)).unwrap()).unwrap();
        let r = replay(&t, &entries).unwrap();
        assert!(graftyard_release::ops::vault_due(&r.fold, rid));
        // `release roll` itself takes the step once it is due: under the install's token it stops there.
        let same = Creds { vault: Some("install-token"), install: Some("install-token"), api: "http://127.0.0.1:9" };
        let err = roll_in(&config, &rhex, &entries, &ckey, 3_600, Some(&dir.join("i2")), None, Some(&vs), &same);
        assert!(err.unwrap_err().contains("install's token"));
        let step = |vault: Option<&VaultStep>, creds: &Creds| vault_step(&t, rid, vault, creds);
        // Owed: no vault account, or no vault token. No API call (the API is unreachable).
        let v = step(None, &none).unwrap();
        assert_eq!(v["vault"], json!("owed"));
        let no_token = Creds { vault: None, ..none };
        assert_eq!(step(Some(&vs), &no_token).unwrap()["vault"], json!("owed"));
        // The install's token or account: refused before any call.
        assert!(step(Some(&vs), &same).unwrap_err().contains("install's token"));
        let acct = VaultStep { account: &t.account, ..vs };
        assert!(step(Some(&acct), &none).unwrap_err().contains("install's account"));
        // Under its own token: N is served, so it uploads R's vault/ keeping the bindings and deploys it; then R is
        // served and the step leaves it.
        let state = Arc::new(Mutex::new(false));
        let auth = Arc::new(Mutex::new(Vec::new()));
        let (st, au, rh) = (state.clone(), auth.clone(), rhex.clone());
        let (url, seen) = serve(10, move |s| {
            let p = s.line.split(' ').nth(1).unwrap_or("").to_string();
            au.lock().unwrap().push(s.headers.iter().find(|(k, _)| k == "authorization").map(|(_, v)| v.clone()));
            let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
            let v = if *st.lock().unwrap() { "new" } else { "old" };
            match (s.line.split(' ').next().unwrap(), p.as_str()) {
                ("GET", "/accounts/vaultacct/workers/scripts/graft-vault/deployments") => ok(json!({ "deployments": [{
                    "versions": [{ "version_id": v, "percentage": 100 }], "created_on": "2026-11-23T00:00:00Z" }] })),
                ("GET", "/accounts/vaultacct/workers/scripts/graft-vault/versions/old") => {
                    ok(json!({ "annotations": { "workers/tag": "aa" } }))
                }
                ("GET", "/accounts/vaultacct/workers/scripts/graft-vault/versions/new") => {
                    ok(json!({ "annotations": { "workers/tag": rh } }))
                }
                ("POST", "/accounts/vaultacct/workers/scripts/graft-vault/versions") => ok(json!({ "id": "new" })),
                ("POST", "/accounts/vaultacct/workers/scripts/graft-vault/deployments") => {
                    *st.lock().unwrap() = true;
                    ok(json!({}))
                }
                (m, p) => panic!("unexpected {m} {p}"),
            }
        });
        let own = Creds { vault: Some("vault-token"), install: Some("install-token"), api: &url };
        assert_eq!(step(Some(&vs), &own).unwrap()["vault"], json!("upgraded"));
        assert_eq!(step(Some(&vs), &own).unwrap()["vault"], json!("at"));
        let mut calls = Vec::new();
        while let Ok(s) = seen.try_recv() {
            calls.push(s);
        }
        let upload = calls.iter().find(|s| s.line.starts_with("POST") && s.line.contains("/versions ")).unwrap();
        let text = String::from_utf8_lossy(&upload.body);
        assert!(text.contains(r#""keep_bindings":["r2_bucket","plain_text","secret_text"]"#), "{text:.300}");
        assert!(text.contains(&rhex) && text.contains("filename=\"entry.js\""));
        assert!(
            auth.lock().unwrap().iter().all(|a| a.as_deref() == Some("Bearer vault-token")),
            "only the vault's token"
        );
        // Another release's folder for R: refused before any upload.
        let other = dir.join("o");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("release.json"), b"{}").unwrap();
        let (url2, _rx2) = serve(2, |s| {
            let r = if s.line.contains("/versions/") {
                json!({ "annotations": { "workers/tag": "aa" } })
            } else {
                json!({ "deployments": [{ "versions": [{ "version_id": "old", "percentage": 100 }] }] })
            };
            (200, json!({ "success": true, "result": r }).to_string())
        });
        let own2 = Creds { api: &url2, ..own };
        let wrong = VaultStep { release: &other, ..vs };
        let err = step(Some(&wrong), &own2).unwrap_err();
        assert!(err.contains("is not release"), "{err}");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// `release entry`'s `Rotate` intent names its secret by `SecretName` and its slot, 0 or 1; any other is refused.
    #[test]
    fn a_rotate_intent_names_its_secret_and_slot() {
        let i = intent(&serde_json::json!({ "Rotate": { "secret": "MintWrapKey", "slot": 1 } })).unwrap();
        assert_eq!(i, Intent::Rotate { secret: SecretName::MintWrapKey, slot: 1 });
        let i = intent(&serde_json::json!({ "Rotate": { "secret": "ReadToken", "slot": 0 } })).unwrap();
        assert_eq!(i, Intent::Rotate { secret: SecretName::ReadToken, slot: 0 });
        assert!(intent(&serde_json::json!({ "Rotate": { "secret": "DeployToken", "slot": 2 } })).is_err());
        assert!(intent(&serde_json::json!({ "Rotate": { "secret": "Deploy", "slot": 1 } })).is_err());
    }
}
