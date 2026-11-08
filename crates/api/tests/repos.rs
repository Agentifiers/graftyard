//! `rest:repos`' create, read, update, and delete renderers (`graftyard_api::repos`) on a repository chain's tiers,
//! after `InitRepo` committed. Each reply is written to `$GRAFTYARD_REPLIES` when it is set, for `gh/validate.py`
//! against the pinned GHES 3.22 description; gh's `createRepository` payload too, against the SDL.

mod common;

use common::*;
use graftyard_api::exec::Request;
use graftyard_api::repos::{apply_body, create_payload, created, defaults, full_repository, get, name_of, taken};
use graftyard_api::wire::Host;
use graftyard_git::issue_ops::IssueOp;
use graftyard_git::issues::{MergeMethod, Visibility};
use graftyard_types::{AccountId, DbId, Index};
use serde_json::{Value, json};

fn host() -> Host {
    Host::new("ghes.graftyard.test", false).unwrap()
}

fn keep(case: &str, method: &str, path: &str, r: &graftyard_api::wire::Reply) {
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        let body = match &r.body {
            graftyard_wire::Body::Json(v) => v.clone(),
            _ => Value::Null,
        };
        let v = json!({ "method": method, "path": path, "status": r.status, "body": body, "location": r.header("location") });
        std::fs::write(format!("{dir}/{case}.rest.json"), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }
}

/// A repository whose `InitRepo` committed with a body's settings.
fn created_chain(body: &Value) -> Chain {
    let mut c = Chain::new(3);
    let s = apply_body(&defaults(AccountId(DbId(OWNER)), None), body, false, "x").unwrap();
    c.commit(OWNER, IssueOp::InitRepo(s)).unwrap();
    c
}

#[test]
fn a_created_repository_renders_ghes_full_repository_with_its_settings() {
    let body = json!({ "name": "hello", "description": "a test", "homepage": "https://example.test", "private": false,
        "has_wiki": true, "allow_rebase_merge": false, "auto_init": true });
    let c = created_chain(&body);
    let (h, dir) = (host(), Dir::new(OWNER));
    let r = c.repo(0, &h, &dir);
    let v = full_repository(&r);
    assert_eq!(v["visibility"], "public");
    assert_eq!(v["private"], false);
    assert_eq!(v["description"], "a test");
    assert_eq!(v["homepage"], "https://example.test");
    assert_eq!((v["has_wiki"].as_bool(), v["has_issues"].as_bool()), (Some(true), Some(true)));
    assert_eq!((v["allow_merge_commit"].as_bool(), v["allow_rebase_merge"].as_bool()), (Some(true), Some(false)));
    assert_eq!(v["default_branch"], "main");
    assert_eq!(v["created_at"], graftyard_wire::iso(c.now));
    assert_eq!(v["permissions"]["admin"], true);
    assert!(v.get("security_and_analysis").is_none(), "the provider would then read vulnerability alerts");
    let made = created(&r);
    assert_eq!(made.status, 201);
    assert_eq!(made.header("location"), Some("https://ghes.graftyard.test/api/v3/repos/o/r"));
    keep("repos-create-user", "POST", "/user/repos", &made);
    keep("repos-create-org", "POST", "/orgs/{org}/repos", &made);
    let g = get(&r, Index(9));
    assert_eq!(g.status, 200);
    assert!(g.header("etag").is_some());
    keep("repos-get", "GET", "/repos/{owner}/{repo}", &g);
    keep("repos-patch", "PATCH", "/repos/{owner}/{repo}", &graftyard_api::wire::Reply::json(200, v));
    keep("repos-delete", "DELETE", "/repos/{owner}/{repo}", &graftyard_api::wire::Reply::empty(204));
    // A reader sees pull only.
    let reader = Dir::new(READER);
    let v = full_repository(&c.repo(0, &h, &reader));
    assert_eq!((v["permissions"]["admin"].as_bool(), v["permissions"]["pull"].as_bool()), (Some(false), Some(true)));
}

#[test]
fn create_and_update_bodies_map_to_settings_and_refuse_what_ghes_refuses() {
    let d = defaults(AccountId(DbId(OWNER)), None);
    // `visibility` wins over `private`, and only an org's repository may be internal.
    let s = apply_body(&d, &json!({ "private": true, "visibility": "public" }), false, "x").unwrap();
    assert_eq!(s.visibility, Visibility::Public);
    assert_eq!(
        apply_body(&d, &json!({ "visibility": "internal" }), true, "x").unwrap().visibility,
        Visibility::Internal
    );
    let r = apply_body(&d, &json!({ "visibility": "internal" }), false, "x").unwrap_err();
    assert_eq!(r.status, 422);
    keep("repos-create-bad-visibility", "POST", "/user/repos", &r);
    // Every merge method off is refused.
    let off = json!({ "allow_merge_commit": false, "allow_squash_merge": false, "allow_rebase_merge": false });
    let r = apply_body(&d, &off, false, "x").unwrap_err();
    keep("repos-patch-no-merge", "PATCH", "/repos/{owner}/{repo}", &r);
    // Turning one back on keeps a stable order.
    let s = apply_body(&d, &json!({ "allow_merge_commit": false }), false, "x").unwrap();
    let s = apply_body(&s, &json!({ "allow_merge_commit": true }), false, "x").unwrap();
    assert_eq!(s.merge_methods.as_slice(), [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase]);
    // An empty description clears it; an absent one keeps it.
    let s = apply_body(&d, &json!({ "description": "x" }), false, "x").unwrap();
    assert_eq!(apply_body(&s, &json!({}), false, "x").unwrap().description.as_deref(), Some("x"));
    assert_eq!(apply_body(&s, &json!({ "description": "" }), false, "x").unwrap().description, None);
    // A name is required and must be a repository name.
    let r = name_of(&json!({}), "x").unwrap_err();
    keep("repos-create-no-name", "POST", "/orgs/{org}/repos", &r);
    assert!(name_of(&json!({ "name": "a b" }), "x").is_err());
    assert_eq!(name_of(&json!({ "name": "ok.git" }), "x").unwrap().as_str(), "ok");
    let t = taken("x");
    assert_eq!(t.status, 422);
    keep("repos-create-taken", "POST", "/orgs/{org}/repos", &t);
}

/// gh's `RepositoryCreate` document (`pkg/cmd/repo/create/http.go`, gh 2.102.1), answered over the new tiers.
#[test]
fn gh_create_repository_gets_its_payload() {
    let q = "mutation RepositoryCreate($input: CreateRepositoryInput!) {\n\t\tcreateRepository(input: $input) {\n\t\t\trepository {\n\t\t\t\tid\n\t\t\t\tname\n\t\t\t\towner { login }\n\t\t\t\turl\n\t\t\t}\n\t\t}\n\t}";
    let vars = json!({ "input": { "name": "r", "visibility": "PRIVATE", "ownerId": "U_x", "hasIssuesEnabled": true, "hasWikiEnabled": true } });
    let body = graftyard_api::repos::graphql_input(&vars["input"]);
    assert_eq!(body, json!({ "name": "r", "visibility": "private", "has_issues": true, "has_wiki": true }));
    let c = created_chain(&body);
    let (h, dir) = (host(), Dir::new(OWNER));
    let req = Request::parse(q, vars.as_object().cloned(), None).unwrap();
    let out = create_payload(&req, &c.repo(0, &h, &dir));
    let repo = &out["data"]["createRepository"]["repository"];
    assert_eq!(repo["name"], "r");
    assert_eq!(repo["owner"]["login"], "o");
    assert_eq!(repo["url"], "https://ghes.graftyard.test/o/r");
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        let v = json!({ "query": q, "variables": vars, "reply": out, "document": "RepositoryCreate" });
        std::fs::write(format!("{dir}/RepositoryCreate.json"), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }
}
