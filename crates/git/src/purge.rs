//! A deleted repository's purge (`design/SHARED.md`, "Repository repair, tombstones, and retention", Purge;
//! `decisions/DECISIONS.md`, "`repo-purge`"): the pure gate, the retire plan, and the bucket pass that the edge's
//! `POST /api/graftyard/v1/admin/repos/{id}/purge` runs.
//!
//! - **Gate** (`gate`). Only a repository whose delete committed is purged: its owner's `Authz` names it `Deleted`,
//!   and its `retain_until` has passed unless the site admin asked for an early purge (`early`). Per chain, the
//!   newest committed config `C` any joined member answers decides who is checked (`decisions/DECISIONS.md`,
//!   "`purge-outside`"): every member must answer, every voter and learner of `C` must be in the plan, each joined one
//!   must hold the chain's `Deleted` tombstone in its own fold (`/status` `purgeable`; a metadata child's member holds
//!   no core tier and passes on its parent's), and none may be in a group change (`changing`). A member outside `C`
//!   (a voter a replacement removed, a `Degraded` config's lost voter, a registry row's member that never received a
//!   config) is not checked. The rule reads each member, not the group, so a purge cut after it retired a majority,
//!   which leaves a group that cannot answer, still passes when it is called again.
//! - **Retire** (`retire_plan`, then the gate's order). Every member a site's registry row or the newest committed
//!   config names, voters and learners, for the repository chain and its metadata child, is sent `/purge_retire`,
//!   outside members first with `{"outside": I}`, `C`'s index, and `C`'s members after. A member checks its own rule
//!   again before it deletes anything (`outside_may_retire`, else `purgeable` and `changing`), so a change that starts
//!   after the gate read is not cut, and a member of any other family deletes nothing.
//! - **Purge** (`purge_bucket`). Every key under the network's prefixes in one site's bucket is listed and deleted,
//!   and listed again. The caller runs it on every site, and again once the sweep runs that started before the first
//!   pass have ended (`second_pass_at`), since one re-put markers on m43 (R25).

use graftyard_raft::GroupConfig;
use graftyard_types::{ChainId, NetworkId, RepoId, SiteId, Timestamp};

use crate::bucket::{Bucket, StoreError};

/// Why a purge is refused before any member is retired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The owner's `Authz` does not name the repository `Deleted`: it is live, or was never this owner's.
    NotDeleted,
    /// Within the 90-day retention (`retain_until`), and the caller did not ask for an early purge.
    Retained { until: Timestamp },
    /// A joined member's fold holds no tombstone: `Audit` still owes it, or the member has not applied it yet.
    NoTombstone { chain: ChainId, member: Box<str> },
    /// A member is joined to a group change (`changing`).
    Changing { chain: ChainId, member: Box<str> },
    /// A member did not answer its status, so the gate cannot tell whether it is changing or retired.
    Unanswered { chain: ChainId, member: Box<str> },
}

/// One member's `/status`, as the gate reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NotJoined,
    /// Joined: whether its fold holds the tombstone (`purgeable`), whether it is in a group change (`changing`), and
    /// its committed config with that config's index (`config`; `None` before its bootstrap entry commits).
    Joined {
        /// Its commit index: the highest a joined member answers is what the feed must carry before the retire
        /// (`decisions/DECISIONS.md`, "`feed-retired`").
        commit: u64,
        purgeable: bool,
        changing: bool,
        config: Option<(u64, GroupConfig)>,
    },
    /// No answer, or an error.
    Unanswered,
}

/// One member to retire, in the order the gate gives: `at` indexes `Facts::members`, and `outside` is `C`'s index
/// for a member outside the chain's newest committed config, which `/purge_retire` receives as `{"outside": I}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retire {
    pub at: usize,
    pub outside: Option<u64>,
}

/// What the gate reads.
pub struct Facts<'a> {
    /// The `retain_until` of the owner's `Authz` name that holds the repository `Deleted` (`AuthzFold::deleted`).
    pub deleted: Option<Timestamp>,
    pub now: Timestamp,
    /// The site admin asked to purge before `retain_until`.
    pub early: bool,
    /// Every member the plan retires, with its status.
    pub members: &'a [(ChainId, SiteId, Box<str>, Status)],
}

/// `Ok` with the retire order when the purge may retire the members and delete the network's objects: every member
/// outside its chain's newest committed config first, then the config's own (`decisions/DECISIONS.md`,
/// "`purge-outside`"). Outside first, so a purge cut between the two steps finds `C` from the in-config members when
/// it is called again; in-config first would leave a stale member alone, whose old config the rerun would take as
/// `C`, and refuse it forever.
pub fn gate(f: &Facts<'_>) -> Result<Vec<Retire>, Refused> {
    let until = f.deleted.ok_or(Refused::NotDeleted)?;
    if f.now < until && !f.early {
        return Err(Refused::Retained { until });
    }
    let mut outside = Vec::new();
    let mut inside = Vec::new();
    let mut chains: Vec<ChainId> = Vec::new();
    for (c, ..) in f.members {
        if !chains.contains(c) {
            chains.push(*c);
        }
    }
    for chain in chains {
        let of: Vec<usize> = (0..f.members.len()).filter(|i| f.members[*i].0 == chain).collect();
        let newest = of
            .iter()
            .filter_map(|i| match &f.members[*i].3 {
                Status::Joined { config: Some((x, c)), .. } => Some((*x, c)),
                _ => None,
            })
            .max_by_key(|(x, _)| *x);
        if let Some((_, c)) = newest {
            // Every voter and learner of `C` answers, so a quorum of it does: one the plan missed is not checked.
            for m in config_members(c) {
                if !of.iter().any(|i| *f.members[*i].2 == *m) {
                    return Err(Refused::Unanswered { chain, member: m.into() });
                }
            }
        }
        for i in of {
            let (_, _, member, s) = &f.members[i];
            let is_in = newest.is_none_or(|(_, c)| in_config(c, member));
            match s {
                Status::Unanswered => return Err(Refused::Unanswered { chain, member: member.clone() }),
                Status::Joined { .. } if !is_in => {
                    outside.push(Retire { at: i, outside: newest.map(|(x, _)| x) });
                    continue;
                }
                Status::Joined { changing: true, .. } => {
                    return Err(Refused::Changing { chain, member: member.clone() });
                }
                Status::Joined { purgeable: false, .. } => {
                    return Err(Refused::NoTombstone { chain, member: member.clone() });
                }
                Status::NotJoined | Status::Joined { .. } => {}
            }
            inside.push(Retire { at: i, outside: None });
        }
    }
    outside.extend(inside);
    Ok(outside)
}

/// Whether `config` names the member `name` as one of the group's: a voter that counts (`quorum::voters`, so not a
/// `Degraded` config's lost voter) or a learner, in either half of a `Joint`.
pub fn in_config(config: &GroupConfig, name: &str) -> bool {
    config_members(config).contains(&name)
}

fn config_members(c: &GroupConfig) -> Vec<&str> {
    let (halves, lost): (Vec<&graftyard_types::Placement>, _) = match c {
        GroupConfig::Normal(p) => (vec![&**p], None),
        GroupConfig::Degraded { placement, lost } => (vec![&**placement], Some(*lost)),
        GroupConfig::Joint { old, new } => (vec![&**old, &**new], None),
    };
    let voters = halves.iter().flat_map(|p| p.voters()).filter(|m| Some(m.site) != lost);
    voters.chain(halves.iter().flat_map(|p| p.learners())).map(|m| &*m.do_name).collect()
}

/// The member's own rule for `/purge_retire` with `{"outside": I}`: it may be retired unchecked only if its committed
/// config `own` (with its index) is older than `I`, absent, or does not name it. A member that caught up into `C`, or
/// past it into a config that names it, after the gate read is held to `C`'s rule (`purgeable` and `changing`).
pub fn outside_may_retire(own: Option<(u64, &GroupConfig)>, me: &str, i: u64) -> bool {
    match own {
        None => true,
        Some((at, c)) => at < i || !in_config(c, me),
    }
}

/// Whether a purge may retire a member holding `fold`, its `/status` field `purgeable`: its own fold holds the
/// repository's `Deleted` tombstone, so a member behind that entry answers `false` and the gate refuses; or it is a
/// metadata child (`repo-meta:N`), which holds no core tier and is purged on its parent's.
pub fn purgeable(fold: &crate::repo::RepoFold) -> bool {
    fold.child || crate::tiers::deleted(&*fold.core).is_some()
}

/// Whether a member is in a group change, so a purge must not retire it: its committed config is `Joint`, its log
/// holds a config past the committed one (a change appended and not yet committed, or a bootstrap not yet committed),
/// or a placement runs on it (`placing`, the daily check's and `place_background`'s marker).
pub fn changing(committed: Option<&GroupConfig>, latest: &GroupConfig, placing: bool) -> bool {
    placing || matches!(committed, Some(GroupConfig::Joint { .. })) || committed != Some(latest)
}

/// The network a repository's packs live under: a network id is its root repository's id, and no route opens a fork
/// yet, so every network holds one repository (`workers/site/src/sweep.rs`, `home_site`). A fork route must make the
/// purge refuse a network another repository still uses (`decisions/DECISIONS.md`, "`repo-purge`", Flip).
pub fn network(repo: RepoId) -> NetworkId {
    NetworkId(repo.0)
}

/// The chains a repository's purge retires: the repository chain and its metadata child.
pub fn chains(repo: RepoId) -> [ChainId; 2] {
    [ChainId::Repo(repo), ChainId::RepoMeta(repo)]
}

/// Every member to retire for a chain: the bootstrap members of each record in the owner's lookup order, each site's
/// registry row, and every voter and learner of the newest committed config any of them answered, once each, in that
/// order. Old retire lists named members by the generation they had when listed, and a later voter generation stayed
/// joined (131336, 131346).
pub fn retire_plan(
    bootstrap: &[(SiteId, Box<str>)],
    rows: &[(SiteId, Box<str>)],
    config: &[(SiteId, Box<str>)],
) -> Vec<(SiteId, Box<str>)> {
    let mut out: Vec<(SiteId, Box<str>)> = Vec::new();
    for (s, n) in bootstrap.iter().chain(rows).chain(config).cloned() {
        if !out.iter().any(|(_, m)| *m == n) {
            out.push((s, n));
        }
    }
    out
}

/// Every key prefix one network's objects live under (`crate::key::R2Key`'s `Display`). The trailing `/` keeps
/// network 13132 from matching 131322.
pub fn prefixes(network: NetworkId) -> Vec<String> {
    let n = network.0.0;
    let by_net = ["pack", "committed/pack", "upload", "compose", "clone", "archive-cache", "blob", "committed/blob"];
    let by_chain = ["archive", "snapshot", "inline-idx"];
    let mut out: Vec<String> = by_net.iter().map(|p| format!("{p}/{n}/")).collect();
    for p in by_chain {
        for c in ["repo", "repo-meta"] {
            out.push(format!("{p}/{c}:{n}/"));
        }
    }
    out
}

/// One bucket's pass: every key under the network's prefixes deleted, 1,000 at a time. Returns (found, left after).
pub async fn purge_bucket<B: Bucket>(b: &B, network: NetworkId) -> Result<(usize, usize), StoreError> {
    let mut found = 0;
    for p in prefixes(network) {
        let keys: Vec<String> = b.list(&p).await?.into_iter().map(|o| o.key).collect();
        found += keys.len();
        for chunk in keys.chunks(1_000) {
            b.delete(chunk).await?;
        }
    }
    let mut left = 0;
    for p in prefixes(network) {
        left += b.list(&p).await?.len();
    }
    Ok((found, left))
}

/// Site sweeps start at */15-minute cron boundaries (`workers/site/src/lib.rs`, `scheduled`).
pub const SWEEP_SLOT_MS: u64 = 900_000;
/// Work already in flight when `crate::sweep::SWEEP_WALL` expires finishes within this.
pub const SWEEP_SETTLE_MS: u64 = 120_000;

/// When every sweep run that started at or before `t` has ended: the latest such run started at `t`'s */15 boundary,
/// stops starting work `SWEEP_WALL` later, and finishes what it started within `SWEEP_SETTLE_MS`.
pub fn second_pass_at(t: Timestamp) -> Timestamp {
    Timestamp(t.0 / SWEEP_SLOT_MS * SWEEP_SLOT_MS + crate::sweep::SWEEP_WALL + SWEEP_SETTLE_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket::Bucket;
    use crate::mem::MemBucket;
    use futures_executor::block_on;
    use graftyard_types::{DbId, Digest};
    use sha2::{Digest as _, Sha256};

    const REPO: RepoId = RepoId(DbId(65567));

    fn put(b: &MemBucket, k: &str) {
        let sum = Digest(Sha256::digest(b"x").into());
        block_on(b.put_new(k, b"x", sum, &[])).unwrap();
    }

    const NOW: Timestamp = Timestamp(1_000_000);

    fn gate_on(deleted: bool, members: &[(ChainId, SiteId, Box<str>, Status)]) -> Result<(), Refused> {
        let deleted = deleted.then_some(Timestamp(NOW.0 - 1));
        gate(&Facts { deleted, now: NOW, early: false, members }).map(|_| ())
    }

    fn joined(purgeable: bool, changing: bool) -> Status {
        Status::Joined { commit: 0, purgeable, changing, config: None }
    }

    /// Within the retention a purge needs `early`; at `retain_until` and after it needs nothing more.
    #[test]
    fn a_purge_inside_the_retention_needs_early() {
        let members = [(ChainId::Repo(REPO), SiteId(0), Box::<str>::from("repo:65567#1#f0"), Status::NotJoined)];
        let until = Timestamp(NOW.0 + 1);
        let f = |now: Timestamp, early: bool| {
            gate(&Facts { deleted: Some(until), now, early, members: &members }).map(|_| ())
        };
        assert_eq!(f(NOW, false), Err(Refused::Retained { until }));
        assert_eq!(f(NOW, true), Ok(()));
        assert_eq!(f(until, false), Ok(()));
    }

    /// The gate refuses a live repository, a joined member whose fold holds no tombstone, a member in a group change,
    /// and a member that does not answer. A purge cut after it retired two of three voters, whose group can no longer
    /// answer, passes when called again: the third member's own fold holds the tombstone.
    #[test]
    fn the_gate_purges_only_a_committed_delete_with_no_group_change() {
        let c = ChainId::Repo(REPO);
        let name: Box<str> = "repo:65567#6#l3".into();
        let one = |s: Status| vec![(c, SiteId(1), name.clone(), s)];
        let quiet = joined(true, false);
        assert_eq!(gate_on(true, &one(quiet.clone())), Ok(()));
        assert_eq!(gate_on(true, &one(Status::NotJoined)), Ok(()));
        assert_eq!(gate_on(false, &one(quiet.clone())), Err(Refused::NotDeleted));
        assert_eq!(gate_on(false, &one(Status::NotJoined)), Err(Refused::NotDeleted));
        let unpurgeable = joined(false, false);
        assert_eq!(gate_on(true, &one(unpurgeable)), Err(Refused::NoTombstone { chain: c, member: name.clone() }));
        let moving = joined(true, true);
        assert_eq!(gate_on(true, &one(moving)), Err(Refused::Changing { chain: c, member: name.clone() }));
        assert_eq!(gate_on(true, &one(Status::Unanswered)), Err(Refused::Unanswered { chain: c, member: name }));
        let cut: Vec<_> = [Status::NotJoined, Status::NotJoined, quiet]
            .into_iter()
            .enumerate()
            .map(|(i, s)| (c, SiteId(i as u8), format!("repo:65567#1#f{i}").into_boxed_str(), s))
            .collect();
        assert_eq!(gate_on(true, &cut), Ok(()), "a cut purge called again");
    }

    /// `purge-outside`'s chain: generation 1 on sites 0, 1, 2, then site 1 replaced by `#4#f1` through `Degraded` and
    /// `Normal`, committed at index 9 (graft-m4a's 65543, before its lost voter: 23 chains held this shape).
    fn replaced() -> (GroupConfig, GroupConfig) {
        let chain = ChainId::Repo(REPO);
        let m = |s: u8, g: u32| graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Full, s, g);
        let p = |v1: u32| -> Box<graftyard_types::Placement> {
            let raw = serde_json::json!({ "voters": [m(0, 1), m(1, v1), m(2, 1)], "learners": [], "home": 0,
                "generation": v1 });
            Box::new(serde_json::from_value(raw).unwrap())
        };
        (GroupConfig::Normal(p(1)), GroupConfig::Normal(p(4)))
    }

    fn at(config: &GroupConfig, x: u64, purgeable: bool) -> Status {
        Status::Joined { commit: 0, purgeable, changing: false, config: Some((x, config.clone())) }
    }

    fn row(s: u8, name: &str, st: Status) -> (ChainId, SiteId, Box<str>, Status) {
        (ChainId::Repo(REPO), SiteId(s), name.into(), st)
    }

    fn order(members: &[(ChainId, SiteId, Box<str>, Status)]) -> Result<Vec<(String, Option<u64>)>, Refused> {
        let d = Some(Timestamp(NOW.0 - 1));
        let o = gate(&Facts { deleted: d, now: NOW, early: false, members })?;
        Ok(o.into_iter().map(|r| (members[r.at].2.to_string(), r.outside)).collect())
    }

    /// R27, fails on main: the first-generation site-1 voter a replacement removed stays joined at its old config
    /// (applied 1 on graft-m4a) and never gets the tombstone. The newest config's voters hold it, so the purge
    /// passes, and the stale voter is retired first with `C`'s index.
    #[test]
    fn a_voter_a_replacement_removed_is_retired_unchecked_and_first() {
        let (old, new) = replaced();
        let members = [
            row(0, "repo:65567#1#f0", at(&new, 9, true)),
            row(1, "repo:65567#1#f1", at(&old, 1, false)),
            row(2, "repo:65567#1#f2", at(&new, 9, true)),
            row(1, "repo:65567#4#f1", at(&new, 9, true)),
        ];
        let want = [
            ("repo:65567#1#f1", Some(9)),
            ("repo:65567#1#f0", None),
            ("repo:65567#1#f2", None),
            ("repo:65567#4#f1", None),
        ];
        assert_eq!(order(&members), Ok(want.map(|(n, o)| (n.to_string(), o)).to_vec()));
        // The same voter answering at the newest config is in it, and its missing tombstone refuses.
        let mut behind = members.clone();
        behind[3] = row(1, "repo:65567#4#f1", at(&new, 9, false));
        assert_eq!(
            order(&behind),
            Err(Refused::NoTombstone { chain: ChainId::Repo(REPO), member: "repo:65567#4#f1".into() })
        );
    }

    /// R27, fails on main: a registry row's member that joined for a placement try and never received a config
    /// (`config: null`, applied 0; 13 chains on graft-m4a) reports `changing`, since its bootstrap config never
    /// committed. It is outside `C`, so it is retired unchecked, first.
    #[test]
    fn a_registry_member_that_never_received_a_config_is_retired_unchecked() {
        let (_, new) = replaced();
        let members = [
            row(0, "repo:65567#1#f0", at(&new, 9, true)),
            row(2, "repo:65567#1#f2", at(&new, 9, true)),
            row(1, "repo:65567#4#f1", at(&new, 9, true)),
            row(1, "repo:65567#11#f1", joined(false, true)),
        ];
        let o = order(&members).unwrap();
        assert_eq!(o[0], ("repo:65567#11#f1".to_string(), Some(9)));
        assert_eq!(o.len(), 4);
    }

    /// The lead's condition 1: the outside members retired, then the purge cut before any in-config member was
    /// retired. Called again, the gate finds `C` from the in-config members and passes, retiring only them.
    #[test]
    fn a_purge_cut_after_the_outside_step_passes_when_called_again() {
        let (_, new) = replaced();
        let members = [
            row(0, "repo:65567#1#f0", at(&new, 9, true)),
            row(1, "repo:65567#1#f1", Status::NotJoined),
            row(2, "repo:65567#1#f2", at(&new, 9, true)),
            row(1, "repo:65567#4#f1", at(&new, 9, true)),
            row(1, "repo:65567#11#f1", Status::NotJoined),
        ];
        let o = order(&members).unwrap();
        assert!(o.iter().all(|(_, outside)| outside.is_none()), "only in-config members are left: {o:?}");
        // Cut again after one in-config voter: the other two still answer `C` and hold the tombstone.
        let mut cut = members.clone();
        cut[0].3 = Status::NotJoined;
        assert!(order(&cut).is_ok());
    }

    /// `C` named a voter the plan did not read, or one that does not answer: the gate cannot tell whether that member
    /// holds the tombstone, so it refuses `Unanswered` and names it.
    #[test]
    fn every_member_of_the_newest_config_must_answer() {
        let (_, new) = replaced();
        let c = ChainId::Repo(REPO);
        let missing = [row(0, "repo:65567#1#f0", at(&new, 9, true)), row(2, "repo:65567#1#f2", at(&new, 9, true))];
        assert_eq!(order(&missing), Err(Refused::Unanswered { chain: c, member: "repo:65567#4#f1".into() }));
        let silent = [
            row(0, "repo:65567#1#f0", at(&new, 9, true)),
            row(2, "repo:65567#1#f2", at(&new, 9, true)),
            row(1, "repo:65567#4#f1", Status::Unanswered),
        ];
        assert_eq!(order(&silent), Err(Refused::Unanswered { chain: c, member: "repo:65567#4#f1".into() }));
    }

    /// A `Degraded` config's lost voter counts toward no quorum (`quorum::voters`), so it is outside `C` (8 of the
    /// graft-m4a chains); a config appended but not committed on an in-config member still refuses.
    #[test]
    fn a_degraded_config_s_lost_voter_is_outside_and_an_in_config_change_refuses() {
        let (old, _) = replaced();
        let GroupConfig::Normal(p) = old.clone() else { unreachable!() };
        let degraded = GroupConfig::Degraded { placement: p, lost: SiteId(1) };
        let members = [
            row(0, "repo:65567#1#f0", at(&degraded, 5, true)),
            row(1, "repo:65567#1#f1", at(&old, 1, false)),
            row(2, "repo:65567#1#f2", at(&degraded, 5, true)),
        ];
        assert_eq!(order(&members).unwrap()[0], ("repo:65567#1#f1".to_string(), Some(5)));
        let mut moving = members.clone();
        moving[2].3 =
            Status::Joined { commit: 0, purgeable: true, changing: true, config: Some((5, degraded.clone())) };
        assert_eq!(
            order(&moving),
            Err(Refused::Changing { chain: ChainId::Repo(REPO), member: "repo:65567#1#f2".into() })
        );
    }

    /// The member's own check for `{"outside": I}`: unchecked only while its committed config is older than `I`,
    /// absent, or does not name it.
    #[test]
    fn a_member_is_retired_unchecked_only_while_it_is_still_outside() {
        let (old, new) = replaced();
        assert!(outside_may_retire(None, "repo:65567#11#f1", 9), "no config");
        assert!(outside_may_retire(Some((1, &old)), "repo:65567#1#f1", 9), "an older config");
        assert!(outside_may_retire(Some((9, &new)), "repo:65567#1#f1", 9), "C does not name it");
        assert!(!outside_may_retire(Some((9, &new)), "repo:65567#4#f1", 9), "C names it");
        assert!(!outside_may_retire(Some((12, &new)), "repo:65567#4#f1", 9), "a later config names it");
    }

    /// A member is changing under a committed `Joint`, under a config appended past the committed one, or while a
    /// placement runs; a committed `Normal` or `Degraded` with nothing past it is not.
    #[test]
    fn a_member_is_changing_in_a_joint_an_uncommitted_change_or_a_placement() {
        let chain = ChainId::Repo(REPO);
        let p = |g: u32| -> Box<graftyard_types::Placement> {
            let voters: Vec<_> = (0..3u8)
                .map(|s| graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Full, s, g))
                .collect();
            let raw = serde_json::json!({ "voters": voters, "learners": [], "home": 0, "generation": g });
            Box::new(serde_json::from_value(raw).unwrap())
        };
        let normal = GroupConfig::Normal(p(1));
        let degraded = GroupConfig::Degraded { placement: p(1), lost: SiteId(2) };
        let joint = GroupConfig::Joint { old: p(1), new: p(2) };
        assert!(!changing(Some(&normal), &normal, false));
        assert!(!changing(Some(&degraded), &degraded, false));
        assert!(changing(Some(&normal), &normal, true), "a placement runs");
        assert!(changing(Some(&joint), &joint, false), "a committed joint");
        assert!(changing(Some(&normal), &joint, false), "a change appended, not committed");
        assert!(changing(None, &normal, false), "the bootstrap config not committed yet");
    }

    /// 65567's case on graft-m4a: the first-generation voters, the rows' later generation, and the config's learners
    /// are each retired once.
    #[test]
    fn the_plan_names_every_generation_and_learner_once() {
        let bootstrap: Vec<(SiteId, Box<str>)> =
            (0..3u8).map(|s| (SiteId(s), format!("repo:65567#1#f{s}").into_boxed_str())).collect();
        let rows: Vec<(SiteId, Box<str>)> =
            vec![(SiteId(0), "repo:65567#1#f0".into()), (SiteId(1), "repo:65567#5#f1".into())];
        let config: Vec<(SiteId, Box<str>)> = vec![
            (SiteId(0), "repo:65567#1#f0".into()),
            (SiteId(1), "repo:65567#5#f1".into()),
            (SiteId(2), "repo:65567#1#f2".into()),
            (SiteId(0), "repo:65567#6#l3".into()),
        ];
        let plan: Vec<String> = retire_plan(&bootstrap, &rows, &config).into_iter().map(|(_, n)| n.into()).collect();
        assert_eq!(
            plan,
            ["repo:65567#1#f0", "repo:65567#1#f1", "repo:65567#1#f2", "repo:65567#5#f1", "repo:65567#6#l3"]
        );
    }

    /// A pass deletes every key of the network, under every prefix, and nothing of network 655671, whose id starts
    /// with this one's.
    #[test]
    fn a_pass_deletes_the_network_and_only_it() {
        let b = MemBucket::default();
        let mine = [
            "pack/65567/a.pack",
            "committed/pack/65567/a",
            "snapshot/repo:65567/core/x",
            "snapshot/repo-meta:65567/metadata/y",
            "archive/repo:65567/z",
            "blob/65567/q",
        ];
        let other = ["pack/655671/a.pack", "snapshot/repo:655671/core/x", "pack/65536/b.pack"];
        for k in mine.iter().chain(other.iter()) {
            put(&b, k);
        }
        for i in 0..1_500 {
            put(&b, &format!("clone/65567/{i:05}/0"));
        }
        assert_eq!(block_on(purge_bucket(&b, network(REPO))).unwrap(), (mine.len() + 1_500, 0));
        assert_eq!(
            b.keys(),
            other
                .iter()
                .map(|k| k.to_string())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_second_pass_waits_out_the_sweeps_started_before_the_first() {
        // First pass at 22:07:00Z: the 22:00 sweep stops starting work at 22:08 and has settled by 22:10.
        let t = Timestamp(1_794_952_820_000);
        let slot = t.0 / SWEEP_SLOT_MS * SWEEP_SLOT_MS;
        assert_eq!(second_pass_at(t), Timestamp(slot + 8 * 60_000 + 120_000));
        assert!(second_pass_at(t) > t);
    }
}
