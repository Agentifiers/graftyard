//! One pass of the lifeboat's puller (`design/DESIGN.md`, "Install operations", Lifeboat; Shape, "Lifeboat feed";
//! `decisions/DECISIONS.md`, "`lifeboat-bundle`"), over a `Host` so it runs natively in tests as the bundle runs it
//! from its puller's alarm. `Puller` decides the sources; this moves the bytes.
//!
//! - **Feed.** Each `export/` object after the puller's position, from the install's export bucket over R2's S3 API
//!   (`crate::feed`), at most `OBJECTS_PER_PASS`. Its rows go to each chain's copy (`Host::import`), and the cursor
//!   moves only by what the copy holds after the import.
//! - **Edge.** Each `Puller::pulls` tail, signed with the host's key, a page at a time; a page that does not start at
//!   the copy's next index is refused, never stitched.
//! - **Packs.** Every stored pack a repository row names, from `packs/{id}?network=` and checked against the row's
//!   `PackRef.checksum` before `graft-blob` keeps it (`Host::put_pack`). One the install does not serve yet stays owed.
//! - **Halt.** A chain whose copy halted is moved by no source again (`Puller::halt`), and the pass names it.
//! - **Unreachable feed.** A feed that does not answer is a stale one: the pass goes on to the edge, and once no
//!   object arrived for `FEED_STALE_MS` the edge serves every chain.
//! - **Current.** A pass that leaves no chain with more to pull, from either source, is the copy's current moment,
//!   which `LifeboatStatus::cursor_age` counts from.

use std::collections::BTreeSet;

use graftyard_git::export::{self, ExportObject};
use graftyard_git::pack::PackId;
use graftyard_git::repo::{REPO_FORMAT_V1, RepoEvent, stored_packs};
use graftyard_raft::{EntryBody, LogEntry, codec};
use graftyard_types::{ChainId, Digest, Index, NetworkId};
use serde::{Deserialize, Serialize};

use crate::puller::{Puller, Rows};

/// Feed objects one pass takes at most [guess, as the vault's `PAGES_PER_PASS` pages of one object each].
pub const OBJECTS_PER_PASS: usize = 100;
/// Edge pages and pack copies one pass makes at most [guess].
pub const PAGES_PER_PASS: usize = 40;
pub const COPIES_PER_PASS: usize = 40;

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
    pub halted: Vec<ChainId>,
    pub stale: bool,
    /// The feed did not answer (R2 down, or the token refused): the pass went on to the edge.
    pub feed_error: Option<String>,
    /// Whether this pass left more to pull.
    pub more: bool,
}

/// The repository network a chain's packs are in.
fn network_of(chain: ChainId) -> Option<NetworkId> {
    match chain {
        ChainId::Repo(r) | ChainId::RepoMeta(r) => Some(NetworkId(r.0)),
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

/// Imports one chain's rows and moves the cursor by what the copy then holds.
async fn take<H: Host>(h: &H, st: &mut State, r: &mut Report, rows: Rows) -> Result<(), String> {
    let got = h.import(rows.chain, &rows.rows).await?;
    let applied = (got.held.0 + 1).saturating_sub(rows.first.0) as usize;
    st.owed.extend(owed_by(rows.chain, &rows.rows[..applied.min(rows.rows.len())]));
    st.puller.applied(rows.chain, got.held);
    if got.halt.is_some() {
        st.puller.halt(rows.chain);
        r.halted.push(rows.chain);
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
            for rows in st.puller.take_object(&o, h.now())? {
                take(h, st, &mut r, rows).await?;
            }
            after = Some(k);
            r.objects += 1;
        }
        if !more {
            break;
        }
    }
    // The edge: every known chain while the feed is stale, else the chains the feed left behind.
    r.stale = st.puller.stale(h.now());
    let mut edge_more = false;
    'edge: loop {
        let pulls = st.puller.pulls(h.now());
        let mut moved = false;
        for p in pulls {
            if r.pages == PAGES_PER_PASS {
                edge_more = true;
                break 'edge;
            }
            // A chain the edge cannot serve now (a 503 while its followers move) is asked again next pass.
            let Ok(Page::Rows { first, rows }) = h.entries(p.chain, p.from, p.limit).await else { continue };
            r.pages += 1;
            if rows.is_empty() {
                continue;
            }
            if !r.edge_chains.contains(&p.chain) {
                r.edge_chains.push(p.chain);
            }
            let rows = st.puller.take_page(p.chain, first, rows)?;
            take(h, st, &mut r, rows).await?;
            moved = true;
        }
        if !moved {
            break;
        }
    }
    // Packs, oldest owed first. One the install does not serve yet (404) stays owed; a corrupt copy stays owed too,
    // and is counted.
    for p in st.owed.clone().into_iter().take(COPIES_PER_PASS) {
        match h.put_pack(&p).await {
            Ok(Copied::Kept) => {
                st.owed.remove(&p);
                r.packs += 1;
            }
            Ok(Copied::Status(_) | Copied::Corrupt) | Err(_) => {}
        }
    }
    r.owed = st.owed.len();
    r.more = feed_more || edge_more || st.owed.len() > COPIES_PER_PASS;
    // Current: nothing left to pull from the feed, and the edge had nothing more for any chain it was asked.
    if !r.more && r.halted.is_empty() && st.puller.halted.is_empty() {
        st.current_at = h.now();
    }
    st.objects += r.objects as u64;
    st.pages += r.pages as u64;
    st.packs += r.packs as u64;
    Ok(r)
}

/// How often a lifeboat reports its decoder at least (`design/DESIGN.md`, "Install operations", Lifeboat: "when its
/// decoder changes and at least daily"): a little under a day, so a daily alarm never slips past one.
pub const DECODER_EVERY_MS: u64 = 23 * 3_600_000;

/// Whether the puller posts a `LifeboatDecoder` for `release` at `now`: none committed yet, another release than the
/// last one committed, or the last one `DECODER_EVERY_MS` old.
pub fn decoder_due(st: &State, release: [u8; 32], now: u64) -> bool {
    st.decoder_sent != Some(release) || now.saturating_sub(st.decoder_at) >= DECODER_EVERY_MS
}

impl State {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("json")
    }

    pub fn decode(b: &[u8]) -> Result<State, String> {
        serde_json::from_slice(b).map_err(|e| e.to_string())
    }
}
