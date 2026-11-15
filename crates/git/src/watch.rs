//! The watcher index and the `Counts` consumer's pure half (`design/DESIGN.md`, Shape, \"Identity\", `UserRecord`;
//! `design/SHARED.md`, `rest:activity`; `build/BUILD-PLAN.md`, M4-WP2).
//!
//! Stars, watches, and thread subscriptions are `IdentityEvent::Record` entries on the actor's own `Identity` chain.
//! The `Counts` consumer reads them and writes a derived copy to each repository's metadata tier, which
//! `/stargazers`, `/subscribers`, the repository's counts, and `Notify` read:
//!
//! - `watcher(account)`: when the account starred the repository, whether it watches or ignores it, and `src`.
//! - `stargazer(starred_at, account)` and `subscriber(account)`: the two lists' orders, as indexes of `watcher`.
//! - `thread_sub(n, account)`: an explicit thread subscription or unsubscription, and `src`.
//! - `watch_count`: the number of stargazers and of subscribers, which `full-repository` renders.
//!
//! `src` is the index of the account's `Identity` chain the row was read at. `Counts` sends each changed (account,
//! repository) pair's state as the account's fold holds it then, never a record's delta, so a row never takes an older
//! `src` and a redelivered or reordered batch changes nothing (`CountsCmd::decide`).
//!
//! **Format 32.19** (`REPO_FORMAT_WATCH`): `CountsCmd` and `RepoEvent::Watchers` (tag 42). A member reads it only
//! after a site day onto a release that lists it, and `Counts` sends none before `FormatsEnable` enabled it. Versions
//! 17 and 18 are other work's.

use std::collections::BTreeMap;

use graftyard_fold::{Read, Rows, Val};
use graftyard_identity::{IdentityEvent, IdentityFold, UserRecord};
use graftyard_raft::{EntryBody, Format, LogEntry, Record, TierSet};
use graftyard_types::{AccountId, DbId, Index, RepoId, RequestId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::issues::IssueNumber;
use crate::repo::{RepoEvent, RepoScratch};
use crate::tiers::{get_rec, int, n_key, put_rec};

pub const REPO_FORMAT_WATCH: Format = Format { family: 0x20, version: 19 };

/// `Counts` commits at most one entry a second per repository, in the Bot class (DESIGN, Shape, `UserRecord`).
pub const COUNTS_EVERY_MS: u64 = 1_000;
/// Updates in one `CountsCmd`, so a viral repository's entry stays small [guess].
pub const COUNTS_BATCH_MAX: usize = 500;

/// One account's row in a repository's watcher index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatcherRow {
    pub starred_at: Option<Timestamp>,
    pub watching: bool,
    pub ignored: bool,
    pub src: Index,
}

impl WatcherRow {
    /// A subscriber in GHES's sense: watching and not ignoring.
    pub fn subscribed(&self) -> bool {
        self.watching && !self.ignored
    }
}

/// One thread subscription row: subscribed or explicitly unsubscribed, and `src`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSubRow {
    pub subscribed: bool,
    pub src: Index,
}

/// What `Counts` writes. Wire tags: add variants at the end only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatcherUpdate {
    /// The account's star and watch state for the repository as of `row.src`.
    Watcher { account: AccountId, row: WatcherRow },
    /// The account's subscription to thread `n` as of `row.src`.
    ThreadSub { n: IssueNumber, account: AccountId, row: ThreadSubRow },
}

impl WatcherUpdate {
    fn src(&self) -> Index {
        match self {
            WatcherUpdate::Watcher { row, .. } => row.src,
            WatcherUpdate::ThreadSub { row, .. } => row.src,
        }
    }
}

/// The repository's counts: stargazers and subscribers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchCount {
    pub stars: u32,
    pub subscribers: u32,
}

// ---- rows ----

fn acct(a: AccountId) -> Val {
    int(a.0.0)
}

pub fn watcher(meta: &(impl Read + ?Sized), account: AccountId) -> Option<WatcherRow> {
    get_rec(meta, "watcher", &[acct(account)])
}

pub fn thread_sub(meta: &(impl Read + ?Sized), n: IssueNumber, account: AccountId) -> Option<ThreadSubRow> {
    get_rec(meta, "thread_sub", &[n_key(n), acct(account)])
}

pub fn watch_count(meta: &(impl Read + ?Sized)) -> WatchCount {
    get_rec(meta, "watch_count", &[]).unwrap_or_default()
}

fn ids(rows: Vec<(Vec<Val>, Vec<Val>)>, col: usize) -> Vec<AccountId> {
    rows.into_iter()
        .filter_map(|(k, _)| match k.get(col) {
            Some(Val::Int(i)) => Some(AccountId(DbId(*i))),
            _ => None,
        })
        .collect()
}

/// Every stargazer with when it starred, oldest first, as GHES lists `/stargazers`.
pub fn stargazers(meta: &(impl Read + ?Sized)) -> Vec<(AccountId, Timestamp)> {
    meta.prefix("stargazer", &[])
        .into_iter()
        .filter_map(|(k, _)| match (k.first(), k.get(1)) {
            (Some(Val::Int(t)), Some(Val::Int(a))) => Some((AccountId(DbId(*a)), Timestamp(*t as u64))),
            _ => None,
        })
        .collect()
}

/// Every subscriber (watching, not ignoring), by account id [inference: GHES documents no order].
pub fn subscribers(meta: &(impl Read + ?Sized)) -> Vec<AccountId> {
    ids(meta.prefix("subscriber", &[]), 0)
}

/// Thread `n`'s explicit subscriptions and unsubscriptions.
pub fn thread_subs(meta: &(impl Read + ?Sized), n: IssueNumber) -> Vec<(AccountId, bool)> {
    meta.prefix("thread_sub", &[n_key(n)])
        .into_iter()
        .filter_map(|(k, _)| match k.get(1) {
            Some(Val::Int(a)) => {
                let a = AccountId(DbId(*a));
                thread_sub(meta, n, a).map(|r| (a, r.subscribed))
            }
            _ => None,
        })
        .collect()
}

/// Applies one update. `CountsCmd::decide` already dropped any whose row has a `src` at or above it.
pub(crate) fn write_meta(w: &mut dyn Rows, u: &WatcherUpdate) {
    match *u {
        WatcherUpdate::Watcher { account, row } => {
            let old = watcher(w, account);
            let mut count = watch_count(w);
            if let Some(o) = old {
                if let Some(t) = o.starred_at {
                    w.delete("stargazer", &[int(t.0 as i64), acct(account)]);
                    count.stars -= 1;
                }
                if o.subscribed() {
                    w.delete("subscriber", &[acct(account)]);
                    count.subscribers -= 1;
                }
            }
            if let Some(t) = row.starred_at {
                put_rec(w, "stargazer", vec![int(t.0 as i64), acct(account)], &());
                count.stars += 1;
            }
            if row.subscribed() {
                put_rec(w, "subscriber", vec![acct(account)], &());
                count.subscribers += 1;
            }
            // An emptied row keeps its `src`, so `decide` still refuses an older update arriving later.
            put_rec(w, "watcher", vec![acct(account)], &row);
            put_rec(w, "watch_count", Vec::new(), &count);
        }
        WatcherUpdate::ThreadSub { n, account, row } => {
            put_rec(w, "thread_sub", vec![n_key(n), acct(account)], &row);
        }
    }
}

// ---- the command ----

/// `Counts`' write, `{"counts": ..}`, format 32.19. It carries no account and no key, as every consumer command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountsCmd {
    pub updates: Vec<WatcherUpdate>,
    pub request: RequestId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountsOut {
    pub applied: u32,
}

impl Record for CountsCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        graftyard_fold::canon::write(out, self);
    }
}

impl CountsCmd {
    /// Keeps only the updates newer than their rows, so a redelivered batch commits nothing. A command whose leader
    /// has no metadata tier loaded yet is retried by the consumer, which sees `applied: 0` and its cursor unmoved.
    pub fn decide(&self, s: &RepoScratch<'_>) -> (SmallVec<[RepoEvent; 4]>, CountsOut) {
        let Some((_, meta)) = s.view() else { return (SmallVec::new(), CountsOut { applied: 0 }) };
        let fresh: Vec<WatcherUpdate> = self
            .updates
            .iter()
            .filter(|u| match u {
                WatcherUpdate::Watcher { account, .. } => watcher(meta, *account).is_none_or(|o| o.src < u.src()),
                WatcherUpdate::ThreadSub { n, account, .. } => {
                    thread_sub(meta, *n, *account).is_none_or(|o| o.src < u.src())
                }
            })
            .copied()
            .collect();
        let applied = fresh.len() as u32;
        if fresh.is_empty() {
            return (SmallVec::new(), CountsOut { applied });
        }
        (smallvec::smallvec![RepoEvent::Watchers(fresh)], CountsOut { applied })
    }

    pub fn replay(&self, events: &[RepoEvent]) -> CountsOut {
        let applied = events.iter().map(|e| if let RepoEvent::Watchers(u) = e { u.len() as u32 } else { 0 }).sum();
        CountsOut { applied }
    }

    pub fn tiers(&self) -> TierSet {
        TierSet::METADATA
    }
}

// ---- the consumer's pure half ----

/// The (repository, account, thread) keys one `Identity` entry's records touch, in order.
fn touched(e: &LogEntry<IdentityEvent>) -> Vec<(RepoId, Option<IssueNumber>)> {
    let EntryBody::Events(evs) = &e.body else { return Vec::new() };
    let mut out = Vec::new();
    for ev in evs {
        let IdentityEvent::Record(r) = ev else { continue };
        let k = match r {
            UserRecord::Star { repo, .. } | UserRecord::Watch { repo, .. } => (*repo, None),
            UserRecord::Subscription { thread, .. } => (thread.repo, Some(IssueNumber(thread.number))),
            UserRecord::Follow { .. } | UserRecord::Thread { .. } => continue,
        };
        if !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

/// One batch of `Counts` work on one account's chain: the account's committed entries after its cursor, read with
/// the account's fold at `applied`. Returns each repository's updates, every one the fold's state for its key with
/// `src = applied`, so a later batch's row always wins and an earlier one's never does. The consumer commits each
/// repository's `CountsCmd` (at most `COUNTS_BATCH_MAX` updates, one entry a second), then the account's
/// `CountsCursor { through: applied }`.
pub fn counts_plan(
    account: AccountId,
    fold: &IdentityFold,
    applied: Index,
    entries: &[LogEntry<IdentityEvent>],
) -> BTreeMap<RepoId, Vec<WatcherUpdate>> {
    let u = fold.user();
    let mut out: BTreeMap<RepoId, Vec<WatcherUpdate>> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.index > u.counts_cursor && e.index <= applied) {
        for (repo, n) in touched(e) {
            let ups = out.entry(repo).or_default();
            let up = match n {
                None => {
                    let w = u.watches.get(&repo);
                    WatcherUpdate::Watcher {
                        account,
                        row: WatcherRow {
                            starred_at: u.stars.get(&repo).map(|s| s.at),
                            watching: w.is_some_and(|w| w.subscribed),
                            ignored: w.is_some_and(|w| w.ignored),
                            src: applied,
                        },
                    }
                }
                Some(n) => {
                    let thread = graftyard_identity::ThreadId { repo, number: n.0 };
                    let subscribed = u.subscriptions.get(&thread).is_some_and(|(s, _)| *s);
                    WatcherUpdate::ThreadSub { n, account, row: ThreadSubRow { subscribed, src: applied } }
                }
            };
            if !ups.contains(&up) {
                ups.push(up);
            }
        }
    }
    out
}

// ---- the cursors of `Notify` and `Search` ----

/// `Notify`'s or `Search`'s position on this chain, `{"consumerCursor": ..}`, format 32.19: the core
/// `consumer_cursor` row (`RepoEvent::ConsumerCursor`, tag 22), committed after the batch's notices are in every
/// `Inbox` or its documents in their shards, so a lost batch runs again and only rewrites what it wrote. Those two and
/// `ActionsTrigger` send it: `PrSync` and `MergeQueue` commit theirs with their own work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsumerCursorCmd {
    pub consumer: crate::issues::Consumer,
    pub through: Index,
    pub request: RequestId,
}

impl Record for ConsumerCursorCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        graftyard_fold::canon::write(out, self);
    }
}

impl ConsumerCursorCmd {
    /// Only forward, and only for `Notify`, `Search`, and `ActionsTrigger` (BF-37, which commits its cursor after the
    /// runs it created); anything else commits nothing.
    pub fn decide(&self, s: &RepoScratch<'_>) -> (SmallVec<[RepoEvent; 4]>, CountsOut) {
        use crate::issues::Consumer::{ActionsTrigger, Notify, Search};
        let cur = crate::tiers::consumer_cursor(s.core(), self.consumer);
        if !matches!(self.consumer, Notify | Search | ActionsTrigger) || self.through <= cur {
            return (SmallVec::new(), CountsOut { applied: 0 });
        }
        let ev = RepoEvent::ConsumerCursor { consumer: self.consumer, through: self.through };
        (smallvec::smallvec![ev], CountsOut { applied: 1 })
    }

    pub fn replay(&self, events: &[RepoEvent]) -> CountsOut {
        CountsOut { applied: events.iter().filter(|e| matches!(e, RepoEvent::ConsumerCursor { .. })).count() as u32 }
    }
}
