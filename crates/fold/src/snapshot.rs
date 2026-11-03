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
    chain: ChainId,
    schema: Schema,
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
        let at = SnapAt { to: m.applied, term: m.term, entry: m.last };
        m.snap = Some(at);
        save_meta(store, &m);
        Ok(SnapshotJob {
            chain,
            schema,
            at,
            release,
            format,
            table: 0,
            after: None,
            sum: [0; 32],
            rows: 0,
            done: false,
        })
    }

    pub fn to(&self) -> Index {
        self.at.to
    }

    /// The next frame, with at most `max_rows` rows, read from the store as it stands now; `None` once the trailer has
    /// been returned. Each call fits one request's limits, so a host runs one per alarm while `apply` goes on.
    pub fn next(&mut self, store: &dyn Store, max_rows: usize) -> Option<SnapshotFrame> {
        if self.done {
            return None;
        }
        let max_rows = max_rows.max(1);
        while let Some(t) = self.schema.tables.get(self.table) {
            let rows = self.read(store, t.name, max_rows);
            if rows.is_empty() {
                self.table += 1;
                self.after = None;
                continue;
            }
            for r in &rows {
                add(&mut self.sum, &row_hash(t.name, &r.key, &Stored { written: r.written, cols: r.cols.clone() }));
            }
            self.rows += rows.len() as u64;
            self.after = rows.last().map(|r| r.key.clone());
            return Some(SnapshotFrame::Rows { table: t.name.into(), rows });
        }
        self.done = true;
        let d = TierDigest { tier: self.schema.tier, applied: self.at.to, term: self.at.term, last: self.at.entry };
        Some(SnapshotFrame::End(SnapshotTrailer {
            chain: self.chain,
            tier: self.schema.tier,
            to: self.at.to,
            term: self.at.term,
            entry: self.at.entry,
            fold_digest: digest_of(&d, &self.sum),
            release: self.release,
            format: self.format,
            rows: self.rows,
        }))
    }

    /// Up to `max` rows of `table` as of `to`, after `self.after`, merging the live table and its undo copies.
    fn read(&self, store: &dyn Store, table: &str, max: usize) -> Vec<Row> {
        let undo = undo_table(table);
        let from = self.after.clone().unwrap_or_default();
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
            let mut out: BTreeMap<Key, Stored> = live.into_iter().filter(|(_, s)| s.written <= self.at.to).collect();
            out.extend(copies);
            if let Some(a) = &self.after {
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
        let undo = undo_table(t.name);
        loop {
            let rows = store.scan(&undo, &[], 512);
            if rows.is_empty() {
                break;
            }
            for (k, _) in rows {
                store.delete(&undo, &k);
            }
        }
    }
    let mut m = meta(store);
    m.snap = None;
    save_meta(store, &m);
}

/// The bytes of a frame stream: the magic, the frame format, then each frame as a `u32` length and its postcard bytes.
pub fn encode_frames(frames: &[SnapshotFrame]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FRAME_FORMAT.to_be_bytes());
    for f in frames {
        let b = postcard::to_allocvec(f).expect("frames encode");
        out.extend_from_slice(&(b.len() as u32).to_be_bytes());
        out.extend_from_slice(&b);
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
    if store.get(META, &[]).is_some() || schema.tables.iter().any(|t| !store.scan(t.name, &[], 1).is_empty()) {
        return Err(SnapshotError::NotEmpty);
    }
    let Some(SnapshotFrame::End(tr)) = frames.last() else {
        return Err(SnapshotError::Malformed("no trailer".into()));
    };
    if (tr.chain, tr.tier, tr.to) != (want.0, schema.tier, want.1) {
        return Err(SnapshotError::Wrong {
            got: format!("{} {:?} at {:?}", tr.chain, tr.tier, tr.to),
            want: format!("{} {:?} at {:?}", want.0, schema.tier, want.1),
        });
    }
    let mut count = 0u64;
    let mut sum = [0u8; 32];
    let mut last: Option<(&str, &Key)> = None;
    for f in &frames[..frames.len() - 1] {
        let SnapshotFrame::Rows { table, rows } = f else {
            return Err(SnapshotError::Malformed("a trailer before the end".into()));
        };
        let def =
            schema.tables.iter().find(|t| t.name == &**table).ok_or_else(|| SnapshotError::Table(table.clone()))?;
        for r in rows {
            if let Some((t, k)) = last
                && t == def.name
                && *k >= r.key
            {
                return Err(SnapshotError::Order(table.clone()));
            }
            last = Some((def.name, &r.key));
            let row = Stored { written: r.written, cols: r.cols.clone() };
            add(&mut sum, &row_hash(def.name, &r.key, &row));
            store.put(def.name, r.key.clone(), row);
            count += 1;
        }
    }
    if count != tr.rows {
        return Err(SnapshotError::Rows { got: count, want: tr.rows });
    }
    save_meta(store, &crate::tier::Meta { applied: tr.to, term: tr.term, last: tr.entry, sum, snap: None });
    let got = refold_digest(store, schema);
    if got != tr.fold_digest {
        return Err(SnapshotError::Digest { got, want: tr.fold_digest });
    }
    Ok(tr.clone())
}
