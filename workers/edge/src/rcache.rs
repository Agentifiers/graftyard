//! The edge's per-isolate read caches (`design/DESIGN.md`, Shape, Identity, `READ_CACHE`, and "Follower read"): a
//! read-class request (a repository `GET`, a GraphQL query, a git fetch) may use an answer up to `READ_CACHE` old for a stored
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
//!
//! Write-class reads of one key in one isolate are batched (`edge-read-batch`): a call with no read of its key in
//! flight starts one, and a call that arrives while one is in flight waits for the next read, which the first waiter
//! to see the in-flight read end starts, and every waiter shares it. A call never takes the answer of a read that
//! started before it arrived, so a write still reads fresh; an error is shared as the answer is, and a later call
//! starts a new read. Waiters poll plain data (read ids, start times, and the last answer per key) on their own timer,
//! as the lookup mark does (`graftyard_lookup`, `lookup-poll`), so no request holds or wakes another's future.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use graftyard_identity::READ_CACHE;
use graftyard_types::Timestamp;

/// How often a write-class call waiting on its key's batch looks at it again, on its own timer [guess].
pub(crate) const BATCH_POLL: Duration = Duration::from_millis(20);
/// A read in flight this long after it started is no longer waited on [guess]: its request may have been cancelled,
/// and a cancelled request's future never runs again or drops (`lookup-poll`). Past it, the next waiter starts a new
/// read that the others share. On m3fb32's burst one identity read took at most about 2 s on the edge (resolve's p99,
/// which spans two reads, was 2,051 ms) [measured, `tests/am1/results/lm1-rows-m3f.md`, "Rows m3fb32"].
pub(crate) const BATCH_STALE: Duration = Duration::from_millis(2_000);

/// One key's write-class batch: the read in flight (its id and start, ms) and the last answer, by read id. Plain data.
struct Flight<V> {
    running: Option<(u64, u64)>,
    done: Option<(u64, Result<Option<V>, String>)>,
}

/// What a write-class call does next with its key's batch.
enum Step<V> {
    /// A read that started after the call arrived has answered.
    Take(Result<Option<V>, String>),
    /// Start the next read, with this id.
    Start(u64),
    /// A read is in flight: look again after BATCH_POLL.
    Wait,
}

/// A map of answers, each stamped with the time its read started, and which answers it keeps, and each key's
/// write-class batch.
pub(crate) struct Ttl<K, V> {
    entries: BTreeMap<K, (Timestamp, V)>,
    cap: usize,
    /// Whether an answer may be stored: only one that lets a request through.
    keep: fn(&V) -> bool,
    flights: BTreeMap<K, Flight<V>>,
    /// The id of the last batched read started on this cache, any key.
    seq: u64,
}

impl<K: Ord, V: Clone> Ttl<K, V> {
    pub(crate) const fn new(cap: usize, keep: fn(&V) -> bool) -> Self {
        Ttl { entries: BTreeMap::new(), cap, keep, flights: BTreeMap::new(), seq: 0 }
    }

    /// The id the first read that starts after now will get: a call takes only answers of reads at or above it.
    fn next_id(&self) -> u64 {
        self.seq + 1
    }

    /// A write-class call's next step on `k`'s batch at `now`, for a call that needs a read with id `need` or later.
    fn step(&mut self, k: K, need: u64, now: u64) -> Step<V> {
        let stale = |at: u64| now >= at + BATCH_STALE.as_millis() as u64;
        if self.flights.len() >= self.cap && !self.flights.contains_key(&k) {
            // Only idle batches go: a waiter whose answer is dropped finds no read in flight and starts one.
            self.flights.retain(|_, f| f.running.is_some());
        }
        let f = self.flights.entry(k).or_insert(Flight { running: None, done: None });
        if let Some((id, r)) = &f.done
            && *id >= need
        {
            return Step::Take(r.clone());
        }
        match f.running {
            Some((_, at)) if !stale(at) => Step::Wait,
            _ => {
                self.seq += 1;
                f.running = Some((self.seq, now));
                Step::Start(self.seq)
            }
        }
    }

    /// Read `id` on `k` was dropped before it answered: its waiters start the next read at once.
    fn abandoned(&mut self, k: &K, id: u64) {
        if let Some(f) = self.flights.get_mut(k)
            && f.running.is_some_and(|(r, _)| r == id)
        {
            f.running = None;
        }
    }

    /// Read `id` on `k` answered `r`: its waiters take it, and the key has no read in flight unless a newer one began.
    fn ended(&mut self, k: K, id: u64, r: Result<Option<V>, String>) {
        let f = self.flights.entry(k).or_insert(Flight { running: None, done: None });
        if f.running.is_some_and(|(r, _)| r == id) {
            f.running = None;
        }
        if f.done.as_ref().is_none_or(|(d, _)| *d < id) {
            f.done = Some((id, r));
        }
    }

    /// The entry for `k` if the request is read-class and the entry is at most `READ_CACHE` old.
    pub(crate) fn get(&self, k: &K, now: Timestamp, read_class: bool) -> Option<V> {
        if !read_class {
            return None;
        }
        let ttl = u64::try_from(READ_CACHE.as_millis()).unwrap_or(u64::MAX);
        self.entries.get(k).filter(|(at, _)| now.0.saturating_sub(at.0) <= ttl).map(|(_, v)| v.clone())
    }

    /// The entry for `k` and when its read started, if it is at most `READ_CACHE` old: for a decision made on it that
    /// is cached in turn, which must age from the same moment (`crate::read_gate`).
    pub(crate) fn stamped(&self, k: &K, now: Timestamp) -> Option<(Timestamp, V)> {
        let at = self.entries.get(k)?.0;
        Some((at, self.get(k, now, true)?))
    }

    /// The entry for `k` at any age, for any request: a guess of which record to read, never an answer (`chained`).
    pub(crate) fn last(&self, k: &K) -> Option<V> {
        self.entries.get(k).map(|(_, v)| v.clone())
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

/// `cache`'s entry for `k` with its stamp, as `Ttl::stamped`.
pub(crate) fn stamped<K: Ord, V: Clone>(
    cache: &'static std::thread::LocalKey<RefCell<Ttl<K, V>>>,
    k: &K,
    now: Timestamp,
) -> Option<(Timestamp, V)> {
    cache.with(|c| c.borrow().stamped(k, now))
}

/// `cache`'s entry for `k` at any age, as `Ttl::last`.
pub(crate) fn last<K: Ord, V: Clone>(cache: &'static std::thread::LocalKey<RefCell<Ttl<K, V>>>, k: &K) -> Option<V> {
    cache.with(|c| c.borrow().last(k))
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
/// whose answer is stored stamped `at` (when the read started) if the cache keeps it. A write-class request reads
/// fresh through `k`'s batch (`batched`), and `t` counts it for `server-timing` (`rcread`, `rcshare`, `rcwait`).
pub(crate) async fn through<K: Ord + Clone + 'static, V: Clone + 'static, E: std::fmt::Display + From<String>>(
    cache: &'static std::thread::LocalKey<RefCell<Ttl<K, V>>>,
    k: K,
    at: Timestamp,
    read_class: bool,
    t: &crate::api::Timing,
    read: impl Future<Output = Result<Option<V>, E>>,
) -> Result<Option<V>, E> {
    if read_class {
        if let Some(v) = get(cache, &k, at, true) {
            return Ok(Some(v));
        }
        let v = read.await?;
        if let Some(v) = &v {
            put(cache, k, at, v.clone());
        }
        return Ok(v);
    }
    let b = batched(cache, k, at, read, || js_sys::Date::now() as u64, worker::Delay::from).await;
    t.rcache(b.shared, b.waited_ms);
    b.answer
}

/// A write-class call's answer from its key's batch, whether it was another call's read, and how long it waited
/// before its own read started or a shared answer came.
pub(crate) struct Batched<V, E> {
    pub(crate) answer: Result<Option<V>, E>,
    pub(crate) shared: bool,
    pub(crate) waited_ms: f64,
}

/// A write-class read of `k` through its batch, with the clock (ms) and timer passed in so a test drives many calls at
/// once. A call takes only the answer of a read whose id is at or above the next id when it arrived, so that read
/// started after it arrived: it waits out the read in flight, then shares the next read, which the first waiter to see
/// the key idle starts. So it waits at most for the read in flight and one more, each counted up to BATCH_STALE, plus
/// a BATCH_POLL per step. An error is shared as the answer is (as text), and the read ended, so the next call starts a
/// new one. The call that runs a read stores a kept answer stamped `at`, as before; a waiter stores nothing.
pub(crate) async fn batched<
    K: Ord + Clone + 'static,
    V: Clone + 'static,
    E: std::fmt::Display + From<String>,
    D: Future<Output = ()>,
>(
    cache: &'static std::thread::LocalKey<RefCell<Ttl<K, V>>>,
    k: K,
    at: Timestamp,
    read: impl Future<Output = Result<Option<V>, E>>,
    now: impl Fn() -> u64,
    delay: impl Fn(Duration) -> D,
) -> Batched<V, E> {
    let since = now();
    let need = cache.with(|c| c.borrow().next_id());
    let id = loop {
        match cache.with(|c| c.borrow_mut().step(k.clone(), need, now())) {
            Step::Take(r) => {
                let waited_ms = now().saturating_sub(since) as f64;
                return Batched { answer: r.map_err(E::from), shared: true, waited_ms };
            }
            Step::Start(id) => break id,
            Step::Wait => delay(BATCH_POLL).await,
        }
    };
    let waited_ms = now().saturating_sub(since) as f64;
    let running = Running { cache, k: Some(k.clone()), id };
    let answer = read.await;
    let shared = match &answer {
        Ok(v) => Ok(v.clone()),
        Err(e) => Err(e.to_string()),
    };
    running.end(shared);
    if let Ok(Some(v)) = &answer {
        put(cache, k, at, v.clone());
    }
    Batched { answer, shared: false, waited_ms }
}

/// A started batch read: answered through `end`, or, if its future is dropped first, abandoned so its waiters do not
/// wait out BATCH_STALE. A cancelled request never drops it, which BATCH_STALE bounds.
struct Running<K: Ord + Clone + 'static, V: Clone + 'static> {
    cache: &'static std::thread::LocalKey<RefCell<Ttl<K, V>>>,
    k: Option<K>,
    id: u64,
}

impl<K: Ord + Clone, V: Clone> Running<K, V> {
    fn end(mut self, r: Result<Option<V>, String>) {
        if let Some(k) = self.k.take() {
            self.cache.with(|c| c.borrow_mut().ended(k, self.id, r));
        }
    }
}

impl<K: Ord + Clone, V: Clone> Drop for Running<K, V> {
    fn drop(&mut self) {
        if let Some(k) = &self.k {
            self.cache.with(|c| c.borrow_mut().abandoned(k, self.id));
        }
    }
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
    use std::cell::Cell;
    use std::rc::Rc;

    use futures_util::FutureExt;

    use super::*;

    thread_local! {
        static T: RefCell<Ttl<u8, bool>> = const { RefCell::new(Ttl::new(8, not_revoked)) };
    }

    /// One `through` call on `T` that must finish at once, and whether it read. A write-class call goes through its
    /// batch with nothing else in flight, so it starts its own read at once.
    fn call(at: u64, read_class: bool, answer: Option<bool>) -> (Option<bool>, bool) {
        let mut read = false;
        let r = async {
            read = true;
            Ok::<_, String>(answer)
        };
        let out = if read_class {
            through(&T, 1, Timestamp(at), true, &crate::api::Timing::default(), r).now_or_never()
        } else {
            batched(&T, 1, Timestamp(at), r, || at, |_| std::future::ready(())).now_or_never().map(|b| b.answer)
        };
        (out.expect("ready").expect("ok"), read)
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

    thread_local! {
        static B: RefCell<Ttl<u8, u32>> = const { RefCell::new(Ttl::new(8, any)) };
    }

    /// A future ready once the shared clock reaches `at`.
    struct At(Rc<Cell<u64>>, u64);
    impl Future for At {
        type Output = ();
        fn poll(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<()> {
            if self.0.get() >= self.1 { std::task::Poll::Ready(()) } else { std::task::Poll::Pending }
        }
    }

    /// One write-class call in a run: when it arrives (ms) and, if it runs a read, how that read ends after `ms`.
    #[derive(Clone, Copy)]
    struct Call {
        arrives: u64,
        read_ms: Option<u64>,
        fails: bool,
    }

    /// A started call's future.
    type Pending = std::pin::Pin<Box<dyn Future<Output = Batched<u32, String>>>>;

    /// What a call got: the answer (the number of the read that made it, 1 first) or the error, whether it shared,
    /// and when it ended.
    type Got = (Result<Option<u32>, String>, bool, u64);

    /// Runs `calls` on `B`'s key 1 on one clock, one poll per ms, and returns what each got and the reads that ran.
    /// A call whose `read_ms` is `None` never answers if it runs its read (a request the runtime stopped polling).
    /// `drop_at` drops call 0's future at that ms, as a cancelled request whose future is dropped.
    fn run(calls: &[Call], drop_at: Option<u64>) -> (Vec<Option<Got>>, u32) {
        B.with(|b| *b.borrow_mut() = Ttl::new(8, any));
        let clock = Rc::new(Cell::new(0u64));
        let reads = Rc::new(Cell::new(0u32));
        let mut futs: Vec<Option<Pending>> = calls.iter().map(|_| None).collect();
        let mut got: Vec<Option<Got>> = calls.iter().map(|_| None).collect();
        for _ in 0..20_000 {
            let now = clock.get();
            if drop_at == Some(now) {
                futs[0] = None;
            }
            for (i, c) in calls.iter().enumerate() {
                if c.arrives == now && got[i].is_none() && !(i == 0 && drop_at.is_some_and(|d| d <= now)) {
                    let (cl, rd, call) = (clock.clone(), reads.clone(), *c);
                    let read = async move {
                        rd.set(rd.get() + 1);
                        let n = rd.get();
                        match call.read_ms {
                            Some(ms) => At(cl.clone(), cl.get() + ms).await,
                            None => std::future::pending::<()>().await,
                        }
                        if call.fails { Err(format!("read {n} failed")) } else { Ok(Some(n)) }
                    };
                    let (c1, c2) = (clock.clone(), clock.clone());
                    futs[i] = Some(Box::pin(batched(
                        &B,
                        1,
                        Timestamp(now),
                        read,
                        move || c1.get(),
                        move |d: Duration| At(c2.clone(), c2.get() + d.as_millis() as u64),
                    )));
                }
            }
            for (i, f) in futs.iter_mut().enumerate() {
                if let Some(fut) = f
                    && let Some(b) = fut.as_mut().now_or_never()
                {
                    got[i] = Some((b.answer, b.shared, clock.get()));
                    *f = None;
                }
            }
            let pending = calls.iter().enumerate().any(|(i, c)| {
                // A call whose own read never answers is not waited for; the others are.
                got[i].is_none()
                    && c.read_ms.is_some()
                    && !(i == 0 && drop_at.is_some())
                    && (c.arrives > now || futs[i].is_some())
            });
            if !pending {
                return (got, reads.get());
            }
            clock.set(now + 1);
        }
        panic!("a batched call never ended");
    }

    fn at(arrives: u64, read_ms: u64) -> Call {
        Call { arrives, read_ms: Some(read_ms), fails: false }
    }

    /// The batch's rule: a write-class call never takes the answer of a read that started before it arrived. Three
    /// calls that arrive while read 1 is in flight all get read 2, which one of them starts once read 1 ends, so two
    /// reads run for four calls, and each waiter waits at most the read in flight plus one read (plus the polls).
    #[test]
    fn calls_during_a_read_share_the_next_read_never_the_one_in_flight() {
        let calls = [at(0, 100), at(10, 100), at(20, 100), at(30, 100)];
        let (got, reads) = run(&calls, None);
        assert_eq!(reads, 2, "one read for the first call, one for the three that came during it");
        let g = |i: usize| got[i].clone().expect("answered");
        assert_eq!((g(0).0, g(0).1), (Ok(Some(1)), false), "the first call runs read 1");
        for i in 1..4 {
            assert_eq!(g(i).0, Ok(Some(2)), "call {i} arrived during read 1, so it never takes read 1's answer");
        }
        assert_eq!([g(1).1, g(2).1, g(3).1].iter().filter(|s| **s).count(), 2, "one waiter runs read 2, two share it");
        let bound = 100 + 100 + 2 * BATCH_POLL.as_millis() as u64;
        for (i, c) in calls.iter().enumerate().skip(1) {
            assert!(g(i).2 <= c.arrives + bound, "call {i} ended at {}, past {bound} after it came", g(i).2);
        }
        // A call that arrives after read 2 ended starts read 3: nothing is reused across batches.
        let (got, reads) = run(&[at(0, 100), at(10, 100), at(250, 50)], None);
        assert_eq!((reads, got[2].clone().expect("answered").0), (3, Ok(Some(3))));
    }

    /// An error of the shared read fails every call that shared it the same way, and the next call starts a fresh
    /// read rather than taking the error.
    #[test]
    fn a_shared_read_that_fails_fails_every_waiter_and_the_next_call_reads_again() {
        let fail = Call { arrives: 10, read_ms: Some(100), fails: true };
        let calls = [at(0, 100), fail, Call { arrives: 20, ..fail }, at(400, 50)];
        let (got, reads) = run(&calls, None);
        let g = |i: usize| got[i].clone().expect("answered");
        assert_eq!(g(0).0, Ok(Some(1)));
        // Calls 1 and 2 arrived during read 1; whichever starts read 2 sees it fail, and the other gets that error.
        assert_eq!((g(1).0, g(2).0), (Err("read 2 failed".into()), Err("read 2 failed".into())));
        assert_eq!((g(3).0, reads), (Ok(Some(3)), 3), "a later call reads again");
    }

    /// A read in flight whose request stopped is not waited on past BATCH_STALE, and one whose future was dropped is
    /// not waited on at all: the waiters start the next read.
    #[test]
    fn a_stuck_or_dropped_read_does_not_hold_its_waiters() {
        let stale = BATCH_STALE.as_millis() as u64;
        let stuck = Call { arrives: 0, read_ms: None, fails: false };
        let (got, reads) = run(&[stuck, at(10, 100), at(20, 100)], None);
        let g = |i: usize| got[i].clone().expect("answered");
        assert_eq!((g(1).0, g(2).0, reads), (Ok(Some(2)), Ok(Some(2)), 2));
        assert!(g(1).2 <= stale + 100 + 2 * BATCH_POLL.as_millis() as u64, "ended at {}", g(1).2);
        let (got, _) = run(&[at(0, 1_000), at(10, 100), at(20, 100)], Some(50));
        let g = |i: usize| got[i].clone().expect("answered");
        assert_eq!((g(1).0, g(2).0), (Ok(Some(2)), Ok(Some(2))));
        assert!(g(1).2 <= 50 + 100 + 2 * BATCH_POLL.as_millis() as u64, "a dropped read freed them: {}", g(1).2);
    }

    /// The call that runs a read stores a kept answer stamped with its own start, so read-class requests in the
    /// isolate still use it within READ_CACHE; a write-class call still never reads the entry.
    #[test]
    fn a_batched_read_refills_the_entry_for_read_class_requests() {
        run(&[at(5, 100)], None);
        assert_eq!(get(&B, &1, Timestamp(5), true), Some(1));
        assert_eq!(get(&B, &1, Timestamp(5), false), None);
    }
}
