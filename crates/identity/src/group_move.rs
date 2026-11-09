//! A group's move to another placement record, one voter site per config change (`design/SHARED.md`, "Multi-chain
//! operations", Transfer; `decisions/DECISIONS.md`, "A transfer's own `Settings` command, and the move between
//! placements pending").
//!
//! `Member::reconfigure` refuses a config that adds or drops more than one voter site, counting the voters of both
//! halves of a `Joint` (`NotOneChange`), so a `Joint` of two placements that differ by two or three sites cannot be
//! expressed [measured: `Joint { old: [0, 1, 2], new: [0, 3, 4] }` is refused, `Joint { old: [0, 1, 2], new:
//! [0, 1, 3] }` commits]. A move therefore swaps one site at a time: `Joint { old, new }` where `new` is `old` with one
//! member replaced by a new member on the target site (adding one voter site), then `Normal(new)` (dropping one).
//! Under the `Joint`, commits and leases need a majority of both halves, which share every site but two.
//!
//! The leader's site never leaves while it leads: leadership first goes to a voter the target keeps, and when the
//! target keeps none of the current sites, to the first new site while the `Joint` holds it. The last steps make
//! the target's home the config's home and move leadership there.
//!
//! `next_step` is pure: the committed config, the leader's site, and the target record. The caller performs the step,
//! waits for its config to commit, and asks again; a step lost with its leader is planned again from the config the
//! next leader holds.

use graftyard_raft::GroupConfig;
use graftyard_types::{PlacementRecord, SiteId};

/// What the group's leader does next to reach `to`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveStep {
    /// Move leadership to `to`, a voter that stays, before the leader's site leaves.
    Transfer { to: SiteId },
    /// Join a new member on `add`, in the slot and kind of the member on `drop` at the next generation, then commit
    /// `Joint { old, new }` with `new` naming it in place of `drop`.
    Swap { add: SiteId, drop: SiteId },
    /// Commit `Normal(new)`: the `Joint`'s old half's extra site leaves.
    Leave,
    /// The voters are the target's: commit `Normal` with the target's home.
    Home { home: SiteId },
    /// The config is a `Degraded` one: a lost member is replaced before the group moves.
    Degraded,
    /// The config's voters and home are the target's, and the leader is on the home.
    Done,
    /// The target's voter count differs from the config's (`Solo` to or from a three-site record), or it names a site
    /// twice: no sequence of one-site swaps reaches it.
    Unsupported,
}

fn sites(p: &graftyard_types::Placement) -> Vec<SiteId> {
    p.voters().iter().map(|m| m.site).collect()
}

/// The next step from the committed config `cfg`, led from `leader`, towards `to`'s sites and home.
pub fn next_step(cfg: &GroupConfig, leader: SiteId, to: &PlacementRecord) -> MoveStep {
    let target = &to.sites;
    let staying = |vs: &[SiteId]| -> Option<SiteId> {
        let keep: Vec<SiteId> = vs.iter().copied().filter(|s| target.contains(s) && *s != leader).collect();
        keep.iter().copied().find(|s| *s == to.home).or(keep.first().copied())
    };
    let p = match cfg {
        GroupConfig::Degraded { .. } => return MoveStep::Degraded,
        GroupConfig::Joint { new, .. } => {
            let vs = sites(new);
            if !vs.contains(&leader) {
                // The leader's site is the old half's extra one: leadership goes to a voter of the new half first.
                let Some(to) = vs.iter().copied().find(|s| *s == to.home).or(vs.first().copied()) else {
                    return MoveStep::Unsupported;
                };
                return MoveStep::Transfer { to };
            }
            return MoveStep::Leave;
        }
        GroupConfig::Normal(p) => p,
    };
    let vs = sites(p);
    let leaving = |s: SiteId| !target.contains(&s);
    if vs.iter().all(|s| target.contains(s)) && vs.len() == target.len() {
        if p.home() != to.home {
            return MoveStep::Home { home: to.home };
        }
        if leader != to.home {
            return MoveStep::Transfer { to: to.home };
        }
        return MoveStep::Done;
    }
    // Leadership leaves a site the target drops before any swap, when a voter that stays can take it.
    if leaving(leader)
        && let Some(stay) = staying(&vs)
    {
        return MoveStep::Transfer { to: stay };
    }
    let Some(&add) = target.iter().find(|s| !vs.contains(s)) else { return MoveStep::Unsupported };
    // A site that leaves, not the leader's while another can go first.
    let Some(&drop) = vs.iter().filter(|s| leaving(**s)).min_by_key(|s| **s == leader) else {
        return MoveStep::Unsupported;
    };
    MoveStep::Swap { add, drop }
}
