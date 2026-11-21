//! Targeted cases: BF-12, BF-13, bootstrap and no-config refusals, `led_term`, persist-before-send, encoding, quorum.

mod common;

use common::cluster::Cluster;
use common::kv::{KvCmd, KvErr};
use common::sim::{CHAIN, SITES, config, record};
use graftyard_raft::{
    ByRequest, CallError, CommitError, Deny, Digest, EntryBody, Index, Member, Message, RequestId, Resolution,
    RestoreError, Role, Term, Verdict, VoteReq, bootstrap, digest, quorums,
};
use graftyard_types::{PlacementName, PlacementRecord, SiteId, Timestamp};

const A: SiteId = SITES[0];
const B: SiteId = SITES[1];
const C: SiteId = SITES[2];

fn put(k: &str, v: &str) -> KvCmd {
    KvCmd::Put { key: k.into(), value: v.into() }
}
fn cas(k: &str, expect: Option<&str>, v: &str) -> KvCmd {
    KvCmd::Cas { key: k.into(), expect: expect.map(Into::into), value: v.into() }
}
fn append(k: &str, s: &str) -> KvCmd {
    KvCmd::Append { key: k.into(), s: s.into() }
}

fn admit(c: &mut Cluster, s: SiteId, id: u128, cmd: KvCmd) -> graftyard_raft::Ticket {
    c.on(s, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(id)), cmd)).expect("admitted").ticket
}

#[test]
fn bootstrap_bytes_are_identical_on_every_voter() {
    let a = bootstrap::<common::kv::KvEvent>(CHAIN, &record()).unwrap();
    let b = bootstrap::<common::kv::KvEvent>(CHAIN, &record()).unwrap();
    assert_eq!(digest(&a), digest(&b));
    assert_eq!((a.term, a.index, a.prev), (Term(1), Index(1), graftyard_raft::PREV_ZERO));
    assert!(matches!(a.body, EntryBody::Config(_)) && a.requests.is_empty());
    // Any field changes the digest: another generation, chain, or `at`.
    let mut r = record();
    r.generation = 2;
    assert_ne!(digest(&bootstrap::<common::kv::KvEvent>(CHAIN, &r).unwrap()), digest(&a));
    let mut r = record();
    r.at = Timestamp(2);
    assert_ne!(digest(&bootstrap::<common::kv::KvEvent>(CHAIN, &r).unwrap()), digest(&a));
    let other = graftyard_types::ChainId::Ids;
    assert_ne!(digest(&bootstrap::<common::kv::KvEvent>(other, &record()).unwrap()), digest(&a));
}

#[test]
fn any_voter_may_bootstrap_and_two_proposers_agree() {
    let mut c = Cluster::new(1);
    // Two edges create the group at once, at A and at C.
    assert_eq!(c.on(A, |m, now| m.campaign(now)), Verdict::Bootstrapping);
    assert_eq!(c.on(C, |m, now| m.campaign(now)), Verdict::Bootstrapping);
    c.deliver_all();
    let leaders: Vec<SiteId> = SITES.iter().copied().filter(|s| c.m(*s).role() == Role::Leader).collect();
    assert_eq!(leaders.len(), 1, "{leaders:?}");
    let d1: Vec<Digest> = SITES.iter().map(|s| c.m(*s).digests()[0]).collect();
    assert!(d1.iter().all(|d| *d == d1[0]));
    // Term 1 holds only the bootstrap entry; the leader's term is 2 and starts with `Noop`.
    let l = c.m(leaders[0]);
    assert_eq!(l.term(), Term(2));
    assert!(matches!(l.log()[1].body, EntryBody::Noop));
    assert_eq!(l.log()[1].term, Term(2));
}

#[test]
fn a_member_with_no_committed_config_grants_no_vote_and_serves_no_read() {
    let mut c = Cluster::new(2);
    let req = VoteReq {
        term: Term(2),
        candidate: B,
        pre: true,
        last_index: Index(5),
        last_term: Term(9),
        transfer: None,
        boots: [Timestamp(0); 2],
        stepped_down_at: None,
        bootstrapped: false,
    };
    c.on(A, |m, now| m.step(now, B, Message::Vote(req.clone())));
    c.on(A, |m, now| m.step(now, B, Message::Vote(VoteReq { pre: false, ..req.clone() })));
    let denies: Vec<_> = c
        .inbox
        .iter()
        .filter_map(|e| match &e.msg {
            Message::VoteRes(r) => Some((r.granted, r.deny)),
            _ => None,
        })
        .collect();
    assert_eq!(denies, [(false, Some(Deny::NoConfig)), (false, Some(Deny::NoConfig))]);
    assert_eq!(c.m(A).state().voted_for, None);
    c.inbox.clear();
    // Holding the bootstrap entry without a committed config is still no config.
    c.on(A, |m, now| m.campaign(now));
    c.inbox.clear();
    assert_eq!(c.m(A).log().len(), 1);
    assert!(c.m(A).committed_config().is_none());
    let r = c.on(A, |m, now| m.read(now));
    let nl = r.expect_err("no read without a committed config");
    assert_eq!(nl.config, None);
    assert!(matches!(
        c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(1)), put("k", "v"))),
        Err(CallError::NotLeader(_))
    ));
    // A member with no committed config stores nothing but the bootstrap entry.
    let mut c2 = Cluster::new(3);
    c2.elect(A);
    let entries = c2.m(A).log().to_vec();
    let req = graftyard_raft::AppendReq {
        term: Term(2),
        leader: A,
        boot: 1,
        round: 1,
        sent_at: c.now,
        prev_index: Index(0),
        prev_term: Term(0),
        prev_digest: graftyard_raft::PREV_ZERO,
        entries,
        commit: Index(0),
        transfer: None,
        lead_seq: 1,
    };
    let fresh = SiteId(1);
    let mut lone = Member::<KvCmd>::new(config(fresh), 9).unwrap();
    let mut disk = graftyard_raft::Stored::new(common::kv::Kv::default());
    let ((), _) = lone.step(c.now, A, Message::Append(req)).persist(&mut disk);
    assert_eq!(lone.log().len(), 1);
    assert!(lone.committed_config().is_none());
}

#[test]
fn votes_are_refused_to_non_members_of_the_committed_config() {
    let mut c = Cluster::new(4);
    c.elect(A);
    c.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1);
    c.inbox.clear();
    let outsider = SiteId(9);
    let req = VoteReq {
        term: Term(10),
        candidate: outsider,
        pre: false,
        last_index: Index(100),
        last_term: Term(9),
        transfer: None,
        boots: [Timestamp(0); 2],
        stepped_down_at: None,
        bootstrapped: true,
    };
    c.on(B, |m, now| m.step(now, outsider, Message::Vote(req)));
    let Message::VoteRes(r) = &c.inbox[0].msg else { panic!() };
    assert_eq!((r.granted, r.deny), (false, Some(Deny::NotMember)));
    // And the outsider's higher term did not move B.
    assert_eq!(c.m(B).term(), Term(2));
}

#[test]
fn promise_bars_a_rival_until_it_lapses() {
    let mut c = Cluster::new(5);
    c.elect(A);
    // C may not win while B's and A's promises run.
    assert_eq!(c.on(C, |m, now| m.campaign(now)), Verdict::Promised);
    c.crash(A);
    c.advance(5_000);
    assert_eq!(c.on(C, |m, now| m.campaign(now)), Verdict::Promised);
    c.advance(5_001);
    assert_eq!(c.on(C, |m, now| m.campaign(now)), Verdict::Started);
    c.deliver_all();
    assert_eq!(c.m(C).role(), Role::Leader);
    assert_eq!(c.m(C).term(), Term(3));
}

/// BF-12: the leader dies after the quorum stored the entry and before its reply left. The edge re-sends the same
/// request id to the new leader, gets the original outcome, and the command applies once.
#[test]
fn bf12_resend_after_leader_loss_is_answered_from_the_request_row() {
    let mut c = Cluster::new(6);
    c.elect(A);
    let t = admit(&mut c, A, 77, append("k", "x"));
    // B stores and acks; A counts the quorum and resolves the ticket, but A crashes before the reply is sent: the
    // test drops A's resolution and every message A queued after that point.
    c.deliver(|e| e.to == B || (e.from == B && e.to == A));
    assert!(c.ticket_results(t).iter().any(|r| matches!(r, Resolution::Ticket { result: Ok(_), .. })));
    c.resolutions.clear();
    c.crash(A);
    // The edge never saw the reply. B wins, and B learns the commit from its own Noop.
    c.elect_after_promises(B);
    assert_eq!(c.value(B, "k").as_deref(), Some("x"));
    let row = c.m(B).group_fold().request(RequestId(graftyard_raft::Ulid(77))).expect("row on the new leader").index;
    // `outcome_by_request` answers from the row.
    match c.m(B).outcome_by_request(RequestId(graftyard_raft::Ulid(77)), &append("k", "x")).unwrap() {
        ByRequest::Done(a) => assert_eq!((a.index, a.out), (row, true)),
        _ => panic!("expected Done"),
    }
    // And a plain re-send of the same id is answered from it too, without being decided again.
    let t2 = admit(&mut c, B, 77, append("k", "x"));
    let got = c.m(B).outcome(t2).expect("resolved at once");
    assert_eq!(got.map(|a| (a.index, a.out)), Ok((row, true)));
    c.deliver_all();
    c.restart(A);
    c.sync(B);
    for s in SITES {
        assert_eq!(c.value(s, "k").as_deref(), Some("x"), "{s:?} applied the command a second time");
    }
}

/// BF-12, many times: kill the leader at every point between quorum and reply, re-send to the new leader.
#[test]
fn bf12_many_kills() {
    for seed in 0..300u64 {
        let mut c = Cluster::new(100 + seed);
        c.elect(A);
        let id = 1_000 + u128::from(seed);
        admit(&mut c, A, id, append("k", "y"));
        // Deliver a seeded prefix of the messages, then kill A.
        let mut budget = seed % 4;
        c.deliver(|e| {
            if e.to == C {
                return false;
            }
            if budget == 0 {
                return false;
            }
            budget -= 1;
            true
        });
        c.crash(A);
        c.resolutions.clear();
        c.inbox.clear();
        c.elect_after_promises(B);
        // Re-send until answered.
        let t = admit(&mut c, B, id, append("k", "y"));
        c.deliver_all();
        let r = c.m(B).outcome(t).expect("resolved");
        assert_eq!(r.map(|a| a.out), Ok(true), "seed {seed}");
        assert_eq!(c.value(B, "k").as_deref(), Some("y"), "seed {seed}: applied twice or never");
        c.restart(A);
        c.sync(B);
        for s in SITES {
            assert_eq!(c.value(s, "k").as_deref(), Some("y"), "seed {seed}: {s:?}");
        }
    }
}

/// BF-13: an entry whose second command is refused `Stale` only because of the first command of the same entry. The
/// leader is lost before the entry commits, so neither ticket may resolve as anything but `NotLeader`.
#[test]
fn bf13_refusal_from_an_uncommitted_entry_is_never_returned() {
    let mut c = Cluster::new(7);
    c.elect(A);
    let t = c.one_entry(A, 1, vec![cas("k", None, "first"), cas("k", None, "second")]);
    let (t1, t2) = (t[0], t[1]);
    let entry = c.m(A).log().last().unwrap().clone();
    let EntryBody::Events(evs) = &entry.body else { panic!("{entry:?}") };
    assert_eq!(evs.len(), 1, "the first cas applied, the second refused within the same entry");
    assert_eq!(entry.requests.len(), 1, "a refused command has no request range");
    // The leader names the refused command and its entry when it decides, before the entry commits.
    let rid = |n: u128| RequestId(graftyard_raft::Ulid(n));
    assert_eq!(c.refused, vec![(A, entry.index, rid(3))]);
    // The leader decided t2 Stale only because of t1, and the entry did not commit: neither ticket resolved.
    assert!(c.ticket_results(t1).is_empty() && c.ticket_results(t2).is_empty(), "resolved before commit");
    assert_eq!(c.m(A).outcome(t2), None, "outcome must wait for the commit");
    c.inbox.clear();
    c.crash(A);
    assert!(
        c.resolutions
            .iter()
            .all(|(_, r)| !matches!(r, Resolution::Ticket { result: Err(CommitError::Refused { .. }), .. }))
    );
    // The edge re-sends both to the new leader, which decides them again on a fold without the first cas.
    c.elect_after_promises(B);
    let t2b = admit(&mut c, B, 3, cas("k", None, "second"));
    c.deliver_all();
    assert_eq!(c.m(B).outcome(t2b).unwrap().map(|a| a.out), Ok(true), "the earlier Stale must not be returned");
    let t1b = admit(&mut c, B, 2, cas("k", None, "first"));
    c.deliver_all();
    let r1 = c.m(B).outcome(t1b).unwrap();
    assert!(matches!(r1, Err(CommitError::Refused { err: KvErr::Stale, .. })), "{r1:?}");
    // And A's tickets: a restarted A answers them NotLeader (another boot), never Stale.
    c.restart(A);
    let r = c.m(A).outcome(t2).unwrap();
    assert!(matches!(r, Err(CommitError::NotLeader(_))), "{r:?}");
}

/// A refusal resolves only once its entry commits, on the commit-advance path.
#[test]
fn refusals_resolve_at_commit_with_the_commit_index() {
    let mut c = Cluster::new(8);
    c.elect(A);
    let t = c.one_entry(A, 1, vec![cas("k", None, "a"), cas("k", None, "b")]);
    let (t1, t2) = (t[0], t[1]);
    assert!(c.ticket_results(t2).is_empty());
    c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64);
    c.deliver_all();
    let index = c.m(A).state().commit;
    let r2 = c.m(A).outcome(t2).unwrap();
    assert_eq!(r2, Err(CommitError::Refused { index, err: KvErr::Stale }));
    let r1 = c.m(A).outcome(t1).unwrap().unwrap();
    assert_eq!((r1.index, r1.out), (index, true));
    assert_eq!(r1.quorum().regions(), 2);
    assert!(r1.quorum().sites().len() >= 2 && r1.quorum().sites().contains(&A));
    // A refused command leaves no row, so its re-send is decided again.
    assert!(c.m(A).group_fold().request(RequestId(graftyard_raft::Ulid(3))).is_none());
    assert!(c.m(A).group_fold().request(RequestId(graftyard_raft::Ulid(2))).is_some());
}

/// A restarted leader resumes `led_term` after one round: same term, no new election, a new `boot`.
#[test]
fn restarted_leader_resumes_led_term() {
    let mut c = Cluster::new(9);
    c.elect(A);
    let t = admit(&mut c, A, 1, put("k", "1"));
    c.deliver_all();
    assert!(c.m(A).outcome(t).unwrap().is_ok());
    let (term, boot) = (c.m(A).term(), c.m(A).boot());
    c.crash(A);
    c.advance(200);
    c.restart(A);
    assert_eq!(c.m(A).state().led_term, Some(term));
    // The next request resumes the term; the old ticket answers NotLeader (another boot).
    let t2 = admit(&mut c, A, 2, put("k", "2"));
    assert!(matches!(c.m(A).outcome(t), Some(Err(CommitError::NotLeader(_)))));
    assert_ne!(c.m(A).boot(), boot);
    assert_eq!(c.m(A).term(), term);
    assert_eq!(c.m(A).outcome(t2), None, "not ready before one round");
    c.deliver_all();
    assert_eq!(c.m(A).term(), term, "resumed, not re-elected");
    assert_eq!(c.m(A).outcome(t2).unwrap().map(|a| a.out), Ok(true));
    assert!(SITES.iter().all(|s| c.m(*s).term() == term));
    // A read on the resumed leader works too.
    let id = c.on(A, |m, now| m.read(now)).unwrap();
    let grant = c
        .resolutions
        .iter()
        .find_map(|(_, r)| match r {
            Resolution::Read { id: i, result: Ok(g) } if *i == id => Some(g.clone()),
            _ => None,
        })
        .expect("lease read");
    assert_eq!(c.m(A).fresh(&grant).unwrap().fold().0["k"], "2");
}

/// Two boots inside RESET_QUARANTINE: pre-vote refuses the flapping member until a voter has seen no leader for
/// 2 x ELECTION_AFTER.
#[test]
fn flapping_member_is_quarantined() {
    let mut c = Cluster::new(10);
    c.elect(A);
    for _ in 0..2 {
        c.on(C, |m, now| m.tick(now));
        c.crash(C);
        c.advance(100);
        c.restart(C);
        c.deliver_all();
    }
    let boots = c.m(C).state().boots;
    assert!(boots[0].0 != 0 && boots[1].0 != 0, "{boots:?}");
    c.crash(A);
    c.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 10);
    c.inbox.clear();
    c.on(C, |m, now| m.campaign(now));
    c.deliver_all();
    assert_ne!(c.m(C).role(), Role::Leader, "a flapping member won");
    // B, not flapping, wins.
    c.on(B, |m, now| m.campaign(now));
    c.deliver_all();
    assert_eq!(c.m(B).role(), Role::Leader);
}

/// LM-1 round 6's "leaderless" groups: every member of a group reset twice inside RESET_QUARANTINE, and a lost
/// campaign moved the term past the old leader's `led_term`, so no member resumes. Campaigns are request-driven, so
/// the group stays leaderless while idle; the first requests' `suspect` reports elect a leader once the restored
/// promises lapse (ELECTION_AFTER after the last append), though every candidate is flapping.
#[test]
fn a_group_whose_every_member_flapped_elects_on_the_first_requests() {
    let mut c = Cluster::new(72);
    c.elect(A);
    c.sync(A);
    for _ in 0..2 {
        for s in SITES {
            c.crash(s);
        }
        c.advance(100);
        for s in SITES {
            c.restart(s);
        }
        c.deliver_all();
    }
    c.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1);
    assert_eq!(c.on(B, |m, now| m.campaign(now)), Verdict::Started);
    // B wins its pre-vote and its votes are granted, then it resets before it hears them: the term moved on.
    c.deliver(|e| !matches!(&e.msg, Message::VoteRes(r) if !r.pre));
    c.crash(B);
    c.inbox.clear();
    c.advance(100);
    c.restart(B);
    assert!(SITES.iter().all(|s| c.m(*s).state().led_term.is_none() && c.m(*s).role() == Role::Follower));
    let start = c.now.0;
    for _ in 0..60 {
        c.advance(500);
        c.deliver_all();
        assert!(SITES.iter().all(|s| c.m(*s).role() != Role::Leader), "a leader with no request");
    }
    // Requests' lookups: every voter answers NotLeader naming none, so each gets a `suspect` report, every 100 ms.
    // Every member is flapping, so none denies a rival as `Contending` (F67); the losers' random backoff, not a
    // lockstep 500 ms cadence that outlasts it and splits every vote, picks the winner.
    let begun = c.now.0;
    for _ in 0..200 {
        let term = SITES.iter().map(|s| c.m(*s).term()).max().unwrap();
        for s in SITES {
            c.on(s, |m, now| m.suspect(now, graftyard_raft::Colo(7), A, term));
        }
        c.deliver_all();
        if let Some(l) = SITES.iter().copied().find(|s| c.m(*s).role() == Role::Leader) {
            assert!(c.now.0 - begun <= 1_000, "elected {} ms after the first report", c.now.0 - begun);
            let t = admit(&mut c, l, 9, put("k", "v"));
            c.deliver_all();
            assert_eq!(c.m(l).outcome(t).unwrap().map(|a| a.out), Ok(true));
            assert!(c.now.0 - start >= 30_000);
            return;
        }
        c.advance(100);
        c.deliver_all();
    }
    panic!("no leader 20 s after the first report");
}

/// A quarantined member campaigning does not contend: B, flapping, refuses no rival, so C wins at once with B's vote.
/// Before F67's fix B denied C as `Contending` while every voter refused B, and with A down nothing led until the
/// waiver after 2 x ELECTION_AFTER (`tests/m0/results/f67.md`, path 4).
#[test]
fn a_quarantined_campaigner_does_not_contend() {
    let mut c = Cluster::new(12);
    c.elect(A);
    for _ in 0..2 {
        c.on(B, |m, now| m.tick(now));
        c.crash(B);
        c.advance(100);
        c.restart(B);
        c.deliver_all();
    }
    c.crash(A);
    c.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 10);
    c.inbox.clear();
    c.on(B, |m, now| m.campaign(now));
    c.on(C, |m, now| m.campaign(now));
    c.deliver_all();
    assert_eq!(c.m(C).role(), Role::Leader, "the flapping member blocked the only candidate that may lead");
}

/// Reads: within the lease from the fold, after it lapses only via a round, never on a minority leader.
#[test]
fn reads_need_the_lease_or_a_round() {
    let mut c = Cluster::new(11);
    c.elect(A);
    let read = |c: &mut Cluster| -> Option<bool> {
        c.resolutions.clear();
        let id = c.on(A, |m, now| m.read(now)).ok()?;
        c.resolutions.iter().find_map(|(_, r)| match r {
            Resolution::Read { id: i, result } if *i == id => {
                Some(result.as_ref().map(|g| g.via_round()).unwrap_or(true))
            }
            _ => None,
        })
    };
    assert_eq!(read(&mut c), Some(false), "inside the lease");
    // Let the lease run out (the lease end is DRIFT early for the leader's own reads): a read waits for a round.
    c.advance(graftyard_raft::LEASE.as_millis() as u64 - graftyard_raft::DRIFT.as_millis() as u64);
    c.inbox.clear();
    assert_eq!(read(&mut c), None, "a lapsed lease must wait for a round");
    c.deliver_all();
    let granted =
        c.resolutions.iter().any(|(_, r)| matches!(r, Resolution::Read { result: Ok(g), .. } if g.via_round()));
    assert!(granted, "a round after the read arrived grants it");
    // Cut the leader off: its read never gets a round and fails NotLeader.
    c.advance(graftyard_raft::LEASE.as_millis() as u64);
    c.inbox.clear();
    assert_eq!(read(&mut c), None);
    for _ in 0..5 {
        c.inbox.clear();
        c.advance(1_000);
    }
    c.inbox.clear();
    let failed = c.resolutions.iter().any(|(_, r)| matches!(r, Resolution::Read { result: Err(_), .. }));
    assert!(failed, "a read that never gets its round fails NotLeader");
    c.advance(1);
    c.deliver_all();
    // A grant is good only before the member's next input.
    // An idle leader sends no heartbeats, so the lease is still lapsed: one read's round renews it, and the next
    // read is granted inside the same input.
    let _ = read(&mut c);
    c.deliver_all();
    c.resolutions.clear();
    let id = c.on(A, |m, now| m.read(now)).unwrap();
    let g = c
        .resolutions
        .iter()
        .find_map(|(_, r)| match r {
            Resolution::Read { id: i, result: Ok(g) } if *i == id => Some(g.clone()),
            _ => None,
        })
        .unwrap();
    // The grant came from the last step, which is the member's latest input, so it is still good.
    assert_eq!(c.m(A).fresh(&g).map(|f| f.index()).ok(), Some(g.index()));
    c.on(A, |m, now| m.tick(now));
    assert!(c.m(A).fresh(&g).is_err(), "a grant outlived the next input");
}

/// A host whose clock may lag (`set_clock_suspect`): inside a lease the read waits for a round sent after it
/// arrived, and only that round grants it. Cleared, the lease grants at once again.
#[test]
fn a_suspect_clock_grants_reads_only_by_a_round() {
    let mut c = Cluster::new(13);
    c.elect(A);
    let read = |c: &mut Cluster| -> (graftyard_raft::ReadId, Option<bool>) {
        c.resolutions.clear();
        let id = c.on(A, |m, now| m.read(now)).expect("leading");
        let r = c.resolutions.iter().find_map(|(_, r)| match r {
            Resolution::Read { id: i, result: Ok(g) } if *i == id => Some(g.via_round()),
            _ => None,
        });
        (id, r)
    };
    assert_eq!(read(&mut c).1, Some(false), "inside the lease, granted at once");
    c.m_mut(A).set_clock_suspect(true);
    // Past the election's round, whose acks were sent in the same millisecond as a read now would arrive.
    c.advance(1);
    c.inbox.clear();
    let (id, at_once) = read(&mut c);
    assert_eq!(at_once, None, "a suspect clock granted a read on the lease");
    // Rounds sent before the read arrived do not count: only the broadcast the read started grants it.
    assert!(c.inbox.iter().any(|e| e.from == A), "the read sent no round");
    c.deliver_all();
    let granted = c.resolutions.iter().find_map(|(_, r)| match r {
        Resolution::Read { id: i, result: Ok(g) } if *i == id => Some(g.via_round()),
        _ => None,
    });
    assert_eq!(granted, Some(true), "the read's own round grants it");
    c.m_mut(A).set_clock_suspect(false);
    assert_eq!(read(&mut c).1, Some(false), "cleared, the lease grants at once");
}

/// The output gate: a member that crashes before its writes are durable sent nothing, so a vote it would have
/// granted is not counted and its term is not lost.
#[test]
fn nothing_leaves_before_its_writes_persist() {
    let mut c = Cluster::new(12);
    c.elect(A);
    c.crash(A);
    c.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1);
    c.on(C, |m, now| m.campaign(now));
    // C's pre-vote reaches B, and B's grant reaches C, which sends its real vote request.
    let pre = c.inbox.iter().find(|e| e.to == B).cloned().expect("pre-vote to B");
    c.inbox.clear();
    c.on(B, |m, now| m.step(now, C, pre.msg));
    let grant = c.inbox.pop().expect("pre-vote grant");
    c.inbox.clear();
    c.on(C, |m, now| m.step(now, B, grant.msg));
    let vote = c.inbox.iter().find(|e| e.to == B).cloned().expect("vote to B");
    c.inbox.clear();
    let Message::Vote(req) = vote.msg.clone() else { panic!("{vote:?}") };
    assert!(!req.pre);
    c.on_then_crash(B, |m, now| m.step(now, C, Message::Vote(req.clone())));
    c.restart(B);
    assert_ne!(c.m(B).state().voted_for, Some(C), "an unpersisted vote survived");
    assert!(c.m(B).term() < req.term);
    assert!(c.inbox.iter().all(|e| !matches!(e.msg, Message::VoteRes(_))), "a reply left before its write");
}

#[test]
fn command_max_and_unknown_formats_are_refused_without_a_change() {
    let mut c = Cluster::new(13);
    c.elect(A);
    let last = c.m(A).log().len();
    let r = c.on(A, |m, now| {
        m.commit(now, RequestId(graftyard_raft::Ulid(1)), KvCmd::Big { bytes: graftyard_raft::COMMAND_MAX })
    });
    assert!(matches!(r, Err(CallError::TooLarge { .. })), "{r:?}");
    let r = c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(2)), KvCmd::Big { bytes: 1_000 }));
    assert!(r.is_ok());
    let r = c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(3)), KvCmd::Future));
    assert!(matches!(r, Err(CallError::Unsupported(u)) if u.format == common::kv::FUTURE_FORMAT), "{r:?}");
    assert_eq!(c.m(A).log().len(), last + 1, "only the in-cap command made an entry");
}

/// Group commit: commands that arrive during a round go into the next entry together.
#[test]
fn group_commit_puts_a_round_of_commands_in_one_entry() {
    let mut c = Cluster::new(14);
    c.elect(A);
    admit(&mut c, A, 1, put("a", "1"));
    let before = c.m(A).log().len();
    for i in 2..12 {
        admit(&mut c, A, i, put("a", &i.to_string()));
    }
    assert_eq!(c.m(A).log().len(), before, "one entry in flight");
    c.deliver_all();
    let log = c.m(A).log();
    assert_eq!(log.len(), before + 1);
    let EntryBody::Events(evs) = &log[before].body else { panic!() };
    assert_eq!((evs.len(), log[before].requests.len()), (10, 10));
    c.sync(A);
    assert_eq!(c.value(B, "a").as_deref(), Some("11"));
}

/// `verify` refuses an entry whose compare-and-set does not hold on the follower's fold, and the follower stores
/// nothing.
#[test]
fn verify_refuses_a_bad_expect_and_a_broken_chain() {
    let mut c = Cluster::new(15);
    c.elect(A);
    admit(&mut c, A, 1, put("k", "v"));
    c.deliver_all();
    let tail = c.m(B).log().last().unwrap().clone();
    let mut bad = tail.clone();
    bad.index = Index(tail.index.0 + 1);
    bad.prev = *c.m(B).digests().last().unwrap();
    bad.requests = smallvec::smallvec![(RequestId(graftyard_raft::Ulid(5)), 0..1)];
    bad.body = EntryBody::Events(vec![common::kv::KvEvent::Set {
        key: "k".into(),
        value: "w".into(),
        expect: Some(Some("not v".into())),
    }]);
    fn view(m: &Member<KvCmd>) -> graftyard_raft::FoldView<'_, common::kv::Kv> {
        graftyard_raft::FoldView {
            fold: &m.group_fold().fold,
            id_block: m.group_fold().id_block(),
            unapplied: &[],
            index: Index(m.log().len() as u64 + 1),
            prev: *m.digests().last().unwrap(),
            boot: m.digests()[0],
            reads: &[common::kv::KV_FORMAT],
            release: config(B).release,
        }
    }
    let r = graftyard_raft::verify(&bad, &view(c.m(B)));
    assert!(r.unwrap_err().reason.contains("cas"));
    let mut chain = bad.clone();
    chain.body = EntryBody::Events(vec![]);
    chain.requests.clear();
    chain.prev = Digest([1; 32]);
    assert!(graftyard_raft::verify(&chain, &view(c.m(B))).unwrap_err().reason.contains("prev"));
    let mut fmt = chain.clone();
    fmt.prev = bad.prev;
    fmt.format = common::kv::FUTURE_FORMAT;
    assert!(graftyard_raft::verify(&fmt, &view(c.m(B))).unwrap_err().reason.contains("format"));
    let mut range = bad.clone();
    range.body = EntryBody::Events(vec![]);
    assert!(graftyard_raft::verify(&range, &view(c.m(B))).unwrap_err().reason.contains("maps to"));
}

#[test]
fn joint_needs_both_majorities_and_witness_rules_hold() {
    use graftyard_raft::GroupConfig;
    use graftyard_types::Placement;
    let p = |sites: [u8; 3], name: PlacementName| {
        let r = PlacementRecord {
            name,
            sites: sites.iter().map(|s| SiteId(*s)).collect(),
            home: SiteId(sites[0]),
            generation: 1,
            moving_from: None,
            at: Timestamp(1),
        };
        Box::new(Placement::from_record(CHAIN, &r).unwrap())
    };
    let set = |s: &[u8]| s.iter().map(|x| SiteId(*x)).collect::<std::collections::BTreeSet<_>>();
    let joint = GroupConfig::Joint { old: p([0, 1, 2], PlacementName::Tri), new: p([0, 3, 4], PlacementName::Tri) };
    assert!(!quorums::election(&joint, &set(&[0, 1])), "old majority only");
    assert!(!quorums::election(&joint, &set(&[0, 3])), "new majority only");
    assert!(quorums::election(&joint, &set(&[0, 1, 3])));
    assert!(quorums::commit(&joint, &set(&[1, 2, 3, 4])));
    assert!(!quorums::commit(&joint, &set(&[0, 1, 2])));
    assert!(quorums::is_voter(&joint, SiteId(4)) && !quorums::is_voter(&joint, SiteId(5)));
    // 2 Full + Witness: a commit needs both Full members; a vote needs any two.
    let w = GroupConfig::Normal(p([0, 1, 2], PlacementName::Us2w));
    assert!(!quorums::commit(&w, &set(&[0, 2])));
    assert!(quorums::commit(&w, &set(&[0, 1])));
    assert!(quorums::election(&w, &set(&[0, 2])));
    // Degraded: the surviving Full member and the witness.
    let d = GroupConfig::Degraded { placement: p([0, 1, 2], PlacementName::Us2w), lost: SiteId(1) };
    assert!(quorums::commit(&d, &set(&[0, 2])));
    assert!(!quorums::commit(&d, &set(&[0])));
}

/// `GroupFold::apply` refuses to apply one request id twice: the BF-12 property at the fold.
#[test]
#[should_panic(expected = "applied twice")]
fn the_fold_panics_on_a_request_applied_twice() {
    let mut c = Cluster::new(16);
    c.elect(A);
    admit(&mut c, A, 1, put("k", "v"));
    c.deliver_all();
    let m = c.m(A);
    let mut f = m.group_fold().clone();
    let mut e = m.log().last().unwrap().clone();
    e.index = Index(m.log().len() as u64 + 1);
    f.apply(&e);
}

/// A voter still promised to a live leader refuses a rival whose own promise lapsed while it was cut off, even when
/// the rival's log is as current as its own; otherwise the old leader's lease reads go stale.
#[test]
fn a_promised_voter_refuses_a_cut_off_rival() {
    let mut c = Cluster::new(17);
    c.elect(A);
    c.sync(A);
    // C hears nothing for longer than ELECTION_AFTER. A renews B's promise by read rounds, which add no entries, so
    // C's log stays as current as B's.
    for _ in 0..6 {
        c.advance(2_000);
        let _ = c.on(A, |m, now| m.read(now));
        c.deliver(|e| e.to != C && e.from != C);
    }
    assert!(c.m(C).state().promise_until < c.now, "C's promise lapsed");
    assert!(c.m(B).state().promise_until > c.now, "B is still promised to A");
    assert_eq!(c.m(C).log().len(), c.m(B).log().len());
    assert_eq!(c.on(C, |m, now| m.campaign(now)), Verdict::Started);
    let pre = c.inbox.iter().find(|e| e.to == B).cloned().expect("pre-vote to B");
    c.inbox.clear();
    c.on(B, |m, now| m.step(now, C, pre.msg));
    let Message::VoteRes(r) = &c.inbox[0].msg else { panic!() };
    assert_eq!((r.granted, r.deny), (false, Some(Deny::Promised)));
    c.deliver_all();
    assert_eq!(c.m(A).role(), Role::Leader);
    assert_ne!(c.m(C).role(), Role::Leader);
}

/// A voter that granted a rival's pre-vote starts no campaign of its own for one CALL_DEADLINE, so the rival's real
/// vote is not split. Before, suspect reports that reached B and C together started both, each passed pre-vote, and
/// each refused the other as `Voted` while A was silent, for three terms (`long_liveness` writes seeds 3052 and 14011).
#[test]
fn a_voter_that_granted_a_rivals_pre_vote_lets_it_win() {
    let mut c = Cluster::new(23);
    c.elect(A);
    c.sync(A);
    let term = c.m(B).term();
    c.crash(A);
    c.advance(10_001);
    assert_eq!(c.on(C, |m, now| m.campaign(now)), Verdict::Started);
    let pre = c.inbox.iter().position(|e| e.to == B).expect("pre-vote to B");
    let pre = c.inbox.remove(pre);
    c.on(B, |m, now| m.step(now, C, pre.msg.clone()));
    // A report about A reaches B before C's real vote does.
    c.advance(50);
    assert_eq!(c.on(B, |m, now| m.suspect(now, graftyard_raft::Colo(3), A, term)), Verdict::Busy);
    // C's pre-vote sent again, as when B's answer is lost, does not extend the wait: one CALL_DEADLINE per term asked.
    c.advance(850);
    c.on(B, |m, now| m.step(now, C, pre.msg));
    c.advance(101);
    assert_eq!(c.on(B, |m, now| m.suspect(now, graftyard_raft::Colo(3), A, term)), Verdict::Started);
    c.deliver(|e| e.to != A);
    assert_eq!(c.m(C).role(), Role::Leader);
    assert_eq!(c.m(C).term(), Term(term.0 + 1));
}

/// A voter whose own pre-vote is out, granting a lower-id rival's pre-vote, drops its own: else a third voter (the gray
/// leader, in `long_liveness` writes seed 15198) grants both, and both split the real vote.
#[test]
fn a_voter_that_grants_a_rivals_pre_vote_drops_its_own() {
    let mut c = Cluster::new(29);
    c.elect(A);
    c.sync(A);
    let term = c.m(B).term();
    // A restarts as a follower and so grants any pre-vote whose log is as current as its own, as the gray leader did.
    c.crash(A);
    c.advance(10_001);
    c.restart(A);
    assert_eq!(c.on(C, |m, now| m.campaign(now)), Verdict::Started);
    assert_eq!(c.on(B, |m, now| m.campaign(now)), Verdict::Started);
    let from_b = c.inbox.iter().position(|e| e.from == B && e.to == C).expect("B's pre-vote to C");
    let from_b = c.inbox.remove(from_b);
    c.on(C, |m, now| m.step(now, B, from_b.msg));
    // A grants both pre-votes, but its storage fails, so no real vote reaches it in time: only B goes on to the real
    // vote, and C grants it.
    c.deliver(|e| !(e.to == A && matches!(&e.msg, Message::Vote(v) if !v.pre)));
    assert_eq!(c.m(B).role(), Role::Leader);
    assert_eq!(c.m(B).term(), Term(term.0 + 1));
    assert_eq!(c.m(C).term(), Term(term.0 + 1));
}

/// Six confirms of 1.05 s on `s`: within one STEP_DOWN window its storage would trip the confirm rule.
fn slow_storage(c: &mut Cluster, s: SiteId) {
    for _ in 0..6 {
        c.slow_confirm(s, 1_050);
    }
}

/// A report about the leader A to `s` from colo `colo`.
fn report(c: &mut Cluster, s: SiteId, colo: u32) -> Verdict {
    let term = c.m(s).term();
    c.on(s, |m, now| m.suspect(now, graftyard_raft::Colo(colo), A, term))
}

/// A gray leader with one slow and one healthy follower. Two colos report A to both, so both withhold their promises,
/// while A keeps appending. Once the promises lapse, the slow follower does not campaign against a leader still
/// appending to it, and the healthy one does and wins. So judging by the candidate's own storage keeps F67's move of
/// a gray leader.
#[test]
fn a_slow_follower_holds_back_while_a_healthy_one_moves_a_gray_leader() {
    let mut c = Cluster::new(31);
    c.elect(A);
    c.sync(A);
    for s in [B, C] {
        assert_eq!(report(&mut c, s, 1), Verdict::Promised);
        assert_eq!(report(&mut c, s, 2), Verdict::Promised);
    }
    // A keeps appending, which renews no promise while they withhold; their promises lapse within ELECTION_AFTER.
    for i in 0..4 {
        c.advance(1_000);
        let _ = admit(&mut c, A, 10 + i, put("k", "v"));
        c.deliver_all();
    }
    let _ = admit(&mut c, A, 20, put("k", "w"));
    c.deliver_all();
    // C's storage had one slow confirm, under `commit_fail_min`'s three: that is no reason to hold back.
    c.slow_confirm(C, 1_050);
    let _ = admit(&mut c, A, 21, put("k", "x"));
    c.deliver_all();
    // B's third slow confirm is still running: it counts, or a member whose storage has stopped answering would look
    // healthier than one whose confirms end slowly. The clock runs on as long as six would take.
    c.advance(3 * 1_050);
    for _ in 0..2 {
        c.slow_confirm(B, 1_050);
    }
    let stuck = c.members[B.0 as usize].as_mut().expect("B is up").confirm_begin(c.now);
    c.advance(1_050);
    let _ = admit(&mut c, A, 22, put("k", "y"));
    c.deliver_all();
    assert!(c.m(B).state().promise_until < c.now && c.m(C).state().promise_until < c.now, "promises lapsed");
    assert_eq!(report(&mut c, B, 3), Verdict::MayNotLead, "a slow follower deposed a leader that still appends");
    c.on(B, |m, now| m.confirm_end(now, stuck));
    assert_eq!(report(&mut c, C, 3), Verdict::Started, "one slow confirm held a healthy follower back");
    c.deliver_all();
    assert_eq!(c.m(C).role(), Role::Leader);
}

/// The hold on a slow follower lasts only while the leader appends: CAMPAIGN_DEADLINE after the last append, a
/// follower whose storage is still slow campaigns as any other does, so a dead leader is replaced.
#[test]
fn a_slow_follower_still_replaces_a_dead_leader() {
    let mut c = Cluster::new(37);
    c.elect(A);
    c.sync(A);
    let t = admit(&mut c, A, 1, put("k", "v"));
    c.deliver_all();
    assert_eq!(c.m(A).outcome(t).unwrap().map(|a| a.out), Ok(true));
    c.crash(A);
    // B's promise to A lapses ELECTION_AFTER after A's last append; its storage is slow over the last 6.3 s of that.
    c.advance(3_750);
    slow_storage(&mut c, B);
    let term = c.m(B).term();
    assert_eq!(c.on(B, |m, now| m.suspect(now, graftyard_raft::Colo(1), A, term)), Verdict::Started);
    c.deliver_all();
    assert_eq!(c.m(B).role(), Role::Leader);
}

/// A vote is persisted before the grant leaves, and a restart keeps it: no second vote in that term.
#[test]
fn a_restarted_voter_does_not_vote_twice_in_a_term() {
    let mut c = Cluster::new(18);
    c.elect(A);
    c.crash(A);
    c.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1);
    // C wins B's vote for term 3, and B's grant is lost.
    c.on(C, |m, now| m.campaign(now));
    c.deliver(|e| !matches!(&e.msg, Message::VoteRes(r) if !r.pre));
    assert_eq!(c.m(B).state().voted_for, Some(C));
    let term = c.m(B).term();
    c.crash(B);
    c.restart(B);
    c.restart(A);
    assert_eq!(c.m(B).state().voted_for, Some(C), "the vote did not survive the restart");
    // A asks B for the same term. B must refuse.
    c.inbox.clear();
    assert_eq!(c.m(A).term(), Term(term.0 - 1));
    c.on(A, |m, now| m.campaign(now));
    c.deliver(|e| e.to != C && e.from != C);
    assert_ne!(c.m(A).role(), Role::Leader, "B voted twice in {term:?}");
}

/// Raft's figure 8: a leader must not commit an entry of an earlier term by counting replicas. A holds a term-2 entry
/// no one else has, C wins term 3 without it, then A wins term 4. A follower's ack that covers the term-2 entry but
/// not A's term-4 `Noop` (an append cut short, as `CATCHUP_MAX` does on a long log) must not commit the term-2 entry:
/// C could still win term 5 and overwrite it.
#[test]
fn an_old_term_entry_is_not_committed_by_counting_replicas() {
    let mut c = Cluster::new(19);
    c.elect(A);
    c.sync(A);
    let base = c.m(A).log().len() as u64;
    // The term-2 entry at base + 1 stays on A.
    c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(500)), put("f8", "old"))).unwrap();
    c.inbox.clear();
    assert_eq!(c.m(A).log().len() as u64, base + 1);
    // C wins term 3 with B's vote; its appends are lost.
    c.crash(A);
    c.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1);
    c.on(C, |m, now| m.campaign(now));
    c.deliver(|e| !matches!(e.msg, Message::Append(_)));
    assert_eq!((c.m(C).role(), c.m(C).term()), (Role::Leader, Term(3)));
    c.inbox.clear();
    c.crash(C);
    // A returns and resumes `led_term` 2; its first round meets B in term 3 and steps it down (B refuses a term-2
    // append whatever it carries). It campaigns until it leads. Appends of a later term that carry entries are lost,
    // so B stores nothing of A's.
    c.restart(A);
    c.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1);
    for _ in 0..3 {
        c.inbox.clear();
        c.on(A, |m, now| m.campaign(now));
        c.deliver(|e| e.to != C && !matches!(&e.msg, Message::Append(a) if !a.entries.is_empty() && a.term > Term(2)));
        c.advance(500);
    }
    c.inbox.clear();
    let a = c.m(A);
    assert_eq!(a.role(), Role::Leader);
    let term = a.term();
    assert!(term >= Term(4), "{term:?}");
    // The term-2 entry, the two `Noop`s A wrote when it resumed term 2, then the `Noop` of A's new term.
    assert_eq!(a.log().len() as u64, base + 4, "A's log: {:?}", a.log().iter().map(|e| e.term).collect::<Vec<_>>());
    assert!(a.log()[base as usize..base as usize + 3].iter().all(|e| e.term == Term(2)));
    assert!(matches!(a.log()[base as usize + 3].body, EntryBody::Noop));
    // An append that carries only the term-2 entry, as one cut short by `CATCHUP_MAX` would, reaches B, and B's ack
    // reaches A.
    let req = graftyard_raft::AppendReq {
        term,
        leader: A,
        boot: a.boot(),
        round: 0,
        sent_at: c.now,
        prev_index: Index(base),
        prev_term: a.log()[base as usize - 1].term,
        prev_digest: a.digests()[base as usize - 1],
        entries: vec![a.log()[base as usize].clone()],
        commit: a.state().commit,
        transfer: None,
        lead_seq: a.state().lead_seq,
    };
    c.on(B, |m, now| m.step(now, A, Message::Append(req)));
    assert_eq!(c.m(B).log().len() as u64, base + 1);
    c.deliver(|e| e.from == B && e.to == A);
    assert_eq!(c.m(A).state().commit, Index(base), "A committed a term-2 entry by counting a replica in {term:?}");
    // Once B also stores the Noop of A's term, both commit.
    c.inbox.clear();
    c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
    c.deliver(|e| e.to != C);
    assert_eq!(c.m(A).state().commit, Index(base + 4));
}

/// The leader's AppendEntries leave before its own write is durable (Raft dissertation 10.2.1), and the leader counts
/// its own copy only once the host hands back the input's `SelfAck`. One follower's ack is not a quorum until then.
#[test]
fn the_leader_counts_itself_only_after_its_own_write_confirms() {
    let mut c = Cluster::new(31);
    c.elect(A);
    c.sync(A);
    let (r, pending) = c.on_unconfirmed(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(900)), put("k", "v")));
    let t = r.expect("admitted").ticket;
    let index = Index(c.m(A).log().len() as u64);
    assert!(
        c.inbox.iter().any(|e| matches!(&e.msg, Message::Append(a) if a.entries.iter().any(|x| x.index == index))),
        "the entry's AppendEntries waited for the leader's own write"
    );
    assert!(pending.out.self_ack.is_some());
    c.deliver(|e| e.to != C && e.from != C);
    assert_eq!(c.m(B).log().len() as u64, index.0, "B stored the entry");
    assert_eq!(c.m(A).outcome(t), None, "A counted its own unconfirmed copy toward the quorum");
    assert!(c.m(A).state().commit < index);
    let b_acked = c.now;
    c.advance(40);
    c.confirm(pending);
    let got = c.m(A).outcome(t).expect("resolved once A's own write confirmed").expect("acked");
    assert_eq!(got.index, index);
    // The proof says so on A's clock: A's own copy counted only once its confirm came back, after B's ack, and the
    // commit came no earlier than either.
    let p = got.quorum();
    assert_eq!((p.leader(), p.index()), (A, index));
    let own = p.acks().iter().find(|d| d.site == A).expect("A counted");
    let b = p.acks().iter().find(|d| d.site == B).expect("B counted");
    assert!(own.durable >= index && b.durable >= index);
    assert_eq!(own.at, c.now, "A's ack is its own confirm");
    assert!(b.at >= p.entry_at() && b.at <= b_acked);
    assert_eq!(p.at(), c.now, "the commit came with A's own confirm");
}

/// A leader's AppendEntries left, then it reset before its own write was durable: C holds an entry at (term, k) that
/// A lost. A resumes `led_term` and writes two `Noop`s from k; it may commit only the second. Had it committed a
/// `Noop` at k with B, B and C would hold different entries at the same (term, k), C's vote request would look as
/// up to date as B's log, and C could win and overwrite the committed index.
#[test]
fn a_resumed_leader_never_commits_beside_an_entry_it_lost() {
    let mut c = Cluster::new(32);
    c.elect(A);
    c.sync(A);
    c.inbox.clear();
    let term = c.m(A).term();
    let k = c.m(A).log().len() as u64 + 1;
    let sent = c.on_send_then_crash(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(901)), put("k", "ghost")));
    assert!(sent > 0, "the AppendEntries left before the crash");
    c.deliver(|e| e.to == C);
    assert_eq!(c.m(C).log().len() as u64, k, "C stored the entry A lost");
    c.inbox.clear();
    c.restart(A);
    assert_eq!(c.m(A).log().len() as u64, k - 1, "A lost it");
    assert_eq!(c.on(A, |m, now| m.campaign(now)), Verdict::Resumed);
    c.deliver(|e| e.to != C && e.from != C);
    assert_eq!(c.m(A).term(), term);
    assert!(matches!(c.m(A).log()[k as usize - 1].body, EntryBody::Noop), "A wrote a Noop at k");
    assert_ne!(c.m(A).digests()[k as usize - 1], c.m(C).digests()[k as usize - 1]);
    assert_eq!(c.m(A).state().commit, Index(k + 1), "A commits the second Noop, with B");
    // A resets again; C campaigns once every promise lapsed. B holds (term, k + 1), so C's log is behind.
    c.crash(A);
    c.inbox.clear();
    c.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1);
    c.deliver(|_| false);
    c.on(C, |m, now| m.campaign(now));
    c.deliver(|e| e.to != A);
    assert_ne!(c.m(C).role(), Role::Leader, "C won with the entry A lost, over the committed index {k}");
    // B, which holds the committed entries, leads, and C's lost entry is replaced.
    c.on(B, |m, now| m.campaign(now));
    c.deliver(|e| e.to != A);
    assert_eq!(c.m(B).role(), Role::Leader);
    c.sync(B);
    assert_eq!(c.m(C).digests()[k as usize - 1], c.m(B).digests()[k as usize - 1]);
    assert_eq!(c.value(C, "k"), None, "the lost command never applied");
}

/// `GroupFold::from_parts` loads a fold from its tables equal to the one the log builds, including the request rows
/// it must drop later.
#[test]
fn a_group_fold_loaded_from_its_tables_equals_the_replayed_one() {
    let mut c = Cluster::new(33);
    c.elect(A);
    for i in 0..5 {
        admit(&mut c, A, 100 + i, append("k", &i.to_string()));
        c.deliver_all();
    }
    c.sync(A);
    let m = c.m(A);
    let g = m.group_fold();
    let loaded = graftyard_raft::GroupFold::from_parts(
        g.fold.clone(),
        g.applied(),
        g.config().cloned(),
        g.id_block(),
        Box::new(g.requests().into_iter().collect::<graftyard_raft::MemRequests<_>>()),
    );
    assert_eq!(&loaded, g);
    // And they stay equal as more entries apply, past REQUEST_RETENTION, so both drop the same rows.
    let (mut a, mut b) = (g.clone(), loaded);
    let mut next = m.log().last().unwrap().clone();
    for k in 1..4u64 {
        next.index = graftyard_raft::Index(next.index.0 + 1);
        next.at = Timestamp(next.at.0 + k * graftyard_raft::REQUEST_RETENTION.as_millis() as u64);
        next.requests = smallvec::smallvec![(RequestId(graftyard_raft::Ulid(900 + u128::from(k))), 0..1)];
        next.body = EntryBody::Events(vec![common::kv::KvEvent::Push { key: "k".into(), s: k.to_string() }]);
        a.apply(&next);
        b.apply(&next);
        assert_eq!(a, b);
    }
    assert_eq!(a.requests().len(), 1, "older rows expired");
}

/// A delayed AppendEntries from a reset leader's earlier instance, in the same term, must not truncate what the
/// resumed instance committed (long sweep, HARSH seed 10385). The earlier instance sent entries before its own write
/// was durable; the resumed one wrote other entries at those indexes.
#[test]
fn a_stale_instance_of_a_resumed_leader_cannot_truncate_its_successor() {
    let mut c = Cluster::new(34);
    c.elect(A);
    c.sync(A);
    c.inbox.clear();
    let k = c.m(A).log().len() as u64 + 1;
    c.on_send_then_crash(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(950)), put("k", "ghost")));
    // The earlier instance's AppendEntries for k is held back.
    let stale: Vec<_> = c.inbox.drain(..).filter(|e| e.to == B).collect();
    assert!(!stale.is_empty());
    c.restart(A);
    assert_eq!(c.on(A, |m, now| m.campaign(now)), Verdict::Resumed);
    c.deliver(|e| e.to != C && e.from != C);
    let commit = c.m(A).state().commit;
    assert!(commit >= Index(k + 1), "the resumed instance committed with B");
    let before = c.m(B).digests()[..commit.0 as usize].to_vec();
    // The stale append arrives at B now.
    for e in stale {
        let graftyard_raft::Envelope { from, to, msg } = e;
        c.on(to, |m, now| m.step(now, from, msg));
    }
    assert_eq!(&c.m(B).digests()[..commit.0 as usize], &before[..], "B lost entries A's quorum committed");
}

/// `Member::id_block`: the block is its own entry, commits before any command queued after it, reaches the fold
/// through `Fold::apply_id_block` on every member, and a block that does not follow the latest one is refused by the
/// leader (`Behind`) and by a follower's `verify`.
#[test]
fn id_blocks_commit_as_their_own_entry_and_never_rewind() {
    use graftyard_raft::{IdBlock, IdBlockError};
    let mut c = Cluster::new(35);
    c.elect(A);
    c.sync(A);
    let b5 = IdBlock::whole(0, 5).unwrap();
    assert_eq!(c.on(A, |m, now| m.id_block(now, b5)), Ok(()));
    // Taking the same block again while it is in flight is a no-op.
    assert_eq!(c.on(A, |m, now| m.id_block(now, b5)), Ok(()));
    let t = admit(&mut c, A, 700, put("k", "v"));
    c.deliver_all();
    c.sync(A);
    let at = c
        .resolutions
        .iter()
        .find_map(|(_, r)| match r {
            Resolution::IdBlock { block, result: Ok(i) } if *block == b5 => Some(*i),
            _ => None,
        })
        .expect("the block committed");
    let ticket_at = c
        .ticket_results(t)
        .iter()
        .find_map(|r| match r {
            Resolution::Ticket { result: Ok(a), .. } => Some(a.index),
            _ => None,
        })
        .unwrap();
    assert!(at < ticket_at, "the block commits before the command queued after it");
    for s in SITES {
        assert_eq!(c.m(s).group_fold().id_block(), Some(b5), "{s:?}");
        assert_eq!(c.value(s, common::kv::ID_KEY), Some(b5.base().0.to_string()), "{s:?} fold saw the block");
        assert!(matches!(&c.m(s).log()[at.0 as usize - 1].body, EntryBody::IdBlock(b) if *b == b5));
        assert!(c.m(s).log()[at.0 as usize - 1].requests.is_empty());
    }
    // An older or equal block is refused by the leader.
    let b4 = IdBlock::whole(0, 4).unwrap();
    for b in [b4, b5] {
        assert_eq!(c.on(A, |m, now| m.id_block(now, b)), Err(IdBlockError::Behind { current: b5 }));
    }
    // And by a follower's `verify`, whatever a faulty leader sends.
    let tail = c.m(B).log().last().unwrap().clone();
    let mut bad = tail.clone();
    bad.index = Index(tail.index.0 + 1);
    bad.prev = *c.m(B).digests().last().unwrap();
    bad.requests.clear();
    bad.body = EntryBody::IdBlock(b4);
    let m = c.m(B);
    let view = graftyard_raft::FoldView {
        fold: &m.group_fold().fold,
        id_block: m.group_fold().id_block(),
        unapplied: &[],
        index: bad.index,
        prev: bad.prev,
        boot: m.digests()[0],
        reads: &[common::kv::KV_FORMAT],
        release: config(B).release,
    };
    assert!(graftyard_raft::verify(&bad, &view).unwrap_err().reason.contains("does not follow"));
    let mut with_request = bad.clone();
    with_request.body = EntryBody::IdBlock(IdBlock::whole(0, 6).unwrap());
    assert!(graftyard_raft::verify(&with_request, &view).is_ok());
    with_request.requests = smallvec::smallvec![(RequestId(graftyard_raft::Ulid(1)), 0..0)];
    assert!(graftyard_raft::verify(&with_request, &view).unwrap_err().reason.contains("requests"));
    // A block taken when the leadership ends answers `NotLeader`.
    let b6 = IdBlock::whole(0, 6).unwrap();
    c.on(A, |m, now| m.id_block(now, b6)).unwrap();
    c.crash(A);
    c.restart(A);
    let lost =
        c.resolutions.iter().any(|(_, r)| matches!(r, Resolution::IdBlock { block, result: Err(_) } if *block == b6));
    let held = c.m(A).log().iter().any(|e| matches!(e.body, EntryBody::IdBlock(b) if b == b6));
    assert!(lost || held, "a block in flight either resolves NotLeader or stays in the stored log");
}

/// A member retried or replaced on its own site (BF-11): `Degraded { lost }`, then `Normal` naming a new member of
/// the next generation on that site, which starts empty and catches up from the leader. Changing the member on a site
/// the config keeps, in one step, is refused, and nothing the old member acknowledged counts for the new one.
#[test]
fn a_member_is_replaced_on_its_own_site_in_two_config_changes() {
    use graftyard_raft::{GroupConfig, ReconfigError};
    use graftyard_types::{Member as MemberRef, MemberKind, Placement, PlacementChecks, Region, Rtt, Site, SiteTable};
    let mut c = Cluster::new(36);
    c.elect(A);
    c.sync(A);
    for i in 0..5 {
        admit(&mut c, A, 800 + i, append("k", &format!("{i},")));
        c.deliver_all();
    }
    c.sync(A);
    let Some((_, GroupConfig::Normal(p))) = c.m(A).committed_config().cloned() else { panic!("normal config") };
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
    let new_b = MemberRef::new(CHAIN, B, MemberKind::Full, 1, p.next_generation());
    let voters = p.voters().iter().map(|m| if m.site == B { new_b.clone() } else { m.clone() }).collect();
    let np = Placement::new(voters, Default::default(), p.home(), &checks).unwrap();
    // One step that swaps the member on a site the config keeps is refused.
    let swap = c.on(A, |m, now| m.reconfigure(now, GroupConfig::Normal(Box::new(np.clone()))));
    assert_eq!(swap, Err(ReconfigError::NotOneChange));
    // The old member is lost; the group goes Degraded without it.
    c.crash(B);
    let d = c.on(A, |m, now| m.reconfigure(now, GroupConfig::Degraded { placement: p.clone(), lost: B })).unwrap();
    // A second change before the first commits is refused.
    assert_eq!(
        c.on(A, |m, now| m.reconfigure(now, GroupConfig::Normal(Box::new(np.clone())))),
        Err(ReconfigError::Busy)
    );
    c.deliver_all();
    c.sync(A);
    assert!(c.m(A).committed_config().is_some_and(|x| x.0 >= d));
    admit(&mut c, A, 900, append("k", "degraded,"));
    c.deliver_all();
    c.sync(A);
    assert_eq!(c.value(A, "k").as_deref(), Some("0,1,2,3,4,degraded,"));
    // The new member: an empty object on site B.
    c.members[B.0 as usize] = Some(Member::new(config(B), 99).unwrap());
    c.disks[B.0 as usize] = graftyard_raft::Stored::new(common::kv::Kv::default());
    let n = c.on(A, |m, now| m.reconfigure(now, GroupConfig::Normal(Box::new(np.clone())))).unwrap();
    c.deliver_all();
    c.sync(A);
    c.sync(A);
    for s in SITES {
        assert!(c.m(s).committed_config().is_some_and(|x| x.0 >= n), "{s:?} committed the new config");
        assert_eq!(c.value(s, "k").as_deref(), Some("0,1,2,3,4,degraded,"), "{s:?}");
    }
    assert_eq!(c.m(B).digests(), c.m(A).digests());
    // The leader leaving the config is refused: transfer first.
    let without_a = GroupConfig::Degraded { placement: Box::new(np), lost: A };
    assert_eq!(c.on(A, |m, now| m.reconfigure(now, without_a)), Err(ReconfigError::RemovesLeader));
}

/// A follower behind on large entries catches up in calls under APPEND_BYTES_MAX, not CATCHUP_MAX entries at once.
/// Milestone 1's repository groups carry packs up to 1 MB inline, and 64 of them in one AppendEntries (9 MB) missed
/// CALL_DEADLINE on every send, so the follower never caught up (`tests/m1/results/m1-wp8.md`, t-256). Each entry here
/// is four times APPEND_BYTES_MAX, as an entry with agent packs or one large command can be: every call still carries
/// one whole entry, so nothing stalls.
#[test]
fn a_lagging_follower_catches_up_in_calls_under_the_byte_cap() {
    let mut c = Cluster::new(23);
    c.elect(A);
    c.sync(A);
    // C misses ten entries of 400 kB each, one command per entry.
    let big = "x".repeat(400_000);
    let behind = c.m(C).log().len();
    for i in 0..10u128 {
        admit(&mut c, A, 900 + i, put(&format!("k{i}"), &big));
        c.deliver(|e| e.to != C && e.from != C);
        c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
        c.deliver(|e| e.to != C && e.from != C);
    }
    c.inbox.clear();
    assert_eq!(c.m(C).log().len(), behind, "C saw none of them");
    assert!(c.m(A).log().len() >= behind + 10, "A: {} entries, C: {behind}", c.m(A).log().len());
    assert!(big.len() > 3 * graftyard_raft::APPEND_BYTES_MAX);
    // Each later round carries one whole entry, and at most APPEND_BYTES_MAX past it; C still catches up.
    let (mut most, mut empty) = (0, 0);
    for _ in 0..40 {
        c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
        let mut sizes = Vec::new();
        let a_last = c.m(A).last_index();
        c.deliver(|e| {
            if let Message::Append(a) = &e.msg
                && e.to == C
            {
                // A call that starts behind A's last entry carries at least one.
                empty += usize::from(a.entries.is_empty() && a.prev_index < a_last);
                if a.entries.len() > 1 {
                    sizes.push(a.entries[1..].iter().map(|x| graftyard_raft::encode_entry(x).len()).sum::<usize>());
                }
            }
            true
        });
        most = sizes.into_iter().fold(most, usize::max);
        if c.m(C).log().len() == c.m(A).log().len() {
            break;
        }
    }
    assert_eq!(c.m(C).digests(), c.m(A).digests(), "C caught up");
    assert_eq!(empty, 0, "calls to C that started behind A's last entry carried none");
    assert!(most <= graftyard_raft::APPEND_BYTES_MAX, "one AppendEntries carried {most} bytes past its first entry");
}

/// A stored fold outside the stored log window (`base <= applied <= base + log.len()`) is refused with an error the
/// host answers 500 to, not a panic, which would reset every object in the isolate.
#[test]
fn restore_refuses_a_fold_outside_the_stored_log_window() {
    let mut c = Cluster::new(46);
    c.elect(A);
    admit(&mut c, A, 9_000, put("k", "v"));
    c.deliver_all();
    c.sync(A);
    let disk = c.disks[B.0 as usize].clone();
    let applied = disk.fold.applied();
    assert!(applied.0 >= 2 && disk.base.index == Index(0), "B applied {applied:?} from a whole log");
    let window = |r: Result<_, RestoreError>| match r {
        Err(RestoreError::Window { base, applied, last }) => (base, applied, last),
        Err(e) => panic!("restore refused for another reason: {e}"),
        Ok(_) => panic!("restore took a fold outside the log window"),
    };

    // The log's base is above the applied fold.
    let mut above = disk.clone();
    above.base.index = Index(applied.0 + 1);
    above.log.clear();
    let next = Index(applied.0 + 1);
    assert_eq!(window(Member::<KvCmd>::restore(config(B), above, c.now, 7)), (next, applied, next));

    // The fold has applied past the log's last entry.
    let mut short = disk;
    short.log.truncate(applied.0 as usize - 1);
    let last = Index(applied.0 - 1);
    assert_eq!(window(Member::<KvCmd>::restore(config(B), short, c.now, 7)), (Index(0), applied, last));
}

/// F45: a window bounded by entry count alone holds 1,000 entries of up to 1.5 MB, which a 128 MB isolate cannot
/// (`tests/m1/results/m1-f43.md`, "A different defect the soak found"). With a byte bound the leader keeps only the
/// newest applied entries that fit, and the last applied one even when it alone is larger; `restore` compacts to the
/// same bound.
#[test]
fn the_in_memory_log_is_bounded_by_bytes() {
    let mut c = Cluster::new(45);
    c.elect(A);
    c.sync(A);
    let budget = 300_000;
    for s in SITES {
        c.members[s.0 as usize].as_mut().unwrap().set_log_window_bytes(1_000, budget);
    }
    let big = "x".repeat(100_000);
    for i in 0..20u128 {
        admit(&mut c, A, 5_000 + i, put(&format!("k{i}"), &big));
        c.deliver_all();
        c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
        c.deliver_all();
    }
    let m = c.m(A);
    let applied = m.state().applied.0 - m.log_base().index.0;
    assert!(m.state().applied.0 >= 20, "the entries applied");
    let held: usize = m.log()[..applied as usize].iter().map(|e| graftyard_raft::encode_entry(e).len()).sum();
    assert!(held <= budget, "{held} bytes of applied entries in memory, budget {budget}");
    assert!(m.log_bytes() < budget + 2 * 101_000, "log_bytes {}", m.log_bytes());
    assert_eq!(c.value(A, "k19").as_deref(), Some(big.as_str()), "the fold is whole");
    // One entry above the budget still stays: the last applied one.
    c.members[0].as_mut().unwrap().set_log_window_bytes(1_000, 1_000);
    let a = c.m(A);
    assert_eq!(a.log_base().index.0, a.state().applied.0 - 1, "only the last applied entry stays");
    assert!(a.entry_at(a.state().applied).is_some());
    // A restart under the same bound holds no more than the bound, whatever the stored log holds.
    c.crash(B);
    c.restart(B);
    c.members[1].as_mut().unwrap().set_log_window_bytes(1_000, budget);
    let b = c.m(B);
    let applied = (b.state().applied.0 - b.log_base().index.0) as usize;
    let held: usize = b.log()[..applied].iter().map(|e| graftyard_raft::encode_entry(e).len()).sum();
    assert!(held <= budget, "restored B holds {held} bytes of applied entries");
    assert!(b.log_base().index.0 > 0, "restore compacted");
}

/// L0-12: a follower learns a stall from the leader's re-send of an entry no quorum acknowledged within CALL_DEADLINE,
/// and marks the entry's `at`, so it can stamp `extend` if it wins the next term. A late copy of an append the group
/// already committed past marks nothing, and neither does a prompt one.
#[test]
fn a_follower_marks_a_stall_only_from_a_late_resend_of_an_uncommitted_entry() {
    let mut c = Cluster::new(17);
    c.elect(A);
    c.sync(A);
    let id = RequestId(graftyard_raft::Ulid(7_000));
    c.on(A, |m, now| m.commit(now, id, put("k", "v"))).expect("leader admits");
    let at = c.m(A).log().last().unwrap().at;
    // The first append and its answers are lost, both ways.
    let first: Vec<_> =
        std::mem::take(&mut c.inbox).into_iter().filter(|e| matches!(e.msg, Message::Append(_))).collect();
    for s in [B, C] {
        assert_eq!(c.m(s).state().stalled_since, None, "{s:?} holds no marker before the stall");
    }
    // A prompt delivery of that append marks nothing.
    let prompt = first.iter().find(|e| e.to == B).cloned().unwrap();
    c.on(B, |m, now| m.step(now, prompt.from, prompt.msg));
    c.inbox.clear();
    assert_eq!(c.m(B).state().stalled_since, None, "an append sent in time is no stall");
    // CALL_DEADLINE later the leader re-sends: B and C mark the entry's `at`.
    c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
    let resent: Vec<_> =
        std::mem::take(&mut c.inbox).into_iter().filter(|e| matches!(e.msg, Message::Append(_))).collect();
    for e in resent.iter().filter(|e| e.to == C) {
        let (from, msg) = (e.from, e.msg.clone());
        c.on(C, |m, now| m.step(now, from, msg));
    }
    assert_eq!(c.m(C).state().stalled_since, Some(at), "the late re-send marks the entry's at");
    // Everything settles and commits; then a delayed copy of the re-send reaches B, which has committed past it.
    // B learns that an entry stamped after the marker committed only from the append after it, so it syncs twice.
    c.inbox.clear();
    c.deliver_all();
    c.sync(A);
    c.sync(A);
    assert!(c.m(B).state().commit.0 >= c.m(A).log().iter().find(|e| e.at == at).unwrap().index.0);
    assert_eq!(c.m(B).state().stalled_since, None, "B committed past the stall");
    let late = resent.into_iter().find(|e| e.to == B).unwrap();
    c.on(B, |m, now| m.step(now, late.from, late.msg));
    assert_eq!(c.m(B).state().stalled_since, None, "a delayed append past the commit marks nothing");
}

/// B0-02's large entries (`tests/m0/results/carried-6.md`, `carried-8.md`): a backlog that queues behind the entry in
/// flight splits into entries within ENTRY_SOFT, each command in admission order and exactly once, and every one
/// commits.
#[test]
fn a_backlog_splits_into_entries_within_entry_soft_in_order() {
    use graftyard_raft::{ENTRY_SOFT, encode_entry};
    let mut c = Cluster::new(41);
    c.elect(A);
    let hold = admit(&mut c, A, 1_000, put("hold", "x"));
    let first = c.m(A).log().len() as u64 + 1;
    // 300 commands of about 1 KB, as B0-02's puts: about 300 KB queued behind `hold`.
    let value = "v".repeat(1_000);
    let tickets: Vec<_> =
        (0..300u32).map(|i| admit(&mut c, A, 2_000 + u128::from(i), append("k", &format!("{i:03}{value}")))).collect();
    for _ in 0..40 {
        c.deliver_all();
        c.advance(10);
    }
    assert!(!c.ticket_results(hold).is_empty());
    let entries: Vec<_> =
        c.m(A).log().iter().filter(|e| e.index.0 >= first && !e.requests.is_empty()).cloned().collect();
    assert!(entries.len() >= 3, "{} entries for 300 KB", entries.len());
    let mut seen = Vec::new();
    for e in &entries {
        if let EntryBody::Events(evs) = &e.body {
            for ev in evs {
                if let common::kv::KvEvent::Push { s, .. } = ev {
                    seen.push(s[..3].parse::<u32>().unwrap());
                }
            }
        }
    }
    // Every entry fits its bound, with its encoding's overhead: ENTRY_SOFT_DEEP while more than DEEP_QUEUE entries'
    // worth was still queued behind it (soft-grow), else ENTRY_SOFT. All but the last have no room for one more
    // command: a cap that never binds or one that splits early both fail here.
    use graftyard_raft::{DEEP_QUEUE, ENTRY_SOFT_DEEP};
    let lens: Vec<usize> = entries.iter().map(|e| encode_entry(e).len()).collect();
    let mut queued: usize = lens.iter().sum();
    for (i, l) in lens.iter().enumerate() {
        let bound = if queued > DEEP_QUEUE * ENTRY_SOFT + 3_000 { ENTRY_SOFT_DEEP } else { ENTRY_SOFT };
        // The encoding adds about 30 bytes a command past what the cut counts.
        assert!(*l <= bound + bound / 30, "entry {i} of {l} bytes from {queued} queued: {lens:?}");
        assert!(i + 1 == lens.len() || *l > bound - 2_000, "entry {i} split early: {lens:?}");
        queued -= l;
    }
    assert_eq!(seen, (0..300).collect::<Vec<_>>(), "every command once, in admission order");
    for t in tickets {
        assert!(c.ticket_results(t).iter().any(|r| matches!(r, Resolution::Ticket { result: Ok(_), .. })));
    }
    // Every follower holds the same entries.
    for s in [B, C] {
        assert_eq!(c.m(s).log().len(), c.m(A).log().len());
    }
}

/// B0-02's far follower (`tests/m0/results/carried-8.md`): the near follower B commits entry k-1 while the far
/// follower C's call for it is still out, then goes silent. Entry k must reach C at once, in a call of its own while
/// the older one is still out, so A and C commit it; with one call per voter it waited for the older call to end.
/// The held calls then arrive late, out of order and twice, and every voter still ends with the leader's log.
#[test]
fn a_far_voter_gets_each_entry_while_its_earlier_calls_are_out() {
    let mut c = Cluster::new(43);
    c.elect(A);
    c.sync(A);
    let first = admit(&mut c, A, 9_000, put("k0", "v"));
    // Entry k-1 goes to both; B answers and it commits, C's call is held unanswered.
    let mut held: Vec<graftyard_raft::Envelope<common::kv::KvEvent>> = Vec::new();
    let to_c = |e: &graftyard_raft::Envelope<common::kv::KvEvent>| {
        e.to == C && matches!(&e.msg, Message::Append(a) if !a.entries.is_empty())
    };
    let mut second = None;
    for _ in 0..6 {
        let (c_calls, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut c.inbox).into_iter().partition(|e| to_c(e));
        held.extend(c_calls);
        for e in rest {
            // Once entry k exists, B hears nothing more.
            if second.is_some() && (e.to == B || e.from == B) {
                continue;
            }
            let graftyard_raft::Envelope { from, to, msg } = e;
            c.on(to, |m, now| m.step(now, from, msg));
        }
        if second.is_none()
            && c.ticket_results(first).iter().any(|r| matches!(r, Resolution::Ticket { result: Ok(_), .. }))
        {
            second = Some(admit(&mut c, A, 9_001, put("k1", "v")));
        }
    }
    let second = second.expect("entry k-1 committed with B");
    let k = c.m(A).last_index();
    // A call carrying entry k left for C while C's call for k-1 was still out.
    let carrying_k =
        held.iter().position(|e| matches!(&e.msg, Message::Append(a) if a.entries.iter().any(|x| x.index == k)));
    let Some(i) = carrying_k else { panic!("entry {k:?} never left for C while its earlier call was out") };
    assert!(i > 0, "the older call to C was sent first");
    // It carries only what follows the calls already out, not their entries again.
    let ends = |e: &graftyard_raft::Envelope<common::kv::KvEvent>| match &e.msg {
        Message::Append(a) => (a.prev_index, a.entries.last().map_or(a.prev_index, |x| x.index)),
        _ => unreachable!(),
    };
    assert_eq!(ends(&held[i]).0, ends(&held[i - 1]).1, "the call for entry k re-sent entries already out");
    // C stores the older calls in order, but their answers are still on the way; then it takes the newer call, and
    // only that answer arrives: A and C commit entry k with B silent.
    for e in &held[..i] {
        let graftyard_raft::Envelope { from, to, msg } = e.clone();
        c.on(to, |m, now| m.step(now, from, msg));
    }
    c.inbox.clear();
    let graftyard_raft::Envelope { from, to, msg } = held[i].clone();
    c.on(to, |m, now| m.step(now, from, msg));
    for e in std::mem::take(&mut c.inbox) {
        if e.to == A {
            let graftyard_raft::Envelope { from, to, msg } = e;
            c.on(to, |m, now| m.step(now, from, msg));
        }
    }
    assert!(
        c.ticket_results(second).iter().any(|r| matches!(r, Resolution::Ticket { result: Ok(_), .. })),
        "entry k did not commit on A and C"
    );
    // The held calls arrive again, newest first and each twice: C ignores what it holds, and B catches up.
    for e in held.iter().rev().chain(held.iter()) {
        let graftyard_raft::Envelope { from, to, msg } = e.clone();
        c.on(to, |m, now| m.step(now, from, msg));
    }
    c.deliver_all();
    c.sync(A);
    for s in [B, C] {
        assert_eq!(c.m(s).digests(), c.m(A).digests(), "{s:?}");
    }
}

/// The pipeline is bounded: while PIPELINE calls to a voter are out and unanswered, no further call leaves for it.
#[test]
fn a_voter_never_has_more_than_pipeline_calls_out() {
    use graftyard_raft::PIPELINE;
    let mut c = Cluster::new(44);
    c.elect(A);
    c.sync(A);
    let mut to_c = 0;
    for i in 0..(PIPELINE as u128 * 3) {
        admit(&mut c, A, 9_500 + i, put(&format!("p{i}"), "v"));
        // B answers everything, so each entry commits and the next is cut; C answers nothing.
        for _ in 0..4 {
            let msgs = std::mem::take(&mut c.inbox);
            for e in msgs {
                if e.to == C {
                    if matches!(&e.msg, Message::Append(a) if !a.entries.is_empty()) {
                        to_c += 1;
                    }
                    continue;
                }
                let graftyard_raft::Envelope { from, to, msg } = e;
                c.on(to, |m, now| m.step(now, from, msg));
            }
        }
    }
    assert_eq!(to_c, PIPELINE, "calls with entries sent to the silent voter");
}

/// Pipelining to a voter stops only on a lossy link (F67 rule 13), not on one missed call. A voter whose calls fail
/// over `commit_errors` in the window gets one call at a time: a half-hung leader whose few answered calls each let
/// PIPELINE more out held a live-sweep group 50 s (seed 11497). A voter that missed one call among many answered keeps
/// its pipeline: with the pipeline off for the window after one miss, B0-02's far voter caught up one backlog call at
/// a time and missed on each (r330-6, `tests/m0/results/carried-8.md`).
#[test]
fn a_voter_is_pipelined_unless_its_link_is_lossy() {
    // Calls with entries that reach C while C answers none of them, after `fail` of C's first calls failed with an
    // error and the rest of the first ten entries were answered.
    let silent_calls = |fail: usize| {
        let mut c = Cluster::new(46);
        c.elect(A);
        c.sync(A);
        let mut failed = 0;
        for i in 0..10u128 {
            admit(&mut c, A, 9_700 + i, put(&format!("g{i}"), "v"));
            for _ in 0..4 {
                for e in std::mem::take(&mut c.inbox) {
                    if e.to == C && failed < fail && matches!(&e.msg, Message::Append(a) if !a.entries.is_empty()) {
                        failed += 1;
                        let f = e.failure(false).expect("an append");
                        c.on(A, |m, now| m.call_failed(now, f));
                        continue;
                    }
                    let graftyard_raft::Envelope { from, to, msg } = e;
                    c.on(to, |m, now| m.step(now, from, msg));
                }
            }
        }
        assert_eq!(failed, fail);
        // Past CALL_DEADLINE, so a leader whose retries to C ran out sends again from its tick.
        c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
        // The idle leader's commit round to a C that holds every entry carries none; C answers it, as it would within
        // a round trip, so it holds no pipeline slot below.
        let (beat, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut c.inbox)
            .into_iter()
            .partition(|e| e.to == C && matches!(&e.msg, Message::Append(a) if a.entries.is_empty()));
        c.inbox = rest;
        for graftyard_raft::Envelope { from, to, msg } in beat {
            c.on(to, |m, now| m.step(now, from, msg));
        }
        let mut to_c = 0;
        for i in 0..(graftyard_raft::PIPELINE as u128 * 3) {
            admit(&mut c, A, 9_800 + i, put(&format!("h{i}"), "v"));
            for _ in 0..4 {
                for e in std::mem::take(&mut c.inbox) {
                    if e.to == C {
                        to_c += usize::from(matches!(&e.msg, Message::Append(a) if !a.entries.is_empty()));
                        continue;
                    }
                    let graftyard_raft::Envelope { from, to, msg } = e;
                    c.on(to, |m, now| m.step(now, from, msg));
                }
            }
        }
        to_c
    };
    // One failure among the answered calls: under `commit_fail_min`, so C stays pipelined.
    assert_eq!(silent_calls(1), graftyard_raft::PIPELINE, "a voter that missed once");
    // Every call failing, at least `commit_fail_min` of them: lossy, one call at a time.
    let lossy = usize::from(graftyard_raft::STEP_DOWN.commit_fail_min);
    assert_eq!(silent_calls(lossy), 1, "a voter on a lossy link");
}

/// A learner's read-index answer is preceded by the entries it needs: when the leader answers a learner's batch at
/// index i and has not sent it entries through i, an AppendEntries through i, carrying a commit at or past i, leaves
/// ahead of the `ReadIndexRes`, so both ride one response to the learner's call. Before, a learner got one call at a
/// time, its entries trailed its answer by one or two more calls, and far learners waited p99 1.9 to 4.5 s for them.
/// With a call in flight the append follows it, with no entry sent twice: re-sent from the acknowledged entry on every
/// read, the leader reached 122 MiB. After a refusal emptied the pipeline it goes from the acknowledged entry at once:
/// a far learner's calls end in any order, and waiting for CALL_DEADLINE's resend held one apac learner's entries p99
/// 4.8 to 7.8 s (`tests/am1/results/narrow-reads-3.md`, "Where a far learner's wait goes"). A learner whose link is
/// lossy still gets one call at a time (F67 rule 13).
#[test]
fn a_learner_read_index_answer_follows_the_entries_it_needs() {
    use graftyard_raft::{GroupConfig, STEP_DOWN};
    use graftyard_types::{
        Member as MemberRef, MemberKind, Placement, PlacementChecks, Region, Rtt, Site, SiteTable, learner_addr,
    };
    // Per read, the first entry of an append ahead of the answer that carried entries through the read index and a
    // commit at or past it, or None; with the tick's call's first entry and the entry after its last. The learner
    // acknowledges no append; `fail` of its first calls failed with an error. Before the second read, a refusal of
    // an older call empties the leader's pipeline to the learner.
    let before_answer = |fail: usize| {
        let mut c = Cluster::new(49);
        c.elect(A);
        c.sync(A);
        let Some((_, GroupConfig::Normal(p))) = c.m(A).committed_config().cloned() else { panic!("normal config") };
        let l = MemberRef::new(CHAIN, B, MemberKind::Learner, 0, p.next_generation());
        let addr = learner_addr(0);
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
        let rtt: Vec<(SiteId, Rtt)> = [(A, B), (A, C), (B, C)]
            .iter()
            .map(|&(x, y)| (x, Rtt { peer: y, median_ms: 80, at: Timestamp(0) }))
            .collect();
        let checks = PlacementChecks { sites: &table, separate_accounts: false, rtt: &rtt, near: &[] };
        let np =
            Placement::new(p.voters().iter().cloned().collect(), [l].into_iter().collect(), p.home(), &checks).unwrap();
        c.on(A, |m, now| m.reconfigure(now, GroupConfig::Normal(Box::new(np.clone())))).unwrap();
        // Voters answer; the learner is not run here, and its calls stay outstanding unless failed.
        let failed = std::cell::Cell::new(0);
        let step = |c: &mut Cluster| {
            for _ in 0..4 {
                for e in std::mem::take(&mut c.inbox) {
                    if e.to != addr {
                        let graftyard_raft::Envelope { from, to, msg } = e;
                        c.on(to, |m, now| m.step(now, from, msg));
                    } else if failed.get() < fail && matches!(&e.msg, Message::Append(_)) {
                        failed.set(failed.get() + 1);
                        let f = e.failure(false).expect("an append");
                        c.on(A, |m, now| m.call_failed(now, f));
                    }
                }
            }
        };
        for i in 0..10u128 {
            admit(&mut c, A, 9_960 + i, put(&format!("r{i}"), "v"));
            step(&mut c);
        }
        assert_eq!(failed.get(), fail);
        // Past CALL_DEADLINE, so a leader whose retries to the learner ran out sends a fresh call from its tick.
        c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
        let mut early = None;
        for _ in 0..4 {
            for e in std::mem::take(&mut c.inbox) {
                match &e.msg {
                    Message::Append(a) if e.to == addr => early = early.or(Some(a.clone())),
                    _ if e.to == addr => {}
                    _ => {
                        let graftyard_raft::Envelope { from, to, msg } = e;
                        c.on(to, |m, now| m.step(now, from, msg));
                    }
                }
            }
        }
        let early = early.expect("the tick's call to the learner");
        // Entries commit while the learner's call is out, so it was sent none of them: exactly one, then two more.
        let mut out = Vec::new();
        for (batch, n) in [(7u64, 1u128), (8, 2), (9, 3)] {
            for i in 0..n {
                // The third read's entries are 60 KB each, so one call carries one of them (APPEND_BYTES_MAX).
                let v = if batch == 9 { "x".repeat(60_000) } else { "v".into() };
                admit(&mut c, A, 9_980 + u128::from(batch) * 10 + i, put(&format!("s{batch}{i}"), &v));
                step(&mut c);
            }
            // A refusal of an older call comes back, as one that a later call overtook would: it empties the leader's
            // pipeline to the learner while the newest call is still out, and is not the current probe, so the
            // leader does not re-send for it.
            if batch == 8 {
                let piped = &early;
                let res = graftyard_raft::AppendRes {
                    term: piped.term,
                    boot: piped.boot,
                    round: 1,
                    sent_at: piped.sent_at,
                    prev_index: Index(1),
                    ok: false,
                    matched: Index(0),
                    last: Index(1),
                    promise: false,
                    refused: None,
                    stepped_down_at: None,
                    boots: [Timestamp(0); 2],
                    release: config(B).release,
                };
                c.on(A, |m, now| m.step(now, addr, Message::AppendRes(res)));
                c.inbox.retain(|e| e.to != addr);
            }
            let read_index = c.m(A).state().commit;
            c.on(A, |m, now| m.step(now, addr, Message::ReadIndex { batch }));
            let to_l: Vec<_> = std::mem::take(&mut c.inbox).into_iter().filter(|e| e.to == addr).collect();
            let res = to_l.iter().position(|e| matches!(e.msg, Message::ReadIndexRes { batch: b, .. } if b == batch));
            let res = res.expect("A answered the learner's batch");
            let Message::ReadIndexRes { index: Some((i, _)), .. } = &to_l[res].msg else { panic!("a granted read") };
            assert_eq!(*i, read_index, "the answer names the leader's commit");
            if batch == 9 {
                // Behind by three calls' bytes: three calls ahead of the answer, together through the read index.
                let calls: Vec<_> = to_l[..res]
                    .iter()
                    .filter_map(|e| match &e.msg {
                        Message::Append(a) if !a.entries.is_empty() => Some(a),
                        _ => None,
                    })
                    .collect();
                let through = calls.iter().filter_map(|a| a.entries.last()).map(|e| e.index).max();
                out.push(through.filter(|t| *t >= read_index).map(|_| calls.len() as u64));
                continue;
            }
            out.push(to_l[..res].iter().find_map(|e| match &e.msg {
                Message::Append(a)
                    if a.entries.last().is_some_and(|x| x.index >= read_index) && a.commit >= read_index =>
                {
                    Some(a.prev_index.0 + 1)
                }
                _ => None,
            }));
        }
        (early.prev_index.0 + 1, out, early.entries.last().map_or(early.prev_index.0, |e| e.index.0) + 1)
    };
    // With the tick's call in flight, the first append follows it; after the refusal emptied the pipeline, the second
    // goes from the entry the learner has not acknowledged, the tick's call's first. A lossy learner gets neither.
    let (first, out, after) = before_answer(0);
    assert_eq!(out[0], Some(after), "a learner on a clean link, with a call in flight");
    assert_eq!(out[1], Some(first), "a learner on a clean link, after a refusal");
    assert_eq!(out[2], Some(3), "a learner three calls' bytes behind gets three calls");
    assert_eq!(before_answer(usize::from(STEP_DOWN.commit_fail_min)).1, [None; 3], "a learner on a lossy link");
}

/// B0-02's r330-6 (`tests/m0/results/carried-8.md`): a far voter more than ten entries behind catches up in calls of
/// at most APPEND_BYTES_MAX past their first entry, each of which answers inside CALL_DEADLINE on the far link, so
/// no call misses and the leader's pipeline to it stays on. At 1.5 MB, its catch-up calls of 319 to 717 KB missed
/// CALL_DEADLINE one after another.
#[test]
fn a_far_voter_behind_catches_up_in_calls_that_answer_in_time() {
    use graftyard_raft::{APPEND_BYTES_MAX, encode_entry};
    let mut c = Cluster::new(47);
    c.elect(A);
    c.sync(A);
    // C misses 14 entries of about 55 KB, as r330-6's far voter missed 7 to 14 a call.
    let value = "v".repeat(55_000);
    let behind = c.m(C).log().len();
    for i in 0..14u128 {
        admit(&mut c, A, 9_900 + i, put(&format!("b{i}"), &value));
        c.deliver(|e| e.to != C && e.from != C);
    }
    c.inbox.clear();
    assert_eq!(c.m(C).log().len(), behind);
    let lag = c.m(A).log().len() - behind;
    assert!(lag >= 10, "C is {lag} entries behind");
    // From here, a call to C answers only if its entries past the first fit what the far link carries in
    // CALL_DEADLINE; any larger call misses, as r330-6's did.
    let (mut calls, mut misses, mut most) = (0, 0, 0);
    for _ in 0..40 {
        c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
        for _ in 0..8 {
            for e in std::mem::take(&mut c.inbox) {
                if let Message::Append(a) = &e.msg
                    && e.to == C
                    && !a.entries.is_empty()
                {
                    let past_first: usize = a.entries[1..].iter().map(|x| encode_entry(x).len()).sum();
                    calls += 1;
                    most = most.max(past_first);
                    if past_first > APPEND_BYTES_MAX {
                        misses += 1;
                        let f = e.failure(true).expect("an append");
                        c.on(A, |m, now| m.call_failed(now, f));
                        continue;
                    }
                }
                let graftyard_raft::Envelope { from, to, msg } = e;
                c.on(to, |m, now| m.step(now, from, msg));
            }
        }
        if c.m(C).log().len() == c.m(A).log().len() {
            break;
        }
    }
    assert_eq!(c.m(C).digests(), c.m(A).digests(), "C caught up");
    assert_eq!(
        misses, 0,
        "{misses} of {calls} calls to C missed; the largest carried {most} bytes past its first entry"
    );
    assert!(calls >= 2, "C caught up in {calls} calls of at most {most} bytes");
}

/// After a voter's call fails COMMIT_RETRIES + 1 times in a row, the leader waits for its tick to send again, and every
/// call it sends the voter starts from what the voter holds, never past the failed call: a pipelined call behind it
/// would leave a gap the voter must refuse.
#[test]
fn after_a_failed_call_the_next_call_starts_from_what_the_voter_holds() {
    let mut c = Cluster::new(45);
    c.elect(A);
    c.sync(A);
    let mut failures = 0;
    let mut to_c = Vec::new();
    for i in 0..8u128 {
        admit(&mut c, A, 9_600 + i, put(&format!("f{i}"), "v"));
        for _ in 0..6 {
            for e in std::mem::take(&mut c.inbox) {
                if e.to == C && matches!(&e.msg, Message::Append(a) if !a.entries.is_empty()) {
                    if failures <= graftyard_raft::COMMIT_RETRIES {
                        // The call fails with an error, reported at once.
                        failures += 1;
                        let f = e.failure(false).expect("an append");
                        c.on(A, |m, now| m.call_failed(now, f));
                        continue;
                    }
                    // Calls reach C in the order they left, so a call behind one C stored starts within C's log.
                    let Message::Append(a) = &e.msg else { unreachable!() };
                    to_c.push((a.prev_index, c.m(C).last_index()));
                }
                let graftyard_raft::Envelope { from, to, msg } = e;
                c.on(to, |m, now| m.step(now, from, msg));
            }
        }
        c.advance(200);
    }
    assert!(failures > graftyard_raft::COMMIT_RETRIES, "only {failures} calls to C failed, {} went", to_c.len());
    assert!(!to_c.is_empty(), "no call reached C after its failures");
    for (prev, held) in &to_c {
        assert!(prev <= held, "a call to C started at {prev:?}, past its log at {held:?}");
    }
    c.deliver_all();
    c.sync(A);
    assert_eq!(c.m(C).digests(), c.m(A).digests());
}

/// `Lead::done` keeps resolved tickets by count, by the bytes their commands carry that no event stores (DONE_BYTES),
/// and for OUTCOME_KEEP. An edge whose `outcome` comes after its ticket was pruned gets `NotLeader` naming the same
/// leader, asks `outcome_by_request`, and gets the committed outcome from the `request` row, `Acked` built inside
/// `outcome::replay`. Kept by count alone, 4,096 issue outcomes with their reply renders held about 60 MB of a
/// leader's heap (`tests/m0/results/memory-budgets.md`).
#[test]
fn a_late_outcome_after_its_ticket_was_pruned_is_answered_from_the_request_row() {
    let w = |k: &str, render: usize| KvCmd::Write {
        key: k.into(),
        expect: None,
        value: "v".into(),
        actor: None,
        push: false,
        pack: 0,
        render,
    };
    let rid = |i: u128| RequestId(graftyard_raft::Ulid(i));
    let late = |c: &Cluster, t: graftyard_raft::Ticket, i: u128, cmd: &KvCmd, index: Index| {
        match c.m(A).outcome(t) {
            Some(Err(CommitError::NotLeader(nl))) => assert_eq!(nl.leader, Some(A), "names the same leader"),
            other => panic!("a pruned ticket answers NotLeader, got {other:?}"),
        }
        match c.m(A).outcome_by_request(rid(i), cmd).unwrap() {
            ByRequest::Done(a) => assert_eq!((a.index, a.out), (index, true), "the committed outcome, from its row"),
            _ => panic!("expected Done from the request row"),
        }
    };
    let mut c = Cluster::new(91);
    c.elect(A);
    c.sync(A);

    // By age: one small command, answered at once, then asked again past OUTCOME_KEEP.
    let small = w("aged", 0);
    let t = admit(&mut c, A, 500, small.clone());
    c.deliver_all();
    let index = c.m(A).outcome(t).expect("resolved").expect("committed").index;
    let keep = graftyard_raft::OUTCOME_KEEP.as_millis() as u64;
    c.run(keep - 1_000, 500, |_| true);
    assert!(matches!(c.m(A).outcome(t), Some(Ok(_))), "kept inside OUTCOME_KEEP");
    c.run(2_000, 500, |_| true);
    assert_eq!(c.m(A).role(), Role::Leader);
    late(&c, t, 500, &small, index);

    // By bytes: 100 commands each carrying 100 KB no event stores, 10 MB against DONE_BYTES, resolved together.
    let big: Vec<_> = (0..100u128).map(|i| (600 + i, w(&format!("b{i}"), 100_000))).collect();
    let tickets: Vec<_> = big.iter().map(|(i, cmd)| admit(&mut c, A, *i, cmd.clone())).collect();
    c.deliver_all();
    c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
    c.deliver_all();
    let newest = c.m(A).outcome(*tickets.last().unwrap()).expect("resolved");
    assert!(newest.is_ok(), "the newest outcome is kept");
    let kept = tickets.iter().filter(|t| matches!(c.m(A).outcome(**t), Some(Ok(_)))).count();
    assert!(kept <= graftyard_raft::DONE_BYTES / 100_000, "{kept} outcomes of 100 KB kept, past DONE_BYTES");
    assert!(kept >= graftyard_raft::DONE_BYTES / 100_000 - 2, "only {kept} kept");
    let first = &big[0];
    let index = c.m(A).group_fold().request(rid(first.0)).expect("its row").index;
    late(&c, tickets[0], first.0, &first.1, index);
}

/// A host's calls to members that never answer, or answer late, as `workers/site`'s `call` treats them: each
/// AppendEntries is reported missed at CALL_DEADLINE, and its body stays alive until its answer comes or the host gives
/// up LATE_LIMIT (10 s) later and reports it ended. The bytes counted are the entries' canonical encoding, measured
/// here, not the leader's own count.
struct Silent {
    to: Vec<SiteId>,
    /// None: they never answer. Some(ms): each answers this long after it was sent, as an empty learner that stores
    /// any call chained to what it holds, and answers a call with no entries at once.
    late: Option<u64>,
    /// What each holds, when they answer.
    held: std::collections::BTreeMap<SiteId, Index>,
    /// How long after a call was sent the host stops awaiting it and reports it ended: CALL_DEADLINE plus LATE_LIMIT,
    /// or sooner for a call that fails late with an error, as one to a callee that reset does.
    ends: u64,
    /// (sent, leader, request, bytes, reported missed).
    calls: Vec<(u64, SiteId, graftyard_raft::Envelope<common::kv::KvEvent>, usize, bool)>,
    /// AppendEntries with entries sent to them.
    sent: usize,
    /// The most bytes alive at once: to one peer, to every learner, and in all.
    peer: usize,
    learners: usize,
    total: usize,
}

const LATE_LIMIT_MS: u64 = 10_000;

impl Silent {
    fn new(to: Vec<SiteId>, late: Option<u64>) -> Silent {
        let ends = graftyard_raft::CALL_DEADLINE.as_millis() as u64 + LATE_LIMIT_MS;
        Silent { to, late, held: Default::default(), ends, calls: Vec::new(), sent: 0, peer: 0, learners: 0, total: 0 }
    }

    /// The answer a late member gives `e`, storing its entries if they chain to what it holds.
    fn answer(&mut self, e: &graftyard_raft::Envelope<common::kv::KvEvent>) -> Option<graftyard_raft::AppendRes> {
        let Message::Append(a) = &e.msg else { return None };
        let held = self.held.entry(e.to).or_insert(Index(0));
        let ok = a.prev_index <= *held;
        if ok {
            *held = (*held).max(Index(a.prev_index.0 + a.entries.len() as u64));
        }
        Some(graftyard_raft::AppendRes {
            term: a.term,
            boot: a.boot,
            round: a.round,
            sent_at: a.sent_at,
            prev_index: a.prev_index,
            ok,
            matched: if ok { Index(a.prev_index.0 + a.entries.len() as u64) } else { Index(0) },
            last: *held,
            promise: false,
            refused: None,
            stepped_down_at: None,
            boots: [Timestamp(0); 2],
            release: config(B).release,
        })
    }

    /// Delivers everything queued: messages to the silent members become calls.
    fn pump(&mut self, c: &mut Cluster) {
        for _ in 0..1_000 {
            let batch = std::mem::take(&mut c.inbox);
            if batch.is_empty() {
                break;
            }
            for e in batch {
                if self.to.contains(&e.to) {
                    match &e.msg {
                        Message::Append(a) if a.entries.is_empty() && self.late.is_some() => {
                            let res = self.answer(&e).expect("an append");
                            let to = e.to;
                            c.on(e.from, |m, now| m.step(now, to, Message::AppendRes(res)));
                        }
                        Message::Append(a) => {
                            let bytes = a.entries.iter().map(|x| graftyard_raft::encode_entry(x).len()).sum();
                            self.sent += 1;
                            self.calls.push((c.now.0, e.from, e, bytes, false));
                        }
                        _ => {}
                    }
                } else if c.members.get(e.to.0 as usize).is_some_and(Option::is_some) {
                    let graftyard_raft::Envelope { from, to, msg } = e;
                    c.on(to, |m, now| m.step(now, from, msg));
                }
            }
        }
        self.measure();
    }

    fn measure(&mut self) {
        let learner = |s: SiteId| graftyard_types::learner_slot(s).is_some();
        let mut by: std::collections::BTreeMap<SiteId, usize> = Default::default();
        for (_, _, e, b, _) in &self.calls {
            *by.entry(e.to).or_default() += b;
        }
        self.peer = self.peer.max(by.values().copied().max().unwrap_or(0));
        self.learners = self.learners.max(by.iter().filter(|(s, _)| learner(**s)).map(|(_, b)| b).sum());
        self.total = self.total.max(by.values().sum());
    }

    /// Time passes by `ms`: late answers arrive, members tick, and the host reports missed deadlines and ended calls.
    fn advance(&mut self, c: &mut Cluster, ms: u64) {
        c.now = Timestamp(c.now.0 + ms);
        let deadline = graftyard_raft::CALL_DEADLINE.as_millis() as u64;
        let now = c.now.0;
        let mut keep = Vec::new();
        for mut call in std::mem::take(&mut self.calls) {
            let leader = call.1;
            if c.members[leader.0 as usize].is_none() {
                continue;
            }
            let Message::Append(a) = &call.2.msg else { unreachable!() };
            let (to, round) = (call.2.to, a.round);
            let answered = self.late.is_some_and(|l| now >= call.0 + l);
            if !call.4 && now >= call.0 + deadline {
                call.4 = true;
                let f = call.2.failure(true).expect("an append");
                c.on(leader, |m, t| m.call_failed(t, f));
            }
            if answered && now < call.0 + self.ends {
                let res = self.answer(&call.2).expect("an append");
                c.on(leader, |m, t| m.step(t, to, Message::AppendRes(res)));
                c.on(leader, |m, t| m.call_ended(t, to, round));
            } else if answered || now >= call.0 + self.ends {
                c.on(leader, |m, t| m.call_ended(t, to, round));
            } else {
                keep.push(call);
            }
        }
        self.calls = keep;
        for s in SITES {
            if c.members[s.0 as usize].is_some() {
                c.on(s, |m, now| m.tick(now));
            }
        }
        self.pump(c);
    }
}

fn big_put(k: &str, bytes: usize) -> KvCmd {
    put(k, &"v".repeat(bytes))
}

/// The outgoing byte budget (`PEER_OUT_MAX`): a follower that answers nothing while the leader cuts 1 MB entries
/// holds at most PEER_OUT_MAX of AppendEntries bodies alive in the leader's host, while the healthy follower commits
/// every entry. Before, the core resent each missed call at once and on every tick, and pipelined up to PIPELINE
/// more, while the host still awaited each for LATE_LIMIT: 10.0 MB alive to one silent voter here, and m3f's new
/// leader sent each voter 10.7 to 11.2 MB in a minute against an 8 MB log (`tests/m0/results/memory-budgets.md`).
#[test]
fn a_silent_follower_holds_at_most_peer_out_max_of_the_leaders_calls() {
    // `ends`: when the host stops awaiting each call to C. Returns the most bytes alive to C, and how many 500 ms
    // steps after C answers again it has caught up.
    let run = |ends: Option<u64>| {
        let mut c = Cluster::new(93);
        c.elect(A);
        c.sync(A);
        let mut h = Silent::new(vec![C], None);
        h.ends = ends.unwrap_or(h.ends);
        for i in 0..40u128 {
            admit(&mut c, A, 7_000 + i, big_put(&format!("big{i}"), 1_000_000));
            h.pump(&mut c);
            h.advance(&mut c, 500);
        }
        for _ in 0..30 {
            h.advance(&mut c, 500);
        }
        // Outcomes are pruned after OUTCOME_KEEP, so read the commits from the leader's applied fold.
        let committed = (0..40).filter(|i| c.value(A, &format!("big{i}")).is_some()).count();
        assert_eq!(committed, 40, "B commits every entry with C silent");
        // C answers again; its lost calls end as the host gives up on them, the budget frees, and C catches up.
        h.to.clear();
        let steps = (1..=40).find(|_| {
            h.advance(&mut c, 500);
            c.m(C).last_index() == c.m(A).last_index()
        });
        (h.peer, h.sent, steps.expect("C caught up"))
    };
    let (peer, never, _) = run(None);
    assert!(peer <= graftyard_raft::PEER_OUT_MAX, "{peer} bytes alive to the silent follower");
    assert!(peer >= 1_000_000, "C was sent its calls: {peer}");
    // Calls that fail late with an error, 2 s after they were sent: the host's `call_ended` frees the budget then, not
    // at OUT_KEEP, so the leader retries C about every 2 s rather than every OUT_KEEP, and C catches up within a few
    // steps of answering again.
    let (peer, retried, steps) = run(Some(2_000));
    assert!(retried >= 3 * never, "{retried} calls to C with late errors against {never} with none ended");
    assert!(peer <= graftyard_raft::PEER_OUT_MAX, "{peer} bytes alive with late errors");
    assert!(steps <= 6, "C caught up {steps} steps after it answered again");
}

/// With one follower silent, small commands commit as fast as with both followers answering: each in the step it is
/// admitted, at the same final commit index. A voter with no call alive always gets one, so the budget never holds
/// back the healthy follower.
#[test]
fn a_silent_follower_costs_the_commit_rate_nothing() {
    let run = |silent: Vec<SiteId>| {
        let mut c = Cluster::new(94);
        c.elect(A);
        c.sync(A);
        let mut h = Silent::new(silent, None);
        let mut late = 0;
        for i in 0..400u128 {
            let t = admit(&mut c, A, 8_000 + i, put(&format!("k{i}"), &"x".repeat(2_000)));
            h.pump(&mut c);
            if !matches!(c.m(A).outcome(t), Some(Ok(_))) {
                late += 1;
            }
            h.advance(&mut c, 50);
        }
        (late, c.m(A).state().commit)
    };
    let healthy = run(Vec::new());
    assert_eq!(healthy.0, 0, "every command commits in its own step with both followers");
    assert_eq!(run(vec![C]), healthy, "the same with C silent");
}

/// A placement of the three voters and `n` learners, all on the sim's three sites.
fn with_learners(c: &mut Cluster, l: SiteId, n: u8) -> Vec<SiteId> {
    use graftyard_raft::GroupConfig;
    use graftyard_types::{Member as MemberRef, MemberKind, Placement, PlacementChecks, Region, Rtt, Site, SiteTable};
    let Some((_, GroupConfig::Normal(p))) = c.m(l).committed_config().cloned() else { panic!("normal config") };
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
    let learners: Vec<MemberRef> = (0..n)
        .map(|k| MemberRef::new(CHAIN, SITES[usize::from(k % 3)], MemberKind::Learner, k, p.next_generation()))
        .collect();
    let addrs = learners.iter().map(|m| m.addr()).collect();
    let np = Placement::new(p.voters().iter().cloned().collect(), learners.into_iter().collect(), p.home(), &checks)
        .unwrap();
    c.on(l, |m, now| m.reconfigure(now, GroupConfig::Normal(Box::new(np.clone())))).unwrap();
    addrs
}

/// `learner-cadence`: a learner's reads that arrive within `LEARNER_READ_EVERY` (200 ms) of its last batch leave
/// together as one batch at the floor, not one call each, and the floor moves only latency: a read that arrived after
/// a write was acknowledged is granted at or above that write, never by the answer to a batch sent before it arrived.
/// A real learner member follows A here; the voters run in the cluster.
#[test]
fn a_learners_reads_within_the_floor_leave_as_one_batch_and_stay_linearizable() {
    use graftyard_raft::{Envelope, LEARNER_READ_EVERY, ReadId, Ready, Stored};
    use graftyard_types::learner_addr;
    type Env = Envelope<common::kv::KvEvent>;
    /// The learner, outside the cluster: its read-index calls are kept in `asks` as they leave.
    struct L {
        m: Member<KvCmd>,
        disk: Stored<common::kv::Kv>,
        granted: Vec<(ReadId, Index)>,
        asks: Vec<Env>,
    }
    impl L {
        fn on<R>(&mut self, c: &mut Cluster, f: impl FnOnce(&mut Member<KvCmd>, Timestamp) -> Ready<KvCmd, R>) -> R {
            let (r, out) = f(&mut self.m, c.now).persist(&mut self.disk);
            for e in out.early.into_iter().chain(out.messages) {
                if matches!(e.msg, Message::ReadIndex { .. }) { self.asks.push(e) } else { c.inbox.push(e) }
            }
            for r in out.resolutions {
                if let Resolution::Read { id, result: Ok(g) } = r {
                    self.granted.push((id, g.floor()));
                }
            }
            r
        }
        /// Delivers everything queued; to the learner only when `hear`.
        fn pump(&mut self, c: &mut Cluster, hear: bool) {
            let me = self.m.me();
            for _ in 0..200 {
                let batch = std::mem::take(&mut c.inbox);
                if batch.is_empty() {
                    return;
                }
                for Envelope { from, to, msg } in batch {
                    if to != me {
                        c.on(to, |m, t| m.step(t, from, msg));
                    } else if hear {
                        self.on(c, |m, t| m.step(t, from, msg));
                    }
                }
            }
        }
        fn tick(&mut self, c: &mut Cluster, ms: u64, hear: bool) {
            c.advance(ms);
            self.on(c, |m, t| m.tick(t));
            self.pump(c, hear);
        }
        fn floor(&self, r: ReadId) -> Option<Index> {
            self.granted.iter().find(|(id, _)| *id == r).map(|(_, f)| *f)
        }
    }
    let floor = LEARNER_READ_EVERY.as_millis() as u64;
    let mut c = Cluster::new(96);
    c.elect(A);
    c.sync(A);
    let addr = learner_addr(0);
    assert_eq!(with_learners(&mut c, A, 1), vec![addr]);
    let mut l = L {
        m: Member::new(config(addr), 96_000).unwrap(),
        disk: Stored::new(common::kv::Kv::default()),
        granted: Vec::new(),
        asks: Vec::new(),
    };
    for _ in 0..20 {
        l.tick(&mut c, 100, true);
    }
    assert_eq!(l.m.leader(), Some(A), "the learner follows A");
    assert!(l.m.group_fold().applied() >= c.m(A).state().commit, "the learner caught up");
    l.asks.clear();
    // The first read leaves at once, in its own batch.
    let r1 = l.on(&mut c, |m, t| m.read_index(t)).expect("A leads");
    let first = std::mem::take(&mut l.asks);
    assert_eq!(first.len(), 1, "a read after an idle floor leaves at once");
    // A answers it before the write below; the answer is held back.
    for Envelope { from, to, msg } in first {
        c.on(to, |m, t| m.step(t, from, msg));
    }
    let answers: Vec<_> = c.inbox.iter().filter(|e| matches!(e.msg, Message::ReadIndexRes { .. })).cloned().collect();
    c.inbox.retain(|e| e.to != addr);
    assert_eq!(answers.len(), 1, "A answered at once, inside its lease");
    // A write commits through A and B while the learner hears nothing.
    admit(&mut c, A, 9_600, put("k", "new"));
    l.pump(&mut c, false);
    let acked = c.m(A).state().commit;
    assert!(l.m.group_fold().applied() < acked, "the learner has not applied the write");
    // Two more reads 50 and 150 ms after the first: both wait for the floor.
    let mut later = Vec::new();
    for step in [50, 100] {
        l.tick(&mut c, step, false);
        later.push(l.on(&mut c, |m, t| m.read_index(t)).expect("A leads"));
        assert!(l.asks.is_empty(), "a read {step} ms into the floor waits");
    }
    l.tick(&mut c, floor - 151, false);
    assert!(l.asks.is_empty(), "nothing leaves before the floor: {:?}", l.asks);
    // The first answer, from before the write, arrives: it grants the first read only.
    for Envelope { from, msg, .. } in answers {
        l.on(&mut c, |m, t| m.step(t, from, msg));
    }
    assert!(l.floor(r1).is_some(), "the first answer grants the first read");
    assert!(
        later.iter().all(|r| l.floor(*r).is_none()),
        "a read that arrived after the write was granted by an answer to a batch sent before it: {:?}",
        l.granted
    );
    // At the floor the two later reads leave as one batch, and its answer grants both at or above the write.
    l.tick(&mut c, 1, false);
    let more = std::mem::take(&mut l.asks);
    assert_eq!(more.len(), 1, "two reads within the floor leave as one batch: {more:?}");
    for Envelope { from, to, msg } in more {
        c.on(to, |m, t| m.step(t, from, msg));
    }
    l.pump(&mut c, true);
    // A's appends the learner missed above are resent once their calls pass CALL_DEADLINE.
    for _ in 0..25 {
        l.tick(&mut c, 100, true);
    }
    for r in &later {
        let f = l.floor(*r);
        assert!(f.is_some_and(|f| f >= acked), "read {r:?} granted at {f:?}, below the write at {acked:?}");
    }
}

/// The outgoing byte budget across peers (`LEARNERS_OUT_MAX`, `OUT_MAX`): a new leader that must catch up eight
/// learners, none of which answers, keeps at most LEARNERS_OUT_MAX alive to them and OUT_MAX in all, and still
/// commits through the healthy voter. Before, each learner got its own catch-up calls and their resends, and B0-33's
/// new leaders reset holding 58.3 MB of call bodies and 42.9 MB of messages for learners
/// (`tests/m0/results/memory-budgets.md`).
#[test]
fn a_new_leader_catching_up_silent_learners_stays_within_out_max() {
    let mut c = Cluster::new(95);
    c.elect(A);
    c.sync(A);
    let learners = with_learners(&mut c, A, 8);
    // The learners answer nothing while A leads, so the log grows past them.
    let mut h = Silent::new(learners.clone(), None);
    h.pump(&mut c);
    h.advance(&mut c, 1_100);
    for i in 0..12u128 {
        admit(&mut c, A, 9_000 + i, big_put(&format!("l{i}"), 1_000_000));
        h.pump(&mut c);
        h.advance(&mut c, 300);
    }
    // A new leader: A goes, and B wins once the promises lapse. Every learner is empty and answers each call 3 s
    // after it was sent: past CALL_DEADLINE, so the leader resends while the first copy is still alive.
    c.crash(A);
    let mut h = Silent::new(learners, Some(3_000));
    for _ in 0..(graftyard_raft::ELECTION_AFTER.as_millis() as u64 / 500 + 1) {
        h.advance(&mut c, 500);
    }
    c.on(B, |m, now| m.campaign(now));
    h.pump(&mut c);
    for _ in 0..4 {
        h.advance(&mut c, 200);
    }
    assert_eq!(c.m(B).role(), Role::Leader, "B won");
    let t = admit(&mut c, B, 9_500, put("after", "v"));
    for _ in 0..60 {
        h.advance(&mut c, 500);
    }
    let _ = t;
    assert_eq!(c.value(B, "after").as_deref(), Some("v"), "B commits through C");
    assert!(h.learners <= graftyard_raft::LEARNERS_OUT_MAX, "{} bytes alive to learners", h.learners);
    assert!(h.total <= graftyard_raft::OUT_MAX, "{} bytes alive in all", h.total);
    assert!(h.peer <= graftyard_raft::PEER_OUT_MAX, "{} bytes alive to one learner", h.peer);
    assert!(h.learners >= 2_000_000, "the learners were sent catch-up calls: {}", h.learners);
}

/// A leadership that ends on a higher term answers each ticket it held under the ticket it issued. Answered under the
/// new term, a host keyed by the issued ticket matched none: B0-33's deposed leaders kept 581 to 1,172 open tickets,
/// each with its command and waiters, for minutes (LM-1 round 19, `tests/m0/results/admitted-bytes.md`).
#[test]
fn a_leadership_deposed_by_a_higher_term_answers_its_tickets_as_issued() {
    let mut c = Cluster::new(92);
    c.elect(A);
    c.sync(A);
    let t = admit(&mut c, A, 700, put("k", "v"));
    c.inbox.clear();
    // B and C elect B without A; B's first append then deposes A.
    c.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1);
    c.inbox.clear();
    c.on(B, |m, now| m.campaign(now));
    c.deliver(|e| e.to != A);
    assert_eq!(c.m(B).role(), Role::Leader);
    for _ in 0..20 {
        c.advance(200);
        c.deliver_all();
    }
    assert!(c.m(A).term() > t.term, "A was deposed by a higher term");
    let r = c.ticket_results(t);
    assert!(
        matches!(r.as_slice(), [Resolution::Ticket { result: Err(CommitError::NotLeader(_)), .. }]),
        "the issued ticket resolves NotLeader once: {r:?}"
    );
}

/// A leader whose term's `Noop` reached no quorum gives up and keeps `led_term`; resuming that term in the same boot
/// issues new seqs, so an old ticket never answers with a newer command's outcome (B0-11's class, within one boot).
#[test]
fn a_term_resumed_in_the_same_boot_never_reissues_a_ticket() {
    let mut c = Cluster::new(93);
    c.elect(A);
    c.sync(A);
    c.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1);
    c.deliver(|_| false);
    // B wins, but no AppendEntries reaches anyone: its `Noop` reaches no quorum.
    c.on(B, |m, now| m.campaign(now));
    c.deliver(|e| !matches!(e.msg, Message::Append(_)));
    assert_eq!(c.m(B).role(), Role::Leader);
    let first = admit(&mut c, B, 701, put("k", "first"));
    for _ in 0..40 {
        c.advance(100);
        c.deliver(|e| !matches!(e.msg, Message::Append(_)));
    }
    assert_eq!(c.m(B).role(), Role::Follower);
    assert_eq!(c.m(B).state().led_term, Some(first.term), "kept led_term to resume");
    let second = admit(&mut c, B, 702, put("k", "second"));
    assert_eq!((second.term, second.boot), (first.term, first.boot), "resumed in the same term and boot");
    assert_ne!(second, first, "a resumed leadership issued a ticket its last one issued");
}

/// A second learner generation on the same slots catches up. `/learners` places each new generation at the same
/// addresses (`learner_addr(slot)`) under new names; the leader reset its per-peer state only for voters, so each new,
/// empty learner kept the old one's `matched`, was probed at that index forever, refused every probe, and applied
/// nothing (B0-33, LM-1 round 23: 1,776 calls and 144.9 MB to eight learners, all refused, for the whole row). The
/// same holds for a learner removed and placed again under a new name.
#[test]
fn a_second_learner_generation_on_the_same_slots_catches_up() {
    let mut c = Cluster::new(97);
    c.elect(A);
    c.sync(A);
    let learners = with_learners(&mut c, A, 2);
    let mut h = Silent::new(learners.clone(), Some(10));
    let fill = |c: &mut Cluster, h: &mut Silent, from: u128| {
        for i in 0..5u128 {
            admit(c, A, from + i, put(&format!("k{}", from + i), "v"));
            h.pump(c);
            h.advance(c, 200);
        }
        for _ in 0..20 {
            h.advance(c, 200);
        }
    };
    fill(&mut c, &mut h, 9_000);
    let first = c.m(A).state().commit;
    assert!(learners.iter().all(|l| h.held[l] >= first), "the first generation caught up: {:?}", h.held);
    // The next generation, on the same sites and slots, with new names: new members that hold nothing. Calls to the
    // old generation are still out, and the old members answer them after the change, with everything they hold.
    admit(&mut c, A, 9_050, put("k9050", "v"));
    h.pump(&mut c);
    let out = std::mem::take(&mut h.calls);
    assert!(!out.is_empty(), "calls to the old generation are out");
    assert_eq!(with_learners(&mut c, A, 2), learners);
    let mut old = Silent::new(learners.clone(), Some(10));
    old.held = std::mem::take(&mut h.held);
    for (_, leader, e, _, _) in out {
        let res = old.answer(&e).expect("an append");
        let (to, round) = (e.to, res.round);
        c.on(leader, |m, t| m.step(t, to, Message::AppendRes(res)));
        c.on(leader, |m, t| m.call_ended(t, to, round));
    }
    fill(&mut c, &mut h, 9_100);
    let last = c.m(A).state().commit;
    assert!(last > first);
    assert!(
        learners.iter().all(|l| h.held.get(l) == Some(&last)),
        "the second generation caught up: {:?} of {last:?}",
        h.held
    );
    // Removed, then placed again under a third generation's names.
    assert!(with_learners(&mut c, A, 0).is_empty());
    for _ in 0..5 {
        h.advance(&mut c, 200);
    }
    assert_eq!(with_learners(&mut c, A, 2), learners);
    h.held.clear();
    fill(&mut c, &mut h, 9_200);
    let last = c.m(A).state().commit;
    assert!(
        learners.iter().all(|l| h.held.get(l) == Some(&last)),
        "the third generation caught up: {:?} of {last:?}",
        h.held
    );
}

/// A commit's proof names the voters that made it durable and no learner, even when learners hold the entry: the edge
/// shows `QuorumProof::sites` to callers as the write's quorum (`graftyard_git::group`, `outcome_wire`), and every
/// address with a matched index used to be listed, so a caught-up learner showed as a site the write was durable on.
#[test]
fn a_commit_with_learners_caught_up_names_only_voters_in_its_proof() {
    let mut c = Cluster::new(98);
    c.elect(A);
    c.sync(A);
    let learners = with_learners(&mut c, A, 2);
    let mut h = Silent::new(learners.clone(), Some(10));
    for i in 0..3u128 {
        admit(&mut c, A, 9_300 + i, put(&format!("w{i}"), "v"));
        h.pump(&mut c);
        h.advance(&mut c, 200);
    }
    for _ in 0..10 {
        h.advance(&mut c, 200);
    }
    // The learners answer first, so they hold the entry by the time the voters' answers commit it.
    let t = admit(&mut c, A, 9_400, put("p", "v"));
    let mut voters = Vec::new();
    for _ in 0..5 {
        for e in std::mem::take(&mut c.inbox) {
            if graftyard_types::learner_slot(e.to).is_some() { c.inbox.push(e) } else { voters.push(e) }
        }
        h.pump(&mut c);
        h.advance(&mut c, 20);
    }
    let entry = c.m(A).log().last().expect("an entry").index;
    assert!(c.m(A).state().commit < entry, "not committed before the voters answer");
    assert!(learners.iter().all(|l| h.held[l] >= entry), "the learners hold the entry: {:?} of {entry:?}", h.held);
    for graftyard_raft::Envelope { from, to, msg } in voters {
        c.on(to, |m, now| m.step(now, from, msg));
    }
    for _ in 0..5 {
        c.deliver_all();
        h.advance(&mut c, 50);
    }
    let r = c.ticket_results(t);
    let [Resolution::Ticket { result: Ok(a), .. }] = r.as_slice() else { panic!("committed: {r:?}") };
    let named: Vec<SiteId> = a.quorum().acks().iter().map(|d| d.site).collect();
    assert_eq!(a.quorum().sites(), named.as_slice());
    assert!(named.iter().all(|s| SITES.contains(s)), "the proof names only voters: {named:?}");
    assert!(named.len() >= 2 && named.contains(&A), "a voter quorum: {named:?}");
}

/// L0-12 m3fr6 (`tests/m0/results/read-round-gap.md`): reads that need a round send at most one round per voter per
/// READ_ROUND_GAP while it is unanswered, and none to a learner, whose answer counts toward no quorum, however the clock
/// moves within a turn. A lease-clock floor raised by each newer peer stamp gave every read in a turn its own reading,
/// and with a round per reading the leader sent up to 99 rounds to each voter in one millisecond. Each read is still
/// granted only by a round that left after it arrived.
#[test]
fn reads_send_at_most_one_round_per_gap_while_the_clock_climbs_and_no_learner_gets_it() {
    let mut c = Cluster::new(43);
    c.elect(A);
    c.sync(A);
    let learners = with_learners(&mut c, A, 8);
    let voters = |e: &graftyard_raft::Envelope<common::kv::KvEvent>| e.to.0 < 192;
    c.deliver(voters);
    c.inbox.clear();
    // Every read needs a round, as on a suspect clock.
    c.m_mut(A).set_clock_suspect(true);
    c.advance(1);
    c.inbox.clear();
    c.resolutions.clear();
    // DESIGN's cap: at most one unanswered read round per 20 ms, 50 a second per voter.
    let cap: u64 = 50;
    let granted = |c: &Cluster, id| {
        c.resolutions
            .iter()
            .any(|(_, r)| matches!(r, Resolution::Read { id: i, result: Ok(g) } if *i == id && g.via_round()))
    };
    // One second of a busy turn's shape: 200 reads, the floored clock one ms higher at each, while no round is
    // answered. Each read arrives at a new clock reading.
    let mut held = Vec::new();
    let mut asked = Vec::new();
    let mut to_learners = 0;
    for _ in 0..1_000u64 {
        c.now = Timestamp(c.now.0 + 1);
        if asked.len() < 200 {
            asked.push((c.now, c.on(A, |m, now| m.read(now)).expect("leading")));
            // Only reads step here, so an AppendEntries to a learner now is a read round, not a heartbeat.
            to_learners += c.inbox.iter().filter(|e| !voters(e) && matches!(e.msg, Message::Append(_))).count();
        } else {
            c.on(A, |m, now| m.tick(now));
        }
        held.extend(std::mem::take(&mut c.inbox).into_iter().filter(voters));
    }
    let per_voter =
        |to: SiteId| held.iter().filter(|e| e.from == A && e.to == to && matches!(e.msg, Message::Append(_))).count();
    for s in [B, C] {
        assert!(
            per_voter(s) as u64 <= cap + 2,
            "{} rounds to {s:?} in 1 s for 200 reads on a climbing clock, against a cap of {cap}",
            per_voter(s)
        );
    }
    assert_eq!(to_learners, 0, "{to_learners} read rounds went to the {} learners", learners.len());
    // Answered in order, every read is granted, each by a round that left at or after it arrived.
    c.inbox = held;
    c.deliver(voters);
    for (at, id) in &asked {
        assert!(granted(&c, *id), "the read asked at {at:?} was never granted");
    }
    // A read 1 ms after an answered round, well inside the gap, sends its own round at once, and only that round
    // grants it.
    c.advance(1);
    c.inbox.retain(voters);
    c.deliver(voters);
    let first = c.on(A, |m, now| m.read(now)).expect("leading");
    c.deliver(voters);
    assert!(granted(&c, first), "a read on a free slot was never granted");
    c.advance(1);
    c.inbox.clear();
    c.resolutions.clear();
    let late = c.on(A, |m, now| m.read(now)).expect("leading");
    assert!(!granted(&c, late), "a read was granted by a round that left before it arrived");
    for s in [B, C] {
        let sent = c.inbox.iter().filter(|e| e.to == s && matches!(e.msg, Message::Append(_))).count();
        assert_eq!(sent, 1, "a read after an answered round sent {sent} rounds to {s:?} instead of one at once");
    }
    c.deliver(voters);
    assert!(granted(&c, late), "the read's own round did not grant it");
}

/// A leader on a suspect clock sends at most one read round per voter in an I/O turn, however far the floored member
/// clock jumps within it. Every input of a turn shares one own-clock reading, and the member clock jumps 25 ms on a
/// newer peer stamp at every fifth input while answers come back, late in the turn some to its own round. Timed on the member
/// clock, the gap and those answers freed the slot again and again: up to 8 rounds to one voter in one turn, 2,264 of
/// 2,402 repeating a last index the turn already sent (m39's leader: 1,146 of 1,245, `tests/m0/results/fanout-dups.md`).
/// Every read is still granted only by a round that left after it arrived.
#[test]
fn reads_send_at_most_one_round_per_voter_per_turn_while_the_floor_jumps() {
    let mut c = Cluster::new(44);
    c.elect(A);
    c.sync(A);
    c.inbox.clear();
    c.m_mut(A).set_clock_suspect(true);
    c.advance(1);
    c.inbox.clear();
    c.resolutions.clear();
    let mut held = Vec::new();
    let mut asked = Vec::new();
    let mut worst = 0;
    for turn in 0..40u64 {
        // The isolate's clock reads one value for the whole turn, 1 ms on from the last turn's floored clock.
        let own = Timestamp(c.now.0 + 1);
        c.now = own;
        let mut older = std::mem::take(&mut held);
        let mut sent = std::collections::BTreeMap::<SiteId, usize>::new();
        for i in 0..30u64 {
            if i % 5 == 4 {
                c.now = Timestamp(c.now.0 + 25);
            }
            if i % 5 == 2 {
                // Two answers to rounds from earlier turns, and from the 15th input on the answers to this turn's own
                // calls: a frozen reading can span seconds of queued events. The follower steps each, its reply
                // reaches A.
                let mut back: Vec<_> = older.drain(..older.len().min(2)).collect();
                if i > 15 {
                    back.append(&mut held);
                }
                for e in back {
                    let graftyard_raft::Envelope { from, to, msg } = e;
                    c.on(to, |m, now| m.step(now, from, msg));
                }
                for e in std::mem::take(&mut c.inbox) {
                    let graftyard_raft::Envelope { from, to, msg } = e;
                    c.on(to, |m, now| {
                        m.set_own_clock(own);
                        m.step(now, from, msg)
                    });
                }
            } else {
                let id = c.on(A, |m, now| {
                    m.set_own_clock(own);
                    m.read(now)
                });
                asked.push((c.now, id.expect("leading")));
            }
            for e in std::mem::take(&mut c.inbox) {
                if matches!(e.msg, Message::Append(_)) {
                    *sent.entry(e.to).or_default() += 1;
                }
                held.push(e);
            }
        }
        worst = worst.max(sent.values().copied().max().unwrap_or(0));
        assert!(sent.values().all(|n| *n <= 1), "turn {turn}: {sent:?} read rounds per voter in one turn, against one");
        held.extend(older);
    }
    assert_eq!(worst, 1, "no read round left at all");
    // Answered in order, every read is granted, each by a round that left at or after it arrived.
    // A later turn, at a new own-clock reading, sends the round the last turn's reads still need.
    c.inbox = held;
    c.advance(1);
    let now = c.now;
    c.m_mut(A).set_own_clock(now);
    c.deliver_all();
    let granted = |id| {
        c.resolutions
            .iter()
            .any(|(_, r)| matches!(r, Resolution::Read { id: i, result: Ok(g) } if *i == id && g.via_round()))
    };
    for (at, id) in &asked {
        assert!(granted(*id), "the read asked at {at:?} was never granted");
    }
}

/// read-beat (`tests/m0/results/read-beat.md`): a read round to a voter whose AppendEntries for the entry in flight is
/// still out carries no entries and leaves that call alone. B0-29 mwkyk9zg's leader, on a suspect clock, sent a round
/// every 20 to 30 ms while each entry took about 200 ms to commit, and each forced round re-sent the entry in flight:
/// 6.4 to 7.8 sends of every entry to each voter and 640 to 940 MB a run, against one send on m27. Each read is still
/// granted only by a round that left after it arrived, and an ack of an empty round grants it.
#[test]
fn a_read_round_behind_an_entry_in_flight_resends_no_entries() {
    let mut c = Cluster::new(45);
    c.elect(A);
    c.sync(A);
    c.inbox.clear();
    c.m_mut(A).set_clock_suspect(true);
    // One entry in flight, its AppendEntries to both voters held unanswered.
    let t =
        c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(7)), put("k", "v"))).expect("admitted").ticket;
    let carries = |e: &graftyard_raft::Envelope<common::kv::KvEvent>| matches!(&e.msg, Message::Append(a) if !a.entries.is_empty() && e.from == A && e.to != A);
    let first: Vec<_> = std::mem::take(&mut c.inbox);
    assert_eq!(first.iter().filter(|e| carries(e)).count(), 2, "the entry did not leave to both voters");
    // Ten reads, each past READ_ROUND_GAP, each needing its own round.
    let mut rounds = Vec::new();
    let mut asked = Vec::new();
    for _ in 0..10 {
        c.now = Timestamp(c.now.0 + 25);
        asked.push(c.on(A, |m, now| m.read(now)).expect("leading"));
        rounds.extend(std::mem::take(&mut c.inbox));
    }
    let to_voters = rounds.iter().filter(|e| matches!(e.msg, Message::Append(_)) && [B, C].contains(&e.to)).count();
    assert!(to_voters >= 18, "only {to_voters} read rounds left for 10 reads");
    let resent = rounds.iter().filter(|e| carries(e)).count();
    assert_eq!(resent, 0, "{resent} of {to_voters} read rounds re-sent the entry in flight");
    // The empty rounds alone, answered: every read is granted by its round, and the entry is still in flight.
    c.inbox = rounds;
    c.deliver(|e| !carries(e));
    let granted = |c: &Cluster, id| {
        c.resolutions
            .iter()
            .any(|(_, r)| matches!(r, Resolution::Read { id: i, result: Ok(g) } if *i == id && g.via_round()))
    };
    for id in &asked {
        assert!(granted(&c, *id), "a read was not granted by the ack of an empty round");
    }
    assert!(c.inbox.iter().all(|e| !carries(e)), "an empty round's answer started a re-send of the entry");
    // The held entry's calls then land and commit it on every voter.
    assert!(c.ticket_results(t).is_empty(), "the entry committed before its calls were answered");
    c.inbox.extend(first);
    c.deliver_all();
    let r = c.ticket_results(t);
    assert!(
        matches!(r.as_slice(), [Resolution::Ticket { result: Ok(_), .. }]),
        "the entry in flight never committed: {r:?}"
    );
    assert_eq!(c.m(B).last_index(), c.m(A).last_index());
    assert_eq!(c.m(C).last_index(), c.m(A).last_index());
}

/// read-beat's other half: a voter whose calls all failed has no pipeline, and a read round to it still carries the
/// entries it lacks, as before, rather than an empty round it would ack without them.
#[test]
fn a_read_round_to_a_voter_with_no_calls_out_still_carries_the_entry() {
    let mut c = Cluster::new(46);
    c.elect(A);
    c.sync(A);
    c.inbox.clear();
    c.m_mut(A).set_clock_suspect(true);
    c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(9)), put("k", "v"))).expect("admitted");
    let to_c = |e: &graftyard_raft::Envelope<common::kv::KvEvent>| e.from == A && e.to == C;
    let carries = |e: &graftyard_raft::Envelope<common::kv::KvEvent>| matches!(&e.msg, Message::Append(a) if !a.entries.is_empty());
    // C's call and both retries fail with an error: the pipeline is empty and the next resend waits for the tick.
    for _ in 0..=graftyard_raft::COMMIT_RETRIES {
        let calls: Vec<_> = c.inbox.drain(..).filter(|e| to_c(e) && carries(e)).collect();
        assert_eq!(calls.len(), 1, "one call to C carried the entry");
        let f = calls[0].failure(false).expect("an append");
        c.on(A, |m, now| m.call_failed(now, f));
    }
    assert!(!c.inbox.iter().any(|e| to_c(e) && carries(e)), "a retry past COMMIT_RETRIES left at once");
    c.now = Timestamp(c.now.0 + 25);
    c.on(A, |m, now| m.read(now)).expect("leading");
    let round: Vec<_> = c.inbox.iter().filter(|e| to_c(e) && matches!(e.msg, Message::Append(_))).collect();
    assert_eq!(round.len(), 1, "no read round to C");
    assert!(carries(round[0]), "the read round to a voter with nothing out carried no entries");
}

/// read-beat's resend: a voter's call that has gone unanswered READ_RESEND or longer, as one whose answer was lost, is
/// re-sent by the next read round, not left to CALL_DEADLINE's tick. Without it the live sweep's seed 13441 lost a
/// near follower's calls and stalled 31 s, past the 30 s bound, where main's re-sending rounds stalled 5 s.
#[test]
fn a_read_round_resends_a_call_outstanding_past_read_resend() {
    let mut c = Cluster::new(47);
    c.elect(A);
    c.sync(A);
    c.inbox.clear();
    c.m_mut(A).set_clock_suspect(true);
    c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(11)), put("k", "v"))).expect("admitted");
    let carries = |e: &graftyard_raft::Envelope<common::kv::KvEvent>| {
        e.from == A && [B, C].contains(&e.to) && matches!(&e.msg, Message::Append(a) if !a.entries.is_empty())
    };
    // The entry's calls are lost.
    assert_eq!(c.inbox.drain(..).filter(|e| carries(e)).count(), 2);
    let gap = graftyard_raft::READ_RESEND.as_millis() as u64;
    // A read inside READ_RESEND sends empty rounds.
    c.now = Timestamp(c.now.0 + gap - 5);
    c.on(A, |m, now| m.read(now)).expect("leading");
    assert_eq!(c.inbox.iter().filter(|e| carries(e)).count(), 0, "a round inside READ_RESEND re-sent the entry");
    c.inbox.clear();
    // The next round, past READ_RESEND and READ_ROUND_GAP, re-sends it to both voters, still before CALL_DEADLINE.
    c.now = Timestamp(c.now.0 + 25);
    assert!(gap < graftyard_raft::CALL_DEADLINE.as_millis() as u64);
    c.on(A, |m, now| m.read(now)).expect("leading");
    let resent = c.inbox.iter().filter(|e| carries(e)).count();
    assert_eq!(resent, 2, "a round past READ_RESEND re-sent the entry to {resent} of 2 voters");
}

/// write-cuts cut (2): a stall under EXTEND_MIN stamps no `extend` on the next entry, that entry's commit clears every
/// member's marker, and a later entry after idle time carries none either, so neither the stall nor the idle time
/// moves a lease. A stall of exactly EXTEND_MIN still stamps one, of its length.
#[test]
fn a_stall_under_extend_min_stamps_no_extend_and_clears_its_marker() {
    let floor = graftyard_raft::EXTEND_MIN.as_millis() as u64;
    let stalled = |c: &mut Cluster, id: u128, ms: u64| {
        c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(id)), put("k", &id.to_string())))
            .expect("leader admits");
        let at = c.m(A).log().last().unwrap().at;
        // The first AppendEntries are lost; the leader's tick `ms` later marks the stall at the entry's `at`, and its
        // re-send gets through.
        c.inbox.retain(|e| !matches!(e.msg, Message::Append(_)));
        c.advance(ms);
        assert_eq!(c.m(A).state().stalled_since, Some(at), "the leader marked its unacknowledged entry");
        c.deliver_all();
        c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(id + 1)), put("k", "next")))
            .expect("leader admits");
        c.deliver_all();
        let next = c.m(A).log().last().unwrap().clone();
        assert_eq!(next.at.0 - at.0, ms, "the next entry is stamped as the stall ends");
        next.extend
    };
    let mut c = Cluster::new(29);
    c.elect(A);
    c.sync(A);
    c.sync(A);
    assert_eq!(stalled(&mut c, 8_000, floor - 500), None, "a sub-floor stall stamps no extend");
    c.sync(A);
    for s in SITES {
        assert_eq!(c.m(s).state().stalled_since, None, "{s:?} cleared its marker");
    }
    // Idle time after it: a kept marker would stamp the whole idle time on the next entry.
    c.run(30_000, 1_000, |_| true);
    c.on(A, |m, now| m.commit(now, RequestId(graftyard_raft::Ulid(8_100)), put("k", "idle"))).expect("admits");
    c.deliver_all();
    assert_eq!(c.m(A).log().last().unwrap().extend, None, "idle time after a sub-floor stall moves nothing");
    c.sync(A);
    let x = stalled(&mut c, 8_200, floor).expect("a stall of EXTEND_MIN stamps an extend");
    assert_eq!(x.by.as_millis() as u64, floor, "{x:?}");
}

/// write-cuts cut (2): the floor judges a stall by its own length from `since`, not by the time since the last entry.
/// A follower that campaigns after the old leader went quiet marks its stall at its campaign start, so a campaign that
/// takes 500 ms stamps no `extend` on the new leader's `Noop`, however long the group was idle before it.
#[test]
fn a_quick_campaign_after_idle_time_stamps_no_extend() {
    let mut c = Cluster::new(31);
    c.elect(A);
    c.sync(A);
    c.sync(A);
    // The election's own marker rode the first syncs; B holds no unapplied entry carrying an `extend`.
    c.sync(A);
    let last = c.m(A).log().last().unwrap().at;
    assert!(c.m(B).log().iter().all(|e| e.index <= c.m(B).state().applied || e.extend.is_none()));
    c.crash(A);
    c.advance(graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 20_000);
    assert_eq!(c.on(B, |m, now| m.campaign(now)), Verdict::Started);
    // The pre-vote settles at once; the real vote's requests are held for 500 ms.
    let mut held = Vec::new();
    while !c.inbox.is_empty() {
        for e in std::mem::take(&mut c.inbox) {
            if matches!(&e.msg, Message::Vote(v) if !v.pre) {
                held.push(e);
            } else if e.to != A {
                c.on(e.to, |m, now| m.step(now, e.from, e.msg));
            }
        }
    }
    assert!(c.m(B).state().stalled_since.is_some(), "the real campaign marked a stall");
    assert!(held.iter().any(|e| matches!(&e.msg, Message::Vote(v) if !v.pre)), "{} held", held.len());
    c.now = Timestamp(c.now.0 + 500);
    c.inbox = held;
    c.deliver(|e| e.to != A);
    assert_eq!(c.m(B).role(), Role::Leader);
    let since_last = c.m(B).log().last().unwrap().at.0 - last.0;
    assert!(since_last > 20_000, "the new leader's first entry is {since_last} ms after the last one");
    let ext: Vec<_> = c.m(B).log().iter().filter(|e| e.at > last).filter_map(|e| e.extend).collect();
    assert!(ext.is_empty(), "a campaign shorter than EXTEND_MIN moved leases: {ext:?}");
}

/// R29: a group that commits an entry and goes quiet still tells its followers the commit within about one
/// CALL_DEADLINE, with no further command; then its leader sends nothing more while idle. Before, a follower learned a
/// commit only from the next append, so on graft-m4a's 65567 both followers sat one commit behind until a `/campaign`.
#[test]
fn an_idle_leader_tells_its_followers_the_last_commit_once() {
    let mut c = Cluster::new(7);
    c.elect(A);
    c.sync(A);
    admit(&mut c, A, 29, put("last", "write"));
    c.deliver_all();
    let commit = c.m(A).state().commit;
    assert_eq!(c.m(A).last_index(), commit, "the leader committed its last entry");
    assert!(c.m(B).state().commit < commit && c.m(C).state().commit < commit, "the followers learn it only later");
    let deadline = graftyard_raft::CALL_DEADLINE.as_millis() as u64;
    // Not before CALL_DEADLINE: a busy group's next entry carries the commit instead of an empty round.
    c.advance(deadline / 2);
    assert!(c.inbox.is_empty(), "a commit round inside CALL_DEADLINE");
    c.advance(deadline / 2 + 1);
    c.deliver_all();
    for s in [B, C] {
        assert_eq!(c.m(s).state().commit, commit, "{s:?} learned the last commit within one CALL_DEADLINE");
    }
    // Told, the followers get nothing more from an idle leader.
    for _ in 0..5 {
        c.advance(deadline + 1);
        assert!(c.inbox.is_empty(), "an idle leader sent {:?}", c.inbox.iter().map(|e| &e.msg).collect::<Vec<_>>());
    }
    assert_eq!(c.m(A).role(), Role::Leader);
}

/// A fresh leader has told no follower anything, so its first idle period sends each follower one empty round after
/// CALL_DEADLINE, which carries its term's commit, and then nothing: right after the group's first election and after a
/// hand-off, and the leader keeps leading through both.
#[test]
fn a_fresh_leaders_first_idle_period_sends_one_round_per_follower_then_nothing() {
    let deadline = graftyard_raft::CALL_DEADLINE.as_millis() as u64;
    // Per idle CALL_DEADLINE, the AppendEntries `l` sent.
    let idle = |c: &mut Cluster, l: SiteId| -> Vec<usize> {
        (0..6)
            .map(|_| {
                c.advance(deadline + 1);
                let n = c.inbox.iter().filter(|e| e.from == l && matches!(e.msg, Message::Append(_))).count();
                c.deliver_all();
                n
            })
            .collect()
    };
    let mut c = Cluster::new(9);
    c.elect(A);
    assert_eq!(idle(&mut c, A), [2, 0, 0, 0, 0, 0], "a new group's leader");
    for s in [B, C] {
        assert_eq!(c.m(s).state().commit, c.m(A).state().commit, "{s:?} holds the first term's commit");
    }
    c.on(A, |m, now| m.transfer(now, B)).expect("started");
    for _ in 0..40 {
        c.advance(100);
        c.deliver_all();
        if c.m(B).role() == Role::Leader && c.m(A).role() != Role::Leader {
            break;
        }
    }
    assert_eq!(c.leader(), Some(B));
    assert_eq!(idle(&mut c, B), [2, 0, 0, 0, 0, 0], "a leader after a hand-off");
    for s in [A, C] {
        assert_eq!(c.m(s).state().commit, c.m(B).state().commit, "{s:?} holds the new term's commit");
    }
    assert_eq!(c.leader(), Some(B));
}
