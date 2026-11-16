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
    /// Every stored `.idx` of the network's packs in the buckets, with the range its custom metadata names, if any.
    async fn idx_ranges(&self) -> Result<BTreeMap<PackId, Option<IdxRange>>, StoreError>;
    /// Whether the history of every ref, and the whole tree at each ref's tip, is on the disk: `Ok(Err(why))` when not.
    /// The check waits for a running repack, so with `repack` the container starts the repack only after it.
    async fn connected(&self, repack: bool) -> Result<Result<(), String>, StoreError>;
    /// A page of the whole ref set; the last page (`done`) also starts the container's repack when `repack`.
    async fn put_refs_page(
        &self,
        refs: &[(String, String)],
        start: bool,
        done: bool,
        repack: bool,
    ) -> Result<(), StoreError>;
    async fn apply_refs(&self, changes: &[(String, Option<String>)]) -> Result<(), StoreError>;
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
    /// Packs loaded through their stored `.idx`, and stored `.idx` the container refused (then `index-pack` ran).
    pub idx_used: u32,
    pub idx_rejected: u32,
    /// The network lock's hold time over the stored packs' ingests.
    pub lock_ms: u64,
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
/// each ingested alone, in its place in the order; those in `with_idx` come with their stored `.idx`. `Ok(n)` with the
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
        let mut i = 0;
        while i < left.len() {
            let (s, l) = left[i];
            let (run, answers) = match l {
                Load::Stored { .. } => {
                    let fetched = if spooled.contains(&s) { Fetched::Spooled } else { ahead.take().await? };
                    match fetched {
                        Fetched::Have => (1, vec![Ingested::Done]),
                        Fetched::Spooled => {
                            *calls += 1;
                            (1, vec![ahead.beside(io.ingest_spooled(s)).await?])
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
/// record would skip packs no promotion holds. This is the read side of `Compacted.consumed`, which nothing writes yet.
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
    CatchUp { cursor: &'a mut IngestCursor, seen: &'a mut BTreeSet<PackId> },
    Tail { state: &'a mut Rehydrating, seen: &'a mut BTreeSet<PackId>, consumed: &'a BTreeSet<PackId> },
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
    evicted: Vec<i64>,
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

/// Applies `chain`'s entries `from..=to` a page at a time, reading the next page while it applies one.
#[allow(clippy::too_many_arguments)]
async fn follow<I: ShardIo, S: Saved, L: ChainLog>(
    io: &I,
    saved: &S,
    log: &L,
    disk: &str,
    chain: ChainId,
    mut from: Index,
    to: Index,
    mode: &mut Mode<'_>,
    t: &mut Tally,
) -> Result<(), Stop> {
    let mut ahead: Option<Result<Vec<LogPacks>, StoreError>> = None;
    while from <= to {
        let page = match ahead.take() {
            Some(p) => p?,
            None => log.entries(chain, from, to).await?,
        };
        let last = page_end(chain, from, to, &page)?;
        let next = Index(last.0 + 1);
        let held = inline_bytes(&page);
        let mut most = held;
        let apply = apply_page(io, saved, disk, chain, from, last, page, mode, t);
        if next <= to {
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
    }
    Ok(())
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
) -> Result<(), Stop> {
    t.entries += page.len();
    let bytes = inline_bytes(&page) as u64;
    if t.room == 0 || t.room < bytes {
        t.evicted.extend(io.make_room(ROOM_EVERY.max(bytes)).await?);
        t.room = ROOM_EVERY.max(bytes);
    }
    t.room -= bytes;
    match mode {
        Mode::CatchUp { cursor, seen } => {
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
            match ingest_all(io, &plan, &BTreeSet::new(), &mut t.calls).await? {
                Ok(n) => t.packs += n,
                Err(n) => return Err(Stop::Unresolved(unresolved(chain, n, from, last))),
            }
            t.refs += changes.len();
            io.apply_refs(&changes).await?;
            cursor.advance(chain, last);
            saved.put_cursor(disk, cursor).await?;
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
            if let Err(n) = ingest_all(io, &plan, &promoted, &mut t.calls).await? {
                return Err(Stop::Unresolved(unresolved(chain, n, from, last)));
            }
            state.tail.advance(chain, last);
            saved.put_rehydrating(disk, state).await?;
        }
    }
    Ok(())
}

/// Catch-up before serving: for each chain whose leader's applied index is past the cursor, its entries a page at a
/// time. No cursor for the container's disk, a cursor below a chain's first log index, or an entry from a member
/// release that sends no ref changes, means a rehydrate. Packs whose bases the disk lacks mean the disk skipped an
/// entry below its cursor: the cursor is dropped, so the next call rehydrates instead of failing on the same pack.
pub async fn catch_up<I: ShardIo, S: Saved, L: ChainLog>(
    io: &I,
    saved: &S,
    log: &L,
    network: NetworkId,
    chains: &[ChainId],
    now: &dyn Fn() -> u64,
) -> Result<Report, StoreError> {
    let t0 = now();
    let mut ms = Vec::new();
    let lap = |ms: &mut Vec<(&'static str, u64)>, name: &'static str| ms.push((name, now().saturating_sub(t0)));
    let disk = io.up().await?;
    lap(&mut ms, "up");
    let mut cursor = saved.cursor().await?.filter(|(d, _)| *d == disk).map(|(_, c)| c);
    lap(&mut ms, "cursor");
    let mut hs = Vec::with_capacity(chains.len());
    for (chain, head) in chains.iter().zip(heads(log, chains).await) {
        let head = head?;
        if cursor.as_ref().is_some_and(|c| c.at(*chain).0 + 1 < head.first.0) {
            cursor = None;
        }
        hs.push(head);
    }
    lap(&mut ms, "heads");
    let Some(mut cursor) = cursor else {
        return rehydrate(io, saved, log, network, chains, disk, ms, t0, now).await;
    };
    // A rehydrate cut off in its last catch-up left its place: this catch-up finishes that one. A place for another
    // disk is a container that is gone, and goes too.
    let finishing = saved.rehydrating().await?.is_some();
    let mut t = Tally::default();
    let mut seen = BTreeSet::new();
    for (chain, head) in chains.iter().zip(hs) {
        let Some(range) = cursor.behind(*chain, head.applied) else { continue };
        let mut mode = Mode::CatchUp { cursor: &mut cursor, seen: &mut seen };
        match follow(io, saved, log, &disk, *chain, *range.start(), *range.end(), &mut mode, &mut t).await {
            Ok(()) => {}
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
    if finishing {
        saved.drop_rehydrating().await?;
    }
    Ok(Report {
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
    })
}

/// `rehydrate`: every stored pack with a marker, minus consumed packs, then the packs each member chain names after its
/// last `Promoted`, read from the log a page at a time, then the refs a page at a time, then a catch-up from the first
/// ref page's index. A run cut off on the same disk resumes: no new repository, no stored pack the container holds
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
    let marks: Vec<(PackId, Marker)> = marks.into_iter().collect();
    r.markers = marks.len();
    let consumed = log.consumed(network).await?;
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
    let marked = rehydrate_plan(&marks, &consumed, &last, &[]);
    let mut seen: BTreeSet<PackId> = marked.iter().filter_map(Load::stored_id).collect();
    let mut evicted = io.make_room(marks.iter().map(|(_, m)| m.bytes).sum()).await?;
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
        let idx = io.idx_ranges().await?;
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
        let mut todo: Vec<Load> =
            marked.into_iter().filter(|l| l.stored_id().is_none_or(|id| !skip.contains(&id))).collect();
        for (id, m) in &marks {
            if promoted.contains(id) && seen.insert(*id) {
                todo.push(Load::Stored { id: *id, bytes: m.bytes, checksum: m.checksum });
            }
        }
        todo.sort_by_key(|l| {
            let id = l.stored_id();
            (!id.is_some_and(|id| promoted.contains(&id)), id.and_then(|id| at.get(&id).copied()))
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
    for (chain, last) in &last {
        let head = log.head(*chain).await?;
        applied.push(head.applied);
        let from = Index(tail_start(last.as_ref()).0.max(state.tail.at(*chain).0 + 1));
        let mut mode = Mode::Tail { state: &mut state, seen: &mut seen, consumed: &consumed };
        match follow(io, saved, log, &disk, *chain, from, head.applied, &mut mode, &mut t).await {
            Ok(()) => {}
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
            io.put_refs_page(&page.refs, r.pages == 0, done, state.skipped.is_empty()).await?;
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
    let mut c = Tally::default();
    for (chain, end) in chains.iter().zip(ends) {
        if end <= cursor.at(*chain) {
            continue;
        }
        let from = Index(cursor.at(*chain).0 + 1);
        let mut mode = Mode::CatchUp { cursor: &mut cursor, seen: &mut seen };
        match follow(io, saved, log, &disk, *chain, from, end, &mut mode, &mut c).await {
            Ok(()) => {}
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
    lap(&mut ms, "catch_up");
    Ok(Report {
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
    })
}
