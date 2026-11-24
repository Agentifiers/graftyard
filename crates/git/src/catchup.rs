//! A git cache shard's catch-up behind its `IngestCursor` and its `rehydrate` (`design/DESIGN.md`, Shape, "Packs" and
//! "Git cache shards (derived)"), generic over the container and the buckets it is filled from (`ShardIo`), the shard
//! object's own storage (`Saved`), and the chain's log (`ChainLog`), so the whole procedure runs in memory in tests.
//! `host::ShardHost` supplies the Workers implementations.
//!
//! Three rules hold whatever the log or the container does, each from a hosted failure on a hot group whose log held
//! gigabytes of inline packs above its last `Promoted` (`tests/am1/results/b0-30.md`, "Blocked on", 8 to 12):
//!
//! - **Memory.** At most two log pages are held at once: the one being applied and the next, read meanwhile. A page is
//!   what one `/entries` call answers, at most `ENTRIES_PAGE_BYTES` of entries on a member, or
//!   `ENTRIES_LOCAL_PAGE_BYTES_MAX` from the shard's own site's member. Inline packs go to the container in calls of
//!   at most `INGEST_BATCH_BYTES`, each one more copy of part of the page being applied. Stored packs go from a
//!   bucket to the container without passing through the shard object's memory (`ShardIo::ingest_stored`).
//! - **The cursor moves only past what was applied.** Each page's packs are ingested and its ref changes applied
//!   before the cursor moves, and only to the last index the page held, never to the end of the range asked for. A
//!   short or empty page from a newly elected leader cannot skip an entry (blocker 12).
//! - **A run goes in pieces.** One call applies at most a `Piece` of whole entries and saves its cursor; the shard
//!   wakes whoever waits and calls again toward the same head (`Run`), so a validate or merge waiting on the disk
//!   gets a turn between pieces instead of waiting out a burst's whole catch-up. The `Run` also carries the room
//!   budget and the entries a piece read and did not apply, so a run of many pieces pays one disk walk per
//!   `ROOM_EVERY` and reads each page once, as one call to the head does.
//! - **Every page is saved before the next is applied.** Catch-up saves its cursor after each page. Rehydrate saves its
//!   tail cursor after each page and skips the stored packs the container already holds (`ShardIo::have`), so a run
//!   cut off at any point, by a caller's deadline or an isolate reset, resumes on the same disk where it stopped.
//!
//! Stored packs are fetched ahead and ingested in log order (`FETCH_AT_ONCE`, `AHEAD_PACKS`, `AHEAD_BYTES`): each is
//! spooled onto the container's disk with no lock while the packs before it ingest, so a pack's bucket read overlaps
//! the others'. A promoted pack comes with its stored `.idx`, which the container checks against the pack and loads in
//! place of `index-pack`. A rehydrate skips the pushed packs a committed `Promoted` covers (`skippable`), the read side
//! of `Compacted.consumed`, and checks the result with `ShardIo::connected` before it serves.
#![allow(async_fn_in_trait)]

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::{Pin, pin};
use std::task::Poll;

use futures_util::stream::{FuturesUnordered, StreamExt};

use graftyard_types::{ChainId, Digest, Index, NetworkId, SiteId};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::bucket::StoreError;
use crate::ingest::{ChainLog, Load, LogPacks, catch_up_plan, heads, ref_changes, rehydrate_plan, tail_start};
use crate::pack::{Marker, PackId, Promoted};
use crate::shard::IngestCursor;

/// A catch-up asks the LRU for room once per this many inline bytes, not once per page: each ask lists the disk's
/// repositories with their sizes.
pub const ROOM_EVERY: u64 = 256 << 20;

/// A rehydrate reads the entry of each marker still in the log this many at a time, to tell promotions apart.
const PROBE_AT_ONCE: usize = 16;

/// The most inline pack bytes one `ShardIo::ingest` call carries; a single larger pack goes alone. One call per pack
/// cost a hot tail 66 to 72 ms a pack, against about 10 ms of `index-pack` (`tests/am1/results/hot-shards-2.md`). One
/// local `/entries` page (`ENTRIES_LOCAL_PAGE_BYTES_MAX` on the site) holds about eight of hot-a's 500 KB packs, so a
/// call carries up to a page. Its body is one more copy of up to this many bytes, outside the wasm heap, beside the two
/// pages a catch-up holds.
pub const INGEST_BATCH_BYTES: usize = 4_000_000;

/// Stored packs being fetched into the container at once. Each `/spool` call waits for its response headers until the
/// container has the whole body, and a Workers invocation has at most six connections waiting for headers [doc,
/// `research/cloudflare.md`, Workers limits]: four spools leave one for the in-order ingest and one for the next log
/// page, so neither queues behind the fetches.
pub const FETCH_AT_ONCE: usize = 4;
/// Stored packs fetched and not yet ingested, at most: they wait on the container's disk, not in the object's memory,
/// which holds none of their bytes (`ShardIo::fetch_stored`).
pub const AHEAD_PACKS: usize = 8;
/// The bytes fetched ahead, at most, before a pack past the first `FETCH_AT_ONCE` starts. Those four start whatever
/// their size: every promoted pack of repo:131296 was 106 to 207 MB, so a cap on them all fetched one at a time, 101 to
/// 155 s of the cold warm on m35 (`tests/am1/results/lm1-memory.md`, round 23). So the disk holds four packs of any
/// size ahead, or up to `AHEAD_PACKS` within this.
pub const AHEAD_BYTES: u64 = 64 << 20;

/// The most one `catch_up` call applies: whole log entries, from the cursor on, until the next would take its packs
/// past `packs` or their bytes past `bytes`, and always at least one entry. It then saves its cursor and answers
/// `Report::behind`, and the shard starts the next piece at once, so a validate or merge waiting on the shard gets a
/// turn between pieces instead of waiting out the whole run (`decisions/DECISIONS.md`, "`catchup-pieces`").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub packs: usize,
    pub bytes: u64,
}

/// A hosted catch-up's piece: about 1.5 to 1.7 s of a burst's staged packs. B0-32 m3fst1's longest catch-up ingested
/// 294 packs, 74.5 MB, in 15.8 s, and two others 164 packs (40.8 MB) in 8.2 s and 350 (86.7 MB) in 15.9 s, so 45 to
/// 54 ms and 0.25 MB a pack, 4.7 to 5.5 MB/s [measured, `tests/fanout/raw/shardev-m3fst1-b0-32.jsonl.xz`]; 32 packs
/// or 8 MiB is 1.4 to 1.8 s at those rates [inference].
pub const PIECE: Piece = Piece { packs: 32, bytes: 8 << 20 };

/// No cap: the call runs to the head it read.
pub const WHOLE: Piece = Piece { packs: usize::MAX, bytes: u64::MAX };

/// What a catch-up run carries from one `catch_up` call, a piece, to the next (`decisions/DECISIONS.md`,
/// "`catchup-lag`"): the head its first piece read, the `make_room` budget left, so the disk walk runs once per
/// `ROOM_EVERY` of the run and not once a piece, and the entries a piece read and did not apply, so the next piece
/// starts from them instead of reading their page again. Since `catchup-pieces` each piece paid both, about 16 and 17
/// to 21 ms a pack on B0-33 erDA's home shard (`tests/am1/results/catchup-lag-design.md`). `Run::default()` before a
/// run's first piece; a call that ends the run, or goes to a rehydrate, leaves it at the default again.
#[derive(Debug, Default)]
pub struct Run {
    /// The head the run's first piece read; `None` before it. The run's later pieces read no heads and go no further.
    pub until: Option<IngestCursor>,
    room: u64,
    /// A chain's entries from just past the cursor, read and not applied; used only while they start there.
    tail: Option<(ChainId, Vec<LogPacks>)>,
}

/// The container's answer to one ingest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ingested {
    Done,
    /// 409: a base or a named object is not on the disk yet; the pack is tried again after the others.
    Later,
}

/// Where `ShardIo::fetch_stored` left a stored pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetched {
    /// On the container's disk for `ShardIo::ingest_spooled`.
    Spooled,
    /// The container already holds it.
    Have,
}

/// The promoted range a stored `.idx`'s custom metadata names: a cache of the pack's `Promoted`, never trusted alone.
pub type IdxRange = (ChainId, Index, Index);

/// The range a stored `.idx`'s custom metadata names (`store::Place::idx_range`), if it names one.
pub fn idx_range(meta: &[(String, String)]) -> Option<IdxRange> {
    let get = |k: &str| meta.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    Some((get("chain")?.parse().ok()?, Index(get("from")?.parse().ok()?), Index(get("to")?.parse().ok()?)))
}

/// The container a shard object starts, and the buckets its stored packs come from.
pub trait ShardIo {
    /// Starts the container if it is not running; the id of its disk, which a restart replaces.
    async fn up(&self) -> Result<String, StoreError>;
    /// Creates the network's repository, deleting any first when `fresh`.
    async fn init(&self, fresh: bool) -> Result<(), StoreError>;
    /// Of these pack checksums, the ones the repository already ingested.
    async fn have(&self, sums: &[Digest]) -> Result<BTreeSet<Digest>, StoreError>;
    /// Inline packs, each with the SHA-256 of its bytes, ingested in order in one call: one answer per pack.
    async fn ingest(&self, packs: &[(Digest, &[u8])]) -> Result<Vec<Ingested>, StoreError>;
    /// A stored pack, from the first bucket whose copy matches `checksum`, streamed onto the container's disk without
    /// its lock, and with `idx` its stored `.idx` beside it. Several run at once.
    async fn fetch_stored(&self, id: PackId, checksum: Digest, idx: bool) -> Result<Fetched, StoreError>;
    /// Ingests a pack `fetch_stored` spooled: through its stored `.idx` when that checks against the pack, otherwise
    /// with `index-pack`. `Later` keeps it spooled for the next try.
    async fn ingest_spooled(&self, checksum: Digest) -> Result<Ingested, StoreError>;
    /// Ingests packs `fetch_stored` spooled, in order, in one call: one `index-pack` for the run, split on failure,
    /// a pack a validate kept aside moved in first (`catchup-lag`). One answer per pack; `Later` keeps it spooled.
    /// The default is one `ingest_spooled` a pack.
    async fn ingest_spooled_batch(&self, checksums: &[Digest]) -> Result<Vec<Ingested>, StoreError> {
        let mut out = Vec::with_capacity(checksums.len());
        for s in checksums {
            out.push(self.ingest_spooled(*s).await?);
        }
        Ok(out)
    }
    /// Every stored `.idx` of the network's packs in the buckets, with the range its custom metadata names, if any.
    async fn idx_ranges(&self) -> Result<BTreeMap<PackId, Option<IdxRange>>, StoreError>;
    /// Whether the history of every ref, and the whole tree at each ref's tip, is on the disk: `Ok(Err(why))` when not.
    /// The check waits for a running repack, so with `repack` the container starts the repack only after it.
    async fn connected(&self, repack: bool) -> Result<Result<(), String>, StoreError>;
    /// A page of the whole ref set; the last page (`done`) also starts the container's repack when `repack`, and points
    /// HEAD at `head`, the repository's default branch, when given (else the container's old rule).
    async fn put_refs_page(
        &self,
        refs: &[(String, String)],
        start: bool,
        done: bool,
        repack: bool,
        head: Option<&str>,
    ) -> Result<(), StoreError>;
    /// Ref changes, and HEAD pointed at `head` when given: with no changes, only HEAD. With `repack` the container
    /// starts its repack after the changes, as a last ref page does; a rehydrate whose backstop check follows sends
    /// false, so the check's walk runs before the repack rather than waiting it out.
    async fn apply_refs(
        &self,
        changes: &[(String, Option<String>)],
        head: Option<&str>,
        repack: bool,
    ) -> Result<(), StoreError>;
    /// Evicts other networks so `incoming` more bytes fit; the networks evicted.
    async fn make_room(&self, incoming: u64) -> Result<Vec<i64>, StoreError>;
    /// The network's `Committed` markers from every bucket of the placement.
    async fn markers(&self) -> Result<Vec<(PackId, Marker)>, StoreError>;
}

/// The order a shard on site `me` reads a stored pack's buckets in: its own site's first, then the others in site
/// order. Each bucket holds a copy of every pack its site placed, so a pack is usually found in the first one read; in
/// site order every shard read site 0's bucket first, and the apac shard of B0-31 m3fr2 pulled every 1 MiB pack from
/// enam, a cross-region read per pack, and fell minutes behind (`tests/am1/results/lm1-rows-m3f.md`, m3fr2, B0-31).
/// Listings that need every bucket (`ShardIo::markers`) read them all, in any order.
pub fn own_first<T>(me: SiteId, buckets: impl IntoIterator<Item = (SiteId, T)>) -> Vec<(SiteId, T)> {
    let mut v: Vec<(SiteId, T)> = buckets.into_iter().collect();
    v.sort_by_key(|(s, _)| (*s != me, s.0));
    v
}

/// The first bucket, in the order given, whose copy `take` accepts, and what it answered: `take` answers `None` for a
/// bucket that lacks the object or holds a copy that does not match its checksum, and the next bucket is tried.
/// `ShardIo::ingest_stored` reads a stored pack this way, over `own_first`'s order.
pub async fn first_copy<T, R, F, Fut>(buckets: &[(SiteId, T)], mut take: F) -> Result<Option<(SiteId, R)>, StoreError>
where
    F: FnMut(&T) -> Fut,
    Fut: std::future::Future<Output = Result<Option<R>, StoreError>>,
{
    for (site, b) in buckets {
        if let Some(r) = take(b).await? {
            return Ok(Some((*site, r)));
        }
    }
    Ok(None)
}

/// Where a catch-up's stored packs came from and where their time went, per pack summed: the whole ingest, the
/// container's spool (the bucket's body streamed onto its disk, so the read from the bucket) and its `index-pack`
/// (unpack, delta resolution, and the index). The shard host fills it; `catch_up` leaves it empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct StoredMs {
    pub packs: u32,
    pub bytes: u64,
    pub total_ms: u64,
    pub spool_ms: u64,
    pub index_ms: u64,
    /// Packs served by each site's bucket.
    pub from_site: BTreeMap<u8, u32>,
    /// Packs a validate had kept aside, moved in with no `index-pack` (`adopt` in the container).
    pub adopted: u32,
    /// Calls that ingested more than one stored pack (`ShardIo::ingest_spooled_batch`).
    pub batches: u32,
    /// Packs loaded through their stored `.idx`, and stored `.idx` the container refused (then `index-pack` ran).
    pub idx_used: u32,
    pub idx_rejected: u32,
    /// The writer's section's hold time over the stored packs' ingests (the container's `writeLock`).
    pub lock_ms: u64,
    /// The container's background multi-pack-index writes (one every `midxEvery` ingests, outside the writer's section
    /// since `midx-background`) that ended during the catch-up's ingests, stored and inline.
    pub midx_ms: u64,
    /// The waits for the network's locks, index-pack's shared one and the writer's section, over every ingest of the
    /// catch-up, stored and inline.
    pub lock_wait_ms: u64,
    /// The waits for the writer's section, the part of `lock_wait_ms` behind other writers and the deleters; since
    /// `lock-split` no longer behind validates and readers (decisions/DECISIONS.md).
    pub lock_excl_wait_ms: u64,
    /// The waits for the writer's section over the catch-up's `/refs` calls, and for the lock alone on a set that
    /// renames a ref between file and directory.
    pub refs_lock_wait_ms: u64,
    /// The backstop checks' wait for a running repack before their walks (`/connected`'s `repack_wait_ms`): with the
    /// repack left to the check, about 0 (`tests/am1/results/b034-mt24.md`, "The home's rehydrate").
    pub repack_wait_ms: u64,
    /// Each stored pack's spool as `[start, end, bytes, site]`, start and end in the shard object's ms since the
    /// epoch, at most `SPOOLS_MAX`: the streams busy at once and the total bucket rate over time, which say whether
    /// a container's R2 reads stop at a per-container ceiling (`tests/am1/results/lm1-memory.md`, round 27b).
    pub spools: Vec<[u64; 4]>,
}

/// The most spools `StoredMs::spools` keeps: a cold rehydrate of repo:131296 spooled 82.
pub const SPOOLS_MAX: usize = 512;

/// A rehydrate in progress on one disk: whether its marked phase finished, how far its tail got, and per chain the
/// tail start its marked phase loaded through (every promotion up to there is on the disk).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rehydrating {
    pub marked: bool,
    pub tail: IngestCursor,
    /// Missing in a place an older release saved: read as nothing loaded, so a resumed run loads the marked phase
    /// again, which `have` makes cheap.
    #[serde(default)]
    pub loaded: Vec<(ChainId, Index)>,
    /// Pushed packs the marked phase skipped because a promotion covers them (`skippable`), for the check before the
    /// rehydrate serves.
    #[serde(default)]
    pub skipped: Vec<PackId>,
}

/// The shard object's own storage for one network, each value with the id of the disk it describes.
pub trait Saved {
    async fn cursor(&self) -> Result<Option<(String, IngestCursor)>, StoreError>;
    async fn put_cursor(&self, disk: &str, c: &IngestCursor) -> Result<(), StoreError>;
    async fn drop_cursor(&self) -> Result<(), StoreError>;
    async fn rehydrating(&self) -> Result<Option<(String, Rehydrating)>, StoreError>;
    async fn put_rehydrating(&self, disk: &str, r: &Rehydrating) -> Result<(), StoreError>;
    async fn drop_rehydrating(&self) -> Result<(), StoreError>;
}

/// What a rehydrate did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Rehydrated {
    pub markers: usize,
    /// Stored packs ingested.
    pub stored: usize,
    /// Inline packs ingested from the tail, and their bytes.
    pub inline: usize,
    pub bytes: usize,
    /// Tail entries read.
    pub entries: usize,
    /// Refs set, and the pages they came in.
    pub refs: usize,
    pub pages: usize,
    /// Continued a run cut off on this disk, rather than starting from an empty repository.
    pub resumed: bool,
    /// Calls to the container that ingested packs (`INGEST_BATCH_BYTES`).
    pub calls: usize,
    /// Pushed packs a promotion covered, not loaded.
    pub skipped: usize,
    /// Why the check after a skip failed, before the skipped packs were loaded and it passed: a fault to report.
    pub fault: Option<String>,
    /// Counted rolls whose produced pack did not load, each with why: their consumed packs loaded instead.
    pub dropped_rolls: Vec<String>,
}

/// What a catch-up did.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub rehydrated: Option<Rehydrated>,
    pub entries: usize,
    pub packs: usize,
    /// Ref changes applied.
    pub refs: usize,
    pub cursor: IngestCursor,
    /// Networks the LRU evicted for this catch-up's packs.
    pub evicted: Vec<i64>,
    /// The most inline pack bytes of log pages held at once.
    pub held_max: usize,
    pub disk: String,
    /// Each step's end, in ms from the start.
    pub ms: Vec<(&'static str, u64)>,
    /// The stored packs' fetch and index time (`StoredMs`).
    pub stored: StoredMs,
    /// Where the log pages' time went (`PageMs`).
    pub pages_ms: PageMs,
    /// The call stopped at its `Piece` before `until`: the cursor is saved there, and the next call goes on.
    pub behind: bool,
    /// The index each chain is caught up to by this run: the head its first piece read (`Run::until`).
    pub until: IngestCursor,
    /// `ShardIo::make_room` calls this call made.
    pub room_calls: usize,
}

/// A catch-up's or a rehydrate tail's time per part of applying its log pages, summed over the pages: `make_room`
/// (the LRU's disk walk and any evict), the ingests (stored packs' fetches and every ingest call), the ref changes'
/// `/refs` calls, and the cursor or place saves. B0-34 mt7's monorepo catch-ups ran 5 to 17.6 s, most of it outside
/// every timed lap (`tests/am1/results/b034-mt7.md`, "1. Catch-up wait is now the tail").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PageMs {
    pub room: u64,
    pub ingest: u64,
    pub refs: u64,
    pub save: u64,
}

impl PageMs {
    fn add(&mut self, o: &PageMs) {
        self.room += o.room;
        self.ingest += o.ingest;
        self.refs += o.refs;
        self.save += o.save;
    }
}

/// The last index of `page`, a run of entries that must start at `from`, be contiguous, and end at or before `to`.
/// An empty page is an error: the member that answered has not committed `from` yet, so the caller tries again.
pub fn page_end(chain: ChainId, from: Index, to: Index, page: &[LogPacks]) -> Result<Index, StoreError> {
    let ok = !page.is_empty() && page.iter().map(|e| e.index.0).eq(from.0..from.0 + page.len() as u64);
    match page.last() {
        Some(l) if ok && l.index <= to => Ok(l.index),
        _ => {
            let got: Vec<u64> = page.iter().map(|e| e.index.0).take(8).collect();
            Err(StoreError(format!("{chain}: log entries from {} through {} answered {got:?}", from.0, to.0)))
        }
    }
}

/// An entry's packs and their bytes, inline or stored, as a `Piece` counts them.
fn weight(e: &LogPacks) -> (usize, u64) {
    let bytes = e
        .packs
        .iter()
        .map(|p| match p {
            crate::pack::PackRef::Inline(b) => b.len() as u64,
            crate::pack::PackRef::Stored { bytes, .. } => *bytes,
        })
        .sum();
    (e.packs.len(), bytes)
}

/// How many of `page`'s entries fit in what is left of `cap` after `took`, the packs and bytes this call applied so
/// far: whole entries, and the first entry of a call that took nothing whatever its size.
fn fits(page: &[LogPacks], took: (usize, u64), cap: Piece) -> usize {
    let (mut n, mut b) = took;
    let mut k = 0;
    for e in page {
        let (pn, pb) = weight(e);
        if (n > 0 || b > 0 || k > 0) && (n.saturating_add(pn) > cap.packs || b.saturating_add(pb) > cap.bytes) {
            break;
        }
        n = n.saturating_add(pn);
        b = b.saturating_add(pb);
        k += 1;
    }
    k
}

fn inline_bytes(page: &[LogPacks]) -> usize {
    page.iter()
        .flat_map(|e| &e.packs)
        .map(|p| match p {
            crate::pack::PackRef::Inline(b) => b.len(),
            crate::pack::PackRef::Stored { .. } => 0,
        })
        .sum()
}

/// One fetch of `Ahead`'s list, answering its place in the list.
type Fetch<'a> = Pin<Box<dyn Future<Output = (usize, Result<Fetched, StoreError>)> + 'a>>;

/// Stored packs fetched ahead of the ingest, in order, within `FETCH_AT_ONCE`, `AHEAD_PACKS`, and `AHEAD_BYTES`.
struct Ahead<'a> {
    list: Vec<(Digest, PackId, u64, bool)>,
    start: Box<dyn Fn(usize) -> Fetch<'a> + 'a>,
    running: FuturesUnordered<Fetch<'a>>,
    done: BTreeMap<usize, Result<Fetched, StoreError>>,
    started: usize,
    taken: usize,
    held: u64,
}

impl<'a> Ahead<'a> {
    fn new<I: ShardIo>(io: &'a I, list: Vec<(Digest, PackId, u64, bool)>) -> Ahead<'a> {
        let l = list.clone();
        Ahead {
            list,
            start: Box::new(move |k| {
                let (sum, id, _, idx) = l[k];
                Box::pin(async move { (k, io.fetch_stored(id, sum, idx).await) })
            }),
            running: FuturesUnordered::new(),
            done: BTreeMap::new(),
            started: 0,
            taken: 0,
            held: 0,
        }
    }

    fn pump(&mut self, cx: &mut std::task::Context<'_>) {
        loop {
            while self.started < self.list.len()
                && self.running.len() < FETCH_AT_ONCE
                && self.started - self.taken < AHEAD_PACKS
                && (self.started - self.taken < FETCH_AT_ONCE || self.held + self.list[self.started].2 <= AHEAD_BYTES)
            {
                self.held += self.list[self.started].2;
                self.running.push((self.start)(self.started));
                self.started += 1;
            }
            match self.running.poll_next_unpin(cx) {
                Poll::Ready(Some((k, r))) => {
                    self.done.insert(k, r);
                }
                _ => return,
            }
        }
    }

    /// The next stored pack's fetch if it has already ended, without waiting for it.
    async fn take_ready(&mut self) -> Option<Result<Fetched, StoreError>> {
        let k = self.taken;
        let r = std::future::poll_fn(|cx| {
            self.pump(cx);
            Poll::Ready(self.done.remove(&k))
        })
        .await?;
        self.taken += 1;
        self.held -= self.list[k].2;
        Some(r)
    }

    /// The next stored pack's fetch, once it ends, with the later ones going on meanwhile.
    async fn take(&mut self) -> Result<Fetched, StoreError> {
        let k = self.taken;
        let r = std::future::poll_fn(|cx| {
            self.pump(cx);
            match self.done.remove(&k) {
                Some(r) => Poll::Ready(r),
                None => Poll::Pending,
            }
        })
        .await;
        self.taken += 1;
        self.held -= self.list[k].2;
        r
    }

    /// `op`, with the fetches going on beside it.
    async fn beside<T>(&mut self, op: impl Future<Output = T>) -> T {
        let mut op = pin!(op);
        std::future::poll_fn(|cx| {
            self.pump(cx);
            op.as_mut().poll(cx)
        })
        .await
    }
}

/// Ingests `loads` in order; a pack answered `Later` is tried again after the others, until a pass makes no progress.
/// Runs of inline packs go in one call each, up to `INGEST_BATCH_BYTES`. Stored packs are fetched ahead (`Ahead`) and
/// ingested in their place in the order: a stored pack whose fetch ended goes in one call with the stored packs right
/// after it whose fetches also ended, up to `INGEST_BATCH_BYTES` (a larger one alone), and with no wait for a fetch
/// still running (`catchup-lag`). Those in `with_idx` come with their stored `.idx` and go alone. `Ok(n)` with the
/// packs ingested, or `Err(Unresolved)` naming how many never resolved. `calls` counts the calls.
async fn ingest_all<I: ShardIo>(
    io: &I,
    loads: &[Load],
    with_idx: &BTreeSet<PackId>,
    calls: &mut usize,
) -> Result<Result<usize, usize>, StoreError> {
    // The packs a call cut off part way through this list already ingested are not sent again: a page's inline packs
    // are up to `ENTRIES_PAGE_BYTES`, and a caller whose deadline falls inside every page would otherwise never pass it.
    let sums: Vec<Digest> = loads.iter().map(load_sum).collect();
    let held = if loads.len() > 1 { io.have(&sums).await? } else { BTreeSet::new() };
    let mut left: Vec<(Digest, &Load)> = sums.into_iter().zip(loads).filter(|(s, _)| !held.contains(s)).collect();
    let mut done = held.len();
    // A pack answered `Later` stays spooled on the container, so the next pass ingests it without a second fetch.
    let mut spooled: BTreeSet<Digest> = BTreeSet::new();
    while !left.is_empty() {
        let before = left.len();
        let mut retry = Vec::new();
        let ahead_list: Vec<(Digest, PackId, u64, bool)> = left
            .iter()
            .filter_map(|(s, l)| match l {
                Load::Stored { id, bytes, .. } if !spooled.contains(s) => {
                    Some((*s, *id, *bytes, with_idx.contains(id)))
                }
                _ => None,
            })
            .collect();
        let mut ahead = Ahead::new(io, ahead_list);
        // Fetches a batch took from `ahead` and did not ingest: the container already held the pack.
        let mut had: BTreeSet<Digest> = BTreeSet::new();
        let mut i = 0;
        while i < left.len() {
            let (s, l) = left[i];
            let (run, answers) = match l {
                Load::Stored { id, bytes: first, .. } => {
                    let fetched = if spooled.contains(&s) {
                        Fetched::Spooled
                    } else if had.remove(&s) {
                        Fetched::Have
                    } else {
                        ahead.take().await?
                    };
                    match fetched {
                        Fetched::Have => (1, vec![Ingested::Done]),
                        Fetched::Spooled if with_idx.contains(id) => {
                            *calls += 1;
                            (1, vec![ahead.beside(io.ingest_spooled(s)).await?])
                        }
                        Fetched::Spooled => {
                            let mut batch = vec![s];
                            let mut bytes = *first;
                            for (s, l) in &left[i + 1..] {
                                let Load::Stored { id, bytes: b, .. } = l else { break };
                                if with_idx.contains(id) || bytes + b > INGEST_BATCH_BYTES as u64 {
                                    break;
                                }
                                if !spooled.contains(s) {
                                    match ahead.take_ready().await {
                                        None => break,
                                        Some(r) => {
                                            if r? == Fetched::Have {
                                                had.insert(*s);
                                                break;
                                            }
                                        }
                                    }
                                }
                                bytes += b;
                                batch.push(*s);
                            }
                            *calls += 1;
                            let a = if batch.len() == 1 {
                                vec![ahead.beside(io.ingest_spooled(s)).await?]
                            } else {
                                ahead.beside(io.ingest_spooled_batch(&batch)).await?
                            };
                            if a.len() != batch.len() {
                                return Err(StoreError(format!("{} answers for {} packs", a.len(), batch.len())));
                            }
                            (batch.len(), a)
                        }
                    }
                }
                Load::Inline { .. } => {
                    let mut batch: Vec<(Digest, &[u8])> = Vec::new();
                    let mut bytes = 0;
                    for (s, l) in &left[i..] {
                        let Load::Inline { bytes: b, .. } = l else { break };
                        if !batch.is_empty() && bytes + b.len() > INGEST_BATCH_BYTES {
                            break;
                        }
                        bytes += b.len();
                        batch.push((*s, b.as_slice()));
                    }
                    let a = ahead.beside(io.ingest(&batch)).await?;
                    if a.len() != batch.len() {
                        return Err(StoreError(format!("{} answers for {} packs", a.len(), batch.len())));
                    }
                    *calls += 1;
                    (batch.len(), a)
                }
            };
            for (k, a) in answers.into_iter().enumerate() {
                match a {
                    Ingested::Done => done += 1,
                    Ingested::Later => {
                        if matches!(left[i + k].1, Load::Stored { .. }) {
                            spooled.insert(left[i + k].0);
                        }
                        retry.push(left[i + k]);
                    }
                }
            }
            i += run;
        }
        if retry.len() == before {
            return Ok(Err(retry.len()));
        }
        left = retry;
    }
    Ok(Ok(done))
}

/// The pushed packs among `marks` that a rehydrate's marked phase need not load, because a committed promotion covers
/// them: a pack is skipped only when its entry lies in a `Promoted` record read from the log (`records`, by promoted
/// pack) and in the range that pack's stored `.idx` names (`idx`), and that promoted pack is itself loaded. The idx
/// metadata alone is never enough: an object's custom metadata is not committed state, and a range wider than the
/// record would skip packs no promotion holds. This is the read side of `Compacted.consumed`: a roll's produced pack
/// covers its consumed range as a `Promoted` record, so `rehydrate` loads it in place of everything in the range.
pub fn skippable(
    marks: &[(PackId, Marker)],
    records: &BTreeMap<PackId, (ChainId, Promoted)>,
    idx: &BTreeMap<PackId, Option<IdxRange>>,
) -> BTreeSet<PackId> {
    let loaded: BTreeSet<PackId> = marks.iter().map(|(id, _)| *id).collect();
    let covers: Vec<(ChainId, Index, Index)> = records
        .iter()
        .filter(|(q, _)| loaded.contains(q))
        .filter_map(|(q, (c, p))| {
            let (ic, f, t) = (*idx.get(q)?)?;
            let (from, to) = (p.from.max(f), p.to.min(t));
            (ic == *c && from <= to).then_some((*c, from, to))
        })
        .collect();
    marks
        .iter()
        .filter(|(id, m)| {
            !records.contains_key(id) && covers.iter().any(|(c, f, t)| *c == m.chain && m.index >= *f && m.index <= *t)
        })
        .map(|(id, _)| *id)
        .collect()
}

fn load_sum(l: &Load) -> Digest {
    match l {
        Load::Inline { bytes, .. } => Digest(Sha256::digest(bytes).into()),
        Load::Stored { checksum, .. } => *checksum,
    }
}

fn unresolved(chain: ChainId, n: usize, from: Index, to: Index) -> StoreError {
    StoreError(format!("{chain}: {n} packs of entries {}..={} have bases no pack provides", from.0, to.0))
}

/// The work a page's entries ask for: a catch-up applies their packs and ref changes and saves its cursor; a
/// rehydrate's tail ingests their packs not yet loaded and saves its tail cursor.
enum Mode<'a> {
    /// `repack`: each page's ref changes may start the container's repack (`ShardIo::apply_refs`).
    CatchUp {
        cursor: &'a mut IngestCursor,
        seen: &'a mut BTreeSet<PackId>,
        repack: bool,
    },
    Tail {
        state: &'a mut Rehydrating,
        seen: &'a mut BTreeSet<PackId>,
        consumed: &'a BTreeSet<PackId>,
    },
}

#[derive(Default)]
struct Tally {
    entries: usize,
    packs: usize,
    inline: usize,
    bytes: usize,
    refs: usize,
    held_max: usize,
    calls: usize,
    room: u64,
    room_calls: usize,
    evicted: Vec<i64>,
    ms: PageMs,
    /// The packs and bytes this call applied, against its `Piece`.
    took: (usize, u64),
}

/// Why a range stopped early.
enum Stop {
    Err(StoreError),
    /// An entry carries no ref changes (an older member release): only a rehydrate can catch up past it.
    NoRefs,
    /// Packs whose bases the disk lacks: the disk is missing an entry below the cursor.
    Unresolved(StoreError),
}

impl From<StoreError> for Stop {
    fn from(e: StoreError) -> Self {
        Stop::Err(e)
    }
}

/// Applies `chain`'s entries `from..=to` a page at a time, reading the next page while it applies one, until `cap`:
/// `Ok(true)` when it stopped there, at an entry boundary with the cursor saved, before `to`. `carry` holds entries
/// read and not applied: on the way in, a previous piece's, used as the first page only when they start at `from`;
/// on the way out, this call's, the rest of the page it stopped in (`Run`).
#[allow(clippy::too_many_arguments)]
async fn follow<I: ShardIo, S: Saved, L: ChainLog>(
    io: &I,
    saved: &S,
    log: &L,
    disk: &str,
    chain: ChainId,
    mut from: Index,
    to: Index,
    cap: Piece,
    mode: &mut Mode<'_>,
    t: &mut Tally,
    carry: &mut Option<Vec<LogPacks>>,
    now: &dyn Fn() -> u64,
) -> Result<bool, Stop> {
    let mut ahead: Option<Result<Vec<LogPacks>, StoreError>> =
        carry.take().filter(|c| c.first().is_some_and(|e| e.index == from)).map(Ok);
    while from <= to {
        // A later chain of a call whose piece an earlier chain filled.
        if t.took != (0, 0) && (t.took.0 >= cap.packs || t.took.1 >= cap.bytes) {
            *carry = ahead.and_then(Result::ok);
            return Ok(true);
        }
        let mut page = match ahead.take() {
            Some(p) => p?,
            None => log.entries(chain, from, to).await?,
        };
        page_end(chain, from, to, &page)?;
        let k = fits(&page, t.took, cap);
        if k == 0 {
            *carry = Some(page);
            return Ok(true);
        }
        let capped = k < page.len();
        let rest = page.split_off(k);
        let last = page_end(chain, from, to, &page)?;
        for e in &page {
            let (n, b) = weight(e);
            t.took = (t.took.0.saturating_add(n), t.took.1.saturating_add(b));
        }
        let full = capped || t.took.0 >= cap.packs || t.took.1 >= cap.bytes;
        let next = Index(last.0 + 1);
        let held = inline_bytes(&page) + inline_bytes(&rest);
        let mut most = held;
        let apply = apply_page(io, saved, disk, chain, from, last, page, mode, t, now);
        // The next page is read beside this one only when this piece goes on to it.
        if next <= to && !full {
            let (r, p) = futures_util::future::join(apply, log.entries(chain, next, to)).await;
            if let Ok(p) = &p {
                most = held + inline_bytes(p);
            }
            r?;
            ahead = Some(p);
        } else {
            apply.await?;
        }
        t.held_max = t.held_max.max(most);
        from = next;
        if full && from <= to {
            if capped {
                *carry = Some(rest);
            }
            return Ok(true);
        }
    }
    Ok(false)
}

/// `&head=<ref>`, percent-encoded, for `/refs` (G26): a branch name may hold `&`, `+`, `#` or `%`.
pub fn head_param(head: Option<&str>) -> String {
    let Some(h) = head else { return String::new() };
    let mut out = String::from("&head=");
    for b in h.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
async fn apply_page<I: ShardIo, S: Saved>(
    io: &I,
    saved: &S,
    disk: &str,
    chain: ChainId,
    from: Index,
    last: Index,
    page: Vec<LogPacks>,
    mode: &mut Mode<'_>,
    t: &mut Tally,
    now: &dyn Fn() -> u64,
) -> Result<(), Stop> {
    t.entries += page.len();
    let bytes = inline_bytes(&page) as u64;
    let mut at = now();
    let mut lap = |part: &mut u64| {
        let n = now();
        *part += n.saturating_sub(at);
        at = n;
    };
    if t.room == 0 || t.room < bytes {
        t.evicted.extend(io.make_room(ROOM_EVERY.max(bytes)).await?);
        t.room = ROOM_EVERY.max(bytes);
        t.room_calls += 1;
    }
    t.room -= bytes;
    lap(&mut t.ms.room);
    match mode {
        Mode::CatchUp { cursor, seen, repack } => {
            let changes = ref_changes(&page, cursor.at(chain), last).ok_or(Stop::NoRefs)?;
            // A `Promoted` entry follows its range, and a catch-up applies entries in order, so the disk already holds
            // every object of a promoted pack it reaches: fetching it again cost each shard the whole range's bytes.
            let promoted: BTreeSet<PackId> =
                page.iter().filter_map(|e| e.promoted.as_ref()?.pack.stored_id()).collect();
            let plan: Vec<Load> = catch_up_plan(cursor, chain, &page, last)
                .into_iter()
                .filter(|l| l.stored_id().is_none_or(|id| !promoted.contains(&id) && seen.insert(id)))
                .collect();
            drop(page);
            let ingested = ingest_all(io, &plan, &BTreeSet::new(), &mut t.calls).await?;
            lap(&mut t.ms.ingest);
            match ingested {
                Ok(n) => t.packs += n,
                Err(n) => return Err(Stop::Unresolved(unresolved(chain, n, from, last))),
            }
            t.refs += changes.len();
            io.apply_refs(&changes, None, *repack).await?;
            lap(&mut t.ms.refs);
            cursor.advance(chain, last);
            saved.put_cursor(disk, cursor).await?;
            lap(&mut t.ms.save);
        }
        Mode::Tail { state, seen, consumed } => {
            let promoted: BTreeSet<PackId> =
                page.iter().filter_map(|e| e.promoted.as_ref()?.pack.stored_id()).collect();
            let plan: Vec<Load> = rehydrate_plan(&[], consumed, &[], &page)
                .into_iter()
                .filter(|l| l.stored_id().is_none_or(|id| seen.insert(id)))
                .collect();
            drop(page);
            for l in &plan {
                match l {
                    Load::Inline { bytes, .. } => {
                        t.inline += 1;
                        t.bytes += bytes.len();
                    }
                    Load::Stored { .. } => t.packs += 1,
                }
            }
            let ingested = ingest_all(io, &plan, &promoted, &mut t.calls).await?;
            lap(&mut t.ms.ingest);
            if let Err(n) = ingested {
                return Err(Stop::Unresolved(unresolved(chain, n, from, last)));
            }
            state.tail.advance(chain, last);
            saved.put_rehydrating(disk, state).await?;
            lap(&mut t.ms.save);
        }
    }
    Ok(())
}

/// Catch-up before serving: for each chain whose leader's applied index is past the cursor, its entries a page at a
/// time, at most `piece` of them (`Report::behind` when it stopped there). A run's first piece reads the heads into
/// `run`; its later ones pass the same `run` and go no further, so the run ends at the head it started from however
/// fast the log moves, which is what a caller waiting for a run that started after it needs. `run` also carries the
/// room budget and the unapplied entries from piece to piece (`Run`). No cursor for the
/// container's disk, a cursor below a chain's first log index, or an entry from a member release that sends no ref
/// changes, means a rehydrate. Packs whose bases the disk lacks mean the disk skipped an
/// entry below its cursor: the cursor is dropped, so the next call rehydrates instead of failing on the same pack.
#[allow(clippy::too_many_arguments)]
pub async fn catch_up<I: ShardIo, S: Saved, L: ChainLog>(
    io: &I,
    saved: &S,
    log: &L,
    network: NetworkId,
    chains: &[ChainId],
    piece: Piece,
    run: &mut Run,
    now: &dyn Fn() -> u64,
) -> Result<Report, StoreError> {
    // Whatever this call ends in, only a call that stops at its piece hands the run on.
    let carried = std::mem::take(run);
    let t0 = now();
    let mut ms = Vec::new();
    let lap = |ms: &mut Vec<(&'static str, u64)>, name: &'static str| ms.push((name, now().saturating_sub(t0)));
    let disk = io.up().await?;
    lap(&mut ms, "up");
    let mut cursor = saved.cursor().await?.filter(|(d, _)| *d == disk).map(|(_, c)| c);
    lap(&mut ms, "cursor");
    // A run's later pieces read no heads: `until` names where the run ends and HEAD's branch. One heads read is a
    // member round trip, 85 to 122 ms on m3fst1, about a tenth of a piece.
    let mut ends: Vec<(Index, Option<String>)> = Vec::with_capacity(chains.len());
    match carried.until.as_ref().filter(|_| cursor.is_some()) {
        Some(u) => ends.extend(chains.iter().enumerate().map(|(i, c)| (u.at(*c), u.head.clone().filter(|_| i == 0)))),
        None => {
            for (chain, head) in chains.iter().zip(heads(log, chains).await) {
                let head = head?;
                if cursor.as_ref().is_some_and(|c| c.at(*chain).0 + 1 < head.first.0) {
                    cursor = None;
                }
                ends.push((head.applied, head.default_branch));
            }
            lap(&mut ms, "heads");
        }
    }
    let Some(mut cursor) = cursor else {
        return rehydrate(io, saved, log, network, chains, disk, ms, t0, now).await;
    };
    // A rehydrate cut off in its last catch-up left its place: this catch-up finishes that one. A place for another
    // disk is a container that is gone, and goes too.
    let finishing = saved.rehydrating().await?.is_some();
    let mut t = Tally { room: carried.room, ..Tally::default() };
    let mut carry = carried.tail;
    let mut seen = BTreeSet::new();
    // The network's own chain, first, names its default branch (G26).
    let head_ref = ends.first().and_then(|(_, b)| b.clone());
    let mut behind = false;
    let mut to = IngestCursor::new(network);
    to.head = head_ref.clone();
    for (chain, (end, _)) in chains.iter().zip(ends) {
        to.advance(*chain, end);
        let Some(range) = cursor.behind(*chain, end) else { continue };
        if behind {
            continue;
        }
        let mut mode = Mode::CatchUp { cursor: &mut cursor, seen: &mut seen, repack: true };
        let mut tail = carry.take().filter(|(c, _)| c == chain).map(|(_, es)| es);
        match follow(
            io,
            saved,
            log,
            &disk,
            *chain,
            *range.start(),
            *range.end(),
            piece,
            &mut mode,
            &mut t,
            &mut tail,
            now,
        )
        .await
        {
            Ok(capped) => {
                behind = capped;
                carry = tail.map(|es| (*chain, es));
            }
            Err(Stop::Err(e)) => return Err(e),
            // Either way the next rehydrate starts from nothing, so a place left by an earlier one goes too.
            Err(Stop::NoRefs) => {
                saved.drop_cursor().await?;
                saved.drop_rehydrating().await?;
                return rehydrate(io, saved, log, network, chains, disk, ms, t0, now).await;
            }
            Err(Stop::Unresolved(e)) => {
                saved.drop_cursor().await?;
                saved.drop_rehydrating().await?;
                return Err(StoreError(format!("{}; the cursor is dropped, so the next call rehydrates", e.0)));
            }
        }
        lap(&mut ms, "ingest");
    }
    // HEAD follows a `Settings` entry that changed the default branch, which carries no ref change; one call, only
    // when it changed since the disk's HEAD was last set, and only once the run's last piece applied its refs.
    if !behind && head_ref.is_some() && cursor.head != head_ref {
        let at = now();
        io.apply_refs(&[], head_ref.as_deref(), true).await?;
        t.ms.refs += now().saturating_sub(at);
        cursor.head = head_ref;
        saved.put_cursor(&disk, &cursor).await?;
    }
    // A rehydrate's place goes only once the disk reached the head (`rehydrate`, its last catch-up).
    if finishing && !behind {
        saved.drop_rehydrating().await?;
    }
    if behind {
        *run = Run { until: Some(to.clone()), room: t.room, tail: carry };
    }
    Ok(Report {
        room_calls: t.room_calls,
        behind,
        until: to,
        rehydrated: None,
        entries: t.entries,
        packs: t.packs,
        refs: t.refs,
        cursor,
        evicted: t.evicted,
        held_max: t.held_max,
        disk,
        ms,
        stored: StoredMs::default(),
        pages_ms: t.ms,
    })
}

/// `rehydrate`: every stored pack with a marker, minus consumed packs, then the packs each member chain names after its
/// last `Promoted`, read from the log a page at a time, then the refs a page at a time, then a catch-up from the first
/// ref page's index, then, when promotions covered packs, the backstop's walk (the `walk` step), which starts the
/// repack. A run cut off on the same disk resumes: no new repository, no stored pack the container holds
/// fetched again, and the tail from after its last saved page.
#[allow(clippy::too_many_arguments)]
async fn rehydrate<I: ShardIo, S: Saved, L: ChainLog>(
    io: &I,
    saved: &S,
    log: &L,
    network: NetworkId,
    chains: &[ChainId],
    disk: String,
    mut ms: Vec<(&'static str, u64)>,
    t0: u64,
    now: &dyn Fn() -> u64,
) -> Result<Report, StoreError> {
    let lap = |ms: &mut Vec<(&'static str, u64)>, name: &'static str| ms.push((name, now().saturating_sub(t0)));
    let mut r = Rehydrated::default();
    let resumed = saved.rehydrating().await?.filter(|(d, _)| *d == disk).map(|(_, s)| s);
    r.resumed = resumed.is_some();
    let mut state = match resumed {
        Some(s) => s,
        None => {
            io.init(true).await?;
            let s = Rehydrating {
                marked: false,
                tail: IngestCursor::new(network),
                loaded: Vec::new(),
                skipped: Vec::new(),
            };
            saved.put_rehydrating(&disk, &s).await?;
            s
        }
    };
    let mut marks: BTreeMap<PackId, Marker> = BTreeMap::new();
    for (id, m) in io.markers().await? {
        marks.entry(id).or_insert(m);
    }
    r.markers = marks.len();
    // Each chain's rolls, oldest first (`ChainLog::compactions`). A roll counts only once its produced pack is
    // stored and marked: a markerless produced pack loads nowhere, so the roll's consumed packs load as before, while
    // they exist (`decisions/DECISIONS.md`, `roll-up`, "Rehydrate prefers a rolled pack").
    let mut rolls: Vec<(ChainId, PackId, Promoted, Vec<PackId>)> = Vec::new();
    for chain in chains {
        for c in log.compactions(*chain).await? {
            let Some(id) = c.produced.stored_id() else { continue };
            rolls.push((*chain, id, Promoted { from: c.from, to: c.to, pack: c.produced.clone() }, c.consumed));
        }
    }
    let marks: Vec<(PackId, Marker)> = marks.into_iter().collect();
    let mut last: Vec<(ChainId, Option<Promoted>)> = Vec::with_capacity(chains.len());
    for chain in chains {
        last.push((*chain, log.last_promoted(*chain).await?.map(|(_, p)| p)));
    }
    // Promotion goes on while a shard rehydrates, on another site's shard (`git::promote_sites`), so a resumed run can
    // find the last `Promoted` past where its tail got. Its tail then starts past the new promotion, whose pack the
    // marked phase never loaded: every later pack thin against the entries in between failed "have bases no pack
    // provides" (`tests/am1/results/hot-shards-2.md`). The marked phase runs again; `have` skips what the disk holds.
    // A place an older release saved records nothing loaded, and its tail cursor may already sit past entries it
    // skipped that way, so it loads the marked phase again too.
    let gap = last.iter().any(|(c, p)| match state.loaded.iter().find(|(x, _)| x == c) {
        Some((_, through)) => tail_start(p.as_ref()) > (*through).max(Index(state.tail.at(*c).0 + 1)),
        None => true,
    });
    if state.marked && gap {
        state.marked = false;
    }
    let mut evicted = io.make_room(marks.iter().map(|(_, m)| m.bytes).sum()).await?;
    let idx = if state.marked { BTreeMap::new() } else { io.idx_ranges().await? };
    // Each counted roll's produced pack loads first, alone, oldest range first. One that does not load (no bucket
    // copy matches its checksum, the container refuses it, or its bases are not on the disk) is dropped for this run:
    // it loads nowhere, and the packs it consumed, which nothing deletes yet, load in its place as before the roll
    // (`decisions/DECISIONS.md`, `roll-up`, "Rehydrate prefers a rolled pack"). A run past its marked phase loaded them.
    let mut rolled: BTreeSet<PackId> = BTreeSet::new();
    let mut dropped: BTreeSet<PackId> = BTreeSet::new();
    if !state.marked {
        rolls.sort_by_key(|(c, _, p, _)| (*c, p.from));
        let mut kept = Vec::with_capacity(rolls.len());
        for roll in rolls {
            let id = roll.1;
            // A markerless produced pack: the roll does not count, and its consumed packs load as before.
            let Some((_, m)) = marks.iter().find(|(x, _)| *x == id) else { continue };
            let load = [Load::Stored { id, bytes: m.bytes, checksum: m.checksum }];
            let with: BTreeSet<PackId> = idx.contains_key(&id).then_some(id).into_iter().collect();
            let why = match ingest_all(io, &load, &with, &mut r.calls).await {
                Ok(Ok(n)) => {
                    r.stored += n;
                    rolled.insert(id);
                    kept.push(roll);
                    continue;
                }
                Ok(Err(_)) => "its bases are not on the disk".to_string(),
                Err(e) => e.0,
            };
            r.dropped_rolls.push(format!("{}..={} ({id}): {why}", roll.2.from.0, roll.2.to.0));
            dropped.insert(id);
        }
        rolls = kept;
    }
    // Every kept roll's consumed packs are dropped wherever they appear, and its produced pack joins the promotions
    // below; a dropped roll's produced pack is left out the same way.
    let consumed: BTreeSet<PackId> =
        rolls.iter().flat_map(|(_, _, _, c)| c.iter().copied()).chain(dropped.iter().copied()).collect();
    let marked = rehydrate_plan(&marks, &consumed, &last, &[]);
    let mut seen: BTreeSet<PackId> = marked.iter().filter_map(Load::stored_id).chain(rolled.iter().copied()).collect();
    lap(&mut ms, "markers");
    if !state.marked {
        // Every promoted pack first, oldest first: each is complete, but its objects name objects of the promotions
        // before it, and the stored packs below the tail build on their objects, so none of them answers `Later` and
        // costs a fetch twice. Promotion runs ahead of archiving, so a chain's earlier promotions are recorded above
        // its tail start too (`log_archive::step`), where `rehydrate_plan` leaves every marked pack to the tail, since
        // a pushed one there may be thin against the tail: each such marker is told apart by its own entry, one read
        // of one entry, which the tail start keeps in the log.
        let mut promoted: BTreeSet<PackId> = last.iter().filter_map(|(_, p)| p.as_ref()?.pack.stored_id()).collect();
        let starts: BTreeMap<ChainId, Index> = last.iter().map(|(c, p)| (*c, tail_start(p.as_ref()))).collect();
        // A pack whose stored `.idx` names a promoted range is read the same way wherever its entry lies, so its
        // `Promoted` record, not the metadata, says what it covers (`skippable`). An entry already archived cannot be
        // read; its range's pushed packs load as before.
        let mut firsts: BTreeMap<ChainId, Index> = BTreeMap::new();
        for chain in chains {
            firsts.insert(*chain, log.head(*chain).await?.first);
        }
        let mut records: BTreeMap<PackId, (ChainId, Promoted)> = BTreeMap::new();
        for (c, p) in &last {
            if let Some(p) = p
                && let Some(id) = p.pack.stored_id()
            {
                records.insert(id, (*c, p.clone()));
            }
        }
        // Each counted roll's produced pack is read like the promotion whose range it replaced: its record makes
        // `skippable` skip the pushed packs of `from..=to`, it loads first from the markers with its stored `.idx`
        // (`with_idx`), and no probe reads its entry.
        for (chain, id, p, _) in &rolls {
            records.insert(*id, (*chain, p.clone()));
            promoted.insert(*id);
        }
        let probe: Vec<&(PackId, Marker)> = marks
            .iter()
            .filter(|(id, m)| {
                let tail = starts.get(&m.chain).is_some_and(|f| m.index >= *f);
                let named = matches!(idx.get(id), Some(Some(_))) && firsts.get(&m.chain).is_some_and(|f| m.index >= *f);
                !consumed.contains(id) && !promoted.contains(id) && (tail || named)
            })
            .collect();
        for chunk in probe.chunks(PROBE_AT_ONCE) {
            let reads = chunk.iter().map(|(_, m)| log.entries(m.chain, m.index, m.index));
            for ((id, m), page) in chunk.iter().zip(futures_util::future::join_all(reads).await) {
                let page = page?;
                let Some(e) = page.iter().find(|e| e.index == m.index) else {
                    return Err(StoreError(format!("{}: entry {} of a marker was not answered", m.chain, m.index.0)));
                };
                if let Some(p) = e.promoted.as_ref().filter(|p| p.pack.stored_id() == Some(*id)) {
                    promoted.insert(*id);
                    records.insert(*id, (m.chain, p.clone()));
                }
            }
        }
        let at: BTreeMap<PackId, Index> = marks.iter().map(|(id, m)| (*id, m.index)).collect();
        let skip = skippable(&marks, &records, &idx);
        let mut todo: Vec<Load> = marked
            .into_iter()
            .filter(|l| l.stored_id().is_none_or(|id| !skip.contains(&id) && !rolled.contains(&id)))
            .collect();
        for (id, m) in &marks {
            if promoted.contains(id) && seen.insert(*id) {
                todo.push(Load::Stored { id: *id, bytes: m.bytes, checksum: m.checksum });
            }
        }
        todo.sort_by_key(|l| {
            let id = l.stored_id();
            // The promoted and produced group first, oldest by the range its record covers: a roll's produced pack is
            // marked at an entry far above the promotions it replaced, far above later promotions' markers, and by
            // marker index it would load after packs whose objects name its. Pushed packs follow, by marker index.
            (
                !id.is_some_and(|id| promoted.contains(&id)),
                id.and_then(|id| records.get(&id).map(|(_, p)| p.from).or_else(|| at.get(&id).copied())),
            )
        });
        let with_idx: BTreeSet<PackId> = promoted.iter().filter(|id| idx.contains_key(id)).copied().collect();
        match ingest_all(io, &todo, &with_idx, &mut r.calls).await? {
            Ok(n) => r.stored += n,
            Err(n) => return Err(StoreError(format!("{n} marked packs have bases no pack provides"))),
        }
        state.skipped = skip.into_iter().collect();
        state.marked = true;
        state.loaded = last.iter().map(|(c, p)| (*c, tail_start(p.as_ref()))).collect();
        saved.put_rehydrating(&disk, &state).await?;
    }
    lap(&mut ms, "marked");
    let mut t = Tally::default();
    let mut applied = Vec::with_capacity(chains.len());
    let mut head_ref = None;
    for (chain, last) in &last {
        let head = log.head(*chain).await?;
        applied.push(head.applied);
        if *chain == chains[0] {
            head_ref = head.default_branch;
        }
        let from = Index(tail_start(last.as_ref()).0.max(state.tail.at(*chain).0 + 1));
        let mut mode = Mode::Tail { state: &mut state, seen: &mut seen, consumed: &consumed };
        match follow(io, saved, log, &disk, *chain, from, head.applied, WHOLE, &mut mode, &mut t, &mut None, now).await
        {
            Ok(_) => {}
            Err(Stop::Err(e) | Stop::Unresolved(e)) => return Err(e),
            Err(Stop::NoRefs) => unreachable!("a tail applies no ref changes"),
        }
    }
    r.inline = t.inline;
    r.bytes = t.bytes;
    r.entries = t.entries;
    r.stored += t.packs;
    r.calls += t.calls;
    lap(&mut ms, "tail");
    // The refs, a page at a time. Pages read while the chain moves are each at their own index, so the cursor starts
    // at the first page's index, and the entries from there on are caught up after: their changes apply again, the
    // same values in the same order, and their packs arrive before the refs that need them.
    let mut cursor = IngestCursor::new(network);
    cursor.head = head_ref.clone();
    let mut ends = Vec::with_capacity(chains.len());
    for (i, chain) in chains.iter().enumerate() {
        let (mut after, mut first) = (None, None);
        let end = loop {
            let (page, index) = log.refs_page(*chain, after.take()).await?;
            let done = page.next.is_none() && i + 1 == chains.len();
            r.refs += page.refs.len();
            // A rehydrate that skipped covered packs runs the backstop next, which waits for any repack: the repack
            // starts after it instead (`ShardIo::connected`). Started here, the backstop waited out a whole repack of
            // the network, 20 minutes and more for 2.6 GB, answering `warming` (`tests/am1/results/lm1-memory.md`,
            // round 21).
            io.put_refs_page(&page.refs, r.pages == 0, done, state.skipped.is_empty(), head_ref.as_deref()).await?;
            r.pages += 1;
            first.get_or_insert(index);
            match page.next {
                Some(n) => after = Some(n),
                None => break index,
            }
        };
        cursor.advance(*chain, applied[i].min(first.unwrap_or(end)));
        ends.push(end);
    }
    saved.put_cursor(&disk, &cursor).await?;
    lap(&mut ms, "refs");
    // The backstop below runs only when promotions covered packs, and its walk waits for any repack, so then no ref
    // change of this catch-up starts one: the check starts it after its walk, as the last ref page left it. Started by
    // a page here, the walk waited out the whole repack and its commit-graph, 99 to 268 s of the home's rehydrate on
    // B0-34 mt21, mt23 and mt24 (`tests/am1/results/b034-mt24.md`, "The home's rehydrate").
    let repack = state.skipped.is_empty();
    let mut c = Tally::default();
    for (chain, end) in chains.iter().zip(ends) {
        if end <= cursor.at(*chain) {
            continue;
        }
        let from = Index(cursor.at(*chain).0 + 1);
        let mut mode = Mode::CatchUp { cursor: &mut cursor, seen: &mut seen, repack };
        match follow(io, saved, log, &disk, *chain, from, end, WHOLE, &mut mode, &mut c, &mut None, now).await {
            Ok(_) => {}
            Err(Stop::Err(e) | Stop::Unresolved(e)) => return Err(e),
            Err(Stop::NoRefs) => {
                return Err(StoreError("an entry carries no ref changes (an older member release)".into()));
            }
        }
    }
    // The place is dropped only now: until the catch-up from the first ref page reached the head, the disk could not
    // serve it, and a shard still marked rehydrating sends a caller with another shard next on at once. Dropped with
    // the cursor, 338 entries of j9h's took 38 s, and each push the home took waited out its 15 s
    // (`tests/am1/results/hot-shards-3.md`, "The 05:12 stall"). A run cut off before here leaves the place, which the
    // next catch-up from the cursor drops once it reaches the head.
    // The backstop for the packs a promotion covered (`skippable`): the history of every ref, and each tip's tree, must
    // be on the disk. A miss loads the skipped packs, in index order, checks again, and is reported as a fault; a disk
    // that still misses an object never serves, and the next call rehydrates from nothing. The check starts the
    // network's repack once its walk ends, passed or failed, since the last ref page left it (above); a second check
    // after a fault waits for that repack, a cost only the fault path pays.
    lap(&mut ms, "catch_up");
    r.skipped = state.skipped.len();
    if !state.skipped.is_empty()
        && let Err(why) = io.connected(true).await?
    {
        r.fault = Some(why);
        let skipped: BTreeSet<PackId> = state.skipped.iter().copied().collect();
        let mut back: Vec<(Index, Load)> = marks
            .iter()
            .filter(|(id, _)| skipped.contains(id))
            .map(|(id, m)| (m.index, Load::Stored { id: *id, bytes: m.bytes, checksum: m.checksum }))
            .collect();
        back.sort_by_key(|(i, _)| *i);
        let back: Vec<Load> = back.into_iter().map(|(_, l)| l).collect();
        let loaded = ingest_all(io, &back, &BTreeSet::new(), &mut r.calls).await?;
        let again = io.connected(true).await?;
        if let (Ok(_), Ok(())) = (&loaded, &again) {
            r.stored += back.len();
        } else {
            saved.drop_cursor().await?;
            saved.drop_rehydrating().await?;
            let why = again.err().unwrap_or_else(|| "skipped packs have bases no pack provides".into());
            return Err(StoreError(format!(
                "network {}: not connected after loading every skipped pack: {why}",
                network.0.0
            )));
        }
    }
    saved.drop_rehydrating().await?;
    evicted.extend(t.evicted);
    evicted.extend(c.evicted);
    // The backstop's walk, with the skipped packs' load and second walk on the fault path; 0 when nothing was skipped.
    lap(&mut ms, "walk");
    let mut pages_ms = t.ms;
    pages_ms.add(&c.ms);
    Ok(Report {
        room_calls: t.room_calls + c.room_calls,
        behind: false,
        until: cursor.clone(),
        entries: c.entries,
        packs: c.packs,
        refs: c.refs,
        cursor,
        evicted,
        held_max: t.held_max.max(c.held_max),
        disk,
        ms,
        rehydrated: Some(r),
        stored: StoredMs::default(),
        pages_ms,
    })
}
