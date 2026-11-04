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
    Dispatcher, MergeAnswer, PrSync, REDELIVER_AFTER_MS, REPO_FORMAT_V3, SyncBatch, SyncRead, TestMerge, commands, scan,
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
        let (m, ready) = Member::restore(config(s), &self.disks[s.0 as usize], self.now, self.boots).unwrap();
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

    fn merge(&mut self, n: IssueNumber, base: ObjectId, head: ObjectId) -> TestMerge {
        self.runs += 1;
        let a = if self.conflicts.contains(&head) {
            MergeAnswer::Conflict { conflict: vec!["f".into()] }
        } else {
            MergeAnswer::Clean { merge: Self::merge_commit(base, head).hex(), pack: vec![0x50; self.pack_bytes] }
        };
        a.test_merge(n, base, head).unwrap()
    }
}

/// The site's halves: the leader's dispatcher, in memory for one leadership, and a queue whose messages the test
/// delivers, drops, or delivers again.
struct Pipe {
    leading: Option<(SiteId, graftyard_raft::Term)>,
    disp: Dispatcher,
    queue: Vec<SyncBatch>,
    cache: Cache,
}

impl Pipe {
    fn new() -> Pipe {
        Pipe {
            leading: None,
            disp: Dispatcher::default(),
            queue: Vec::new(),
            cache: Cache { conflicts: BTreeSet::new(), pack_bytes: 200, runs: 0 },
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
            scan(&mut self.disp, &fold.core, REPO, cursor, applied, |i| m.entry_at(i).cloned()).expect("in memory");
        }
        if let Some(b) = self.disp.batch(cursor, g.now) {
            self.queue.push(b);
        }
    }

    /// The consumer Worker on one batch: fresh views, the missing test merges, the batch's commands.
    fn plan(&mut self, g: &G, b: &SyncBatch) -> Vec<PrSync> {
        // Through the member route's JSON, as the consumer Worker reads it.
        let wire = serde_json::to_value(SyncRead::of(&g.fold().core, REPO, &b.prs)).unwrap();
        let vs = serde_json::from_value::<SyncRead>(wire).unwrap().views;
        let merges =
            vs.iter().filter_map(|v| v.needs_merge().map(|(base, head)| self.cache.merge(v.n, base, head))).collect();
        commands(b, &vs, merges)
    }

    fn consume(&mut self, g: &mut G, b: &SyncBatch) {
        for c in self.plan(g, b) {
            g.commit(c);
        }
    }

    /// Alarms and deliveries until the queue is empty and the dispatcher sends nothing more.
    fn drain(&mut self, g: &mut G) {
        for _ in 0..20 {
            self.alarm(g);
            let Some(b) = self.queue.pop() else { return };
            self.consume(g, &b);
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
