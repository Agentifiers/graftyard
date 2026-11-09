//! LF-13's fault, on real members (`build/BUILD-PLAN.md`, AM1-WP4, LF-13; `tests/am1/rest/lf13.mjs` runs it hosted).
//! The row drops a follower's AppendEntries for 5 s "so the next one leaves a gap". From the same leader it never
//! does: a leader keeps one round in flight per follower and moves `next` only on an ok answer, so after any number
//! of lost rounds it sends again from the follower's `matched`. Two faults do leave the follower behind its leader's
//! next round, and the feed's host marks both (`workers/site/src/family.rs`, `stepped`):
//!
//! - **gap**: the follower misses rounds, then leadership moves to the other follower, whose first round to it
//!   starts at its own last entry, past the follower's: a refused AppendEntries (`AppendRes { ok: false }`).
//! - **install**: the follower misses rounds while the leader's stored log is archived and truncated past the
//!   follower's last entry (`set_install`): it is sent an `InstallSnapshot` (`Out::install`).

mod common;

use common::cluster::Cluster;
use common::kv::{KvCmd, KvEvent};
use common::sim::SITES;
use graftyard_raft::{Envelope, Index, InstallSource, Message, RequestId, Role, Ulid};
use graftyard_types::SiteId;

const A: SiteId = SITES[0];
const B: SiteId = SITES[1];
const C: SiteId = SITES[2];

/// Commits `n` entries through `l` while every message to or from `C` is dropped.
fn commit_without_c(c: &mut Cluster, l: SiteId, from: u128, n: u128) {
    for k in 0..n {
        let cmd = KvCmd::Put { key: format!("k{k}"), value: "v".into() };
        c.on(l, |m, now| m.commit(now, RequestId(Ulid(from + k)), cmd)).expect("leader admits");
        c.deliver(|e| e.to != C && e.from != C);
        c.advance(100);
        c.deliver(|e| e.to != C && e.from != C);
    }
}

/// What `C` answers to every AppendEntries and `InstallSnapshot` it is sent, once its messages flow again.
fn answers_of_c(c: &mut Cluster) -> (Vec<(SiteId, bool)>, usize) {
    let mut res = Vec::new();
    let mut installs = 0;
    c.deliver(|e: &Envelope<KvEvent>| {
        match &e.msg {
            Message::AppendRes(r) if e.from == C => res.push((e.to, r.ok)),
            Message::Install(_) if e.to == C => installs += 1,
            _ => {}
        }
        // An install the cluster cannot load: C asks for it and the harness stops there.
        !matches!(e.msg, Message::Install(_))
    });
    (res, installs)
}

#[test]
fn rounds_lost_to_the_same_leader_leave_no_gap() {
    let mut c = Cluster::new(71);
    c.elect(A);
    c.sync(A);
    commit_without_c(&mut c, A, 1_000, 20);
    assert!(c.m(C).log().len() + 20 <= c.m(A).log().len(), "C fell behind");
    // The fault lifts: A sends from C's matched index, and every answer is ok.
    c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
    let (res, installs) = answers_of_c(&mut c);
    assert!(!res.is_empty() && res.iter().all(|(_, ok)| *ok), "same leader: {res:?}");
    assert_eq!(installs, 0);
    assert_eq!(c.m(C).log().len(), c.m(A).log().len());
}

#[test]
fn a_new_leader_s_first_round_leaves_a_gap_on_the_lagging_follower() {
    let mut c = Cluster::new(72);
    c.elect(A);
    c.sync(A);
    commit_without_c(&mut c, A, 2_000, 20);
    // Leadership moves to B while C is still cut off; the transfer waits out C's missing ack.
    c.on(A, |m, now| m.transfer(now, B)).expect("transfer starts");
    c.run(3_000, 100, |e| e.to != C && e.from != C);
    assert_eq!(c.m(B).role(), Role::Leader);
    let behind = c.m(C).log().len();
    c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
    let (res, installs) = answers_of_c(&mut c);
    // C's first answer to B refuses (the gap), then B backs up and C catches up.
    let first = res.iter().find(|(to, _)| *to == B).copied();
    assert_eq!(first, Some((B, false)), "C's first answer to the new leader: {res:?}");
    assert!(res.iter().any(|(to, ok)| *to == B && *ok), "C caught up: {res:?}");
    assert_eq!(installs, 0);
    assert!(c.m(C).log().len() > behind);
    assert_eq!(c.m(C).log().len(), c.m(B).log().len());
}

#[test]
fn a_log_truncated_past_the_follower_sends_it_an_install() {
    let mut c = Cluster::new(73);
    c.elect(A);
    c.sync(A);
    let c_last = c.m(C).log().len() as u64;
    commit_without_c(&mut c, A, 3_000, 20);
    // The leader's host archived and truncated through `through`, past C's last entry.
    let through = Index(c.m(A).state().commit.0 - 2);
    assert!(through.0 > c_last);
    let e = c.m(A).entry_at(through).expect("the leader holds through").clone();
    let src = InstallSource {
        core: "snapshot/core".into(),
        metadata: None,
        archives: ["archive/1".into()].into_iter().collect(),
        through,
        through_term: e.term,
        through_digest: graftyard_raft::digest(&e),
        config: c.m(A).group_fold().config().cloned().expect("a committed config"),
        id_block: None,
    };
    c.members[A.0 as usize].as_mut().unwrap().set_install(Some(src));
    c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
    let (_, installs) = answers_of_c(&mut c);
    assert!(installs >= 1, "C was sent no InstallSnapshot");
}
