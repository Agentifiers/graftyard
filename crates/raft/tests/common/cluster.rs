//! A hand-driven three-member group for targeted scenarios: the test decides which messages arrive and when.

#![allow(dead_code)]

use graftyard_raft::{DepsUsed, DepsWanted, Envelope, Member, Ready, Resolution, Role, Stored, Ticket};
use graftyard_types::{SiteId, Timestamp};

use super::kv::{Kv, KvCmd, KvEvent};
use super::sim::{SITES, config};

/// Storage that loses every write: the object reset before they were durable.
struct Discard;

impl graftyard_raft::Storage<KvEvent> for Discard {
    fn write(&mut self, _: &graftyard_raft::Write<KvEvent>) {}
}

/// An input whose writes are not yet durable (`Cluster::on_unconfirmed`).
pub struct Unconfirmed {
    pub site: SiteId,
    pub writes: Vec<graftyard_raft::Write<KvEvent>>,
    pub out: graftyard_raft::Out<KvCmd>,
}

pub struct Cluster {
    pub now: Timestamp,
    pub members: Vec<Option<Member<KvCmd>>>,
    pub disks: Vec<Stored<Kv>>,
    pub inbox: Vec<Envelope<KvEvent>>,
    pub resolutions: Vec<(SiteId, Resolution<KvCmd>)>,
    /// Every `Out::refused` released: (site, entry index, request id).
    pub refused: Vec<(SiteId, graftyard_raft::Index, graftyard_raft::RequestId)>,
    /// Every dependency read asked for, and every entry decided on reads, with the site and time.
    pub deps: Vec<(SiteId, Timestamp, DepsWanted)>,
    pub deps_used: Vec<(SiteId, DepsUsed)>,
    boots: u64,
    alarms: Vec<(SiteId, graftyard_raft::Alarm)>,
}

impl Cluster {
    pub fn new(seed: u64) -> Cluster {
        Cluster {
            now: Timestamp(10_000),
            members: SITES.iter().map(|s| Some(Member::new(config(*s), seed + u64::from(s.0)).unwrap())).collect(),
            disks: SITES.iter().map(|_| Stored::new(Kv::default())).collect(),
            inbox: Vec::new(),
            resolutions: Vec::new(),
            refused: Vec::new(),
            deps: Vec::new(),
            deps_used: Vec::new(),
            boots: seed * 1000,
            alarms: Vec::new(),
        }
    }

    pub fn m(&self, s: SiteId) -> &Member<KvCmd> {
        self.members[s.0 as usize].as_ref().expect("member is up")
    }

    /// The member at `s`, for a host setting such as `set_clock_suspect`.
    pub fn m_mut(&mut self, s: SiteId) -> &mut Member<KvCmd> {
        self.members[s.0 as usize].as_mut().expect("member is up")
    }

    /// Runs one input on `s`, persists it, and queues its outputs. Storage confirms at once, so the input's
    /// `SelfAck` goes straight back to the member.
    pub fn on<R>(&mut self, s: SiteId, f: impl FnOnce(&mut Member<KvCmd>, Timestamp) -> Ready<KvCmd, R>) -> R {
        let now = self.now;
        let m = self.members[s.0 as usize].as_mut().expect("member is up");
        let (r, mut out) = f(m, now).persist(&mut self.disks[s.0 as usize]);
        loop {
            self.refused.extend(out.refused.drain(..).map(|(i, r, _)| (s, i, r)));
            self.deps.extend(out.deps.drain(..).map(|d| (s, now, d)));
            self.deps_used.extend(out.deps_used.drain(..).map(|d| (s, d)));
            self.inbox.extend(out.early.drain(..).chain(out.messages.drain(..)));
            self.resolutions.extend(out.resolutions.drain(..).map(|r| (s, r)));
            self.alarms.extend(out.alarms.drain(..).map(|a| (s, a)));
            let Some(ack) = out.self_ack.take() else { break };
            let m = self.members[s.0 as usize].as_mut().expect("member is up");
            out = m.self_ack(now, ack).persist(&mut self.disks[s.0 as usize]).1;
        }
        r
    }

    /// Like `on`, but the member crashes before its writes are durable: neither the writes nor any output survive.
    pub fn on_then_crash<R>(&mut self, s: SiteId, f: impl FnOnce(&mut Member<KvCmd>, Timestamp) -> Ready<KvCmd, R>) {
        let now = self.now;
        let m = self.members[s.0 as usize].as_mut().expect("member is up");
        drop(f(m, now));
        self.members[s.0 as usize] = None;
    }

    /// Like `on_then_crash`, but the input's `early` AppendEntries left before the crash: the followers may store
    /// entries the leader lost. Returns how many left.
    pub fn on_send_then_crash<R>(
        &mut self,
        s: SiteId,
        f: impl FnOnce(&mut Member<KvCmd>, Timestamp) -> Ready<KvCmd, R>,
    ) -> usize {
        let now = self.now;
        let m = self.members[s.0 as usize].as_mut().expect("member is up");
        let (_, out) = f(m, now).persist(&mut Discard);
        let n = out.early.len();
        self.crash(s);
        self.inbox.extend(out.early);
        n
    }

    /// Runs one input on `s` whose writes are not yet durable: the `early` AppendEntries are queued now, and the
    /// writes, the other outputs, and the `SelfAck` are returned for `confirm`.
    pub fn on_unconfirmed<R>(
        &mut self,
        s: SiteId,
        f: impl FnOnce(&mut Member<KvCmd>, Timestamp) -> Ready<KvCmd, R>,
    ) -> (R, Unconfirmed) {
        let now = self.now;
        let m = self.members[s.0 as usize].as_mut().expect("member is up");
        let ready = f(m, now);
        let w = ready.writes.clone();
        let (r, mut out) = ready.persist(&mut Discard);
        self.inbox.append(&mut out.early);
        (r, Unconfirmed { site: s, writes: w, out })
    }

    /// The writes of an `on_unconfirmed` input became durable: they reach the disk, the held outputs leave, and the
    /// `SelfAck` goes back to the member.
    pub fn confirm(&mut self, u: Unconfirmed) {
        let Unconfirmed { site: s, writes, mut out } = u;
        for w in &writes {
            graftyard_raft::Storage::write(&mut self.disks[s.0 as usize], w);
        }
        self.inbox.append(&mut out.messages);
        self.resolutions.extend(out.resolutions.drain(..).map(|r| (s, r)));
        self.alarms.extend(out.alarms.drain(..).map(|a| (s, a)));
        let now = self.now;
        self.deps.extend(out.deps.drain(..).map(|d| (s, now, d)));
        self.deps_used.extend(out.deps_used.drain(..).map(|d| (s, d)));
        if let Some(ack) = out.self_ack.take() {
            self.on(s, |m, now| m.self_ack(now, ack));
        }
    }

    pub fn crash(&mut self, s: SiteId) {
        self.members[s.0 as usize] = None;
        self.inbox.retain(|e| e.to != s);
    }

    pub fn restart(&mut self, s: SiteId) {
        self.restart_with(s, config(s));
    }

    /// Restarts `s` with another config, such as another release.
    pub fn restart_with(&mut self, s: SiteId, cfg: graftyard_raft::MemberConfig) {
        self.boots += 1;
        let (m, ready) = Member::restore(cfg, self.disks[s.0 as usize].clone(), self.now, self.boots).unwrap();
        self.members[s.0 as usize] = Some(m);
        let ((), out) = ready.persist(&mut self.disks[s.0 as usize]);
        self.inbox.extend(out.messages);
    }

    /// Delivers queued messages that `keep` accepts, repeatedly, until none are left that it accepts. Rejected ones
    /// are dropped. Messages to a crashed member are dropped.
    pub fn deliver(&mut self, mut keep: impl FnMut(&Envelope<KvEvent>) -> bool) {
        for _ in 0..10_000 {
            if self.inbox.is_empty() {
                return;
            }
            let batch = std::mem::take(&mut self.inbox);
            for e in batch {
                if !keep(&e) || self.members[e.to.0 as usize].is_none() {
                    continue;
                }
                let Envelope { from, to, msg } = e;
                self.on(to, |m, now| m.step(now, from, msg));
            }
        }
        let tail: Vec<String> =
            self.inbox.iter().take(6).map(|e| format!("{:?}->{:?} {:?}", e.from, e.to, e.msg)).collect();
        panic!("messages never settled: {}", tail.join("\n").chars().take(3000).collect::<String>());
    }

    pub fn deliver_all(&mut self) {
        self.deliver(|_| true);
    }

    pub fn advance(&mut self, ms: u64) {
        self.now = Timestamp(self.now.0 + ms);
        for s in SITES {
            if self.members[s.0 as usize].is_some() {
                self.on(s, |m, now| m.tick(now));
            }
        }
    }

    /// Bootstraps the group with `s` taking the creating call, and runs until it leads with its `Noop` committed.
    pub fn elect(&mut self, s: SiteId) {
        self.on(s, |m, now| m.campaign(now));
        self.deliver_all();
        if self.m(s).role() != Role::Leader {
            self.on(s, |m, now| m.campaign(now));
            self.deliver_all();
        }
        assert_eq!(self.m(s).role(), Role::Leader, "{s:?} did not win");
    }

    /// Campaigns `s` once every promise lapsed.
    pub fn elect_after_promises(&mut self, s: SiteId) {
        self.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1);
        self.deliver(|_| false);
        self.on(s, |m, now| m.campaign(now));
        self.deliver_all();
        assert_eq!(self.m(s).role(), Role::Leader, "{s:?} did not win");
    }

    /// Commits one more entry through leader `l`, and runs until every follower holds the leader's commit (the idle
    /// leader's commit round, one CALL_DEADLINE after the entry) and then one more CALL_DEADLINE with nothing sent, so
    /// a `suspect` report right after counts as late.
    pub fn sync(&mut self, l: SiteId) {
        let id = graftyard_raft::RequestId(graftyard_raft::Ulid(u128::MAX - u128::from(self.now.0)));
        let cmd = KvCmd::Put { key: "sync".into(), value: self.now.0.to_string() };
        self.on(l, |m, now| m.commit(now, id, cmd)).expect("leader admits");
        self.deliver_all();
        for _ in 0..2 {
            self.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
            self.deliver_all();
        }
    }

    /// Admits `hold` so one entry is in flight, then `cmds`, which group into the next entry; delivers until that
    /// next entry exists on the leader and has not reached any follower. Returns the tickets of `cmds`.
    pub fn one_entry(&mut self, l: SiteId, first_id: u128, cmds: Vec<KvCmd>) -> Vec<Ticket> {
        let hold = KvCmd::Put { key: "hold".into(), value: first_id.to_string() };
        self.on(l, |m, now| m.commit(now, graftyard_raft::RequestId(graftyard_raft::Ulid(first_id)), hold))
            .expect("admitted");
        let batch = self.m(l).log().len() as u64 + 1;
        let tickets = cmds
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                let id = graftyard_raft::RequestId(graftyard_raft::Ulid(first_id + 1 + i as u128));
                self.on(l, |m, now| m.commit(now, id, c)).expect("admitted").ticket
            })
            .collect();
        assert_eq!(self.m(l).log().len() as u64, batch - 1, "the batch must wait for the entry in flight");
        self.deliver(|e| match &e.msg {
            graftyard_raft::Message::Append(a) => a.entries.iter().all(|x| x.index.0 < batch),
            _ => true,
        });
        assert_eq!(self.m(l).log().len() as u64, batch, "the batch entry was appended");
        tickets
    }

    pub fn ticket_results(&self, t: Ticket) -> Vec<&Resolution<KvCmd>> {
        self.resolutions
            .iter()
            .filter(|(_, r)| matches!(r, Resolution::Ticket { ticket, .. } if *ticket == t))
            .map(|(_, r)| r)
            .collect()
    }

    /// One storage confirm on `s` that takes `ms`: begun now, ended `ms` later, with time passing meanwhile.
    pub fn slow_confirm(&mut self, s: SiteId, ms: u64) {
        let now = self.now;
        let id = self.members[s.0 as usize].as_mut().expect("member is up").confirm_begin(now);
        self.advance(ms);
        self.on(s, |m, now| m.confirm_end(now, id));
    }

    /// One storage confirm on `s` that takes `ms` from its begin, of which its storage took `storage_ms`: the rest
    /// waited for the host's deferred flush and the input's turn (`Member::confirm_end_timed`).
    pub fn timed_confirm(&mut self, s: SiteId, ms: u64, storage_ms: u32) {
        let now = self.now;
        let id = self.members[s.0 as usize].as_mut().expect("member is up").confirm_begin(now);
        self.advance(ms);
        self.on(s, |m, now| m.confirm_end_timed(now, id, storage_ms));
    }

    /// Every alarm released so far, with its site.
    pub fn alarms(&self) -> &[(SiteId, graftyard_raft::Alarm)] {
        &self.alarms
    }

    /// Advances in steps of `step_ms`, delivering what `keep` accepts after each, for `total_ms`.
    pub fn run(&mut self, total_ms: u64, step_ms: u64, mut keep: impl FnMut(&Envelope<KvEvent>) -> bool) {
        let mut t = 0;
        while t < total_ms {
            self.advance(step_ms);
            self.deliver(&mut keep);
            t += step_ms;
        }
    }

    pub fn leader(&self) -> Option<SiteId> {
        SITES
            .iter()
            .copied()
            .filter(|s| self.members[s.0 as usize].as_ref().is_some_and(|m| m.role() == Role::Leader))
            .max_by_key(|s| self.m(*s).term())
    }

    pub fn value(&self, s: SiteId, k: &str) -> Option<String> {
        self.m(s).group_fold().fold.0.get(k).cloned()
    }
}
