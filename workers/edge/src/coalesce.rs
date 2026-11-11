use std::cell::RefCell;
use std::collections::{HashMap, hash_map::Entry};
use std::future::Future;

use futures_channel::oneshot;

pub(crate) type Shared = std::result::Result<Option<serde_json::Value>, String>;

enum Turn {
    Lead(Vec<oneshot::Sender<Turn>>),
    Done(Shared),
}

thread_local! {
    /// A present key has a call in flight; `Some` holds the callers waiting for the next round.
    static ROUNDS: RefCell<HashMap<String, Option<Vec<oneshot::Sender<Turn>>>>> = RefCell::new(HashMap::new());
}

/// Runs one round and hands off even if the client drops its future while the call is in flight.
struct Leader {
    key: String,
    followers: Vec<oneshot::Sender<Turn>>,
    armed: bool,
}

impl Leader {
    fn finish(mut self, value: Shared) -> Shared {
        for follower in self.followers.drain(..) {
            let _ = follower.send(Turn::Done(value.clone()));
        }
        hand_off(&self.key);
        self.armed = false;
        value
    }
}

impl Drop for Leader {
    fn drop(&mut self) {
        if self.armed {
            hand_off(&self.key);
        }
    }
}

/// Coalesces identical calls in one isolate without weakening `fresh`: a caller shares only a call sent after it
/// arrived, never the call already in flight, so each caller either leads or waits for the next round. Whole-tier
/// reads of about 150 KB to 1.2 MB each were the leader's memory and CPU at 500 agents
/// (`tests/am1/results/lm1-memory.md`).
pub(crate) async fn coalesce<F: Future<Output = Shared>>(key: &str, call: impl FnOnce() -> F) -> Shared {
    let (tx, rx) = oneshot::channel();
    let waiting = ROUNDS.with(|rounds| {
        let mut rounds = rounds.borrow_mut();
        match rounds.entry(key.to_owned()) {
            Entry::Vacant(entry) => {
                entry.insert(None);
                false
            }
            Entry::Occupied(entry) => {
                let round = entry.into_mut();
                if let Some(waiters) = round {
                    waiters.push(tx);
                } else {
                    *round = Some(vec![tx]);
                }
                true
            }
        }
    });
    if !waiting {
        return lead(key, Vec::new(), call).await;
    }
    match rx.await {
        Ok(Turn::Lead(followers)) => lead(key, followers, call).await,
        Ok(Turn::Done(value)) => value,
        // The leader went away with its client. This caller still needs its own fresh call, but must not re-enter the
        // round that its canceled leader left behind.
        Err(_) => call().await,
    }
}

async fn lead<F: Future<Output = Shared>>(
    key: &str,
    followers: Vec<oneshot::Sender<Turn>>,
    call: impl FnOnce() -> F,
) -> Shared {
    let leader = Leader { key: key.to_owned(), followers, armed: true };
    let value = call().await;
    leader.finish(value)
}

/// Gives the next round to its first surviving caller. A dead receiver returns its `Lead` message, whose followers
/// are retained for the next attempt.
fn hand_off(key: &str) {
    ROUNDS.with(|rounds| {
        let mut rounds = rounds.borrow_mut();
        let Some(mut waiting) = rounds.remove(key).flatten() else { return };
        loop {
            let mut waiters = waiting.into_iter();
            let Some(next) = waiters.next() else { return };
            let rest = waiters.collect();
            match next.send(Turn::Lead(rest)) {
                Ok(()) => {
                    rounds.insert(key.to_owned(), None);
                    return;
                }
                Err(Turn::Lead(rest)) => waiting = rest,
                Err(Turn::Done(_)) => unreachable!("hand-off sends only leadership"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use futures_channel::oneshot;
    use serde_json::Value;

    use super::{Shared, coalesce};

    fn answer(value: &str) -> Shared {
        Ok(Some(Value::String(value.to_owned())))
    }

    async fn read(started: &Cell<u32>, gate: oneshot::Receiver<&'static str>) -> Shared {
        started.set(started.get() + 1);
        answer(gate.await.unwrap())
    }

    #[test]
    fn a_read_never_joins_a_call_sent_before_it_arrived() {
        let started = Cell::new(0);
        let (a_tx, a_rx) = oneshot::channel();
        let (b_tx, b_rx) = oneshot::channel();
        let (_c_tx, c_rx) = oneshot::channel();
        let mut a = pin!(coalesce("coalesce/fresh", || read(&started, a_rx)));
        let mut b = pin!(coalesce("coalesce/fresh", || read(&started, b_rx)));
        let mut c = pin!(coalesce("coalesce/fresh", || read(&started, c_rx)));
        let mut cx = Context::from_waker(Waker::noop());

        assert_eq!(a.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(started.get(), 1);
        assert_eq!(b.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(c.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(started.get(), 1);

        a_tx.send("one").unwrap();
        assert_eq!(a.as_mut().poll(&mut cx), Poll::Ready(answer("one")));
        assert_eq!(b.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(started.get(), 2, "B leads the next call rather than taking A's answer");
        assert_eq!(c.as_mut().poll(&mut cx), Poll::Pending);

        b_tx.send("two").unwrap();
        assert_eq!(b.as_mut().poll(&mut cx), Poll::Ready(answer("two")));
        assert_eq!(c.as_mut().poll(&mut cx), Poll::Ready(answer("two")));
        assert_eq!(started.get(), 2, "three reads made two calls");
    }

    #[test]
    fn a_dropped_leader_hands_off() {
        let started = Cell::new(0);
        let (_a_tx, a_rx) = oneshot::channel();
        let (b_tx, b_rx) = oneshot::channel();
        let mut b = pin!(coalesce("coalesce/dropped", || read(&started, b_rx)));
        let mut cx = Context::from_waker(Waker::noop());

        {
            let mut a = pin!(coalesce("coalesce/dropped", || read(&started, a_rx)));
            assert_eq!(a.as_mut().poll(&mut cx), Poll::Pending);
            assert_eq!(b.as_mut().poll(&mut cx), Poll::Pending);
            assert_eq!(started.get(), 1);
        }
        assert_eq!(b.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(started.get(), 2, "B became leader when A was dropped");

        b_tx.send("two").unwrap();
        assert_eq!(b.as_mut().poll(&mut cx), Poll::Ready(answer("two")));

        let (d_tx, d_rx) = oneshot::channel();
        let mut d = pin!(coalesce("coalesce/dropped", || read(&started, d_rx)));
        assert_eq!(d.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(started.get(), 3, "the completed hand-off left no call in flight");
        d_tx.send("four").unwrap();
        assert_eq!(d.as_mut().poll(&mut cx), Poll::Ready(answer("four")));
    }

    #[test]
    fn an_idle_key_reads_at_once() {
        let started = Cell::new(0);
        let (a_tx, a_rx) = oneshot::channel();
        let (b_tx, b_rx) = oneshot::channel();
        let mut cx = Context::from_waker(Waker::noop());

        let mut a = pin!(coalesce("coalesce/idle", || read(&started, a_rx)));
        assert_eq!(a.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(started.get(), 1);
        a_tx.send("one").unwrap();
        assert_eq!(a.as_mut().poll(&mut cx), Poll::Ready(answer("one")));

        let mut b = pin!(coalesce("coalesce/idle", || read(&started, b_rx)));
        assert_eq!(b.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(started.get(), 2, "the first read removed its key");
        b_tx.send("two").unwrap();
        assert_eq!(b.as_mut().poll(&mut cx), Poll::Ready(answer("two")));
    }
}
