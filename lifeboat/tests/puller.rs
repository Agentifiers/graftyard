//! The puller's sources: the feed while it moves, the edge once it is stale and for a chain the feed left behind, and
//! back; rows always from the copy's next index, a halted chain never moved, and nothing skipped.

use graftyard_git::export::{ChangeBatch, ExportObject};
use graftyard_lifeboat::puller::{FEED_STALE_MS, Puller, Rows};
use graftyard_types::{ChainId, DbId, Index, RepoId};

fn repo(n: i64) -> ChainId {
    ChainId::Repo(RepoId(DbId(n)))
}

fn row(c: ChainId, i: u64) -> Vec<u8> {
    format!("{c}@{i}").into_bytes()
}

fn object(seq: u32, chains: &[(ChainId, u64, u64)]) -> ExportObject {
    ExportObject {
        batch: ChangeBatch { minute: 100, seq, chains: chains.iter().map(|(c, _, to)| (*c, Index(*to))).collect() },
        entries: chains
            .iter()
            .map(|(c, from, to)| (*c, Index(*from), (*from..=*to).map(|i| row(*c, i)).collect()))
            .collect(),
    }
}

fn apply(p: &mut Puller, rs: &[Rows]) {
    for r in rs {
        p.applied(r.chain, Index(r.first.0 + r.rows.len() as u64 - 1));
    }
}

#[test]
fn the_feed_then_stale_edge_pulls_then_the_feed_again_skip_nothing() {
    let (a, b) = (repo(1), repo(2));
    let mut p = Puller::default();
    assert!(p.stale(0), "no feed object yet: the edge");
    let got = p.take_object(&object(0, &[(a, 1, 3), (b, 1, 2)]), 1_000).unwrap();
    assert_eq!(
        got.iter().map(|r| (r.chain, r.first, r.rows.len())).collect::<Vec<_>>(),
        [(a, Index(1), 3), (b, Index(1), 2)]
    );
    apply(&mut p, &got);
    assert!(!p.stale(1_000 + FEED_STALE_MS - 1));
    assert!(p.pulls(1_000 + FEED_STALE_MS - 1).is_empty(), "a fresh feed pulls nothing from the edge");
    // The feed stops: every known chain is pulled from its next index.
    let now = 1_000 + FEED_STALE_MS;
    assert!(p.stale(now));
    let pulls = p.pulls(now);
    assert_eq!(pulls.iter().map(|x| (x.chain, x.from)).collect::<Vec<_>>(), [(a, Index(4)), (b, Index(3))]);
    // The edge gives a's tail 4..=6; a page that starts elsewhere is refused.
    assert!(p.take_page(a, Index(5), vec![row(a, 5)]).is_err());
    let r = p.take_page(a, Index(4), (4..=6).map(|i| row(a, i)).collect()).unwrap();
    apply(&mut p, &[r]);
    // The feed moves again with an object from before the edge pulls: a's rows the copy holds are dropped.
    let got = p.take_object(&object(1, &[(a, 4, 7), (b, 3, 3)]), now + 5_000).unwrap();
    assert_eq!(
        got.iter().map(|r| (r.chain, r.first, r.rows.clone())).collect::<Vec<_>>(),
        [(a, Index(7), vec![row(a, 7)]), (b, Index(3), vec![row(b, 3)])]
    );
    apply(&mut p, &got);
    assert_eq!((p.held.get(a), p.held.get(b)), (Index(7), Index(3)));
    // An object at or before the position is ignored.
    assert!(p.take_object(&object(1, &[(a, 4, 7)]), now + 6_000).unwrap().is_empty());
}

/// A chain whose feed rows start past the copy's next index (first carried past index 1, or the copy fell behind) is
/// not stitched: it is pulled from the edge while the feed is fresh, until it reaches the feed's index.
#[test]
fn a_gap_in_the_feed_is_filled_from_the_edge_never_stitched() {
    let a = repo(1);
    let mut p = Puller::default();
    let got = p.take_object(&object(0, &[(a, 40, 42)]), 1_000).unwrap();
    assert!(got.is_empty(), "rows from 40 for a copy that holds nothing");
    assert_eq!(p.pulls(1_000).iter().map(|x| (x.chain, x.from)).collect::<Vec<_>>(), [(a, Index(1))]);
    let r = p.take_page(a, Index(1), (1..=42).map(|i| row(a, i)).collect()).unwrap();
    apply(&mut p, &[r]);
    assert!(p.pulls(1_000).is_empty(), "caught up to the feed: no more edge pulls while it is fresh");
}

#[test]
fn a_halted_chain_is_moved_by_no_source_and_the_state_round_trips() {
    let (a, b) = (repo(1), repo(2));
    let mut p = Puller::default();
    let got = p.take_object(&object(0, &[(a, 1, 2), (b, 1, 2)]), 0).unwrap();
    apply(&mut p, &got);
    p.halt(a);
    let got = p.take_object(&object(1, &[(a, 3, 4), (b, 3, 4)]), 10).unwrap();
    assert_eq!(got.iter().map(|r| r.chain).collect::<Vec<_>>(), [b]);
    assert!(p.pulls(FEED_STALE_MS * 10).iter().all(|x| x.chain != a));
    assert!(p.take_page(a, Index(3), vec![row(a, 3)]).is_err());
    assert_eq!(Puller::decode(&p.encode()).unwrap(), p);
    // A malformed object moves nothing.
    let mut bad = object(2, &[(b, 3, 5)]);
    bad.entries[0].2.pop();
    assert!(p.take_object(&bad, 20).is_err());
    assert_eq!(p.feed.unwrap().seq, 1);
}
