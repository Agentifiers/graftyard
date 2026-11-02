//! Test hosts for the identity commands.
//!
//! `entry` decides a batch the way the leader does (one scratch view, each command seeing the earlier commands'
//! events), applies it, and checks every produced event against a fresh scratch as a follower's `verify` does.
//! `Group` hosts three real `graftyard_raft::Member`s of one chain and delivers their messages by hand, as
//! `crates/raft/tests/common/cluster.rs` does for its register fold. That harness is fixed to `KvCmd`, so it cannot be
//! reused from this crate without editing `crates/raft`.

#![allow(dead_code)]

use graftyard_identity::{IdentityDep, Login, LoginKey};
use graftyard_raft::{
    Acked, Admitted, CallError, Command, CommitError, DepsWanted, Envelope, Ev, Fold, Format, Member, MemberConfig,
    Message, ReleaseTag, RequestId, Resolution, Role, Scratch, Stored, Ticket, Ulid,
};
use graftyard_types::{AccountId, ChainId, DbId, PlacementName, PlacementRecord, Region, ReleaseId, SiteId, Timestamp};

pub const SITES: [SiteId; 3] = [SiteId(0), SiteId(1), SiteId(2)];
pub const A: SiteId = SITES[0];
pub const B: SiteId = SITES[1];
pub const C: SiteId = SITES[2];

pub fn login(s: &str) -> Login {
    Login::parse(s).unwrap()
}

pub fn key(s: &str) -> LoginKey {
    login(s).key()
}

pub fn acct(cell: u8, block: u32, offset: u16) -> AccountId {
    AccountId(DbId::pack(cell, block, offset).unwrap())
}

pub type Outcome<C> = Result<Acked<<C as Command>::Out>, CommitError<<C as Command>::Err>>;

pub type Decided<C> = Result<<C as Command>::Out, <C as Command>::Err>;

/// Decides `cmds` as one entry on `fold`, then applies the entry. Panics if a follower's check would refuse an
/// event the leader produced.
pub fn entry<C: Command<Dep = IdentityDep>>(
    fold: &mut C::Fold,
    cmds: &[C],
    deps: &[(ChainId, IdentityDep)],
) -> Vec<Decided<C>> {
    let mut out = Vec::new();
    let mut events: Vec<Ev<C>> = Vec::new();
    {
        let mut s = fold.scratch();
        for c in cmds {
            match c.decide(&s, deps) {
                Ok((evs, o)) => {
                    for e in &evs {
                        s.apply(e);
                    }
                    events.extend(evs);
                    out.push(Ok(o));
                }
                Err(e) => out.push(Err(e)),
            }
        }
    }
    {
        let mut v = fold.scratch();
        for (i, e) in events.iter().enumerate() {
            if let Err(why) = v.check(e) {
                panic!("follower refuses event {i} the leader produced: {why}: {e:?}");
            }
            v.apply(e);
        }
    }
    for e in &events {
        fold.apply(e);
    }
    out
}

pub fn record() -> PlacementRecord {
    PlacementRecord {
        name: PlacementName::Tri,
        sites: SITES.iter().copied().collect(),
        home: A,
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    }
}

/// Three members of one chain on `Tri`, driven by hand.
pub struct Group<C: Command> {
    pub chain: ChainId,
    pub format: Format,
    pub now: Timestamp,
    pub members: Vec<Option<Member<C>>>,
    pub disks: Vec<Stored<C::Fold>>,
    pub inbox: Vec<Envelope<Ev<C>>>,
    pub resolutions: Vec<Resolution<C>>,
    pub deps: Vec<(SiteId, DepsWanted)>,
    pub alarms: Vec<(SiteId, graftyard_raft::Alarm)>,
    boots: u64,
}

impl<C: Command> Group<C> {
    pub fn config(&self, me: SiteId) -> MemberConfig {
        MemberConfig {
            chain: self.chain,
            me,
            record: record(),
            regions: vec![(A, Region::Enam), (B, Region::Weur), (C, Region::Apac)],
            release: ReleaseTag { id: ReleaseId([7; 32]), seq: 1 },
            reads: vec![self.format],
        }
    }

    /// Members that have stored nothing, whose fold starts as `fold` (a shard's fold names its shard).
    pub fn new(chain: ChainId, format: Format, fold: C::Fold) -> Group<C> {
        let mut g = Group {
            chain,
            format,
            now: Timestamp(10_000),
            members: Vec::new(),
            disks: SITES.iter().map(|_| Stored::new(fold.clone())).collect(),
            inbox: Vec::new(),
            resolutions: Vec::new(),
            deps: Vec::new(),
            alarms: Vec::new(),
            boots: 0,
        };
        for s in SITES {
            g.members.push(None);
            g.restart(s);
        }
        g
    }

    pub fn m(&self, s: SiteId) -> &Member<C> {
        self.members[s.0 as usize].as_ref().expect("member is up")
    }

    /// Runs one input on `s`, persists it, and queues its outputs.
    pub fn on<R>(&mut self, s: SiteId, f: impl FnOnce(&mut Member<C>, Timestamp) -> graftyard_raft::Ready<C, R>) -> R {
        let now = self.now;
        let m = self.members[s.0 as usize].as_mut().expect("member is up");
        let (r, mut out) = f(m, now).persist(&mut self.disks[s.0 as usize]);
        // Storage confirms at once, so the leader's `SelfAck` goes straight back to it.
        loop {
            self.inbox.extend(out.early.drain(..).chain(out.messages.drain(..)));
            self.resolutions.append(&mut out.resolutions);
            self.deps.extend(out.deps.drain(..).map(|d| (s, d)));
            self.alarms.extend(out.alarms.drain(..).map(|a| (s, a)));
            let Some(ack) = out.self_ack.take() else { break };
            let m = self.members[s.0 as usize].as_mut().expect("member is up");
            out = m.self_ack(now, ack).persist(&mut self.disks[s.0 as usize]).1;
        }
        r
    }

    pub fn crash(&mut self, s: SiteId) {
        self.members[s.0 as usize] = None;
        self.inbox.retain(|e| e.to != s && e.from != s);
        self.deps.retain(|(d, _)| *d != s);
    }

    pub fn restart(&mut self, s: SiteId) {
        self.boots += 1;
        let cfg = self.config(s);
        let (m, ready) = Member::restore(cfg, &self.disks[s.0 as usize], self.now, self.boots).unwrap();
        self.members[s.0 as usize] = Some(m);
        let ((), out) = ready.persist(&mut self.disks[s.0 as usize]);
        self.inbox.extend(out.early.into_iter().chain(out.messages));
    }

    pub fn deliver(&mut self, mut keep: impl FnMut(&Envelope<Ev<C>>) -> bool) {
        for _ in 0..10_000 {
            if self.inbox.is_empty() {
                return;
            }
            for e in std::mem::take(&mut self.inbox) {
                if !keep(&e) || self.members[e.to.0 as usize].is_none() {
                    continue;
                }
                let Envelope { from, to, msg } = e;
                self.on(to, |m, now| m.step(now, from, msg));
            }
        }
        panic!("messages never settled");
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

    /// Bootstraps the group with `s` taking the creating call and runs until it leads with its `Noop` committed, as
    /// `crates/raft/tests/common/cluster.rs` does.
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

    pub fn leader(&self) -> Option<SiteId> {
        SITES
            .iter()
            .copied()
            .filter(|s| self.members[s.0 as usize].as_ref().is_some_and(|m| m.role() == Role::Leader))
            .max_by_key(|s| self.m(*s).term())
    }

    pub fn commit(&mut self, s: SiteId, id: u128, cmd: C) -> Result<Admitted, CallError> {
        self.on(s, |m, now| m.commit(now, RequestId(Ulid(id)), cmd))
    }

    /// What the host does with a block the `Ids` group leased: records it on the group's leader
    /// (`Member::id_block`).
    pub fn id_block(&mut self, s: SiteId, b: graftyard_identity::IdBlock) -> Result<(), graftyard_raft::IdBlockError> {
        self.on(s, |m, now| m.id_block(now, b))
    }

    /// Whether the block committed on some leader.
    pub fn block_committed(&self, b: graftyard_identity::IdBlock) -> bool {
        self.resolutions.iter().any(|r| matches!(r, Resolution::IdBlock { block, result: Ok(_) } if *block == b))
    }

    /// Answers every dependency read the members asked for, reading each chain through `read`.
    pub fn answer_deps(&mut self, read: &mut dyn FnMut(ChainId) -> C::Dep) {
        for (s, want) in std::mem::take(&mut self.deps) {
            if self.members[s.0 as usize].is_none() {
                continue;
            }
            let deps = want.chains.iter().map(|c| (*c, read(*c))).collect();
            self.on(s, |m, now| m.deps_read(now, want.id, deps));
        }
    }

    /// Delivers and answers dependency reads until nothing is left, then lets a round pass so every follower learns
    /// the commit index.
    pub fn settle(&mut self, read: &mut dyn FnMut(ChainId) -> C::Dep) {
        for _ in 0..1_000 {
            self.deliver_all();
            if self.deps.is_empty() {
                self.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
                self.deliver_all();
                if self.deps.is_empty() && self.inbox.is_empty() {
                    return;
                }
            }
            self.answer_deps(read);
        }
        panic!("never settled");
    }

    /// What an edge does: send to whoever leads, settle, and re-send under the same request id while the ticket
    /// ends `NotLeader` or unresolved. Returns the final outcome.
    pub fn submit(&mut self, id: u128, cmd: C, read: &mut dyn FnMut(ChainId) -> C::Dep) -> Outcome<C> {
        for _ in 0..20 {
            let Some(s) = self.leader() else {
                self.settle(read);
                continue;
            };
            let Ok(a) = self.commit(s, id, cmd.clone()) else {
                self.settle(read);
                continue;
            };
            self.settle(read);
            match self.result(a.ticket) {
                Some(Err(CommitError::NotLeader(_))) | None => continue,
                Some(r) => return r.clone(),
            }
        }
        panic!("request {id} never resolved");
    }

    pub fn result(&self, t: Ticket) -> Option<&Outcome<C>> {
        self.resolutions.iter().find_map(|r| match r {
            Resolution::Ticket { ticket, result } if *ticket == t => Some(result),
            _ => None,
        })
    }

    pub fn results(&self, t: Ticket) -> Vec<&Outcome<C>> {
        self.resolutions
            .iter()
            .filter_map(|r| match r {
                Resolution::Ticket { ticket, result } if *ticket == t => Some(result),
                _ => None,
            })
            .collect()
    }

    /// The leader's fold through `fresh`: a read grant, then the view it allows.
    pub fn fresh(&mut self, s: SiteId) -> C::Fold {
        let id = self.on(s, |m, now| m.read(now)).expect("leader reads");
        self.deliver_all();
        let grant = self
            .resolutions
            .iter()
            .find_map(|r| match r {
                Resolution::Read { id: i, result } if *i == id => Some(result.clone().expect("granted")),
                _ => None,
            })
            .expect("read resolved");
        self.m(s).fresh(&grant).expect("fresh").fold().clone()
    }

    /// The events of every entry at or below the member's commit index, in log order.
    pub fn committed_events(&self, s: SiteId) -> Vec<Ev<C>> {
        let commit = self.m(s).state().commit.0 as usize;
        self.m(s).log()[..commit]
            .iter()
            .filter_map(|e| match &e.body {
                graftyard_raft::EntryBody::Events(evs) => Some(evs.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// Whether the message carries an entry at or above `index`.
    pub fn carries(e: &Envelope<Ev<C>>, index: u64) -> bool {
        matches!(&e.msg, Message::Append(a) if a.entries.iter().any(|x| x.index.0 >= index))
    }
}
