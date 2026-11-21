//! A local CPU profile of a repository leader's commit path under a B0-32-shaped burst (`tests/am1/results/
//! lm1-rows-m3f.md`, "The leader's commit path, profiled"). Not a gate: it prices each part of what the leader's
//! isolate does per command, natively, so a hosted row's CPU-seconds can be compared against it.
//!
//! Three real `Member<RepoCmd>` voters and seven learners answered by stubs, on a simulated clock: one-way latency
//! LAT_MS, dependency reads DEP_MS, the edge's place of a staged pack STAGE_MS, the host's TICK. Beside the core's
//! inputs it runs what the site host does per input, as `workers/site/src/host.rs` and `member.rs` do it: the
//! `/commit` body's JSON decode and pricing, the log row and request row encodings (`SqlStore`), each AppendEntries'
//! `encode_entry` per entry per peer and the call body's codec (`send`, `call`), the reply render, the host's
//! linear scans of its admitted map (`note`, `release`) and `Member::outcome` per wait, `/outcome` past COMMIT_HOLD,
//! and one `/outcome_by_request` per pending ticket. SQLite statements and the Workers runtime are not run.
//!
//! `cargo test -p graftyard-git --release --test commit_profile -- --ignored --nocapture`, with `SHAPE=b033` for
//! B0-33's inline 10 to 70 KB packs, and `N`, `BURST_MS` to change the burst.

mod common;
use common::*;

use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;
use std::time::Instant;

use graftyard_git::group::{AdmitWire, ClassWire, PushReport};
use graftyard_git::pack::PackId;
use graftyard_raft::admit::AdmitError;
use graftyard_raft::{
    Acked, AppendRes, DepsWanted, EntryBody, Fold, GroupConfig, Index, Message, Out, Ready, Storage, Ticket, Write,
    codec, encode_entry,
};
use graftyard_types::{
    Member as MemberRef, MemberKind, Placement, PlacementChecks, Rtt, Site, SiteLink, SiteSet, SiteTable,
};

const DEP_MS: u64 = 100;
const STAGE_MS: u64 = 1_200;
const TICK_MS: u64 = 100;
/// m3fhs1's learner `/raft`: about 115 a second over seven learners.
const LEARNER_READ_MS: u64 = 60;
const LEARNERS: u8 = 7;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Decode,
    DecodeInline,
    Admit,
    Decide,
    DepDecode,
    AppendRes,
    Apply,
    SelfAck,
    Read,
    Tick,
    Store,
    PeerCount,
    PeerEncode,
    Render,
    Scan,
    OutcomeCall,
    ByRequest,
}

const PARTS: [(Part, &str); 17] = [
    (Part::DecodeInline, "/commit body decode + price, inline pack"),
    (Part::Decode, "/commit body decode + price, stored pack"),
    (Part::Admit, "commit_as (admission, cut)"),
    (Part::Decide, "deps_read (decide, entry build)"),
    (Part::DepDecode, "dep folds unhex + decode"),
    (Part::AppendRes, "AppendRes, no apply"),
    (Part::Apply, "AppendRes that applies (commit, apply, outcomes)"),
    (Part::SelfAck, "self_ack"),
    (Part::Read, "learner ReadIndex"),
    (Part::Tick, "tick"),
    (Part::Store, "log + request row encode, digest"),
    (Part::PeerCount, "per-peer encode_entry (send's byte count)"),
    (Part::PeerEncode, "per-peer call body codec (call)"),
    (Part::Render, "reply render (PushReport, JSON)"),
    (Part::Scan, "host scans: admitted find, outcome()"),
    (Part::OutcomeCall, "/outcome past COMMIT_HOLD"),
    (Part::ByRequest, "/outcome_by_request (decode + lookup)"),
];

const BUCKETS: [&str; 4] = ["<50", "50-149", "150-299", ">=300"];

fn bucket(open: usize) -> usize {
    match open {
        0..50 => 0,
        50..150 => 1,
        150..300 => 2,
        _ => 3,
    }
}

#[derive(Default)]
struct Prof {
    on: bool,
    ns: [[u64; 4]; 17],
    calls: [[u64; 4]; 17],
    by_sec: BTreeMap<u64, u64>,
    now: u64,
    open: usize,
}

impl Prof {
    fn add(&mut self, p: Part, t: Instant) {
        if !self.on {
            return;
        }
        let d = t.elapsed().as_nanos() as u64;
        let b = bucket(self.open);
        self.ns[p as usize][b] += d;
        self.calls[p as usize][b] += 1;
        *self.by_sec.entry(self.now / 1_000).or_default() += d;
    }
}

struct Null;
impl Storage<RepoEvent> for Null {
    fn write(&mut self, _: &Write<RepoEvent>) {}
}

#[derive(serde::Deserialize)]
struct Req {
    rid: String,
    cmd: RepoCmd,
    #[serde(default)]
    admit: Option<AdmitWire>,
}

#[allow(clippy::large_enum_variant)]
enum Ev {
    Send(usize, bool),
    Msg(Envelope<RepoEvent>),
    Deps(DepsWanted),
    Tick,
    LearnerRead(SiteId),
    Outcome(Ticket, usize),
}

struct Push {
    inline: Option<Vec<u8>>,
    stored: Vec<u8>,
}

#[derive(Default)]
struct Counts {
    stage: usize,
    full: usize,
    ok: usize,
    refused: usize,
    entries: usize,
    commands: usize,
    entry_bytes: usize,
    catchup: usize,
    outcome_calls: usize,
}

struct Sim {
    w: World,
    l: SiteId,
    now: u64,
    q: BTreeMap<(u64, u64), Ev>,
    seq: u64,
    deps: HashMap<ChainId, String>,
    learners: Vec<SiteId>,
    held: HashMap<SiteId, Index>,
    host: HashMap<Ticket, (RequestId, u64, Option<RepoCmd>)>,
    applied: Index,
    prof: Prof,
    pushes: Vec<Push>,
    first: HashMap<RequestId, u64>,
    lat: Vec<u64>,
    n: Counts,
    batch: u64,
    reads_until: u64,
    lat_ms: u64,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn render(a: &Acked<RepoOut>) -> serde_json::Value {
    let q = a.quorum();
    serde_json::json!({ "done": { "ok": {
        "index": a.index.0,
        "out": serde_json::to_value(PushReport::of(a)).unwrap(),
        "quorum": q.sites().iter().map(|s| s.0).collect::<Vec<_>>(),
        "ack": { "index": a.index.0, "acks": q.acks().iter()
            .map(|d| serde_json::json!({ "site": d.site.0, "durable": d.durable.0, "at": d.at.0 }))
            .collect::<Vec<_>>() },
    } } })
}

impl Sim {
    fn at(&mut self, delay: u64, ev: Ev) {
        self.seq += 1;
        self.q.insert((self.now + delay, self.seq), ev);
    }

    fn lead<R>(&mut self, p: Part, f: impl FnOnce(&mut Member<RepoCmd>, Timestamp) -> Ready<RepoCmd, R>) -> R {
        let i = self.l.0 as usize;
        self.prof.open = self.host.len();
        let t = Instant::now();
        let rd = f(&mut self.w.repo.members[i], Timestamp(self.now));
        let applies = rd.writes.iter().any(|w| matches!(w, Write::Apply(_)));
        self.prof.add(if p == Part::AppendRes && applies { Part::Apply } else { p }, t);
        self.host_writes(&rd.writes);
        let (r, out) = rd.persist(&mut Null);
        self.host_out(out);
        r
    }

    /// `SqlStore::write`'s encodings, then `note`'s scan of the admitted map per appended request.
    fn host_writes(&mut self, writes: &[Write<RepoEvent>]) {
        let m = &self.w.repo.members[self.l.0 as usize];
        let t = Instant::now();
        for w in writes {
            match w {
                Write::State(s) => {
                    black_box(codec::to_bytes(s));
                }
                Write::Append(e) => {
                    black_box((graftyard_raft::digest(e), codec::to_bytes(e)));
                }
                Write::Apply(to) => {
                    for idx in self.applied.0 + 1..=to.0 {
                        let Some(e) = m.entry_at(Index(idx)) else { continue };
                        let EntryBody::Events(evs) = &e.body else { continue };
                        for (_, r) in &e.requests {
                            let row: Vec<RepoEvent> =
                                evs[r.start as usize..r.end as usize].iter().map(RepoFold::row_event).collect();
                            black_box(codec::to_bytes(&row));
                        }
                    }
                    self.applied = self.applied.max(*to);
                }
                _ => {}
            }
        }
        self.prof.add(Part::Store, t);
        let t = Instant::now();
        let term = m.term();
        for w in writes {
            if let Write::Append(e) = w
                && e.term == term
                && matches!(e.body, EntryBody::Events(_))
            {
                if self.prof.on {
                    self.n.entries += 1;
                    self.n.commands += e.requests.len();
                    self.n.entry_bytes += encode_entry(e).len();
                }
                for (rid, _) in &e.requests {
                    black_box(self.host.values().find(|(r, _, _)| r == rid));
                }
            }
        }
        self.prof.add(Part::Scan, t);
    }

    fn host_out(&mut self, mut out: Out<RepoCmd>) {
        assert!(out.alarms.is_empty(), "alarm: {:?}", out.alarms);
        let t = Instant::now();
        for (_, rid, cmd) in out.refused.drain(..) {
            if let Some(a) = self.host.values_mut().find(|(r, _, _)| *r == rid) {
                a.2 = Some(cmd);
            }
        }
        self.prof.add(Part::Scan, t);
        for r in out.resolutions.drain(..) {
            let Resolution::Ticket { ticket, result } = r else { continue };
            let admitted = self.host.remove(&ticket);
            match result {
                Ok(a) => {
                    let t = Instant::now();
                    black_box(serde_json::to_vec(&render(&a)).unwrap());
                    self.prof.add(Part::Render, t);
                    if let Some((rid, _, _)) = admitted {
                        self.n.ok += 1;
                        self.lat.push(self.now - self.first[&rid]);
                    }
                }
                Err(e) => {
                    if admitted.is_some() {
                        self.n.refused += 1;
                        if self.n.refused == 1 {
                            eprintln!("first refusal: {e:?}");
                        }
                    }
                }
            }
        }
        let msgs: Vec<_> = out.early.drain(..).chain(out.messages.drain(..)).collect();
        for e in &msgs {
            let t = Instant::now();
            if let Message::Append(a) = &e.msg {
                black_box(a.entries.iter().map(|x| encode_entry(x).len()).sum::<usize>());
            }
            self.prof.add(Part::PeerCount, t);
            let t = Instant::now();
            black_box(codec::to_bytes(std::slice::from_ref(e)));
            self.prof.add(Part::PeerEncode, t);
        }
        for e in msgs {
            self.at(self.lat_ms, Ev::Msg(e));
        }
        for d in out.deps.drain(..) {
            self.at(DEP_MS, Ev::Deps(d));
        }
        self.n.catchup += out.catchup.len();
        if let Some(ack) = out.self_ack.take() {
            self.lead(Part::SelfAck, |m, t| m.self_ack(t, ack));
        }
    }

    fn follow(&mut self, s: SiteId, f: impl FnOnce(&mut Member<RepoCmd>, Timestamp) -> Ready<RepoCmd, ()>) {
        let i = s.0 as usize;
        let now = Timestamp(self.now);
        let ((), mut out) = f(&mut self.w.repo.members[i], now).persist(&mut self.w.repo.disks[i]);
        loop {
            assert!(out.alarms.is_empty(), "alarm on {s:?}: {:?}", out.alarms);
            for e in out.early.drain(..).chain(out.messages.drain(..)) {
                self.at(self.lat_ms, Ev::Msg(e));
            }
            let Some(ack) = out.self_ack.take() else { break };
            out = self.w.repo.members[i].self_ack(now, ack).persist(&mut self.w.repo.disks[i]).1;
        }
    }

    fn deliver(&mut self, e: Envelope<RepoEvent>) {
        let Envelope { from, to, msg } = e;
        if to == self.l {
            match msg {
                Message::AppendRes(r) => {
                    let round = r.round;
                    self.lead(Part::AppendRes, |m, t| m.step(t, from, Message::AppendRes(r)));
                    self.lead(Part::AppendRes, |m, t| m.call_ended(t, from, round));
                }
                Message::ReadIndex { .. } => self.lead(Part::Read, |m, t| m.step(t, from, msg)),
                msg => self.lead(Part::AppendRes, |m, t| m.step(t, from, msg)),
            }
        } else if self.learners.contains(&to) {
            // A learner that stores any call chained to what it holds, as `scenarios.rs`'s `Silent` does.
            let Message::Append(a) = &msg else { return };
            let held = self.held.entry(to).or_insert(Index(0));
            let ok = a.prev_index <= *held;
            if ok {
                *held = (*held).max(Index(a.prev_index.0 + a.entries.len() as u64));
            }
            let res = AppendRes {
                term: a.term,
                boot: a.boot,
                round: a.round,
                sent_at: a.sent_at,
                prev_index: a.prev_index,
                ok,
                matched: if ok { Index(a.prev_index.0 + a.entries.len() as u64) } else { Index(0) },
                last: *held,
                promise: false,
                refused: None,
                stepped_down_at: None,
                boots: [Timestamp(0); 2],
                release: config(ChainId::Repo(REPO), SITES[0], Vec::new()).release,
            };
            self.at(self.lat_ms, Ev::Msg(Envelope { from: to, to: from, msg: Message::AppendRes(res) }));
        } else {
            self.follow(to, |m, t| m.step(t, from, msg));
        }
    }

    fn step(&mut self, ev: Ev) {
        match ev {
            Ev::Msg(e) => self.deliver(e),
            Ev::Tick => {
                self.lead(Part::Tick, |m, t| m.tick(t));
                let l = self.l;
                for s in SITES.into_iter().filter(|s| *s != l) {
                    self.follow(s, |m, t| m.tick(t));
                }
                self.at(TICK_MS, Ev::Tick);
            }
            Ev::LearnerRead(s) => {
                if self.now < self.reads_until {
                    self.batch += 1;
                    let batch = self.batch;
                    self.lead(Part::Read, |m, t| m.step(t, s, Message::ReadIndex { batch }));
                    self.at(LEARNER_READ_MS, Ev::LearnerRead(s));
                }
            }
            Ev::Deps(want) => {
                // What `read_deps` decodes: each chain's fold as hex of its codec bytes.
                let mut hexes = Vec::new();
                for c in &want.chains {
                    if !self.deps.contains_key(c) {
                        let install = self.w.install.fresh(self.w.now);
                        let b = match self.w.ident.dep(*c, &install) {
                            IdentityDep::Names(f) => codec::to_bytes(&f),
                            IdentityDep::Tokens(f) => codec::to_bytes(&f),
                            IdentityDep::Authz(f) => codec::to_bytes(&f),
                        };
                        self.deps.insert(*c, hex(&b));
                    }
                    hexes.push((*c, self.deps[c].clone()));
                }
                self.prof.open = self.host.len();
                let t = Instant::now();
                let deps: Vec<(ChainId, IdentityDep)> = hexes
                    .iter()
                    .map(|(c, h)| {
                        let b = unhex(h);
                        let d = match c {
                            ChainId::Names(_) => IdentityDep::Names(codec::from_bytes(&b).unwrap()),
                            ChainId::Tokens(_) => IdentityDep::Tokens(codec::from_bytes(&b).unwrap()),
                            _ => IdentityDep::Authz(codec::from_bytes(&b).unwrap()),
                        };
                        (*c, d)
                    })
                    .collect();
                self.prof.add(Part::DepDecode, t);
                self.lead(Part::Decide, |m, t| m.deps_read(t, want.id, deps));
            }
            Ev::Send(i, inline) => {
                let p = &self.pushes[i];
                let body = if inline { p.inline.clone().expect("an inline body") } else { p.stored.clone() };
                self.prof.open = self.host.len();
                let t = Instant::now();
                let req: Req = serde_json::from_slice(&body).unwrap();
                let rid: RequestId = req.rid.parse().unwrap();
                black_box(req.cmd.stored_len());
                let marked = req.cmd.marked();
                self.prof.add(if inline { Part::DecodeInline } else { Part::Decode }, t);
                self.first.entry(rid).or_insert(self.now);
                let a = req.admit.expect("an agent admission").admit();
                let cmd = req.cmd;
                match self.lead(Part::Admit, |m, t| m.commit_as(t, rid, cmd, a)) {
                    Ok(ad) => {
                        let t = Instant::now();
                        black_box(self.w.repo.members[self.l.0 as usize].outcome(ad.ticket));
                        self.prof.add(Part::Scan, t);
                        self.host.entry(ad.ticket).or_insert((rid, self.now, marked));
                        self.at(graftyard_raft::COMMIT_HOLD.as_millis() as u64, Ev::Outcome(ad.ticket, i));
                    }
                    Err(AdmitError::Stage) => {
                        self.n.stage += 1;
                        self.at(STAGE_MS, Ev::Send(i, false));
                    }
                    Err(AdmitError::Full { .. }) => self.n.full += 1,
                    Err(e) => panic!("{e:?}"),
                }
            }
            Ev::Outcome(ticket, i) => {
                if !self.host.contains_key(&ticket) {
                    return;
                }
                self.n.outcome_calls += 1;
                let m = &self.w.repo.members[self.l.0 as usize];
                self.prof.open = self.host.len();
                let t = Instant::now();
                // `/outcome`: `Member::outcome`, then `wait_ticket`'s own `outcome` before it parks a waiter.
                black_box((m.outcome(ticket), m.outcome(ticket)));
                self.prof.add(Part::OutcomeCall, t);
                let t = Instant::now();
                let req: Req = serde_json::from_slice(&self.pushes[i].stored).unwrap();
                let rid: RequestId = req.rid.parse().unwrap();
                black_box(m.outcome_by_request(rid, &req.cmd).is_ok());
                self.prof.add(Part::ByRequest, t);
            }
        }
    }

    fn run_until(&mut self, end: u64, done: impl Fn(&Sim) -> bool) {
        while let Some((&(t, _), _)) = self.q.first_key_value() {
            if t > end || done(self) {
                return;
            }
            let ((t, _), ev) = self.q.pop_first().unwrap();
            self.now = t;
            self.prof.now = t;
            self.step(ev);
        }
    }

    fn add_learners(&mut self) {
        let m = &self.w.repo.members[self.l.0 as usize];
        let Some((_, GroupConfig::Normal(p))) = m.committed_config().cloned() else { panic!("normal config") };
        let site = |id: SiteId, region| Site {
            id,
            script: String::new(),
            account: "x".into(),
            region,
            jurisdiction: None,
            bucket: String::new(),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare: false,
        };
        let table =
            SiteTable(vec![site(SITES[0], Region::Enam), site(SITES[1], Region::Weur), site(SITES[2], Region::Apac)]);
        let rtt: Vec<(SiteId, Rtt)> = [(0, 1), (0, 2), (1, 2)]
            .iter()
            .map(|&(x, y)| (SITES[x], Rtt { peer: SITES[y], median_ms: 80, at: Timestamp(0) }))
            .collect();
        let checks = PlacementChecks { sites: &table, separate_accounts: false, rtt: &rtt, near: &[] };
        let learners: Vec<MemberRef> = (0..LEARNERS)
            .map(|k| {
                MemberRef::new(
                    ChainId::Repo(REPO),
                    SITES[usize::from(k % 3)],
                    MemberKind::Learner,
                    k,
                    p.next_generation(),
                )
            })
            .collect();
        self.learners = learners.iter().map(MemberRef::addr).collect();
        let np =
            Placement::new(p.voters().iter().cloned().collect(), learners.into_iter().collect(), p.home(), &checks)
                .unwrap();
        self.lead(Part::Tick, |m, t| m.reconfigure(t, GroupConfig::Normal(Box::new(np)))).unwrap();
    }
}

fn env(k: &str, d: u64) -> u64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

#[test]
#[ignore = "profile: SHAPE=b032|b033, N, BURST_MS"]
fn the_commit_path_under_a_burst() {
    let shape = std::env::var("SHAPE").unwrap_or_else(|_| "b032".into());
    let n = env("N", 500) as usize;
    let burst = env("BURST_MS", 4_000);
    let mut w = World::new();
    let l = w.repo.leader();
    w.repo.members[l.0 as usize].set_log_window_bytes(graftyard_raft::LOG_WINDOW, graftyard_raft::LOG_BYTES);
    let applied = w.repo.members[l.0 as usize].group_fold().applied();
    // Each push's `/commit` bodies as the edge sends them: B0-32's 256 KiB packs, half sent inline first (Stage,
    // then the stored resend) and half stored first; B0-33's 10 to 70 KB packs, all inline first.
    let mut pushes = Vec::new();
    for i in 0..n {
        let rid = w.rid();
        let size = if shape == "b033" { 10_000 + (i * 7_919) % 60_000 } else { 256 << 10 };
        let p = push(app(CLAUDE), vec![up(&format!("refs/heads/b{i}"), None, Some(oid(10_000 + i as u32)))], &[]);
        let body = |pack: PackRef, inline: usize| {
            let mut p = p.clone();
            p.pack = Some(pack);
            let admit = AdmitWire {
                class: ClassWire::Agent,
                actor: Some(1 << 63 | 3),
                push: true,
                inline,
                placement_ok: true,
                budget_ms: None,
            };
            serde_json::to_vec(&serde_json::json!({ "rid": rid.to_string(), "cmd": RepoCmd::Push(p), "admit": admit }))
                .unwrap()
        };
        let pack: Vec<u8> = (0..size).map(|b| (b * 31 + i) as u8).collect();
        let stored = PackRef::Stored {
            id: PackId(Ulid(i as u128 + 1)),
            checksum: Digest([1; 32]),
            bytes: size as u64,
            sites: SiteSet(3),
        };
        let inline_first = shape == "b033" || i % 2 == 0;
        pushes.push(Push { inline: inline_first.then(|| body(PackRef::Inline(pack), size)), stored: body(stored, 0) });
    }
    let start = w.now.0;
    let mut s = Sim {
        w,
        l,
        now: start,
        q: BTreeMap::new(),
        seq: 0,
        deps: HashMap::new(),
        learners: Vec::new(),
        held: HashMap::new(),
        host: HashMap::new(),
        applied,
        prof: Prof::default(),
        pushes,
        first: HashMap::new(),
        lat: Vec::new(),
        n: Counts::default(),
        batch: 0,
        reads_until: u64::MAX,
        lat_ms: env("LAT_MS", 50),
    };
    // The learners join and catch up, with ticks and their reads running, before the burst.
    s.add_learners();
    s.at(0, Ev::Tick);
    for e in std::mem::take(&mut s.w.repo.inbox) {
        s.at(0, Ev::Msg(e));
    }
    for k in 0..s.learners.len() {
        let a = s.learners[k];
        s.at(k as u64 * 9, Ev::LearnerRead(a));
    }
    s.run_until(start + 3_000, |_| false);
    assert!(s.learners.iter().all(|a| s.held.get(a).is_some_and(|i| i.0 > 0)), "learners caught up: {:?}", s.held);

    s.prof.on = true;
    let t0 = s.now;
    for i in 0..n {
        let inline = s.pushes[i].inline.is_some();
        s.at((i as u64 * burst) / n as u64, Ev::Send(i, inline));
    }
    let wall = Instant::now();
    s.run_until(t0 + 120_000, |s| s.n.ok + s.n.full + s.n.refused >= n);
    let wall = wall.elapsed();
    let sim_ms = s.now - t0;

    let total: u64 = s.prof.ns.iter().flatten().sum();
    let ok = s.n.ok.max(1) as f64;
    println!("\n## {shape}: {n} agent pushes over {burst} ms, 3 voters + {LEARNERS} learners (native, --release)\n");
    println!(
        "ok {}, refused {}, full {}, Stage answers {}, /outcome calls {}; {} entries, {:.1} commands an entry, \
         {:.0} B an entry (canonical); catchup asks {}",
        s.n.ok,
        s.n.refused,
        s.n.full,
        s.n.stage,
        s.n.outcome_calls,
        s.n.entries,
        s.n.commands as f64 / s.n.entries.max(1) as f64,
        s.n.entry_bytes as f64 / s.n.entries.max(1) as f64,
        s.n.catchup
    );
    s.lat.sort_unstable();
    let pct = |p: usize| s.lat.get(s.lat.len() * p / 100).copied().unwrap_or(0);
    println!(
        "simulated span {sim_ms} ms; ack latency on the simulated clock p50 {} p99 {} ms; profiled CPU {:.1} ms \
         (process wall {:.0} ms)",
        pct(50),
        pct(99),
        total as f64 / 1e6,
        wall.as_secs_f64() * 1e3
    );
    println!(
        "\n| Part | ms | share | µs per ok command | calls | µs per call by commands open: {} |",
        BUCKETS.join(" / ")
    );
    println!("|---|---|---|---|---|---|");
    for (p, name) in PARTS {
        let ns: u64 = s.prof.ns[p as usize].iter().sum();
        let calls: u64 = s.prof.calls[p as usize].iter().sum();
        let per: Vec<String> = (0..4)
            .map(|b| {
                let c = s.prof.calls[p as usize][b];
                if c == 0 { "-".into() } else { format!("{:.1}", s.prof.ns[p as usize][b] as f64 / c as f64 / 1e3) }
            })
            .collect();
        println!(
            "| {name} | {:.2} | {:.1}% | {:.1} | {calls} | {} |",
            ns as f64 / 1e6,
            100.0 * ns as f64 / total.max(1) as f64,
            ns as f64 / ok / 1e3,
            per.join(" / ")
        );
    }
    println!("| **total** | {:.2} | 100% | {:.1} | | |", total as f64 / 1e6, total as f64 / ok / 1e3);
    let secs: Vec<String> =
        s.prof.by_sec.iter().map(|(sec, ns)| format!("{}:{:.1}", sec - t0 / 1_000, *ns as f64 / 1e6)).collect();
    println!("\nCPU ms per simulated second (second:ms): {}", secs.join(", "));
}
