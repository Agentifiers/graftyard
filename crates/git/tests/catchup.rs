//! `catchup::catch_up` and its rehydrate on a hot group's log, in memory: a model container whose packs need the
//! objects of earlier packs (a pushed pack is thin), a log that pages as a member's `/entries` does and can answer a
//! short or empty page as a newly elected leader does, and cut-offs at every step. The failures they reproduce are
//! `tests/am1/results/b0-30.md`, "Blocked on", 8 to 12.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use futures_executor::block_on;
use graftyard_git::catchup::{
    Fetched, INGEST_BATCH_BYTES, IdxRange, Ingested, PIECE, Piece, Rehydrating, Report, Run, Saved, ShardIo, WHOLE,
    catch_up,
};
use graftyard_git::group::RefsPage;
use graftyard_git::{ChainLog, Compacted, Head, IngestCursor, LogPacks, Marker, PackId, PackRef, Promoted, StoreError};
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
    /// HEAD as the container keeps it, and the `/refs` calls that named it (G26).
    head: RefCell<Option<String>>,
    head_calls: Cell<usize>,
    /// Each fork's namespace HEAD, `refs/namespaces/<id>/HEAD`, as a namespaced `head=` sets it.
    ns_heads: RefCell<BTreeMap<String, String>>,
    bucket: RefCell<BTreeMap<PackId, Vec<u8>>>,
    markers: RefCell<Vec<(PackId, Marker)>>,
    inits: Cell<usize>,
    ingests: Cell<usize>,
    /// Calls that ingested inline packs.
    calls: Cell<usize>,
    fetched: Cell<usize>,
    /// The ids stored packs were fetched in, in ingest-list order.
    fetch_order: RefCell<Vec<PackId>>,
    /// The length of each `have` ask.
    asks: RefCell<Vec<usize>>,
    /// Every call fails once this many more ingests have run: a caller's deadline or an isolate reset.
    cut_after: Cell<Option<usize>>,
    /// Stored `.idx` objects: the index (here the SHA-256 of the pack it indexes, or other bytes for a bad one) and
    /// the range its custom metadata names.
    idx: RefCell<BTreeMap<PackId, (Digest, Option<IdxRange>)>>,
    /// Packs spooled onto the disk, each with its staged index if one came.
    spooled: RefCell<BTreeMap<Digest, (PackId, Option<Digest>)>>,
    /// Packs ingested through their stored index, and packs that went through `index-pack` (a batch's packs each count).
    idx_used: Cell<usize>,
    index_packs: Cell<usize>,
    /// Checksums in the order the container ingested them.
    order: RefCell<Vec<Digest>>,
    /// The virtual clock, and how long a pack's fetch (`slow`'s time for that pack, else `fetch_ms`) and its
    /// `index-pack` take on it.
    clock: Clock,
    fetch_ms: Cell<u64>,
    slow: RefCell<BTreeMap<PackId, u64>>,
    index_ms: Cell<u64>,
    /// How long a `/refs` call and a `make_room` take on the clock.
    refs_ms: Cell<u64>,
    room_ms: Cell<u64>,
    /// `make_room` calls.
    rooms: Cell<usize>,
    /// `ingest_spooled_batch` calls, and the packs each carried.
    batches: RefCell<Vec<usize>>,
    /// Fetches in flight now, and the most at once.
    in_flight: Cell<usize>,
    in_flight_max: Cell<usize>,
    connected_calls: Cell<usize>,
    /// The container's repack starts and backstop checks in order: "repack" when a call started the repack (the last
    /// ref page, or a check after its walk), "connected" when a check walked.
    events: RefCell<Vec<&'static str>>,
}

/// A virtual clock: `sleep` ends once `run` has moved the clock to its time, which it does whenever every task waits.
#[derive(Default)]
struct Clock {
    now: Cell<u64>,
    timers: RefCell<Vec<(u64, std::task::Waker)>>,
}

impl Clock {
    async fn sleep(&self, ms: u64) {
        let until = self.now.get() + ms;
        std::future::poll_fn(|cx| {
            if self.now.get() >= until {
                return std::task::Poll::Ready(());
            }
            self.timers.borrow_mut().push((until, cx.waker().clone()));
            std::task::Poll::Pending
        })
        .await
    }

    /// Runs `f` to its end on this clock.
    fn run<T>(&self, f: impl std::future::Future<Output = T>) -> T {
        let mut f = std::pin::pin!(f);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            let mut t = self.timers.borrow_mut();
            let next = t.iter().map(|(u, _)| *u).min().expect("every task waits and no timer is set");
            self.now.set(self.now.get().max(next));
            let due: Vec<_> = t.extract_if(.., |(u, _)| *u <= next).collect();
            drop(t);
            for (_, w) in due {
                w.wake();
            }
        }
    }
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
        self.spooled.borrow_mut().clear();
        *self.head.borrow_mut() = None;
        self.ns_heads.borrow_mut().clear();
    }

    /// As the container's `setHead`: a namespaced head points that namespace's HEAD, any other HEAD itself.
    fn set_head(&self, h: &str) {
        self.head_calls.set(self.head_calls.get() + 1);
        match h.strip_prefix("refs/namespaces/").and_then(|r| r.split_once('/')) {
            Some((ns, _)) => {
                self.ns_heads.borrow_mut().insert(format!("refs/namespaces/{ns}/HEAD"), h.to_owned());
            }
            None => *self.head.borrow_mut() = Some(h.to_owned()),
        }
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
        self.order.borrow_mut().push(s);
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
    async fn ingest(&self, packs: &[(Digest, &[u8])]) -> Result<Vec<Ingested>, StoreError> {
        self.tick()?;
        self.calls.set(self.calls.get() + 1);
        let bytes: usize = packs.iter().map(|(_, b)| b.len()).sum();
        assert!(packs.len() == 1 || bytes <= INGEST_BATCH_BYTES, "{} packs of {bytes} bytes in one call", packs.len());
        // As the container does: each pack in turn, so a pack may build on an earlier one of the same call.
        Ok(packs
            .iter()
            .map(|(s, b)| {
                self.ingests.set(self.ingests.get() + 1);
                assert_eq!(sum(b), *s);
                self.take(*s, b)
            })
            .collect())
    }
    async fn fetch_stored(&self, id: PackId, checksum: Digest, idx: bool) -> Result<Fetched, StoreError> {
        self.fetch_order.borrow_mut().push(id);
        self.fetched.set(self.fetched.get() + 1);
        if self.ingested.borrow().contains(&checksum) {
            return Ok(Fetched::Have);
        }
        let b = self.bucket.borrow().get(&id).cloned().ok_or_else(|| StoreError("no such pack".into()))?;
        assert_eq!(sum(&b), checksum);
        self.in_flight.set(self.in_flight.get() + 1);
        self.in_flight_max.set(self.in_flight_max.get().max(self.in_flight.get()));
        let ms = self.slow.borrow().get(&id).copied().unwrap_or(self.fetch_ms.get());
        self.clock.sleep(ms).await;
        self.in_flight.set(self.in_flight.get() - 1);
        let staged = if idx { self.idx.borrow().get(&id).map(|(i, _)| *i) } else { None };
        self.spooled.borrow_mut().insert(checksum, (id, staged));
        Ok(Fetched::Spooled)
    }
    async fn ingest_spooled(&self, checksum: Digest) -> Result<Ingested, StoreError> {
        self.tick()?;
        self.ingests.set(self.ingests.get() + 1);
        let (id, staged) = *self.spooled.borrow().get(&checksum).ok_or_else(|| StoreError("not spooled".into()))?;
        let b = self.bucket.borrow().get(&id).cloned().unwrap();
        // As the container checks a staged index against the pack's trailer: a bad one falls back to index-pack.
        let used = staged == Some(checksum);
        if !used {
            self.index_packs.set(self.index_packs.get() + 1);
            self.clock.sleep(self.index_ms.get()).await;
        }
        let a = self.take(checksum, &b);
        if a == Ingested::Done {
            self.spooled.borrow_mut().remove(&checksum);
            if used {
                self.idx_used.set(self.idx_used.get() + 1);
            }
        }
        Ok(a)
    }
    /// As the container's `/ingest-spooled-batch`: one `index-pack` for the call, each pack in turn so a later one may
    /// build on an earlier one of the same call.
    async fn ingest_spooled_batch(&self, checksums: &[Digest]) -> Result<Vec<Ingested>, StoreError> {
        self.tick()?;
        self.batches.borrow_mut().push(checksums.len());
        self.index_packs.set(self.index_packs.get() + checksums.len());
        self.clock.sleep(self.index_ms.get()).await;
        let mut out = Vec::with_capacity(checksums.len());
        for s in checksums {
            self.ingests.set(self.ingests.get() + 1);
            let (id, _) = *self.spooled.borrow().get(s).ok_or_else(|| StoreError("not spooled".into()))?;
            let b = self.bucket.borrow().get(&id).cloned().unwrap();
            let a = self.take(*s, &b);
            if a == Ingested::Done {
                self.spooled.borrow_mut().remove(s);
            }
            out.push(a);
        }
        Ok(out)
    }
    async fn idx_ranges(&self) -> Result<BTreeMap<PackId, Option<IdxRange>>, StoreError> {
        Ok(self.idx.borrow().iter().map(|(id, (_, r))| (*id, *r)).collect())
    }
    /// History is a chain, object i's parent i - 1: connected when every object's parent is here, and every ref's.
    async fn connected(&self, repack: bool) -> Result<Result<(), String>, StoreError> {
        self.connected_calls.set(self.connected_calls.get() + 1);
        self.events.borrow_mut().push("connected");
        if repack {
            self.events.borrow_mut().push("repack");
        }
        let objs = self.objects.borrow();
        if let Some(o) = objs.iter().find(|o| **o > 1 && !objs.contains(&(**o - 1))) {
            return Ok(Err(format!("missing object {}", o - 1)));
        }
        for o in self.refs.borrow().values() {
            if !objs.contains(&u64::from_str_radix(o, 16).unwrap()) {
                return Ok(Err(format!("ref to missing {o}")));
            }
        }
        Ok(Ok(()))
    }
    async fn put_refs_page(
        &self,
        refs: &[(String, String)],
        start: bool,
        done: bool,
        repack: bool,
        head: Option<&str>,
    ) -> Result<(), StoreError> {
        self.tick()?;
        if done && repack {
            self.events.borrow_mut().push("repack");
        }
        if done && let Some(h) = head {
            self.set_head(h);
        }
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
    /// As the container's `POST /refs`: a call with changes starts the repack unless `repack` is false (`repack=0`);
    /// one that only names HEAD starts none.
    async fn apply_refs(
        &self,
        changes: &[(String, Option<String>)],
        head: Option<&str>,
        repack: bool,
    ) -> Result<(), StoreError> {
        self.tick()?;
        self.clock.sleep(self.refs_ms.get()).await;
        if repack && !changes.is_empty() {
            self.events.borrow_mut().push("repack");
        }
        if let Some(h) = head {
            self.set_head(h);
        }
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
        self.rooms.set(self.rooms.get() + 1);
        self.clock.sleep(self.room_ms.get()).await;
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
    /// The index the first refs page is read at, when it is older than the head: the chain moved while the pages were
    /// read, so the rehydrate catches up from it.
    first_refs_at: Cell<Option<u64>>,
    /// The chain's committed rolls, oldest first, as `ChainLog::compactions` reads them from the fold.
    rolls: RefCell<Vec<Compacted>>,
    /// The `settings` row's default branch, which the `ranges` read answers beside the head (G26).
    default_branch: RefCell<Option<String>>,
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
        Ok(Head {
            first: Index(self.first.get()),
            applied: Index(self.head()),
            default_branch: self.default_branch.borrow().clone(),
        })
    }
    async fn refs_page(&self, _c: ChainId, after: Option<String>) -> Result<(RefsPage, Index), StoreError> {
        let all = self.refs();
        let rest: Vec<(String, String)> =
            all.into_iter().filter(|(n, _)| after.as_ref().is_none_or(|a| n > a)).take(self.refs_page + 1).collect();
        let more = rest.len() > self.refs_page;
        let refs: Vec<(String, String)> = rest.into_iter().take(self.refs_page).collect();
        let next = more.then(|| refs.last().unwrap().0.clone());
        let at = if after.is_none() { self.first_refs_at.get().unwrap_or(self.head()) } else { self.head() };
        Ok((RefsPage { refs, next }, Index(at)))
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
    async fn compactions(&self, _c: ChainId) -> Result<Vec<Compacted>, StoreError> {
        Ok(self.rolls.borrow().clone())
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
            compacted: None,
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
        first_refs_at: Cell::new(None),
        rolls: RefCell::new(Vec::new()),
        default_branch: RefCell::new(None),
    };
    Bed { disk, store: Store::default(), log }
}

impl Bed {
    fn run(&self) -> Result<Report, StoreError> {
        block_on(catch_up(&self.disk, &self.store, &self.log, NET, &[CHAIN], WHOLE, &mut Run::default(), &|| 0))
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
            compacted: None,
            consumed: vec![],
            refs: Some(vec![]),
        });
        *self.log.promoted.borrow_mut() = Some(pr);
    }

    /// Entry `i`'s push lands as a stored pack with a marker, as a push over `INLINE_MAX` bytes does.
    fn stored_at(&self, i: u64) {
        let mut entries = self.log.entries.borrow_mut();
        let e = entries.iter_mut().find(|e| e.index.0 == i).expect("an entry at i");
        let PackRef::Inline(b) = e.packs[0].clone() else { panic!("entry {i} already stored") };
        let id = PackId(Ulid(i as u128));
        self.disk.bucket.borrow_mut().insert(id, b.clone());
        self.disk
            .markers
            .borrow_mut()
            .push((id, Marker { chain: CHAIN, index: Index(i), checksum: sum(&b), bytes: b.len() as u64 }));
        e.packs[0] = PackRef::Stored { id, checksum: sum(&b), bytes: b.len() as u64, sites: SiteSet(3) };
    }

    /// One roll (`decisions/DECISIONS.md`, `roll-up`): the consumed promotions rolled into one complete, stored pack,
    /// recorded as `Compacted` at a new entry and listed by `ChainLog::compactions`. Its `Committed` marker is this
    /// caller's to write, as `mark_committed` writes a promotion's.
    fn roll(&self, from: u64, to: u64, consumed: &[PackId]) -> PackId {
        // Padded, so a roll of one promotion is not that promotion's bytes and checksum.
        let needs = if from > 1 { vec![from - 1] } else { vec![] };
        let p = Pack { brings: (from..=to).collect(), needs, pad: 1 };
        let b = bytes_of(&p);
        let id = PackId(Ulid(2_000_000 + to as u128));
        self.disk.bucket.borrow_mut().insert(id, b.clone());
        let produced = PackRef::Stored { id, checksum: sum(&b), bytes: b.len() as u64, sites: SiteSet(3) };
        let c = Compacted { consumed: consumed.to_vec(), produced, from: Index(from), to: Index(to) };
        self.log.rolls.borrow_mut().push(c.clone());
        let at = self.log.head() + 1;
        self.log.entries.borrow_mut().push(LogPacks {
            index: Index(at),
            packs: vec![],
            promoted: None,
            compacted: Some(c),
            consumed: vec![],
            refs: Some(vec![]),
        });
        id
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
        // Every stored pack (the promoted one, 22 below the tail and 33 in it) fetched once, plus at each cut the packs
        // fetched ahead and not yet ingested (`AHEAD_PACKS`), and every pack ingested once, plus the one in flight.
        let stored = b.disk.bucket.borrow().len();
        assert_eq!(stored, 56);
        let refetch = attempts * graftyard_git::catchup::AHEAD_PACKS;
        assert!(b.disk.fetched.get() <= stored + refetch, "cut {cut}: {} stored fetches", b.disk.fetched.get());
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

/// A rehydrate's last step catches up from its first refs page to the head. Until that reaches the head the disk
/// cannot serve, so the shard must still say it is rehydrating, and a push with another shard next is sent on at once:
/// j9h's home shard dropped its place with the cursor, 33 s before its last catch-up of 338 entries ended, and 16 pushes
/// each waited out the home's 15 s (`tests/am1/results/hot-shards-3.md`). A run cut off in that catch-up keeps the
/// place, and the catch-up that finishes it drops it.
#[test]
fn a_rehydrate_is_marked_rehydrating_until_its_last_catch_up_reaches_the_head() {
    let mut cut_in_last = 0;
    for cut in 1..400 {
        let b = bed(300, 1_000, 9);
        b.promote(100);
        b.log.first_refs_at.set(Some(250));
        b.disk.cut_after.set(Some(cut));
        let first = b.run();
        if first.is_ok() {
            assert!(b.store.rehydrating.borrow().is_none(), "cut {cut}: a finished rehydrate keeps its place");
            break;
        }
        if b.store.cursor.borrow().is_none() {
            continue;
        }
        // Cut off after the refs, in the catch-up from entry 251.
        cut_in_last += 1;
        assert!(b.store.rehydrating.borrow().is_some(), "cut {cut}: the place went before the disk reached the head");
        b.disk.cut_after.set(None);
        let r = b.run().unwrap_or_else(|e| panic!("cut {cut}: {}", e.0));
        assert!(r.rehydrated.is_none(), "cut {cut}: the cursor is kept, so this is a catch-up");
        assert!(b.store.rehydrating.borrow().is_none(), "cut {cut}: the finishing catch-up keeps the place");
        assert_eq!(b.disk.inits.get(), 1);
        b.assert_serves_head();
    }
    assert!(cut_in_last > 0, "no cut fell in the last catch-up");
}

/// Promotion keeps running while a shard rehydrates (on another site's shard), so a cut-off rehydrate resumes after
/// the last `Promoted` has moved past where its tail got. It must not start its tail at the new promotion's end without
/// that promotion's pack: the entries in between are never read, and the next tail pack fails "have bases no pack
/// provides" on every call, as site a's shard did after the kill on graft-m3c (`tests/am1/results/hot-shards-2.md`).
#[test]
fn a_rehydrate_resumed_after_a_new_promotion_loads_it_and_skips_nothing() {
    // Each cut falls in the tail after at least one of its pages was saved: entries of 10 KB make the tail several
    // pages of `PAGE_BYTES`, where 1 KB ones made it one page, and a place saved before it said nothing about the tail.
    for cut in [20, 40, 60] {
        let b = bed(400, 10_000, 9);
        b.promote(100);
        b.disk.cut_after.set(Some(cut));
        assert!(b.run().is_err(), "cut {cut}: the first run is cut off in its tail");
        let (_, s) = b.store.rehydrating.borrow().clone().expect("a saved place");
        assert!(s.marked, "cut {cut}: cut after the marked phase");
        let got = s.tail.at(CHAIN).0;
        assert!(got > 100, "cut {cut}: cut after a tail page was saved");
        // The archive promotes past where the tail got while the shard is cut off.
        b.promote_range(101, got + 150);
        b.disk.cut_after.set(None);
        let r = b.run().unwrap_or_else(|e| panic!("cut {cut}: {}", e.0));
        assert!(r.rehydrated.expect("a rehydrate").resumed, "cut {cut}");
        assert_eq!(b.disk.inits.get(), 1, "cut {cut}: the repository was started again");
        b.assert_serves_head();
    }
}

/// A place saved by a release that skipped that way records nothing loaded and a tail cursor past entries the disk
/// never ingested (site a's shard on graft-m3c resumed at 16,600 with 6,171 onwards missing). A resumed run loads the
/// marked phase again, which brings every promotion still in the log, and the disk serves the head.
#[test]
fn a_rehydrate_resumed_from_an_older_place_loads_the_marked_phase_again() {
    let b = bed(400, 10_000, 9);
    b.promote(100);
    // Cut in the tail just after its first page was saved, so the disk holds none of the entries past it that the older
    // release's place claims.
    b.disk.cut_after.set(Some(17));
    assert!(b.run().is_err());
    let (disk, mut s) = b.store.rehydrating.borrow().clone().expect("a saved place");
    assert!(s.marked, "cut in the tail");
    let got = s.tail.at(CHAIN).0;
    assert!(got > 100, "cut after a tail page was saved");
    b.promote_range(101, got + 150);
    // The older release resumed at the new tail start and saved its tail past the entries it skipped.
    s.tail.advance(CHAIN, Index(got + 150));
    s.loaded.clear();
    *b.store.rehydrating.borrow_mut() = Some((disk, s));
    b.disk.cut_after.set(None);
    b.run().unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(b.disk.inits.get(), 1, "the repository was started again");
    b.assert_serves_head();
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
    // Nothing skipped, so no check: the last ref page starts the repack, as before the backstop existed.
    assert_eq!(h.skipped, 0);
    assert_eq!(*b.disk.events.borrow(), ["repack"]);
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

/// A rehydrate's tail sends each run of inline packs in calls of up to `INGEST_BATCH_BYTES`, not one call a pack, and a
/// pack in the middle of a call whose base comes later in the log (here entry 150, which names an object of 151) is
/// answered `Later` alone and sent again after the rest: the packs after it in the same call still go in.
#[test]
fn a_rehydrate_ingests_runs_of_inline_packs_a_call_at_a_time() {
    let b = bed(400, 30_000, 0);
    {
        let mut es = b.log.entries.borrow_mut();
        for (i, needs) in [(150usize, vec![149, 151]), (151, vec![149])] {
            let p = Pack { brings: vec![i as u64], needs, pad: 30_000 };
            es[i - 1].packs = vec![PackRef::Inline(bytes_of(&p))];
        }
    }
    b.promote(100);
    let r = b.run().unwrap_or_else(|e| panic!("{}", e.0));
    let h = r.rehydrated.expect("a rehydrate");
    assert_eq!(h.inline, 300);
    // 300 packs of about 30 KB are 16 pages of 600 KB: one call a page (a page is under INGEST_BATCH_BYTES), and one
    // more for entry 150 sent again. One call a pack would be 301.
    let calls = b.disk.calls.get();
    let one = bytes_of(&Pack { brings: vec![200], needs: vec![199], pad: 30_000 }).len();
    let pages = 300usize.div_ceil(PAGE_BYTES / one);
    assert!(calls <= pages + 1, "{calls} calls for 300 packs in {pages} pages");
    assert_eq!(b.disk.ingests.get(), 1 + 300 + 1, "the promoted pack, each tail pack once, and entry 150 again");
    b.assert_serves_head();
}

/// A shard reads a stored pack from its own site's bucket first, and from the others in site order when its own
/// copy is missing or does not match the pack's checksum: B0-31 m3fr2's apac shard read every 1 MiB pack from enam's
/// bucket, site 0's, because the list was in site order (`tests/am1/results/lm1-rows-m3f.md`, m3fr2, B0-31).
#[test]
fn a_shard_reads_its_own_sites_bucket_first_and_falls_back() {
    use graftyard_git::catchup::{first_copy, own_first};
    use graftyard_git::mem::MemBucket;
    use graftyard_git::{Bucket, PutNew};
    use graftyard_types::SiteId;

    let key = "pack/9/p.pack";
    let good = b"the pack".to_vec();
    let want = sum(&good);
    let buckets: Vec<(SiteId, MemBucket)> = (0..3).map(|s| (SiteId(s), MemBucket::default())).collect();
    let put = |s: usize, b: &[u8]| {
        let r = block_on(buckets[s].1.put_new(key, b, sum(b), &[])).unwrap();
        assert_eq!(r, PutNew::Created);
    };
    // Every site holds a copy, as a placed pack's three buckets do once the filler ran.
    for s in 0..3 {
        put(s, &good);
    }
    // Who answered, in the order asked, and the copy that matched.
    let read = |me: u8| {
        let asked = RefCell::new(Vec::new());
        let order = own_first(SiteId(me), buckets.iter().map(|(s, b)| (*s, b)));
        let got = block_on(first_copy(&order, |b: &&MemBucket| {
            let b: &MemBucket = b;
            let s = buckets.iter().find(|(_, x)| std::ptr::eq(x, b)).unwrap().0;
            asked.borrow_mut().push(s.0);
            async move { Ok(b.get(key).await?.filter(|c| sum(c) == want)) }
        }))
        .unwrap();
        (got.map(|(s, _)| s.0), asked.into_inner())
    };
    for me in 0..3 {
        assert_eq!(read(me), (Some(me), vec![me]), "site {me} reads its own copy and no other");
    }
    // Site 2's own copy is gone: it reads site 0's, then stops.
    block_on(buckets[2].1.delete(&[key.into()])).unwrap();
    assert_eq!(read(2), (Some(0), vec![2, 0]));
    // Site 0's copy is corrupt too: it goes on to site 1's.
    block_on(buckets[0].1.delete(&[key.into()])).unwrap();
    put(0, b"not the pack");
    assert_eq!(read(2), (Some(1), vec![2, 0, 1]));
    // No good copy anywhere: every bucket asked, none answered.
    block_on(buckets[1].1.delete(&[key.into()])).unwrap();
    assert_eq!(read(2), (None, vec![2, 0, 1]));
    // Site 1 reads its own first, then the others in site order.
    assert_eq!(read(1).1, vec![1, 0, 2]);
}

impl Bed {
    fn run_timed(&self) -> Result<Report, StoreError> {
        let now = || self.disk.clock.now.get();
        self.disk.clock.run(catch_up(
            &self.disk,
            &self.store,
            &self.log,
            NET,
            &[CHAIN],
            WHOLE,
            &mut Run::default(),
            &now,
        ))
    }

    /// Stores the `.idx` of promotion `1_000_000 + to`, good or not, with the range its metadata names.
    fn idx(&self, to: u64, good: bool, range: Option<(u64, u64)>) {
        let id = PackId(Ulid(1_000_000 + to as u128));
        let b = self.disk.bucket.borrow().get(&id).cloned().expect("a promoted pack");
        let i = if good { sum(&b) } else { sum(b"another pack") };
        self.disk.idx.borrow_mut().insert(id, (i, range.map(|(f, t)| (CHAIN, Index(f), Index(t)))));
    }

    fn pushed_fetched(&self) -> usize {
        // Every pushed pack the bed holds is `PackId(i)` for entry i; promotions are above 1,000,000.
        self.disk.order.borrow().len() - self.disk.idx_used.get()
    }
}

/// G5: a rehydrate loads a promoted pack through its stored `.idx`, with no `index-pack`, and the pushed packs the
/// promotion covers are not fetched at all; the network still serves every ref at the head. On main every pushed pack
/// and the promotion each ran `index-pack`, 6.6 to 7.3 s a 10 MB pack on m29 (`tests/fanout/raw/rehydrate-131296-m29.json`).
#[test]
fn a_rehydrate_of_promoted_packs_loads_their_stored_idx_and_skips_the_pushes_they_cover() {
    let b = bed(150, 0, 1);
    b.promote_range(1, 100);
    b.idx(100, true, Some((1, 100)));
    let r = b.run().unwrap_or_else(|e| panic!("{}", e.0));
    let h = r.rehydrated.expect("a rehydrate");
    assert_eq!(h.skipped, 100, "pushed packs 1..=100 are covered");
    assert!(h.fault.is_none(), "{:?}", h.fault);
    assert_eq!(b.disk.idx_used.get(), 1, "the promotion went in through its idx");
    assert_eq!(b.disk.index_packs.get(), 50, "only the tail's 50 pushed packs ran index-pack");
    assert_eq!(b.pushed_fetched(), 50);
    assert_eq!(b.disk.connected_calls.get(), 1, "one check before serving");
    // The check walks before the repack starts: started by the last ref page, the check waited out the whole repack
    // (`tests/am1/results/lm1-memory.md`, round 21).
    assert_eq!(*b.disk.events.borrow(), ["connected", "repack"]);
    b.assert_serves_head();
}

/// The rehydrate's last catch-up, from its first refs page to the head, starts no repack when the backstop follows:
/// the check's walk waits for any repack, so a repack one of its ref changes started put the whole repack and its
/// commit-graph inside the rehydrate, 99 to 268 s of the home's on B0-34 mt21, mt23 and mt24
/// (`tests/am1/results/b034-mt24.md`, "The home's rehydrate"). The walk runs first and starts the repack itself.
#[test]
fn a_rehydrates_last_catch_up_leaves_the_repack_to_the_backstop_after_its_walk() {
    let b = bed(150, 0, 1);
    b.promote_range(1, 100);
    b.idx(100, true, Some((1, 100)));
    // The log moved on while the refs were paged: the last catch-up applies entries 121..=151.
    b.log.first_refs_at.set(Some(120));
    let r = b.run().unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(r.entries, 31, "the last catch-up ran over 121..=151");
    assert_eq!(r.rehydrated.as_ref().expect("a rehydrate").skipped, 100);
    assert_eq!(*b.disk.events.borrow(), ["connected", "repack"], "the repack started before the walk");
    let steps: Vec<&str> = r.ms.iter().map(|(s, _)| *s).collect();
    assert!(steps.ends_with(&["refs", "catch_up", "walk"]), "{steps:?}");
    b.assert_serves_head();
}

/// A stored `.idx` that does not check against its pack (here, another pack's) is refused by the container, which
/// runs `index-pack` instead; the rehydrate is otherwise the same and serves the head.
#[test]
fn a_corrupt_stored_idx_falls_back_to_index_pack() {
    let b = bed(150, 0, 1);
    b.promote_range(1, 100);
    b.idx(100, false, Some((1, 100)));
    let r = b.run().unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(r.rehydrated.expect("a rehydrate").skipped, 100);
    assert_eq!(b.disk.idx_used.get(), 0);
    assert_eq!(b.disk.index_packs.get(), 51, "the promotion and the tail's 50 ran index-pack");
    b.assert_serves_head();
}

/// Rule 1: what a promotion covers comes from its committed `Promoted` record, and the `.idx` metadata only caches it.
/// Here the first promotion's metadata claims 1..=100 while its record says 1..=50, and the last promotion (51..=100)
/// has no stored idx. Pushed packs 51..=100 lie in no range both agree on, so they all load: trusting the metadata
/// would skip them with nothing to flag it.
#[test]
fn idx_metadata_wider_than_its_promoted_record_skips_nothing_outside_the_record() {
    let b = bed(150, 0, 1);
    b.promote_range(1, 50);
    b.idx(50, true, Some((1, 100)));
    b.promote_range(51, 100);
    let r = b.run().unwrap_or_else(|e| panic!("{}", e.0));
    let h = r.rehydrated.expect("a rehydrate");
    assert_eq!(h.skipped, 50, "only 1..=50, the record's range");
    let loaded: BTreeSet<u64> = b.disk.order.borrow().iter().filter_map(|s| b.pushed_index(s)).collect();
    assert!((51..=100).all(|i| loaded.contains(&i)), "a pushed pack in 51..=100 was skipped");
    assert!(!(1..=50).any(|i| loaded.contains(&i)));
    b.assert_serves_head();
}

/// The backstop: a promotion whose pack lacks an object (planted: entry 60's) passes the marked phase, since nothing
/// there checks connectivity, and fails the check before serving. The rehydrate loads the skipped pushed packs, checks
/// again, reports the fault, and serves the head.
#[test]
fn a_planted_missing_object_in_a_covered_pack_falls_back_to_the_skipped_packs_and_passes() {
    let b = bed(150, 0, 1);
    let p = Pack { brings: (1..=100).filter(|o| *o != 60).collect(), needs: vec![], pad: 0 };
    let bytes = bytes_of(&p);
    let id = PackId(Ulid(1_000_100));
    b.disk.bucket.borrow_mut().insert(id, bytes.clone());
    let at = b.log.head() + 1;
    b.disk.markers.borrow_mut().push((id, Marker { chain: CHAIN, index: Index(at), checksum: sum(&bytes), bytes: 1 }));
    let pack = PackRef::Stored { id, checksum: sum(&bytes), bytes: bytes.len() as u64, sites: SiteSet(3) };
    let pr = Promoted { from: Index(1), to: Index(100), pack };
    b.log.entries.borrow_mut().push(LogPacks {
        index: Index(at),
        packs: vec![],
        promoted: Some(pr.clone()),
        compacted: None,
        consumed: vec![],
        refs: Some(vec![]),
    });
    *b.log.promoted.borrow_mut() = Some(pr);
    b.idx(100, true, Some((1, 100)));
    let r = b.run().unwrap_or_else(|e| panic!("{}", e.0));
    let h = r.rehydrated.expect("a rehydrate");
    assert!(h.fault.as_deref().is_some_and(|f| f.contains("60")), "{:?}", h.fault);
    assert_eq!(b.disk.connected_calls.get(), 2, "checked, loaded the skipped packs, checked again");
    // A failed walk starts the repack too; only the second check, on the fault path, follows it.
    assert_eq!(*b.disk.events.borrow(), ["connected", "repack", "connected", "repack"]);
    b.assert_serves_head();
    assert!(b.store.rehydrating.borrow().is_none());
}

/// Three promotions of 1..=30, the first two rolled into R (1..=20) whose marker and stored `.idx` say the rolled
/// range, and pushed packs at 5, 15, 25. The rehydrate loads R in P1 and P2's place, oldest first with its `.idx`,
/// then P3: nothing else loads, so a cold shard reads 2 packs where it read 3 and every pushed one
/// (`decisions/DECISIONS.md`, `roll-up`, "Rehydrate prefers a rolled pack").
#[test]
fn a_roll_loads_its_produced_pack_in_place_of_the_promotions_it_consumed() {
    let b = bed(30, 0, 0);
    for i in [5, 15, 25] {
        b.stored_at(i);
    }
    b.promote_range(1, 10);
    b.promote_range(11, 20);
    b.promote_range(21, 30);
    let p1 = PackId(Ulid(1_000_010));
    let p2 = PackId(Ulid(1_000_020));
    let rolled = b.roll(1, 20, &[p1, p2]);
    let produced = b.disk.bucket.borrow()[&rolled].clone();
    b.disk.markers.borrow_mut().push((
        rolled,
        Marker { chain: CHAIN, index: Index(b.log.head()), checksum: sum(&produced), bytes: produced.len() as u64 },
    ));
    b.rolled_idx(rolled, Some((1, 20)));
    b.idx(30, true, Some((21, 30)));
    let r = b.run().unwrap_or_else(|e| panic!("{}", e.0));
    let h = r.rehydrated.expect("a rehydrate");
    // R loads first, then P3: nothing loads twice between them, and no consumed promotion loads.
    assert_eq!(b.ingested_ids(), [2_000_020, 1_000_030], "R replaced P1 and P2 and loads first");
    assert_eq!(
        *b.disk.fetch_order.borrow(),
        [rolled, PackId(Ulid(1_000_030))],
        "the produced pack is fetched before the promotion after its range"
    );
    // The pushed packs of the rolled range and of P3's range load nowhere.
    assert_eq!(h.skipped, 3, "the pushed packs at 5, 15 and 25");
    assert!(h.fault.is_none(), "{:?}", h.fault);
    assert_eq!(b.disk.idx_used.get(), 2, "R and P3 both went in through their idx");
    assert_eq!(b.disk.index_packs.get(), 0);
    b.assert_serves_head();
}

/// The same log with R's marker never written: the roll does not count, so its consumed promotions still load, while
/// they exist, and the produced pack loads nowhere. The disk ends up with the same objects either way.
#[test]
fn a_roll_without_a_marker_for_its_produced_pack_consumes_nothing() {
    let b = bed(30, 0, 0);
    for i in [5, 15, 25] {
        b.stored_at(i);
    }
    b.promote_range(1, 10);
    b.promote_range(11, 20);
    b.promote_range(21, 30);
    let p1 = PackId(Ulid(1_000_010));
    let p2 = PackId(Ulid(1_000_020));
    let rolled = b.roll(1, 20, &[p1, p2]);
    b.idx(30, true, Some((21, 30)));
    b.run().unwrap_or_else(|e| panic!("{}", e.0));
    // The three promotions load in range order, the pushed packs nothing covers behind them, and R never.
    assert_eq!(b.ingested_ids(), [1_000_010, 1_000_020, 1_000_030, 5, 15]);
    assert!(!b.disk.fetch_order.borrow().contains(&rolled), "R has no marker, so nothing loads it");
    b.assert_serves_head();
}

/// The same roll, counted (marked, with its `.idx`), but its produced pack is in no bucket: the rehydrate drops the
/// roll and loads the promotions it consumed, which nothing deletes yet, then serves the head. Without the fallback
/// the produced pack's failed fetch ends the rehydrate, and every later one fails the same way
/// (`decisions/DECISIONS.md`, `roll-up`, "Rehydrate prefers a rolled pack").
#[test]
fn a_roll_whose_produced_pack_does_not_load_falls_back_to_the_packs_it_consumed() {
    let b = bed(40, 0, 0);
    for i in [5, 15, 25] {
        b.stored_at(i);
    }
    b.promote_range(1, 10);
    b.promote_range(11, 20);
    b.promote_range(21, 30);
    let p1 = PackId(Ulid(1_000_010));
    let p2 = PackId(Ulid(1_000_020));
    let rolled = b.mark_roll(1, 20, &[p1, p2]);
    // A promotion after the roll whose range reaches past the roll's entry puts the roll's marker below the tail
    // start, in the marked phase, where only the drop keeps the missing pack out of the plan.
    let at = b.log.head();
    b.promote_range(31, at);
    b.idx(10, true, Some((1, 10)));
    b.idx(20, true, Some((11, 20)));
    b.idx(30, true, Some((21, 30)));
    b.disk.bucket.borrow_mut().remove(&rolled);
    let r = b.run().unwrap_or_else(|e| panic!("the rehydrate failed on the dropped roll: {}", e.0));
    let h = r.rehydrated.expect("a rehydrate");
    assert_eq!(h.dropped_rolls.len(), 1, "{:?}", h.dropped_rolls);
    // The consumed promotions load in R's place, in range order, through their `.idx`; the pushed packs they cover
    // stay skipped, as before any roll.
    assert_eq!(b.ingested_ids(), [1_000_010, 1_000_020, 1_000_030, 1_000_000 + at as u128]);
    assert_eq!(h.skipped, 3);
    assert!(h.fault.is_none(), "{:?}", h.fault);
    b.assert_serves_head();
}

/// Two rolls, the later range's produced pack building on the earlier's: each loads, oldest range first, before
/// the promotion after them, and neither is dropped.
#[test]
fn two_rolls_load_oldest_first_and_both_count() {
    let b = bed(40, 0, 0);
    for k in 1..=4 {
        b.promote_range(10 * k - 9, 10 * k);
    }
    let p = |to: u128| PackId(Ulid(1_000_000 + to));
    b.mark_roll(21, 30, &[p(30)]);
    b.mark_roll(1, 20, &[p(10), p(20)]);
    b.idx(40, true, Some((31, 40)));
    let r = b.run().unwrap_or_else(|e| panic!("{}", e.0));
    assert!(r.rehydrated.expect("a rehydrate").dropped_rolls.is_empty());
    assert_eq!(b.ingested_ids(), [2_000_020, 2_000_030, 1_000_040]);
    b.assert_serves_head();
}

impl Bed {
    /// `roll`, with the produced pack's `Committed` marker at the roll's entry and its stored `.idx`.
    fn mark_roll(&self, from: u64, to: u64, consumed: &[PackId]) -> PackId {
        let id = self.roll(from, to, consumed);
        let b = self.disk.bucket.borrow()[&id].clone();
        self.disk.markers.borrow_mut().push((
            id,
            Marker { chain: CHAIN, index: Index(self.log.head()), checksum: sum(&b), bytes: b.len() as u64 },
        ));
        self.rolled_idx(id, Some((from, to)));
        id
    }

    /// The produced pack's stored `.idx`, with the range its custom metadata names.
    fn rolled_idx(&self, id: PackId, range: Option<(u64, u64)>) {
        let b = self.disk.bucket.borrow().get(&id).cloned().expect("the produced pack");
        self.disk.idx.borrow_mut().insert(id, (sum(&b), range.map(|(f, t)| (CHAIN, Index(f), Index(t)))));
    }

    /// The ids of the packs the container ingested, in ingested order.
    fn ingested_ids(&self) -> Vec<u128> {
        self.disk
            .order
            .borrow()
            .iter()
            .map(|s| {
                self.disk
                    .bucket
                    .borrow()
                    .iter()
                    .find(|(_, b)| sum(b) == *s)
                    .map(|(id, _)| id.0.0)
                    .expect("the container ingested a bucket pack")
            })
            .collect()
    }

    /// The entry index of a pushed pack by its checksum, or `None` for a promotion.
    fn pushed_index(&self, s: &Digest) -> Option<u64> {
        self.disk
            .bucket
            .borrow()
            .iter()
            .find(|(_, b)| sum(b) == *s)
            .map(|(id, _)| id.0.0 as u64)
            .filter(|i| *i < 1_000_000)
    }
}

/// A catch-up behind K stored packs fetches several at once and still ingests them in log order: the first pack's
/// fetch is slow, so the later ones finish first, and none goes in before it. Within `FETCH_AT_ONCE`, which leaves two
/// of a Workers invocation's six connections waiting for headers to the ingest and the next log page [doc].
#[test]
fn a_catch_up_fetches_stored_packs_at_once_and_ingests_them_in_log_order() {
    use graftyard_git::catchup::FETCH_AT_ONCE;
    let b = bed(80, 0, 1);
    let marks = b.disk.markers.borrow().clone();
    b.log.entries.borrow_mut().truncate(20);
    b.disk.markers.borrow_mut().retain(|(_, m)| m.index.0 <= 20);
    b.run().unwrap();
    b.log.entries.replace(bed(80, 0, 1).log.entries.into_inner());
    *b.disk.markers.borrow_mut() = marks;
    b.disk.order.borrow_mut().clear();
    b.disk.fetch_ms.set(500);
    b.disk.slow.borrow_mut().insert(PackId(Ulid(21)), 5_000);
    let r = b.run_timed().unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(r.packs, 60);
    let order: Vec<u64> = b.disk.order.borrow().iter().filter_map(|s| b.pushed_index(s)).collect();
    assert_eq!(order, (21..=80).collect::<Vec<_>>(), "ingested out of log order");
    assert!(b.disk.in_flight_max.get() > 1 && b.disk.in_flight_max.get() <= FETCH_AT_ONCE);
    const { assert!(FETCH_AT_ONCE + 2 <= 6, "the ingest and the next page keep a connection each") };
    // One after another: 5 s + 59 × 0.5 s = 34.5 s. Ahead: while pack 21 takes 5 s, only `AHEAD_PACKS` - 1 more are
    // fetched past it, so the other 52 follow at `FETCH_AT_ONCE` at a time: 5 s + 13 × 0.5 s = 11.5 s.
    let serial = 5_000 + 59 * 500;
    let ahead = 5_000 + (59 - (graftyard_git::catchup::AHEAD_PACKS as u64 - 1)).div_ceil(FETCH_AT_ONCE as u64) * 500;
    let took = b.disk.clock.now.get();
    assert!(took <= ahead, "{took} ms against {ahead} ms bounded, {serial} ms one at a time");
    b.assert_serves_head();
}

/// The 120 s line for a cold rehydrate of a 5 GB network (`design/DESIGN.md`, "Failure modes", git caches), projected
/// from m29's measured cold rehydrate of repo:131296 (`tests/fanout/raw/rehydrate-131296-m29.json`): 257 packs,
/// 2,623,972,857 bytes. Site c, the slower: container up 34.2 s; spool 94.0 s in all, so 27.9 MB/s on one stream; and
/// 96 s outside spool and `index-pack`, 375 ms a pack. A stored `.idx` adds its bytes, 3.5% of the pack's (a 57.9 MB
/// promoted pack's was 2.0 MB), and holds the lock about 1 ms (`containers/git/server/stored_test.go`).
///
/// The open variable is R2's throughput with `FETCH_AT_ONCE` streams into one container: the projection assumes each
/// stream keeps the one-stream rate. If the container's link or R2 caps the sum at the one-stream rate, the fetch saves
/// only the per-pack overhead, and the 5 GB case does not reach 120 s.
#[test]
fn a_cold_rehydrate_of_5_gb_projects_under_120_s_and_the_serial_fetch_does_not() {
    use graftyard_git::catchup::FETCH_AT_ONCE;
    const UP_MS: u64 = 34_200;
    const PER_PACK_MS: u64 = 375;
    const BYTES_PER_MS: f64 = 2_623_972_857.0 / 94_004.0;
    const IDX_SHARE: f64 = 0.035;
    let fetch = |bytes: f64| PER_PACK_MS + (bytes * (1.0 + IDX_SHARE) / BYTES_PER_MS) as u64;
    let mut out = Vec::new();
    // The seed as it stands (257 packs of 10.2 MB), and 5 GB in promotions of `PROMOTE_MAX` (48 MiB) each.
    for (what, n, bytes) in [("seed", 257u64, 2_623_972_857.0 / 257.0), ("5 GB", 100, 50_331_648.0)] {
        let b = bed(n, 0, 1);
        for k in 1..=n {
            b.promote_range(k, k);
            b.idx(k, true, Some((k, k)));
        }
        b.disk.fetch_ms.set(fetch(bytes));
        let r = b.run_timed().unwrap_or_else(|e| panic!("{what}: {}", e.0));
        let h = r.rehydrated.expect("a rehydrate");
        assert_eq!((h.skipped as u64, b.disk.idx_used.get() as u64), (n, n), "{what}");
        assert_eq!(b.disk.index_packs.get(), 0, "{what}");
        let took = UP_MS + b.disk.clock.now.get();
        let serial = UP_MS + n * fetch(bytes);
        out.push((what, took / 1_000, serial / 1_000));
        assert!(took <= 120_000, "{what}: {took} ms with {FETCH_AT_ONCE} fetches at once");
        assert!(serial > 120_000, "{what}: {serial} ms one at a time is already under the line");
        b.assert_serves_head();
    }
    println!("cold rehydrate, site c rates: (case, s with the fetch ahead, s one at a time) {out:?}");
}

/// Repo:131296's promoted packs were 106 to 207 MB each, every one past `AHEAD_BYTES`, so main's cap fetched them one
/// at a time: the marked phase took 101 to 155 s of m35's cold warm (`tests/am1/results/lm1-memory.md`, round 23). The
/// first `FETCH_AT_ONCE` packs ahead start whatever their size, and the disk, not the object, holds them.
#[test]
fn large_promoted_packs_fetch_four_at_once_and_still_ingest_in_log_order() {
    use graftyard_git::catchup::{AHEAD_BYTES, FETCH_AT_ONCE};
    const BIG: u64 = 207_000_000;
    const { assert!(BIG > AHEAD_BYTES) };
    let n = 12;
    let b = bed(n, 0, 1);
    for k in 1..=n {
        b.promote_range(k, k);
        b.idx(k, true, Some((k, k)));
    }
    // The markers the rehydrate plans from name each promoted pack at 207 MB.
    for (id, m) in b.disk.markers.borrow_mut().iter_mut() {
        if id.0.0 >= 1_000_000 {
            m.bytes = BIG;
        }
    }
    b.disk.fetch_ms.set(2_000);
    let r = b.run_timed().unwrap_or_else(|e| panic!("{}", e.0));
    let h = r.rehydrated.expect("a rehydrate");
    assert_eq!((h.skipped as u64, b.disk.idx_used.get() as u64), (n, n));
    assert_eq!(b.disk.in_flight_max.get(), FETCH_AT_ONCE, "large packs fetched {} at once", b.disk.in_flight_max.get());
    let promoted: Vec<Digest> =
        (1..=n).map(|k| sum(&b.disk.bucket.borrow()[&PackId(Ulid(1_000_000 + k as u128))])).collect();
    assert_eq!(*b.disk.order.borrow(), promoted, "ingested out of log order");
    // Twelve 2 s fetches four at a time: 6 s, against 24 s one at a time.
    let took = b.disk.clock.now.get();
    assert!(took <= 3 * 2_000 + 1_000, "{took} ms for {n} large packs");
    b.assert_serves_head();
}

/// G26: the shard's HEAD is the repository's default branch, which the head read answers. A rehydrate names it on its
/// last ref page; a catch-up names it only when it changed since (a `Settings` entry carries no ref change, so nothing
/// else would move HEAD), once, and not on every catch-up after; a new disk names it again. A member that answers no
/// default branch leaves the container's own rule.
#[test]
fn head_follows_the_default_branch_and_is_named_only_when_it_changes() {
    let b = bed(30, 10, 0);
    *b.log.default_branch.borrow_mut() = Some("refs/heads/b3".into());
    assert!(b.run().unwrap().rehydrated.is_some());
    assert_eq!(b.disk.head.borrow().as_deref(), Some("refs/heads/b3"));
    assert_eq!(b.disk.head_calls.get(), 1, "the rehydrate names HEAD once, on its last ref page");

    // Catch-ups with the default unchanged name nothing, though their entries move refs.
    b.log.entries.replace(bed(60, 10, 0).log.entries.into_inner());
    b.run().unwrap();
    b.run().unwrap();
    assert_eq!(b.disk.head_calls.get(), 1, "an unchanged default branch costs no call");

    // A Settings entry changes the default and moves no ref: the next catch-up names it, the one after does not.
    *b.log.default_branch.borrow_mut() = Some("refs/heads/b5".into());
    b.log.entries.borrow_mut().push(LogPacks {
        index: Index(61),
        packs: vec![],
        promoted: None,
        compacted: None,
        consumed: vec![],
        refs: Some(vec![]),
    });
    b.run().unwrap();
    assert_eq!(b.disk.head.borrow().as_deref(), Some("refs/heads/b5"));
    assert_eq!(b.disk.head_calls.get(), 2);
    b.run().unwrap();
    assert_eq!(b.disk.head_calls.get(), 2);

    // A new disk rehydrates and names it again, whatever the cursor said before.
    b.disk.reset("disk-2");
    assert!(b.run().unwrap().rehydrated.is_some());
    assert_eq!(b.disk.head.borrow().as_deref(), Some("refs/heads/b5"));
    assert_eq!(b.disk.head_calls.get(), 3);
    b.assert_serves_head();

    // An older member answers no default branch: nothing is named, and the container keeps its rule.
    let old = bed(30, 10, 0);
    old.run().unwrap();
    old.log.entries.replace(bed(40, 10, 0).log.entries.into_inner());
    old.run().unwrap();
    assert_eq!((old.disk.head_calls.get(), old.disk.head.borrow().clone()), (0, None));
}

/// `&head=` survives the characters a branch name may hold and a query string would otherwise split or decode.
#[test]
fn the_head_parameter_is_percent_encoded() {
    use graftyard_git::catchup::head_param;
    assert_eq!(head_param(None), "");
    assert_eq!(head_param(Some("refs/heads/v1.x")), "&head=refs/heads/v1.x");
    assert_eq!(head_param(Some("refs/heads/a&b+c#d%e")), "&head=refs/heads/a%26b%2Bc%23d%25e");
    assert_eq!(head_param(Some("refs/heads/é")), "&head=refs/heads/%C3%A9");
}

/// A catch-up's page time splits into `make_room`, the ingests, the `/refs` calls and the cursor saves (`PageMs`), so a
/// row can tell which one ran long: B0-34 mt7's monorepo catch-ups ran 5 to 17.6 s with most of it in no timed lap
/// (`tests/am1/results/b034-mt7.md`). Each part takes its own time here, so a part counted in another, or a page timed
/// once instead of per part, gives other sums.
#[test]
fn a_catch_up_times_room_ingest_and_refs_apart() {
    let b = bed(30, 0, 5);
    b.log.entries.borrow_mut().truncate(10);
    b.disk.markers.borrow_mut().retain(|(_, m)| m.index.0 <= 10);
    b.run().unwrap();
    b.log.entries.replace(bed(30, 0, 5).log.entries.into_inner());
    *b.disk.markers.borrow_mut() = bed(30, 0, 5).disk.markers.into_inner();
    b.disk.fetch_ms.set(300);
    b.disk.index_ms.set(0);
    b.disk.refs_ms.set(1_700);
    b.disk.room_ms.set(900);
    let (t0, calls) = (b.disk.clock.now.get(), b.log.calls.get());
    let r = b.run_timed().unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(r.entries, 20);
    let pages = b.log.calls.get() - calls;
    let p = &r.pages_ms;
    assert_eq!(pages, 1, "20 small entries are one page");
    assert_eq!(p.room, 900, "one make_room for the page");
    assert_eq!(p.refs, 1_700, "one /refs call for the page's ref changes");
    // Entries 15, 20, 25 and 30 are stored: one fetch round at FETCH_AT_ONCE, then the inline batch at once.
    assert_eq!(p.ingest, 300);
    assert_eq!(p.save, 0);
    assert_eq!(p.room + p.ingest + p.refs + p.save, b.disk.clock.now.get() - t0, "the parts cover the page");
    b.assert_serves_head();
}

/// G34: a catch-up behind a burst of staged packs goes in pieces. Each call applies whole entries up to `PIECE`, saves
/// its cursor, and says another follows; the later pieces go to the head the first read and no further, however far the
/// log moved meanwhile, and a lone entry over the cap still goes. On B0-32 m3fst1 one call ingested 294 packs (74.5 MB)
/// in 15.8 s while 53 validates and a merge waited on it (`decisions/DECISIONS.md`, "`catchup-pieces`").
#[test]
fn a_burst_catch_up_goes_in_capped_pieces_to_the_head_its_first_piece_read() {
    let b = bed(120, 0, 1);
    let all = b.log.entries.replace(Vec::new());
    let marks = b.disk.markers.borrow().clone();
    b.log.entries.borrow_mut().extend(all.iter().take(10).cloned());
    b.disk.markers.borrow_mut().retain(|(_, m)| m.index.0 <= 10);
    b.run().unwrap();
    *b.disk.markers.borrow_mut() = marks;
    b.log.entries.borrow_mut().extend(all.iter().skip(10).take(90).cloned());
    let run = RefCell::new(Run::default());
    let piece = |fresh: bool| {
        if fresh {
            *run.borrow_mut() = Run::default();
        }
        block_on(catch_up(&b.disk, &b.store, &b.log, NET, &[CHAIN], PIECE, &mut run.borrow_mut(), &|| 0)).unwrap()
    };
    let at = || b.store.cursor.borrow().clone().unwrap().1.at(CHAIN).0;
    // 90 stored packs past the cursor, one an entry: 32, 32, then 26.
    let first = piece(true);
    assert!(first.behind, "the first piece stops at the cap");
    assert_eq!((first.packs, at()), (PIECE.packs, 10 + PIECE.packs as u64), "the cursor is saved at the cap");
    assert_eq!(first.until.at(CHAIN), Index(100), "the run ends at the head the first piece read");
    // The log moves on while the run goes; its pieces still end at 100.
    b.log.entries.borrow_mut().extend(all.iter().skip(100).cloned());
    let second = piece(false);
    assert!(second.behind);
    assert_eq!((second.packs, at()), (PIECE.packs, 10 + 2 * PIECE.packs as u64));
    let last = piece(false);
    assert!(!last.behind, "the run ends at its head");
    assert_eq!((last.packs, at()), (90 - 2 * PIECE.packs, 100));
    assert!(b.disk.objects.borrow().contains(&100) && !b.disk.objects.borrow().contains(&101));
    // The next run reads the head again and goes past it.
    while piece(true).behind {}
    b.assert_serves_head();
    // A lone entry past the cap goes alone, never zero entries a call.
    let big = Piece { packs: 1, bytes: 1 };
    let c = bed(4, 2_000, 0);
    let more = c.log.entries.replace(Vec::new());
    c.log.entries.borrow_mut().push(more[0].clone());
    c.run().unwrap();
    c.log.entries.borrow_mut().extend(more.into_iter().skip(1));
    let mut calls = 0;
    loop {
        calls += 1;
        let r = block_on(catch_up(&c.disk, &c.store, &c.log, NET, &[CHAIN], big, &mut Run::default(), &|| 0)).unwrap();
        assert_eq!(r.entries, 1, "one entry a call at a cap below one entry's bytes");
        if !r.behind {
            break;
        }
    }
    assert_eq!(calls, 3);
    c.assert_serves_head();
}

/// A disk caught up to entry 10 of `b`'s log, the rest of the log committed after.
fn warm_to_10(b: &Bed) {
    let all = b.log.entries.replace(Vec::new());
    let marks = b.disk.markers.borrow().clone();
    b.log.entries.borrow_mut().extend(all.iter().take(10).cloned());
    b.disk.markers.borrow_mut().retain(|(_, m)| m.index.0 <= 10);
    b.run().unwrap();
    *b.disk.markers.borrow_mut() = marks;
    b.log.entries.borrow_mut().extend(all.into_iter().skip(10));
}

/// G39 (`decisions/DECISIONS.md`, "`catchup-lag`"): a run's pieces share one `Run`, so the run asks `make_room` once per
/// `ROOM_EVERY` and reads each log page once, as one call to the head would. Before, every piece walked the disk and
/// read its page again from the cursor: B0-33 erDA's home shard spent 16 and 17 to 21 ms a pack there, and took 11.8
/// packs a second against 18 offered (`tests/am1/results/catchup-lag-design.md`).
#[test]
fn a_runs_pieces_walk_the_disk_once_and_read_each_page_once() {
    let b = bed(100, 0, 1);
    warm_to_10(&b);
    let (rooms, reads, served, ingests) =
        (b.disk.rooms.get(), b.log.calls.get(), b.log.served.get(), b.disk.ingests.get());
    let mut run = Run::default();
    let mut pieces = Vec::new();
    loop {
        let r = block_on(catch_up(&b.disk, &b.store, &b.log, NET, &[CHAIN], PIECE, &mut run, &|| 0)).unwrap();
        pieces.push((r.packs, r.room_calls));
        if !r.behind {
            break;
        }
    }
    // 90 stored packs, one an entry, all in one 600 KB page (a stored pack counts 100 bytes there): 32, 32, 26.
    assert_eq!(pieces, vec![(32, 1), (32, 0), (26, 0)], "one disk walk for the run, in its first piece");
    assert_eq!(b.disk.rooms.get() - rooms, 1);
    assert_eq!(b.log.calls.get() - reads, 1, "the page is read once, not once a piece");
    assert_eq!(b.log.served.get() - served, 90, "no entry is read twice");
    assert_eq!(b.disk.ingests.get() - ingests, 90, "no pack is ingested twice");
    assert!(run.until.is_none(), "the run's last piece leaves nothing to carry");
    b.assert_serves_head();
    // The next run starts again: one more disk walk for its first page.
    let more = bed(101, 0, 1);
    let id = PackId(Ulid(101));
    b.disk.bucket.borrow_mut().insert(id, more.disk.bucket.borrow()[&id].clone());
    b.disk.markers.borrow_mut().push(more.disk.markers.borrow()[100].clone());
    b.log.entries.borrow_mut().push(more.log.entries.borrow()[100].clone());
    let r = block_on(catch_up(&b.disk, &b.store, &b.log, NET, &[CHAIN], PIECE, &mut run, &|| 0)).unwrap();
    assert_eq!((r.packs, r.room_calls, r.behind), (1, 1, false));
}

/// A carried tail is used only while it starts just past the cursor: a cursor that moved between two pieces (here
/// another run's) reads its page again rather than applying entries the disk already holds or skipping some.
#[test]
fn a_carried_tail_past_the_cursor_is_read_again() {
    let b = bed(100, 0, 1);
    warm_to_10(&b);
    let mut run = Run::default();
    let first = block_on(catch_up(&b.disk, &b.store, &b.log, NET, &[CHAIN], PIECE, &mut run, &|| 0)).unwrap();
    assert!(first.behind);
    // Another run takes the cursor to 60 between the pieces; this run's tail starts at 43.
    let mut other = Run::default();
    let cap = Piece { packs: 18, bytes: u64::MAX };
    let r = block_on(catch_up(&b.disk, &b.store, &b.log, NET, &[CHAIN], cap, &mut other, &|| 0)).unwrap();
    assert_eq!(r.cursor.at(CHAIN), Index(60));
    let (reads, ingests) = (b.log.calls.get(), b.disk.ingests.get());
    let mut pieces = 0;
    loop {
        pieces += 1;
        let r = block_on(catch_up(&b.disk, &b.store, &b.log, NET, &[CHAIN], PIECE, &mut run, &|| 0))
            .unwrap_or_else(|e| panic!("{}", e.0));
        if !r.behind {
            break;
        }
    }
    assert_eq!(pieces, 2, "61..=100 in two pieces");
    assert!(b.log.calls.get() > reads, "the page from 61 is read, not the tail from 43");
    assert_eq!(b.disk.ingests.get() - ingests, 40, "61..=100 once each");
    b.assert_serves_head();
}

/// G39 (`catchup-lag`): stored packs whose fetches have ended go to the container in one call, one `index-pack`, in log
/// order; a pack whose fetch is still running is not waited for, so a batch never holds an ingest behind a fetch. On
/// erDA's home shard each stored pack was its own call and `index-pack`, 48.5 ms a pack against 27 to 33 on the pools.
#[test]
fn ready_stored_packs_go_in_one_call_and_a_slow_fetch_is_not_waited_for() {
    let b = bed(30, 0, 1);
    warm_to_10(&b);
    b.disk.fetch_ms.set(100);
    b.disk.index_ms.set(40);
    // Entry 15's fetch takes 5 s: the packs before it go without it, and it does not hold them.
    b.disk.slow.borrow_mut().insert(PackId(Ulid(15)), 5_000);
    let (t0, ingests, before) = (b.disk.clock.now.get(), b.disk.ingests.get(), b.disk.batches.borrow().len());
    let r = b.run_timed().unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(r.packs, 20);
    assert_eq!(b.disk.ingests.get() - ingests, 20, "every pack once");
    let calls = b.disk.batches.borrow()[before..].to_vec();
    assert_eq!(calls.first(), Some(&4), "11..=14 go without waiting on 15's fetch: {calls:?}");
    assert_eq!(calls.iter().sum::<usize>(), 20, "every pack in a batch: {calls:?}");
    assert!(calls.len() <= 5, "a batch carries the packs whose fetches ended, not one pack: {calls:?}");
    // The ingest order is the log's, whatever the batches.
    let order: Vec<u64> = b
        .disk
        .order
        .borrow()
        .iter()
        .rev()
        .take(20)
        .rev()
        .map(|s| b.disk.bucket.borrow().iter().find(|(_, v)| sum(v) == *s).map(|(k, _)| k.0.0 as u64).unwrap())
        .collect();
    assert_eq!(order, (11..=30).collect::<Vec<_>>());
    // The slow fetch plus a few batches; one index-pack a pack after it (16..=30) would be 5,000 + 15 * 40.
    let took = b.disk.clock.now.get() - t0;
    assert!(took < 5_000 + 10 * 40, "{took} ms");
    b.assert_serves_head();
}

// ---- AM3-WP2a head 4: a fork chain's refs in its own namespace on the network's disk ----

const FORK: ChainId = ChainId::Repo(RepoId(DbId(77)));

/// The network's two member chains, the root's (`CHAIN`) and a fork's (`FORK`), each its own group's log. `down`:
/// the fork's group answers nothing, as another owner's placement that is down or a deleted fork's.
struct Net<'a> {
    root: &'a Log,
    fork: &'a Log,
    down: Cell<bool>,
}

impl Net<'_> {
    fn of(&self, c: ChainId) -> &Log {
        if c == FORK { self.fork } else { self.root }
    }
}

impl ChainLog for Net<'_> {
    async fn head(&self, c: ChainId) -> Result<Head, StoreError> {
        if c == FORK && self.down.get() {
            return Err(StoreError("repo:77: no group".into()));
        }
        ChainLog::head(self.of(c), c).await
    }
    async fn refs_page(&self, c: ChainId, after: Option<String>) -> Result<(RefsPage, Index), StoreError> {
        self.of(c).refs_page(c, after).await
    }
    async fn entries(&self, c: ChainId, from: Index, to: Index) -> Result<Vec<LogPacks>, StoreError> {
        self.of(c).entries(c, from, to).await
    }
    async fn last_promoted(&self, c: ChainId) -> Result<Option<(Index, Promoted)>, StoreError> {
        self.of(c).last_promoted(c).await
    }
    async fn compactions(&self, c: ChainId) -> Result<Vec<Compacted>, StoreError> {
        self.of(c).compactions(c).await
    }
}

/// A fork's log, entries 1..=n: each a push of object 500 + i onto the root's object 3, to the root's own branch
/// names (`refs/heads/b<i % 7>`) at other values, so a fork ref written outside its namespace overwrites a root's.
fn fork_log(n: u64) -> Log {
    let l = bed(0, 0, 0).log;
    for i in 1..=n {
        let p = Pack { brings: vec![500 + i], needs: vec![if i == 1 { 3 } else { 500 + i - 1 }], pad: 0 };
        let mut refs = vec![(format!("refs/heads/b{}", i % 7), Some(oid(500 + i)))];
        if i == 9 {
            refs.push(("refs/heads/b4".to_owned(), None));
        }
        l.entries.borrow_mut().push(LogPacks {
            index: Index(i),
            packs: vec![PackRef::Inline(bytes_of(&p))],
            promoted: None,
            compacted: None,
            consumed: vec![],
            refs: Some(refs),
        });
    }
    *l.default_branch.borrow_mut() = Some("refs/heads/b2".into());
    l
}

/// The disk's refs a network serves: the root's where they always were, each fork's under `refs/namespaces/77/`.
fn network_refs(root: &Log, fork: &Log) -> BTreeMap<String, String> {
    let mut want = root.refs();
    want.extend(fork.refs().into_iter().map(|(n, o)| (format!("refs/namespaces/77/{n}"), o)));
    want
}

/// A rehydrate and the catch-ups after it put a fork chain's refs and its HEAD in the fork's namespace, beside the
/// root's unchanged.
#[test]
fn a_fork_chains_refs_and_head_land_in_its_namespace_beside_the_roots() {
    let b = bed(30, 10, 0);
    *b.log.default_branch.borrow_mut() = Some("refs/heads/b3".into());
    let fork = fork_log(5);
    let net = Net { root: &b.log, fork: &fork, down: Cell::new(false) };
    let run = |chains: &[ChainId]| {
        block_on(catch_up(&b.disk, &b.store, &net, NET, chains, WHOLE, &mut Run::default(), &|| 0)).unwrap()
    };
    assert!(run(&[CHAIN, FORK]).rehydrated.is_some());
    assert_eq!(*b.disk.refs.borrow(), network_refs(&b.log, &fork), "after the rehydrate");
    assert_eq!(b.disk.head.borrow().as_deref(), Some("refs/heads/b3"));
    let ns_head = Some(("refs/namespaces/77/HEAD".to_owned(), "refs/namespaces/77/refs/heads/b2".to_owned()));
    assert_eq!(b.disk.ns_heads.borrow().clone().into_iter().next(), ns_head);

    // Both chains move, a fork ref is deleted, and the fork's default branch changes with no ref change.
    b.log.entries.replace(bed(45, 10, 0).log.entries.into_inner());
    fork.entries.replace(fork_log(9).entries.into_inner());
    *fork.default_branch.borrow_mut() = Some("refs/heads/b5".into());
    let calls = b.disk.head_calls.get();
    let r = run(&[CHAIN, FORK]);
    assert!(r.rehydrated.is_none());
    assert_eq!((r.cursor.at(CHAIN), r.cursor.at(FORK)), (Index(45), Index(9)));
    assert_eq!(*b.disk.refs.borrow(), network_refs(&b.log, &fork), "after the catch-up");
    assert!(!b.disk.refs.borrow().contains_key("refs/namespaces/77/refs/heads/b4"), "the fork's delete");
    assert!(b.disk.refs.borrow().contains_key("refs/heads/b4"), "the root's ref of the same name stays");
    assert_eq!(b.disk.head.borrow().as_deref(), Some("refs/heads/b3"), "the root's HEAD is not the fork's");
    assert_eq!(b.disk.ns_heads.borrow()["refs/namespaces/77/HEAD"], "refs/namespaces/77/refs/heads/b5");
    assert_eq!(b.disk.head_calls.get(), calls + 1, "only the fork's changed HEAD is named");
    run(&[CHAIN, FORK]);
    assert_eq!(b.disk.head_calls.get(), calls + 1, "an unchanged fork HEAD costs no call");

    // A root push names the fork's namespace, to set and then delete a ref there: neither reaches the fork's refs.
    let at = b.log.head();
    let fork_main = "refs/namespaces/77/refs/heads/b2".to_owned();
    for (i, o) in [(at + 1, Some(oid(45))), (at + 2, None)] {
        b.log.entries.borrow_mut().push(LogPacks {
            index: Index(i),
            packs: vec![],
            promoted: None,
            compacted: None,
            consumed: vec![],
            refs: Some(vec![(fork_main.clone(), o)]),
        });
    }
    let want = b.disk.refs.borrow()[&fork_main].clone();
    b.log.entries.borrow_mut().pop();
    run(&[CHAIN, FORK]);
    assert_eq!(b.disk.refs.borrow()[&fork_main], want, "a root's set is not the fork's ref");
    b.log.entries.borrow_mut().push(LogPacks {
        index: Index(at + 2),
        packs: vec![],
        promoted: None,
        compacted: None,
        consumed: vec![],
        refs: Some(vec![(fork_main.clone(), None)]),
    });
    run(&[CHAIN, FORK]);
    assert_eq!(b.disk.refs.borrow()[&fork_main], want, "a root's delete is not the fork's ref");
    // A new disk rehydrates without the root's ref of that name.
    b.disk.reset("disk-2");
    assert!(run(&[CHAIN, FORK]).rehydrated.is_some());
    assert_eq!(b.disk.refs.borrow()[&fork_main], want, "a rehydrate keeps the fork's own ref");
}

/// A fork chain that does not answer is left out of the run with its cursor where it was: the root's catch-up goes on.
/// And a fork new to a disk that already serves the network, or one whose log was archived past its cursor, joins
/// alone, with no rehydrate of the network.
#[test]
fn a_fork_that_does_not_answer_holds_nothing_back_and_a_new_fork_joins_without_a_rehydrate() {
    let b = bed(30, 10, 0);
    let fork = fork_log(5);
    let net = Net { root: &b.log, fork: &fork, down: Cell::new(false) };
    let run = |chains: &[ChainId]| {
        block_on(catch_up(&b.disk, &b.store, &net, NET, chains, WHOLE, &mut Run::default(), &|| 0))
    };
    // The root alone first: the disk serves it.
    assert!(run(&[CHAIN]).unwrap().rehydrated.is_some());
    let inits = b.disk.inits.get();
    // The fork's first call: it joins, its refs land in its namespace, and the root's disk is not rebuilt.
    let r = run(&[CHAIN, FORK]).unwrap();
    assert!(r.rehydrated.is_none(), "a fork's first call is no rehydrate of the network");
    assert_eq!(b.disk.inits.get(), inits);
    assert_eq!(r.cursor.at(FORK), Index(5));
    assert_eq!(*b.disk.refs.borrow(), network_refs(&b.log, &fork));
    assert_eq!(b.disk.ns_heads.borrow()["refs/namespaces/77/HEAD"], "refs/namespaces/77/refs/heads/b2");

    // The fork's group stops answering while both chains move: the root catches up, the fork's cursor stays.
    b.log.entries.replace(bed(40, 10, 0).log.entries.into_inner());
    fork.entries.replace(fork_log(8).entries.into_inner());
    net.down.set(true);
    let r = run(&[CHAIN, FORK]).unwrap();
    assert_eq!((r.cursor.at(CHAIN), r.cursor.at(FORK)), (Index(40), Index(5)));
    assert_eq!(*b.disk.refs.borrow(), network_refs(&b.log, &fork_log(5)), "the root moved, the fork did not");
    // Back: the fork catches up from its cursor.
    net.down.set(false);
    let r = run(&[CHAIN, FORK]).unwrap();
    assert_eq!(r.cursor.at(FORK), Index(8));
    assert_eq!(*b.disk.refs.borrow(), network_refs(&b.log, &fork));

    // The fork's log is archived past its cursor: it joins again, alone, and the root's disk is not rebuilt.
    // As the archive does only after a promotion: 1..=11 promoted into one stored pack with its marker, complete as
    // a promoted pack is (it carries the root's object 3 its first push built on).
    fork.entries.replace(fork_log(14).entries.into_inner());
    let p = Pack { brings: std::iter::once(3).chain(501..=511).collect(), needs: vec![], pad: 0 };
    let pb = bytes_of(&p);
    let id = PackId(Ulid(3_000_011));
    b.disk.bucket.borrow_mut().insert(id, pb.clone());
    let marker = Marker { chain: FORK, index: Index(12), checksum: sum(&pb), bytes: pb.len() as u64 };
    b.disk.markers.borrow_mut().push((id, marker));
    let pack = PackRef::Stored { id, checksum: sum(&pb), bytes: pb.len() as u64, sites: SiteSet(3) };
    *fork.promoted.borrow_mut() = Some(Promoted { from: Index(1), to: Index(11), pack });
    fork.first.set(12);
    let r = run(&[CHAIN, FORK]).unwrap();
    assert!(r.rehydrated.is_none());
    assert_eq!(b.disk.inits.get(), inits);
    assert_eq!(r.cursor.at(FORK), Index(14));
    assert_eq!(*b.disk.refs.borrow(), network_refs(&b.log, &fork));
    b.disk.reset("disk-2");
    assert!(run(&[CHAIN, FORK]).unwrap().rehydrated.is_some());
    assert_eq!(*b.disk.refs.borrow(), network_refs(&b.log, &fork), "a rehydrate holds the fork's refs exactly");
}

/// A network no fork's call named keeps the cursor it saved before forks: no `fork_heads`, and the root's refs only.
#[test]
fn a_root_only_networks_cursor_and_refs_are_as_before_forks() {
    let b = bed(30, 10, 0);
    *b.log.default_branch.borrow_mut() = Some("refs/heads/b3".into());
    b.run().unwrap();
    let c = b.store.cursor.borrow().clone().unwrap().1;
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v.as_object().unwrap().keys().collect::<Vec<_>>(), ["chains", "head", "network"]);
    assert!(b.disk.ns_heads.borrow().is_empty());
    assert!(b.disk.refs.borrow().keys().all(|n| !n.starts_with("refs/namespaces/")));
    b.assert_serves_head();
}
