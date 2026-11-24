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
