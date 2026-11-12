//! `catchup::catch_up` and its rehydrate on a hot group's log, in memory: a model container whose packs need the
//! objects of earlier packs (a pushed pack is thin), a log that pages as a member's `/entries` does and can answer a
//! short or empty page as a newly elected leader does, and cut-offs at every step. The failures they reproduce are
//! `tests/am1/results/b0-30.md`, "Blocked on", 8 to 12.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use futures_executor::block_on;
use graftyard_git::catchup::{Ingested, Rehydrating, Report, Saved, ShardIo, catch_up};
use graftyard_git::group::RefsPage;
use graftyard_git::{ChainLog, Head, IngestCursor, LogPacks, Marker, PackId, PackRef, Promoted, StoreError};
use graftyard_types::{ChainId, DbId, Digest, Index, NetworkId, RepoId, SiteSet, Ulid};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const NET: NetworkId = NetworkId(DbId(9));
const CHAIN: ChainId = ChainId::Repo(RepoId(DbId(9)));
/// A member's `ENTRIES_PAGE_BYTES` (`workers/site/src/host.rs`).
const PAGE_BYTES: usize = 600_000;

/// A model pack: the objects it brings and the objects it needs on the disk first.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pack {
    brings: Vec<u64>,
    needs: Vec<u64>,
    #[serde(default)]
    pad: usize,
}

fn bytes_of(p: &Pack) -> Vec<u8> {
    let mut b = serde_json::to_vec(p).unwrap();
    b.resize(b.len() + p.pad, b' ');
    b
}

fn sum(b: &[u8]) -> Digest {
    Digest(Sha256::digest(b).into())
}

fn oid(o: u64) -> String {
    format!("{o:040x}")
}

/// The container and the buckets.
#[derive(Default)]
struct Disk {
    id: RefCell<String>,
    objects: RefCell<BTreeSet<u64>>,
    ingested: RefCell<BTreeSet<Digest>>,
    refs: RefCell<BTreeMap<String, String>>,
    bucket: RefCell<BTreeMap<PackId, Vec<u8>>>,
    markers: RefCell<Vec<(PackId, Marker)>>,
    inits: Cell<usize>,
    ingests: Cell<usize>,
    fetched: Cell<usize>,
    /// The length of each `have` ask.
    asks: RefCell<Vec<usize>>,
    /// Every call fails once this many more ingests have run: a caller's deadline or an isolate reset.
    cut_after: Cell<Option<usize>>,
}

impl Disk {
    fn new() -> Disk {
        let d = Disk::default();
        *d.id.borrow_mut() = "disk-1".into();
        d
    }

    /// A new container: an empty disk under a new id.
    fn reset(&self, id: &str) {
        *self.id.borrow_mut() = id.into();
        self.objects.borrow_mut().clear();
        self.ingested.borrow_mut().clear();
        self.refs.borrow_mut().clear();
    }

    fn tick(&self) -> Result<(), StoreError> {
        if let Some(n) = self.cut_after.get() {
            if n == 0 {
                return Err(StoreError("cut off".into()));
            }
            self.cut_after.set(Some(n - 1));
        }
        Ok(())
    }

    fn take(&self, s: Digest, b: &[u8]) -> Ingested {
        if self.ingested.borrow().contains(&s) {
            return Ingested::Done;
        }
        let p: Pack = serde_json::from_slice(b.trim_ascii_end()).unwrap();
        if !p.needs.iter().all(|o| self.objects.borrow().contains(o)) {
            return Ingested::Later;
        }
        self.objects.borrow_mut().extend(p.brings);
        self.ingested.borrow_mut().insert(s);
        Ingested::Done
    }
}

impl ShardIo for Disk {
    async fn up(&self) -> Result<String, StoreError> {
        Ok(self.id.borrow().clone())
    }
    async fn init(&self, fresh: bool) -> Result<(), StoreError> {
        self.tick()?;
        assert!(fresh);
        self.inits.set(self.inits.get() + 1);
        self.objects.borrow_mut().clear();
        self.ingested.borrow_mut().clear();
        self.refs.borrow_mut().clear();
        Ok(())
    }
    async fn have(&self, sums: &[Digest]) -> Result<BTreeSet<Digest>, StoreError> {
        self.asks.borrow_mut().push(sums.len());
        Ok(sums.iter().filter(|s| self.ingested.borrow().contains(s)).copied().collect())
    }
    async fn ingest(&self, s: Digest, body: &[u8]) -> Result<Ingested, StoreError> {
        self.tick()?;
        self.ingests.set(self.ingests.get() + 1);
        assert_eq!(sum(body), s);
        Ok(self.take(s, body))
    }
    async fn ingest_stored(&self, id: PackId, checksum: Digest) -> Result<Ingested, StoreError> {
        self.tick()?;
        self.ingests.set(self.ingests.get() + 1);
        self.fetched.set(self.fetched.get() + 1);
        let b = self.bucket.borrow().get(&id).cloned().ok_or_else(|| StoreError("no such pack".into()))?;
        assert_eq!(sum(&b), checksum);
        Ok(self.take(checksum, &b))
    }
    async fn put_refs_page(&self, refs: &[(String, String)], start: bool, _done: bool) -> Result<(), StoreError> {
        self.tick()?;
        let mut r = self.refs.borrow_mut();
        if start {
            r.clear();
        }
        for (n, o) in refs {
            // The container skips a ref naming an object it does not hold.
            if self.objects.borrow().contains(&u64::from_str_radix(o, 16).unwrap()) {
                r.insert(n.clone(), o.clone());
            }
        }
        Ok(())
    }
    async fn apply_refs(&self, changes: &[(String, Option<String>)]) -> Result<(), StoreError> {
        self.tick()?;
        let mut r = self.refs.borrow_mut();
        for (n, o) in changes {
            match o {
                Some(o) if self.objects.borrow().contains(&u64::from_str_radix(o, 16).unwrap()) => {
                    r.insert(n.clone(), o.clone());
                }
                Some(_) => {}
                None => {
                    r.remove(n);
                }
            }
        }
        Ok(())
    }
    async fn make_room(&self, _incoming: u64) -> Result<Vec<i64>, StoreError> {
        Ok(Vec::new())
    }
    async fn markers(&self) -> Result<Vec<(PackId, Marker)>, StoreError> {
        Ok(self.markers.borrow().clone())
    }
}

/// The shard object's storage.
#[derive(Default)]
struct Store {
    cursor: RefCell<Option<(String, IngestCursor)>>,
    rehydrating: RefCell<Option<(String, Rehydrating)>>,
}

impl Saved for Store {
    async fn cursor(&self) -> Result<Option<(String, IngestCursor)>, StoreError> {
        Ok(self.cursor.borrow().clone())
    }
    async fn put_cursor(&self, disk: &str, c: &IngestCursor) -> Result<(), StoreError> {
        *self.cursor.borrow_mut() = Some((disk.into(), c.clone()));
        Ok(())
    }
    async fn drop_cursor(&self) -> Result<(), StoreError> {
        *self.cursor.borrow_mut() = None;
        Ok(())
    }
    async fn rehydrating(&self) -> Result<Option<(String, Rehydrating)>, StoreError> {
        Ok(self.rehydrating.borrow().clone())
    }
    async fn put_rehydrating(&self, disk: &str, r: &Rehydrating) -> Result<(), StoreError> {
        *self.rehydrating.borrow_mut() = Some((disk.into(), r.clone()));
        Ok(())
    }
    async fn drop_rehydrating(&self) -> Result<(), StoreError> {
        *self.rehydrating.borrow_mut() = None;
        Ok(())
    }
}

/// What one `/entries` call answers: as the member does, or as a newly elected leader whose commit index lags does.
#[derive(Clone, Copy, Debug)]
enum Answer {
    /// At most this many entries.
    Short(usize),
    Empty,
    Fail,
}

/// The chain's log on its leader: entries `first..`, each the packs and ref changes of one push.
struct Log {
    entries: RefCell<Vec<LogPacks>>,
    first: Cell<u64>,
    promoted: RefCell<Option<Promoted>>,
    faults: RefCell<VecDeque<Answer>>,
    calls: Cell<usize>,
    /// Entries answered in all.
    served: Cell<usize>,
    refs_page: usize,
}

impl Log {
    fn head(&self) -> u64 {
        self.entries.borrow().last().map_or(0, |e| e.index.0)
    }

    /// The refs a fold at the head holds.
    fn refs(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for e in self.entries.borrow().iter() {
            for (n, o) in e.refs.clone().unwrap_or_default() {
                match o {
                    Some(o) => out.insert(n, o),
                    None => out.remove(&n),
                };
            }
        }
        out
    }
}

impl ChainLog for Log {
    async fn head(&self, _c: ChainId) -> Result<Head, StoreError> {
        Ok(Head { first: Index(self.first.get()), applied: Index(self.head()) })
    }
    async fn refs_page(&self, _c: ChainId, after: Option<String>) -> Result<(RefsPage, Index), StoreError> {
        let all = self.refs();
        let rest: Vec<(String, String)> =
            all.into_iter().filter(|(n, _)| after.as_ref().is_none_or(|a| n > a)).take(self.refs_page + 1).collect();
        let more = rest.len() > self.refs_page;
        let refs: Vec<(String, String)> = rest.into_iter().take(self.refs_page).collect();
        let next = more.then(|| refs.last().unwrap().0.clone());
        Ok((RefsPage { refs, next }, Index(self.head())))
    }
    async fn entries(&self, _c: ChainId, from: Index, to: Index) -> Result<Vec<LogPacks>, StoreError> {
        self.calls.set(self.calls.get() + 1);
        if from.0 < self.first.get() {
            return Err(StoreError(format!("entry {} is archived", from.0)));
        }
        let fault = self.faults.borrow_mut().pop_front();
        let mut out = Vec::new();
        let mut bytes = 0;
        for e in self.entries.borrow().iter().filter(|e| e.index >= from && e.index <= to) {
            let b: usize = e
                .packs
                .iter()
                .map(|p| match p {
                    PackRef::Inline(b) => b.len(),
                    PackRef::Stored { .. } => 100,
                })
                .sum();
            if !out.is_empty() && bytes + b > PAGE_BYTES {
                break;
            }
            bytes += b;
            out.push(e.clone());
        }
        let out: Vec<LogPacks> = match fault {
            Some(Answer::Short(n)) => out.into_iter().take(n).collect(),
            Some(Answer::Empty) => Vec::new(),
            _ => out,
        };
        self.served.set(self.served.get() + out.len());
        match fault {
            None => Ok(out),
            Some(Answer::Short(_) | Answer::Empty) => Ok(out),
            Some(Answer::Fail) => Err(StoreError("the read failed".into())),
        }
    }
    async fn last_promoted(&self, _c: ChainId) -> Result<Option<(Index, Promoted)>, StoreError> {
        Ok(self.promoted.borrow().clone().map(|p| (p.to, p)))
    }
    async fn consumed(&self, _n: NetworkId) -> Result<BTreeSet<PackId>, StoreError> {
        Ok(BTreeSet::new())
    }
}

struct Bed {
    disk: Disk,
    store: Store,
    log: Log,
}

/// A hot group's log: entries 1..=n, each one push of a thin pack of `pad` bytes building on the previous entry's
/// objects, to one of 7 branches; every `stored_every`th push went to R2 as a stored pack.
fn bed(n: u64, pad: usize, stored_every: u64) -> Bed {
    let disk = Disk::new();
    let mut entries = Vec::new();
    for i in 1..=n {
        let p = Pack { brings: vec![i], needs: if i > 1 { vec![i - 1] } else { vec![] }, pad };
        let b = bytes_of(&p);
        let pack = if stored_every > 0 && i % stored_every == 0 {
            let id = PackId(Ulid(i as u128));
            disk.bucket.borrow_mut().insert(id, b.clone());
            disk.markers
                .borrow_mut()
                .push((id, Marker { chain: CHAIN, index: Index(i), checksum: sum(&b), bytes: b.len() as u64 }));
            PackRef::Stored { id, checksum: sum(&b), bytes: b.len() as u64, sites: SiteSet(3) }
        } else {
            PackRef::Inline(b)
        };
        let mut refs = vec![(format!("refs/heads/b{}", i % 7), Some(oid(i)))];
        if i % 11 == 0 {
            refs.push((format!("refs/heads/b{}", (i + 3) % 7), None));
        }
        entries.push(LogPacks {
            index: Index(i),
            packs: vec![pack],
            promoted: None,
            consumed: vec![],
            refs: Some(refs),
        });
    }
    let log = Log {
        entries: RefCell::new(entries),
        first: Cell::new(1),
        promoted: RefCell::new(None),
        faults: RefCell::new(VecDeque::new()),
        calls: Cell::new(0),
        served: Cell::new(0),
        refs_page: 3,
    };
    Bed { disk, store: Store::default(), log }
}

impl Bed {
    fn run(&self) -> Result<Report, StoreError> {
        block_on(catch_up(&self.disk, &self.store, &self.log, NET, &[CHAIN], &|| 0))
    }

    /// Promotes entries 1..=to into one stored, complete pack, as the archive alarm does, and records `Promoted` at a
    /// new entry.
    fn promote(&self, to: u64) {
        self.promote_range(1, to);
    }

    /// Promotes `from..=to`: a complete pack, whose objects name the last object of the promotion before it, as
    /// `index-pack --strict` checks.
    fn promote_range(&self, from: u64, to: u64) {
        let needs = if from > 1 { vec![from - 1] } else { vec![] };
        let p = Pack { brings: (from..=to).collect(), needs, pad: 0 };
        let b = bytes_of(&p);
        let id = PackId(Ulid(1_000_000 + to as u128));
        self.disk.bucket.borrow_mut().insert(id, b.clone());
        let at = self.log.head() + 1;
        self.disk
            .markers
            .borrow_mut()
            .push((id, Marker { chain: CHAIN, index: Index(at), checksum: sum(&b), bytes: b.len() as u64 }));
        let pack = PackRef::Stored { id, checksum: sum(&b), bytes: b.len() as u64, sites: SiteSet(3) };
        let pr = Promoted { from: Index(from), to: Index(to), pack };
        self.log.entries.borrow_mut().push(LogPacks {
            index: Index(at),
            packs: vec![],
            promoted: Some(pr.clone()),
            consumed: vec![],
            refs: Some(vec![]),
        });
        *self.log.promoted.borrow_mut() = Some(pr);
    }

    /// The disk serves the head: every object, and the fold's refs exactly.
    fn assert_serves_head(&self) {
        let head = self.log.head();
        let objs = self.disk.objects.borrow();
        let want: Vec<u64> = self
            .log
            .entries
            .borrow()
            .iter()
            .filter(|e| !e.packs.is_empty())
            .map(|e| e.index.0)
            .filter(|i| !objs.contains(i))
            .collect();
        assert!(want.is_empty(), "objects missing on the disk: {want:?}");
        assert_eq!(*self.disk.refs.borrow(), self.log.refs(), "refs at {head}");
        let c = self.store.cursor.borrow().clone().expect("a cursor").1;
        assert_eq!(c.at(CHAIN), Index(head));
    }
}

/// Blocker 12: a newly elected leader answers a page short, or empty, below the head the shard read before. The shard
/// must neither skip those entries nor stop for good: later calls apply them, and the disk serves the head.
#[test]
fn a_short_or_empty_page_never_moves_the_cursor_past_what_was_applied() {
    let b = bed(120, 10, 0);
    b.log.entries.borrow_mut().truncate(40);
    b.run().unwrap();
    b.log.entries.replace(bed(120, 10, 0).log.entries.into_inner());
    // The first page answers 3 of the entries asked, the next is empty.
    b.log.faults.borrow_mut().extend([Answer::Short(3), Answer::Empty]);
    let first = b.run();
    assert!(first.is_err(), "the empty page stops the call");
    let at = b.store.cursor.borrow().clone().unwrap().1.at(CHAIN);
    assert_eq!(at, Index(43), "the cursor is at the last entry applied, not the end of a page asked for");
    assert!(b.disk.objects.borrow().contains(&43) && !b.disk.objects.borrow().contains(&44));
    b.run().unwrap();
    b.assert_serves_head();
}

/// A shard far behind a hot group's head holds at most two pages of entries at once, however long the gap
/// (blocker 8: 4,480 entries, 1.4 GB, read whole on every attempt).
#[test]
fn a_shard_far_behind_holds_at_most_two_pages() {
    let b = bed(400, 290_000, 0);
    b.log.entries.borrow_mut().truncate(1);
    b.run().unwrap();
    b.log.entries.replace(bed(400, 290_000, 0).log.entries.into_inner());
    let r = b.run().unwrap();
    assert_eq!(r.entries, 399);
    assert!(r.held_max <= 2 * PAGE_BYTES, "held {} bytes of inline packs", r.held_max);
    assert!(r.held_max > PAGE_BYTES, "the next page is read while one applies");
    b.assert_serves_head();
}

/// A catch-up cut off part way (a caller's deadline, an isolate reset) keeps every page it applied: the next call
/// starts after the last one, and the pages are read about once in all.
#[test]
fn a_cut_off_catch_up_resumes_after_its_last_page() {
    let b = bed(300, 120_000, 0);
    b.log.entries.borrow_mut().truncate(1);
    b.run().unwrap();
    b.log.entries.replace(bed(300, 120_000, 0).log.entries.into_inner());
    let mut attempts = 0;
    loop {
        attempts += 1;
        b.disk.cut_after.set(Some(40));
        if b.run().is_ok() {
            break;
        }
        assert!(attempts < 20);
    }
    assert!(attempts > 1);
    // 299 entries: each ingested once, plus at most the one page per attempt that was in flight at the cut.
    assert!(b.disk.ingests.get() <= 299 + attempts * 5, "{} ingests in {attempts} attempts", b.disk.ingests.get());
    b.disk.cut_after.set(None);
    b.assert_serves_head();
}

/// Blockers 9 to 11: a shard with no cursor rehydrates a tail of inline packs above the last `Promoted`. Cut off at any
/// point, it resumes on the same disk: one new repository in all, no stored pack fetched twice, and the tail read
/// about once.
#[test]
fn a_cut_off_rehydrate_resumes_and_never_starts_again_from_nothing() {
    for cut in [7, 12, 30, 95, 180] {
        let b = bed(500, 40_000, 9);
        b.promote(200);
        let mut attempts = 0;
        loop {
            attempts += 1;
            b.disk.cut_after.set(Some(cut));
            if b.run().is_ok() {
                break;
            }
            assert!(attempts < 200, "cut {cut}: no progress");
        }
        b.disk.cut_after.set(None);
        assert_eq!(b.disk.inits.get(), 1, "cut {cut}: the repository was started again");
        // Every stored pack (the promoted one, 22 below the tail and 33 in it) fetched once, and every pack ingested
        // once, plus at most the one pack in flight at each cut.
        let stored = b.disk.bucket.borrow().len();
        assert_eq!(stored, 56);
        assert!(b.disk.fetched.get() <= stored + attempts, "cut {cut}: {} stored fetches", b.disk.fetched.get());
        assert!(b.disk.ingests.get() <= stored + 267 + attempts, "cut {cut}: {} ingests", b.disk.ingests.get());
        assert!(b.store.rehydrating.borrow().is_none());
        // The 301 tail entries above the promotion are read about once: each cut-off run re-reads at most the two pages
        // it held (15 of these 40 KB entries a page), and a run still in the marked phase reads the entry of each of the
        // 33 markers in the tail, one entry a read.
        let page = PAGE_BYTES / 40_000;
        let read = b.log.served.get();
        assert!(
            read <= 301 + (attempts + 1) * (2 * page + 33),
            "cut {cut}: {read} entries read in {attempts} attempts"
        );
        // The marked phase runs to its end once: a resumed run past it does not list the stored packs again. Each
        // attempt asks `have` about once for the page it resumes on; the tail's 301 entries are about 21 pages.
        let asks = b.disk.asks.borrow().len();
        assert!(asks <= attempts + 301 / page + 4, "cut {cut}: {asks} asks of `have` in {attempts} attempts");
        b.assert_serves_head();
    }
}

/// A rehydrate reads only the tail above the last `Promoted`: the promoted pack brings everything below it.
#[test]
fn a_rehydrate_reads_the_log_only_above_the_last_promoted() {
    let b = bed(500, 1_000, 0);
    b.promote(450);
    let r = b.run().unwrap();
    let h = r.rehydrated.expect("a rehydrate");
    assert_eq!(h.entries, 51, "entries 451..=501");
    assert_eq!(h.inline, 50);
    b.assert_serves_head();
}

/// A disk that skipped an entry below its cursor (a build without the short-page guard did, on site c of B0-30) fails
/// on the first pack thin against it. The cursor is dropped, so the next call rehydrates the disk instead of failing on
/// the same pack on every call.
#[test]
fn a_disk_missing_an_entry_below_its_cursor_rehydrates_instead_of_failing_for_ever() {
    let b = bed(60, 10, 0);
    b.log.entries.borrow_mut().truncate(30);
    b.run().unwrap();
    // Plant the old skip: the disk lost entry 30's objects, and its cursor says 30.
    b.disk.objects.borrow_mut().remove(&30);
    b.log.entries.replace(bed(60, 10, 0).log.entries.into_inner());
    let e = b.run().unwrap_err();
    assert!(e.0.contains("rehydrates"), "{}", e.0);
    let r = b.run().unwrap();
    assert!(r.rehydrated.is_some());
    b.assert_serves_head();
}

/// A cursor for another disk (the container restarted), or below the log's first index (archived and truncated),
/// means a rehydrate; a read error leaves the cursor where it was.
#[test]
fn a_new_disk_or_an_archived_cursor_rehydrates_and_a_failed_read_keeps_the_cursor() {
    let b = bed(50, 10, 0);
    b.run().unwrap();
    b.disk.reset("disk-2");
    assert!(b.run().unwrap().rehydrated.is_some());
    b.assert_serves_head();

    let b = bed(80, 10, 0);
    b.log.entries.borrow_mut().truncate(20);
    b.run().unwrap();
    b.log.entries.replace(bed(80, 10, 0).log.entries.into_inner());
    b.log.faults.borrow_mut().push_back(Answer::Fail);
    assert!(b.run().is_err());
    assert_eq!(b.store.cursor.borrow().clone().unwrap().1.at(CHAIN), Index(20));
    b.promote(60);
    b.log.first.set(61);
    assert!(b.run().unwrap().rehydrated.is_some());
    b.assert_serves_head();
}

/// Promotion runs ahead of archiving, so a hot group's earlier promotions are recorded in the log above the last one's
/// range, where the tail starts. The last promoted pack names objects only they bring: a rehydrate that loaded it alone
/// in its marked phase failed every run on graft-m3c with "1 marked packs have bases no pack provides"
/// (`tests/am1/results/hot-shards.md`). A pushed stored pack in the tail, thin against the inline pack before it, is
/// still left to the tail.
#[test]
fn a_rehydrate_loads_every_promotion_still_in_the_log_before_the_last_one() {
    let b = bed(300, 1_000, 9);
    b.promote_range(1, 100);
    b.promote_range(101, 180);
    b.promote_range(181, 240);
    let r = b.run().unwrap();
    let h = r.rehydrated.expect("a rehydrate");
    assert_eq!(h.entries, 63, "the tail is 241..=303");
    assert_eq!(b.disk.inits.get(), 1);
    // Each stored pack fetched once: 33 pushed, 3 promoted.
    assert_eq!(b.disk.fetched.get(), 36);
    b.assert_serves_head();
}
