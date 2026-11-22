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
    next_v: u8,
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
            next_v: 1,
        }
    }

    /// Runs `ops::run` with a one-minute clock step between runs, as a cron would, until nothing is current and no
    /// upload is pending, at most `n` runs.
    fn cron(&mut self, n: usize) -> Vec<Ran> {
        let mut all = Vec::new();
        for _ in 0..n {
            let log = block_on(ops::run(self, Dwell { step: 0, first_day: 0 }));
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

impl Exec for Fake {
    fn now(&self) -> Timestamp {
        self.g.at
    }

    fn scripts(&self) -> Vec<Script> {
        vec![Script::Edge, Script::Ops]
    }

    async fn view(&mut self) -> Result<ReleaseFold, String> {
        Ok(self.lag.take().unwrap_or_else(|| self.g.f.clone()))
    }

    async fn deployment(&mut self, s: Script) -> Result<Deployment, String> {
        self.deps.get(&s).cloned().ok_or_else(|| format!("no {s:?}"))
    }

    async fn gate(&mut self, _fold: &ReleaseFold) -> Gate {
        self.gate.clone()
    }

    async fn deploy(&mut self, s: Script, split: &Split) -> Result<(), String> {
        self.deps.insert(s, Deployment { split: split.clone(), created: self.g.at });
        self.deploys.push((s, split.clone()));
        Ok(())
    }

    async fn submit(&mut self, bytes: &[u8]) -> Submit {
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
        runs.push(block_on(ops::run(&mut f, Dwell { step: 0, first_day: 0 })));
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
        let log = block_on(ops::run(&mut f, Dwell { step: 0, first_day: HOUR }));
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
    let log = block_on(ops::run(&mut f, Dwell { step: 0, first_day: HOUR }));
    assert_eq!(log.last(), Some(&Ran::Refused { kind: "Rollback", variant: "UnreadableFormat".into() }), "{log:?}");
    assert_eq!(f.pages.len(), 1, "{:?}", f.pages);
    assert!(f.pages[0].contains("Rollback refused, UnreadableFormat"), "{}", f.pages[0]);
    assert_eq!(f.g.f, before.0, "nothing committed");
    assert_eq!(f.deploys.len(), before.1, "nothing deployed");
}
