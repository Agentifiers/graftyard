//! The group's rules, one refusal per test, and BF-6's replay. Each test drives `ReleaseFold::decide` and `apply` as
//! the member does: decide in an entry stamped `at`, then apply what it accepted.

use graftyard_types::{RepoId, Timestamp, Ulid};
use sha2::Sha256;

use super::*;
use crate::group::{presign_undo, undo_entry};

const VENDOR: [u8; 32] = [2; 32];
const RELEASE: [u8; 32] = [3; 32];
const DRAIN: [u8; 32] = [4; 32];
const EXECUTOR: [u8; 32] = [5; 32];
const AGENTS: [u8; 32] = [6; 32];
const NEW_CUSTOMER: [u8; 32] = [9; 32];
const HOUR: u64 = 3_600_000;

/// A group with its anchor and the time of the next entry, which each step advances by a minute.
struct G {
    a: Anchor,
    f: ReleaseFold,
    at: Timestamp,
    /// Every accepted entry's bytes, in order: the history BF-6 replays.
    log: Vec<Vec<u8>>,
}

fn rel(n: u8) -> (ReleaseId, Vec<u8>) {
    let rec = format!(
        r#"{{"label":"r{n}","files":[{{"name":"formats.txt","sha256":"{}"}}]}}"#,
        crate::types::hex(&Sha256::digest(reads(n).file().as_bytes()))
    );
    (ReleaseId(Sha256::digest(rec.as_bytes()).into()), rec.into_bytes())
}

/// Release n reads formats 32.1 to 32.n.
fn reads(n: u8) -> FormatSet {
    FormatSet::new((1..=u16::from(n)).map(|v| Format { family: 32, version: v }))
}

fn vid(n: u8, s: u8) -> VersionId {
    VersionId([n, s, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])
}

fn id(n: u8) -> IntentId {
    IntentId(Ulid(u128::from(n) + 1000))
}

impl G {
    fn new() -> G {
        let mut g = G { a: anchor(), f: ReleaseFold::default(), at: T0, log: Vec::new() };
        let mut gs = genesis_for(install(1), &CUSTOMER);
        gs.keys.agents_executor = Some(public_key(&AGENTS));
        g.ok(signed(gs, &CUSTOMER));
        g
    }

    fn try_(&mut self, bytes: &[u8]) -> Result<(), Refusal> {
        self.at = Timestamp(self.at.0 + 60_000);
        let ev = self.f.decide(&self.a, self.at, bytes)?;
        // A follower verifies the same event on the same fold at the same time.
        self.f.check(&self.a, self.at, &ev).unwrap();
        self.f.apply(&ev, self.at);
        self.log.push(bytes.to_vec());
        Ok(())
    }

    fn ok(&mut self, bytes: Vec<u8>) {
        if let Err(e) = self.try_(&bytes) {
            panic!("refused {:?}: {e}", SignedEntry::decode(&bytes).map(|e| e.body.kind()));
        }
    }

    fn no(&mut self, bytes: Vec<u8>) -> Refusal {
        let before = self.f.clone();
        let e = self.try_(&bytes).expect_err("accepted");
        assert_eq!(self.f, before, "a refusal changes nothing");
        e
    }

    /// `body` bound to the current digest and signed by `seeds`.
    fn entry(&self, body: ReleaseEntry, seeds: &[&[u8; 32]]) -> Vec<u8> {
        let b = Bound { install: install(1), group: ChainId::Release, prev: self.f.last, parent: None };
        let mut e = SignedEntry::unsigned(b, body);
        for s in seeds {
            e.sign(s);
        }
        e.encode()
    }

    fn upload_body(n: u8, scripts: &[Script]) -> ReleaseEntry {
        let (release, record) = rel(n);
        let sk = ed25519_dalek::SigningKey::from_bytes(&RELEASE);
        let record_sig = Signature(ed25519_dalek::Signer::sign(&sk, &record).to_bytes());
        ReleaseEntry::ReleaseUploaded(Box::new(ReleaseUploaded {
            release,
            record: record.into(),
            record_sig,
            versions: scripts.iter().enumerate().map(|(i, s)| (*s, vid(n, i as u8))).collect(),
            images: SmallVec::new(),
            reads: reads(n),
        }))
    }

    fn upload(&mut self, n: u8) {
        let all =
            [Script::Site(SiteId(0)), Script::Site(SiteId(1)), Script::Site(SiteId(2)), Script::Edge, Script::Ops];
        let b = self.entry(G::upload_body(n, &all), &[&EXECUTOR]);
        self.ok(b);
    }

    fn intent_body(&self, n: u8, intent: Intent, undo: &[ReleaseEntry], signer: &[u8; 32], ttl: u64) -> ReleaseEntry {
        let undo = undo.iter().map(|u| (u.clone(), presign_undo(install(1), id(n), u, signer))).collect();
        ReleaseEntry::Intent(Box::new(SignedIntent {
            id: id(n),
            issued_at: self.at,
            not_after: Timestamp(self.at.0 + ttl),
            intent,
            undo,
        }))
    }

    fn intent(&self, n: u8, intent: Intent, undo: &[ReleaseEntry], signer: &[u8; 32]) -> Vec<u8> {
        self.entry(self.intent_body(n, intent, undo, signer, 4 * HOUR), &[signer])
    }

    fn undo(&self, n: u8, u: &ReleaseEntry, signer: &[u8; 32]) -> Vec<u8> {
        undo_entry(install(1), id(n), u, public_key(signer), presign_undo(install(1), id(n), u, signer)).encode()
    }

    fn step(&self, n: u8, step: u8) -> Vec<u8> {
        self.entry(ReleaseEntry::IntentStep { intent: id(n), step }, &[&EXECUTOR])
    }

    fn done(&self, n: u8) -> Vec<u8> {
        self.entry(ReleaseEntry::IntentDone { intent: id(n) }, &[&EXECUTOR])
    }

    fn keys(&self, f: impl FnOnce(&mut Keys)) -> Keys {
        let mut k = self.f.keys.clone().unwrap();
        k.generation += 1;
        f(&mut k);
        k
    }

    fn drain(&self, site: u8, mode: Mode, seed: &[u8; 32]) -> Vec<u8> {
        self.entry(ReleaseEntry::SiteMode(SiteMode { site: SiteId(site), mode }), &[seed])
    }
}

fn day(site: u8, n: u8) -> Intent {
    Intent::SiteDay { target: DayTarget::Site(SiteId(site)), release: rel(n).0 }
}

fn rollback(site: u8, n: u8) -> ReleaseEntry {
    ReleaseEntry::Rollback { target: DayTarget::Site(SiteId(site)), release: rel(n).0 }
}

/// A group with releases 1 and 2 uploaded and the edge on release 1.
fn rolled() -> G {
    let mut g = G::new();
    g.upload(1);
    g.upload(2);
    let b = g.intent(1, Intent::EdgeRoll { release: rel(1).0 }, &[], &VENDOR);
    g.ok(b);
    for s in 0..4 {
        let b = g.step(1, s);
        g.ok(b);
    }
    let b = g.done(1);
    g.ok(b);
    g
}

// ---- Keys, generations, signers ----

#[test]
fn keys_take_the_next_generation_and_the_customer_key_old_and_new() {
    let mut g = G::new();
    let skip = g.keys(|k| k.generation += 1);
    assert_eq!(
        g.no(g.entry(ReleaseEntry::Keys(Box::new(skip)), &[&CUSTOMER])),
        Refusal::Generation { want: 1, got: 2 }
    );
    let same = g.keys(|_| {});
    let mut zero = same.clone();
    zero.generation = 0;
    assert_eq!(
        g.no(g.entry(ReleaseEntry::Keys(Box::new(zero)), &[&CUSTOMER])),
        Refusal::Generation { want: 1, got: 0 }
    );
    // The vendor key cannot sign a Keys, and an unknown key is named as unknown.
    assert_eq!(g.no(g.entry(ReleaseEntry::Keys(Box::new(same.clone())), &[&VENDOR])), Refusal::WrongSigner("Keys"));
    assert_eq!(
        g.no(g.entry(ReleaseEntry::Keys(Box::new(same.clone())), &[&OTHER])),
        Refusal::UnknownKey(public_key(&OTHER))
    );
    // A customer change needs both the old and the new key.
    let rotated = g.keys(|k| k.customer = public_key(&NEW_CUSTOMER));
    assert_eq!(
        g.no(g.entry(ReleaseEntry::Keys(Box::new(rotated.clone())), &[&CUSTOMER])),
        Refusal::WrongSigner("Keys")
    );
    assert_eq!(
        g.no(g.entry(ReleaseEntry::Keys(Box::new(rotated.clone())), &[&NEW_CUSTOMER])),
        Refusal::WrongSigner("Keys")
    );
    g.ok(g.entry(ReleaseEntry::Keys(Box::new(rotated)), &[&CUSTOMER, &NEW_CUSTOMER]));
    assert_eq!(g.f.keys.as_ref().unwrap().generation, 1);
    // The old customer key is now unknown.
    let next = g.keys(|_| {});
    assert_eq!(
        g.no(g.entry(ReleaseEntry::Keys(Box::new(next.clone())), &[&CUSTOMER])),
        Refusal::UnknownKey(public_key(&CUSTOMER))
    );
    g.ok(g.entry(ReleaseEntry::Keys(Box::new(next)), &[&NEW_CUSTOMER]));
    // Two roles on one key are refused.
    let dup = g.keys(|k| k.drain = k.executor);
    assert!(matches!(g.no(g.entry(ReleaseEntry::Keys(Box::new(dup)), &[&NEW_CUSTOMER])), Refusal::Rejected(_)));
}

#[test]
fn each_kind_takes_only_its_signers() {
    let mut g = G::new();
    g.upload(1);
    // The executor key signs no intent but ReplaceMember, and the drain key signs only SiteMode.
    for k in [&EXECUTOR, &DRAIN, &RELEASE, &AGENTS] {
        assert_eq!(g.no(g.intent(1, day(0, 1), &[], k)), Refusal::WrongSigner("Intent"));
    }
    assert_eq!(g.no(g.entry(ReleaseEntry::Halt { intent: id(1) }, &[&VENDOR])), Refusal::WrongSigner("Halt"));
    for body in [
        ReleaseEntry::SiteLost { site: SiteId(1) },
        ReleaseEntry::StaleMode { scope: Scope::Install, until: Timestamp(T0.0 + HOUR) },
        ReleaseEntry::RepoReset { repo: RepoId(DbId(1)), at: T0, reason: "x".into() },
        ReleaseEntry::LifeboatAdd { host: "lb".into(), key: public_key(&[11; 32]) },
    ] {
        let kind = body.kind();
        assert_eq!(g.no(g.entry(body.clone(), &[&VENDOR])), Refusal::WrongSigner(kind), "{kind}");
        assert_eq!(g.no(g.entry(body.clone(), &[&EXECUTOR])), Refusal::WrongSigner(kind), "{kind}");
    }
    // ReleaseUploaded takes the executor key; the agents executor key only for AgentsFront and Controller alone.
    let edge = G::upload_body(2, &[Script::Edge]);
    for k in [&CUSTOMER, &VENDOR, &AGENTS, &DRAIN] {
        assert_eq!(g.no(g.entry(edge.clone(), &[k])), Refusal::WrongSigner("ReleaseUploaded"));
    }
    let agents = G::upload_body(2, &[Script::AgentsFront, Script::Controller]);
    g.ok(g.entry(agents, &[&AGENTS]));
    // An entry with no signature, or a signature that does not verify.
    assert_eq!(g.no(g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[])), Refusal::WrongSigner("SiteLost"));
    let mut forged = SignedEntry::decode(&g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[&CUSTOMER])).unwrap();
    forged.body = ReleaseEntry::SiteLost { site: SiteId(2) };
    assert_eq!(g.no(forged.encode()), Refusal::BadSignature);
}

#[test]
fn every_entry_binds_the_install_and_the_last_digest() {
    let mut g = G::new();
    let mut e = SignedEntry::decode(&g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[])).unwrap();
    e.bound.install = install(2);
    e.sign(&CUSTOMER);
    assert_eq!(g.no(e.encode()), Refusal::WrongInstall { want: install(1), got: install(2) });
    let mut e = SignedEntry::decode(&g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[])).unwrap();
    e.bound.group = ChainId::Ids;
    e.sign(&CUSTOMER);
    assert_eq!(g.no(e.encode()), Refusal::WrongGroup(ChainId::Ids));
    let stale = g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[&CUSTOMER]);
    let last = g.f.last.unwrap();
    g.upload(1);
    assert_eq!(g.no(stale), Refusal::StalePrev(g.f.last.unwrap()));
    assert_ne!(g.f.last.unwrap(), last);
    assert_eq!(Refusal::StalePrev(last).status(), 409);
    // No prev at all, and a parent on a forward entry.
    let mut e = SignedEntry::decode(&g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[])).unwrap();
    e.bound.prev = None;
    e.sign(&CUSTOMER);
    assert_eq!(g.no(e.encode()), Refusal::StalePrev(g.f.last.unwrap()));
    let mut e = SignedEntry::decode(&g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[])).unwrap();
    e.bound.parent = Some(id(9));
    e.sign(&CUSTOMER);
    assert!(matches!(g.no(e.encode()), Refusal::BadBound(_)));
}

/// BF-42's pure half: an entry signed offline against digest d meets a 409 naming d' once `graft-ops` commits an
/// `IntentStep`; signed again over d' it commits.
#[test]
fn an_entry_signed_before_a_step_commits_meets_409_and_commits_re_signed() {
    let mut g = G::new();
    g.upload(1);
    let b = g.intent(1, day(0, 1), &[rollback(0, 1)], &VENDOR);
    g.ok(b);
    let offline = g.entry(ReleaseEntry::Halt { intent: id(1) }, &[&CUSTOMER]);
    let b = g.step(1, 0);
    g.ok(b);
    let now = g.f.last.unwrap();
    let e = g.no(offline.clone());
    assert_eq!((e.status(), e.current()), (409, Some(now)));
    let mut again = SignedEntry::decode(&offline).unwrap();
    again.bound.prev = e.current();
    again.sigs.clear();
    again.sign(&CUSTOMER);
    g.ok(again.encode());
    assert!(matches!(g.f.current.as_ref().unwrap().state, IntentState::Halted { step: 1 }));
}

// ---- Intents ----

#[test]
fn an_intent_past_not_after_is_refused() {
    let mut g = G::new();
    g.upload(1);
    let body = g.intent_body(1, day(0, 1), &[], &VENDOR, 30_000);
    assert!(matches!(g.no(g.entry(body, &[&VENDOR])), Refusal::Expired { .. }));
    let body = g.intent_body(1, day(0, 1), &[], &VENDOR, 60_000);
    g.ok(g.entry(body, &[&VENDOR]));
}

/// BF-40's pure half.
#[test]
fn a_second_intent_and_one_naming_a_release_with_no_upload_are_refused() {
    let mut g = G::new();
    g.upload(1);
    assert_eq!(g.no(g.intent(1, day(0, 2), &[], &VENDOR)), Refusal::NoUpload(rel(2).0));
    // An undo naming an unuploaded release is refused the same way.
    assert_eq!(g.no(g.intent(1, day(0, 1), &[rollback(0, 2)], &VENDOR)), Refusal::NoUpload(rel(2).0));
    let b = g.intent(1, day(0, 1), &[], &VENDOR);
    g.ok(b);
    g.upload(2);
    assert_eq!(g.no(g.intent(2, day(1, 2), &[], &CUSTOMER)), Refusal::IntentCurrent(id(1)));
    assert_eq!(g.no(g.intent(2, Intent::EdgeRoll { release: rel(2).0 }, &[], &VENDOR)), Refusal::IntentCurrent(id(1)));
    // After IntentDone a new intent commits, but never one with a used id.
    let b = g.done(1);
    g.ok(b);
    assert!(matches!(g.no(g.intent(1, day(1, 1), &[], &VENDOR)), Refusal::Rejected(_)));
    let b = g.intent(2, day(1, 2), &[], &VENDOR);
    g.ok(b);
}

#[test]
fn steps_run_in_order_and_stop_while_halted() {
    let mut g = G::new();
    g.upload(1);
    let b = g.intent(1, day(0, 1), &[], &VENDOR);
    g.ok(b);
    assert_eq!(g.no(g.step(1, 1)), Refusal::WrongStep { want: 0, got: 1 });
    assert_eq!(g.no(g.step(2, 0)), Refusal::NotCurrent(id(2)));
    let b = g.step(1, 0);
    g.ok(b);
    assert_eq!(g.no(g.step(1, 0)), Refusal::WrongStep { want: 1, got: 0 });
    g.ok(g.entry(ReleaseEntry::Halt { intent: id(1) }, &[&CUSTOMER]));
    assert!(matches!(g.no(g.step(1, 1)), Refusal::BadState(_)));
    assert!(matches!(g.no(g.done(1)), Refusal::BadState(_)));
    g.ok(g.entry(ReleaseEntry::Resume { intent: id(1), undo: Vec::new() }, &[&CUSTOMER]));
    let b = g.step(1, 1);
    g.ok(b);
}

// ---- Undos ----

#[test]
fn an_undo_commits_only_byte_for_byte_from_the_current_intents_list() {
    let mut g = G::new();
    g.upload(1);
    g.upload(2);
    let b = g.intent(1, day(0, 2), &[rollback(0, 1)], &VENDOR);
    g.ok(b);
    // Not in the list, signed by the right key.
    assert_eq!(g.no(g.undo(1, &rollback(1, 1), &VENDOR)), Refusal::NotAnUndo("Rollback"));
    // In the list but signed by another known key.
    assert_eq!(g.no(g.undo(1, &rollback(0, 1), &CUSTOMER)), Refusal::NotAnUndo("Rollback"));
    // Bound as a forward entry.
    assert!(matches!(g.no(g.entry(rollback(0, 1), &[&VENDOR])), Refusal::BadBound(_)));
    // For another intent.
    assert_eq!(g.no(g.undo(7, &rollback(0, 1), &VENDOR)), Refusal::NotCurrent(id(7)));
    let b = g.undo(1, &rollback(0, 1), &VENDOR);
    g.ok(b.clone());
    assert_eq!(g.f.current.as_ref().unwrap().state, IntentState::Undoing);
    assert_eq!(g.no(b), Refusal::UndoUsed("Rollback"));
    // The undo's steps, then done; the undo dies with its parent.
    let b = g.step(1, 0);
    g.ok(b);
    let b = g.done(1);
    g.ok(b);
    assert_eq!(g.no(g.undo(1, &rollback(0, 1), &VENDOR)), Refusal::NotCurrent(id(1)));
}

#[test]
fn a_pointer_undo_pins_the_version_and_the_next_roll_clears_it() {
    let mut g = rolled();
    let pin = ReleaseEntry::EdgePointer { pin: rel(1).0 };
    let b = g.intent(2, Intent::EdgeRoll { release: rel(2).0 }, std::slice::from_ref(&pin), &VENDOR);
    g.ok(b);
    let b = g.undo(2, &pin, &VENDOR);
    g.ok(b);
    assert_eq!(g.f.pointers.edge, Some(vid(1, 3)));
    let b = g.step(2, 0);
    g.ok(b);
    let b = g.done(2);
    g.ok(b);
    let b = g.intent(3, Intent::EdgeRoll { release: rel(2).0 }, &[], &VENDOR);
    g.ok(b);
    assert_eq!(g.f.pointers.edge, None);
}

#[test]
fn an_edge_roll_that_drops_an_undos_version_is_refused() {
    let mut g = rolled();
    g.upload(3);
    // The edge runs release 1; rolling to 3 keeps 1 and 3 in the deployment. An undo to 2 could never apply.
    let to2 = ReleaseEntry::EdgePointer { pin: rel(2).0 };
    assert_eq!(
        g.no(g.intent(2, Intent::EdgeRoll { release: rel(3).0 }, &[to2], &VENDOR)),
        Refusal::DropsUndoVersion(rel(2).0)
    );
    let to1 = ReleaseEntry::EdgePointer { pin: rel(1).0 };
    let b = g.intent(2, Intent::EdgeRoll { release: rel(3).0 }, &[to1], &VENDOR);
    g.ok(b);
}

#[test]
fn a_release_or_undo_that_cannot_read_an_enabled_format_is_refused() {
    let mut g = rolled();
    g.upload(3);
    let f = Format { family: 32, version: 3 };
    let sk = ed25519_dalek::SigningKey::from_bytes(&RELEASE);
    let half = Signature(ed25519_dalek::Signer::sign(&sk, &formats_enable_message(f)).to_bytes());
    // The customer half alone, and the release half alone, are refused.
    assert_eq!(
        g.no(g.entry(ReleaseEntry::FormatsEnable { format: f }, &[&CUSTOMER])),
        Refusal::WrongSigner("FormatsEnable")
    );
    let mut e = SignedEntry::decode(&g.entry(ReleaseEntry::FormatsEnable { format: f }, &[&CUSTOMER])).unwrap();
    e.sigs.push((public_key(&RELEASE), Signature([0; 64])));
    assert_eq!(g.no(e.encode()), Refusal::BadSignature);
    let mut e = SignedEntry::decode(&g.entry(ReleaseEntry::FormatsEnable { format: f }, &[&CUSTOMER])).unwrap();
    e.sigs.push((public_key(&RELEASE), half));
    g.ok(e.encode());
    let want = |r| Refusal::UnreadableFormat { release: rel(r).0, format: f };
    assert_eq!(g.no(g.intent(2, day(0, 2), &[], &VENDOR)), want(2));
    assert_eq!(g.no(g.intent(2, day(0, 3), &[rollback(0, 2)], &VENDOR)), want(2));
    assert_eq!(
        g.no(g.intent(
            2,
            Intent::EdgeRoll { release: rel(3).0 },
            &[ReleaseEntry::EdgePointer { pin: rel(1).0 }],
            &VENDOR
        )),
        want(1)
    );
    let b = g.intent(2, day(0, 3), &[], &VENDOR);
    g.ok(b);
    assert_eq!(g.no(g.entry(ReleaseEntry::Abort { intent: id(2), target: rel(2).0 }, &[&CUSTOMER])), want(2));
    g.ok(g.entry(ReleaseEntry::Abort { intent: id(2), target: rel(3).0 }, &[&CUSTOMER]));
}

// ---- Site modes ----

#[test]
fn the_drain_key_cannot_drain_a_second_site() {
    let mut g = G::new();
    let b = g.drain(0, Mode::Drain, &DRAIN);
    g.ok(b);
    assert_eq!(g.no(g.drain(1, Mode::Drain, &DRAIN)), Refusal::DrainGuard(SiteId(0)));
    // An intent key may, and a drain-key drain ends only by a drain-key Normal.
    let b = g.drain(1, Mode::Drain, &CUSTOMER);
    g.ok(b);
    assert_eq!(g.no(g.drain(0, Mode::Normal, &CUSTOMER)), Refusal::WrongSigner("SiteMode"));
    assert_eq!(g.no(g.drain(1, Mode::Normal, &DRAIN)), Refusal::WrongSigner("SiteMode"));
    assert_eq!(g.no(g.drain(0, Mode::Drain, &EXECUTOR)), Refusal::WrongSigner("SiteMode"));
    let b = g.drain(0, Mode::Normal, &DRAIN);
    g.ok(b);
    // Site 1 is still drained by an intent key, so the drain key still drains no other site.
    assert_eq!(g.no(g.drain(2, Mode::Drain, &DRAIN)), Refusal::DrainGuard(SiteId(1)));
}

/// BF-45's pure half: the executor key's forgeries.
#[test]
fn the_executor_key_replaces_only_a_lost_site_and_pins_nothing() {
    let mut g = rolled();
    let replace = Intent::ReplaceMember { lost: SiteId(1), spare: SiteId(3) };
    // An undrained, unlost site.
    assert_eq!(g.no(g.intent(2, replace.clone(), &[], &EXECUTOR)), Refusal::NotLost(SiteId(1)));
    // A pointer signed by the executor key, as an undo of nothing and as a forward entry.
    let pin = ReleaseEntry::EdgePointer { pin: rel(1).0 };
    assert!(matches!(g.no(g.entry(pin.clone(), &[&EXECUTOR])), Refusal::BadBound(_)));
    assert_eq!(g.no(g.undo(1, &pin, &EXECUTOR)), Refusal::NotCurrent(id(1)));
    // A forged ReleaseUploaded: a record the release key did not sign, and an id that is not the record's digest.
    let mut forged = G::upload_body(3, &[Script::Edge]);
    if let ReleaseEntry::ReleaseUploaded(u) = &mut forged {
        u.record_sig = Signature([7; 64]);
    }
    assert!(matches!(g.no(g.entry(forged, &[&EXECUTOR])), Refusal::BadRecord(_)));
    let mut forged = G::upload_body(3, &[Script::Edge]);
    if let ReleaseEntry::ReleaseUploaded(u) = &mut forged {
        u.reads = reads(4);
    }
    assert!(matches!(g.no(g.entry(forged, &[&EXECUTOR])), Refusal::BadRecord(_)));
    // Drained, but not yet for ZERO_ACKS.
    let b = g.drain(1, Mode::Drain, &DRAIN);
    g.ok(b);
    g.at = Timestamp(g.at.0 + ZERO_ACKS - 10 * 60_000);
    assert_eq!(g.no(g.intent(2, replace.clone(), &[], &EXECUTOR)), Refusal::NotLost(SiteId(1)));
    // An undo list is refused on an executor intent: the executor key pins nothing.
    g.at = Timestamp(g.at.0 + 10 * 60_000);
    assert!(matches!(g.no(g.intent(2, replace.clone(), &[rollback(1, 1)], &EXECUTOR)), Refusal::Rejected(_)));
    // Onto a voter site.
    let onto_voter = Intent::ReplaceMember { lost: SiteId(1), spare: SiteId(2) };
    assert!(matches!(g.no(g.intent(2, onto_voter, &[], &EXECUTOR)), Refusal::Rejected(_)));
    let b = g.intent(2, replace, &[], &EXECUTOR);
    g.ok(b);
    // A correctly signed ReleaseUploaded commits.
    g.upload(3);
}

#[test]
fn a_replace_member_halts_the_current_intent_which_resumes_after_it() {
    let mut g = G::new();
    g.upload(1);
    let b = g.intent(1, day(0, 1), &[rollback(0, 1)], &VENDOR);
    g.ok(b);
    g.ok(g.entry(ReleaseEntry::SiteLost { site: SiteId(2) }, &[&CUSTOMER]));
    let b = g.intent(2, Intent::ReplaceMember { lost: SiteId(2), spare: SiteId(3) }, &[], &EXECUTOR);
    g.ok(b);
    assert_eq!(g.f.under.as_ref().unwrap().state, IntentState::Halted { step: 0 });
    // A second ReplaceMember is refused while one runs.
    let b = g.intent(3, Intent::ReplaceMember { lost: SiteId(2), spare: SiteId(3) }, &[], &EXECUTOR);
    assert_eq!(g.no(b), Refusal::IntentCurrent(id(2)));
    // The halted intent's undo stays valid while it is halted under the replacement.
    let b = g.undo(1, &rollback(0, 1), &VENDOR);
    g.ok(b);
    let b = g.done(2);
    g.ok(b);
    assert_eq!(g.f.current.as_ref().unwrap().intent.id, id(1));
}

// ---- BF-6 ----

/// A history with every kind of entry an install collects, pre-signed undos and `McpPointer` and `ControllerPointer`
/// undos included, then the revocation runbook: `Halt`, `Keys { vendor: None }`. Returns the group and the entries
/// to replay, each as it was submitted.
fn history() -> (G, Vec<Vec<u8>>) {
    let mut g = G::new();
    let all = [
        Script::Site(SiteId(0)),
        Script::Site(SiteId(1)),
        Script::Site(SiteId(2)),
        Script::Workflows,
        Script::Front,
        Script::Edge,
        Script::Ops,
        Script::Mcp,
    ];
    for n in 1..=3 {
        let b = g.entry(G::upload_body(n, &all), &[&EXECUTOR]);
        g.ok(b);
    }
    let b = g.entry(G::upload_body(4, &[Script::AgentsFront, Script::Controller]), &[&AGENTS]);
    g.ok(b);
    // Every rolled script, each intent with its undos, some undos used, run to IntentDone. Vendor and customer both
    // sign intents.
    let mut n = 0;
    fn roll(
        g: &mut G,
        n: &mut u8,
        intent: Intent,
        undo: Vec<ReleaseEntry>,
        use_undo: bool,
        signer: &[u8; 32],
        steps: u8,
    ) {
        *n += 1;
        let b = g.intent(*n, intent, &undo, signer);
        g.ok(b);
        for s in 0..steps {
            let b = g.step(*n, s);
            g.ok(b);
        }
        if use_undo {
            for u in &undo {
                let b = g.undo(*n, u, signer);
                g.ok(b);
            }
        }
        let b = g.done(*n);
        g.ok(b);
    }
    for r in [1u8, 2] {
        let prev = r.max(2) - 1;
        for s in 0..3 {
            let b = g.drain(s, Mode::Drain, &DRAIN);
            g.ok(b);
            roll(&mut g, &mut n, day(s, r), vec![rollback(s, prev)], r == 2 && s == 1, &VENDOR, 2);
            let b = g.drain(s, Mode::Normal, &DRAIN);
            g.ok(b);
            let b = g.entry(ReleaseEntry::SiteRelease { site: SiteId(s), release: rel(r).0 }, &[&EXECUTOR]);
            g.ok(b);
        }
        roll(
            &mut g,
            &mut n,
            Intent::SiteDay { target: DayTarget::Workflows, release: rel(r).0 },
            vec![],
            false,
            &VENDOR,
            4,
        );
        let p = rel(prev).0;
        roll(
            &mut g,
            &mut n,
            Intent::EdgeRoll { release: rel(r).0 },
            vec![ReleaseEntry::EdgePointer { pin: p }],
            r == 2,
            &VENDOR,
            4,
        );
        roll(
            &mut g,
            &mut n,
            Intent::OpsRoll { release: rel(r).0 },
            vec![ReleaseEntry::OpsPointer { pin: p }],
            r == 2,
            &CUSTOMER,
            2,
        );
        roll(&mut g, &mut n, Intent::FrontRoll { release: rel(r).0 }, vec![], false, &VENDOR, 2);
        roll(
            &mut g,
            &mut n,
            Intent::McpRoll { release: rel(r).0 },
            vec![ReleaseEntry::McpPointer { pin: p }],
            r == 2,
            &VENDOR,
            2,
        );
        let containers = ReleaseEntry::Rollback { target: DayTarget::Containers(SiteId(0)), release: p };
        roll(
            &mut g,
            &mut n,
            Intent::ContainerDay { site: SiteId(0), release: rel(r).0 },
            vec![containers],
            false,
            &VENDOR,
            3,
        );
    }
    roll(&mut g, &mut n, Intent::ControllerRoll { release: rel(4).0 }, vec![], false, &VENDOR, 2);
    let undo = vec![ReleaseEntry::ControllerPointer { pin: rel(4).0 }];
    roll(&mut g, &mut n, Intent::ControllerRoll { release: rel(4).0 }, undo, true, &VENDOR, 1);
    roll(&mut g, &mut n, Intent::Rotate { secret: SecretName::DeployToken, slot: 1 }, vec![], false, &VENDOR, 1);
    roll(&mut g, &mut n, Intent::RestorePlacement { site: SiteId(1) }, vec![], false, &CUSTOMER, 1);
    let customer: &[&[u8; 32]] = &[&CUSTOMER];
    for body in [
        ReleaseEntry::StaleMode { scope: Scope::Install, until: Timestamp(g.at.0 + 4 * HOUR) },
        ReleaseEntry::LifeboatAdd { host: "lb.example".into(), key: public_key(&[11; 32]) },
        ReleaseEntry::RepoRestore {
            repos: vec![RepoId(DbId(5))],
            refs: vec!["refs/heads/main".into()],
            at: T0,
            reason: "r".into(),
        },
        ReleaseEntry::RepoReset { repo: RepoId(DbId(5)), at: T0, reason: "r".into() },
    ] {
        let b = g.entry(body, customer);
        g.ok(b);
    }
    let b = g.entry(ReleaseEntry::LifeboatDecoder { host: "lb.example".into(), release: rel(3).0 }, &[&[11; 32]]);
    g.ok(b);
    let k = g.keys(|_| {});
    let b = g.entry(ReleaseEntry::Keys(Box::new(k)), customer);
    g.ok(b);
    let b = g.entry(ReleaseEntry::SiteLost { site: SiteId(2) }, customer);
    g.ok(b);
    roll(&mut g, &mut n, Intent::ReplaceMember { lost: SiteId(2), spare: SiteId(3) }, vec![], false, &EXECUTOR, 2);
    let b = g.entry(ReleaseEntry::LifeboatRemove { host: "lb.example".into() }, customer);
    g.ok(b);
    // The intent the revocation runbook halts, with its undos, a pointer among them, then Keys { vendor: None }.
    n += 1;
    let live = vec![rollback(0, 2), ReleaseEntry::EdgePointer { pin: rel(2).0 }];
    let b = g.intent(n, day(0, 3), &live, &VENDOR);
    g.ok(b);
    let b = g.step(n, 0);
    g.ok(b);
    let b = g.entry(ReleaseEntry::Halt { intent: id(n) }, customer);
    g.ok(b);
    let k = g.keys(|k| k.vendor = None);
    let b = g.entry(ReleaseEntry::Keys(Box::new(k)), customer);
    g.ok(b);
    let submitted = g.log[1..].to_vec();
    (g, submitted)
}

/// BF-6: every committed entry, as submitted, and every pre-signed undo of every finished intent, replayed after
/// `Keys { vendor: None }`: 100 of 100 refused (the history holds more; every one is refused). The halted intent's
/// own unused undos are not historical entries: the revocation runbook keeps them valid under the `Keys` of the
/// intent's index, which `the_halted_intents_undos_survive_keys_vendor_none` checks.
#[test]
fn bf6_every_historical_entry_is_refused_after_keys_vendor_none() {
    let (g, submitted) = history();
    let halted = g.f.current.as_ref().unwrap().intent.id;
    let mut replay = vec![g.log[0].clone()];
    replay.extend(submitted.iter().cloned());
    let mut kinds = std::collections::BTreeSet::new();
    for b in &submitted {
        let e = SignedEntry::decode(b).unwrap();
        kinds.insert(e.body.kind());
        if let ReleaseEntry::Intent(i) = &e.body
            && i.id != halted
        {
            for (u, sig) in &i.undo {
                kinds.insert(u.kind());
                replay.push(undo_entry(install(1), i.id, u, e.sigs[0].0, *sig).encode());
            }
        }
    }
    for k in [
        "Keys",
        "Intent",
        "SiteMode",
        "ReleaseUploaded",
        "IntentStep",
        "IntentDone",
        "SiteRelease",
        "Rollback",
        "EdgePointer",
        "OpsPointer",
        "McpPointer",
        "ControllerPointer",
        "SiteLost",
        "StaleMode",
        "LifeboatAdd",
        "LifeboatRemove",
        "LifeboatDecoder",
        "RepoRestore",
        "RepoReset",
        "Halt",
    ] {
        assert!(kinds.contains(k), "the history has no {k}");
    }
    assert!(replay.len() >= 100, "only {} entries to replay", replay.len());
    let at = Timestamp(g.at.0 + 60_000);
    let mut why = std::collections::BTreeMap::<&str, usize>::new();
    for b in &replay {
        match g.f.decide(&g.a, at, b) {
            Ok(_) => panic!("replayed {} accepted", SignedEntry::decode(b).unwrap().body.kind()),
            Err(e) => *why.entry(e.variant()).or_default() += 1,
        }
    }
    assert_eq!(why.values().sum::<usize>(), replay.len());
    println!("BF-6: {} of {} refused: {why:?}", replay.len(), replay.len());
    // The stronger case: a leaked vendor key re-signs each vendor entry over the current digest, and each vendor
    // undo over its intent. Every one is refused for its key, not its position.
    let mut fresh = 0;
    for b in &submitted {
        let e = SignedEntry::decode(b).unwrap();
        if e.sigs[0].0 != public_key(&VENDOR) || e.bound.parent.is_some() {
            continue;
        }
        let mut again = e.clone();
        again.bound.prev = g.f.last;
        again.sigs.clear();
        again.sign(&VENDOR);
        assert_eq!(g.f.decide(&g.a, at, &again.encode()), Err(Refusal::UnknownKey(public_key(&VENDOR))));
        fresh += 1;
    }
    assert!(fresh >= 20, "{fresh}");
    println!("BF-6: {fresh} vendor entries re-signed over the current digest, all refused UnknownKey");
}

#[test]
fn the_halted_intents_undos_survive_keys_vendor_none() {
    let (mut g, _) = history();
    let c = g.f.current.as_ref().unwrap().clone();
    let (u, sig) = c.undo[0].clone();
    let b = undo_entry(install(1), c.intent.id, &u, c.undo_key, sig).encode();
    g.ok(b.clone());
    assert_eq!(g.no(b), Refusal::UndoUsed("Rollback"));
    // A vendor-signed intent is refused: the vendor key is gone.
    let b = g.intent(200, day(1, 3), &[], &VENDOR);
    assert!(matches!(g.no(b), Refusal::UnknownKey(_) | Refusal::IntentCurrent(_)));
}

#[test]
fn a_vendor_intent_resumes_after_keys_vendor_none_only_with_a_re_signed_undo() {
    let (mut g, _) = history();
    let id = g.f.current.as_ref().unwrap().intent.id;
    let empty = g.entry(ReleaseEntry::Resume { intent: id, undo: Vec::new() }, &[&CUSTOMER]);
    assert!(matches!(g.no(empty), Refusal::Rejected(_)));
    // An undo list still signed by the vendor key does not verify as the customer's.
    let u = rollback(0, 2);
    let vendor_sig = presign_undo(install(1), id, &u, &VENDOR);
    let b = g.entry(ReleaseEntry::Resume { intent: id, undo: vec![(u.clone(), vendor_sig)] }, &[&CUSTOMER]);
    assert_eq!(g.no(b), Refusal::BadSignature);
    let sig = presign_undo(install(1), id, &u, &CUSTOMER);
    let b = g.entry(ReleaseEntry::Resume { intent: id, undo: vec![(u.clone(), sig)] }, &[&CUSTOMER]);
    g.ok(b);
    // The old vendor-signed undo is no longer live; the customer's is.
    let c = g.f.current.as_ref().unwrap();
    assert_eq!(c.undo_key, public_key(&CUSTOMER));
    let old = undo_entry(install(1), id, &u, public_key(&VENDOR), vendor_sig).encode();
    assert_eq!(g.no(old), Refusal::NotAnUndo("Rollback"));
    let new = undo_entry(install(1), id, &u, public_key(&CUSTOMER), sig).encode();
    g.ok(new);
}

#[test]
fn a_vendor_signed_intent_after_keys_vendor_none_is_refused() {
    let (mut g, _) = history();
    let id = g.f.current.as_ref().unwrap().intent.id;
    g.ok(g.entry(ReleaseEntry::Abort { intent: id, target: rel(3).0 }, &[&CUSTOMER]));
    let b = g.entry(ReleaseEntry::IntentDone { intent: id }, &[&EXECUTOR]);
    g.ok(b);
    let b = g.intent(90, day(1, 3), &[], &VENDOR);
    assert_eq!(g.no(b), Refusal::UnknownKey(public_key(&VENDOR)));
    let b = g.intent(90, day(1, 3), &[], &CUSTOMER);
    g.ok(b);
}

#[test]
fn the_codec_round_trips_every_entry_kind() {
    let (_, submitted) = history();
    for b in &submitted {
        let e = SignedEntry::decode(b).unwrap();
        assert_eq!(&e.encode(), b, "{}", e.body.kind());
    }
    // An undo list that holds a forward entry does not decode.
    let mut g = G::new();
    g.upload(1);
    let mut bad = SignedEntry::decode(&g.intent(1, day(0, 1), &[rollback(0, 1)], &VENDOR)).unwrap();
    if let ReleaseEntry::Intent(i) = &mut bad.body {
        i.undo[0].0 = ReleaseEntry::SiteLost { site: SiteId(1) };
    }
    assert!(SignedEntry::decode(&bad.encode()).is_err());
}

/// The `Release` codec is append-only: every install's log holds a genesis, and from M5-WP1 any entry. One sample of
/// each kind's stored body, pinned in `entry_tags.golden` as `<tag> <kind> <body hex>`. A new variant takes the next
/// tag and adds a line; no line changes. `GRAFTYARD_PIN=1` prints the table.
#[test]
fn every_entry_kind_keeps_its_tag_and_bytes() {
    let (g, submitted) = history();
    let mut by_kind = std::collections::BTreeMap::new();
    let mut add = |e: &ReleaseEntry| {
        let mut out = Vec::new();
        crate::types::entry(&mut crate::codec::Enc(&mut out), e);
        by_kind.entry(out[0]).or_insert_with(|| format!("{} {} {}", out[0], e.kind(), crate::types::hex(&out)));
    };
    for b in g.log.iter().take(1).chain(&submitted) {
        let e = SignedEntry::decode(b).unwrap();
        add(&e.body);
        if let ReleaseEntry::Intent(i) = &e.body {
            for (u, _) in &i.undo {
                add(u);
            }
        }
    }
    let extra = [
        ReleaseEntry::FormatsEnable { format: Format { family: 32, version: 19 } },
        ReleaseEntry::Resume { intent: id(1), undo: vec![(rollback(0, 1), Signature([1; 64]))] },
        ReleaseEntry::Abort { intent: id(1), target: rel(1).0 },
    ];
    for e in &extra {
        add(e);
    }
    let got: Vec<String> = by_kind.into_values().collect();
    if std::env::var("GRAFTYARD_PIN").is_ok() {
        println!("{}", got.join("\n"));
        return;
    }
    let want: Vec<&str> = include_str!("entry_tags.golden").lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(got.len(), 24, "every ReleaseEntry kind has a sample");
    for (i, w) in want.iter().enumerate() {
        assert_eq!(got.get(i).map(String::as_str), Some(*w), "tag {i} changed: the Release codec is append-only");
    }
}
