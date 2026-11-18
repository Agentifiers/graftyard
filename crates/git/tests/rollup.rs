//! Repository-chain rolls (`decisions/DECISIONS.md`, `roll-up`): leader records, follower verification, and the fold's
//! promotion and compaction rows.

mod common;
use common::*;

use std::cell::RefCell;
use std::collections::BTreeSet;

use graftyard_git::repo::REPO_FORMAT_ROLLUP;
use graftyard_git::{Compacted, PackId, Promoted};
use graftyard_raft::{Fold, Record, Scratch};
use graftyard_types::{Index, SiteSet};

struct Bed {
    w: RefCell<World>,
}

impl Bed {
    fn new() -> Bed {
        Bed { w: RefCell::new(World::new()) }
    }

    /// Record one leader event entry exactly as the archive host does, resolving only after every follower settled.
    fn record(&self, format: Format, events: Vec<RepoEvent>) -> Result<Index, String> {
        let mut w = self.w.borrow_mut();
        let request = w.rid();
        let l = w.repo.leader();
        let now = w.now;
        w.repo.on(l, now, |m, t| m.record(t, request, format, events)).map_err(|e| format!("{e:?}"))?;
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

    fn promotions(&self) -> Vec<Promoted> {
        let promotions: Vec<_> = (1..=4).map(promotion).collect();
        for p in &promotions {
            self.record(REPO_FORMAT_ROLLUP, vec![RepoEvent::Promoted(p.clone())]).expect("promotion commits");
        }
        promotions
    }
}

fn pack(n: u128) -> PackRef {
    PackRef::Stored {
        id: PackId(Ulid(n)),
        checksum: Digest([n as u8; 32]),
        bytes: 100 + n as u64,
        sites: SiteSet(0b111),
    }
}

fn promotion(n: u64) -> Promoted {
    Promoted { from: Index(n), to: Index(n), pack: pack(u128::from(n)) }
}

fn compacted(promotions: &[Promoted], produced: PackRef) -> Compacted {
    Compacted {
        consumed: promotions.iter().map(|p| p.pack.stored_id().expect("a promotion is stored")).collect(),
        produced,
        from: promotions.first().expect("a roll has inputs").from,
        to: promotions.last().expect("a roll has inputs").to,
    }
}

#[test]
fn a_roll_of_consecutive_promotions_commits_and_folds() {
    let b = Bed::new();
    let p = b.promotions();
    let first = compacted(&p[..2], pack(11));
    let second = compacted(&p[2..], pack(12));
    b.record(REPO_FORMAT_ROLLUP, vec![RepoEvent::Compacted(first.clone())]).expect("first roll commits");
    b.record(REPO_FORMAT_ROLLUP, vec![RepoEvent::Compacted(second.clone())]).expect("second roll commits");
    // One more entry carries the second roll's commit index to every member before their folds are compared.
    b.record(REPO_FORMAT_ROLLUP, Vec::new()).expect("commit notice");
    let rolls = vec![first, second];
    let consumed: BTreeSet<_> = p.iter().map(|p| p.pack.stored_id().unwrap()).collect();
    let w = b.w.borrow();
    for member in &w.repo.members {
        let fold = &member.group_fold().fold;
        assert_eq!(fold.compactions(), rolls);
        assert_eq!(fold.consumed(), consumed);
        assert_eq!(fold.rolled_to(), p[3].to);
        for p in &p {
            assert_eq!(fold.promoted_from(p.from), Some(p.clone()));
        }
    }
}

#[test]
fn verify_refuses_a_roll_that_skips_overlaps_or_misnames_promotions() {
    let b = Bed::new();
    let p = b.promotions();
    let fold = b.w.borrow_mut().fold();
    let first = compacted(&p[..2], pack(11));
    let skipped = compacted(&p[1..3], pack(20));
    let gap = compacted(&[p[0].clone(), p[2].clone()], pack(21));
    let single = compacted(&p[..1], pack(22));
    let wrong_end = Compacted { to: p[2].to, ..first.clone() };
    let mut wrong_pack = first.clone();
    wrong_pack.consumed[1] = PackId(Ulid(999));
    let inline = Compacted { produced: PackRef::Inline(b"PACK".to_vec()), ..first.clone() };
    for (name, c) in [
        ("starts at the second promotion", skipped),
        ("skips the second promotion", gap),
        ("consumes only one promotion", single),
        ("ends past its inputs", wrong_end),
        ("misnames the second input", wrong_pack),
        ("produces an inline pack", inline),
    ] {
        let ev = RepoEvent::Compacted(c);
        assert!(fold.scratch().check(&ev).is_err(), "{name}");
    }

    // A scratch applies its earlier events just as an entry does: the first roll moves the next roll's required start.
    let mut scratch = fold.scratch();
    scratch.check(&RepoEvent::Compacted(first.clone())).expect("the first roll verifies");
    scratch.apply(&RepoEvent::Compacted(first));
    let overlap = compacted(&p[1..3], pack(23));
    assert!(scratch.check(&RepoEvent::Compacted(overlap)).is_err(), "a roll does not overlap an earlier roll");
    let next = compacted(&p[2..], pack(24));
    scratch.check(&RepoEvent::Compacted(next)).expect("the next consecutive roll verifies");
}

/// A 32.1 `Promoted`, committed before roll-up or with it off, is not logged for roll-up, so every release folds it
/// to the digest its members kept (F88), and no roll may consume it.
#[test]
fn a_promotion_committed_before_roll_up_is_not_rollable() {
    let b = Bed::new();
    let old = promotion(1);
    b.record(REPO_FORMAT_V1, vec![RepoEvent::Promoted(old.clone())]).expect("a 32.1 promotion commits");
    let new = promotion(2);
    b.record(REPO_FORMAT_ROLLUP, vec![RepoEvent::Promoted(new.clone())]).expect("a 32.25 promotion commits");
    b.record(REPO_FORMAT_ROLLUP, Vec::new()).expect("commit notice");
    let fold = b.w.borrow_mut().fold();
    assert_eq!(fold.promoted(), Some(new.clone()), "the last promotion still moves on");
    assert_eq!(fold.promoted_from(old.from), None);
    assert_eq!(fold.promoted_from(new.from), Some(new.clone()));
    let c = compacted(&[old, new], pack(11));
    assert!(fold.scratch().check(&RepoEvent::Compacted(c)).is_err(), "a roll over a 32.1 promotion");
}

#[test]
fn compacted_keeps_its_wire_tag() {
    let p = [promotion(1), promotion(2)];
    let ev = RepoEvent::Compacted(compacted(&p, pack(11)));
    // `codec::to_bytes` prefixes its own version byte; the following postcard payload starts with the variant tag.
    let postcard = graftyard_raft::codec::to_bytes(&ev);
    assert_eq!(postcard[0], graftyard_raft::codec::CODEC_VERSION);
    assert_eq!(postcard[1], 47);
    let mut record = Vec::new();
    Record::encode(&ev, &mut record);
    assert_eq!(record[0], 47);
}
