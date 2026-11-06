//! Members and placements (`design/DESIGN.md`, Shape, "Sites and placement").

use std::time::Duration;

use arrayvec::ArrayVec;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::chain::ChainId;
use crate::ids::Timestamp;
use crate::site::{Region, SiteId, SiteTable};

/// [guess]: intra-region round trips are a few ms, enam to weur about 80 ms.
pub const MIN_PAIR_RTT: Duration = Duration::from_millis(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberKind {
    Full,
    /// Stores (term, index, digest), never content: used for residency.
    Witness,
    /// Stores content, never votes, serves read-index reads: hot groups only.
    Learner,
}

impl MemberKind {
    fn letter(self) -> char {
        match self {
            MemberKind::Full => 'f',
            MemberKind::Witness => 'w',
            MemberKind::Learner => 'l',
        }
    }
}

/// `do_name` is "{chain}#{generation}#{kind}{slot}": `kind` is `f`, `w`, or `l`, and `slot` is the member's voter or
/// learner position, so a learner beside its group's voter on one site, or two learners on one site, never share an
/// object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub site: SiteId,
    pub kind: MemberKind,
    pub slot: u8,
    pub generation: u32,
    pub do_name: Box<str>,
}

/// The first raft address of a learner: learner slot `k` is addressed as `SiteId(LEARNER_ADDR + k)`, above every
/// site id (sites are below 64), so a learner beside a voter on one site, or two learners on one site, never share an
/// address in a group's messages, its leader's progress, or its quorum counts. Not in the design, whose messages name
/// sites only.
pub const LEARNER_ADDR: u8 = 192;

impl Member {
    pub fn new(chain: ChainId, site: SiteId, kind: MemberKind, slot: u8, generation: u32) -> Member {
        let do_name = format!("{chain}#{generation}#{}{slot}", kind.letter()).into_boxed_str();
        Member { site, kind, slot, generation, do_name }
    }

    /// Where the group's messages to this member go: its site for a voter, `learner_addr(slot)` for a learner.
    pub fn addr(&self) -> SiteId {
        match self.kind {
            MemberKind::Learner => learner_addr(self.slot),
            MemberKind::Full | MemberKind::Witness => self.site,
        }
    }
}

pub fn learner_addr(slot: u8) -> SiteId {
    assert!(usize::from(slot) < MAX_LEARNERS, "learner slot {slot} is not below MAX_LEARNERS");
    SiteId(LEARNER_ADDR + slot)
}

/// The learner slot an address names, or `None` for a voter's site.
pub fn learner_slot(addr: SiteId) -> Option<u8> {
    addr.0.checked_sub(LEARNER_ADDR).filter(|s| usize::from(*s) < MAX_LEARNERS)
}

/// A group's read rate (req/s) that adds learners.
pub const LEARNER_AT: u32 = 500;
pub const MAX_LEARNERS: usize = 16;
/// Reads a second one learner takes [inference: `design/DESIGN.md`, Scale, "1,000 req/s per learner"].
pub const LEARNER_READS: u32 = 1_000;

/// One group's reads, as `graft-ops` sums them from Analytics Engine counters: per region the edges' read rate
/// (req/s) at the day's peak. The design names `GroupLoad` without defining it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupLoad {
    pub reads: SmallVec<[(Region, u32); 6]>,
}

/// The regions to place learners in for one group: none while the group's reads stay under `LEARNER_AT`; above it,
/// `ceil(rate / LEARNER_READS)` per region with reads, handed out one at a time to the busiest region still owed one,
/// up to `MAX_LEARNERS`. A region may appear more than once, so two learners can share a site. Pure: `graft-ops`'s
/// daily cron reads the counters and applies the answer with a config change.
pub fn place_learners(load: &GroupLoad) -> ArrayVec<Region, MAX_LEARNERS> {
    let mut out = ArrayVec::new();
    let total: u64 = load.reads.iter().map(|(_, r)| u64::from(*r)).sum();
    if total < u64::from(LEARNER_AT) {
        return out;
    }
    let mut owed: Vec<(Region, u32, u32)> =
        load.reads.iter().filter(|(_, r)| *r > 0).map(|&(g, r)| (g, r, r.div_ceil(LEARNER_READS))).collect();
    // Busiest first; ties by region order, so every run answers the same.
    owed.sort_by_key(|&(g, r, _)| (std::cmp::Reverse(r), g as u8));
    while !out.is_full() {
        let mut gave = false;
        for (g, _, n) in owed.iter_mut() {
            if *n > 0 && !out.is_full() {
                out.push(*g);
                *n -= 1;
                gave = true;
            }
        }
        if !gave {
            break;
        }
    }
    out
}

/// One row per other voter in each member's own SQLite, outside `GroupConfig` and the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rtt {
    pub peer: SiteId,
    /// Median of 20 binding round trips.
    pub median_ms: u16,
    pub at: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlacementName {
    /// A enam, B weur, C apac, all Full.
    Tri,
    /// enam, wnam, weur, all Full.
    Na3,
    /// enam (us), wnam (us) Full, weur Witness.
    Us2w,
    /// weur (eu), eeur (eu) Full, enam Witness.
    Eu2w,
    /// apac, oc Full, wnam Witness.
    Apac2w,
    /// One Full member on one site. Evaluation only.
    Solo,
}

impl PlacementName {
    /// The voter kinds in record order. The `*2w` placements put their Witness last.
    pub fn kinds(self) -> &'static [MemberKind] {
        use MemberKind::{Full, Witness};
        match self {
            PlacementName::Tri | PlacementName::Na3 => &[Full, Full, Full],
            PlacementName::Us2w | PlacementName::Eu2w | PlacementName::Apac2w => &[Full, Full, Witness],
            PlacementName::Solo => &[Full],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SystemChain {
    Release,
    Names,
    Tokens,
    Ids,
}

/// Where one owner's chains, or one system chain, live. `sites` are logical and in record order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementRecord {
    pub name: PlacementName,
    pub sites: SmallVec<[SiteId; 3]>,
    pub home: SiteId,
    pub generation: u32,
    pub moving_from: Option<Box<PlacementRecord>>,
    pub at: Timestamp,
}

impl PlacementRecord {
    /// The bootstrap voters a caller with no cached config calls, in record order, all at the record's generation.
    pub fn bootstrap_members(&self, chain: ChainId) -> Vec<Member> {
        self.sites
            .iter()
            .zip(self.name.kinds())
            .enumerate()
            .map(|(slot, (&site, &kind))| Member::new(chain, site, kind, slot as u8, self.generation))
            .collect()
    }

    /// The records a lookup tries in order: this one, then each `moving_from` behind it.
    pub fn lookup_order(&self) -> impl Iterator<Item = &PlacementRecord> {
        std::iter::successors(Some(self), |r| r.moving_from.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlacementError {
    #[error("{name:?} needs {want} sites, got {got}")]
    SiteCount { name: PlacementName, want: usize, got: usize },
    #[error("site {0:?} named twice")]
    SameSite(SiteId),
    #[error("sites {0:?} and {1:?} are in one region")]
    SameRegion(SiteId, SiteId),
    #[error("sites {0:?} and {1:?} are in one account, and the install separates accounts")]
    SameAccount(SiteId, SiteId),
    #[error("voters on {0:?} and {1:?} are {2} ms apart, under MIN_PAIR_RTT")]
    TooClose(SiteId, SiteId, u16),
    #[error("no round-trip measurement between {0:?} and {1:?}")]
    Unmeasured(SiteId, SiteId),
    #[error("fewer than two Full voters")]
    TooFewFull,
    #[error("a Solo placement needs the install's signed evaluation flag")]
    SoloNeedsEvaluation,
    #[error("home {0:?} is not a Full voter")]
    BadHome(SiteId),
    #[error("site {0:?} is not in the site table")]
    UnknownSite(SiteId),
    #[error("a learner is not a voter kind, and a voter cannot be a learner")]
    WrongKind,
    #[error("two learners hold slot {0}, or it is not below MAX_LEARNERS")]
    SameSlot(u8),
    #[error("two members on site {0:?} would share the object {1}")]
    SameObject(SiteId, Box<str>),
}

/// What `Placement::new` checks a placement against: the install's sites, whether it separates accounts, and the
/// joining members' `Rtt` rows, each read from the member on site `.0`.
pub struct PlacementChecks<'a> {
    pub sites: &'a SiteTable,
    pub separate_accounts: bool,
    pub rtt: &'a [(SiteId, Rtt)],
}

impl PlacementChecks<'_> {
    fn pair_ms(&self, a: SiteId, b: SiteId) -> Option<u16> {
        self.rtt
            .iter()
            .filter(|(from, r)| (*from == a && r.peer == b) || (*from == b && r.peer == a))
            .map(|(_, r)| r.median_ms)
            .min()
    }
}

/// Built only by `Placement::new` or `Placement::from_record`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    voters: ArrayVec<Member, 3>,
    learners: ArrayVec<Member, 16>,
    /// Preferred leader site, the owner's home; transfer restores it.
    home: SiteId,
    /// Highest member generation this group has named.
    generation: u32,
}

impl Placement {
    /// Refuses two voters in one region or one site, a voter pair under MIN_PAIR_RTT or unmeasured, fewer than two
    /// Full voters, and, when the install separates accounts, two voters in one account.
    pub fn new(
        voters: ArrayVec<Member, 3>,
        learners: ArrayVec<Member, 16>,
        home: SiteId,
        checks: &PlacementChecks,
    ) -> Result<Placement, PlacementError> {
        if voters.iter().any(|m| m.kind == MemberKind::Learner)
            || learners.iter().any(|m| m.kind != MemberKind::Learner)
        {
            return Err(PlacementError::WrongKind);
        }
        if voters.iter().filter(|m| m.kind == MemberKind::Full).count() < 2 {
            return Err(PlacementError::TooFewFull);
        }
        for (i, a) in voters.iter().enumerate() {
            let sa = checks.sites.get(a.site).ok_or(PlacementError::UnknownSite(a.site))?;
            for b in &voters[i + 1..] {
                let sb = checks.sites.get(b.site).ok_or(PlacementError::UnknownSite(b.site))?;
                if a.site == b.site {
                    return Err(PlacementError::SameSite(a.site));
                }
                if sa.region == sb.region {
                    return Err(PlacementError::SameRegion(a.site, b.site));
                }
                if checks.separate_accounts && sa.account == sb.account {
                    return Err(PlacementError::SameAccount(a.site, b.site));
                }
                match checks.pair_ms(a.site, b.site) {
                    None => return Err(PlacementError::Unmeasured(a.site, b.site)),
                    Some(ms) if u128::from(ms) < MIN_PAIR_RTT.as_millis() => {
                        return Err(PlacementError::TooClose(a.site, b.site, ms));
                    }
                    Some(_) => {}
                }
            }
        }
        for (i, l) in learners.iter().enumerate() {
            checks.sites.get(l.site).ok_or(PlacementError::UnknownSite(l.site))?;
            if usize::from(l.slot) >= MAX_LEARNERS || learners[..i].iter().any(|o| o.slot == l.slot) {
                return Err(PlacementError::SameSlot(l.slot));
            }
        }
        // Members on one site share its member class, so two of them with one name would be one object.
        let all: Vec<&Member> = voters.iter().chain(&learners).collect();
        for (i, a) in all.iter().enumerate() {
            if all[..i].iter().any(|b| b.site == a.site && b.do_name == a.do_name) {
                return Err(PlacementError::SameObject(a.site, a.do_name.clone()));
            }
        }
        Self::with_home(voters, learners, home)
    }

    /// The one-voter `Solo` placement: one Full member on a known site. It needs the install's signed `evaluation`
    /// flag (`Genesis.evaluation`), which the caller reads from the `Release` view and passes here.
    pub fn solo(voter: Member, evaluation: bool, checks: &PlacementChecks) -> Result<Placement, PlacementError> {
        if !evaluation {
            return Err(PlacementError::SoloNeedsEvaluation);
        }
        if voter.kind != MemberKind::Full {
            return Err(PlacementError::WrongKind);
        }
        checks.sites.get(voter.site).ok_or(PlacementError::UnknownSite(voter.site))?;
        let home = voter.site;
        Self::with_home(std::iter::once(voter).collect(), ArrayVec::new(), home)
    }

    /// A group's first placement, from its `PlacementRecord` and nothing else. The record's source (the `Names` leader
    /// or the signed genesis) already ran `Placement::new`'s checks when it committed the record.
    pub fn from_record(chain: ChainId, record: &PlacementRecord) -> Result<Placement, PlacementError> {
        let want = record.name.kinds().len();
        if record.sites.len() != want {
            return Err(PlacementError::SiteCount { name: record.name, want, got: record.sites.len() });
        }
        for (i, s) in record.sites.iter().enumerate() {
            if record.sites[i + 1..].contains(s) {
                return Err(PlacementError::SameSite(*s));
            }
        }
        // A Solo record's signed evaluation flag is checked where the record is written (`placement set`, genesis).
        let voters = record.bootstrap_members(chain).into_iter().collect();
        Self::with_home(voters, ArrayVec::new(), record.home)
    }

    fn with_home(
        voters: ArrayVec<Member, 3>,
        learners: ArrayVec<Member, 16>,
        home: SiteId,
    ) -> Result<Placement, PlacementError> {
        if !voters.iter().any(|m| m.site == home && m.kind == MemberKind::Full) {
            return Err(PlacementError::BadHome(home));
        }
        let generation = voters.iter().chain(&learners).map(|m| m.generation).max().unwrap_or(0);
        Ok(Placement { voters, learners, home, generation })
    }

    pub fn voters(&self) -> &[Member] {
        &self.voters
    }
    pub fn learners(&self) -> &[Member] {
        &self.learners
    }
    pub fn home(&self) -> SiteId {
        self.home
    }
    pub fn generation(&self) -> u32 {
        self.generation
    }
    /// The generation a member added by a retry, a replacement, or a move takes, so it never wakes a removed
    /// member's object.
    pub fn next_generation(&self) -> u32 {
        self.generation + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{DbId, RepoId};
    use crate::site::{Region, Site, SiteLink};

    fn site(id: u8, region: Region, account: &str) -> Site {
        Site {
            id: SiteId(id),
            script: format!("graft-site-{id}"),
            account: account.into(),
            region,
            jurisdiction: None,
            bucket: format!("graft-packs-{id}"),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare: false,
        }
    }

    fn table() -> SiteTable {
        SiteTable(vec![
            site(0, Region::Enam, "x"),
            site(1, Region::Weur, "x"),
            site(2, Region::Apac, "x"),
            site(3, Region::Enam, "y"),
        ])
    }

    const REPO: ChainId = ChainId::Repo(RepoId(DbId(42)));

    fn rtt(pairs: &[(u8, u8, u16)]) -> Vec<(SiteId, Rtt)> {
        pairs.iter().map(|&(a, b, ms)| (SiteId(a), Rtt { peer: SiteId(b), median_ms: ms, at: Timestamp(0) })).collect()
    }

    fn voters(sites: &[(u8, MemberKind)]) -> ArrayVec<Member, 3> {
        sites.iter().enumerate().map(|(i, &(s, k))| Member::new(REPO, SiteId(s), k, i as u8, 1)).collect()
    }

    fn new(v: &[(u8, MemberKind)], rtt_rows: &[(SiteId, Rtt)], separate: bool) -> Result<Placement, PlacementError> {
        let t = table();
        let checks = PlacementChecks { sites: &t, separate_accounts: separate, rtt: rtt_rows };
        Placement::new(voters(v), ArrayVec::new(), SiteId(v[0].0), &checks)
    }

    use MemberKind::{Full, Witness};

    #[test]
    fn member_names_separate_kind_slot_and_generation() {
        assert_eq!(&*Member::new(REPO, SiteId(1), Full, 1, 1).do_name, "repo:42#1#f1");
        assert_eq!(&*Member::new(REPO, SiteId(1), MemberKind::Learner, 1, 1).do_name, "repo:42#1#l1");
        assert_eq!(&*Member::new(ChainId::Ids, SiteId(2), Witness, 2, 12).do_name, "ids#12#w2");
    }

    #[test]
    fn new_accepts_tri_at_the_rtt_boundary_and_refuses_just_under_it() {
        let ok = rtt(&[(0, 1, 30), (0, 2, 180), (1, 2, 200)]);
        assert!(new(&[(0, Full), (1, Full), (2, Full)], &ok, false).is_ok());
        // The row may come from either side of the pair; the lower reading counts.
        let low = rtt(&[(0, 1, 80), (1, 0, 29), (0, 2, 180), (2, 1, 200)]);
        assert_eq!(
            new(&[(0, Full), (1, Full), (2, Full)], &low, false),
            Err(PlacementError::TooClose(SiteId(0), SiteId(1), 29))
        );
        let missing = rtt(&[(0, 1, 80), (0, 2, 180)]);
        assert_eq!(
            new(&[(0, Full), (1, Full), (2, Full)], &missing, false),
            Err(PlacementError::Unmeasured(SiteId(1), SiteId(2)))
        );
    }

    #[test]
    fn new_refuses_shared_region_site_and_account() {
        // Sites 0 and 3 are both enam, though far apart by the (bogus) reading: region wins.
        let r = rtt(&[(0, 3, 90), (0, 1, 80), (1, 3, 80)]);
        assert_eq!(
            new(&[(0, Full), (1, Full), (3, Full)], &r, false),
            Err(PlacementError::SameRegion(SiteId(0), SiteId(3)))
        );
        assert_eq!(new(&[(0, Full), (0, Full), (1, Full)], &r, false), Err(PlacementError::SameSite(SiteId(0))));
        let r = rtt(&[(0, 1, 80), (0, 2, 180), (1, 2, 200)]);
        assert_eq!(
            new(&[(0, Full), (1, Full), (2, Full)], &r, true),
            Err(PlacementError::SameAccount(SiteId(0), SiteId(1)))
        );
    }

    #[test]
    fn new_needs_two_full_voters_and_a_full_home() {
        let r = rtt(&[(0, 1, 80), (0, 2, 180), (1, 2, 200)]);
        assert!(new(&[(0, Full), (1, Full), (2, Witness)], &r, false).is_ok());
        assert_eq!(new(&[(0, Full), (1, Witness), (2, Witness)], &r, false), Err(PlacementError::TooFewFull));
        assert_eq!(new(&[(0, Full)], &r, false), Err(PlacementError::TooFewFull));
        assert_eq!(new(&[(2, Witness), (0, Full), (1, Full)], &r, false), Err(PlacementError::BadHome(SiteId(2))));
    }

    fn record(name: PlacementName, sites: &[u8], home: u8, generation: u32) -> PlacementRecord {
        PlacementRecord {
            name,
            sites: sites.iter().map(|&s| SiteId(s)).collect(),
            home: SiteId(home),
            generation,
            moving_from: None,
            at: Timestamp(1),
        }
    }

    #[test]
    fn from_record_builds_the_bootstrap_voters_and_checks_shape_only() {
        let r = record(PlacementName::Us2w, &[3, 4, 1], 4, 7);
        let p = Placement::from_record(REPO, &r).unwrap();
        let names: Vec<&str> = p.voters().iter().map(|m| &*m.do_name).collect();
        assert_eq!(names, ["repo:42#7#f0", "repo:42#7#f1", "repo:42#7#w2"]);
        assert_eq!((p.home(), p.generation(), p.next_generation()), (SiteId(4), 7, 8));
        assert_eq!(
            Placement::from_record(REPO, &record(PlacementName::Tri, &[0, 1], 0, 1)),
            Err(PlacementError::SiteCount { name: PlacementName::Tri, want: 3, got: 2 })
        );
        assert_eq!(
            Placement::from_record(REPO, &record(PlacementName::Tri, &[0, 1, 0], 0, 1)),
            Err(PlacementError::SameSite(SiteId(0)))
        );
        // The witness slot cannot be home.
        assert_eq!(
            Placement::from_record(REPO, &record(PlacementName::Eu2w, &[1, 5, 0], 0, 1)),
            Err(PlacementError::BadHome(SiteId(0)))
        );
        assert!(Placement::from_record(REPO, &record(PlacementName::Solo, &[0], 0, 1)).is_ok());
    }

    fn learners(at: &[(u8, u8)]) -> ArrayVec<Member, 16> {
        at.iter().map(|&(s, slot)| Member::new(REPO, SiteId(s), MemberKind::Learner, slot, 1)).collect()
    }

    /// BF-28's naming half: a learner on its voter's site and two learners on one site are distinct objects with
    /// distinct addresses, and a placement that would name one object twice is refused.
    #[test]
    fn learners_beside_a_voter_and_on_one_site_are_distinct_objects() {
        let t = table();
        let r = rtt(&[(0, 1, 80), (0, 2, 180), (1, 2, 200)]);
        let checks = PlacementChecks { sites: &t, separate_accounts: false, rtt: &r };
        let v = voters(&[(0, Full), (1, Full), (2, Full)]);
        let p = Placement::new(v.clone(), learners(&[(0, 0), (1, 1), (1, 2)]), SiteId(0), &checks).unwrap();
        let all: Vec<&Member> = p.voters().iter().chain(p.learners()).collect();
        let names: std::collections::BTreeSet<(SiteId, &str)> = all.iter().map(|m| (m.site, &*m.do_name)).collect();
        let addrs: std::collections::BTreeSet<SiteId> = all.iter().map(|m| m.addr()).collect();
        assert_eq!((names.len(), addrs.len()), (6, 6), "{all:?}");
        assert_eq!(&*p.learners()[0].do_name, "repo:42#1#l0");
        assert_eq!(p.learners()[0].addr(), learner_addr(0));
        assert_eq!(learner_slot(p.learners()[2].addr()), Some(2));
        assert_eq!(learner_slot(SiteId(2)), None);
        assert_eq!(
            Placement::new(v.clone(), learners(&[(1, 1), (2, 1)]), SiteId(0), &checks),
            Err(PlacementError::SameSlot(1))
        );
        assert_eq!(
            Placement::new(v, learners(&[(1, MAX_LEARNERS as u8)]), SiteId(0), &checks),
            Err(PlacementError::SameSlot(MAX_LEARNERS as u8))
        );
    }

    fn load(r: &[(Region, u32)]) -> GroupLoad {
        GroupLoad { reads: r.iter().copied().collect() }
    }

    #[test]
    fn place_learners_adds_none_below_learner_at_and_spreads_by_rate_up_to_max() {
        assert!(place_learners(&load(&[(Region::Enam, 300), (Region::Weur, 199)])).is_empty());
        assert_eq!(
            place_learners(&load(&[(Region::Enam, 300), (Region::Weur, 200)])).as_slice(),
            [Region::Enam, Region::Weur]
        );
        // 2,500/s in apac needs three, 1,200/s in weur two, 100/s in enam one: busiest first, one round at a time.
        let got = place_learners(&load(&[(Region::Enam, 100), (Region::Weur, 1_200), (Region::Apac, 2_500)]));
        use Region::{Apac, Enam, Weur};
        assert_eq!(got.as_slice(), [Apac, Weur, Enam, Apac, Weur, Apac]);
        // B0-18's 16,000 reads/s: capped at MAX_LEARNERS.
        let got = place_learners(&load(&[(Enam, 6_000), (Weur, 5_000), (Apac, 9_000)]));
        assert_eq!(got.len(), MAX_LEARNERS);
        assert_eq!(got.iter().filter(|r| **r == Apac).count(), 6);
    }

    #[test]
    fn lookup_order_walks_moving_from() {
        let mut new_rec = record(PlacementName::Tri, &[0, 1, 2], 0, 3);
        let mut old = record(PlacementName::Na3, &[0, 5, 1], 0, 2);
        old.moving_from = Some(Box::new(record(PlacementName::Tri, &[0, 1, 2], 0, 1)));
        new_rec.moving_from = Some(Box::new(old));
        let gens: Vec<u32> = new_rec.lookup_order().map(|r| r.generation).collect();
        assert_eq!(gens, [3, 2, 1]);
    }
}
