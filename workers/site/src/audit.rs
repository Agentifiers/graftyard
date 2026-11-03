//! The site script's fold audit on its Cron Trigger (`design/SHARED.md`, "Folds", `fold_audit`; `design/DESIGN.md`,
//! Stack, Cron Triggers). Each run takes `SAMPLE` repository chains this site homes, from this site's
//! `MemberRegistry` shards, starting after the last run's cursor. For each chain:
//!
//! - its leader replays its stored log from genesis into a fold in memory (`/fold {"replay"}`, `crate::snap::replay`),
//!   which gives each tier's refolded digest at an index;
//! - every voter reports its own `fold_digest` of each tier at that index from its digest ring
//!   (`/fold {"at": index}`), whatever batches it applied in;
//! - `fold_audit_compare` compares them, and any mismatch alarms. A member that cannot answer for the index (behind
//!   it, or its metadata tier still loading) is listed, not compared.
//!
//! The report goes to the script's logs, and the alarms to `console_error`, with the cursor in this site's `Registry`
//! object `fold_audit`.

use graftyard_fold::fold_audit_compare;
use graftyard_lookup::Bindings;
use graftyard_types::{ChainId, Digest, Index, MemberRegistry, PlacementRecord, REGISTRY_SHARDS, RegistryRow, Tier};
use serde_json::{Value, json};
use worker::*;

use crate::SiteEnv;

/// Chains audited per run [guess]: each costs one replay of up to `DIGESTS_KEEP` entries and one call per voter.
const SAMPLE: usize = 4;
/// The `Registry` object that holds this site's audit cursor.
const CURSOR: &str = "fold_audit";

fn digest(v: &Value) -> Option<Digest> {
    let b = crate::host::unhex(v.as_str()?)?;
    Some(Digest(b.try_into().ok()?))
}

/// Every repository chain whose registry row on this site names it first, the chain's home.
async fn homed(env: &Env, site: &SiteEnv) -> Result<Vec<RegistryRow>> {
    let mut out = Vec::new();
    for shard in 0..REGISTRY_SHARDS {
        let reg = MemberRegistry { site: site.id, shard };
        let stub = site.registry(env, site.id, &reg.do_name())?;
        let mut res = stub.fetch_with_str(&format!("https://registry/rows?site={}", site.id.0)).await?;
        let rows: Vec<RegistryRow> = res.json().await?;
        out.extend(
            rows.into_iter().filter(|r| matches!(r.chain, ChainId::Repo(_)) && r.sites.first() == Some(&site.id)),
        );
    }
    out.sort_by_key(|r| r.chain.to_string());
    Ok(out)
}

async fn cursor(env: &Env, site: &SiteEnv, put: Option<(&str, &Value)>) -> Result<Option<String>> {
    let stub = site.registry(env, site.id, CURSOR)?;
    let mut init = RequestInit::new();
    if let Some((next, last)) = put {
        init.with_method(Method::Put).with_body(Some(json!({ "next": next, "last": last }).to_string().into()));
    }
    let mut res = stub.fetch_with_request(Request::new_with_init("https://registry/fold_audit", &init)?).await?;
    let v: Value = res.json().await?;
    Ok(v["next"].as_str().map(str::to_owned))
}

/// One chain: the leader's replay, then each voter's digests at the replay's index, compared per tier.
async fn audit_chain(env: &Env, site: &SiteEnv, row: &RegistryRow) -> Value {
    let b = Bindings { env, sites: &site.sites };
    let record = PlacementRecord {
        name: if row.sites.len() == 1 {
            graftyard_types::PlacementName::Solo
        } else {
            graftyard_types::PlacementName::Tri
        },
        home: site.id,
        sites: row.sites.clone(),
        generation: row.generation,
        moving_from: None,
        at: graftyard_types::Timestamp(0),
    };
    let replay = match graftyard_lookup::call(&b, row.chain, &record, "/fold", &json!({ "replay": true })).await {
        Ok((v, _)) => v,
        Err(e) => return json!({ "chain": row.chain.to_string(), "error": e }),
    };
    if replay.get("skip").is_some() || replay.get("index").is_none() {
        return json!({ "chain": row.chain.to_string(), "replay": replay });
    }
    let index = Index(replay["index"].as_u64().unwrap_or(0));
    let mut members: [Vec<(graftyard_types::SiteId, Digest)>; 2] = [Vec::new(), Vec::new()];
    let mut missing = Vec::new();
    for s in &row.sites {
        let body = serde_json::to_vec(&json!({ "at": index.0 })).unwrap_or_default();
        let Some((200, v)) = graftyard_lookup::post(&b, *s, &row.do_name, "/fold", &body).await else {
            missing.push(json!({ "site": s.0, "error": "no answer" }));
            continue;
        };
        for (k, t) in ["core", "metadata"].iter().enumerate() {
            match digest(&v[t]["at"]) {
                Some(d) => members[k].push((*s, d)),
                // A member behind the index, or one whose metadata tier is still loading, cannot answer for it.
                None => missing
                    .push(json!({ "site": s.0, "tier": t, "applied": v[t]["applied"], "loaded": v[t]["loaded"] })),
            }
        }
    }
    let mut tiers = Vec::new();
    let mut alarm = false;
    for (k, (t, name)) in [(Tier::Core, "core"), (Tier::Metadata, "metadata")].into_iter().enumerate() {
        let Some(want) = digest(&replay[name]) else { continue };
        let r = fold_audit_compare(t, index, want, &members[k]);
        alarm |= r.alarm();
        tiers.push(json!({
            "tier": name,
            "compared": members[k].len(),
            "mismatches": r.mismatches.iter().map(|m| json!({ "site": m.member.0, "got": m.got.hex() })).collect::<Vec<_>>(),
        }));
    }
    if alarm {
        console_error!("fold audit mismatch on {} at {}: {tiers:?}", row.chain, index.0);
    }
    json!({ "chain": row.chain.to_string(), "index": index.0, "alarm": alarm, "tiers": tiers, "missing": missing })
}

/// One cron run: `SAMPLE` chains after the cursor, wrapping to the first.
pub async fn run(env: &Env) -> Result<Value> {
    let site = SiteEnv::read(env)?;
    let all = homed(env, &site).await?;
    let from = cursor(env, &site, None).await?;
    let start = from.as_ref().map_or(0, |f| all.iter().position(|r| r.chain.to_string() > *f).unwrap_or(0));
    let picked: Vec<&RegistryRow> = all.iter().cycle().skip(start).take(SAMPLE.min(all.len())).collect();
    let mut chains = Vec::new();
    for r in &picked {
        chains.push(audit_chain(env, &site, r).await);
    }
    let next = picked.last().map(|r| r.chain.to_string()).unwrap_or_default();
    let report = json!({
        "site": site.id.0,
        "homed": all.len(),
        "alarms": chains.iter().filter(|c| c["alarm"] == json!(true)).count(),
        "chains": chains,
        "at": crate::now_ms(),
    });
    cursor(env, &site, Some((&next, &report))).await?;
    Ok(report)
}
