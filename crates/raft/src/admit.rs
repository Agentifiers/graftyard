//! Leader admission inside group commit (`design/DESIGN.md`, Shape, "Agent load and agent safety"): the classes,
//! their constants, what a host passes with a command, and the per-class lanes the leader cuts each entry from.
//!
//! Every entry takes Human and MergeQueue first, then Agent up to AGENT_FLOOR of capacity, then Bot, then Agent while
//! the AGENT_SHARE token bucket holds. Inside each class, commands are fair-queued by actor, round robin. A host that
//! calls `Member::commit` sends Human commands with no actor, which queue in one FIFO lane as before admission existed.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use graftyard_types::Timestamp;

pub const AGENT_SHARE: f32 = 0.5;
/// Of capacity, which Bot cannot take.
pub const AGENT_FLOOR: f32 = 0.2;
/// Write commands per actor per minute on one group before its commands take the Agent class.
pub const DEMOTE_PER_MIN: u32 = 60;
pub const AGENT_WAIT: Duration = Duration::from_secs(30);
/// An agent push is answered within this of its arrival at the edge: committed, or refused `Full` (B0-32's pass line,
/// `build/BUILD-PLAN.md`). The edge sends what is left of it as `Admit::budget`.
pub const AGENT_PUSH_BOUND: Duration = Duration::from_secs(5);
/// Queued agent commands and bytes per group.
pub const AGENT_QUEUE_MAX: (u32, usize) = (1_000, 64 << 20);
/// A hot group inlines only up to this while placement succeeds; `INLINE_MAX` stays 1 MiB.
pub const INLINE_MAX_HOT: usize = 64 << 10;
/// Agent-class inline pack bytes per entry, on every group.
pub const AGENT_INLINE_ENTRY: usize = 128 << 10;
/// Agent-class commands per entry, on every group (`decisions/DECISIONS.md`, "`agent-cmds-entry`"): B0-32 m3fgt1's
/// burst entries of 191 and 160 agent commands took 2.6 and 2.8 s beside ticker stalls of 1.7 and 0.8 s, where entries
/// of 31 to 106 took at most 578 ms on m3fwi3 [measured, `tests/am1/results/lm1-rows-m3f.md`, "Row m3fgt1"; 64 is a
/// guess between them].
pub const AGENT_CMDS_ENTRY: usize = 64;
/// A group above this many pushes a second, over `HOT_WINDOW`, is hot.
pub const HOT_PUSHES: f32 = 2.0;
/// The window `hot` counts pushes over (the M0-WP9 prototype's 5 s; the design gives no window).
pub const HOT_WINDOW: Duration = Duration::from_secs(5);
/// The window DEMOTE_PER_MIN counts over.
pub const DEMOTE_WINDOW: Duration = Duration::from_secs(60);
/// "Pressure" is more agent commands queued than the agent bucket admits in this long, or more agent bytes queued
/// than one entry holds [inference, the M0-WP9 prototype's rule].
pub const PRESSURE_AGE: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdmitClass {
    Human,
    MergeQueue,
    Bot,
    Agent,
}

impl AdmitClass {
    pub const ALL: [AdmitClass; 4] = [AdmitClass::Human, AdmitClass::MergeQueue, AdmitClass::Bot, AdmitClass::Agent];

    pub(crate) fn lane(self) -> usize {
        self as usize
    }
}

/// The App or user a command's writes count for, as the host keys it. Fair queuing and DEMOTE_PER_MIN are per key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActorKey(pub u64);

/// What a host sends beside a command. `class` is the edge's classification: `AgentApp`, agent-prefix or `AgentMeta`
/// causation, a classing trailer, or `X-Graftyard-Agent-Session`. The leader may demote Human or Bot to Agent.
/// `actor` is `None` for commands that do not count toward DEMOTE_PER_MIN (task API commands, host work). `inline` is
/// the pack bytes the command carries inline, 0 for a stored pack or none. `placement_ok` is the edge's view that
/// `PackStore::place` succeeds and every bucket's probe is green. `budget` is what is left of AGENT_PUSH_BOUND when the
/// edge sent it: a command the leader admits as Agent is refused `Full` at once when its expected wait is longer, and
/// once it has waited that long while still queued (`expire_agents`). `None` waits up to AGENT_WAIT, as before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admit {
    pub class: AdmitClass,
    pub actor: Option<ActorKey>,
    pub push: bool,
    pub inline: usize,
    pub placement_ok: bool,
    pub budget: Option<Duration>,
}

impl Admit {
    /// What `Member::commit` sends: Human, no actor, no inline pack. It is never demoted, staged, or refused.
    pub fn human() -> Admit {
        Admit { class: AdmitClass::Human, actor: None, push: false, inline: 0, placement_ok: true, budget: None }
    }
}

/// Why `Member::commit_as` did not queue a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmitError {
    Call(crate::CallError),
    /// Agent admission is full, or the agent pack cannot be staged while placement fails. Git gets HTTP 200 with
    /// `ng <ref> agent admission full, retry after <n>s`, and REST gets 429 with `retry-after: <n>`.
    Full {
        retry_after_s: u32,
    },
    /// Stage the pack in the buckets as `PackRef::Stored` and send the command again under the same request id.
    Stage,
}

/// One committed request as admission reads it from the group's fold: whose write it was, for DEMOTE_PER_MIN, and
/// whether it was a push, for `hot`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteMark {
    pub actor: Option<ActorKey>,
    pub push: bool,
}

/// The last `DEMOTE_WINDOW` of committed writes, by entry time, kept in the group fold so a new leader reads the same
/// counts (`GroupFold::actor_writes`, `GroupFold::pushes`). Derived from committed entries on every member; a member
/// restored from its tables starts it empty [inference: the design keeps no table for it].
#[derive(Clone, Debug, Default)]
pub(crate) struct Recent {
    by_actor: BTreeMap<ActorKey, VecDeque<Timestamp>>,
    pushes: VecDeque<Timestamp>,
    order: VecDeque<(Timestamp, Option<ActorKey>)>,
    /// The last entry noted, so an entry is noted once whether it arrives by `apply` or by `Member::restore`.
    pub(crate) through: graftyard_types::Index,
}

impl Recent {
    pub(crate) fn note(&mut self, at: Timestamp, m: WriteMark) {
        if let Some(a) = m.actor {
            self.by_actor.entry(a).or_default().push_back(at);
        }
        if m.push {
            self.pushes.push_back(at);
        }
        self.order.push_back((at, m.actor));
    }

    /// Drops writes before `cut`.
    pub(crate) fn expire(&mut self, cut: Timestamp) {
        while let Some(&(at, a)) = self.order.front() {
            if at >= cut {
                break;
            }
            self.order.pop_front();
            if let Some(a) = a
                && let Some(q) = self.by_actor.get_mut(&a)
            {
                q.pop_front();
                if q.is_empty() {
                    self.by_actor.remove(&a);
                }
            }
        }
        while self.pushes.front().is_some_and(|t| *t < cut) {
            self.pushes.pop_front();
        }
    }

    pub(crate) fn actor_writes(&self, a: ActorKey, from: Timestamp, before: Timestamp) -> u32 {
        self.by_actor.get(&a).map_or(0, |q| between(q, from, before))
    }

    pub(crate) fn pushes(&self, from: Timestamp, before: Timestamp) -> u32 {
        between(&self.pushes, from, before)
    }
}

/// Times in `[from, before)` of a queue in entry order. Entry times rise along the log, since a leader stamps its
/// own clock and a new leader starts after the last commit it read [inference: across leaders the clocks are
/// different hosts', so this counts approximately there].
fn between(q: &VecDeque<Timestamp>, from: Timestamp, before: Timestamp) -> u32 {
    (q.partition_point(|t| *t < before) - q.partition_point(|t| *t < from)) as u32
}

/// One class's queue: a FIFO per actor, served round robin. With one actor it is a FIFO.
pub(crate) struct Lane<T> {
    queues: BTreeMap<Option<ActorKey>, VecDeque<T>>,
    order: VecDeque<Option<ActorKey>>,
    len: usize,
}

impl<T> Default for Lane<T> {
    fn default() -> Self {
        Lane { queues: BTreeMap::new(), order: VecDeque::new(), len: 0 }
    }
}

impl<T> Lane<T> {
    pub(crate) fn push(&mut self, actor: Option<ActorKey>, t: T) {
        let q = self.queues.entry(actor).or_default();
        if q.is_empty() {
            self.order.push_back(actor);
        }
        q.push_back(t);
        self.len += 1;
    }

    pub(crate) fn front(&self) -> Option<&T> {
        self.order.front().and_then(|a| self.queues.get(a)).and_then(VecDeque::front)
    }

    /// The front of the next actor's queue; that actor goes to the back of the order if it has more.
    pub(crate) fn pop(&mut self) -> Option<T> {
        let a = self.order.pop_front()?;
        let q = self.queues.get_mut(&a).expect("ordered actors have a queue");
        let t = q.pop_front().expect("ordered queues are not empty");
        if q.is_empty() {
            self.queues.remove(&a);
        } else {
            self.order.push_back(a);
        }
        self.len -= 1;
        Some(t)
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Removes and returns every item `old` holds for, from any place in its actor's queue: deadlines differ by item.
    pub(crate) fn take_old(&mut self, old: impl Fn(&T) -> bool) -> Vec<T> {
        let mut out = Vec::new();
        for q in self.queues.values_mut() {
            let (gone, kept): (VecDeque<T>, VecDeque<T>) = std::mem::take(q).into_iter().partition(|t| old(t));
            *q = kept;
            out.extend(gone);
        }
        if !out.is_empty() {
            self.len -= out.len();
            self.queues.retain(|_, q| !q.is_empty());
            let queues = &self.queues;
            self.order.retain(|a| queues.contains_key(a));
        }
        out
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.queues.values().flatten()
    }

    pub(crate) fn drain(self) -> impl Iterator<Item = T> {
        self.queues.into_values().flatten()
    }
}

/// The retry hint a refusal carries: the seconds the agent bucket needs to drain what is queued, plus one.
pub fn retry_after_s(agent_queued: usize, capacity: f32) -> u32 {
    let rate = (AGENT_SHARE * capacity).max(1.0);
    ((agent_queued as f32 / rate).ceil() as u32 + 1).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lane_with_one_actor_is_a_fifo_and_actors_take_turns() {
        let mut l = Lane::default();
        for i in 0..4 {
            l.push(None, (0, i));
        }
        assert_eq!((0..4).map(|_| l.pop().unwrap().1).collect::<Vec<_>>(), [0, 1, 2, 3]);
        let (a, b) = (Some(ActorKey(1)), Some(ActorKey(2)));
        for i in 0..3 {
            l.push(a, (1, i));
        }
        l.push(b, (2, 0));
        let got: Vec<_> = std::iter::from_fn(|| l.pop()).collect();
        assert_eq!(got, [(1, 0), (2, 0), (1, 1), (1, 2)]);
    }

    #[test]
    fn old_items_leave_from_every_actor() {
        let mut l = Lane::default();
        l.push(Some(ActorKey(1)), 1);
        l.push(Some(ActorKey(2)), 2);
        l.push(Some(ActorKey(1)), 10);
        assert_eq!(l.take_old(|x| *x < 5), [1, 2]);
        assert_eq!((l.len(), l.pop()), (1, Some(10)));
    }

    #[test]
    fn retry_after_counts_the_queue_at_the_agent_rate() {
        assert_eq!(retry_after_s(0, 300.0), 1);
        assert_eq!(retry_after_s(1_000, 300.0), 8);
    }
}
