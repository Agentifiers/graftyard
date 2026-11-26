//! The site script's fold audit on its Cron Trigger (`design/SHARED.md`, "Folds", `fold_audit`; `design/DESIGN.md`,
//! Stack, Cron Triggers; `decisions/DECISIONS.md`, "F48"). Each run audits `SAMPLE` repository chains this site homes,
//! from this site's `MemberRegistry` shards, starting after the last run's cursor. A row whose chain no member joins
//! (`retired`), a tombstoned one, or a probe costs a look and no sample, up to `LOOK_MAX` looks a run
//! (`decisions/DECISIONS.md`, "`audit-cover`"). For each chain:
//!
//! - its leader refolds the chain from each tier's newest committed snapshot plus the log tail (`crate::load`), in
//!   alarm-driven batches, so the refold holds one batch in memory whatever the log's size. It writes no tier table:
//!   the snapshot is staged as pages and the tail applies over them (`decisions/DECISIONS.md`, "`refold-stream`"). A run
//!   starts the job (`/fold {"audit": {"start": true}}`) when the last one is a day old and the chain wrote its fold
//!   since (`decisions/DECISIONS.md`, "`idle-stall`"), and reads the last report;
//! - every voter reports its own `fold_digest` of each tier at the report's index from its digest ring
//!   (`/fold {"at": index}`), whatever batches it applied in;
//! - `fold_audit_compare` compares them, and any mismatch alarms. A member that cannot answer for the index (behind
//!   it, or its metadata tier still loading) is listed, not compared; one whose tier is only not loaded still lets
//!   the chain match;
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
/// Rows one run looks at, at most [guess]. A row that is retired, deleted, or not a repository group costs a look and
/// no sample (one call to this site's member, and for a retired one a registry read and a call per other site), so a
/// site whose rows are mostly those still audits `SAMPLE` live chains a run, and a run's calls stay bounded.
const LOOK_MAX: usize = 64;
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
        // Not joined here: a retired member whose registry row stayed, or a live group's lost home slot. Only the other
        // sites' members are asked, directly, so a group none of them holds starts no lookup and no election.
        Some((409, _)) => {
            if !joined_elsewhere(env, site, row).await {
                return json!({ "chain": row.chain.to_string(), "retired": true });
            }
            Value::Null
        }
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
            let body = serde_json::to_vec(&json!({ "at": index, "since": index, "live": true })).unwrap_or_default();
            match graftyard_lookup::post(&b, *s, &name, "/fold", &body).await {
                Some((200, v)) => answers.push((*s, v)),
                _ => break,
            }
        }
        let live = live_report(row.chain, &answers);
        match idle(seen, &answers, row.sites.len()) {
            Idle::Moved => {}
            Idle::Matched => {
                return json!({ "chain": row.chain.to_string(), "idle": true, "index": index, "live": live });
            }
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
    let mut answers = Vec::new();
    for s in &row.sites {
        // Each site's member has its own name, which that site's registry row holds.
        let Some(name) = member_name(env, site, *s, row.chain).await else {
            missing.push(json!({ "site": s.0, "error": "no registry row" }));
            continue;
        };
        let body = serde_json::to_vec(&json!({ "at": index.0, "live": true })).unwrap_or_default();
        let Some((200, v)) = graftyard_lookup::post(&b, *s, &name, "/fold", &body).await else {
            missing.push(json!({ "site": s.0, "error": "no answer" }));
            continue;
        };
        answers.push((*s, v.clone()));
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
    let matched = !alarm && missing.iter().all(unloaded);
    let mark = json!({ "index": index.0, "core": replay["core"], "metadata": replay["metadata"], "matched": matched });
    let body = serde_json::to_vec(&json!({ "audit": { "seen": mark } })).unwrap_or_default();
    let _ = graftyard_lookup::post(&b, site.id, &row.do_name, "/fold", &body).await;
    json!({
        "chain": row.chain.to_string(), "index": index.0, "alarm": alarm, "tiers": tiers, "missing": missing,
        "refold": { "batches": replay["batches"], "entries": replay["entries"], "rows": replay["rows"], "bytes": replay["bytes"],
                    "ms": replay["ms"], "memMaxMb": replay["memMaxMb"], "from": replay["tiers"], "segments": replay["segments"],
                    "pages": replay["pages"], "saved": replay["saved"] },
        "running": running,
        "live": live_report(row.chain, &answers),
    })
}

/// Each voter's last live-row check (`crate::live`), from its `/fold {"live": true}` answer: whether this visit
/// started one, and the tiers whose rows did not match the sum `apply` kept, which alarm. A mismatch is the
/// operator's to repair; it does not clear the refold's match, which reads no live row.
fn live_report(chain: ChainId, answers: &[(graftyard_types::SiteId, Value)]) -> Value {
    let mut alarms = Vec::new();
    let mut started = 0;
    for (s, v) in answers {
        started += usize::from(v["live"]["started"] == json!(true));
        for t in crate::live::mismatches(&v["live"]) {
            alarms.push(json!({ "site": s.0, "tier": t["tier"], "to": t["to"], "want": t["want"], "got": t["got"] }));
        }
    }
    if !alarms.is_empty() {
        console_error!("fold live-row mismatch on {chain}: {alarms:?}");
    }
    json!({ "started": started, "alarms": alarms })
}

/// A `missing` entry for a voter's tier that is not loaded (`crate::snap::tiers_json` answers it no digest). It does
/// not stop a match: the tier is compared once it loads, and until then each visit would send the chain back through
/// its leader (graft-m2b, 2026-11-26; `decisions/DECISIONS.md`, "`idle-stall`"). A voter with no registry row, one
/// that did not answer, and a loaded tier behind the index still do.
fn unloaded(m: &Value) -> bool {
    m.get("tier").is_some() && m["loaded"] == json!(false)
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
/// `voters`; a voter that did not, or whose ring no longer holds the match's index, sends the chain to a refold. A
/// voter's unloaded tier answers no digest and is left out of that tier's comparison, as `unloaded` leaves it out of
/// the match.
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
                None if v[name]["loaded"] == json!(false) => {}
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

/// The rows from `start`, wrapping, each at most once, until `SAMPLE` were audited or `LOOK_MAX` looked at
/// (`decisions/DECISIONS.md`, "`audit-cover`"). Answers each row's report and the index of the last row looked at,
/// the next run's cursor.
async fn walk<'a, R, F>(rows: &'a [R], start: usize, mut visit: impl FnMut(&'a R) -> F) -> (Vec<Value>, Option<usize>)
where
    F: std::future::Future<Output = Value>,
{
    let (mut out, mut last, mut audited) = (Vec::new(), None, 0);
    for k in 0..rows.len().min(LOOK_MAX) {
        if audited == SAMPLE {
            break;
        }
        let i = (start + k) % rows.len();
        let v = visit(&rows[i]).await;
        audited +=
            usize::from(!(v["retired"] == json!(true) || v["deleted"] == json!(true) || v.get("skipped").is_some()));
        out.push(v);
        last = Some(i);
    }
    (out, last)
}

/// Whether a member of `row`'s chain on another of its sites answers as joined.
async fn joined_elsewhere(env: &Env, site: &SiteEnv, row: &RegistryRow) -> bool {
    let b = Bindings { env, sites: &site.sites };
    for s in row.sites.iter().filter(|s| **s != site.id) {
        let Some(name) = member_name(env, site, *s, row.chain).await else { continue };
        if !matches!(graftyard_lookup::post(&b, *s, &name, "/fold", br#"{"audit":{}}"#).await, Some((409, _))) {
            return true;
        }
    }
    false
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
    let (chains, last) = walk(&all, start, |r| audit_chain(env, &site, r, force)).await;
    let next = last.map(|i| all[i].chain.to_string()).unwrap_or_default();
    let report = json!({
        "site": site.id.0,
        "homed": all.len(),
        "alarms": chains.iter().filter(|c| c["alarm"] == json!(true)).count(),
        "skipped": chains.iter().filter(|c| c.get("skipped").is_some()).count(),
        "idle": chains.iter().filter(|c| c["idle"] == json!(true)).count(),
        "deleted": chains.iter().filter(|c| c["deleted"] == json!(true)).count(),
        "retired": chains.iter().filter(|c| c["retired"] == json!(true)).count(),
        "liveStarted": chains.iter().filter_map(|c| c["live"]["started"].as_u64()).sum::<u64>(),
        "liveAlarms": chains.iter().filter_map(|c| c["live"]["alarms"].as_array().map(Vec::len)).sum::<usize>(),
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

    fn walk(rows: &[&str], start: usize) -> (Vec<String>, Option<usize>) {
        let visit = |r: &&str| {
            let v = match r.split_once(':').map_or(*r, |(k, _)| k) {
                "retired" => serde_json::json!({ "chain": r, "retired": true }),
                "deleted" => serde_json::json!({ "chain": r, "deleted": true }),
                "probe" => serde_json::json!({ "chain": r, "skipped": "not a repository group" }),
                _ => serde_json::json!({ "chain": r, "idle": true }),
            };
            std::future::ready(v)
        };
        let mut f = std::pin::pin!(super::walk(rows, start, visit));
        let std::task::Poll::Ready((out, last)) =
            f.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
        else {
            panic!("walk awaits only its visits")
        };
        (out.iter().map(|v| v["chain"].as_str().unwrap_or_default().to_owned()).collect(), last)
    }

    /// `decisions/DECISIONS.md`, "`audit-cover`": graft-m3f's six crons on 2026-11-18 spent every sample on retired
    /// test chains whose registry rows stayed. A run over mostly retired, deleted and probe rows still reaches the live
    /// chains past them, audits `SAMPLE` of them, and leaves its cursor on the last row it looked at.
    #[test]
    fn a_run_over_mostly_retired_rows_still_audits_the_live_ones() {
        let mut rows: Vec<String> = (0..30).map(|i| format!("retired:{i}")).collect();
        rows[3] = "deleted:3".into();
        rows[5] = "probe:5".into();
        for i in [12, 25, 26, 27, 29] {
            rows[i] = format!("live:{i}");
        }
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let (seen, last) = walk(&rows, 2);
        let live: Vec<&String> = seen.iter().filter(|c| c.starts_with("live")).collect();
        assert_eq!(live, ["live:12", "live:25", "live:26", "live:27"], "the first SAMPLE live rows from the cursor");
        assert_eq!((seen.first().map(String::as_str), last), (Some("retired:2"), Some(27)));
        // From the next cursor it wraps past the end: live:29, then live:12, live:25 and live:26 again.
        let (seen, last) = walk(&rows, 28);
        assert_eq!(seen.iter().filter(|c| c.starts_with("live")).count(), 4);
        assert_eq!(last, Some(26));
        // With no live row at all, a run looks at each row once, never past LOOK_MAX.
        let dead: Vec<&str> = vec!["retired:x"; 200];
        let (seen, last) = walk(&dead, 0);
        assert_eq!((seen.len(), last), (super::LOOK_MAX, Some(super::LOOK_MAX - 1)));
        let (seen, _) = walk(&dead[..7], 3);
        assert_eq!(seen.len(), 7, "a short site's rows once each, not cycled");
    }
}

#[cfg(test)]
mod idle_tests {
    use graftyard_types::SiteId;
    use serde_json::{Value, json};

    use super::{Idle, idle, unloaded};

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

    /// graft-m2b, 2026-11-26 (`decisions/DECISIONS.md`, "`idle-stall`"): a voter whose metadata tier is not loaded
    /// answers no digest for it. That keeps neither path from matching: the idle path compares the other voters and
    /// the loaded tier, and a refold whose only `missing` is such a tier opens the idle path. A loaded tier with no
    /// digest at the index (its ring passed it), a voter that did not answer, or one with no registry row still do.
    #[test]
    fn an_unloaded_tier_does_not_keep_a_chain_from_matching() {
        let cold = (
            SiteId(1),
            json!({ "core": { "at": CORE, "loaded": true }, "metadata": { "at": null, "loaded": false }, "moved": false }),
        );
        let behind = (
            SiteId(1),
            json!({ "core": { "at": CORE, "loaded": true }, "metadata": { "at": null, "loaded": true }, "moved": false }),
        );
        let three = |one: (SiteId, Value)| {
            vec![answer(0, Some(CORE), Some(META), false), one, answer(2, Some(CORE), Some(META), false)]
        };
        assert_eq!(idle(&seen(), &three(cold.clone()), 3), Idle::Matched);
        assert_eq!(idle(&seen(), &three(behind), 3), Idle::Moved, "a loaded tier past its ring");
        let mut wrong = three(cold);
        wrong[2] = answer(2, Some(CORE), Some(OTHER), false);
        assert!(matches!(idle(&seen(), &wrong, 3), Idle::Mismatch(_)), "site 2 still compared on metadata");
        // The led path's `missing` entries, as `audit_chain` builds them.
        assert!(unloaded(&json!({ "site": 1, "tier": "metadata", "applied": 7, "loaded": false })));
        assert!(!unloaded(&json!({ "site": 1, "tier": "metadata", "applied": 7, "loaded": true })), "behind the index");
        assert!(!unloaded(&json!({ "site": 1, "error": "no answer" })));
        assert!(!unloaded(&json!({ "site": 1, "error": "no registry row" })));
    }
}
