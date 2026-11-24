//! `graftyard agents init` (`design/AGENT-LAYER.md`, Shape, "First-party Apps"; "Deployment modes", "Enable agents",
//! step 2; journey A8; `build/BUILD-PLAN.md`, AM3-WP1): registers `graftyard-agent` and `graftyard-workspace`, both
//! `GraftyardRun` Apps owned by the install account, and stores their keys, `graftyard-agent`'s webhook secret, and the
//! agents executor key in the agents account's Secrets Store.
//!
//! It is `graftyard apps init` (`cli/src/apps.rs`) for the agents account, and runs in the same two steps, because the
//! App ids are the slug claims' answers and no operator route exists yet (`decisions/OPEN-QUESTIONS.md`, R14):
//!
//! - `--host <host>` draws an RSA-2048 key per App, a 32-byte webhook secret (64 hex characters), and a 32-byte
//!   Ed25519 seed for the agents executor key, writes all four to the agents account's Secrets Store and nowhere else,
//!   then writes `agents.json` (public halves only, and the executor's public key) and the two `ClaimSlug` requests.
//!   It refuses when `agents.json` exists, so a rerun never replaces a registered App's key. It prints the executor
//!   public key, which the customer adds as `Keys.agents_executor` with a signed `Keys` entry.
//!   - `--account <id>` is the agents account, which is not the install's (test installs use the runner account until
//!     go-live, `build/BUILD-PLAN.md`, AM3-WP1, "Account"). Without it the install.toml's account is used. The token
//!     is `CLOUDFLARE_API_TOKEN`, which must be that account's.
//!   - `--store <id>` writes to that store instead of finding or creating `graft-agents-<install>`.
//!   - `--secret-prefix <p>` puts `<p>` before each of the four names.
//!   - `--webhook-url <url>` is `graftyard-agent`'s webhook, the controller's hostname. Without it the App has none.
//! - `--app-ids <agent>,<workspace>` reads `agents.json` and writes, per App, `PutApp`, `RegisterApp`,
//!   `RegisterClient`, and its `GraftyardRun` `AgentApp` row (`PutAgentApp`). It needs no network.
//!
//! `PutApp` carries no webhook secret: `AppRecord.webhook_secret` holds ciphertext wrapped by the install's `graft-ops`,
//! which this command cannot reach. After the registration commits, the operator sets it with `PATCH /app/hook/config`
//! under `graftyard-agent`'s App JWT, sending the value of `AGENTS_WEBHOOK_SECRET_A` (`design/SHARED.md`, `PATCH
//! /app/hook/config`). The first-party `graftyard_task` delivery is not a subscribable event, so `events` does not list
//! it (`design/AGENT-LAYER.md`, "API reference"). Neither App has a client secret or a callback. Both are `public`, as
//! `apps init`'s are, because each org installs them on its own repositories and workspace org [inference: the table
//! names no visibility].
//!
//! Not done here: creating `graft-agents-front`, `graft-controller`, and the agents `graft-ops` with their
//! `AgentsAnchor` variables, and registering the webhook hostname in the agents account's zone (A8).

use std::path::Path;

use graftyard_identity::{
    Access, AgentApp, AgentKind, AppId, AppKey, AppRecord, AuthzOp, ClientId, EventKind, NamesOp, PermissionSet,
    RsaPublic, Url, install_account,
};
use graftyard_types::{DbId, Timestamp};
use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey as _, EncodePublicKey as _, LineEnding};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use smallvec::smallvec;

use crate::apps::{Planned, check_host, check_prefix, slug, write_new, write_requests};
use crate::cf::Cf;
use crate::config::{InstallToml, Res, hex, now, random, unhex};

/// Slot A of each rotatable secret, spelled as `apps init`'s [inference: the design names none of them].
pub const AGENTS_APP_KEY: &str = "AGENTS_APP_KEY_A";
pub const WORKSPACE_APP_KEY: &str = "WORKSPACE_APP_KEY_A";
/// `graftyard-agent`'s webhook secret, 32 bytes as 64 hex characters.
pub const AGENTS_WEBHOOK_SECRET: &str = "AGENTS_WEBHOOK_SECRET_A";
/// The agents `graft-ops`'s signing key, `Keys.agents_executor`: a 32-byte Ed25519 seed as 64 hex characters, as
/// `graftyard genesis` stores the executor's.
pub const AGENTS_EXECUTOR_KEY: &str = "AGENTS_EXECUTOR_KEY";

const STATE: &str = "agents.json";
const CLAIMS: &str = "agents-claims.request.json";
const REGISTER: &str = "agents-register.request.json";

/// One row of AGENT-LAYER's "First-party Apps" table, plus the implied `metadata: read`. Both rows are `GraftyardRun`.
struct AppSpec {
    slug: &'static str,
    permissions: &'static [(&'static str, Access)],
    events: &'static [&'static str],
    /// `graftyard-agent`: the App whose webhook is `--webhook-url` and whose secret is `AGENTS_WEBHOOK_SECRET`.
    hook: bool,
    key_secret: &'static str,
}

const APPS: [AppSpec; 2] = [
    AppSpec {
        slug: "graftyard-agent",
        permissions: &[
            ("contents", Access::Read),
            ("issues", Access::Write),
            ("pull_requests", Access::Write),
            ("checks", Access::Read),
            ("statuses", Access::Read),
            ("actions", Access::Read),
            ("metadata", Access::Read),
        ],
        events: &["issues", "issue_comment", "pull_request", "pull_request_review", "check_run", "status"],
        hook: true,
        key_secret: AGENTS_APP_KEY,
    },
    AppSpec {
        slug: "graftyard-workspace",
        permissions: &[("contents", Access::Write), ("metadata", Access::Read)],
        events: &[],
        hook: false,
        key_secret: WORKSPACE_APP_KEY,
    },
];

/// `agents.json`: what step 2 and the agents deploy need, and nothing secret.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub host: String,
    /// The Secrets Store id the four secrets went to.
    pub store: String,
    /// `--secret-prefix`: what each Secrets Store name starts with.
    pub secret_prefix: String,
    /// `--webhook-url`: `graftyard-agent`'s webhook.
    pub webhook_url: Option<String>,
    pub apps: Vec<SavedApp>,
    /// The agents executor's Ed25519 public key, 64 hex characters, for `Keys.agents_executor`.
    pub agents_executor: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedApp {
    pub slug: String,
    /// DER `SubjectPublicKeyInfo`, hex.
    pub public_key: String,
    /// The key's `AppKey.created`, ms since the epoch.
    pub key_created: u64,
}

/// Step 1's choices beyond the host.
#[derive(Clone, Debug, Default)]
pub struct InitOpts {
    pub store: Option<String>,
    pub secret_prefix: String,
    pub webhook_url: Option<String>,
}

/// An `https` URL whose host is a bare hostname and whose path, if any, is printable ASCII.
fn check_webhook(u: &str) -> Res<()> {
    let ok = u.strip_prefix("https://").is_some_and(|rest| {
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        check_host(host).is_ok() && path.bytes().all(|c| c.is_ascii_graphic())
    });
    if ok { Ok(()) } else { Err(format!("--webhook-url {u:?}: an https URL such as https://agents.example.com/hook")) }
}

/// The two `ClaimSlug` commands, as the install account. Each answers `NamesOut::App(id)`.
pub fn claims() -> Vec<Planned> {
    APPS.iter().map(|a| Planned::names(NamesOp::ClaimSlug { slug: slug(a.slug), owner: install_account() })).collect()
}

/// The `AppRecord` of one App, from its claimed id and `agents.json`.
fn record(spec: &AppSpec, id: AppId, webhook: Option<&str>, saved: &SavedApp) -> Res<AppRecord> {
    let public = unhex(&saved.public_key).ok_or_else(|| format!("{STATE}: {}'s public_key is not hex", spec.slug))?;
    Ok(AppRecord {
        id,
        slug: slug(spec.slug),
        name: spec.slug.into(),
        client: ClientId::for_app(id),
        keys: smallvec![AppKey {
            id: DbId(1),
            public: RsaPublic(public.into()),
            created: Timestamp(saved.key_created)
        }],
        permissions: PermissionSet(spec.permissions.iter().map(|(k, a)| ((*k).into(), *a)).collect()),
        events: spec.events.iter().map(|e| EventKind((*e).into())).collect(),
        webhook: webhook.filter(|_| spec.hook).map(|u| Url(u.into())),
        // Ciphertext from the install's `graft-ops`, set later by `PATCH /app/hook/config`.
        webhook_secret: None,
        client_secrets: smallvec![],
        callbacks: smallvec![],
        public: true,
    })
}

/// Pure: every command after the slug claims, per App in the table's order. `ids` are the claims' answers in the
/// same order: `graftyard-agent`, then `graftyard-workspace`.
pub fn register_commands(saved: &Saved, ids: [AppId; 2]) -> Res<Vec<(String, Planned)>> {
    check_host(&saved.host)?;
    if let Some(u) = &saved.webhook_url {
        check_webhook(u)?;
    }
    let owner = install_account();
    let mut out = Vec::new();
    for (spec, id) in APPS.iter().zip(ids) {
        let s = saved.apps.iter().find(|a| a.slug == spec.slug).ok_or_else(|| format!("{STATE}: no {}", spec.slug))?;
        let rec = record(spec, id, saved.webhook_url.as_deref(), s)?;
        let client = rec.client;
        let tag = |op: &str| format!("{op} ({})", spec.slug);
        out.push((tag("PutApp"), Planned::authz(AuthzOp::PutApp { record: Box::new(rec) })));
        out.push((tag("RegisterApp"), Planned::names(NamesOp::RegisterApp { app: id, owner })));
        out.push((tag("RegisterClient"), Planned::names(NamesOp::RegisterClient { client, owner, app: Some(id) })));
        let row = AgentApp { app: id, kind: AgentKind::GraftyardRun, confine: None };
        out.push((tag("PutAgentApp"), Planned::authz(AuthzOp::PutAgentApp(row))));
    }
    Ok(out)
}

/// Step 1 in the agents account: `account` overrides the install.toml's.
pub fn init(config: &Path, account: Option<&str>, host: &str, opts: &InitOpts, out: &Path) -> Res<()> {
    let t = InstallToml::load(config)?;
    init_with(&Cf::from_env(account.unwrap_or(&t.account))?, &t, host, opts, out)
}

fn init_with(cf: &Cf, t: &InstallToml, host: &str, opts: &InitOpts, out: &Path) -> Res<()> {
    check_host(host)?;
    check_prefix(&opts.secret_prefix)?;
    if let Some(u) = &opts.webhook_url {
        check_webhook(u)?;
    }
    let state = out.join(STATE);
    if state.exists() {
        return Err(format!(
            "{} exists: the Apps' keys are drawn once; run with --app-ids to write the registration",
            state.display()
        ));
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let (store_name, store) = match &opts.store {
        Some(id) => (format!("store {id}"), id.clone()),
        None => {
            let name = format!("graft-agents-{}", t.install.to_ascii_lowercase());
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
        let comment = format!("graftyard agents init: the {} App's RS256 key, PKCS#8 PEM", spec.slug);
        cf.put_secret(&store, &name(spec.key_secret), &pem, &comment)?;
        println!("  {}: written to Secrets Store {store_name} ({store}); no copy kept", name(spec.key_secret));
        apps.push(SavedApp { slug: spec.slug.into(), public_key: hex(der.as_bytes()), key_created: created });
    }
    let n = name(AGENTS_WEBHOOK_SECRET);
    let comment = "graftyard agents init: graftyard-agent's webhook secret, hex";
    cf.put_secret(&store, &n, &hex(&random::<32>()), comment)?;
    println!("  {n}: written to Secrets Store {store_name}");
    let seed: [u8; 32] = random();
    let n = name(AGENTS_EXECUTOR_KEY);
    cf.put_secret(&store, &n, &hex(&seed), "graftyard agents init: the agents executor's Ed25519 seed, hex")?;
    println!("  {n}: written to Secrets Store {store_name}; no copy kept");
    let saved = Saved {
        host: host.into(),
        store: store.clone(),
        secret_prefix: opts.secret_prefix.clone(),
        webhook_url: opts.webhook_url.clone(),
        apps,
        agents_executor: graftyard_release::public_key(&seed).hex(),
    };
    write_new(&state, &(serde_json::to_string_pretty(&saved).expect("json") + "\n"), 0o644)?;
    let note = "Commit as the operator on `chain`. The answer, `NamesOut::App(id)`, is the App's id.";
    let reqs: Vec<Value> =
        claims().iter().zip(&APPS).map(|(p, a)| p.request(&format!("ClaimSlug ({})", a.slug), note)).collect();
    let claims_path = out.join(CLAIMS);
    write_requests(&claims_path, &reqs)?;
    println!("wrote {} (public keys only)", state.display());
    println!("agents executor public key: {}", saved.agents_executor);
    println!(
        "the customer adds it as Keys.agents_executor with a signed Keys entry: `graftyard release entry` with \
         {{\"Keys\": {{\"agents_executor\": \"{}\"}}}} and --keys <latest genesis or Keys entry>, then `graftyard sign` \
         with the customer key and `graftyard release post`",
        saved.agents_executor
    );
    println!(
        "no operator route yet (R14): send the two ClaimSlug requests in {}, then run `graftyard agents init \
         --app-ids <graftyard-agent id>,<graftyard-workspace id> --out {}`",
        claims_path.display(),
        out.display()
    );
    Ok(())
}

pub fn register(out: &Path, ids: &[i64]) -> Res<()> {
    let [agent, workspace] = ids else {
        return Err("--app-ids takes two ids: graftyard-agent's, then graftyard-workspace's".into());
    };
    let state = out.join(STATE);
    let text = std::fs::read_to_string(&state)
        .map_err(|e| format!("{}: {e} (run `graftyard agents init --host <host>` first)", state.display()))?;
    let saved: Saved = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", state.display()))?;
    let cmds = register_commands(&saved, [AppId(DbId(*agent)), AppId(DbId(*workspace))])?;
    let note = "Commit as the operator on `chain`, in this file's order: each step reads the one before it.";
    let reqs: Vec<Value> = cmds.iter().map(|(op, p)| p.request(op, note)).collect();
    let path = out.join(REGISTER);
    write_requests(&path, &reqs)?;
    println!(
        "wrote {} commands to {}: graftyard-agent is App {agent}, graftyard-workspace App {workspace}",
        reqs.len(),
        path.display()
    );
    println!(
        "once they commit, set graftyard-agent's webhook secret: PATCH /app/hook/config under its App JWT with \
         {{\"secret\": <the value of {}{AGENTS_WEBHOOK_SECRET}>}}",
        saved.secret_prefix
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use graftyard_identity::{
        AccountKind, AuthzCmd, AuthzFold, AuthzRead as _, NameKey, NamesCmd, NamesOut, NamesRead as _, name_shard,
    };
    use graftyard_types::{ChainId, CredentialShard};
    use rsa::RsaPublicKey;
    use rsa::pkcs8::{DecodePrivateKey as _, DecodePublicKey as _};
    use serde_json::json;

    use super::*;
    use crate::apps::tests::{Leaders, files, install};
    use crate::genesis::tests::serve;

    /// Runs step 1 against a Secrets Store stub whose stores are `st0` (`other`) and `st1` (`graft-agents-<install>`).
    /// Returns the output folder and every secret POSTed, by name, with the path it went to.
    fn step_one(opts: &InitOpts) -> (std::path::PathBuf, BTreeMap<String, (String, String)>) {
        let out = std::env::temp_dir().join(format!("graftyard-agents-{}", hex(&random::<6>())));
        let t = install();
        let store = format!("graft-agents-{}", t.install.to_ascii_lowercase());
        let posted = Arc::new(Mutex::new(BTreeMap::new()));
        let log = posted.clone();
        // One store lookup unless --store named it, then a search and a create for each of the four secrets.
        let (url, _rx) = serve(if opts.store.is_some() { 8 } else { 9 }, move |s| {
            let (m, path) = (s.line.split(' ').next().unwrap(), s.line.split(' ').nth(1).unwrap());
            let path = path.strip_prefix("/accounts/g7").unwrap().to_string();
            let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
            match (m, path.as_str()) {
                ("GET", "/secrets_store/stores") => {
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
        init_with(&Cf::with(&url, "g7", "t"), &t, "git.acme.example", opts, &out).unwrap();
        let posted = posted.lock().unwrap().clone();
        (out, posted)
    }

    fn saved(out: &Path) -> Saved {
        serde_json::from_str(&std::fs::read_to_string(out.join(STATE)).unwrap()).unwrap()
    }

    #[test]
    fn init_stores_four_secrets_in_the_agents_store_and_writes_none_of_them_to_disk() {
        let (out, posted) = step_one(&InitOpts::default());
        let names: Vec<&str> = posted.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            ["AGENTS_APP_KEY_A", "AGENTS_EXECUTOR_KEY", "AGENTS_WEBHOOK_SECRET_A", "WORKSPACE_APP_KEY_A"]
        );
        for (name, (path, _)) in &posted {
            assert_eq!(path, "/secrets_store/stores/st1/secrets", "{name} went to graft-agents-<install>");
        }
        let v = |n: &str| posted[n].1.as_str();
        for k in [AGENTS_APP_KEY, WORKSPACE_APP_KEY] {
            assert!(v(k).starts_with("-----BEGIN PRIVATE KEY-----"), "{k} is PKCS#8 PEM");
            RsaPrivateKey::from_pkcs8_pem(v(k)).unwrap();
        }
        assert_ne!(v(AGENTS_APP_KEY), v(WORKSPACE_APP_KEY), "each App has its own key");
        for k in [AGENTS_WEBHOOK_SECRET, AGENTS_EXECUTOR_KEY] {
            assert!(v(k).len() == 64 && unhex(v(k)).is_some(), "{k} is 32 bytes of hex");
        }
        assert_ne!(v(AGENTS_WEBHOOK_SECRET), v(AGENTS_EXECUTOR_KEY));
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
        // agents.json: the executor's public half of the stored seed, and each App's public key.
        let s = saved(&out);
        let seed: [u8; 32] = unhex(v(AGENTS_EXECUTOR_KEY)).unwrap().try_into().unwrap();
        assert_eq!(s.agents_executor, graftyard_release::public_key(&seed).hex());
        assert_eq!((s.host.as_str(), s.store.as_str(), s.secret_prefix.as_str()), ("git.acme.example", "st1", ""));
        assert_eq!(s.webhook_url, None);
        let slugs: Vec<&str> = s.apps.iter().map(|a| a.slug.as_str()).collect();
        assert_eq!(slugs, ["graftyard-agent", "graftyard-workspace"]);
        for (a, k) in s.apps.iter().zip([AGENTS_APP_KEY, WORKSPACE_APP_KEY]) {
            let public = RsaPublicKey::from_public_key_der(&unhex(&a.public_key).unwrap()).unwrap();
            assert_eq!(public, RsaPrivateKey::from_pkcs8_pem(v(k)).unwrap().to_public_key(), "{}", a.slug);
        }
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(out.join(STATE)).unwrap().permissions().mode() & 0o777, 0o644);
        // The claims: one per App, as the install account, each on its slug's shard.
        let reqs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(out.join(CLAIMS)).unwrap()).unwrap();
        assert_eq!(reqs.len(), 2);
        for (r, a) in reqs.iter().zip(["graftyard-agent", "graftyard-workspace"]) {
            let cmd: NamesCmd = serde_json::from_value(r["cmd"].clone()).unwrap();
            assert_eq!(cmd.op, NamesOp::ClaimSlug { slug: slug(a), owner: install_account() });
            assert!(cmd.cred.is_operator());
            let chain = ChainId::Names(name_shard(&NameKey::AppSlug(slug(a))));
            assert_eq!((r["kind"].as_str(), &r["chain"]), (Some("graftyard.names.command"), &json!(chain)));
        }
        // A rerun refuses before it draws or writes anything: the stub served its nine requests and is gone.
        let snapshot = || files(&out).into_iter().collect::<BTreeMap<_, _>>();
        let before = snapshot();
        let cf = Cf::with("http://127.0.0.1:9", "g7", "t");
        let again = init_with(&cf, &install(), "git.acme.example", &InitOpts::default(), &out);
        assert!(again.unwrap_err().contains("agents.json exists"));
        assert_eq!(snapshot(), before, "the rerun wrote nothing");
        std::fs::remove_dir_all(&out).unwrap();
    }

    /// Step 1, the claims committed for their answers, then step 2, whose every command commits in file order on the
    /// chain the request names and leaves records and `GraftyardRun` rows that match what Secrets Store holds.
    #[test]
    fn app_ids_write_a_registration_that_commits_and_matches_the_stored_secrets() {
        let hook = "https://graft-agents-front.sub.workers.dev/webhook";
        let (out, posted) = step_one(&InitOpts { webhook_url: Some(hook.into()), ..InitOpts::default() });
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
        let (agent, workspace) = (ids[0], ids[1]);
        assert_ne!(agent, workspace);
        register(&out, &[agent.0.0, workspace.0.0]).unwrap();
        assert!(register(&out, &[agent.0.0]).is_err(), "two ids");

        let reqs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(out.join(REGISTER)).unwrap()).unwrap();
        let ops: Vec<&str> = reqs.iter().map(|r| r["op"].as_str().unwrap()).collect();
        assert_eq!(
            ops,
            [
                "PutApp (graftyard-agent)",
                "RegisterApp (graftyard-agent)",
                "RegisterClient (graftyard-agent)",
                "PutAgentApp (graftyard-agent)",
                "PutApp (graftyard-workspace)",
                "RegisterApp (graftyard-workspace)",
                "RegisterClient (graftyard-workspace)",
                "PutAgentApp (graftyard-workspace)",
            ]
        );
        let mut records = BTreeMap::new();
        for r in &reqs {
            match r["kind"].as_str().unwrap() {
                "graftyard.authz.command" => {
                    assert_eq!(chain(r), ChainId::Authz(install_account()));
                    let cmd: AuthzCmd = serde_json::from_value(r["cmd"].clone()).unwrap();
                    if let AuthzOp::PutApp { record } = &cmd.op {
                        records.insert(record.id, (**record).clone());
                    }
                    l.authz(&cmd);
                }
                "graftyard.names.command" => {
                    let cmd: NamesCmd = serde_json::from_value(r["cmd"].clone()).unwrap();
                    assert_eq!(chain(r), ChainId::Names(cmd.shard().unwrap()), "on its key's shard");
                    l.names(&cmd);
                }
                k => panic!("{k}"),
            }
        }
        // The rows the leaders hold: both Apps the install's, both `GraftyardRun` with no confine.
        let (names, authz) = (&l.names, &l.authz);
        for id in [agent, workspace] {
            assert_eq!(names[&name_shard(&NameKey::App(id))].app_owner(id), Some(install_account()));
            assert!(
                names[&name_shard(&NameKey::Client(ClientId::for_app(id)))].client(&ClientId::for_app(id)).is_some()
            );
            let row = authz.agent_app(id).unwrap();
            assert_eq!((row.kind, row.confine.as_ref()), (AgentKind::GraftyardRun, None));
            assert_eq!(authz.app(id), Some(&records[&id]), "the fold holds the planned record");
        }
        // The records: graftyard-agent reads contents and hears its six events at the webhook; graftyard-workspace
        // alone writes contents and hears nothing.
        let (a, w) = (&records[&agent], &records[&workspace]);
        assert_eq!((a.slug.as_str(), w.slug.as_str()), ("graftyard-agent", "graftyard-workspace"));
        let perms = |r: &AppRecord| -> Vec<(String, Access)> {
            r.permissions.0.iter().map(|(k, v)| (k.to_string(), *v)).collect()
        };
        let p =
            |v: &[(&str, Access)]| -> Vec<(String, Access)> { v.iter().map(|(k, a)| (k.to_string(), *a)).collect() };
        use Access::{Read, Write};
        assert_eq!(
            perms(a),
            p(&[
                ("actions", Read),
                ("checks", Read),
                ("contents", Read),
                ("issues", Write),
                ("metadata", Read),
                ("pull_requests", Write),
                ("statuses", Read),
            ])
        );
        assert_eq!(perms(w), p(&[("contents", Write), ("metadata", Read)]));
        let events: Vec<&str> = a.events.iter().map(|e| &*e.0).collect();
        assert_eq!(events, ["issues", "issue_comment", "pull_request", "pull_request_review", "check_run", "status"]);
        assert!(w.events.is_empty());
        assert_eq!(a.webhook, Some(Url(hook.into())));
        assert_eq!(w.webhook, None);
        for (rec, secret) in [(a, AGENTS_APP_KEY), (w, WORKSPACE_APP_KEY)] {
            assert_eq!(rec.webhook_secret, None, "set later through PATCH /app/hook/config");
            assert!(rec.client_secrets.is_empty() && rec.callbacks.is_empty());
            assert_eq!((rec.client, rec.public, rec.keys.len()), (ClientId::for_app(rec.id), true, 1));
            let public = RsaPublicKey::from_public_key_der(&rec.keys[0].public.0).unwrap();
            let private = RsaPrivateKey::from_pkcs8_pem(&posted[secret].1).unwrap();
            assert_eq!(public, private.to_public_key(), "{} signs with the key {secret} holds", rec.slug.as_str());
        }
        std::fs::remove_dir_all(&out).unwrap();
    }

    /// `--store` and `--secret-prefix`: the secrets go to the named store under the prefix, with no store lookup, and
    /// a bad prefix or webhook URL refuses before anything is drawn.
    #[test]
    fn store_and_prefix_move_the_secrets_and_bad_flags_refuse() {
        let opts = InitOpts { store: Some("st0".into()), secret_prefix: "graft-m3f-".into(), webhook_url: None };
        let (out, posted) = step_one(&opts);
        let names: Vec<&str> = posted.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "graft-m3f-AGENTS_APP_KEY_A",
                "graft-m3f-AGENTS_EXECUTOR_KEY",
                "graft-m3f-AGENTS_WEBHOOK_SECRET_A",
                "graft-m3f-WORKSPACE_APP_KEY_A"
            ]
        );
        assert!(posted.values().all(|(path, _)| path == "/secrets_store/stores/st0/secrets"), "the named store");
        let s = saved(&out);
        assert_eq!((s.store.as_str(), s.secret_prefix.as_str()), ("st0", "graft-m3f-"));
        std::fs::remove_dir_all(&out).unwrap();
        let cf = Cf::with("http://127.0.0.1:9", "g7", "t");
        let bad = |o: InitOpts| init_with(&cf, &install(), "git.acme.example", &o, &out).unwrap_err();
        assert!(bad(InitOpts { secret_prefix: "a/b".into(), ..InitOpts::default() }).contains("--secret-prefix"));
        for u in ["http://x.example/hook", "https://", "https://x.example/a b"] {
            let e = bad(InitOpts { webhook_url: Some(u.into()), ..InitOpts::default() });
            assert!(e.contains("--webhook-url"), "{u}: {e}");
        }
        assert!(!out.exists(), "nothing written");
    }
}
