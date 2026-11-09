//! BF-57 and BF-58 (`build/BUILD-PLAN.md`, M4-WP1) in memory: each repository is three real
//! `graftyard_raft::Member<RepoCmd>`s, the `Webhooks` consumer reads each leader's committed log from the chain's
//! `HookCursor` (`graftyard_git::sequencer::plan`, `consumer_start`) and records rows in the owner's `Sequencer`, and
//! delivery Workers run its attempts against simulated hooks. Time is simulated in 100 ms steps.
//!
//! - BF-57: one hook that times out at 10 s beside 49 healthy hooks of the same owner, 10 events/s. Pass: healthy
//!   first attempts' p99 at most 2 s after the entry committed, every slow-hook delivery started in chain order, and
//!   the cursor's `read` at most 5 s behind each entry, not growing with the slow hook's backlog.
//! - BF-58: the site holding the owner's `Sequencer` (and every repository leader) is killed with deliveries pending.
//!   The store is rebuilt empty, leaders fail over, and the consumer replays from the committed `replay_from`. Pass:
//!   every (event, hook) reaches its hook at least once, and every GUID sent is the one the event and hook derive.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_git::hooks::{
    ATTEMPT_DEADLINE_MS, Guid, HOOK_OWNER_CONCURRENCY, HookContentType, HookCursor, HookCursorCmd, HookId, HookRecord,
    REPO_FORMAT_HOOKS, delivery_guid, hook_cursor, hooks,
};
use graftyard_git::refs::{ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{REPO_FORMAT_V1, RepoCmd, RepoEvent, RepoFold};
use graftyard_git::sequencer::{Sequencer, Start, consumer_start, cursor_to_commit, plan};
use graftyard_identity::{EventKind, Url};
use graftyard_raft::{
    Envelope, Ev, Index, Member, MemberConfig, ReleaseTag, RequestId, Resolution, Role, Stored, Ulid,
};
use graftyard_types::{
    AccountId, ChainId, DbId, PlacementName, PlacementRecord, Region, ReleaseId, RepoId, SiteId, Timestamp,
};
use smallvec::smallvec;

const SITES: [SiteId; 3] = [SiteId(0), SiteId(1), SiteId(2)];
const OWNER: AccountId = AccountId(DbId(100));
/// The install's restore generation, which every GUID includes.
const GEN: u64 = 1;
const STEP: u64 = 100;
/// How often the consumer may commit a chain's cursor [guess: one commit round per second per chain].
const CURSOR_EVERY_MS: u64 = 1_000;

fn name(s: &str) -> RefName {
    RefName::parse(s.as_bytes()).unwrap()
}

fn oid(n: u32) -> ObjectId {
    let mut o = [0u8; 20];
    o[..4].copy_from_slice(&n.to_be_bytes());
    o[19] = 1;
    ObjectId(o)
}

fn config(chain: ChainId, me: SiteId) -> MemberConfig {
    MemberConfig {
        chain,
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
        release: ReleaseTag { id: ReleaseId([4; 32]), seq: 1 },
        reads: vec![REPO_FORMAT_V1, REPO_FORMAT_HOOKS],
    }
}

/// One repository's three members, driven by hand; a crashed member's messages are dropped.
struct G {
    repo: RepoId,
    members: Vec<Option<Member<RepoCmd>>>,
    disks: Vec<Stored<RepoFold>>,
    inbox: Vec<Envelope<Ev<RepoCmd>>>,
    resolutions: Vec<Resolution<RepoCmd>>,
    boots: u64,
    main: Option<ObjectId>,
    /// When each entry committed, as the leader saw it.
    committed_at: BTreeMap<Index, Timestamp>,
    /// When the committed `HookCursor.read` first passed each entry.
    covered_at: BTreeMap<Index, Timestamp>,
    hooks: Vec<HookId>,
    /// The entry that created the hooks, whose pings it carries.
    hook_entry: Index,
}

impl G {
    fn new(repo: RepoId, now: Timestamp) -> G {
        let mut g = G {
            repo,
            members: Vec::new(),
            disks: SITES.iter().map(|_| Stored::new(RepoFold::new(repo, OWNER))).collect(),
            inbox: Vec::new(),
            resolutions: Vec::new(),
            boots: 0,
            main: None,
            committed_at: BTreeMap::new(),
            covered_at: BTreeMap::new(),
            hooks: Vec::new(),
            hook_entry: Index(0),
        };
        for s in SITES {
            g.members.push(None);
            g.restart(s, now);
        }
        g.elect(SITES[0], now);
        g
    }

    fn chain(&self) -> ChainId {
        ChainId::Repo(self.repo)
    }

    fn m(&self, s: SiteId) -> &Member<RepoCmd> {
        self.members[s.0 as usize].as_ref().expect("member is up")
    }

    fn on<R>(
        &mut self,
        s: SiteId,
        now: Timestamp,
        f: impl FnOnce(&mut Member<RepoCmd>, Timestamp) -> graftyard_raft::Ready<RepoCmd, R>,
    ) -> R {
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

    fn restart(&mut self, s: SiteId, now: Timestamp) {
        self.boots += 1;
        let disk = self.disks[s.0 as usize].clone();
        let (m, ready) = Member::restore(config(self.chain(), s), disk, now, self.boots).unwrap();
        self.members[s.0 as usize] = Some(m);
        let ((), out) = ready.persist(&mut self.disks[s.0 as usize]);
        self.inbox.extend(out.early.into_iter().chain(out.messages));
    }

    fn crash(&mut self, s: SiteId) {
        self.members[s.0 as usize] = None;
        self.inbox.retain(|e| e.to != s && e.from != s);
    }

    fn deliver(&mut self, now: Timestamp) {
        for _ in 0..10_000 {
            if self.inbox.is_empty() {
                self.stamp(now);
                return;
            }
            for e in std::mem::take(&mut self.inbox) {
                if self.members[e.to.0 as usize].is_none() {
                    continue;
                }
                let Envelope { from, to, msg } = e;
                self.on(to, now, |m, t| m.step(t, from, msg));
            }
        }
        panic!("messages never settled");
    }

    fn tick(&mut self, now: Timestamp) {
        for s in SITES {
            if self.members[s.0 as usize].is_some() {
                self.on(s, now, |m, t| m.tick(t));
            }
        }
        self.deliver(now);
    }

    fn elect(&mut self, s: SiteId, now: Timestamp) {
        for _ in 0..3 {
            self.on(s, now, |m, t| m.campaign(t));
            self.deliver(now);
            if self.m(s).role() == Role::Leader {
                return;
            }
        }
        panic!("{s:?} did not win");
    }

    fn leader(&self) -> Option<SiteId> {
        SITES
            .iter()
            .filter(|s| self.members[s.0 as usize].as_ref().is_some_and(|m| m.role() == Role::Leader))
            .max_by_key(|s| self.m(**s).term())
            .copied()
    }

    /// Stamps newly committed entries, and entries the committed cursor newly passed, with `now`.
    fn stamp(&mut self, now: Timestamp) {
        let Some(l) = self.leader() else { return };
        let m = self.m(l);
        let commit = m.state().commit;
        let from = self.committed_at.keys().next_back().map_or(0, |i| i.0);
        let read = hook_cursor(&*m.group_fold().fold.core).read;
        for i in from + 1..=commit.0 {
            self.committed_at.insert(Index(i), now);
        }
        let from = self.covered_at.keys().next_back().map_or(0, |i| i.0);
        for i in from + 1..=read.0 {
            self.covered_at.insert(Index(i), now);
        }
    }

    /// One entry on the leader, as a push or an issue command commits it; its index.
    fn record(&mut self, rid: RequestId, now: Timestamp, format: graftyard_raft::Format, evs: Vec<RepoEvent>) -> Index {
        let l = self.leader().expect("a leader");
        self.on(l, now, |m, t| m.record(t, rid, format, evs)).expect("recorded");
        self.deliver(now);
        assert!(self.resolutions.iter().any(|r| matches!(r, Resolution::Recorded { id, result: Ok(_) } if *id == rid)));
        self.m(l).state().commit
    }

    fn push(&mut self, rid: RequestId, now: Timestamp, n: u32) -> Index {
        let ev = RepoEvent::Refs {
            push: rid,
            actor: None,
            updates: vec![RefUpdate { name: name("refs/heads/main"), expect: self.main, new: Some(oid(n)) }],
            pack: None,
        };
        self.main = Some(oid(n));
        self.record(rid, now, REPO_FORMAT_V1, vec![ev])
    }

    /// The consumer's cursor commit, as `HookCursorCmd` through the leader.
    fn commit_cursor(&mut self, rid: RequestId, now: Timestamp, cursor: HookCursor) {
        let Some(l) = self.leader() else { return };
        let cmd = RepoCmd::HookCursor(HookCursorCmd { cursor, request: rid });
        if self.on(l, now, |m, t| m.commit(t, rid, cmd)).is_ok() {
            self.deliver(now);
        }
    }

    fn committed_cursor(&self) -> Option<HookCursor> {
        Some(hook_cursor(&*self.m(self.leader()?).group_fold().fold.core))
    }
}

fn hook(retry: bool) -> HookRecord {
    HookRecord {
        url: Url("https://hooks.example/h".into()),
        events: smallvec![EventKind("*".into())],
        content_type: HookContentType::Json,
        active: true,
        insecure_ssl: false,
        secret: None,
        retry,
    }
}

/// How a simulated hook answers: in `ms`, with a 2xx, or no answer before the deadline.
#[derive(Clone, Copy)]
enum Behaves {
    Healthy,
    TimesOut,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// The owner's sequencer, its consumer, and the delivery Workers.
struct World {
    now: Timestamp,
    groups: Vec<G>,
    seq: Sequencer,
    behaves: BTreeMap<HookId, Behaves>,
    rng: Rng,
    next: u128,
    /// Attempts in flight: (when it ends, the attempt). Lost with their site in BF-58.
    flying: Vec<(Timestamp, Start)>,
    /// Every attempt a delivery Worker sent: (GUID, hook, when, whether it was answered 2xx).
    sent: Vec<(Guid, HookId, Timestamp, bool)>,
    first_start: BTreeMap<Guid, Timestamp>,
    /// Per (chain, hook): the (index, slot) of each delivery's first attempt, in start order.
    order: BTreeMap<(ChainId, HookId), Vec<(Index, u16)>>,
    last_cursor: BTreeMap<RepoId, Timestamp>,
    /// Chains with rows recorded since their last cursor commit.
    dirty: BTreeSet<RepoId>,
    max_in_flight: (usize, usize),
    /// When false, the owner's home site is down and nothing runs the sequencer.
    up: bool,
}

impl World {
    fn new(hooks_per_repo: &[usize]) -> World {
        let now = Timestamp(10_000);
        let mut w = World {
            now,
            groups: Vec::new(),
            seq: Sequencer::new(OWNER, now),
            behaves: BTreeMap::new(),
            rng: Rng(0x9e37_79b9_7f4a_7c15),
            next: 1,
            flying: Vec::new(),
            sent: Vec::new(),
            first_start: BTreeMap::new(),
            order: BTreeMap::new(),
            last_cursor: BTreeMap::new(),
            dirty: BTreeSet::new(),
            max_in_flight: (0, 0),
            up: true,
        };
        for (k, n) in hooks_per_repo.iter().enumerate() {
            let repo = RepoId(DbId(7 + k as i64));
            let mut g = G::new(repo, now);
            let rid = w.rid();
            // A millisecond before the hooks: no hook gets it.
            g.push(rid, Timestamp(now.0 - 1), 1);
            let ids: Vec<HookId> = (0..*n).map(|j| HookId(DbId(1_000 * (k as i64 + 1) + j as i64))).collect();
            let evs = ids
                .iter()
                .map(|id| RepoEvent::Hook { id: *id, record: Some(hook(false)), actor: Some(OWNER), created: true })
                .collect();
            let rid = w.rid();
            g.hook_entry = g.record(rid, now, REPO_FORMAT_HOOKS, evs);
            for id in &ids {
                w.behaves.insert(*id, Behaves::Healthy);
            }
            g.hooks = ids;
            w.groups.push(g);
        }
        w
    }

    fn rid(&mut self) -> RequestId {
        self.next += 1;
        RequestId(Ulid(self.next << 64))
    }

    /// The consumer on each chain whose leader is up: read from `consumer_start`, record, and commit the cursor.
    fn consume(&mut self) {
        if !self.up {
            return;
        }
        let now = self.now;
        for k in 0..self.groups.len() {
            let g = &self.groups[k];
            let Some(l) = g.leader() else { continue };
            let chain = g.chain();
            let m = g.m(l);
            let core = &m.group_fold().fold.core;
            let cursor = hook_cursor(&**core);
            let applied = m.state().applied;
            let start = consumer_start(cursor, self.seq.recorded(chain));
            self.seq.set_targets(&hooks(&**core));
            let mut rows = Vec::new();
            for i in start.0 + 1..=applied.0 {
                let e = m.entry_at(Index(i)).expect("the entry is in the member's window");
                rows.extend(plan(core, g.repo, GEN, e, |w| {
                    Some(format!("{{\"event\":\"{}\",\"index\":{i}}}", w.event).into_bytes())
                }));
            }
            if !rows.is_empty() {
                self.dirty.insert(g.repo);
            }
            if applied > start || self.seq.recorded(chain).is_none() {
                self.seq.record(chain, applied, rows, now);
            }
            let want = self.seq.cursor(chain);
            let Some(c) = g.committed_cursor() else { continue };
            let due = self.last_cursor.get(&g.repo).is_none_or(|t| now.0 >= t.0 + CURSOR_EVERY_MS);
            if let Some(next) = cursor_to_commit(c, want, self.dirty.contains(&g.repo)).filter(|_| due) {
                let repo = g.repo;
                let rid = self.rid();
                self.groups[k].commit_cursor(rid, now, next);
                self.last_cursor.insert(repo, now);
                self.dirty.remove(&repo);
            }
        }
    }

    /// The delivery Workers: start what the sequencer hands out, and report what is due.
    fn deliver_hooks(&mut self) {
        if !self.up {
            return;
        }
        let now = self.now;
        let due: Vec<(Timestamp, Start)> = self.flying.iter().filter(|(t, _)| *t <= now).cloned().collect();
        self.flying.retain(|(t, _)| *t > now);
        for (_, s) in due {
            let b = self.behaves[&s.hook];
            match b {
                Behaves::Healthy => self.seq.report(s.attempt, now, Some(200), false, None),
                Behaves::TimesOut => self.seq.report(s.attempt, now, None, true, None),
            }
            self.sent.push((s.guid, s.hook, now, matches!(b, Behaves::Healthy)));
        }
        for s in self.seq.start(now) {
            let d = self.seq.delivery(&s.guid).expect("held");
            let (chain, index, slot) = d.entry.expect("a chain delivery");
            if !s.redelivery && self.first_start.insert(s.guid, now).is_none() {
                self.order.entry((chain, s.hook)).or_default().push((index, slot));
            }
            let ms = match self.behaves[&s.hook] {
                Behaves::Healthy => 30 + self.rng.next() % 370,
                Behaves::TimesOut => ATTEMPT_DEADLINE_MS,
            };
            self.flying.push((Timestamp(now.0 + ms), s));
        }
        let f = self.seq.in_flight();
        self.max_in_flight = (self.max_in_flight.0.max(f.0), self.max_in_flight.1.max(f.1));
    }

    fn step(&mut self) {
        self.now = Timestamp(self.now.0 + STEP);
        if self.now.0 % 1_000 == 0 {
            let now = self.now;
            for g in &mut self.groups {
                g.tick(now);
            }
        }
        self.consume();
        self.deliver_hooks();
        for g in &mut self.groups {
            let now = self.now;
            g.stamp(now);
        }
    }

    /// The deliveries the committed log names, derived here from what the test committed rather than from `plan`:
    /// each push entry to every hook of its repository, and each hook's ping, slot by slot.
    fn expected(&self, pushes: &[(usize, Index)]) -> BTreeMap<Guid, (ChainId, Index, u16, HookId)> {
        let mut out = BTreeMap::new();
        for g in &self.groups {
            let chain = g.chain();
            for (slot, h) in g.hooks.iter().enumerate() {
                let slot = slot as u16;
                let i = g.hook_entry;
                out.insert(delivery_guid(GEN, chain, i, slot, *h), (chain, i, slot, *h));
            }
        }
        for (k, i) in pushes {
            let g = &self.groups[*k];
            for h in &g.hooks {
                out.insert(delivery_guid(GEN, g.chain(), *i, 0, *h), (g.chain(), *i, 0, *h));
            }
        }
        out
    }

    fn push_round_robin(&mut self, n: u32) -> (usize, Index) {
        let k = n as usize % self.groups.len();
        let rid = self.rid();
        let now = self.now;
        (k, self.groups[k].push(rid, now, 100 + n))
    }
}

fn p(sorted: &[u64], q: f64) -> u64 {
    sorted[((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1]
}

#[test]
fn bf57_one_slow_hook_beside_49_healthy_ones() {
    // 50 hooks of one owner over three repositories (a repository holds at most 20); the slow one is on the first.
    let mut w = World::new(&[20, 20, 10]);
    let slow = w.groups[0].hooks[0];
    w.behaves.insert(slow, Behaves::TimesOut);
    let load_ms = 60_000;
    let mut pushes = Vec::new();
    let mut n = 0;
    let start = w.now;
    while w.now.0 < start.0 + load_ms {
        // 10 events/s: one push entry per 100 ms step, round robin over the repositories.
        n += 1;
        pushes.push(w.push_round_robin(n));
        w.step();
    }
    let load_end = w.now;
    // Drain: the slow hook gets one attempt per 10 s, so its backlog of ~200 takes ~2000 s.
    for _ in 0..30_000 {
        w.step();
        if w.seq.idle() && w.flying.is_empty() {
            break;
        }
    }
    assert!(w.seq.idle(), "the sequencer drained");

    let expected = w.expected(&pushes);
    // Healthy first-attempt latency, from the entry's commit to the attempt's start.
    let mut lat: Vec<u64> = Vec::new();
    for (g, (chain, i, _, h)) in &expected {
        let started = *w.first_start.get(g).unwrap_or_else(|| panic!("{g} to {h:?} never started"));
        if *h != slow {
            let grp = w.groups.iter().find(|x| x.chain() == *chain).unwrap();
            lat.push(started.0 - grp.committed_at[i].0);
        }
    }
    lat.sort_unstable();
    let (p50, p99, max) = (p(&lat, 0.5), p(&lat, 0.99), *lat.last().unwrap());
    // Every slow-hook delivery started, in chain order, once each.
    let slow_order = &w.order[&(w.groups[0].chain(), slow)];
    let slow_expected = expected.values().filter(|(_, _, _, h)| *h == slow).count();
    assert_eq!(slow_order.len(), slow_expected, "the slow hook got every event once");
    assert!(slow_order.windows(2).all(|x| x[0] < x[1]), "the slow hook's deliveries started in chain order");
    for ((c, h), o) in &w.order {
        assert!(o.windows(2).all(|x| x[0] < x[1]), "{c} hook {h:?} out of order");
    }
    // Cursor lag over the load entries: from commit to the committed `read` passing it.
    let mut lag_first = 0u64;
    let mut lag_second = 0u64;
    for (k, i) in &pushes {
        let g = &w.groups[*k];
        let c = g.committed_at[i];
        let lag = g.covered_at.get(i).map_or(u64::MAX, |t| t.0 - c.0);
        if c.0 < start.0 + load_ms / 2 {
            lag_first = lag_first.max(lag);
        } else {
            lag_second = lag_second.max(lag);
        }
    }
    let slow_backlog_at_end = slow_order.len();
    println!(
        "BF-57: {} deliveries to 50 hooks from {} push entries in {} s; healthy first attempt p50 {p50} ms, p99 {p99} \
         ms, max {max} ms; slow hook {} deliveries in chain order (drained {} s after the load); cursor lag max {} ms \
         in the first half, {} ms in the second; in flight at most {} ({} failing)",
        expected.len(),
        pushes.len(),
        load_ms / 1_000,
        slow_backlog_at_end,
        (w.now.0 - load_end.0) / 1_000,
        lag_first,
        lag_second,
        w.max_in_flight.0,
        w.max_in_flight.1
    );
    assert!(p99 <= 2_000, "healthy first-attempt p99 {p99} ms");
    assert!(lag_first.max(lag_second) <= 5_000, "cursor lag {lag_first} / {lag_second} ms");
    assert!(lag_second <= lag_first + STEP * 10, "cursor lag grows with the slow hook");
    assert!(w.max_in_flight.0 <= usize::from(HOOK_OWNER_CONCURRENCY));
    assert!(w.max_in_flight.1 <= usize::from(HOOK_OWNER_CONCURRENCY) / 2);
}

#[test]
fn bf58_the_sequencer_site_is_killed_with_deliveries_pending() {
    // Three repositories, 5 hooks each, one of them slow enough to keep a backlog; every leader is on site 0, which
    // also holds the owner's sequencer.
    let mut w = World::new(&[5, 5, 5]);
    let backlog = w.groups[1].hooks[2];
    w.behaves.insert(backlog, Behaves::TimesOut);
    let mut pushes = Vec::new();
    for n in 1..=60 {
        pushes.push(w.push_round_robin(n));
        w.step();
    }
    let pending = w.seq.len() - w.sent.iter().map(|s| s.0).collect::<BTreeSet<_>>().len();
    let flying = w.flying.len();
    let cursors: Vec<HookCursor> = w.groups.iter().map(|g| g.committed_cursor().unwrap()).collect();
    assert!(pending > 0 && !w.seq.idle(), "deliveries are pending at the kill");
    // Kill site 0: its members, the sequencer, and the attempts its delivery Workers were running.
    for g in &mut w.groups {
        g.crash(SITES[0]);
    }
    let old = std::mem::replace(&mut w.seq, Sequencer::new(OWNER, w.now));
    w.flying.clear();
    w.up = false;
    let killed = w.now;
    // Leaders fail over once their promises lapse; the owner's new home site starts an empty store.
    for _ in 0..((graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1_000) / STEP) {
        w.step();
    }
    for g in &mut w.groups {
        let now = w.now;
        g.elect(SITES[1], now);
    }
    w.seq = Sequencer::new(OWNER, w.now);
    w.up = true;
    assert!(w.seq.recorded(w.groups[0].chain()).is_none(), "the rebuilt store holds nothing");
    let resumed = w.now;
    for n in 61..=90 {
        pushes.push(w.push_round_robin(n));
        w.step();
    }
    for _ in 0..30_000 {
        w.step();
        if w.seq.idle() && w.flying.is_empty() {
            break;
        }
    }
    assert!(w.seq.idle());

    let expected = w.expected(&pushes);
    let sent: BTreeSet<Guid> = w.sent.iter().map(|s| s.0).collect();
    let unknown: Vec<&Guid> = sent.iter().filter(|g| !expected.contains_key(g)).collect();
    assert!(unknown.is_empty(), "attempts under GUIDs no event and hook derive: {unknown:?}");
    let never: Vec<_> = expected.iter().filter(|(g, _)| !sent.contains(g)).map(|(_, v)| v).collect();
    assert!(never.is_empty(), "never delivered: {never:?}");
    // The rebuilt store's rows carry the lost store's GUIDs for every entry both recorded.
    let both: Vec<&Guid> =
        expected.keys().filter(|g| old.delivery(g).is_some() && w.seq.delivery(g).is_some()).collect();
    for g in &both {
        let (a, b) = (old.delivery(g).unwrap(), w.seq.delivery(g).unwrap());
        assert_eq!((a.hook, a.entry), (b.hook, b.entry));
    }
    let mut times: BTreeMap<Guid, usize> = BTreeMap::new();
    for (g, ..) in &w.sent {
        *times.entry(*g).or_default() += 1;
    }
    let again = times.values().filter(|n| **n > 1).count();
    println!(
        "BF-58: {} (event, hook) deliveries; at the kill {} rows held, {} not yet attempted, {} attempts in flight, \
         committed replay_from {:?}; leaders failed over and the store was rebuilt {} s later; {} of {} delivered at \
         least once, {} more than once (replayed under their GUIDs), {} rows recorded by both stores with the same \
         GUIDs, 0 under a new GUID",
        expected.len(),
        old.len(),
        pending,
        flying,
        cursors.iter().map(|c| (c.read.0, c.replay_from.0)).collect::<Vec<_>>(),
        (resumed.0 - killed.0) / 1_000,
        expected.len() - never.len(),
        expected.len(),
        again,
        both.len()
    );
}
