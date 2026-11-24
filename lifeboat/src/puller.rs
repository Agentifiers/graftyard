//! Which source the copy pulls from, per chain (`design/DESIGN.md`, "Failure modes", `lifeboat` control-plane: "When
//! the R2 feed is stale it pulls log tails and packs from followers through the edge's export endpoints, signed with
//! its registered key").
//!
//! - **Feed.** Each `ExportObject` after the puller's position, in order. Each chain's rows go to the importer from the
//!   copy's next index: rows the copy already holds (an edge pull got there first) are dropped, and a chain whose rows
//!   start past the copy's next index is marked behind, to be pulled from the edge up to the object's index, never
//!   stitched. That covers a chain the feed first carried past index 1, and one the copy fell behind on.
//! - **Stale.** With no new feed object for `FEED_STALE_MS`, every known chain is pulled from the edge from its next
//!   index, a page at a time. The edge serves the entries endpoint from followers and learners, never the leader. A
//!   chain the feed never named is unknown here and waits for the feed [inference: no endpoint lists chains].
//! - **Turns.** The pulls go round the known chains from the one after the last chain asked (`turn`), so every chain
//!   is asked within one cycle whatever the pass budgets, never only the first ones in key order. A cycle starts each
//!   time the turn comes round; one in which every chain answered caught up (or gone) is the copy's current moment as
//!   of the cycle's start (`decisions/DECISIONS.md`, "`lifeboat-turns`").
//! - **Halts.** A chain the importer halted on (a refused row, or one that does not chain) is moved by no source again,
//!   and `halted` names it for the alarm. Nothing is ever skipped: a cursor moves only by what the importer applied.
//! - **Gone.** A chain the edge answers 404 has no group: a repository purged or retired for good, every member
//!   retired (`decisions/DECISIONS.md`, "`export-retired`"). The copy keeps what it holds, no source moves it again,
//!   and it holds no pass back from current. Before, the edge answered such a chain 503 "retry", and a stale feed
//!   asked for it on every pass for good.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_git::export::{ExportCursors, ExportObject, FeedPosition};
use graftyard_types::{ChainId, Index};
use serde::{Deserialize, Serialize};

/// With no new feed object this long, the puller turns to the edge: four feed rounds at their measured ~15 s
/// (`export-period`) [guess].
pub const FEED_STALE_MS: u64 = 60_000;
/// One tail pull's page, as the entries endpoint bounds it.
pub const TAIL_PAGE: u64 = graftyard_git::export::ENTRIES_PAGE;

/// What the puller keeps in its state file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Puller {
    /// The newest feed object taken.
    pub feed: Option<FeedPosition>,
    /// When it was taken, in the lifeboat's clock.
    pub feed_seen: u64,
    /// Each known chain's last index the copy holds.
    pub held: ExportCursors,
    /// Chains whose feed rows started past the copy's next index, with the index the feed had reached.
    pub behind: BTreeMap<ChainId, Index>,
    pub halted: BTreeSet<ChainId>,
    #[serde(default)]
    pub gone: BTreeSet<ChainId>,
    /// The last chain whose edge pull ended: the next pass starts after it.
    #[serde(default)]
    pub turn: Option<ChainId>,
    /// While the feed is stale: when the current cycle of the turns started, and whether a chain in it failed to
    /// answer caught up.
    #[serde(default)]
    pub cycle: Option<u64>,
    #[serde(default)]
    pub cycle_dirty: bool,
}

/// Rows for the importer of one chain, starting at the copy's next index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rows {
    pub chain: ChainId,
    pub first: Index,
    pub rows: Vec<Vec<u8>>,
}

/// One tail pull: `GET export/chains/{chain}/entries?from=&limit=`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pull {
    pub chain: ChainId,
    pub from: Index,
    pub limit: u64,
}

impl Puller {
    /// Whether the feed is stale at `now`: no object yet, or none new for `FEED_STALE_MS`.
    pub fn stale(&self, now: u64) -> bool {
        self.feed.is_none() || now.saturating_sub(self.feed_seen) >= FEED_STALE_MS
    }

    pub fn next(&self, c: ChainId) -> Index {
        Index(self.held.get(c).0 + 1)
    }

    /// Takes the feed object after `feed`; one at or before it is ignored. Returns each chain's rows from the copy's
    /// next index. An object whose shape does not hold (a chain named without its rows, or a row count that does not
    /// reach its index) is refused whole and the position stays.
    pub fn take_object(&mut self, o: &ExportObject, now: u64) -> Result<Vec<Rows>, String> {
        if self.feed.is_some_and(|p| o.batch.position() <= p) {
            return Ok(Vec::new());
        }
        if o.entries.len() != o.batch.chains.len() {
            return Err(format!("{} chains named, {} with entries", o.batch.chains.len(), o.entries.len()));
        }
        for ((c, to), (ec, first, rows)) in o.batch.chains.iter().zip(&o.entries) {
            if c != ec || first.0 == 0 || first.0 + rows.len() as u64 != to.0 + 1 {
                return Err(format!("{c}: entries {}..+{} do not reach {}", first.0, rows.len(), to.0));
            }
        }
        self.feed = Some(o.batch.position());
        self.feed_seen = now;
        let mut out = Vec::new();
        for ((c, to), (_, first, rows)) in o.batch.chains.iter().zip(&o.entries) {
            if self.halted.contains(c) || self.gone.contains(c) {
                continue;
            }
            let next = self.next(*c);
            if !self.held.0.contains_key(c) {
                self.held.0.insert(*c, Index(0));
            }
            if first.0 > next.0 {
                let b = self.behind.entry(*c).or_insert(*to);
                *b = (*b).max(*to);
                continue;
            }
            let skip = (next.0 - first.0) as usize;
            if skip < rows.len() {
                out.push(Rows { chain: *c, first: next, rows: rows[skip..].to_vec() });
            }
        }
        Ok(out)
    }

    /// The importer now holds `chain` through `to`. A cursor never moves back, and a chain caught up to the index the
    /// feed had reached is no longer behind.
    pub fn applied(&mut self, chain: ChainId, to: Index) {
        let at = self.held.0.entry(chain).or_insert(Index(0));
        *at = (*at).max(to);
        if self.behind.get(&chain).is_some_and(|b| *at >= *b) {
            self.behind.remove(&chain);
        }
    }

    /// The importer halted `chain`: no source moves it again.
    pub fn halt(&mut self, chain: ChainId) {
        self.halted.insert(chain);
        self.behind.remove(&chain);
    }

    /// The edge answered 404 for `chain`: it has no group, and nothing moves it again.
    pub fn gone(&mut self, chain: ChainId) {
        self.gone.insert(chain);
        self.behind.remove(&chain);
    }

    /// This pass's tail pulls at `now`: every known chain while the feed is stale, and otherwise only the chains
    /// behind it, from the chain after `turn` round to it. Never a halted or gone one.
    pub fn pulls(&self, now: u64) -> Vec<Pull> {
        let stale = self.stale(now);
        let due: Vec<ChainId> = self
            .held
            .0
            .keys()
            .filter(|c| !self.halted.contains(*c) && !self.gone.contains(*c) && (stale || self.behind.contains_key(*c)))
            .copied()
            .collect();
        let (later, earlier): (Vec<ChainId>, Vec<ChainId>) =
            due.into_iter().partition(|c| self.turn.is_some_and(|t| *c > t));
        later.into_iter().chain(earlier).map(|c| Pull { chain: c, from: self.next(c), limit: TAIL_PAGE }).collect()
    }

    /// An edge page for `chain` starting at `first`: the rows, if they start at the copy's next index. A page that
    /// starts elsewhere is refused, never stitched.
    pub fn take_page(&self, chain: ChainId, first: Index, rows: Vec<Vec<u8>>) -> Result<Rows, String> {
        let want = self.next(chain);
        if self.halted.contains(&chain) {
            return Err(format!("{chain} is halted"));
        }
        if first != want {
            return Err(format!("{chain}: the page starts at {} where the copy needs {}", first.0, want.0));
        }
        Ok(Rows { chain, first, rows })
    }

    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("json")
    }

    pub fn decode(b: &[u8]) -> Result<Puller, String> {
        serde_json::from_slice(b).map_err(|e| e.to_string())
    }
}
