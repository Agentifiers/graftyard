//! F59 end to end on real members: only the owner's `Authz` consumer finishes a repository create or delete after
//! every edge cut. The identity groups come from the identity test host; the repository group is absent until the
//! edge push or the consumer's `Init` needs it.

#[path = "../../identity/tests/common/mod.rs"]
mod common;

use common::world::{FreshLeader, OPS, World};
use common::{A, Group};
use graftyard_git::issue_ops::call::{IssueCall, REPO_FORMAT_ISSUES};
use graftyard_git::issue_ops::{IssueErr, IssueOp};
use graftyard_git::issues::{MergeMethod, RepoSettings, Visibility};
use graftyard_git::lifecycle::{REPO_FORMAT_LIFECYCLE, of_step};
use graftyard_git::pack::PackRef;
use graftyard_git::refs::{Atomicity, ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{Push, REPO_FORMAT_REPOS, REPO_FORMAT_V1, RepoCmd, RepoFold, RepoOut};
use graftyard_git::tiers::{created, deleted, get_rec};
use graftyard_identity::audit::{self, AuditRead, Finisher, Owed, RepoRead, Want};
use graftyard_identity::finish::{CREATE_ABORT_AFTER, RepoLife, RepoStep, Step};
use graftyard_identity::{
    AccountKind, Actor, AuthzCmd, AuthzFold, AuthzOp, AuthzRead, Credential, IdBlock, NameKey, NamesCmd, NamesFold,
    NamesOp, NamesRead, Principal, RepoEntry, RepoName, ScopeSet, TokenKind, TokensCmd, TokensOp, UserId,
    account_shard, name_shard,
};
use graftyard_raft::{CommitError, Index};
use graftyard_types::{AccountId, ChainId, CredentialShard, DbId, RepoId, SiteId, Timestamp, Ulid};
use smallvec::{SmallVec, smallvec};

const OWNER: AccountId = AccountId(DbId(59));
const TOKEN: [u8; 32] = [0x59; 32];
const SITES: [SiteId; 3] = common::SITES;
const MINUTE: u64 = 60_000;
const DAY: u64 = 86_400_000;

fn as_user() -> CredentialShard {
    CredentialShard { account: Some(OWNER), key: None }
}

fn principal() -> Principal {
    Principal {
        actor: Actor::User(UserId(OWNER)),
        scopes: ScopeSet(1),
        credential: Credential::Stored(TOKEN),
        sso_orgs: SmallVec::new(),
    }
}

fn settings(description: &str) -> RepoSettings {
    RepoSettings {
        owner: OWNER,
        enterprise: None,
        visibility: Visibility::Private,
        default_branch: RefName::parse(b"refs/heads/main").unwrap(),
        archived: false,
        description: Some(description.into()),
        homepage: None,
        topics: SmallVec::new(),
        has_issues: true,
        has_wiki: false,
        has_projects: false,
        merge_methods: smallvec![MergeMethod::Merge],
        delete_branch_on_merge: false,
        allow_auto_merge: false,
        allow_update_branch: false,
        check_retention_days: 90,
    }
}

struct Create {
    repo: RepoId,
    name: RepoName,
    expected: RepoSettings,
    init: Box<str>,
}

/// Three real identity groups (or as many shards as the commands ask for), and at most one real repository group.
struct Harness {
    ids: World,
    repo: Option<Group<RepoCmd>>,
    refuse_repo_create: bool,
    next: u128,
}

impl Harness {
    fn new() -> Harness {
        let mut ids = World::default();
        ids.authz_cmd(OWNER, AuthzCmd { cred: OPS, op: AuthzOp::Open { kind: AccountKind::User, enterprise: None } })
            .unwrap();
        let g = ids.authz(OWNER);
        g.id_block(A, IdBlock::whole(0, 59).unwrap()).unwrap();
        g.settle(&mut |c| panic!("Authz Open asked for {c}"));

        // The edge principal is backed by current folds of real Names and Tokens groups too.
        let put = NamesOp::PutAccount { id: OWNER, suspended: false, sessions_not_before: Timestamp(0) };
        ids.names_cmd(ChainId::Names(account_shard(OWNER)), NamesCmd { cred: OPS, op: put }).unwrap();
        let store =
            TokensOp::Store { hash: TOKEN, kind: TokenKind::Pat, account: OWNER, scopes: ScopeSet(1), expires: None };
        ids.tokens_cmd(TokensCmd { cred: as_user(), op: store }).unwrap();
        Harness { ids, repo: None, refuse_repo_create: false, next: 0x5900 }
    }

    fn rid(&mut self) -> graftyard_types::RequestId {
        self.next += 1;
        graftyard_types::RequestId(Ulid(self.next << 40))
    }

    fn now(&mut self) -> Timestamp {
        self.ids.authz(OWNER).now
    }

    fn authz_create(&mut self, name: &RepoName, init: &str) -> RepoId {
        let op = AuthzOp::CreateRepoWith { name: name.clone(), init: init.into() };
        let out = self.ids.authz_cmd(OWNER, AuthzCmd { cred: as_user(), op }).unwrap().out;
        let graftyard_identity::AuthzOut::Repo { repo, .. } = out else { panic!("create returned no repository") };
        repo
    }

    fn begin_create(&mut self, label: &str) -> Create {
        let name = RepoName::parse(label).unwrap();
        let expected = settings(&format!("settings requested by {label}"));
        let init = serde_json::to_string(&expected).unwrap().into_boxed_str();
        let repo = self.authz_create(&name, &init);
        assert!(self.repo.is_none(), "{label}: the repository group starts absent");
        Create { repo, name, expected, init }
    }

    fn repo_owner(&mut self, repo: RepoId) {
        self.ids
            .names_cmd(
                ChainId::Names(name_shard(&NameKey::Repo(repo))),
                NamesCmd { cred: OPS, op: NamesOp::RepoOwner { repo, owner: OWNER } },
            )
            .unwrap();
    }

    fn make_repo(&mut self, repo: RepoId) {
        assert!(self.repo.is_none());
        let reads = vec![REPO_FORMAT_V1, REPO_FORMAT_ISSUES, REPO_FORMAT_REPOS, REPO_FORMAT_LIFECYCLE];
        let mut g = Group::reading(ChainId::Repo(repo), reads, RepoFold::new(repo, OWNER));
        g.now = self.now();
        g.elect(A);
        self.repo = Some(g);
    }

    fn repo_cmd(&mut self, cmd: RepoCmd) -> common::Outcome<RepoCmd> {
        let id = self.rid().0.0;
        let mut g = self.repo.take().expect("a repository group");
        let r = g.submit(id, cmd, &mut |c| self.ids.read(c));
        self.repo = Some(g);
        r
    }

    fn edge_init(&mut self, expected: RepoSettings) {
        let call = IssueCall::new(IssueOp::InitRepo(expected), principal(), self.rid(), serde_json::Value::Null);
        self.repo_cmd(RepoCmd::Issue(Box::new(call))).unwrap();
        self.settle_repo();
    }

    /// Let every follower learn the just-committed index before asserting its fold, as `lifecycle.rs` does.
    fn settle_repo(&mut self) {
        if let Some(g) = &mut self.repo {
            let target = g.m(g.leader().unwrap()).state().commit;
            for _ in 0..20 {
                if SITES.iter().all(|s| g.m(*s).state().applied >= target) {
                    return;
                }
                g.advance(1_000);
                g.deliver_all();
                // An up-to-date leader sends no heartbeat merely to advance its followers' commit indexes. A read
                // past its lease needs a quorum round, which carries the commit index to them.
                let _ = g.fresh(A);
            }
            panic!("repository followers did not learn commit {target:?}");
        }
    }

    fn edge_delete(&mut self, retain_until: Timestamp) {
        let call =
            IssueCall::new(IssueOp::DeleteRepo { retain_until }, principal(), self.rid(), serde_json::Value::Null);
        self.repo_cmd(RepoCmd::Issue(Box::new(call))).unwrap();
        self.settle_repo();
    }

    /// The same inline-pack push shape as the repository raft tests: no git-cache-only command is needed.
    fn readme(&mut self, repo: RepoId) {
        if self.repo.is_none() {
            self.make_repo(repo);
        }
        let push = Push {
            updates: vec![RefUpdate {
                name: RefName::parse(b"refs/heads/main").unwrap(),
                expect: None,
                new: Some(ObjectId([0x59; 20])),
            }],
            atomicity: Atomicity::PerRef,
            pack: Some(PackRef::Inline(b"PACK\0\0\0\x02\0\0\0\0".to_vec())),
            actor: principal(),
            options: Vec::new(),
            request: self.rid(),
            agent: None,
            provenance: Vec::new(),
            protected: Vec::new(),
            changed: Vec::new(),
            leases: false,
            test_format: false,
        };
        match self.repo_cmd(RepoCmd::Push(push)).unwrap().out {
            RepoOut::Push(out) => assert!(out.refs.iter().all(|(_, r)| *r == graftyard_git::repo::RefOutcome::Ok)),
            other => panic!("README push returned {other:?}"),
        }
        self.settle_repo();
        let g = self.repo.as_ref().unwrap();
        for s in SITES {
            let fold = &g.m(s).group_fold().fold;
            assert_eq!(fold.get(&RefName::parse(b"refs/heads/main").unwrap()), Some(ObjectId([0x59; 20])));
            assert_eq!(created(&*fold.core), None, "the README cut precedes settings and Created");
        }
    }

    fn advance(&mut self, ms: u64) {
        fn tick<C: graftyard_raft::Command>(g: &mut Group<C>, ms: u64) {
            g.advance(ms);
            g.deliver_all();
            if g.leader().is_none() {
                g.elect(A);
            }
        }
        self.ids.names.values_mut().for_each(|g| tick(g, ms));
        self.ids.tokens.values_mut().for_each(|g| tick(g, ms));
        self.ids.identity.values_mut().for_each(|g| tick(g, ms));
        self.ids.authz.values_mut().for_each(|g| tick(g, ms));
        if let Some(g) = &mut self.repo {
            tick(g, ms);
        }
    }

    fn advance_to(&mut self, target: Timestamp) {
        let now = self.now();
        assert!(target > now, "target {target:?} is not after {now:?}");
        self.advance(target.0 - now.0);
    }

    fn read(&mut self, chain: ChainId) -> AuditRead {
        match chain {
            ChainId::Names(_) => AuditRead::Names(self.ids.names(chain).fresh_leader()),
            ChainId::Tokens(s) => AuditRead::Tokens(self.ids.tokens(s).fresh_leader()),
            ChainId::Authz(o) => AuditRead::Authz(self.ids.authz(o).fresh_leader()),
            ChainId::Identity(a) => AuditRead::Identity(self.ids.identity(a).fresh_leader()),
            ChainId::Repo(repo) => AuditRead::Repo(self.repo_read(repo)),
            other => panic!("audit reads no {other}"),
        }
    }

    fn repo_read(&mut self, repo: RepoId) -> RepoRead {
        let Some(g) = &mut self.repo else {
            return RepoRead { owner: OWNER, enterprise: None, voters: Vec::new(), home: A, life: RepoLife::NoGroup };
        };
        assert_eq!(g.chain, ChainId::Repo(repo));
        let fold: RepoFold = g.fresh_leader();
        let settings: Option<RepoSettings> = get_rec(&*fold.core, "settings", &[]);
        let life = if deleted(&*fold.core).is_some() {
            RepoLife::Deleted
        } else if created(&*fold.core).is_some() {
            RepoLife::Created
        } else {
            RepoLife::Open
        };
        RepoRead {
            owner: settings.as_ref().map_or(OWNER, |s| s.owner),
            enterprise: settings.as_ref().and_then(|s| s.enterprise),
            voters: SITES.to_vec(),
            home: A,
            life,
        }
    }

    fn submit(&mut self, step: Step) -> bool {
        match step {
            Step::Names(chain, cmd) => self.ids.names_cmd(chain, cmd).is_err(),
            Step::Authz(ChainId::Authz(OWNER), cmd) => self.ids.authz_cmd(OWNER, *cmd).is_err(),
            Step::Repo(repo, step) => {
                assert!(matches!(step, RepoStep::Init { .. } | RepoStep::Delete { .. }));
                if self.repo.is_none() {
                    if self.refuse_repo_create {
                        return true;
                    }
                    self.make_repo(repo);
                }
                let cmd = of_step(&step, self.rid()).expect("an F59 step maps").expect("valid create settings JSON");
                self.repo_cmd(RepoCmd::Lifecycle(cmd)).is_err()
            }
            other => panic!("unexpected finisher step {other:?}"),
        }
    }

    /// One scan and finisher batch over the Authz leader's committed log. `Err` returns a stuck item without hiding it.
    fn batch(&mut self) -> Result<Option<audit::Report>, Owed> {
        let (now, cursor, log) = {
            let g = self.ids.authz(OWNER);
            let leader = g.leader().unwrap();
            let m = g.m(leader);
            let committed = m.state().commit.0 as usize;
            (g.now, m.group_fold().fold.audit_cursor(), m.log()[..committed].to_vec())
        };
        let mut owed = Vec::new();
        let scanned = audit::scan(
            cursor,
            Index(log.len() as u64),
            now,
            2_000,
            |i| log.get(i.0 as usize - 1).cloned(),
            |_, e| owed.extend(audit::authz_owed(OWNER, e)),
        )
        .expect("the committed log is in memory");
        if !audit::needs_batch(cursor, scanned.through, &owed, 0) {
            return Ok(None);
        }
        let chain = ChainId::Authz(OWNER);
        let mut f = Finisher::new(chain, now, cursor, scanned.through, owed);
        for _ in 0..200 {
            match f.want() {
                Want::Read(chains) => {
                    for c in chains {
                        let fold = self.read(c);
                        f.read(c, fold);
                    }
                }
                Want::Submit(step) => {
                    let refused = self.submit(step);
                    f.submitted(refused);
                }
                Want::Done(report) => return Ok(Some(report)),
                Want::Stuck(owed) => return Err(owed),
            }
        }
        panic!("the finisher batch did not converge");
    }

    fn name_fold(&mut self) -> AuthzFold {
        self.ids.authz(OWNER).fresh_leader()
    }

    fn owner_fold(&mut self, repo: RepoId) -> NamesFold {
        self.ids.names(ChainId::Names(name_shard(&NameKey::Repo(repo)))).fresh_leader()
    }

    fn log_lengths(&self) -> Vec<(String, usize)> {
        fn len<C: graftyard_raft::Command>(g: &Group<C>) -> usize {
            let leader = g.leader().expect("a leader");
            g.m(leader).log().len()
        }
        let mut out = Vec::new();
        out.extend(self.ids.authz.iter().map(|(id, g)| (format!("Authz({id})"), len(g))));
        out.extend(self.ids.names.iter().map(|(id, g)| (format!("Names({id})"), len(g))));
        out.extend(self.ids.tokens.iter().map(|(id, g)| (format!("Tokens({id})"), len(g))));
        out.extend(self.ids.identity.iter().map(|(id, g)| (format!("Identity({id})"), len(g))));
        if let Some(g) = &self.repo {
            out.push((g.chain.to_string(), len(g)));
        }
        out
    }

    fn rerun_is_quiescent(&mut self, cut: &str) {
        let before = self.log_lengths();
        let steps = self.batch().unwrap_or_else(|o| panic!("{cut}: rerun stuck on {o:?}")).map_or(0, |r| r.steps);
        assert_eq!(steps, 0, "{cut}: the second batch owes no steps");
        assert_eq!(self.log_lengths(), before, "{cut}: no group's log changed on the second batch");
    }
}

#[derive(Clone, Copy)]
enum CreateCut {
    Name,
    Owner,
    Readme,
    Done,
}

impl CreateCut {
    const ALL: [CreateCut; 4] = [Self::Name, Self::Owner, Self::Readme, Self::Done];
    fn label(self) -> &'static str {
        match self {
            Self::Name => "after-name",
            Self::Owner => "after-repo-owner",
            Self::Readme => "after-readme",
            Self::Done => "whole-create-control",
        }
    }
    fn steps(self) -> u32 {
        match self {
            Self::Name => 2,
            Self::Owner | Self::Readme => 1,
            Self::Done => 0,
        }
    }
}

#[test]
fn audit_alone_finishes_every_create_cut() {
    for cut in CreateCut::ALL {
        let label = cut.label();
        let mut h = Harness::new();
        let create = h.begin_create(label);
        if !matches!(cut, CreateCut::Name) {
            h.repo_owner(create.repo);
        }
        if matches!(cut, CreateCut::Readme | CreateCut::Done) {
            h.readme(create.repo);
        }
        if matches!(cut, CreateCut::Done) {
            h.edge_init(create.expected.clone());
        }

        h.advance(MINUTE + 1_000);
        let report = h.batch().unwrap_or_else(|o| panic!("{label}: stuck on {o:?}")).expect("the create is due");
        h.settle_repo();
        assert_eq!((report.owed, report.steps, report.refused), (1, cut.steps(), 0), "{label}");
        assert_eq!(h.owner_fold(create.repo).repo_owner(create.repo), Some(OWNER), "{label}: RepoOwner");
        let authz = h.name_fold();
        assert!(
            matches!(authz.name(&create.name.key()), Some(RepoEntry::Live { repo, .. }) if repo == create.repo),
            "{label}: the name stays Live"
        );
        let repo = h.repo.as_ref().unwrap_or_else(|| panic!("{label}: consumer made no repository group"));
        assert_eq!(repo.chain, ChainId::Repo(create.repo));
        for s in SITES {
            let fold = &repo.m(s).group_fold().fold;
            assert!(created(&*fold.core).is_some(), "{label}: {s:?} has Created");
            let have: Option<RepoSettings> = get_rec(&*fold.core, "settings", &[]);
            assert_eq!(
                have.as_ref().and_then(|s| s.description.as_deref()),
                create.expected.description.as_deref(),
                "{label}: {s:?} settings description comes from CreateRepoWith ({})",
                create.init
            );
            assert_eq!(have, Some(create.expected.clone()), "{label}: {s:?} settings");
        }
        h.rerun_is_quiescent(label);
    }
}

#[test]
fn audit_alone_finishes_a_delete_cut_short_and_a_finished_delete_is_inert() {
    for edge_finished in [false, true] {
        let label = if edge_finished { "whole-delete-control" } else { "delete-after-name" };
        let mut h = Harness::new();
        let create = h.begin_create(label);
        h.repo_owner(create.repo);
        h.readme(create.repo);
        h.edge_init(create.expected);

        let retain_until = Timestamp(h.now().0 + 90 * DAY);
        h.ids
            .authz_cmd(
                OWNER,
                AuthzCmd { cred: as_user(), op: AuthzOp::DeleteRepo { key: create.name.key(), retain_until } },
            )
            .unwrap();
        if edge_finished {
            h.edge_delete(retain_until);
        }

        h.advance(MINUTE + 1_000);
        let report = h.batch().unwrap_or_else(|o| panic!("{label}: stuck on {o:?}")).expect("the delete is due");
        h.settle_repo();
        assert_eq!(report.owed, 2, "{label}: the completed create and delete commit points are scanned");
        assert_eq!(report.steps, u32::from(!edge_finished), "{label}: only the missing tombstone is submitted");
        assert_eq!(report.refused, 0, "{label}");
        let authz = h.name_fold();
        assert_eq!(authz.deleted(create.repo), Some(retain_until), "{label}: the name's deadline");
        let repo = h.repo.as_ref().unwrap();
        for s in SITES {
            let fold = &repo.m(s).group_fold().fold;
            assert_eq!(deleted(&*fold.core).map(|(until, _)| until), Some(retain_until), "{label}: {s:?}");
        }
        h.rerun_is_quiescent(label);
    }
}

#[test]
fn a_create_whose_group_never_forms_is_aborted_and_a_late_edge_init_is_not_found() {
    let mut h = Harness::new();
    let create = h.begin_create("abort-after-refused-init");
    h.refuse_repo_create = true;
    h.advance(MINUTE + 1_000);
    let owed = h.batch().expect_err("six refused Init attempts leave the batch stuck");
    let (repo, at) = match owed {
        Owed::Create { repo, at, .. } => (repo, at),
        other => panic!("unexpected stuck item {other:?}"),
    };
    assert_eq!(repo, create.repo);
    assert!(h.repo.is_none(), "the harness refused every repository-group creation");
    assert!(
        matches!(h.name_fold().name(&create.name.key()), Some(RepoEntry::Live { repo, .. }) if repo == create.repo)
    );
    assert_eq!(h.owner_fold(repo).repo_owner(repo), Some(OWNER));
    assert!(h.now().0 < at.0 + CREATE_ABORT_AFTER.as_millis() as u64, "the first batch is before the abort deadline");

    h.advance_to(Timestamp(at.0 + CREATE_ABORT_AFTER.as_millis() as u64 + 1_000));
    let report = h.batch().unwrap().expect("the abort is due");
    assert_eq!((report.owed, report.steps, report.refused), (1, 1, 0));
    assert_eq!(h.name_fold().name(&create.name.key()), None, "601 seconds after the commit point the name is gone");

    // An edge that was partitioned until after the abort may still build the old id's group, but cannot revive it.
    h.refuse_repo_create = false;
    h.make_repo(repo);
    let call = IssueCall::new(IssueOp::InitRepo(create.expected), principal(), h.rid(), serde_json::Value::Null);
    assert!(matches!(
        h.repo_cmd(RepoCmd::Issue(Box::new(call))),
        Err(CommitError::Refused { err: graftyard_git::repo::RepoErr::Issue { err: IssueErr::NotFound, .. }, .. })
    ));
    let repo_group = h.repo.as_ref().unwrap();
    for s in SITES {
        let fold = &repo_group.m(s).group_fold().fold;
        assert_eq!(created(&*fold.core), None, "{s:?} was not initialized by the late edge");
    }
}
