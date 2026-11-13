//! `MemberRegistry`: one object per (site, shard) on each site script. A member writes its row before it stores its
//! first entry and rewrites it on each config change. `ReplaceMember` and the owner-placement move enumerate groups
//! from the rows whose `sites` include a site (`design/DESIGN.md`, Shape, the registry paragraph).

use graftyard_types::{ChainId, RegistryRow, SiteId};
use serde::Deserialize;
use worker::*;

#[durable_object(fetch)]
pub struct Registry {
    state: State,
    env: Env,
}

#[derive(Deserialize)]
struct Row {
    row: String,
}

impl DurableObject for Registry {
    fn new(state: State, env: Env) -> Self {
        state
            .storage()
            .sql()
            .exec("CREATE TABLE IF NOT EXISTS registry (chain TEXT PRIMARY KEY, generation INTEGER NOT NULL, row TEXT NOT NULL)", None)
            .expect("registry schema");
        state
            .storage()
            .sql()
            .exec("CREATE TABLE IF NOT EXISTS claim (chain TEXT PRIMARY KEY, generation INTEGER NOT NULL)", None)
            .expect("claim schema");
        Self { state, env }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if crate::site_down(&self.env) {
            return Response::error("site down", 503);
        }
        let sql = self.state.storage().sql();
        let url = req.url()?;
        match (req.method(), url.path()) {
            // A row with a lower generation than the stored one is stale (a removed member's late write) and changes
            // nothing, so a row never moves backwards.
            (Method::Put, "/row") => {
                let row: RegistryRow = req.json().await?;
                sql.exec(
                    "INSERT INTO registry (chain, generation, row) VALUES (?, ?, ?)
                     ON CONFLICT (chain) DO UPDATE SET generation = excluded.generation, row = excluded.row
                     WHERE excluded.generation >= registry.generation",
                    vec![
                        row.chain.to_string().into(),
                        i64::from(row.generation).into(),
                        serde_json::to_string(&row)?.into(),
                    ],
                )?;
                Response::empty()
            }
            (Method::Get, "/row") => {
                let chain: ChainId = query(&url, "chain")?.parse().map_err(|e| Error::RustError(format!("{e}")))?;
                let rows: Vec<Row> =
                    sql.exec("SELECT row FROM registry WHERE chain = ?", vec![chain.to_string().into()])?.to_array()?;
                match rows.first() {
                    Some(r) => Response::from_json(&serde_json::from_str::<RegistryRow>(&r.row)?),
                    None => Response::error("no row", 404),
                }
            }
            // Which record generation bootstraps a chain: the first to claim it. Every bootstrap member claims on the
            // registry of the oldest record in its record's `lookup_order`, before it stores the bootstrap entry, so
            // edges holding two generations of an owner's record during a move build one group, not two. Not in the
            // design (see `tests/m1/results/m1-wp7.md`, "On Durable Objects").
            (Method::Post, "/claim") => {
                #[derive(Deserialize)]
                struct Claim {
                    chain: ChainId,
                    generation: u32,
                }
                let c: Claim = req.json().await?;
                sql.exec(
                    "INSERT INTO claim (chain, generation) VALUES (?, ?) ON CONFLICT (chain) DO NOTHING",
                    vec![c.chain.to_string().into(), i64::from(c.generation).into()],
                )?;
                #[derive(Deserialize)]
                struct G {
                    generation: i64,
                }
                let rows: Vec<G> = sql
                    .exec("SELECT generation FROM claim WHERE chain = ?", vec![c.chain.to_string().into()])?
                    .to_array()?;
                let g = rows.first().map_or(i64::from(c.generation), |r| r.generation);
                Response::from_json(&serde_json::json!({ "generation": g }))
            }
            // `crate::search`'s generation map, in the object `search_map`: the newest generation of each (domain,
            // shard). A lower generation than the stored one changes nothing, so the map only moves forward.
            (Method::Get, "/search_map") | (Method::Put, "/search_map") => {
                sql.exec(
                    "CREATE TABLE IF NOT EXISTS search_map (k TEXT PRIMARY KEY, generation INTEGER NOT NULL)",
                    None,
                )?;
                if req.method() == Method::Put {
                    #[derive(Deserialize)]
                    struct Put {
                        key: String,
                        generation: i64,
                    }
                    let p: Put = req.json().await?;
                    sql.exec(
                        "INSERT INTO search_map (k, generation) VALUES (?, ?) ON CONFLICT (k) DO UPDATE SET \
                         generation = excluded.generation WHERE excluded.generation > search_map.generation",
                        vec![p.key.into(), p.generation.into()],
                    )?;
                }
                #[derive(Deserialize)]
                struct G {
                    k: String,
                    generation: i64,
                }
                let rows: Vec<G> = sql.exec("SELECT k, generation FROM search_map", None)?.to_array()?;
                let m: std::collections::BTreeMap<String, i64> =
                    rows.into_iter().map(|g| (g.k, g.generation)).collect();
                Response::from_json(&m)
            }
            (Method::Get, "/rows") => {
                let site = SiteId(query(&url, "site")?.parse().map_err(|_| Error::RustError("site".into()))?);
                let rows: Vec<Row> = sql.exec("SELECT row FROM registry ORDER BY chain", None)?.to_array()?;
                let mut out = Vec::new();
                for r in rows {
                    let row: RegistryRow = serde_json::from_str(&r.row)?;
                    if row.sites.contains(&site) {
                        out.push(row);
                    }
                }
                Response::from_json(&out)
            }
            // Test installs only: BM-1's site fault plan (`crate::faults::SiteFault`), in the object `faults`.
            #[cfg(feature = "test-faults")]
            (Method::Get, "/site_fault") | (Method::Put, "/site_fault") => {
                #[derive(Deserialize)]
                struct Plan {
                    plan: String,
                }
                sql.exec(
                    "CREATE TABLE IF NOT EXISTS site_fault (k INTEGER PRIMARY KEY CHECK (k = 0), plan TEXT NOT NULL)",
                    None,
                )?;
                if req.method() == Method::Put {
                    let p: crate::faults::SiteFault = req.json().await?;
                    sql.exec(
                        "INSERT OR REPLACE INTO site_fault (k, plan) VALUES (0, ?)",
                        vec![serde_json::to_string(&p)?.into()],
                    )?;
                }
                let rows: Vec<Plan> = sql.exec("SELECT plan FROM site_fault WHERE k = 0", None)?.to_array()?;
                let p: crate::faults::SiteFault =
                    rows.first().map(|r| serde_json::from_str(&r.plan)).transpose()?.unwrap_or_default();
                Response::from_json(&p)
            }
            // Test installs only: B0-07's site modes (`crate::faults::SiteModes`), in the object `faults`.
            #[cfg(feature = "test-faults")]
            (Method::Get, "/site_modes") | (Method::Put, "/site_modes") => {
                #[derive(Deserialize)]
                struct Modes {
                    modes: String,
                }
                sql.exec(
                    "CREATE TABLE IF NOT EXISTS site_modes (k INTEGER PRIMARY KEY CHECK (k = 0), modes TEXT NOT NULL)",
                    None,
                )?;
                if req.method() == Method::Put {
                    let m: crate::faults::SiteModes = req.json().await?;
                    sql.exec(
                        "INSERT OR REPLACE INTO site_modes (k, modes) VALUES (0, ?)",
                        vec![serde_json::to_string(&m)?.into()],
                    )?;
                }
                let rows: Vec<Modes> = sql.exec("SELECT modes FROM site_modes WHERE k = 0", None)?.to_array()?;
                let m: crate::faults::SiteModes =
                    rows.first().map(|r| serde_json::from_str(&r.modes)).transpose()?.unwrap_or_default();
                Response::from_json(&m)
            }
            // Test installs only: B0-07's `SiteHealth` stand-in, in the object `health` on the observer's site. A PUT
            // adds `{"counts": [[target, calls, failed], ..]}` to the current second; a GET with `?since=` (epoch ms)
            // sums every second from then, per target. Seconds older than 10 minutes are dropped.
            #[cfg(feature = "test-faults")]
            (Method::Put, "/health") | (Method::Get, "/health") => {
                sql.exec(
                    "CREATE TABLE IF NOT EXISTS health (sec INTEGER NOT NULL, target INTEGER NOT NULL, calls INTEGER NOT NULL, failed INTEGER NOT NULL, PRIMARY KEY (sec, target))",
                    None,
                )?;
                let now = crate::now_ms() / 1000;
                if req.method() == Method::Put {
                    #[derive(Deserialize)]
                    struct Put {
                        counts: Vec<(u8, u64, u64)>,
                    }
                    let p: Put = req.json().await?;
                    for (t, c, f) in p.counts {
                        sql.exec(
                            "INSERT INTO health (sec, target, calls, failed) VALUES (?, ?, ?, ?)
                             ON CONFLICT (sec, target) DO UPDATE SET calls = calls + excluded.calls, failed = failed + excluded.failed",
                            vec![(now as i64).into(), i64::from(t).into(), (c as i64).into(), (f as i64).into()],
                        )?;
                    }
                    sql.exec("DELETE FROM health WHERE sec < ?", vec![(now as i64 - 600).into()])?;
                    return Response::empty();
                }
                #[derive(Deserialize, serde::Serialize)]
                struct Sum {
                    target: i64,
                    calls: i64,
                    failed: i64,
                }
                let since: i64 = query(&url, "since").ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0) / 1000;
                let rows: Vec<Sum> = sql
                    .exec(
                        "SELECT target, SUM(calls) AS calls, SUM(failed) AS failed FROM health WHERE sec >= ? GROUP BY target",
                        vec![since.into()],
                    )?
                    .to_array()?;
                Response::from_json(&rows)
            }
            // The sweeper cron's cursor (`crate::sweep`), held only by this site's object `sweep`: the network the next
            // run starts at (null: from the first), and the last run's report.
            // Test installs only: one sweeper run, the cron's own, with its clock at `?now=` ms (BF-19).
            #[cfg(feature = "test-faults")]
            (Method::Post, "/sweep_run") => {
                let now =
                    crate::registry::query(&url, "now").ok().and_then(|v| v.parse().ok()).unwrap_or(crate::now_ms());
                Response::from_json(&crate::sweep::run_at(&self.env, graftyard_types::Timestamp(now)).await?)
            }
            // The fold audit cron's cursor (`crate::fold_audit`), held only by this site's object `fold_audit`: the last
            // chain audited, and the last run's report.
            (Method::Get, "/fold_cursor") | (Method::Put, "/fold_cursor") => {
                #[derive(Deserialize)]
                struct Audit {
                    next: String,
                    last: String,
                }
                sql.exec(
                    "CREATE TABLE IF NOT EXISTS audit (k INTEGER PRIMARY KEY CHECK (k = 0), next TEXT NOT NULL, last TEXT NOT NULL)",
                    None,
                )?;
                if req.method() == Method::Put {
                    let v: serde_json::Value = req.json().await?;
                    sql.exec(
                        "INSERT OR REPLACE INTO audit (k, next, last) VALUES (0, ?, ?)",
                        vec![v["next"].as_str().unwrap_or("").into(), v["last"].to_string().into()],
                    )?;
                }
                let rows: Vec<Audit> = sql.exec("SELECT next, last FROM audit WHERE k = 0", None)?.to_array()?;
                let last = rows.first().map_or(serde_json::Value::Null, |r| {
                    serde_json::from_str(&r.last).unwrap_or(serde_json::Value::Null)
                });
                Response::from_json(&serde_json::json!({ "next": rows.first().map(|r| r.next.clone()), "last": last }))
            }
            (Method::Get, "/sweep") | (Method::Put, "/sweep") => {
                #[derive(Deserialize)]
                struct Sweep {
                    next: Option<i64>,
                    last: String,
                }
                sql.exec(
                    "CREATE TABLE IF NOT EXISTS sweep (k INTEGER PRIMARY KEY CHECK (k = 0), next INTEGER, last TEXT NOT NULL)",
                    None,
                )?;
                if req.method() == Method::Put {
                    let v: serde_json::Value = req.json().await?;
                    sql.exec(
                        "INSERT OR REPLACE INTO sweep (k, next, last) VALUES (0, ?, ?)",
                        vec![v["next"].as_i64().into(), v["last"].to_string().into()],
                    )?;
                }
                let rows: Vec<Sweep> = sql.exec("SELECT next, last FROM sweep WHERE k = 0", None)?.to_array()?;
                let last = rows.first().map_or(serde_json::Value::Null, |r| {
                    serde_json::from_str(&r.last).unwrap_or(serde_json::Value::Null)
                });
                Response::from_json(&serde_json::json!({ "next": rows.first().and_then(|r| r.next), "last": last }))
            }
            _ => Response::error("not found", 404),
        }
    }
}

pub(crate) fn query(url: &Url, key: &str) -> Result<String> {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .ok_or_else(|| Error::RustError(format!("missing ?{key}")))
}
