//! Test installs only: BF-2's queue (`build/BUILD-PLAN.md`, M3-WP2). `POST /_bed/rows/queue?repo=ID` with the bed
//! token and the caller's token sends `n` pushes under that token, each a new ref, to the repository's leader in one
//! `/commit_many` call, whose `hold_deps_ms` holds the leader's first dependency read so they all wait in admission.
//! The answer is every push's outcome and the leader's `deps_log` (the index of each chain read per entry), so the
//! driver can compare every acknowledged push's entry with the revocation's `Tokens` index.

use graftyard_git::group::CommitReq;
use graftyard_git::refs::{Atomicity, ObjectId, RefName, RefUpdate};
use graftyard_git::repo::Push;
use graftyard_types::{ChainId, DbId, RepoId};
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;

pub const PREFIX: &str = "/_bed/rows/";

fn rid() -> graftyard_types::RequestId {
    graftyard_types::RequestId(graftyard_types::Ulid::new(crate::now(), crate::rand10()))
}

pub async fn serve(c: &Ctx<'_>, req: &mut Request, path: &str) -> Result<Response> {
    let token = c.env.secret("BED_TOKEN")?.to_string();
    if req.headers().get("x-bed-token")?.as_deref() != Some(token.as_str()) || path != "/_bed/rows/queue" {
        return crate::text(c.rid, 404, "not found\n");
    }
    let Some(actor) = crate::authenticate(c, req).await? else { return crate::unauthorized(c.rid, "Bad credentials") };
    let b: Value = req.json().await?;
    let id = b["repo"].as_i64().map(|r| RepoId(DbId(r))).ok_or("repo")?;
    let n = b["n"].as_u64().unwrap_or(100) as usize;
    let repo = crate::repo_by_id(c, id).await.map_err(Error::RustError)?;
    let chain = ChainId::Repo(repo.id);
    let tag = b["tag"].as_str().unwrap_or("q");
    let mut cmds = Vec::with_capacity(n);
    for i in 0..n {
        let name = RefName::parse(format!("refs/heads/bf2/{tag}/{i}").as_bytes()).map_err(|e| e.to_string())?;
        let mut oid = [0u8; 20];
        oid[..10].copy_from_slice(&crate::rand10());
        oid[10..18].copy_from_slice(&(i as u64).to_be_bytes());
        let r = rid();
        let push = Push {
            updates: vec![RefUpdate { name, expect: None, new: Some(ObjectId(oid)) }],
            atomicity: Atomicity::PerRef,
            pack: None,
            actor: actor.clone(),
            options: Vec::new(),
            request: r,
            agent: None,
            provenance: Vec::new(),
            protected: Vec::new(),
            changed: Vec::new(),
            leases: false,
            test_format: false,
        };
        cmds.push(serde_json::to_value(CommitReq { rid: r.to_string(), cmd: push })?);
    }
    // The leader: the member that answers a read.
    let (_, _, lead) = graftyard_lookup::call_raw(&c.bindings(), chain, &repo.record, "/read", b"{\"q\":{}}")
        .await
        .map_err(Error::RustError)?;
    let stub = c.bindings().member(lead.site, &lead.do_name)?;
    let body = json!({ "cmds": cmds, "hold_deps_ms": b["hold_deps_ms"], "hold_ms": b["hold_ms"] }).to_string();
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(body.into()));
    let t0 = crate::now().0;
    let mut r = stub.fetch_with_request(Request::new_with_init("https://member/commit_many", &init)?).await?;
    let results: Value = r.json().await?;
    let ms = crate::now().0 - t0;
    let mut d = stub.fetch_with_str("https://member/deps_log").await?;
    let deps: Value = d.json().await?;
    let since: Vec<Value> =
        deps.as_array().into_iter().flatten().filter(|e| e["at"].as_u64().is_some_and(|a| a >= t0)).cloned().collect();
    Response::from_json(&json!({
        "leader": { "site": lead.site.0, "name": lead.do_name.to_string() }, "t0": t0, "ms": ms,
        "results": results["results"], "deps": since,
    }))
}
