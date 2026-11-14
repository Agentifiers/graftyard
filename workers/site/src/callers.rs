//! Which messages to a learner leave as calls, and which wait for that learner's next call to this member.
//!
//! The Workers runtime charges each call an object makes to the newest request the object received, and refuses a
//! call nested 16 deep ("Subrequest depth limit exceeded"). A learner calls the leader with `ReadIndex` about 20 times
//! a second. If the leader also called the learner, with AppendEntries or a late `ReadIndexRes`, each call would
//! leave from inside the other's newest request, one level deeper per exchange. That cycle reached 16 in B0-18's runs
//! on `graft-m2b`, and failed the leader's AppendEntries to its voters as well, so `STEP_DOWN` tripped and the learners'
//! reads failed (`tests/m3/results/m3-wp5.md`, "B0-18 traced on `graft-m2b`").
//!
//! So a member does not call a learner while that learner calls it. A learner with a call in progress here, or one
//! that called within `QUIET` ago, gets its messages in the response to its next call. A learner that stopped calling
//! is called again once `QUIET` has passed (`due`). An idle learner, such as one that serves only feed polls, is
//! called as before.

use std::collections::BTreeMap;

use futures_channel::oneshot;

use graftyard_types::{SiteId, learner_slot};

/// How long after a learner's last call this member still holds its messages for the next one: a learner sends a
/// read-index batch at most every 50 ms, plus its round trip, so a busy learner calls well within this
/// (`raft::CALL_DEADLINE`, [guess]).
pub const QUIET: u64 = 1_000;

/// Per learner that called this member: its calls in progress, when the last began or ended, and the messages held
/// for it, oldest first.
pub struct Callers<E> {
    by: BTreeMap<SiteId, Caller<E>>,
}

struct Caller<E> {
    open: u32,
    last: u64,
    held: Vec<E>,
    /// The encoded bytes of `held`, and the most it held at once since `/stats` was read (narrow-reads-3).
    held_bytes: usize,
    held_peak: usize,
    /// A call waiting for a message to be held for it (`wake`).
    wake: Vec<oneshot::Sender<()>>,
}

impl<E> Default for Callers<E> {
    fn default() -> Self {
        Callers { by: BTreeMap::new() }
    }
}

impl<E> Callers<E> {
    /// A call from `from` began at `now`. Only learners are tracked: voters never call a member often enough to form a
    /// cycle, and their AppendEntries must leave at once.
    pub fn began(&mut self, from: SiteId, now: u64) {
        if learner_slot(from).is_some() {
            let c = self.by.entry(from).or_insert(Caller {
                open: 0,
                last: now,
                held: Vec::new(),
                held_bytes: 0,
                held_peak: 0,
                wake: Vec::new(),
            });
            c.open += 1;
            c.last = now;
        }
    }

    /// That call ended at `now`. Its response takes what `take` returns for it, then this.
    pub fn ended(&mut self, from: SiteId, now: u64) {
        if let Some(c) = self.by.get_mut(&from) {
            c.open = c.open.saturating_sub(1);
            c.last = now;
        }
    }

    /// `msg` for `to` at `now`, of `bytes` encoded: held for `to`'s next call, or handed back to be sent as a call.
    pub fn route(&mut self, to: SiteId, msg: E, bytes: usize, now: u64) -> Option<E> {
        match self.by.get_mut(&to) {
            Some(c) if c.open > 0 || now < c.last + QUIET => {
                c.held.push(msg);
                c.held_bytes += bytes;
                c.held_peak = c.held_peak.max(c.held_bytes);
                for w in c.wake.drain(..) {
                    let _ = w.send(());
                }
                None
            }
            _ => Some(msg),
        }
    }

    /// The messages held for `from`, for the response to its call.
    pub fn take(&mut self, from: SiteId) -> Vec<E> {
        self.by
            .get_mut(&from)
            .map(|c| {
                c.held_bytes = 0;
                std::mem::take(&mut c.held)
            })
            .unwrap_or_default()
    }

    /// Per learner: messages and bytes held now, the most bytes held at once since the last call with `reset`, and
    /// calls open. Clears the peaks when `reset`.
    pub fn held_stats(&mut self, reset: bool) -> Vec<(SiteId, usize, usize, usize, u32)> {
        let out = self.by.iter().map(|(s, c)| (*s, c.held.len(), c.held_bytes, c.held_peak, c.open)).collect();
        if reset {
            for c in self.by.values_mut() {
                c.held_peak = c.held_bytes;
            }
        }
        out
    }

    /// Held messages for learners that stopped calling: no call in progress and none for `QUIET`. They leave as calls,
    /// and the learner is forgotten until it calls again.
    pub fn due(&mut self, now: u64) -> Vec<E> {
        let idle: Vec<SiteId> =
            self.by.iter().filter(|(_, c)| c.open == 0 && now >= c.last + QUIET).map(|(s, _)| *s).collect();
        idle.into_iter().flat_map(|s| self.by.remove(&s).map(|c| c.held).unwrap_or_default()).collect()
    }

    /// Whether a message held for `from` matches `f`.
    pub fn holds(&self, from: SiteId, f: impl Fn(&E) -> bool) -> bool {
        self.by.get(&from).is_some_and(|c| c.held.iter().any(f))
    }

    /// Resolves when the next message is held for `from`, a learner whose call is open.
    pub fn wake(&mut self, from: SiteId) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        if let Some(c) = self.by.get_mut(&from) {
            c.wake.push(tx);
        }
        rx
    }

    /// The encoded bytes held for every learner now (`Host::mark_mem`).
    #[cfg(feature = "test-faults")]
    pub fn held_total(&self) -> usize {
        self.by.values().map(|c| c.held_bytes).sum()
    }

    /// Whether messages wait for a learner, so the ticker keeps running until `due` sends them.
    pub fn holding(&self) -> bool {
        self.by.values().any(|c| !c.held.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use graftyard_types::{SiteId, learner_addr};

    use super::{Callers, QUIET};

    const LEADER: SiteId = SiteId(0);
    /// Nested calls the runtime allows; the 17th is refused.
    const LIMIT: u32 = 16;

    /// Calls between one leader and one learner, as the runtime counts them: every call leaves at its sender's newest
    /// incoming request's depth plus one. As in B0-18's runs with `--pushes 0`, the learner's reads arrive inside one
    /// long outside request (`read_bench`, depth 2) and nothing else calls the leader. Each 50 ms the learner calls
    /// the leader with a read-index batch, which the leader answers in its response; each 20 ms the leader has an
    /// AppendEntries for the learner, which it sends as a call, or with `callers` holds for the learner's next call.
    /// Returns the deepest call made in 30 s.
    fn deepest(mut callers: Option<&mut Callers<u32>>) -> u32 {
        let l = learner_addr(0);
        let (mut leader_newest, mut learner_newest) = (2u32, 2u32);
        let mut deepest = 0;
        for ms in 0..30_000u64 {
            if ms % 50 == 0 {
                let d = learner_newest + 1;
                deepest = deepest.max(d);
                leader_newest = d;
                if let Some(c) = callers.as_deref_mut() {
                    c.began(l, ms);
                    // The held AppendEntries ride in this call's response, and the learner steps them there.
                    let _ = c.take(l);
                    c.ended(l, ms + 30);
                }
            }
            if ms % 20 == 0 {
                let call = match callers.as_deref_mut() {
                    Some(c) => c.route(l, 1, 0, ms).is_some() || !c.due(ms).is_empty(),
                    None => true,
                };
                if call {
                    let d = leader_newest + 1;
                    deepest = deepest.max(d);
                    learner_newest = d;
                }
            }
        }
        deepest
    }

    /// B0-18: with the leader calling the learner that calls it, each exchange nests one level deeper until the
    /// runtime refuses the call; holding the learner's messages for its own calls keeps every call shallow.
    #[test]
    fn a_learner_that_calls_the_leader_is_never_called_back_so_depth_stays_bounded() {
        assert!(deepest(None) > LIMIT, "calling a learner back as it calls nests without bound");
        let mut callers = Callers::default();
        let d = deepest(Some(&mut callers));
        assert_eq!(d, 3, "held for its calls, every call is one hop from an outside request");
    }

    #[test]
    fn a_learner_that_stops_calling_is_called_again_after_quiet() {
        let l = learner_addr(3);
        let mut c: Callers<u32> = Callers::default();
        assert_eq!(c.route(l, 1, 0, 0), Some(1), "a learner that never called is called");
        assert_eq!(c.route(LEADER, 2, 0, 0), Some(2), "a voter is always called");
        c.began(l, 10);
        assert_eq!(c.route(l, 3, 0, 20), None, "held while its call is open");
        c.ended(l, 40);
        assert_eq!(c.route(l, 4, 0, 40 + QUIET - 1), None, "held inside QUIET");
        assert!(c.due(40 + QUIET - 1).is_empty() && c.holding());
        assert_eq!(c.due(40 + QUIET), vec![3, 4], "sent once it stopped calling");
        assert!(!c.holding());
        assert_eq!(c.route(l, 5, 0, 40 + QUIET), Some(5), "then called as before");
        c.began(l, 2_000);
        c.route(l, 6, 0, 2_001);
        assert_eq!(c.take(l), vec![6], "its next call takes what was held");
    }
}
