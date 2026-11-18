//! A deleted repository's purge (`design/SHARED.md`, "Repository repair, tombstones, and retention", Purge;
//! `decisions/DECISIONS.md`, "`repo-purge`"): the pure gate, the retire plan, and the bucket pass that the edge's
//! `POST /api/graftyard/v1/admin/repos/{id}/purge` runs.
//!
//! - **Gate** (`gate`). Only a repository whose delete committed is purged: its owner's `Authz` names it `Deleted`,
//!   its `retain_until` has passed unless the site admin asked for an early purge (`early`), and every member still joined holds the chain's `Deleted` tombstone in its own fold (`/status` `purgeable`; a
//!   metadata child's member holds no core tier and passes on its parent's). No member may be in a group change
//!   (`changing`), and every member must answer. The rule reads each member, not the group, so a purge cut after it
//!   retired a majority, which leaves a group that cannot answer, still passes when it is called again.
//! - **Retire** (`retire_plan`). Every member a site's registry row or the newest committed config names, voters and
//!   learners, for the repository chain and its metadata child, is sent `/purge_retire`, which deletes its storage.
//!   The member checks `purgeable` and `changing` again itself, so a change that starts after the gate read is not
//!   cut, and a member of any other family deletes nothing.
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
    /// Joined: whether its fold holds the tombstone (`purgeable`), and whether it is in a group change (`changing`).
    Joined {
        purgeable: bool,
        changing: bool,
    },
    /// No answer, or an error.
    Unanswered,
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

/// `Ok` when the purge may retire the members and delete the network's objects.
pub fn gate(f: &Facts<'_>) -> Result<(), Refused> {
    let until = f.deleted.ok_or(Refused::NotDeleted)?;
    if f.now < until && !f.early {
        return Err(Refused::Retained { until });
    }
    for (chain, _, member, s) in f.members {
        let (chain, member) = (*chain, member.clone());
        match *s {
            Status::NotJoined | Status::Joined { purgeable: true, changing: false } => {}
            Status::Joined { changing: true, .. } => return Err(Refused::Changing { chain, member }),
            Status::Joined { purgeable: false, .. } => return Err(Refused::NoTombstone { chain, member }),
            Status::Unanswered => return Err(Refused::Unanswered { chain, member }),
        }
    }
    Ok(())
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
        gate(&Facts { deleted, now: NOW, early: false, members })
    }

    /// Within the retention a purge needs `early`; at `retain_until` and after it needs nothing more.
    #[test]
    fn a_purge_inside_the_retention_needs_early() {
        let members = [(ChainId::Repo(REPO), SiteId(0), Box::<str>::from("repo:65567#1#f0"), Status::NotJoined)];
        let until = Timestamp(NOW.0 + 1);
        let f = |now: Timestamp, early: bool| gate(&Facts { deleted: Some(until), now, early, members: &members });
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
        let quiet = Status::Joined { purgeable: true, changing: false };
        assert_eq!(gate_on(true, &one(quiet.clone())), Ok(()));
        assert_eq!(gate_on(true, &one(Status::NotJoined)), Ok(()));
        assert_eq!(gate_on(false, &one(quiet.clone())), Err(Refused::NotDeleted));
        assert_eq!(gate_on(false, &one(Status::NotJoined)), Err(Refused::NotDeleted));
        let unpurgeable = Status::Joined { purgeable: false, changing: false };
        assert_eq!(gate_on(true, &one(unpurgeable)), Err(Refused::NoTombstone { chain: c, member: name.clone() }));
        let moving = Status::Joined { purgeable: true, changing: true };
        assert_eq!(gate_on(true, &one(moving)), Err(Refused::Changing { chain: c, member: name.clone() }));
        assert_eq!(gate_on(true, &one(Status::Unanswered)), Err(Refused::Unanswered { chain: c, member: name }));
        let cut: Vec<_> = [Status::NotJoined, Status::NotJoined, quiet]
            .into_iter()
            .enumerate()
            .map(|(i, s)| (c, SiteId(i as u8), format!("repo:65567#1#f{i}").into_boxed_str(), s))
            .collect();
        assert_eq!(gate_on(true, &cut), Ok(()), "a cut purge called again");
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
