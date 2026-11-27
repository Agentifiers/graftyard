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
    AccountKind, Actor, AuthzEvent, AuthzFold, Credential, IdentityDep, NameEpoch, NamesEvent, NamesFold, Principal,
    RepoEntry, RepoName, RepoPermission, ScopeSet, TokenKind, TokensEvent, TokensFold, UserId, shard_of_hash,
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
    authz.apply(&AuthzEvent::RepoName {
        key: RepoName::parse("tool").unwrap().key(),
        entry: Some(RepoEntry::Live { repo: REPO, epoch: NameEpoch(1) }),
    });
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
    // The install's own `Authz(Genesis.account)`, which every App command reads for its `AgentApp` rows (AM1-WP2):
    // none here, so no App is an agent.
    let install = graftyard_identity::install_account();
    out.push((ChainId::Authz(install), IdentityDep::Authz(AuthzFold::new(install))));
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
    // A status minted with a lower id but committed later is the newer one: ids on a commit follow commit order (F51).
    c.commit(WRITER, status(9, StatusState::Failure)).unwrap();
    assert_eq!(c.required(oid(7)), vec![("jenkins".into(), Some(CheckConclusion::Failure))], "the later status");
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

/// F51: two edges whose clocks differ by a minute report the same required context. The later report comes from
/// the slower edge, so its minted id is lower; it must still win, because a status's id is its version and ids on one
/// commit follow commit order. Before the fix, the earlier, faster-clocked `success` kept the check green after a
/// re-run reported `pending`: BF-23's "a re-run blocks a merge again" failed through two edges.
#[test]
fn a_later_status_from_an_edge_whose_clock_trails_still_wins() {
    let mut c = setup(3, 90);
    c.commit(OWNER, protect("main", &["ci"])).unwrap();
    c.push("refs/heads/main", oid(7));
    let status = |minted: i64, state| {
        IssueOp::RecordStatus(StatusUpdate {
            id: StatusId(DbId(minted)),
            sha: oid(7),
            context: "ci".into(),
            state,
            target_url: None,
            description: None,
            creator: AccountId(DbId(WRITER)),
        })
    };
    let fast = 1_793_947_060_000; // edge A, a minute ahead
    let slow = 1_793_947_000_000; // edge B
    c.commit(WRITER, status(fast, StatusState::Success)).unwrap();
    assert_eq!(c.required(oid(7)), vec![("ci".into(), Some(CheckConclusion::Success))]);
    // The re-run, reported through edge B after A's report committed.
    let Ok(IssueOut::Id(rerun)) = c.commit(WRITER, status(slow, StatusState::Pending)) else { panic!() };
    assert!(rerun.0 > fast, "the later status's id is above every status the commit holds: {rerun:?}");
    assert_eq!(c.required(oid(7)), vec![("ci".into(), None)], "the re-run blocks the merge again");
    // Its result, through edge A again: still later in commit order, so it wins.
    c.commit(WRITER, status(fast + 1, StatusState::Failure)).unwrap();
    assert_eq!(c.required(oid(7)), vec![("ci".into(), Some(CheckConclusion::Failure))]);
    assert_eq!(c.rows(Tier::Metadata, "status"), 3);
    c.agree();
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

/// A pattern of many stars against a long branch it misses: backtracking tried about n^k splits.
#[test]
fn a_many_star_pattern_against_a_long_branch_finishes() {
    let branch = name(&format!("refs/heads/{}", "a".repeat(880)));
    assert!(!branch_matches(&format!("{}b", "*a".repeat(12)), &branch));
    assert!(!branch_matches(&format!("{}b", "**a".repeat(12)), &branch));
    assert!(branch_matches(&"*a".repeat(12), &branch));
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

/// A review comment outside a review is a single-comment review's, as GHES makes one: the comment names a COMMENTED
/// review with no body, submitted in the same entry, whose id follows the thread's in Core's block, and the command
/// still answers the comment's id. Two threads in one entry take four distinct ids. A reply gets its own review too.
/// Before format 32.22 (`IssueCmd::decide`), a commenter whose verdict approves keeps it, so that comment names no
/// review; 32.22's `SingleComment` gives it one (`agent_gates.rs`, `an_approvers_review_comment_gets_its_review_...`).
#[test]
fn a_review_comment_outside_a_review_is_a_single_comment_reviews() {
    let mut c = setup(1, 90);
    c.push("refs/heads/f", oid(2));
    let n = open_pr(&mut c, "refs/heads/f", oid(2));
    let review_of = |c: &Chain, id: DbId| {
        let m = &c.members[0];
        let Some(Val::Blob(b)) = m.meta.get("comment", &[Val::Int(id.0)]).and_then(|v| v.into_iter().next()) else {
            panic!("no comment {id:?}")
        };
        let row: graftyard_git::tiers::CommentRow = postcard::from_bytes(&b).unwrap();
        let CommentTarget::Review { review, .. } = row.on else { panic!() };
        review.map(|r| r.0)
    };
    let reviews = |c: &Chain| {
        graftyard_git::merge::reviews_of(&c.members[0].core, n).into_iter().map(|r| r.state).collect::<Vec<_>>()
    };
    let thread = review_comment(&mut c, n, None);
    assert_eq!(review_of(&c, thread), Some(DbId(thread.0 + 1)), "Core's next id, after the thread's");
    assert_eq!(c.log.last().unwrap().tiers, TierSet::CORE.union(TierSet::METADATA));
    assert_eq!(reviews(&c), [ReviewState::Commented]);
    assert_eq!(c.rows(Tier::Metadata, "review"), 1);
    let reply = review_comment(&mut c, n, Some(graftyard_git::issues::CommentId(thread)));
    let r = review_of(&c, reply).expect("a reply's own review");
    assert!(r != DbId(thread.0 + 1) && r != reply && r != thread);
    // Two new threads in one entry.
    let at = graftyard_git::issues::LineAnchor {
        commit: oid(2),
        path: "b.rs".into(),
        line: 2,
        start_line: None,
        side: graftyard_git::issues::DiffSide::Right,
    };
    let cmd = |i: u128| IssueCmd {
        op: IssueOp::Comment {
            on: CommentTarget::Review { n, review: None, at: at.clone() },
            body: "nit".into(),
            reply_to: None,
        },
        actor: who(READER),
        request: RequestId(Ulid(1_000 + i)),
    };
    let outs = c.commit_all(&[cmd(0), cmd(1)]);
    let [Ok(IssueOut::Id(a)), Ok(IssueOut::Id(b))] = outs.as_slice() else { panic!("{outs:?}") };
    let mut all = vec![*a, *b, review_of(&c, *a).unwrap(), review_of(&c, *b).unwrap()];
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 4, "{all:?}");
    // An approver's comment leaves the approval as it is.
    c.commit(READER, IssueOp::Review { n, state: ReviewState::Approved, commit: oid(2), body: "".into() }).unwrap();
    let before = c.rows(Tier::Metadata, "review");
    let after_approval = review_comment(&mut c, n, None);
    assert_eq!(review_of(&c, after_approval), None);
    assert_eq!(reviews(&c), [ReviewState::Approved]);
    assert_eq!(c.rows(Tier::Metadata, "review"), before);
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
        IssueOut::Number(_)
        | IssueOut::Done
        | IssueOut::Task(_)
        | IssueOut::TaskFinal(..)
        | IssueOut::Lease(_)
        | IssueOut::Created => {}
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
    // A withdrawn invitation (32.11) can no longer be accepted, and only an admin withdraws one.
    let IssueOut::Id(id3) = c.commit(OWNER, invite(103, RepoPermission::Triage)).unwrap() else { panic!() };
    let cancel = IssueOp::CancelInvitation { id: InvitationId(id3) };
    assert_eq!(c.commit(103, cancel.clone()), Err(IssueErr::Forbidden), "only an admin withdraws");
    c.commit(OWNER, cancel.clone()).unwrap();
    assert_eq!(c.commit(OWNER, cancel), Err(IssueErr::NotFound), "withdrawn once");
    let late = IssueOp::AnswerInvitation { id: InvitationId(id3), accept: true };
    assert_eq!(c.commit(103, late), Err(IssueErr::NotFound), "a withdrawn invitation grants nothing");
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

// ---- M3 close: issue commands under a stateless installation token ----

const APP: i64 = 900;
const INSTALLATION: i64 = 901;

fn claims(perms: &[(&str, graftyard_identity::Access)]) -> graftyard_identity::stateless::InstallationClaims {
    use graftyard_identity::{AppId, InstallationId, Jti, Kid, PermissionSet, RepoScope};
    graftyard_identity::stateless::InstallationClaims {
        app: AppId(DbId(APP)),
        installation: InstallationId(DbId(INSTALLATION)),
        repos: RepoScope::Selected(smallvec![REPO]),
        permissions: PermissionSet(perms.iter().map(|(k, a)| (Box::from(*k), *a)).collect()),
        jti: Jti(Ulid(0x0193_0000_0000_0000_0000_0000_0000_0042)),
        iat: Timestamp(DAY),
        exp: Timestamp(DAY + 3_600_000),
        kid: Kid(1),
        task: None,
    }
}

/// An issue command under a `ghs_` token, as the edge sends it and a member decodes it: through `IssueCall`'s JSON.
fn app_cmd(c: &graftyard_identity::stateless::InstallationClaims, op: IssueOp) -> IssueCmd {
    use graftyard_git::issue_ops::call::{IssueCall, REPO_FORMAT_APP_CALLS};
    let blobless = op.map_blob(|_| Err::<graftyard_git::blob::BlobRef, ()>(())).expect("no blob");
    let call = IssueCall::new(blobless, Principal::installation(c), RequestId(Ulid(77)), serde_json::json!({}));
    assert_eq!(call.format(), REPO_FORMAT_APP_CALLS);
    let wire = serde_json::to_value(&call).unwrap();
    assert_eq!(wire["format"], serde_json::json!([0x20, 9]));
    let back: IssueCall = serde_json::from_value(wire).unwrap();
    assert_eq!(back.actor, Principal::installation(c), "the claims survive the binding");
    IssueCmd { op: back.op.map_blob(|_| Err::<_, ()>(())).expect("no blob"), actor: back.actor, request: back.request }
}

/// The App's installation on the owner, with `perms`, and the token's `jti` shard read as the leader reads it.
fn install(c: &mut Chain, perms: &[(&str, graftyard_identity::Access)]) {
    use graftyard_identity::{AppId, InstallationId, InstallationRow, PermissionSet, RepoScope};
    let claims = claims(perms);
    for d in c.deps.iter_mut() {
        if let (ChainId::Authz(_), IdentityDep::Authz(a)) = d {
            a.apply(&AuthzEvent::Installation {
                id: InstallationId(DbId(INSTALLATION)),
                row: Some(InstallationRow {
                    app: AppId(DbId(APP)),
                    repos: RepoScope::Selected(smallvec![REPO]),
                    permissions: PermissionSet(perms.iter().map(|(k, a)| (Box::from(*k), *a)).collect()),
                }),
                not_before: Timestamp(0),
            });
        }
    }
    for ch in graftyard_identity::credential_chains(&Principal::installation(&claims).credential_shard(), None) {
        let ChainId::Tokens(s) = ch else { unreachable!() };
        c.deps.retain(|(k, _)| *k != ch);
        c.deps.push((ch, IdentityDep::Tokens(TokensFold::new(s))));
    }
}

/// M3 close, item 1: an installation token's issue write is authorized by its App permissions against the
/// installation's row, not refused for want of a stored token. Old code sent such a command with an all-zero hash
/// and no claims, so every one was refused `Unauthorized(NoToken)` and the edge answered 403 whatever the App held.
#[test]
fn an_installation_tokens_issue_writes_follow_its_app_permissions() {
    use graftyard_identity::Access::{Read, Write};
    let mut c = setup(3, 90);
    c.commit(OWNER, IssueOp::Settings(settings(90))).unwrap();
    install(&mut c, &[("issues", Write), ("metadata", Read)]);
    let label = |n: &str| IssueOp::DefineLabel {
        id: None,
        name: graftyard_git::issues::LabelName(n.into()),
        color: [1, 2, 3],
        description: None,
    };
    let open = || IssueOp::OpenIssue {
        title: "t".into(),
        body: "".into(),
        labels: SmallVec::new(),
        assignees: SmallVec::new(),
    };
    let run = |c: &mut Chain, cmd: IssueCmd| c.commit_all(&[cmd]).pop().unwrap();

    // With `issues: write`: a label and an issue, attributed to no user.
    let w = claims(&[("issues", Write), ("metadata", Read)]);
    assert!(matches!(run(&mut c, app_cmd(&w, label("bot"))), Ok(IssueOut::Id(_))));
    let Ok(IssueOut::Number(n)) = run(&mut c, app_cmd(&w, open())) else { panic!("an App opens an issue") };
    assert!(matches!(run(&mut c, app_cmd(&w, IssueOp::Close { n })), Ok(IssueOut::Done)));

    // Claims narrowed to `issues: read`: 403 on a repository the token reaches, as GitHub's "Resource not
    // accessible by integration".
    let r = claims(&[("issues", Read), ("metadata", Read)]);
    assert_eq!(run(&mut c, app_cmd(&r, label("ro"))), Err(IssueErr::Forbidden));
    // Claims wider than the installation's row are cut to the row: `administration` was never installed.
    let wide = claims(&[("issues", Write), ("administration", Write)]);
    assert_eq!(run(&mut c, app_cmd(&wide, IssueOp::Settings(settings(30)))), Err(IssueErr::Forbidden));
    // A token whose claims select another repository: the private repository is hidden, 404.
    let other = graftyard_identity::stateless::InstallationClaims {
        repos: graftyard_identity::RepoScope::Selected(smallvec![RepoId(DbId(8))]),
        ..w.clone()
    };
    assert_eq!(run(&mut c, app_cmd(&other, label("x"))), Err(IssueErr::NotFound));
    // A revoked `jti` is 401 on the next write.
    let shard = graftyard_identity::jti_shard(&w.jti);
    for d in c.deps.iter_mut() {
        if let (ChainId::Tokens(s), IdentityDep::Tokens(t)) = d
            && *s == shard
        {
            t.apply(&TokensEvent::JtiRevoked { jti: w.jti, exp: w.exp });
        }
    }
    assert!(matches!(run(&mut c, app_cmd(&w, label("late"))), Err(IssueErr::Unauthorized(_))));
    c.agree();
}

/// BF-77's token half (`decisions/DECISIONS.md`, "`app-bot`"): an installation token sets a commit status with
/// `statuses: write` (a `GITHUB_TOKEN`'s), as its App's bot account, in format 32.29. Without the permission it is 403,
/// and naming another creator (a user, or another App's bot) is refused. On main every App's status was 403: the
/// `statuses` permission was never asked and an App has no account to be the creator.
#[test]
fn an_installation_token_sets_a_status_as_its_apps_bot_with_statuses_write() {
    use graftyard_git::issue_ops::call::{IssueCall, REPO_FORMAT_ROUTES};
    use graftyard_identity::Access::{Read, Write};
    let mut c = setup(3, 90);
    c.commit(OWNER, protect("main", &["ci"])).unwrap();
    c.push("refs/heads/main", oid(7));
    install(&mut c, &[("statuses", Write), ("contents", Write), ("metadata", Read)]);
    let app = graftyard_identity::AppId(DbId(APP));
    let status = |creator: AccountId, state| {
        IssueOp::RecordStatus(StatusUpdate {
            id: StatusId(DbId(1_000)),
            sha: oid(7),
            context: "ci".into(),
            state,
            target_url: None,
            description: None,
            creator,
        })
    };
    // Through the binding: an App's status is 32.29 with its claims, and a user's keeps 32.5.
    let cmd = |claims: &graftyard_identity::stateless::InstallationClaims, op: IssueOp| {
        let blobless = op.map_blob(|_| Err::<graftyard_git::blob::BlobRef, ()>(())).expect("no blob");
        let call =
            IssueCall::new(blobless, Principal::installation(claims), RequestId(Ulid(78)), serde_json::json!({}));
        assert_eq!(call.format(), REPO_FORMAT_ROUTES);
        let back: IssueCall = serde_json::from_value(serde_json::to_value(&call).unwrap()).unwrap();
        assert_eq!(back.actor, Principal::installation(claims));
        IssueCmd {
            op: back.op.map_blob(|_| Err::<_, ()>(())).expect("no blob"),
            actor: back.actor,
            request: back.request,
        }
    };
    let user = IssueCall::new(
        status(AccountId(DbId(WRITER)), StatusState::Success).map_blob(|_| Err::<_, ()>(())).expect("no blob"),
        who(WRITER),
        RequestId(Ulid(79)),
        serde_json::json!({}),
    );
    assert_eq!(user.format(), graftyard_git::issue_ops::call::REPO_FORMAT_ISSUES, "a user's status keeps its bytes");
    let run = |c: &mut Chain, cmd: IssueCmd| c.commit_all(&[cmd]).pop().unwrap();

    let rw = claims(&[("statuses", Write), ("contents", Write), ("metadata", Read)]);
    assert!(matches!(run(&mut c, cmd(&rw, status(app.bot(), StatusState::Success))), Ok(IssueOut::Id(_))));
    assert_eq!(
        c.required(oid(7)),
        vec![("ci".into(), Some(CheckConclusion::Success))],
        "it counts as a required check"
    );
    // The bot's id is the App's negated: no account `Ids` assigns.
    assert!(app.bot().0.0 < 0 && graftyard_identity::AppId::of_bot(app.bot()) == Some(app));
    // A creator other than its own bot: a user, or another App's bot.
    let other = graftyard_identity::AppId(DbId(APP + 1)).bot();
    for creator in [AccountId(DbId(WRITER)), other] {
        let r = run(&mut c, cmd(&rw, status(creator, StatusState::Failure)));
        assert!(matches!(r, Err(IssueErr::Invalid(_))), "{creator:?}: {r:?}");
    }
    // Without `statuses: write`, as BF-77's `default` and `ro` jobs: 403, GitHub's "Resource not accessible by
    // integration".
    for perms in [&[("contents", Read), ("metadata", Read)][..], &[("statuses", Read), ("contents", Write)][..]] {
        let r = run(&mut c, cmd(&claims(perms), status(app.bot(), StatusState::Failure)));
        assert_eq!(r, Err(IssueErr::Forbidden), "{perms:?}");
    }
    assert_eq!(c.required(oid(7)), vec![("ci".into(), Some(CheckConclusion::Success))], "no refused status landed");

    // A REST issue create by the App (32.29, `decide_in`'s `bot_author`) is authored by its bot, as GHES shows
    // `github-actions[bot]` as `user`; the same `OpenIssue` from GraphQL `createIssue` (32.9) keeps no author.
    install(&mut c, &[("issues", Write), ("statuses", Write), ("contents", Write), ("metadata", Read)]);
    let w = claims(&[("issues", Write), ("metadata", Read)]);
    let open =
        IssueOp::OpenIssue { title: "t".into(), body: "".into(), labels: SmallVec::new(), assignees: SmallVec::new() };
    let cmd = app_cmd(&w, open);
    let m = &c.members[0];
    let view = RepoView::new(tiers(), &m.core, &m.meta, c.now);
    let mut needed: Vec<ChainId> = cmd.deps(&view).into_iter().collect();
    needed.dedup();
    let d: Vec<_> = needed.iter().map(|ch| c.deps.iter().find(|(k, _)| k == ch).expect("a dep").clone()).collect();
    let author = |bot: bool| match cmd.decide_in(&view, &d, false, None, false, false, bot, None).unwrap().0[0] {
        RepoEvent::IssueOpened { ref capture, .. } => capture.author,
        ref e => panic!("{e:?}"),
    };
    assert_eq!(author(true), Some(app.bot()), "REST: the App's bot");
    assert_eq!(author(false), None, "GraphQL: no author, as before");
    drop(view);
    c.agree();
}

/// F137: a REST issue comment rides format 32.32 through the binding, a user's and an App's alike, whose
/// `What::IssueCommentCreated` an older member cannot render; the same `Comment` from GraphQL `addComment` keeps its
/// format. The mark is only for an issue comment: 32.32 on a review comment is malformed. An App's REST comment is its
/// bot's, as its REST issue is.
#[test]
fn a_rest_issue_comment_rides_32_32_and_an_apps_is_its_bots() {
    use graftyard_git::issue_ops::call::{
        IssueCall, REPO_FORMAT_APP_CALLS, REPO_FORMAT_ISSUE_COMMENT, REPO_FORMAT_ISSUES,
    };
    use graftyard_identity::Access::{Read, Write};
    let mut c = setup(3, 90);
    install(&mut c, &[("issues", Write), ("metadata", Read)]);
    let w = claims(&[("issues", Write), ("metadata", Read)]);
    let open =
        IssueOp::OpenIssue { title: "t".into(), body: "".into(), labels: SmallVec::new(), assignees: SmallVec::new() };
    c.commit(WRITER, open).unwrap();
    let comment = || IssueOp::Comment { on: CommentTarget::Issue(IssueNumber(1)), body: "b".into(), reply_to: None };
    let blobless = |op: IssueOp| op.map_blob(|_| Err::<graftyard_git::blob::BlobRef, ()>(())).expect("no blob");
    for (actor, before) in [(who(WRITER), REPO_FORMAT_ISSUES), (Principal::installation(&w), REPO_FORMAT_APP_CALLS)] {
        let mut call = IssueCall::new(blobless(comment()), actor.clone(), RequestId(Ulid(80)), serde_json::json!({}));
        assert_eq!(call.format(), before, "addComment keeps its bytes");
        call.rest_comment = true;
        assert_eq!(call.format(), REPO_FORMAT_ISSUE_COMMENT);
        let wire = serde_json::to_value(&call).unwrap();
        assert_eq!(wire["format"], serde_json::json!([0x20, 32]));
        let back: IssueCall = serde_json::from_value(wire).unwrap();
        assert!(back.rest_comment && back.actor == actor, "{back:?}");
    }
    let mut review = IssueCall::new(
        blobless(IssueOp::Comment {
            on: CommentTarget::Review {
                n: IssueNumber(1),
                review: None,
                at: graftyard_git::issues::LineAnchor {
                    commit: oid(1),
                    path: "a".into(),
                    line: 1,
                    start_line: None,
                    side: graftyard_git::issues::DiffSide::Right,
                },
            },
            body: "b".into(),
            reply_to: None,
        }),
        who(WRITER),
        RequestId(Ulid(81)),
        serde_json::json!({}),
    );
    let mut wire = serde_json::to_value(&review).unwrap();
    wire["format"] = serde_json::json!([0x20, 32]);
    assert!(serde_json::from_value::<IssueCall>(wire).is_err(), "32.32 marks only an issue comment");
    review.rest_comment = IssueCall::issue_comment(&review.op);
    assert_eq!(review.format(), REPO_FORMAT_ISSUES);

    let cmd = app_cmd(&w, comment());
    let m = &c.members[0];
    let view = RepoView::new(tiers(), &m.core, &m.meta, c.now);
    let mut needed: Vec<ChainId> = cmd.deps(&view).into_iter().collect();
    needed.dedup();
    let d: Vec<_> = needed.iter().map(|ch| c.deps.iter().find(|(k, _)| k == ch).expect("a dep").clone()).collect();
    let author = |bot: bool| match &cmd.decide_in(&view, &d, false, None, false, false, bot, None).unwrap().0[0] {
        RepoEvent::Comment(graftyard_git::issues::CommentEvent::Created { author, .. }) => *author,
        e => panic!("{e:?}"),
    };
    let app = graftyard_identity::AppId(DbId(APP));
    assert_eq!((author(true), author(false)), (Some(app.bot()), None), "REST: the App's bot; GraphQL: none");
}

/// `rest:repos`' create (format 32.13): `InitRepo` writes the chain's first settings and `Created` once, needs Admin,
/// names the chain's own owner, and a re-sent create commits nothing.
#[test]
fn init_repo_writes_the_first_settings_once_and_needs_an_admin() {
    let mut c = Chain::new(3);
    assert_eq!(c.commit(WRITER, IssueOp::InitRepo(settings(90))), Err(IssueErr::Forbidden));
    let mut other = settings(90);
    other.owner = AccountId(DbId(5));
    assert_eq!(c.commit(OWNER, IssueOp::InitRepo(other)), Err(IssueErr::OwnerChange));
    let n = c.log.len();
    assert_eq!(c.commit(OWNER, IssueOp::InitRepo(settings(90))), Ok(IssueOut::Done));
    assert_eq!(c.log.len(), n + 1);
    let created = graftyard_git::tiers::created(&c.members[0].core);
    assert_eq!(created, Some(c.now), "created_at is the entry's time");
    // A second create (the edge re-sent it, or a later client) commits nothing and keeps the first settings.
    c.now = Timestamp(2 * DAY);
    let mut later = settings(30);
    later.description = Some("later".into());
    assert_eq!(c.commit(OWNER, IssueOp::InitRepo(later)), Ok(IssueOut::Done));
    assert_eq!(c.log.len(), n + 1, "no entry");
    assert_eq!(graftyard_git::tiers::created(&c.members[0].core), created);
    // The tier keeps the first `Created`'s time even if a later entry carries another (`decide` never sends one).
    c.entry(EntryBody::Events(vec![RepoEvent::Created { actor: None }]), TierSet::CORE);
    assert_eq!(graftyard_git::tiers::created(&c.members[0].core), created, "created_at does not move");
    c.agree();
}

/// `rest:repos`' delete: `DeleteRepo` is the chain's `Deleted` tombstone, kept until `retain_until`. Only an admin
/// sends it, the chain then refuses every client write, and a repeat commits nothing.
#[test]
fn delete_repo_tombstones_the_chain_and_refuses_every_later_write() {
    let mut c = setup(3, 90);
    let keep = Timestamp(c.now.0 + 90 * DAY);
    assert_eq!(c.commit(WRITER, IssueOp::DeleteRepo { retain_until: keep }), Err(IssueErr::Forbidden));
    assert!(matches!(c.commit(OWNER, IssueOp::DeleteRepo { retain_until: c.now }), Err(IssueErr::Invalid(_))));
    assert_eq!(c.commit(OWNER, IssueOp::DeleteRepo { retain_until: keep }), Ok(IssueOut::Done));
    assert_eq!(
        graftyard_git::tiers::deleted(&c.members[0].core),
        Some((keep, Some(AccountId(DbId(OWNER))))),
        "the tombstone keeps the repository until retain_until"
    );
    let n = c.log.len();
    assert_eq!(c.commit(OWNER, IssueOp::DeleteRepo { retain_until: keep }), Ok(IssueOut::Done));
    assert_eq!(c.log.len(), n, "a repeated delete commits nothing");
    let open =
        IssueOp::OpenIssue { title: "t".into(), body: "".into(), labels: SmallVec::new(), assignees: SmallVec::new() };
    assert_eq!(c.commit(OWNER, open), Err(IssueErr::NotFound));
    assert_eq!(c.commit(OWNER, IssueOp::Settings(settings(30))), Err(IssueErr::NotFound));
    assert_eq!(c.commit(OWNER, IssueOp::InitRepo(settings(30))), Err(IssueErr::NotFound));
    // The refs stay, so GC keeps the objects until `retain_until`.
    assert_eq!(c.ref_of(&name("refs/heads/main")), Some(oid(1)));
    c.agree();
}

/// M4-WP3's secret and variable commands (BF-59): admin only; a repository's and an environment's rows are kept
/// apart, so a job of one environment never reads another's; the stored row is the sealed bytes as sent, so no entry
/// holds a plaintext; a variable's create refuses an existing name (GHES's 409, which `gh variable set` turns into its
/// PATCH) and its update a missing one; an org scope is refused on a repository chain.
#[test]
fn secret_and_variable_commands_keep_scopes_apart_and_store_only_the_sealed_bytes() {
    use graftyard_git::secrets::{all_secrets, secret, secrets, variable, variables};
    use graftyard_identity::sealed::{ActionsName, EnvName, SecretScope, SecretVisibility};
    let n = |s: &str| ActionsName::parse(s).unwrap();
    let prod = SecretScope::Env(EnvName("production".into()));
    let mut sealed = vec![9u8; 48];
    sealed.extend_from_slice(b"ciphertext");
    let put = |scope: &SecretScope, name: &str, sealed: &[u8]| IssueOp::PutActionsSecret {
        scope: scope.clone(),
        name: n(name),
        sealed: sealed.into(),
        key_id: "568250167242549743".into(),
    };
    let mut c = Chain::new(3);
    assert_eq!(c.commit(WRITER, put(&SecretScope::Repo, "TOKEN", &sealed)), Err(IssueErr::Forbidden));
    assert_eq!(c.commit(OWNER, put(&SecretScope::Repo, "token", &sealed)), Ok(IssueOut::Created));
    assert_eq!(c.commit(OWNER, put(&SecretScope::Repo, "TOKEN", &sealed)), Ok(IssueOut::Done), "an update is 204");
    assert_eq!(
        c.commit(OWNER, put(&prod, "TOKEN", b"short")),
        Err(IssueErr::Invalid("a secret is a sealed box of at most 48 KB under the owner's key"))
    );
    let mut other = sealed.clone();
    other[0] = 1;
    assert_eq!(c.commit(OWNER, put(&prod, "TOKEN", &other)), Ok(IssueOut::Created));
    let org = SecretScope::Org { visibility: SecretVisibility::All, repos: SmallVec::new() };
    assert!(matches!(c.commit(OWNER, put(&org, "TOKEN", &sealed)), Err(IssueErr::Invalid(_))));
    let core = &c.members[2].core;
    assert_eq!(secret(core, &SecretScope::Repo, &n("TOKEN")).unwrap().sealed.as_ref(), sealed.as_slice());
    assert_eq!(secret(core, &prod, &n("TOKEN")).unwrap().sealed.as_ref(), other.as_slice());
    assert_eq!(secret(core, &SecretScope::Env(EnvName("staging".into())), &n("TOKEN")), None);
    assert_eq!((secrets(core, &SecretScope::Repo).len(), all_secrets(core).len()), (1, 2));
    assert!(!format!("{:?}", secret(core, &SecretScope::Repo, &n("TOKEN"))).contains("ciphertext"));
    let var = |scope: &SecretScope, value: &str, create: bool| IssueOp::PutActionsVariable {
        scope: scope.clone(),
        name: n("REGION"),
        value: value.into(),
        create,
    };
    assert_eq!(c.commit(OWNER, var(&SecretScope::Repo, "enam", false)), Err(IssueErr::NotFound));
    assert_eq!(c.commit(OWNER, var(&SecretScope::Repo, "enam", true)), Ok(IssueOut::Created));
    assert_eq!(c.commit(OWNER, var(&SecretScope::Repo, "weur", true)), Err(IssueErr::Exists));
    assert_eq!(c.commit(OWNER, var(&SecretScope::Repo, "weur", false)), Ok(IssueOut::Done));
    assert_eq!(c.commit(OWNER, var(&prod, "apac", true)), Ok(IssueOut::Created));
    assert_eq!(variable(&c.members[1].core, &SecretScope::Repo, &n("REGION")).unwrap().value.as_ref(), "weur");
    assert_eq!(variables(&c.members[1].core, &prod)[0].1.value.as_ref(), "apac");
    let del = IssueOp::DeleteActionsSecret { scope: SecretScope::Repo, name: n("TOKEN") };
    assert_eq!(c.commit(READER, del.clone()), Err(IssueErr::Forbidden));
    assert_eq!(c.commit(OWNER, del.clone()), Ok(IssueOut::Done));
    assert_eq!(c.commit(OWNER, del), Err(IssueErr::NotFound));
    assert_eq!(secret(&c.members[0].core, &prod, &n("TOKEN")).map(|r| r.sealed.len()), Some(other.len()));
    c.agree();
}

/// M4-WP1's hook commands: admin only; a create takes the next core id and is flagged `created` (so the consumer
/// pings it) while an edit is not; an identical edit commits nothing; a second hook on the same URL is refused; at
/// most 20 per repository; a delete removes the row. Every member reaches the same digests.
#[test]
fn hook_commands_are_admin_only_and_a_create_alone_is_pinged() {
    use graftyard_git::hooks::{HOOKS_PER_REPO_MAX, HookContentType, HookId, HookRecord, hook, hooks, webhooks};
    let rec = |url: &str| HookRecord {
        url: graftyard_identity::Url(url.into()),
        events: smallvec![graftyard_identity::EventKind("*".into())],
        content_type: HookContentType::Json,
        active: true,
        insecure_ssl: false,
        secret: None,
        retry: false,
    };
    let mut c = Chain::new(3);
    let put = |id, r: HookRecord| IssueOp::PutHook { id, record: r };
    assert_eq!(c.commit(WRITER, put(None, rec("https://a.example/"))), Err(IssueErr::Forbidden));
    let id = match c.commit(OWNER, put(None, rec("https://a.example/"))) {
        Ok(IssueOut::Id(i)) => HookId(i),
        o => panic!("{o:?}"),
    };
    let pings = |c: &Chain| {
        c.log.iter().flat_map(|e| webhooks(&c.members[0].core, REPO, e)).filter(|w| w.event == "ping").count()
    };
    assert_eq!(pings(&c), 1, "a create pings its hook");
    assert_eq!(c.commit(OWNER, put(None, rec("https://a.example/"))), Err(IssueErr::Exists));
    let edited = HookRecord { active: false, ..rec("https://a.example/") };
    let len = c.log.len();
    assert_eq!(c.commit(OWNER, put(Some(id), edited.clone())), Ok(IssueOut::Id(id.0)));
    assert_eq!(c.log.len(), len + 1);
    assert_eq!(c.commit(OWNER, put(Some(id), edited.clone())), Ok(IssueOut::Id(id.0)));
    assert_eq!(c.log.len(), len + 1, "an identical edit commits nothing");
    assert_eq!(pings(&c), 1, "an edit does not ping");
    assert_eq!(hook(&c.members[2].core, id), Some(edited));
    assert_eq!(c.commit(READER, IssueOp::DeleteHook { id }), Err(IssueErr::Forbidden));
    for i in 1..HOOKS_PER_REPO_MAX {
        c.commit(OWNER, put(None, rec(&format!("https://h{i}.example/")))).unwrap();
    }
    let full = c.commit(OWNER, put(None, rec("https://full.example/")));
    assert!(matches!(full, Err(IssueErr::Invalid(_))), "{full:?} with {} hooks", hooks(&c.members[0].core).len());
    assert_eq!(c.commit(OWNER, IssueOp::DeleteHook { id }), Ok(IssueOut::Done));
    assert_eq!(hook(&c.members[1].core, id), None);
    assert_eq!(hooks(&c.members[1].core).len(), HOOKS_PER_REPO_MAX - 1);
    assert_eq!(c.commit(OWNER, IssueOp::DeleteHook { id }), Err(IssueErr::NotFound));
    c.agree();
}

/// F137's multi-field REST `PATCH` (format 32.35): one `Edit` writes every part in one entry or nothing. A title,
/// body, state and label set commit together; with one bad part (a title over the limit) nothing is written, though
/// each other part alone would commit. Each part keeps its own command's rule: a reader may edit their own issue's
/// title and close it, but their label set is dropped, as GHES drops it. A reasonless lock is `Unspecified` and
/// reads locked; its unlock clears it. An `Edit` crosses the binding only as 32.35, and the comment edit and lock
/// marks as 32.35.
#[test]
fn a_multi_field_issue_edit_commits_whole_or_not_at_all() {
    use graftyard_git::issue_ops::call::{IssueCall, REPO_FORMAT_ISSUE_EDIT, REPO_FORMAT_ISSUES};
    use graftyard_git::issues::{IssueChange, LabelId, LabelName, LockReason, StateReason};
    use graftyard_git::tiers::{IssueRow, get_rec, n_key};
    let mut c = setup(1, 90);
    let label = |c: &mut Chain, name: &str| match c
        .commit(
            WRITER,
            IssueOp::DefineLabel { id: None, name: LabelName(name.into()), color: [1, 2, 3], description: None },
        )
        .unwrap()
    {
        IssueOut::Id(id) => LabelId(id),
        o => panic!("{o:?}"),
    };
    let (bug, ui) = (label(&mut c, "bug"), label(&mut c, "ui"));
    let open = |c: &mut Chain, who: i64| match c
        .commit(
            who,
            IssueOp::OpenIssue {
                title: "t".into(),
                body: "b".into(),
                labels: smallvec![bug],
                assignees: SmallVec::new(),
            },
        )
        .unwrap()
    {
        IssueOut::Number(n) => n,
        o => panic!("{o:?}"),
    };
    let row = |c: &Chain, n: IssueNumber| get_rec::<IssueRow>(&c.members[0].meta, "issue", &[n_key(n)]).unwrap();
    let labels = |c: &Chain, n: IssueNumber| -> Vec<i64> {
        c.members[0]
            .meta
            .prefix("issue_label", &[n_key(n)])
            .into_iter()
            .map(|(k, _)| match k[1] {
                Val::Int(i) => i,
                _ => panic!(),
            })
            .collect()
    };
    let edit = |n, changes: SmallVec<[IssueChange; 3]>, labels: Option<SmallVec<[LabelId; 4]>>| IssueOp::Edit {
        n,
        changes,
        labels,
        assignees: None,
    };
    let n = open(&mut c, WRITER);
    let before = c.log.len();
    let all = smallvec![
        IssueChange::Title("new".into()),
        IssueChange::Body("nb".into()),
        IssueChange::State { open: false, reason: Some(StateReason::NotPlanned) }
    ];
    assert_eq!(c.commit(WRITER, edit(n, all, Some(smallvec![ui]))), Ok(IssueOut::Done));
    assert_eq!(c.log.len(), before + 1, "one entry");
    let r = row(&c, n);
    assert_eq!((&*r.title, &*r.body, r.open, r.reason), ("new", "nb", false, Some(StateReason::NotPlanned)));
    assert_eq!(labels(&c, n), [ui.0.0], "the set replaced");
    // One part refused refuses the whole edit, and writes nothing.
    let long = "x".repeat(300);
    let bad = smallvec![IssueChange::Body("never".into()), IssueChange::Title(long.into())];
    let before = c.log.len();
    assert!(matches!(c.commit(WRITER, edit(n, bad, Some(smallvec![bug]))), Err(IssueErr::Invalid(_))));
    assert_eq!(c.log.len(), before, "no entry");
    assert_eq!((&*row(&c, n).body, labels(&c, n)), ("nb", vec![ui.0.0]));
    assert_eq!(
        c.commit(WRITER, IssueOp::Change { n, change: IssueChange::Body("alone".into()) }),
        Ok(IssueOut::Done),
        "the body alone would commit"
    );
    // A reader's own issue: title and close commit, the label set is dropped.
    let mine = open(&mut c, READER);
    let parts = smallvec![IssueChange::Title("mine".into()), IssueChange::State { open: false, reason: None }];
    assert_eq!(c.commit(READER, edit(mine, parts, Some(SmallVec::new()))), Ok(IssueOut::Done));
    assert_eq!((&*row(&c, mine).title, row(&c, mine).open, labels(&c, mine)), ("mine", false, vec![bug.0.0]));
    // Another's issue: the reader's edit is refused whole.
    let theirs = smallvec![IssueChange::Title("x".into()), IssueChange::Body("y".into())];
    assert_eq!(c.commit(READER, edit(n, theirs, None)), Err(IssueErr::Forbidden));
    // A reasonless lock, and its unlock.
    let lock = |r| IssueOp::Change { n, change: IssueChange::Locked(r) };
    assert_eq!(c.commit(READER, lock(Some(LockReason::Unspecified))), Err(IssueErr::Forbidden));
    c.commit(WRITER, lock(Some(LockReason::Unspecified))).unwrap();
    assert_eq!(row(&c, n).locked, Some(LockReason::Unspecified));
    c.commit(WRITER, lock(None)).unwrap();
    assert_eq!(row(&c, n).locked, None);

    // The wire: an `Edit` is 32.35 under any principal; a comment edit and a lock are 32.35 only when marked, and the
    // mark on any other command is malformed.
    let blobless = |op: IssueOp| op.map_blob(|_| Err::<graftyard_git::blob::BlobRef, ()>(())).expect("no blob");
    let call = |op| IssueCall::new(blobless(op), who(WRITER), RequestId(Ulid(90)), serde_json::json!({}));
    let e = call(edit(n, smallvec![IssueChange::Title("w".into())], None));
    assert_eq!(e.format(), REPO_FORMAT_ISSUE_EDIT);
    let back: IssueCall = serde_json::from_value(serde_json::to_value(&e).unwrap()).unwrap();
    assert_eq!(back.op, e.op);
    for op in [
        lock(Some(LockReason::Unspecified)),
        IssueOp::EditComment { id: graftyard_git::issues::CommentId(DbId(1)), body: "b".into() },
    ] {
        let mut m = call(op);
        assert_eq!(m.format(), REPO_FORMAT_ISSUES, "unmarked, it keeps its bytes");
        m.rest_edit = IssueCall::edit_mark(&m.op);
        assert_eq!(m.format(), REPO_FORMAT_ISSUE_EDIT);
        let wire = serde_json::to_value(&m).unwrap();
        assert_eq!(wire["format"], serde_json::json!([0x20, 35]));
        let back: IssueCall = serde_json::from_value(wire).unwrap();
        assert!(back.rest_edit, "{back:?}");
    }
    let mut wire = serde_json::to_value(call(IssueOp::Change { n, change: IssueChange::Title("t".into()) })).unwrap();
    wire["format"] = serde_json::json!([0x20, 35]);
    assert!(serde_json::from_value::<IssueCall>(wire).is_err(), "32.35 marks only an edit, a comment edit or a lock");
}

/// F137's REST label update (format 32.36): a `DefineLabel` of an existing label's id renames it, recolors it, and
/// clears its description in one entry, keeping its issues; a rename to another label's name, in any case, is
/// refused. The update crosses the binding as 32.36 only when marked; a create, or 32.36 on one, is not a label update.
#[test]
fn a_rest_label_update_is_32_36_and_keeps_its_issues() {
    use graftyard_git::issue_ops::call::{IssueCall, REPO_FORMAT_ISSUES, REPO_FORMAT_LABEL_UPDATE};
    use graftyard_git::issues::{LabelId, LabelName};
    use graftyard_git::tiers::{LabelRow, get_rec, int, n_key};
    let mut c = setup(1, 91);
    let define = |id, name: &str, d: Option<&str>| IssueOp::DefineLabel {
        id,
        name: LabelName(name.into()),
        color: [1, 2, 3],
        description: d.map(Into::into),
    };
    let id = |out| match out {
        Ok(IssueOut::Id(i)) => LabelId(i),
        o => panic!("{o:?}"),
    };
    let bug = id(c.commit(WRITER, define(None, "bug", Some("broken"))));
    let ui = id(c.commit(WRITER, define(None, "ui", None)));
    let n = match c.commit(
        WRITER,
        IssueOp::OpenIssue { title: "t".into(), body: "".into(), labels: smallvec![bug], assignees: SmallVec::new() },
    ) {
        Ok(IssueOut::Number(n)) => n,
        o => panic!("{o:?}"),
    };
    let update =
        IssueOp::DefineLabel { id: Some(bug), name: LabelName("defect".into()), color: [9, 9, 9], description: None };
    assert_eq!(c.commit(WRITER, update.clone()), Ok(IssueOut::Id(bug.0)));
    let row = get_rec::<LabelRow>(&c.members[0].meta, "label", &[int(bug.0.0)]).unwrap();
    assert_eq!((&*row.name.0, row.color, row.description.as_deref()), ("defect", [9, 9, 9], None));
    assert!(c.members[0].meta.get("issue_label", &[n_key(n), int(bug.0.0)]).is_some(), "the issue keeps the label");
    assert_eq!(c.commit(WRITER, define(Some(bug), "UI", None)), Err(IssueErr::Exists), "another label's name");
    assert_eq!(c.commit(READER, define(Some(ui), "x", None)), Err(IssueErr::Forbidden));

    let blobless = |op: IssueOp| op.map_blob(|_| Err::<graftyard_git::blob::BlobRef, ()>(())).expect("no blob");
    let call = |op| IssueCall::new(blobless(op), who(WRITER), RequestId(Ulid(91)), serde_json::json!({}));
    let mut m = call(update);
    assert_eq!(m.format(), REPO_FORMAT_ISSUES, "unmarked, it keeps its bytes");
    m.rest_label = IssueCall::label_mark(&m.op);
    assert_eq!(m.format(), REPO_FORMAT_LABEL_UPDATE);
    let wire = serde_json::to_value(&m).unwrap();
    assert_eq!(wire["format"], serde_json::json!([0x20, 36]));
    let back: IssueCall = serde_json::from_value(wire).unwrap();
    assert!(back.rest_label && back.op == m.op, "{back:?}");
    assert!(!IssueCall::label_mark(&define(None, "new", None)), "a create is not a label update");
    let mut wire = serde_json::to_value(call(define(None, "new", None))).unwrap();
    wire["format"] = serde_json::json!([0x20, 36]);
    assert!(serde_json::from_value::<IssueCall>(wire).is_err(), "32.36 marks only a label update");
}

/// Milestones (format 32.37): numbers count from 1 and are never reused, a title another milestone has in any case is
/// refused, an issue's milestone moves its counts and so does closing or reopening the issue in the entry that does it,
/// a delete clears every issue's milestone, a reader's set changes nothing, and an edit with a milestone commits whole
/// or not at all. Every milestone command crosses the binding as 32.37, and 32.37 on any other command is malformed.
#[test]
fn milestones_number_count_and_clear_in_one_entry() {
    use graftyard_git::issue_ops::call::{IssueCall, REPO_FORMAT_MILESTONES};
    use graftyard_git::tiers::{issue_milestone, milestone, milestone_counts};
    let mut c = setup(2, 91);
    let define = |n, title: &str, open| IssueOp::DefineMilestone {
        n,
        title: title.into(),
        description: None,
        open,
        due_on: Some(Timestamp(5 * DAY)),
    };
    let open_issue = |c: &mut Chain| match c.commit(
        WRITER,
        IssueOp::OpenIssue { title: "t".into(), body: "".into(), labels: SmallVec::new(), assignees: SmallVec::new() },
    ) {
        Ok(IssueOut::Number(n)) => n,
        o => panic!("{o:?}"),
    };
    let meta = |c: &Chain| c.members[1].meta.clone();
    assert!(matches!(c.commit(WRITER, define(None, "v1", true)), Ok(IssueOut::Id(_))));
    assert!(matches!(c.commit(WRITER, define(None, "v2", true)), Ok(IssueOut::Id(_))));
    assert_eq!(c.commit(WRITER, define(None, "V1", true)), Err(IssueErr::Exists), "a taken title, in any case");
    assert_eq!(c.commit(READER, define(None, "v3", true)), Err(IssueErr::Forbidden));
    assert_eq!(c.commit(WRITER, define(Some(9), "v9", true)), Err(IssueErr::NotFound));
    assert_eq!(c.commit(WRITER, define(None, " ", true)), Err(IssueErr::Invalid("milestone title")));
    assert_eq!(milestone(&meta(&c), 2).map(|m| m.title), Some("v2".into()));

    let (a, b) = (open_issue(&mut c), open_issue(&mut c));
    let set = |n, m| IssueOp::SetMilestone { n, milestone: m };
    c.commit(WRITER, set(a, Some(1))).unwrap();
    c.commit(WRITER, set(b, Some(1))).unwrap();
    assert_eq!(milestone_counts(&meta(&c), 1), (2, 0));
    assert_eq!(c.commit(READER, set(a, Some(2))), Ok(IssueOut::Done), "a reader's set is ignored");
    assert_eq!(issue_milestone(&meta(&c), a), Some(1));
    assert_eq!(c.commit(WRITER, set(a, Some(7))), Err(IssueErr::Invalid("milestone")));

    // Closing and reopening an issue moves its milestone's counts in the same entry.
    let state = |open| IssueChange::State { open, reason: None };
    c.commit(WRITER, IssueOp::Change { n: a, change: state(false) }).unwrap();
    assert_eq!(milestone_counts(&meta(&c), 1), (1, 1));
    c.commit(WRITER, IssueOp::Close { n: b }).unwrap();
    assert_eq!(milestone_counts(&meta(&c), 1), (0, 2));
    c.commit(WRITER, IssueOp::Change { n: a, change: state(true) }).unwrap();
    assert_eq!(milestone_counts(&meta(&c), 1), (1, 1));
    // Moving an issue to another milestone moves it between their counts; clearing it removes it.
    c.commit(WRITER, set(a, Some(2))).unwrap();
    assert_eq!((milestone_counts(&meta(&c), 1), milestone_counts(&meta(&c), 2)), ((0, 1), (1, 0)));
    c.commit(WRITER, set(a, None)).unwrap();
    assert_eq!((issue_milestone(&meta(&c), a), milestone_counts(&meta(&c), 2)), (None, (0, 0)));

    // Closing a milestone stamps `closed`; editing it closed keeps the stamp; reopening clears it.
    c.commit(WRITER, define(Some(1), "v1", false)).unwrap();
    let closed = milestone(&meta(&c), 1).unwrap().closed;
    assert!(closed.is_some());
    c.now = Timestamp(c.now.0 + 1000);
    c.commit(WRITER, define(Some(1), "v1.0", false)).unwrap();
    assert_eq!(milestone(&meta(&c), 1).map(|m| (m.title, m.closed)), Some(("v1.0".into(), closed)));
    c.commit(WRITER, define(Some(1), "v1.0", true)).unwrap();
    assert_eq!(milestone(&meta(&c), 1).unwrap().closed, None);

    // A delete clears its issues' milestone, and its number is not handed out again.
    assert_eq!(c.commit(READER, IssueOp::DeleteMilestone { n: 1 }), Err(IssueErr::Forbidden));
    c.commit(WRITER, IssueOp::DeleteMilestone { n: 1 }).unwrap();
    assert_eq!((milestone(&meta(&c), 1), issue_milestone(&meta(&c), b)), (None, None));
    assert_eq!(c.rows(Tier::Metadata, "milestone_issue"), 0);
    c.commit(WRITER, define(None, "v1", true)).unwrap();
    assert!(milestone(&meta(&c), 3).is_some() && milestone(&meta(&c), 1).is_none(), "number 3, not 1");

    // An edit with a milestone is all or nothing: a missing milestone refuses the title change with it.
    let edit = |m| IssueOp::EditWithMilestone {
        n: b,
        changes: smallvec![IssueChange::Title("renamed".into())],
        labels: None,
        assignees: None,
        milestone: m,
    };
    let title = |c: &Chain| {
        graftyard_git::tiers::get_rec::<graftyard_git::tiers::IssueRow>(
            &c.members[0].meta,
            "issue",
            &[graftyard_git::tiers::n_key(b)],
        )
        .unwrap()
        .title
    };
    assert_eq!(c.commit(WRITER, edit(Some(1))), Err(IssueErr::Invalid("milestone")));
    assert_eq!(&*title(&c), "t");
    c.commit(WRITER, edit(Some(3))).unwrap();
    assert_eq!((&*title(&c), issue_milestone(&meta(&c), b)), ("renamed", Some(3)));
    // A create with a milestone sets it in the create's own entry; a reader's create keeps the issue and drops it.
    let create = IssueOp::OpenIssueWithMilestone {
        title: "new".into(),
        body: "".into(),
        labels: SmallVec::new(),
        assignees: SmallVec::new(),
        milestone: 3,
    };
    let entries = c.log.len();
    let Ok(IssueOut::Number(n)) = c.commit(WRITER, create.clone()) else { panic!() };
    assert_eq!((c.log.len(), issue_milestone(&meta(&c), n)), (entries + 1, Some(3)));
    let Ok(IssueOut::Number(r)) = c.commit(READER, create.clone()) else { panic!() };
    assert_eq!(issue_milestone(&meta(&c), r), None);
    // `b`, closed above, joined milestone 3 as closed; the writer's new issue is open.
    assert_eq!(milestone_counts(&meta(&c), 3), (1, 1));
    c.agree();

    // The wire: every milestone command is 32.37, and 32.37 on another command is malformed.
    let blobless = |op: IssueOp| op.map_blob(|_| Err::<graftyard_git::blob::BlobRef, ()>(())).expect("no blob");
    let call = |op| IssueCall::new(blobless(op), who(WRITER), RequestId(Ulid(92)), serde_json::json!({}));
    for op in [define(None, "x", true), IssueOp::DeleteMilestone { n: 3 }, set(a, None), edit(None), create] {
        let m = call(op);
        assert_eq!(m.format(), REPO_FORMAT_MILESTONES, "{:?}", m.op);
        let wire = serde_json::to_value(&m).unwrap();
        assert_eq!(wire["format"], serde_json::json!([0x20, 37]));
        let back: IssueCall = serde_json::from_value(wire).unwrap();
        assert_eq!(back.op, m.op);
    }
    let mut wire = serde_json::to_value(call(IssueOp::Close { n: a })).unwrap();
    wire["format"] = serde_json::json!([0x20, 37]);
    assert!(serde_json::from_value::<IssueCall>(wire).is_err(), "32.37 is only a milestone command's");
}

// ---- AM3-WP2a: a pull request's head in another repository of the base's network ----

/// Adds `rows` to every `Names` dep, and a `Names` dep for each of `repos`' `RepoOwner` shards the deps lack, so the
/// leader finds each head repository's row on the shard `IssueCmd::deps` names.
fn with_repo_rows(c: &mut Chain, repos: &[RepoId], rows: &[NamesEvent]) {
    use graftyard_identity::{NameKey, name_shard};
    for r in repos {
        let chain = ChainId::Names(name_shard(&NameKey::Repo(*r)));
        if !c.deps.iter().any(|(k, _)| *k == chain) {
            c.deps.push((chain, IdentityDep::Names(NamesFold::new(name_shard(&NameKey::Repo(*r))))));
        }
    }
    for (_, d) in &mut c.deps {
        if let IdentityDep::Names(f) = d {
            for row in rows {
                f.apply(row);
            }
        }
    }
}

fn fork_pr(head: RepoId, base_sha: ObjectId) -> IssueOp {
    IssueOp::OpenPr {
        head: HeadSpec { repo: head, branch: name("refs/heads/f") },
        base: name("refs/heads/main"),
        title: "t".into(),
        body: "".into(),
        draft: false,
        head_sha: oid(5),
        base_sha,
    }
}

#[test]
fn a_pull_request_head_in_another_repository_must_be_in_the_base_repositorys_network() {
    use graftyard_identity::{NameKey, name_shard};
    use graftyard_types::NetworkId;
    let owner = AccountId(DbId(OWNER));
    // REPO (7) roots network 7; FORK (8) is its fork. OTHER (9) roots network 9 and STRAY (10) is its fork. GHOST
    // (11) has no row. ROOT2 (20) roots network 20 and SIBLING (21) is its fork.
    let (fork, other, stray, ghost) = (RepoId(DbId(8)), RepoId(DbId(9)), RepoId(DbId(10)), RepoId(DbId(11)));
    let (root2, sibling) = (RepoId(DbId(20)), RepoId(DbId(21)));
    let rows = [
        NamesEvent::RepoOwner { repo: REPO, owner },
        NamesEvent::ForkOwner { repo: fork, owner, network: NetworkId::of_root(REPO) },
        NamesEvent::RepoOwner { repo: other, owner },
        NamesEvent::ForkOwner { repo: stray, owner, network: NetworkId::of_root(other) },
        NamesEvent::RepoOwner { repo: root2, owner },
        NamesEvent::ForkOwner { repo: sibling, owner, network: NetworkId::of_root(root2) },
    ];
    let heads = [fork, other, stray, ghost, root2, sibling];

    // The base is a root: its network is its own id.
    let mut c = setup(1, 90);
    with_repo_rows(&mut c, &heads, &rows);
    let base_sha = c.ref_of(&name("refs/heads/main")).unwrap();

    // A same-repository pull request reads the deps it read before: no `Names` shard for its head.
    c.push("refs/heads/f", oid(2));
    let same = IssueCmd {
        op: IssueOp::OpenPr {
            head: HeadSpec { repo: REPO, branch: name("refs/heads/f") },
            base: name("refs/heads/main"),
            title: "t".into(),
            body: "".into(),
            draft: false,
            head_sha: oid(2),
            base_sha,
        },
        actor: who(WRITER),
        request: RequestId(Ulid(1)),
    };
    let issue = IssueCmd {
        op: IssueOp::OpenIssue {
            title: "t".into(),
            body: "".into(),
            labels: SmallVec::new(),
            assignees: SmallVec::new(),
        },
        actor: who(WRITER),
        request: RequestId(Ulid(1)),
    };
    let from_fork = IssueCmd { op: fork_pr(fork, base_sha), actor: who(WRITER), request: RequestId(Ulid(1)) };
    {
        let m = &c.members[0];
        let view = RepoView::new(tiers(), &m.core, &m.meta, c.now);
        assert_eq!(same.deps(&view), issue.deps(&view), "a same-repository pull request's deps are unchanged");
        assert!(!same.deps(&view).contains(&ChainId::Names(name_shard(&NameKey::Repo(REPO)))));
        let names = ChainId::Names(name_shard(&NameKey::Repo(fork)));
        let mut want = issue.deps(&view);
        if !want.contains(&names) {
            want.push(names);
        }
        assert_eq!(from_fork.deps(&view), want, "a fork head also reads its repository's `Names` shard");
        // A leader that did not read that shard fails closed.
        let without: Vec<_> = c.deps.iter().filter(|(k, _)| *k != names).cloned().collect();
        assert!(matches!(from_fork.decide(&view, &without), Err(IssueErr::Unauthorized(_))));
    }
    assert!(matches!(c.commit(WRITER, same.op.clone()), Ok(IssueOut::Number(_))), "same repository still opens");

    let refused = Err(IssueErr::Invalid("the head repository is not in the base repository's network"));
    assert!(matches!(c.commit(WRITER, fork_pr(fork, base_sha)), Ok(IssueOut::Number(_))), "a fork in the network");
    assert_eq!(c.commit(WRITER, fork_pr(other, base_sha)), refused, "another network's root");
    assert_eq!(c.commit(WRITER, fork_pr(stray, base_sha)), refused, "another network's fork");
    assert_eq!(c.commit(WRITER, fork_pr(ghost, base_sha)), Err(IssueErr::HeadMissing), "no such repository");

    // The base is itself a fork of ROOT2: its network is 20, not its own id 7. Its root and a sibling fork may be
    // heads; a fork of network 7 may not.
    let mut c = setup(1, 90);
    with_repo_rows(&mut c, &heads, &rows);
    let genesis = RepoEvent::Genesis {
        owner,
        network: NetworkId::of_root(root2),
        name: RepoName::parse("tool").unwrap(),
        fork_of: Some((root2, Index(3))),
    };
    c.entry(EntryBody::Events(vec![genesis]), TierSet::CORE);
    let base_sha = c.ref_of(&name("refs/heads/main")).unwrap();
    assert!(matches!(c.commit(WRITER, fork_pr(root2, base_sha)), Ok(IssueOut::Number(_))), "the base's root");
    assert!(matches!(c.commit(WRITER, fork_pr(sibling, base_sha)), Ok(IssueOut::Number(_))), "a sibling fork");
    assert_eq!(c.commit(WRITER, fork_pr(fork, base_sha)), refused, "a fork of the base's own id");
    assert_eq!(c.commit(WRITER, fork_pr(ghost, base_sha)), Err(IssueErr::HeadMissing), "no such repository");
}
