//! Test installs only: what each incoming invocation of a member object caused, logged inside it, so Workers Logs'
//! per-invocation `cpuTimeMs` can be split by cause (narrow-reads-3, option (b) after the gate 16 A/B).
//!
//! The working assumption [inference] is that the runtime charges work to the request whose context it runs in, and
//! that a promise's continuation runs in the context that started the I/O: so a learner's apply of the entries riding
//! the answer to its `ReadIndex` call is charged to the `/read` that sent the call, even after other requests began
//! (on arm "with", 116 of l3's 147 `/read` invocations of 50 ms or more spent more CPU than the time to the next
//! arrival). The run checks it: CPU should follow these counts per invocation. This module tags work the same way:
//! `run` tags an incoming invocation's future, `spawn` tags a task with the invocation that spawned it, and `add`
//! counts on the tag of the task being polled. Each invocation logs one `{"inv": path}` line as it returns and,
//! for work its spawned tasks did later, one `{"tail": id}` line from inside each such task as it ends; Workers Logs
//! files both under the invocation's `requestId`. Untagged work (the ticker, alarms) is counted in `orphan`.
//! Product builds keep only the plain `spawn_local` and futures.

use std::future::Future;

#[cfg(feature = "test-faults")]
use std::cell::{Cell, RefCell};
#[cfg(feature = "test-faults")]
use std::pin::Pin;
#[cfg(feature = "test-faults")]
use std::task::{Context, Poll};

/// What one invocation caused.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct Counts {
    /// Raft inputs stepped (`Host::step`).
    pub steps: u32,
    /// Entries applied, and the events they carried.
    pub applied: u32,
    pub events: u32,
    /// Bytes of member-call bodies decoded: `/raft` requests taken and call responses read.
    pub decoded: u64,
    /// Statements committed (`Host::flush`), and their bytes.
    pub rows: u32,
    pub row_bytes: u64,
    /// Storage confirms that ended in a task of this invocation, and their ms.
    pub confirms: u32,
    pub confirm_ms: u64,
    /// SQL reads of fold rows (`TierSql::query`).
    pub sql_reads: u32,
    /// Answers built: whole-tier dumps, narrow REST, narrow GraphQL; and cached answers served.
    pub dumps: u32,
    pub rest: u32,
    pub gql: u32,
    pub hits: u32,
}

#[cfg(feature = "test-faults")]
#[derive(Default)]
struct Isolate {
    next: u64,
    counts: std::collections::HashMap<u64, Counts>,
    orphan: Counts,
    /// Starts of the storage confirms in flight, oldest first.
    syncs: std::collections::VecDeque<u64>,
}

#[cfg(feature = "test-faults")]
thread_local! {
    static CUR: Cell<u64> = const { Cell::new(0) };
    static INV: RefCell<Isolate> = RefCell::new(Isolate::default());
}

/// Counts work on the invocation of the task being polled, or on `orphan`.
pub fn add(_f: impl FnOnce(&mut Counts)) {
    #[cfg(feature = "test-faults")]
    INV.with(|i| {
        let mut i = i.borrow_mut();
        match CUR.get() {
            0 => _f(&mut i.orphan),
            id => _f(i.counts.entry(id).or_default()),
        }
    });
}

/// A storage confirm began.
pub fn sync_began() {
    #[cfg(feature = "test-faults")]
    INV.with(|i| i.borrow_mut().syncs.push_back(crate::now_ms()));
}

/// The oldest storage confirm in flight ended, after `ms`; confirms end in order.
pub fn sync_ended(_ms: u64) {
    #[cfg(feature = "test-faults")]
    {
        INV.with(|i| i.borrow_mut().syncs.pop_front());
        add(|c| {
            c.confirms += 1;
            c.confirm_ms += _ms;
        });
    }
}

#[cfg(feature = "test-faults")]
fn pending_sync_ms() -> u64 {
    INV.with(|i| i.borrow().syncs.front().map_or(0, |t| crate::now_ms().saturating_sub(*t)))
}

#[cfg(feature = "test-faults")]
fn take(id: u64) -> Option<Counts> {
    INV.with(|i| i.borrow_mut().counts.remove(&id))
}

/// A future polled with `id` as the current invocation.
#[cfg(feature = "test-faults")]
struct Tagged<'a, T> {
    id: u64,
    f: Pin<Box<dyn Future<Output = T> + 'a>>,
}

#[cfg(feature = "test-faults")]
impl<T> Future for Tagged<'_, T> {
    type Output = T;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let prev = CUR.replace(self.id);
        let r = self.f.as_mut().poll(cx);
        CUR.set(prev);
        r
    }
}

/// Runs an incoming invocation of `path`, tagged, and logs what it caused as it returns.
pub async fn run<T>(_path: &str, f: impl Future<Output = T>) -> T {
    #[cfg(feature = "test-faults")]
    {
        let id = INV.with(|i| {
            let mut i = i.borrow_mut();
            i.next += 1;
            i.next
        });
        let (start, pend) = (crate::now_ms(), pending_sync_ms());
        let r = Tagged { id, f: Box::pin(f) }.await;
        let c = take(id).unwrap_or_default();
        let orphan = INV.with(|i| i.borrow().orphan);
        worker::console_log!(
            "{}",
            serde_json::json!({ "inv": _path, "id": id, "start": start, "end": crate::now_ms(), "pendStart": pend,
                "pendEnd": pending_sync_ms(), "c": c, "orphan": orphan })
        );
        r
    }
    #[cfg(not(feature = "test-faults"))]
    f.await
}

/// `spawn_local`, with the task tagged as the invocation that spawned it. When it ends it logs, from inside that
/// invocation's context, what it caused since the invocation's last line.
pub fn spawn(f: impl Future<Output = ()> + 'static) {
    #[cfg(feature = "test-faults")]
    {
        let id = CUR.get();
        worker::wasm_bindgen_futures::spawn_local(Tagged {
            id,
            f: Box::pin(async move {
                f.await;
                if id != 0
                    && let Some(c) = take(id)
                    && c != Counts::default()
                {
                    worker::console_log!("{}", serde_json::json!({ "tail": id, "c": c }));
                }
            }),
        });
    }
    #[cfg(not(feature = "test-faults"))]
    worker::wasm_bindgen_futures::spawn_local(f);
}

/// `spawn_local` for a task that outlives every invocation (the ticker): its work is counted in `orphan`.
pub fn detached(f: impl Future<Output = ()> + 'static) {
    #[cfg(feature = "test-faults")]
    worker::wasm_bindgen_futures::spawn_local(Tagged { id: 0, f: Box::pin(f) });
    #[cfg(not(feature = "test-faults"))]
    worker::wasm_bindgen_futures::spawn_local(f);
}
