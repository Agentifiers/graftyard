//! A leader's resolved tickets do not hold the reply renders of thousands of issue commands (`Lead::done`,
//! DONE_BYTES and OUTCOME_KEEP). Each issue command carries its render, about 10 KB of the owner's Authz fold as hex
//! plus logins, and its outcome keeps a copy; kept by count alone, 4,096 of them held about 60 MB of the leader's heap
//! (`tests/m0/results/memory-budgets.md`). Its own test binary, because it counts every allocation the thread makes.

mod common;
use common::*;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use graftyard_git::issue_ops::IssueOp;
use graftyard_git::repo::RepoCmd;
use smallvec::SmallVec;

struct Counting;
thread_local!(static LIVE: Cell<usize> = const { Cell::new(0) });
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = LIVE.try_with(|v| v.set(v.get() + l.size()));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let _ = LIVE.try_with(|v| v.set(v.get().saturating_sub(l.size())));
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static A: Counting = Counting;

/// A render shaped like the edge's for m3org: about 10 KB of Authz hex and 18 logins (LM-1 round 15b).
fn render() -> serde_json::Value {
    let logins: serde_json::Map<String, serde_json::Value> =
        (0..18).map(|i| (format!("{}", 1000 + i), serde_json::json!(format!("user-{i:04}")))).collect();
    serde_json::json!({
        "host": "graft-m3c.example",
        "isolation": false,
        "dir": { "home": null, "authz": "ab".repeat(4_987), "logins": logins, "viewer": 101 },
        "what": { "Comment": { "n": 1 } },
    })
}

/// The live heap the run added: `n` issue commands committed 64 to an entry, every outcome dropped by the test.
fn added(with_render: bool, n: usize) -> usize {
    let mut w = World::new();
    let r = if with_render { render() } else { serde_json::Value::Null };
    let base = LIVE.with(Cell::get);
    for b in 0..n / 64 {
        let cmds: Vec<RepoCmd> = (0..64)
            .map(|i| {
                let op = IssueOp::OpenIssue {
                    title: format!("t{b}/{i}").into(),
                    body: "".into(),
                    labels: SmallVec::new(),
                    assignees: SmallVec::new(),
                };
                let mut c = issue(user(WRITER), op);
                if let RepoCmd::Issue(ic) = &mut c {
                    ic.render = r.clone();
                }
                c
            })
            .collect();
        assert!(w.commit_all(cmds).iter().all(Result::is_ok));
        // The test's own log of resolutions holds every outcome too; only the members' state counts here.
        w.repo.resolutions.clear();
    }
    LIVE.with(Cell::get).saturating_sub(base)
}

#[test]
fn thousands_of_rendered_issue_outcomes_hold_about_done_bytes_of_the_leaders_heap() {
    let (with, without) = (added(true, 8_192), added(false, 8_192));
    let renders = with.saturating_sub(without);
    println!("8,192 issue commands: +{with} B with renders, +{without} B without; renders hold {renders} B");
    // DONE_BYTES prices a render at its encoded JSON (about 10 KB); held as a `serde_json::Value` it is about 1.4 times
    // that, so the bound on the heap is DONE_BYTES times that overhead, not DONE_BYTES itself.
    let bound = graftyard_raft::DONE_BYTES * 3 / 2;
    assert!(renders <= bound, "the renders of resolved tickets hold {renders} B of the heap, over {bound}");
}
