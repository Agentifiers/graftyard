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
//!   script's version, reusing one an earlier run uploaded under the same tag (`version_tag`, `<seq>-<release id>`), then commits
//!   `ReleaseUploaded`.
//! - **Forward steps.** `ROLL_STEPS` per kind (`group::roll_steps`, and a site day's drain, 1%, 10%, 50%, 100%,
//!   undrain, first day). Each deploy step waits `STEP_DWELL` (a 1%-for-a-day roll's first step `FIRST_DAY`) and a
//!   passing gate, then commits `IntentStep`; a 100% step waits none, except a front or MCP roll's, which holds
//!   `STEP_DWELL` and is judged. A site's 100% commits `SiteRelease` before its `IntentStep`. The last
//!   step's `IntentStep` is followed by `IntentDone`.
//! - **A breach** submits the intent's first unused pre-signed undo. The group refuses one that cannot read an enabled
//!   format, and pages; `graft-ops` then pages and moves nothing.
//! - **Undo and `Abort`.** A `Rollback` runs drain, N at 100%, then undrain; a pointer undo deploys the pin at 100%; an
//!   `Abort` moves the intent's target to `target` the same way. Each step commits an `IntentStep` under the intent, and
//!   `IntentDone` ends it.
//! - **`release roll`.** `next_roll` picks the first intent of the roll order that R has not finished, skipping a script
//!   or images R leaves unchanged, with its pre-signed undos.
//! - **`Rotate`** (M5-WP6). `group::rotate_steps` lists a secret's steps; the host runs each (`Exec::rotate`) and then
//!   commits its `IntentStep`. The switch step's `IntentStep` moves the fold's active slot, which every reader of the
//!   secret takes from its view. A `Rotate` of a secret this `graft-ops` does not switch, or to the active slot, pages
//!   by name and moves nothing until the customer's `Abort`.

use std::collections::BTreeMap;

use graftyard_types::{ReleaseId, SiteId, Timestamp};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::gate::{STUCK_DAY, Verdict};
use crate::group::{Current, ReleaseFold, RotateStep, Upload, roll_steps, rotate_steps, undo_entry, verify_raw};
use crate::types::{
    DayTarget, Format, FormatSet, ImageRef, Intent, IntentId, IntentState, ReleaseEntry, ReleaseUploaded, Script,
    SecretName, Signature, VersionId,
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
    /// A `MintWrapKey` `Rotate`'s wait between publish and switch: `ROTATE_PUBLISHED_FOR`, lowered only by a
    /// test-faults `graft-ops` (`OPS_ROTATE_PUBLISHED_MS`).
    pub rotate_published: u64,
    /// An `SshHostKeys` `Rotate`'s wait between its probe and switch: `SSH_ANNOUNCE_FOR`, lowered only by a test-faults
    /// `graft-ops` (`OPS_SSH_ANNOUNCE_MS`).
    pub ssh_announce: u64,
    /// An `ActionsOidcKey` `Rotate`'s wait between its probe and switch: `OIDC_PUBLISHED_FOR`, lowered only by a
    /// test-faults `graft-ops` (`OPS_OIDC_PUBLISHED_MS`).
    pub oidc_published: u64,
    /// How long a registered lifeboat may send no `LifeboatDecoder` before `graft-ops` pages: `LIFEBOAT_SILENT`,
    /// lowered only by a test-faults `graft-ops` (`OPS_LIFEBOAT_SILENT_MS`), so BF-53 reads the alarm in hours.
    pub lifeboat_silent: u64,
}

impl Default for Dwell {
    fn default() -> Dwell {
        Dwell {
            step: STEP_DWELL,
            first_day: FIRST_DAY,
            rotate_published: ROTATE_PUBLISHED_FOR,
            ssh_announce: SSH_ANNOUNCE_FOR,
            oidc_published: OIDC_PUBLISHED_FOR,
            lifeboat_silent: LIFEBOAT_SILENT,
        }
    }
}

/// A lifeboat sends a `LifeboatDecoder` when its decoder changes and at least daily; one silent this long pages
/// (`design/DESIGN.md`, "Install operations", Lifeboat: "an alarm fires when a registered lifeboat has sent no
/// `LifeboatDecoder` for 7 days").
pub const LIFEBOAT_SILENT: u64 = 7 * 24 * 3_600_000;

/// The registered lifeboats heard from neither by their `LifeboatAdd` nor by a `LifeboatDecoder` for more than
/// `silent` ms at `now`, by host. A removed lifeboat is not registered, so it never pages.
pub fn silent_lifeboats(fold: &ReleaseFold, now: Timestamp, silent: u64) -> Vec<Box<str>> {
    fold.lifeboats
        .keys()
        .filter(|h| fold.lifeboat_seen.get(*h).is_none_or(|t| now.0.saturating_sub(t.0) > silent))
        .cloned()
        .collect()
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

/// The account a `graft-ops` runs in. The install's runs every intent but the agents account's, and drains, replaces,
/// and pages for lifeboats. The agents account's (`design/AGENT-LAYER.md`, Shape, \"Controller\") reads the install's
/// view over HTTPS, runs only `ControllerRoll` (`group::agents_intent`), deploys only `graft-agents-front` and
/// `graft-controller`, and signs with `Keys.agents_executor`; it holds no drain key and reads no site health.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Account {
    #[default]
    Install,
    Agents,
}

/// Whether `account`'s `graft-ops` runs `i`. Every other intent it leaves to the other account and sits idle, because
/// the group holds one intent in flight.
pub fn runs(account: Account, i: &Intent) -> bool {
    match account {
        Account::Install => !matches!(i, Intent::ControllerRoll { .. }),
        Account::Agents => crate::group::agents_intent(i),
    }
}

/// What `graft-ops` read this run.
#[derive(Clone, Debug)]
pub struct World {
    pub account: Account,
    pub now: Timestamp,
    /// Each script's deployment, from the deployments API. A script missing here is one the API did not answer for.
    pub deployments: BTreeMap<Script, Deployment>,
    pub gate: Gate,
    /// Each voter site's verdict from both other sites' `SiteHealth` (`crate::gate::verdict`); a site missing here is
    /// one with no verdict, which drains and undrains nothing.
    pub health: BTreeMap<u8, Verdict>,
    /// The current `ReplaceMember` or `RestorePlacement`'s Workflow instance, by the intent's id (`Exec::move_run`).
    pub moves: MoveRun,
    /// `SecretsWrapKey`'s slot from `SECRETS_WRAP_KEY_CURRENT` (`Exec::wrap_var`), its active slot until a `Rotate` of
    /// it switches (`active_slot`).
    pub wrap_var: u8,
}

/// `secret`'s active slot as `graft-ops` uses it: the fold's, except that `SecretsWrapKey` with no committed switch is
/// `SECRETS_WRAP_KEY_CURRENT`'s, the slot `/wrap` wraps under (`workers/ops`, `wrap_keys`). Reading 0 there, a first
/// `Rotate` to 0 on an install whose variable names 1 was refused as "the active slot" (BF-46, graft-m4a).
pub fn active_slot(fold: &ReleaseFold, secret: SecretName, w: &World) -> u8 {
    match (secret, fold.slots.get(&secret)) {
        (SecretName::SecretsWrapKey, None) => w.wrap_var,
        _ => fold.slot(secret),
    }
}

/// A `ReplaceMember` or `RestorePlacement`'s coordinator Workflow instance, named by the intent's id, as the host
/// reads it: one instance per intent, which starts one instance per group it moves ("Install operations", Site loss).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MoveRun {
    #[default]
    Absent,
    Running,
    /// Every group it enumerated holds `to` as a caught-up voter and no member on `from`.
    Done,
    Failed(Box<str>),
}

/// What a coordinator moves: every group with a voter on `from` gets one on `to` instead, under a new member
/// generation. `dead` for a lost site, whose members are never asked (a `*2w` group's lost Full member goes out by
/// `Degraded` first); `home` for a restore, whose groups' leaders move back to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MoveSpec {
    pub from: SiteId,
    pub to: SiteId,
    pub dead: bool,
    pub home: Option<SiteId>,
}

/// How long a `ReplaceMember` graft-ops signs stays valid: one hour, so a run that met 409 signs a new one [guess].
pub const REPLACE_TTL: u64 = 3_600_000;
/// How often a run looks at a running move again [guess]: the cron's own period.
pub const MOVE_POLL: u64 = 60_000;

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
    /// Run `step` of a `Rotate` to `slot` on the host; once it succeeds, commit the current step's `IntentStep`.
    Rotate { secret: SecretName, slot: u8, step: RotateStep },
    /// Start the coordinator Workflow instance named by `intent` (idempotent: an instance of that name that exists is
    /// left as it is).
    StartMove { intent: IntentId, spec: MoveSpec },
}

/// How long a new installation-token signing key's `VerifyingKey` row is published before `graft-ops` signs with it:
/// one installation-token lifetime, `graftyard_identity::stateless::INSTALLATION_TOKEN_TTL`, so every edge has read
/// the row (`rotation_step`). `workers/ops`'s tests hold the two equal.
pub const ROTATE_PUBLISHED_FOR: u64 = 3_600_000;
/// How long every `graft-sshd` announces a new `SshHostKeys` slot before it serves key exchange, so `UpdateHostKeys`
/// clients have learned it (\"Install operations\", Rotation runbook) [guess].
pub const SSH_ANNOUNCE_FOR: u64 = 30 * 24 * 3_600_000;
/// How long every site publishes a new `ActionsOidcKey` slot in its JWKS before tokens are signed under it, so a
/// relying party that cached the JWKS has fetched it again (\"Install operations\", Rotation runbook) [guess:
/// `decisions/OPEN-QUESTIONS.md`, R64].
pub const OIDC_PUBLISHED_FOR: u64 = 24 * 3_600_000;

/// A `Rotate`'s next action (`group::rotate_steps`). An `Abort` of one moves nothing, so it ends at once and leaves the
/// slot the last switch chose.
fn rotate_next(fold: &ReleaseFold, c: &Current, secret: SecretName, slot: u8, w: &World, dwell: &Dwell) -> Action {
    let id = c.intent.id;
    let done = Action::Commit { body: ReleaseEntry::IntentDone { intent: id }, signer: Signer::Executor };
    if !matches!(c.state, IntentState::Running { .. }) {
        return done;
    }
    // Refused by name, never skipped: it stays current, paging, until the customer signs an `Abort` (R45).
    let steps = match rotate_steps(secret) {
        Ok(s) => s,
        Err(why) => return Action::Page(format!("intent {}: graft-ops refuses Rotate {secret:?}: {why}", id.0).into()),
    };
    if c.next == 0 && active_slot(fold, secret, w) == slot {
        return Action::Page(
            format!("intent {}: graft-ops refuses Rotate {secret:?} to slot {slot}, the active slot", id.0).into(),
        );
    }
    let step = ReleaseEntry::IntentStep { intent: id, step: c.next };
    match steps.get(usize::from(c.next)) {
        None => done,
        Some(RotateStep::Switch) => Action::Commit { body: step, signer: Signer::Executor },
        Some(RotateStep::Wait) => {
            let wait = match secret {
                SecretName::SshHostKeys => dwell.ssh_announce,
                SecretName::ActionsOidcKey => dwell.oidc_published,
                _ => dwell.rotate_published,
            };
            let until = Timestamp(c.since.0.saturating_add(wait));
            if w.now < until {
                Action::Wait { until: Some(until), why: format!("{secret:?}'s new slot is announced").into() }
            } else {
                Action::Commit { body: step, signer: Signer::Executor }
            }
        }
        Some(s) => Action::Rotate { secret, slot, step: *s },
    }
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
    /// A `ReplaceMember`'s or `RestorePlacement`'s one step: its coordinator Workflow, started and run to `Done`.
    /// `None` for a restore of a site no `ReplaceMember` moved, which moves nothing.
    Move(Option<MoveSpec>),
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

/// The version a new upload copies its bindings and runtime settings from: the one serving the most traffic. A version
/// uploaded since and never deployed, or N+1 at 1%, is not it (R57).
pub fn upload_source(split: &Split) -> Option<VersionId> {
    split.iter().max_by_key(|(_, p)| *p).map(|(v, _)| *v)
}

/// The binding types a version read returns without their value, so an upload keeps them (`keep_bindings`) instead of
/// sending them. Cloudflare keeps them from the newest uploaded version, which may not be the deployed one: only its
/// beta Workers API version endpoints take an `inherit` binding's `version_id` (R57).
pub const KEPT_BINDINGS: &[&str] = &["secret_text"];

/// A version upload's metadata for `main` tagged `tag`, configured as `source` is: `source` is the `result` of the
/// versions API's read of `upload_source`'s version. Every binding but a `KEPT_BINDINGS` one is sent as that version
/// holds it, so a version uploaded since and never deployed sets none of the upload's vars (`keep_bindings` takes them
/// from the newest upload: DECISIONS "`deployed-bindings`"). The compatibility date, flags, limits and container map
/// come from `source` too: an upload keeps none of them from any version.
pub fn upload_metadata(source: &Value, main: &str, tag: &str) -> Value {
    let bindings: Vec<Value> = source["resources"]["bindings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| !KEPT_BINDINGS.contains(&b["type"].as_str().unwrap_or("")))
        .cloned()
        .collect();
    let mut meta = json!({
        "main_module": main,
        "bindings": bindings,
        "keep_bindings": KEPT_BINDINGS,
        "keep_assets": true,
        "annotations": { "workers/tag": tag, "workers/message": "graft-ops upload" },
    });
    let rt = &source["resources"]["script_runtime"];
    for k in ["compatibility_date", "compatibility_flags", "limits", "containers"] {
        if !rt[k].is_null() {
            meta[k] = rt[k].clone();
        }
    }
    meta
}

/// The `workers/tag` of every version `graft-ops` uploads for `r`: `<seq>-<release id>`, seq being r's place in the
/// group's uploads (`Upload::seq`), or the next place while r has none. A member reads its `ReleaseTag` from it
/// (`parse_version_tag`), so a release's order is the group's, never a version's upload time: a version made later for
/// an older release keeps that release's seq.
pub fn version_tag(fold: &ReleaseFold, r: &ReleaseId) -> String {
    let seq = fold.uploads.get(&r.0).map_or(fold.uploads.len() as u32 + 1, |u| u.seq);
    format!("{seq}-{}", hex(r))
}

/// The seq and release a `version_tag` names, or `None` for any other tag: a wrangler deploy's, or one from before
/// tags carried a seq, which is a bare release id.
pub fn parse_version_tag(tag: &str) -> Option<(u32, ReleaseId)> {
    let (seq, id) = tag.split_once('-')?;
    if seq.is_empty() || seq.len() > 10 || !seq.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seq: u32 = seq.parse().ok().filter(|s| *s > 0)?;
    Some((seq, ReleaseId(graftyard_types::Digest::parse_hex(id)?.0)))
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
        IntentState::Running { .. } | IntentState::Halted { .. } if let Intent::ReplaceMember { lost, spare } = i => {
            Ok(vec![Step::Move(Some(MoveSpec { from: *lost, to: *spare, dead: true, home: None }))])
        }
        IntentState::Running { .. } | IntentState::Halted { .. } if let Intent::RestorePlacement { site } = i => {
            let spare = fold.replaced.get(&site.0).map(|s| SiteId(*s));
            Ok(vec![Step::Move(spare.map(|from| MoveSpec { from, to: *site, dead: false, home: Some(*site) }))])
        }
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
                // A front or MCP roll's 100% step holds `STEP_DWELL` and is judged like the others: its 1% step
                // rarely reaches `STEP_MIN_ANSWERS`, so this is where a build failing under about 1% of calls is
                // first judged, against N's counts from the intent's start (`gate::judge_roll`; R47's 1% gap).
                (Intent::FrontRoll { .. } | Intent::McpRoll { .. }, Some(ps)) => {
                    ps.iter().map(|p| pct(*p, if *p < 100 { dwell.first_day } else { dwell.step })).collect()
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
    if w.account == Account::Install {
        if let Some(a) = auto_drain(fold, w) {
            return a;
        }
        if let Some(a) = auto_replace(fold, w) {
            return a;
        }
    }
    let Some(c) = fold.current.as_deref() else { return Action::Idle };
    if !runs(w.account, &c.intent.intent) {
        return Action::Idle;
    }
    if let IntentState::Halted { .. } = c.state {
        return Action::Idle;
    }
    if let (Account::Agents, Intent::Rotate { secret, .. }) = (w.account, &c.intent.intent) {
        return Action::Page(
            format!("intent {}: the agents graft-ops does not rotate {secret:?} yet", c.intent.id.0).into(),
        );
    }
    if let Intent::Rotate { secret, slot } = c.intent.intent {
        return rotate_next(fold, c, secret, slot, w, &dwell);
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
    // The wait on another drained site is bounded like the day's own (`STUCK_DAY`): past it, from the later of that
    // drain and the day's last entry, the day pages instead of waiting in silence (OPEN-QUESTIONS R53).
    let waits = |d: u8, why: String| {
        let from = fold.drained.get(&d).map_or(c.since, |x| Timestamp(x.since.0.max(c.since.0)));
        let until = Timestamp(from.0.saturating_add(STUCK_DAY));
        if w.now >= until {
            return Action::Page(
                format!("intent {}: {why} for STUCK_DAY; it needs site {d}'s Normal or an Abort", id.0).into(),
            );
        }
        Action::Wait { until: None, why: why.into() }
    };
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
            waits(d, format!("site {d} is drained; site {} waits to drain", s.0))
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
        Step::Move(None) => done,
        Step::Move(Some(spec)) => match &w.moves {
            MoveRun::Absent => Action::StartMove { intent: id, spec: *spec },
            MoveRun::Running => Action::Wait {
                until: Some(Timestamp(w.now.0.saturating_add(MOVE_POLL))),
                why: format!("intent {}: groups move from site {} to {}", id.0, spec.from.0, spec.to.0).into(),
            },
            MoveRun::Done => done,
            MoveRun::Failed(why) => Action::Page(format!("intent {}: the move Workflow failed: {why}", id.0).into()),
        },
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
                return waits(d, format!("site {d} is drained; the site day waits"));
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
                // Too few entries: dwell on, up to twice the step's dwell, then page. The 1% step's floor is pooled
                // with the 10% step's instead (R38): it commits after its dwell, and 10% counts from its start
                // (`gate::window_start`). A breach at 1% still runs the undo above.
                if let Gate::Starved(n) = w.gate
                    && *dwell > 0
                    && *pct != 1
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
    // A lost or replaced site undrains only once it has recovered (`RECOVER_AFTER` under `UNDRAIN_BURN`), not on the
    // 15-minute calm of an automatic drain: its drain is what holds `RestorePlacement` off ("Install operations", Site
    // loss).
    let ends = |s: &u8| {
        let v = verdict(s);
        let loss = fold.lost.contains(s) || fold.replaced.contains_key(s);
        !v.burning && if loss { v.recovered } else { v.calm }
    };
    if let Some((s, _)) = fold.drained.iter().find(|(s, d)| d.by_drain_key && day != Some(SiteId(**s)) && ends(s)) {
        return Some(mode(*s, crate::types::Mode::Normal));
    }
    // A customer-signed `SiteLost` site is drained before it is replaced, so its recovery is judged by the drain and
    // `RestorePlacement` waits for its undrain ("Install operations", Site loss). Once replaced, its undrain is final.
    if fold.drained.is_empty()
        && let Some(s) = fold.lost.iter().find(|s| !fold.replaced.contains_key(*s))
    {
        return Some(mode(*s, crate::types::Mode::Drain));
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

/// The executor key's `ReplaceMember` (journey 5; "Install operations", Site loss), before any intent's step, since it
/// commits over a current intent and halts it: for a site with a customer-signed `SiteLost`, or one drained for
/// `ZERO_ACKS` that both other sites read as silent over that window, onto the spare on its continent
/// (`SiteTable::spare_for`), once no `ReplaceMember` runs and the site has none yet. A spare whose last `SiteRelease`
/// cannot read every enabled format is refused by a page, as is a lost site with no spare: graft-ops signs nothing.
/// The id derives from the drain's time and the site, so a run that lost its answer signs the same id again, which the
/// group refuses as used once it committed.
fn auto_replace(fold: &ReleaseFold, w: &World) -> Option<Action> {
    let g = fold.genesis.as_ref()?;
    let replacing = |c: &Option<Box<Current>>| {
        c.as_deref().is_some_and(|c| matches!(c.intent.intent, Intent::ReplaceMember { .. }))
    };
    if replacing(&fold.current) || fold.under.is_some() {
        return None;
    }
    let due = |s: &u8| {
        let drained_long = fold.drained.get(s).is_some_and(|d| {
            d.since.0.saturating_add(crate::ZERO_ACKS) <= w.now.0 && w.health.get(s).is_some_and(|v| v.silent)
        });
        !fold.replaced.contains_key(s) && (fold.lost.contains(s) || drained_long)
    };
    let lost = g.sites.iter().filter(|s| !s.spare).map(|s| s.id).find(|s| due(&s.0))?;
    let table = graftyard_types::SiteTable(g.sites.to_vec());
    let Some(spare) = table.spare_for(lost) else {
        return Some(Action::Page(format!("site {} is lost and its continent has no spare", lost.0).into()));
    };
    let reads = fold.site_release.get(&spare.0).and_then(|r| fold.uploads.get(&r.0)).map(|u| &u.reads);
    if let Some(f) = fold.enabled.iter().find(|f| !reads.is_some_and(|r| r.contains(f))) {
        return Some(Action::Page(
            format!(
                "site {} is lost, but spare {}'s release cannot read format {f}: no ReplaceMember",
                lost.0, spare.0
            )
            .into(),
        ));
    }
    let at = fold.drained.get(&lost.0).map_or(w.now, |d| d.since);
    let id = IntentId(graftyard_types::Ulid::new(at, [b'R', b'M', lost.0, spare.0, 0, 0, 0, 0, 0, 0]));
    let intent = crate::types::SignedIntent {
        id,
        issued_at: w.now,
        not_after: Timestamp(w.now.0.saturating_add(REPLACE_TTL)),
        intent: Intent::ReplaceMember { lost, spare },
        undo: Vec::new(),
    };
    Some(Action::Commit { body: ReleaseEntry::Intent(Box::new(intent)), signer: Signer::Executor })
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
/// `formats.txt` and every other file with its SHA-256, and every file it lists is there. Returns the scripts this
/// account uploads: each genesis site's and `scripts` for the install, `scripts` alone for the agents account, whose
/// `ReleaseUploaded` names `AgentsFront` and `Controller` only (`scripts` names the non-site scripts this `graft-ops`
/// deploys).
pub fn check_bundle(
    fold: &ReleaseFold,
    record: &[u8],
    sig: Signature,
    files: &[(String, Vec<u8>)],
    scripts: &[Script],
    account: Account,
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
    let mut want: Vec<Script> = match account {
        Account::Install => genesis.sites.iter().map(|s| Script::Site(s.id)).collect(),
        Account::Agents => Vec::new(),
    };
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
    /// Upload this script's modules as a version tagged `version_tag`, without deploying it.
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
/// `McpRoll` (`release roll` runs the vault step here, outside the group: `vault_due`), then a `ContainerDay` for each
/// site whose images changed. It skips an intent whose script, or whose images, R leaves as the release running them
/// has them. Each carries its undo: a `Rollback` to N for a day target, a
/// pointer to N for a pinned script, none for `FrontRoll` and for a script with no N yet.
pub fn next_roll(fold: &ReleaseFold, r: ReleaseId) -> Roll {
    let Some(up) = fold.uploads.get(&r.0) else {
        return Roll::Wait(format!("release {} has no ReleaseUploaded", hex(&r)).into());
    };
    if let Some(c) = &fold.current {
        return Roll::Wait(format!("intent {} ({}) is current", c.intent.id.0, c.intent.intent.kind()).into());
    }
    if let Some((intent, undo)) = next_code(fold, up, r) {
        return Roll::Next { intent, undo };
    }
    let g = fold.genesis.as_ref().expect("installed");
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

/// Whether `release roll R`'s vault step is due ("Install operations", Release roll, step 4): R lists a `vault/`
/// module, and every code intent of the order through `McpRoll` is done or skipped for R. A `ContainerDay` does not
/// hold it back. It is no group intent, because graft-ops cannot reach the vault account (`decisions/DECISIONS.md`,
/// "`vault-roll`"): `release roll` runs `vault upgrade` on the operator's machine under the vault account's token.
pub fn vault_due(fold: &ReleaseFold, r: ReleaseId) -> bool {
    fold.uploads
        .get(&r.0)
        .is_some_and(|up| up.files.iter().any(|(n, _)| n.starts_with(VAULT_FOLDER)) && next_code(fold, up, r).is_none())
}

/// The first code intent of the roll order that R has not finished, with its undos: each member site's `SiteDay`,
/// each spare's, `SiteDay { Workflows }`, then `EdgeRoll`, `OpsRoll`, `FrontRoll`, `McpRoll`, and `ControllerRoll`.
fn next_code(fold: &ReleaseFold, up: &Upload, r: ReleaseId) -> Option<(Intent, Vec<ReleaseEntry>)> {
    let g = fold.genesis.as_ref().expect("installed");
    let sites = g.sites.iter().filter(|s| !s.spare).chain(g.sites.iter().filter(|s| s.spare));
    let mut order: Vec<Intent> = sites.map(|s| Intent::SiteDay { target: DayTarget::Site(s.id), release: r }).collect();
    order.push(Intent::SiteDay { target: DayTarget::Workflows, release: r });
    order.extend([
        Intent::EdgeRoll { release: r },
        Intent::OpsRoll { release: r },
        Intent::FrontRoll { release: r },
        Intent::McpRoll { release: r },
        // The agents account's release: its own `ReleaseUploaded` names only `AgentsFront` and `Controller`, so every
        // intent above skips it (`design/AGENT-LAYER.md`, Shape, "Controller").
        Intent::ControllerRoll { release: r },
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
        return Some((i, undo));
    }
    None
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
    /// The account this `graft-ops` runs in: the install's unless the host says otherwise.
    fn account(&self) -> Account {
        Account::Install
    }
    fn now(&self) -> Timestamp;
    /// `SECRETS_WRAP_KEY_CURRENT`, the slot `SecretsWrapKey` wraps under before its first `Rotate` (`World::wrap_var`).
    fn wrap_var(&self) -> u8 {
        0
    }
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
    /// The versions an earlier run uploaded with `tag` (`version_tag`).
    async fn tagged(&mut self, tag: &str, scripts: &[Script]) -> Result<BTreeMap<Script, VersionId>, String>;
    /// Uploads one script's modules as a version tagged `tag`, without deploying it.
    async fn upload(&mut self, s: Script, tag: &str, modules: &[(String, Vec<u8>)]) -> Result<(), String>;
    /// Runs one host step of a `Rotate` to `slot` (`decisions/DECISIONS.md`, "`rotate-probe`"). Idempotent: a step
    /// rerun after a kill finds its work done.
    async fn rotate(
        &mut self,
        fold: &ReleaseFold,
        secret: SecretName,
        slot: u8,
        step: RotateStep,
    ) -> Result<(), String>;
    /// The coordinator Workflow instance named by `intent`, as the Workflow binding reports it.
    async fn move_run(&mut self, intent: IntentId) -> MoveRun;
    /// Creates the coordinator Workflow instance named by `intent`; one that exists already is left as it is.
    async fn start_move(&mut self, intent: IntentId, spec: MoveSpec) -> Result<(), String>;
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
    /// A move's coordinator Workflow was started for this intent.
    Moving(IntentId),
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
    for pass in 0..RUN_MAX {
        let fold = match e.view().await {
            Ok(f) => f,
            Err(x) => {
                log.push(Ran::Error(format!("view: {x}").into()));
                break;
            }
        };
        // Once a run, beside whatever else it does: a silent lifeboat moves nothing, so it only pages. The install's
        // `graft-ops` alone pages for it.
        if pass == 0 && e.account() == Account::Install {
            for h in silent_lifeboats(&fold, e.now(), dwell.lifeboat_silent) {
                let why = format!("lifeboat {h} has sent no LifeboatDecoder for {} ms", dwell.lifeboat_silent);
                e.page(&why);
                log.push(Ran::Paged(why.into()));
            }
        }
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
        let account = e.account();
        let gate = if fold.current.is_some() { e.gate(&fold).await } else { Gate::Pending };
        let health = e.health(&fold).await;
        let moves = match fold.current.as_deref() {
            Some(c) if matches!(c.intent.intent, Intent::ReplaceMember { .. } | Intent::RestorePlacement { .. }) => {
                e.move_run(c.intent.id).await
            }
            _ => MoveRun::Absent,
        };
        let w = World { account, now: e.now(), deployments, gate, health, moves, wrap_var: e.wrap_var() };
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
            Action::StartMove { intent, spec } => {
                match e.start_move(intent, spec).await {
                    Ok(()) => log.push(Ran::Moving(intent)),
                    Err(x) => {
                        log.push(Ran::Error(format!("start move {}: {x}", intent.0).into()));
                        break;
                    }
                }
                continue;
            }
            // A step that fails leaves its `IntentStep` uncommitted and pages, because the new slot's value is the
            // customer's to fix; the next run tries the step again.
            Action::Rotate { secret, slot, step } => {
                if let Err(x) = e.rotate(&fold, secret, slot, step).await {
                    let why = format!("Rotate {secret:?} to slot {slot}, {step:?}: {x}");
                    e.page(&why);
                    log.push(Ran::Paged(why.into()));
                    break;
                }
                let c = fold.current.as_deref().expect("a Rotate is current");
                let body = ReleaseEntry::IntentStep { intent: c.intent.id, step: c.next };
                match signed(e, &fold, body, Signer::Executor) {
                    Ok(b) => (b, "IntentStep"),
                    Err(r) => {
                        log.push(r);
                        break;
                    }
                }
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
    let c = check_bundle(fold, &b.record, b.sig, &b.files, &e.scripts(), e.account())
        .map_err(|r| Ran::Error(format!("release bundle refused: {r}").into()))?;
    let scripts: Vec<Script> = c.scripts.keys().copied().collect();
    // Every script's version carries the same tag: a version uploaded under an earlier seq (another release's upload
    // committed since) is not taken, so the scripts are uploaded again under the seq the group will record.
    let tag = version_tag(fold, &c.release);
    let tagged = e.tagged(&tag, &scripts).await.map_err(|x| Ran::Error(format!("versions: {x}").into()))?;
    match upload_next(fold, &c, &tagged) {
        UploadStep::Done => Ok(None),
        UploadStep::Upload(s) => {
            e.upload(s, &tag, &c.scripts[&s]).await.map_err(|x| Ran::Error(format!("upload {s:?}: {x}").into()))?;
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
