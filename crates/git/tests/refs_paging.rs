//! No member reply grows with a repository's ref count (`decisions/DECISIONS.md`, "No member reply grows with a
//! repository's ref count"; `tests/m3/results/wire-fix.md`).
//!
//! `repo:65536` (`m2b/bf20`) holds 571,872 refs named like `refs/heads/bf20/c300a1/1042`. Its leader answered the git
//! cache shard's catch-up, `/read {"q": {}}`, with every ref in one JSON reply and died in `handle_alloc_error`, 41
//! times in 26 hours. These tests build a core tier of that size and shape and drive the reads a catch-up, a rehydrate,
//! and the edge's info/refs make, as the member answers them (`graftyard_git::group::refs_read`), checking each reply's
//! size against a bound that does not depend on the ref count.

use graftyard_fold::{MemStore, Val, seed};
use graftyard_git::group::{RefsPage, refs_query, refs_read};
use graftyard_git::ingest::{LogPacks, ref_changes};
use graftyard_git::refs::{ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{REFS_PAGE_MAX, RepoEvent, RepoFold, entry_ref_changes};
use graftyard_git::smart::{receive_pack_advertisement, receive_pack_head, receive_pack_lines, receive_pack_tail};
use graftyard_git::tiers::{CORE, ref_key};
use graftyard_types::{AccountId, DbId, Index, RepoId, RequestId, Ulid};

/// bf20's ref count, read from a follower's core tier on 2026-11-09 (`tests/m3/results/wire-fix.md`).
const BF20_REFS: usize = 571_872;

/// The largest reply any of these reads may give, whatever the ref count: one page of `REFS_PAGE_MAX` refs.
const REPLY_MAX: usize = 1 << 20;

fn oid(k: u64) -> ObjectId {
    let mut o = [0u8; 20];
    o[..8].copy_from_slice(&k.to_be_bytes());
    o[8..16].copy_from_slice(&1_794_000_000_000u64.to_be_bytes());
    ObjectId(o)
}

/// bf20's names: 33 load runs of about 17,300 refs each under `refs/heads/bf20/<run>/<k>`, and `refs/heads/main`.
fn name(k: usize) -> String {
    if k == 0 {
        return "refs/heads/main".into();
    }
    let runs = ["a", "s", "t", "z"];
    let run = k % 33;
    format!("refs/heads/bf20/c300{}{}/{}", runs[run % 4], run / 4 + 1, k / 33)
}

fn fold(n: usize) -> RepoFold {
    let mut f = RepoFold::new(RepoId(DbId(65536)), AccountId(DbId(7_000_001)));
    let rows = (0..n)
        .map(|k| {
            let key = vec![ref_key(&RefName::parse(name(k).as_bytes()).unwrap())];
            (key, vec![Val::Blob(postcard::to_allocvec(&oid(k as u64)).unwrap().into())])
        })
        .collect();
    let mut core = MemStore::default();
    seed(&mut core, &CORE, "ref", Index(0), rows);
    f.core = Box::new(core);
    f
}

fn bytes(v: &serde_json::Value) -> usize {
    serde_json::to_vec(v).unwrap().len()
}

/// Every page of a paged read, as a member answers `refs_query` (`ChainLog::refs_page`, the edge's `refs_page`).
fn pages(f: &RepoFold, prefix: &[u8]) -> Vec<RefsPage> {
    let mut out = Vec::new();
    let mut after = None;
    loop {
        let q = refs_query(prefix, after.as_deref(), REFS_PAGE_MAX)["q"].clone();
        let v = refs_read(f, &q);
        assert!(bytes(&v) <= REPLY_MAX, "a page of {} bytes", bytes(&v));
        let p: RefsPage = serde_json::from_value(v).unwrap();
        after = p.next.clone();
        out.push(p);
        if after.is_none() {
            return out;
        }
    }
}

/// The failure, before the fix: `{"q": {}}` answered every ref, so the reply grew with the ref count, to 49 MB of
/// JSON at bf20's size. Now it answers `{"paged": REFS_PAGE_MAX}` past one page, and every page is under REPLY_MAX.
#[test]
fn at_bf20s_ref_count_no_read_reply_grows_with_the_refs() {
    let f = fold(BF20_REFS);
    let whole = serde_json::to_vec(&f.refs().iter().map(|(n, o)| (n.display(), o.hex())).collect::<Vec<_>>()).unwrap();
    assert!(whole.len() > 40 << 20, "the old reply: {} bytes", whole.len());

    let all = refs_read(&f, &serde_json::json!({}));
    assert!(bytes(&all) <= REPLY_MAX, "`{{\"q\": {{}}}}` answered {} bytes at {BF20_REFS} refs", bytes(&all));
    assert!(all == serde_json::json!({ "paged": REFS_PAGE_MAX }), "`{{\"q\": {{}}}}` past one page says it is paged");

    // A rehydrate's pages: every ref once, in name order, each page bounded.
    let ps = pages(&f, b"");
    let got: Vec<(String, String)> = ps.iter().flat_map(|p| p.refs.clone()).collect();
    assert_eq!(got.len(), BF20_REFS);
    assert_eq!(ps.len(), BF20_REFS.div_ceil(REFS_PAGE_MAX));
    let want: Vec<(String, String)> = f.refs().iter().map(|(n, o)| (n.display(), o.hex())).collect();
    assert_eq!(got, want);

    // The clone cache's tag digest reads `refs/tags/` alone: bf20 has none, so one empty page.
    let tags = pages(&f, b"refs/tags/");
    assert_eq!(tags.len(), 1);
    assert!(tags[0].refs.is_empty());
}

/// A repository that fits one page still answers `{"q": {}}` with every ref, as before, so a member of this release
/// answers an older edge the same bytes for every repository it could serve.
#[test]
fn a_small_repository_answers_every_ref_as_before() {
    let f = fold(3);
    let v = refs_read(&f, &serde_json::json!({}));
    let want: Vec<(String, String)> = f.refs().iter().map(|(n, o)| (n.display(), o.hex())).collect();
    assert_eq!(v, serde_json::json!(want));
    let ps = pages(&f, b"refs/heads/bf20/");
    assert_eq!(ps.len(), 1);
    assert_eq!(ps[0].refs.len(), 2);
}

/// The edge's receive-pack advertisement streamed a page at a time is byte for byte the advertisement of the whole
/// set, and no piece is larger than a page's.
#[test]
fn the_streamed_advertisement_is_the_whole_advertisement() {
    let f = fold(25_001);
    let caps = "report-status delete-refs agent=git/test";
    let whole = receive_pack_advertisement(&f.refs(), caps);
    let mut streamed = receive_pack_head();
    for (i, p) in pages(&f, b"").iter().enumerate() {
        let refs: Vec<(RefName, ObjectId)> = p
            .refs
            .iter()
            .map(|(n, o)| (RefName::parse(n.as_bytes()).unwrap(), ObjectId::parse_hex(o.as_bytes()).unwrap()))
            .collect();
        let piece = receive_pack_lines(&refs, caps, i == 0);
        assert!(piece.len() <= REPLY_MAX);
        streamed.extend(piece);
    }
    streamed.extend(receive_pack_tail());
    assert_eq!(streamed, whole);
    // An empty repository's one empty page is `capabilities^{}`, as before.
    let empty = fold(0);
    let mut one = receive_pack_head();
    one.extend(receive_pack_lines(&[], caps, true));
    one.extend(receive_pack_tail());
    assert_eq!(one, receive_pack_advertisement(&empty.refs(), caps));
}

/// Catch-up reads no ref set: it applies the ref changes of the entries past its cursor, which are what the fold
/// applied, so the shard's refs after a catch-up are the fold's.
#[test]
fn catch_up_applies_the_entries_ref_changes_and_ends_at_the_folds_refs() {
    let mut refs: std::collections::BTreeMap<String, String> =
        (0..50).map(|k| (name(k), oid(k as u64).hex())).collect();
    let up = |n: &str, new: Option<u64>| RefUpdate {
        name: RefName::parse(n.as_bytes()).unwrap(),
        expect: None,
        new: new.map(oid),
    };
    let refs_ev =
        |updates: Vec<RefUpdate>| RepoEvent::Refs { push: RequestId(Ulid(1)), actor: None, updates, pack: None };
    let entries: Vec<LogPacks> = [
        vec![refs_ev(vec![up(&name(3), None), up("refs/heads/new", Some(900))])],
        vec![refs_ev(vec![up("refs/heads/new", Some(901)), up(&name(4), Some(902))])],
        vec![RepoEvent::TestMerged {
            n: graftyard_git::issues::IssueNumber(7),
            base: oid(1),
            head: oid(2),
            result: graftyard_git::issues::MergeabilityResult::Clean { merge: oid(903) },
            pack: None,
        }],
    ]
    .iter()
    .enumerate()
    .map(|(i, evs)| LogPacks {
        index: Index(11 + i as u64),
        packs: Vec::new(),
        promoted: None,
        consumed: Vec::new(),
        refs: Some(entry_ref_changes(evs)),
    })
    .collect();
    let changes = ref_changes(&entries, Index(10), Index(13)).unwrap();
    for (n, o) in &changes {
        match o {
            Some(o) => refs.insert(n.clone(), o.clone()),
            None => refs.remove(n),
        };
    }
    assert!(!refs.contains_key(&name(3)));
    assert_eq!(refs["refs/heads/new"], oid(901).hex());
    assert_eq!(refs[&name(4)], oid(902).hex());
    assert_eq!(refs["refs/pull/7/merge"], oid(903).hex());
    // The reply that carries them is the entries', bounded by the entries page, not by the repository's ref count.
    assert!(bytes(&serde_json::json!(changes)) < 1_000);
}
