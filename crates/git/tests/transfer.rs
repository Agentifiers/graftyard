//! BF-30's repository half on three real `graftyard_raft::Member<RepoCmd>`s: the transfer's own `Settings`
//! (format 32.10, `graftyard_git::transfer`) names the new owner and enterprise, after which a push's `deps` name them
//! and not the old owner, only the new owner's grants write, and the ordinary `Settings` still refuses an owner
//! change. A transfer `Settings` whose owner holds no reservation for the repository is refused.

use graftyard_git::issue_ops::IssueErr;
use graftyard_git::issue_ops::call::IssueCall;
use graftyard_git::issues::RepoSettings;
use graftyard_git::refs::{Atomicity, ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{
    Push, PushErr, REPO_FORMAT_V1, RefOutcome, RepoCmd, RepoErr, RepoFold, RepoOut, RepoScratch,
};
use graftyard_git::transfer::{REPO_FORMAT_TRANSFER, TransferErr, TransferSettings};
use graftyard_identity::{
    AccountKind, Actor, AuthzEvent, AuthzFold, Credential, IdentityDep, NameEpoch, NamesEvent, NamesFold, OrgRole,
    Principal, RepoEntry, RepoName, RepoPermission, ScopeSet, TokenKind, TokensEvent, TokensFold, UserId,
};
use graftyard_raft::{
    Command, CommitError, Envelope, Ev, Fold as _, Member, MemberConfig, ReleaseTag, Resolution, Role, Stored,
};
use graftyard_types::{
    AccountId, ChainId, DbId, NameShard, PlacementName, PlacementRecord, Region, ReleaseId, RepoId, RequestId, SiteId,
    Timestamp, TokenShard, Ulid,
};
use smallvec::SmallVec;

const SITES: [SiteId; 3] = [SiteId(0), SiteId(1), SiteId(2)];
const REPO: RepoId = RepoId(DbId(7));
const ALPHA: AccountId = AccountId(DbId(100));
const BETA: AccountId = AccountId(DbId(200));
const E1: AccountId = AccountId(DbId(300));
const E2: AccountId = AccountId(DbId(301));
/// Alpha's admin.
const OLD_ADMIN: i64 = 10;
/// Granted `Write` on the repository by beta only.
const NEW_WRITER: i64 = 11;

fn config(me: SiteId) -> MemberConfig {
    MemberConfig {
        chain: ChainId::Repo(REPO),
        me,
        record: PlacementRecord {
            name: PlacementName::Tri,
            sites: SITES.iter().copied().collect(),
            home: SITES[0],
            generation: 1,
            moving_from: None,
            at: Timestamp(1),
        },
        regions: vec![(SITES[0], Region::Enam), (SITES[1], Region::Weur), (SITES[2], Region::Apac)],
        release: ReleaseTag { id: ReleaseId([3; 32]), seq: 1 },
        reads: vec![REPO_FORMAT_V1, graftyard_git::issue_ops::call::REPO_FORMAT_ISSUES, REPO_FORMAT_TRANSFER],
    }
}

fn who(account: i64) -> Principal {
    Principal {
        actor: Actor::User(UserId(AccountId(DbId(account)))),
        scopes: ScopeSet(1),
        credential: Credential::Stored([account as u8; 32]),
        sso_orgs: SmallVec::new(),
    }
}

fn org(o: AccountId, e: Option<AccountId>) -> AuthzFold {
    let mut f = AuthzFold::new(o);
    f.apply(&AuthzEvent::Opened { kind: AccountKind::Org, enterprise: e });
    f
}

/// The folds the leader reads through `fresh`: the users' accounts and tokens, alpha (OLD_ADMIN its admin) in E1,
/// beta in E2 holding the repository's name as `reserved` says, and both enterprises.
#[derive(Clone)]
struct Deps {
    beta: AuthzFold,
    alpha: AuthzFold,
}

impl Deps {
    fn new() -> Deps {
        let mut alpha = org(ALPHA, Some(E1));
        alpha.apply(&AuthzEvent::Member { user: UserId(AccountId(DbId(OLD_ADMIN))), role: Some(OrgRole::Admin) });
        let tool = RepoName::parse("tool").unwrap().key();
        alpha.apply(&AuthzEvent::RepoName {
            key: tool,
            entry: Some(RepoEntry::Live { repo: REPO, epoch: NameEpoch(1) }),
        });
        let mut beta = org(BETA, Some(E2));
        let w = UserId(AccountId(DbId(NEW_WRITER)));
        beta.apply(&AuthzEvent::Member { user: w, role: Some(OrgRole::Member) });
        beta.apply(&AuthzEvent::Grant { repo: REPO, grantee: w, permission: Some(RepoPermission::Write) });
        Deps { beta, alpha }
    }

    fn reserve(&mut self) {
        let key = RepoName::parse("tool").unwrap().key();
        self.beta.apply(&AuthzEvent::RepoName { key, entry: Some(RepoEntry::Reserved { repo: REPO, from: ALPHA }) });
    }

    fn read(&self, c: ChainId) -> IdentityDep {
        match c {
            ChainId::Names(_) => {
                let mut n = NamesFold::new(NameShard(0));
                for a in [OLD_ADMIN, NEW_WRITER] {
                    let id = AccountId(DbId(a));
                    n.apply(&NamesEvent::Account { id, suspended: false, sessions_not_before: Timestamp(0) });
                }
                IdentityDep::Names(n)
            }
            ChainId::Tokens(s) => {
                let mut t = TokensFold::new(TokenShard(0));
                for a in [OLD_ADMIN, NEW_WRITER] {
                    t.apply(&TokensEvent::Stored {
                        hash: [a as u8; 32],
                        kind: TokenKind::Pat,
                        account: AccountId(DbId(a)),
                        scopes: ScopeSet(1),
                        expires: None,
                    });
                }
                t.shard = Some(s);
                IdentityDep::Tokens(t)
            }
            ChainId::Authz(o) if o == ALPHA => IdentityDep::Authz(self.alpha.clone()),
            ChainId::Authz(o) if o == BETA => IdentityDep::Authz(self.beta.clone()),
            ChainId::Authz(o) => IdentityDep::Authz(org(o, None)),
            other => panic!("no dep {other}"),
        }
    }
}

struct G {
    now: Timestamp,
    members: Vec<Option<Member<RepoCmd>>>,
    disks: Vec<Stored<RepoFold>>,
    inbox: Vec<Envelope<Ev<RepoCmd>>>,
    resolutions: Vec<Resolution<RepoCmd>>,
    deps: Deps,
    /// Every dependency set a leader asked for, in order.
    asked: Vec<Vec<ChainId>>,
    boots: u64,
    next: u128,
}

impl G {
    fn new() -> G {
        let mut g = G {
            now: Timestamp(10_000),
            members: Vec::new(),
            disks: SITES.iter().map(|_| Stored::new(RepoFold::new(REPO, ALPHA))).collect(),
            inbox: Vec::new(),
            resolutions: Vec::new(),
            deps: Deps::new(),
            asked: Vec::new(),
            boots: 0,
            next: 1,
        };
        for s in SITES {
            g.boots += 1;
            let (m, ready) =
                Member::restore(config(s), g.disks[s.0 as usize].clone(), g.now, g.boots).expect("restore");
            g.members.push(Some(m));
            let ((), out) = ready.persist(&mut g.disks[s.0 as usize]);
            g.inbox.extend(out.early.into_iter().chain(out.messages));
        }
        for _ in 0..3 {
            g.on(SITES[0], |m, now| m.campaign(now));
            g.deliver();
            if g.m(SITES[0]).role() == Role::Leader {
                break;
            }
        }
        assert_eq!(g.m(SITES[0]).role(), Role::Leader);
        g.pass(2_000);
        g
    }

    fn m(&self, s: SiteId) -> &Member<RepoCmd> {
        self.members[s.0 as usize].as_ref().expect("member is up")
    }

    fn on<R>(
        &mut self,
        s: SiteId,
        f: impl FnOnce(&mut Member<RepoCmd>, Timestamp) -> graftyard_raft::Ready<RepoCmd, R>,
    ) -> R {
        let now = self.now;
        let m = self.members[s.0 as usize].as_mut().expect("member is up");
        let (r, out) = f(m, now).persist(&mut self.disks[s.0 as usize]);
        let mut outs = vec![out];
        while let Some(mut out) = outs.pop() {
            self.inbox.extend(out.early.drain(..).chain(out.messages.drain(..)));
            self.resolutions.append(&mut out.resolutions);
            let m = self.members[s.0 as usize].as_mut().expect("member is up");
            let disk = &mut self.disks[s.0 as usize];
            if let Some(a) = out.self_ack.take() {
                outs.push(m.self_ack(now, a).persist(disk).1);
            }
            for d in out.deps.drain(..) {
                self.asked.push(d.chains.to_vec());
                let read: Vec<_> = d.chains.iter().map(|c| (*c, self.deps.read(*c))).collect();
                outs.push(m.deps_read(now, d.id, read).persist(disk).1);
            }
        }
        r
    }

    fn deliver(&mut self) {
        for _ in 0..10_000 {
            if self.inbox.is_empty() {
                return;
            }
            for e in std::mem::take(&mut self.inbox) {
                if self.members[e.to.0 as usize].is_none() {
                    continue;
                }
                let Envelope { from, to, msg } = e;
                self.on(to, |m, now| m.step(now, from, msg));
            }
        }
        panic!("messages never settled");
    }

    fn pass(&mut self, ms: u64) {
        let end = self.now.0 + ms;
        while self.now.0 < end {
            self.now = Timestamp((self.now.0 + 1_000).min(end));
            for s in SITES {
                if self.members[s.0 as usize].is_some() {
                    self.on(s, |m, now| m.tick(now));
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
        let a = self.on(SITES[0], |m, now| m.commit(now, rid, cmd)).expect("admitted");
        self.pass(2_000);
        let r = self.resolutions.iter().rev().find_map(|r| match r {
            Resolution::Ticket { ticket, result } if *ticket == a.ticket => Some(result.clone()),
            _ => None,
        });
        match r {
            Some(Ok(a)) => Ok(a.out),
            Some(Err(CommitError::Refused { err, .. })) => Err(err),
            other => panic!("unresolved: {:?}", other.map(|r| r.is_ok())),
        }
    }

    fn transfer(&mut self, owner: AccountId, enterprise: Option<AccountId>) -> Result<RepoOut, RepoErr> {
        let request = self.rid();
        self.commit(RepoCmd::Transfer(TransferSettings { owner, enterprise, request }))
    }

    fn push(&mut self, account: i64, n: u8) -> Result<RepoOut, RepoErr> {
        let name = RefName::parse(format!("refs/heads/b{n}").as_bytes()).unwrap();
        let p = Push {
            updates: vec![RefUpdate { name, expect: None, new: Some(ObjectId([n; 20])) }],
            atomicity: Atomicity::PerRef,
            pack: Some(graftyard_git::pack::PackRef::Inline(b"PACK\0\0\0\x02\0\0\0\0".to_vec())),
            actor: who(account),
            request: RequestId(Ulid(u128::from(n) << 64 | 1)),
            options: Vec::new(),
            agent: None,
            provenance: Vec::new(),
            protected: Vec::new(),
            changed: Vec::new(),
            leases: false,
        };
        self.commit(RepoCmd::Push(p))
    }

    fn settings(&self) -> Option<RepoSettings> {
        let s: RepoScratch<'_> = self.m(SITES[0]).group_fold().fold.scratch();
        graftyard_git::tiers::get_rec(s.core(), "settings", &[])
    }

    /// The `deps` a push would name now, from the leader's fold.
    fn push_deps(&self) -> Vec<ChainId> {
        RepoCmd::Push(Push {
            updates: Vec::new(),
            atomicity: Atomicity::PerRef,
            pack: None,
            actor: who(NEW_WRITER),
            request: RequestId(Ulid(9)),
            options: Vec::new(),
            agent: None,
            provenance: Vec::new(),
            protected: Vec::new(),
            changed: Vec::new(),
            leases: false,
        })
        .deps(&self.m(SITES[0]).group_fold().fold)
        .to_vec()
    }

    fn agree(&self) {
        let d = |s: SiteId| self.m(s).digests().to_vec();
        for s in &SITES[1..] {
            assert!(d(*s).len() <= d(SITES[0]).len() && d(SITES[0]).starts_with(&d(*s)), "{s:?} diverged");
        }
    }
}

fn pushed(r: &Result<RepoOut, RepoErr>) -> bool {
    matches!(r, Ok(RepoOut::Push(o)) if o.refs.iter().all(|(_, x)| *x == RefOutcome::Ok))
}

fn refused(r: &Result<RepoOut, RepoErr>) -> bool {
    match r {
        Ok(RepoOut::Push(o)) => o.refs.iter().all(|(_, x)| *x != RefOutcome::Ok),
        Err(RepoErr::Push(PushErr::Forbidden | PushErr::Unauthorized(_))) => true,
        _ => false,
    }
}

#[test]
fn the_transfer_settings_moves_deps_to_the_new_owner_and_its_enterprise() {
    let mut g = G::new();
    assert!(pushed(&g.push(OLD_ADMIN, 1)), "before: alpha's admin writes");
    assert!(refused(&g.push(NEW_WRITER, 2)), "before: beta's grant does not reach");
    assert!(g.push_deps().contains(&ChainId::Authz(ALPHA)));

    // No reservation on beta yet: the transfer's `Settings` is refused, and nothing changed.
    assert_eq!(g.transfer(BETA, Some(E2)), Err(RepoErr::Transfer(TransferErr::NotReserved)));
    assert_eq!(g.settings(), None);

    g.deps.reserve();
    // The enterprise must be the one beta's chain names.
    assert_eq!(g.transfer(BETA, Some(E1)), Err(RepoErr::Transfer(TransferErr::WrongEnterprise)));
    let asked_before = g.asked.len();
    match g.transfer(BETA, Some(E2)) {
        Ok(RepoOut::Transfer(o)) => assert!(o.changed && o.owner == BETA && o.enterprise == Some(E2)),
        other => panic!("{other:?}"),
    }
    assert_eq!(g.asked[asked_before..].concat(), vec![ChainId::Authz(BETA)], "it reads only the new owner's chain");
    let s = g.settings().expect("a settings row");
    assert_eq!((s.owner, s.enterprise), (BETA, Some(E2)));
    assert_eq!(s.check_retention_days, 90, "the other fields take the defaults");

    // `deps` name the new owner and enterprise, and never the old owner.
    let d = g.push_deps();
    assert!(d.contains(&ChainId::Authz(BETA)) && d.contains(&ChainId::Authz(E2)), "{d:?}");
    assert!(!d.contains(&ChainId::Authz(ALPHA)) && !d.contains(&ChainId::Authz(E1)), "stale owner in {d:?}");
    assert!(pushed(&g.push(NEW_WRITER, 3)), "after: beta's grant writes");
    assert!(refused(&g.push(OLD_ADMIN, 4)), "after: alpha's admin no longer writes");

    // Re-sent, it writes nothing.
    match g.transfer(BETA, Some(E2)) {
        Ok(RepoOut::Transfer(o)) => assert!(!o.changed),
        other => panic!("{other:?}"),
    }

    // The ordinary `Settings` still refuses an owner change, even from the new owner's grantee.
    let back = RepoSettings { owner: ALPHA, ..s.clone() };
    let call = IssueCall::new(
        graftyard_git::issue_ops::IssueOp::Settings(back),
        who(NEW_WRITER),
        g.rid(),
        serde_json::Value::Null,
    );
    match g.commit(RepoCmd::Issue(Box::new(call))) {
        Err(RepoErr::Issue { err, .. }) => {
            assert!(matches!(err, IssueErr::OwnerChange | IssueErr::Forbidden), "{err:?}")
        }
        other => panic!("{other:?}"),
    }
    g.pass(4_000);
    g.agree();
}

/// An org's enterprise change: the org keeps its repository, and its `Settings` names the new enterprise.
#[test]
fn an_enterprise_change_moves_only_the_enterprise() {
    let mut g = G::new();
    match g.transfer(ALPHA, Some(E1)) {
        Ok(RepoOut::Transfer(o)) => assert!(o.changed),
        other => panic!("{other:?}"),
    }
    g.deps.alpha = {
        let mut a = g.deps.alpha.clone();
        a.apply(&AuthzEvent::Enterprise { enterprise: Some(E2) });
        a
    };
    assert_eq!(g.transfer(ALPHA, Some(E1)), Err(RepoErr::Transfer(TransferErr::WrongEnterprise)));
    assert!(g.transfer(ALPHA, Some(E2)).is_ok());
    let s = g.settings().unwrap();
    assert_eq!((s.owner, s.enterprise), (ALPHA, Some(E2)));
    let d = g.push_deps();
    assert!(d.contains(&ChainId::Authz(E2)) && !d.contains(&ChainId::Authz(E1)), "{d:?}");
    assert!(pushed(&g.push(OLD_ADMIN, 5)));
}

/// The wire form: `{"transfer": {"format": [32, 10], ..}}`; a member that reads another format refuses it.
#[test]
fn the_transfer_settings_wire_form_names_its_format_first() {
    let c = RepoCmd::Transfer(TransferSettings { owner: BETA, enterprise: Some(E2), request: RequestId(Ulid(5)) });
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["transfer"]["format"], serde_json::json!([32, 10]));
    let back: RepoCmd = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(back, c);
    assert_eq!(back.format(), REPO_FORMAT_TRANSFER);
    let mut old = v;
    old["transfer"]["format"] = serde_json::json!([32, 9]);
    assert!(serde_json::from_value::<RepoCmd>(old).is_err());
}
