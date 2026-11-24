//! `graftyard apps init` (`design/AGENT-LAYER.md`, Shape, "First-party Apps"; "Deployment modes", "Enable agents",
//! step 1; `build/BUILD-PLAN.md`, AM2-WP1): registers `Graftyard MCP` and `graftyard-insights` in the install account
//! and stores their keys, the MCP client secret, and `graft-mcp`'s sealing key in the install's Secrets Store.
//!
//! No operator route exists yet (`decisions/OPEN-QUESTIONS.md`, R14), so, as `admin bootstrap` does, the command
//! writes the commands each leader decides as request files. The App ids are the slug claims' answers, so it runs in
//! two steps, as `tests/am1/rest/apps.mjs` sends them:
//!
//! - `--host <host>` draws an RSA-2048 key per App, the MCP client secret, and the sealing key, writes all four to
//!   Secrets Store and nowhere else, then writes `apps.json` (public halves and hashes only) and the two `ClaimSlug`
//!   requests. It refuses when `apps.json` exists, so a rerun never replaces a registered App's key.
//!   - `--store <id>` writes to that store instead of finding or creating `graft-ops-<install>`. Secrets Store allows
//!     one store and 100 secrets per account (`research/cloudflare.md`), so installs that share an account share it.
//!   - `--secret-prefix <p>` puts `<p>` before each of the four names, so two installs in one store do not collide.
//!   - `--mcp-origin <url>` registers `<url>/callback` instead of `https://mcp.<host>/callback`, for a `graft-mcp`
//!     served somewhere other than `mcp.<host>` (a test install's workers.dev name, before AM2-WP3's routing).
//!
//!   Without them it behaves as before. `apps.json` records the prefix and origin when they are set.
//! - `--app-ids <mcp>,<insights>` reads `apps.json` and writes, per App, `PutApp`, `RegisterApp`, `RegisterClient`,
//!   and for `Graftyard MCP` the `Mcp` `AgentApp` row (`PutAgentApp`). It needs no network.
//!
//! The install account's `Authz` chain is opened by `install --genesis` (`workers/edge/src/release.rs`), so no `Open`
//! is planned here.

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use graftyard_identity::appauth::secret_hash;
use graftyard_identity::{
    Access, AgentApp, AgentKind, AppId, AppKey, AppRecord, AuthzCmd, AuthzOp, ClientId, NamesCmd, NamesOp,
    PermissionSet, RsaPublic, SecretHash, Slug, Url, install_account,
};
use graftyard_types::{ChainId, CredentialShard, DbId, Timestamp};
use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey as _, EncodePublicKey as _, LineEnding};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use smallvec::smallvec;

use crate::cf::Cf;
use crate::config::{InstallToml, Res, hex, now, random, unhex};

/// Slot A of each rotatable secret. "Install operations", Rotation runbook, binds two slots of each secret at
/// install; these names and the `_A`/`_B` spelling are this command's [inference: the design names neither].
pub const MCP_APP_KEY: &str = "MCP_APP_KEY_A";
pub const INSIGHTS_APP_KEY: &str = "INSIGHTS_APP_KEY_A";
pub const MCP_CLIENT_SECRET: &str = "MCP_CLIENT_SECRET_A";
/// `graft-mcp`'s AES-256-GCM sealing key for `McpToken` and `McpRefresh`, 32 bytes as 64 hex characters
/// (`build/BUILD-PLAN.md`, AM2-WP1, "The sealing key in Secrets Store").
pub const MCP_SEAL_KEY: &str = "MCP_SEAL_KEY_A";

const STATE: &str = "apps.json";
const CLAIMS: &str = "apps-claims.request.json";
const REGISTER: &str = "apps-register.request.json";

/// One row of AGENT-LAYER's "First-party Apps" table, plus the implied `metadata: read`.
struct AppSpec {
    slug: &'static str,
    name: &'static str,
    permissions: &'static [(&'static str, Access)],
    /// `Graftyard MCP`: an `Mcp` `AgentApp`, with a client secret and the `https://mcp.<host>/callback` callback.
    mcp: bool,
    key_secret: &'static str,
}

const APPS: [AppSpec; 2] = [
    AppSpec {
        slug: "graftyard-mcp",
        name: "Graftyard MCP",
        permissions: &[
            ("contents", Access::Write),
            ("issues", Access::Write),
            ("pull_requests", Access::Write),
            ("checks", Access::Read),
            ("statuses", Access::Read),
            ("actions", Access::Read),
            ("metadata", Access::Read),
        ],
        mcp: true,
        key_secret: MCP_APP_KEY,
    },
    AppSpec {
        slug: "graftyard-insights",
        name: "graftyard-insights",
        permissions: &[
            ("checks", Access::Write),
            ("issues", Access::Write),
            ("pull_requests", Access::Read),
            ("metadata", Access::Read),
        ],
        mcp: false,
        key_secret: INSIGHTS_APP_KEY,
    },
];

/// `apps.json`: what step 2 needs, and nothing secret.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub host: String,
    /// `--mcp-origin`: where `graft-mcp` is served when not at `https://mcp.<host>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_origin: Option<String>,
    /// `--secret-prefix`: what each Secrets Store name starts with, for the deploy that binds them.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_prefix: String,
    pub apps: Vec<SavedApp>,
}

/// Step 1's choices beyond the host; the default is the command as it was before the flags.
#[derive(Clone, Debug, Default)]
pub struct InitOpts {
    pub store: Option<String>,
    pub secret_prefix: String,
    pub mcp_origin: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedApp {
    pub slug: String,
    /// DER `SubjectPublicKeyInfo`, hex.
    pub public_key: String,
    /// The key's `AppKey.created`, ms since the epoch.
    pub key_created: u64,
    /// SHA-256 of the client secret, hex; `Graftyard MCP` only.
    pub client_secret_sha256: Option<String>,
}

/// One planned command and the chain whose leader decides it.
#[derive(Clone, Debug, PartialEq)]
pub enum Planned {
    Authz(ChainId, AuthzCmd),
    Names(ChainId, NamesCmd),
}

impl Planned {
    pub(crate) fn names(op: NamesOp) -> Planned {
        let cmd = NamesCmd { cred: CredentialShard::NONE, op };
        Planned::Names(ChainId::Names(cmd.shard().expect("an App op names its shard")), cmd)
    }

    pub(crate) fn authz(op: AuthzOp) -> Planned {
        Planned::Authz(ChainId::Authz(install_account()), AuthzCmd { cred: CredentialShard::NONE, op })
    }

    pub(crate) fn request(&self, op: &str, note: &str) -> Value {
        match self {
            Planned::Authz(chain, cmd) => {
                json!({ "kind": "graftyard.authz.command", "chain": chain, "op": op, "cmd": cmd, "note": note })
            }
            Planned::Names(chain, cmd) => {
                json!({ "kind": "graftyard.names.command", "chain": chain, "op": op, "cmd": cmd, "note": note })
            }
        }
    }
}

pub(crate) fn check_host(host: &str) -> Res<()> {
    let ok = !host.is_empty() && host.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.');
    if ok { Ok(()) } else { Err(format!("--host {host:?}: a bare hostname such as git.example.com")) }
}

pub(crate) fn check_prefix(p: &str) -> Res<()> {
    let ok = p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
    if ok { Ok(()) } else { Err(format!("--secret-prefix {p:?}: letters, digits, '-' and '_' only")) }
}

/// An `https` origin with nothing after the host: `<origin>/callback` is the callback.
fn check_origin(o: &str) -> Res<()> {
    let ok = o.strip_prefix("https://").is_some_and(|h| check_host(h).is_ok());
    if ok { Ok(()) } else { Err(format!("--mcp-origin {o:?}: an https origin such as https://mcp.git.example.com")) }
}

/// `Graftyard MCP`'s callback: `graft-mcp`'s `/callback`, at `https://mcp.<host>` unless `--mcp-origin` moved it.
fn callback(saved: &Saved) -> String {
    match &saved.mcp_origin {
        Some(o) => format!("{o}/callback"),
        None => format!("https://mcp.{}/callback", saved.host),
    }
}

pub(crate) fn slug(s: &str) -> Slug {
    Slug::parse(s).expect("the table's slugs parse")
}

/// The two `ClaimSlug` commands, as the install account. Each answers `NamesOut::App(id)`.
pub fn claims() -> Vec<Planned> {
    APPS.iter().map(|a| Planned::names(NamesOp::ClaimSlug { slug: slug(a.slug), owner: install_account() })).collect()
}

/// The `AppRecord` of one App, from its claimed id and `apps.json`.
fn record(spec: &AppSpec, id: AppId, callback: &str, saved: &SavedApp) -> Res<AppRecord> {
    let public = unhex(&saved.public_key).ok_or_else(|| format!("{STATE}: {}'s public_key is not hex", spec.slug))?;
    let client_secrets = match (&saved.client_secret_sha256, spec.mcp) {
        (Some(h), true) => {
            let h: [u8; 32] = unhex(h)
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| format!("{STATE}: {}'s client_secret_sha256 is not 32 bytes of hex", spec.slug))?;
            smallvec![SecretHash(h)]
        }
        (None, false) => smallvec![],
        _ => return Err(format!("{STATE}: {} has a client secret hash only if it is Graftyard MCP", spec.slug)),
    };
    Ok(AppRecord {
        id,
        slug: slug(spec.slug),
        name: spec.name.into(),
        client: ClientId::for_app(id),
        // Key id 1, as `tests/am1/rest/apps.mjs` writes it [inference].
        keys: smallvec![AppKey {
            id: DbId(1),
            public: RsaPublic(public.into()),
            created: Timestamp(saved.key_created)
        }],
        permissions: PermissionSet(spec.permissions.iter().map(|(k, a)| ((*k).into(), *a)).collect()),
        events: smallvec![],
        webhook: None,
        webhook_secret: None,
        client_secrets,
        callbacks: if spec.mcp { smallvec![Url(callback.into())] } else { smallvec![] },
        public: true,
    })
}

/// Pure: every command after the slug claims, per App in the table's order. `ids` are the claims' answers in the
/// same order: `Graftyard MCP`, then `graftyard-insights`.
pub fn register_commands(saved: &Saved, ids: [AppId; 2]) -> Res<Vec<(String, Planned)>> {
    check_host(&saved.host)?;
    if let Some(o) = &saved.mcp_origin {
        check_origin(o)?;
    }
    let owner = install_account();
    let mut out = Vec::new();
    for (spec, id) in APPS.iter().zip(ids) {
        let s = saved.apps.iter().find(|a| a.slug == spec.slug).ok_or_else(|| format!("{STATE}: no {}", spec.slug))?;
        let rec = record(spec, id, &callback(saved), s)?;
        let client = rec.client;
        let tag = |op: &str| format!("{op} ({})", spec.slug);
        out.push((tag("PutApp"), Planned::authz(AuthzOp::PutApp { record: Box::new(rec) })));
        out.push((tag("RegisterApp"), Planned::names(NamesOp::RegisterApp { app: id, owner })));
        out.push((tag("RegisterClient"), Planned::names(NamesOp::RegisterClient { client, owner, app: Some(id) })));
        if spec.mcp {
            let row = AgentApp { app: id, kind: AgentKind::Mcp, confine: None };
            out.push((tag("PutAgentApp"), Planned::authz(AuthzOp::PutAgentApp(row))));
        }
    }
    Ok(out)
}

pub(crate) fn write_new(path: &Path, text: &str, mode: u32) -> Res<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

pub(crate) fn write_requests(path: &Path, v: &[Value]) -> Res<()> {
    std::fs::write(path, serde_json::to_string_pretty(v).expect("json") + "\n")
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub fn init(config: &Path, host: &str, opts: &InitOpts, out: &Path) -> Res<()> {
    let t = InstallToml::load(config)?;
    init_with(&Cf::from_env(&t.account)?, &t, host, opts, out)
}

fn init_with(cf: &Cf, t: &InstallToml, host: &str, opts: &InitOpts, out: &Path) -> Res<()> {
    check_host(host)?;
    check_prefix(&opts.secret_prefix)?;
    if let Some(o) = &opts.mcp_origin {
        check_origin(o)?;
    }
    let state = out.join(STATE);
    if state.exists() {
        return Err(format!(
            "{} exists: the Apps' keys are drawn once; run with --app-ids to write the registration",
            state.display()
        ));
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    // The install's store, as `graftyard genesis` names it (`cli/src/genesis.rs`), unless `--store` names one.
    let (store_name, store) = match &opts.store {
        Some(id) => (format!("store {id}"), id.clone()),
        None => {
            let name = format!("graft-ops-{}", t.install.to_ascii_lowercase());
            let id = cf.store(&name)?;
            (name, id)
        }
    };
    let name = |n: &str| format!("{}{n}", opts.secret_prefix);
    let created = now().0;
    let mut apps = Vec::new();
    for spec in &APPS {
        let key = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).map_err(|e| format!("RSA keygen: {e}"))?;
        let pem = key.to_pkcs8_pem(LineEnding::LF).map_err(|e| format!("PKCS#8: {e}"))?;
        let der = key.to_public_key().to_public_key_der().map_err(|e| format!("SPKI: {e}"))?;
        let comment = format!("graftyard apps init: the {} App's RS256 key, PKCS#8 PEM", spec.slug);
        cf.put_secret(&store, &name(spec.key_secret), &pem, &comment)?;
        println!("  {}: written to Secrets Store {store_name} ({store}); no copy kept", name(spec.key_secret));
        let client_secret_sha256 = if spec.mcp {
            // 40 hex characters, as GitHub's client secrets are [inference].
            let secret = hex(&random::<20>());
            let n = name(MCP_CLIENT_SECRET);
            cf.put_secret(&store, &n, &secret, "graftyard apps init: Graftyard MCP's client secret")?;
            println!("  {n}: written to Secrets Store {store_name}");
            Some(hex(&secret_hash(&secret).0))
        } else {
            None
        };
        apps.push(SavedApp {
            slug: spec.slug.into(),
            public_key: hex(der.as_bytes()),
            key_created: created,
            client_secret_sha256,
        });
    }
    let seal = hex(&random::<32>());
    let n = name(MCP_SEAL_KEY);
    cf.put_secret(&store, &n, &seal, "graftyard apps init: graft-mcp's AES-256-GCM sealing key, hex")?;
    println!("  {n}: written to Secrets Store {store_name}");
    let saved = Saved {
        host: host.into(),
        mcp_origin: opts.mcp_origin.clone(),
        secret_prefix: opts.secret_prefix.clone(),
        apps,
    };
    write_new(&state, &(serde_json::to_string_pretty(&saved).expect("json") + "\n"), 0o644)?;
    let note = "Commit as the operator on `chain`. The answer, `NamesOut::App(id)`, is the App's id.";
    let reqs: Vec<Value> =
        claims().iter().zip(&APPS).map(|(p, a)| p.request(&format!("ClaimSlug ({})", a.slug), note)).collect();
    let claims_path = out.join(CLAIMS);
    write_requests(&claims_path, &reqs)?;
    println!("wrote {} (public keys and the client secret's hash only)", state.display());
    println!(
        "no operator route yet (R14): send the two ClaimSlug requests in {}, then run `graftyard apps init \
         --app-ids <graftyard-mcp id>,<graftyard-insights id> --out {}`",
        claims_path.display(),
        out.display()
    );
    Ok(())
}

pub fn register(out: &Path, ids: &[i64]) -> Res<()> {
    let [mcp, insights] = ids else {
        return Err("--app-ids takes two ids: graftyard-mcp's, then graftyard-insights'".into());
    };
    let state = out.join(STATE);
    let text = std::fs::read_to_string(&state)
        .map_err(|e| format!("{}: {e} (run `graftyard apps init --host <host>` first)", state.display()))?;
    let saved: Saved = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", state.display()))?;
    let cmds = register_commands(&saved, [AppId(DbId(*mcp)), AppId(DbId(*insights))])?;
    let note = "Commit as the operator on `chain`, in this file's order: each step reads the one before it.";
    let reqs: Vec<Value> = cmds.iter().map(|(op, p)| p.request(op, note)).collect();
    let path = out.join(REGISTER);
    write_requests(&path, &reqs)?;
    println!(
        "wrote {} commands to {}: graftyard-mcp is App {mcp}, graftyard-insights App {insights}",
        reqs.len(),
        path.display()
    );
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use graftyard_identity::{AccountKind, AuthzFold, IdBlock, IdentityDep, NameKey, NamesFold, NamesOut, name_shard};
    use graftyard_raft::{Command, Fold};
    use graftyard_types::NameShard;
    use rsa::RsaPublicKey;
    use rsa::pkcs8::{DecodePrivateKey as _, DecodePublicKey as _};

    use super::*;
    use crate::config::mint_install;
    use crate::genesis::tests::serve;

    pub(crate) fn install() -> InstallToml {
        InstallToml {
            install: mint_install().to_string(),
            account: "be2a".into(),
            zone: "git.acme.example".into(),
            customer_key: graftyard_release::public_key(&[9; 32]).hex(),
            evaluation: false,
            genesis: None,
            sites: Vec::new(),
            rtt: Vec::new(),
            default_placement: None,
        }
    }

    /// Runs step 1 against a Secrets Store stub whose stores are `st0` (`other`) and `st1` (the install's). Returns
    /// the output folder and every secret POSTed, by name, with the path it went to.
    fn step_one(opts: &InitOpts) -> (std::path::PathBuf, BTreeMap<String, (String, String)>) {
        let out = std::env::temp_dir().join(format!("graftyard-apps-{}", hex(&random::<6>())));
        let t = install();
        let store = format!("graft-ops-{}", t.install.to_ascii_lowercase());
        let posted = Arc::new(Mutex::new(BTreeMap::new()));
        let log = posted.clone();
        // One store lookup unless --store named it, then a search and a create for each of the four secrets.
        let (url, _rx) = serve(if opts.store.is_some() { 8 } else { 9 }, move |s| {
            let (m, path) = (s.line.split(' ').next().unwrap(), s.line.split(' ').nth(1).unwrap());
            let path = path.strip_prefix("/accounts/be2a").unwrap().to_string();
            let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
            match (m, path.as_str()) {
                ("GET", "/secrets_store/stores") => {
                    // `step_one` serves 8 requests with `--store`, so a lookup there would starve a secret's.
                    ok(json!([{ "id": "st0", "name": "other" }, { "id": "st1", "name": store }]))
                }
                ("GET", p) if p.contains("/secrets?search=") => ok(json!([])),
                ("POST", p) if p.ends_with("/secrets") => {
                    let v: Value = serde_json::from_slice(&s.body).unwrap();
                    let name = v[0]["name"].as_str().unwrap().to_string();
                    log.lock().unwrap().insert(name, (path.clone(), v[0]["value"].as_str().unwrap().to_string()));
                    ok(json!([{ "id": "x" }]))
                }
                _ => (404, json!({ "success": false, "errors": [{ "code": 1, "message": path }] }).to_string()),
            }
        });
        init_with(&Cf::with(&url, "be2a", "t"), &t, "git.acme.example", opts, &out).unwrap();
        let posted = posted.lock().unwrap().clone();
        (out, posted)
    }

    pub(crate) fn files(dir: &Path) -> Vec<(std::path::PathBuf, String)> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                let text = std::fs::read_to_string(&p).unwrap();
                (p, text)
            })
            .collect()
    }

    #[test]
    fn init_stores_four_secrets_in_the_install_store_and_writes_none_of_them_to_disk() {
        let (out, posted) = step_one(&InitOpts::default());
        let names: Vec<&str> = posted.keys().map(String::as_str).collect();
        assert_eq!(names, ["INSIGHTS_APP_KEY_A", "MCP_APP_KEY_A", "MCP_CLIENT_SECRET_A", "MCP_SEAL_KEY_A"]);
        for (name, (path, _)) in &posted {
            assert_eq!(path, "/secrets_store/stores/st1/secrets", "{name} went to the install's store");
        }
        let v = |n: &str| posted[n].1.as_str();
        for k in [MCP_APP_KEY, INSIGHTS_APP_KEY] {
            assert!(v(k).starts_with("-----BEGIN PRIVATE KEY-----"), "{k} is PKCS#8 PEM");
            RsaPrivateKey::from_pkcs8_pem(v(k)).unwrap();
        }
        assert_ne!(v(MCP_APP_KEY), v(INSIGHTS_APP_KEY), "each App has its own key");
        assert!(v(MCP_CLIENT_SECRET).len() == 40 && unhex(v(MCP_CLIENT_SECRET)).is_some());
        assert!(v(MCP_SEAL_KEY).len() == 64 && unhex(v(MCP_SEAL_KEY)).is_some());
        // No secret, nor any line of a key's PEM body, in anything written under --out.
        let written = files(&out);
        let mut names: Vec<String> =
            written.iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into()).collect();
        names.sort();
        assert_eq!(names, [CLAIMS, STATE]);
        for (p, text) in &written {
            for (name, (_, value)) in &posted {
                assert!(!text.contains(value.as_str()), "{name} in {}", p.display());
                for line in value.lines().filter(|l| !l.starts_with("-----")) {
                    assert!(!text.contains(line), "a line of {name} in {}", p.display());
                }
            }
        }
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(out.join(STATE)).unwrap().permissions().mode() & 0o777, 0o644);
        // The claims: one per App, as the install account, each on its slug's shard.
        let reqs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(out.join(CLAIMS)).unwrap()).unwrap();
        assert_eq!(reqs.len(), 2);
        for (r, a) in reqs.iter().zip(&APPS) {
            let cmd: NamesCmd = serde_json::from_value(r["cmd"].clone()).unwrap();
            assert_eq!(cmd.op, NamesOp::ClaimSlug { slug: slug(a.slug), owner: install_account() });
            assert!(cmd.cred.is_operator());
            let chain = ChainId::Names(name_shard(&NameKey::AppSlug(slug(a.slug))));
            assert_eq!((r["kind"].as_str(), &r["chain"]), (Some("graftyard.names.command"), &json!(chain)));
        }
        // A rerun refuses before it draws or writes anything: the stub served its nine requests and is gone.
        let cf = Cf::with("http://127.0.0.1:9", "be2a", "t");
        let again = init_with(&cf, &install(), "git.acme.example", &InitOpts::default(), &out);
        assert!(again.unwrap_err().contains("apps.json exists"));
        std::fs::remove_dir_all(&out).unwrap();
    }

    /// Decides `cmd` as its leader does and applies its events.
    fn decide<C: Command<Dep = IdentityDep>>(fold: &mut C::Fold, cmd: &C, deps: &[(ChainId, IdentityDep)]) -> C::Out {
        let s = fold.scratch();
        let (evs, out) = cmd.decide(&s, deps).unwrap_or_else(|e| panic!("{e:?}"));
        drop(s);
        for e in &evs {
            fold.apply(e);
        }
        out
    }

    /// The install account's `Authz` leader and every `Names` shard leader, each given exactly the folds its
    /// command's `deps` name, as a leader reads them through `fresh`.
    pub(crate) struct Leaders {
        pub(crate) names: BTreeMap<NameShard, NamesFold>,
        pub(crate) authz: AuthzFold,
    }

    impl Leaders {
        fn shard(&mut self, s: NameShard) -> &mut NamesFold {
            self.names.entry(s).or_insert_with(|| {
                let mut f = NamesFold::new(s);
                // A distinct id block per shard, as `Ids` leases them.
                f.apply_id_block(IdBlock::whole(0, 10 + u32::from(s.0)).unwrap());
                f
            })
        }

        fn deps(&mut self, chains: &[ChainId]) -> Vec<(ChainId, IdentityDep)> {
            let deps = chains.iter().map(|c| match *c {
                ChainId::Names(s) => (*c, IdentityDep::Names(self.shard(s).clone())),
                ChainId::Authz(a) if a == install_account() => (*c, IdentityDep::Authz(self.authz.clone())),
                other => panic!("an unexpected dep {other}"),
            });
            deps.collect()
        }

        pub(crate) fn names(&mut self, cmd: &NamesCmd) -> NamesOut {
            let s = cmd.shard().expect("a keyed op");
            let chains = cmd.deps(self.shard(s));
            let d = self.deps(&chains);
            decide(self.shard(s), cmd, &d)
        }

        pub(crate) fn authz(&mut self, cmd: &AuthzCmd) {
            let chains = cmd.deps(&self.authz);
            let d = self.deps(&chains);
            decide(&mut self.authz, cmd, &d);
        }
    }

    /// Step 1, the claims committed for their answers, then step 2, whose every command commits in file order on the
    /// chain the request names and leaves records that match what Secrets Store holds.
    #[test]
    fn app_ids_write_a_registration_that_commits_and_matches_the_stored_secrets() {
        let (out, posted) = step_one(&InitOpts::default());
        let mut l = Leaders { names: BTreeMap::new(), authz: AuthzFold::new(install_account()) };
        // `install --genesis` opens the install account's chain.
        l.authz(&AuthzCmd {
            cred: CredentialShard::NONE,
            op: AuthzOp::Open { kind: AccountKind::Org, enterprise: None },
        });
        let chain = |r: &Value| -> ChainId { serde_json::from_value(r["chain"].clone()).unwrap() };
        let claims: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(out.join(CLAIMS)).unwrap()).unwrap();
        let mut ids = Vec::new();
        for r in &claims {
            let cmd: NamesCmd = serde_json::from_value(r["cmd"].clone()).unwrap();
            let NamesOut::App(id) = l.names(&cmd) else { panic!("an App id") };
            ids.push(id);
        }
        let (mcp, insights) = (ids[0], ids[1]);
        assert_ne!(mcp, insights);
        register(&out, &[mcp.0.0, insights.0.0]).unwrap();
        assert!(register(&out, &[mcp.0.0]).is_err(), "two ids");

        let reqs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(out.join(REGISTER)).unwrap()).unwrap();
        let ops: Vec<&str> = reqs.iter().map(|r| r["op"].as_str().unwrap()).collect();
        assert_eq!(
            ops,
            [
                "PutApp (graftyard-mcp)",
                "RegisterApp (graftyard-mcp)",
                "RegisterClient (graftyard-mcp)",
                "PutAgentApp (graftyard-mcp)",
                "PutApp (graftyard-insights)",
                "RegisterApp (graftyard-insights)",
                "RegisterClient (graftyard-insights)",
            ]
        );
        let mut records = BTreeMap::new();
        let mut agent_rows = Vec::new();
        for r in &reqs {
            match r["kind"].as_str().unwrap() {
                "graftyard.authz.command" => {
                    assert_eq!(chain(r), ChainId::Authz(install_account()));
                    let cmd: AuthzCmd = serde_json::from_value(r["cmd"].clone()).unwrap();
                    match &cmd.op {
                        AuthzOp::PutApp { record } => {
                            records.insert(record.id, (**record).clone());
                        }
                        AuthzOp::PutAgentApp(row) => agent_rows.push(row.clone()),
                        op => panic!("{op:?}"),
                    }
                    l.authz(&cmd);
                }
                "graftyard.names.command" => {
                    let cmd: NamesCmd = serde_json::from_value(r["cmd"].clone()).unwrap();
                    assert_eq!(chain(r), ChainId::Names(cmd.shard().unwrap()), "on its key's shard");
                    if let NamesOp::RegisterClient { client, app, .. } = &cmd.op {
                        assert_eq!(Some(*client), app.map(ClientId::for_app));
                    }
                    l.names(&cmd);
                }
                k => panic!("{k}"),
            }
        }
        // The rows the leaders hold.
        use graftyard_identity::{AuthzRead as _, NamesRead as _};
        let (names, authz) = (&l.names, &l.authz);
        assert_eq!(authz.agent_app(mcp).map(|a| a.kind), Some(AgentKind::Mcp));
        assert_eq!(authz.agent_app(insights), None, "graftyard-insights is not an AgentApp");
        assert_eq!(agent_rows, [AgentApp { app: mcp, kind: AgentKind::Mcp, confine: None }]);
        for id in [mcp, insights] {
            let f = &names[&name_shard(&NameKey::Client(ClientId::for_app(id)))];
            assert!(f.client(&ClientId::for_app(id)).is_some());
            assert_eq!(names[&name_shard(&NameKey::App(id))].app_owner(id), Some(install_account()));
        }
        // The records: MCP's secret hash and keys are what Secrets Store holds.
        let m = &records[&mcp];
        assert_eq!(&*m.name, "Graftyard MCP");
        assert_eq!(m.client_secrets.as_slice(), [secret_hash(&posted[MCP_CLIENT_SECRET].1)]);
        assert_eq!(m.callbacks.as_slice(), [Url("https://mcp.git.acme.example/callback".into())]);
        assert_eq!(m.permissions.0.get("contents"), Some(&Access::Write));
        let i = &records[&insights];
        assert!(i.client_secrets.is_empty() && i.callbacks.is_empty());
        assert_eq!(i.permissions.0.get("checks"), Some(&Access::Write));
        assert_eq!(i.permissions.0.get("contents"), None);
        for (rec, secret) in [(m, MCP_APP_KEY), (i, INSIGHTS_APP_KEY)] {
            assert_eq!((rec.client, rec.public, rec.keys.len()), (ClientId::for_app(rec.id), true, 1));
            let public = RsaPublicKey::from_public_key_der(&rec.keys[0].public.0).unwrap();
            let private = RsaPrivateKey::from_pkcs8_pem(&posted[secret].1).unwrap();
            assert_eq!(public, private.to_public_key(), "{} signs with the key {secret} holds", rec.slug.as_str());
        }
        std::fs::remove_dir_all(&out).unwrap();
    }
    /// `--store`, `--secret-prefix` and `--mcp-origin`: the secrets go to the named store under the prefix, with no
    /// store lookup or create, and the registration's callback is the given origin's, not `mcp.<host>`'s.
    #[test]
    fn store_prefix_and_origin_move_the_secrets_and_the_callback() {
        let opts = InitOpts {
            store: Some("st0".into()),
            secret_prefix: "graft-m3f-".into(),
            mcp_origin: Some("https://graft-m3f-mcp.sub.workers.dev".into()),
        };
        let (out, posted) = step_one(&opts);
        let names: Vec<&str> = posted.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "graft-m3f-INSIGHTS_APP_KEY_A",
                "graft-m3f-MCP_APP_KEY_A",
                "graft-m3f-MCP_CLIENT_SECRET_A",
                "graft-m3f-MCP_SEAL_KEY_A"
            ]
        );
        assert!(posted.values().all(|(path, _)| path == "/secrets_store/stores/st0/secrets"), "the named store");
        register(&out, &[327680, 393216]).unwrap();
        let reqs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(out.join(REGISTER)).unwrap()).unwrap();
        let cmd: AuthzCmd = serde_json::from_value(reqs[0]["cmd"].clone()).unwrap();
        let AuthzOp::PutApp { record } = cmd.op else { panic!("PutApp first") };
        assert_eq!(record.id, AppId(DbId(327680)));
        assert_eq!(record.callbacks.as_slice(), [Url("https://graft-m3f-mcp.sub.workers.dev/callback".into())]);
        assert_eq!(record.client_secrets.as_slice(), [secret_hash(&posted["graft-m3f-MCP_CLIENT_SECRET_A"].1)]);
        let saved: Saved = serde_json::from_str(&std::fs::read_to_string(out.join(STATE)).unwrap()).unwrap();
        assert_eq!(saved.secret_prefix, "graft-m3f-");
        std::fs::remove_dir_all(&out).unwrap();
        // A bad prefix or origin refuses before anything is drawn.
        let cf = Cf::with("http://127.0.0.1:9", "be2a", "t");
        let bad = |o: InitOpts| init_with(&cf, &install(), "git.acme.example", &o, &out).unwrap_err();
        assert!(bad(InitOpts { secret_prefix: "a/b".into(), ..InitOpts::default() }).contains("--secret-prefix"));
        for o in ["http://x.example", "https://x.example/path", "https://"] {
            assert!(
                bad(InitOpts { mcp_origin: Some(o.into()), ..InitOpts::default() }).contains("--mcp-origin"),
                "{o}"
            );
        }
    }
}
