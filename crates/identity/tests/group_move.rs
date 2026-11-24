//! BF-30's move between placements on real `graftyard_raft::Member`s (`graftyard_identity::group_move`): a group on
//! sites 0, 1, 2 moves to a record that shares two of them, and to one that shares none, one voter site per config
//! change through a one-site `Joint`, transferring leadership before the leader's site leaves. Commands commit on
//! every step, every committed command survives on the final members, and the group never names a config its leader
//! is not in.

use graftyard_identity::group_move::{
    MoveStep, TransferGate, WARM_BEFORE_TRANSFER, WarmWait, holds, next_step, replace_plan, replace_step,
    transfer_gate, warm_site,
};
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
    /// The record the group was created under.
    rec: PlacementRecord,
}

fn config(me: SiteId, record: &PlacementRecord) -> MemberConfig {
    MemberConfig {
        chain: CHAIN,
        me,
        record: record.clone(),
        regions: (0..N as u8).map(|s| (SiteId(s), REGIONS[s as usize])).collect(),
        release: ReleaseTag { id: ReleaseId([7; 32]), seq: 1 },
        reads: vec![NAMES_FORMAT],
    }
}

impl G {
    fn new() -> G {
        G::on_record(record([0, 1, 2], 0))
    }

    /// A group created under `rec`, on its three sites, led from its first.
    fn on_record(rec: PlacementRecord) -> G {
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
            rec,
        };
        let (first, sites) = (g.rec.sites[0], g.rec.sites.clone());
        for s in sites {
            g.start(s);
        }
        for _ in 0..3 {
            g.on(first, |m, now| m.campaign(now));
            g.deliver();
            if g.m(first).role() == Role::Leader {
                break;
            }
        }
        g.pass(2_000);
        g
    }

    /// A member on `s` that has stored nothing, as a joining member is.
    fn start(&mut self, s: SiteId) {
        self.boots += 1;
        let (m, ready) = Member::restore(config(s, &self.rec), self.disks[s.0 as usize].clone(), self.now, self.boots)
            .expect("restore");
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
        let checks = PlacementChecks { sites: &t, separate_accounts: false, rtt: &r, near: &[] };
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
            (MoveStep::Degrade { lost }, GroupConfig::Normal(p)) => {
                self.change(GroupConfig::Degraded { placement: p, lost }).expect("degraded");
            }
            (MoveStep::Restore { add }, GroupConfig::Degraded { placement: p, lost }) => {
                let old = p.voters().iter().find(|m| m.site == lost).unwrap().clone();
                let new = MemberRef::new(CHAIN, add, old.kind, old.slot, p.next_generation());
                let voters = p.voters().iter().map(|m| if m.site == lost { new.clone() } else { m.clone() }).collect();
                let home = if p.home() == lost { add } else { p.home() };
                let np = Placement::new(voters, Default::default(), home, &checks).expect("placement");
                self.start(add);
                self.change(GroupConfig::Normal(Box::new(np))).expect("restore");
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
    let checks = PlacementChecks { sites: &t, separate_accounts: false, rtt: &r, near: &[] };
    let new: arrayvec::ArrayVec<MemberRef, 3> = [(0u8, 0u8), (3, 1), (4, 2)]
        .iter()
        .map(|&(s, slot)| MemberRef::new(CHAIN, SiteId(s), MemberKind::Full, slot, p.next_generation()))
        .collect();
    let np = Placement::new(new, Default::default(), SiteId(0), &checks).unwrap();
    assert_eq!(g.change(GroupConfig::Joint { old: p, new: Box::new(np) }), Err(ReconfigError::NotOneChange));
}

/// A leader elected on the site a `Joint` drops (the old leader moved leadership there, or it won an election while
/// the `Joint` held): it moves leadership to a voter of the new half before the `Normal` that drops its site.
#[test]
fn a_leader_on_the_site_a_joint_drops_transfers_before_it_leaves() {
    let mut g = G::new();
    let to = record([3, 1, 2], 3);
    // Leadership on 1, then the swap of 0 for 3: the `Joint` drops 0 while 1 leads.
    g.perform(MoveStep::Transfer { to: SiteId(1) }, &to);
    g.perform(MoveStep::Swap { add: SiteId(3), drop: SiteId(0) }, &to);
    // Leadership moves back to 0, which the `Joint`'s old half still names.
    g.perform(MoveStep::Transfer { to: SiteId(0) }, &to);
    assert_eq!(g.leader(), SiteId(0));
    assert!(matches!(next_step(&g.cfg(), SiteId(0), &to), MoveStep::Transfer { .. }));
    let steps = g.move_to(&to, next_step);
    assert!(matches!(steps[0], MoveStep::Transfer { .. }), "{steps:?}");
    assert_eq!(voters(&g.cfg()), vec![1, 2, 3]);
    g.assert_all_kept(&[1, 2, 3]);
}

/// A move that changes the voter count has no one-site path: the planner says so instead of panicking.
#[test]
fn a_move_between_solo_and_three_sites_is_unsupported() {
    let solo =
        PlacementRecord { name: PlacementName::Solo, sites: [SiteId(0)].into_iter().collect(), ..record([0, 1, 2], 0) };
    let tri = record([0, 1, 2], 0);
    let normal = |r: &PlacementRecord| GroupConfig::Normal(Box::new(Placement::from_record(CHAIN, r).unwrap()));
    assert_eq!(next_step(&normal(&solo), SiteId(0), &tri), MoveStep::Unsupported, "Solo to Tri");
    assert_eq!(next_step(&normal(&tri), SiteId(0), &solo), MoveStep::Unsupported, "Tri to Solo");
}

/// The target site's git cache shard, as `move_step`'s `shard_warm` sees it: warm from `warm_at` (ms) on, or never.
struct Shard {
    warm_at: Option<u64>,
}

/// What one gated `/move` ask did (`move_step`): held on the shard, or the step taken, and whether cold.
#[derive(Debug, PartialEq)]
enum Ask {
    Held,
    Took(MoveStep, bool),
}

impl G {
    /// One `/move` ask as `move_step` answers it: a step that moves the leader (`warm_site`) asks the shard and
    /// `transfer_gate` first, and a held step changes nothing.
    fn ask(&mut self, to: &PlacementRecord, shard: &Shard, wait: &mut Option<WarmWait>) -> Ask {
        let (cfg, l) = (self.cfg(), self.leader());
        let step = next_step(&cfg, l, to);
        let mut cold = false;
        if warm_site(&cfg, l, to, step).is_some() {
            let warm = shard.warm_at.is_some_and(|w| self.now.0 >= w);
            let (gate, next, _) = transfer_gate(warm, *wait, self.now);
            *wait = next;
            match gate {
                TransferGate::Hold => return Ask::Held,
                TransferGate::Go => {}
                TransferGate::GoCold => cold = true,
            }
        }
        if step != MoveStep::Done {
            self.perform(step, to);
        }
        Ask::Took(step, cold)
    }

    fn home(&self) -> SiteId {
        match self.cfg() {
            GroupConfig::Normal(p) => p.home(),
            c => panic!("{c:?}"),
        }
    }
}

/// G11 after m3fg11: once a config naming another home is in the log, the core hands leadership to it by itself, so
/// a planned home-only move holds `Home`, not only `Transfer`, while the new home's shard warms. Held, nothing changes:
/// the home and the leader stay. Once the shard is warm `Home` commits, and leadership follows with no `Transfer`.
#[test]
fn a_planned_home_holds_while_the_new_homes_shard_warms_and_the_hand_off_follows() {
    let mut g = G::new();
    let to = record([0, 1, 2], 2);
    let shard = Shard { warm_at: Some(g.now.0 + 20_000) };
    let mut wait = None;
    let mut held = 0;
    loop {
        match g.ask(&to, &shard, &mut wait) {
            Ask::Held => {
                held += 1;
                assert_eq!((g.home(), g.leader()), (SiteId(0), SiteId(0)), "a held Home changes nothing");
                g.pass(5_000);
            }
            Ask::Took(step, cold) => {
                assert_eq!((step, cold), (MoveStep::Home { home: SiteId(2) }, false));
                break;
            }
        }
    }
    assert_eq!(held, 4, "asked every 5 s, warm 20 s in");
    assert_eq!(g.home(), SiteId(2));
    // The core's hand-off back to `home` (`maybe_hand_off`, after HOME_CALLS answered calls to it): writes keep the
    // group's calls going, as client load does, and no `Transfer` is asked.
    for k in 0..30 {
        if g.leader() == SiteId(2) {
            break;
        }
        g.write(200 + k);
    }
    assert_eq!(g.leader(), SiteId(2), "leadership follows the committed home");
    assert_eq!(g.ask(&to, &shard, &mut wait), Ask::Took(MoveStep::Done, false));
    g.assert_all_kept(&[0, 1, 2]);
}

/// A shard that never warms: `Home` commits cold at WARM_BEFORE_TRANSFER, and a warm one at the first ask commits at once.
#[test]
fn a_planned_home_commits_cold_at_the_deadline_or_at_once_when_warm() {
    let mut g = G::new();
    let to = record([0, 1, 2], 1);
    let t0 = g.now.0;
    let (mut wait, mut asks) = (None, 0);
    let (took, at) = loop {
        asks += 1;
        let at = g.now.0;
        match g.ask(&to, &Shard { warm_at: None }, &mut wait) {
            Ask::Held => g.pass(5_000),
            t => break (t, at),
        }
    };
    assert_eq!(took, Ask::Took(MoveStep::Home { home: SiteId(1) }, true));
    assert_eq!(at - t0, WARM_BEFORE_TRANSFER.as_millis() as u64, "cold at the deadline, {asks} asks");
    assert_eq!(g.home(), SiteId(1));

    let mut g = G::new();
    let mut wait = None;
    let to = record([0, 1, 2], 1);
    assert_eq!(
        g.ask(&to, &Shard { warm_at: Some(0) }, &mut wait),
        Ask::Took(MoveStep::Home { home: SiteId(1) }, false)
    );
    assert_eq!(g.home(), SiteId(1));
}

/// Only a planned step waits: with `Home` held on a cold shard, the leader's loss still elects a new leader at the
/// first campaign (the host campaigns on a request), which serves writes at once, and the move goes on from it, gated
/// again as a planned step.
#[test]
fn a_failover_never_waits_for_the_shard() {
    let mut g = G::new();
    g.write(1);
    let to = record([0, 1, 2], 2);
    let shard = Shard { warm_at: None };
    let mut wait = None;
    assert_eq!(g.ask(&to, &shard, &mut wait), Ask::Held);
    g.members[0] = None;
    g.inbox.retain(|e| e.from != SiteId(0));
    let lost = g.now.0;
    while g.m(SiteId(1)).role() != Role::Leader {
        assert!(g.now.0 - lost < 10_000, "site 1 not elected 10 s after the loss");
        g.pass(1_000);
        g.on(SiteId(1), |m, now| m.campaign(now));
        g.deliver();
    }
    g.write(2);
    assert_eq!(g.home(), SiteId(0), "the held Home never committed");
    // The new leader's planned move is still gated on the cold shard.
    assert_eq!(g.ask(&to, &shard, &mut wait), Ask::Held);
}

/// `warm_site` names the site a step makes the next leader: a `Transfer`'s target, a `Home`'s home, and a `Swap`'s only
/// when it drops the home; never the leader's own site.
#[test]
fn warm_site_names_the_next_leaders_site() {
    let g = G::new();
    let cfg = g.cfg();
    let s = SiteId;
    assert_eq!(warm_site(&cfg, s(0), &record([0, 1, 2], 2), MoveStep::Home { home: s(2) }), Some(s(2)));
    assert_eq!(warm_site(&cfg, s(0), &record([3, 1, 2], 3), MoveStep::Transfer { to: s(1) }), Some(s(1)));
    // The swap drops site 1, not the home: no leader moves.
    assert_eq!(warm_site(&cfg, s(0), &record([0, 3, 2], 0), MoveStep::Swap { add: s(3), drop: s(1) }), None);
    // The swap drops the home (0) and adds the target's home (3): the `Joint` names 3, which the core hands off to.
    assert_eq!(warm_site(&cfg, s(1), &record([3, 1, 2], 3), MoveStep::Swap { add: s(3), drop: s(0) }), Some(s(3)));
    // Home on the leader's own site moves no leader.
    assert_eq!(warm_site(&cfg, s(2), &record([0, 1, 2], 2), MoveStep::Home { home: s(2) }), None);
}

impl G {
    /// Site `s` goes dark for good: its member answers nothing and nothing it sent arrives.
    fn blackhole(&mut self, s: SiteId) {
        self.members[s.0 as usize] = None;
        self.inbox.retain(|e| e.from != s);
    }

    /// `s` campaigns until it leads, as the host campaigns on a request once its leader stops answering.
    fn elect(&mut self, s: SiteId) {
        let t0 = self.now.0;
        while self.m(s).role() != Role::Leader {
            assert!(self.now.0 - t0 < 30_000, "{s:?} not elected 30 s after the loss");
            self.pass(1_000);
            self.on(s, |m, now| m.campaign(now));
            self.deliver();
        }
    }
}

/// M6-WP1 (BF-50), a Tri group whose leader's site is lost for good: a survivor is elected, and `ReplaceMember`'s
/// steps move the lost member onto the spare through a one-site `Joint` that commits on the two survivors, then make
/// the spare the home it took over. Every write acknowledged before and during the loss is on the final members.
#[test]
fn a_tri_group_replaces_its_lost_leaders_site_with_the_spare_and_keeps_every_write() {
    let mut g = G::new();
    for n in 0..5 {
        g.write(n);
    }
    assert_eq!(g.leader(), SiteId(0));
    g.blackhole(SiteId(0));
    g.elect(SiteId(1));
    g.write(5);
    let (lost, spare) = (SiteId(0), SiteId(3));
    let steps = g.move_to(&record([3, 1, 2], 3), |c, l, _| replace_step(c, l, lost, spare));
    assert!(matches!(steps[0], MoveStep::Swap { add: SiteId(3), drop: SiteId(0) }), "{steps:?}");
    assert!(!steps.iter().any(|s| matches!(s, MoveStep::Degrade { .. })), "Tri never degrades: {steps:?}");
    assert_eq!(voters(&g.cfg()), vec![1, 2, 3]);
    assert_eq!(g.leader(), SiteId(3), "the spare leads, as the lost home did");
    let GroupConfig::Normal(p) = g.cfg() else { panic!() };
    assert!(p.voters().iter().all(|m| m.site != spare || m.generation == 2), "the spare's member is a new generation");
    g.assert_all_kept(&[1, 2, 3]);
}

/// `*2w`: losing a Full member stops every commit, since `Normal`'s rule and a `Joint`'s old half both need both Full
/// members. With the leader on the surviving Full site the steps are `Degraded { lost }` first, which commits on that
/// member and the witness, then `Normal` with the spare in the lost member's slot and kind. A leader lost with the site
/// is the gap OPEN-QUESTIONS R48 records: its successor's Noop never commits, so `reconfigure` answers `Busy`.
#[test]
fn a_2w_group_loses_a_full_member_and_restores_it_on_the_spare_through_degraded() {
    let rec = PlacementRecord { name: PlacementName::Us2w, ..record([0, 1, 2], 0) };
    let mut g = G::on_record(rec);
    for n in 0..5 {
        g.write(n);
    }
    g.blackhole(SiteId(1));
    let (lost, spare) = (SiteId(1), SiteId(3));
    let steps = g.move_to(&record([0, 3, 2], 0), |c, l, _| replace_step(c, l, lost, spare));
    assert_eq!(steps, vec![MoveStep::Degrade { lost }, MoveStep::Restore { add: spare }]);
    let GroupConfig::Normal(p) = g.cfg() else { panic!() };
    let kinds: Vec<(u8, MemberKind, u8)> = p.voters().iter().map(|m| (m.site.0, m.kind, m.slot)).collect();
    assert_eq!(kinds, vec![(0, MemberKind::Full, 0), (3, MemberKind::Full, 1), (2, MemberKind::Witness, 2)]);
    assert_eq!((p.home(), g.leader()), (SiteId(0), SiteId(0)));
    g.assert_all_kept(&[0, 3]);
}

/// `*2w` with its witness lost: the halves of a one-site `Joint` need only the Full members, so the witness moves to
/// the spare by `Swap` with no `Degraded` in between, and the home stays.
#[test]
fn a_2w_group_moves_a_lost_witness_by_joint() {
    let rec = PlacementRecord { name: PlacementName::Us2w, ..record([0, 1, 2], 0) };
    let mut g = G::on_record(rec);
    g.write(1);
    g.blackhole(SiteId(2));
    let (lost, spare) = (SiteId(2), SiteId(3));
    let steps = g.move_to(&record([0, 1, 3], 0), |c, l, _| replace_step(c, l, lost, spare));
    assert_eq!(steps, vec![MoveStep::Swap { add: spare, drop: lost }, MoveStep::Leave]);
    assert_eq!(voters(&g.cfg()), vec![0, 1, 3]);
    g.assert_all_kept(&[0, 1]);
}

/// `holds`: the groups `ReplaceMember` moves are those still holding a voter on the lost site, including one whose
/// replace already committed `Degraded { lost }` and one mid-`Joint` in either half. A group that never had one there,
/// or whose move finished, is left alone.
#[test]
fn a_group_is_replaced_while_any_half_of_its_config_holds_the_lost_site() {
    let p = |sites: [u8; 3]| Box::new(Placement::from_record(CHAIN, &record(sites, sites[0])).unwrap());
    let s = SiteId;
    assert!(holds(&GroupConfig::Normal(p([0, 1, 2])), s(0)));
    assert!(!holds(&GroupConfig::Normal(p([3, 1, 2])), s(0)));
    assert!(holds(&GroupConfig::Degraded { placement: p([0, 1, 2]), lost: s(0) }, s(0)), "mid-replace");
    assert!(holds(&GroupConfig::Joint { old: p([0, 1, 2]), new: p([3, 1, 2]) }, s(0)), "the old half");
    assert!(holds(&GroupConfig::Joint { old: p([3, 1, 2]), new: p([0, 1, 2]) }, s(0)), "the new half");
}

/// `replace_plan`, the hosted ask: a group that never had a voter on the lost site is `Done` before any step, with its
/// leader left where it is (here off its home, which `next_step`'s identity target would transfer). A group that does
/// hold it takes the same steps as `replace_step`, and once started the steps after `Leave` still run.
#[test]
fn a_replace_leaves_a_group_that_never_held_the_lost_site_alone() {
    let mut g = G::new();
    g.write(1);
    g.perform(MoveStep::Transfer { to: SiteId(1) }, &record([0, 1, 2], 0));
    let (cfg, l) = (g.cfg(), g.leader());
    assert_eq!(l, SiteId(1));
    assert_eq!(replace_plan(&cfg, l, SiteId(4), SiteId(5), false, true), MoveStep::Done, "never held site 4");
    assert_eq!(
        replace_step(&cfg, l, SiteId(4), SiteId(5)),
        MoveStep::Transfer { to: SiteId(0) },
        "the identity target"
    );
    assert_eq!(
        replace_plan(&cfg, l, SiteId(2), SiteId(3), false, true),
        MoveStep::Swap { add: SiteId(3), drop: SiteId(2) }
    );
    let steps = g.move_to(&record([0, 1, 3], 0), |c, l, _| replace_plan(c, l, SiteId(2), SiteId(3), true, true));
    assert_eq!(voters(&g.cfg()), vec![0, 1, 3], "{steps:?}");
    g.assert_all_kept(&[0, 1, 3]);
}

/// A `RestorePlacement` moves a `*2w` group's Full member off a live spare: both Full members answer, so the move is a
/// one-site `Joint` and the group never drops to one content copy by `Degraded`, as a dead site's replace does.
#[test]
fn a_restore_moves_a_2w_full_member_back_by_joint_without_degrading() {
    let rec = PlacementRecord { name: PlacementName::Us2w, ..record([0, 3, 2], 0) };
    let mut g = G::on_record(rec);
    g.write(1);
    let (from, to) = (SiteId(3), SiteId(1));
    assert_eq!(
        replace_plan(&g.cfg(), SiteId(0), from, to, false, true),
        MoveStep::Degrade { lost: from },
        "a dead site"
    );
    let steps = g.move_to(&record([0, 1, 2], 0), |c, l, _| replace_plan(c, l, from, to, true, false));
    assert_eq!(steps, vec![MoveStep::Swap { add: to, drop: from }, MoveStep::Leave]);
    let GroupConfig::Normal(p) = g.cfg() else { panic!() };
    let kinds: Vec<(u8, MemberKind)> = p.voters().iter().map(|m| (m.site.0, m.kind)).collect();
    assert_eq!(kinds, vec![(0, MemberKind::Full), (1, MemberKind::Full), (2, MemberKind::Witness)]);
    g.assert_all_kept(&[0, 1]);
}
