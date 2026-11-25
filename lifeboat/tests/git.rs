//! The git copy's rules (`graftyard_lifeboat::git`; `decisions/DECISIONS.md`, "`lifeboat-clone`"): the shards' own
//! `catch_up` over a lifeboat's learner (`first` 1, no promotion read from the record, the copy's rolls as its only
//! markers) against a model `graft-git` whose packs are thin and a `graft-blob` that never held the packs a roll
//! consumed; and when the front serves a network.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use futures_executor::block_on;
use graftyard_git::catchup::{Fetched, IdxRange, Ingested, PIECE, Rehydrating, Run, Saved, ShardIo, catch_up};
use graftyard_git::group::RefsPage;
use graftyard_git::{Compacted, IngestCursor, LogPacks, Marker, PackId, PackRef, Promoted, StoreError};
use graftyard_lifeboat::git::{self, Facts, GitCopy, Learner, LearnerLog, Net, Ready};
use graftyard_types::{ChainId, DbId, Digest, Index, NetworkId, RepoId, SiteSet, Ulid};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const NET: NetworkId = NetworkId(DbId(9));
const CHAIN: ChainId = ChainId::Repo(RepoId(DbId(9)));

/// A model pack: the objects it brings and the ones it needs on the disk first.
#[derive(Serialize, Deserialize)]
struct Pack {
    brings: Vec<u64>,
    needs: Vec<u64>,
    pad: u8,
}

fn bytes_of(brings: Vec<u64>, needs: Vec<u64>, pad: u8) -> Vec<u8> {
    serde_json::to_vec(&Pack { brings, needs, pad }).unwrap()
}

fn sum(b: &[u8]) -> Digest {
    Digest(Sha256::digest(b).into())
}

fn stored(id: u128, b: &[u8]) -> PackRef {
    PackRef::Stored { id: PackId(Ulid(id)), checksum: sum(b), bytes: b.len() as u64, sites: SiteSet(3) }
}

/// `graft-git` and `graft-blob`.
#[derive(Default)]
struct Host {
    disk: RefCell<String>,
    objects: RefCell<BTreeSet<u64>>,
    ingested: RefCell<BTreeSet<Digest>>,
    refs: RefCell<BTreeMap<String, String>>,
    spooled: RefCell<BTreeMap<Digest, Vec<u8>>>,
    blob: RefCell<BTreeMap<PackId, Vec<u8>>>,
    fetched: RefCell<Vec<PackId>>,
    rolls: Vec<Compacted>,
}

impl Host {
    fn take(&self, s: Digest, b: &[u8]) -> Ingested {
        if self.ingested.borrow().contains(&s) {
            return Ingested::Done;
        }
        let p: Pack = serde_json::from_slice(b).unwrap();
        if !p.needs.iter().all(|o| self.objects.borrow().contains(o)) {
            return Ingested::Later;
        }
        self.objects.borrow_mut().extend(p.brings);
        self.ingested.borrow_mut().insert(s);
        Ingested::Done
    }
}

impl ShardIo for Host {
    async fn up(&self) -> Result<String, StoreError> {
        Ok(self.disk.borrow().clone())
    }
    async fn init(&self, _fresh: bool) -> Result<(), StoreError> {
        self.objects.borrow_mut().clear();
        self.ingested.borrow_mut().clear();
        self.refs.borrow_mut().clear();
        Ok(())
    }
    async fn have(&self, sums: &[Digest]) -> Result<BTreeSet<Digest>, StoreError> {
        Ok(sums.iter().filter(|s| self.ingested.borrow().contains(s)).copied().collect())
    }
    async fn ingest(&self, packs: &[(Digest, &[u8])]) -> Result<Vec<Ingested>, StoreError> {
        Ok(packs.iter().map(|(s, b)| self.take(*s, b)).collect())
    }
    /// From `graft-blob`, which serves a pack only if it holds it: a consumed pack the install deleted is not there.
    async fn fetch_stored(&self, id: PackId, checksum: Digest, _idx: bool) -> Result<Fetched, StoreError> {
        self.fetched.borrow_mut().push(id);
        let b = self.blob.borrow().get(&id).cloned().ok_or_else(|| StoreError(format!("graft-blob: no {id}")))?;
        assert_eq!(sum(&b), checksum);
        self.spooled.borrow_mut().insert(checksum, b);
        Ok(Fetched::Spooled)
    }
    async fn ingest_spooled(&self, checksum: Digest) -> Result<Ingested, StoreError> {
        let b = self.spooled.borrow().get(&checksum).cloned().ok_or_else(|| StoreError("not spooled".into()))?;
        Ok(self.take(checksum, &b))
    }
    async fn idx_ranges(&self) -> Result<BTreeMap<PackId, Option<IdxRange>>, StoreError> {
        Ok(BTreeMap::new())
    }
    async fn connected(&self, _repack: bool) -> Result<Result<(), String>, StoreError> {
        let objs = self.objects.borrow();
        Ok(match objs.iter().find(|o| **o > 1 && !objs.contains(&(**o - 1))) {
            Some(o) => Err(format!("missing {}", o - 1)),
            None => Ok(()),
        })
    }
    async fn put_refs_page(
        &self,
        refs: &[(String, String)],
        start: bool,
        _done: bool,
        _repack: bool,
        _head: Option<&str>,
    ) -> Result<(), StoreError> {
        let mut r = self.refs.borrow_mut();
        if start {
            r.clear();
        }
        r.extend(refs.iter().cloned());
        Ok(())
    }
    async fn apply_refs(
        &self,
        changes: &[(String, Option<String>)],
        _head: Option<&str>,
        _repack: bool,
    ) -> Result<(), StoreError> {
        let mut r = self.refs.borrow_mut();
        for (n, o) in changes {
            match o {
                Some(o) => r.insert(n.clone(), o.clone()),
                None => r.remove(n),
            };
        }
        Ok(())
    }
    async fn make_room(&self, _incoming: u64) -> Result<Vec<i64>, StoreError> {
        Ok(Vec::new())
    }
    async fn markers(&self) -> Result<Vec<(PackId, Marker)>, StoreError> {
        Ok(git::roll_markers(CHAIN, &self.rolls))
    }
}

#[derive(Default)]
struct Store {
    cursor: RefCell<Option<(String, IngestCursor)>>,
    rehydrating: RefCell<Option<(String, Rehydrating)>>,
}

impl Saved for Store {
    async fn cursor(&self) -> Result<Option<(String, IngestCursor)>, StoreError> {
        Ok(self.cursor.borrow().clone())
    }
    async fn put_cursor(&self, disk: &str, c: &IngestCursor) -> Result<(), StoreError> {
        *self.cursor.borrow_mut() = Some((disk.into(), c.clone()));
        Ok(())
    }
    async fn drop_cursor(&self) -> Result<(), StoreError> {
        *self.cursor.borrow_mut() = None;
        Ok(())
    }
    async fn rehydrating(&self) -> Result<Option<(String, Rehydrating)>, StoreError> {
        Ok(self.rehydrating.borrow().clone())
    }
    async fn put_rehydrating(&self, disk: &str, r: &Rehydrating) -> Result<(), StoreError> {
        *self.rehydrating.borrow_mut() = Some((disk.into(), r.clone()));
        Ok(())
    }
    async fn drop_rehydrating(&self) -> Result<(), StoreError> {
        *self.rehydrating.borrow_mut() = None;
        Ok(())
    }
}

/// The copy's member: every entry from 1, in pages of 7, and its fold's rolls.
struct Copy(Vec<LogPacks>, Vec<Compacted>);

impl Learner for Copy {
    async fn entries(&self, _c: ChainId, from: Index, to: Index) -> Result<Vec<LogPacks>, StoreError> {
        Ok(self.0.iter().filter(|e| e.index >= from && e.index <= to).take(7).cloned().collect())
    }
    async fn refs(&self, _c: ChainId, after: Option<String>) -> Result<(RefsPage, Index), StoreError> {
        let mut all = BTreeMap::new();
        for e in &self.0 {
            for (n, o) in e.refs.clone().unwrap_or_default() {
                match o {
                    Some(o) => all.insert(n, o),
                    None => all.remove(&n),
                };
            }
        }
        let refs = all.into_iter().filter(|(n, _)| after.as_ref().is_none_or(|a| n > a)).collect();
        Ok((RefsPage { refs, next: None }, Index(self.0.len() as u64)))
    }
    async fn ranges(&self, _c: ChainId) -> Result<(Index, Option<String>, Vec<Compacted>), StoreError> {
        Ok((Index(self.0.len() as u64), None, self.1.clone()))
    }
}

/// A repository's log as the copy holds it: pushes 1..=30, each a thin pack building on the one before, inline but
/// every fifth stored; promotions of 1..=10 and 11..=20, committed at entries 31 and 32; and a roll of both at 33. The
/// install deleted the consumed promotions after `CONSUMED_KEEP`, so `graft-blob` never held them.
fn rolled_network() -> (Host, Copy, PackId, [PackId; 2]) {
    let mut host = Host::default();
    *host.disk.borrow_mut() = "disk-1".into();
    let mut log = Vec::new();
    for i in 1..=30u64 {
        let b = bytes_of(vec![i], if i > 1 { vec![i - 1] } else { vec![] }, 0);
        let pack = if i % 5 == 0 {
            host.blob.borrow_mut().insert(PackId(Ulid(i.into())), b.clone());
            stored(i.into(), &b)
        } else {
            PackRef::Inline(b)
        };
        log.push(LogPacks {
            index: Index(i),
            packs: vec![pack],
            promoted: None,
            compacted: None,
            consumed: Vec::new(),
            refs: Some(vec![(format!("refs/heads/b{}", i % 3), Some(format!("{i:040x}")))]),
        });
    }
    let promo = |from: u64, to: u64| {
        let b = bytes_of((from..=to).collect(), if from > 1 { vec![from - 1] } else { vec![] }, 0);
        (
            PackId(Ulid(1_000 + u128::from(to))),
            Promoted { from: Index(from), to: Index(to), pack: stored(1_000 + u128::from(to), &b) },
        )
    };
    let (p1, pr1) = promo(1, 10);
    let (p2, pr2) = promo(11, 20);
    for (at, p) in [(31, pr1), (32, pr2)] {
        log.push(LogPacks {
            index: Index(at),
            packs: vec![],
            promoted: Some(p),
            compacted: None,
            consumed: Vec::new(),
            refs: Some(vec![]),
        });
    }
    let rb = bytes_of((1..=20).collect(), vec![], 1);
    let rolled = PackId(Ulid(2_020));
    host.blob.borrow_mut().insert(rolled, rb.clone());
    let roll = Compacted { consumed: vec![p1, p2], produced: stored(2_020, &rb), from: Index(1), to: Index(20) };
    host.rolls = vec![roll.clone()];
    // `/entries` names no roll (`Family::entry_json`); the fold's `ranges` does.
    log.push(LogPacks {
        index: Index(33),
        packs: vec![],
        promoted: None,
        compacted: None,
        consumed: Vec::new(),
        refs: Some(vec![]),
    });
    (host, Copy(log, vec![roll]), rolled, [p1, p2])
}

fn run(host: &Host, store: &Store, copy: &Copy) -> Result<graftyard_git::catchup::Report, StoreError> {
    let log = LearnerLog { learner: copy };
    block_on(catch_up(host, store, &log, NET, &[CHAIN], PIECE, &mut Run::default(), &|| 0))
}

/// A copy enrolled after the install deleted a roll's consumed packs still rebuilds: the produced pack loads first, in
/// their place, no consumed pack is ever asked of `graft-blob`, and the refs are the copy's. Without the rolls as
/// markers the rehydrate's tail asks for the consumed promotions at entries 31 and 32 and fails on the first.
#[test]
fn a_rolled_network_rebuilds_without_the_packs_its_roll_consumed() {
    let (host, copy, rolled, consumed) = rolled_network();
    let store = Store::default();
    let mut held = run(&host, &store, &copy).unwrap_or_else(|e| panic!("{}", e.0));
    while held.behind {
        held = run(&host, &store, &copy).unwrap_or_else(|e| panic!("{}", e.0));
    }
    let r = held.rehydrated.as_ref().or(None);
    let fetched = host.fetched.borrow().clone();
    assert_eq!(fetched.first(), Some(&rolled), "the produced pack loads first: {fetched:?}");
    assert!(fetched.iter().all(|id| !consumed.contains(id)), "a consumed pack was asked for: {fetched:?}");
    assert!(r.is_none_or(|h| h.dropped_rolls.is_empty()));
    assert_eq!(*host.objects.borrow(), (1..=30).collect::<BTreeSet<u64>>());
    assert_eq!(host.refs.borrow().len(), 3);
    assert_eq!(host.refs.borrow()["refs/heads/b0"], format!("{:040x}", 30));
    assert_eq!(store.cursor.borrow().as_ref().unwrap().1.at(CHAIN), Index(33));
    assert!(store.rehydrating.borrow().is_none(), "the rebuild finished");

    // The same log with no roll in the fold: the consumed promotions are owed, and the rebuild stops on the first.
    let (mut host, mut copy, _, consumed) = rolled_network();
    host.rolls.clear();
    copy.1.clear();
    let e = run(&host, &Store::default(), &copy).map(|_| ()).unwrap_err();
    assert!(e.0.contains(&consumed[0].to_string()), "{}", e.0);
}

/// A new disk (another `graft-git` directory) rehydrates from the copy alone; the same disk goes on from its cursor,
/// fetching only the stored packs past it.
#[test]
fn a_new_disk_rebuilds_from_the_copy_and_the_same_disk_goes_on() {
    let (mut host, full, _, _) = rolled_network();
    let rolls = std::mem::take(&mut host.rolls);
    // The copy at 20, before the promotions and the roll.
    let early = Copy(full.0[..20].to_vec(), Vec::new());
    let store = Store::default();
    while run(&host, &store, &early).unwrap().behind {}
    assert_eq!(store.cursor.borrow().as_ref().unwrap().1.at(CHAIN), Index(20));
    let before = host.fetched.borrow().len();
    host.rolls = rolls;
    while run(&host, &store, &full).unwrap().behind {}
    assert_eq!(store.cursor.borrow().as_ref().unwrap().1.at(CHAIN), Index(33));
    assert_eq!(host.fetched.borrow()[before..], [PackId(Ulid(25)), PackId(Ulid(30))], "only the packs past the cursor");
    // A new disk id: everything again, from the copy.
    *host.disk.borrow_mut() = "disk-2".into();
    host.objects.borrow_mut().clear();
    host.ingested.borrow_mut().clear();
    host.refs.borrow_mut().clear();
    let r = run(&host, &store, &full).unwrap();
    assert!(r.rehydrated.is_some());
    while run(&host, &store, &full).unwrap().behind {}
    assert_eq!(*host.objects.borrow(), (1..=30).collect::<BTreeSet<u64>>());
    assert_eq!(store.cursor.borrow().as_ref().unwrap().0, "disk-2");
}

/// When the front serves a network's git: only with its cursor on `graft-git`'s disk, no rebuild saved, and checked on
/// `graft-git`'s current run. A clone in any other state gets 503, never an empty or short pack.
#[test]
fn a_network_serves_only_whole_and_checked_on_this_run() {
    let net = Net { served: Some((Index(40), 1_000)), error: None, failed_at: 0, checked: Some("run-1".into()) };
    let ok = Facts {
        health: Some(("disk-1".into(), "run-1".into())),
        cursor_disk: Some("disk-1".into()),
        rehydrating: false,
        halted: false,
    };
    assert_eq!(git::ready(Some(&net), &ok, Index(40), 5_000), Ready::Ready { at: Index(40), lag_ms: 0 });
    assert_eq!(git::ready(Some(&net), &ok, Index(41), 5_000), Ready::Ready { at: Index(40), lag_ms: 4_000 });
    let not = |f: Facts| matches!(git::ready(Some(&net), &f, Index(40), 5_000), Ready::NotReady { retry_s, .. } if retry_s > 0);
    assert!(not(Facts { rehydrating: true, ..ok.clone() }), "a rebuild in progress");
    assert!(not(Facts { cursor_disk: None, ..ok.clone() }), "a rebuild dropped the cursor before its /init");
    assert!(not(Facts { cursor_disk: Some("disk-0".into()), ..ok.clone() }), "a cursor for another disk");
    assert!(not(Facts { health: Some(("disk-1".into(), "run-2".into())), ..ok.clone() }), "not checked on this run");
    assert!(not(Facts { health: None, ..ok.clone() }), "graft-git down");
    assert_eq!(git::ready(Some(&net), &Facts { halted: true, ..ok.clone() }, Index(40), 5_000), Ready::Halted);
    assert!(matches!(git::ready(None, &ok, Index(40), 5_000), Ready::NotReady { .. }), "never caught up");
}

/// Which networks a git turn takes: moved past what was served, never served, or not checked on this run; never one
/// still owed a pack, and one that failed waits `GIT_RETRY_MS`; from the one after the last turn.
#[test]
fn a_turn_takes_moved_networks_round_and_waits_out_owed_and_failed_ones() {
    let n = |i: i64| NetworkId(DbId(i));
    let held: BTreeMap<NetworkId, Index> = (1..=5).map(|i| (n(i), Index(10))).collect();
    let mut g = GitCopy::default();
    let served = |at: u64| Net { served: Some((Index(at), 0)), checked: Some("r".into()), ..Net::default() };
    g.nets.insert(n(1), served(10));
    g.nets.insert(n(2), served(9));
    g.nets.insert(n(3), Net { checked: Some("old".into()), ..served(10) });
    g.nets.insert(n(4), Net { failed_at: 100, ..served(9) });
    g.turn = Some(n(2));
    let waiting = BTreeSet::from([n(5)]);
    assert_eq!(g.due(&held, &waiting, "r", 100 + git::GIT_RETRY_MS - 1), [n(3), n(2)]);
    assert_eq!(g.due(&held, &waiting, "r", 100 + git::GIT_RETRY_MS), [n(3), n(4), n(2)]);
    assert_eq!(g.due(&held, &BTreeSet::new(), "r", 100 + git::GIT_RETRY_MS), [n(3), n(4), n(5), n(2)]);
}
