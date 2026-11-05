//! Merges and the merge queue on three real `graftyard_raft::Member<Cmd>`s (M2-WP3's pure gates). The
//! `MergeQueue` consumer's `step` plans on a fresh read of the leader's core tier, a fake git cache merges as a function
//! of its inputs, and a fake CI reports required checks on queue refs. Both followers run `verify` on every entry
//! before they store it.
//!
//! - BF-1: one entry holding a 5-PR group's landing plus a batch of bot ref moves commits on all three members.
//! - BF-21: a `HeadGreen` group of 5 whose 3rd fails its check, 100 seeds, the leader killed mid-landing in 10: the
//!   base moves only by `QueueMerge` entries that land a whole group, each a fast-forward; the 3rd is ejected and the
//!   other 4 land.
//! - BF-23: a required check re-requested before the merge refuses it until the new run concludes, 100 of 100.
//! - BM-2's pure part: a `Build` or `QueueMerge` whose `parent_tip` is stale is refused, 100 of 100.

use std::collections::BTreeSet;

use graftyard_git::issues::{
    CheckConclusion, CheckRunId, CheckSource, Grouping, HeadSpec, IssueCapture, IssueNumber, MergeMethod,
    MergeQueueSettings, PrCapture, PrState, Protection, RepoSettings, RequiredChecks, Visibility,
};
use graftyard_git::merge::{
    EngineVersion, Merge, MergeCmd, MergeCmdOut, MergeError, MergePlan, REPO_FORMAT_V4, RuleViolation, TreeDiffDigest,
    merge_request,
};
use graftyard_git::queue::{EjectReason, MergeQueueEntry, QueueAction, QueueEvent, QueueMerge, QueueOp, queue_ref};
use graftyard_git::queue_consumer::{CacheAnswer, CacheAsk, QueueView, Step, on_refused, request_id, step};
use graftyard_git::refs::{ObjectId, RefName, RefUpdate};
use graftyard_git::repo::RepoScratch;
use graftyard_git::repo::{REPO_FORMAT_V1, RepoEvent, RepoFold};
use graftyard_git::tiers::{get_rec, pull_head, ref_key};
use graftyard_git::{DurablePack, ObjectMeta, PackId, PackIndex};
use graftyard_identity::{
    AccountKind, Actor, AuthzEvent, AuthzFold, Credential, IdentityDep, NamesEvent, NamesFold, Principal,
    RepoPermission, ScopeSet, TokenKind, TokensEvent, TokensFold, UserId, credential_chains,
};
use graftyard_raft::{
    Command, CommitError, EntryBody, Envelope, Ev, Fold as _, Index, Member, MemberConfig, Record, ReleaseTag,
    RequestId, Resolution, Role, Stored, Ulid,
};
use graftyard_types::{
    AccountId, ChainId, DbId, Digest, NameShard, NetworkId, PlacementName, PlacementRecord, Region, ReleaseId, RepoId,
    SiteId, Timestamp, TokenShard,
};
use smallvec::{SmallVec, smallvec};

const SITES: [SiteId; 3] = [SiteId(0), SiteId(1), SiteId(2)];
const REPO: RepoId = RepoId(DbId(7));
const OWNER: i64 = 100;
const WRITER: i64 = 101;
const CI: &str = "ci";
const MINUTE: u64 = 60_000;

fn name(s: &str) -> RefName {
    RefName::parse(s.as_bytes()).unwrap()
}

fn main_ref() -> RefName {
    name("refs/heads/main")
}

fn oid(n: u32) -> ObjectId {
    let mut o = [0u8; 20];
    o[..4].copy_from_slice(&n.to_be_bytes());
    o[19] = 1;
    ObjectId(o)
}

fn who(account: i64) -> Principal {
    Principal {
        actor: Actor::User(UserId(AccountId(DbId(account)))),
        scopes: ScopeSet(1),
        credential: Credential::Stored([account as u8; 32]),
        sso_orgs: SmallVec::new(),
    }
}

/// The folds the leader reads once per entry for a user's command: accounts, tokens, and the owner's `Authz` with a
/// write grant for WRITER.
fn deps_for(chains: &[ChainId]) -> Vec<(ChainId, IdentityDep)> {
    let mut names = NamesFold::new(NameShard(0));
    let mut tokens = TokensFold::new(TokenShard(0));
    let mut authz = AuthzFold::new(AccountId(DbId(OWNER)));
    authz.apply(&AuthzEvent::Opened { kind: AccountKind::User, enterprise: None });
    for a in [OWNER, WRITER] {
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
    authz.apply(&AuthzEvent::Grant {
        repo: REPO,
        grantee: UserId(AccountId(DbId(WRITER))),
        permission: Some(RepoPermission::Write),
    });
    let mut all = Vec::new();
    for a in [OWNER, WRITER] {
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
            all.push((c, d));
        }
    }
    all.push((ChainId::Authz(AccountId(DbId(OWNER))), IdentityDep::Authz(authz)));
    chains.iter().map(|c| all.iter().find(|(k, _)| k == c).expect("a dep").clone()).collect()
}

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
        reads: vec![REPO_FORMAT_V1, REPO_FORMAT_V4],
    }
}

/// An inline pack, sealed by `PackStore::place` as a validated one is.
fn pack(bytes: &[u8]) -> DurablePack {
    use graftyard_git::mem::MemBucket;
    use graftyard_types::{Site, SiteLink, SiteTable};
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
    let sites = SiteTable(vec![site(0, Region::Enam), site(1, Region::Weur), site(2, Region::Apac)]);
    let a = MemBucket::default();
    let bk = [(SiteId(0), &a)];
    let store = graftyard_git::PackStore { sites: &sites, buckets: &bk, copies: graftyard_git::Copies::TwoRegions };
    let index = PackIndex {
        checksum: Digest(sha2::Sha256::digest(bytes).into()),
        bytes: bytes.len() as u64,
        objects: 1,
        trailer: "t".into(),
    };
    let place = graftyard_git::Place {
        network: NetworkId(DbId(1)),
        id: PackId(Ulid(1)),
        meta: ObjectMeta { chain: ChainId::Repo(REPO), request: RequestId(Ulid(1)) },
        bytes,
        index: &index,
        idx: None,
        inline_max: graftyard_git::INLINE_MAX,
    };
    futures_executor::block_on(store.place(place)).unwrap()
}

// ---- the host's command ----

/// What a repository member hosts here: the merge commands, and a stand-in for M2-WP5's `PrSync` that moves
/// `refs/pull/N/head` of open pull requests to their heads. A site member hosts both inside one enum, as M2-WP5's
/// `RepoCmd` hosts `Push` and `PrSync`.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
enum Cmd {
    M(MergeCmd),
    Sync(Vec<(IssueNumber, ObjectId)>, RequestId),
}

impl Record for Cmd {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Cmd::M(m) => m.encode(out),
            Cmd::Sync(h, r) => graftyard_fold::canon::write(out, &(h, r.0.0)),
        }
    }
}

impl Command for Cmd {
    type Fold = RepoFold;
    type Dep = IdentityDep;
    type Out = MergeCmdOut;
    type Err = (IssueNumber, MergeError);
    fn format(&self) -> graftyard_raft::Format {
        REPO_FORMAT_V4
    }
    fn tiers(&self) -> graftyard_raft::TierSet {
        match self {
            Cmd::M(m) => m.tiers(),
            Cmd::Sync(..) => graftyard_raft::TierSet::CORE,
        }
    }
    fn deps(&self, fold: &RepoFold) -> SmallVec<[ChainId; 3]> {
        match self {
            Cmd::M(m) => m.deps(fold),
            Cmd::Sync(..) => SmallVec::new(),
        }
    }
    fn decide(
        &self,
        s: &RepoScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, MergeCmdOut), (IssueNumber, MergeError)> {
        match self {
            Cmd::M(m) => m.decide(s, deps),
            Cmd::Sync(heads, push) => {
                let updates: Vec<RefUpdate> = heads
                    .iter()
                    .filter(|(n, h)| pull_head(s.core(), *n).is_some_and(|p| p.head_sha == *h))
                    .map(|(n, h)| {
                        let r = name(&format!("refs/pull/{}/head", n.0));
                        RefUpdate { expect: s.get(&r), name: r, new: Some(*h) }
                    })
                    .collect();
                Ok((smallvec![RepoEvent::Refs { push: *push, actor: None, updates, pack: None }], MergeCmdOut::Queue))
            }
        }
    }
    fn replay(&self, events: &[RepoEvent]) -> MergeCmdOut {
        match self {
            Cmd::M(m) => m.replay(events),
            Cmd::Sync(..) => MergeCmdOut::Queue,
        }
    }
}

// ---- the group ----

/// Three members of `repo:7`. Every message is delivered unless a crash drops it.
struct G {
    now: Timestamp,
    members: Vec<Option<Member<Cmd>>>,
    disks: Vec<Stored<RepoFold>>,
    inbox: Vec<Envelope<Ev<Cmd>>>,
    resolutions: Vec<Resolution<Cmd>>,
    boots: u64,
    next: u128,
}

impl G {
    fn new() -> G {
        let mut g = G {
            now: Timestamp(10_000),
            members: Vec::new(),
            disks: SITES.iter().map(|_| Stored::new(RepoFold::new(REPO, AccountId(DbId(OWNER))))).collect(),
            inbox: Vec::new(),
            resolutions: Vec::new(),
            boots: 0,
            next: 1,
        };
        for s in SITES {
            g.members.push(None);
            g.restart(s);
        }
        g.elect(SITES[0]);
        g
    }

    fn m(&self, s: SiteId) -> &Member<Cmd> {
        self.members[s.0 as usize].as_ref().expect("member is up")
    }

    /// One input on `s`, its writes stored and its outputs queued; the deps a decision asks for are read at once.
    fn on<R>(&mut self, s: SiteId, f: impl FnOnce(&mut Member<Cmd>, Timestamp) -> graftyard_raft::Ready<Cmd, R>) -> R {
        let now = self.now;
        let m = self.members[s.0 as usize].as_mut().expect("member is up");
        let (r, out) = f(m, now).persist(&mut self.disks[s.0 as usize]);
        self.take(s, out);
        r
    }

    /// Queues an output's messages and resolutions, returns its self-ack, and answers each deps request at once with
    /// the folds the leader reads through `fresh`, whose outputs are handled the same way.
    fn take(&mut self, s: SiteId, out: graftyard_raft::Out<Cmd>) {
        let now = self.now;
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
                outs.push(m.deps_read(now, d.id, deps_for(&d.chains)).persist(disk).1);
            }
        }
    }

    fn crash(&mut self, s: SiteId) {
        self.members[s.0 as usize] = None;
        self.inbox.retain(|e| e.to != s && e.from != s);
    }

    fn restart(&mut self, s: SiteId) {
        self.boots += 1;
        let (m, ready) = Member::restore(config(s), self.disks[s.0 as usize].clone(), self.now, self.boots).unwrap();
        self.members[s.0 as usize] = Some(m);
        let ((), out) = ready.persist(&mut self.disks[s.0 as usize]);
        self.inbox.extend(out.early.into_iter().chain(out.messages));
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

    fn elect(&mut self, s: SiteId) {
        for _ in 0..3 {
            self.on(s, |m, now| m.campaign(now));
            self.deliver();
            if self.m(s).role() == Role::Leader {
                self.pass(2_000);
                return;
            }
        }
        panic!("{s:?} did not win");
    }

    fn leader(&self) -> SiteId {
        *SITES
            .iter()
            .filter(|s| self.members[s.0 as usize].as_ref().is_some_and(|m| m.role() == Role::Leader))
            .max_by_key(|s| self.m(**s).term())
            .expect("a leader")
    }

    fn fold(&self) -> &RepoFold {
        &self.m(self.leader()).group_fold().fold
    }

    fn rid(&mut self) -> RequestId {
        self.next += 1;
        RequestId(Ulid(self.next << 64))
    }

    /// Events recorded as one entry, as a push or an issue command commits them, re-sent under the same id after a
    /// lost leader as a host does.
    fn record(&mut self, events: Vec<RepoEvent>) {
        let rid = self.rid();
        for _ in 0..5 {
            let l = self.leader();
            if self.on(l, |m, now| m.record(now, rid, REPO_FORMAT_V1, events.clone())).is_err() {
                self.pass(2_000);
                continue;
            }
            self.pass(2_000);
            if self.resolutions.iter().any(|r| matches!(r, Resolution::Recorded { id, result: Ok(_) } if *id == rid)) {
                return;
            }
        }
        panic!("the entry never committed");
    }

    fn get(&self, r: &RefName) -> Option<ObjectId> {
        get_rec(&*self.fold().core, "ref", &[ref_key(r)])
    }

    fn push(&mut self, branch: &str, new: ObjectId) {
        let r = name(branch);
        let expect = self.get(&r);
        let rid = self.rid();
        let ev = RepoEvent::Refs {
            push: rid,
            actor: None,
            updates: vec![RefUpdate { name: r, expect, new: Some(new) }],
            pack: None,
        };
        self.record(vec![ev]);
    }

    fn open(&mut self, n: u32, branch: &str) {
        let head = self.get(&name(branch)).unwrap();
        let base_sha = self.get(&main_ref()).unwrap();
        let issue = IssueCapture {
            id: DbId((1 << 40) + i64::from(n)),
            author: None,
            title: "t".into(),
            body: "".into(),
            labels: Default::default(),
            assignees: Default::default(),
        };
        let capture = PrCapture {
            head_sha: head,
            base_sha,
            state: PrState::Open { draft: false },
            labels: Vec::new(),
            issue,
            changes_from: None,
        };
        let ev = RepoEvent::PrOpened {
            n: IssueNumber(n),
            head: HeadSpec { repo: REPO, branch: name(branch) },
            base: main_ref(),
            capture,
        };
        self.record(vec![ev]);
    }

    /// A required conclusion, as `record_check` commits it for a SHA a rule guards.
    fn check(&mut self, sha: ObjectId, run: i64, version: u64, conclusion: Option<CheckConclusion>) {
        let ev = RepoEvent::RequiredCheck {
            sha,
            name: CI.into(),
            source: CheckSource::Run { app: graftyard_git::issues::AppId(DbId(5)), run: CheckRunId(DbId(run)) },
            version,
            conclusion,
        };
        self.record(vec![ev]);
    }

    /// Commits `cmd` under `rid`, re-sent after a lost leader as an edge does. The committed outcome.
    fn commit_as(&mut self, rid: RequestId, cmd: MergeCmd) -> Result<MergeCmdOut, (IssueNumber, MergeError)> {
        for _ in 0..5 {
            let l = self.leader();
            let Ok(a) = self.on(l, |m, now| m.commit(now, rid, Cmd::M(cmd.clone()))) else {
                self.pass(2_000);
                continue;
            };
            self.pass(2_000);
            let r = self.resolutions.iter().rev().find_map(|r| match r {
                Resolution::Ticket { ticket, result } if *ticket == a.ticket => Some(result.clone()),
                _ => None,
            });
            match r {
                Some(Ok(a)) => return Ok(a.out),
                Some(Err(CommitError::Refused { err, .. })) => return Err(err),
                _ => {}
            }
        }
        panic!("the command never resolved");
    }

    fn commit(&mut self, cmd: MergeCmd) -> Result<MergeCmdOut, (IssueNumber, MergeError)> {
        let rid = self.rid();
        self.commit_as(rid, cmd)
    }

    /// Every committed entry's events, in order, from the leader's log.
    fn committed(&self) -> Vec<Vec<RepoEvent>> {
        let m = self.m(self.leader());
        (1..=m.state().commit.0)
            .filter_map(|i| m.entry_at(Index(i)))
            .filter_map(|e| match &e.body {
                EntryBody::Events(evs) => Some(evs.clone()),
                _ => None,
            })
            .collect()
    }

    /// Every member stored every committed entry (both followers ran `verify` on each), and the followers, which
    /// learn a commit from the next append, hold the fold the leader held at their applied index: two more entries
    /// carry the commit index to them, and then all three folds are equal.
    fn assert_agree(&mut self) {
        let commit = self.m(self.leader()).state().commit;
        for s in SITES {
            assert!(self.m(s).last_index() >= commit, "{s:?} is missing committed entries");
        }
        for _ in 0..2 {
            let (l, rid) = (self.leader(), self.rid());
            let ev = RepoEvent::Refused { push: rid, refs: Vec::new() };
            self.on(l, |m, now| m.record(now, rid, REPO_FORMAT_V1, vec![ev])).expect("recorded");
            self.pass(2_000);
        }
        let commit = self.m(self.leader()).state().commit;
        let at: Vec<(Index, &RepoFold)> =
            self.members.iter().flatten().map(|m| (m.state().applied, &m.group_fold().fold)).collect();
        assert!(
            at.iter().all(|a| a.0.0 + 1 >= commit.0),
            "a member trails: {:?}",
            at.iter().map(|a| a.0).collect::<Vec<_>>()
        );
        let ends: Vec<&RepoFold> = at.iter().filter(|a| a.0 == at[0].0).map(|a| a.1).collect();
        assert!(ends.windows(2).all(|w| w[0] == w[1]), "members' folds differ at one index");
    }
}

// ---- the fake git cache and CI ----

/// A merge of `head` onto `on`: a function of the pair, as the container's deterministic `commit-tree` is.
fn merged(on: ObjectId, head: ObjectId) -> ObjectId {
    ObjectId(std::array::from_fn(|i| on.0[i].wrapping_mul(31) ^ head.0[i].wrapping_add(7) ^ 0x5a))
}

fn plan_of(base_old: ObjectId, head: ObjectId, method: MergeMethod) -> MergePlan {
    let commit = merged(base_old, head);
    MergePlan {
        base_old,
        head,
        commit,
        pack: pack(&commit.0),
        method,
        up_to_date: true,
        verified: TreeDiffDigest(Digest([1; 32])),
        engine: EngineVersion("git 2.51".into()),
    }
}

fn answer(ask: &CacheAsk) -> CacheAnswer {
    match ask {
        CacheAsk::Build { on, prs, .. } => {
            let mut tip = *on;
            let entries = prs
                .iter()
                .map(|(n, head)| {
                    let e = MergeQueueEntry { pr: *n, parent_tip: tip, group_tip: merged(tip, *head) };
                    tip = e.group_tip;
                    e
                })
                .collect();
            CacheAnswer::Built { ask: ask.clone(), entries, pack: Some(pack(&tip.0)), conflict: None }
        }
        CacheAsk::Plan { entries, method, .. } => {
            let plans = entries.iter().map(|(e, head)| (e.pr, plan_of(e.parent_tip, *head, *method))).collect();
            CacheAnswer::Planned { ask: ask.clone(), plans }
        }
    }
}

/// The base rule: `ci` required, and a merge queue under `grouping`.
fn rule(grouping: Grouping) -> RepoEvent {
    RepoEvent::Rule(graftyard_git::issues::BranchRule {
        id: DbId(1),
        pattern: "main".into(),
        protection: Some(Protection {
            checks: Some(RequiredChecks { strict: false, checks: smallvec![(CI.into(), None)] }),
            reviews: None,
            push: None,
            enforce_admins: false,
            linear_history: false,
            force_pushes: false,
            deletions: false,
            block_creations: false,
            conversation_resolution: true,
            lock_branch: false,
            signatures: false,
            merge_queue: Some(MergeQueueSettings {
                check_timeout_minutes: 60,
                grouping,
                max_build: 5,
                min_merge: 5,
                max_merge: 5,
                min_wait_minutes: 5,
                method: MergeMethod::Merge,
            }),
        }),
    })
}

fn settings() -> RepoSettings {
    RepoSettings {
        owner: AccountId(DbId(OWNER)),
        enterprise: None,
        visibility: Visibility::Private,
        default_branch: main_ref(),
        archived: false,
        description: None,
        homepage: None,
        topics: SmallVec::new(),
        has_issues: true,
        has_wiki: false,
        has_projects: false,
        merge_methods: smallvec![MergeMethod::Merge, MergeMethod::Squash],
        delete_branch_on_merge: true,
        allow_auto_merge: false,
        allow_update_branch: false,
        check_retention_days: 90,
    }
}

/// `main` at oid(1) under a queue rule, and pull requests 1..=k from `f1..fk`, each enqueued by WRITER.
fn queued_group(grouping: Grouping, k: u32) -> G {
    let mut g = G::new();
    g.push("refs/heads/main", oid(1));
    g.record(vec![RepoEvent::Settings(settings()), rule(grouping)]);
    for n in 1..=k {
        g.push(&format!("refs/heads/f{n}"), oid(100 + n));
        g.open(n, &format!("refs/heads/f{n}"));
        let op =
            QueueOp { op: QueueAction::Enqueue { n: IssueNumber(n), head: oid(100 + n) }, actor: Some(who(WRITER)) };
        g.commit(MergeCmd::Queue(op)).expect("enqueued");
    }
    g
}

/// The consumer, planned on fresh reads until it has nothing to submit. Each command goes under its content's id.
/// `fail` names the pull requests CI fails on their own queue refs; every other queue ref passes.
struct Consumer {
    fail: BTreeSet<IssueNumber>,
    /// Queue refs CI has reported, so each gets one run.
    reported: BTreeSet<RefName>,
    run: i64,
}

impl Consumer {
    fn new(fail: &[u32]) -> Consumer {
        Consumer { fail: fail.iter().map(|n| IssueNumber(*n)).collect(), reported: BTreeSet::new(), run: 1_000 }
    }

    fn view(g: &G) -> QueueView {
        QueueView::of(&g.fold().core, &main_ref())
    }

    /// CI on every new queue ref: a ref holds the changes of its entry and every earlier one, so it fails when any
    /// of them is in `fail`.
    fn ci(&mut self, g: &mut G) {
        let v = Self::view(g);
        let mut bad = false;
        for e in &v.entries {
            let Some(b) = e.built.as_ref().map(|b| &b.entry) else { break };
            bad |= self.fail.contains(&e.n);
            let r = queue_ref(&main_ref(), b);
            if self.reported.insert(r) {
                self.run += 1;
                let c = if bad { CheckConclusion::Failure } else { CheckConclusion::Success };
                g.check(b.group_tip, self.run, 1, Some(c));
            }
        }
    }

    /// One planning round: the commands `step` gives on a fresh view, with the cache's answers.
    fn plan(g: &G) -> (QueueView, Vec<MergeCmd>) {
        let v = Self::view(g);
        let mut answers = Vec::new();
        for _ in 0..4 {
            match step(&v, g.now, &answers) {
                Step::Ask(a) => answers.push(answer(&a)),
                Step::Submit(c) => return (v, c),
                Step::Idle { .. } => return (v, Vec::new()),
            }
        }
        panic!("step kept asking");
    }

    /// Runs until quiet. `kill` is called before a landing's commit and may crash the leader.
    fn drain(&mut self, g: &mut G, mut kill: impl FnMut(&mut G, &MergeCmd)) {
        for _ in 0..50 {
            self.ci(g);
            let (v, cmds) = Self::plan(g);
            if cmds.is_empty() {
                return;
            }
            for c in cmds {
                kill(g, &c);
                let rid = request_id(&c, &v);
                if let Err((n, e)) = g.commit_as(rid, c.clone())
                    && let Some(f) = on_refused(&c, n, &e)
                {
                    let _ = g.commit(f);
                }
            }
        }
        panic!("the consumer never went quiet");
    }
}

/// What BF-21 asserts over the committed log: the base moves only in entries whose `Merged` events land a whole
/// built group through `Landed`, each from the base's current value, and only to `group_tip`s that descend from it.
/// Returns the landed pull requests in order.
fn audit_base(g: &G) -> Vec<IssueNumber> {
    let main = main_ref();
    let mut tip: Option<ObjectId> = None;
    let mut landed = Vec::new();
    for evs in g.committed() {
        let mut moved = Vec::new();
        for ev in &evs {
            for u in ev.updates() {
                if u.name == main {
                    assert_eq!(u.expect, tip, "every base move starts at the base's value");
                    moved.push((ev, u.new));
                    tip = u.new;
                }
            }
        }
        if moved.is_empty() {
            continue;
        }
        if moved.len() == 1
            && matches!(moved[0].0, RepoEvent::Refs { .. })
            && landed.is_empty()
            && moved[0].1 == Some(oid(1))
        {
            continue;
        }
        let ns: Vec<IssueNumber> = moved
            .iter()
            .map(|(ev, _)| match ev {
                RepoEvent::Merged { n, .. } => *n,
                other => panic!("the base moved by {other:?}, not a QueueMerge"),
            })
            .collect();
        let through = evs.iter().find_map(|e| match e {
            RepoEvent::Queue(QueueEvent::Landed { through, .. }) => Some(*through),
            _ => None,
        });
        assert_eq!(through, ns.last().copied(), "a base move without `Landed` through its last pull request");
        landed.extend(ns);
    }
    landed
}

fn merged_state(g: &G, n: u32) -> bool {
    matches!(pull_head(&*g.fold().core, IssueNumber(n)).unwrap().state, PrState::Merged { .. })
}

// ---- BF-21 ----

/// A `HeadGreen` group of 5 whose 3rd fails its check on its queue ref. `kill_at` crashes the leader before the
/// landing commit and restores it, with the next leader elected while it is down.
/// Returns, for a killed run, whether the dead leader's landing entry committed under the next leader.
fn bf21(seed: u64, kill: bool) -> Option<bool> {
    let mut survived = None;
    let mut g = queued_group(Grouping::HeadGreen, 5);
    g.pass(5 * MINUTE + seed % 7 * 1_000);
    let mut c = Consumer::new(&[3]);
    let mut killed = false;
    c.drain(&mut g, |g, cmd| {
        if kill && !killed && matches!(cmd, MergeCmd::Land(_)) {
            killed = true;
            let l = g.leader();
            let to = SITES[((l.0 as u64 + 1 + seed % 2) % 3) as usize];
            // The landing is admitted and appended, then the leader dies: with its AppendEntries delivered to both
            // followers, to one, or to none. The re-send below goes under the same request id.
            let rid = request_id(cmd, &Consumer::view(g));
            g.on(l, |m, now| m.commit(now, rid, Cmd::M(cmd.clone()))).expect("admitted");
            match seed % 3 {
                0 => g.deliver(),
                1 => {
                    let one = SITES[((l.0 + 1) % 3) as usize];
                    g.inbox.retain(|e| e.to == one);
                    for e in std::mem::take(&mut g.inbox) {
                        let Envelope { from, to, msg } = e;
                        g.on(to, |m, now| m.step(now, from, msg));
                    }
                }
                _ => {}
            }
            g.crash(l);
            g.pass(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1_000);
            g.elect(to);
            g.restart(l);
            g.pass(2_000);
            survived = Some(g.committed().iter().flatten().any(|e| matches!(e, RepoEvent::Merged { .. })));
        }
    });

    assert_eq!(killed, kill, "seed {seed}: the kill ran");
    let landed = audit_base(&g);
    let want: Vec<IssueNumber> = [1, 2, 4, 5].map(IssueNumber).to_vec();
    assert_eq!(landed.iter().copied().collect::<BTreeSet<_>>(), want.iter().copied().collect(), "seed {seed}: landed");
    assert_eq!(landed.len(), 4, "seed {seed}: a pull request landed twice");
    assert!(!merged_state(&g, 3), "seed {seed}: the failing pull request merged");
    let ejected = g.committed().into_iter().flatten().any(|e| {
        matches!(
            e,
            RepoEvent::Queue(QueueEvent::Ejected { n: IssueNumber(3), reason: EjectReason::CheckFailed { .. }, .. })
        )
    });
    assert!(ejected, "seed {seed}: #3 was not ejected for its check");
    for n in [1, 2, 4, 5] {
        assert!(merged_state(&g, n), "seed {seed}: #{n} was lost");
    }
    assert!(Consumer::view(&g).entries.is_empty(), "seed {seed}: the queue is not empty");
    g.assert_agree();
    survived
}

#[test]
fn bf21_a_head_green_group_ejects_the_failing_third_and_lands_the_rest_whole() {
    let mut kills = Vec::new();
    for seed in 0..100 {
        kills.extend(bf21(seed, seed % 10 == 0));
    }
    assert_eq!(kills.len(), 10);
    // Both ways a kill mid-landing can go: its entry reached a follower and commits under the next leader, or it was
    // lost and the consumer lands the group again.
    assert!(kills.contains(&true) && kills.contains(&false), "{kills:?}");
}

/// Under `HeadGreen`, the consumer's first landing of a group of 5 whose 3rd fails is refused with #3's number when
/// the failure on #3's ref is reported after the consumer planned, so no entry lands past a failed check.
#[test]
fn a_landing_planned_before_a_failure_is_refused_whole() {
    let mut g = queued_group(Grouping::HeadGreen, 5);
    g.pass(5 * MINUTE);
    let (_, cmds) = Consumer::plan(&g);
    let [MergeCmd::Queue(_)] = cmds.as_slice() else { panic!("a build first: {cmds:?}") };
    g.commit(cmds[0].clone()).expect("built");
    let v = Consumer::view(&g);
    let tips: Vec<ObjectId> = v.entries.iter().map(|e| e.built.as_ref().unwrap().entry.group_tip).collect();
    // Only the head passes so far: `HeadGreen` plans to land all 5.
    g.check(tips[4], 1, 1, Some(CheckConclusion::Success));
    let (_, cmds) = Consumer::plan(&g);
    let [land @ MergeCmd::Land(_)] = cmds.as_slice() else { panic!("a landing: {cmds:?}") };
    // CI then reports #3's own ref failing, before the landing commits.
    g.check(tips[2], 2, 1, Some(CheckConclusion::Failure));
    let before = g.get(&main_ref());
    let err = g.commit(land.clone()).unwrap_err();
    assert_eq!(err.0, IssueNumber(3));
    assert!(matches!(err.1, MergeError::Protection(RuleViolation::CheckFailing { .. })), "{err:?}");
    assert_eq!(g.get(&main_ref()), before, "nothing landed");
}

// ---- BF-23 ----

/// `main` under a rule requiring `ci` and no queue, and pull request #1 from `f` at oid(201).
fn direct() -> G {
    let mut g = G::new();
    g.push("refs/heads/main", oid(1));
    let RepoEvent::Rule(mut r) = rule(Grouping::AllGreen) else { unreachable!() };
    r.protection.as_mut().unwrap().merge_queue = None;
    g.record(vec![RepoEvent::Settings(settings()), RepoEvent::Rule(r)]);
    g.push("refs/heads/f", oid(201));
    g.open(1, "refs/heads/f");
    g
}

/// The edge's merge: the request from the fresh core tier, the git cache's plan, then `Merge` committed.
fn merge(g: &mut G, method: MergeMethod) -> Result<MergeCmdOut, (IssueNumber, MergeError)> {
    let req = merge_request(&g.fold().core, IssueNumber(1), method).map_err(|e| (IssueNumber(1), e))?;
    let plan = plan_of(req.base_old, req.head, method);
    g.commit(MergeCmd::Merge(Merge { n: IssueNumber(1), plan, expected_head: None, actor: who(WRITER) }))
}

/// A required check passes, is re-requested (the same run's next version, queued), and the merge is refused until
/// the new run concludes, then succeeds. 100 of 100, alternating a re-run by a new run id and a re-request of the
/// same run, and once more on the queue: a re-requested check on a queue ref holds the landing.
#[test]
fn bf23_a_re_requested_check_refuses_the_merge_until_the_new_run_concludes() {
    let mut refused = 0;
    for i in 0..100u64 {
        let mut g = direct();
        g.check(oid(201), 10, 1, Some(CheckConclusion::Success));
        let (run, version) = if i % 2 == 0 { (10, 2) } else { (11, 1) };
        g.check(oid(201), run, version, None);
        let before = g.get(&main_ref());
        let err = merge(&mut g, MergeMethod::Merge).unwrap_err();
        assert!(matches!(err.1, MergeError::Protection(RuleViolation::CheckPending { .. })), "run {i}: {err:?}");
        assert_eq!(g.get(&main_ref()), before, "run {i}: the base moved on a stale success");
        refused += 1;
        g.check(oid(201), run, version + 1, Some(CheckConclusion::Success));
        assert!(merge(&mut g, MergeMethod::Merge).is_ok(), "run {i}: the merge after the new run");
        assert_eq!(g.get(&main_ref()), Some(merged(oid(1), oid(201))));
        assert!(merged_state(&g, 1));
        // The head branch is deleted in the same entry, as `delete_branch_on_merge` asks.
        assert_eq!(g.get(&name("refs/heads/f")), None);
    }
    assert_eq!(refused, 100);
}

#[test]
fn bf23_on_the_queue_a_re_requested_check_holds_the_landing_until_it_concludes() {
    for i in 0..100u64 {
        let mut g = queued_group(Grouping::AllGreen, 1);
        g.pass(5 * MINUTE);
        let mut c = Consumer::new(&[]);
        let (v, cmds) = Consumer::plan(&g);
        g.commit_as(request_id(&cmds[0], &v), cmds[0].clone()).expect("built");
        c.ci(&mut g);
        let (_, cmds) = Consumer::plan(&g);
        let [land @ MergeCmd::Land(_)] = cmds.as_slice() else { panic!("run {i}: a landing: {cmds:?}") };
        let tip = Consumer::view(&g).entries[0].built.as_ref().unwrap().entry.group_tip;
        // Re-requested after the consumer planned: the landing is refused, and the consumer waits rather than ejects.
        g.check(tip, c.run, 2, None);
        let err = g.commit(land.clone()).unwrap_err();
        assert!(matches!(err.1, MergeError::Protection(RuleViolation::CheckPending { .. })), "run {i}: {err:?}");
        assert_eq!(on_refused(land, err.0, &err.1), None, "run {i}: a pending re-run ejects nothing");
        assert_eq!(g.get(&main_ref()), Some(oid(1)), "run {i}: landed on a stale success");
        let (_, cmds) = Consumer::plan(&g);
        assert!(cmds.is_empty(), "run {i}: the consumer waits: {cmds:?}");
        g.check(tip, c.run, 3, Some(CheckConclusion::Success));
        c.drain(&mut g, |_, _| {});
        assert!(merged_state(&g, 1), "run {i}: landed after the new run");
    }
}

// ---- BM-2's pure part ----

/// A queue entry whose `parent_tip` is not the previous entry's `group_tip`, nor the base tip for the first, is
/// refused: a `Build` that skips or rewrites a built entry, and a `QueueMerge` that names a stale chain. 100 of 100
/// for each, with the stale tip drawn per run.
#[test]
fn bm2_a_queue_entry_with_a_stale_parent_tip_is_refused() {
    let mut g = queued_group(Grouping::AllGreen, 3);
    g.pass(5 * MINUTE);
    let v = Consumer::view(&g);
    let CacheAnswer::Built { entries, pack, .. } = answer(&CacheAsk::Build {
        base: main_ref(),
        on: oid(1),
        prs: v.entries.iter().map(|e| (e.n, e.pinned)).collect(),
        method: MergeMethod::Merge,
    }) else {
        unreachable!()
    };
    let pack = pack.unwrap();
    // Two built; the third waits.
    let build = |es: &[MergeQueueEntry]| {
        MergeCmd::Queue(QueueOp {
            op: QueueAction::Build { base: main_ref(), entries: es.iter().copied().collect(), pack: pack.clone() },
            actor: None,
        })
    };
    g.commit(build(&entries[..2])).expect("built");
    let before = g.fold().clone();
    for i in 0..100u32 {
        let stale = oid(5_000 + i);
        // A third entry chained to something other than #2's group tip.
        let mut e = entries[2];
        e.parent_tip = if i % 2 == 0 { stale } else { entries[0].group_tip };
        let err = g.commit(build(&[e])).unwrap_err();
        assert_eq!(err.1, MergeError::StaleChain, "run {i}");
        // A chain inside one `Build` that breaks after its first entry.
        let mut es = entries.clone();
        es[1].parent_tip = stale;
        let err = g.commit(build(&es)).unwrap_err();
        assert_eq!(err.1, MergeError::StaleChain, "run {i}");
        // A landing of #1 and #2 whose second plan names a stale `base_old`.
        let mut plans: Vec<(IssueNumber, MergePlan)> =
            entries[..2].iter().map(|e| (e.pr, plan_of(e.parent_tip, oid(100 + e.pr.0), MergeMethod::Merge))).collect();
        // The right merge commit, from the wrong parent: only the `parent_tip` rule refuses it.
        plans[1].1.base_old = stale;
        let err = g.commit(MergeCmd::Land(QueueMerge { plans, entry: entries[1] })).unwrap_err();
        assert_eq!(err, (IssueNumber(2), MergeError::StaleChain), "run {i}");
    }
    assert!(*g.fold() == before || g.get(&main_ref()) == Some(oid(1)), "a refused entry changed the queue");
    assert_eq!(g.get(&main_ref()), Some(oid(1)));
    assert_eq!(Consumer::view(&g).valid_built(), 2, "the built chain is unchanged");
    // The good extension still builds.
    g.commit(build(&entries[2..])).expect("the chained entry builds");
    assert_eq!(Consumer::view(&g).valid_built(), 3);
}

/// A base push that no `QueueMerge` made leaves the built entries chained to the old tip: a landing on them is refused,
/// and the consumer rebuilds from the new tip, so a group cannot drop the pushed change.
#[test]
fn a_base_push_outside_the_queue_rebuilds_from_the_new_tip() {
    let mut g = queued_group(Grouping::AllGreen, 2);
    g.pass(5 * MINUTE);
    let mut c = Consumer::new(&[]);
    let (v, cmds) = Consumer::plan(&g);
    g.commit_as(request_id(&cmds[0], &v), cmds[0].clone()).expect("built");
    c.ci(&mut g);
    let (_, cmds) = Consumer::plan(&g);
    let [land @ MergeCmd::Land(_)] = cmds.as_slice() else { panic!("a landing: {cmds:?}") };
    g.push("refs/heads/main", oid(9));
    let err = g.commit(land.clone()).unwrap_err();
    assert_eq!(err.1, MergeError::BaseModified);
    c.drain(&mut g, |_, _| {});
    let tip = g.get(&main_ref()).unwrap();
    assert_eq!(tip, merged(merged(oid(9), oid(101)), oid(102)), "both landed on the pushed tip");
}

// ---- BF-1 ----

/// One entry holding a 5-PR group's landing plus a `PrSync` batch's bot ref moves (`refs/pull/N/head`) commits on
/// all three members: both followers stored it, so each one's `verify` accepted every event in it.
#[test]
fn bf1_a_group_landing_and_a_bot_batch_in_one_entry_commit_on_all_three() {
    let mut g = queued_group(Grouping::AllGreen, 5);
    g.pass(5 * MINUTE);
    let mut c = Consumer::new(&[]);
    let (v, cmds) = Consumer::plan(&g);
    g.commit_as(request_id(&cmds[0], &v), cmds[0].clone()).expect("built");
    c.ci(&mut g);
    let (_, cmds) = Consumer::plan(&g);
    let [land @ MergeCmd::Land(_)] = cmds.as_slice() else { panic!("a landing: {cmds:?}") };
    let l = g.leader();
    let at = g.m(l).last_index();
    // An entry in flight, so the batch and the landing queue behind it and group into the next one.
    let hold = g.rid();
    let ev = RepoEvent::Refused { push: hold, refs: Vec::new() };
    g.on(l, |m, now| m.record(now, hold, REPO_FORMAT_V1, vec![ev])).expect("recorded");
    let sync = Cmd::Sync((1..=5).map(|n| (IssueNumber(n), oid(100 + n))).collect(), g.rid());
    let sid = g.rid();
    let s = g.on(l, |m, now| m.commit(now, sid, sync)).expect("admitted");
    let lid = g.rid();
    let a = g.on(l, |m, now| m.commit(now, lid, Cmd::M(land.clone()))).expect("admitted");
    g.pass(4_000);
    let ok =
        g.resolutions.iter().any(|r| matches!(r, Resolution::Ticket { ticket, result: Ok(_) } if *ticket == a.ticket));
    assert!(ok, "the landing committed");
    let ok =
        g.resolutions.iter().any(|r| matches!(r, Resolution::Ticket { ticket, result: Ok(_) } if *ticket == s.ticket));
    assert!(ok, "the batch committed");
    // The entry holding both.
    let m = g.m(l);
    let both = (at.0 + 1..=m.state().commit.0).find(|i| {
        let EntryBody::Events(evs) = &m.entry_at(Index(*i)).unwrap().body else { return false };
        evs.iter().filter(|e| matches!(e, RepoEvent::Merged { .. })).count() == 5
            && evs.iter().any(|e| matches!(e, RepoEvent::Refs { updates, .. } if updates.len() == 5))
    });
    let both = Index(both.expect("one entry holds the landing and the bot batch"));
    for s in SITES {
        assert!(g.m(s).last_index() >= both, "{s:?} did not store it");
        assert_eq!(g.m(s).entry_at(both), g.m(l).entry_at(both), "{s:?} holds another entry");
    }
    g.assert_agree();
    for n in 1..=5 {
        assert!(merged_state(&g, n), "#{n} merged");
        assert_eq!(g.get(&name(&format!("refs/pull/{n}/head"))), Some(oid(100 + n)));
    }
}

// ---- direct merges and the rules ----

/// The merge `decide` re-checks the plan on the fold it commits on: a head push or a base push between plan and
/// commit refuses it with GitHub's text, and nothing moves.
#[test]
fn a_merge_planned_before_a_head_or_base_push_is_refused() {
    let mut g = direct();
    g.check(oid(201), 10, 1, Some(CheckConclusion::Success));
    let req = merge_request(&g.fold().core, IssueNumber(1), MergeMethod::Merge).unwrap();
    let plan = plan_of(req.base_old, req.head, MergeMethod::Merge);
    let cmd =
        |plan: MergePlan| MergeCmd::Merge(Merge { n: IssueNumber(1), plan, expected_head: None, actor: who(WRITER) });
    g.push("refs/heads/f", oid(202));
    let err = g.commit(cmd(plan.clone())).unwrap_err().1;
    assert_eq!(err, MergeError::HeadModified);
    assert_eq!(err.message(), "Head branch was modified. Review and try the merge again.");
    let mut g = direct();
    g.check(oid(201), 10, 1, Some(CheckConclusion::Success));
    g.push("refs/heads/main", oid(2));
    let err = g.commit(cmd(plan)).unwrap_err().1;
    assert_eq!(err, MergeError::BaseModified);
    assert_eq!(err.message(), "Base branch was modified. Review and try the merge again.");
    assert_eq!(g.get(&main_ref()), Some(oid(2)));
    // GitHub's `sha` parameter must match the head.
    let req = merge_request(&g.fold().core, IssueNumber(1), MergeMethod::Merge).unwrap();
    let plan = plan_of(req.base_old, req.head, MergeMethod::Merge);
    let m = Merge { n: IssueNumber(1), plan, expected_head: Some(oid(9)), actor: who(WRITER) };
    assert_eq!(g.commit(MergeCmd::Merge(m)).unwrap_err().1, MergeError::HeadModified);
}

/// A rule's failing check, and a missing one, refuse a direct merge; a merge queue on the base refuses it too.
#[test]
fn a_direct_merge_needs_every_required_check_and_no_queue() {
    let mut g = direct();
    let err = merge(&mut g, MergeMethod::Merge).unwrap_err().1;
    assert!(matches!(err, MergeError::Protection(RuleViolation::CheckExpected { .. })), "{err:?}");
    g.check(oid(201), 10, 1, Some(CheckConclusion::Failure));
    let err = merge(&mut g, MergeMethod::Merge).unwrap_err().1;
    assert!(matches!(err, MergeError::Protection(RuleViolation::CheckFailing { .. })), "{err:?}");
    let mut g = queued_group(Grouping::AllGreen, 1);
    let err = merge(&mut g, MergeMethod::Merge).unwrap_err().1;
    assert_eq!(err, MergeError::Protection(RuleViolation::MergeQueueRequired));
}

mod rules {
    use graftyard_git::issues::{
        AppId, BranchRule, CheckConclusion, CheckRunId, CheckSource, IssueNumber, MergeMethod, Protection, Pusher,
        RequiredChecks, RequiredReviews, ReviewState,
    };
    use graftyard_git::merge::{PrView, RequiredCheckView, ReviewView, RuleViolation, Via, check_protection};
    use graftyard_git::refs::ObjectId;
    use graftyard_identity::UserId;
    use graftyard_types::{AccountId, DbId};
    use smallvec::smallvec;

    const HEAD: ObjectId = ObjectId([2; 20]);

    fn base() -> Protection {
        Protection {
            checks: None,
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
        }
    }

    fn pr() -> PrView {
        PrView {
            n: IssueNumber(1),
            head: HEAD,
            method: MergeMethod::Merge,
            unresolved: 0,
            up_to_date: true,
            actor: Some(UserId(AccountId(DbId(101)))),
            admin: false,
            via: Via::Direct,
        }
    }

    fn check(p: Protection, pr: &PrView, req: &[RequiredCheckView], rev: &[ReviewView]) -> Result<(), RuleViolation> {
        check_protection(&BranchRule { id: DbId(1), pattern: "main".into(), protection: Some(p) }, pr, req, rev)
    }

    fn approve(commit: ObjectId) -> ReviewView {
        ReviewView { reviewer: Some(AccountId(DbId(102))), state: ReviewState::Approved, commit }
    }

    fn reviews(count: u8) -> RequiredReviews {
        RequiredReviews {
            count,
            dismiss_stale: false,
            code_owners: false,
            last_push: false,
            dismissers: smallvec![],
            bypass: smallvec![],
        }
    }

    #[test]
    fn reviews_count_approvals_and_a_change_request_blocks() {
        let p = Protection { reviews: Some(reviews(1)), ..base() };
        assert_eq!(check(p.clone(), &pr(), &[], &[]), Err(RuleViolation::ReviewsRequired { count: 1 }));
        assert_eq!(check(p.clone(), &pr(), &[], &[approve(HEAD)]), Ok(()));
        let no =
            ReviewView { reviewer: Some(AccountId(DbId(103))), state: ReviewState::ChangesRequested, commit: HEAD };
        assert_eq!(check(p.clone(), &pr(), &[], &[approve(HEAD), no]), Err(RuleViolation::ChangesRequested));
        // `dismiss_stale`: an approval of an older commit does not count.
        let stale = Protection { reviews: Some(RequiredReviews { dismiss_stale: true, ..reviews(1) }), ..base() };
        assert_eq!(
            check(stale, &pr(), &[], &[approve(ObjectId([1; 20]))]),
            Err(RuleViolation::ReviewsRequired { count: 1 })
        );
        // A bypass actor skips the review rule.
        let by = RequiredReviews { bypass: smallvec![Pusher::User(UserId(AccountId(DbId(101))))], ..reviews(2) };
        assert_eq!(check(Protection { reviews: Some(by), ..base() }, &pr(), &[], &[]), Ok(()));
    }

    #[test]
    fn conversation_resolution_linear_history_lock_push_and_strict() {
        let mut open = pr();
        open.unresolved = 2;
        assert_eq!(
            check(Protection { conversation_resolution: true, ..base() }, &open, &[], &[]),
            Err(RuleViolation::Unresolved { count: 2 })
        );
        assert_eq!(
            check(Protection { linear_history: true, ..base() }, &pr(), &[], &[]),
            Err(RuleViolation::LinearHistory)
        );
        let mut squash = pr();
        squash.method = MergeMethod::Squash;
        assert_eq!(check(Protection { linear_history: true, ..base() }, &squash, &[], &[]), Ok(()));
        assert_eq!(check(Protection { lock_branch: true, ..base() }, &pr(), &[], &[]), Err(RuleViolation::Locked));
        assert_eq!(check(Protection { signatures: true, ..base() }, &pr(), &[], &[]), Err(RuleViolation::Signatures));
        let push = Protection { push: Some(smallvec![Pusher::User(UserId(AccountId(DbId(7))))]), ..base() };
        assert_eq!(check(push, &pr(), &[], &[]), Err(RuleViolation::PushRestricted));
        let strict = Protection { checks: Some(RequiredChecks { strict: true, checks: smallvec![] }), ..base() };
        let mut behind = pr();
        behind.up_to_date = false;
        assert_eq!(check(strict, &behind, &[], &[]), Err(RuleViolation::NotUpToDate));
    }

    #[test]
    fn required_checks_by_app_and_admin_bypass() {
        let run = |app: i64, c: Option<CheckConclusion>| RequiredCheckView {
            name: "ci".into(),
            source: CheckSource::Run { app: AppId(DbId(app)), run: CheckRunId(DbId(1)) },
            conclusion: c,
        };
        let only5 = Protection {
            checks: Some(RequiredChecks { strict: false, checks: smallvec![("ci".into(), Some(AppId(DbId(5))))] }),
            ..base()
        };
        // Another App's success does not satisfy a check pinned to App 5.
        let err = check(only5.clone(), &pr(), &[run(6, Some(CheckConclusion::Success))], &[]);
        assert_eq!(err, Err(RuleViolation::CheckExpected { name: "ci".into() }));
        assert_eq!(check(only5.clone(), &pr(), &[run(5, Some(CheckConclusion::Skipped))], &[]), Ok(()));
        assert!(check(only5.clone(), &pr(), &[run(5, None)], &[]).is_err());
        // An admin bypasses a rule that does not enforce admins, and not one that does.
        let mut admin = pr();
        admin.admin = true;
        assert_eq!(check(only5.clone(), &admin, &[], &[]), Ok(()));
        assert!(check(Protection { enforce_admins: true, ..only5 }, &admin, &[], &[]).is_err());
    }
}

/// The entries that ask the consumer to look at `main`: an enqueue, a build, a required conclusion on a queue ref, a
/// landing, and a push to `main`; a push to an unqueued branch asks for nothing.
#[test]
fn touched_names_the_base_of_every_entry_the_consumer_must_see() {
    use graftyard_git::queue_consumer::touched;
    let mut g = queued_group(Grouping::AllGreen, 1);
    g.pass(5 * MINUTE);
    let mut c = Consumer::new(&[]);
    let from = g.m(g.leader()).state().commit;
    c.drain(&mut g, |_, _| {});
    g.push("refs/heads/main", oid(9));
    g.push("refs/heads/other", oid(10));
    let m = g.m(g.leader());
    let core = &g.fold().core;
    let mut asked = Vec::new();
    for i in 1..=m.state().commit.0 {
        let e = m.entry_at(Index(i)).unwrap();
        let EntryBody::Events(evs) = &e.body else { continue };
        let kind = match &evs[0] {
            RepoEvent::Queue(QueueEvent::Enqueued { .. }) => "enqueued",
            RepoEvent::Queue(QueueEvent::GroupBuilt { .. }) => "built",
            RepoEvent::RequiredCheck { .. } if Index(i) > from => "check",
            RepoEvent::Merged { .. } => "landed",
            RepoEvent::Refs { updates, .. } if updates[0].name == main_ref() && Index(i) > from => "push main",
            RepoEvent::Refs { updates, .. } if updates[0].name == name("refs/heads/other") => "push other",
            _ => continue,
        };
        asked.push((kind, touched(core, REPO, e)));
    }
    let main = vec![main_ref()];
    for (kind, want) in [("enqueued", &main), ("built", &main), ("landed", &main), ("push other", &Vec::new())] {
        assert!(asked.iter().any(|(k, t)| *k == kind && t == want), "{kind}: {asked:?}");
    }
    // The check reached the queue ref while it was built; by the time it is read here the queue has landed, so the
    // fold no longer names its base. The consumer reads entries right after they commit, when it does.
    assert!(asked.iter().any(|(k, _)| *k == "check"), "{asked:?}");
}
