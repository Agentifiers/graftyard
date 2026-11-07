//! `graftyard genesis`, `graftyard sign`, and `graftyard install --genesis` (`design/DESIGN.md`, "Install
//! operations", Genesis steps 3 to 5; Shape, "Release group").

use std::fs;
use std::io::Read as _;
use std::path::Path;

use graftyard_identity::validate_record;
use graftyard_release::{
    Bound, Genesis, KeyId, Keys, ReleaseEntry, SignedEntry, accept_genesis, check_genesis, public_key,
};
use graftyard_types::{ChainId, MemberKind, PlacementChecks, PlacementName, PlacementRecord, SiteId, SystemChain};
use serde_json::Value;
use smallvec::SmallVec;

use crate::cf::Cf;
use crate::config::{InstallToml, Res, hex, now, random, read_private_key, read_public_key};
use crate::placement::install_account;

/// The path the edge takes signed `Release` entries on.
pub const ENTRIES_PATH: &str = "/api/graftyard/v1/release/entries";
/// The body is the entry's stored bytes.
pub const ENTRY_CONTENT_TYPE: &str = "application/octet-stream";
/// The path the edge creates `Tokens` shard groups on, a range at a time, with the signed genesis as the body.
pub const TOKENS_PATH: &str = "/api/graftyard/v1/release/tokens";
/// Shards per request (the edge's `RANGE_MAX`) and requests at once.
const TOKENS_RANGE: u16 = 16;
const TOKENS_AT_ONCE: usize = 8;

const SYSTEM: [SystemChain; 4] = [SystemChain::Release, SystemChain::Names, SystemChain::Tokens, SystemChain::Ids];

/// Where the system chains go: `Solo` on the first active site of an evaluation install, otherwise the named sites
/// (the first three active sites by default) as `Tri`, or as `Na3` when they are not enam, weur, and apac. Checked
/// with `Placement::new`'s rules against the recorded `Rtt` rows.
fn system_record(t: &InstallToml, sites: Option<&[SiteId]>) -> Res<PlacementRecord> {
    let active: Vec<SiteId> = t.sites.iter().filter(|s| !s.spare).map(|s| s.id).collect();
    let (name, sites): (PlacementName, SmallVec<[SiteId; 3]>) = if t.evaluation {
        let s = sites.and_then(|s| s.first().copied()).or(active.first().copied());
        (PlacementName::Solo, s.into_iter().collect())
    } else {
        let s: SmallVec<[SiteId; 3]> = match sites {
            Some(s) => s.iter().copied().collect(),
            None => active.iter().take(3).copied().collect(),
        };
        let regions: Vec<_> = s.iter().filter_map(|id| t.table().get(*id).map(|x| x.region.hint())).collect();
        let name = if regions == ["enam", "weur", "apac"] { PlacementName::Tri } else { PlacementName::Na3 };
        (name, s)
    };
    let record = PlacementRecord {
        name,
        home: sites.first().copied().unwrap_or(SiteId(0)),
        sites,
        generation: 1,
        moving_from: None,
        at: now(),
    };
    let rtt = t.rtt_rows();
    let table = t.table();
    let checks = PlacementChecks { sites: &table, separate_accounts: t.separate_accounts(), rtt: &rtt };
    validate_record(install_account(), &record, t.evaluation, &checks)
        .map_err(|e| format!("system-chain placement {:?} on {:?}: {e}", record.name, record.sites))?;
    debug_assert!(name != PlacementName::Na3 || name.kinds().iter().all(|k| *k == MemberKind::Full));
    Ok(record)
}

/// A drain or executor key pair, made on this machine. The private half goes to `graft-ops`'s Secrets Store and is
/// not kept.
fn make_key(cf: Option<(&Cf, &str)>, name: &str) -> Res<KeyId> {
    let seed: [u8; 32] = random();
    let key = public_key(&seed);
    match cf {
        Some((cf, store)) => {
            cf.put_secret(store, name, &hex(&seed), "graftyard genesis: Ed25519 seed, hex")?;
            println!("  {name}: public {} written to Secrets Store {store}; no copy kept", key.hex());
        }
        None => println!("  {name}: public {}; --offline-keys: the private half is discarded", key.hex()),
    }
    Ok(key)
}

pub struct GenesisArgs<'a> {
    pub out: &'a Path,
    pub release_key: &'a Path,
    pub vendor_key: Option<&'a Path>,
    pub sites: Option<Vec<SiteId>>,
    pub offline_keys: bool,
}

pub fn genesis(config: &Path, a: GenesisArgs) -> Res<()> {
    let t = InstallToml::load(config)?;
    if t.genesis.is_some() {
        return Err("this install already has a genesis".into());
    }
    let record = system_record(&t, a.sites.as_deref())?;
    let release = read_public_key(a.release_key)?;
    let vendor = a.vendor_key.map(read_public_key).transpose()?;
    let anchor = t.anchor()?;
    let cf = if a.offline_keys { None } else { Some(Cf::from_env(&t.account)?) };
    let store_name = format!("graft-ops-{}", t.install.to_ascii_lowercase());
    let store = match &cf {
        Some(cf) => Some(cf.store(&store_name)?),
        None => None,
    };
    let target = cf.as_ref().zip(store.as_deref());
    // Slot 0 of each rotatable key ("Install operations", Rotation runbook): both slots are bound at install.
    let drain = make_key(target, "graftyard-drain-0")?;
    let executor = make_key(target, "graftyard-executor-0")?;
    let g = Genesis {
        install: anchor.install,
        account: install_account(),
        keys: Keys {
            generation: 0,
            customer: anchor.customer,
            vendor,
            release,
            drain,
            executor,
            agents_executor: None,
            ring: SmallVec::new(),
        },
        sites: t.sites.iter().cloned().collect(),
        placements: SYSTEM.iter().map(|c| (*c, record.clone())).collect(),
        evaluation: t.evaluation,
        restore: None,
    };
    let entry = SignedEntry::unsigned(
        Bound { install: anchor.install, group: ChainId::Release, prev: None, parent: None },
        ReleaseEntry::Genesis(Box::new(g)),
    );
    // Everything but the signature must already pass, so the customer signs nothing the group would refuse.
    let mut probe = entry.clone();
    probe.sigs.push((anchor.customer, graftyard_release::Signature([0; 64])));
    match check_genesis(&anchor, &probe) {
        Err(graftyard_release::Refusal::BadSignature) => {}
        Err(e) => return Err(format!("the genesis would be refused: {e}")),
        Ok(()) => unreachable!("a zero signature verified"),
    }
    let bytes = entry.encode();
    fs::write(a.out, &bytes).map_err(|e| format!("{}: {e}", a.out.display()))?;
    println!(
        "wrote unsigned genesis {} ({} bytes): install {}, {} sites, system chains {:?} on {:?}",
        a.out.display(),
        bytes.len(),
        t.install,
        t.sites.len(),
        record.name,
        record.sites.iter().map(|s| s.0).collect::<Vec<_>>()
    );
    println!(
        "not created: graft-front, the edge, graft-ops, and the Workflows script (no M1 bundle exists for them); \
         each must bind every site when it is built"
    );
    Ok(())
}

/// What the customer is about to sign, in words.
fn describe(g: &Genesis) -> String {
    let mut s = format!(
        "install {}\naccount {}\nevaluation {}\ncustomer key {}\nvendor key {}\nrelease key {}\ndrain key {}\nexecutor key {}\n",
        g.install,
        g.account.0.0,
        g.evaluation,
        g.keys.customer.hex(),
        g.keys.vendor.map(|k| k.hex()).unwrap_or_else(|| "none (self-hosted)".into()),
        g.keys.release.hex(),
        g.keys.drain.hex(),
        g.keys.executor.hex(),
    );
    for site in &g.sites {
        s += &format!(
            "site {} {} {}{} bucket {}\n",
            site.id.0,
            site.script,
            site.region.hint(),
            if site.spare { " spare" } else { "" },
            site.bucket
        );
    }
    for (c, r) in &g.placements {
        s += &format!(
            "{c:?}: {:?} on {:?}, home {}\n",
            r.name,
            r.sites.iter().map(|x| x.0).collect::<Vec<_>>(),
            r.home.0
        );
    }
    s
}

/// Offline: no network, no install.toml. Refuses a key that is not the genesis's own customer key, then checks the
/// signed result against the anchor the genesis names.
pub fn sign(input: &Path, out: &Path, key: &Path) -> Res<()> {
    let bytes = fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let mut entry = SignedEntry::decode(&bytes).map_err(|e| format!("{}: {e}", input.display()))?;
    let ReleaseEntry::Genesis(g) = &entry.body else {
        return Err(format!("{}: a {} entry, not a genesis", input.display(), entry.body.kind()));
    };
    let seed = read_private_key(key)?;
    if public_key(&seed) != g.keys.customer {
        return Err(format!(
            "{} is key {}, but the genesis names customer key {}",
            key.display(),
            public_key(&seed).hex(),
            g.keys.customer.hex()
        ));
    }
    print!("{}", describe(g));
    let anchor = graftyard_release::Anchor { install: g.install, customer: g.keys.customer };
    entry.sigs.clear();
    entry.sign(&seed);
    let signed = entry.encode();
    accept_genesis(&anchor, &signed).map_err(|e| format!("the signed genesis does not verify: {e}"))?;
    fs::write(out, &signed).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("signed: wrote {} ({} bytes), digest {}", out.display(), signed.len(), entry.digest().hex());
    Ok(())
}

/// The edge's answer, as `graftyard install` reads it.
#[derive(Debug, PartialEq)]
pub enum Answer {
    Created {
        index: u64,
        digest: String,
    },
    /// 409: `prev` is not the group's current digest. For a genesis, `prev` is the digest of the genesis the group holds.
    Stale {
        prev: String,
        index: u64,
    },
    Refused {
        status: u16,
        message: String,
        refusal: String,
    },
}

/// `POST {edge}/api/graftyard/v1/release/entries` with the entry's stored bytes and no token.
pub fn post_entry(edge: &str, bytes: &[u8]) -> Res<Answer> {
    let url = format!("{}{ENTRIES_PATH}", edge.trim_end_matches('/'));
    let r =
        ureq::post(&url).set("content-type", ENTRY_CONTENT_TYPE).set("accept", "application/json").send_bytes(bytes);
    let (status, text) = match r {
        Ok(r) => (r.status(), r.into_string().map_err(|e| e.to_string())?),
        Err(ureq::Error::Status(code, r)) => (code, r.into_string().unwrap_or_default()),
        Err(e) => return Err(format!("POST {url}: {e}")),
    };
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    Ok(match status {
        201 => Answer::Created { index: v["index"].as_u64().unwrap_or(0), digest: s("digest") },
        409 => Answer::Stale { prev: s("prev"), index: v["index"].as_u64().unwrap_or(0) },
        _ => Answer::Refused {
            status,
            message: if v.is_null() { text.chars().take(300).collect() } else { s("message") },
            refusal: s("refusal"),
        },
    })
}

/// Creates every `Tokens` shard's group after the genesis commits (R20): `TOKEN_SHARDS` in ranges of `TOKENS_RANGE`,
/// `TOKENS_AT_ONCE` ranges at a time. The edge's create is idempotent, so a failed range is retried by running
/// `install` again. Returns the ranges that failed, with the edge's answer.
pub fn precreate_tokens(edge: &str, bytes: &[u8]) -> Vec<(u16, String)> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU16, Ordering};
    let next = AtomicU16::new(0);
    let failed = Mutex::new(Vec::new());
    let base = edge.trim_end_matches('/');
    std::thread::scope(|s| {
        for _ in 0..TOKENS_AT_ONCE {
            s.spawn(|| {
                loop {
                    let from = next.fetch_add(TOKENS_RANGE, Ordering::Relaxed);
                    if from >= graftyard_identity::TOKEN_SHARDS {
                        return;
                    }
                    let url = format!("{base}{TOKENS_PATH}?from={from}&count={TOKENS_RANGE}");
                    let r = ureq::post(&url)
                        .timeout(std::time::Duration::from_secs(600))
                        .set("content-type", ENTRY_CONTENT_TYPE)
                        .send_bytes(bytes);
                    let err = match r {
                        Ok(_) => continue,
                        Err(ureq::Error::Status(code, r)) => {
                            format!("HTTP {code} {}", r.into_string().unwrap_or_default())
                        }
                        Err(e) => e.to_string(),
                    };
                    failed.lock().expect("no thread panics holding it").push((from, err));
                }
            });
        }
    });
    let mut f = failed.into_inner().expect("no thread panicked");
    f.sort();
    f
}

pub fn install(config: &Path, genesis: &Path, edge: Option<&str>) -> Res<()> {
    let mut t = InstallToml::load(config)?;
    let mut bytes = Vec::new();
    fs::File::open(genesis)
        .and_then(|mut f| f.read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", genesis.display()))?;
    // The same acceptance the Release group runs, against this install's anchor, before anything is sent.
    accept_genesis(&t.anchor()?, &bytes).map_err(|e| format!("refused locally ({}): {e}", e.variant()))?;
    let ours = SignedEntry::decode(&bytes).expect("accepted").digest().hex();
    let edge = edge.map(String::from).unwrap_or_else(|| format!("https://{}", t.zone));
    match post_entry(&edge, &bytes)? {
        Answer::Created { index, digest } if digest == ours => {
            println!("installed: the Release group committed genesis {digest} at index {index}");
        }
        Answer::Created { digest, .. } => {
            return Err(format!("the edge answered 201 with digest {digest}, but this genesis is {ours}"));
        }
        // A retry after a lost 201: the group holds this very genesis.
        Answer::Stale { prev, index } if prev == ours => {
            println!("already installed: the Release group holds this genesis ({ours}) and is at index {index}");
        }
        Answer::Stale { prev, .. } => {
            return Err(format!("refused: the Release group already holds another genesis ({prev})"));
        }
        Answer::Refused { status, message, refusal } => {
            return Err(format!("refused by the edge: HTTP {status} {refusal}: {message}"));
        }
    }
    t.genesis = Some(ours);
    t.save(config)?;
    let failed = precreate_tokens(&edge, &bytes);
    if !failed.is_empty() {
        let first: Vec<String> = failed.iter().take(5).map(|(f, e)| format!("from {f}: {e}")).collect();
        return Err(format!(
            "installed, but {} of {} Tokens shard ranges did not start; run install again ({})",
            failed.len(),
            graftyard_identity::TOKEN_SHARDS.div_ceil(TOKENS_RANGE),
            first.join("; ")
        ));
    }
    println!("created all {} Tokens shard groups", graftyard_identity::TOKEN_SHARDS);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    use super::*;

    /// One request as the stub saw it.
    pub struct Seen {
        pub line: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    /// An HTTP stub on localhost that serves `n` requests: records each and answers what `answer` returns.
    pub fn serve(
        n: usize,
        mut answer: impl FnMut(&Seen) -> (u16, String) + Send + 'static,
    ) -> (String, mpsc::Receiver<Seen>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..n {
                let (mut s, _) = l.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let mut headers = Vec::new();
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap();
                    headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
                }
                let len = headers.iter().find(|(k, _)| k == "content-length").map_or(0, |(_, v)| v.parse().unwrap());
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                let seen = Seen { line: line.trim_end().into(), headers, body };
                let (status, text) = answer(&seen);
                write!(
                    s,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                    text.len()
                )
                .unwrap();
                tx.send(seen).unwrap();
            }
        });
        (url, rx)
    }

    pub fn stub(status: u16, answer: String) -> (String, mpsc::Receiver<Seen>) {
        serve(1, move |_| (status, answer.clone()))
    }

    #[test]
    fn the_entry_is_posted_as_its_stored_bytes_with_no_token() {
        let bytes = b"graftyard/release/1 example".to_vec();
        let (url, seen) = stub(201, r#"{"index": 2, "digest": "ab"}"#.into());
        assert_eq!(post_entry(&url, &bytes).unwrap(), Answer::Created { index: 2, digest: "ab".into() });
        let s = seen.recv().unwrap();
        assert_eq!(s.line, format!("POST {ENTRIES_PATH} HTTP/1.1"));
        assert_eq!(s.body, bytes);
        let h = |k: &str| s.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(h("content-type"), Some(ENTRY_CONTENT_TYPE));
        assert_eq!(h("content-length"), Some(bytes.len().to_string().as_str()));
        assert_eq!(h("authorization"), None, "the signatures are the credential");
    }

    /// The edge as the design specifies it, over `ReleaseFold`: 201 with the index and digest after the commit, 409
    /// with the current digest for a stale `prev` (a genesis into a group that holds one), and 422 with the refusal.
    fn edge(anchor: graftyard_release::Anchor, n: usize) -> (String, mpsc::Receiver<Seen>) {
        let mut fold = graftyard_release::ReleaseFold::default();
        let mut index = 1; // the bootstrap Config entry
        serve(n, move |seen| match fold.decide(&anchor, &seen.body) {
            _ if seen.line.starts_with(&format!("POST {TOKENS_PATH}")) => (200, "{}".into()),
            Ok(ev) => {
                fold.apply(&ev);
                index += 2; // the leader's term Noop, then the entry
                (201, format!(r#"{{"index": {index}, "digest": "{}"}}"#, ev.digest().hex()))
            }
            Err(graftyard_release::Refusal::AlreadyInstalled(d)) => {
                (409, format!(r#"{{"message": "stale prev", "prev": "{}", "index": {index}}}"#, d.hex()))
            }
            Err(e) => (422, serde_json::json!({ "message": e.to_string(), "refusal": e.variant() }).to_string()),
        })
    }

    fn scratch() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("graftyard-cli-test-{}", hex(&random::<6>())));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn install_toml(dir: &Path, customer: &[u8; 32]) -> std::path::PathBuf {
        use graftyard_types::{Region, Site, SiteLink, Timestamp};
        let site = |id: u8, region: Region, spare: bool| Site {
            id: SiteId(id),
            script: format!("graft-site-{id}"),
            account: "acct".into(),
            region,
            jurisdiction: None,
            bucket: format!("graft-site-{id}-packs"),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare,
        };
        let row = |from: u8, peer: u8, ms| crate::config::RttRow {
            from: SiteId(from),
            peer: SiteId(peer),
            median_ms: ms,
            at: Timestamp(1),
        };
        let t = InstallToml {
            install: crate::config::mint_install().to_string(),
            account: "acct".into(),
            zone: "git.acme.example".into(),
            customer_key: public_key(customer).hex(),
            evaluation: false,
            genesis: None,
            sites: vec![
                site(0, Region::Enam, false),
                site(1, Region::Weur, false),
                site(2, Region::Apac, false),
                site(3, Region::Eeur, true),
            ],
            rtt: vec![row(0, 1, 98), row(0, 2, 194), row(1, 2, 187)],
            default_placement: None,
        };
        let p = dir.join("install.toml");
        t.save(&p).unwrap();
        p
    }

    fn key_file(dir: &Path, name: &str, seed: &[u8; 32], public: bool) -> std::path::PathBuf {
        let p = dir.join(name);
        let text = if public { public_key(seed).hex() } else { hex(seed) };
        fs::write(&p, text + "\n").unwrap();
        p
    }

    fn make_genesis(dir: &Path, config: &Path, out: &str) -> std::path::PathBuf {
        let release = key_file(dir, "release.pub", &[3; 32], true);
        let out = dir.join(out);
        let args = GenesisArgs { out: &out, release_key: &release, vendor_key: None, sites: None, offline_keys: true };
        genesis(config, args).unwrap();
        out
    }

    #[test]
    fn genesis_sign_install_then_replay_and_a_second_genesis() {
        let dir = scratch();
        let customer = [7; 32];
        let config = install_toml(&dir, &customer);
        let t = InstallToml::load(&config).unwrap();
        let unsigned = make_genesis(&dir, &config, "genesis.unsigned");
        let key = key_file(&dir, "customer.key", &customer, false);
        let signed = dir.join("genesis.signed");
        sign(&unsigned, &signed, &key).unwrap();
        // The unsigned file is refused by the edge's own rule before anything is sent.
        assert!(install(&config, &unsigned, Some("http://127.0.0.1:9")).is_err_and(|e| e.contains("WrongKey")));

        // The entry three times, and after each of the two that install accepts, every Tokens range.
        let ranges = usize::from(graftyard_identity::TOKEN_SHARDS.div_ceil(TOKENS_RANGE));
        let (url, seen) = edge(t.anchor().unwrap(), 3 + 2 * ranges);
        install(&config, &signed, Some(&url)).unwrap();
        let first = seen.recv().unwrap();
        assert_eq!(first.body, fs::read(&signed).unwrap());
        let digest = InstallToml::load(&config).unwrap().genesis.unwrap();
        assert_eq!(digest, SignedEntry::decode(&first.body).unwrap().digest().hex());

        // A replay of the same genesis (a retry after a lost 201) is recognised and changes nothing.
        install(&config, &signed, Some(&url)).unwrap();
        // A second, valid genesis for this install is refused: the group holds another.
        let mut t2 = InstallToml::load(&config).unwrap();
        t2.genesis = None;
        t2.save(&config).unwrap();
        let unsigned2 = make_genesis(&dir, &config, "genesis2.unsigned");
        let signed2 = dir.join("genesis2.signed");
        sign(&unsigned2, &signed2, &key).unwrap();
        let r = install(&config, &signed2, Some(&url));
        assert!(r.as_ref().is_err_and(|e| e.contains("already holds another genesis")), "{r:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sign_refuses_another_key_and_the_edge_refuses_another_install() {
        let dir = scratch();
        let customer = [7; 32];
        let config = install_toml(&dir, &customer);
        let unsigned = make_genesis(&dir, &config, "genesis.unsigned");
        let other = key_file(&dir, "other.key", &[8; 32], false);
        let r = sign(&unsigned, &dir.join("x"), &other);
        assert!(r.is_err_and(|e| e.contains("but the genesis names customer key")));

        // A genesis for another install, signed by that install's key, sent straight to this install's edge.
        let other_dir = scratch();
        let other_config = install_toml(&other_dir, &[8; 32]);
        let foreign = make_genesis(&other_dir, &other_config, "g.unsigned");
        let foreign_signed = other_dir.join("g.signed");
        sign(&foreign, &foreign_signed, &other).unwrap();
        let anchor = InstallToml::load(&config).unwrap().anchor().unwrap();
        let (url, _seen) = edge(anchor, 1);
        assert_eq!(
            post_entry(&url, &fs::read(&foreign_signed).unwrap()).unwrap(),
            Answer::Refused {
                status: 422,
                message: format!(
                    "bound to install {}, this install is {}",
                    InstallToml::load(&other_config).unwrap().install,
                    anchor.install
                ),
                refusal: "WrongInstall".into(),
            }
        );
        // And `install` refuses it locally against this install's anchor.
        assert!(install(&config, &foreign_signed, Some(&url)).is_err_and(|e| e.contains("WrongInstall")));
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&other_dir).unwrap();
    }

    #[test]
    fn genesis_refuses_system_chains_it_cannot_place() {
        let dir = scratch();
        let config = install_toml(&dir, &[7; 32]);
        let mut t = InstallToml::load(&config).unwrap();
        t.rtt.pop();
        t.save(&config).unwrap();
        let release = key_file(&dir, "release.pub", &[3; 32], true);
        let out = dir.join("g");
        let args = GenesisArgs { out: &out, release_key: &release, vendor_key: None, sites: None, offline_keys: true };
        assert!(genesis(&config, args).is_err_and(|e| e.contains("no round-trip measurement")));
        let args = GenesisArgs {
            out: &out,
            release_key: &release,
            vendor_key: None,
            sites: Some(vec![SiteId(0), SiteId(1), SiteId(3)]),
            offline_keys: true,
        };
        assert!(genesis(&config, args).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// R20: install creates every `Tokens` shard once, in ranges, and reports the ranges the edge did not start.
    #[test]
    fn install_creates_every_tokens_shard_and_reports_the_ranges_that_failed() {
        let ranges = usize::from(graftyard_identity::TOKEN_SHARDS.div_ceil(TOKENS_RANGE));
        let (url, seen) =
            serve(ranges, |s| if s.line.contains("from=32&") { (503, "{}".into()) } else { (200, "{}".into()) });
        let failed = precreate_tokens(&url, b"genesis bytes");
        assert_eq!(failed.iter().map(|(f, _)| *f).collect::<Vec<_>>(), vec![32]);
        let mut covered = vec![0u8; usize::from(graftyard_identity::TOKEN_SHARDS)];
        for _ in 0..ranges {
            let s = seen.recv().unwrap();
            assert_eq!(s.body, b"genesis bytes", "the signed genesis is the credential");
            let q = s.line.split(['?', ' ']).nth(2).unwrap();
            let get = |k: &str| q.split('&').find_map(|p| p.strip_prefix(k)).unwrap().parse::<usize>().unwrap();
            let (from, count) = (get("from="), get("count="));
            let end = (from + count).min(covered.len());
            for c in &mut covered[from..end] {
                *c += 1;
            }
        }
        assert!(covered.iter().all(|c| *c == 1), "every shard exactly once");
    }

    #[test]
    fn stale_and_refused_answers_are_read() {
        let (url, _seen) = stub(409, r#"{"message": "stale prev", "prev": "cd", "index": 1}"#.into());
        assert_eq!(post_entry(&url, b"x").unwrap(), Answer::Stale { prev: "cd".into(), index: 1 });
        let (url, _seen) = stub(422, r#"{"message": "wrong key", "refusal": "WrongKey"}"#.into());
        assert_eq!(
            post_entry(&url, b"x").unwrap(),
            Answer::Refused { status: 422, message: "wrong key".into(), refusal: "WrongKey".into() }
        );
    }
}
