//! AM1-WP3's task rules in memory, on the same real members as `agent_gates.rs` (`common`): three
//! `graftyard_raft::Member<AuthzCmd>` of the install's chain and three `Member<RepoCmd>` per repository, whose leader
//! reads the install's chain through `fresh` once per entry. LF-1 runs in `crates/api/tests/task_api.rs`, which reads
//! the tasks through the task API's renderer on the same harness.
//!
//! - External tasks: who may create, `already_exists`, `PATCH` and cancel rules, final states staying final, the open-
//!   task caps, and `TASK_IDLE`.
//! - Task-to-session linking: a token's `task` claim binds a Graftyard-run App's push to its task's prefix, a
//!   `Graftyard-Session` trailer links for the task's App only, `graftyard-workspace` writes its task's session ref, and
//!   the requester reaches `AgentMeta` and `AgentGate`.
//! - Minting: `task::token_task`'s checks on the fold a token request reads.

mod common;
use common::*;

use graftyard_git::task::{
    self, AgentTask, NewTask, TASK_IDLE, TASKS_OPEN_PER_REPO, TASKS_OPEN_PER_REQUESTER, TaskActor, TaskApiState,
    TaskErr, TaskKind, TaskTokenError,
};
use graftyard_identity::{AuthzRead, SessionId};
use graftyard_types::Index;

/// The install's `graftyard-workspace` App.
const WS: AppId = AppId(DbId(8));
const ORIGIN: &str = "https://graftyard.example/o/r";
const RUN_TASK: u128 = 0x0193_a1b2_c3d4_8e5f_8a9b_0c1d_2e3f_4a5b;

fn uid(a: i64) -> UserId {
    UserId(AccountId(DbId(a)))
}

fn task_err(o: &RepoOutcome) -> Option<TaskErr> {
    match o {
        Err(RepoErr::Issue { err: IssueErr::Task(t), .. }) => Some(t.clone()),
        _ => None,
    }
}

fn task_out(o: &RepoOutcome) -> IssueOut {
    match o {
        Ok(RepoOut::Issue(c)) => c.out.clone(),
        other => panic!("not a task reply: {other:?}"),
    }
}

fn create(session: &str, requester: Option<i64>) -> IssueOp<graftyard_git::blob::BlobRef> {
    IssueOp::CreateTask(NewTask {
        session: session.into(),
        requester: requester.map(uid),
        issue: None,
        summary: "".into(),
    })
}

fn patch(id: &SessionId, state: Option<TaskApiState>) -> IssueOp<graftyard_git::blob::BlobRef> {
    IssueOp::UpdateTask { id: id.clone(), state, pr: None, summary: None }
}

fn cancel(id: &SessionId) -> IssueOp<graftyard_git::blob::BlobRef> {
    IssueOp::CancelTask { id: id.clone() }
}

fn ext(app: AppId, name: &str) -> SessionId {
    SessionId::External { app, name: name.into() }
}

fn stored(w: &mut World, id: &SessionId) -> task::TaskCore {
    task::task_core(&*w.fold().core, id).expect("a task row")
}

fn effective(w: &mut World, id: &SessionId) -> TaskApiState {
    let t = stored(w, id);
    task::effective(&t, &w.ident.owner, w.current, w.now)
}

/// The entries' formats on the repository leader's log, with their events.
fn task_entries(w: &World) -> Vec<(Format, Vec<RepoEvent>)> {
    let l = w.repo.leader();
    w.repo.members[l.0 as usize]
        .log()
        .iter()
        .filter_map(|e| match &e.body {
            graftyard_raft::EntryBody::Events(evs) => Some((e.format, evs.clone())),
            _ => None,
        })
        .filter(|(_, evs)| evs.iter().any(|e| matches!(e, RepoEvent::TaskCreated(_) | RepoEvent::TaskUpdated { .. })))
        .collect()
}

// ---- external tasks ----

/// The API reference's rules for `kind: external`, each through `decide` on the real members.
#[test]
fn external_tasks_follow_the_reference_rules() {
    let mut w = World::new();
    w.flag(claude_row());
    let id = ext(CLAUDE, "fix-1");
    // Only an `AgentApp`'s installation token creates one, and only in its own App's namespace.
    assert_eq!(task_out(&w.commit(issue(app(CLAUDE), create("fix-1", Some(WRITER))))), IssueOut::Task(id.clone()));
    let t = stored(&mut w, &id);
    assert_eq!(
        (t.kind, t.state, t.prefix.clone(), t.requester, t.app),
        (TaskKind::External, TaskApiState::Queued, None, Some(uid(WRITER)), CLAUDE)
    );
    assert_eq!(task_err(&w.commit(issue(app(CLAUDE), create("fix-1", None)))), Some(TaskErr::SessionExists));
    assert_eq!(task_err(&w.commit(issue(app(CLAUDE), create("3.fix-1", None)))), Some(TaskErr::SessionExists));
    assert_eq!(task_err(&w.commit(issue(app(PLAIN), create("p", None)))), Some(TaskErr::NotAgentApp));
    assert_eq!(task_err(&w.commit(issue(user(WRITER), create("u", None)))), Some(TaskErr::NotAgentApp));
    assert_eq!(task_err(&w.commit(issue(app(CLAUDE), create("6.other", None)))), Some(TaskErr::BadSession));
    assert_eq!(task_err(&w.commit(issue(app(CLAUDE), create("Bad Name", None)))), Some(TaskErr::BadSession));
    // `PATCH`: the task's App only; never `cancelled` or `timed_out`; a `pr_number` must name a pull request.
    let mut summary = patch(&id, Some(TaskApiState::InProgress));
    if let IssueOp::UpdateTask { summary: s, .. } = &mut summary {
        *s = Some("working".into());
    }
    assert_eq!(task_out(&w.commit(issue(app(CLAUDE), summary))), IssueOut::Task(id.clone()));
    let f = w.fold();
    assert_eq!(task::task_meta(&*f.meta, &id).map(|m| m.summary), Some("working".into()));
    assert_eq!(task::task_core(&*f.core, &id).map(|t| t.state), Some(TaskApiState::InProgress));
    assert_eq!(task_err(&w.commit(issue(app(MCP), patch(&id, None)))), Some(TaskErr::NotYours));
    assert_eq!(task_err(&w.commit(issue(user(WRITER), patch(&id, None)))), Some(TaskErr::NotYours));
    for s in [TaskApiState::Cancelled, TaskApiState::TimedOut] {
        assert_eq!(task_err(&w.commit(issue(app(CLAUDE), patch(&id, Some(s))))), Some(TaskErr::HostState));
    }
    let bad_pr = IssueOp::UpdateTask { id: id.clone(), state: None, pr: Some(IssueNumber(99)), summary: None };
    assert_eq!(task_err(&w.commit(issue(app(CLAUDE), bad_pr))), Some(TaskErr::Invalid("pr_number")));
    // Cancel: the requester, a repository admin, or the task's App; a reader may not; a stranger finds nothing.
    assert_eq!(task_err(&w.commit(issue(user(READER), cancel(&id)))), Some(TaskErr::NotYours));
    assert_eq!(task_err(&w.commit(issue(user(WRITER2), cancel(&id)))), Some(TaskErr::NotYours));
    let o = w.commit(issue(user(SITE_ADMIN), cancel(&id)));
    assert!(matches!(o, Err(RepoErr::Issue { err: IssueErr::NotFound, .. })), "{o:?}");
    assert_eq!(task_out(&w.commit(issue(user(WRITER), cancel(&id)))), IssueOut::Task(id.clone()));
    assert_eq!(stored(&mut w, &id).state, TaskApiState::Cancelled);
    // Final stays final: `PATCH` and cancel answer 409 with the state and write nothing.
    let before = task_entries(&w).len();
    for s in [None, Some(TaskApiState::InProgress), Some(TaskApiState::Completed)] {
        assert_eq!(
            task_err(&w.commit(issue(app(CLAUDE), patch(&id, s)))),
            Some(TaskErr::Final(TaskApiState::Cancelled))
        );
    }
    assert_eq!(task_err(&w.commit(issue(user(OWNER), cancel(&id)))), Some(TaskErr::Final(TaskApiState::Cancelled)));
    assert_eq!(task_entries(&w).len(), before);
    // An admin and the App cancel too; `completed` is final as well.
    for (who, name) in [(user(OWNER), "by-admin"), (app(CLAUDE), "by-app")] {
        w.commit(issue(app(CLAUDE), create(name, None))).unwrap();
        let t = ext(CLAUDE, name);
        assert_eq!(task_out(&w.commit(issue(who, cancel(&t)))), IssueOut::Task(t.clone()));
        assert_eq!(stored(&mut w, &t).state, TaskApiState::Cancelled);
    }
    w.commit(issue(app(CLAUDE), create("done", None))).unwrap();
    let done = ext(CLAUDE, "done");
    w.commit(issue(app(CLAUDE), patch(&done, Some(TaskApiState::Completed)))).unwrap();
    assert_eq!(
        task_err(&w.commit(issue(app(CLAUDE), patch(&done, Some(TaskApiState::InProgress))))),
        Some(TaskErr::Final(TaskApiState::Completed))
    );
    assert_eq!(task_err(&w.commit(issue(user(OWNER), cancel(&done)))), Some(TaskErr::Final(TaskApiState::Completed)));
    // Every task entry is 32.12; no other entry carries a task event.
    let entries = task_entries(&w);
    assert!(entries.iter().all(|(f, _)| *f == graftyard_git::repo::REPO_FORMAT_TASKS), "{entries:?}");
    eprintln!("external tasks: {} task entries, all 32.12", entries.len());
}

/// `TASKS_OPEN_PER_REQUESTER` per requester (or per App without one), `TASKS_OPEN_PER_REPO` per repository; a final
/// task frees its place.
#[test]
fn open_task_caps() {
    let mut w = World::new();
    w.flag(claude_row());
    let per = usize::from(TASKS_OPEN_PER_REQUESTER);
    let cmds = (0..per).map(|i| issue(app(CLAUDE), create(&format!("w-{i}"), Some(WRITER)))).collect();
    assert!(w.commit_all(cmds).iter().all(|o| o.is_ok()));
    let over = w.commit(issue(app(CLAUDE), create("w-over", Some(WRITER))));
    assert_eq!(task_err(&over), Some(TaskErr::Cap { repo: false }));
    // Another requester, and the App's own (no requester), still have room.
    w.commit(issue(app(CLAUDE), create("w2-0", Some(WRITER2)))).unwrap();
    w.commit(issue(app(CLAUDE), create("none-0", None))).unwrap();
    // A cancel frees one place; a completed task does too.
    w.commit(issue(user(WRITER), cancel(&ext(CLAUDE, "w-0")))).unwrap();
    w.commit(issue(app(CLAUDE), create("w-over", Some(WRITER)))).unwrap();
    assert_eq!(
        task_err(&w.commit(issue(app(CLAUDE), create("w-x", Some(WRITER))))),
        Some(TaskErr::Cap { repo: false })
    );
    w.commit(issue(app(CLAUDE), patch(&ext(CLAUDE, "w-1"), Some(TaskApiState::Completed)))).unwrap();
    w.commit(issue(app(CLAUDE), create("w-x", Some(WRITER)))).unwrap();
    // The repository's cap: fill it with requesters of 100 each.
    let have = 102;
    let mut k = 0;
    let mut made = have;
    while made < usize::from(TASKS_OPEN_PER_REPO) {
        let n = (usize::from(TASKS_OPEN_PER_REPO) - made).min(per);
        let cmds = (0..n).map(|i| issue(app(CLAUDE), create(&format!("r{k}-{i}"), Some(1_000 + k)))).collect();
        assert!(w.commit_all(cmds).iter().all(|o| o.is_ok()));
        made += n;
        k += 1;
    }
    assert_eq!(task_err(&w.commit(issue(app(CLAUDE), create("full", Some(9_999))))), Some(TaskErr::Cap { repo: true }));
    eprintln!("caps: {per} open per requester, then 422; {} open per repository, then 422", TASKS_OPEN_PER_REPO);
}

/// A task with no update for `TASK_IDLE` reads `timed_out`, with no sweep; any update before then keeps it; its next
/// command records the state and answers it.
#[test]
fn an_idle_task_times_out_and_its_next_command_records_it() {
    let mut w = World::new();
    w.flag(claude_row());
    for n in ["idle", "kept"] {
        w.commit(issue(app(CLAUDE), create(n, Some(WRITER)))).unwrap();
    }
    let (idle, kept) = (ext(CLAUDE, "idle"), ext(CLAUDE, "kept"));
    let start = w.now;
    let day = TASK_IDLE.as_millis() as u64;
    w.now = Timestamp(start.0 + day - 60_000);
    w.commit(issue(app(CLAUDE), patch(&kept, None))).expect("an empty PATCH is an update");
    w.now = Timestamp(start.0 + day + 1);
    let entries = task_entries(&w).len();
    assert_eq!(effective(&mut w, &idle), TaskApiState::TimedOut);
    assert_eq!(effective(&mut w, &kept), TaskApiState::Queued);
    assert_eq!(stored(&mut w, &idle).state, TaskApiState::Queued, "no sweep wrote it");
    assert_eq!(task_entries(&w).len(), entries);
    let o = w.commit(issue(app(CLAUDE), patch(&idle, Some(TaskApiState::InProgress))));
    assert_eq!(task_out(&o), IssueOut::TaskFinal(idle.clone(), TaskApiState::TimedOut));
    assert_eq!(stored(&mut w, &idle).state, TaskApiState::TimedOut);
    assert_eq!(task_err(&w.commit(issue(user(WRITER), cancel(&idle)))), Some(TaskErr::Final(TaskApiState::TimedOut)));
}

// ---- task-to-session linking ----

fn run_task() -> SessionId {
    SessionId::Run(RUN_TASK)
}

fn seed_run_task(w: &mut World, s: &SessionId, app: AppId, requester: i64) {
    let t = AgentTask {
        session: s.clone(),
        kind: TaskKind::GraftyardRun,
        state: TaskApiState::InProgress,
        prefix: task::run_prefix(s),
        requester: Some(uid(requester)),
        app,
        installation: claims(app).installation,
        creator: TaskActor::User(uid(requester)),
        issue: None,
        snapshot: Index(1),
        prompt: Some("fix the flaky test".into()),
        pr: None,
        summary: "".into(),
    };
    // Graftyard-run creation is AM3-WP3's (the controller and `agents_enabled`): the host records the row here.
    w.record_in(graftyard_git::repo::REPO_FORMAT_TASKS, vec![RepoEvent::TaskCreated(t)]);
}

fn claimed(a: AppId, s: Option<&SessionId>) -> Principal {
    let mut c = claims(a);
    c.task = s.cloned();
    Principal::installation(&c)
}

/// The Graftyard-run App, the install's `graftyard-workspace`, both installed on the repository.
fn run_world() -> World {
    let mut w = World::new();
    register_own(&mut w, RUN, "graftyard-agent");
    w.flag(AgentApp { app: RUN, kind: AgentKind::GraftyardRun, confine: None });
    register_own(&mut w, WS, "graftyard-workspace");
    w.flag(AgentApp { app: WS, kind: AgentKind::GraftyardRun, confine: None });
    let c = claims(WS);
    let row = InstallationRow { app: WS, repos: c.repos.clone(), permissions: everything() };
    w.ident.owner.apply(&AuthzEvent::Installation { id: c.installation, row: Some(row), not_before: Timestamp(0) });
    w.flag(claude_row());
    seed_run_task(&mut w, &run_task(), RUN, WRITER);
    w
}

fn trailer_commit(session: &SessionId) -> Vec<u8> {
    let mut b = b"tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\nauthor A <a@x> 1 +0000\ncommitter A <a@x> 1 +0000\n\n"
        .to_vec();
    b.extend_from_slice(
        format!("Fix\n\nGraftyard-Session: {ORIGIN}/_graftyard/sessions/{}\n", session.text()).as_bytes(),
    );
    b
}

/// A push of one new commit carrying `session`'s trailer to `branch`, linked as the edge links it for `who`, through
/// the member wire.
fn traced_push(w: &mut World, who: Principal, branch: &str, n: u32, session: &SessionId) -> RepoOutcome {
    let r = name(branch);
    let raw = trailer_commit(session);
    let commit = oid(n);
    let prov = agent::provenance_of(&r, &[(commit, raw.as_slice())], &[]);
    let pusher = match who.actor {
        Actor::App { app, .. } => Some(app),
        _ => None,
    };
    let f = w.fold();
    let mut own = pusher.map(|a| task::app_sessions(&*f.core, a)).unwrap_or_default();
    own.extend(task::claim(&who).cloned());
    let linked = agent::link(&prov, pusher, &own, Some(ORIGIN));
    let mut p = push(who, vec![up(branch, w.get(branch), Some(commit))], &["src/lib.rs"]);
    p.provenance = linked;
    let p = graftyard_git::repo::Push::from_wire(&p.to_wire()).expect("the push crosses the member wire");
    w.commit(RepoCmd::Push(p))
}

fn stored_sources(w: &mut World, commit: ObjectId) -> Vec<agent::Source> {
    let f = w.fold();
    graftyard_fold::Read::prefix(&*f.meta, "provenance", &[])
        .into_iter()
        .filter(|(k, _)| k[0] == tiers::sha_key(&commit))
        .flat_map(|(_, cols)| {
            let graftyard_fold::Val::Blob(b) = &cols[0] else { panic!("a record") };
            postcard::from_bytes::<SmallVec<[agent::Source; 2]>>(b).unwrap().into_vec()
        })
        .collect()
}

/// A push from a live task's session: accepted inside the task's prefix (where AM1-WP2 refused every Graftyard-run
/// push `NoLiveTask`), refused outside it or without the claim, and its `Graftyard-Session` trailer links.
#[test]
fn a_live_tasks_push_is_accepted_and_its_trailer_links() {
    const N: u32 = 100;
    let mut w = run_world();
    let s = run_task();
    let base = format!("refs/heads/graftyard/{}", s.text());
    let mut counts = [0u32; 6];
    for i in 0..N {
        let inside = format!("{base}/w{i}");
        let o = traced_push(&mut w, claimed(RUN, Some(&s)), &inside, 10_000 + i, &s);
        counts[0] += u32::from(refs(&o) == [RefOutcome::Ok]);
        counts[1] += u32::from(stored_sources(&mut w, oid(10_000 + i)) == [agent::Source::Session(s.clone())]);
        let o = w.commit(RepoCmd::Push(push(
            claimed(RUN, None),
            vec![up(&format!("{base}/n{i}"), None, Some(oid(7)))],
            &["a"],
        )));
        counts[2] += u32::from(agent_rule(&o) == Some(AgentRule::NoLiveTask));
        let o = w.commit(RepoCmd::Push(push(
            claimed(RUN, Some(&s)),
            vec![up(&format!("refs/heads/run/x{i}"), None, Some(oid(7)))],
            &["a"],
        )));
        counts[3] += u32::from(agent_rule(&o) == Some(AgentRule::OutsidePrefix));
        // Another task's prefix, and the bare `refs/heads/graftyard/<uuid>` with no segment after the id.
        let other = format!("refs/heads/graftyard/{}/x{i}", SessionId::Run(RUN_TASK + 1).text());
        for r in [other, base.clone()] {
            let o = w.commit(RepoCmd::Push(push(claimed(RUN, Some(&s)), vec![up(&r, None, Some(oid(7)))], &["a"])));
            counts[4] += u32::from(agent_rule(&o) == Some(AgentRule::OutsidePrefix));
        }
        // A claim naming a task that does not exist binds nothing.
        let ghost = SessionId::Run(RUN_TASK + 2);
        let o = w.commit(RepoCmd::Push(push(
            claimed(RUN, Some(&ghost)),
            vec![up(&format!("refs/heads/graftyard/{}/g{i}", ghost.text()), None, Some(oid(7)))],
            &["a"],
        )));
        counts[5] += u32::from(agent_rule(&o) == Some(AgentRule::NoLiveTask));
    }
    eprintln!(
        "linking: {}/{N} claimed pushes inside the prefix accepted, {}/{N} trailers linked Session; refused: {}/{N} \
         without the claim (NoLiveTask), {}/{N} outside the prefix, {}/{} on another prefix or the bare id, {}/{N} \
         with a claim on no task",
        counts[0],
        counts[1],
        counts[2],
        counts[3],
        counts[4],
        2 * N,
        counts[5]
    );
    assert_eq!(counts, [N, N, N, N, 2 * N, N]);
    // The same trailer pushed by another App stays `Claimed`, and a `Session` it forges on the wire is dropped.
    let o = traced_push(&mut w, app(CLAUDE), "refs/heads/claude/t", 20_000, &s);
    assert_eq!(refs(&o), [RefOutcome::Ok]);
    let src = stored_sources(&mut w, oid(20_000));
    assert!(matches!(&src[..], [agent::Source::Claimed { key: agent::TrailerKey::GraftyardSession, .. }]), "{src:?}");
    let r = name("refs/heads/claude/u");
    let mut p = push(app(CLAUDE), vec![up("refs/heads/claude/u", None, Some(oid(20_001)))], &["a"]);
    p.provenance = vec![agent::Provenance {
        commit: oid(20_001),
        refname: r,
        sources: smallvec![agent::Source::Session(s.clone())],
    }];
    assert_eq!(refs(&w.commit(RepoCmd::Push(p))), [RefOutcome::Ok]);
    assert!(stored_sources(&mut w, oid(20_001)).is_empty(), "a forged Session is dropped");
    // A paused task, then a cancelled one, binds nothing: the next push is `NoLiveTask`.
    let pause = RepoEvent::TaskUpdated {
        session: s.clone(),
        state: Some(TaskApiState::WaitingForUser),
        pr: None,
        summary: None,
        actor: TaskActor::User(uid(WRITER)),
    };
    w.record_in(graftyard_git::repo::REPO_FORMAT_TASKS, vec![pause]);
    let o = w.commit(RepoCmd::Push(push(
        claimed(RUN, Some(&s)),
        vec![up(&format!("{base}/p"), None, Some(oid(7)))],
        &["a"],
    )));
    assert_eq!(agent_rule(&o), Some(AgentRule::NoLiveTask), "paused");
    assert_eq!(task_out(&w.commit(issue(user(WRITER), cancel(&s)))), IssueOut::Task(s.clone()));
    let o = w.commit(RepoCmd::Push(push(
        claimed(RUN, Some(&s)),
        vec![up(&format!("{base}/c"), None, Some(oid(7)))],
        &["a"],
    )));
    assert_eq!(agent_rule(&o), Some(AgentRule::NoLiveTask), "cancelled");
    // A claimed push's entry is 32.12; an unclaimed App push's stays 32.8.
    let l = w.repo.leader();
    let formats: std::collections::BTreeSet<_> =
        w.repo.members[l.0 as usize].log().iter().map(|e| (e.format.family, e.format.version)).collect();
    assert!(formats.contains(&(0x20, 12)) && formats.contains(&(0x20, 8)), "{formats:?}");
}

/// `graftyard-workspace` may create and move its live task's `refs/graftyard/sessions/<id>` and nothing else there;
/// it never deletes it, and no other App writes it.
#[test]
fn the_workspace_writes_only_its_tasks_session_ref() {
    let mut w = run_world();
    let s = run_task();
    let own = format!("refs/graftyard/sessions/{}", s.text());
    let o = w.commit(RepoCmd::Push(push(claimed(WS, Some(&s)), vec![up(&own, None, Some(oid(30)))], &["log.json"])));
    assert_eq!(refs(&o), [RefOutcome::Ok], "create");
    let o = w.commit(RepoCmd::Push(push(claimed(WS, Some(&s)), vec![up(&own, Some(oid(30)), Some(oid(31)))], &["l"])));
    assert_eq!(refs(&o), [RefOutcome::Ok], "move from the tip it names");
    let o = w.commit(RepoCmd::Push(push(claimed(WS, Some(&s)), vec![up(&own, Some(oid(30)), Some(oid(32)))], &["l"])));
    assert_ne!(refs(&o), [RefOutcome::Ok], "a stale tip");
    let o = w.commit(RepoCmd::Push(push(claimed(WS, Some(&s)), vec![up(&own, Some(oid(31)), None)], &[])));
    assert_eq!(refs(&o), [RefOutcome::Rejected(RejectReason::Hidden)], "delete");
    let other = format!("refs/graftyard/sessions/{}", SessionId::Run(RUN_TASK + 1).text());
    let o = w.commit(RepoCmd::Push(push(claimed(WS, Some(&s)), vec![up(&other, None, Some(oid(33)))], &["l"])));
    assert_eq!(refs(&o), [RefOutcome::Rejected(RejectReason::Hidden)], "another session");
    for who in [claimed(RUN, Some(&s)), app(CLAUDE), user(OWNER), claimed(WS, None)] {
        let o = w.commit(RepoCmd::Push(push(who, vec![up(&own, Some(oid(31)), Some(oid(34)))], &["l"])));
        assert_eq!(refs(&o), [RefOutcome::Rejected(RejectReason::Hidden)]);
    }
    // The workspace pushes the task's branches too, inside its prefix.
    let br = format!("refs/heads/graftyard/{}/ws", s.text());
    assert_eq!(
        refs(&w.commit(RepoCmd::Push(push(claimed(WS, Some(&s)), vec![up(&br, None, Some(oid(35)))], &["a"])))),
        [RefOutcome::Ok]
    );
    assert_eq!(w.get(&own), Some(oid(31)));
}

/// A PR the task's App opens records the task's session and requester in `AgentMeta`, so the requester's approval
/// writes no gate and one other writer's does; a session the client names counts only for its own App's task.
#[test]
fn agent_meta_takes_the_session_and_requester_from_the_task() {
    let mut w = run_world();
    let s = run_task();
    let br = format!("refs/heads/graftyard/{}/pr", s.text());
    assert_eq!(
        refs(&w.commit(RepoCmd::Push(push(claimed(RUN, Some(&s)), vec![up(&br, None, Some(oid(40)))], &["a"])))),
        [RefOutcome::Ok]
    );
    let n = number(&open_pr(&mut w, claimed(RUN, Some(&s)), &br, true));
    let meta = tiers::agent_meta(&*w.fold().core, IssueNumber(n)).expect("AgentMeta");
    assert_eq!((meta.app, meta.session.clone(), meta.requester), (RUN, Some(s.clone()), Some(uid(WRITER))));
    let gates = |w: &mut World| tiers::agent_gates(&*w.fold().core, IssueNumber(n), &oid(40)).len();
    approve(&mut w, user(WRITER), n).unwrap();
    assert_eq!(gates(&mut w), 0, "the requester's approval is not a gate");
    approve(&mut w, user(WRITER2), n).unwrap();
    assert_eq!(gates(&mut w), 1);
    // An external task's session in `X-Graftyard-Agent-Session`.
    w.commit(issue(app(CLAUDE), create("ext", Some(WRITER2)))).unwrap();
    for (session, want) in [(ext(CLAUDE, "ext"), Some(uid(WRITER2))), (s.clone(), None)] {
        let br = format!("refs/heads/claude/h{}", want.is_some());
        w.set(&br, oid(41 + u32::from(want.is_some())));
        let head = w.get(&br).unwrap();
        let op = IssueOp::OpenPr {
            head: HeadSpec { repo: REPO, branch: name(&br) },
            base: name("refs/heads/main"),
            title: "t".into(),
            body: "b".into(),
            draft: false,
            head_sha: head,
            base_sha: w.get("refs/heads/main").unwrap(),
        };
        let mut c = IssueCall::new(op, app(CLAUDE), RequestId(Ulid(0)), serde_json::Value::Null);
        c.agent = true;
        c.session = Some(session.clone());
        let c = serde_json::from_value::<RepoCmd>(serde_json::to_value(RepoCmd::Issue(Box::new(c))).unwrap()).unwrap();
        let n = number(&w.commit(c));
        let meta = tiers::agent_meta(&*w.fold().core, IssueNumber(n)).expect("AgentMeta");
        assert_eq!(meta.requester, want);
        assert_eq!(meta.session.is_some(), want.is_some(), "another App's session is not recorded");
    }
}

// ---- minting ----

/// `graftyard_task` on a token request, checked on the repository's fold and the owner's `Authz`.
#[test]
fn a_task_token_needs_a_live_run_task_of_the_app_on_its_repository() {
    let mut w = run_world();
    let s = run_task();
    w.commit(issue(app(CLAUDE), create("ext", None))).unwrap();
    let one = RepoScope::Selected(smallvec![REPO]);
    let check = |w: &mut World, app: AppId, repos: &RepoScope, s: &SessionId| {
        let f = w.fold();
        let install = w.install.fresh(w.now);
        task::token_task(&*f.core, &w.ident.owner, Some(&install as &dyn AuthzRead), w.now, app, REPO, repos, s)
    };
    assert_eq!(check(&mut w, RUN, &one, &s), Ok(s.clone()));
    assert_eq!(check(&mut w, WS, &one, &s), Ok(s.clone()), "the install's graftyard-workspace");
    assert_eq!(check(&mut w, CLAUDE, &one, &s), Err(TaskTokenError::OtherApp));
    assert_eq!(check(&mut w, RUN, &RepoScope::All, &s), Err(TaskTokenError::NeedsRepositories));
    assert_eq!(check(&mut w, RUN, &RepoScope::Selected(smallvec![]), &s), Err(TaskTokenError::NeedsRepositories));
    let two = RepoScope::Selected(smallvec![REPO, RepoId(DbId(8))]);
    assert_eq!(check(&mut w, RUN, &two, &s), Err(TaskTokenError::OtherRepository));
    assert_eq!(check(&mut w, RUN, &one, &SessionId::Run(1)), Err(TaskTokenError::NoTask));
    assert_eq!(check(&mut w, CLAUDE, &one, &ext(CLAUDE, "ext")), Err(TaskTokenError::NotLive), "external");
    // Uninstalled, then cancelled.
    let id = claims(RUN).installation;
    let row = w.ident.owner.installation(id).cloned().unwrap();
    w.ident.owner.apply(&AuthzEvent::Installation { id, row: None, not_before: w.now });
    assert_eq!(check(&mut w, RUN, &one, &s), Err(TaskTokenError::NotLive), "uninstalled");
    w.ident.owner.apply(&AuthzEvent::Installation { id, row: row.row, not_before: w.now });
    assert_eq!(check(&mut w, RUN, &one, &s), Err(TaskTokenError::NotLive), "a reinstall does not revive it");
}
