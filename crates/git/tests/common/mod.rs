//! The in-memory gate harness of AM1's agent packages: three real `graftyard_raft::Member<AuthzCmd>` of the install's
//! own `Authz(Genesis.account)` chain, and three real `Member<RepoCmd>` of `repo:7`, whose leader answers every deps
//! request by reading the install's chain through `fresh` on its own leader, as a site host does once per entry. Moved
//! here from `agent_gates.rs` (AM1-WP2) so `task_gates.rs` (AM1-WP3) runs on the same members.

#![allow(dead_code, unused_imports)]

pub use graftyard_git::agent::{self, AgentPolicy, AgentRule, ChangedPaths};
pub use graftyard_git::issue_ops::call::IssueCall;
pub use graftyard_git::issue_ops::{IssueErr, IssueOp, IssueOut};
pub use graftyard_git::issues::{
    AppId, CheckRunId, CheckRunUpdate, CheckStatus, CheckSuiteId, HeadSpec, IssueChange, IssueNumber, LabelId,
    LabelName, MergeMethod, Protection, Pusher, RepoSettings, ReviewState, StatusId, StatusState, StatusUpdate,
    Visibility,
};
pub use graftyard_git::merge::{EngineVersion, Merge, MergeCmd, MergeError, MergePlan, TreeDiffDigest};
pub use graftyard_git::merge_call::MergeCall;
pub use graftyard_git::pack::PackRef;
pub use graftyard_git::refs::{Atomicity, ObjectId, RefName, RefUpdate};
pub use graftyard_git::repo::{
    PushErr, REPO_FORMAT_AGENT, REPO_FORMAT_STATELESS, REPO_FORMAT_V1, REPO_FORMAT_V2, RefOutcome, RejectReason,
    RepoCmd, RepoErr, RepoEvent, RepoFold, RepoOut,
};
pub use graftyard_git::{DurablePack, tiers};
pub use graftyard_identity::glob::RefPattern;
pub use graftyard_identity::stateless::InstallationClaims;
pub use graftyard_identity::{
    AUTHZ_FORMAT, AUTHZ_FORMAT_V2, AUTHZ_FORMAT_V3, Access, AccountKind, Actor, AgentApp, AgentKind, AuthzCmd,
    AuthzEvent, AuthzFold, AuthzOp, Credential, IdentityDep, InstallationId, InstallationRow, Jti, Kid, NamesEvent,
    NamesFold, OrgRole, PermissionSet, Principal, RepoPermission, RepoScope, ScopeSet, TokenFields, TokenKind,
    TokensEvent, TokensFold, UserId, install_account,
};
pub use graftyard_raft::{
    Command, CommitError, Envelope, Ev, Fold as _, Format, Member, MemberConfig, ReleaseTag, RequestId, Resolution,
    Role, Stored, Ulid,
};
pub use graftyard_types::{
    AccountId, ChainId, CredentialShard, DbId, Digest, NameShard, PlacementName, PlacementRecord, Region, ReleaseId,
    RepoId, SiteId, Timestamp, TokenShard,
};
pub use smallvec::{SmallVec, smallvec};

pub const SITES: [SiteId; 3] = [SiteId(0), SiteId(1), SiteId(2)];
pub const REPO: RepoId = RepoId(DbId(7));
pub const OWNER: i64 = 100;
pub const WRITER: i64 = 101;
pub const WRITER2: i64 = 102;
pub const READER: i64 = 103;
/// The install org's admin, who flags Apps.
pub const SITE_ADMIN: i64 = 104;
/// A user who signed in to `CLAUDE` and holds its user-to-server token.
pub const VIA_APP: i64 = 105;
pub const CLAUDE: AppId = AppId(DbId(3));
pub const RUN: AppId = AppId(DbId(4));
pub const PLAIN: AppId = AppId(DbId(5));
pub const MCP: AppId = AppId(DbId(6));

// ---- fixtures ----

pub fn name(s: &str) -> RefName {
    RefName::parse(s.as_bytes()).unwrap()
}

pub fn oid(n: u32) -> ObjectId {
    let mut o = [0u8; 20];
    o[..4].copy_from_slice(&n.to_be_bytes());
    o[19] = 7;
    ObjectId(o)
}

pub fn user(a: i64) -> Principal {
    Principal {
        actor: Actor::User(UserId(AccountId(DbId(a)))),
        scopes: ScopeSet(1),
        credential: Credential::Stored([a as u8; 32]),
        sso_orgs: SmallVec::new(),
    }
}

pub fn claims(app: AppId) -> InstallationClaims {
    InstallationClaims {
        app,
        installation: InstallationId(DbId(900 + app.0.0)),
        repos: RepoScope::Selected(smallvec![REPO]),
        permissions: everything(),
        jti: Jti(Ulid(0x0193_0000_0000_0000_0000_0000_0000_0000 + app.0.0 as u128)),
        iat: Timestamp(9_000),
        exp: Timestamp(9_000 + 3_600_000),
        kid: Kid(1),
        task: None,
    }
}

/// An installation widened to every permission, as B0-36 runs the Graftyard-run token: a refusal must come from the
/// agent rule, not from a missing permission.
pub fn everything() -> PermissionSet {
    let names = ["contents", "issues", "pull_requests", "checks", "statuses", "administration", "metadata", "actions"];
    PermissionSet(names.iter().map(|n| (Box::from(*n), Access::Write)).collect())
}

pub fn app(a: AppId) -> Principal {
    Principal::installation(&claims(a))
}

pub fn claude_row() -> AgentApp {
    AgentApp {
        app: CLAUDE,
        kind: AgentKind::ThirdParty,
        confine: Some(RefPattern::parse("refs/heads/claude/**").unwrap()),
    }
}

pub fn record() -> PlacementRecord {
    PlacementRecord {
        name: PlacementName::Tri,
        sites: SITES.iter().copied().collect(),
        home: SITES[0],
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    }
}

pub fn config(chain: ChainId, me: SiteId, reads: Vec<Format>) -> MemberConfig {
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
pub struct Ident {
    pub names: NamesFold,
    pub tokens: TokensFold,
    pub owner: AuthzFold,
}

impl Ident {
    pub fn new() -> Ident {
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

    pub fn dep(&self, c: ChainId, install: &AuthzFold) -> IdentityDep {
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

pub struct Group<C: Command> {
    pub members: Vec<Member<C>>,
    pub disks: Vec<Stored<C::Fold>>,
    pub inbox: Vec<Envelope<Ev<C>>>,
    pub resolutions: Vec<Resolution<C>>,
    pub deps: Vec<(SiteId, graftyard_raft::DepsWanted)>,
}

impl<C: Command> Group<C> {
    pub fn new(chain: ChainId, reads: Vec<Format>, fold: C::Fold, now: Timestamp) -> Group<C> {
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

    pub fn on<R>(
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

    pub fn deliver(&mut self, now: Timestamp) {
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

    pub fn leader(&self) -> SiteId {
        *SITES
            .iter()
            .filter(|s| self.members[s.0 as usize].role() == Role::Leader)
            .max_by_key(|s| self.members[s.0 as usize].term())
            .expect("a leader")
    }

    pub fn elect(&mut self, now: Timestamp) {
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
    pub fn fresh(&mut self, now: Timestamp) -> C::Fold {
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

/// The formats every repository member here reads: the base's and AM1's.
pub fn repo_reads() -> Vec<Format> {
    vec![
        REPO_FORMAT_V1,
        REPO_FORMAT_V2,
        graftyard_git::prsync::REPO_FORMAT_V3,
        graftyard_git::merge::REPO_FORMAT_V4,
        graftyard_git::issue_ops::call::REPO_FORMAT_ISSUES,
        graftyard_git::issue_ops::call::REPO_FORMAT_COLLABORATORS,
        REPO_FORMAT_STATELESS,
        REPO_FORMAT_AGENT,
        graftyard_git::issue_ops::call::REPO_FORMAT_APP_CALLS,
        graftyard_git::repo::REPO_FORMAT_TASKS,
        graftyard_git::repo::REPO_FORMAT_LEASES,
    ]
}

/// Three members of `repo:<id>`, owned by `OWNER`, with a leader.
pub fn repo_group(id: RepoId, now: Timestamp) -> Group<RepoCmd> {
    let mut g = Group::new(ChainId::Repo(id), repo_reads(), RepoFold::new(id, AccountId(DbId(OWNER))), now);
    g.elect(now);
    g
}

/// The install's group and the repository's group, with time.
pub struct World {
    pub now: Timestamp,
    pub install: Group<AuthzCmd>,
    pub repo: Group<RepoCmd>,
    pub ident: Ident,
    pub next: u128,
    /// The planted-bug hook for LF-2's stale class: the leader's deps from a copy of the install fold taken once.
    pub pinned: Option<AuthzFold>,
    /// Other repositories of the same owner, each its own three members, parked while `repo` is another's
    /// (`World::add_repo`, `World::switch`).
    pub parked: Vec<(RepoId, Group<RepoCmd>)>,
    pub current: RepoId,
}

pub type RepoOutcome = Result<RepoOut, RepoErr>;

impl World {
    pub fn new() -> World {
        let now = Timestamp(10_000);
        let install = install_account();
        let mut iv = Group::new(
            ChainId::Authz(install),
            vec![AUTHZ_FORMAT, AUTHZ_FORMAT_V2, AUTHZ_FORMAT_V3],
            AuthzFold::new(install),
            now,
        );
        iv.elect(now);
        let mut w = World {
            now,
            install: iv,
            repo: repo_group(REPO, now),
            ident: Ident::new(),
            next: 1,
            pinned: None,
            parked: Vec::new(),
            current: REPO,
        };
        // The install is an org whose admin flags Apps; ids for its rows; the repository's ids and its default branch.
        w.authz(AuthzOp::Open { kind: AccountKind::Org, enterprise: None }, CredentialShard::NONE).unwrap();
        let op = AuthzOp::SetMember { user: UserId(AccountId(DbId(SITE_ADMIN))), role: Some(OrgRole::Admin) };
        w.authz(op, CredentialShard::NONE).unwrap();
        w.seed_repo();
        w
    }

    /// The new repository's ids and its default branch.
    fn seed_repo(&mut self) {
        let l = self.repo.leader();
        let now = self.now;
        self.repo.on(l, now, |m, t| m.id_block(t, graftyard_types::IdBlock::whole(0, 3).unwrap())).unwrap();
        self.settle();
        self.record(vec![
            RepoEvent::Settings(settings()),
            RepoEvent::Refs {
                push: RequestId(Ulid(1)),
                actor: None,
                updates: vec![up("refs/heads/main", None, Some(oid(1)))],
                pack: None,
            },
        ]);
    }

    /// Another repository of the same owner on its own three members, seeded as `repo:7` is, and parked. The owner's
    /// `Authz` (`Ident::owner`) is the one every repository's leader reads.
    pub fn add_repo(&mut self, id: RepoId) {
        let was = self.current;
        let g = repo_group(id, self.now);
        self.parked.push((id, g));
        self.switch(id);
        self.seed_repo();
        self.switch(was);
    }

    /// Makes `id`'s group the one `commit`, `record`, and `fold` use, and settles it at the current time.
    pub fn switch(&mut self, id: RepoId) {
        if id == self.current {
            return;
        }
        let i = self.parked.iter().position(|(r, _)| *r == id).expect("a parked repository");
        let (_, g) = self.parked.remove(i);
        let old = std::mem::replace(&mut self.repo, g);
        self.parked.push((self.current, old));
        self.current = id;
        // A group parked while time moved on: its followers may have given up on the leader, so make sure one leads.
        if !self.repo.members.iter().any(|m| m.role() == Role::Leader) {
            self.repo.elect(self.now);
        }
        self.settle();
    }

    pub fn rid(&mut self) -> RequestId {
        self.next += 1;
        RequestId(Ulid(self.next << 40))
    }

    /// Delivers both groups and answers the repository leader's deps from the install leader's `fresh` read.
    pub fn settle(&mut self) {
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
    pub fn install_deps(&mut self) {
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

    pub fn authz(&mut self, op: AuthzOp, cred: CredentialShard) -> Result<(), String> {
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

    pub fn flag(&mut self, row: AgentApp) {
        self.authz(AuthzOp::PutAgentApp(row), admin()).expect("flagged");
    }

    pub fn unflag(&mut self, app: AppId) {
        self.authz(AuthzOp::DeleteAgentApp { app }, admin()).expect("unflagged");
    }

    pub fn record(&mut self, events: Vec<RepoEvent>) {
        self.record_in(REPO_FORMAT_V1, events);
    }

    /// `record` in `format`, as a host records an entry of a later format's events.
    pub fn record_in(&mut self, format: Format, events: Vec<RepoEvent>) {
        let rid = self.rid();
        let l = self.repo.leader();
        let now = self.now;
        self.repo.on(l, now, |m, t| m.record(t, rid, format, events)).unwrap();
        self.settle();
        assert!(
            self.repo.resolutions.iter().any(|r| matches!(r, Resolution::Recorded { id, result: Ok(_) } if *id == rid))
        );
    }

    /// Commits `cmds` on the repository leader as one entry, as one `/commit_many` does, and returns each outcome.
    pub fn commit_all(&mut self, cmds: Vec<RepoCmd>) -> Vec<RepoOutcome> {
        self.commit_all_acked(cmds).into_iter().map(|r| r.map(|a| a.out)).collect()
    }

    /// `commit_all` with each acknowledgement the core built, for a renderer that takes `&Acked`.
    pub fn commit_all_acked(&mut self, cmds: Vec<RepoCmd>) -> Vec<Result<graftyard_raft::Acked<RepoOut>, RepoErr>> {
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
                    Some(Ok(a)) => Ok(a),
                    Some(Err(CommitError::Refused { err, .. })) => Err(err),
                    other => panic!("unresolved: {other:?}"),
                }
            })
            .collect()
    }

    pub fn commit(&mut self, c: RepoCmd) -> RepoOutcome {
        self.commit_all(vec![c]).pop().unwrap()
    }

    pub fn fold(&mut self) -> RepoFold {
        self.repo.fresh(self.now)
    }

    pub fn get(&mut self, r: &str) -> Option<ObjectId> {
        let f = self.fold();
        tiers::get_rec(&*f.core, "ref", &[tiers::ref_key(&name(r))])
    }

    pub fn set(&mut self, r: &str, new: ObjectId) {
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

/// Registers `app` as the install's own App, the way `PutApp` commits after its `Names` slug claim, so a
/// `GraftyardRun` row for it is allowed.
pub fn register_own(w: &mut World, a: AppId, slug: &str) {
    let rec = graftyard_identity::AppRecord {
        id: a,
        slug: graftyard_identity::Slug::parse(slug).unwrap(),
        name: slug.into(),
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

pub fn admin() -> CredentialShard {
    CredentialShard { account: Some(AccountId(DbId(SITE_ADMIN))), key: None }
}

pub fn settings() -> RepoSettings {
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

pub fn up(n: &str, expect: Option<ObjectId>, new: Option<ObjectId>) -> RefUpdate {
    RefUpdate { name: name(n), expect, new }
}

/// A push by `who`. An App's push carries `validate`'s outputs for every created or moved ref, as the edge sends one
/// once 32.8 is enabled; `paths` are its changed paths.
pub fn push(who: Principal, updates: Vec<RefUpdate>, paths: &[&str]) -> graftyard_git::repo::Push {
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
        leases: false,
    }
}

pub fn issue(who: Principal, op: IssueOp<graftyard_git::blob::BlobRef>) -> RepoCmd {
    let mut c = IssueCall::new(op, who, RequestId(Ulid(0)), serde_json::Value::Null);
    // The edge sends 32.8 for the commands that may write agent events once `FormatsEnable` enabled it.
    c.agent = matches!(c.op, IssueOp::OpenPr { .. } | IssueOp::Review { .. });
    RepoCmd::Issue(Box::new(c))
}

pub fn merge(w: &mut World, who: Principal, n: u32) -> RepoCmd {
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

pub fn refs(o: &RepoOutcome) -> Vec<RefOutcome> {
    match o {
        Ok(RepoOut::Push(p)) => p.refs.iter().map(|(_, r)| r.clone()).collect(),
        other => panic!("not a push outcome: {other:?}"),
    }
}

pub fn agent_rule(o: &RepoOutcome) -> Option<AgentRule> {
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

pub fn open_pr(w: &mut World, who: Principal, branch: &str, draft: bool) -> RepoOutcome {
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

pub fn number(o: &RepoOutcome) -> u32 {
    match o {
        Ok(RepoOut::Issue(c)) => match c.out {
            IssueOut::Number(n) => n.0,
            ref x => panic!("{x:?}"),
        },
        other => panic!("{other:?}"),
    }
}

pub fn approve(w: &mut World, who: Principal, n: u32) -> RepoOutcome {
    let head = tiers::pull_head(&*w.fold().core, IssueNumber(n)).unwrap().head_sha;
    let op = IssueOp::Review { n: IssueNumber(n), state: ReviewState::Approved, commit: head, body: "".into() };
    w.commit(issue(who, op))
}
