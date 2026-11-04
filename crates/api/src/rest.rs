//! REST routes of `rest:issues`, `rest:pulls`, `rest:checks`, and `rest:commits` that a T0 oracle calls
//! (`design/SHARED.md`, "Surfaces, tiers, and oracles").
//!
//! gh 2.102.1's `gh pr` and `gh issue` commands call no REST route in these four categories: they read and write
//! through GraphQL, plus `GET /meta` and `DELETE /git/refs/{ref}` (`rest:meta` and `rest:git`, M2-WP4). The one route
//! here is the one BF-22 names, `GET /repos/{owner}/{repo}/commits/{ref}/statuses` (`rest:commits`, "statuses").

use graftyard_git::refs::{ObjectId, RefName};
use serde_json::{Value, json};

use crate::view::Repo;
use crate::wire::{NodeType, Page, Reply, iso, scoped, simple_user};

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
    let Some(sha) = resolve(r, reference) else { return Reply::not_found() };
    let skip = page.skip();
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
                "node_id": scoped(NodeType::StatusContext, r, s.id.0.0),
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
    let path = format!("/commits/{}/statuses", sha.hex());
    let link = graftyard_wire::link(&api, r.at.id.0.0, &path, &[], page, more, None);
    let reply = Reply::json(200, Value::Array(items));
    match link {
        Some(l) => reply.with("link", l),
        None => reply,
    }
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

// ---- meta-endpoint, rest:meta, rest:rate-limit, and the root documents ----

/// `GET /api/v3/meta`: GHES's `api-overview`, which gh reads for feature detection and GCM for its probe.
pub fn meta() -> Reply {
    let version = graftyard_wire::ENTERPRISE_VERSION;
    Reply::json(200, serde_json::json!({ "verifiable_password_authentication": false, "installed_version": version }))
}

/// `GET /api/v3/`: the root document, whose `rate_limit_url` Jenkins reads (`design/SHARED.md`, "Hostnames and
/// endpoints").
pub fn root(h: &crate::wire::Host) -> Reply {
    let (api, web) = (h.api(), h.web());
    let search = |k: &str| format!("{api}/search/{k}?q={{query}}{{&page,per_page,sort,order}}");
    Reply::json(
        200,
        json!({
            "current_user_url": format!("{api}/user"),
            "current_user_authorizations_html_url": format!("{web}/settings/connections/applications{{/client_id}}"),
            "authorizations_url": format!("{api}/authorizations"),
            "code_search_url": search("code"),
            "commit_search_url": search("commits"),
            "emails_url": format!("{api}/user/emails"),
            "emojis_url": format!("{api}/emojis"),
            "events_url": format!("{api}/events"),
            "feeds_url": format!("{api}/feeds"),
            "followers_url": format!("{api}/user/followers"),
            "following_url": format!("{api}/user/following{{/target}}"),
            "gists_url": format!("{api}/gists{{/gist_id}}"),
            "hub_url": format!("{api}/hub"),
            "issue_search_url": search("issues"),
            "issues_url": format!("{api}/issues"),
            "keys_url": format!("{api}/user/keys"),
            "label_search_url": format!("{api}/search/labels?q={{query}}&repository_id={{repository_id}}{{&page,per_page}}"),
            "notifications_url": format!("{api}/notifications"),
            "organization_url": format!("{api}/orgs/{{org}}"),
            "organization_repositories_url": format!("{api}/orgs/{{org}}/repos{{?type,page,per_page,sort}}"),
            "organization_teams_url": format!("{api}/orgs/{{org}}/teams"),
            "public_gists_url": format!("{api}/gists/public"),
            "rate_limit_url": format!("{api}/rate_limit"),
            "repository_url": format!("{api}/repos/{{owner}}/{{repo}}"),
            "repository_search_url": search("repositories"),
            "current_user_repositories_url": format!("{api}/user/repos{{?type,page,per_page,sort}}"),
            "starred_url": format!("{api}/user/starred{{/owner}}{{/repo}}"),
            "starred_gists_url": format!("{api}/gists/starred"),
            "topic_search_url": format!("{api}/search/topics?q={{query}}{{&page,per_page}}"),
            "user_url": format!("{api}/users/{{user}}"),
            "user_organizations_url": format!("{api}/user/orgs"),
            "user_repositories_url": format!("{api}/users/{{user}}/repos{{?type,page,per_page,sort}}"),
            "user_search_url": search("users"),
        }),
    )
}

/// `GET /api/v3/status`, which VS Code Pull Requests reads for a body that starts `GitHub lives!`, and `GET /status`
/// on the bare host, which Dependabot reads for a 200 with `X-GitHub-Request-Id` (`finish` adds it).
pub fn status() -> Reply {
    Reply::bytes(200, "text/plain; charset=utf-8", b"GitHub lives! (Graftyard)\n".to_vec())
}

/// `GET /api/v3/rate_limit`: 404 while rate limits are off, as GHES answers by default.
pub fn rate_limit(rate: &graftyard_wire::RateLimits, version: graftyard_wire::ApiVersion) -> Reply {
    let (status, body) = rate.body(version == graftyard_wire::ApiVersion::V2026_03_10);
    Reply::json(status, body)
}

// ---- rest:git ----

/// `DELETE /repos/{o}/{r}/git/refs/{ref}`, which `gh release delete --cleanup-tag` and `gh pr merge --delete-branch`
/// send: the ref update the edge commits as a `Push` (M1's write path, so rules apply as to a `git push --delete`).
/// `reference` is the path segment, `tags%2Fv1` or `heads/f`.
pub fn plan_delete_ref(r: &Repo<'_>, reference: &str) -> Result<graftyard_git::refs::RefUpdate, Reply> {
    let full = format!("refs/{}", reference.replace("%2F", "/").replace("%2f", "/"));
    let name =
        RefName::parse(full.as_bytes()).map_err(|_| Reply::error(422, "Reference does not exist", "rest/git/refs"))?;
    match r.ref_value(&name) {
        Some(o) => Ok(graftyard_git::refs::RefUpdate { name, expect: Some(o), new: None }),
        None => Err(Reply::error(422, "Reference does not exist", "rest/git/refs#delete-a-reference")),
    }
}
