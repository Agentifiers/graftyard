//! The request and answer body bytes a member holds open, and their peak beside the peak of requests open at once
//! (`/stats` `memSplit` `open`). memSplit counted what a member keeps (log, fold caches, held messages, calls out,
//! streamed answers) but not what each request holds while its handler runs: its body and the answer it builds. On
//! B0-33 lpB the leader's wasm memory followed the running peak of open requests (r 0.90 over 391 samples, 16 to 753
//! open) while every counted part stayed flat (`tests/am1/results/archive-memory.md`, "The per-invocation read").
//!
//! An answer that streams after its handler returns (a read's `held_body`) is not counted here: `inFlight.answers`
//! counts it. wasm memory only grows, so it is a high-water mark; the peaks here are too, from the last
//! `/stats?reset=1`, so a row's samples compare them. A plain sample never resets them.

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;

#[derive(Default)]
pub struct OpenBytes {
    bytes: Cell<usize>,
    high: Cell<usize>,
    open: Cell<u32>,
    open_high: Cell<u32>,
}

/// One sample: bytes held now and at most since the last reset, and requests open now and at most.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub bytes: usize,
    pub high: usize,
    pub open: u32,
    pub open_high: u32,
}

impl OpenBytes {
    /// Counts one request holding `body` bytes until the returned hold drops, by any path: the handler's answer, an
    /// error, or its future dropped when the caller goes away.
    pub fn enter(this: &Rc<OpenBytes>, body: usize) -> Open {
        this.open.set(this.open.get() + 1);
        this.open_high.set(this.open_high.get().max(this.open.get()));
        this.bytes.set(this.bytes.get() + body);
        this.mark();
        Open { of: this.clone(), bytes: body }
    }

    fn mark(&self) {
        self.high.set(self.high.get().max(self.bytes.get()));
    }

    pub fn sample(&self) -> Sample {
        Sample {
            bytes: self.bytes.get(),
            high: self.high.get(),
            open: self.open.get(),
            open_high: self.open_high.get(),
        }
    }

    /// `/stats?reset=1`: the peaks start again from what is held now.
    pub fn reset(&self) {
        self.high.set(self.bytes.get());
        self.open_high.set(self.open.get());
    }
}

/// A request's hold on its counted bytes.
pub struct Open {
    of: Rc<OpenBytes>,
    bytes: usize,
}

impl Open {
    /// Adds the handler's answer body, held until this hold drops.
    pub fn answer(&mut self, n: usize) {
        self.bytes += n;
        self.of.bytes.set(self.of.bytes.get() + n);
        self.of.mark();
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        self.of.bytes.set(self.of.bytes.get().saturating_sub(self.bytes));
        self.of.open.set(self.of.open.get().saturating_sub(1));
    }
}

/// Runs a request's handler `f` counted: `body` bytes from the start, and its answer (`answer_len` of an `Ok`) until
/// it returns. An `Err` or a dropped future releases the count as an answer does.
pub async fn held<T, E>(
    of: &Rc<OpenBytes>,
    body: usize,
    answer_len: impl FnOnce(&T) -> usize,
    f: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let mut open = OpenBytes::enter(of, body);
    let r = f.await;
    if let Ok(v) = &r {
        open.answer(answer_len(v));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;

    async fn ok(n: usize) -> Result<Vec<u8>, String> {
        Ok(vec![0; n])
    }

    /// Every way a handler ends releases its bytes and its open count: an answer, an error, and a future dropped
    /// mid-await (the caller went away). The answer is counted while the handler still holds it.
    #[test]
    fn a_request_releases_its_bytes_however_it_ends() {
        let ob = Rc::new(OpenBytes::default());
        let r = held(&ob, 1_000, |v: &Vec<u8>| v.len(), ok(5_000)).now_or_never().unwrap();
        assert_eq!(r.unwrap().len(), 5_000);
        assert_eq!((ob.sample().bytes, ob.sample().open), (0, 0), "released after an answer");
        assert_eq!(ob.sample().high, 6_000, "the body and the answer were held together");

        let r = held(&ob, 700, |v: &Vec<u8>| v.len(), async { Err::<Vec<u8>, _>("refused".to_owned()) })
            .now_or_never()
            .unwrap();
        assert!(r.is_err());
        assert_eq!((ob.sample().bytes, ob.sample().open), (0, 0), "released after an error");

        let mut pending = Box::pin(held(
            &ob,
            3_000,
            |v: &Vec<u8>| v.len(),
            futures_util::future::pending::<Result<Vec<u8>, String>>(),
        ));
        assert!(pending.as_mut().now_or_never().is_none());
        assert_eq!((ob.sample().bytes, ob.sample().open), (3_000, 1), "held while the handler waits");
        drop(pending);
        assert_eq!((ob.sample().bytes, ob.sample().open), (0, 0), "released when the caller goes away");
    }

    /// Requests open together add up, and the peaks hold across plain samples once they end; only a reset moves them,
    /// and then to what is held at the reset, never below it. A later, larger moment raises them again.
    #[test]
    fn the_peaks_hold_until_a_reset_and_restart_from_what_is_held() {
        let ob = Rc::new(OpenBytes::default());
        let a = OpenBytes::enter(&ob, 4_000);
        let mut b = OpenBytes::enter(&ob, 2_000);
        b.answer(10_000);
        assert_eq!(ob.sample(), Sample { bytes: 16_000, high: 16_000, open: 2, open_high: 2 });
        drop(b);
        for _ in 0..3 {
            assert_eq!(
                ob.sample(),
                Sample { bytes: 4_000, high: 16_000, open: 1, open_high: 2 },
                "a sample resets nothing"
            );
        }
        ob.reset();
        assert_eq!(ob.sample(), Sample { bytes: 4_000, high: 4_000, open: 1, open_high: 1 }, "a reset starts from now");
        drop(a);
        assert_eq!(ob.sample().high, 4_000, "the peak outlives the request that set it");
        let c = OpenBytes::enter(&ob, 9_000);
        assert_eq!(ob.sample(), Sample { bytes: 9_000, high: 9_000, open: 1, open_high: 1 });
        drop(c);
    }
}
