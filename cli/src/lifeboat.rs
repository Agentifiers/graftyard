//! `graftyard lifeboat init`, `enroll`, and `remove` (`design/DESIGN.md`, "Install operations", Lifeboat; Shape,
//! "Lifeboat feed"; `decisions/DECISIONS.md`, "`lifeboat-bundle`").
//!
//! - `init` makes the host's own Ed25519 key (seed in `<dir>/lifeboat.key`, mode 0600), signs `LifeboatAdd { host,
//!   key }` with the customer key against the group's last digest (a probe's 409 names it), posts it, and writes the
//!   bundle's `lifeboat.json`: the install's trust anchor (`GRAFTYARD_INSTALL`, `GRAFTYARD_CUSTOMER_KEY`), the same
//!   anchor the sites get, which the copy's `Release` learner accepts the genesis against; the signed genesis, which
//!   the front binds as an edge does (`GRAFTYARD_GENESIS_SIGNED`); the site table; the edge and host; and the bundle's
//!   `unique_key`, which its Durable Object ids derive from. `lifeboat.json` and `lifeboat.key` are the host's
//!   backed-up state.
//! - `enroll` calls `POST /api/graftyard/v1/lifeboat/tokens` with the developer's own token (read from a file, never
//!   argv) and writes the returned `lifeboat:read` token into gh's `hosts.yml` under the lifeboat host and a
//!   0600 file a git credential helper reads. The token is never printed.
//! - `remove` signs and posts `LifeboatRemove { host }` with the customer key.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use graftyard_release::{Bound, ReleaseEntry, SignedEntry, public_key};
use graftyard_types::{ChainId, Digest};
use serde_json::{Value, json};

use crate::config::{InstallToml, Res, hex, random, read_private_key};
use crate::genesis::{Answer, post_entry};

fn write_secret(path: &Path, text: &str) -> Res<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Signs `body` with `seed` against the group's last digest and posts it; a stale `prev` is retried once at the digest
/// the 409 names. Prints the answer.
fn post_signed(t: &InstallToml, edge: &str, body: ReleaseEntry, seed: &[u8; 32]) -> Res<Answer> {
    let install = t.anchor()?.install;
    let mut prev = Digest([0; 32]);
    for _ in 0..2 {
        let mut e = SignedEntry::unsigned(
            Bound { install, group: ChainId::Release, prev: Some(prev), parent: None },
            body.clone(),
        );
        e.sign(seed);
        match post_entry(edge, &e.encode())? {
            Answer::Stale { prev: p, .. } => {
                prev = Digest::parse_hex(&p).ok_or_else(|| format!("a 409 named prev {p:?}"))?;
            }
            a => return Ok(a),
        }
    }
    Err("the group's last digest moved twice; run again".into())
}

fn answer_line(a: &Answer) -> Value {
    match a {
        Answer::Created { index, digest } => json!({ "status": 201, "index": index, "digest": digest }),
        Answer::Stale { prev, index } => json!({ "status": 409, "prev": prev, "index": index }),
        Answer::Refused { status, message, refusal } => {
            json!({ "status": status, "refusal": refusal, "message": message })
        }
    }
}

pub struct InitArgs<'a> {
    pub host: &'a str,
    pub key: &'a Path,
    pub dir: &'a Path,
    pub edge: Option<&'a str>,
    /// The signed genesis the install committed, whose SHA-256 is install.toml's `genesis`.
    pub genesis: &'a Path,
}

pub fn init(config: &Path, a: InitArgs) -> Res<()> {
    let t = InstallToml::load(config)?;
    let edge = a.edge.map(String::from).unwrap_or_else(|| format!("https://{}", t.zone));
    let customer = read_private_key(a.key)?;
    fs::create_dir_all(a.dir).map_err(|e| format!("{}: {e}", a.dir.display()))?;
    let key_path = a.dir.join("lifeboat.key");
    let seed: [u8; 32] = if key_path.exists() {
        read_private_key(&key_path)?
    } else {
        let s = random::<32>();
        write_secret(&key_path, &hex(&s))?;
        s
    };
    let key = public_key(&seed);
    let a2 = post_signed(&t, &edge, ReleaseEntry::LifeboatAdd { host: a.host.into(), key }, &customer)?;
    println!("LifeboatAdd {} key {}: {}", a.host, key.hex(), answer_line(&a2));
    let registered = matches!(a2, Answer::Created { .. })
        || matches!(&a2, Answer::Refused { message, .. } if message.contains("already registered"));
    if !registered {
        return Err("the group did not register the lifeboat".into());
    }
    let anchor = t.anchor()?;
    // The genesis the sites hold, checked here against install.toml's committed digest and later, by the copy's
    // `Release` learner and the front, against the anchor.
    let genesis = fs::read(a.genesis).map_err(|e| format!("{}: {e}", a.genesis.display()))?;
    let digest = hex(&<sha2::Sha256 as sha2::Digest>::digest(&genesis));
    if t.genesis.as_deref() != Some(digest.as_str()) {
        return Err(format!("{} is not the genesis install.toml names", a.genesis.display()));
    }
    graftyard_release::accept_genesis(&anchor, &genesis).map_err(|e| format!("{}: {e}", a.genesis.display()))?;
    // Kept from an earlier init: the bundle's Durable Object ids derive from it, so a new one would lose the copy.
    let old: Value =
        fs::read(a.dir.join("lifeboat.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let unique = old["unique_key"].as_str().map(str::to_string).unwrap_or_else(|| hex(&random::<16>()));
    let cfg = json!({
        "host": a.host,
        "edge": edge,
        "install": anchor.install.to_string(),
        "customer_key": t.customer_key,
        "key": key.hex(),
        "sites": t.table(),
        "genesis_signed": hex(&genesis),
        "unique_key": unique,
    });
    let out = a.dir.join("lifeboat.json");
    fs::write(&out, serde_json::to_vec_pretty(&cfg).expect("json")).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("wrote {} and {} (0600)", out.display(), key_path.display());
    Ok(())
}

pub fn remove(config: &Path, host: &str, key: &Path, edge: Option<&str>) -> Res<()> {
    let t = InstallToml::load(config)?;
    let edge = edge.map(String::from).unwrap_or_else(|| format!("https://{}", t.zone));
    let a = post_signed(&t, &edge, ReleaseEntry::LifeboatRemove { host: host.into() }, &read_private_key(key)?)?;
    println!("LifeboatRemove {host}: {}", answer_line(&a));
    match a {
        Answer::Created { .. } => Ok(()),
        _ => Err("the group did not remove the lifeboat".into()),
    }
}

pub struct EnrollArgs<'a> {
    pub host: &'a str,
    pub edge: &'a str,
    /// A file holding the developer's own token for the main host.
    pub token_file: &'a Path,
    /// gh's config directory: its `hosts.yml` gets the lifeboat host's entry.
    pub gh_config: &'a Path,
    /// Where the `lifeboat:read` token is written (0600), for a git credential helper.
    pub out: &'a Path,
}

pub fn enroll(a: EnrollArgs) -> Res<()> {
    let mine = fs::read_to_string(a.token_file).map_err(|e| format!("{}: {e}", a.token_file.display()))?;
    let url = format!("{}/api/graftyard/v1/lifeboat/tokens", a.edge.trim_end_matches('/'));
    let r = ureq::post(&url)
        .set("authorization", &format!("token {}", mine.trim()))
        .set("user-agent", "graftyard-cli")
        .send_json(json!({ "host": a.host }));
    let (status, v): (u16, Value) = match r {
        Ok(r) => (r.status(), r.into_json().map_err(|e| e.to_string())?),
        Err(ureq::Error::Status(s, r)) => (s, r.into_json().unwrap_or(Value::Null)),
        Err(e) => return Err(format!("POST {url}: {e}")),
    };
    let Some(tok) = v["token"].as_str().filter(|_| status == 201) else {
        return Err(format!("{status}: {}", v["message"].as_str().unwrap_or("no token")));
    };
    write_secret(a.out, tok)?;
    fs::create_dir_all(a.gh_config).map_err(|e| format!("{}: {e}", a.gh_config.display()))?;
    let hosts = a.gh_config.join("hosts.yml");
    let old = fs::read_to_string(&hosts).unwrap_or_default();
    // Drops an earlier entry for this host (its block: the host line and its indented lines), then appends the new one.
    let mut kept = String::new();
    let mut skipping = false;
    for line in old.lines() {
        if !line.starts_with(' ') {
            skipping = line.trim_end_matches(':') == a.host;
        }
        if !skipping {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    kept.push_str(&format!("{}:\n    oauth_token: {tok}\n    git_protocol: https\n", a.host));
    write_secret(&hosts, &kept)?;
    println!(
        "enrolled on {}: token written to {} and {} (0600); the main host refuses it",
        a.host,
        a.out.display(),
        hosts.display()
    );
    Ok(())
}

/// What `bundle` reads beside `lifeboat.json`: the builds and where the host's own services listen.
pub struct BundleArgs<'a> {
    /// `lifeboat init`'s directory: `lifeboat.json`, `lifeboat.key`, and the feed's `feed.json` (`FEED_URL`,
    /// `FEED_BUCKET`, `FEED_KEY_ID`, `FEED_SECRET`, optional `FEED_REGION`), which the operator writes from the
    /// bucket-scoped R2 read token.
    pub dir: &'a Path,
    /// The release's `site/`, `edge/`, and `lifeboat/` builds (`worker-build --release` each).
    pub builds: &'a Path,
    /// The release the decoder reports: the SHA-256 of its `release.json`.
    pub release: &'a str,
    /// The front's listen address, and its TLS key and chain (PEM) if it serves HTTPS.
    pub listen: &'a str,
    pub tls: Option<(&'a Path, &'a Path)>,
    /// graft-blob's base URL.
    pub blob: &'a str,
    /// A localhost-only socket for the puller's `GET /status` and `POST /pass`, for the host's own checks.
    pub admin: Option<&'a str>,
    /// Where workerd keeps the copy: a directory created fresh for each workerd version.
    pub state: &'a Path,
    /// The release's pinned workerd binary, which `start.sh` runs.
    pub workerd: &'a str,
    pub out: &'a Path,
    /// The git copy, when the bundle runs one (`decisions/DECISIONS.md`, "`lifeboat-clone`").
    pub git: Option<GitArgs<'a>>,
}

/// `graft-git`: the release's `lifeboat-git` build (`usr/local` and `shard-server`, from `containers/git/Dockerfile`),
/// the directory its repositories live in, fresh for each git version, its loopback address, and graft-blob's root,
/// whose packs the free-disk floor counts.
pub struct GitArgs<'a> {
    pub build: &'a Path,
    pub data: &'a Path,
    pub listen: &'a str,
    pub blob_root: &'a Path,
}

/// The state directory's record of the git version whose repositories `--git-data` holds.
pub const GIT_VERSION_FILE: &str = "GIT_VERSION";
/// The free disk the git copy starts with, at least: this many times the repository bytes it rebuilds from, the
/// packs graft-blob holds and the copies' state [guess: one for the repositories, one for a repack's new pack beside
/// the old, one for growth] (`decisions/OPEN-QUESTIONS.md`, R64).
pub const GIT_DISK_TIMES: u64 = 3;

/// A loopback address, the only kind `graft-git` listens on: it answers anyone who reaches it, as a shard's container
/// answers its object.
fn loopback(listen: &str) -> bool {
    listen.starts_with("127.") || listen.starts_with("[::1]:") || listen.starts_with("localhost:")
}

/// `git --version` of the build's git, run from the build.
fn git_version(build: &Path) -> Res<String> {
    let bin = build.join("usr/local/bin/git");
    let o = std::process::Command::new(&bin)
        .arg("--version")
        .env("GIT_EXEC_PATH", build.join("usr/local/libexec/git-core"))
        .output()
        .map_err(|e| format!("{} --version: {e} (--git names the release's lifeboat-git build)", bin.display()))?;
    let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if !o.status.success() || v.is_empty() {
        return Err(format!("{} --version answered nothing", bin.display()));
    }
    Ok(v)
}

/// Cap'n Proto text for a string.
fn capnp(s: &str) -> String {
    serde_json::to_string(s).expect("json")
}

/// `to` as the config's `embed` takes it: relative to the config file's directory `from`, since capnp resolves an
/// absolute embed path against its import roots, not the filesystem.
fn relative(from: &Path, to: &Path) -> Res<String> {
    let abs = |p: &Path| fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()));
    let (from, to) = (abs(from)?, abs(to)?);
    let common = from.components().zip(to.components()).take_while(|(a, b)| a == b).count();
    let up = from.components().count() - common;
    let mut out: Vec<String> = std::iter::repeat_n("..".to_string(), up).collect();
    out.extend(to.components().skip(common).map(|c| c.as_os_str().to_string_lossy().into_owned()));
    Ok(out.join("/"))
}

/// `graftyard lifeboat bundle`: the workerd config of the bundle (`decisions/DECISIONS.md`, "`lifeboat-bundle`"). Three
/// Workers in one process: the site build as `member` (`GRAFTYARD_LIFEBOAT=1`, the class `Member` on `localDisk`
/// under `unique_key`), the edge build as `front` in lifeboat mode, the only socket, and the lifeboat build as `puller`
/// on its own alarm. The front reaches `MEMBER` and `PULLER` by binding; the puller reaches `MEMBER`, and the install
/// (the feed, the edge) and graft-blob over the network, with the system's CA store for HTTPS: workerd's `network`
/// has no TLS client until `tlsOptions` names one, so every HTTPS fetch to a real install failed ("this client has no
/// TLS configuration"; BF-53 on graft-m3f, which lifeboatbed's plain-HTTP front never reached). The copies and the
/// puller's state live under `state`; a new workerd version gets a fresh `state` and rebuilds from the feed.
pub fn bundle(a: BundleArgs) -> Res<()> {
    let read = |p: &Path| -> Res<Value> {
        serde_json::from_slice(&fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?)
            .map_err(|e| format!("{}: {e}", p.display()))
    };
    let cfg = read(&a.dir.join("lifeboat.json"))?;
    let feed = read(&a.dir.join("feed.json"))?;
    let seed = fs::read_to_string(a.dir.join("lifeboat.key")).map_err(|e| format!("lifeboat.key: {e}"))?;
    let s = |v: &Value, k: &str| v[k].as_str().map(str::to_string).ok_or_else(|| format!("no {k}"));
    let unique = s(&cfg, "unique_key")?;
    for b in ["site", "edge", "lifeboat"] {
        let p = a.builds.join(b).join("index.js");
        if !p.exists() {
            return Err(format!("{} is missing", p.display()));
        }
    }
    fs::create_dir_all(a.state).map_err(|e| format!("{}: {e}", a.state.display()))?;
    let version = workerd_version(a.workerd)?;
    pin_state(a.state, &version)?;
    let git = match &a.git {
        Some(g) => {
            if !loopback(g.listen) {
                return Err(format!("--git-listen {}: graft-git listens on a loopback address only", g.listen));
            }
            if !g.build.join("shard-server").exists() {
                return Err(format!("{}: no shard-server (the lifeboat-git build)", g.build.display()));
            }
            let v = git_version(g.build)?;
            fs::create_dir_all(g.data).map_err(|e| format!("{}: {e}", g.data.display()))?;
            pin(g.data, GIT_VERSION_FILE, &v, "repositories", "--git-data", "rebuilds from the copy")?;
            Some(v)
        }
        None => None,
    };

    let base = a.out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let embed = |p: &Path| relative(base, p).map(|r| format!("embed {}", capnp(&r)));
    let text = |name: &str, v: &str| format!("    (name = {}, text = {}),\n", capnp(name), capnp(v));
    let anchor =
        text("GRAFTYARD_INSTALL", &s(&cfg, "install")?) + &text("GRAFTYARD_CUSTOMER_KEY", &s(&cfg, "customer_key")?);
    let sites = text("GRAFTYARD_SITES", &cfg["sites"].to_string());
    let git_url = a.git.as_ref().map(|g| text("GIT_URL", &format!("http://{}", g.listen))).unwrap_or_default();
    let modules = |b: &str| -> Res<String> {
        let d = a.builds.join(b);
        Ok(format!(
            "  modules = [\n    (name = \"index.js\", esModule = {}),\n    (name = \"index_bg.wasm\", wasm = {}),\n  ],\n",
            embed(&d.join("index.js"))?,
            embed(&d.join("index_bg.wasm"))?
        ))
    };
    let compat = "  compatibilityDate = \"2026-10-01\",\n";
    let socket = match a.tls {
        Some((key, chain)) => format!(
            "https = (options = (), tlsOptions = (keypair = (privateKey = {}, certificateChain = {})))",
            embed(key)?,
            embed(chain)?
        ),
        None => "http = ()".into(),
    };
    let feed_vars = ["FEED_URL", "FEED_BUCKET"]
        .iter()
        .chain(feed.get("FEED_REGION").map(|_| &"FEED_REGION"))
        .map(|k| s(&feed, k).map(|v| text(k, &v)))
        .collect::<Res<String>>()?;
    let feed_secrets =
        ["FEED_KEY_ID", "FEED_SECRET"].iter().map(|k| s(&feed, k).map(|v| text(k, &v))).collect::<Res<String>>()?;
    let config = format!(
        r#"# Written by `graftyard lifeboat bundle`. Holds the host's key and the feed token: keep it 0600.
using Workerd = import "/workerd/workerd.capnp";
const config :Workerd.Config = (
  services = [
    (name = "front", worker = .front),
    (name = "member", worker = .member),
    (name = "puller", worker = .puller),
    (name = "state", disk = (path = {state}, writable = true)),
    (name = "internet", network = (allow = ["public", "private"], tlsOptions = (trustBrowserCas = true))),
  ],
  sockets = [ (name = "front", address = {listen}, {socket}, service = "front"){admin} ],
);
const member :Workerd.Worker = (
{member_modules}{compat}  bindings = [
    (name = "SITE_ID", text = "0"),
{sites}    (name = "GRAFTYARD_LIFEBOAT", text = "1"),
{anchor}    (name = "MEMBER", durableObjectNamespace = "Member"),
  ],
  durableObjectNamespaces = [ (className = "Member", uniqueKey = {member_key}, enableSql = true) ],
  durableObjectStorage = (localDisk = "state"),
);
const front :Workerd.Worker = (
{edge_modules}{compat}  bindings = [
{sites}    (name = "GRAFTYARD_LIFEBOAT", text = "1"),
{anchor}    (name = "GRAFTYARD_GENESIS_SIGNED", text = {genesis}),
    (name = "HOST", text = {host}),
    (name = "PLAN", text = "pro"),
    (name = "SSH_OPTION", text = "off"),
    (name = "MEMBER", durableObjectNamespace = (serviceName = "member", className = "Member")),
    (name = "PULLER", durableObjectNamespace = (serviceName = "puller", className = "Puller")),
{git_url}  ],
);
const puller :Workerd.Worker = (
{lifeboat_modules}{compat}  bindings = [
{feed_vars}{feed_secrets}    (name = "EDGE_URL", text = {edge}),
    (name = "LIFEBOAT_HOST", text = {host}),
    (name = "LIFEBOAT_KEY", text = {seed}),
    (name = "BLOB_URL", text = {blob}),
    (name = "LIFEBOAT_RELEASE", text = {release}),
{anchor}{git_url}    (name = "MEMBER", durableObjectNamespace = (serviceName = "member", className = "Member")),
    (name = "PULLER", durableObjectNamespace = "Puller"),
  ],
  durableObjectNamespaces = [ (className = "Puller", uniqueKey = {puller_key}) ],
  durableObjectStorage = (localDisk = "state"),
);
"#,
        state = capnp(&fs::canonicalize(a.state).map_err(|e| e.to_string())?.display().to_string()),
        listen = capnp(a.listen),
        member_modules = modules("site")?,
        edge_modules = modules("edge")?,
        lifeboat_modules = modules("lifeboat")?,
        member_key = capnp(&format!("{unique}-member")),
        puller_key = capnp(&format!("{unique}-puller")),
        genesis = capnp(&s(&cfg, "genesis_signed")?),
        host = capnp(&s(&cfg, "host")?),
        edge = capnp(&s(&cfg, "edge")?),
        seed = capnp(seed.trim()),
        blob = capnp(a.blob),
        release = capnp(a.release),
        admin = a
            .admin
            .map(|x| format!(",\n    (name = \"admin\", address = {}, http = (), service = \"puller\")", capnp(x)))
            .unwrap_or_default(),
    );
    write_secret(a.out, &config)?;
    let floor = open_files_floor(feed_chains(&feed)?);
    let state = fs::canonicalize(a.state).map_err(|e| e.to_string())?.display().to_string();
    let start = base.join("start.sh");
    let name = a.out.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("--out names no file")?;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o700)
        .open(&start)
        .and_then(|mut f| {
            let g = match (&a.git, &git) {
                (Some(g), Some(v)) => Some(GitStart {
                    build: canonical(g.build)?,
                    data: canonical(g.data)?,
                    listen: g.listen.to_string(),
                    blob_root: canonical(g.blob_root)?,
                    version: v.clone(),
                }),
                _ => None,
            };
            f.write_all(start_script(&name, &state, floor, a.workerd, &version, g.as_ref()).as_bytes())
        })
        .map_err(|e| format!("{}: {e}", start.display()))?;
    println!(
        "wrote {} (0600) and {}: start the bundle with it, which raises the open-file limit to at least {floor} and \
         runs only {version}",
        a.out.display(),
        start.display()
    );
    Ok(())
}

/// The state directory's record of the workerd version whose copies it holds (`design/DESIGN.md`, "Install
/// operations", Lifeboat: "A workerd version change rebuilds the lifeboat on a fresh directory").
pub const WORKERD_VERSION_FILE: &str = "WORKERD_VERSION";

/// `workerd --version`'s line, such as `workerd 2026-10-23`.
fn workerd_version(workerd: &str) -> Res<String> {
    let o = std::process::Command::new(workerd)
        .arg("--version")
        .output()
        .map_err(|e| format!("{workerd} --version: {e} (--workerd names the release's pinned workerd)"))?;
    let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if !o.status.success() || v.is_empty() {
        return Err(format!("{workerd} --version answered nothing"));
    }
    Ok(v)
}

/// Records `version` in `state`, or refuses a state another workerd version wrote: its copies are rebuilt on a fresh
/// directory, never opened by another version. A state from before the record (no file) is taken as this version's.
pub fn pin_state(state: &Path, version: &str) -> Res<()> {
    pin(state, WORKERD_VERSION_FILE, version, "copies", "--state", "rebuilds from the feed")
}

/// Records `version` in `dir`'s `file`, or refuses a directory another version wrote, naming both.
fn pin(dir: &Path, file: &str, version: &str, what: &str, flag: &str, then: &str) -> Res<()> {
    let p = dir.join(file);
    match fs::read_to_string(&p) {
        Ok(had) if had.trim() == version => Ok(()),
        Ok(had) => Err(format!(
            "{} holds {what} of {}, not {version}: give the new version a fresh {flag}, which {then}",
            dir.display(),
            had.trim()
        )),
        Err(_) => fs::write(&p, format!("{version}\n")).map_err(|e| format!("{}: {e}", p.display())),
    }
}

fn canonical(p: &Path) -> std::io::Result<String> {
    Ok(fs::canonicalize(p)?.display().to_string())
}

/// What `start.sh` needs to start `graft-git` (`GitArgs`, with absolute paths).
pub struct GitStart {
    pub build: String,
    pub data: String,
    pub listen: String,
    pub blob_root: String,
    pub version: String,
}

/// The fewest open files the bundle starts with: 4 for each chain the install's `export-state` names, at least
/// `OPEN_FILES_MIN` [guess]. Every chain's copy is a Durable Object with its own SQLite file, which workerd keeps open,
/// and at the inherited 1,024 every pass failed with `SQLITE_CANTOPEN` once about 1,000 chains were open (BF-53 on
/// graft-m3f, `tests/m6/results/bf53.md`).
pub const OPEN_FILES_MIN: u64 = 65_536;
pub const OPEN_FILES_PER_CHAIN: u64 = 4;

pub fn open_files_floor(chains: u64) -> u64 {
    (chains * OPEN_FILES_PER_CHAIN).max(OPEN_FILES_MIN)
}

/// The chains the install's `export-state` names, read with the feed token as the puller reads the feed: 0 before the
/// feed's first round wrote it.
fn feed_chains(feed: &Value) -> Res<u64> {
    let s = |k: &str| feed[k].as_str().map(str::to_string).ok_or_else(|| format!("feed.json: no {k}"));
    let s3 = graftyard_lifeboat::feed::S3 {
        endpoint: s("FEED_URL")?,
        bucket: s("FEED_BUCKET")?,
        key_id: s("FEED_KEY_ID")?,
        secret: s("FEED_SECRET")?,
        region: s("FEED_REGION").unwrap_or_else(|_| "auto".into()),
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?;
    let signed = s3.get(Some("export-state"), &[], now.as_millis() as u64);
    let mut req = ureq::get(&signed.url);
    for (k, v) in &signed.headers {
        req = req.set(k, v);
    }
    let state: Value = match req.call() {
        Ok(r) => r.into_json().map_err(|e| format!("export-state: {e}"))?,
        Err(ureq::Error::Status(404, _)) => return Ok(0),
        Err(e) => return Err(format!("export-state, for the open-file floor: {e}")),
    };
    Ok(state["cursors"].as_object().map_or(0, |c| c.len() as u64))
}

/// A shell word for `s`.
fn sh(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `start.sh`: refuses a workerd (`$WORKERD`, else the `workerd` the bundle was written with) other than the pinned
/// `version`, and a state directory another version wrote; raises the open-file limit to the hard limit, refuses to
/// start below `floor` or below 4 for each copy `state` holds, whichever is higher (a copy grows past the chains named
/// at bundle time); and runs workerd on the config.
fn start_script(config: &str, state: &str, floor: u64, workerd: &str, version: &str, git: Option<&GitStart>) -> String {
    let tail = match git {
        None => format!("exec \"$wd\" serve {} --experimental\n", sh(config)),
        Some(g) => git_start(config, state, g),
    };
    format!(
        r#"#!/bin/bash
# Written by `graftyard lifeboat bundle`: starts the bundle on its pinned workerd with an open-file limit its copies
# fit in.
cd "$(dirname "$0")" || exit 1
wd=${{WORKERD:-{workerd}}}
pin={version}
have=$("$wd" --version 2>/dev/null)
if [ "$have" != "$pin" ]; then
  echo "lifeboat: $wd is ${{have:-no workerd}}, not the bundle's pinned $pin: start it with the release's workerd" >&2
  exit 1
fi
held=$(cat {state}/{file} 2>/dev/null)
if [ "$held" != "$pin" ]; then
  echo "lifeboat: the state directory holds copies of ${{held:-an unknown workerd}}, not $pin: bundle the new version on a fresh --state, which rebuilds from the feed" >&2
  exit 1
fi
floor={floor}
copies=$(find {state} -name '*.sqlite' 2>/dev/null | wc -l)
[ $((copies * {per})) -gt $floor ] && floor=$((copies * {per}))
hard=$(ulimit -Hn)
[ "$hard" = unlimited ] && hard=$floor
ulimit -n "$hard" 2>/dev/null
have=$(ulimit -n)
if [ "$have" != unlimited ] && [ "$have" -lt "$floor" ]; then
  echo "lifeboat: the open-file limit is $have (hard $hard), under the bundle's floor of $floor: raise the hard limit (ulimit -Hn, LimitNOFILE= for systemd) and start again" >&2
  exit 1
fi
{tail}"#,
        per = OPEN_FILES_PER_CHAIN,
        state = sh(state),
        workerd = sh(workerd),
        version = sh(version),
        file = WORKERD_VERSION_FILE,
    )
}

/// `start.sh`'s git half: refuses a git other than the pinned one and a `--git-data` another version wrote, naming
/// both, and free disk under `GIT_DISK_TIMES` the repository bytes; then runs `graft-git` on its loopback address
/// beside workerd, and stops the other when either exits, so the host's supervisor restarts both.
fn git_start(config: &str, state: &str, g: &GitStart) -> String {
    format!(
        r#"gb={build}
gd={data}
export GIT_EXEC_PATH="$gb/usr/local/libexec/git-core"
gpin={version}
ghave=$("$gb/usr/local/bin/git" --version 2>/dev/null)
if [ "$ghave" != "$gpin" ]; then
  echo "lifeboat: the bundle's git is ${{ghave:-missing}}, not its pinned $gpin: bundle the release's lifeboat-git" >&2
  exit 1
fi
gheld=$(cat "$gd/{file}" 2>/dev/null)
if [ "$gheld" != "$gpin" ]; then
  echo "lifeboat: the git directory holds repositories of ${{gheld:-an unknown git}}, not $gpin: bundle the new version on a fresh --git-data, which rebuilds from the copy" >&2
  exit 1
fi
repo=$(du -sbc {blob}/pack {state} 2>/dev/null | tail -1 | cut -f1)
used=$(du -sb "$gd" | cut -f1)
free=$(df -B1 --output=avail "$gd" | tail -1 | tr -d ' ')
if [ $((free + used)) -lt $((repo * {times})) ]; then
  echo "lifeboat: the git directory has $((free + used)) bytes free or in use, under {times} x the $repo repository bytes it rebuilds from: give --git-data a larger disk" >&2
  exit 1
fi
GRAFT_LISTEN={listen} GRAFT_LIFEBOAT=1 GRAFT_DATA="$gd" PATH="$gb/usr/local/bin:$PATH" "$gb/shard-server" &
gp=$!
"$wd" serve {config} --experimental &
wp=$!
trap 'kill $gp $wp 2>/dev/null' EXIT INT TERM
wait -n $gp $wp
"#,
        build = sh(&g.build),
        data = sh(&g.data),
        version = sh(&g.version),
        file = GIT_VERSION_FILE,
        blob = sh(&g.blob_root),
        state = sh(state),
        times = GIT_DISK_TIMES,
        listen = sh(&g.listen),
        config = sh(config),
    )
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::{
        GIT_VERSION_FILE, GitStart, OPEN_FILES_MIN, WORKERD_VERSION_FILE, open_files_floor, pin_state, start_script,
    };

    const PIN: &str = "workerd 2026-10-23";

    /// The start script refuses a hard limit under the floor, counting the copies the state holds as well as the
    /// chains named at bundle time, and starts workerd with the limit raised to the hard limit when it fits. It runs
    /// only the pinned workerd, on a state directory that version wrote, and names both versions when it refuses.
    #[test]
    fn start_script_refuses_an_open_file_limit_under_the_floor_and_another_workerd() {
        assert_eq!(open_files_floor(10), OPEN_FILES_MIN);
        assert_eq!(open_files_floor(20_000), 80_000);
        let d = std::env::temp_dir().join(format!("lbstart-{}", std::process::id()));
        let state = d.join("st ate");
        std::fs::create_dir_all(state.join("x")).unwrap();
        pin_state(&state, PIN).unwrap();
        let copies = |n: usize| (0..n).for_each(|i| std::fs::write(state.join(format!("x/{i}.sqlite")), b"").unwrap());
        copies(5);
        let wd = d.join("wd").display().to_string();
        std::fs::write(
            d.join("start.sh"),
            start_script("config.capnp", &state.display().to_string(), 2_000, &wd, PIN, None),
        )
        .unwrap();
        // A stand-in workerd of version `$V`: prints the soft limit it was started with and its arguments.
        std::fs::write(
            d.join("wd"),
            "#!/bin/bash\n[ \"$1\" = --version ] && { echo \"workerd ${V:-2026-10-23}\"; exit 0; }\necho \"$(ulimit -n) $*\"\n",
        )
        .unwrap();
        std::fs::set_permissions(d.join("wd"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let run = |hard: u64, v: &str| {
            let o = Command::new("bash")
                .arg("-c")
                .arg(format!("ulimit -Sn 1024 && ulimit -Hn {hard} && bash ./start.sh"))
                .env("V", v)
                .env_remove("WORKERD")
                .current_dir(&d)
                .output()
                .unwrap();
            (
                o.status.success(),
                String::from_utf8_lossy(&o.stdout).into_owned(),
                String::from_utf8_lossy(&o.stderr).into_owned(),
            )
        };
        let (ok, out, err) = run(4_096, "2026-10-23");
        assert!(ok, "{err}");
        assert_eq!(out.trim(), "4096 serve config.capnp --experimental");
        let (ok, out, err) = run(1_999, "2026-10-23");
        assert!(!ok && out.is_empty());
        assert!(err.contains("under the bundle's floor of 2000"), "{err}");
        // 600 copies need 2,400, above the 2,000 named at bundle time.
        copies(600);
        let (ok, _, err) = run(2_100, "2026-10-23");
        assert!(!ok && err.contains("floor of 2400"), "{err}");
        // Another workerd on the path: refused, naming both versions, and never started.
        let (ok, out, err) = run(4_096, "2026-11-30");
        assert!(!ok && out.is_empty(), "{out}");
        assert!(err.contains("workerd 2026-11-30") && err.contains("pinned workerd 2026-10-23"), "{err}");
        // A state another version wrote: refused by the script and by the next bundle, naming both versions.
        std::fs::write(state.join(WORKERD_VERSION_FILE), "workerd 2026-09-01\n").unwrap();
        let (ok, out, err) = run(4_096, "2026-10-23");
        assert!(!ok && out.is_empty(), "{out}");
        assert!(err.contains("copies of workerd 2026-09-01, not workerd 2026-10-23"), "{err}");
        let e = pin_state(&state, PIN).unwrap_err();
        assert!(e.contains("copies of workerd 2026-09-01, not workerd 2026-10-23"), "{e}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// With a git build, the start script runs only the pinned git on a git directory that version wrote, and only
    /// with 3 times the repository bytes free there, naming both versions or the bytes when it refuses; then it
    /// starts `graft-git` on its address beside workerd.
    #[test]
    fn start_script_refuses_another_git_and_a_disk_under_the_floor() {
        let d = std::env::temp_dir().join(format!("lbgit-{}", std::process::id()));
        let (state, data, blob, gb) = (d.join("state"), d.join("git"), d.join("blob"), d.join("build"));
        for p in [&state, &data, &blob.join("pack"), &gb.join("usr/local/bin")] {
            std::fs::create_dir_all(p).unwrap();
        }
        pin_state(&state, PIN).unwrap();
        std::fs::write(data.join(GIT_VERSION_FILE), "git version 2.56.0\n").unwrap();
        let exe = |p: std::path::PathBuf, body: &str| {
            std::fs::write(&p, format!("#!/bin/bash\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        };
        exe(gb.join("usr/local/bin/git"), "echo \"git version ${G:-2.56.0}\"");
        exe(gb.join("shard-server"), "echo \"graft-git $GRAFT_LISTEN $GRAFT_LIFEBOAT\"");
        exe(d.join("wd"), "[ \"$1\" = --version ] && { echo 'workerd 2026-10-23'; exit 0; }; sleep 1; echo workerd");
        let g = GitStart {
            build: gb.display().to_string(),
            data: data.display().to_string(),
            listen: "127.0.0.1:8792".into(),
            blob_root: blob.display().to_string(),
            version: "git version 2.56.0".into(),
        };
        let wd = d.join("wd").display().to_string();
        std::fs::write(
            d.join("start.sh"),
            start_script("c.capnp", &state.display().to_string(), 2_000, &wd, PIN, Some(&g)),
        )
        .unwrap();
        let run = |g: &str| {
            let o = Command::new("bash")
                .arg("-c")
                .arg("ulimit -Sn 1024 && ulimit -Hn 4096 && bash ./start.sh")
                .env("G", g)
                .env_remove("WORKERD")
                .current_dir(&d)
                .output()
                .unwrap();
            (
                o.status.success(),
                String::from_utf8_lossy(&o.stdout).into_owned(),
                String::from_utf8_lossy(&o.stderr).into_owned(),
            )
        };
        let (ok, out, err) = run("2.56.0");
        assert!(ok, "{err}");
        assert!(out.contains("graft-git 127.0.0.1:8792 1"), "{out}");
        let (ok, out, err) = run("2.57.0");
        assert!(!ok && out.is_empty(), "{out}");
        assert!(err.contains("git version 2.57.0, not its pinned git version 2.56.0"), "{err}");
        std::fs::write(data.join(GIT_VERSION_FILE), "git version 2.55.0\n").unwrap();
        let (ok, out, err) = run("2.56.0");
        assert!(!ok && out.is_empty(), "{out}");
        assert!(err.contains("repositories of git version 2.55.0, not git version 2.56.0"), "{err}");
        std::fs::write(data.join(GIT_VERSION_FILE), "git version 2.56.0\n").unwrap();
        // Repository bytes past a third of the disk's free space: a sparse file is counted by `du -b` at its size.
        let free: u64 =
            String::from_utf8(Command::new("df").args(["-B1", "--output=avail"]).arg(&data).output().unwrap().stdout)
                .unwrap()
                .lines()
                .last()
                .unwrap()
                .trim()
                .parse()
                .unwrap();
        std::fs::File::create(blob.join("pack/big")).unwrap().set_len(free / 3 + (1 << 20)).unwrap();
        let (ok, out, err) = run("2.56.0");
        assert!(!ok && out.is_empty(), "{out}");
        assert!(err.contains("under 3 x the") && err.contains("give --git-data a larger disk"), "{err}");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
