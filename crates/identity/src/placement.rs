//! The check `graftyard placement set` runs before it submits `NamesOp::MovePlacement` or `InitPlacement`
//! (`design/DESIGN.md`, "Usage (caller's view)" and Shape, "Sites and placement").
//!
//! `Placement::from_record`, which `decide` runs, checks shape only; the record's writer runs `Placement::new`'s
//! checks against the site table and the joining members' `Rtt` rows, and `Placement::solo`'s evaluation flag.

use arrayvec::ArrayVec;
use graftyard_types::{
    AccountId, ChainId, MemberKind, Placement, PlacementChecks, PlacementError, PlacementName, PlacementRecord,
};

/// Validates a record for `owner` as `Placement::new` (or `Placement::solo` for `Solo`) would build its bootstrap
/// voters: site count, distinct sites, distinct regions, accounts when the install separates them, voter pairs at or
/// over `MIN_PAIR_RTT`, two Full voters, a Full home, and for `Solo` the install's signed `evaluation` flag.
pub fn validate_record(
    owner: AccountId,
    record: &PlacementRecord,
    evaluation: bool,
    checks: &PlacementChecks,
) -> Result<Placement, PlacementError> {
    let chain = ChainId::Authz(owner);
    // `from_record` checks the site count and duplicates, so the voters below are well formed.
    Placement::from_record(chain, record)?;
    let mut voters: Vec<_> = record.bootstrap_members(chain);
    if record.name == PlacementName::Solo {
        let p = Placement::solo(voters.remove(0), evaluation, checks)?;
        return if p.home() == record.home { Ok(p) } else { Err(PlacementError::BadHome(record.home)) };
    }
    debug_assert!(voters.iter().all(|m| m.kind != MemberKind::Learner));
    Placement::new(voters.into_iter().collect::<ArrayVec<_, 3>>(), ArrayVec::new(), record.home, checks)
}

#[cfg(test)]
mod tests {
    use graftyard_types::{DbId, Region, Rtt, Site, SiteId, SiteLink, SiteTable, Timestamp};

    use super::*;

    fn site(id: u8, region: Region) -> Site {
        Site {
            id: SiteId(id),
            script: format!("graft-site-{id}"),
            account: "acct".into(),
            region,
            jurisdiction: None,
            bucket: format!("b{id}"),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare: false,
        }
    }

    fn rec(name: PlacementName, sites: &[u8], home: u8) -> PlacementRecord {
        PlacementRecord {
            name,
            sites: sites.iter().map(|s| SiteId(*s)).collect(),
            home: SiteId(home),
            generation: 1,
            moving_from: None,
            at: Timestamp(1),
        }
    }

    #[test]
    fn placement_set_reuses_placement_new_and_solo_needs_the_flag() {
        let t =
            SiteTable(vec![site(0, Region::Enam), site(1, Region::Weur), site(2, Region::Apac), site(3, Region::Enam)]);
        let rtt: Vec<(SiteId, Rtt)> = [(0, 1, 80), (0, 2, 180), (1, 2, 200), (0, 3, 40), (1, 3, 85)]
            .iter()
            .map(|&(a, b, ms)| (SiteId(a), Rtt { peer: SiteId(b), median_ms: ms, at: Timestamp(0) }))
            .collect();
        let checks = PlacementChecks { sites: &t, separate_accounts: false, rtt: &rtt };
        let owner = AccountId(DbId(1 << 16));
        assert!(validate_record(owner, &rec(PlacementName::Tri, &[0, 1, 2], 0), false, &checks).is_ok());
        assert_eq!(
            validate_record(owner, &rec(PlacementName::Tri, &[0, 1, 3], 0), false, &checks),
            Err(PlacementError::SameRegion(SiteId(0), SiteId(3)))
        );
        assert_eq!(
            validate_record(owner, &rec(PlacementName::Tri, &[0, 1], 0), false, &checks),
            Err(PlacementError::SiteCount { name: PlacementName::Tri, want: 3, got: 2 })
        );
        assert_eq!(
            validate_record(owner, &rec(PlacementName::Us2w, &[0, 1, 2], 2), false, &checks),
            Err(PlacementError::BadHome(SiteId(2)))
        );
        // `graftyard placement set eval-org solo --site A`.
        assert_eq!(
            validate_record(owner, &rec(PlacementName::Solo, &[0], 0), false, &checks),
            Err(PlacementError::SoloNeedsEvaluation)
        );
        assert!(validate_record(owner, &rec(PlacementName::Solo, &[0], 0), true, &checks).is_ok());
        assert_eq!(
            validate_record(owner, &rec(PlacementName::Solo, &[9], 9), true, &checks),
            Err(PlacementError::UnknownSite(SiteId(9)))
        );
    }
}
