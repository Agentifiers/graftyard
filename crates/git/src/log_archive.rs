//! The leader's `LOG_MAX_BYTES` archive, its pure half (`design/DESIGN.md`, Shape, `LOG_MAX_BYTES` and "Packs";
//! `build/BUILD-PLAN.md`, AM1-WP1 and AM1-WP6). The site's alarm (`workers/site/src/archive.rs`) reads the stored log,
//! asks `due` for the next range, promotes the range's packs first when no `Promoted` covers any of it
//! (`next`, `promotion_sums`), puts `archive_object`'s bytes in two buckets, and records `Archived`. Members
//! truncate their stored log once they can HEAD the key in two buckets (`Host::truncate`), and an installing member
//! or the fold audit reads the entries back from the object.
//!
//! `verify` refuses an `Archived` past the last `Promoted` (`crate::repo`, `check_archived`), so every range is
//! promoted first, even one whose entries name no pack: its promotion is an empty pack.

use graftyard_fold::{archive_due_walk, archive_header, encode_archive_row};
use graftyard_raft::{LogEntry, codec};
use graftyard_types::{ChainId, Digest, Index, RequestId, Timestamp, Ulid};
use sha2::{Digest as _, Sha256};

use crate::key::R2Key;
use crate::pack::{Archived, PackRef, Promoted};
use crate::repo::{RepoEvent, packs_of};

/// The most stored log bytes one archive object takes [guess]. The site builds the object in memory beside the
/// member's state, and a 4 MB batch of a large member's log took its isolate past 128 MB (`workers/site/src/load.rs`,
/// `BATCH_BYTES`), so a due range goes in pieces of this size; the alarm runs again at once after each piece.
pub const ARCHIVE_RANGE_MAX: u64 = 2 << 20;
/// The most pack bytes one promotion takes [guess]: the shard's complete pack comes back whole into the site's
/// memory before it is placed, and is about the size of the packs it packs. A range whose packs name more is not
/// promoted, and the alarm reports it, rather than ending the isolate.
pub const PROMOTE_MAX: u64 = 48 << 20;

/// Once the log passes the cap, the archive takes it down to the cap less this share of it [guess]. Without the
/// slack a log held at the cap archives on every alarm: each archive adds its own `Promoted` and `Archived` entries,
/// which put the log over again by their bytes.
pub const ARCHIVE_SLACK: u64 = 16;

/// The next range to archive, with its stored rows as `(index, at, row)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Due {
    pub from: Index,
    pub to: Index,
    pub rows: Vec<(Index, Timestamp, Vec<u8>)>,
}

/// `archive_due_walk` over the stored rows above the last archived range, in index order, each the raft codec of
/// a `LogEntry<RepoEvent>` as the member stores it. `total` is the rows' total bytes. Only the rows the walk reads are
/// decoded and kept: the range and one more. A gap in the indexes or a row that does not decode is an error. Over the
/// cap, the range is cut against the cap less `max_bytes / ARCHIVE_SLACK`.
#[allow(clippy::too_many_arguments)]
pub fn due(
    total: u64,
    rows: impl IntoIterator<Item = (Index, Vec<u8>)>,
    now: Timestamp,
    snapshotted: bool,
    committed: Index,
    max_bytes: u64,
    range_max: u64,
) -> Result<Option<Due>, String> {
    let mut kept: Vec<(Index, Timestamp, Vec<u8>)> = Vec::new();
    let mut fail: Option<String> = None;
    let walk = rows.into_iter().map_while(|(i, row)| {
        if let Some((last, _, _)) = kept.last()
            && i.0 != last.0 + 1
        {
            fail = Some(format!("the stored log lacks entry {}", last.0 + 1));
            return None;
        }
        match codec::from_bytes::<LogEntry<RepoEvent>>(&row) {
            Ok(e) if e.index == i => {
                let item = (i, e.at, row.len() as u64);
                kept.push((i, e.at, row));
                Some(item)
            }
            Ok(e) => {
                fail = Some(format!("row {} holds entry {}", i.0, e.index.0));
                None
            }
            Err(e) => {
                fail = Some(format!("row {}: {e}", i.0));
                None
            }
        }
    });
    let target = if total > max_bytes { max_bytes - max_bytes / ARCHIVE_SLACK } else { max_bytes };
    let range = archive_due_walk(total, walk, now, snapshotted, committed, target, range_max);
    if let Some(f) = fail {
        return Err(f);
    }
    Ok(range.map(|(from, to)| {
        kept.retain(|(i, _, _)| *i <= to);
        Due { from, to, rows: kept }
    }))
}

/// What to do with the due range `from..=to`, given the last `Promoted`: archive the part it covers, or, when it
/// covers none of it, promote the whole range first. Archiving only the covered part keeps the archive moving: a
/// `Promoted` entry adds log bytes, so under the byte cap the next due range reaches one entry further each time,
/// and a rule that promoted until it covered the whole due range would never archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    Promote { from: Index, to: Index },
    Archive { from: Index, to: Index },
}

pub fn next(last: Option<&Promoted>, from: Index, to: Index) -> Next {
    let done = last.map_or(0, |p| p.to.0);
    if done < from.0 { Next::Promote { from, to } } else { Next::Archive { from, to: Index(to.0.min(done)) } }
}

fn pack_bytes(e: &LogEntry<RepoEvent>) -> u64 {
    let graftyard_raft::EntryBody::Events(evs) = &e.body else { return 0 };
    packs_of(evs)
        .iter()
        .map(|p| match p {
            PackRef::Inline(b) => b.len() as u64,
            PackRef::Stored { bytes, .. } => *bytes,
        })
        .sum()
}

/// The first entries whose packs fit `promote_max` together, at least one: a promotion takes the range in pieces
/// as an archive does. A first entry alone over it is refused, and the alarm reports it each time it runs.
pub fn within_promote_max(
    entries: Vec<LogEntry<RepoEvent>>,
    promote_max: u64,
) -> Result<Vec<LogEntry<RepoEvent>>, String> {
    let mut taken = 0u64;
    let mut out = Vec::new();
    for e in entries {
        let b = pack_bytes(&e);
        if taken.saturating_add(b) > promote_max {
            if out.is_empty() {
                return Err(format!(
                    "entry {} names {b} pack bytes, over the {promote_max} a promotion holds",
                    e.index.0
                ));
            }
            break;
        }
        taken += b;
        out.push(e);
    }
    Ok(out)
}

/// The SHA-256 of every pack the entries name, in order and once each: the checksums under which the git cache shard
/// ingested them, which its `/promote` packs into one complete pack. Inline packs and stored ones both count: a pushed
/// pack is thin, so a stored pack's bases may be in an inline pack of the range, and the reverse (BF-18).
pub fn promotion_sums(entries: &[LogEntry<RepoEvent>]) -> Vec<Digest> {
    let mut out: Vec<Digest> = Vec::new();
    for e in entries {
        let graftyard_raft::EntryBody::Events(evs) = &e.body else { continue };
        for p in packs_of(evs) {
            let sum = match p {
                PackRef::Inline(b) => Digest(Sha256::digest(&b).into()),
                PackRef::Stored { checksum, .. } => checksum,
            };
            if !out.contains(&sum) {
                out.push(sum);
            }
        }
    }
    out
}

/// The archive object of `rows`, which must be a contiguous range, and the `Archived` that names it.
pub fn archive_object(chain: ChainId, rows: &[(Index, Timestamp, Vec<u8>)]) -> Result<(Vec<u8>, Archived), String> {
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else { return Err("an empty range".into()) };
    if rows.windows(2).any(|w| w[1].0.0 != w[0].0.0 + 1) {
        return Err("the rows are not contiguous".into());
    }
    let mut buf = archive_header();
    for (_, _, row) in rows {
        encode_archive_row(row, &mut buf);
    }
    let digest = Digest(Sha256::digest(&buf).into());
    let (from, to) = (first.0, last.0);
    let key = R2Key::Archive { chain, from, to, digest };
    Ok((buf, Archived { key, from, to, from_at: first.1, to_at: last.1, digest }))
}

fn request(parts: &[&[u8]]) -> RequestId {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u32).to_be_bytes());
        h.update(p);
    }
    let d: [u8; 32] = h.finalize().into();
    RequestId(Ulid(u128::from_be_bytes(d[..16].try_into().expect("16"))))
}

/// The request id of the `Promoted` for `from..=to`: the same for every attempt, so a leader whose earlier attempt
/// committed is answered from its `request` row and records nothing twice.
pub fn promote_request(chain: ChainId, from: Index, to: Index) -> RequestId {
    request(&[b"graftyard-log-promote", chain.to_string().as_bytes(), &from.0.to_be_bytes(), &to.0.to_be_bytes()])
}

/// The request id of the `Archived` for an archive object, from its digest.
pub fn archive_request(digest: Digest) -> RequestId {
    request(&[b"graftyard-log-archive", &digest.0])
}

/// The stored rows `ArchiveHost::rows` yields, `(index, row)` in index order.
pub type Rows<'a> = Box<dyn Iterator<Item = (Index, Vec<u8>)> + 'a>;

/// What the archiving leader knows when its alarm runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub chain: ChainId,
    pub commit: Index,
    /// The time of the newest applied entry, which `archive_due` reads as now.
    pub now: Timestamp,
    /// Both tiers have a committed snapshot.
    pub snapshotted: bool,
    pub promoted: Option<Promoted>,
    /// The last archived range's `to`, 0 before any.
    pub archived_to: Index,
    /// The stored log's bytes above `archived_to`.
    pub total: u64,
}

impl View {
    /// The view of a repository member's fold: `total` is its stored log's bytes above the last archive.
    pub fn of(chain: ChainId, fold: &crate::repo::RepoFold, commit: Index, total: u64) -> View {
        let snapshotted = [graftyard_types::Tier::Core, graftyard_types::Tier::Metadata]
            .into_iter()
            .all(|t| !crate::tiers::snapshots(&*fold.core, t).is_empty());
        View {
            chain,
            commit,
            now: graftyard_fold::try_meta(&*fold.core).map_or(Timestamp(0), |m| m.at),
            snapshotted,
            promoted: fold.promoted(),
            archived_to: fold.archived().map_or(Index(0), |a| a.to),
            total,
        }
    }
}

/// One alarm's work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Idle,
    Promoted { from: Index, to: Index, index: Index },
    Archived { from: Index, to: Index, index: Index, key: String },
}

/// The leader's I/O, which the site implements over its storage, its shard, and its buckets.
#[allow(async_fn_in_trait)]
pub trait ArchiveHost {
    /// `None` unless this member leads and may archive now.
    fn view(&self) -> Option<View>;
    /// The stored rows above `after`, in index order, read lazily.
    fn rows(&self, after: Index) -> Result<Rows<'_>, String>;
    /// The packs under these checksums as one complete pack, validated and placed: the home shard's `/promote` and
    /// `/validate`, then `PackStore::place`. Always `Stored`.
    async fn promote(&self, sums: &[Digest], request: RequestId) -> Result<PackRef, String>;
    /// The `Committed` marker of a stored pack at `index`.
    async fn mark(&self, pack: &PackRef, index: Index);
    /// Create-only puts of the archive object; how many buckets hold it.
    async fn put(&self, key: &str, bytes: &[u8], digest: Digest) -> Result<usize, String>;
    /// Records the events under `request` and waits for the entry: its index.
    async fn record(&self, request: RequestId, events: Vec<RepoEvent>) -> Result<Index, String>;
}

/// One step of the archive: nothing, a promotion of the next due range, or, once it is promoted, its archive.
pub async fn step<H: ArchiveHost>(h: &H, max_bytes: u64, range_max: u64, promote_max: u64) -> Result<Step, String> {
    let Some(v) = h.view() else { return Ok(Step::Idle) };
    let rows = h.rows(v.archived_to)?;
    let Some(d) = due(v.total, rows, v.now, v.snapshotted, v.commit, max_bytes, range_max)? else {
        return Ok(Step::Idle);
    };
    if d.from.0 != v.archived_to.0 + 1 {
        return Err(format!("the stored log starts at {} after an archive through {}", d.from.0, v.archived_to.0));
    }
    let (from, to) = match next(v.promoted.as_ref(), d.from, d.to) {
        Next::Archive { from, to } => (from, to),
        Next::Promote { from, to } => {
            let decoded: Vec<LogEntry<RepoEvent>> = d
                .rows
                .iter()
                .map(|(_, _, r)| codec::from_bytes(r).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            let entries = within_promote_max(decoded, promote_max)?;
            let to = entries.last().map_or(to, |e| e.index);
            let request = promote_request(v.chain, from, to);
            let pack = h.promote(&promotion_sums(&entries), request).await?;
            if !matches!(pack, PackRef::Stored { .. }) {
                return Err("a promoted pack is always stored".into());
            }
            // The range's own stored packs are marked again below the promotion, as the bed's archive route does.
            for e in &entries {
                let graftyard_raft::EntryBody::Events(evs) = &e.body else { continue };
                for p in packs_of(evs).iter().filter(|p| p.stored_id().is_some()) {
                    h.mark(p, e.index).await;
                }
            }
            let p = Promoted { from, to, pack: pack.clone() };
            let index = h.record(request, vec![RepoEvent::Promoted(p)]).await?;
            h.mark(&pack, index).await;
            return Ok(Step::Promoted { from, to, index });
        }
    };
    let rows: Vec<_> = d.rows.into_iter().filter(|(i, _, _)| *i <= to).collect();
    let (bytes, a) = archive_object(v.chain, &rows)?;
    let key = a.key.to_string();
    let held = h.put(&key, &bytes, a.digest).await?;
    if held < 2 {
        return Err(format!("archive {key} is in {held} buckets"));
    }
    let index = h.record(archive_request(a.digest), vec![RepoEvent::Archived(a)]).await?;
    Ok(Step::Archived { from, to, index, key })
}
