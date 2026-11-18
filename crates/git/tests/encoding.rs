//! The stored encodings of the Milestone 1 `RepoEvent` variants, pinned to their bytes at commit 313d4a3, so adding
//! the Milestone 2 variants changes no existing entry's digest.

use graftyard_git::refs::{ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{RejectReason, RepoEvent};
use graftyard_git::{Archived, PackId, PackRef, Promoted, R2Key};
use graftyard_raft::{EntryBody, Format, LogEntry, PREV_ZERO, Term, TierSet};
use graftyard_raft::{Record, digest};
use graftyard_types::{AccountId, ChainId, DbId, Digest, Index, RepoId, RequestId, SiteSet, Timestamp, Ulid};
use smallvec::smallvec;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn events() -> Vec<RepoEvent> {
    let name = |s: &str| RefName::parse(s.as_bytes()).unwrap();
    let stored = PackRef::Stored { id: PackId(Ulid(9)), checksum: Digest([1; 32]), bytes: 10, sites: SiteSet(3) };
    vec![
        RepoEvent::Refs {
            push: RequestId(Ulid(7 << 64)),
            actor: Some(AccountId(DbId(101))),
            updates: vec![
                RefUpdate { name: name("refs/heads/main"), expect: None, new: Some(ObjectId([0xab; 20])) },
                RefUpdate { name: name("refs/tags/v1"), expect: Some(ObjectId([0xcd; 20])), new: None },
            ],
            pack: Some(PackRef::Inline(b"PACK".to_vec())),
        },
        RepoEvent::Refused {
            push: RequestId(Ulid(8)),
            refs: vec![(name("refs/heads/a"), RejectReason::Stale), (name("refs/heads/b"), RejectReason::NameConflict)],
        },
        RepoEvent::Promoted(Promoted { from: Index(1), to: Index(8), pack: stored }),
        RepoEvent::Archived(Archived {
            key: R2Key::Archive {
                chain: ChainId::Repo(RepoId(DbId(7))),
                from: Index(1),
                to: Index(8),
                digest: Digest([5; 32]),
            },
            from: Index(1),
            to: Index(8),
            from_at: Timestamp(1),
            to_at: Timestamp(2),
            digest: Digest([5; 32]),
        }),
    ]
}

#[test]
fn milestone_1_repo_events_keep_their_bytes_and_digest() {
    let got: Vec<String> = events()
        .iter()
        .map(|e| {
            let mut v = Vec::new();
            e.encode(&mut v);
            hex(&v)
        })
        .collect();
    let want = GOLDEN;
    assert_eq!(got, want);
    let entry = LogEntry {
        term: Term(3),
        index: Index(9),
        prev: PREV_ZERO,
        format: Format { family: 0x20, version: 1 },
        tiers: TierSet::CORE,
        at: Timestamp(1_000),
        requests: smallvec![(RequestId(Ulid(7 << 64)), 0..2)],
        extend: None,
        body: EntryBody::Events(events()),
    };
    assert_eq!(digest(&entry).hex(), GOLDEN_DIGEST);
}

const GOLDEN: [&str; 4] = [
    "0000000000000000070000000000000000010000000000000065000000020000000f726566732f68656164732f6d61696e0001abababababababababababababababababababab0000000c726566732f746167732f763101cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd000100000000045041434b",
    "0100000000000000000000000000000008000000020000000c726566732f68656164732f61000000000c726566732f68656164732f6205",
    "020000000000000001000000000000000801000000000000000000000000000000090101010101010101010101010101010101010101010101010101010101010101000000000000000a0000000000000003",
    "0300000079617263686976652f7265706f3a372f30303030303030303030303030303030303030312d30303030303030303030303030303030303030382d3035303530353035303530353035303530353035303530353035303530353035303530353035303530353035303530353035303530353035303530353035303500000000000000010000000000000008000000000000000100000000000000020505050505050505050505050505050505050505050505050505050505050505",
];
const GOLDEN_DIGEST: &str = "10586b5f7f402489541fde3fff7dc88095977c3515f5b903d625360fe893f0db";

/// F88: a committed 32.1 `Promoted` entry folds to the same core rows on every release, so a fold audit's refold on a
/// later release agrees with a member that applied the entry live on an earlier one. Pinned to the core digest the
/// release before roll-up (main `0d212879`) gives this entry; roll-up's `promoted_log` row changed it, and every chain
/// with a promotion in its audited tail then alarmed (`decisions/OPEN-QUESTIONS.md`, F88).
#[test]
fn a_committed_promoted_entry_folds_to_the_digest_it_had_when_it_committed() {
    use graftyard_fold::fold_digest;
    use graftyard_git::repo::{REPO_FORMAT_V1, RepoFold};
    use graftyard_raft::Fold;
    use graftyard_types::Tier;
    let mut fold = RepoFold::new(RepoId(DbId(65565)), AccountId(DbId(1)));
    let stored = PackRef::Stored { id: PackId(Ulid(9)), checksum: Digest([1; 32]), bytes: 10, sites: SiteSet(7) };
    let e = LogEntry {
        term: Term(2),
        index: Index(1),
        prev: PREV_ZERO,
        format: REPO_FORMAT_V1,
        tiers: TierSet::CORE,
        at: Timestamp(1_000),
        requests: Default::default(),
        extend: None,
        body: EntryBody::Events(vec![RepoEvent::Promoted(Promoted { from: Index(1), to: Index(17), pack: stored })]),
    };
    fold.apply_entry(&e);
    assert_eq!(
        fold_digest(&*fold.core, Tier::Core).hex(),
        "4ceb4070716938547b46317ff54b1d0650b802d024467bfd9c89eeba56b377cc"
    );
}
