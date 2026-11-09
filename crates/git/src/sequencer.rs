//! `HookSequencer { owner }`'s pure half: SHARED's delivery store and sequencer, one per hook owner on its home site
//! (`design/DESIGN.md`, Shape, "Identity"; `design/SHARED.md`, "Event fan-out"; "Failure modes", `event pipeline`).
//!
//! - **Rows.** One `HookDelivery` per (webhook, hook), keyed by its GUID. `record` is idempotent by GUID, so a batch
//!   the consumer sends twice, or a range replayed after a rebuild, adds nothing it already holds.
//! - **Queues.** One per (chain, hook), in the order the consumer recorded them, which is chain order. A queue runs at
//!   most one attempt at a time, so order holds per (chain, hook). Pings and manual redeliveries ride a queue of
//!   their own per hook (`chain: None`), so they never hold, and are never held by, the chain's queue.
//! - **Slots.** `start` picks queues round robin and runs at most `HOOK_OWNER_CONCURRENCY` attempts at once; hooks
//!   whose last attempt failed or timed out share at most half of those, so the healthy hooks keep at least 25.
//! - **Attempts.** One automatic attempt per delivery, as on GitHub. A hook with `retry` gets retries with backoff for
//!   `DELIVERY_KEEP_MS` under the same GUID, holding its queue meanwhile. An attempt unreported `RESEND_AFTER_MS`
//!   after it started is re-sent under its GUID (its delivery Worker died), and a late report of it is ignored.
//! - **Cursor halves.** `recorded(chain)` is how far this store holds the chain's rows: the consumer moves
//!   `HookCursor.read` only to it. `replay_from(chain)` is the oldest entry with a delivery not yet finished.
//! - **Loss.** A store lost with its site is rebuilt empty on the owner's new home site: `recorded` is then `None`, so
//!   the consumer replays from the committed `replay_from` (`consumer_start`), which re-records every unfinished
//!   delivery, and redelivers the finished ones in that range, under their own GUIDs. History before is lost.
//!
//! The rows a Durable Object stores are these types through serde; their enums carry pinned wire tags
//! (`crates/git/tests/wire_tags.rs`).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use graftyard_types::{AccountId, ChainId, Index, RepoId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::hooks::{
    ATTEMPT_DEADLINE_MS, DELIVERY_KEEP_MS, Guid, HOOK_OWNER_CONCURRENCY, HookCursor, HookId, HookRecord,
};

/// An attempt its delivery Worker never reported is sent again this long after it started (DESIGN, Shape,
/// `HookSequencer`: "One unreported for 30 s is re-sent under its GUID").
pub const RESEND_AFTER_MS: u64 = 30_000;
/// The first retry's delay for a hook with `retry` set, doubling to at most `RETRY_MAX_MS` [guess].
pub const RETRY_FIRST_MS: u64 = 10_000;
pub const RETRY_MAX_MS: u64 = 3_600_000;

/// How one attempt ended. Wire tags: add variants at the end only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// A 2xx.
    Ok,
    /// Any other status, or a connection the endpoint refused.
    Failed,
    /// No answer within `ATTEMPT_DEADLINE_MS`.
    TimedOut,
}

/// One attempt, which the deliveries endpoints list as one delivery (GitHub's `id` is per attempt).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub id: u64,
    pub at: Timestamp,
    pub duration_ms: u32,
    pub status: Option<u16>,
    pub outcome: Outcome,
    pub redelivery: bool,
    /// The response body's start, for `GET .../deliveries/{id}` [guess: 1 KiB].
    pub response: Option<Box<str>>,
}

/// SHARED's `HookDelivery`, with what the attempts send. `entry` is `None` for a ping sent with no chain entry
/// (`POST .../pings`), whose GUID the host draws. `body` is the rendered JSON payload; `done` once its automatic
/// attempts ended, which `replay_from` reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookDelivery {
    pub guid: Guid,
    pub hook: HookId,
    pub entry: Option<(ChainId, Index, u16)>,
    pub event: Box<str>,
    pub action: Option<Box<str>>,
    pub repo: Option<RepoId>,
    pub body: Box<[u8]>,
    pub attempts: SmallVec<[Attempt; 4]>,
    pub expires: Timestamp,
    pub done: bool,
}

/// What the consumer records: one delivery, before any attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDelivery {
    pub guid: Guid,
    pub hook: HookId,
    pub entry: Option<(ChainId, Index, u16)>,
    pub event: &'static str,
    pub action: Option<&'static str>,
    pub repo: Option<RepoId>,
    pub body: Vec<u8>,
}

/// An attempt to start: the delivery Worker POSTs it (`graftyard_api::webhooks::request`) with a
/// `ATTEMPT_DEADLINE_MS` deadline and reports it with `report`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Start {
    pub guid: Guid,
    pub hook: HookId,
    pub attempt: u64,
    pub redelivery: bool,
}

/// A queue: (chain, hook), or (`None`, hook) for pings and manual redeliveries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct QueueKey {
    pub chain: Option<ChainId>,
    pub hook: HookId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Queue {
    /// GUIDs in order; `true` marks a manual redelivery, which takes one attempt and no retry.
    items: VecDeque<(Guid, bool)>,
    running: Option<Running>,
    not_before: Timestamp,
}

impl Default for Queue {
    fn default() -> Queue {
        Queue { items: VecDeque::new(), running: None, not_before: Timestamp(0) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Running {
    attempt: u64,
    started: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedeliverErr {
    NotFound,
    /// Past `expires`: GitHub's 3 days.
    Expired,
}

/// One owner's store. Everything is a row the object keeps in its SQLite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sequencer {
    pub owner: AccountId,
    targets: BTreeMap<HookId, HookRecord>,
    deliveries: BTreeMap<Guid, HookDelivery>,
    by_attempt: BTreeMap<u64, Guid>,
    queues: BTreeMap<QueueKey, Queue>,
    /// The queue `start` served last, so the next round starts after it.
    last: Option<QueueKey>,
    failing: BTreeSet<HookId>,
    recorded: BTreeMap<ChainId, Index>,
    /// Unfinished deliveries per (chain, index), for `replay_from`.
    pending: BTreeMap<(ChainId, Index), u32>,
    next_attempt: u64,
}

impl Sequencer {
    /// A new or rebuilt store at `now`. Attempt ids start from the time, so a rebuilt store never hands out an id a
    /// client saw from the lost one [inference: GitHub's delivery ids are large and only increase].
    pub fn new(owner: AccountId, now: Timestamp) -> Sequencer {
        Sequencer {
            owner,
            targets: BTreeMap::new(),
            deliveries: BTreeMap::new(),
            by_attempt: BTreeMap::new(),
            queues: BTreeMap::new(),
            last: None,
            failing: BTreeSet::new(),
            recorded: BTreeMap::new(),
            pending: BTreeMap::new(),
            next_attempt: now.0.saturating_mul(1_000).max(1),
        }
    }

    /// The hooks' current records, as the consumer read them with the batch. An attempt goes to the hook's current
    /// URL with its current secret; a hook no longer listed, or inactive, gets no further attempt.
    pub fn set_targets(&mut self, chain_hooks: &[(HookId, HookRecord)]) {
        for (id, r) in chain_hooks {
            self.targets.insert(*id, r.clone());
        }
    }

    pub fn forget_hook(&mut self, id: HookId) {
        self.targets.remove(&id);
    }

    pub fn target(&self, id: HookId) -> Option<&HookRecord> {
        self.targets.get(&id)
    }

    /// Records the rows of `chain`'s entries through `through`, in order. A GUID already held adds nothing.
    pub fn record(&mut self, chain: ChainId, through: Index, rows: Vec<NewDelivery>, now: Timestamp) {
        for r in rows {
            self.add(r, Some(chain), now);
        }
        let at = self.recorded.entry(chain).or_default();
        *at = (*at).max(through);
    }

    fn add(&mut self, r: NewDelivery, chain: Option<ChainId>, now: Timestamp) {
        if self.deliveries.contains_key(&r.guid) {
            return;
        }
        let key = QueueKey { chain, hook: r.hook };
        self.queues.entry(key).or_default().items.push_back((r.guid, false));
        if let Some((c, i, _)) = r.entry {
            *self.pending.entry((c, i)).or_default() += 1;
        }
        self.deliveries.insert(
            r.guid,
            HookDelivery {
                guid: r.guid,
                hook: r.hook,
                entry: r.entry,
                event: r.event.into(),
                action: r.action.map(Into::into),
                repo: r.repo,
                body: r.body.into(),
                attempts: SmallVec::new(),
                expires: Timestamp(now.0 + DELIVERY_KEEP_MS),
                done: false,
            },
        );
    }

    /// `POST .../hooks/{id}/pings`: a ping with no chain entry, under a GUID the host drew.
    pub fn ping(&mut self, r: NewDelivery, now: Timestamp) {
        self.add(NewDelivery { entry: None, ..r }, None, now);
    }

    /// How far this store holds `chain`'s rows; `None` for a store that never recorded the chain (a rebuilt one).
    pub fn recorded(&self, chain: ChainId) -> Option<Index> {
        self.recorded.get(&chain).copied()
    }

    /// SHARED's `replay_from`: the oldest entry of `chain` with a delivery not yet finished, else just past what is
    /// recorded.
    pub fn replay_from(&self, chain: ChainId) -> Index {
        let lo = (chain, Index(0));
        match self.pending.range(lo..).next() {
            Some(((c, i), _)) if *c == chain => *i,
            _ => Index(self.recorded(chain).map_or(0, |r| r.0) + 1),
        }
    }

    /// The cursor the consumer commits once a batch through `through` is recorded.
    pub fn cursor(&self, chain: ChainId) -> HookCursor {
        let read = self.recorded(chain).unwrap_or_default();
        HookCursor { read, replay_from: self.replay_from(chain) }
    }

    /// Attempts in flight now, and how many of them are failing hooks'.
    pub fn in_flight(&self) -> (usize, usize) {
        let running = self.queues.iter().filter(|(_, q)| q.running.is_some());
        let mut n = (0, 0);
        for (k, _) in running {
            n.0 += 1;
            n.1 += usize::from(self.failing.contains(&k.hook));
        }
        n
    }

    /// Attempts to start now: round robin over the queues after the last one served, at most one per queue, at most
    /// `HOOK_OWNER_CONCURRENCY` in flight, failing hooks at most half of them. Also re-sends an attempt unreported for
    /// `RESEND_AFTER_MS`, and drops deliveries whose hook is gone or inactive.
    pub fn start(&mut self, now: Timestamp) -> Vec<Start> {
        for q in self.queues.values_mut() {
            if q.running.is_some_and(|r| now.0 >= r.started.0 + RESEND_AFTER_MS) {
                q.running = None;
            }
        }
        let cap = usize::from(HOOK_OWNER_CONCURRENCY);
        let (mut total, mut failing) = self.in_flight();
        let keys: Vec<QueueKey> = match self.last {
            // After `l`, then from the start through it; `l` may be gone (its queue emptied), so exclude it by bound.
            Some(l) => {
                use std::ops::Bound::{Excluded, Unbounded};
                let after = self.queues.range((Excluded(l), Unbounded));
                after.chain(self.queues.range(..=l)).map(|(k, _)| *k).collect()
            }
            None => self.queues.keys().copied().collect(),
        };
        let mut out = Vec::new();
        for k in keys {
            if total >= cap {
                break;
            }
            let is_failing = self.failing.contains(&k.hook);
            if is_failing && failing >= cap / 2 {
                continue;
            }
            let live = self.targets.get(&k.hook).is_some_and(|t| t.active);
            let q = self.queues.get_mut(&k).expect("listed");
            if q.running.is_some() || now < q.not_before {
                continue;
            }
            let Some(&(guid, manual)) = q.items.front() else { continue };
            if !live {
                // The hook was deleted or deactivated: its queued deliveries end with no attempt.
                let items: Vec<Guid> = q.items.drain(..).map(|(g, _)| g).collect();
                for g in items {
                    self.finish(g);
                }
                continue;
            }
            let attempt = self.next_attempt;
            self.next_attempt += 1;
            q.running = Some(Running { attempt, started: now });
            self.by_attempt.insert(attempt, guid);
            out.push(Start { guid, hook: k.hook, attempt, redelivery: manual });
            self.last = Some(k);
            total += 1;
            failing += usize::from(is_failing);
        }
        self.queues.retain(|_, q| !q.items.is_empty() || q.running.is_some());
        out
    }

    fn finish(&mut self, guid: Guid) {
        let Some(d) = self.deliveries.get_mut(&guid) else { return };
        if d.done {
            return;
        }
        d.done = true;
        if let Some((c, i, _)) = d.entry
            && let Some(n) = self.pending.get_mut(&(c, i))
        {
            *n -= 1;
            if *n == 0 {
                self.pending.remove(&(c, i));
            }
        }
    }

    /// An attempt's result: `status` is the response's, `None` for a timeout or a refused connection
    /// (`timed_out`). A report for an attempt that is no longer the queue's running one (re-sent meanwhile, or from
    /// a store before a rebuild) changes nothing.
    pub fn report(
        &mut self,
        attempt: u64,
        now: Timestamp,
        status: Option<u16>,
        timed_out: bool,
        response: Option<&str>,
    ) {
        let Some(&guid) = self.by_attempt.get(&attempt) else { return };
        let Some(d) = self.deliveries.get(&guid) else { return };
        let hook = d.hook;
        let key = match d.entry {
            Some((c, _, _)) => QueueKey { chain: Some(c), hook },
            None => QueueKey { chain: None, hook },
        };
        // A manual redelivery of a chain delivery rides the hook's own queue.
        let key = [key, QueueKey { chain: None, hook }]
            .into_iter()
            .find(|k| self.queues.get(k).and_then(|q| q.running).is_some_and(|r| r.attempt == attempt));
        let Some(key) = key else { return };
        let q = self.queues.get_mut(&key).expect("found");
        let started = q.running.take().expect("running").started;
        let manual = q.items.front().is_some_and(|(g, m)| *g == guid && *m);
        let outcome = match status {
            Some(s) if (200..300).contains(&s) => Outcome::Ok,
            _ if timed_out => Outcome::TimedOut,
            _ => Outcome::Failed,
        };
        let d = self.deliveries.get_mut(&guid).expect("held");
        d.attempts.push(Attempt {
            id: attempt,
            at: started,
            duration_ms: now.0.saturating_sub(started.0).min(u64::from(u32::MAX)) as u32,
            status,
            outcome,
            redelivery: manual,
            response: response.map(|r| r.chars().take(1024).collect::<String>().into()),
        });
        d.expires = Timestamp(now.0 + DELIVERY_KEEP_MS);
        let retries = d.attempts.iter().filter(|a| !a.redelivery).count() as u32;
        let first = d.attempts.iter().find(|a| !a.redelivery).map_or(now, |a| a.at);
        if outcome == Outcome::Ok {
            self.failing.remove(&hook);
        } else {
            self.failing.insert(hook);
        }
        let retry = self.targets.get(&hook).is_some_and(|t| t.retry);
        let again = outcome != Outcome::Ok && retry && !manual && now.0 < first.0 + DELIVERY_KEEP_MS;
        if again {
            let backoff = RETRY_FIRST_MS.saturating_mul(1 << (retries - 1).min(20)).min(RETRY_MAX_MS);
            q.not_before = Timestamp(now.0 + backoff);
            return;
        }
        q.items.pop_front();
        if !manual {
            self.finish(guid);
        }
    }

    /// `POST .../hooks/{hook}/deliveries/{id}/attempts`: one more attempt of that delivery, under its GUID, on the
    /// hook's own queue, while its 3 days last.
    pub fn redeliver(&mut self, hook: HookId, id: u64, now: Timestamp) -> Result<Guid, RedeliverErr> {
        let guid = *self.by_attempt.get(&id).ok_or(RedeliverErr::NotFound)?;
        let d = self.deliveries.get(&guid).ok_or(RedeliverErr::NotFound)?;
        if d.hook != hook || !d.attempts.iter().any(|a| a.id == id) {
            return Err(RedeliverErr::NotFound);
        }
        if now >= d.expires {
            return Err(RedeliverErr::Expired);
        }
        self.queues.entry(QueueKey { chain: None, hook }).or_default().items.push_back((guid, true));
        Ok(guid)
    }

    pub fn delivery(&self, guid: &Guid) -> Option<&HookDelivery> {
        self.deliveries.get(guid)
    }

    /// `GET .../hooks/{hook}/deliveries`: one item per attempt, newest first, after the attempt id `before` (the
    /// cursor), at most `per_page`.
    pub fn list(&self, hook: HookId, before: Option<u64>, per_page: usize) -> Vec<(&HookDelivery, &Attempt)> {
        let top = before.unwrap_or(u64::MAX);
        self.by_attempt
            .range(..top)
            .rev()
            .filter_map(|(id, g)| {
                let d = self.deliveries.get(g)?;
                let a = d.attempts.iter().find(|a| a.id == *id)?;
                (d.hook == hook).then_some((d, a))
            })
            .take(per_page)
            .collect()
    }

    /// `GET .../hooks/{hook}/deliveries/{id}`.
    pub fn get(&self, hook: HookId, id: u64) -> Option<(&HookDelivery, &Attempt)> {
        let d = self.deliveries.get(self.by_attempt.get(&id)?)?;
        let a = d.attempts.iter().find(|a| a.id == id)?;
        (d.hook == hook).then_some((d, a))
    }

    /// Drops finished deliveries past their 3 days.
    pub fn prune(&mut self, now: Timestamp) {
        let gone: Vec<Guid> = self.deliveries.values().filter(|d| d.done && now >= d.expires).map(|d| d.guid).collect();
        for g in gone {
            self.deliveries.remove(&g);
        }
        let live = &self.deliveries;
        self.by_attempt.retain(|_, g| live.contains_key(g));
    }

    /// Deliveries held, for tests and the store's size.
    pub fn len(&self) -> usize {
        self.deliveries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.deliveries.is_empty()
    }

    /// Whether every queue is empty and nothing runs.
    pub fn idle(&self) -> bool {
        self.queues.values().all(|q| q.items.is_empty() && q.running.is_none())
    }
}

/// Where the consumer starts reading `chain`: past what the store holds, or, for a store that holds nothing of the
/// chain (a new or a rebuilt one), from the committed `replay_from`.
pub fn consumer_start(cursor: HookCursor, recorded: Option<Index>) -> Index {
    match recorded {
        Some(r) => r.max(cursor.read),
        None => Index(cursor.replay_from.0.saturating_sub(1).min(cursor.read.0)),
    }
}

/// The cursor the consumer commits after recording, or `None`. A cursor entry is itself an entry, so moving `read`
/// past each one would chase its own tail: it commits when the batch recorded rows (`HookCursor.read` passes them
/// only so), when `replay_from` moved within what the committed cursor covers, or every `CURSOR_STRIDE` entries
/// (`crate::hooks::CURSOR_STRIDE`). The host also commits at most one per chain per round.
pub fn cursor_to_commit(committed: HookCursor, want: HookCursor, recorded_rows: bool) -> Option<HookCursor> {
    let replay_moved = want.replay_from.min(Index(committed.read.0 + 1)) != committed.replay_from;
    let stride = want.read.0 >= committed.read.0 + crate::hooks::CURSOR_STRIDE;
    (want != committed && (recorded_rows || replay_moved || stride)).then_some(want)
}

/// The `Webhooks` consumer's rows for one committed entry `e` of `repo`'s chain: one per (webhook, hook that takes
/// it), with the GUID `delivery_guid` derives from (`generation`, chain, index, slot, hook), so a replay derives the
/// same ones. A ping goes to its own hook only. `core` is the fold the consumer reads with the entry, whose hooks get
/// the rows; `render` is the payload renderer (`graftyard_api::webhooks::payload`), called once per webhook some hook
/// takes, and a webhook it renders as `None` is skipped.
pub fn plan(
    core: &dyn graftyard_fold::Read,
    repo: RepoId,
    generation: u64,
    e: &graftyard_raft::LogEntry<crate::repo::RepoEvent>,
    mut render: impl FnMut(&crate::hooks::Webhook) -> Option<Vec<u8>>,
) -> Vec<NewDelivery> {
    let chain = ChainId::Repo(repo);
    // A hook gets the entries from its own creation on: the consumer reads behind the log, so the fold may already
    // hold a hook an older entry never saw [inference: GitHub sends an event to the hooks it had then].
    let hooks: Vec<(HookId, HookRecord)> = crate::hooks::hooks(core)
        .into_iter()
        .filter(|(id, _)| crate::hooks::hook_row(core, *id).is_some_and(|r| r.created <= e.at))
        .collect();
    let mut out = Vec::new();
    for (slot, w) in crate::hooks::webhooks(core, repo, e).iter().enumerate() {
        let slot = u16::try_from(slot).unwrap_or(u16::MAX);
        let to: Vec<HookId> = hooks
            .iter()
            .filter(|(id, r)| match w.target {
                Some(t) => t == *id && r.active,
                None => r.takes(w.event),
            })
            .map(|(id, _)| *id)
            .collect();
        if to.is_empty() {
            continue;
        }
        let Some(body) = render(w) else { continue };
        for hook in to {
            out.push(NewDelivery {
                guid: crate::hooks::delivery_guid(generation, chain, e.index, slot, hook),
                hook,
                entry: Some((chain, e.index, slot)),
                event: w.event,
                action: w.action,
                repo: Some(repo),
                body: body.clone(),
            });
        }
    }
    out
}

/// The deadline every attempt runs under.
pub const fn deadline_ms() -> u64 {
    ATTEMPT_DEADLINE_MS
}

#[cfg(test)]
mod tests {
    use graftyard_identity::{EventKind, Url};
    use graftyard_types::DbId;

    use super::*;
    use crate::hooks::{HookContentType, delivery_guid};

    const CHAIN: ChainId = ChainId::Repo(RepoId(DbId(7)));

    fn rec(retry: bool) -> HookRecord {
        HookRecord {
            url: Url("https://h.test/".into()),
            events: smallvec::smallvec![EventKind("*".into())],
            content_type: HookContentType::Json,
            active: true,
            insecure_ssl: false,
            secret: None,
            retry,
        }
    }

    fn row(i: u64, hook: i64) -> NewDelivery {
        NewDelivery {
            guid: delivery_guid(1, CHAIN, Index(i), 0, HookId(DbId(hook))),
            hook: HookId(DbId(hook)),
            entry: Some((CHAIN, Index(i), 0)),
            event: "push",
            action: None,
            repo: Some(RepoId(DbId(7))),
            body: b"{}".to_vec(),
        }
    }

    fn seq(hooks: &[(i64, bool)]) -> Sequencer {
        let mut s = Sequencer::new(AccountId(DbId(1)), Timestamp(0));
        s.set_targets(&hooks.iter().map(|(h, r)| (HookId(DbId(*h)), rec(*r))).collect::<Vec<_>>());
        s
    }

    #[test]
    fn one_attempt_per_queue_in_order_and_record_is_idempotent() {
        let mut s = seq(&[(1, false)]);
        s.record(CHAIN, Index(3), vec![row(1, 1), row(2, 1), row(3, 1)], Timestamp(0));
        s.record(CHAIN, Index(3), vec![row(1, 1), row(2, 1)], Timestamp(0));
        assert_eq!(s.len(), 3);
        let a = s.start(Timestamp(1));
        assert_eq!(a.len(), 1);
        assert!(s.start(Timestamp(2)).is_empty(), "one attempt in flight per (chain, hook)");
        assert_eq!(s.replay_from(CHAIN), Index(1));
        s.report(a[0].attempt, Timestamp(5), Some(200), false, None);
        assert_eq!(s.replay_from(CHAIN), Index(2));
        let b = s.start(Timestamp(6));
        assert_eq!(s.delivery(&b[0].guid).unwrap().entry.unwrap().1, Index(2));
        // A late report of an attempt already re-sent is ignored.
        let c = s.start(Timestamp(6 + RESEND_AFTER_MS));
        assert_eq!(c[0].guid, b[0].guid, "re-sent under its GUID");
        s.report(b[0].attempt, Timestamp(7 + RESEND_AFTER_MS), Some(200), false, None);
        assert_eq!(s.replay_from(CHAIN), Index(2));
        s.report(c[0].attempt, Timestamp(8 + RESEND_AFTER_MS), Some(500), false, None);
        assert_eq!(s.replay_from(CHAIN), Index(3), "a default hook gets one attempt, failed or not");
    }

    #[test]
    fn a_retry_hook_holds_its_queue_and_retries_under_the_guid() {
        let mut s = seq(&[(1, true)]);
        s.record(CHAIN, Index(2), vec![row(1, 1), row(2, 1)], Timestamp(0));
        let a = s.start(Timestamp(0));
        s.report(a[0].attempt, Timestamp(10), Some(500), false, None);
        assert!(s.start(Timestamp(11)).is_empty(), "backoff");
        let b = s.start(Timestamp(10 + RETRY_FIRST_MS));
        assert_eq!(b[0].guid, a[0].guid);
        s.report(b[0].attempt, Timestamp(20 + RETRY_FIRST_MS), Some(204), false, None);
        let d = s.delivery(&a[0].guid).unwrap();
        assert_eq!(d.attempts.len(), 2);
        assert!(d.done);
    }

    #[test]
    fn round_robin_resumes_after_a_queue_that_emptied() {
        // Hook 1's queue empties and is dropped while it was the last served; hook 2's must still be served.
        let mut s = seq(&[(1, false), (2, false)]);
        s.record(CHAIN, Index(1), vec![row(1, 1)], Timestamp(0));
        let a = s.start(Timestamp(0));
        s.report(a[0].attempt, Timestamp(1), Some(200), false, None);
        s.start(Timestamp(1));
        s.record(CHAIN, Index(2), vec![row(2, 2), row(2, 1)], Timestamp(2));
        let b = s.start(Timestamp(2));
        assert_eq!(b.len(), 2, "both queues start: {b:?}");
    }

    #[test]
    fn failing_hooks_share_at_most_half_the_slots() {
        let hooks: Vec<(i64, bool)> = (1..=80).map(|h| (h, false)).collect();
        let mut s = seq(&hooks);
        let rows: Vec<NewDelivery> = (1..=80).flat_map(|h| [row(1, h), row(2, h)]).collect();
        s.record(CHAIN, Index(2), rows, Timestamp(0));
        // Hooks 1 to 60 fail their first attempt.
        let mut now = 0;
        loop {
            let st = s.start(Timestamp(now));
            if st.is_empty() {
                break;
            }
            for a in st {
                let bad = a.hook.0.0 <= 60;
                s.report(a.attempt, Timestamp(now), (!bad).then_some(200), bad, None);
            }
            now += 1;
            if s.failing.len() == 60 {
                break;
            }
        }
        let st = s.start(Timestamp(now + 1));
        let failing = st.iter().filter(|a| a.hook.0.0 <= 60).count();
        assert!(failing <= 25, "{failing} failing attempts at once");
        assert!(st.len() <= 50);
    }

    #[test]
    fn a_rebuilt_store_replays_from_replay_from_with_the_same_guids() {
        let mut s = seq(&[(1, false), (2, false)]);
        s.record(CHAIN, Index(4), (1..=4).flat_map(|i| [row(i, 1), row(i, 2)]).collect(), Timestamp(0));
        for a in s.start(Timestamp(0)) {
            s.report(a.attempt, Timestamp(1), Some(200), false, None);
        }
        let cur = s.cursor(CHAIN);
        assert_eq!(cur, HookCursor { read: Index(4), replay_from: Index(2) });
        let mut fresh = seq(&[(1, false), (2, false)]);
        assert_eq!(consumer_start(cur, fresh.recorded(CHAIN)), Index(1));
        fresh.record(CHAIN, Index(4), (2..=4).flat_map(|i| [row(i, 1), row(i, 2)]).collect(), Timestamp(2));
        let g: BTreeSet<Guid> = fresh.deliveries.keys().copied().collect();
        assert!(g.iter().all(|x| s.deliveries.contains_key(x)));
        assert_eq!(consumer_start(cur, Some(Index(4))), Index(4));
    }

    #[test]
    fn manual_redelivery_lists_and_expires() {
        let mut s = seq(&[(1, false)]);
        s.record(CHAIN, Index(1), vec![row(1, 1)], Timestamp(0));
        let a = s.start(Timestamp(0)).remove(0);
        s.report(a.attempt, Timestamp(3), Some(500), false, Some("no"));
        let g = s.redeliver(HookId(DbId(1)), a.attempt, Timestamp(4)).unwrap();
        assert_eq!(g, a.guid);
        let b = s.start(Timestamp(5)).remove(0);
        assert!(b.redelivery && b.guid == a.guid);
        s.report(b.attempt, Timestamp(6), Some(200), false, None);
        let l = s.list(HookId(DbId(1)), None, 30);
        assert_eq!(l.iter().map(|(_, x)| x.id).collect::<Vec<_>>(), vec![b.attempt, a.attempt]);
        assert!(l[0].1.redelivery && !l[1].1.redelivery);
        assert!(s.list(HookId(DbId(2)), None, 30).is_empty());
        assert_eq!(s.redeliver(HookId(DbId(2)), a.attempt, Timestamp(7)), Err(RedeliverErr::NotFound));
        let late = Timestamp(6 + DELIVERY_KEEP_MS);
        assert_eq!(s.redeliver(HookId(DbId(1)), a.attempt, late), Err(RedeliverErr::Expired));
        s.prune(late);
        assert!(s.is_empty());
    }
}
