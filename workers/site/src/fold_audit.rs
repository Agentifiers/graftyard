//! The site script's fold audit on its Cron Trigger (`design/SHARED.md`, "Folds", `fold_audit`; `design/DESIGN.md`,
//! Stack, Cron Triggers; `decisions/DECISIONS.md`, "F48"). Each run takes `SAMPLE` repository chains this site homes,
//! from this site's `MemberRegistry` shards, starting after the last run's cursor. For each chain:
//!
//! - its leader refolds the chain from each tier's newest committed snapshot plus the log tail (`crate::load`), in
//!   alarm-driven batches, so the refold holds one batch in memory whatever the log's size. It writes no tier table:
//!   the snapshot is staged as pages and the tail applies over them (`decisions/DECISIONS.md`, "`refold-stream`"). A run
//!   starts the job (`/fold {"audit": {"start": true}}`) when the last one is a day old, and reads the last report;
//! - every voter reports its own `fold_digest` of each tier at the report's index from its digest ring
//!   (`/fold {"at": index}`), whatever batches it applied in;
//! - `fold_audit_compare` compares them, and any mismatch alarms. A member that cannot answer for the index (behind
//!   it, or its metadata tier still loading) is listed, not compared;
//! - a chain whose last refold matched every voter, and on which no voter holds an entry since that writes the fold,
//!   is `idle`: each voter's digests at that index are compared with the refold's again, with no refold and no lookup
//!   (so no election). A tombstoned repository is `deleted`, and neither refolded nor compared until its purge
//!   (`decisions/DECISIONS.md`, "audit-moved"). What the run found is marked on this site's member (`seen`).
//!
//! The report goes to the script's logs, and the alarms to `console_error`, with the cursor in this site's `Registry`
//! object `fold_audit`.

use graftyard_fold::fold_audit_compare;
use graftyard_lookup::Bindings;
use graftyard_types::{
    ChainId, DbId, Digest, Index, MemberRegistry, PlacementRecord, REGISTRY_SHARDS, RegistryRow, RepoId, Tier,
};
use serde_json::{Value, json};
use worker::*;

use crate::SiteEnv;

/// Chains audited per run [guess]: each costs one call to its leader and one per voter; the refold runs on the leader.
const SAMPLE: usize = 4;
/// The `Registry` object that holds this site's audit cursor.
const CURSOR: &str = "fold_audit";

fn digest(v: &Value) -> Option<Digest> {
    let b = crate::host::unhex(v.as_str()?)?;
    Some(Digest(b.try_into().ok()?))
}

/// Every repository chain whose registry row on this site names it first, the chain's home. `repo:0` is the CLI
/// latency probe's chain (id 0 is never assigned): its members join, never campaign, and hold no repository.
async fn homed(env: &Env, site: &SiteEnv) -> Result<Vec<RegistryRow>> {
    let mut out = Vec::new();
    for shard in 0..REGISTRY_SHARDS {
        let reg = MemberRegistry { site: site.id, shard };
        let stub = site.registry(env, site.id, &reg.do_name())?;
        let mut res = stub.fetch_with_str(&format!("https://registry/rows?site={}", site.id.0)).await?;
        let rows: Vec<RegistryRow> = res.json().await?;
        out.extend(rows.into_iter().filter(|r| auditable(r.chain) && r.sites.first() == Some(&site.id)));
    }
    out.sort_by_key(|r| r.chain.to_string());
    Ok(out)
}

fn auditable(chain: ChainId) -> bool {
    matches!(chain, ChainId::Repo(r) if r != PROBE)
}

/// The CLI latency probe's chain (`cli/src/sites.rs`, `PROBE_CHAIN`).
const PROBE: RepoId = RepoId(DbId(0));

async fn cursor(env: &Env, site: &SiteEnv, put: Option<(&str, &Value)>) -> Result<Option<String>> {
    let stub = site.registry(env, site.id, CURSOR)?;
    let mut init = RequestInit::new();
    if let Some((next, last)) = put {
        init.with_method(Method::Put).with_body(Some(json!({ "next": next, "last": last }).to_string().into()));
    }
    let mut res = stub.fetch_with_request(Request::new_with_init("https://registry/fold_cursor", &init)?).await?;
    let v: Value = res.json().await?;
    Ok(v["next"].as_str().map(str::to_owned))
}

/// One chain: the leader's last refold report (and a new refold started if it is due, or with `force`), then each
/// voter's digests at the report's index, compared per tier.
async fn audit_chain(env: &Env, site: &SiteEnv, row: &RegistryRow, force: bool) -> Value {
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
    // This site's own member first, once: a `repo:` member that hosts no repository fold (a test register) answers
    // 404, which the lookup would ask again for LOOKUP_FOR on every run (graft-m1, 2026-11-16: 21 `/fold` calls per
    // chain per run). It is skipped, not an error.
    let own = match graftyard_lookup::post(&b, site.id, &row.do_name, "/fold", br#"{"audit":{}}"#).await {
        Some((404, _)) => return json!({ "chain": row.chain.to_string(), "skipped": "not a repository group" }),
        Some((200, v)) => v,
        _ => Value::Null,
    };
    // A tombstoned repository is not refolded or compared until its purge (`decisions/DECISIONS.md`, "audit-moved").
    if own["deleted"] == json!(true) && !force {
        return json!({ "chain": row.chain.to_string(), "deleted": true });
    }
    // A chain whose last refold matched every voter, and that has committed nothing since that writes its fold, is
    // not refolded: each voter's digests at that index are compared with the refold's again, and no lookup runs, so
    // no election starts on a leaderless chain. One whose last refold alarmed goes to its leader until one matches.
    let seen = &own["seen"];
    if seen["matched"] == json!(true) && !force {
        let index = seen["index"].as_u64().unwrap_or(0);
        let mut answers = Vec::new();
        for s in &row.sites {
            let Some(name) = member_name(env, site, *s, row.chain).await else { break };
            let body = serde_json::to_vec(&json!({ "at": index, "since": index })).unwrap_or_default();
            match graftyard_lookup::post(&b, *s, &name, "/fold", &body).await {
                Some((200, v)) => answers.push((*s, v)),
                _ => break,
            }
        }
        match idle(seen, &answers, row.sites.len()) {
            Idle::Moved => {}
            Idle::Matched => return json!({ "chain": row.chain.to_string(), "idle": true, "index": index }),
            Idle::Mismatch(tiers) => {
                console_error!("fold audit mismatch on {} at {index}, idle: {tiers:?}", row.chain);
                let mut v = seen.clone();
                v["matched"] = json!(false);
                let body = serde_json::to_vec(&json!({ "audit": { "seen": v } })).unwrap_or_default();
                let _ = graftyard_lookup::post(&b, site.id, &row.do_name, "/fold", &body).await;
                return json!({ "chain": row.chain.to_string(), "idle": true, "index": index, "alarm": true, "tiers": tiers });
            }
        }
    }
    let body = json!({ "audit": { "start": true, "force": force } });
    let led = match graftyard_lookup::call(&b, row.chain, &record, "/fold", &body).await {
        Ok((v, _)) => v,
        Err(e) => return json!({ "chain": row.chain.to_string(), "error": e }),
    };
    let replay = led["last"].clone();
    let index = Index(replay["index"].as_u64().unwrap_or(0));
    let running = &led["running"];
    if replay.get("error").is_some() || index == Index(0) {
        return json!({ "chain": row.chain.to_string(), "refold": replay, "running": running });
    }
    let mut members: [Vec<(graftyard_types::SiteId, Digest)>; 2] = [Vec::new(), Vec::new()];
    let mut missing = Vec::new();
    for s in &row.sites {
        // Each site's member has its own name, which that site's registry row holds.
        let Some(name) = member_name(env, site, *s, row.chain).await else {
            missing.push(json!({ "site": s.0, "error": "no registry row" }));
            continue;
        };
        let body = serde_json::to_vec(&json!({ "at": index.0 })).unwrap_or_default();
        let Some((200, v)) = graftyard_lookup::post(&b, *s, &name, "/fold", &body).await else {
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
    // Only a refold that every voter answered and matched opens the idle path; after an alarm, or with a voter that
    // could not answer, the chain goes through its leader again, which refolds it when due, until one matches.
    let matched = !alarm && missing.is_empty();
    let mark = json!({ "index": index.0, "core": replay["core"], "metadata": replay["metadata"], "matched": matched });
    let body = serde_json::to_vec(&json!({ "audit": { "seen": mark } })).unwrap_or_default();
    let _ = graftyard_lookup::post(&b, site.id, &row.do_name, "/fold", &body).await;
    json!({
        "chain": row.chain.to_string(), "index": index.0, "alarm": alarm, "tiers": tiers, "missing": missing,
        "refold": { "batches": replay["batches"], "entries": replay["entries"], "rows": replay["rows"], "bytes": replay["bytes"],
                    "ms": replay["ms"], "memMaxMb": replay["memMaxMb"], "from": replay["tiers"], "segments": replay["segments"],
                    "pages": replay["pages"], "saved": replay["saved"] },
        "running": running,
    })
}

/// What the idle path found from each voter's `/fold {"at": n, "since": n}` answer, against the refold marked at `n`.
#[derive(Debug, PartialEq)]
enum Idle {
    /// A voter holds an entry past the match that writes the fold, or could not answer: refold as usual.
    Moved,
    /// Every voter's digests at the match equal the match's.
    Matched,
    /// The tiers some voter's digest at the match differs on, as `audit_chain` reports them.
    Mismatch(Vec<Value>),
}

/// The idle path's verdict (`decisions/DECISIONS.md`, "audit-moved"). `answers` holds the voters that answered, of
/// `voters`; a voter that did not, or whose ring no longer holds the match's index, sends the chain to a refold, as
/// does an unloaded tier, which answers no digest.
fn idle(seen: &Value, answers: &[(graftyard_types::SiteId, Value)], voters: usize) -> Idle {
    if answers.len() < voters || answers.iter().any(|(_, v)| v["moved"] != json!(false)) {
        return Idle::Moved;
    }
    let mut tiers = Vec::new();
    for name in ["core", "metadata"] {
        let Some(want) = digest(&seen[name]) else { continue };
        let mut got = Vec::new();
        for (s, v) in answers {
            match digest(&v[name]["at"]) {
                Some(d) => got.push((*s, d)),
                None => return Idle::Moved,
            }
        }
        let t = if name == "core" { Tier::Core } else { Tier::Metadata };
        let r = fold_audit_compare(t, Index(seen["index"].as_u64().unwrap_or(0)), want, &got);
        if r.alarm() {
            tiers.push(json!({
                "tier": name,
                "compared": got.len(),
                "mismatches": r.mismatches.iter().map(|m| json!({ "site": m.member.0, "got": m.got.hex() })).collect::<Vec<_>>(),
            }));
        }
    }
    if tiers.is_empty() { Idle::Matched } else { Idle::Mismatch(tiers) }
}

/// The name of `chain`'s member on site `at`, from that site's registry row.
async fn member_name(env: &Env, site: &SiteEnv, at: graftyard_types::SiteId, chain: ChainId) -> Option<String> {
    let stub = site.registry(env, at, &MemberRegistry::for_chain(at, chain).do_name()).ok()?;
    let mut res = stub.fetch_with_str(&format!("https://registry/row?chain={chain}")).await.ok()?;
    if res.status_code() != 200 {
        return None;
    }
    Some(res.json::<RegistryRow>().await.ok()?.do_name.into())
}

/// One cron run: `SAMPLE` chains after the cursor, wrapping to the first. With `only`, that chain alone, if this site
/// homes it (test installs).
pub async fn run(env: &Env, only: Option<ChainId>, force: bool) -> Result<Value> {
    let site = SiteEnv::read(env)?;
    let mut all = homed(env, &site).await?;
    if let Some(c) = only {
        all.retain(|r| r.chain == c);
    }
    let from = cursor(env, &site, None).await?;
    let start = from.as_ref().map_or(0, |f| all.iter().position(|r| r.chain.to_string() > *f).unwrap_or(0));
    let picked: Vec<&RegistryRow> = all.iter().cycle().skip(start).take(SAMPLE.min(all.len())).collect();
    let mut chains = Vec::new();
    for r in &picked {
        chains.push(audit_chain(env, &site, r, force).await);
    }
    let next = picked.last().map(|r| r.chain.to_string()).unwrap_or_default();
    let report = json!({
        "site": site.id.0,
        "homed": all.len(),
        "alarms": chains.iter().filter(|c| c["alarm"] == json!(true)).count(),
        "skipped": chains.iter().filter(|c| c.get("skipped").is_some()).count(),
        "idle": chains.iter().filter(|c| c["idle"] == json!(true)).count(),
        "deleted": chains.iter().filter(|c| c["deleted"] == json!(true)).count(),
        "chains": chains,
        "at": crate::now_ms(),
    });
    cursor(env, &site, Some((&next, &report))).await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use graftyard_types::{ChainId, DbId, RepoId};

    /// The audit samples repository chains only, and never the CLI latency probe's `repo:0`, whose members hold no
    /// repository (graft-m3f site c, 2026-11-16: its only homed chain, a 404 on every run).
    #[test]
    fn the_audit_skips_the_latency_probe_chain() {
        assert!(super::auditable(ChainId::Repo(RepoId(DbId(1)))));
        assert!(!super::auditable(ChainId::Repo(RepoId(DbId(0)))));
        assert!(!super::auditable(ChainId::Ids));
    }
}

#[cfg(test)]
mod idle_tests {
    use graftyard_types::SiteId;
    use serde_json::{Value, json};

    use super::{Idle, idle};

    const CORE: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const META: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const OTHER: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    fn seen() -> Value {
        json!({ "index": 2942, "core": CORE, "metadata": META, "matched": true })
    }

    fn answer(site: u8, core: Option<&str>, meta: Option<&str>, moved: bool) -> (SiteId, Value) {
        (SiteId(site), json!({ "core": { "at": core }, "metadata": { "at": meta }, "moved": moved }))
    }

    /// `decisions/DECISIONS.md`, "audit-moved": an unmoved chain whose voters all still hold the matched refold's
    /// digests at its index is idle, and costs no refold. Any voter that moved, could not answer, or no longer holds
    /// the index sends it to a refold; one whose digest at that index changed alarms on that tier and site.
    #[test]
    fn an_unmoved_chain_is_compared_at_its_last_match_and_not_refolded() {
        let all = |m: bool| {
            vec![
                answer(0, Some(CORE), Some(META), false),
                answer(1, Some(CORE), Some(META), m),
                answer(2, Some(CORE), Some(META), false),
            ]
        };
        assert_eq!(idle(&seen(), &all(false), 3), Idle::Matched);
        assert_eq!(idle(&seen(), &all(true), 3), Idle::Moved, "site 1 committed an events entry since");
        assert_eq!(idle(&seen(), &all(false)[..2], 3), Idle::Moved, "site 2 did not answer");
        let lost = vec![
            answer(0, Some(CORE), Some(META), false),
            answer(1, None, Some(META), false),
            answer(2, Some(CORE), Some(META), false),
        ];
        assert_eq!(
            idle(&seen(), &lost, 3),
            Idle::Moved,
            "site 1's ring no longer holds the index, or its tier is loading"
        );
        let changed = vec![
            answer(0, Some(CORE), Some(META), false),
            answer(1, Some(CORE), Some(META), false),
            answer(2, Some(CORE), Some(OTHER), false),
        ];
        let Idle::Mismatch(tiers) = idle(&seen(), &changed, 3) else { panic!("site 2's metadata digest changed") };
        assert_eq!(tiers.len(), 1);
        assert_eq!((tiers[0]["tier"].as_str(), &tiers[0]["mismatches"][0]["site"]), (Some("metadata"), &json!(2)));
    }
}
