//! REST routes of `rest:issues`, `rest:pulls`, `rest:checks`, and `rest:commits` that a T0 oracle calls
//! (`design/SHARED.md`, "Surfaces, tiers, and oracles").
//!
//! gh 2.102.1's `gh pr` and `gh issue` commands call no REST route in these four categories: they read and write
//! through GraphQL, plus `GET /meta` and `DELETE /git/refs/{ref}` (`rest:meta` and `rest:git`, M2-WP4). The routes
//! here are the one BF-22 names, `GET /repos/{owner}/{repo}/commits/{ref}/statuses` (`rest:commits`, "statuses"), and
//! journey 3's (`design/DESIGN.md`, "Commit and read paths"): `GET /repos/{owner}/{repo}/issues`, which
//! `gh api repos/o/r/issues?sort=updated` calls, and `POST /repos/{owner}/{repo}/labels`, which `gh label create`
//! calls before `gh issue create -l bug`.

use graftyard_git::refs::{ObjectId, RefName};
use serde_json::{Value, json};

use crate::view::{Item, Repo, StateFilter};
use crate::wire::{NodeType, Page, Reply, color, iso, scoped, simple_user};

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
    let items: Vec<Value> = rows.iter().map(|t| status_json(r, t)).collect();
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

/// `PUT /repos/{o}/{r}/pulls/{n}/merge`'s body: the method (default `merge`) and `sha`, the head it must be at.
pub fn plan_rest_merge(
    r: &Repo<'_>,
    n: &str,
    body: &Value,
) -> Result<(graftyard_git::issues::IssueNumber, graftyard_git::merge::MergeRequest, Option<ObjectId>), Reply> {
    use graftyard_git::issues::{IssueNumber, MergeMethod};
    let n = IssueNumber(n.parse().map_err(|_| Reply::not_found())?);
    let method = match body.get("merge_method").and_then(Value::as_str).unwrap_or("merge") {
        "merge" => MergeMethod::Merge,
        "squash" => MergeMethod::Squash,
        "rebase" => MergeMethod::Rebase,
        _ => return Err(Reply::error(422, "Validation Failed", "rest/pulls/pulls#merge-a-pull-request")),
    };
    if r.item(n).is_none_or(|it| !it.is_pull()) {
        return Err(Reply::not_found());
    }
    if r.settings().is_some_and(|s| !s.merge_methods.contains(&method)) {
        return Err(refused_merge(&graftyard_git::merge::MergeError::MethodNotAllowed));
    }
    let sha = match body.get("sha").and_then(Value::as_str) {
        Some(h) => Some(
            ObjectId::parse_hex(h.as_bytes())
                .ok_or_else(|| Reply::error(422, "Validation Failed", "rest/pulls/pulls#merge-a-pull-request"))?,
        ),
        None => None,
    };
    let req = graftyard_git::merge::merge_request(r.core, n, method).map_err(|e| refused_merge(&e))?;
    if sha.is_some_and(|s| s != req.head) {
        return Err(refused_merge(&graftyard_git::merge::MergeError::HeadModified));
    }
    Ok((n, req, sha))
}

/// GHES's REST reply to a refused merge: 409 for a moved head or base, 403 and 404 as their names say, and 405
/// "Pull Request is not mergeable" with the rule's text for the rest [unverified: GitHub's 405 bodies for each rule].
pub fn refused_merge(e: &graftyard_git::merge::MergeError) -> Reply {
    use graftyard_git::merge::MergeError as E;
    let doc = "rest/pulls/pulls#merge-a-pull-request";
    match e {
        E::HeadModified | E::BaseModified => Reply::error(409, &e.message(), doc),
        E::Unauthorized(_) => Reply::error(401, "Bad credentials", "rest"),
        E::Forbidden => Reply::error(403, "Resource not accessible by integration", doc),
        E::Agent(a) => Reply::error(403, &a.text, doc),
        E::NotFound => Reply::not_found(),
        E::Store(_) => Reply::error(503, "Service Unavailable", "rest").with("retry-after", "5"),
        E::Protection(_) | E::MethodNotAllowed | E::NotOpen | E::Draft => Reply::error(405, &e.message(), doc),
        _ => Reply::error(405, "Pull Request is not mergeable", doc),
    }
}

fn status_json(r: &Repo<'_>, t: &graftyard_git::tiers::Timed<graftyard_git::issues::StatusUpdate>) -> Value {
    let s = &t.rec;
    let creator = simple_user(r.host, r.dir, Some(s.creator));
    json!({
        "url": format!("{}/repos/{}/{}/statuses/{}", r.host.api(), r.at.owner, r.at.name, s.sha.hex()),
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
}

/// `POST /repos/{o}/{r}/statuses/{sha}`'s command: a `RecordStatus` by the caller. `id` is the status's id, which the
/// edge mints increasing (a later status of the same context and creator wins by id) [inference: the repository
/// chain assigns no status ids yet].
pub fn plan_create_status(
    sha: &str,
    body: &Value,
    creator: graftyard_types::AccountId,
    id: graftyard_types::DbId,
) -> Result<graftyard_git::issue_ops::IssueOp, Reply> {
    use graftyard_git::issues::{StatusId, StatusState, StatusUpdate, Url};
    let doc = "rest/commits/statuses#create-a-commit-status";
    let invalid = || Reply::error(422, "Validation Failed", doc);
    let sha = ObjectId::parse_hex(sha.as_bytes()).ok_or_else(invalid)?;
    let state = match body.get("state").and_then(Value::as_str) {
        Some("success") => StatusState::Success,
        Some("failure") => StatusState::Failure,
        Some("error") => StatusState::Error,
        Some("pending") => StatusState::Pending,
        _ => return Err(invalid()),
    };
    let text = |k: &str| body.get(k).and_then(Value::as_str).map(Into::into);
    Ok(graftyard_git::issue_ops::IssueOp::RecordStatus(StatusUpdate {
        id: StatusId(id),
        sha,
        context: text("context").unwrap_or_else(|| "default".into()),
        state,
        target_url: body.get("target_url").and_then(Value::as_str).map(|u| Url(u.into())),
        description: text("description"),
        creator,
    }))
}

/// The 201 of a committed `RecordStatus`: the status as the leader's tiers hold it after the entry applied. `decide`
/// may raise the edge's `id` above the commit's newest status, so the committed id is `acked.out`'s; `id` is the
/// edge's, read only from a member that answers `Done`.
pub fn status_created(
    acked: &graftyard_raft::Acked<graftyard_git::issue_ops::IssueOut>,
    after: &Repo<'_>,
    sha: ObjectId,
    id: i64,
) -> Reply {
    let id = match acked.out {
        graftyard_git::issue_ops::IssueOut::Id(d) => d.0,
        _ => id,
    };
    match after.statuses(&sha, 0, 1000).into_iter().find(|t| t.rec.id.0.0 == id) {
        Some(t) => Reply::json(201, status_json(after, &t)),
        None => Reply::error(500, "Internal Server Error", "rest"),
    }
}

// ---- journey 3: issues and labels ----

const ISSUES_DOC: &str = "rest/issues/issues#list-repository-issues";
const LABELS_DOC: &str = "rest/issues/labels#create-a-label";

fn param<'q>(query: &'q str, k: &str) -> Option<&'q str> {
    query.split('&').filter_map(|kv| kv.split_once('=')).find(|(a, _)| *a == k).map(|(_, v)| v)
}

/// The pinned `label` schema's object.
pub fn label_json(r: &Repo<'_>, id: graftyard_types::DbId, l: &graftyard_git::tiers::LabelRow) -> Value {
    let (o, n) = (&r.at.owner, &r.at.name);
    json!({
        "id": id.0,
        "node_id": scoped(NodeType::Label, r, id.0),
        "url": format!("{}/repos/{o}/{n}/labels/{}", r.host.api(), js_path(&l.name.0)),
        "name": &*l.name.0,
        "description": l.description.as_deref(),
        "color": color(l.color),
        "default": false,
        "archived_at": null,
        "archived_by": null,
    })
}

/// A path segment with the characters a label name may hold that a URL path may not, percent-encoded.
fn js_path(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The pinned `issue` schema's object, for an issue or a pull request.
pub fn issue_json(r: &Repo<'_>, it: &Item) -> Value {
    let (api, web) = (r.host.api(), r.host.web());
    let (o, n) = (&r.at.owner, &r.at.name);
    let num = it.n.0;
    let row = &it.row;
    let labels: Vec<Value> = r.labels_of(it.n).iter().map(|(id, l)| label_json(r, *id, l)).collect();
    let assignees: Vec<Value> = r.assignees_of(it.n).into_iter().map(|a| simple_user(r.host, r.dir, Some(a))).collect();
    let kind = if it.is_pull() { "pull" } else { "issues" };
    let mut v = json!({
        "id": row.id.0,
        "node_id": scoped(if it.is_pull() { NodeType::PullRequest } else { NodeType::Issue }, r, row.id.0),
        "url": format!("{api}/repos/{o}/{n}/issues/{num}"),
        "repository_url": format!("{api}/repos/{o}/{n}"),
        "labels_url": format!("{api}/repos/{o}/{n}/issues/{num}/labels{{/name}}"),
        "comments_url": format!("{api}/repos/{o}/{n}/issues/{num}/comments"),
        "events_url": format!("{api}/repos/{o}/{n}/issues/{num}/events"),
        "html_url": format!("{web}/{o}/{n}/{kind}/{num}"),
        "number": num,
        "state": if row.open { "open" } else { "closed" },
        "state_reason": row.reason.map(|s| match s {
            graftyard_git::issues::StateReason::Completed => "completed",
            graftyard_git::issues::StateReason::NotPlanned => "not_planned",
            graftyard_git::issues::StateReason::Reopened => "reopened",
        }),
        "title": &*row.title,
        "body": (!row.body.is_empty()).then_some(&*row.body),
        "user": simple_user(r.host, r.dir, row.author),
        "labels": labels,
        "assignee": assignees.first().cloned().unwrap_or(Value::Null),
        "assignees": assignees,
        "milestone": null,
        "locked": row.locked.is_some(),
        "comments": row.comments,
        "created_at": iso(row.created),
        "updated_at": iso(row.updated),
        "closed_at": row.closed.map(iso),
        "author_association": crate::wire::association(r, row.author),
    });
    if let Some((_, head)) = &it.pull {
        let merged = matches!(head.state, graftyard_git::issues::PrState::Merged { .. });
        v["pull_request"] = json!({
            "url": format!("{api}/repos/{o}/{n}/pulls/{num}"),
            "html_url": format!("{web}/{o}/{n}/pull/{num}"),
            "diff_url": format!("{web}/{o}/{n}/pull/{num}.diff"),
            "patch_url": format!("{web}/{o}/{n}/pull/{num}.patch"),
            "merged_at": if merged { row.closed.map(iso) } else { None },
        });
    }
    v
}

/// `GET /repos/{owner}/{repo}/issues`: issues and pull requests in `state` (default open), with every one of the
/// comma-separated `labels`, sorted by `sort` (`created`, the default, `updated`, or `comments`) in `direction`
/// (default `desc`), paged. One read of the fresh tiers, so an issue committed before the read is in it (journey 3).
pub fn list_issues(r: &Repo<'_>, query: &str) -> Reply {
    let state = match param(query, "state") {
        None | Some("open") => StateFilter::Open,
        Some("closed") => StateFilter::Closed,
        Some("all") => StateFilter::All,
        Some(_) => return Reply::error(422, "Validation Failed", ISSUES_DOC),
    };
    let want: Vec<String> = param(query, "labels")
        .map(|l| l.split(',').map(|s| s.replace("%20", " ").replace('+', " ").to_lowercase()).collect())
        .unwrap_or_default();
    let has = |it: &Item| {
        let mine: Vec<String> = r.labels_of(it.n).iter().map(|(_, l)| l.name.0.to_lowercase()).collect();
        want.iter().all(|w| mine.contains(w))
    };
    let mut all: Vec<Item> = Vec::new();
    for kind in [graftyard_git::issues::IssueKind::Issue, graftyard_git::issues::IssueKind::PullRequest] {
        all.extend(r.list(kind, state, None, usize::MAX, &has).0);
    }
    let asc = param(query, "direction") == Some("asc");
    match param(query, "sort") {
        None | Some("created") => all.sort_by_key(|i| (i.row.created, i.n)),
        Some("updated") => all.sort_by_key(|i| (i.row.updated, i.n)),
        Some("comments") => all.sort_by_key(|i| (i.row.comments, i.n)),
        Some(_) => return Reply::error(422, "Validation Failed", ISSUES_DOC),
    }
    if !asc {
        all.reverse();
    }
    let page = Page::parse(query);
    let more = all.len() > page.skip() + page.per_page;
    let items: Vec<Value> = all.iter().skip(page.skip()).take(page.per_page).map(|it| issue_json(r, it)).collect();
    let keep: Vec<(&str, &str)> = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .filter(|(k, _)| !matches!(*k, "page" | "per_page"))
        .collect();
    let link = graftyard_wire::link(&r.host.api(), r.at.id.0.0, "/issues", &keep, page, more, None);
    let reply = Reply::json(200, Value::Array(items));
    match link {
        Some(l) => reply.with("link", l),
        None => reply,
    }
}

/// `POST /repos/{o}/{r}/labels`'s command: a `DefineLabel`. `color` is six hex digits, as `gh label create` sends it.
pub fn plan_create_label(body: &Value) -> Result<graftyard_git::issue_ops::IssueOp, Reply> {
    use graftyard_git::issues::LabelName;
    let invalid = || Reply::error(422, "Validation Failed", LABELS_DOC);
    let name = body.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).ok_or_else(invalid)?;
    let hex = body.get("color").and_then(Value::as_str).unwrap_or("ededed").trim_start_matches('#');
    let byte = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    let (Some(a), Some(b), Some(c), 6) = (byte(0), byte(2), byte(4), hex.len()) else { return Err(invalid()) };
    let description = body.get("description").and_then(Value::as_str).filter(|d| !d.is_empty()).map(Into::into);
    Ok(graftyard_git::issue_ops::IssueOp::DefineLabel {
        id: None,
        name: LabelName(name.into()),
        color: [a, b, c],
        description,
    })
}

/// The 201 of a committed `DefineLabel`: the label as the leader's tiers hold it after the entry applied.
pub fn label_created(acked: &graftyard_raft::Acked<graftyard_git::issue_ops::IssueOut>, after: &Repo<'_>) -> Reply {
    let graftyard_git::issue_ops::IssueOut::Id(id) = acked.out else {
        return Reply::error(500, "Internal Server Error", "rest");
    };
    match after.labels().into_iter().find(|(i, _)| *i == id) {
        Some((id, l)) => Reply::json(201, label_json(after, id, &l)),
        None => Reply::error(500, "Internal Server Error", "rest"),
    }
}

/// A refused `DefineLabel`: GitHub's 422 `already_exists` for a taken name, which `gh label create` reads.
pub fn label_refused(e: &graftyard_git::issue_ops::IssueErr) -> Reply {
    use graftyard_git::issue_ops::IssueErr as E;
    match e {
        E::Exists => Reply::json(
            422,
            json!({ "message": "Validation Failed",
                "errors": [{ "resource": "Label", "code": "already_exists", "field": "name" }],
                "documentation_url": format!("https://docs.github.com/enterprise-server@3.22/{LABELS_DOC}"),
                "status": "422" }),
        ),
        E::Forbidden | E::Unauthorized(_) => Reply::error(403, "Resource not accessible by integration", LABELS_DOC),
        E::Agent(a) => Reply::error(403, &a.text, LABELS_DOC),
        E::NotFound => Reply::not_found(),
        _ => Reply::error(422, "Validation Failed", LABELS_DOC),
    }
}
