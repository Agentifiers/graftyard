//! M5-WP4's gate inputs (`design/DESIGN.md`, \"Install operations\", Site day; \"Commit and read paths\", journey 5,
//! Automatic drain), pure: what `graft-ops` judges a step on, and the `SiteHealth` burn it drains and undrains a site
//! on. The host reads the numbers (the N and N+1 members' counters, each observer site's `SiteHealth` object) and asks
//! these functions; `ops::next` acts on the answer.
//!
//! - **A step's gate.** `judge_site` compares the N+1 members of the rolling site with its N members:
//!   `GATE_EXCEPTIONS`, `GATE_STEP_DOWN`, `GATE_APPLY_LAG`, and `GATE_ACK_RATE` breach; fewer than `STEP_MIN_ENTRIES`
//!   applied by N+1 is `Starved`, which `next` dwells on up to twice `STEP_DWELL` and then pages. `judge_errors` is
//!   `GATE_ERRORS` for the edge, front, ops, MCP, and controller rolls.
//! - **Burn.** A target site's burn, from one observer, is its missed share of calls over the allowed share
//!   (`ALLOWED_MISS`). `verdict` calls a site `burning` when both other voter sites' `DRAIN_WINDOW` reports are at
//!   `DRAIN_BURN` or more, and `calm` when both `UNDRAIN_AFTER` reports are under `UNDRAIN_BURN`. A report with fewer
//!   than `MIN_CALLS` calls is no evidence either way.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_types::SiteId;

use crate::ReleaseFold;
use crate::ops::Gate;
use crate::types::{DayTarget, Intent, IntentState};

/// Entries the N+1 members must apply in a step before its gate is judged (`STEP_MIN_ENTRIES`) [guess].
pub const STEP_MIN_ENTRIES: u64 = 1_000;
/// N+1's p99 apply lag may be at most twice N's and at most this (`GATE_APPLY_LAG`) [guess].
pub const GATE_APPLY_LAG_MS: u64 = 500;
/// N+1's ack rate may be at most this many basis points below N's (`GATE_ACK_RATE`, 0.1 points) [guess].
pub const GATE_ACK_RATE_BP: u32 = 10;
/// N+1's 5xx rate may be at most this many basis points above N's (`GATE_ERRORS`, 0.1 points) [guess].
pub const GATE_ERRORS_BP: u32 = 10;
/// A site day that cannot advance for this long undrains its site under the 1% lead limit (`STUCK_DAY`) [guess].
pub const STUCK_DAY: u64 = 3_600_000;
/// The share of calls the SLO allows to miss: the write journey's 99.95% (\"Downtime\", SLOs), because member calls are
/// the commit path [inference]. As a ratio, 1 in 2,000.
pub const ALLOWED_MISS_DENOM: u64 = 2_000;
/// Burn that drains a site, held over `DRAIN_WINDOW` from both other sites (`DRAIN_BURN`) [guess].
pub const DRAIN_BURN: u64 = 10;
pub const DRAIN_WINDOW: u64 = 5 * 60_000;
/// Burn under which a drained site undrains, held over `UNDRAIN_AFTER` from both other sites [guess].
pub const UNDRAIN_BURN: u64 = 1;
pub const UNDRAIN_AFTER: u64 = 30 * 60_000;
/// Fewest calls in a report that counts as evidence [guess]: below it one missed call is a 10x burn.
pub const MIN_CALLS: u64 = 100;

/// One side of a step's comparison: the N+1 members of the rolling site, or its N members, over the step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Side {
    pub apply_lag_p99_ms: u64,
    /// Apply and verify exceptions.
    pub exceptions: u64,
    /// Acks over calls, in basis points.
    pub ack_bp: u32,
    /// `STEP_DOWN` trips on this side's leaders.
    pub step_downs: u64,
    /// Entries applied in the step.
    pub entries: u64,
}

/// A site day's step gate: N+1 against the same site's N members.
pub fn judge_site(n1: &Side, n: &Side) -> Gate {
    if n1.exceptions > 0 {
        return Gate::Breach(format!("GATE_EXCEPTIONS: {} on N+1", n1.exceptions).into());
    }
    if n1.step_downs > 0 {
        return Gate::Breach(format!("GATE_STEP_DOWN: {} on N+1 leaders", n1.step_downs).into());
    }
    if n1.apply_lag_p99_ms > GATE_APPLY_LAG_MS || n1.apply_lag_p99_ms > 2 * n.apply_lag_p99_ms {
        return Gate::Breach(
            format!("GATE_APPLY_LAG: N+1 p99 {} ms, N {} ms", n1.apply_lag_p99_ms, n.apply_lag_p99_ms).into(),
        );
    }
    if n1.ack_bp + GATE_ACK_RATE_BP < n.ack_bp {
        return Gate::Breach(format!("GATE_ACK_RATE: N+1 {} bp, N {} bp", n1.ack_bp, n.ack_bp).into());
    }
    if n1.entries < STEP_MIN_ENTRIES {
        return Gate::Starved(n1.entries);
    }
    Gate::Pass
}

/// A pinned or front script's step gate: N+1's 5xx rate against N's, in basis points.
pub fn judge_errors(n1_bp: u32, n_bp: u32) -> Gate {
    if n1_bp > n_bp + GATE_ERRORS_BP {
        return Gate::Breach(format!("GATE_ERRORS: N+1 {n1_bp} bp of 5xx, N {n_bp} bp").into());
    }
    Gate::Pass
}

/// One observer site's counts of calls to one target site that missed `CALL_DEADLINE`, over a window
/// (`design/DESIGN.md`, Shape, `SiteHealth`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SiteHealth {
    pub observer: SiteId,
    pub target: SiteId,
    pub calls: u64,
    pub missed: u64,
}

impl SiteHealth {
    fn evidence(&self) -> bool {
        self.calls >= MIN_CALLS
    }

    /// Burn at `x` times the allowed miss rate or more.
    fn at_least(&self, x: u64) -> bool {
        self.evidence() && self.missed * ALLOWED_MISS_DENOM >= x * self.calls
    }

    /// Burn under `x` times the allowed miss rate.
    fn under(&self, x: u64) -> bool {
        self.evidence() && self.missed * ALLOWED_MISS_DENOM < x * self.calls
    }
}

/// Both other sites' verdict on one target site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// Both other voter sites report `DRAIN_BURN` or more over `DRAIN_WINDOW`.
    pub burning: bool,
    /// Both other voter sites report under `UNDRAIN_BURN` over `UNDRAIN_AFTER`.
    pub calm: bool,
}

/// Each voter site's verdict from the reports of the others. `fast` holds the `DRAIN_WINDOW` reports and `slow` the
/// `UNDRAIN_AFTER` ones; a site judges no site, itself included, alone, so the sick site never decides for itself.
pub fn verdict(voters: &[SiteId], fast: &[SiteHealth], slow: &[SiteHealth]) -> BTreeMap<u8, Verdict> {
    let all = |t: SiteId, reports: &[SiteHealth], ok: &dyn Fn(&SiteHealth) -> bool| {
        voters.iter().filter(|o| **o != t).all(|o| reports.iter().any(|h| h.observer == *o && h.target == t && ok(h)))
    };
    voters
        .iter()
        .map(|t| {
            let burning = all(*t, fast, &|h| h.at_least(DRAIN_BURN));
            let calm = all(*t, slow, &|h| h.under(UNDRAIN_BURN));
            (t.0, Verdict { burning, calm })
        })
        .collect()
}

/// An observer's `SiteHealth` counts per second and target, as its object keeps them: added to by every call, summed
/// over a window, and pruned past the longest window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HealthLog {
    /// (second, target) to (calls, missed).
    pub secs: BTreeMap<(u64, u8), (u64, u64)>,
}

impl HealthLog {
    pub fn add(&mut self, now_ms: u64, target: SiteId, calls: u64, missed: u64) {
        let c = self.secs.entry((now_ms / 1000, target.0)).or_default();
        c.0 += calls;
        c.1 += missed;
        let keep = (now_ms.saturating_sub(UNDRAIN_AFTER)) / 1000;
        self.secs.retain(|(s, _), _| *s >= keep);
    }

    /// The counts to `target` over the `window` ms before `now_ms`, as `observer` reports them.
    pub fn report(&self, observer: SiteId, target: SiteId, now_ms: u64, window: u64) -> SiteHealth {
        let from = now_ms.saturating_sub(window) / 1000;
        let (calls, missed) = self
            .secs
            .iter()
            .filter(|((s, t), _)| *s >= from && *t == target.0)
            .fold((0, 0), |(c, m), (_, (dc, dm))| (c + dc, m + dm));
        SiteHealth { observer, target, calls, missed }
    }
}

/// The site under the 1% lead limit: the current site day's site once its drain step is done and while it is not
/// drained, which is its first day after undrain, or the rest of a day an automatic drain preempted or `STUCK_DAY`
/// undrained (\"Install operations\", Site day, `FIRST_DAY`; journey 5, \"Automatic drain\").
pub fn lead_limited(fold: &ReleaseFold) -> Option<SiteId> {
    let c = fold.current.as_deref()?;
    match (c.state, &c.intent.intent) {
        (
            IntentState::Running { .. } | IntentState::Halted { .. },
            Intent::SiteDay { target: DayTarget::Site(s), .. },
        ) if c.next >= 1 && !fold.drained.contains_key(&s.0) => Some(*s),
        _ => None,
    }
}

/// The sites whose members may not lead `chain`: every drained site, and the lead-limited site unless `chain` is in
/// its 1% (an FNV-1a hash of the chain's text form, mod 100, is 0, the rule B0-07's stand-in measured) [inference: the
/// design names no hash]. What a member host steps into `set_drained`.
pub fn may_not_lead(fold: &ReleaseFold, chain: &str) -> BTreeSet<SiteId> {
    let h = chain.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
    let mut out: BTreeSet<SiteId> = fold.drained.keys().map(|s| SiteId(*s)).collect();
    if let Some(s) = lead_limited(fold)
        && h % 100 != 0
    {
        out.insert(s);
    }
    out
}

/// How far `site`'s shards have rolled onto the git image of the site's version (`graftyard_git::image_for`'s
/// `day_step`): `Some(k)` covers `ROLL_STEPS` step k's share of shards, and `None` keeps every shard on `git_prev`.
/// - During that site's `ContainerDay`, the step it runs: a step's shards restart as the step starts.
/// - During its undo or `Abort`, `None`: shards return to `git_prev` with no API call.
/// - Otherwise all shards when the site's containers last rolled to the release its script runs, and none when its
///   script has moved to a newer release since, whose version names the older image as `git_prev`.
pub fn shard_day(fold: &ReleaseFold, site: SiteId) -> Option<u8> {
    if let Some(c) = fold.current.as_deref()
        && let Intent::ContainerDay { site: s, .. } = c.intent.intent
        && s == site
    {
        return match c.state {
            IntentState::Running { .. } | IntentState::Halted { .. } => Some(c.next.min(2)),
            IntentState::Undoing | IntentState::Aborting { .. } => None,
        };
    }
    let rolled = fold.containers.get(&site.0)?;
    (fold.site_release.get(&site.0) == Some(rolled)).then_some(2)
}
