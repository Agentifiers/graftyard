//! The controller's `TaskBox` rules in memory (`crates/git/src/taskbox.rs`): LM-3's kill row with its three kill
//! points, late reports from an old Sandbox, the version restart, and acked-push detection from a receive-pack
//! response.

use graftyard_git::controller::{
    RepoRef, Resume, SandboxId, StepErr, TASK_WALL, Task, TaskBudget, TaskEvent, TaskState,
};
use graftyard_git::pkt::{self, Band};
use graftyard_git::refs::ObjectId;
use graftyard_git::task::run_refs;
use graftyard_git::taskbox::{Act, HEARTBEAT, Input, Pointer, Report, SILENT_MAX, TaskBox, acked_pushes};
use graftyard_identity::{Login, RepoName, SessionId, UserId, WorkspaceMode};
use graftyard_types::{AccountId, DbId, Index};

const RUN: u128 = 0x0e6a_1b2c_0000_4000_8000_0000_0000_0001;
const ID: &str = "0e6a1b2c-0000-4000-8000-000000000001";

fn task() -> Task {
    let base = RepoRef { owner: Login::parse("acme").unwrap(), name: RepoName::parse("app").unwrap() };
    let budget = TaskBudget { task_tokens: 1_000, org_month_tokens: 50_000, wall: TASK_WALL };
    Task::new(
        SessionId::Run(RUN),
        None,
        Some(Index(4)),
        UserId(AccountId(DbId(7))),
        base,
        WorkspaceMode::InRepo,
        budget,
    )
    .unwrap()
}

fn sb(n: u32) -> SandboxId {
    SandboxId(format!("{ID}-{n}").into())
}

fn alarm(running: bool, version: &str) -> Input {
    Input::Alarm { running, version: version.into(), pointer: Pointer::Clear }
}

fn pinned(running: bool, version: &str, pointer: &str) -> Input {
    Input::Alarm { running, version: version.into(), pointer: Pointer::To(pointer.into()) }
}

fn pushed(from: SandboxId, head: ObjectId) -> Input {
    Input::Report { from, event: TaskEvent::Pushed { head } }
}

/// Starts a box on version `v1` at t = 0.
fn started() -> TaskBox {
    let mut b = TaskBox::new(task());
    assert_eq!(b.start("v1", 0), Ok(Act::Start { sandbox: sb(1), from: Resume::Base }));
    b
}

/// LM-3's kill row, its three kill points: before the first push the next Sandbox starts from the base; between
/// pushes from the last acked one; during a push whose `ok` never came back from the last acked one before it.
#[test]
fn a_killed_sandbox_restarts_from_its_last_acked_push_at_each_kill_point() {
    let (h1, h2) = (ObjectId([1; 20]), ObjectId([2; 20]));
    let hb = HEARTBEAT.as_millis() as u64;

    // Before the first push.
    let mut b = started();
    assert_eq!(b.on(alarm(true, "v1"), hb), Ok(Act::Renew), "a running Sandbox only renews its timeout");
    assert_eq!(b.on(alarm(false, "v1"), 2 * hb), Ok(Act::Start { sandbox: sb(2), from: Resume::Base }));
    assert_eq!(b.task.state, TaskState::Running { sandbox: sb(2), last_push: None });

    // Between pushes: h1 acked, then killed.
    let mut b = started();
    b.on(pushed(sb(1), h1), 10).unwrap();
    assert_eq!(b.on(alarm(false, "v1"), hb), Ok(Act::Start { sandbox: sb(2), from: Resume::Push(h1) }));

    // During a push: h1 acked, h2 sent but never acked (the router saw no `ok`, so no report came), then killed.
    let mut b = started();
    b.on(pushed(sb(1), h1), 10).unwrap();
    assert_eq!(b.on(alarm(false, "v1"), hb), Ok(Act::Start { sandbox: sb(2), from: Resume::Push(h1) }));
    // The new Sandbox goes on from h1 and pushes h2 itself.
    b.on(pushed(sb(2), h2), hb + 10).unwrap();
    assert_eq!(b.task.resume(), Some(Resume::Push(h2)));
    assert_eq!(b.generation, 2, "one Sandbox per generation, never two at once");
}

/// Every report from a Sandbox the task no longer holds is refused, counted, and moves nothing: a late push, a late
/// usage report, a late finish, and a report from a Sandbox that never existed.
#[test]
fn every_late_report_from_an_old_sandbox_is_refused_and_counted() {
    let (h1, h9) = (ObjectId([1; 20]), ObjectId([9; 20]));
    let mut b = started();
    b.on(pushed(sb(1), h1), 10).unwrap();
    b.on(alarm(false, "v1"), 20).unwrap();
    let before = b.task.clone();
    for (i, ev) in [TaskEvent::Pushed { head: h9 }, TaskEvent::Finished, TaskEvent::Failed { reason: "x".into() }]
        .into_iter()
        .enumerate()
    {
        assert_eq!(b.on(Input::Report { from: sb(1), event: ev }, 30), Err(Report::Late { from: sb(1) }));
        assert_eq!(b.late, i as u64 + 1);
    }
    assert_eq!(b.on(pushed(sb(7), h9), 30), Err(Report::Late { from: sb(7) }));
    assert_eq!((b.task.clone(), b.late), (before, 4), "nothing moved, all four counted");
    // The current Sandbox's report is taken.
    b.on(pushed(sb(2), h9), 40).unwrap();
    assert_eq!(b.task.resume(), Some(Resume::Push(h9)));
    // A queued task holds no Sandbox, so any report is late.
    let mut q = TaskBox::new(task());
    assert_eq!(q.on(pushed(sb(1), h1), 0), Err(Report::Late { from: sb(1) }));
    // A current Sandbox's report the task refuses is refused, not counted late.
    let mut d = started();
    d.on(Input::Report { from: sb(1), event: TaskEvent::Finished }, 5).unwrap();
    assert_eq!(d.task.state, TaskState::Done);
    assert_eq!(
        d.on(Input::Report { from: sb(1), event: TaskEvent::Finished }, 6),
        Err(Report::Refused(StepErr::Final))
    );
    assert_eq!(d.late, 0);
}

/// The alarm moves a task's Sandbox onto the version the object runs now, so a roll and its swap-back both reach a
/// running task by its next alarm; a silent Sandbox is lost; a final task stops its container and starts none.
#[test]
fn the_alarm_restarts_on_a_new_version_or_a_silent_sandbox_and_a_final_task_stops() {
    let h1 = ObjectId([1; 20]);
    let mut b = started();
    b.on(pushed(sb(1), h1), 10).unwrap();
    // The object now runs v2 (a roll assigned it): restart there from the acked push.
    assert_eq!(b.on(alarm(true, "v2"), 20), Ok(Act::Start { sandbox: sb(2), from: Resume::Push(h1) }));
    assert_eq!(b.version.as_deref(), Some("v2"));
    assert_eq!(b.on(alarm(true, "v2"), 30), Ok(Act::Renew));
    // Swapped back to v1: again at the next alarm.
    assert_eq!(b.on(alarm(true, "v1"), 40), Ok(Act::Start { sandbox: sb(3), from: Resume::Push(h1) }));
    // Silent for SILENT_MAX while its container runs: lost.
    let quiet = 40 + SILENT_MAX.as_millis() as u64;
    assert_eq!(b.on(alarm(true, "v1"), quiet - 1), Ok(Act::Renew));
    assert_eq!(b.on(alarm(true, "v1"), quiet), Ok(Act::Start { sandbox: sb(4), from: Resume::Push(h1) }));
    // Final: the container stops and no alarm starts another.
    b.on(Input::Report { from: sb(4), event: TaskEvent::Cancelled }, quiet + 1).unwrap();
    assert_eq!(b.on(alarm(true, "v2"), quiet + 2), Ok(Act::Stop));
    assert_eq!(b.on(alarm(false, "v2"), quiet + 3), Ok(Act::Keep));
    // A queued task's alarm starts nothing; only `start` does, and only once.
    let mut q = TaskBox::new(task());
    assert_eq!(q.on(alarm(false, "v1"), 0), Ok(Act::Keep));
    q.start("v1", 0).unwrap();
    assert_eq!(q.start("v1", 1), Err(StepErr::NotNow));
    assert_eq!(q.generation, 1, "a refused start takes no generation");
}

/// A `ControllerPointer` deploys nothing, so a box on N+1 stays on N+1 until the agents `graft-ops` deploys N: an alarm
/// that sees a pointer to N holds the box (its Sandbox stopped, every report and request late) for as long as the
/// object still runs N+1, and the first alarm without such a pointer (the object on N, or the pointer gone) starts the
/// next Sandbox from the last acked push. A pointer to the version the box runs is no hold, a queued task takes none,
/// and a hold past `HOLD_MAX` fails the task.
#[test]
fn a_pointer_to_another_version_holds_the_box_until_its_object_runs_that_version() {
    use graftyard_git::taskbox::HOLD_MAX;
    let h1 = ObjectId([1; 20]);
    let mut b = TaskBox::new(task());
    b.start("n1", 0).unwrap();
    b.on(pushed(sb(1), h1), 10).unwrap();
    assert_eq!(b.on(pinned(true, "n1", "n1"), 15), Ok(Act::Renew), "a pointer to the version the box runs is no hold");
    assert_eq!(b.on(pinned(true, "n1", "n"), 20), Ok(Act::Stop));
    assert_eq!(b.hold, Some(("n".into(), 20)));
    // Held: the old Sandbox's late push and its outbound traffic are refused and counted; alarms on N+1 keep it down.
    assert_eq!(b.on(pushed(sb(1), ObjectId([9; 20])), 25), Err(Report::Late { from: sb(1) }));
    assert_eq!(b.admit(&sb(1), 26), Err(Report::Late { from: sb(1) }));
    assert_eq!(b.late, 2);
    assert_eq!(b.on(pinned(true, "n1", "n"), 30), Ok(Act::Stop));
    assert_eq!(b.on(pinned(false, "n1", "n"), 40), Ok(Act::Keep));
    assert_eq!(b.hold, Some(("n".into(), 20)), "the hold keeps its first time");
    // A failed read of the view on N+1 releases nothing.
    let unread = |running, version: &str| Input::Alarm { running, version: version.into(), pointer: Pointer::Unread };
    assert_eq!(b.on(unread(false, "n1"), 45), Ok(Act::Keep));
    assert_eq!(b.hold, Some(("n".into(), 20)));
    // The ops deployed N: the first alarm there starts the next Sandbox from the acked push.
    assert_eq!(b.on(pinned(false, "n", "n"), 50), Ok(Act::Start { sandbox: sb(2), from: Resume::Push(h1) }));
    assert_eq!((b.hold.clone(), b.version.as_deref()), (None, Some("n")));
    assert_eq!(b.admit(&sb(2), 55), Ok(()));
    // An unheld box with a failed read goes on as with no pointer.
    assert_eq!(b.on(unread(true, "n"), 60), Ok(Act::Renew));
    // The pointer cleared while still on N+1 (a later roll moved on): released there too.
    let mut c = TaskBox::new(task());
    c.start("n1", 0).unwrap();
    c.on(pinned(true, "n1", "n"), 10).unwrap();
    assert_eq!(c.on(alarm(false, "n1"), 20), Ok(Act::Start { sandbox: sb(2), from: Resume::Base }));
    // A queued task takes no hold.
    let mut q = TaskBox::new(task());
    assert_eq!(q.on(pinned(false, "n1", "n"), 0), Ok(Act::Keep));
    assert_eq!(q.hold, None);
    // Held past HOLD_MAX with N never deployed: the task fails and its container stops.
    let mut h = TaskBox::new(task());
    h.start("n1", 0).unwrap();
    h.on(pinned(true, "n1", "n"), 100).unwrap();
    let max = HOLD_MAX.as_millis() as u64;
    assert_eq!(h.on(pinned(false, "n1", "n"), 100 + max - 1), Ok(Act::Keep));
    assert_eq!(h.on(pinned(false, "n1", "n"), 100 + max), Ok(Act::Stop));
    assert!(matches!(h.task.state, TaskState::Failed { .. }));
}

fn request(cmds: &[(&str, ObjectId)]) -> Vec<u8> {
    let mut b = Vec::new();
    for (i, (r, id)) in cmds.iter().enumerate() {
        let caps = if i == 0 { "\0report-status side-band-64k" } else { "" };
        pkt::line(&mut b, &format!("{} {} {r}{caps}", "0".repeat(40), id.hex()));
    }
    pkt::flush(&mut b);
    b.extend_from_slice(b"PACK....");
    b
}

fn status(lines: &[&str]) -> Vec<u8> {
    let mut s = Vec::new();
    for l in lines {
        pkt::line(&mut s, l);
    }
    pkt::flush(&mut s);
    s
}

fn banded(inner: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    pkt::sideband(&mut b, Band::Large, 2, b"remote: resolving\n");
    pkt::sideband(&mut b, Band::Large, 1, inner);
    pkt::flush(&mut b);
    b
}

#[test]
fn acked_pushes_are_the_hosts_ok_lines_for_the_tasks_refs_with_the_requests_ids() {
    let refs = run_refs(&SessionId::Run(RUN)).unwrap();
    let (a, b) = (format!("refs/heads/graftyard/{ID}/a"), format!("refs/heads/graftyard/{ID}/b"));
    let (h1, h2) = (ObjectId([1; 20]), ObjectId([2; 20]));
    let req = request(&[(&a, h1), (&b, h2)]);
    // Plain and side-band: both refs ok.
    let ok = status(&["unpack ok", &format!("ok {a}"), &format!("ok {b}")]);
    assert_eq!(acked_pushes(&req, &ok, &refs), vec![(a.clone(), h1), (b.clone(), h2)]);
    assert_eq!(acked_pushes(&req, &banded(&ok), &refs), vec![(a.clone(), h1), (b.clone(), h2)]);
    // An `ng` ref, an unpack failure, a ref outside the task's refs, a delete, and garbage give nothing for them.
    let ng = status(&["unpack ok", &format!("ok {a}"), &format!("ng {b} fetch first")]);
    assert_eq!(acked_pushes(&req, &banded(&ng), &refs), vec![(a.clone(), h1)]);
    let unpack = status(&["unpack index-pack failed", &format!("ok {a}")]);
    assert!(acked_pushes(&req, &unpack, &refs).is_empty());
    let other = request(&[("refs/heads/main", h1)]);
    assert!(acked_pushes(&other, &status(&["unpack ok", "ok refs/heads/main"]), &refs).is_empty());
    let del = request(&[(&a, ObjectId::ZERO)]);
    assert!(acked_pushes(&del, &status(&["unpack ok", &format!("ok {a}")]), &refs).is_empty());
    assert!(acked_pushes(&req, b"zz", &refs).is_empty());
    // An `ok` for a ref the request never named is not an ack of any id.
    let stray = status(&["unpack ok", &format!("ok refs/heads/graftyard/{ID}/c")]);
    assert!(acked_pushes(&req, &stray, &refs).is_empty());
}

/// The first-party delivery's `cancelled` ends the task and stops its Sandbox; the old Sandbox's next report is
/// refused as final, and a second cancel changes nothing.
#[test]
fn a_cancel_delivery_stops_the_box_and_refuses_its_sandbox() {
    let mut b = started();
    assert_eq!(b.cancel(), Act::Stop);
    assert_eq!(b.task.state, TaskState::Cancelled);
    assert_eq!(b.on(pushed(sb(1), ObjectId([7; 20])), 1), Err(Report::Refused(StepErr::Final)));
    assert_eq!(b.cancel(), Act::Stop);
    assert_eq!(b.task.state, TaskState::Cancelled);
    assert_eq!(b.on(alarm(false, "v1"), HEARTBEAT.as_millis() as u64), Ok(Act::Keep), "a final box starts nothing");
}
