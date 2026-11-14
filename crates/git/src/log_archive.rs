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

/// A promotion takes up to this many archive ranges of log at once, and one archive step puts up to `ARCHIVE_PIECES`
/// objects of `ARCHIVE_RANGE_MAX` each and records them in one entry [inference from B0-30's rerun]. Each step pays
/// its entry's commit and twice that again in pacing (`workers/site/src/host.rs`, `paced`), about 1.35 s, so with 4
/// ranges a promotion and one range an archive the rerun's drain spent 2.3 s per 2 MiB archived and 3.4 s per 8 MiB
/// promoted: about 0.67 MB/s against 0.72 MB/s of log applied, and the log passed its cap by 5% at the first leader
/// change (`tests/am1/results/b030-split.md`, "B0-30 rerun"). At 8 and 8, with an object's two bucket puts at once,
/// the same costs give about 1.7 MB/s, past twice the rerun's log rate (`crates/git/tests/log_archive.rs`,
/// `the_drain_outruns_twice_the_rerun_s_log_rate_at_its_step_costs`). `PROMOTE_MAX` still bounds a promotion's pack
/// bytes, and an archive step builds one object at a time, so memory holds one range of rows as before.
pub const PROMOTE_RANGES: u64 = 8;
pub const ARCHIVE_PIECES: usize = 8;

/// The leader promotes ahead of archiving once the stored log above the last `Promoted` passes the cap divided by
/// this [guess: 64 MiB at `LOG_MAX_BYTES`]. A git cache shard rehydrates from the last `Promoted` by reading every
/// entry above it, and with promotion only when a range was due to archive that tail stood near the whole cap (2 GiB
/// of inline packs), which no rehydrate finished inside a push's deadline (`tests/am1/results/b0-30.md`, blockers 9
/// to 11). Promoted packs are stored, so a rehydrate reads them from R2 instead.
pub const PROMOTE_AHEAD: u64 = 32;

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
    let range = archive_due_walk(total, walk, now, snapshotted, committed, target(total, max_bytes), range_max);
    if let Some(f) = fail {
        return Err(f);
    }
    Ok(range.map(|(from, to)| {
        kept.retain(|(i, _, _)| *i <= to);
        Due { from, to, rows: kept }
    }))
}

/// The size `due` cuts the log down to: the cap less `max_bytes / ARCHIVE_SLACK` once `total` is over the cap.
fn target(total: u64, max_bytes: u64) -> u64 {
    if total > max_bytes { max_bytes - max_bytes / ARCHIVE_SLACK } else { max_bytes }
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

/// The request id of the entry that records these archive objects, from their digests in order: one object's is the
/// same as before `ARCHIVE_PIECES`.
pub fn archive_request(digests: &[Digest]) -> RequestId {
    let mut parts: Vec<&[u8]> = vec![b"graftyard-log-archive"];
    parts.extend(digests.iter().map(|d| &d.0[..]));
    request(&parts)
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
    /// The stored log's bytes above the last `Promoted`'s `to`.
    pub unpromoted: u64,
}

impl View {
    /// The view of a repository member's fold: `total` is its stored log's bytes above the last archive, and
    /// `unpromoted` those above the last promotion.
    pub fn of(chain: ChainId, fold: &crate::repo::RepoFold, commit: Index, total: u64, unpromoted: u64) -> View {
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
            unpromoted,
        }
    }
}

/// One alarm's work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Idle,
    Promoted {
        from: Index,
        to: Index,
        index: Index,
    },
    /// One entry's archive objects, `from..=to` in all, and their keys in order.
    Archived {
        from: Index,
        to: Index,
        index: Index,
        keys: Vec<String>,
    },
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
    /// Lets the member's other work run: a promotion decodes and hashes its range on the leader, about 4 ms a MiB
    /// in wasm, and pauses every `PAUSE_EVERY` bytes of rows so its loop never falls behind by a whole range.
    async fn pause(&self) {}
}

/// A promotion pauses after this many bytes of rows (`ArchiveHost::pause`), and reads its rows again after each.
pub const PAUSE_EVERY: u64 = 2 << 20;

/// The next promotion, read from the stored log above `after` one row at a time: up to `budget` bytes of committed
/// rows and at least one, cut before an entry whose packs would take the total past `promote_max`. An entry alone
/// over it is refused. A gap or a row that does not decode is an error.
struct Plan {
    to: Index,
    /// `promotion_sums` of the range.
    sums: Vec<Digest>,
    /// The range's stored packs and their entries, which the promotion marks again.
    stored: Vec<(PackRef, Index)>,
}

async fn plan<H: ArchiveHost>(
    h: &H,
    after: Index,
    committed: Index,
    budget: u64,
    promote_max: u64,
) -> Result<Option<Plan>, String> {
    let (mut p, mut taken, mut packed, mut paused) =
        (Plan { to: after, sums: Vec::new(), stored: Vec::new() }, 0, 0, 0);
    'read: loop {
        let mut more = false;
        for (i, row) in h.rows(p.to)? {
            let first = p.to == after;
            if i > committed || (!first && taken + row.len() as u64 > budget) {
                break 'read;
            }
            if i.0 != p.to.0 + 1 {
                return Err(format!("the stored log lacks entry {}", p.to.0 + 1));
            }
            let e: LogEntry<RepoEvent> = codec::from_bytes(&row).map_err(|e| format!("row {}: {e}", i.0))?;
            if e.index != i {
                return Err(format!("row {} holds entry {}", i.0, e.index.0));
            }
            let b = pack_bytes(&e);
            if packed + b > promote_max {
                if first {
                    return Err(format!(
                        "entry {} names {b} pack bytes, over the {promote_max} a promotion holds",
                        i.0
                    ));
                }
                break 'read;
            }
            packed += b;
            taken += row.len() as u64;
            p.to = i;
            for sum in promotion_sums(std::slice::from_ref(&e)) {
                if !p.sums.contains(&sum) {
                    p.sums.push(sum);
                }
            }
            if let graftyard_raft::EntryBody::Events(evs) = &e.body {
                p.stored.extend(packs_of(evs).into_iter().filter(|x| x.stored_id().is_some()).map(|x| (x, i)));
            }
            if taken >= paused + PAUSE_EVERY {
                (paused, more) = (taken, true);
                break;
            }
        }
        if !more {
            break;
        }
        // The cursor is dropped before the pause, so no write meanwhile runs under it; the read resumes above `to`.
        h.pause().await;
    }
    Ok((p.to != after).then_some(p))
}

/// One step of the archive: nothing; a promotion; or up to `ARCHIVE_PIECES` archive objects of the next due ranges,
/// as far as the last promotion covers them, recorded in one entry. A promotion goes first once the log above the
/// last promotion passes `max_bytes / PROMOTE_AHEAD`, even with a range due, unless the log is over its cap by more
/// than `max_bytes / ARCHIVE_SLACK`: in the rerun, archiving first left each promotion just ahead of the archive, and
/// the log above it stood at 2.07 GB, the whole tail a cold shard's rehydrate reads. Otherwise a due range the last
/// promotion does not cover is promoted. Every promotion starts right after the
/// last one and takes up to `PROMOTE_RANGES` ranges of log and `promote_max` pack bytes.
pub async fn step<H: ArchiveHost>(h: &H, max_bytes: u64, range_max: u64, promote_max: u64) -> Result<Step, String> {
    let Some(v) = h.view() else { return Ok(Step::Idle) };
    let d = due(v.total, h.rows(v.archived_to)?, v.now, v.snapshotted, v.commit, max_bytes, range_max)?;
    let done = Index(v.promoted.as_ref().map_or(0, |p| p.to.0));
    if let Some(d) = d.as_ref()
        && d.from.0 != v.archived_to.0 + 1
    {
        return Err(format!("the stored log starts at {} after an archive through {}", d.from.0, v.archived_to.0));
    }
    // Ahead of archiving only once both tiers have a snapshot, as an archive waits for: a group that has none yet is
    // new, and its first snapshot comes on its first alarms (`graftyard_fold::snapshot_due`).
    // Promoting ahead goes first while the log is within one slack of its cap, and archiving first past that: a new
    // leader whose tail is far above the bound would otherwise promote for minutes while the log grows past the cap.
    let ahead = v.snapshotted && v.unpromoted > max_bytes / PROMOTE_AHEAD;
    let ahead_first = ahead && v.total <= max_bytes + max_bytes / ARCHIVE_SLACK;
    match d {
        Some(d) if !ahead_first && matches!(next(v.promoted.as_ref(), d.from, d.to), Next::Archive { .. }) => {
            return archive(h, &v, d, max_bytes, range_max).await;
        }
        None if !ahead => return Ok(Step::Idle),
        _ => {}
    }
    let budget = range_max.saturating_mul(PROMOTE_RANGES);
    let Some(plan) = plan(h, done, v.commit, budget, promote_max).await? else { return Ok(Step::Idle) };
    let (from, to) = (Index(done.0 + 1), plan.to);
    let request = promote_request(v.chain, from, to);
    let pack = h.promote(&plan.sums, request).await?;
    if !matches!(pack, PackRef::Stored { .. }) {
        return Err("a promoted pack is always stored".into());
    }
    // The range's own stored packs are marked again below the promotion, as the bed's archive route does.
    for (p, index) in &plan.stored {
        h.mark(p, *index).await;
    }
    let p = Promoted { from, to, pack: pack.clone() };
    let index = h.record(request, vec![RepoEvent::Promoted(p)]).await?;
    h.mark(&pack, index).await;
    Ok(Step::Promoted { from, to, index })
}

/// Archives `first`, a due range the last promotion covers at least in part, and the due ranges after it while the
/// promotion covers them, up to `ARCHIVE_PIECES` objects: each object is built and put in two buckets before the next
/// range is read, so memory holds one range of rows, and one entry records them all.
async fn archive<H: ArchiveHost>(h: &H, v: &View, first: Due, max_bytes: u64, range_max: u64) -> Result<Step, String> {
    let (mut total, mut d, mut done) = (v.total, Some(first), Vec::new());
    while let Some(Due { from, to, rows }) = d.take() {
        let Next::Archive { to, .. } = next(v.promoted.as_ref(), from, to) else { break };
        let rows: Vec<_> = rows.into_iter().filter(|(i, _, _)| *i <= to).collect();
        total = total.saturating_sub(rows.iter().map(|(_, _, r)| r.len() as u64).sum());
        let (object, a) = archive_object(v.chain, &rows)?;
        drop(rows);
        let key = a.key.to_string();
        let held = h.put(&key, &object, a.digest).await?;
        if held < 2 {
            return Err(format!("archive {key} is in {held} buckets"));
        }
        done.push((a, key));
        // Against the step's own target: once its first piece took the log under the cap, the cap would be the
        // target again and nothing past it due, so at the row's own rate every step carried one object (B0-30's
        // resume at day 1.82, 17:26Z to 17:29Z: 125 entries a minute archived against 129 applied).
        if done.len() < ARCHIVE_PIECES {
            d = due(total, h.rows(to)?, v.now, v.snapshotted, v.commit, target(v.total, max_bytes), range_max)?;
        }
    }
    let (from, to) = (done[0].0.from, done[done.len() - 1].0.to);
    let request = archive_request(&done.iter().map(|(a, _)| a.digest).collect::<Vec<_>>());
    let (events, keys): (Vec<_>, Vec<_>) = done.into_iter().map(|(a, k)| (RepoEvent::Archived(a), k)).unzip();
    let index = h.record(request, events).await?;
    Ok(Step::Archived { from, to, index, keys })
}
