//! `rest:activity` at T1 and `rest:search`'s `GET /search/issues` (`design/SHARED.md`, "Surfaces, tiers, and
//! oracles"; `build/BUILD-PLAN.md`, M4-WP2): the stargazer and subscriber lists from the watcher index, the
//! repository's own subscription, the notification threads from the `Inbox` with the account's read and done state,
//! and the search reply merged from the shards. Bodies follow GHES 3.22's `simple-user`, `stargazer`,
//! `repository-subscription`, `thread`, `thread-subscription`, and `issue-search-result-item` schemas.
//!
//! Before this package the edge answered these routes with 404, so no earlier reply changes. `full-repository`'s
//! `stargazers_count`, `watchers`, `watchers_count`, and `subscribers_count` read the watcher index's counts, which are
//! 0 for a repository nobody starred or watched, the value it always rendered.

use graftyard_git::inbox::{Inbox, Listed};
use graftyard_git::issues::{IssueKind, IssueNumber};
use graftyard_git::search::Merged;
use graftyard_git::watch;
use graftyard_identity::{ThreadId, UserRows};
use graftyard_types::Timestamp;
use serde_json::{Value, json};

use crate::view::Repo;
use crate::wire::{Page, Reply, iso, simple_user};

const STAR_MEDIA: &str = "application/vnd.github.star+json";

/// One page of `all`, rendered only for the page, with GHES's `link` header.
fn paged<T>(r: &Repo<'_>, path: &str, query: &str, all: Vec<T>, render: impl Fn(T) -> Value) -> Reply {
    let page = Page::parse(query);
    let more = all.len() > page.end();
    let items: Vec<Value> = all.into_iter().skip(page.skip()).take(page.per_page).map(render).collect();
    let reply = Reply::json(200, Value::Array(items));
    match graftyard_wire::link(&r.host.api(), r.at.id.0.0, path, &[], page, more, None) {
        Some(l) => reply.with("link", l),
        None => reply,
    }
}

/// `GET /repos/{o}/{r}/stargazers`, oldest star first. With `Accept: application/vnd.github.star+json` each item is
/// a `stargazer` (`starred_at` and `user`); otherwise a `simple-user`.
pub fn stargazers(r: &Repo<'_>, query: &str, accept: &str) -> Reply {
    let star = accept.split(',').any(|a| a.trim().starts_with(STAR_MEDIA));
    paged(r, "/stargazers", query, watch::stargazers(r.meta), |(a, at)| {
        let u = simple_user(r.host, r.dir, Some(a));
        if star { json!({ "starred_at": iso(at), "user": u }) } else { u }
    })
}

/// `GET /repos/{o}/{r}/subscribers`.
pub fn subscribers(r: &Repo<'_>, query: &str) -> Reply {
    paged(r, "/subscribers", query, watch::subscribers(r.meta), |a| simple_user(r.host, r.dir, Some(a)))
}

/// The counts `full-repository` renders: `(stargazers_count, subscribers_count)`.
pub fn counts(r: &Repo<'_>) -> (u32, u32) {
    let c = watch::watch_count(r.meta);
    (c.stars, c.subscribers)
}

/// `GET /repos/{o}/{r}/subscription` for `viewer`, from the viewer's own `Identity` fold, which is current at once;
/// 404 when the viewer neither watches nor ignores it, as GHES answers.
pub fn repo_subscription(r: &Repo<'_>, user: &UserRows) -> Reply {
    let Some(w) = user.watches.get(&r.at.id) else { return Reply::not_found() };
    let api = r.host.api();
    let (o, n) = (&r.at.owner, &r.at.name);
    Reply::json(
        200,
        json!({
            "subscribed": w.subscribed, "ignored": w.ignored, "reason": null, "created_at": iso(w.set.at),
            "url": format!("{api}/repos/{o}/{n}/subscription"), "repository_url": format!("{api}/repos/{o}/{n}"),
        }),
    )
}

/// GHES's `thread` object.
pub fn thread_json(r: &Repo<'_>, l: &Listed) -> Value {
    let api = r.host.api();
    let (o, n) = (&r.at.owner, &r.at.name);
    let id = l.thread.rest_id();
    let num = l.thread.number;
    let (ty, path) = match l.row.kind {
        IssueKind::Issue => ("Issue", "issues"),
        IssueKind::PullRequest => ("PullRequest", "pulls"),
    };
    let subject_url = format!("{api}/repos/{o}/{n}/{path}/{num}");
    let latest = match l.row.latest_comment {
        Some(c) => format!("{api}/repos/{o}/{n}/issues/comments/{}", c.0.0),
        None => subject_url.clone(),
    };
    json!({
        "id": id.to_string(),
        "repository": minimal_repository(r),
        "subject": { "title": &*l.row.title, "url": subject_url, "latest_comment_url": latest, "type": ty },
        "reason": l.row.reason.as_str(),
        "unread": l.unread,
        "updated_at": iso(l.row.updated),
        "last_read_at": l.last_read.map(iso),
        "url": format!("{api}/notifications/threads/{id}"),
        "subscription_url": format!("{api}/notifications/threads/{id}/subscription"),
    })
}

/// GHES's `minimal-repository`: `full-repository` without the fields only it has.
pub fn minimal_repository(r: &Repo<'_>) -> Value {
    let mut v = crate::repos::full_repository(r);
    if let Some(m) = v.as_object_mut() {
        for k in ["organization", "permissions", "network_count", "subscribers_count", "template_repository"] {
            m.remove(k);
        }
    }
    v
}

/// The query of `GET /notifications` and `GET /repos/{o}/{r}/notifications`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NotificationQuery {
    pub all: bool,
    pub participating: bool,
    pub since: Option<Timestamp>,
    pub before: Option<Timestamp>,
}

fn param<'a>(q: &'a str, k: &str) -> Option<&'a str> {
    q.split('&').filter_map(|kv| kv.split_once('=')).find(|(a, _)| *a == k).map(|(_, v)| v)
}

impl NotificationQuery {
    pub fn parse(q: &str) -> Result<NotificationQuery, Reply> {
        let time = |k| match param(q, k) {
            None => Ok(None),
            Some(v) => graftyard_wire::parse_iso(&v.replace("%3A", ":")).map(Some).ok_or_else(|| {
                Reply::error(
                    422,
                    "Validation Failed",
                    "rest/activity/notifications#list-notifications-for-the-authenticated-user",
                )
            }),
        };
        Ok(NotificationQuery {
            all: param(q, "all") == Some("true"),
            participating: param(q, "participating") == Some("true"),
            since: time("since")?,
            before: time("before")?,
        })
    }
}

/// The threads `GET /notifications` lists for a page, newest first, before rendering: the edge reads each one's
/// repository on a follower and renders it with `thread_json`.
pub fn notification_page(
    inbox: &Inbox,
    user: &UserRows,
    q: &NotificationQuery,
    repo: Option<graftyard_types::RepoId>,
    query: &str,
) -> (Vec<Listed>, bool) {
    let page = Page::parse(query);
    let all: Vec<Listed> = inbox
        .view(user, q.all, q.participating, repo)
        .into_iter()
        .filter(|l| q.since.is_none_or(|s| l.row.updated > s) && q.before.is_none_or(|b| l.row.updated < b))
        .collect();
    let more = all.len() > page.end();
    (all.into_iter().skip(page.skip()).take(page.per_page).collect(), more)
}

/// GHES's `thread-subscription` for a thread the account set a subscription on, or the repository-level default.
pub fn thread_subscription(api: &str, thread: ThreadId, user: &UserRows) -> Value {
    let id = thread.rest_id();
    let (subscribed, at) = match user.subscriptions.get(&thread) {
        Some((s, set)) => (*s, Some(set.at)),
        None => (user.watches.get(&thread.repo).is_some_and(|w| w.subscribed && !w.ignored), None),
    };
    json!({
        "subscribed": subscribed, "ignored": false, "reason": null, "created_at": at.map(iso),
        "url": format!("{api}/notifications/threads/{id}/subscription"),
        "thread_url": format!("{api}/notifications/threads/{id}"),
    })
}

/// A thread of the viewer by its REST id, or `None` (404) for a malformed id.
pub fn thread_id(s: &str) -> Option<ThreadId> {
    s.parse::<u64>().ok().map(ThreadId::from_rest_id)
}

/// One `GET /search/issues` item: `issue_json` plus `score`, as GHES's `issue-search-result-item` has it.
pub fn search_item(r: &Repo<'_>, n: IssueNumber, score: f64) -> Option<Value> {
    let it = r.item(n)?;
    let mut v = crate::rest::issue_json(r, &it);
    v["score"] = json!(if score == 0.0 { 1.0 } else { -score });
    v["repository_url"] = json!(format!("{}/repos/{}/{}", r.host.api(), r.at.owner, r.at.name));
    Some(v)
}

/// The search reply's envelope. `search_type` is `lexical`, the only kind FTS5 serves; GHES 3.22 requires the key.
pub fn search_reply(m: &Merged, items: Vec<Value>) -> Reply {
    Reply::json(
        200,
        json!({ "total_count": m.total, "incomplete_results": m.incomplete, "items": items, "search_type": "lexical" }),
    )
}

/// An unsupported qualifier: 422 rather than a wrong answer.
pub fn search_unsupported(q: &str) -> Reply {
    let _ = q;
    Reply::json(
        422,
        crate::wire::validation_error(
            "Validation Failed",
            "rest/search/search#search-issues-and-pull-requests",
            &[crate::wire::FieldError { resource: "Search", field: "q", code: "invalid" }],
        ),
    )
}
