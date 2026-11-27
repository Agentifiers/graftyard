//! `POST` and `GET /repos/{owner}/{repo}/forks` (`design/SHARED.md`, the `rest:repos` row, "forks in their network";
//! "Multi-chain operations", Fork; `build/BUILD-PLAN.md`, AM3-WP2a).
//!
//! A fork's commit point is `CreateFork` on the fork owner's `Authz` chain, under the caller's credential (Authz
//! 0x12.16): the leader refuses a caller who does not administer that owner and a taken name, and records the parent
//! and the network with the name's `Live` (`AuthzEvent::ForkCreate`). The parent was read here through the read gate,
//! so a caller who cannot read it is answered 404, and its network is the one this request fixed (`Repo::network`).
//! The edge then asks the fork owner's `Authz` leader to finish the fork in band (`/finish`, the site's `Audit` batch
//! for that one item, as a transfer does): `RepoOwner` with the network (Names 0x10.4), `ForkAdded` on the network
//! chain (0x21.1), and the fork chain's first touch (repository 32.34), which writes `Genesis`, the first settings
//! and `Created`, and the parent's branches and tags with no pack, since the packs are the network's. It answers 202
//! with the fork, as GHES does, before those finish: GitHub creates a fork asynchronously too. An edge that dies after
//! the commit point leaves the rest to the `Audit` consumer's cursor on that chain.
//!
//! Sent only once `FormatsEnable` enabled every format the finish writes.

use graftyard_api::repos::{DOC_CREATE, defaults, taken};
use graftyard_identity::{AccountKind, Actor, RepoName};
use graftyard_raft::Format;
use graftyard_types::{AccountId, ChainId, NetworkId, RepoId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::api::Read;

const DOC_FORK: &str = "rest/repos/forks#create-a-fork";

/// Every format a fork's commit point and its finish write.
const NEEDS: [Format; 4] = [
    graftyard_identity::AUTHZ_FORMAT_V16,
    graftyard_identity::NAMES_FORMAT_V4,
    graftyard_git::network::NETWORK_FORMAT,
    graftyard_git::issue_ops::call::REPO_FORMAT_FORKS,
];

fn enabled(c: &Ctx<'_>) -> std::result::Result<(), Reply> {
    match NEEDS.iter().find(|f| !c.cfg.enabled.contains(f)) {
        None => Ok(()),
        Some(f) => Err(crate::api::unavailable(&format!(
            "a fork needs record format {}.{}, which FormatsEnable has not enabled on this install",
            f.family, f.version
        ))),
    }
}

/// `POST` or `GET .../forks` on a repository the read gate let the caller read; `None` for any other path.
pub async fn repo_route(
    c: &Ctx<'_>,
    req: &mut Request,
    rd: &Read,
    who: Option<&graftyard_identity::Principal>,
    tail: &[&str],
) -> Result<Option<Reply>> {
    if tail != ["forks"] {
        return Ok(None);
    }
    if let Err(r) = enabled(c) {
        return Ok(Some(r));
    }
    Ok(Some(match req.method() {
        Method::Post => {
            let body: Value = req.json().await.unwrap_or(Value::Null);
            create(c, rd, who, &body).await?
        }
        Method::Get => list(c, rd).await?,
        _ => Reply::not_found(),
    }))
}

/// The fork's owner and name from the body: `organization` or the caller, and `name` or the parent's.
async fn target(
    c: &Ctx<'_>,
    rd: &Read,
    me: AccountId,
    body: &Value,
) -> Result<std::result::Result<(AccountId, String, RepoName, bool), Reply>> {
    let (owner, login, org) = match body.get("organization").and_then(Value::as_str) {
        Some(o) => match crate::ident::account_of(c, o).await.map_err(Error::RustError)? {
            Some((a, AccountKind::Org)) => (a, o.to_owned(), true),
            _ => return Ok(Err(Reply::error(422, "Invalid organization", DOC_FORK))),
        },
        None => (me, crate::ident::login(c, me).await.unwrap_or_default(), false),
    };
    let name = body.get("name").and_then(Value::as_str).unwrap_or(&rd.at.name);
    let Ok(name) = RepoName::parse(name) else {
        let e = graftyard_wire::FieldError { resource: "Repository", field: "name", code: "invalid" };
        return Ok(Err(Reply::json(422, graftyard_wire::validation_error("Validation Failed", DOC_FORK, &[e]))));
    };
    Ok(Ok((owner, login, name, org)))
}

async fn create(c: &Ctx<'_>, rd: &Read, who: Option<&graftyard_identity::Principal>, body: &Value) -> Result<Reply> {
    let Some(who) = who else { return Ok(Reply::error(401, "Requires authentication", DOC_FORK)) };
    let Actor::User(me) = who.actor else {
        return Ok(Reply::error(403, "Resource not accessible by integration", DOC_FORK));
    };
    let (owner, login, name, org) = match target(c, rd, me.0, body).await? {
        Ok(t) => t,
        Err(r) => return Ok(r),
    };
    // A fork keeps the parent's settings but its own owner and no enterprise of the parent's; the finisher sets the
    // owner and enterprise its owner's `Authz` names (`lifecycle::init_settings`).
    let mut settings: graftyard_git::issues::RepoSettings =
        graftyard_git::tiers::get_rec(&rd.core, "settings", &[]).unwrap_or_else(|| defaults(owner, None));
    settings.owner = owner;
    settings.enterprise = None;
    let network = rd.repo.network();
    let init = serde_json::to_string(&settings)?;
    let cmd = json!({ "cred": who.credential_shard(), "op": create_op(&name, &init, rd.repo.id, network) });
    let done = match crate::ident::commit_owner(c, ChainId::Authz(owner), owner, cmd).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    match crate::ident::refused(&done).as_deref() {
        None => {}
        Some("Taken") => return Ok(taken(DOC_CREATE)),
        Some("Forbidden") => {
            return Ok(Reply::error(
                403,
                "You do not have permission to create repositories on this account",
                DOC_FORK,
            ));
        }
        Some("Unauthorized") => return Ok(Reply::error(401, "Bad credentials", "rest")),
        Some(e) => return Ok(crate::api::unavailable(&format!("the fork did not commit: {e}"))),
    }
    let Some(id) = done.pointer("/ok/out/Repo/repo").and_then(Value::as_i64) else {
        return Ok(crate::api::unavailable(&format!("CreateFork answered {done}")));
    };
    let repo = RepoId(graftyard_types::DbId(id));
    // Test installs: the edge dies right after the commit point, so only the `Audit` consumer can finish it.
    #[cfg(feature = "test-faults")]
    let killed = c.kill.as_deref() == Some("after_fork");
    #[cfg(not(feature = "test-faults"))]
    let killed = false;
    if !killed {
        let owed = owed(owner, &name, repo, &init, rd.repo.id, network, crate::now());
        if let Err(e) = crate::transfer::finish(c, owner, &owed).await {
            console_error!("fork {id}: in-band finish not started: {e}");
        }
    }
    Ok(Reply::json(202, fork_json(c, rd, id, owner, &login, name.as_str(), org)))
}

/// `CreateFork`'s op as the edge sends it to the owner's `Authz` leader.
fn create_op(name: &RepoName, init: &str, parent: RepoId, network: NetworkId) -> Value {
    json!({ "CreateFork": { "name": name.as_str(), "init": init, "parent": parent, "network": network } })
}

/// The in-band finish's item (`graftyard_identity::audit::Owed::Fork`), as the commit point's `ForkCreate` names it.
fn owed(
    owner: AccountId,
    name: &RepoName,
    repo: RepoId,
    init: &str,
    parent: RepoId,
    network: NetworkId,
    at: graftyard_types::Timestamp,
) -> Value {
    json!({ "owed": { "Fork": { "owner": owner, "key": name.key(), "name": name.as_str(), "repo": repo,
        "init": init, "parent": parent, "network": network, "at": at } } })
}

/// The fork as GHES's 202 renders it: `fork: true`, `parent` the repository forked and `source` its network's root.
/// `gh repo fork` reads `full_name`, `clone_url` and `parent`.
fn fork_json(c: &Ctx<'_>, rd: &Read, id: i64, owner: AccountId, login: &str, name: &str, org: bool) -> Value {
    let mut v = crate::transfer::minimal_repository(c, id, owner, login, name, org);
    let parent = parent_json(c, rd);
    let m = v.as_object_mut().expect("object");
    m.insert("fork".into(), json!(true));
    m.insert("source".into(), source_json(rd, &parent));
    m.insert("parent".into(), parent);
    v
}

fn parent_json(c: &Ctx<'_>, rd: &Read) -> Value {
    let org = rd.dir.authz.as_ref().and_then(graftyard_identity::AuthzRead::opened) == Some(AccountKind::Org);
    let mut p = crate::transfer::minimal_repository(c, rd.repo.id.0.0, rd.repo.owner, &rd.at.owner, &rd.at.name, org);
    p["fork"] = json!(rd.repo.network().root() != rd.repo.id);
    p
}

/// The network's root: the parent itself, or, for a fork of a fork, the root by id (its name needs the root's own
/// read, which head 4's fork fields add).
fn source_json(rd: &Read, parent: &Value) -> Value {
    let root = rd.repo.network().root();
    if root == rd.repo.id {
        return parent.clone();
    }
    json!({ "id": root.0.0, "fork": false })
}

/// `GET .../forks`: the repository's direct forks from the network chain, each as `minimal-repository` with
/// `fork: true`, in id order (GHES's default sort is `newest`; ids grow with creation [inference]).
async fn list(c: &Ctx<'_>, rd: &Read) -> Result<Reply> {
    let network = rd.repo.network();
    let forks = match network_forks(c, network, rd.repo.id).await {
        Ok(f) => f,
        Err(e) => return Ok(crate::api::unavailable(&e)),
    };
    let mut out = Vec::new();
    for f in forks.into_iter().rev() {
        let Ok(r) = crate::repo_by_id(c, f).await else { continue };
        let Some((login, name, r, a)) = crate::api::repo_named(c, r).await else { continue };
        let org = graftyard_identity::AuthzRead::opened(&a) == Some(AccountKind::Org);
        let mut v = crate::transfer::minimal_repository(c, f.0.0, r.owner, &login, &name, org);
        v["fork"] = json!(true);
        out.push(v);
    }
    Ok(Reply::json(200, Value::Array(out)))
}

/// The direct forks of `parent` that the network chain lists; none when the chain has no group yet.
pub(crate) async fn network_forks(
    c: &Ctx<'_>,
    network: NetworkId,
    parent: RepoId,
) -> std::result::Result<Vec<RepoId>, String> {
    let root = crate::repo_by_id(c, network.root()).await?;
    let b = c.bindings();
    let q = json!({ "q": { "forks": parent.0.0 } }).to_string();
    let Some((v, _, _)) =
        graftyard_lookup::call_opt(&b, ChainId::Network(network), &root.record, "/read", q.as_bytes()).await?
    else {
        return Ok(Vec::new());
    };
    let ids: Vec<i64> = serde_json::from_value(v.pointer("/value/forks").cloned().unwrap_or(Value::Null))
        .map_err(|e| format!("{network:?} forks: {e}"))?;
    Ok(ids.into_iter().map(|i| RepoId(graftyard_types::DbId(i))).collect())
}

#[cfg(test)]
mod tests {
    use graftyard_identity::audit::Owed;
    use graftyard_identity::{AuthzCmd, AuthzOp, RepoName};
    use graftyard_types::{AccountId, DbId, NetworkId, RepoId, Timestamp};
    use serde_json::json;

    /// The edge's two bodies decode as the site reads them: a `CreateFork` naming the parent and its network, which a
    /// fork of a fork takes from its parent (not the parent's own id), and the in-band finish's `Owed::Fork`.
    #[test]
    fn a_forks_commit_and_finish_bodies_decode_as_the_site_reads_them() {
        let (parent, network) = (RepoId(DbId(41)), NetworkId(DbId(40)));
        let name = RepoName::parse("tool").unwrap();
        let cmd =
            json!({ "cred": { "account": 7, "key": null }, "op": super::create_op(&name, "{}", parent, network) });
        let cmd: AuthzCmd = serde_json::from_value(cmd).unwrap();
        assert_eq!(cmd.op, AuthzOp::CreateFork { name: name.clone(), init: "{}".into(), parent, network });

        let owner = AccountId(DbId(7));
        let v = super::owed(owner, &name, RepoId(DbId(42)), "{}", parent, network, Timestamp(9));
        let owed: Owed = serde_json::from_value(v["owed"].clone()).unwrap();
        let want = Owed::Fork {
            owner,
            key: name.key(),
            name,
            repo: RepoId(DbId(42)),
            init: "{}".into(),
            parent,
            network,
            at: Timestamp(9),
        };
        assert_eq!(owed, want);
    }
}
