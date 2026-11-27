//! The puller's pass against a fake install and copy: the feed, then the edge once the feed is stale, packs owed by
//! repository rows and kept only when they hash to the entry's checksum, and a halted chain moved by nothing again.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use graftyard_git::export::{self, ChangeBatch, ExportObject};
use graftyard_git::pack::{PackId, PackRef};
use graftyard_git::repo::{REPO_FORMAT_V1, RepoEvent};
use graftyard_lifeboat::pass::{self, Copied, Host, Imported, Owed, Page, State};
use graftyard_lifeboat::puller::FEED_STALE_MS;
use graftyard_raft::{EntryBody, LogEntry, Term, TierSet, codec};
use graftyard_types::{
    AccountId, ChainId, DbId, Digest, Index, NetworkId, RepoId, RequestId, SiteSet, Timestamp, Ulid,
};
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
    /// Chains the edge answers 404 (no group: retired or purged), and ones it answers 503.
    gone: RefCell<BTreeSet<ChainId>>,
    down: RefCell<BTreeSet<ChainId>>,
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
        if self.gone.borrow().contains(&chain) {
            return Ok(Page::Status(404));
        }
        if self.down.borrow().contains(&chain) {
            return Ok(Page::Status(503));
        }
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

/// A chain the edge answers 404 has no group (retired or purged): it is asked once and never again, and it does not
/// hold the copy back from current. A chain the edge answers 503 is asked again on the next pass. Before, a retired chain
/// answered 503 "retry", and a stale feed asked for it on every pass for good (graft-m3f `repo:65577`, R59's read).
#[test]
fn a_chain_the_edge_answers_404_is_asked_no_more() {
    let h = Fake::default();
    let mut st = State::default();
    *h.now.borrow_mut() = 1_000;
    let other = ChainId::Names(graftyard_types::NameShard(3));
    h.round(0, &[(IDS, 1, 2), (other, 1, 1)]);
    run(&h, &mut st);

    // The feed stops. `IDS` was retired at the install; `other` is down for now.
    h.gone.borrow_mut().insert(IDS);
    h.down.borrow_mut().insert(other);
    *h.now.borrow_mut() = 1_000 + FEED_STALE_MS;
    let r = run(&h, &mut st);
    assert_eq!(r.gone, [IDS]);
    assert_eq!(h.copy.borrow()[&IDS].len(), 2, "the copy keeps what it holds");

    h.edge_calls.borrow_mut().clear();
    *h.now.borrow_mut() += 1;
    let r = run(&h, &mut st);
    assert!(r.gone.is_empty());
    assert_eq!(h.edge_calls.borrow().as_slice(), [(other, 2)], "the gone chain is asked no more; the down one is");

    // The gone chain holds nothing back: the copy is current though the gone chain's copy is short of the log, as of
    // the start of the cycle in which every chain answered (the pass before, whose cycle the down chain spoiled).
    h.down.borrow_mut().clear();
    h.commit(IDS, 4);
    *h.now.borrow_mut() += 1;
    run(&h, &mut st);
    assert_eq!(st.current_at, *h.now.borrow() - 1);
    assert_eq!(h.copy.borrow()[&IDS].len(), 2);
    assert_eq!(State::decode(&st.encode()).unwrap(), st);
}

/// `lifeboat-turns`: with the feed stale, the edge pulls go round every chain on a turn, so 900 chains all catch up
/// although one pass takes 400 empty answers at most, and the copy is current only once a whole cycle answered, as of
/// that cycle's start. Before, every pass began at the first chain in key order and took 40 pages, so the rest of a
/// large install never moved and the copy was never current.
#[test]
fn a_stale_round_reaches_every_chain_and_is_current_once_a_cycle_ends() {
    let h = Fake::default();
    let mut st = State::default();
    *h.now.borrow_mut() = 1_000;
    let chains: Vec<ChainId> = (0..900u16).map(|i| ChainId::Names(graftyard_types::NameShard(i))).collect();
    let all: Vec<(ChainId, u64, u64)> = chains.iter().map(|c| (*c, 1, 1)).collect();
    h.round(0, &all);
    run(&h, &mut st);
    assert_eq!(st.current_at, 1_000);

    // The feed stops. One chain at the end of the key order moves: a pass that always started at the first key would
    // never reach it.
    let tail = *chains.last().unwrap();
    h.commit(tail, 3);
    let start = 1_000 + FEED_STALE_MS;
    let mut passes = 0;
    while st.puller.held.get(tail) < Index(3) {
        passes += 1;
        assert!(passes <= 3, "the turn reached the last chain within ceil(900 / PROBES_PER_PASS) passes");
        *h.now.borrow_mut() = start + passes;
        let r = run(&h, &mut st);
        assert!(r.probes <= pass::PROBES_PER_PASS + pass::AT_ONCE, "{} probes", r.probes);
    }
    assert_eq!(passes, 3);
    assert_eq!(st.current_at, start + 1, "current as of the cycle that started at the first stale pass");
    // Each chain is asked once a cycle, never twice in a row before the others.
    let asked: Vec<ChainId> = h.edge_calls.borrow().iter().map(|(c, _)| *c).collect();
    let mut seen = std::collections::BTreeSet::new();
    assert!(asked.iter().take(900).all(|c| seen.insert(*c)), "one cycle asks each chain once");

    // A chain the edge cannot serve (503) spoils its cycle: the copy is not current as of that cycle's start, and is
    // again once a later cycle answers whole.
    let mid = chains[450];
    h.down.borrow_mut().insert(mid);
    let held_at = st.current_at;
    let cycle_start = st.puller.cycle.unwrap();
    for k in 1..=3 {
        *h.now.borrow_mut() = start + 3 + k;
        run(&h, &mut st);
    }
    assert_eq!(st.current_at, held_at, "the cycle {cycle_start} had a chain that did not answer");
    h.down.borrow_mut().clear();
    for k in 4..=9 {
        *h.now.borrow_mut() = start + 3 + k;
        run(&h, &mut st);
    }
    assert!(st.current_at > held_at, "a whole cycle answered once the chain was back");
    assert!(st.current_at < *h.now.borrow(), "as of a cycle's start, not the pass that ended it");
}

/// `lifeboat-turns`: owed packs go round by network. A network whose packs the install no longer serves and whose
/// chain the edge answers 404 is dropped after one probe; a pack unserved on a live chain stays owed and counts toward
/// the alarm; and neither holds back another network's pack. Before, 41 dead packs ahead of a live one in key order
/// took every pass's 40 copies for good.
#[test]
fn owed_packs_go_round_by_network_and_a_gone_networks_are_dropped() {
    let h = Fake::default();
    let mut st = State::default();
    *h.now.borrow_mut() = 1_000;
    let net = |n: i64| graftyard_types::NetworkId(DbId(n));
    let owed = |n: i64, i: u128, b: &[u8]| Owed {
        network: net(n),
        id: PackId(Ulid(i)),
        checksum: Digest(Sha256::digest(b).into()),
        bytes: b.len() as u64,
    };
    // Network 1: 45 packs of a purged repository; network 2: one pack unserved on a live chain; network 3: one live.
    for i in 1..=45 {
        st.owed.insert(owed(1, i, b"gone"));
    }
    h.gone.borrow_mut().insert(ChainId::Repo(RepoId(DbId(1))));
    st.owed.insert(owed(2, 200, b"missing"));
    st.owed.insert(owed(3, 300, b"live"));
    h.served.borrow_mut().insert(PackId(Ulid(300)), b"live".to_vec());
    let r = run(&h, &mut st);
    assert!(h.kept.borrow().contains_key(&PackId(Ulid(300))), "the live pack is copied on the first pass");
    assert!(st.owed.iter().all(|o| o.network != net(1)), "a gone network's packs are dropped");
    let probes = h.edge_calls.borrow().iter().filter(|(c, _)| *c == ChainId::Repo(RepoId(DbId(1)))).count();
    assert_eq!(probes, 1, "one probe of the gone chain, not one per pack");
    assert_eq!(r.gone, [ChainId::Repo(RepoId(DbId(1)))]);
    assert_eq!(st.owed.len(), 1, "the pack unserved on a live chain stays owed");
    assert_eq!((r.unserved, r.unserved_alarm), (1, 0));
    *h.now.borrow_mut() = 1_000 + pass::UNSERVED_ALARM_MS;
    let r = run(&h, &mut st);
    assert_eq!((r.unserved, r.unserved_alarm), (1, 1), "unserved for an hour on a live chain alarms");
    assert_eq!(State::decode(&st.encode()).unwrap(), st);
}

/// The weekly scrub: due with none run, not again for a week after one answered, an hour after one that did not;
/// and graft-blob's answer read so a key no sibling can repair names the copy to pull it from again.
#[test]
fn the_scrub_is_weekly_and_a_lost_key_names_its_copy() {
    let mut st = State::default();
    let t = 10 * pass::SCRUB_EVERY_MS;
    assert!(pass::scrub_due(&st, t), "never scrubbed");
    // Tried, no answer: again an hour later, not before.
    st.scrub_tried = t;
    assert!(!pass::scrub_due(&st, t + pass::SCRUB_RETRY_MS - 1));
    assert!(pass::scrub_due(&st, t + pass::SCRUB_RETRY_MS));
    // Answered: a week later, not after the hour.
    st.scrub_at = t;
    assert!(!pass::scrub_due(&st, t + pass::SCRUB_RETRY_MS));
    assert!(!pass::scrub_due(&st, t + pass::SCRUB_EVERY_MS - 1));
    assert!(pass::scrub_due(&st, t + pass::SCRUB_EVERY_MS));

    let r = pass::scrubbed(
        "pack/12/01JAAAAAAAAAAAAAAAAAAAAAAA ok\npack/34/01JBBBBBBBBBBBBBBBBBBBBBBB lost\npack/12/01JC repaired\n\
         pack/5/01JD restored\nother/x error\n",
    );
    assert_eq!((r.ok, r.repaired, r.restored), (1, 1, 1));
    let lost: Vec<_> = r.lost.iter().map(|l| (l.key.as_str(), l.outcome.as_str(), l.copy)).collect();
    assert_eq!(
        lost,
        [
            ("pack/34/01JBBBBBBBBBBBBBBBBBBBBBBB", "lost", Some(ChainId::Repo(RepoId(DbId(34))))),
            ("other/x", "error", None)
        ]
    );
}

/// The guard for rolls (`lifeboat-clone`): a copy that took a roll's entry owes its produced pack, and the packs the
/// roll consumed, which the install deletes `CONSUMED_KEEP` after the roll, are owed no more once the produced pack is
/// held. Before, a copy enrolled later owed them for good, alarmed on a live chain, and held its git copy back.
#[test]
fn a_held_rolls_consumed_packs_are_owed_no_more() {
    use graftyard_git::pack::Compacted;
    let h = Fake::default();
    let mut st = State::default();
    *h.now.borrow_mut() = 1_000;
    // Entries 1 and 2 push packs 1 and 2; the install deleted both after rolling them into pack 9 at entry 3.
    h.round(0, &[(REPO, 1, 2)]);
    let rolled = b"the rolled pack".to_vec();
    let roll = Compacted {
        consumed: vec![PackId(Ulid(1)), PackId(Ulid(2))],
        produced: PackRef::Stored {
            id: PackId(Ulid(9)),
            checksum: Digest(Sha256::digest(&rolled).into()),
            bytes: rolled.len() as u64,
            sites: SiteSet(1),
        },
        from: Index(1),
        to: Index(2),
    };
    let row3 = codec::to_bytes(&LogEntry {
        term: Term(1),
        index: Index(3),
        prev: Digest([0; 32]),
        format: graftyard_git::repo::REPO_FORMAT_ROLLUP,
        tiers: TierSet::CORE,
        at: Timestamp(1),
        requests: Default::default(),
        extend: None,
        body: EntryBody::Events(vec![RepoEvent::Compacted(roll)]),
    });
    h.log.borrow_mut().get_mut(&REPO).unwrap().push(row3.clone());
    let o = ExportObject {
        batch: ChangeBatch { minute: 100, seq: 1, chains: vec![(REPO, Index(3))] },
        entries: vec![(REPO, Index(3), vec![row3])],
    };
    h.feed.borrow_mut().push((o.batch.key().to_string(), export::encode_object(&o)));
    // Pack 9 is not served yet: every pack stays owed, and the consumed ones count as unserved on a live chain.
    let r = run(&h, &mut st);
    assert_eq!((r.owed, r.rolled_off), (3, 0), "{r:?}");
    assert_eq!(r.unserved, 3);
    // Served and held: the consumed packs go with it, and nothing is left to alarm.
    h.served.borrow_mut().insert(PackId(Ulid(9)), rolled);
    *h.now.borrow_mut() += 1;
    let r = run(&h, &mut st);
    assert_eq!((r.owed, r.rolled_off, r.unserved), (0, 2, 0), "{r:?}");
    assert!(h.kept.borrow().contains_key(&PackId(Ulid(9))));
    assert!(st.rolls.is_empty());
    assert_eq!(State::decode(&st.encode()).unwrap(), st);
}

/// A fork's packs are owed under its network, which its chain's `Genesis` names (AM3-WP2a), never under its own id;
/// a pack owed under the fork's own id before its `Genesis` arrived (a later row first) moves to the network.
#[test]
fn a_fork_s_packs_are_owed_under_its_network_from_its_genesis() {
    let fork = ChainId::Repo(RepoId(DbId(9)));
    let net = NetworkId::of_root(RepoId(DbId(7)));
    let genesis = codec::to_bytes(&LogEntry {
        term: Term(1),
        index: Index(1),
        prev: Digest([0; 32]),
        format: graftyard_raft::Format { family: 0x20, version: 31 },
        tiers: TierSet::CORE,
        at: Timestamp(1),
        requests: Default::default(),
        extend: None,
        body: EntryBody::Events(vec![RepoEvent::Genesis {
            owner: AccountId(DbId(1)),
            network: net,
            name: serde_json::from_str("\"fork\"").unwrap(),
            fork_of: Some((RepoId(DbId(7)), Index(2))),
        }]),
    });
    let owed = |i: u64, network| {
        let b = pack_of(i);
        Owed {
            network,
            id: PackId(Ulid(u128::from(i))),
            checksum: Digest(Sha256::digest(&b).into()),
            bytes: b.len() as u64,
        }
    };
    let mut st = State::default();
    assert_eq!(pass::owed_by(&mut st, fork, &[genesis.clone(), repo_row(2)]), vec![owed(2, net)]);
    assert_eq!(st.networks, BTreeMap::from([(RepoId(DbId(9)), net)]));
    assert_eq!(pass::owed_by(&mut st, fork, &[repo_row(3)]), vec![owed(3, net)]);

    let own = NetworkId::of_root(RepoId(DbId(9)));
    let mut st = State::default();
    let early = pass::owed_by(&mut st, fork, &[repo_row(3)]);
    assert_eq!(early, vec![owed(3, own)], "a root, as far as the copy knows");
    st.owed.extend(early);
    assert_eq!(pass::owed_by(&mut st, fork, &[genesis, repo_row(2)]), vec![owed(2, net)]);
    assert_eq!(st.owed, BTreeSet::from([owed(3, net)]));

    // A root's rows are owed under its own id, and its state is written without `networks`.
    let mut st = State::default();
    assert_eq!(pass::owed_by(&mut st, REPO, &[repo_row(4)]), vec![owed(4, NetworkId::of_root(RepoId(DbId(7))))]);
    assert!(!String::from_utf8(st.encode()).unwrap().contains("networks"));
}
