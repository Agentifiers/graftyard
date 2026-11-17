//! Targeted cases for M1-WP3 (`build/BUILD-PLAN.md`): `transfer`, `suspect`, `Health` and `STEP_DOWN`, the transfer
//! back to `home`, the drain hand-off, release-aware refusals, and BF-14. Each names the rule it pins and, where the
//! design cites one, the past failure it reproduces.

mod common;

use common::cluster::Cluster;
use common::kv::{FUTURE_FORMAT, KV_FORMAT, KvCmd, KvEvent};
use common::sim::{SITES, config};
use graftyard_raft::{
    Alarm, Colo, CommitError, Deny, Envelope, LEASE, Message, Receipt, ReleaseTag, RequestId, Resolution, Role,
    StepDownWhy, Term, TransferError, Verdict, VoteReq,
};
use graftyard_types::{ReleaseId, SiteId, Timestamp};

const A: SiteId = SITES[0];
const B: SiteId = SITES[1];
const C: SiteId = SITES[2];

fn ms(d: std::time::Duration) -> u64 {
    d.as_millis() as u64
}

fn put(k: &str, v: &str) -> KvCmd {
    KvCmd::Put { key: k.into(), value: v.into() }
}

/// Drives the group for `total` ms in 100 ms steps. Each step the current leader takes one write (when `load`), and
/// every request `fail` accepts fails: it is not delivered, and the host reports it to its sender with
/// `call_failed`. Replies are never failed, so each failed call is reported exactly once.
fn drive(
    c: &mut Cluster,
    total: u64,
    load: bool,
    next_id: &mut u128,
    mut fail: impl FnMut(&Envelope<KvEvent>) -> bool,
) {
    let mut t = 0;
    while t < total {
        c.advance(100);
        if load && let Some(l) = c.leader() {
            *next_id += 1;
            let id = RequestId(graftyard_raft::Ulid(*next_id));
            let _ = c.on(l, |m, now| m.commit(now, id, put("load", &id.0.to_string())));
        }
        for _ in 0..200 {
            if c.inbox.is_empty() {
                break;
            }
            let batch = std::mem::take(&mut c.inbox);
            for e in batch {
                // A call to a member that is down misses its deadline; the host reports it like any failure.
                let down = c.members[e.to.0 as usize].is_none();
                if down || fail(&e) {
                    if let Some(f) = e.failure(false) {
                        let from = e.from;
                        if c.members[from.0 as usize].is_some() {
                            c.on(from, |m, now| m.call_failed(now, f));
                        }
                    }
                    continue;
                }
                let Envelope { from, to, msg } = e;
                c.on(to, |m, now| m.step(now, from, msg));
            }
        }
        t += 100;
    }
}

/// A request, as `drive` sees it: AppendEntries or a probe's ping.
fn request(e: &Envelope<KvEvent>) -> bool {
    matches!(e.msg, Message::Append(_) | Message::Ping { .. })
}

fn step_downs(c: &Cluster) -> Vec<(SiteId, StepDownWhy, Option<SiteId>)> {
    c.alarms()
        .iter()
        .filter_map(|(s, a)| match a {
            Alarm::StepDown { why, to } => Some((*s, why.clone(), *to)),
            _ => None,
        })
        .collect()
}

fn vote_res(c: &Cluster, from: SiteId, to: SiteId) -> Option<graftyard_raft::VoteRes> {
    c.inbox.iter().rev().find_map(|e| match &e.msg {
        Message::VoteRes(r) if e.from == from && e.to == to => Some(r.clone()),
        _ => None,
    })
}

fn vote(
    term: Term,
    candidate: SiteId,
    pre: bool,
    last: (u64, Term),
    transfer: Option<graftyard_raft::TransferId>,
) -> VoteReq {
    VoteReq {
        term,
        candidate,
        pre,
        last_index: graftyard_raft::Index(last.0),
        last_term: last.1,
        transfer,
        boots: [Timestamp(0); 2],
        stepped_down_at: None,
        bootstrapped: true,
    }
}

// ---- transfer ----

#[test]
fn transfer_hands_leadership_to_a_caught_up_target() {
    let mut c = Cluster::new(31);
    c.elect(A);
    c.sync(A);
    let term = c.m(A).term();
    assert!(matches!(c.on(A, |m, now| m.transfer(now, A)), Err(TransferError::Target(_))));
    assert!(matches!(c.on(B, |m, now| m.transfer(now, C)), Err(TransferError::NotLeader(_))));
    let id = c.on(A, |m, now| m.transfer(now, B)).expect("started");
    assert!(matches!(c.on(A, |m, now| m.transfer(now, C)), Err(TransferError::Busy(i)) if i == id));
    // Every AppendEntries while it runs carries the target and id, and no follower promises on it.
    let mut seen = 0;
    c.deliver(|e| {
        if let Message::Append(a) = &e.msg {
            assert_eq!(a.transfer, Some((B, id)));
            seen += 1;
        }
        if let Message::AppendRes(r) = &e.msg {
            assert!(!r.promise, "an ack to a transfer round promised");
        }
        true
    });
    assert!(seen >= 2);
    c.run(3_000, 100, |_| true);
    assert_eq!(c.m(B).role(), Role::Leader);
    assert_eq!(c.m(B).term(), Term(term.0 + 1));
    assert_eq!(c.m(A).role(), Role::Follower);
    // A transfer asked for by the host is not a STEP_DOWN.
    assert_eq!(c.m(A).state().stepped_down_at, None);
    c.sync(B);
    assert!(SITES.iter().all(|s| c.value(*s, "sync").is_some()));
}

/// B0-07's 40 s leaderless group: the target's vote to the old leader hung, and the other follower, which had not
/// seen the transfer, refused it as promised. With the wait, the leader sends `TimeoutNow` only once C accepted a
/// round carrying the id, so C grants the transfer vote alone.
#[test]
fn transfer_waits_for_the_other_follower_so_the_target_wins_without_the_old_leader() {
    let mut c = Cluster::new(32);
    c.elect(A);
    c.sync(A);
    c.on(A, |m, now| m.transfer(now, B)).expect("started");
    // B's vote request to A hangs throughout, as on the gray link in B0-07. C's appends are lost for 1.5 s: no
    // TimeoutNow may leave in that time, though B is caught up.
    let hung = |e: &Envelope<KvEvent>| e.from == B && e.to == A && matches!(e.msg, Message::Vote(_));
    let mut early = 0;
    for _ in 0..15 {
        c.advance(100);
        c.deliver(|e| {
            early += usize::from(matches!(e.msg, Message::TimeoutNow { .. }));
            !(e.to == C && matches!(e.msg, Message::Append(_))) && !hung(e)
        });
    }
    assert_eq!(early, 0, "TimeoutNow before C accepted the transfer id");
    c.run(3_000, 100, |e| !hung(e));
    assert_eq!(c.m(B).role(), Role::Leader, "the target did not win with C's vote");
}

/// Without the wait (the leader sending `TimeoutNow` before C saw the id), C refuses the target as promised: the
/// control for the case above, so that test proves the wait and not something else.
#[test]
fn a_voter_that_has_not_seen_the_transfer_refuses_the_target_as_promised() {
    let mut c = Cluster::new(33);
    c.elect(A);
    c.sync(A);
    let term = c.m(A).term();
    let last = (c.m(A).log().len() as u64, term);
    let id = c.on(A, |m, now| m.transfer(now, B)).expect("started");
    c.inbox.clear();
    c.on(C, |m, now| m.step(now, B, Message::Vote(vote(Term(term.0 + 1), B, false, last, Some(id)))));
    assert_eq!(vote_res(&c, C, B).and_then(|r| r.deny), Some(Deny::Promised));
}

/// A voter disregards its promise only for a transfer vote naming the transfer id of the last AppendEntries it
/// accepted.
#[test]
fn only_the_accepted_transfer_id_breaks_a_promise() {
    let mut c = Cluster::new(34);
    c.elect(A);
    c.sync(A);
    let term = c.m(A).term();
    let last = (c.m(A).log().len() as u64, term);
    let id = c.on(A, |m, now| m.transfer(now, B)).expect("started");
    c.deliver(|e| !matches!(e.msg, Message::TimeoutNow { .. }));
    assert_eq!(c.m(C).state().transfer, Some(id));
    let next = Term(term.0 + 1);
    c.inbox.clear();
    c.on(C, |m, now| m.step(now, B, Message::Vote(vote(next, B, false, last, None))));
    assert_eq!(vote_res(&c, C, B).and_then(|r| r.deny), Some(Deny::Promised), "no transfer id");
    c.on(C, |m, now| {
        m.step(now, B, Message::Vote(vote(next, B, false, last, Some(graftyard_raft::TransferId(id.0 ^ 1)))))
    });
    assert_eq!(vote_res(&c, C, B).and_then(|r| r.deny), Some(Deny::Promised), "another transfer id");
    c.on(C, |m, now| m.step(now, B, Message::Vote(vote(next, B, false, last, Some(id)))));
    assert!(vote_res(&c, C, B).is_some_and(|r| r.granted), "the accepted id");
}

/// A target that cannot be reached is retried until the LEASE runs out, then the transfer is abandoned: the leader
/// sends `transfer: None` and serves `fresh` only after a full round, never on the old lease.
#[test]
fn an_unreachable_target_is_retried_until_the_lease_then_abandoned() {
    let mut c = Cluster::new(35);
    c.elect(A);
    c.sync(A);
    c.on(A, |m, now| m.transfer(now, B)).expect("started");
    c.run(ms(LEASE) - 500, 100, |e| e.to != B);
    assert!(c.m(A).transferring().is_some(), "gave up before the LEASE");
    let mut after_abandon = Vec::new();
    let mut read = None;
    for _ in 0..10 {
        c.advance(100);
        let abandoned = c.m(A).transferring().is_none();
        if abandoned && read.is_none() {
            // A read at the moment of abandonment: the old lease is gone, so it waits for a round.
            let id = c.on(A, |m, now| m.read(now)).expect("leader");
            assert!(!c.resolutions.iter().any(|(_, r)| matches!(r, Resolution::Read { id: i, .. } if *i == id)));
            read = Some(id);
        }
        c.deliver(|e| {
            if let Message::Append(a) = &e.msg
                && abandoned
            {
                after_abandon.push(a.transfer);
            }
            e.to != B
        });
    }
    assert_eq!(c.m(A).transferring(), None, "not abandoned after one LEASE");
    assert_eq!(c.m(A).role(), Role::Leader);
    // The rounds after it carry `transfer: None`, and C took the None.
    assert!(!after_abandon.is_empty() && after_abandon.iter().all(Option::is_none), "{after_abandon:?}");
    assert_eq!(c.m(C).state().transfer, None);
    let id = read.expect("read");
    let grant = c.resolutions.iter().find_map(|(_, r)| match r {
        Resolution::Read { id: i, result: Ok(g) } if *i == id => Some(g.clone()),
        _ => None,
    });
    assert!(grant.expect("granted after a round").via_round());
}

/// A target that refuses (its own site is drained, which the leader has not learned) makes the leader abandon at
/// once, and the leader's lease from before the transfer is gone: the next read needs a round.
#[test]
fn a_refused_transfer_is_abandoned_at_once_and_leaves_no_lease() {
    let mut c = Cluster::new(38);
    c.elect(A);
    c.sync(A);
    c.on(B, |m, now| m.set_drained(now, [B]));
    c.on(A, |m, now| m.transfer(now, B)).expect("started");
    // Deliver everything until the refusal comes back, then nothing more: the round A sends on abandonment must not
    // be answered before the read.
    let mut t = 0;
    while c.m(A).transferring().is_some() && t < 4_000 {
        c.advance(100);
        let mut refused = false;
        c.deliver(|e| {
            let ok = !refused;
            refused |= matches!(e.msg, Message::TransferRefused { .. });
            ok
        });
        t += 100;
    }
    assert_eq!(c.m(A).transferring(), None, "not abandoned");
    assert!(t < ms(LEASE), "waited out the LEASE for a refusing target");
    assert_eq!(c.m(A).role(), Role::Leader);
    assert!(c.alarms().is_empty());
    let id = c.on(A, |m, now| m.read(now)).expect("leader");
    assert!(
        !c.resolutions.iter().any(|(_, r)| matches!(r, Resolution::Read { id: i, .. } if *i == id)),
        "served from a lease that predates the transfer"
    );
}

/// A target that comes back within the LEASE still takes leadership: catch-up retries do not stop at COMMIT_RETRIES.
#[test]
fn a_target_that_returns_within_the_lease_still_takes_over() {
    let mut c = Cluster::new(36);
    c.elect(A);
    c.sync(A);
    c.on(A, |m, now| m.transfer(now, B)).expect("started");
    c.run(5_000, 100, |e| e.to != B && e.from != B);
    c.run(3_000, 100, |_| true);
    assert_eq!(c.m(B).role(), Role::Leader);
}

/// A transfer vote is a promise to the target. Without it, a voter whose promise still names the old leader grants
/// the old leader a vote, and the old leader, deposed and hearing a `suspect` report before its target's first
/// append reaches that voter, wins the group straight back (liveness runs, calls seed 108).
#[test]
fn a_transfer_vote_promises_the_target() {
    let mut c = Cluster::new(39);
    c.elect(A);
    c.sync(A);
    c.on(A, |m, now| m.transfer(now, B)).expect("started");
    // Every append from B is lost (B sends appends only once it leads), so neither A nor C has heard from B as leader:
    // C's promise can come only from its transfer vote, and A knows of no leader.
    let cut = |e: &Envelope<KvEvent>| e.from == B && matches!(e.msg, Message::Append(_));
    for _ in 0..40 {
        c.advance(100);
        c.deliver(|e| !cut(e));
        if c.m(B).role() == Role::Leader {
            break;
        }
    }
    assert_eq!(c.m(B).role(), Role::Leader);
    assert_eq!(c.m(C).state().voted_for, Some(B));
    assert_eq!(c.m(C).state().promised_to, Some(B));
    let term = c.m(B).term();
    // An edge reports A, which names no leader, to A itself. A's promise named itself, so it may campaign.
    let nl = c.m(A).not_leader();
    assert_eq!(nl.leader, None);
    c.on(A, |m, now| m.suspect(now, Colo(4), A, nl.term));
    c.run(1_000, 100, |e| !cut(e));
    assert!(SITES.iter().all(|s| c.m(*s).term() == term), "the old leader started a new term and won its group back");
    assert_eq!(c.m(B).role(), Role::Leader);
}

/// A `TimeoutNow` delayed past the abandonment is ignored: its target would otherwise depose a leader whose new
/// lease rests on the target's promise.
#[test]
fn a_late_timeout_now_after_abandonment_is_ignored() {
    let mut c = Cluster::new(37);
    c.elect(A);
    c.sync(A);
    c.on(A, |m, now| m.transfer(now, B)).expect("started");
    let mut held = Vec::new();
    for _ in 0..30 {
        c.advance(100);
        c.deliver(|e| {
            if matches!(e.msg, Message::TimeoutNow { .. }) {
                held.push(e.clone());
                return false;
            }
            true
        });
    }
    assert!(!held.is_empty());
    c.run(ms(LEASE), 100, |e| !matches!(e.msg, Message::TimeoutNow { .. }));
    assert_eq!(c.m(A).transferring(), None);
    let term = c.m(A).term();
    for e in held {
        let Envelope { from, to, msg } = e;
        c.on(to, |m, now| m.step(now, from, msg));
    }
    c.run(2_000, 100, |_| true);
    assert_eq!(c.m(A).role(), Role::Leader);
    assert_eq!(c.m(A).term(), term);
}

// ---- suspect ----

fn report(c: &mut Cluster, to: SiteId, colo: u32, about: SiteId) -> Verdict {
    let term = c.m(to).term();
    c.on(to, |m, now| m.suspect(now, Colo(colo), about, term))
}

/// A report that reaches `to` while its appends from its leader are late: CALL_DEADLINE passes with nothing delivered,
/// and what queued meanwhile goes out after the report. Only such a report counts toward SUSPECT (`late-suspect`).
fn late_report(c: &mut Cluster, to: SiteId, colo: u32, about: SiteId) -> Verdict {
    c.advance(ms(graftyard_raft::CALL_DEADLINE));
    let v = report(c, to, colo, about);
    c.deliver_all();
    v
}

const LAX: u32 = u32::from_be_bytes(*b"\0LAX");
const NRT: u32 = u32::from_be_bytes(*b"\0NRT");

/// Reports from two colos within SUSPECT's window, each while the follower's appends are late, stop promises to a
/// leader that still renews, so a leader that cannot measure its own sickness still loses its lease; two colos a window
/// apart do not, nor do reports about a member that is not the follower's leader.
#[test]
fn two_colos_within_the_window_stop_promises_to_a_renewing_leader() {
    let (colos, window) = graftyard_raft::SUSPECT;
    assert_eq!(colos, 2);
    let mut c = Cluster::new(41);
    let mut id = 0;
    c.elect(A);
    c.sync(A);
    assert_eq!(late_report(&mut c, B, 1, A), Verdict::Promised);
    drive(&mut c, ms(window) + 200, true, &mut id, |_| false);
    assert_eq!(late_report(&mut c, B, 2, A), Verdict::Promised);
    let before = c.m(B).state().promise_until;
    drive(&mut c, 500, true, &mut id, |_| false);
    assert!(c.m(B).state().promise_until > before, "B stopped promising on reports a window apart");
    // A third colo inside the window of the second: B stops promising, though A keeps renewing.
    assert_eq!(late_report(&mut c, B, 3, A), Verdict::Promised);
    let stopped = c.m(B).state().promise_until;
    drive(&mut c, 3_000, true, &mut id, |_| false);
    assert_eq!(c.m(B).state().promise_until, stopped, "B promised again inside 2 x SUSPECT");
    // Reports about B, which is not C's leader, count for nothing at C.
    late_report(&mut c, C, 1, B);
    late_report(&mut c, C, 2, B);
    let c_promise = c.m(C).state().promise_until;
    drive(&mut c, 500, true, &mut id, |_| false);
    assert!(c.m(C).state().promise_until > c_promise);
}

/// A follower counts a report only while its own appends from the reported leader are late (`late-suspect`). On
/// B0-33 m44 the edge's reports came from LAX and NRT while both followers took the leader's appends every few hundred
/// ms; both withheld, and the group went through 18 terms in 5 minutes. A leader that is overloaded but still
/// appending keeps its promises and its term, even with one late moment's report from one colo in the window.
#[test]
fn an_overloaded_leader_still_appending_keeps_its_promises() {
    let mut c = Cluster::new(46);
    let mut id = 0;
    c.elect(A);
    c.sync(A);
    drive(&mut c, 1_000, true, &mut id, |_| false);
    let term = c.m(A).term();
    // 15 s of reports from two colos every 100 ms, past ELECTION_AFTER and SUSPECT's window.
    for i in 0..150 {
        for s in [B, C] {
            for colo in [LAX, NRT] {
                assert_eq!(report(&mut c, s, colo, A), Verdict::Promised, "{s:?} campaigned at {i}");
            }
        }
        // One late moment: that report counts, but alone it is one colo, and the timely ones after it add none.
        if i == 50 {
            for s in [B, C] {
                assert_eq!(late_report(&mut c, s, LAX, A), Verdict::Promised);
            }
        }
        drive(&mut c, 100, true, &mut id, |_| false);
    }
    assert_eq!(c.m(A).role(), Role::Leader);
    assert_eq!(c.m(A).term(), term, "an overloaded leader that still appends lost its term");
    for s in [B, C] {
        assert!(c.m(s).state().promise_until > c.now, "{s:?} stopped promising to a leader that still appends");
    }
    // A real late spell at B alone: two colos inside the window, so B withholds. C still promises, so A keeps leading,
    // and once A's appends are timely again B's bar ends after 2 x the window though both colos keep reporting.
    late_report(&mut c, B, LAX, A);
    late_report(&mut c, B, NRT, A);
    let stopped = c.m(B).state().promise_until;
    let (_, window) = graftyard_raft::SUSPECT;
    for _ in 0..(ms(window) * 2 / 100 + 10) {
        for colo in [LAX, NRT] {
            report(&mut c, B, colo, A);
        }
        drive(&mut c, 100, true, &mut id, |_| false);
    }
    assert_eq!(c.m(A).role(), Role::Leader);
    assert!(c.m(B).state().promise_until > stopped, "timely appends' reports kept B withholding past 2 x SUSPECT");
}

/// The gray leader `late-suspect` must still move: A's appends reach its followers only in bursts every 1.5 s, so it
/// keeps renewing their promises, while two colos report it. Reports in the late part of each gap count, both
/// followers withhold, and another member leads within the Gray region's 20 s target.
#[test]
fn a_gray_leader_whose_appends_come_late_still_loses_its_lease() {
    let mut c = Cluster::new(47);
    let mut id = 0u128;
    c.elect(A);
    c.sync(A);
    let term = c.m(A).term();
    let start = c.now.0;
    let mut held: Vec<Envelope<KvEvent>> = Vec::new();
    let mut moved = None;
    for step in 0..300 {
        c.advance(100);
        if let Some(l) = c.leader() {
            id += 1;
            let rid = RequestId(graftyard_raft::Ulid(id));
            let _ = c.on(l, |m, now| m.commit(now, rid, put("load", &id.to_string())));
        }
        if c.m(A).role() == Role::Leader {
            for s in [B, C] {
                for colo in [LAX, NRT] {
                    report(&mut c, s, colo, A);
                }
            }
        }
        c.deliver(|e| {
            let gray = e.from == A && matches!(e.msg, Message::Append(_));
            if gray {
                held.push(e.clone());
            }
            !gray
        });
        if step % 15 == 14 {
            c.inbox.append(&mut held);
            c.deliver_all();
        }
        if let Some(l) = c.leader().filter(|l| *l != A) {
            moved = Some((l, c.now.0 - start));
            break;
        }
    }
    let (l, after_ms) = moved.expect("the gray leader kept its lease for 30 s");
    assert!(c.m(l).term() > term);
    assert!(after_ms <= 20_000, "moved after {after_ms} ms, past the Gray region's 20 s");
}

/// A report from `Colo::UNKNOWN` (a site script's lookup, which runs in no edge) is no second colo: with one edge colo
/// and site-script reports beside it, B0-33 m43's followers withheld from a leader still committing and the group went
/// through 19 terms in 6 minutes. It still starts a campaign once the promise has lapsed.
#[test]
fn an_unknown_colo_is_no_second_witness_but_still_starts_a_campaign() {
    // What a site script's report carries: its lookup never sets a colo, and the member reads a missing one as 0.
    let site_script = 0;
    let mut c = Cluster::new(44);
    let mut id = 0;
    c.elect(A);
    c.sync(A);
    for unknown_first in [true, false] {
        let (one, two) = if unknown_first { (site_script, LAX) } else { (LAX, site_script) };
        assert_eq!(late_report(&mut c, B, one, A), Verdict::Promised);
        assert_eq!(late_report(&mut c, B, two, A), Verdict::Promised);
        assert_eq!(late_report(&mut c, B, site_script, A), Verdict::Promised);
        let before = c.m(B).state().promise_until;
        drive(&mut c, 500, true, &mut id, |_| false);
        assert!(c.m(B).state().promise_until > before, "B withheld on one colo plus unknown-colo reports");
    }
    // A second real colo inside the window still withholds.
    assert_eq!(late_report(&mut c, B, 7, A), Verdict::Promised);
    let stopped = c.m(B).state().promise_until;
    drive(&mut c, 500, true, &mut id, |_| false);
    assert_eq!(c.m(B).state().promise_until, stopped, "two colos no longer withhold");
    // Idle until C's promise lapses: one unknown-colo report then starts its campaign.
    c.run(ms(graftyard_raft::ELECTION_AFTER) + 200, 200, |_| true);
    assert_eq!(report(&mut c, C, site_script, A), Verdict::Started);
}

/// With both followers withholding, the leader loses its lease, and promises come back after 2 x the window.
#[test]
fn promises_return_after_twice_the_window() {
    let (_, window) = graftyard_raft::SUSPECT;
    let mut c = Cluster::new(42);
    let mut id = 0;
    c.elect(A);
    c.sync(A);
    for s in [B, C] {
        report(&mut c, s, 1, A);
        report(&mut c, s, 2, A);
    }
    let stopped = c.m(B).state().promise_until;
    drive(&mut c, ms(window) * 2 - 1_000, true, &mut id, |_| false);
    assert_eq!(c.m(B).state().promise_until, stopped);
    // A's lease is gone: a read needs a round.
    let rid = c.on(A, |m, now| m.read(now)).expect("A leads");
    c.deliver_all();
    let grant = c.resolutions.iter().find_map(|(_, r)| match r {
        Resolution::Read { id: i, result: Ok(g) } if *i == rid => Some(g.clone()),
        _ => None,
    });
    assert!(grant.expect("granted after a round").via_round(), "A served from a lease nobody promised");
    drive(&mut c, 2_000, true, &mut id, |_| false);
    assert!(c.m(B).state().promise_until > stopped, "promises did not return after 2 x SUSPECT");
}

/// A higher term restores promises, but pre-vote still refuses the reported leader for 2 x the window, so the gray
/// leader cannot win back the next election (Milestone 0 measured a 24 s move without this rule).
#[test]
fn a_higher_term_restores_promises_but_pre_vote_still_refuses_the_reported_leader() {
    let (_, window) = graftyard_raft::SUSPECT;
    let mut c = Cluster::new(43);
    c.elect(A);
    c.sync(A);
    let t0 = c.now;
    for s in [B, C] {
        report(&mut c, s, 1, A);
        report(&mut c, s, 2, A);
    }
    // Idle, so the promises lapse ELECTION_AFTER after the last append; one more report makes B campaign. B's
    // appends to C are lost for now, so C votes for B but promises it nothing.
    c.run(ms(graftyard_raft::ELECTION_AFTER) + 200, 200, |_| true);
    assert_eq!(report(&mut c, B, 3, A), Verdict::Started);
    c.deliver(|e| !(e.from == B && e.to == C && matches!(e.msg, Message::Append(_))));
    c.run(500, 100, |e| !(e.from == B && e.to == C && matches!(e.msg, Message::Append(_))));
    assert_eq!(c.m(B).role(), Role::Leader, "B did not win");
    assert!(c.now.0 < t0.0 + ms(window) * 2);
    // C's pre-vote refuses A as suspected: C promises nobody, but two colos reported A within 2 x the window.
    let term = c.m(C).term();
    let last = (c.m(A).log().len() as u64, c.m(A).log().last().unwrap().term);
    c.inbox.clear();
    c.on(C, |m, now| m.step(now, A, Message::Vote(vote(Term(term.0 + 1), A, true, last, None))));
    assert_eq!(vote_res(&c, C, A).and_then(|r| r.deny), Some(Deny::Suspected));
    // The higher term restored C's promises: B's next append gets one, inside the window that withheld them.
    c.inbox.clear();
    c.sync(B);
    assert!(c.now.0 < t0.0 + ms(window) * 2);
    assert_eq!(c.m(C).state().promised_to, Some(B));
    assert!(c.m(C).state().promise_until > c.now);
    // After 2 x the window, and with C's promise to B lapsed, C grants A's pre-vote.
    c.crash(B);
    c.run(ms(graftyard_raft::ELECTION_AFTER) + 500, 500, |_| false);
    assert!(c.now.0 > t0.0 + ms(window) * 2);
    let term = c.m(C).term();
    let last = (c.m(C).log().len() as u64, c.m(C).log().last().unwrap().term);
    c.on(C, |m, now| m.step(now, A, Message::Vote(vote(Term(term.0 + 1), A, true, last, None))));
    assert!(vote_res(&c, C, A).is_some_and(|r| r.granted));
}

/// A follower whose promise has lapsed campaigns on one report; while it runs, a report only counts.
#[test]
fn one_report_starts_a_campaign_once_the_promise_has_lapsed() {
    let mut c = Cluster::new(44);
    c.elect(A);
    c.sync(A);
    c.crash(A);
    assert_eq!(report(&mut c, B, 1, A), Verdict::Promised);
    c.run(ms(graftyard_raft::ELECTION_AFTER), 500, |_| true);
    assert_eq!(report(&mut c, B, 1, A), Verdict::Started);
    c.deliver_all();
    assert_eq!(c.m(B).role(), Role::Leader);
}

/// The `NotLeader`-with-no-leader path: a deposed leader holding the newest log is reported to as well, and its
/// followers' promises to it do not bar it, so it wins the next term.
#[test]
fn not_leader_naming_no_leader_is_reported_to_that_member_too() {
    let mut c = Cluster::new(45);
    c.elect(A);
    c.sync(A);
    // A loses its leadership without a successor: its Noop round fails after a restart.
    c.crash(A);
    c.advance(100);
    c.restart(A);
    let _ = c.on(A, |m, now| m.read(now));
    c.run(4_000, 200, |e| e.from != A);
    assert_ne!(c.m(A).role(), Role::Leader);
    let nl = c.m(A).not_leader();
    assert_eq!(nl.leader, None);
    // The edge's receipt for A turns that into reports to every voter, A included.
    let config = nl.config.clone().unwrap().1;
    let mut r = Receipt::new(RequestId(graftyard_raft::Ulid(1)), put("k", "v"), A, Colo(9));
    let reports = r.not_leader(&nl, &config);
    assert!(reports.iter().any(|s| s.to == A), "{reports:?}");
    assert_eq!(reports.len(), 3);
    // B and C still promise A, which does not bar A: A campaigns and wins.
    for s in reports {
        let v = c.on(s.to, |m, now| m.suspect(now, s.from, s.about, s.term));
        r.verdict(s.to, v);
    }
    c.run(2_000, 100, |_| true);
    assert_eq!(c.leader(), Some(A));
}

// ---- Health and STEP_DOWN ----

/// Fails `per_mille` of the requests (appends and pings) that touch `site`, drawn from a fixed sequence.
fn gray(site: SiteId, per_mille: u64, seed: u64) -> impl FnMut(&Envelope<KvEvent>) -> bool {
    let mut rng = common::sim::Rng::new(seed);
    move |e| request(e) && (e.from == site || e.to == site) && rng.chance(per_mille)
}

/// A leader whose calls fail 30% trips the commit-error rule, the other follower's probe qualifies a target, and
/// the leader transfers there: never onto a sick site, and it records `stepped_down_at`.
#[test]
fn commit_errors_trip_and_the_probe_picks_a_healthy_target() {
    let mut c = Cluster::new(51);
    let mut id = 0;
    c.elect(A);
    drive(&mut c, 3_000, true, &mut id, |_| false);
    drive(&mut c, 15_000, true, &mut id, gray(A, 300, 1));
    let sd = step_downs(&c);
    let (site, why, to) = sd.first().cloned().expect("STEP_DOWN tripped");
    assert_eq!(site, A);
    let StepDownWhy::CommitErrors { calls } = why else { panic!("{why:?}") };
    assert!(calls.iter().all(|(_, n, f)| *f >= 3 && *f as f32 / *n as f32 > 0.05), "{calls:?}");
    assert!(matches!(to, Some(t) if t != A));
    assert_ne!(c.leader(), Some(A));
    assert!(c.m(A).state().stepped_down_at.is_some());
}

/// One or two failed calls per follower never trip the commit-error rule, whatever the rate (`commit_fail_min`,
/// F37: a far follower misses CALL_DEADLINE about once in 10 to 15 calls with no fault).
#[test]
fn fewer_than_commit_fail_min_failures_never_trip() {
    let mut c = Cluster::new(52);
    let mut id = 0;
    c.elect(A);
    drive(&mut c, 1_000, false, &mut id, |_| false);
    // With no load, one call per follower a second: 2 of every ~10 fail, a 20% rate, but only 2 a window.
    let mut n = 0u32;
    drive(&mut c, 30_000, false, &mut id, |e| {
        if !matches!(e.msg, Message::Append(_)) || e.from != A {
            return false;
        }
        n += 1;
        n.is_multiple_of(13)
    });
    assert!(step_downs(&c).is_empty(), "{:?}", step_downs(&c));
    assert_eq!(c.leader(), Some(A));
}

/// The confirm rule: six confirms over 500 ms in the window move the leader, with no probe, to a follower whose
/// calls from it fail under `commit_errors`. Five never do (a healthy object reached 4 in Milestone 0).
#[test]
fn six_slow_confirms_trip_and_five_do_not() {
    for (slow, trips) in [(5u32, false), (6, true)] {
        let mut c = Cluster::new(53);
        let mut id = 0;
        c.elect(A);
        drive(&mut c, 2_000, true, &mut id, |_| false);
        for _ in 0..slow {
            c.slow_confirm(A, 750);
            c.deliver_all();
        }
        drive(&mut c, 3_000, true, &mut id, |_| false);
        let sd = step_downs(&c);
        assert_eq!(!sd.is_empty(), trips, "{slow} slow confirms: {sd:?}");
        if trips {
            assert!(matches!(sd[0].1, StepDownWhy::Confirm { slow: 6, .. }), "{sd:?}");
            assert!(!c.inbox.iter().any(|e| matches!(e.msg, Message::Probe { .. })));
            assert_ne!(c.leader(), Some(A));
        }
    }
}

/// LOOP_LAG: two confirms within 2 s that read whole ticks of 200 ms or more move the leader, as a confirm trip does.
/// One does not, nor do two that are not whole ticks, nor two whole ticks of 100 ms, nor two 3 s apart
/// (`tests/m0/results/carried-8.md`).
#[test]
fn two_clamped_confirms_trip_loop_lag() {
    let cases = [(300u64, 1, 0, false), (250, 2, 0, false), (100, 2, 0, false), (300, 2, 3_000, false)];
    for (ms, n, gap, trips) in cases.into_iter().chain([(200, 2, 0, true), (700, 2, 0, true), (300, 2, 1_000, true)]) {
        let mut c = Cluster::new(53);
        let mut id = 0;
        c.elect(A);
        drive(&mut c, 2_000, true, &mut id, |_| false);
        for i in 0..n {
            if i > 0 && gap > 0 {
                drive(&mut c, gap, true, &mut id, |_| false);
            }
            c.slow_confirm(A, ms);
            c.deliver_all();
        }
        drive(&mut c, 3_000, true, &mut id, |_| false);
        let sd = step_downs(&c);
        assert_eq!(!sd.is_empty(), trips, "{n} confirms of {ms} ms, {gap} ms apart: {sd:?}");
        if trips {
            assert!(matches!(sd[0].1, StepDownWhy::LoopLag { clamped: 2 }), "{sd:?}");
            assert!(sd[0].2.is_some(), "held: {sd:?}");
            assert_ne!(c.leader(), Some(A));
        }
    }
}

/// A confirm trip disqualifies a target only on a link that fails `commit_fail_min` times or more, as the commit-error
/// rule counts one: one or two missed calls in a quiet window pass 5% on noise alone (F37). In B0-02's r330-9 a leader
/// storage stall tripped `Confirm { p99_ms: 1042, slow: 6, of: 21 }` and held with `to: None`, and the stall stayed on
/// the leader for the rest of the run (`tests/m0/results/carried-2.md`). Two failed calls to each follower now
/// transfer, and three to B move the leader to C.
#[test]
fn a_confirm_trip_ignores_fewer_than_commit_fail_min_failures_to_a_target() {
    for (to_b, to_c, target) in [(2u32, 2u32, B), (3, 2, C)] {
        let mut c = Cluster::new(53);
        let mut id = 0;
        c.elect(A);
        drive(&mut c, 1_000, true, &mut id, |_| false);
        let (mut nb, mut nc) = (0, 0);
        drive(&mut c, 500, true, &mut id, |e| {
            if !request(e) || e.from != A {
                return false;
            }
            let (n, max) = if e.to == B { (&mut nb, to_b) } else { (&mut nc, to_c) };
            *n += 1;
            *n <= max
        });
        for _ in 0..6 {
            c.slow_confirm(A, 750);
            c.deliver_all();
        }
        drive(&mut c, 1_000, true, &mut id, |_| false);
        let sd = step_downs(&c);
        let (_, why, to) = sd.first().cloned().expect("STEP_DOWN tripped");
        assert!(matches!(why, StepDownWhy::Confirm { .. }), "{to_b}/{to_c}: {sd:?}");
        assert_eq!(to, Some(target), "{to_b} failed calls to B, {to_c} to C: {sd:?}");
    }
}

/// The own-stall exclusion: calls whose deadline elapsed while the leader's own storage confirm was outstanding count
/// toward neither `commit_errors` nor `commit_fail_min`. In LF-12 one 3.5 s confirm made every failed call to the
/// near follower in the window, and the commit-error rule moved a healthy group.
#[test]
fn deadline_misses_during_the_leaders_own_stall_are_excused() {
    let run = |excuse: bool| {
        let mut c = Cluster::new(54);
        let mut id = 0;
        c.elect(A);
        drive(&mut c, 3_000, true, &mut id, |_| false);
        // One 3.5 s confirm on A. Every call A sends meanwhile misses its deadline.
        let now = c.now;
        let cid = c.members[A.0 as usize].as_mut().unwrap().confirm_begin(now);
        let mut stalled = Vec::new();
        for _ in 0..35 {
            c.advance(100);
            id += 1;
            let rid = RequestId(graftyard_raft::Ulid(id));
            let _ = c.on(A, |m, now| m.commit(now, rid, put("load", "x")));
            stalled.extend(c.inbox.drain(..).filter(|e| e.from == A && request(e)));
            c.deliver_all();
        }
        c.on(A, |m, now| m.confirm_end(now, cid));
        for e in stalled {
            // `excuse` false reports the misses as errors, which the exclusion does not cover: the control.
            let f = e.failure(excuse).unwrap();
            c.on(A, |m, now| m.call_failed(now, f));
        }
        drive(&mut c, 2_000, true, &mut id, |_| false);
        step_downs(&c)
    };
    let excused = run(true);
    assert!(excused.iter().all(|s| !matches!(s.1, StepDownWhy::CommitErrors { .. })), "{excused:?}");
    // The control: the same failures as errors trip the commit-error rule.
    let counted = run(false);
    assert!(counted.iter().any(|s| matches!(s.1, StepDownWhy::CommitErrors { .. })), "{counted:?}");
}

/// One stall of a follower counts once per CALL_DEADLINE, however many rounds it missed (LM-1: a 1.9 s stall missed
/// 17 to 31 per-read rounds at once and moved healthy identity leaders). Misses sent a deadline or more apart still
/// count, so a follower that keeps failing still trips the rule.
#[test]
fn one_follower_stall_counts_once_per_deadline() {
    let run = |reads: u32, gap_ms: u64| {
        let mut c = Cluster::new(56);
        let mut id = 0;
        c.elect(A);
        drive(&mut c, 3_000, true, &mut id, |_| false);
        // An idle leader's lease lapses, so each read starts a round to each follower; every one misses its deadline.
        c.advance(graftyard_raft::LEASE.as_millis() as u64);
        c.deliver_all();
        c.inbox.clear();
        let mut misses = Vec::new();
        for _ in 0..reads {
            c.now = Timestamp(c.now.0 + gap_ms);
            let _ = c.on(A, |m, now| m.read(now));
            misses.extend(c.inbox.drain(..).filter(|e| e.from == A && request(e)));
        }
        c.now = Timestamp(c.now.0 + 1_000);
        for e in misses {
            let f = e.failure(true).unwrap();
            c.on(A, |m, now| m.call_failed(now, f));
        }
        // A commit-error trip first has the other follower probe each target.
        drive(&mut c, 2_000, false, &mut id, |_| false);
        step_downs(&c)
    };
    // One stall: 30 rounds sent within 300 ms count as one miss per follower, under `commit_fail_min`.
    let stall = run(30, 10);
    assert!(stall.iter().all(|s| !matches!(s.1, StepDownWhy::CommitErrors { .. })), "{stall:?}");
    // The control: 5 misses a deadline apart are 5 failures per follower and trip the rule.
    let failing = run(5, graftyard_raft::CALL_DEADLINE.as_millis() as u64);
    assert!(failing.iter().any(|s| matches!(s.1, StepDownWhy::CommitErrors { .. })), "{failing:?}");
}

/// A new leader counts a follower's failed calls only once that follower answered it in its term: a killed leader
/// still resetting is not a sign of the new leader's sickness (7 of 60 groups changed again without this).
#[test]
fn a_new_leader_does_not_count_a_follower_it_has_not_heard_from() {
    let mut c = Cluster::new(55);
    let mut id = 0;
    c.elect(A);
    c.sync(A);
    c.crash(A);
    c.elect_after_promises(B);
    // A is down for 15 s; every call to it fails. C misses 3 calls in the window too.
    let mut n = 0;
    drive(&mut c, 15_000, true, &mut id, |e| {
        if e.to == A && request(e) {
            return true;
        }
        n += 1;
        e.to == C && request(e) && n % 7 == 0
    });
    assert!(step_downs(&c).is_empty(), "{:?}", step_downs(&c));
    assert_eq!(c.leader(), Some(B));
}

/// Calls into A fail 40% (A's own pings out work, as for a site whose writes fail), and the B-C link fails 12%: the
/// healthy pair's link is over 5% only through background misses, as in Milestone 0 F35.
fn into_a_and_b_c(seed: u64) -> impl FnMut(&Envelope<KvEvent>) -> bool {
    let mut r = common::sim::Rng::new(seed);
    move |e| request(e) && (e.to == A && r.chance(400) || (matches!((e.from, e.to), (B, C) | (C, B)) && r.chance(120)))
}

/// Two healthy sites passing a group back and forth while a third is sick (Milestone 0 F35, `f35-slow3`: 6 changes,
/// and 11 at 10% slow writes). A leaves by the confirm rule, B moves the group to C on commit errors, and C must then
/// hold, because B stepped down within RESET_QUARANTINE; without the rule C's probe of B (from A) reads 0.00.
#[test]
fn a_stepped_down_member_is_not_a_target_so_healthy_sites_do_not_ping_pong() {
    let mut c = Cluster::new(56);
    let mut id = 0;
    c.elect(A);
    drive(&mut c, 3_000, true, &mut id, |_| false);
    for _ in 0..6 {
        c.slow_confirm(A, 750);
        c.deliver_all();
    }
    drive(&mut c, 3_000, true, &mut id, |_| false);
    assert_eq!(c.leader(), Some(B), "{:?}", step_downs(&c));
    drive(&mut c, 60_000, true, &mut id, into_a_and_b_c(3));
    let sd = step_downs(&c);
    let moves: Vec<(SiteId, SiteId)> = sd.iter().filter_map(|s| s.2.map(|t| (s.0, t))).collect();
    assert_eq!(moves, vec![(A, B), (B, C)], "{sd:?}");
    assert!(sd.iter().filter(|s| s.0 == C).all(|s| s.2.is_none()), "C moved: {sd:?}");
    assert!(sd.iter().any(|s| s.0 == C), "C never tripped, so the case was not reached: {sd:?}");
    assert_eq!(c.leader(), Some(C));
}

/// The transfer onto the sick site (Milestone 0 `tgt-writes1`; B0-33 p4r4 cost 6,188 `suspect` reports): B's calls to
/// A fail, and to C just over 5%. Judged on B's own calls neither qualifies, and judged on each target's calls to the
/// other follower only A did. The other follower's probe reads `c>a` high and `a>c` 0.00, so B moves to C
/// (`dr-writes2`).
#[test]
fn the_probe_picks_the_healthy_site_never_the_sick_one() {
    let mut c = Cluster::new(57);
    let mut id = 0;
    c.elect(B);
    drive(&mut c, 3_000, true, &mut id, |_| false);
    drive(&mut c, 20_000, true, &mut id, into_a_and_b_c(9));
    let sd = step_downs(&c);
    let (site, _, to) = sd.first().cloned().expect("B never tripped");
    assert_eq!((site, to), (B, Some(C)), "{sd:?}");
    assert!(sd.iter().all(|s| s.2 != Some(A)), "transferred onto the sick site: {sd:?}");
    assert_eq!(c.leader(), Some(C));
}

/// A sick prober can refuse a healthy target, and then the leader holds; it never transfers onto the sick site.
/// Here A fails both ways, so A's probe of C fails too (`dr-calls1` `-g2`).
#[test]
fn a_sick_prober_makes_the_leader_hold() {
    let mut c = Cluster::new(60);
    let mut id = 0;
    c.elect(B);
    drive(&mut c, 3_000, true, &mut id, |_| false);
    let mut r = common::sim::Rng::new(9);
    let mut f = |e: &Envelope<KvEvent>| {
        request(e) && ((e.from == A || e.to == A) && r.chance(400) || (e.from == B && e.to == C && r.chance(100)))
    };
    drive(&mut c, 30_000, true, &mut id, &mut f);
    let sd = step_downs(&c);
    assert!(!sd.is_empty(), "B never tripped");
    assert!(sd.iter().all(|s| s.2.is_none()), "{sd:?}");
    assert_eq!(c.leader(), Some(B));
}

/// A sick prober can refuse a healthy target, and then the leader holds; it never transfers onto the sick site.
/// And when every site fails alike (the June 2025 class), no probe reads under `commit_errors`, so it holds too.
#[test]
fn the_leader_holds_when_no_probe_qualifies_a_target() {
    let mut c = Cluster::new(58);
    let mut id = 0;
    c.elect(A);
    drive(&mut c, 3_000, true, &mut id, |_| false);
    // Every link fails 30%.
    let mut r = common::sim::Rng::new(11);
    drive(&mut c, 15_000, true, &mut id, |e| request(e) && r.chance(300));
    let sd = step_downs(&c);
    assert!(!sd.is_empty(), "never tripped");
    assert!(sd.iter().all(|s| s.2.is_none()), "transferred while every site fails: {sd:?}");
    assert_eq!(c.leader(), Some(A));
    // The hold lasts one window: trips are at least `window_s` apart.
    let n = sd.len();
    assert!(n <= 2, "{n} trips in 15 s: the hold did not hold");
}

/// A healthy group under load never trips.
#[test]
fn a_healthy_leader_never_trips() {
    let mut c = Cluster::new(59);
    let mut id = 0;
    c.elect(A);
    drive(&mut c, 60_000, true, &mut id, |_| false);
    assert!(step_downs(&c).is_empty());
    assert_eq!(c.leader(), Some(A));
}

// ---- home, drain, and led_term ----

/// A group elected away from its `home` (A) transfers back once A has recovered, answered a window of calls with
/// none failed, and holds every committed entry.
#[test]
fn leadership_returns_home_after_it_recovers_and_catches_up() {
    let mut c = Cluster::new(61);
    let mut id = 0;
    c.elect(A);
    c.sync(A);
    c.crash(A);
    c.elect_after_promises(B);
    drive(&mut c, 15_000, true, &mut id, |_| false);
    assert_eq!(c.leader(), Some(B), "moved home while A was down");
    c.restart(A);
    // A is back but failing some calls: not recovered, so no transfer home.
    let mut r = common::sim::Rng::new(3);
    drive(&mut c, 15_000, true, &mut id, |e| request(e) && e.to == A && r.chance(200));
    assert_eq!(c.leader(), Some(B), "moved home onto a failing site");
    drive(&mut c, 15_000, true, &mut id, |_| false);
    assert_eq!(c.leader(), Some(A), "did not return home");
    // Nothing committed was lost on the way.
    c.sync(A);
    assert!(SITES.iter().all(|s| c.value(*s, "sync").is_some()));
}

/// A `home` that stepped down within RESET_QUARANTINE is not transferred back to.
#[test]
fn a_quarantined_home_gets_no_transfer() {
    let mut c = Cluster::new(62);
    let mut id = 0;
    c.elect(A);
    drive(&mut c, 2_000, true, &mut id, |_| false);
    for _ in 0..6 {
        c.slow_confirm(A, 750);
        c.deliver_all();
    }
    drive(&mut c, 30_000, true, &mut id, |_| false);
    assert!(c.m(A).state().stepped_down_at.is_some());
    assert_ne!(c.leader(), Some(A), "returned home inside RESET_QUARANTINE");
}

/// A restarted leader resumes `led_term` after one round, and a resumed leader away from home still goes home.
#[test]
fn a_resumed_led_term_still_goes_home() {
    let mut c = Cluster::new(63);
    let mut id = 0;
    c.elect(A);
    c.sync(A);
    c.crash(A);
    c.elect_after_promises(B);
    let term = c.m(B).term();
    c.restart(A);
    drive(&mut c, 2_000, true, &mut id, |_| false);
    c.crash(B);
    c.advance(200);
    c.restart(B);
    assert_eq!(c.m(B).state().led_term, Some(term));
    // The next request resumes B's term with no election.
    id += 1;
    let rid = RequestId(graftyard_raft::Ulid(id));
    c.on(B, |m, now| m.commit(now, rid, put("k", "v"))).expect("resumed");
    c.deliver_all();
    assert_eq!(c.m(B).term(), term);
    drive(&mut c, 15_000, true, &mut id, |_| false);
    assert_eq!(c.leader(), Some(A));
}

/// A leader on a drained site hands off within one LEASE, a drained member refuses a transfer (so the leader
/// abandons at once), and pre-vote refuses a drained candidate.
#[test]
fn a_drained_site_hands_off_and_takes_no_leadership() {
    let mut c = Cluster::new(64);
    let mut id = 0;
    c.elect(A);
    c.sync(A);
    for s in SITES {
        c.on(s, |m, now| m.set_drained(now, [A, B]));
    }
    // B is drained too, so A must pick C.
    drive(&mut c, ms(LEASE), true, &mut id, |_| false);
    assert_eq!(c.leader(), Some(C));
    // A transfer to drained B is refused.
    assert!(matches!(c.on(C, |m, now| m.transfer(now, B)), Err(TransferError::Target(t)) if t == B));
    // Pre-vote refuses drained A.
    c.crash(C);
    c.run(ms(graftyard_raft::ELECTION_AFTER) + 100, 500, |_| false);
    let term = c.m(B).term();
    let last = (c.m(B).log().len() as u64, c.m(B).log().last().unwrap().term);
    c.inbox.clear();
    c.on(B, |m, now| m.step(now, A, Message::Vote(vote(Term(term.0 + 1), A, true, last, None))));
    assert_eq!(vote_res(&c, B, A).and_then(|r| r.deny), Some(Deny::Drained));
    assert_eq!(report(&mut c, A, 1, C), Verdict::MayNotLead);
}

// ---- release-aware refusals (`verify`) ----

fn release(s: SiteId, seq: u32, reads_future: bool) -> graftyard_raft::MemberConfig {
    let mut cfg = config(s);
    cfg.release = ReleaseTag { id: ReleaseId([seq as u8; 32]), seq };
    cfg.reads = if reads_future { vec![KV_FORMAT, FUTURE_FORMAT] } else { vec![KV_FORMAT] };
    cfg
}

/// Restarts each member on the given release, then elects A and lets the group settle.
fn on_releases(seed: u64, rel: [(u32, bool); 3]) -> Cluster {
    let mut c = Cluster::new(seed);
    c.elect(A);
    c.sync(A);
    for (s, (seq, fut)) in SITES.iter().zip(rel) {
        c.crash(*s);
        c.advance(6_000);
        c.restart_with(*s, release(*s, seq, fut));
    }
    c.inbox.clear();
    let _ = c.on(A, |m, now| m.campaign(now));
    c.run(3_000, 100, |_| true);
    if c.leader() != Some(A) {
        c.elect_after_promises(A);
    }
    c.sync(A);
    c
}

fn future(c: &mut Cluster, id: u128) -> graftyard_raft::Ticket {
    c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(id)), KvCmd::Future)).expect("admitted").ticket
}

/// An older follower refuses an uncommitted entry: the leader truncates its uncommitted tail, persists
/// `no_lead_until`, and transfers; the tickets answer `NotLeader` and nothing unreadable commits. The bar clears when
/// the leader's release changes.
#[test]
fn an_older_refuser_makes_the_leader_truncate_and_hand_off() {
    let mut c = on_releases(71, [(2, true), (1, false), (1, false)]);
    let commit = c.m(A).state().commit;
    let t = future(&mut c, 900);
    c.run(4_000, 100, |_| true);
    assert!(c.alarms().iter().any(|(s, a)| *s == A && matches!(a, Alarm::NoLead { .. })), "{:?}", c.alarms());
    assert_eq!(c.m(A).state().no_lead_until, Some(ReleaseId([2; 32])));
    assert!(
        c.ticket_results(t)
            .iter()
            .any(|r| matches!(r, Resolution::Ticket { result: Err(CommitError::NotLeader(_)), .. }))
    );
    assert!(c.m(A).log().iter().all(|e| e.format != FUTURE_FORMAT), "the refused entry is still in A's log");
    assert!(SITES.iter().all(|s| c.m(*s).state().commit >= commit));
    let l = c.leader().expect("a new leader");
    assert_ne!(l, A);
    // A neither leads nor campaigns while the bar holds.
    let nl = c.m(A).not_leader();
    assert_ne!(nl.leader, Some(A));
    // A restart on a new release clears it.
    c.crash(A);
    c.advance(6_000);
    c.restart_with(A, release(A, 3, true));
    assert_eq!(c.m(A).state().no_lead_until, None);
}

/// Both followers on the leader's own release refuse: its own defect. It truncates, alarms, and keeps leading.
#[test]
fn both_same_release_refusers_are_the_leaders_own_defect() {
    let mut c = on_releases(72, [(1, true), (1, false), (1, false)]);
    let t = future(&mut c, 900);
    c.run(4_000, 100, |_| true);
    assert!(c.alarms().iter().any(|(_, a)| matches!(a, Alarm::OwnDefect { .. })), "{:?}", c.alarms());
    assert!(
        c.ticket_results(t)
            .iter()
            .any(|r| matches!(r, Resolution::Ticket { result: Err(CommitError::NotLeader(_)), .. }))
    );
    assert_eq!(c.leader(), Some(A));
    assert!(c.m(A).state().cannot_follow.is_empty());
    c.sync(A);
    assert!(SITES.iter().all(|s| c.value(*s, "sync").is_some()));
}

/// One same-or-newer refuser while the other follower stores the entry: it commits, the leader keeps leading, adds
/// the refuser to `cannot_follow` and alarms. The refuser leaves `cannot_follow` once its release changes and it
/// stores the leader's entries.
#[test]
fn a_same_release_refuser_of_a_committed_entry_cannot_follow_until_its_release_changes() {
    let mut c = on_releases(73, [(1, true), (1, false), (2, true)]);
    let t = future(&mut c, 900);
    c.run(4_000, 100, |_| true);
    assert!(c.ticket_results(t).iter().any(|r| matches!(r, Resolution::Ticket { result: Ok(_), .. })));
    assert!(c.alarms().iter().any(|(_, a)| matches!(a, Alarm::Refused { site, .. } if *site == B)));
    assert_eq!(c.m(A).state().cannot_follow.as_slice(), &[B]);
    assert_eq!(c.leader(), Some(A));
    c.crash(B);
    c.advance(1_000);
    c.restart_with(B, release(B, 2, true));
    c.run(12_000, 500, |_| true);
    assert!(c.m(A).state().cannot_follow.is_empty());
    assert_eq!(c.m(B).log().len(), c.m(A).log().len());
}

// ---- BF-14 ----

/// A missed receipt yields `suspect` reports, which carry no command, and leaves the receipt's one target as it was:
/// the API has no way to send the command to a second member. The target moves only on a member's answer.
#[test]
fn bf14_a_missed_receipt_is_reported_never_forwarded() {
    let mut c = Cluster::new(81);
    c.elect(A);
    c.sync(A);
    let config = c.m(B).committed_config().unwrap().1.clone();
    let mut r = Receipt::new(RequestId(graftyard_raft::Ulid(7)), put("k", "v"), A, Colo(1));
    for _ in 0..5 {
        let reports = r.missed(&config);
        assert_eq!(reports.iter().map(|s| s.to).collect::<Vec<_>>(), vec![B, C]);
        assert!(reports.iter().all(|s| s.about == A));
        assert_eq!(r.call().0, A, "a missed receipt moved the command");
    }
    // A `Promised` verdict from a follower does not move it either; a `Leader` verdict does.
    r.verdict(B, Verdict::Promised);
    assert_eq!(r.target(), A);
    r.verdict(B, Verdict::Leader);
    assert_eq!(r.target(), B);
}

/// The group's `home` is the latest config's, not the bootstrap record's: after a config change re-homes the group on
/// B (an owner move), a leader transferred to B stays there, and a leader on A goes to B. Found on Durable Objects
/// (BF-11): the leader read `home` from its record and pulled leadership back to A after every move.
#[test]
fn home_follows_the_config_not_the_bootstrap_record() {
    use graftyard_raft::GroupConfig;
    let mut c = Cluster::new(64);
    let mut id = 0;
    c.elect(A);
    c.sync(A);
    let Some((_, GroupConfig::Normal(p))) = c.m(A).committed_config().cloned() else { panic!("normal config") };
    use graftyard_types::{Placement, PlacementChecks, Region, Rtt, Site, SiteTable};
    let site = |id: SiteId, region| Site {
        id,
        script: String::new(),
        account: "x".into(),
        region,
        jurisdiction: None,
        bucket: String::new(),
        release: None,
        link: graftyard_types::SiteLink::Binding { member_class: "Member".into() },
        spare: false,
    };
    let table = SiteTable(vec![site(A, Region::Enam), site(B, Region::Weur), site(C, Region::Apac)]);
    let rtt: Vec<(SiteId, Rtt)> =
        [(A, B), (A, C), (B, C)].iter().map(|&(x, y)| (x, Rtt { peer: y, median_ms: 80, at: Timestamp(0) })).collect();
    let checks = PlacementChecks { sites: &table, separate_accounts: false, rtt: &rtt, near: &[] };
    let rehomed = Placement::new(p.voters().iter().cloned().collect(), Default::default(), B, &checks).unwrap();
    c.on(A, |m, now| m.reconfigure(now, GroupConfig::Normal(Box::new(rehomed)))).unwrap();
    c.deliver_all();
    c.sync(A);
    drive(&mut c, 30_000, true, &mut id, |_| false);
    assert_eq!(c.leader(), Some(B), "a leader on A did not go to the config's home B");
    drive(&mut c, 30_000, true, &mut id, |_| false);
    assert_eq!(c.leader(), Some(B), "leadership left the config's home");
}

// ---- F67: a leaderless group with requests waiting ----

/// Check quorum: A's AppendEntries reach B and C but every answer is lost (a half-hung site), so B and C keep
/// renewing their promises to A while A commits nothing. A must stop leading within ELECTION_AFTER of the write it
/// cannot commit, so the promises lapse and a `suspect` report elects B. Before the fix A led forever and every report
/// answered `Promised` (`tests/m0/results/f67.md`, path 1).
#[test]
fn a_leader_no_quorum_answers_stops_so_the_followers_can_elect() {
    let mut c = Cluster::new(67);
    c.elect(A);
    c.sync(A);
    let _ = c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(1)), put("k", "v")));
    let half_hung = |e: &Envelope<KvEvent>| !(e.to == A && matches!(e.msg, Message::AppendRes(_)));
    c.run(ms(graftyard_raft::ELECTION_AFTER) + 500, 100, half_hung);
    assert_ne!(c.m(A).role(), Role::Leader, "A still leads with no quorum answering");
    // Its followers' promises lapse once A's appends stop; one report then elects B.
    c.run(ms(graftyard_raft::ELECTION_AFTER), 100, half_hung);
    let term = c.m(B).term();
    assert_eq!(c.on(B, |m, now| m.suspect(now, Colo(1), A, term)), Verdict::Started);
    c.run(2_000, 100, half_hung);
    assert_eq!(c.leader(), Some(B));
}

/// An idle leader whose answers are lost keeps leading: check quorum holds only while a write, read, or transfer
/// waits on a quorum, so a group with no requests is never moved (campaigns stay request-driven).
#[test]
fn an_idle_leader_with_lost_answers_keeps_leading() {
    let mut c = Cluster::new(68);
    c.elect(A);
    c.sync(A);
    c.run(3 * ms(graftyard_raft::ELECTION_AFTER), 500, |e| !(e.to == A && matches!(e.msg, Message::AppendRes(_))));
    assert_eq!(c.m(A).role(), Role::Leader);
}

/// A follower's read-index batch the leader never answers counts as the read-index call failing: the follower names
/// no leader, so the edge's next read reports `suspect`, and with its promise lapsed it campaigns at once. Before the
/// fix the follower kept naming A, its reads shed with no report, and a group read only by read-index never elected
/// (`tests/m0/results/f67.md`, path 2).
#[test]
fn an_unanswered_read_index_batch_forgets_the_leader_and_campaigns() {
    let mut c = Cluster::new(69);
    c.elect(A);
    c.sync(A);
    c.crash(A);
    c.advance(ms(graftyard_raft::ELECTION_AFTER) + 1);
    assert_eq!(c.m(B).leader(), Some(A));
    c.on(B, |m, now| m.read_index(now)).expect("queued for A");
    c.run(4_000, 100, |_| true);
    assert_eq!(c.leader(), Some(B), "B never campaigned on its failed read-index call");
}

/// A transfer vote promises the target only for the campaign's own deadline: a target that cannot win (here its
/// requests to C are lost and A has stepped aside) leaves C free to vote within seconds, not ELECTION_AFTER. Before
/// the fix C refused every other candidate as `Promised` for 10 s while nothing led (`tests/m0/results/f67.md`,
/// path 3).
#[test]
fn a_transfer_vote_promises_the_target_only_for_its_campaign() {
    let mut c = Cluster::new(70);
    c.elect(A);
    c.sync(A);
    c.on(A, |m, now| m.transfer(now, B)).expect("started");
    // B's campaign reaches C, which grants it; C's answer is lost, and A's answer too, so B cannot win.
    let lost = |e: &Envelope<KvEvent>| !(e.to == B && matches!(e.msg, Message::VoteRes(_)));
    c.run(3_000, 100, lost);
    assert!(c.m(C).state().promised_to == Some(B) || c.m(B).role() != Role::Leader);
    assert_ne!(c.m(B).role(), Role::Leader);
    // B's campaign ended; C has granted it nothing more, and within a few seconds C votes for A again.
    c.crash(B);
    c.run(3_000, 100, |_| true);
    let term = c.m(C).term();
    let v = c.on(C, |m, now| m.suspect(now, Colo(1), B, term));
    c.run(3_000, 100, |_| true);
    assert!(c.leader() == Some(C) || c.leader() == Some(A), "{v:?}: nobody led 6 s after a failed transfer");
}

/// Both followers refuse an entry (the leader's own defect), so it truncates it, keeps leading, and appends another
/// entry at that index in the same term, which commits. A round sent before the truncation, carrying the refused
/// entry, may arrive after that commit: the follower must refuse it as an older leader instance's, not delete the
/// committed entry (found by the harsh sweep, seed 3013, once id blocks re-sent across a leader restart were refused).
#[test]
fn a_round_from_before_an_own_defect_truncation_cannot_touch_a_commit() {
    let mut c = on_releases(73, [(1, true), (1, false), (1, false)]);
    let _ = future(&mut c, 900);
    // A copy of the first round to B that carries the refused entry is delayed (the network duplicated it); the
    // original arrives.
    let mut held = Vec::new();
    for _ in 0..40 {
        c.advance(100);
        if held.is_empty()
            && let Some(e) = c.inbox.iter().find(|e| {
                e.to == B && matches!(&e.msg, Message::Append(a) if a.entries.iter().any(|x| x.format == FUTURE_FORMAT))
            })
        {
            held.push(e.clone());
        }
        c.deliver_all();
    }
    assert!(c.alarms().iter().any(|(_, a)| matches!(a, Alarm::OwnDefect { .. })), "{:?}", c.alarms());
    assert_eq!(held.len(), 1, "no round with the refused entry was held");
    // Two writes: B learns the first one's commit from the round that carries the second.
    for id in [901, 902] {
        c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(id)), put("k", "v"))).expect("admitted");
        c.run(2_000, 100, |_| true);
    }
    let commit = c.m(B).state().commit;
    let refused_at = match &held[0].msg {
        Message::Append(a) => a.entries.iter().find(|x| x.format == FUTURE_FORMAT).map(|x| x.index).expect("held"),
        _ => unreachable!(),
    };
    assert!(commit >= refused_at, "B has not committed the replacement at {refused_at:?}");
    let digest = c.m(B).digests()[refused_at.0 as usize - 1];
    c.inbox.extend(held);
    c.deliver_all();
    assert_eq!(c.m(B).digests()[refused_at.0 as usize - 1], digest, "B replaced a committed entry");
}

/// A transfer target that a voter elected but never heard from (its appends to that voter are lost) is not that
/// voter's leader for `suspect`: reports about it, sent while it was still a candidate, must not bar it in pre-vote.
/// Before the fix C refused B as `Suspected` for 2 x SUSPECT's window while B alone held the newest entry and refused
/// every other candidate as `LogBehind`, so nobody led for 17 s with requests waiting (F67, liveness calls seed 107).
#[test]
fn reports_about_an_unheard_transfer_target_do_not_bar_it() {
    let mut c = Cluster::new(74);
    c.elect(A);
    c.sync(A);
    c.on(A, |m, now| m.transfer(now, B)).expect("started");
    let cut = |e: &Envelope<KvEvent>| e.from == B && e.to == C && matches!(e.msg, Message::Append(_));
    for _ in 0..40 {
        c.advance(100);
        c.deliver(|e| !cut(e));
        if c.m(B).role() == Role::Leader {
            break;
        }
    }
    assert_eq!(c.m(B).role(), Role::Leader);
    assert_eq!(c.m(C).state().promised_to, Some(B));
    assert_eq!(c.m(C).leader(), None, "C heard from B");
    let term = c.m(C).term();
    for colo in [1, 2] {
        c.on(C, |m, now| m.suspect(now, Colo(colo), B, term));
    }
    let last = (c.m(B).log().len() as u64, c.m(B).log().last().unwrap().term);
    c.inbox.clear();
    c.on(C, |m, now| m.step(now, B, Message::Vote(vote(Term(term.0 + 1), B, true, last, None))));
    assert_ne!(vote_res(&c, C, B).and_then(|r| r.deny), Some(Deny::Suspected));
}

/// F67's idle shape: the member holding `led_term` restarts as a follower (an idle eviction), so all three are
/// followers with no leader. Campaigns are request-driven, so one ordinary request must elect, with the edge's
/// reports as `graftyard_lookup` sends them: a write to the member holding `led_term` resumes it; a write a follower
/// refuses is reported about the member it named; and a follower's read-index read, which the restarted member answers
/// as not leading, fails naming no leader, which `call_opt` reports to every voter. Each elects within a few seconds.
#[test]
fn one_request_elects_an_idle_group_whose_leader_restarted() {
    // (to, write): the request's target and kind.
    for (to, write) in [(A, true), (B, true), (B, false), (C, false)] {
        let mut c = Cluster::new(71);
        c.elect(A);
        c.sync(A);
        c.crash(A);
        c.advance(ms(graftyard_raft::ELECTION_AFTER) * 3);
        c.restart(A);
        c.deliver_all();
        assert_eq!(c.leader(), None, "the idle group has no leader");
        assert_eq!(c.m(A).state().led_term, Some(c.m(A).term()));
        let t0 = c.now;
        let report_all = |c: &mut Cluster, about: SiteId| {
            for s in [A, B, C] {
                let term = c.m(s).term();
                c.on(s, |m, now| m.suspect(now, Colo(1), about, term));
            }
        };
        if write {
            if c.on(to, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(9)), put("k", "v"))).is_err() {
                // The follower names A; A, asked next, resumes.
                c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(9)), put("k", "v"))).expect("A resumes");
            }
        } else {
            let id = c.on(to, |m, now| m.read_index(now)).expect("a follower naming A queues the read");
            c.run(1_000, 100, |_| true);
            let failed = c.resolutions.iter().any(|(s, r)| {
                *s == to && matches!(r, Resolution::Read { id: i, result: Err(nl) } if *i == id && nl.leader.is_none())
            });
            assert!(failed, "the read fails naming no leader");
            report_all(&mut c, to);
        }
        c.run(4_000, 100, |_| true);
        assert!(c.leader().is_some(), "to {to:?} write {write}: no leader {} ms after one request", c.now.0 - t0.0);
    }
}

/// A read granted on a follower carries the leader's read index as its floor, and `fresh_after` answers it later at
/// any state the follower has applied at or above that floor, not below it and not after a restart. A host that
/// answers a waited read from `fresh_after` therefore never answers older than the commit index the leader confirmed
/// for that read (`design/DESIGN.md`, "Follower read").
#[test]
fn a_grant_answers_later_only_at_or_above_the_leaders_read_index() {
    let mut c = Cluster::new(71);
    c.elect(A);
    c.sync(A);
    let at_read = c.m(A).group_fold().applied();
    c.resolutions.clear();
    let id = c.on(B, |m, now| m.read_index(now)).expect("queued for A");
    // A write commits on A after the read arrived but before its answer reaches B: B must not be granted below it.
    c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(7_100)), put("k", "late"))).expect("admitted");
    c.deliver_all();
    let g = c
        .resolutions
        .iter()
        .find_map(|(s, r)| match r {
            Resolution::Read { id: i, result: Ok(g) } if *s == B && *i == id => Some(g.clone()),
            _ => None,
        })
        .expect("B's read was granted");
    assert!(g.floor() >= at_read, "the floor is the leader's read index, at least its applied index at the read");
    assert!(g.floor() <= g.index(), "the grant's own state is at or above its floor");
    let floor = g.floor();
    let later = c.m(B).fresh_after(&g).expect("a later input answers at B's applied state");
    assert!(later.index() >= floor);
    assert_eq!(later.index(), c.m(B).group_fold().applied(), "at B's current state, not the grant's");
    // `fresh` itself refuses after the next input, while `fresh_after` still answers, at the newer state.
    c.sync(A);
    assert!(c.m(B).fresh(&g).is_err(), "a grant is good for `fresh` only in its own input");
    let newer = c.m(B).fresh_after(&g).expect("still answers");
    assert!(newer.index() > floor, "later states are at least as fresh");
    // A member that restarted refuses it: its fold is reloaded, and the host's waiter is gone with the object.
    c.restart(B);
    assert!(c.m(B).fresh_after(&g).is_err(), "not after a restart");
}

/// A follower that restarted numbers its read-index batches from the start again, and an answer the leader sent its
/// previous boot (held for the learner's next call, or late on the network) carries the same batch number. Taken as
/// the new batch's answer it would grant a read below the commit index of a write acknowledged before that read
/// arrived: a stale read. Found by the live sweep with reads answered later (`tests/am1/results/narrow-reads-3.md`).
#[test]
fn a_read_index_answer_from_a_previous_boot_grants_nothing() {
    let mut c = Cluster::new(72);
    c.elect(A);
    c.sync(A);
    // B asks, and A's answer is held back.
    c.resolutions.clear();
    c.on(B, |m, now| m.read_index(now)).expect("queued for A");
    for e in std::mem::take(&mut c.inbox).into_iter().filter(|e| matches!(e.msg, Message::ReadIndex { .. })) {
        let graftyard_raft::Envelope { from, to, msg } = e;
        c.on(to, |m, now| m.step(now, from, msg));
    }
    let held: Vec<_> = c.inbox.drain(..).filter(|e| matches!(e.msg, Message::ReadIndexRes { .. })).collect();
    assert_eq!(held.len(), 1, "A answered at once, inside its lease");
    // B restarts, and A's next append reaches it, so B names A again. Then a write commits through A and C while B
    // hears nothing, so B has not applied it.
    c.restart(B);
    c.inbox.clear();
    c.sync(A);
    assert_eq!(c.m(B).leader(), Some(A), "A's append reached B");
    c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(7_200)), put("k", "new"))).expect("admitted");
    c.deliver(|e| e.to != B && e.from != B);
    assert_eq!(c.value(A, "k").as_deref(), Some("new"));
    let acked = c.m(A).state().commit;
    assert!(c.m(B).group_fold().applied() < acked, "B has not applied the write");
    // B's new boot reads; the old boot's answer arrives first.
    c.resolutions.clear();
    let id = c.on(B, |m, now| m.read_index(now)).expect("B names A again");
    c.inbox.retain(|e| e.to != A);
    for e in held {
        let graftyard_raft::Envelope { from, to, msg } = e;
        c.on(to, |m, now| m.step(now, from, msg));
    }
    let stale = c.resolutions.iter().any(|(s, r)| {
        *s == B && matches!(r, Resolution::Read { id: i, result: Ok(g) } if *i == id && g.floor() < acked)
    });
    assert!(!stale, "B granted its new read below the write A acknowledged before it");
}

/// A follower keeps several read-index batches in flight, and each answer grants only its own batch's reads. B's
/// first batch is answered at index i1 but the answer is held back; a write then commits through A and C, and a second
/// read on B goes out in its own batch at once rather than waiting behind the first. When the first answer arrives
/// it grants the first read only: granted the second too, that read would be served below a write acknowledged before
/// it arrived. The second read is granted by its own answer, at or above that write (`design/DESIGN.md`, "Follower
/// read"; narrow-reads-3, gate 12's learner wait).
#[test]
fn each_read_index_answer_grants_only_its_own_batch() {
    for reversed in [false, true] {
        let mut c = Cluster::new(73);
        c.elect(A);
        c.sync(A);
        let ask = |c: &mut Cluster| {
            let is_ask = |e: &Envelope<KvEvent>| e.from == B && matches!(e.msg, Message::ReadIndex { .. });
            let asks: Vec<_> = c.inbox.iter().filter(|e| is_ask(e)).cloned().collect();
            c.inbox.retain(|e| !is_ask(e));
            for e in &asks {
                let graftyard_raft::Envelope { from, to, msg } = e.clone();
                c.on(to, |m, now| m.step(now, from, msg));
            }
            let is_res = |e: &Envelope<KvEvent>| e.to == B && matches!(e.msg, Message::ReadIndexRes { .. });
            let held: Vec<_> = c.inbox.iter().filter(|e| is_res(e)).cloned().collect();
            c.inbox.retain(|e| !is_res(e));
            (asks.len(), held)
        };
        let granted = |c: &Cluster, id| {
            c.resolutions.iter().find_map(|(s, r)| match r {
                Resolution::Read { id: i, result: Ok(g) } if *s == B && *i == id => Some(g.floor()),
                _ => None,
            })
        };
        let step_all = |c: &mut Cluster, es: Vec<Envelope<KvEvent>>| {
            for graftyard_raft::Envelope { from, to, msg } in es {
                c.on(to, |m, now| m.step(now, from, msg));
            }
        };
        c.resolutions.clear();
        let first = c.on(B, |m, now| m.read_index(now)).expect("queued for A");
        let (n, held1) = ask(&mut c);
        assert_eq!((n, held1.len()), (1, 1), "A answered B's first batch at once, inside its lease");
        // A write commits through A and C while B hears nothing.
        c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(7_300)), put("k", "new"))).expect("admitted");
        c.deliver(|e| e.to != B && e.from != B);
        let acked = c.m(A).state().commit;
        assert!(c.m(B).group_fold().applied() < acked, "B has not applied the write");
        c.advance(graftyard_raft::LEARNER_READ_EVERY.as_millis() as u64);
        c.inbox.retain(|e| e.to != B || matches!(e.msg, Message::ReadIndexRes { .. }));
        let second = c.on(B, |m, now| m.read_index(now)).expect("queued for A");
        let (n, held2) = ask(&mut c);
        assert_eq!(n, 1, "the second read left in its own batch while the first was in flight");
        assert_eq!(held2.len(), 1);
        // The answers arrive in order, or the second first (a far link answers calls in any order).
        if reversed {
            step_all(&mut c, held2);
            step_all(&mut c, held1);
        } else {
            step_all(&mut c, held1);
            assert_eq!(granted(&c, second), None, "the second read waits for its own answer");
            step_all(&mut c, held2);
        }
        assert!(granted(&c, first).is_some(), "reversed {reversed}: the first answer grants the first read");
        let below = granted(&c, second).filter(|f| *f < acked);
        assert_eq!(
            below, None,
            "reversed {reversed}: the second read was granted below the write acknowledged before it"
        );
        // A's appends to B were lost above; its next one brings B the write.
        c.sync(A);
        let f = granted(&c, second).expect("the second answer grants the second read once B applied it");
        assert!(f >= acked, "reversed {reversed}: the second read's floor {f:?} is below the write ({acked:?})");
    }
}

/// A leader whose read-index answers to B are lost, while its appends still arrive (a half-hung leader for reads).
/// B's first unanswered batch is a failed call: B forgets A and fails the read naming no leader. A's next append
/// teaches B the leader again, and a later read's unanswered batch must count as a failed call too. Before the fix the
/// emptied batch stayed in flight and was resent forever, the later read failed as queued behind it, naming A, and B
/// never forgot A again (with `READ_PIPELINE` batches in flight, once that many emptied ones filled it), so a read-only group behind a half-hung leader never elected (F67 rule 12, liveness seed 9
/// once 30% of follower reads are answered later).
#[test]
fn every_unanswered_read_index_batch_is_a_failed_call() {
    let mut c = Cluster::new(76);
    c.elect(A);
    c.sync(A);
    let lost = |e: &Envelope<KvEvent>| !(e.to == B && matches!(e.msg, Message::ReadIndexRes { .. }));
    let failed_naming = |c: &Cluster, id| {
        c.resolutions.iter().find_map(|(s, r)| match r {
            Resolution::Read { id: i, result: Err(nl) } if *s == B && *i == id => Some(nl.leader),
            _ => None,
        })
    };
    // More rounds than batches B keeps in flight: kept, the emptied batches fill the pipeline and block the next.
    for round in 0..graftyard_raft::READ_PIPELINE as u128 + 2 {
        // A write makes A append, which teaches B its leader again after the first round forgot it.
        c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(round + 1)), put("k", "v"))).expect("A leads");
        c.run(1_000, 100, lost);
        assert_eq!(c.m(B).leader(), Some(A), "round {round}: B follows A");
        let id = c.on(B, |m, now| m.read_index(now)).expect("queued for A");
        c.run(4_000, 100, lost);
        assert_eq!(failed_naming(&c, id), Some(None), "round {round}: the read failed naming no leader");
    }
}

/// A member this one refuses in pre-vote as `Suspected` is no transfer target either. B led, two colos reported it,
/// so C suspects it; C then leads, and its site is drained. Before the fix C handed leadership straight back to B, the
/// gray leader its own pre-vote refuses, and on a half-hung B that hand-off repeated until check quorum moved B each
/// time: 37.4 s leaderless with two healthy voters (F67 rule 10, live seed 12073 on main 0f7bf06).
#[test]
fn a_leader_hands_off_to_no_member_it_suspects() {
    let mut c = Cluster::new(77);
    let mut id = 0;
    c.elect(B);
    c.sync(B);
    let term = c.m(C).term();
    for colo in [1, 2] {
        c.on(C, |m, now| m.suspect(now, Colo(colo), B, term));
    }
    c.crash(B);
    c.run(ms(graftyard_raft::ELECTION_AFTER) + 100, 100, |_| true);
    c.elect(C);
    c.sync(C);
    c.restart(B);
    c.run(1_000, 100, |_| true);
    assert_eq!(c.leader(), Some(C));
    assert!(matches!(c.on(C, |m, now| m.transfer(now, B)), Err(TransferError::Target(t)) if t == B));
    // C's site is drained: it hands off to A, the one target it does not suspect.
    for s in SITES {
        c.on(s, |m, now| m.set_drained(now, [C]));
    }
    drive(&mut c, ms(LEASE), true, &mut id, |_| false);
    assert_eq!(c.leader(), Some(A));
}

/// A bar that `suspect` reports set while the leader ran a transfer is the hand-off's, not a gray leader's: once this
/// member accepts another leader's append, it no longer refuses the old leader as `Suspected`. The same reports sent
/// before the transfer began are a gray leader's, and that bar holds for its whole window. Kept, two leaders in turn,
/// each reported during its own hand-off, refused each other as `Suspected` and nobody led for 31.3 s (F67 rule 11,
/// live seed 11121).
#[test]
fn a_hand_offs_suspect_bar_ends_when_another_leader_is_heard() {
    for during_transfer in [true, false] {
        let mut c = Cluster::new(78);
        c.elect(A);
        c.sync(A);
        let term = c.m(C).term();
        let report = |c: &mut Cluster| {
            for colo in [1, 2] {
                c.on(C, |m, now| m.suspect(now, Colo(colo), A, term));
            }
        };
        if !during_transfer {
            report(&mut c);
        }
        // A hands off to B; C accepts a round carrying the transfer before B's campaign.
        c.on(A, |m, now| m.transfer(now, B)).expect("started");
        c.deliver(|e| !matches!(e.msg, Message::TimeoutNow { .. }));
        assert!(c.m(C).state().transfer.is_some());
        if during_transfer {
            report(&mut c);
        }
        c.run(3_000, 100, |_| true);
        assert_eq!(c.leader(), Some(B), "the hand-off completed");
        c.sync(B);
        assert_eq!(c.m(C).leader(), Some(B));
        // A and B go down, so C's promise lapses and only the bar can refuse A.
        let last = (c.m(B).log().len() as u64, c.m(B).log().last().unwrap().term);
        c.crash(A);
        c.crash(B);
        c.advance(ms(graftyard_raft::ELECTION_AFTER) + 100);
        let next = Term(c.m(C).term().0 + 1);
        c.inbox.clear();
        c.on(C, |m, now| m.step(now, A, Message::Vote(vote(next, A, true, last, None))));
        let deny = vote_res(&c, C, A).and_then(|r| r.deny);
        if during_transfer {
            assert_ne!(deny, Some(Deny::Suspected), "a hand-off's bar outlived the hand-off");
        } else {
            assert_eq!(deny, Some(Deny::Suspected), "a gray leader's bar ended early");
        }
    }
}

/// A `/transfer` or drain hand-off goes only to a follower whose AppendEntries this leader saw answered, with at most
/// `commit_errors` of its calls to it failed, in the window. A half-hung target answers some calls; handed leadership,
/// it won the transfer vote and led with its answers lost, or the leader served nothing for the transfer's LEASE
/// (F67 rule 13, live seeds 10273, 11401, and 12761 once rule 12 dropped emptied read-index batches). The control is
/// the same target with every call answered.
#[test]
fn a_leader_hands_off_to_no_follower_whose_answers_it_loses() {
    for half_hung in [true, false] {
        let mut c = Cluster::new(79);
        let mut id = 0;
        c.elect(A);
        c.sync(A);
        // One AppendEntries to B in three loses its answer: under the commit-error rule's `commit_fail_min` in a
        // quiet window, so no STEP_DOWN, but not a link to hand leadership over.
        let mut n = 0;
        drive(&mut c, 4_000, true, &mut id, |e| {
            let to_b = e.to == B && matches!(e.msg, Message::Append(_));
            n += u32::from(to_b);
            half_hung && to_b && n % 3 == 0
        });
        assert_eq!(c.leader(), Some(A));
        let r = c.on(A, |m, now| m.transfer(now, B));
        if half_hung {
            assert!(matches!(r, Err(TransferError::Target(t)) if t == B), "{r:?}");
            // A's site is drained: it hands off to C, the follower it hears in full, not to B.
            for s in SITES {
                c.on(s, |m, now| m.set_drained(now, [A]));
            }
            drive(&mut c, ms(LEASE), true, &mut id, |_| false);
            assert_eq!(c.leader(), Some(C));
        } else {
            assert!(r.is_ok(), "{r:?}");
        }
    }
}
