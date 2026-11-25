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
    /// Versions uploaded by (tag, script), as the versions API lists them by tag.
    tags: BTreeMap<(String, Script), VersionId>,
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
    /// The account this `graft-ops` runs in: the install's, or the agents account's, which signs with `AGENTS`.
    account: ops::Account,
    /// What `wrap_var` answers: the host's `SECRETS_WRAP_KEY_CURRENT`.
    wrap_var: u8,
}

impl Fake {
    fn new(g: G) -> Fake {
        let mut deps = BTreeMap::new();
        for s in [Script::Site(SiteId(0)), Script::Site(SiteId(1)), Script::Site(SiteId(2)), Script::Site(SiteId(3))]
            .into_iter()
            .chain([Script::Edge, Script::Ops, Script::AgentsFront, Script::Controller])
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
            account: ops::Account::Install,
            wrap_var: 0,
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
    fn account(&self) -> ops::Account {
        self.account
    }

    fn now(&self) -> Timestamp {
        self.g.at
    }

    fn wrap_var(&self) -> u8 {
        self.wrap_var
    }

    fn scripts(&self) -> Vec<Script> {
        match self.account {
            ops::Account::Install => vec![Script::Edge, Script::Ops],
            ops::Account::Agents => vec![Script::AgentsFront, Script::Controller],
        }
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
        e.sign(match (self.account, signer) {
            (ops::Account::Agents, Signer::Executor) => &AGENTS,
            (ops::Account::Agents, Signer::Drain) => return Err("no drain key".into()),
            (_, Signer::Executor) => &EXECUTOR,
            (_, Signer::Drain) => &DRAIN,
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

    async fn tagged(&mut self, tag: &str, scripts: &[Script]) -> Result<BTreeMap<Script, VersionId>, String> {
        Ok(scripts.iter().filter_map(|s| Some((*s, *self.tags.get(&(tag.to_string(), *s))?))).collect())
    }

    async fn upload(&mut self, s: Script, tag: &str, _modules: &[(String, Vec<u8>)]) -> Result<(), String> {
        self.next_v += 1;
        self.tags.insert((tag.to_string(), s), VersionId([self.next_v; 16]));
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

/// BM-5's `ReleaseTag`: every version of a release carries `<seq>-<release id>`, seq being the release's place in the
/// group's uploads. R1 is 1 and R2 is 2 on every script and every site, and a version made later for R1 (as a secret's
/// site day makes one) still names seq 1: the order is the group's, not the version's upload time. A tag in any other
/// shape names no release and never panics.
#[test]
fn bm5_a_release_s_seq_is_its_place_in_the_group_not_its_upload_time() {
    let mut f = Fake::new(G::new());
    let b1 = raw("t1", 3, &RELEASE);
    let r1 = release_of(&b1);
    f.bucket.push(b1);
    f.cron(10);
    let b2 = raw("t2", 4, &RELEASE);
    let r2 = release_of(&b2);
    f.bucket.push(b2);
    f.cron(10);
    let hex = |r: &ReleaseId| crate::types::hex(&r.0);
    let (t1, t2) = (format!("1-{}", hex(&r1)), format!("2-{}", hex(&r2)));
    assert_eq!(ops::version_tag(&f.g.f, &r1), t1);
    assert_eq!(ops::version_tag(&f.g.f, &r2), t2);
    // Each site's version and every pinned script's carry the release's one tag.
    for s in [Script::Site(SiteId(0)), Script::Site(SiteId(1)), Script::Site(SiteId(2)), Script::Edge, Script::Ops] {
        assert_eq!(f.tags.get(&(t1.clone(), s)), ops::version(&f.g.f, &r1, s).as_ref(), "{s:?} under R1");
        assert_eq!(f.tags.get(&(t2.clone(), s)), ops::version(&f.g.f, &r2, s).as_ref(), "{s:?} under R2");
    }
    // A version uploaded for R1 after R2's upload: its tag is still R1's, so it ranks below R2.
    let (s1, _) = ops::parse_version_tag(&ops::version_tag(&f.g.f, &r1)).unwrap();
    let (s2, _) = ops::parse_version_tag(&t2).unwrap();
    assert!(s1 < s2, "R1 re-uploaded after R2 must rank below R2");
    assert_eq!(ops::parse_version_tag(&t1), Some((1, r1)));
    // A release not yet uploaded takes the next place.
    assert_eq!(ops::version_tag(&f.g.f, &ReleaseId([7; 32])), format!("3-{}", hex(&ReleaseId([7; 32]))));
    // Tags that name no release: a bare id (an upload before this change), a wrangler tag, seq 0, a short id, junk.
    for bad in [
        hex(&r1),
        "v1.0.1".to_string(),
        format!("0-{}", hex(&r1)),
        format!("-{}", hex(&r1)),
        format!("1-{}", &hex(&r1)[..63]),
        format!("1-{}", hex(&r1).to_uppercase()),
        format!("99999999999-{}", hex(&r1)),
        "1-".to_string(),
        String::new(),
    ] {
        assert_eq!(ops::parse_version_tag(&bad), None, "{bad:?}");
    }
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
    let e1 = ops::version(&f.g.f, &r1, Script::Edge).unwrap();
    let e2 = ops::version(&f.g.f, &r2, Script::Edge).unwrap();
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

/// R45: the App credentials and `SecretsWrapKey` probe the new slot, then switch, with no deploy; the switch moves only
/// the rotated secret's slot, which each holder reads from its view.
#[test]
fn r45_app_credentials_probe_then_switch_with_no_deploy() {
    let mut f = Fake::new(G::new());
    let secrets =
        [SecretName::McpAppKey, SecretName::McpClientSecret, SecretName::InsightsAppKey, SecretName::SecretsWrapKey];
    for (n, secret) in (40..).zip(secrets) {
        rotate(&mut f, n, secret, 1, &CUSTOMER);
        let log = f.cron(5);
        assert!(f.g.f.current.is_none(), "{secret:?}: {log:?}");
        assert_eq!(f.rotated.last(), Some(&(secret, 1, crate::RotateStep::Probe)));
        let steps: Vec<_> = last_intent_entries(&f).into_iter().map(|e| e.kind()).collect();
        assert_eq!(steps, ["IntentStep", "IntentStep", "IntentDone"], "{secret:?}");
    }
    for secret in secrets {
        assert_eq!(f.g.f.slot(secret), 1, "{secret:?}");
    }
    assert_eq!(f.g.f.slot(SecretName::DeployToken), 0, "no other secret moved");
    assert!(f.deploys.is_empty() && f.uploads.is_empty(), "no new script version");
    // A probe the App's record fails (the new key is not listed yet): it pages, and the slot stays.
    rotate(&mut f, 50, SecretName::McpAppKey, 0, &CUSTOMER);
    f.rotate_fails = Some(crate::RotateStep::Probe);
    f.cron(3);
    assert_eq!(f.g.f.slot(SecretName::McpAppKey), 1);
    assert!(f.pages.iter().any(|p| p.contains("McpAppKey")), "{:?}", f.pages);
}

/// `SecretsWrapKey` before its first `Rotate`: the active slot is `SECRETS_WRAP_KEY_CURRENT`'s, the slot `/wrap` already
/// wraps under, not the fold's default 0. An install whose variable names 1 (graft-m4a since BF-59) rotates to 0 at
/// once, and a `Rotate` to 1 is refused as the active slot. After a switch the fold decides, whatever the variable says.
#[test]
fn a_first_secrets_wrap_key_rotate_reads_the_variable_s_slot() {
    let mut f = Fake::new(G::new());
    f.wrap_var = 1;
    rotate(&mut f, 60, SecretName::SecretsWrapKey, 0, &CUSTOMER);
    let log = f.cron(5);
    assert!(f.g.f.current.is_none(), "{log:?}; pages {:?}", f.pages);
    assert_eq!(f.rotated.last(), Some(&(SecretName::SecretsWrapKey, 0, crate::RotateStep::Probe)));
    assert_eq!(f.g.f.slots.get(&SecretName::SecretsWrapKey).map(|(s, _)| *s), Some(0), "switched to 0");
    // Switched: the fold now decides. A Rotate to 0 again is the active slot even though the variable still says 1.
    rotate(&mut f, 61, SecretName::SecretsWrapKey, 0, &CUSTOMER);
    f.cron(2);
    assert!(f.pages.iter().any(|p| p.contains("SecretsWrapKey to slot 0, the active slot")), "{:?}", f.pages);

    let mut f = Fake::new(G::new());
    f.wrap_var = 1;
    rotate(&mut f, 62, SecretName::SecretsWrapKey, 1, &CUSTOMER);
    f.cron(2);
    assert!(f.pages.iter().any(|p| p.contains("SecretsWrapKey to slot 1, the active slot")), "{:?}", f.pages);
    assert!(f.rotated.is_empty(), "no probe of the slot already in use");
}

/// R45: `SshHostKeys` probes every site's new slot, waits `SSH_ANNOUNCE_FOR` from the probe's commit while each
/// `graft-sshd` announces both slots, then switches. A day short of it, key exchange still serves the old slot.
#[test]
fn r45_ssh_host_keys_wait_the_announce_before_they_switch() {
    let mut f = Fake::new(G::new());
    rotate(&mut f, 30, SecretName::SshHostKeys, 1, &CUSTOMER);
    let log = block_on(ops::run(&mut f, Dwell::default()));
    assert_eq!(f.rotated, [(SecretName::SshHostKeys, 1, crate::RotateStep::Probe)]);
    let probed = f.g.f.current.as_deref().unwrap().since;
    // DESIGN's 30-day announce, written out so a changed constant fails here.
    let days30 = 30 * 24 * 3_600_000;
    assert!(matches!(log.last(), Some(Ran::Wait { until: Some(t), .. }) if t.0 == probed.0 + days30), "{log:?}");
    f.g.at = Timestamp(probed.0 + ops::SSH_ANNOUNCE_FOR - 24 * 3_600_000);
    block_on(ops::run(&mut f, Dwell::default()));
    assert_eq!(f.g.f.slot(SecretName::SshHostKeys), 0, "a day short: still slot 0");
    // `MintWrapKey`'s wait does not stand in for it: a lowered publish wait leaves the announce as it is.
    block_on(ops::run(&mut f, Dwell { rotate_published: 0, ..Dwell::default() }));
    assert_eq!(f.g.f.slot(SecretName::SshHostKeys), 0);
    f.g.at = Timestamp(probed.0 + ops::SSH_ANNOUNCE_FOR);
    f.cron(5);
    assert!(f.g.f.current.is_none());
    assert_eq!(f.g.f.slot(SecretName::SshHostKeys), 1);
    assert!(f.deploys.is_empty() && f.uploads.is_empty(), "no site day");
    // A test install's lowered announce (`OPS_SSH_ANNOUNCE_MS`) moves only this wait.
    let short = Dwell { ssh_announce: 120_000, ..Dwell::default() };
    rotate(&mut f, 31, SecretName::SshHostKeys, 0, &CUSTOMER);
    block_on(ops::run(&mut f, short));
    let probed = f.g.f.current.as_deref().unwrap().since;
    f.g.at = Timestamp(probed.0 + 120_000);
    for _ in 0..5 {
        block_on(ops::run(&mut f, short));
    }
    assert_eq!(f.g.f.slot(SecretName::SshHostKeys), 0);
}

/// `ActionsOidcKey` (actions-oidc): probe every site's JWKS for the new slot's key, wait `OIDC_PUBLISHED_FOR` from the
/// probe's commit while relying parties refetch it, then switch, with no deploy. A test install's lowered wait
/// (`OPS_OIDC_PUBLISHED_MS`) moves this wait alone, and neither other wait stands in for it.
#[test]
fn an_actions_oidc_key_probes_waits_a_day_then_switches() {
    let mut f = Fake::new(G::new());
    rotate(&mut f, 30, SecretName::ActionsOidcKey, 1, &CUSTOMER);
    let log = block_on(ops::run(&mut f, Dwell::default()));
    assert_eq!(f.rotated, [(SecretName::ActionsOidcKey, 1, crate::RotateStep::Probe)]);
    let probed = f.g.f.current.as_deref().unwrap().since;
    // The 24 h of DESIGN's runbook, written out so a changed constant fails here.
    let day = 24 * 3_600_000;
    assert!(matches!(log.last(), Some(Ran::Wait { until: Some(t), .. }) if t.0 == probed.0 + day), "{log:?}");
    f.g.at = Timestamp(probed.0 + day - 60_000);
    block_on(ops::run(&mut f, Dwell { rotate_published: 0, ssh_announce: 0, ..Dwell::default() }));
    assert_eq!(f.g.f.slot(SecretName::ActionsOidcKey), 0, "a minute short, whatever the other waits");
    f.g.at = Timestamp(probed.0 + day);
    f.cron(5);
    assert!(f.g.f.current.is_none());
    assert_eq!(f.g.f.slot(SecretName::ActionsOidcKey), 1);
    assert!(f.deploys.is_empty() && f.uploads.is_empty(), "no new script version");
    let short = Dwell { oidc_published: 120_000, ..Dwell::default() };
    rotate(&mut f, 31, SecretName::ActionsOidcKey, 0, &CUSTOMER);
    block_on(ops::run(&mut f, short));
    let probed = f.g.f.current.as_deref().unwrap().since;
    f.g.at = Timestamp(probed.0 + 120_000);
    for _ in 0..5 {
        block_on(ops::run(&mut f, short));
    }
    assert_eq!(f.g.f.slot(SecretName::ActionsOidcKey), 0);
    // A probe that fails (a site's JWKS lacks the key) pages, and the slot stays.
    rotate(&mut f, 32, SecretName::ActionsOidcKey, 1, &CUSTOMER);
    f.rotate_fails = Some(crate::RotateStep::Probe);
    f.cron(3);
    assert_eq!(f.g.f.slot(SecretName::ActionsOidcKey), 0);
    assert!(f.pages.iter().any(|p| p.contains("ActionsOidcKey")), "{:?}", f.pages);
}

/// A `Rotate` this `graft-ops` does not switch, or one to the active slot, is refused by name: a page each run and no
/// step, until the customer's `Abort` ends it with the slot unchanged.
#[test]
fn bf46_a_rotate_graft_ops_does_not_run_pages_by_name_and_an_abort_ends_it() {
    for (secret, slot, said) in [
        (SecretName::HostedRunnersAppKey, 1, "runner account"),
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

/// The 7-day silence alarm: `graft-ops` pages for a registered lifeboat heard neither by its `LifeboatAdd` nor by a
/// `LifeboatDecoder` for more than `LIFEBOAT_SILENT`, once a run, and moves nothing for it. A decoder entry, or the
/// lifeboat's removal, ends the page.
#[test]
fn a_lifeboat_silent_for_seven_days_pages_and_a_decoder_or_removal_clears_it() {
    let mut f = Fake::new(G::new());
    let lb = [11u8; 32];
    let host = "lb.acme.example";
    let b = f.g.entry(ReleaseEntry::LifeboatAdd { host: host.into(), key: public_key(&lb) }, &[&CUSTOMER]);
    f.g.ok(b);
    assert_eq!(f.g.f.lifeboat_seen.get(host), Some(&f.g.at));
    let silent = ops::LIFEBOAT_SILENT;
    let run = |f: &mut Fake| {
        f.pages.clear();
        block_on(ops::run(f, Dwell::default()));
        f.pages.iter().filter(|p| p.contains("lifeboat")).count()
    };
    let added = f.g.at;
    f.g.at = Timestamp(added.0 + silent);
    assert_eq!(run(&mut f), 0, "exactly 7 days is not yet silent");
    f.g.at = Timestamp(added.0 + silent + 1);
    assert_eq!(run(&mut f), 1, "past 7 days pages, once a run");
    super::ops::upload_all(&mut f.g, 1);
    let b = f.g.entry(ReleaseEntry::LifeboatDecoder { host: host.into(), release: rel(1).0 }, &[&lb]);
    f.g.ok(b);
    assert_eq!(run(&mut f), 0, "a decoder entry resets the clock");
    f.g.at = Timestamp(f.g.at.0 + silent + 1);
    assert_eq!(run(&mut f), 1);
    let b = f.g.entry(ReleaseEntry::LifeboatRemove { host: host.into() }, &[&CUSTOMER]);
    f.g.ok(b);
    assert!(f.g.f.lifeboat_seen.is_empty());
    assert_eq!(run(&mut f), 0, "a removed lifeboat never pages");
}

/// R57: an upload is configured as the deployed version is, not as the newest upload. The deployment serves N at 99%
/// and N+1 at 1%; a version uploaded since by hand, never deployed, has a stray var and another value for a shared one.
/// The upload sends N's vars, buckets and runtime settings as N holds them, keeps no plain-text binding from the newest
/// upload, and sends no secret (it is kept: no read returns its value).
#[test]
fn r57_an_upload_takes_the_deployed_version_s_bindings_not_the_newest_upload_s() {
    use serde_json::{Value, json};
    let (n, n1, stray) = (VersionId([1; 16]), VersionId([2; 16]), VersionId([3; 16]));
    let read = |v: VersionId| -> Value {
        let (formats, extra, cpu) = match v {
            v if v == n => ("32.1,32.4", None, 300_000),
            v if v == stray => (
                "32.1,32.4,20.4",
                Some(json!({ "type": "plain_text", "name": "FORCE_FORMAT", "text": "32.65520" })),
                50,
            ),
            _ => ("32.1", None, 10),
        };
        let mut b = vec![
            json!({ "type": "plain_text", "name": "FORMATS_ENABLED", "text": formats }),
            json!({ "type": "r2_bucket", "name": "BUCKET_0", "bucket_name": "packs-a" }),
            json!({ "type": "secret_text", "name": "BED_TOKEN" }),
        ];
        b.extend(extra);
        json!({ "resources": { "bindings": b, "script_runtime": { "compatibility_date": "2026-10-01", "limits": { "cpu_ms": cpu } } } })
    };
    // N+1 at 1% and N at 99%, in the order the deployments API lists them; the stray version is in no deployment.
    let split = vec![(n1, 1), (n, 99)];
    let source = ops::upload_source(&split).expect("a deployed version");
    assert_eq!(source, n, "the version serving the most traffic, not N+1 and not the newest upload");
    let meta = ops::upload_metadata(&read(source), "index.js", "7-ab");
    let bindings = meta["bindings"].as_array().unwrap();
    let var = |name: &str| bindings.iter().find(|b| b["name"] == json!(name)).map(|b| b["text"].clone());
    assert_eq!(var("FORMATS_ENABLED"), Some(json!("32.1,32.4")), "N's value, not the stray upload's");
    assert_eq!(var("FORCE_FORMAT"), None, "the stray upload's var is not carried");
    assert!(bindings.iter().any(|b| b["name"] == json!("BUCKET_0") && b["bucket_name"] == json!("packs-a")));
    assert!(bindings.iter().all(|b| b["type"] != json!("secret_text")), "a secret's value is never sent");
    assert_eq!(meta["keep_bindings"], json!(["secret_text"]), "only secrets are kept from the newest upload");
    assert_eq!(meta["limits"], json!({ "cpu_ms": 300_000 }), "N's runtime settings");
    assert_eq!(meta["annotations"]["workers/tag"], json!("7-ab"));
    assert_eq!(meta["main_module"], json!("index.js"));
    assert_ne!(read(stray)["resources"], read(n)["resources"], "the newest upload is configured otherwise");
}

// ---- AM3-WP1: the agents account's `graft-ops` ----

/// A signed bundle carrying the agents account's scripts, tagged `tag`, beside a site and an edge script the agents
/// account must not upload.
fn agents_raw(tag: &str) -> RawBundle {
    let files: Vec<(String, Vec<u8>)> = vec![
        ("formats.txt".into(), reads(3).file().into_bytes()),
        ("site/index.js".into(), format!("site {tag}").into_bytes()),
        ("edge/index.js".into(), format!("edge {tag}").into_bytes()),
        ("agents-front/index.js".into(), format!("agents-front {tag}").into_bytes()),
        ("controller/index.js".into(), format!("controller {tag}").into_bytes()),
    ];
    let listed: Vec<String> = files
        .iter()
        .map(|(f, b)| format!(r#"{{"name":"{f}","sha256":"{}"}}"#, crate::types::hex(&Sha256::digest(b))))
        .collect();
    let record = format!(r#"{{"label":"{tag}","files":[{}]}}"#, listed.join(",")).into_bytes();
    RawBundle { sig: sign_record(&RELEASE, &record), record, files }
}

/// Every entry after the first `ReleaseUploaded` of the agents scripts, with the key that signed it.
fn signers_since(f: &Fake, from: usize) -> Vec<(&'static str, KeyId)> {
    f.g.log[from..]
        .iter()
        .map(|b| {
            let e = SignedEntry::decode(b).unwrap();
            (e.body.kind(), e.sigs[0].0)
        })
        .collect()
}

/// The agents account's `graft-ops` uploads only `graft-agents-front` and `graft-controller` and commits their
/// `ReleaseUploaded` under `Keys.agents_executor`; `release roll` signs a `ControllerRoll`, which it alone runs, 1%
/// then 100% with N kept at 0%, every step under the agents key. The install's `graft-ops` sits idle while it is
/// current, and the agents one sits idle while any other intent is, drains no site, and pages for no lifeboat.
#[test]
fn the_agents_graft_ops_runs_controller_rolls_alone_under_the_agents_key() {
    let mut f = Fake::new(G::new());
    f.account = ops::Account::Agents;
    let b1 = agents_raw("c1");
    let r1 = release_of(&b1);
    f.bucket.push(b1);
    let from = f.g.log.len();
    f.cron(5);
    assert_eq!(f.uploads, [Script::AgentsFront, Script::Controller]);
    assert!(roll(&mut f, r1, 40), "release roll signs the ControllerRoll");
    assert!(matches!(f.g.f.current.as_deref().unwrap().intent.intent, Intent::ControllerRoll { .. }));
    // The install's graft-ops leaves it alone: no deploy, no step.
    f.account = ops::Account::Install;
    assert_eq!(block_on(ops::run(&mut f, Dwell::default())), vec![Ran::Idle]);
    assert!(f.deploys.is_empty());
    f.account = ops::Account::Agents;
    // A burning site with no drain: the agents account drains nothing, and the roll goes on.
    f.health.insert(1, crate::gate::Verdict { burning: true, ..Default::default() });
    f.cron(10);
    assert!(f.g.f.current.is_none(), "{:?}", f.pages);
    assert!(f.g.f.drained.is_empty(), "the agents graft-ops drained a site");
    let c1 = ops::version(&f.g.f, &r1, Script::Controller).unwrap();
    let old = VersionId([0xee; 16]);
    let ctl: Vec<_> = f.deploys.iter().filter(|(s, _)| *s == Script::Controller).map(|(_, x)| x.clone()).collect();
    assert_eq!(ctl, vec![vec![(c1, 1), (old, 99)], vec![(c1, 100)]]);
    assert!(f.deploys.iter().all(|(s, _)| *s == Script::Controller), "{:?}", f.deploys);
    assert_eq!(f.g.f.rolled.get(&Script::Controller), Some(&r1));
    let agents = public_key(&AGENTS);
    let entries = signers_since(&f, from);
    let mine: Vec<_> = entries.iter().filter(|(k, _)| *k != "Intent").collect();
    assert_eq!(
        mine.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
        ["ReleaseUploaded", "IntentStep", "IntentStep", "IntentDone"]
    );
    assert!(mine.iter().all(|(_, key)| *key == agents), "every graft-ops entry is under Keys.agents_executor");
    // Any other intent: the agents graft-ops is idle and moves nothing.
    f.g.upload(5);
    let b = f.g.intent(41, Intent::EdgeRoll { release: rel(5).0 }, &[], &VENDOR);
    f.g.ok(b);
    let deploys = f.deploys.len();
    assert_eq!(block_on(ops::run(&mut f, Dwell::default())), vec![Ran::Idle]);
    assert_eq!(f.deploys.len(), deploys);
    assert!(f.pages.is_empty(), "{:?}", f.pages);
}

/// LF-7's in-memory half: a bad `graft-controller` at 1% breaches while the agents account's API is down. The run
/// submits the pre-signed `ControllerPointer` to N, a signed entry, so `Pointers.controller` pins N at once and an
/// `AgentsReleaseView` of the feed carries the pin; the return to N at 100%, N+1 kept at 0%, waits for the API.
#[test]
fn lf7_a_controller_breach_with_the_api_down_commits_the_pointer_and_deploys_later() {
    let mut f = Fake::new(G::new());
    f.account = ops::Account::Agents;
    let (b1, b2) = (agents_raw("c1"), agents_raw("c2"));
    let (r1, r2) = (release_of(&b1), release_of(&b2));
    f.bucket.push(b1);
    f.cron(5);
    assert!(roll(&mut f, r1, 40));
    f.cron(10);
    f.bucket.push(b2);
    f.cron(5);
    assert!(roll(&mut f, r2, 41));
    let c = f.g.f.current.as_deref().unwrap();
    assert_eq!(c.undo.len(), 1, "the ControllerRoll carries its ControllerPointer back to N");
    block_on(ops::run(&mut f, Dwell { step: HOUR, first_day: HOUR, ..Dwell::default() }));
    let (c1, c2) = (
        ops::version(&f.g.f, &r1, Script::Controller).unwrap(),
        ops::version(&f.g.f, &r2, Script::Controller).unwrap(),
    );
    assert_eq!(f.deps[&Script::Controller].split, vec![(c2, 1), (c1, 99)]);
    f.api_down = true;
    f.gate = Gate::Breach("test-bad".into());
    let deploys = f.deploys.len();
    let log = block_on(ops::run(&mut f, Dwell { step: HOUR, first_day: HOUR, ..Dwell::default() }));
    assert!(log.contains(&Ran::Committed("ControllerPointer")), "{log:?}");
    assert_eq!(f.g.f.pointers.get(Script::Controller), Some(c1), "pinned to N with the API down");
    assert_eq!(f.deploys.len(), deploys);
    let a = crate::agents::AgentsAnchor { install: f.g.a.install, customer: f.g.a.customer, host: "h".into() };
    let page: Vec<_> =
        f.g.log
            .iter()
            .zip(&f.g.ats)
            .enumerate()
            .map(|(i, (b, at))| (graftyard_types::Index(i as u64 + 1), b.clone(), *at))
            .collect();
    let mut v = crate::agents::AgentsReleaseView::default();
    v.extend(&a, &page).unwrap();
    assert_eq!(v.controller(), Some(c1), "graft-agents-front's view pins N from the feed alone");
    f.api_down = false;
    f.gate = Gate::Pass;
    f.cron(10);
    assert!(f.g.f.current.is_none());
    assert_eq!(f.deps[&Script::Controller].split, vec![(c1, 100), (c2, 0)]);
    assert!(f.pages.is_empty(), "{:?}", f.pages);
}
