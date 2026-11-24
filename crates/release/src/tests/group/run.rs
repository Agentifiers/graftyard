//! `ops::run`, the loop the `graft-ops` Worker runs on each cron tick, against a fake world over the real group: the
//! release bucket, the versions and deployments API, and member commits that answer 201, 409, or 422 as the group
//! decides. These test what the host adds to `ops::next`: uploads before intents, a fresh view before each action, a
//! 409 retried on the next view, a refusal that ends the run, and a page for a refusal the group pages on.

use std::collections::BTreeMap;

use futures_executor::block_on;

use super::*;
use crate::ops::{self, Deployment, Dwell, Exec, Gate, Ran, RawBundle, Roll, Signer, Split, Submit};

/// The world one `graft-ops` sees. `fold` is read through `view`, which a test may hold back a commit from, as a
/// member one round behind would.
struct Fake {
    g: G,
    deps: BTreeMap<Script, Deployment>,
    gate: Gate,
    bucket: Vec<RawBundle>,
    /// Versions uploaded by (release, script), as the versions API lists them by tag.
    tags: BTreeMap<([u8; 32], Script), VersionId>,
    uploads: Vec<Script>,
    deploys: Vec<(Script, Split)>,
    pages: Vec<String>,
    /// The next `view` answers the fold as it was before the last commit, once.
    lag: Option<ReleaseFold>,
    /// The next `submit` is lost after it committed: the run sees no answer.
    lose_next: bool,
    /// The Cloudflare API refuses every call: no deployment read, no deploy.
    api_down: bool,
    /// Calls this run may still make before `graft-ops` is killed: a killed isolate's deploys and commits never
    /// happen, and every later call of that run fails. `None` never kills.
    kill_after: Option<usize>,
    next_v: u8,
    /// Each `Rotate` host step run, in order; a step named in `rotate_fails` fails instead.
    rotated: Vec<(SecretName, u8, crate::RotateStep)>,
    rotate_fails: Option<crate::RotateStep>,
    /// Each coordinator Workflow by its intent, and every start, in order (a start of one that exists changes nothing).
    moves: BTreeMap<IntentId, ops::MoveRun>,
    started: Vec<(IntentId, ops::MoveSpec)>,
    /// What `health` answers.
    health: BTreeMap<u8, crate::gate::Verdict>,
}

impl Fake {
    fn new(g: G) -> Fake {
        let mut deps = BTreeMap::new();
        for s in [Script::Site(SiteId(0)), Script::Site(SiteId(1)), Script::Site(SiteId(2)), Script::Site(SiteId(3))]
            .into_iter()
            .chain([Script::Edge, Script::Ops])
        {
            deps.insert(s, Deployment { split: vec![(VersionId([0xee; 16]), 100)], created: T0 });
        }
        Fake {
            g,
            deps,
            gate: Gate::Pass,
            bucket: Vec::new(),
            tags: BTreeMap::new(),
            uploads: Vec::new(),
            deploys: Vec::new(),
            pages: Vec::new(),
            lag: None,
            lose_next: false,
            api_down: false,
            kill_after: None,
            next_v: 1,
            rotated: Vec::new(),
            rotate_fails: None,
            moves: BTreeMap::new(),
            started: Vec::new(),
            health: BTreeMap::new(),
        }
    }

    /// Runs `ops::run` with a one-minute clock step between runs, as a cron would, until nothing is current and no
    /// upload is pending, at most `n` runs.
    fn cron(&mut self, n: usize) -> Vec<Ran> {
        let mut all = Vec::new();
        for _ in 0..n {
            let log = block_on(ops::run(self, Dwell { step: 0, first_day: 0, ..Dwell::default() }));
            let idle = matches!(log.last(), Some(Ran::Idle));
            all.extend(log);
            self.g.at = Timestamp(self.g.at.0 + 60_000);
            if idle {
                break;
            }
        }
        all
    }
}

impl Fake {
    /// Whether the run was killed before this call.
    fn killed(&mut self) -> bool {
        match &mut self.kill_after {
            Some(0) => true,
            Some(n) => {
                *n -= 1;
                false
            }
            None => false,
        }
    }
}

impl Exec for Fake {
    fn now(&self) -> Timestamp {
        self.g.at
    }

    fn scripts(&self) -> Vec<Script> {
        vec![Script::Edge, Script::Ops]
    }

    async fn view(&mut self) -> Result<ReleaseFold, String> {
        if self.killed() {
            return Err("killed".into());
        }
        Ok(self.lag.take().unwrap_or_else(|| self.g.f.clone()))
    }

    async fn deployment(&mut self, s: Script) -> Result<Deployment, String> {
        if self.api_down {
            return Err("HTTP 503".into());
        }
        self.deps.get(&s).cloned().ok_or_else(|| format!("no {s:?}"))
    }

    async fn gate(&mut self, _fold: &ReleaseFold) -> Gate {
        self.gate.clone()
    }

    async fn health(&mut self, _fold: &ReleaseFold) -> BTreeMap<u8, crate::gate::Verdict> {
        self.health.clone()
    }

    async fn deploy(&mut self, s: Script, split: &Split) -> Result<(), String> {
        if self.api_down || self.killed() {
            return Err("HTTP 503".into());
        }
        self.deps.insert(s, Deployment { split: split.clone(), created: self.g.at });
        self.deploys.push((s, split.clone()));
        Ok(())
    }

    async fn submit(&mut self, bytes: &[u8]) -> Submit {
        if self.killed() {
            return Submit::Failed("killed".into());
        }
        let before = self.g.f.clone();
        let r = match self.g.try_(bytes) {
            Ok(()) => Submit::Committed,
            Err(Refusal::StalePrev(_)) => Submit::Stale,
            Err(e) => Submit::Refused { variant: e.variant().into(), message: e.to_string().into() },
        };
        if r == Submit::Committed && std::mem::take(&mut self.lose_next) {
            self.lag = Some(before);
            return Submit::Failed("no outcome".into());
        }
        r
    }

    fn sign(&self, signer: Signer, e: &mut SignedEntry) -> Result<(), String> {
        e.sign(match signer {
            Signer::Executor => &EXECUTOR,
            Signer::Drain => &DRAIN,
        });
        Ok(())
    }

    fn page(&mut self, why: &str) {
        self.pages.push(why.to_string());
    }

    async fn bundle(&mut self, fold: &ReleaseFold) -> Result<Option<RawBundle>, String> {
        Ok(self
            .bucket
            .iter()
            .find(|b| !fold.uploads.contains_key(&<[u8; 32]>::from(Sha256::digest(&b.record))))
            .cloned())
    }

    async fn tagged(&mut self, release: ReleaseId, scripts: &[Script]) -> Result<BTreeMap<Script, VersionId>, String> {
        Ok(scripts.iter().filter_map(|s| Some((*s, *self.tags.get(&(release.0, *s))?))).collect())
    }

    async fn upload(&mut self, s: Script, release: ReleaseId, _modules: &[(String, Vec<u8>)]) -> Result<(), String> {
        self.next_v += 1;
        self.tags.insert((release.0, s), VersionId([self.next_v; 16]));
        self.uploads.push(s);
        Ok(())
    }

    async fn rotate(
        &mut self,
        _fold: &ReleaseFold,
        secret: SecretName,
        slot: u8,
        step: crate::RotateStep,
    ) -> Result<(), String> {
        if self.killed() {
            return Err("killed".into());
        }
        if self.rotate_fails == Some(step) {
            return Err("the new slot's value was refused".into());
        }
        self.rotated.push((secret, slot, step));
        Ok(())
    }

    async fn move_run(&mut self, intent: IntentId) -> ops::MoveRun {
        self.moves.get(&intent).cloned().unwrap_or_default()
    }

    async fn start_move(&mut self, intent: IntentId, spec: ops::MoveSpec) -> Result<(), String> {
        if self.killed() {
            return Err("killed".into());
        }
        self.moves.entry(intent).or_insert(ops::MoveRun::Running);
        self.started.push((intent, spec));
        Ok(())
    }
}

/// A signed bundle whose site, edge, and ops files carry `tag`, reading formats 32.1 to 32.`n`.
fn raw(tag: &str, n: u8, seed: &[u8; 32]) -> RawBundle {
    let files: Vec<(String, Vec<u8>)> = vec![
        ("formats.txt".into(), reads(n).file().into_bytes()),
        ("site/index.js".into(), format!("site {tag}").into_bytes()),
        ("edge/index.js".into(), format!("edge {tag}").into_bytes()),
        ("ops/index.js".into(), format!("ops {tag}").into_bytes()),
    ];
    let listed: Vec<String> = files
        .iter()
        .map(|(f, b)| format!(r#"{{"name":"{f}","sha256":"{}"}}"#, crate::types::hex(&Sha256::digest(b))))
        .collect();
    let record = format!(r#"{{"label":"{tag}","files":[{}]}}"#, listed.join(",")).into_bytes();
    RawBundle { sig: sign_record(seed, &record), record, files }
}

fn release_of(b: &RawBundle) -> ReleaseId {
    ReleaseId(Sha256::digest(&b.record).into())
}

/// `release roll R` once, as the operator's CLI does: the next intent, signed on the group's last digest.
fn roll(f: &mut Fake, r: ReleaseId, n: u8) -> bool {
    match ops::next_roll(&f.g.f, r) {
        Roll::Next { intent, undo } => {
            let e = ops::sign_intent(&f.g.f, intent, &undo, &VENDOR, id(n), f.g.at, 4 * HOUR);
            f.g.ok(e.encode());
            true
        }
        Roll::Done => false,
        Roll::Wait(why) => panic!("{why}"),
    }
}

/// A release in the bucket reaches every site from cron runs alone, with `release roll` signing each next intent: the
/// bundle uploads once per script, `ReleaseUploaded` commits, and every intent runs to `IntentDone`.
#[test]
fn cron_runs_upload_the_bundle_and_run_every_intent_to_done() {
    let mut f = Fake::new(G::new());
    let b = raw("r4", 4, &RELEASE);
    let r = release_of(&b);
    f.bucket.push(b);
    let log = f.cron(10);
    assert!(log.contains(&Ran::Committed("ReleaseUploaded")), "{log:?}");
    assert_eq!(
        f.uploads,
        [
            Script::Site(SiteId(0)),
            Script::Site(SiteId(1)),
            Script::Site(SiteId(2)),
            Script::Site(SiteId(3)),
            Script::Edge,
            Script::Ops
        ]
    );
    let mut n = 0;
    while roll(&mut f, r, 50 + n) {
        n += 1;
        let log = f.cron(40);
        assert!(f.g.f.current.is_none(), "intent {n} did not finish: {log:?}");
        assert!(f.pages.is_empty(), "{:?}", f.pages);
    }
    assert_eq!(n, 6, "four site days (the spare's included), then the edge and graft-ops");
    for s in 0..4 {
        assert_eq!(f.g.f.site_release.get(&s), Some(&r));
    }
    assert!(f.g.f.drained.is_empty());
    // Nothing more to do: a run reads the view and stops.
    assert_eq!(f.cron(1), vec![Ran::Idle]);
}

/// A bundle signed by a key that is not `Keys.release` never uploads, and the run still drives the current intent.
#[test]
fn a_bundle_that_does_not_verify_uploads_nothing() {
    let mut f = Fake::new(G::new());
    f.bucket.push(raw("forged", 4, &OTHER));
    let log = block_on(ops::run(&mut f, Dwell::default()));
    assert!(f.uploads.is_empty());
    assert!(f.g.f.uploads.is_empty());
    assert!(matches!(&log[0], Ran::Error(e) if e.contains("release bundle refused")), "{log:?}");
    assert_eq!(log.last(), Some(&Ran::Idle), "the run goes on to the intents after a refused bundle");
}

/// A commit whose answer was lost: the next action is decided on a view one round behind, so the run resends a commit
/// that already landed. The group answers 409, the run reads the view again and goes on; no step commits twice.
#[test]
fn a_lost_commit_answer_is_resent_meets_409_and_the_run_goes_on() {
    let mut f = Fake::new(G::new());
    let b = raw("r4", 4, &RELEASE);
    let r = release_of(&b);
    f.bucket.push(b);
    f.cron(10);
    assert!(roll(&mut f, r, 60));
    // Lose the answer of every third commit.
    let mut runs = Vec::new();
    for i in 0..60 {
        if f.g.f.current.is_none() {
            break;
        }
        f.lose_next = i % 3 == 0;
        runs.push(block_on(ops::run(&mut f, Dwell { step: 0, first_day: 0, ..Dwell::default() })));
        f.g.at = Timestamp(f.g.at.0 + 60_000);
    }
    assert!(f.g.f.current.is_none(), "{runs:?}");
    // The run that met 409 read the view again and went on within the same run.
    let run = runs.iter().find(|r| r.iter().any(|x| matches!(x, Ran::Stale(_)))).expect("a resent commit met 409");
    let after = run.iter().position(|x| matches!(x, Ran::Stale(_))).unwrap();
    assert!(matches!(run.get(after + 1), Some(Ran::Committed(_) | Ran::Deployed(..))), "{run:?}");
    let decoded: Vec<_> = f.g.log.iter().map(|b| SignedEntry::decode(b).unwrap().body).collect();
    let step_entries: Vec<u8> = decoded
        .iter()
        .filter_map(|b| if let ReleaseEntry::IntentStep { step, .. } = b { Some(*step) } else { None })
        .collect();
    assert_eq!(step_entries, (0..7).collect::<Vec<_>>(), "each step committed once, in order");
    assert_eq!(decoded.iter().filter(|b| matches!(b, ReleaseEntry::IntentDone { .. })).count(), 1);
}

/// BF-43's undo with the Cloudflare API down: an `EdgeRoll` at 1% breaches while no deployment can be read. The run
/// still submits the pre-signed `EdgePointer` to N, a signed entry that needs no API call, so the pin moves at once;
/// the return to N at 100% waits for the API and then finishes the intent.
#[test]
fn an_edge_breach_with_the_api_down_commits_the_pointer_undo_and_deploys_later() {
    let mut f = Fake::new(G::new());
    let b1 = raw("e1", 3, &RELEASE);
    let r1 = release_of(&b1);
    f.bucket.push(b1);
    f.cron(10);
    let mut n = 0;
    while roll(&mut f, r1, 90 + n) {
        n += 1;
        f.cron(40);
    }
    let b2 = raw("e2", 3, &RELEASE);
    let r2 = release_of(&b2);
    f.bucket.push(b2);
    f.cron(10);
    for i in 0..4 {
        assert!(roll(&mut f, r2, 100 + i));
        f.cron(40);
    }
    assert!(roll(&mut f, r2, 110));
    assert!(matches!(f.g.f.current.as_deref().unwrap().intent.intent, Intent::EdgeRoll { .. }));
    // The 1% step deploys, then the API goes down and the gate breaches.
    block_on(ops::run(&mut f, Dwell { step: HOUR, first_day: HOUR, ..Dwell::default() }));
    let e1 = f.tags[&(r1.0, Script::Edge)];
    let e2 = f.tags[&(r2.0, Script::Edge)];
    assert_eq!(f.deps[&Script::Edge].split, vec![(e2, 1), (e1, 99)]);
    f.api_down = true;
    f.gate = Gate::Breach("GATE_EXCEPTIONS".into());
    let deploys = f.deploys.len();
    let log = block_on(ops::run(&mut f, Dwell { step: HOUR, first_day: HOUR, ..Dwell::default() }));
    assert!(log.contains(&Ran::Committed("EdgePointer")), "{log:?}");
    assert_eq!(f.g.f.pointers.edge, Some(e1), "the pin moved to N with the API down");
    assert_eq!(f.deploys.len(), deploys, "nothing deployed while the API was down");
    assert!(f.g.f.current.is_some(), "the return to N waits for the API");
    f.api_down = false;
    f.gate = Gate::Pass;
    f.cron(10);
    assert!(f.g.f.current.is_none());
    assert_eq!(f.deps[&Script::Edge].split, vec![(e1, 100), (e2, 0)]);
    assert!(f.pages.is_empty(), "{:?}", f.pages);
}

/// BF-7 through the host: after `FormatsEnable`, a breach makes the run submit the pre-signed `Rollback` to N. The group
/// refuses it with `UnreadableFormat`, so the run writes a page, stops, and moves nothing.
#[test]
fn bf7_a_refused_rollback_pages_from_the_run_and_moves_nothing() {
    let mut f = Fake::new(G::new());
    let b3 = raw("r3", 3, &RELEASE);
    let b4 = raw("r4", 4, &RELEASE);
    let (r3, r4) = (release_of(&b3), release_of(&b4));
    f.bucket.push(b3);
    f.cron(10);
    let mut n = 0;
    while roll(&mut f, r3, 70 + n) {
        n += 1;
        f.cron(40);
    }
    f.bucket.push(b4);
    f.cron(10);
    // Sites 0 to 3 move to r4, which reads 32.4; the edge and graft-ops stay on r3.
    for i in 0..4 {
        assert!(roll(&mut f, r4, 80 + i));
        if i < 3 {
            f.cron(40);
        }
    }
    // The spare's day, up to its first day.
    for _ in 0..40 {
        if f.g.f.current.as_deref().is_some_and(|c| c.next == 6) {
            break;
        }
        f.gate = Gate::Pass;
        let log = block_on(ops::run(&mut f, Dwell { step: 0, first_day: HOUR, ..Dwell::default() }));
        if matches!(log.last(), Some(Ran::Wait { .. })) {
            break;
        }
    }
    let fe = Format { family: 32, version: 4 };
    let sk = ed25519_dalek::SigningKey::from_bytes(&RELEASE);
    let half = Signature(ed25519_dalek::Signer::sign(&sk, &formats_enable_message(fe)).to_bytes());
    let mut e = SignedEntry::decode(&f.g.entry(ReleaseEntry::FormatsEnable { format: fe }, &[&CUSTOMER])).unwrap();
    e.sigs.push((public_key(&RELEASE), half));
    f.g.ok(e.encode());
    let before = (f.g.f.clone(), f.deploys.len());
    f.gate = Gate::Breach("GATE_APPLY_LAG".into());
    let log = block_on(ops::run(&mut f, Dwell { step: 0, first_day: HOUR, ..Dwell::default() }));
    assert_eq!(log.last(), Some(&Ran::Refused { kind: "Rollback", variant: "UnreadableFormat".into() }), "{log:?}");
    assert_eq!(f.pages.len(), 1, "{:?}", f.pages);
    assert!(f.pages[0].contains("Rollback refused, UnreadableFormat"), "{}", f.pages[0]);
    assert_eq!(f.g.f, before.0, "nothing committed");
    assert_eq!(f.deploys.len(), before.1, "nothing deployed");
}

/// BF-39 in memory: `graft-ops` killed in every cron run after one to three of its calls (a view read, a deploy, or
/// a commit), across a release's four site days, its `EdgeRoll` and its `OpsRoll`, from three seeds, over 100 kills.
/// Each restart resumes after the last `IntentStep`; no step is skipped or doubled, each intent gets one `IntentDone`,
/// and the deployments end where an unkilled run leaves them.
#[test]
fn bf39_graft_ops_killed_at_every_step_resumes_after_the_last_intent_step() {
    let mut kills = 0;
    for seed in 0..3u64 {
        kills += killed_roll(seed);
    }
    assert!(kills >= 100, "{kills} kills");
}

/// One release rolled with `graft-ops` killed in every run; returns the kills.
fn killed_roll(seed: u64) -> usize {
    let mut f = Fake::new(G::new());
    let b1 = raw("k1", 3, &RELEASE);
    let r1 = release_of(&b1);
    f.bucket.push(b1);
    f.cron(10);
    let mut n = 0;
    while roll(&mut f, r1, 120 + n) {
        n += 1;
        f.cron(40);
    }
    let b2 = raw("k2", 3, &RELEASE);
    let r2 = release_of(&b2);
    f.bucket.push(b2);
    f.cron(10);
    let start = f.g.log.len();
    let mut rng: u64 = 0x05ee_db39 + seed;
    let mut kills = 0;
    let mut i = 0;
    while roll(&mut f, r2, 140 + i) {
        i += 1;
        for _ in 0..400 {
            if f.g.f.current.is_none() {
                break;
            }
            rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            f.kill_after = Some(1 + ((rng >> 33) % 3) as usize);
            kills += 1;
            block_on(ops::run(&mut f, Dwell { step: 0, first_day: 0, ..Dwell::default() }));
            f.kill_after = None;
            f.g.at = Timestamp(f.g.at.0 + 60_000);
        }
        assert!(f.g.f.current.is_none(), "seed {seed}: intent {i} did not finish");
    }
    assert_eq!(i, 6, "four site days, then the edge and graft-ops");
    // Per intent, in log order: its IntentSteps and IntentDone.
    let mut per: Vec<(Vec<u8>, usize)> = Vec::new();
    for b in &f.g.log[start..] {
        match SignedEntry::decode(b).unwrap().body {
            ReleaseEntry::Intent(_) => per.push((Vec::new(), 0)),
            ReleaseEntry::IntentStep { step, .. } => per.last_mut().unwrap().0.push(step),
            ReleaseEntry::IntentDone { .. } => per.last_mut().unwrap().1 += 1,
            _ => {}
        }
    }
    let want: Vec<(Vec<u8>, usize)> = [7u8, 7, 7, 7, 4, 2].iter().map(|k| ((0..*k).collect(), 1)).collect();
    assert_eq!(per, want, "seed {seed}: each step once and in order, one IntentDone per intent");
    for s in 0..4 {
        assert_eq!(f.g.f.site_release.get(&s), Some(&r2));
        let v = f.g.f.uploads[&r2.0].versions.iter().find(|(x, _)| *x == Script::Site(SiteId(s))).unwrap().1;
        assert_eq!(f.deps[&Script::Site(SiteId(s))].split, vec![(v, 100)]);
    }
    assert!(f.g.f.drained.is_empty());
    let edge = |r: &ReleaseId| f.g.f.uploads[&r.0].versions.iter().find(|(x, _)| *x == Script::Edge).unwrap().1;
    assert_eq!(f.deps[&Script::Edge].split, vec![(edge(&r2), 100), (edge(&r1), 0)]);
    assert!(f.pages.is_empty(), "{:?}", f.pages);
    kills
}

// ---- M5-WP6: `Rotate` (BF-46's in-memory half) ----

/// A `Rotate` signed by `signer` and committed, with nothing else current.
fn rotate(f: &mut Fake, n: u8, secret: SecretName, slot: u8, signer: &[u8; 32]) {
    let b = f.g.intent(n, Intent::Rotate { secret, slot }, &[], signer);
    f.g.ok(b);
}

/// Every `IntentStep` and `IntentDone` of the last intent, in log order.
fn last_intent_entries(f: &Fake) -> Vec<ReleaseEntry> {
    let start = f.g.log.iter().rposition(|b| matches!(SignedEntry::decode(b).unwrap().body, ReleaseEntry::Intent(_)));
    f.g.log[start.unwrap() + 1..].iter().map(|b| SignedEntry::decode(b).unwrap().body).collect()
}

/// `DeployToken` and `ReadToken`: one confirming call with the new slot's value, then the switch, which moves the
/// slot every reader takes from the view; no script is deployed. A second `Rotate` back to slot 0 runs the same way.
#[test]
fn bf46_a_token_rotate_probes_the_new_slot_then_switches_with_no_deploy() {
    let mut f = Fake::new(G::new());
    for (n, secret) in [(30, SecretName::DeployToken), (31, SecretName::ReadToken)] {
        assert_eq!(f.g.f.slot(secret), 0);
        rotate(&mut f, n, secret, 1, &CUSTOMER);
        let log = f.cron(5);
        assert!(f.g.f.current.is_none(), "{log:?}");
        assert_eq!(f.g.f.slot(secret), 1, "{secret:?} switched");
        assert_eq!(f.rotated.last(), Some(&(secret, 1, crate::RotateStep::Probe)));
        let steps: Vec<_> = last_intent_entries(&f).into_iter().map(|e| e.kind()).collect();
        assert_eq!(steps, ["IntentStep", "IntentStep", "IntentDone"]);
    }
    rotate(&mut f, 32, SecretName::DeployToken, 0, &VENDOR);
    f.cron(5);
    assert_eq!((f.g.f.slot(SecretName::DeployToken), f.g.f.slot(SecretName::ReadToken)), (0, 1));
    assert!(f.deploys.is_empty() && f.uploads.is_empty(), "no new script version");
    assert!(f.pages.is_empty(), "{:?}", f.pages);
}

/// A probe the new value fails: the step pages and nothing moves, run after run, until the customer fixes the value.
/// The switch is never committed before its probe passed.
#[test]
fn bf46_a_failed_probe_pages_and_never_switches() {
    let mut f = Fake::new(G::new());
    rotate(&mut f, 30, SecretName::DeployToken, 1, &CUSTOMER);
    f.rotate_fails = Some(crate::RotateStep::Probe);
    for _ in 0..3 {
        let log = block_on(ops::run(&mut f, Dwell::default()));
        assert!(matches!(log.last(), Some(Ran::Paged(why)) if why.contains("DeployToken")), "{log:?}");
    }
    assert_eq!(f.g.f.slot(SecretName::DeployToken), 0);
    assert_eq!(f.g.f.current.as_deref().unwrap().next, 0, "no step committed");
    f.rotate_fails = None;
    f.cron(5);
    assert_eq!(f.g.f.slot(SecretName::DeployToken), 1);
}

/// `MintWrapKey`: publish the new key's `VerifyingKey` row, wait one installation-token lifetime from that step, switch,
/// then retire the old key. The switch waits the full lifetime, measured from the publish step's commit.
#[test]
fn bf46_mint_wrap_key_publishes_waits_a_token_lifetime_switches_then_retires() {
    use crate::RotateStep::*;
    let mut f = Fake::new(G::new());
    rotate(&mut f, 30, SecretName::MintWrapKey, 1, &CUSTOMER);
    let log = block_on(ops::run(&mut f, Dwell::default()));
    assert_eq!(f.rotated, [(SecretName::MintWrapKey, 1, Publish)]);
    let published = f.g.f.current.as_deref().unwrap().since;
    assert!(
        matches!(log.last(), Some(Ran::Wait { until: Some(t), .. }) if t.0 == published.0 + ops::ROTATE_PUBLISHED_FOR),
        "{log:?}"
    );
    // A minute short of the lifetime: still waiting, still slot 0.
    f.g.at = Timestamp(published.0 + ops::ROTATE_PUBLISHED_FOR - 60_000);
    block_on(ops::run(&mut f, Dwell::default()));
    assert_eq!(f.g.f.slot(SecretName::MintWrapKey), 0);
    f.g.at = Timestamp(published.0 + ops::ROTATE_PUBLISHED_FOR);
    f.cron(5);
    assert!(f.g.f.current.is_none());
    assert_eq!(f.g.f.slot(SecretName::MintWrapKey), 1);
    let (_, switched) = f.g.f.slots[&SecretName::MintWrapKey];
    assert!(switched.0 >= published.0 + ops::ROTATE_PUBLISHED_FOR, "switched only after the lifetime");
    assert_eq!(f.rotated, [(SecretName::MintWrapKey, 1, Publish), (SecretName::MintWrapKey, 1, Retire)]);
    // A test install's lowered wait (`OPS_ROTATE_PUBLISHED_MS`) moves the switch, and only the switch: back to slot 0.
    let short = Dwell { rotate_published: 120_000, ..Dwell::default() };
    rotate(&mut f, 31, SecretName::MintWrapKey, 0, &CUSTOMER);
    block_on(ops::run(&mut f, short));
    let published = f.g.f.current.as_deref().unwrap().since;
    f.g.at = Timestamp(published.0 + 60_000);
    block_on(ops::run(&mut f, short));
    assert_eq!(f.g.f.slot(SecretName::MintWrapKey), 1, "a minute in: still slot 1");
    f.g.at = Timestamp(published.0 + 120_000);
    for _ in 0..5 {
        block_on(ops::run(&mut f, short));
    }
    assert!(f.g.f.current.is_none());
    assert_eq!(f.g.f.slot(SecretName::MintWrapKey), 0);
}

/// A `Rotate` this `graft-ops` does not switch, or one to the active slot, is refused by name: a page each run and no
/// step, until the customer's `Abort` ends it with the slot unchanged.
#[test]
fn bf46_a_rotate_graft_ops_does_not_run_pages_by_name_and_an_abort_ends_it() {
    for (secret, slot, said) in [
        (SecretName::SshHostKeys, 1, "SSH_HOST_KEY_ACTIVE"),
        (SecretName::McpAppKey, 1, "binds no slot"),
        (SecretName::HostedRunnersAppKey, 1, "runner account"),
        (SecretName::SecretsWrapKey, 1, "no row"),
        (SecretName::TunnelToken, 1, "retired"),
        (SecretName::DeployToken, 0, "the active slot"),
    ] {
        let mut f = Fake::new(G::new());
        rotate(&mut f, 30, secret, slot, &CUSTOMER);
        let log = block_on(ops::run(&mut f, Dwell::default()));
        assert!(
            matches!(log.last(), Some(Ran::Paged(why)) if why.contains(said) && why.contains("refuses")),
            "{secret:?}: {log:?}"
        );
        assert!(f.rotated.is_empty() && f.g.f.current.as_deref().unwrap().next == 0, "{secret:?}");
        f.g.upload(1);
        let b = f.g.entry(ReleaseEntry::Abort { intent: id(30), target: rel(1).0 }, &[&CUSTOMER]);
        f.g.ok(b);
        f.cron(3);
        assert!(f.g.f.current.is_none(), "{secret:?}: the Abort ended it");
        assert_eq!(f.g.f.slot(secret), 0, "{secret:?}");
    }
}

/// The agents account's secrets are its own `graft-ops`'s: the install's pages and steps nothing, and the agents
/// executor key may step them.
#[test]
fn bf46_the_install_graft_ops_leaves_the_agents_secrets_to_the_agents_account() {
    let mut f = Fake::new(G::new());
    rotate(&mut f, 30, SecretName::AgentsAppKey, 1, &CUSTOMER);
    let log = block_on(ops::run(&mut f, Dwell::default()));
    assert!(matches!(log.last(), Some(Ran::Paged(why)) if why.contains("agents account")), "{log:?}");
    let b = f.g.entry(ReleaseEntry::IntentStep { intent: id(30), step: 0 }, &[&AGENTS]);
    f.g.ok(b);
}

/// BF-39's rule for `Rotate`: killed after one to three calls in every run (a view read, a host step, a commit), a `MintWrapKey` rotation still commits each
/// step once, in order, with one `IntentDone`, and the host steps it reruns are the idempotent ones.
#[test]
fn a_killed_graft_ops_reruns_a_rotate_step_and_commits_each_step_once() {
    let mut f = Fake::new(G::new());
    rotate(&mut f, 30, SecretName::MintWrapKey, 1, &CUSTOMER);
    let mut rng: u64 = 0x46;
    for _ in 0..200 {
        if f.g.f.current.is_none() {
            break;
        }
        rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        f.kill_after = Some(1 + ((rng >> 33) % 3) as usize);
        block_on(ops::run(&mut f, Dwell::default()));
        f.kill_after = None;
        f.g.at = Timestamp(f.g.at.0 + 20 * 60_000);
    }
    assert!(f.g.f.current.is_none());
    let steps: Vec<_> = last_intent_entries(&f)
        .into_iter()
        .map(|e| match e {
            ReleaseEntry::IntentStep { step, .. } => i32::from(step),
            _ => -1,
        })
        .collect();
    assert_eq!(steps, [0, 1, 2, 3, -1], "each step once, in order, then one IntentDone");
    assert_eq!(f.g.f.slot(SecretName::MintWrapKey), 1);
}
