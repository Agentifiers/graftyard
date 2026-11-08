//! AM1-WP2's gates in memory, on real members: three `graftyard_raft::Member<AuthzCmd>` of the install's own
//! `Authz(Genesis.account)` chain, and three `Member<RepoCmd>` of `repo:7`, whose leader answers every deps request by
//! reading the install's chain through `fresh` on its own leader, as a site host does once per entry.
//!
//! - LF-2: after `DeleteAgentApp` is acknowledged, the next repository decision no longer classes the App an `Agent`.
//! - B0-36 (pure rerun): each refusal for a Graftyard-run App and a confined vendor App; an unflagged or unconfined App
//!   is not refused; `Co-authored-by` alone never classes; `AgentGate` counts; a default PR merges as before.
//! - B0-37 (pure rerun): 10,000 pushes with malformed trailers and notes, forged sessions, and `refs/graftyard/` writes.

use graftyard_git::agent::{self, AgentPolicy, AgentRule, ChangedPaths};
use graftyard_git::issue_ops::call::IssueCall;
use graftyard_git::issue_ops::{IssueErr, IssueOp, IssueOut};
use graftyard_git::issues::{
    AppId, CheckRunId, CheckRunUpdate, CheckStatus, CheckSuiteId, HeadSpec, IssueChange, IssueNumber, LabelId,
    LabelName, MergeMethod, Protection, Pusher, RepoSettings, ReviewState, StatusId, StatusState, StatusUpdate,
    Visibility,
};
use graftyard_git::merge::{EngineVersion, Merge, MergeCmd, MergeError, MergePlan, TreeDiffDigest};
use graftyard_git::merge_call::MergeCall;
use graftyard_git::pack::PackRef;
use graftyard_git::refs::{Atomicity, ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{
    PushErr, REPO_FORMAT_AGENT, REPO_FORMAT_STATELESS, REPO_FORMAT_V1, REPO_FORMAT_V2, RefOutcome, RejectReason,
    RepoCmd, RepoErr, RepoEvent, RepoFold, RepoOut,
};
use graftyard_git::{DurablePack, tiers};
use graftyard_identity::glob::RefPattern;
use graftyard_identity::stateless::InstallationClaims;
use graftyard_identity::{
    AUTHZ_FORMAT, AUTHZ_FORMAT_V2, AUTHZ_FORMAT_V3, Access, AccountKind, Actor, AgentApp, AgentKind, AuthzCmd,
    AuthzEvent, AuthzFold, AuthzOp, Credential, IdentityDep, InstallationId, InstallationRow, Jti, Kid, NamesEvent,
    NamesFold, OrgRole, PermissionSet, Principal, RepoPermission, RepoScope, ScopeSet, TokenFields, TokenKind,
    TokensEvent, TokensFold, UserId, install_account,
};
use graftyard_raft::{
    Command, CommitError, Envelope, Ev, Fold as _, Format, Member, MemberConfig, ReleaseTag, RequestId, Resolution,
    Role, Stored, Ulid,
};
use graftyard_types::{
    AccountId, ChainId, CredentialShard, DbId, Digest, NameShard, PlacementName, PlacementRecord, Region, ReleaseId,
    RepoId, SiteId, Timestamp, TokenShard,
};
use smallvec::{SmallVec, smallvec};

const SITES: [SiteId; 3] = [SiteId(0), SiteId(1), SiteId(2)];
const REPO: RepoId = RepoId(DbId(7));
const OWNER: i64 = 100;
const WRITER: i64 = 101;
const WRITER2: i64 = 102;
const READER: i64 = 103;
/// The install org's admin, who flags Apps.
const SITE_ADMIN: i64 = 104;
/// A user who signed in to `CLAUDE` and holds its user-to-server token.
const VIA_APP: i64 = 105;
const CLAUDE: AppId = AppId(DbId(3));
const RUN: AppId = AppId(DbId(4));
const PLAIN: AppId = AppId(DbId(5));
const MCP: AppId = AppId(DbId(6));

// ---- fixtures ----

fn name(s: &str) -> RefName {
    RefName::parse(s.as_bytes()).unwrap()
}

fn oid(n: u32) -> ObjectId {
    let mut o = [0u8; 20];
    o[..4].copy_from_slice(&n.to_be_bytes());
    o[19] = 7;
    ObjectId(o)
}

fn user(a: i64) -> Principal {
    Principal {
        actor: Actor::User(UserId(AccountId(DbId(a)))),
        scopes: ScopeSet(1),
        credential: Credential::Stored([a as u8; 32]),
        sso_orgs: SmallVec::new(),
    }
}

fn claims(app: AppId) -> InstallationClaims {
    InstallationClaims {
        app,
        installation: InstallationId(DbId(900 + app.0.0)),
        repos: RepoScope::Selected(smallvec![REPO]),
        permissions: everything(),
        jti: Jti(Ulid(0x0193_0000_0000_0000_0000_0000_0000_0000 + app.0.0 as u128)),
        iat: Timestamp(9_000),
        exp: Timestamp(9_000 + 3_600_000),
        kid: Kid(1),
    }
}

/// An installation widened to every permission, as B0-36 runs the Graftyard-run token: a refusal must come from the
/// agent rule, not from a missing permission.
fn everything() -> PermissionSet {
    let names = ["contents", "issues", "pull_requests", "checks", "statuses", "administration", "metadata", "actions"];
    PermissionSet(names.iter().map(|n| (Box::from(*n), Access::Write)).collect())
}

fn app(a: AppId) -> Principal {
    Principal::installation(&claims(a))
}

fn claude_row() -> AgentApp {
    AgentApp {
        app: CLAUDE,
        kind: AgentKind::ThirdParty,
        confine: Some(RefPattern::parse("refs/heads/claude/**").unwrap()),
    }
}

fn record() -> PlacementRecord {
    PlacementRecord {
        name: PlacementName::Tri,
        sites: SITES.iter().copied().collect(),
        home: SITES[0],
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    }
}

fn config(chain: ChainId, me: SiteId, reads: Vec<Format>) -> MemberConfig {
    MemberConfig {
        chain,
        me,
        record: record(),
        regions: vec![(SITES[0], Region::Enam), (SITES[1], Region::Weur), (SITES[2], Region::Apac)],
        release: ReleaseTag { id: ReleaseId([9; 32]), seq: 1 },
        reads,
    }
}

/// The folds a repository leader reads besides the install's: accounts, tokens (a user-to-server token for
/// `VIA_APP`), and the owner's `Authz` with two writers, a reader, and every App installed on the repository.
struct Ident {
    names: NamesFold,
    tokens: TokensFold,
    owner: AuthzFold,
}

impl Ident {
    fn new() -> Ident {
        let mut names = NamesFold::new(NameShard(0));
        let mut tokens = TokensFold::new(TokenShard(0));
        let mut owner = AuthzFold::new(AccountId(DbId(OWNER)));
        owner.apply(&AuthzEvent::Opened { kind: AccountKind::User, enterprise: None });
        for a in [OWNER, WRITER, WRITER2, READER, SITE_ADMIN, VIA_APP] {
            let id = AccountId(DbId(a));
            names.apply(&NamesEvent::Account { id, suspended: false, sessions_not_before: Timestamp(0) });
            let (hash, kind, account, scopes, expires) = ([a as u8; 32], TokenKind::Pat, id, ScopeSet(1), None);
            if a == VIA_APP {
                let fields = Box::new(TokenFields { app: Some(CLAUDE), ..TokenFields::default() });
                let kind = TokenKind::UserToServer;
                tokens.apply(&TokensEvent::StoredWith { hash, kind, account, scopes, expires, fields });
            } else {
                tokens.apply(&TokensEvent::Stored { hash, kind, account, scopes, expires });
            }
        }
        for (a, p) in
            [(WRITER, RepoPermission::Write), (WRITER2, RepoPermission::Write), (VIA_APP, RepoPermission::Write)]
        {
            owner.apply(&AuthzEvent::Grant { repo: REPO, grantee: UserId(AccountId(DbId(a))), permission: Some(p) });
        }
        owner.apply(&AuthzEvent::Grant {
            repo: REPO,
            grantee: UserId(AccountId(DbId(READER))),
            permission: Some(RepoPermission::Read),
        });
        for a in [CLAUDE, RUN, PLAIN, MCP] {
            let c = claims(a);
            let row = InstallationRow { app: a, repos: c.repos.clone(), permissions: everything() };
            owner.apply(&AuthzEvent::Installation { id: c.installation, row: Some(row), not_before: Timestamp(0) });
        }
        Ident { names, tokens, owner }
    }

    fn dep(&self, c: ChainId, install: &AuthzFold) -> IdentityDep {
        match c {
            ChainId::Names(_) => IdentityDep::Names(self.names.clone()),
            ChainId::Tokens(s) => {
                let mut t = self.tokens.clone();
                t.shard = Some(s);
                IdentityDep::Tokens(t)
            }
            ChainId::Authz(o) if o == install_account() => IdentityDep::Authz(install.clone()),
            ChainId::Authz(o) if o == AccountId(DbId(OWNER)) => IdentityDep::Authz(self.owner.clone()),
            ChainId::Authz(o) => IdentityDep::Authz(AuthzFold::new(o)),
            other => panic!("unexpected dep {other}"),
        }
    }
}

// ---- three members of one chain ----

struct Group<C: Command> {
    members: Vec<Member<C>>,
    disks: Vec<Stored<C::Fold>>,
    inbox: Vec<Envelope<Ev<C>>>,
    resolutions: Vec<Resolution<C>>,
    deps: Vec<(SiteId, graftyard_raft::DepsWanted)>,
}

impl<C: Command> Group<C> {
    fn new(chain: ChainId, reads: Vec<Format>, fold: C::Fold, now: Timestamp) -> Group<C> {
        let mut g = Group {
            members: Vec::new(),
            disks: Vec::new(),
            inbox: Vec::new(),
            resolutions: Vec::new(),
            deps: Vec::new(),
        };
        for (i, s) in SITES.iter().enumerate() {
            let (m, ready) =
                Member::restore(config(chain, *s, reads.clone()), Stored::new(fold.clone()), now, i as u64 + 1)
                    .unwrap();
            let mut disk = Stored::new(fold.clone());
            let ((), out) = ready.persist(&mut disk);
            g.inbox.extend(out.early.into_iter().chain(out.messages));
            g.members.push(m);
            g.disks.push(disk);
        }
        g
    }

    fn on<R>(
        &mut self,
        s: SiteId,
        now: Timestamp,
        f: impl FnOnce(&mut Member<C>, Timestamp) -> graftyard_raft::Ready<C, R>,
    ) -> R {
        let i = s.0 as usize;
        let (r, mut out) = f(&mut self.members[i], now).persist(&mut self.disks[i]);
        loop {
            self.inbox.extend(out.early.drain(..).chain(out.messages.drain(..)));
            self.resolutions.append(&mut out.resolutions);
            // A follower's refusal (`OwnDefect`, `Refused`) means an entry `decide` built does not verify.
            assert!(out.alarms.is_empty(), "alarm on {s:?}: {:?}", out.alarms);
            self.deps.extend(out.deps.drain(..).map(|d| (s, d)));
            let Some(ack) = out.self_ack.take() else { break };
            out = self.members[i].self_ack(now, ack).persist(&mut self.disks[i]).1;
        }
        r
    }

    fn deliver(&mut self, now: Timestamp) {
        for _ in 0..10_000 {
            if self.inbox.is_empty() {
                return;
            }
            for Envelope { from, to, msg } in std::mem::take(&mut self.inbox) {
                self.on(to, now, |m, t| m.step(t, from, msg));
            }
        }
        panic!("messages never settled");
    }

    fn leader(&self) -> SiteId {
        *SITES
            .iter()
            .filter(|s| self.members[s.0 as usize].role() == Role::Leader)
            .max_by_key(|s| self.members[s.0 as usize].term())
            .expect("a leader")
    }

    fn elect(&mut self, now: Timestamp) {
        for _ in 0..3 {
            self.on(SITES[0], now, |m, t| m.campaign(t));
            self.deliver(now);
            if self.members[0].role() == Role::Leader {
                return;
            }
        }
        panic!("no leader");
    }

    /// The leader's fold through `fresh`: a read grant, then the view it allows.
    fn fresh(&mut self, now: Timestamp) -> C::Fold {
        // A read past the lease needs a round, and the acks after the one that granted it are later inputs, which void
        // the grant: the round renewed the lease, so a second read is granted at once.
        let l = self.leader();
        for _ in 0..3 {
            let id = self.on(l, now, |m, t| m.read(t)).expect("leader reads");
            self.deliver(now);
            let grant = self
                .resolutions
                .iter()
                .rev()
                .find_map(|r| match r {
                    Resolution::Read { id: i, result } if *i == id => Some(result.clone().expect("granted")),
                    _ => None,
                })
                .expect("read resolved");
            if let Ok(f) = self.members[l.0 as usize].fresh(&grant) {
                return f.fold().clone();
            }
        }
        panic!("no fresh read granted three times");
    }
}

/// The install's group and the repository's group, with time.
struct World {
    now: Timestamp,
    install: Group<AuthzCmd>,
    repo: Group<RepoCmd>,
    ident: Ident,
    next: u128,
    /// The planted-bug hook for LF-2's stale class: the leader's deps from a copy of the install fold taken once.
    pinned: Option<AuthzFold>,
}

type RepoOutcome = Result<RepoOut, RepoErr>;

impl World {
    fn new() -> World {
        let now = Timestamp(10_000);
        let install = install_account();
        let mut iv = Group::new(
            ChainId::Authz(install),
            vec![AUTHZ_FORMAT, AUTHZ_FORMAT_V2, AUTHZ_FORMAT_V3],
            AuthzFold::new(install),
            now,
        );
        iv.elect(now);
        let reads = vec![
            REPO_FORMAT_V1,
            REPO_FORMAT_V2,
            graftyard_git::prsync::REPO_FORMAT_V3,
            graftyard_git::merge::REPO_FORMAT_V4,
            graftyard_git::issue_ops::call::REPO_FORMAT_ISSUES,
            graftyard_git::issue_ops::call::REPO_FORMAT_COLLABORATORS,
            REPO_FORMAT_STATELESS,
            REPO_FORMAT_AGENT,
            graftyard_git::issue_ops::call::REPO_FORMAT_APP_CALLS,
        ];
        let mut rg = Group::new(ChainId::Repo(REPO), reads, RepoFold::new(REPO, AccountId(DbId(OWNER))), now);
        rg.elect(now);
        let mut w = World { now, install: iv, repo: rg, ident: Ident::new(), next: 1, pinned: None };
        // The install is an org whose admin flags Apps; ids for its rows; the repository's ids and its default branch.
        w.authz(AuthzOp::Open { kind: AccountKind::Org, enterprise: None }, CredentialShard::NONE).unwrap();
        let op = AuthzOp::SetMember { user: UserId(AccountId(DbId(SITE_ADMIN))), role: Some(OrgRole::Admin) };
        w.authz(op, CredentialShard::NONE).unwrap();
        let l = w.repo.leader();
        let now = w.now;
        w.repo.on(l, now, |m, t| m.id_block(t, graftyard_types::IdBlock::whole(0, 3).unwrap())).unwrap();
        w.settle();
        w.record(vec![
            RepoEvent::Settings(settings()),
            RepoEvent::Refs {
                push: RequestId(Ulid(1)),
                actor: None,
                updates: vec![up("refs/heads/main", None, Some(oid(1)))],
                pack: None,
            },
        ]);
        w
    }

    fn rid(&mut self) -> RequestId {
        self.next += 1;
        RequestId(Ulid(self.next << 40))
    }

    /// Delivers both groups and answers the repository leader's deps from the install leader's `fresh` read.
    fn settle(&mut self) {
        for _ in 0..200 {
            self.install_deps();
            self.install.deliver(self.now);
            self.repo.deliver(self.now);
            if self.repo.deps.is_empty() {
                self.now = Timestamp(self.now.0 + graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
                for s in SITES {
                    let now = self.now;
                    self.install.on(s, now, |m, t| m.tick(t));
                    self.repo.on(s, now, |m, t| m.tick(t));
                }
                self.install.deliver(self.now);
                self.repo.deliver(self.now);
                if self.repo.deps.is_empty() && self.repo.inbox.is_empty() && self.install.inbox.is_empty() {
                    return;
                }
                continue;
            }
            self.install_deps();
            let install = match &self.pinned {
                Some(f) => f.clone(),
                None => self.install.fresh(self.now),
            };
            for (s, want) in std::mem::take(&mut self.repo.deps) {
                let deps = want.chains.iter().map(|c| (*c, self.ident.dep(*c, &install))).collect();
                let now = self.now;
                self.repo.on(s, now, |m, t| m.deps_read(t, want.id, deps));
            }
        }
        panic!("never settled");
    }

    /// The install's chain asks for its own deps (the admin's `Account` row): answers them from `Ident`.
    fn install_deps(&mut self) {
        for _ in 0..10 {
            if self.install.deps.is_empty() {
                return;
            }
            let empty = AuthzFold::new(install_account());
            for (s, want) in std::mem::take(&mut self.install.deps) {
                let deps = want.chains.iter().map(|c| (*c, self.ident.dep(*c, &empty))).collect();
                let now = self.now;
                self.install.on(s, now, |m, t| m.deps_read(t, want.id, deps));
            }
            self.install.deliver(self.now);
        }
    }

    fn authz(&mut self, op: AuthzOp, cred: CredentialShard) -> Result<(), String> {
        let rid = self.rid();
        let l = self.install.leader();
        let now = self.now;
        let a =
            self.install.on(l, now, |m, t| m.commit(t, rid, AuthzCmd { cred, op })).map_err(|e| format!("{e:?}"))?;
        self.settle();
        match self.install.resolutions.iter().rev().find_map(|r| match r {
            Resolution::Ticket { ticket, result } if *ticket == a.ticket => Some(result.clone()),
            _ => None,
        }) {
            Some(Ok(_)) => Ok(()),
            other => Err(format!("{other:?}")),
        }
    }

    fn flag(&mut self, row: AgentApp) {
        self.authz(AuthzOp::PutAgentApp(row), admin()).expect("flagged");
    }

    fn unflag(&mut self, app: AppId) {
        self.authz(AuthzOp::DeleteAgentApp { app }, admin()).expect("unflagged");
    }

    fn record(&mut self, events: Vec<RepoEvent>) {
        let rid = self.rid();
        let l = self.repo.leader();
        let now = self.now;
        self.repo.on(l, now, |m, t| m.record(t, rid, REPO_FORMAT_V1, events)).unwrap();
        self.settle();
        assert!(
            self.repo.resolutions.iter().any(|r| matches!(r, Resolution::Recorded { id, result: Ok(_) } if *id == rid))
        );
    }

    /// Commits `cmds` on the repository leader as one entry, as one `/commit_many` does, and returns each outcome.
    fn commit_all(&mut self, cmds: Vec<RepoCmd>) -> Vec<RepoOutcome> {
        let l = self.repo.leader();
        let now = self.now;
        let tickets: Vec<_> = cmds
            .into_iter()
            .map(|c| {
                let rid = self.rid();
                self.repo.on(l, now, |m, t| m.commit(t, rid, c)).expect("admitted").ticket
            })
            .collect();
        self.settle();
        tickets
            .iter()
            .map(|t| {
                match self.repo.resolutions.iter().rev().find_map(|r| match r {
                    Resolution::Ticket { ticket, result } if ticket == t => Some(result.clone()),
                    _ => None,
                }) {
                    Some(Ok(a)) => Ok(a.out),
                    Some(Err(CommitError::Refused { err, .. })) => Err(err),
                    other => panic!("unresolved: {other:?}"),
                }
            })
            .collect()
    }

    fn commit(&mut self, c: RepoCmd) -> RepoOutcome {
        self.commit_all(vec![c]).pop().unwrap()
    }

    fn fold(&mut self) -> RepoFold {
        self.repo.fresh(self.now)
    }

    fn get(&mut self, r: &str) -> Option<ObjectId> {
        let f = self.fold();
        tiers::get_rec(&*f.core, "ref", &[tiers::ref_key(&name(r))])
    }

    fn set(&mut self, r: &str, new: ObjectId) {
        let expect = self.get(r);
        let rid = self.rid();
        self.record(vec![RepoEvent::Refs {
            push: rid,
            actor: None,
            updates: vec![up(r, expect, Some(new))],
            pack: None,
        }]);
    }
}

fn admin() -> CredentialShard {
    CredentialShard { account: Some(AccountId(DbId(SITE_ADMIN))), key: None }
}

fn settings() -> RepoSettings {
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
        check_retention_days: 90,
    }
}

fn up(n: &str, expect: Option<ObjectId>, new: Option<ObjectId>) -> RefUpdate {
    RefUpdate { name: name(n), expect, new }
}

/// A push by `who`. An App's push carries `validate`'s outputs for every created or moved ref, as the edge sends one
/// once 32.8 is enabled; `paths` are its changed paths.
fn push(who: Principal, updates: Vec<RefUpdate>, paths: &[&str]) -> graftyard_git::repo::Push {
    let app = matches!(who.actor, Actor::App { .. });
    let changed: Vec<ChangedPaths> = if app {
        updates.iter().filter(|u| u.new.is_some()).map(|u| agent::changed_of(&u.name, paths)).collect()
    } else {
        Vec::new()
    };
    let protected = if app {
        updates.iter().filter_map(|u| agent::protected_of(&u.name, paths, &[])).collect()
    } else {
        Vec::new()
    };
    graftyard_git::repo::Push {
        updates,
        atomicity: Atomicity::PerRef,
        pack: Some(PackRef::Inline(b"PACK\0\0\0\x02\0\0\0\0".to_vec())),
        actor: who,
        options: Vec::new(),
        request: RequestId(Ulid(0)),
        agent: None,
        provenance: Vec::new(),
        protected,
        changed,
    }
}

fn issue(who: Principal, op: IssueOp<graftyard_git::blob::BlobRef>) -> RepoCmd {
    let mut c = IssueCall::new(op, who, RequestId(Ulid(0)), serde_json::Value::Null);
    // The edge sends 32.8 for the commands that may write agent events once `FormatsEnable` enabled it.
    c.agent = matches!(c.op, IssueOp::OpenPr { .. } | IssueOp::Review { .. });
    RepoCmd::Issue(Box::new(c))
}

fn merge(w: &mut World, who: Principal, n: u32) -> RepoCmd {
    let f = w.fold();
    let ph = tiers::pull_head(&*f.core, IssueNumber(n)).expect("a pull request");
    let base_old: ObjectId = tiers::get_rec(&*f.core, "ref", &[tiers::ref_key(&ph.base)]).unwrap();
    let commit = oid(0x00ff_0000 + n);
    let plan = MergePlan {
        base_old,
        head: ph.head_sha,
        commit,
        pack: DurablePack::trusted(PackRef::Inline(commit.0.to_vec())),
        method: MergeMethod::Merge,
        up_to_date: true,
        verified: TreeDiffDigest(Digest([1; 32])),
        engine: EngineVersion("git 2.51".into()),
    };
    let m = Merge { n: IssueNumber(n), plan, expected_head: Some(ph.head_sha), actor: who };
    RepoCmd::Merge(Box::new(MergeCall { cmd: MergeCmd::Merge(m), render: serde_json::Value::Null }))
}

fn refs(o: &RepoOutcome) -> Vec<RefOutcome> {
    match o {
        Ok(RepoOut::Push(p)) => p.refs.iter().map(|(_, r)| r.clone()).collect(),
        other => panic!("not a push outcome: {other:?}"),
    }
}

fn agent_rule(o: &RepoOutcome) -> Option<AgentRule> {
    match o {
        Ok(RepoOut::Push(p)) => p.refs.iter().find_map(|(_, r)| match r {
            RefOutcome::Rejected(RejectReason::Agent(a)) => Some(a.rule),
            _ => None,
        }),
        Err(RepoErr::Push(PushErr::Agent(a))) => Some(a.rule),
        Err(RepoErr::Issue { err: IssueErr::Agent(a), .. }) => Some(a.rule),
        Err(RepoErr::Merge { err: MergeError::Agent(a), .. }) => Some(a.rule),
        _ => None,
    }
}

fn open_pr(w: &mut World, who: Principal, branch: &str, draft: bool) -> RepoOutcome {
    let head = w.get(branch).expect("the head branch");
    let base = w.get("refs/heads/main").unwrap();
    let op = IssueOp::OpenPr {
        head: HeadSpec { repo: REPO, branch: name(branch) },
        base: name("refs/heads/main"),
        title: "t".into(),
        body: "b".into(),
        draft,
        head_sha: head,
        base_sha: base,
    };
    w.commit(issue(who, op))
}

fn number(o: &RepoOutcome) -> u32 {
    match o {
        Ok(RepoOut::Issue(c)) => match c.out {
            IssueOut::Number(n) => n.0,
            ref x => panic!("{x:?}"),
        },
        other => panic!("{other:?}"),
    }
}

fn approve(w: &mut World, who: Principal, n: u32) -> RepoOutcome {
    let head = tiers::pull_head(&*w.fold().core, IssueNumber(n)).unwrap().head_sha;
    let op = IssueOp::Review { n: IssueNumber(n), state: ReviewState::Approved, commit: head, body: "".into() };
    w.commit(issue(who, op))
}

// ---- LF-2 ----

/// LF-2: a confined App is refused outside its prefix; once the unflag is acknowledged, the very next decision on the
/// repository classes it `Bot` and accepts the same push. Repeated with the unflag and the push in consecutive
/// entries, and with the push admitted while the unflag's entry was still in flight.
#[test]
fn lf2_the_first_decision_after_an_unflag_no_longer_classes_the_app_an_agent() {
    const N: u32 = 25;
    let mut w = World::new();
    let mut after = 0;
    for i in 0..N {
        w.flag(claude_row());
        let branch = format!("refs/heads/x{i}");
        let o = w.commit(RepoCmd::Push(push(app(CLAUDE), vec![up(&branch, None, Some(oid(1000 + i)))], &["src/a.rs"])));
        assert_eq!(agent_rule(&o), Some(AgentRule::OutsidePrefix), "flagged: {o:?}");
        assert!(refs(&o).iter().all(|r| *r != RefOutcome::Ok));
        w.unflag(CLAUDE);
        let o = w.commit(RepoCmd::Push(push(app(CLAUDE), vec![up(&branch, None, Some(oid(1000 + i)))], &["src/a.rs"])));
        assert_eq!(refs(&o), [RefOutcome::Ok], "after the unflag's ack: {o:?}");
        after += 1;
    }
    // The unflag admitted on the install's leader before the push reaches the repository leader: the push's deps
    // read is answered after the unflag commits, so it decides on the new row too.
    w.flag(claude_row());
    let rid = w.rid();
    let l = w.install.leader();
    let now = w.now;
    let unflag = AuthzCmd { cred: admin(), op: AuthzOp::DeleteAgentApp { app: CLAUDE } };
    w.install.on(l, now, |m, t| m.commit(t, rid, unflag)).unwrap();
    let o = w.commit(RepoCmd::Push(push(app(CLAUDE), vec![up("refs/heads/y", None, Some(oid(9)))], &["a"])));
    assert_eq!(refs(&o), [RefOutcome::Ok], "{o:?}");
    eprintln!("LF-2: {after}/{N} first decisions after the unflag's ack classed the App Bot and accepted the push");
}

/// LF-2's planted bug as a test of the harness: a leader that decides on an install fold read before the unflag
/// (a cached copy) keeps refusing, so the gate can fail.
#[test]
fn lf2_control_a_stale_install_read_keeps_the_old_class() {
    let mut w = World::new();
    w.flag(claude_row());
    w.pinned = Some(w.install.fresh(w.now));
    w.unflag(CLAUDE);
    let o = w.commit(RepoCmd::Push(push(app(CLAUDE), vec![up("refs/heads/z", None, Some(oid(3)))], &["a"])));
    assert_eq!(agent_rule(&o), Some(AgentRule::OutsidePrefix));
}

// ---- B0-36 ----

/// Every refusal B0-36 names that `decide` can see before AM1-WP3 (no tasks, no dispatch, no leases), for the
/// Graftyard-run App and the confined vendor App, 100 times each; then the controls.
#[test]
fn b0_36_agents_are_refused_and_others_are_not() {
    let mut w = World::new();
    w.flag(claude_row());
    // The vendor App's PR, opened on its own prefix, and a protected-path setup.
    w.set("refs/heads/claude/a", oid(20));
    let pr = open_pr(&mut w, app(CLAUDE), "refs/heads/claude/a", false);
    let n = number(&pr);
    let f = w.fold();
    assert_eq!(tiers::agent_meta(&*f.core, IssueNumber(n)).map(|m| m.app), Some(CLAUDE), "AgentMeta on the agent's PR");
    // Labels: `agent` on the allowlist, `release` not.
    let lab = |w: &mut World, s: &str| {
        let op = IssueOp::DefineLabel { id: None, name: LabelName(s.into()), color: [1, 2, 3], description: None };
        match w.commit(issue(user(OWNER), op)) {
            Ok(RepoOut::Issue(c)) => match c.out {
                IssueOut::Id(id) => LabelId(id),
                x => panic!("{x:?}"),
            },
            o => panic!("{o:?}"),
        }
    };
    let ok_label = lab(&mut w, "agent");
    let bad_label = lab(&mut w, "release");
    let policy = AgentPolicy { labels: smallvec!["agent".into()], ..AgentPolicy::default() };
    let mut pc =
        IssueCall::new(IssueOp::SetAgentPolicy(policy), user(OWNER), RequestId(Ulid(0)), serde_json::Value::Null);
    pc.agent = true;
    w.commit(RepoCmd::Issue(Box::new(pc))).expect("policy set by the repository admin");
    // A rule over `claude/**` that lists the App as a pusher: an agent push needs that bypass.
    let rule = IssueOp::SetRule {
        id: None,
        pattern: "claude/locked".into(),
        protection: Some(Protection {
            checks: None,
            reviews: None,
            push: Some(smallvec![Pusher::App(CLAUDE)]),
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
    };
    w.commit(issue(user(OWNER), rule)).expect("rule");
    let head = w.get("refs/heads/claude/a").unwrap();
    let status = move |ctx: &str| -> IssueOp<graftyard_git::blob::BlobRef> {
        let creator = AccountId(DbId(0));
        IssueOp::RecordStatus(StatusUpdate {
            id: StatusId(DbId(1 << 30)),
            sha: head,
            context: ctx.into(),
            state: StatusState::Success,
            target_url: None,
            description: None,
            creator,
        })
    };
    let check = move |a: AppId, nm: &str| -> IssueOp<graftyard_git::blob::BlobRef> {
        IssueOp::RecordCheck(CheckRunUpdate {
            run: CheckRunId(DbId(1 << 31)),
            suite: CheckSuiteId(DbId(1 << 31)),
            app: a,
            head_sha: head,
            name: nm.into(),
            version: 1,
            status: CheckStatus::Completed,
            conclusion: Some(graftyard_git::issues::CheckConclusion::Success),
            started: None,
            completed: None,
            external_id: None,
            details_url: None,
            output: None,
        })
    };
    type Case = (&'static str, AgentRule, Box<dyn Fn(&mut World, AppId, u32) -> RepoCmd>);
    let cases: Vec<Case> = vec![
        (
            "approve",
            AgentRule::Approve,
            Box::new(move |w, a, _| {
                let head = tiers::pull_head(&*w.fold().core, IssueNumber(n)).unwrap().head_sha;
                issue(
                    app(a),
                    IssueOp::Review { n: IssueNumber(n), state: ReviewState::Approved, commit: head, body: "".into() },
                )
            }),
        ),
        ("merge", AgentRule::Merge, Box::new(move |w, a, _| merge(w, app(a), n))),
        (
            "ready for review",
            AgentRule::ReadyForReview,
            Box::new(move |_, a, _| {
                issue(app(a), IssueOp::Change { n: IssueNumber(n), change: IssueChange::Draft(false) })
            }),
        ),
        (
            "rule edit",
            AgentRule::Rules,
            Box::new(|_, a, _| issue(app(a), IssueOp::SetRule { id: None, pattern: "x".into(), protection: None })),
        ),
        ("settings", AgentRule::Rules, Box::new(|_, a, _| issue(app(a), IssueOp::Settings(settings())))),
        (
            "release",
            AgentRule::Release,
            Box::new(move |_, a, i| {
                issue(
                    app(a),
                    IssueOp::PutRelease {
                        id: None,
                        tag: name(&format!("refs/tags/v{i}")),
                        target: oid(1),
                        name: "r".into(),
                        body: "".into(),
                        draft: false,
                        prerelease: false,
                        latest: true,
                    },
                )
            }),
        ),
        (
            "label outside the allowlist",
            AgentRule::Label,
            Box::new(move |_, a, _| {
                issue(
                    app(a),
                    IssueOp::ApplyLabels { n: IssueNumber(n), add: smallvec![bad_label], remove: SmallVec::new() },
                )
            }),
        ),
        (
            "graftyard/ status",
            AgentRule::GraftyardContext,
            Box::new(move |_, a, _| issue(app(a), status("graftyard/agent-gate"))),
        ),
        (
            "graftyard/ check run",
            AgentRule::GraftyardContext,
            Box::new(move |_, a, _| issue(app(a), check(a, "graftyard/agent-gate"))),
        ),
        (
            "tag push",
            AgentRule::Tag,
            Box::new(|_, a, i| {
                RepoCmd::Push(push(app(a), vec![up(&format!("refs/tags/t{i}"), None, Some(oid(7)))], &["a"]))
            }),
        ),
        (
            "push outside the prefix",
            AgentRule::OutsidePrefix,
            Box::new(|_, a, i| {
                RepoCmd::Push(push(app(a), vec![up(&format!("refs/heads/main{i}"), None, Some(oid(7)))], &["a"]))
            }),
        ),
    ];
    let mut counts = Vec::new();
    for (label, rule, make) in &cases {
        let mut refused = 0;
        for i in 0..100 {
            let c = make(&mut w, CLAUDE, i);
            if agent_rule(&w.commit(c)) == Some(*rule) {
                refused += 1;
            }
        }
        counts.push((*label, refused));
        assert_eq!(refused, 100, "{label}: {refused}/100");
    }
    // Pushes inside the prefix: a protected path and a bypass are refused, an ordinary change is accepted.
    let mut prot = 0;
    for i in 0..100 {
        let r = format!("refs/heads/claude/p{i}");
        let o = w.commit(RepoCmd::Push(push(
            app(CLAUDE),
            vec![up(&r, None, Some(oid(7)))],
            &["src/x.rs", ".github/workflows/ci.yml"],
        )));
        prot += usize::from(agent_rule(&o) == Some(AgentRule::Protected));
    }
    assert_eq!(prot, 100);
    w.set("refs/heads/claude/locked", oid(30));
    let o = w.commit(RepoCmd::Push(push(
        app(CLAUDE),
        vec![up("refs/heads/claude/locked", Some(oid(30)), Some(oid(31)))],
        &["a"],
    )));
    assert_eq!(agent_rule(&o), Some(AgentRule::Bypass), "{o:?}");
    let o =
        w.commit(RepoCmd::Push(push(app(CLAUDE), vec![up("refs/heads/claude/ok", None, Some(oid(8)))], &["src/a.rs"])));
    assert_eq!(refs(&o), [RefOutcome::Ok]);
    // The allowlisted label is applied.
    let o = w.commit(issue(
        app(CLAUDE),
        IssueOp::ApplyLabels { n: IssueNumber(n), add: smallvec![ok_label], remove: SmallVec::new() },
    ));
    assert!(o.is_ok(), "{o:?}");
    // `refs/graftyard/` is the host's for every actor.
    let o = w.commit(RepoCmd::Push(push(user(OWNER), vec![up("refs/graftyard/sessions/x", None, Some(oid(9)))], &[])));
    assert_eq!(refs(&o), [RefOutcome::Rejected(RejectReason::Hidden)]);

    // The Graftyard-run App: with no live task (none exists before AM1-WP3) every push is refused, and so is
    // everything above.
    register_own(&mut w, RUN);
    w.flag(AgentApp { app: RUN, kind: AgentKind::GraftyardRun, confine: None });
    let mut run_counts = Vec::new();
    for (label, rule, make) in &cases {
        let mut refused = 0;
        for i in 0..100 {
            let c = make(&mut w, RUN, 1_000 + i);
            let r = agent_rule(&w.commit(c));
            // A push is refused first for the missing task, in the design's order.
            let want = if matches!(*rule, AgentRule::OutsidePrefix) { AgentRule::NoLiveTask } else { *rule };
            if r == Some(want) {
                refused += 1;
            }
        }
        run_counts.push((*label, refused));
        assert_eq!(refused, 100, "graftyard-run {label}: {refused}/100");
    }
    let o = w.commit(RepoCmd::Push(push(app(RUN), vec![up("refs/heads/run/a", None, Some(oid(8)))], &["a"])));
    assert_eq!(agent_rule(&o), Some(AgentRule::NoLiveTask));
    w.set("refs/heads/run/b", oid(40));
    let o = open_pr(&mut w, app(RUN), "refs/heads/run/b", false);
    assert_eq!(agent_rule(&o), Some(AgentRule::DraftOnly), "{o:?}");
    assert!(open_pr(&mut w, app(RUN), "refs/heads/run/b", true).is_ok());

    // Controls: an unflagged App, a flagged App without `confine` (Bot), and `Graftyard MCP` are never refused by an
    // agent rule; a user is not either.
    w.flag(AgentApp { app: MCP, kind: AgentKind::ThirdParty, confine: None });
    for (who, label) in
        [(app(PLAIN), "unflagged App"), (app(MCP), "flagged App without confine"), (user(WRITER), "user")]
    {
        let o = w.commit(RepoCmd::Push(push(
            who.clone(),
            vec![up(&format!("refs/heads/ctl-{}", label.len()), None, Some(oid(11)))],
            &["a"],
        )));
        assert_eq!(refs(&o), [RefOutcome::Ok], "{label}: {o:?}");
        assert_eq!(
            agent::actor_kind(&who, None),
            if matches!(who.actor, Actor::App { .. }) { agent::ActorKind::Bot } else { agent::ActorKind::User }
        );
    }
    eprintln!(
        "B0-36 confined vendor App: {counts:?}; protected path {prot}/100; bypass refused; allowlisted label applied"
    );
    eprintln!("B0-36 Graftyard-run App: {run_counts:?}; draft-only refused a ready PR");
}

/// Registers `app` as the install's own App, the way `PutApp` commits after its `Names` slug claim, so a
/// `GraftyardRun` row for it is allowed.
fn register_own(w: &mut World, a: AppId) {
    let rec = graftyard_identity::AppRecord {
        id: a,
        slug: graftyard_identity::Slug::parse("graftyard-agent").unwrap(),
        name: "graftyard-agent".into(),
        client: graftyard_identity::ClientId::for_app(a),
        keys: SmallVec::new(),
        permissions: everything(),
        events: SmallVec::new(),
        webhook: None,
        webhook_secret: None,
        client_secrets: SmallVec::new(),
        callbacks: SmallVec::new(),
        public: false,
    };
    w.ident.names.apply(&NamesEvent::AppSlug { slug: rec.slug.clone(), app: Some(a), owner: install_account() });
    w.authz(AuthzOp::PutApp { record: Box::new(rec) }, admin()).expect("the install registers its own App");
}

// ---- AgentGate ----

fn gate_rule(o: &RepoOutcome) -> Option<String> {
    match o {
        Err(RepoErr::Merge { err: MergeError::Agent(a), .. }) if a.rule == AgentRule::Gate => Some(a.text.to_string()),
        _ => None,
    }
}

/// A PR an `AgentApp` opened merges only once `AgentGate` holds on its current head: with no recorded requester (no
/// tasks before AM1-WP3) two distinct human writers; an approval through the App's user-to-server token writes no
/// gate; a head move voids the gates on the old head.
#[test]
fn agent_gate_counts_human_writers_on_the_current_head() {
    let mut w = World::new();
    w.flag(claude_row());
    w.set("refs/heads/claude/g", oid(50));
    let n = number(&open_pr(&mut w, app(CLAUDE), "refs/heads/claude/g", false));
    let meta = tiers::agent_meta(&*w.fold().core, IssueNumber(n)).expect("AgentMeta");
    assert_eq!((meta.app, meta.requester), (CLAUDE, None));
    let gates = |w: &mut World| {
        let f = w.fold();
        let head = tiers::pull_head(&*f.core, IssueNumber(n)).unwrap().head_sha;
        tiers::agent_gates(&*f.core, IssueNumber(n), &head).len()
    };
    let m = merge(&mut w, user(WRITER), n);
    let t = gate_rule(&w.commit(m)).expect("refused before any approval");
    assert!(t.contains("0 of 2"), "{t}");
    approve(&mut w, user(WRITER), n).unwrap();
    assert_eq!(gates(&mut w), 1);
    // The same writer again, and a writer approving through CLAUDE's user-to-server token: no new gate.
    approve(&mut w, user(WRITER), n).unwrap();
    approve(&mut w, user(VIA_APP), n).unwrap();
    assert_eq!(gates(&mut w), 1, "a repeat and an AgentApp's user token count nothing");
    let m = merge(&mut w, user(WRITER), n);
    assert!(gate_rule(&w.commit(m)).expect("1 of 2").contains("1 of 2"));
    // A reader's approval does not qualify.
    let _ = approve(&mut w, user(READER), n);
    assert_eq!(gates(&mut w), 1);
    approve(&mut w, user(WRITER2), n).unwrap();
    assert_eq!(gates(&mut w), 2);
    // The App pushes again: the gates were for the old head.
    let o = w.commit(RepoCmd::Push(push(
        app(CLAUDE),
        vec![up("refs/heads/claude/g", Some(oid(50)), Some(oid(51)))],
        &["src/a.rs"],
    )));
    assert_eq!(refs(&o), [RefOutcome::Ok]);
    assert_eq!(gates(&mut w), 0, "a head move voids the gate");
    let m = merge(&mut w, user(WRITER), n);
    assert!(gate_rule(&w.commit(m)).expect("0 of 2 on the new head").contains("0 of 2"));
    approve(&mut w, user(WRITER), n).unwrap();
    approve(&mut w, user(WRITER2), n).unwrap();
    let m = merge(&mut w, user(WRITER), n);
    assert!(matches!(w.commit(m), Ok(RepoOut::Merge(_))), "merges once the gate holds");
    // `extra_human_approvals` raises the count.
    let policy = AgentPolicy { extra_human_approvals: 1, ..AgentPolicy::default() };
    let mut pc =
        IssueCall::new(IssueOp::SetAgentPolicy(policy), user(OWNER), RequestId(Ulid(0)), serde_json::Value::Null);
    pc.agent = true;
    w.commit(RepoCmd::Issue(Box::new(pc))).unwrap();
    w.set("refs/heads/claude/h", oid(60));
    let n = number(&open_pr(&mut w, app(CLAUDE), "refs/heads/claude/h", false));
    approve(&mut w, user(WRITER), n).unwrap();
    approve(&mut w, user(WRITER2), n).unwrap();
    let m = merge(&mut w, user(WRITER), n);
    assert!(gate_rule(&w.commit(m)).expect("2 of 3").contains("2 of 3"));
    approve(&mut w, user(OWNER), n).unwrap();
    let m = merge(&mut w, user(OWNER), n);
    assert!(matches!(w.commit(m), Ok(RepoOut::Merge(_))));
}

/// A user's PR, opened and reviewed in the default formats and in 32.8, carries no `AgentMeta`, writes no
/// `AgentGate`, and merges with no approval as before.
#[test]
fn a_default_pr_merges_as_before() {
    let mut w = World::new();
    w.flag(claude_row());
    for (i, agent) in [false, true].into_iter().enumerate() {
        let branch = format!("refs/heads/feat{i}");
        w.set(&branch, oid(70 + i as u32));
        let head = w.get(&branch).unwrap();
        let op = IssueOp::OpenPr {
            head: HeadSpec { repo: REPO, branch: name(&branch) },
            base: name("refs/heads/main"),
            title: "t".into(),
            body: "b".into(),
            draft: false,
            head_sha: head,
            base_sha: w.get("refs/heads/main").unwrap(),
        };
        let mut c = IssueCall::new(op, user(WRITER), RequestId(Ulid(0)), serde_json::Value::Null);
        c.agent = agent;
        let n = number(&w.commit(RepoCmd::Issue(Box::new(c))));
        let op = IssueOp::Review { n: IssueNumber(n), state: ReviewState::Approved, commit: head, body: "".into() };
        let mut c = IssueCall::new(op, user(WRITER2), RequestId(Ulid(0)), serde_json::Value::Null);
        c.agent = agent;
        w.commit(RepoCmd::Issue(Box::new(c))).unwrap();
        let f = w.fold();
        assert!(tiers::agent_meta(&*f.core, IssueNumber(n)).is_none());
        assert!(tiers::agent_gates(&*f.core, IssueNumber(n), &head).is_empty());
        let m = merge(&mut w, user(WRITER), n);
        assert!(matches!(w.commit(m), Ok(RepoOut::Merge(_))), "agent={agent}");
    }
    let l = w.repo.leader();
    let agent_events = w.repo.members[l.0 as usize]
        .log()
        .iter()
        .filter_map(|e| match &e.body {
            graftyard_raft::EntryBody::Events(evs) => Some(evs.clone()),
            _ => None,
        })
        .flatten()
        .filter(|e| matches!(e, RepoEvent::AgentMeta { .. } | RepoEvent::AgentGate(_) | RepoEvent::Provenance { .. }))
        .count();
    assert_eq!(agent_events, 0);
}

/// BF-34 through a real leader: an invitation's `expires` is the entry's time plus a week, so the invitee can list
/// and accept it (the leader once decided issue commands at time 0, so every invitation had already lapsed).
#[test]
fn a_repository_invitation_sent_through_the_leader_is_pending_and_accepted() {
    let mut w = World::new();
    // A clock past a week since the epoch, as a hosted leader's is (2026-11-08).
    w.now = Timestamp(1_794_000_000_000);
    w.settle();
    let invite = IssueOp::InviteCollaborator { user: AccountId(DbId(READER)), permission: RepoPermission::Write };
    let id = match w.commit(issue(user(OWNER), invite)) {
        Ok(RepoOut::Issue(c)) => match c.out {
            IssueOut::Id(id) => id,
            ref x => panic!("{x:?}"),
        },
        other => panic!("{other:?}"),
    };
    let f = w.fold();
    let row: graftyard_identity::InvitationRow =
        tiers::get_rec(&*f.core, "invitation", &[graftyard_fold::Val::Int(id.0)]).expect("a pending invitation");
    assert!(row.expires > w.now, "expires {:?} is not after the entry's time {:?}", row.expires, w.now);
    let accept = IssueOp::AnswerInvitation { id: graftyard_identity::InvitationId(id), accept: true };
    w.commit(issue(user(READER), accept)).expect("the invitee accepts");
    let f = w.fold();
    assert_eq!(graftyard_git::issue_ops::collaborator(&*f.core, AccountId(DbId(READER))), Some(RepoPermission::Write));
}

// ---- Co-authored-by ----

fn raw_commit(message: &[u8]) -> Vec<u8> {
    let mut b = b"tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\nauthor A <a@x> 1 +0000\ncommitter A <a@x> 1 +0000\n\n"
        .to_vec();
    b.extend_from_slice(message);
    b
}

/// A commit whose only trailer is `Co-authored-by` is recorded `Claimed` and classes nothing: a user-token push of it
/// stays Human and is accepted; an unflagged App's stays Bot. `Assisted-by` classes the App's push Agent for
/// admission only.
#[test]
fn co_authored_by_alone_never_classes() {
    let mut w = World::new();
    let r = name("refs/heads/co");
    let raw = raw_commit(b"Fix\n\nCo-authored-by: Claude <noreply@anthropic.com>\n");
    let prov = agent::provenance_of(&r, &[(oid(80), &raw)], &[]);
    assert_eq!(prov.len(), 1);
    assert!(!agent::classes(&prov));
    let install = w.install.fresh(w.now);
    let read = Some(&install as &dyn graftyard_identity::AuthzRead);
    assert_eq!(agent::admit_class(&user(WRITER), read, None, &prov, false), graftyard_raft::admit::AdmitClass::Human);
    assert_eq!(agent::admit_class(&app(PLAIN), read, None, &prov, false), graftyard_raft::admit::AdmitClass::Bot);
    let raw2 = raw_commit(b"Fix\n\nAssisted-by: Amp\n");
    let p2 = agent::provenance_of(&r, &[(oid(81), &raw2)], &[]);
    assert_eq!(agent::admit_class(&app(PLAIN), read, None, &p2, false), graftyard_raft::admit::AdmitClass::Agent);
    assert_eq!(agent::admit_class(&user(WRITER), read, None, &p2, false), graftyard_raft::admit::AdmitClass::Human);
    let mut p = push(user(WRITER), vec![up("refs/heads/co", None, Some(oid(80)))], &[]);
    p.provenance = prov;
    let o = w.commit(RepoCmd::Push(p));
    assert_eq!(refs(&o), [RefOutcome::Ok]);
}

// ---- B0-37 ----

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

const ORIGIN: &str = "https://graftyard.example/o/r";

/// One malformed or forged commit message.
fn hostile_message(g: &mut Rng, i: u32) -> Vec<u8> {
    let keys = [
        "Graftyard-Session",
        "graftyard-session",
        "GRAFTYARD-SESSION",
        "Claude-Session",
        "Amp-Thread-ID",
        "Co-authored-by",
        "Assisted-by",
        "Signed-off-by",
        "X-Unknown",
        "Graftyard Session",
        "-bad",
    ];
    let uuid = format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", g.next() as u32, i as u16, 1u16, 2u16, g.next() >> 16);
    let forged = [
        format!("{ORIGIN}/_graftyard/sessions/{uuid}"),
        format!("https://evil.example/o/r/_graftyard/sessions/{uuid}"),
        format!("{ORIGIN}/_graftyard/sessions/{uuid}/../x"),
        format!("{ORIGIN}/_graftyard/sessions/%2e%2e"),
        format!("{ORIGIN}/_graftyard/sessions/3.mine"),
        format!("{ORIGIN}/_graftyard/sessions/mine"),
        format!("{ORIGIN}/_graftyard/sessions/"),
        format!("http://graftyard.example/o/r/_graftyard/sessions/{uuid}"),
        format!("{}/_graftyard/sessions/{uuid}", ORIGIN.to_uppercase()),
        format!("{ORIGIN}/_graftyard/sessions/{uuid}\u{202e}"),
    ];
    match g.below(8) {
        0 => {
            let n = g.below(400) as usize;
            g.bytes(n)
        }
        1 => {
            let mut m = b"subject\n\n".to_vec();
            let k = g.below(40);
            for _ in 0..k {
                let key = keys[g.below(keys.len() as u64) as usize];
                let sep = [": ", ":", " : ", ":\t", "="][g.below(5) as usize];
                let n = g.below(64) as usize;
                let mut v = g.bytes(n);
                v.retain(|b| *b != b'\n');
                m.extend_from_slice(key.as_bytes());
                m.extend_from_slice(sep.as_bytes());
                m.extend_from_slice(&v);
                m.extend_from_slice([b"\n".as_slice(), b"\r\n", b"\n continued\n", b"\r"][g.below(4) as usize]);
            }
            m
        }
        2 | 3 => {
            let f = &forged[g.below(forged.len() as u64) as usize];
            let key = keys[g.below(3) as usize];
            format!("Do it\n\nbody\n\n{key}: {f}\nCo-authored-by: X <x@y>\n").into_bytes()
        }
        4 => {
            let long = "é".repeat(5_000);
            format!("s\n\nAmp-Thread-ID: {long}\nClaude-Session: {long}\n").into_bytes()
        }
        5 => ["", "\n", "\n\n", "only subject", "s\n\n\n\n", "s\r\n\r\nAssisted-by: a\r\n"][g.below(6) as usize]
            .as_bytes()
            .to_vec(),
        6 => {
            // A trailer block buried in prose, under a quarter trailers.
            let mut m = "s\n\n".to_string();
            for _ in 0..8 {
                m.push_str("prose line\n");
            }
            m.push_str(&format!("Graftyard-Session: {}\n", forged[0]));
            m.into_bytes()
        }
        _ => {
            if i % 1_000 == 7 {
                // Past MESSAGE_SCAN_MAX: only the tail is read.
                let mut m = vec![b'x'; agent::MESSAGE_SCAN_MAX + 10];
                m.extend_from_slice(format!("\n\nGraftyard-Session: {}\n", forged[0]).as_bytes());
                m
            } else {
                format!("s\n\nGraftyard-Session: {}\n", forged[0]).into_bytes()
            }
        }
    }
}

fn hostile_notes(g: &mut Rng) -> Vec<(String, bool, ObjectId)> {
    (0..g.below(4))
        .map(|_| {
            let hex: String =
                (0..40).map(|_| "0123456789abcdefABCDEFxz/"[g.below(25) as usize..][..1].to_string()).collect();
            let path = match g.below(4) {
                0 => format!("{}/{}", &hex[..2], &hex[2..]),
                1 => hex,
                2 => format!("{}/{}/{}", &hex[..2], &hex[2..4], &hex[4..]),
                _ => String::from_utf8_lossy(&g.bytes(12)).into_owned(),
            };
            (path, g.below(3) != 0, oid(g.next() as u32))
        })
        .collect()
}

/// B0-37 in memory: 10,000 pushes whose commits carry malformed trailers and notes and forged `Graftyard-Session`
/// trailers, through `validate`'s parser, the push wire, and `decide` on the real members; one in ten also writes
/// `refs/graftyard/`. Zero fail on parsing, every forged trailer stays `Claimed` (a `Session` sent on the wire with no
/// task is dropped), and every `refs/graftyard/` update is refused `Hidden`.
#[test]
fn b0_37_hostile_provenance_and_host_refs() {
    const PUSHES: u32 = 10_000;
    let mut w = World::new();
    w.flag(claude_row());
    let mut g = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut parse_fail, mut forged_seen, mut forged_linked, mut hidden_tried, mut hidden_refused) = (0, 0, 0, 0, 0);
    let mut wire_sessions = 0;
    let mut batch = Vec::new();
    let mut expect_hidden = Vec::new();
    for i in 0..PUSHES {
        let who = match i % 4 {
            0 => user(WRITER),
            1 => app(PLAIN),
            2 => app(MCP),
            _ => app(CLAUDE),
        };
        let prefix = if i % 4 == 3 { "refs/heads/claude/b37-" } else { "refs/heads/b37-" };
        let branch = format!("{prefix}{i}");
        let r = name(&branch);
        let raws: Vec<Vec<u8>> = (0..1 + g.below(3)).map(|_| raw_commit(&hostile_message(&mut g, i))).collect();
        let commits: Vec<(ObjectId, &[u8])> =
            raws.iter().enumerate().map(|(k, b)| (oid(i * 4 + k as u32 + 1_000_000), b.as_slice())).collect();
        let notes = hostile_notes(&mut g);
        let entries: Vec<agent::NoteEntry<'_>> =
            notes.iter().map(|(p, blob, o)| agent::NoteEntry { path: p, blob: *blob, oid: *o }).collect();
        let parsed = std::panic::catch_unwind(|| agent::provenance_of(&r, &commits, &entries));
        let Ok(prov) = parsed else {
            parse_fail += 1;
            continue;
        };
        // The edge's linking with the pusher's sessions: none exist, and a session the App owns does not match a
        // forged URL.
        let pusher = match who.actor {
            Actor::App { app, .. } => Some(app),
            _ => None,
        };
        let own = [graftyard_git::agent::SessionId::Run(0x1234)];
        let mut linked = agent::link(&prov, pusher, &own, Some(ORIGIN));
        for p in &linked {
            for s in &p.sources {
                match s {
                    agent::Source::Claimed { key: agent::TrailerKey::GraftyardSession, .. } => forged_seen += 1,
                    agent::Source::Session(_) => forged_linked += 1,
                    _ => {}
                }
            }
        }
        // One in five also forges a `Session` source on the wire.
        if i % 5 == 0 && !linked.is_empty() {
            linked[0].sources.push(agent::Source::Session(graftyard_git::agent::SessionId::Run(u128::from(i))));
            wire_sessions += 1;
        }
        let mut updates = vec![up(&branch, None, Some(oid(i + 2_000_000)))];
        if i % 10 == 0 {
            updates.push(up(&format!("refs/graftyard/sessions/s{i}"), None, Some(oid(i + 3_000_000))));
            hidden_tried += 1;
        }
        let mut p = push(who, updates, &["src/lib.rs"]);
        p.provenance = linked.into_iter().filter(|x| x.refname == r).collect();
        // The push crosses the member wire.
        let Ok(p) = graftyard_git::repo::Push::from_wire(&p.to_wire()) else {
            parse_fail += 1;
            continue;
        };
        expect_hidden.push(i % 10 == 0);
        batch.push(RepoCmd::Push(p));
        if batch.len() == 100 || i + 1 == PUSHES {
            for (o, hid) in w.commit_all(std::mem::take(&mut batch)).iter().zip(std::mem::take(&mut expect_hidden)) {
                let rs = match o {
                    Ok(RepoOut::Push(p)) => p.refs.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>(),
                    _ => {
                        parse_fail += 1;
                        continue;
                    }
                };
                if rs[0] != RefOutcome::Ok {
                    parse_fail += 1;
                }
                if hid {
                    hidden_refused += usize::from(rs[1] == RefOutcome::Rejected(RejectReason::Hidden));
                }
            }
        }
    }
    let f = w.fold();
    let rows = graftyard_fold::Read::prefix(&*f.meta, "provenance", &[]);
    let mut stored_sessions = 0;
    let mut stored_claimed = 0;
    for (_, cols) in &rows {
        let graftyard_fold::Val::Blob(b) = &cols[0] else { panic!("a record") };
        let sources: SmallVec<[agent::Source; 2]> = postcard::from_bytes(b).unwrap();
        for s in sources {
            match s {
                agent::Source::Session(_) => stored_sessions += 1,
                agent::Source::Claimed { key: agent::TrailerKey::GraftyardSession, .. } => stored_claimed += 1,
                _ => {}
            }
        }
    }
    let host_refs = graftyard_fold::Read::prefix(&*f.core, "ref", &[])
        .iter()
        .filter(|(k, _)| matches!(&k[0], graftyard_fold::Val::Blob(b) if b.starts_with(agent::HOST_REFS.as_bytes())))
        .count();
    eprintln!(
        "B0-37: {PUSHES} pushes, {parse_fail} failed on parsing; {forged_seen} forged Graftyard-Session trailers, \
         {forged_linked} linked; {wire_sessions} Session sources forged on the wire, {stored_sessions} stored; \
         {stored_claimed} stored Claimed; refs/graftyard/ {hidden_refused}/{hidden_tried} refused Hidden, {host_refs} \
         in the fold; {} provenance rows",
        rows.len()
    );
    assert_eq!(parse_fail, 0);
    assert!(forged_seen > 1_000, "the run forged trailers: {forged_seen}");
    assert_eq!(forged_linked, 0);
    assert!(wire_sessions > 100);
    assert_eq!(stored_sessions, 0);
    assert!(stored_claimed > 1_000);
    assert_eq!(hidden_refused, hidden_tried);
    assert_eq!(host_refs, 0);
}
