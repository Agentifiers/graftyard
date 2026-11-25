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

/// What the consumer records: one delivery, before any attempt. It crosses from the repository leader to the owner's
/// `HookSequencer` in the raft codec.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewDelivery {
    pub guid: Guid,
    pub hook: HookId,
    pub entry: Option<(ChainId, Index, u16)>,
    pub event: Box<str>,
    pub action: Option<Box<str>>,
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

/// One owner's store. The object keeps it in its SQLite as two parts: this struct's small fields (targets, round
/// robin, failing hooks, `recorded`, the next attempt id), one row written whenever it changes, and one row per
/// delivery, written when `take_dirty` names it. The queues and `pending` are rebuilt from the deliveries not yet done
/// (`restore`), in chain order; an attempt running when the object reset is started again under its GUID, and a
/// retry hook's backoff starts over [inference: at-least-once, as a delivery Worker lost mid-attempt].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sequencer {
    pub owner: AccountId,
    targets: BTreeMap<HookId, HookRecord>,
    #[serde(skip)]
    deliveries: BTreeMap<Guid, HookDelivery>,
    #[serde(skip)]
    by_attempt: BTreeMap<u64, Guid>,
    #[serde(skip)]
    queues: BTreeMap<QueueKey, Queue>,
    /// The queue `start` served last, so the next round starts after it.
    last: Option<QueueKey>,
    failing: BTreeSet<HookId>,
    recorded: BTreeMap<ChainId, Index>,
    /// Each chain's hooks as its consumer last listed them, so a hook its chain deleted loses its target.
    chain_hooks: BTreeMap<ChainId, BTreeSet<HookId>>,
    /// Unfinished deliveries per (chain, index), for `replay_from`.
    #[serde(skip)]
    pending: BTreeMap<(ChainId, Index), u32>,
    next_attempt: u64,
    /// Deliveries added, changed, or dropped since the last `take_dirty`.
    #[serde(skip)]
    dirty: BTreeSet<Guid>,
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
            chain_hooks: BTreeMap::new(),
            pending: BTreeMap::new(),
            next_attempt: now.0.saturating_mul(1_000).max(1),
            dirty: BTreeSet::new(),
        }
    }

    /// A store read back from its rows: the small part and every delivery row. Each delivery not done goes back on
    /// its queue in chain order, (index, slot); one with no chain entry (a ping) on its hook's own queue in the order
    /// of its GUID. A manual redelivery not yet started when the object reset is lost.
    pub fn restore(mut core: Sequencer, deliveries: impl IntoIterator<Item = HookDelivery>) -> Sequencer {
        let mut undone: Vec<(QueueKey, (u64, u16), Guid)> = Vec::new();
        for d in deliveries {
            for a in &d.attempts {
                core.by_attempt.insert(a.id, d.guid);
            }
            if !d.done {
                let (chain, at) = match d.entry {
                    Some((c, i, slot)) => {
                        *core.pending.entry((c, i)).or_default() += 1;
                        (Some(c), (i.0, slot))
                    }
                    None => (None, (0, 0)),
                };
                undone.push((QueueKey { chain, hook: d.hook }, at, d.guid));
            }
            core.deliveries.insert(d.guid, d);
        }
        undone.sort();
        for (k, _, g) in undone {
            core.queues.entry(k).or_default().items.push_back((g, false));
        }
        core.next_attempt = core.next_attempt.max(core.by_attempt.keys().next_back().map_or(0, |a| a + 1));
        core.dirty.clear();
        core
    }

    /// The deliveries to write since the last call: `Some` to upsert, `None` to delete.
    pub fn take_dirty(&mut self) -> Vec<(Guid, Option<&HookDelivery>)> {
        let dirty = std::mem::take(&mut self.dirty);
        dirty.into_iter().map(|g| (g, self.deliveries.get(&g))).collect()
    }

    /// The hooks' current records, as the consumer read them with the batch. An attempt goes to the hook's current
    /// URL with its current secret; a hook no longer listed, or inactive, gets no further attempt.
    pub fn set_targets(&mut self, chain_hooks: &[(HookId, HookRecord)]) {
        for (id, r) in chain_hooks {
            self.targets.insert(*id, r.clone());
        }
    }

    /// `chain`'s hooks as its consumer read them with a batch: their current records, and a hook the chain listed
    /// before and no longer does (deleted) loses its target, so its queued deliveries end with no attempt. An App's
    /// webhook is listed by every chain its installation reaches, so it keeps its target while another chain lists it.
    pub fn set_chain_targets(&mut self, chain: ChainId, hooks: &[(HookId, HookRecord)]) {
        let now: BTreeSet<HookId> = hooks.iter().map(|(id, _)| *id).collect();
        let others = |h: &HookId| self.chain_hooks.iter().any(|(c, hs)| *c != chain && hs.contains(h));
        let gone: Vec<HookId> = self
            .chain_hooks
            .get(&chain)
            .into_iter()
            .flatten()
            .filter(|h| !now.contains(h) && !others(h))
            .copied()
            .collect();
        for g in gone {
            self.targets.remove(&g);
        }
        self.set_targets(hooks);
        self.chain_hooks.insert(chain, now);
    }

    /// The consumer moved `chain` to another store: its queued deliveries are deleted here, and an attempt running
    /// now is the last. What it recorded is forgotten, so a later batch fills it again from `replay_from`; a deleted
    /// delivery is then recorded again under its GUID, where one only marked finished would read as held and never
    /// be attempted.
    pub fn drop_chain(&mut self, chain: ChainId) {
        let keys: Vec<QueueKey> = self.queues.keys().filter(|k| k.chain == Some(chain)).copied().collect();
        for k in keys {
            let q = self.queues.get_mut(&k).expect("listed");
            let keep = q.running.and(q.items.pop_front());
            let gone: Vec<Guid> = q.items.drain(..).map(|(g, _)| g).collect();
            q.items.extend(keep);
            for g in gone {
                self.finish(g);
                self.deliveries.remove(&g);
                self.by_attempt.retain(|_, a| *a != g);
            }
        }
        self.recorded.remove(&chain);
        self.chain_hooks.remove(&chain);
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
        self.dirty.insert(r.guid);
        self.deliveries.insert(
            r.guid,
            HookDelivery {
                guid: r.guid,
                hook: r.hook,
                entry: r.entry,
                event: r.event,
                action: r.action,
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

    /// After `prev`'s report, its queue's next attempt, for whoever ran `prev` to run at once, so a busy queue does not
    /// wait for the next `start` round. `None` unless a slot is free with one left for every other queue waiting to
    /// start (and, for a failing hook, within its half for every failing one waiting), so `start` still reaches them.
    pub fn next_in_queue(&mut self, prev: &Start, now: Timestamp) -> Option<Start> {
        let d = self.deliveries.get(&prev.guid)?;
        let key = if prev.redelivery {
            QueueKey { chain: None, hook: d.hook }
        } else {
            QueueKey { chain: d.entry.map(|(c, _, _)| c), hook: d.hook }
        };
        let cap = usize::from(HOOK_OWNER_CONCURRENCY);
        let (total, failing) = self.in_flight();
        let is_failing = self.failing.contains(&key.hook);
        let (mut waiting, mut waiting_failing) = (0, 0);
        for (k, q) in &self.queues {
            if *k != key && q.running.is_none() && !q.items.is_empty() && now >= q.not_before {
                waiting += 1;
                waiting_failing += usize::from(self.failing.contains(&k.hook));
            }
        }
        if total + waiting >= cap || (is_failing && failing + waiting_failing >= cap / 2) {
            return None;
        }
        if !self.targets.get(&key.hook).is_some_and(|t| t.active) {
            return None;
        }
        let q = self.queues.get_mut(&key)?;
        if q.running.is_some() || now < q.not_before {
            return None;
        }
        let &(guid, manual) = q.items.front()?;
        let attempt = self.next_attempt;
        self.next_attempt += 1;
        q.running = Some(Running { attempt, started: now });
        self.by_attempt.insert(attempt, guid);
        Some(Start { guid, hook: key.hook, attempt, redelivery: manual })
    }

    /// The deliveries queued behind `s` on its queue, at most `max`, none a manual redelivery: what a host may send
    /// after `s` in the same run, in order, while the queue's one slot is `s`'s. Empty unless `s` is the queue's
    /// running attempt.
    pub fn queued_after(&self, s: &Start, max: usize) -> Vec<Guid> {
        let Some(key) = self.chain_queue(s) else { return Vec::new() };
        let Some(q) = self.queues.get(&key) else { return Vec::new() };
        if q.running.map(|r| r.attempt) != Some(s.attempt) {
            return Vec::new();
        }
        q.items.iter().skip(1).take_while(|(_, manual)| !manual).take(max).map(|(g, _)| *g).collect()
    }

    /// After `prev`'s report, `guid` as the attempt the host already sent at `at` in the same run as `prev`
    /// (`queued_after`). It takes `prev`'s slot, not a new one. `None` unless `guid` is now the queue's front and the
    /// queue may run it (nothing running, no backoff, the hook still active); the host then leaves it to be sent
    /// again under its GUID, which at-least-once allows.
    pub fn next_in_run(&mut self, prev: &Start, guid: Guid, at: Timestamp) -> Option<Start> {
        let key = self.chain_queue(prev)?;
        if !self.targets.get(&key.hook).is_some_and(|t| t.active) {
            return None;
        }
        let q = self.queues.get_mut(&key)?;
        if q.running.is_some() || at < q.not_before || q.items.front() != Some(&(guid, false)) {
            return None;
        }
        let attempt = self.next_attempt;
        self.next_attempt += 1;
        q.running = Some(Running { attempt, started: at });
        self.by_attempt.insert(attempt, guid);
        Some(Start { guid, hook: key.hook, attempt, redelivery: false })
    }

    /// The queue an automatic attempt `s` runs on; `None` for a manual redelivery, which runs alone.
    fn chain_queue(&self, s: &Start) -> Option<QueueKey> {
        let d = self.deliveries.get(&s.guid).filter(|_| !s.redelivery)?;
        Some(QueueKey { chain: d.entry.map(|(c, _, _)| c), hook: d.hook })
    }

    fn finish(&mut self, guid: Guid) {
        let Some(d) = self.deliveries.get_mut(&guid) else { return };
        if d.done {
            return;
        }
        d.done = true;
        self.dirty.insert(guid);
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
        self.dirty.insert(guid);
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

    /// Takes a delivery's body out, leaving it empty, for a host that stores bodies apart from the rows and keeps
    /// only unfinished deliveries' bodies in memory; `None` for a delivery not held (pruned or dropped).
    pub fn take_body(&mut self, guid: &Guid) -> Option<Box<[u8]>> {
        self.deliveries.get_mut(guid).map(|d| std::mem::take(&mut d.body))
    }

    /// Puts back a body `take_body` took, or one the host read from its own storage.
    pub fn set_body(&mut self, guid: &Guid, body: Box<[u8]>) {
        if let Some(d) = self.deliveries.get_mut(guid) {
            d.body = body;
        }
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
            self.dirty.insert(g);
        }
        let live = &self.deliveries;
        self.by_attempt.retain(|_, g| live.contains_key(g));
    }

    /// Every delivery held, done or not.
    pub fn deliveries(&self) -> impl Iterator<Item = &HookDelivery> {
        self.deliveries.values()
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

/// Where the consumer starts reading `chain`: past what the store holds; for a store that holds nothing of the chain
/// (a new or a rebuilt one), from the committed `replay_from`; and for one that holds it only to before the committed
/// `read` (the consumer recorded the rest in another site's store and came back), past what it holds or from
/// `replay_from`, whichever is later, since every entry before `replay_from` is finished. Starting from `replay_from`
/// every time instead never caught up once a batch was cut short of `read`: graft-m4a's j4 then moved one entry per
/// slow-hook attempt (BF-57 run 4). The consumer sends a batch as a replay when it starts past what the store holds.
pub fn consumer_start(cursor: HookCursor, recorded: Option<Index>) -> Index {
    let from_replay = Index(cursor.replay_from.0.saturating_sub(1).min(cursor.read.0));
    match recorded {
        Some(r) if r >= cursor.read => r,
        Some(r) => r.max(from_replay),
        None => from_replay,
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

/// An App installed on the repository's owner, as a delivery target (SHARED, "Event fan-out", App deliveries): its
/// installation, the installation's permissions, and its webhook as a hook record keyed `HookId::app`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppTarget {
    pub installation: graftyard_identity::InstallationId,
    pub permissions: graftyard_identity::PermissionSet,
    pub hook: HookId,
    pub record: HookRecord,
}

/// The App targets of `repo`: each live installation on its owner (`installations`, the owner's `Authz` rows) whose
/// repository selection reaches it and whose App's record (`app`, read from the App owner's `Authz`) has a webhook
/// and subscribes to an event. `webhook_secret` holds the raft-codec bytes of a `Wrapped`; a secret that does not
/// decode leaves the App out, so no attempt goes unsigned.
pub fn app_targets<'a>(
    installations: &std::collections::BTreeMap<
        graftyard_identity::InstallationId,
        graftyard_identity::InstallationState,
    >,
    repo: RepoId,
    app: impl Fn(graftyard_identity::AppId) -> Option<&'a graftyard_identity::AppRecord>,
) -> Vec<AppTarget> {
    let mut out = Vec::new();
    for (id, st) in installations {
        let Some(row) = &st.row else { continue };
        let reaches = match &row.repos {
            graftyard_identity::RepoScope::All => true,
            graftyard_identity::RepoScope::Selected(r) => r.contains(&repo),
        };
        let Some(rec) = app(row.app).filter(|r| reaches && !r.events.is_empty()) else { continue };
        let Some(url) = rec.webhook.clone() else { continue };
        let secret = match &rec.webhook_secret {
            Some(b) => match graftyard_raft::codec::from_bytes::<graftyard_identity::sealed::Wrapped>(b) {
                Ok(w) => Some(w),
                Err(_) => continue,
            },
            None => None,
        };
        let record = HookRecord {
            url,
            events: rec.events.clone(),
            content_type: crate::hooks::HookContentType::Json,
            active: true,
            insecure_ssl: false,
            secret,
            retry: false,
        };
        out.push(AppTarget {
            installation: *id,
            permissions: row.permissions.clone(),
            hook: HookId::app(row.app),
            record,
        });
    }
    out
}

/// The `Webhooks` consumer's rows for one committed entry `e` of `repo`'s chain: one per (webhook, hook that takes
/// it), with the GUID `delivery_guid` derives from (`generation`, chain, index, slot, hook), so a replay derives the
/// same ones. A ping goes to its own hook only. `core` is the fold the consumer reads with the entry, whose hooks get
/// the rows; `apps` are the App targets, each of which gets a webhook it subscribes to when its installation has
/// read on the event's permission (`crate::hooks::app_permission`). `render` is the payload renderer
/// (`graftyard_api::webhooks::payload`), called once per webhook some hook takes with `None`, and once per App that
/// takes it with its installation; a webhook it renders as `None` is skipped.
pub fn plan(
    core: &dyn graftyard_fold::Read,
    repo: RepoId,
    generation: u64,
    e: &graftyard_raft::LogEntry<crate::repo::RepoEvent>,
    apps: &[AppTarget],
    mut render: impl FnMut(&crate::hooks::Webhook, Option<graftyard_identity::InstallationId>) -> Option<Vec<u8>>,
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
        let row = |hook, body| NewDelivery {
            guid: crate::hooks::delivery_guid(generation, chain, e.index, slot, hook),
            hook,
            entry: Some((chain, e.index, slot)),
            event: w.event.into(),
            action: w.action.map(Into::into),
            repo: Some(repo),
            body,
        };
        if !to.is_empty()
            && let Some(body) = render(w, None)
        {
            out.extend(to.into_iter().map(|hook| row(hook, body.clone())));
        }
        // A delivery aimed at an App's own hook (the first-party `graftyard_task`) goes to that App alone, and to no
        // repository hook, since no repository hook has a negative id.
        if let Some(t) = w.target.filter(|t| t.as_app().is_some()) {
            for a in apps.iter().filter(|a| a.hook == t) {
                out.extend(render(w, Some(a.installation)).map(|body| row(a.hook, body)));
            }
            continue;
        }
        let Some(need) = crate::hooks::app_permission(w.event).filter(|_| w.target.is_none()) else { continue };
        for a in apps {
            let read = a.permissions.0.get(need).is_some_and(|x| *x >= graftyard_identity::Access::Read);
            if read
                && a.record.takes(w.event)
                && let Some(body) = render(w, Some(a.installation))
            {
                out.push(row(a.hook, body));
            }
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
            event: "push".into(),
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
    fn a_run_sends_a_queues_waiting_deliveries_in_order_under_one_slot() {
        let mut s = seq(&[(1, false), (2, false)]);
        s.record(CHAIN, Index(4), vec![row(1, 1), row(2, 1), row(3, 1), row(4, 1), row(1, 2)], Timestamp(0));
        let a = s.start(Timestamp(0)).into_iter().find(|x| x.hook == HookId(DbId(1))).unwrap();
        let after = s.queued_after(&a, 2);
        let idx = |s: &Sequencer, g: &Guid| s.delivery(g).unwrap().entry.unwrap().1;
        assert_eq!(
            after.iter().map(|g| idx(&s, g)).collect::<Vec<_>>(),
            [Index(2), Index(3)],
            "behind the head, at most 2"
        );
        // Not before its predecessor is reported: the queue's slot is still `a`'s.
        assert!(s.next_in_run(&a, after[0], Timestamp(1)).is_none());
        s.report(a.attempt, Timestamp(1), Some(200), false, None);
        // Only the queue's front, in order.
        assert!(s.next_in_run(&a, after[1], Timestamp(1)).is_none(), "entry 3 is not the front");
        let b = s.next_in_run(&a, after[0], Timestamp(1)).expect("entry 2 is the front");
        assert_eq!((b.guid, b.hook), (after[0], HookId(DbId(1))));
        assert!(s.queued_after(&a, 2).is_empty(), "a reported attempt has no run behind it");
        assert_eq!(s.in_flight().0, 2, "the run keeps hook 1's one slot; hook 2 has the other");
        s.report(b.attempt, Timestamp(2), Some(200), false, None);
        assert!(s.delivery(&after[0]).unwrap().done);
        // A deleted hook's queue runs nothing more.
        s.set_chain_targets(CHAIN, &[(HookId(DbId(1)), rec(false)), (HookId(DbId(2)), rec(false))]);
        s.set_chain_targets(CHAIN, &[(HookId(DbId(2)), rec(false))]);
        assert!(s.next_in_run(&b, after[1], Timestamp(3)).is_none());
    }

    #[test]
    fn a_report_starts_its_queues_next_attempt_only_with_a_slot_left_for_each_waiting_queue() {
        let mut s = seq(&[(1, false), (2, false)]);
        s.record(CHAIN, Index(3), vec![row(1, 1), row(2, 1), row(3, 1)], Timestamp(0));
        let a = s.start(Timestamp(0)).remove(0);
        s.report(a.attempt, Timestamp(1), Some(200), false, None);
        let b = s.next_in_queue(&a, Timestamp(1)).expect("nothing else waits");
        assert_eq!(s.delivery(&b.guid).unwrap().entry.unwrap().1, Index(2), "chain order");
        assert!(s.next_in_queue(&a, Timestamp(1)).is_none(), "the queue already runs one");
        // 51 healthy queues and 50 slots: the one `start` left waiting keeps its slot.
        let mut h = seq(&[]);
        let hooks: Vec<_> = (1..=51).map(|h| (HookId(DbId(h)), rec(false))).collect();
        h.set_targets(&hooks);
        h.record(CHAIN, Index(2), (1..=51).flat_map(|k| [row(1, k), row(2, k)]).collect(), Timestamp(0));
        let first = h.start(Timestamp(0));
        assert_eq!(first.len(), usize::from(HOOK_OWNER_CONCURRENCY));
        h.report(first[0].attempt, Timestamp(1), Some(200), false, None);
        assert!(h.next_in_queue(&first[0], Timestamp(1)).is_none(), "the 51st queue waits for this slot");
        assert_eq!(h.start(Timestamp(1)).len(), 1);
        // A failing hook's chained start stays within its half of the slots.
        let mut f = seq(&[]);
        let hooks: Vec<_> = (1..=26).map(|h| (HookId(DbId(h)), rec(false))).collect();
        f.set_targets(&hooks);
        f.record(CHAIN, Index(3), (1..=26).flat_map(|h| [row(1, h), row(2, h), row(3, h)]).collect(), Timestamp(0));
        for x in f.start(Timestamp(0)) {
            f.report(x.attempt, Timestamp(1), Some(500), false, None);
        }
        let again = f.start(Timestamp(2));
        assert_eq!(again.len(), usize::from(HOOK_OWNER_CONCURRENCY) / 2, "26 failing hooks, 25 slots");
        let last = again[0].clone();
        f.report(last.attempt, Timestamp(3), Some(500), false, None);
        let next = f.next_in_queue(&last, Timestamp(3));
        assert!(next.is_none(), "the waiting 26th failing queue is not passed over: {next:?}");
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
    fn round_robin_serves_every_queue_before_any_twice() {
        // 60 hooks with a backlog each and 50 slots: the round after hooks 1 to 50 starts with 51 to 60, not 1 again.
        let hooks: Vec<(i64, bool)> = (1..=60).map(|h| (h, false)).collect();
        let mut s = seq(&hooks);
        s.record(CHAIN, Index(3), (1..=3).flat_map(|i| (1..=60).map(move |h| row(i, h))).collect(), Timestamp(0));
        let first = s.start(Timestamp(0));
        assert_eq!(first.iter().map(|a| a.hook.0.0).max(), Some(50));
        for a in &first {
            s.report(a.attempt, Timestamp(1), Some(200), false, None);
        }
        let second: BTreeSet<i64> = s.start(Timestamp(1)).iter().map(|a| a.hook.0.0).collect();
        assert!((51..=60).all(|h| second.contains(&h)), "hooks 51 to 60 waited a whole round: {second:?}");
    }

    #[test]
    fn a_rebuilt_store_never_hands_out_an_attempt_id_the_lost_one_did() {
        let mut lost = seq(&[(1, false)]);
        lost.record(CHAIN, Index(1), vec![row(1, 1)], Timestamp(1_000));
        let a = lost.start(Timestamp(1_000)).remove(0);
        lost.report(a.attempt, Timestamp(1_100), Some(500), false, None);
        // Rebuilt a minute later: a late report or redelivery naming the old attempt finds nothing.
        let mut fresh = Sequencer::new(AccountId(DbId(1)), Timestamp(61_000));
        fresh.set_targets(&[(HookId(DbId(1)), rec(false))]);
        fresh.record(CHAIN, Index(1), vec![row(1, 1)], Timestamp(61_000));
        let b = fresh.start(Timestamp(61_000)).remove(0);
        assert!(b.attempt > a.attempt, "{} after {}", b.attempt, a.attempt);
        assert_eq!(fresh.redeliver(HookId(DbId(1)), a.attempt, Timestamp(61_001)), Err(RedeliverErr::NotFound));
    }

    #[test]
    fn a_redelivery_names_its_own_hook() {
        let mut s = seq(&[(1, false), (2, false)]);
        s.record(CHAIN, Index(1), vec![row(1, 1)], Timestamp(0));
        let a = s.start(Timestamp(0)).remove(0);
        s.report(a.attempt, Timestamp(1), Some(500), false, None);
        assert_eq!(s.redeliver(HookId(DbId(2)), a.attempt, Timestamp(2)), Err(RedeliverErr::NotFound));
        assert!(s.start(Timestamp(3)).is_empty(), "nothing was queued for hook 2");
    }

    #[test]
    fn at_most_hook_owner_concurrency_attempts_at_once() {
        let hooks: Vec<(i64, bool)> = (1..=80).map(|h| (h, false)).collect();
        let mut s = seq(&hooks);
        s.record(CHAIN, Index(1), (1..=80).map(|h| row(1, h)).collect(), Timestamp(0));
        assert_eq!(s.start(Timestamp(0)).len(), usize::from(HOOK_OWNER_CONCURRENCY));
        assert!(s.start(Timestamp(1)).is_empty(), "every slot is taken");
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
        // A store the consumer comes back to holds the chain to 50 while the committed cursor reads 100 with entry 10
        // the oldest unfinished: it continues past 50, not from 9 again, and one that holds less than 9 starts at 9.
        let back = HookCursor { read: Index(100), replay_from: Index(10) };
        assert_eq!(consumer_start(back, Some(Index(50))), Index(50));
        assert_eq!(consumer_start(back, Some(Index(5))), Index(9));
        assert_eq!(consumer_start(back, Some(Index(120))), Index(120));
    }

    #[test]
    fn a_store_written_through_take_dirty_reads_back_and_resends_what_ran() {
        // The object writes the small part and the dirty delivery rows after each change; a reset reads them back.
        // Queues come back in chain order, the attempt that was running starts again under its GUID with a new id,
        // its old id's late report changes nothing, and pruned rows are deleted.
        let mut s = seq(&[(1, false), (2, true)]);
        let mut rows: BTreeMap<Guid, HookDelivery> = BTreeMap::new();
        let write = |s: &mut Sequencer, rows: &mut BTreeMap<Guid, HookDelivery>| {
            for (g, d) in s.take_dirty() {
                match d {
                    Some(d) => rows.insert(g, d.clone()),
                    None => rows.remove(&g),
                };
            }
            postcard::to_allocvec(&*s).unwrap()
        };
        // Recorded out of chain order within the batch, so a rebuild that kept insertion order would differ.
        s.record(CHAIN, Index(3), vec![row(3, 1), row(1, 1), row(2, 1), row(1, 2)], Timestamp(0));
        let a = s.start(Timestamp(0));
        let first = a.iter().find(|x| x.hook == HookId(DbId(1))).unwrap().clone();
        let running = a.iter().find(|x| x.hook == HookId(DbId(2))).unwrap().clone();
        s.report(first.attempt, Timestamp(1), Some(200), false, None);
        let core = write(&mut s, &mut rows);
        let mut back = Sequencer::restore(postcard::from_bytes(&core).unwrap(), rows.values().cloned());
        assert_eq!(back.replay_from(CHAIN), s.replay_from(CHAIN));
        assert_eq!(back.recorded(CHAIN), Some(Index(3)));
        back.report(running.attempt, Timestamp(2), Some(200), false, None);
        let again = back.start(Timestamp(3));
        let by_hook = |h: i64| again.iter().find(|x| x.hook == HookId(DbId(h))).unwrap().clone();
        assert_eq!(by_hook(2).guid, running.guid, "the running attempt is sent again under its GUID");
        assert!(by_hook(2).attempt > running.attempt);
        // Hook 1's queue held 3, 1, 2 in the order recorded, and 3 ran first; the rebuilt queue is 1, then 2.
        assert_eq!(s.delivery(&first.guid).unwrap().entry.unwrap().1, Index(3));
        assert_eq!(back.delivery(&by_hook(1).guid).unwrap().entry.unwrap().1, Index(1));
        back.report(by_hook(1).attempt, Timestamp(4), Some(200), false, None);
        let next = back.start(Timestamp(5));
        assert_eq!(back.delivery(&next[0].guid).unwrap().entry.unwrap().1, Index(2));
        for x in back.start(Timestamp(5)).into_iter().chain(next).chain([by_hook(2)]) {
            back.report(x.attempt, Timestamp(6), Some(200), false, None);
        }
        assert!(back.idle());
        write(&mut back, &mut rows);
        back.prune(Timestamp(10 + DELIVERY_KEEP_MS));
        write(&mut back, &mut rows);
        assert!(rows.is_empty(), "pruned rows are deleted");
    }

    #[test]
    fn a_hook_its_chain_no_longer_lists_gets_no_more_attempts() {
        let mut s = seq(&[]);
        s.set_chain_targets(CHAIN, &[(HookId(DbId(1)), rec(false)), (HookId(DbId(2)), rec(false))]);
        s.record(CHAIN, Index(2), vec![row(1, 1), row(2, 1), row(1, 2)], Timestamp(0));
        let a = s.start(Timestamp(0));
        assert_eq!(a.len(), 2);
        s.set_chain_targets(CHAIN, &[(HookId(DbId(2)), rec(false))]);
        for x in a {
            s.report(x.attempt, Timestamp(1), Some(200), false, None);
        }
        assert!(s.start(Timestamp(2)).is_empty(), "hook 1 was deleted; its entry 2 is dropped");
        assert_eq!(s.replay_from(CHAIN), Index(3));
    }

    #[test]
    fn a_dropped_chain_stops_delivering_and_refills_from_replay_from() {
        let mut s = seq(&[(1, false)]);
        s.record(CHAIN, Index(3), vec![row(1, 1), row(2, 1), row(3, 1)], Timestamp(0));
        let a = s.start(Timestamp(0)).remove(0);
        s.drop_chain(CHAIN);
        assert_eq!(s.recorded(CHAIN), None, "the consumer fills it again from replay_from");
        s.report(a.attempt, Timestamp(1), Some(200), false, None);
        assert!(s.start(Timestamp(2)).is_empty(), "entries 2 and 3 are not attempted here");
        assert!(s.idle());
        // The consumer comes back to this store and replays from `replay_from` under the same GUIDs: the entries it
        // dropped unattempted are attempted now, and the one attempted before is not again.
        s.record(CHAIN, Index(3), vec![row(1, 1), row(2, 1), row(3, 1)], Timestamp(3));
        let b = s.start(Timestamp(3));
        assert_eq!(b.len(), 1);
        assert_eq!(s.delivery(&b[0].guid).unwrap().entry.unwrap().1, Index(2));
        s.report(b[0].attempt, Timestamp(4), Some(200), false, None);
        let c = s.start(Timestamp(5));
        assert_eq!(s.delivery(&c[0].guid).unwrap().entry.unwrap().1, Index(3));
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
