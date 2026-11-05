//! The `PrSync` consumer's pure half (`design/SHARED.md`, "Event fan-out" and "Issues and pull requests";
//! `design/DESIGN.md`, "Dominant access patterns", the webhook path: leader alarm, cursor, Queue message, consumer).
//!
//! - The cursor is the core-tier row `consumer_cursor`, written only by a committed `ConsumerCursor` event in the same
//!   entry as the work it covers. It is the group's state, so a new leader and a restored member read the same one.
//! - The leader's `Dispatcher` is memory only. It reads the log from the cursor, names the pull requests the entries
//!   touch, and hands out `SyncBatch`es. A batch always starts at the committed cursor. Its first attempt runs in
//!   band (`Delivery::First`): the leader runs the consumer's handler itself. Work the cursor has not passed
//!   `REDELIVER_AFTER_MS` after it was first handed out is sent again (`Delivery::Again`), through the site's queue, so
//!   a lost attempt, a dead consumer, or a new leader only delays.
//! - The consumer reads each pull request's state fresh (`views`), has the git cache compute missing test merges, and
//!   commits a `PrSync`. Its `decide` keeps only what still holds on the fold, so a batch delivered twice, or out of
//!   order with a later one, changes nothing the first did not.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_fold::Read;
use graftyard_raft::{Command, EntryBody, Format, LogEntry, Record, TierSet};
use graftyard_types::{ChainId, Index, RepoId, RequestId, Timestamp, Ulid};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::issues::{Consumer, IssueChange, IssueNumber, Mergeability, MergeabilityResult, PrState};
use crate::pack::PackRef;
use crate::refs::{ObjectId, RefName, RefUpdate};
use crate::repo::{RepoEvent, RepoFold, RepoScratch};
use crate::tiers::{consumer_cursor, get_rec, head_ref, merge_ref, mergeability, pull_head, pulls_from, ref_key};

/// The repository chain's records that carry `PrSync` and the `TestMerged` and `ConsumerCursor` events. A member
/// reads it only once a release enables it (`design/SHARED.md`, "Record formats and the two-release rule").
pub const REPO_FORMAT_V3: Format = Format { family: 0x20, version: 3 };

/// Work the cursor has not passed this long after it was first handed out is sent again, through the queue [guess].
/// The first attempt runs in band, so this covers an attempt that failed silently or never returned, a message lost
/// with its consumer, or a leader change; Queues retries a failed queue delivery itself.
pub const REDELIVER_AFTER_MS: u64 = 30_000;
/// A stretch of entries that touch no pull request is still covered by a cursor entry this often [guess], so a new
/// leader never reads more than this many entries back.
pub const CURSOR_STRIDE: u64 = 1_000;
/// Inline merge-pack bytes per `PrSync` command, under `COMMAND_MAX` with room for the rest of the command. A test
/// merge whose own objects pack larger is not recorded: its merge ref is deleted and `mergeable` stays null, as DESIGN
/// allows past the test-merge budget ("Shape", `TestMerge`) [guess: a merge commit and its new trees are a few KB].
pub const MERGE_BYTES_PER_COMMAND: usize = 900_000;
/// Entries one leader alarm reads for the dispatcher [guess]; the alarm comes back for the rest.
pub const SCAN_MAX: u64 = 2_000;
/// How soon after the leader applies entries its alarm runs the dispatcher [guess]: BF-24's 10 s holds the queue's
/// delivery, the test merge, and a commit round as well.
pub const ALARM_MS: i64 = 50;
/// The leader's alarm while the cursor trails a sent batch, so an overdue one is sent again [guess].
pub const WAIT_MS: i64 = 5_000;

/// One Queue message: the pull requests the entries `(from, through]` touched. `from` is the committed cursor when
/// the leader sent it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncBatch {
    pub from: Index,
    pub through: Index,
    pub prs: Vec<IssueNumber>,
}

/// The pull requests an entry asks `PrSync` to look at: a push to an open pull request's head branch, an open, or a
/// reopen. `PrSync`'s own events touch none of these, so its entries ask for nothing more. A base branch's move is
/// not here: DESIGN runs a test merge "on a head push, on Actions dispatch ..., and
/// on a single-PR GET" ("Shape", `TestMerge`), so a base move leaves `mergeable` visibly stale.
pub fn touched(core: &dyn Read, repo: RepoId, e: &LogEntry<RepoEvent>) -> Vec<IssueNumber> {
    let EntryBody::Events(evs) = &e.body else { return Vec::new() };
    let mut out = Vec::new();
    for ev in evs {
        match ev {
            RepoEvent::Refs { updates, .. } => {
                for u in updates.iter().filter(|u| u.new.is_some()) {
                    out.extend(pulls_from(core, repo, &u.name));
                }
            }
            RepoEvent::PrOpened { n, .. } => out.push(*n),
            RepoEvent::IssueChanged { n, change: IssueChange::State { open: true, .. }, .. } => out.push(*n),
            _ => {}
        }
    }
    out
}

/// How a batch goes out: its first attempt, which the leader runs in band, or a re-send of work the cursor has not
/// passed in `REDELIVER_AFTER_MS`, which goes through the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    First,
    Again,
}

/// The leader's half of the cursor, in memory only: `scanned` is the last entry read, `pending` the pull requests
/// each unhandled entry touched, `sent` the last batch's `through`, and `sent_at` when the work the cursor has not
/// passed was first handed out, or last sent again. Nothing here outlives the leadership; the committed cursor is
/// where a new one starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dispatcher {
    scanned: Index,
    pending: BTreeMap<Index, Vec<IssueNumber>>,
    sent: Index,
    sent_at: u64,
}

impl Dispatcher {
    /// The entries to read before `batch`: `(scanned, applied]`. A dispatcher behind the committed cursor, a new
    /// leader's, starts at the cursor; entries the cursor passed are dropped.
    pub fn to_read(&mut self, cursor: Index, applied: Index) -> Option<(Index, Index)> {
        self.scanned = self.scanned.max(cursor);
        self.pending = self.pending.split_off(&Index(cursor.0 + 1));
        (applied > self.scanned).then_some((self.scanned, applied))
    }

    /// Records what entry `index` touched. Entries come in order, right after `to_read`.
    pub fn read(&mut self, index: Index, prs: Vec<IssueNumber>) {
        debug_assert_eq!(index.0, self.scanned.0 + 1, "entries are read in order");
        self.scanned = index;
        if !prs.is_empty() {
            self.pending.insert(index, prs);
        }
    }

    /// The batch to send now, if any, and how. `Again` once work the cursor has not passed was handed out
    /// `REDELIVER_AFTER_MS` ago, however many batches went out since, so a first attempt that never returns is
    /// re-sent while pushes keep coming. Otherwise `First` when new entries touched a pull request, or when
    /// `CURSOR_STRIDE` entries passed the cursor, unless a first attempt is still `running`: the next one then waits
    /// for it and covers everything since. A batch always starts at the committed cursor and names every pull request
    /// after it, so no entry between the cursor and `through` goes unhandled, whoever led before.
    pub fn batch(&mut self, cursor: Index, now: Timestamp, running: bool) -> Option<(SyncBatch, Delivery)> {
        if self.scanned <= cursor {
            return None;
        }
        let outstanding = self.sent > cursor;
        let fresh = self.scanned > self.sent;
        let overdue = outstanding && now.0 >= self.sent_at.saturating_add(REDELIVER_AFTER_MS);
        let stride = self.scanned.0 - cursor.0 >= CURSOR_STRIDE;
        let touched = self.pending.range(Index(self.sent.0 + 1)..).next().is_some();
        let how = if overdue {
            Delivery::Again
        } else if fresh && (touched || stride) && !running {
            Delivery::First
        } else {
            return None;
        };
        if how == Delivery::Again || !outstanding {
            self.sent_at = now.0;
        }
        self.sent = self.scanned;
        let prs: BTreeSet<IssueNumber> = self.pending.values().flatten().copied().collect();
        Some((SyncBatch { from: cursor, through: self.scanned, prs: prs.into_iter().collect() }, how))
    }

    /// Whether the cursor still trails work, so the leader's alarm should come back for it.
    pub fn waiting(&self, cursor: Index) -> bool {
        self.sent > cursor || !self.pending.is_empty()
    }

    /// The last batch did not reach the queue, or its first attempt failed: its work is due again at once.
    pub fn unsent(&mut self) {
        self.sent_at = 0;
    }
}

/// The leader alarm's read: up to `SCAN_MAX` entries past what `d` has read, from the committed cursor on, each by
/// `entry`. Stops at an entry `entry` cannot give (`Err` names it), so nothing past a missing entry is read.
pub fn scan(
    d: &mut Dispatcher,
    core: &dyn Read,
    repo: RepoId,
    cursor: Index,
    applied: Index,
    mut entry: impl FnMut(Index) -> Option<LogEntry<RepoEvent>>,
) -> Result<(), Index> {
    let Some((from, to)) = d.to_read(cursor, applied) else { return Ok(()) };
    for i in from.0 + 1..=to.0.min(from.0 + SCAN_MAX) {
        let e = entry(Index(i)).ok_or(Index(i))?;
        d.read(Index(i), touched(core, repo, &e));
    }
    Ok(())
}

/// REST's `mergeable` (GraphQL `MERGEABLE`, `CONFLICTING`, or `UNKNOWN`): the recorded test merge, only while it is of
/// the pull request's current head and its base's current tip. Otherwise it is null until the next test merge, as
/// GHES 3.22 documents for one not yet computed.
pub fn mergeable(m: Option<&Mergeability>, head: ObjectId, base_tip: Option<ObjectId>) -> Option<bool> {
    let m = m.filter(|m| m.head == head && Some(m.base) == base_tip)?;
    Some(matches!(m.result, MergeabilityResult::Clean { .. }))
}

/// What the consumer reads of one pull request, through the leader's `fresh`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrView {
    pub n: IssueNumber,
    pub open: bool,
    /// The head is a branch of this repository. A fork's head is on the fork's chain, which `PrSync` does not read.
    pub same_repo: bool,
    pub head: ObjectId,
    pub base: RefName,
    pub base_tip: Option<ObjectId>,
    pub head_ref: Option<ObjectId>,
    pub merge: Option<Mergeability>,
}

impl PrView {
    pub fn mergeable(&self) -> Option<bool> {
        mergeable(self.merge.as_ref(), self.head, self.base_tip)
    }

    /// The test merge this pull request lacks: none, or one already recorded, for its (base tip, head).
    pub fn needs_merge(&self) -> Option<(ObjectId, ObjectId)> {
        let base = self.base_tip?;
        let current = self.merge.as_ref().is_some_and(|m| m.base == base && m.head == self.head);
        (self.open && self.same_repo && !current).then_some((base, self.head))
    }
}

/// The cursor and each pull request's view, from the core tier: the leader's answer to the consumer's read.
pub fn views(core: &dyn Read, repo: RepoId, prs: &[IssueNumber]) -> (Index, Vec<PrView>) {
    let views = prs
        .iter()
        .filter_map(|n| {
            let ph = pull_head(core, *n)?;
            Some(PrView {
                n: *n,
                open: matches!(ph.state, PrState::Open { .. }),
                same_repo: ph.head.repo == repo,
                head: ph.head_sha,
                base_tip: get_rec(core, "ref", &[ref_key(&ph.base)]),
                base: ph.base,
                head_ref: get_rec(core, "ref", &[ref_key(&head_ref(*n))]),
                merge: mergeability(core, *n),
            })
        })
        .collect();
    (consumer_cursor(core, Consumer::PrSync), views)
}

/// The leader's answer to the consumer's read (`{"q": {"prsync": [n..]}}`): the cursor, each pull request's view,
/// and the `mergeable` GraphQL renders for each. One type on both sides of the member route, so they cannot drift.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRead {
    pub cursor: Index,
    pub views: Vec<PrView>,
    pub mergeable: Vec<Option<bool>>,
}

impl SyncRead {
    pub fn of(core: &dyn Read, repo: RepoId, prs: &[IssueNumber]) -> SyncRead {
        let (cursor, views) = views(core, repo, prs);
        let mergeable = views.iter().map(PrView::mergeable).collect();
        SyncRead { cursor, views, mergeable }
    }
}

/// One test merge the git cache computed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestMerge {
    pub n: IssueNumber,
    pub base: ObjectId,
    pub head: ObjectId,
    pub result: MergeabilityResult,
    /// The merge commit's own objects, for a clean merge.
    pub pack: Option<PackRef>,
}

/// The git cache's answer to a test merge (`POST /merge` on the shard's container): a clean merge's commit and a pack of
/// its new objects, or a conflict's paths. Only the clean answer's pack enters the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MergeAnswer {
    Clean {
        merge: String,
        #[serde(with = "crate::pack::b64")]
        pack: Vec<u8>,
    },
    Conflict {
        conflict: Vec<String>,
    },
}

impl MergeAnswer {
    /// The test merge of pull request `n` at (base, head), or `None` when the answer names no object id. A conflict
    /// keeps its first four paths, as `MergeabilityResult::Conflict` holds.
    pub fn test_merge(self, n: IssueNumber, base: ObjectId, head: ObjectId) -> Option<TestMerge> {
        let (result, pack) = match self {
            MergeAnswer::Clean { merge, pack } => {
                let merge = ObjectId::parse_hex(merge.as_bytes())?;
                (MergeabilityResult::Clean { merge }, Some(PackRef::Inline(pack)))
            }
            MergeAnswer::Conflict { conflict } => {
                let paths = conflict.into_iter().take(4).map(Into::into).collect();
                (MergeabilityResult::Conflict { paths }, None)
            }
        };
        Some(TestMerge { n, base, head, result, pack })
    }
}

/// The command the consumer commits. `heads` moves `refs/pull/N/head` to each head; `merges` records each test
/// merge as `TestMerged`; `unmerged` names heads whose test merge could not be recorded, whose old merge ref is
/// deleted rather than left on an older head; `through`, on the batch's last command, moves the cursor. `from` is the
/// batch's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrSync {
    pub from: Index,
    pub through: Option<Index>,
    pub heads: Vec<(IssueNumber, ObjectId)>,
    pub merges: Vec<TestMerge>,
    pub unmerged: Vec<(IssueNumber, ObjectId)>,
    pub request: RequestId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncOut {
    /// Events this command committed: 0 for a batch the cursor had already passed.
    pub events: u32,
    pub cursor: Index,
}

/// A command's request id, from its content: a redelivered batch planned on the same fold re-sends the same commands
/// under the same ids, which their `request` rows answer, and a retry planned on a fold its first attempt changed
/// gets new ids, so no command is answered from a row of a different one. The top 16 bits are a tag no edge-minted
/// id has before the year 10889, since those hold milliseconds there.
pub fn request_id(c: &PrSync) -> RequestId {
    use sha2::Digest as _;
    let mut zeroed = c.clone();
    zeroed.request = RequestId(Ulid(0));
    let mut b = Vec::new();
    graftyard_fold::canon::write(&mut b, &zeroed);
    let h = sha2::Sha256::digest(&b);
    let low = u128::from_be_bytes(h[..16].try_into().expect("16 bytes")) & ((1u128 << 112) - 1);
    RequestId(Ulid((0x5059u128 << 112) | low))
}

/// The commands for a batch: the head moves and the test merges, at most `MERGE_BYTES_PER_COMMAND` of merge packs
/// each, the last moving the cursor. A merge whose pack alone is past the bound goes to `unmerged`.
pub fn commands(batch: &SyncBatch, views: &[PrView], merges: Vec<TestMerge>) -> Vec<PrSync> {
    let heads: Vec<(IssueNumber, ObjectId)> =
        views.iter().filter(|v| v.open && v.same_repo && v.head_ref != Some(v.head)).map(|v| (v.n, v.head)).collect();
    let bytes = |m: &TestMerge| match &m.pack {
        Some(PackRef::Inline(b)) => b.len(),
        _ => 0,
    };
    let mut unmerged = Vec::new();
    let mut chunks: Vec<Vec<TestMerge>> = vec![Vec::new()];
    let mut used = 0;
    for m in merges {
        let n = bytes(&m);
        if n > MERGE_BYTES_PER_COMMAND {
            unmerged.push((m.n, m.head));
            continue;
        }
        if used + n > MERGE_BYTES_PER_COMMAND {
            chunks.push(Vec::new());
            used = 0;
        }
        used += n;
        chunks.last_mut().expect("one chunk").push(m);
    }
    let last = chunks.len() - 1;
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, merges)| {
            let mut c = PrSync {
                from: batch.from,
                through: (i == last).then_some(batch.through),
                heads: if i == 0 { heads.clone() } else { Vec::new() },
                merges,
                unmerged: if i == 0 { unmerged.clone() } else { Vec::new() },
                request: RequestId(Ulid(0)),
            };
            c.request = request_id(&c);
            c
        })
        .collect()
}

impl PrSync {
    /// Pure: the events that still hold on the fold. A head move only to the pull request's current head; a test
    /// merge only of its current (base tip, head), and only once; the cursor only forward.
    pub fn decide_sync(&self, s: &RepoScratch<'_>) -> (SmallVec<[RepoEvent; 4]>, SyncOut) {
        let core = s.core();
        let cursor = consumer_cursor(core, Consumer::PrSync);
        if self.through.is_some_and(|t| t <= cursor) {
            return (SmallVec::new(), SyncOut { events: 0, cursor });
        }
        let open_head = |n: IssueNumber| {
            pull_head(core, n).filter(|ph| matches!(ph.state, PrState::Open { .. })).map(|ph| (ph.head_sha, ph.base))
        };
        let mut updates = Vec::new();
        for (n, new) in &self.heads {
            let name = head_ref(*n);
            let have = s.get(&name);
            if open_head(*n).is_some_and(|(h, _)| h == *new) && have != Some(*new) {
                updates.push(RefUpdate { name, expect: have, new: Some(*new) });
            }
        }
        for (n, head) in &self.unmerged {
            let name = merge_ref(*n);
            let have = s.get(&name);
            let recorded = mergeability(core, *n).is_some_and(|r| r.head == *head);
            if open_head(*n).is_some_and(|(h, _)| h == *head) && !recorded && have.is_some() {
                updates.push(RefUpdate { name, expect: have, new: None });
            }
        }
        let mut events: SmallVec<[RepoEvent; 4]> = SmallVec::new();
        if !updates.is_empty() {
            events.push(RepoEvent::Refs { push: self.request, actor: None, updates, pack: None });
        }
        // A merge of the pull request's current head is recorded even when its base has moved since: the row is keyed
        // by the SHA pair, so `mergeable` shows it stale rather than leaving the merge ref on an older head.
        for m in &self.merges {
            let Some((head, _)) = open_head(m.n) else { continue };
            let recorded = mergeability(core, m.n).is_some_and(|r| r.base == m.base && r.head == m.head);
            if head != m.head || recorded {
                continue;
            }
            events.push(RepoEvent::TestMerged {
                n: m.n,
                base: m.base,
                head: m.head,
                result: m.result.clone(),
                pack: m.pack.clone(),
            });
        }
        let mut next = cursor;
        if let Some(t) = self.through {
            events.push(RepoEvent::ConsumerCursor { consumer: Consumer::PrSync, through: t });
            next = t;
        }
        let n = events.len() as u32;
        (events, SyncOut { events: n, cursor: next })
    }
}

impl Record for PrSync {
    fn encode(&self, out: &mut Vec<u8>) {
        graftyard_fold::canon::write(out, self);
    }
}

/// `PrSync` alone as a command; a member hosts it inside `crate::repo::RepoCmd`.
impl Command for PrSync {
    type Fold = RepoFold;
    type Dep = graftyard_identity::IdentityDep;
    type Out = SyncOut;
    type Err = std::convert::Infallible;

    fn format(&self) -> Format {
        REPO_FORMAT_V3
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    /// A consumer's command names no credential (`design/DESIGN.md`, Shape, `CredentialShard`).
    fn deps(&self, _fold: &RepoFold) -> SmallVec<[ChainId; 3]> {
        SmallVec::new()
    }
    fn decide(
        &self,
        s: &RepoScratch<'_>,
        _deps: &[(ChainId, Self::Dep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, SyncOut), Self::Err> {
        Ok(self.decide_sync(s))
    }
    fn replay(&self, events: &[RepoEvent]) -> SyncOut {
        let cursor = events
            .iter()
            .find_map(|e| match e {
                RepoEvent::ConsumerCursor { through, .. } => Some(*through),
                _ => None,
            })
            .unwrap_or(self.from);
        SyncOut { events: events.len() as u32, cursor }
    }
}
