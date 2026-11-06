//! Tier snapshots (`design/DESIGN.md`, Shape, Packs): the writer's choice and cadence, `snap_undo`, the frames, their
//! encoding, restore, and which snapshots `RESTORE_WINDOW` keeps.
//!
//! A snapshot is taken at one index, `to`, without pausing `apply`. `SnapshotJob::begin` records `to` in the tier's
//! meta row; from then on `apply` copies a row's prior value into its table's undo table the first time it changes or
//! deletes a row last written at or below `to`. `SnapshotJob::next` reads each table in key order in batches, merging
//! the table with its undo table: an undo copy wins, a row written after `to` with no copy is skipped, and an undo copy
//! whose row was deleted is still emitted. The last frame's trailer carries the digest of the rows emitted, which
//! equals `fold_digest` of the tier at `to`. `end_snapshot` drops the undo tables once the leader commits `Archived`
//! for the snapshot (or the writer gives it up).

use std::collections::BTreeMap;
use std::time::Duration;

use graftyard_raft::Term;
use graftyard_types::{ChainId, Digest, Index, MemberKind, Placement, ReleaseId, SiteId, Tier, Timestamp};
use serde::{Deserialize, Serialize};

use crate::store::{Key, META, Schema, Store, Stored, Val, undo_table};
use crate::tier::{SnapAt, TierDigest, add, digest_of, meta, refold_digest, row_hash, save_meta};

/// Entries between snapshots of a tier [guess].
pub const SNAPSHOT_EVERY: u64 = 100_000;
/// Log bytes between snapshots of a tier [guess].
pub const SNAPSHOT_BYTES: u64 = 256 << 20;
/// Each tier keeps a snapshot at or below every index inside this window.
pub const RESTORE_WINDOW: Duration = Duration::from_secs(35 * 86_400);

const DAY_MS: u64 = 86_400_000;
/// The first bytes of every encoded frame stream, then a format version.
const MAGIC: &[u8; 8] = b"gyfsnap\0";
const FRAME_FORMAT: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub key: Key,
    pub written: Index,
    pub cols: Vec<Val>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotFrame {
    Rows { table: Box<str>, rows: Vec<Row> },
    End(SnapshotTrailer),
}

/// `entry` is the digest of entry `to`, which entry `to + 1` names as `prev`: not in the design's trailer, which a
/// restored tier needs to refuse a wrong `prev` on the next entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotTrailer {
    pub chain: ChainId,
    pub tier: Tier,
    pub to: Index,
    pub term: Term,
    pub entry: Digest,
    /// Entry `to`'s time. Not in the design's trailer: a restored tier needs it for snapshot cadence and retention.
    pub at: Timestamp,
    pub fold_digest: Digest,
    pub release: ReleaseId,
    pub format: graftyard_raft::Format,
    pub rows: u64,
}

/// The member that writes the snapshot at `to`: the Full follower at position `to / SNAPSHOT_EVERY` modulo the
/// number of Full followers, in config order, and for a one-voter (`Solo`) placement the member itself. `None` when
/// the placement has no Full follower.
pub fn snapshot_writer(to: Index, placement: &Placement, leader: SiteId) -> Option<SiteId> {
    let voters = placement.voters();
    if let [solo] = voters {
        return Some(solo.site);
    }
    let full: Vec<SiteId> =
        voters.iter().filter(|m| m.kind == MemberKind::Full && m.site != leader).map(|m| m.site).collect();
    if full.is_empty() {
        return None;
    }
    Some(full[((to.0 / SNAPSHOT_EVERY) % full.len() as u64) as usize])
}

/// Whether a tier is due a snapshot at `applied`, whose entry has time `at`: SNAPSHOT_EVERY entries or SNAPSHOT_BYTES
/// of log since the last one, or a day boundary (UTC) passed since it while the group committed. Times are entries'
/// `at`, never a clock, so every member answers the same.
pub fn snapshot_due(last: Option<(Index, Timestamp)>, applied: Index, at: Timestamp, log_bytes_since: u64) -> bool {
    let Some((to, to_at)) = last else { return applied > Index(0) };
    if applied <= to {
        return false;
    }
    applied.0 - to.0 >= SNAPSHOT_EVERY || log_bytes_since >= SNAPSHOT_BYTES || at.0 / DAY_MS > to_at.0 / DAY_MS
}

/// The log a member keeps, beside the 30-day rule (`design/DESIGN.md`, Shape, `LOG_MAX_BYTES`).
pub const LOG_MAX_BYTES: u64 = 2 << 30;
/// The age past which the log is archived whatever its size (`design/DESIGN.md`, Shape, Packs: 30 days of log).
pub const LOG_KEEP: Duration = Duration::from_secs(30 * 86_400);

/// The range the leader archives next, `from..=through`, given the stored log above the last archived range as
/// `(index, at, encoded bytes)` in index order, the newest entry's time `now`, and whether both tiers have a snapshot
/// (`snapshotted`): every entry older than LOG_KEEP, and then the oldest entries until the rest fits LOG_MAX_BYTES.
/// Only committed entries, and only once a snapshot exists, so an install can start from the newest snapshot at or
/// below the archive boundary and read the archives after it (a range below every snapshot is still archived, and an
/// install past it starts from the snapshot). `None` when nothing is due. Times are entries' `at`, so every member
/// answers the same.
pub fn archive_due(
    log: &[(Index, Timestamp, u64)],
    now: Timestamp,
    snapshotted: bool,
    committed: Index,
    max_bytes: u64,
) -> Option<(Index, Index)> {
    let first = log.first()?.0;
    if !snapshotted {
        return None;
    }
    let cut = now.0.saturating_sub(LOG_KEEP.as_millis() as u64);
    let mut over = log.iter().map(|e| e.2).sum::<u64>().saturating_sub(max_bytes);
    let mut through = None;
    for (i, at, bytes) in log {
        if at.0 >= cut && over == 0 {
            break;
        }
        if *i > committed {
            break;
        }
        over = over.saturating_sub(*bytes);
        through = Some(*i);
    }
    through.map(|t| (first, t))
}

/// Of a tier's committed snapshots as `(to, to_at)`, those to keep when the newest entry has time `now`: every
/// snapshot inside `RESTORE_WINDOW`, and the newest one before it, which is at or below the window's first index.
pub fn snapshots_to_keep(snaps: &[(Index, Timestamp)], now: Timestamp) -> Vec<Index> {
    let cutoff = now.0.saturating_sub(RESTORE_WINDOW.as_millis() as u64);
    let mut keep: Vec<Index> = snaps.iter().filter(|(_, t)| t.0 >= cutoff).map(|(i, _)| *i).collect();
    if let Some((i, _)) = snaps.iter().filter(|(_, t)| t.0 < cutoff).max_by_key(|(i, _)| *i) {
        keep.push(*i);
    }
    keep.sort();
    keep.dedup();
    keep
}

/// A snapshot being written: which table it is in, the last key it emitted, and the hash sum of rows so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotJob {
    schema: Schema,
    cursor: SnapshotCursor,
}

/// Where a `SnapshotJob` stands, which a host stores at a part boundary and resumes from after its object restarts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotCursor {
    chain: ChainId,
    at: SnapAt,
    release: ReleaseId,
    format: graftyard_raft::Format,
    table: usize,
    after: Option<Key>,
    sum: [u8; 32],
    rows: u64,
    done: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    #[error("a snapshot of this tier is already running at {0:?}")]
    Running(Index),
    #[error("not a snapshot frame stream: {0}")]
    Malformed(String),
    #[error("snapshot frame format {0} is not one this release reads")]
    Format(u16),
    #[error("the snapshot is of {got}, not {want}")]
    Wrong { got: String, want: String },
    #[error("table {0} is not in the tier's schema")]
    Table(Box<str>),
    #[error("rows out of key order in table {0}")]
    Order(Box<str>),
    #[error("the trailer counts {want} rows, the frames hold {got}")]
    Rows { got: u64, want: u64 },
    #[error("the rows digest to {got:?}, the trailer says {want:?}")]
    Digest { got: Digest, want: Digest },
    #[error("restore needs an empty tier")]
    NotEmpty,
}

impl SnapshotJob {
    /// Starts a snapshot of the tier at its `applied` index. From here `apply` keeps `snap_undo` copies.
    pub fn begin(
        store: &mut dyn Store,
        schema: Schema,
        chain: ChainId,
        release: ReleaseId,
        format: graftyard_raft::Format,
    ) -> Result<SnapshotJob, SnapshotError> {
        let mut m = meta(store);
        if let Some(s) = m.snap {
            return Err(SnapshotError::Running(s.to));
        }
        let at = SnapAt { to: m.applied, term: m.term, entry: m.last, at: m.at };
        m.snap = Some(at);
        m.snap_sum = m.sum;
        save_meta(store, &m);
        let cursor =
            SnapshotCursor { chain, at, release, format, table: 0, after: None, sum: [0; 32], rows: 0, done: false };
        Ok(SnapshotJob { schema, cursor })
    }

    /// A job from a stored cursor, over the tier whose meta row still names its `to`.
    pub fn resume(schema: Schema, cursor: SnapshotCursor) -> SnapshotJob {
        SnapshotJob { schema, cursor }
    }

    pub fn cursor(&self) -> &SnapshotCursor {
        &self.cursor
    }

    pub fn to(&self) -> Index {
        self.cursor.at.to
    }

    /// Entry `to`'s time.
    pub fn to_at(&self) -> Timestamp {
        self.cursor.at.at
    }

    /// Whether the trailer has been returned.
    pub fn done(&self) -> bool {
        self.cursor.done
    }

    /// The tier's `fold_digest` at `to`, from the sum `apply` kept when the job began: what the trailer must carry,
    /// so a host can name the snapshot's key before it reads a row.
    pub fn want(&self, store: &dyn Store) -> Digest {
        let m = meta(store);
        assert_eq!(m.snap, Some(self.cursor.at), "the tier's running snapshot is this job's");
        let a = self.cursor.at;
        digest_of(&TierDigest { tier: self.schema.tier, applied: a.to, term: a.term, last: a.entry }, &m.snap_sum)
    }

    /// The next frame, with at most `max_rows` rows, read from the store as it stands now; `None` once the trailer has
    /// been returned. Each call fits one request's limits, so a host runs one per alarm while `apply` goes on.
    pub fn next(&mut self, store: &dyn Store, max_rows: usize) -> Option<SnapshotFrame> {
        if self.cursor.done {
            return None;
        }
        let max_rows = max_rows.max(1);
        while let Some(t) = self.schema.tables.get(self.cursor.table) {
            let rows = self.read(store, t.name, max_rows);
            if rows.is_empty() {
                self.cursor.table += 1;
                self.cursor.after = None;
                continue;
            }
            for r in &rows {
                add(
                    &mut self.cursor.sum,
                    &row_hash(t.name, &r.key, &Stored { written: r.written, cols: r.cols.clone() }),
                );
            }
            self.cursor.rows += rows.len() as u64;
            self.cursor.after = rows.last().map(|r| r.key.clone());
            return Some(SnapshotFrame::Rows { table: t.name.into(), rows });
        }
        self.cursor.done = true;
        let d = TierDigest {
            tier: self.schema.tier,
            applied: self.cursor.at.to,
            term: self.cursor.at.term,
            last: self.cursor.at.entry,
        };
        Some(SnapshotFrame::End(SnapshotTrailer {
            chain: self.cursor.chain,
            tier: self.schema.tier,
            to: self.cursor.at.to,
            term: self.cursor.at.term,
            entry: self.cursor.at.entry,
            at: self.cursor.at.at,
            fold_digest: digest_of(&d, &self.cursor.sum),
            release: self.cursor.release,
            format: self.cursor.format,
            rows: self.cursor.rows,
        }))
    }

    /// Up to `max` rows of `table` as of `to`, after `self.cursor.after`, merging the live table and its undo copies.
    fn read(&self, store: &dyn Store, table: &str, max: usize) -> Vec<Row> {
        let undo = undo_table(table);
        let from = self.cursor.after.clone().unwrap_or_default();
        // Each side is read `batch` rows at a time. A side that filled its batch is known only up to its last key, so
        // rows are final below the lower such key; read more until `max` final rows exist or both sides ran out.
        let mut batch = max + 1;
        loop {
            let live = store.scan(table, &from, batch);
            let copies = store.scan(&undo, &from, batch);
            let end = |side: &[(Key, Stored)]| (side.len() == batch).then(|| side[batch - 1].0.clone());
            let cut = match (end(&live), end(&copies)) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let mut out: BTreeMap<Key, Stored> =
                live.into_iter().filter(|(_, s)| s.written <= self.cursor.at.to).collect();
            out.extend(copies);
            if let Some(a) = &self.cursor.after {
                out.remove(a);
            }
            let rows: Vec<Row> = out
                .into_iter()
                .filter(|(k, _)| cut.as_ref().is_none_or(|c| k <= c))
                .take(max)
                .map(|(key, s)| Row { key, written: s.written, cols: s.cols })
                .collect();
            if rows.len() == max || cut.is_none() {
                return rows;
            }
            batch *= 2;
        }
    }
}

/// Drops every undo copy and the running snapshot's mark. The writer calls it once the leader has committed
/// `Archived` for the snapshot, or when it abandons one.
pub fn end_snapshot(store: &mut dyn Store, schema: &Schema) {
    for t in schema.tables {
        store.clear(&undo_table(t.name));
    }
    let mut m = meta(store);
    m.snap = None;
    m.snap_sum = [0; 32];
    save_meta(store, &m);
}

/// The bytes of a frame stream: the magic, the frame format, then each frame as a `u32` length and its postcard bytes.
pub fn encode_frames(frames: &[SnapshotFrame]) -> Vec<u8> {
    let mut out = frames_header();
    for f in frames {
        encode_frame(f, &mut out);
    }
    out
}

/// Parses a frame stream: exactly one trailer, and it comes last.
pub fn decode_frames(mut b: &[u8]) -> Result<Vec<SnapshotFrame>, SnapshotError> {
    let bad = |s: &str| SnapshotError::Malformed(s.into());
    if b.len() < 10 || &b[..8] != MAGIC {
        return Err(bad("magic"));
    }
    let v = u16::from_be_bytes([b[8], b[9]]);
    if v != FRAME_FORMAT {
        return Err(SnapshotError::Format(v));
    }
    b = &b[10..];
    let mut out = Vec::new();
    while !b.is_empty() {
        if b.len() < 4 {
            return Err(bad("truncated length"));
        }
        let n = u32::from_be_bytes(b[..4].try_into().expect("4")) as usize;
        b = &b[4..];
        if b.len() < n {
            return Err(bad("truncated frame"));
        }
        if matches!(out.last(), Some(SnapshotFrame::End(_))) {
            return Err(bad("frames after the trailer"));
        }
        out.push(postcard::from_bytes(&b[..n]).map_err(|e| SnapshotError::Malformed(e.to_string()))?);
        b = &b[n..];
    }
    if !matches!(out.last(), Some(SnapshotFrame::End(_))) {
        return Err(bad("no trailer"));
    }
    Ok(out)
}

/// Checks a frame stream as it arrives, without holding it: what `restore` checks, for a reader that cannot hold a
/// whole tier (`tests/m2`'s verifier streams a 4 GB snapshot through it). Each table of the schema at most once, in
/// schema order, rows in key order, the row count, and the digest of the rows against the trailer.
#[derive(Clone, Debug)]
pub struct FrameCheck {
    schema: Schema,
    table: usize,
    last: Option<Key>,
    sum: [u8; 32],
    rows: u64,
    trailer: Option<SnapshotTrailer>,
}

impl FrameCheck {
    pub fn new(schema: Schema) -> FrameCheck {
        FrameCheck { schema, table: 0, last: None, sum: [0; 32], rows: 0, trailer: None }
    }

    pub fn frame(&mut self, f: &SnapshotFrame) -> Result<(), SnapshotError> {
        if self.trailer.is_some() {
            return Err(SnapshotError::Malformed("frames after the trailer".into()));
        }
        match f {
            SnapshotFrame::Rows { table, rows } => {
                let at = self.schema.tables.iter().position(|t| t.name == &**table);
                let at = at.ok_or_else(|| SnapshotError::Table(table.clone()))?;
                if at < self.table {
                    return Err(SnapshotError::Order(table.clone()));
                }
                if at > self.table {
                    (self.table, self.last) = (at, None);
                }
                for r in rows {
                    if self.last.as_ref().is_some_and(|l| *l >= r.key) {
                        return Err(SnapshotError::Order(table.clone()));
                    }
                    add(&mut self.sum, &row_hash(table, &r.key, &Stored { written: r.written, cols: r.cols.clone() }));
                    self.last = Some(r.key.clone());
                    self.rows += 1;
                }
                Ok(())
            }
            SnapshotFrame::End(tr) => {
                if tr.tier != self.schema.tier {
                    return Err(SnapshotError::Wrong {
                        got: format!("{:?}", tr.tier),
                        want: format!("{:?}", self.schema.tier),
                    });
                }
                self.trailer = Some(tr.clone());
                Ok(())
            }
        }
    }

    /// The trailer, once the rows' count and digest match it.
    pub fn finish(self) -> Result<SnapshotTrailer, SnapshotError> {
        let tr = self.trailer.ok_or_else(|| SnapshotError::Malformed("no trailer".into()))?;
        if self.rows != tr.rows {
            return Err(SnapshotError::Rows { got: self.rows, want: tr.rows });
        }
        let got = digest_of(&TierDigest { tier: tr.tier, applied: tr.to, term: tr.term, last: tr.entry }, &self.sum);
        if got != tr.fold_digest {
            return Err(SnapshotError::Digest { got, want: tr.fold_digest });
        }
        Ok(tr)
    }
}

/// The frame stream's header, then each frame's bytes as `encode_frames` writes them, so a writer can emit a stream
/// one frame at a time.
pub fn frames_header() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&FRAME_FORMAT.to_be_bytes());
    out
}

pub fn encode_frame(f: &SnapshotFrame, out: &mut Vec<u8>) {
    let b = postcard::to_allocvec(f).expect("frames encode");
    out.extend_from_slice(&(b.len() as u32).to_be_bytes());
    out.extend_from_slice(&b);
}

/// Splits complete frames off the front of `buf`, after its header has been taken off: the frames, and how many
/// bytes they used.
pub fn decode_some(buf: &[u8]) -> Result<(Vec<SnapshotFrame>, usize), SnapshotError> {
    let mut at = 0;
    let mut out = Vec::new();
    while buf.len() >= at + 4 {
        let n = u32::from_be_bytes(buf[at..at + 4].try_into().expect("4")) as usize;
        if buf.len() < at + 4 + n {
            break;
        }
        out.push(postcard::from_bytes(&buf[at + 4..at + 4 + n]).map_err(|e| SnapshotError::Malformed(e.to_string()))?);
        at += 4 + n;
    }
    Ok((out, at))
}

/// Installs a snapshot into an empty tier: the rows, then a meta row at the trailer's `to` that chains to its `entry`.
/// Refuses a snapshot of another chain, tier, or index than `want`, a table outside the schema, rows out of order,
/// a row count or a digest that does not match the trailer. Returns the trailer. A refused restore may leave rows
/// behind, so a host restores into a fresh store and drops it on an error.
pub fn restore(
    store: &mut dyn Store,
    schema: &Schema,
    frames: &[SnapshotFrame],
    want: (ChainId, Index),
) -> Result<SnapshotTrailer, SnapshotError> {
    let mut r = Restore::begin(store, *schema, want)?;
    for f in frames {
        r.frame(store, f)?;
    }
    r.finish(store)
}

/// `restore` one frame at a time, so a host loads a snapshot of any size into its own tables while it holds one
/// frame: an `InstallSnapshot` from R2, or a fold audit's refold (`refold_from`).
pub struct Restore {
    want: (ChainId, Index),
    check: FrameCheck,
}

impl Restore {
    /// Refuses a store that already holds the tier.
    pub fn begin(store: &dyn Store, schema: Schema, want: (ChainId, Index)) -> Result<Restore, SnapshotError> {
        if store.get(META, &[]).is_some() || schema.tables.iter().any(|t| !store.scan(t.name, &[], 1).is_empty()) {
            return Err(SnapshotError::NotEmpty);
        }
        Ok(Restore { want, check: FrameCheck::new(schema) })
    }

    pub fn frame(&mut self, store: &mut dyn Store, f: &SnapshotFrame) -> Result<(), SnapshotError> {
        self.check.frame(f)?;
        if let SnapshotFrame::Rows { table, rows } = f {
            let def = self.check.schema.tables.iter().find(|t| t.name == &**table).expect("checked");
            for r in rows {
                store.put(def.name, r.key.clone(), Stored { written: r.written, cols: r.cols.clone() });
            }
        }
        Ok(())
    }

    /// Where the restore stands, which a host stores with each batch's rows and resumes from (`resume`), so a load
    /// that spans many requests survives its object's reset.
    pub fn cursor(&self) -> RestoreCursor {
        let c = &self.check;
        RestoreCursor {
            want: self.want,
            table: c.table,
            last: c.last.clone(),
            sum: c.sum,
            rows: c.rows,
            trailer: c.trailer.clone(),
        }
    }

    pub fn resume(schema: Schema, c: RestoreCursor) -> Restore {
        let check = FrameCheck { schema, table: c.table, last: c.last, sum: c.sum, rows: c.rows, trailer: c.trailer };
        Restore { want: c.want, check }
    }

    /// Checks the trailer, writes the tier's meta row at `to`, and refolds the stored rows against the trailer's
    /// digest, a page at a time.
    pub fn finish(self, store: &mut dyn Store) -> Result<SnapshotTrailer, SnapshotError> {
        let schema = self.check.schema;
        let tr = self.finish_rows(store)?;
        let got = refold_digest(store, &schema);
        if got != tr.fold_digest {
            return Err(SnapshotError::Digest { got, want: tr.fold_digest });
        }
        Ok(tr)
    }

    /// `finish` without the refold of the stored rows: the rows the frames carried are checked against the trailer,
    /// not the rows the store holds. For a host whose tier is too large to refold in one request; it refolds the store
    /// in pages itself, or trusts its own writes.
    pub fn finish_rows(self, store: &mut dyn Store) -> Result<SnapshotTrailer, SnapshotError> {
        let schema = self.check.schema;
        let sum = self.check.sum;
        let tr = self.check.finish()?;
        if (tr.chain, tr.to) != self.want {
            return Err(SnapshotError::Wrong {
                got: format!("{} {:?} at {:?}", tr.chain, tr.tier, tr.to),
                want: format!("{} {:?} at {:?}", self.want.0, schema.tier, self.want.1),
            });
        }
        save_meta(
            store,
            &crate::tier::Meta {
                applied: tr.to,
                term: tr.term,
                at: tr.at,
                last: tr.entry,
                sum,
                snap: None,
                snap_sum: [0; 32],
            },
        );
        Ok(tr)
    }
}

/// A `Restore` between batches: what its `FrameCheck` has seen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreCursor {
    want: (ChainId, Index),
    table: usize,
    last: Option<Key>,
    sum: [u8; 32],
    rows: u64,
    trailer: Option<SnapshotTrailer>,
}

/// Why `refold_segment` gave no digest.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RefoldError {
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    #[error(transparent)]
    Apply(#[from] crate::tier::FoldError),
}

/// A fold audit's refold of one tier that holds no more than one frame and one entry at a time (F48): the snapshot at
/// `from` restored into `store`, which must be empty (a host's scratch SQLite tables), then `entries` applied in
/// order. Returns the tier's `fold_digest` after the last entry, which the audit compares with each member's
/// `digest_at` of the same index, or with the next snapshot's trailer when the entries end at its `to`. Chaining
/// segments from one snapshot to the next refolds a chain from its first snapshot whatever its size.
pub fn refold_segment<T: crate::tier::Tiered>(
    t: &T,
    store: &mut dyn Store,
    schema: Schema,
    from: (ChainId, Index),
    frames: impl IntoIterator<Item = SnapshotFrame>,
    entries: impl IntoIterator<Item = graftyard_raft::LogEntry<T::Event>>,
) -> Result<Digest, RefoldError> {
    let mut r = Restore::begin(store, schema, from)?;
    for f in frames {
        r.frame(store, &f)?;
    }
    r.finish(store)?;
    for e in entries {
        crate::tier::apply(t, store, schema.tier, &e)?;
    }
    Ok(crate::tier::fold_digest(store, schema.tier))
}
