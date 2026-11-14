//! Three real `graftyard_raft::Member`s of one chain on `Tri`, driven by hand, with crashes and restarts, as
//! `crates/git/tests/hooks_gates.rs` drives a repository. Generic over the command, so an Actions shard and a
//! repository share it. A host with no leader campaigns once a second, as the site host does once a leader is silent.

#![allow(dead_code)]

use graftyard_raft::{
    Acked, Alarm, Command, CommitError, Envelope, Ev, Format, Member, MemberConfig, ReleaseTag, RequestId, Resolution,
    Role, Stored, Ulid,
};
use graftyard_types::{ChainId, IdBlock, Index, PlacementName, PlacementRecord, Region, ReleaseId, SiteId, Timestamp};

pub const SITES: [SiteId; 3] = [SiteId(0), SiteId(1), SiteId(2)];

pub type Outcome<C> = Result<Acked<<C as Command>::Out>, CommitError<<C as Command>::Err>>;

pub struct Group<C: Command> {
    pub chain: ChainId,
    reads: Vec<Format>,
    pub members: Vec<Option<Member<C>>>,
    pub disks: Vec<Stored<C::Fold>>,
    inbox: Vec<Envelope<Ev<C>>>,
    pub resolutions: Vec<Resolution<C>>,
    boots: u64,
    last_campaign: Timestamp,
}

impl<C: Command> Group<C> {
    pub fn new(chain: ChainId, reads: Vec<Format>, fold: C::Fold, now: Timestamp) -> Group<C> {
        let mut g = Group {
            chain,
            reads,
            members: SITES.iter().map(|_| None).collect(),
            disks: SITES.iter().map(|_| Stored::new(fold.clone())).collect(),
            inbox: Vec::new(),
            resolutions: Vec::new(),
            boots: 0,
            last_campaign: Timestamp(0),
        };
        for s in SITES {
            g.restart(s, now);
        }
        for _ in 0..3 {
            g.on(SITES[0], now, |m, t| m.campaign(t));
            g.deliver(now);
            if g.leader() == Some(SITES[0]) {
                return g;
            }
        }
        panic!("{chain} elected no leader");
    }

    fn config(&self, me: SiteId) -> MemberConfig {
        MemberConfig {
            chain: self.chain,
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
            release: ReleaseTag { id: ReleaseId([5; 32]), seq: 1 },
            reads: self.reads.clone(),
        }
    }

    pub fn m(&self, s: SiteId) -> &Member<C> {
        self.members[s.0 as usize].as_ref().expect("member is up")
    }

    pub fn up(&self, s: SiteId) -> bool {
        self.members[s.0 as usize].is_some()
    }

    pub fn on<R>(
        &mut self,
        s: SiteId,
        now: Timestamp,
        f: impl FnOnce(&mut Member<C>, Timestamp) -> graftyard_raft::Ready<C, R>,
    ) -> R {
        let i = s.0 as usize;
        let m = self.members[i].as_mut().expect("member is up");
        let (r, mut out) = f(m, now).persist(&mut self.disks[i]);
        loop {
            self.inbox.extend(out.early.drain(..).chain(out.messages.drain(..)));
            self.resolutions.append(&mut out.resolutions);
            for a in out.alarms.drain(..) {
                // A follower's refusal means an entry `decide` built does not verify.
                assert!(matches!(a, Alarm::StepDown { .. }), "alarm on {s:?} of {}: {a:?}", self.chain);
            }
            let Some(ack) = out.self_ack.take() else { break };
            let m = self.members[i].as_mut().expect("member is up");
            out = m.self_ack(now, ack).persist(&mut self.disks[i]).1;
        }
        r
    }

    pub fn restart(&mut self, s: SiteId, now: Timestamp) {
        self.boots += 1;
        let cfg = self.config(s);
        let (m, ready) = Member::restore(cfg, self.disks[s.0 as usize].clone(), now, self.boots).unwrap();
        self.members[s.0 as usize] = Some(m);
        let ((), out) = ready.persist(&mut self.disks[s.0 as usize]);
        self.inbox.extend(out.early.into_iter().chain(out.messages));
    }

    /// The member and every message to or from it are gone; its disk stays.
    pub fn crash(&mut self, s: SiteId) {
        self.members[s.0 as usize] = None;
        self.inbox.retain(|e| e.to != s && e.from != s);
    }

    pub fn deliver(&mut self, now: Timestamp) {
        for _ in 0..10_000 {
            if self.inbox.is_empty() {
                return;
            }
            for Envelope { from, to, msg } in std::mem::take(&mut self.inbox) {
                if self.up(to) {
                    self.on(to, now, |m, t| m.step(t, from, msg));
                }
            }
        }
        panic!("messages never settled");
    }

    /// One host tick on every live member, and a campaign by the first live member while none leads.
    pub fn tick(&mut self, now: Timestamp) {
        for s in SITES {
            if self.up(s) {
                self.on(s, now, |m, t| m.tick(t));
            }
        }
        self.deliver(now);
        if self.leader().is_none()
            && now.0 >= self.last_campaign.0 + 1_000
            && let Some(s) = SITES.iter().copied().find(|s| self.up(*s))
        {
            self.last_campaign = now;
            self.on(s, now, |m, t| m.campaign(t));
            self.deliver(now);
        }
    }

    pub fn leader(&self) -> Option<SiteId> {
        SITES
            .iter()
            .copied()
            .filter(|s| self.members[s.0 as usize].as_ref().is_some_and(|m| m.role() == Role::Leader))
            .max_by_key(|s| self.m(*s).term())
    }

    /// Commits on the leader and settles: the outcome, or `None` with no leader or no resolution.
    pub fn commit(&mut self, now: Timestamp, id: u128, cmd: C) -> Option<Outcome<C>> {
        let l = self.leader()?;
        let a = self.on(l, now, |m, t| m.commit(t, RequestId(Ulid(id)), cmd)).ok()?;
        self.deliver(now);
        self.resolutions.iter().rev().find_map(|r| match r {
            Resolution::Ticket { ticket, result } if *ticket == a.ticket => Some(result.clone()),
            _ => None,
        })
    }

    /// Records events on the leader, as a push or a test's setup does.
    pub fn record(&mut self, now: Timestamp, id: u128, format: Format, evs: Vec<Ev<C>>) {
        let l = self.leader().expect("a leader");
        let rid = RequestId(Ulid(id));
        self.on(l, now, |m, t| m.record(t, rid, format, evs)).expect("recorded");
        self.deliver(now);
        assert!(self.resolutions.iter().any(|r| matches!(r, Resolution::Recorded { id, result: Ok(_) } if *id == rid)));
    }

    pub fn id_block(&mut self, now: Timestamp, b: IdBlock) {
        let l = self.leader().expect("a leader");
        self.on(l, now, |m, t| m.id_block(t, b)).expect("block taken");
        self.deliver(now);
    }

    /// The leader's applied fold.
    pub fn fold(&self) -> &C::Fold {
        &self.m(self.leader().expect("a leader")).group_fold().fold
    }

    pub fn commit_index(&self) -> Index {
        self.leader().map_or(Index(0), |l| self.m(l).state().commit)
    }

    /// The events of every committed entry after `after`, with each entry's index, on the leader.
    pub fn committed_after(&self, after: Index) -> Vec<(Index, Vec<Ev<C>>)> {
        let Some(l) = self.leader() else { return Vec::new() };
        let m = self.m(l);
        let commit = m.state().commit;
        m.log()
            .iter()
            .filter(|e| e.index > after && e.index <= commit)
            .map(|e| match &e.body {
                graftyard_raft::EntryBody::Events(evs) => (e.index, evs.clone()),
                _ => (e.index, Vec::new()),
            })
            .collect()
    }
}
