//! What a leader host and its core hold for open tickets (`tests/m0/results/admitted-bytes.md`). A host keeps, per
//! admitted ticket, only what the marker puts read after an `Acked` (`RepoCmd::marked`), and drops it when the ticket
//! resolves, by the ticket it was issued. B0-33's deposed leaders kept 581 to 1,172 open tickets with their whole
//! commands for minutes, because a leadership ended by a higher term answered its tickets under that term (LM-1
//! round 19). Its own test binary, because it counts every allocation the thread makes.

mod common;
use common::*;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashMap;

use graftyard_git::pack::PackId;
use graftyard_raft::Ticket;
use graftyard_raft::admit::{AGENT_INLINE_ENTRY, ActorKey, Admit, AdmitClass, AdmitError};
use graftyard_types::SiteSet;

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

fn live() -> usize {
    LIVE.with(Cell::get)
}

const PACK: usize = 64 << 10;

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

#[test]
fn a_thousand_open_agent_commands_hold_one_copy_and_none_after_a_higher_term() {
    let mut w = World::new();
    let l = w.repo.leader();
    let now = w.now;
    let agent = |inline| Admit {
        class: AdmitClass::Agent,
        actor: Some(ActorKey(1 << 63 | 3)),
        push: true,
        inline,
        placement_ok: true,
        budget: None,
    };
    // The host's map: each ticket to what it keeps for the ticket's resolution.
    let mut host: HashMap<Ticket, Option<RepoCmd>> = HashMap::new();
    let base = live();
    let mut inline = 0;
    // No dependency read is answered, so nothing commits: every ticket stays open, as behind a stalled round. Half are
    // agent issue commands carrying their reply render, half agent pushes with 64 KB packs: B0-33's side mix.
    for i in 0..500u32 {
        let op = IssueOp::OpenIssue {
            title: format!("t{i}").into(),
            body: "".into(),
            labels: SmallVec::new(),
            assignees: SmallVec::new(),
        };
        let mut c = issue(app(CLAUDE), op);
        if let RepoCmd::Issue(ic) = &mut c {
            ic.render = render();
        }
        let rid = w.rid();
        let keep = c.marked();
        let a = Admit { push: false, ..agent(0) };
        host.insert(w.repo.on(l, now, |m, t| m.commit_as(t, rid, c, a)).expect("admitted").ticket, keep);
    }
    for i in 0..500u32 {
        let mut p = push(app(CLAUDE), vec![up(&format!("refs/heads/a{i}"), None, Some(oid(1_000 + i)))], &[]);
        p.pack = Some(PackRef::Inline(vec![7; PACK]));
        let rid = w.rid();
        let c = RepoCmd::Push(p.clone());
        let keep = c.marked();
        let t = match w.repo.on(l, now, |m, t| m.commit_as(t, rid, c, agent(PACK))) {
            Ok(a) => {
                inline += 1;
                a.ticket
            }
            // As the edge does: the pack is placed in the buckets and the push sent again under its request id.
            Err(AdmitError::Stage) => {
                p.pack = Some(PackRef::Stored {
                    id: PackId(Ulid(u128::from(i))),
                    checksum: Digest([1; 32]),
                    bytes: PACK as u64,
                    sites: SiteSet(3),
                });
                let c = RepoCmd::Push(p);
                let keep = c.marked();
                let t = w.repo.on(l, now, |m, t| m.commit_as(t, rid, c, agent(0))).expect("admitted").ticket;
                host.insert(t, keep);
                continue;
            }
            Err(e) => panic!("{e:?}"),
        };
        host.insert(t, keep);
    }
    let open = live() - base;
    let core = w.repo.members[l.0 as usize].admitted_bytes();
    println!("1,000 open agent commands ({inline} pushes inline): heap +{open} B, core admitted_bytes {core}");
    assert_eq!(host.len(), 1_000);
    // The core's copy is bounded: about 11 KB an issue command with its render, about 1 KB a staged push, and inline
    // pack bytes within AGENT_INLINE_ENTRY (admission stages the rest).
    let encoded = 500 * 11_000 + 500 * 1_000 + AGENT_INLINE_ENTRY;
    assert!(core <= encoded, "the core holds {core} B, over {encoded}");
    // As a heap that is at most 1.7 times its encoding (a render held as a `serde_json::Value` is about 1.5 times its
    // JSON). A host copy of every command beside the core's, as on main, takes it to about 3 times.
    let bound = encoded * 17 / 10;
    assert!(open <= bound, "1,000 open tickets hold {open} B of heap, over {bound}");

    // A rival takes a higher term without the leader, whose leadership then ends on that term.
    w.repo.inbox.clear();
    w.now = Timestamp(w.now.0 + graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1);
    let (b, now) = (SITES[(l.0 as usize + 1) % 3], w.now);
    w.repo.on(b, now, |m, t| m.campaign(t));
    for _ in 0..20 {
        for e in std::mem::take(&mut w.repo.inbox) {
            if e.to != l {
                w.repo.on(e.to, now, |m, t| m.step(t, e.from, e.msg));
            }
        }
    }
    assert_eq!(w.repo.members[b.0 as usize].role(), Role::Leader);
    // The rival's next round reaches the old leader too.
    let now = Timestamp(now.0 + graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
    w.repo.on(b, now, |m, t| m.tick(t));
    w.repo.deliver(now);
    assert_ne!(w.repo.members[l.0 as usize].role(), Role::Leader, "deposed");
    // The host drops what it kept as each ticket resolves, by the ticket it was issued.
    for r in std::mem::take(&mut w.repo.resolutions) {
        if let Resolution::Ticket { ticket, .. } = r {
            host.remove(&ticket);
        }
    }
    w.repo.deps.clear();
    assert!(host.is_empty(), "{} tickets of the lost term never resolved for the host", host.len());
    drop(host);
    let after = live().saturating_sub(base);
    println!("after the higher term: heap +{after} B");
    assert!(after <= open / 4, "the deposed leader still holds {after} B of its open tickets");
}
