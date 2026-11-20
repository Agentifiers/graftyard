//! A repository chain for the surface tests: members applying one log to both tiers, commands decided the way the
//! leader decides them (`crates/git/tests/issues.rs` drives the same model), and the `Directory` the edge would read.
//!
//! The repository command does not commit through a Raft member yet (M2-WP1 wires it), so a mutation's `&Acked` comes
//! from `ack`: a three-member group of the real Raft core that commits a command whose output is the one the
//! repository command decided. The core alone builds `Acked`, so a renderer here still runs only after a quorum.

#![allow(dead_code)]

use std::collections::BTreeMap;

use graftyard_api::view::{Compare, Directory, Repo, RepoRef};
use graftyard_api::wire::Host;
use graftyard_fold::{MemStore, apply};
use graftyard_git::issue_ops::{IssueCmd, IssueErr, IssueOp, IssueOut, RepoView};
use graftyard_git::refs::{ObjectId, RefName, RefUpdate};
use graftyard_git::repo::RepoEvent;
use graftyard_git::tiers::{RepoTiers, tier_of};
use graftyard_identity::{
    AccountKind, Actor, AuthzEvent, AuthzFold, Credential, IdentityDep, NameEpoch, NamesEvent, NamesFold, Principal,
    RepoEntry, RepoName, RepoPermission, ScopeSet, TokenKind, TokensEvent, TokensFold, UserId, credential_chains,
};
use graftyard_raft::{
    Acked, Command, CommitError, EntryBody, Ev, Fold, Format, LogEntry, Member, MemberConfig, PREV_ZERO, Record,
    ReleaseTag, Resolution, Role, Scratch, Stored, Term, TierSet, digest,
};
use graftyard_types::{
    AccountId, ChainId, DbId, IdBlock, Index, NameShard, PlacementName, PlacementRecord, Region, ReleaseId, RepoId,
    RequestId, SiteId, Tier, Timestamp, TokenShard, Ulid,
};
use smallvec::{SmallVec, smallvec};

pub const REPO: RepoId = RepoId(DbId(7));
pub const OWNER: i64 = 100;
pub const WRITER: i64 = 101;
pub const READER: i64 = 102;
pub const DAY: u64 = 86_400_000;

pub fn tiers() -> RepoTiers {
    RepoTiers { repo: REPO, owner: AccountId(DbId(OWNER)) }
}

pub fn oid(n: u8) -> ObjectId {
    ObjectId([n; 20])
}

pub fn name(s: &str) -> RefName {
    RefName::parse(s.as_bytes()).unwrap()
}

pub fn who(account: i64) -> Principal {
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
    // The owner names the repository `Live`, which an `InitRepo` needs (F59: a create the finisher aborted is not).
    authz.apply(&AuthzEvent::RepoName {
        key: RepoName::parse("tool").unwrap().key(),
        entry: Some(RepoEntry::Live { repo: REPO, epoch: NameEpoch(1) }),
    });
    for a in [OWNER, WRITER, READER] {
        let id = AccountId(DbId(a));
        names.apply(&NamesEvent::Account { id, suspended: false, sessions_not_before: Timestamp(0) });
        tokens.apply(&TokensEvent::Stored {
            hash: [a as u8; 32],
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
        for c in credential_chains(&who(a).credential_shard(), None) {
            let d = match c {
                ChainId::Names(_) => IdentityDep::Names(names.clone()),
                ChainId::Tokens(s) => {
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

/// One member's two tiers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Member2 {
    pub core: MemStore,
    pub meta: MemStore,
}

impl Member2 {
    fn apply(&mut self, e: &LogEntry<RepoEvent>) {
        apply(&tiers(), &mut self.core, Tier::Core, e).unwrap();
        apply(&tiers(), &mut self.meta, Tier::Metadata, e).unwrap();
    }
}

/// A repository chain applied by `members` members.
pub struct Chain {
    pub members: Vec<Member2>,
    pub log_len: u64,
    pub last: graftyard_types::Digest,
    pub now: Timestamp,
    /// Every committed entry, in order, as a consumer reads the log.
    pub log: Vec<LogEntry<RepoEvent>>,
    deps: Vec<(ChainId, IdentityDep)>,
    next_request: u128,
}

impl Chain {
    pub fn new(members: usize) -> Chain {
        let mut c = Chain {
            members: vec![Member2::default(); members],
            log_len: 0,
            last: PREV_ZERO,
            now: Timestamp(1_793_000_000_000),
            log: Vec::new(),
            deps: deps(),
            next_request: 1,
        };
        c.entry(EntryBody::IdBlock(IdBlock::whole(0, 1).unwrap()), TierSet::CORE);
        c
    }

    pub fn entry(&mut self, body: EntryBody<RepoEvent>, tiers: TierSet) {
        let e = LogEntry {
            term: Term(2),
            index: Index(self.log_len + 1),
            prev: self.last,
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
        self.log_len += 1;
        self.last = digest(&e);
        self.log.push(e);
    }

    /// Decides `ops` in order as one entry on member 0's tiers, as the leader does, and commits what passed.
    pub fn commit_all(&mut self, ops: Vec<(i64, IssueOp)>) -> Vec<Result<IssueOut, IssueErr>> {
        let m = &self.members[0];
        let mut view = RepoView::new(tiers(), &m.core, &m.meta, self.now);
        let (mut events, mut outs) = (Vec::new(), Vec::new());
        for (actor, op) in ops {
            let cmd = IssueCmd { op, actor: who(actor), request: RequestId(Ulid(self.next_request)) };
            self.next_request += 1;
            let wanted: Vec<ChainId> = cmd.deps(&view).into_iter().collect();
            let d: Vec<_> = wanted.iter().map(|c| self.deps.iter().find(|(k, _)| k == c).unwrap().clone()).collect();
            match cmd.decide(&view, &d) {
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

    /// Decides `m` on member 0's tiers as the leader does and commits its events as one entry.
    pub fn commit_merge(
        &mut self,
        m: graftyard_git::merge::Merge,
    ) -> Result<graftyard_git::merge::MergeOut, graftyard_git::merge::MergeError> {
        let mem = &self.members[0];
        let view = RepoView::new(tiers(), &mem.core, &mem.meta, self.now);
        let wanted: Vec<ChainId> = m.deps(Some(view.owner()), None).into_iter().collect();
        let d: Vec<_> = wanted.iter().map(|c| self.deps.iter().find(|(k, _)| k == c).unwrap().clone()).collect();
        let (events, out) = m.decide(&view, &d)?;
        drop(view);
        let tag = events.iter().fold(TierSet::NONE, |t, e| t.union(tier_of(e)));
        self.entry(EntryBody::Events(events.into_vec()), tag);
        Ok(out)
    }

    /// Revokes `account`'s token on every `Tokens` dep the leader reads, as a `Revoked` entry on its shard does.
    pub fn revoke(&mut self, account: i64) {
        for (_, d) in &mut self.deps {
            if let IdentityDep::Tokens(t) = d {
                t.apply(&TokensEvent::Revoked { hash: [account as u8; 32] });
            }
        }
    }

    pub fn commit(&mut self, actor: i64, op: IssueOp) -> Result<IssueOut, IssueErr> {
        self.commit_all(vec![(actor, op)]).pop().unwrap()
    }

    /// An outside collaborator's row, as an accepted invitation's entry writes it.
    pub fn collaborator(&mut self, account: i64, permission: RepoPermission) {
        let ev = RepoEvent::Collaborator { who: AccountId(DbId(account)), permission: Some(permission) };
        self.entry(EntryBody::Events(vec![ev]), TierSet::CORE);
    }

    pub fn push(&mut self, branch: &str, new: ObjectId) {
        let r = name(branch);
        let m = &self.members[0];
        let expect = RepoView::new(tiers(), &m.core, &m.meta, self.now).ref_value(&r);
        let ev = RepoEvent::Refs {
            push: RequestId(Ulid(9)),
            actor: Some(AccountId(DbId(WRITER))),
            updates: vec![RefUpdate { name: r, expect, new: Some(new) }],
            pack: None,
        };
        self.entry(EntryBody::Events(vec![ev]), TierSet::CORE);
    }

    /// Member `i`'s tiers as the handlers read them.
    pub fn repo<'a>(&'a self, i: usize, host: &'a Host, dir: &'a Dir) -> Repo<'a> {
        let m = &self.members[i];
        Repo { host, at: dir.home(), core: &m.core, meta: &m.meta, dir }
    }
}

/// The accounts and the signed-in viewer, as the edge would read them from `Names`, `Identity`, and `Authz`.
pub struct Dir {
    pub viewer: Option<AccountId>,
    logins: BTreeMap<i64, &'static str>,
}

impl Dir {
    pub fn new(viewer: i64) -> Dir {
        let logins = [(OWNER, "o"), (WRITER, "alice"), (READER, "bob")].into_iter().collect();
        Dir { viewer: Some(AccountId(DbId(viewer))), logins }
    }

    pub fn home(&self) -> RepoRef {
        RepoRef { id: REPO, owner: "o".into(), owner_id: AccountId(DbId(OWNER)), name: "r".into() }
    }
}

impl Directory for Dir {
    fn login(&self, id: AccountId) -> Option<String> {
        self.logins.get(&id.0.0).map(|s| s.to_string())
    }
    fn name(&self, id: AccountId) -> Option<String> {
        self.login(id).map(|l| l.to_uppercase())
    }
    fn is_org(&self, _: AccountId) -> bool {
        false
    }
    fn by_login(&self, login: &str) -> Option<AccountId> {
        self.logins.iter().find(|(_, l)| **l == login).map(|(id, _)| AccountId(DbId(*id)))
    }
    fn permission(&self, id: AccountId) -> Option<RepoPermission> {
        match id.0.0 {
            OWNER => Some(RepoPermission::Admin),
            WRITER => Some(RepoPermission::Write),
            READER => Some(RepoPermission::Read),
            _ => None,
        }
    }
    fn repo(&self, id: RepoId) -> Option<RepoRef> {
        (id == REPO).then(|| self.home())
    }
    fn ref_of(&self, _: RepoId, _: &RefName) -> Option<ObjectId> {
        None
    }
    fn team(&self, _: DbId) -> Option<(String, String, String)> {
        None
    }
    fn viewer(&self) -> Option<AccountId> {
        self.viewer
    }
    fn compare(&self, _: ObjectId, _: ObjectId) -> Compare {
        Compare { commits: 1, additions: 3, deletions: 1, changed_files: 1 }
    }
}

// ---- a real `Acked` ----

/// A command whose output is fixed: what the repository command decided.
#[derive(Clone, Debug, PartialEq)]
struct Decided<T>(T);

#[derive(Clone, Debug, Default, PartialEq)]
struct Nothing;

#[derive(Clone, Debug, PartialEq)]
struct NoEvent;

impl Record for NoEvent {
    fn encode(&self, _: &mut Vec<u8>) {}
}

struct NoScratch;

impl Scratch for NoScratch {
    type Event = NoEvent;
    fn apply(&mut self, _: &NoEvent) {}
    fn check(&self, _: &NoEvent) -> Result<(), Box<str>> {
        Ok(())
    }
}

impl Fold for Nothing {
    type Event = NoEvent;
    type Scratch<'a> = NoScratch;
    fn scratch(&self) -> NoScratch {
        NoScratch
    }
    fn apply(&mut self, _: &NoEvent) {}
}

impl<T> Record for Decided<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(0);
    }
}

const FORMAT: Format = Format { family: 0x7f, version: 1 };

impl<T: Clone + std::fmt::Debug + PartialEq> Command for Decided<T> {
    type Fold = Nothing;
    type Dep = ();
    type Out = T;
    type Err = ();
    fn format(&self) -> Format {
        FORMAT
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, _: &Nothing) -> SmallVec<[ChainId; 3]> {
        SmallVec::new()
    }
    fn decide(&self, _: &NoScratch, _: &[(ChainId, ())]) -> Result<(SmallVec<[Ev<Self>; 4]>, T), ()> {
        Ok((smallvec![NoEvent], self.0.clone()))
    }
    fn replay(&self, _: &[NoEvent]) -> T {
        self.0.clone()
    }
}

/// What `ack` does with one member's output: queue its messages, record the ticket, and run its self-ack.
type Take<'a, T> = dyn FnMut(
        usize,
        &mut Vec<Member<Decided<T>>>,
        &mut Vec<Stored<Nothing>>,
        &mut Vec<graftyard_raft::Envelope<NoEvent>>,
        graftyard_raft::Out<Decided<T>>,
    ) + 'a;

/// `out` acknowledged by the Raft core after its entry committed on a three-member `Tri` group whose messages are
/// delivered at once, as `crates/identity/tests/common`'s `Group` drives one.
pub fn ack<T: Clone + std::fmt::Debug + PartialEq>(out_value: T) -> Acked<T> {
    let sites = [SiteId(0), SiteId(1), SiteId(2)];
    let chain = ChainId::Repo(RepoId(DbId(99)));
    let record = PlacementRecord {
        name: PlacementName::Tri,
        sites: sites.iter().copied().collect(),
        home: sites[0],
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    };
    let cfg = |me| MemberConfig {
        chain,
        me,
        record: record.clone(),
        regions: vec![(sites[0], Region::Enam), (sites[1], Region::Weur), (sites[2], Region::Apac)],
        release: ReleaseTag { id: ReleaseId([7; 32]), seq: 1 },
        reads: vec![FORMAT],
    };
    let mut ms: Vec<Member<Decided<T>>> =
        sites.iter().map(|s| Member::new(cfg(*s), 1 + u64::from(s.0)).unwrap()).collect();
    let mut disks: Vec<Stored<Nothing>> = sites.iter().map(|_| Stored::new(Nothing)).collect();
    let now = Timestamp(10_000);
    let mut inbox = Vec::new();
    let mut acked = None;
    let mut take = |i: usize,
                    ms: &mut Vec<Member<Decided<T>>>,
                    disks: &mut Vec<Stored<Nothing>>,
                    inbox: &mut Vec<graftyard_raft::Envelope<NoEvent>>,
                    mut out: graftyard_raft::Out<Decided<T>>| loop {
        inbox.extend(out.early.drain(..).chain(out.messages.drain(..)));
        for res in out.resolutions.drain(..) {
            if let Resolution::Ticket { result, .. } = res {
                acked = Some(result);
            }
        }
        let Some(a) = out.self_ack.take() else { break };
        out = ms[i].self_ack(now, a).persist(&mut disks[i]).1;
    };
    let deliver = |ms: &mut Vec<Member<Decided<T>>>,
                   disks: &mut Vec<Stored<Nothing>>,
                   inbox: &mut Vec<graftyard_raft::Envelope<NoEvent>>,
                   take: &mut Take<'_, T>| {
        while !inbox.is_empty() {
            for e in std::mem::take(inbox) {
                let i = e.to.0 as usize;
                let (_, out) = ms[i].step(now, e.from, e.msg).persist(&mut disks[i]);
                take(i, ms, disks, inbox, out);
            }
        }
    };
    for _ in 0..2 {
        if ms[0].role() != Role::Leader {
            let (_, out) = ms[0].campaign(now).persist(&mut disks[0]);
            take(0, &mut ms, &mut disks, &mut inbox, out);
            deliver(&mut ms, &mut disks, &mut inbox, &mut take);
        }
    }
    assert_eq!(ms[0].role(), Role::Leader);
    let (admitted, out) = ms[0].commit(now, RequestId(Ulid(1)), Decided(out_value)).persist(&mut disks[0]);
    admitted.expect("admitted");
    take(0, &mut ms, &mut disks, &mut inbox, out);
    deliver(&mut ms, &mut disks, &mut inbox, &mut take);
    match acked.expect("the entry committed") {
        Ok(a) => a,
        Err(CommitError::Refused { .. } | CommitError::NotLeader(_) | CommitError::Full { .. }) => {
            panic!("not acknowledged")
        }
    }
}

// ---- the git cache ----

/// A plan from the git cache for `req`: the merge commit is a function of the pair, and the tree-diff check passed.
pub fn cache_plan(req: &graftyard_git::merge::MergeRequest) -> graftyard_git::merge::MergePlan {
    use graftyard_git::merge::{EngineVersion, MergePlan, TreeDiffDigest};
    let commit = graftyard_git::refs::ObjectId(std::array::from_fn(|i| req.base_old.0[i] ^ req.head.0[i] ^ 0x5a));
    MergePlan {
        base_old: req.base_old,
        head: req.head,
        commit,
        pack: inline_pack(&commit.0),
        method: req.method,
        up_to_date: true,
        verified: TreeDiffDigest(graftyard_types::Digest([1; 32])),
        engine: EngineVersion("git 2.51".into()),
    }
}

pub fn inline_pack(bytes: &[u8]) -> graftyard_git::DurablePack {
    use graftyard_git::mem::MemBucket;
    use graftyard_types::{Region, Site, SiteId, SiteLink, SiteTable};
    use sha2::Digest as _;
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
    let sites = SiteTable(vec![site(0, Region::Enam), site(1, Region::Weur)]);
    let a = MemBucket::default();
    let bk = [(SiteId(0), &a)];
    let store = graftyard_git::PackStore { sites: &sites, buckets: &bk, copies: graftyard_git::Copies::TwoRegions };
    let index = graftyard_git::PackIndex {
        checksum: graftyard_types::Digest(sha2::Sha256::digest(bytes).into()),
        bytes: bytes.len() as u64,
        objects: 1,
        trailer: "t".into(),
    };
    let place = graftyard_git::Place {
        network: graftyard_types::NetworkId(DbId(1)),
        id: graftyard_git::PackId(graftyard_types::Ulid(1)),
        meta: graftyard_git::ObjectMeta {
            chain: graftyard_types::ChainId::Repo(REPO),
            request: graftyard_types::RequestId(graftyard_types::Ulid(1)),
        },
        bytes,
        index: &index,
        idx: None,
        idx_range: None,
        inline_max: graftyard_git::INLINE_MAX,
    };
    futures_executor::block_on(store.place(place)).unwrap()
}

// ---- the trust view's byte identity (AM1-WP5) ----

/// Every `Accept` the captured clients sent (gh's GraphQL and REST, git-lfs, release downloads), the documented
/// GitHub media types, and none.
pub const ACCEPTS: &[Option<&str>] = &[
    None,
    Some("application/vnd.github.merge-info-preview+json, application/vnd.github.nebula-preview"),
    Some(
        "application/vnd.github.merge-info-preview+json, application/vnd.github.nebula-preview, application/vnd.github.antiope-preview",
    ),
    Some("application/vnd.git-lfs+json"),
    Some("application/octet-stream, application/json"),
    Some("application/octet-stream"),
    Some("application/vnd.github+json"),
    Some("application/vnd.github.v3+json"),
    Some("application/vnd.github.raw+json"),
    Some("application/vnd.github.html+json"),
    Some("application/vnd.github.full+json"),
    Some("application/json"),
    Some("*/*"),
];

/// The bytes the edge puts on the wire for a handler's reply: status, every header in order, and the body as
/// `serde_json::to_vec` writes it (`workers/edge/src/api.rs`, `frame`).
pub fn framed(accept: Option<&str>, surface: graftyard_wire::Surface, reply: graftyard_wire::Reply) -> Vec<u8> {
    use graftyard_wire::{ApiVersion, Body, Incoming, Meta, RateLimits, finish};
    let i = Incoming { user_agent: Some("GitHub CLI 2.102.1"), accept, ..Incoming::default() };
    let m =
        Meta { rid: RequestId(Ulid(0x0123_4567_89ab_cdef)), version: ApiVersion::V2022_11_28, rate: RateLimits::Off };
    let r = finish(&i, reply, surface, &m);
    let mut out = format!("{}\n", r.status).into_bytes();
    for (k, v) in &r.headers {
        out.extend_from_slice(format!("{k}: {v}\n").as_bytes());
    }
    out.push(b'\n');
    match r.body {
        Body::Json(v) => out.extend(serde_json::to_vec(&v).unwrap()),
        Body::Bytes(b) => out.extend(b),
        Body::Empty => {}
    }
    out
}

/// Checks one handler reply as the edge sends it, for every `Accept` in `ACCEPTS`: the bytes with the trust view in the
/// edge's path equal the bytes without it. `rest` is a REST reply's method and OpenAPI path template, which the edge
/// hands to `trust::layer` as a concrete path under `/api/v3`; `None` is a GraphQL or LFS reply, which the edge never
/// layers, so its frame must not change under the agent media type either.
///
/// Without the media type `layer` reads no row, so the repository it gets here is empty. When `$GRAFTYARD_REPLIES` is
/// set and the route is a trust resource, the opted-in body is written as `<case>.agent.rest.json` for `validate.py`.
pub fn same_bytes_without_opt_in(
    case: &str,
    surface: graftyard_wire::Surface,
    rest: Option<(&str, &str)>,
    reply: &graftyard_wire::Reply,
) {
    let rest_path = rest.map(|(_, t)| t);
    let (core, meta, dir, host) =
        (MemStore::default(), MemStore::default(), Dir::new(WRITER), Host::new("ghes.example.com", false).unwrap());
    let r = Repo { host: &host, at: dir.home(), core: &core, meta: &meta, dir: &dir };
    let path = rest_path.map(concrete);
    for accept in ACCEPTS {
        let before = framed(*accept, surface, reply.clone());
        let layered = match &path {
            Some(p) => graftyard_api::trust::layer(*accept, p, &r, reply.clone()),
            None => reply.clone(),
        };
        let after = framed(*accept, surface, layered);
        assert!(
            before == after,
            "{case}: {path:?} with Accept {accept:?} changed:\n{}\n---\n{}",
            String::from_utf8_lossy(&before),
            String::from_utf8_lossy(&after)
        );
    }
    let agent = Some(graftyard_api::trust::AGENT_MEDIA_TYPE);
    match (&path, rest_path) {
        (None, _) => assert!(framed(agent, surface, reply.clone()) == framed(None, surface, reply.clone()), "{case}"),
        (Some(p), Some(template)) => {
            let on = graftyard_api::trust::layer(agent, p, &r, reply.clone());
            if on != *reply
                && let Ok(dir) = std::env::var("GRAFTYARD_REPLIES")
            {
                let method = rest.map_or("GET", |(m, _)| m);
                let v =
                    serde_json::json!({ "method": method, "path": template, "status": on.status, "body": on.value() });
                std::fs::write(format!("{dir}/{case}.agent.rest.json"), serde_json::to_vec_pretty(&v).unwrap())
                    .unwrap();
            }
        }
        _ => unreachable!(),
    }
}

/// An OpenAPI path template as a concrete request path: `{owner}/{repo}` as `o/r`, and every other parameter as `1`.
pub fn concrete(template: &str) -> String {
    let s = template.replace("{owner}", "o").replace("{repo}", "r");
    s.split('/').map(|seg| if seg.starts_with('{') { "1" } else { seg }).collect::<Vec<_>>().join("/")
}
