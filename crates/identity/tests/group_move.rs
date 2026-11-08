//! BF-30's move between placements on real `graftyard_raft::Member`s (`graftyard_identity::group_move`): a group on
//! sites 0, 1, 2 moves to a record that shares two of them, and to one that shares none, one voter site per config
//! change through a one-site `Joint`, transferring leadership before the leader's site leaves. Commands commit on
//! every step, every committed command survives on the final members, and the group never names a config its leader
//! is not in.

use graftyard_identity::group_move::{MoveStep, next_step};
use graftyard_identity::{NAMES_FORMAT, NamesCmd, NamesFold, NamesOp, NamesRead};
use graftyard_raft::{
    Envelope, Ev, GroupConfig, Member, MemberConfig, ReconfigError, ReleaseTag, RequestId, Resolution, Role, Stored,
    Ulid,
};
use graftyard_types::{
    AccountId, ChainId, CredentialShard, DbId, Member as MemberRef, MemberKind, NameShard, Placement, PlacementChecks,
    PlacementName, PlacementRecord, Region, ReleaseId, Rtt, Site, SiteId, SiteLink, SiteTable, Timestamp,
};

const N: usize = 6;
const SHARD: NameShard = NameShard(5);
const CHAIN: ChainId = ChainId::Names(SHARD);
const REGIONS: [Region; N] = [Region::Enam, Region::Weur, Region::Apac, Region::Wnam, Region::Eeur, Region::Oc];

fn record(sites: [u8; 3], home: u8) -> PlacementRecord {
    PlacementRecord {
        name: PlacementName::Tri,
        sites: sites.iter().map(|s| SiteId(*s)).collect(),
        home: SiteId(home),
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    }
}

fn table() -> SiteTable {
    SiteTable(
        (0..N as u8)
            .map(|id| Site {
                id: SiteId(id),
                script: format!("s{id}"),
                account: "x".into(),
                region: REGIONS[id as usize],
                jurisdiction: None,
                bucket: String::new(),
                release: None,
                link: SiteLink::Binding { member_class: "Member".into() },
                spare: false,
            })
            .collect(),
    )
}

fn rtt() -> Vec<(SiteId, Rtt)> {
    let mut v = Vec::new();
    for a in 0..N as u8 {
        for b in 0..N as u8 {
            if a != b {
                v.push((SiteId(a), Rtt { peer: SiteId(b), median_ms: 80, at: Timestamp(0) }));
            }
        }
    }
    v
}

/// Six sites, of which the group's members are those its configs name: each a real member with its own disk.
struct G {
    now: Timestamp,
    members: Vec<Option<Member<NamesCmd>>>,
    disks: Vec<Stored<NamesFold>>,
    inbox: Vec<Envelope<Ev<NamesCmd>>>,
    resolutions: Vec<Resolution<NamesCmd>>,
    boots: u64,
    next: u128,
    /// Every committed `RepoOwner` this test sent, by request id.
    sent: Vec<(u128, i64)>,
}

fn config(me: SiteId) -> MemberConfig {
    MemberConfig {
        chain: CHAIN,
        me,
        record: record([0, 1, 2], 0),
        regions: (0..N as u8).map(|s| (SiteId(s), REGIONS[s as usize])).collect(),
        release: ReleaseTag { id: ReleaseId([7; 32]), seq: 1 },
        reads: vec![NAMES_FORMAT],
    }
}

impl G {
    fn new() -> G {
        let mut g = G {
            now: Timestamp(10_000),
            members: (0..N).map(|_| None).collect(),
            // A fold with no shard checks no key placement, so one group holds every write's `RepoOwner` row.
            disks: (0..N).map(|_| Stored::new(NamesFold::default())).collect(),
            inbox: Vec::new(),
            resolutions: Vec::new(),
            boots: 0,
            next: 1,
            sent: Vec::new(),
        };
        for s in 0..3u8 {
            g.start(SiteId(s));
        }
        for _ in 0..3 {
            g.on(SiteId(0), |m, now| m.campaign(now));
            g.deliver();
            if g.m(SiteId(0)).role() == Role::Leader {
                break;
            }
        }
        g.pass(2_000);
        g
    }

    /// A member on `s` that has stored nothing, as a joining member is.
    fn start(&mut self, s: SiteId) {
        self.boots += 1;
        let (m, ready) =
            Member::restore(config(s), self.disks[s.0 as usize].clone(), self.now, self.boots).expect("restore");
        self.members[s.0 as usize] = Some(m);
        let ((), out) = ready.persist(&mut self.disks[s.0 as usize]);
        self.inbox.extend(out.early.into_iter().chain(out.messages));
    }

    fn m(&self, s: SiteId) -> &Member<NamesCmd> {
        self.members[s.0 as usize].as_ref().expect("member is up")
    }

    fn on<R>(
        &mut self,
        s: SiteId,
        f: impl FnOnce(&mut Member<NamesCmd>, Timestamp) -> graftyard_raft::Ready<NamesCmd, R>,
    ) -> R {
        let now = self.now;
        let m = self.members[s.0 as usize].as_mut().expect("member is up");
        let (r, mut out) = f(m, now).persist(&mut self.disks[s.0 as usize]);
        loop {
            self.inbox.extend(out.early.drain(..).chain(out.messages.drain(..)));
            self.resolutions.append(&mut out.resolutions);
            assert!(out.deps.is_empty(), "RepoOwner reads nothing");
            let Some(ack) = out.self_ack.take() else { break };
            let m = self.members[s.0 as usize].as_mut().expect("member is up");
            out = m.self_ack(now, ack).persist(&mut self.disks[s.0 as usize]).1;
        }
        r
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

    fn pass(&mut self, ms: u64) {
        let end = self.now.0 + ms;
        while self.now.0 < end {
            self.now = Timestamp((self.now.0 + 500).min(end));
            for s in 0..N {
                if self.members[s].is_some() {
                    self.on(SiteId(s as u8), |m, now| m.tick(now));
                }
            }
            self.deliver();
        }
    }

    fn leader(&self) -> SiteId {
        (0..N as u8)
            .map(SiteId)
            .filter(|s| self.members[s.0 as usize].as_ref().is_some_and(|m| m.role() == Role::Leader))
            .max_by_key(|s| self.m(*s).term())
            .expect("a leader")
    }

    fn cfg(&self) -> GroupConfig {
        self.m(self.leader()).committed_config().expect("a config").1.clone()
    }

    /// One command through whoever leads, re-sent under its id while unresolved: the edge's rule.
    fn write(&mut self, n: i64) {
        self.next += 1;
        let id = self.next;
        let op = NamesOp::RepoOwner { repo: graftyard_types::RepoId(DbId(1_000 + n)), owner: AccountId(DbId(n)) };
        let cmd = NamesCmd { cred: CredentialShard::NONE, op };
        for _ in 0..10 {
            let l = self.leader();
            let Ok(a) = self.on(l, |m, now| m.commit(now, RequestId(Ulid(id)), cmd.clone())) else {
                self.pass(1_000);
                continue;
            };
            self.pass(1_000);
            match self.resolutions.iter().rev().find_map(|r| match r {
                Resolution::Ticket { ticket, result } if *ticket == a.ticket => Some(result.is_ok()),
                _ => None,
            }) {
                Some(true) => {
                    self.sent.push((id, n));
                    return;
                }
                Some(false) | None => continue,
            }
        }
        panic!("write {n} never committed");
    }

    /// One config change on the leader, waiting for it to commit.
    fn change(&mut self, next: GroupConfig) -> Result<(), ReconfigError> {
        let l = self.leader();
        let i = self.on(l, |m, now| m.reconfigure(now, next))?;
        for _ in 0..20 {
            self.pass(500);
            if self.m(self.leader()).committed_config().is_some_and(|c| c.0 >= i) {
                return Ok(());
            }
        }
        panic!("config {i:?} never committed");
    }

    /// Performs one planned step, the way the site's move route does.
    fn perform(&mut self, step: MoveStep, to: &PlacementRecord) {
        let t = table();
        let r = rtt();
        let checks = PlacementChecks { sites: &t, separate_accounts: false, rtt: &r };
        match (step, self.cfg()) {
            (MoveStep::Transfer { to }, _) => {
                let l = self.leader();
                self.on(l, |m, now| m.transfer(now, to)).expect("transfer starts");
                for _ in 0..20 {
                    self.pass(500);
                    if self.leader() == to {
                        return;
                    }
                }
                panic!("leadership never reached {to:?}");
            }
            (MoveStep::Swap { add, drop }, GroupConfig::Normal(p)) => {
                let old = p.voters().iter().find(|m| m.site == drop).unwrap().clone();
                let new = MemberRef::new(CHAIN, add, old.kind, old.slot, p.next_generation());
                let voters = p.voters().iter().map(|m| if m.site == drop { new.clone() } else { m.clone() }).collect();
                let home = if p.home() == drop { to.home } else { p.home() };
                let home = if voters_has(&voters, home) { home } else { add };
                let np = Placement::new(voters, Default::default(), home, &checks).expect("placement");
                self.start(add);
                self.change(GroupConfig::Joint { old: p.clone(), new: Box::new(np) }).expect("one-site joint");
            }
            (MoveStep::Leave, GroupConfig::Joint { new, .. }) => {
                self.change(GroupConfig::Normal(new)).expect("leave");
            }
            (MoveStep::Home { home }, GroupConfig::Normal(p)) => {
                let np = Placement::new(p.voters().iter().cloned().collect(), Default::default(), home, &checks)
                    .expect("placement");
                self.change(GroupConfig::Normal(Box::new(np))).expect("home");
            }
            (s, c) => panic!("{s:?} on {c:?}"),
        }
    }

    /// Runs the planner to `Done`, a command committed after every step. Returns the steps taken.
    fn move_to(
        &mut self,
        to: &PlacementRecord,
        plan: impl Fn(&GroupConfig, SiteId, &PlacementRecord) -> MoveStep,
    ) -> Vec<MoveStep> {
        let mut steps = Vec::new();
        for k in 0..30 {
            let (cfg, l) = (self.cfg(), self.leader());
            assert!(graftyard_raft::quorums::is_voter(&cfg, l), "the leader {l:?} is not in {cfg:?}");
            let step = plan(&cfg, l, to);
            if step == MoveStep::Done {
                return steps;
            }
            steps.push(step);
            self.perform(step, to);
            self.write(k);
        }
        panic!("the move never finished: {steps:?}");
    }

    /// Every member of `sites` holds every committed write: one more write first, so each follower learns the commit
    /// index of the last one from its append.
    fn assert_all_kept(&mut self, sites: &[u8]) {
        self.write(999);
        self.sent.pop();
        self.pass(3_000);
        for s in sites {
            let f = &self.m(SiteId(*s)).group_fold().fold;
            for (_, n) in &self.sent {
                let repo = graftyard_types::RepoId(DbId(1_000 + n));
                assert_eq!(f.repo_owner(repo), Some(AccountId(DbId(*n))), "site {s} lost write {n}");
            }
        }
    }
}

fn voters_has(v: &arrayvec::ArrayVec<MemberRef, 3>, s: SiteId) -> bool {
    v.iter().any(|m| m.site == s && m.kind == MemberKind::Full)
}

fn voters(c: &GroupConfig) -> Vec<u8> {
    let mut v: Vec<u8> = graftyard_raft::quorums::voters(c).into_iter().map(|s| s.0).collect();
    v.sort();
    v
}

/// The second placement shares two sites with the first, and the leader's site is the one that leaves.
#[test]
fn a_move_to_a_placement_sharing_two_sites_swaps_one_and_keeps_writing() {
    let mut g = G::new();
    g.write(100);
    assert_eq!(g.leader(), SiteId(0));
    let to = record([3, 1, 2], 3);
    let steps = g.move_to(&to, next_step);
    assert_eq!(voters(&g.cfg()), vec![1, 2, 3]);
    assert_eq!(g.leader(), SiteId(3), "leadership ends on the new home");
    assert!(matches!(steps[0], MoveStep::Transfer { .. }), "leadership leaves site 0 before it does: {steps:?}");
    assert_eq!(steps.iter().filter(|s| matches!(s, MoveStep::Swap { .. })).count(), 1, "{steps:?}");
    g.assert_all_kept(&[1, 2, 3]);
}

/// A move to three new sites: three swaps, each a one-site `Joint` and its `Normal`.
#[test]
fn a_move_to_disjoint_sites_goes_one_site_per_config_change() {
    let mut g = G::new();
    g.write(100);
    let to = record([3, 4, 5], 4);
    let steps = g.move_to(&to, next_step);
    assert_eq!(voters(&g.cfg()), vec![3, 4, 5]);
    assert_eq!(g.leader(), SiteId(4));
    assert_eq!(steps.iter().filter(|s| matches!(s, MoveStep::Swap { .. })).count(), 3, "{steps:?}");
    g.assert_all_kept(&[3, 4, 5]);
}

/// A home-only move: one config change and a transfer.
#[test]
fn a_home_only_move_is_one_config_change_and_a_transfer() {
    let mut g = G::new();
    let to = record([0, 1, 2], 1);
    let steps = g.move_to(&to, next_step);
    assert_eq!(steps, vec![MoveStep::Home { home: SiteId(1) }, MoveStep::Transfer { to: SiteId(1) }]);
    g.assert_all_kept(&[0, 1, 2]);
}

/// The design's two-placement `Joint` cannot be expressed: `reconfigure` refuses it.
#[test]
fn a_joint_of_two_placements_differing_by_two_sites_is_refused() {
    let mut g = G::new();
    let GroupConfig::Normal(p) = g.cfg() else { panic!() };
    let t = table();
    let r = rtt();
    let checks = PlacementChecks { sites: &t, separate_accounts: false, rtt: &r };
    let new: arrayvec::ArrayVec<MemberRef, 3> = [(0u8, 0u8), (3, 1), (4, 2)]
        .iter()
        .map(|&(s, slot)| MemberRef::new(CHAIN, SiteId(s), MemberKind::Full, slot, p.next_generation()))
        .collect();
    let np = Placement::new(new, Default::default(), SiteId(0), &checks).unwrap();
    assert_eq!(g.change(GroupConfig::Joint { old: p, new: Box::new(np) }), Err(ReconfigError::NotOneChange));
}
