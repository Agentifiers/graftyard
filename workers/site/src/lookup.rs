//! The group lookup rule on the site side: a member that calls another group (a dependency read, an id-block lease,
//! the bootstrap check of an owner's record) finds it the way an edge does, with `graftyard_lookup` over this
//! script's `MEMBER_<n>` and `REGISTRY_<n>` bindings.

use graftyard_lookup::Bindings;
pub use graftyard_lookup::Step;
use graftyard_types::{ChainId, PlacementRecord};
use serde_json::Value;
use worker::Env;

use crate::SiteEnv;

pub async fn call(
    env: &Env,
    chain: ChainId,
    record: &PlacementRecord,
    path: &str,
    body: &Value,
) -> Result<(Value, Step), String> {
    let site = SiteEnv::read(env).map_err(|e| e.to_string())?;
    graftyard_lookup::call(&Bindings { env, sites: &site.sites }, chain, record, path, body).await
}

/// A dependency read, hedged after `graftyard_lookup::DEP_HEDGE` (`graftyard_lookup::call_hedged`).
pub async fn call_hedged(
    env: &Env,
    chain: ChainId,
    record: &PlacementRecord,
    path: &str,
    body: &Value,
) -> (Result<(Value, Step), String>, graftyard_lookup::Hedged) {
    match SiteEnv::read(env) {
        Ok(site) => {
            graftyard_lookup::call_hedged(&Bindings { env, sites: &site.sites }, chain, record, path, body).await
        }
        Err(e) => (Err(e.to_string()), graftyard_lookup::Hedged::First),
    }
}

pub async fn commit(env: &Env, chain: ChainId, record: &PlacementRecord, cmd: Value) -> Result<Value, String> {
    let site = SiteEnv::read(env).map_err(|e| e.to_string())?;
    graftyard_lookup::commit(&Bindings { env, sites: &site.sites }, chain, record, cmd).await
}
