//! `graft-ops` as the executor of signed intents, the pure half (`build/BUILD-PLAN.md`, M5-WP3; `design/DESIGN.md`,
//! Shape, "Release group", `Intent` and `SignedIntent`; "Install operations", Tools, Release roll, and Site day).
//!
//! `graft-ops` holds no state of its own. Each run reads the `Release` view, the scripts' deployments, and the current
//! step's gate, and `next` says the one thing to do. Every step is idempotent: a deploy that the deployment already
//! shows is skipped, a drain the view already holds is skipped, and the group refuses an `IntentStep` that is not the
//! next one. So a restarted `graft-ops` reruns only the first step with no `IntentStep`.
//!
//! - **Uploads.** `check_bundle` takes the release bucket's files only when `release.json` verifies under
//!   `Keys.release` and lists every file with its SHA-256, and no file it lists is missing. `upload_next` uploads each
//!   script's version, reusing one an earlier run uploaded under the same release tag, then commits `ReleaseUploaded`.
//! - **Forward steps.** `ROLL_STEPS` per kind (`group::roll_steps`, and a site day's drain, 1%, 10%, 50%, 100%,
//!   undrain, first day). Each deploy step waits `STEP_DWELL` (a 1%-for-a-day roll's first step `FIRST_DAY`) and a
//!   passing gate, then commits `IntentStep`. A site's 100% commits `SiteRelease` before its `IntentStep`. The last
//!   step's `IntentStep` is followed by `IntentDone`.
//! - **A breach** submits the intent's first unused pre-signed undo. The group refuses one that cannot read an enabled
//!   format, and pages; `graft-ops` then pages and moves nothing.
//! - **Undo and `Abort`.** A `Rollback` runs drain, N at 100%, then undrain; a pointer undo deploys the pin at 100%; an
//!   `Abort` moves the intent's target to `target` the same way. Each step commits an `IntentStep` under the intent, and
//!   `IntentDone` ends it.
//! - **`release roll`.** `next_roll` picks the first intent of the roll order that R has not finished, skipping a script
//!   or images R leaves unchanged, with its pre-signed undos.

use std::collections::BTreeMap;

use graftyard_types::{ReleaseId, SiteId, Timestamp};
use sha2::{Digest as _, Sha256};

use crate::gate::{STUCK_DAY, Verdict};
use crate::group::{Current, ReleaseFold, Upload, roll_steps, undo_entry, verify_raw};
use crate::types::{
    DayTarget, Format, FormatSet, ImageRef, Intent, IntentState, ReleaseEntry, ReleaseUploaded, Script, Signature,
    VersionId,
};
use crate::{Refusal, SignedEntry};

/// Least time at each of 1%, 10%, and 50% before the step's gate is judged (\"Install operations\", Site day) [guess].
pub const STEP_DWELL: u64 = 2 * 3_600_000;
/// A site's first day after undrain, and a 1%-for-a-day roll's first step (`FIRST_DAY`) [guess].
pub const FIRST_DAY: u64 = 24 * 3_600_000;

/// The dwells `next` waits. Only a test install lowers them, so a hosted run walks a roll in minutes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dwell {
    pub step: u64,
    pub first_day: u64,
}

impl Default for Dwell {
    fn default() -> Dwell {
        Dwell { step: STEP_DWELL, first_day: FIRST_DAY }
    }
}

/// A script's deployment: each version and its percentage, a version at 0% included.
pub type Split = Vec<(VersionId, u8)>;

/// A script's current deployment and when it was created, from the deployments API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deployment {
    pub split: Split,
    pub created: Timestamp,
}

/// The current step's gate on the N+1 members or version, as `graft-ops` read it (`crate::gate` judges it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    Pass,
    /// Not judged: the inputs could not be read.
    Pending,
    /// No breach, but the N+1 members applied only this many entries, under `STEP_MIN_ENTRIES`.
    Starved(u64),
    Breach(Box<str>),
}

/// What `graft-ops` read this run.
#[derive(Clone, Debug)]
pub struct World {
    pub now: Timestamp,
    /// Each script's deployment, from the deployments API. A script missing here is one the API did not answer for.
    pub deployments: BTreeMap<Script, Deployment>,
    pub gate: Gate,
    /// Each voter site's verdict from both other sites' `SiteHealth` (`crate::gate::verdict`); a site missing here is
    /// one with no verdict, which drains and undrains nothing.
    pub health: BTreeMap<u8, Verdict>,
}

/// The key a commit takes: the executor key, or the drain key for a site day's `SiteMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signer {
    Executor,
    Drain,
}

/// The one thing to do now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing current, or the current intent is halted.
    Idle,
    /// Create this deployment of `script` through the versions and deployments API.
    Deploy { script: Script, split: Split },
    /// Commit `body` bound to the group's last digest and signed by `signer`.
    Commit { body: ReleaseEntry, signer: Signer },
    /// Submit this pre-signed undo as it is.
    Undo(Box<SignedEntry>),
    /// Run again later: a dwell, a pending gate, or a deployment the API did not report.
    Wait { until: Option<Timestamp>, why: Box<str> },
    /// Write a page and move nothing.
    Page(Box<str>),
}

/// One step of an intent, an undo, or an `Abort`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// A drain-key `SiteMode { Drain }`, done once the view holds the site drained.
    Drain(SiteId),
    /// A drain-key `SiteMode { Normal }`, done once the view holds the site undrained.
    Undrain(SiteId),
    /// The script's deployment set to `split`; then `dwell` and the gate (when `gated`), and for a site at 100%, its
    /// `SiteRelease`. `pct` is R's share the step means; with no N recorded, `split` is R alone, which only a
    /// deployment that already serves R alone may take.
    Deploy { script: Script, split: Split, release: ReleaseId, dwell: u64, gated: bool, pct: u8 },
    /// A wait with no deployment change: a site's first day.
    Hold(u64),
    /// A `ContainerDay` step: `pct` of the site's shards, in rendezvous order, restart onto the new image as they read
    /// the view (`ReleaseFold::shard_day`); no API call. Then `dwell` from the step's start and the gate.
    Shards { pct: u8, dwell: u64 },
}

pub(crate) fn version(fold: &ReleaseFold, r: &ReleaseId, s: Script) -> Option<VersionId> {
    fold.uploads.get(&r.0)?.versions.iter().find(|(x, _)| *x == s).map(|(_, v)| *v)
}

/// The release a script runs now, as the group recorded it: a site's `SiteRelease`, a pinned script's kept release
/// while its roll runs, or the release its last finished roll left.
pub fn running(fold: &ReleaseFold, s: Script) -> Option<ReleaseId> {
    match s {
        Script::Site(site) => fold.site_release.get(&site.0).copied().or_else(|| {
            let g = fold.genesis.as_ref()?;
            g.sites.iter().find(|x| x.id == site)?.release
        }),
        _ => fold.rolled.get(&s).copied(),
    }
}

/// A deploy of `to` at 100%, with `keep` beside it at 0% so a pin to it still applies.
fn at_full(fold: &ReleaseFold, s: Script, to: ReleaseId, keep: Option<ReleaseId>) -> Result<Split, Box<str>> {
    let v = version(fold, &to, s).ok_or_else(|| format!("release {} uploaded no {s:?} version", hex(&to)))?;
    let mut split = vec![(v, 100)];
    if let Some(k) = keep.and_then(|k| version(fold, &k, s)).filter(|k| *k != v) {
        split.push((k, 0));
    }
    Ok(split)
}

fn hex(r: &ReleaseId) -> String {
    crate::types::hex(&r.0)
}

/// A move of `target` to `release` with no gate: drain, the release at 100%, undrain for a site; the release at 100%
/// for a script, with the rolled release `rolled` kept beside it at 0% for a pinned script, so the group's record of
/// the deployment (`deployed`) holds. A `Containers` target needs no API call: shards read the view.
fn return_steps(
    fold: &ReleaseFold,
    target: Script,
    release: ReleaseId,
    rolled: Option<ReleaseId>,
) -> Result<Vec<Step>, Box<str>> {
    let keep = match target {
        Script::Edge | Script::Ops | Script::Mcp | Script::Controller => rolled,
        _ => None,
    };
    let split = at_full(fold, target, release, keep)?;
    let deploy = Step::Deploy { script: target, split, release, dwell: 0, gated: false, pct: 100 };
    Ok(match target {
        Script::Site(s) => vec![Step::Drain(s), deploy, Step::Undrain(s)],
        _ => vec![deploy],
    })
}

/// The steps of the current intent in its state. `Err` for a kind this executor does not run.
pub fn steps(fold: &ReleaseFold, c: &Current, dwell: Dwell) -> Result<Vec<Step>, Box<str>> {
    let i = &c.intent.intent;
    match c.state {
        IntentState::Undoing => {
            let mut out = Vec::new();
            for u in &c.used {
                match u {
                    ReleaseEntry::Rollback { target: DayTarget::Containers(_), .. } => {}
                    ReleaseEntry::Rollback { target: DayTarget::Site(s), release } => {
                        out.extend(return_steps(fold, Script::Site(*s), *release, None)?);
                    }
                    ReleaseEntry::Rollback { target: DayTarget::Workflows, release } => {
                        out.extend(return_steps(fold, Script::Workflows, *release, None)?);
                    }
                    u => {
                        let (s, pin) = u.pointer().ok_or("an undo that is no Rollback and no pointer")?;
                        out.extend(return_steps(fold, s, pin, i.release())?);
                    }
                }
            }
            Ok(out)
        }
        IntentState::Aborting { target } => match i {
            Intent::ContainerDay { .. } => Ok(Vec::new()),
            _ => match i.script() {
                Some(s) => return_steps(fold, s, target, i.release()),
                None => Err(format!("an Abort of a {} moves nothing this executor runs", i.kind()).into()),
            },
        },
        IntentState::Running { .. } | IntentState::Halted { .. } if matches!(i, Intent::ContainerDay { .. }) => {
            let ps = roll_steps(i).expect("a ContainerDay has roll steps");
            Ok(ps.iter().map(|p| Step::Shards { pct: *p, dwell: if *p < 100 { dwell.step } else { 0 } }).collect())
        }
        IntentState::Running { .. } | IntentState::Halted { .. } => {
            let r = i.release().ok_or_else(|| format!("{} is not run by this executor", i.kind()))?;
            let s = i.script().ok_or_else(|| format!("{} is M5-WP4's", i.kind()))?;
            let v = version(fold, &r, s).ok_or_else(|| format!("release {} uploaded no {s:?} version", hex(&r)))?;
            // N: a pinned script's kept release, recorded when this intent committed; any other script's running one.
            let n = match i.pinned() {
                Some(_) => fold.deployed.get(&s).and_then(|(kept, _)| *kept),
                None => running(fold, s),
            }
            .filter(|n| *n != r)
            .and_then(|n| version(fold, &n, s));
            // A pinned script keeps N beside R at 0% after 100%, so its pre-signed pointer undo still applies ("A
            // version override will only be applied if the specified version is in the current deployment").
            let pinned = i.pinned().is_some();
            let pct = |p: u8, d: u64| {
                let split = match n {
                    Some(n) if p < 100 || pinned => vec![(v, p), (n, 100 - p)],
                    _ => vec![(v, 100)],
                };
                Step::Deploy { script: s, split, release: r, dwell: d, gated: true, pct: p }
            };
            Ok(match (i, roll_steps(i)) {
                (Intent::SiteDay { target: DayTarget::Site(site), .. }, _) => vec![
                    Step::Drain(*site),
                    pct(1, dwell.step),
                    pct(10, dwell.step),
                    pct(50, dwell.step),
                    pct(100, 0),
                    Step::Undrain(*site),
                    Step::Hold(dwell.first_day),
                ],
                (Intent::SiteDay { .. }, _) => {
                    vec![pct(1, dwell.step), pct(10, dwell.step), pct(50, dwell.step), pct(100, 0)]
                }
                (Intent::EdgeRoll { .. }, Some(ps)) => {
                    ps.iter().map(|p| pct(*p, if *p < 100 { dwell.step } else { 0 })).collect()
                }
                (_, Some(ps)) => ps.iter().map(|p| pct(*p, if *p < 100 { dwell.first_day } else { 0 })).collect(),
                (_, None) => return Err(format!("{} has no roll steps", i.kind()).into()),
            })
        }
    }
}

/// Whether a deployment is `want`: the same versions at the same percentages, in any order.
fn same(have: &Split, want: &Split) -> bool {
    let (mut a, mut b) = (have.clone(), want.clone());
    a.sort_unstable();
    b.sort_unstable();
    a == b
}

/// The one thing `graft-ops` does now for the current intent.
pub fn next(fold: &ReleaseFold, w: &World, dwell: Dwell) -> Action {
    if let Some(a) = auto_drain(fold, w) {
        return a;
    }
    let Some(c) = fold.current.as_deref() else { return Action::Idle };
    if let IntentState::Halted { .. } = c.state {
        return Action::Idle;
    }
    let id = c.intent.id;
    let steps = match steps(fold, c, dwell) {
        Ok(s) => s,
        Err(why) => return Action::Page(format!("intent {}: {why}", id.0).into()),
    };
    let k = usize::from(c.next);
    let Some(step) = steps.get(k) else {
        return Action::Commit { body: ReleaseEntry::IntentDone { intent: id }, signer: Signer::Executor };
    };
    let done = Action::Commit { body: ReleaseEntry::IntentStep { intent: id, step: c.next }, signer: Signer::Executor };
    // Another site drained (an automatic drain, or a human's) stops a site day's drain and forward deploys: the group
    // refuses a second drain-key drain, and the day's N+1 members stay under the 1% lead limit until it ends.
    let other_drained = |s: SiteId| fold.drained.keys().find(|d| **d != s.0).copied();
    let undrain = |s: SiteId| Action::Commit {
        body: ReleaseEntry::SiteMode(crate::types::SiteMode { site: s, mode: crate::types::Mode::Normal }),
        signer: Signer::Drain,
    };
    // `STUCK_DAY`: a site day drained by the drain key that cannot advance from `from` undrains its site under the 1%
    // lead limit, and goes on from the same step once it can.
    let stuck = |script: &Script, from: Timestamp| match script {
        Script::Site(s)
            if fold.drained.get(&s.0).is_some_and(|d| d.by_drain_key)
                && w.now.0 >= from.0.saturating_add(STUCK_DAY) =>
        {
            Some(undrain(*s))
        }
        _ => None,
    };
    match step {
        Step::Drain(s) if fold.drained.contains_key(&s.0) => done,
        Step::Drain(s) if let Some(d) = other_drained(*s) => {
            Action::Wait { until: None, why: format!("site {d} is drained; site {} waits to drain", s.0).into() }
        }
        Step::Drain(s) => Action::Commit {
            body: ReleaseEntry::SiteMode(crate::types::SiteMode { site: *s, mode: crate::types::Mode::Drain }),
            signer: Signer::Drain,
        },
        Step::Undrain(s) => match fold.drained.get(&s.0) {
            None => done,
            Some(d) if d.by_drain_key => undrain(*s),
            Some(_) => {
                Action::Page(format!("site {} was drained by an intent key; its Normal is a human's", s.0).into())
            }
        },
        Step::Hold(d) => {
            // The first day is gated like every step: a breach in it runs the undo ("Install operations", Site day).
            if let Gate::Breach(why) = &w.gate {
                return breach(fold, c, why);
            }
            let until = Timestamp(c.since.0.saturating_add(*d));
            if w.now < until { Action::Wait { until: Some(until), why: "first day".into() } } else { done }
        }
        Step::Shards { pct, dwell } => {
            if let Gate::Breach(why) = &w.gate {
                return breach(fold, c, why);
            }
            let until = Timestamp(c.since.0.saturating_add(*dwell));
            if w.now < until {
                return Action::Wait { until: Some(until), why: format!("STEP_DWELL at {pct}% of shards").into() };
            }
            if w.gate == Gate::Pending {
                return Action::Wait { until: None, why: "gate pending".into() };
            }
            done
        }
        Step::Deploy { script, split, release, dwell, gated, pct } => {
            // A breach needs no deployment read: the undo is a signed entry, so it goes in while the API is down
            // ("Install operations", the November 2, 2023 class), and its return steps wait for the API.
            if *gated && let Gate::Breach(why) = &w.gate {
                return breach(fold, c, why);
            }
            if *gated
                && let Script::Site(s) = script
                && let Some(d) = other_drained(*s)
            {
                return Action::Wait { until: None, why: format!("site {d} is drained; the site day waits").into() };
            }
            let Some(dep) = w.deployments.get(script) else {
                if let Some(a) = stuck(script, c.since) {
                    return a;
                }
                return Action::Wait { until: None, why: format!("no deployment of {script:?} read").into() };
            };
            let have = &dep.split;
            // With no N in the group's record (a script's first roll), N is the one other version the deployment holds.
            let mut split = split.clone();
            if *pct < 100 && split.len() == 1 {
                let v = split[0].0;
                let others: Vec<VersionId> = have.iter().map(|(x, _)| *x).filter(|x| *x != v).collect();
                let [n] = others.as_slice() else {
                    return Action::Page(
                        format!("intent {}: no N for {script:?}'s {pct}% step in {have:?}", id.0).into(),
                    );
                };
                split = vec![(v, *pct), (*n, 100 - pct)];
            }
            let split = &split;
            if !same(have, split) {
                return Action::Deploy { script: *script, split: split.clone() };
            }
            if *gated {
                // The dwell runs from when the split took effect, which a restart between the deploy and the step
                // cannot shorten.
                let start = Timestamp(c.since.0.max(dep.created.0));
                if w.gate == Gate::Pending {
                    if let Some(a) = stuck(script, start) {
                        return a;
                    }
                    return Action::Wait { until: None, why: "gate pending".into() };
                }
                let until = Timestamp(start.0.saturating_add(*dwell));
                if w.now < until {
                    return Action::Wait { until: Some(until), why: "STEP_DWELL".into() };
                }
                // Too few entries: dwell on, up to twice the step's dwell, then page.
                if let Gate::Starved(n) = w.gate
                    && *dwell > 0
                {
                    let cap = Timestamp(start.0.saturating_add(2 * *dwell));
                    if w.now < cap {
                        return Action::Wait { until: Some(cap), why: "STEP_MIN_ENTRIES".into() };
                    }
                    return Action::Page(
                        format!(
                            "intent {}: {script:?}'s {pct}% step applied {n} entries in 2 x its dwell, under \
                             STEP_MIN_ENTRIES; it needs a Halt, an Abort, or traffic",
                            id.0
                        )
                        .into(),
                    );
                }
            }
            if let Script::Site(s) = script
                && split.iter().any(|(_, p)| *p == 100)
                && fold.site_release.get(&s.0) != Some(release)
            {
                return Action::Commit {
                    body: ReleaseEntry::SiteRelease { site: *s, release: *release },
                    signer: Signer::Executor,
                };
            }
            done
        }
    }
}

/// The site the current intent's site day rolls.
fn day_site(fold: &ReleaseFold) -> Option<SiteId> {
    match fold.current.as_deref()?.intent.intent {
        Intent::SiteDay { target: DayTarget::Site(s), .. } => Some(s),
        _ => None,
    }
}

/// Automatic drain and undrain by the drain key (journey 5, \"Automatic drain\"), before any intent's step:
/// - a drain-key drain that is not the current site day's own ends once both other sites call the site calm;
/// - a site both other sites call burning is drained, unless one is drained already;
/// - a drain-key drain of the current site day's site gives way first: its `Normal` goes in, the day's N+1 members
///   stay under the 1% lead limit (`ReleaseFold::lead_limited`), and the sick site is drained on the next action;
/// - a burning site with another site drained by any other cause pages, because the group refuses a second drain.
fn auto_drain(fold: &ReleaseFold, w: &World) -> Option<Action> {
    let day = day_site(fold);
    let mode = |site: u8, mode: crate::types::Mode| Action::Commit {
        body: ReleaseEntry::SiteMode(crate::types::SiteMode { site: SiteId(site), mode }),
        signer: Signer::Drain,
    };
    let verdict = |s: &u8| w.health.get(s).copied().unwrap_or_default();
    if let Some((s, _)) = fold
        .drained
        .iter()
        .find(|(s, d)| d.by_drain_key && day != Some(SiteId(**s)) && verdict(s).calm && !verdict(s).burning)
    {
        return Some(mode(*s, crate::types::Mode::Normal));
    }
    let (sick, _) =
        w.health.iter().find(|(s, v)| v.burning && !fold.drained.contains_key(*s) && !fold.lost.contains(*s))?;
    Some(match fold.drained.iter().next() {
        None => mode(*sick, crate::types::Mode::Drain),
        Some((s, d)) if d.by_drain_key && day == Some(SiteId(*s)) => mode(*s, crate::types::Mode::Normal),
        Some((s, _)) => Action::Page(
            format!("site {sick} burns DRAIN_BURN, but site {s} is drained, so the drain key cannot drain it").into(),
        ),
    })
}

/// A gate breach on a forward step: the intent's first pre-signed undo not yet committed, or, for an intent that
/// carries none (`FrontRoll`), N at 100% through the API under the same intent, and a page.
fn breach(fold: &ReleaseFold, c: &Current, why: &str) -> Action {
    let install = fold.genesis.as_ref().expect("installed").install;
    if let Some((u, sig)) = c.undo.iter().find(|(u, _)| !c.used.contains(u)) {
        return Action::Undo(Box::new(undo_entry(install, c.intent.id, u, c.undo_key, *sig)));
    }
    Action::Page(
        format!("intent {}: gate breach ({why}) and no undo left; it needs a Halt or an Abort", c.intent.id.0).into(),
    )
}

/// A release bundle from the release bucket that passed `check_bundle`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub release: ReleaseId,
    pub record: Box<[u8]>,
    pub sig: Signature,
    /// Each script and its modules: the bundle files under the script's folder, folder removed.
    pub scripts: BTreeMap<Script, Vec<(String, Vec<u8>)>>,
    pub images: Vec<ImageRef>,
    pub reads: FormatSet,
}

/// The bundle folder each script's modules are under. `site/` holds the one site script bundle every site's script
/// runs, so a release uploads it once per site.
pub fn folder(s: Script) -> &'static str {
    match s {
        Script::Site(_) => "site/",
        Script::Workflows => "workflows/",
        Script::Front => "front/",
        Script::Edge => "edge/",
        Script::Ops => "ops/",
        Script::Mcp => "mcp/",
        Script::AgentsFront => "agents-front/",
        Script::Controller => "controller/",
    }
}

fn bad(why: impl Into<String>) -> Refusal {
    Refusal::BadRecord(why.into().into_boxed_str())
}

/// Takes a release's files from the release bucket only when `release.json` verifies under `Keys.release`, lists
/// `formats.txt` and every other file with its SHA-256, and every file it lists is there. Returns the scripts each
/// genesis site and this account's scripts upload (`scripts` names the non-site scripts this `graft-ops` deploys).
pub fn check_bundle(
    fold: &ReleaseFold,
    record: &[u8],
    sig: Signature,
    files: &[(String, Vec<u8>)],
    scripts: &[Script],
) -> Result<Checked, Refusal> {
    let keys = fold.keys.as_ref().ok_or(Refusal::BadState("no Release view yet"))?;
    verify_raw(&keys.release, record, &sig).map_err(|_| bad("release.json does not verify under Keys.release"))?;
    let rec: serde_json::Value = serde_json::from_slice(record).map_err(|_| bad("release.json is not JSON"))?;
    let listed: BTreeMap<&str, &str> = rec["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| Some((f["name"].as_str()?, f["sha256"].as_str()?)))
        .collect();
    for (name, bytes) in files {
        let want = crate::types::hex(&Sha256::digest(bytes));
        if listed.get(name.as_str()) != Some(&want.as_str()) {
            return Err(bad(format!("release.json lists no {name} with SHA-256 {want}")));
        }
    }
    if let Some(missing) = listed.keys().find(|n| !files.iter().any(|(f, _)| f == *n)) {
        return Err(bad(format!("the bucket holds no {missing}, which release.json lists")));
    }
    let formats = files.iter().find(|(n, _)| n == "formats.txt").ok_or_else(|| bad("the bundle has no formats.txt"))?;
    let reads = parse_formats(&formats.1).ok_or_else(|| bad("formats.txt is not one family.version per line"))?;
    let genesis = fold.genesis.as_ref().expect("installed");
    let mut want: Vec<Script> = genesis.sites.iter().map(|s| Script::Site(s.id)).collect();
    want.extend(scripts.iter().copied().filter(|s| !matches!(s, Script::Site(_))));
    let mut out = BTreeMap::new();
    for s in want {
        let mods: Vec<(String, Vec<u8>)> =
            files.iter().filter_map(|(n, b)| n.strip_prefix(folder(s)).map(|m| (m.to_string(), b.clone()))).collect();
        if !mods.is_empty() {
            out.insert(s, mods);
        }
    }
    if out.is_empty() {
        return Err(bad("the bundle holds no script this install runs"));
    }
    let images = rec["images"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| {
            let d = i["digest"].as_str()?.strip_prefix("sha256:")?;
            Some(ImageRef { name: i["name"].as_str()?.into(), digest: graftyard_types::Digest::parse_hex(d)? })
        })
        .collect();
    Ok(Checked {
        release: ReleaseId(Sha256::digest(record).into()),
        record: record.into(),
        sig,
        scripts: out,
        images,
        reads,
    })
}

/// The bundle folder the vault Worker's modules are under. No `Script` names it: `graft-ops` never deploys into the
/// vault account, so `check_bundle` leaves the folder alone and only `graftyard vault init` and `vault upgrade` take it.
pub const VAULT_FOLDER: &str = "vault/";

/// The vault Worker of one verified release: its id and its modules, folder removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultArtifact {
    pub release: ReleaseId,
    pub modules: Vec<(String, Vec<u8>)>,
}

/// Takes a release's vault Worker only when `release.json` verifies under `release_key` (the genesis's
/// `Keys.release`), lists every file under `vault/` it is given with that file's SHA-256, every `vault/` file it lists
/// is given, and the folder holds `entry.js`. Files outside `vault/` are not read, so a whole release folder passes.
pub fn check_vault(
    release_key: &crate::types::KeyId,
    record: &[u8],
    sig: Signature,
    files: &[(String, Vec<u8>)],
) -> Result<VaultArtifact, Refusal> {
    verify_raw(release_key, record, &sig).map_err(|_| bad("release.json does not verify under Keys.release"))?;
    let rec: serde_json::Value = serde_json::from_slice(record).map_err(|_| bad("release.json is not JSON"))?;
    let listed: BTreeMap<&str, &str> = rec["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| Some((f["name"].as_str()?, f["sha256"].as_str()?)))
        .filter(|(n, _)| n.starts_with(VAULT_FOLDER))
        .collect();
    let mut modules = Vec::new();
    for (name, bytes) in files {
        let Some(module) = name.strip_prefix(VAULT_FOLDER) else { continue };
        let want = crate::types::hex(&Sha256::digest(bytes));
        if listed.get(name.as_str()) != Some(&want.as_str()) {
            return Err(bad(format!("release.json lists no {name} with SHA-256 {want}")));
        }
        modules.push((module.to_string(), bytes.clone()));
    }
    if let Some(missing) = listed.keys().find(|n| !files.iter().any(|(f, _)| f == *n)) {
        return Err(bad(format!("the release holds no {missing}, which release.json lists")));
    }
    if !modules.iter().any(|(m, _)| m == "entry.js") {
        return Err(bad("the release has no vault/entry.js"));
    }
    modules.sort();
    Ok(VaultArtifact { release: ReleaseId(Sha256::digest(record).into()), modules })
}

fn parse_formats(b: &[u8]) -> Option<FormatSet> {
    let text = std::str::from_utf8(b).ok()?;
    let mut v = Vec::new();
    for l in text.lines() {
        let (f, ver) = l.split_once('.')?;
        v.push(Format { family: f.parse().ok()?, version: ver.parse().ok()? });
    }
    let set = FormatSet::new(v);
    // The group checks the file's SHA-256 against what it rebuilds from `reads`, so only the canonical form passes.
    (set.file().as_bytes() == b).then_some(set)
}

/// One upload step: a script's version, or the `ReleaseUploaded` once every script has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadStep {
    /// Upload this script's modules as a version tagged with the release id, without deploying it.
    Upload(Script),
    Commit(Box<ReleaseUploaded>),
    /// The group already holds this release's `ReleaseUploaded`.
    Done,
}

/// The next upload step. `tagged` holds the versions an earlier run uploaded with this release's tag, read back from
/// the versions API, so a restart uploads no script twice.
pub fn upload_next(fold: &ReleaseFold, c: &Checked, tagged: &BTreeMap<Script, VersionId>) -> UploadStep {
    if fold.uploads.contains_key(&c.release.0) {
        return UploadStep::Done;
    }
    if let Some(s) = c.scripts.keys().find(|s| !tagged.contains_key(s)) {
        return UploadStep::Upload(*s);
    }
    UploadStep::Commit(Box::new(ReleaseUploaded {
        release: c.release,
        record: c.record.clone(),
        record_sig: c.sig,
        versions: c.scripts.keys().map(|s| (*s, tagged[s])).collect(),
        images: c.images.iter().cloned().collect(),
        reads: c.reads.clone(),
    }))
}

/// What `graftyard release roll R` does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Roll {
    /// R has no `ReleaseUploaded` yet, or an intent is current.
    Wait(Box<str>),
    /// Sign and submit this intent with these pre-signed undos.
    Next {
        intent: Intent,
        undo: Vec<ReleaseEntry>,
    },
    Done,
}

/// The files of `s`'s folder in an upload, by name and SHA-256.
fn files_of(u: &Upload, s: Script) -> Vec<&(Box<str>, [u8; 32])> {
    u.files.iter().filter(|(n, _)| n.starts_with(folder(s))).collect()
}

/// The first intent of the roll order (\"Install operations\", Release roll) that R has not finished: each member
/// site's `SiteDay`, then each spare's, then `SiteDay { Workflows }`, then `EdgeRoll`, `OpsRoll`, `FrontRoll`, and
/// `McpRoll`, then a `ContainerDay` for each site whose images changed. It skips an intent whose script, or whose
/// images, R leaves as the release running them has them. Each carries its undo: a `Rollback` to N for a day target, a
/// pointer to N for a pinned script, none for `FrontRoll` and for a script with no N yet.
pub fn next_roll(fold: &ReleaseFold, r: ReleaseId) -> Roll {
    let Some(up) = fold.uploads.get(&r.0) else {
        return Roll::Wait(format!("release {} has no ReleaseUploaded", hex(&r)).into());
    };
    if let Some(c) = &fold.current {
        return Roll::Wait(format!("intent {} ({}) is current", c.intent.id.0, c.intent.intent.kind()).into());
    }
    let g = fold.genesis.as_ref().expect("installed");
    let sites = g.sites.iter().filter(|s| !s.spare).chain(g.sites.iter().filter(|s| s.spare));
    let mut order: Vec<Intent> = sites.map(|s| Intent::SiteDay { target: DayTarget::Site(s.id), release: r }).collect();
    order.push(Intent::SiteDay { target: DayTarget::Workflows, release: r });
    order.extend([
        Intent::EdgeRoll { release: r },
        Intent::OpsRoll { release: r },
        Intent::FrontRoll { release: r },
        Intent::McpRoll { release: r },
    ]);
    for i in order {
        let s = i.script().expect("a code intent names its script");
        if !up.versions.iter().any(|(x, _)| *x == s) {
            continue;
        }
        let n = running(fold, s);
        if n == Some(r) {
            continue;
        }
        // Unchanged only when both records list the script's files and they match; a record that lists none for it
        // cannot show it unchanged.
        let mine = files_of(up, s);
        if let Some(nu) = n.and_then(|n| fold.uploads.get(&n.0))
            && !mine.is_empty()
            && files_of(nu, s) == mine
        {
            continue;
        }
        let undo = match (n, &i) {
            (None, _) | (_, Intent::FrontRoll { .. }) => Vec::new(),
            (Some(n), Intent::SiteDay { target, .. }) => vec![ReleaseEntry::Rollback { target: *target, release: n }],
            (Some(n), _) => vec![pointer(s, n)],
        };
        return Roll::Next { intent: i, undo };
    }
    for s in &g.sites {
        if fold.containers.get(&s.id.0) == Some(&r) || up.images.is_empty() {
            continue;
        }
        let n = fold.containers.get(&s.id.0).copied().or_else(|| running(fold, Script::Site(s.id)));
        if let Some(nu) = n.and_then(|n| fold.uploads.get(&n.0))
            && nu.images == up.images
        {
            continue;
        }
        let undo = n
            .map(|n| vec![ReleaseEntry::Rollback { target: DayTarget::Containers(s.id), release: n }])
            .unwrap_or_default();
        return Roll::Next { intent: Intent::ContainerDay { site: s.id, release: r }, undo };
    }
    Roll::Done
}

fn pointer(s: Script, pin: ReleaseId) -> ReleaseEntry {
    match s {
        Script::Edge => ReleaseEntry::EdgePointer { pin },
        Script::Ops => ReleaseEntry::OpsPointer { pin },
        Script::Mcp => ReleaseEntry::McpPointer { pin },
        _ => ReleaseEntry::ControllerPointer { pin },
    }
}

/// `graftyard release roll`'s entry: `intent` with id `id`, issued at `now`, valid for `ttl` ms, its undos pre-signed by
/// the same key, bound to the group's last digest and signed by the key whose seed is `seed`.
pub fn sign_intent(
    fold: &ReleaseFold,
    intent: Intent,
    undo: &[ReleaseEntry],
    seed: &[u8; 32],
    id: crate::types::IntentId,
    now: Timestamp,
    ttl: u64,
) -> SignedEntry {
    let install = fold.genesis.as_ref().expect("installed").install;
    let undo = undo.iter().map(|u| (u.clone(), crate::group::presign_undo(install, id, u, seed))).collect();
    let body = ReleaseEntry::Intent(Box::new(crate::types::SignedIntent {
        id,
        issued_at: now,
        not_after: Timestamp(now.0.saturating_add(ttl)),
        intent,
        undo,
    }));
    let bound = crate::Bound { install, group: graftyard_types::ChainId::Release, prev: fold.last, parent: None };
    let mut e = SignedEntry::unsigned(bound, body);
    e.sign(seed);
    e
}

/// How a commit `graft-ops` sent ended: 201, 409 (another writer moved the group since its view), 422 with the
/// refusal's variant and text, or no outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submit {
    Committed,
    Stale,
    Refused { variant: Box<str>, message: Box<str> },
    Failed(Box<str>),
}

/// A release's files as the release bucket holds them: `release.json`, its signature, and every other file by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawBundle {
    pub record: Vec<u8>,
    pub sig: Signature,
    pub files: Vec<(String, Vec<u8>)>,
}

/// What one `graft-ops` run reads and does. The Worker (`workers/ops`) reaches the `Release` group, the deployments
/// API with the customer's deploy token, Secrets Store, and the release bucket; the tests use a fake world over the
/// real group.
#[allow(async_fn_in_trait)]
pub trait Exec {
    fn now(&self) -> Timestamp;
    /// The non-site scripts this `graft-ops` deploys (`check_bundle`'s `scripts`).
    fn scripts(&self) -> Vec<Script>;
    /// The group's view, refreshed from a member this run.
    async fn view(&mut self) -> Result<ReleaseFold, String>;
    async fn deployment(&mut self, s: Script) -> Result<Deployment, String>;
    /// The current step's gate (`crate::gate::judge_site`, `judge_errors`).
    async fn gate(&mut self, fold: &ReleaseFold) -> Gate;
    /// Each voter site's verdict from both other sites' `SiteHealth` objects (`crate::gate::verdict`).
    async fn health(&mut self, fold: &ReleaseFold) -> BTreeMap<u8, Verdict>;
    async fn deploy(&mut self, s: Script, split: &Split) -> Result<(), String>;
    async fn submit(&mut self, bytes: &[u8]) -> Submit;
    fn sign(&self, signer: Signer, e: &mut SignedEntry) -> Result<(), String>;
    fn page(&mut self, why: &str);
    /// The first release in the bucket that the group holds no `ReleaseUploaded` for.
    async fn bundle(&mut self, fold: &ReleaseFold) -> Result<Option<RawBundle>, String>;
    /// The versions an earlier run uploaded for `release`, by the release's tag.
    async fn tagged(&mut self, release: ReleaseId, scripts: &[Script]) -> Result<BTreeMap<Script, VersionId>, String>;
    /// Uploads one script's modules as a version tagged with `release`, without deploying it.
    async fn upload(&mut self, s: Script, release: ReleaseId, modules: &[(String, Vec<u8>)]) -> Result<(), String>;
}

/// What a run did, one line per action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ran {
    Uploaded(Script),
    Deployed(Script, Split),
    Committed(&'static str),
    /// A 409: the next pass of the run reads the view again.
    Stale(&'static str),
    Refused {
        kind: &'static str,
        variant: Box<str>,
    },
    Paged(Box<str>),
    Wait {
        until: Option<Timestamp>,
        why: Box<str>,
    },
    Idle,
    Error(Box<str>),
}

/// Most actions one run takes; the next cron run goes on from the group.
pub const RUN_MAX: usize = 32;

/// One `graft-ops` run: up to RUN_MAX actions, each decided on a view read just before it, so a run killed anywhere
/// leaves nothing a later run must know. Uploads come first, one script at a time; a bundle that fails its checks is
/// reported and the run goes on to the current intent. A commit the group refuses ends the run, and a refusal the
/// group pages on (`pages`) writes a page.
pub async fn run<E: Exec>(e: &mut E, dwell: Dwell) -> Vec<Ran> {
    let mut log = Vec::new();
    let mut uploads = true;
    for _ in 0..RUN_MAX {
        let fold = match e.view().await {
            Ok(f) => f,
            Err(x) => {
                log.push(Ran::Error(format!("view: {x}").into()));
                break;
            }
        };
        if uploads {
            match upload_step(e, &fold).await {
                Ok(None) => uploads = false,
                Ok(Some(Ran::Stale(k))) => {
                    log.push(Ran::Stale(k));
                    continue;
                }
                Ok(Some(r @ (Ran::Uploaded(_) | Ran::Committed(_)))) => {
                    log.push(r);
                    continue;
                }
                Ok(Some(r)) | Err(r) => {
                    log.push(r);
                    uploads = false;
                }
            }
        }
        let mut deployments = BTreeMap::new();
        if let Some(c) = fold.current.as_deref()
            && let Ok(steps) = steps(&fold, c, dwell)
        {
            for s in steps.iter().filter_map(|s| if let Step::Deploy { script, .. } = s { Some(*script) } else { None })
            {
                if deployments.contains_key(&s) {
                    continue;
                }
                if let Ok(d) = e.deployment(s).await {
                    deployments.insert(s, d);
                }
            }
        }
        let gate = if fold.current.is_some() { e.gate(&fold).await } else { Gate::Pending };
        let health = e.health(&fold).await;
        let w = World { now: e.now(), deployments, gate, health };
        let (bytes, kind) = match next(&fold, &w, dwell) {
            Action::Idle => {
                log.push(Ran::Idle);
                break;
            }
            Action::Wait { until, why } => {
                log.push(Ran::Wait { until, why });
                break;
            }
            Action::Page(why) => {
                e.page(&why);
                log.push(Ran::Paged(why));
                break;
            }
            Action::Deploy { script, split } => {
                match e.deploy(script, &split).await {
                    Ok(()) => log.push(Ran::Deployed(script, split)),
                    Err(x) => {
                        log.push(Ran::Error(format!("deploy {script:?}: {x}").into()));
                        break;
                    }
                }
                continue;
            }
            Action::Commit { body, signer } => {
                let kind = body.kind();
                match signed(e, &fold, body, signer) {
                    Ok(b) => (b, kind),
                    Err(r) => {
                        log.push(r);
                        break;
                    }
                }
            }
            Action::Undo(u) => {
                let kind = u.body.kind();
                (u.encode(), kind)
            }
        };
        if !submitted(e, &bytes, kind, &mut log).await {
            break;
        }
    }
    log
}

fn signed<E: Exec>(e: &E, fold: &ReleaseFold, body: ReleaseEntry, signer: Signer) -> Result<Vec<u8>, Ran> {
    let install = fold.genesis.as_ref().ok_or(Ran::Error("no genesis in the view".into()))?.install;
    let bound = crate::Bound { install, group: graftyard_types::ChainId::Release, prev: fold.last, parent: None };
    let mut s = SignedEntry::unsigned(bound, body);
    e.sign(signer, &mut s).map_err(|x| Ran::Error(format!("sign: {x}").into()))?;
    Ok(s.encode())
}

/// Sends one entry. True when the run goes on: it committed, or it met 409 and the next pass reads the view again.
async fn submitted<E: Exec>(e: &mut E, bytes: &[u8], kind: &'static str, log: &mut Vec<Ran>) -> bool {
    match e.submit(bytes).await {
        Submit::Committed => {
            log.push(Ran::Committed(kind));
            true
        }
        Submit::Stale => {
            log.push(Ran::Stale(kind));
            true
        }
        Submit::Refused { variant, message } => {
            if crate::pages(&variant) {
                let why = format!("{kind} refused, {variant}: {message}");
                e.page(&why);
                log.push(Ran::Paged(why.into()));
            }
            log.push(Ran::Refused { kind, variant });
            false
        }
        Submit::Failed(x) => {
            log.push(Ran::Error(format!("{kind}: {x}").into()));
            false
        }
    }
}

/// One upload action, or `None` when the bucket holds no release the group lacks.
async fn upload_step<E: Exec>(e: &mut E, fold: &ReleaseFold) -> Result<Option<Ran>, Ran> {
    let b = match e.bundle(fold).await {
        Ok(Some(b)) => b,
        Ok(None) => return Ok(None),
        Err(x) => return Err(Ran::Error(format!("release bucket: {x}").into())),
    };
    let c = check_bundle(fold, &b.record, b.sig, &b.files, &e.scripts())
        .map_err(|r| Ran::Error(format!("release bundle refused: {r}").into()))?;
    let scripts: Vec<Script> = c.scripts.keys().copied().collect();
    let tagged = e.tagged(c.release, &scripts).await.map_err(|x| Ran::Error(format!("versions: {x}").into()))?;
    match upload_next(fold, &c, &tagged) {
        UploadStep::Done => Ok(None),
        UploadStep::Upload(s) => {
            e.upload(s, c.release, &c.scripts[&s])
                .await
                .map_err(|x| Ran::Error(format!("upload {s:?}: {x}").into()))?;
            Ok(Some(Ran::Uploaded(s)))
        }
        UploadStep::Commit(u) => {
            let bytes = signed(e, fold, ReleaseEntry::ReleaseUploaded(u), Signer::Executor)?;
            let mut log = Vec::new();
            submitted(e, &bytes, "ReleaseUploaded", &mut log).await;
            Ok(log.pop())
        }
    }
}
