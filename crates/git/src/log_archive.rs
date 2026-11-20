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
use graftyard_raft::{Format, LogEntry, codec};
use graftyard_types::{ChainId, Digest, Index, RequestId, Timestamp, Ulid};
use sha2::{Digest as _, Sha256};

use crate::key::R2Key;
use crate::pack::{Archived, Compacted, PackRef, Promoted};
use crate::repo::{REPO_FORMAT_ROLLUP, REPO_FORMAT_V1, RepoEvent, packs_of};

/// The most stored log bytes one archive object takes [guess]. The site builds the object in memory beside the
/// member's state, and a 4 MB batch of a large member's log took its isolate past 128 MB (`workers/site/src/load.rs`,
/// `BATCH_BYTES`), so a due range goes in pieces of this size; the alarm runs again at once after each piece.
pub const ARCHIVE_RANGE_MAX: u64 = 2 << 20;
/// The most pack bytes one promotion batches [guess]: about 19 s of the shard's `pack-objects` and validate at the
/// measured 2.62 MB/s (`decisions/DECISIONS.md`, "Promotion's measured rate is under B0-31's push rate"), inside the
/// site's `STEP_FOR` (`workers/site/src/archive.rs`). It bounds no memory: the site streams the shard's pack to the
/// buckets in parts and to the validate, holding two parts whatever its size (`PackStore::place_streamed`), so a
/// first entry over it promotes alone, never refused. Refused, B0-34's drain stalled for good at a 53 MB push
/// (`tests/am1/results/lm1-memory.md`, round 20).
pub const PROMOTE_MAX: u64 = 48 << 20;

/// The most stored pack bytes one roll takes [guess]: 2 GiB, whose measured `pack-objects` resident set is about
/// 3.6 GB beside the container's other work (`decisions/DECISIONS.md`, `roll-up`, "The memory bound"). The walk keeps
/// the first promotion past the cap so `roll_plan` can tell that the run reached it, but never includes it in a roll.
pub const ROLL_IN_MAX: u64 = 2 << 30;
/// A roll below `ROLL_IN_MAX` waits for this many promoted packs [guess], the batch measured at half its input bytes
/// (`decisions/DECISIONS.md`, `roll-up`, "Measured on B0-34's plan").
pub const ROLL_MIN_PACKS: usize = 8;
/// A run of at least `ROLL_MIN_PACKS` below `ROLL_IN_MAX` rolls once its oldest pack is this old [guess], so a quiet
/// chain eventually gets the smaller read set without rolling every few promotions (`decisions/DECISIONS.md`,
/// `roll-up`, "What rolls").
pub const ROLL_AGE_MS: u64 = 86_400_000;

/// Whether a site day turned roll-up on: the site variable `GRAFTYARD_ROLLUP` is exactly `1`. A site day sets it only
/// after `FormatsEnable` enabled 32.25, so no leader records a `Compacted` before every member can read one; unset, the
/// alarm sends no `/roll` and records nothing (`decisions/DECISIONS.md`, `roll-up`, "Status").
pub fn rollup_enabled(var: Option<&str>) -> bool {
    var == Some("1")
}

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

/// The archive's hysteresis band, a share of the cap: a drain run starts once the stored log passes `trigger`, the cap
/// less half of `max_bytes / ARCHIVE_SLACK`, and archives down to `floor`, the cap less all of it, before it goes
/// `Idle` (`Run`). An idle leader sums its log once a `SUM_EVERY_MS` (`workers/site/src/archive.rs`), so the log can
/// grow one minute's worth past the trigger before a run starts: the trigger sits below the cap by more than that at
/// `DESIGN_LOG_BYTES_PER_S`, which the site asserts. At 8, the band is 268 MB and its lower half 134 MB, against 72 MB
/// a minute at twice the row's log rate [inference from B0-30's second resume]. At 16, with the slack held per step
/// only, the second resume's log swung from 8 MB under the cap to 40 MB over it and never reached the floor
/// (`tests/am1/results/b030-split.md`, "B0-30 rerun").
pub const ARCHIVE_SLACK: u64 = 8;
/// The log rate the band is sized for: twice B0-30's measured log growth at the row's own rate, 113 entries a minute
/// of about 320 KB [measured, the second resume, 18:34Z to 18:40Z].
pub const DESIGN_LOG_BYTES_PER_S: u64 = 1_200_000;

/// The stored log size that starts a drain run.
pub fn trigger(max_bytes: u64) -> u64 {
    max_bytes - max_bytes / ARCHIVE_SLACK / 2
}

/// The stored log size a drain run archives down to.
pub fn floor(max_bytes: u64) -> u64 {
    max_bytes - max_bytes / ARCHIVE_SLACK
}

/// One drain run's state, held across its steps and across the runs that continue it after `DRAIN_FOR_MS`: whether
/// it has passed `trigger` and so archives down to `floor`. It holds until a step archives all the way to the floor,
/// not until the log is under it: input goes on while each step runs, so a run held to "under the floor" would take
/// a step of one object after each step for as long as pushes come, the per-entry cost the drain was built to avoid.
/// A step that finds nothing due ends the run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Run {
    #[serde(skip)]
    draining: bool,
    /// The last promotion that failed, kept across the steps that find nothing to do (`retry`).
    failed: Option<Failed>,
}

/// A promotion that failed: the first entry of its range, how many tries of that range failed in a row, when the last
/// one ended, and when the last try that could start a stopped shard ran (wall ms, `ArchiveHost::wall_ms`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Failed {
    from: Index,
    times: u32,
    at: u64,
    started: u64,
}

/// A failed promotion's range is tried again after `RETRY_FIRST_MS`, doubling with each failure in a row up to
/// `RETRY_MAX_MS` [guess]. Without it, a range whose promotion could never succeed was tried at every alarm: on
/// graft-m1, `repo:1048577`'s range (1, 4) asked every site's shard for entry 504 of a 4-entry log, failed the same way
/// each time, and started all three containers on each try (`decisions/DECISIONS.md`, "`retire-sticks`").
pub const RETRY_FIRST_MS: u64 = 60_000;
pub const RETRY_MAX_MS: u64 = 8 * 60_000;
/// A retry of a failed range is a warm call, which starts no stopped shard, unless this long has passed since the last
/// try that could [guess]. The shards a failed try started stay up `PROMOTE_LAPSE` (10 minutes), past the first
/// retries, so a failure that passes is retried on them; one that repeats starts containers four times a day, not at
/// every alarm.
pub const RESTART_EVERY_MS: u64 = 6 * 3_600_000;

/// How the next promotion of the range starting at `from` may go, given the run's last failure, at wall time `now`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Retry {
    /// Not yet: the step is idle.
    Wait,
    /// A warm call only (`ArchiveHost::promote`'s `warm`).
    Warm,
    /// As a first try: a due range's promotion may start a stopped shard.
    Start,
}

fn backoff(times: u32) -> u64 {
    RETRY_FIRST_MS.saturating_mul(1 << times.saturating_sub(1).min(16)).min(RETRY_MAX_MS)
}

impl Run {
    /// What the site keeps in the member's storage across isolate resets: the last failure alone (`Run`'s serde form
    /// skips the band). An idle leader's isolate is often gone by its next alarm, and a failure kept in memory only
    /// would start every shard again on each new isolate's first try.
    pub fn kept(&self) -> Run {
        Run { draining: false, failed: self.failed }
    }

    fn retry(&self, from: Index, now: u64) -> Retry {
        match self.failed.filter(|f| f.from == from) {
            None => Retry::Start,
            Some(f) if now < f.at.saturating_add(backoff(f.times)) => Retry::Wait,
            Some(f) if now >= f.started.saturating_add(RESTART_EVERY_MS) => Retry::Start,
            Some(_) => Retry::Warm,
        }
    }

    /// A promotion of the range starting at `from` failed at `now`; `started`: it could start a stopped shard.
    fn fail(&mut self, from: Index, now: u64, started: bool) {
        let last = self.failed.filter(|f| f.from == from);
        self.failed = Some(Failed {
            from,
            times: last.map_or(1, |f| f.times.saturating_add(1)),
            at: now,
            started: if started { now } else { last.map_or(0, |f| f.started) },
        });
    }
}

impl Run {
    /// The size this run's next step cuts the log down to: `floor` once the run has seen the log over `trigger`, and
    /// `trigger` before, so only the time rule (`LOG_KEEP`) makes anything due under it.
    fn target(&mut self, total: u64, max_bytes: u64) -> u64 {
        self.draining |= total > trigger(max_bytes);
        if self.draining { floor(max_bytes) } else { trigger(max_bytes) }
    }
}

/// The next range to archive, with its stored rows as `(index, at, row)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Due {
    pub from: Index,
    pub to: Index,
    pub rows: Vec<(Index, Timestamp, Vec<u8>)>,
}

/// `archive_due_walk` over the stored rows above the last archived range, in index order, each the raft codec of
/// a `LogEntry<RepoEvent>` as the member stores it. `total` is the rows' total bytes. Only the rows the walk reads are
/// decoded and kept: the range and one more. A gap in the indexes or a row that does not decode is an error. `target` is
/// the size the log is cut down to (`Run::target`).
#[allow(clippy::too_many_arguments)]
pub fn due(
    total: u64,
    rows: impl IntoIterator<Item = (Index, Vec<u8>)>,
    now: Timestamp,
    snapshotted: bool,
    committed: Index,
    target: u64,
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

/// The request id of the `Compacted` for `from..=to`: the same for every attempt, so a leader whose earlier attempt
/// committed is answered from its `request` row and records nothing twice (`decisions/DECISIONS.md`, `roll-up`).
pub fn roll_request(chain: ChainId, from: Index, to: Index) -> RequestId {
    request(&[b"graftyard-log-roll", chain.to_string().as_bytes(), &from.0.to_be_bytes(), &to.0.to_be_bytes()])
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
    /// The committed promotions no roll has consumed, consecutive from `rolled_to + 1` and oldest first. The walk
    /// stops at the first gap, or after the first stored pack that takes the run past `ROLL_IN_MAX`.
    pub rollable: Vec<Promoted>,
    /// The last archived range's `to`, 0 before any.
    pub archived_to: Index,
    /// The stored log's bytes above `archived_to`.
    pub total: u64,
    /// The pack bytes the entries above the last `Promoted`'s `to` name, inline and stored (`pack_bytes_above`): what a
    /// rehydrate indexes on top of the promotions.
    pub unpromoted: u64,
}

/// The pack bytes the entries of `rows` name, inline and stored, counted until past `stop_past`: promote-ahead's
/// measure (`View::unpromoted`). A stored pack's row is only its pointer, so the log's row bytes left a network made
/// of stored pushes unpromoted for good: repo:131296's 257 pushed packs, 2.62 GB, sat above its last promotion in a
/// 294-entry log, and a cold rehydrate re-indexed every one for 1,694 to 1,872 s (`tests/am1/results/lm1-rows-m3f.md`,
/// "Stored packs and their index").
///
/// Each row comes as its length and, when it is under `SHORT_ROW`, its bytes: a row that long or longer is counted by
/// its length, which an inline pack's row about equals, so the host never reads a long row back. A short row is
/// decoded for the stored packs it names; one that does not decode counts its length.
pub fn pack_bytes_above(rows: impl IntoIterator<Item = (u64, Option<Vec<u8>>)>, stop_past: u64) -> u64 {
    let mut sum = 0u64;
    for (len, row) in rows {
        sum += match row {
            Some(r) => codec::from_bytes::<LogEntry<RepoEvent>>(&r).map_or(len, |e| pack_bytes(&e).max(len)),
            None => len,
        };
        if sum > stop_past {
            break;
        }
    }
    sum
}

/// Rows under this many bytes are read back and decoded by `pack_bytes_above`; longer ones count their length.
pub const SHORT_ROW: u64 = 4_096;

impl View {
    /// The view of a repository member's fold: `total` is its stored log's bytes above the last archive, and
    /// `unpromoted` the pack bytes its entries above the last promotion name (`pack_bytes_above`).
    pub fn of(chain: ChainId, fold: &crate::repo::RepoFold, commit: Index, total: u64, unpromoted: u64) -> View {
        let snapshotted = [graftyard_types::Tier::Core, graftyard_types::Tier::Metadata]
            .into_iter()
            .all(|t| !crate::tiers::snapshots(&*fold.core, t).is_empty());
        let mut rollable = Vec::new();
        let mut at = fold.rolled_to().0 + 1;
        let mut bytes = 0u64;
        while let Some(p) = fold.promoted_from(Index(at)) {
            at = p.to.0 + 1;
            bytes = bytes.saturating_add(stored_bytes(&p.pack));
            rollable.push(p);
            if bytes > ROLL_IN_MAX {
                break;
            }
        }
        View {
            chain,
            commit,
            now: graftyard_fold::try_meta(&*fold.core).map_or(Timestamp(0), |m| m.at),
            snapshotted,
            promoted: fold.promoted(),
            rollable,
            archived_to: fold.archived().map_or(Index(0), |a| a.to),
            total,
            unpromoted,
        }
    }
}

fn stored_bytes(pack: &PackRef) -> u64 {
    match pack {
        PackRef::Stored { bytes, .. } => *bytes,
        PackRef::Inline(_) => 0,
    }
}

/// The longest prefix of the oldest unconsumed promotions whose stored bytes fit `ROLL_IN_MAX`, when it is due to
/// roll. A following promotion means the run reached the byte cap; otherwise the prefix needs `ROLL_MIN_PACKS` and an
/// oldest pack at least `ROLL_AGE_MS` old (`decisions/DECISIONS.md`, `roll-up`, "What rolls"). Verify admits only
/// stored promotions, but an inline one contributes no bytes here.
pub fn roll_plan(rollable: &[Promoted], now: Timestamp) -> Option<Vec<Promoted>> {
    let mut bytes = 0u64;
    let mut prefix = 0;
    for p in rollable {
        let next = bytes.saturating_add(stored_bytes(&p.pack));
        if next > ROLL_IN_MAX {
            break;
        }
        bytes = next;
        prefix += 1;
    }
    if prefix < 2 {
        return None;
    }
    let reached = prefix < rollable.len();
    let old = rollable[0].pack.stored_id().is_some_and(|id| now.0.saturating_sub(id.time().0) >= ROLL_AGE_MS);
    (reached || (prefix >= ROLL_MIN_PACKS && old)).then(|| rollable[..prefix].to_vec())
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
    /// One roll of the oldest consecutive promotions' combined range, recorded at `index`.
    Rolled {
        from: Index,
        to: Index,
        index: Index,
    },
    /// One entry's archive objects, `from..=to` in all, and their keys in order. `reached`: the log is at the step's
    /// target after them, with nothing more due (`Run`).
    Archived {
        from: Index,
        to: Index,
        index: Index,
        keys: Vec<String>,
        reached: bool,
    },
}

/// The leader's I/O, which the site implements over its storage, its shard, and its buckets.
#[allow(async_fn_in_trait)]
pub trait ArchiveHost {
    /// `None` unless this member leads and may archive now.
    fn view(&self) -> Option<View>;
    /// Whether this site may roll (`rollup_enabled`).
    fn rollup(&self) -> bool;
    /// The stored rows above `after`, in index order, read lazily.
    fn rows(&self, after: Index) -> Result<Rows<'_>, String>;
    /// The packs under these checksums as one complete pack, validated and placed with its `.idx`, which names the
    /// range `from..=to`: the home shard's `/promote` and `/validate`, then `PackStore::place`. Always `Stored`.
    /// `warm`: promote-ahead's, a warm call (`crate::Caller::Warm`) that starts no stopped shard; `None` when every
    /// shard it would ask is stopped.
    async fn promote(
        &self,
        sums: &[Digest],
        from: Index,
        to: Index,
        request: RequestId,
        warm: bool,
    ) -> Result<Option<PackRef>, String>;
    /// The packs under these checksums, oldest first, as one complete pack of every object they hold, validated and
    /// placed with its `.idx`, which names the combined range `from..=to`: the home shard's `/roll` and `/validate`,
    /// then `PackStore::place_promoted`. Always `Stored`. Always a warm call (`crate::Caller::Warm`) that starts no
    /// stopped shard; `None` when every shard it would ask is stopped.
    async fn roll(
        &self,
        sums: &[Digest],
        from: Index,
        to: Index,
        request: RequestId,
    ) -> Result<Option<PackRef>, String>;
    /// The `Committed` marker of a stored pack at `index`.
    async fn mark(&self, pack: &PackRef, index: Index);
    /// Create-only puts of the archive object; how many buckets hold it.
    async fn put(&self, key: &str, bytes: &[u8], digest: Digest) -> Result<usize, String>;
    /// Records the events under `request` in `format` and waits for the entry: its index.
    async fn record(&self, request: RequestId, format: Format, events: Vec<RepoEvent>) -> Result<Index, String>;
    /// The wall clock in ms, which paces a failed promotion's retries (`RETRY_FIRST_MS`). Not the fold's time, which an
    /// idle group does not move.
    fn wall_ms(&self) -> u64;
    /// Lets the member's other work run: a promotion decodes and hashes its range on the leader, about 4 ms a MiB
    /// in wasm, and pauses every `PAUSE_EVERY` bytes of rows so its loop never falls behind by a whole range.
    async fn pause(&self) {}
}

/// A promotion pauses after this many bytes of rows (`ArchiveHost::pause`), and reads its rows again after each.
pub const PAUSE_EVERY: u64 = 2 << 20;

/// The next promotion, read from the stored log above `after` one row at a time: up to `budget` bytes of committed
/// rows and at least one, cut before an entry whose packs would take the total past `promote_max`. A first entry
/// alone over it promotes alone. A gap or a row that does not decode is an error.
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
            // A first entry alone over `promote_max` promotes alone.
            if !first && packed + b > promote_max {
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

/// One step of `run`: nothing; a promotion; up to `ARCHIVE_PIECES` archive objects of the next due ranges, as far as
/// the last promotion covers them, recorded in one entry; or, when none of those has work, one roll. What is due
/// follows the run's band (`Run`), and a step
/// that finds nothing to do ends the run. A promotion goes first once the log above the last promotion passes
/// `max_bytes / PROMOTE_AHEAD`, even with a range due, unless the run has passed its trigger: in the rerun, archiving first
/// left each promotion just ahead of the archive, and the log above it stood at 2.07 GB, the whole tail a cold
/// shard's rehydrate reads. Otherwise a due range the last promotion does not cover is promoted. Every promotion
/// starts right after the last one and takes up to `PROMOTE_RANGES` ranges of log and `promote_max` pack bytes.
pub async fn step<H: ArchiveHost>(
    h: &H,
    run: &mut Run,
    max_bytes: u64,
    range_max: u64,
    promote_max: u64,
) -> Result<Step, String> {
    let s = step_in(h, run, max_bytes, range_max, promote_max).await;
    match &s {
        // A step with nothing to do ends the run's band, but a failed range keeps its retry pacing.
        Ok(Step::Idle) => *run = Run { failed: run.failed, ..Run::default() },
        Ok(Step::Archived { reached: true, .. }) => run.draining = false,
        _ => {}
    }
    s
}

/// A roll only fills an otherwise idle step: archive ranges and promotions, including warm promote-ahead, always go
/// first (`decisions/DECISIONS.md`, `roll-up`, "Who writes it").
async fn roll<H: ArchiveHost>(h: &H, v: &View) -> Result<Step, String> {
    if !h.rollup() {
        return Ok(Step::Idle);
    }
    let Some(ps) = roll_plan(&v.rollable, v.now) else { return Ok(Step::Idle) };
    let from = ps[0].from;
    let to = ps[ps.len() - 1].to;
    let mut sums = Vec::with_capacity(ps.len());
    let mut consumed = Vec::with_capacity(ps.len());
    for p in &ps {
        let PackRef::Stored { id, checksum, .. } = &p.pack else {
            return Err("a promoted pack is always stored".into());
        };
        sums.push(*checksum);
        consumed.push(*id);
    }
    let request = roll_request(v.chain, from, to);
    let Some(pack) = h.roll(&sums, from, to, request).await? else { return Ok(Step::Idle) };
    if !matches!(pack, PackRef::Stored { .. }) {
        return Err("a rolled pack is always stored".into());
    }
    let c = Compacted { consumed, produced: pack.clone(), from, to };
    let index = h.record(request, REPO_FORMAT_ROLLUP, vec![RepoEvent::Compacted(c)]).await?;
    h.mark(&pack, index).await;
    Ok(Step::Rolled { from, to, index })
}

async fn step_in<H: ArchiveHost>(
    h: &H,
    run: &mut Run,
    max_bytes: u64,
    range_max: u64,
    promote_max: u64,
) -> Result<Step, String> {
    let Some(v) = h.view() else { return Ok(Step::Idle) };
    let target = run.target(v.total, max_bytes);
    let d = due(v.total, h.rows(v.archived_to)?, v.now, v.snapshotted, v.commit, target, range_max)?;
    let done = Index(v.promoted.as_ref().map_or(0, |p| p.to.0));
    if let Some(d) = d.as_ref()
        && d.from.0 != v.archived_to.0 + 1
    {
        return Err(format!("the stored log starts at {} after an archive through {}", d.from.0, v.archived_to.0));
    }
    // Ahead of archiving only once both tiers have a snapshot, as an archive waits for: a group that has none yet is
    // new, and its first snapshot comes on its first alarms (`graftyard_fold::snapshot_due`).
    // Promoting ahead goes first until the run passes its trigger, and archiving first from then until it reaches the
    // floor: a new leader whose tail is far above the bound would otherwise promote for minutes while the log grows
    // past the cap, and a run that promoted first between its trigger and the cap carried the log over the cap.
    let ahead = v.snapshotted && v.unpromoted > max_bytes / PROMOTE_AHEAD;
    let ahead_first = ahead && !run.draining;
    // No range due: promote-ahead's warm call (`ArchiveHost::promote`).
    let warm = d.is_none();
    match d {
        Some(d) if !ahead_first && matches!(next(v.promoted.as_ref(), d.from, d.to), Next::Archive { .. }) => {
            return archive(h, &v, d, target, range_max).await;
        }
        None if !ahead => return roll(h, &v).await,
        _ => {}
    }
    let budget = range_max.saturating_mul(PROMOTE_RANGES);
    let Some(plan) = plan(h, done, v.commit, budget, promote_max).await? else {
        return if warm { roll(h, &v).await } else { Ok(Step::Idle) };
    };
    let (from, to) = (Index(done.0 + 1), plan.to);
    let request = promote_request(v.chain, from, to);
    // Promote-ahead is no client's work, so it runs only on a shard already running; a due range starts one, since
    // the log cannot archive past it otherwise. A range whose promotion failed waits `backoff`, and is then a warm call
    // until `RESTART_EVERY_MS` has passed since a try that could start a shard.
    let now = h.wall_ms();
    let warm = match run.retry(from, now) {
        Retry::Wait => return Ok(Step::Idle),
        Retry::Warm => true,
        Retry::Start => warm,
    };
    let pack = match h.promote(&plan.sums, from, to, request, warm).await {
        Err(e) => {
            run.fail(from, now, !warm);
            return Err(e);
        }
        // Every shard stopped: the range's next warm try waits its backoff again.
        Ok(None) => {
            if let Some(f) = run.failed.as_mut().filter(|f| f.from == from) {
                f.at = now;
            }
            return Ok(Step::Idle);
        }
        Ok(Some(pack)) => pack,
    };
    run.failed = None;
    if !matches!(pack, PackRef::Stored { .. }) {
        return Err("a promoted pack is always stored".into());
    }
    // The range's own stored packs are marked again below the promotion, as the bed's archive route does.
    for (p, index) in &plan.stored {
        h.mark(p, *index).await;
    }
    let p = Promoted { from, to, pack: pack.clone() };
    // In 32.25 once roll-up is on, so the fold logs the promotion for a later roll; 32.1 otherwise, which every
    // release folds the same (F88). A site day turns roll-up on only after `FormatsEnable` enabled 32.25.
    let format = if h.rollup() { REPO_FORMAT_ROLLUP } else { REPO_FORMAT_V1 };
    let index = h.record(request, format, vec![RepoEvent::Promoted(p)]).await?;
    h.mark(&pack, index).await;
    Ok(Step::Promoted { from, to, index })
}

/// Archives `first`, a due range the last promotion covers at least in part, and the due ranges after it while the
/// promotion covers them, up to `ARCHIVE_PIECES` objects: each object is built and put in two buckets before the next
/// range is read, so memory holds one range of rows, and one entry records them all.
async fn archive<H: ArchiveHost>(h: &H, v: &View, first: Due, target: u64, range_max: u64) -> Result<Step, String> {
    let (mut total, mut d, mut done, mut reached) = (v.total, Some(first), Vec::new(), false);
    while let Some(Due { from, to, rows }) = d.take() {
        // The whole due range, not only the part the last promotion covers.
        let due_to = to;
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
        // Against the run's target: once its first piece took the log under the cap, the cap would be the target
        // again and nothing past it due, so at the row's own rate every step carried one object (B0-30's resume at
        // day 1.82, 17:26Z to 17:29Z: 125 entries a minute archived against 129 applied).
        let more = due(total, h.rows(to)?, v.now, v.snapshotted, v.commit, target, range_max)?;
        reached = to == due_to && more.is_none();
        if done.len() < ARCHIVE_PIECES {
            d = more;
        }
    }
    let (from, to) = (done[0].0.from, done[done.len() - 1].0.to);
    let request = archive_request(&done.iter().map(|(a, _)| a.digest).collect::<Vec<_>>());
    let (events, keys): (Vec<_>, Vec<_>) = done.into_iter().map(|(a, k)| (RepoEvent::Archived(a), k)).unzip();
    let index = h.record(request, REPO_FORMAT_V1, events).await?;
    Ok(Step::Archived { from, to, index, keys, reached })
}
