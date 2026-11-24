//! The vault Worker's pass (`graftyard_vault::pass`) against a model install served by the export endpoints' own rules
//! (`graftyard_git::export::serve`), with a vault bucket that enforces 35-day bucket locks: every chain's every entry is
//! kept exactly once, `Release` and `Ids` included, every stored pack and snapshot an entry names is copied and
//! checked, chains first carried past index 1 are filled, and a pack is deleted only `RESTORE_WINDOW` after the
//! install's GC deleted it.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use futures_executor::block_on;
use graftyard_git::export::{self, ChangeBatch, ExportObject, ExportSource, FeedPosition, FeedState, Route, StoredLog};
use graftyard_git::pack::{Archived, Compacted, PackId, PackRef, Promoted};
use graftyard_git::repo::{REPO_FORMAT_V1, RepoEvent};
use graftyard_git::{CONSUMED_KEEP, R2Key};
use graftyard_raft::{EntryBody, LogEntry, Term, TierSet, codec};
use graftyard_types::{
    AccountId, ChainId, DbId, Digest, Index, NetworkId, RepoId, RequestId, SiteSet, Tier, Timestamp, TokenShard, Ulid,
};
use graftyard_vault::{
    Copied, Got, Held, Host, LOCK_DAYS, PackCheck, SnapPart, SnapshotReader, Taken, VaultState, pass, put_once, taken,
};
use sha2::{Digest as _, Sha256};
use smallvec::SmallVec;

const DAY: u64 = 86_400_000;
const T0: u64 = 1_800_000_000_000;

fn repo(n: i64) -> ChainId {
    ChainId::Repo(RepoId(DbId(n)))
}

fn sum(b: &[u8]) -> Digest {
    Digest(Sha256::digest(b).into())
}

fn entry(i: u64, at: u64, events: Vec<RepoEvent>) -> Vec<u8> {
    codec::to_bytes(&LogEntry {
        term: Term(1),
        index: Index(i),
        prev: Digest([0; 32]),
        format: REPO_FORMAT_V1,
        tiers: TierSet::CORE,
        at: Timestamp(at),
        requests: SmallVec::new(),
        extend: None,
        body: EntryBody::Events(events),
    })
}

fn stored(id: PackId, bytes: &[u8]) -> PackRef {
    PackRef::Stored { id, checksum: sum(bytes), bytes: bytes.len() as u64, sites: SiteSet(7) }
}

fn push(i: u64, pack: Option<PackRef>) -> RepoEvent {
    RepoEvent::Refs { push: RequestId(Ulid(u128::from(i))), actor: None, updates: Vec::new(), pack }
}

/// The install, as its followers, buckets, and feed hold it. `first` is a chain's first stored index (archived and
/// dropped below it unless `archived`).
#[derive(Default)]
struct Install {
    logs: BTreeMap<ChainId, Vec<Vec<u8>>>,
    first: BTreeMap<ChainId, (u64, bool)>,
    objects: BTreeMap<FeedPosition, Vec<u8>>,
    state: FeedState,
    snapshots: BTreeMap<R2Key, Vec<u8>>,
    /// Pack bytes by id, its network, and the bytes it serves (another value: a corrupt copy).
    packs: BTreeMap<PackId, (NetworkId, Vec<u8>)>,
    /// Packs marked committed whose bytes are in no bucket: 410.
    lost: BTreeSet<PackId>,
}

impl Install {
    fn add(&mut self, chain: ChainId, row: Vec<u8>) {
        self.logs.entry(chain).or_default().push(row);
    }

    fn fill(&mut self, chain: ChainId, n: usize) {
        for _ in 0..n {
            let i = self.logs.get(&chain).map_or(0, Vec::len) + 1;
            self.add(chain, format!("{chain}#{i}").into_bytes());
        }
    }

    /// One feed round, as the consumer runs it: plan from every chain's commit, read entries from each chain's first
    /// stored index after its cursor, check, write.
    fn feed(&mut self, now_ms: u64) {
        let commits: Vec<(ChainId, Index)> = self.logs.iter().map(|(c, l)| (*c, Index(l.len() as u64))).collect();
        let Some(batch) = export::plan(&self.state, &commits, now_ms) else { return };
        let entries = batch
            .chains
            .iter()
            .map(|(c, to)| {
                let first = self.first.get(c).map_or(1, |f| f.0);
                let from = (self.state.cursors.get(*c).0 + 1).max(first);
                (*c, Index(from), self.logs[c][from as usize - 1..to.0 as usize].to_vec())
            })
            .collect();
        let o = ExportObject { batch: batch.clone(), entries };
        export::check_object(&o, &self.state.cursors).unwrap();
        self.objects.insert(batch.position(), export::encode_object(&o));
        self.state.advance(&batch);
    }
}

impl ExportSource for Install {
    fn objects_after(&self, after: FeedPosition, n: usize) -> Vec<Vec<u8>> {
        use std::ops::Bound::{Excluded, Unbounded};
        self.objects.range((Excluded(after), Unbounded)).take(n).map(|(_, o)| o.clone()).collect()
    }
    fn log(&self, chain: ChainId, from: Index, limit: u64) -> Option<StoredLog> {
        let l = self.logs.get(&chain)?;
        let (first, archived) = self.first.get(&chain).copied().unwrap_or((1, false));
        let start = if archived { from.0 } else { from.0.max(first) };
        let rows = (start..=l.len() as u64).take(limit as usize + 1).map(|i| (Index(i), l[i as usize - 1].clone()));
        Some(StoredLog { first: Index(first), archived, to: Index(l.len() as u64), rows: rows.collect() })
    }
    fn snapshots(&self, chain: ChainId) -> Vec<R2Key> {
        self.snapshots
            .keys()
            .filter(|k| matches!(k, R2Key::Snapshot { chain: c, .. } if *c == chain))
            .cloned()
            .collect()
    }
    fn object(&self, key: &R2Key) -> Option<Vec<u8>> {
        self.snapshots.get(key).cloned()
    }
    fn pack(&self, network: NetworkId, id: PackId) -> Option<(Digest, Vec<u8>)> {
        // The marker's checksum is the entry's; the served bytes may differ (a corrupt copy).
        self.packs.get(&id).filter(|p| p.0 == network).map(|p| (Digest([0; 32]), p.1.clone()))
    }
}

/// An object in the vault's bucket: its bytes, its write time, and its `sha256` metadata.
type Stored = (Vec<u8>, u64, Option<String>);

/// The vault account: its bucket with 35-day lock rules on every prefix but `state/`, and the install it reaches over
/// HTTPS under an install-scoped token (`revoked` answers 401).
struct Vault<'a> {
    inst: &'a RefCell<Install>,
    now: Cell<u64>,
    /// Each object's bytes, its write time, and its `sha256` metadata.
    bucket: RefCell<BTreeMap<String, Stored>>,
    /// Create-only puts refused, by a lock (409) or by the precondition.
    refused_puts: Cell<u32>,
    etag: Cell<u64>,
    revoked: Cell<bool>,
    /// Revoked after the pass's batches page, before its copies: pack reads answer 401.
    revoked_packs: Cell<bool>,
    /// A pass that meets another: the next state write finds the etag moved.
    race: Cell<bool>,
    refused_deletes: Cell<u32>,
    probes: RefCell<Vec<String>>,
    /// The network of each pack copy tried, in order.
    copied: RefCell<Vec<NetworkId>>,
}

impl<'a> Vault<'a> {
    fn new(inst: &'a RefCell<Install>) -> Vault<'a> {
        Vault {
            inst,
            now: Cell::new(T0),
            bucket: RefCell::default(),
            refused_puts: Cell::new(0),
            etag: Cell::new(0),
            revoked: Cell::new(false),
            revoked_packs: Cell::new(false),
            race: Cell::new(false),
            refused_deletes: Cell::new(0),
            probes: RefCell::default(),
            copied: RefCell::default(),
        }
    }

    fn serve(&self, path_and_query: &str) -> Result<export::ExportReply, u16> {
        if self.revoked.get() {
            return Err(401);
        }
        let (path, q) = path_and_query.split_once('?').unwrap_or((path_and_query, ""));
        let q: Vec<(String, String)> = q
            .split('&')
            .filter(|s| !s.is_empty())
            .map(|kv| kv.split_once('=').unwrap())
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        let route = Route::parse("GET", &format!("{}/{path}", export::EXPORT_PREFIX), &q).map_err(|e| e.status())?;
        export::serve(&route, &|_| true, &*self.inst.borrow(), self.now.get(), "https://git.acme.example")
            .map_err(|e| e.status())
    }

    fn keys(&self, prefix: &str) -> Vec<String> {
        self.bucket.borrow().keys().filter(|k| k.starts_with(prefix)).cloned().collect()
    }

    fn state(&self) -> VaultState {
        VaultState::decode(&self.bucket.borrow()["state/vault"].0).unwrap()
    }

    fn run(&self) -> graftyard_vault::Report {
        block_on(pass(self)).unwrap().0
    }

    /// Passes until one has nothing left.
    fn drain(&self) {
        for _ in 0..50 {
            if !self.run().more {
                return;
            }
        }
        panic!("the vault never caught up");
    }
}

impl Host for Vault<'_> {
    fn now(&self) -> Timestamp {
        Timestamp(self.now.get())
    }
    async fn get(&self, path: &str) -> Result<Got, String> {
        Ok(match self.serve(path) {
            Ok(r) => Got::Ok { body: r.body, more: r.next.is_some() },
            Err(s) => Got::Status(s),
        })
    }
    async fn copy_pack(&self, network: NetworkId, id: PackId, checksum: Digest, bytes: u64) -> Result<Copied, String> {
        self.copied.borrow_mut().push(network);
        if self.revoked_packs.get() {
            return Ok(Copied::Status(401));
        }
        // As the Worker: HEAD first, so a pack already held is never fetched again.
        let key = graftyard_vault::pack_key(network, id);
        match taken(self.head(&key).await?, bytes, Some(&checksum)) {
            Taken::Same => return Ok(Copied::Exists),
            Taken::Different(why) => return Ok(Copied::Corrupt(format!("{key} holds another object: {why}"))),
            Taken::Absent => {}
        }
        if self.inst.borrow().lost.contains(&id) {
            return Ok(Copied::Status(410));
        }
        let body = match self.serve(&format!("packs/{id}?network={}", network.0.0)) {
            Ok(r) => r.body,
            Err(s) => return Ok(Copied::Status(s)),
        };
        let mut check = PackCheck::new(checksum, bytes);
        for c in body.chunks(3) {
            if !check.push(c) {
                break;
            }
        }
        if let Err(e) = check.done() {
            return Ok(Copied::Corrupt(e));
        }
        Ok(if put_once(self, &key, &body).await? { Copied::Stored } else { Copied::Exists })
    }
    async fn copy_snapshot(&self, chain: ChainId, at: Index) -> Result<Copied, String> {
        let body = match self.serve(&format!("chains/{chain}/snapshots?at={}", at.0)) {
            Ok(r) => r.body,
            Err(s) => return Ok(Copied::Status(s)),
        };
        let mut rd = SnapshotReader::default();
        let (mut out, mut key, mut buf) = (Copied::Exists, String::new(), Vec::new());
        for c in body.chunks(5) {
            for p in rd.push(c)? {
                match p {
                    SnapPart::Start { key: k, .. } => (key, buf) = (k, Vec::new()),
                    SnapPart::Bytes(b) => buf.extend(b),
                    SnapPart::End { ok } => {
                        if !ok || !graftyard_vault::snapshot_of(&key, chain) {
                            return Ok(Copied::Corrupt(key));
                        }
                        if put_once(self, &key, &buf).await? {
                            out = Copied::Stored;
                        }
                    }
                }
            }
        }
        Ok(out)
    }
    async fn pack_status(&self, network: NetworkId, id: PackId) -> Result<u16, String> {
        self.probes.borrow_mut().push(id.to_string());
        if self.inst.borrow().lost.contains(&id) {
            return Ok(410);
        }
        Ok(self.serve(&format!("packs/{id}?network={}", network.0.0)).map_or_else(|s| s, |_| 200))
    }
    async fn put_create(&self, key: &str, body: &[u8], sha256: &Digest) -> Result<bool, String> {
        let mut b = self.bucket.borrow_mut();
        if let Some((_, at, _)) = b.get(key) {
            self.refused_puts.set(self.refused_puts.get() + 1);
            // R2 checks the lock before the precondition: a locked key answers 409, not 412.
            if self.now.get() < at + LOCK_DAYS * DAY && !key.starts_with("state/") {
                return Err("409 ObjectLockedByBucketPolicy".into());
            }
            return Ok(false);
        }
        b.insert(key.into(), (body.to_vec(), self.now.get(), Some(sha256.hex())));
        Ok(true)
    }
    async fn head(&self, key: &str) -> Result<Option<Held>, String> {
        Ok(self.bucket.borrow().get(key).map(|(b, _, s)| Held { size: b.len() as u64, sha256: s.clone() }))
    }
    async fn state(&self) -> Result<Option<(Vec<u8>, String)>, String> {
        Ok(self.bucket.borrow().get("state/vault").map(|(b, ..)| (b.clone(), self.etag.get().to_string())))
    }
    async fn put_state(&self, body: &[u8], etag: Option<&str>) -> Result<Option<String>, String> {
        if self.race.replace(false) {
            self.etag.set(self.etag.get() + 1);
        }
        let held = self.bucket.borrow().get("state/vault").map(|_| self.etag.get().to_string());
        if held.as_deref() != etag {
            return Ok(None);
        }
        self.etag.set(self.etag.get() + 1);
        self.bucket.borrow_mut().insert("state/vault".into(), (body.to_vec(), self.now.get(), None));
        Ok(Some(self.etag.get().to_string()))
    }
    async fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        Ok(self.keys(prefix))
    }
    async fn delete(&self, key: &str) -> Result<bool, String> {
        let mut b = self.bucket.borrow_mut();
        let Some((_, at, _)) = b.get(key) else { return Ok(true) };
        // The bucket lock: a delete before the object is LOCK_DAYS old is refused.
        if self.now.get() < at + LOCK_DAYS * DAY && !key.starts_with("state/") {
            self.refused_deletes.set(self.refused_deletes.get() + 1);
            return Ok(false);
        }
        b.remove(key);
        Ok(true)
    }
}

/// Every entry the vault holds, per chain and index, from its feed objects and its filled pages; an index kept twice
/// fails.
fn kept(v: &Vault) -> BTreeMap<ChainId, BTreeMap<u64, Vec<u8>>> {
    let mut at: BTreeMap<ChainId, BTreeMap<u64, Vec<u8>>> = BTreeMap::new();
    let mut put = |c: ChainId, i: u64, row: Vec<u8>| {
        assert!(at.entry(c).or_default().insert(i, row).is_none(), "{c} entry {i} kept twice");
    };
    for k in v.keys("feed/") {
        let o = export::decode_object(&v.bucket.borrow()[&k].0).unwrap();
        for (c, first, rows) in o.entries {
            for (n, r) in rows.into_iter().enumerate() {
                put(c, first.0 + n as u64, r);
            }
        }
    }
    for k in v.keys("entries/") {
        let (c, range) = k.strip_prefix("entries/").unwrap().split_once('/').unwrap();
        let from: u64 = range.split_once('-').unwrap().0.parse().unwrap();
        for (n, r) in export::unframe(&v.bucket.borrow()[&k].0).unwrap().into_iter().enumerate() {
            put(c.parse().unwrap(), from + n as u64, r);
        }
    }
    at
}

/// `kept` as each chain's rows from 1, which fails on a gap.
fn kept_from_1(v: &Vault, skip: &[ChainId]) -> BTreeMap<ChainId, Vec<Vec<u8>>> {
    kept(v)
        .into_iter()
        .filter(|(c, _)| !skip.contains(c))
        .map(|(c, m)| {
            assert_eq!(
                m.keys().copied().collect::<Vec<_>>(),
                (1..=m.len() as u64).collect::<Vec<_>>(),
                "{c} has a gap"
            );
            (c, m.into_values().collect())
        })
        .collect()
}

#[test]
fn the_vault_keeps_every_chain_s_every_entry_once_with_its_packs_and_snapshots() {
    let inst = RefCell::new(Install::default());
    let v = Vault::new(&inst);
    let r1 = repo(1);
    let net1 = NetworkId(DbId(1));
    let pack = |n: u128| PackId(Ulid((u128::from(T0) << 80) | n));
    let bytes = |n: u128| format!("PACK pack {n} of repo 1").into_bytes();
    let snap = |chain, to: u64| R2Key::Snapshot {
        chain,
        tier: Tier::Core,
        from: Index(0),
        to: Index(to),
        digest: Digest([4; 32]),
    };
    let system =
        [ChainId::Release, ChainId::Ids, ChainId::Tokens(TokenShard(9)), ChainId::Identity(AccountId(DbId(7100002)))];
    {
        let mut i = inst.borrow_mut();
        for n in 1..=7 {
            i.packs.insert(pack(n), (net1, bytes(n)));
        }
        // Pack 5's served bytes are not the entry's: a corrupt copy at the install.
        i.packs.insert(pack(5), (net1, b"PACK not these bytes".to_vec()));
        i.lost.insert(pack(6));
        i.snapshots.insert(snap(r1, 3), b"core rows at 3".to_vec());
        // repo:5 was archived below 11 before the feed carried it, its archive objects still served; repo:6 below 21
        // and dropped, with a snapshot at 15 standing in.
        i.first.insert(repo(5), (11, true));
        i.first.insert(repo(6), (21, false));
        i.snapshots.insert(snap(repo(6), 15), b"repo 6 at 15".to_vec());
        i.fill(repo(5), 25);
        i.fill(repo(6), 30);
    }
    let mut now = T0;
    let mut round = |k: usize, v: &Vault| {
        let mut i = inst.borrow_mut();
        for (n, c) in system.iter().enumerate() {
            if (k + n).is_multiple_of(2) {
                i.fill(*c, 1 + (k + n) % 3);
            }
        }
        let next = i.logs.get(&r1).map_or(1, Vec::len) as u64 + 1;
        let ev = match k {
            0 => vec![push(next, Some(stored(pack(1), &bytes(1)))), push(next + 100, Some(stored(pack(2), &bytes(2))))],
            1 => vec![push(next, None)],
            2 => vec![RepoEvent::Archived(Archived {
                key: snap(r1, 3),
                from: Index(0),
                to: Index(3),
                from_at: Timestamp(now),
                to_at: Timestamp(now),
                digest: Digest([4; 32]),
            })],
            3 => vec![push(next, Some(stored(pack(3), &bytes(3))))],
            4 => vec![push(next, Some(stored(pack(5), &bytes(5)))), push(next + 100, Some(stored(pack(6), &bytes(6))))],
            5 => vec![push(next, Some(PackRef::Inline(b"PACK inline".to_vec())))],
            6 => vec![RepoEvent::Promoted(Promoted { from: Index(1), to: Index(2), pack: stored(pack(7), &bytes(7)) })],
            _ => vec![push(next, None)],
        };
        i.add(r1, entry(next, now, ev));
        i.feed(now);
        now += 15_000;
        v.now.set(now);
    };
    for k in 0..12 {
        round(k, &v);
        // A pass every other round; the third meets another pass and stops at its first state write.
        if k % 2 == 1 {
            v.race.set(k == 5);
            let r = v.run();
            assert_eq!(r.lost, k == 5);
        }
    }
    v.drain();
    let st = v.state();
    let mut held = inst.borrow().logs.clone();
    let six = held.remove(&repo(6)).unwrap();
    assert_eq!(kept_from_1(&v, &[repo(6)]), held, "every chain's every entry, once, Release and Ids included");
    // repo:6's entries below 21 were dropped at the install before the feed carried it: its snapshot at 15 stands in.
    let kept6 = kept(&v).remove(&repo(6)).unwrap();
    assert_eq!(kept6.keys().copied().collect::<Vec<_>>(), (21..=30).collect::<Vec<_>>());
    assert_eq!(kept6.into_values().collect::<Vec<_>>(), six[20..].to_vec());
    // The feed's objects, byte for byte.
    let objects: Vec<Vec<u8>> = inst.borrow().objects.values().cloned().collect();
    let feed: Vec<Vec<u8>> = v.keys("feed/").iter().map(|k| v.bucket.borrow()[k].0.clone()).collect();
    assert_eq!(feed, objects);
    // Stored packs as the entries named them; the corrupt and the lost one counted and never stored.
    let packs: Vec<String> = v.keys("pack/");
    let want: Vec<String> = [1, 2, 3, 7].iter().map(|n| graftyard_vault::pack_key(net1, pack(*n))).collect();
    assert_eq!(packs, want);
    for n in [1, 2, 3, 7] {
        assert_eq!(v.bucket.borrow()[&graftyard_vault::pack_key(net1, pack(n))].0, bytes(n));
    }
    assert_eq!(st.missed, 2, "pack 5 (bad bytes) and pack 6 (410)");
    // Snapshots: repo:1's, and repo:6's stand-in for its dropped entries 1..=20.
    assert_eq!(v.keys("snapshot/"), vec![snap(r1, 3).to_string(), snap(repo(6), 15).to_string()]);
    assert!(st.copies.is_empty() && st.backfill.is_empty(), "nothing owed: {:?} {:?}", st.copies, st.backfill);
    assert_eq!(st.undecoded, 0);
    assert_eq!(v.keys("entries/repo:5/").len(), 1, "repo:5's 1..=10 filled by the entries endpoint, in one page");
    // Nothing the vault wrote under a locked prefix was ever overwritten or deleted.
    assert_eq!(v.refused_deletes.get(), 0);
}

/// Packs a roll consumed: probed only once the roll is `CONSUMED_KEEP` old, at most daily, and deleted from the vault
/// only `RESTORE_WINDOW` after the install's GC deleted them, which the bucket lock would refuse any earlier. A pack
/// whose bytes the install lost (410) is never deleted; nor is a pack no roll consumed.
#[test]
fn a_consumed_pack_leaves_the_vault_only_a_restore_window_after_the_install_s_gc() {
    let inst = RefCell::new(Install::default());
    let v = Vault::new(&inst);
    let net = NetworkId(DbId(1));
    let pack = |n: u128| PackId(Ulid((u128::from(T0) << 80) | n));
    let bytes = |n: u128| format!("PACK {n}").into_bytes();
    {
        let mut i = inst.borrow_mut();
        for n in 1..=4 {
            i.packs.insert(pack(n), (net, bytes(n)));
        }
        let rolled = stored(pack(4), &bytes(4));
        i.add(repo(1), entry(1, T0, vec![push(1, Some(stored(pack(1), &bytes(1))))]));
        i.add(repo(1), entry(2, T0, vec![push(2, Some(stored(pack(2), &bytes(2))))]));
        i.add(repo(1), entry(3, T0, vec![push(3, Some(stored(pack(3), &bytes(3))))]));
        let c = Compacted { consumed: vec![pack(1), pack(2)], produced: rolled, from: Index(1), to: Index(2) };
        i.add(repo(1), entry(4, T0 + DAY, vec![RepoEvent::Compacted(c)]));
        i.feed(T0 + DAY);
    }
    v.now.set(T0 + DAY + 60_000);
    v.drain();
    assert_eq!(v.keys("pack/").len(), 4);
    assert!(v.probes.borrow().is_empty(), "no probe before CONSUMED_KEEP");
    // The install's GC: pack 1 deleted on day 9 (8 days after the roll), pack 2's bytes lost (410), pack 3 kept.
    let gc_day = T0 + DAY + CONSUMED_KEEP + DAY;
    let one = graftyard_vault::pack_key(net, pack(1));
    let mut gone_at = None;
    for day in 0..(CONSUMED_KEEP / DAY + 3 + LOCK_DAYS + 2) {
        let t = T0 + DAY + 60_000 + day * DAY;
        if t >= gc_day {
            inst.borrow_mut().packs.remove(&pack(1));
            inst.borrow_mut().lost.insert(pack(2));
        }
        for at in [t, t + 3_600_000] {
            // Twice the same day probes nothing again.
            v.now.set(at);
            v.run();
            gone_at = gone_at.or(v.state().gc.get(&one).and_then(|g| g.gone).map(|g| g.0));
            let held = v.keys("pack/").contains(&one);
            match gone_at {
                Some(g) => {
                    assert!(g >= gc_day, "gone before the install deleted it");
                    assert_eq!(held, at < g + LOCK_DAYS * DAY, "day {day}");
                }
                None => assert!(held, "day {day}"),
            }
        }
    }
    assert!(gone_at.is_some_and(|g| g < gc_day + DAY), "probed within a day of the install's GC");
    let held = v.keys("pack/");
    assert!(!held.contains(&graftyard_vault::pack_key(net, pack(1))), "pack 1 leaves a restore window after its GC");
    for n in [2, 3, 4] {
        assert!(held.contains(&graftyard_vault::pack_key(net, pack(n))), "pack {n} stays");
    }
    let probes = v.probes.borrow();
    for n in [3u128, 4] {
        assert!(!probes.contains(&pack(n).to_string()), "pack {n} was never consumed, so never probed");
    }
    let days = CONSUMED_KEEP / DAY + 3 + LOCK_DAYS + 2;
    assert!(probes.len() as u64 <= 2 * days, "at most daily per candidate: {}", probes.len());
    assert_eq!(v.refused_deletes.get(), 0, "no delete the bucket lock had to refuse");
}

/// A revoked token stops the pass with the 401 named and moves nothing; a pass after it resumes where it was.
#[test]
fn a_refused_token_stops_the_pass_and_moves_nothing() {
    let inst = RefCell::new(Install::default());
    let v = Vault::new(&inst);
    inst.borrow_mut().fill(ChainId::Release, 5);
    inst.borrow_mut().feed(T0);
    v.now.set(T0 + 30_000);
    v.drain();
    let before = v.state();
    inst.borrow_mut().fill(ChainId::Release, 2);
    inst.borrow_mut().feed(T0 + 15_000);
    v.revoked.set(true);
    let e = block_on(pass(&v)).unwrap_err();
    assert!(e.contains("401"), "{e}");
    assert_eq!(v.state(), before);
    v.revoked.set(false);
    v.drain();
    assert_eq!(kept_from_1(&v, &[])[&ChainId::Release].len(), 7);
}

/// One network's backlog does not hold back another's copies (`vault-turns`): owed copies go a network at a time, and
/// each pass starts after the network where the last one stopped. Network 1 owes 100 packs and networks 2 to 60 one
/// each: all of 2 to 60 are copied in 2 passes, where taking copies in key order would need 4.
#[test]
fn one_network_s_backlog_waits_its_turn_and_the_next_pass_starts_after_the_last() {
    let inst = RefCell::new(Install::default());
    let v = Vault::new(&inst);
    let pack = |net: i64, n: u128| PackId(Ulid((u128::from(T0) << 80) | (net as u128) << 16 | n));
    let bytes = |net: i64, n: u128| format!("PACK {n} of network {net}").into_bytes();
    {
        let mut i = inst.borrow_mut();
        for net in 1..=60 {
            let owed = if net == 1 { 100 } else { 1 };
            let events = (1..=owed)
                .map(|n| {
                    i.packs.insert(pack(net, n), (NetworkId(DbId(net)), bytes(net, n)));
                    push(n as u64, Some(stored(pack(net, n), &bytes(net, n))))
                })
                .collect();
            i.add(repo(net), entry(1, T0, events));
        }
        i.feed(T0);
    }
    v.now.set(T0 + 30_000);
    let held = |net: i64| v.keys(&format!("pack/{net}/")).len();
    let nets = |from: usize| v.copied.borrow()[from..].iter().map(|n| n.0.0).collect::<Vec<i64>>();

    let one = v.run();
    assert!(one.more);
    assert_eq!(nets(0), (1..=40).collect::<Vec<_>>(), "one copy per network, network 1 once");
    assert_eq!(v.state().copy_turn, Some(graftyard_vault::Turn::Network(NetworkId(DbId(40)))));

    let two = v.run();
    assert!(two.more);
    let mut want: Vec<i64> = (41..=60).collect();
    want.extend([1; 20]);
    assert_eq!(nets(40), want, "pass 2 starts after network 40, then network 1 takes the rest of the pass");
    for net in 2..=60 {
        assert_eq!(held(net), 1, "network {net} copied within 2 passes");
    }
    assert_eq!(held(1), 21);

    v.drain();
    assert_eq!(held(1), 100);
    assert!(v.state().copies.is_empty());
}

/// A pack the install deleted and then served again (a restore put it back) stays: the vault probes it again before
/// it deletes, and its clock starts over at the next 404.
#[test]
fn a_pack_served_again_after_its_gc_keeps_its_vault_copy_until_a_window_after_the_next() {
    let inst = RefCell::new(Install::default());
    let v = Vault::new(&inst);
    let net = NetworkId(DbId(1));
    let pack = |n: u128| PackId(Ulid((u128::from(T0) << 80) | n));
    let bytes = |n: u128| format!("PACK {n}").into_bytes();
    {
        let mut i = inst.borrow_mut();
        for n in 1..=2 {
            i.packs.insert(pack(n), (net, bytes(n)));
        }
        i.add(repo(1), entry(1, T0, vec![push(1, Some(stored(pack(1), &bytes(1))))]));
        let c =
            Compacted { consumed: vec![pack(1)], produced: stored(pack(2), &bytes(2)), from: Index(1), to: Index(1) };
        i.add(repo(1), entry(2, T0, vec![RepoEvent::Compacted(c)]));
        i.feed(T0);
    }
    let one = graftyard_vault::pack_key(net, pack(1));
    let day = |d: u64| T0 + CONSUMED_KEEP + d * DAY + 60_000;
    let at = |d: u64| {
        v.now.set(day(d));
        v.drain();
        v.keys("pack/").contains(&one)
    };
    assert!(at(0));
    // Gone on day 1, served again from day 30 to day 40 (inside the first window), gone again from day 41.
    inst.borrow_mut().packs.remove(&pack(1));
    for d in 1..30 {
        assert!(at(d), "day {d}");
    }
    inst.borrow_mut().packs.insert(pack(1), (net, bytes(1)));
    for d in 30..=40 {
        assert!(at(d), "day {d}: served again");
    }
    inst.borrow_mut().packs.remove(&pack(1));
    for d in 41..41 + LOCK_DAYS {
        assert!(at(d), "day {d}: inside the window after the second 404");
    }
    assert!(!at(41 + LOCK_DAYS + 1), "deleted a window after the second 404");
    assert_eq!(v.refused_deletes.get(), 0);
}

/// A feed object that skips an index against the vault's cursor is refused: the pass fails and stores nothing from it.
#[test]
fn a_feed_object_that_skips_an_index_is_refused_and_not_stored() {
    let inst = RefCell::new(Install::default());
    let v = Vault::new(&inst);
    inst.borrow_mut().fill(ChainId::Ids, 3);
    inst.borrow_mut().feed(T0);
    v.now.set(T0 + 30_000);
    v.drain();
    let before = (v.keys(""), v.state());
    // The next object carries ids from 5: entry 4 is missing.
    let bad = ExportObject {
        batch: ChangeBatch { minute: T0 / 60_000 + 1, seq: 0, chains: vec![(ChainId::Ids, Index(5))] },
        entries: vec![(ChainId::Ids, Index(5), vec![b"ids#5".to_vec()])],
    };
    inst.borrow_mut().objects.insert(bad.batch.position(), export::encode_object(&bad));
    v.now.set(T0 + 90_000);
    let e = block_on(pass(&v)).unwrap_err();
    assert!(e.contains("feed"), "{e}");
    assert_eq!((v.keys(""), v.state()), before);
}

/// A vault that fell past the feed's 7-day lifecycle lists again from the oldest object and fills the gap from the
/// entries endpoint: every entry once, no gap.
#[test]
fn a_vault_past_the_feed_s_retention_resyncs_and_fills_the_gap() {
    let inst = RefCell::new(Install::default());
    let v = Vault::new(&inst);
    let c = ChainId::Release;
    inst.borrow_mut().fill(c, 4);
    inst.borrow_mut().feed(T0);
    v.now.set(T0 + 30_000);
    v.drain();
    // Rounds the vault never pulled, then 8 days on; the lifecycle drops all but the newest object.
    for k in 1..=3 {
        inst.borrow_mut().fill(c, 2);
        inst.borrow_mut().feed(T0 + k * 60_000);
    }
    let late = T0 + 8 * DAY;
    inst.borrow_mut().fill(c, 3);
    inst.borrow_mut().feed(late);
    inst.borrow_mut().objects.retain(|p, _| p.minute * 60_000 >= late - 60_000);
    v.now.set(late + 30_000);
    v.drain();
    assert_eq!(kept_from_1(&v, &[])[&c], inst.borrow().logs[&c], "every entry once, the gap filled");
    assert!(!v.keys("entries/release/").is_empty());
}

/// A token revoked between the pass's feed page and its copies: the 401 stops the pass, the pack stays owed and is not
/// counted missed, and a pass after the token is back stores it.
#[test]
fn a_401_on_a_copy_stops_the_pass_and_keeps_the_copy_owed() {
    let inst = RefCell::new(Install::default());
    let v = Vault::new(&inst);
    let net = NetworkId(DbId(1));
    let id = PackId(Ulid((u128::from(T0) << 80) | 1));
    let body = b"PACK one".to_vec();
    inst.borrow_mut().packs.insert(id, (net, body.clone()));
    inst.borrow_mut().add(repo(1), entry(1, T0, vec![push(1, Some(stored(id, &body)))]));
    inst.borrow_mut().feed(T0);
    v.now.set(T0 + 30_000);
    v.revoked_packs.set(true);
    let e = block_on(pass(&v)).unwrap_err();
    assert!(e.contains("401"), "{e}");
    assert!(v.keys("pack/").is_empty());
    v.revoked_packs.set(false);
    v.drain();
    assert_eq!(v.keys("pack/"), vec![graftyard_vault::pack_key(net, id)]);
    assert_eq!(v.state().missed, 0);
}

/// A pass rerun from an older `state/vault` repeats writes an earlier pass made under the lock. R2 checks the lock
/// before `If-None-Match`, so each repeated create-only put fails with 409 rather than 412; the pass must take the
/// same object as already stored, finish, and store nothing new.
#[test]
fn a_pass_rerun_from_an_older_state_moves_on_over_locked_objects_and_stores_nothing_new() {
    let inst = RefCell::new(Install::default());
    let v = Vault::new(&inst);
    let net = NetworkId(DbId(1));
    let id = PackId(Ulid((u128::from(T0) << 80) | 1));
    let body = b"PACK one".to_vec();
    let snap =
        R2Key::Snapshot { chain: repo(1), tier: Tier::Core, from: Index(0), to: Index(2), digest: Digest([4; 32]) };
    {
        let mut i = inst.borrow_mut();
        i.packs.insert(id, (net, body.clone()));
        i.snapshots.insert(snap.clone(), b"core rows at 2".to_vec());
        // repo:5 first carried past index 1, so the pass fills entries/ too.
        i.first.insert(repo(5), (4, true));
        i.fill(repo(5), 6);
        i.fill(ChainId::Release, 3);
        i.add(repo(1), entry(1, T0, vec![push(1, Some(stored(id, &body)))]));
        i.add(
            repo(1),
            entry(
                2,
                T0,
                vec![RepoEvent::Archived(Archived {
                    key: snap.clone(),
                    from: Index(0),
                    to: Index(2),
                    from_at: Timestamp(T0),
                    to_at: Timestamp(T0),
                    digest: Digest([4; 32]),
                })],
            ),
        );
        i.feed(T0);
    }
    v.now.set(T0 + 30_000);
    let older = v.bucket.borrow().get("state/vault").cloned();
    assert!(older.is_none(), "the first pass starts from no state");
    v.drain();
    let held: Vec<(String, Vec<u8>)> = v
        .keys("")
        .into_iter()
        .filter(|k| !k.starts_with("state/"))
        .map(|k| {
            let b = v.bucket.borrow()[&k].0.clone();
            (k, b)
        })
        .collect();
    assert!(held.iter().any(|(k, _)| k.starts_with("feed/")));
    assert!(held.iter().any(|(k, _)| k.starts_with("entries/")));
    assert!(held.iter().any(|(k, _)| k.starts_with("pack/")));
    assert!(held.iter().any(|(k, _)| k.starts_with("snapshot/")));
    let after = v.state();
    // Back to no state: the next pass writes every object again, inside the lock.
    v.bucket.borrow_mut().remove("state/vault");
    v.now.set(T0 + 60_000);
    let refused = v.refused_puts.get();
    let (r, _) = block_on(pass(&v)).expect("a repeated pass over locked objects must not fail");
    assert!(v.refused_puts.get() > refused, "the rerun hit the lock's 409");
    let again: Vec<(String, Vec<u8>)> = v
        .keys("")
        .into_iter()
        .filter(|k| !k.starts_with("state/"))
        .map(|k| {
            let b = v.bucket.borrow()[&k].0.clone();
            (k, b)
        })
        .collect();
    assert_eq!(again, held, "nothing new stored, nothing changed");
    assert_eq!((r.packs, r.snapshots), (0, 0));
    assert_eq!(v.state().feed, after.feed);
    assert_eq!(v.state().missed, 0);
}

/// Another object at a key the vault means to write is never called stored: a feed object fails the pass, and a pack
/// whose key holds other bytes counts missed and is left alone.
#[test]
fn another_object_at_the_key_is_never_taken_for_the_one_meant() {
    let inst = RefCell::new(Install::default());
    let v = Vault::new(&inst);
    inst.borrow_mut().fill(ChainId::Ids, 2);
    inst.borrow_mut().feed(T0);
    let pos = *inst.borrow().objects.keys().next().unwrap();
    let key = graftyard_vault::feed_key(pos);
    v.bucket.borrow_mut().insert(key.clone(), (b"not the object".to_vec(), T0, Some("00".repeat(32))));
    v.now.set(T0 + 30_000);
    let e = block_on(pass(&v)).unwrap_err();
    assert!(e.contains("holds another object"), "{e}");
    assert_eq!(v.bucket.borrow()[&key].0, b"not the object");
    // The same length and no digest metadata is not the same object either.
    let held = Held { size: 8, sha256: None };
    assert!(matches!(taken(Some(held), 8, Some(&Digest([1; 32]))), Taken::Different(_)));
    assert_eq!(taken(Some(Held { size: 8, sha256: None }), 8, None), Taken::Same);
    // A snapshot object has no digest to check, so its size alone tells another object apart.
    assert!(matches!(taken(Some(Held { size: 9, sha256: None }), 8, None), Taken::Different(_)));

    let inst = RefCell::new(Install::default());
    let v = Vault::new(&inst);
    let net = NetworkId(DbId(1));
    let id = PackId(Ulid((u128::from(T0) << 80) | 1));
    let body = b"PACK one".to_vec();
    inst.borrow_mut().packs.insert(id, (net, body.clone()));
    inst.borrow_mut().add(repo(1), entry(1, T0, vec![push(1, Some(stored(id, &body)))]));
    inst.borrow_mut().feed(T0);
    let pk = graftyard_vault::pack_key(net, id);
    v.bucket.borrow_mut().insert(pk.clone(), (b"PACK two".to_vec(), T0, Some(Digest([2; 32]).hex())));
    v.now.set(T0 + 30_000);
    v.drain();
    assert_eq!(v.state().missed, 1);
    assert_eq!(v.bucket.borrow()[&pk].0, b"PACK two");
}
