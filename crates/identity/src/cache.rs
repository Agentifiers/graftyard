//! The per-isolate identity cache (`design/DESIGN.md`, Shape, "Identity", `READ_CACHE` and
//! `IDENTITY_BUDGET_PER_ISOLATE`; "Commit and read paths", journey 7, "Tenfold identity reads").
//!
//! A read-class request may use an entry up to `READ_CACHE` old. A write-class request always reads the group, and
//! the leader reads the credential again once per entry. Each isolate reads identity groups at most
//! `IDENTITY_BUDGET_PER_ISOLATE` times a second: beyond it, read-class requests use cached entries and write-class
//! requests get 503. The clock is the caller's.

use std::collections::BTreeMap;
use std::time::Duration;

use graftyard_types::Timestamp;

pub const READ_CACHE: Duration = Duration::from_secs(10);
/// Group reads per second per isolate; compiled, never config.
pub const IDENTITY_BUDGET_PER_ISOLATE: u32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestClass {
    Read,
    Write,
}

/// What the caller does for one identity lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup<V> {
    /// Use this entry, no older than `READ_CACHE`.
    Hit(V),
    /// Read the group (on its leader for a write-class request), then `insert` the answer. The read was counted.
    Fetch,
    /// Over budget: a read-class request uses this entry, whatever its age.
    Stale(V),
    /// Over budget with nothing to serve: 503.
    Overloaded,
}

#[derive(Clone, Debug)]
pub struct IdentityCache<K, V> {
    entries: BTreeMap<K, (Timestamp, V)>,
    /// The second the budget counts in, and the reads counted in it.
    second: u64,
    reads: u32,
    /// The most entries kept; the oldest go first.
    cap: usize,
}

impl<K: Ord + Clone, V: Clone> IdentityCache<K, V> {
    pub fn new(cap: usize) -> Self {
        IdentityCache { entries: BTreeMap::new(), second: 0, reads: 0, cap: cap.max(1) }
    }

    fn take_budget(&mut self, now: Timestamp) -> bool {
        let second = now.0 / 1000;
        if second != self.second {
            self.second = second;
            self.reads = 0;
        }
        if self.reads < IDENTITY_BUDGET_PER_ISOLATE {
            self.reads += 1;
            true
        } else {
            false
        }
    }

    pub fn lookup(&mut self, now: Timestamp, key: &K, class: RequestClass) -> Lookup<V> {
        let cached = self.entries.get(key).cloned();
        if class == RequestClass::Read
            && let Some((at, v)) = &cached
            && now.0.saturating_sub(at.0) <= READ_CACHE.as_millis() as u64
        {
            return Lookup::Hit(v.clone());
        }
        if self.take_budget(now) {
            return Lookup::Fetch;
        }
        match (class, cached) {
            (RequestClass::Read, Some((_, v))) => Lookup::Stale(v),
            _ => Lookup::Overloaded,
        }
    }

    /// Stores what a `Fetch` read, stamped with the time the read started, so the entry ages from the group's
    /// answer, not from when it arrived.
    pub fn insert(&mut self, read_at: Timestamp, key: K, value: V) {
        if self.entries.get(&key).is_some_and(|(at, _)| *at > read_at) {
            return;
        }
        self.entries.insert(key, (read_at, value));
        while self.entries.len() > self.cap {
            let oldest = self.entries.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone()).expect("non-empty");
            self.entries.remove(&oldest);
        }
    }

    /// Drops an entry this isolate saw revoked, so its next read-class request fetches.
    pub fn forget(&mut self, key: &K) {
        self.entries.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_uses_entries_up_to_10_s_and_writes_always_fetch() {
        let mut c = IdentityCache::new(8);
        assert_eq!(c.lookup(Timestamp(1_000), &1, RequestClass::Read), Lookup::Fetch);
        c.insert(Timestamp(1_000), 1, "row");
        assert_eq!(c.lookup(Timestamp(11_000), &1, RequestClass::Read), Lookup::Hit("row"));
        assert_eq!(c.lookup(Timestamp(11_001), &1, RequestClass::Read), Lookup::Fetch);
        assert_eq!(c.lookup(Timestamp(2_000), &1, RequestClass::Write), Lookup::Fetch);
        // An older answer never replaces a newer one.
        c.insert(Timestamp(20_000), 1, "new");
        c.insert(Timestamp(19_000), 1, "old");
        assert_eq!(c.lookup(Timestamp(20_001), &1, RequestClass::Read), Lookup::Hit("new"));
    }

    #[test]
    fn budget_is_50_reads_a_second_then_stale_reads_and_503_writes() {
        let mut c = IdentityCache::new(8);
        c.insert(Timestamp(0), 7, "seven");
        let now = Timestamp(60_500);
        for _ in 0..IDENTITY_BUDGET_PER_ISOLATE {
            assert_eq!(c.lookup(now, &1, RequestClass::Write), Lookup::Fetch);
        }
        assert_eq!(c.lookup(now, &1, RequestClass::Write), Lookup::Overloaded);
        assert_eq!(c.lookup(now, &7, RequestClass::Read), Lookup::Stale("seven"));
        assert_eq!(c.lookup(now, &8, RequestClass::Read), Lookup::Overloaded);
        // The next second has a fresh budget.
        assert_eq!(c.lookup(Timestamp(61_000), &1, RequestClass::Write), Lookup::Fetch);
    }

    #[test]
    fn capacity_drops_the_oldest() {
        let mut c = IdentityCache::new(2);
        c.insert(Timestamp(1), 1, ());
        c.insert(Timestamp(2), 2, ());
        c.insert(Timestamp(3), 3, ());
        assert_eq!(c.lookup(Timestamp(4), &1, RequestClass::Read), Lookup::Fetch);
        assert_eq!(c.lookup(Timestamp(4), &3, RequestClass::Read), Lookup::Hit(()));
    }
}
