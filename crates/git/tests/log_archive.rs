//! The hosted archive alarm's logic (`graftyard_git::log_archive`) in memory, on the real members of `common`: three
//! `graftyard_raft::Member<RepoCmd>` of `repo:7`, with the git cache shard's `/promote` and three buckets stubbed in
//! memory. B0-30's rule at a scaled cap (`build/BUILD-PLAN.md`, M0-WP9, and AM1-WP1's `LOG_MAX_BYTES`):
//!
//! - nothing is archived before both tiers have a snapshot;
//! - each due range is promoted first, its `Promoted` naming every pack its entries name (an entry with none still
//!   promotes, as an empty pack), and then archived in two buckets up to the last promotion, in pieces of at most the
//!   range cap, every `Promoted` and `Archived` verified by both followers;
//! - members truncate only through ranges both buckets hold, and no stored log passes the cap by more than the
//!   entries one alarm adds;
//! - every archived entry reads back from the objects, byte for byte, with its digest chain, and by time, as
//!   `RestoredTo` reads it;
//! - a re-sent step after a lost answer records nothing twice.

mod common;
use common::*;

use std::cell::RefCell;

use futures_executor::block_on;
use graftyard_fold::{ARCHIVE_HEADER, check_archive_header, decode_archive_some};
use graftyard_git::log_archive::{self, ArchiveHost, Next, Step, View};
use graftyard_git::mem::MemBucket;
use graftyard_git::{Bucket, PackId, Promoted, PutNew, R2Key};
use graftyard_raft::{LogEntry, codec};
use graftyard_types::{Index, SiteSet, Tier};
use sha2::{Digest as _, Sha256};

const CAP: u64 = 40_000;

struct Bed {
    w: RefCell<World>,
    buckets: [MemBucket; 3],
    /// Each member's lowest stored index: it keeps the row at an archive's `to` and drops those below.
    held_from: RefCell<[u64; 3]>,
    /// Every `/promote` call: its checksums.
    promoted: RefCell<Vec<Vec<Digest>>>,
    marks: RefCell<usize>,
    /// Planted faults: buckets an archive put reaches.
    put_to: usize,
}

fn row(e: &LogEntry<RepoEvent>) -> Vec<u8> {
    codec::to_bytes(e)
}

impl Bed {
    fn new() -> Bed {
        Bed {
            w: RefCell::new(World::new()),
            buckets: Default::default(),
            held_from: RefCell::new([1; 3]),
            promoted: RefCell::new(Vec::new()),
            marks: RefCell::new(0),
            put_to: 2,
        }
    }

    fn leader(&self) -> usize {
        self.w.borrow().repo.leader().0 as usize
    }

    fn stored(&self, m: usize) -> Vec<LogEntry<RepoEvent>> {
        let from = self.held_from.borrow()[m];
        self.w.borrow().repo.disks[m].log.iter().filter(|e| e.index.0 >= from).cloned().collect()
    }

    /// Each member truncates through the newest archive once both its keys' buckets hold it.
    fn truncate(&self) {
        let w = self.w.borrow();
        for (m, member) in w.repo.members.iter().enumerate().take(3) {
            let f = &member.group_fold().fold;
            let Some(a) = f.archived() else { continue };
            let key = a.key.to_string();
            let held = self.buckets.iter().filter(|b| block_on(b.head(&key)).unwrap().is_some()).count();
            if held >= 2 {
                let h = &mut self.held_from.borrow_mut()[m];
                *h = (*h).max(a.to.0);
            }
        }
    }

    fn bytes(&self, m: usize) -> u64 {
        self.stored(m).iter().map(|e| row(e).len() as u64).sum()
    }

    fn step(&self, range_max: u64) -> Step {
        let s = block_on(log_archive::step(self, CAP, range_max, log_archive::PROMOTE_MAX)).expect("an archive step");
        self.truncate();
        s
    }
}

impl ArchiveHost for Bed {
    fn view(&self) -> Option<View> {
        let l = self.leader();
        let w = self.w.borrow();
        let m = &w.repo.members[l];
        let fold = &m.group_fold().fold;
        let after = fold.archived().map_or(0, |a| a.to.0);
        let total = self.stored(l).iter().filter(|e| e.index.0 > after).map(|e| row(e).len() as u64).sum();
        Some(View::of(ChainId::Repo(REPO), fold, m.state().commit, total))
    }

    fn rows(&self, after: Index) -> Result<log_archive::Rows<'_>, String> {
        let rows: Vec<_> =
            self.stored(self.leader()).iter().filter(|e| e.index > after).map(|e| (e.index, row(e))).collect();
        Ok(Box::new(rows.into_iter()))
    }

    async fn promote(&self, sums: &[Digest], _request: RequestId) -> Result<PackRef, String> {
        self.promoted.borrow_mut().push(sums.to_vec());
        // The shard's complete pack of those objects; here, a stand-in body naming them.
        let mut bytes = b"PACK".to_vec();
        for s in sums {
            bytes.extend_from_slice(&s.0);
        }
        let checksum = Digest(Sha256::digest(&bytes).into());
        let n = self.promoted.borrow().len() as u128;
        Ok(PackRef::Stored { id: PackId(Ulid(n << 80)), checksum, bytes: bytes.len() as u64, sites: SiteSet(0b11) })
    }

    async fn mark(&self, _pack: &PackRef, _index: Index) {
        *self.marks.borrow_mut() += 1;
    }

    async fn put(&self, key: &str, bytes: &[u8], digest: Digest) -> Result<usize, String> {
        let mut held = 0;
        for b in self.buckets.iter().take(self.put_to) {
            match b.put_new(key, bytes, digest, &[]).await.map_err(|e| e.0)? {
                PutNew::Created => held += 1,
                PutNew::Exists(Some(d)) if d == digest => held += 1,
                PutNew::Exists(_) => {}
            }
        }
        Ok(held)
    }

    async fn record(&self, request: RequestId, events: Vec<RepoEvent>) -> Result<Index, String> {
        let mut w = self.w.borrow_mut();
        let l = w.repo.leader();
        let now = w.now;
        w.repo.on(l, now, |m, t| m.record(t, request, REPO_FORMAT_V1, events)).map_err(|e| format!("{e:?}"))?;
        w.settle();
        w.repo
            .resolutions
            .iter()
            .rev()
            .find_map(|r| match r {
                Resolution::Recorded { id, result } if *id == request => Some(result.clone()),
                _ => None,
            })
            .ok_or("unresolved")?
            .map_err(|e| format!("{e:?}"))
    }
}

/// A push entry: a fresh ref with an inline pack of `n` bytes, or a stored pack, or none.
fn push(b: &Bed, k: u32, n: usize) {
    let pack = match k % 5 {
        0 => None,
        1 => Some(PackRef::Stored {
            id: PackId(Ulid(u128::from(k) << 64)),
            checksum: Digest([k as u8; 32]),
            bytes: 9_000,
            sites: SiteSet(0b111),
        }),
        _ => Some(PackRef::Inline(vec![k as u8; n])),
    };
    let ev = RepoEvent::Refs {
        push: RequestId(Ulid(u128::from(k) << 32 | 9)),
        actor: None,
        updates: vec![up(&format!("refs/heads/b{k}"), None, Some(oid(k)))],
        pack,
    };
    b.w.borrow_mut().record(vec![ev]);
}

/// Both tiers' snapshots at the leader's applied index, as the snapshot writer commits them.
fn snapshot(b: &Bed) {
    snapshot_of(b, &[Tier::Core, Tier::Metadata]);
}

/// `tiers`' snapshots at the leader's applied index.
fn snapshot_of(b: &Bed, tiers: &[Tier]) {
    let (to, at) = {
        let w = b.w.borrow();
        let m = &w.repo.members[b.leader()];
        let e = m.entry_at(m.state().applied).expect("in memory").clone();
        (e.index, e.at)
    };
    let evs = tiers
        .iter()
        .map(|&tier| {
            let digest = Digest([tier as u8 + 1; 32]);
            let key = R2Key::Snapshot { chain: ChainId::Repo(REPO), tier, from: Index(0), to, digest };
            RepoEvent::Archived(graftyard_git::Archived { key, from: Index(0), to, from_at: at, to_at: at, digest })
        })
        .collect();
    b.w.borrow_mut().record(evs);
}

/// The entries of every archive object, read from the second bucket.
fn read_back(b: &Bed, keys: &[String]) -> Vec<(Vec<u8>, LogEntry<RepoEvent>)> {
    let mut out = Vec::new();
    for k in keys {
        let o = block_on(b.buckets[1].get(k)).unwrap().expect("the second copy");
        check_archive_header(&o).unwrap();
        let (rows, used, _) = decode_archive_some(&o[ARCHIVE_HEADER..]);
        assert_eq!(ARCHIVE_HEADER + used, o.len(), "{k}: whole rows only");
        for r in rows {
            out.push((r.to_vec(), codec::from_bytes(r).unwrap()));
        }
    }
    out
}

#[test]
fn the_archive_alarm_promotes_then_archives_to_two_buckets_and_holds_every_member_under_the_cap() {
    let b = Bed::new();
    let range_max = CAP / 5;
    // Under the cap and with no snapshot, nothing; over the cap with no snapshot, still nothing.
    for k in 0..60 {
        push(&b, k, 1_500);
        assert_eq!(b.step(range_max), Step::Idle, "entry {k}: no snapshot yet");
    }
    assert!(b.bytes(b.leader()) > CAP);
    snapshot(&b);
    let (mut promotions, mut archives, mut peak, mut largest) = (Vec::new(), Vec::new(), 0u64, 0u64);
    for k in 60..400 {
        push(&b, k, 300 + (k as usize * 37) % 2_000);
        if k % 80 == 0 {
            snapshot(&b);
        }
        // The alarm runs until it has nothing to do, as the site re-arms it after each step.
        for _ in 0..20 {
            match b.step(range_max) {
                Step::Idle => break,
                Step::Promoted { from, to, .. } => promotions.push((from.0, to.0)),
                Step::Archived { from, to, key, .. } => archives.push((from.0, to.0, key)),
            }
        }
        for m in 0..3 {
            peak = peak.max(b.bytes(m));
            largest = largest.max(b.stored(m).iter().map(|e| row(e).len() as u64).max().unwrap_or(0));
        }
    }
    // One more entry carries the commit index of the last `Archived` to the followers.
    push(&b, 400, 10);
    b.truncate();
    let w = b.w.borrow();
    let lead = &w.repo.members[b.leader()];
    println!(
        "archive alarm in memory: {} entries, cap {CAP} bytes, range cap {range_max}; {} promotions, {} archives; \
         peak stored log {peak} bytes (largest entry {largest}); {} marker puts",
        lead.last_index().0,
        promotions.len(),
        archives.len(),
        b.marks.borrow()
    );
    assert!(archives.len() >= 10 && promotions.len() >= 3, "{promotions:?} {archives:?}");
    // No member's stored log passes the cap by more than what one alarm's run adds: the entry just written and the
    // `Promoted` and `Archived` entries before the truncation catches up.
    assert!(peak <= CAP + 3 * largest, "a stored log reached {peak} bytes against {CAP}");
    // Ranges are contiguous from 1, each piece at most the range cap (or one entry), and each archive lies inside the
    // promotions so far.
    assert_eq!(archives[0].0, 1);
    assert!(archives.windows(2).all(|a| a[1].0 == a[0].1 + 1));
    assert!(promotions.windows(2).all(|p| p[1].0 == p[0].1 + 1));
    assert!(archives.iter().all(|a| a.1 <= promotions.last().unwrap().1));
    // Every member agrees on the last archive and promotion, and every member truncated to the last archive.
    let f0 = w.repo.members[0].group_fold().fold.archived();
    for (m, member) in w.repo.members.iter().enumerate().take(3) {
        assert_eq!(member.group_fold().fold.archived(), f0);
        assert_eq!(b.held_from.borrow()[m], archives.last().unwrap().1);
    }
    // Each promotion named every pack its range's entries name, once each.
    let all: Vec<LogEntry<RepoEvent>> = w.repo.disks[0].log.clone();
    for ((from, to), sums) in promotions.iter().zip(b.promoted.borrow().iter()) {
        let range: Vec<_> = all.iter().filter(|e| e.index.0 >= *from && e.index.0 <= *to).cloned().collect();
        assert_eq!(*sums, log_archive::promotion_sums(&range));
    }
    assert!(b.promoted.borrow().iter().any(|s| s.iter().any(|d| d.0 == [1; 32])), "a stored pack was promoted");
    // Every archived entry reads back from the second bucket, byte for byte with the leader's log, digests chained.
    let keys: Vec<String> = archives.iter().map(|a| a.2.clone()).collect();
    let back = read_back(&b, &keys);
    assert_eq!(back.len() as u64, archives.last().unwrap().1);
    for (r, e) in &back {
        assert_eq!(*r, row(&all[(e.index.0 - 1) as usize]), "entry {}", e.index.0);
    }
    assert!(back.windows(2).all(|p| p[1].1.prev == graftyard_raft::digest(&p[0].1)));
    // By time, as `RestoredTo` reads it: for every archived entry's time, the last entry at or before it.
    for (_, e) in &back {
        let found = back.iter().rfind(|(_, x)| x.at <= e.at).unwrap();
        let want = all.iter().rfind(|x| x.at <= e.at && x.index.0 <= archives.last().unwrap().1).unwrap();
        assert_eq!(found.1.index, want.index);
    }
    drop(w);
    // A step re-sent after its answer was lost records nothing new: the same request id finds its row.
    let (p, a) = {
        let w = b.w.borrow();
        let f = &w.repo.members[b.leader()].group_fold().fold;
        (f.promoted().unwrap(), f.archived().unwrap())
    };
    let before = b.w.borrow().repo.members[0].last_index();
    let req = log_archive::promote_request(ChainId::Repo(REPO), p.from, p.to);
    let again = block_on(b.record(req, vec![RepoEvent::Promoted(p)])).unwrap();
    let again2 = block_on(b.record(log_archive::archive_request(a.digest), vec![RepoEvent::Archived(a)])).unwrap();
    assert!(again <= before && again2 <= before);
    assert_eq!(b.w.borrow().repo.members[0].last_index(), before);
}

/// A bucket that does not take the object: the step fails, nothing is recorded, and no member truncates.
#[test]
fn an_archive_held_in_one_bucket_is_never_recorded() {
    let mut b = Bed::new();
    b.put_to = 1;
    for k in 0..80 {
        push(&b, k, 1_500);
    }
    snapshot(&b);
    assert!(matches!(b.step(CAP), Step::Promoted { .. }));
    let e = block_on(log_archive::step(&b, CAP, CAP, log_archive::PROMOTE_MAX)).unwrap_err();
    assert!(e.contains("in 1 buckets"), "{e}");
    let w = b.w.borrow();
    assert!(w.repo.members.iter().all(|m| m.group_fold().fold.archived().is_none()));
    assert_eq!(*b.held_from.borrow(), [1; 3]);
}

/// Promotions are cut so their packs fit the cap, and an entry whose pack alone passes it is refused before the
/// shard is asked: nothing is recorded for it.
#[test]
fn promotions_fit_the_cap_and_an_entry_over_it_alone_is_refused() {
    let b = Bed::new();
    for k in 0..80 {
        push(&b, k, 1_500);
    }
    snapshot(&b);
    let mut steps = Vec::new();
    let err = loop {
        match block_on(log_archive::step(&b, CAP, CAP, 5_000)) {
            Ok(s) => steps.push(s),
            Err(e) => break e,
        }
        b.truncate();
        assert!(steps.len() < 50);
    };
    // The first stored pack (9,000 bytes, push 1) is the first entry over the cap.
    assert!(err.contains("names 9000 pack bytes, over the 5000 a promotion holds"), "{err}");
    assert!(steps.iter().any(|s| matches!(s, Step::Promoted { .. })), "{steps:?}");
    let w = b.w.borrow();
    let all = &w.repo.disks[0].log;
    let p = w.repo.members[0].group_fold().fold.promoted().unwrap();
    let bytes: usize = all
        .iter()
        .filter(|e| e.index >= p.from && e.index <= p.to)
        .filter_map(|e| match &e.body {
            graftyard_raft::EntryBody::Events(evs) => Some(graftyard_git::repo::packs_of(evs)),
            _ => None,
        })
        .flatten()
        .map(|p| match p {
            PackRef::Inline(b) => b.len(),
            PackRef::Stored { bytes, .. } => bytes as usize,
        })
        .sum();
    assert!(bytes <= 5_000, "{bytes}");
}

#[test]
fn next_archives_what_the_last_promotion_covers_and_promotes_otherwise() {
    let p = |to| Promoted { from: Index(1), to: Index(to), pack: PackRef::Inline(Vec::new()) };
    assert_eq!(log_archive::next(None, Index(1), Index(9)), Next::Promote { from: Index(1), to: Index(9) });
    assert_eq!(log_archive::next(Some(&p(5)), Index(1), Index(9)), Next::Archive { from: Index(1), to: Index(5) });
    assert_eq!(log_archive::next(Some(&p(12)), Index(1), Index(9)), Next::Archive { from: Index(1), to: Index(9) });
    assert_eq!(log_archive::next(Some(&p(5)), Index(6), Index(9)), Next::Promote { from: Index(6), to: Index(9) });
}

/// `due` refuses a stored log with a gap or a row that is not the entry its index names.
#[test]
fn due_refuses_a_gap_and_a_row_of_another_entry() {
    let b = Bed::new();
    for k in 0..5 {
        push(&b, k, 100);
    }
    let rows: Vec<(Index, Vec<u8>)> = b.stored(0).iter().map(|e| (e.index, row(e))).collect();
    let now = Timestamp(u64::MAX / 2);
    let mut gap = rows.clone();
    gap.remove(2);
    assert!(log_archive::due(1, gap, now, true, Index(99), 0, u64::MAX).unwrap_err().contains("lacks entry 3"));
    let mut wrong = rows.clone();
    wrong[1].1 = wrong[2].1.clone();
    assert!(log_archive::due(1, wrong, now, true, Index(99), 0, u64::MAX).unwrap_err().contains("holds entry"));
    let d = log_archive::due(1, rows, now, true, Index(3), 0, u64::MAX).unwrap().unwrap();
    assert_eq!((d.from, d.to, d.rows.len()), (Index(1), Index(3), 3));
}

/// `ARCHIVE_SLACK`: a log one byte over the cap is cut to `cap - cap/16`, not to the cap, so one alarm archives a
/// range of entries rather than one entry each time a write crosses the cap.
#[test]
fn a_log_over_the_cap_is_cut_below_it_by_the_slack() {
    let b = Bed::new();
    for k in 0..40 {
        push(&b, k, 100);
    }
    let rows: Vec<(Index, Vec<u8>)> = b.stored(0).iter().map(|e| (e.index, row(e))).collect();
    let total: u64 = rows.iter().map(|(_, r)| r.len() as u64).sum();
    let now = Timestamp(0);
    let cap = total - 1;
    let d = log_archive::due(total, rows.clone(), now, true, Index(u64::MAX), cap, u64::MAX).unwrap().unwrap();
    let cut: u64 = d.rows.iter().map(|(_, _, r)| r.len() as u64).sum();
    assert!(total - cut <= cap - cap / log_archive::ARCHIVE_SLACK, "left {} over {}", total - cut, cap);
    assert!(d.rows.len() > 1, "one alarm archives more than the entry that crossed the cap");
    // Under the cap and inside LOG_KEEP, nothing is due.
    assert_eq!(log_archive::due(total, rows, now, true, Index(u64::MAX), total, u64::MAX).unwrap(), None);
}

/// Nothing is archived while only one tier has a snapshot: an install needs both to load (`View::of`).
#[test]
fn one_tier_s_snapshot_alone_archives_nothing() {
    let b = Bed::new();
    for k in 0..60 {
        push(&b, k, 1_500);
    }
    assert!(b.bytes(b.leader()) > CAP);
    snapshot_of(&b, &[Tier::Core]);
    assert_eq!(b.step(CAP / 5), Step::Idle, "the core tier alone");
    snapshot_of(&b, &[Tier::Metadata]);
    assert!(matches!(b.step(CAP / 5), Step::Promoted { .. }), "both tiers");
}
