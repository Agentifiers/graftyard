//! `rest:collaborators` and `rest:teams` as GHES 3.22 shapes them (`build/BUILD-PLAN.md`, M3-WP3, BF-34), and the
//! invitee's side of repository and org invitations:
//!
//! - `GET /repos/{o}/{r}/collaborators[/{u}[/permission]]`, `PUT` and `DELETE /repos/{o}/{r}/collaborators/{u}`,
//!   `GET /repos/{o}/{r}/invitations`, `DELETE /repos/{o}/{r}/invitations/{id}`;
//! - `GET /user/repository_invitations`, `PATCH` (accept) and `DELETE` (decline) `/user/repository_invitations/{id}`;
//! - `POST /orgs/{org}/teams`, `GET /orgs/{org}/teams[/{slug}]`, `PUT` and `DELETE
//!   /orgs/{org}/teams/{slug}/memberships/{u}`, `PUT /orgs/{org}/teams/{slug}/repos/{o}/{r}`;
//! - `GET` and `PATCH /user/memberships/orgs/{org}`, which accepts a pending org invitation; a `DELETE` of it
//!   declines one [inference: GHES 3.22 lists no route that declines an org invitation; this install takes `DELETE`
//!   on the same path].
//!
//! Every write is one command on the chain that owns the fact (`design/SHARED.md`, `InvitationEvent`): the
//! repository chain for an outside collaborator, the org's `Authz` chain for a member, a team, or an org invitation.
//! `PUT .../collaborators/{u}` for a member of an org owner is refused with 422, since a member's permission is a
//! `Grant` on the org's chain (`IssueErr::OrgMember`). GHES 3.22's description lists 204, 403, and 422 for that `PUT`,
//! not the 201 with a `repository-invitation` GitHub.com answers, so a new invitation answers 204 here too.
//!
//! The pending repository invitations a user holds are in no fold the edge can read for the user alone (`Notify`'s
//! `Inbox` is not built), so `/user/repository_invitations` takes `?repo=o/r` and lists that repository's
//! invitations for the caller [inference]; without it the list is empty.

use graftyard_api::hosted::What;
use graftyard_git::issue_ops::IssueOp;
use graftyard_git::tiers::get_rec;
use graftyard_identity::{
    AuthzFold, AuthzRead, InvitationGrant, InvitationId, InvitationRow, Invitee, OrgRole, Principal, RepoPermission,
    TeamId, UserId,
};
use graftyard_types::{AccountId, ChainId, DbId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::api::{Read, commit_issue};
use crate::ident::{account_of, commit_owner, refused, user_json};

fn perm_name(p: RepoPermission) -> &'static str {
    match p {
        RepoPermission::Read => "read",
        RepoPermission::Triage => "triage",
        RepoPermission::Write => "write",
        RepoPermission::Maintain => "maintain",
        RepoPermission::Admin => "admin",
    }
}

/// The REST permission names: `pull`/`read`, `triage`, `push`/`write`, `maintain`, `admin`; `push` when absent.
fn parse_perm(s: Option<&str>) -> Option<RepoPermission> {
    Some(match s.unwrap_or("push") {
        "pull" | "read" => RepoPermission::Read,
        "triage" => RepoPermission::Triage,
        "push" | "write" => RepoPermission::Write,
        "maintain" => RepoPermission::Maintain,
        "admin" => RepoPermission::Admin,
        _ => return None,
    })
}

fn perms_json(p: Option<RepoPermission>) -> Value {
    let at = |x| p >= Some(x);
    json!({ "pull": at(RepoPermission::Read), "triage": at(RepoPermission::Triage), "push": at(RepoPermission::Write),
        "maintain": at(RepoPermission::Maintain), "admin": at(RepoPermission::Admin) })
}

/// GitHub's `simple-user`: `public-user` without the profile fields.
fn simple_user(c: &Ctx<'_>, id: AccountId, login: &str) -> Value {
    const PROFILE: [&str; 13] = [
        "name",
        "company",
        "blog",
        "location",
        "email",
        "hireable",
        "bio",
        "public_repos",
        "public_gists",
        "followers",
        "following",
        "created_at",
        "updated_at",
    ];
    let mut u = user_json(c, id, login, false);
    u.as_object_mut().expect("object").retain(|k, _| !PROFILE.contains(&k.as_str()));
    u
}

/// GitHub's `minimal-repository`, from what the edge knows of a repository.
fn minimal_repo(c: &Ctx<'_>, rd: &Read) -> Value {
    let (o, n) = (&rd.at.owner, &rd.at.name);
    let api = format!("https://{}/api/v3/repos/{o}/{n}", c.cfg.host);
    let u = |p: &str| format!("{api}{p}");
    let node = graftyard_wire::NodeId::top(graftyard_wire::NodeType::Repository, rd.at.id.0.0).encode();
    let mut v = json!({
        "id": rd.at.id.0.0, "node_id": node, "name": n, "full_name": format!("{o}/{n}"),
        "owner": simple_user(c, rd.at.owner_id, o), "private": true,
        "html_url": format!("https://{}/{o}/{n}", c.cfg.host), "description": null, "fork": false, "url": api,
    });
    const URLS: [(&str, &str); 36] = [
        ("archive_url", "/{archive_format}{/ref}"),
        ("assignees_url", "/assignees{/user}"),
        ("blobs_url", "/git/blobs{/sha}"),
        ("branches_url", "/branches{/branch}"),
        ("collaborators_url", "/collaborators{/collaborator}"),
        ("comments_url", "/comments{/number}"),
        ("commits_url", "/commits{/sha}"),
        ("compare_url", "/compare/{base}...{head}"),
        ("contents_url", "/contents/{+path}"),
        ("contributors_url", "/contributors"),
        ("deployments_url", "/deployments"),
        ("downloads_url", "/downloads"),
        ("events_url", "/events"),
        ("forks_url", "/forks"),
        ("git_commits_url", "/git/commits{/sha}"),
        ("git_refs_url", "/git/refs{/sha}"),
        ("git_tags_url", "/git/tags{/sha}"),
        ("issue_comment_url", "/issues/comments{/number}"),
        ("issue_events_url", "/issues/events{/number}"),
        ("issues_url", "/issues{/number}"),
        ("keys_url", "/keys{/key_id}"),
        ("labels_url", "/labels{/name}"),
        ("languages_url", "/languages"),
        ("merges_url", "/merges"),
        ("milestones_url", "/milestones{/number}"),
        ("notifications_url", "/notifications{?since,all,participating}"),
        ("pulls_url", "/pulls{/number}"),
        ("releases_url", "/releases{/id}"),
        ("stargazers_url", "/stargazers"),
        ("statuses_url", "/statuses/{sha}"),
        ("subscribers_url", "/subscribers"),
        ("subscription_url", "/subscription"),
        ("tags_url", "/tags"),
        ("teams_url", "/teams"),
        ("trees_url", "/git/trees{/sha}"),
        ("hooks_url", "/hooks"),
    ];
    let m = v.as_object_mut().expect("object");
    for (k, p) in URLS {
        m.insert(k.into(), json!(u(p)));
    }
    v
}

/// An account's login: the repository read's, else its `AccountLogin` row.
async fn login(c: &Ctx<'_>, rd: &Read, a: AccountId) -> String {
    match rd.dir.logins.get(&a.0.0) {
        Some(l) => l.clone(),
        None => crate::ident::login(c, a).await.unwrap_or_else(|| "ghost".into()),
    }
}

fn invitation_json(
    c: &Ctx<'_>,
    rd: &Read,
    id: InvitationId,
    row: &InvitationRow,
    logins: &[(AccountId, String)],
) -> Value {
    let name = |a: AccountId| logins.iter().find(|(x, _)| *x == a).map(|(_, l)| l.clone()).unwrap_or_default();
    let invitee = match row.invitee {
        Invitee::User(u) => simple_user(c, u.0, &name(u.0)),
        Invitee::Email(_) => Value::Null,
    };
    let inviter = row.inviter.map_or(Value::Null, |a| simple_user(c, a, &name(a)));
    let p = match row.grant {
        InvitationGrant::Repo(p) => perm_name(p),
        InvitationGrant::Org { .. } => "read",
    };
    let (o, n) = (&rd.at.owner, &rd.at.name);
    let node = graftyard_wire::NodeId::scoped(graftyard_wire::NodeType::Repository, rd.at.id, id.0.0).encode();
    json!({
        "id": id.0.0, "node_id": node, "repository": minimal_repo(c, rd), "invitee": invitee, "inviter": inviter,
        "permissions": p, "created_at": "1970-01-01T00:00:00Z", "expired": false,
        "url": format!("https://{}/api/v3/user/repository_invitations/{}", c.cfg.host, id.0.0),
        "html_url": format!("https://{}/{o}/{n}/invitations", c.cfg.host),
    })
}

fn invitations(rd: &Read) -> Vec<(InvitationId, InvitationRow)> {
    use graftyard_fold::Read as _;
    rd.core
        .prefix("invitation", &[])
        .into_iter()
        .filter_map(|(k, _)| {
            let graftyard_fold::Val::Int(id) = k.first()? else { return None };
            let row = get_rec::<InvitationRow>(&rd.core, "invitation", &k)?;
            (row.expires > crate::now()).then_some((InvitationId(DbId(*id)), row))
        })
        .collect()
}

fn collaborators(rd: &Read) -> Vec<AccountId> {
    use graftyard_fold::Read as _;
    rd.core
        .prefix("collaborator", &[])
        .into_iter()
        .filter_map(|(k, _)| match k.first()? {
            graftyard_fold::Val::Int(id) => Some(AccountId(DbId(*id))),
            _ => None,
        })
        .collect()
}

/// An account's permission on the repository: the owner's `Authz` grants and the collaborator rows.
fn permission(rd: &Read, user: AccountId) -> Option<RepoPermission> {
    let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
    let p = Principal {
        actor: graftyard_identity::Actor::User(UserId(user)),
        scopes: graftyard_identity::ScopeSet(1),
        credential: graftyard_identity::Credential::Stored([0; 32]),
        sso_orgs: Default::default(),
    };
    let folds: Vec<&dyn AuthzRead> = rd.dir.authz.iter().map(|a| a as &dyn AuthzRead).collect();
    graftyard_identity::permission(&p, &folds, &access)
}

fn need_user(who: Option<&Principal>) -> std::result::Result<(Principal, AccountId), Reply> {
    let who = who.ok_or_else(|| Reply::error(401, "Requires authentication", "rest"))?;
    match who.actor {
        graftyard_identity::Actor::User(u) => Ok((who.clone(), u.0)),
        _ => Err(Reply::error(403, "Resource not accessible by integration", "rest")),
    }
}

/// The repository routes this module serves, or `None` for one it does not.
pub async fn repo_route(
    c: &Ctx<'_>,
    req: &mut Request,
    rd: &Read,
    who: Option<&Principal>,
    tail: &[&str],
) -> Result<Option<Reply>> {
    let doc = "rest/collaborators/collaborators";
    let method = req.method();
    // The web app's chrome reads (`decisions/OPEN-QUESTIONS.md`, F61), for any caller who passed the read gate.
    if method == Method::Get && matches!(tail, ["assignees"] | ["teams"]) {
        return Ok(Some(people_list(c, req, rd, tail[0]).await?));
    }
    if !matches!(tail.first(), Some(&"collaborators" | &"invitations")) {
        return Ok(None);
    }
    let (who, me) = match need_user(who) {
        Ok(x) => x,
        Err(r) => return Ok(Some(r)),
    };
    let r = match (method, tail) {
        (Method::Get, ["collaborators"]) => {
            if permission(rd, me).is_none() {
                return Ok(Some(Reply::not_found()));
            }
            // Org members with a grant and outside collaborators, each with the permission `authorize` gives.
            let mut ids = collaborators(rd);
            if let Some(a) = &rd.dir.authz {
                ids.extend(a.members.keys().map(|u| u.0));
            }
            ids.sort();
            ids.dedup();
            let mut out = Vec::new();
            for a in ids {
                let Some(p) = permission(rd, a) else { continue };
                let mut u = simple_user(c, a, &login(c, rd, a).await);
                u["permissions"] = perms_json(Some(p));
                u["role_name"] = json!(perm_name(p));
                out.push(u);
            }
            Reply::json(200, Value::Array(out))
        }
        (Method::Get, ["collaborators", user]) => {
            let target = account_of(c, user).await.map_err(Error::RustError)?;
            match (permission(rd, me), target.and_then(|(a, _)| permission(rd, a))) {
                (Some(_), Some(_)) => Reply::empty(204),
                _ => Reply::not_found(),
            }
        }
        (Method::Get, ["collaborators", user, "permission"]) => {
            if permission(rd, me).is_none() {
                return Ok(Some(Reply::not_found()));
            }
            let Some((a, _)) = account_of(c, user).await.map_err(Error::RustError)? else {
                return Ok(Some(Reply::not_found()));
            };
            let p = permission(rd, a);
            let mut u = simple_user(c, a, user);
            u["permissions"] = perms_json(p);
            let legacy = match p {
                Some(RepoPermission::Admin) => "admin",
                Some(RepoPermission::Write | RepoPermission::Maintain) => "write",
                Some(_) => "read",
                None => "none",
            };
            Reply::json(200, json!({ "permission": legacy, "role_name": p.map_or("none", perm_name), "user": u }))
        }
        (Method::Put, ["collaborators", user]) => {
            let body: Value = serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null);
            let Some(p) = parse_perm(body["permission"].as_str()) else {
                return Ok(Some(Reply::error(422, "Validation Failed", doc)));
            };
            let Some((a, _)) = account_of(c, user).await.map_err(Error::RustError)? else {
                return Ok(Some(Reply::not_found()));
            };
            if collaborators(rd).contains(&a) {
                // GHES 3.22: "an existing collaborator is added as a collaborator" answers 204.
                return Ok(Some(Reply::empty(204)));
            }
            let r = commit_issue(c, rd, &who, IssueOp::InviteCollaborator { user: a, permission: p }, What::Deleted)
                .await?;
            // An org member's write is refused by its leader (`OrgMember`), rendered as 422 here.
            if r.status == 422 {
                Reply::json(
                    422,
                    json!({ "message": "Validation Failed", "documentation_url": format!("https://docs.github.com/enterprise-server@3.22/{doc}#add-a-repository-collaborator"),
                    "errors": [{ "resource": "Repository", "code": "custom", "message": "an organization member's permission is an org grant, not a collaborator invitation" }] }),
                )
            } else {
                r
            }
        }
        (Method::Delete, ["collaborators", user]) => {
            let Some((a, _)) = account_of(c, user).await.map_err(Error::RustError)? else {
                return Ok(Some(Reply::not_found()));
            };
            if !collaborators(rd).contains(&a) {
                // Not an outside collaborator: nothing on this chain to remove (an org member's grant is the org's).
                return Ok(Some(Reply::empty(204)));
            }
            commit_issue(c, rd, &who, IssueOp::RemoveCollaborator { user: a }, What::Deleted).await?
        }
        (Method::Get, ["invitations"]) => {
            if permission(rd, me) < Some(RepoPermission::Admin) {
                return Ok(Some(Reply::not_found()));
            }
            let rows = invitations(rd);
            let logins = names(c, rd, &rows).await;
            Reply::json(200, Value::Array(rows.iter().map(|(id, r)| invitation_json(c, rd, *id, r, &logins)).collect()))
        }
        (Method::Delete, ["invitations", id]) => {
            let Ok(id) = id.parse::<i64>() else { return Ok(Some(Reply::not_found())) };
            if !c.cfg.enabled.contains(&graftyard_git::issue_ops::call::REPO_FORMAT_INVITATIONS) {
                return Ok(Some(crate::api::unavailable(
                    "withdrawing an invitation needs record format 32.11, which FormatsEnable has not enabled",
                )));
            }
            commit_issue(c, rd, &who, IssueOp::CancelInvitation { id: InvitationId(DbId(id)) }, What::Deleted).await?
        }
        _ => return Ok(None),
    };
    Ok(Some(r))
}

async fn names(c: &Ctx<'_>, rd: &Read, rows: &[(InvitationId, InvitationRow)]) -> Vec<(AccountId, String)> {
    let mut out: Vec<(AccountId, String)> = Vec::new();
    for (_, r) in rows {
        let invitee = match r.invitee {
            Invitee::User(u) => Some(u.0),
            Invitee::Email(_) => None,
        };
        for a in invitee.into_iter().chain(r.inviter) {
            if !out.iter().any(|(x, _)| *x == a) {
                out.push((a, login(c, rd, a).await));
            }
        }
    }
    out
}

/// `/user/repository_invitations[/{id}]?repo=o/r`: the invitee's side of a repository invitation.
pub async fn user_invitation(c: &Ctx<'_>, req: &mut Request, id: Option<&str>) -> Result<Reply> {
    let who = crate::authenticate(c, req).await?;
    let (who, me) = match need_user(who.as_ref()) {
        Ok(x) => x,
        Err(r) => return Ok(r),
    };
    let url = req.url()?;
    let repo = url.query_pairs().find(|(k, _)| k == "repo").map(|(_, v)| v.into_owned());
    let Some((o, n)) = repo.as_deref().and_then(|r| r.split_once('/')) else {
        return Ok(if id.is_none() { Reply::json(200, json!([])) } else { Reply::not_found() });
    };
    let rd = match crate::api::read_for_invitee(c, o, n, &who).await? {
        Ok(r) => r,
        Err(r) => return Ok(r),
    };
    let mine: Vec<(InvitationId, InvitationRow)> =
        invitations(&rd).into_iter().filter(|(_, r)| matches!(r.invitee, Invitee::User(u) if u.0 == me)).collect();
    // A private repository the caller neither reads nor is invited to answers as an unknown one.
    if mine.is_empty() && !crate::api::readable(c, &rd, &who) {
        return Ok(Reply::not_found());
    }
    Ok(match (req.method(), id.and_then(|i| i.parse::<i64>().ok())) {
        (Method::Get, None) => {
            let logins = names(c, &rd, &mine).await;
            Reply::json(200, Value::Array(mine.iter().map(|(i, r)| invitation_json(c, &rd, *i, r, &logins)).collect()))
        }
        (m @ (Method::Patch | Method::Delete), Some(i)) => {
            let op = IssueOp::AnswerInvitation { id: InvitationId(DbId(i)), accept: m == Method::Patch };
            commit_issue(c, &rd, &who, op, What::Deleted).await?
        }
        _ => Reply::not_found(),
    })
}

// ---- the org's chain ----

fn slug(name: &str) -> String {
    let mut s = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch.to_ascii_lowercase());
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    s.trim_matches('-').to_owned()
}

fn team_by_slug<'a>(a: &'a AuthzFold, s: &str) -> Option<(TeamId, &'a graftyard_identity::TeamRow)> {
    a.org().teams.iter().find(|(_, t)| slug(&t.name) == s).map(|(id, t)| (*id, t))
}

/// GitHub's `team` (`graftyard_api::rest::team_json`), or `team-full` with the org.
fn team_json(c: &Ctx<'_>, org: &str, o: AccountId, id: TeamId, t: &graftyard_identity::TeamRow, full: bool) -> Value {
    let (api, web) = (format!("https://{}/api/v3", c.cfg.host), format!("https://{}", c.cfg.host));
    let mut v = graftyard_api::rest::team_json(&api, &web, org, o.0.0, id.0.0, &t.name, &slug(&t.name));
    if full {
        let org_v = json!({
            "login": org, "id": o.0.0, "node_id": graftyard_wire::NodeId::top(graftyard_wire::NodeType::Organization, o.0.0).encode(),
            "url": format!("{api}/orgs/{org}"), "repos_url": format!("{api}/orgs/{org}/repos"),
            "events_url": format!("{api}/orgs/{org}/events"), "hooks_url": format!("{api}/orgs/{org}/hooks"),
            "issues_url": format!("{api}/orgs/{org}/issues"), "members_url": format!("{api}/orgs/{org}/members{{/member}}"),
            "public_members_url": format!("{api}/orgs/{org}/public_members{{/member}}"),
            "avatar_url": format!("{web}/identicons/{org}.png"), "description": null, "html_url": format!("{web}/{org}"),
            "has_organization_projects": false, "has_repository_projects": false, "public_repos": 0, "public_gists": 0,
            "followers": 0, "following": 0, "type": "Organization", "created_at": "2026-11-01T00:00:00Z",
            "updated_at": "2026-11-01T00:00:00Z", "archived_at": null,
        });
        let m = v.as_object_mut().expect("object");
        m.remove("parent");
        m.insert("members_count".into(), json!(t.members.len()));
        m.insert("repos_count".into(), json!(0));
        m.insert("created_at".into(), json!("2026-11-01T00:00:00Z"));
        m.insert("updated_at".into(), json!("2026-11-01T00:00:00Z"));
        m.insert("organization".into(), org_v);
    }
    v
}

fn authz_refusal(done: &Value, doc: &str) -> Option<Reply> {
    Some(match refused(done)?.as_str() {
        "Forbidden" => Reply::error(403, "You must be an admin to do that.", doc),
        "Unauthorized" => Reply::error(401, "Bad credentials", "rest"),
        "NoInvitation" => Reply::not_found(),
        "AlreadyMember" | "NotAMember" | "NoTeam" => Reply::error(422, "Validation Failed", doc),
        e => crate::api::unavailable(&format!("the command did not commit: {e}")),
    })
}

/// `GET /repos/{o}/{r}/assignees` or `/teams`, rendered by `graftyard_api::rest`.
async fn people_list(c: &Ctx<'_>, req: &Request, rd: &Read, what: &str) -> Result<Reply> {
    let host = graftyard_wire::Host::new(&c.cfg.host, false).map_err(|e| Error::RustError(e.to_string()))?;
    let query = req.url()?.query().unwrap_or("").to_owned();
    if what == "teams" {
        let teams = rd.dir.authz.as_ref().map_or_else(Vec::new, |a| {
            let org = a.org();
            org.team_grants
                .iter()
                .filter(|((repo, _), _)| *repo == rd.repo.id)
                .filter_map(|((_, id), p)| {
                    let t = org.teams.get(id)?;
                    let (name, slug) = (t.name.to_string(), slug(&t.name));
                    Some(graftyard_api::rest::RepoTeam { id: id.0.0, name, slug, permission: *p })
                })
                .collect()
        });
        return Ok(graftyard_api::rest::list_teams(&rd.view(&host), teams, &query));
    }
    // Collaborators, org members, and the owner, each with the permission `authorize` gives.
    let mut ids = collaborators(rd);
    ids.push(rd.repo.owner);
    if let Some(a) = &rd.dir.authz {
        ids.extend(a.members.keys().map(|u| u.0));
    }
    ids.sort();
    ids.dedup();
    let mut dir = rd.dir.clone();
    let mut people = Vec::new();
    for a in ids {
        let p = permission(rd, a);
        if p >= Some(RepoPermission::Write) && !dir.logins.contains_key(&a.0.0) {
            dir.logins.insert(a.0.0, login(c, rd, a).await);
        }
        people.push((a, p));
    }
    let view = graftyard_api::view::Repo { host: &host, at: rd.at.clone(), core: &rd.core, meta: &rd.meta, dir: &dir };
    Ok(graftyard_api::rest::list_assignees(&view, &people, &query))
}

/// The caller's pending invitation to the org, by account.
fn pending_invitation(a: &AuthzFold, me: AccountId) -> Option<(InvitationId, OrgRole)> {
    let now = crate::now();
    a.org().invitations.iter().find_map(|(id, r)| match (&r.invitee, &r.grant) {
        (Invitee::User(u), InvitationGrant::Org { role, .. }) if u.0 == me && r.expires > now => Some((*id, *role)),
        _ => None,
    })
}

/// `/orgs/{org}/teams...` and `/user/memberships/orgs/{org}`.
pub async fn org_route(c: &Ctx<'_>, req: &mut Request, p: &str) -> Result<Reply> {
    let segs: Vec<&str> = p.trim_end_matches('/').split('/').skip(1).collect();
    let who = crate::authenticate(c, req).await?;
    let (who, me) = match need_user(who.as_ref()) {
        Ok(x) => x,
        Err(r) => return Ok(r),
    };
    let org = match segs.as_slice() {
        ["orgs", org, ..] | ["user", "memberships", "orgs", org] => *org,
        _ => return Ok(Reply::not_found()),
    };
    let Some((o, graftyard_identity::AccountKind::Org)) = account_of(c, org).await.map_err(Error::RustError)? else {
        return Ok(Reply::not_found());
    };
    let Some(a) = crate::ident::authz_of(c, o).await.map_err(Error::RustError)? else {
        return Ok(Reply::not_found());
    };
    let cred = who.credential_shard();
    let commit = |op: Value| commit_owner(c, ChainId::Authz(o), o, json!({ "cred": cred, "op": op }));
    let method = req.method();
    let doc = "rest/teams/teams";
    let me_login = || async { crate::ident::login(c, me).await.unwrap_or_default() };
    Ok(match (method.clone(), &segs[..]) {
        (Method::Get, ["orgs", _, "teams"]) => {
            if a.role(UserId(me)).is_none() {
                return Ok(Reply::not_found());
            }
            let teams = a.org().teams.iter().map(|(id, t)| team_json(c, org, o, *id, t, false)).collect();
            Reply::json(200, Value::Array(teams))
        }
        (Method::Post, ["orgs", _, "teams"]) => {
            let body: Value = req.json().await.unwrap_or(Value::Null);
            let Some(name) = body["name"].as_str().filter(|n| !slug(n).is_empty()) else {
                return Ok(Reply::error(422, "Validation Failed", doc));
            };
            if team_by_slug(&a, &slug(name)).is_some() {
                return Ok(Reply::error(422, "Validation Failed", doc));
            }
            let done = match commit(json!({ "CreateTeam": { "name": name } })).await {
                Ok(d) => d,
                Err(x) => return Ok(x),
            };
            if let Some(r) = authz_refusal(&done, doc) {
                return Ok(r);
            }
            let Some(id) = done.pointer("/ok/out/Team").and_then(Value::as_i64) else {
                return Ok(crate::api::unavailable(&format!("no team id: {done}")));
            };
            let row = graftyard_identity::TeamRow { name: name.into(), members: Default::default() };
            Reply::json(201, team_json(c, org, o, TeamId(DbId(id)), &row, true))
        }
        (Method::Get, ["orgs", _, "teams", s]) => match team_by_slug(&a, s) {
            Some((id, t)) if a.role(UserId(me)).is_some() => Reply::json(200, team_json(c, org, o, id, t, true)),
            _ => Reply::not_found(),
        },
        (Method::Put | Method::Delete, ["orgs", _, "teams", s, "memberships", user]) => {
            let Some((id, _)) = team_by_slug(&a, s) else { return Ok(Reply::not_found()) };
            let Some((u, _)) = account_of(c, user).await.map_err(Error::RustError)? else {
                return Ok(Reply::not_found());
            };
            let on = method == Method::Put;
            let done = match commit(json!({ "SetTeamMember": { "team": id, "user": u, "on": on } })).await {
                Ok(d) => d,
                Err(x) => return Ok(x),
            };
            if let Some(r) = authz_refusal(&done, doc) {
                return Ok(r);
            }
            if !on {
                return Ok(Reply::empty(204));
            }
            let url = format!("https://{}/api/v3/teams/{}/memberships/{user}", c.cfg.host, id.0.0);
            Reply::json(200, json!({ "url": url, "role": "member", "state": "active" }))
        }
        (Method::Put, ["orgs", _, "teams", s, "repos", ro, rn]) => {
            let Some((id, _)) = team_by_slug(&a, s) else { return Ok(Reply::not_found()) };
            let Some(r) = crate::resolve(c, ro, rn).await?.map_err(Error::RustError)? else {
                return Ok(Reply::not_found());
            };
            let body: Value = serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null);
            let Some(p) = parse_perm(body["permission"].as_str()) else {
                return Ok(Reply::error(422, "Validation Failed", doc));
            };
            let done = match commit(json!({ "SetTeamGrant": { "repo": r.id, "team": id, "permission": p } })).await {
                Ok(d) => d,
                Err(x) => return Ok(x),
            };
            authz_refusal(&done, doc).unwrap_or_else(|| Reply::empty(204))
        }
        (Method::Get, ["user", "memberships", "orgs", _]) => match (a.role(UserId(me)), pending_invitation(&a, me)) {
            (Some(r), _) => Reply::json(200, crate::ident::membership_json(c, org, o, &me_login().await, me, r)),
            (None, Some((_, r))) => {
                let mut v = crate::ident::membership_json(c, org, o, &me_login().await, me, r);
                v["state"] = json!("pending");
                Reply::json(200, v)
            }
            _ => Reply::not_found(),
        },
        (m @ (Method::Patch | Method::Delete), ["user", "memberships", "orgs", _]) => {
            let Some((id, role)) = pending_invitation(&a, me) else {
                return Ok(match a.role(UserId(me)) {
                    Some(r) if m == Method::Patch => {
                        Reply::json(200, crate::ident::membership_json(c, org, o, &me_login().await, me, r))
                    }
                    _ => Reply::not_found(),
                });
            };
            if m == Method::Patch {
                let body: Value = req.json().await.unwrap_or(Value::Null);
                if body["state"].as_str() != Some("active") {
                    return Ok(Reply::error(422, "Validation Failed", "rest/orgs/members"));
                }
            }
            let op =
                if m == Method::Patch { json!({ "Accept": { "id": id } }) } else { json!({ "Decline": { "id": id } }) };
            let done = match commit(op).await {
                Ok(d) => d,
                Err(x) => return Ok(x),
            };
            if let Some(r) = authz_refusal(&done, "rest/orgs/members") {
                return Ok(r);
            }
            if m == Method::Delete {
                return Ok(Reply::empty(204));
            }
            Reply::json(200, crate::ident::membership_json(c, org, o, &me_login().await, me, role))
        }
        _ => Reply::not_found(),
    })
}
