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

use std::time::Duration;

use graftyard_raft::GroupConfig;
use graftyard_types::{PlacementRecord, SiteId, Timestamp};

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

/// The `Joint`'s home after `Swap { add, drop }` on `p`: the home stays unless its site leaves; then the target's home
/// if the new half holds it as a Full voter, else the new member's site, until `Home` moves it.
pub fn swap_home(p: &graftyard_types::Placement, add: SiteId, drop: SiteId, to_home: SiteId) -> SiteId {
    if p.home() != drop {
        return p.home();
    }
    // The new member on `add` takes the kind of the one on `drop`.
    let at = if to_home == add { drop } else { to_home };
    let full =
        to_home != drop && p.voters().iter().any(|m| m.site == at && m.kind == graftyard_types::MemberKind::Full);
    if full { to_home } else { add }
}

/// The site a planned step makes the group's next leader, whose git cache shard the step waits for first (G11):
/// a `Transfer`'s target, or the new home a `Home` or a home-moving `Swap` commits. Once a config naming another home
/// is in the log, the raft core hands leadership to it by itself (`maybe_hand_off`), so the wait comes before that
/// config, not before a `Transfer` the core makes moot (m3fg11: `Home` committed and the leader moved 1.5 s later,
/// with no `Transfer` asked). `None` when the step moves no leader.
pub fn warm_site(cfg: &GroupConfig, leader: SiteId, to: &PlacementRecord, step: MoveStep) -> Option<SiteId> {
    let next = match (step, cfg) {
        (MoveStep::Transfer { to }, _) => to,
        (MoveStep::Home { home }, _) => home,
        (MoveStep::Swap { add, drop }, GroupConfig::Normal(p)) => {
            let home = swap_home(p, add, drop, to.home);
            if home == p.home() {
                return None;
            }
            home
        }
        _ => return None,
    };
    (next != leader).then_some(next)
}

/// How long a repository group's planned move waits for the next leader site's git cache shard to answer its
/// catch-up before it moves leadership anyway [guess]. After B0-31 m3fr7's rehome to apac, that site's shard took
/// 69.0 s to catch up (268 MB in 257 packs), and every MergeQueue merge in that window answered `warming`
/// (`decisions/OPEN-QUESTIONS.md`, G11). Waiting first moves that window ahead of the hand-off, while the old leader
/// still serves. The bound keeps a shard that never warms from stopping the move.
pub const WARM_BEFORE_TRANSFER: Duration = Duration::from_secs(60);

/// One planned transfer's wait for the target shard, per leader isolate: when the first `warming` answer came, and
/// the latest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WarmWait {
    pub since: Timestamp,
    pub last: Timestamp,
}

/// What a planned `Transfer` does after asking the target site's shard to catch up (`transfer_gate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferGate {
    /// The shard is still warming and the wait is under WARM_BEFORE_TRANSFER: answer the step without transferring.
    /// The caller asks again.
    Hold,
    /// The shard answered warm: transfer now.
    Go,
    /// WARM_BEFORE_TRANSFER passed with the shard still warming: transfer cold, and say so.
    GoCold,
}

/// The gate for a planned `Transfer` whose target shard answered `warm`, given this isolate's wait so far. Returns
/// the gate, the wait to keep (`None` once the transfer goes), and the ms waited. A wait whose last ask is older than
/// WARM_BEFORE_TRANSFER belongs to an earlier move that stopped asking, so it starts again: a move resumed hours later
/// does not go cold on its first ask.
pub fn transfer_gate(warm: bool, wait: Option<WarmWait>, now: Timestamp) -> (TransferGate, Option<WarmWait>, u64) {
    let bound = WARM_BEFORE_TRANSFER.as_millis() as u64;
    let wait = wait.filter(|w| now.0.saturating_sub(w.last.0) <= bound);
    let waited = wait.map_or(0, |w| now.0.saturating_sub(w.since.0));
    if warm {
        return (TransferGate::Go, None, waited);
    }
    if waited >= bound {
        // The expired wait is kept, so the next step of the same move (a `Transfer` after a cold `Home` the core did
        // not hand off) goes cold at once instead of waiting a second bound.
        return (TransferGate::GoCold, wait.map(|w| WarmWait { last: now, ..w }), waited);
    }
    let since = wait.map_or(now, |w| w.since);
    (TransferGate::Hold, Some(WarmWait { since, last: now }), waited)
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000;

    /// Asks the gate every `every` ms from `t0` with the shard warm from `warm_at` on, until it transfers. Returns
    /// the gate it ended on and when.
    fn run(t0: u64, every: u64, warm_at: Option<u64>) -> (TransferGate, u64) {
        let mut wait = None;
        let mut now = t0;
        // A gate that never lets the transfer go fails here rather than hanging the test.
        for _ in 0..1_000 {
            let warm = warm_at.is_some_and(|w| now >= w);
            let (gate, next, _) = transfer_gate(warm, wait, Timestamp(now));
            if gate != TransferGate::Hold {
                return (gate, now);
            }
            wait = next;
            now += every;
        }
        panic!("the transfer never went: held from {t0} to {now}");
    }

    #[test]
    fn a_transfer_holds_while_the_target_shard_warms_and_goes_once_it_is_warm() {
        // Warm 25 s into the wait, asked every 10 s: holds at 0, 10 and 20 s, goes at 30 s, warm.
        assert_eq!(run(100 * S, 10 * S, Some(125 * S)), (TransferGate::Go, 130 * S));
        // Warm at the first ask: no hold at all.
        assert_eq!(transfer_gate(true, None, Timestamp(5 * S)), (TransferGate::Go, None, 0));
    }

    #[test]
    fn a_shard_that_never_warms_lets_the_transfer_go_cold_at_the_deadline() {
        // Asked every 10 s from 100 s: holds through 150 s (50 s waited), goes cold at 160 s, 60 s waited.
        assert_eq!(run(100 * S, 10 * S, None), (TransferGate::GoCold, 160 * S));
        // One ms short of the bound still holds.
        let w = Some(WarmWait { since: Timestamp(0), last: Timestamp(59 * S) });
        assert_eq!(transfer_gate(false, w, Timestamp(60 * S - 1)).0, TransferGate::Hold);
        let (gate, kept, waited) = transfer_gate(false, w, Timestamp(60 * S));
        assert_eq!((gate, waited), (TransferGate::GoCold, 60 * S));
        // The expired wait is kept: the move's next step to the same site goes cold at once, not after a second bound.
        assert_eq!(transfer_gate(false, kept, Timestamp(63 * S)).0, TransferGate::GoCold);
        // Warm by the deadline: it goes warm, never held.
        assert_eq!(transfer_gate(true, w, Timestamp(60 * S)), (TransferGate::Go, None, 60 * S));
    }

    #[test]
    fn a_wait_left_by_a_move_that_stopped_asking_starts_again() {
        // The last ask was 61 s ago: that move stopped. This ask holds, with a new wait from now.
        let old = Some(WarmWait { since: Timestamp(0), last: Timestamp(10 * S) });
        let (gate, next, waited) = transfer_gate(false, old, Timestamp(71 * S));
        assert_eq!((gate, waited), (TransferGate::Hold, 0));
        assert_eq!(next, Some(WarmWait { since: Timestamp(71 * S), last: Timestamp(71 * S) }));
    }
}
