//! `PrSync` on three real `graftyard_raft::Member<RepoCmd>`s: the leader's dispatcher reads the committed log from the
//! group's cursor, a queue carries its batches at least once, and a consumer commits `PrSync` from a fake git cache.
//! Each test fails on one plausible bug: a dispatcher that skips entries after a leader change, a redelivered batch
//! applied twice, a merge ref left on an older head, and a conflict that leaves the merge ref in place.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_git::issues::{
    Consumer, HeadSpec, IssueCapture, IssueChange, IssueNumber, MergeabilityResult, PrCapture, PrState,
};
use graftyard_git::pack::PackRef;
use graftyard_git::prsync::{
    Delivery, Dispatcher, MergeAnswer, PARK_AFTER, PrSync, REDELIVER_AFTER_MS, REPO_FORMAT_V3, SyncBatch, SyncRead,
    TestMerge, commands, scan, test_merges,
};
use graftyard_git::refs::{ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{REPO_FORMAT_V1, RepoCmd, RepoEvent, RepoFold, RepoOut};
use graftyard_git::tiers::{consumer_cursor, get_rec, head_ref, merge_ref, mergeability, pull_head, ref_key};
use graftyard_raft::{
    EntryBody, Envelope, Ev, Index, Member, MemberConfig, ReleaseTag, RequestId, Resolution, Role, Stored, Ulid,
};
use graftyard_types::{
    AccountId, ChainId, DbId, PlacementName, PlacementRecord, Region, ReleaseId, RepoId, SiteId, Timestamp,
};

const SITES: [SiteId; 3] = [SiteId(0), SiteId(1), SiteId(2)];
const REPO: RepoId = RepoId(DbId(7));
const OWNER: AccountId = AccountId(DbId(100));

fn name(s: &str) -> RefName {
    RefName::parse(s.as_bytes()).unwrap()
}

fn oid(n: u32) -> ObjectId {
    let mut o = [0u8; 20];
    o[..4].copy_from_slice(&n.to_be_bytes());
    o[19] = 1;
    ObjectId(o)
}

/// Three members of `repo:7`, driven by hand; every message is delivered unless dropped by a crash.
struct G {
    now: Timestamp,
    members: Vec<Option<Member<RepoCmd>>>,
    disks: Vec<Stored<RepoFold>>,
    inbox: Vec<Envelope<Ev<RepoCmd>>>,
    resolutions: Vec<Resolution<RepoCmd>>,
    boots: u64,
    next: u128,
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
        reads: vec![REPO_FORMAT_V1, REPO_FORMAT_V3],
    }
}

impl G {
    fn new() -> G {
        let mut g = G {
            now: Timestamp(10_000),
            members: Vec::new(),
            disks: SITES.iter().map(|_| Stored::new(RepoFold::new(REPO, OWNER))).collect(),
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
        let (r, mut out) = f(m, now).persist(&mut self.disks[s.0 as usize]);
        loop {
            self.inbox.extend(out.early.drain(..).chain(out.messages.drain(..)));
            self.resolutions.append(&mut out.resolutions);
            let Some(ack) = out.self_ack.take() else { break };
            let m = self.members[s.0 as usize].as_mut().expect("member is up");
            out = m.self_ack(now, ack).persist(&mut self.disks[s.0 as usize]).1;
        }
        r
    }

    fn crash(&mut self, s: SiteId) {
        self.members[s.0 as usize] = None;
        self.inbox.retain(|e| e.to != s && e.from != s);
    }

    /// A member restored from what it stored, as a reset Durable Object is.
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

    /// Time passes in 1 s ticks, so the leader keeps its followers.
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

    /// `s` takes over from a crashed leader once its promises lapse.
    fn fail_over(&mut self, from: SiteId, to: SiteId) {
        self.crash(from);
        self.pass(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1_000);
        self.elect(to);
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

    /// Events the leader records as one entry, as a push or an issue command would commit them.
    fn record(&mut self, events: Vec<RepoEvent>) {
        let (l, rid) = (self.leader(), self.rid());
        self.on(l, |m, now| m.record(now, rid, REPO_FORMAT_V1, events)).expect("recorded");
        self.pass(2_000);
        let ok = self.resolutions.iter().any(|r| matches!(r, Resolution::Recorded { id, result: Ok(_) } if *id == rid));
        assert!(ok, "the entry committed");
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
        let base_sha = self.get(&name("refs/heads/main")).unwrap();
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
            base: name("refs/heads/main"),
            capture,
        };
        self.record(vec![ev]);
    }

    /// The consumer's commit, re-sent under its request id after a lost leader as a site's `commit_as` does.
    fn commit(&mut self, c: PrSync) -> RepoOut {
        for _ in 0..5 {
            let l = self.leader();
            let a = self.on(l, |m, now| m.commit(now, c.request, RepoCmd::Sync(c.clone()))).expect("admitted");
            self.pass(2_000);
            let r = self.resolutions.iter().rev().find_map(|r| match r {
                Resolution::Ticket { ticket, result } if *ticket == a.ticket => Some(result.clone()),
                _ => None,
            });
            if let Some(Ok(a)) = r {
                return a.out;
            }
        }
        panic!("the sync never committed");
    }

    /// Every committed entry's events, in log order, from the leader.
    fn committed(&self) -> Vec<RepoEvent> {
        let m = self.m(self.leader());
        (1..=m.state().commit.0)
            .filter_map(|i| m.entry_at(Index(i)))
            .filter_map(|e| match &e.body {
                EntryBody::Events(evs) => Some(evs.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }
}

/// The git cache's test merge: clean unless the head is in `conflicts`; a clean merge's commit is a function of the
/// pair, as the container's deterministic `commit-tree` is.
struct Cache {
    conflicts: BTreeSet<ObjectId>,
    pack_bytes: usize,
    runs: usize,
}

impl Cache {
    fn merge_commit(base: ObjectId, head: ObjectId) -> ObjectId {
        ObjectId(std::array::from_fn(|i| base.0[i].wrapping_mul(31) ^ head.0[i].wrapping_add(7) ^ 0x5a))
    }

    fn answer(&mut self, base: ObjectId, head: ObjectId) -> MergeAnswer {
        self.runs += 1;
        if self.conflicts.contains(&head) {
            MergeAnswer::Conflict { conflict: vec!["f".into()] }
        } else {
            MergeAnswer::Clean { merge: Self::merge_commit(base, head).hex(), pack: vec![0x50; self.pack_bytes] }
        }
    }
}

/// The site's halves: the leader's dispatcher, in memory for one leadership, and a queue whose messages the test
/// delivers, drops, or delivers again.
struct Pipe {
    leading: Option<(SiteId, graftyard_raft::Term)>,
    disp: Dispatcher,
    queue: Vec<SyncBatch>,
    /// How each batch went out, in order: the first attempt in band, or a re-send through the queue.
    sent: Vec<Delivery>,
    cache: Cache,
    /// TEST_MERGE_BUDGET as the shard applies it, when set (`budgeted`), with the shard's clock, which each merge
    /// advances by the given ms.
    budget: Option<(std::cell::RefCell<graftyard_git::prsync::Budgets>, std::cell::Cell<u64>, u64)>,
}

impl Pipe {
    fn new() -> Pipe {
        Pipe {
            leading: None,
            disp: Dispatcher::default(),
            queue: Vec::new(),
            sent: Vec::new(),
            cache: Cache { conflicts: BTreeSet::new(), pack_bytes: 200, runs: 0 },
            budget: None,
        }
    }

    /// The leader's alarm: a new leadership starts a new dispatcher, which reads from the committed cursor.
    fn alarm(&mut self, g: &G) {
        let l = g.leader();
        let me = (l, g.m(l).term());
        if self.leading != Some(me) {
            self.leading = Some(me);
            self.disp = Dispatcher::default();
        }
        let m = g.m(l);
        let fold = &m.group_fold().fold;
        let cursor = consumer_cursor(&*fold.core, Consumer::PrSync);
        let applied = m.state().applied;
        for _ in 0..4 {
            scan(&mut self.disp, &fold.core, REPO, cursor, applied, |i| m.entry_at(i).map(|e| (e.clone(), 0)))
                .expect("in memory");
        }
        if let Some((b, how)) = self.disp.batch(cursor, g.now) {
            self.sent.push(how);
            self.queue.push(b);
        }
    }

    /// The consumer Worker on one batch: fresh views, the missing test merges, the batch's commands.
    fn plan(&mut self, g: &G, b: &SyncBatch) -> Vec<PrSync> {
        // Through the member route's JSON, as the consumer Worker reads it.
        let wire = serde_json::to_value(SyncRead::of(&g.fold().core, REPO, &b.prs)).unwrap();
        let vs = serde_json::from_value::<SyncRead>(wire).unwrap().views;
        // The site's merge loop (`test_merges`), with no deadline: these tests do not time the attempt.
        let got = futures_executor::block_on(test_merges(
            &vs,
            || 0,
            u64::MAX,
            |n, base, head, _| {
                let a = self.answer(g, n, base, head);
                async move { Ok(a) }
            },
        ))
        .unwrap();
        commands(b, &vs, got.merges, got.shed)
    }

    /// The shard's `/merge` answer, through JSON as the consumer reads it: the cache's merge, under the budget when
    /// one is set.
    fn answer(&mut self, g: &G, n: IssueNumber, base: ObjectId, head: ObjectId) -> MergeAnswer {
        let Some((budgets, clock, cost)) = &self.budget else { return self.cache.answer(base, head) };
        clock.set(clock.get().max(g.now.0));
        let cache = &mut self.cache;
        // Every future here is ready at once, and `plan` already runs inside an executor.
        let a = futures_util::FutureExt::now_or_never(graftyard_git::prsync::budgeted(
            budgets,
            graftyard_types::NetworkId(REPO.0),
            graftyard_git::prsync::MergeCause::HeadPush,
            n,
            || clock.get(),
            || async { Ok::<(), ()>(()) },
            || async {
                clock.set(clock.get() + cost);
                // No CPU reported, so the shard charges the merge's wall time on its clock.
                Ok((cache.answer(base, head), None))
            },
        ))
        .expect("ready")
        .unwrap()
        .unwrap_or(MergeAnswer::Shed { shed: true });
        serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap()
    }

    fn consume(&mut self, g: &mut G, b: &SyncBatch) {
        for c in self.plan(g, b) {
            g.commit(c);
        }
    }

    /// Alarms and deliveries until the queue is empty and the dispatcher sends nothing more. Each delivery returns
    /// to the dispatcher, as the leader's in-band attempt does.
    fn drain(&mut self, g: &mut G) {
        for _ in 0..20 {
            self.alarm(g);
            let Some(b) = self.queue.pop() else { return };
            self.consume(g, &b);
            self.disp.finished(true);
        }
        panic!("the pipeline never went quiet");
    }
}

/// What BF-24 checks on every open pull request once the pipeline is quiet: `refs/pull/N/head` at its head, and the
/// test merge of its current (base tip, head) recorded, with `refs/pull/N/merge` at the merge commit or deleted on a
/// conflict.
fn assert_synced(g: &G, cache: &Cache, prs: &[u32]) {
    let core = &*g.fold().core;
    for n in prs.iter().map(|n| IssueNumber(*n)) {
        let ph = pull_head(core, n).unwrap();
        let base = g.get(&ph.base).unwrap();
        assert_eq!(g.get(&head_ref(n)), Some(ph.head_sha), "#{} head ref", n.0);
        let m = mergeability(core, n).unwrap_or_else(|| panic!("#{} has no test merge", n.0));
        assert_eq!((m.base, m.head), (base, ph.head_sha), "#{} test merge is of an older pair", n.0);
        let want =
            if cache.conflicts.contains(&ph.head_sha) { None } else { Some(Cache::merge_commit(base, ph.head_sha)) };
        assert_eq!(g.get(&merge_ref(n)), want, "#{} merge ref", n.0);
        match m.result {
            MergeabilityResult::Clean { merge } => assert_eq!(Some(merge), want),
            MergeabilityResult::Conflict { .. } => assert_eq!(want, None),
        }
        let mergeable = graftyard_git::prsync::mergeable(Some(&m), ph.head_sha, Some(base));
        assert_eq!(mergeable, Some(want.is_some()), "#{} mergeable", n.0);
    }
}

/// Each test merge, head move, and cursor position is committed at most once.
fn assert_once(g: &G) {
    let mut merges = BTreeMap::new();
    let mut heads = BTreeMap::new();
    let mut cursor = Index(0);
    for e in g.committed() {
        match e {
            RepoEvent::TestMerged { n, base, head, .. } => *merges.entry((n, base, head)).or_insert(0) += 1,
            RepoEvent::Refs { updates, .. } => {
                for u in updates.iter().filter(|u| u.name.as_bytes().starts_with(b"refs/pull/")) {
                    *heads.entry((u.name.clone(), u.new)).or_insert(0) += 1;
                }
            }
            RepoEvent::ConsumerCursor { through, .. } => {
                assert!(through > cursor, "the cursor moved from {cursor:?} to {through:?}");
                cursor = through;
            }
            _ => {}
        }
    }
    assert!(merges.values().all(|c| *c == 1), "a test merge committed twice: {merges:?}");
    assert!(heads.values().all(|c| *c == 1), "a pull ref move committed twice: {heads:?}");
}

fn setup() -> (G, Pipe) {
    let mut g = G::new();
    g.push("refs/heads/main", oid(1));
    g.push("refs/heads/f", oid(2));
    g.open(1, "refs/heads/f");
    (g, Pipe::new())
}

#[test]
fn a_head_push_moves_both_pull_refs_and_a_conflict_deletes_the_merge_ref() {
    let (mut g, mut p) = setup();
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    let first = g.get(&merge_ref(IssueNumber(1))).unwrap();
    // A head push: the merge ref moves to the new pair's merge, never staying on the old head's.
    g.push("refs/heads/f", oid(3));
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    assert_ne!(g.get(&merge_ref(IssueNumber(1))), Some(first));
    // A conflicting push: the ref is deleted and mergeable is false.
    p.cache.conflicts.insert(oid(4));
    g.push("refs/heads/f", oid(4));
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    assert_eq!(g.get(&merge_ref(IssueNumber(1))), None);
    // Clean again after the conflict.
    g.push("refs/heads/f", oid(5));
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    assert_once(&g);
    assert_eq!(p.cache.runs, 4, "one test merge per head");
}

#[test]
fn a_base_move_leaves_mergeable_unknown_until_the_next_head_push() {
    let (mut g, mut p) = setup();
    p.drain(&mut g);
    g.push("refs/heads/main", oid(9));
    p.drain(&mut g);
    let core = &*g.fold().core;
    let m = mergeability(core, IssueNumber(1)).unwrap();
    assert_eq!(graftyard_git::prsync::mergeable(Some(&m), oid(2), Some(oid(9))), None, "stale, not wrong");
    g.push("refs/heads/f", oid(3));
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
}

#[test]
fn a_merge_too_large_to_record_deletes_the_older_heads_merge_ref() {
    let (mut g, mut p) = setup();
    p.drain(&mut g);
    assert!(g.get(&merge_ref(IssueNumber(1))).is_some());
    p.cache.pack_bytes = graftyard_git::prsync::MERGE_BYTES_PER_COMMAND + 1;
    g.push("refs/heads/f", oid(3));
    p.drain(&mut g);
    assert_eq!(g.get(&head_ref(IssueNumber(1))), Some(oid(3)));
    assert_eq!(g.get(&merge_ref(IssueNumber(1))), None, "no merge ref of the old head");
    let m = mergeability(&*g.fold().core, IssueNumber(1)).unwrap();
    assert_eq!(graftyard_git::prsync::mergeable(Some(&m), oid(3), Some(oid(1))), None, "mergeable is null");
}

#[test]
fn a_new_leader_reads_from_the_committed_cursor_and_skips_nothing() {
    let (mut g, mut p) = setup();
    p.drain(&mut g);
    let before = consumer_cursor(&*g.fold().core, Consumer::PrSync);
    assert!(before > Index(0));
    // A head push whose batch is lost with its consumer, then the leader is lost before it sends it again.
    g.push("refs/heads/f", oid(3));
    p.alarm(&g);
    assert_eq!(p.queue.len(), 1);
    p.queue.clear();
    g.fail_over(SITES[0], SITES[1]);
    assert_eq!(consumer_cursor(&*g.fold().core, Consumer::PrSync), before, "the cursor is the group's");
    // More work under the new leader, then the old one comes back from its storage.
    g.open(2, "refs/heads/f");
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1, 2]);
    g.restart(SITES[0]);
    g.pass(3_000);
    let after = consumer_cursor(&*g.fold().core, Consumer::PrSync);
    assert!(after > before);
    let restored = &g.m(SITES[0]).group_fold().fold;
    assert_eq!(consumer_cursor(&*restored.core, Consumer::PrSync), after, "a restored member holds the cursor");
    // And leads with it: a push under the restored member's leadership is synced too.
    g.fail_over(SITES[1], SITES[0]);
    g.restart(SITES[1]);
    g.push("refs/heads/f", oid(4));
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1, 2]);
    assert_once(&g);
}

#[test]
fn a_redelivered_batch_applies_once() {
    let (mut g, mut p) = setup();
    p.alarm(&g);
    let first = p.queue.pop().unwrap();
    // Its consumer is slow: the batch is overdue, so the leader sends it again, over the entries since. A second
    // consumer plans the resent batch before the first commits, so both hold the same head move and test merge
    // under different request ids.
    let planned = p.plan(&g, &first);
    g.push("refs/heads/other", oid(7));
    g.pass(REDELIVER_AFTER_MS);
    p.alarm(&g);
    let again = p.queue.pop().expect("resent once overdue");
    assert_eq!(again.from, first.from);
    assert!(again.through > first.through);
    let replanned = p.plan(&g, &again);
    assert_eq!(p.cache.runs, 2, "both consumers ran the test merge");
    for c in planned {
        g.commit(c);
    }
    for c in replanned {
        g.commit(c);
    }
    // Then Queues delivers the first message again, identical, and its consumer plans and commits it once more.
    p.consume(&mut g, &first);
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    assert_once(&g);
    assert_eq!(consumer_cursor(&*g.fold().core, Consumer::PrSync), again.through);
}

#[test]
fn a_late_command_of_an_older_head_moves_nothing_back() {
    let (mut g, mut p) = setup();
    p.alarm(&g);
    let old = p.queue.pop().unwrap();
    let stale = p.plan(&g, &old);
    // Its attempt returns without committing (its isolate is slow), so the next push goes out at once.
    p.disp.finished(true);
    g.push("refs/heads/f", oid(3));
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    let at = consumer_cursor(&*g.fold().core, Consumer::PrSync);
    for c in stale {
        match g.commit(c) {
            RepoOut::Sync(o) => assert_eq!(o.events, 0, "a batch the cursor passed commits nothing"),
            other => panic!("{other:?}"),
        }
    }
    assert_synced(&g, &p.cache, &[1]);
    assert_eq!(consumer_cursor(&*g.fold().core, Consumer::PrSync), at);
    assert_once(&g);
}

#[test]
fn a_consumer_overtaken_by_a_head_push_records_nothing_of_the_older_head() {
    let (mut g, mut p) = setup();
    p.alarm(&g);
    let b = p.queue.pop().unwrap();
    let planned = p.plan(&g, &b);
    // The head moves after the consumer read its views and before it commits. Its batch is still ahead of the
    // cursor, so only the fold's current head keeps its test merge out.
    g.push("refs/heads/f", oid(3));
    for c in planned {
        g.commit(c);
    }
    p.disp.finished(true);
    let n = IssueNumber(1);
    assert_eq!(g.get(&merge_ref(n)), None, "no merge ref of the older head");
    assert_eq!(g.get(&head_ref(n)), None, "no head ref at the older head");
    assert!(mergeability(&*g.fold().core, n).is_none());
    assert_eq!(consumer_cursor(&*g.fold().core, Consumer::PrSync), b.through);
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    assert_once(&g);
}

#[test]
fn a_reopened_pull_request_is_synced_at_its_branch_tip() {
    let (mut g, mut p) = setup();
    p.drain(&mut g);
    g.record(vec![RepoEvent::PrClosed { n: IssueNumber(1), final_head: oid(2) }]);
    g.push("refs/heads/f", oid(3));
    p.drain(&mut g);
    assert_eq!(g.get(&head_ref(IssueNumber(1))), Some(oid(2)), "a closed pull request's refs stay");
    g.record(vec![RepoEvent::IssueChanged {
        n: IssueNumber(1),
        change: IssueChange::State { open: true, reason: None },
        actor: None,
    }]);
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    assert_eq!(g.get(&head_ref(IssueNumber(1))), Some(oid(3)));
}

#[test]
fn the_member_wire_form_reads_a_push_and_a_sync() {
    let mut c = PrSync {
        from: Index(3),
        through: Some(Index(9)),
        heads: vec![(IssueNumber(1), oid(2))],
        merges: vec![TestMerge {
            n: IssueNumber(1),
            base: oid(1),
            head: oid(2),
            result: MergeabilityResult::Clean { merge: oid(5) },
            pack: Some(PackRef::Inline(b"PACK".to_vec())),
        }],
        unmerged: Vec::new(),
        request: RequestId(Ulid(0)),
    };
    c.request = graftyard_git::prsync::request_id(&c);
    let mut again = c.clone();
    again.request = RequestId(Ulid(7));
    assert_eq!(graftyard_git::prsync::request_id(&again), c.request, "the id is the content's, not the field's");
    let v = serde_json::to_value(RepoCmd::Sync(c.clone())).unwrap();
    assert!(v.get("prSync").is_some());
    assert_eq!(serde_json::from_value::<RepoCmd>(v).unwrap(), RepoCmd::Sync(c.clone()));
    let push = graftyard_git::repo::Push {
        updates: vec![RefUpdate { name: name("refs/heads/main"), expect: None, new: Some(oid(1)) }],
        atomicity: graftyard_git::refs::Atomicity::PerRef,
        pack: None,
        actor: graftyard_identity::Principal {
            actor: graftyard_identity::Actor::Anonymous,
            scopes: graftyard_identity::ScopeSet(0),
            credential: graftyard_identity::Credential::Stored([1; 32]),
            sso_orgs: Default::default(),
        },
        options: Vec::new(),
        request: RequestId(Ulid(1)),
        agent: None,
        provenance: Vec::new(),
        protected: Vec::new(),
        changed: Vec::new(),
        leases: false,
    };
    let v = serde_json::to_value(RepoCmd::Push(push.clone())).unwrap();
    assert!(v.is_string(), "a push keeps its base64 wire form");
    assert_eq!(serde_json::from_value::<RepoCmd>(v).unwrap(), RepoCmd::Push(push));
    let mut later = c.clone();
    later.through = Some(Index(10));
    assert_ne!(graftyard_git::prsync::request_id(&later), c.request, "another plan, another id");
}

#[test]
fn a_batch_split_over_two_commands_moves_the_cursor_when_its_retry_is_planned_again() {
    let (mut g, mut p) = setup();
    g.push("refs/heads/g", oid(20));
    g.open(2, "refs/heads/g");
    p.cache.pack_bytes = graftyard_git::prsync::MERGE_BYTES_PER_COMMAND / 2 + 1;
    p.alarm(&g);
    let b = p.queue.pop().unwrap();
    let cmds = p.plan(&g, &b);
    assert_eq!(cmds.len(), 2, "two merges of over half the bound each take two commands");
    // The first command commits and the consumer dies; Queues retries, and the retry plans again on the new fold.
    g.commit(cmds[0].clone());
    p.consume(&mut g, &b);
    assert_eq!(consumer_cursor(&*g.fold().core, Consumer::PrSync), b.through, "the retry moves the cursor");
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1, 2]);
    assert_once(&g);
}

#[test]
fn a_first_attempt_runs_in_band_and_only_a_re_send_goes_to_the_queue() {
    let (mut g, mut p) = setup();
    p.drain(&mut g);
    assert!(p.sent.iter().all(|d| *d == Delivery::First), "every batch of a quiet pipeline is a first attempt");
    // A head push whose first attempt is lost: nothing more goes out until the work is overdue, then a re-send.
    p.sent.clear();
    g.push("refs/heads/f", oid(3));
    p.alarm(&g);
    p.queue.clear();
    p.alarm(&g);
    g.pass(REDELIVER_AFTER_MS - 1_000);
    p.alarm(&g);
    assert_eq!(p.sent, [Delivery::First], "no re-send before REDELIVER_AFTER_MS");
    g.pass(1_000);
    p.alarm(&g);
    assert_eq!(p.sent, [Delivery::First, Delivery::Again]);
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    assert_once(&g);
}

#[test]
fn a_batch_that_fails_every_time_is_parked_after_park_after_attempts() {
    let (mut g, mut p) = setup();
    p.drain(&mut g);
    p.sent.clear();
    g.push("refs/heads/f", oid(3));
    for _ in 0..30 {
        p.alarm(&g);
        if !p.queue.is_empty() {
            p.queue.clear();
            p.disp.finished(false);
        }
        g.pass(REDELIVER_AFTER_MS);
    }
    assert_eq!(p.sent.len(), PARK_AFTER as usize);
    assert_eq!(p.sent.first(), Some(&Delivery::First));
    assert!(p.sent[1..].iter().all(|d| *d == Delivery::Again));
    let cursor = consumer_cursor(&*g.fold().core, Consumer::PrSync);
    assert!(p.disp.parked(cursor));
    assert!(!p.disp.waiting(cursor));

    // More work does not unpark the chain: only the committed cursor moving does.
    g.push("refs/heads/f", oid(4));
    for _ in 0..3 {
        p.alarm(&g);
        assert!(p.queue.is_empty());
        g.pass(REDELIVER_AFTER_MS);
    }
    assert_eq!(p.sent.len(), PARK_AFTER as usize);

    // A new leadership restores the member's park instead of spending another ten attempts.
    let mut restored = Dispatcher::default();
    restored.park(cursor);
    p.disp = restored;
    p.alarm(&g);
    assert!(p.queue.is_empty());
    assert_eq!(p.sent.len(), PARK_AFTER as usize);

    // The operator replays the parked batch; its commit moves the cursor and unparks the chain.
    let through = g.m(g.leader()).state().applied;
    let replay = SyncBatch { from: cursor, through, prs: vec![IssueNumber(1)] };
    for c in p.plan(&g, &replay) {
        g.commit(c);
    }
    assert!(consumer_cursor(&*g.fold().core, Consumer::PrSync) > cursor);
    g.push("refs/heads/f", oid(5));
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    assert_once(&g);
}

#[test]
fn a_healthy_pipeline_never_parks_however_many_batches_it_sends() {
    let (mut g, mut p) = setup();
    for i in 0..3 * PARK_AFTER {
        g.push("refs/heads/f", oid(10 + i));
        p.drain(&mut g);
    }
    assert!(p.sent.len() > PARK_AFTER as usize);
    assert_synced(&g, &p.cache, &[1]);
    assert_once(&g);
}

#[test]
fn a_first_attempt_that_never_returns_is_re_sent_while_pushes_keep_coming() {
    let (mut g, mut p) = setup();
    p.drain(&mut g);
    p.sent.clear();
    // The first attempt never returns, and a push lands every 10 s: they wait behind it, and once the work handed
    // out first is REDELIVER_AFTER_MS old, all of it goes out again through the queue.
    for i in 0..3 {
        g.push("refs/heads/f", oid(10 + i));
        p.alarm(&g);
        p.queue.clear();
        g.pass(10_000);
    }
    p.alarm(&g);
    assert_eq!(p.sent, [Delivery::First, Delivery::Again]);
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    assert_once(&g);
}

#[test]
fn first_attempts_that_return_without_passing_the_cursor_do_not_hold_back_the_re_send() {
    let (mut g, mut p) = setup();
    p.drain(&mut g);
    p.sent.clear();
    // Each first attempt reports back, but its commits never land, and a push lands every 10 s: each push goes out
    // as a new first attempt, and the work the cursor has not passed is still re-sent once it is REDELIVER_AFTER_MS
    // old. The dispatcher trusts the cursor, not the attempt's report.
    for i in 0..3 {
        g.push("refs/heads/f", oid(10 + i));
        p.alarm(&g);
        p.queue.clear();
        p.disp.finished(true);
        g.pass(10_000);
    }
    p.alarm(&g);
    assert_eq!(p.sent, [Delivery::First, Delivery::First, Delivery::First, Delivery::Again]);
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    assert_once(&g);
}

#[test]
fn a_first_attempt_lost_with_its_isolate_does_not_hold_later_pushes() {
    let (mut g, mut p) = setup();
    p.drain(&mut g);
    p.sent.clear();
    // A head push's first attempt dies with its isolate and never returns. The re-send covers it.
    g.push("refs/heads/f", oid(3));
    p.alarm(&g);
    p.queue.clear();
    g.pass(REDELIVER_AFTER_MS);
    p.alarm(&g);
    let again = p.queue.pop().expect("re-sent");
    p.consume(&mut g, &again);
    assert_eq!(p.sent, [Delivery::First, Delivery::Again]);
    // The next push must still go out, though the lost attempt never reported back.
    g.push("refs/heads/f", oid(4));
    p.alarm(&g);
    assert_eq!(p.sent.len(), 3, "a later push waited forever behind the lost attempt");
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    assert_once(&g);
}

/// TEST_MERGE_BUDGET: past the budget a head push's test merge is not computed, its old merge ref is deleted, and
/// `mergeable` reads null (GraphQL `UNKNOWN`), as for a merge too large to record. Within it the merge runs. The shard
/// charges each merge's measured time: on a `standard-2` (0.25 vCPU of budget) a 10 s merge leaves the bucket about
/// 7 s short when it ends, so the next push is shed, though its 50 ms estimate alone would have let it run.
#[test]
fn a_head_push_past_the_test_merge_budget_leaves_mergeable_null() {
    let (mut g, mut p) = setup();
    let budgets = graftyard_git::prsync::Budgets::new(graftyard_git::prsync::instance_vcpu("standard-2").unwrap());
    p.budget = Some((std::cell::RefCell::new(budgets), std::cell::Cell::new(g.now.0), 10_000));
    p.drain(&mut g);
    assert_synced(&g, &p.cache, &[1]);
    let runs = p.cache.runs;
    g.push("refs/heads/f", oid(3));
    p.drain(&mut g);
    let core = &*g.fold().core;
    let m = mergeability(core, IssueNumber(1));
    assert_eq!(graftyard_git::prsync::mergeable(m.as_ref(), oid(3), Some(oid(1))), None, "null past the budget");
    assert_eq!(g.get(&merge_ref(IssueNumber(1))), None, "the older head's merge ref is gone");
    assert_eq!(p.cache.runs, runs, "no merge computed past the budget");
    assert_eq!(g.get(&head_ref(IssueNumber(1))), Some(oid(3)), "the head ref still moves");
    let st = p.budget.as_ref().unwrap().0.borrow().stats(graftyard_types::NetworkId(REPO.0)).unwrap();
    assert_eq!((st.computed, st.shed, st.spent_ms), (1, 1, 10_000.0), "one charged at its measured time, one shed");
}

/// LM-1 round 16d: a first attempt must end inside FIRST_FOR_MS with its cursor moved, however long the shard's line.
/// On B0-29 each attempt asked its merges one after another behind the network's turn, whose line on B0-35 reached
/// 3,593 merges and 265 s of wait with nothing shed, so no attempt of 100 or more merges ended in 25 s, the leader
/// handed out a new one every 25 s while the old ones ran, and 14 failed together past 128 MB.
///
/// Here 30 merges from other callers are in line at t = 0, each 1 s of wall on the shard (20 ms of CPU, so the budget
/// itself sheds nothing), and a 40-PR batch's attempt starts with them. Its merges must stop by COMMIT_RESERVE_MS before
/// FIRST_FOR_MS, every pull request must come out merged or shed, and no merge may run after waiting past
/// TURN_WAIT_MS. Main's loop, which asked every merge and waited out every turn, ended its merges at 41 s here
/// (`tests/firstonce/plant.py`, mutants 0 and 1 together).
#[test]
fn a_batch_behind_a_long_merge_line_ends_inside_first_for_and_sheds_the_rest() {
    use futures_util::task::LocalSpawnExt;
    use graftyard_git::prsync::{
        Budgets, COMMIT_RESERVE_MS, FIRST_FOR_MS, MergeCause, TURN_WAIT_MS, budgeted, instance_vcpu, merges_until,
    };
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    #[derive(Default)]
    struct Clock {
        now: Cell<u64>,
        waiting: RefCell<Vec<(u64, std::task::Waker)>>,
    }
    struct Sleep(Rc<Clock>, u64);
    impl std::future::Future for Sleep {
        type Output = ();
        fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
            if self.0.now.get() >= self.1 {
                return std::task::Poll::Ready(());
            }
            self.0.waiting.borrow_mut().push((self.1, cx.waker().clone()));
            std::task::Poll::Pending
        }
    }
    const WALL: u64 = 1_000;
    assert_eq!(merges_until(0, FIRST_FOR_MS), FIRST_FOR_MS - COMMIT_RESERVE_MS);
    assert_eq!(merges_until(1_000, 4_000), 2_500, "a short test-install deadline keeps half for its commits");
    let net = graftyard_types::NetworkId(REPO.0);
    let budgets = Rc::new(RefCell::new(Budgets::new(instance_vcpu("standard-4").unwrap())));
    let clock = Rc::new(Clock::default());
    let ran = Rc::new(Cell::new(0u32));
    // One merge on the shard: in line for the network's turn, then WALL of wall time.
    let shard = {
        let (budgets, clock, ran) = (budgets.clone(), clock.clone(), ran.clone());
        move |n: u32| {
            let (budgets, clock, ran) = (budgets.clone(), clock.clone(), ran.clone());
            async move {
                let c = clock.clone();
                budgeted(
                    &budgets,
                    net,
                    MergeCause::HeadPush,
                    IssueNumber(n),
                    || c.now.get(),
                    || async { Ok::<(), String>(()) },
                    || async {
                        ran.set(ran.get() + 1);
                        Sleep(clock.clone(), clock.now.get() + WALL).await;
                        Ok((MergeAnswer::Conflict { conflict: vec!["f".into()] }, Some(20.0)))
                    },
                )
                .await
                .map(|a| a.unwrap_or(MergeAnswer::Shed { shed: true }))
            }
        }
    };
    let mut pool = futures_executor::LocalPool::new();
    for i in 0..30 {
        let m = shard(1_000 + i);
        pool.spawner().spawn_local(async move { m.await.map(drop).unwrap() }).unwrap();
    }
    // The batch's views: 40 open PRs, each lacking its merge.
    let views: Vec<graftyard_git::prsync::PrView> = (1..=40)
        .map(|n| graftyard_git::prsync::PrView {
            n: IssueNumber(n),
            open: true,
            same_repo: true,
            head: oid(100 + n),
            base: name("refs/heads/main"),
            base_tip: Some(oid(1)),
            head_ref: None,
            merge: None,
        })
        .collect();
    let out = Rc::new(RefCell::new(None));
    let (o, c, views2, shard2) = (out.clone(), clock.clone(), views.clone(), shard.clone());
    let until = merges_until(0, FIRST_FOR_MS);
    pool.spawner()
        .spawn_local(async move {
            let now = || c.now.get();
            let got = test_merges(&views2, now, until, |n, _, _, left| {
                // The site's `within`: an answer not back by the time merges stop counts as shed.
                let (m, stop) = (shard2(n.0), Sleep(c.clone(), c.now.get() + left));
                async move {
                    match futures_util::future::select(Box::pin(m), Box::pin(stop)).await {
                        futures_util::future::Either::Left((a, _)) => a,
                        futures_util::future::Either::Right(_) => Ok(MergeAnswer::Shed { shed: true }),
                    }
                }
            })
            .await
            .unwrap();
            *o.borrow_mut() = Some((got, c.now.get()));
        })
        .unwrap();
    loop {
        pool.run_until_stalled();
        let next = clock.waiting.borrow().iter().map(|(t, _)| *t).min();
        let Some(next) = next else { break };
        clock.now.set(next);
        let due: Vec<_> = clock.waiting.borrow_mut().drain(..).collect();
        due.into_iter().for_each(|(_, w)| w.wake());
    }
    let (got, back) = out.borrow_mut().take().expect("the batch's merges ended");
    assert!(back <= FIRST_FOR_MS - COMMIT_RESERVE_MS, "the batch's merges ended at {back} ms, past their deadline");
    assert_eq!(got.merges.len() + got.shed.len(), views.len(), "every pull request is merged or shed");
    assert_eq!(got.asked + got.unasked, 40);
    assert!(got.unasked >= 25, "15 s of 1 s merges cannot ask 40: {} unasked", got.unasked);
    let s = budgets.borrow().stats(net).unwrap();
    assert_eq!(s.computed, ran.get());
    assert_eq!(s.computed + s.shed, 30 + got.asked, "every merge that reached the shard ran or was shed");
    assert!(s.late > 0, "the line's tail waited past TURN_WAIT_MS and was shed");
    assert!(s.wait_max_ms <= TURN_WAIT_MS as f32, "a merge ran after waiting {} ms", s.wait_max_ms);
    // The batch's commands still move its heads and the cursor, and name every shed head, so `mergeable` reads null.
    let batch = SyncBatch { from: Index(5), through: Index(9), prs: views.iter().map(|v| v.n).collect() };
    let shed = got.shed.len();
    let cmds = commands(&batch, &views, got.merges, got.shed);
    assert_eq!(cmds.last().unwrap().through, Some(Index(9)));
    assert_eq!((cmds[0].heads.len(), cmds[0].unmerged.len()), (40, shed));
}

/// A30: an on-demand merge (a single-PR GET's or Actions dispatch's, `on_demand`) records the PR's current test merge
/// without moving the consumer's cursor, so the batches go on as before. The same merge asked twice commits once, a
/// merge of a head the PR has since left records nothing, and a shed answer commits nothing.
#[test]
fn an_on_demand_merge_records_the_current_head_and_moves_no_cursor() {
    use graftyard_git::prsync::on_demand;
    let (mut g, _p) = setup();
    let cursor = consumer_cursor(&*g.fold().core, Consumer::PrSync);
    let clean = |base: ObjectId, head: ObjectId| MergeAnswer::Clean {
        merge: Cache::merge_commit(base, head).hex(),
        pack: vec![0x50; 8],
    };
    assert_eq!(on_demand(IssueNumber(1), oid(1), oid(2), MergeAnswer::Shed { shed: true }), None, "shed: no command");
    let c = on_demand(IssueNumber(1), oid(1), oid(2), clean(oid(1), oid(2))).unwrap();
    assert_eq!(c, on_demand(IssueNumber(1), oid(1), oid(2), clean(oid(1), oid(2))).unwrap(), "one request id");
    assert!(matches!(g.commit(c.clone()), RepoOut::Sync(o) if o.events == 1));
    let core = &*g.fold().core;
    assert_eq!(
        graftyard_git::prsync::mergeable(mergeability(core, IssueNumber(1)).as_ref(), oid(2), Some(oid(1))),
        Some(true)
    );
    assert_eq!(consumer_cursor(core, Consumer::PrSync), cursor, "no cursor moved");
    g.commit(c);
    let merged = |g: &G| g.committed().iter().filter(|e| matches!(e, RepoEvent::TestMerged { .. })).count();
    assert_eq!(merged(&g), 1, "asked twice, recorded once");
    // The head moved before the GET's merge committed: nothing of the old head is recorded.
    g.push("refs/heads/f", oid(3));
    let stale =
        on_demand(IssueNumber(1), oid(1), oid(2), MergeAnswer::Conflict { conflict: vec!["f".into()] }).unwrap();
    assert!(matches!(g.commit(stale), RepoOut::Sync(o) if o.events == 0));
    assert_eq!(merged(&g), 1);
    let m = mergeability(&*g.fold().core, IssueNumber(1)).unwrap();
    assert_eq!(
        (m.head, m.result.clone()),
        (oid(2), MergeabilityResult::Clean { merge: Cache::merge_commit(oid(1), oid(2)) })
    );
    let big =
        MergeAnswer::Clean { merge: oid(7).hex(), pack: vec![0; graftyard_git::prsync::MERGE_BYTES_PER_COMMAND + 1] };
    assert_eq!(on_demand(IssueNumber(1), oid(1), oid(3), big), None, "a pack past the command bound commits nothing");
}

/// merge-budget-4: a first attempt that outlives its deadline and its re-send both merge the same head, the re-send at
/// the base's new tip. When the slower, older-base merge commits second it must not replace the current one: that put
/// `mergeable` back to null and the merge ref on the older base 32 times in merge-budget-3's run 1. A merge of a new
/// head, or of a head whose recorded merge is itself stale, still records as before.
#[test]
fn an_older_base_merge_committed_late_does_not_replace_the_current_one() {
    use graftyard_git::prsync::on_demand;
    let (mut g, _p) = setup();
    let n = IssueNumber(1);
    let clean = |base: ObjectId, head: ObjectId| MergeAnswer::Clean {
        merge: Cache::merge_commit(base, head).hex(),
        pack: vec![0x50; 8],
    };
    let commit = |g: &mut G, base: u32, head: u32| match g
        .commit(on_demand(n, oid(base), oid(head), clean(oid(base), oid(head))).unwrap())
    {
        RepoOut::Sync(o) => o.events,
        other => panic!("{other:?}"),
    };
    let pair = |g: &G| mergeability(&*g.fold().core, n).map(|m| (m.base, m.head));
    // The base moves from 1 to 9 while the first attempt still merges at 1; the re-send's merge at 9 commits first.
    g.push("refs/heads/main", oid(9));
    assert_eq!(commit(&mut g, 9, 2), 1);
    assert_eq!(pair(&g), Some((oid(9), oid(2))));
    // The abandoned first attempt's merge at the old tip arrives last: nothing changes.
    assert_eq!(commit(&mut g, 1, 2), 0, "an older base's merge is not recorded over the tip's");
    assert_eq!(pair(&g), Some((oid(9), oid(2))));
    assert_eq!(g.get(&merge_ref(n)), Some(Cache::merge_commit(oid(9), oid(2))));
    let m = mergeability(&*g.fold().core, n);
    assert_eq!(graftyard_git::prsync::mergeable(m.as_ref(), oid(2), Some(oid(9))), Some(true));
    // A new head with a merge planned before the base moved again still records, stale, as before.
    g.push("refs/heads/f", oid(3));
    g.push("refs/heads/main", oid(10));
    assert_eq!(commit(&mut g, 9, 3), 1, "the new head's first merge records even at an older tip");
    assert_eq!(pair(&g), Some((oid(9), oid(3))));
    // And the tip's merge then replaces it.
    assert_eq!(commit(&mut g, 10, 3), 1);
    assert_eq!(pair(&g), Some((oid(10), oid(3))));
}

/// A new leader of a hot group reads entries from the stored log, each row with its inline packs, from a committed
/// cursor far behind. One alarm reads at most `SCAN_BYTES` of rows and says more are left; the alarms after it read
/// the rest in order, so the batch still names every pull request and the whole read is never one alarm's work. On
/// graft-m3c an unbounded read of 362 MB took 30 to 34 s of a leader's alarm CPU, and every leader reset (hot-shards).
#[test]
fn a_leader_far_behind_reads_the_stored_log_a_bounded_slice_per_alarm() {
    use graftyard_git::prsync::SCAN_BYTES;
    let (mut g, _) = setup();
    for i in 0..40 {
        g.push("refs/heads/f", oid(10 + i));
    }
    g.open(2, "refs/heads/f");
    let l = g.leader();
    let m = g.m(l);
    let fold = &m.group_fold().fold;
    let applied = m.state().applied;
    // Every entry above 2 is a stored row of a 900 KB push; entries 1 and 2 are in memory.
    let row = 900_000usize;
    let mut d = Dispatcher::default();
    let (mut alarms, mut read) = (0, Vec::new());
    loop {
        alarms += 1;
        let mut bytes = 0;
        let more = scan(&mut d, &fold.core, REPO, Index(0), applied, |i| {
            let e = m.entry_at(i).cloned()?;
            let n = if i.0 > 2 { row } else { 0 };
            bytes += n;
            read.push(i.0);
            Some((e, n))
        })
        .expect("in memory");
        assert!(bytes <= SCAN_BYTES + row, "alarm {alarms} read {bytes} bytes of rows");
        if !more {
            break;
        }
        assert!(alarms < 100, "no progress");
    }
    let rows = applied.0.saturating_sub(2) as usize;
    assert!(alarms >= rows * row / (SCAN_BYTES + row), "{alarms} alarms for {rows} rows");
    assert!(alarms > 1, "the read was cut at least once");
    assert_eq!(read, (1..=applied.0).collect::<Vec<_>>(), "each entry read once, in order");
    let (b, _) = d.batch(Index(0), g.now).expect("the pushes and openings touched pull requests");
    assert_eq!(b.through, applied);
    assert_eq!(b.prs, vec![IssueNumber(1), IssueNumber(2)]);
}
