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

/// The ticket's latest resolution.
fn result(c: &Cluster, t: Ticket) -> Option<Result<bool, CommitError<KvErr>>> {
    c.resolutions.iter().rev().find_map(|(_, r)| match r {
        Resolution::Ticket { ticket, result } if *ticket == t => Some(result.clone().map(|a| a.out)),
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

/// Fails if a command admitted after the early read was asked for joins that batch on that read: its admission
/// postdates the read, so a revocation acknowledged in between would not bind it. (A re-ask asked after it may bring
/// it in, `reask-read`; here none answers before the entry in flight applies.)
#[test]
fn a_command_admitted_after_the_early_cut_waits_for_the_next_entry() {
    let mut c = warm(84);
    let k = entry_in_flight(&mut c, 40, put("x", "1"));
    let from = c.deps.len();
    admit(&mut c, A, 41, put_dep("y", "1"));
    let w = early_ask(&mut c, from);
    let late = admit(&mut c, A, 42, put_dep("z", "1"));
    answer(&mut c, A, &w, |_| Kv::default());
    let answered = c.deps.len();
    flush(&mut c);
    assert_eq!(ids(&c, k + 1), [41], "a command admitted after the read joined its batch");
    // The late command reads its own dependency for the entry after; a re-ask asked before k applied is dropped.
    let mut seen = vec![w.id];
    for _ in 0..4 {
        for d in asked(&c, A, answered) {
            if !seen.contains(&d.id) {
                seen.push(d.id);
                answer(&mut c, A, &d, |_| Kv::default());
            }
        }
        flush(&mut c);
    }
    assert_eq!(result(&c, late), Some(Ok(true)));
    assert_eq!(ids(&c, k + 2), [42]);
}

/// `reask-read`: a command admitted after a frozen batch's read joins the batch once a re-ask, asked after its
/// admission, answers before the entry in flight applies. The dependency changes between the batch's read and the
/// re-ask (the re-ask carries a revocation): every command of the entry, the one cut first included, must decide on
/// the re-ask alone. Fails if the late command rides the entry after (no top-up), if the batch's commands keep the
/// first read beside the re-ask (two reads, so `decide` would see a stale one for the first command), or if
/// `DepsUsed` names any read but the re-ask.
#[test]
fn a_re_ask_answered_before_the_apply_brings_in_the_late_command_and_every_command_decides_on_it() {
    let mut c = warm(93);
    let k = entry_in_flight(&mut c, 130, put("x", "1"));
    let from = c.deps.len();
    let first = admit(&mut c, A, 131, put_dep("y", "1"));
    let w = early_ask(&mut c, from);
    let late = admit(&mut c, A, 132, put_dep("z", "1"));
    assert_eq!(c.m(A).last_index().0, k);
    let from = c.deps.len();
    answer(&mut c, A, &w, |_| Kv::default());
    let re = asked(&c, A, from);
    assert_eq!(re.len(), 1, "no re-ask once the batch's read answered with a command queued behind it");
    assert_eq!(re[0].chains.as_slice(), [owner_chain(&c.m(A).group_fold().fold)]);
    // Admitted after the re-ask was asked: it must not join on it.
    let after = admit(&mut c, A, 133, put("q", "1"));
    hold(&mut c);
    answer(&mut c, A, &re[0], |_| revoked());
    assert_eq!(c.m(A).last_index().0, k, "decided while the entry was in flight");
    flush(&mut c);
    assert_eq!(result(&c, after), Some(Ok(true)));
    assert!(!ids(&c, k + 1).contains(&133), "a command admitted after the re-ask was asked joined on it");
    // A refused command leaves no request row, so the entry is named by each refusal's index.
    for t in [first, late] {
        assert_eq!(
            result(&c, t),
            Some(Err(CommitError::Refused { index: graftyard_raft::Index(k + 1), err: KvErr::Revoked })),
            "{t:?} did not decide in entry k+1 on the re-ask"
        );
    }
    let used = &c.deps_used.last().expect("decided on reads").1;
    assert_eq!((used.index.0, used.ids.as_slice()), (k + 1, [re[0].id].as_slice()));
}

/// `reask-read`: a re-ask still out when the entry in flight applies is dropped; the batch decides at once on the read
/// it has, without the late command, and an answer to the dropped re-ask decides nothing. Fails if the batch waits
/// for the re-ask (the decide is held), or if the dropped re-ask's answer, which carries a revocation, binds anything.
#[test]
fn an_unanswered_re_ask_never_holds_the_batch() {
    let mut c = warm(94);
    let k = entry_in_flight(&mut c, 140, put("x", "1"));
    let from = c.deps.len();
    let first = admit(&mut c, A, 141, put_dep("y", "1"));
    let w = early_ask(&mut c, from);
    let late = admit(&mut c, A, 142, put_dep("z", "1"));
    let from = c.deps.len();
    answer(&mut c, A, &w, |_| Kv::default());
    // One re-ask out at a time: ticks with it unanswered ask no other.
    for _ in 0..5 {
        c.advance(100);
        hold(&mut c);
    }
    let re = asked(&c, A, from);
    assert_eq!(re.len(), 1, "a second re-ask left while one was out");
    flush(&mut c);
    assert_eq!(result(&c, first), Some(Ok(true)), "the batch waited for its re-ask");
    assert_eq!(ids(&c, k + 1), [141]);
    answer(&mut c, A, &re[0], |_| revoked());
    let mut seen = vec![w.id, re[0].id];
    for _ in 0..4 {
        for d in asked(&c, A, from) {
            if !seen.contains(&d.id) {
                seen.push(d.id);
                answer(&mut c, A, &d, |_| Kv::default());
            }
        }
        flush(&mut c);
    }
    assert_eq!(result(&c, late), Some(Ok(true)), "the dropped re-ask's revocation bound a later entry");
    assert_eq!(ids(&c, k + 2), [142]);
}

/// `reask-read`'s caps. Fails if a re-ask brings in a command past the batch's soft bytes, past AGENT_CMDS_ENTRY Agent
/// commands, or when the re-ask's answer lacks a chain it asked (the batch would then decide on no read of it).
#[test]
fn a_re_ask_keeps_the_batchs_caps_and_needs_every_chain() {
    use graftyard_raft::admit::{AGENT_CMDS_ENTRY, ActorKey, Admit, AdmitClass};
    // Soft bytes: at 1 byte, the batch's first command fills it.
    let mut c = warm(96);
    c.m_mut(A).set_entry_soft(1);
    let k = entry_in_flight(&mut c, 160, put("x", "1"));
    let from = c.deps.len();
    admit(&mut c, A, 161, put_dep("y", "1"));
    let w = early_ask(&mut c, from);
    admit(&mut c, A, 162, put_dep("z", "1"));
    let from = c.deps.len();
    answer(&mut c, A, &w, |_| Kv::default());
    for r in asked(&c, A, from) {
        answer(&mut c, A, &r, |_| Kv::default());
    }
    flush(&mut c);
    assert_eq!(ids(&c, k + 1), [161], "a re-ask took the batch past its soft bytes");

    // AGENT_CMDS_ENTRY: a cut of exactly that many Agent commands ends `Empty`, and takes no more.
    let mut c = warm(97);
    let k = entry_in_flight(&mut c, 170, put("x", "1"));
    let agent = |c: &mut Cluster, i: u128| {
        let a = Admit {
            class: AdmitClass::Agent,
            actor: Some(ActorKey(i as u64)),
            push: false,
            inline: 0,
            placement_ok: true,
            budget: None,
        };
        c.on(A, |m, now| m.commit_as(now, RequestId(Ulid(i)), put_dep(&format!("a{i}"), "1"), a)).expect("admitted");
    };
    for i in 0..AGENT_CMDS_ENTRY as u128 {
        agent(&mut c, 2_000 + i);
    }
    let from = c.deps.len();
    let w = early_ask(&mut c, from);
    agent(&mut c, 2_999);
    let from = c.deps.len();
    answer(&mut c, A, &w, |_| Kv::default());
    for r in asked(&c, A, from) {
        answer(&mut c, A, &r, |_| Kv::default());
    }
    flush(&mut c);
    assert_eq!(ids(&c, k + 1).len(), AGENT_CMDS_ENTRY, "a re-ask took the batch past AGENT_CMDS_ENTRY");

    // A re-ask answered without the chain it asked changes nothing: the batch keeps its read and its commands.
    let mut c = warm(98);
    let k = entry_in_flight(&mut c, 180, put("x", "1"));
    let from = c.deps.len();
    let first = admit(&mut c, A, 181, put_dep("y", "1"));
    let w = early_ask(&mut c, from);
    admit(&mut c, A, 182, put_dep("z", "1"));
    let from = c.deps.len();
    answer(&mut c, A, &w, |_| Kv::default());
    let re = asked(&c, A, from);
    assert_eq!(re.len(), 1);
    c.on(A, |m, now| m.deps_read(now, re[0].id, Vec::new()));
    hold(&mut c);
    flush(&mut c);
    assert_eq!(result(&c, first), Some(Ok(true)), "a re-ask missing its chain replaced the batch's read");
    assert_eq!(ids(&c, k + 1)[0], 181);
}

/// `reask-read` leaves a backlogged cut alone (`human-join` was reversed on B0-33, where frozen batches had an agent
/// backlog behind them). With AGENT_CMDS_ENTRY plus one Agent commands queued before the early cut, the cut ends
/// `AgentCmds`; a command admitted after it must ride the entry after, and no re-ask is asked. Fails if a cut that
/// left a backlog takes a re-ask or a joiner.
#[test]
fn a_cut_that_left_a_backlog_asks_no_re_ask() {
    use graftyard_raft::admit::{AGENT_CMDS_ENTRY, ActorKey, Admit, AdmitClass};
    let mut c = warm(95);
    let k = entry_in_flight(&mut c, 150, put("x", "1"));
    let agent = |c: &mut Cluster, i: u128| {
        let a = Admit {
            class: AdmitClass::Agent,
            actor: Some(ActorKey(i as u64)),
            push: false,
            inline: 0,
            placement_ok: true,
            budget: None,
        };
        c.on(A, |m, now| m.commit_as(now, RequestId(Ulid(i)), put_dep(&format!("a{i}"), "1"), a)).expect("admitted");
    };
    for i in 0..=AGENT_CMDS_ENTRY as u128 {
        agent(&mut c, 1_000 + i);
    }
    let from = c.deps.len();
    let w = early_ask(&mut c, from);
    let late = admit(&mut c, A, 151, put_dep("z", "1"));
    let from = c.deps.len();
    answer(&mut c, A, &w, |_| Kv::default());
    assert!(asked(&c, A, from).is_empty(), "a backlogged batch asked a re-ask");
    hold(&mut c);
    flush(&mut c);
    assert_eq!(ids(&c, k + 1).len(), AGENT_CMDS_ENTRY, "the backlogged batch took more than its cut");
    assert!(!ids(&c, k + 1).contains(&151), "a command admitted after a backlogged cut joined it");
    let _ = late;
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

/// B0-31's apac leader: a round of about 200 ms and dependency reads of about 370 ms, with commands arriving all the
/// time. Each message takes 100 ms one way and each read answers 370 ms after it is asked. Fails on a leader that
/// holds one frozen batch at a time: its cycle is a read plus `decide`, one entry per 370 ms or more; holding a
/// second batch whose read started a round later, the cycle falls to about a round. Also fails if more than one
/// entry is ever in flight.
#[test]
fn reads_longer_than_a_round_overlap_two_batches_so_the_cycle_is_about_a_round() {
    const ONE_WAY: u64 = 100;
    const READ: u64 = 370;
    const STEP: u64 = 10;
    let mut c = Cluster::new(88);
    c.elect(A);
    c.sync(A);
    let term0 = c.m(A).state().term;
    let mut wire: Vec<(u64, graftyard_raft::Envelope<common::kv::KvEvent>)> = Vec::new();
    let mut answered = c.deps.len();
    let mut next_id = 1_000u128;
    let mut commits: Vec<(u64, u64)> = Vec::new();
    let mut held_max = 0;
    let start = c.now.0;
    while c.now.0 < start + 20_000 {
        if (c.now.0 - start).is_multiple_of(20) {
            next_id += 1;
            admit(&mut c, A, next_id, put_dep(&format!("k{next_id}"), "1"));
        }
        c.advance(STEP);
        let now = c.now.0;
        wire.extend(std::mem::take(&mut c.inbox).into_iter().map(|e| (now + ONE_WAY, e)));
        let (due, later): (Vec<_>, Vec<_>) = wire.drain(..).partition(|(at, _)| *at <= now);
        wire = later;
        for (_, graftyard_raft::Envelope { from, to, msg }) in due {
            c.on(to, |m, t| m.step(t, from, msg));
        }
        while answered < c.deps.len() && c.deps[answered].1.0 + READ <= now {
            let (s, _, w) = c.deps[answered].clone();
            answer(&mut c, s, &w, |_| Kv::default());
            answered += 1;
        }
        wire.extend(std::mem::take(&mut c.inbox).into_iter().map(|e| (now + ONE_WAY, e)));
        assert!(in_flight(&c, A) <= 1, "more than one entry in flight");
        let commit = c.m(A).state().commit.0;
        if commits.last().is_none_or(|(_, k)| *k < commit) {
            commits.push((now, commit));
        }
        if now >= start + 10_000 {
            held_max = held_max.max(c.m(A).admission().expect("leading").held);
        }
    }
    // The second half, once the round and read averages settled.
    let half: Vec<_> = commits.iter().filter(|(t, _)| *t >= start + 10_000).collect();
    let (t0, k0) = *half[0];
    let (t1, k1) = *half[half.len() - 1];
    let cycle = (t1 - t0) / (k1 - k0);
    assert_eq!(c.m(A).state().term, term0, "the leader changed");
    assert!(cycle <= 250, "one entry per {cycle} ms: the cycle is a read, not a round (round about 200, read {READ})");
    assert!(cycle >= 190, "one entry per {cycle} ms is under one round: more than one entry in flight?");
    assert_eq!(held_max, 2, "ceil(370 / 200) batches held behind the entry in flight");
}

/// A leader whose dependency read (400 ms) outlasts its round, so it holds two batches behind the entry in flight.
fn warm_slow(seed: u64) -> Cluster {
    let mut c = Cluster::new(seed);
    c.elect(A);
    c.sync(A);
    let from = c.deps.len();
    let t = admit(&mut c, A, 1, put_dep("w", "0"));
    let w = asked(&c, A, from).pop().expect("a read for the first entry");
    c.advance(400);
    answer(&mut c, A, &w, |_| Kv::default());
    c.deliver_all();
    assert_eq!(result(&c, t), Some(Ok(true)));
    c
}

/// Two batches frozen behind entry k: k+1's reads, then k+2's. Fails if a read of k+2 that never answers, answers
/// without a chain the host could not read, or was asked before entry k+1 moved its dependency, touches k+1: k+1
/// decides and commits as it would alone. And k+2 never decides on the read entry k+1 made stale: it reads the chain
/// the fold names after k+1 applied, so the stale read's revocation binds nothing.
#[test]
fn a_failed_or_stale_read_for_a_later_batch_leaves_the_earlier_batch_alone() {
    // A read for k+2 that never answers, then answers empty (the host could not read the chain).
    let mut c = warm_slow(89);
    let k = entry_in_flight(&mut c, 90, put("x", "1"));
    let from = c.deps.len();
    let t1 = admit(&mut c, A, 91, put_dep("y", "1"));
    let r1 = early_ask(&mut c, from);
    let t2 = admit(&mut c, A, 92, put_dep("z", "1"));
    let r2 = early_ask(&mut c, from + 1);
    assert_ne!(r1.id, r2.id);
    assert_eq!(c.m(A).last_index().0, k, "a held batch was decided while entry k was in flight");
    answer(&mut c, A, &r1, |_| Kv::default());
    flush(&mut c);
    assert_eq!(result(&c, t1), Some(Ok(true)), "k+1 waited on k+2's read");
    assert_eq!(ids(&c, k + 1), [91]);
    assert!(result(&c, t2).is_none(), "k+2 decided before its read answered");
    c.on(A, |m, now| m.deps_read(now, r2.id, Vec::new()));
    flush(&mut c);
    assert_eq!(
        result(&c, t2),
        Some(Err(CommitError::Refused { index: graftyard_raft::Index(k + 2), err: KvErr::NoDep }))
    );
    assert_eq!(result(&c, t1), Some(Ok(true)));

    // Entry k+1 moves the dependency (`OWNER_KEY`) that k+2's early read was asked for; that read carries a revocation.
    // A batch is cut once a command with a dependency is queued, so k+1 is the owner move and `y`, and k+2 is `z`.
    let mut c = warm_slow(90);
    let k = entry_in_flight(&mut c, 100, put("x", "1"));
    let from = c.deps.len();
    let t1 = admit(&mut c, A, 101, put(OWNER_KEY, "2"));
    let t1y = admit(&mut c, A, 102, put_dep("y", "1"));
    let t2 = admit(&mut c, A, 103, put_dep("z", "1"));
    let mut early = Vec::new();
    for _ in 0..20 {
        early = asked(&c, A, from);
        if early.len() == 2 {
            break;
        }
        c.advance(100);
        hold(&mut c);
    }
    assert_eq!(early.len(), 2, "both batches' reads are asked while entry k is in flight");
    let at: Vec<u64> = c.deps[from..].iter().map(|d| d.1.0).collect();
    assert!(at[1] >= at[0] + 50, "k+2's read was asked with k+1's ({at:?}), not a round after it");
    assert!(early.iter().all(|w| w.chains.as_slice() == [authz(0)]), "asked on the fold before k+1 applied");
    assert_eq!(c.m(A).last_index().0, k);
    answer(&mut c, A, &early[1], |_| revoked());
    answer(&mut c, A, &early[0], |_| Kv::default());
    let from = c.deps.len();
    flush(&mut c);
    assert_eq!((result(&c, t1), result(&c, t1y)), (Some(Ok(true)), Some(Ok(true))), "k+1 was touched by k+2's read");
    assert_eq!(ids(&c, k + 1), [101, 102]);
    let again = asked(&c, A, from);
    assert_eq!(again.len(), 1, "no re-read after k+1 moved k+2's dependency");
    assert_eq!(again[0].chains.as_slice(), [authz(2)]);
    assert!(result(&c, t2).is_none(), "k+2 decided on the read k+1 made stale");
    answer(&mut c, A, &again[0], |_| Kv::default());
    flush(&mut c);
    assert_eq!(result(&c, t2), Some(Ok(true)), "k+2 decided on the read k+1 made stale");
}

/// Two batches frozen behind entry k: k+1 holds the owner move and is cut first, k+2 reads the old owner's chain
/// early. Then a third batch is cut behind them once k+1 is decided. Fails if the re-read k+2 needs once k+1 applied
/// is asked for the batch cut last: k+2 then never decides.
#[test]
fn the_re_read_goes_to_the_batch_being_decided() {
    let mut c = warm_slow(91);
    let k = entry_in_flight(&mut c, 110, put("x", "1"));
    let from = c.deps.len();
    let early = |c: &mut Cluster, n: usize| {
        for _ in 0..30 {
            if asked(c, A, from).len() >= n {
                break;
            }
            c.advance(100);
            hold(c);
        }
        asked(c, A, from)
    };
    let t1 = admit(&mut c, A, 111, put(OWNER_KEY, "2"));
    admit(&mut c, A, 112, put_dep("y", "1"));
    early(&mut c, 1);
    let t2 = admit(&mut c, A, 113, put_dep("z", "1"));
    let reads = early(&mut c, 2);
    assert_eq!(reads.len(), 2, "two batches held behind entry k");
    assert_eq!(c.m(A).last_index().0, k);
    answer(&mut c, A, &reads[0], |_| Kv::default());
    // k+1 commits with k+2's read still out; a third batch arrives and is cut behind k+2 while k+1 is in flight.
    c.deliver_all();
    let t3 = admit(&mut c, A, 114, put_dep("w", "1"));
    for _ in 0..10 {
        c.advance(100);
        hold(&mut c);
    }
    // Every read still out is answered as it is asked: those asked on the fold before k+1 applied name its old
    // owner, and any asked after name the new one.
    let mut done = 1;
    for _ in 0..8 {
        let out = asked(&c, A, from);
        for w in &out[done..] {
            answer(&mut c, A, w, |_| Kv::default());
        }
        done = out.len();
        flush(&mut c);
    }
    let after: Vec<_> =
        c.deps[from..].iter().filter(|d| d.1.0 > c.deps[from].1.0 + 1_000).map(|d| d.2.clone()).collect();
    assert!(after.iter().any(|w| w.chains.as_slice() == [authz(2)]), "no re-read of the chain k+1 named");
    assert_eq!(result(&c, t1), Some(Ok(true)));
    assert_eq!(result(&c, t2), Some(Ok(true)), "k+2 never decided: its re-read went to another batch");
    assert_eq!(result(&c, t3), Some(Ok(true)));
}

/// A transfer with two batches held. Fails if it drops only the front batch's reads: the second would then decide on a
/// read from before the transfer, though a revocation was acknowledged during it.
#[test]
fn a_transfer_drops_every_held_batchs_deps() {
    let mut c = warm_slow(92);
    entry_in_flight(&mut c, 120, put("x", "1"));
    let from = c.deps.len();
    let t1 = admit(&mut c, A, 121, put_dep("y", "1"));
    for _ in 0..20 {
        if !asked(&c, A, from).is_empty() {
            break;
        }
        c.advance(100);
        hold(&mut c);
    }
    let t2 = admit(&mut c, A, 122, put_dep("z", "1"));
    for _ in 0..20 {
        if asked(&c, A, from).len() >= 2 {
            break;
        }
        c.advance(100);
        hold(&mut c);
    }
    let early = asked(&c, A, from);
    assert_eq!(early.len(), 2, "two batches held behind the entry in flight");
    for w in &early {
        answer(&mut c, A, w, |_| Kv::default());
    }
    c.on(A, |m, now| m.transfer(now, B)).expect("started");
    let from = c.deps.len();
    let lease = graftyard_raft::LEASE.as_millis() as u64;
    c.run(lease + 1_000, 100, |e| !matches!(e.msg, Message::TimeoutNow { .. }));
    assert!(c.m(A).transferring().is_none(), "the transfer was not abandoned");
    for _ in 0..6 {
        for w in asked(&c, A, from) {
            answer(&mut c, A, &w, |_| revoked());
        }
        let from2 = c.deps.len();
        flush(&mut c);
        for w in asked(&c, A, from2) {
            answer(&mut c, A, &w, |_| revoked());
        }
        if result(&c, t1).is_some() && result(&c, t2).is_some() {
            break;
        }
    }
    for t in [t1, t2] {
        assert!(
            matches!(result(&c, t), Some(Err(CommitError::Refused { err: KvErr::Revoked, .. }))),
            "{t:?} decided on a read from before the transfer: {:?}",
            result(&c, t)
        );
    }
}
