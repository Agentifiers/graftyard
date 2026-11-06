//! The scratch view `decide` and `verify` run on: rows written by earlier events of the entry, over the fold.

use std::collections::BTreeMap;

#[derive(Debug)]
pub(crate) struct Overlay<'a, K, V> {
    base: &'a BTreeMap<K, V>,
    over: BTreeMap<K, Option<V>>,
}

impl<'a, K: Ord + Clone, V> Overlay<'a, K, V> {
    pub(crate) fn new(base: &'a BTreeMap<K, V>) -> Self {
        Overlay { base, over: BTreeMap::new() }
    }

    pub(crate) fn get(&self, k: &K) -> Option<&V> {
        match self.over.get(k) {
            Some(v) => v.as_ref(),
            None => self.base.get(k),
        }
    }

    pub(crate) fn put(&mut self, k: K, v: V) {
        self.over.insert(k, Some(v));
    }

    pub(crate) fn delete(&mut self, k: K) {
        self.over.insert(k, None);
    }
}

/// The writes one event makes, shared by a fold's `apply` and its scratch's, so the two never diverge.
pub(crate) trait Table<K, V> {
    fn get(&self, k: &K) -> Option<&V>;
    fn put(&mut self, k: K, v: V);
    fn del(&mut self, k: &K);
}

impl<K: Ord, V> Table<K, V> for BTreeMap<K, V> {
    fn get(&self, k: &K) -> Option<&V> {
        BTreeMap::get(self, k)
    }
    fn put(&mut self, k: K, v: V) {
        self.insert(k, v);
    }
    fn del(&mut self, k: &K) {
        self.remove(k);
    }
}

impl<K: Ord + Clone, V> Table<K, V> for Overlay<'_, K, V> {
    fn get(&self, k: &K) -> Option<&V> {
        Overlay::get(self, k)
    }
    fn put(&mut self, k: K, v: V) {
        Overlay::put(self, k, v)
    }
    fn del(&mut self, k: &K) {
        Overlay::delete(self, k.clone())
    }
}

impl<K: Ord + Clone> Table<K, ()> for std::collections::BTreeSet<K> {
    fn get(&self, k: &K) -> Option<&()> {
        self.contains(k).then_some(&())
    }
    fn put(&mut self, k: K, _: ()) {
        self.insert(k);
    }
    fn del(&mut self, k: &K) {
        self.remove(k);
    }
}

/// An overlay of a set: the members earlier events of the entry added or removed.
#[derive(Debug)]
pub(crate) struct SetOverlay<'a, K> {
    base: &'a std::collections::BTreeSet<K>,
    over: BTreeMap<K, bool>,
}

impl<'a, K: Ord + Clone> SetOverlay<'a, K> {
    pub(crate) fn new(base: &'a std::collections::BTreeSet<K>) -> Self {
        SetOverlay { base, over: BTreeMap::new() }
    }

    pub(crate) fn contains(&self, k: &K) -> bool {
        self.over.get(k).copied().unwrap_or_else(|| self.base.contains(k))
    }
}

impl<K: Ord + Clone> Table<K, ()> for SetOverlay<'_, K> {
    fn get(&self, k: &K) -> Option<&()> {
        self.contains(k).then_some(&())
    }
    fn put(&mut self, k: K, _: ()) {
        self.over.insert(k, true);
    }
    fn del(&mut self, k: &K) {
        self.over.insert(k.clone(), false);
    }
}
