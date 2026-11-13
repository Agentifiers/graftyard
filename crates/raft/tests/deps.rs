//! Dependency reads that overlap the entry in flight (`design/DESIGN.md`, Shape, "The write path"): the leader may
//! cut the next batch and ask for its reads while one entry is in flight, and decides that batch only once the entry
//! committed and applied, on the fold then, with exactly the chains `deps()` names on it. Each test names the wrong
//! implementation it fails on.

mod common;

use common::cluster::Cluster;
use common::kv::{Kv, KvCmd, KvErr, OWNER_KEY, REVOKED_KEY, owner_chain};
use common::sim::SITES;
use graftyard_raft::{CommitError, DepsWanted, Message, RequestId, Resolution, Ticket, Ulid};
use graftyard_types::{AccountId, ChainId, DbId, SiteId};

const A: SiteId = SITES[0];
const B: SiteId = SITES[1];

fn put(k: &str, v: &str) -> KvCmd {
    KvCmd::Put { key: k.into(), value: v.into() }
}
fn put_dep(k: &str, v: &str) -> KvCmd {
    KvCmd::PutDep { key: k.into(), value: v.into() }
}
fn cas(k: &str, expect: &str, v: &str) -> KvCmd {
    KvCmd::Cas { key: k.into(), expect: Some(expect.into()), value: v.into() }
}
fn authz(n: i64) -> ChainId {
    ChainId::Authz(AccountId(DbId(n)))
}
fn revoked() -> Kv {
    Kv([(REVOKED_KEY.to_owned(), "1".to_owned())].into())
}

fn admit(c: &mut Cluster, s: SiteId, id: u128, cmd: KvCmd) -> Ticket {
    c.on(s, |m, now| m.commit(now, RequestId(Ulid(id)), cmd)).expect("admitted").ticket
}

/// The ticket's latest resolution. A leadership that ends on a higher term answers `NotLeader` under that term, so
/// such an answer matches by boot and seq; seq restarts with each leadership, so any other must match whole.
fn result(c: &Cluster, t: Ticket) -> Option<Result<bool, CommitError<KvErr>>> {
    c.resolutions.iter().rev().find_map(|(_, r)| match r {
        Resolution::Ticket { ticket, result } if *ticket == t => Some(result.clone().map(|a| a.out)),
        Resolution::Ticket { ticket, result: Err(e @ CommitError::NotLeader(_)) }
            if (ticket.boot, ticket.seq) == (t.boot, t.seq) && ticket.term.0 == t.term.0 + 1 =>
        {
            Some(Err(e.clone()))
        }
        _ => None,
    })
}

/// The reads asked for since `from`, by this member.
fn asked(c: &Cluster, s: SiteId, from: usize) -> Vec<DepsWanted> {
    c.deps[from..].iter().filter(|d| d.0 == s).map(|d| d.2.clone()).collect()
}

fn answer(c: &mut Cluster, s: SiteId, w: &DepsWanted, fold: impl Fn(ChainId) -> Kv) {
    let deps = w.chains.iter().map(|ch| (*ch, fold(*ch))).collect();
    c.on(s, |m, now| m.deps_read(now, w.id, deps));
}

/// Delivers everything, then lets the leader resend what `hold` dropped, which it does after CALL_DEADLINE.
fn flush(c: &mut Cluster) {
    c.deliver_all();
    c.advance(graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
    c.deliver_all();
}

/// Holds every AppendEntries and reply: the entry in flight stays in flight.
fn hold(c: &mut Cluster) {
    c.inbox.clear();
}

fn in_flight(c: &Cluster, s: SiteId) -> u64 {
    c.m(s).last_index().0 - c.m(s).state().commit.0
}

/// A leader on A that has measured one dependency read of 50 ms, so it may cut early.
fn warm(seed: u64) -> Cluster {
    let mut c = Cluster::new(seed);
    c.elect(A);
    c.sync(A);
    let from = c.deps.len();
    let t = admit(&mut c, A, 1, put_dep("w", "0"));
    let w = asked(&c, A, from).pop().expect("a read for the first entry");
    c.advance(50);
    answer(&mut c, A, &w, |_| Kv::default());
    c.deliver_all();
    assert_eq!(result(&c, t), Some(Ok(true)));
    c
}

/// Puts entry `k` (a plain put, so no read) in flight on A and holds it there. Returns its index.
fn entry_in_flight(c: &mut Cluster, id: u128, cmd: KvCmd) -> u64 {
    admit(c, A, id, cmd);
    hold(c);
    assert_eq!(in_flight(c, A), 1);
    c.m(A).last_index().0
}

/// Advances past the early-cut point (the round less one read) with the entry still in flight, and returns the read
/// the leader asked for the next batch.
fn early_ask(c: &mut Cluster, from: usize) -> DepsWanted {
    for _ in 0..10 {
        c.advance(100);
        hold(c);
        assert!(in_flight(c, A) <= 1, "more than one entry in flight");
        if let Some(w) = asked(c, A, from).pop() {
            return w;
        }
    }
    panic!("no read was asked while the entry was in flight");
}

/// Fails if the leader waits for the entry in flight to commit before it asks for the next batch's reads (today's
/// path alone), and if it ever has a second entry in flight.
#[test]
fn the_next_entrys_deps_are_read_while_one_entry_is_in_flight() {
    let mut c = warm(81);
    let k = entry_in_flight(&mut c, 10, put("x", "1"));
    let from = c.deps.len();
    let t = admit(&mut c, A, 11, put_dep("y", "1"));
    assert!(asked(&c, A, from).is_empty(), "asked before the early-cut point");
    let w = early_ask(&mut c, from);
    assert_eq!(w.chains.as_slice(), [owner_chain(&c.m(A).group_fold().fold)]);
    answer(&mut c, A, &w, |_| Kv::default());
    assert_eq!(c.m(A).last_index().0, k, "decided while the entry was in flight");
    assert!(result(&c, t).is_none());
    flush(&mut c);
    assert_eq!(result(&c, t), Some(Ok(true)));
    assert_eq!(c.m(A).state().commit.0, k + 1);
    assert_eq!(c.deps_used.last().map(|d| (d.1.index.0, d.1.ids.as_slice() == [w.id])), Some((k + 1, true)));
}

/// Fails if the batch is decided when its reads arrive, on the fold before the entry in flight applied: the
/// compare-and-set expects the value that entry writes, so it is refused `Stale` there.
#[test]
fn an_early_batch_decides_on_the_fold_after_the_entry_in_flight() {
    let mut c = warm(82);
    let k = entry_in_flight(&mut c, 20, put("x", "1"));
    let from = c.deps.len();
    let td = admit(&mut c, A, 21, put_dep("y", "1"));
    let tc = admit(&mut c, A, 22, cas("x", "1", "2"));
    let w = early_ask(&mut c, from);
    answer(&mut c, A, &w, |_| Kv::default());
    assert_eq!(c.m(A).last_index().0, k, "decided while the entry was in flight");
    flush(&mut c);
    assert_eq!(result(&c, td), Some(Ok(true)));
    assert_eq!(result(&c, tc), Some(Ok(true)), "the batch was decided before the entry in flight applied");
    assert_eq!(c.value(A, "x").as_deref(), Some("2"));
}

/// The entry in flight moves the dependency (`OWNER_KEY`, as a transfer moves `RepoSettings`). Fails if the batch is
/// decided on the early read alone (no re-read: `NoDep` for the old chain), or with the early read passed beside
/// the new one (stale extra deps: `NoDep` for two chains), or if the re-read asks again for the chain already read.
#[test]
fn deps_changed_by_the_entry_in_flight_are_read_after_it_applies() {
    let mut c = warm(83);
    assert_eq!(owner_chain(&c.m(A).group_fold().fold), authz(0));
    let k = entry_in_flight(&mut c, 30, put(OWNER_KEY, "2"));
    let from = c.deps.len();
    let t = admit(&mut c, A, 31, put_dep("y", "1"));
    let early = early_ask(&mut c, from);
    assert_eq!(early.chains.as_slice(), [authz(0)]);
    answer(&mut c, A, &early, |_| Kv::default());
    let from = c.deps.len();
    // The entry in flight commits; its apply names Authz(2).
    flush(&mut c);
    let again = asked(&c, A, from);
    assert_eq!(again.len(), 1, "no re-read for the chain the applied entry named");
    assert_eq!(again[0].chains.as_slice(), [authz(2)], "the re-read must name only the missing chain");
    assert_eq!(c.m(A).last_index().0, k);
    answer(&mut c, A, &again[0], |_| Kv::default());
    flush(&mut c);
    assert_eq!(result(&c, t), Some(Ok(true)), "decided without exactly the dependency the fold names");
    let used = &c.deps_used.last().expect("decided on reads").1;
    assert_eq!((used.index.0, used.ids.as_slice()), (k + 1, [early.id, again[0].id].as_slice()));
}

/// Fails if a command admitted after the early read was asked for joins that batch: its admission postdates the
/// read, so a revocation acknowledged in between would not bind it.
#[test]
fn a_command_admitted_after_the_early_cut_waits_for_the_next_entry() {
    let mut c = warm(84);
    let k = entry_in_flight(&mut c, 40, put("x", "1"));
    let from = c.deps.len();
    admit(&mut c, A, 41, put_dep("y", "1"));
    let w = early_ask(&mut c, from);
    let late = admit(&mut c, A, 42, put_dep("z", "1"));
    answer(&mut c, A, &w, |_| Kv::default());
    flush(&mut c);
    assert_eq!(ids(&c, k + 1), [41], "a command admitted after the read joined its batch");
    // The late command reads its own dependency for the entry after.
    let next = asked(&c, A, from).into_iter().find(|d| d.id != w.id).expect("the late command's read");
    answer(&mut c, A, &next, |_| Kv::default());
    flush(&mut c);
    assert_eq!(result(&c, late), Some(Ok(true)));
    assert_eq!(ids(&c, k + 2), [42]);
}

fn ids(c: &Cluster, i: u64) -> Vec<u128> {
    c.m(A).entry_at(graftyard_raft::Index(i)).expect("held").requests.iter().map(|(r, _)| r.0.0).collect()
}

/// Fails if dependency ids restart with each leadership: the answer to a read the first leadership asked for, which
/// carries a revoked credential, would then decide the second leadership's batch.
#[test]
fn a_deps_read_from_an_earlier_leadership_decides_nothing() {
    let mut c = Cluster::new(85);
    c.elect(A);
    c.sync(A);
    let from = c.deps.len();
    let t1 = admit(&mut c, A, 50, put_dep("y", "1"));
    let stale = asked(&c, A, from).pop().expect("asked");
    // A loses the lead to B, then takes it back, with no restart.
    c.elect_after_promises(B);
    assert!(matches!(result(&c, t1), Some(Err(CommitError::NotLeader(_)))));
    c.sync(B);
    c.elect_after_promises(A);
    c.sync(A);
    let from = c.deps.len();
    let t2 = admit(&mut c, A, 51, put_dep("y", "2"));
    let fresh = asked(&c, A, from).pop().expect("asked");
    assert_ne!(fresh.id, stale.id, "a dependency id was reused by a later leadership");
    answer(&mut c, A, &stale, |_| revoked());
    flush(&mut c);
    assert!(result(&c, t2).is_none(), "an earlier leadership's read decided the batch: {:?}", result(&c, t2));
    answer(&mut c, A, &fresh, |_| Kv::default());
    flush(&mut c);
    assert_eq!(result(&c, t2), Some(Ok(true)));
}

/// A transfer may run for a LEASE. Fails if the held batch keeps reads from before it: a revocation acknowledged
/// during the transfer must bind the batch, so once the transfer is abandoned the batch reads again.
#[test]
fn a_transfer_drops_held_deps() {
    let mut c = warm(86);
    entry_in_flight(&mut c, 60, put("x", "1"));
    let from = c.deps.len();
    let t = admit(&mut c, A, 61, put_dep("y", "1"));
    let w = early_ask(&mut c, from);
    answer(&mut c, A, &w, |_| Kv::default());
    c.on(A, |m, now| m.transfer(now, B)).expect("started");
    let from = c.deps.len();
    // The target never gets `TimeoutNow`, so the transfer is abandoned after a LEASE; meanwhile the token is revoked.
    let lease = graftyard_raft::LEASE.as_millis() as u64;
    c.run(lease + 1_000, 100, |e| !matches!(e.msg, Message::TimeoutNow { .. }));
    assert!(c.m(A).transferring().is_none(), "the transfer was not abandoned");
    let again = asked(&c, A, from);
    assert_eq!(again.len(), 1, "the batch kept the reads it held before the transfer");
    answer(&mut c, A, &again[0], |_| revoked());
    flush(&mut c);
    assert_eq!(result(&c, t), Some(Err(CommitError::Refused { index: c.m(A).state().commit, err: KvErr::Revoked })));
}

/// The end of a leadership answers `NotLeader` for both the entry in flight and the batch held behind it.
#[test]
fn leadership_end_answers_the_held_batch_and_the_entry_in_flight() {
    let mut c = warm(87);
    let tk = admit(&mut c, A, 70, put("x", "1"));
    hold(&mut c);
    let from = c.deps.len();
    let th = admit(&mut c, A, 71, put_dep("y", "1"));
    early_ask(&mut c, from);
    c.elect_after_promises(B);
    for t in [tk, th] {
        assert!(matches!(result(&c, t), Some(Err(CommitError::NotLeader(_)))), "{t:?}: {:?}", result(&c, t));
    }
}
