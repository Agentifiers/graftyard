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
    /// A virtual clock in ms, and the shard's promotion rate in bytes/s (0: free): each `/promote` advances the clock
    /// by the pack bytes it names over the rate, as the shard's pack-objects and validate of them take.
    clock: std::cell::Cell<f64>,
    rate: std::cell::Cell<f64>,
    sizes: RefCell<std::collections::HashMap<Digest, u64>>,
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
            clock: Default::default(),
            rate: Default::default(),
            sizes: Default::default(),
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
        let sum = |after: u64| self.stored(l).iter().filter(|e| e.index.0 > after).map(|e| row(e).len() as u64).sum();
        let total = sum(fold.archived().map_or(0, |a| a.to.0));
        // As the site reads it: each row's length, and its bytes when it is short.
        let after = fold.promoted().map_or(0, |p| p.to.0);
        let rows = self.stored(l).into_iter().filter(|e| e.index.0 > after).map(|e| {
            let r = row(&e);
            let len = r.len() as u64;
            (len, (len < log_archive::SHORT_ROW).then_some(r))
        });
        let unpromoted = log_archive::pack_bytes_above(rows, u64::MAX);
        Some(View::of(ChainId::Repo(REPO), fold, m.state().commit, total, unpromoted))
    }

    fn rows(&self, after: Index) -> Result<log_archive::Rows<'_>, String> {
        let rows: Vec<_> =
            self.stored(self.leader()).iter().filter(|e| e.index > after).map(|e| (e.index, row(e))).collect();
        Ok(Box::new(rows.into_iter()))
    }

    async fn promote(&self, sums: &[Digest], _from: Index, _to: Index, _request: RequestId) -> Result<PackRef, String> {
        self.promoted.borrow_mut().push(sums.to_vec());
        if self.rate.get() > 0.0 {
            let bytes: u64 = sums.iter().map(|s| self.sizes.borrow().get(s).copied().unwrap_or(0)).sum();
            self.clock.set(self.clock.get() + bytes as f64 * 1_000.0 / self.rate.get());
        }
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

/// Promotions are cut so their packs fit the cap, and an entry whose pack alone passes it promotes alone, so the
/// archive passes it. Refused, B0-34's drain stopped for good at a 53 MB push over `PROMOTE_MAX`
/// (`tests/am1/results/lm1-memory.md`, round 20).
#[test]
fn promotions_fit_the_cap_and_an_entry_over_it_promotes_alone() {
    let b = Bed::new();
    for k in 0..80 {
        push(&b, k, 1_500);
    }
    snapshot(&b);
    let mut steps = Vec::new();
    loop {
        match block_on(log_archive::step(&b, CAP, CAP, 5_000)).expect("no entry is refused") {
            Step::Idle => break,
            s => steps.push(s),
        }
        b.truncate();
        assert!(steps.len() < 200, "{steps:?}");
    }
    let w = b.w.borrow();
    let all = &w.repo.disks[0].log;
    let packs = |from: Index, to: Index| -> Vec<(Index, usize)> {
        all.iter()
            .filter(|e| e.index >= from && e.index <= to)
            .flat_map(|e| match &e.body {
                graftyard_raft::EntryBody::Events(evs) => graftyard_git::repo::packs_of(evs)
                    .into_iter()
                    .map(|p| match p {
                        PackRef::Inline(b) => (e.index, b.len()),
                        PackRef::Stored { bytes, .. } => (e.index, bytes as usize),
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .collect()
    };
    let promotions: Vec<(Index, Index)> = steps
        .iter()
        .filter_map(|s| match s {
            Step::Promoted { from, to, .. } => Some((*from, *to)),
            _ => None,
        })
        .collect();
    // Each promotion fits the cap, or is one entry whose pack alone passes it: every stored push (9,000 bytes).
    let mut alone = 0;
    for (from, to) in &promotions {
        let p = packs(*from, *to);
        let bytes: usize = p.iter().map(|(_, n)| n).sum();
        if bytes > 5_000 {
            assert!(p.iter().all(|(i, _)| *i == p[0].0), "{from:?}..{to:?} holds {p:?} over the cap");
            assert_eq!(p.first().map(|(i, _)| *i), Some(*from), "the entry over the cap starts its promotion");
            alone += 1;
        }
    }
    // Pushes 1, 6, ..., 76 are stored: 16 of them, each promoted alone.
    assert_eq!(alone, 16, "{promotions:?}");
    // The archive passes the first of them.
    let first = packs(Index(1), Index(u64::MAX)).into_iter().find(|(_, n)| *n > 5_000).unwrap().0;
    assert!(steps.iter().any(|s| matches!(s, Step::Archived { to, .. } if *to > first)), "{steps:?}");
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

/// Promotion runs ahead of archiving: with the log far under its cap, so no range is due, the leader still promotes
/// once the log above the last `Promoted` passes `cap / PROMOTE_AHEAD`, so a git cache shard's rehydrate, which reads
/// every entry above the last promotion, never faces more than that and one promotion's range. B0-30 promoted only
/// what was due to archive, and the tail above the last promotion stood near the whole 2 GiB cap (blockers 9 to 11).
/// Each promotion takes several archive ranges' worth of log, with no gap.
#[test]
fn promotion_runs_ahead_of_archiving_and_bounds_the_tail_a_rehydrate_reads() {
    let b = Bed::new();
    let cap = 20 * CAP;
    let range_max = CAP / 10;
    let ahead = cap / log_archive::PROMOTE_AHEAD;
    let tail = |b: &Bed| b.w.borrow().repo.members[b.leader()].group_fold().fold.promoted().map_or(0, |p| p.to.0);
    let unpromoted = |b: &Bed| {
        let after = tail(b);
        b.stored(b.leader()).iter().filter(|e| e.index.0 > after).map(|e| row(e).len() as u64).sum::<u64>()
    };
    // Before both tiers have a snapshot nothing runs ahead, as nothing archives.
    for k in 0..30 {
        push(&b, k, 1_500);
    }
    assert!(unpromoted(&b) > ahead);
    let s = block_on(log_archive::step(&b, cap, range_max, log_archive::PROMOTE_MAX)).unwrap();
    assert_eq!(s, Step::Idle, "no snapshot yet");
    snapshot(&b);
    let (mut promotions, mut worst, mut largest) = (Vec::new(), 0u64, 0u64);
    for k in 30..400 {
        push(&b, k, 300 + (k as usize * 37) % 2_000);
        for _ in 0..20 {
            match block_on(log_archive::step(&b, cap, range_max, log_archive::PROMOTE_MAX)).unwrap() {
                Step::Idle => break,
                Step::Promoted { from, to, .. } => promotions.push((from.0, to.0)),
                Step::Archived { .. } => panic!("nothing is due under a cap of {cap}"),
            }
        }
        worst = worst.max(unpromoted(&b));
        largest = largest.max(b.stored(b.leader()).iter().map(|e| row(e).len() as u64).max().unwrap_or(0));
    }
    assert!(b.bytes(b.leader()) < cap, "the log stays under its cap, so every promotion ran ahead");
    assert!(worst <= ahead + 2 * largest, "the tail above the last promotion reached {worst} bytes against {ahead}");
    assert!(promotions.len() >= 3, "{promotions:?}");
    assert_eq!(promotions[0].0, 1);
    assert!(promotions.windows(2).all(|p| p[1].0 == p[0].1 + 1), "{promotions:?}");
    // A promotion takes more than one archive range of log when that much is waiting: the first one, which starts
    // with the whole pre-snapshot backlog above the bound.
    let all = b.w.borrow().repo.disks[0].log.clone();
    let first: u64 = all
        .iter()
        .filter(|e| e.index.0 >= promotions[0].0 && e.index.0 <= promotions[0].1)
        .map(|e| row(e).len() as u64)
        .sum();
    assert!(first > range_max, "the first promotion took {first} bytes of log, one range is {range_max}");
    assert!(first <= range_max * log_archive::PROMOTE_RANGES + largest);
}

/// A pushed pack of `bytes` stored in the buckets: its entry's row is a pointer of about a hundred bytes.
fn push_stored(b: &Bed, k: u32, bytes: u64) {
    b.sizes.borrow_mut().insert(Digest([k as u8; 32]), bytes);
    let ev = RepoEvent::Refs {
        push: RequestId(Ulid(u128::from(k) << 32 | 7)),
        actor: None,
        updates: vec![up(&format!("refs/heads/s{k}"), None, Some(oid(k)))],
        pack: Some(PackRef::Stored {
            id: PackId(Ulid(u128::from(k) << 64 | 1)),
            checksum: Digest([k as u8; 32]),
            bytes,
            sites: SiteSet(0b111),
        }),
    };
    b.w.borrow_mut().record(vec![ev]);
}

/// Promote-ahead counts the pack bytes above the last promotion, stored packs included, not the log's row bytes. A
/// network made only of stored pushes (B0-34's seed, repo:131296: 257 packs of about 10 MB, 2.62 GB, in a 294-entry
/// log) never passed `cap / PROMOTE_AHEAD` of rows, so it never promoted, and every cold rehydrate re-indexed every
/// pushed pack (`tests/am1/results/lm1-rows-m3f.md`, "Stored packs and their index"). Here 40 stored pushes of a
/// fifth of the bound each, with the log's rows far under it, promote, in ranges that each fit `PROMOTE_MAX`.
#[test]
fn stored_pushes_alone_promote_ahead_once_their_pack_bytes_pass_the_bound() {
    let b = Bed::new();
    let cap = 1_000 * CAP;
    let ahead = cap / log_archive::PROMOTE_AHEAD;
    snapshot(&b);
    let pack = ahead / 5;
    let mut promotions = Vec::new();
    for k in 0..40 {
        push_stored(&b, k, pack);
        for _ in 0..20 {
            match block_on(log_archive::step(&b, cap, CAP, log_archive::PROMOTE_MAX)).unwrap() {
                Step::Idle => break,
                Step::Promoted { from, to, .. } => promotions.push((from.0, to.0)),
                Step::Archived { .. } => panic!("nothing is due under a cap of {cap}"),
            }
        }
        let unpromoted = b.view().unwrap().unpromoted;
        assert!(unpromoted <= ahead + pack, "push {k}: {unpromoted} pack bytes above the last promotion");
    }
    assert!(b.bytes(b.leader()) < ahead / 100, "the rows alone stay far under the bound");
    assert!(promotions.len() >= 5, "{promotions:?}");
    assert!(promotions.windows(2).all(|p| p[1].0 == p[0].1 + 1), "{promotions:?}");
    // Each promotion names the stored packs of its range.
    assert!(b.promoted.borrow().iter().all(|s| !s.is_empty()));
}

/// The promote-ahead trigger against push rates, on a virtual clock: stored pushes arrive at a rate, and each
/// promotion takes its pack bytes at the shard's measured promotion rate. The rate is an estimate from the hosted
/// index-pack rate of thin pushed packs, 1.4 to 1.55 MB/s (repo:131296 on m29, `tests/am1/results/lm1-rows-m3f.md`,
/// "Stored packs and their index"), since a promotion's validate indexes the same bytes; it is not yet measured hosted.
/// Below the rate the tail above the last promotion stays within the bound plus one promotion. Above it, as B0-31's
/// agents at about 4 MB/s are, the tail grows at the difference, and the trigger stays bounded: every promotion takes
/// at most `PROMOTE_MAX` of packs, so one step holds the archive run at most `PROMOTE_MAX` over the rate, under its
/// `STEP_FOR`. That growth is the `DECISIONS.md` flip to shard-streamed promotion ("Promote-ahead counts pack bytes").
#[test]
fn promotion_keeps_up_below_its_rate_and_stays_bounded_above_it() {
    // Measured on git/git's first-parent history, 30 thin pushes of 100 commits (29.2 MB) promoted at once, on 4 CPUs:
    // `pack-objects` 4.65 s and the validate's threaded `index-pack` 6.50 s, so 2.62 MB/s of pushed bytes
    // (`tests/am1/results/lm1-rows-m3f.md`, "Promotion rate"). Above it the tail grows; B0-31's agents staged about 4.
    const RATE: f64 = 2.62e6;
    const PACK: u64 = 1 << 20;
    let mut out = Vec::new();
    for push_rate in [1.0e6, 2.0e6, 4.0e6, 6.0e6] {
        let b = Bed::new();
        b.rate.set(RATE);
        let cap = 4_000 * CAP;
        let ahead = cap / log_archive::PROMOTE_AHEAD;
        snapshot(&b);
        let gap_ms = PACK as f64 * 1_000.0 / push_rate;
        let (mut next_push, mut k, mut worst_step_ms, mut tail_end) = (0.0f64, 0u32, 0.0f64, 0u64);
        let end_ms = 600_000.0;
        while b.clock.get() < end_ms {
            // Every push due by now, then one alarm step.
            while next_push <= b.clock.get() {
                push_stored(&b, k, PACK);
                k += 1;
                next_push += gap_ms;
            }
            let t0 = b.clock.get();
            match block_on(log_archive::step(&b, cap, CAP, log_archive::PROMOTE_MAX)).unwrap() {
                Step::Idle => b.clock.set(next_push),
                Step::Promoted { .. } => worst_step_ms = worst_step_ms.max(b.clock.get() - t0),
                Step::Archived { .. } => panic!("nothing is due under a cap of {cap}"),
            }
            tail_end = b.view().unwrap().unpromoted;
        }
        let one = log_archive::PROMOTE_MAX as f64 * 1_000.0 / RATE;
        assert!(worst_step_ms <= one + 1.0, "{push_rate}: a promotion step took {worst_step_ms} ms");
        if push_rate < RATE {
            assert!(
                tail_end <= ahead + log_archive::PROMOTE_MAX + PACK,
                "{push_rate} B/s: {tail_end} pack bytes above the last promotion, bound {ahead}"
            );
        } else {
            // Behind by about the difference over the run, less what the bound absorbs.
            let behind = (push_rate - RATE) * end_ms / 1_000.0;
            assert!(tail_end as f64 > behind * 0.5, "{push_rate} B/s: {tail_end} above, expected about {behind}");
        }
        out.push((push_rate / 1e6, tail_end / 1_000_000, worst_step_ms as u64));
    }
    println!(
        "promotion at {RATE} B/s over 600 s: (push MB/s, MB above the last promotion at the end, longest step ms) {out:?}"
    );
}
