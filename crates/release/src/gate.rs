//! M5-WP4's gate inputs (`design/DESIGN.md`, \"Install operations\", Site day; \"Commit and read paths\", journey 5,
//! Automatic drain), pure: what `graft-ops` judges a step on, and the `SiteHealth` burn it drains and undrains a site
//! on. The host reads the numbers (the N and N+1 members' counters, each observer site's `SiteHealth` object) and asks
//! these functions; `ops::next` acts on the answer.
//!
//! - **A step's gate.** `judge_site` compares the N+1 members of the rolling site with its N members:
//!   `GATE_EXCEPTIONS`, `GATE_STEP_DOWN`, `GATE_APPLY_LAG`, and `GATE_ACK_RATE` breach. `GATE_APPLY_LAG` judges
//!   N+1's tail against N's own p99 on the same site, so a drained site's followers, which apply about
//!   `CALL_DEADLINE` after their leader on a quiet chain (R29's commit round), are judged against followers
//!   (`decisions/DECISIONS.md`, "`gate-relative`"); fewer than `STEP_MIN_ENTRIES`
//!   applied by N+1 is `Starved`, which `next` dwells on up to twice `STEP_DWELL` and then pages. `judge_errors` is
//!   `GATE_ERRORS` for the edge, front, ops, MCP, and controller rolls.
//! - **Burn.** A target site's burn, from one observer, is its missed share of calls over the allowed share
//!   (`ALLOWED_MISS`). `verdict` calls a site `burning` when both other voter sites' `DRAIN_WINDOW` reports are at
//!   `DRAIN_BURN` or more, and `calm` when both `UNDRAIN_AFTER` reports are under `UNDRAIN_BURN`. A report with fewer
//!   than `MIN_CALLS` calls is no evidence either way.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_types::{SiteId, Timestamp};

use crate::ReleaseFold;
use crate::group::Current;
use crate::ops::{Gate, Step};
use crate::types::{DayTarget, Intent, IntentState, ReleaseEntry, Script, VersionId};

/// Entries the N+1 members must apply in a step before its gate is judged (`STEP_MIN_ENTRIES`) [guess].
pub const STEP_MIN_ENTRIES: u64 = 1_000;
/// `GATE_APPLY_LAG`: at most this share of N+1's applied entries, in basis points, may be slower than N's p99 on
/// the same site [measured, BF-39 hosted: a healthy N+1 put 0.07 to 1.01% there, a +300 ms regression 47 to 50%].
pub const GATE_APPLY_LAG_SHARE_BP: u64 = 200;
/// `GATE_APPLY_LAG`'s backstop, with or without N: at most `GATE_APPLY_LAG_CAP_BP` of N+1's entries slower than
/// this [measured, BF-39 hosted: a healthy N+1 put at most 0.27% over 5 s, a 10 s stall on 1.5% of entries 1.5%].
pub const GATE_APPLY_LAG_CAP_MS: u64 = 5_000;
pub const GATE_APPLY_LAG_CAP_BP: u64 = 100;
/// `GATE_ACK_RATE`: N+1's refused share of AppendEntries answers may exceed N's by this much, 0.1 points [guess],
/// and breaches only when the excess is past it by `GATE_ACK_RATE_Z` standard errors of the two shares, a one-sided
/// test at about 1 in 1,000 per read. The gate check row read a healthy N+1 of 100 entries 0.56 points under N
/// (OPEN-QUESTIONS R43).
pub const GATE_ACK_RATE_MARGIN: f64 = 0.001;
pub const GATE_ACK_RATE_Z: f64 = 3.09;
/// N+1's 5xx rate may be at most this many basis points above N's (`GATE_ERRORS`, 0.1 points) [guess].
pub const GATE_ERRORS_BP: u32 = 10;
/// A site day that cannot advance for this long undrains its site under the 1% lead limit (`STUCK_DAY`) [guess].
pub const STUCK_DAY: u64 = 3_600_000;
/// The share of member calls allowed to miss `CALL_DEADLINE`, as 1 in this: 1% [measured, one install: graft-m4a's
/// steady 5-minute miss rate was 0.2 to 0.3%, at most 0.99%, in BF-39 hosted]. Not the write SLO's 1 in 2,000: a
/// missed deadline is a slow call the next round retries, not a failed write (OPEN-QUESTIONS R41).
pub const ALLOWED_MISS_DENOM: u64 = 100;
/// Burn that drains a site, held over `DRAIN_WINDOW` from both other sites (`DRAIN_BURN`): 5% of calls missed
/// [measured, one install: B0-07's gray fault read 9.6 to 27.5 x from 2 minutes in, steady load at most 1.0 x].
pub const DRAIN_BURN: u64 = 5;
pub const DRAIN_WINDOW: u64 = 5 * 60_000;
/// Burn under which a drained site undrains, held over `UNDRAIN_AFTER` from both other sites [measured, one
/// install: calm 15 min 40 s after BF-39's gray fault lifted].
pub const UNDRAIN_BURN: u64 = 2;
pub const UNDRAIN_AFTER: u64 = 15 * 60_000;
/// Fewest calls in a report that counts as evidence: 300, so 15 missed is the drain burn and 3 missed are not
/// [inference from BF-39 hosted, where 3 misses in 582 calls counted at 1 in 2,000].
pub const MIN_CALLS: u64 = 300;

/// One side of a step's comparison: the N+1 members of the rolling site, or its N members, over the step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Side {
    pub apply_lag_p99_ms: u64,
    /// Apply and verify exceptions.
    pub exceptions: u64,
    /// AppendEntries answered, and how many of those acked.
    pub appends: u64,
    pub acks: u64,
    /// `STEP_DOWN` trips on this side's leaders.
    pub step_downs: u64,
    /// Entries applied in the step.
    pub entries: u64,
    /// Entries applied per `LAG_EDGES` bucket.
    pub lag: Vec<u64>,
}

impl Side {
    /// The share of this side's entries, in basis points, in buckets above the one whose upper bound is `ms`.
    pub fn share_over_bp(&self, ms: u64) -> u64 {
        let total: u64 = self.lag.iter().sum();
        let over: u64 = self.lag.iter().zip(LAG_EDGES).filter(|(_, e)| *e > ms).map(|(n, _)| n).sum();
        (over * 10_000).checked_div(total).unwrap_or(0)
    }
}

/// A site day's step gate: N+1 against the same site's N members.
pub fn judge_site(n1: &Side, n: &Side) -> Gate {
    if n1.exceptions > 0 {
        return Gate::Breach(format!("GATE_EXCEPTIONS: {} on N+1", n1.exceptions).into());
    }
    if n1.step_downs > 0 {
        return Gate::Breach(format!("GATE_STEP_DOWN: {} on N+1 leaders", n1.step_downs).into());
    }
    // The shares (`GATE_APPLY_LAG`, `GATE_ACK_RATE`) are judged only once N+1 has `STEP_MIN_ENTRIES` entries and as
    // many answers: under it one slow entry is over 2% (1 of 20 is 5%), and one refused answer of 180 is 0.56 points,
    // so a healthy step would breach on chance (R43). An N under it is no lag baseline either, since its p99 bucket is
    // not yet its own (a site's first release: only the backstop). At 100% and on the first day no member runs N, so
    // the host passes N's counters from the whole intent, when N last ran beside N+1 (`ops::Exec::gate`).
    if n1.entries < STEP_MIN_ENTRIES || n1.appends < STEP_MIN_ENTRIES {
        return Gate::Starved(n1.entries.min(n1.appends));
    }
    if let Some(z) = ack_excess_z(n1, n)
        && z >= GATE_ACK_RATE_Z
    {
        return Gate::Breach(
            format!(
                "GATE_ACK_RATE: N+1 refused {} of {} answers, N {} of {}, z {z:.2}",
                n1.appends - n1.acks,
                n1.appends,
                n.appends - n.acks,
                n.appends
            )
            .into(),
        );
    }
    let tail = n1.share_over_bp(n.apply_lag_p99_ms);
    if n.entries >= STEP_MIN_ENTRIES && tail > GATE_APPLY_LAG_SHARE_BP {
        return Gate::Breach(
            format!("GATE_APPLY_LAG: {tail} bp of N+1 entries over N's p99 {} ms", n.apply_lag_p99_ms).into(),
        );
    }
    let stalled = n1.share_over_bp(GATE_APPLY_LAG_CAP_MS);
    if stalled > GATE_APPLY_LAG_CAP_BP {
        return Gate::Breach(
            format!("GATE_APPLY_LAG: {stalled} bp of N+1 entries over {GATE_APPLY_LAG_CAP_MS} ms").into(),
        );
    }
    Gate::Pass
}

/// How many standard errors N+1's refused share exceeds N's plus `GATE_ACK_RATE_MARGIN`; `None` with no N answers.
/// The variance is floored at `ACK_VAR_FLOOR`, so two sides that refused nothing (or everything) give a finite z.
fn ack_excess_z(n1: &Side, n: &Side) -> Option<f64> {
    let refused = |s: &Side| ((s.appends - s.acks) as f64 / s.appends as f64, s.appends as f64);
    let (p1, c1) = refused(n1);
    let (p0, c0) = (n.appends > 0).then(|| refused(n))?;
    let var = (p1 * (1.0 - p1) / c1 + p0 * (1.0 - p0) / c0).max(ACK_VAR_FLOOR);
    Some((p1 - p0 - GATE_ACK_RATE_MARGIN) / var.sqrt())
}

/// `ack_excess_z`'s variance floor: one refused answer in a million each side, far under any real step's.
const ACK_VAR_FLOOR: f64 = 1e-12;

/// Upper bounds, in ms, of the apply-lag histogram's buckets; the last holds everything above.
pub const LAG_EDGES: [u64; 24] = [
    1,
    2,
    5,
    10,
    20,
    30,
    50,
    70,
    100,
    150,
    200,
    250,
    300,
    400,
    500,
    600,
    800,
    1_000,
    1_500,
    2_000,
    3_000,
    5_000,
    10_000,
    u64::MAX,
];

/// One script version's members' gate counters on one site over a window, as each isolate counts them and the site's
/// `gate` registry object sums them: entries applied with their apply lag, apply and verify exceptions, AppendEntries
/// answered and acked, and `STEP_DOWN` trips.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Counters {
    pub entries: u64,
    pub exceptions: u64,
    /// AppendEntries this version's members answered, and how many of those answers acked.
    pub appends: u64,
    pub acks: u64,
    pub step_downs: u64,
    /// Entries applied per `LAG_EDGES` bucket; shorter when the top buckets are empty.
    pub lag: Vec<u64>,
}

impl Counters {
    /// One entry applied `ms` after its leader appended it.
    pub fn applied(&mut self, ms: u64) {
        let b = LAG_EDGES.iter().position(|e| ms <= *e).unwrap_or(LAG_EDGES.len() - 1);
        if self.lag.len() <= b {
            self.lag.resize(b + 1, 0);
        }
        self.lag[b] += 1;
        self.entries += 1;
    }

    pub fn add(&mut self, o: &Counters) {
        self.entries += o.entries;
        self.exceptions += o.exceptions;
        self.appends += o.appends;
        self.acks += o.acks;
        self.step_downs += o.step_downs;
        if self.lag.len() < o.lag.len() {
            self.lag.resize(o.lag.len(), 0);
        }
        for (a, b) in self.lag.iter_mut().zip(&o.lag) {
            *a += b;
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Counters::default()
    }

    /// The window as `judge_site` reads one side: the p99 lag is its bucket's upper bound.
    pub fn side(&self) -> Side {
        let total: u64 = self.lag.iter().sum();
        let mut seen = 0;
        let mut p99 = 0;
        for (i, n) in self.lag.iter().enumerate() {
            seen += n;
            if *n > 0 && seen * 100 >= total * 99 {
                p99 = LAG_EDGES[i];
                break;
            }
        }
        Side {
            apply_lag_p99_ms: p99,
            exceptions: self.exceptions,
            appends: self.appends,
            acks: self.acks,
            step_downs: self.step_downs,
            entries: self.entries,
            lag: self.lag.clone(),
        }
    }
}

/// The current site day's site with its N+1 version and its N version: the version the intent's release uploaded for
/// the site, and the one the release the site runs uploaded, if it differs. Once `SiteRelease` names the day's own
/// release (100% and the first day), N is the release of the intent's pre-signed `Rollback` for the site, which is
/// what the site ran before (OPEN-QUESTIONS R44). `None` with no running site day.
pub fn day_versions(fold: &ReleaseFold) -> Option<(SiteId, VersionId, Option<VersionId>)> {
    let c = fold.current.as_deref()?;
    let (
        IntentState::Running { .. } | IntentState::Halted { .. },
        Intent::SiteDay { target: DayTarget::Site(s), release },
    ) = (c.state, &c.intent.intent)
    else {
        return None;
    };
    let script = Script::Site(*s);
    let n1 = crate::ops::version(fold, release, script)?;
    let undo = c.undo.iter().find_map(|(u, _)| match u {
        ReleaseEntry::Rollback { target: DayTarget::Site(t), release: r } if t == s => Some(*r),
        _ => None,
    });
    let n = crate::ops::running(fold, script)
        .filter(|n| n != release)
        .or(undo)
        .and_then(|n| crate::ops::version(fold, &n, script));
    Some((*s, n1, n))
}

/// Where the current step's gate starts counting: the step's start, or at a 10% step that follows a 1% step, the 1%
/// step's start. So the 10% gate judges both steps' entries and its `STEP_MIN_ENTRIES` floor pools them, while the 1%
/// step, whose N+1 members carry almost no traffic, commits `Starved` after its dwell (`ops::next`, OPEN-QUESTIONS
/// R38). Only the floor moves: a breach at 1% still runs the undo.
pub fn window_start(fold: &ReleaseFold, c: &Current) -> Timestamp {
    let steps = crate::ops::steps(fold, c, crate::ops::Dwell::default()).unwrap_or_default();
    let pct = |k: usize| match steps.get(k) {
        Some(Step::Deploy { pct, .. }) => Some(*pct),
        _ => None,
    };
    let k = usize::from(c.next);
    if k > 0 && pct(k) == Some(10) && pct(k - 1) == Some(1) { c.before } else { c.since }
}

/// Whether N's counters over the step's window are too few to judge against, so the host reads N's from the
/// intent's start instead: the 1, 10 and 50% steps, where N last ran beside N+1. At 100% N's new window holds only
/// the members that had not yet moved (207 entries on graft-m4a, OPEN-QUESTIONS R44).
pub fn n_from_start(n_step: &Counters) -> bool {
    n_step.entries < STEP_MIN_ENTRIES
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
    /// Both other voter sites report at least `MIN_CALLS` calls to it over `ZERO_ACKS` and every one missed: the
    /// "zero acks" a `ZERO_ACKS` `ReplaceMember` needs (`loss`).
    pub silent: bool,
    /// Both other voter sites report under `UNDRAIN_BURN` over `RECOVER_AFTER`: a site `ReplaceMember` moved off has
    /// recovered and may be undrained ("Install operations", Site loss).
    pub recovered: bool,
}

/// How long a replaced site must stay under `UNDRAIN_BURN` from both other sites before the drain key undrains it
/// ("Install operations", Site loss: "under `UNDRAIN_BURN` for 24 h"). Only a test-faults `graft-ops` reads a shorter
/// window (`OPS_RECOVER_MS`), for a hosted row whose lines do not measure this length.
pub const RECOVER_AFTER: u64 = 24 * 3_600_000;

/// Adds `silent` and `recovered` to `v` from the reports over `ZERO_ACKS` (`zero`) and over the host's recovery window
/// (`recover`), with the same rule as `verdict`: both other voter sites, each report at least `MIN_CALLS`.
pub fn loss(v: &mut BTreeMap<u8, Verdict>, voters: &[SiteId], zero: &[SiteHealth], recover: &[SiteHealth]) {
    let all = |t: SiteId, reports: &[SiteHealth], ok: &dyn Fn(&SiteHealth) -> bool| {
        voters.iter().filter(|o| **o != t).all(|o| reports.iter().any(|h| h.observer == *o && h.target == t && ok(h)))
    };
    for t in voters {
        let e = v.entry(t.0).or_default();
        e.silent = all(*t, zero, &|h| h.evidence() && h.missed == h.calls);
        e.recovered = all(*t, recover, &|h| h.under(UNDRAIN_BURN));
    }
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
            (t.0, Verdict { burning, calm, ..Verdict::default() })
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
