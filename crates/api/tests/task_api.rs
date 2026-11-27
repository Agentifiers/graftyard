//! AM1-WP3's task API in memory: the read endpoints and the write replies of `graftyard_api::tasks`, over the tiers of
//! real repository members (`crates/git/tests/common`, AM1's harness, included by path), and LF-1.
//!
//! LF-1: an App with 100 open tasks across 10 repositories is uninstalled. Every task reads `cancelled` at the next read
//! with no sweep command, and its next command records it. The uninstall is the event `AuthzOp::Uninstall` decides,
//! applied to the owner's `Authz` fold the repository leaders read through `deps`, as `Ident` holds that fold.

#[path = "../../git/tests/common/mod.rs"]
mod common;
use common::*;

use graftyard_api::tasks::{self, Reader};
use graftyard_api::view::{Compare, Directory, Repo, RepoRef};
use graftyard_api::wire::Host;
use graftyard_git::task::{self as task_rules, NewTask, TaskApiState};
use graftyard_identity::{AuthzRead, SessionId};
use graftyard_wire::{Body, Reply};
use serde_json::{Value, json};

/// The repositories LF-1 spreads the tasks over: `repo:7` and nine more of the same owner.
fn repos() -> Vec<RepoId> {
    std::iter::once(REPO).chain((11..20).map(|i| RepoId(DbId(i)))).collect()
}

struct Dir;

impl Directory for Dir {
    fn login(&self, id: AccountId) -> Option<String> {
        Some(match id.0.0 {
            OWNER => "o".into(),
            WRITER => "alice".into(),
            WRITER2 => "carol".into(),
            n => format!("u{n}"),
        })
    }
    fn name(&self, id: AccountId) -> Option<String> {
        self.login(id)
    }
    fn is_org(&self, _: AccountId) -> bool {
        false
    }
    fn by_login(&self, login: &str) -> Option<AccountId> {
        match login {
            "alice" => Some(AccountId(DbId(WRITER))),
            "carol" => Some(AccountId(DbId(WRITER2))),
            _ => None,
        }
    }
    fn permission(&self, _: AccountId) -> Option<RepoPermission> {
        None
    }
    fn repo(&self, _: RepoId) -> Option<RepoRef> {
        None
    }
    fn ref_of(&self, _: RepoId, _: &RefName) -> Option<ObjectId> {
        None
    }
    fn team(&self, _: DbId) -> Option<(String, String, String)> {
        None
    }
    fn viewer(&self) -> Option<AccountId> {
        None
    }
    fn compare(&self, _: ObjectId, _: ObjectId) -> Compare {
        Compare { commits: 0, additions: 0, deletions: 0, changed_files: 0 }
    }
}

fn host() -> Host {
    Host::new("graftyard.example", false).unwrap()
}

fn at(id: RepoId) -> RepoRef {
    RepoRef { id, owner: "o".into(), owner_id: AccountId(DbId(OWNER)), name: format!("r{}", id.0.0) }
}

fn json_of(r: &Reply) -> &Value {
    match &r.body {
        Body::Json(v) => v,
        other => panic!("not JSON: {other:?}"),
    }
}

const ALL: Reader = Reader { metadata: true, contents: true };

/// Every task of the current repository as `GET .../tasks` lists it, page by page through `Link`.
fn list_all(w: &mut World, query: &str, reader: Reader) -> Vec<Value> {
    let f = w.fold();
    let (h, d) = (host(), Dir);
    let r = Repo { host: &h, at: at(w.current), core: &f.core, meta: &f.meta, dir: &d };
    let mut out = Vec::new();
    let mut q = query.to_owned();
    loop {
        let filter = tasks::parse_filter(&q, &|l| d.by_login(l)).expect("a valid query");
        let reply = tasks::list(&r, &w.ident.owner, w.now, &filter, reader, "/repos/o/r/tasks");
        assert_eq!(reply.status, 200);
        out.extend(json_of(&reply).as_array().unwrap().iter().cloned());
        let Some(link) = reply.header("link") else { return out };
        q = link.split_once('?').unwrap().1.split_once('>').unwrap().0.to_owned();
    }
}

/// The stored `session.json` serves only as task `sid`'s schema 1 log; the page serves it as JSON or as HTML with
/// every harness string escaped.
fn session_log_reads(r: &Repo<'_>, sid: &SessionId) {
    use graftyard_git::session_log::{Harness, LogReport, Step, StepKind, build, encode};
    let evil = "</pre><script>alert(1)</script>";
    let report = LogReport {
        issue_snapshot: Some(json!({ "visible": evil })),
        plan: evil.into(),
        steps: vec![Step {
            at: 1_793_000_000_000,
            kind: StepKind::Test,
            tool: Some(evil.into()),
            input: "cargo test".into(),
            output: evil.into(),
            truncated: true,
        }],
        prs: vec![graftyard_git::issues::IssueNumber(4)],
        harness: Harness { name: evil.into(), version: "1".into(), model: "m".into() },
    };
    let log = build(sid.clone(), &[ObjectId([5; 20])], report, &[]);
    let text = String::from_utf8(encode(&log)).unwrap();
    let stored = tasks::stored_log(sid, Some(&text)).expect("the task's own log");
    assert_eq!(stored, log);
    let other = SessionId::parse("3.other", None).unwrap();
    assert_eq!(tasks::stored_log(&other, Some(&text)), None, "another task's log is not this task's");
    assert_eq!(tasks::stored_log(sid, Some(&text.replace("\"schema\":1", "\"schema\":2"))), None, "schema 2");
    assert_eq!(tasks::stored_log(sid, None), None);
    let j = tasks::log(Some(&stored));
    assert_eq!(
        (j.status, json_of(&j)["schema"].clone(), json_of(&j)["id"].clone()),
        (200, json!(1), json!(sid.text()))
    );
    assert_eq!(json_of(&tasks::session_page(r, Some(&stored), true)), json_of(&j));
    let page = tasks::session_page(r, Some(&stored), false);
    assert_eq!((page.status, page.header("content-type")), (200, Some("text/html; charset=utf-8")));
    let Body::Bytes(b) = &page.body else { panic!("HTML is bytes") };
    let html = String::from_utf8(b.clone()).unwrap();
    assert!(!html.contains("<script>"), "every harness string is escaped");
    assert_eq!(html.matches("&lt;script&gt;").count(), 5, "snapshot, plan, tool, output, harness name");
    assert!(html.contains(&format!("/o/r7/commit/{}", ObjectId([5; 20]).hex())) && html.contains("/o/r7/pull/4"));
    assert!(html.contains("(truncated)"));
}

fn get(w: &mut World, id: &str, reader: Reader) -> Reply {
    let f = w.fold();
    let (h, d) = (host(), Dir);
    let r = Repo { host: &h, at: at(w.current), core: &f.core, meta: &f.meta, dir: &d };
    tasks::get(&r, &w.ident.owner, w.now, id, "", reader)
}

/// CLAUDE's claims, reaching every LF-1 repository.
fn wide(a: AppId) -> Principal {
    let mut c = claims(a);
    c.repos = RepoScope::Selected(repos().into_iter().collect());
    Principal::installation(&c)
}

fn install_wide(w: &mut World, a: AppId) {
    let c = claims(a);
    let row = InstallationRow {
        app: a,
        repos: RepoScope::Selected(repos().into_iter().collect()),
        permissions: everything(),
    };
    w.ident.owner.apply(&AuthzEvent::Installation { id: c.installation, row: Some(row), not_before: Timestamp(0) });
}

fn create(name: &str, requester: i64) -> IssueOp<graftyard_git::blob::BlobRef> {
    IssueOp::CreateTask(NewTask {
        session: name.into(),
        requester: Some(UserId(AccountId(DbId(requester)))),
        issue: None,
        summary: format!("task {name}").into(),
    })
}

/// The task events on the repository leader's log: what a sweep or a command would have written.
fn log_len(w: &World) -> usize {
    let l = w.repo.leader();
    w.repo.members[l.0 as usize]
        .log()
        .iter()
        .filter_map(|e| match &e.body {
            graftyard_raft::EntryBody::Events(evs) => Some(evs),
            _ => None,
        })
        .flatten()
        .filter(|e| matches!(e, RepoEvent::TaskCreated(_) | RepoEvent::TaskUpdated { .. }))
        .count()
}

#[test]
fn lf1_uninstall_cancels_every_task_with_no_sweep() {
    const PER_REPO: usize = 10;
    let mut w = World::new();
    w.flag(claude_row());
    // A second confined App on the same repositories: the control, whose tasks stay live.
    w.flag(AgentApp {
        app: MCP,
        kind: AgentKind::ThirdParty,
        confine: Some(RefPattern::parse("refs/heads/mcp/**").unwrap()),
    });
    install_wide(&mut w, CLAUDE);
    install_wide(&mut w, MCP);
    for r in repos() {
        for u in [WRITER, WRITER2] {
            let grantee = UserId(AccountId(DbId(u)));
            w.ident.owner.apply(&AuthzEvent::Grant { repo: r, grantee, permission: Some(RepoPermission::Write) });
        }
        if r != REPO {
            w.add_repo(r);
        }
    }
    // 100 open tasks of CLAUDE, 10 on each repository, half requested by each writer, some moved to in_progress;
    // one MCP task on each as the control.
    let mut ids = Vec::new();
    for r in repos() {
        w.switch(r);
        let mut cmds: Vec<RepoCmd> = (0..PER_REPO)
            .map(|i| issue(wide(CLAUDE), create(&format!("lf1-{i}"), if i % 2 == 0 { WRITER } else { WRITER2 })))
            .collect();
        cmds.push(issue(wide(MCP), create("control", WRITER)));
        assert!(w.commit_all(cmds).iter().all(|o| o.is_ok()));
        let started: Vec<RepoCmd> = (0..PER_REPO)
            .step_by(3)
            .map(|i| {
                let id = SessionId::External { app: CLAUDE, name: format!("lf1-{i}").into() };
                issue(
                    wide(CLAUDE),
                    IssueOp::UpdateTask { id, state: Some(TaskApiState::InProgress), pr: None, summary: None },
                )
            })
            .collect();
        assert!(w.commit_all(started).iter().all(|o| o.is_ok()));
        ids.extend((0..PER_REPO).map(|i| (r, format!("{}.lf1-{i}", CLAUDE.0.0))));
    }
    assert_eq!(ids.len(), 100);
    // Before: every task reads live.
    let mut live_before = 0;
    for r in repos() {
        w.switch(r);
        live_before +=
            list_all(&mut w, "per_page=4", ALL).iter().filter(|t| t["app_id"] == CLAUDE.0.0 && !is_final(t)).count();
    }
    assert_eq!(live_before, 100);
    // The uninstall, as `AuthzOp::Uninstall` decides it on the owner's chain.
    let id = claims(CLAUDE).installation;
    let not_before = Timestamp(w.now.0.max(w.ident.owner.installation(id).unwrap().not_before.0));
    w.ident.owner.apply(&AuthzEvent::Installation { id, row: None, not_before });
    // The next read: every task reads cancelled, by the list, the single GET, and the state filter, and no entry was
    // written on any repository.
    let logs: Vec<usize> = repos()
        .into_iter()
        .map(|r| {
            w.switch(r);
            log_len(&w)
        })
        .collect();
    let (mut listed, mut single, mut filtered, mut controls) = (0, 0, 0, 0);
    for r in repos() {
        w.switch(r);
        let all = list_all(&mut w, "per_page=3", ALL);
        listed += all.iter().filter(|t| t["app_id"] == CLAUDE.0.0 && t["state"] == "cancelled").count();
        controls += all.iter().filter(|t| t["app_id"] == MCP.0.0 && t["state"] == "queued").count();
        filtered += list_all(&mut w, &format!("state=cancelled&app_id={}", CLAUDE.0.0), ALL).len();
        assert!(list_all(&mut w, &format!("state=queued&app_id={}", CLAUDE.0.0), ALL).is_empty());
        for (_, t) in ids.iter().filter(|(x, _)| *x == r) {
            let g = get(&mut w, t, ALL);
            single += usize::from(g.status == 200 && json_of(&g)["state"] == "cancelled");
        }
    }
    let logs_after: Vec<usize> = repos()
        .into_iter()
        .map(|r| {
            w.switch(r);
            log_len(&w)
        })
        .collect();
    assert_eq!(logs, logs_after, "no sweep: the reads wrote nothing");
    // The next command on each task records it: the requester's (or the admin's) cancel answers 409 `cancelled` and
    // writes `TaskUpdated { state: Cancelled }`; the App's own PATCH no longer authenticates and writes nothing.
    let (mut recorded, mut answered, mut app_refused) = (0, 0, 0);
    for r in repos() {
        w.switch(r);
        let mut cmds = Vec::new();
        for (i, (_, t)) in ids.iter().filter(|(x, _)| *x == r).enumerate() {
            let sid = SessionId::parse(t, None).unwrap();
            let who = match i % 3 {
                0 => user(WRITER),
                1 => user(WRITER2),
                _ => user(OWNER),
            };
            // An admin cancels any task; a writer only the ones they requested, so the other writer's go to the admin.
            let requester = if i % 2 == 0 { WRITER } else { WRITER2 };
            let who = if matches!(who.actor, Actor::User(u) if u.0.0.0 != requester && u.0.0.0 != OWNER) {
                user(OWNER)
            } else {
                who
            };
            cmds.push(issue(who, IssueOp::CancelTask { id: sid }));
        }
        for o in w.commit_all_acked(cmds) {
            let acked = o.expect("the cancel commits");
            let out = match &acked.out {
                RepoOut::Issue(c) => c.out.clone(),
                x => panic!("{x:?}"),
            };
            let f = w.fold();
            let (h, d) = (host(), Dir);
            let rr = Repo { host: &h, at: at(r), core: &f.core, meta: &f.meta, dir: &d };
            let reply = tasks::render_ok(&rr, &w.ident.owner, w.now, &acked.map(|_| out.clone()), false, ALL);
            answered += usize::from(reply.status == 409 && json_of(&reply)["state"] == "cancelled");
            if let IssueOut::TaskFinal(id, TaskApiState::Cancelled) = &out {
                recorded += usize::from(task_rules::task_core(&*f.core, id).unwrap().state == TaskApiState::Cancelled);
            }
        }
        let first = SessionId::External { app: CLAUDE, name: "lf1-0".into() };
        let before = log_len(&w);
        let o = w.commit(issue(wide(CLAUDE), IssueOp::UpdateTask { id: first, state: None, pr: None, summary: None }));
        app_refused += usize::from(matches!(o, Err(RepoErr::Issue { err: IssueErr::NotFound, .. })));
        assert_eq!(log_len(&w), before);
    }
    eprintln!(
        "LF-1: {live_before}/100 live before; after the uninstall {listed}/100 read cancelled in the list, {single}/100 \
         in the single GET, {filtered}/100 under state=cancelled, 0 live; logs unchanged by the reads on {} \
         repositories; the next command recorded {recorded}/100 and answered 409 cancelled {answered}/100; the App's \
         own PATCH refused {app_refused}/10; control App {controls}/10 still queued",
        logs.len()
    );
    assert_eq!(
        (listed, single, filtered, recorded, answered, app_refused, controls),
        (100, 100, 100, 100, 100, 10, 10)
    );
}

fn is_final(t: &Value) -> bool {
    TaskApiState::parse(t["state"].as_str().unwrap()).is_some_and(|s| s.is_final())
}

/// The read endpoints as the API reference gives them: fields, `prompt` only with `contents: read`, `leases` only on
/// the single GET, filters, `per_page` and `Link` with an opaque `after`, 404 without `metadata: read`, and `/log` 404
/// until the log is pushed. A create's reply is 201 with the task, rendered from the `&Acked`.
#[test]
fn the_read_endpoints_serve_the_reference_shape() {
    let mut w = World::new();
    w.flag(claude_row());
    let mut acks = w.commit_all_acked(
        (0..7).map(|i| issue(app(CLAUDE), create(&format!("t{i}"), if i < 4 { WRITER } else { WRITER2 }))).collect(),
    );
    let first = acks.remove(0).expect("created");
    let f = w.fold();
    let (h, d) = (host(), Dir);
    let r = Repo { host: &h, at: at(REPO), core: &f.core, meta: &f.meta, dir: &d };
    let out = match &first.out {
        RepoOut::Issue(c) => c.out.clone(),
        x => panic!("{x:?}"),
    };
    let reply = tasks::render_ok(&r, &w.ident.owner, w.now, &first.map(|_| out.clone()), true, ALL);
    assert_eq!(reply.status, 201);
    let t = json_of(&reply);
    let id = format!("{}.t0", CLAUDE.0.0);
    assert_eq!(t["id"], json!(id));
    assert_eq!((t["kind"].as_str(), t["state"].as_str()), (Some("external"), Some("queued")));
    assert_eq!(t["requester"]["login"], "alice");
    assert_eq!(t["creator"], json!({ "type": "App", "id": CLAUDE.0.0 }));
    assert_eq!(t["prefix"], Value::Null);
    assert_eq!(t["leases"], json!([]));
    assert_eq!(t["html_url"], json!(format!("https://graftyard.example/o/r7/_graftyard/sessions/{id}")));
    for k in ["app_id", "installation_id", "issue_number", "pr_number", "summary", "created_at", "updated_at", "prompt"]
    {
        assert!(t.get(k).is_some(), "{k}");
    }
    drop(f);
    // Single GET with and without `contents: read`, and without `metadata: read`.
    let g = get(&mut w, &id, ALL);
    assert_eq!(g.status, 200);
    assert!(json_of(&g).get("prompt").is_some());
    let no_contents = get(&mut w, &id, Reader { metadata: true, contents: false });
    assert!(json_of(&no_contents).get("prompt").is_none());
    assert_eq!(get(&mut w, &id, Reader { metadata: false, contents: false }).status, 404);
    assert_eq!(get(&mut w, "3.nope", ALL).status, 404);
    // The list: id order, no `leases`, pages of 3 with `Link`, filters by requester login and state.
    let all = list_all(&mut w, "per_page=3", ALL);
    assert_eq!(all.len(), 7);
    assert!(all.iter().all(|t| t.get("leases").is_none()));
    let names: Vec<_> = all.iter().map(|t| t["id"].as_str().unwrap().to_owned()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert_eq!(list_all(&mut w, "requester=carol", ALL).len(), 3);
    assert_eq!(list_all(&mut w, "requester=nobody", ALL).len(), 0);
    assert_eq!(list_all(&mut w, "state=queued", ALL).len(), 7);
    assert_eq!(list_all(&mut w, &format!("app_id={}", MCP.0.0), ALL).len(), 0);
    let f = w.fold();
    let r = Repo { host: &h, at: at(REPO), core: &f.core, meta: &f.meta, dir: &d };
    let filter = tasks::parse_filter("per_page=2", &|_| None).unwrap();
    let page = tasks::list(&r, &w.ident.owner, w.now, &filter, ALL, "/repos/o/r7/tasks");
    let link = page.header("link").expect("more remain");
    assert!(
        link.starts_with("<https://graftyard.example/api/graftyard/v1/repos/o/r7/tasks?per_page=2&after="),
        "{link}"
    );
    assert!(link.ends_with("rel=\"next\""));
    assert_eq!(
        tasks::list(&r, &w.ident.owner, w.now, &filter, Reader { metadata: false, contents: false }, "/x").status,
        404
    );
    assert_eq!(tasks::parse_filter("state=bogus", &|_| None).unwrap_err().status, 422);
    assert_eq!(tasks::parse_filter("after=zz", &|_| None).unwrap_err().status, 422);
    // `/log` and the session page (AM3-WP4, L0-15): 404 without `metadata: read` or for no such task, 403 with
    // `metadata: read` alone, and for a reader with `contents: read` 404 until the log is pushed, then the log.
    let meta_only = Reader { metadata: true, contents: false };
    assert_eq!(tasks::log_reader(&r, &id, Reader { metadata: false, contents: false }).unwrap_err().status, 404);
    assert_eq!(tasks::log_reader(&r, "3.nope", ALL).unwrap_err().status, 404);
    assert_eq!(tasks::log_reader(&r, &id, meta_only).unwrap_err().status, 403);
    let sid = tasks::log_reader(&r, &id, ALL).expect("a reader with contents: read");
    assert_eq!(tasks::log(None).status, 404);
    assert_eq!(tasks::session_page(&r, None, false).status, 404);
    // `?fields=prompt` without `contents: read` is 403; a plain read is 200 without `prompt`.
    let f2 = w.fold();
    let r2 = Repo { host: &h, at: at(REPO), core: &f2.core, meta: &f2.meta, dir: &d };
    assert_eq!(tasks::get(&r2, &w.ident.owner, w.now, &id, "fields=prompt", meta_only).status, 403);
    assert_eq!(tasks::get(&r2, &w.ident.owner, w.now, &id, "per_page=1&fields=prompt", ALL).status, 200);
    let plain = tasks::get(&r2, &w.ident.owner, w.now, &id, "", meta_only);
    assert!(plain.status == 200 && json_of(&plain).get("prompt").is_none());
    session_log_reads(&r2, &sid);
    // Refusals render as the reference names them.
    use graftyard_git::task::TaskErr;
    let status = |e: TaskErr| tasks::refused(&IssueErr::Task(e)).status;
    assert_eq!(status(TaskErr::SessionExists), 422);
    assert_eq!(status(TaskErr::NotYours), 403);
    assert_eq!(status(TaskErr::Final(TaskApiState::Completed)), 409);
    assert_eq!(
        json_of(&tasks::refused(&IssueErr::Task(TaskErr::Final(TaskApiState::Completed))))["state"],
        "completed"
    );
    let exists = tasks::refused(&IssueErr::Task(TaskErr::SessionExists));
    assert_eq!(json_of(&exists)["errors"][0]["code"], "already_exists");
    // The owner fold read through `AuthzRead` is what decides the derived state.
    let _: &dyn AuthzRead = &w.ident.owner;
}

// ---- `command-rows` and `leader-render` (`decisions/DECISIONS.md`) ----

/// The rows a member answers to a command read (`TierRows::command_rows`), round-tripped as the edge parses them, and
/// a store of its core rows.
fn command_answer(
    w: &mut World,
    spec: &graftyard_api::hosted::CommandSpec,
) -> (graftyard_api::hosted::TierRows, graftyard_fold::MemStore) {
    use graftyard_api::hosted::{TierRows, accounts};
    let f = w.fold();
    let named = || accounts(&f.core, &f.meta);
    let t = TierRows::command_rows(&f.core, Some(&f.meta), 9, spec, &named);
    let t = TierRows::from_value(&serde_json::to_value(&t).unwrap()).unwrap();
    let core = TierRows::store(&t.core);
    (t, core)
}

/// A task or lease command reads the read gate's rows, its one task's `agent_task` row, and that task's leases, and
/// never the metadata tier: on main the edge read both whole tiers for these commands (`read`, `read_core`), and a
/// lease command dropped the metadata tier after streaming it. A command that names no task reads no task row. The
/// logins a task command's reply names are the ones a whole-tier read of the same tiers named: the requester and a
/// user creator, and a create's requester the edge resolved; an account the tiers do not name is not one.
#[test]
fn a_command_reads_its_task_and_the_gate_not_the_tiers() {
    use graftyard_api::hosted::CommandSpec;
    use graftyard_api::narrow::{LOGIN_SHARDS, LOGINS};
    let mut w = World::new();
    w.flag(claude_row());
    named_by_an_issue(&mut w, &[WRITER, READER]);
    w.commit_all(vec![issue(app(CLAUDE), create("a", WRITER)), issue(app(CLAUDE), create("b", WRITER2))]);
    let lease = IssueOp::AcquireLease(graftyard_git::lease::NewLease {
        paths: vec!["pkg/**".into()],
        refs: Vec::new(),
        mode: graftyard_git::lease::LeaseMode::Exclusive,
        ttl_seconds: None,
        intent: "x".into(),
        session: Some("a".into()),
    });
    w.commit(issue(app(CLAUDE), lease)).expect("a lease");
    let other = IssueOp::AcquireLease(graftyard_git::lease::NewLease {
        paths: vec!["docs/**".into()],
        refs: Vec::new(),
        mode: graftyard_git::lease::LeaseMode::Exclusive,
        ttl_seconds: None,
        intent: "y".into(),
        session: Some("b".into()),
    });
    w.commit(issue(app(CLAUDE), other)).expect("another task's lease");
    let a = SessionId::parse(&format!("{}.a", CLAUDE.0.0), None).unwrap();
    let spec = CommandSpec { session: Some(a.clone()), loaded: true, accounts: vec![WRITER2], owner: OWNER };
    let (t, core) = command_answer(&mut w, &spec);
    assert!(t.complete);
    assert_eq!(t.meta, Some(Vec::new()), "no metadata row, and the tier is loaded");
    let f = w.fold();
    let gate = graftyard_api::hosted::TierRows::access_rows(&f.core);
    assert!(!gate.is_empty());
    assert!(gate.iter().all(|r| t.core.contains(r)), "the read gate's rows");
    assert!(
        t.core
            .iter()
            .all(|(t, _, _)| ["settings", "collaborator", "agent_task", "lease_by_session"].contains(&t.as_str())),
        "no other core row: no refs, pull heads, or other tasks"
    );
    assert!(graftyard_fold::Read::prefix(&f.core, "agent_task", &[]).len() == 2, "two tasks on the whole tier");
    assert_eq!(task_rules::task_core(&core, &a), task_rules::task_core(&*f.core, &a), "the task's own row");
    assert_eq!(t.core.iter().filter(|(t, _, _)| t == "agent_task").count(), 1, "only the named task");
    assert_eq!(graftyard_git::lease::session_rows(&core, &a).len(), 0, "the lease rows themselves are not read");
    assert_eq!(t.core.iter().filter(|(t, _, _)| t == "lease_by_session").count(), 1, "not task b's lease");
    // `lease::marks` reads the same for an update of the task on these rows as on the whole tiers.
    let patch: IssueOp<graftyard_git::blob::BlobRef> =
        IssueOp::UpdateTask { id: a.clone(), state: None, pr: None, summary: Some("s".into()) };
    assert!(graftyard_git::lease::marks(&patch, &core));
    assert_eq!(graftyard_git::lease::marks(&patch, &core), graftyard_git::lease::marks(&patch, &*f.core));
    // The logins the reply may name, as a whole-tier read knew them: of the task's requester and the create's
    // (WRITER2), those the tiers name. WRITER2 is named by task b's requester alone, which a whole-tier read now reads
    // too (F138), and READER, named but not asked, is not read.
    let picks = graftyard_api::narrow::Picks::of(t.picks.clone());
    let named = picks.login_accounts(None, AccountId(DbId(OWNER))).unwrap();
    assert_eq!(named, vec![AccountId(DbId(WRITER)), AccountId(DbId(WRITER2))]);
    assert!(t.picks.as_ref().unwrap().iter().any(|(k, _)| k == LOGIN_SHARDS));
    assert!(t.picks.as_ref().unwrap().iter().any(|(k, _)| k == LOGINS));
    drop(f);
    // A lease command and a token claim name no task row's account: no picks but the empty ones, and the edge reads
    // only the viewer's login. A command that names no task reads none.
    let (t, _) = command_answer(&mut w, &CommandSpec { owner: OWNER, ..CommandSpec::default() });
    assert!(t.core.iter().all(|(t, _, _)| t == "settings" || t == "collaborator"));
    let picks = graftyard_api::narrow::Picks::of(t.picks.clone());
    assert_eq!(picks.login_accounts(None, AccountId(DbId(OWNER))), Some(Vec::new()));
    // A member whose metadata tier is loading answers no metadata, and a command that needs it gets no picks.
    let f = w.fold();
    let named = || graftyard_api::hosted::accounts(&f.core, &f.meta);
    let loading = graftyard_api::hosted::TierRows::command_rows(&f.core, None, 9, &spec, &named);
    assert_eq!((loading.meta, loading.picks), (None, None));
}

/// A task `PATCH`'s reply, rendered on the leader over the logins the edge read from a command answer's picks, is the
/// bytes it renders over the logins a whole-tier read gave the edge: every account a task names is either one the
/// tiers name or one the edge asked for. The no-response-change invariant: the same `requester` login, the same
/// `creator`, and the agent's own fields.
#[test]
fn a_patch_reply_over_the_command_logins_is_the_whole_tier_reply() {
    use graftyard_api::hosted::{CommandSpec, HostedDir, Render, What, accounts, fill_authz, render_ok};
    let mut w = World::new();
    w.flag(claude_row());
    named_by_an_issue(&mut w, &[WRITER]);
    w.commit_all(vec![issue(app(CLAUDE), create("a", WRITER)), issue(app(CLAUDE), create("b", WRITER2))]);
    let login = |id: i64| format!("login{id}");
    // b's requester, WRITER2, is named by no row but its task's: it renders its login, not `ghost` (F138).
    for (name, requester) in [("a", login(WRITER)), ("b", login(WRITER2))] {
        let id = SessionId::parse(&format!("{}.{name}", CLAUDE.0.0), None).unwrap();
        let patch = IssueOp::UpdateTask { id: id.clone(), state: None, pr: None, summary: Some("s".into()) };
        let acked = w.commit_all_acked(vec![issue(app(CLAUDE), patch)]).pop().unwrap().expect("patched");
        let acked = acked.map(|o| match o {
            RepoOut::Issue(c) => c.out.clone(),
            x => panic!("{x:?}"),
        });
        // The whole-tier edge (`read_logins` on main): every account the tiers name, but the owner from the path.
        let f = w.fold();
        let mut whole: Vec<AccountId> = accounts(&f.core, &f.meta).into_iter().collect();
        whole.retain(|a| a.0.0 != OWNER);
        drop(f);
        // The command edge: the accounts its picks name.
        let spec = CommandSpec { session: Some(id), loaded: true, accounts: Vec::new(), owner: OWNER };
        let (t, _) = command_answer(&mut w, &spec);
        let picked = graftyard_api::narrow::Picks::of(t.picks).login_accounts(None, AccountId(DbId(OWNER))).unwrap();
        let f = w.fold();
        let render = |ids: &[AccountId]| {
            let mut dir = HostedDir { home: Some(at(REPO)), authz: None, ..HostedDir::default() };
            dir.logins = ids.iter().map(|a| (a.0.0, login(a.0.0))).collect();
            dir.logins.insert(OWNER, "o".into());
            let what = What::Task { created: false, contents: true };
            let mut r = Render { host: "graftyard.example".into(), isolation: false, dir, what, doc: None };
            fill_authz(&mut r, Some(&w.ident.owner));
            render_ok(&r, &acked, &f.core, &f.meta)
        };
        let (by_whole, by_command) = (render(&whole), render(&picked));
        assert_eq!(by_whole.status, 200);
        assert_eq!(serde_json::to_vec(&by_command).unwrap(), serde_json::to_vec(&by_whole).unwrap(), "{name}");
        let body = by_command.body.unwrap();
        assert_eq!(body["requester"]["login"].as_str(), Some(requester.as_str()));
        assert_eq!(body["summary"], "s");
        assert_eq!(body["creator"], json!({ "type": "App", "id": CLAUDE.0.0 }));
        assert_eq!(body["state"], "queued");
    }
}

/// Issues opened by `authors`, so the tiers name them (`hosted::accounts`).
fn named_by_an_issue(w: &mut World, authors: &[i64]) {
    for a in authors {
        let op =
            IssueOp::OpenIssue { title: "t".into(), body: "b".into(), labels: smallvec![], assignees: smallvec![] };
        w.commit(issue(user(*a), op)).expect("an issue");
    }
}

/// `leader-render`: an issue command whose render carries no owner `Authz` gets the leader's `Authz` dependency of its
/// entry in its output, one fold for every command of the entry, and its reply, filled from it, is the bytes the
/// edge's render with the same fold gives. A render that carries one keeps it; a replay carries none. The render
/// without the fold is about the fold's hex shorter, so `ENTRY_MAX`, which counts `encoded_len`, no longer counts it.
#[test]
fn a_leader_rendered_reply_is_the_edge_rendered_reply() {
    use graftyard_api::hosted::{HostedDir, Render, What, fill_authz, render_ok};
    let mut w = World::new();
    w.flag(claude_row());
    let render = |authz: Option<AuthzFold>, what: What| Render {
        host: "graftyard.example".into(),
        isolation: false,
        dir: HostedDir { home: Some(at(REPO)), authz, ..HostedDir::default() },
        what,
        doc: None,
    };
    let call = |op, r: &Render, rid| {
        let mut c = IssueCall::new(op, app(CLAUDE), RequestId(Ulid(rid)), serde_json::to_value(r).unwrap());
        c.agent = false;
        RepoCmd::Issue(Box::new(c))
    };
    let created = What::Task { created: true, contents: true };
    let bare = render(None, created.clone());
    let edge = render(Some(w.ident.owner.clone()), created.clone());
    let (with, without) = (call(create("a", WRITER), &edge, 1), call(create("b", WRITER), &bare, 2));
    // ENTRY_MAX counts `encoded_len`: the bare render is shorter by about the fold's hex.
    use graftyard_raft::Record;
    let hex = graftyard_raft::codec::to_bytes(&w.ident.owner).len() * 2;
    assert!(hex > 100, "the owner's fold is not trivial");
    assert!(with.encoded_len() >= without.encoded_len() + hex - 8, "{} {}", with.encoded_len(), without.encoded_len());
    let acks = w.commit_all_acked(vec![with, without, call(create("c", WRITER2), &bare, 3)]);
    let outs: Vec<_> = acks
        .into_iter()
        .map(|a| {
            let a = a.expect("created");
            match &a.out {
                RepoOut::Issue(c) => (a.map(|_| c.out.clone()), c.authz.clone()),
                x => panic!("{x:?}"),
            }
        })
        .collect();
    assert!(outs[0].1.is_none(), "an edge render with its own fold gets none");
    let dep = outs[1].1.clone().expect("the leader's Authz dependency");
    assert_eq!(*dep, w.ident.owner, "the owner's fold, the one the edge would have sent");
    assert!(std::rc::Rc::ptr_eq(&dep, outs[2].1.as_ref().unwrap()), "one fold for every command decided against it");
    let f = w.fold();
    for (acked, authz) in &outs[1..] {
        let mut by_leader = bare.clone();
        assert_eq!(fill_authz(&mut by_leader, authz.as_deref()), Some("dep"));
        let leader = render_ok(&by_leader, acked, &f.core, &f.meta);
        let edge_reply = render_ok(&edge, acked, &f.core, &f.meta);
        assert_eq!(leader.status, 201);
        assert_eq!(serde_json::to_vec(&leader).unwrap(), serde_json::to_vec(&edge_reply).unwrap());
    }
    let mut kept = edge.clone();
    assert_eq!(fill_authz(&mut kept, Some(&AuthzFold::new(AccountId(DbId(1))))), Some("render"));
    assert_eq!(kept.dir.authz.as_ref(), Some(&w.ident.owner), "the edge's own fold wins");
    // Unfilled, a task's reply is the 503 it was before: the fold matters to the reply.
    let none = render_ok(&bare, &outs[1].0, &f.core, &f.meta);
    assert_eq!(none.status, 503);
    // A replay (a duplicate answered from its `request` row) carries no dependency.
    let RepoCmd::Issue(c) = call(create("b", WRITER), &bare, 2) else { unreachable!() };
    assert!(c.render_lacks_authz());
    assert!(c.replay(&[]).authz.is_none());
}

/// AM3-WP3's `graftyard_task` delivery: a human writer's Graftyard-run create renders the API reference's payload
/// (`action`, `task` with its prompt, `repository`, `sender`) with the App's `installation`, and the controller reads
/// back the task it starts from that payload alone (`Task::of_delivery`): its id, base, requester, and in-repo
/// workspace. An external task's create carries no `graftyard_task`.
#[test]
fn the_graftyard_task_payload_is_what_the_controller_starts_from() {
    use graftyard_git::controller::{Delivery, TASK_WALL, Task, TaskBudget, Workspace};
    use graftyard_git::hooks::{Subject, TASK_EVENT, webhooks};
    use graftyard_identity::WorkspaceMode::InRepo;
    let mut w = World::new();
    register_own(&mut w, RUN, "graftyard-agent");
    w.flag(AgentApp { app: RUN, kind: AgentKind::GraftyardRun, confine: None });
    const WS: AppId = AppId(DbId(8));
    register_own(&mut w, WS, "graftyard-workspace");
    w.flag(AgentApp { app: WS, kind: AgentKind::GraftyardRun, confine: None });
    let c = claims(WS);
    let row = InstallationRow { app: WS, repos: c.repos.clone(), permissions: everything() };
    w.ident.owner.apply(&AuthzEvent::Installation { id: c.installation, row: Some(row), not_before: Timestamp(0) });
    w.ident.owner.apply(&AuthzEvent::OrgAgents { enabled: true, workspace: InRepo });
    w.flag(claude_row());
    w.commit(issue(app(CLAUDE), create("ext", WRITER))).unwrap();
    let op = IssueOp::CreateRun(task_rules::NewRun {
        issue: None,
        prompt: Some("fix the flaky test".into()),
        summary: "".into(),
    });
    let out = w.commit(RepoCmd::Issue(Box::new(IssueCall::new(op, user(WRITER), RequestId(Ulid(0x51)), Value::Null))));
    let Ok(RepoOut::Issue(o)) = out else { panic!("{out:?}") };
    let IssueOut::Task(id) = o.out else { panic!() };
    let f = w.fold();
    let (h, d) = (host(), Dir);
    let r = Repo { host: &h, at: at(REPO), core: &f.core, meta: &f.meta, dir: &d };
    let l = w.repo.leader();
    let mut bodies = Vec::new();
    for e in w.repo.members[l.0 as usize].log() {
        for hook in webhooks(&f.core, REPO, e).into_iter().filter(|x| x.event == TASK_EVENT) {
            assert!(matches!(&hook.subject, Subject::Task { session, .. } if *session == id), "only the run task's");
            let v = graftyard_api::webhooks::payload(&r, &hook, None, e.at).expect("a payload");
            bodies.push(graftyard_api::webhooks::with_installation(v, claims(RUN).installation));
        }
    }
    let [p] = bodies.as_slice() else { panic!("one delivery, got {}", bodies.len()) };
    assert_eq!(p["action"], "created");
    assert_eq!((p["task"]["id"].as_str(), p["task"]["kind"].as_str()), (Some(&*id.text()), Some("graftyard_run")));
    assert_eq!(p["task"]["prompt"], "fix the flaky test");
    assert_eq!(p["task"]["app_id"], RUN.0.0);
    assert_eq!(p["sender"]["login"], "alice");
    assert_eq!(p["installation"]["id"], claims(RUN).installation.0.0);
    assert_eq!(p["repository"]["full_name"], "o/r7");
    let budget = TaskBudget { task_tokens: 10, org_month_tokens: 100, wall: TASK_WALL };
    let (action, t) = Task::of_delivery(p, InRepo, budget).expect("the controller's task");
    assert_eq!(action, Delivery::Created);
    assert_eq!(
        (t.id.clone(), t.requester, t.workspace.clone()),
        (id.clone(), UserId(AccountId(DbId(WRITER))), Workspace::InRepo)
    );
    assert_eq!((t.base.owner.as_str(), t.base.name.as_str()), ("o", "r7"));
    assert_eq!(t.branch, task_rules::run_prefix(&id).unwrap());
    // A payload for an external task, or with the task's state moved on, starts nothing.
    let mut ext = p.clone();
    ext["task"]["kind"] = json!("external");
    assert!(Task::of_delivery(&ext, InRepo, budget).is_none());
    let mut late = p.clone();
    late["task"]["state"] = json!("in_progress");
    assert!(Task::of_delivery(&late, InRepo, budget).is_none());
    let mut cancel = p.clone();
    cancel["action"] = json!("cancelled");
    assert_eq!(Task::of_delivery(&cancel, InRepo, budget).map(|x| x.0), Some(Delivery::Cancelled));
}

/// L0-14's off phase as the edge serves `POST .../tasks` without `kind`: the session header's refusal
/// (`tasks::agent_session_create`), then the commit, whose refusal `tasks::refused` renders. While `agents_enabled` is
/// off every creator answers 422, the session header's included (F151); once on, the header's create is the
/// non-writer's 403, an App without `issues: write` gets 403, and a plain create goes on to `decide`.
#[test]
fn every_graftyard_run_create_answers_422_while_agents_are_off() {
    let mut w = World::new();
    let run =
        || IssueOp::CreateRun(task_rules::NewRun { issue: None, prompt: Some("fix it".into()), summary: "".into() });
    let edge = |w: &mut World, who: Principal, session: bool| -> u16 {
        if let Some(r) = tasks::agent_session_create(&run(), session, &w.ident.owner) {
            return r.status;
        }
        let rid = w.rid();
        match w.commit(RepoCmd::Issue(Box::new(IssueCall::new(run(), who, rid, Value::Null)))) {
            Err(RepoErr::Issue { err, .. }) => tasks::refused(&err).status,
            o => panic!("{o:?}"),
        }
    };
    // An installation token that reaches the repository without `issues: write`.
    let mut c = claims(PLAIN);
    c.permissions = PermissionSet(
        [("contents", Access::Read), ("metadata", Access::Read)].map(|(n, a)| (n.into(), a)).into_iter().collect(),
    );
    let no_issues = Principal::installation(&c);
    let creators = [
        (no_issues.clone(), false),
        (user(WRITER), true),
        (user(WRITER), false),
        (app(RUN), false),
        (app(CLAUDE), false),
        (user(VIA_APP), false),
        (user(VIA_MCP), false),
        (user(READER), false),
    ];
    for (who, session) in creators {
        assert_eq!(edge(&mut w, who.clone(), session), 422, "{who:?}, session header {session}");
    }
    w.ident.owner.apply(&AuthzEvent::OrgAgents { enabled: true, workspace: graftyard_identity::WorkspaceMode::InRepo });
    // Once on, the App without the permission gets GHES's 403 for it, and the header's create the non-writer's.
    assert_eq!(edge(&mut w, no_issues, false), 403);
    assert_eq!(tasks::agent_session_create(&run(), true, &w.ident.owner).map(|r| r.status), Some(403));
    assert!(tasks::agent_session_create(&run(), false, &w.ident.owner).is_none());
    let ext =
        IssueOp::CreateTask(NewTask { session: "fix-1".into(), requester: None, issue: None, summary: "".into() });
    assert!(tasks::agent_session_create(&ext, true, &w.ident.owner).is_none());
}

/// F155: an App's command names its bot account in a row its reply renders exactly when `IssueCall::names_bot` says
/// so, each committed as the edge marks it, on a repository whose tiers named no bot before. The login the edge
/// records with `name_bot` is the one the committed reply renders; without it the reply names `ghost[bot]`, as
/// F137's octokit milestone create did.
#[test]
fn an_apps_reply_names_its_bot_by_the_login_the_edge_read() {
    use graftyard_api::hosted::{HostedDir, Render, What, accounts, render_ok};
    use graftyard_git::issues::CommentTarget;
    type Op = IssueOp<graftyard_git::blob::BlobRef>;
    let bot = PLAIN.bot();
    let define =
        |n, title: &str| Op::DefineMilestone { n, title: title.into(), description: None, open: true, due_on: None };
    let open = || Op::OpenIssue { title: "t".into(), body: "b".into(), labels: smallvec![], assignees: smallvec![] };
    let comment = || Op::Comment { on: CommentTarget::Issue(IssueNumber(1)), body: "c".into(), reply_to: None };
    let status = Op::RecordStatus(StatusUpdate {
        id: StatusId(DbId(20)),
        sha: oid(2),
        context: "ci".into(),
        state: StatusState::Success,
        target_url: None,
        description: None,
        creator: bot,
    });
    let with_milestone = Op::OpenIssueWithMilestone {
        title: "t".into(),
        body: "b".into(),
        labels: smallvec![],
        assignees: smallvec![],
        milestone: 1,
    };
    let label = Op::DefineLabel { id: None, name: LabelName("bug".into()), color: [1, 2, 3], description: None };
    let assign = Op::Assign { n: IssueNumber(1), add: smallvec![AccountId(DbId(WRITER))], remove: smallvec![] };
    // (the command, whether the edge marks it REST, the reply and where it names the bot, whether it names it)
    type Case = (Op, bool, Option<(What, &'static str)>, bool);
    let cases: Vec<Case> = vec![
        (define(None, "app"), false, Some((What::MilestoneSaved { created: true }, "/creator/login")), true),
        (define(Some(1), "renamed"), false, None, false),
        (open(), true, Some((What::IssueCreated, "/user/login")), true),
        (open(), false, None, false),
        (with_milestone, false, None, true),
        (comment(), true, Some((What::IssueCommentCreated { n: 1 }, "/user/login")), true),
        (comment(), false, None, false),
        (status, false, Some((What::StatusCreated { sha: oid(2), id: 20 }, "/creator/login")), true),
        (label, false, None, false),
        (assign, false, None, false),
    ];
    for (op, rest, reply, names) in cases {
        let mut w = World::new();
        let seed = vec![issue(user(WRITER), open()), issue(user(WRITER), define(None, "seed"))];
        assert!(w.commit_all(seed).iter().all(Result::is_ok));
        let what = reply.as_ref().map_or(What::Deleted, |(w, _)| w.clone());
        let logins = [(OWNER, "o".to_owned()), (WRITER, "alice".to_owned())].into();
        let render = Render {
            host: "graftyard.example".into(),
            isolation: false,
            dir: HostedDir { home: Some(at(REPO)), logins, ..HostedDir::default() },
            what,
            doc: None,
        };
        let mut call = IssueCall::new(op, app(PLAIN), RequestId(Ulid(9)), serde_json::to_value(&render).unwrap());
        call.rest_issue = rest && matches!(call.op, Op::OpenIssue { .. });
        call.rest_comment = rest && IssueCall::issue_comment(&call.op);
        let label = format!("{:?} rest={rest}", call.op);
        assert_eq!(call.names_bot(), names.then_some(PLAIN), "{label}");
        let before = call.render.clone();
        call.name_bot("plain-app[bot]".into());
        assert_eq!(call.render != before, names, "{label}: only a command that names the bot records its login");
        let named: Render = serde_json::from_value(call.render.clone()).unwrap();
        let acked = match w.commit_all_acked(vec![RepoCmd::Issue(Box::new(call))]).pop().unwrap() {
            Ok(a) => match &a.out {
                RepoOut::Issue(c) => a.map(|_| c.out.clone()),
                x => panic!("{label}: {x:?}"),
            },
            Err(e) => panic!("{label}: {e:?}"),
        };
        let f = w.fold();
        assert_eq!(accounts(&f.core, &f.meta).contains(&bot), names, "{label}: names_bot is what decide wrote");
        if let Some((_, at)) = reply {
            let ok = render_ok(&named, &acked, &f.core, &f.meta).reply();
            assert_eq!(ok.value().pointer(at), Some(&json!("plain-app[bot]")), "{label}: {}", ok.value());
            assert_eq!(ok.value().pointer(&at.replace("login", "type")), Some(&json!("Bot")), "{label}");
            let bare: Render = serde_json::from_value(before).unwrap();
            let ghost = render_ok(&bare, &acked, &f.core, &f.meta).reply();
            assert_eq!(ghost.value().pointer(at), Some(&json!("ghost[bot]")), "{label}: without the edge's login");
        }
    }
}
