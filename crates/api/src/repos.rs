//! `rest:repos`' create, read, update, and delete as GHES 3.22 shapes them (`design/SHARED.md`, the `rest:repos`
//! row; branch `m3-repos`): `full-repository` from one repository's tiers, the request bodies of `POST
//! /orgs/{org}/repos`, `POST /user/repos`, and `PATCH /repos/{o}/{r}` turned into `RepoSettings`, and gh's GraphQL
//! `createRepository` payload.
//!
//! A repository's name lives on its owner's `Authz` chain (`CreateRepo`, `RenameRepo`, `DeleteRepo`) and its
//! settings on its own chain (`IssueOp::InitRepo`, `IssueOp::Settings`, `IssueOp::DeleteRepo`, format 32.13). The
//! edge sequences them; everything here is pure.
//!
//! Fields the install holds no fact for render as GitHub renders a new repository: counts 0, `license` and
//! `language` null, `has_pages` and `has_discussions` false, `disabled` false. `security_and_analysis` is left out,
//! because the Terraform provider then calls the vulnerability-alert routes, which no fact here can answer.
//! `pushed_at` and `updated_at` are the creation time [inference: the tiers keep no time of the last push].

use graftyard_git::issues::{MergeMethod, RepoSettings, Visibility};
use graftyard_git::refs::RefName;
use graftyard_identity::{RepoName, RepoPermission, Url};
use graftyard_raft::Acked;
use graftyard_types::{AccountId, Timestamp};
use serde_json::{Map, Value, json};
use smallvec::SmallVec;

use crate::exec::{Args, Obj, Out, Request, leaf, obj};
use crate::view::Repo;
use crate::wire::{FieldError, NodeId, NodeType, Reply, iso, simple_user, validation_error};

pub const DOC_CREATE: &str = "rest/repos/repos#create-an-organization-repository";
pub const DOC_CREATE_USER: &str = "rest/repos/repos#create-a-repository-for-the-authenticated-user";
pub const DOC_UPDATE: &str = "rest/repos/repos#update-a-repository";
pub const DOC_DELETE: &str = "rest/repos/repos#delete-a-repository";

/// The repository's URL templates under `/repos/{o}/{r}`, as GitHub lists them.
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
    ("hooks_url", "/hooks"),
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
];

/// What a repository with no `Settings` row reads as (`graftyard_git::transfer`'s defaults).
pub fn defaults(owner: AccountId, enterprise: Option<AccountId>) -> RepoSettings {
    RepoSettings {
        owner,
        enterprise,
        visibility: Visibility::Private,
        default_branch: RefName::parse(b"refs/heads/main").expect("a valid ref"),
        archived: false,
        description: None,
        homepage: None,
        topics: SmallVec::new(),
        has_issues: true,
        has_wiki: false,
        has_projects: false,
        merge_methods: smallvec::smallvec![MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase],
        delete_branch_on_merge: false,
        allow_auto_merge: false,
        allow_update_branch: false,
        check_retention_days: graftyard_git::issues::CHECK_RETENTION_DEFAULT,
    }
}

fn branch(r: &RefName) -> String {
    let s = r.display();
    s.strip_prefix("refs/heads/").map(str::to_owned).unwrap_or(s)
}

fn vis_name(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Private => "private",
        Visibility::Internal => "internal",
    }
}

/// GHES's `full-repository` for the repository `r` reads, with the viewer's `permissions` when it has one.
pub fn full_repository(r: &Repo<'_>) -> Value {
    let s = r.settings().unwrap_or_else(|| defaults(r.at.owner_id, None));
    let (o, n) = (&r.at.owner, &r.at.name);
    let (api, web) = (r.host.api(), r.host.web());
    let url = format!("{api}/repos/{o}/{n}");
    let at = graftyard_git::tiers::created(r.core).unwrap_or(Timestamp(0));
    let id = r.at.id.0.0;
    let issues = r.issue_count();
    let mut v = json!({
        "id": id, "node_id": NodeId::top(NodeType::Repository, id).encode(),
        "name": n, "full_name": format!("{o}/{n}"), "owner": simple_user(r.host, r.dir, Some(r.at.owner_id)),
        "private": s.visibility != Visibility::Public, "visibility": vis_name(s.visibility),
        "html_url": format!("{web}/{o}/{n}"), "description": s.description.as_deref(), "fork": false, "url": url,
        "clone_url": format!("{web}/{o}/{n}.git"), "git_url": format!("git://{}/{o}/{n}.git", r.host.name()),
        "ssh_url": format!("git@{}:{o}/{n}.git", r.host.name()), "svn_url": format!("{web}/{o}/{n}"),
        "mirror_url": null, "homepage": s.homepage.as_ref().map(|u| &*u.0),
        "language": null, "license": null, "size": 0, "default_branch": branch(&s.default_branch),
    });
    let rest = json!({
        "forks": 0, "forks_count": 0, "network_count": 0, "open_issues": issues, "open_issues_count": issues,
        "stargazers_count": 0, "watchers": 0, "watchers_count": 0, "subscribers_count": 0,
        "has_issues": s.has_issues, "has_projects": s.has_projects, "has_wiki": s.has_wiki, "has_pages": false,
        "has_downloads": false, "has_discussions": false, "archived": s.archived, "disabled": false,
        "is_template": false, "topics": s.topics.iter().map(|t| &**t).collect::<Vec<_>>(),
        "allow_merge_commit": s.merge_methods.contains(&MergeMethod::Merge),
        "allow_squash_merge": s.merge_methods.contains(&MergeMethod::Squash),
        "allow_rebase_merge": s.merge_methods.contains(&MergeMethod::Rebase),
        "allow_auto_merge": s.allow_auto_merge, "allow_update_branch": s.allow_update_branch,
        "delete_branch_on_merge": s.delete_branch_on_merge, "allow_forking": false,
        "web_commit_signoff_required": false, "use_squash_pr_title_as_default": false,
        "squash_merge_commit_title": "COMMIT_OR_PR_TITLE", "squash_merge_commit_message": "COMMIT_MESSAGES",
        "merge_commit_title": "MERGE_MESSAGE", "merge_commit_message": "PR_TITLE",
        "created_at": iso(at), "updated_at": iso(at), "pushed_at": iso(at), "template_repository": null,
    });
    let m = v.as_object_mut().expect("object");
    m.extend(rest.as_object().expect("object").clone());
    for (k, tail) in URLS {
        m.insert(k.into(), json!(format!("{url}{tail}")));
    }
    if r.dir.is_org(r.at.owner_id) {
        m.insert("organization".into(), simple_user(r.host, r.dir, Some(r.at.owner_id)));
    }
    if let Some(p) = r.dir.viewer().and_then(|u| r.dir.permission(u)) {
        let at = |x| p >= x;
        m.insert(
            "permissions".into(),
            json!({ "admin": at(RepoPermission::Admin), "maintain": at(RepoPermission::Maintain),
                "push": at(RepoPermission::Write), "triage": at(RepoPermission::Triage), "pull": true }),
        );
    }
    v
}

/// `GET /repos/{o}/{r}`: 200 with an ETag that changes with the tiers' index.
pub fn get(r: &Repo<'_>, index: graftyard_types::Index) -> Reply {
    let tag = graftyard_wire::etag(&format!("repos/{}", r.at.id.0.0), index);
    Reply::json(200, full_repository(r)).with("etag", tag)
}

/// A committed create's reply, 201 with `Location` as GHES documents it, or an update's 200: only from `&Acked`.
pub fn saved<T>(_acked: &Acked<T>, r: &Repo<'_>, created: bool) -> Reply {
    if !created {
        return Reply::json(200, full_repository(r));
    }
    let loc = format!("{}/repos/{}/{}", r.host.api(), r.at.owner, r.at.name);
    Reply::json(201, full_repository(r)).with("location", loc)
}

fn invalid(doc: &str, field: &'static str, code: &'static str) -> Reply {
    let e = FieldError { resource: "Repository", field, code };
    Reply::json(422, validation_error("Repository creation failed.", doc, &[e]))
}

/// GitHub's 422 for a name the owner already holds (`research/wire-census.md` #20): `already_exists` on `name`.
pub fn taken(doc: &str) -> Reply {
    Reply::json(
        422,
        json!({ "message": "Repository creation failed.",
            "errors": [{ "resource": "Repository", "code": "custom", "field": "name",
                "message": "name already exists on this account" }],
            "documentation_url": format!("https://docs.github.com/enterprise-server@3.22/{doc}"), "status": "422" }),
    )
}

/// A create body's name, checked as `RepoName` does.
pub fn name_of(body: &Value, doc: &str) -> Result<RepoName, Reply> {
    match body.get("name").and_then(Value::as_str) {
        None => Err(invalid(doc, "name", "missing_field")),
        Some(n) => RepoName::parse(n).map_err(|_| invalid(doc, "name", "invalid")),
    }
}

fn flag(body: &Value, k: &str) -> Option<bool> {
    body.get(k).and_then(Value::as_bool)
}

/// The settings a create or update body asks for, over `cur`. `visibility` wins over the deprecated `private`; an
/// org repository may be `internal`, a user's may not. Unknown fields are ignored, as GitHub ignores them.
pub fn apply_body(cur: &RepoSettings, body: &Value, org: bool, doc: &str) -> Result<RepoSettings, Reply> {
    let mut s = cur.clone();
    let vis = match body.get("visibility").and_then(Value::as_str) {
        Some("public") => Some(Visibility::Public),
        Some("private") => Some(Visibility::Private),
        Some("internal") if org => Some(Visibility::Internal),
        Some(_) => return Err(invalid(doc, "visibility", "invalid")),
        None => flag(body, "private").map(|p| if p { Visibility::Private } else { Visibility::Public }),
    };
    if let Some(v) = vis {
        s.visibility = v;
    }
    if let Some(d) = body.get("description") {
        s.description = d.as_str().filter(|d| !d.is_empty()).map(Into::into);
    }
    if let Some(h) = body.get("homepage") {
        s.homepage = h.as_str().filter(|h| !h.is_empty()).map(|h| Url(h.into()));
    }
    for (k, f) in
        [("has_issues", &mut s.has_issues), ("has_wiki", &mut s.has_wiki), ("has_projects", &mut s.has_projects)]
    {
        if let Some(b) = flag(body, k) {
            *f = b;
        }
    }
    for (k, f) in [
        ("archived", &mut s.archived),
        ("delete_branch_on_merge", &mut s.delete_branch_on_merge),
        ("allow_auto_merge", &mut s.allow_auto_merge),
        ("allow_update_branch", &mut s.allow_update_branch),
    ] {
        if let Some(b) = flag(body, k) {
            *f = b;
        }
    }
    for (k, m) in [
        ("allow_merge_commit", MergeMethod::Merge),
        ("allow_squash_merge", MergeMethod::Squash),
        ("allow_rebase_merge", MergeMethod::Rebase),
    ] {
        match flag(body, k) {
            Some(true) if !s.merge_methods.contains(&m) => s.merge_methods.push(m),
            Some(false) => s.merge_methods.retain(|x| *x != m),
            _ => {}
        }
    }
    s.merge_methods.sort_by_key(|m| *m as u8);
    if s.merge_methods.is_empty() {
        let e = FieldError { resource: "Repository", field: "merge_commit_allowed", code: "invalid" };
        return Err(Reply::json(422, validation_error("Validation Failed", doc, &[e])));
    }
    if let Some(b) = body.get("default_branch").and_then(Value::as_str) {
        s.default_branch = RefName::parse(format!("refs/heads/{b}").as_bytes())
            .map_err(|_| invalid(doc, "default_branch", "invalid"))?;
    }
    Ok(s)
}

/// `createRepository(input:)`'s settings and name, from gh's `CreateRepositoryInput`: `name`, `visibility`
/// (`PUBLIC`, `PRIVATE`, `INTERNAL`), `description`, `homepageUrl`, `hasIssuesEnabled`, `hasWikiEnabled`.
pub fn graphql_input(input: &Value) -> Value {
    let mut b = Map::new();
    let mut put = |k: &str, v: Option<&Value>| {
        if let Some(v) = v.filter(|v| !v.is_null()) {
            b.insert(k.into(), v.clone());
        }
    };
    put("name", input.get("name"));
    put(
        "visibility",
        input.get("visibility").and_then(Value::as_str).map(|v| v.to_ascii_lowercase()).map(Value::from).as_ref(),
    );
    put("description", input.get("description"));
    put("homepage", input.get("homepageUrl"));
    put("has_issues", input.get("hasIssuesEnabled"));
    put("has_wiki", input.get("hasWikiEnabled"));
    Value::Object(b)
}

/// The `createRepository` payload over the new repository's tiers, once its `InitRepo` is acknowledged.
pub fn create_payload<T>(_acked: &Acked<T>, req: &Request<'_>, r: &Repo<'_>) -> Value {
    struct Root<'a>(&'a Repo<'a>);
    struct Payload<'a>(&'a Repo<'a>);
    struct Created<'a>(&'a Repo<'a>);
    struct Owner<'a>(&'a Repo<'a>);
    impl Obj for Root<'_> {
        fn type_name(&self) -> &'static str {
            "Mutation"
        }
        fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
            (f == "createRepository").then(|| obj(Payload(self.0)))
        }
    }
    impl Obj for Payload<'_> {
        fn type_name(&self) -> &'static str {
            "CreateRepositoryPayload"
        }
        fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
            match f {
                "clientMutationId" => Some(leaf(Value::Null)),
                "repository" => Some(obj(Created(self.0))),
                _ => None,
            }
        }
    }
    impl Obj for Owner<'_> {
        fn type_name(&self) -> &'static str {
            if self.0.dir.is_org(self.0.at.owner_id) { "Organization" } else { "User" }
        }
        fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
            match f {
                "login" => Some(leaf(self.0.at.owner.clone())),
                "id" => Some(leaf(crate::wire::account_node(self.0.dir, self.0.at.owner_id))),
                _ => None,
            }
        }
    }
    impl Obj for Created<'_> {
        fn type_name(&self) -> &'static str {
            "Repository"
        }
        fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
            let r = self.0;
            let s = r.settings().unwrap_or_else(|| defaults(r.at.owner_id, None));
            Some(match f {
                "id" => leaf(NodeId::top(NodeType::Repository, r.at.id.0.0).encode()),
                "databaseId" => leaf(r.at.id.0.0),
                "name" => leaf(r.at.name.clone()),
                "nameWithOwner" => leaf(format!("{}/{}", r.at.owner, r.at.name)),
                "owner" => obj(Owner(r)),
                "url" => leaf(format!("{}/{}/{}", r.host.web(), r.at.owner, r.at.name)),
                "isPrivate" => leaf(s.visibility != Visibility::Public),
                "visibility" => leaf(vis_name(s.visibility).to_ascii_uppercase()),
                "description" => leaf(s.description.as_deref().map(str::to_owned)),
                _ => return None,
            })
        }
    }
    req.run(&Root(r))
}

/// A refused `InitRepo`, `Settings`, or `DeleteRepo`: GHES's 403 for a caller who is not an admin, 404 for a
/// repository that is gone, and 422 for settings it refuses.
pub fn refused(e: &graftyard_git::issue_ops::IssueErr) -> Reply {
    use graftyard_git::issue_ops::IssueErr as E;
    match e {
        E::Unauthorized(_) => Reply::error(401, "Bad credentials", "rest"),
        E::Forbidden => Reply::error(403, "Must have admin rights to Repository.", DOC_UPDATE),
        E::Agent(a) => Reply::error(403, &a.text, DOC_UPDATE),
        E::NotFound => Reply::not_found(),
        E::NeedIdBlock | E::Unavailable => Reply::error(503, "Service Unavailable", "rest"),
        _ => Reply::json(422, validation_error("Validation Failed", DOC_UPDATE, &[])),
    }
}
