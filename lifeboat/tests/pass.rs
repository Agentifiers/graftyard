//! The puller's pass against a fake install and copy: the feed, then the edge once the feed is stale, packs owed by
//! repository rows and kept only when they hash to the entry's checksum, and a halted chain moved by nothing again.

use std::cell::RefCell;
use std::collections::BTreeMap;

use graftyard_git::export::{self, ChangeBatch, ExportObject};
use graftyard_git::pack::{PackId, PackRef};
use graftyard_git::repo::{REPO_FORMAT_V1, RepoEvent};
use graftyard_lifeboat::pass::{self, Copied, Host, Imported, Owed, Page, State};
use graftyard_lifeboat::puller::FEED_STALE_MS;
use graftyard_raft::{EntryBody, LogEntry, Term, TierSet, codec};
use graftyard_types::{ChainId, DbId, Digest, Index, RepoId, RequestId, SiteSet, Timestamp, Ulid};
use sha2::{Digest as _, Sha256};

const REPO: ChainId = ChainId::Repo(RepoId(DbId(7)));
const IDS: ChainId = ChainId::Ids;

fn pack_of(i: u64) -> Vec<u8> {
    format!("PACK bytes of entry {i}").into_bytes()
}

/// A repository row at `i` whose push names a stored pack of `pack_of(i)`.
fn repo_row(i: u64) -> Vec<u8> {
    let b = pack_of(i);
    codec::to_bytes(&LogEntry {
        term: Term(1),
        index: Index(i),
        prev: Digest([0; 32]),
        format: REPO_FORMAT_V1,
        tiers: TierSet::CORE,
        at: Timestamp(1),
        requests: Default::default(),
        extend: None,
        body: EntryBody::Events(vec![RepoEvent::Refs {
            push: RequestId(Ulid(u128::from(i))),
            actor: None,
            updates: Vec::new(),
            pack: Some(PackRef::Stored {
                id: PackId(Ulid(u128::from(i))),
                checksum: Digest(Sha256::digest(&b).into()),
                bytes: b.len() as u64,
                sites: SiteSet(1),
            }),
        }]),
    })
}

fn row(c: ChainId, i: u64) -> Vec<u8> {
    if c == REPO { repo_row(i) } else { format!("{c}@{i}").into_bytes() }
}

#[derive(Default)]
struct Fake {
    now: RefCell<u64>,
    feed: RefCell<Vec<(String, Vec<u8>)>>,
    /// The install's log per chain (what the edge serves), from index 1.
    log: RefCell<BTreeMap<ChainId, Vec<Vec<u8>>>>,
    /// The copy per chain, and the index at which it halts.
    copy: RefCell<BTreeMap<ChainId, Vec<Vec<u8>>>>,
    halt_at: RefCell<BTreeMap<ChainId, u64>>,
    /// Packs the edge serves: the bytes, which may not be the entry's.
    served: RefCell<BTreeMap<PackId, Vec<u8>>>,
    kept: RefCell<BTreeMap<PackId, Vec<u8>>>,
    edge_calls: RefCell<Vec<(ChainId, u64)>>,
    feed_down: RefCell<bool>,
    /// The next import takes at most this many rows and halts on none.
    short: RefCell<Option<usize>>,
}

impl Fake {
    fn commit(&self, c: ChainId, to: u64) {
        let mut log = self.log.borrow_mut();
        let l = log.entry(c).or_default();
        while (l.len() as u64) < to {
            l.push(row(c, l.len() as u64 + 1));
        }
    }

    /// The feed writes one object carrying `chains` from their previous object's index.
    fn round(&self, seq: u32, chains: &[(ChainId, u64, u64)]) {
        for (c, _, to) in chains {
            self.commit(*c, *to);
        }
        let o = ExportObject {
            batch: ChangeBatch { minute: 100, seq, chains: chains.iter().map(|(c, _, to)| (*c, Index(*to))).collect() },
            entries: chains
                .iter()
                .map(|(c, from, to)| (*c, Index(*from), (*from..=*to).map(|i| row(*c, i)).collect()))
                .collect(),
        };
        self.feed.borrow_mut().push((o.batch.key().to_string(), export::encode_object(&o)));
    }
}

impl Host for Fake {
    fn now(&self) -> u64 {
        *self.now.borrow()
    }
    async fn list(&self, after: Option<&str>) -> Result<(Vec<String>, bool), String> {
        if *self.feed_down.borrow() {
            return Err("503".into());
        }
        let feed = self.feed.borrow();
        let keys = feed.iter().map(|(k, _)| k.clone()).filter(|k| after.is_none_or(|a| k.as_str() > a)).collect();
        Ok((keys, false))
    }
    async fn object(&self, key: &str) -> Result<Vec<u8>, String> {
        Ok(self.feed.borrow().iter().find(|(k, _)| k == key).unwrap().1.clone())
    }
    async fn entries(&self, chain: ChainId, from: Index, limit: u64) -> Result<Page, String> {
        self.edge_calls.borrow_mut().push((chain, from.0));
        let log = self.log.borrow();
        let l = log.get(&chain).cloned().unwrap_or_default();
        let rows: Vec<Vec<u8>> = l.into_iter().skip(from.0 as usize - 1).take(limit as usize).collect();
        Ok(Page::Rows { first: from, rows })
    }
    async fn import(&self, chain: ChainId, rows: &[Vec<u8>]) -> Result<Imported, String> {
        let mut copy = self.copy.borrow_mut();
        let c = copy.entry(chain).or_default();
        let halt = self.halt_at.borrow().get(&chain).copied();
        let rows = match self.short.borrow_mut().take() {
            Some(n) => &rows[..n],
            None => rows,
        };
        for r in rows {
            if Some(c.len() as u64 + 1) == halt {
                return Ok(Imported { held: Index(c.len() as u64), halt: Some("planted".into()) });
            }
            c.push(r.clone());
        }
        Ok(Imported { held: Index(c.len() as u64), halt: None })
    }
    async fn put_pack(&self, p: &Owed) -> Result<Copied, String> {
        let Some(b) = self.served.borrow().get(&p.id).cloned() else { return Ok(Copied::Status(404)) };
        if Sha256::digest(&b).as_slice() != p.checksum.0 {
            return Ok(Copied::Corrupt);
        }
        self.kept.borrow_mut().insert(p.id, b);
        Ok(Copied::Kept)
    }
}

fn run(h: &Fake, st: &mut State) -> pass::Report {
    futures_executor::block_on(pass::pass(h, st)).unwrap()
}

#[test]
fn the_feed_then_the_edge_with_packs_checked_and_a_halt_held() {
    let h = Fake::default();
    let mut st = State::default();
    for i in 1..=4 {
        h.served.borrow_mut().insert(PackId(Ulid(i)), pack_of(i as u64));
    }
    // Entry 3's pack is served with other bytes: never kept, still owed.
    h.served.borrow_mut().insert(PackId(Ulid(3)), b"other bytes".to_vec());

    // The feed moves: two rounds, two chains.
    *h.now.borrow_mut() = 1_000;
    h.round(0, &[(REPO, 1, 2), (IDS, 1, 3)]);
    h.round(1, &[(REPO, 3, 3)]);
    let r = run(&h, &mut st);
    assert_eq!((r.objects, r.pages, r.stale), (2, 0, false), "{r:?}");
    assert_eq!(h.copy.borrow()[&REPO].len(), 3);
    assert_eq!(st.puller.held.get(IDS), Index(3));
    assert_eq!(h.kept.borrow().len(), 2, "packs 1 and 2 kept; 3 served corrupt");
    assert!(!h.kept.borrow().contains_key(&PackId(Ulid(3))));
    assert_eq!(st.owed.len(), 1);
    assert_eq!(st.current_at, 1_000, "nothing left to pull from either source");

    // The feed stops; the install commits on. Once stale, every known chain is pulled from the edge, from the copy's
    // next index, and never earlier.
    h.commit(REPO, 4);
    h.commit(IDS, 5);
    *h.now.borrow_mut() = 1_000 + FEED_STALE_MS;
    h.edge_calls.borrow_mut().clear();
    let r = run(&h, &mut st);
    assert!(r.stale);
    assert_eq!(r.objects, 0);
    assert_eq!(h.copy.borrow()[&REPO].len(), 4);
    assert_eq!(h.copy.borrow()[&IDS].len(), 5);
    let first: Vec<(ChainId, u64)> = h.edge_calls.borrow()[..2].to_vec();
    assert_eq!(first, [(REPO, 4), (IDS, 4)], "each from its copy's next index");
    assert!(h.kept.borrow().contains_key(&PackId(Ulid(4))), "a pack a pulled row names is kept");
    assert_eq!(st.current_at, 1_000 + FEED_STALE_MS);

    // A row the copy refuses halts its chain: the copy keeps the rows before it, and no source moves it again.
    h.halt_at.borrow_mut().insert(IDS, 7);
    h.commit(IDS, 8);
    *h.now.borrow_mut() += 1;
    let r = run(&h, &mut st);
    assert_eq!(r.halted, [IDS]);
    assert_eq!(h.copy.borrow()[&IDS].len(), 6);
    assert_eq!(st.puller.held.get(IDS), Index(6));
    h.edge_calls.borrow_mut().clear();
    *h.now.borrow_mut() += 1;
    let r = run(&h, &mut st);
    assert!(h.edge_calls.borrow().iter().all(|(c, _)| *c != IDS), "a halted chain is pulled no more");
    assert!(r.halted.is_empty());
    assert_eq!(st.current_at, 1_000 + FEED_STALE_MS, "a halted copy is never current");

    // The feed goes down (R2 answers 503): the pass goes on to the edge, and nothing is lost.
    *h.feed_down.borrow_mut() = true;
    h.commit(REPO, 6);
    *h.now.borrow_mut() += 1;
    let r = run(&h, &mut st);
    assert!(r.feed_error.is_some());
    assert_eq!(h.copy.borrow()[&REPO].len(), 6, "{r:?}");
    assert_eq!(st.current_at, 1_000 + FEED_STALE_MS, "a halted copy is still never current");

    // The decoder: due with none sent, then not until a day later or another release.
    let rel = [9u8; 32];
    assert!(pass::decoder_due(&st, rel, 5));
    st.decoder_sent = Some(rel);
    st.decoder_at = 5;
    assert!(!pass::decoder_due(&st, rel, 5 + pass::DECODER_EVERY_MS - 1));
    assert!(pass::decoder_due(&st, rel, 5 + pass::DECODER_EVERY_MS));
    assert!(pass::decoder_due(&st, [8; 32], 6), "a new decoder is reported at once");

    // A copy that takes fewer rows than it is sent is behind: the edge fills it, even while the feed is fresh.
    let mut st2 = State::default();
    let h2 = Fake::default();
    *h2.now.borrow_mut() = 50;
    h2.round(0, &[(IDS, 1, 2)]);
    *h2.short.borrow_mut() = Some(1);
    let r = run(&h2, &mut st2);
    assert!(!r.stale);
    assert_eq!(h2.copy.borrow()[&IDS].len(), 2, "row 2 came from the edge: {r:?}");
    assert_eq!(h2.edge_calls.borrow().as_slice(), [(IDS, 2)]);
    assert!(st2.puller.behind.is_empty());
    assert_eq!(st2.current_at, 50);
    // A feed that does not answer before it is stale confirms nothing.
    *h2.feed_down.borrow_mut() = true;
    *h2.now.borrow_mut() = 60;
    run(&h2, &mut st2);
    assert_eq!(st2.current_at, 50);

    // The state round-trips.
    assert_eq!(State::decode(&st.encode()).unwrap(), st);
}
