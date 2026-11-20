//! Native git over SSH at the edge (`design/DESIGN.md`, Shape, "SSH front"; journey 4, step 3; `build/BUILD-PLAN.md`,
//! M6-WP2).
//!
//! - `connect`, the script's `connect()` handler, is the origin of the install's Spectrum TCP app on port 22
//!   (`traffic_type: "worker"`). It opens each site's `SshFront` object (`ssh#<site>`, binding `SSHFRONT_<n>`) in turn
//!   until one answers with `graft-sshd`'s banner, and only then pipes the client's bytes, so a front that is down is
//!   passed over before the client has sent it anything.
//! - Calls from a site's `SshOutbound` arrive over that site's `EDGE` binding with `props` `{"ssh": true}`
//!   (`from_front`). Their caller is the key in `x-graftyard-ssh-key`, which `graft-sshd` verified; they reach only
//!   `GET /_graftyard/ssh/key` and git's smart-HTTP routes. A public request has no props, so the header means nothing
//!   on it. A push then takes the HTTPS path whole, with `Credential::Ssh` (format 32.26), and its leader re-reads the
//!   key's `Tokens` row once per entry, so a removed key refuses every command decided after the removal (BF-52).
//! - `GET /api/graftyard/v1/meta/ssh` lists both host-key slots from `SSH_HOST_KEYS`, which the deploy writes (BF-51).
//! - `DELETE /user/keys/{id}` (GHES's "Delete a public SSH key") removes the key's `Tokens` row, then its listing.

use graftyard_identity::{Actor, Credential, Fingerprint, Principal, ScopeSet, UserId, fingerprint_shard};
use graftyard_types::{AccountId, ChainId, Digest};
use graftyard_wire::Reply;
use js_sys::{Function, Object, Reflect, Uint8Array};
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use worker::*;

use crate::Ctx;

/// The key lookup `graft-sshd` makes before it accepts a client key.
pub const KEY_PATH: &str = "/_graftyard/ssh/key";
/// The header that carries the verified key's SHA-256 fingerprint, hex.
pub const KEY_HEADER: &str = "x-graftyard-ssh-key";
pub const META_PATH: &str = "/api/graftyard/v1/meta/ssh";

/// Whether this invocation came over an `EDGE` binding whose `props` mark it as a site's SSH front.
pub fn from_front(ctx: &Context) -> bool {
    ctx.props::<Value>().ok().is_some_and(|p| p["ssh"] == json!(true))
}

/// The only paths a front's call may reach: the key lookup and git's smart-HTTP routes.
pub fn front_path(path: &str) -> bool {
    path == KEY_PATH || ["/info/refs", "/git-upload-pack", "/git-receive-pack"].iter().any(|s| path.ends_with(s))
}

#[derive(serde::Deserialize)]
struct KeyRow {
    account: AccountId,
    repo: Option<Value>,
}

/// The key's `Tokens` row on its `fingerprint_shard`, read through `fresh`: `Some(account)` for a user's key, `None`
/// for an unknown or removed one and for a deploy key, which never acts as a user.
async fn key_owner(c: &Ctx<'_>, fp: &Fingerprint) -> std::result::Result<Option<AccountId>, String> {
    let v =
        crate::system_read(c, ChainId::Tokens(fingerprint_shard(fp)), json!({ "sshKey": Digest(fp.0).hex() })).await?;
    let row: Option<KeyRow> = serde_json::from_value(v["sshKey"].clone()).map_err(|e| e.to_string())?;
    Ok(row.filter(|r| r.repo.is_none()).map(|r| r.account))
}

fn fingerprint(s: &str) -> Option<Fingerprint> {
    Digest::parse_hex(s).map(|d| Fingerprint(d.0))
}

/// The caller of a front's request: the user whose key it is, or `None`.
pub async fn principal(c: &Ctx<'_>, req: &Request) -> Result<Option<Principal>> {
    let Some(fp) = req.headers().get(KEY_HEADER)?.as_deref().and_then(fingerprint) else { return Ok(None) };
    let owner = key_owner(c, &fp).await.map_err(Error::RustError)?;
    Ok(owner.map(|a| Principal {
        actor: Actor::User(UserId(a)),
        scopes: ScopeSet(1),
        credential: Credential::Ssh(fp),
        sso_orgs: smallvec::SmallVec::new(),
    }))
}

/// `GET /_graftyard/ssh/key?fp=<hex>`, a front's only: 200 for a user's key, 404 for none, 503 when the read failed,
/// which `graft-sshd` retries.
pub async fn key(c: &Ctx<'_>, req: &Request) -> Result<Response> {
    if !c.ssh_front {
        return crate::text(c.rid, 404, "Not Found\n");
    }
    let url = req.url()?;
    let fp = url.query_pairs().find(|(k, _)| k == "fp").and_then(|(_, v)| fingerprint(&v));
    let Some(fp) = fp else { return crate::text(c.rid, 400, "fp is 64 hex characters\n") };
    match key_owner(c, &fp).await {
        Ok(Some(a)) => crate::text(c.rid, 200, &json!({ "account": a.0.0 }).to_string()),
        Ok(None) => crate::text(c.rid, 404, "no such key\n"),
        Err(e) => crate::text(c.rid, 503, &format!("the key's row did not answer; retry ({e})\n")),
    }
}

/// `GET /api/graftyard/v1/meta/ssh`: both host-key slots, `{"active", "slots": {"A": [...], "B": [...]}}`, each key's
/// type, public key, and SHA-256 fingerprint, as the deploy wrote them; 404 where the install has SSH off.
pub fn meta(c: &Ctx<'_>) -> Result<Response> {
    let keys = c.env.var("SSH_HOST_KEYS").ok().map(|v| v.to_string()).filter(|_| c.cfg.ssh_option);
    let Some(keys) = keys else { return crate::text(c.rid, 404, "SSH is off on this install\n") };
    let h = crate::headers(c.rid)?;
    h.set("content-type", "application/json; charset=utf-8")?;
    Ok(Response::ok(keys)?.with_headers(h))
}

/// `DELETE /user/keys/{id}`: the caller's key with that id (`keys::key_id`) is removed on its `fingerprint_shard`, the
/// commit point a push's leader re-reads, then from the caller's list. 204 after both, 404 for a key the caller
/// does not list.
pub async fn delete_key(c: &Ctx<'_>, req: &Request, id: &str) -> Result<Reply> {
    const DOC: &str = "rest/users/keys#delete-a-public-ssh-key-for-the-authenticated-user";
    let Some(who) = crate::authenticate(c, req).await? else {
        return Ok(Reply::error(401, "Requires authentication", DOC));
    };
    let Actor::User(u) = who.actor else { return Ok(Reply::error(403, "Resource not accessible by integration", DOC)) };
    let Some(fp) = crate::keys::listed(c, u.0).await?.into_iter().find(|f| id.parse() == Ok(crate::keys::key_id(f)))
    else {
        return Ok(Reply::error(404, "Not Found", DOC));
    };
    let cred = who.credential_shard();
    let remove = json!({ "cred": cred, "op": { "RemoveSshKey": { "fingerprint": fp } } });
    match crate::ident::commit_system(c, ChainId::Tokens(fingerprint_shard(&fp)), remove).await {
        Ok(d) if d.get("ok").is_some() || d.pointer("/refused/err").and_then(Value::as_str) == Some("NoKey") => {}
        Ok(d) => return Ok(crate::api::unavailable(&format!("the key's removal was refused: {}", d["refused"]))),
        Err(x) => return Ok(x),
    }
    let unlist = json!({ "cred": cred, "op": { "RemoveCredential": { "credential": { "SshKey": fp } } } });
    match crate::ident::commit_owner(c, ChainId::Identity(u.0), u.0, unlist).await {
        Ok(d) if d.get("ok").is_some() => Ok(Reply::empty(204)),
        // The key is gone from `Tokens`, so it authenticates nothing; only its listing is left.
        Ok(d) => Ok(crate::api::unavailable(&format!("the key's listing was not removed: {}", d["refused"]))),
        Err(x) => Ok(x),
    }
}

// ---- the `connect()` handler ----

fn call(o: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue> {
    let f: Function = Reflect::get(o, &name.into())?.dyn_into()?;
    Ok(f.apply(o, &args.iter().collect())?)
}

async fn wait(p: JsValue) -> Result<JsValue> {
    Ok(JsFuture::from(js_sys::Promise::from(p)).await?)
}

/// Site `site`'s front, opened, with the first bytes it sent (`graft-sshd`'s banner), or `None` when it closed first.
async fn open_front(env: &Env, site: &graftyard_types::Site) -> Result<Option<(JsValue, Uint8Array)>> {
    let ns = env.durable_object(&format!("SSHFRONT_{}", site.id.0))?;
    let stub: JsValue =
        ns.get_by_name_with_location_hint(&format!("ssh#{}", site.id.0), site.region.hint())?.into_rpc();
    let opts = Object::new();
    Reflect::set(&opts, &"allowHalfOpen".into(), &true.into())?;
    let sock = call(&stub, "connect", &["ssh:22".into(), opts.into()])?;
    wait(Reflect::get(&sock, &"opened".into())?).await?;
    let reader = call(&Reflect::get(&sock, &"readable".into())?, "getReader", &[])?;
    let first = wait(call(&reader, "read", &[])?).await?;
    call(&reader, "releaseLock", &[])?;
    let value = Reflect::get(&first, &"value".into())?;
    if Reflect::get(&first, &"done".into())?.is_truthy() || value.is_undefined() {
        let _ = call(&sock, "close", &[]);
        return Ok(None);
    }
    Ok(Some((sock, value.unchecked_into())))
}

/// The order the sites' fronts are tried in: the active sites, from one picked by the time so sessions spread over
/// the fronts [guess], then the rest in turn.
pub fn order(sites: &[graftyard_types::Site], now_ms: u64) -> Vec<&graftyard_types::Site> {
    let active: Vec<_> = sites.iter().filter(|s| !s.spare).collect();
    let n = active.len().max(1);
    let start = (now_ms as usize) % n;
    active.iter().cycle().skip(start).take(active.len()).copied().collect()
}

#[event(connect)]
async fn connect(socket: worker_sys::Socket, env: Env, _ctx: Context) -> Result<()> {
    let client: JsValue = socket.into();
    if env.var("SSH_OPTION").map(|v| v.to_string()).ok().as_deref() != Some("on") {
        let _ = call(&client, "close", &[]);
        return Ok(());
    }
    let sites: graftyard_types::SiteTable = serde_json::from_str(&env.var("GRAFTYARD_SITES")?.to_string())?;
    let mut front = None;
    for site in order(&sites.0, js_sys::Date::now() as u64) {
        match open_front(&env, site).await {
            Ok(Some(f)) => {
                front = Some(f);
                break;
            }
            Ok(None) => console_error!("ssh: site {}'s front closed before its banner", site.id.0),
            Err(e) => console_error!("ssh: site {}'s front: {e}", site.id.0),
        }
    }
    let Some((front, banner)) = front else {
        console_error!("ssh: no site's front answered");
        let _ = call(&client, "close", &[]);
        return Ok(());
    };
    let writer = call(&Reflect::get(&client, &"writable".into())?, "getWriter", &[])?;
    wait(call(&writer, "write", &[banner.into()])?).await?;
    call(&writer, "releaseLock", &[])?;
    let pipe = |from: &JsValue, to: &JsValue| -> Result<JsFuture> {
        let p = call(&Reflect::get(from, &"readable".into())?, "pipeTo", &[Reflect::get(to, &"writable".into())?])?;
        Ok(JsFuture::from(js_sys::Promise::from(p)))
    };
    let (up, down) = (pipe(&client, &front)?, pipe(&front, &client)?);
    // Either direction may end with an error when the other side resets; the session is over either way.
    let _ = futures_util::future::join(up, down).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use graftyard_types::{Region, Site, SiteId, SiteLink};

    use super::{front_path, order};

    fn site(id: u8, spare: bool) -> Site {
        Site {
            id: SiteId(id),
            script: format!("s{id}"),
            account: "a".into(),
            region: Region::Enam,
            jurisdiction: None,
            bucket: format!("b{id}"),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare,
        }
    }

    /// Every active site is tried once, starting from a different one as time moves; a spare site holds no front.
    #[test]
    fn every_active_front_is_tried_once_from_a_moving_start() {
        let sites = [site(1, false), site(2, true), site(3, false), site(4, false)];
        let ids = |now| order(&sites, now).iter().map(|s| s.id.0).collect::<Vec<_>>();
        assert_eq!(ids(0), [1, 3, 4]);
        assert_eq!(ids(1), [3, 4, 1]);
        assert_eq!(ids(5), [4, 1, 3]);
        assert!(order(&[], 7).is_empty());
    }

    /// A front's call reaches the key lookup and git's three routes, and nothing else the edge serves.
    #[test]
    fn a_front_reaches_only_the_key_lookup_and_git() {
        for p in ["/_graftyard/ssh/key", "/o/r.git/info/refs", "/o/r.git/git-upload-pack", "/o/r/git-receive-pack"] {
            assert!(front_path(p), "{p}");
        }
        for p in ["/api/v3/user", "/api/graftyard/v1/release/entries", "/_bed/kill", "/o/r", "/graphql", "/"] {
            assert!(!front_path(p), "{p}");
        }
    }
}
