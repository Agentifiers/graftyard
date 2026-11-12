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
//!   `ENTRIES_LOCAL_PAGE_BYTES_MAX` from the shard's own site's member. Stored packs go from a
//!   bucket to the container without passing through the shard object's memory (`ShardIo::ingest_stored`).
//! - **The cursor moves only past what was applied.** Each page's packs are ingested and its ref changes applied
//!   before the cursor moves, and only to the last index the page held, never to the end of the range asked for. A
//!   short or empty page from a newly elected leader cannot skip an entry (blocker 12).
//! - **Every page is saved before the next is applied.** Catch-up saves its cursor after each page. Rehydrate saves its
//!   tail cursor after each page and skips the stored packs the container already holds (`ShardIo::have`), so a run
//!   cut off at any point, by a caller's deadline or an isolate reset, resumes on the same disk where it stopped.
#![allow(async_fn_in_trait)]

use std::collections::{BTreeMap, BTreeSet};

use graftyard_types::{ChainId, Digest, Index, NetworkId};
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

/// The container's answer to one ingest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ingested {
    Done,
    /// 409: a base or a named object is not on the disk yet; the pack is tried again after the others.
    Later,
}

/// The container a shard object starts, and the buckets its stored packs come from.
pub trait ShardIo {
    /// Starts the container if it is not running; the id of its disk, which a restart replaces.
    async fn up(&self) -> Result<String, StoreError>;
    /// Creates the network's repository, deleting any first when `fresh`.
    async fn init(&self, fresh: bool) -> Result<(), StoreError>;
    /// Of these pack checksums, the ones the repository already ingested.
    async fn have(&self, sums: &[Digest]) -> Result<BTreeSet<Digest>, StoreError>;
    async fn ingest(&self, sum: Digest, body: &[u8]) -> Result<Ingested, StoreError>;
    /// A stored pack, from the first bucket whose copy matches `checksum`, straight to the container.
    async fn ingest_stored(&self, id: PackId, checksum: Digest) -> Result<Ingested, StoreError>;
    async fn put_refs_page(&self, refs: &[(String, String)], start: bool, done: bool) -> Result<(), StoreError>;
    async fn apply_refs(&self, changes: &[(String, Option<String>)]) -> Result<(), StoreError>;
    /// Evicts other networks so `incoming` more bytes fit; the networks evicted.
    async fn make_room(&self, incoming: u64) -> Result<Vec<i64>, StoreError>;
    /// The network's `Committed` markers from every bucket of the placement.
    async fn markers(&self) -> Result<Vec<(PackId, Marker)>, StoreError>;
}

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

/// Ingests `loads` in order; a pack answered `Later` is tried again after the others, until a pass makes no progress.
/// `Ok(n)` with the packs ingested, or `Err(Unresolved)` naming how many never resolved.
async fn ingest_all<I: ShardIo>(io: &I, loads: &[Load]) -> Result<Result<usize, usize>, StoreError> {
    // The packs a call cut off part way through this list already ingested are not sent again: a page's inline packs
    // are up to `ENTRIES_PAGE_BYTES`, and a caller whose deadline falls inside every page would otherwise never pass it.
    let sums: Vec<Digest> = loads.iter().map(load_sum).collect();
    let held = if loads.len() > 1 { io.have(&sums).await? } else { BTreeSet::new() };
    let mut left: Vec<&Load> = loads.iter().filter(|l| !held.contains(&load_sum(l))).collect();
    let mut done = held.len();
    while !left.is_empty() {
        let before = left.len();
        let mut retry = Vec::new();
        for l in left {
            let r = match l {
                Load::Inline { bytes, .. } => io.ingest(Digest(Sha256::digest(bytes).into()), bytes).await?,
                Load::Stored { id, checksum, .. } => io.ingest_stored(*id, *checksum).await?,
            };
            match r {
                Ingested::Done => done += 1,
                Ingested::Later => retry.push(l),
            }
        }
        if retry.len() == before {
            return Ok(Err(retry.len()));
        }
        left = retry;
    }
    Ok(Ok(done))
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
            let plan: Vec<Load> = catch_up_plan(cursor, chain, &page, last)
                .into_iter()
                .filter(|l| l.stored_id().is_none_or(|id| seen.insert(id)))
                .collect();
            drop(page);
            match ingest_all(io, &plan).await? {
                Ok(n) => t.packs += n,
                Err(n) => return Err(Stop::Unresolved(unresolved(chain, n, from, last))),
            }
            t.refs += changes.len();
            io.apply_refs(&changes).await?;
            cursor.advance(chain, last);
            saved.put_cursor(disk, cursor).await?;
        }
        Mode::Tail { state, seen, consumed } => {
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
            if let Err(n) = ingest_all(io, &plan).await? {
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
    let mut t = Tally::default();
    let mut seen = BTreeSet::new();
    for (chain, head) in chains.iter().zip(hs) {
        let Some(range) = cursor.behind(*chain, head.applied) else { continue };
        let mut mode = Mode::CatchUp { cursor: &mut cursor, seen: &mut seen };
        match follow(io, saved, log, &disk, *chain, *range.start(), *range.end(), &mut mode, &mut t).await {
            Ok(()) => {}
            Err(Stop::Err(e)) => return Err(e),
            Err(Stop::NoRefs) => {
                saved.drop_cursor().await?;
                return rehydrate(io, saved, log, network, chains, disk, ms, t0, now).await;
            }
            Err(Stop::Unresolved(e)) => {
                saved.drop_cursor().await?;
                return Err(StoreError(format!("{}; the cursor is dropped, so the next call rehydrates", e.0)));
            }
        }
        lap(&mut ms, "ingest");
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
            let s = Rehydrating { marked: false, tail: IngestCursor::new(network), loaded: Vec::new() };
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
        let probe: Vec<&(PackId, Marker)> = marks
            .iter()
            .filter(|(id, m)| {
                !consumed.contains(id) && !promoted.contains(id) && starts.get(&m.chain).is_some_and(|f| m.index >= *f)
            })
            .collect();
        for chunk in probe.chunks(PROBE_AT_ONCE) {
            let reads = chunk.iter().map(|(_, m)| log.entries(m.chain, m.index, m.index));
            for ((id, m), page) in chunk.iter().zip(futures_util::future::join_all(reads).await) {
                let page = page?;
                let Some(e) = page.iter().find(|e| e.index == m.index) else {
                    return Err(StoreError(format!("{}: entry {} of a marker was not answered", m.chain, m.index.0)));
                };
                if e.promoted.as_ref().and_then(|p| p.pack.stored_id()) == Some(*id) {
                    promoted.insert(*id);
                }
            }
        }
        let at: BTreeMap<PackId, Index> = marks.iter().map(|(id, m)| (*id, m.index)).collect();
        let mut todo = marked;
        for (id, m) in &marks {
            if promoted.contains(id) && seen.insert(*id) {
                todo.push(Load::Stored { id: *id, bytes: m.bytes, checksum: m.checksum });
            }
        }
        todo.sort_by_key(|l| {
            let id = l.stored_id();
            (!id.is_some_and(|id| promoted.contains(&id)), id.and_then(|id| at.get(&id).copied()))
        });
        match ingest_all(io, &todo).await? {
            Ok(n) => r.stored += n,
            Err(n) => return Err(StoreError(format!("{n} marked packs have bases no pack provides"))),
        }
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
            io.put_refs_page(&page.refs, r.pages == 0, done).await?;
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
    saved.drop_rehydrating().await?;
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
    })
}
