//! `colo-gate` (`design/DESIGN.md`, "Follower read" and Shape's git caches paragraph; `decisions/DECISIONS.md`,
//! "`colo-gate`"): the read gate's `Allow` decisions and checked chain-pack header sets, shared between the edge
//! isolates of one data center through the Workers Cache API.
//!
//! A shared decision carries the time of the oldest fold read it was made on, and is served only while that time is
//! under `READ_CACHE` less `COLO_SKEW`, so a revocation binds within one `READ_CACHE` whichever isolate holds it. Only
//! `Allow` is put; a miss, a refusal, an error, or a match not answered within `COLO_MATCH_WITHIN` reads fresh. A request
//! served from an entry at least `COLO_AHEAD` old starts one fresh gate read after its answer, whose `Allow` replaces the
//! entry and whose refusal is not put. `gate` and `reread` hold those rules over `GateFx`, so they run on the served path
//! natively; the rest of this file is the Cache API glue.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use graftyard_identity::{Decision, READ_CACHE};
use graftyard_types::{NetworkId, PlacementRecord, SiteId, Timestamp};
use sha2::{Digest, Sha256};

/// Taken off `READ_CACHE` for an entry another isolate stamped, whose clock is another machine's [guess].
pub(crate) const COLO_SKEW: Duration = Duration::from_secs(1);
/// The longest a data-center match may take before the gate reads members as before [guess].
pub(crate) const COLO_MATCH_WITHIN: Duration = Duration::from_millis(50);
/// A request served from an entry at least this old starts one fresh gate read after its answer [guess].
pub(crate) const COLO_AHEAD: Duration = Duration::from_secs(5);
/// The keys' version: an edge whose key rule changes bumps it, so it never reads another rule's entries.
const KEY_VERSION: u8 = 1;
/// The keys' origin: fixed, since through a service binding a request's URL carries the caller's hostname, and no
/// request ever reaches it (`tests/am1/results/b034-colo.md`).
const ORIGIN: &str = "https://colo.graftyard.invalid";

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Whether a decision stamped `at` may be served at `now`. One this isolate read itself (`foreign` false) keeps the
/// isolate cache's rule, at most `READ_CACHE` old; one another isolate stamped must be under `READ_CACHE` less
/// `COLO_SKEW`, and never stamped past `now` by more than the skew.
pub(crate) fn servable(at: Timestamp, now: Timestamp, foreign: bool) -> bool {
    let age = now.0.saturating_sub(at.0);
    if foreign {
        at.0 <= now.0.saturating_add(ms(COLO_SKEW)) && age < ms(READ_CACHE) - ms(COLO_SKEW)
    } else {
        age <= ms(READ_CACHE)
    }
}

/// Where a gate decision came from, with its age in ms for `server-timing` (`gate_iso`, `gate_colo`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Served {
    Read,
    Isolate(u64),
    Colo(u64),
}

/// A gate's answer: the decision, where it came from, whether a re-read ahead is due, and the rows the fill that made
/// the decision read beside it (`GateFx::Rows`), never another fill's.
#[derive(Debug, PartialEq)]
pub(crate) struct Gated<E, R> {
    pub(crate) decision: Result<Decision, E>,
    pub(crate) served: Served,
    pub(crate) ahead: bool,
    pub(crate) rows: Option<R>,
}

/// An isolate entry: the decision's stamp, the decision, whether another isolate stamped it, and its fill's rows.
pub(crate) type Held<R> = (Timestamp, Decision, bool, Option<R>);
/// A fresh read's answer: the decision, its stamp (`None`: not to be cached), and the rows read with it.
pub(crate) type Fill<R> = (Decision, Option<Timestamp>, Option<R>);

/// What the gate does to the world: its clock, the isolate's entry, the data center's entry, and a fresh read, which
/// answers the decision and the time of the oldest fold read under it (`None`: not to be cached, as for a repository
/// whose group is not created yet).
#[allow(async_fn_in_trait)]
pub(crate) trait GateFx {
    type Err;
    /// What a fresh read read beside its decision: a push's agent rows (`agent-rows-cache`). They are held in the
    /// decision's own entry and served only with it, so a cached push gate needs no read of its own, and rows never
    /// outlive or come from another fill than the decision they ride with. A data-center copy carries none.
    type Rows: Clone;
    fn now(&self) -> Timestamp;
    /// The isolate's entry, at any age: the decision, its stamp, whether another isolate stamped it, and its rows.
    fn held(&self) -> Option<Held<Self::Rows>>;
    fn hold(&self, at: Timestamp, decision: Decision, foreign: bool, rows: Option<Self::Rows>);
    /// The data center's entry's stamp, if one answered within `COLO_MATCH_WITHIN`.
    async fn matched(&self) -> Option<Timestamp>;
    async fn share(&self, at: Timestamp);
    async fn read(&self) -> Result<Fill<Self::Rows>, Self::Err>;
    /// Whether the last fresh read also found the caller may push to the repository (`Share::push`).
    fn writable(&self) -> bool;
    /// The decision on the access rows read beside the front on the guessed repository (`beside`), kept only when
    /// the resolve named that repository: when those rows' read started, and the decision with the stamp a fresh read
    /// at the gate would give it. `None` when no rows were read beside, or they were dropped.
    async fn beside(&self) -> Option<(Timestamp, Result<(Decision, Option<Timestamp>), Self::Err>)>;
}

/// The gate's fresh decision: on the rows read beside the front when they allow, stamped no later than their read's
/// start, since the front ran between that read and the gate; else a fresh read. A refusal on them, or a repository
/// not created yet, is read again fresh, as a refusal on a cached fold is (`beside`). Rows read beside the front are
/// access rows only, so that decision carries no `Rows`.
async fn fresh<F: GateFx>(fx: &F) -> Result<Fill<F::Rows>, F::Err> {
    match fx.beside().await {
        Some((start, Ok((Decision::Allow, Some(at))))) => Ok((Decision::Allow, Some(at.min(start)), None)),
        Some((_, Err(e))) => Err(e),
        _ => fx.read().await,
    }
}

/// How a gate uses the data center's entries. A fetch both matches and puts, and re-reads ahead (`READ`). A
/// receive-pack advertisement only puts, and only an `Allow` its fresh read found it may also push on (`ADVERT`): it
/// sends refs, so it decides on its own read. A receive-pack POST only matches (`POST`): its leader's `decide`
/// re-reads the credential and the owner's `Authz` and binds, and nothing reaches the client before it, so it serves
/// from an advertisement's entry under the same stamp rule and never puts or re-reads ahead (`cold-write`, C). The
/// two receive-pack uses key their entries apart from a fetch's (`crate::gate::push_key`), so a fetch's `Allow`
/// never serves a POST. Anything else, a session, or the test arm with the share off, uses neither (`OFF`).
/// Under a receive-pack's key (`push`), a fresh `Allow` is held or put only when that read found the caller may also
/// push (`GateFx::writable`), so a read-only caller's `Allow` never serves a POST. An advertisement served from its
/// isolate's own read puts that read's stamp, so a POST's earlier read on the isolate never keeps the entry from the
/// next POST on another isolate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Share {
    pub(crate) matches: bool,
    pub(crate) puts: bool,
    pub(crate) push: bool,
}

pub(crate) const READ: Share = Share { matches: true, puts: true, push: false };
pub(crate) const ADVERT: Share = Share { matches: false, puts: true, push: true };
pub(crate) const POST: Share = Share { matches: true, puts: false, push: true };
pub(crate) const OFF: Share = Share { matches: false, puts: false, push: false };

/// The read gate on the served path, sharing as `share` says; with `OFF` it keeps the isolate cache alone, as before
/// `colo-gate`.
pub(crate) async fn gate<F: GateFx>(fx: &F, share: Share) -> Gated<F::Err, F::Rows> {
    let ahead_ok = share == READ;
    let now = fx.now();
    // A copy a POST took of the data center's entry never answers an advertisement, which decides on its own
    // isolate's reads.
    if let Some((at, d, foreign, rows)) = fx.held()
        && servable(at, now, foreign)
        && !(share.push && !share.matches && foreign)
    {
        let ahead = ahead_ok && d == Decision::Allow && now.0.saturating_sub(at.0) >= ms(COLO_AHEAD);
        if share.push && share.puts && d == Decision::Allow && !foreign {
            fx.share(at).await;
        }
        return Gated { decision: Ok(d), served: Served::Isolate(now.0.saturating_sub(at.0)), ahead, rows };
    }
    if share.matches
        && let Some(at) = fx.matched().await
    {
        let now = fx.now();
        if servable(at, now, true) {
            // The copy keeps the entry's stamp, so the isolate never serves it longer than the data center would. It
            // replaces the isolate's entry whole: the rows of the fill it replaces go with it.
            fx.hold(at, Decision::Allow, true, None);
            let age = now.0.saturating_sub(at.0);
            let ahead = ahead_ok && age >= ms(COLO_AHEAD);
            return Gated { decision: Ok(Decision::Allow), served: Served::Colo(age), ahead, rows: None };
        }
    }
    let (decision, rows) = match fresh(fx).await {
        Ok((d, Some(at), rows)) => {
            let keep = d != Decision::Allow || !share.push || fx.writable();
            if keep {
                fx.hold(at, d, false, rows.clone());
            }
            if share.puts && d == Decision::Allow && keep {
                fx.share(at).await;
            }
            (Ok(d), rows)
        }
        Ok((d, None, rows)) => (Ok(d), rows),
        Err(e) => (Err(e), None),
    };
    Gated { decision, served: Served::Read, ahead: false, rows }
}

/// The re-read ahead, after a request's answer: a fresh read whose `Allow` replaces both entries with its own stamp;
/// a refusal is held in the isolate and not put, so the data center's entry lapses on its own stamp.
pub(crate) async fn reread<F: GateFx>(fx: &F) {
    if let Ok((d, Some(at), rows)) = fx.read().await {
        fx.hold(at, d, false, rows);
        if d == Decision::Allow {
            fx.share(at).await;
        }
    }
}

/// The data center's key for a gate decision: a SHA-256 of the key version, the credential (`gate::Key`'s hash of a
/// stored token or `ghs_` `jti`, or none; never a session, never a raw token), the repository, and the placement
/// record.
pub(crate) fn gate_url(
    repo: graftyard_types::RepoId,
    credential: Option<[u8; 32]>,
    record: &PlacementRecord,
) -> String {
    let mut h = Sha256::new();
    h.update(b"graftyard-colo-gate\0");
    h.update([KEY_VERSION]);
    h.update(repo.0.0.to_be_bytes());
    match credential {
        Some(c) => {
            h.update([1]);
            h.update(c);
        }
        None => h.update([0]),
    }
    h.update(serde_json::to_vec(record).unwrap_or_default());
    format!("{ORIGIN}/gate/{}", graftyard_lookup::hex(&h.finalize()))
}

/// The data center's key for a chain bucket's header set: the bucket's site, the network, and the version of the
/// bucket's `head` object it was checked under, which every put of `head` changes.
pub(crate) fn chain_url(site: SiteId, network: NetworkId, version: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"graftyard-colo-chain\0");
    h.update([KEY_VERSION]);
    h.update(site.0.to_be_bytes());
    h.update(network.0.0.to_be_bytes());
    h.update(version.as_bytes());
    format!("{ORIGIN}/chain/{}", graftyard_lookup::hex(&h.finalize()))
}

/// The data center's key for a link guess (`beside`): the owner's login and the repository's name, both in their
/// compared forms. Its value names the repository, its owner and placement record that a fresh `resolve` last
/// answered, a guess at any age that addresses reads and never answers one; a guess for another login or name is
/// under another key, and one for another repository, owner or record is not kept (`crate::beside_rows`).
pub(crate) fn link_url(login: &graftyard_identity::LoginKey, name: &graftyard_identity::RepoNameKey) -> String {
    let mut h = Sha256::new();
    h.update(b"graftyard-colo-link\0");
    h.update([KEY_VERSION]);
    h.update((login.as_str().len() as u32).to_be_bytes());
    h.update(login.as_str().as_bytes());
    h.update(name.as_str().as_bytes());
    format!("{ORIGIN}/link/{}", graftyard_lookup::hex(&h.finalize()))
}

/// How long the data center keeps a link guess. A moved or renamed repository costs one read again until it lapses.
const LINK_KEEP: Duration = Duration::from_secs(3600);

/// The data center's link guess at `url`.
pub(crate) async fn match_link(url: &str) -> Option<crate::Repo> {
    let v: serde_json::Value = serde_json::from_str(&match_text(url).await?).ok()?;
    Some(crate::Repo {
        id: serde_json::from_value(v["id"].clone()).ok()?,
        owner: serde_json::from_value(v["owner"].clone()).ok()?,
        record: serde_json::from_value(v["record"].clone()).ok()?,
    })
}

/// Puts the repository a fresh `resolve` answered as `url`'s link guess.
pub(crate) async fn put_link(url: String, r: crate::Repo) {
    let body = serde_json::json!({ "id": r.id, "owner": r.owner, "record": r.record }).to_string();
    put_text(url, body, LINK_KEEP).await;
}

thread_local! {
    /// Gate keys with a re-read ahead in flight in this isolate: at most one each.
    static REREADING: RefCell<BTreeSet<crate::gate::Key>> = const { RefCell::new(BTreeSet::new()) };
    /// Header sets this isolate holds, by `chain_url`.
    static SETS: RefCell<BTreeMap<String, graftyard_git::compose::HeaderSet>> = const { RefCell::new(BTreeMap::new()) };
}

/// The most header sets one isolate keeps; past it the map starts empty again.
const SETS_MAX: usize = 256;

/// Starts `k`'s re-read ahead, unless one is in flight here.
pub(crate) fn begin(k: crate::gate::Key) -> bool {
    REREADING.with(|r| r.borrow_mut().insert(k))
}

pub(crate) fn end(k: &crate::gate::Key) {
    REREADING.with(|r| r.borrow_mut().remove(k));
}

/// The Cache API's default cache, where the runtime has one.
fn cache() -> Option<worker::Cache> {
    js_sys::Reflect::has(&js_sys::global(), &"caches".into()).ok()?.then(worker::Cache::default)
}

async fn match_text(url: &str) -> Option<String> {
    let c = cache()?;
    let got = graftyard_git::compose::within(COLO_MATCH_WITHIN, c.get(url, false), worker::Delay::from).await?;
    got.ok()??.text().await.ok()
}

async fn put_text(url: String, body: String, max_age: Duration) {
    let Some(c) = cache() else { return };
    let Ok(mut r) = worker::Response::ok(body) else { return };
    if r.headers_mut().set("cache-control", &format!("max-age={}", max_age.as_secs())).is_err() {
        return;
    }
    let _ = c.put(url.as_str(), r).await;
}

/// The data center's gate entry at `url`: its stamp, and a fork's network (`gate_entry`).
pub(crate) async fn match_gate(url: &str) -> Option<(Timestamp, Option<NetworkId>)> {
    gate_entry(&match_text(url).await?)
}

/// Puts an `Allow` stamped `at` under `url`, with a fork's network (`gate_body`).
pub(crate) async fn put_gate(url: String, at: Timestamp, net: Option<NetworkId>) {
    put_text(url, gate_body(at, net), READ_CACHE).await;
}

/// A gate entry's body: `{"at"}`, and `net` only for a fork, so a root's entry keeps its bytes and a data-center copy
/// of a fork's decision carries the network its fill learned (`decisions/DECISIONS.md`, "`fork-network-read`").
fn gate_body(at: Timestamp, net: Option<NetworkId>) -> String {
    match net {
        None => serde_json::json!({ "at": at.0 }).to_string(),
        Some(n) => serde_json::json!({ "at": at.0, "net": n }).to_string(),
    }
}

fn gate_entry(text: &str) -> Option<(Timestamp, Option<NetworkId>)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    Some((Timestamp(v["at"].as_u64()?), serde_json::from_value(v["net"].clone()).ok().flatten()))
}

/// A chain bucket's header sets: the isolate's, then the data center's, put to both once every check passed
/// (`graftyard_git::compose::SetStore`). `share` false keeps them in the isolate alone.
pub(crate) struct ChainSets<'a> {
    pub(crate) c: &'a crate::Ctx<'a>,
    pub(crate) network: NetworkId,
    pub(crate) sites: Vec<SiteId>,
    pub(crate) share: bool,
}

impl ChainSets<'_> {
    fn url(&self, bucket: usize, version: &str) -> Option<String> {
        self.sites.get(bucket).map(|s| chain_url(*s, self.network, version))
    }
}

impl graftyard_git::compose::SetStore for ChainSets<'_> {
    fn enabled(&self) -> bool {
        true
    }
    async fn get(&self, bucket: usize, version: &str) -> Option<graftyard_git::compose::HeaderSet> {
        let url = self.url(bucket, version)?;
        if let Some(s) = SETS.with(|m| m.borrow().get(&url).cloned()) {
            return Some(s);
        }
        if !self.share {
            return None;
        }
        let s: graftyard_git::compose::HeaderSet = serde_json::from_str(&match_text(&url).await?).ok()?;
        hold_set(url, s.clone());
        Some(s)
    }

    fn put(&self, bucket: usize, version: &str, set: graftyard_git::compose::HeaderSet) {
        let Some(url) = self.url(bucket, version) else { return };
        if self.share
            && let Ok(body) = serde_json::to_string(&set)
        {
            let at = url.clone();
            self.c.ctx.wait_until(put_text(at, body, Duration::from_secs(3600)));
        }
        hold_set(url, set);
    }

    fn served(&self, s: graftyard_git::compose::Served) {
        self.c.timing.put("chain_set", s.set as f64);
        self.c.timing.put("chain_head", s.head as f64);
        self.c.timing.put("chain_chk", s.checked as f64);
    }
}

fn hold_set(url: String, set: graftyard_git::compose::HeaderSet) {
    SETS.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= SETS_MAX {
            m.clear();
        }
        m.insert(url, set);
    });
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use futures_util::FutureExt;
    use graftyard_identity::Decision::{Allow, Hidden};
    use graftyard_types::{DbId, PlacementName, RepoId};

    use super::*;

    /// One isolate's view of a shared world: a clock, a data-center entry, and a grant that holds until `revoked`.
    struct World {
        now: Cell<u64>,
        colo: RefCell<Option<Timestamp>>,
        revoked: Cell<Option<u64>>,
        read_ms: u64,
        /// The caller may read but not push: every fresh read allows a fetch only.
        read_only: Cell<bool>,
    }

    struct Isolate<'w> {
        w: &'w World,
        /// The entry, with its fill's rows: here the start of the read that filled it.
        held: RefCell<Option<Held<u64>>>,
        /// Rows read beside the front: their read's start, and whether they granted (`Err`: the read failed).
        beside: RefCell<Option<(u64, Result<bool, ()>)>>,
        reads: Cell<u32>,
    }

    impl GateFx for Isolate<'_> {
        type Err = String;
        type Rows = u64;
        fn now(&self) -> Timestamp {
            Timestamp(self.w.now.get())
        }
        fn held(&self) -> Option<Held<u64>> {
            *self.held.borrow()
        }
        fn hold(&self, at: Timestamp, d: Decision, foreign: bool, rows: Option<u64>) {
            *self.held.borrow_mut() = Some((at, d, foreign, rows));
        }
        async fn matched(&self) -> Option<Timestamp> {
            *self.w.colo.borrow()
        }
        async fn share(&self, at: Timestamp) {
            *self.w.colo.borrow_mut() = Some(at);
        }
        /// The fold read starts now, takes `read_ms`, and is stamped at its start; its rows are named by that start.
        async fn read(&self) -> Result<Fill<u64>, String> {
            self.reads.set(self.reads.get() + 1);
            let start = self.w.now.get();
            self.w.now.set(start + self.w.read_ms);
            let granted = self.w.revoked.get().is_none_or(|t| start < t);
            Ok((if granted { Allow } else { Hidden }, Some(Timestamp(start)), Some(start)))
        }
        /// Rows read beside the front from `beside`'s start, decided at the gate as a fresh read there would stamp
        /// it (now), or the error they answered.
        async fn beside(&self) -> Option<(Timestamp, Result<(Decision, Option<Timestamp>), String>)> {
            let (start, rows) = self.beside.borrow_mut().take()?;
            let d = |granted| (if granted { Allow } else { Hidden }, Some(Timestamp(self.w.now.get())));
            Some((Timestamp(start), rows.map(d).map_err(|()| "shed".to_string())))
        }
        fn writable(&self) -> bool {
            !self.w.read_only.get()
        }
    }

    fn world(read_ms: u64) -> World {
        World {
            now: Cell::new(0),
            colo: RefCell::new(None),
            revoked: Cell::new(None),
            read_ms,
            read_only: Cell::new(false),
        }
    }

    fn iso(w: &World) -> Isolate<'_> {
        Isolate { w, held: RefCell::new(None), beside: RefCell::new(None), reads: Cell::new(0) }
    }

    impl Isolate<'_> {
        /// The entry without its rows.
        fn entry(&self) -> Option<(Timestamp, Decision, bool)> {
            self.held().map(|(at, d, f, _)| (at, d, f))
        }
    }

    fn at(w: &World, t: u64) {
        w.now.set(t);
    }

    fn run<T>(f: impl std::future::Future<Output = T>) -> T {
        f.now_or_never().expect("ready")
    }

    /// The bound: an entry this isolate read is served while at most `READ_CACHE` old, as before; one another isolate
    /// stamped only while under `READ_CACHE` less `COLO_SKEW`, and never one stamped past now by more than the skew.
    #[test]
    fn a_shared_decision_is_served_only_while_its_stamp_is_under_read_cache_less_the_skew() {
        let limit = ms(READ_CACHE) - ms(COLO_SKEW);
        assert_eq!(limit, 9_000);
        let t = |v| Timestamp(v);
        assert!(servable(t(1_000), t(1_000 + limit - 1), true));
        assert!(!servable(t(1_000), t(1_000 + limit), true), "a foreign stamp at READ_CACHE less the skew");
        assert!(servable(t(1_000), t(1_000 + ms(READ_CACHE)), false), "an own read keeps the isolate cache's rule");
        assert!(!servable(t(1_000), t(1_001 + ms(READ_CACHE)), false));
        assert!(servable(t(2_000), t(1_000), true), "a stamp ahead by the skew");
        assert!(!servable(t(2_001), t(1_000), true), "a stamp from another isolate's clock past the skew");
    }

    /// The served path, step by step: the shared entry is stamped at its fold read's start, not its put; an isolate's
    /// copy keeps that stamp; it lapses at READ_CACHE less the skew whoever holds it; a re-read ahead's `Allow`
    /// replaces the stamp and its refusal leaves the entry alone; a refused fresh read is never put.
    #[test]
    fn the_shared_gate_serves_by_the_fold_reads_stamp_on_every_path() {
        let w = world(600);
        let (a, b) = (iso(&w), iso(&w));
        // Isolate a reads fresh at 1,000; the read takes 600 ms.
        at(&w, 1_000);
        let g = run(gate(&a, READ));
        assert_eq!((g.decision, g.served, g.ahead), (Ok(Allow), Served::Read, false));
        assert_eq!(*w.colo.borrow(), Some(Timestamp(1_000)), "stamped at the fold read's start, not at its put");
        assert_eq!(a.entry(), Some((Timestamp(1_000), Allow, false)));
        // Isolate b matches at 5,000 and copies the entry with its stamp.
        at(&w, 5_000);
        let g = run(gate(&b, READ));
        assert_eq!((g.decision, g.served, g.ahead), (Ok(Allow), Served::Colo(4_000), false));
        assert_eq!(b.entry(), Some((Timestamp(1_000), Allow, true)), "the copy keeps the entry's stamp");
        // At 9,999 b serves its copy and a re-read ahead is due; at 10,000 the copy and the entry have both lapsed.
        at(&w, 9_999);
        let g = run(gate(&b, READ));
        assert_eq!((g.decision, g.served, g.ahead), (Ok(Allow), Served::Isolate(8_999), true));
        at(&w, 10_000);
        let g = run(gate(&b, READ));
        assert_eq!(g.served, Served::Read, "past READ_CACHE less the skew nothing serves it");
        assert_eq!(*w.colo.borrow(), Some(Timestamp(10_000)));
        // A re-read ahead at 15,000 replaces the entry with its own stamp.
        at(&w, 15_000);
        run(reread(&a));
        assert_eq!(*w.colo.borrow(), Some(Timestamp(15_000)), "the re-read's Allow replaces the stamp");
        assert_eq!(a.entry(), Some((Timestamp(15_000), Allow, false)));
        // The grant is removed at 16,000; a re-read at 17,000 refuses, is held in the isolate, and is not put.
        w.revoked.set(Some(16_000));
        at(&w, 17_000);
        run(reread(&a));
        assert_eq!(*w.colo.borrow(), Some(Timestamp(15_000)), "a refused re-read never touches the entry");
        assert_eq!(a.entry(), Some((Timestamp(17_000), Hidden, false)));
        // A fresh read that refuses is never put, and the share off never matches or puts.
        let w = world(100);
        w.revoked.set(Some(0));
        let c = iso(&w);
        at(&w, 1_000);
        assert_eq!(run(gate(&c, READ)).decision, Ok(Hidden));
        assert_eq!(*w.colo.borrow(), None, "a refusal is never put");
        let w = world(100);
        *w.colo.borrow_mut() = Some(Timestamp(900));
        let d = iso(&w);
        at(&w, 1_000);
        let g = run(gate(&d, OFF));
        assert_eq!(g.served, Served::Read, "the share off reads members as before");
        assert_eq!(*w.colo.borrow(), Some(Timestamp(900)), "and puts nothing");
    }

    /// A link guess's key names the login and the name in their compared forms, so `O/R` and `o/r` share one, and
    /// no other login or name, nor a split that moves a character between them, shares it.
    #[test]
    fn a_link_key_names_the_login_and_the_repository_name() {
        let k = |l: &str, n: &str| {
            link_url(
                &graftyard_identity::Login::parse(l).unwrap().key(),
                &graftyard_identity::RepoName::parse(n).unwrap().key(),
            )
        };
        assert_eq!(k("Octo", "Repo"), k("octo", "repo"));
        assert_ne!(k("octo", "repo"), k("octo", "repo2"));
        assert_ne!(k("octo", "repo"), k("octa", "repo"));
        assert_ne!(k("octo", "arepo"), k("octoa", "repo"));
        assert!(k("octo", "repo").starts_with("https://colo.graftyard.invalid/link/"));
    }

    /// `beside`: the rows read beside the front decide at the gate, stamped at their read's start, not at the gate
    /// after the front; a refusal on them is read again fresh and never put; an error on them fails the gate as a
    /// fresh read's would; and a removed grant is never served past READ_CACHE from the read that preceded it.
    #[test]
    fn a_gate_on_rows_read_beside_the_front_keeps_their_reads_stamp_and_rereads_a_refusal() {
        let w = world(600);
        let a = iso(&w);
        // The rows' read started at 1,000; the front (1.2 s cold) ends at 2,200, when the gate runs.
        *a.beside.borrow_mut() = Some((1_000, Ok(true)));
        at(&w, 2_200);
        let g = run(gate(&a, READ));
        assert_eq!((g.decision, g.served), (Ok(Allow), Served::Read));
        assert_eq!(a.reads.get(), 0, "the rows read beside are the gate's read");
        assert_eq!(a.entry(), Some((Timestamp(1_000), Allow, false)), "stamped at the rows' read, not at the gate");
        assert_eq!(*w.colo.borrow(), Some(Timestamp(1_000)));
        // The grant is removed at 1,500: the entry from the read at 1,000 lapses for every isolate by 10,000.
        w.revoked.set(Some(1_500));
        let b = iso(&w);
        at(&w, 9_999);
        assert_eq!(run(gate(&b, READ)).served, Served::Colo(8_999));
        let b = iso(&w);
        at(&w, 10_000);
        assert_eq!(run(gate(&b, READ)).decision, Ok(Hidden), "past READ_CACHE less the skew from the rows' read");
        at(&w, 11_000);
        assert_eq!(run(gate(&a, READ)).served, Served::Isolate(10_000));
        at(&w, 11_001);
        assert_eq!(run(gate(&a, READ)).decision, Ok(Hidden), "the isolate's own copy lapses past READ_CACHE");
        // Rows read after the removal refuse: the gate reads again fresh, refuses, and puts nothing.
        let w = world(100);
        w.revoked.set(Some(500));
        let c = iso(&w);
        *c.beside.borrow_mut() = Some((1_000, Ok(false)));
        at(&w, 2_000);
        assert_eq!(run(gate(&c, READ)).decision, Ok(Hidden));
        assert_eq!((c.reads.get(), *w.colo.borrow()), (1, None), "a refusal on the rows read beside is read fresh");
        // A grant given since the rows' read opens on the fresh read.
        let w = world(100);
        w.revoked.set(Some(0));
        let d = iso(&w);
        *d.beside.borrow_mut() = Some((1_000, Ok(false)));
        at(&w, 2_000);
        w.revoked.set(None);
        assert_eq!((run(gate(&d, READ)).decision, d.reads.get()), (Ok(Allow), 1));
        // An error on the rows read beside fails the gate; it is not hidden by another read.
        let w = world(100);
        let e = iso(&w);
        *e.beside.borrow_mut() = Some((1_000, Err(())));
        at(&w, 2_000);
        assert_eq!((run(gate(&e, READ)).decision, e.reads.get()), (Err("shed".into()), 0));
    }

    /// A revocation: the grant is removed at t. Five isolates serve a request every 700 ms each way it can be served,
    /// with every re-read ahead run as soon as it is due. A fresh read after t refuses, no request after
    /// t + READ_CACHE is served `Allow` from any entry, and the shared entry is never stamped at or after t.
    #[test]
    fn a_revoked_grant_is_never_served_past_read_cache_from_any_entry() {
        let w = world(300);
        let isolates: Vec<Isolate<'_>> = (0..5).map(|_| iso(&w)).collect();
        let t = 15_000;
        w.revoked.set(Some(t));
        let (mut served_after_t, mut late) = (0, Vec::new());
        let mut colo_max = 0;
        for (n, now) in (0..60_000).step_by(700).enumerate() {
            at(&w, now);
            let i = &isolates[n % isolates.len()];
            let g = run(gate(i, READ));
            if g.served == Served::Read && now >= t {
                assert_eq!(g.decision, Ok(Hidden), "a fresh read at {now} after the revocation");
            }
            if g.decision == Ok(Allow) && now >= t {
                served_after_t += 1;
                if now > t + ms(READ_CACHE) {
                    late.push((now, g.served));
                }
            }
            if g.ahead {
                run(reread(i));
            }
            colo_max = colo_max.max(w.colo.borrow().map_or(0, |s| s.0));
        }
        assert!(late.is_empty(), "served Allow past t + READ_CACHE: {late:?}");
        assert!(served_after_t > 0, "entries from before t still serve within the bound");
        assert!(colo_max < t, "the shared entry was stamped at {colo_max}, at or after the revocation at {t}");
    }

    /// `cold-write` C, the modes: an advertisement decides on its own read every time and only puts; a POST serves the
    /// advertisement's entry under the same stamp rule, never puts, and never re-reads ahead; a refusal is never put
    /// for a POST to find.
    #[test]
    fn a_push_post_serves_only_an_advertisements_entry_and_an_advertisement_never_serves_one() {
        let w = world(300);
        let (adv, post) = (iso(&w), iso(&w));
        // An entry already there does not answer an advertisement: it reads fresh, and puts its own stamp.
        *w.colo.borrow_mut() = Some(Timestamp(500));
        at(&w, 1_000);
        let g = run(gate(&adv, ADVERT));
        assert_eq!((g.decision, g.served, adv.reads.get()), (Ok(Allow), Served::Read, 1), "an advertisement reads");
        assert_eq!(*w.colo.borrow(), Some(Timestamp(1_000)), "and puts its fresh Allow");
        // Its own isolate copy still answers its next advertisement within READ_CACHE, as before colo-gate.
        at(&w, 2_000);
        assert_eq!(run(gate(&adv, ADVERT)).served, Served::Isolate(1_000));
        // A POST on a cold isolate serves the entry, with no read, and is never due a re-read ahead.
        at(&w, 7_000);
        let g = run(gate(&post, POST));
        assert_eq!((g.decision, g.served, g.ahead, post.reads.get()), (Ok(Allow), Served::Colo(6_000), false, 0));
        at(&w, 9_500);
        let g = run(gate(&post, POST));
        assert_eq!((g.served, g.ahead), (Served::Isolate(8_500), false), "a POST's copy never re-reads ahead");
        // Past READ_CACHE less the skew the POST reads fresh, and puts nothing.
        let post2 = iso(&w);
        at(&w, 10_000);
        let g = run(gate(&post2, POST));
        assert_eq!((g.served, post2.reads.get()), (Served::Read, 1));
        assert_eq!(*w.colo.borrow(), Some(Timestamp(1_000)), "a POST never puts");
        assert_eq!(post2.entry(), Some((Timestamp(10_000), Allow, false)), "it holds its own read for its next POST");
        // An advertisement on that isolate serves the POST's read and puts its stamp, not now, for a POST elsewhere.
        at(&w, 11_000);
        assert_eq!((run(gate(&post2, ADVERT)).served, post2.reads.get()), (Served::Isolate(1_000), 1));
        assert_eq!(*w.colo.borrow(), Some(Timestamp(10_000)));
        // A copy a POST took from the data center never answers an advertisement: it reads fresh.
        let post3 = iso(&w);
        at(&w, 11_500);
        assert_eq!(run(gate(&post3, POST)).served, Served::Colo(1_500));
        at(&w, 12_000);
        assert_eq!((run(gate(&post3, ADVERT)).served, post3.reads.get()), (Served::Read, 1));
        assert_eq!(*w.colo.borrow(), Some(Timestamp(12_000)));
        // A refused advertisement puts nothing, so a POST after it reads fresh.
        let w = world(100);
        w.revoked.set(Some(0));
        let (adv, post) = (iso(&w), iso(&w));
        at(&w, 1_000);
        assert_eq!(run(gate(&adv, ADVERT)).decision, Ok(Hidden));
        assert_eq!(*w.colo.borrow(), None);
        assert_eq!((run(gate(&post, POST)).decision, post.reads.get()), (Ok(Hidden), 1));
        // A caller who may fetch but not push: its advertisement's `Allow` is neither put nor held, so its next
        // advertisement and a POST each read fresh, while a fetch's own `Allow` is held and put as before.
        let w = world(100);
        w.read_only.set(true);
        let (adv, post) = (iso(&w), iso(&w));
        at(&w, 1_000);
        assert_eq!(run(gate(&adv, ADVERT)).decision, Ok(Allow));
        assert_eq!((*w.colo.borrow(), adv.entry()), (None, None), "a read-only Allow is never a push's entry");
        at(&w, 2_000);
        assert_eq!((run(gate(&adv, ADVERT)).served, adv.reads.get()), (Served::Read, 2));
        assert_eq!((run(gate(&post, POST)).served, post.reads.get(), post.entry()), (Served::Read, 1, None));
        let fetch = iso(&w);
        run(gate(&fetch, READ));
        assert!(w.colo.borrow().is_some() && fetch.entry().is_some(), "a fetch's Allow is kept under its own key");
    }

    /// `agent-rows-cache`: a push gate's entry holds the rows its own fill read. A POST served from the entry gets
    /// those rows with no read; past the decision's `READ_CACHE` both are read again together; a data-center copy
    /// carries no rows and replaces an entry whole, so rows from an older fill never ride another decision; and a
    /// fresh refusal holds that refusing fill's rows, not the last `Allow`'s.
    #[test]
    fn a_push_gate_serves_the_agent_rows_of_its_own_fill_and_no_other() {
        let w = world(300);
        let (adv, other) = (iso(&w), iso(&w));
        // The advertisement reads at 1,000: decision and rows from that one read, and it puts the decision.
        at(&w, 1_000);
        let g = run(gate(&adv, ADVERT));
        assert_eq!((g.decision, g.served, g.rows, adv.reads.get()), (Ok(Allow), Served::Read, Some(1_000), 1));
        // Its POST on the same isolate is served from the entry with the same fill's rows, and reads nothing.
        at(&w, 4_000);
        let g = run(gate(&adv, POST));
        assert_eq!((g.served, g.rows, adv.reads.get()), (Served::Isolate(3_000), Some(1_000), 1), "no second read");
        // A POST on another isolate takes the data center's entry: a decision with no rows, so the push reads its own.
        let g = run(gate(&other, POST));
        assert_eq!((g.served, g.rows, other.reads.get()), (Served::Colo(3_000), None, 0));
        // At READ_CACHE past the fill the entry still serves; one past it, decision and rows are read again together.
        at(&w, 11_000);
        assert_eq!(run(gate(&adv, POST)).rows, Some(1_000));
        at(&w, 11_001);
        let g = run(gate(&adv, POST));
        assert_eq!(
            (g.served, g.rows, adv.reads.get()),
            (Served::Read, Some(11_001), 2),
            "rows never outlive READ_CACHE"
        );
        // An isolate whose own entry, with an older fill's rows, has lapsed takes a newer data-center copy: the copy
        // replaces the entry whole, so the older rows are gone and never serve beside the copy's decision.
        let c = iso(&w);
        c.hold(Timestamp(1), Allow, false, Some(1));
        *w.colo.borrow_mut() = Some(Timestamp(19_500));
        at(&w, 25_000);
        let g = run(gate(&c, POST));
        assert_eq!((g.served, g.rows), (Served::Colo(5_500), None), "a copy carries no rows");
        assert_eq!(c.held(), Some((Timestamp(19_500), Allow, true, None)), "and the old fill's rows are gone");
        // The grant is removed: the next fresh read refuses, and the entry holds that read's rows, not the Allow's.
        let w = world(300);
        let d = iso(&w);
        at(&w, 1_000);
        run(gate(&d, ADVERT));
        w.revoked.set(Some(5_000));
        at(&w, 12_000);
        let g = run(gate(&d, POST));
        assert_eq!((g.decision, g.rows), (Ok(Hidden), Some(12_000)));
        assert_eq!(d.held(), Some((Timestamp(12_000), Hidden, false, Some(12_000))));
    }

    /// A root's entry keeps main's bytes, `{"at":N}`, which an edge before forks reads; a fork's adds `net`, read back
    /// whole, and an older entry reads as no network.
    #[test]
    fn a_gate_entry_carries_a_forks_network_and_a_roots_keeps_its_bytes() {
        let net = NetworkId(graftyard_types::DbId(3));
        assert_eq!(gate_body(Timestamp(1_000), None), r#"{"at":1000}"#);
        assert_eq!(gate_entry(r#"{"at":1000}"#), Some((Timestamp(1_000), None)));
        let fork = gate_body(Timestamp(1_000), Some(net));
        assert_eq!(gate_entry(&fork), Some((Timestamp(1_000), Some(net))));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&fork).unwrap()["at"],
            1_000,
            "an older edge reads its stamp"
        );
        assert_eq!(gate_entry(r#"{"net":3}"#), None);
    }

    /// Every input of the key is in it: the credential, the repository, and the placement record, and the raw
    /// credential bytes never appear in the URL.
    #[test]
    fn a_gate_key_names_the_credential_the_repository_and_the_record() {
        let record = |home: u8, generation: u32| PlacementRecord {
            name: PlacementName::Tri,
            sites: [SiteId(0), SiteId(1), SiteId(2)].into_iter().collect(),
            home: SiteId(home),
            generation,
            moving_from: None,
            at: Timestamp(1),
        };
        let (r, cred) = (RepoId(DbId(7)), Some([9u8; 32]));
        let base = gate_url(r, cred, &record(0, 1));
        assert!(base.starts_with("https://colo.graftyard.invalid/gate/"));
        assert_eq!(base, gate_url(r, cred, &record(0, 1)));
        for other in [
            gate_url(r, Some([8; 32]), &record(0, 1)),
            gate_url(r, None, &record(0, 1)),
            gate_url(RepoId(DbId(8)), cred, &record(0, 1)),
            gate_url(r, cred, &record(1, 1)),
            gate_url(r, cred, &record(0, 2)),
        ] {
            assert_ne!(base, other);
        }
        assert!(!base.contains(&graftyard_lookup::hex(&[9u8; 32])), "the credential is hashed, never in the URL");
        let net = NetworkId(DbId(5));
        assert_ne!(chain_url(SiteId(0), net, "v1"), chain_url(SiteId(0), net, "v2"));
        assert_ne!(chain_url(SiteId(0), net, "v1"), chain_url(SiteId(1), net, "v1"));
        assert_ne!(chain_url(SiteId(0), net, "v1"), chain_url(SiteId(0), NetworkId(DbId(6)), "v1"));
    }
}
