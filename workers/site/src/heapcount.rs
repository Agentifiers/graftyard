//! The isolate's live heap bytes and their peak (`/stats` `memSplit` `heapLive` and `heapPeak`), counted by a wrapper
//! around the global allocator. wasm memory only grows, so `wasmMemMb` cannot tell a heap that holds more from one that
//! freed its bytes but keeps the pages: on B0-33 clA the leader climbed to 89.1 MiB while `open.high` stopped at 4.2 MiB
//! and every counted part of memSplit stayed under 3 MiB (`tests/am1/results/archive-memory.md`, "clA"). Live heap
//! that tracks `wasmMemMb` is a leak or an owner memSplit does not count; live heap flat under a rising `wasmMemMb` is
//! the allocator's fragmentation.
//!
//! A site isolate runs one thread, so on wasm32 the counts are plain statics. A native build (the lib's tests) runs
//! many threads, so there each thread counts its own allocations. `heapPeak` is the isolate's high-water mark, as
//! `wasmMemMb` is: no read resets it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// Live bytes and their peak.
pub struct Counts {
    live: Cell<usize>,
    peak: Cell<usize>,
}

impl Counts {
    const fn new() -> Counts {
        Counts { live: Cell::new(0), peak: Cell::new(0) }
    }

    fn grow(&self, n: usize) {
        let live = self.live.get() + n;
        self.live.set(live);
        self.peak.set(self.peak.get().max(live));
    }

    fn shrink(&self, n: usize) {
        self.live.set(self.live.get().saturating_sub(n));
    }

    /// A reallocation from `old` to `new` bytes: one block replaces another.
    fn moved(&self, old: usize, new: usize) {
        if new >= old { self.grow(new - old) } else { self.shrink(old - new) }
    }
}

#[cfg(target_arch = "wasm32")]
struct One(Counts);
// SAFETY: a site isolate's wasm instance runs a single thread; nothing else reads or writes these cells.
#[cfg(target_arch = "wasm32")]
unsafe impl Sync for One {}
#[cfg(target_arch = "wasm32")]
static COUNTS: One = One(Counts::new());

#[cfg(target_arch = "wasm32")]
fn with<R>(f: impl FnOnce(&Counts) -> R) -> Option<R> {
    Some(f(&COUNTS.0))
}

#[cfg(not(target_arch = "wasm32"))]
thread_local!(static COUNTS: Counts = const { Counts::new() });

#[cfg(not(target_arch = "wasm32"))]
fn with<R>(f: impl FnOnce(&Counts) -> R) -> Option<R> {
    COUNTS.try_with(f).ok()
}

/// The system allocator, counted.
pub struct Counting;

// SAFETY: every method forwards to `System` with the same arguments and only adds to or subtracts from the counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            with(|c| c.grow(l.size()));
        }
        p
    }

    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            with(|c| c.grow(l.size()));
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        with(|c| c.shrink(l.size()));
        unsafe { System.dealloc(p, l) }
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            with(|c| c.moved(l.size(), new));
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Live heap bytes now and their peak since the isolate started.
pub fn sample() -> (usize, usize) {
    with(|c| (c.live.get(), c.peak.get())).unwrap_or((0, 0))
}

#[cfg(test)]
mod tests {
    use super::sample;

    const MB: usize = 1 << 20;

    /// What the thread holds above `base`, rounded to whole MiB so the few small allocations between steps (the
    /// harness, `format!`) do not move it.
    fn held(base: usize) -> usize {
        (sample().0.saturating_sub(base) + MB / 2) / MB
    }

    /// An allocation counts while it lives and not after it is freed; a reallocation that grows a block counts the
    /// new size once, not the old and the new; one that shrinks it counts the smaller size; a zeroed allocation
    /// counts as any other.
    #[test]
    fn live_bytes_follow_allocations_reallocations_and_frees() {
        let base = sample().0;
        let mut v: Vec<u8> = Vec::with_capacity(3 * MB);
        assert_eq!(held(base), 3, "a 3 MiB allocation");
        v.reserve_exact(8 * MB);
        assert!(v.capacity() >= 8 * MB);
        assert_eq!(held(base), 8, "grown to 8 MiB by realloc: the 3 MiB block is replaced, not kept");
        v.shrink_to(2 * MB);
        assert_eq!(held(base), 2, "shrunk to 2 MiB");
        let z = vec![0u8; 5 * MB];
        assert_eq!(held(base), 7, "a zeroed 5 MiB allocation beside it");
        drop(v);
        drop(z);
        assert_eq!(held(base), 0, "both freed");
    }

    /// The peak is the most held at once: it outlives the allocation that set it, survives any number of reads, and
    /// rises only past itself.
    #[test]
    fn the_peak_holds_across_reads_and_rises_past_itself() {
        let base = sample().0;
        let big = vec![1u8; 16 * MB];
        let (_, at) = sample();
        drop(big);
        for _ in 0..3 {
            let (live, peak) = sample();
            assert_eq!(peak, at, "a read leaves the peak where the 16 MiB block set it");
            assert!(peak >= live + 15 * MB, "the peak outlives the block (live {live}, peak {peak})");
        }
        let a = vec![1u8; 10 * MB];
        let b = vec![1u8; 10 * MB];
        let (live, peak) = sample();
        assert!(peak >= base + 20 * MB && peak == live.max(at), "20 MiB at once raises it past 16");
        drop((a, b));
    }
}
