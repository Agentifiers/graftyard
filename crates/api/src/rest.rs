//! REST routes of `rest:issues`, `rest:pulls`, `rest:checks`, and `rest:commits` that a T0 oracle calls
//! (`design/SHARED.md`, "Surfaces, tiers, and oracles").
//!
//! gh 2.102.1's `gh pr` and `gh issue` commands call no REST route in these four categories: they read and write
//! through GraphQL, plus `GET /meta` and `DELETE /git/refs/{ref}` (`rest:meta` and `rest:git`, M2-WP4). The routes
//! here are the one BF-22 names, `GET /repos/{owner}/{repo}/commits/{ref}/statuses` (`rest:commits`, "statuses"), and
//! journey 3's (`design/DESIGN.md`, "Commit and read paths"): `GET /repos/{owner}/{repo}/issues`, which
//! `gh api repos/o/r/issues?sort=updated` calls, and `POST /repos/{owner}/{repo}/labels`, which `gh label create`
//! calls before `gh issue create -l bug`.

use graftyard_fold::Val;
use graftyard_git::issues::{
    AnnotationLevel, CheckAnnotation, CheckConclusion, CheckOutput, CheckRunId, CheckRunUpdate, CheckStatus,
    CheckSuiteId, CommentId, CommentTarget, DiffSide, IssueNumber, LineAnchor, ReviewState, Url,
};
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_git::tiers::{CommentRow, IssueRow};
use graftyard_types::{DbId, Timestamp};
use serde_json::{Value, json};

use crate::view::{Item, Pick, Repo, StateFilter};
use crate::wire::{NodeType, Page, Reply, color, iso, scoped, simple_user};

/// A commit ref as the route takes it: a full SHA, a branch name, or a tag name.
pub fn resolve(r: &Repo<'_>, reference: &str) -> Option<ObjectId> {
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

/// `POST /repos/{o}/{r}/git/refs` (`{ref, sha}`) and `PATCH /repos/{o}/{r}/git/refs/{ref}` (`{sha, force}`): the ref
/// update the edge commits as a `Push`, which `validate` checks on an empty pack for an agent (AM1). `reference` is
/// `None` for a create. A `PATCH` without `force` must be a fast-forward, which the git cache's detail reports; the
/// caller checks it.
pub fn plan_write_ref(
    r: &Repo<'_>,
    reference: Option<&str>,
    body: &Value,
) -> Result<(graftyard_git::refs::RefUpdate, bool), Reply> {
    let doc = "rest/git/refs";
    let bad = |m: &str| Reply::error(422, m, doc);
    let sha = body["sha"]
        .as_str()
        .and_then(|s| ObjectId::parse_hex(s.as_bytes()))
        .ok_or_else(|| bad("Invalid request.\n\n\"sha\" wasn't supplied."))?;
    match reference {
        None => {
            let full = body["ref"].as_str().ok_or_else(|| bad("Invalid request.\n\n\"ref\" wasn't supplied."))?;
            let name = RefName::parse(full.as_bytes())
                .ok()
                .filter(|_| full.starts_with("refs/") && full.matches('/').count() >= 2)
                .ok_or_else(|| bad("Reference name must start with 'refs/' and have at least two slashes."))?;
            if r.ref_value(&name).is_some() {
                return Err(bad("Reference already exists"));
            }
            Ok((graftyard_git::refs::RefUpdate { name, expect: None, new: Some(sha) }, true))
        }
        Some(reference) => {
            let full = format!("refs/{}", reference.replace("%2F", "/").replace("%2f", "/"));
            let name = RefName::parse(full.as_bytes()).map_err(|_| bad("Reference does not exist"))?;
            let old = r.ref_value(&name).ok_or_else(|| bad("Reference does not exist"))?;
            let force = body["force"].as_bool().unwrap_or(false);
            Ok((graftyard_git::refs::RefUpdate { name, expect: Some(old), new: Some(sha) }, force))
        }
    }
}

/// A `git-ref` object as GHES renders it.
pub fn ref_json(r: &Repo<'_>, name: &RefName, sha: &ObjectId) -> Value {
    let n = name.display();
    let url = format!("{}/repos/{}/{}/git/{}", r.host.api(), r.at.owner, r.at.name, n);
    let kind = "commit";
    let object_url = format!("{}/repos/{}/{}/git/commits/{}", r.host.api(), r.at.owner, r.at.name, sha.hex());
    // GitHub's legacy `Ref` id: base64 of `<len>:Ref<repo id>:<name>`.
    let raw = format!("Ref{}:{n}", r.at.id.0.0);
    let node_id = graftyard_git::pack::b64::encode(format!("{:02}:{raw}", raw.len()).as_bytes());
    json!({
        "ref": n,
        "node_id": node_id,
        "url": url,
        "object": { "type": kind, "sha": sha.hex(), "url": object_url },
    })
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

/// `GET /repos/{owner}/{repo}/pulls/{pull_number}`.
pub fn get_pull(r: &Repo<'_>, n: &str) -> Reply {
    get_pull_asks(r, n).0
}

/// `get_pull`, with the test merge its `mergeable` lacks, as GraphQL's single-PR read asks for it
/// (`crate::gql::test_merge`): the edge sends it to the shard as a GET's (`MergeCause::Get`) after the reply. Until the
/// merge is recorded the reply keeps the webhook payload's `mergeable: null` and `mergeable_state: "unknown"`, as GHES
/// 3.22 answers while its background job computes; once it is, `mergeable` is its result and `mergeable_state` the
/// lower case of GraphQL's `mergeStateStatus` (`crate::gql::merge_state`). `rebaseable` stays null, since no rebase
/// is tried [unverified: what GHES 3.22 answers for `rebaseable` beside a computed `mergeable`].
pub fn get_pull_asks(r: &Repo<'_>, n: &str) -> (Reply, crate::gql::MergeAsks) {
    let Some(n) = n.parse::<u32>().ok().map(IssueNumber) else { return (Reply::not_found(), Default::default()) };
    match r.item(n) {
        Some(it) if it.is_pull() => {
            let mut asks = crate::gql::MergeAsks::default();
            let (got, ask) = crate::gql::test_merge(r, &it);
            if let Some(pair) = ask {
                asks.single.insert(n, pair);
            }
            let mut v = crate::webhooks::pull_request(r, &it);
            let open = it.pull.as_ref().filter(|(_, h)| matches!(h.state, graftyard_git::issues::PrState::Open { .. }));
            if let (Some(ok), Some((_, head))) = (got, open) {
                let state = match head.state {
                    graftyard_git::issues::PrState::Open { draft: true } => "draft".to_owned(),
                    _ => crate::gql::merge_state(r, &it, got).to_ascii_lowercase(),
                };
                v["mergeable"] = json!(ok);
                v["mergeable_state"] = json!(state);
            }
            (Reply::json(200, v), asks)
        }
        _ => (Reply::not_found(), Default::default()),
    }
}

/// The pinned `issue-comment` schema's object.
pub fn issue_comment_json(r: &Repo<'_>, n: &Item, id: CommentId, row: &CommentRow) -> Value {
    let (api, web) = (r.host.api(), r.host.web());
    let (o, name) = (&r.at.owner, &r.at.name);
    let kind = if n.is_pull() { "pull" } else { "issues" };
    json!({
        "url": format!("{api}/repos/{o}/{name}/issues/comments/{}", id.0.0),
        "html_url": format!("{web}/{o}/{name}/{kind}/{}#issuecomment-{}", n.n.0, id.0.0),
        "issue_url": format!("{api}/repos/{o}/{name}/issues/{}", n.n.0),
        "id": id.0.0, "node_id": scoped(NodeType::IssueComment, r, id.0.0),
        "user": simple_user(r.host, r.dir, row.author), "created_at": iso(row.created),
        "updated_at": iso(row.updated), "author_association": crate::wire::association(r, row.author),
        "performed_via_github_app": null, "body": &*row.body, "reactions": reactions(&format!(
            "{api}/repos/{o}/{name}/issues/comments/{}/reactions", id.0.0)),
    })
}

fn reactions(url: &str) -> Value {
    json!({ "url": url, "total_count": 0, "+1": 0, "-1": 0, "laugh": 0, "confused": 0, "heart": 0, "hooray": 0,
        "eyes": 0, "rocket": 0 })
}

fn paged_values(r: &Repo<'_>, path: &str, query: &str, all: Vec<Value>) -> Reply {
    let page = Page::parse(query);
    let more = all.len() > page.end();
    let items: Vec<Value> = all.into_iter().skip(page.skip()).take(page.per_page).collect();
    let link = graftyard_wire::link(&r.host.api(), r.at.id.0.0, path, &[], page, more, None);
    let reply = Reply::json(200, Value::Array(items));
    match link {
        Some(l) => reply.with("link", l),
        None => reply,
    }
}

/// `GET /repos/{owner}/{repo}/issues/{issue_number}/comments`, oldest first.
pub fn list_issue_comments(r: &Repo<'_>, n: &str, query: &str) -> Reply {
    let Some(n) = n.parse::<u32>().ok().map(IssueNumber) else { return Reply::not_found() };
    let Some(it) = r.item(n) else { return Reply::not_found() };
    let all = r.comments_of(n).into_iter().map(|(id, row)| issue_comment_json(r, &it, id, &row)).collect();
    paged_values(r, &format!("/issues/{}/comments", n.0), query, all)
}

/// `GET /repos/{owner}/{repo}/pulls/{pull_number}/reviews`, in review id order.
pub fn list_reviews(r: &Repo<'_>, n: &str, query: &str) -> Reply {
    let Some(num) = n.parse::<u32>().ok().map(IssueNumber) else { return Reply::not_found() };
    if r.item(num).is_none_or(|it| !it.is_pull()) {
        return Reply::not_found();
    }
    let (api, web) = (r.host.api(), r.host.web());
    let (o, name) = (&r.at.owner, &r.at.name);
    let mut rows = r.reviews_of(num);
    rows.retain(|(_, row)| row.state != ReviewState::Pending);
    rows.sort_by_key(|(id, _)| id.0);
    let all = rows
        .into_iter()
        .map(|(id, row)| {
            let state = if row.dismissed.is_some() {
                "DISMISSED"
            } else {
                match row.state {
                    ReviewState::Approved => "APPROVED",
                    ReviewState::ChangesRequested => "CHANGES_REQUESTED",
                    ReviewState::Commented | ReviewState::Pending => "COMMENTED",
                }
            };
            let html = format!("{web}/{o}/{name}/pull/{}#pullrequestreview-{}", num.0, id.0);
            let pull = format!("{api}/repos/{o}/{name}/pulls/{}", num.0);
            json!({
                "id": id.0, "node_id": scoped(NodeType::PullRequestReview, r, id.0),
                "user": simple_user(r.host, r.dir, row.reviewer), "body": &*row.body, "state": state,
                "html_url": html, "pull_request_url": pull,
                "_links": { "html": { "href": html }, "pull_request": { "href": pull } },
                "submitted_at": iso(row.at), "commit_id": row.commit.hex(),
                "author_association": crate::wire::association(r, row.reviewer),
            })
        })
        .collect();
    paged_values(r, &format!("/pulls/{}/reviews", num.0), query, all)
}

/// The pinned `pull-request-review-comment` schema's object.
pub(crate) fn review_comment_json(r: &Repo<'_>, num: IssueNumber, id: CommentId, row: &CommentRow) -> Option<Value> {
    let (api, web) = (r.host.api(), r.host.web());
    let (o, name) = (&r.at.owner, &r.at.name);
    let CommentTarget::Review { review, at, .. } = &row.on else {
        return None;
    };
    let url = format!("{api}/repos/{o}/{name}/pulls/comments/{}", id.0.0);
    let html = format!("{web}/{o}/{name}/pull/{}#discussion_r{}", num.0, id.0.0);
    let pull = format!("{api}/repos/{o}/{name}/pulls/{}", num.0);
    let side = match at.side {
        DiffSide::Left => "LEFT",
        DiffSide::Right => "RIGHT",
    };
    let mut v = json!({
        "url": url, "id": id.0.0, "node_id": scoped(NodeType::IssueComment, r, id.0.0),
        "pull_request_review_id": review.map(|id| id.0.0),
        "diff_hunk": "", "path": &*at.path,
        "commit_id": at.commit.hex(), "original_commit_id": at.commit.hex(),
        "user": simple_user(r.host, r.dir, row.author), "body": &*row.body,
        "created_at": iso(row.created), "updated_at": iso(row.updated),
        "html_url": html, "pull_request_url": pull,
        "author_association": crate::wire::association(r, row.author),
        "_links": {
            "self": { "href": url }, "html": { "href": html }, "pull_request": { "href": pull },
        },
        "line": at.line, "original_line": at.line,
        "start_line": at.start_line, "original_start_line": at.start_line,
        "side": side, "start_side": at.start_line.map(|_| side), "subject_type": "line",
    });
    if let Some(reply) = row.reply_to {
        v["in_reply_to_id"] = json!(reply.0.0);
    }
    Some(v)
}

/// `GET /repos/{owner}/{repo}/pulls/{pull_number}/comments`, oldest first.
pub fn list_review_comments(r: &Repo<'_>, n: &str, query: &str) -> Reply {
    let Some(num) = n.parse::<u32>().ok().map(IssueNumber) else { return Reply::not_found() };
    if r.item(num).is_none_or(|it| !it.is_pull()) {
        return Reply::not_found();
    }
    let all =
        r.review_comments_of(num).into_iter().filter_map(|(id, row)| review_comment_json(r, num, id, &row)).collect();
    paged_values(r, &format!("/pulls/{}/comments", num.0), query, all)
}

/// `POST /repos/{owner}/{repo}/pulls/{pull_number}/comments`: a new review thread, or a reply that inherits its
/// parent's line anchor.
pub fn plan_create_review_comment(
    r: &Repo<'_>,
    n: &str,
    body: &Value,
) -> Result<graftyard_git::issue_ops::IssueOp, Reply> {
    const DOC: &str = "rest/pulls/comments#create-a-review-comment-for-a-pull-request";
    let invalid = || Reply::error(422, "Validation Failed", DOC);
    let Some(num) = n.parse::<u32>().ok().map(IssueNumber) else { return Err(Reply::not_found()) };
    if r.item(num).is_none_or(|it| !it.is_pull()) {
        return Err(Reply::not_found());
    }
    let text = body.get("body").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(invalid)?;
    if let Some(parent) = body.get("in_reply_to") {
        let parent = parent.as_i64().ok_or_else(invalid)?;
        let Some((id, at)) = r.review_comments_of(num).into_iter().find_map(|(id, row)| {
            if id.0.0 != parent {
                return None;
            }
            let CommentTarget::Review { at, .. } = row.on else { return None };
            Some((id, at))
        }) else {
            return Err(Reply::not_found());
        };
        return Ok(graftyard_git::issue_ops::IssueOp::Comment {
            on: CommentTarget::Review { n: num, review: None, at },
            body: text.into(),
            reply_to: Some(id),
        });
    }
    let commit = body
        .get("commit_id")
        .and_then(Value::as_str)
        .and_then(|s| ObjectId::parse_hex(s.as_bytes()))
        .ok_or_else(invalid)?;
    let path = body.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(invalid)?;
    let positive = |v: Option<&Value>| -> Option<u32> {
        v?.as_u64().filter(|n| *n > 0 && *n <= u64::from(u32::MAX)).map(|n| n as u32)
    };
    let line = positive(body.get("line")).ok_or_else(invalid)?;
    let side = match body.get("side") {
        None => DiffSide::Right,
        Some(Value::String(s)) if s == "RIGHT" => DiffSide::Right,
        Some(Value::String(s)) if s == "LEFT" => DiffSide::Left,
        _ => return Err(invalid()),
    };
    let start_line = match body.get("start_line") {
        None => None,
        Some(v) => Some(positive(Some(v)).filter(|start| *start < line).ok_or_else(invalid)?),
    };
    if body.get("start_side").is_some_and(|v| {
        v.as_str()
            != Some(match side {
                DiffSide::Left => "LEFT",
                DiffSide::Right => "RIGHT",
            })
    }) {
        return Err(invalid());
    }
    if body.get("subject_type").is_some_and(|v| v.as_str() != Some("line")) {
        return Err(invalid());
    }
    Ok(graftyard_git::issue_ops::IssueOp::Comment {
        on: CommentTarget::Review {
            n: num,
            review: None,
            at: LineAnchor { commit, path: path.into(), line, start_line, side },
        },
        body: text.into(),
        reply_to: None,
    })
}

/// The 201 of a committed review comment, read from the member after the entry applied.
pub fn review_comment_created(
    acked: &graftyard_raft::Acked<graftyard_git::issue_ops::IssueOut>,
    after: &Repo<'_>,
    n: u32,
) -> Reply {
    let graftyard_git::issue_ops::IssueOut::Id(id) = acked.out else {
        return Reply::error(500, "Internal Server Error", "rest");
    };
    let id = CommentId(id);
    match after.review_comments_of(IssueNumber(n)).into_iter().find(|(found, _)| *found == id) {
        Some((id, row)) => match review_comment_json(after, IssueNumber(n), id, &row) {
            Some(v) => Reply::json(201, v),
            None => Reply::error(500, "Internal Server Error", "rest"),
        },
        None => Reply::error(500, "Internal Server Error", "rest"),
    }
}

/// `GET /repos/{owner}/{repo}/commits/{ref}/check-runs`.
pub fn list_check_runs_for_ref(r: &Repo<'_>, reference: &str, query: &str) -> Reply {
    let Some(sha) = resolve(r, reference) else {
        return Reply::error(
            422,
            &format!("No commit found for SHA: {reference}"),
            "rest/checks/runs#list-check-runs-for-a-git-reference",
        );
    };
    let mut rows = r.check_runs(&sha);
    rows.sort_by_key(|t| std::cmp::Reverse(t.rec.run));
    let total_count = rows.len();
    let page = Page::parse(query);
    let more = total_count > page.end();
    let check_runs: Vec<Value> =
        rows.iter().skip(page.skip()).take(page.per_page).map(|t| check_run_json(r, &t.rec, t.at)).collect();
    let link = graftyard_wire::link(
        &r.host.api(),
        r.at.id.0.0,
        &format!("/commits/{}/check-runs", sha.hex()),
        &[],
        page,
        more,
        None,
    );
    let reply = Reply::json(200, json!({ "total_count": total_count, "check_runs": check_runs }));
    match link {
        Some(l) => reply.with("link", l),
        None => reply,
    }
}

/// `GET /repos/{owner}/{repo}/check-runs/{check_run_id}`.
pub fn get_check_run(r: &Repo<'_>, id: &str) -> Reply {
    let Some(id) = id.parse::<i64>().ok() else { return Reply::not_found() };
    match r.check_run(CheckRunId(DbId(id))) {
        Some(t) => Reply::json(200, check_run_json(r, &t.rec, t.at)),
        None => Reply::not_found(),
    }
}

/// The pinned `check-run` schema's object, shared with its webhook.
pub fn check_run_json(r: &Repo<'_>, c: &CheckRunUpdate, at: Timestamp) -> Value {
    let (api, web) = (r.host.api(), r.host.web());
    let (o, name) = (&r.at.owner, &r.at.name);
    let id = c.run.0.0;
    json!({
        "id": id, "node_id": scoped(NodeType::CheckRun, r, id), "head_sha": c.head_sha.hex(), "name": &*c.name,
        "url": format!("{api}/repos/{o}/{name}/check-runs/{id}"),
        "html_url": format!("{web}/{o}/{name}/runs/{id}"),
        "details_url": c.details_url.as_ref().map(|u| &*u.0), "external_id": c.external_id.as_deref().unwrap_or(""),
        "status": check_status(c.status), "conclusion": c.conclusion.map(conclusion),
        "started_at": c.started.map(iso), "completed_at": c.completed.map(iso),
        "check_suite": {
            "id": c.suite.0.0, "node_id": scoped(NodeType::CheckRun, r, c.suite.0.0),
            "head_sha": c.head_sha.hex(), "url": format!("{api}/repos/{o}/{name}/check-suites/{}", c.suite.0.0),
            "status": check_status(c.status), "conclusion": c.conclusion.map(conclusion),
            "before": null, "after": c.head_sha.hex(), "head_branch": null, "pull_requests": [], "app": null,
            "created_at": iso(at), "updated_at": iso(at),
        },
        "output": {
            "title": c.output.as_ref().map(|x| &*x.title), "summary": c.output.as_ref().map(|x| &*x.summary),
            "text": c.output.as_ref().and_then(|x| x.text.as_deref()),
            "annotations_count": c.output.as_ref().map_or(0, |x| x.annotations.len()),
            "annotations_url": format!("{api}/repos/{o}/{name}/check-runs/{id}/annotations"),
        },
        "app": null, "pull_requests": [],
    })
}

pub(crate) fn check_status(s: CheckStatus) -> &'static str {
    match s {
        CheckStatus::Queued => "queued",
        CheckStatus::InProgress => "in_progress",
        CheckStatus::Completed => "completed",
        CheckStatus::Waiting | CheckStatus::Requested | CheckStatus::Pending => "pending",
    }
}

/// REST's and the webhook's lower-case conclusion.
pub(crate) fn conclusion(c: CheckConclusion) -> &'static str {
    match c {
        CheckConclusion::ActionRequired => "action_required",
        CheckConclusion::Cancelled => "cancelled",
        CheckConclusion::Failure => "failure",
        CheckConclusion::Neutral => "neutral",
        CheckConclusion::Success => "success",
        CheckConclusion::Skipped => "skipped",
        CheckConclusion::Stale => "stale",
        CheckConclusion::TimedOut => "timed_out",
        CheckConclusion::StartupFailure => "startup_failure",
    }
}

/// `POST /repos/{owner}/{repo}/check-runs`'s command. The edge supplies the App; `decide` gives the run and its suite
/// their id from the repository's `IdBlock` cursor (`IssueOp::CreateCheck`), so the ids here are placeholders.
pub fn plan_create_check_run(
    body: &Value,
    app: graftyard_identity::AppId,
    now: Timestamp,
) -> Result<graftyard_git::issue_ops::IssueOp, Reply> {
    const DOC: &str = "rest/checks/runs#create-a-check-run";
    let invalid = || Reply::error(422, "Validation Failed", DOC);
    let name = body.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(invalid)?;
    let head_sha = body
        .get("head_sha")
        .and_then(Value::as_str)
        .and_then(|s| ObjectId::parse_hex(s.as_bytes()))
        .ok_or_else(invalid)?;
    let status_text = match body.get("status") {
        None => "queued",
        Some(Value::String(s)) => s,
        Some(_) => return Err(invalid()),
    };
    let mut status = match status_text {
        "queued" => CheckStatus::Queued,
        "in_progress" => CheckStatus::InProgress,
        "completed" => CheckStatus::Completed,
        "waiting" => CheckStatus::Waiting,
        "requested" => CheckStatus::Requested,
        "pending" => CheckStatus::Pending,
        _ => return Err(invalid()),
    };
    let conclusion = match body.get("conclusion") {
        None => None,
        Some(Value::String(s)) => Some(match s.as_str() {
            "action_required" => CheckConclusion::ActionRequired,
            "cancelled" => CheckConclusion::Cancelled,
            "failure" => CheckConclusion::Failure,
            "neutral" => CheckConclusion::Neutral,
            "success" => CheckConclusion::Success,
            "skipped" => CheckConclusion::Skipped,
            "stale" => CheckConclusion::Stale,
            "timed_out" => CheckConclusion::TimedOut,
            _ => return Err(invalid()),
        }),
        Some(_) => return Err(invalid()),
    };
    if conclusion.is_some() {
        status = CheckStatus::Completed;
    }
    if status == CheckStatus::Completed && conclusion.is_none() {
        return Err(invalid());
    }
    let time = |key: &str| -> Result<Option<Timestamp>, Reply> {
        match body.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => graftyard_wire::parse_iso(s).map(Some).ok_or_else(invalid),
            Some(_) => Err(invalid()),
        }
    };
    let started = Some(time("started_at")?.unwrap_or(now));
    let completed = match time("completed_at")? {
        Some(at) => Some(at),
        None if status == CheckStatus::Completed => Some(now),
        None => None,
    };
    let details_url = match body.get("details_url") {
        None => None,
        Some(Value::String(s)) => Some(Url(s.as_str().into())),
        Some(_) => return Err(invalid()),
    };
    let external_id = match body.get("external_id") {
        None => None,
        Some(Value::String(s)) => Some(s.as_str().into()),
        Some(_) => return Err(invalid()),
    };
    let output = match body.get("output") {
        None => None,
        Some(Value::Object(o)) => {
            let required = |key: &str| o.get(key).and_then(Value::as_str).map(Box::<str>::from).ok_or_else(invalid);
            let text = match o.get("text") {
                None => None,
                Some(Value::String(s)) => Some(s.as_str().into()),
                Some(_) => return Err(invalid()),
            };
            let annotations = match o.get("annotations") {
                None => Vec::new(),
                Some(Value::Array(rows)) if rows.len() <= 50 => rows
                    .iter()
                    .map(|row| {
                        let Some(row) = row.as_object() else { return Err(invalid()) };
                        let positive = |key: &str| -> Result<u32, Reply> {
                            row.get(key)
                                .and_then(Value::as_u64)
                                .filter(|n| *n > 0 && *n <= u64::from(u32::MAX))
                                .map(|n| n as u32)
                                .ok_or_else(invalid)
                        };
                        let path = row.get("path").and_then(Value::as_str).ok_or_else(invalid)?;
                        let message = row.get("message").and_then(Value::as_str).ok_or_else(invalid)?;
                        let level = match row.get("annotation_level").and_then(Value::as_str) {
                            Some("notice") => AnnotationLevel::Notice,
                            Some("warning") => AnnotationLevel::Warning,
                            Some("failure") => AnnotationLevel::Failure,
                            _ => return Err(invalid()),
                        };
                        let title = match row.get("title") {
                            None => None,
                            Some(Value::String(s)) => Some(s.as_str().into()),
                            Some(_) => return Err(invalid()),
                        };
                        Ok(CheckAnnotation {
                            path: path.into(),
                            start_line: positive("start_line")?,
                            end_line: positive("end_line")?,
                            level,
                            message: message.into(),
                            title,
                        })
                    })
                    .collect::<Result<Vec<_>, Reply>>()?,
                Some(_) => return Err(invalid()),
            };
            Some(CheckOutput { title: required("title")?, summary: required("summary")?, text, annotations })
        }
        Some(_) => return Err(invalid()),
    };
    Ok(graftyard_git::issue_ops::IssueOp::CreateCheck(CheckRunUpdate {
        run: CheckRunId(DbId(0)),
        suite: CheckSuiteId(DbId(0)),
        app,
        head_sha,
        name: name.into(),
        version: 1,
        status,
        conclusion,
        started,
        completed,
        external_id,
        details_url,
        output,
    }))
}

/// The 201 of a committed check-run create, rendered from its row after the entry applied: the id `decide` assigned,
/// from the `&Acked`. `minted` is the id an `lf4-rest-2` edge minted and sent in `What::CheckRunCreated`, read only
/// when the ack carries no id, as a `RecordCheck` it sent acks.
pub fn check_run_created(
    acked: &graftyard_raft::Acked<graftyard_git::issue_ops::IssueOut>,
    after: &Repo<'_>,
    minted: i64,
) -> Reply {
    let id = match acked.out {
        graftyard_git::issue_ops::IssueOut::Id(id) => id.0,
        _ => minted,
    };
    match after.check_run(CheckRunId(DbId(id))) {
        Some(t) => Reply::json(201, check_run_json(after, &t.rec, t.at)),
        None => Reply::error(500, "Internal Server Error", "rest"),
    }
}

/// A refused REST review-comment or check-run write.
pub fn write_refused(e: &graftyard_git::issue_ops::IssueErr, doc: &str) -> Reply {
    use graftyard_git::issue_ops::IssueErr as E;
    match e {
        E::Unauthorized(_) => Reply::error(401, "Bad credentials", doc),
        E::Forbidden => Reply::error(403, "Resource not accessible by integration", doc),
        E::Agent(a) => Reply::error(403, &a.text, doc),
        E::NotFound | E::NotPull => Reply::not_found(),
        E::NeedIdBlock | E::Unavailable => Reply::error(503, "Service Unavailable", doc),
        _ => Reply::error(422, "Validation Failed", doc),
    }
}

fn raw_commit_user(raw: &[u8]) -> Option<Value> {
    let close = raw.iter().rposition(|b| *b == b'>')?;
    let open = raw[..close].iter().rposition(|b| *b == b'<')?;
    let mut fields = raw[close + 1..].split(|b| b.is_ascii_whitespace()).filter(|f| !f.is_empty());
    let epoch = std::str::from_utf8(fields.next()?).ok()?.parse::<u64>().ok()?;
    fields.next()?;
    let name = raw[..open].strip_suffix(b" ").unwrap_or(&raw[..open]);
    Some(json!({
        "name": String::from_utf8_lossy(name),
        "email": String::from_utf8_lossy(&raw[open + 1..close]),
        "date": iso(Timestamp(epoch * 1000)),
    }))
}

/// `GET /repos/{owner}/{repo}/commits/{ref}` from the raw commit object.
pub fn get_commit(r: &Repo<'_>, sha: ObjectId, raw: &[u8]) -> Reply {
    let internal = || Reply::error(500, "Internal Server Error", "rest");
    let Some(split) = raw.windows(2).position(|w| w == b"\n\n") else { return internal() };
    let (headers, message) = (&raw[..split], &raw[split + 2..]);
    let mut tree = None;
    let mut parents = Vec::new();
    let mut author = None;
    let mut committer = None;
    for line in headers.split(|b| *b == b'\n') {
        if let Some(hex) = line.strip_prefix(b"tree ") {
            tree = ObjectId::parse_hex(hex);
        } else if let Some(hex) = line.strip_prefix(b"parent ") {
            let Some(parent) = ObjectId::parse_hex(hex) else { return internal() };
            parents.push(parent);
        } else if let Some(user) = line.strip_prefix(b"author ") {
            author = raw_commit_user(user);
        } else if let Some(user) = line.strip_prefix(b"committer ") {
            committer = raw_commit_user(user);
        }
    }
    let (Some(tree), Some(author), Some(committer)) = (tree, author, committer) else { return internal() };
    let message = String::from_utf8_lossy(message.strip_suffix(b"\n").unwrap_or(message));
    let (api, web) = (r.host.api(), r.host.web());
    let (o, name) = (&r.at.owner, &r.at.name);
    let sha = sha.hex();
    let url = format!("{api}/repos/{o}/{name}/commits/{sha}");
    let parents: Vec<Value> = parents
        .into_iter()
        .map(|parent| {
            let sha = parent.hex();
            json!({
                "sha": sha,
                "url": format!("{api}/repos/{o}/{name}/commits/{sha}"),
                "html_url": format!("{web}/{o}/{name}/commit/{sha}"),
            })
        })
        .collect();
    Reply::json(
        200,
        json!({
            "sha": sha, "url": url, "html_url": format!("{web}/{o}/{name}/commit/{sha}"),
            "comments_url": format!("{url}/comments"),
            "node_id": graftyard_git::pack::b64::encode(
                format!("06:Commit{}:{sha}", r.at.id.0.0).as_bytes()),
            "commit": {
                "url": format!("{api}/repos/{o}/{name}/git/commits/{sha}"),
                "author": author, "committer": committer, "message": message, "comment_count": 0,
                "tree": {
                    "sha": tree.hex(), "url": format!("{api}/repos/{o}/{name}/git/trees/{}", tree.hex()),
                },
                "verification": {
                    "verified": false, "reason": "unsigned", "payload": null, "signature": null, "verified_at": null,
                },
            },
            "author": null, "committer": null, "parents": parents,
        }),
    )
}

pub fn commit_not_found(reference: &str) -> Reply {
    Reply::error(422, &format!("No commit found for SHA: {reference}"), "rest/commits/commits#get-a-commit")
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
        want.is_empty() || {
            let mine: Vec<String> = r.labels_of(it.n).iter().map(|(_, l)| l.name.0.to_lowercase()).collect();
            want.iter().all(|w| mine.contains(w))
        }
    };
    let sort = param(query, "sort");
    if !matches!(sort, None | Some("created" | "updated" | "comments")) {
        return Reply::error(422, "Validation Failed", ISSUES_DOC);
    }
    let page = Page::parse(query);
    // Filter, sort, then page, over every item: a selection (`crate::narrow`).
    // With no label filter, the sort keys are all on the `issue` row, so the selection reads the `issue` table in a
    // few scans and no `pull` or `pull_head` row; with labels, every item as before.
    let mut select = || {
        let mut all: Vec<(IssueNumber, IssueRow)> = Vec::new();
        if want.is_empty() {
            let opens = state.opens();
            for (k, cols) in r.meta.prefix("issue", &[]) {
                let (Some(Val::Int(n)), Some(Val::Blob(b))) = (k.first(), cols.first()) else { continue };
                let row: IssueRow = postcard::from_bytes(b).expect("a fold row this release wrote");
                if opens.contains(&row.open) {
                    all.push((IssueNumber(*n as u32), row));
                }
            }
        } else {
            for kind in [graftyard_git::issues::IssueKind::Issue, graftyard_git::issues::IssueKind::PullRequest] {
                all.extend(r.list(kind, state, None, usize::MAX, &has).0.into_iter().map(|i| (i.n, i.row)));
            }
        }
        match sort {
            Some("updated") => all.sort_by_key(|(n, row)| (row.updated, *n)),
            Some("comments") => all.sort_by_key(|(n, row)| (row.comments, *n)),
            _ => all.sort_by_key(|(n, row)| (row.created, *n)),
        }
        if param(query, "direction") != Some("asc") {
            all.reverse();
        }
        let more = all.len() > page.end();
        Pick::of_numbers(all.iter().skip(page.skip()).take(page.per_page).map(|(n, _)| *n), all.len(), more)
    };
    let pick = r.dir.pick(&format!("issues?{query}"), &mut select);
    let more = pick.more;
    let items: Vec<Value> = pick.numbers().into_iter().filter_map(|n| r.item(n)).map(|it| issue_json(r, &it)).collect();
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

// ---- issue labels (B0-36's labels write) ----

const ISSUE_LABELS_DOC: &str = "rest/issues/labels#add-labels-to-an-issue";

/// A path segment percent-decoded (a label name such as `good first issue` arrives as `good%20first%20issue`).
fn unpercent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (b.get(i + 1).and_then(|c| hex(*c)), b.get(i + 2).and_then(|c| hex(*c)))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The label names of a `POST` or `PUT /repos/{o}/{r}/issues/{n}/labels` body: `{"labels": [..]}`, a bare array, or
/// one string, each name or `{"name": ..}`, as GHES 3.22's description allows.
fn label_names(body: &Value) -> Option<Vec<String>> {
    let list = match body {
        Value::Object(m) => m.get("labels")?,
        other => other,
    };
    let one = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("name").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    };
    match list {
        Value::Array(a) => a.iter().map(one).collect(),
        Value::String(s) => Some(vec![s.clone()]),
        _ => None,
    }
}

/// `POST`, `PUT`, `DELETE /repos/{o}/{r}/issues/{n}/labels`, and `DELETE .../labels/{name}`: one `ApplyLabels`. `POST`
/// adds, `PUT` replaces (removes every label the issue has that the body does not name), `DELETE` removes all or
/// one. A name with no label is 422 for `POST` and `PUT`, as GHES answers when it cannot create the label, and 404
/// for a `DELETE` of one name. `None` with nothing to change.
pub fn plan_issue_labels(
    r: &Repo<'_>,
    method: &str,
    n: &str,
    name: Option<&str>,
    body: &Value,
) -> Result<graftyard_git::issue_ops::IssueOp, Reply> {
    use graftyard_git::issues::LabelId;
    let n = n
        .parse::<u32>()
        .ok()
        .map(graftyard_git::issues::IssueNumber)
        .filter(|n| r.item(*n).is_some())
        .ok_or_else(Reply::not_found)?;
    let all = r.labels();
    let id_of = |name: &str| all.iter().find(|(_, l)| l.name.0.eq_ignore_ascii_case(name)).map(|(id, _)| LabelId(*id));
    let have: Vec<LabelId> = r.labels_of(n).iter().map(|(id, _)| LabelId(*id)).collect();
    let invalid = || Reply::error(422, "Validation Failed", ISSUE_LABELS_DOC);
    let named = |body: &Value| -> Result<Vec<LabelId>, Reply> {
        label_names(body).ok_or_else(invalid)?.iter().map(|x| id_of(x).ok_or_else(invalid)).collect()
    };
    let (add, remove): (Vec<LabelId>, Vec<LabelId>) = match (method, name) {
        ("POST", None) => {
            let want = named(body)?;
            if want.is_empty() {
                return Err(invalid());
            }
            (want.into_iter().filter(|l| !have.contains(l)).collect(), Vec::new())
        }
        ("PUT", None) => {
            let want = named(body)?;
            let add = want.iter().filter(|l| !have.contains(l)).copied().collect();
            (add, have.iter().filter(|l| !want.contains(l)).copied().collect())
        }
        ("DELETE", None) => (Vec::new(), have.clone()),
        ("DELETE", Some(x)) => {
            let id = id_of(&unpercent(x)).filter(|id| have.contains(id)).ok_or_else(|| {
                Reply::error(404, "Label does not exist", "rest/issues/labels#remove-a-label-from-an-issue")
            })?;
            (Vec::new(), vec![id])
        }
        _ => return Err(Reply::error(405, "Method Not Allowed", ISSUE_LABELS_DOC)),
    };
    Ok(graftyard_git::issue_ops::IssueOp::ApplyLabels {
        n,
        add: add.into_iter().collect(),
        remove: remove.into_iter().collect(),
    })
}

/// The reply of a committed `ApplyLabels`: the issue's labels after the entry applied, 200, or 204 for `DELETE` of
/// every label.
pub fn issue_labels_ok(after: &Repo<'_>, n: u32, empty: bool) -> Reply {
    if empty {
        return Reply::empty(204);
    }
    let labels: Vec<Value> = after
        .labels_of(graftyard_git::issues::IssueNumber(n))
        .iter()
        .map(|(id, l)| label_json(after, *id, l))
        .collect();
    Reply::json(200, Value::Array(labels))
}

/// A refused `ApplyLabels`: an agent rule is 403 with its text, a missing permission 403, an unknown label 422.
pub fn issue_labels_refused(e: &graftyard_git::issue_ops::IssueErr) -> Reply {
    use graftyard_git::issue_ops::IssueErr as E;
    match e {
        E::Forbidden | E::Unauthorized(_) => {
            Reply::error(403, "Resource not accessible by integration", ISSUE_LABELS_DOC)
        }
        E::Agent(a) => Reply::error(403, &a.text, ISSUE_LABELS_DOC),
        E::NotFound => Reply::not_found(),
        _ => Reply::error(422, "Validation Failed", ISSUE_LABELS_DOC),
    }
}
