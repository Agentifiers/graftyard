//! The lease API and the activity feed under `/api/graftyard/v1/repos/{owner}/{repo}` (`design/AGENT-LAYER.md`,
//! "Leases", "Tasks and the activity feed", and "API reference"; `build/BUILD-PLAN.md`, AM1-WP4). Never under
//! `/api/v3/agents/*`.
//!
//! Reads are pure functions of one read of the repository's core tier and its owner's `Authz`: a lease is listed only
//! while `LeaseView::holds` says it is held, so an expired lease, one whose holder lost push access, and one whose
//! task went final are absent with no sweep. Writes parse to the `IssueOp`s `graftyard_git::lease::decide` decides,
//! and their replies render on the leader from the `&Acked` (`crate::hosted::What::Lease`). Every free-text value
//! (`intent`, a pull request's title) is a `TrustView`.

use graftyard_git::feed::{self, ActivityEvent, FeedCursor, FeedQuery, QueryErr, Start};
use graftyard_git::issue_ops::{IssueErr, IssueOp, IssueOut};
use graftyard_git::lease::{
    ActorId, INTENT_MAX, Lease, LeaseErr, LeaseId, LeaseMode, LeaseRow, LeaseView, NewLease, PATHS_PER_LEASE,
    REFS_PER_LEASE, lease_row,
};
use graftyard_identity::{AuthzRead, glob::PathGlob};
use graftyard_raft::Acked;
use graftyard_types::{Index, Timestamp};
use graftyard_wire::{FieldError, Reply};
use serde_json::{Value, json};

use crate::view::Repo;

const DOC: &str = "graftyard/leases";
const FEED_DOC: &str = "graftyard/activity";

/// A route under `PREFIX` (`crate::agents::PREFIX`) that this module serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route<'p> {
    /// `/repos/{owner}/{repo}/leases`
    Leases { owner: &'p str, repo: &'p str },
    /// `/repos/{owner}/{repo}/leases/{id}`
    Lease { owner: &'p str, repo: &'p str, id: &'p str },
    /// `/repos/{owner}/{repo}/leases/{id}/renew`
    Renew { owner: &'p str, repo: &'p str, id: &'p str },
    /// `/repos/{owner}/{repo}/activity`
    Activity { owner: &'p str, repo: &'p str },
}

pub fn route(rest: &str) -> Option<Route<'_>> {
    let segs: Vec<&str> = rest.strip_prefix('/')?.split('/').collect();
    match segs.as_slice() {
        ["repos", owner, repo, "leases"] => Some(Route::Leases { owner, repo }),
        ["repos", owner, repo, "leases", id] => Some(Route::Lease { owner, repo, id }),
        ["repos", owner, repo, "leases", id, "renew"] => Some(Route::Renew { owner, repo, id }),
        ["repos", owner, repo, "activity"] => Some(Route::Activity { owner, repo }),
        _ => None,
    }
}

fn invalid(field: &'static str, code: &'static str) -> Reply {
    let e = FieldError { resource: "Lease", field, code };
    Reply::json(422, crate::wire::validation_error("Validation Failed", DOC, &[e]))
}

// ---- rendering ----

/// One lease as `lease` in the API reference. `intent` is a `TrustView` of the holder's text.
pub fn lease_json(r: &Repo<'_>, row: &LeaseRow, at: Timestamp) -> Value {
    let l = &row.lease;
    let (actor, who, is_bot) = match l.holder.actor {
        ActorId::User(u) => (json!({ "type": "User", "id": u.0.0.0 }), Some(u.0), false),
        ActorId::App { app, .. } => (json!({ "type": "App", "id": app.0.0 }), None, true),
    };
    let intent = crate::trust::TrustView::of(r, &l.intent, who, is_bot, l.holder.app.is_some());
    json!({
        "id": l.id.text(),
        "holder": {
            "actor": actor,
            "app_id": l.holder.app.map(|a| a.0.0),
            "session": l.holder.session.as_ref().map(|s| s.text()),
        },
        "paths": l.scope.paths.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
        "refs": l.scope.refs.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
        "mode": l.mode_at(at).as_str(),
        "plan_block": row.plan.is_some(),
        "intent": intent,
        "granted_at": crate::wire::iso(l.granted),
        "expires_at": crate::wire::iso(l.expires),
    })
}

/// The leases the single task GET lists: every lease of the task that is held at `at`.
pub fn task_leases(r: &Repo<'_>, owner: &dyn AuthzRead, at: Timestamp, s: &graftyard_identity::SessionId) -> Value {
    let v = LeaseView { core: r.core, owner, repo: r.at.id, at };
    Value::Array(v.session_live(s).iter().map(|row| lease_json(r, row, at)).collect())
}

fn query_pairs(query: &str) -> impl Iterator<Item = (&str, &str)> {
    query.split('&').filter(|kv| !kv.is_empty()).map(|kv| kv.split_once('=').unwrap_or((kv, "")))
}

/// `GET /leases?paths=&per_page=&after=`: the live leases, in id order, those overlapping any `paths` when given, at
/// most `per_page` (default 30, at most 100), with GitHub's `Link` while more remain. Needs `contents: read`, as the
/// feed's path fields do.
pub fn list(r: &Repo<'_>, owner: &dyn AuthzRead, at: Timestamp, query: &str, contents: bool, path: &str) -> Reply {
    if !contents {
        return Reply::not_found();
    }
    let (mut paths, mut per_page, mut after) = (Vec::new(), 30usize, None);
    for (k, v) in query_pairs(query) {
        match k {
            "paths" | "paths[]" => {
                let p = v.replace("%2F", "/").replace("%2f", "/").replace("%2A", "*").replace("%2a", "*");
                if PathGlob::parse(&p).is_err() {
                    return invalid("paths", "invalid");
                }
                paths.push(p);
            }
            "per_page" => per_page = v.parse::<usize>().ok().filter(|n| *n > 0).unwrap_or(30).min(100),
            "after" => after = Some(LeaseId::parse(v).ok_or_else(|| invalid("after", "invalid"))),
            _ => {}
        }
    }
    if paths.len() > feed::FEED_PATHS_MAX {
        return invalid("paths", "invalid");
    }
    let after = match after.transpose() {
        Ok(a) => a,
        Err(e) => return e,
    };
    let v = LeaseView { core: r.core, owner, repo: r.at.id, at };
    let filter: Vec<&str> = paths.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    let mut more = false;
    let mut last = None;
    for row in v.live() {
        if after.is_some_and(|a| row.lease.id <= a) {
            continue;
        }
        if !filter.is_empty() && !row.lease.overlaps(&filter, &[]) {
            continue;
        }
        if out.len() == per_page {
            more = true;
            break;
        }
        last = Some(row.lease.id);
        out.push(lease_json(r, &row, at));
    }
    let mut reply = Reply::json(200, Value::Array(out));
    if more && let Some(l) = last {
        let root = format!("{}{}", r.host.web(), crate::agents::PREFIX);
        let next = format!("<{root}{path}?per_page={per_page}&after={}>; rel=\"next\"", l.text());
        reply = reply.with("link", next);
    }
    reply
}

// ---- writes ----

/// `POST /leases` with `{paths, refs?, mode?, ttl_seconds?, intent?, session?}` and the session header, which wins
/// over the body's `session`.
pub fn parse_acquire(body: &Value, header_session: Option<&str>) -> Result<NewLease, Reply> {
    let strings = |k: &'static str, max: usize, required: bool| -> Result<Vec<Box<str>>, Reply> {
        match body.get(k) {
            None | Some(Value::Null) if !required => Ok(Vec::new()),
            None | Some(Value::Null) => Err(invalid(k, "missing_field")),
            Some(Value::Array(a)) if a.len() <= max && (!required || !a.is_empty()) => {
                a.iter().map(|x| x.as_str().map(Box::from).ok_or_else(|| invalid(k, "invalid"))).collect()
            }
            Some(_) => Err(invalid(k, "invalid")),
        }
    };
    let paths = strings("paths", PATHS_PER_LEASE, true)?;
    let refs = strings("refs", REFS_PER_LEASE, false)?;
    let mode = match body.get("mode").and_then(Value::as_str) {
        None => LeaseMode::Shared,
        Some("shared") => LeaseMode::Shared,
        Some("exclusive") => LeaseMode::Exclusive,
        Some(_) => return Err(invalid("mode", "invalid")),
    };
    let ttl_seconds = match body.get("ttl_seconds") {
        None | Some(Value::Null) => None,
        Some(v) => {
            Some(v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| invalid("ttl_seconds", "invalid"))?)
        }
    };
    let intent = match body.get("intent") {
        None | Some(Value::Null) => "",
        Some(Value::String(s)) if s.len() <= INTENT_MAX => s.as_str(),
        Some(_) => return Err(invalid("intent", "invalid")),
    };
    let session = header_session.or_else(|| body.get("session").and_then(Value::as_str)).map(Box::from);
    Ok(NewLease { paths, refs, mode, ttl_seconds, intent: intent.into(), session })
}

/// `POST /leases/{id}/renew` with `{ttl_seconds?}`.
pub fn parse_renew(id: &str, body: &Value) -> Result<IssueOp<graftyard_git::blob::BlobRef>, Reply> {
    let Some(id) = LeaseId::parse(id) else { return Err(Reply::not_found()) };
    let ttl_seconds = match body.get("ttl_seconds") {
        None | Some(Value::Null) => None,
        Some(v) => {
            Some(v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| invalid("ttl_seconds", "invalid"))?)
        }
    };
    Ok(IssueOp::RenewLease { id, ttl_seconds })
}

/// The reply to a committed lease command, rendered on the leader from its `&Acked` and its core tier after the entry
/// applied: 201 `{lease, overlaps}` for an acquire, where `overlaps` is every other live lease overlapping it; 200
/// with the lease for a renew; 204 for a release.
pub fn render_ok(r: &Repo<'_>, owner: &dyn AuthzRead, at: Timestamp, acked: &Acked<IssueOut>, created: bool) -> Reply {
    match &acked.out {
        IssueOut::Lease(id) => {
            let Some(row) = lease_row(r.core, *id) else { return Reply::error(500, "no lease row", DOC) };
            if !created {
                return Reply::json(200, lease_json(r, &row, at));
            }
            let v = LeaseView { core: r.core, owner, repo: r.at.id, at };
            let paths: Vec<&str> = row.lease.scope.paths.iter().map(|g| g.as_str()).collect();
            let refs: Vec<&str> = row.lease.scope.refs.iter().map(|g| g.as_str()).collect();
            let overlaps: Vec<Value> = v
                .live()
                .iter()
                .filter(|o| o.lease.id != *id && o.lease.overlaps(&paths, &refs))
                .map(|o| lease_json(r, o, at))
                .collect();
            Reply::json(201, json!({ "lease": lease_json(r, &row, at), "overlaps": overlaps }))
        }
        IssueOut::Done => Reply::empty(204),
        _ => Reply::error(500, "not a lease reply", DOC),
    }
}

fn conflict(r: &Repo<'_>, at: Timestamp, c: &[Lease]) -> Reply {
    let first = c.first().and_then(|l| l.scope.paths.first()).map_or("", |g| g.as_str());
    let mut v = graftyard_wire::basic_error(409, &format!("{first} is leased Exclusive by another holder"), DOC);
    let rows: Vec<Value> = c.iter().map(|l| lease_json(r, &LeaseRow { lease: l.clone(), plan: None }, at)).collect();
    v["conflicts"] = Value::Array(rows);
    Reply::json(409, v)
}

/// The reply to a refused lease command. `r` and `at` render a 409's `conflicts`.
pub fn refused(r: Option<(&Repo<'_>, Timestamp)>, e: &IssueErr) -> Reply {
    match e {
        IssueErr::Unauthorized(_) => Reply::error(401, "Bad credentials", "rest"),
        IssueErr::Forbidden => Reply::error(403, "Resource not accessible by integration", DOC),
        IssueErr::NotFound => Reply::not_found(),
        IssueErr::NeedIdBlock | IssueErr::Unavailable => {
            Reply::error(503, "Service Unavailable", "rest").with("retry-after", "5")
        }
        IssueErr::Lease(l) => match l {
            LeaseErr::NoPush => Reply::error(403, "A lease needs push access", DOC),
            LeaseErr::NoSession => Reply::error(422, "An App token must name its session", DOC),
            LeaseErr::Invalid(f) => invalid(f, "invalid"),
            LeaseErr::RefScope(p) => Reply::error(422, &format!("{p} is outside the task's prefix"), DOC),
            LeaseErr::NoLiveTask => Reply::error(422, "An Exclusive lease needs a live task", DOC),
            LeaseErr::Literal(g) => Reply::error(422, &format!("{g} needs a literal first segment"), DOC),
            LeaseErr::OnePerTask(id) => {
                Reply::error(422, &format!("The task already holds Exclusive lease {}", id.text()), DOC)
            }
            LeaseErr::Cap(c) => Reply::error(422, &format!("Too many live leases ({c})"), DOC),
            LeaseErr::Conflict(c) => match r {
                Some((r, at)) => conflict(r, at, c),
                None => Reply::error(409, "Leased Exclusive by another holder", DOC),
            },
            LeaseErr::NotFound => Reply::not_found(),
            LeaseErr::NotHolder => Reply::error(403, "Only the holder, or a repository admin, may do this", DOC),
        },
        _ => Reply::error(422, "Unprocessable Entity", DOC),
    }
}

// ---- the activity feed ----

/// One feed event as the API reference's `event`. `contents` gates path fields, which are omitted without it.
pub fn event_json(r: &Repo<'_>, e: &ActivityEvent, at: Timestamp, contents: bool) -> Value {
    match e {
        ActivityEvent::Lease { lease, change } => {
            let mut l = lease_json(r, &LeaseRow { lease: lease.clone(), plan: None }, at);
            if !contents {
                l.as_object_mut().map(|m| m.remove("paths"));
            }
            json!({ "type": "lease", "change": change.as_str(), "lease": l })
        }
        ActivityEvent::Task { task, state } => {
            json!({ "type": "task", "task_id": task.text(), "state": state.as_str() })
        }
        ActivityEvent::AgentPush { refname, head, paths } => {
            let mut v = json!({ "type": "agent_push", "ref": refname.display(), "head_sha": head.hex() });
            if contents {
                v["paths"] = json!(paths.iter().map(|p| &**p).collect::<Vec<_>>());
            }
            v
        }
        ActivityEvent::AgentPr { pr, action, title, author } => json!({
            "type": "agent_pr",
            "number": pr.0,
            "action": action.as_str(),
            "title": crate::trust::TrustView::of(r, title, *author, false, true),
        }),
    }
}

/// A poll's query, or 422 for a bad `wait`, a bad glob, or more than FEED_PATHS_MAX `paths` (LF-3).
pub fn parse_feed(query: &str) -> Result<FeedQuery, Reply> {
    let e = |field: &'static str| {
        let fe = FieldError { resource: "Activity", field, code: "invalid" };
        Reply::json(422, crate::wire::validation_error("Validation Failed", FEED_DOC, &[fe]))
    };
    feed::parse_query(query).map_err(|q| match q {
        QueryErr::Wait => e("wait"),
        QueryErr::Paths | QueryErr::TooManyPaths => e("paths"),
    })
}

/// 410 `{message, oldest_cursor}` for a cursor of another group or older than the ring.
pub fn gone(oldest: FeedCursor) -> Reply {
    let mut v = graftyard_wire::basic_error(410, "The cursor is older than the activity feed keeps", FEED_DOC);
    v["oldest_cursor"] = json!(oldest.encode());
    Reply::json(410, v)
}

/// 429 with `Retry-After` of FEED_BATCH_ALL: over FEED_POLLS, or every tried member full.
pub fn too_many() -> Reply {
    Reply::error(429, "Too many activity polls", FEED_DOC)
        .with("retry-after", feed::FEED_BATCH_ALL.as_secs().to_string())
}

/// `GET /activity`'s 200 `{cursor, events}`, from the events past the start the serving member read, the last index
/// it read through, and the poll's `paths`. Without `contents: read` the reply is 404, as for the other path fields.
pub fn activity(
    r: &Repo<'_>,
    start: Start,
    through: Index,
    events: &[(Index, Timestamp, ActivityEvent)],
    q: &FeedQuery,
    contents: bool,
) -> Reply {
    if !contents {
        return Reply::not_found();
    }
    let after = match start {
        Start::Gone { oldest } => return gone(oldest),
        Start::After(i) => i,
    };
    let filter: Vec<&str> = q.paths.iter().map(String::as_str).collect();
    let out: Vec<Value> = events
        .iter()
        .filter(|(i, _, e)| *i > after && e.matches(&filter))
        .map(|(_, at, e)| event_json(r, e, *at, contents))
        .collect();
    let cursor = FeedCursor { group: r.at.id, index: through.max(after) };
    Reply::json(200, json!({ "cursor": cursor.encode(), "events": out }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_parse_only_the_reference_paths() {
        assert_eq!(route("/repos/o/r/leases"), Some(Route::Leases { owner: "o", repo: "r" }));
        assert_eq!(route("/repos/o/r/leases/x"), Some(Route::Lease { owner: "o", repo: "r", id: "x" }));
        assert_eq!(route("/repos/o/r/leases/x/renew"), Some(Route::Renew { owner: "o", repo: "r", id: "x" }));
        assert_eq!(route("/repos/o/r/activity"), Some(Route::Activity { owner: "o", repo: "r" }));
        assert_eq!(route("/agents/leases"), None);
    }

    #[test]
    fn acquire_bodies_parse_as_the_reference_names_them() {
        let n =
            parse_acquire(&json!({ "paths": ["a/**"], "mode": "exclusive", "ttl_seconds": 60 }), Some("s")).unwrap();
        assert_eq!((n.mode, n.ttl_seconds, n.session.as_deref()), (LeaseMode::Exclusive, Some(60), Some("s")));
        assert_eq!(parse_acquire(&json!({ "paths": [] }), None).unwrap_err().status, 422);
        assert_eq!(parse_acquire(&json!({}), None).unwrap_err().status, 422);
        assert_eq!(parse_acquire(&json!({ "paths": ["a"], "mode": "x" }), None).unwrap_err().status, 422);
        let many: Vec<String> = (0..33).map(|i| format!("p{i}")).collect();
        assert_eq!(parse_acquire(&json!({ "paths": many }), None).unwrap_err().status, 422);
        let n = parse_acquire(&json!({ "paths": ["a"], "session": "b" }), None).unwrap();
        assert_eq!(n.session.as_deref(), Some("b"));
    }
}
