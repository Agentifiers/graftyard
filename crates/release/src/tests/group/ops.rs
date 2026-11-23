//! `graft-ops` as the executor (M5-WP3), in memory: `ops::next` driven against a fake world (each script's deployment
//! and the step's gate) and the real group (`ReleaseFold::decide` and `apply`), with `graftyard release roll`'s
//! `next_roll` choosing the intents. BF-7 is `bf7_...`; the kill test is BF-39's rule (M5-WP4 runs it hosted).

use std::collections::BTreeMap;

use super::*;
use crate::gate::Verdict;
use crate::ops::{self, Action, Deployment, Dwell, Gate, Roll, Signer, UploadStep, World};

/// The world `graft-ops` reads: every script's deployment, the gate, and the time, which is the group's next entry
/// time (`G.at`), so `since` and `now` are one clock.
pub(super) struct Ops {
    pub(super) g: G,
    pub(super) deps: BTreeMap<Script, Deployment>,
    pub(super) gate: Gate,
    pub(super) health: BTreeMap<u8, Verdict>,
    /// Every deployment created, in order.
    pub(super) deployed: Vec<(Script, ops::Split)>,
    /// The intents `release roll` submitted, in order.
    pub(super) rolled: Vec<Intent>,
    pub(super) next_id: u8,
}

pub(super) const OLD: VersionId = VersionId([0xee; 16]);

impl Ops {
    pub(super) fn new(g: G) -> Ops {
        let mut deps = BTreeMap::new();
        for s in [
            Script::Site(SiteId(0)),
            Script::Site(SiteId(1)),
            Script::Site(SiteId(2)),
            Script::Site(SiteId(3)),
            Script::Workflows,
            Script::Edge,
            Script::Ops,
        ] {
            deps.insert(s, Deployment { split: vec![(OLD, 100)], created: T0 });
        }
        Ops {
            g,
            deps,
            gate: Gate::Pass,
            health: BTreeMap::new(),
            deployed: Vec::new(),
            rolled: Vec::new(),
            next_id: 100,
        }
    }

    pub(super) fn world(&self) -> World {
        World { now: self.g.at, deployments: self.deps.clone(), gate: self.gate.clone(), health: self.health.clone() }
    }

    fn seed(s: Signer) -> &'static [u8; 32] {
        match s {
            Signer::Executor => &EXECUTOR,
            Signer::Drain => &DRAIN,
        }
    }

    /// Does what `next` says once. Returns the action.
    pub(super) fn act(&mut self) -> Action {
        let a = ops::next(&self.g.f, &self.world(), Dwell::default());
        match &a {
            Action::Deploy { script, split } => {
                self.deps.insert(*script, Deployment { split: split.clone(), created: self.g.at });
                self.deployed.push((*script, split.clone()));
            }
            Action::Commit { body, signer } => {
                let b = self.g.entry(body.clone(), &[Ops::seed(*signer)]);
                self.g.ok(b);
            }
            Action::Undo(e) => {
                let b = e.encode();
                self.g.ok(b);
            }
            Action::Wait { until: Some(t), .. } => self.g.at = Timestamp(self.g.at.0.max(t.0)),
            Action::Wait { until: None, why } => panic!("waits with no time: {why}"),
            Action::Page(why) => panic!("paged: {why}"),
            Action::Rotate { .. } => panic!("a roll runs no Rotate step"),
            Action::Idle => {}
        }
        a
    }

    /// Runs the current intent to its `IntentDone`.
    pub(super) fn finish(&mut self) {
        for _ in 0..200 {
            if self.g.f.current.is_none() {
                return;
            }
            self.act();
        }
        panic!("the intent did not finish");
    }

    /// `release roll R` once: signs and commits the next intent.
    pub(super) fn roll(&mut self, r: u8) -> Roll {
        let next = ops::next_roll(&self.g.f, rel(r).0);
        if let Roll::Next { intent, undo } = &next {
            self.next_id += 1;
            let id = id(self.next_id);
            let e = ops::sign_intent(&self.g.f, intent.clone(), undo, &VENDOR, id, self.g.at, 4 * HOUR);
            self.g.ok(e.encode());
            self.rolled.push(intent.clone());
        }
        next
    }

    /// `release roll R` until done, running each intent to its end.
    pub(super) fn roll_all(&mut self, r: u8) {
        for _ in 0..30 {
            match self.roll(r) {
                Roll::Next { .. } => self.finish(),
                Roll::Done => return,
                Roll::Wait(why) => panic!("roll waits: {why}"),
            }
        }
        panic!("the roll did not end");
    }
}

/// Every script this install runs, the spare's site included.
const ALL: [Script; 6] = [
    Script::Site(SiteId(0)),
    Script::Site(SiteId(1)),
    Script::Site(SiteId(2)),
    Script::Site(SiteId(3)),
    Script::Workflows,
    Script::Edge,
];

pub(super) fn upload_all(g: &mut G, n: u8) {
    let b = g.entry(G::upload_body(n, &[&ALL[..], &[Script::Ops]].concat()), &[&EXECUTOR]);
    g.ok(b);
}

pub(super) fn site_day(s: u8, n: u8) -> Intent {
    Intent::SiteDay { target: DayTarget::Site(SiteId(s)), release: rel(n).0 }
}

pub(super) fn version_of(n: u8, s: Script) -> VersionId {
    let all = [&ALL[..], &[Script::Ops]].concat();
    vid(n, all.iter().position(|x| *x == s).unwrap() as u8)
}

#[test]
fn release_roll_runs_the_order_and_graft_ops_runs_each_intent_to_done() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    o.roll_all(1);
    let want1 = vec![
        site_day(0, 1),
        site_day(1, 1),
        site_day(2, 1),
        site_day(3, 1),
        Intent::SiteDay { target: DayTarget::Workflows, release: rel(1).0 },
        Intent::EdgeRoll { release: rel(1).0 },
        Intent::OpsRoll { release: rel(1).0 },
    ];
    assert_eq!(o.rolled, want1, "member sites, the spare, Workflows, then the edge and graft-ops");
    for s in 0..4 {
        assert_eq!(o.g.f.site_release.get(&s), Some(&rel(1).0), "site {s} reports release 1");
        assert_eq!(o.deps[&Script::Site(SiteId(s))].split, vec![(version_of(1, Script::Site(SiteId(s))), 100)]);
    }
    assert!(o.g.f.drained.is_empty(), "every site day undrained its site");
    // A site day deploys 1%, 10%, 50%, then 100%, with the old version beside it until 100%.
    let site0: Vec<_> =
        o.deployed.iter().filter(|(s, _)| *s == Script::Site(SiteId(0))).map(|(_, x)| x.clone()).collect();
    let v = version_of(1, Script::Site(SiteId(0)));
    assert_eq!(
        site0,
        vec![vec![(v, 1), (OLD, 99)], vec![(v, 10), (OLD, 90)], vec![(v, 50), (OLD, 50)], vec![(v, 100)]]
    );

    // Release 2: every intent now carries its undo to 1, and the roll is idempotent once done.
    upload_all(&mut o.g, 2);
    o.rolled.clear();
    match ops::next_roll(&o.g.f, rel(2).0) {
        Roll::Next { intent, undo } => {
            assert_eq!(intent, site_day(0, 2));
            assert_eq!(undo, vec![rollback(0, 1)]);
        }
        r => panic!("{r:?}"),
    }
    o.roll_all(2);
    assert_eq!(o.rolled.len(), 7);
    assert_eq!(ops::next_roll(&o.g.f, rel(2).0), Roll::Done);
    // A pinned script's roll keeps N beside R: 1%, 10%, 50%, then R at 100% with N at 0%, so a pin to N still applies.
    let (e1, e2) = (version_of(1, Script::Edge), version_of(2, Script::Edge));
    let edge: Vec<_> = o.deployed.iter().filter(|(s, _)| *s == Script::Edge).map(|(_, x)| x.clone()).skip(4).collect();
    assert_eq!(
        edge,
        vec![vec![(e2, 1), (e1, 99)], vec![(e2, 10), (e1, 90)], vec![(e2, 50), (e1, 50)], vec![(e2, 100), (e1, 0)]]
    );
    // A site's 100% drops N: its undo is a Rollback, which deploys N again.
    assert_eq!(o.deps[&Script::Site(SiteId(1))].split, vec![(version_of(2, Script::Site(SiteId(1))), 100)]);
}

/// `FormatsEnable { f }` signed by the customer key, with the release-signing key's half over the format.
fn formats_enable(g: &G, f: Format) -> Vec<u8> {
    let sk = ed25519_dalek::SigningKey::from_bytes(&RELEASE);
    let half = Signature(ed25519_dalek::Signer::sign(&sk, &formats_enable_message(f)).to_bytes());
    let mut e = SignedEntry::decode(&g.entry(ReleaseEntry::FormatsEnable { format: f }, &[&CUSTOMER])).unwrap();
    e.sigs.push((public_key(&RELEASE), half));
    e.encode()
}

/// Runs `next` until the current intent is on its last forward step (the first day of a site day).
fn to_first_day(o: &mut Ops) {
    for _ in 0..100 {
        let c = o.g.f.current.as_deref().unwrap();
        if usize::from(c.next) == 6 {
            return;
        }
        o.act();
    }
    panic!("no first day");
}

/// BF-7: a site day to R = 3 with its pre-signed `Rollback` to N = 2. `FormatsEnable` for 32.3, which N cannot read,
/// commits only once every site, the spare included, reports R. A breach in the day gate then submits the undo: the
/// group refuses it and pages, and so it does a customer's `Abort { intent, target: N }`. Nothing moves; an `Abort` to
/// R commits and runs to `IntentDone`.
#[test]
fn bf7_after_formats_enable_the_rollback_and_the_abort_to_n_are_refused_and_page() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 2);
    o.roll_all(2);
    upload_all(&mut o.g, 3);
    let f = Format { family: 32, version: 3 };
    for _ in 0..3 {
        assert!(matches!(o.roll(3), Roll::Next { .. }));
        o.finish();
    }
    // Sites 0 to 2 report 3; the spare still reports 2, so 32.3 cannot be enabled yet (SHARED rule 3).
    assert_eq!(o.g.no(formats_enable(&o.g, f)), Refusal::NotReported { format: f, by: "site 3".into() });
    // The spare's day, to its first day: its SiteRelease commits at 100%, before undrain.
    match o.roll(3) {
        Roll::Next { intent, undo } => {
            assert_eq!(intent, site_day(3, 3));
            assert_eq!(undo, vec![rollback(3, 2)]);
        }
        r => panic!("{r:?}"),
    }
    to_first_day(&mut o);
    let b = formats_enable(&o.g, f);
    o.g.ok(b);
    // The day gate breaches: graft-ops submits the pre-signed Rollback to N, which cannot read 32.3.
    o.gate = Gate::Breach("GATE_APPLY_LAG".into());
    let Action::Undo(undo) = ops::next(&o.g.f, &o.world(), Dwell::default()) else { panic!("no undo on a breach") };
    assert_eq!(undo.body, rollback(3, 2));
    let want = Refusal::UnreadableFormat { release: rel(2).0, format: f };
    let r = o.g.no(undo.encode());
    assert_eq!(r, want);
    assert!(r.pages(), "the refused rollback pages");
    let intent = o.g.f.current.as_ref().unwrap().intent.id;
    let r = o.g.no(o.g.entry(ReleaseEntry::Abort { intent, target: rel(2).0 }, &[&CUSTOMER]));
    assert_eq!(r, want);
    assert!(r.pages(), "the refused Abort pages");
    assert!(crate::pages(r.variant()), "a member's answer names a paging refusal");
    // Neither applied: the spare still serves 3, its day still on the first day, nothing deployed since.
    let c = o.g.f.current.as_deref().unwrap();
    assert_eq!((c.state, c.next), (IntentState::Running { step: 6 }, 6));
    assert_eq!(o.deps[&Script::Site(SiteId(3))].split, vec![(version_of(3, Script::Site(SiteId(3))), 100)]);
    assert_eq!(o.g.f.site_release.get(&3), Some(&rel(3).0));
    // The customer names a newer target: an Abort to 3 commits, and graft-ops runs it to IntentDone with no API change.
    let b = o.g.entry(ReleaseEntry::Abort { intent, target: rel(3).0 }, &[&CUSTOMER]);
    o.g.ok(b);
    let before = o.deployed.len();
    o.gate = Gate::Pass;
    o.finish();
    assert_eq!(o.deployed.len(), before, "the spare already served 3");
    assert!(o.g.f.drained.is_empty());
}

/// BF-39's rule in memory: `graft-ops` killed after every action of a site day, then restarted with nothing but the
/// group and the deployments. Each restart may also resend what it last did, as a run whose answer was lost does:
/// a resent commit meets 409 (its `prev` is stale) and a resent deploy is already the deployment. Every step commits
/// once, in order, and one `IntentDone` ends the day. A restart never shortens a dwell.
#[test]
fn a_killed_graft_ops_reruns_only_the_first_unrecorded_step() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    assert!(matches!(o.roll(1), Roll::Next { .. }));
    let start = o.g.log.len();
    let mut deployed_at = Vec::new();
    for _ in 0..200 {
        if o.g.f.current.is_none() {
            break;
        }
        let before = o.g.log.len();
        // graft-ops was down for half an hour before each deploy: the dwell still runs from the deploy.
        if let Action::Deploy { .. } = ops::next(&o.g.f, &o.world(), Dwell::default()) {
            o.g.at = Timestamp(o.g.at.0 + HOUR / 2);
        }
        let a = o.act();
        // Killed here; the restart resends the last action first.
        match &a {
            Action::Commit { .. } | Action::Undo(_) => {
                let last = o.g.log[before].clone();
                assert!(matches!(o.g.no(last), Refusal::StalePrev(_) | Refusal::NotAnUndo(_) | Refusal::UndoUsed(_)));
            }
            Action::Deploy { script, split } => {
                deployed_at.push(o.g.at);
                let again = ops::next(&o.g.f, &o.world(), Dwell::default());
                assert!(!matches!(&again, Action::Deploy { script: s, split: x } if s == script && x == split));
            }
            _ => {}
        }
    }
    let mut steps = Vec::new();
    let mut done = 0;
    let mut step_at = Vec::new();
    for (b, at) in o.g.log[start..].iter().zip(&o.g.ats[start..]) {
        match SignedEntry::decode(b).unwrap().body {
            ReleaseEntry::IntentStep { step, .. } => {
                steps.push(step);
                step_at.push(*at);
            }
            ReleaseEntry::IntentDone { .. } => done += 1,
            _ => {}
        }
    }
    assert_eq!(steps, (0..7).collect::<Vec<u8>>(), "drain, 1%, 10%, 50%, 100%, undrain, first day, once each");
    assert_eq!(done, 1);
    // Steps 1 to 3 (1%, 10%, 50%) each commit at least STEP_DWELL after their deploy; the first day after undrain.
    for k in 0..3 {
        assert!(step_at[k + 1].0 >= deployed_at[k].0 + ops::STEP_DWELL, "step {} dwelt too little", k + 1);
    }
    assert!(step_at[6].0 >= step_at[5].0 + ops::FIRST_DAY);
}

#[test]
fn a_breach_runs_the_undo_and_graft_ops_returns_the_site_to_n() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    o.roll_all(1);
    upload_all(&mut o.g, 2);
    assert!(matches!(o.roll(2), Roll::Next { .. }));
    // Drain, 1%, then a breach at 10%.
    while o.g.f.current.as_deref().unwrap().next < 2 {
        o.act();
    }
    o.act(); // the 10% deploy
    o.gate = Gate::Breach("GATE_EXCEPTIONS".into());
    assert!(matches!(o.act(), Action::Undo(_)));
    assert_eq!(o.g.f.current.as_deref().unwrap().state, IntentState::Undoing);
    o.gate = Gate::Pass;
    o.finish();
    let s0 = Script::Site(SiteId(0));
    assert_eq!(o.deps[&s0].split, vec![(version_of(1, s0), 100)], "N at 100%");
    assert!(o.g.f.drained.is_empty(), "undrained after the rollback");
    assert_eq!(o.g.f.site_release.get(&0), Some(&rel(1).0));
    assert_eq!(ops::running(&o.g.f, s0), Some(rel(1).0));
    // release roll offers the same day again.
    assert!(matches!(ops::next_roll(&o.g.f, rel(2).0), Roll::Next { intent, .. } if intent == site_day(0, 2)));
}

#[test]
fn an_abort_of_an_edge_roll_returns_the_edge_to_target_with_the_roll_kept_beside_it() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    o.roll_all(1);
    upload_all(&mut o.g, 2);
    for _ in 0..5 {
        assert!(matches!(o.roll(2), Roll::Next { .. }));
        o.finish();
    }
    assert!(matches!(o.roll(2), Roll::Next { intent: Intent::EdgeRoll { .. }, .. }));
    while o.g.f.current.as_deref().unwrap().next < 2 {
        o.act();
    }
    let intent = o.g.f.current.as_ref().unwrap().intent.id;
    let b = o.g.entry(ReleaseEntry::Halt { intent }, &[&CUSTOMER]);
    o.g.ok(b);
    assert_eq!(ops::next(&o.g.f, &o.world(), Dwell::default()), Action::Idle, "a halted intent stops");
    let b = o.g.entry(ReleaseEntry::Abort { intent, target: rel(1).0 }, &[&CUSTOMER]);
    o.g.ok(b);
    o.finish();
    let (e1, e2) = (version_of(1, Script::Edge), version_of(2, Script::Edge));
    assert_eq!(o.deps[&Script::Edge].split, vec![(e1, 100), (e2, 0)]);
    assert_eq!(ops::running(&o.g.f, Script::Edge), Some(rel(1).0));
    assert_eq!(o.g.f.deployed[&Script::Edge], (Some(rel(2).0), rel(1).0));
    // An Abort to a third release: the edge runs the target after it, not the roll's release or the one before.
    upload_all(&mut o.g, 3);
    o.next_id += 1;
    let b = o.g.intent(o.next_id, Intent::EdgeRoll { release: rel(3).0 }, &[], &VENDOR);
    o.g.ok(b);
    o.act();
    let intent = o.g.f.current.as_ref().unwrap().intent.id;
    let b = o.g.entry(ReleaseEntry::Abort { intent, target: rel(2).0 }, &[&CUSTOMER]);
    o.g.ok(b);
    o.finish();
    assert_eq!(ops::running(&o.g.f, Script::Edge), Some(rel(2).0));
    assert_eq!(o.deps[&Script::Edge].split, vec![(e2, 100), (version_of(3, Script::Edge), 0)]);
}

/// BM-5's revocation mid-roll ("Install operations", Revocation runbook): release 2 is rolling under vendor-signed
/// intents when the customer signs `Halt`, then `Keys { vendor: None }`, then `Abort { intent, target: 1 }`, once with
/// site 1's day at 10% and once with the edge's roll at 10%. `graft-ops` stops at the halt, runs the abort under the
/// executor key with the vendor key gone, and a vendor-signed intent is refused. The sites that finished intents
/// moved return to 1 by `release roll 1` signed by the customer, so every site and pinned script runs 1.
#[test]
fn bm5_halt_keys_vendor_none_then_abort_mid_roll_returns_every_script_to_n() {
    for mid in [1usize, 5] {
        let mut o = Ops::new(G::new());
        upload_all(&mut o.g, 1);
        o.roll_all(1);
        upload_all(&mut o.g, 2);
        for _ in 0..mid {
            assert!(matches!(o.roll(2), Roll::Next { .. }));
            o.finish();
        }
        let Roll::Next { intent: rolling, .. } = o.roll(2) else { panic!("no intent {mid}") };
        let s = rolling.script().unwrap();
        let (v1, v2) = (version_of(1, s), version_of(2, s));
        while o.deps[&s].split != vec![(v2, 10), (v1, 90)] {
            o.act();
        }
        let intent = o.g.f.current.as_ref().unwrap().intent.id;
        let b = o.g.entry(ReleaseEntry::Halt { intent }, &[&CUSTOMER]);
        o.g.ok(b);
        let k = o.g.keys(|k| k.vendor = None);
        let b = o.g.entry(ReleaseEntry::Keys(Box::new(k)), &[&CUSTOMER]);
        o.g.ok(b);
        assert_eq!(ops::next(&o.g.f, &o.world(), Dwell::default()), Action::Idle, "{s:?}: halted, graft-ops stops");
        let b = o.g.entry(ReleaseEntry::Abort { intent, target: rel(1).0 }, &[&CUSTOMER]);
        o.g.ok(b);
        o.finish();
        assert_eq!(ops::running(&o.g.f, s), Some(rel(1).0), "{s:?}");
        let back = if s == Script::Edge { vec![(v1, 100), (v2, 0)] } else { vec![(v1, 100)] };
        assert_eq!(o.deps[&s].split, back, "{s:?}");
        // A vendor-signed intent, release roll's own included, is refused now.
        let Roll::Next { intent: i, undo } = ops::next_roll(&o.g.f, rel(1).0) else { panic!("nothing to return") };
        let vendor = ops::sign_intent(&o.g.f, i, &undo, &VENDOR, id(200), o.g.at, 4 * HOUR);
        assert_eq!(o.g.no(vendor.encode()), Refusal::UnknownKey(public_key(&VENDOR)), "{s:?}");
        // The customer signs release roll 1 for the scripts finished intents moved.
        let mut rolled = Vec::new();
        for n in 201..=220 {
            match ops::next_roll(&o.g.f, rel(1).0) {
                Roll::Next { intent, undo } => {
                    let e = ops::sign_intent(&o.g.f, intent.clone(), &undo, &CUSTOMER, id(n), o.g.at, 4 * HOUR);
                    o.g.ok(e.encode());
                    rolled.push(intent);
                    o.finish();
                }
                Roll::Done => break,
                Roll::Wait(why) => panic!("{why}"),
            }
        }
        let before: Vec<Intent> = o.rolled[o.rolled.len() - 1 - mid..o.rolled.len() - 1]
            .iter()
            .map(|i| match i {
                Intent::SiteDay { target, .. } => Intent::SiteDay { target: *target, release: rel(1).0 },
                i => panic!("{i:?}"),
            })
            .collect();
        assert_eq!(rolled, before, "{s:?}: only the finished intents' scripts roll back");
        for s in [&ALL[..], &[Script::Ops]].concat() {
            assert_eq!(ops::running(&o.g.f, s), Some(rel(1).0), "{s:?}");
        }
        assert!(o.g.f.drained.is_empty());
    }
}

/// A site an intent key drained is undrained only by an intent key: `graft-ops`'s site day skips its own drain and
/// pages at undrain, and never signs that `Normal` with the drain key.
#[test]
fn a_site_an_intent_key_drained_is_left_for_a_human_to_undrain() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    let b = o.g.drain(0, Mode::Drain, &CUSTOMER);
    o.g.ok(b);
    assert!(matches!(o.roll(1), Roll::Next { .. }));
    for _ in 0..50 {
        match ops::next(&o.g.f, &o.world(), Dwell::default()) {
            Action::Page(why) => {
                assert!(why.contains("intent key"), "{why}");
                assert_eq!(o.g.f.current.as_deref().unwrap().next, 5, "paged at undrain");
                assert!(o.g.f.drained.contains_key(&0));
                return;
            }
            Action::Commit { body: ReleaseEntry::SiteMode(_), .. } => panic!("graft-ops signed a SiteMode"),
            _ => {
                o.act();
            }
        }
    }
    panic!("no page");
}

/// SHARED rule 3 counts registered lifeboats: `FormatsEnable` waits for each one's `LifeboatDecoder` to name a release
/// that reads the format, and a removed lifeboat no longer counts.
#[test]
fn formats_enable_waits_for_every_registered_lifeboat() {
    let mut g = G::new();
    upload_all(&mut g, 3);
    for s in 0..4 {
        let b = g.entry(ReleaseEntry::SiteRelease { site: SiteId(s), release: rel(3).0 }, &[&EXECUTOR]);
        g.ok(b);
    }
    let lb = [11u8; 32];
    let b = g.entry(ReleaseEntry::LifeboatAdd { host: "lb.example".into(), key: public_key(&lb) }, &[&CUSTOMER]);
    g.ok(b);
    let f = Format { family: 32, version: 3 };
    let want = Refusal::NotReported { format: f, by: "lifeboat lb.example".into() };
    assert_eq!(g.no(formats_enable(&g, f)), want, "no decoder reported");
    upload_all(&mut g, 2);
    let b = g.entry(ReleaseEntry::LifeboatDecoder { host: "lb.example".into(), release: rel(2).0 }, &[&lb]);
    g.ok(b);
    assert_eq!(g.no(formats_enable(&g, f)), want, "its decoder's release cannot read 32.3");
    let b = g.entry(ReleaseEntry::LifeboatDecoder { host: "lb.example".into(), release: rel(3).0 }, &[&lb]);
    g.ok(b);
    let b = formats_enable(&g, f);
    g.ok(b);
    let b = g.entry(ReleaseEntry::LifeboatRemove { host: "lb.example".into() }, &[&CUSTOMER]);
    g.ok(b);
    assert!(g.f.lifeboat_release.is_empty());
}

#[test]
fn a_front_roll_breach_pages_because_it_carries_no_undo() {
    let mut o = Ops::new(G::new());
    o.deps.insert(Script::Front, Deployment { split: vec![(OLD, 100)], created: T0 });
    let b = o.g.entry(G::upload_body(1, &[Script::Front]), &[&EXECUTOR]);
    o.g.ok(b);
    let b = o.g.intent(1, Intent::FrontRoll { release: rel(1).0 }, &[], &VENDOR);
    o.g.ok(b);
    o.act();
    o.gate = Gate::Breach("mint errors".into());
    assert!(matches!(ops::next(&o.g.f, &o.world(), Dwell::default()), Action::Page(_)));
}

/// A bundle in the release bucket: `formats.txt`, the site, edge, and ops modules, and `release.json` listing each
/// with its SHA-256, signed by `seed`.
type Files = Vec<(String, Vec<u8>)>;

fn bundle(seed: &[u8; 32]) -> (Vec<u8>, Signature, Files) {
    let files: Files = vec![
        ("formats.txt".into(), reads(4).file().into_bytes()),
        ("site/index.js".into(), b"site".to_vec()),
        ("site/index.wasm".into(), b"\0asm site".to_vec()),
        ("edge/index.js".into(), b"edge".to_vec()),
        ("ops/index.js".into(), b"ops".to_vec()),
    ];
    let listed: Vec<String> = files
        .iter()
        .map(|(n, b)| format!(r#"{{"name":"{n}","sha256":"{}"}}"#, crate::types::hex(&Sha256::digest(b))))
        .collect();
    let rec = format!(r#"{{"label":"r4","files":[{}]}}"#, listed.join(",")).into_bytes();
    let sig = Signature(ed25519_dalek::Signer::sign(&ed25519_dalek::SigningKey::from_bytes(seed), &rec).to_bytes());
    (rec, sig, files)
}

#[test]
fn graft_ops_uploads_only_a_verified_bundle_and_each_script_once() {
    let mut g = G::new();
    let scripts = [Script::Edge, Script::Ops];
    let (rec, sig, files) = bundle(&RELEASE);
    // Signed by another key, a file the record does not list, and a listed file missing: each refused.
    let (orec, osig, _) = bundle(&OTHER);
    assert!(matches!(ops::check_bundle(&g.f, &orec, osig, &files, &scripts), Err(Refusal::BadRecord(_))));
    let mut extra = files.clone();
    extra.push(("edge/evil.js".into(), b"x".to_vec()));
    assert!(matches!(ops::check_bundle(&g.f, &rec, sig, &extra, &scripts), Err(Refusal::BadRecord(_))));
    let mut changed = files.clone();
    changed[3].1 = b"edge, changed".to_vec();
    assert!(matches!(ops::check_bundle(&g.f, &rec, sig, &changed, &scripts), Err(Refusal::BadRecord(_))));
    assert!(matches!(ops::check_bundle(&g.f, &rec, sig, &files[..4], &scripts), Err(Refusal::BadRecord(_))));
    let c = ops::check_bundle(&g.f, &rec, sig, &files, &scripts).unwrap();
    // Every genesis site (the spare included) uploads the one site bundle; the edge and graft-ops their own.
    let want: Vec<Script> = (0..4).map(|s| Script::Site(SiteId(s))).chain([Script::Edge, Script::Ops]).collect();
    assert_eq!(c.scripts.keys().copied().collect::<Vec<_>>(), want);
    assert_eq!(
        c.scripts[&Script::Site(SiteId(2))].iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["index.js", "index.wasm"]
    );
    // Uploads, killed after each: the versions read back by tag are never uploaded again.
    let mut tagged = BTreeMap::new();
    let mut uploads = Vec::new();
    let entry = loop {
        match ops::upload_next(&g.f, &c, &tagged) {
            UploadStep::Upload(s) => {
                assert!(!uploads.contains(&s), "{s:?} uploaded twice");
                uploads.push(s);
                tagged.insert(s, vid(4, uploads.len() as u8));
            }
            UploadStep::Commit(u) => break u,
            UploadStep::Done => panic!("done before the commit"),
        }
    };
    assert_eq!(uploads, want);
    let b = g.entry(ReleaseEntry::ReleaseUploaded(entry), &[&EXECUTOR]);
    g.ok(b);
    assert_eq!(ops::upload_next(&g.f, &c, &tagged), UploadStep::Done);
    let up = &g.f.uploads[&c.release.0];
    assert_eq!(up.files.len(), 5);
    assert!(up.reads.contains(&Format { family: 32, version: 4 }));
}

#[test]
fn release_roll_skips_a_script_the_release_leaves_unchanged() {
    let mut o = Ops::new(G::new());
    let (rec, sig, files) = bundle(&RELEASE);
    let c = ops::check_bundle(&o.g.f, &rec, sig, &files, &[Script::Edge, Script::Ops]).unwrap();
    let versions: BTreeMap<Script, VersionId> =
        c.scripts.keys().enumerate().map(|(i, s)| (*s, vid(4, i as u8))).collect();
    let UploadStep::Commit(u) = ops::upload_next(&o.g.f, &c, &versions) else { panic!() };
    let b = o.g.entry(ReleaseEntry::ReleaseUploaded(u), &[&EXECUTOR]);
    o.g.ok(b);
    let r4 = c.release;
    // Release 5 changes only the edge's files.
    let mut files5 = files.clone();
    files5[3].1 = b"edge 5".to_vec();
    let listed: Vec<String> = files5
        .iter()
        .map(|(n, b)| format!(r#"{{"name":"{n}","sha256":"{}"}}"#, crate::types::hex(&Sha256::digest(b))))
        .collect();
    let rec5 = format!(r#"{{"label":"r5","files":[{}]}}"#, listed.join(",")).into_bytes();
    let sig5 = sign_record(&RELEASE, &rec5);
    let c5 = ops::check_bundle(&o.g.f, &rec5, sig5, &files5, &[Script::Edge, Script::Ops]).unwrap();
    let v5: BTreeMap<Script, VersionId> = c5.scripts.keys().enumerate().map(|(i, s)| (*s, vid(5, i as u8))).collect();
    let UploadStep::Commit(u) = ops::upload_next(&o.g.f, &c5, &v5) else { panic!() };
    let b = o.g.entry(ReleaseEntry::ReleaseUploaded(u), &[&EXECUTOR]);
    o.g.ok(b);
    // Roll 4 everywhere, faking the roll's steps through `next`.
    let mut n = 0;
    loop {
        match ops::next_roll(&o.g.f, r4) {
            Roll::Next { intent, undo } => {
                n += 1;
                let e = ops::sign_intent(&o.g.f, intent, &undo, &VENDOR, id(200 + n), o.g.at, 4 * HOUR);
                o.g.ok(e.encode());
                o.finish();
            }
            Roll::Done => break,
            Roll::Wait(w) => panic!("{w}"),
        }
    }
    assert_eq!(n, 6, "four site days, the edge, and graft-ops");
    match ops::next_roll(&o.g.f, c5.release) {
        Roll::Next { intent, undo } => {
            assert_eq!(intent, Intent::EdgeRoll { release: c5.release });
            assert_eq!(undo, vec![ReleaseEntry::EdgePointer { pin: r4 }]);
        }
        r => panic!("{r:?}"),
    }
}

/// The vault Worker is taken from a release folder only when `release.json` verifies under the release key and lists
/// every `vault/` file with its SHA-256; files outside `vault/` are not read, and a tampered folder is refused.
#[test]
fn the_vault_worker_comes_only_from_a_verified_release() {
    let key = crate::public_key(&RELEASE);
    let sha = |b: &[u8]| crate::types::hex(&<Sha256 as sha2::Digest>::digest(b));
    let files: Files = vec![
        ("vault/entry.js".into(), b"vault".to_vec()),
        ("vault/ab12-index_bg.wasm".into(), b"\0asm vault".to_vec()),
        ("edge/index.js".into(), b"edge".to_vec()),
    ];
    let record = |fs: &[(String, Vec<u8>)]| {
        let listed: Vec<String> =
            fs.iter().map(|(n, b)| format!(r#"{{"name":"{n}","sha256":"{}"}}"#, sha(b))).collect();
        format!(r#"{{"label":"v5","files":[{}]}}"#, listed.join(",")).into_bytes()
    };
    let sign = |seed: &[u8; 32], rec: &[u8]| crate::sign_record(seed, rec);
    let rec = record(&files);
    let a = ops::check_vault(&key, &rec, sign(&RELEASE, &rec), &files).unwrap();
    assert_eq!(a.release, ReleaseId(<Sha256 as sha2::Digest>::digest(&rec).into()));
    let names: Vec<&str> = a.modules.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["ab12-index_bg.wasm", "entry.js"], "only vault/'s modules, folder removed");
    // The edge's file is never read: a changed one outside vault/ does not matter here.
    let mut other = files.clone();
    other[2].1 = b"edge, changed".to_vec();
    assert!(ops::check_vault(&key, &rec, sign(&RELEASE, &rec), &other).is_ok());
    let refused = |rec: &[u8], sig, fs: &[(String, Vec<u8>)], why: &str| match ops::check_vault(&key, rec, sig, fs) {
        Err(Refusal::BadRecord(m)) => assert!(m.contains(why), "{m}"),
        r => panic!("{why}: {r:?}"),
    };
    refused(&rec, sign(&[9; 32], &rec), &files, "does not verify");
    let mut bad = rec.clone();
    let n = bad.len() - 3;
    bad[n] ^= 1;
    refused(&bad, sign(&RELEASE, &rec), &files, "does not verify");
    let mut changed = files.clone();
    changed[1].1 = b"\0asm evil".to_vec();
    refused(&rec, sign(&RELEASE, &rec), &changed, "lists no vault/ab12-index_bg.wasm");
    let mut extra = files.clone();
    extra.push(("vault/evil.js".into(), b"x".to_vec()));
    refused(&rec, sign(&RELEASE, &rec), &extra, "lists no vault/evil.js");
    refused(&rec, sign(&RELEASE, &rec), &files[1..], "holds no vault/entry.js");
    let wasm_only = record(&files[1..]);
    refused(&wasm_only, sign(&RELEASE, &wasm_only), &files[1..], "no vault/entry.js");
}

/// BF-47's order (\"Install operations\", Release roll, step 4): the vault step is due once every code intent through
/// `McpRoll` is done or skipped for R, before R's `ContainerDay`s and whether or not they ran, and only for a release
/// that carries a vault Worker. It is never a group intent.
#[test]
fn bf47_the_vault_step_follows_the_code_intents_and_does_not_wait_for_container_days() {
    let mut o = Ops::new(G::new());
    // Every site's shards on release 1's images (none), so release 4's image changes them.
    upload_all(&mut o.g, 1);
    o.roll_all(1);
    for site in 0..4 {
        o.next_id += 1;
        let b = o.g.intent(o.next_id, Intent::ContainerDay { site: SiteId(site), release: rel(1).0 }, &[], &VENDOR);
        o.g.ok(b);
        o.finish();
    }
    let sha = |b: &[u8]| crate::types::hex(&Sha256::digest(b));
    // Release 4 lists the site's, the edge's, and graft-ops's modules, a vault Worker, and one container image.
    let mut files = bundle(&RELEASE).2;
    files.push(("vault/entry.js".into(), b"vault".to_vec()));
    let listed: Vec<String> = files.iter().map(|(n, b)| format!(r#"{{"name":"{n}","sha256":"{}"}}"#, sha(b))).collect();
    let img = format!(r#"{{"name":"git","digest":"sha256:{}"}}"#, "ab".repeat(32));
    let rec = format!(r#"{{"label":"r4v","files":[{}],"images":[{img}]}}"#, listed.join(",")).into_bytes();
    let c = ops::check_bundle(&o.g.f, &rec, sign_record(&RELEASE, &rec), &files, &[Script::Edge, Script::Ops]).unwrap();
    assert!(!c.scripts.keys().any(|s| crate::ops::folder(*s) == ops::VAULT_FOLDER), "no script deploys vault/");
    let r = c.release;
    assert!(!ops::vault_due(&o.g.f, r), "no ReleaseUploaded yet");
    let tagged: BTreeMap<Script, VersionId> =
        c.scripts.keys().enumerate().map(|(i, s)| (*s, vid(4, i as u8))).collect();
    let UploadStep::Commit(u) = ops::upload_next(&o.g.f, &c, &tagged) else { panic!() };
    let b = o.g.entry(ReleaseEntry::ReleaseUploaded(u), &[&EXECUTOR]);
    o.g.ok(b);
    let mut order = Vec::new();
    loop {
        let due = ops::vault_due(&o.g.f, r);
        match ops::next_roll(&o.g.f, r) {
            Roll::Next { intent, undo } => {
                let code = !matches!(intent, Intent::ContainerDay { .. });
                assert_eq!(due, !code, "vault due before {intent:?}: {due}");
                if due && !order.contains(&"vault") {
                    order.push("vault");
                }
                order.push(intent.kind());
                o.next_id += 1;
                let e = ops::sign_intent(&o.g.f, intent, &undo, &VENDOR, id(o.next_id), o.g.at, 4 * HOUR);
                o.g.ok(e.encode());
                o.finish();
            }
            Roll::Done => {
                assert!(due, "still due once the roll is done: release roll runs it then if it had not");
                break;
            }
            Roll::Wait(w) => panic!("{w}"),
        }
    }
    assert_eq!(
        order,
        [
            "SiteDay",
            "SiteDay",
            "SiteDay",
            "SiteDay",
            "EdgeRoll",
            "OpsRoll",
            "vault",
            "ContainerDay",
            "ContainerDay",
            "ContainerDay",
            "ContainerDay"
        ],
        "the vault step after the last code intent and before the ContainerDays"
    );
    // A release with no vault Worker owes no vault step.
    upload_all(&mut o.g, 5);
    o.roll_all(5);
    assert!(!ops::vault_due(&o.g.f, rel(5).0));
    // Mid-roll, with a code intent current, the vault step is not due.
    upload_all(&mut o.g, 6);
    let mut files6 = files.clone();
    files6[1].1 = b"site 6".to_vec();
    let listed6: Vec<String> =
        files6.iter().map(|(n, b)| format!(r#"{{"name":"{n}","sha256":"{}"}}"#, sha(b))).collect();
    let rec6 = format!(r#"{{"label":"r6v","files":[{}]}}"#, listed6.join(",")).into_bytes();
    let c6 =
        ops::check_bundle(&o.g.f, &rec6, sign_record(&RELEASE, &rec6), &files6, &[Script::Edge, Script::Ops]).unwrap();
    let t6: BTreeMap<Script, VersionId> = c6.scripts.keys().enumerate().map(|(i, s)| (*s, vid(7, i as u8))).collect();
    let UploadStep::Commit(u) = ops::upload_next(&o.g.f, &c6, &t6) else { panic!() };
    let b = o.g.entry(ReleaseEntry::ReleaseUploaded(u), &[&EXECUTOR]);
    o.g.ok(b);
    assert!(matches!(ops::next_roll(&o.g.f, c6.release), Roll::Next { intent: Intent::SiteDay { .. }, .. }));
    assert!(!ops::vault_due(&o.g.f, c6.release), "a site day is owed first");
}
