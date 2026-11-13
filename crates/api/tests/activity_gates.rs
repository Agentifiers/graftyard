//! BF-5, BF-67, and BF-68 (`build/BUILD-PLAN.md`, M4-WP2) in memory, on real `graftyard_raft::Member`s.
//!
//! Each account's `Identity` chain and each repository's chain are three real members (`common::Group`). `Counts`
//! reads every account's committed `Identity` log after its cursor and commits `RepoCmd::Counts` on each repository's
//! leader, then the account's `CountsCursor`. `Notify` reads each repository's committed log after its
//! `ConsumerCursor` on a follower and fills each account's `Inbox`, which writes its daily `Snapshot`. `Search` writes
//! each changed document to its `SearchShard`, a real SQLite database with the FTS5 tables of `search::DDL`
//! (`rusqlite`, bundled SQLite, the engine the C15 probe ran in Durable Object SQLite). Time is simulated.
//!
//! - BF-5: star a repository and comment on a watched issue, then delete and rebuild the watcher's `Inbox`. Pass:
//!   `stargazers_count` and `/notifications` show both within 10 s, and the rebuilt `Inbox` keeps read state.
//! - BF-67: delete an `Inbox` and rebuild it; also 10,000 stars, then a refold. Pass: read and done state kept,
//!   nothing from `sources` lost, `/stargazers` lists all 10,000.
//! - BF-68: an EU-resident org's search documents; kill one `SearchShard`. Pass: documents only in `eu`-jurisdiction
//!   shards; `incomplete_results` until the rebuild, then full results.

#[path = "../../git/tests/common/mod.rs"]
mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::*;
use graftyard_git::inbox::{self, Inbox, Notice, Snapshot};
use graftyard_git::issues::{CommentTarget, Consumer};
use graftyard_git::search::{self, Arg, DocOp, Domain, Hit, RepoInfo, SearchShard, ShardAnswer};
use graftyard_git::watch::{self, ConsumerCursorCmd, CountsCmd, counts_plan};
use graftyard_identity::{
    IDENTITY_FORMAT, IDENTITY_FORMAT_V2, IDENTITY_FORMAT_V3, IdentityCmd, IdentityFold, IdentityOp, ThreadId,
    UserRecord,
};
use graftyard_raft::LogEntry;
use graftyard_types::{Jurisdiction, Site, SiteLink, SiteTable};

const ALICE: i64 = 201;
const BOB: i64 = 202;
const STEP_MS: u64 = 100;
/// A commit round to the far follower and back [guess: B0-12's enam to apac median is about 200 ms].
const ROUND_MS: u64 = 200;
/// How often a consumer's Queue delivers a batch [guess: Queues' default batch wait is 1 s or less].
const POLL_MS: u64 = 1_000;

fn acct(a: i64) -> AccountId {
    AccountId(DbId(a))
}

fn id_reads() -> Vec<Format> {
    vec![IDENTITY_FORMAT, IDENTITY_FORMAT_V2, IDENTITY_FORMAT_V3]
}

/// Every committed entry of a member's log, as a consumer reads them from a follower.
fn committed<C: Command>(g: &Group<C>, s: SiteId) -> Vec<LogEntry<Ev<C>>> {
    let m = &g.members[s.0 as usize];
    let c = m.state().commit;
    m.log().iter().filter(|e| e.index <= c).cloned().collect()
}

/// The whole install: identity groups, repository groups, inboxes, search shards, and the consumers' state.
struct Bed {
    now: Timestamp,
    next: u128,
    ident: Ident,
    install: Group<AuthzCmd>,
    ids: BTreeMap<i64, Group<IdentityCmd>>,
    repos: BTreeMap<RepoId, (Group<RepoCmd>, RepoSettings)>,
    inboxes: BTreeMap<i64, Inbox>,
    /// The newest snapshot of each Inbox, as the two buckets hold it.
    snapshots: BTreeMap<i64, Snapshot>,
    shards: BTreeMap<SearchShard, Shard>,
    /// Shard objects lost with their site, until a rebuild under the next generation replaces them.
    lost: BTreeSet<SearchShard>,
    /// Sites `SiteLost` named, which shard placement skips.
    lost_sites: Vec<SiteId>,
    /// The EU org's `Authz`, which grants Alice and Bob write on its repositories.
    eu_authz: AuthzFold,
    /// Each owner's placement, whose name is its residency domain.
    placements: BTreeMap<AccountId, PlacementName>,
    /// When each (account, repository) star committed and when `watch_count` first showed it.
    logins: BTreeMap<String, AccountId>,
    /// `Counts`' last commit per repository, for the one-entry-a-second pacing.
    counts_last: BTreeMap<RepoId, u64>,
}

/// One `SearchShard` object: its SQLite, and which jurisdiction it was created under.
struct Shard {
    db: rusqlite::Connection,
    jurisdiction: Option<Jurisdiction>,
    site: SiteId,
}

impl Shard {
    fn new(jurisdiction: Option<Jurisdiction>, site: SiteId) -> Shard {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        for s in search::DDL {
            db.execute_batch(s).unwrap();
        }
        Shard { db, jurisdiction, site }
    }

    fn run(&self, stmts: &[search::Stmt]) {
        for s in stmts {
            let args: Vec<Box<dyn rusqlite::ToSql>> = s.args.iter().map(to_sql).collect();
            self.db.execute(s.sql, rusqlite::params_from_iter(args.iter())).unwrap();
        }
    }

    fn count(&self) -> i64 {
        self.db.query_row("SELECT count(*) FROM doc", [], |r| r.get(0)).unwrap()
    }

    fn query(&self, sql: &str, args: &[Arg]) -> (Vec<Hit>, u64) {
        let a: Vec<Box<dyn rusqlite::ToSql>> = args.iter().map(to_sql).collect();
        let mut st = self.db.prepare(sql).unwrap();
        let mut total = 0;
        let hits = st
            .query_map(rusqlite::params_from_iter(a.iter()), |r| {
                total = r.get::<_, i64>(4)? as u64;
                Ok(Hit {
                    repo: RepoId(DbId(r.get(0)?)),
                    number: r.get::<_, i64>(1)? as u32,
                    updated: Timestamp(r.get::<_, i64>(2)? as u64),
                    score: r.get(3)?,
                })
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        (hits, total)
    }
}

fn to_sql(a: &Arg) -> Box<dyn rusqlite::ToSql> {
    match a {
        Arg::Int(i) => Box::new(*i),
        Arg::Text(t) => Box::new(t.to_string()),
    }
}

impl Bed {
    fn new() -> Bed {
        let now = Timestamp(1_793_000_000_000);
        let install = install_account();
        let mut iv =
            Group::new(ChainId::Authz(install), vec![AUTHZ_FORMAT, AUTHZ_FORMAT_V2], AuthzFold::new(install), now);
        iv.elect(now);
        let mut b = Bed {
            now,
            next: 1,
            ident: Ident::new(),
            install: iv,
            ids: BTreeMap::new(),
            repos: BTreeMap::new(),
            inboxes: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            shards: BTreeMap::new(),
            lost: BTreeSet::new(),
            lost_sites: Vec::new(),
            placements: BTreeMap::new(),
            eu_authz: AuthzFold::new(acct(EU_ORG)),
            logins: BTreeMap::new(),
            counts_last: BTreeMap::new(),
        };
        // Alice and Bob: accounts, a PAT each, and write on every test repository, which the owner's `Authz` grants.
        for a in [ALICE, BOB] {
            let id = acct(a);
            b.ident.names.apply(&NamesEvent::Account { id, suspended: false, sessions_not_before: Timestamp(0) });
            let (hash, kind, scopes) = ([a as u8; 32], TokenKind::Pat, ScopeSet(1));
            b.ident.tokens.apply(&TokensEvent::Stored { hash, kind, account: id, scopes, expires: None });
            for repo in [REPO, RepoId(DbId(8)), RepoId(DbId(9))] {
                let ev = AuthzEvent::Grant { repo, grantee: UserId(id), permission: Some(RepoPermission::Write) };
                b.ident.owner.apply(&ev);
            }
        }
        b.eu_authz.apply(&AuthzEvent::Opened { kind: AccountKind::Org, enterprise: None });
        for a in [ALICE, BOB] {
            for repo in [RepoId(DbId(8)), RepoId(DbId(9))] {
                let ev = AuthzEvent::Grant { repo, grantee: UserId(acct(a)), permission: Some(RepoPermission::Write) };
                b.eu_authz.apply(&ev);
            }
        }
        b.logins.insert("alice".into(), acct(ALICE));
        b.logins.insert("bob".into(), acct(BOB));
        b
    }

    fn rid(&mut self) -> RequestId {
        self.next += 1;
        RequestId(Ulid(self.next << 40))
    }

    /// Moves time on and ticks every member, so leaders heartbeat and followers learn the commit index.
    fn pass(&mut self, ms: u64) {
        self.now = Timestamp(self.now.0 + ms);
        let now = self.now;
        for s in SITES {
            self.install.on(s, now, |m, t| m.tick(t));
            for (g, _) in self.repos.values_mut() {
                g.on(s, now, |m, t| m.tick(t));
            }
            for g in self.ids.values_mut() {
                g.on(s, now, |m, t| m.tick(t));
            }
        }
        self.install.deliver(now);
        self.settle();
    }

    // ---- groups ----

    fn identity(&mut self, a: i64) -> &mut Group<IdentityCmd> {
        let now = self.now;
        self.ids.entry(a).or_insert_with(|| {
            let mut g = Group::new(ChainId::Identity(acct(a)), id_reads(), IdentityFold::new(acct(a)), now);
            g.elect(now);
            // The signup's `Created`, which only the operator sends.
            let capture = graftyard_identity::AccountCapture {
                login: graftyard_identity::Login::parse(&format!("u{a}")).unwrap(),
                kind: AccountKind::User,
                created: now,
            };
            let l = g.leader();
            let cmd = IdentityCmd { cred: CredentialShard::NONE, op: IdentityOp::Create(capture) };
            g.on(l, now, |m, t| m.commit(t, RequestId(Ulid(1)), cmd)).expect("admitted");
            g.deliver(now);
            g
        })
    }

    fn add_repo(&mut self, repo: RepoId, owner: i64, public: bool) {
        let now = self.now;
        let mut g = Group::new(ChainId::Repo(repo), repo_reads(), RepoFold::new(repo, acct(owner)), now);
        g.elect(now);
        let l = g.leader();
        g.on(l, now, |m, t| m.id_block(t, graftyard_types::IdBlock::whole(0, 3 + repo.0.0 as u32).unwrap())).unwrap();
        g.deliver(now);
        let mut s = settings();
        s.owner = acct(owner);
        if public {
            s.visibility = Visibility::Public;
        }
        let rid = self.rid();
        g.on(l, now, |m, t| m.record(t, rid, REPO_FORMAT_V1, vec![RepoEvent::Settings(s.clone())])).unwrap();
        g.deliver(now);
        self.repos.insert(repo, (g, s));
    }

    /// Delivers every group's messages and answers deps, ticking until quiet, as `World::settle` does.
    fn settle(&mut self) {
        for _ in 0..50 {
            let install = self.install.fresh(self.now);
            let mut busy = false;
            for (g, _) in self.repos.values_mut() {
                g.deliver(self.now);
                for (s, want) in std::mem::take(&mut g.deps) {
                    busy = true;
                    let deps = want
                        .chains
                        .iter()
                        .map(|c| match c {
                            ChainId::Authz(o) if *o == acct(EU_ORG) => (*c, IdentityDep::Authz(self.eu_authz.clone())),
                            _ => (*c, self.ident.dep(*c, &install)),
                        })
                        .collect();
                    g.on(s, self.now, |m, t| m.deps_read(t, want.id, deps));
                }
                g.deliver(self.now);
            }
            for g in self.ids.values_mut() {
                g.deliver(self.now);
                for (s, want) in std::mem::take(&mut g.deps) {
                    busy = true;
                    let deps = want.chains.iter().map(|c| (*c, self.ident.dep(*c, &install))).collect();
                    g.on(s, self.now, |m, t| m.deps_read(t, want.id, deps));
                }
                g.deliver(self.now);
            }
            if !busy {
                return;
            }
        }
        panic!("never settled");
    }

    fn id_cmd(&mut self, a: i64, op: IdentityOp) {
        let rid = self.rid();
        let cred = if matches!(op, IdentityOp::CountsCursor { .. }) {
            CredentialShard::NONE
        } else {
            CredentialShard { account: Some(acct(a)), key: None }
        };
        let now = self.now;
        let g = self.identity(a);
        let l = g.leader();
        let t = g.on(l, now, |m, t| m.commit(t, rid, IdentityCmd { cred, op })).expect("admitted").ticket;
        self.settle();
        self.pass(ROUND_MS);
        let g = self.ids.get(&a).unwrap();
        let r = g.resolutions.iter().rev().find_map(|r| match r {
            Resolution::Ticket { ticket, result } if *ticket == t => Some(result.clone()),
            _ => None,
        });
        assert!(matches!(r, Some(Ok(_))), "identity command on {a}: {r:?}");
    }

    fn repo_cmd(&mut self, repo: RepoId, c: RepoCmd) -> RepoOut {
        let rid = self.rid();
        let now = self.now;
        let (g, _) = self.repos.get_mut(&repo).unwrap();
        let l = g.leader();
        let t = g.on(l, now, |m, t| m.commit(t, rid, c)).expect("admitted").ticket;
        self.settle();
        self.pass(ROUND_MS);
        let (g, _) = self.repos.get(&repo).unwrap();
        match g.resolutions.iter().rev().find_map(|r| match r {
            Resolution::Ticket { ticket, result } if *ticket == t => Some(result.clone()),
            _ => None,
        }) {
            Some(Ok(a)) => a.out,
            other => panic!("repository command: {other:?}"),
        }
    }

    fn issue(&mut self, repo: RepoId, actor: i64, op: IssueOp<graftyard_git::blob::BlobRef>) -> IssueOut {
        let rid = self.rid();
        let call = IssueCall::new(op, user(actor), rid, serde_json::Value::Null);
        match self.repo_cmd(repo, RepoCmd::Issue(Box::new(call))) {
            RepoOut::Issue(o) => o.out,
            other => panic!("{other:?}"),
        }
    }

    fn follower_fold(&self, repo: RepoId) -> RepoFold {
        let (g, _) = &self.repos[&repo];
        let f = SITES.iter().find(|s| **s != g.leader()).unwrap();
        g.members[f.0 as usize].group_fold().fold.clone()
    }

    fn leader_fold(&mut self, repo: RepoId) -> RepoFold {
        let now = self.now;
        self.repos.get_mut(&repo).unwrap().0.fresh(now)
    }

    // ---- the consumers ----

    /// One `Counts` pass over every account, at most one entry a second per repository.
    fn run_counts(&mut self) {
        let accounts: Vec<i64> = self.ids.keys().copied().collect();
        for a in accounts {
            let g = &self.ids[&a];
            let l = g.leader();
            let fold = g.members[l.0 as usize].group_fold().fold.clone();
            let applied = g.members[l.0 as usize].group_fold().applied();
            if applied <= fold.user().counts_cursor {
                continue;
            }
            let entries = committed(g, l);
            let plan = counts_plan(acct(a), &fold, applied, &entries);
            let mut all_done = true;
            for (repo, ups) in plan {
                if self.counts_last.get(&repo).is_some_and(|t| self.now.0 < t + watch::COUNTS_EVERY_MS) {
                    all_done = false;
                    continue;
                }
                for chunk in ups.chunks(watch::COUNTS_BATCH_MAX) {
                    let rid = self.rid();
                    self.repo_cmd(repo, RepoCmd::Counts(CountsCmd { updates: chunk.to_vec(), request: rid }));
                }
                self.counts_last.insert(repo, self.now.0);
            }
            if all_done {
                self.id_cmd(a, IdentityOp::CountsCursor { through: applied });
            }
        }
    }

    /// `Counts` over accounts whose `Identity` folds are held outside `ids` (BF-67's 10,000 stargazers): one plan per
    /// account, each repository's updates batched across accounts as the consumer's queue does, one `CountsCmd` per
    /// `COUNTS_BATCH_MAX` updates and at most one entry a second per repository. Returns the commands it sent.
    fn counts_for(
        &mut self,
        accounts: &[(AccountId, IdentityFold, Vec<LogEntry<graftyard_identity::IdentityEvent>>)],
    ) -> Vec<RepoCmd> {
        let mut by_repo: BTreeMap<RepoId, Vec<watch::WatcherUpdate>> = BTreeMap::new();
        for (a, fold, log) in accounts {
            let applied = log.last().map_or(graftyard_types::Index(0), |e| e.index);
            for (r, ups) in counts_plan(*a, fold, applied, log) {
                by_repo.entry(r).or_default().extend(ups);
            }
        }
        let mut sent = Vec::new();
        for (repo, ups) in by_repo {
            for chunk in ups.chunks(watch::COUNTS_BATCH_MAX) {
                let rid = self.rid();
                let c = RepoCmd::Counts(CountsCmd { updates: chunk.to_vec(), request: rid });
                self.repo_cmd(repo, c.clone());
                sent.push(c);
                self.pass(watch::COUNTS_EVERY_MS);
            }
        }
        sent
    }

    /// One `Notify` and `Search` pass over every repository, from each consumer's cursor, on a follower.
    fn run_notify_and_search(&mut self) {
        let repos: Vec<RepoId> = self.repos.keys().copied().collect();
        for repo in repos {
            let f = self.follower_fold(repo);
            let (g, s) = &self.repos[&repo];
            let fl = SITES.iter().find(|x| **x != g.leader()).copied().unwrap();
            let entries = committed(g, fl);
            let applied = g.members[fl.0 as usize].group_fold().applied();
            let notify_from = tiers::consumer_cursor(&*f.core, Consumer::Notify);
            let search_from = tiers::consumer_cursor(&*f.core, Consumer::Search);
            let public = s.visibility == Visibility::Public;
            let owner = s.owner;
            let logins = self.logins.clone();
            let login = |l: &str| logins.get(l).copied();
            for e in entries.iter().filter(|e| e.index > notify_from) {
                let mut by: BTreeMap<AccountId, Vec<Notice>> = BTreeMap::new();
                for (a, n) in inbox::notices(repo, &f.meta, e, &login) {
                    by.entry(a).or_default().push(n);
                }
                for (a, ns) in by {
                    self.inboxes.entry(a.0.0).or_default().apply(ChainId::Repo(repo), e.index, &ns);
                }
            }
            let info = RepoInfo { repo, owner, public };
            let mut reached = true;
            for e in entries.iter().filter(|e| e.index > search_from) {
                for op in search::docs_for(info, &f.meta, e) {
                    reached &= self.search_write(info, op, &f);
                }
            }
            let rid = self.rid();
            let moving: &[Consumer] = if reached { &[Consumer::Notify, Consumer::Search] } else { &[Consumer::Notify] };
            for c in moving.iter().copied() {
                if applied > tiers::consumer_cursor(&*f.core, c) {
                    let rid = RequestId(Ulid(rid.0.0 + c as u128));
                    self.repo_cmd(
                        repo,
                        RepoCmd::ConsumerCursor(ConsumerCursorCmd { consumer: c, through: applied, request: rid }),
                    );
                }
            }
        }
        // Each Inbox writes its snapshot on a day it changed.
        let now = self.now;
        for (a, i) in self.inboxes.iter_mut() {
            if i.snapshot_due(now) {
                self.snapshots.insert(*a, i.snapshot(now));
            }
        }
    }

    // ---- search ----

    fn sites() -> SiteTable {
        let site = |id: u8, region, jurisdiction| Site {
            id: SiteId(id),
            script: format!("graft-site-{id}"),
            account: "a".into(),
            region,
            jurisdiction,
            bucket: format!("b{id}"),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare: false,
        };
        SiteTable(vec![
            site(0, Region::Enam, None),
            site(1, Region::Weur, None),
            site(2, Region::Apac, None),
            site(3, Region::Weur, Some(Jurisdiction::Eu)),
            site(4, Region::Eeur, Some(Jurisdiction::Eu)),
        ])
    }

    fn domain_of(&self, owner: AccountId) -> Domain {
        Domain::of(self.placements.get(&owner).copied().unwrap_or(PlacementName::Tri))
    }

    /// The domain's shard object for `n`, its newest generation, created on first use; `None` while it is lost.
    fn shard_obj(&mut self, domain: Domain, n: u8) -> Option<SearchShard> {
        let newest = self
            .shards
            .keys()
            .chain(self.lost.iter())
            .filter(|k| k.domain == domain && k.shard == n)
            .map(|k| k.generation)
            .max()
            .unwrap_or(1);
        let key = SearchShard { domain, shard: n, generation: newest };
        if self.lost.contains(&key) {
            return None;
        }
        if !self.shards.contains_key(&key) {
            let site = key.site(&Self::sites(), &self.lost_sites).expect("a site for the domain");
            self.shards.insert(key, Shard::new(domain.jurisdiction(), site));
        }
        Some(key)
    }

    /// One `Search` write; `false` when the shard is unreachable, so the consumer pauses that chain.
    fn search_write(&mut self, info: RepoInfo, op: DocOp, f: &RepoFold) -> bool {
        let Some(key) = self.shard_obj(self.domain_of(info.owner), search::shard_of(info.repo)) else { return false };
        let sh = &self.shards[&key];
        match op {
            DocOp::Put(d) => sh.run(&d.stmts()),
            DocOp::Delete(n) => sh.run(&search::delete_stmts(info.repo, n)),
            DocOp::Repo => {
                sh.run(&search::delete_repo_stmts(info.repo));
                for d in search::all_docs(info, &f.core, &f.meta) {
                    sh.run(&d.stmts());
                }
            }
        }
        true
    }

    /// `GET /search/issues?q=` for a viewer whose orgs live in `domains`, with lost shards left out.
    fn search(&self, q: &str, domains: &[Domain], reach: &[RepoId]) -> search::Merged {
        let query = search::parse(q);
        let resolved = search::Resolved::default();
        let (sql, args) = search::query_sql(&query, &resolved, &search::Reach(reach.to_vec()), false, 100).unwrap();
        let newest = |d: Domain, s: u8| {
            self.shards
                .keys()
                .chain(self.lost.iter())
                .filter(|k| k.domain == d && k.shard == s)
                .map(|k| k.generation)
                .max()
                .unwrap_or(1)
        };
        let answers: Vec<ShardAnswer> = search::shards_for(domains, newest)
            .into_iter()
            .map(|k| {
                if self.lost.contains(&k) {
                    None
                } else {
                    Some(self.shards.get(&k).map_or((Vec::new(), 0), |s| s.query(&sql, &args)))
                }
            })
            .collect();
        search::merge(answers, false, 1, 100)
    }

    /// A shard rebuild under the next generation on the domain's next site, from every repository's metadata tier
    /// that `MemberRegistry` lists.
    fn rebuild_shard(&mut self, old: SearchShard) -> SearchShard {
        let new = SearchShard { generation: old.generation + 1, ..old };
        let site = new.site(&Self::sites(), &self.lost_sites).expect("the domain's next site");
        self.shards.insert(new, Shard::new(old.domain.jurisdiction(), site));
        let repos: Vec<RepoId> = self.repos.keys().copied().collect();
        for repo in repos {
            let s = self.repos[&repo].1.clone();
            if self.domain_of(s.owner) != old.domain || search::shard_of(repo) != old.shard {
                continue;
            }
            let f = self.follower_fold(repo);
            let info = RepoInfo { repo, owner: s.owner, public: s.visibility == Visibility::Public };
            for d in search::all_docs(info, &f.core, &f.meta) {
                self.shards[&new].run(&d.stmts());
            }
        }
        new
    }

    // ---- the Inbox ----

    /// DESIGN's rebuild: the newest snapshot, then each chain in `sources` from its index and each watched or
    /// subscribed repository from INBOX_WINDOW before the snapshot, read from a follower's log.
    fn rebuild_inbox(&mut self, a: i64) -> Inbox {
        let snap = self.snapshots.get(&a).cloned().expect("a snapshot");
        let g = &self.ids[&a];
        let user = g.members[g.leader().0 as usize].group_fold().fold.user().clone();
        let mut ib = snap.inbox();
        let logins = self.logins.clone();
        let login = |l: &str| logins.get(l).copied();
        for (chain, from) in snap.rebuild_from(&user) {
            let ChainId::Repo(repo) = chain else { continue };
            let Some((g, _)) = self.repos.get(&repo) else { continue };
            let fl = SITES.iter().find(|x| **x != g.leader()).copied().unwrap();
            let f = g.members[fl.0 as usize].group_fold().fold.clone();
            for e in committed(g, fl).iter().filter(|e| inbox::replays(from, e)) {
                let ns: Vec<Notice> = inbox::notices(repo, &f.meta, e, &login)
                    .into_iter()
                    .filter(|(x, _)| *x == acct(a))
                    .map(|(_, n)| n)
                    .collect();
                ib.apply(chain, e.index, &ns);
            }
        }
        ib
    }

    fn user(&self, a: i64) -> graftyard_identity::UserRows {
        let g = &self.ids[&a];
        g.members[g.leader().0 as usize].group_fold().fold.user().clone()
    }
}

const EU_ORG: i64 = 300;

fn open_issue(title: &str, body: &str) -> IssueOp<graftyard_git::blob::BlobRef> {
    IssueOp::OpenIssue { title: title.into(), body: body.into(), labels: SmallVec::new(), assignees: SmallVec::new() }
}

fn comment(n: u32, body: &str) -> IssueOp<graftyard_git::blob::BlobRef> {
    IssueOp::Comment { on: CommentTarget::Issue(IssueNumber(n)), body: body.into(), reply_to: None }
}

fn number(o: IssueOut) -> u32 {
    match o {
        IssueOut::Number(n) => n.0,
        other => panic!("{other:?}"),
    }
}

/// Runs the pipeline every STEP_MS until `done` holds or `limit_ms` passes; the time it took.
fn until(b: &mut Bed, limit_ms: u64, done: &mut dyn FnMut(&mut Bed) -> bool) -> Option<u64> {
    let start = b.now.0;
    let mut polled = 0;
    while b.now.0 - start <= limit_ms {
        if b.now.0 >= polled + POLL_MS {
            polled = b.now.0;
            b.run_counts();
            b.run_notify_and_search();
        }
        if done(b) {
            return Some(b.now.0 - start);
        }
        b.pass(STEP_MS);
    }
    None
}

#[test]
fn bf5_a_star_and_a_watched_comment_show_within_10_s_and_a_rebuilt_inbox_keeps_read_state() {
    let mut b = Bed::new();
    b.add_repo(REPO, OWNER, true);
    // Bob watches the repository and opened an issue; Alice will star it and comment.
    b.identity(ALICE);
    b.id_cmd(BOB, IdentityOp::SetRecord(UserRecord::Watch { repo: REPO, subscribed: true, ignored: false }));
    // The watch reaches the repository's watcher index through `Counts` before the issue opens.
    until(&mut b, 60_000, &mut |b| watch::subscribers(&b.follower_fold(REPO).meta) == vec![acct(BOB)]).unwrap();
    let n = number(b.issue(REPO, OWNER, open_issue("first", "an issue")));
    let first = until(&mut b, 60_000, &mut |b| b.inboxes.get(&BOB).is_some_and(|i| !i.threads.is_empty())).unwrap();
    // Bob read that thread before the new comment.
    let thread = ThreadId { repo: REPO, number: n };
    b.pass(1_000);
    b.id_cmd(BOB, IdentityOp::SetRecord(UserRecord::Thread { thread, read: true, done: false }));
    let n2 = number(b.issue(REPO, OWNER, open_issue("second", "another")));
    let thread2 = ThreadId { repo: REPO, number: n2 };
    until(&mut b, 60_000, &mut |b| b.inboxes[&BOB].threads.contains_key(&thread2)).unwrap();
    b.pass(1_000);
    b.id_cmd(BOB, IdentityOp::SetRecord(UserRecord::Thread { thread: thread2, read: true, done: false }));
    // The next day: the Inbox's alarm writes the snapshot of the day it changed.
    b.pass(inbox::DAY_MS);
    b.run_notify_and_search();
    assert!(b.snapshots.contains_key(&BOB), "the day's snapshot");

    // The star and the comment on the watched issue, at t0, after the snapshot.
    let t0 = b.now.0;
    b.id_cmd(ALICE, IdentityOp::SetRecord(UserRecord::Star { repo: REPO, on: true }));
    b.issue(REPO, ALICE, comment(n, "hello @bob"));
    let mut star_at = None;
    let mut note_at = None;
    let lag = until(&mut b, 60_000, &mut |b| {
        let f = b.leader_fold(REPO);
        let host = graftyard_api::wire::Host::new("ghes.graftyard.test", false).unwrap();
        let dir = Dir;
        let r = graftyard_api::view::Repo { host: &host, at: dir.home(), core: &f.core, meta: &f.meta, dir: &dir };
        let full = graftyard_api::repos::full_repository(&r);
        if star_at.is_none() && full["stargazers_count"] == 1 {
            star_at = Some(b.now.0 - t0);
        }
        let user = b.user(BOB);
        let list = b.inboxes[&BOB].view(&user, false, false, None);
        if note_at.is_none() && list.iter().any(|l| l.thread == thread && l.unread) {
            note_at = Some(b.now.0 - t0);
        }
        star_at.is_some() && note_at.is_some()
    })
    .expect("both show within 60 s");
    let (star_ms, note_ms) = (star_at.unwrap(), note_at.unwrap());
    println!(
        "BF-5: first notice after {first} ms; star in stargazers_count after {star_ms} ms, comment in /notifications after {note_ms} ms (both by {lag} ms)"
    );
    assert!(star_ms <= 10_000 && note_ms <= 10_000, "star {star_ms} ms, notice {note_ms} ms");

    // The comment's thread is unread again (a notice after the read mark); the other thread stays read.
    let user = b.user(BOB);
    let before = b.inboxes[&BOB].view(&user, true, false, None);
    let t2 = before.iter().find(|l| l.thread == thread2).unwrap();
    assert!(!t2.unread && t2.last_read.is_some(), "thread 2 is read");

    // Delete Bob's Inbox and rebuild it from its newest snapshot, which predates the comment: the comment's chain is in
    // `sources`, so the rebuild replays it.
    b.inboxes.remove(&BOB);
    let rebuilt = b.rebuild_inbox(BOB);
    let after = rebuilt.view(&user, true, false, None);
    let key =
        |v: &[inbox::Listed]| v.iter().map(|l| (l.thread, l.unread, l.last_read, l.row.reason)).collect::<Vec<_>>();
    assert_eq!(key(&after), key(&before), "the rebuilt Inbox lists the same threads with the same read state");
    println!("BF-5: rebuilt Inbox lists {} threads, read state kept", after.len());
}

/// A `Directory` for the renderers: every account has a login.
struct Dir;

impl graftyard_api::view::Directory for Dir {
    fn login(&self, id: AccountId) -> Option<String> {
        Some(format!("u{}", id.0.0))
    }
    fn name(&self, _: AccountId) -> Option<String> {
        None
    }
    fn is_org(&self, id: AccountId) -> bool {
        id == acct(EU_ORG)
    }
    fn by_login(&self, l: &str) -> Option<AccountId> {
        l.strip_prefix('u').and_then(|n| n.parse().ok()).map(acct)
    }
    fn permission(&self, _: AccountId) -> Option<RepoPermission> {
        None
    }
    fn repo(&self, _: RepoId) -> Option<graftyard_api::view::RepoRef> {
        None
    }
    fn ref_of(&self, _: RepoId, _: &RefName) -> Option<ObjectId> {
        None
    }
    fn team(&self, _: DbId) -> Option<(String, String, String)> {
        None
    }
    fn viewer(&self) -> Option<AccountId> {
        None
    }
    fn compare(&self, _: ObjectId, _: ObjectId) -> graftyard_api::view::Compare {
        graftyard_api::view::Compare { commits: 0, additions: 0, deletions: 0, changed_files: 0 }
    }
}

impl Dir {
    fn home(&self) -> graftyard_api::view::RepoRef {
        graftyard_api::view::RepoRef { id: REPO, owner: "o".into(), owner_id: acct(OWNER), name: "r".into() }
    }
}

/// Each account's threads as `/notifications?all=true` lists them, with what read and done state shows.
fn listing(
    i: &Inbox,
    user: &graftyard_identity::UserRows,
) -> Vec<(ThreadId, bool, Option<Timestamp>, inbox::Reason, Timestamp)> {
    i.view(user, true, false, None)
        .iter()
        .map(|l| (l.thread, l.unread, l.last_read, l.row.reason, l.row.updated))
        .collect()
}

#[test]
fn bf67_a_rebuilt_inbox_keeps_read_done_and_every_sources_event_and_10_000_stars_survive_a_refold() {
    let mut b = Bed::new();
    let (r1, r2, r3) = (REPO, RepoId(DbId(8)), RepoId(DbId(9)));
    for r in [r1, r2, r3] {
        b.add_repo(r, OWNER, true);
    }
    b.identity(ALICE);
    // Bob watches r1, subscribes to one thread of r2, and is only mentioned on r3.
    b.id_cmd(BOB, IdentityOp::SetRecord(UserRecord::Watch { repo: r1, subscribed: true, ignored: false }));
    let r5 = RepoId(DbId(11));
    b.add_repo(r5, OWNER, true);
    b.id_cmd(BOB, IdentityOp::SetRecord(UserRecord::Watch { repo: r5, subscribed: true, ignored: false }));
    let t2 = number(b.issue(r2, OWNER, open_issue("r2 thread", "x")));
    let sub = ThreadId { repo: r2, number: t2 };
    b.id_cmd(BOB, IdentityOp::SetRecord(UserRecord::Subscription { thread: sub, subscribed: true }));
    until(&mut b, 60_000, &mut |b| {
        !watch::subscribers(&b.follower_fold(r1).meta).is_empty()
            && !watch::subscribers(&b.follower_fold(r5).meta).is_empty()
    })
    .unwrap();

    // Day 1: five threads on r1, comments on the r2 thread, a mention on r3.
    let mut r1_threads = Vec::new();
    for i in 0..5 {
        r1_threads.push(number(b.issue(r1, OWNER, open_issue(&format!("r1 #{i}"), "body"))));
    }
    b.issue(r2, ALICE, comment(t2, "first reply"));
    let t3 = number(b.issue(r3, ALICE, open_issue("r3", "cc @bob")));
    until(&mut b, 60_000, &mut |b| b.inboxes.get(&BOB).is_some_and(|i| i.threads.len() == 7)).expect("seven threads");
    // Bob reads two, marks one done.
    let th = |r, n| ThreadId { repo: r, number: n };
    for (n, read, done) in [(r1_threads[0], true, false), (r1_threads[1], true, false), (r1_threads[2], true, true)] {
        b.id_cmd(BOB, IdentityOp::SetRecord(UserRecord::Thread { thread: th(r1, n), read, done }));
    }
    // The day's snapshot, written by the alarm at the next day's start.
    b.pass(inbox::DAY_MS);
    b.run_notify_and_search();
    let snap_day1 = b.snapshots[&BOB].clone();
    assert_eq!(snap_day1.sources.len(), 3, "r1, r2, and r3 are in sources");

    // Day 2: more notices on every sources chain, a notice on the done thread, and a first mention from a chain the
    // snapshot never saw.
    let r4 = RepoId(DbId(10));
    b.add_repo(r4, OWNER, true);
    b.issue(r1, ALICE, comment(r1_threads[3], "day two on r1"));
    b.issue(r2, ALICE, comment(t2, "day two on r2"));
    // A mention on r3, which Bob reaches only through `sources`.
    b.issue(r3, ALICE, comment(t3, "day two on r3, @bob"));
    // A first notice from r5, which Bob watches and which no snapshot named: the rebuild reaches it through
    // INBOX_WINDOW.
    let t5 = number(b.issue(r5, OWNER, open_issue("r5", "watched, first notice on day two")));
    b.issue(r1, ALICE, comment(r1_threads[4], "day two on r1 #4"));
    let t4 = number(b.issue(r4, ALICE, open_issue("r4", "hey @bob")));
    until(&mut b, 60_000, &mut |b| {
        let i = &b.inboxes[&BOB];
        i.threads.contains_key(&th(r4, t4)) && i.threads.contains_key(&th(r5, t5))
    })
    .unwrap();
    assert_eq!(b.inboxes[&BOB].threads[&th(r3, t3)].reason, inbox::Reason::Mention, "the day-two mention on r3");
    // A read after the day-two notice on r1 #3.
    b.id_cmd(BOB, IdentityOp::SetRecord(UserRecord::Thread { thread: th(r1, r1_threads[3]), read: true, done: false }));
    b.run_notify_and_search();
    let user = b.user(BOB);
    let before = listing(&b.inboxes[&BOB], &user);
    let done = th(r1, r1_threads[2]);
    assert!(!before.iter().any(|t| t.0 == done), "the done thread is hidden");
    assert!(before.iter().any(|t| t.0 == th(r1, r1_threads[0]) && !t.1 && t.2.is_some()), "a read thread is read");

    // Delete the Inbox mid-day-two and rebuild it from the newest snapshot, day one's.
    let live = b.inboxes.remove(&BOB).unwrap();
    let rebuilt = b.rebuild_inbox(BOB);
    let after = listing(&rebuilt, &user);
    // Expected, derived from what the test did: every thread but r4's, whose chain no snapshot named and Bob neither
    // watches nor subscribes to (DESIGN: missing until that chain's next event).
    let want: Vec<_> = before.iter().filter(|t| t.0.repo != r4).cloned().collect();
    assert_eq!(after, want, "rebuilt from the day-one snapshot");
    for (c, i) in &snap_day1.sources {
        assert!(rebuilt.sources[c] >= *i, "{c}: the rebuild replayed from its sources index");
    }
    // The next day's snapshot holds day two, so a rebuild from it loses nothing.
    b.inboxes.insert(BOB, live);
    b.pass(inbox::DAY_MS);
    b.run_notify_and_search();
    b.inboxes.remove(&BOB);
    let rebuilt2 = b.rebuild_inbox(BOB);
    assert_eq!(listing(&rebuilt2, &user), before, "rebuilt from the day-two snapshot");
    println!(
        "BF-67: Inbox of {} threads rebuilt from day one's snapshot ({} kept, r4's first mention missing as designed) and from day two's (all {} kept, read and done state kept)",
        before.len(),
        after.len(),
        before.len()
    );

    // ---- 10,000 stars, then a refold ----
    const N: i64 = 10_000;
    let mut accounts = Vec::new();
    for i in 0..N {
        let a = acct(10_000 + i);
        let mut f = IdentityFold::new(a);
        // Each stargazer's own `Identity` chain: its `Created`, then the star, as a member applies them.
        let star_at = Timestamp(b.now.0 + i as u64);
        let mut log = Vec::new();
        for (k, ev) in [
            graftyard_identity::IdentityEvent::Created(graftyard_identity::AccountCapture {
                login: graftyard_identity::Login::parse(&format!("u{}", 10_000 + i)).unwrap(),
                kind: AccountKind::User,
                created: star_at,
            }),
            graftyard_identity::IdentityEvent::Record(UserRecord::Star { repo: r1, on: true }),
        ]
        .into_iter()
        .enumerate()
        {
            let e = LogEntry {
                term: graftyard_raft::Term(1),
                index: graftyard_types::Index(k as u64 + 1),
                prev: graftyard_raft::PREV_ZERO,
                format: IDENTITY_FORMAT_V3,
                tiers: graftyard_raft::TierSet::CORE,
                at: star_at,
                requests: SmallVec::new(),
                extend: None,
                body: graftyard_raft::EntryBody::Events(vec![ev]),
            };
            graftyard_raft::Fold::apply_entry(&mut f, &e);
            log.push(e);
        }
        accounts.push((a, f, log));
    }
    let sent = b.counts_for(&accounts);
    // A redelivery of every batch, newest first, changes nothing.
    for c in sent.iter().rev() {
        assert_eq!(b.repo_cmd(r1, c.clone()), RepoOut::Counts(watch::CountsOut { applied: 0 }), "a redelivered batch");
    }
    let host = graftyard_api::wire::Host::new("ghes.graftyard.test", false).unwrap();
    let dir = Dir;
    let pages = |f: &RepoFold| {
        let r = graftyard_api::view::Repo { host: &host, at: dir.home(), core: &f.core, meta: &f.meta, dir: &dir };
        let mut seen = Vec::new();
        for page in 1.. {
            let reply = graftyard_api::activity::stargazers(&r, &format!("per_page=100&page={page}"), STAR);
            let items = reply.value().as_array().unwrap().clone();
            if items.is_empty() {
                break;
            }
            for it in items {
                seen.push((it["user"]["id"].as_i64().unwrap(), it["starred_at"].as_str().unwrap().to_owned()));
            }
        }
        (seen, graftyard_api::repos::full_repository(&r)["stargazers_count"].clone())
    };
    let live = b.leader_fold(r1);
    let (seen, count) = pages(&live);
    let ids: BTreeSet<i64> = seen.iter().map(|(i, _)| *i).collect();
    assert_eq!((seen.len(), ids.len(), count), (N as usize, N as usize, serde_json::json!(N)));
    assert!(ids.iter().all(|i| (10_000..10_000 + N).contains(i)), "only the stargazers");
    assert!(seen.windows(2).all(|w| w[0].1 <= w[1].1), "oldest star first");

    // The refold: the repository's whole committed log replayed into a new fold, as a new member or the fold audit
    // builds it.
    let (g, _) = &b.repos[&r1];
    let mut refold = RepoFold::new(r1, acct(OWNER));
    for e in committed(g, g.leader()) {
        graftyard_raft::Fold::apply_entry(&mut refold, &e);
    }
    assert_eq!(
        graftyard_fold::fold_digest(&*refold.meta, graftyard_types::Tier::Metadata),
        graftyard_fold::fold_digest(&*live.meta, graftyard_types::Tier::Metadata),
        "the refold's metadata tier is the live one"
    );
    let (again, count) = pages(&refold);
    assert_eq!((again, count), (seen.clone(), serde_json::json!(N)), "/stargazers lists all 10,000 after the refold");
    println!(
        "BF-67: {} stargazers over {} Counts entries; /stargazers lists all {} live and after a refold",
        N,
        sent.len(),
        seen.len()
    );

    // Then the first stargazer unstars, the second watches, the third watches and ignores, and the original star
    // batch arrives again after them: the list and counts follow the newer state, never the older batch.
    let later = Timestamp(b.now.0 + 1);
    for (i, rec) in [
        vec![UserRecord::Star { repo: r1, on: false }],
        vec![UserRecord::Watch { repo: r1, subscribed: true, ignored: false }],
        vec![
            UserRecord::Watch { repo: r1, subscribed: true, ignored: false },
            UserRecord::Watch { repo: r1, subscribed: true, ignored: true },
        ],
    ]
    .into_iter()
    .enumerate()
    {
        let (_, f, log) = &mut accounts[i];
        for r in rec {
            let e = LogEntry {
                index: graftyard_types::Index(log.len() as u64 + 1),
                at: later,
                body: graftyard_raft::EntryBody::Events(vec![graftyard_identity::IdentityEvent::Record(r)]),
                ..log[0].clone()
            };
            graftyard_raft::Fold::apply_entry(f, &e);
            log.push(e);
        }
    }
    let changed: Vec<_> = accounts[..3].iter().map(|(a, f, log)| (*a, f.clone(), log[2..].to_vec())).collect();
    b.counts_for(&changed);
    b.repo_cmd(r1, sent[0].clone());
    let f = b.leader_fold(r1);
    let (seen, count) = pages(&f);
    assert_eq!((seen.len(), count), (N as usize - 1, serde_json::json!(N - 1)), "the unstar removed one stargazer");
    assert!(!seen.iter().any(|(i, _)| *i == 10_000), "the unstarred account is gone from /stargazers");
    let r = graftyard_api::view::Repo { host: &host, at: dir.home(), core: &f.core, meta: &f.meta, dir: &dir };
    let subs: Vec<i64> = graftyard_api::activity::subscribers(&r, "")
        .value()
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["id"].as_i64().unwrap())
        .collect();
    assert!(
        subs.contains(&10_001) && !subs.contains(&10_002),
        "a watcher subscribes; an ignoring one does not: {subs:?}"
    );
    let count = graftyard_api::repos::full_repository(&r)["subscribers_count"].clone();
    assert_eq!(count, serde_json::json!(subs.len()), "subscribers_count is the list's length");
}

const STAR: &str = "application/vnd.github.star+json";

#[test]
fn bf68_eu_documents_stay_in_eu_shards_a_killed_shard_reports_incomplete_until_its_rebuild() {
    let mut b = Bed::new();
    b.placements.insert(acct(EU_ORG), PlacementName::Eu2w);
    let (us_repo, eu_pub, eu_priv) = (REPO, RepoId(DbId(8)), RepoId(DbId(9)));
    b.add_repo(us_repo, OWNER, true);
    b.add_repo(eu_pub, EU_ORG, true);
    b.add_repo(eu_priv, EU_ORG, false);
    b.identity(ALICE);
    // (repository, title) of every document the test writes, which the expected results derive from.
    let mut made: Vec<(RepoId, u32, String)> = Vec::new();
    for (repo, k) in [(us_repo, 4), (eu_pub, 6), (eu_priv, 3)] {
        for i in 0..k {
            let title = format!("kumquat {} {i}", repo.0.0);
            let n = number(b.issue(repo, ALICE, open_issue(&title, "shared fruit")));
            made.push((repo, n, title));
        }
    }
    b.issue(eu_pub, ALICE, comment(made.iter().find(|m| m.0 == eu_pub).unwrap().1, "a durian in a comment"));
    until(&mut b, 60_000, &mut |b| b.shards.values().map(Shard::count).sum::<i64>() == made.len() as i64)
        .expect("indexed");

    // Residency: every EU-owned document is in an `eu`-jurisdiction shard on an `eu` site, and only there.
    let sites = Bed::sites();
    for (k, sh) in &b.shards {
        let eu_docs: i64 = sh.db.query_row("SELECT count(*) FROM doc WHERE owner = ?", [EU_ORG], |r| r.get(0)).unwrap();
        if k.domain == Domain::Eu {
            assert_eq!(sh.jurisdiction, Some(Jurisdiction::Eu), "{}", k.do_name());
            assert_eq!(
                sites.get(sh.site).unwrap().jurisdiction,
                Some(Jurisdiction::Eu),
                "{} on an eu site",
                k.do_name()
            );
        } else {
            assert_eq!(eu_docs, 0, "{} holds an EU document", k.do_name());
        }
    }
    let want = |m: &[(RepoId, u32, String)], reach: &[RepoId]| -> BTreeSet<(RepoId, u32)> {
        m.iter().filter(|x| x.0 != eu_priv || reach.contains(&eu_priv)).map(|x| (x.0, x.1)).collect()
    };
    let got = |r: &search::Merged| r.hits.iter().map(|h| (h.repo, h.number)).collect::<BTreeSet<_>>();
    let full = b.search("kumquat", &[Domain::Eu], &[eu_priv]);
    assert_eq!((got(&full), full.incomplete, full.total), (want(&made, &[eu_priv]), false, made.len() as u64));
    let outside = b.search("kumquat", &[Domain::Eu], &[]);
    assert_eq!(got(&outside), want(&made, &[]), "no private document without reach");
    let in_comments = b.search("durian in:comments", &[Domain::Eu], &[]);
    assert_eq!(in_comments.hits.len(), 1, "a comment's words find its issue");
    let quoted = b.search("\"kumquat\" OR NEAR(", &[Domain::Eu], &[eu_priv]);
    assert!(quoted.hits.is_empty(), "query text is never FTS5 syntax");

    // Kill the EU shard that holds the public EU repository: its site is lost with it.
    let killed = *b.shards.keys().find(|k| k.domain == Domain::Eu && k.shard == search::shard_of(eu_pub)).unwrap();
    let dead_site = b.shards[&killed].site;
    b.shards.remove(&killed);
    b.lost.insert(killed);
    b.lost_sites.push(dead_site);
    let partial = b.search("kumquat", &[Domain::Eu], &[eu_priv]);
    let lost_docs: BTreeSet<_> = made
        .iter()
        .filter(|m| search::shard_of(m.0) == killed.shard && b.domain_of(b.repos[&m.0].1.owner) == Domain::Eu)
        .map(|m| (m.0, m.1))
        .collect();
    assert!(!lost_docs.is_empty());
    assert!(partial.incomplete, "a partial result sets incomplete_results");
    assert_eq!(
        got(&partial),
        want(&made, &[eu_priv]).difference(&lost_docs).copied().collect(),
        "the other shards answer"
    );

    // Writes during the outage: the consumer pauses the lost shard's chains, and its cursor holds.
    let n = number(b.issue(eu_pub, ALICE, open_issue("kumquat late", "after the kill")));
    made.push((eu_pub, n, "kumquat late".into()));
    for _ in 0..5 {
        b.pass(1_000);
        b.run_notify_and_search();
    }
    assert!(b.search("kumquat", &[Domain::Eu], &[eu_priv]).incomplete, "incomplete until the rebuild");

    // The rebuild: the next generation on the domain's next site, from the metadata tiers, then the consumer resumes.
    b.lost.remove(&killed);
    let new = b.rebuild_shard(killed);
    b.lost.insert(killed);
    assert_eq!((new.generation, b.shards[&new].jurisdiction), (killed.generation + 1, Some(Jurisdiction::Eu)));
    assert_ne!(b.shards[&new].site, dead_site);
    assert_eq!(sites.get(b.shards[&new].site).unwrap().jurisdiction, Some(Jurisdiction::Eu));
    // A shard object lost on a site that lives on also moves: the next generation is on the domain's next site.
    assert_ne!(new.site(&sites, &[]), killed.site(&sites, &[]), "a rebuild never reuses the lost object's site");
    for _ in 0..3 {
        b.pass(1_000);
        b.run_notify_and_search();
    }
    let after = b.search("kumquat", &[Domain::Eu], &[eu_priv]);
    assert_eq!((got(&after), after.incomplete, after.total), (want(&made, &[eu_priv]), false, made.len() as u64));
    // Default-domain shards still hold no EU document.
    for (k, sh) in &b.shards {
        let eu_docs: i64 = sh.db.query_row("SELECT count(*) FROM doc WHERE owner = ?", [EU_ORG], |r| r.get(0)).unwrap();
        assert!(k.domain == Domain::Eu || eu_docs == 0);
    }
    println!(
        "BF-68: {} documents, EU ones only in eu shards; with {} killed: incomplete_results and {} of {}; after the rebuild on site {:?}: {} of {}, complete",
        made.len() - 1,
        killed.do_name(),
        partial.hits.len(),
        made.len() - 1,
        b.shards[&new].site,
        after.hits.len(),
        made.len()
    );
}

/// Notify's rules, on one comment: the actor is never notified, an ignored repository notifies nobody, an explicit
/// thread unsubscription stops a watcher but not a mention, and the author hears of a comment. Then
/// `ConsumerCursorCmd` moves only forward, and only for `Notify` and `Search`.
#[test]
fn notify_skips_the_actor_ignorers_and_unsubscribers_but_not_a_mention_and_cursors_only_move_forward() {
    let mut b = Bed::new();
    b.add_repo(REPO, OWNER, true);
    let n = number(b.issue(REPO, BOB, open_issue("rules", "who hears of it")));
    let (ignorer, unsubscribed, mentioned) = (acct(401), acct(402), acct(403));
    b.logins.insert("erin".into(), mentioned);
    b.logins.insert("ivan".into(), ignorer);
    let w = |a: AccountId, ignored: bool| watch::WatcherUpdate::Watcher {
        account: a,
        row: watch::WatcherRow { starred_at: None, watching: true, ignored, src: graftyard_types::Index(1) },
    };
    let off = |a: AccountId| watch::WatcherUpdate::ThreadSub {
        n: IssueNumber(n),
        account: a,
        row: watch::ThreadSubRow { subscribed: false, src: graftyard_types::Index(1) },
    };
    let updates = vec![
        w(acct(ALICE), false),
        w(acct(BOB), false),
        w(ignorer, true),
        w(unsubscribed, false),
        off(unsubscribed),
        w(mentioned, false),
        off(mentioned),
    ];
    let rid = b.rid();
    b.repo_cmd(REPO, RepoCmd::Counts(CountsCmd { updates, request: rid }));
    // An older batch that still had the unsubscribed watcher subscribed to the thread arrives late: it changes nothing.
    let stale = watch::WatcherUpdate::ThreadSub {
        n: IssueNumber(n),
        account: unsubscribed,
        row: watch::ThreadSubRow { subscribed: true, src: graftyard_types::Index(0) },
    };
    let rid = b.rid();
    let late = b.repo_cmd(REPO, RepoCmd::Counts(CountsCmd { updates: vec![stale], request: rid }));
    assert_eq!(late, RepoOut::Counts(watch::CountsOut { applied: 0 }), "an older thread subscription is refused");
    let from = {
        let (g, _) = &b.repos[&REPO];
        g.members[g.leader().0 as usize].group_fold().applied()
    };
    // Ivan ignores the repository, so even a mention does not reach him [inference: GitHub's "Ignore" stops all
    // notifications from the repository].
    b.issue(REPO, ALICE, comment(n, "cc @erin, @ivan, and @nobody"));
    let f = b.leader_fold(REPO);
    let (g, _) = &b.repos[&REPO];
    let fl = g.leader();
    let logins = b.logins.clone();
    let login = |l: &str| logins.get(l).copied();
    let got: BTreeMap<AccountId, inbox::Reason> = committed(g, fl)
        .iter()
        .filter(|e| e.index > from)
        .flat_map(|e| inbox::notices(REPO, &f.meta, e, &login))
        .map(|(a, nt)| (a, nt.reason))
        .collect();
    let want: BTreeMap<AccountId, inbox::Reason> =
        [(acct(BOB), inbox::Reason::Author), (mentioned, inbox::Reason::Mention)].into_iter().collect();
    assert_eq!(got, want, "Bob as author, Erin by mention; not Alice, the ignorer, or the unsubscribed watcher");

    // The cursor: forward commits, backward and a consumer that commits its own do not.
    let cur = |b: &mut Bed| tiers::consumer_cursor(&*b.leader_fold(REPO).core, Consumer::Notify);
    let mv = |b: &mut Bed, consumer, through: u64| {
        let rid = b.rid();
        let c = ConsumerCursorCmd { consumer, through: graftyard_types::Index(through), request: rid };
        b.repo_cmd(REPO, RepoCmd::ConsumerCursor(c))
    };
    assert_eq!(mv(&mut b, Consumer::Notify, 5), RepoOut::Counts(watch::CountsOut { applied: 1 }));
    assert_eq!(mv(&mut b, Consumer::Notify, 3), RepoOut::Counts(watch::CountsOut { applied: 0 }), "never back");
    assert_eq!(cur(&mut b), graftyard_types::Index(5));
    assert_eq!(
        mv(&mut b, Consumer::PrSync, 9),
        RepoOut::Counts(watch::CountsOut { applied: 0 }),
        "PrSync commits its own"
    );
}

/// The Inbox keeps each thread's newest notice whatever order chains deliver in, and a redelivered or older entry of
/// a chain changes nothing, not even `sources` or `participating`.
#[test]
fn an_inbox_keeps_the_newest_notice_and_ignores_an_entry_at_or_below_its_sources() {
    let thread = ThreadId { repo: REPO, number: 1 };
    let notice = |reason, at: u64, index: u64| Notice {
        thread,
        reason,
        at: Timestamp(at),
        index: graftyard_types::Index(index),
        title: "t".into(),
        kind: graftyard_git::issues::IssueKind::Issue,
        latest_comment: None,
    };
    let (a, b) = (ChainId::Repo(REPO), ChainId::Repo(RepoId(DbId(8))));
    let mut i = Inbox::default();
    assert!(i.apply(a, graftyard_types::Index(9), &[notice(inbox::Reason::Subscribed, 900, 9)]));
    assert!(
        !i.apply(a, graftyard_types::Index(4), &[notice(inbox::Reason::Mention, 400, 4)]),
        "an older entry of the chain"
    );
    assert!(!i.apply(a, graftyard_types::Index(9), &[notice(inbox::Reason::Mention, 900, 9)]), "a redelivery");
    i.apply(b, graftyard_types::Index(2), &[notice(inbox::Reason::Comment, 500, 2)]);
    let r = &i.threads[&thread];
    assert_eq!((r.reason, r.updated), (inbox::Reason::Subscribed, Timestamp(900)), "the newest notice wins");
    assert!(r.participating, "an older participating notice from another chain still counts");
    assert_eq!(i.sources[&a], graftyard_types::Index(9), "sources never moves back");
}

/// A REST reply for `crates/api/gh/validate.py`, written when `$GRAFTYARD_REPLIES` is set.
fn keep_rest(case: &str, path: &str, status: u16, body: serde_json::Value) {
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        let v = serde_json::json!({ "method": "GET", "path": path, "status": status, "body": body });
        std::fs::write(format!("{dir}/{case}.rest.json"), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }
}

/// `rest:activity` and `GET /search/issues` rendered from a live bed, for the schema check: both `/stargazers` media
/// types, `/subscribers`, the repository subscription, the notification threads, a thread subscription, and the search
/// reply. Also: the thread id round-trips, and `/stargazers` without the star media type is a `simple-user` list.
#[test]
fn activity_and_search_replies_render_ghes_objects() {
    use graftyard_api::activity;
    let mut b = Bed::new();
    b.add_repo(REPO, OWNER, true);
    b.identity(ALICE);
    b.id_cmd(BOB, IdentityOp::SetRecord(UserRecord::Watch { repo: REPO, subscribed: true, ignored: false }));
    b.id_cmd(ALICE, IdentityOp::SetRecord(UserRecord::Star { repo: REPO, on: true }));
    until(&mut b, 60_000, &mut |b| !watch::subscribers(&b.follower_fold(REPO).meta).is_empty()).unwrap();
    let n = number(b.issue(REPO, ALICE, open_issue("kumquat thread", "for @bob")));
    b.issue(REPO, ALICE, comment(n, "a comment"));
    until(&mut b, 60_000, &mut |b| b.inboxes.get(&BOB).is_some_and(|i| !i.threads.is_empty())).unwrap();
    until(&mut b, 60_000, &mut |b| b.shards.values().map(Shard::count).sum::<i64>() == 1).unwrap();
    let thread = ThreadId { repo: REPO, number: n };
    b.id_cmd(BOB, IdentityOp::SetRecord(UserRecord::Thread { thread, read: true, done: false }));
    b.id_cmd(BOB, IdentityOp::SetRecord(UserRecord::Subscription { thread, subscribed: true }));

    let f = b.leader_fold(REPO);
    let host = graftyard_api::wire::Host::new("ghes.graftyard.test", false).unwrap();
    let dir = Dir;
    let r = graftyard_api::view::Repo { host: &host, at: dir.home(), core: &f.core, meta: &f.meta, dir: &dir };
    let stars = activity::stargazers(&r, "", "application/vnd.github+json");
    assert!(stars.value()[0]["login"].is_string() && stars.value()[0].get("starred_at").is_none(), "simple-user items");
    keep_rest("activity-stargazers", "/repos/{owner}/{repo}/stargazers", stars.status, stars.value().clone());
    let starred = activity::stargazers(&r, "", STAR);
    assert!(starred.value()[0]["starred_at"].is_string() && starred.value()[0]["user"]["login"].is_string());
    keep_rest("activity-stargazers-star", "/repos/{owner}/{repo}/stargazers", starred.status, starred.value().clone());
    let subs = activity::subscribers(&r, "");
    keep_rest("activity-subscribers", "/repos/{owner}/{repo}/subscribers", subs.status, subs.value().clone());
    let user = b.user(BOB);
    let sub = activity::repo_subscription(&r, &user);
    assert_eq!(sub.status, 200);
    keep_rest("activity-subscription", "/repos/{owner}/{repo}/subscription", sub.status, sub.value().clone());
    assert_eq!(activity::repo_subscription(&r, &b.user(ALICE)).status, 404, "a viewer who never watched");

    let q = activity::NotificationQuery { all: true, ..Default::default() };
    let (page, _) = activity::notification_page(&b.inboxes[&BOB], &user, &q, None, "");
    let threads: Vec<serde_json::Value> = page.iter().map(|l| activity::thread_json(&r, l)).collect();
    assert_eq!(threads.len(), 1);
    // The reason is the newest notice's, as GitHub's is: the comment reached Bob as a watcher after the mention.
    assert_eq!(
        (threads[0]["unread"].clone(), threads[0]["reason"].clone()),
        (serde_json::json!(false), serde_json::json!("subscribed"))
    );
    let (mine, _) = activity::notification_page(
        &b.inboxes[&BOB],
        &user,
        &activity::NotificationQuery { all: true, participating: true, ..Default::default() },
        None,
        "",
    );
    assert_eq!(mine.len(), 1, "the earlier mention keeps the thread participating");
    let id = threads[0]["id"].as_str().unwrap();
    assert_eq!(activity::thread_id(id), Some(thread), "the thread id round-trips");
    keep_rest("activity-notifications", "/notifications", 200, serde_json::Value::Array(threads));
    let ts = activity::thread_subscription(&host.api(), thread, &user);
    assert_eq!(ts["subscribed"], serde_json::json!(true));
    keep_rest("activity-thread-subscription", "/notifications/threads/{thread_id}/subscription", 200, ts);

    let m = b.search("kumquat", &[], &[]);
    let items: Vec<serde_json::Value> =
        m.hits.iter().filter_map(|h| activity::search_item(&r, IssueNumber(h.number), h.score)).collect();
    assert_eq!(items.len(), 1);
    let reply = activity::search_reply(&m, items);
    keep_rest("search-issues", "/search/issues", reply.status, reply.value().clone());
    let bad = activity::search_unsupported("language:rust");
    keep_rest("search-issues-422", "/search/issues", bad.status, bad.value().clone());
}

/// An install with no `eu` site (`graft-m4a`): EU shards bind on the `weur` and `eeur` sites, each generation on the
/// next, while one that has `eu` sites keeps them there; and a shard's name parses back to itself.
#[test]
fn eu_shards_bind_on_eu_regions_when_no_site_is_eu_and_names_round_trip() {
    let site = |id: u8, region, jurisdiction| Site {
        id: SiteId(id),
        script: format!("s{id}"),
        account: "a".into(),
        region,
        jurisdiction,
        bucket: format!("b{id}"),
        release: None,
        link: SiteLink::Binding { member_class: "Member".into() },
        spare: false,
    };
    let m4a = SiteTable(vec![site(0, Region::Enam, None), site(1, Region::Weur, None), site(2, Region::Apac, None)]);
    let k = |generation| SearchShard { domain: Domain::Eu, shard: 3, generation };
    assert_eq!((k(1).site(&m4a, &[]), k(2).site(&m4a, &[])), (Some(SiteId(1)), Some(SiteId(1))));
    assert_eq!(k(1).site(&m4a, &[SiteId(1)]), None, "no EU-region site left");
    let d = SearchShard { domain: Domain::Default, shard: 3, generation: 2 };
    assert_eq!(d.site(&m4a, &[]), Some(SiteId(1)), "Default still takes any site");
    let with_eu = Bed::sites();
    assert_eq!(k(1).site(&with_eu, &[]), Some(SiteId(3)), "an install with eu sites uses only those");
    for s in [k(1), k(7), d, SearchShard { domain: Domain::Apac, shard: 15, generation: 40 }] {
        assert_eq!(SearchShard::parse(&s.do_name()), Some(s));
    }
    assert_eq!(SearchShard::parse("search#eu#3"), None);
    assert_eq!(SearchShard::parse("search#mars#3#1"), None);
}
