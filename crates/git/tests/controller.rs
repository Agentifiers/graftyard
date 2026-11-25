//! AM3-WP1's controller task model in memory (`crates/git/src/controller.rs`): a new task's workspace and prefix, its
//! budget at each limit, every step its states take, the pause rule, and the base state each state sends.

use std::time::Duration;

use graftyard_git::controller::{
    NoWorkspaceOrg, RepoRef, SandboxId, StepErr, TASK_WALL, Task, TaskBudget, TaskEvent, TaskState, Usage, Workspace,
    workspace,
};
use graftyard_git::issues::IssueNumber;
use graftyard_git::refs::ObjectId;
use graftyard_git::task::TaskApiState;
use graftyard_identity::{AppId, Login, RepoName, SessionId, UserId, WorkspaceMode};
use graftyard_types::{AccountId, DbId, Index};

const RUN: u128 = 0x0e6a_1b2c_0000_4000_8000_0000_0000_0001;

fn repo(owner: &str, name: &str) -> RepoRef {
    RepoRef { owner: Login::parse(owner).unwrap(), name: RepoName::parse(name).unwrap() }
}

fn budget() -> TaskBudget {
    TaskBudget { task_tokens: 1_000, org_month_tokens: 50_000, wall: TASK_WALL }
}

fn task(mode: WorkspaceMode) -> Task {
    let alice = UserId(AccountId(DbId(7)));
    Task::new(SessionId::Run(RUN), None, Some(Index(40)), alice, repo("Acme", "web"), mode, budget()).unwrap()
}

fn sandbox() -> SandboxId {
    SandboxId("sb-1".into())
}

fn used(task_tokens: u64, org_month_tokens: u64, mins: u64) -> Usage {
    Usage { task_tokens, org_month_tokens, elapsed: Duration::from_secs(mins * 60) }
}

#[test]
fn a_new_task_is_queued_under_the_hosts_prefix_with_its_orgs_workspace() {
    let t = task(WorkspaceMode::Fork);
    assert_eq!(t.state, TaskState::Queued);
    assert_eq!(t.workspace, Workspace::Fork(repo("Acme-agents", "web")), "the fork keeps the base's name");
    assert_eq!(t.branch.as_str(), "refs/heads/graftyard/0e6a1b2c-0000-4000-8000-000000000001/**");
    assert_eq!(task(WorkspaceMode::InRepo).workspace, Workspace::InRepo);
    // An external session is no Graftyard-run task.
    let ext = SessionId::External { app: AppId(DbId(3)), name: "fix-1".into() };
    let alice = UserId(AccountId(DbId(7)));
    assert!(Task::new(ext, None, None, alice, repo("acme", "web"), WorkspaceMode::Fork, budget()).is_none());
    // `<org>-agents` must itself be a login of at most 39 characters: a 32-character org fits, a 33-character one
    // does not, and then there is no fork workspace (in-repo still works).
    let (fits, over) = ("a".repeat(32), "a".repeat(33));
    assert!(matches!(workspace(WorkspaceMode::Fork, &repo(&fits, "r")), Ok(Workspace::Fork(_))));
    assert_eq!(workspace(WorkspaceMode::Fork, &repo(&over, "r")), Err(NoWorkspaceOrg));
    assert_eq!(workspace(WorkspaceMode::InRepo, &repo(&over, "r")), Ok(Workspace::InRepo));
}

#[test]
fn a_budget_ends_the_task_past_any_limit_and_at_the_wall() {
    let b = budget();
    assert_eq!(b.over(used(1_000, 50_000, 59)), None, "at a token limit is within it");
    assert!(b.over(used(1_001, 0, 0)).is_some_and(|r| r.contains("task's token")));
    assert!(b.over(used(0, 50_001, 0)).is_some_and(|r| r.contains("monthly")));
    assert!(b.over(used(0, 0, 60)).is_some_and(|r| r.contains("wall-clock")), "the 60-minute cap is reached at 60");
    assert_eq!(TASK_WALL, Duration::from_secs(3_600));
    // A running task over its budget fails with the reason; under it, usage changes nothing.
    let mut t = task(WorkspaceMode::Fork);
    t.step(TaskEvent::Started { sandbox: sandbox() }).unwrap();
    let before = t.clone();
    t.step(TaskEvent::Used(used(999, 10, 30))).unwrap();
    assert_eq!(t, before);
    t.step(TaskEvent::Used(used(1_001, 10, 30))).unwrap();
    assert_eq!(t.state, TaskState::Failed { reason: "the task's token budget is used up".into() });
    assert_eq!(t.step(TaskEvent::Used(used(0, 0, 0))), Err(StepErr::Final));
}

#[test]
fn a_task_runs_pushes_and_ends_and_a_final_task_takes_no_step() {
    let head = ObjectId([7; 20]);
    let mut t = task(WorkspaceMode::Fork);
    assert_eq!(t.step(TaskEvent::Pushed { head }), Err(StepErr::NotNow), "no push before a Sandbox runs it");
    assert_eq!(t.step(TaskEvent::Finished), Err(StepErr::NotNow));
    assert_eq!(t.state, TaskState::Queued, "a refused step changes nothing");
    t.step(TaskEvent::Started { sandbox: sandbox() }).unwrap();
    assert_eq!(t.state, TaskState::Running { sandbox: sandbox(), last_push: None });
    assert_eq!(t.step(TaskEvent::Started { sandbox: SandboxId("sb-2".into()) }), Err(StepErr::NotNow));
    t.step(TaskEvent::Pushed { head }).unwrap();
    assert_eq!(t.state, TaskState::Running { sandbox: sandbox(), last_push: Some(head) });
    let mut done = t.clone();
    t.step(TaskEvent::Drafted { pr: IssueNumber(12) }).unwrap();
    assert_eq!(t.state, TaskState::Draft { pr: IssueNumber(12) });
    done.step(TaskEvent::Finished).unwrap();
    assert_eq!(done.state, TaskState::Done);
    // Every final state refuses every event, the cancel delivery included.
    let mut failed = task(WorkspaceMode::Fork);
    failed.step(TaskEvent::Failed { reason: "tests".into() }).unwrap();
    let mut cancelled = task(WorkspaceMode::Fork);
    cancelled.step(TaskEvent::Cancelled).unwrap();
    for mut f in [t, done, failed, cancelled] {
        for ev in
            [TaskEvent::Cancelled, TaskEvent::Pushed { head }, TaskEvent::Reassigned { by_writer: true, at: Index(90) }]
        {
            let before = f.clone();
            assert_eq!(f.step(ev), Err(StepErr::Final), "{:?}", before.state);
            assert_eq!(f, before);
        }
    }
}

#[test]
fn a_non_writers_edit_pauses_the_task_until_a_writer_reassigns_it() {
    let mut t = task(WorkspaceMode::Fork);
    t.step(TaskEvent::Started { sandbox: sandbox() }).unwrap();
    // A writer's edit, and anyone's reassignment of a running task, change nothing.
    let running = t.clone();
    t.step(TaskEvent::IssueEdited { by_writer: true }).unwrap();
    t.step(TaskEvent::Reassigned { by_writer: true, at: Index(50) }).unwrap();
    assert_eq!(t, running);
    t.step(TaskEvent::IssueEdited { by_writer: false }).unwrap();
    assert!(matches!(t.state, TaskState::Paused { .. }));
    assert_eq!(t.state.api(), Some(TaskApiState::WaitingForUser));
    // Paused: no Sandbox starts, another non-writer edit keeps it paused, and a non-writer's reassignment does nothing.
    assert_eq!(t.step(TaskEvent::Started { sandbox: sandbox() }), Err(StepErr::NotNow));
    let paused = t.clone();
    t.step(TaskEvent::IssueEdited { by_writer: false }).unwrap();
    t.step(TaskEvent::Reassigned { by_writer: false, at: Index(60) }).unwrap();
    assert_eq!(t, paused);
    // A writer's reassignment queues it again with the snapshot at that entry.
    t.step(TaskEvent::Reassigned { by_writer: true, at: Index(70) }).unwrap();
    assert_eq!((t.state.clone(), t.snapshot), (TaskState::Queued, Some(Index(70))));
    // A queued task pauses too, and the cancel delivery ends a paused one.
    t.step(TaskEvent::IssueEdited { by_writer: false }).unwrap();
    t.step(TaskEvent::Cancelled).unwrap();
    assert_eq!(t.state, TaskState::Cancelled);
}

/// Each controller state sends the base the state "Tasks and the activity feed" maps it to, never one `PATCH` may
/// not set, and a controller state is final exactly when the base state it sends is.
#[test]
fn each_state_sends_the_base_its_api_state_and_never_cancelled_or_timed_out() {
    let states = [
        (TaskState::Queued, Some(TaskApiState::Queued)),
        (TaskState::Running { sandbox: sandbox(), last_push: None }, Some(TaskApiState::InProgress)),
        (TaskState::Paused { reason: "r".into() }, Some(TaskApiState::WaitingForUser)),
        (TaskState::Draft { pr: IssueNumber(3) }, Some(TaskApiState::Completed)),
        (TaskState::Done, Some(TaskApiState::Completed)),
        (TaskState::Failed { reason: "r".into() }, Some(TaskApiState::Failed)),
        (TaskState::Cancelled, None),
    ];
    for (s, want) in states {
        assert_eq!(s.api(), want, "{s:?}");
        assert!(!matches!(s.api(), Some(TaskApiState::Cancelled | TaskApiState::TimedOut)));
        assert_eq!(s.is_final(), s.api().is_none_or(TaskApiState::is_final), "{s:?}");
    }
}

/// LM-3's "a Sandbox killed mid-task resumes", the rule (`design/AGENT-LAYER.md`, Failure modes, controller instance: a
/// lost Sandbox restarts from the last push): the lost Sandbox's report moves a running task onto the new Sandbox and
/// keeps `last_push`, so the new one starts from that push, or from the base when the task never pushed. A report about
/// a Sandbox the task no longer holds, a new Sandbox that is the lost one, and a task that is not running move nothing.
#[test]
fn a_lost_sandbox_restarts_the_task_from_its_last_push_or_the_base_under_a_new_sandbox() {
    use graftyard_git::controller::Resume;
    let (sb1, sb2, sb3) = (sandbox(), SandboxId("sb-2".into()), SandboxId("sb-3".into()));
    let head = ObjectId([7; 20]);
    // Never pushed: the new Sandbox starts from the base.
    let mut t = task(WorkspaceMode::Fork);
    assert_eq!(t.resume(), None, "a queued task has no Sandbox to restart");
    assert_eq!(t.step(TaskEvent::Lost { lost: sb1.clone(), sandbox: sb2.clone() }), Err(StepErr::NotNow));
    t.step(TaskEvent::Started { sandbox: sb1.clone() }).unwrap();
    assert_eq!(t.resume(), Some(Resume::Base));
    t.step(TaskEvent::Lost { lost: sb1.clone(), sandbox: sb2.clone() }).unwrap();
    assert_eq!(t.state, TaskState::Running { sandbox: sb2.clone(), last_push: None });
    assert_eq!(t.resume(), Some(Resume::Base));
    // Pushed, then lost: the next Sandbox starts from that push, and its pushes go on from there.
    t.step(TaskEvent::Pushed { head }).unwrap();
    t.step(TaskEvent::Lost { lost: sb2.clone(), sandbox: sb3.clone() }).unwrap();
    assert_eq!(t.state, TaskState::Running { sandbox: sb3.clone(), last_push: Some(head) });
    assert_eq!(t.resume(), Some(Resume::Push(head)));
    // A late report about sb2, which the task no longer holds, or a "new" Sandbox that is the lost one, is refused.
    let before = t.clone();
    assert_eq!(t.step(TaskEvent::Lost { lost: sb2.clone(), sandbox: SandboxId("sb-4".into()) }), Err(StepErr::NotNow));
    assert_eq!(t.step(TaskEvent::Lost { lost: sb3.clone(), sandbox: sb3.clone() }), Err(StepErr::NotNow));
    assert_eq!(t, before);
    // The restart is no new task: the budget's wall still counts from the first start, and the base state stays.
    assert_eq!(t.state.api(), Some(TaskApiState::InProgress));
    t.step(TaskEvent::Drafted { pr: IssueNumber(9) }).unwrap();
    assert_eq!(t.resume(), None, "a final task restarts nothing");
    assert_eq!(t.step(TaskEvent::Lost { lost: sb3, sandbox: sb1 }), Err(StepErr::Final));
}
