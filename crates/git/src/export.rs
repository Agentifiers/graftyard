//! The change feed and the export endpoints, their pure half (`design/DESIGN.md`, Shape, "Lifeboat feed";
//! `build/BUILD-PLAN.md`, M5-WP5). The site's feed consumer and the edge's export routes host it.
//!
//! - **The feed.** Every 10 s (`FEED_EVERY_MS`) a Queue consumer reads the leaders' commit indexes, and `plan` turns
//!   them and the feed's `FeedState` into the next `ChangeBatch`: each chain whose commit index passed its cursor, to
//!   that index. The consumer reads each moved chain's stored entries after its cursor from a follower, writes the
//!   batch with those bytes as one `ExportObject` under `R2Key::Export { minute, seq }`, then advances the state and
//!   writes it under `R2Key::ExportState`. A round that starts by reading the state advances it through every object
//!   written after the state's position (`FeedState::recover`), so a crash between the two writes repeats no entry
//!   and skips none.
//! - **Bodies.** An `ExportObject`, an entries page, and a snapshot are `application/octet-stream` frames
//!   (`frame`): a 4-byte big-endian length, the stored bytes, and their SHA-256.
//! - **The endpoints.** `Route::parse` maps a path and query to one of the four, with the design's errors: a bad
//!   cursor is 422, a range past retention 410, and a chain outside the token's scope 404 (`ExportError`); `serve`
//!   answers the read routes from an `ExportSource`, which the edge backs with a follower, never the leader.

use std::collections::BTreeMap;

use graftyard_types::{ChainId, Digest, Index};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::key::R2Key;
use crate::pack::PackId;

/// How often the feed writes a batch.
pub const FEED_EVERY_MS: u64 = 10_000;
/// `GET export/batches` answers at most this many objects a page.
pub const BATCHES_PAGE: usize = 100;
/// An entries page holds at most this many entries [guess].
pub const ENTRIES_PAGE: u64 = 1_000;
/// An entries page holds at most this many bytes of frames [guess]: a page always holds its first entry.
pub const ENTRIES_PAGE_BYTES: usize = 8 << 20;
/// The most chains one batch names [guess]: an install with more moved chains in 10 s writes them over the next
/// batches, furthest behind first. Each costs the round one source read, six at a time (a Worker's connections waiting
/// for headers), so 200 reads of about 100 ms across regions take about 3.5 s of the 10 s round [inference].
pub const BATCH_CHAINS_MAX: usize = 200;
/// The most stored bytes one source read asks a member for (`/log_rows`), inside the lookup's 3.5 s CALL [guess].
pub const LOG_PAGE_BYTES: usize = 1 << 20;
/// A batches page also stops once it holds this many bytes of objects (always one), so a page of large objects
/// stays inside an edge isolate's memory [guess].
pub const BATCHES_PAGE_BYTES: usize = 8 << 20;
/// The most entry bytes one `ExportObject` holds (always one entry) [guess]: a round past it trims its later chains'
/// ranges and writes the rest in the next rounds, so the consumer and every reader hold one bounded object.
pub const OBJECT_BYTES_MAX: usize = 8 << 20;
/// The export bucket's lifecycle rule [guess]: an object older than this is gone, so a cursor before the oldest
/// object is a range past retention.
pub const EXPORT_RETENTION_MS: u64 = 7 * 24 * 3600 * 1000;

/// Which chains moved in one batch and to what index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeBatch {
    pub minute: u64,
    pub seq: u32,
    pub chains: Vec<(ChainId, Index)>,
}

impl ChangeBatch {
    pub fn key(&self) -> R2Key {
        R2Key::Export { minute: self.minute, seq: self.seq }
    }

    /// The batch's position in the feed, `<minute>.<seq>`, which `export/batches?after=` takes.
    pub fn position(&self) -> FeedPosition {
        FeedPosition { minute: self.minute, seq: self.seq }
    }
}

/// A batch plus, per chain, the stored bytes of every entry after the previous batch's index, from `first`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportObject {
    pub batch: ChangeBatch,
    pub entries: Vec<(ChainId, Index, Vec<Vec<u8>>)>,
}

/// The feed's position on one chain: the last index an `ExportObject` holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportCursor {
    pub chain: ChainId,
    pub index: Index,
}

/// `<minute>.<seq>`: a batch's place in the feed, ordered as the objects are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FeedPosition {
    pub minute: u64,
    pub seq: u32,
}

impl FeedPosition {
    pub fn parse(s: &str) -> Option<FeedPosition> {
        let (m, q) = s.split_once('.')?;
        let digits = |v: &str| !v.is_empty() && v.len() <= 20 && v.bytes().all(|b| b.is_ascii_digit());
        (digits(m) && digits(q)).then_some(())?;
        Some(FeedPosition { minute: m.parse().ok()?, seq: q.parse().ok()? })
    }

    pub fn text(&self) -> String {
        format!("{}.{}", self.minute, self.seq)
    }

    /// The export key a listing starts after: every object at a later position sorts after it.
    pub fn key(&self) -> R2Key {
        R2Key::Export { minute: self.minute, seq: self.seq }
    }
}

/// The feed's cursors, one per chain it has exported.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportCursors(pub BTreeMap<ChainId, Index>);

impl ExportCursors {
    pub fn get(&self, chain: ChainId) -> Index {
        self.0.get(&chain).copied().unwrap_or(Index(0))
    }

    pub fn cursors(&self) -> impl Iterator<Item = ExportCursor> + '_ {
        self.0.iter().map(|(c, i)| ExportCursor { chain: *c, index: *i })
    }
}

/// What the feed consumer stores under `R2Key::ExportState`: the newest object it has folded in, and the cursors
/// after it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedState {
    pub last: Option<FeedPosition>,
    pub cursors: ExportCursors,
}

impl FeedState {
    /// Folds in one batch once its object is written: its position, and each chain it names to its index. A cursor
    /// never moves back.
    pub fn advance(&mut self, batch: &ChangeBatch) {
        self.last = self.last.max(Some(batch.position()));
        for (c, i) in &batch.chains {
            let at = self.cursors.0.entry(*c).or_insert(Index(0));
            *at = (*at).max(*i);
        }
    }

    /// Folds in the batches of the objects stored after `last`, oldest first: the ones a round wrote before it could
    /// write the state.
    pub fn recover<'a>(&mut self, later: impl IntoIterator<Item = &'a ChangeBatch>) {
        for b in later {
            if self.last.is_none_or(|l| b.position() > l) {
                self.advance(b);
            }
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("json")
    }

    pub fn decode(b: &[u8]) -> Result<FeedState, String> {
        serde_json::from_slice(b).map_err(|e| e.to_string())
    }
}

/// The next batch: every chain whose commit index (`commits`, as the leaders reported it) passed its cursor, to that
/// index, at most `BATCH_CHAINS_MAX` of them, the furthest behind first. `seq` follows the state's newest object
/// within a minute and restarts at 0 in a new one; a clock behind that object's minute keeps its minute. `None` when
/// nothing moved: the feed writes no empty object.
pub fn plan(state: &FeedState, commits: &[(ChainId, Index)], now_ms: u64) -> Option<ChangeBatch> {
    let mut moved: Vec<(ChainId, Index, Index)> = commits
        .iter()
        .filter_map(|(c, i)| {
            let at = state.cursors.get(*c);
            (*i > at).then_some((*c, at, *i))
        })
        .collect();
    if moved.is_empty() {
        return None;
    }
    // Most entries behind first, then the chain id, so the choice is the same however `commits` is ordered.
    moved.sort_by(|a, b| (b.2.0 - b.1.0).cmp(&(a.2.0 - a.1.0)).then(a.0.cmp(&b.0)));
    moved.truncate(BATCH_CHAINS_MAX);
    let mut chains: Vec<(ChainId, Index)> = moved.into_iter().map(|(c, _, i)| (c, i)).collect();
    chains.sort();
    let minute = now_ms / 60_000;
    let (minute, seq) = match state.last {
        Some(p) if p.minute >= minute => (p.minute, p.seq + 1),
        _ => (minute, 0),
    };
    Some(ChangeBatch { minute, seq, chains })
}

/// Checks an `ExportObject` before it is written: every chain the batch names has exactly the entries from its
/// cursor + 1 through its index, in order, and no other chain has any. A chain the feed has never carried (cursor 0)
/// may start at the first index its source still stores: entries archived and dropped before the feed existed are
/// the vault's to read from the entries endpoint or a snapshot. Once carried, a chain never skips an index.
pub fn check_object(o: &ExportObject, cursors: &ExportCursors) -> Result<(), String> {
    if o.entries.len() != o.batch.chains.len() {
        return Err(format!("{} chains named, {} with entries", o.batch.chains.len(), o.entries.len()));
    }
    for ((c, to), (ec, first, rows)) in o.batch.chains.iter().zip(&o.entries) {
        if c != ec {
            return Err(format!("entries for {ec} where the batch names {c}"));
        }
        let from = cursors.get(*c).0 + 1;
        let starts = if from == 1 { first.0 >= 1 && first.0 <= to.0 } else { first.0 == from };
        if !starts || rows.len() as u64 != to.0 + 1 - first.0 {
            return Err(format!("{c}: entries {}..+{} where the feed needs {from}..={}", first.0, rows.len(), to.0));
        }
    }
    Ok(())
}

/// What one source read gave for a chain: the first index it holds at or after the asked one, and the stored rows
/// from there in order, stopping at its own commit index or its page's end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub first: Index,
    pub rows: Vec<Vec<u8>>,
}

/// The object a round writes from its planned batch and what its sources gave: each chain to the last row fetched,
/// in the batch's order, until `OBJECT_BYTES_MAX` of rows (always one row). A chain with no rows, one whose source
/// skipped past its cursor + 1 after the feed carried it, or one past the byte cap, is left out, so its cursor stays
/// and a later round carries it. `None` when nothing is left.
pub fn fill(
    batch: &ChangeBatch,
    cursors: &ExportCursors,
    fetched: &BTreeMap<ChainId, Fetched>,
) -> Option<ExportObject> {
    let (mut chains, mut entries, mut bytes) = (Vec::new(), Vec::new(), 0usize);
    for (c, to) in &batch.chains {
        let Some(f) = fetched.get(c) else { continue };
        let from = cursors.get(*c).0 + 1;
        if f.first.0 < from || (from > 1 && f.first.0 != from) || f.first > *to {
            continue;
        }
        let mut rows = Vec::new();
        for r in f.rows.iter().take((to.0 + 1 - f.first.0) as usize) {
            if bytes > 0 && bytes + r.len() > OBJECT_BYTES_MAX {
                break;
            }
            bytes += r.len();
            rows.push(r.clone());
        }
        if rows.is_empty() {
            continue;
        }
        chains.push((*c, Index(f.first.0 + rows.len() as u64 - 1)));
        entries.push((*c, f.first, rows));
        if bytes >= OBJECT_BYTES_MAX {
            break;
        }
    }
    (!chains.is_empty())
        .then_some(ExportObject { batch: ChangeBatch { minute: batch.minute, seq: batch.seq, chains }, entries })
}

/// Whether a leader reports its commit index to the feed now: only past the last index it reported, and at most once
/// every `FEED_EVERY_MS`, so an idle group sends nothing and a busy one one report per round.
pub fn report_due(last: Option<(Index, u64)>, commit: Index, now_ms: u64) -> bool {
    match last {
        None => commit.0 > 0,
        Some((i, at)) => commit > i && now_ms.saturating_sub(at) >= FEED_EVERY_MS,
    }
}

/// A member's `/log_rows` answer: the first index it holds at or after the asked one, its commit index, and the stored
/// rows from there; or, when the asked index is below its stored log, the archive object that holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogPage {
    pub first: Index,
    pub commit: Index,
    #[serde(skip)]
    pub rows: Vec<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<(Index, Index, String)>,
}

/// The rows a member answers from its stored log `(index, row)` read from `from` in order: from the first stored
/// index at or after `from`, through `min(to, commit)`, stopping before `page_bytes` (always one row) and at a gap.
pub fn log_page(
    stored: impl IntoIterator<Item = (Index, Vec<u8>)>,
    from: Index,
    to: Index,
    commit: Index,
    page_bytes: usize,
) -> LogPage {
    let end = to.min(commit);
    let (mut first, mut rows, mut bytes) = (None, Vec::new(), 0usize);
    for (i, row) in stored {
        if i < from {
            continue;
        }
        if i > end || first.is_some_and(|f: Index| i.0 != f.0 + rows.len() as u64) {
            break;
        }
        if !rows.is_empty() && bytes + row.len() > page_bytes {
            break;
        }
        first.get_or_insert(i);
        bytes += row.len();
        rows.push(row);
    }
    LogPage { first: first.unwrap_or(Index(end.0 + 1)), commit, rows, archive: None }
}

/// A `LogPage` as bytes: one JSON frame of its header, then one frame per row.
pub fn encode_log_page(p: &LogPage) -> Vec<u8> {
    let mut out = Vec::new();
    frame(&mut out, &serde_json::to_vec(p).expect("json"));
    for r in &p.rows {
        frame(&mut out, r);
    }
    out
}

pub fn decode_log_page(b: &[u8]) -> Result<LogPage, String> {
    let mut frames = unframe(b)?.into_iter();
    let mut p: LogPage = serde_json::from_slice(&frames.next().ok_or("no header frame")?).map_err(|e| e.to_string())?;
    p.rows = frames.collect();
    Ok(p)
}

impl LogPage {
    /// The rows as a source read's `Fetched`, or `None` for a page that holds none.
    pub fn fetched(self) -> Option<Fetched> {
        (!self.rows.is_empty()).then_some(Fetched { first: self.first, rows: self.rows })
    }
}

/// One frame: a 4-byte big-endian length, the bytes, and their SHA-256.
pub fn frame(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
    out.extend_from_slice(&Sha256::digest(bytes));
}

/// The frames of a body, each checked against its digest.
pub fn unframe(mut b: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::new();
    while !b.is_empty() {
        let n = b.get(..4).ok_or("a frame shorter than its length")?;
        let n = u32::from_be_bytes(n.try_into().expect("4 bytes")) as usize;
        let body = b.get(4..4 + n).ok_or("a frame shorter than its length")?;
        let sum = b.get(4 + n..4 + n + 32).ok_or("a frame without its digest")?;
        if Sha256::digest(body).as_slice() != sum {
            return Err(format!("frame {} fails its digest", out.len()));
        }
        out.push(body.to_vec());
        b = &b[4 + n + 32..];
    }
    Ok(out)
}

/// An `ExportObject`'s stored bytes: the JSON of the batch, then one frame per entry, chain by chain. The JSON's
/// `entries` give each chain's first index and count, so the frames need no header of their own.
pub fn encode_object(o: &ExportObject) -> Vec<u8> {
    let head = serde_json::json!({
        "batch": o.batch,
        "entries": o.entries.iter().map(|(c, first, rows)| (c, first, rows.len())).collect::<Vec<_>>(),
    });
    let mut out = Vec::new();
    frame(&mut out, &serde_json::to_vec(&head).expect("json"));
    for (_, _, rows) in &o.entries {
        for r in rows {
            frame(&mut out, r);
        }
    }
    out
}

pub fn decode_object(b: &[u8]) -> Result<ExportObject, String> {
    #[derive(Deserialize)]
    struct Head {
        batch: ChangeBatch,
        entries: Vec<(ChainId, Index, usize)>,
    }
    let mut frames = unframe(b)?.into_iter();
    let head: Head = serde_json::from_slice(&frames.next().ok_or("no header frame")?).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    for (c, first, n) in head.entries {
        let rows: Vec<Vec<u8>> = frames.by_ref().take(n).collect();
        if rows.len() != n {
            return Err(format!("{c}: {} of {n} entries", rows.len()));
        }
        entries.push((c, first, rows));
    }
    if frames.next().is_some() {
        return Err("frames past the last chain's entries".into());
    }
    Ok(ExportObject { batch: head.batch, entries })
}

/// One of the four export endpoints, parsed from its path below `/api/graftyard/v1/export` and its query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Batches {
        after: FeedPosition,
    },
    Entries {
        chain: ChainId,
        from: Index,
        limit: u64,
    },
    Snapshots {
        chain: ChainId,
        at: Index,
    },
    /// A pack's network comes with it (`?network=<id>`): every pack key starts with its network, so the edge reads
    /// the pack and its `Committed` marker by key, and the scope check names the network's chain.
    Pack {
        network: graftyard_types::NetworkId,
        id: PackId,
    },
    /// `DELETE export/token`: the token revokes itself.
    RevokeToken,
}

/// Why an export request fails, as the design's status codes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportError {
    /// 422: a cursor, index, limit, chain, or pack id that does not parse.
    BadCursor(String),
    /// 410: the range is past retention (the export bucket's lifecycle, or entries archived and dropped).
    Gone(String),
    /// 404: a chain outside the token's scope, or one the install has no group for. Both answer alike, so a token
    /// learns nothing about chains outside its scope.
    NotFound,
    /// 405: a method the path does not take.
    Method,
}

impl ExportError {
    pub fn status(&self) -> u16 {
        match self {
            ExportError::BadCursor(_) => 422,
            ExportError::Gone(_) => 410,
            ExportError::NotFound => 404,
            ExportError::Method => 405,
        }
    }

    pub fn message(&self) -> String {
        match self {
            ExportError::BadCursor(m) => format!("Validation Failed: {m}"),
            ExportError::Gone(m) => format!("Gone: {m}"),
            ExportError::NotFound => "Not Found".into(),
            ExportError::Method => "Method Not Allowed".into(),
        }
    }
}

pub const EXPORT_PREFIX: &str = "/api/graftyard/v1/export";

impl Route {
    /// `path` is the request path, `query` its pairs. A path under `EXPORT_PREFIX` that names no endpoint is 404.
    pub fn parse(method: &str, path: &str, query: &[(String, String)]) -> Result<Route, ExportError> {
        let rest = path.strip_prefix(EXPORT_PREFIX).ok_or(ExportError::NotFound)?;
        let q = |k: &str| query.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let index = |k: &str, v: Option<&str>| -> Result<Index, ExportError> {
            let v = v.ok_or_else(|| ExportError::BadCursor(format!("{k} is required")))?;
            let ok = !v.is_empty() && v.len() <= 20 && v.bytes().all(|b| b.is_ascii_digit());
            ok.then(|| v.parse().ok()).flatten().map(Index).ok_or_else(|| ExportError::BadCursor(format!("{k} {v:?}")))
        };
        let chain = |c: &str| c.parse::<ChainId>().map_err(|_| ExportError::BadCursor(format!("chain {c:?}")));
        let parts: Vec<&str> = rest.split('/').collect();
        let route = match parts.as_slice() {
            ["", "batches"] => {
                let after = match q("after") {
                    None => FeedPosition::default(),
                    Some(a) => FeedPosition::parse(a).ok_or_else(|| ExportError::BadCursor(format!("after {a:?}")))?,
                };
                Route::Batches { after }
            }
            ["", "chains", c, "entries"] => {
                let limit = match q("limit") {
                    None => ENTRIES_PAGE,
                    Some(l) => match l.parse::<u64>() {
                        Ok(n) if (1..=ENTRIES_PAGE).contains(&n) => n,
                        _ => return Err(ExportError::BadCursor(format!("limit {l:?} (1 to {ENTRIES_PAGE})"))),
                    },
                };
                let from = index("from", q("from"))?;
                if from.0 == 0 {
                    return Err(ExportError::BadCursor("from 0: the first entry is 1".into()));
                }
                Route::Entries { chain: chain(c)?, from, limit }
            }
            ["", "chains", c, "snapshots"] => Route::Snapshots { chain: chain(c)?, at: index("at", q("at"))? },
            ["", "packs", id] => {
                let n = q("network").ok_or_else(|| ExportError::BadCursor("network is required".into()))?;
                let ok = !n.is_empty() && n.len() <= 19 && n.bytes().all(|b| b.is_ascii_digit());
                let network = ok
                    .then(|| n.parse::<i64>().ok())
                    .flatten()
                    .map(|v| graftyard_types::NetworkId(graftyard_types::DbId(v)))
                    .ok_or_else(|| ExportError::BadCursor(format!("network {n:?}")))?;
                Route::Pack { network, id: id.parse().map_err(|_| ExportError::BadCursor(format!("pack id {id:?}")))? }
            }
            ["", "token"] => Route::RevokeToken,
            _ => return Err(ExportError::NotFound),
        };
        let want = if matches!(route, Route::RevokeToken) { "DELETE" } else { "GET" };
        if method != want {
            return Err(ExportError::Method);
        }
        Ok(route)
    }

    /// The chain the route reads, which the token's scope must cover. A pack's chain is its network's.
    pub fn chain(&self) -> Option<ChainId> {
        match self {
            Route::Entries { chain, .. } | Route::Snapshots { chain, .. } => Some(*chain),
            Route::Pack { network, .. } => Some(ChainId::Network(*network)),
            Route::Batches { .. } | Route::RevokeToken => None,
        }
    }
}

/// The `Link` header of a page with more after it: `rel="next"` with the next cursor. `next` is the query that
/// continues the page, `after=<minute>.<seq>` or `from=<index>&limit=<n>`.
pub fn next_link(root: &str, path: &str, next: &str) -> String {
    format!("<{root}{path}?{next}>; rel=\"next\"")
}

/// An entries page from stored rows `(index, bytes)` read from `from`, in order: at most `limit` entries and
/// `ENTRIES_PAGE_BYTES` of frames, always holding its first entry. `first` is the member's first stored index: a
/// `from` below it is past retention unless an archive object serves it (`archived`). Returns the body and the next
/// index when more lies within `to`, its commit index: a page cut at `limit` or the byte bound, or rows that ended
/// before `to` (the member's own page bound), links on from the next index.
pub fn entries_page(
    rows: impl IntoIterator<Item = (Index, Vec<u8>)>,
    from: Index,
    limit: u64,
    first: Index,
    archived: bool,
    to: Index,
) -> Result<(Vec<u8>, Option<Index>), ExportError> {
    if from < first && !archived {
        return Err(ExportError::Gone(format!("entries before {} are archived and gone", first.0)));
    }
    if from > to {
        return Ok((Vec::new(), None));
    }
    let (mut out, mut n, mut want) = (Vec::new(), 0u64, from);
    for (i, row) in rows {
        if i > to {
            break;
        }
        if i != want {
            return Err(ExportError::Gone(format!("entry {} is missing from the stored log", want.0)));
        }
        if n > 0 && (n == limit || out.len() + 36 + row.len() > ENTRIES_PAGE_BYTES) {
            return Ok((out, Some(i)));
        }
        frame(&mut out, &row);
        n += 1;
        want = Index(i.0 + 1);
    }
    Ok((out, (n > 0 && want <= to).then_some(want)))
}

/// The newest snapshot key at or below `at` for `chain` among `keys`, as a member installs it: its core tier and its
/// metadata tier at the same `to`, core first. `None` when no snapshot is at or below `at`.
pub fn snapshot_at(keys: &[R2Key], chain: ChainId, at: Index) -> Option<Vec<R2Key>> {
    let mut by_to: BTreeMap<Index, Vec<R2Key>> = BTreeMap::new();
    for k in keys {
        if let R2Key::Snapshot { chain: c, to, .. } = k
            && *c == chain
            && *to <= at
        {
            by_to.entry(*to).or_default().push(k.clone());
        }
    }
    let (_, mut ks) = by_to.into_iter().next_back()?;
    ks.sort_by_key(|k| !matches!(k, R2Key::Snapshot { tier: graftyard_types::Tier::Core, .. }));
    Some(ks)
}

/// The digest a client checks a pack against: `X-Graftyard-Checksum`, the `PackRef.checksum`'s hex.
pub fn checksum_header(d: &Digest) -> String {
    d.hex()
}

/// What a member, learner, or archive gives the export endpoints. The edge implements it over a follower's reads and
/// the export and site buckets; it never asks the leader.
pub trait ExportSource {
    /// Stored `ExportObject` bytes after `after`, in order: at most `n` objects, stopping once they hold
    /// `BATCHES_PAGE_BYTES` (`batches_take`).
    fn objects_after(&self, after: FeedPosition, n: usize) -> Vec<Vec<u8>>;
    /// The chain's stored log rows from `from`: `None` for a chain with no group. `first` is the member's first stored
    /// index, `archived` whether archive objects serve the indexes below it, and `to` the member's commit index.
    fn log(&self, chain: ChainId, from: Index, limit: u64) -> Option<StoredLog>;
    /// Every snapshot key of `chain`.
    fn snapshots(&self, chain: ChainId) -> Vec<R2Key>;
    /// A snapshot object's bytes.
    fn object(&self, key: &R2Key) -> Option<Vec<u8>>;
    /// A stored pack's checksum and bytes, by its network and id: `None` when no `Committed` marker names it, so an
    /// uncommitted or swept pack is never served.
    fn pack(&self, network: graftyard_types::NetworkId, id: PackId) -> Option<(Digest, Vec<u8>)>;
}

/// What the edge read for one request, as a source for `serve`: it reads asynchronously from members and buckets
/// first, then answers through the same rules the model install uses.
#[derive(Clone, Debug, Default)]
pub struct Prefetched {
    pub objects: Vec<Vec<u8>>,
    pub log: Option<StoredLog>,
    pub snapshots: Vec<R2Key>,
    pub snapshot_bytes: BTreeMap<R2Key, Vec<u8>>,
}

impl ExportSource for Prefetched {
    fn objects_after(&self, _after: FeedPosition, n: usize) -> Vec<Vec<u8>> {
        self.objects.iter().take(n).cloned().collect()
    }
    fn log(&self, _chain: ChainId, _from: Index, _limit: u64) -> Option<StoredLog> {
        self.log.clone()
    }
    fn snapshots(&self, _chain: ChainId) -> Vec<R2Key> {
        self.snapshots.clone()
    }
    fn object(&self, key: &R2Key) -> Option<Vec<u8>> {
        self.snapshot_bytes.get(key).cloned()
    }
    /// The edge streams packs itself (`in_scope`, then the marker and the bucket).
    fn pack(&self, _network: graftyard_types::NetworkId, _id: PackId) -> Option<(Digest, Vec<u8>)> {
        None
    }
}

/// An archive object's rows `from..` read as its bytes arrive, so the edge never holds the whole object: the object
/// starts at `first` (its range's first index) after `graftyard_fold::ARCHIVE_HEADER`, and each row is a 4-byte length
/// and the stored bytes. It keeps rows from `from` through `to`, at most `limit` of them and about
/// `ENTRIES_PAGE_BYTES`, which is as much as one entries page serves.
#[derive(Clone, Debug)]
pub struct ArchiveRows {
    at: Index,
    from: Index,
    to: Index,
    limit: u64,
    header: bool,
    buf: Vec<u8>,
    bytes: usize,
    pub rows: Vec<(Index, Vec<u8>)>,
}

impl ArchiveRows {
    pub fn new(first: Index, from: Index, to: Index, limit: u64) -> ArchiveRows {
        ArchiveRows { at: first, from, to, limit, header: false, buf: Vec::new(), bytes: 0, rows: Vec::new() }
    }

    /// Takes the next chunk of the object. `Ok(true)` once the page is full or the range is past `to`, so the caller
    /// stops reading.
    pub fn push(&mut self, chunk: &[u8]) -> Result<bool, String> {
        self.buf.extend_from_slice(chunk);
        if !self.header {
            if self.buf.len() < graftyard_fold::ARCHIVE_HEADER {
                return Ok(false);
            }
            graftyard_fold::check_archive_header(&self.buf).map_err(|e| e.to_string())?;
            self.buf.drain(..graftyard_fold::ARCHIVE_HEADER);
            self.header = true;
        }
        let (rows, used, _) = graftyard_fold::decode_archive_some(&self.buf);
        let rows: Vec<Vec<u8>> = rows.into_iter().map(<[u8]>::to_vec).collect();
        self.buf.drain(..used);
        for row in rows {
            if self.done() {
                break;
            }
            if self.at >= self.from {
                self.bytes += row.len();
                self.rows.push((self.at, row));
            }
            self.at = Index(self.at.0 + 1);
        }
        Ok(self.done())
    }

    fn done(&self) -> bool {
        self.at > self.to || self.rows.len() as u64 >= self.limit || self.bytes >= ENTRIES_PAGE_BYTES
    }
}

/// Whether a member answers `/log_rows`: never a leader of a group that has another member to ask, so neither the
/// feed nor an export read lands on a leader (`design/DESIGN.md`, Shape, "Lifeboat feed"). A group of one voter and
/// no learner (`Solo`) has no other member.
pub fn serves_log(leading: bool, voters: usize, learners: usize) -> bool {
    !leading || (voters <= 1 && learners == 0)
}

/// Whether a feed round may start: at least `FEED_EVERY_MS` less a second after the last one started, so the cron's
/// kick and a delayed queue message that meet collapse into one chain of rounds.
pub fn round_due(last_ms: Option<u64>, now_ms: u64) -> bool {
    last_ms.is_none_or(|l| now_ms.saturating_sub(l) >= FEED_EVERY_MS - 1_000)
}

/// Whether a leader's commit report replaces the one held for its chain: only a higher commit index, so a deposed
/// leader's late report never moves the feed's view back.
pub fn report_newer(held: Option<Index>, reported: Index) -> bool {
    held.is_none_or(|h| reported > h)
}

/// The chains every round probes beside the reports (`decisions/DECISIONS.md`, `export-host`, the sweep): the system
/// chains with one group each, whose records the genesis holds. A site day resets every member, and a group nothing
/// calls elects no leader, so no report would ever name them (m3fx49b).
pub const ROUND_PROBES: [ChainId; 2] = [ChainId::Release, ChainId::Ids];
/// `MemberRegistry` shards the sweep reads a round [guess]: one, so a sweep of three sites' 64 shards takes 192 rounds,
/// 32 minutes, and probes each chain once in that time, about 100 chains a round at 2,000 developers
/// (`scripts/cost.py`, the sweep).
pub const SWEEP_SHARDS: u64 = 1;

/// The registry slots `(site, shard)` the sweep's round `n` reads, `k` of them, over `sites` sites in the table's
/// order: round `n` takes slots `n·k` to `n·k + k − 1` of the sweep, which visits every site's every shard once.
pub fn sweep_slots(n: u64, k: u64, sites: usize) -> Vec<(usize, u8)> {
    let per = u64::from(graftyard_types::REGISTRY_SHARDS);
    let total = per * sites as u64;
    if total == 0 {
        return Vec::new();
    }
    let start = n.wrapping_mul(k) % total;
    (0..k.min(total)).map(|j| (start + j) % total).map(|i| ((i / per) as usize, (i % per) as u8)).collect()
}

/// The chains a sweep slot probes, from one site's registry rows: each on the site its row names first, so the sweep
/// probes a chain once whichever sites hold it, and none of the round's own probes.
pub fn swept(
    rows: &[graftyard_types::RegistryRow],
    site: graftyard_types::SiteId,
) -> Vec<&graftyard_types::RegistryRow> {
    rows.iter().filter(|r| r.sites.first() == Some(&site) && !ROUND_PROBES.contains(&r.chain)).collect()
}

/// The commit indexes a round plans from: each chain's leader report, raised by a probe of a non-leader's commit, the
/// higher winning, so a chain no leader reports is planned from its probe and a report is never moved back.
pub fn with_probes(reports: &[(ChainId, Index)], probes: &[(ChainId, Index)]) -> Vec<(ChainId, Index)> {
    let mut at: BTreeMap<ChainId, Index> = BTreeMap::new();
    for (c, i) in reports.iter().chain(probes) {
        let e = at.entry(*c).or_insert(*i);
        *e = (*e).max(*i);
    }
    at.into_iter().collect()
}

/// Whether a batches page that already holds `held` objects of `bytes` takes another: under `BATCHES_PAGE` objects
/// and `BATCHES_PAGE_BYTES`, so a page always holds its first object.
pub fn batches_take(held: usize, bytes: usize) -> bool {
    held < BATCHES_PAGE && (held == 0 || bytes < BATCHES_PAGE_BYTES)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredLog {
    pub first: Index,
    pub archived: bool,
    pub to: Index,
    pub rows: Vec<(Index, Vec<u8>)>,
}

/// A 2xx export reply: an `application/octet-stream` body, its `Link` next, and a pack's `X-Graftyard-Checksum`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportReply {
    pub body: Vec<u8>,
    pub next: Option<String>,
    pub checksum: Option<String>,
}

/// A route's chain (`Route::chain`) outside the token's scope answers 404, before any read: the edge streams packs
/// itself and checks this first, as `serve` does. The feed (`Batches`) names every chain of the install, so only a
/// token that covers every chain reads it; `covers(ChainId::Release)` holds only for such a token
/// (`graftyard_identity::export_token::covers`), so an org's token reads its chains' entries and never the feed.
pub fn in_scope(route: &Route, covers: &dyn Fn(ChainId) -> bool) -> Result<(), ExportError> {
    match (route, route.chain()) {
        (Route::Batches { .. }, _) if !covers(ChainId::Release) => Err(ExportError::NotFound),
        (_, Some(c)) if !covers(c) => Err(ExportError::NotFound),
        _ => Ok(()),
    }
}

/// Serves a parsed read route. `covers` is the token's scope over a chain (`export_token::covers` with the chain's
/// owner); `now_ms` dates the feed's retention. A chain or pack outside `covers` answers as one that does not exist.
pub fn serve(
    route: &Route,
    covers: &dyn Fn(ChainId) -> bool,
    src: &dyn ExportSource,
    now_ms: u64,
    root: &str,
) -> Result<ExportReply, ExportError> {
    let reply = |body, next| ExportReply { body, next, checksum: None };
    in_scope(route, covers)?;
    match route {
        Route::Batches { after } => {
            if *after != FeedPosition::default() && (after.minute * 60_000).saturating_add(EXPORT_RETENTION_MS) < now_ms
            {
                return Err(ExportError::Gone(format!("batches after {} are past the feed's retention", after.text())));
            }
            let objects = src.objects_after(*after, BATCHES_PAGE);
            let last = objects.last().map(|o| decode_object(o).map(|o| o.batch.position())).transpose();
            let last = last.map_err(|e| ExportError::Gone(format!("a stored batch does not decode: {e}")))?;
            let mut body = Vec::new();
            for o in &objects {
                frame(&mut body, o);
            }
            let full =
                objects.len() >= BATCHES_PAGE || objects.iter().map(Vec::len).sum::<usize>() >= BATCHES_PAGE_BYTES;
            let next = full.then(|| {
                next_link(root, &format!("{EXPORT_PREFIX}/batches"), &format!("after={}", last.expect("a page").text()))
            });
            Ok(reply(body, next))
        }
        Route::Entries { chain, from, limit } => {
            let log = src.log(*chain, *from, *limit).ok_or(ExportError::NotFound)?;
            let (body, next) = entries_page(log.rows, *from, *limit, log.first, log.archived, log.to)?;
            let next = next.map(|i| {
                next_link(
                    root,
                    &format!("{EXPORT_PREFIX}/chains/{chain}/entries"),
                    &format!("from={}&limit={limit}", i.0),
                )
            });
            Ok(reply(body, next))
        }
        Route::Snapshots { chain, at } => {
            let keys = snapshot_at(&src.snapshots(*chain), *chain, *at).ok_or(ExportError::NotFound)?;
            let mut body = Vec::new();
            for k in &keys {
                let bytes = src.object(k).ok_or_else(|| ExportError::Gone(format!("{k} is gone")))?;
                frame(&mut body, k.to_string().as_bytes());
                frame(&mut body, &bytes);
            }
            Ok(reply(body, None))
        }
        Route::Pack { network, id } => {
            let (sum, bytes) = src.pack(*network, *id).ok_or(ExportError::NotFound)?;
            Ok(ExportReply { body: bytes, next: None, checksum: Some(checksum_header(&sum)) })
        }
        Route::RevokeToken => Err(ExportError::Method),
    }
}

#[cfg(test)]
mod tests {
    use graftyard_types::{DbId, NetworkId, RepoId, Tier, TokenShard};

    use super::*;

    fn repo(n: i64) -> ChainId {
        ChainId::Repo(RepoId(DbId(n)))
    }

    fn cursors(at: &[(ChainId, u64)]) -> ExportCursors {
        ExportCursors(at.iter().map(|(c, i)| (*c, Index(*i))).collect())
    }

    fn state(at: &[(ChainId, u64)], last: Option<(u64, u32)>) -> FeedState {
        FeedState { last: last.map(|(minute, seq)| FeedPosition { minute, seq }), cursors: cursors(at) }
    }

    /// The sweep visits every site's every shard exactly once per 192 rounds, then starts again; it probes a chain on
    /// its first site only, never `Release` or `Ids` (probed every round); and a probe only ever raises a report.
    #[test]
    fn the_sweep_visits_each_shard_once_and_a_probe_never_lowers_a_report() {
        use graftyard_types::{REGISTRY_SHARDS, RegistryRow, SiteId};
        let per = u64::from(REGISTRY_SHARDS);
        let rounds = per * 3 / SWEEP_SHARDS;
        let seen: Vec<(usize, u8)> = (0..rounds).flat_map(|n| sweep_slots(n, SWEEP_SHARDS, 3)).collect();
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!((seen.len(), sorted.len()), (192, 192), "every slot once a sweep");
        assert_eq!(sweep_slots(rounds, SWEEP_SHARDS, 3), sweep_slots(0, SWEEP_SHARDS, 3), "then again from the start");
        assert_eq!(
            (sweep_slots(0, 1, 3), sweep_slots(64, 1, 3), sweep_slots(65, 1, 3)),
            (vec![(0, 0)], vec![(1, 0)], vec![(1, 1)])
        );
        assert_eq!(sweep_slots(5, 1, 0), vec![]);
        let row = |c: ChainId, sites: &[u8]| RegistryRow {
            chain: c,
            do_name: "x".into(),
            generation: 1,
            sites: sites.iter().map(|s| SiteId(*s)).collect(),
        };
        let rows = [row(repo(1), &[1, 0, 2]), row(repo(2), &[0, 1, 2]), row(ChainId::Release, &[0, 1, 2])];
        let got: Vec<ChainId> = swept(&rows, SiteId(0)).into_iter().map(|r| r.chain).collect();
        assert_eq!(got, vec![repo(2)]);
        let merged = with_probes(
            &[(repo(1), Index(9)), (repo(2), Index(4))],
            &[(repo(1), Index(7)), (repo(2), Index(6)), (ChainId::Ids, Index(3))],
        );
        assert_eq!(merged, vec![(repo(1), Index(9)), (repo(2), Index(6)), (ChainId::Ids, Index(3))]);
    }

    #[test]
    fn plan_names_only_chains_past_their_cursor_and_numbers_batches_by_minute() {
        let at = [(repo(1), 10), (repo(2), 7), (ChainId::Release, 3)];
        let commits = [(repo(1), Index(10)), (repo(2), Index(9)), (ChainId::Release, Index(4)), (repo(3), Index(2))];
        let b = plan(&state(&at, None), &commits, 120_000 + 59_999).unwrap();
        assert_eq!((b.minute, b.seq), (2, 0));
        assert_eq!(b.chains, vec![(repo(2), Index(9)), (repo(3), Index(2)), (ChainId::Release, Index(4))]);
        // The next batch in the same minute follows the newest object's seq; a new minute starts at 0.
        assert_eq!(plan(&state(&at, Some((2, 0))), &commits, 130_000).map(|b| (b.minute, b.seq)), Some((2, 1)));
        assert_eq!(plan(&state(&at, Some((2, 1))), &commits, 180_000).map(|b| (b.minute, b.seq)), Some((3, 0)));
        // A clock behind the newest object (another isolate's) never writes a position before it.
        assert_eq!(plan(&state(&at, Some((2, 4))), &commits, 60_000).map(|b| (b.minute, b.seq)), Some((2, 5)));
        assert_eq!(plan(&state(&at, None), &[(repo(1), Index(10)), (repo(2), Index(7))], 120_000), None);
    }

    #[test]
    fn advance_moves_named_cursors_never_back_and_recover_folds_only_later_objects() {
        let mut s = state(&[(repo(1), 10), (repo(2), 7)], Some((5, 2)));
        let old = ChangeBatch { minute: 5, seq: 2, chains: vec![(repo(2), Index(100))] };
        let b1 = ChangeBatch { minute: 5, seq: 3, chains: vec![(repo(1), Index(4)), (repo(3), Index(2))] };
        let b2 = ChangeBatch { minute: 6, seq: 0, chains: vec![(repo(1), Index(12))] };
        // The state was written after `old`; `b1` and `b2` were written by a round that crashed before its state.
        s.recover([&old, &b1, &b2]);
        assert_eq!(s.last, Some(b2.position()));
        let got = (s.cursors.get(repo(1)), s.cursors.get(repo(2)), s.cursors.get(repo(3)));
        assert_eq!(got, (Index(12), Index(7), Index(2)));
        // After recovery, the same commits plan nothing, so no entry is written twice.
        let commits = [(repo(1), Index(12)), (repo(2), Index(7)), (repo(3), Index(2))];
        assert_eq!(plan(&s, &commits, 400_000), None);
        assert_eq!(FeedState::decode(&s.encode()), Ok(s.clone()));
        // A batch naming a chain at a lower index (planned before a later one) moves nothing back.
        s.advance(&ChangeBatch { minute: 6, seq: 1, chains: vec![(repo(1), Index(4))] });
        assert_eq!(s.cursors.get(repo(1)), Index(12));
    }

    #[test]
    fn a_full_batch_takes_the_chains_furthest_behind() {
        let n = BATCH_CHAINS_MAX + 3;
        // Chain i is i + 1 entries behind; the 3 with the fewest are left for the next batch.
        let commits: Vec<_> = (0..n as i64).map(|i| (repo(i), Index(i as u64 + 1))).collect();
        let b = plan(&FeedState::default(), &commits, 0).unwrap();
        assert_eq!(b.chains.len(), BATCH_CHAINS_MAX);
        assert!(b.chains.iter().all(|(c, _)| !matches!(c, ChainId::Repo(RepoId(DbId(0..=2))))), "{:?}", &b.chains[..3]);
    }

    #[test]
    fn check_object_refuses_a_gap_an_overlap_or_another_chain() {
        let cur = cursors(&[(repo(1), 10)]);
        let batch = ChangeBatch { minute: 1, seq: 0, chains: vec![(repo(1), Index(12)), (repo(2), Index(1))] };
        let rows = |n: usize| (0..n).map(|i| vec![i as u8]).collect::<Vec<_>>();
        let good = ExportObject {
            batch: batch.clone(),
            entries: vec![(repo(1), Index(11), rows(2)), (repo(2), Index(1), rows(1))],
        };
        assert_eq!(check_object(&good, &cur), Ok(()));
        for entries in [
            vec![(repo(1), Index(10), rows(3)), (repo(2), Index(1), rows(1))],
            vec![(repo(1), Index(11), rows(1)), (repo(2), Index(1), rows(1))],
            vec![(repo(1), Index(12), rows(1)), (repo(2), Index(1), rows(1))],
            // The right count, starting one late: entry 11 is skipped.
            vec![(repo(1), Index(12), rows(2)), (repo(2), Index(1), rows(1))],
            vec![(repo(1), Index(11), rows(2)), (repo(3), Index(1), rows(1))],
            vec![(repo(1), Index(11), rows(2))],
        ] {
            assert!(
                check_object(&ExportObject { batch: batch.clone(), entries: entries.clone() }, &cur).is_err(),
                "{entries:?}"
            );
        }
    }

    fn fetched(first: u64, rows: &[&[u8]]) -> Fetched {
        Fetched { first: Index(first), rows: rows.iter().map(|r| r.to_vec()).collect() }
    }

    #[test]
    fn fill_carries_what_sources_gave_and_never_skips_a_carried_index() {
        let cur = cursors(&[(repo(1), 10), (repo(2), 4)]);
        let batch = ChangeBatch {
            minute: 7,
            seq: 2,
            chains: vec![(repo(1), Index(14)), (repo(2), Index(9)), (repo(3), Index(30)), (repo(4), Index(5))],
        };
        let f = BTreeMap::from([
            // A follower behind the leader's report: 11 and 12 only, so repo:1 goes to 12, not 14.
            (repo(1), fetched(11, &[b"11", b"12"])),
            // A source whose log starts past the cursor: carried, it must not skip 5..=6.
            (repo(2), fetched(7, &[b"7", b"8"])),
            // Never carried: it may start where its source's stored log starts.
            (repo(3), fetched(21, &[b"21", b"22", b"23"])),
            // No rows: left out.
            (repo(4), fetched(1, &[])),
        ]);
        let o = fill(&batch, &cur, &f).unwrap();
        assert_eq!((o.batch.minute, o.batch.seq), (7, 2));
        assert_eq!(o.batch.chains, vec![(repo(1), Index(12)), (repo(3), Index(23))]);
        assert_eq!(check_object(&o, &cur), Ok(()));
        // Rows past the batch's index are not taken.
        let more = BTreeMap::from([(repo(1), fetched(11, &[b"11", b"12", b"13", b"14", b"15"]))]);
        assert_eq!(fill(&batch, &cur, &more).unwrap().batch.chains, vec![(repo(1), Index(14))]);
        // A row at or below the cursor (an overlap) is refused, not re-sent.
        let back = BTreeMap::from([(repo(1), fetched(10, &[b"10", b"11"]))]);
        assert_eq!(fill(&batch, &cur, &back), None);
        // The byte cap: the first row always goes; repo:1 is trimmed at the row that would pass it, a later chain's
        // row that fits still goes, and one that does not waits.
        let big = vec![0u8; OBJECT_BYTES_MAX - 10];
        let capped = BTreeMap::from([
            (repo(1), Fetched { first: Index(11), rows: vec![big.clone(), vec![1; 20]] }),
            (repo(2), Fetched { first: Index(5), rows: vec![vec![2; 11]] }),
            (repo(3), fetched(21, &[b"21"])),
        ]);
        let o = fill(&batch, &cur, &capped).unwrap();
        assert_eq!(o.batch.chains, vec![(repo(1), Index(11)), (repo(3), Index(21))]);
        assert_eq!(check_object(&o, &cur), Ok(()));
        let huge = BTreeMap::from([(repo(1), Fetched { first: Index(11), rows: vec![vec![0; OBJECT_BYTES_MAX + 5]] })]);
        assert_eq!(fill(&batch, &cur, &huge).unwrap().batch.chains, vec![(repo(1), Index(11))]);
        // check_object holds a carried chain to its cursor + 1 and a new one to an index inside its batch range.
        let late = ExportObject {
            batch: ChangeBatch { minute: 7, seq: 2, chains: vec![(repo(2), Index(8))] },
            entries: vec![(repo(2), Index(7), vec![b"7".to_vec(), b"8".to_vec()])],
        };
        assert!(check_object(&late, &cur).is_err());
        let past = ExportObject {
            batch: ChangeBatch { minute: 7, seq: 2, chains: vec![(repo(3), Index(20))] },
            entries: vec![(repo(3), Index(21), vec![])],
        };
        assert!(check_object(&past, &cur).is_err());
    }

    #[test]
    fn reports_are_paced() {
        assert!(report_due(None, Index(1), 0));
        assert!(!report_due(None, Index(0), 0));
        assert!(!report_due(Some((Index(5), 1_000)), Index(5), 99_000), "nothing new");
        assert!(!report_due(Some((Index(5), 1_000)), Index(9), 1_000 + FEED_EVERY_MS - 1), "within a round");
        assert!(report_due(Some((Index(5), 1_000)), Index(9), 1_000 + FEED_EVERY_MS));
    }

    #[test]
    fn batches_pages_stop_at_the_count_or_the_bytes() {
        assert!(batches_take(0, usize::MAX), "a page always holds one");
        assert!(batches_take(BATCHES_PAGE - 1, 0));
        assert!(!batches_take(BATCHES_PAGE, 0));
        assert!(batches_take(3, BATCHES_PAGE_BYTES - 1));
        assert!(!batches_take(3, BATCHES_PAGE_BYTES));
        let pack = Route::Pack { network: NetworkId(DbId(9)), id: PackId(graftyard_types::Ulid(1)) };
        assert_eq!(in_scope(&pack, &|c| c != ChainId::Network(NetworkId(DbId(9)))), Err(ExportError::NotFound));
        assert_eq!(in_scope(&pack, &|_| true), Ok(()));
        let feed = Route::Batches { after: FeedPosition::default() };
        assert_eq!(in_scope(&feed, &|c| c == ChainId::Release), Ok(()));
        assert_eq!(in_scope(&feed, &|c| c != ChainId::Release), Err(ExportError::NotFound), "an org token");
        assert_eq!(in_scope(&Route::RevokeToken, &|_| false), Ok(()));
    }

    #[test]
    fn a_log_page_starts_at_the_first_stored_index_and_stops_at_commit_bytes_or_a_gap() {
        let stored =
            |idx: &[u64], size: usize| idx.iter().map(|i| (Index(*i), vec![*i as u8; size])).collect::<Vec<_>>();
        // A truncated log starting at 40: a read from 1 starts there.
        let p = log_page(stored(&[40, 41, 42, 43], 3), Index(1), Index(100), Index(42), 1 << 20);
        assert_eq!((p.first, p.rows.len(), p.commit), (Index(40), 3, Index(42)), "never past commit");
        let p = log_page(stored(&[40, 41, 42, 43], 3), Index(41), Index(41), Index(43), 1 << 20);
        assert_eq!((p.first, p.rows.len()), (Index(41), 1), "never past to");
        let p = log_page(stored(&[1, 2, 3], 10), Index(1), Index(9), Index(9), 15);
        assert_eq!(p.rows.len(), 1, "the byte bound, always one row");
        let p = log_page(stored(&[5, 6, 8, 9], 1), Index(5), Index(9), Index(9), 1 << 20);
        assert_eq!((p.first, p.rows.len()), (Index(5), 2), "stops at a gap");
        let p = log_page(stored(&[], 1), Index(7), Index(9), Index(5), 1 << 20);
        assert_eq!((p.first, p.rows.len(), p.clone().fetched()), (Index(6), 0, None));
        let mut p = log_page(stored(&[3, 4], 2), Index(3), Index(9), Index(9), 1 << 20);
        p.archive = Some((Index(1), Index(2), "archive/repo:1/x".into()));
        let b = encode_log_page(&p);
        assert_eq!(decode_log_page(&b), Ok(p.clone()));
        let mut bad = b.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(decode_log_page(&bad).is_err());
        assert_eq!(p.fetched(), Some(Fetched { first: Index(3), rows: vec![vec![3, 3], vec![4, 4]] }));
    }

    #[test]
    fn a_page_cut_short_by_the_member_links_on_and_the_hosted_rules_hold() {
        let rows = |a: u64, b: u64| (a..=b).map(|i| (Index(i), vec![i as u8])).collect::<Vec<_>>();
        // The member's page ended at 7 below commit 20: the client is sent on from 8.
        let (body, next) = entries_page(rows(5, 7), Index(5), 1_000, Index(1), false, Index(20)).unwrap();
        assert_eq!((unframe(&body).unwrap().len(), next), (3, Some(Index(8))));
        // At commit: the end.
        assert_eq!(entries_page(rows(5, 7), Index(5), 1_000, Index(1), false, Index(7)).unwrap().1, None);
        // A member that holds nothing at `from` (behind) gives an empty page with no link, not a loop.
        assert_eq!(entries_page(rows(1, 0), Index(5), 1_000, Index(1), false, Index(20)), Ok((Vec::new(), None)));
        assert!(serves_log(false, 3, 2));
        assert!(!serves_log(true, 3, 0));
        assert!(!serves_log(true, 1, 1));
        assert!(serves_log(true, 1, 0), "Solo");
        assert!(round_due(None, 0));
        assert!(!round_due(Some(100_000), 100_000 + FEED_EVERY_MS - 1_001));
        assert!(round_due(Some(100_000), 100_000 + FEED_EVERY_MS - 1_000));
        assert!(report_newer(None, Index(0)));
        assert!(report_newer(Some(Index(4)), Index(5)));
        assert!(!report_newer(Some(Index(5)), Index(5)));
        assert!(!report_newer(Some(Index(6)), Index(5)));
        // A prefetched page serves through the same rules as the model.
        let src = Prefetched {
            log: Some(StoredLog { first: Index(3), archived: false, to: Index(9), rows: rows(3, 4) }),
            ..Default::default()
        };
        let r = serve(&Route::Entries { chain: repo(1), from: Index(3), limit: 10 }, &|_| true, &src, 0, "https://h")
            .unwrap();
        assert_eq!(
            r.next.as_deref(),
            Some("<https://h/api/graftyard/v1/export/chains/repo:1/entries?from=5&limit=10>; rel=\"next\"")
        );
        let gone =
            serve(&Route::Entries { chain: repo(1), from: Index(2), limit: 10 }, &|_| true, &src, 0, "https://h");
        assert_eq!(gone.map(|_| ()).map_err(|e| e.status()), Err(410));
    }

    #[test]
    fn objects_and_frames_round_trip_and_a_flipped_byte_fails() {
        let o = ExportObject {
            batch: ChangeBatch {
                minute: 29_000_000,
                seq: 3,
                chains: vec![(repo(1), Index(12)), (ChainId::Tokens(TokenShard(7)), Index(1))],
            },
            entries: vec![
                (repo(1), Index(11), vec![b"eleven".to_vec(), Vec::new()]),
                (ChainId::Tokens(TokenShard(7)), Index(1), vec![vec![0xff; 300]]),
            ],
        };
        let bytes = encode_object(&o);
        assert_eq!(decode_object(&bytes), Ok(o.clone()));
        for at in [0, 3, 4, 40, bytes.len() - 1] {
            let mut bad = bytes.clone();
            bad[at] ^= 1;
            assert!(decode_object(&bad).is_err(), "byte {at}");
        }
        assert!(decode_object(&bytes[..bytes.len() - 1]).is_err());
        // A valid frame after the last chain's entries is an entry no chain claims.
        let mut extra = bytes.clone();
        frame(&mut extra, b"stray");
        assert!(decode_object(&extra).is_err());
        // An entry moved from one chain to the next changes the header, which its digest covers.
        let mut body = Vec::new();
        frame(&mut body, b"abc");
        frame(&mut body, b"");
        assert_eq!(unframe(&body), Ok(vec![b"abc".to_vec(), Vec::new()]));
        assert_eq!(body.len(), (4 + 3 + 32) + (4 + 32));
        assert_eq!(&body[..4], &[0, 0, 0, 3]);
    }

    #[test]
    fn export_keys_sort_as_feed_positions() {
        let ps = [(9, 10), (10, 2), (10, 11), (100, 0), (1_000_000, 4_000_000_000)];
        let keys: Vec<String> =
            ps.iter().map(|(m, s)| FeedPosition { minute: *m, seq: *s }.key().to_string()).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        for (k, (m, s)) in keys.iter().zip(ps) {
            assert_eq!(k.parse::<R2Key>(), Ok(R2Key::Export { minute: m, seq: s }));
        }
    }

    fn q(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn routes_parse_with_the_designs_errors() {
        let p = |m: &str, path: &str, query: &[(&str, &str)]| Route::parse(m, path, &q(query));
        assert_eq!(
            p("GET", "/api/graftyard/v1/export/batches", &[("after", "29000000.3")]),
            Ok(Route::Batches { after: FeedPosition { minute: 29_000_000, seq: 3 } })
        );
        assert_eq!(
            p("GET", "/api/graftyard/v1/export/batches", &[]),
            Ok(Route::Batches { after: FeedPosition::default() })
        );
        assert_eq!(
            p("GET", "/api/graftyard/v1/export/chains/tokens:7/entries", &[("from", "5"), ("limit", "20")]),
            Ok(Route::Entries { chain: ChainId::Tokens(TokenShard(7)), from: Index(5), limit: 20 })
        );
        assert_eq!(
            p("GET", "/api/graftyard/v1/export/chains/release/entries", &[("from", "1")]),
            Ok(Route::Entries { chain: ChainId::Release, from: Index(1), limit: ENTRIES_PAGE })
        );
        assert_eq!(
            p("GET", "/api/graftyard/v1/export/chains/repo:9/snapshots", &[("at", "400")]),
            Ok(Route::Snapshots { chain: repo(9), at: Index(400) })
        );
        let id = PackId(graftyard_types::Ulid(0x0190_0000_0000_0000_0000_0000_0000_0042));
        assert_eq!(
            p("GET", &format!("/api/graftyard/v1/export/packs/{id}"), &[("network", "77")]),
            Ok(Route::Pack { network: NetworkId(DbId(77)), id })
        );
        assert_eq!(p("DELETE", "/api/graftyard/v1/export/token", &[]), Ok(Route::RevokeToken));
        for (path, query) in [
            ("/api/graftyard/v1/export/batches", &[("after", "3")][..]),
            ("/api/graftyard/v1/export/batches", &[("after", "3.-1")]),
            ("/api/graftyard/v1/export/batches", &[("after", "3.99999999999")]),
            ("/api/graftyard/v1/export/chains/repo:9/entries", &[]),
            ("/api/graftyard/v1/export/chains/repo:9/entries", &[("from", "0")]),
            ("/api/graftyard/v1/export/chains/repo:9/entries", &[("from", "+5")]),
            ("/api/graftyard/v1/export/chains/repo:9/entries", &[("from", "5"), ("limit", "1001")]),
            ("/api/graftyard/v1/export/chains/repo:9/entries", &[("from", "5"), ("limit", "0")]),
            ("/api/graftyard/v1/export/chains/nosuch:9/entries", &[("from", "5")]),
            ("/api/graftyard/v1/export/chains/repo:9/snapshots", &[]),
            ("/api/graftyard/v1/export/packs/not-a-ulid", &[("network", "77")]),
            ("/api/graftyard/v1/export/packs/01J0000000000000000000000A", &[]),
            ("/api/graftyard/v1/export/packs/01J0000000000000000000000A", &[("network", "-1")]),
        ] {
            assert_eq!(p("GET", path, query).map_err(|e| e.status()), Err(422), "{path} {query:?}");
        }
        assert_eq!(p("GET", "/api/graftyard/v1/export/chains/repo:9", &[]), Err(ExportError::NotFound));
        assert_eq!(p("GET", "/api/graftyard/v1/exports/batches", &[]), Err(ExportError::NotFound));
        assert_eq!(p("GET", "/api/graftyard/v1/export/token", &[]), Err(ExportError::Method));
        assert_eq!(p("DELETE", "/api/graftyard/v1/export/batches", &[]), Err(ExportError::Method));
    }

    fn stored(from: u64, n: u64, size: usize) -> Vec<(Index, Vec<u8>)> {
        (from..from + n).map(|i| (Index(i), vec![i as u8; size])).collect()
    }

    #[test]
    fn entries_pages_stop_at_the_limit_the_byte_cap_and_the_commit_index() {
        let (body, next) = entries_page(stored(5, 30, 10), Index(5), 20, Index(1), false, Index(100)).unwrap();
        let rows = unframe(&body).unwrap();
        assert_eq!((rows.len(), next), (20, Some(Index(25))));
        assert_eq!(rows[0], vec![5u8; 10]);
        // Entries past the commit index are never served.
        let (body, next) = entries_page(stored(5, 30, 10), Index(5), 1_000, Index(1), false, Index(9)).unwrap();
        assert_eq!((unframe(&body).unwrap().len(), next), (5, None));
        // 3 MiB entries: two fit in 8 MiB, the third starts the next page; one larger than the cap still goes alone.
        let (body, next) = entries_page(stored(1, 3, 3 << 20), Index(1), 1_000, Index(1), false, Index(3)).unwrap();
        assert_eq!((unframe(&body).unwrap().len(), next), (2, Some(Index(3))));
        let (body, next) = entries_page(stored(1, 2, 9 << 20), Index(1), 1_000, Index(1), false, Index(2)).unwrap();
        assert_eq!((unframe(&body).unwrap().len(), next), (1, Some(Index(2))));
        assert_eq!(
            entries_page(stored(5, 3, 1), Index(5), 10, Index(6), false, Index(7)).map(|_| ()).map_err(|e| e.status()),
            Err(410)
        );
        assert!(
            entries_page(stored(6, 3, 1), Index(5), 10, Index(6), true, Index(8)).is_err(),
            "a gap is never served"
        );
        assert_eq!(entries_page(stored(5, 3, 1), Index(9), 10, Index(1), false, Index(7)), Ok((Vec::new(), None)));
    }

    /// An archive range 10..=14 read in chunks that split the header and rows: only `from..=to` comes back, in
    /// order, at most `limit`, and a bad header is refused.
    #[test]
    fn archive_rows_keep_the_asked_range_across_chunks() {
        let mut obj = graftyard_fold::archive_header();
        for i in 10u8..=14 {
            graftyard_fold::encode_archive_row(&vec![i; i as usize], &mut obj);
        }
        let read = |from, to, limit, step: usize| {
            let mut r = ArchiveRows::new(Index(10), Index(from), Index(to), limit);
            for c in obj.chunks(step) {
                if r.push(c).unwrap() {
                    break;
                }
            }
            r.rows.into_iter().map(|(i, row)| (i.0, row[0], row.len())).collect::<Vec<_>>()
        };
        assert_eq!(read(12, 13, 10, 3), vec![(12, 12, 12), (13, 13, 13)]);
        assert_eq!(read(11, 99, 2, 7), vec![(11, 11, 11), (12, 12, 12)]);
        assert_eq!(read(10, 14, 10, 1000), (10..=14).map(|i| (i, i as u8, i as usize)).collect::<Vec<_>>());
        let mut bad = ArchiveRows::new(Index(1), Index(1), Index(9), 10);
        assert!(bad.push(&[0u8; 32]).is_err());
    }

    #[test]
    fn snapshot_at_takes_the_newest_at_or_below_core_first() {
        let snap = |chain, tier, to| R2Key::Snapshot {
            chain,
            tier,
            from: Index(0),
            to: Index(to),
            digest: Digest([to as u8; 32]),
        };
        let keys = vec![
            snap(repo(1), Tier::Metadata, 200),
            snap(repo(1), Tier::Core, 200),
            snap(repo(1), Tier::Core, 100),
            snap(repo(1), Tier::Core, 300),
            snap(repo(2), Tier::Core, 250),
            R2Key::Pack { network: NetworkId(DbId(1)), id: PackId(graftyard_types::Ulid(1)) },
        ];
        assert_eq!(
            snapshot_at(&keys, repo(1), Index(299)),
            Some(vec![snap(repo(1), Tier::Core, 200), snap(repo(1), Tier::Metadata, 200)])
        );
        assert_eq!(snapshot_at(&keys, repo(1), Index(300)), Some(vec![snap(repo(1), Tier::Core, 300)]));
        assert_eq!(snapshot_at(&keys, repo(1), Index(99)), None);
    }
}
