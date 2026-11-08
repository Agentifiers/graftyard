//! `POST /repos/{owner}/{repo}/transfer` (`design/SHARED.md`, "Multi-chain operations", Transfer; BF-30).
//!
//! The commit point is `ReserveTransfer` on the destination owner's `Authz` chain, under the caller's credential: the
//! leader reads the source owner's chain and refuses a caller who does not administer both. The edge then asks the
//! destination's `Authz` leader to finish the transfer in band (`/finish`, the site's `Audit` batch for that one
//! item): the group's move when the owners' placements differ, the transfer's own `Settings` (repository format
//! 32.10), the `Live` name, the old name's `Alias`, and `RepoOwner`. It answers 202 with the repository, as GHES does,
//! before those finish: GitHub's transfer is asynchronous too. An edge that dies after the commit point leaves the
//! rest to the `Audit` consumer's cursor on that chain (Authz format 0x12.4).
//!
//! Sent only once `FormatsEnable` (the test edge's `FORMATS_ENABLED`) enabled 32.10 and 18.4: until a site day put
//! every member on a release that reads them, no member could finish the transfer.

use graftyard_identity::RepoName;
use graftyard_raft::Format;
use graftyard_types::{AccountId, ChainId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;

const NEEDS: [Format; 2] = [graftyard_git::transfer::REPO_FORMAT_TRANSFER, graftyard_identity::AUTHZ_FORMAT_V4];

/// GHES 3.22's `minimal-repository` for the repository under its new owner: every required field.
fn minimal_repository(c: &Ctx<'_>, id: i64, owner: AccountId, login: &str, name: &str, org: bool) -> Value {
    let api = format!("https://{}/api/v3", c.cfg.host);
    let web = format!("https://{}", c.cfg.host);
    let r = format!("{api}/repos/{login}/{name}");
    let mut owner_json = crate::ident::user_json(c, owner, login, org);
    for k in [
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
    ] {
        owner_json.as_object_mut().expect("object").remove(k);
    }
    let mut v = json!({
        "id": id, "node_id": graftyard_wire::NodeId::top(graftyard_wire::NodeType::Repository, id).encode(),
        "name": name, "full_name": format!("{login}/{name}"), "owner": owner_json,
        "private": true, "html_url": format!("{web}/{login}/{name}"), "description": null, "fork": false,
        "url": r, "visibility": "private",
    });
    let urls = [
        ("archive_url", "{archive_format}{/ref}"),
        ("assignees_url", "assignees{/user}"),
        ("blobs_url", "git/blobs{/sha}"),
        ("branches_url", "branches{/branch}"),
        ("collaborators_url", "collaborators{/collaborator}"),
        ("comments_url", "comments{/number}"),
        ("commits_url", "commits{/sha}"),
        ("compare_url", "compare/{base}...{head}"),
        ("contents_url", "contents/{+path}"),
        ("contributors_url", "contributors"),
        ("deployments_url", "deployments"),
        ("downloads_url", "downloads"),
        ("events_url", "events"),
        ("forks_url", "forks"),
        ("git_commits_url", "git/commits{/sha}"),
        ("git_refs_url", "git/refs{/sha}"),
        ("git_tags_url", "git/tags{/sha}"),
        ("hooks_url", "hooks"),
        ("issue_comment_url", "issues/comments{/number}"),
        ("issue_events_url", "issues/events{/number}"),
        ("issues_url", "issues{/number}"),
        ("keys_url", "keys{/key_id}"),
        ("labels_url", "labels{/name}"),
        ("languages_url", "languages"),
        ("merges_url", "merges"),
        ("milestones_url", "milestones{/number}"),
        ("notifications_url", "notifications{?since,all,participating}"),
        ("pulls_url", "pulls{/number}"),
        ("releases_url", "releases{/id}"),
        ("stargazers_url", "stargazers"),
        ("statuses_url", "statuses/{sha}"),
        ("subscribers_url", "subscribers"),
        ("subscription_url", "subscription"),
        ("tags_url", "tags"),
        ("teams_url", "teams"),
        ("trees_url", "git/trees{/sha}"),
    ];
    let m = v.as_object_mut().expect("object");
    for (k, tail) in urls {
        m.insert(k.into(), json!(format!("{r}/{tail}")));
    }
    v
}

pub async fn transfer(c: &Ctx<'_>, req: &mut Request, owner: &str, name: &str) -> Result<Reply> {
    for f in NEEDS {
        if !c.cfg.enabled.contains(&f) {
            return Ok(crate::api::unavailable(&format!(
                "a transfer needs record format {}.{}, which FormatsEnable has not enabled on this install",
                f.family, f.version
            )));
        }
    }
    let Some(who) = crate::authenticate(c, req).await? else {
        return Ok(Reply::error(401, "Requires authentication", "rest/repos/repos#transfer-a-repository"));
    };
    let body: Value = req.json().await.unwrap_or(Value::Null);
    let Some(new_owner) = body["new_owner"].as_str() else {
        return Ok(Reply::error(422, "new_owner is required", "rest/repos/repos#transfer-a-repository"));
    };
    let repo = match crate::resolve(c, owner, name).await? {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(Reply::not_found()),
        Err(e) => return Ok(crate::api::unavailable(&e)),
    };
    let (to, kind) = match crate::ident::account_of(c, new_owner).await.map_err(Error::RustError)? {
        Some(x) => x,
        None => return Ok(Reply::error(422, "Invalid new_owner", "rest/repos/repos#transfer-a-repository")),
    };
    let new_name = body["new_name"].as_str().unwrap_or(name);
    let Ok(rn) = RepoName::parse(new_name) else {
        return Ok(Reply::error(422, "Invalid new_name", "rest/repos/repos#transfer-a-repository"));
    };
    let cmd = json!({ "cred": who.credential_shard(),
        "op": { "ReserveTransfer": { "name": rn.as_str(), "repo": repo.id, "from": repo.owner } } });
    let done = match crate::ident::commit_owner(c, ChainId::Authz(to), to, cmd).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    match crate::ident::refused(&done).as_deref() {
        None => {}
        Some("Forbidden") => {
            return Ok(Reply::error(
                403,
                "You must be an admin of both owners",
                "rest/repos/repos#transfer-a-repository",
            ));
        }
        Some("Taken") => {
            return Ok(Reply::error(
                422,
                &format!("{new_owner} already has a repository named {new_name}"),
                "rest/repos/repos#transfer-a-repository",
            ));
        }
        Some("Unauthorized") => return Ok(Reply::error(401, "Bad credentials", "rest")),
        Some(e) => return Ok(crate::api::unavailable(&format!("the transfer did not commit: {e}"))),
    }
    // Test installs: the edge dies right after the commit point, so only the `Audit` consumer can finish it.
    #[cfg(feature = "test-faults")]
    let killed = c.kill.as_deref() == Some("after_reserved");
    #[cfg(not(feature = "test-faults"))]
    let killed = false;
    if !killed {
        let owed =
            json!({ "owed": { "Transfer": { "repo": repo.id, "from": repo.owner, "to": to, "key": rn.key() } } });
        if let Err(e) = finish(c, to, &owed).await {
            console_error!("transfer {}: in-band finish not started: {e}", repo.id.0.0);
        }
    }
    let org = kind == graftyard_identity::AccountKind::Org;
    Ok(Reply::json(202, minimal_repository(c, repo.id.0.0, to, new_owner, new_name, org)))
}

/// Starts the in-band finish on the destination's `Authz` leader.
async fn finish(c: &Ctx<'_>, to: AccountId, owed: &Value) -> std::result::Result<(), String> {
    let record = crate::placement(c, to).await?.ok_or("no placement")?;
    let b = c.bindings();
    let (v, _, _) = graftyard_lookup::call_opt(&b, ChainId::Authz(to), &record, "/finish", owed.to_string().as_bytes())
        .await?
        .ok_or("no Authz group")?;
    match v.get("error") {
        Some(e) => Err(e.to_string()),
        None => Ok(()),
    }
}
