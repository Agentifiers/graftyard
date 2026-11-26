//! One pass of the lifeboat's puller (`design/DESIGN.md`, "Install operations", Lifeboat; Shape, "Lifeboat feed";
//! `decisions/DECISIONS.md`, "`lifeboat-bundle`"), over a `Host` so it runs natively in tests as the bundle runs it
//! from its puller's alarm. `Puller` decides the sources; this moves the bytes.
//!
//! - **Feed.** Each `export/` object after the puller's position, from the install's export bucket over R2's S3 API
//!   (`crate::feed`), at most `OBJECTS_PER_PASS`. Its rows go to each chain's copy (`Host::import`), and the cursor
//!   moves only by what the copy holds after the import.
//! - **Edge.** Each `Puller::pulls` tail, signed with the host's key, a page at a time; a page that does not start at
//!   the copy's next index is refused, never stitched. The pulls go round the chains on a turn (`Puller::turn`),
//!   `AT_ONCE` first pages at once, each chain pulled to its end before the turn passes it. Pages that carry rows count
//!   against `PAGES_PER_PASS`, and answers with none (caught up, or any status) against `PROBES_PER_PASS`, so a stale
//!   feed's round of thousands of caught-up chains is not held to the data budget (`decisions/DECISIONS.md`,
//!   "`lifeboat-turns`").
//! - **Packs.** Every stored pack a repository row names, from `packs/{id}?network=` and checked against the row's
//!   `PackRef.checksum` before `graft-blob` keeps it (`Host::put_pack`), one network at a time on a turn. A pack the
//!   install answers 404 sends one probe to its repository chain: a chain the edge answers 404 is gone, and its
//!   network's owed packs are dropped. On a live chain the pack stays owed, and one unserved for
//!   `UNSERVED_ALARM_MS` alarms (`unserved_alarm`).
//! - **Rolls.** A pack a committed roll consumed is owed no more once the roll's produced pack is held
//!   (`rolled_off`), so a copy enrolled after the install deleted it neither alarms nor waits on it.
//! - **Halt.** A chain whose copy halted is moved by no source again (`Puller::halt`), and the pass names it.
//! - **Gone.** A chain the edge answers 404 has no group, retired or purged for good (`Puller::gone`): it is asked no
//!   more, and the pass names it. Any other status is asked again next pass.
//! - **Unreachable feed.** A feed that does not answer is a stale one: the pass goes on to the edge, and once no
//!   object arrived for `FEED_STALE_MS` the edge serves every chain.
//! - **Current.** With the feed fresh, a pass that leaves no chain with more to pull, from either source, is the copy's
//!   current moment, which `LifeboatStatus::cursor_age` counts from. With it stale, a cycle of the turns in which every
//!   chain answered caught up is current as of the cycle's start.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use graftyard_git::export::{self, ExportObject};
use graftyard_git::pack::PackId;
use graftyard_git::repo::{REPO_FORMAT_V1, RepoEvent, stored_packs};
use graftyard_raft::{EntryBody, LogEntry, codec};
use graftyard_types::{ChainId, DbId, Digest, Index, NetworkId, RepoId};
use serde::{Deserialize, Serialize};

use crate::puller::{Puller, Rows};

/// Feed objects one pass takes at most [guess, as the vault's `PAGES_PER_PASS` pages of one object each].
pub const OBJECTS_PER_PASS: usize = 100;
/// Edge pages that carry rows, and pack copies, one pass makes at most [guess]; a pass may run `AT_ONCE` past them.
pub const PAGES_PER_PASS: usize = 40;
pub const COPIES_PER_PASS: usize = 40;
/// Edge answers with no rows (a caught-up chain, or any status) one pass makes at most [guess]: a round of graft-m3f's
/// 3,236 chains took 131 to 150 s at the bundle's 10 s alarm and `AT_ONCE` 64, against the 5-minute lag alarm
/// (`decisions/DECISIONS.md`, "`lifeboat-turns`").
pub const PROBES_PER_PASS: usize = 400;
/// Edge requests in flight at once [guess]: 64, from 16 after BF-53's stale passes took about 30 s, their probes queued
/// 16 at a time (`decisions/DECISIONS.md`, "`lifeboat-turns`").
pub const AT_ONCE: usize = 64;
/// A pack the install answers 404 on a live repository chain alarms once it has been unserved this long [guess].
pub const UNSERVED_ALARM_MS: u64 = 3_600_000;

/// A pack the copy owes: its network, id, and the checksum and length its entry names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Owed {
    pub network: NetworkId,
    pub id: PackId,
    pub checksum: Digest,
    pub bytes: u64,
}

/// The puller's state: its sources, the packs it owes, and when the copy was last current.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub puller: Puller,
    pub owed: BTreeSet<Owed>,
    pub current_at: u64,
    /// The last `LifeboatDecoder` the group committed: its release and when (`decoder_due`).
    #[serde(default)]
    pub decoder_sent: Option<[u8; 32]>,
    #[serde(default)]
    pub decoder_at: u64,
    /// Packs kept, and edge pages and feed objects taken, over the copy's life.
    pub packs: u64,
    pub pages: u64,
    pub objects: u64,
    /// The network whose owed pack was tried last: the next pass starts after it.
    #[serde(default)]
    pub pack_turn: Option<NetworkId>,
    /// Owed packs the install answered 404 while their repository chain is live, and when it first did.
    #[serde(default)]
    pub unserved: BTreeMap<PackId, u64>,
    /// The last `graft-blob` scrub that answered, the last one tried, and the keys it could not repair
    /// (`scrub_due`, `scrubbed`).
    #[serde(default)]
    pub scrub_at: u64,
    #[serde(default)]
    pub scrub_tried: u64,
    #[serde(default)]
    pub scrub_lost: Vec<Lost>,
    /// Each committed roll the copy took, by its produced pack: the packs it consumed (`rolled_off`).
    #[serde(default)]
    pub rolls: BTreeMap<PackId, Vec<PackId>>,
    /// The git copy (`crate::git`).
    #[serde(default)]
    pub git: crate::git::GitCopy,
}

/// What an import answered: the last index the copy holds, and the halt if the copy stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    pub held: Index,
    pub halt: Option<String>,
}

/// An edge page: its rows' first index and the rows, or the status the edge answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    Rows { first: Index, rows: Vec<Vec<u8>> },
    Status(u16),
}

/// What a pack copy did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Copied {
    /// Kept, or already kept with the same bytes.
    Kept,
    /// The edge answered this status (404: not served yet).
    Status(u16),
    /// The bytes do not hash to the entry's checksum: never kept.
    Corrupt,
}

#[allow(async_fn_in_trait)]
pub trait Host {
    fn now(&self) -> u64;
    /// The feed's object keys after `after` (a key), in order, and whether more follow.
    async fn list(&self, after: Option<&str>) -> Result<(Vec<String>, bool), String>;
    async fn object(&self, key: &str) -> Result<Vec<u8>, String>;
    /// A signed `export/chains/{chain}/entries?from=&limit=`.
    async fn entries(&self, chain: ChainId, from: Index, limit: u64) -> Result<Page, String>;
    /// Steps `rows`, from the copy's next index, into `chain`'s copy.
    async fn import(&self, chain: ChainId, rows: &[Vec<u8>]) -> Result<Imported, String>;
    /// A signed `export/packs/{id}?network=`, kept only if it hashes to `checksum`.
    async fn put_pack(&self, p: &Owed) -> Result<Copied, String>;
}

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub objects: usize,
    pub pages: usize,
    /// Chains a page came from, from the edge.
    pub edge_chains: Vec<ChainId>,
    pub packs: usize,
    pub owed: usize,
    /// Owed packs a held roll consumed, owed no more (`rolled_off`).
    pub rolled_off: usize,
    pub halted: Vec<ChainId>,
    /// Chains the edge answered 404 this pass: no group, asked no more.
    pub gone: Vec<ChainId>,
    /// Edge answers with no rows: caught-up chains, and any status.
    pub probes: usize,
    /// Owed packs unserved on a live chain (`State::unserved`), and how many of them alarm.
    pub unserved: usize,
    pub unserved_alarm: usize,
    pub stale: bool,
    /// The feed did not answer (R2 down, or the token refused): the pass went on to the edge.
    pub feed_error: Option<String>,
    /// Whether this pass left more to pull.
    pub more: bool,
}

/// The repository network a chain's packs are in.
fn network_of(chain: ChainId) -> Option<NetworkId> {
    match chain {
        ChainId::Repo(r) | ChainId::RepoMeta(r) => Some(NetworkId::of_root(r)),
        _ => None,
    }
}

/// The packs `rows` of `chain` name. A repository row that does not decode is the copy's to halt on, not the
/// puller's: it is skipped here.
pub fn owed_by(chain: ChainId, rows: &[Vec<u8>]) -> Vec<Owed> {
    let Some(network) = network_of(chain) else { return Vec::new() };
    let mut out = Vec::new();
    for row in rows {
        let Ok(e) = codec::from_bytes::<LogEntry<RepoEvent>>(row) else { continue };
        if e.format.family != REPO_FORMAT_V1.family {
            continue;
        }
        let EntryBody::Events(evs) = &e.body else { continue };
        out.extend(stored_packs(evs).into_iter().map(|(id, checksum, bytes)| Owed { network, id, checksum, bytes }));
    }
    out
}

/// The rolls `rows` of `chain` commit: each produced pack with the packs it consumed.
pub fn rolls_in(chain: ChainId, rows: &[Vec<u8>]) -> Vec<(PackId, Vec<PackId>)> {
    if network_of(chain).is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for row in rows {
        let Ok(e) = codec::from_bytes::<LogEntry<RepoEvent>>(row) else { continue };
        let EntryBody::Events(evs) = &e.body else { continue };
        for ev in evs {
            if let RepoEvent::Compacted(c) = ev
                && let Some(id) = c.produced.stored_id()
            {
                out.push((id, c.consumed.clone()));
            }
        }
    }
    out
}

/// The guard for rolls (`decisions/DECISIONS.md`, "`lifeboat-clone`"): once a roll's produced pack is held (owed no
/// more), the packs it consumed are owed no more either. The install deletes a consumed pack `CONSUMED_KEEP` after
/// its roll, so a copy enrolled later would owe it for good, alarm on a live chain, and hold its git copy back; the
/// git copy loads the produced pack in their place (`crate::git::roll_markers`). Answers the packs dropped.
pub fn rolled_off(st: &mut State) -> usize {
    let held: Vec<PackId> = st.rolls.keys().filter(|p| !st.owed.iter().any(|o| o.id == **p)).copied().collect();
    let mut dropped = 0;
    for p in held {
        let consumed: BTreeSet<PackId> = st.rolls.remove(&p).unwrap_or_default().into_iter().collect();
        let before = st.owed.len();
        st.owed.retain(|o| !consumed.contains(&o.id));
        dropped += before - st.owed.len();
        for id in &consumed {
            st.unserved.remove(id);
        }
    }
    dropped
}

/// Imports one chain's rows and moves the cursor by what the copy then holds.
async fn take<H: Host>(h: &H, st: &mut State, r: &mut Report, rows: Rows) -> Result<(), String> {
    let got = h.import(rows.chain, &rows.rows).await?;
    let applied = (got.held.0 + 1).saturating_sub(rows.first.0) as usize;
    let rows_in = &rows.rows[..applied.min(rows.rows.len())];
    st.owed.extend(owed_by(rows.chain, rows_in));
    st.rolls.extend(rolls_in(rows.chain, rows_in));
    st.puller.applied(rows.chain, got.held);
    let top = Index(rows.first.0 + rows.rows.len() as u64 - 1);
    if got.halt.is_some() {
        st.puller.halt(rows.chain);
        r.halted.push(rows.chain);
    } else if got.held < top {
        // The copy took fewer rows than it was sent (a repository whose owner its `Names` copy does not name yet):
        // the rest come from the edge, never from a later feed object stitched over the gap.
        let b = st.puller.behind.entry(rows.chain).or_insert(top);
        *b = (*b).max(top);
    }
    Ok(())
}

pub async fn pass<H: Host>(h: &H, st: &mut State) -> Result<Report, String> {
    let mut r = Report::default();
    // The feed, from the puller's position.
    let mut after = st.puller.feed.map(|p| export::FeedPosition::key(&p).to_string());
    let mut feed_more = false;
    'feed: while r.objects < OBJECTS_PER_PASS {
        let (keys, more) = match h.list(after.as_deref()).await {
            Ok(l) => l,
            Err(e) => {
                r.feed_error = Some(e);
                break;
            }
        };
        if keys.is_empty() {
            break;
        }
        for k in keys {
            if r.objects == OBJECTS_PER_PASS {
                feed_more = true;
                break 'feed;
            }
            let bytes = match h.object(&k).await {
                Ok(b) => b,
                Err(e) => {
                    r.feed_error = Some(e);
                    break 'feed;
                }
            };
            let o: ExportObject = export::decode_object(&bytes)?;
            // Every other chain before a repository's: its copy is joined with the owner its `Names` shard's
            // `RepoOwner` row names, which an object carries with or before the repository's first rows.
            let mut taken = st.puller.take_object(&o, h.now())?;
            taken.sort_by_key(|r| matches!(r.chain, ChainId::Repo(_)));
            for rows in taken {
                take(h, st, &mut r, rows).await?;
            }
            after = Some(k);
            r.objects += 1;
        }
        if !more {
            break;
        }
    }
    // The edge: every known chain while the feed is stale, else the chains the feed left behind, round the turns.
    let now = h.now();
    r.stale = st.puller.stale(now);
    if !r.stale {
        st.puller.cycle = None;
    } else if st.puller.cycle.is_none() {
        // A cycle starts at the first key, so it ends at the last one.
        (st.puller.cycle, st.puller.cycle_dirty, st.puller.turn) = (Some(now), false, None);
    }
    let pulls = st.puller.pulls(now);
    let last = pulls.iter().map(|p| p.chain).max();
    let mut edge_more = false;
    'edge: for chunk in pulls.chunks(AT_ONCE) {
        if r.pages >= PAGES_PER_PASS || r.probes >= PROBES_PER_PASS {
            edge_more = true;
            break;
        }
        let firsts = futures_util::future::join_all(chunk.iter().map(|p| h.entries(p.chain, p.from, p.limit))).await;
        for (p, got) in chunk.iter().zip(firsts) {
            let mut got = got;
            // Each chain to its end before the turn passes it: a chain cut off by the budget is the next pass's first.
            let clean = loop {
                match got {
                    Ok(Page::Rows { first, rows }) if !rows.is_empty() => {
                        r.pages += 1;
                        if !r.edge_chains.contains(&p.chain) {
                            r.edge_chains.push(p.chain);
                        }
                        let n = rows.len() as u64;
                        let rows = st.puller.take_page(p.chain, first, rows)?;
                        take(h, st, &mut r, rows).await?;
                        if st.puller.halted.contains(&p.chain) || st.puller.behind.contains_key(&p.chain) {
                            break st.puller.halted.contains(&p.chain);
                        }
                        if n < p.limit {
                            break true;
                        }
                        if r.pages >= PAGES_PER_PASS {
                            edge_more = true;
                            break 'edge;
                        }
                        got = h.entries(p.chain, st.puller.next(p.chain), p.limit).await;
                    }
                    Ok(Page::Rows { .. }) => {
                        r.probes += 1;
                        break true;
                    }
                    // No group: asked no more.
                    Ok(Page::Status(404)) => {
                        r.probes += 1;
                        st.puller.gone(p.chain);
                        r.gone.push(p.chain);
                        break true;
                    }
                    // A chain the edge cannot serve now (a 503 while its followers move) is asked again next turn.
                    _ => {
                        r.probes += 1;
                        break false;
                    }
                }
            };
            st.puller.turn = Some(p.chain);
            if !clean {
                st.puller.cycle_dirty = true;
            }
            if r.stale && Some(p.chain) == last {
                let start = st.puller.cycle.unwrap_or(now);
                if !st.puller.cycle_dirty && st.puller.behind.is_empty() && st.puller.halted.is_empty() {
                    st.current_at = st.current_at.max(start);
                }
                (st.puller.cycle, st.puller.cycle_dirty) = (Some(h.now()), false);
            }
        }
    }
    // Packs, one network at a time round the turns.
    let mut probed: BTreeMap<NetworkId, bool> = BTreeMap::new();
    for p in in_turns(&st.owed, st.pack_turn, COPIES_PER_PASS) {
        if !st.owed.contains(&p) {
            continue;
        }
        st.pack_turn = Some(p.network);
        match h.put_pack(&p).await {
            Ok(Copied::Kept) => {
                st.owed.remove(&p);
                st.unserved.remove(&p.id);
                r.packs += 1;
            }
            Ok(Copied::Status(404)) => {
                let chain = ChainId::Repo(RepoId(p.network.0));
                let gone = match probed.get(&p.network) {
                    Some(g) => *g,
                    None if st.puller.gone.contains(&chain) => true,
                    None => {
                        let g = matches!(h.entries(chain, st.puller.next(chain), 1).await, Ok(Page::Status(404)));
                        probed.insert(p.network, g);
                        g
                    }
                };
                if gone {
                    st.puller.gone(chain);
                    if !r.gone.contains(&chain) {
                        r.gone.push(chain);
                    }
                    let dropped: Vec<PackId> =
                        st.owed.iter().filter(|o| o.network == p.network).map(|o| o.id).collect();
                    st.owed.retain(|o| o.network != p.network);
                    for id in dropped {
                        st.unserved.remove(&id);
                    }
                } else {
                    st.unserved.entry(p.id).or_insert(now);
                }
            }
            // A corrupt copy, another status, or no answer: still owed.
            Ok(Copied::Status(_) | Copied::Corrupt) | Err(_) => {}
        }
    }
    r.rolled_off = rolled_off(st);
    r.owed = st.owed.len();
    r.unserved = st.unserved.len();
    r.unserved_alarm = unserved_alarm(st, h.now());
    r.more = feed_more || edge_more || st.owed.len() > COPIES_PER_PASS;
    // Current with the feed fresh: a source answered for every chain, nothing is left to pull, and no chain is behind
    // or halted. With it stale, only a clean cycle of the turns, above, or a copy that knows no chain to pull.
    let settled = !r.more && st.puller.behind.is_empty() && st.puller.halted.is_empty();
    if settled && (if r.stale { last.is_none() } else { r.feed_error.is_none() }) {
        st.current_at = h.now();
    }
    st.objects += r.objects as u64;
    st.pages += r.pages as u64;
    st.packs += r.packs as u64;
    Ok(r)
}

/// Owed packs unserved on a live chain for `UNSERVED_ALARM_MS` or more at `now`.
pub fn unserved_alarm(st: &State, now: u64) -> usize {
    st.unserved.values().filter(|t| now.saturating_sub(**t) >= UNSERVED_ALARM_MS).count()
}

/// The owed packs one pass tries: one per network, round and round, starting with the first network after `after`
/// (where the last pass stopped), at most `n`, as the vault takes its copies (`in_turns` there). So packs the install
/// no longer serves delay another network's by at most a turn each, not by their number.
fn in_turns(owed: &BTreeSet<Owed>, after: Option<NetworkId>, n: usize) -> Vec<Owed> {
    let mut turns: BTreeMap<NetworkId, VecDeque<Owed>> = BTreeMap::new();
    for o in owed {
        turns.entry(o.network).or_default().push_back(*o);
    }
    let (later, earlier): (Vec<_>, Vec<_>) = turns.into_iter().partition(|(t, _)| after.is_some_and(|a| *t > a));
    let mut queue: VecDeque<VecDeque<Owed>> = later.into_iter().chain(earlier).map(|(_, q)| q).collect();
    let mut out = Vec::new();
    while out.len() < n {
        let Some(mut q) = queue.pop_front() else { break };
        out.extend(q.pop_front());
        if !q.is_empty() {
            queue.push_back(q);
        }
    }
    out
}

/// How often a lifeboat reports its decoder at least (`design/DESIGN.md`, "Install operations", Lifeboat: "when its
/// decoder changes and at least daily"): a little under a day, so a daily alarm never slips past one.
pub const DECODER_EVERY_MS: u64 = 23 * 3_600_000;

/// Whether the puller posts a `LifeboatDecoder` for `release` at `now`: none committed yet, another release than the
/// last one committed, or the last one `DECODER_EVERY_MS` old.
pub fn decoder_due(st: &State, release: [u8; 32], now: u64) -> bool {
    st.decoder_sent != Some(release) || now.saturating_sub(st.decoder_at) >= DECODER_EVERY_MS
}

/// How often the puller asks `graft-blob` to scrub (`design/DESIGN.md`, "Install operations", Lifeboat: "a weekly
/// scrub"), and how soon it tries again after a scrub that did not answer [guess].
pub const SCRUB_EVERY_MS: u64 = 7 * 24 * 3_600_000;
pub const SCRUB_RETRY_MS: u64 = 3_600_000;

/// Whether the puller scrubs at `now`: a week since the last scrub that answered, and an hour since the last try.
pub fn scrub_due(st: &State, now: u64) -> bool {
    now.saturating_sub(st.scrub_at) >= SCRUB_EVERY_MS && now.saturating_sub(st.scrub_tried) >= SCRUB_RETRY_MS
}

/// A key the scrub could not repair: its outcome (`lost`, or `error` if graft-blob could not check it), and the copy
/// whose entry named it, so an operator can pull it again: a pack `pack/{network}/{id}` came from `repo:{network}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lost {
    pub key: String,
    pub outcome: String,
    pub copy: Option<ChainId>,
}

/// What one scrub found, from `graft-blob`'s `POST /_scrub` answer (a `{key} {outcome}` line a key).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ScrubReport {
    pub ok: usize,
    pub repaired: usize,
    pub restored: usize,
    pub lost: Vec<Lost>,
}

pub fn scrubbed(answer: &str) -> ScrubReport {
    let mut r = ScrubReport::default();
    for line in answer.lines() {
        let Some((key, outcome)) = line.trim().rsplit_once(' ') else { continue };
        match outcome {
            "ok" => r.ok += 1,
            "repaired" => r.repaired += 1,
            "restored" => r.restored += 1,
            _ => {
                let copy = key
                    .strip_prefix("pack/")
                    .and_then(|k| k.split_once('/'))
                    .and_then(|(n, _)| n.parse().ok())
                    .map(|n| ChainId::Repo(RepoId(DbId(n))));
                r.lost.push(Lost { key: key.to_string(), outcome: outcome.to_string(), copy });
            }
        }
    }
    r
}

impl State {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("json")
    }

    pub fn decode(b: &[u8]) -> Result<State, String> {
        serde_json::from_slice(b).map_err(|e| e.to_string())
    }
}
