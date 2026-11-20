//! `GET` and `POST /user/keys` (`build/BUILD-PLAN.md`, M3-WP3, BF-35; GHES 3.22's `key` shape), with no `Audit`
//! consumer: the list is the caller's `Identity` fold read through `fresh`, and a new key is listed by the edge
//! itself right after its `Tokens` row commits.
//!
//! `POST` commits `AddSshKey` on the key's `fingerprint_shard` (the commit point, where a fingerprint has one owner),
//! then `IdentityOp::AddKey` with the key's text (format 0x14.2), whose leader refuses text that does not hash to the
//! fingerprint. A key's `id` is the first 6 bytes of its fingerprint as a number [inference: GHES assigns ids from a
//! sequence; a fingerprint is already unique on the install and needs no id block], so the same key always shows the
//! same id. `created_at` is not stored, so it is the Unix epoch [inference]. A key added before 0x14.2 lists with no
//! text and is left out, since `key` is required.

use graftyard_identity::{Fingerprint, IDENTITY_FORMAT_V2, fingerprint_shard, key_fingerprint};
use graftyard_types::{AccountId, ChainId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::ident::{commit_owner, commit_system};

const DOC_LIST: &str = "rest/users/keys#list-public-ssh-keys-for-the-authenticated-user";
const DOC_ADD: &str = "rest/users/keys#create-a-public-ssh-key-for-the-authenticated-user";

pub(crate) fn key_id(fp: &Fingerprint) -> i64 {
    let mut b = [0u8; 8];
    b[2..].copy_from_slice(&fp.0[..6]);
    i64::from_be_bytes(b)
}

fn key_json(c: &Ctx<'_>, fp: &Fingerprint, key: &str, title: &str) -> Value {
    let id = key_id(fp);
    json!({
        "key": key, "id": id, "url": format!("https://{}/api/v3/user/keys/{id}", c.cfg.host), "title": title,
        "created_at": "1970-01-01T00:00:00Z", "verified": true, "read_only": false, "last_used": null,
    })
}

async fn me(
    c: &Ctx<'_>,
    req: &Request,
) -> Result<std::result::Result<(graftyard_identity::Principal, AccountId), Reply>> {
    let Some(who) = crate::authenticate(c, req).await? else {
        return Ok(Err(Reply::error(401, "Requires authentication", DOC_LIST)));
    };
    let graftyard_identity::Actor::User(u) = who.actor else {
        return Ok(Err(Reply::error(403, "Resource not accessible by integration", DOC_LIST)));
    };
    Ok(Ok((who, u.0)))
}

/// `GET /user/keys`: every SSH key on the caller's `Identity` fold that has its text.
pub async fn list(c: &Ctx<'_>, req: &Request) -> Result<Reply> {
    let (_, account) = match me(c, req).await? {
        Ok(x) => x,
        Err(r) => return Ok(r),
    };
    let Some(record) = crate::placement(c, account).await.map_err(Error::RustError)? else {
        return Ok(Reply::json(200, json!([])));
    };
    let body = json!({ "q": {} }).to_string();
    let chain = ChainId::Identity(account);
    let v = match graftyard_lookup::call_opt(&c.bindings(), chain, &record, "/read", body.as_bytes()).await {
        Ok(Some((v, _, _))) => v["value"].clone(),
        Ok(None) => return Ok(Reply::json(200, json!([]))),
        Err(e) => return Ok(crate::api::unavailable(&e)),
    };
    let keys: Vec<Value> = v["credentials"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|k| {
            let fp = graftyard_types::Digest::parse_hex(k["sshKey"].as_str()?)?;
            Some(key_json(c, &Fingerprint(fp.0), k["key"].as_str()?, k["title"].as_str().unwrap_or("")))
        })
        .collect();
    Ok(Reply::json(200, Value::Array(keys)))
}

/// The fingerprints of every SSH key on `account`'s `Identity` fold, read through `fresh`, with or without text.
pub(crate) async fn listed(c: &Ctx<'_>, account: AccountId) -> Result<Vec<Fingerprint>> {
    let Some(record) = crate::placement(c, account).await.map_err(Error::RustError)? else { return Ok(Vec::new()) };
    let body = json!({ "q": {} }).to_string();
    let chain = ChainId::Identity(account);
    let v = match graftyard_lookup::call_opt(&c.bindings(), chain, &record, "/read", body.as_bytes()).await {
        Ok(Some((v, _, _))) => v["value"].clone(),
        Ok(None) => return Ok(Vec::new()),
        Err(e) => return Err(Error::RustError(e)),
    };
    Ok(v["credentials"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|k| graftyard_types::Digest::parse_hex(k["sshKey"].as_str()?).map(|d| Fingerprint(d.0)))
        .collect())
}

fn invalid(message: &str, code: &'static str) -> Reply {
    let e = graftyard_wire::FieldError { resource: "PublicKey", field: "key", code };
    Reply::json(422, graftyard_wire::validation_error(message, DOC_ADD, &[e]))
}

/// `POST /user/keys` with `{title, key}`: 201 with the `key` once both commits are done.
pub async fn add(c: &Ctx<'_>, req: &mut Request) -> Result<Reply> {
    let (who, account) = match me(c, req).await? {
        Ok(x) => x,
        Err(r) => return Ok(r),
    };
    if !c.cfg.enabled.contains(&IDENTITY_FORMAT_V2) {
        return Ok(crate::api::unavailable("SSH keys need record format 20.2, which FormatsEnable has not enabled"));
    }
    let body: Value = req.json().await.unwrap_or(Value::Null);
    let text = body["key"].as_str().unwrap_or_default();
    // The type and the blob; GitHub drops the comment.
    let key: String = text.split_ascii_whitespace().take(2).collect::<Vec<_>>().join(" ");
    let Some(fp) = key_fingerprint(&key) else { return Ok(invalid("Validation Failed", "custom")) };
    let title = body["title"].as_str().unwrap_or_default().to_owned();
    let cred = who.credential_shard();
    let add = json!({ "cred": cred, "op": { "AddSshKey": { "fingerprint": fp, "account": account, "repo": null } } });
    let done = match commit_system(c, ChainId::Tokens(fingerprint_shard(&fp)), add).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    if let Some(e) = done.pointer("/refused/err").and_then(Value::as_str) {
        return Ok(if e.starts_with("KeyTaken") {
            invalid("key is already in use", "custom")
        } else {
            Reply::error(403, &format!("the key was refused: {e}"), DOC_ADD)
        });
    }
    let list = json!({ "cred": cred, "op": { "AddKey": { "fingerprint": fp, "key": key, "title": title } } });
    match commit_owner(c, ChainId::Identity(account), account, list).await {
        Ok(d) if d.get("ok").is_some() => {}
        // The key is live on `Tokens`; `Audit` lists it (without its text) if this commit did not land.
        Ok(d) => {
            return Ok(crate::api::unavailable(&format!("the key's listing was refused: {}", d["refused"]["err"])));
        }
        Err(x) => return Ok(x),
    }
    Ok(Reply::json(201, key_json(c, &fp, &key, &title)))
}
