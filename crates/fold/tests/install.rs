//! M3-WP5's gates in memory, on real raft members whose fold is the two-tier fold of `crates/fold`: BF-28 (learners
//! beside a voter and two on one site are distinct objects, replicate, serve read-index reads, and never count), BF-29
//! (a follower offline past the archive boundary rejoins by `InstallSnapshot` with matching fold digests), and F48's
//! bounded refold. The host here is a model of the site host: R2 is a map of objects, a leader's storage forgets the
//! entries an archive covers, and an install loads a tier snapshot and the archives after it.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use arrayvec::ArrayVec;
use graftyard_fold::{
    MemStore, Read, Restore, Rows, Schema, SnapshotFrame, SnapshotJob, TableDef, Tiered, Val, apply, decode_frames,
    digest_at, encode_frame, fold_digest, frames_header, refold_digest, refold_segment, restore,
};
use graftyard_raft::{
    CALL_DEADLINE, Catchup, Command, EntryBody, Envelope, Fold, Format, GroupConfig, GroupFold, Index, InstallSource,
    InstallWanted, LogEntry, MemRequests, Member, MemberConfig, Message, Record, ReleaseTag, RequestId, RequestRow,
    Resolution, Role, Scratch, Stored, TierSet, Ulid, Write, codec,
};
use graftyard_types::{
    ChainId, DbId, Member as MemberRef, MemberKind, Placement, PlacementChecks, PlacementName, PlacementRecord, Region,
    ReleaseId, RepoId, Rtt, Site, SiteId, SiteLink, SiteTable, Tier, Timestamp, learner_addr,
};
use smallvec::SmallVec;

const CHAIN: ChainId = ChainId::Repo(RepoId(DbId(7)));
const FMT: Format = Format { family: 1, version: 1 };
const A: SiteId = SiteId(0);
const B: SiteId = SiteId(1);
const C: SiteId = SiteId(2);
const VOTERS: [SiteId; 3] = [A, B, C];
/// Request rows are kept this long, so an install must rebuild them from the archives.
const RETENTION: Duration = Duration::from_secs(60);
/// Applied entries each member keeps in memory: small, so the leader's window moves past the archive boundary.
const WINDOW: u64 = 4;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Ev {
    Core(i64, i64),
    Meta(i64, i64),
}

impl Record for Ev {
    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(format!("{self:?}").as_bytes());
    }
}

struct Toy;
const CORE: Schema = Schema { tier: Tier::Core, tables: &[TableDef { name: "c", key: &["k"] }] };
const META: Schema = Schema { tier: Tier::Metadata, tables: &[TableDef { name: "m", key: &["k"] }] };

impl Tiered for Toy {
    type Event = Ev;
    fn tier_of(ev: &Ev) -> TierSet {
        match ev {
            Ev::Core(..) => TierSet::CORE,
            Ev::Meta(..) => TierSet::METADATA,
        }
    }
    fn schema(tier: Tier) -> Schema {
        if tier == Tier::Core { CORE } else { META }
    }
    fn write(&self, ev: &Ev, _: Tier, _: Timestamp, _: Format, w: &mut dyn Rows) {
        match ev {
            Ev::Core(k, v) => w.put("c", vec![Val::Int(*k)], vec![Val::Int(*v)]),
            Ev::Meta(k, v) => w.put("m", vec![Val::Int(*k)], vec![Val::Int(*v)]),
        }
    }
}

/// A repository-like fold: one store per tier, applied whole entries at a time.
#[derive(Clone, Debug, Default, PartialEq)]
struct Two {
    core: MemStore,
    meta: MemStore,
}

impl Two {
    fn get(&self, k: i64) -> Option<i64> {
        match Read::get(&self.core, "c", &[Val::Int(k)])?.first() {
            Some(Val::Int(v)) => Some(*v),
            _ => None,
        }
    }
}

struct Pass;
impl Scratch for Pass {
    type Event = Ev;
    fn apply(&mut self, _: &Ev) {}
    fn check(&self, _: &Ev) -> Result<(), Box<str>> {
        Ok(())
    }
}

impl Fold for Two {
    type Event = Ev;
    type Scratch<'a> = Pass;
    fn scratch(&self) -> Pass {
        Pass
    }
    fn apply(&mut self, _: &Ev) {
        unreachable!("a tiered fold applies whole entries")
    }
    fn apply_entry(&mut self, e: &LogEntry<Ev>) {
        apply(&Toy, &mut self.core, Tier::Core, e).unwrap();
        apply(&Toy, &mut self.meta, Tier::Metadata, e).unwrap();
    }
    fn tiers(ev: &Ev) -> TierSet {
        Toy::tier_of(ev)
    }
}

#[derive(Clone, Debug)]
struct Cmd(Vec<Ev>);

impl Record for Cmd {
    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(format!("{self:?}").as_bytes());
    }
}

impl Command for Cmd {
    type Fold = Two;
    type Dep = ();
    type Out = ();
    type Err = ();
    fn format(&self) -> Format {
        FMT
    }
    fn tiers(&self) -> TierSet {
        self.0.iter().fold(TierSet::NONE, |t, e| t.union(Toy::tier_of(e)))
    }
    fn deps(&self, _: &Two) -> SmallVec<[ChainId; 3]> {
        SmallVec::new()
    }
    fn decide(&self, _: &Pass, _: &[(ChainId, ())]) -> Result<(SmallVec<[Ev; 4]>, ()), ()> {
        Ok((self.0.iter().cloned().collect(), ()))
    }
    fn replay(&self, _: &[Ev]) {}
}

fn record() -> PlacementRecord {
    PlacementRecord {
        name: PlacementName::Tri,
        sites: VOTERS.iter().copied().collect(),
        home: A,
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    }
}

fn config(me: SiteId) -> MemberConfig {
    MemberConfig {
        chain: CHAIN,
        me,
        record: record(),
        regions: vec![(A, Region::Enam), (B, Region::Weur), (C, Region::Apac)],
        release: ReleaseTag { id: ReleaseId([7; 32]), seq: 1 },
        reads: vec![FMT],
    }
}

/// One member object and the host around it: its disk, and the first index its storage still holds.
struct Node {
    m: Option<Member<Cmd>>,
    disk: Stored<Two>,
    held_from: Index,
    seed: u64,
}

/// The host's side of every member: delivery, storage, R2, catchup reads, and installs.
struct Net {
    now: Timestamp,
    nodes: BTreeMap<SiteId, Node>,
    /// The object each address names: (site, `do_name`). Two members on one object would be one member.
    objects: BTreeMap<SiteId, (SiteId, Box<str>)>,
    /// Addresses whose messages are dropped both ways.
    cut: BTreeSet<SiteId>,
    inbox: Vec<Envelope<Ev>>,
    resolutions: Vec<(SiteId, Resolution<Cmd>)>,
    catchups: Vec<(SiteId, Catchup)>,
    installs: Vec<(SiteId, InstallWanted)>,
    r2: BTreeMap<String, Vec<u8>>,
    /// Every message sent: (from, to, kind), for the learner read-index rate checks.
    sent: Vec<(Timestamp, SiteId, SiteId, &'static str)>,
    /// `InstallSnapshot` loads the hosts ran, each with how many acks had been sent before it, and the AppendEntries
    /// acks each member sent, in order: (time, from, matched).
    installed: Vec<(usize, SiteId)>,
    acks: Vec<(Timestamp, SiteId, Index)>,
    /// Each granted read's value of core key 50, read through `fresh` in the step that granted it, as a host must.
    seen: BTreeMap<(SiteId, graftyard_raft::ReadId), Option<i64>>,
    next_rid: u128,
}

fn kind<E>(m: &Message<E>) -> &'static str {
    match m {
        Message::Append(_) => "append",
        Message::AppendRes(_) => "appendRes",
        Message::Install(_) => "install",
        Message::ReadIndex { .. } => "readIndex",
        Message::ReadIndexRes { .. } => "readIndexRes",
        _ => "other",
    }
}

impl Net {
    fn new() -> Net {
        let mut n = Net {
            now: Timestamp(10_000),
            nodes: BTreeMap::new(),
            objects: BTreeMap::new(),
            cut: BTreeSet::new(),
            inbox: Vec::new(),
            resolutions: Vec::new(),
            catchups: Vec::new(),
            installs: Vec::new(),
            r2: BTreeMap::new(),
            sent: Vec::new(),
            installed: Vec::new(),
            acks: Vec::new(),
            seen: BTreeMap::new(),
            next_rid: 1,
        };
        for m in record().bootstrap_members(CHAIN) {
            n.add(&m);
        }
        n
    }

    /// Creates the object for `m` on its site, at its address.
    fn add(&mut self, m: &MemberRef) {
        let addr = m.addr();
        let object = (m.site, m.do_name.clone());
        assert!(
            !self.objects.values().any(|o| *o == object),
            "{} on {:?} names an object another member has",
            m.do_name,
            m.site
        );
        assert!(self.objects.insert(addr, object).is_none(), "address {addr:?} already names a member");
        let seed = 100 + u64::from(addr.0);
        let mut mem = Member::new(config(addr), seed).unwrap();
        mem.set_log_window(WINDOW);
        mem.set_request_retention(RETENTION);
        self.nodes.insert(addr, Node { m: Some(mem), disk: Stored::new(Two::default()), held_from: Index(1), seed });
    }

    fn m(&self, s: SiteId) -> &Member<Cmd> {
        self.nodes[&s].m.as_ref().expect("member is up")
    }

    fn on<R>(&mut self, s: SiteId, f: impl FnOnce(&mut Member<Cmd>, Timestamp) -> graftyard_raft::Ready<Cmd, R>) -> R {
        let now = self.now;
        let node = self.nodes.get_mut(&s).expect("node");
        let m = node.m.as_mut().expect("member is up");
        let ready = f(m, now);
        let installed = ready.writes.iter().any(|w| matches!(w, Write::Install(_)));
        let (r, mut out) = ready.persist(&mut node.disk);
        if installed {
            // The host stores the fold it loaded with the install's write.
            node.disk.fold = node.m.as_ref().unwrap().group_fold().clone();
            self.installed.push((self.acks.len(), s));
        }
        loop {
            for e in out.early.drain(..).chain(out.messages.drain(..)) {
                self.sent.push((now, e.from, e.to, kind(&e.msg)));
                if let Message::AppendRes(r) = &e.msg
                    && r.ok
                {
                    self.acks.push((now, e.from, r.matched));
                }
                self.inbox.push(e);
            }
            for r in out.resolutions.drain(..) {
                if let Resolution::Read { id, result: Ok(g) } = &r {
                    let m = self.nodes[&s].m.as_ref().unwrap();
                    let v = m.fresh(g).expect("a grant is good in the step that made it").fold().get(50);
                    self.seen.insert((s, *id), v);
                }
                self.resolutions.push((s, r));
            }
            self.catchups.extend(out.catchup.drain(..).map(|c| (s, c)));
            self.installs.extend(out.install.drain(..).map(|w| (s, w)));
            let Some(ack) = out.self_ack.take() else { break };
            let node = self.nodes.get_mut(&s).unwrap();
            out = node.m.as_mut().unwrap().self_ack(now, ack).persist(&mut node.disk).1;
        }
        r
    }

    fn up(&self, s: SiteId) -> bool {
        self.nodes.get(&s).is_some_and(|n| n.m.is_some())
    }

    fn deliver(&mut self) {
        for _ in 0..10_000 {
            if self.inbox.is_empty() && self.catchups.is_empty() && self.installs.is_empty() {
                return;
            }
            for e in std::mem::take(&mut self.inbox) {
                if self.cut.contains(&e.from) || self.cut.contains(&e.to) || !self.up(e.to) {
                    continue;
                }
                let Envelope { from, to, msg } = e;
                self.on(to, |m, now| m.step(now, from, msg));
            }
            for (s, c) in std::mem::take(&mut self.catchups) {
                self.serve_catchup(s, c);
            }
            for (s, w) in std::mem::take(&mut self.installs) {
                if self.up(s) {
                    self.serve_install(s, w);
                }
            }
        }
        panic!("messages never settled");
    }

    fn advance(&mut self, ms: u64) {
        self.now = Timestamp(self.now.0 + ms);
        let up: Vec<SiteId> = self.nodes.keys().copied().filter(|s| self.up(*s)).collect();
        for s in up {
            self.on(s, |m, now| m.tick(now));
        }
        self.deliver();
    }

    /// A follower behind the leader's window: the entries from storage, if storage still holds them.
    fn serve_catchup(&mut self, s: SiteId, c: Catchup) {
        let node = &self.nodes[&s];
        let from = c.from.0.max(1);
        if from < node.held_from.0 {
            return;
        }
        let d = &node.disk;
        let entries: Vec<LogEntry<Ev>> =
            d.log.iter().filter(|e| e.index.0 >= from && e.index <= c.through).cloned().collect();
        self.on(s, |m, now| m.catchup(now, c, entries));
    }

    /// The host's half of `InstallSnapshot`: each tier from its snapshot, the archives' entries above it applied,
    /// and the `request` rows of the archived entries still inside the retention, rebuilt.
    fn serve_install(&mut self, s: SiteId, w: InstallWanted) {
        let src = w.install.source.clone();
        let frames = |key: &str| decode_frames(&self.r2[key]).unwrap();
        let core_frames = frames(&src.core);
        let Some(SnapshotFrame::End(tr)) = core_frames.last() else { panic!("no trailer") };
        let to = tr.to;
        let mut fold = Two::default();
        restore(&mut fold.core, &CORE, &core_frames, (CHAIN, to)).unwrap();
        let meta_frames = frames(src.metadata.as_deref().expect("a metadata snapshot"));
        restore(&mut fold.meta, &META, &meta_frames, (CHAIN, to)).unwrap();
        let mut entries: Vec<LogEntry<Ev>> = Vec::new();
        for key in &src.archives {
            entries.extend(codec::from_bytes::<Vec<LogEntry<Ev>>>(&self.r2[&**key]).unwrap());
        }
        assert!(entries.windows(2).all(|p| p[1].index.0 == p[0].index.0 + 1), "archives are contiguous");
        let last_at = entries.last().expect("archived entries").at;
        let cut = Timestamp(last_at.0.saturating_sub(RETENTION.as_millis() as u64));
        let rows: MemRequests<Ev> = entries
            .iter()
            .filter(|e| e.index <= to && e.at >= cut)
            .flat_map(|e| {
                let EntryBody::Events(evs) = &e.body else { return Vec::new() };
                e.requests
                    .iter()
                    .map(|(id, r)| {
                        let events = evs[r.start as usize..r.end as usize].to_vec();
                        (*id, RequestRow { index: e.index, at: e.at, events })
                    })
                    .collect()
            })
            .collect();
        let mut gf = GroupFold::from_parts(fold, to, Some(src.config.clone()), src.id_block, Box::new(rows));
        gf.set_retention(RETENTION);
        for e in entries.iter().filter(|e| e.index > to) {
            gf.apply(e);
        }
        let r = self.on(s, |m, now| m.install(now, w, gf));
        assert!(r.is_ok() || matches!(r, Err(graftyard_raft::InstallError::NotNeeded)), "{r:?}");
    }

    fn commit(&mut self, l: SiteId, evs: Vec<Ev>) -> RequestId {
        let id = RequestId(Ulid(self.next_rid));
        self.next_rid += 1;
        self.on(l, |m, now| m.commit(now, id, Cmd(evs))).expect("admitted");
        self.deliver();
        id
    }

    /// One entry a second through `l`, touching both tiers.
    fn write(&mut self, l: SiteId, n: i64) {
        for k in 0..n {
            let v = self.now.0 as i64;
            self.commit(l, vec![Ev::Core(k % 7, v), Ev::Meta(k % 11, v + 1)]);
            self.advance(1_000);
        }
    }

    fn committed(&self, id: RequestId) -> bool {
        self.nodes.values().filter_map(|n| n.m.as_ref()).any(|m| m.group_fold().request(id).is_some())
    }

    fn elect(&mut self, s: SiteId) {
        for _ in 0..3 {
            self.on(s, |m, now| m.campaign(now));
            self.deliver();
            if self.m(s).role() == Role::Leader {
                break;
            }
        }
        assert_eq!(self.m(s).role(), Role::Leader);
        self.advance(CALL_DEADLINE.as_millis() as u64 + 1);
    }

    fn crash(&mut self, s: SiteId) {
        self.nodes.get_mut(&s).unwrap().m = None;
    }

    fn restart(&mut self, s: SiteId) {
        let node = self.nodes.get_mut(&s).unwrap();
        node.seed += 1000;
        let (mut m, ready) = Member::restore(config(s), node.disk.clone(), self.now, node.seed).unwrap();
        m.set_log_window(WINDOW);
        m.set_request_retention(RETENTION);
        let ((), out) = ready.persist(&mut node.disk);
        node.m = Some(m);
        self.inbox.extend(out.messages);
    }

    /// Snapshots both tiers of `s` at its applied index into R2, as its snapshot writer would, and returns `to`.
    fn snapshot(&mut self, s: SiteId) -> Index {
        let fold = self.m(s).group_fold().fold.clone();
        let mut to = Index(0);
        for (schema, mut store) in [(CORE, fold.core), (META, fold.meta)] {
            let mut job = SnapshotJob::begin(&mut store, schema, CHAIN, ReleaseId([7; 32]), FMT).unwrap();
            let mut bytes = frames_header();
            while let Some(f) = job.next(&store, 5) {
                encode_frame(&f, &mut bytes);
            }
            to = job.to();
            self.r2.insert(format!("snap/{:?}/{}", schema.tier, to.0), bytes);
        }
        to
    }

    /// The leader archives its stored entries through `through`, from `from`, in one object, forgets them, and names
    /// the snapshot at `snap` and the archive in its `InstallSnapshot`.
    fn archive(&mut self, l: SiteId, snap: Index, from: Index, through: Index) -> InstallSource {
        let node = self.nodes.get_mut(&l).unwrap();
        let entries: Vec<LogEntry<Ev>> =
            node.disk.log.iter().filter(|e| e.index >= from && e.index <= through).cloned().collect();
        assert_eq!(entries.len() as u64, through.0 - from.0 + 1);
        let key = format!("archive/{}-{}", from.0, through.0);
        self.r2.insert(key.clone(), codec::to_bytes(&entries));
        node.held_from = through;
        let last = entries.last().unwrap();
        let m = node.m.as_ref().unwrap();
        let source = InstallSource {
            core: format!("snap/Core/{}", snap.0).into(),
            metadata: Some(format!("snap/Metadata/{}", snap.0).into()),
            archives: SmallVec::from_iter([key.into()]),
            through,
            through_term: last.term,
            through_digest: graftyard_raft::digest(last),
            config: m.committed_config().cloned().expect("committed config"),
            id_block: m.group_fold().id_block(),
        };
        node.m.as_mut().unwrap().set_install(Some(source.clone()));
        source
    }

    /// The first index whose entry is inside the request retention of entry `through`.
    fn retention_start(&self, l: SiteId, through: Index) -> Index {
        let log = &self.nodes[&l].disk.log;
        let at = log[through.0 as usize - 1].at;
        let cut = at.0.saturating_sub(RETENTION.as_millis() as u64);
        log.iter().find(|e| e.at.0 >= cut).map(|e| e.index).unwrap()
    }
}

/// Every member holds the leader's log tail, and at the last index all have applied, the same fold digests in both
/// tiers; at its own applied index each fold refolds to its kept digest, and its request rows are the leader's at or
/// below that index. A follower learns the commit index from the next append, so it may apply one entry less.
fn assert_converged(n: &Net, l: SiteId, who: &[SiteId]) {
    let lm = n.m(l);
    let at = who.iter().map(|s| n.m(*s).group_fold().applied()).min().unwrap();
    assert!(lm.group_fold().applied().0 - at.0 <= 1, "every member applied all but at most the newest entry");
    let tiers = |m: &Member<Cmd>| {
        let f = &m.group_fold().fold;
        (digest_at(&f.core, at), digest_at(&f.meta, at))
    };
    for s in who {
        let m = n.m(*s);
        assert_eq!(m.last_index(), lm.last_index(), "{s:?} is not stuck");
        assert_eq!(m.digest_of(m.last_index()), lm.digest_of(lm.last_index()), "{s:?} holds the leader's tail");
        let d = tiers(m);
        assert!(d.0.is_some() && d.1.is_some(), "{s:?} has digests at {at:?}");
        assert_eq!(d, tiers(lm), "{s:?} fold digests at {at:?}");
        // Rows expire as entries apply, so compare where both still keep them.
        let lo = |m: &Member<Cmd>| m.group_fold().requests().iter().map(|(_, r)| r.index).min().unwrap_or(Index(0));
        let lo = lo(m).max(lo(lm));
        let rows = |m: &Member<Cmd>| {
            m.group_fold().requests().into_iter().filter(|(_, r)| r.index >= lo && r.index <= at).collect::<Vec<_>>()
        };
        assert!(!rows(m).is_empty(), "{s:?} keeps request rows");
        assert_eq!(rows(m), rows(lm), "{s:?} request rows");
        assert_eq!(m.group_fold().config(), lm.group_fold().config(), "{s:?} config");
        let (core, meta) = (&m.group_fold().fold.core, &m.group_fold().fold.meta);
        assert_eq!(fold_digest(core, Tier::Core), refold_digest(core, &CORE), "{s:?} core refolds");
        assert_eq!(fold_digest(meta, Tier::Metadata), refold_digest(meta, &META), "{s:?} metadata refolds");
    }
}

/// BF-29: C is offline while the leader commits, snapshots, archives, and forgets the entries C needs next. On its
/// return it is sent `InstallSnapshot`, loads the snapshot and the archives, acks nothing below `through` until then,
/// and ends with the fold digests, request rows, and log of the other members.
#[test]
fn bf29_a_follower_offline_past_the_archive_boundary_rejoins_by_install_snapshot() {
    let mut n = Net::new();
    n.elect(A);
    n.write(A, 5);
    let c_left = n.m(C).last_index();
    n.crash(C);
    n.write(A, 40);
    let snap = n.snapshot(B);
    n.write(A, 30);
    let through = n.m(A).group_fold().applied();
    let from = n.retention_start(A, through).min(Index(snap.0 + 1));
    let source = n.archive(A, snap, from, through);
    n.write(A, 10);
    assert!(n.m(A).log_base().index > through, "the leader's window moved past the boundary");
    assert!(c_left < snap && snap < through, "C left at {c_left:?}, snapshot {snap:?}, through {through:?}");
    // Request rows C can only have from the archives: entries after the snapshot, through the boundary, that the
    // leader still keeps.
    let archived: Vec<RequestId> = n
        .m(A)
        .group_fold()
        .requests()
        .into_iter()
        .filter(|(_, r)| r.index > snap && r.index <= through)
        .map(|(id, _)| id)
        .collect();
    assert!(!archived.is_empty(), "the leader keeps rows of archived entries");
    n.restart(C);
    for _ in 0..20 {
        n.advance(CALL_DEADLINE.as_millis() as u64 + 1);
    }
    n.write(A, 3);
    n.advance(CALL_DEADLINE.as_millis() as u64 + 1);
    let installs: Vec<_> = n.installed.iter().filter(|(_, s)| *s == C).collect();
    assert_eq!(installs.len(), 1, "C installed once");
    let before = &n.acks[..installs[0].0];
    assert!(
        before.iter().filter(|(_, s, _)| *s == C).all(|(_, _, m)| *m <= c_left),
        "C acked nothing past where it left before it installed"
    );
    assert!(n.m(C).log_base().index >= source.through, "C starts above the boundary");
    let kept: Vec<&RequestId> = archived.iter().filter(|id| n.m(A).group_fold().request(**id).is_some()).collect();
    assert!(!kept.is_empty(), "rows of archived entries are still inside retention");
    for id in kept {
        assert!(n.m(C).group_fold().request(*id).is_some(), "C has request {id:?}'s row from the archives");
    }
    assert_converged(&n, A, &[B, C]);
    let at = VOTERS.iter().map(|s| n.m(*s).group_fold().applied()).min().unwrap();
    let d = |s: SiteId| {
        let f = &n.m(s).group_fold().fold;
        (digest_at(&f.core, at).unwrap(), digest_at(&f.meta, at).unwrap())
    };
    println!(
        "BF-29: C left at {c_left:?}; snapshot at {snap:?}; archive {from:?}..={through:?}; leader base {:?}; C \
         installed once, after {} acks in the run; C last {:?}; at {at:?} core/meta digests A {:?} B {:?} C {:?}; \
         request rows of archived entries checked on C: {}",
        n.m(A).log_base().index,
        installs[0].0,
        n.m(C).last_index(),
        d(A),
        d(B),
        d(C),
        archived.len(),
    );
    // C still follows: the next write commits on it.
    let id = n.commit(A, vec![Ev::Core(99, 1)]);
    n.write(A, 1);
    assert!(n.m(C).group_fold().request(id).is_some());
    assert_eq!(n.m(C).group_fold().fold.get(99), Some(1));
}

fn site(id: SiteId, region: Region) -> Site {
    Site {
        id,
        script: String::new(),
        account: "x".into(),
        region,
        jurisdiction: None,
        bucket: String::new(),
        release: None,
        link: SiteLink::Binding { member_class: "Member".into() },
        spare: false,
    }
}

/// The group's placement with three learners: slot 0 on the leader's site A, slots 1 and 2 both on site B.
fn with_learners(n: &Net) -> (Placement, Vec<MemberRef>) {
    let Some((_, GroupConfig::Normal(p))) = n.m(A).committed_config().cloned() else { panic!("normal config") };
    let table = SiteTable(vec![site(A, Region::Enam), site(B, Region::Weur), site(C, Region::Apac)]);
    let rtt: Vec<(SiteId, Rtt)> =
        [(A, B), (A, C), (B, C)].iter().map(|&(x, y)| (x, Rtt { peer: y, median_ms: 80, at: Timestamp(0) })).collect();
    let checks = PlacementChecks { sites: &table, separate_accounts: false, rtt: &rtt, near: &[] };
    let g = p.next_generation();
    let learners: ArrayVec<MemberRef, 16> =
        [(A, 0), (B, 1), (B, 2)].iter().map(|&(s, k)| MemberRef::new(CHAIN, s, MemberKind::Learner, k, g)).collect();
    let v = learners.to_vec();
    (Placement::new(p.voters().iter().cloned().collect(), learners, p.home(), &checks).unwrap(), v)
}

/// BF-28: a learner on its voter's site and two learners on one site are distinct objects. Each joins by
/// `InstallSnapshot`, holds the voters' fold, serves read-index reads that see the latest write, and never counts
/// toward a commit.
#[test]
fn bf28_learners_beside_a_voter_and_on_one_site_are_distinct_objects_that_never_vote() {
    let mut n = Net::new();
    n.elect(A);
    n.write(A, 30);
    let snap = n.snapshot(B);
    n.write(A, 10);
    let through = n.m(A).group_fold().applied();
    let from = n.retention_start(A, through).min(Index(snap.0 + 1));
    n.archive(A, snap, from, through);
    n.write(A, 5);
    let (p, learners) = with_learners(&n);
    for l in &learners {
        n.add(l);
    }
    let addrs: Vec<SiteId> = learners.iter().map(|l| l.addr()).collect();
    assert_eq!(n.objects.len(), 6, "three voters and three learners, each its own object: {:?}", n.objects);
    let objects: BTreeSet<_> = n.objects.values().collect();
    assert_eq!(objects.len(), 6);
    assert!(addrs.iter().all(|a| !VOTERS.contains(a)), "no learner shares a voter's address");
    n.on(A, |m, now| m.reconfigure(now, GroupConfig::Normal(Box::new(p.clone())))).unwrap();
    n.deliver();
    for _ in 0..10 {
        n.advance(CALL_DEADLINE.as_millis() as u64 + 1);
    }
    n.write(A, 3);
    n.advance(CALL_DEADLINE.as_millis() as u64 + 1);
    for a in &addrs {
        assert!(n.installed.iter().any(|(_, s)| s == a), "learner {a:?} joined by InstallSnapshot");
        assert_eq!(n.m(*a).role(), Role::Follower);
    }
    assert_converged(&n, A, &[B, C]);
    assert_converged(&n, A, &addrs);
    // A learner read sees the write committed just before it.
    let id = n.commit(A, vec![Ev::Core(50, 777)]);
    assert!(n.committed(id));
    for a in &addrs {
        let rid = n.on(*a, |m, now| m.read_index(now)).expect("a learner reads");
        n.deliver();
        n.advance(60);
        assert_eq!(n.seen.get(&(*a, rid)), Some(&Some(777)), "learner {a:?} read the latest write");
    }
    // Learners never count: with both followers cut off, a write does not commit however many learners hold it.
    n.cut.extend([B, C]);
    let id = RequestId(Ulid(9_999_999));
    n.on(A, |m, now| m.commit(now, id, Cmd(vec![Ev::Core(51, 1)]))).unwrap();
    for _ in 0..5 {
        n.advance(CALL_DEADLINE.as_millis() as u64 + 1);
    }
    assert!(addrs.iter().all(|a| n.m(*a).last_index() == n.m(A).last_index()), "the learners hold the entry");
    assert!(!n.committed(id), "learners' acks made no commit");
    n.cut.clear();
    for _ in 0..3 {
        n.advance(CALL_DEADLINE.as_millis() as u64 + 1);
    }
    assert!(n.committed(id), "the voters commit it once they hear the leader");
    // A learner never campaigns.
    assert_eq!(n.on(addrs[0], |m, now| m.campaign(now)), graftyard_raft::Verdict::MayNotLead);
    println!("BF-28: objects {:?}; learner addresses {addrs:?}; installs {:?}", n.objects, n.installed);
}

/// "Follower read" for a learner (`design/DESIGN.md`): a learner behind never grants a read-index read from its older
/// fold, exactly as a follower: cut off, it grants nothing and fails the read at CAMPAIGN_DEADLINE (the member then
/// answers `overloaded` after FOLLOWER_WAIT), and once back its read sees the write made while it was away.
#[test]
fn a_learner_behind_grants_no_read_from_its_older_fold() {
    let mut n = Net::new();
    n.elect(A);
    n.write(A, 2);
    let (p, learners) = with_learners(&n);
    for l in &learners {
        n.add(l);
    }
    n.on(A, |m, now| m.reconfigure(now, GroupConfig::Normal(Box::new(p.clone())))).unwrap();
    for _ in 0..5 {
        n.advance(CALL_DEADLINE.as_millis() as u64 + 1);
    }
    let l = learners[1].addr();
    n.commit(A, vec![Ev::Core(50, 1)]);
    // The next entry carries the commit index to the learner.
    n.write(A, 1);
    assert_eq!(n.m(l).group_fold().fold.get(50), Some(1), "the learner applied the first write");
    // Behind by the commit index alone: it holds the next entry but no append has told it the entry committed. Its
    // read waits for the leader's index and applies through it first, so it sees the new value, never the old one.
    n.commit(A, vec![Ev::Core(50, 3)]);
    assert_eq!(n.m(l).group_fold().fold.get(50), Some(1), "the learner has not applied the new entry");
    let rid = n.on(l, |m, now| m.read_index(now)).expect("a learner reads");
    for _ in 0..4 {
        n.advance(CALL_DEADLINE.as_millis() as u64 / 4);
    }
    assert_eq!(n.seen.get(&(l, rid)), Some(&Some(3)), "the read saw the entry committed before it");
    n.commit(A, vec![Ev::Core(50, 1)]);
    n.write(A, 1);
    n.cut.insert(l);
    n.commit(A, vec![Ev::Core(50, 2)]);
    n.advance(1);
    assert_eq!(n.m(l).group_fold().fold.get(50), Some(1), "the learner is behind");
    let rid = n.on(l, |m, now| m.read_index(now)).expect("a learner reads");
    for _ in 0..4 {
        n.advance(CALL_DEADLINE.as_millis() as u64 / 2);
        assert!(!n.seen.contains_key(&(l, rid)), "no grant from the learner's older fold");
    }
    n.advance(CALL_DEADLINE.as_millis() as u64 * 4);
    let failed = n
        .resolutions
        .iter()
        .any(|(s, r)| *s == l && matches!(r, Resolution::Read { id, result: Err(_) } if *id == rid));
    assert!(failed, "the read failed instead of answering stale");
    n.cut.clear();
    let mut seen = None;
    for _ in 0..6 {
        let rid = n.on(l, |m, now| m.read_index(now)).ok();
        for _ in 0..4 {
            n.advance(CALL_DEADLINE.as_millis() as u64 / 2);
        }
        seen = rid.and_then(|r| n.seen.get(&(l, r)).copied());
        if seen.is_some() {
            break;
        }
    }
    assert_eq!(seen, Some(Some(2)), "once back, the learner's read sees the write made while it was away");
}

/// A learner sends one read-index request at a time and at most one every 50 ms; a follower batches every 10 ms.
#[test]
fn learner_read_index_keeps_one_in_flight_and_a_50_ms_floor() {
    let mut n = Net::new();
    n.elect(A);
    n.write(A, 2);
    let (p, learners) = with_learners(&n);
    for l in &learners {
        n.add(l);
    }
    n.on(A, |m, now| m.reconfigure(now, GroupConfig::Normal(Box::new(p.clone())))).unwrap();
    for _ in 0..5 {
        n.advance(CALL_DEADLINE.as_millis() as u64 + 1);
    }
    let l = learners[1].addr();
    let sent = |n: &Net, from: SiteId, since: Timestamp| {
        n.sent.iter().filter(|(t, f, _, k)| *f == from && *t >= since && *k == "readIndex").map(|(t, ..)| *t).collect()
    };
    for (who, floor) in [(l, 50), (B, 10)] {
        let t0 = n.now;
        // Each read arrives 2 ms after the last, answered 1 ms after each step: 40 reads over 80 ms.
        for _ in 0..40 {
            n.on(who, |m, now| m.read_index(now)).unwrap();
            n.advance(2);
        }
        let times: Vec<Timestamp> = sent(&n, who, t0);
        assert!(times.len() >= 2, "{who:?} sent {times:?}");
        assert!(
            times.windows(2).all(|w| w[1].0 - w[0].0 >= floor),
            "{who:?} batches at most every {floor} ms: {times:?}"
        );
        n.advance(100);
        let reads: Vec<_> =
            n.resolutions.iter().filter(|(s, r)| *s == who && matches!(r, Resolution::Read { .. })).collect();
        let ok = reads.iter().filter(|(_, r)| matches!(r, Resolution::Read { result: Ok(_), .. })).count();
        assert_eq!((reads.len(), ok), (40, 40), "every read of {who:?} was granted: {reads:?}");
        println!(
            "read-index from {who:?}: {} batches for 40 reads, gaps {:?} ms",
            times.len(),
            times.windows(2).map(|w| w[1].0 - w[0].0).collect::<Vec<_>>()
        );
        n.resolutions.clear();
    }
}

/// F48: a fold audit's refold that holds one page and one entry at a time: the snapshot at `from` staged as pages
/// (`refold-stream`), then the log tail applied through a `SnapView`, compared with each member's `digest_at` of the same index; and refolding
/// from one snapshot to the next reaches the next trailer's digest. A skipped or changed entry is caught.
#[test]
fn f48_a_refold_from_a_snapshot_plus_the_log_tail_matches_every_member() {
    let mut n = Net::new();
    n.elect(A);
    n.write(A, 20);
    let s1 = n.snapshot(B);
    n.write(A, 25);
    let s2 = n.snapshot(C);
    n.write(A, 5);
    // Every voter has applied through `to`; the leader one more.
    let to = VOTERS.iter().map(|s| n.m(*s).group_fold().applied()).min().unwrap();
    let tail: Vec<LogEntry<Ev>> =
        n.nodes[&A].disk.log.iter().filter(|e| e.index > s1 && e.index <= to).cloned().collect();
    for (schema, tier) in [(CORE, Tier::Core), (META, Tier::Metadata)] {
        let frames = decode_frames(&n.r2[&format!("snap/{tier:?}/{}", s1.0)]).unwrap();
        let got = refold_segment(&Toy, schema, (CHAIN, s1), frames.clone(), tail.clone()).unwrap();
        for s in VOTERS {
            let f = &n.m(s).group_fold().fold;
            let store = if tier == Tier::Core { &f.core } else { &f.meta };
            assert_eq!(digest_at(store, to), Some(got), "{s:?} {tier:?} at {to:?}");
        }
        // Segment to the next snapshot: its trailer's digest.
        let next = decode_frames(&n.r2[&format!("snap/{tier:?}/{}", s2.0)]).unwrap();
        let Some(SnapshotFrame::End(tr)) = next.last() else { panic!() };
        let seg: Vec<_> = tail.iter().filter(|e| e.index <= s2).cloned().collect();
        let got = refold_segment(&Toy, schema, (CHAIN, s1), frames.clone(), seg.clone()).unwrap();
        assert_eq!(got, tr.fold_digest, "{tier:?} refolds from {s1:?} to the snapshot at {s2:?}");
        // A dropped entry is refused, and a changed one digests differently.
        let mut gap = seg.clone();
        gap.remove(3);
        assert!(refold_segment(&Toy, schema, (CHAIN, s1), frames.clone(), gap).is_err());
        let mut changed = seg.clone();
        let last = changed.len() - 1;
        if let EntryBody::Events(evs) = &mut changed[last].body {
            evs[0] = Ev::Core(1, -1);
            evs[1] = Ev::Meta(1, -1);
        }
        let bad = refold_segment(&Toy, schema, (CHAIN, s1), frames, changed).unwrap();
        assert_ne!(bad, tr.fold_digest, "{tier:?}: a changed entry is caught");
    }
    let _ = learner_addr(0);
}

/// The site host's loader (`workers/site/src/load.rs`) restores a snapshot over many requests: a frame at a time,
/// with the `Restore` stored between them as JSON, and `finish_rows`. It reaches the trailer's digest as `restore`
/// does, and a frame lost between two batches is refused at the trailer, not loaded short.
#[test]
fn a_restore_resumed_across_batches_from_its_stored_cursor_ends_at_the_trailers_digest() {
    let mut n = Net::new();
    n.elect(A);
    n.write(A, 30);
    let s = n.snapshot(B);
    for (schema, tier) in [(CORE, Tier::Core), (META, Tier::Metadata)] {
        let frames = decode_frames(&n.r2[&format!("snap/{tier:?}/{}", s.0)]).unwrap();
        assert!(frames.len() >= 3, "{tier:?}: {} frames", frames.len());
        let Some(SnapshotFrame::End(tr)) = frames.last() else { panic!() };
        let resumed = |skip: Option<usize>| -> Result<MemStore, graftyard_fold::SnapshotError> {
            let mut store = MemStore::default();
            let mut cursor: Option<String> = None;
            for (k, f) in frames.iter().enumerate() {
                if Some(k) == skip {
                    continue;
                }
                let mut r = match cursor.take() {
                    Some(c) => Restore::resume(schema, serde_json::from_str(&c).unwrap()),
                    None => Restore::begin(&store, schema, (CHAIN, s)).unwrap(),
                };
                r.frame(&mut store, f)?;
                if k + 1 == frames.len() {
                    r.finish_rows(&mut store)?;
                    return Ok(store);
                }
                cursor = Some(serde_json::to_string(&r.cursor()).unwrap());
            }
            unreachable!()
        };
        let store = resumed(None).unwrap();
        assert_eq!(fold_digest(&store, tier), tr.fold_digest, "{tier:?}");
        assert_eq!(refold_digest(&store, &schema), tr.fold_digest, "{tier:?} refolds");
        assert!(resumed(Some(1)).is_err(), "{tier:?}: a lost frame is refused");
    }
}

/// B0-30 in memory (`build/BUILD-PLAN.md`, M0-WP9; AM1-WP1's `LOG_MAX_BYTES`): 30 days of writes, an entry an hour,
/// with the log cap scaled to `CAP` bytes. After each write the leader asks `archive_due` for the next range, puts it
/// in two buckets (`R2Key::Archive` in two of the placement's buckets), and every member truncates its stored log
/// through that range only once it can find the key in both. Checks: no member's stored log passes the cap by more
/// than one entry, every archived entry is readable by its time from the archives (`RestoredTo`), and a follower
/// offline past every archive rejoins by `InstallSnapshot` from them.
#[test]
fn b0_30_log_max_bytes_archives_to_two_buckets_and_keeps_every_member_under_the_cap() {
    use graftyard_fold::archive_due;
    const CAP: u64 = 6_000;
    const HOUR: u64 = 3_600_000;
    let mut n = Net::new();
    n.elect(A);
    n.crash(C);
    let mut archived: Vec<(Index, Index, String)> = Vec::new();
    let mut peak = 0u64;
    let mut snaps: Vec<Index> = Vec::new();
    let mut truncations = 0;
    for h in 0..30 * 24 {
        let v = h as i64;
        n.commit(A, vec![Ev::Core(v % 13, v), Ev::Meta(v % 17, v)]);
        n.now = Timestamp(n.now.0 + HOUR);
        n.advance(CALL_DEADLINE.as_millis() as u64 + 1);
        // A snapshot of both tiers once a day, as the snapshot writer would.
        if h % 24 == 23 {
            snaps.push(n.snapshot(B));
        }
        let from = archived.last().map_or(Index(1), |a| Index(a.1.0 + 1));
        let stored: Vec<(Index, Timestamp, u64)> = n.nodes[&A]
            .disk
            .log
            .iter()
            .filter(|e| e.index >= from)
            .map(|e| (e.index, e.at, graftyard_raft::encode_entry(e).len() as u64))
            .collect();
        let committed = n.m(A).state().commit;
        if let Some((lo, hi)) = archive_due(&stored, n.now, !snaps.is_empty(), committed, CAP) {
            let entries: Vec<LogEntry<Ev>> =
                n.nodes[&A].disk.log.iter().filter(|e| e.index >= lo && e.index <= hi).cloned().collect();
            let key = format!("archive/{}-{}", lo.0, hi.0);
            for bucket in ["b0", "b1"] {
                n.r2.insert(format!("{bucket}/{key}"), codec::to_bytes(&entries));
            }
            archived.push((lo, hi, key));
        }
        // Each live member truncates through the latest range both buckets hold.
        if let Some((_, hi, key)) = archived.last() {
            let both = ["b0", "b1"].iter().all(|b| n.r2.contains_key(&format!("{b}/{key}")));
            for s in [A, B] {
                let node = n.nodes.get_mut(&s).unwrap();
                let applied = node.m.as_ref().unwrap().group_fold().applied();
                // The row at `hi` stays, as the site host keeps it (`truncate`): it anchors the next entry's `prev`
                // and a catchup's first entry.
                if both && *hi <= applied && node.disk.base.index.0 + 1 < hi.0 {
                    let k = (hi.0 - node.disk.base.index.0) as usize - 1;
                    let last = &node.disk.log[k - 1];
                    node.disk.base = graftyard_raft::LogBase {
                        index: last.index,
                        term: last.term,
                        digest: graftyard_raft::digest(last),
                    };
                    node.disk.log.drain(..k);
                    node.held_from = *hi;
                    truncations += 1;
                }
            }
        }
        for s in [A, B] {
            let bytes: u64 = n.nodes[&s].disk.log.iter().map(|e| graftyard_raft::encode_entry(e).len() as u64).sum();
            peak = peak.max(bytes);
        }
    }
    let largest = n.nodes[&A].disk.log.iter().map(|e| graftyard_raft::encode_entry(e).len() as u64).max().unwrap();
    // Every archived entry is readable by time from the archives: the last entry at or before each hour.
    let mut read = 0;
    for (_, _, key) in &archived {
        let es: Vec<LogEntry<Ev>> = codec::from_bytes(&n.r2[&format!("b1/{key}")]).unwrap();
        assert!(es.windows(2).all(|p| p[1].index.0 == p[0].index.0 + 1 && p[1].prev == graftyard_raft::digest(&p[0])));
        read += es.len();
    }
    let total = n.m(A).last_index().0;
    println!(
        "B0-30 in memory: {} hourly entries over 30 days, cap {CAP} bytes (largest entry {largest}); peak stored log \
         {peak} bytes; {} archive ranges in two buckets, {read} entries read back with intact digests; {truncations} \
         member truncations",
        30 * 24,
        archived.len()
    );
    // Before the first snapshot (day one) nothing archives; 24 hourly entries are under the cap.
    assert!(peak <= CAP + largest, "a member's stored log reached {peak} bytes against {CAP}");
    assert!(archived.len() > 10);
    // The archives cover every entry from the first through the last range, with no gap.
    assert!(archived.windows(2).all(|w| w[1].0.0 == w[0].1.0 + 1));
    assert_eq!(archived[0].0, Index(1));
    assert!(read as u64 + n.nodes[&A].disk.log.len() as u64 >= total - 1);
    // C, offline throughout, rejoins from the newest snapshot at or below the archive boundary and the archives after
    // it, read from the first bucket.
    let through = archived.last().unwrap().1;
    let snap = *snaps.iter().filter(|s| **s < through).max().expect("a snapshot below the boundary");
    let keys: Vec<Box<str>> =
        archived.iter().filter(|a| a.1 > snap).map(|a| format!("b0/{}", a.2).into_boxed_str()).collect();
    let tail: Vec<LogEntry<Ev>> = codec::from_bytes(&n.r2[&format!("b0/{}", archived.last().unwrap().2)]).unwrap();
    let at_through = tail.last().unwrap();
    assert_eq!(at_through.index, through);
    let last =
        graftyard_raft::LogBase { index: through, term: at_through.term, digest: graftyard_raft::digest(at_through) };
    let src = InstallSource {
        core: format!("snap/Core/{}", snap.0).into(),
        metadata: Some(format!("snap/Metadata/{}", snap.0).into()),
        archives: SmallVec::from_iter(keys),
        through,
        through_term: last.term,
        through_digest: last.digest,
        config: n.m(A).committed_config().cloned().unwrap(),
        id_block: n.m(A).group_fold().id_block(),
    };
    n.nodes.get_mut(&A).unwrap().m.as_mut().unwrap().set_install(Some(src));
    n.restart(C);
    for _ in 0..20 {
        n.advance(CALL_DEADLINE.as_millis() as u64 + 1);
    }
    n.write(A, 2);
    assert!(n.installed.iter().any(|(_, s)| *s == C), "C installed from the archives");
    assert_converged(&n, A, &[B, C]);
}

/// "Follower read" (`design/DESIGN.md`): a follower that is behind never grants a read-index read from its older
/// fold. Cut off from the leader, it grants nothing and fails the read at CAMPAIGN_DEADLINE; once it can reach the
/// leader again, the read it is granted sees the write committed while it was away.
#[test]
fn a_follower_behind_grants_no_read_from_its_older_fold() {
    let mut n = Net::new();
    n.elect(A);
    n.commit(A, vec![Ev::Core(50, 1)]);
    n.write(A, 2);
    assert_eq!(n.m(C).group_fold().fold.get(50), Some(1), "C applied the first write");
    n.cut.insert(C);
    n.commit(A, vec![Ev::Core(50, 2)]);
    n.advance(1);
    assert_eq!(n.m(C).group_fold().fold.get(50), Some(1), "C is behind");
    let rid = n.on(C, |m, now| m.read_index(now)).expect("a follower reads");
    for _ in 0..4 {
        n.advance(CALL_DEADLINE.as_millis() as u64 / 2);
        assert!(!n.seen.contains_key(&(C, rid)), "no grant from C's older fold");
    }
    n.advance(CALL_DEADLINE.as_millis() as u64 * 4);
    let failed = n
        .resolutions
        .iter()
        .any(|(s, r)| *s == C && matches!(r, Resolution::Read { id, result: Err(_) } if *id == rid));
    assert!(failed, "the read failed instead of answering stale");
    // The failed read-index call made C forget A (F67): it names a leader again once A's next round reaches it.
    n.cut.clear();
    n.advance(CALL_DEADLINE.as_millis() as u64 + 1);
    let rid = n.on(C, |m, now| m.read_index(now)).expect("a follower reads");
    for _ in 0..4 {
        n.advance(CALL_DEADLINE.as_millis() as u64 / 2);
    }
    assert_eq!(n.seen.get(&(C, rid)), Some(&Some(2)), "once back, C's read sees the write made while it was away");
}
