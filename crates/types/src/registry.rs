//! `MemberRegistry` (`design/DESIGN.md`, Shape, "Release group", the registry paragraph).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use smallvec::SmallVec;

use crate::chain::ChainId;
use crate::site::SiteId;

/// 64 shards by chain hash per site [guess].
pub const REGISTRY_SHARDS: u8 = 64;

/// One per (site, shard) on each site script. A member writes its row before it stores its first entry, and rewrites
/// it on each config change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemberRegistry {
    pub site: SiteId,
    pub shard: u8,
}

impl MemberRegistry {
    pub fn for_chain(site: SiteId, chain: ChainId) -> MemberRegistry {
        let h = Sha256::digest(chain.to_string().as_bytes());
        MemberRegistry { site, shard: h[0] % REGISTRY_SHARDS }
    }

    /// The registry's Durable Object name on its site's registry class.
    pub fn do_name(&self) -> String {
        format!("registry#{}", self.shard)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryRow {
    pub chain: ChainId,
    pub do_name: Box<str>,
    pub generation: u32,
    pub sites: SmallVec<[SiteId; 3]>,
}

/// One site's registry answer for a chain, as `repo-gone` reads it (`decisions/DECISIONS.md`, "`repo-gone`"): a row, a
/// definite no row (404), or no answer (any other status, or none within the lookup's `CALL`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowRead {
    Row,
    NoRow,
    Failed,
}

impl RowRead {
    /// A `GET /row` answer's status: 200 a row, 404 none, anything else or no answer a read that failed.
    pub fn of(status: Option<u16>) -> RowRead {
        match status {
            Some(200) => RowRead::Row,
            Some(404) => RowRead::NoRow,
            _ => RowRead::Failed,
        }
    }
}

/// Whether every site's registry answered that it holds no row for the chain: positive proof that the chain has no
/// voter anywhere, whatever record a lookup used. Every voter writes its row on its own site before it stores
/// anything, and only a purge deletes a row (`graftyard_git::purge`), so a live group, moved or in a transfer, always
/// leaves one. A read that failed proves nothing.
pub fn no_row_anywhere(reads: &[RowRead]) -> bool {
    !reads.is_empty() && reads.iter().all(|r| *r == RowRead::NoRow)
}

/// A `PUT /row` to a site's registry: `row`, with `replacing` empty for a member's own write, or naming the placement
/// tries whose rows a restore may replace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowPut {
    pub row: RegistryRow,
    #[serde(default)]
    pub replacing: Vec<Box<str>>,
}

impl RowPut {
    /// The restore a re-placement on one site sends once its tries are done: `kept`'s row (the member the group
    /// kept on that site, the old one or the try that went in), replacing every other try's. `None` when no other try
    /// ran, so nothing on the site can name one.
    pub fn restore(chain: ChainId, sites: &[SiteId], kept: &crate::Member, tried: &[crate::Member]) -> Option<RowPut> {
        let replacing: Vec<Box<str>> =
            tried.iter().filter(|t| t.do_name != kept.do_name).map(|t| t.do_name.clone()).collect();
        (!replacing.is_empty()).then(|| RowPut {
            row: RegistryRow {
                chain,
                do_name: kept.do_name.clone(),
                generation: kept.generation,
                sites: sites.iter().copied().collect(),
            },
            replacing,
        })
    }

    /// Whether the put replaces `current`, the site's stored row for the chain. A member's own write never moves the
    /// row to a lower generation, so a removed member's late write changes nothing. A restore (`replacing` named)
    /// replaces only a row naming one of those tries, at any generation: every try's `/join` wrote its row at a
    /// generation above the member the group kept, and a row left naming a try that never went in sent the edge's
    /// registry lookups to a member outside the group, 63,387 `/read` 409s from `repo:131342#9#f1` in round 29
    /// (`tests/am1/results/lm1-memory.md`, round 31). A row that names anyone else, a newer placement's member say, stays.
    pub fn replaces(&self, current: Option<&RegistryRow>) -> bool {
        match current {
            _ if self.replacing.is_empty() => current.is_none_or(|c| self.row.generation >= c.generation),
            Some(c) => self.replacing.contains(&c.do_name),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{DbId, RepoId};

    #[test]
    fn shard_is_stable_and_spreads() {
        let a = MemberRegistry::for_chain(SiteId(0), ChainId::Repo(RepoId(DbId(1))));
        assert_eq!(a, MemberRegistry::for_chain(SiteId(0), ChainId::Repo(RepoId(DbId(1)))));
        // Same chain, other site: same shard number, so `ReplaceMember` reads one shard per surviving site.
        assert_eq!(a.shard, MemberRegistry::for_chain(SiteId(2), ChainId::Repo(RepoId(DbId(1)))).shard);
        let mut seen = [0u32; REGISTRY_SHARDS as usize];
        for i in 0..6_400 {
            seen[MemberRegistry::for_chain(SiteId(0), ChainId::Repo(RepoId(DbId(i)))).shard as usize] += 1;
        }
        assert!(seen.iter().all(|&n| (50..=160).contains(&n)), "{seen:?}");
    }

    /// `repo-gone`: only every site answering 404 proves the chain has no voter; one row, one failed read (a 5xx, a
    /// timeout), or no site at all proves nothing.
    #[test]
    fn only_every_site_answering_no_row_proves_no_voter() {
        use RowRead::*;
        assert_eq!(
            [Some(200), Some(404), Some(500), Some(409), None].map(RowRead::of),
            [Row, NoRow, Failed, Failed, Failed]
        );
        assert!(no_row_anywhere(&[NoRow, NoRow, NoRow]));
        for reads in [&[Row, NoRow, NoRow][..], &[NoRow, Failed, NoRow], &[NoRow, NoRow, Row], &[]] {
            assert!(!no_row_anywhere(reads), "{reads:?}");
        }
    }

    /// The site's registry as `PUT /row` keeps it: one row per chain, each put applied when `replaces` allows.
    fn apply(stored: &mut Option<RegistryRow>, put: &RowPut) -> bool {
        let ok = put.replaces(stored.as_ref());
        if ok {
            *stored = Some(put.row.clone());
        }
        ok
    }

    fn own(chain: ChainId, m: &crate::Member) -> RowPut {
        RowPut {
            row: RegistryRow { chain, do_name: m.do_name.clone(), generation: m.generation, sites: SmallVec::new() },
            replacing: Vec::new(),
        }
    }

    /// Round 29's create placement on site b of `repo:131342`: the group's voter is `#3#f1`, and tries `#4` to `#9`
    /// each join, writing their rows, and none goes in. On main the row stayed `#9#f1`, a member outside the group,
    /// and the edge's registry stage read it about 61,000 times per site and got 63,387 `/read` 409s
    /// (`tests/am1/results/lm1-memory.md`, round 31). The restore puts `#3#f1` back below the tries' generations, a
    /// member's own write still never moves the row back, and a row naming a member the restore does not know, a newer
    /// placement's, stays.
    #[test]
    fn a_placement_that_kept_its_member_puts_its_row_back_over_every_try() {
        use crate::{Member, MemberKind};
        let chain = ChainId::Repo(RepoId(DbId(131_342)));
        let b = SiteId(1);
        let m = |g| Member::new(chain, b, MemberKind::Full, 1, g);
        let kept = m(3);
        let mut stored = None;
        assert!(apply(&mut stored, &own(chain, &kept)));
        let tries: Vec<Member> = (4..=9).map(m).collect();
        for t in &tries {
            assert!(apply(&mut stored, &own(chain, t)), "each try's /join writes its row");
        }
        assert_eq!(stored.as_ref().unwrap().do_name, m(9).do_name);
        // A member's own write still never moves the row back: the old member re-writing its row at 3 is refused.
        assert!(!apply(&mut stored, &own(chain, &kept)));
        let restore = RowPut::restore(chain, &[SiteId(0), b, SiteId(2)], &kept, &tries).expect("tries ran");
        assert!(apply(&mut stored, &restore), "the restore replaces a try's row at a higher generation");
        let row = stored.clone().unwrap();
        assert_eq!((&*row.do_name, row.generation), (&*kept.do_name, 3));
        assert_eq!(&row.sites[..], &[SiteId(0), b, SiteId(2)]);
        // A try that went in is the kept member, and the rows of the others are replaced by it.
        let mut stored2 = None;
        for t in &tries[..3] {
            apply(&mut stored2, &own(chain, t));
        }
        let won = &tries[1];
        let r2 = RowPut::restore(chain, &[b], won, &tries[..3]).unwrap();
        assert!(!r2.replacing.contains(&won.do_name));
        assert!(apply(&mut stored2, &r2));
        assert_eq!(stored2.unwrap().do_name, won.do_name);
        // A later placement's member (generation 12, not one of these tries) is never overwritten by this restore.
        let mut newer = Some(own(chain, &m(12)).row);
        assert!(!apply(&mut newer, &restore));
        assert_eq!(newer.unwrap().generation, 12);
        // No row at all (the registry lost it): a restore writes nothing, so it never creates a row by itself.
        assert!(!apply(&mut None, &restore));
        // A single try that was itself kept leaves nothing to restore.
        assert_eq!(RowPut::restore(chain, &[b], &tries[0], &tries[..1]), None);
    }
}
