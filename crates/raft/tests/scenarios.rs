//! Targeted cases: BF-12, BF-13, bootstrap and no-config refusals, `led_term`, persist-before-send, encoding, quorum.

mod common;

use common::cluster::Cluster;
use common::kv::{KvCmd, KvErr};
use common::sim::{CHAIN, SITES, config, record};
use graftyard_raft::{
    ByRequest, CallError, CommitError, Deny, Digest, EntryBody, Index, Member, Message, RequestId, Resolution, Role,
    Term, Verdict, VoteReq, bootstrap, digest, quorums,
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
        g.requests().map(|(id, r)| (*id, r.clone())),
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
    assert_eq!(a.requests().count(), 1, "older rows expired");
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
    let checks = PlacementChecks { sites: &table, separate_accounts: false, rtt: &rtt };
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
/// CALL_DEADLINE on every send, so the follower never caught up (`tests/m1/results/m1-wp8.md`, t-256).
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
    // Each later round carries at most APPEND_BYTES_MAX past its first entry; C still catches up.
    let mut most = 0;
    for _ in 0..40 {
        c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
        let mut sizes = Vec::new();
        c.deliver(|e| {
            if let Message::Append(a) = &e.msg
                && e.to == C
                && a.entries.len() > 1
            {
                sizes.push(a.entries.iter().map(|x| graftyard_raft::encode_entry(x).len()).sum::<usize>());
            }
            true
        });
        most = sizes.into_iter().fold(most, usize::max);
        if c.m(C).log().len() == c.m(A).log().len() {
            break;
        }
    }
    assert_eq!(c.m(C).digests(), c.m(A).digests(), "C caught up");
    assert!(most <= graftyard_raft::APPEND_BYTES_MAX, "one AppendEntries carried {most} bytes");
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
