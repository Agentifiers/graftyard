//! The task API under `/api/graftyard/v1/repos/{owner}/{repo}/tasks` (`design/AGENT-LAYER.md`, "Tasks and the activity
//! feed" and "API reference"; `build/BUILD-PLAN.md`, AM1-WP3): external creation, `PATCH`, cancel, and the read
//! endpoints. Never under `/api/v3/agents/*`.
//!
//! Reads are pure functions of one `fresh` read of the repository's tiers and its owner's `Authz` fold: a task reads
//! as `effective` gives it, so an uninstalled App's tasks read `cancelled` and an idle one `timed_out` with no sweep.
//! Writes parse to the `IssueOp`s `graftyard_git::task::decide` decides, and their replies render on the leader from
//! the `&Acked` (`crate::hosted::What::Task`).

use graftyard_git::issue_ops::{IssueErr, IssueOp, IssueOut};
use graftyard_git::issues::IssueNumber;
use graftyard_git::task::{
    NewTask, SUMMARY_MAX, TaskActor, TaskApiState, TaskCore, TaskErr, TaskMeta, effective, task_core, task_ids,
    task_meta,
};
use graftyard_identity::{
    Access as AppAccess, Actor, AppId, AuthzRead, Credential, Principal, RepoPermission, SessionId, UserId,
};
use graftyard_raft::Acked;
use graftyard_types::{AccountId, DbId, Timestamp};
use graftyard_wire::{FieldError, Reply};
use serde_json::{Value, json};

use crate::view::Repo;

const DOC: &str = "graftyard/tasks";

/// A route under `PREFIX` (`crate::agents::PREFIX`) that this module serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route<'p> {
    /// `/repos/{owner}/{repo}/tasks`
    Tasks { owner: &'p str, repo: &'p str },
    /// `/repos/{owner}/{repo}/tasks/{id}`
    Task { owner: &'p str, repo: &'p str, id: &'p str },
    /// `/repos/{owner}/{repo}/tasks/{id}/cancel`
    Cancel { owner: &'p str, repo: &'p str, id: &'p str },
    /// `/repos/{owner}/{repo}/tasks/{id}/log`
    Log { owner: &'p str, repo: &'p str, id: &'p str },
}

pub fn route(rest: &str) -> Option<Route<'_>> {
    let segs: Vec<&str> = rest.strip_prefix('/')?.split('/').collect();
    match segs.as_slice() {
        ["repos", owner, repo, "tasks"] => Some(Route::Tasks { owner, repo }),
        ["repos", owner, repo, "tasks", id] => Some(Route::Task { owner, repo, id }),
        ["repos", owner, repo, "tasks", id, "cancel"] => Some(Route::Cancel { owner, repo, id }),
        ["repos", owner, repo, "tasks", id, "log"] => Some(Route::Log { owner, repo, id }),
        _ => None,
    }
}

fn invalid(field: &'static str, code: &'static str) -> Reply {
    let e = FieldError { resource: "Task", field, code };
    Reply::json(422, crate::wire::validation_error("Validation Failed", DOC, &[e]))
}

// ---- who may read what ----

/// The caller's permissions the task API checks, from the edge's authentication and the owner's `Authz`: whether it
/// may read the repository at all (`metadata: read`), and whether it may see prompts (`contents: read`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reader {
    pub metadata: bool,
    pub contents: bool,
}

impl Reader {
    /// A user's: `Read` or more on the repository gives both. An installation token's comes from its claims.
    pub fn of_user(perm: Option<RepoPermission>) -> Reader {
        let r = perm >= Some(RepoPermission::Read);
        Reader { metadata: r, contents: r }
    }

    /// The caller's, from the owner's `Authz` the edge read with the tiers: a user's role (`Reader::of_user`); an
    /// installation token reaching the repository has `metadata: read`, as every installation does, and `contents:
    /// read` when both its claims and its row grant `contents`. A public repository is readable by anyone signed in.
    pub fn of(who: Option<&Principal>, owner: &dyn AuthzRead, repo: &graftyard_git::issue_ops::Access<'_>) -> Reader {
        use graftyard_identity::RepoAccess as _;
        let Some(p) = who else { return Reader { metadata: false, contents: false } };
        let public = repo.visibility() == graftyard_identity::Visibility::Public;
        let floor = Reader { metadata: public, contents: public };
        let r = match (&p.actor, &p.credential) {
            (Actor::App { .. }, Credential::Stateless(c)) => {
                let folds = [owner];
                let reach = graftyard_identity::app_permission(c, &folds, repo).is_some();
                let contents = graftyard_identity::authorize_app(c, &[("contents", AppAccess::Read)], &folds, repo);
                Reader { metadata: reach, contents: reach && contents == graftyard_identity::Decision::Allow }
            }
            (Actor::User(_), _) => Reader::of_user(graftyard_identity::permission(p, &[owner], repo)),
            _ => floor,
        };
        Reader { metadata: r.metadata || floor.metadata, contents: r.contents || floor.contents }
    }
}

// ---- rendering ----

fn actor_json(r: &Repo<'_>, a: TaskActor) -> Value {
    match a {
        TaskActor::User(u) => crate::wire::simple_user(r.host, r.dir, Some(u.0)),
        TaskActor::App { app, .. } => json!({ "type": "App", "id": app.0.0 }),
    }
}

/// One task as `task` in the API reference, in the state it reads as at `at`. `prompt` is omitted without
/// `contents: read`. `leases` is the single GET's: the task's leases held at `at` (`crate::leases::task_leases`).
#[allow(clippy::too_many_arguments)]
pub fn task_json(
    r: &Repo<'_>,
    owner: &dyn AuthzRead,
    at: Timestamp,
    id: &SessionId,
    core: &TaskCore,
    meta: Option<&TaskMeta>,
    reader: Reader,
    single: bool,
) -> Value {
    let state = effective(core, owner, r.at.id, at);
    let web = r.host.web();
    let mut v = json!({
        "id": id.text(),
        "kind": core.kind.as_str(),
        "state": state.as_str(),
        "app_id": core.app.0.0,
        "installation_id": core.installation.0.0,
        "requester": core.requester.map_or(Value::Null, |u| crate::wire::simple_user(r.host, r.dir, Some(u.0))),
        "creator": meta.map_or(Value::Null, |m| actor_json(r, m.creator)),
        "issue_number": meta.and_then(|m| m.issue).map(|n| n.0),
        "pr_number": meta.and_then(|m| m.pr).map(|n| n.0),
        "summary": meta.map_or("", |m| &m.summary),
        "prefix": core.prefix.as_ref().map(|p| p.as_str().trim_end_matches("**").to_owned()),
        "html_url": format!("{web}/{}/{}/_graftyard/sessions/{}", r.at.owner, r.at.name, id.text()),
        "created_at": meta.map(|m| crate::wire::iso(m.created)),
        "updated_at": crate::wire::iso(core.updated),
    });
    if reader.contents {
        v["prompt"] = meta.and_then(|m| m.prompt.as_deref()).map_or(Value::Null, |p| Value::String(p.into()));
    }
    if single {
        v["leases"] = crate::leases::task_leases(r, owner, at, id);
    }
    v
}

/// `GET /tasks/{id}`.
pub fn get(r: &Repo<'_>, owner: &dyn AuthzRead, at: Timestamp, id: &str, reader: Reader) -> Reply {
    if !reader.metadata {
        return Reply::not_found();
    }
    let Some(id) = SessionId::parse(id, None) else { return Reply::not_found() };
    let Some(core) = task_core(r.core, &id) else { return Reply::not_found() };
    let meta = task_meta(r.meta, &id);
    Reply::json(200, task_json(r, owner, at, &id, &core, meta.as_ref(), reader, true))
}

/// `GET /tasks` filters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub state: Option<TaskApiState>,
    pub app: Option<AppId>,
    pub requester: Option<UserId>,
    pub issue: Option<IssueNumber>,
    pub per_page: usize,
    pub after: Option<SessionId>,
}

/// The list's query: `state`, `app_id`, `requester` (a login, resolved through `dir`), `issue_number`, `per_page`
/// (default 30, at most 100), and the opaque `after`. An unknown login matches nothing.
pub fn parse_filter(query: &str, by_login: &dyn Fn(&str) -> Option<AccountId>) -> Result<Filter, Reply> {
    let mut f = Filter { per_page: 30, ..Filter::default() };
    for (k, v) in query.split('&').filter_map(|kv| kv.split_once('=')) {
        match k {
            "state" => f.state = Some(TaskApiState::parse(v).ok_or_else(|| invalid("state", "invalid"))?),
            "app_id" => f.app = Some(AppId(DbId(v.parse().map_err(|_| invalid("app_id", "invalid"))?))),
            "requester" => f.requester = Some(UserId(by_login(v).unwrap_or(AccountId(DbId(-1))))),
            "issue_number" => f.issue = Some(IssueNumber(v.parse().map_err(|_| invalid("issue_number", "invalid"))?)),
            "per_page" => f.per_page = v.parse::<usize>().ok().filter(|n| *n > 0).unwrap_or(30).min(100),
            "after" => {
                f.after = Some(decode_after(v).ok_or_else(|| invalid("after", "invalid"))?);
            }
            _ => {}
        }
    }
    Ok(f)
}

/// The opaque `after`: the last listed task's id, hex.
fn encode_after(id: &SessionId) -> String {
    use std::fmt::Write as _;
    id.text().bytes().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn decode_after(s: &str) -> Option<SessionId> {
    let bytes: Option<Vec<u8>> =
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect();
    SessionId::parse(std::str::from_utf8(&bytes?).ok()?, None)
}

/// `GET /tasks`: the tasks that match `f`, in id order, each in the state it reads as at `at`, with GitHub's `Link`
/// header while more remain.
pub fn list(r: &Repo<'_>, owner: &dyn AuthzRead, at: Timestamp, f: &Filter, reader: Reader, path: &str) -> Reply {
    if !reader.metadata {
        return Reply::not_found();
    }
    let mut out = Vec::new();
    let mut last = None;
    let mut more = false;
    for id in task_ids(r.core, f.after.as_ref(), usize::MAX) {
        let Some(core) = task_core(r.core, &id) else { continue };
        let meta = task_meta(r.meta, &id);
        let keep = f.state.is_none_or(|s| effective(&core, owner, r.at.id, at) == s)
            && f.app.is_none_or(|a| core.app == a)
            && f.requester.is_none_or(|u| core.requester == Some(u))
            && f.issue.is_none_or(|n| meta.as_ref().and_then(|m| m.issue) == Some(n));
        if !keep {
            continue;
        }
        if out.len() == f.per_page {
            more = true;
            break;
        }
        out.push(task_json(r, owner, at, &id, &core, meta.as_ref(), reader, false));
        last = Some(id);
    }
    let mut reply = Reply::json(200, Value::Array(out));
    if more && let Some(l) = last {
        let root = format!("{}{}", r.host.web(), crate::agents::PREFIX);
        let next = format!("<{root}{path}?per_page={}&after={}>; rel=\"next\"", f.per_page, encode_after(&l));
        reply = reply.with("link", next);
    }
    reply
}

/// `GET /tasks/{id}/log`: the `SessionLog` is pushed under `refs/graftyard/sessions/<id>` (AM3), so until then 404.
pub fn log(r: &Repo<'_>, id: &str, reader: Reader) -> Reply {
    let found = SessionId::parse(id, None).and_then(|id| task_core(r.core, &id));
    if !reader.contents || found.is_none() {
        return Reply::not_found();
    }
    Reply::error(404, "The session log has not been pushed", DOC)
}

// ---- writes ----

/// `POST /tasks`. Only `kind: external` is served here: a Graftyard-run task (`issue_number` or `prompt` without
/// `kind`) is AM3-WP3's, which also checks `agents_enabled`, so it is 422 with the message the reference gives while
/// `agents_enabled` is off. `requester` is a login the edge resolves.
pub fn parse_create(body: &Value, requester: Option<UserId>) -> Result<NewTask, Reply> {
    let str_of = |k: &'static str| -> Result<Option<&str>, Reply> {
        match body.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(invalid(k, "invalid")),
        }
    };
    match str_of("kind")? {
        Some("external") => {}
        Some(_) => return Err(invalid("kind", "invalid")),
        None => {
            return Err(Reply::error(422, "Graftyard-run tasks are not enabled for this organization", DOC));
        }
    }
    if body.get("prompt").is_some_and(|p| !p.is_null()) {
        return Err(invalid("prompt", "invalid"));
    }
    let session = str_of("session")?.ok_or_else(|| invalid("session", "missing_field"))?;
    let summary = str_of("summary")?.unwrap_or("");
    if summary.len() > SUMMARY_MAX {
        return Err(invalid("summary", "invalid"));
    }
    let issue = match body.get("issue_number") {
        None | Some(Value::Null) => None,
        Some(v) => Some(IssueNumber(
            v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| invalid("issue_number", "invalid"))?,
        )),
    };
    if str_of("requester")?.is_some() && requester.is_none() {
        return Err(invalid("requester", "invalid"));
    }
    Ok(NewTask { session: session.into(), requester, issue, summary: summary.into() })
}

/// `PATCH /tasks/{id}` with `{state?, pr_number?, summary?}`.
pub fn parse_patch(id: SessionId, body: &Value) -> Result<IssueOp<graftyard_git::blob::BlobRef>, Reply> {
    let state = match body.get("state") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(TaskApiState::parse(s).ok_or_else(|| invalid("state", "invalid"))?),
        Some(_) => return Err(invalid("state", "invalid")),
    };
    if matches!(state, Some(TaskApiState::Cancelled | TaskApiState::TimedOut)) {
        return Err(invalid("state", "invalid"));
    }
    let pr = match body.get("pr_number") {
        None | Some(Value::Null) => None,
        Some(v) => Some(IssueNumber(
            v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| invalid("pr_number", "invalid"))?,
        )),
    };
    let summary = match body.get("summary") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.len() <= SUMMARY_MAX => Some(s.as_str().into()),
        Some(_) => return Err(invalid("summary", "invalid")),
    };
    Ok(IssueOp::UpdateTask { id, state, pr, summary })
}

/// The task id in a path: the UUID, `<app_id>.<name>`, or a bare name the calling App scopes.
pub fn path_id(id: &str, caller: Option<AppId>) -> Option<SessionId> {
    SessionId::parse(id, caller)
}

/// The reply to a committed task command, rendered on the leader from its `&Acked` and its tiers after the entry
/// applied: 201 for a create, 200 with the task otherwise, and 409 with the state when the command found the task
/// final by derivation and recorded it.
pub fn render_ok(
    r: &Repo<'_>,
    owner: &dyn AuthzRead,
    at: Timestamp,
    acked: &Acked<IssueOut>,
    created: bool,
    reader: Reader,
) -> Reply {
    match &acked.out {
        IssueOut::Task(id) => {
            let Some(core) = task_core(r.core, id) else { return Reply::error(500, "no task row", DOC) };
            let meta = task_meta(r.meta, id);
            Reply::json(
                if created { 201 } else { 200 },
                task_json(r, owner, at, id, &core, meta.as_ref(), reader, true),
            )
        }
        IssueOut::TaskFinal(_, s) => final_state(*s),
        _ => Reply::error(500, "not a task reply", DOC),
    }
}

/// 409 `{message, state}` for a final task.
fn final_state(s: TaskApiState) -> Reply {
    let mut v = graftyard_wire::basic_error(409, &format!("The task is {}", s.as_str()), DOC);
    v["state"] = json!(s.as_str());
    Reply::json(409, v)
}

/// The reply to a refused task command.
pub fn refused(e: &IssueErr) -> Reply {
    match e {
        IssueErr::Unauthorized(_) => Reply::error(401, "Bad credentials", "rest"),
        IssueErr::Forbidden => Reply::error(403, "Resource not accessible by integration", DOC),
        IssueErr::NotFound => Reply::not_found(),
        IssueErr::NeedIdBlock | IssueErr::Unavailable => {
            Reply::error(503, "Service Unavailable", "rest").with("retry-after", "5")
        }
        IssueErr::Task(t) => match t {
            TaskErr::NotAgentApp => {
                Reply::error(403, "Only an agent App's installation token creates an external task", DOC)
            }
            TaskErr::NotYours => Reply::error(403, "Must be the task's App, its requester, or a repository admin", DOC),
            TaskErr::NoTask => Reply::not_found(),
            TaskErr::BadSession => invalid("session", "invalid"),
            TaskErr::SessionExists => invalid("session", "already_exists"),
            TaskErr::Cap { repo: true } => Reply::error(422, "The repository has too many open tasks", DOC),
            TaskErr::Cap { repo: false } => Reply::error(422, "The requester has too many open tasks", DOC),
            TaskErr::Invalid(f) => invalid(f, "invalid"),
            TaskErr::HostState => invalid("state", "invalid"),
            TaskErr::Final(s) => final_state(*s),
        },
        _ => Reply::error(422, "Unprocessable Entity", DOC),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_parse_only_the_reference_paths_and_never_v3() {
        assert_eq!(route("/repos/o/r/tasks"), Some(Route::Tasks { owner: "o", repo: "r" }));
        assert_eq!(route("/repos/o/r/tasks/3.fix"), Some(Route::Task { owner: "o", repo: "r", id: "3.fix" }));
        assert_eq!(route("/repos/o/r/tasks/x/cancel"), Some(Route::Cancel { owner: "o", repo: "r", id: "x" }));
        assert_eq!(route("/repos/o/r/tasks/x/log"), Some(Route::Log { owner: "o", repo: "r", id: "x" }));
        assert_eq!(route("/repos/o/r/tasks/x/y"), None);
        assert_eq!(route("/agents/tasks"), None);
    }

    #[test]
    fn bodies_parse_as_the_reference_names_them() {
        let n = parse_create(&json!({ "kind": "external", "session": "fix-1", "summary": "s" }), None).unwrap();
        assert_eq!((&*n.session, &*n.summary), ("fix-1", "s"));
        assert_eq!(parse_create(&json!({ "kind": "external" }), None).unwrap_err().status, 422);
        assert_eq!(parse_create(&json!({ "issue_number": 3 }), None).unwrap_err().status, 422, "graftyard_run");
        assert_eq!(parse_create(&json!({ "kind": "x", "session": "a" }), None).unwrap_err().status, 422);
        let id = SessionId::parse("3.fix", None).unwrap();
        assert!(matches!(
            parse_patch(id.clone(), &json!({ "state": "in_progress", "pr_number": 4 })),
            Ok(IssueOp::UpdateTask { state: Some(TaskApiState::InProgress), pr: Some(IssueNumber(4)), .. })
        ));
        assert_eq!(parse_patch(id.clone(), &json!({ "state": "cancelled" })).unwrap_err().status, 422);
        assert_eq!(parse_patch(id, &json!({ "state": "timed_out" })).unwrap_err().status, 422);
        let after = SessionId::parse("12.a-b", None).unwrap();
        assert_eq!(decode_after(&encode_after(&after)), Some(after));
    }
}
