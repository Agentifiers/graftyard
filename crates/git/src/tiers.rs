//! The repository chain's two fold tiers (`design/SHARED.md`, "Folds"; `build/BUILD-PLAN.md`, M2-WP1 and M2-WP2):
//! `tier_of`, each tier's tables, and what each event writes.
//!
//! Core holds what git, merges, and authorization read: `ref`, `rule`, `settings`, `counter` (the issue-number
//! counter), `pull_head` and its two indexes `pull_by_head` and `open_head`, `review_verdict` and its index
//! `verdict_by_review`, `required_check`, and the count of unresolved review threads per pull request (`unresolved`,
//! with `review_thread` mapping each thread to its pull request), which a rule requiring conversation resolution
//! reads. Metadata holds `issue` with its indexes `issue_order` (kind, open, newest first) and `issue_by_id`,
//! `issue_label`, `issue_assignee`, `label`, `pull`, `review` and its index by pull
//! request `review_by_pull`, `review_request`, `comment` and its index by issue `comment_by_issue`, `check_run` and
//! `check_suite` keyed by head SHA first, `status` keyed by SHA and newest first, `check_expiry` (the pruning index),
//! and `retention` (`RepoSettings.check_retention_days`). Each tier also keeps `ids`, its own `DbId` cursor
//! (`design/DESIGN.md`, Shape, "Chains": "Each fold tier keeps its own `IdBlock` cursor"), over its own half of every
//! block (`tier_block`), so the two never assign the same id, before a `MetaSplit` or after it, when the parent's core
//! and the child's metadata tier assign alone. An event that touches Core takes its id from Core, because after a split
//! its entry commits on the parent; an event that touches only Metadata takes it from Metadata. A row's record is one
//! postcard blob; its key columns are real columns a host indexes.
//!
//! Required checks are pruned in Core when their pull request closes or its head moves, never by time. Check runs,
//! suites, and statuses are pruned in Metadata once an entry's `at` passes the row's time (the `at` of the entry that
//! last wrote it) plus the retention, so every member prunes the same rows.

use graftyard_fold::{Read, Rows, Schema, TableDef, Tiered, Val, has};
use graftyard_identity::IdCursor;
use graftyard_raft::TierSet;
use graftyard_types::{AccountId, DbId, IdBlock, Index, RepoId, Tier, Timestamp};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::blob::BlobRef;
use crate::issues::{AssetRecord, LfsLockEvent, ReleaseRecord, RepoReleaseId};
use crate::issues::{
    CHECK_RETENTION_DEFAULT, CheckConclusion, CheckRunUpdate, CheckSource, CheckSuiteUpdate, CommentEvent,
    CommentTarget, Consumer, Grantee, HeadSpec, IssueChange, IssueKind, IssueNumber, LabelEvent, LabelName, LockReason,
    Mergeability, MergeabilityResult, PrState, PullHead, ReviewEvent, ReviewState, StateReason, StatusUpdate,
};
use crate::refs::{ObjectId, RefName};
use crate::repo::RepoEvent;

const DAY_MS: u64 = 86_400_000;

/// The tiers an event touches. Every event the design lists for a tier is here; an `IssueChanged` that changes a
/// state or draft flag touches Core too whether or not `n` is a pull request, because the event alone cannot say.
pub fn tier_of(e: &RepoEvent) -> TierSet {
    let both = TierSet::CORE.union(TierSet::METADATA);
    match e {
        RepoEvent::Refs { .. } | RepoEvent::Refused { .. } | RepoEvent::Promoted(_) | RepoEvent::Archived(_) => {
            TierSet::CORE
        }
        RepoEvent::RequiredCheck { .. } => TierSet::CORE,
        RepoEvent::Rule(_) => TierSet::CORE,
        RepoEvent::IssueOpened { .. } | RepoEvent::PrOpened { .. } | RepoEvent::PrClosed { .. } => both,
        RepoEvent::Settings(_) => both,
        RepoEvent::IssueChanged { change: IssueChange::State { .. } | IssueChange::Draft(_), .. } => both,
        RepoEvent::IssueChanged { .. } => TierSet::METADATA,
        RepoEvent::Review(ReviewEvent::Submitted { .. } | ReviewEvent::Dismissed { .. }) => both,
        RepoEvent::Review(ReviewEvent::Requested { .. }) => TierSet::METADATA,
        // The core count of unresolved review threads per pull request (`review_thread` and `unresolved`). SHARED's
        // `tier_of` says "Comment only Metadata, except a `Resolved`, which touches both", but `Resolved` names only
        // the thread, so Core must also see the comment that starts a thread (it is unresolved from then, and its row
        // maps the thread to its pull request) and every deletion (the event does not say whether it named a thread).
        RepoEvent::Comment(
            CommentEvent::Created { on: CommentTarget::Review { .. }, reply_to: None, .. }
            | CommentEvent::Resolved { .. }
            | CommentEvent::Deleted { .. },
        ) => both,
        RepoEvent::Comment(_) | RepoEvent::Labels(_) => TierSet::METADATA,
        RepoEvent::CheckRun(_) | RepoEvent::CheckSuite(_) | RepoEvent::Status(_) => TierSet::METADATA,
        RepoEvent::Release { tag: Some(_), .. } => both,
        RepoEvent::Release { tag: None, .. } => TierSet::METADATA,
        RepoEvent::ReleaseAsset { .. } | RepoEvent::LfsObject { .. } | RepoEvent::LfsLock(_) => TierSet::METADATA,
        RepoEvent::TestMerged { .. } | RepoEvent::ConsumerCursor { .. } => TierSet::CORE,
    }
}

const fn t(name: &'static str, key: &'static [&'static str]) -> TableDef {
    TableDef { name, key }
}

pub const CORE: Schema = Schema {
    tier: Tier::Core,
    tables: &[
        t("counter", &[]),
        t("ids", &[]),
        t("open_head", &["sha", "n"]),
        t("pull_by_head", &["repo", "branch", "n"]),
        t("pull_head", &["n"]),
        t("ref", &["name"]),
        t("required_check", &["sha", "name", "source"]),
        t("review_thread", &["thread"]),
        t("review_verdict", &["n", "reviewer"]),
        t("rule", &["id"]),
        t("settings", &[]),
        t("unresolved", &["n"]),
        t("verdict_by_review", &["review"]),
        // The last `Promoted` and `Archived` of a log range, and each tier's committed snapshots by (tier, to), which
        // `verify` and the snapshot writer read.
        t("promoted", &[]),
        t("archived", &[]),
        t("snapshot", &["tier", "to"]),
        // Each pull request's last test merge (`Mergeability`, keyed by the SHA pair it holds), and each consumer's
        // cursor on this chain.
        t("mergeability", &["n"]),
        t("consumer_cursor", &["consumer"]),
    ],
};

pub const METADATA: Schema = Schema {
    tier: Tier::Metadata,
    tables: &[
        t("check_expiry", &["at", "kind", "key"]),
        t("check_run", &["sha", "run"]),
        t("check_suite", &["sha", "suite"]),
        t("comment", &["id"]),
        t("comment_by_issue", &["n", "id"]),
        t("ids", &[]),
        t("issue", &["n"]),
        t("issue_by_id", &["id"]),
        t("issue_assignee", &["n", "account"]),
        t("issue_label", &["n", "label"]),
        t("issue_order", &["kind", "open", "rev_n"]),
        t("label", &["id"]),
        t("pull", &["n"]),
        t("retention", &[]),
        t("review", &["id"]),
        t("review_by_pull", &["n", "id"]),
        t("review_request", &["n", "grantee"]),
        t("status", &["sha", "rev_id"]),
        // Releases keyed by id negated, so a forward scan lists the newest first; `release_by_tag` maps a tag to its
        // release. Assets by id, with `asset_by_release`. LFS objects by oid, locks by id with `lfs_lock_by_path`.
        t("release", &["rev_id"]),
        t("release_by_tag", &["tag"]),
        t("asset", &["id"]),
        t("asset_by_release", &["release", "id"]),
        t("lfs_object", &["oid"]),
        t("lfs_lock", &["id"]),
        t("lfs_lock_by_path", &["path"]),
    ],
};

// ---- rows ----

pub fn int(v: i64) -> Val {
    Val::Int(v)
}
pub fn n_key(n: IssueNumber) -> Val {
    Val::Int(i64::from(n.0))
}
pub fn sha_key(o: &ObjectId) -> Val {
    Val::Blob(o.0.to_vec().into())
}
pub fn ref_key(r: &RefName) -> Val {
    Val::Blob(r.as_bytes().to_vec().into())
}
fn text(s: &str) -> Val {
    Val::Text(s.into())
}

/// The core `required_check` key's third column: which source reports the name, so a re-run by the same App or a
/// new status from the same creator replaces the row it supersedes.
pub(crate) fn source_key(s: &CheckSource) -> Val {
    match s {
        CheckSource::Run { app, .. } => Val::Text(format!("app:{}", app.0.0).into()),
        CheckSource::Status { creator } => Val::Text(format!("status:{}", creator.0.0).into()),
    }
}

pub fn get_rec<T: DeserializeOwned>(r: &(impl Read + ?Sized), table: &str, key: &[Val]) -> Option<T> {
    match r.get(table, key)?.first() {
        Some(Val::Blob(b)) => Some(postcard::from_bytes(b).expect("a fold row this release wrote")),
        _ => panic!("row {table} has no record"),
    }
}

fn put_rec<T: Serialize>(w: &mut dyn Rows, table: &str, key: Vec<Val>, rec: &T) {
    let b = postcard::to_allocvec(rec).expect("a fold row encodes");
    w.put(table, key, vec![Val::Blob(b.into())]);
}

fn mark(w: &mut dyn Rows, table: &str, key: Vec<Val>) {
    w.put(table, key, Vec::new());
}

/// The core `required_check` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct RequiredRow {
    pub source: CheckSource,
    pub version: u64,
    pub conclusion: Option<CheckConclusion>,
}

impl RequiredRow {
    /// Whether `new` replaces this row of the same (sha, name, source). Run and status ids are `DbId`s a group assigns
    /// in increasing order, so a re-run's later run, or a later status, wins over an older one even when the older one's
    /// update commits last; within one run the higher version wins, so a repeat changes nothing. A status's `version`
    /// is its `StatusId`.
    pub(crate) fn superseded_by(&self, new: &RequiredRow) -> bool {
        let order = |r: &RequiredRow| match r.source {
            CheckSource::Run { run, .. } => (run.0.0, r.version),
            CheckSource::Status { .. } => (r.version as i64, 0),
        };
        order(new) > order(self)
    }
}

/// The core `review_verdict` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct VerdictRow {
    pub review: crate::issues::ReviewId,
    pub state: ReviewState,
    pub commit: ObjectId,
}

/// The metadata `issue` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct IssueRow {
    pub id: graftyard_types::DbId,
    pub kind: IssueKind,
    pub open: bool,
    pub author: Option<AccountId>,
    pub title: Box<str>,
    pub body: Box<str>,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub closed: Option<Timestamp>,
    pub comments: u32,
    pub locked: Option<LockReason>,
    /// The reason of the last `State` change, which GraphQL's `stateReason` reads.
    pub reason: Option<StateReason>,
}

/// The metadata `pull` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct PullRow {
    pub head: HeadSpec,
    pub base: RefName,
    pub head_sha: ObjectId,
    pub base_sha: ObjectId,
    pub draft: bool,
}

/// The metadata `comment` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct CommentRow {
    pub on: CommentTarget,
    pub author: Option<AccountId>,
    pub body: Box<str>,
    pub reply_to: Option<crate::issues::CommentId>,
    pub resolved: bool,
    pub created: Timestamp,
    pub updated: Timestamp,
}

/// The metadata `review` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ReviewRow {
    pub n: IssueNumber,
    pub reviewer: Option<AccountId>,
    pub state: ReviewState,
    pub commit: ObjectId,
    pub body: Box<str>,
    pub dismissed: Option<Box<str>>,
    pub at: Timestamp,
}

/// The metadata `label` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct LabelRow {
    pub name: LabelName,
    pub color: [u8; 3],
    pub description: Option<Box<str>>,
}

/// The metadata `release` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ReleaseRow {
    pub rec: ReleaseRecord,
    pub created: Timestamp,
    /// When it was first published, `None` while a draft.
    pub published: Option<Timestamp>,
}

/// The metadata `asset` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AssetRow {
    pub release: RepoReleaseId,
    pub rec: AssetRecord,
    pub uploader: Option<AccountId>,
    pub created: Timestamp,
    pub updated: Timestamp,
}

/// The metadata `lfs_object` row: the blob a `verify` committed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct LfsRow {
    pub blob: BlobRef,
    pub at: Timestamp,
}

/// The metadata `lfs_lock` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct LockRow {
    pub path: Box<str>,
    pub refspec: Option<RefName>,
    pub owner: AccountId,
    pub at: Timestamp,
}

pub fn release_key(id: RepoReleaseId) -> Val {
    int(-id.0.0)
}

pub fn release_row(r: &(impl Read + ?Sized), id: RepoReleaseId) -> Option<ReleaseRow> {
    get_rec(r, "release", &[release_key(id)])
}

pub fn release_by_tag(r: &(impl Read + ?Sized), tag: &RefName) -> Option<RepoReleaseId> {
    get_rec::<i64>(r, "release_by_tag", &[ref_key(tag)]).map(|i| RepoReleaseId(DbId(i)))
}

pub fn lfs_key(oid: &graftyard_types::Digest) -> Val {
    Val::Blob(oid.0.to_vec().into())
}

/// A metadata check row: the entry time that last wrote it, which pruning reads, and the record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Timed<T> {
    pub at: Timestamp,
    pub rec: T,
}

const KIND_RUN: i64 = 0;
const KIND_SUITE: i64 = 1;
const KIND_STATUS: i64 = 2;

fn check_table(kind: i64) -> &'static str {
    match kind {
        KIND_RUN => "check_run",
        KIND_SUITE => "check_suite",
        _ => "status",
    }
}

/// The `check_expiry` key of a row of `kind` written at `at`: its key columns ride in one blob.
fn expiry_key(at: Timestamp, kind: i64, key: &[Val]) -> Vec<Val> {
    let b = postcard::to_allocvec(key).expect("a key encodes");
    vec![int(at.0 as i64), int(kind), Val::Blob(b.into())]
}

/// Writes a metadata check row stamped with `at`, moving its `check_expiry` row from its previous time.
fn put_timed<T: Serialize + DeserializeOwned>(w: &mut dyn Rows, kind: i64, key: Vec<Val>, at: Timestamp, rec: T) {
    let table = check_table(kind);
    if let Some(old) = get_rec::<Timed<T>>(w, table, &key) {
        w.delete("check_expiry", &expiry_key(old.at, kind, &key));
    }
    mark(w, "check_expiry", expiry_key(at, kind, &key));
    put_rec(w, table, key, &Timed { at, rec });
}

// ---- reads shared by apply and decide ----

pub fn pull_head(r: &(impl Read + ?Sized), n: IssueNumber) -> Option<PullHead> {
    get_rec(r, "pull_head", &[n_key(n)])
}

/// Open pull requests whose head is `branch` of `repo`.
pub(crate) fn pulls_from(r: &(impl Read + ?Sized), repo: RepoId, branch: &RefName) -> Vec<IssueNumber> {
    r.prefix("pull_by_head", &[int(repo.0.0), ref_key(branch)])
        .into_iter()
        .filter_map(|(k, _)| match k.get(2) {
            Some(Val::Int(n)) => Some(IssueNumber(*n as u32)),
            _ => None,
        })
        .collect()
}

/// The core `review_thread` row: the pull request a review thread is on, and whether it is resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ThreadRow {
    pub n: IssueNumber,
    pub resolved: bool,
}

/// The core count of unresolved review threads on pull request `n`, which `check_protection` reads when a rule sets
/// `conversation_resolution`.
pub fn unresolved_threads(core: &(impl Read + ?Sized), n: IssueNumber) -> u32 {
    get_rec(core, "unresolved", &[n_key(n)]).unwrap_or(0)
}

/// Adds `by` to `n`'s unresolved count, keeping no row for zero.
fn count_unresolved(w: &mut dyn Rows, n: IssueNumber, by: i32) {
    let c = unresolved_threads(w, n).checked_add_signed(by).expect("an unresolved count never goes below zero");
    if c == 0 { w.delete("unresolved", &[n_key(n)]) } else { put_rec(w, "unresolved", vec![n_key(n)], &c) }
}

/// The tier an event's ids come from: Core when the event touches Core, else Metadata.
pub fn id_tier(set: TierSet) -> Tier {
    if has(set, Tier::Core) { Tier::Core } else { Tier::Metadata }
}

/// `tier`'s half of a leased block: Metadata assigns its lower half, Core its upper half, so the tiers' cursors never
/// meet. `None` for Metadata's half of a one-id block.
pub fn tier_block(block: IdBlock, tier: Tier) -> Option<IdBlock> {
    let low = block.len() / 2;
    match tier {
        Tier::Metadata => IdBlock::new(block.base(), low),
        Tier::Core => IdBlock::new(DbId(block.base().0 + i64::from(low)), block.len() - low),
    }
}

/// A tier's own `DbId` cursor.
pub(crate) fn ids(r: &(impl Read + ?Sized)) -> IdCursor {
    get_rec(r, "ids", &[]).unwrap_or_default()
}

/// Records that `ev`'s write assigned `id`, in the tier `id_tier` gives it, so only that tier's cursor moves.
fn assigned(w: &mut dyn Rows, ev: &RepoEvent, tier: Tier, id: DbId) {
    if id_tier(tier_of(ev)) != tier {
        return;
    }
    let mut c = ids(w);
    c.apply_assigned(id);
    put_rec(w, "ids", Vec::new(), &c);
}

fn retention_days(r: &(impl Read + ?Sized)) -> u16 {
    get_rec(r, "retention", &[]).unwrap_or(CHECK_RETENTION_DEFAULT)
}

/// A tier's snapshot rows, `(to, entry time at to, archived record)`, in index order.
pub fn snapshots(
    r: &(impl Read + ?Sized),
    tier: Tier,
) -> Vec<(graftyard_types::Index, Timestamp, crate::pack::Archived)> {
    r.prefix("snapshot", &[tier_key(tier)])
        .into_iter()
        .filter_map(|(k, _)| get_rec::<crate::pack::Archived>(r, "snapshot", &k))
        .map(|a| (a.to, a.to_at, a))
        .collect()
}

fn tier_key(t: Tier) -> Val {
    int(match t {
        Tier::Core => 0,
        Tier::Metadata => 1,
    })
}

/// A committed snapshot of `tier` at `to`: its row, then every row `snapshots_to_keep` no longer keeps at this
/// entry's time, so each tier keeps one snapshot at or below every index inside `RESTORE_WINDOW` and every member
/// drops the same rows. The snapshot's objects in R2 go once their row has (`workers/site`).
fn snapshot_committed(w: &mut dyn Rows, tier: Tier, to: graftyard_types::Index, a: &crate::pack::Archived) {
    put_rec(w, "snapshot", vec![tier_key(tier), int(to.0 as i64)], a);
    let all = snapshots(w, tier);
    let keep = graftyard_fold::snapshots_to_keep(&all.iter().map(|(i, t, _)| (*i, *t)).collect::<Vec<_>>(), a.to_at);
    for (i, _, _) in all {
        if !keep.contains(&i) {
            w.delete("snapshot", &[tier_key(tier), int(i.0 as i64)]);
        }
    }
}

// ---- writes ----

/// The repository chain's tiers. `repo` is the chain's own repository, which a same-repository head belongs to, and
/// `owner` the account whose `Authz` gates it until the first `Settings` entry names one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepoTiers {
    pub repo: RepoId,
    pub owner: AccountId,
}

/// Drops every required check of `sha` unless another open pull request still has it as its head.
fn prune_required(w: &mut dyn Rows, sha: &ObjectId) {
    if !w.prefix("open_head", &[sha_key(sha)]).is_empty() {
        return;
    }
    for (k, _) in w.prefix("required_check", &[sha_key(sha)]) {
        w.delete("required_check", &k);
    }
}

/// Moves open pull request `n`'s head to `new`, pruning the old head's required checks.
fn move_head(w: &mut dyn Rows, n: IssueNumber, new: ObjectId) {
    let Some(mut ph) = pull_head(w, n) else { return };
    if !matches!(ph.state, PrState::Open { .. }) || ph.head_sha == new {
        return;
    }
    let old = ph.head_sha;
    w.delete("open_head", &[sha_key(&old), n_key(n)]);
    mark(w, "open_head", vec![sha_key(&new), n_key(n)]);
    ph.head_sha = new;
    put_rec(w, "pull_head", vec![n_key(n)], &ph);
    prune_required(w, &old);
}

/// Opens or reopens `n` in Core at `head_sha`.
fn open_core(w: &mut dyn Rows, n: IssueNumber, ph: &PullHead) {
    mark(w, "open_head", vec![sha_key(&ph.head_sha), n_key(n)]);
    mark(w, "pull_by_head", vec![int(ph.head.repo.0.0), ref_key(&ph.head.branch), n_key(n)]);
    put_rec(w, "pull_head", vec![n_key(n)], ph);
}

/// Closes `n` in Core: it leaves the head indexes and its head's required checks are pruned.
fn close_core(w: &mut dyn Rows, n: IssueNumber, state: PrState) {
    let Some(mut ph) = pull_head(w, n) else { return };
    if !matches!(ph.state, PrState::Open { .. }) {
        return;
    }
    w.delete("open_head", &[sha_key(&ph.head_sha), n_key(n)]);
    w.delete("pull_by_head", &[int(ph.head.repo.0.0), ref_key(&ph.head.branch), n_key(n)]);
    ph.state = state;
    put_rec(w, "pull_head", vec![n_key(n)], &ph);
    prune_required(w, &ph.head_sha);
}

/// `refs/pull/{n}/head`.
pub fn head_ref(n: IssueNumber) -> RefName {
    RefName::parse(format!("refs/pull/{}/head", n.0).as_bytes()).expect("a pull ref is a ref name")
}

/// `refs/pull/{n}/merge`, which a `TestMerged` moves.
pub fn merge_ref(n: IssueNumber) -> RefName {
    RefName::parse(format!("refs/pull/{}/merge", n.0).as_bytes()).expect("a pull ref is a ref name")
}

/// Pull request `n`'s last test merge.
pub fn mergeability(r: &(impl Read + ?Sized), n: IssueNumber) -> Option<Mergeability> {
    get_rec(r, "mergeability", &[n_key(n)])
}

/// Every event at or below this index is handled by `consumer`; 0 before its first cursor entry.
pub fn consumer_cursor(r: &(impl Read + ?Sized), consumer: Consumer) -> Index {
    get_rec(r, "consumer_cursor", &[int(consumer.tag())]).unwrap_or_default()
}

/// `refs/pull/{n}/head`'s number.
fn pull_ref(name: &RefName) -> Option<IssueNumber> {
    let s = std::str::from_utf8(name.as_bytes()).ok()?;
    let n = s.strip_prefix("refs/pull/")?.strip_suffix("/head")?;
    n.parse().ok().filter(|v: &u32| *v > 0 && n == v.to_string()).map(IssueNumber)
}

impl RepoTiers {
    fn write_core(&self, ev: &RepoEvent, at: Timestamp, w: &mut dyn Rows) {
        match ev {
            RepoEvent::Refs { updates, .. } => {
                for u in updates {
                    match u.new {
                        Some(o) => put_rec(w, "ref", vec![ref_key(&u.name)], &o),
                        None => w.delete("ref", &[ref_key(&u.name)]),
                    }
                    let Some(new) = u.new else { continue };
                    if let Some(n) = pull_ref(&u.name) {
                        move_head(w, n, new);
                    }
                    for n in pulls_from(w, self.repo, &u.name) {
                        move_head(w, n, new);
                    }
                }
            }
            RepoEvent::Refused { .. } => {}
            RepoEvent::Promoted(p) => put_rec(w, "promoted", Vec::new(), p),
            RepoEvent::Archived(a) => match &a.key {
                crate::key::R2Key::Snapshot { tier, to, .. } => snapshot_committed(w, *tier, *to, a),
                _ => put_rec(w, "archived", Vec::new(), a),
            },
            RepoEvent::IssueOpened { n, capture } => {
                assigned(w, ev, Tier::Core, capture.id);
                put_rec(w, "counter", Vec::new(), &n.0)
            }
            RepoEvent::PrOpened { n, head, base, capture } => {
                assigned(w, ev, Tier::Core, capture.issue.id);
                put_rec(w, "counter", Vec::new(), &n.0);
                let ph = PullHead {
                    head: head.clone(),
                    base: base.clone(),
                    head_sha: capture.head_sha,
                    state: capture.state,
                };
                open_core(w, *n, &ph);
            }
            RepoEvent::PrClosed { n, final_head } => close_core(w, *n, PrState::Closed { at, final_head: *final_head }),
            RepoEvent::IssueChanged { n, change, .. } => {
                let Some(mut ph) = pull_head(w, *n) else { return };
                match (change, ph.state) {
                    (IssueChange::State { open: true, .. }, PrState::Closed { .. }) => {
                        // A same-repository head reopens at its branch's tip, which may have moved while closed.
                        if ph.head.repo == self.repo
                            && let Some(tip) = get_rec::<ObjectId>(w, "ref", &[ref_key(&ph.head.branch)])
                        {
                            ph.head_sha = tip;
                        }
                        ph.state = PrState::Open { draft: false };
                        open_core(w, *n, &ph);
                    }
                    (IssueChange::State { open: false, .. }, PrState::Open { .. }) => {
                        close_core(w, *n, PrState::Closed { at, final_head: ph.head_sha })
                    }
                    (IssueChange::Draft(d), PrState::Open { .. }) => {
                        ph.state = PrState::Open { draft: *d };
                        put_rec(w, "pull_head", vec![n_key(*n)], &ph);
                    }
                    _ => {}
                }
            }
            RepoEvent::Review(ReviewEvent::Submitted { id, n, reviewer, state, commit, .. }) => {
                assigned(w, ev, Tier::Core, id.0);
                if *state == ReviewState::Pending {
                    return;
                }
                let who = int(reviewer.map_or(0, |a| a.0.0));
                if let Some(old) = get_rec::<VerdictRow>(w, "review_verdict", &[n_key(*n), who.clone()]) {
                    w.delete("verdict_by_review", &[int(old.review.0.0)]);
                }
                put_rec(
                    w,
                    "review_verdict",
                    vec![n_key(*n), who.clone()],
                    &VerdictRow { review: *id, state: *state, commit: *commit },
                );
                put_rec(w, "verdict_by_review", vec![int(id.0.0)], &(n.0, reviewer.map_or(0, |a| a.0.0)));
            }
            RepoEvent::Review(ReviewEvent::Dismissed { id, .. }) => {
                if let Some((n, who)) = get_rec::<(u32, i64)>(w, "verdict_by_review", &[int(id.0.0)]) {
                    w.delete("verdict_by_review", &[int(id.0.0)]);
                    w.delete("review_verdict", &[n_key(IssueNumber(n)), int(who)]);
                }
            }
            RepoEvent::Review(ReviewEvent::Requested { .. }) => {}
            RepoEvent::Rule(r) => {
                assigned(w, ev, Tier::Core, r.id);
                match &r.protection {
                    Some(_) => put_rec(w, "rule", vec![int(r.id.0)], r),
                    None => w.delete("rule", &[int(r.id.0)]),
                }
            }
            RepoEvent::Settings(s) => put_rec(w, "settings", Vec::new(), s),
            RepoEvent::RequiredCheck { sha, name, source, version, conclusion } => {
                let key = vec![sha_key(sha), text(name), source_key(source)];
                let row = RequiredRow { source: *source, version: *version, conclusion: *conclusion };
                let newer = get_rec::<RequiredRow>(w, "required_check", &key).is_none_or(|o| o.superseded_by(&row));
                if newer {
                    put_rec(w, "required_check", key, &row);
                }
            }
            RepoEvent::Comment(CommentEvent::Created {
                id,
                on: CommentTarget::Review { n, .. },
                reply_to: None,
                ..
            }) => {
                assigned(w, ev, Tier::Core, id.0);
                put_rec(w, "review_thread", vec![int(id.0.0)], &ThreadRow { n: *n, resolved: false });
                count_unresolved(w, *n, 1);
            }
            RepoEvent::Comment(CommentEvent::Resolved { thread, resolved, .. }) => {
                let key = vec![int(thread.0.0)];
                if let Some(mut t) = get_rec::<ThreadRow>(w, "review_thread", &key)
                    && t.resolved != *resolved
                {
                    t.resolved = *resolved;
                    put_rec(w, "review_thread", key, &t);
                    count_unresolved(w, t.n, if *resolved { -1 } else { 1 });
                }
            }
            RepoEvent::Comment(CommentEvent::Deleted { id, .. }) => {
                if let Some(t) = get_rec::<ThreadRow>(w, "review_thread", &[int(id.0.0)]) {
                    w.delete("review_thread", &[int(id.0.0)]);
                    if !t.resolved {
                        count_unresolved(w, t.n, -1);
                    }
                }
            }
            RepoEvent::Comment(_) | RepoEvent::Labels(_) => {}
            RepoEvent::CheckRun(_) | RepoEvent::CheckSuite(_) | RepoEvent::Status(_) => {}
            RepoEvent::Release { id, tag, .. } => {
                assigned(w, ev, Tier::Core, id.0);
                if let Some(u) = tag {
                    match u.new {
                        Some(o) => put_rec(w, "ref", vec![ref_key(&u.name)], &o),
                        None => w.delete("ref", &[ref_key(&u.name)]),
                    }
                }
            }
            RepoEvent::ReleaseAsset { .. } | RepoEvent::LfsObject { .. } | RepoEvent::LfsLock(_) => {}
            // `PrSync::decide_sync` commits one only for an open pull request's current head.
            RepoEvent::TestMerged { n, base, head, result, .. } => {
                let m = Mergeability { base: *base, head: *head, result: result.clone() };
                put_rec(w, "mergeability", vec![n_key(*n)], &m);
                let merge_ref = merge_ref(*n);
                match result {
                    MergeabilityResult::Clean { merge } => put_rec(w, "ref", vec![ref_key(&merge_ref)], merge),
                    MergeabilityResult::Conflict { .. } => w.delete("ref", &[ref_key(&merge_ref)]),
                }
            }
            RepoEvent::ConsumerCursor { consumer, through } => {
                let key = vec![int(consumer.tag())];
                if get_rec::<Index>(w, "consumer_cursor", &key).is_none_or(|c| c < *through) {
                    put_rec(w, "consumer_cursor", key, through);
                }
            }
        }
    }

    fn write_meta(&self, ev: &RepoEvent, at: Timestamp, w: &mut dyn Rows) {
        let issue = |w: &dyn Rows, n: IssueNumber| get_rec::<IssueRow>(w, "issue", &[n_key(n)]);
        let touch = |w: &mut dyn Rows, n: IssueNumber, f: &dyn Fn(&mut IssueRow)| {
            if let Some(mut row) = get_rec::<IssueRow>(w, "issue", &[n_key(n)]) {
                let was = row.open;
                f(&mut row);
                row.updated = at;
                if was != row.open {
                    w.delete("issue_order", &order_key(row.kind, was, n));
                    mark(w, "issue_order", order_key(row.kind, row.open, n));
                }
                put_rec(w, "issue", vec![n_key(n)], &row);
            }
        };
        match ev {
            RepoEvent::IssueOpened { n, capture }
            | RepoEvent::PrOpened { n, capture: crate::issues::PrCapture { issue: capture, .. }, .. } => {
                let kind =
                    if matches!(ev, RepoEvent::PrOpened { .. }) { IssueKind::PullRequest } else { IssueKind::Issue };
                assigned(w, ev, Tier::Metadata, capture.id);
                put_rec(
                    w,
                    "issue",
                    vec![n_key(*n)],
                    &IssueRow {
                        id: capture.id,
                        kind,
                        open: true,
                        author: capture.author,
                        title: capture.title.clone(),
                        body: capture.body.clone(),
                        created: at,
                        updated: at,
                        closed: None,
                        comments: 0,
                        locked: None,
                        reason: None,
                    },
                );
                mark(w, "issue_order", order_key(kind, true, *n));
                put_rec(w, "issue_by_id", vec![int(capture.id.0)], &n.0);
                for l in &capture.labels {
                    mark(w, "issue_label", vec![n_key(*n), int(l.0.0)]);
                }
                for a in &capture.assignees {
                    mark(w, "issue_assignee", vec![n_key(*n), int(a.0.0)]);
                }
                if let RepoEvent::PrOpened { head, base, capture: c, .. } = ev {
                    let draft = matches!(c.state, PrState::Open { draft: true });
                    put_rec(
                        w,
                        "pull",
                        vec![n_key(*n)],
                        &PullRow {
                            head: head.clone(),
                            base: base.clone(),
                            head_sha: c.head_sha,
                            base_sha: c.base_sha,
                            draft,
                        },
                    );
                }
            }
            RepoEvent::PrClosed { n, .. } => touch(w, *n, &|r| {
                r.open = false;
                r.closed = Some(at);
            }),
            RepoEvent::IssueChanged { n, change, .. } => match change {
                IssueChange::Title(t) => touch(w, *n, &|r| r.title = t.clone()),
                IssueChange::Body(b) => touch(w, *n, &|r| r.body = b.clone()),
                IssueChange::State { open, reason } => touch(w, *n, &|r| {
                    r.open = *open;
                    r.closed = (!open).then_some(at);
                    r.reason = *reason;
                }),
                IssueChange::Locked(l) => touch(w, *n, &|r| r.locked = *l),
                IssueChange::Draft(d) => {
                    if let Some(mut p) = get_rec::<PullRow>(w, "pull", &[n_key(*n)]) {
                        p.draft = *d;
                        put_rec(w, "pull", vec![n_key(*n)], &p);
                        touch(w, *n, &|_| {});
                    }
                }
            },
            RepoEvent::Comment(c) => match c {
                CommentEvent::Created { id, on, author, body, reply_to } => {
                    assigned(w, ev, Tier::Metadata, id.0);
                    put_rec(
                        w,
                        "comment",
                        vec![int(id.0.0)],
                        &CommentRow {
                            on: on.clone(),
                            author: *author,
                            body: body.clone(),
                            reply_to: *reply_to,
                            resolved: false,
                            created: at,
                            updated: at,
                        },
                    );
                    if let Some(n) = target_issue(on) {
                        touch(w, n, &|r| r.comments += 1);
                    }
                    if let CommentTarget::Issue(n) = on {
                        mark(w, "comment_by_issue", vec![n_key(*n), int(id.0.0)]);
                    }
                }
                CommentEvent::Edited { id, body, .. } => {
                    if let Some(mut row) = get_rec::<CommentRow>(w, "comment", &[int(id.0.0)]) {
                        row.body = body.clone();
                        row.updated = at;
                        put_rec(w, "comment", vec![int(id.0.0)], &row);
                    }
                }
                CommentEvent::Deleted { id, .. } => {
                    if let Some(row) = get_rec::<CommentRow>(w, "comment", &[int(id.0.0)]) {
                        w.delete("comment", &[int(id.0.0)]);
                        if let CommentTarget::Issue(n) = row.on {
                            w.delete("comment_by_issue", &[n_key(n), int(id.0.0)]);
                        }
                        if let Some(n) = target_issue(&row.on) {
                            touch(w, n, &|r| r.comments = r.comments.saturating_sub(1));
                        }
                    }
                }
                CommentEvent::Resolved { thread, resolved, .. } => {
                    if let Some(mut row) = get_rec::<CommentRow>(w, "comment", &[int(thread.0.0)]) {
                        row.resolved = *resolved;
                        put_rec(w, "comment", vec![int(thread.0.0)], &row);
                    }
                }
            },
            RepoEvent::Review(r) => match r {
                ReviewEvent::Submitted { id, n, reviewer, state, commit, body } => {
                    assigned(w, ev, Tier::Metadata, id.0);
                    put_rec(
                        w,
                        "review",
                        vec![int(id.0.0)],
                        &ReviewRow {
                            n: *n,
                            reviewer: *reviewer,
                            state: *state,
                            commit: *commit,
                            body: body.clone(),
                            dismissed: None,
                            at,
                        },
                    );
                    mark(w, "review_by_pull", vec![n_key(*n), int(id.0.0)]);
                    touch(w, *n, &|_| {});
                }
                ReviewEvent::Dismissed { id, message, .. } => {
                    if let Some(mut row) = get_rec::<ReviewRow>(w, "review", &[int(id.0.0)]) {
                        row.dismissed = Some(message.clone());
                        put_rec(w, "review", vec![int(id.0.0)], &row);
                    }
                }
                ReviewEvent::Requested { n, reviewers, removed, .. } => {
                    for g in reviewers {
                        let key = vec![n_key(*n), grantee_key(g)];
                        if *removed { w.delete("review_request", &key) } else { mark(w, "review_request", key) }
                    }
                }
            },
            RepoEvent::Labels(l) => match l {
                LabelEvent::Defined { id, name, color, description } => {
                    assigned(w, ev, Tier::Metadata, id.0);
                    put_rec(
                        w,
                        "label",
                        vec![int(id.0.0)],
                        &LabelRow { name: name.clone(), color: *color, description: description.clone() },
                    )
                }
                LabelEvent::Deleted { id } => {
                    w.delete("label", &[int(id.0.0)]);
                    // Every issue's row for the label; the table is keyed by issue first.
                    for (k, _) in w.prefix("issue_label", &[]) {
                        if k.get(1) == Some(&int(id.0.0)) {
                            w.delete("issue_label", &k);
                        }
                    }
                }
                LabelEvent::Applied { n, add, remove, .. } => {
                    for l in add {
                        mark(w, "issue_label", vec![n_key(*n), int(l.0.0)]);
                    }
                    for l in remove {
                        w.delete("issue_label", &[n_key(*n), int(l.0.0)]);
                    }
                    if issue(w, *n).is_some() {
                        touch(w, *n, &|_| {});
                    }
                }
            },
            RepoEvent::Settings(s) => put_rec(w, "retention", Vec::new(), &s.check_retention_days),
            RepoEvent::CheckRun(u) => {
                put_newer(w, KIND_RUN, vec![sha_key(&u.head_sha), int(u.run.0.0)], at, u, |u: &CheckRunUpdate| {
                    u.version
                })
            }
            RepoEvent::CheckSuite(u) => {
                put_newer(w, KIND_SUITE, vec![sha_key(&u.head_sha), int(u.suite.0.0)], at, u, |u: &CheckSuiteUpdate| {
                    u.version
                })
            }
            RepoEvent::Status(s) => {
                let key = vec![sha_key(&s.sha), int(-s.id.0.0)];
                put_newer(w, KIND_STATUS, key, at, s, |_: &StatusUpdate| 0)
            }
            RepoEvent::Release { id, record, .. } => {
                assigned(w, ev, Tier::Metadata, id.0);
                let old = release_row(w, *id);
                if let Some(o) = &old {
                    w.delete("release_by_tag", &[ref_key(&o.rec.tag)]);
                }
                match record {
                    Some(rec) => {
                        let published = old.as_ref().and_then(|o| o.published).or((!rec.draft).then_some(at));
                        let created = old.as_ref().map_or(at, |o| o.created);
                        put_rec(w, "release_by_tag", vec![ref_key(&rec.tag)], &id.0.0);
                        put_rec(
                            w,
                            "release",
                            vec![release_key(*id)],
                            &ReleaseRow { rec: rec.clone(), created, published },
                        );
                    }
                    None => {
                        w.delete("release", &[release_key(*id)]);
                        for (k, _) in w.prefix("asset_by_release", &[int(id.0.0)]) {
                            w.delete("asset", &k[1..]);
                            w.delete("asset_by_release", &k);
                        }
                    }
                }
            }
            RepoEvent::ReleaseAsset { release, id, asset, actor } => {
                assigned(w, ev, Tier::Metadata, id.0);
                let key = vec![int(id.0.0)];
                match asset {
                    Some(rec) => {
                        let old = get_rec::<AssetRow>(w, "asset", &key);
                        let created = old.as_ref().map_or(at, |o| o.created);
                        let uploader = old.as_ref().map_or(*actor, |o| o.uploader);
                        mark(w, "asset_by_release", vec![int(release.0.0), int(id.0.0)]);
                        let row = AssetRow { release: *release, rec: rec.clone(), uploader, created, updated: at };
                        put_rec(w, "asset", key, &row);
                    }
                    None => {
                        w.delete("asset", &key);
                        w.delete("asset_by_release", &[int(release.0.0), int(id.0.0)]);
                    }
                }
            }
            RepoEvent::LfsObject { blob, .. } => {
                put_rec(w, "lfs_object", vec![lfs_key(&blob.oid)], &LfsRow { blob: *blob, at })
            }
            RepoEvent::LfsLock(LfsLockEvent::Created { id, path, refspec, owner }) => {
                assigned(w, ev, Tier::Metadata, id.0);
                put_rec(w, "lfs_lock_by_path", vec![text(path)], &id.0.0);
                let row = LockRow { path: path.clone(), refspec: refspec.clone(), owner: *owner, at };
                put_rec(w, "lfs_lock", vec![int(id.0.0)], &row);
            }
            RepoEvent::LfsLock(LfsLockEvent::Deleted { id, .. }) => {
                if let Some(row) = get_rec::<LockRow>(w, "lfs_lock", &[int(id.0.0)]) {
                    w.delete("lfs_lock_by_path", &[text(&row.path)]);
                    w.delete("lfs_lock", &[int(id.0.0)]);
                }
            }
            _ => {}
        }
    }
}

/// Writes a check row unless the stored one has a version at or above it, so a repeat or a late older update
/// changes nothing.
fn put_newer<T: Clone + Serialize + DeserializeOwned>(
    w: &mut dyn Rows,
    kind: i64,
    key: Vec<Val>,
    at: Timestamp,
    rec: &T,
    version: impl Fn(&T) -> u64,
) {
    if let Some(old) = get_rec::<Timed<T>>(w, check_table(kind), &key)
        && version(&old.rec) >= version(rec)
    {
        return;
    }
    put_timed(w, kind, key, at, rec.clone());
}

pub fn grantee_key(g: &Grantee) -> Val {
    match g {
        Grantee::User(u) => Val::Text(format!("user:{}", u.0.0.0).into()),
        Grantee::Team(t) => Val::Text(format!("team:{}", t.0.0).into()),
    }
}

/// The `issue_order` key: kind, open, and the number negated, so a forward scan lists the newest first.
pub fn order_key(kind: IssueKind, open: bool, n: IssueNumber) -> Vec<Val> {
    vec![int(kind as i64), int(i64::from(open)), int(-i64::from(n.0))]
}

/// The issue or pull request a comment counts toward.
pub(crate) fn target_issue(on: &CommentTarget) -> Option<IssueNumber> {
    match on {
        CommentTarget::Issue(n) | CommentTarget::Review { n, .. } => Some(*n),
        CommentTarget::Commit { .. } => None,
    }
}

impl Tiered for RepoTiers {
    type Event = RepoEvent;

    fn tier_of(ev: &RepoEvent) -> TierSet {
        tier_of(ev)
    }

    fn schema(tier: Tier) -> Schema {
        match tier {
            Tier::Core => CORE,
            Tier::Metadata => METADATA,
        }
    }

    fn write(&self, ev: &RepoEvent, tier: Tier, at: Timestamp, rows: &mut dyn Rows) {
        match tier {
            Tier::Core => self.write_core(ev, at, rows),
            Tier::Metadata => self.write_meta(ev, at, rows),
        }
    }

    /// Each tier's cursor moves to its own half of the block.
    fn id_block(&self, block: IdBlock, tier: Tier, w: &mut dyn Rows) {
        if let Some(half) = tier_block(block, tier) {
            let mut c = ids(w);
            c.apply_block(half);
            put_rec(w, "ids", Vec::new(), &c);
        }
    }

    /// Metadata only: deletes every check run, suite, and status whose time plus the retention is before `at`.
    fn on_entry(&self, tier: Tier, at: Timestamp, w: &mut dyn Rows) {
        if tier != Tier::Metadata {
            return;
        }
        let keep = u64::from(retention_days(w)) * DAY_MS;
        loop {
            let batch = w.scan("check_expiry", &[], 256);
            let mut any = false;
            for (k, _) in &batch {
                let (Some(Val::Int(t)), Some(Val::Int(kind)), Some(Val::Blob(b))) = (k.first(), k.get(1), k.get(2))
                else {
                    panic!("check_expiry key");
                };
                if (*t as u64).saturating_add(keep) >= at.0 {
                    return;
                }
                let key: Vec<Val> = postcard::from_bytes(b).expect("check_expiry key blob");
                w.delete(check_table(*kind), &key);
                w.delete("check_expiry", k);
                any = true;
            }
            if !any || batch.len() < 256 {
                return;
            }
        }
    }
}
