//! The lifeboat's importer (`graftyard_raft::import`): a learner fed only committed rows, never by a leader, builds the
//! group's own fold, and halts rather than skip on a row it cannot take.

mod common;

use common::cluster::Cluster;
use common::kv::{FUTURE_FORMAT, KV_FORMAT, Kv, KvCmd, KvEvent};
use common::sim::{CHAIN, SITES, config};
use graftyard_raft::import::{Halt, IMPORTER, Importer};
use graftyard_raft::{
    GroupFold, Index, InstallSource, LogEntry, Member, Message, RequestId, Role, Stored, Ulid, Verdict, bootstrap,
    codec, digest,
};
use graftyard_types::{SiteId, Timestamp, learner_addr};

const A: SiteId = SITES[0];
const B: SiteId = SITES[1];

fn learner_cfg() -> graftyard_raft::MemberConfig {
    let mut cfg = config(A);
    cfg.me = learner_addr(15);
    cfg
}

/// The lifeboat's member and its storage.
struct Copy {
    m: Member<KvCmd>,
    disk: Stored<Kv>,
    imp: Importer,
}

impl Copy {
    fn new() -> Copy {
        Copy { m: Member::new(learner_cfg(), 99).unwrap(), disk: Stored::new(Kv::default()), imp: Importer::new() }
    }

    /// Feeds `rows`, persists what the member did, and returns the halt, if any. Nothing the member sends leaves: it
    /// has no peer but the importer.
    fn feed(&mut self, now: Timestamp, rows: &[Vec<u8>]) -> Option<Halt> {
        let fed = self.imp.feed(&mut self.m, now, rows);
        if let Some(r) = fed.ready {
            let (_, out) = r.persist(&mut self.disk);
            assert!(out.messages.iter().chain(&out.early).all(|e| e.to == IMPORTER), "{:?}", out.messages);
        }
        fed.halt
    }
}

fn put(c: &mut Cluster, l: SiteId, id: u128, k: &str, v: &str) {
    c.on(l, |m, now| m.commit(now, RequestId(Ulid(id)), KvCmd::Put { key: k.into(), value: v.into() })).unwrap();
    c.deliver_all();
}

fn committed(c: &Cluster, s: SiteId) -> Vec<Vec<u8>> {
    let m = c.m(s);
    m.log().iter().filter(|e| e.index <= m.state().commit).map(codec::to_bytes).collect()
}

fn group(seed: u64, n: u128) -> Cluster {
    let mut c = Cluster::new(seed);
    c.elect(A);
    for k in 0..n {
        put(&mut c, A, 10 + k, &format!("k{}", k % 7), &k.to_string());
    }
    c.sync(A);
    c
}

#[test]
fn a_learner_fed_committed_pages_matches_the_group_and_never_leads() {
    let c = group(5, 30);
    let rows = committed(&c, A);
    let mut l = Copy::new();
    // In pages, overlapping by one row as a retried page would.
    for (i, page) in rows.chunks(4).enumerate() {
        let from = if i == 0 { 0 } else { i * 4 - 1 };
        assert_eq!(l.feed(c.now, &rows[from..from + page.len() + usize::from(i > 0)]), None);
    }
    assert_eq!(l.m.state().commit, c.m(A).state().commit);
    assert_eq!(l.m.group_fold().fold, c.m(A).group_fold().fold);
    assert_eq!(l.imp.next(), Index(c.m(A).state().commit.0 + 1));
    let (v, _) = l.m.campaign(c.now).persist(&mut l.disk);
    assert_eq!(v, Verdict::MayNotLead);
    assert_eq!(l.m.role(), Role::Follower);
}

/// A lifeboat member never asks its "leader" for a read index: the importer is no member, so a read-index read there
/// would wait out CAMPAIGN_DEADLINE. Its reads are the host's stale reads; this pins that no tick sends one.
#[test]
fn a_fed_learner_sends_nothing_but_answers_to_the_importer() {
    let c = group(11, 8);
    let rows = committed(&c, A);
    let mut l = Copy::new();
    assert_eq!(l.feed(c.now, &rows), None);
    for k in 1..=40 {
        let (_, out) = l.m.tick(Timestamp(c.now.0 + k * 250)).persist(&mut l.disk);
        assert!(out.messages.is_empty() && out.early.is_empty(), "a tick sent {:?}", out.messages);
    }
}

#[test]
fn a_restarted_copy_resumes_across_a_leader_change() {
    let mut c = group(6, 10);
    c.crash(A);
    c.elect_after_promises(B);
    for k in 0..10u128 {
        put(&mut c, B, 200 + k, &format!("b{k}"), &k.to_string());
    }
    c.sync(B);
    let rows = committed(&c, B);
    assert!(c.m(B).log().iter().map(|e| e.term).collect::<std::collections::BTreeSet<_>>().len() >= 2);
    let mut l = Copy::new();
    let half = rows.len() / 2;
    assert_eq!(l.feed(c.now, &rows[..half]), None);
    let (m, _) = Member::<KvCmd>::restore(learner_cfg(), l.disk.clone(), Timestamp(c.now.0 + 50), 7).unwrap();
    l.m = m;
    l.imp = Importer::resume(&l.m);
    assert_eq!(l.imp.next(), Index(half as u64 + 1));
    assert_eq!(l.feed(Timestamp(c.now.0 + 100), &rows[half..]), None);
    assert_eq!(l.m.group_fold().fold, c.m(B).group_fold().fold);
}

/// A row in a format this release does not read halts the copy at that row, on every retry, with the rows before it
/// committed.
#[test]
fn an_unreadable_row_halts_the_copy_at_its_index() {
    let mut c = Cluster::new(8);
    for s in SITES {
        let mut cfg = config(s);
        cfg.reads = vec![KV_FORMAT, FUTURE_FORMAT];
        c.crash(s);
        c.restart_with(s, cfg);
    }
    c.elect(A);
    c.sync(A);
    c.on(A, |m, now| m.commit(now, RequestId(Ulid(900)), KvCmd::Future)).unwrap();
    c.deliver_all();
    put(&mut c, A, 901, "after", "1");
    c.sync(A);
    let rows = committed(&c, A);
    let fut = c.m(A).log().iter().find(|e| e.format == FUTURE_FORMAT).unwrap().index;
    let mut l = Copy::new();
    for _ in 0..2 {
        let h = l.feed(c.now, &rows);
        assert!(matches!(&h, Some(Halt::Refused(r)) if r.index == fut), "{h:?}");
        assert_eq!(l.m.state().commit, Index(fut.0 - 1));
        assert_eq!(l.imp.next(), fut);
    }
}

/// The importer's own check: a row whose `prev` does not chain halts the copy before the member sees it, both above
/// the member's commit and at or below it, where the member itself only `debug_assert`s.
#[test]
fn a_row_that_does_not_chain_halts_the_copy() {
    let c = group(12, 12);
    let rows = committed(&c, A);
    let mut l = Copy::new();
    assert_eq!(l.feed(c.now, &rows[..6]), None);
    let commit = l.m.state().commit;
    // A different row 6 (at the member's commit), as a source that diverged would serve it.
    let mut forged: LogEntry<KvEvent> = codec::from_bytes(&rows[5]).unwrap();
    forged.at = Timestamp(forged.at.0 + 1);
    let mut page = vec![codec::to_bytes(&forged)];
    page.extend_from_slice(&rows[6..]);
    let h = l.feed(c.now, &page);
    assert!(matches!(&h, Some(Halt::Chain { index, .. }) if *index == commit), "{h:?}");
    // A row 7 that names another row 6.
    let mut seven: LogEntry<KvEvent> = codec::from_bytes(&rows[6]).unwrap();
    seven.prev = digest(&forged);
    let h = l.feed(c.now, &[codec::to_bytes(&seven)]);
    assert!(matches!(&h, Some(Halt::Chain { index: Index(7), .. })), "{h:?}");
    // A gap.
    let h = l.feed(c.now, &rows[7..]);
    assert!(matches!(&h, Some(Halt::Chain { index: Index(8), .. })), "{h:?}");
    assert_eq!(l.m.state().commit, commit, "nothing past the halt was stored");
    assert_eq!(l.feed(c.now, &rows[6..]), None, "the real rows still go on");
    assert_eq!(l.m.group_fold().fold, c.m(A).group_fold().fold);
}

/// An archived range comes as an install from the importer, and rows above `through` go on from it.
#[test]
fn an_install_then_rows_builds_the_group_fold() {
    let c = group(9, 20);
    let m = c.m(A);
    let through = Index(12);
    let te = m.entry_at(through).unwrap().clone();
    let mut l = Copy::new();
    let src = InstallSource {
        core: "snapshot/core".into(),
        metadata: None,
        archives: Default::default(),
        through,
        through_term: te.term,
        through_digest: digest(&te),
        config: m.group_fold().config().cloned().unwrap(),
        id_block: None,
    };
    let (_, out) = l.imp.install(&mut l.m, c.now, src).persist(&mut l.disk);
    let wanted = out.install.into_iter().next().expect("the learner asks for the install");
    let mut fold = GroupFold::new(Kv::default());
    for e in m.log().iter().filter(|e| e.index <= through) {
        fold.apply(e);
    }
    let (r, out) = l.m.install(c.now, wanted, fold.clone()).persist(&mut l.disk);
    r.unwrap();
    l.disk.fold = fold;
    assert!(out.messages.iter().any(|e| matches!(&e.msg, Message::AppendRes(x) if x.ok && x.matched == through)));
    let rows: Vec<Vec<u8>> = committed(&c, A).split_off(through.0 as usize);
    assert_eq!(l.feed(c.now, &rows), None);
    assert_eq!(l.m.group_fold().fold, m.group_fold().fold);
}

/// No `ExportObject` carries a chain's `PlacementRecord`, which the member's config needs: `record_of` rebuilds it
/// from entry 1 so that `bootstrap` of it is entry 1's bytes, and `verify` accepts entry 1. A row that is not a
/// bootstrap config rebuilds nothing.
#[test]
fn a_record_rebuilt_from_entry_one_gives_the_same_bootstrap() {
    let c = group(10, 2);
    let rows = committed(&c, A);
    let rec = graftyard_raft::import::record_of::<KvEvent>(CHAIN, &rows[0]).expect("entry 1 rebuilds its record");
    assert_eq!(codec::to_bytes(&bootstrap::<KvEvent>(CHAIN, &rec).unwrap()), rows[0]);
    assert_eq!(graftyard_raft::import::record_of::<KvEvent>(CHAIN, &rows[1]), None, "entry 2 is no bootstrap");
}
