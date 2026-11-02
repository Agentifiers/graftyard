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
}

impl<K: Ord, V> Table<K, V> for BTreeMap<K, V> {
    fn get(&self, k: &K) -> Option<&V> {
        BTreeMap::get(self, k)
    }
    fn put(&mut self, k: K, v: V) {
        self.insert(k, v);
    }
}

impl<K: Ord + Clone, V> Table<K, V> for Overlay<'_, K, V> {
    fn get(&self, k: &K) -> Option<&V> {
        Overlay::get(self, k)
    }
    fn put(&mut self, k: K, v: V) {
        Overlay::put(self, k, v)
    }
}
