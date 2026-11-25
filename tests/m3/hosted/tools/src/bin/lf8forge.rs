//! LF-8's forged `release/entries` (`build/BUILD-PLAN.md`, AM3-WP1, LF-8): a synthetic install's `Release` history and
//! 300 forgeries, built offline from throwaway keys drawn for this run and never written anywhere but the output
//! directory, which the caller keeps outside the repository and shreds after the row.
//!
//!   lf8forge <out dir> <seed hex, 32 bytes>
//!
//! The seed drives every key and id, so a run is one draw; the driver reads 32 bytes from /dev/urandom. Writes:
//! - `history.json`: the install's committed entries, `[{"index", "entry" (base64), "at"}]`, as the edge pages them.
//!   Genesis, two uploads, a `ControllerRoll` run to its end, a second `ControllerRoll` at its 1% step, and its
//!   pre-signed `ControllerPointer` committed, so a view of it pins release 3's controller.
//! - `forgeries.json`: `[{"class", "k", "case", "index", "entry", "at", "sha256", "field"}]`, 100 per class, each a
//!   one-entry page the forger serves after the history.
//! - `control.json`: the group's real next entry, as one page.
//! - `cold.json`: install B's own genesis as a one-entry page from index 1, for a cold view anchored to install A.
//! - `anchor.txt`: `install = "..."` and `customer_key = "..."`, as `install.toml` carries them, for the front's vars.
//! - `manifest.tsv`: class, k, case, sha256 of each forgery's bytes, and the one field it differs from the control in.
//!   This file is committed; nothing else is.
//!
//! Before writing, every forgery is checked:
//! - it differs from the control in exactly the one field its class names, by decoding both and comparing every other
//!   part (`bound.install`, `bound.prev`, `bound.parent`, the body, the signer, and the index), so a forgery cannot be
//!   refused for a reason other than its own;
//! - the agents view, as `graft-agents-front` holds it after reading the history, refuses it with its class's refusal,
//!   moves nothing, and still accepts the control afterwards.
//!
//! Classes:
//! - `install`: the control's step re-bound to another install (a different one for each k) and signed by the same agents
//!   executor key (100). For a cold view, `cold.json` carries install B's genesis, signed by B's customer key.
//! - `keys`: a `Keys` at the next generation, the control's index and prev, signed by a key that is not the anchored
//!   customer key: the vendor, the executor, the agents executor, a stranger, or a new customer key alone (20 each).
//! - `order`: the control's exact bytes at the wrong index: a gap (index + 2 to + 8, 50), or a replay at an index the
//!   view holds with other bytes (the control at the last held index, 25, or at the one before it, 25). A replay at
//!   a held index is skipped by design (`AgentsReleaseView::extend`: "entries the view holds are skipped"), so its
//!   check is that the view does not move and does not take the bytes, not a refusal.

use std::fs;
use std::io::Write as _;

use graftyard_release::agents::{AgentsAnchor, AgentsReleaseView};
use graftyard_release::{
    Anchor, Bound, Format, FormatSet, Genesis, Intent, IntentId, InstallId, Keys, ReleaseEntry, ReleaseFold,
    ReleaseUploaded, Script, SignedEntry, SignedIntent, VersionId, presign_undo, public_key, sign_record, undo_entry,
};
use graftyard_types::{
    AccountId, ChainId, DbId, Index, PlacementName, PlacementRecord, Region, ReleaseId, Site, SiteId, SiteLink,
    SystemChain, Timestamp, Ulid,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use smallvec::SmallVec;

const HOUR: u64 = 3_600_000;

/// Every key and id of one run, derived from the run's seed.
struct Keyring {
    seed: [u8; 32],
}

impl Keyring {
    fn key(&self, label: &str) -> [u8; 32] {
        Sha256::new().chain_update(self.seed).chain_update(label.as_bytes()).finalize().into()
    }
    fn install(&self, label: &str, t0: Timestamp) -> InstallId {
        let k = self.key(label);
        InstallId(Ulid::new(t0, k[..10].try_into().unwrap()))
    }
}

fn b64(b: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn reads(n: u8) -> FormatSet {
    FormatSet::new((1..=u16::from(n)).map(|v| Format { family: 32, version: v }))
}

fn rel(n: u8) -> (ReleaseId, Vec<u8>) {
    let file = reads(n).file();
    let rec = format!(r#"{{"label":"lf8-r{n}","files":[{{"name":"formats.txt","sha256":"{}"}}]}}"#, hex(&Sha256::digest(file)));
    (ReleaseId(Sha256::digest(rec.as_bytes()).into()), rec.into_bytes())
}

fn vid(n: u8, s: u8) -> VersionId {
    VersionId([n, s, 0x1f, 0x8a, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 1])
}

fn id(n: u8) -> IntentId {
    IntentId(Ulid(u128::from(n) + 0x1f8_0000))
}

fn site(id: u8, region: Region, spare: bool) -> Site {
    Site {
        id: SiteId(id),
        script: format!("graft-lf8-site-{id}"),
        account: "acct".into(),
        region,
        jurisdiction: None,
        bucket: format!("graft-lf8-packs-{id}"),
        release: None,
        link: SiteLink::Binding { member_class: "Member".into() },
        spare,
    }
}

fn genesis(install: InstallId, keys: Keys, customer: &[u8; 32]) -> Vec<u8> {
    let tri = PlacementRecord {
        name: PlacementName::Tri,
        sites: [SiteId(0), SiteId(1), SiteId(2)].into_iter().collect(),
        home: SiteId(0),
        generation: 1,
        moving_from: None,
        at: Timestamp(5),
    };
    let g = Genesis {
        install,
        account: AccountId(DbId(1)),
        keys,
        sites: [site(0, Region::Enam, false), site(1, Region::Weur, false), site(2, Region::Apac, false), site(3, Region::Eeur, true)]
            .into_iter()
            .collect(),
        placements: [SystemChain::Release, SystemChain::Names, SystemChain::Tokens, SystemChain::Ids]
            .into_iter()
            .map(|c| (c, tri.clone()))
            .collect(),
        evaluation: false,
        restore: None,
    };
    let bound = Bound { install, group: ChainId::Release, prev: None, parent: None };
    let mut e = SignedEntry::unsigned(bound, ReleaseEntry::Genesis(Box::new(g)));
    e.sign(customer);
    e.encode()
}

struct Group {
    anchor: Anchor,
    fold: ReleaseFold,
    at: Timestamp,
    log: Vec<(Vec<u8>, Timestamp)>,
}

impl Group {
    fn ok(&mut self, bytes: Vec<u8>) {
        self.at = Timestamp(self.at.0 + 60_000);
        let ev = self.fold.decide(&self.anchor, self.at, &bytes).expect("the synthetic group accepts its own entry");
        self.fold.apply(&ev, self.at);
        self.log.push((bytes, self.at));
    }
    fn entry(&self, body: ReleaseEntry, seed: &[u8; 32]) -> Vec<u8> {
        let b = Bound { install: self.anchor.install, group: ChainId::Release, prev: self.fold.last, parent: None };
        let mut e = SignedEntry::unsigned(b, body);
        e.sign(seed);
        e.encode()
    }
}

fn page(rows: &[(u64, &[u8], Timestamp)]) -> Value {
    Value::Array(rows.iter().map(|(i, e, at)| json!({ "index": i, "entry": b64(e), "at": at.0 })).collect())
}

/// The fields a forgery may differ from the control in; everything else must be equal.
fn differs(control: &SignedEntry, ci: u64, f: &SignedEntry, fi: u64) -> Vec<&'static str> {
    let mut d = Vec::new();
    if f.bound.install != control.bound.install {
        d.push("bound.install");
    }
    if f.bound.group != control.bound.group {
        d.push("bound.group");
    }
    if f.bound.prev != control.bound.prev {
        d.push("bound.prev");
    }
    if f.bound.parent != control.bound.parent {
        d.push("bound.parent");
    }
    if f.body != control.body {
        d.push("body");
    }
    let signer = |e: &SignedEntry| e.sigs.iter().map(|(k, _)| *k).collect::<Vec<_>>();
    if signer(f) != signer(control) {
        d.push("signer");
    }
    if fi != ci {
        d.push("index");
    }
    d
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [out, seed] = a.as_slice() else { panic!("usage: lf8forge <out dir> <seed hex>") };
    let seed: [u8; 32] = (0..32).map(|i| u8::from_str_radix(&seed[2 * i..2 * i + 2], 16).unwrap()).collect::<Vec<_>>().try_into().unwrap();
    let r = Keyring { seed };
    let t0 = Timestamp(1_795_000_000_000);
    let (customer, vendor, release, drain, executor, agents) =
        (r.key("customer"), r.key("vendor"), r.key("release"), r.key("drain"), r.key("executor"), r.key("agents"));
    let keys = Keys {
        generation: 0,
        customer: public_key(&customer),
        vendor: Some(public_key(&vendor)),
        release: public_key(&release),
        drain: public_key(&drain),
        executor: public_key(&executor),
        agents_executor: Some(public_key(&agents)),
        ring: SmallVec::new(),
    };
    let install_a = r.install("install-a", t0);
    let install_b = r.install("install-b", t0);
    let anchor = Anchor { install: install_a, customer: public_key(&customer) };
    let agents_anchor = AgentsAnchor { install: install_a, customer: anchor.customer, host: "lf8.invalid".into() };

    // The history: genesis, uploads 3 and 4, a ControllerRoll to 3 run to its end, a ControllerRoll to 4 at 1% with
    // its pre-signed ControllerPointer to 3 committed.
    let mut g = Group { anchor, fold: ReleaseFold::default(), at: t0, log: Vec::new() };
    g.ok(genesis(install_a, keys.clone(), &customer));
    for n in [3u8, 4] {
        let (release_id, record) = rel(n);
        let body = ReleaseEntry::ReleaseUploaded(Box::new(ReleaseUploaded {
            release: release_id,
            record_sig: sign_record(&release, &record),
            record: record.into(),
            versions: [(Script::Controller, vid(n, 1)), (Script::AgentsFront, vid(n, 2))].into_iter().collect(),
            images: SmallVec::new(),
            reads: reads(n),
        }));
        let b = g.entry(body, &agents);
        g.ok(b);
    }
    let roll = |g: &Group, n: u8, to: u8, undo: &[ReleaseEntry]| {
        let undo = undo.iter().map(|u| (u.clone(), presign_undo(install_a, id(n), u, &vendor))).collect();
        let body = ReleaseEntry::Intent(Box::new(SignedIntent {
            id: id(n),
            issued_at: g.at,
            not_after: Timestamp(g.at.0 + 1000 * 24 * HOUR),
            intent: Intent::ControllerRoll { release: rel(to).0 },
            undo,
        }));
        g.entry(body, &vendor)
    };
    let b = roll(&g, 1, 3, &[]);
    g.ok(b);
    for s in 0..2 {
        let b = g.entry(ReleaseEntry::IntentStep { intent: id(1), step: s }, &agents);
        g.ok(b);
    }
    let b = g.entry(ReleaseEntry::IntentDone { intent: id(1) }, &agents);
    g.ok(b);
    let pointer = ReleaseEntry::ControllerPointer { pin: rel(3).0 };
    let b = roll(&g, 2, 4, std::slice::from_ref(&pointer));
    g.ok(b);
    let b = g.entry(ReleaseEntry::IntentStep { intent: id(2), step: 0 }, &agents);
    g.ok(b);
    let u = undo_entry(install_a, id(2), &pointer, public_key(&vendor), presign_undo(install_a, id(2), &pointer, &vendor));
    g.ok(u.encode());

    let held = g.log.len() as u64;
    let mut view = AgentsReleaseView::default();
    let hist: Vec<(Index, Vec<u8>, Timestamp)> =
        g.log.iter().enumerate().map(|(i, (b, at))| (Index(i as u64 + 1), b.clone(), *at)).collect();
    view.extend(&agents_anchor, &hist).expect("the view reads the synthetic history");
    assert_eq!((view.index(), view.controller()), (Index(held), Some(vid(3, 1))), "the view pins release 3's controller");

    // The control: the group's real next entry, the undo's step 0 by the agents executor.
    let next = held + 1;
    let at = Timestamp(g.at.0 + 60_000);
    let control_bytes = g.entry(ReleaseEntry::IntentStep { intent: id(2), step: 0 }, &agents);
    let control = SignedEntry::decode(&control_bytes).unwrap();
    assert_eq!(view.clone().extend(&agents_anchor, &[(Index(next), control_bytes.clone(), at)]), Ok(1), "the control is accepted");

    let mut forgeries = Vec::new();
    let mut manifest = String::from("class\tk\tcase\tsha256\tfield\n");
    let mut push = |class: &str, k: u32, case: &str, index: u64, bytes: Vec<u8>, field: &str, expect: String| {
        let sha = hex(&Sha256::digest(&bytes));
        manifest.push_str(&format!("{class}\t{k}\t{case}\t{sha}\t{field}\n"));
        forgeries.push(json!({ "class": class, "k": k, "case": case, "index": index, "entry": b64(&bytes), "at": at.0,
                               "sha256": sha, "field": field, "expect": expect }));
    };
    let refused = |v: &AgentsReleaseView, i: u64, bytes: &[u8]| {
        let mut w = v.clone();
        let e = w.extend(&agents_anchor, &[(Index(i), bytes.to_vec(), at)]).expect_err("a forgery was accepted");
        assert_eq!(&w, v, "a refused entry moves nothing");
        assert_eq!(w.clone().extend(&agents_anchor, &[(Index(next), control_bytes.clone(), at)]), Ok(1), "the control still lands");
        e
    };

    // 1. Another install: the control re-bound to install B, signed by the same agents executor.
    for k in 0..100u32 {
        // Install B_k, a different install for each k, so the 100 forgeries are distinct and each refusal names its own.
        let other = r.install(&format!("install-b-{k}"), t0);
        let mut e = SignedEntry::unsigned(Bound { install: other, ..control.bound.clone() }, control.body.clone());
        e.sign(&agents);
        let bytes = e.encode();
        assert_eq!(differs(&control, next, &e, next), ["bound.install"]);
        let e = refused(&view, next, &bytes);
        assert_eq!(e.variant(), "WrongInstall");
        let expect = other.to_string();
        assert!(e.to_string().contains(&expect), "the refusal names install B_k");
        push("install", k, "rebound", next, bytes, "bound.install", expect);
    }
    // 2. A Keys at the next generation, not signed by the anchored customer key. The control here is the group's
    // valid Keys at the next generation (signed by the customer), so the one field that differs is the signer.
    let mut new = keys.clone();
    new.generation = 1;
    let keys_control_bytes = g.entry(ReleaseEntry::Keys(Box::new(new.clone())), &customer);
    let keys_control = SignedEntry::decode(&keys_control_bytes).unwrap();
    assert_eq!(view.clone().extend(&agents_anchor, &[(Index(next), keys_control_bytes.clone(), at)]), Ok(1), "a customer-signed Keys is accepted");
    for k in 0..100u32 {
        let stranger = r.key(&format!("stranger-{k}"));
        let new_customer = r.key(&format!("new-customer-{k}"));
        let (case, signer) = match k % 5 {
            0 => ("vendor", vendor),
            1 => ("executor", executor),
            2 => ("agents_executor", agents),
            3 => ("stranger", stranger),
            _ => ("new_customer_alone", new_customer),
        };
        let mut body = new.clone();
        if k % 5 == 4 {
            body.customer = public_key(&new_customer);
        }
        let mut e = SignedEntry::unsigned(keys_control.bound.clone(), ReleaseEntry::Keys(Box::new(body)));
        e.sign(&signer);
        let bytes = e.encode();
        let want: &[&str] = if k % 5 == 4 { &["body", "signer"] } else { &["signer"] };
        assert_eq!(differs(&keys_control, next, &e, next), want, "keys k={k}");
        let e = refused(&view, next, &bytes);
        let v = e.variant();
        assert!(matches!(v, "UnknownKey" | "WrongSigner"), "keys k={k}: {v}");
        // A stranger's refusal names its key; the others name the kind.
        let expect = if case == "stranger" { public_key(&stranger).hex()[..12].to_string() } else { "Keys is not signed".into() };
        assert!(e.to_string().contains(&expect), "keys k={k}: {e}");
        push("keys", k, case, next, bytes, if k % 5 == 4 { "signer (and the new customer key it names)" } else { "signer" }, expect);
    }
    // 3. Out of index order: the control's bytes at a gap, or a replay at a held index with other bytes.
    for k in 0..100u32 {
        let (case, index) = match k {
            0..50 => ("gap", next + 1 + u64::from(k % 7)),
            50..75 => ("replay_last", held),
            _ => ("replay_before", held - 1),
        };
        assert_eq!(differs(&control, next, &control, index), ["index"]);
        if case == "gap" {
            assert_eq!(refused(&view, index, &control_bytes), graftyard_release::Refusal::BadState("a release/entries page skips an index"));
        } else {
            assert_ne!(g.log[index as usize - 1].0, control_bytes, "a replay carries other bytes than the held entry");
            // A held index is skipped: the view keeps its fold, its index and its pin, and takes nothing.
            let mut w = view.clone();
            assert_eq!(w.extend(&agents_anchor, &[(Index(index), control_bytes.clone(), at)]), Ok(0));
            assert_eq!(w, view, "a replay moves nothing");
        }
        let expect = if case == "gap" { "skips an index".to_string() } else { String::new() };
        push("order", k, case, index, control_bytes.clone(), "index", expect);
    }

    // The cold view: install B's own genesis, signed by B's customer, from index 1, for a front anchored to A.
    let b_customer = r.key("install-b-customer");
    let mut b_keys = keys.clone();
    b_keys.customer = public_key(&b_customer);
    let cold = genesis(install_b, b_keys, &b_customer);
    let mut c = AgentsReleaseView::default();
    assert_eq!(c.extend(&agents_anchor, &[(Index(1), cold.clone(), t0)]).unwrap_err().variant(), "WrongInstall");
    assert_eq!(c.index(), Index(0));

    fs::create_dir_all(out).unwrap();
    let rows: Vec<(u64, &[u8], Timestamp)> = g.log.iter().enumerate().map(|(i, (b, at))| (i as u64 + 1, b.as_slice(), *at)).collect();
    fs::write(format!("{out}/history.json"), page(&rows).to_string()).unwrap();
    fs::write(format!("{out}/forgeries.json"), Value::Array(forgeries).to_string()).unwrap();
    fs::write(format!("{out}/control.json"), page(&[(next, &control_bytes, at)]).to_string()).unwrap();
    fs::write(format!("{out}/cold.json"), page(&[(1, &cold, t0)]).to_string()).unwrap();
    fs::write(format!("{out}/manifest.tsv"), manifest).unwrap();
    fs::write(format!("{out}/anchor.txt"), format!("install = \"{install_a}\"\ncustomer_key = \"{}\"\n", anchor.customer.hex())).unwrap();
    let mut s = std::io::stdout();
    writeln!(s, "{}", json!({ "history": held, "next": next, "pinned": vid(3, 1).to_string(), "forgeries": 300,
        "install_a": install_a.to_string(), "install_b": install_b.to_string() })).unwrap();
}
