//! A timed load net for the admission rows (B0-29, B0-31, B0-32, B0-33 in memory): three real members, messages
//! delivered after a one-way delay per link plus a per-byte cost, storage that confirms at once, and edge clients that
//! send each command to the leader under admission, stage a pack when asked, and give up on `Full`.
//!
//! The network is a model, not Cloudflare: one-way delays from `Tri`'s sites (enam, weur, apac) and the M0-WP2 echo
//! cost of 0.3 ms per KB [measured, `tests/m0/results/m0-wp2.md`, B0-01: 96 ms empty, 533 ms at 1.5 MB]. So the latency
//! numbers here measure admission's queueing and entry sizes on top of that model.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use graftyard_raft::admit::{ActorKey, Admit, AdmitClass, AdmitError};
use graftyard_raft::{
    CommitError, EntryBody, Envelope, Index, Member, Message, Ready, RequestId, Resolution, Role, Stored, Ticket, Ulid,
};
use graftyard_types::{SiteId, Timestamp};

use super::kv::{Kv, KvCmd, KvEvent};
use super::sim::{Rng, SITES, config};

/// One-way delay in ms between sites 0 (enam), 1 (weur), and 2 (apac) [guess, from public RTTs].
const ONE_WAY: [[u64; 3]; 3] = [[0, 40, 100], [40, 0, 110], [100, 110, 0]];
/// Per byte of AppendEntries, in microseconds: 0.3 ms per KB.
const US_PER_BYTE: f64 = 0.3;
/// The edge's R2 PUT of a staged pack [measured, `tests/m0/results/m0-wp9.md`, B0-32: stage p50 451 to 506 ms].
pub const STAGE_MS: u64 = 500;
const TICK_MS: u64 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Push,
    Merge,
    Side,
}

/// One client request: sent at `at` with the edge's classification.
#[derive(Clone, Debug)]
pub struct Op {
    pub at: u64,
    pub class: AdmitClass,
    pub actor: Option<u64>,
    pub kind: Kind,
    pub key: String,
    /// Inline pack bytes when first sent; 0 once staged.
    pub pack: usize,
    /// For a push, the ref's value it expects.
    pub expect: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum End {
    Acked {
        index: u64,
    },
    Stale,
    /// Refused by admission with this retry hint, at once (`commit_as`) or after AGENT_WAIT (`CommitError::Full`).
    Full {
        retry_after_s: u32,
        waited: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Done {
    pub op: Op,
    pub end: End,
    /// From the first send to the answer, ms, staging included.
    pub ms: u64,
    pub staged: bool,
    /// The class the leader admitted it under.
    pub admitted: Option<AdmitClass>,
    /// The leader's last index when it was admitted, and its commit index: for "in one of the next 2 entries".
    pub last_at_admit: u64,
}

struct Live {
    op: Op,
    /// The inline pack bytes it is sent with now: `op.pack`, then 0 once staged.
    pack: usize,
    rid: RequestId,
    first: u64,
    staged: bool,
    admitted: Option<AdmitClass>,
    last_at_admit: u64,
    ticket: Option<(SiteId, Ticket)>,
}

pub struct Load {
    pub now: u64,
    pub members: Vec<Member<KvCmd>>,
    disks: Vec<Stored<Kv>>,
    wire: BTreeMap<(u64, u64), Envelope<KvEvent>>,
    seq: u64,
    rng: Rng,
    pending: Vec<(u64, Op)>,
    live: BTreeMap<u64, Live>,
    next_live: u64,
    /// Ops to (re)send at a time: (time, live id).
    resend: BTreeSet<(u64, u64)>,
    pub done: Vec<Done>,
    /// Leaders seen, the first at setup: (time, site, term). `changes` counts the rest.
    pub leaders: Vec<(u64, SiteId, u64)>,
    /// Every `pressure`/queue sample of the leader, once a second: (time, agent queued, agent bytes).
    pub samples: Vec<(u64, usize, usize)>,
    /// Entries the leader cut: (index, commands, bytes, inline agent bytes is not visible; inline bytes).
    pub entries: Vec<(u64, usize, usize)>,
    /// Placement fails while this holds; the edge sends `placement_ok: false`.
    pub placement_ok: bool,
    /// Every alarm a member raised: (time, site, alarm).
    pub alarms: Vec<(u64, SiteId, String)>,
    pub fixed_capacity: Option<f32>,
}

fn event_bytes(e: &KvEvent) -> usize {
    match e {
        KvEvent::Write { pack, key, value, .. } => pack + key.len() + value.len() + 24,
        _ => 64,
    }
}

fn msg_bytes(m: &Message<KvEvent>) -> usize {
    match m {
        Message::Append(a) => {
            a.entries
                .iter()
                .map(|e| match &e.body {
                    EntryBody::Events(evs) => evs.iter().map(event_bytes).sum::<usize>() + 120,
                    _ => 120,
                })
                .sum::<usize>()
                + 100
        }
        _ => 100,
    }
}

impl Load {
    /// Three members, `leader` elected and serving.
    pub fn new(seed: u64, leader: SiteId) -> Load {
        let mut l = Load {
            now: 10_000,
            // The placement's home is the leader's site, so the home-return transfer keeps it there.
            members: SITES
                .iter()
                .map(|s| {
                    let mut c = config(*s);
                    c.record.home = leader;
                    Member::new(c, seed + u64::from(s.0)).unwrap()
                })
                .collect(),
            disks: SITES.iter().map(|_| Stored::new(Kv::default())).collect(),
            wire: BTreeMap::new(),
            seq: 0,
            rng: Rng::new(seed),
            pending: Vec::new(),
            live: BTreeMap::new(),
            next_live: 0,
            resend: BTreeSet::new(),
            done: Vec::new(),
            leaders: Vec::new(),
            samples: Vec::new(),
            entries: Vec::new(),
            placement_ok: true,
            alarms: Vec::new(),
            fixed_capacity: None,
        };
        l.on(leader, |m, now| m.campaign(now));
        for _ in 0..400 {
            l.step();
            if l.members[leader.0 as usize].role() == Role::Leader && l.members[leader.0 as usize].admission().is_some()
            {
                break;
            }
        }
        // Serve: one command through, so the term's `Noop` committed and the leader is ready.
        let rid = RequestId(Ulid(u128::MAX));
        let warm = KvCmd::Put { key: "warm".into(), value: "1".into() };
        l.on(leader, |m, now| m.commit(now, rid, warm)).expect("the new leader admits");
        l.run_for(2_000);
        assert_eq!(l.leader(), Some(leader), "{leader:?} leads");
        let term = l.members[leader.0 as usize].term().0;
        l.leaders = vec![(l.now, leader, term)];
        l
    }

    pub fn leader(&self) -> Option<SiteId> {
        SITES
            .iter()
            .copied()
            .filter(|s| self.members[s.0 as usize].role() == Role::Leader)
            .max_by_key(|s| self.members[s.0 as usize].term())
    }

    pub fn set_capacity(&mut self, cap: Option<f32>) {
        self.fixed_capacity = cap;
        for m in &mut self.members {
            m.set_admit_capacity(cap);
        }
    }

    fn on<R>(&mut self, s: SiteId, f: impl FnOnce(&mut Member<KvCmd>, Timestamp) -> Ready<KvCmd, R>) -> R {
        let now = Timestamp(self.now);
        let i = s.0 as usize;
        let before = self.members[i].last_index();
        let (r, mut out) = f(&mut self.members[i], now).persist(&mut self.disks[i]);
        loop {
            for e in out.early.drain(..).chain(out.messages.drain(..)) {
                let bytes = msg_bytes(&e.msg);
                let jitter = self.rng.below(20);
                let at = self.now
                    + ONE_WAY[e.from.0 as usize][e.to.0 as usize]
                    + (bytes as f64 * US_PER_BYTE / 1000.0) as u64
                    + jitter;
                self.seq += 1;
                self.wire.insert((at, self.seq), e);
            }
            for a in out.alarms.drain(..) {
                if std::env::var("AM1_TRACE").is_ok() {
                    eprintln!("{} {s:?} alarm {a:?}", self.now);
                }
                self.alarms.push((self.now, s, format!("{a:?}")));
            }
            for r in out.resolutions.drain(..) {
                self.resolved(s, r);
            }
            let Some(ack) = out.self_ack.take() else { break };
            out = self.members[i].self_ack(now, ack).persist(&mut self.disks[i]).1;
        }
        let m = &self.members[i];
        if m.role() == Role::Leader && m.last_index() > before {
            for k in before.0 + 1..=m.last_index().0 {
                if let Some(e) = m.log().iter().find(|e| e.index.0 == k)
                    && let EntryBody::Events(evs) = &e.body
                {
                    self.entries.push((k, e.requests.len(), evs.iter().map(event_bytes).sum()));
                }
            }
        }
        r
    }

    fn resolved(&mut self, s: SiteId, r: Resolution<KvCmd>) {
        let Resolution::Ticket { ticket, result } = r else { return };
        let Some((&id, _)) = self.live.iter().find(|(_, l)| l.ticket == Some((s, ticket))) else { return };
        match result {
            Ok(a) => self.finish(id, End::Acked { index: a.index.0 }),
            Err(CommitError::Refused { .. }) => self.finish(id, End::Stale),
            Err(CommitError::Full { retry_after_s }) => self.finish(id, End::Full { retry_after_s, waited: true }),
            Err(CommitError::NotLeader(_)) => {
                let l = self.live.get_mut(&id).expect("live");
                l.ticket = None;
                self.resend.insert((self.now + 50, id));
            }
        }
    }

    fn finish(&mut self, id: u64, end: End) {
        let l = self.live.remove(&id).expect("live");
        self.done.push(Done {
            ms: self.now - l.first,
            op: l.op,
            end,
            staged: l.staged,
            admitted: l.admitted,
            last_at_admit: l.last_at_admit,
        });
    }

    /// Queues `ops`, each sent at its `at`.
    pub fn offer(&mut self, ops: impl IntoIterator<Item = Op>) {
        self.pending.extend(ops.into_iter().map(|o| (o.at, o)));
        self.pending.sort_by_key(|p| std::cmp::Reverse(p.0));
    }

    fn send(&mut self, id: u64) {
        let Some(to) = self.leader() else {
            self.resend.insert((self.now + 100, id));
            return;
        };
        let l = self.live.get(&id).expect("live");
        let op = &l.op;
        let cmd = KvCmd::Write {
            key: op.key.clone(),
            expect: op.expect.clone(),
            value: format!("{}", l.rid.0.0),
            actor: op.actor,
            push: op.kind != Kind::Side,
            pack: l.pack,
        };
        let a = Admit {
            class: op.class,
            actor: op.actor.map(ActorKey),
            push: op.kind != Kind::Side,
            inline: l.pack,
            placement_ok: self.placement_ok,
        };
        let rid = l.rid;
        let last = self.members[to.0 as usize].last_index().0;
        match self.on(to, |m, now| m.commit_as(now, rid, cmd, a)) {
            Ok(adm) => {
                let l = self.live.get_mut(&id).expect("live");
                l.ticket = Some((to, adm.ticket));
                l.admitted = Some(adm.class);
                l.last_at_admit = last;
                if let Some(o) = self.members[to.0 as usize].outcome(adm.ticket) {
                    self.resolved(to, Resolution::Ticket { ticket: adm.ticket, result: o });
                }
            }
            Err(AdmitError::Stage) => {
                let l = self.live.get_mut(&id).expect("live");
                assert!(!l.staged, "a staged command was asked to stage again: {:?}", l.op);
                l.staged = true;
                l.pack = 0;
                self.resend.insert((self.now + STAGE_MS, id));
            }
            Err(AdmitError::Full { retry_after_s }) => self.finish(id, End::Full { retry_after_s, waited: false }),
            Err(AdmitError::Call(_)) => {
                self.resend.insert((self.now + 50, id));
            }
        }
    }

    /// One 5 ms step: due messages, due sends, and a tick every TICK_MS.
    pub fn step(&mut self) {
        self.now += 5;
        while let Some((&(at, k), _)) = self.wire.first_key_value() {
            if at > self.now {
                break;
            }
            let e = self.wire.remove(&(at, k)).expect("first");
            let Envelope { from, to, msg } = e;
            self.on(to, |m, now| m.step(now, from, msg));
        }
        while self.pending.last().is_some_and(|p| p.0 <= self.now) {
            let (_, op) = self.pending.pop().expect("last");
            self.next_live += 1;
            let id = self.next_live;
            let rid = RequestId(Ulid(u128::from(id) << 16));
            self.live.insert(
                id,
                Live {
                    pack: op.pack,
                    op,
                    rid,
                    first: self.now,
                    staged: false,
                    admitted: None,
                    last_at_admit: 0,
                    ticket: None,
                },
            );
            self.send(id);
        }
        while let Some(&(at, id)) = self.resend.first() {
            if at > self.now {
                break;
            }
            self.resend.pop_first();
            if self.live.contains_key(&id) {
                self.send(id);
            }
        }
        if self.now.is_multiple_of(TICK_MS) {
            for s in SITES {
                self.on(s, |m, now| m.tick(now));
            }
            if let Some(l) = self.leader() {
                let term = self.members[l.0 as usize].term().0;
                if self.leaders.last().is_none_or(|x| x.2 != term) {
                    self.leaders.push((self.now, l, term));
                }
            }
        }
        if self.now.is_multiple_of(1000)
            && let Some(l) = self.leader()
            && let Some(v) = self.members[l.0 as usize].admission()
        {
            self.samples.push((self.now, v.agent_queued, v.agent_bytes));
        }
    }

    pub fn run_for(&mut self, ms: u64) {
        let end = self.now + ms;
        while self.now < end {
            self.step();
        }
    }

    /// Runs until every offered op ended, or `max_ms` more passed.
    pub fn drain(&mut self, max_ms: u64) {
        let end = self.now + max_ms;
        while self.now < end && (!self.pending.is_empty() || !self.live.is_empty()) {
            self.step();
        }
    }

    pub fn changes(&self) -> usize {
        self.leaders.len() - 1
    }

    /// Starts a leadership transfer from the current leader to `to`.
    pub fn transfer(&mut self, to: SiteId) {
        let l = self.leader().expect("a leader");
        self.on(l, |m, now| m.transfer(now, to)).expect("transfer starts");
    }

    pub fn commit_index(&self) -> Index {
        self.leader().map_or(Index(0), |l| self.members[l.0 as usize].state().commit)
    }
}

/// The p-th percentile (0 to 100) of `xs`, nearest rank.
pub fn pct(xs: &mut [u64], p: f64) -> u64 {
    if xs.is_empty() {
        return 0;
    }
    xs.sort_unstable();
    let k = ((p / 100.0) * xs.len() as f64).ceil().max(1.0) as usize - 1;
    xs[k.min(xs.len() - 1)]
}

/// The latencies of acknowledged ops matching `f`.
pub fn acked_ms(done: &[Done], f: impl Fn(&Done) -> bool) -> Vec<u64> {
    done.iter().filter(|d| matches!(d.end, End::Acked { .. }) && f(d)).map(|d| d.ms).collect()
}
