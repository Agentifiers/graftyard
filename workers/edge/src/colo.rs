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

/// A gate's answer: the decision, where it came from, and whether a re-read ahead is due.
#[derive(Debug, PartialEq)]
pub(crate) struct Gated<E> {
    pub(crate) decision: Result<Decision, E>,
    pub(crate) served: Served,
    pub(crate) ahead: bool,
}

/// What the gate does to the world: its clock, the isolate's entry, the data center's entry, and a fresh read, which
/// answers the decision and the time of the oldest fold read under it (`None`: not to be cached, as for a repository
/// whose group is not created yet).
#[allow(async_fn_in_trait)]
pub(crate) trait GateFx {
    type Err;
    fn now(&self) -> Timestamp;
    /// The isolate's entry, at any age: the decision, its stamp, and whether another isolate stamped it.
    fn held(&self) -> Option<(Timestamp, Decision, bool)>;
    fn hold(&self, at: Timestamp, decision: Decision, foreign: bool);
    /// The data center's entry's stamp, if one answered within `COLO_MATCH_WITHIN`.
    async fn matched(&self) -> Option<Timestamp>;
    async fn share(&self, at: Timestamp);
    async fn read(&self) -> Result<(Decision, Option<Timestamp>), Self::Err>;
}

/// The read gate on the served path. `share` is false for a write, a session, or the test arm with the share off,
/// which then keeps the isolate cache alone, as before `colo-gate`.
pub(crate) async fn gate<F: GateFx>(fx: &F, share: bool) -> Gated<F::Err> {
    let now = fx.now();
    if let Some((at, d, foreign)) = fx.held()
        && servable(at, now, foreign)
    {
        let ahead = share && d == Decision::Allow && now.0.saturating_sub(at.0) >= ms(COLO_AHEAD);
        return Gated { decision: Ok(d), served: Served::Isolate(now.0.saturating_sub(at.0)), ahead };
    }
    if share && let Some(at) = fx.matched().await {
        let now = fx.now();
        if servable(at, now, true) {
            // The copy keeps the entry's stamp, so the isolate never serves it longer than the data center would.
            fx.hold(at, Decision::Allow, true);
            let age = now.0.saturating_sub(at.0);
            return Gated { decision: Ok(Decision::Allow), served: Served::Colo(age), ahead: age >= ms(COLO_AHEAD) };
        }
    }
    let decision = match fx.read().await {
        Ok((d, Some(at))) => {
            fx.hold(at, d, false);
            if share && d == Decision::Allow {
                fx.share(at).await;
            }
            Ok(d)
        }
        Ok((d, None)) => Ok(d),
        Err(e) => Err(e),
    };
    Gated { decision, served: Served::Read, ahead: false }
}

/// The re-read ahead, after a request's answer: a fresh read whose `Allow` replaces both entries with its own stamp;
/// a refusal is held in the isolate and not put, so the data center's entry lapses on its own stamp.
pub(crate) async fn reread<F: GateFx>(fx: &F) {
    if let Ok((d, Some(at))) = fx.read().await {
        fx.hold(at, d, false);
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

/// The data center's gate entry at `url`: its stamp.
pub(crate) async fn match_gate(url: &str) -> Option<Timestamp> {
    let v: serde_json::Value = serde_json::from_str(&match_text(url).await?).ok()?;
    v["at"].as_u64().map(Timestamp)
}

/// Puts an `Allow` stamped `at` under `url`.
pub(crate) async fn put_gate(url: String, at: Timestamp) {
    put_text(url, serde_json::json!({ "at": at.0 }).to_string(), READ_CACHE).await;
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

    fn served(&self, from_set: usize, checked: usize) {
        self.c.timing.put("chain_set", from_set as f64);
        self.c.timing.put("chain_chk", checked as f64);
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
    }

    struct Isolate<'w> {
        w: &'w World,
        held: RefCell<Option<(Timestamp, Decision, bool)>>,
    }

    impl GateFx for Isolate<'_> {
        type Err = String;
        fn now(&self) -> Timestamp {
            Timestamp(self.w.now.get())
        }
        fn held(&self) -> Option<(Timestamp, Decision, bool)> {
            *self.held.borrow()
        }
        fn hold(&self, at: Timestamp, d: Decision, foreign: bool) {
            *self.held.borrow_mut() = Some((at, d, foreign));
        }
        async fn matched(&self) -> Option<Timestamp> {
            *self.w.colo.borrow()
        }
        async fn share(&self, at: Timestamp) {
            *self.w.colo.borrow_mut() = Some(at);
        }
        /// The fold read starts now, takes `read_ms`, and is stamped at its start.
        async fn read(&self) -> Result<(Decision, Option<Timestamp>), String> {
            let start = self.w.now.get();
            self.w.now.set(start + self.w.read_ms);
            let granted = self.w.revoked.get().is_none_or(|t| start < t);
            Ok((if granted { Allow } else { Hidden }, Some(Timestamp(start))))
        }
    }

    fn world(read_ms: u64) -> World {
        World { now: Cell::new(0), colo: RefCell::new(None), revoked: Cell::new(None), read_ms }
    }

    fn iso(w: &World) -> Isolate<'_> {
        Isolate { w, held: RefCell::new(None) }
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
        let g = run(gate(&a, true));
        assert_eq!((g.decision, g.served, g.ahead), (Ok(Allow), Served::Read, false));
        assert_eq!(*w.colo.borrow(), Some(Timestamp(1_000)), "stamped at the fold read's start, not at its put");
        assert_eq!(a.held(), Some((Timestamp(1_000), Allow, false)));
        // Isolate b matches at 5,000 and copies the entry with its stamp.
        at(&w, 5_000);
        let g = run(gate(&b, true));
        assert_eq!((g.decision, g.served, g.ahead), (Ok(Allow), Served::Colo(4_000), false));
        assert_eq!(b.held(), Some((Timestamp(1_000), Allow, true)), "the copy keeps the entry's stamp");
        // At 9,999 b serves its copy and a re-read ahead is due; at 10,000 the copy and the entry have both lapsed.
        at(&w, 9_999);
        let g = run(gate(&b, true));
        assert_eq!((g.decision, g.served, g.ahead), (Ok(Allow), Served::Isolate(8_999), true));
        at(&w, 10_000);
        let g = run(gate(&b, true));
        assert_eq!(g.served, Served::Read, "past READ_CACHE less the skew nothing serves it");
        assert_eq!(*w.colo.borrow(), Some(Timestamp(10_000)));
        // A re-read ahead at 15,000 replaces the entry with its own stamp.
        at(&w, 15_000);
        run(reread(&a));
        assert_eq!(*w.colo.borrow(), Some(Timestamp(15_000)), "the re-read's Allow replaces the stamp");
        assert_eq!(a.held(), Some((Timestamp(15_000), Allow, false)));
        // The grant is removed at 16,000; a re-read at 17,000 refuses, is held in the isolate, and is not put.
        w.revoked.set(Some(16_000));
        at(&w, 17_000);
        run(reread(&a));
        assert_eq!(*w.colo.borrow(), Some(Timestamp(15_000)), "a refused re-read never touches the entry");
        assert_eq!(a.held(), Some((Timestamp(17_000), Hidden, false)));
        // A fresh read that refuses is never put, and the share off never matches or puts.
        let w = world(100);
        w.revoked.set(Some(0));
        let c = iso(&w);
        at(&w, 1_000);
        assert_eq!(run(gate(&c, true)).decision, Ok(Hidden));
        assert_eq!(*w.colo.borrow(), None, "a refusal is never put");
        let w = world(100);
        *w.colo.borrow_mut() = Some(Timestamp(900));
        let d = iso(&w);
        at(&w, 1_000);
        let g = run(gate(&d, false));
        assert_eq!(g.served, Served::Read, "the share off reads members as before");
        assert_eq!(*w.colo.borrow(), Some(Timestamp(900)), "and puts nothing");
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
            let g = run(gate(i, true));
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
