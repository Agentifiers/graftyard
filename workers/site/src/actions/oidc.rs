//! Actions OIDC on the shard leader (`graftyard_actions::oidc`; `decisions/DECISIONS.md`, "`actions-oidc`"): the
//! run service's `idtoken` op, which signs a job's ID token under `ActionsOidcKey`'s active slot, and the slots' public
//! halves that the site's Registry serves at `/oidc_slots`.
//!
//! - **The key.** Both slots are Secrets Store bindings, `ACTIONS_OIDC_KEY_A` and `_B`, which every site script binds.
//!   The active one is the `Release` view's `ActionsOidcKey` slot, so a committed `Rotate` switch is signed under with
//!   no new script version; an install with no anchor runs no `Release` group and signs under slot 0, the fold's
//!   default. A parsed key is kept `KEY_FOR_MS`, keyed by its slot, so a slot written again is read within that.
//! - **The facts the shard does not hold** come from the edge, which read them for this job: the repository's `o/r`,
//!   its visibility, its enterprise, and whether the job's runner is a hosted one (`workers/edge/src/actions/oidc.rs`).

use std::cell::RefCell;
use std::rc::Rc;

use graftyard_actions::JobId;
use graftyard_actions::oidc::{self, OidcKey};
use graftyard_identity::Visibility;
use graftyard_types::{AccountId, DbId};
use serde_json::{Value, json};

use super::{Actions, fresh_rid, refused};
use crate::host::Host;

/// The two slots' bindings, slot 0 first.
pub const SLOT_BINDINGS: [&str; 2] = ["ACTIONS_OIDC_KEY_A", "ACTIONS_OIDC_KEY_B"];
/// How long an isolate keeps a slot's parsed key [guess: as the SSH host-key slots, `SLOTS_FOR_MS`].
const KEY_FOR_MS: u64 = 60_000;

thread_local! {
    static KEY: RefCell<Option<(u8, u64, Rc<OidcKey>)>> = const { RefCell::new(None) };
}

/// A slot's text, or `None` for an unbound or empty one.
pub async fn slot_text(env: &worker::Env, slot: usize) -> Option<String> {
    let s = env.secret_store(SLOT_BINDINGS[slot]).ok()?;
    s.get().await.ok().flatten().filter(|t| !t.trim().is_empty())
}

/// `ActionsOidcKey`'s active slot: the `Release` view's, or 0 with no anchor.
async fn active_slot(env: &worker::Env) -> Result<u8, String> {
    let Some(anchor) = crate::release::anchor(env) else { return Ok(0) };
    let site = crate::SiteEnv::read(env).map_err(|e| e.to_string())?;
    graftyard_lookup::view::ensure(&graftyard_lookup::Bindings { env, sites: &site.sites }, &anchor, None).await;
    graftyard_lookup::view::with(|r| {
        (r.count() > 0).then(|| r.fold.slot(graftyard_release::SecretName::ActionsOidcKey))
    })
    .ok_or_else(|| "ActionsOidcKey: no Release view yet".into())
}

/// The active slot's private key.
async fn active_key(env: &worker::Env) -> Result<Rc<OidcKey>, String> {
    let slot = active_slot(env).await?;
    let now = crate::now_ms();
    let kept = KEY.with(|k| {
        k.borrow().as_ref().filter(|(s, at, _)| *s == slot && now < at + KEY_FOR_MS).map(|(_, _, k)| k.clone())
    });
    if let Some(k) = kept {
        return Ok(k);
    }
    let name = SLOT_BINDINGS[usize::from(slot)];
    let text = slot_text(env, usize::from(slot)).await.ok_or_else(|| format!("{name} is not bound or is empty"))?;
    let key = Rc::new(oidc::parse_key(&text).map_err(|e| format!("{name}: {e}"))?);
    KEY.with(|k| *k.borrow_mut() = Some((slot, now, key.clone())));
    Ok(key)
}

/// `{"op": "idtoken", "grant": Job, "audience"?, "repository", "visibility", "enterprise"?, "hosted"}`: the job's ID
/// token as the toolkit reads it, `{"value": jwt}`, or 403 with why not (`oidc::may_mint`).
pub async fn mint(h: &Rc<Host<Actions>>, b: &Value, owner: AccountId, job: JobId, attempt: u16) -> Value {
    let (Some(repository), Ok(visibility)) =
        (b["repository"].as_str(), serde_json::from_value::<Visibility>(b["visibility"].clone()))
    else {
        return refused(400, "repository and visibility");
    };
    let enterprise = b["enterprise"].as_array().and_then(|e| Some((e.first()?.as_str()?, e.get(1)?.as_i64()?)));
    // F122: the owner's `sub` template, which the edge read from the owner's shard 0 (`orgSub`).
    let org: Option<Vec<Box<str>>> = serde_json::from_value(b["org_sub"].clone()).unwrap_or_default();
    let read = {
        let core = h.core.borrow();
        core.as_ref().and_then(|c| c.m.group_fold().fold.claim_facts(job, org.as_deref()))
    };
    let Some(cf) = read else { return refused(404, "no job") };
    let server = super::urls(&h.env).server;
    let enterprise = enterprise.map(|(l, id)| (l, AccountId(DbId(id))));
    let f = cf.facts(&server, repository, owner, visibility, enterprise, b["hosted"].as_bool() == Some(true));
    let (row, run, declared) = (&cf.row, &cf.run, &cf.declared);
    // Refused before the key is read, so a job with no grant never reaches the Secrets Store.
    if let Err(e) = oidc::may_mint(row, run, attempt, declared.as_ref()) {
        return refused(403, e);
    }
    let key = match active_key(&h.env).await {
        Ok(k) => k,
        Err(e) => return refused(503, e),
    };
    let audience = b["audience"].as_str();
    let jti = fresh_rid().0.to_string();
    match oidc::mint(&f, attempt, declared.as_ref(), audience, crate::now_ms() / 1000, &jti, &key) {
        Ok(t) => json!({ "status": 200, "body": { "value": t } }),
        Err(e) => refused(403, e),
    }
}

/// The Registry's `GET /oidc_slots`: `oidc::published` of both slots, so no private byte leaves.
pub async fn published(env: &worker::Env) -> Result<Value, String> {
    let (a, b) = (slot_text(env, 0).await, slot_text(env, 1).await);
    oidc::published([a.as_deref(), b.as_deref()]).map_err(|(slot, e)| format!("ACTIONS_OIDC_KEY_{slot}: {e}"))
}
