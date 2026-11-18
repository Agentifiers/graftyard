//! `POST /api/graftyard/v1/admin/repos/{id}/purge`: a deleted repository's purge, on product builds
//! (`design/SHARED.md`, "Repository repair, tombstones, and retention", Purge; `decisions/DECISIONS.md`,
//! "`repo-purge`"; the pure half is `graftyard_git::purge`).
//!
//! - **Who.** A site admin: an admin of the install's own org, whose `Authz` fold the edge reads, as for
//!   `instance-auth`. Anyone else gets 404, so the route does not show which repository ids exist.
//! - **What.** The repository's owner from `RepoOwner`, its `Authz` name, and each member's `/status` for the
//!   repository chain and its metadata child. The gate (`purge::gate`) refuses 409 with its reason, and nothing is
//!   retired, unless the name is `Deleted`, its `retain_until` has passed or the body says `{"early": true}`, every
//!   member answers, every joined member's fold holds the tombstone, and none is in a group change.
//! - **Then.** Each member gets `/purge_retire`, which refuses on its own a member that began a change since, and its
//!   site's registry row naming it is deleted. Then each site's bucket loses the network's objects, once
//!   (`purge::purge_bucket`). 200 with the counts. The second pass is a second call once `secondPassAt` has passed:
//!   the gate passes again on members that answer "not joined", and the response says how many objects that pass
//!   found.
//!
//! No request path calls the Cloudflare API: the members and registries are bindings and the buckets `BUCKET_<n>`.

use graftyard_git::host::R2;
use graftyard_git::purge::{self, Facts, Refused, Status};
use graftyard_lookup::Bindings;
use graftyard_raft::GroupConfig;
use graftyard_types::{ChainId, DbId, MemberRegistry, RepoId, SiteId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;

pub const PREFIX: &str = "/api/graftyard/v1/admin/repos/";

/// The route, or `None` for another path.
pub async fn route(c: &Ctx<'_>, req: &mut Request, path: &str) -> Result<Option<Reply>> {
    let Some(id) = path.strip_prefix(PREFIX).and_then(|r| r.strip_suffix("/purge")) else { return Ok(None) };
    let Ok(id) = id.parse::<i64>() else { return Ok(Some(Reply::not_found())) };
    if req.method() != Method::Post {
        return Ok(Some(Reply::error(405, "Method Not Allowed", "graftyard")));
    }
    let Some(who) = crate::authenticate(c, req).await? else {
        return Ok(Some(Reply::error(401, "Requires authentication", "rest")));
    };
    let install = graftyard_identity::install_account();
    let Some(record) = crate::placement(c, install).await.map_err(Error::RustError)? else {
        return Ok(Some(Reply::not_found()));
    };
    let Some(f) = crate::authz_fold(c, install, &record).await else {
        return Ok(Some(crate::api::unavailable("the install's Authz chain did not answer; retry")));
    };
    if !matches!(who.actor, graftyard_identity::Actor::User(u) if graftyard_identity::AuthzRead::is_admin(&f, u)) {
        return Ok(Some(Reply::not_found()));
    }
    let early = req.json::<Value>().await.ok().and_then(|b| b["early"].as_bool()).unwrap_or(false);
    Ok(Some(purge_repo(c, RepoId(DbId(id)), early).await.unwrap_or_else(|e| crate::api::unavailable(&e))))
}

async fn purge_repo(c: &Ctx<'_>, repo: RepoId, early: bool) -> std::result::Result<Reply, String> {
    let r = crate::repo_by_id(c, repo).await.map_err(|e| format!("the repository's owner: {e}"))?;
    let authz = crate::ident::authz_of(c, r.owner).await?.ok_or("the owner's Authz chain did not answer; retry")?;
    let b = c.bindings();
    let mut members = Vec::new();
    for chain in purge::chains(repo) {
        for (site, name) in plan(&b, &r.record, chain).await {
            let s = status(&b, site, &name).await;
            members.push((chain, site, name, s));
        }
    }
    let facts = Facts { deleted: authz.deleted(repo), now: crate::now(), early, members: &members };
    if let Err(why) = purge::gate(&facts) {
        return Ok(refused(&why));
    }
    let mut retired = Vec::new();
    for (chain, site, name, s) in &members {
        // A member already not joined still loses its row: a purge cut between the two left it.
        let sent = match s {
            Status::NotJoined => Some((200, Value::Null)),
            _ => graftyard_lookup::post(&b, *site, name, "/purge_retire", b"{}").await,
        };
        match (s, sent) {
            (_, Some((200, _))) => {}
            // The member aborts as it answers, so the reply may be lost: its status after is the check.
            (_, None) if status(&b, *site, name).await == Status::NotJoined => {}
            (_, Some((409, v))) => {
                return Ok(Reply::json(
                    409,
                    json!({ "message": "A member refused its purge", "member": &**name, "refused": v["refused"], "retired": retired }),
                ));
            }
            (_, other) => return Err(format!("{name}: purge_retire answered {other:?}; retry")),
        }
        unregister(&b, *site, *chain, name).await?;
        if *s != Status::NotJoined {
            retired.push(name.to_string());
        }
    }
    let network = purge::network(repo);
    let mut found = 0;
    for s in &c.sites.0 {
        let bucket = R2(c.env.bucket(&format!("BUCKET_{}", s.id.0)).map_err(|e| e.to_string())?);
        let (n, left) = purge::purge_bucket(&bucket, network).await.map_err(|e| format!("site {}: {e}", s.id.0))?;
        if left > 0 {
            return Err(format!("site {}: {left} of the network's objects are still listed; retry", s.id.0));
        }
        found += n;
    }
    let second = purge::second_pass_at(crate::now());
    Ok(Reply::json(200, json!({ "repo": repo.0.0, "retired": retired, "objects": found, "secondPassAt": second.0 })))
}

/// Every member to retire for `chain`: each record's bootstrap members, each site's registry row, and the newest
/// committed config's voters and learners that any of them answers.
async fn plan(b: &Bindings<'_>, record: &graftyard_types::PlacementRecord, chain: ChainId) -> Vec<(SiteId, Box<str>)> {
    let bootstrap: Vec<(SiteId, Box<str>)> =
        record.lookup_order().flat_map(|r| r.bootstrap_members(chain)).map(|m| (m.site, m.do_name)).collect();
    let mut rows = Vec::new();
    for s in &b.sites.0 {
        if let Some(row) = registry_row(b, s.id, chain).await {
            rows.push((s.id, row));
        }
    }
    let mut newest: Option<(u64, GroupConfig)> = None;
    for (site, name) in bootstrap.iter().chain(&rows) {
        let Some(v) = member_status(b, *site, name).await else { continue };
        let index = v.pointer("/config/index").and_then(Value::as_u64).unwrap_or(0);
        if let Ok(cfg) = serde_json::from_value::<GroupConfig>(v.pointer("/config/config").cloned().unwrap_or_default())
            && newest.as_ref().is_none_or(|(i, _)| index > *i)
        {
            newest = Some((index, cfg));
        }
    }
    let config: Vec<(SiteId, Box<str>)> = match &newest {
        Some((_, GroupConfig::Normal(p) | GroupConfig::Degraded { placement: p, .. })) => members_of(&[p]),
        Some((_, GroupConfig::Joint { old, new })) => members_of(&[old, new]),
        None => Vec::new(),
    };
    purge::retire_plan(&bootstrap, &rows, &config)
}

fn members_of(ps: &[&graftyard_types::Placement]) -> Vec<(SiteId, Box<str>)> {
    ps.iter().flat_map(|p| p.voters().iter().chain(p.learners())).map(|m| (m.site, m.do_name.clone())).collect()
}

async fn registry_row(b: &Bindings<'_>, site: SiteId, chain: ChainId) -> Option<Box<str>> {
    let reg = MemberRegistry::for_chain(site, chain);
    let mut res = b
        .registry(site, &reg.do_name())
        .ok()?
        .fetch_with_str(&format!("https://registry/row?chain={chain}"))
        .await
        .ok()?;
    if res.status_code() != 200 {
        return None;
    }
    res.json::<graftyard_types::RegistryRow>().await.ok().map(|r| r.do_name)
}

/// The site's registry row for `chain`, if it still names `name`.
async fn unregister(b: &Bindings<'_>, site: SiteId, chain: ChainId, name: &str) -> std::result::Result<(), String> {
    let reg = MemberRegistry::for_chain(site, chain);
    let stub = b.registry(site, &reg.do_name()).map_err(|e| e.to_string())?;
    let url = format!("https://registry/row?chain={chain}&do_name={}", js_sys::encode_uri_component(name));
    let mut init = RequestInit::new();
    init.with_method(Method::Delete);
    let res = stub.fetch_with_request(Request::new_with_init(&url, &init).map_err(|e| e.to_string())?).await;
    match res.map(|r| r.status_code()) {
        Ok(200) => Ok(()),
        other => Err(format!("site {}: the registry row of {name} was not deleted ({other:?}); retry", site.0)),
    }
}

/// A member's `/status`, within the lookup's `CALL`.
async fn member_status(b: &Bindings<'_>, site: SiteId, name: &str) -> Option<Value> {
    let stub = b.member(site, name).ok()?;
    let call = async move {
        let mut r = stub.fetch_with_str("https://member/status").await.ok()?;
        if r.status_code() != 200 {
            return None;
        }
        r.json::<Value>().await.ok()
    };
    graftyard_lookup::within(graftyard_lookup::CALL, call).await.flatten()
}

async fn status(b: &Bindings<'_>, site: SiteId, name: &str) -> Status {
    match member_status(b, site, name).await {
        Some(v) if v["joined"] == json!(false) => Status::NotJoined,
        Some(v) if v["joined"] == json!(true) => Status::Joined {
            purgeable: v["purgeable"].as_bool() == Some(true),
            changing: v["changing"].as_bool() != Some(false),
        },
        _ => Status::Unanswered,
    }
}

fn refused(why: &Refused) -> Reply {
    let (message, v) = match why {
        Refused::NotDeleted => ("The repository is not deleted", json!({})),
        Refused::Retained { until } => (
            "The repository is within its retention; send {\"early\": true} to purge it now",
            json!({ "retainUntil": until.0 }),
        ),
        Refused::NoTombstone { chain, member } => (
            "A member has not applied the repository's tombstone",
            json!({ "chain": chain.to_string(), "member": &**member }),
        ),
        Refused::Changing { chain, member } => {
            ("A member is in a group change", json!({ "chain": chain.to_string(), "member": &**member }))
        }
        Refused::Unanswered { chain, member } => {
            ("A member did not answer", json!({ "chain": chain.to_string(), "member": &**member }))
        }
    };
    let mut body = json!({ "message": message, "documentation_url": "graftyard" });
    if let Value::Object(m) = v {
        body.as_object_mut().expect("an object").extend(m);
    }
    Reply::json(409, body)
}
