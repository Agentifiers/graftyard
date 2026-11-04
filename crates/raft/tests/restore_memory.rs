//! Restore holds no `request` row in memory when the host keeps the table on its storage (`RequestStore`), and a
//! re-send is still answered from its row (BF-12). A member of `repo:196628` held 90,048 rows, and restoring it
//! grew its wasm memory from 91 MB to 174.8 MB, past its 128 MB isolate (`tests/m2/results/m2-wp1.md`, "The restore
//! defect"). Its own test binary, because it counts every allocation the thread makes.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use common::cluster::Cluster;
use common::kv::{KvCmd, KvEvent};
use common::sim::{SITES, config};
use graftyard_raft::{ByRequest, GroupFold, Index, Member, RequestId, RequestRow, RequestStore, Stored, Ulid};
use graftyard_types::Timestamp;

struct Counting;

thread_local!(static ALLOCATED: Cell<usize> = const { Cell::new(0) });

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCATED.try_with(|a| a.set(a.get() + l.size()));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn allocated() -> usize {
    ALLOCATED.with(Cell::get)
}

/// The host's table: rows on its storage, read one at a time. Clones share the table, as `RequestSql` handles share
/// the member's SQLite.
#[derive(Clone, Debug, Default)]
struct DiskRows(Rc<RefCell<BTreeMap<RequestId, RequestRow<KvEvent>>>>);

impl RequestStore<KvEvent> for DiskRows {
    fn get(&self, id: RequestId) -> Option<RequestRow<KvEvent>> {
        self.0.borrow().get(&id).cloned()
    }
    fn put(&mut self, id: RequestId, row: RequestRow<KvEvent>) -> bool {
        self.0.borrow_mut().insert(id, row).is_none()
    }
    fn expire(&mut self, cut: Timestamp) {
        self.0.borrow_mut().retain(|_, r| r.at >= cut);
    }
    fn rows(&self) -> Vec<(RequestId, RequestRow<KvEvent>)> {
        self.0.borrow().iter().map(|(k, v)| (*k, v.clone())).collect()
    }
    fn clone_box(&self) -> Box<dyn RequestStore<KvEvent>> {
        Box::new(self.clone())
    }
}

const ROWS: u128 = 90_048;
const B: graftyard_types::SiteId = SITES[1];

#[test]
fn restore_reads_no_request_row_and_a_resend_is_answered_from_storage() {
    let mut c = Cluster::new(41);
    c.elect(SITES[0]);
    let first = RequestId(Ulid(77));
    let cmd = KvCmd::Append { key: "k".into(), s: "x".into() };
    c.on(SITES[0], |m, now| m.commit(now, first, cmd.clone())).expect("admitted");
    c.deliver_all();
    c.sync(SITES[0]);
    let index = c.m(B).group_fold().request(first).expect("row on B").index;

    // B's disk with its `request` table on storage: its own rows plus the rest of two days of pushes, each about
    // the 93 bytes of events a `repo:196628` row held.
    let disk = c.disks[B.0 as usize].clone();
    let table = DiskRows::default();
    for (id, row) in disk.fold.requests() {
        table.0.borrow_mut().insert(id, row);
    }
    let at = c.now;
    for k in 0..ROWS {
        let events = vec![KvEvent::Push { key: format!("refs/heads/bf20/{k}"), s: "0".repeat(64) }];
        table.0.borrow_mut().insert(RequestId(Ulid(1_000_000 + k)), RequestRow { index: Index(2), at, events });
    }
    let g = &disk.fold;
    let fold = GroupFold::from_parts(g.fold.clone(), g.applied(), g.config().cloned(), g.id_block(), Box::new(table));
    let stored = Stored { state: disk.state.clone(), base: disk.base, log: disk.log.clone(), fold };

    let before = allocated();
    let (m, ready) = Member::restore(config(B), &stored, c.now, 7).expect("restore");
    let grew = allocated() - before;
    assert!(grew < 1_000_000, "restore allocated {grew} bytes for {ROWS} request rows on storage");

    // B leads, and both a re-send of the first request and one of a row only storage holds are answered from their
    // rows, with the original index, and nothing applies twice.
    c.members[B.0 as usize] = Some(m);
    let ((), out) = ready.persist(&mut c.disks[B.0 as usize]);
    c.inbox.extend(out.messages);
    c.crash(SITES[0]);
    c.elect_after_promises(B);
    match c.m(B).outcome_by_request(first, &cmd).expect("leads") {
        ByRequest::Done(a) => assert_eq!(a.index, index),
        _ => panic!("the first request's row is not found"),
    }
    let old = RequestId(Ulid(1_000_000 + ROWS - 1));
    let t = c.on(B, |m, now| m.commit(now, old, cmd.clone())).expect("admitted").ticket;
    assert_eq!(c.m(B).outcome(t).expect("answered at once").map(|a| a.index), Ok(Index(2)));
    c.deliver_all();
    assert_eq!(c.value(B, "k").as_deref(), Some("x"), "neither re-send applied again");
}
