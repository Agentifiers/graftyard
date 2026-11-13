//! The edge's per-isolate read caches (`design/DESIGN.md`, Shape, Identity, `READ_CACHE`, and "Follower read"): a
//! read-class request (a repository `GET`, a GraphQL query) may use an answer up to `READ_CACHE` old for a stored
//! token's row, the `ghs_` verifying keys and a `jti`'s revocation, a login's account, an owner's `Placement` row, the
//! owner's `Authz` fold, and an account's login. A write-class request reads each one fresh, and its answer refreshes
//! the entry.
//!
//! An entry holds only an answer that lets a request through: a usable token, a `jti` not revoked, a claimed login, a
//! fold. Every negative answer (no such token, login, or repository, a gate that hides the repository) is read again
//! fresh before it is returned, so a revocation binds reads within `READ_CACHE` while a new token, repository, or grant
//! is served at once. An entry ages from the moment its read started, not from when the answer arrived, and an older
//! answer never replaces a newer one. Entries are values, never shared promises: a Workers isolate refuses a promise
//! resolved in another request's context (F65).

use std::cell::RefCell;
use std::collections::BTreeMap;

use graftyard_identity::READ_CACHE;
use graftyard_types::Timestamp;

/// A map of answers, each stamped with the time its read started, and which answers it keeps.
pub(crate) struct Ttl<K, V> {
    entries: BTreeMap<K, (Timestamp, V)>,
    cap: usize,
    /// Whether an answer may be stored: only one that lets a request through.
    keep: fn(&V) -> bool,
}

impl<K: Ord, V: Clone> Ttl<K, V> {
    pub(crate) const fn new(cap: usize, keep: fn(&V) -> bool) -> Self {
        Ttl { entries: BTreeMap::new(), cap, keep }
    }

    /// The entry for `k` if the request is read-class and the entry is at most `READ_CACHE` old.
    pub(crate) fn get(&self, k: &K, now: Timestamp, read_class: bool) -> Option<V> {
        if !read_class {
            return None;
        }
        let ttl = u64::try_from(READ_CACHE.as_millis()).unwrap_or(u64::MAX);
        self.entries.get(k).filter(|(at, _)| now.0.saturating_sub(at.0) <= ttl).map(|(_, v)| v.clone())
    }

    /// Stores an answer whose read started at `read_at`, unless the entry holds a newer one. Past `cap` entries the
    /// map starts empty again, as the read gate's does.
    pub(crate) fn put(&mut self, k: K, read_at: Timestamp, v: V) {
        if !(self.keep)(&v) || self.entries.get(&k).is_some_and(|(at, _)| *at > read_at) {
            return;
        }
        if self.entries.len() >= self.cap && !self.entries.contains_key(&k) {
            self.entries.clear();
        }
        self.entries.insert(k, (read_at, v));
    }
}

/// One cache per kind of answer, with the most entries it holds and the answers it keeps. The `Authz` folds are the
/// large ones, so fewer are kept.
macro_rules! caches {
    ($($name:ident: $k:ty => $v:ty, $cap:expr, $keep:expr;)*) => {
        thread_local! {
            $(pub(crate) static $name: RefCell<Ttl<$k, $v>> = const { RefCell::new(Ttl::new($cap, $keep)) };)*
        }
    };
}

caches! {
    TOKENS: [u8; 32] => crate::TokenRow, 4_096, usable_row;
    KEYS: () => graftyard_identity::TokensFold, 1, any;
    JTIS: u128 => bool, 4_096, not_revoked;
    LOGINS: graftyard_identity::LoginKey => graftyard_types::AccountId, 4_096, any;
    PLACEMENTS: graftyard_types::AccountId => graftyard_types::PlacementRecord, 4_096, any;
    AUTHZ: graftyard_types::AccountId => graftyard_identity::AuthzFold, 64, any;
    ACCOUNT_LOGINS: i64 => String, 4_096, any;
}

/// `cache`'s entry for `k`, as `Ttl::get`.
pub(crate) fn get<K: Ord, V: Clone>(
    cache: &'static std::thread::LocalKey<RefCell<Ttl<K, V>>>,
    k: &K,
    now: Timestamp,
    read_class: bool,
) -> Option<V> {
    cache.with(|c| c.borrow().get(k, now, read_class))
}

/// Stores `v` in `cache`, as `Ttl::put`.
pub(crate) fn put<K: Ord, V: Clone>(
    cache: &'static std::thread::LocalKey<RefCell<Ttl<K, V>>>,
    k: K,
    read_at: Timestamp,
    v: V,
) {
    cache.with(|c| c.borrow_mut().put(k, read_at, v));
}

/// A read through `cache`: a read-class request's entry for `k` when one is at most `READ_CACHE` old, else `read`,
/// whose answer is stored stamped `at` (when the read started) if the cache keeps it. A write-class request always
/// reads and refreshes the entry.
pub(crate) async fn through<K: Ord, V: Clone, E>(
    cache: &'static std::thread::LocalKey<RefCell<Ttl<K, V>>>,
    k: K,
    at: Timestamp,
    read_class: bool,
    read: impl std::future::Future<Output = Result<Option<V>, E>>,
) -> Result<Option<V>, E> {
    if let Some(v) = get(cache, &k, at, read_class) {
        return Ok(Some(v));
    }
    let v = read.await?;
    if let Some(v) = &v {
        put(cache, k, at, v.clone());
    }
    Ok(v)
}

/// Every answer is kept: a fold, a placement, a claimed login.
fn any<V>(_: &V) -> bool {
    true
}

/// A stored token's row is kept unless it is revoked; its expiry is checked on every use.
fn usable_row(r: &crate::TokenRow) -> bool {
    !r.revoked
}

/// A `jti`'s revocation read is kept only when it found no revocation.
fn not_revoked(revoked: &bool) -> bool {
    !*revoked
}

#[cfg(test)]
mod tests {
    use futures_util::FutureExt;

    use super::*;

    thread_local! {
        static T: RefCell<Ttl<u8, bool>> = const { RefCell::new(Ttl::new(8, not_revoked)) };
    }

    /// One `through` call on `T` that must finish at once, and whether it read.
    fn call(at: u64, read_class: bool, answer: Option<bool>) -> (Option<bool>, bool) {
        let mut read = false;
        let out = through(&T, 1, Timestamp(at), read_class, async {
            read = true;
            Ok::<_, ()>(answer)
        })
        .now_or_never()
        .expect("ready")
        .expect("ok");
        (out, read)
    }

    #[test]
    fn through_keeps_only_what_lets_a_request_through_and_writes_always_read() {
        // A revoked answer is returned but never kept: the next read-class request reads again.
        assert_eq!(call(1_000, true, Some(true)), (Some(true), true));
        assert_eq!(call(1_001, true, Some(false)), (Some(false), true));
        // A kept answer serves read-class requests without a read, up to READ_CACHE from its read's start.
        assert_eq!(call(11_001, true, Some(true)), (Some(false), false));
        assert_eq!(call(11_002, true, Some(true)), (Some(true), true));
        // A write-class request reads even with a fresh entry; its answer (here a revocation) is the one that counts,
        // and a kept one refreshes the entry.
        assert_eq!(call(11_003, true, Some(false)), (Some(false), true));
        assert_eq!(call(11_004, false, Some(true)), (Some(true), true));
        assert_eq!(call(11_005, false, Some(false)), (Some(false), true));
        assert_eq!(call(21_005, true, Some(true)), (Some(false), false));
        // No answer (no such row) is never kept.
        T.with(|t| *t.borrow_mut() = Ttl::new(8, not_revoked));
        assert_eq!(call(0, true, None), (None, true));
        assert_eq!(call(1, true, Some(false)), (Some(false), true));
    }

    /// Whether `cache` keeps `v`.
    fn keeps<K: Ord + Default, V: Clone>(cache: &'static std::thread::LocalKey<RefCell<Ttl<K, V>>>, v: V) -> bool {
        cache.with(|c| {
            let mut c = c.borrow_mut();
            c.put(K::default(), Timestamp(u64::MAX), v);
            let kept = c.get(&K::default(), Timestamp(u64::MAX), true).is_some();
            c.entries.clear();
            kept
        })
    }

    #[test]
    fn the_identity_caches_never_keep_a_revocation() {
        let row = |revoked| crate::TokenRow { account: 1, revoked, fields: None, kind: None, expires: None };
        assert!(keeps(&TOKENS, row(false)));
        assert!(!keeps(&TOKENS, row(true)));
        assert!(keeps(&JTIS, false));
        assert!(!keeps(&JTIS, true));
    }

    #[test]
    fn a_read_uses_an_entry_up_to_read_cache_old_and_a_write_never() {
        let mut c: Ttl<u8, &str> = Ttl::new(8, any);
        c.put(1, Timestamp(1_000), "row");
        assert_eq!(c.get(&1, Timestamp(1_000), true), Some("row"));
        assert_eq!(c.get(&1, Timestamp(11_000), true), Some("row"));
        // One millisecond past READ_CACHE from the read's start, the entry is gone.
        assert_eq!(c.get(&1, Timestamp(11_001), true), None);
        // A write-class request never reads an entry, however new.
        assert_eq!(c.get(&1, Timestamp(1_000), false), None);
        assert_eq!(c.get(&2, Timestamp(1_000), true), None);
    }

    #[test]
    fn an_entry_ages_from_its_read_start_and_an_older_answer_never_wins() {
        let mut c: Ttl<u8, &str> = Ttl::new(8, any);
        // A read that started at 20 s and answered late is still stamped 20 s.
        c.put(1, Timestamp(20_000), "new");
        // A slower read that started earlier answers after it: the newer answer stays.
        c.put(1, Timestamp(15_000), "old");
        assert_eq!(c.get(&1, Timestamp(25_000), true), Some("new"));
        assert_eq!(c.get(&1, Timestamp(30_001), true), None);
        // A later read replaces it.
        c.put(1, Timestamp(26_000), "newer");
        assert_eq!(c.get(&1, Timestamp(30_001), true), Some("newer"));
    }

    #[test]
    fn past_its_cap_the_map_starts_empty_and_keys_stay_apart() {
        let mut c: Ttl<u8, u8> = Ttl::new(2, any);
        c.put(1, Timestamp(0), 10);
        c.put(2, Timestamp(0), 20);
        assert_eq!((c.get(&1, Timestamp(0), true), c.get(&2, Timestamp(0), true)), (Some(10), Some(20)));
        // Updating a key already held does not clear the map.
        c.put(2, Timestamp(1), 21);
        assert_eq!(c.get(&1, Timestamp(1), true), Some(10));
        c.put(3, Timestamp(1), 30);
        assert_eq!(c.get(&1, Timestamp(1), true), None);
        assert_eq!(c.get(&3, Timestamp(1), true), Some(30));
    }
}
