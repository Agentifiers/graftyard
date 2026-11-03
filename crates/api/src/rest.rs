//! REST routes of `rest:issues`, `rest:pulls`, `rest:checks`, and `rest:commits` that a T0 oracle calls
//! (`design/SHARED.md`, "Surfaces, tiers, and oracles").
//!
//! gh 2.102.1's `gh pr` and `gh issue` commands call no REST route in these four categories: they read and write
//! through GraphQL, plus `GET /meta` and `DELETE /git/refs/{ref}` (`rest:meta` and `rest:git`, M2-WP4). The one route
//! here is the one BF-22 names, `GET /repos/{owner}/{repo}/commits/{ref}/statuses` (`rest:commits`, "statuses").

use graftyard_git::refs::{ObjectId, RefName};
use serde_json::{Value, json};

use crate::view::Repo;
use crate::wire::{iso, node_id, simple_user};

/// A rendered REST answer: status, JSON body, and the `Link` header when a page follows.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
    pub link: Option<String>,
}

fn not_found() -> Reply {
    let body = json!({
        "message": "Not Found",
        "documentation_url": "https://docs.github.com/enterprise-server@3.22/rest",
        "status": "404",
    });
    Reply { status: 404, body, link: None }
}

/// `per_page` clamps to 100 silently and `page` starts at 1 (`design/SHARED.md`, "Cross-cutting conventions").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    pub per_page: usize,
    pub page: usize,
}

impl Page {
    pub fn parse(query: &str) -> Page {
        let get = |k: &str| {
            query
                .split('&')
                .filter_map(|kv| kv.split_once('='))
                .find(|(a, _)| *a == k)
                .and_then(|(_, v)| v.parse().ok())
        };
        Page { per_page: get("per_page").unwrap_or(30usize).clamp(1, 100), page: get("page").unwrap_or(1usize).max(1) }
    }
}

/// A commit ref as the route takes it: a full SHA, a branch name, or a tag name.
fn resolve(r: &Repo<'_>, reference: &str) -> Option<ObjectId> {
    if let Some(o) = ObjectId::parse_hex(reference.as_bytes()) {
        return Some(o);
    }
    ["refs/heads/", "refs/tags/", ""]
        .iter()
        .filter_map(|p| RefName::parse(format!("{p}{reference}").as_bytes()).ok())
        .find_map(|n| r.ref_value(&n))
}

/// `GET /repos/{owner}/{repo}/commits/{ref}/statuses`: every status on the commit, newest first, as the pinned
/// `status` schema renders it, until its row is pruned after `check_retention_days`.
pub fn list_commit_statuses(r: &Repo<'_>, reference: &str, page: Page) -> Reply {
    let Some(sha) = resolve(r, reference) else { return not_found() };
    let skip = (page.page - 1) * page.per_page;
    let mut rows = r.statuses(&sha, skip, page.per_page + 1);
    let more = rows.len() > page.per_page;
    rows.truncate(page.per_page);
    let api = r.host.api();
    let items: Vec<Value> = rows
        .iter()
        .map(|t| {
            let s = &t.rec;
            let creator = simple_user(r.host, r.dir, Some(s.creator));
            json!({
                "url": format!("{api}/repos/{}/{}/statuses/{}", r.at.owner, r.at.name, sha.hex()),
                "avatar_url": creator["avatar_url"],
                "id": s.id.0.0,
                "node_id": node_id("SC", &[r.at.id.0.0, s.id.0.0]),
                "state": status_state(s.state),
                "description": s.description.as_deref(),
                "target_url": s.target_url.as_ref().map(|u| &*u.0),
                "context": &*s.context,
                "created_at": iso(t.at),
                "updated_at": iso(t.at),
                "creator": creator,
            })
        })
        .collect();
    let link = more.then(|| {
        format!(
            "<{api}/repositories/{}/commits/{}/statuses?per_page={}&page={}>; rel=\"next\"",
            r.at.id.0.0,
            sha.hex(),
            page.per_page,
            page.page + 1
        )
    });
    Reply { status: 200, body: Value::Array(items), link }
}

pub fn status_state(s: graftyard_git::issues::StatusState) -> &'static str {
    use graftyard_git::issues::StatusState as S;
    match s {
        S::Error => "error",
        S::Failure => "failure",
        S::Pending => "pending",
        S::Success => "success",
    }
}
