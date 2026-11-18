//! A metadata split (`graftyard_git::split`; `design/DESIGN.md`, Shape, `CHILD_GROUP_AT`) on in-memory members: the
//! parent's steps as `plan` names them, its `MetaSlim` pass, and a read that fills each slimmed field from a child
//! restored from the parent's own metadata snapshot.

use std::cell::RefCell;

#[path = "stream_eq/mod.rs"]
mod stream_eq;

use graftyard_fold::{
    MemStore, Read, Restore, SnapshotFrame, SnapshotJob, Val, fold_digest, refold_digest, undo_table,
};
use graftyard_git::issue_ops::{IssueCmd, IssueErr, IssueOp, IssueOut, RepoView};
use graftyard_git::issues::{CommentId, CommentTarget, IssueChange, IssueNumber};
use graftyard_git::key::R2Key;
use graftyard_git::pack::Archived;
use graftyard_git::repo::{RepoEvent, RepoFold};
use graftyard_git::split::{ChildView, Filled, SLIM_ROWS, SLIMMED, SplitEntry, SplitStep, plan, slimmed};
use graftyard_git::tiers::{CORE, CommentRow, IssueRow, METADATA, RepoTiers, get_rec, int, n_key, tier_of};
use graftyard_identity::{
    AccountKind, Actor, AuthzEvent, AuthzFold, Credential, IdentityDep, NamesEvent, NamesFold, Principal,
    RepoPermission, ScopeSet, TokenKind, TokensEvent, TokensFold, UserId,
};
use graftyard_raft::{EntryBody, Fold, Format, LogEntry, PREV_ZERO, Scratch, Term, TierSet, digest};
use graftyard_types::{
    AccountId, ChainId, DbId, Digest, IdBlock, Index, NameShard, ReleaseId, RepoId, RequestId, Tier, Timestamp,
    TokenShard, Ulid,
};
use smallvec::SmallVec;

const REPO: RepoId = RepoId(DbId(7));
const OWNER: i64 = 100;
const DAY: u64 = 86_400_000;

fn tiers() -> RepoTiers {
    RepoTiers { repo: REPO, owner: AccountId(DbId(OWNER)) }
}

fn who(account: i64) -> Principal {
    Principal {
        actor: Actor::User(UserId(AccountId(DbId(account)))),
        scopes: ScopeSet(1),
        credential: Credential::Stored([account as u8; 32]),
        sso_orgs: SmallVec::new(),
    }
}

fn deps() -> Vec<(ChainId, IdentityDep)> {
    let mut names = NamesFold::new(NameShard(0));
    let mut tokens = TokensFold::new(TokenShard(0));
    let mut authz = AuthzFold::new(AccountId(DbId(OWNER)));
    authz.apply(&AuthzEvent::Opened { kind: AccountKind::User, enterprise: None });
    let id = AccountId(DbId(OWNER));
    names.apply(&NamesEvent::Account { id, suspended: false, sessions_not_before: Timestamp(0) });
    tokens.apply(&TokensEvent::Stored {
        hash: [OWNER as u8; 32],
        kind: TokenKind::Pat,
        account: id,
        scopes: ScopeSet(1),
        expires: None,
    });
    let mut out = Vec::new();
    for c in graftyard_identity::credential_chains(&who(OWNER).credential_shard(), None) {
        out.push(match c {
            ChainId::Names(_) => (c, IdentityDep::Names(names.clone())),
            ChainId::Tokens(s) => {
                let mut t = tokens.clone();
                t.shard = Some(s);
                (c, IdentityDep::Tokens(t))
            }
            _ => unreachable!(),
        });
    }
    out.push((ChainId::Authz(id), IdentityDep::Authz(authz)));
    let install = graftyard_identity::install_account();
    out.push((ChainId::Authz(install), IdentityDep::Authz(AuthzFold::new(install))));
    let _ = RepoPermission::Read;
    out
}

/// One parent member, the log it applied, and its leader's clock.
struct Parent {
    fold: RepoFold,
    log: Vec<LogEntry<RepoEvent>>,
    now: Timestamp,
    deps: Vec<(ChainId, IdentityDep)>,
}

impl Parent {
    fn new() -> Parent {
        let mut p = Parent {
            fold: RepoFold::new(REPO, AccountId(DbId(OWNER))),
            log: Vec::new(),
            now: Timestamp(DAY),
            deps: deps(),
        };
        p.entry(EntryBody::IdBlock(IdBlock::whole(0, 1).unwrap()), TierSet::CORE, Format { family: 0x20, version: 3 });
        p
    }

    fn entry(&mut self, body: EntryBody<RepoEvent>, tiers: TierSet, format: Format) -> LogEntry<RepoEvent> {
        let e = LogEntry {
            term: Term(2),
            index: Index(self.log.len() as u64 + 1),
            prev: self.log.last().map_or(PREV_ZERO, digest),
            format,
            tiers,
            at: self.now,
            requests: SmallVec::new(),
            extend: None,
            body,
        };
        self.fold.apply_entry(&e);
        self.log.push(e.clone());
        e
    }

    /// A leader's own record, after the follower's `verify` replay on the fold as it stands.
    fn record(&mut self, ev: RepoEvent) -> Result<Index, Box<str>> {
        let tag = tier_of(&ev);
        self.fold.scratch_at(self.now, tag).check(&ev)?;
        Ok(self.entry(EntryBody::Events(vec![ev]), tag, graftyard_git::split::REPO_FORMAT_SPLIT).index)
    }

    fn commit(&mut self, op: IssueOp) -> Result<IssueOut, IssueErr> {
        let cmd = IssueCmd { op, actor: who(OWNER), request: RequestId(Ulid(self.log.len() as u128)) };
        let view = RepoView::new(tiers(), &self.fold.core, &self.fold.meta, self.now);
        let needed: Vec<ChainId> = cmd.deps(&view).into_iter().collect();
        let d: Vec<_> = needed.iter().map(|k| self.deps.iter().find(|(x, _)| x == k).unwrap().clone()).collect();
        let (evs, out) = cmd.decide(&view, &d)?;
        drop(view);
        let tag = evs.iter().fold(TierSet::NONE, |t, e| t.union(tier_of(e)));
        self.entry(EntryBody::Events(evs.into_vec()), tag, Format { family: 0x20, version: 5 });
        Ok(out)
    }

    fn open_issue(&mut self, body: &str) -> IssueNumber {
        let op = IssueOp::OpenIssue {
            title: "t".into(),
            body: body.into(),
            labels: SmallVec::new(),
            assignees: SmallVec::new(),
        };
        match self.commit(op).unwrap() {
            IssueOut::Number(n) => n,
            o => panic!("{o:?}"),
        }
    }

    fn comment(&mut self, n: IssueNumber, body: &str) -> CommentId {
        match self.commit(IssueOp::Comment { on: CommentTarget::Issue(n), body: body.into(), reply_to: None }).unwrap()
        {
            IssueOut::Id(id) => CommentId(id),
            o => panic!("{o:?}"),
        }
    }

    /// The metadata snapshot at the tier's `applied`, as the snapshot writer writes it, and its `Archived` committed.
    fn snapshot(&mut self) -> (R2Key, Vec<SnapshotFrame>) {
        let mut job = SnapshotJob::begin(
            &mut *self.fold.meta,
            METADATA,
            ChainId::Repo(REPO),
            ReleaseId([0; 32]),
            Format { family: 0x20, version: 1 },
        )
        .unwrap();
        let d = job.want(&self.fold.meta);
        let mut frames = Vec::new();
        while let Some(f) = job.next(&self.fold.meta, 3) {
            frames.push(f);
        }
        graftyard_fold::end_snapshot(&mut *self.fold.meta, &METADATA);
        let key = R2Key::Snapshot {
            chain: ChainId::Repo(REPO),
            tier: Tier::Metadata,
            from: Index(0),
            to: job.to(),
            digest: d,
        };
        let a = Archived {
            key: key.clone(),
            from: Index(0),
            to: job.to(),
            from_at: Timestamp(0),
            to_at: job.to_at(),
            digest: d,
        };
        self.entry(EntryBody::Events(vec![RepoEvent::Archived(a)]), TierSet::CORE, Format { family: 0x20, version: 1 });
        (key, frames)
    }

    fn step(&self, child: Option<&ChildView>) -> SplitStep {
        plan(&self.fold.core, &self.fold.meta, 8 << 30, 7 << 30, child)
    }

    fn row<T: serde::de::DeserializeOwned>(&self, table: &str, key: &[Val]) -> Option<T> {
        get_rec(&self.fold.meta, table, key)
    }
}

/// A child member's metadata tier, restored from the parent's snapshot frames.
fn restore(frames: &[SnapshotFrame], at: Index) -> MemStore {
    let mut s = MemStore::default();
    let mut r = Restore::begin(&s, METADATA, (ChainId::Repo(REPO), at)).unwrap();
    for f in frames {
        r.frame(&mut s, f).unwrap();
    }
    r.finish(&mut s).unwrap();
    s
}

/// The parent's tier read through `Filled` over the child's rows.
fn filled<T: serde::de::DeserializeOwned>(parent: &dyn Read, child: &MemStore, table: &str, key: &[Val]) -> Option<T> {
    let missed = RefCell::new(Vec::new());
    let ch = |t: &str, k: &[Val]| Some(child.get(t, k));
    let f = Filled { meta: parent, child: &ch, missed: &missed };
    let v = get_rec(&f, table, key);
    assert!(missed.borrow().is_empty());
    v
}

/// Runs a parent through every step to the end of its slim pass, with the child restored from the seed's snapshot.
/// Returns the child's tier and the index `MetaSeed` names.
fn split(p: &mut Parent) -> (MemStore, Index) {
    assert_eq!(p.step(None), SplitStep::Split);
    p.record(RepoEvent::Split(SplitEntry::MetaSplit { child: ChainId::RepoMeta(REPO) })).unwrap();
    assert_eq!(p.step(None), SplitStep::WaitSnapshot);
    let (key, frames) = p.snapshot();
    let SplitStep::Seed { at, key: k } = p.step(None) else { panic!("{:?}", p.step(None)) };
    assert_eq!(k, key);
    p.record(RepoEvent::Split(SplitEntry::MetaSeed { at, key: k.clone() })).unwrap();
    assert_eq!(p.step(None), SplitStep::Child { at, key: k });
    let child = restore(&frames, at);
    assert_eq!(p.step(Some(&ChildView { seeded: true, loaded: false })), SplitStep::Idle);
    assert_eq!(p.step(Some(&ChildView { seeded: true, loaded: true })), SplitStep::Drop { seed: at });
    p.record(RepoEvent::Split(SplitEntry::MetaDropped { seed: at })).unwrap();
    let mut slims = 0;
    while p.step(Some(&ChildView { seeded: true, loaded: true })) == SplitStep::Slim {
        p.record(RepoEvent::Split(SplitEntry::MetaSlim)).unwrap();
        slims += 1;
        assert!(slims < 10_000);
    }
    (child, at)
}

/// The text fields a read sees are the same before and after the split, for rows written before, at, and after the
/// seed, edited after it, and deleted after it; the parent holds no text of a row the seed covered.
#[test]
fn a_read_after_the_split_sees_every_text_it_saw_before() {
    let mut p = Parent::new();
    let n = p.open_issue("issue body before");
    let mut before = Vec::new();
    for i in 0..(SLIM_ROWS as u32 * 2 + 7) {
        before.push(p.comment(n, &format!("comment {i}, written before the seed")));
    }
    let edited = before[3];
    let deleted = before[5];
    // A row the seed covers and an edit after the seed: the edit wins on every read.
    let (child, seed) = split_with(&mut p, |p| {
        p.commit(IssueOp::EditComment { id: edited, body: "edited after the seed".into() }).unwrap();
        p.commit(IssueOp::DeleteComment { id: deleted }).unwrap();
    });
    let after = p.comment(n, "written after the seed");
    let meta = &p.fold.meta;
    for (i, id) in before.iter().enumerate() {
        let k = [int(id.0.0)];
        if *id == deleted {
            assert!(filled::<CommentRow>(meta, &child, "comment", &k).is_none(), "a deleted row is never filled");
            assert!(child.get("comment", &k).is_some(), "the child still holds it");
            continue;
        }
        let want = if *id == edited {
            "edited after the seed".to_string()
        } else {
            format!("comment {i}, written before the seed")
        };
        assert_eq!(&*filled::<CommentRow>(meta, &child, "comment", &k).unwrap().body, want);
        // Every row the seed covers is slimmed; the edited one, last written after it, keeps its text.
        let raw: CommentRow = p.row("comment", &k).unwrap();
        assert_eq!(&*raw.body == SLIMMED, *id != edited, "row {i}, seed {seed:?}");
    }
    assert_eq!(
        &*filled::<CommentRow>(meta, &child, "comment", &[int(after.0.0)]).unwrap().body,
        "written after the seed"
    );
    assert!(!slimmed("comment", &p.fold.meta.get("comment", &[int(after.0.0)]).unwrap()));
    // The issue's body was written before the seed, its comment count after: the count is the parent's.
    let issue: IssueRow = filled(meta, &child, "issue", &[n_key(n)]).unwrap();
    assert_eq!((&*issue.body, issue.comments), ("issue body before", before.len() as u32));
}

/// `split`, with `between` run after `MetaSeed` and before `MetaDropped`.
fn split_with(p: &mut Parent, between: impl FnOnce(&mut Parent)) -> (MemStore, Index) {
    p.record(RepoEvent::Split(SplitEntry::MetaSplit { child: ChainId::RepoMeta(REPO) })).unwrap();
    let (key, frames) = p.snapshot();
    let SplitStep::Seed { at, key: k } = p.step(None) else { panic!() };
    assert_eq!(k, key);
    p.record(RepoEvent::Split(SplitEntry::MetaSeed { at, key: k })).unwrap();
    between(p);
    let child = restore(&frames, at);
    p.record(RepoEvent::Split(SplitEntry::MetaDropped { seed: at })).unwrap();
    while p.step(Some(&ChildView { seeded: true, loaded: true })) == SplitStep::Slim {
        p.record(RepoEvent::Split(SplitEntry::MetaSlim)).unwrap();
    }
    (child, at)
}

/// Every command still decides on the parent after the split as it did before: an edit by the author, a delete, a
/// reply to a slimmed comment, a body change on a slimmed issue, and the issue's other fields.
#[test]
fn commands_decide_the_same_on_the_slimmed_parent() {
    let mut p = Parent::new();
    let n = p.open_issue("body");
    let c = p.comment(n, "first");
    let (child, _) = split(&mut p);
    assert_eq!(&*p.row::<CommentRow>("comment", &[int(c.0.0)]).unwrap().body, SLIMMED);
    p.commit(IssueOp::Comment { on: CommentTarget::Issue(n), body: "reply".into(), reply_to: Some(c) }).unwrap();
    p.commit(IssueOp::EditComment { id: c, body: "edited".into() }).unwrap();
    p.commit(IssueOp::Change { n, change: IssueChange::Title("new title".into()) }).unwrap();
    let issue: IssueRow = filled(&p.fold.meta, &child, "issue", &[n_key(n)]).unwrap();
    assert_eq!((&*issue.title, &*issue.body), ("new title", "body"));
    p.commit(IssueOp::Change { n, change: IssueChange::Body("new body".into()) }).unwrap();
    let issue: IssueRow = filled(&p.fold.meta, &child, "issue", &[n_key(n)]).unwrap();
    assert_eq!(&*issue.body, "new body");
    assert_eq!(&*filled::<CommentRow>(&p.fold.meta, &child, "comment", &[int(c.0.0)]).unwrap().body, "edited");
    p.commit(IssueOp::DeleteComment { id: c }).unwrap();
    assert!(p.row::<CommentRow>("comment", &[int(c.0.0)]).is_none());
}

/// Members that apply the same log keep the same digests through the split and its slim pass, and the kept sum stays
/// the rows' own; the slim pass ends and shrinks the tier.
#[test]
fn members_agree_through_the_slim_pass_and_it_ends() {
    let mut p = Parent::new();
    let n = p.open_issue(&"x".repeat(500));
    for i in 0..(SLIM_ROWS * 3) {
        p.comment(n, &format!("{i} {}", "y".repeat(400)));
    }
    let bytes = |s: &dyn Read| -> usize {
        METADATA.tables.iter().flat_map(|t| s.prefix(t.name, &[])).map(|(_, c)| format!("{c:?}").len()).sum()
    };
    let size0 = bytes(&p.fold.meta);
    split(&mut p);
    let slims = p
        .log
        .iter()
        .filter(|e| matches!(&e.body, EntryBody::Events(v) if v == &[RepoEvent::Split(SplitEntry::MetaSlim)]))
        .count();
    assert!(slims >= 3, "{slims} slim entries for {} rows", SLIM_ROWS * 3);
    assert!(bytes(&p.fold.meta) * 4 < size0, "the tier shrank from {size0} to {}", bytes(&p.fold.meta));
    let mut late = RepoFold::new(REPO, AccountId(DbId(OWNER)));
    for e in &p.log {
        late.apply_entry(e);
    }
    for (t, s) in [(Tier::Core, CORE), (Tier::Metadata, METADATA)] {
        let (a, b) = match t {
            Tier::Core => (&p.fold.core, &late.core),
            Tier::Metadata => (&p.fold.meta, &late.meta),
        };
        assert_eq!(fold_digest(&**a, t), fold_digest(&**b, t));
        assert_eq!(fold_digest(&**a, t), refold_digest(&**a, &s));
    }
    assert_eq!(p.step(Some(&ChildView { seeded: true, loaded: true })), SplitStep::Idle, "one pass only");
}

/// The followers' `verify` refuses each step out of order, twice, on the wrong group, or naming the wrong snapshot.
#[test]
fn verify_refuses_steps_out_of_order() {
    let mut p = Parent::new();
    p.open_issue("b");
    let drop0 = RepoEvent::Split(SplitEntry::MetaDropped { seed: Index(1) });
    assert!(p.record(drop0).is_err(), "no drop before the seed");
    assert!(p.record(RepoEvent::Split(SplitEntry::MetaSlim)).is_err(), "no slim before the drop");
    let wrong = RepoEvent::Split(SplitEntry::MetaSplit { child: ChainId::RepoMeta(RepoId(DbId(8))) });
    assert!(p.record(wrong).is_err(), "another repository's child");
    p.record(RepoEvent::Split(SplitEntry::MetaSplit { child: ChainId::RepoMeta(REPO) })).unwrap();
    assert!(p.record(RepoEvent::Split(SplitEntry::MetaSplit { child: ChainId::RepoMeta(REPO) })).is_err());
    let (key, _) = p.snapshot();
    let R2Key::Snapshot { to, digest: d, .. } = key.clone() else { panic!() };
    let other = R2Key::Snapshot {
        chain: ChainId::Repo(REPO),
        tier: Tier::Metadata,
        from: Index(0),
        to,
        digest: Digest([9; 32]),
    };
    assert!(p.record(RepoEvent::Split(SplitEntry::MetaSeed { at: to, key: other })).is_err(), "an uncommitted key");
    let core = R2Key::Snapshot { chain: ChainId::Repo(REPO), tier: Tier::Core, from: Index(0), to, digest: d };
    assert!(p.record(RepoEvent::Split(SplitEntry::MetaSeed { at: to, key: core })).is_err(), "a core snapshot");
    assert!(p.record(RepoEvent::Split(SplitEntry::MetaSeed { at: Index(to.0 - 1), key: key.clone() })).is_err());
    p.record(RepoEvent::Split(SplitEntry::MetaSeed { at: to, key: key.clone() })).unwrap();
    assert!(p.record(RepoEvent::Split(SplitEntry::MetaSeed { at: to, key })).is_err(), "one seed");
    assert!(p.record(RepoEvent::Split(SplitEntry::MetaDropped { seed: Index(to.0 + 1) })).is_err(), "the seed's index");
    p.record(RepoEvent::Split(SplitEntry::MetaDropped { seed: to })).unwrap();
    assert!(p.record(RepoEvent::Split(SplitEntry::MetaDropped { seed: to })).is_err(), "one drop");
    p.record(RepoEvent::Split(SplitEntry::MetaSlim)).unwrap();
}

/// The child takes only its own seed: a `MetaSeed` on a fold marked as the child needs no split before it, and the
/// child refuses `MetaSplit` and `MetaDropped`.
#[test]
fn the_child_takes_only_its_seed() {
    let mut p = Parent::new();
    p.open_issue("b");
    let (key, _) = p.snapshot();
    let R2Key::Snapshot { to, .. } = key else { panic!() };
    let mut c = Parent::new();
    c.fold.child = true;
    assert!(c.record(RepoEvent::Split(SplitEntry::MetaSplit { child: ChainId::RepoMeta(REPO) })).is_err());
    c.record(RepoEvent::Split(SplitEntry::MetaSeed { at: to, key: key.clone() })).unwrap();
    assert!(c.record(RepoEvent::Split(SplitEntry::MetaDropped { seed: to })).is_err());
    let mut parent = Parent::new();
    assert!(
        parent.record(RepoEvent::Split(SplitEntry::MetaSeed { at: to, key })).is_err(),
        "a parent seeds after its split"
    );
}

/// A read whose child row is not held yet counts the miss, and the field reads empty rather than `SLIMMED`.
#[test]
fn a_missing_child_row_is_counted() {
    let mut p = Parent::new();
    let n = p.open_issue("b");
    let c = p.comment(n, "hello");
    split(&mut p);
    let missed = RefCell::new(Vec::new());
    let none = |_: &str, _: &[Val]| None;
    let f = Filled { meta: &p.fold.meta, child: &none, missed: &missed };
    let row: CommentRow = get_rec(&f, "comment", &[int(c.0.0)]).unwrap();
    assert_eq!(&*row.body, "");
    assert_eq!(missed.borrow().as_slice(), &[("comment".to_string(), vec![int(c.0.0)])]);
    assert!(slimmed("comment", &p.fold.meta.get("comment", &[int(c.0.0)]).unwrap()));
    let _ = undo_table("comment");
}

/// A child member's restored tier is the parent's snapshot and nothing after it: the child's own entries (its
/// `MetaSeed`, and any event a test records there) leave its metadata digest unchanged, while its core tier follows.
#[test]
fn a_child_tier_takes_no_entry_after_its_restore() {
    let mut p = Parent::new();
    let n = p.open_issue("seeded");
    p.comment(n, "seeded comment");
    let (key, frames) = p.snapshot();
    let R2Key::Snapshot { to, .. } = key.clone() else { panic!() };
    let mut c = Parent::new();
    c.fold.child = true;
    c.fold.meta = Box::new(restore(&frames, to));
    let before = fold_digest(&*c.fold.meta, Tier::Metadata);
    let core0 = graftyard_fold::meta(&*c.fold.core).applied;
    c.record(RepoEvent::Split(SplitEntry::MetaSeed { at: to, key })).unwrap();
    // A metadata event in the child's log, as a buggy leader could record: the restored tier ignores it.
    c.entry(
        EntryBody::Events(vec![RepoEvent::Split(SplitEntry::MetaSlim)]),
        TierSet::CORE.union(TierSet::METADATA),
        graftyard_git::split::REPO_FORMAT_SPLIT,
    );
    assert_eq!(fold_digest(&*c.fold.meta, Tier::Metadata), before);
    assert_eq!(graftyard_fold::meta(&*c.fold.meta).applied, to, "the tier stays at the seed's index");
    assert_eq!(
        graftyard_fold::meta(&*c.fold.core).applied,
        Index(core0.0 + 2),
        "the core tier follows the child's log"
    );
}

/// The slim pass touches each row once across batch boundaries: with rows in exact multiples of `SLIM_ROWS`, the
/// pass takes the number of `MetaSlim` entries its rows need, every covered row is slimmed, and the rows on either
/// side of each boundary are slimmed too.
#[test]
fn the_slim_pass_takes_each_row_once_across_batches() {
    let mut p = Parent::new();
    let n = p.open_issue("b");
    let ids: Vec<_> = (0..SLIM_ROWS * 2).map(|i| p.comment(n, &format!("c{i}"))).collect();
    split(&mut p);
    let slims = p
        .log
        .iter()
        .filter(|e| matches!(&e.body, EntryBody::Events(v) if v == &[RepoEvent::Split(SplitEntry::MetaSlim)]))
        .count();
    // 1 issue row and 2 * SLIM_ROWS comments in batches of SLIM_ROWS: three entries.
    assert_eq!(slims, 3, "{slims} slim entries");
    for (i, id) in ids.iter().enumerate() {
        let r: CommentRow = p.row("comment", &[int(id.0.0)]).unwrap();
        assert_eq!(&*r.body, SLIMMED, "comment {i}");
    }
}

/// A `MetaSlim` entry stops at the row that reaches `SLIM_BYTES`, however few rows that is, and the next entry starts at
/// the row after it: with 60 KB bodies the first entry slims some but not all of nine comments, the pass takes at least
/// three entries, and every comment ends slimmed.
#[test]
fn a_slim_entry_stops_at_its_bytes_and_the_next_takes_the_rest() {
    use graftyard_git::split::SLIM_BYTES;
    let mut p = Parent::new();
    let n = p.open_issue("b");
    let big = "x".repeat(60_000);
    assert!(SLIM_BYTES < 9 * big.len() / 3, "nine bodies need at least three entries");
    let ids: Vec<_> = (0..9).map(|_| p.comment(n, &big)).collect();
    let body = |p: &Parent, id: &CommentId| p.row::<CommentRow>("comment", &[int(id.0.0)]).unwrap().body;
    p.record(RepoEvent::Split(SplitEntry::MetaSplit { child: ChainId::RepoMeta(REPO) })).unwrap();
    let _ = p.snapshot();
    let SplitStep::Seed { at, key } = p.step(None) else { panic!() };
    p.record(RepoEvent::Split(SplitEntry::MetaSeed { at, key })).unwrap();
    p.record(RepoEvent::Split(SplitEntry::MetaDropped { seed: at })).unwrap();
    p.record(RepoEvent::Split(SplitEntry::MetaSlim)).unwrap();
    let first = ids.iter().filter(|id| &*body(&p, id) == SLIMMED).count();
    assert!(first > 0 && first < ids.len(), "the first entry slimmed {first} of {}", ids.len());
    let mut slims = 1;
    while p.step(Some(&ChildView { seeded: true, loaded: true })) == SplitStep::Slim {
        p.record(RepoEvent::Split(SplitEntry::MetaSlim)).unwrap();
        slims += 1;
        assert!(slims < 100);
    }
    assert!(slims >= 3, "{slims} slim entries");
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(&*body(&p, id), SLIMMED, "comment {i}");
    }
}

/// A fill takes only the slimmed field: a task whose prompt was slimmed and whose summary the parent set after the
/// seed reads the child's prompt and the parent's summary; a release body slimmed reads the child's.
#[test]
fn a_fill_takes_only_the_slimmed_fields() {
    use graftyard_git::split::fill;
    use graftyard_git::task::TaskMeta;
    let enc = |t: &TaskMeta| vec![Val::Blob(postcard::to_allocvec(t).unwrap().into())];
    let base = TaskMeta {
        creator: graftyard_git::task::TaskActor::User(UserId(AccountId(DbId(OWNER)))),
        issue: None,
        snapshot: Index(1),
        prompt: Some("the prompt".into()),
        pr: None,
        summary: "old summary".into(),
        created: Timestamp(1),
    };
    let parent = TaskMeta { prompt: Some(SLIMMED.into()), summary: "new summary".into(), ..base.clone() };
    let got: TaskMeta = postcard::from_bytes(match &fill("agent_task_meta", &enc(&parent), Some(&enc(&base)))[0] {
        Val::Blob(b) => b,
        v => panic!("{v:?}"),
    })
    .unwrap();
    assert_eq!((got.prompt.as_deref(), &*got.summary), (Some("the prompt"), "new summary"));
    // The other way round: the parent's own prompt stays, its slimmed summary is the child's.
    let parent = TaskMeta { prompt: Some("mine".into()), summary: SLIMMED.into(), ..base.clone() };
    let got: TaskMeta = postcard::from_bytes(match &fill("agent_task_meta", &enc(&parent), Some(&enc(&base)))[0] {
        Val::Blob(b) => b,
        v => panic!("{v:?}"),
    })
    .unwrap();
    assert_eq!((got.prompt.as_deref(), &*got.summary), (Some("mine"), "old summary"));
}

/// With two committed metadata snapshots, the seed is the newer one: the child then holds every row written up to
/// it, and the parent slims them all.
#[test]
fn the_seed_is_the_newest_snapshot() {
    let mut p = Parent::new();
    let n = p.open_issue("b");
    p.record(RepoEvent::Split(SplitEntry::MetaSplit { child: ChainId::RepoMeta(REPO) })).unwrap();
    let (old, _) = p.snapshot();
    let late = p.comment(n, "after the first snapshot");
    let (new, _) = p.snapshot();
    let SplitStep::Seed { key, .. } = p.step(None) else { panic!("{:?}", p.step(None)) };
    assert_ne!(key, old);
    assert_eq!(key, new);
    let _ = late;
}

/// (i-1): the slimmed tier is snapshotted once its pass ends, so a later install names it and not the whole tier from
/// before `MetaDropped`. Not due before the split, mid-pass, or once a snapshot at or after the pass's end committed;
/// due when the newest snapshot predates the end, even one taken after `MetaDropped`.
#[test]
fn the_slimmed_tier_is_snapshotted_once_its_pass_ends() {
    use graftyard_git::split::slim_snapshot_due;
    let mut p = Parent::new();
    let n = p.open_issue("body");
    for i in 0..(SLIM_ROWS * 2) {
        p.comment(n, &format!("comment {i} {}", "z".repeat(300)));
    }
    let due = |p: &Parent| slim_snapshot_due(&p.fold.core, &*p.fold.meta);
    assert!(!due(&p), "no split yet");
    p.record(RepoEvent::Split(SplitEntry::MetaSplit { child: ChainId::RepoMeta(REPO) })).unwrap();
    let (whole, _) = p.snapshot();
    let SplitStep::Seed { at, key } = p.step(None) else { panic!() };
    p.record(RepoEvent::Split(SplitEntry::MetaSeed { at, key })).unwrap();
    p.record(RepoEvent::Split(SplitEntry::MetaDropped { seed: at })).unwrap();
    p.record(RepoEvent::Split(SplitEntry::MetaSlim)).unwrap();
    assert_eq!(p.step(Some(&ChildView { seeded: true, loaded: true })), SplitStep::Slim, "the pass runs on");
    assert!(!due(&p), "mid-pass: the tier is still being slimmed");
    while p.step(Some(&ChildView { seeded: true, loaded: true })) == SplitStep::Slim {
        p.record(RepoEvent::Split(SplitEntry::MetaSlim)).unwrap();
    }
    assert!(due(&p), "the pass ended after the newest snapshot ({whole})");
    // A write after the end does not make it any less due.
    p.comment(n, "after the pass");
    assert!(due(&p));
    let (slim, frames) = p.snapshot();
    assert!(!due(&p), "a snapshot after the end holds the slimmed tier");
    // The new snapshot is what an install names (the newest at or below `through`), and it holds no seeded text.
    let newest = graftyard_git::tiers::snapshots(&p.fold.core, Tier::Metadata).pop().unwrap().2.key;
    assert_eq!(newest, slim);
    let restored = restore(&frames, Index(p.log.len() as u64 - 1));
    let texts = restored.prefix("comment", &[]).into_iter().filter(|(_, c)| !slimmed("comment", c)).count();
    assert_eq!(texts, 1, "only the comment written after the pass keeps its text in the slim snapshot");
}

/// (i-1)'s boundary: a snapshot taken at exactly the entry that ended the pass holds the slimmed tier, so nothing is due
/// after it, or the writer would snapshot the same tier again and again.
#[test]
fn a_snapshot_at_the_pass_end_satisfies_it() {
    use graftyard_git::split::slim_snapshot_due;
    let mut p = Parent::new();
    let n = p.open_issue("body");
    p.comment(n, "c");
    split(&mut p);
    assert!(slim_snapshot_due(&p.fold.core, &*p.fold.meta));
    p.snapshot();
    assert!(!slim_snapshot_due(&p.fold.core, &*p.fold.meta));
}

/// A fold audit of a split parent (`decisions/OPEN-QUESTIONS.md`, F88, graft-m1's metadata alarms) restores the
/// parent's metadata tier from a committed snapshot and replays the log after it (`refold_segment`, as
/// `workers/site/src/load.rs` does), and must reach the digest the live members kept. Snapshots taken before the split,
/// at the seed, mid-pass (begun before slim entries and read after them, so the slim's rewrites go through
/// `snap_undo`), and after the pass all refold through the split, the slim pass and later writes to the live digest.
#[test]
fn a_refold_of_a_split_parent_s_metadata_tier_matches_its_members() {
    use graftyard_fold::{SnapshotFrame, refold_segment};
    let mut p = Parent::new();
    let n = p.open_issue("body");
    for i in 0..(SLIM_ROWS * 3 + 5) {
        p.comment(n, &format!("comment {i} {}", "w".repeat(200)));
    }
    let mut snaps: Vec<(&str, Index, Vec<SnapshotFrame>)> = Vec::new();
    let at = |p: &Parent| Index(p.log.len() as u64);
    let (_, f) = p.snapshot();
    snaps.push(("before the split", Index(at(&p).0 - 1), f));
    p.record(RepoEvent::Split(SplitEntry::MetaSplit { child: ChainId::RepoMeta(REPO) })).unwrap();
    let (_, f) = p.snapshot();
    let SplitStep::Seed { at: seed, key } = p.step(None) else { panic!("{:?}", p.step(None)) };
    snaps.push(("the seed", seed, f));
    p.record(RepoEvent::Split(SplitEntry::MetaSeed { at: seed, key })).unwrap();
    // A row the seed covers, edited before the drop: the slim keeps its text.
    p.comment(n, "between the seed and the drop");
    p.record(RepoEvent::Split(SplitEntry::MetaDropped { seed })).unwrap();
    p.record(RepoEvent::Split(SplitEntry::MetaSlim)).unwrap();
    // A snapshot begun mid-pass and read after more slim entries and a write.
    let mut job = SnapshotJob::begin(
        &mut *p.fold.meta,
        METADATA,
        ChainId::Repo(REPO),
        ReleaseId([0; 32]),
        Format { family: 0x20, version: 1 },
    )
    .unwrap();
    let mid = job.to();
    p.record(RepoEvent::Split(SplitEntry::MetaSlim)).unwrap();
    p.comment(n, "while the mid-pass snapshot is read");
    let mut f = Vec::new();
    while let Some(x) = job.next(&p.fold.meta, 3) {
        f.push(x);
    }
    graftyard_fold::end_snapshot(&mut *p.fold.meta, &METADATA);
    snaps.push(("mid-pass", mid, f));
    while p.step(Some(&ChildView { seeded: true, loaded: true })) == SplitStep::Slim {
        p.record(RepoEvent::Split(SplitEntry::MetaSlim)).unwrap();
    }
    let (_, f) = p.snapshot();
    snaps.push(("after the pass", Index(at(&p).0 - 1), f));
    p.comment(n, "after the pass");
    p.commit(IssueOp::Change { n, change: IssueChange::Title("later".into()) }).unwrap();
    let live = fold_digest(&*p.fold.meta, Tier::Metadata);
    for (name, from, frames) in snaps {
        let entries = p.log[from.0 as usize..].iter().cloned();
        let got = refold_segment(&tiers(), METADATA, (ChainId::Repo(REPO), from), frames.clone(), entries).unwrap();
        assert_eq!(got, live, "a refold from the snapshot {name} at {from:?}");
        // `refold-stream`: the staged refold equals a restore into a store at every index, pages of a few rows.
        let tail = &p.log[from.0 as usize..];
        stream_eq::equal_at_every_index(&tiers(), METADATA, ChainId::Repo(REPO), from, &frames, tail, 600);
    }
}

/// A `MetaSlim` entry folds the same on every release (`decisions/DECISIONS.md`, "A 32.20 `MetaSlim` keeps
/// `SLIM_BYTES`"). `a0e4d7bc` bounded each entry
/// by `SLIM_BYTES` in place, at the same format 32.20, so `repo:1048587`'s 512-row slim entries from B0-30's smoke 2
/// refolded on a later release to a metadata digest its members never held, and the fold audit alarmed on every run
/// from 2026-11-14 18:33Z. Pinned to what this slim pass folds to since `a0e4d7bc`, over rows large enough that
/// `SLIM_BYTES`, not `SLIM_ROWS`, ends each entry. A change here needs a new format and SHARED's dual-read release.
#[test]
fn a_slim_pass_folds_to_the_digest_its_release_gave() {
    let mut p = Parent::new();
    let n = p.open_issue("body");
    for i in 0..(SLIM_ROWS + 40) {
        p.comment(n, &format!("comment {i} {}", "s".repeat(1_800)));
    }
    split(&mut p);
    let slims = p
        .log
        .iter()
        .filter(|e| matches!(&e.body, EntryBody::Events(v) if v == &[RepoEvent::Split(SplitEntry::MetaSlim)]))
        .count();
    assert_eq!(slims, 8, "SLIM_BYTES ends each entry before SLIM_ROWS");
    assert_eq!(
        fold_digest(&*p.fold.meta, Tier::Metadata).hex(),
        "dd06e4b5ab5a9c0fca91a8fc693cc61b250de93657504fcb86d5fc30a9902bc7"
    );
}

/// `refold-stream` on the slimmed chain above: from a snapshot of the 552 comments taken before the split, through the
/// split, the seed, the drop and every `MetaSlim` entry, the staged refold equals a restore into a store at every
/// index, with pages of a few rows read through a cache of one page.
#[test]
fn a_staged_refold_through_a_slim_pass_equals_a_restore_at_every_index() {
    let mut p = Parent::new();
    let n = p.open_issue("body");
    for i in 0..(SLIM_ROWS + 40) {
        p.comment(n, &format!("comment {i} {}", "s".repeat(1_800)));
    }
    // Frames read as a snapshot writer reads them, with no `Archived` committed, so the split's own seed is the
    // tier's first snapshot.
    let from = Index(p.log.len() as u64);
    let mut job = SnapshotJob::begin(
        &mut *p.fold.meta,
        METADATA,
        ChainId::Repo(REPO),
        ReleaseId([0; 32]),
        Format { family: 0x20, version: 1 },
    )
    .unwrap();
    let mut frames = Vec::new();
    while let Some(f) = job.next(&p.fold.meta, 50) {
        frames.push(f);
    }
    graftyard_fold::end_snapshot(&mut *p.fold.meta, &METADATA);
    split(&mut p);
    p.comment(n, "after the pass");
    let tail = &p.log[from.0 as usize..];
    let most = stream_eq::equal_at_every_index(&tiers(), METADATA, ChainId::Repo(REPO), from, &frames, tail, 20_000);
    println!("most pages one entry read: {most}");
}
