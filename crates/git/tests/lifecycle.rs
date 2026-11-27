//! F59's repository finisher on three real `graftyard_raft::Member<RepoCmd>`s. The owner's editable `Authz` fold is
//! the commit point: only its `Live` name lets a create finish, and only a matching `Deleted` name lets a delete finish.

use graftyard_git::issue_ops::call::REPO_FORMAT_FORKS;
use graftyard_git::issue_ops::call::{IssueCall, REPO_FORMAT_ISSUES};
use graftyard_git::issue_ops::{IssueErr, IssueOp};
use graftyard_git::issues::{MergeMethod, RepoSettings, Visibility};
use graftyard_git::lifecycle::{
    FORK_REFS_MAX, Lifecycle, LifecycleErr, LifecycleOp, LifecycleOut, REPO_FORMAT_LIFECYCLE, fork_of_step, of_step,
};
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_git::repo::{
    REPO_FORMAT_LEASES, REPO_FORMAT_REPOS, REPO_FORMAT_V1, RepoCmd, RepoErr, RepoFold, RepoOut, RepoScratch,
};
use graftyard_git::tiers::{ForkRow, created, deleted, get_rec};
use graftyard_identity::{
    AccountKind, Actor, AuthzEvent, AuthzFold, Credential, IdentityDep, NameEpoch, NamesEvent, NamesFold, OrgRole,
    Principal, RepoEntry, RepoName, ScopeSet, TokenKind, TokensEvent, TokensFold, UserId,
};
use graftyard_raft::{
    CallError, Command, CommitError, Envelope, Ev, Fold as _, Format, Member, MemberConfig, Record, ReleaseTag,
    Resolution, Role, Stored,
};
use graftyard_types::{
    AccountId, ChainId, DbId, Index, NameShard, NetworkId, PlacementName, PlacementRecord, Region, ReleaseId, RepoId,
    RequestId, SiteId, Timestamp, TokenShard, Ulid,
};
use smallvec::{SmallVec, smallvec};

const SITES: [SiteId; 3] = [SiteId(0), SiteId(1), SiteId(2)];
const REPO: RepoId = RepoId(DbId(7));
const OTHER_REPO: RepoId = RepoId(DbId(8));
const OWNER: AccountId = AccountId(DbId(100));
const OTHER_OWNER: AccountId = AccountId(DbId(200));
const ENTERPRISE: AccountId = AccountId(DbId(300));
const OTHER_ENTERPRISE: AccountId = AccountId(DbId(301));
const ADMIN: i64 = 10;
const DAY: u64 = 86_400_000;

fn placement() -> PlacementRecord {
    PlacementRecord {
        name: PlacementName::Tri,
        sites: SITES.iter().copied().collect(),
        home: SITES[0],
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    }
}

fn config(me: SiteId, reads: &[Format]) -> MemberConfig {
    MemberConfig {
        chain: ChainId::Repo(REPO),
        me,
        record: placement(),
        regions: vec![(SITES[0], Region::Enam), (SITES[1], Region::Weur), (SITES[2], Region::Apac)],
        release: ReleaseTag { id: ReleaseId([3; 32]), seq: 1 },
        reads: reads.to_vec(),
    }
}

fn formats() -> Vec<Format> {
    vec![REPO_FORMAT_V1, REPO_FORMAT_ISSUES, REPO_FORMAT_REPOS, REPO_FORMAT_LIFECYCLE]
}

fn who(account: i64) -> Principal {
    Principal {
        actor: Actor::User(UserId(AccountId(DbId(account)))),
        scopes: ScopeSet(1),
        credential: Credential::Stored([account as u8; 32]),
        sso_orgs: SmallVec::new(),
    }
}

fn org(owner: AccountId, enterprise: Option<AccountId>) -> AuthzFold {
    let mut fold = AuthzFold::new(owner);
    fold.apply(&AuthzEvent::Opened { kind: AccountKind::Org, enterprise });
    fold
}

/// Editable dependencies. `read_authz = false` omits the owner's fold rather than supplying an empty one.
#[derive(Clone)]
struct Deps {
    owner: AuthzFold,
    other_owner: AuthzFold,
    read_authz: bool,
}

impl Deps {
    fn new() -> Deps {
        let mut owner = org(OWNER, Some(ENTERPRISE));
        owner.apply(&AuthzEvent::Member { user: UserId(AccountId(DbId(ADMIN))), role: Some(OrgRole::Admin) });
        let live =
            AuthzEvent::RepoName { key: Self::key(), entry: Some(RepoEntry::Live { repo: REPO, epoch: NameEpoch(1) }) };
        owner.apply(&live);
        let mut other_owner = org(OTHER_OWNER, Some(ENTERPRISE));
        other_owner.apply(&live);
        Deps { owner, other_owner, read_authz: true }
    }

    fn key() -> graftyard_identity::RepoNameKey {
        RepoName::parse("tool").unwrap().key()
    }

    fn name(&mut self, entry: Option<RepoEntry>) {
        self.owner.apply(&AuthzEvent::RepoName { key: Self::key(), entry });
    }

    fn read(&self, chain: ChainId) -> Option<IdentityDep> {
        match chain {
            ChainId::Names(_) => {
                let mut names = NamesFold::new(NameShard(0));
                names.apply(&NamesEvent::Account {
                    id: AccountId(DbId(ADMIN)),
                    suspended: false,
                    sessions_not_before: Timestamp(0),
                });
                Some(IdentityDep::Names(names))
            }
            ChainId::Tokens(shard) => {
                let mut tokens = TokensFold::new(TokenShard(0));
                tokens.apply(&TokensEvent::Stored {
                    hash: [ADMIN as u8; 32],
                    kind: TokenKind::Pat,
                    account: AccountId(DbId(ADMIN)),
                    scopes: ScopeSet(1),
                    expires: None,
                });
                tokens.shard = Some(shard);
                Some(IdentityDep::Tokens(tokens))
            }
            ChainId::Authz(OWNER) if self.read_authz => Some(IdentityDep::Authz(self.owner.clone())),
            ChainId::Authz(OWNER) => None,
            ChainId::Authz(OTHER_OWNER) => Some(IdentityDep::Authz(self.other_owner.clone())),
            ChainId::Authz(owner) => Some(IdentityDep::Authz(org(owner, None))),
            other => panic!("no dep {other}"),
        }
    }
}

/// Three members of one repository group, with the same commit/deps plumbing as `transfer.rs`.
struct G {
    now: Timestamp,
    members: Vec<Option<Member<RepoCmd>>>,
    disks: Vec<Stored<RepoFold>>,
    inbox: Vec<Envelope<Ev<RepoCmd>>>,
    resolutions: Vec<Resolution<RepoCmd>>,
    deps: Deps,
    boots: u64,
    next: u128,
}

impl G {
    fn new() -> G {
        G::with_reads(formats())
    }

    fn with_reads(reads: Vec<Format>) -> G {
        let mut g = G {
            now: Timestamp(10_000),
            members: Vec::new(),
            disks: SITES.iter().map(|_| Stored::new(RepoFold::new(REPO, OWNER))).collect(),
            inbox: Vec::new(),
            resolutions: Vec::new(),
            deps: Deps::new(),
            boots: 0,
            next: 1,
        };
        for site in SITES {
            g.boots += 1;
            let (member, ready) =
                Member::restore(config(site, &reads), g.disks[site.0 as usize].clone(), g.now, g.boots)
                    .expect("restore");
            g.members.push(Some(member));
            let ((), out) = ready.persist(&mut g.disks[site.0 as usize]);
            g.inbox.extend(out.early.into_iter().chain(out.messages));
        }
        for _ in 0..3 {
            g.on(SITES[0], |member, now| member.campaign(now));
            g.deliver();
            if g.m(SITES[0]).role() == Role::Leader {
                break;
            }
        }
        assert_eq!(g.m(SITES[0]).role(), Role::Leader);
        g.pass(2_000);
        g
    }

    fn m(&self, site: SiteId) -> &Member<RepoCmd> {
        self.members[site.0 as usize].as_ref().expect("member is up")
    }

    fn on<R>(
        &mut self,
        site: SiteId,
        f: impl FnOnce(&mut Member<RepoCmd>, Timestamp) -> graftyard_raft::Ready<RepoCmd, R>,
    ) -> R {
        let now = self.now;
        let member = self.members[site.0 as usize].as_mut().expect("member is up");
        let (result, out) = f(member, now).persist(&mut self.disks[site.0 as usize]);
        let mut outs = vec![out];
        while let Some(mut out) = outs.pop() {
            self.inbox.extend(out.early.drain(..).chain(out.messages.drain(..)));
            self.resolutions.append(&mut out.resolutions);
            let member = self.members[site.0 as usize].as_mut().expect("member is up");
            let disk = &mut self.disks[site.0 as usize];
            if let Some(ack) = out.self_ack.take() {
                outs.push(member.self_ack(now, ack).persist(disk).1);
            }
            for dep in out.deps.drain(..) {
                let read: Vec<_> =
                    dep.chains.iter().filter_map(|chain| Some((*chain, self.deps.read(*chain)?))).collect();
                outs.push(member.deps_read(now, dep.id, read).persist(disk).1);
            }
        }
        result
    }

    fn deliver(&mut self) {
        for _ in 0..10_000 {
            if self.inbox.is_empty() {
                return;
            }
            for envelope in std::mem::take(&mut self.inbox) {
                if self.members[envelope.to.0 as usize].is_none() {
                    continue;
                }
                let Envelope { from, to, msg } = envelope;
                self.on(to, |member, now| member.step(now, from, msg));
            }
        }
        panic!("messages never settled");
    }

    fn pass(&mut self, ms: u64) {
        let end = self.now.0 + ms;
        while self.now.0 < end {
            self.now = Timestamp((self.now.0 + 1_000).min(end));
            for site in SITES {
                if self.members[site.0 as usize].is_some() {
                    self.on(site, |member, now| member.tick(now));
                }
            }
            self.deliver();
        }
    }

    fn rid(&mut self) -> RequestId {
        self.next += 1;
        RequestId(Ulid(self.next << 64))
    }

    fn commit(&mut self, cmd: RepoCmd) -> Result<RepoOut, RepoErr> {
        let rid = self.rid();
        let admitted = self.on(SITES[0], |member, now| member.commit(now, rid, cmd)).expect("admitted");
        self.pass(2_000);
        let result = self.resolutions.iter().rev().find_map(|resolution| match resolution {
            Resolution::Ticket { ticket, result } if *ticket == admitted.ticket => Some(result.clone()),
            _ => None,
        });
        match result {
            Some(Ok(acked)) => Ok(acked.out),
            Some(Err(CommitError::Refused { err, .. })) => Err(err),
            other => panic!("unresolved: {:?}", other.map(|result| result.is_ok())),
        }
    }

    fn lifecycle(&mut self, op: LifecycleOp) -> Result<LifecycleOut, LifecycleErr> {
        let request = self.rid();
        match self.commit(RepoCmd::Lifecycle(Lifecycle { op, request })) {
            Ok(RepoOut::Lifecycle(out)) => Ok(out),
            Err(RepoErr::Lifecycle(err)) => Err(err),
            other => panic!("unexpected lifecycle result: {other:?}"),
        }
    }

    fn issue(&mut self, op: IssueOp<graftyard_git::blob::BlobRef>) -> Result<RepoOut, RepoErr> {
        let request = self.rid();
        let call = IssueCall::new(op, who(ADMIN), request, serde_json::Value::Null);
        self.commit(RepoCmd::Issue(Box::new(call)))
    }

    fn settings(&self, site: SiteId) -> Option<RepoSettings> {
        let scratch: RepoScratch<'_> = self.m(site).group_fold().fold.scratch();
        get_rec(scratch.core(), "settings", &[])
    }

    fn created(&self, site: SiteId) -> Option<Timestamp> {
        created(self.m(site).group_fold().fold.scratch().core())
    }

    fn deleted(&self, site: SiteId) -> Option<(Timestamp, Option<AccountId>)> {
        deleted(self.m(site).group_fold().fold.scratch().core())
    }
}

fn settings(description: &str) -> RepoSettings {
    RepoSettings {
        owner: OWNER,
        enterprise: Some(ENTERPRISE),
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

/// The root `REPO` is forked from, and so its network.
const ROOT: RepoId = RepoId(DbId(5));
const NETWORK: NetworkId = NetworkId(DbId(5));
const CUT: Index = Index(42);

fn rf(name: &str) -> RefName {
    RefName::parse(name.as_bytes()).unwrap()
}

fn fork_op(refs: Vec<(RefName, ObjectId)>) -> LifecycleOp {
    LifecycleOp::Fork {
        settings: settings("a fork"),
        name: RepoName::parse("tool").unwrap(),
        network: NETWORK,
        parent: ROOT,
        cut: CUT,
        refs,
    }
}

fn fork_refs() -> Vec<(RefName, ObjectId)> {
    vec![(rf("refs/heads/main"), ObjectId([1; 20])), (rf("refs/tags/v1"), ObjectId([2; 20]))]
}

fn init(g: &mut G, settings: RepoSettings) -> Result<LifecycleOut, LifecycleErr> {
    g.lifecycle(LifecycleOp::Init(settings))
}

fn delete(g: &mut G, retain_until: Timestamp) -> Result<LifecycleOut, LifecycleErr> {
    g.lifecycle(LifecycleOp::Delete { retain_until })
}

#[test]
fn init_commits_settings_and_created_once() {
    let mut g = G::new();
    let wanted = settings("from the create commit point");
    assert_eq!(init(&mut g, wanted.clone()), Ok(LifecycleOut { changed: true }));
    assert_eq!(init(&mut g, wanted.clone()), Ok(LifecycleOut { changed: false }));
    g.pass(4_000);
    for site in SITES {
        assert!(g.created(site).is_some(), "{site:?} has the Created row");
        assert_eq!(g.settings(site), Some(wanted.clone()), "the no-op keeps the first settings");
    }
}

#[test]
fn a_forks_first_touch_writes_genesis_settings_created_and_the_parents_refs() {
    let mut reads = formats();
    reads.push(REPO_FORMAT_FORKS);
    let mut g = G::with_reads(reads);
    assert_eq!(g.lifecycle(fork_op(fork_refs())), Ok(LifecycleOut { changed: true }));
    g.pass(4_000);
    for site in SITES {
        let scratch: RepoScratch<'_> = g.m(site).group_fold().fold.scratch();
        let core = scratch.core();
        let row = graftyard_git::tiers::fork(core);
        assert_eq!(row, Some(ForkRow { network: NETWORK, fork_of: Some((ROOT, CUT)) }), "{site:?}");
        assert_eq!(graftyard_git::tiers::network(core, REPO), NETWORK);
        assert!(created(core).is_some(), "{site:?} has the Created row");
        assert_eq!(get_rec::<RepoSettings>(core, "settings", &[]), Some(settings("a fork")));
        for (name, oid) in fork_refs() {
            assert_eq!(scratch.get(&name), Some(oid), "{site:?} reads {name:?}");
        }
    }
    assert_eq!(g.lifecycle(fork_op(fork_refs())), Ok(LifecycleOut { changed: false }), "a re-sent first touch");
}

#[test]
fn a_fork_first_touch_is_refused_off_its_rules() {
    let mut reads = formats();
    reads.push(REPO_FORMAT_FORKS);
    let mut g = G::with_reads(reads);
    let pull = vec![(rf("refs/pull/1/head"), ObjectId([3; 20]))];
    assert_eq!(g.lifecycle(fork_op(pull)), Err(LifecycleErr::Invalid));
    let twice = vec![(rf("refs/heads/main"), ObjectId([3; 20])), (rf("refs/heads/main"), ObjectId([4; 20]))];
    assert_eq!(g.lifecycle(fork_op(twice)), Err(LifecycleErr::Invalid), "a ref named twice");
    let LifecycleOp::Fork { settings, name, parent, cut, refs, .. } = fork_op(fork_refs()) else { unreachable!() };
    let own = LifecycleOp::Fork { settings, name, network: NetworkId::of_root(REPO), parent, cut, refs };
    assert_eq!(g.lifecycle(own), Err(LifecycleErr::Invalid), "a fork's network is never its own");
    let many: Vec<(RefName, ObjectId)> =
        (0..=FORK_REFS_MAX).map(|i| (rf(&format!("refs/heads/b{i}")), ObjectId([4; 20]))).collect();
    assert_eq!(g.lifecycle(fork_op(many)), Err(LifecycleErr::Invalid));
    g.deps.name(None);
    assert_eq!(g.lifecycle(fork_op(fork_refs())), Err(LifecycleErr::NotLive));
    for site in SITES {
        assert_eq!(g.created(site), None);
        assert_eq!(g.settings(site), None);
    }
}

#[test]
fn init_refuses_when_not_live() {
    for entry in [None, Some(RepoEntry::Live { repo: OTHER_REPO, epoch: NameEpoch(2) })] {
        let mut g = G::new();
        g.deps.name(entry);
        assert_eq!(init(&mut g, settings("after abort")), Err(LifecycleErr::NotLive));
        for site in SITES {
            assert_eq!(g.created(site), None);
            assert_eq!(g.settings(site), None);
        }
    }
}

#[test]
fn init_refuses_wrong_owner_or_enterprise() {
    let mut g = G::new();
    let cases = [
        RepoSettings { owner: OTHER_OWNER, ..settings("another owner") },
        RepoSettings { enterprise: Some(OTHER_ENTERPRISE), ..settings("another enterprise") },
    ];
    for wanted in cases {
        assert_eq!(init(&mut g, wanted), Err(LifecycleErr::WrongOwner));
    }
    for site in SITES {
        assert_eq!(g.created(site), None);
        assert_eq!(g.settings(site), None);
    }
}

#[test]
fn init_refuses_without_the_authz_dep() {
    let mut g = G::new();
    g.deps.read_authz = false;
    assert_eq!(init(&mut g, settings("no authorization read")), Err(LifecycleErr::NotRead));
    for site in SITES {
        assert_eq!(g.created(site), None);
        assert_eq!(g.settings(site), None);
    }
}

#[test]
fn init_after_delete_is_gone() {
    let mut g = G::new();
    let retain_until = Timestamp(g.now.0 + DAY);
    g.deps.name(Some(RepoEntry::Deleted { repo: REPO, retain_until }));
    assert_eq!(delete(&mut g, retain_until), Ok(LifecycleOut { changed: true }));
    assert_eq!(init(&mut g, settings("too late")), Err(LifecycleErr::Gone));
}

#[test]
fn init_after_an_edge_init_repo_keeps_the_edge_settings() {
    let mut g = G::new();
    assert!(g.issue(IssueOp::InitRepo(settings("edge"))).is_ok());
    assert_eq!(init(&mut g, settings("finisher")), Ok(LifecycleOut { changed: false }));
    for site in SITES {
        assert_eq!(g.settings(site).and_then(|settings| settings.description), Some("edge".into()));
    }
}

#[test]
fn delete_needs_the_deleted_name_and_matching_retain_until() {
    let mut g = G::new();
    assert_eq!(init(&mut g, settings("created")), Ok(LifecycleOut { changed: true }));

    let retain_until = Timestamp(g.now.0 + 90 * DAY);
    assert_eq!(delete(&mut g, retain_until), Err(LifecycleErr::NotDeleted));

    g.deps.name(Some(RepoEntry::Deleted { repo: REPO, retain_until }));
    assert_eq!(delete(&mut g, Timestamp(retain_until.0 + 1)), Err(LifecycleErr::NotDeleted));

    let at_entry = g.now;
    g.deps.name(Some(RepoEntry::Deleted { repo: REPO, retain_until: at_entry }));
    assert_eq!(delete(&mut g, at_entry), Err(LifecycleErr::Invalid));

    g.deps.name(Some(RepoEntry::Deleted { repo: REPO, retain_until }));
    assert_eq!(delete(&mut g, retain_until), Ok(LifecycleOut { changed: true }));
    assert_eq!(delete(&mut g, retain_until), Ok(LifecycleOut { changed: false }));
    g.pass(4_000);
    for site in SITES {
        assert_eq!(g.deleted(site), Some((retain_until, None)), "{site:?} has the finisher's tombstone");
    }

    let open = IssueOp::OpenIssue {
        title: "after delete".into(),
        body: "".into(),
        labels: SmallVec::new(),
        assignees: SmallVec::new(),
    };
    assert!(matches!(g.issue(open), Err(RepoErr::Issue { err: IssueErr::NotFound, .. })));
}

#[test]
fn lifecycle_wire_shape() {
    let request = RequestId(Ulid(0x59));
    let cmd = RepoCmd::Lifecycle(Lifecycle { op: LifecycleOp::Init(settings("wire")), request });
    assert_eq!(cmd.format(), REPO_FORMAT_LIFECYCLE);
    assert_eq!((REPO_FORMAT_LIFECYCLE.family, REPO_FORMAT_LIFECYCLE.version), (32, 17));

    let value = serde_json::to_value(&cmd).unwrap();
    assert_eq!(value["lifecycle"]["format"], serde_json::json!([32, 17]));
    assert_eq!(serde_json::from_value::<RepoCmd>(value.clone()).unwrap(), cmd);
    let mut wrong = value;
    wrong["lifecycle"]["format"] = serde_json::json!([32, 16]);
    assert!(serde_json::from_value::<RepoCmd>(wrong).is_err());

    let mut encoded = Vec::new();
    cmd.encode(&mut encoded);
    assert_eq!(encoded.first(), Some(&0x4c));

    let mut old = G::with_reads(vec![REPO_FORMAT_V1, REPO_FORMAT_ISSUES, REPO_FORMAT_REPOS, REPO_FORMAT_LEASES]);
    let rid = old.rid();
    let result = old.on(SITES[0], |member, now| member.commit(now, rid, cmd));
    assert!(matches!(result, Err(CallError::Unsupported(unsupported)) if unsupported.format == REPO_FORMAT_LIFECYCLE));
    assert_eq!(old.created(SITES[0]), None);

    let fork = RepoCmd::Lifecycle(Lifecycle { op: fork_op(fork_refs()), request });
    assert_eq!(fork.format(), REPO_FORMAT_FORKS);
    let value = serde_json::to_value(&fork).unwrap();
    assert_eq!(value["lifecycle"]["format"], serde_json::json!([32, 34]));
    assert_eq!(serde_json::from_value::<RepoCmd>(value.clone()).unwrap(), fork);
    let mut labelled = value;
    labelled["lifecycle"]["format"] = serde_json::json!([32, 17]);
    assert!(serde_json::from_value::<RepoCmd>(labelled).is_err(), "a Fork is never read as 32.17");

    use graftyard_identity::finish::RepoStep;
    let json_settings = RepoSettings { owner: OTHER_OWNER, enterprise: Some(OTHER_ENTERPRISE), ..settings("a fork") };
    let init = serde_json::to_string(&json_settings).unwrap().into_boxed_str();
    let step = RepoStep::Fork {
        owner: OWNER,
        enterprise: Some(ENTERPRISE),
        init: init.clone(),
        name: RepoName::parse("tool").unwrap(),
        network: NETWORK,
        parent: ROOT,
    };
    let mapped = fork_of_step(&step, CUT, fork_refs(), request).expect("Fork maps").expect("valid settings JSON");
    assert_eq!(mapped, Lifecycle { op: fork_op(fork_refs()), request });
    assert!(of_step(&step, request).is_none(), "of_step has no parent refs");
    let step = RepoStep::Init { owner: OWNER, enterprise: Some(ENTERPRISE), init };
    assert!(fork_of_step(&step, CUT, Vec::new(), request).is_none());
}

#[test]
fn of_step_maps_init_and_delete_only() {
    use graftyard_identity::finish::RepoStep;

    let request = RequestId(Ulid(0x5959));
    let json_settings = RepoSettings { owner: OTHER_OWNER, enterprise: Some(OTHER_ENTERPRISE), ..settings("step") };
    let init = serde_json::to_string(&json_settings).unwrap();
    let step = RepoStep::Init { owner: OWNER, enterprise: Some(ENTERPRISE), init: init.into_boxed_str() };
    let command = of_step(&step, request).expect("Init maps").expect("valid settings JSON");
    assert_eq!(command.request, request);
    let expected = RepoSettings { owner: OWNER, enterprise: Some(ENTERPRISE), ..json_settings };
    assert_eq!(command.op, LifecycleOp::Init(expected));

    let retain_until = Timestamp(123_456);
    let step = RepoStep::Delete { retain_until };
    let command = of_step(&step, request).expect("Delete maps").unwrap();
    assert_eq!(command, Lifecycle { op: LifecycleOp::Delete { retain_until }, request });

    let movement = RepoStep::Move { to: placement() };
    assert!(of_step(&movement, request).is_none());
    let transfer = RepoStep::Settings { owner: OTHER_OWNER, enterprise: Some(OTHER_ENTERPRISE) };
    assert!(of_step(&transfer, request).is_none());
}

#[test]
fn edge_init_repo_after_abort_is_refused() {
    let mut g = G::new();
    g.deps.name(None);
    assert!(matches!(
        g.issue(IssueOp::InitRepo(settings("late edge create"))),
        Err(RepoErr::Issue { err: IssueErr::NotFound, .. })
    ));
    for site in SITES {
        assert_eq!(g.created(site), None);
        assert_eq!(g.settings(site), None);
    }
}

/// `repo-purge`'s gate on real members: a voter that was away while the tombstone committed is joined but behind it,
/// so its own fold answers `purgeable = false`, and the purge refuses though the other two hold the tombstone. Once it
/// catches up, the gate passes.
#[test]
fn a_voter_behind_the_tombstone_is_not_purgeable_and_the_purge_refuses() {
    use graftyard_git::purge::{Facts, Refused, Status, gate, purgeable};
    let mut g = G::new();
    assert_eq!(init(&mut g, settings("created")), Ok(LifecycleOut { changed: true }));
    let retain_until = Timestamp(g.now.0 + 90 * DAY);
    g.deps.name(Some(RepoEntry::Deleted { repo: REPO, retain_until }));
    let away = g.members[2].take();
    assert_eq!(delete(&mut g, retain_until), Ok(LifecycleOut { changed: true }));
    // Site 1 learns that commit from the leader's next append: the same delete again, which changes nothing.
    assert_eq!(delete(&mut g, retain_until), Ok(LifecycleOut { changed: false }));
    g.members[2] = away;
    let check = |g: &G| {
        let members: Vec<(ChainId, SiteId, Box<str>, Status)> = SITES
            .iter()
            .map(|s| {
                let st = Status::Joined {
                    commit: 0,
                    purgeable: purgeable(&g.m(*s).group_fold().fold),
                    changing: false,
                    config: None,
                };
                (ChainId::Repo(REPO), *s, format!("repo:7#1#f{}", s.0).into_boxed_str(), st)
            })
            .collect();
        gate(&Facts { deleted: Some(retain_until), now: g.now, early: true, members: &members }).map(|_| ())
    };
    assert!(g.deleted(SITES[0]).is_some() && g.deleted(SITES[1]).is_some());
    assert_eq!(g.deleted(SITES[2]), None, "the voter that was away is joined but behind the tombstone");
    assert!(!purgeable(&g.m(SITES[2]).group_fold().fold));
    assert_eq!(check(&g), Err(Refused::NoTombstone { chain: ChainId::Repo(REPO), member: "repo:7#1#f2".into() }));
    g.pass(4_000);
    assert!(g.deleted(SITES[2]).is_some(), "caught up");
    assert_eq!(check(&g), Ok(()));
}

/// `purge-outside` (R27) on real members, graft-m4a's shape: site 1's first-generation voter is away while the group
/// replaces it through `Degraded { lost: 1 }` and `Normal` with `#4#f1`, then the repository is deleted. The stale
/// voter keeps its old config and never holds the tombstone, which refused every try on main; it is outside the
/// newest committed config, so the purge passes and retires it first.
#[test]
fn a_voter_a_replacement_removed_does_not_block_the_purge() {
    use graftyard_git::purge::{Facts, Refused, Status, changing, gate, purgeable};
    use graftyard_raft::GroupConfig;
    use graftyard_types::{Member as Ref, MemberKind, Placement};
    let chain = ChainId::Repo(REPO);
    let placement = |v1: u32| -> Box<Placement> {
        let m = |s: u8, g: u32| Ref::new(chain, SiteId(s), MemberKind::Full, s, g);
        let raw =
            serde_json::json!({ "voters": [m(0, 1), m(1, v1), m(2, 1)], "learners": [], "home": 0, "generation": v1 });
        Box::new(serde_json::from_value(raw).unwrap())
    };
    let mut g = G::new();
    assert_eq!(init(&mut g, settings("created")), Ok(LifecycleOut { changed: true }));
    let stale = g.members[1].take().expect("site 1 is up");
    let degraded = GroupConfig::Degraded { placement: placement(1), lost: SITES[1] };
    g.on(SITES[0], |m, now| m.reconfigure(now, degraded)).expect("degraded");
    g.pass(2_000);
    g.boots += 1;
    g.disks[1] = Stored::new(RepoFold::new(REPO, OWNER));
    let (fresh, ready) = Member::restore(config(SITES[1], &formats()), g.disks[1].clone(), g.now, g.boots).unwrap();
    g.members[1] = Some(fresh);
    let ((), out) = ready.persist(&mut g.disks[1]);
    g.inbox.extend(out.early.into_iter().chain(out.messages));
    let index = g.on(SITES[0], |m, now| m.reconfigure(now, GroupConfig::Normal(placement(4)))).expect("normal");
    g.pass(4_000);
    assert!(g.m(SITES[0]).committed_config().is_some_and(|c| c.0 >= index), "the replacement committed");
    let retain_until = Timestamp(g.now.0 + 90 * DAY);
    g.deps.name(Some(RepoEntry::Deleted { repo: REPO, retain_until }));
    assert_eq!(delete(&mut g, retain_until), Ok(LifecycleOut { changed: true }));
    // The followers learn that commit from the leader's next append: the same delete again, which changes nothing.
    assert_eq!(delete(&mut g, retain_until), Ok(LifecycleOut { changed: false }));
    assert!(SITES.iter().all(|s| g.deleted(*s).is_some()), "the config's voters hold the tombstone");
    let status = |m: &Member<RepoCmd>| Status::Joined {
        commit: 0,
        purgeable: purgeable(&m.group_fold().fold),
        changing: changing(m.committed_config().map(|c| &c.1), m.latest_config(), false),
        config: m.committed_config().map(|(i, c)| (i.0, c.clone())),
    };
    let named = |name: &str, s: Status| (chain, SiteId(0), Box::<str>::from(name), s);
    let members = [
        named("repo:7#1#f0", status(g.m(SITES[0]))),
        named("repo:7#1#f1", status(&stale)),
        named("repo:7#1#f2", status(g.m(SITES[2]))),
        named("repo:7#4#f1", status(g.m(SITES[1]))),
    ];
    assert!(!purgeable(&stale.group_fold().fold), "the stale voter never got the tombstone");
    let order = gate(&Facts { deleted: Some(retain_until), now: g.now, early: true, members: &members })
        .map(|o| o.into_iter().map(|r| (members[r.at].2.to_string(), r.outside.is_some())).collect::<Vec<_>>());
    let first = order.as_ref().map(|o| o[0].clone());
    assert_eq!(first, Ok(("repo:7#1#f1".to_string(), true)), "{order:?}");
    // The replacement behind the tombstone still refuses: it is in the config.
    let mut behind = members.clone();
    behind[3].3 = Status::Joined {
        commit: 0,
        purgeable: false,
        changing: false,
        config: g.m(SITES[1]).committed_config().map(|(i, c)| (i.0, c.clone())),
    };
    let r = gate(&Facts { deleted: Some(retain_until), now: g.now, early: true, members: &behind }).map(|_| ());
    assert_eq!(r, Err(Refused::NoTombstone { chain, member: "repo:7#4#f1".into() }));
}
