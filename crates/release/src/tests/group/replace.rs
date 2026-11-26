//! M6-WP1 in memory: `graft-ops` signs `ReplaceMember` with the executor key for a site with a customer-signed
//! `SiteLost`, or one drained `ZERO_ACKS` that both other sites read as silent; runs it through its coordinator
//! Workflow; refuses a spare that cannot read every enabled format; holds the lost site's drain until it has recovered;
//! and `RestorePlacement` then moves the groups back and clears the loss (`design/DESIGN.md`, journey 5 and "Install
//! operations", Site loss; `build/BUILD-PLAN.md`, BF-50).

use std::collections::BTreeMap;

use super::ops::{Ops, upload_all};
use super::*;
use crate::gate::Verdict;
use crate::group::RESTORE_GATE_FROM;
use crate::ops::{self, Action, Dwell, MoveRun, MoveSpec, Signer};
use crate::view::Replica;

fn next(o: &Ops) -> Action {
    ops::next(&o.g.f, &o.world(), Dwell::default())
}

/// A verdict with no burn: `calm` is the automatic drain's 15-minute calm, which a lost site's drain ignores.
fn v(calm: bool, silent: bool, recovered: bool) -> Verdict {
    Verdict { burning: false, calm, silent, recovered }
}

/// An install on release 1 everywhere, the spare (site 3, eeur) included, with site 1 (weur) drained by the drain key
/// as the automatic drain leaves a site that stopped answering.
fn drained_weur() -> Ops {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    for s in 0..4 {
        let b = o.g.entry(ReleaseEntry::SiteRelease { site: SiteId(s), release: rel(1).0 }, &[&EXECUTOR]);
        o.g.ok(b);
    }
    let b = o.g.drain(1, Mode::Drain, &DRAIN);
    o.g.ok(b);
    o
}

fn replace_of(a: &Action) -> Option<(u8, u8)> {
    match a {
        Action::Commit { body: ReleaseEntry::Intent(i), signer: Signer::Executor } => match i.intent {
            Intent::ReplaceMember { lost, spare } => Some((lost.0, spare.0)),
            _ => None,
        },
        _ => None,
    }
}

/// `ZERO_ACKS` is a held drain and zero acks read by graft-ops: neither alone signs, and both sign a `ReplaceMember`
/// onto the spare on weur's continent (eeur), which the group accepts.
#[test]
fn graft_ops_replaces_a_site_drained_zero_acks_only_once_both_other_sites_read_it_silent() {
    let mut o = drained_weur();
    let since = o.g.f.drained[&1].since;
    o.health = BTreeMap::from([(1, v(false, true, false))]);
    o.g.at = Timestamp(since.0 + ZERO_ACKS - 60_000);
    assert_eq!(next(&o), Action::Idle, "silent, but not yet drained ZERO_ACKS");
    o.g.at = Timestamp(since.0 + ZERO_ACKS);
    o.health = BTreeMap::from([(1, v(false, false, false))]);
    assert_eq!(next(&o), Action::Idle, "drained ZERO_ACKS, but some call was answered");
    o.health = BTreeMap::from([(1, v(false, true, false))]);
    assert_eq!(replace_of(&next(&o)), Some((1, 3)));
    o.act();
    assert_eq!(o.g.f.replaced, BTreeMap::from([(1, 3)]));
    // Its one step starts the coordinator, waits while it runs, then the intent ends.
    let spec = MoveSpec { from: SiteId(1), to: SiteId(3), dead: true, home: None };
    assert!(matches!(next(&o), Action::StartMove { spec: s, .. } if s == spec));
    o.act();
    assert!(matches!(next(&o), Action::Wait { .. }), "{:?}", next(&o));
    o.moves = MoveRun::Done;
    o.act();
    o.act();
    assert!(o.g.f.current.is_none(), "IntentStep and IntentDone");
    assert_eq!(o.started, vec![spec]);
    o.moves = MoveRun::Absent;
    assert_eq!(next(&o), Action::Idle, "a replaced site is not replaced again");
}

/// A customer-signed `SiteLost` needs no wait and no health. A spare whose release cannot read every enabled format,
/// here the second of two, is refused with a page, and nothing is signed until its site day puts a release on it that
/// reads them all.
#[test]
fn a_site_lost_replaces_at_once_but_never_onto_a_spare_that_cannot_read_every_enabled_format() {
    let mut o = drained_weur();
    let b = o.g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[&CUSTOMER]);
    o.g.ok(b);
    // Release 3 reads 32.1 to 32.3; every site, the spare included, reports it, and 32.2 and 32.3 are enabled.
    upload_all(&mut o.g, 3);
    for s in 0..4 {
        let e = o.g.entry(ReleaseEntry::SiteRelease { site: SiteId(s), release: rel(3).0 }, &[&EXECUTOR]);
        o.g.ok(e);
    }
    for v in [2, 3] {
        let b = super::ops::formats_enable(&o.g, Format { family: 32, version: v });
        o.g.ok(b);
    }
    // The spare's view as if its release were 2: it reads the first enabled format, 32.2, and not the second.
    upload_all(&mut o.g, 2);
    o.g.f.site_release.insert(3, rel(2).0);
    match next(&o) {
        Action::Page(why) => assert!(why.contains("spare 3") && why.contains("32.3"), "{why}"),
        a => panic!("signed onto a spare that cannot read 32.3: {a:?}"),
    }
    o.g.f.site_release.insert(3, rel(3).0);
    assert_eq!(replace_of(&next(&o)), Some((1, 3)));
}

/// A lost site with no spare on its continent pages and signs nothing.
#[test]
fn a_lost_site_with_no_spare_on_its_continent_pages() {
    let mut o = drained_weur();
    let b = o.g.entry(ReleaseEntry::SiteLost { site: SiteId(2) }, &[&CUSTOMER]);
    o.g.ok(b);
    match next(&o) {
        Action::Page(why) => assert!(why.contains("site 2") && why.contains("no spare"), "{why}"),
        a => panic!("{a:?}"),
    }
}

/// The replaced site's drain holds through an automatic drain's 15-minute calm and ends only once both other sites
/// read it recovered; `RestorePlacement` is refused while it is drained, then moves the spare's groups back home and
/// clears the loss, so the site can be lost and replaced again.
#[test]
fn a_replaced_site_undrains_only_once_recovered_and_its_restore_moves_back_and_clears_the_loss() {
    let mut o = drained_weur();
    let b = o.g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[&CUSTOMER]);
    o.g.ok(b);
    o.act();
    o.act();
    o.moves = MoveRun::Done;
    o.act();
    o.act();
    assert!(o.g.f.current.is_none());
    o.moves = MoveRun::Absent;
    o.health = BTreeMap::from([(1, v(true, false, false))]);
    assert_eq!(next(&o), Action::Idle, "calm, not recovered: still drained");
    // A restore while it is drained is refused by the group.
    let restore = Intent::RestorePlacement { site: SiteId(1) };
    let b = o.g.intent(60, restore.clone(), &[], &CUSTOMER);
    assert!(matches!(o.g.no(b), Refusal::Rejected(m) if m.contains("drained")));
    o.health = BTreeMap::from([(1, v(true, false, true))]);
    let a = next(&o);
    assert!(
        matches!(&a, Action::Commit { body: ReleaseEntry::SiteMode(m), signer: Signer::Drain }
        if m.site == SiteId(1) && m.mode == Mode::Normal),
        "{a:?}"
    );
    o.act();
    let b = o.g.intent(61, restore, &[], &CUSTOMER);
    o.g.ok(b);
    let back = MoveSpec { from: SiteId(3), to: SiteId(1), dead: false, home: Some(SiteId(1)) };
    assert!(matches!(next(&o), Action::StartMove { spec, .. } if spec == back));
    o.act();
    o.moves = MoveRun::Done;
    o.act();
    o.act();
    assert!(o.g.f.current.is_none());
    assert!(o.g.f.lost.is_empty() && o.g.f.replaced.is_empty(), "the restore clears the loss");
    assert_eq!(o.started.last(), Some(&back));
}

/// m6aL2's order: a customer-signed `SiteLost`, then a `RestorePlacement` before graft-ops has drained the site. The
/// group refuses it while the site is lost and not replaced, though nothing is drained yet; a restore that committed
/// anyway and halted under the `ReplaceMember` resumes only once the site is undrained.
#[test]
fn a_restore_inside_the_drain_window_is_refused_and_a_halted_one_resumes_only_undrained() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    for s in 0..4 {
        let b = o.g.entry(ReleaseEntry::SiteRelease { site: SiteId(s), release: rel(1).0 }, &[&EXECUTOR]);
        o.g.ok(b);
    }
    let b = o.g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[&CUSTOMER]);
    o.g.ok(b);
    assert!(o.g.f.drained.is_empty(), "the SiteLost drains nothing");
    let restore = Intent::RestorePlacement { site: SiteId(1) };
    // The next entry is stamped exactly RESTORE_GATE_FROM, the first instant the rule holds.
    o.g.at = Timestamp(RESTORE_GATE_FROM.0 - 60_000);
    let b = o.g.intent(60, restore.clone(), &[], &CUSTOMER);
    assert!(matches!(o.g.no(b), Refusal::Rejected(m) if m.contains("lost and not yet replaced")));
    // After the gate, the same site once replaced, recovered and undrained restores.
    o.act();
    assert_eq!(replace_of(&o.act()), Some((1, 3)));
    o.act();
    o.moves = MoveRun::Done;
    o.finish();
    assert_eq!(o.g.f.replaced.get(&1), Some(&3));
    o.health = BTreeMap::from([(1, v(true, false, true))]);
    o.act();
    assert!(o.g.f.drained.is_empty());
    let b = o.g.intent(61, restore.clone(), &[], &CUSTOMER);
    o.g.ok(b);

    // The restore as m6aL2 committed it, on the fold before this rule: halted under the replace once it is done.
    let mut h = Ops::new(G::new());
    upload_all(&mut h.g, 1);
    for s in 0..4 {
        let b = h.g.entry(ReleaseEntry::SiteRelease { site: SiteId(s), release: rel(1).0 }, &[&EXECUTOR]);
        h.g.ok(b);
    }
    let b = h.g.intent(60, restore.clone(), &[], &CUSTOMER);
    h.g.ok(b);
    let b = h.g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[&CUSTOMER]);
    h.g.ok(b);
    h.act();
    h.act();
    h.moves = MoveRun::Done;
    h.act();
    h.act();
    let c = h.g.f.current.as_deref().expect("the restore is current again");
    assert_eq!(c.intent.intent, restore);
    assert!(matches!(c.state, IntentState::Halted { .. }));
    assert!(h.g.f.drained.contains_key(&1));
    let resume = |g: &G| g.entry(ReleaseEntry::Resume { intent: id(60), undo: Vec::new() }, &[&CUSTOMER]);
    let b = resume(&h.g);
    assert!(matches!(h.g.no(b), Refusal::Rejected(m) if m.contains("undrained")));
    h.health = BTreeMap::from([(1, v(true, false, true))]);
    h.act();
    assert!(h.g.f.drained.is_empty(), "recovered: the drain key undrains it");
    let b = resume(&h.g);
    h.g.ok(b);
    assert!(matches!(h.g.f.current.as_deref().unwrap().state, IntentState::Running { .. }));
}

/// m6aL2's committed history, stamped before RESTORE_GATE_FROM: `SiteLost`, a restore before the drain, the drain, then
/// the `ReplaceMember`. A replica replays every entry, as a follower and every view isolate must, and `graft-ops` on it
/// starts the move: on a history cut at the restore it signed a drain on a stale prev every pass, so the
/// `ReplaceMember`'s `IntentDone` never committed (the m6aL2 continuation, `tests/m6/results/m6-wp1.md`).
#[test]
fn a_restore_committed_before_the_gate_replays_and_graft_ops_moves_on_from_it() {
    let mut h = Ops::new(G::new());
    upload_all(&mut h.g, 1);
    for s in 0..4 {
        let b = h.g.entry(ReleaseEntry::SiteRelease { site: SiteId(s), release: rel(1).0 }, &[&EXECUTOR]);
        h.g.ok(b);
    }
    // m6aL2's times: `SiteLost` at index 11 (2026-11-26T01:13:39Z), the restore at 12, 42 s later; each entry here is
    // a minute after the last, all before RESTORE_GATE_FROM.
    h.g.at = Timestamp(1_795_655_619_107 - 60_000);
    let b = h.g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[&CUSTOMER]);
    h.g.ok(b);
    let b = h.g.intent(60, Intent::RestorePlacement { site: SiteId(1) }, &[], &CUSTOMER);
    h.g.ok(b);
    h.act();
    assert!(h.g.f.drained.contains_key(&1), "graft-ops drained the lost site");
    assert_eq!(replace_of(&h.act()), Some((1, 3)));
    assert!(h.g.at < RESTORE_GATE_FROM);
    let n = h.g.log.len();
    let mut r = Replica::default();
    assert_eq!(r.extend(&h.g.a, 0, &page(&h.g, 0, n)).unwrap(), n as u64);
    assert_eq!(r.fold, h.g.f);
    let w = h.world();
    let a = ops::next(&r.fold, &w, Dwell::default());
    assert!(matches!(a, Action::StartMove { spec, .. } if spec.from == SiteId(1) && spec.to == SiteId(3)), "{a:?}");
}

/// A `RestorePlacement` of a site no `ReplaceMember` moved has nothing to move back: one step, no Workflow.
#[test]
fn a_restore_of_a_site_never_replaced_moves_nothing() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    let b = o.g.intent(60, Intent::RestorePlacement { site: SiteId(2) }, &[], &CUSTOMER);
    o.g.ok(b);
    o.finish();
    assert!(o.started.is_empty());
}

/// A coordinator that failed pages and ends nothing; the intent waits for a human.
#[test]
fn a_failed_move_workflow_pages() {
    let mut o = drained_weur();
    let b = o.g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[&CUSTOMER]);
    o.g.ok(b);
    o.act();
    o.act();
    o.moves = MoveRun::Failed("group repo:9 never caught up".into());
    match next(&o) {
        Action::Page(why) => assert!(why.contains("repo:9"), "{why}"),
        a => panic!("{a:?}"),
    }
}

/// A customer-signed `SiteLost` for a site nobody drained: graft-ops drains it with the drain key before it replaces
/// it, so its restore waits for the recovered undrain like a `ZERO_ACKS` site's.
#[test]
fn a_site_lost_that_was_never_drained_is_drained_first() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    for s in 0..4 {
        let b = o.g.entry(ReleaseEntry::SiteRelease { site: SiteId(s), release: rel(1).0 }, &[&EXECUTOR]);
        o.g.ok(b);
    }
    let b = o.g.entry(ReleaseEntry::SiteLost { site: SiteId(1) }, &[&CUSTOMER]);
    o.g.ok(b);
    let a = next(&o);
    assert!(
        matches!(&a, Action::Commit { body: ReleaseEntry::SiteMode(m), signer: Signer::Drain }
        if m.site == SiteId(1) && m.mode == Mode::Drain),
        "{a:?}"
    );
    o.act();
    assert_eq!(replace_of(&next(&o)), Some((1, 3)));
}

/// `gate::loss`: silent needs both other sites, each with `MIN_CALLS` calls and every one missed; recovered needs both
/// under `UNDRAIN_BURN` over the recovery window. One answered call, one observer, or too few calls is not silent.
#[test]
fn a_site_is_silent_only_when_both_others_missed_every_one_of_enough_calls() {
    use crate::gate::{self, SiteHealth};
    let voters = [SiteId(0), SiteId(1), SiteId(2)];
    let h = |o: u8, calls: u64, missed: u64| SiteHealth { observer: SiteId(o), target: SiteId(1), calls, missed };
    let run = |zero: &[SiteHealth], rec: &[SiteHealth]| {
        let mut m = BTreeMap::new();
        gate::loss(&mut m, &voters, zero, rec);
        m[&1]
    };
    assert!(run(&[h(0, 300, 300), h(2, 5_000, 5_000)], &[]).silent);
    assert!(!run(&[h(0, 300, 300), h(2, 5_000, 4_999)], &[]).silent, "one call answered");
    assert!(!run(&[h(0, 300, 300)], &[]).silent, "one observer alone");
    assert!(!run(&[h(0, 299, 299), h(2, 5_000, 5_000)], &[]).silent, "under MIN_CALLS is no evidence");
    // The site itself says nothing: its own report about itself is not an observer's.
    let own = SiteHealth { observer: SiteId(1), target: SiteId(1), calls: 1_000, missed: 1_000 };
    assert!(!run(&[h(0, 300, 300), own], &[]).silent);
    assert!(run(&[], &[h(0, 2_000, 39), h(2, 2_000, 0)]).recovered);
    assert!(!run(&[], &[h(0, 2_000, 40), h(2, 2_000, 0)]).recovered, "2 x is not under UNDRAIN_BURN");
}
