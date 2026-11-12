//! The `webhooks` surface at T0 (`design/SHARED.md`, "Surfaces, tiers, and oracles", the `webhooks` row, and "Event
//! fan-out"; `build/BUILD-PLAN.md`, M4-WP1): payloads rendered against `ghes-3.22.json`'s `x-webhooks` schemas, the
//! delivery request with `X-Hub-Signature-256`, the REST `hook` object, and the deliveries endpoints' objects.
//!
//! A payload is rendered from the fold the consumer reads with the entry: the entry's captured fields where SHARED
//! captures them (a push's SHAs, a status, a check run), and the issue, pull request, and comment rows otherwise. The
//! consumer renders each entry as soon as it reads it, so a delayed delivery still carries what it said then: the
//! body is stored in the `HookDelivery` row, and an attempt or a redelivery sends those bytes.
//!
//! `merge_group` is dotcom's (`api.github.com.json` at the pinned commit; GHES 3.22's description has no
//! `merge_group`), which the build plan asks for at T0 because `on: merge_group` workflows need it.
//!
//! Nothing here changes any reply to a client that uses no webhooks: every function is new, and no existing renderer
//! calls one.

use graftyard_git::hooks::{Guid, HookId, HookRecord, HookRow, Subject, Webhook};
use graftyard_git::issues::PrState;
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_git::sequencer::{Attempt, HookDelivery, Outcome};
use graftyard_identity::sealed::{HookSecret, signature_256};
use graftyard_types::{AccountId, Timestamp};
use serde_json::{Map, Value, json};

use crate::view::{Item, Repo};
use crate::wire::{NodeId, NodeType, Reply, iso, scoped, simple_user};

/// The `User-Agent` GHES sends (`research/wire-census.md` #36: "prefixed `GitHub-Hookshot/`") [unverified suffix].
pub const USER_AGENT: &str = "GitHub-Hookshot/graftyard";

fn branch(r: &RefName) -> String {
    let s = r.display();
    s.strip_prefix("refs/heads/").map(str::to_owned).unwrap_or(s)
}

/// The webhook `repository` object: `full-repository`'s fields, which cover every field the 3.22 webhook schemas
/// require, without the caller's `permissions`.
pub fn repository(r: &Repo<'_>) -> Value {
    let mut v = crate::repos::full_repository(r);
    if let Some(m) = v.as_object_mut() {
        m.remove("permissions");
    }
    v
}

fn sender(r: &Repo<'_>, who: Option<AccountId>) -> Value {
    match who {
        Some(a) => simple_user(r.host, r.dir, Some(a)),
        // A host bot (a consumer, a finisher): GitHub names `ghost` for an actor it no longer has [inference].
        None => ghost(r),
    }
}

fn ghost(r: &Repo<'_>) -> Value {
    let (api, web) = (r.host.api(), r.host.web());
    json!({
        "login": "ghost", "id": 10137, "node_id": NodeId::top(NodeType::User, 10137).encode(),
        "avatar_url": format!("{web}/identicons/ghost.png"), "gravatar_id": "",
        "url": format!("{api}/users/ghost"), "html_url": format!("{web}/ghost"),
        "followers_url": format!("{api}/users/ghost/followers"),
        "following_url": format!("{api}/users/ghost/following{{/other_user}}"),
        "gists_url": format!("{api}/users/ghost/gists{{/gist_id}}"),
        "starred_url": format!("{api}/users/ghost/starred{{/owner}}{{/repo}}"),
        "subscriptions_url": format!("{api}/users/ghost/subscriptions"),
        "organizations_url": format!("{api}/users/ghost/orgs"), "repos_url": format!("{api}/users/ghost/repos"),
        "events_url": format!("{api}/users/ghost/events{{/privacy}}"),
        "received_events_url": format!("{api}/users/ghost/received_events"), "type": "User", "site_admin": false,
    })
}

/// The top-level fields every repository event carries: `repository`, `sender`, and `organization` for an org.
fn envelope(r: &Repo<'_>, who: Option<AccountId>, mut body: Map<String, Value>) -> Value {
    body.insert("repository".into(), repository(r));
    body.insert("sender".into(), sender(r, who));
    if r.dir.is_org(r.at.owner_id) {
        let login = r.dir.login(r.at.owner_id).unwrap_or_default();
        let api = r.host.api();
        body.insert(
            "organization".into(),
            json!({
                "login": login, "id": r.at.owner_id.0.0,
                "node_id": NodeId::top(NodeType::Organization, r.at.owner_id.0.0).encode(),
                "url": format!("{api}/orgs/{login}"), "repos_url": format!("{api}/orgs/{login}/repos"),
                "events_url": format!("{api}/orgs/{login}/events"), "hooks_url": format!("{api}/orgs/{login}/hooks"),
                "issues_url": format!("{api}/orgs/{login}/issues"),
                "members_url": format!("{api}/orgs/{login}/members{{/member}}"),
                "public_members_url": format!("{api}/orgs/{login}/public_members{{/member}}"),
                "avatar_url": format!("{}/identicons/{login}.png", r.host.web()), "description": null,
            }),
        );
    }
    Value::Object(body)
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// The payload of `w`, or `None` when the fold no longer holds what it is about (an issue the entry names but the
/// tiers lack, which only a fold behind the entry could show).
pub fn payload(r: &Repo<'_>, w: &Webhook, hook: Option<(HookId, &HookRow)>, at: Timestamp) -> Option<Value> {
    let with_action = |m: Map<String, Value>| {
        let mut out = Map::new();
        if let Some(a) = w.action {
            out.insert("action".into(), json!(a));
        }
        // `edited` requires `changes`, none of whose members: the entry carries the new title or body, not the old
        // one, so the pure half sends `{}` [inference: a client reads `changes.*.from` only when present].
        if w.action == Some("edited") {
            out.insert("changes".into(), json!({}));
        }
        out.extend(m);
        out
    };
    Some(match &w.subject {
        Subject::Push { r#ref, before, after, pusher } => {
            envelope(r, *pusher, push(r, r#ref, *before, *after, *pusher, at))
        }
        Subject::Ref { r#ref, pusher } => {
            let tag = r#ref.as_bytes().starts_with(b"refs/tags/");
            let name = r#ref.display();
            let short = name.strip_prefix(if tag { "refs/tags/" } else { "refs/heads/" }).unwrap_or(&name).to_owned();
            let s = r.settings();
            let mut m =
                obj(json!({ "ref": short, "ref_type": if tag { "tag" } else { "branch" }, "pusher_type": "user" }));
            if w.event == "create" {
                m.insert(
                    "master_branch".into(),
                    json!(s.as_ref().map_or("main".into(), |s| branch(&s.default_branch))),
                );
                m.insert("description".into(), json!(s.as_ref().and_then(|s| s.description.as_deref())));
            }
            envelope(r, *pusher, m)
        }
        Subject::Pull { n, before, after, actor } => {
            let it = r.item(*n)?;
            let mut m = obj(json!({ "number": n.0, "pull_request": pull_request(r, &it) }));
            if let (Some(b), Some(a)) = (before, after) {
                m.insert("before".into(), json!(b.hex()));
                m.insert("after".into(), json!(a.hex()));
            }
            envelope(r, *actor, with_action(m))
        }
        Subject::Queue { n, reason, actor } => {
            let it = r.item(*n)?;
            let mut m = obj(json!({ "number": n.0, "pull_request": pull_request(r, &it) }));
            if let Some(x) = reason {
                m.insert("reason".into(), json!(x));
            }
            envelope(r, *actor, with_action(m))
        }
        Subject::Issue { n, actor } => {
            let it = r.item(*n)?;
            envelope(r, *actor, with_action(obj(json!({ "issue": issue(r, &it) }))))
        }
        Subject::Comment { n, id, actor } => {
            let it = r.item(*n)?;
            let row = r.comments_of(*n).into_iter().find(|(c, _)| c == id)?.1;
            let comment = crate::rest::issue_comment_json(r, &it, *id, &row);
            envelope(r, *actor, with_action(obj(json!({ "issue": issue(r, &it), "comment": comment }))))
        }
        Subject::Status(s) => {
            let (api, web) = (r.host.api(), r.host.web());
            let (o, name) = (&r.at.owner, &r.at.name);
            let sha = s.sha.hex();
            let at = iso(at);
            let branches: Vec<Value> = r
                .core
                .prefix("ref", &[])
                .into_iter()
                .filter_map(|(k, _)| {
                    let n: RefName = match k.first() {
                        Some(graftyard_fold::Val::Blob(b)) => RefName::parse(b).ok()?,
                        _ => return None,
                    };
                    (n.as_bytes().starts_with(b"refs/heads/") && r.ref_value(&n) == Some(s.sha)).then(|| {
                        json!({ "name": branch(&n), "commit": { "sha": sha, "url": format!("{api}/repos/{o}/{name}/commits/{sha}") }, "protected": false })
                    })
                })
                .collect();
            let m = obj(json!({
                "id": s.id.0.0, "sha": sha, "name": format!("{o}/{name}"), "target_url": s.target_url.as_ref().map(|u| &*u.0),
                "context": &*s.context, "description": s.description.as_deref(),
                "state": crate::rest::status_state(s.state), "created_at": at, "updated_at": at, "branches": branches,
                "commit": {
                    "sha": sha, "node_id": scoped(NodeType::StatusContext, r, s.id.0.0),
                    "url": format!("{api}/repos/{o}/{name}/commits/{sha}"),
                    "html_url": format!("{web}/{o}/{name}/commit/{sha}"),
                    "comments_url": format!("{api}/repos/{o}/{name}/commits/{sha}/comments"),
                    "commit": {
                        "author": { "name": "", "email": "", "date": at },
                        "committer": { "name": "", "email": "", "date": at },
                        "message": "", "url": format!("{api}/repos/{o}/{name}/git/commits/{sha}"),
                        "tree": { "sha": ObjectId::ZERO.hex(), "url": format!("{api}/repos/{o}/{name}/git/trees/{}", ObjectId::ZERO.hex()) },
                        "comment_count": 0, "verification": { "verified": false, "reason": "unsigned", "payload": null, "signature": null, "verified_at": null },
                    },
                    "author": null, "committer": null, "parents": [],
                },
            }));
            envelope(r, Some(s.creator), m)
        }
        Subject::CheckRun(c) => {
            let check_run = crate::rest::check_run_json(r, c, at);
            envelope(r, None, with_action(obj(json!({ "check_run": check_run }))))
        }
        Subject::MergeGroup { base, head_ref, head, base_sha, reason } => {
            let at = iso(at);
            let mut m = obj(json!({ "merge_group": {
                "head_sha": head.hex(), "head_ref": head_ref.display(), "base_sha": base_sha.hex(),
                "base_ref": base.display(),
                "head_commit": { "id": head.hex(), "tree_id": ObjectId::ZERO.hex(), "message": "", "timestamp": at,
                    "author": { "name": "", "email": "" }, "committer": { "name": "", "email": "" } },
            }}));
            if let Some(x) = reason {
                m.insert("reason".into(), json!(x));
            }
            envelope(r, None, with_action(m))
        }
        Subject::Ping { hook: id } => {
            let (id, row) = hook.filter(|(h, _)| h == id)?;
            ping(r, id, row)
        }
    })
}

/// A `ping` for hook `id` (a new hook's, or `POST .../pings`).
pub fn ping(r: &Repo<'_>, id: HookId, row: &HookRow) -> Value {
    let zen = "Design for failure.";
    envelope(r, None, obj(json!({ "zen": zen, "hook_id": id.0.0, "hook": hook_json(r, id, row) })))
}

fn push(
    r: &Repo<'_>,
    name: &RefName,
    before: ObjectId,
    after: ObjectId,
    pusher: Option<AccountId>,
    at: Timestamp,
) -> Map<String, Value> {
    let (o, n) = (&r.at.owner, &r.at.name);
    let web = r.host.web();
    let (created, deleted) = (before == ObjectId::ZERO, after == ObjectId::ZERO);
    let login = pusher.and_then(|p| r.dir.login(p)).unwrap_or_else(|| "ghost".into());
    // The commits list needs the git cache: the pure half sends the head commit's id only [inference: no client the
    // T0 oracles name reads `commits[]` beyond the head].
    let head = (!deleted).then(|| {
        json!({
            "id": after.hex(), "tree_id": ObjectId::ZERO.hex(), "distinct": true, "message": "", "timestamp": iso(at),
            "url": format!("{web}/{o}/{n}/commit/{}", after.hex()),
            "author": { "name": login, "email": null }, "committer": { "name": login, "email": null },
            "added": [], "removed": [], "modified": [],
        })
    });
    let compare = if created {
        format!("{web}/{o}/{n}/commit/{}", after.hex())
    } else {
        format!("{web}/{o}/{n}/compare/{}...{}", &before.hex()[..12], &after.hex()[..12])
    };
    obj(json!({
        "ref": name.display(), "before": before.hex(), "after": after.hex(), "created": created, "deleted": deleted,
        "forced": false, "base_ref": null, "compare": compare,
        "commits": head.iter().cloned().collect::<Vec<_>>(), "head_commit": head,
        "pusher": { "name": login, "email": null },
    }))
}

fn reactions(url: &str) -> Value {
    json!({ "url": url, "total_count": 0, "+1": 0, "-1": 0, "laugh": 0, "confused": 0, "heart": 0, "hooray": 0,
        "eyes": 0, "rocket": 0 })
}

/// The webhook `issue` object: REST's issue with the fields the webhook schema adds.
fn issue(r: &Repo<'_>, it: &Item) -> Value {
    let mut v = crate::rest::issue_json(r, it);
    let api = r.host.api();
    let (o, n) = (&r.at.owner, &r.at.name);
    if let Some(m) = v.as_object_mut() {
        m.insert("active_lock_reason".into(), Value::Null);
        m.insert("reactions".into(), reactions(&format!("{api}/repos/{o}/{n}/issues/{}/reactions", it.n.0)));
        m.insert("performed_via_github_app".into(), Value::Null);
    }
    v
}

/// The webhook `pull_request` object (`pull-request` with the webhook's additions).
pub(crate) fn pull_request(r: &Repo<'_>, it: &Item) -> Value {
    let (api, web) = (r.host.api(), r.host.web());
    let (o, n) = (&r.at.owner, &r.at.name);
    let num = it.n.0;
    let row = &it.row;
    let Some((pull, head)) = &it.pull else { return Value::Null };
    let merged = matches!(head.state, PrState::Merged { .. });
    let (merge_commit, merged_by) = match head.state {
        PrState::Merged { merge_commit, by, .. } => (Some(merge_commit.hex()), Some(by.0)),
        _ => (None, None),
    };
    let url = format!("{api}/repos/{o}/{n}/pulls/{num}");
    let repo = repository(r);
    let side = |b: &RefName, sha: ObjectId| {
        json!({ "label": format!("{o}:{}", branch(b)), "ref": branch(b), "sha": sha.hex(),
            "user": simple_user(r.host, r.dir, Some(r.at.owner_id)), "repo": repo.clone() })
    };
    let base_tip = r.ref_value(&pull.base).unwrap_or(pull.base_sha);
    let c = r.dir.compare(base_tip, head.head_sha);
    let labels: Vec<Value> = r.labels_of(it.n).iter().map(|(id, l)| crate::rest::label_json(r, *id, l)).collect();
    let assignees: Vec<Value> = r.assignees_of(it.n).into_iter().map(|a| simple_user(r.host, r.dir, Some(a))).collect();
    let open = matches!(head.state, PrState::Open { .. });
    let draft = matches!(head.state, PrState::Open { draft: true });
    let link = |h: String| json!({ "href": h });
    let mut v = json!({
        "url": url, "id": row.id.0, "node_id": scoped(NodeType::PullRequest, r, row.id.0),
        "html_url": format!("{web}/{o}/{n}/pull/{num}"), "diff_url": format!("{web}/{o}/{n}/pull/{num}.diff"),
        "patch_url": format!("{web}/{o}/{n}/pull/{num}.patch"), "issue_url": format!("{api}/repos/{o}/{n}/issues/{num}"),
        "commits_url": format!("{url}/commits"), "review_comments_url": format!("{url}/comments"),
        "review_comment_url": format!("{api}/repos/{o}/{n}/pulls/comments{{/number}}"),
        "comments_url": format!("{api}/repos/{o}/{n}/issues/{num}/comments"),
        "statuses_url": format!("{api}/repos/{o}/{n}/statuses/{}", head.head_sha.hex()),
        "number": num, "state": if open { "open" } else { "closed" }, "locked": row.locked.is_some(),
        "title": &*row.title, "user": simple_user(r.host, r.dir, row.author),
        "body": (!row.body.is_empty()).then_some(&*row.body), "labels": labels, "milestone": null,
        "active_lock_reason": null, "created_at": iso(row.created), "updated_at": iso(row.updated),
        "closed_at": row.closed.map(iso), "merged_at": if merged { row.closed.map(iso) } else { None },
        "merge_commit_sha": merge_commit, "assignee": assignees.first().cloned().unwrap_or(Value::Null),
        "assignees": assignees, "requested_reviewers": [], "requested_teams": [],
        "head": side(&head.head.branch, head.head_sha), "base": side(&pull.base, base_tip),
    });
    let rest = json!({
        "_links": {
            "self": link(url.clone()), "html": link(format!("{web}/{o}/{n}/pull/{num}")),
            "issue": link(format!("{api}/repos/{o}/{n}/issues/{num}")),
            "comments": link(format!("{api}/repos/{o}/{n}/issues/{num}/comments")),
            "review_comments": link(format!("{url}/comments")),
            "review_comment": link(format!("{api}/repos/{o}/{n}/pulls/comments{{/number}}")),
            "commits": link(format!("{url}/commits")),
            "statuses": link(format!("{api}/repos/{o}/{n}/statuses/{}", head.head_sha.hex())),
        },
        "author_association": crate::wire::association(r, row.author), "auto_merge": null, "draft": draft,
        "merged": merged, "mergeable": null, "rebaseable": null, "mergeable_state": "unknown",
        "merged_by": merged_by.map(|b| simple_user(r.host, r.dir, Some(b))), "comments": row.comments,
        "review_comments": 0, "maintainer_can_modify": false, "commits": c.commits, "additions": c.additions,
        "deletions": c.deletions, "changed_files": c.changed_files,
    });
    v.as_object_mut().expect("object").extend(obj(rest));
    v
}

// ---- the delivery request ----

/// One attempt's request: the headers GHES 3.22 sends (`research/wire-census.md` #36; `x-webhooks` parameters) and
/// the body. `X-Hub-Signature-256` is set only for a hook with a secret, which `graft-ops` unwrapped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

/// The body as the hook's content type sends it: the JSON itself, or `payload=` and the URL-encoded JSON for `form`.
pub fn encode(record: &HookRecord, json: &[u8]) -> Vec<u8> {
    match record.content_type {
        graftyard_git::hooks::HookContentType::Json => json.to_vec(),
        graftyard_git::hooks::HookContentType::Form => {
            let mut out = b"payload=".to_vec();
            for b in json {
                match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => out.push(*b),
                    b' ' => out.push(b'+'),
                    _ => out.extend(format!("%{b:02X}").bytes()),
                }
            }
            out
        }
    }
}

/// The request for delivery `d` to hook `record`, from `host`. `secret` is the hook secret `graft-ops` unwrapped.
pub fn request(
    host: &str,
    d: &HookDelivery,
    record: &HookRecord,
    target: (&'static str, i64),
    secret: Option<&HookSecret>,
) -> Request {
    let body = encode(record, &d.body);
    let ct = match record.content_type {
        graftyard_git::hooks::HookContentType::Json => "application/json",
        graftyard_git::hooks::HookContentType::Form => "application/x-www-form-urlencoded",
    };
    let mut headers = vec![
        ("user-agent", USER_AGENT.to_owned()),
        ("content-type", ct.to_owned()),
        ("x-github-delivery", d.guid.to_string()),
        ("x-github-event", d.event.to_string()),
        ("x-github-hook-id", d.hook.0.0.to_string()),
        ("x-github-hook-installation-target-type", target.0.to_owned()),
        ("x-github-hook-installation-target-id", target.1.to_string()),
        ("x-github-enterprise-version", graftyard_wire::ENTERPRISE_VERSION.to_owned()),
        ("x-github-enterprise-host", host.to_owned()),
    ];
    if let Some(s) = secret {
        headers.push(("x-hub-signature-256", signature_256(s, &body)));
    }
    Request { url: record.url.0.to_string(), headers, body }
}

// ---- REST objects ----

/// GHES's `hook` object for a repository hook. The secret is never rendered: `config.secret` is `********` when set.
pub fn hook_json(r: &Repo<'_>, id: HookId, row: &HookRow) -> Value {
    let api = r.host.api();
    let (o, n) = (&r.at.owner, &r.at.name);
    let url = format!("{api}/repos/{o}/{n}/hooks/{}", id.0.0);
    let h = &row.record;
    let mut config = json!({
        "content_type": match h.content_type {
            graftyard_git::hooks::HookContentType::Json => "json",
            graftyard_git::hooks::HookContentType::Form => "form",
        },
        "insecure_ssl": if h.insecure_ssl { "1" } else { "0" }, "url": &*h.url.0,
    });
    if h.secret.is_some() {
        config["secret"] = json!("********");
    }
    json!({
        "type": "Repository", "id": id.0.0, "name": "web", "active": h.active,
        "events": h.events.iter().map(|e| &*e.0).collect::<Vec<_>>(), "config": config,
        "updated_at": iso(row.updated), "created_at": iso(row.created), "url": url,
        "test_url": format!("{url}/test"), "ping_url": format!("{url}/pings"), "deliveries_url": format!("{url}/deliveries"),
        "last_response": { "code": null, "status": "unused", "message": null },
    })
}

fn status_text(a: &Attempt) -> String {
    match (a.outcome, a.status) {
        (Outcome::Ok, _) => "OK".into(),
        (Outcome::TimedOut, _) => "timed out".into(),
        (Outcome::Failed, Some(s)) => format!("Invalid HTTP Response: {s}"),
        (Outcome::Failed, None) => "failed to connect".into(),
    }
}

/// `hook-delivery-item`: one attempt in the deliveries list.
pub fn delivery_item(d: &HookDelivery, a: &Attempt) -> Value {
    json!({
        "id": a.id, "guid": d.guid.to_string(), "delivered_at": iso(a.at), "redelivery": a.redelivery,
        "duration": f64::from(a.duration_ms) / 1000.0, "status": status_text(a),
        "status_code": a.status.unwrap_or(0), "event": &*d.event, "action": d.action.as_deref(),
        "installation_id": null, "repository_id": d.repo.map(|r| r.0.0), "throttled_at": null,
    })
}

/// `hook-delivery`: one attempt with its request and response. The request's headers are the ones the attempt sent,
/// without the signature, which GitHub shows but this store does not keep [inference: a stored HMAC adds nothing].
pub fn delivery(d: &HookDelivery, a: &Attempt, url: &str) -> Value {
    let mut v = delivery_item(d, a);
    let payload: Value = serde_json::from_slice(&d.body).unwrap_or(Value::Null);
    v["url"] = json!(url);
    v["request"] = json!({
        "headers": { "X-GitHub-Delivery": d.guid.to_string(), "X-GitHub-Event": &*d.event,
            "X-GitHub-Hook-ID": d.hook.0.0.to_string(), "User-Agent": USER_AGENT },
        "payload": payload,
    });
    v["response"] = json!({ "headers": {}, "payload": a.response.as_deref() });
    v
}

/// The deliveries list's `Link` header: `rel="next"` with the cursor of the last attempt listed, when `more`.
pub fn deliveries_link(r: &Repo<'_>, hook: HookId, last: Option<u64>, per_page: usize, more: bool) -> Option<String> {
    let last = last.filter(|_| more)?;
    let api = r.host.api();
    let (o, n) = (&r.at.owner, &r.at.name);
    let cursor = graftyard_wire::cursor(last as i64);
    Some(format!(
        "<{api}/repos/{o}/{n}/hooks/{}/deliveries?per_page={per_page}&cursor={cursor}>; rel=\"next\"",
        hook.0.0
    ))
}

/// The GUID a ping without a chain entry takes, from the host's random bytes.
pub fn ping_guid(random: [u8; 16]) -> Guid {
    Guid::from_bytes(random)
}

// ---- REST bodies ----

/// The REST docs URL GHES names in a hook route's errors.
pub const DOC: &str = "rest/repos/webhooks";

/// What a `POST .../hooks` or `PATCH .../hooks/{id}` body sets, over `cur` (`None` for a create): the record without
/// its secret, and the secret the edge asks `graft-ops` to wrap. `Some(None)` clears it; `None` keeps it. `retry` is
/// Graftyard's opt-in (`design/SHARED.md`, "Known deviations from GitHub"), a `config` key GHES ignores.
pub fn hook_body(cur: Option<&HookRecord>, body: &Value) -> Result<(HookRecord, Option<Option<String>>), Reply> {
    use graftyard_git::hooks::{EVENTS, HookContentType};
    use graftyard_identity::{EventKind, Url};
    let bad = |field: &'static str, code: &'static str| {
        let e = crate::wire::FieldError { resource: "Hook", field, code };
        Reply::json(422, crate::wire::validation_error("Validation Failed", DOC, &[e]))
    };
    if body.get("name").is_some_and(|n| n != "web") {
        return Err(bad("name", "invalid"));
    }
    let config = body.get("config");
    let url = match (config.and_then(|c| c.get("url")).and_then(Value::as_str), cur) {
        (Some(u), _) => u.to_owned(),
        (None, Some(c)) => c.url.0.to_string(),
        (None, None) => return Err(bad("url", "missing_field")),
    };
    let host = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    if !host.is_some_and(|h| !h.is_empty() && !h.starts_with('/')) || url.len() > 2_048 {
        return Err(bad("url", "invalid"));
    }
    let content_type = match config.and_then(|c| c.get("content_type")).and_then(Value::as_str) {
        Some("json") => HookContentType::Json,
        Some("form") => HookContentType::Form,
        Some(_) => return Err(bad("content_type", "invalid")),
        None => cur.map_or(HookContentType::Form, |c| c.content_type),
    };
    let insecure_ssl = match config.and_then(|c| c.get("insecure_ssl")) {
        Some(Value::String(s)) => s == "1",
        Some(Value::Number(n)) => n.as_i64() == Some(1),
        Some(_) => return Err(bad("insecure_ssl", "invalid")),
        None => cur.is_some_and(|c| c.insecure_ssl),
    };
    let retry = match config.and_then(|c| c.get("retry")) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "1",
        _ => cur.is_some_and(|c| c.retry),
    };
    let strings = |k: &str| -> Option<Result<Vec<String>, Reply>> {
        let a = body.get(k)?.as_array();
        Some(
            a.map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_owned)).collect())
                .ok_or_else(|| bad("events", "invalid")),
        )
    };
    let mut events: Vec<String> = match strings("events").transpose()? {
        Some(e) => e,
        None => cur.map_or_else(|| vec!["push".into()], |c| c.events.iter().map(|e| e.0.to_string()).collect()),
    };
    if let Some(add) = strings("add_events").transpose()? {
        events.extend(add);
    }
    if let Some(rm) = strings("remove_events").transpose()? {
        events.retain(|e| !rm.contains(e));
    }
    let mut seen = std::collections::BTreeSet::new();
    events.retain(|e| seen.insert(e.clone()));
    if events.is_empty() || events.iter().any(|e| e != "*" && !EVENTS.contains(&e.as_str())) {
        return Err(bad("events", "invalid"));
    }
    let active = body.get("active").and_then(Value::as_bool).unwrap_or(cur.is_none_or(|c| c.active));
    let secret = match config.and_then(|c| c.get("secret")) {
        Some(Value::String(s)) if s.is_empty() => Some(None),
        Some(Value::String(s)) => Some(Some(s.clone())),
        Some(Value::Null) => Some(None),
        _ => None,
    };
    let record = HookRecord {
        url: Url(url.into()),
        events: events.into_iter().map(|e| EventKind(e.into())).collect(),
        content_type,
        active,
        insecure_ssl,
        secret: cur.and_then(|c| c.secret.clone()),
        retry,
    };
    Ok((record, secret))
}

/// A committed `PutHook`'s reply, from its `&Acked`: 201 with `Location` for a create, else 200.
pub fn saved(acked: &graftyard_raft::Acked<graftyard_git::issue_ops::IssueOut>, r: &Repo<'_>, created: bool) -> Reply {
    let graftyard_git::issue_ops::IssueOut::Id(id) = acked.out else {
        return Reply::error(500, "a hook command answered no id", "rest");
    };
    let id = HookId(id);
    let Some(row) = graftyard_git::hooks::hook_row(r.core, id) else { return Reply::not_found() };
    let v = hook_json(r, id, &row);
    if created {
        let loc = v["url"].as_str().unwrap_or_default().to_owned();
        Reply::json(201, v).with("location", loc)
    } else {
        Reply::json(200, v)
    }
}

/// A refused hook command's reply.
pub fn refused(e: &graftyard_git::issue_ops::IssueErr) -> Reply {
    use graftyard_git::issue_ops::IssueErr;
    match e {
        IssueErr::Unauthorized(_) => Reply::error(401, "Bad credentials", "rest"),
        IssueErr::Forbidden | IssueErr::Agent(_) => Reply::not_found(),
        IssueErr::NotFound => Reply::not_found(),
        IssueErr::Exists => {
            let e = crate::wire::FieldError { resource: "Hook", field: "url", code: "custom" };
            Reply::json(422, crate::wire::validation_error("Validation Failed", DOC, &[e]))
        }
        IssueErr::NeedIdBlock | IssueErr::Unavailable => Reply::error(503, "Service Unavailable", "rest"),
        _ => Reply::error(422, "Validation Failed", DOC),
    }
}

// ---- the sequencer's routes ----

/// `POST .../hooks/{id}/pings`, `GET .../deliveries`, `GET .../deliveries/{id}`, and `POST .../deliveries/{id}/attempts`
/// on the owner's `HookSequencer` (`seq`), for hook `id` whose row the edge read. `tail` is the path after
/// `hooks/{id}`; `query` the raw query string; `random` the host's bytes for a ping's GUID. `None` for a route this
/// does not serve.
#[allow(clippy::too_many_arguments)]
pub fn sequencer_route(
    seq: &mut graftyard_git::sequencer::Sequencer,
    r: &Repo<'_>,
    id: HookId,
    row: &HookRow,
    method: &str,
    tail: &[&str],
    query: &str,
    now: Timestamp,
    random: [u8; 16],
) -> Option<Reply> {
    use graftyard_git::sequencer::{NewDelivery, RedeliverErr};
    let param = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('=')).map(str::to_owned);
    Some(match (method, tail) {
        ("POST", ["pings"]) => {
            let body = serde_json::to_vec(&ping(r, id, row)).unwrap_or_default();
            let d = NewDelivery {
                guid: ping_guid(random),
                hook: id,
                entry: None,
                event: "ping",
                action: None,
                repo: Some(r.at.id),
                body,
            };
            seq.set_targets(&[(id, row.record.clone())]);
            seq.ping(d, now);
            Reply::empty(204)
        }
        ("GET", ["deliveries"]) => {
            let per_page = param("per_page").and_then(|p| p.parse::<usize>().ok()).unwrap_or(30).clamp(1, 100);
            let before = match param("cursor") {
                Some(c) => match graftyard_wire::parse_cursor(&c) {
                    Some(n) => Some(n as u64),
                    None => return Some(Reply::error(400, "Bad Request", DOC)),
                },
                None => None,
            };
            let mut rows = seq.list(id, before, per_page + 1);
            let more = rows.len() > per_page;
            rows.truncate(per_page);
            let last = rows.last().map(|(_, a)| a.id);
            let v: Vec<Value> = rows.iter().map(|(d, a)| delivery_item(d, a)).collect();
            let reply = Reply::json(200, Value::Array(v));
            match deliveries_link(r, id, last, per_page, more) {
                Some(l) => reply.with("link", l),
                None => reply,
            }
        }
        ("GET", ["deliveries", n]) => match n.parse::<u64>().ok().and_then(|n| seq.get(id, n)) {
            Some((d, a)) => Reply::json(200, delivery(d, a, &row.record.url.0)),
            None => Reply::not_found(),
        },
        ("POST", ["deliveries", n, "attempts"]) => {
            let Ok(n) = n.parse::<u64>() else { return Some(Reply::not_found()) };
            seq.set_targets(&[(id, row.record.clone())]);
            match seq.redeliver(id, n, now) {
                Ok(_) => Reply::json(202, json!({})),
                Err(RedeliverErr::NotFound) => Reply::not_found(),
                Err(RedeliverErr::Expired) => {
                    let e = crate::wire::FieldError { resource: "HookDelivery", field: "delivery_id", code: "expired" };
                    Reply::json(422, crate::wire::validation_error("Validation Failed", DOC, &[e]))
                }
            }
        }
        _ => return None,
    })
}
