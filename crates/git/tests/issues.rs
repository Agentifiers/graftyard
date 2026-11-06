//! Issues, pull requests, reviews, rules, and checks on the repository chain's two tiers, driven the way a member
//! drives them: each command decides on a view of both tiers plus the entry's earlier events, its events form an
//! entry tagged by `tier_of`, and every member applies that entry to both tiers.

use graftyard_fold::{MemStore, Read, Tiered, Val, apply, fold_digest, has, meta, refold_digest};
use graftyard_git::issue_ops::{IssueCmd, IssueErr, IssueOp, IssueOut, RepoView, branch_matches, queue_base};
use graftyard_git::issues::{
    AppId, CheckConclusion, CheckRunId, CheckRunUpdate, CheckSource, CheckStatus, CheckSuiteId, CommentTarget,
    HeadSpec, IssueChange, IssueNumber, MergeMethod, PrState, Protection, RepoSettings, RequiredChecks, ReviewState,
    StatusId, StatusState, StatusUpdate, Visibility,
};
use graftyard_git::refs::{ObjectId, RefName, RefUpdate};
use graftyard_git::repo::RepoEvent;
use graftyard_git::tiers::{CORE, METADATA, RepoTiers, RequiredRow, tier_of};
use graftyard_identity::{
    AccountKind, Actor, AuthzEvent, AuthzFold, Credential, IdentityDep, NamesEvent, NamesFold, Principal,
    RepoPermission, ScopeSet, TokenKind, TokensEvent, TokensFold, UserId, shard_of_hash,
};
use graftyard_raft::{EntryBody, Fold, Format, LogEntry, PREV_ZERO, Term, TierSet, digest};
use graftyard_types::{
    AccountId, ChainId, DbId, IdBlock, Index, NameShard, RepoId, RequestId, Tier, Timestamp, TokenShard, Ulid,
};
use smallvec::{SmallVec, smallvec};

const REPO: RepoId = RepoId(DbId(7));
const OWNER: i64 = 100;
const WRITER: i64 = 101;
const READER: i64 = 102;
const DAY: u64 = 86_400_000;

fn tiers() -> RepoTiers {
    RepoTiers { repo: REPO, owner: AccountId(DbId(OWNER)) }
}

fn oid(n: u8) -> ObjectId {
    ObjectId([n; 20])
}

fn name(s: &str) -> RefName {
    RefName::parse(s.as_bytes()).unwrap()
}

fn who(account: i64) -> Principal {
    Principal {
        actor: Actor::User(UserId(AccountId(DbId(account)))),
        scopes: ScopeSet(1),
        credential: Credential::Stored([account as u8; 32]),
        sso_orgs: SmallVec::new(),
    }
}

/// The folds the leader reads once per entry: every user's account and token, and the owner's `Authz` with a
/// write grant for WRITER and a read grant for READER.
fn deps() -> Vec<(ChainId, IdentityDep)> {
    let mut names = NamesFold::new(NameShard(0));
    let mut tokens = TokensFold::new(TokenShard(0));
    let mut authz = AuthzFold::new(AccountId(DbId(OWNER)));
    authz.apply(&AuthzEvent::Opened { kind: AccountKind::User, enterprise: None });
    for a in [OWNER, WRITER, READER] {
        let id = AccountId(DbId(a));
        names.apply(&NamesEvent::Account { id, suspended: false, sessions_not_before: Timestamp(0) });
        let hash = [a as u8; 32];
        tokens.apply(&TokensEvent::Stored {
            hash,
            kind: TokenKind::Pat,
            account: id,
            scopes: ScopeSet(1),
            expires: None,
        });
    }
    for (a, p) in [(WRITER, RepoPermission::Write), (READER, RepoPermission::Read)] {
        authz.apply(&AuthzEvent::Grant { repo: REPO, grantee: UserId(AccountId(DbId(a))), permission: Some(p) });
    }
    let mut out = Vec::new();
    for a in [OWNER, WRITER, READER] {
        let cred = who(a).credential_shard();
        for c in graftyard_identity::credential_chains(&cred, None) {
            let d = match c {
                ChainId::Names(_) => IdentityDep::Names(names.clone()),
                ChainId::Tokens(s) => {
                    assert_eq!(s, shard_of_hash(&[a as u8; 32]));
                    let mut t = tokens.clone();
                    t.shard = Some(s);
                    IdentityDep::Tokens(t)
                }
                _ => unreachable!(),
            };
            out.push((c, d));
        }
    }
    out.push((ChainId::Authz(AccountId(DbId(OWNER))), IdentityDep::Authz(authz)));
    out
}

/// One member: both tiers and the log it applied.
#[derive(Clone, Debug, Default)]
struct Member {
    core: MemStore,
    meta: MemStore,
}

impl Member {
    fn apply(&mut self, e: &LogEntry<RepoEvent>) {
        apply(&tiers(), &mut self.core, Tier::Core, e).unwrap();
        apply(&tiers(), &mut self.meta, Tier::Metadata, e).unwrap();
    }
}

/// A group of members applying one log, and a leader that decides on member 0.
struct Chain {
    members: Vec<Member>,
    log: Vec<LogEntry<RepoEvent>>,
    now: Timestamp,
    deps: Vec<(ChainId, IdentityDep)>,
}

impl Chain {
    fn new(members: usize) -> Chain {
        let mut c =
            Chain { members: vec![Member::default(); members], log: Vec::new(), now: Timestamp(DAY), deps: deps() };
        c.entry(EntryBody::IdBlock(IdBlock::whole(0, 1).unwrap()), TierSet::CORE);
        c
    }

    fn entry(&mut self, body: EntryBody<RepoEvent>, tiers: TierSet) -> LogEntry<RepoEvent> {
        let e = LogEntry {
            term: Term(2),
            index: Index(self.log.len() as u64 + 1),
            prev: self.log.last().map_or(PREV_ZERO, digest),
            format: Format { family: 0x20, version: 3 },
            tiers,
            at: self.now,
            requests: SmallVec::new(),
            extend: None,
            body,
        };
        for m in &mut self.members {
            m.apply(&e);
        }
        self.log.push(e.clone());
        e
    }

    /// Decides `cmds` in order on one view, as one entry, and commits the events of those that passed.
    fn commit_all(&mut self, cmds: &[IssueCmd]) -> Vec<Result<IssueOut, IssueErr>> {
        let m = &self.members[0];
        let mut view = RepoView::new(tiers(), &m.core, &m.meta, self.now);
        let mut events = Vec::new();
        let mut outs = Vec::new();
        for c in cmds {
            let mut needed: Vec<ChainId> = c.deps(&view).into_iter().collect();
            needed.dedup();
            let d: Vec<(ChainId, IdentityDep)> =
                needed.iter().map(|c| self.deps.iter().find(|(k, _)| k == c).expect("a dep").clone()).collect();
            match c.decide(&view, &d) {
                Ok((evs, out)) => {
                    for e in &evs {
                        view.apply(e);
                    }
                    events.extend(evs);
                    outs.push(Ok(out));
                }
                Err(e) => outs.push(Err(e)),
            }
        }
        drop(view);
        if !events.is_empty() {
            let tag = events.iter().fold(TierSet::NONE, |t, e| t.union(tier_of(e)));
            self.entry(EntryBody::Events(events), tag);
        }
        outs
    }

    fn commit(&mut self, actor: i64, op: IssueOp) -> Result<IssueOut, IssueErr> {
        let cmd = IssueCmd { op, actor: who(actor), request: RequestId(Ulid(self.log.len() as u128)) };
        self.commit_all(&[cmd]).pop().unwrap()
    }

    /// A push, as `Push::decide` would commit it.
    fn push(&mut self, branch: &str, new: ObjectId) {
        let r = name(branch);
        let expect = self.ref_of(&r);
        let ev = RepoEvent::Refs {
            push: RequestId(Ulid(9)),
            actor: Some(AccountId(DbId(WRITER))),
            updates: vec![RefUpdate { name: r, expect, new: Some(new) }],
            pack: None,
        };
        self.entry(EntryBody::Events(vec![ev]), TierSet::CORE);
    }

    fn ref_of(&self, r: &RefName) -> Option<ObjectId> {
        let m = &self.members[0];
        RepoView::new(tiers(), &m.core, &m.meta, self.now).ref_value(r)
    }

    fn rows(&self, tier: Tier, table: &str) -> usize {
        let m = &self.members[0];
        let s = if tier == Tier::Core { &m.core } else { &m.meta };
        s.0.get(table).map_or(0, |t| t.len())
    }

    fn required(&self, sha: ObjectId) -> Vec<(Box<str>, Option<CheckConclusion>)> {
        let m = &self.members[0];
        m.core
            .prefix("required_check", &[Val::Blob(sha.0.to_vec().into())])
            .into_iter()
            .map(|(k, v)| {
                let Some(Val::Text(n)) = k.get(1) else { panic!() };
                let Some(Val::Blob(b)) = v.first() else { panic!() };
                let row: RequiredRow = postcard::from_bytes(b).unwrap();
                (n.clone(), row.conclusion)
            })
            .collect()
    }

    fn agree(&self) {
        for tier in [Tier::Core, Tier::Metadata] {
            let schema = if tier == Tier::Core { CORE } else { METADATA };
            let d: Vec<_> = self
                .members
                .iter()
                .map(|m| {
                    let s = if tier == Tier::Core { &m.core } else { &m.meta };
                    assert_eq!(fold_digest(s, tier), refold_digest(s, &schema), "kept sum equals the rows");
                    fold_digest(s, tier)
                })
                .collect();
            assert!(d.windows(2).all(|w| w[0] == w[1]), "{tier:?} digests differ: {d:?}");
        }
    }
}

fn settings(retention: u16) -> RepoSettings {
    RepoSettings {
        owner: AccountId(DbId(OWNER)),
        enterprise: None,
        visibility: Visibility::Private,
        default_branch: name("refs/heads/main"),
        archived: false,
        description: None,
        homepage: None,
        topics: SmallVec::new(),
        has_issues: true,
        has_wiki: false,
        has_projects: false,
        merge_methods: smallvec![MergeMethod::Merge],
        delete_branch_on_merge: false,
        allow_auto_merge: false,
        allow_update_branch: false,
        check_retention_days: retention,
    }
}

fn protect(pattern: &str, checks: &[&str]) -> IssueOp {
    let checks = checks.iter().map(|c| (Box::from(*c), None)).collect();
    IssueOp::SetRule {
        id: None,
        pattern: pattern.into(),
        protection: Some(Protection {
            checks: Some(RequiredChecks { strict: false, checks }),
            reviews: None,
            push: None,
            enforce_admins: false,
            linear_history: false,
            force_pushes: false,
            deletions: false,
            block_creations: false,
            conversation_resolution: false,
            lock_branch: false,
            signatures: false,
            merge_queue: None,
        }),
    }
}

fn open_pr(c: &mut Chain, branch: &str, head: ObjectId) -> IssueNumber {
    let base_sha = c.ref_of(&name("refs/heads/main")).unwrap();
    let op = IssueOp::OpenPr {
        head: HeadSpec { repo: REPO, branch: name(branch) },
        base: name("refs/heads/main"),
        title: "t".into(),
        body: "".into(),
        draft: false,
        head_sha: head,
        base_sha,
    };
    match c.commit(WRITER, op).unwrap() {
        IssueOut::Number(n) => n,
        o => panic!("{o:?}"),
    }
}

fn run(id: i64, sha: ObjectId, check: &str, version: u64, conclusion: Option<CheckConclusion>) -> IssueOp {
    IssueOp::RecordCheck(CheckRunUpdate {
        run: CheckRunId(DbId(id)),
        suite: CheckSuiteId(DbId(1)),
        app: AppId(DbId(5)),
        head_sha: sha,
        name: check.into(),
        version,
        status: if conclusion.is_some() { CheckStatus::Completed } else { CheckStatus::InProgress },
        conclusion,
        started: None,
        completed: None,
        external_id: None,
        details_url: None,
        output: None,
    })
}

/// A repository with `main` at oid 1, settings, and a rule on `main` requiring `ci`.
fn setup(members: usize, retention: u16) -> Chain {
    let mut c = Chain::new(members);
    c.push("refs/heads/main", oid(1));
    c.commit(OWNER, IssueOp::Settings(settings(retention))).unwrap();
    c.commit(OWNER, protect("main", &["ci"])).unwrap();
    c
}

#[test]
fn tier_of_routes_each_event_to_the_tiers_shared_names() {
    let both = TierSet::CORE.union(TierSet::METADATA);
    let mut c = setup(1, 90);
    c.push("refs/heads/f", oid(2));
    let n = open_pr(&mut c, "refs/heads/f", oid(2));
    let tags: Vec<TierSet> = c.log.iter().skip(1).map(|e| e.tiers).collect();
    // push, settings, rule (Core alone: it takes its id from Core's cursor), push, PrOpened
    assert_eq!(tags, vec![TierSet::CORE, both, TierSet::CORE, TierSet::CORE, both]);
    let comment = RepoEvent::Comment(graftyard_git::issues::CommentEvent::Edited {
        id: graftyard_git::issues::CommentId(DbId(1)),
        body: "b".into(),
        actor: None,
    });
    assert_eq!(tier_of(&comment), TierSet::METADATA);
    let resolved = RepoEvent::Comment(graftyard_git::issues::CommentEvent::Resolved {
        thread: graftyard_git::issues::CommentId(DbId(1)),
        resolved: true,
        actor: None,
    });
    assert_eq!(tier_of(&resolved), both, "a resolution changes the core unresolved count");
    let req = RepoEvent::RequiredCheck {
        sha: oid(2),
        name: "ci".into(),
        source: CheckSource::Status { creator: AccountId(DbId(1)) },
        version: 1,
        conclusion: None,
    };
    assert_eq!(tier_of(&req), TierSet::CORE);
    let title = RepoEvent::IssueChanged { n, change: IssueChange::Title("x".into()), actor: None };
    assert_eq!(tier_of(&title), TierSet::METADATA);
    let draft = RepoEvent::IssueChanged { n, change: IssueChange::Draft(true), actor: None };
    assert!(has(tier_of(&draft), Tier::Core));
    // What each tier holds after these entries.
    assert_eq!(c.rows(Tier::Core, "pull_head"), 1);
    assert_eq!(c.rows(Tier::Metadata, "pull"), 1);
    assert_eq!(c.rows(Tier::Core, "issue"), 0);
    assert_eq!(c.rows(Tier::Metadata, "pull_head"), 0);
    assert_eq!(c.rows(Tier::Metadata, "ref"), 0, "refs never reach the metadata tier");
    for tier in [Tier::Core, Tier::Metadata] {
        let m = &c.members[0];
        let s = if tier == Tier::Core { &m.core } else { &m.meta };
        assert_eq!(meta(s).applied, Index(c.log.len() as u64), "{tier:?} applied every entry");
    }
}

#[test]
fn issue_numbers_are_one_space_and_commands_in_one_entry_see_each_other() {
    let mut c = setup(1, 90);
    c.push("refs/heads/f", oid(2));
    let open = |t: &str| IssueCmd {
        op: IssueOp::OpenIssue {
            title: t.into(),
            body: "".into(),
            labels: SmallVec::new(),
            assignees: SmallVec::new(),
        },
        actor: who(READER),
        request: RequestId(Ulid(1)),
    };
    let outs = c.commit_all(&[open("a"), open("b")]);
    assert_eq!(outs, vec![Ok(IssueOut::Number(IssueNumber(1))), Ok(IssueOut::Number(IssueNumber(2)))]);
    assert_eq!(open_pr(&mut c, "refs/heads/f", oid(2)), IssueNumber(3), "pull requests share the space");
    // A second open pull request from the same head to the same base is refused.
    let base_sha = c.ref_of(&name("refs/heads/main")).unwrap();
    let again = IssueOp::OpenPr {
        head: HeadSpec { repo: REPO, branch: name("refs/heads/f") },
        base: name("refs/heads/main"),
        title: "t".into(),
        body: "".into(),
        draft: false,
        head_sha: oid(2),
        base_sha,
    };
    assert_eq!(c.commit(WRITER, again), Err(IssueErr::Exists));
    assert_eq!(
        c.commit(READER, IssueOp::Change { n: IssueNumber(9), change: IssueChange::Title("x".into()) }),
        Err(IssueErr::NotFound)
    );
    assert_eq!(
        c.commit(READER, IssueOp::SetRule { id: None, pattern: "x".into(), protection: None }),
        Err(IssueErr::Forbidden)
    );
}

#[test]
fn a_required_check_is_recorded_only_for_a_guarded_sha_and_pruned_when_its_head_moves() {
    let mut c = setup(3, 90);
    c.push("refs/heads/f", oid(2));
    let n = open_pr(&mut c, "refs/heads/f", oid(2));
    c.commit(WRITER, run(1, oid(2), "ci", 1, None)).unwrap();
    c.commit(WRITER, run(2, oid(2), "lint", 1, Some(CheckConclusion::Success))).unwrap();
    c.commit(WRITER, run(3, oid(9), "ci", 1, Some(CheckConclusion::Success))).unwrap();
    assert_eq!(c.required(oid(2)), vec![("ci".into(), None)], "only ci is required, and only on the head");
    assert_eq!(c.required(oid(9)), vec![], "oid 9 heads nothing");
    assert_eq!(c.rows(Tier::Metadata, "check_run"), 3, "every run is a metadata row");
    c.commit(WRITER, run(1, oid(2), "ci", 2, Some(CheckConclusion::Failure))).unwrap();
    c.commit(WRITER, run(1, oid(2), "ci", 1, None)).unwrap();
    assert_eq!(c.required(oid(2)), vec![("ci".into(), Some(CheckConclusion::Failure))], "the highest version wins");
    // A re-run is a new run with no conclusion: it blocks again, and the old run's late update does not unblock it.
    c.commit(WRITER, run(4, oid(2), "ci", 1, None)).unwrap();
    assert_eq!(c.required(oid(2)), vec![("ci".into(), None)]);
    c.commit(WRITER, run(1, oid(2), "ci", 3, Some(CheckConclusion::Success))).unwrap();
    assert_eq!(c.required(oid(2)), vec![("ci".into(), None)]);
    // The branch tip of a protected branch is guarded too.
    c.commit(WRITER, run(5, oid(1), "ci", 1, Some(CheckConclusion::Success))).unwrap();
    assert_eq!(c.required(oid(1)).len(), 1);
    // A push moves the pull request's head: its old head's required rows go, metadata rows stay.
    c.push("refs/heads/f", oid(3));
    assert_eq!(c.required(oid(2)), vec![]);
    assert_eq!(c.rows(Tier::Metadata, "check_run"), 5);
    assert_eq!(c.required(oid(1)).len(), 1, "a branch tip's row is untouched");
    c.commit(WRITER, run(6, oid(3), "ci", 1, None)).unwrap();
    assert_eq!(c.required(oid(3)).len(), 1);
    // Closing prunes the head's rows.
    c.commit(WRITER, IssueOp::Close { n }).unwrap();
    assert_eq!(c.required(oid(3)), vec![]);
    c.agree();
}

#[test]
fn a_required_row_shared_by_two_open_pull_requests_stays_until_neither_heads_it() {
    let mut c = setup(1, 90);
    c.push("refs/heads/a", oid(2));
    c.push("refs/heads/b", oid(2));
    let a = open_pr(&mut c, "refs/heads/a", oid(2));
    let _b = open_pr(&mut c, "refs/heads/b", oid(2));
    c.commit(WRITER, run(1, oid(2), "ci", 1, Some(CheckConclusion::Success))).unwrap();
    c.commit(WRITER, IssueOp::Close { n: a }).unwrap();
    assert_eq!(c.required(oid(2)).len(), 1, "b still has oid 2 as its head");
    c.push("refs/heads/b", oid(4));
    assert_eq!(c.required(oid(2)), vec![]);
}

#[test]
fn a_required_status_maps_its_state_and_a_merge_queue_ref_is_guarded() {
    let mut c = setup(1, 90);
    c.commit(OWNER, protect("main", &["ci", "jenkins"])).unwrap();
    c.push("refs/heads/gh-readonly-queue/main/pr-1-0101", oid(7));
    assert_eq!(queue_base(&name("refs/heads/gh-readonly-queue/main/pr-1-0101")), Some(name("refs/heads/main")));
    let status = |id: i64, state| {
        IssueOp::RecordStatus(StatusUpdate {
            id: StatusId(DbId(id)),
            sha: oid(7),
            context: "jenkins".into(),
            state,
            target_url: None,
            description: None,
            creator: AccountId(DbId(WRITER)),
        })
    };
    c.commit(WRITER, status(10, StatusState::Pending)).unwrap();
    assert_eq!(c.required(oid(7)), vec![("jenkins".into(), None)]);
    c.commit(WRITER, status(11, StatusState::Error)).unwrap();
    assert_eq!(c.required(oid(7)), vec![("jenkins".into(), Some(CheckConclusion::Failure))]);
    c.commit(WRITER, status(12, StatusState::Success)).unwrap();
    assert_eq!(c.required(oid(7)), vec![("jenkins".into(), Some(CheckConclusion::Success))]);
    c.commit(WRITER, status(9, StatusState::Failure)).unwrap();
    assert_eq!(c.required(oid(7)), vec![("jenkins".into(), Some(CheckConclusion::Success))], "an older status");
    assert_eq!(c.rows(Tier::Metadata, "status"), 4);
    assert_eq!(c.commit(READER, status(13, StatusState::Success)), Err(IssueErr::Forbidden));
}

/// The edge mints a status's id from its clock, so two statuses on one commit can arrive with the same id (two in one
/// millisecond, BF-22's hosted run lost 5 of 500 that way). Each keeps its own row, and the later one is the newer.
#[test]
fn two_statuses_minted_in_one_millisecond_keep_both_rows_and_the_later_wins() {
    let mut c = setup(1, 90);
    c.commit(OWNER, protect("main", &["ci"])).unwrap();
    c.push("refs/heads/main", oid(7));
    let status = |context: &str, state, request: u128| IssueCmd {
        op: IssueOp::RecordStatus(StatusUpdate {
            id: StatusId(DbId(1_793_947_000_000)),
            sha: oid(7),
            context: context.into(),
            state,
            target_url: None,
            description: None,
            creator: AccountId(DbId(WRITER)),
        }),
        actor: who(WRITER),
        request: RequestId(Ulid(request)),
    };
    let outs = c.commit_all(&[status("a", StatusState::Success, 1), status("b", StatusState::Success, 2)]);
    assert_eq!(outs.len(), 2);
    let [Ok(IssueOut::Id(a)), Ok(IssueOut::Id(b))] = [outs[0].clone(), outs[1].clone()] else { panic!("{outs:?}") };
    assert!(b.0 > a.0, "ids on one commit are unique and increase in commit order: {a:?} {b:?}");
    c.commit(WRITER, status("ci", StatusState::Pending, 3).op).unwrap();
    c.commit(WRITER, status("ci", StatusState::Success, 4).op).unwrap();
    assert_eq!(c.rows(Tier::Metadata, "status"), 4, "no status replaced another");
    assert_eq!(c.required(oid(7)), vec![("ci".into(), Some(CheckConclusion::Success))], "the later status wins");
}

#[test]
fn check_rows_are_pruned_by_entry_time_past_retention_on_every_member_and_required_rows_are_not() {
    let mut c = setup(3, 2);
    c.push("refs/heads/f", oid(2));
    let _n = open_pr(&mut c, "refs/heads/f", oid(2));
    let core_before = c.members[0].core.clone();
    c.commit(WRITER, run(1, oid(2), "ci", 1, Some(CheckConclusion::Success))).unwrap();
    c.commit(WRITER, run(2, oid(2), "other", 1, Some(CheckConclusion::Success))).unwrap();
    let core_after_checks = c.members[0].core.clone();
    assert_ne!(core_before, core_after_checks, "the required ci row is in core");
    // Run 2 is updated a day later: its time is that entry's, so it outlives run 1.
    c.now = Timestamp(c.now.0 + DAY);
    c.commit(WRITER, run(2, oid(2), "other", 2, Some(CheckConclusion::Failure))).unwrap();
    // Exactly the retention after run 1 was written: kept (pruned only once `at` passes it).
    c.now = Timestamp(DAY + 2 * DAY);
    c.commit(READER, IssueOp::Comment { on: CommentTarget::Issue(IssueNumber(1)), body: "x".into(), reply_to: None })
        .unwrap();
    assert_eq!(c.rows(Tier::Metadata, "check_run"), 2);
    // One millisecond later, an entry that touches only the core tier still prunes the metadata tier.
    c.now = Timestamp(c.now.0 + 1);
    c.push("refs/heads/other", oid(8));
    assert_eq!(c.rows(Tier::Metadata, "check_run"), 1, "run 1 pruned by entry time");
    assert_eq!(c.required(oid(2)).len(), 1, "a required row is never pruned by time");
    c.now = Timestamp(c.now.0 + DAY);
    c.push("refs/heads/other", oid(9));
    assert_eq!(c.rows(Tier::Metadata, "check_run"), 0);
    assert_eq!(c.rows(Tier::Metadata, "check_expiry"), 0);
    assert_eq!(c.required(oid(2)).len(), 1);
    c.agree();
}

#[test]
fn a_late_member_reaches_the_same_digests_from_the_same_log() {
    let mut c = setup(2, 1);
    c.push("refs/heads/f", oid(2));
    let n = open_pr(&mut c, "refs/heads/f", oid(2));
    c.commit(WRITER, run(1, oid(2), "ci", 1, None)).unwrap();
    c.commit(READER, IssueOp::Review { n, state: ReviewState::Approved, commit: oid(2), body: "ok".into() }).unwrap();
    c.commit(WRITER, IssueOp::Change { n, change: IssueChange::Draft(true) }).unwrap();
    c.now = Timestamp(c.now.0 + 5 * DAY);
    c.push("refs/heads/f", oid(3));
    let mut late = Member::default();
    for e in &c.log {
        late.apply(e);
    }
    for tier in [Tier::Core, Tier::Metadata] {
        let (a, b) =
            if tier == Tier::Core { (&c.members[0].core, &late.core) } else { (&c.members[0].meta, &late.meta) };
        assert_eq!(fold_digest(a, tier), fold_digest(b, tier));
    }
    let m = &c.members[0];
    let view = RepoView::new(tiers(), &m.core, &m.meta, c.now);
    assert_eq!(view.last_number(), 1);
    let ph: graftyard_git::issues::PullHead = {
        let Some(Val::Blob(b)) = m.core.get("pull_head", &[Val::Int(1)]).and_then(|v| v.into_iter().next()) else {
            panic!()
        };
        postcard::from_bytes(&b).unwrap()
    };
    assert_eq!((ph.head_sha, ph.state), (oid(3), PrState::Open { draft: true }));
}

#[test]
fn a_reader_cannot_approve_their_own_pull_request_and_a_dismissal_removes_the_core_verdict() {
    let mut c = setup(1, 90);
    c.push("refs/heads/f", oid(2));
    let n = open_pr(&mut c, "refs/heads/f", oid(2));
    let approve = IssueOp::Review { n, state: ReviewState::Approved, commit: oid(2), body: "".into() };
    assert_eq!(c.commit(WRITER, approve.clone()), Err(IssueErr::OwnApproval));
    let Ok(IssueOut::Id(id)) = c.commit(READER, approve) else { panic!() };
    assert_eq!(c.rows(Tier::Core, "review_verdict"), 1);
    c.commit(WRITER, IssueOp::Dismiss { review: graftyard_git::issues::ReviewId(id), message: "stale".into() })
        .unwrap();
    assert_eq!(c.rows(Tier::Core, "review_verdict"), 0);
    assert_eq!(c.rows(Tier::Metadata, "review"), 1, "the body stays in metadata");
}

#[test]
fn settings_refuse_an_owner_change_and_a_retention_out_of_range() {
    let mut c = setup(1, 90);
    let mut s = settings(0);
    assert!(matches!(c.commit(OWNER, IssueOp::Settings(s.clone())), Err(IssueErr::Invalid(_))));
    s.check_retention_days = 401;
    assert!(matches!(c.commit(OWNER, IssueOp::Settings(s.clone())), Err(IssueErr::Invalid(_))));
    s.check_retention_days = 400;
    s.owner = AccountId(DbId(5));
    assert_eq!(c.commit(OWNER, IssueOp::Settings(s)), Err(IssueErr::OwnerChange));
    assert_eq!(c.commit(WRITER, IssueOp::Settings(settings(30))), Err(IssueErr::Forbidden));
}

#[test]
fn branch_patterns_match_as_fnmatch() {
    let m = |p: &str, b: &str| branch_matches(p, &name(&format!("refs/heads/{b}")));
    assert!(m("main", "main"));
    assert!(!m("main", "main2"));
    assert!(m("release/*", "release/1"));
    assert!(!m("release/*", "release/1/x"));
    assert!(m("release/**", "release/1/x"));
    assert!(m("v[0-9]", "v7"));
    assert!(!m("v[!0-9]", "v7"));
    assert!(m("fe?ture", "feature"));
    assert!(!branch_matches("main", &name("refs/tags/main")));
}

#[test]
fn a_command_that_touches_no_tier_it_tags_is_impossible() {
    // Every op's events fit the tiers the command declares.
    let mut c = setup(1, 90);
    c.push("refs/heads/f", oid(2));
    let n = open_pr(&mut c, "refs/heads/f", oid(2));
    let ops = vec![
        IssueOp::Change { n, change: IssueChange::Draft(true) },
        IssueOp::Change { n, change: IssueChange::Title("t2".into()) },
        run(1, oid(2), "ci", 1, None),
        IssueOp::Comment { on: CommentTarget::Issue(n), body: "b".into(), reply_to: None },
        protect("dev", &["x"]),
        IssueOp::Close { n },
    ];
    for op in ops {
        let cmd = IssueCmd { op, actor: who(OWNER), request: RequestId(Ulid(1)) };
        let m = &c.members[0];
        let view = RepoView::new(tiers(), &m.core, &m.meta, c.now);
        let needed: Vec<ChainId> = cmd.deps(&view).into_iter().collect();
        let d: Vec<_> = needed.iter().map(|k| c.deps.iter().find(|(x, _)| x == k).unwrap().clone()).collect();
        let (evs, _) = cmd.decide(&view, &d).unwrap();
        for e in &evs {
            assert_eq!(tier_of(e).0 & !cmd.tiers().0, 0, "{:?} writes outside {:?}", e, cmd.tiers());
        }
        drop(view);
        c.commit_all(&[cmd]);
    }
    let _ = RepoTiers::schema(Tier::Core);
}

fn review_comment(c: &mut Chain, n: IssueNumber, reply_to: Option<graftyard_git::issues::CommentId>) -> DbId {
    let at = graftyard_git::issues::LineAnchor {
        commit: oid(2),
        path: "a.rs".into(),
        line: 1,
        start_line: None,
        side: graftyard_git::issues::DiffSide::Right,
    };
    let on = CommentTarget::Review { n, review: None, at };
    match c.commit(READER, IssueOp::Comment { on, body: "nit".into(), reply_to }).unwrap() {
        IssueOut::Id(id) => id,
        o => panic!("{o:?}"),
    }
}

fn resolve(c: &mut Chain, thread: DbId, resolved: bool) {
    let thread = graftyard_git::issues::CommentId(thread);
    c.commit(WRITER, IssueOp::Resolve { thread, resolved }).unwrap();
}

fn unresolved(c: &Chain, n: IssueNumber) -> u32 {
    graftyard_git::tiers::unresolved_threads(&c.members[0].core, n)
}

#[test]
fn the_core_tier_counts_unresolved_review_threads_per_pull_request() {
    let mut c = setup(3, 90);
    c.push("refs/heads/f", oid(2));
    c.push("refs/heads/g", oid(3));
    let n = open_pr(&mut c, "refs/heads/f", oid(2));
    let other = open_pr(&mut c, "refs/heads/g", oid(3));
    let a = review_comment(&mut c, n, None);
    let b = review_comment(&mut c, n, None);
    let _reply = review_comment(&mut c, n, Some(graftyard_git::issues::CommentId(a)));
    let o = review_comment(&mut c, other, None);
    c.commit(READER, IssueOp::Comment { on: CommentTarget::Issue(n), body: "plain".into(), reply_to: None }).unwrap();
    assert_eq!((unresolved(&c, n), unresolved(&c, other)), (2, 1), "a reply and an issue comment start no thread");
    resolve(&mut c, a, true);
    assert_eq!(unresolved(&c, n), 1);
    resolve(&mut c, a, true);
    assert_eq!(unresolved(&c, n), 1, "resolving twice counts once");
    resolve(&mut c, a, false);
    assert_eq!(unresolved(&c, n), 2, "unresolving counts again");
    resolve(&mut c, a, false);
    assert_eq!(unresolved(&c, n), 2);
    // Deleting a resolved thread leaves the count; deleting an unresolved one lowers it.
    resolve(&mut c, b, true);
    assert_eq!(unresolved(&c, n), 1);
    c.commit(READER, IssueOp::DeleteComment { id: graftyard_git::issues::CommentId(b) }).unwrap();
    assert_eq!(unresolved(&c, n), 1, "a resolved thread was already uncounted");
    c.commit(READER, IssueOp::DeleteComment { id: graftyard_git::issues::CommentId(a) }).unwrap();
    assert_eq!(unresolved(&c, n), 0);
    assert_eq!(c.rows(Tier::Core, "unresolved"), 1, "no row for a zero count; `other` keeps one");
    // A thread on a closed pull request still counts, and resolving it there is kept for a reopen.
    c.commit(WRITER, IssueOp::Close { n: other }).unwrap();
    assert_eq!(unresolved(&c, other), 1);
    resolve(&mut c, o, true);
    assert_eq!(unresolved(&c, other), 0);
    c.commit(WRITER, IssueOp::Change { n: other, change: IssueChange::State { open: true, reason: None } }).unwrap();
    assert_eq!(unresolved(&c, other), 0);
    let t = review_comment(&mut c, other, None);
    assert_eq!(unresolved(&c, other), 1);
    // Each change is in the core tier's digest on every member, and a late member gets the same counts.
    c.agree();
    let mut late = Member::default();
    for e in &c.log {
        late.apply(e);
    }
    assert_eq!(fold_digest(&late.core, Tier::Core), fold_digest(&c.members[0].core, Tier::Core));
    assert_eq!(graftyard_git::tiers::unresolved_threads(&late.core, other), 1);
    // The resolution entry is tagged with both tiers; a plain comment's only Metadata.
    resolve(&mut c, t, true);
    assert_eq!(c.log.last().unwrap().tiers, TierSet::CORE.union(TierSet::METADATA));
    c.commit(READER, IssueOp::Comment { on: CommentTarget::Issue(n), body: "x".into(), reply_to: None }).unwrap();
    assert_eq!(c.log.last().unwrap().tiers, TierSet::METADATA);
}

/// After a simulated `MetaSplit`, the parent keeps only Core and the child only Metadata
/// (`design/DESIGN.md`, Shape, `CHILD_GROUP_AT`). Each assigns ids from its own cursor and reads nothing of the other.
#[test]
fn after_a_split_each_tier_assigns_ids_from_its_own_cursor_with_no_collision() {
    use std::collections::BTreeSet;
    let mut c = setup(1, 90);
    c.push("refs/heads/f", oid(2));
    let n = open_pr(&mut c, "refs/heads/f", oid(2));
    let split = c.members[0].clone();
    let at = c.log.len();
    let mut ids = BTreeSet::new();
    let mut take = |out: Result<IssueOut, IssueErr>| match out.unwrap() {
        IssueOut::Id(id) => assert!(ids.insert(id), "id {id:?} assigned twice"),
        IssueOut::Number(_) | IssueOut::Done => {}
    };
    // The parent: core-tier commands decided on a view whose metadata tier is empty and never written, as a parent
    // after `MetaDropped` holds none.
    let empty = MemStore::default();
    let mut parent = split.core.clone();
    let mut core_ids = Vec::new();
    for i in 0..40u8 {
        let op = if i % 2 == 0 {
            protect(&format!("rel{i}"), &["ci"])
        } else {
            IssueOp::Review { n, state: ReviewState::Commented, commit: oid(2), body: "".into() }
        };
        let cmd = IssueCmd { op, actor: who(OWNER), request: RequestId(Ulid(u128::from(i))) };
        let view = RepoView::new(tiers(), &parent, &empty, c.now);
        let needed: Vec<ChainId> = cmd.deps(&view).into_iter().collect();
        let d: Vec<_> = needed.iter().map(|k| c.deps.iter().find(|(x, _)| x == k).unwrap().clone()).collect();
        let (evs, out) = cmd.decide(&view, &d).unwrap();
        drop(view);
        if let IssueOut::Id(id) = out {
            core_ids.push(id);
        }
        take(Ok(out));
        let idx = at + core_ids.len();
        // A both-tier entry commits whole on the parent, which applies only its core rows.
        let tag = evs.iter().fold(TierSet::NONE, |t, e| t.union(tier_of(e)));
        let e = LogEntry {
            term: Term(2),
            index: Index(idx as u64),
            prev: graftyard_fold::meta(&parent).last,
            format: Format { family: 0x20, version: 3 },
            tiers: tag,
            at: c.now,
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(evs.into_vec()),
        };
        apply(&tiers(), &mut parent, Tier::Core, &e).unwrap();
    }
    // The child: metadata-only commands on a view whose core tier is the split point's, frozen.
    let mut child = split.meta.clone();
    let mut meta_ids = Vec::new();
    for i in 0..40u8 {
        let op = if i % 2 == 0 {
            IssueOp::Comment { on: CommentTarget::Issue(n), body: "c".into(), reply_to: None }
        } else {
            IssueOp::DefineLabel {
                id: None,
                name: graftyard_git::issues::LabelName(format!("l{i}").into()),
                color: [0; 3],
                description: None,
            }
        };
        let cmd = IssueCmd { op, actor: who(OWNER), request: RequestId(Ulid(100 + u128::from(i))) };
        assert_eq!(cmd.tiers(), TierSet::METADATA);
        let view = RepoView::new(tiers(), &split.core, &child, c.now);
        let needed: Vec<ChainId> = cmd.deps(&view).into_iter().collect();
        let d: Vec<_> = needed.iter().map(|k| c.deps.iter().find(|(x, _)| x == k).unwrap().clone()).collect();
        let (evs, out) = cmd.decide(&view, &d).unwrap();
        drop(view);
        if let IssueOut::Id(id) = out {
            meta_ids.push(id);
        }
        take(Ok(out));
        let e = LogEntry {
            term: Term(2),
            index: Index((at + meta_ids.len()) as u64),
            prev: graftyard_fold::meta(&child).last,
            format: Format { family: 0x20, version: 3 },
            tiers: TierSet::METADATA,
            at: c.now,
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(evs.into_vec()),
        };
        apply(&tiers(), &mut child, Tier::Metadata, &e).unwrap();
    }
    assert_eq!((core_ids.len(), meta_ids.len()), (40, 40));
    assert!(core_ids.windows(2).all(|w| w[0] < w[1]) && meta_ids.windows(2).all(|w| w[0] < w[1]));
    // Both halves come from the one block the group leased.
    let block = IdBlock::whole(0, 1).unwrap();
    for id in core_ids.iter().chain(&meta_ids) {
        assert!(block.index_of(*id).is_some(), "{id:?} is outside the leased block");
    }
}

// ---- releases, assets, and LFS (M2-WP4) ----

fn durable(bytes: &[u8]) -> graftyard_git::blob::DurableBlob {
    use graftyard_git::blob::BlobStore;
    use graftyard_git::mem::MemBucket;
    use graftyard_types::{Region, Site, SiteId, SiteLink, SiteTable};
    let site = |id: u8, region| Site {
        id: SiteId(id),
        script: format!("s{id}"),
        account: "x".into(),
        region,
        jurisdiction: None,
        bucket: format!("b{id}"),
        release: None,
        link: SiteLink::Binding { member_class: "Member".into() },
        spare: false,
    };
    let sites = SiteTable(vec![site(0, Region::Enam), site(1, Region::Weur), site(2, Region::Apac)]);
    let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
    let bk = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
    let store = BlobStore { sites: &sites, buckets: &bk, copies: graftyard_git::Copies::TwoRegions };
    let meta = graftyard_git::ObjectMeta { chain: ChainId::Repo(REPO), request: RequestId(Ulid(1)) };
    futures_executor::block_on(store.place(REPO, bytes, None, meta)).unwrap()
}

fn release_op(id: Option<DbId>, tag: &str, draft: bool) -> IssueOp {
    IssueOp::PutRelease {
        id: id.map(graftyard_git::issues::RepoReleaseId),
        tag: name(tag),
        target: oid(1),
        name: "".into(),
        body: "".into(),
        draft,
        prerelease: false,
        latest: true,
    }
}

fn id_of(out: Result<IssueOut, IssueErr>) -> DbId {
    match out.unwrap() {
        IssueOut::Id(id) => id,
        o => panic!("{o:?}"),
    }
}

#[test]
fn publishing_a_release_creates_its_tag_in_core_and_a_draft_does_not() {
    use graftyard_git::tiers::{release_by_tag, release_row};
    let mut c = setup(3, 90);
    let draft = id_of(c.commit(WRITER, release_op(None, "refs/tags/v2", true)));
    assert_eq!(c.log.last().unwrap().tiers, TierSet::METADATA, "a draft touches only Metadata");
    assert_eq!(c.ref_of(&name("refs/tags/v2")), None);
    let pubd = id_of(c.commit(WRITER, release_op(None, "refs/tags/v1", false)));
    assert_eq!(c.log.last().unwrap().tiers, TierSet::CORE.union(TierSet::METADATA));
    assert_eq!(c.ref_of(&name("refs/tags/v1")), Some(oid(1)), "the tag is created in the same entry");
    // Publishing the draft (gh's `PATCH {draft:false}` after its uploads) creates its tag then.
    c.commit(WRITER, release_op(Some(draft), "refs/tags/v2", false)).unwrap();
    assert_eq!(c.ref_of(&name("refs/tags/v2")), Some(oid(1)));
    // An existing tag wins over the requested target, and a second release on one tag is refused.
    c.push("refs/tags/v3", oid(9));
    let third = id_of(c.commit(WRITER, release_op(None, "refs/tags/v3", false)));
    let m = &c.members[0];
    assert_eq!(release_row(&m.meta, graftyard_git::issues::RepoReleaseId(third)).unwrap().rec.target, oid(9));
    assert_eq!(c.commit(WRITER, release_op(None, "refs/tags/v1", false)), Err(IssueErr::Exists));
    assert_eq!(c.commit(READER, release_op(None, "refs/tags/v9", false)), Err(IssueErr::Forbidden));
    assert_eq!(
        c.commit(WRITER, release_op(None, "refs/heads/main", false)).map(|_| ()).unwrap_err(),
        IssueErr::Invalid("a release tag is under refs/tags/")
    );
    let m = &c.members[0];
    assert_eq!(release_by_tag(&m.meta, &name("refs/tags/v1")), Some(graftyard_git::issues::RepoReleaseId(pubd)));
    c.agree();
}

#[test]
fn an_asset_names_a_durable_blob_and_goes_with_its_release() {
    use graftyard_git::issues::{AssetId, RepoReleaseId};
    use graftyard_git::tiers::AssetRow;
    let mut c = setup(3, 90);
    let rel = RepoReleaseId(id_of(c.commit(WRITER, release_op(None, "refs/tags/v1", false))));
    let blob = durable(b"app");
    let put = |name: &str| IssueOp::PutAsset {
        release: rel,
        name: name.into(),
        label: None,
        content_type: "application/gzip".into(),
        blob,
    };
    let a = AssetId(id_of(c.commit(WRITER, put("app.tgz"))));
    assert_eq!(c.log.last().unwrap().tiers, TierSet::METADATA);
    assert_eq!(c.commit(WRITER, put("app.tgz")), Err(IssueErr::Exists), "one name per release");
    let _b = id_of(c.commit(WRITER, put("notes.txt")));
    let row: AssetRow = graftyard_git::tiers::get_rec(&c.members[0].meta, "asset", &[Val::Int(a.0.0)]).unwrap();
    assert_eq!(row.rec.blob, blob.blob());
    assert_eq!(row.uploader, Some(AccountId(DbId(WRITER))));
    // gh's `--clobber`: delete the asset of that name, then upload again.
    c.commit(WRITER, IssueOp::DeleteAsset { id: a }).unwrap();
    assert_eq!(c.rows(Tier::Metadata, "asset"), 1);
    id_of(c.commit(WRITER, put("app.tgz")));
    // Deleting the release deletes its assets; its tag stays, as GitHub's does until `--cleanup-tag` deletes the ref.
    c.commit(WRITER, IssueOp::DeleteRelease { id: rel }).unwrap();
    assert_eq!((c.rows(Tier::Metadata, "asset"), c.rows(Tier::Metadata, "asset_by_release")), (0, 0));
    assert_eq!(c.rows(Tier::Metadata, "release_by_tag"), 0);
    assert!(c.ref_of(&name("refs/tags/v1")).is_some());
    assert_eq!(c.commit(WRITER, put("late")), Err(IssueErr::NotFound));
    c.agree();
}

#[test]
fn lfs_objects_and_locks_are_metadata_rows() {
    use graftyard_git::issues::LockId;
    use graftyard_git::tiers::{LfsRow, get_rec, lfs_key};
    let mut c = setup(3, 90);
    let blob = durable(b"lfs object");
    c.commit(WRITER, IssueOp::LfsObject(blob)).unwrap();
    assert_eq!(c.log.last().unwrap().tiers, TierSet::METADATA);
    let row: LfsRow = get_rec(&c.members[0].meta, "lfs_object", &[lfs_key(&blob.blob().oid)]).unwrap();
    assert_eq!(row.blob, blob.blob());
    assert_eq!(c.commit(READER, IssueOp::LfsObject(blob)), Err(IssueErr::Forbidden), "an upload needs write");
    let lock = |path: &str| IssueOp::LockLfs { path: path.into(), refspec: Some(name("refs/heads/main")) };
    let l = LockId(id_of(c.commit(WRITER, lock("a.bin"))));
    assert_eq!(c.commit(OWNER, lock("a.bin")), Err(IssueErr::Exists), "one lock per path");
    assert_eq!(c.commit(OWNER, IssueOp::UnlockLfs { id: l, force: false }), Err(IssueErr::Forbidden));
    c.commit(OWNER, IssueOp::UnlockLfs { id: l, force: true }).unwrap();
    assert_eq!(c.rows(Tier::Metadata, "lfs_lock") + c.rows(Tier::Metadata, "lfs_lock_by_path"), 0);
    let l = LockId(id_of(c.commit(OWNER, lock("a.bin"))));
    assert_eq!(
        c.commit(WRITER, IssueOp::UnlockLfs { id: l, force: true }),
        Err(IssueErr::Forbidden),
        "force needs admin"
    );
    c.commit(OWNER, IssueOp::UnlockLfs { id: l, force: false }).unwrap();
    c.agree();
}

#[test]
fn release_and_blob_commands_write_only_inside_their_declared_tiers() {
    let mut c = setup(1, 90);
    let rel = graftyard_git::issues::RepoReleaseId(id_of(c.commit(WRITER, release_op(None, "refs/tags/v0", true))));
    let ops = vec![
        release_op(None, "refs/tags/v1", false),
        release_op(None, "refs/tags/v2", true),
        IssueOp::PutAsset {
            release: rel,
            name: "a".into(),
            label: None,
            content_type: "x".into(),
            blob: durable(b"a"),
        },
        IssueOp::LfsObject(durable(b"b")),
        IssueOp::LockLfs { path: "p".into(), refspec: None },
        IssueOp::DeleteRelease { id: rel },
    ];
    for op in ops {
        let cmd = IssueCmd { op, actor: who(OWNER), request: RequestId(Ulid(1)) };
        let m = &c.members[0];
        let view = RepoView::new(tiers(), &m.core, &m.meta, c.now);
        let needed: Vec<ChainId> = cmd.deps(&view).into_iter().collect();
        let d: Vec<_> = needed.iter().map(|k| c.deps.iter().find(|(x, _)| x == k).unwrap().clone()).collect();
        let (evs, _) = cmd.decide(&view, &d).unwrap();
        for e in &evs {
            assert_eq!(tier_of(e).0 & !cmd.tiers().0, 0, "{:?} writes outside {:?}", e, cmd.tiers());
        }
        drop(view);
        c.commit_all(&[cmd]);
    }
}

/// BF-34, the repository half (`TestAccGithubRepositoryCollaborator`'s writes): an outside collaborator is invited and
/// accepts, which grants in the same entry as `Accepted`; a declined invitation grants nothing; a `Collaborator`
/// invitation for a member of the org owner is refused, since that is a `Grant` on the org's chain; and `authorize`
/// takes the higher of a former collaborator's row and a later member's grant.
#[test]
fn collaborators_and_org_grants_never_overlap_and_acceptance_grants_in_one_entry() {
    use graftyard_identity::{InvitationEvent, InvitationId, OrgRole};
    let mut c = setup(2, 90);
    // The owner is an org: OWNER its admin, WRITER a member with a Write grant; READER and 103 are outsiders.
    let org = AccountId(DbId(OWNER));
    let mut authz = AuthzFold::new(org);
    authz.apply(&AuthzEvent::Opened { kind: AccountKind::Org, enterprise: None });
    authz.apply(&AuthzEvent::Member { user: UserId(org), role: Some(OrgRole::Admin) });
    authz.apply(&AuthzEvent::Member { user: UserId(AccountId(DbId(WRITER))), role: Some(OrgRole::Member) });
    authz.apply(&AuthzEvent::Grant {
        repo: REPO,
        grantee: UserId(AccountId(DbId(WRITER))),
        permission: Some(RepoPermission::Write),
    });
    for d in c.deps.iter_mut() {
        if d.0 == ChainId::Authz(org) {
            d.1 = IdentityDep::Authz(authz.clone());
        }
    }
    let invite = |user: i64, p| IssueOp::InviteCollaborator { user: AccountId(DbId(user)), permission: p };
    assert_eq!(
        c.commit(OWNER, invite(WRITER, RepoPermission::Admin)),
        Err(IssueErr::OrgMember),
        "a member's write is refused"
    );
    assert_eq!(
        c.commit(READER, invite(OWNER, RepoPermission::Read)),
        Err(IssueErr::Forbidden),
        "only an admin invites"
    );
    let IssueOut::Id(id) = c.commit(OWNER, invite(READER, RepoPermission::Maintain)).unwrap() else { panic!() };
    let id = InvitationId(id);
    // Only the invitee answers.
    let answer = |accept| IssueOp::AnswerInvitation { id, accept };
    assert_eq!(c.commit(WRITER, answer(true)), Err(IssueErr::Forbidden));
    c.commit(READER, answer(true)).unwrap();
    let last = c.log.last().unwrap();
    let EntryBody::Events(evs) = &last.body else { panic!() };
    assert!(
        matches!(
            evs[..],
            [
                RepoEvent::Invitation(InvitationEvent::Accepted { .. }),
                RepoEvent::Collaborator { permission: Some(RepoPermission::Maintain), .. }
            ]
        ),
        "{evs:?}"
    );
    assert_eq!(c.commit(READER, answer(true)), Err(IssueErr::NotFound), "answered once");
    // Declining grants nothing.
    c.deps.extend(deps_for_account(103));
    let IssueOut::Id(id2) = c.commit(OWNER, invite(103, RepoPermission::Write)).unwrap() else { panic!() };
    c.commit(103, IssueOp::AnswerInvitation { id: InvitationId(id2), accept: false }).unwrap();
    let core = &c.members[0].core;
    assert_eq!(graftyard_git::issue_ops::collaborator(core, AccountId(DbId(READER))), Some(RepoPermission::Maintain));
    assert_eq!(graftyard_git::issue_ops::collaborator(core, AccountId(DbId(103))), None, "declined");
    assert_eq!(c.rows(Tier::Core, "invitation"), 0);
    c.agree();
    // READER joins the org with a Read grant: `authorize` keeps the higher, Maintain.
    authz.apply(&AuthzEvent::Member { user: UserId(AccountId(DbId(READER))), role: Some(OrgRole::Member) });
    authz.apply(&AuthzEvent::Grant {
        repo: REPO,
        grantee: UserId(AccountId(DbId(READER))),
        permission: Some(RepoPermission::Read),
    });
    let access = graftyard_git::issue_ops::Access { repo: REPO, owner: org, core };
    let p = graftyard_identity::permission(&who(READER), &[&authz], &access);
    assert_eq!(p, Some(RepoPermission::Maintain));
}

/// The deps of one more account with its token, as `deps()` builds them.
fn deps_for_account(a: i64) -> Vec<(ChainId, IdentityDep)> {
    let id = AccountId(DbId(a));
    let mut names = NamesFold::new(NameShard(0));
    names.apply(&NamesEvent::Account { id, suspended: false, sessions_not_before: Timestamp(0) });
    let hash = [a as u8; 32];
    let mut out = Vec::new();
    for c in graftyard_identity::credential_chains(&who(a).credential_shard(), None) {
        let d = match c {
            ChainId::Names(_) => IdentityDep::Names(names.clone()),
            ChainId::Tokens(s) => {
                let mut t = TokensFold::new(s);
                t.apply(&TokensEvent::Stored {
                    hash,
                    kind: TokenKind::Pat,
                    account: id,
                    scopes: ScopeSet(1),
                    expires: None,
                });
                IdentityDep::Tokens(t)
            }
            _ => unreachable!(),
        };
        out.push((c, d));
    }
    out
}
