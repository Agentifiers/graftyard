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

fn get(w: &mut World, id: &str, reader: Reader) -> Reply {
    let f = w.fold();
    let (h, d) = (host(), Dir);
    let r = Repo { host: &h, at: at(w.current), core: &f.core, meta: &f.meta, dir: &d };
    tasks::get(&r, &w.ident.owner, w.now, id, reader)
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
    // `/log`: 404 until AM3 pushes the session log, and 404 without `contents: read`.
    assert_eq!(tasks::log(&r, &id, ALL).status, 404);
    assert_eq!(tasks::log(&r, &id, Reader { metadata: true, contents: false }).status, 404);
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
