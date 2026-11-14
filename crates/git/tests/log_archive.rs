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
use graftyard_git::log_archive::{self, ArchiveHost, Next, Run, Step, View};
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
    /// The drain's modeled time in ms at B0-30's sizes, once `scale` (hosted bytes per bed byte) is set.
    clock: std::cell::Cell<f64>,
    scale: Option<f64>,
    /// How often a promotion's read paused (`ArchiveHost::pause`).
    pauses: std::cell::Cell<usize>,
    /// Each stored entry's row length, by index and `prev`, so summing the log does not re-encode it.
    lens: RefCell<std::collections::HashMap<(u64, [u8; 32]), u64>>,
    /// The drain run the steps belong to, as the site keeps it (`log_archive::Run`).
    run: RefCell<Run>,
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
            clock: std::cell::Cell::new(0.0),
            scale: None,
            pauses: std::cell::Cell::new(0),
            lens: RefCell::new(std::collections::HashMap::new()),
            run: RefCell::new(Run::default()),
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
        self.sum_after(m, 0)
    }

    /// The bytes of member `m`'s stored rows above `after`.
    fn sum_after(&self, m: usize, after: u64) -> u64 {
        let (w, from, mut lens) = (self.w.borrow(), self.held_from.borrow()[m], self.lens.borrow_mut());
        let held = w.repo.disks[m].log.iter().filter(|e| e.index.0 >= from && e.index.0 > after);
        held.map(|e| *lens.entry((e.index.0, e.prev.0)).or_insert_with(|| row(e).len() as u64)).sum()
    }

    fn step(&self, range_max: u64) -> Step {
        let s = block_on(log_archive::step(self, &mut self.run.borrow_mut(), CAP, range_max, log_archive::PROMOTE_MAX))
            .expect("an archive step");
        self.truncate();
        s
    }
}

impl Bed {
    fn charge(&self, ms: f64) {
        self.clock.set(self.clock.get() + ms);
    }
}

// B0-30's rerun step costs [Workers Logs, 12:30Z to 12:49Z, `tests/am1/results/b030-split.md`, "B0-30 rerun"]: a
// promotion of about 25 entries (8 MB) answered `/archive/promote` in 1.7 to 2.5 s; an archive step of one 2 MiB
// object, its two puts one after the other, its entry and its pacing, every 2.2 s. The split is an inference: 450 ms
// a commit, paced twice that again, leaves 850 ms for two puts, so 430 ms an object with both puts at once.
const COMMIT_MS: f64 = 450.0;
const PUT_MS: f64 = 430.0;
const PROMOTE_MS: f64 = 1_000.0;
const PROMOTE_MS_PER_MB: f64 = 110.0;
/// Twice B0-30's pushes: 2.48 GB a simulated day at 18 simulated days a real day, in MB a second.
const TWICE_THE_ROW_MB_S: f64 = 2.0 * 2.48e3 / (86_400.0 / 18.0);

impl ArchiveHost for Bed {
    fn view(&self) -> Option<View> {
        let l = self.leader();
        let w = self.w.borrow();
        let m = &w.repo.members[l];
        let fold = &m.group_fold().fold;
        let (archived, promoted) = (fold.archived().map_or(0, |a| a.to.0), fold.promoted().map_or(0, |p| p.to.0));
        let view = |total, unpromoted| View::of(ChainId::Repo(REPO), fold, m.state().commit, total, unpromoted);
        let (total, unpromoted) = (self.sum_after(l, archived), self.sum_after(l, promoted));
        Some(view(total, unpromoted))
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

    /// A write lands between pieces of the read, as the member's inputs do while it pauses.
    async fn pause(&self) {
        self.pauses.set(self.pauses.get() + 1);
        let k = 900 + self.pauses.get() as u32;
        drop(self.w.try_borrow_mut().map(|mut w| {
            w.record(vec![RepoEvent::Refs {
                push: RequestId(Ulid(u128::from(k) << 32 | 9)),
                actor: None,
                updates: vec![up(&format!("refs/heads/b{k}"), None, Some(oid(k)))],
                pack: Some(PackRef::Inline(vec![1; 10])),
            }])
        }));
    }

    async fn mark(&self, _pack: &PackRef, _index: Index) {
        *self.marks.borrow_mut() += 1;
    }

    async fn put(&self, key: &str, bytes: &[u8], digest: Digest) -> Result<usize, String> {
        if self.scale.is_some() {
            self.charge(PUT_MS);
        }
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
        if let Some(k) = self.scale {
            // A promotion's shard time, by the hosted bytes of its range's log, then the entry's commit and pacing.
            if let Some(RepoEvent::Promoted(p)) = events.first() {
                let bytes: u64 = self
                    .stored(self.leader())
                    .iter()
                    .filter(|e| e.index >= p.from && e.index <= p.to)
                    .map(|e| row(e).len() as u64)
                    .sum();
                self.charge(PROMOTE_MS + PROMOTE_MS_PER_MB * bytes as f64 * k / 1e6);
            }
            self.charge(COMMIT_MS * 3.0);
        }
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
                Step::Archived { from, to, keys, .. } => archives.push((from.0, to.0, keys)),
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
    let keys: Vec<String> = archives.iter().flat_map(|a| a.2.clone()).collect();
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
    let again2 = block_on(b.record(log_archive::archive_request(&[a.digest]), vec![RepoEvent::Archived(a)])).unwrap();
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
    let e = block_on(log_archive::step(&b, &mut b.run.borrow_mut(), CAP, CAP, log_archive::PROMOTE_MAX)).unwrap_err();
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
        match block_on(log_archive::step(&b, &mut b.run.borrow_mut(), CAP, CAP, 5_000)) {
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

/// `due` cuts the log to the target it is given: a run that passed the trigger asks for the floor, the cap less
/// `cap / ARCHIVE_SLACK`, so one alarm archives a range of entries rather than one entry each time a write crosses it.
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
    let d = log_archive::due(total, rows.clone(), now, true, Index(u64::MAX), log_archive::floor(cap), u64::MAX);
    let d = d.unwrap().unwrap();
    let cut: u64 = d.rows.iter().map(|(_, _, r)| r.len() as u64).sum();
    assert!(total - cut <= log_archive::floor(cap), "left {} over {}", total - cut, cap);
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
    let s = block_on(log_archive::step(&b, &mut b.run.borrow_mut(), cap, range_max, log_archive::PROMOTE_MAX)).unwrap();
    assert_eq!(s, Step::Idle, "no snapshot yet");
    snapshot(&b);
    let (mut promotions, mut worst, mut largest) = (Vec::new(), 0u64, 0u64);
    for k in 30..400 {
        push(&b, k, 300 + (k as usize * 37) % 2_000);
        for _ in 0..20 {
            match block_on(log_archive::step(&b, &mut b.run.borrow_mut(), cap, range_max, log_archive::PROMOTE_MAX))
                .unwrap()
            {
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

/// The drain keeps up with B0-30's pushes from half to twice the row's rate at the rerun's measured step costs, both
/// archiving and promoting, and with margin. The bed is B0-30 scaled down with its ratios kept: an archive range of six
/// entries of about 1.6 KB, as hosted six of 323 KB fill 2 MiB, and a cap of 64 ranges; each bed byte stands for `k`
/// hosted bytes in the cost model. At each rate, from a log at its cap and fully promoted, pushes arrive on the drain's
/// modeled clock for 2.5 modeled minutes and the drain steps whenever anything is due:
///
/// - the log never passes its cap by more than the slack;
/// - promotion keeps pace: the tail above the last promotion stays inside `cap / PROMOTE_AHEAD` and one promotion, and
///   `promotedTo` advances as far as the log does, less that tail;
/// - the drain is idle at least 0.8 of `1 - r/2` of the time at `r` times the row's rate: about what a drain with
///   twice the row's rate in hand would be. The resume's bug (one object a step at the row's own rate) idled 16% at 1x
///   here against 48%.
///
/// The rates straddle the point where one archive range builds up between steps, where the resume at day 1.82 found
/// steps of one object each (125 entries a minute archived and 77 promoted against 129 applied). Then, at twice the
/// row, a backlog of 32 ranges lands at once, as after a leader change: the first step archives before it promotes,
/// and the drain's rate through the backlog is at least 1.5 times the input's. The rerun's constants (4 ranges a
/// promotion, one object an archive step) fail it (plants 27 and 28 in `tests/am1/b030split/plant.py`).
#[test]
fn the_drain_outruns_twice_the_rerun_s_log_rate_at_its_step_costs() {
    let rates = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
    let failures: Vec<String> = std::thread::scope(|sc| {
        let runs: Vec<_> = rates.iter().map(|&r| sc.spawn(move || drain_at(r, r == 2.0))).collect();
        runs.into_iter().filter_map(|t| t.join().expect("no panic").err()).collect()
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// One rate of `the_drain_outruns_twice_the_rerun_s_log_rate_at_its_step_costs`, as `r` times the row's pushes.
fn drain_at(r: f64, backlog: bool) -> Result<(), String> {
    let mut b = Bed::new();
    let range_max = 6 * 1_700;
    let k = (log_archive::ARCHIVE_RANGE_MAX as f64) / range_max as f64;
    b.scale = Some(k);
    let cap = 64 * range_max;
    let promote_max = (log_archive::PROMOTE_MAX as f64 / k) as u64;
    let step = |b: &Bed| {
        let s =
            block_on(log_archive::step(b, &mut b.run.borrow_mut(), cap, range_max, promote_max)).expect("a drain step");
        b.truncate();
        s
    };
    let mut n = 0u32;
    while b.bytes(b.leader()) < cap {
        push(&b, n, 1_400);
        n += 1;
    }
    snapshot(&b);
    // The drain catches up before the clock starts, as it had hosted before the rerun's load.
    while step(&b) != Step::Idle {}
    let fold = |b: &Bed| {
        let w = b.w.borrow();
        let m = &w.repo.members[b.leader()];
        let f = &m.group_fold().fold;
        (m.last_index().0, f.promoted().map_or(0, |p| p.to.0), f.archived().map_or(0, |a| a.to.0))
    };
    let tail = |b: &Bed| {
        let after = fold(b).1;
        b.stored(b.leader()).iter().filter(|e| e.index.0 > after).map(|e| row(e).len() as u64).sum::<u64>()
    };
    let rate = r * TWICE_THE_ROW_MB_S / 2.0;
    // The modeled time between pushes: each one's hosted bytes over the input rate.
    let gap = |b: &Bed| {
        let e = b.stored(b.leader()).last().map_or(0, |e| row(e).len()) as f64;
        e * k / 1e6 / rate * 1_000.0
    };
    let (end, mut next, mut idle, mut peak, mut worst) = (150_000.0, 0.0, 0.0, 0u64, 0u64);
    let (last0, promoted0, _) = fold(&b);
    b.clock.set(0.0);
    while b.clock.get() < end {
        while next <= b.clock.get() {
            push(&b, n, 1_400);
            n += 1;
            next += gap(&b);
        }
        // The drain's entries are charged by the bed; the step's own CPU is not modeled.
        if step(&b) == Step::Idle {
            idle += next - b.clock.get();
            b.clock.set(next);
        }
        peak = peak.max(b.bytes(b.leader()));
        worst = worst.max(tail(&b));
    }
    let (last1, promoted1, _) = fold(&b);
    let ahead = cap / log_archive::PROMOTE_AHEAD;
    let bound = ahead + range_max * log_archive::PROMOTE_RANGES;
    let entry = row(b.stored(b.leader()).last().unwrap()).len() as u64;
    let idle_share = idle / end;
    let mut out = format!(
        "{r}x the row ({rate:.2} MB/s): {} entries applied, {} promoted; idle {:.0}%; peak log {peak} of a cap of \
         {cap}, worst unpromoted tail {worst} of {bound}",
        last1 - last0,
        promoted1 - promoted0,
        100.0 * idle_share
    );
    let mut bad = Vec::new();
    if peak > cap {
        bad.push("the log passed its cap");
    }
    if worst > bound || promoted1 - promoted0 + bound / entry < last1 - last0 {
        bad.push("promotion fell behind the log");
    }
    // At r times the row a drain with twice the row in hand idles 1 - r/2 of the time; a fifth of that is the
    // model's slack for the time a step's entries wait on one another.
    if idle_share < 0.8 * (1.0 - r / 2.0) {
        bad.push("the drain has less than about twice the row's rate in hand");
    }
    if backlog {
        let (from, t0) = (fold(&b).2, b.clock.get());
        for _ in 0..192 {
            push(&b, n, 1_400);
            n += 1;
        }
        // A log far over its cap archives before it promotes ahead, however long the tail: only an archive truncates.
        let first = step(&b);
        if !matches!(first, Step::Archived { .. }) {
            bad.push("a log over its cap promoted before it archived");
        }
        while step(&b) != Step::Idle {}
        let all = b.w.borrow().repo.disks[0].log.clone();
        let to = fold(&b).2;
        let bytes: u64 = all.iter().filter(|e| e.index.0 > from && e.index.0 <= to).map(|e| row(e).len() as u64).sum();
        let through = bytes as f64 * k / 1e6 / ((b.clock.get() - t0) / 1_000.0);
        out += &format!("; a 32-range backlog drained at {through:.2} MB/s");
        if through < 1.5 * TWICE_THE_ROW_MB_S {
            bad.push("the backlog drained under 1.5 times twice the row");
        }
    }
    println!("drain at {out}");
    if bad.is_empty() { Ok(()) } else { Err(format!("{out}: {}", bad.join(", "))) }
}

/// A promotion of many megabytes reads its range in pieces of `PAUSE_EVERY` bytes and lets the member run between
/// them (`ArchiveHost::pause`), so a range of 8 archive ranges never holds the leader's loop for its whole decode and
/// hash. Entries written during a pause do not join the range out of order or twice: the promotion names exactly the
/// packs of the entries it covers, and starts right after the last one.
#[test]
fn a_large_promotion_reads_its_range_in_pieces_and_names_every_pack_once() {
    let b = Bed::new();
    for k in 0..24 {
        push(&b, k, 380_000);
    }
    snapshot(&b);
    let range_max = 1 << 20;
    // A tail of about 9 MB is over `cap / PROMOTE_AHEAD` and nothing is due to archive.
    let cap = 64 << 20;
    let s = block_on(log_archive::step(&b, &mut b.run.borrow_mut(), cap, range_max, log_archive::PROMOTE_MAX)).unwrap();
    let Step::Promoted { from, to, .. } = s else { panic!("{s:?}") };
    assert!(b.pauses.get() >= 2, "{} pauses for {} entries", b.pauses.get(), to.0 - from.0 + 1);
    let all = b.w.borrow().repo.disks[0].log.clone();
    let range: Vec<_> = all.iter().filter(|e| e.index >= from && e.index <= to).cloned().collect();
    let bytes: u64 = range.iter().map(|e| row(e).len() as u64).sum();
    assert!(bytes > range_max * 4 && bytes <= range_max * log_archive::PROMOTE_RANGES + 400_000, "{bytes}");
    assert_eq!(from, Index(1));
    assert_eq!(b.promoted.borrow().last().unwrap(), &log_archive::promotion_sums(&range));
}

/// The second resume's stall, at 0.5 to 2 times the row's rate: a drain run starts once the log passes the trigger,
/// archives down to the floor before it goes `Idle`, and the log never passes the cap, though an idle leader sums its
/// log only once a `SUM_EVERY_MS` and input goes on arriving through the wait. The bed models the site's cadence
/// (`workers/site/src/archive.rs`): steps one after another, each charged its entry and pacing as in
/// `the_drain_outruns_twice_the_rerun_s_log_rate_at_its_step_costs`; a run continued at once after `DRAIN_FOR_MS`
/// with its `Run`; and after `Idle` the next run a sum wait later. Its cap is 64 archive ranges, `LOG_MAX_BYTES / 16`
/// at hosted sizes, and its sum wait is `SUM_EVERY_MS / 16`, so a wait's input is the same share of the band as hosted.
/// It starts just under the trigger. On e9418153's rule (a run starts only over the cap and cuts below it per step)
/// the log passes the cap in every wait (plant 38), and with the trigger but without the run's hold (plant 39) a run
/// goes `Idle` above the floor.
#[test]
fn the_log_stays_under_its_cap_through_the_sum_wait() {
    let rates: Vec<f64> = match std::env::var("SUM_WAIT_RATE") {
        Ok(r) => vec![r.parse().expect("a rate")],
        Err(_) => vec![0.5, 1.0, 1.5, 2.0],
    };
    let failures: Vec<String> = std::thread::scope(|sc| {
        let runs: Vec<_> = rates.iter().map(|&r| sc.spawn(move || sum_wait_at(r))).collect();
        runs.into_iter().filter_map(|t| t.join().expect("no panic").err()).collect()
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The site's sum wait and run length (`workers/site/src/archive.rs`, `SUM_EVERY_MS`, `DRAIN_FOR_MS`).
const SUM_EVERY_MS: f64 = 60_000.0;
const DRAIN_FOR_MS: f64 = 90_000.0;

fn sum_wait_at(r: f64) -> Result<(), String> {
    let mut b = Bed::new();
    let range_max = 6 * 1_700;
    let k = (log_archive::ARCHIVE_RANGE_MAX as f64) / range_max as f64;
    b.scale = Some(k);
    let scale: f64 = std::env::var("SUM_WAIT_SCALE").map_or(4.0, |v| v.parse().unwrap());
    let cap = (graftyard_fold::LOG_MAX_BYTES as f64 / k / scale) as u64;
    let wait = SUM_EVERY_MS / scale;
    let promote_max = (log_archive::PROMOTE_MAX as f64 / k) as u64;
    let step = |b: &Bed| {
        let s = block_on(log_archive::step(b, &mut b.run.borrow_mut(), cap, range_max, promote_max)).expect("a step");
        b.truncate();
        s
    };
    let total = |b: &Bed| {
        let m = b.leader();
        let after = b.w.borrow().repo.members[m].group_fold().fold.archived().map_or(0, |a| a.to.0);
        b.sum_after(m, after)
    };
    let mut n = 0u32;
    while b.bytes(b.leader()) < log_archive::trigger(cap) {
        push(&b, n, 1_400);
        n += 1;
    }
    snapshot(&b);
    // Promoted up to the log's head, as the rerun was; then pushes take the log over the trigger during the wait.
    let mut promoting = 0;
    while block_on(log_archive::step(&b, &mut Run::default(), cap, range_max, promote_max)).unwrap() != Step::Idle {
        promoting += 1;
        assert!(promoting < 100, "the seed's promotions do not end");
    }
    let rate = r * TWICE_THE_ROW_MB_S / 2.0;
    let gap = |b: &Bed| {
        let e = b.stored(b.leader()).last().map_or(0, |e| row(e).len()) as f64;
        e * k / 1e6 / rate * 1_000.0
    };
    let (end, mut next, mut peak, mut runs, mut bad) = (300_000.0, 0.0f64, 0u64, 0u32, Vec::new());
    let (mut idle_short, mut short) = (0u32, 0u64);
    b.clock.set(0.0);
    let mut sum_at = wait;
    let feed = |b: &Bed, n: &mut u32, next: &mut f64, peak: &mut u64| {
        while *next <= b.clock.get() {
            push(b, *n, 1_400);
            *n += 1;
            *next += gap(b);
        }
        // Only pushes in between, so the log is at its largest after the last.
        *peak = (*peak).max(total(b));
    };
    while b.clock.get() < end {
        // Idle until the next sum, input arriving all along.
        while b.clock.get() < sum_at {
            b.clock.set(next.min(sum_at));
            feed(&b, &mut n, &mut next, &mut peak);
        }
        // One run: steps until `Idle`, each run continued at once after `DRAIN_FOR_MS` with its `Run` kept. A run
        // that saw the log over the trigger must archive down to the floor (`reached`) before it goes `Idle`.
        let began = b.clock.get();
        let (mut over, mut low) = (false, u64::MAX);
        loop {
            over |= total(&b) > log_archive::trigger(cap);
            let s = step(&b);
            // The log right after an archive step, before the input that arrived while it ran.
            if matches!(s, Step::Archived { .. }) {
                low = low.min(total(&b));
            }
            feed(&b, &mut n, &mut next, &mut peak);
            if s == Step::Idle {
                break;
            }
            if b.clock.get() > end + DRAIN_FOR_MS {
                return Err(format!("{r}x the row: a run that never goes Idle"));
            }
            if b.clock.get() >= began + DRAIN_FOR_MS {
                b.charge(200.0);
            }
        }
        runs += 1;
        // Within the bytes of the run's own last entries.
        if over && low > log_archive::floor(cap) + range_max / 2 {
            idle_short += 1;
            short = short.max(low - log_archive::floor(cap));
        }
        sum_at = b.clock.get() + wait;
    }
    let out = format!(
        "{r}x the row: {runs} runs in {:.0} modeled s; peak log {peak} against a cap of {cap} (trigger {}, floor {}); \
         {idle_short} runs over the trigger went Idle short of the floor, by up to {short}",
        end / 1_000.0,
        log_archive::trigger(cap),
        log_archive::floor(cap)
    );
    println!("sum wait at {out}");
    if peak > cap {
        bad.push("the log passed its cap");
    }
    if idle_short > 0 {
        bad.push("a run over the trigger went Idle before it reached the floor");
    }
    if runs < 2 {
        bad.push("fewer than two runs: the wait was never crossed");
    }
    if bad.is_empty() { Ok(()) } else { Err(format!("{out}: {}", bad.join(", "))) }
}
