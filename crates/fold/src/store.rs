//! The rows a fold tier is made of, and the store that holds them (`design/SHARED.md`, "Folds";
//! `design/DESIGN.md`, Shape, "Reused with the additions below").
//!
//! A tier is a set of tables. A row is a primary key and columns of SQLite's storage classes, so a Durable Object host
//! keeps each table as a SQLite table with real columns its list queries index, and tests keep it in a `MemStore`.
//! `Val`'s derived order is SQLite's for these classes (NULL, then integers, then text by BINARY collation, then blobs
//! by `memcmp`), so a scan returns rows in the order a SQLite primary key does.

use std::collections::BTreeMap;

use graftyard_types::Index;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Val {
    Null,
    Int(i64),
    Text(Box<str>),
    Blob(Box<[u8]>),
}

/// A primary key. A shorter key used as a scan's start sorts before every longer key it prefixes, as SQLite orders
/// `(a) >= (a, NULL)`, so scanning from a prefix starts at the prefix's first row.
pub type Key = Vec<Val>;

/// A row as stored: the index of the entry that last wrote it, and its columns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub written: Index,
    pub cols: Vec<Val>,
}

/// One table of a tier: its name and its key columns, for the host's DDL. Besides its key a row has the index that
/// wrote it and its columns; the chain's tables store one column, the row's record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableDef {
    pub name: &'static str,
    pub key: &'static [&'static str],
}

/// The tables of one tier, in the order `fold_digest` and snapshots visit them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schema {
    pub tier: graftyard_types::Tier,
    pub tables: &'static [TableDef],
}

/// What a host implements: rows by table name and key. `scan` returns at most `limit` rows with key `>= from`, in
/// key order. Table names outside the schema (`META`, `undo_table`) hold the tier's own bookkeeping.
pub trait Store {
    fn get(&self, table: &str, key: &[Val]) -> Option<Stored>;
    fn put(&mut self, table: &str, key: Key, row: Stored);
    fn delete(&mut self, table: &str, key: &[Val]);
    fn scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Stored)>;
    /// Deletes every row of `table`.
    fn clear(&mut self, table: &str) {
        loop {
            let rows = self.scan(table, &[], 512);
            if rows.is_empty() {
                return;
            }
            for (k, _) in rows {
                self.delete(table, &k);
            }
        }
    }
}

/// A store a fold value owns: a `MemStore`, or a host's handle on its own tables, which clones as another handle on the
/// same tables. A fold that holds `Box<dyn TierStore>` so runs unchanged in tests and on a Durable Object.
pub trait TierStore: Store + std::fmt::Debug {
    fn clone_box(&self) -> Box<dyn TierStore>;
    /// The concrete store, for a host that reads its own tables beside the trait (a search index).
    fn as_any(&self) -> &dyn std::any::Any;
    /// Whether the tier has caught up and `apply` writes it. A host's metadata tier loads lazily: until it has, a
    /// fold applies the core tier alone, and a command that reads the metadata tier is refused.
    fn loaded(&self) -> bool {
        true
    }
}

/// A boxed tier is a store, so `&Box<dyn TierStore>` reads as `&dyn Read`.
impl Store for Box<dyn TierStore> {
    fn get(&self, table: &str, key: &[Val]) -> Option<Stored> {
        (**self).get(table, key)
    }
    fn put(&mut self, table: &str, key: Key, row: Stored) {
        (**self).put(table, key, row)
    }
    fn delete(&mut self, table: &str, key: &[Val]) {
        (**self).delete(table, key)
    }
    fn scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Stored)> {
        (**self).scan(table, from, limit)
    }
    fn clear(&mut self, table: &str) {
        (**self).clear(table)
    }
}

impl Clone for Box<dyn TierStore> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

impl TierStore for MemStore {
    fn clone_box(&self) -> Box<dyn TierStore> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The in-memory store tests use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemStore(pub BTreeMap<String, BTreeMap<Key, Stored>>);

impl Store for MemStore {
    fn get(&self, table: &str, key: &[Val]) -> Option<Stored> {
        self.0.get(table)?.get(key).cloned()
    }
    fn put(&mut self, table: &str, key: Key, row: Stored) {
        self.0.entry(table.to_owned()).or_default().insert(key, row);
    }
    fn delete(&mut self, table: &str, key: &[Val]) {
        if let Some(t) = self.0.get_mut(table) {
            t.remove(key);
        }
    }
    fn scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Stored)> {
        let Some(t) = self.0.get(table) else { return Vec::new() };
        t.range(from.to_vec()..).take(limit).map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

/// The tier's own row: `applied`, the digest of entry `applied`, its `at`, the row-hash accumulator, and the running
/// snapshot's `to`.
pub const META: &str = "_fold";

/// The `snap_undo` rows of `table` (`design/DESIGN.md`, Shape, Packs).
pub fn undo_table(table: &str) -> String {
    format!("{table}~undo")
}

/// Reads over a tier, or over a tier with an entry's earlier events on top.
pub trait Read {
    fn get(&self, table: &str, key: &[Val]) -> Option<Vec<Val>>;
    fn scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Vec<Val>)>;

    /// Every row whose key starts with `prefix`, in key order.
    fn prefix(&self, table: &str, prefix: &[Val]) -> Vec<(Key, Vec<Val>)> {
        let mut out = Vec::new();
        let mut from = prefix.to_vec();
        loop {
            let batch = self.scan(table, &from, 256);
            let full = batch.len() == 256;
            for (k, v) in batch {
                if !k.starts_with(prefix) {
                    return out;
                }
                if out.last().is_some_and(|(l, _): &(Key, Vec<Val>)| *l == k) {
                    continue;
                }
                from = k.clone();
                out.push((k, v));
            }
            if !full {
                return out;
            }
        }
    }
}

/// Writes. `apply` and a `decide` scratch view run the same write code through this trait, so they never diverge.
pub trait Rows: Read {
    fn put(&mut self, table: &str, key: Key, cols: Vec<Val>);
    fn delete(&mut self, table: &str, key: &[Val]);
}

/// A tier with uncommitted writes on top: the scratch view `decide` and `verify` run on, then drop.
pub struct Overlay<'a, R: Read + ?Sized> {
    base: &'a R,
    over: BTreeMap<(String, Key), Option<Vec<Val>>>,
}

impl<'a, R: Read + ?Sized> Overlay<'a, R> {
    pub fn new(base: &'a R) -> Self {
        Overlay { base, over: BTreeMap::new() }
    }
}

impl<R: Read + ?Sized> Read for Overlay<'_, R> {
    fn get(&self, table: &str, key: &[Val]) -> Option<Vec<Val>> {
        match self.over.get(&(table.to_owned(), key.to_vec())) {
            Some(v) => v.clone(),
            None => self.base.get(table, key),
        }
    }

    /// Fetches enough base rows that the overlay's deletes cannot leave fewer than `limit`, so any row past the last
    /// fetched base row sorts after `limit` visible ones and is cut.
    fn scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Vec<Val>)> {
        let lo = (table.to_owned(), from.to_vec());
        let mine: Vec<(&Key, &Option<Vec<Val>>)> =
            self.over.range(lo..).take_while(|((t, _), _)| t == table).map(|((_, k), v)| (k, v)).collect();
        let deletes = mine.iter().filter(|(_, v)| v.is_none()).count();
        let mut merged: BTreeMap<Key, Vec<Val>> = self.base.scan(table, from, limit + deletes).into_iter().collect();
        for (k, v) in mine {
            match v {
                Some(v) => merged.insert(k.clone(), v.clone()),
                None => merged.remove(k),
            };
        }
        merged.into_iter().take(limit).collect()
    }
}

impl<R: Read + ?Sized> Rows for Overlay<'_, R> {
    fn put(&mut self, table: &str, key: Key, cols: Vec<Val>) {
        self.over.insert((table.to_owned(), key), Some(cols));
    }
    fn delete(&mut self, table: &str, key: &[Val]) {
        self.over.insert((table.to_owned(), key.to_vec()), None);
    }
}
