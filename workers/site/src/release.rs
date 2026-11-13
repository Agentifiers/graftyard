//! The `Release` group on a member (`design/DESIGN.md`, Shape, "Release group"; `tests/m1/results/m1-cli.md`), and the
//! `Release` view every site isolate reads system chains' records from.
//!
//! - The fold is `graftyard_release::ReleaseFold` with this script's trust anchor, the `GRAFTYARD_INSTALL` and
//!   `GRAFTYARD_CUSTOMER_KEY` variables of its own version, which `decide` and a follower's `verify` both check
//!   against. A script with no anchor refuses every entry.
//! - The one command is `Submit`, a signed entry's stored bytes; the one event is the accepted entry's canonical bytes,
//!   which are also its `Record` encoding, so every member digests the same bytes.
//! - The view (`graftyard_lookup::genesis`): a site isolate finds the `Release` group by its `MemberRegistry` rows
//!   (its record is in the genesis, which a cold isolate does not hold yet), reads the genesis, accepts it against its
//!   own anchor, and caches it. In Milestone 1 nothing changes a genesis (`LeaveEvaluation` is M5), so the cache is
//!   never refreshed. With the `test-faults` feature only, `GRAFTYARD_GENESIS` stands in for the view on test
//!   installs that run no `Release` group.

use std::cell::RefCell;

use graftyard_lookup::Bindings;
use graftyard_raft::{self as raft, Command, Enc, EntryBody, Ev, Fold, Format, LogEntry, Record, Scratch, TierSet};
use graftyard_release::{Anchor, ReleaseFold, accept_genesis};
use graftyard_types::{ChainId, Digest, PlacementRecord, SystemChain, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use smallvec::{SmallVec, smallvec};
use worker::{Env, SqlStorage};

use crate::family::{Family, Source, Stmt};
use crate::host::{blob, hex, rows, unhex};
use crate::member::Join;

/// The `Release` group's command and event format. `crates/raft` uses family 0, the register 1, identity 0x10 to
/// 0x13, and the repository 0x20. Version 1 is genesis, the only entry Milestone 1 built.
pub const RELEASE_FORMAT: Format = Format { family: 0x30, version: 1 };
/// Every entry after genesis (M5-WP1). A member on a release that reads only version 1 answers `Unsupported` for it,
/// rather than refusing it as `NotGenesis`, so the two-release rule holds across the site day that ships it.
pub const RELEASE_FORMAT_2: Format = Format { family: 0x30, version: 2 };

thread_local! {
    static ANCHOR: RefCell<Option<Option<Anchor>>> = const { RefCell::new(None) };
}

/// This script's trust anchor, read once per isolate. `None` when the script has none (test installs).
pub fn anchor(env: &Env) -> Option<Anchor> {
    ANCHOR.with(|a| {
        *a.borrow_mut().get_or_insert_with(|| {
            let v = |k: &str| env.var(k).ok().map(|v| v.to_string());
            Anchor::from_vars(&v("GRAFTYARD_INSTALL")?, &v("GRAFTYARD_CUSTOMER_KEY")?).ok()
        })
    })
}

fn cached_anchor() -> Option<Anchor> {
    ANCHOR.with(|a| a.borrow().flatten())
}

/// The `Release` record of the genesis a `Release` member was joined with, accepted against this script's anchor.
pub fn genesis_record(env: &Env, join: &Join) -> Result<PlacementRecord, String> {
    let anchor = anchor(env).ok_or("this site script has no trust anchor")?;
    let bytes = join.genesis.as_deref().and_then(unhex).ok_or("a Release member is joined with its genesis, hex")?;
    let g = accept_genesis(&anchor, &bytes).map_err(|e| format!("{}: {e}", e.variant()))?;
    g.placement(SystemChain::Release).cloned().ok_or_else(|| "the genesis has no Release record".into())
}

/// A system chain's record, from the `Release` view (`graftyard_lookup::system_record`).
pub async fn system_record(env: &Env, chain: ChainId) -> Result<PlacementRecord, String> {
    let site = crate::SiteEnv::read(env).map_err(|e| e.to_string())?;
    #[cfg(feature = "test-faults")]
    if let Some(r) = site.genesis_shortcut(chain) {
        return Ok(r);
    }
    let anchor = anchor(env).ok_or("this site script has no trust anchor")?;
    graftyard_lookup::system_record(&Bindings { env, sites: &site.sites }, &anchor, chain).await
}

/// Any chain's record: a system chain's from the `Release` view, an owner chain's (`Authz(owner)`, `Identity(owner)`)
/// from the owner's `Placement` row on `Names`.
pub async fn record_for(env: &Env, chain: ChainId) -> Result<PlacementRecord, String> {
    match chain {
        ChainId::Authz(owner) | ChainId::Identity(owner) => crate::member::owner_record(env, owner).await,
        c => system_record(env, c).await,
    }
}

// ---- the fold, command, and event ----

/// The `Release` group's fold, with the anchor its rules check against.
#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseState {
    pub fold: ReleaseFold,
    pub anchor: Option<Anchor>,
    /// Every accepted entry's bytes, in order: what the `release_entry` table stores. The entry times `apply` reads
    /// are in the fold itself (a drain's `since`), from `release_at`.
    pub entries: Vec<Box<[u8]>>,
}

/// The accepted entry's canonical bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accepted(pub Vec<u8>);

impl Record for Accepted {
    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.0);
    }
}

impl Accepted {
    fn release(&self) -> graftyard_release::Accepted {
        graftyard_release::Accepted { bytes: self.0.clone().into_boxed_slice() }
    }
}

/// The fold an entry decides or verifies on, with that entry's time: the leader's `at` when it decides, the entry's
/// own when a follower verifies, so both judge `not_after` and `ZERO_ACKS` at the same instant.
pub struct ReleaseScratch<'a> {
    base: &'a ReleaseState,
    over: Option<ReleaseFold>,
    at: Timestamp,
}

impl ReleaseScratch<'_> {
    fn fold(&self) -> &ReleaseFold {
        self.over.as_ref().unwrap_or(&self.base.fold)
    }
}

/// The refusal for a script with no anchor: nothing can be accepted.
fn no_anchor() -> graftyard_release::Refusal {
    graftyard_release::Refusal::BadGenesis("this site script has no trust anchor".into())
}

impl Scratch for ReleaseScratch<'_> {
    type Event = Accepted;
    fn apply(&mut self, ev: &Accepted) {
        let mut f = self.fold().clone();
        f.apply(&ev.release(), self.at);
        self.over = Some(f);
    }
    fn check(&self, ev: &Accepted) -> Result<(), Box<str>> {
        let anchor = self.base.anchor.ok_or_else(|| no_anchor().to_string())?;
        self.fold().check(&anchor, self.at, &ev.release()).map_err(|e| e.to_string().into())
    }
}

impl ReleaseState {
    fn apply_at(&mut self, ev: &Accepted, at: Timestamp) {
        self.fold.apply(&ev.release(), at);
        self.entries.push(ev.0.clone().into_boxed_slice());
    }
}

impl Fold for ReleaseState {
    type Event = Accepted;
    type Scratch<'a> = ReleaseScratch<'a>;
    fn scratch(&self) -> ReleaseScratch<'_> {
        ReleaseScratch { base: self, over: None, at: Timestamp(0) }
    }
    fn scratch_at(&self, at: Timestamp, _tiers: TierSet) -> ReleaseScratch<'_> {
        ReleaseScratch { base: self, over: None, at }
    }
    /// Only through `apply_entry`, which has the entry's time.
    fn apply(&mut self, ev: &Accepted) {
        self.apply_at(ev, Timestamp(0));
    }
    fn apply_entry(&mut self, e: &LogEntry<Accepted>) {
        if let EntryBody::Events(evs) = &e.body {
            for ev in evs {
                self.apply_at(ev, e.at);
            }
        }
    }
}

/// A signed entry, as `POST /api/graftyard/v1/release/entries` received it. JSON form: the bytes as hex.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submit(pub Vec<u8>);

impl Serialize for Submit {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for Submit {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Submit, D::Error> {
        let s = String::deserialize(d)?;
        unhex(&s).map(Submit).ok_or_else(|| serde::de::Error::custom("a Release entry is hex"))
    }
}

impl Record for Submit {
    fn encode(&self, out: &mut Vec<u8>) {
        Enc(out).bytes(&self.0);
    }
}

impl Command for Submit {
    type Fold = ReleaseState;
    type Dep = ();
    type Out = Digest;
    type Err = graftyard_release::Refusal;

    fn format(&self) -> Format {
        match graftyard_release::SignedEntry::decode(&self.0).map(|e| e.body) {
            Ok(graftyard_release::ReleaseEntry::Genesis(_)) | Err(_) => RELEASE_FORMAT,
            Ok(_) => RELEASE_FORMAT_2,
        }
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, _fold: &ReleaseState) -> SmallVec<[ChainId; 3]> {
        SmallVec::new()
    }
    /// `ReleaseFold::decide` on the fold plus the earlier commands of the entry, against the anchor.
    fn decide(
        &self,
        s: &ReleaseScratch<'_>,
        _deps: &[(ChainId, ())],
    ) -> Result<(SmallVec<[Ev<Self>; 4]>, Digest), graftyard_release::Refusal> {
        let anchor = s.base.anchor.ok_or_else(no_anchor)?;
        let a = s.fold().decide(&anchor, s.at, &self.0)?;
        Ok((smallvec![Accepted(a.bytes.to_vec())], a.digest()))
    }
    fn replay(&self, events: &[Accepted]) -> Digest {
        events.first().map(|e| e.release().digest()).unwrap_or(Digest([0; 32]))
    }
}

/// The `Release` family. Accepted entries are stored as `release_entry` rows, one per entry, and the fold is rebuilt
/// from them on restore.
pub struct Release;

impl Family for Release {
    type Ev = Accepted;
    type Fold = ReleaseState;
    type C = Submit;

    fn reads() -> Vec<Format> {
        vec![RELEASE_FORMAT, RELEASE_FORMAT_2]
    }
    fn new_fold(_sql: &SqlStorage, _join: &Join) -> ReleaseState {
        ReleaseState { fold: ReleaseFold::default(), anchor: cached_anchor(), entries: Vec::new() }
    }
    fn store_fold(_fold: &ReleaseState, entries: &[&raft::LogEntry<Accepted>], stmts: &mut Vec<Stmt>) {
        for e in entries {
            if let raft::EntryBody::Events(evs) = &e.body {
                for (k, ev) in evs.iter().enumerate() {
                    stmts.push((
                        "INSERT OR REPLACE INTO release_entry (idx, k, bytes) VALUES (?, ?, ?)",
                        vec![crate::host::i(e.index.0), crate::host::i(k as u64), ev.0.clone().into()],
                    ));
                    stmts.push((
                        "INSERT OR REPLACE INTO release_at (idx, at) VALUES (?, ?)",
                        vec![crate::host::i(e.index.0), crate::host::i(e.at.0)],
                    ));
                }
            }
        }
    }
    fn load_fold(sql: &SqlStorage, join: &Join) -> worker::Result<ReleaseState> {
        let mut f = Self::new_fold(sql, join);
        // A row written before `release_at` existed is the genesis, whose apply reads no time, so 0 restores the
        // same fold the log replays.
        let q = "SELECT e.bytes, COALESCE(a.at, 0) FROM release_entry e LEFT JOIN release_at a ON a.idx = e.idx \
                 ORDER BY e.idx, e.k";
        for r in rows(sql, q, vec![])? {
            f.apply_at(&Accepted(blob(&r[0])?.to_vec()), Timestamp(crate::host::int(&r[1])? as u64));
        }
        Ok(f)
    }
    fn out_json(_fold: &Self::Fold, a: &raft::Acked<Digest>) -> Value {
        json!({ "digest": a.out.hex() })
    }
    /// 409 with the current digest for a stale `prev`, a second genesis included; 422 for every other refusal
    /// (`design/DESIGN.md`, Shape, the release endpoint).
    fn err_json(e: &graftyard_release::Refusal) -> Value {
        let mut v = json!({ "err": e.variant(), "message": e.to_string(), "status": e.status() });
        if let Some(d) = e.current() {
            v["prev"] = json!(d.hex());
        }
        v
    }
    /// The view: the genesis's bytes, the last digest, the `Keys` generation, the current intent and its state, site
    /// modes, pointers, and enabled formats. `{"after": n}` adds up to 100 entries from position `n` (0 is the
    /// genesis), hex.
    fn read_json(fold: &ReleaseState, q: &Value) -> Value {
        let f = &fold.fold;
        let mut v = json!({
            "genesis": fold.entries.first().map(|b| hex(b)),
            "last": f.last.map(|d| d.hex()),
            "entries": fold.entries.len(),
            "keys": f.keys.as_ref().map(|k| json!({
                "generation": k.generation,
                "customer": k.customer.hex(),
                "vendor": k.vendor.map(|x| x.hex()),
                "release": k.release.hex(),
                "drain": k.drain.hex(),
                "executor": k.executor.hex(),
                "agents_executor": k.agents_executor.map(|x| x.hex()),
                "ring": k.ring.iter().map(|(s, x)| json!([s.0, x.hex()])).collect::<Vec<_>>(),
            })),
            "current": f.current.as_ref().map(|c| json!({
                "intent": c.intent.id.0.to_string(),
                "kind": c.intent.intent.kind(),
                "state": format!("{:?}", c.state),
                "next": c.next,
            })),
            "drained": f.drained.iter().map(|(s, d)| json!({ "site": s, "drain_key": d.by_drain_key, "since": d.since.0 })).collect::<Vec<_>>(),
            "uploads": f.uploads.keys().map(|r| hex(r)).collect::<Vec<_>>(),
            "enabled": f.enabled.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
            "pointers": {
                "edge": f.pointers.edge.map(|v| v.to_string()),
                "ops": f.pointers.ops.map(|v| v.to_string()),
                "mcp": f.pointers.mcp.map(|v| v.to_string()),
                "controller": f.pointers.controller.map(|v| v.to_string()),
            },
        });
        if let Some(after) = q.get("after").and_then(Value::as_u64) {
            v["page"] = fold.entries.iter().skip(after as usize).take(100).map(|b| hex(b)).collect();
        }
        v
    }
    fn fold_bytes(_fold: &ReleaseState) -> Vec<u8> {
        Vec::new()
    }
    fn dep(chain: ChainId, _bytes: &[u8]) -> Result<(), String> {
        Err(format!("the Release group reads no dependency ({chain})"))
    }
    fn source(_join: &Join) -> Source {
        Source::Anchor
    }
}

pub const RELEASE_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS release_entry (idx INTEGER NOT NULL, k INTEGER NOT NULL, bytes BLOB NOT NULL, PRIMARY KEY (idx, k))";
/// Each `release_entry` row's entry time, which `apply` reads (a drain's `since`). A table of its own, so a member
/// stored before it existed needs no migration: its rows are the genesis alone.
pub const RELEASE_AT_SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS release_at (idx INTEGER PRIMARY KEY, at INTEGER NOT NULL)";
