//! OPEN-QUESTIONS R47 in memory: `GATE_ERRORS`'s input. `graft-front` counts each answer by the version that ran it
//! (`gate::count`), every isolate sends a script's counts to that script's home site (`gate::errors_sites`), and
//! `graft-ops` judges an edge, front, or MCP roll on the sum (`gate::judge_roll`), so a bad `graft-mcp` at 1% swaps
//! back through its pre-signed `McpPointer` with no forced `GATE`.

use std::collections::BTreeMap;

use super::ops::{OLD, Ops};
use super::*;
use crate::gate::{self, Answers, ScriptErrors};
use crate::ops::{self, Action, Deployment, Dwell, Gate};

fn a(answers: u64, fivexx: u64) -> Answers {
    Answers { answers, fivexx }
}

fn breach(g: &Gate) -> bool {
    matches!(g, Gate::Breach(_))
}

/// Each answer counts under the script that answered, by the version it reported, and under the front by its own
/// version. A thrown call is a 5xx of the front and `threw` of the script; an answer with no version is `unversioned`.
#[test]
fn front_counts_each_answer_by_the_version_that_ran_it() {
    let mut p = BTreeMap::new();
    gate::count(&mut p, Script::Mcp, Some("m2"), Some("f1"), Some(200));
    gate::count(&mut p, Script::Mcp, Some("m2"), Some("f1"), Some(503));
    gate::count(&mut p, Script::Mcp, Some("m1"), Some("f1"), Some(404));
    gate::count(&mut p, Script::Mcp, None, Some("f1"), None);
    gate::count(&mut p, Script::Mcp, None, Some("f1"), Some(500));
    gate::count(&mut p, Script::Edge, Some("e1"), Some("f1"), Some(502));
    let mcp = &p["mcp"];
    assert_eq!(mcp.versions.get("m2"), Some(&a(2, 1)));
    assert_eq!(mcp.versions.get("m1"), Some(&a(1, 0)), "a 4xx is an answer, not a 5xx");
    assert!(!mcp.versions.contains_key("f1"), "the front's version is not the script's");
    assert_eq!(mcp.threw, 1);
    assert_eq!(mcp.unversioned, a(1, 1));
    assert_eq!(p["edge"].versions.get("e1"), Some(&a(1, 1)));
    // The front served all six: the thrown call and the four 5xx are its 5xx too.
    assert_eq!(p["front"].versions.get("f1"), Some(&a(6, 4)));
    // The registry object's sum is the same counts added.
    let mut sum = ScriptErrors::default();
    sum.add(mcp);
    sum.add(mcp);
    assert_eq!((sum.versions["m2"], sum.threw, sum.unversioned), (a(4, 2), 2, a(2, 2)));
}

/// A script's counts go to one home site, the same from every isolate whatever order its site table lists, then to
/// each next site only on failover; the homes differ by script, so not every write lands on the lowest site.
#[test]
fn each_scripts_counts_go_to_its_home_site_then_the_next() {
    let voters = [SiteId(0), SiteId(1), SiteId(2)];
    let mut homes = Vec::new();
    for key in ["edge", "mcp", "front"] {
        let order = gate::errors_sites(key, &voters);
        assert_eq!(gate::errors_sites(key, &[SiteId(2), SiteId(0), SiteId(1), SiteId(0)]), order, "{key}");
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, voters, "{key}: every site once");
        // The failover order is the sites after the home, by id, wrapping.
        let i = order[0].0 as usize;
        assert_eq!(order, [voters[i..].to_vec(), voters[..i].to_vec()].concat(), "{key}");
        homes.push(order[0]);
    }
    assert_eq!(homes, [SiteId(0), SiteId(0), SiteId(2)], "edge and mcp home on a, front on c");
    assert!(gate::errors_sites("mcp", &[]).is_empty());
}

/// `GATE_ERRORS`: N+1's rate against N's plus 0.1 points, past chance. A few 5xx of a few answers breach under the
/// floor; one 5xx of one answer, or a rate within N's plus 0.1 points, does not; with no breach, under
/// `STEP_MIN_ANSWERS` is `Starved`.
#[test]
fn gate_errors_breaches_on_n1s_excess_over_n_and_starves_under_the_floor() {
    let clean = a(10_000, 0);
    assert!(breach(&gate::judge_errors(a(2, 2), 0, clean)), "2 of 2 at 1%");
    assert!(breach(&gate::judge_errors(a(3, 3), 0, Answers::default())), "3 of 3 with no N yet");
    assert_eq!(gate::judge_errors(a(1, 1), 0, clean), Gate::Starved(1), "one 5xx in a quiet step is chance");
    assert_eq!(gate::judge_errors(a(10, 0), 0, clean), Gate::Starved(10));
    assert_eq!(gate::judge_errors(a(999, 0), 0, clean), Gate::Starved(999));
    assert_eq!(gate::judge_errors(a(1_000, 1), 0, clean), Gate::Pass, "0.1% is within 0.1 points of 0%");
    assert!(breach(&gate::judge_errors(a(1_000, 10), 0, clean)), "1% against 0%");
    // N's own rate is the baseline: 2.1% against N's 2% passes, the same against a clean N breaches.
    let busy = a(10_000, 200);
    assert_eq!(gate::judge_errors(a(2_000, 42), 0, busy), Gate::Pass);
    assert!(breach(&gate::judge_errors(a(2_000, 42), 0, clean)));
    assert!(breach(&gate::judge_errors(a(2_000, 80), 0, busy)), "4% against 2%");
    // A thrown call is one of N+1's 5xx: 1,000 clean answers and 10 thrown calls breach.
    assert!(breach(&gate::judge_errors(a(1_000, 0), 10, clean)));
    assert_eq!(gate::judge_errors(a(1_010, 0), 0, clean), Gate::Pass, "the same calls answered clean");
    assert!(breach(&gate::judge_errors(a(990, 0), 9, clean)) && breach(&gate::judge_errors(a(999, 9), 0, clean)));
}

/// The host's judgment from the summed counts: N+1's version in the window; N's in the window, or from the intent's
/// start once the window holds under the floor of N's; answers with no version never count against N+1.
#[test]
fn a_roll_is_judged_on_its_versions_and_a_missing_version_header_never_breaches() {
    let v = |n: u8| VersionId([n; 16]);
    let errs = |rows: &[(u8, Answers)], threw: u64, unversioned: Answers| ScriptErrors {
        versions: rows.iter().map(|(n, x)| (v(*n).to_string(), *x)).collect(),
        threw,
        unversioned,
    };
    // At 100% N serves 0%: its window is empty, and from the intent's start it ran at 3%.
    let window = errs(&[(2, a(5_000, 140))], 0, Answers::default());
    let whole = errs(&[(2, a(6_000, 170)), (1, a(50_000, 1_500))], 0, Answers::default());
    assert_eq!(gate::judge_roll(&window, &whole, v(2), Some(v(1))), Gate::Pass, "2.8% against N's 3%");
    assert!(breach(&gate::judge_roll(&window, &errs(&[], 0, Answers::default()), v(2), Some(v(1)))));
    // N with enough answers in the window is judged there, not from the start.
    let window = errs(&[(2, a(5_000, 140)), (1, a(5_000, 0))], 0, Answers::default());
    assert!(breach(&gate::judge_roll(&window, &whole, v(2), Some(v(1)))), "2.8% against N's 0% in the window");
    // Another version's answers are not N+1's.
    let window = errs(&[(9, a(5_000, 5_000)), (2, a(5_000, 0))], 0, Answers::default());
    assert_eq!(gate::judge_roll(&window, &window, v(2), None), Gate::Pass);
    // 500 answers with no version header, every one a 5xx, alarm in the host and do not breach.
    let window = errs(&[(2, a(2_000, 0))], 0, a(500, 500));
    assert_eq!(gate::judge_roll(&window, &window, v(2), None), Gate::Pass);
    // A thrown binding call does.
    let window = errs(&[(2, a(2_000, 0))], 30, Answers::default());
    assert!(breach(&gate::judge_roll(&window, &window, v(2), None)));
}

fn upload(o: &mut Ops, n: u8, scripts: &[Script]) {
    let b = o.g.entry(G::upload_body(n, scripts), &[&EXECUTOR]);
    o.g.ok(b);
}

/// The swap-back's trigger end to end in memory: release 2's `McpRoll` at 1%, a `graft-mcp` that answers 5xx, its
/// counts judged with no `GATE`: the run submits the pre-signed `McpPointer` to release 1. A healthy N+1 under the
/// floor commits its 1% step after its dwell; only the edge, front, and MCP rolls are judged on errors.
#[test]
fn a_bad_graft_mcp_at_1pct_is_judged_on_its_5xx_and_swapped_back_by_its_pointer() {
    let mut o = Ops::new(G::new());
    o.deps.insert(Script::Mcp, Deployment { split: vec![(OLD, 100)], created: T0 });
    upload(&mut o, 1, &[Script::Mcp]);
    o.roll_all(1);
    upload(&mut o, 2, &[Script::Mcp]);
    assert!(matches!(o.roll(2), ops::Roll::Next { intent: Intent::McpRoll { .. }, .. }));
    let (v1, v2) = (vid(1, 0), vid(2, 0));
    assert_eq!(gate::roll_versions(&o.g.f), Some((Script::Mcp, v2, Some(v1))));
    o.act(); // the 1% deploy
    assert_eq!(o.deps[&Script::Mcp].split, vec![(v2, 1), (v1, 99)]);
    // The front's counts in the first minutes: N+1 answered 5 of its 6 with 500, N 600 clean.
    let mut p = BTreeMap::new();
    for i in 0..606u32 {
        let (ran, status) = if i < 6 { (v2, if i == 0 { 200 } else { 500 }) } else { (v1, 200) };
        gate::count(&mut p, Script::Mcp, Some(&ran.to_string()), Some("f"), Some(status));
    }
    o.gate = gate::judge_roll(&p["mcp"], &p["mcp"], v2, Some(v1));
    assert!(breach(&o.gate), "{:?}", o.gate);
    assert!(matches!(o.act(), Action::Undo(_)), "the breach submits the undo before the dwell ends");
    assert_eq!(o.g.f.pointers.mcp, Some(v1), "pinned to N");
    o.gate = Gate::Pass;
    o.finish();
    assert_eq!(o.deps[&Script::Mcp].split, vec![(v1, 100), (v2, 0)]);

    // Release 3 is healthy and quiet: 40 clean answers at 1% starve, the step commits after its dwell.
    upload(&mut o, 3, &[Script::Mcp]);
    assert!(matches!(o.roll(3), ops::Roll::Next { intent: Intent::McpRoll { .. }, .. }));
    o.act();
    let start = o.deps[&Script::Mcp].created;
    o.gate = gate::judge_errors(a(40, 0), 0, a(4_000, 0));
    assert_eq!(o.gate, Gate::Starved(40));
    o.g.at = Timestamp(start.0 + ops::FIRST_DAY);
    assert!(matches!(
        ops::next(&o.g.f, &o.world(), Dwell::default()),
        Action::Commit { body: ReleaseEntry::IntentStep { step: 0, .. }, .. }
    ));

    // graft-ops's own roll is not judged on errors: no front sees its answers.
    let mut o = Ops::new(G::new());
    upload(&mut o, 1, &[Script::Ops]);
    o.roll_all(1);
    upload(&mut o, 2, &[Script::Ops]);
    assert!(matches!(o.roll(2), ops::Roll::Next { intent: Intent::OpsRoll { .. }, .. }));
    assert_eq!(gate::roll_versions(&o.g.f), None);
}

/// How a fresh install replaces an N whose versions were deleted (`tests/am1/results/install-m4a.md`, "Before the next
/// `FrontRoll` or `McpRoll` here"): a release w4 with only `front/` and `mcp/` is rolled, and the customer signs
/// `Abort { target: w4 }` for each intent before its first step. Each returns to w4 at 100%, and the group records w4
/// as the release each script runs, so w4 is N for the next roll and its 1% split names w4's versions, not the deleted
/// ones.
#[test]
fn an_abort_to_the_intents_own_release_records_that_release_as_n() {
    let mut o = Ops::new(G::new());
    o.deps.insert(Script::Mcp, Deployment { split: vec![(OLD, 100)], created: T0 });
    o.deps.insert(Script::Front, Deployment { split: vec![(OLD, 100)], created: T0 });
    upload(&mut o, 1, &[Script::Mcp, Script::Front]);
    o.roll_all(1);
    // Release 1's versions are gone: the scripts were deleted and redeployed by hand as another version.
    let hand = VersionId([0x77; 16]);
    for s in [Script::Mcp, Script::Front] {
        o.deps.insert(s, Deployment { split: vec![(hand, 100)], created: o.g.at });
    }
    upload(&mut o, 4, &[Script::Mcp, Script::Front]);
    for kind in ["FrontRoll", "McpRoll"] {
        let ops::Roll::Next { intent, .. } = o.roll(4) else { panic!("no {kind}") };
        assert_eq!(intent.kind(), kind);
        let c = o.g.f.current.as_deref().unwrap();
        assert_eq!(c.next, 0, "{kind}: the Abort goes in before step 0 commits");
        let deployed = o.deployed.len();
        let b = o.g.entry(ReleaseEntry::Abort { intent: c.intent.id, target: rel(4).0 }, &[&CUSTOMER]);
        o.g.ok(b);
        o.finish();
        assert_eq!(o.deployed.len(), deployed + 1, "{kind}: one deploy, the return to w4");
    }
    let (m4, f4) = (vid(4, 0), vid(4, 1));
    assert_eq!(o.deps[&Script::Mcp].split, vec![(m4, 100)]);
    assert_eq!(o.deps[&Script::Front].split, vec![(f4, 100)]);
    for s in [Script::Mcp, Script::Front] {
        assert_eq!(ops::running(&o.g.f, s), Some(rel(4).0), "{s:?} runs w4");
    }
    assert_eq!(ops::next_roll(&o.g.f, rel(4).0), ops::Roll::Done);
    // The next release's McpRoll keeps w4 as N: its undo pins w4, and its 1% split is w5 beside w4.
    upload(&mut o, 5, &[Script::Mcp]);
    let ops::Roll::Next { intent: Intent::McpRoll { .. }, undo } = o.roll(5) else { panic!("no McpRoll") };
    assert_eq!(undo, vec![ReleaseEntry::McpPointer { pin: rel(4).0 }]);
    assert_eq!(gate::roll_versions(&o.g.f), Some((Script::Mcp, vid(5, 0), Some(m4))));
    o.act();
    assert_eq!(o.deps[&Script::Mcp].split, vec![(vid(5, 0), 1), (m4, 99)]);
}

/// R47's 1% gap closed: a front or MCP roll's 100% step holds `STEP_DWELL` and is judged on `GATE_ERRORS` against
/// N's counts from the intent's start, since N serves 0% there. A build that fails 1% of calls, too few answers to
/// breach at 1%, swaps back at 100% by its pointer; a starved 100% step dwells to twice the dwell and pages; one that
/// passes commits; a front roll, with no pre-signed undo, pages on a breach.
#[test]
fn a_front_or_mcp_rolls_100pct_step_holds_and_is_judged_against_n_from_the_intents_start() {
    let errs = |rows: &[(VersionId, Answers)]| ScriptErrors {
        versions: rows.iter().map(|(v, x)| (v.to_string(), *x)).collect(),
        ..ScriptErrors::default()
    };
    // Runs the current roll's 1% step (starved, so it commits after FIRST_DAY) and deploys 100%.
    let past_1pct = |o: &mut Ops| {
        o.act();
        o.gate = Gate::Starved(40);
        o.g.at = Timestamp(o.g.at.0 + ops::FIRST_DAY);
        assert!(matches!(o.act(), Action::Commit { body: ReleaseEntry::IntentStep { step: 0, .. }, .. }));
        o.act();
    };
    let mut o = Ops::new(G::new());
    o.deps.insert(Script::Mcp, Deployment { split: vec![(OLD, 100)], created: T0 });
    o.deps.insert(Script::Front, Deployment { split: vec![(OLD, 100)], created: T0 });
    upload(&mut o, 1, &[Script::Mcp, Script::Front]);
    o.roll_all(1);
    upload(&mut o, 2, &[Script::Mcp]);
    assert!(matches!(o.roll(2), ops::Roll::Next { intent: Intent::McpRoll { .. }, .. }));
    let (v1, v2) = (vid(1, 0), vid(2, 0));
    past_1pct(&mut o);
    assert_eq!(o.deps[&Script::Mcp].split, vec![(v2, 100), (v1, 0)]);
    let start = o.deps[&Script::Mcp].created;

    // A passing read right after the 100% deploy does not end the intent: the step holds STEP_DWELL.
    o.gate = Gate::Pass;
    let Action::Wait { until: Some(until), .. } = ops::next(&o.g.f, &o.world(), Dwell::default()) else {
        panic!("the 100% step commits with no dwell")
    };
    assert_eq!(until, Timestamp(start.0 + ops::STEP_DWELL));

    // N served 0% at 100%, so its window is empty; from the intent's start it answered 50,000 at 3%. N+1 at 2.8%
    // passes against that, and would breach against N's empty window.
    let whole_n = (v1, a(50_000, 1_500));
    let window = errs(&[(v2, a(5_000, 140))]);
    assert_eq!(gate::judge_roll(&window, &errs(&[whole_n]), v2, Some(v1)), Gate::Pass);
    assert!(breach(&gate::judge_roll(&window, &window, v2, Some(v1))));
    // Starved after the dwell: dwell on to twice the dwell, then page.
    o.g.at = until;
    o.gate = gate::judge_roll(&errs(&[(v2, a(300, 0))]), &errs(&[whole_n]), v2, Some(v1));
    assert_eq!(o.gate, Gate::Starved(300));
    let Action::Wait { until: Some(cap), .. } = ops::next(&o.g.f, &o.world(), Dwell::default()) else {
        panic!("a starved 100% step commits")
    };
    assert_eq!(cap, Timestamp(start.0 + 2 * ops::STEP_DWELL));
    o.g.at = cap;
    assert!(matches!(ops::next(&o.g.f, &o.world(), Dwell::default()), Action::Page(_)));
    // Enough answers, passing: the 100% step commits and the intent ends on release 2.
    o.gate = gate::judge_roll(&window, &errs(&[whole_n]), v2, Some(v1));
    o.finish();
    assert_eq!(ops::running(&o.g.f, Script::Mcp), Some(rel(2).0));

    // Release 3 fails 1% of calls: 1 of 80 at 1% is chance, and 20 of 2,000 at 100% against N's clean 7,920 from
    // the intent's start breaches before the dwell ends, so the run submits the McpPointer to release 2.
    upload(&mut o, 3, &[Script::Mcp]);
    assert!(matches!(o.roll(3), ops::Roll::Next { intent: Intent::McpRoll { .. }, .. }));
    let v3 = vid(3, 0);
    o.act();
    assert_eq!(
        gate::judge_roll(&errs(&[(v3, a(80, 1)), (v2, a(7_920, 0))]), &errs(&[]), v3, Some(v2)),
        Gate::Starved(80)
    );
    o.gate = Gate::Starved(80);
    o.g.at = Timestamp(o.g.at.0 + ops::FIRST_DAY);
    assert!(matches!(o.act(), Action::Commit { body: ReleaseEntry::IntentStep { step: 0, .. }, .. }));
    o.act();
    assert_eq!(o.deps[&Script::Mcp].split, vec![(v3, 100), (v2, 0)]);
    o.gate = gate::judge_roll(&errs(&[(v3, a(2_000, 20))]), &errs(&[(v2, a(7_920, 0))]), v3, Some(v2));
    assert!(breach(&o.gate), "{:?}", o.gate);
    assert!(matches!(o.act(), Action::Undo(_)));
    assert_eq!(o.g.f.pointers.mcp, Some(v2), "pinned to N");
    o.gate = Gate::Pass;
    o.finish();

    // A front roll breaching at 100% has no pre-signed undo: it pages, and the return needs the API.
    upload(&mut o, 4, &[Script::Front]);
    let ops::Roll::Next { intent: Intent::FrontRoll { .. }, undo } = o.roll(4) else { panic!("no FrontRoll") };
    assert!(undo.is_empty());
    past_1pct(&mut o);
    o.gate = Gate::Breach("GATE_ERRORS".into());
    let Action::Page(why) = ops::next(&o.g.f, &o.world(), Dwell::default()) else { panic!("no page") };
    assert!(why.contains("no undo left"), "{why}");
}

/// BM-5's swap-back trigger in memory: release 2's `OpsRoll` at 1%, a `graft-ops` N+1 whose mints answer 503, judged
/// by one edge isolate's own mint counts (`MintWatch`) with the API down and no `GATE`. The isolate submits the
/// pre-signed `OpsPointer` to release 1, the group commits it, so the pin moves with no deploy, and `graft-ops` returns
/// to N at 100% once the API answers. N's errors, a 4xx, a quiet N+1, and another roll never swap back.
#[test]
fn bm5_a_bad_graft_ops_at_1pct_is_swapped_back_by_an_edge_isolates_mint_errors() {
    let mut o = Ops::new(G::new());
    upload(&mut o, 1, &[Script::Ops]);
    o.roll_all(1);
    upload(&mut o, 2, &[Script::Ops]);
    let ops::Roll::Next { intent: Intent::OpsRoll { .. }, undo } = o.roll(2) else { panic!("no OpsRoll") };
    assert_eq!(undo, vec![ReleaseEntry::OpsPointer { pin: rel(1).0 }]);
    let (v1, v2) = (vid(1, 0), vid(2, 0));
    o.act(); // the 1% deploy
    assert_eq!(o.deps[&Script::Ops].split, vec![(v2, 1), (v1, 99)]);
    assert_eq!(o.gate, Gate::Pass);
    o.gate = Gate::Pending; // the host's gate for an OpsRoll: no front sees its answers
    let (s1, s2) = (v1.to_string(), v2.to_string());
    let mut w = gate::MintWatch::default();
    let mut t = 0;
    let mut feed = |w: &mut gate::MintWatch, f: &ReleaseFold, ran: Option<&str>, status: Option<u16>| {
        t += 100;
        w.record(f, ran, status, t)
    };

    // N's own 503s (a Secrets Store failure every version shares) and the grant's 4xx swap nothing back.
    for _ in 0..50 {
        assert!(feed(&mut w, &o.g.f, Some(&s1), Some(503)).is_none());
        assert!(feed(&mut w, &o.g.f, Some(&s2), Some(422)).is_none());
    }
    // One 503 of N+1's 51 answers is chance at 1%.
    assert!(feed(&mut w, &o.g.f, Some(&s2), Some(503)).is_none());
    // Mints N+1 answers 503: the sixth more makes 7 of N+1's 57 answers, past chance over 1% (a KL bound of ln
    // 10,000, worked out apart from the code), and the isolate submits the undo once.
    let mut sent = None;
    for i in 0..20 {
        if let Some(e) = feed(&mut w, &o.g.f, Some(&s2), Some(503)) {
            sent = Some((i, e));
            break;
        }
    }
    let (i, e) = sent.expect("the isolate submits the OpsPointer");
    assert_eq!(i, 5, "the sixth failed mint");
    assert!(feed(&mut w, &o.g.f, Some(&s2), Some(503)).is_none(), "not again within MINT_RESEND_MS");

    // The API is down: the group commits the pointer, graft-ops cannot deploy, and the pin is N.
    let deployed = o.deployed.len();
    o.g.ok(e.encode());
    assert_eq!(o.g.f.pointers.ops, Some(v1), "pinned to N with no API call");
    assert_eq!(o.deployed.len(), deployed);
    t += gate::MINT_RESEND_MS;
    assert!(w.record(&o.g.f, Some(&s2), Some(503), t).is_none(), "a committed undo is never sent again");
    // graft-ops returns to N at 100% once the API answers, and finishes the intent.
    o.gate = Gate::Pass;
    o.finish();
    assert_eq!(o.deps[&Script::Ops].split, vec![(v1, 100), (v2, 0)]);

    // A thrown binding call is one of N+1's errors: release 3's roll swaps back on throws alone.
    upload(&mut o, 3, &[Script::Ops]);
    assert!(matches!(o.roll(3), ops::Roll::Next { intent: Intent::OpsRoll { .. }, .. }));
    o.act();
    let mut w = gate::MintWatch::default();
    let mut got = None;
    for k in 0..10 {
        got = got.or(w.record(&o.g.f, None, None, k));
    }
    assert!(got.is_some(), "thrown mints swap back");

    // A healthy, quiet N+1 never swaps back, and an EdgeRoll's mint errors are not graft-ops's to undo.
    let mut o = Ops::new(G::new());
    upload(&mut o, 1, &[Script::Ops, Script::Edge]);
    o.roll_all(1);
    upload(&mut o, 2, &[Script::Ops, Script::Edge]);
    assert!(matches!(o.roll(2), ops::Roll::Next { intent: Intent::EdgeRoll { .. }, .. }));
    o.act();
    let mut w = gate::MintWatch::default();
    for k in 0..30 {
        assert!(w.record(&o.g.f, None, None, k).is_none(), "an EdgeRoll");
    }
    o.finish();
    assert!(matches!(o.roll(2), ops::Roll::Next { intent: Intent::OpsRoll { .. }, .. }));
    o.act();
    let v2 = o.deps[&Script::Ops].split[0].0.to_string();
    for k in 0..2_000 {
        assert!(w.record(&o.g.f, Some(&v2), Some(200), k).is_none());
    }
    // 1% of N+1's mints failing is the line itself, not past it.
    for k in 0..20 {
        assert!(w.record(&o.g.f, Some(&v2), Some(500), 2_000 + k).is_none(), "{k}");
    }
}

/// OPEN-QUESTIONS A57 (LM-3 r2): the edge sums the sites that answered, and the agents `graft-ops`' controller gate
/// logs a line on every outcome: a missed read is `Pending` and names the read, never a silent wait; both reads judge
/// as `judge_roll` does.
#[test]
fn a57_the_errors_read_sums_the_sites_that_answered_and_a_missed_gate_read_logs_its_pending() {
    let (n1, n) = (VersionId([0x21; 16]), VersionId([0x20; 16]));
    let site = |bad: u64, good: u64| {
        let mut e = ScriptErrors::default();
        e.versions.insert(n1.to_string(), Answers { answers: bad, fivexx: bad });
        e.versions.insert(n.to_string(), Answers { answers: good, fivexx: 0 });
        e
    };
    // Two of three sites answer: their sum, and the count of sites that did; none answering is None.
    let (sum, sites) = gate::sum_sites([Some(site(3, 400)), None, Some(site(5, 600))]).expect("two answered");
    assert_eq!(
        (sum.of(n1), sum.of(n), sites),
        (Answers { answers: 8, fivexx: 8 }, Answers { answers: 1000, fivexx: 0 }, 2)
    );
    assert_eq!(gate::sum_sites([None, None, None]), None);
    // The sites are asked at once: every read starts before any ends. Asked in turn, one would be in flight at a time.
    let (in_flight, peak) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
    let read = |k: u64| {
        let (in_flight, peak) = (&in_flight, &peak);
        async move {
            in_flight.set(in_flight.get() + 1);
            peak.set(peak.get().max(in_flight.get()));
            Yield(false).await;
            in_flight.set(in_flight.get() - 1);
            (k != 1).then(|| site(k, 100))
        }
    };
    let got = futures_executor::block_on(gate::read_sites(&[0u64, 1, 2], read));
    assert_eq!(
        (peak.get(), got.map(|(s, n)| (s.of(n1).answers, n))),
        (3, Some((2, 2))),
        "three reads at once, two answered"
    );
    // A missed read waits and says which.
    for (w, a) in [
        (Err("since=1: no answer within 5000 ms".to_string()), Ok(sum.clone())),
        (Ok(sum.clone()), Err("since=0: 503".into())),
    ] {
        let missed = w.as_ref().err().or(a.as_ref().err()).cloned().unwrap();
        let (g, line) = gate::controller_gate(w, a, n1, Some(n));
        assert_eq!(g, Gate::Pending);
        assert!(line.contains("pending") && line.contains(&missed), "{line}");
    }
    // Both reads: the judgement and its numbers.
    let (g, line) = gate::controller_gate(Ok(sum.clone()), Ok(sum.clone()), n1, Some(n));
    assert!(matches!(g, Gate::Breach(_)), "{g:?}");
    assert!(line.contains("Breach") && line.contains("answers: 8"), "{line}");
    // LM-3 r2's slowest read from the agents account took 1,550 ms: the cron's deadline covers it.
    let slowest_r2_read_ms = [790u64, 1_220, 970, 1_140, 1_550].into_iter().max().unwrap();
    assert!(gate::GATE_READ_MS > slowest_r2_read_ms, "the cron's read is not held to a request's deadline");
}

/// A future that is pending once, so a sequential reader would finish each read before it starts the next.
struct Yield(bool);

impl std::future::Future for Yield {
    type Output = ();
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if self.0 {
            return std::task::Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    }
}
