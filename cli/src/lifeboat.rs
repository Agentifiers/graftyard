//! `graftyard lifeboat init`, `enroll`, and `remove` (`design/DESIGN.md`, "Install operations", Lifeboat; Shape,
//! "Lifeboat feed"; `decisions/DECISIONS.md`, "`lifeboat-bundle`").
//!
//! - `init` makes the host's own Ed25519 key (seed in `<dir>/lifeboat.key`, mode 0600), signs `LifeboatAdd { host,
//!   key }` with the customer key against the group's last digest (a probe's 409 names it), posts it, and writes the
//!   bundle's `lifeboat.json`: the install's trust anchor (`GRAFTYARD_INSTALL`, `GRAFTYARD_CUSTOMER_KEY`), the same
//!   anchor the sites get, which the copy's `Release` learner accepts the genesis against, and the edge and host.
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
    let cfg = json!({
        "host": a.host,
        "edge": edge,
        "install": anchor.install.to_string(),
        "customer_key": t.customer_key,
        "key": key.hex(),
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
