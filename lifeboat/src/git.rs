//! The lifeboat's git copy (`design/DESIGN.md`, "Install operations", Lifeboat; `decisions/DECISIONS.md`,
//! "`lifeboat-clone`"): `graft-git`, the git cache shard's own server and git on the host beside `graft-blob`, fed by
//! the shards' own `graftyard_git::catchup::catch_up` from each repository's local learner, so a clone is answered
//! from the same rules a shard serves by.
//!
//! - **The log.** The network's chain read from its learner (`LearnerLog`), which keeps every entry from index 1: a
//!   lifeboat never truncates. No promotion is read from the record, so a rehydrate loads every pack from the tail in
//!   index order; the rolls come from the copy's fold, never from buckets.
//! - **The packs.** A stored pack comes from `graft-blob` (`State::owed` put it there), checked by `graft-git`'s
//!   `/spool` against its SHA-256 as it writes; one `graft-blob` lacks stops the run there, and the network waits.
//! - **Rolls.** A pack a committed roll consumed is never fetched, so a copy enrolled after the install deleted it
//!   still rebuilds: the roll's produced pack loads first in its place (`roll_markers`), and the pass stops owing the
//!   consumed packs once it holds the produced one (`pass::rolled_off`).
//! - **The cursor.** `IngestCursor` and a rehydrate's place, in the puller's storage as `catch_up` saves them, keyed
//!   by `graft-git`'s disk id, so a new disk rehydrates and a restart on the same one goes on. A `graft-git` run the
//!   puller has not seen (`/health`'s `boot`) is checked with `/connected` per repository first, and a failed check
//!   drops the cursor, so the repository rehydrates.
//! - **Readiness.** A network serves only once its cursor describes `graft-git`'s disk, no rehydrate is saved, and it
//!   was checked on `graft-git`'s current run (`ready`); the front answers every other git request 503 with
//!   `Retry-After`, never an empty, short, or partial pack.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_git::group::RefsPage;
use graftyard_git::pack::{Compacted, Marker, PackId, PackRef, Promoted};
use graftyard_git::{ChainLog, Head, LogPacks, StoreError};
use graftyard_types::{ChainId, Index, NetworkId};
use serde::{Deserialize, Serialize};

/// Git pieces one pass runs at most, round the networks [guess]: each is a `catch_up` `PIECE`, about 1.5 s hosted.
pub const GIT_PIECES_PER_PASS: usize = 4;
/// What the front tells a client to wait while a network is not ready [guess].
pub const NOT_READY_RETRY_S: u32 = 30;
/// How long a network whose last catch-up failed waits before the next [guess].
pub const GIT_RETRY_MS: u64 = 30_000;

/// One network's git copy, in the puller's state. Its `IngestCursor` and a rehydrate's place are kept apart, written
/// as `catch_up` saves them, so a puller that stops mid-rehydrate never takes a half-built repository for a whole one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Net {
    /// The copy's index the last catch-up that reached it ended at, and when, in the lifeboat's clock: what the
    /// front serves, and the lag it publishes for git. `None` until a catch-up on the current `graft-git` run ends.
    pub served: Option<(Index, u64)>,
    /// Why the last catch-up failed, and when: the front's 503 says it, and the network waits `GIT_RETRY_MS`.
    pub error: Option<String>,
    #[serde(default)]
    pub failed_at: u64,
    /// The `graft-git` run (`/health`'s `boot`) this network's repository was last checked or caught up on: a run
    /// the puller did not start it on is checked with `/connected` before it serves again.
    #[serde(default)]
    pub checked: Option<String>,
}

/// Every network's git copy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCopy {
    pub nets: BTreeMap<NetworkId, Net>,
    /// The network the last pass's git work ended on: the next pass starts after it.
    pub turn: Option<NetworkId>,
}

/// What the front answers for a network's git requests.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Ready {
    /// Served from `graft-git` as it is, `lag_ms` behind the copy.
    Ready { at: Index, lag_ms: u64 },
    /// 503 with `Retry-After`: no repository yet, a rebuild in progress, a check after a restart, a catch-up waiting
    /// on a pack, or `graft-git` down.
    NotReady { why: String, retry_s: u32 },
    /// 503 with no `Retry-After`: the copy halted, and no retry moves it.
    Halted,
}

/// What `ready` reads beside a network's `Net`, each from where it is written at once rather than with the pass's
/// state: `graft-git`'s `/health` now, and the cursor and rehydrate keys `catch_up` saves.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// `graft-git`'s disk and run now; `None` when it does not answer.
    pub health: Option<(String, String)>,
    /// The disk the network's saved cursor describes.
    pub cursor_disk: Option<String>,
    pub rehydrating: bool,
    pub halted: bool,
}

/// Whether a network serves, with its copy at `held` at `now`. Only a repository whose cursor describes the disk
/// `graft-git` runs on, with no rehydrate saved and checked on this run, serves: a rehydrate drops or lacks the cursor
/// before its `/init` empties the repository, so a clone during one gets 503, never an empty or short pack.
pub fn ready(net: Option<&Net>, f: &Facts, held: Index, now: u64) -> Ready {
    let wait = |why: &str| Ready::NotReady { why: why.into(), retry_s: NOT_READY_RETRY_S };
    if f.halted {
        return Ready::Halted;
    }
    let Some((disk, boot)) = &f.health else {
        return Ready::NotReady { why: "graft-git does not answer".into(), retry_s: DOWN_RETRY_S };
    };
    if f.rehydrating {
        return wait("the git copy is rebuilding this repository");
    }
    if f.cursor_disk.as_ref() != Some(disk) {
        return match net.and_then(|n| n.error.as_deref()) {
            Some(e) => wait(&format!("the git copy is not ready: {e}")),
            None => wait("the git copy has not reached this repository yet"),
        };
    }
    match net {
        Some(Net { served: Some((at, when)), checked: Some(c), .. }) if c == boot => {
            Ready::Ready { at: *at, lag_ms: if *at >= held { 0 } else { now.saturating_sub(*when) } }
        }
        _ => wait("graft-git restarted; the git copy is checking this repository"),
    }
}

/// What the front tells a client to wait while `graft-git` does not answer [guess].
pub const DOWN_RETRY_S: u32 = 5;

impl GitCopy {
    /// The networks to catch up at `now`, from the one after `turn`: the copy moved past what was served, nothing was
    /// served yet, or the repository was not checked on `boot`, the run `graft-git` answers now. A network in
    /// `waiting` (its packs are still owed, or its copy halted or is gone) and one that failed less than
    /// `GIT_RETRY_MS` ago wait.
    pub fn due(
        &self,
        held: &BTreeMap<NetworkId, Index>,
        waiting: &BTreeSet<NetworkId>,
        boot: &str,
        now: u64,
    ) -> Vec<NetworkId> {
        let moved = |n: &NetworkId, at: &Index| match self.nets.get(n) {
            None => true,
            Some(g) => {
                now.saturating_sub(g.failed_at) >= GIT_RETRY_MS
                    && (g.checked.as_deref() != Some(boot) || g.served.is_none_or(|(s, _)| s < *at))
            }
        };
        let (later, earlier): (Vec<_>, Vec<_>) = held
            .iter()
            .filter(|(n, at)| !waiting.contains(n) && moved(n, at))
            .map(|(n, _)| *n)
            .partition(|n| self.turn.is_some_and(|t| *n > t));
        later.into_iter().chain(earlier).collect()
    }
}

/// One `/entries` page of the copy's member, as a git cache shard's `GroupLog::entries` reads it
/// (`Family::entry_json`): each entry's packs, the `Promoted` it commits, and its ref changes.
pub fn entries_page(v: &serde_json::Value) -> Result<Vec<LogPacks>, StoreError> {
    let bad = |e: serde_json::Error| StoreError(format!("an /entries page: {e}"));
    let page: Vec<serde_json::Value> = serde_json::from_value(v["entries"].clone()).map_err(bad)?;
    page.into_iter()
        .map(|e| {
            let field = |k: &str| e.get(k).cloned().unwrap_or(serde_json::Value::Null);
            Ok(LogPacks {
                index: Index(e["index"].as_u64().unwrap_or(0)),
                packs: serde_json::from_value(field("packs")).map_err(bad)?,
                promoted: serde_json::from_value(field("promoted")).map_err(bad)?,
                compacted: None,
                consumed: Vec::new(),
                refs: serde_json::from_value(field("refs")).map_err(bad)?,
            })
        })
        .collect()
}

/// A network's rolls as `ShardIo::markers` answers them: each produced pack's marker, at its roll's `to`. A shard's
/// rehydrate counts a roll only when its produced pack is marked, then loads it first and drops every pack it consumed
/// (`decisions/DECISIONS.md`, `roll-up`, "Rehydrate prefers a rolled pack"). A lifeboat lists no buckets; the rolls its
/// copy's fold holds are the same record. The index stands in for the roll's own entry, which the fold does not keep:
/// a rehydrate reads it only to order loads, and loads a counted roll's pack before any other.
pub fn roll_markers(chain: ChainId, rolls: &[Compacted]) -> Vec<(PackId, Marker)> {
    rolls
        .iter()
        .filter_map(|r| match &r.produced {
            PackRef::Stored { id, checksum, bytes, .. } => {
                Some((*id, Marker { chain, index: r.to, checksum: *checksum, bytes: *bytes }))
            }
            PackRef::Inline(_) => None,
        })
        .collect()
}

/// The learner a lifeboat reads a network's chain from: its committed entries, its refs, and its fold's ranges.
#[allow(async_fn_in_trait)]
pub trait Learner {
    /// Committed entries from `from` through at most `to`, a page at a time (`/entries?local=1`): fewer than asked
    /// is a page, never a gap.
    async fn entries(&self, chain: ChainId, from: Index, to: Index) -> Result<Vec<LogPacks>, StoreError>;
    /// One page of the chain's refs after `after`, with the index it was read at.
    async fn refs(&self, chain: ChainId, after: Option<String>) -> Result<(RefsPage, Index), StoreError>;
    /// The fold's index, its default branch, and its rolls, oldest first, from one read (`ranges`).
    async fn ranges(&self, chain: ChainId) -> Result<(Index, Option<String>, Vec<Compacted>), StoreError>;
}

/// `ChainLog` over a lifeboat's learner. `first` is 1: the learner holds every entry, so no `Promoted` is read from
/// the record and a rehydrate loads every pushed and promoted pack from the tail in index order. The head is read
/// from the copy's fold each time, as a shard's `GroupLog` reads its leader, so a rehydrate whose refs pages were
/// read past its tail catches up to them. The rolls, with `roll_markers` on the `ShardIo` side, make a rehydrate load
/// each produced pack first and fetch no pack a roll consumed.
pub struct LearnerLog<'a, L: Learner> {
    pub learner: &'a L,
}

impl<L: Learner> ChainLog for LearnerLog<'_, L> {
    async fn head(&self, chain: ChainId) -> Result<Head, StoreError> {
        let (applied, default_branch, _) = self.learner.ranges(chain).await?;
        Ok(Head { first: Index(1), applied, default_branch })
    }
    async fn refs_page(&self, chain: ChainId, after: Option<String>) -> Result<(RefsPage, Index), StoreError> {
        self.learner.refs(chain, after).await
    }
    async fn entries(&self, chain: ChainId, from: Index, to: Index) -> Result<Vec<LogPacks>, StoreError> {
        self.learner.entries(chain, from, to).await
    }
    async fn last_promoted(&self, _chain: ChainId) -> Result<Option<(Index, Promoted)>, StoreError> {
        Ok(None)
    }
    async fn compactions(&self, chain: ChainId) -> Result<Vec<Compacted>, StoreError> {
        Ok(self.learner.ranges(chain).await?.2)
    }
}

/// The repository chain whose packs a network holds: its root repository's (`NetworkId::root`).
pub fn chain_of(network: NetworkId) -> ChainId {
    ChainId::Repo(network.root())
}
