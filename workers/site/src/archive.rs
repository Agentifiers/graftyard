//! The leader's `LOG_MAX_BYTES` archive alarm (`design/DESIGN.md`, Shape, `LOG_MAX_BYTES` and "Packs";
//! `build/BUILD-PLAN.md`, AM1-WP1, "the leader's archive alarm that calls `archive_due`"). The decisions are
//! `graftyard_git::log_archive`'s, tested in memory on real members; this module is its host:
//!
//! - **When.** Each alarm of a leading repository member, after the snapshot writer's work, starts one run unless one
//!   is in flight on this member. A run sums the stored log above the last archive at most once a minute
//!   (`SUM_EVERY_MS`), so an idle or small group pays one SQLite aggregate a minute, and nothing when not leading.
//! - **Promote.** A due range no `Promoted` covers is promoted first: this site's git cache shard of the network
//!   catches up and answers `/promote` for the SHA-256s of the range's packs, then `/validate`s the result, which
//!   `PackStore::place` puts in the placement's buckets; `Promoted` is recorded and the pack and the range's stored
//!   packs are marked `Committed`.
//! - **Archive.** The covered part of the range, in pieces of `ARCHIVE_RANGE_MAX` bytes, goes create-only to two
//!   buckets of the record's sites, and `Archived` is recorded. Members truncate once the key is in two buckets
//!   (`Host::truncate`), and an installing member or the fold audit reads the entries back (`crate::load`).
//! - **Idempotent.** Request ids come from the range and the object's digest, so a run re-sent after a lost answer
//!   or a new leadership is answered from the `request` row and records nothing twice.
//!
//! The run is spawned, not awaited, so the alarm's Raft tick is never held behind the shard or R2. A run drains: it
//! takes steps one after another until nothing is due or `DRAIN_FOR` has passed, and each step commits on its own, so
//! a run cut off anywhere keeps every step before it. A run that fails notes why in the member's alarms and is
//! retried on a later alarm. One step that outlasts `STEP_FOR` is abandoned; a fixed deadline on the whole run
//! (`RUN_FOR`, 120 s) stopped B0-30's archive for good once each promotion's validate went to a cold shard
//! (`tests/am1/results/b0-30.md`, "Verdict").

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use graftyard_git::log_archive::{self, ARCHIVE_RANGE_MAX, ArchiveHost, PROMOTE_MAX, Step, View};
use graftyard_git::{self as git, Bucket as _, PackIndex, PackRef, PutNew};
use graftyard_raft::{Index, RequestId, Role};
use graftyard_types::{ChainId, Digest, NetworkId, SiteId, Timestamp, Ulid};
use worker::wasm_bindgen_futures::spawn_local;
use worker::*;

use crate::family::Repo;
use crate::host::{Host, blob, int, rows};
use crate::{SiteEnv, now_ms};

/// How often a leader sums its stored log's bytes to ask whether an archive is due [guess].
const SUM_EVERY_MS: u64 = 60_000;
/// After a run that did work, the next alarm comes this soon, so a due range drains piece by piece.
const BUSY_MS: i64 = 200;
/// A run starts no new step after this long [guess], so a member's isolate is not held by one drain for ever.
const DRAIN_FOR_MS: u64 = 90_000;
/// One step that has not finished in this long is abandoned [guess]: a shard's catch-up behind it saves its cursor
/// piece by piece, so the next step resumes the work. Over a push's 180 s, because a promotion waits on the same
/// catch-up a push would.
const STEP_FOR: std::time::Duration = std::time::Duration::from_secs(600);

thread_local! {
    /// Test installs: the cap and range size in place of `LOG_MAX_BYTES` and `ARCHIVE_RANGE_MAX`, for this isolate
    /// (`/bench_seed {"archiveCap": {"max": n, "range": n}}`), so a hosted smoke archives without 2 GiB of log.
    #[cfg(feature = "test-faults")]
    static CAP: std::cell::Cell<Option<(u64, u64)>> = const { std::cell::Cell::new(None) };
    /// Members of this isolate with a run in flight, by object name.
    static RUNNING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// When each member last summed its log, by object name.
    static SUMMED: RefCell<std::collections::HashMap<String, u64>> = RefCell::new(Default::default());
}

/// Starts one run on a leading repository member, unless one is in flight or the log was summed under a minute ago
/// and the last run found nothing to do.
pub fn on_alarm(h: &Rc<Host<Repo>>) {
    let key = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref().filter(|c| c.m.role() == Role::Leader) else { return };
        if !matches!(c.join.chain, ChainId::Repo(_)) {
            return;
        }
        c.join.me.do_name.to_string()
    };
    if RUNNING.with(|r| r.borrow().contains(&key)) {
        return;
    }
    let due = SUMMED.with(|s| s.borrow().get(&key).is_none_or(|at| now_ms() >= at + SUM_EVERY_MS));
    if !due {
        return;
    }
    RUNNING.with(|r| r.borrow_mut().insert(key.clone()));
    let w = Rc::downgrade(h);
    spawn_local(async move {
        let r = match w.upgrade() {
            Some(h) => drain(&h).await,
            None => Ok(Step::Idle),
        };
        RUNNING.with(|r| r.borrow_mut().remove(&key));
        let Some(h) = w.upgrade() else { return };
        match r {
            // Nothing due: the next sum waits a minute.
            Ok(Step::Idle) => {
                SUMMED.with(|s| s.borrow_mut().insert(key, now_ms()));
            }
            // The drain stopped at its budget with work left: the next run starts at once.
            Ok(_) => h.arm_in(BUSY_MS),
            Err(e) => {
                SUMMED.with(|s| s.borrow_mut().insert(key, now_ms()));
                note(&h, format!("archive: {e}"));
            }
        }
    });
}

/// Steps until nothing is due, a step fails, or `DRAIN_FOR_MS` has passed: the last step's result.
async fn drain(h: &Rc<Host<Repo>>) -> std::result::Result<Step, String> {
    let began = now_ms();
    loop {
        let s = crate::host::within(STEP_FOR, run(h)).await.unwrap_or_else(|| Err("a step timed out".into()))?;
        if s == Step::Idle {
            return Ok(s);
        }
        console_log!("archive {s:?}");
        h.stats.borrow_mut().archive_steps += 1;
        if now_ms() >= began + DRAIN_FOR_MS {
            return Ok(s);
        }
    }
}

fn note(h: &Rc<Host<Repo>>, m: String) {
    console_error!("{m}");
    let mut s = h.stats.borrow_mut();
    if s.alarms.len() < 50 {
        s.alarms.push(format!("{} {m}", now_ms()));
    }
}

async fn run(h: &Rc<Host<Repo>>) -> std::result::Result<Step, String> {
    h.settled().await;
    let site = SiteEnv::read(&h.env).map_err(|e| e.to_string())?;
    let sql = h.state.storage().sql();
    let s = SiteHost { h, site, sql };
    #[cfg(feature = "test-faults")]
    if let Some((max, range)) = CAP.with(|c| c.get()) {
        return log_archive::step(&s, max, range, PROMOTE_MAX).await;
    }
    log_archive::step(&s, graftyard_fold::LOG_MAX_BYTES, ARCHIVE_RANGE_MAX, PROMOTE_MAX).await
}

/// Test installs: sets or clears this isolate's cap override, and forgets when each member last summed its log so
/// the next alarm asks at once.
#[cfg(feature = "test-faults")]
pub fn set_cap(v: &serde_json::Value) -> serde_json::Value {
    let cap = v["max"].as_u64().map(|m| (m, v["range"].as_u64().unwrap_or(ARCHIVE_RANGE_MAX)));
    CAP.with(|c| c.set(cap));
    SUMMED.with(|s| s.borrow_mut().clear());
    serde_json::json!({ "archiveCap": cap })
}

struct SiteHost<'a> {
    h: &'a Rc<Host<Repo>>,
    site: SiteEnv,
    sql: SqlStorage,
}

impl SiteHost<'_> {
    fn chain(&self) -> std::result::Result<(ChainId, graftyard_types::PlacementRecord), String> {
        let core = self.h.core.borrow();
        let c = core.as_ref().ok_or("not joined")?;
        Ok((c.join.chain, c.join.record.clone()))
    }

    /// This site's git cache shard of the network, whose catch-up reads the group through the record.
    async fn shard(&self, path: &str, body: Vec<u8>) -> std::result::Result<Vec<u8>, String> {
        let (chain, record) = self.chain()?;
        let ChainId::Repo(repo) = chain else { return Err("not a repository chain".into()) };
        let me = self.site.id;
        let region = self.site.sites.get(me).ok_or("this site is not in GRAFTYARD_SITES")?.region;
        let stub = self
            .h
            .env
            .durable_object(&format!("SHARD_{}", me.0))
            .and_then(|ns| ns.get_by_name_with_location_hint(&format!("git#{}#net:{}", me.0, repo.0.0), region.hint()))
            .map_err(|e| e.to_string())?;
        let rec = serde_json::to_string(&record).map_err(|e| e.to_string())?;
        let url = format!("https://shard{path}repo={}&record={}", repo.0.0, js_sys::encode_uri_component(&rec));
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(js_sys::Uint8Array::from(body.as_slice()).into()));
        let req = Request::new_with_init(&url, &init).map_err(|e| e.to_string())?;
        let mut r = stub.fetch_with_request(req).await.map_err(|e| e.to_string())?;
        if r.status_code() != 200 {
            return Err(format!("shard {path}: {} {}", r.status_code(), r.text().await.unwrap_or_default()));
        }
        r.bytes().await.map_err(|e| e.to_string())
    }

    /// The record's sites' buckets.
    fn buckets(&self) -> std::result::Result<Vec<(SiteId, git::host::R2)>, String> {
        let (_, record) = self.chain()?;
        record
            .sites
            .iter()
            .map(|s| Ok((*s, git::host::R2(self.h.env.bucket(&format!("BUCKET_{}", s.0)).map_err(|e| e.to_string())?))))
            .collect()
    }
}

impl ArchiveHost for SiteHost<'_> {
    fn view(&self) -> Option<View> {
        let core = self.h.core.borrow();
        let c = core.as_ref().filter(|c| c.m.role() == Role::Leader)?;
        let fold = &c.m.group_fold().fold;
        let sum = |after: u64| -> Option<u64> {
            let q = "SELECT COALESCE(SUM(length(row)), 0) FROM log WHERE idx > ?";
            Some(rows(&self.sql, q, vec![crate::host::i(after)]).ok()?.first().and_then(|r| int(&r[0]).ok())? as u64)
        };
        let total = sum(fold.archived().map_or(0, |a| a.to.0))?;
        let unpromoted = sum(fold.promoted().map_or(0, |p| p.to.0))?;
        Some(View::of(c.join.chain, fold, c.m.state().commit, total, unpromoted))
    }

    fn rows(&self, after: Index) -> std::result::Result<log_archive::Rows<'_>, String> {
        // Read lazily, so the walk stops one row past the range without loading the log.
        let it = self
            .sql
            .exec("SELECT idx, row FROM log WHERE idx > ? ORDER BY idx", vec![crate::host::i(after.0)])
            .map_err(|e| e.to_string())?
            .raw();
        Ok(Box::new(it.map_while(|r| {
            let r = r.ok()?;
            Some((Index(int(&r[0]).ok()? as u64), blob(&r[1]).ok()?.to_vec()))
        })))
    }

    async fn promote(&self, sums: &[Digest], request: RequestId) -> std::result::Result<PackRef, String> {
        let (chain, _) = self.chain()?;
        let ChainId::Repo(repo) = chain else { return Err("not a repository chain".into()) };
        let body = serde_json::to_vec(&sums.iter().map(|s| s.hex()).collect::<Vec<_>>()).map_err(|e| e.to_string())?;
        // `wait=60`: a shard still catching up answers 503 "warming" after a minute and goes on in the background, so a
        // cold shard costs a step a minute rather than `STEP_FOR` (the git cache shard's bounded wait).
        let pack = self.shard("/promote?wait=60&", body).await?;
        // `via=0`: the shard that built the pack validates it. Through `fanout` it went to a pool shard, whose
        // catch-up on a hot group is a rehydrate of gigabytes, and archiving stopped (`tests/am1/results/b0-30.md`).
        let ix: PackIndex =
            serde_json::from_slice(&self.shard("/validate?tipsn=0&via=0&wait=60&", pack.clone()).await?)
                .map_err(|e| e.to_string())?;
        let raw = self.buckets()?;
        let refs: Vec<(SiteId, &git::host::R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
        let store = git::PackStore { sites: &self.site.sites, buckets: &refs, copies: git::Copies::TwoRegions };
        // The id is fresh per attempt: an attempt whose `Promoted` never committed leaves an unmarked pack, which
        // the sweeper deletes once its `request` row is still missing past `ORPHAN_AFTER`.
        let mut rand = [0u8; 10];
        rand.copy_from_slice(&request.0.0.to_be_bytes()[6..]);
        let place = git::Place {
            network: NetworkId(repo.0),
            id: git::PackId(Ulid::new(Timestamp(now_ms()), rand)),
            meta: git::ObjectMeta { chain, request },
            bytes: &pack,
            index: &ix,
            idx: None,
            inline_max: 0,
        };
        store.place(place).await.map(|d| d.into_pack()).map_err(|e| e.to_string())
    }

    async fn mark(&self, pack: &PackRef, index: Index) {
        let (Ok((chain, _)), Ok(raw)) = (self.chain(), self.buckets()) else { return };
        let ChainId::Repo(repo) = chain else { return };
        let refs: Vec<(SiteId, &git::host::R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
        let failed = git::mark_committed(&refs, NetworkId(repo.0), pack, chain, index).await;
        self.h.stats.borrow_mut().marker_failures += failed as u32;
    }

    /// The first two of the record's sites, as the bed's archive route put them; `Host::truncate` HEADs every
    /// site's bucket, so any two count.
    async fn put(&self, key: &str, bytes: &[u8], digest: Digest) -> std::result::Result<usize, String> {
        let mut held = 0;
        for (_, b) in self.buckets()?.iter().take(2) {
            match b.put_new(key, bytes, digest, &[]).await.map_err(|e| e.0)? {
                PutNew::Created => held += 1,
                PutNew::Exists(Some(d)) if d == digest => held += 1,
                PutNew::Exists(_) => {}
            }
        }
        Ok(held)
    }

    async fn record(
        &self,
        request: RequestId,
        events: Vec<git::repo::RepoEvent>,
    ) -> std::result::Result<Index, String> {
        let rx = self.h.wait_recorded(request);
        let r =
            self.h.input(|m, t| m.record(t, request, git::repo::REPO_FORMAT_V1, events)).map_err(|e| e.to_string())?;
        r.map_err(|e| crate::host::call_error_json(&e).to_string())?;
        self.h.served();
        match crate::host::within(std::time::Duration::from_secs(10), rx).await {
            Some(Ok(Ok(index))) => Ok(index),
            Some(Ok(Err(nl))) => Err(format!("not leader: {}", crate::host::nl_json(&nl))),
            Some(Err(_)) => Err("member reset".into()),
            None => Err("the record did not commit in 10 s".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use graftyard_git::log_archive;
    use graftyard_git::repo::RepoEvent;
    use graftyard_git::{PackId, PackRef, Promoted};
    use graftyard_raft::{EntryBody, LogEntry, TierSet, codec};
    use graftyard_types::{ChainId, DbId, Digest, Index, RepoId, RequestId, SiteSet, Timestamp, Ulid};
    use smallvec::SmallVec;

    use crate::family::{Family, Repo};

    fn entry(i: u64, events: Vec<RepoEvent>) -> LogEntry<RepoEvent> {
        LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(i),
            prev: Digest([0; 32]),
            format: graftyard_git::repo::REPO_FORMAT_V1,
            tiers: TierSet::CORE,
            at: Timestamp(1_000 * i),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(events),
        }
    }

    /// The `Archived` the alarm records is one `Host::truncate` acts on: `Family::archived` reads its key and range,
    /// and the key names the object the alarm put, so a member HEADs the same key in the buckets.
    #[test]
    fn the_alarm_s_archived_is_the_range_members_truncate_through() {
        let rows: Vec<_> =
            (3..=5).map(|i| (Index(i), Timestamp(1_000 * i), codec::to_bytes(&entry(i, vec![])))).collect();
        let chain = ChainId::Repo(RepoId(DbId(9)));
        let (bytes, a) = log_archive::archive_object(chain, &rows).unwrap();
        assert!(bytes.len() > 3 * rows[0].2.len());
        let key = a.key.to_string();
        assert!(key.starts_with("archive/repo:9/"), "{key}");
        let r = Repo::archived(&entry(6, vec![RepoEvent::Archived(a)])).expect("an archive range");
        assert_eq!((r.key, r.from, r.to, r.to_at), (key, 3, 5, 5_000));
    }

    /// The sweeper's `request` row for a `Promoted` names its stored pack, so a marker the alarm failed to put is
    /// repaired and the pack is never taken for an orphan.
    #[test]
    fn a_promoted_entry_s_request_row_names_its_pack() {
        let pack =
            PackRef::Stored { id: PackId(Ulid(7 << 80)), checksum: Digest([7; 32]), bytes: 70, sites: SiteSet(3) };
        let p = Promoted { from: Index(1), to: Index(5), pack: pack.clone() };
        let v = Repo::row_packs(&[RepoEvent::Promoted(p)]);
        let named: Vec<PackRef> = serde_json::from_value(v).unwrap();
        assert_eq!(named, vec![pack]);
        // A push's row still names its own pack alone.
        let push = RepoEvent::Refs { push: RequestId(Ulid(1)), actor: None, updates: vec![], pack: None };
        assert_eq!(Repo::row_packs(&[push]), serde_json::json!([]));
    }
}
