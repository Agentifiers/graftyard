//! `MemberRegistry`: one object per (site, shard) on each site script. A member writes its row before it stores its
//! first entry and rewrites it on each config change. The owner-placement move enumerates groups from the rows whose
//! `sites` include a site, and `ReplaceMember` from every row (`/all_rows`, `crate::replace`) (`design/DESIGN.md`,
//! Shape, the registry paragraph).

use graftyard_types::{ChainId, RegistryRow, RowPut, SiteId};
use serde::Deserialize;
use worker::*;

#[durable_object(alarm)]
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
            // A member's own write (a bare `RegistryRow`) never moves the row to a lower generation, so a removed
            // member's late write changes nothing. A re-placement's restore (`RowPut` with `replacing`) puts back the
            // kept member's row over a try's, and only a try's (`RowPut::replaces`). One object serves one request at
            // a time and nothing awaits between the read and the write, so the check and the write are one step.
            (Method::Put, "/row") => {
                let body: serde_json::Value = req.json().await?;
                let put: RowPut = if body.get("row").is_some() {
                    serde_json::from_value(body)?
                } else {
                    RowPut { row: serde_json::from_value(body)?, replacing: Vec::new() }
                };
                let chain = put.row.chain.to_string();
                let rows: Vec<Row> =
                    sql.exec("SELECT row FROM registry WHERE chain = ?", vec![chain.clone().into()])?.to_array()?;
                let current = rows.first().map(|r| serde_json::from_str::<RegistryRow>(&r.row)).transpose()?;
                let replaced = put.replaces(current.as_ref());
                if replaced {
                    sql.exec(
                        "INSERT OR REPLACE INTO registry (chain, generation, row) VALUES (?, ?, ?)",
                        vec![
                            chain.into(),
                            i64::from(put.row.generation).into(),
                            serde_json::to_string(&put.row)?.into(),
                        ],
                    )?;
                }
                Response::from_json(&serde_json::json!({ "replaced": replaced }))
            }
            // A deleted repository's purge (`graftyard_git::purge`), after it retired the member: the row goes only if it
            // still names `do_name`, so a later generation's row, written after the purge read it, stays.
            (Method::Delete, "/row") => {
                let chain = query(&url, "chain")?;
                let name = query(&url, "do_name")?;
                let rows: Vec<Row> =
                    sql.exec("SELECT row FROM registry WHERE chain = ?", vec![chain.clone().into()])?.to_array()?;
                let named = rows
                    .first()
                    .map(|r| serde_json::from_str::<RegistryRow>(&r.row))
                    .transpose()?
                    .is_some_and(|r| *r.do_name == *name);
                if named {
                    sql.exec("DELETE FROM registry WHERE chain = ?", vec![chain.into()])?;
                }
                Response::from_json(&serde_json::json!({ "deleted": named }))
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
            // The copy of the `Release` group's committed entries outside the group, in the `Release` chain's own
            // registry object on each site (`design/DESIGN.md`, Shape, `POINTER_TTL`): each signed entry's bytes and
            // time by position, as the members applied them. A PUT `{"from": n, "entries": [[hex, at], ..]}` adds the
            // entries at positions from `n` that the copy lacks, and changes nothing when `n` is past the copy (a gap),
            // so the copy only grows and never moves backward; it answers the copy's count. A GET answers
            // `{"count": n, "entries": [[hex, at], ..]}`. A cold isolate checks every entry against its own anchor.
            (Method::Put, "/release_copy") | (Method::Get, "/release_copy") => {
                sql.exec(
                    "CREATE TABLE IF NOT EXISTS release_copy (idx INTEGER PRIMARY KEY, bytes BLOB NOT NULL, at INTEGER NOT NULL)",
                    None,
                )?;
                #[derive(Deserialize)]
                struct N {
                    n: i64,
                }
                let count = |sql: &SqlStorage| -> Result<i64> {
                    let r: Vec<N> = sql.exec("SELECT COUNT(*) AS n FROM release_copy", None)?.to_array()?;
                    Ok(r.first().map_or(0, |r| r.n))
                };
                if req.method() == Method::Put {
                    #[derive(Deserialize)]
                    struct Put {
                        from: i64,
                        entries: Vec<(String, i64)>,
                    }
                    let p: Put = req.json().await?;
                    if p.from <= count(&sql)? {
                        for (k, (hex, at)) in p.entries.iter().enumerate() {
                            let bytes = crate::host::unhex(hex).ok_or_else(|| Error::RustError("entry: hex".into()))?;
                            sql.exec(
                                "INSERT OR IGNORE INTO release_copy (idx, bytes, at) VALUES (?, ?, ?)",
                                vec![(p.from + k as i64).into(), bytes.into(), (*at).into()],
                            )?;
                        }
                    }
                    return Response::from_json(&serde_json::json!({ "count": count(&sql)? }));
                }
                let rows = crate::host::rows(&sql, "SELECT bytes, at FROM release_copy ORDER BY idx", vec![])?;
                let entries = rows
                    .iter()
                    .map(|r| {
                        Ok(serde_json::json!([crate::host::hex(crate::host::blob(&r[0])?), crate::host::int(&r[1])?]))
                    })
                    .collect::<Result<Vec<_>>>()?;
                Response::from_json(&serde_json::json!({ "count": entries.len(), "entries": entries }))
            }
            // M6-WP1 (`crate::replace`): every row of this shard, and the refill of a spare's bucket.
            (Method::Get, "/all_rows") => Response::from_json(&crate::replace::all_rows(&sql)?),
            (Method::Post, "/refill") => {
                let body: serde_json::Value = req.json().await?;
                Response::from_json(&crate::replace::refill(&self.env, body).await?)
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
            // M5-WP4's gate counters and `SiteHealth`, and `graft-front`'s `GATE_ERRORS` counts (`crate::gatecount`),
            // in the objects `gate`, `site_health` and `errors` on this site, which `graft-ops` reads over its
            // `REGISTRY_<n>` bindings.
            (m @ (Method::Put | Method::Get), p @ ("/gate" | "/site_health" | "/errors")) => {
                let body = if m == Method::Put { Some(req.json::<serde_json::Value>().await?) } else { None };
                // M6-WP1's drained-site probe: `graft-ops`'s read names the lost and replaced sites (`crate::replace`).
                if let (Method::Get, "/site_health", Ok(q)) = (&m, p, query(&url, "probe")) {
                    self.probe_until(&sql, &q).await?;
                }
                Response::from_json(&crate::gatecount::route(&sql, &p[1..], body, &|k| query(&url, k).ok())?)
            }
            // The public halves of both SSH host-key slots on this site, `{"A": [{"type", "key", "fingerprint"}, ..],
            // "B": [..]}`, as deploy/ssh-front/hostkeys.mjs's public JSON lists them: the edge serves them at
            // /api/graftyard/v1/meta/ssh, and `graft-ops` counts the new slot's as the `SshHostKeys` `Rotate`'s probe,
            // over its `REGISTRY_<n>` bindings. No private byte leaves.
            (Method::Get, "/ssh_slots") => {
                let mut out = serde_json::Map::new();
                for slot in ["A", "B"] {
                    let value = match self.env.secret_store(&format!("SSH_HOST_KEY_{slot}")) {
                        Ok(s) => s.get().await.ok().flatten().unwrap_or_default(),
                        Err(_) => String::new(),
                    };
                    let keys = match ssh_public_keys(&value) {
                        Ok(k) => k,
                        Err(e) => return Response::error(format!("SSH_HOST_KEY_{slot}: {e}"), 500),
                    };
                    let keys: Vec<_> = keys
                        .iter()
                        .map(|k| serde_json::json!({ "type": k.kind, "key": k.key, "fingerprint": k.fingerprint }))
                        .collect();
                    out.insert(slot.into(), keys.into());
                }
                Response::from_json(&out)
            }
            // This site's `SshFront` (`ssh#<site>`, the name the edge opens) answering its `GET /status`:
            // `{"running", "image", "version_image", "banner"}` (workers/site/ssh.mjs, `status`), which never starts
            // the container. `graft-ops`' `SshHostKeys` probe reads it beside `/ssh_slots` and refuses a running
            // `graft-sshd` that reads the slot only at start (`workers/ops/src/lib.rs`, `ssh_fronts_ready`).
            (Method::Get, "/ssh_front") => {
                let id = crate::SiteEnv::read(&self.env)?.id;
                let Ok(ns) = self.env.durable_object(&format!("SSHFRONT_{}", id.0)) else {
                    return Response::error("this site binds no SSHFRONT: SSH is off", 404);
                };
                ns.get_by_name(&format!("ssh#{}", id.0))?.fetch_with_str("https://front/status").await
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
            // Test installs only: the change feed's hold, in the feed site's object `export`. `?hold=1` stops every
            // round claim, `?hold=0` lets them go again; either answers the state.
            #[cfg(feature = "test-faults")]
            (Method::Post, "/export_hold") => {
                if let Ok(v) = query(&url, "hold") {
                    sql.exec(crate::export_feed::HOLD_SCHEMA, None)?;
                    sql.exec(crate::export_feed::HOLD_PUT, vec![i64::from(v == "1").into()])?;
                }
                Response::from_json(&serde_json::json!({ "held": export_held(&sql)? }))
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
            // The change feed (`crate::export_feed`), held only by the feed site's object `export`: each leader's
            // newest commit report, kept only when higher (`export::report_newer`), and the last round's start, which
            // `export::round_due` paces. One object serves one request at a time, so a claim is one step.
            (Method::Put, "/export_report") => {
                #[derive(Deserialize)]
                struct Held {
                    idx: i64,
                }
                sql.exec(crate::export_feed::REPORTS_SCHEMA, None)?;
                let v: serde_json::Value = req.json().await?;
                let chain = v["chain"].as_str().unwrap_or_default().to_owned();
                let commit = graftyard_types::Index(v["commit"].as_u64().unwrap_or(0));
                let held: Vec<Held> =
                    sql.exec(crate::export_feed::REPORT_HELD, vec![chain.clone().into()])?.to_array()?;
                let newer = graftyard_git::export::report_newer(
                    held.first().map(|h| graftyard_types::Index(h.idx as u64)),
                    commit,
                );
                if newer {
                    sql.exec(
                        crate::export_feed::REPORT_PUT,
                        vec![chain.into(), (commit.0 as i64).into(), v["record"].to_string().into()],
                    )?;
                }
                Response::from_json(&serde_json::json!({ "newer": newer }))
            }
            (Method::Post, "/export_round") => {
                #[derive(Deserialize)]
                struct Last {
                    last: i64,
                }
                #[derive(Deserialize)]
                struct Report {
                    chain: String,
                    idx: i64,
                    record: String,
                }
                sql.exec(crate::export_feed::REPORTS_SCHEMA, None)?;
                sql.exec(crate::export_feed::ROUND_SCHEMA, None)?;
                // Test installs only: a held feed claims no round.
                #[cfg(feature = "test-faults")]
                if export_held(&sql)? {
                    return Response::from_json(&serde_json::json!({ "due": false, "held": true }));
                }
                let now = crate::now_ms();
                let last: Vec<Last> = sql.exec(crate::export_feed::ROUND_LAST, None)?.to_array()?;
                if !graftyard_git::export::round_due(last.first().map(|l| l.last as u64), now) {
                    return Response::from_json(&serde_json::json!({ "due": false }));
                }
                sql.exec(crate::export_feed::ROUND_PUT, vec![(now as i64).into()])?;
                // The sweep's round number: this round's, then the next one's for the next claim.
                #[derive(Deserialize)]
                struct Sweep {
                    n: i64,
                }
                sql.exec(crate::export_feed::SWEEP_SCHEMA, None)?;
                let sweep: Vec<Sweep> = sql.exec(crate::export_feed::SWEEP_N, None)?.to_array()?;
                let sweep = sweep.first().map_or(0, |s| s.n);
                sql.exec(crate::export_feed::SWEEP_PUT, vec![(sweep + 1).into()])?;
                let reports: Vec<Report> = sql.exec(crate::export_feed::REPORTS_ALL, None)?.to_array()?;
                let reports: Vec<serde_json::Value> = reports
                    .into_iter()
                    .map(|r| {
                        let record: serde_json::Value = serde_json::from_str(&r.record).unwrap_or_default();
                        serde_json::json!({ "chain": r.chain, "commit": r.idx, "record": record })
                    })
                    .collect();
                Response::from_json(&serde_json::json!({ "due": true, "reports": reports, "sweep": sweep }))
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
                #[derive(Deserialize)]
                struct H {
                    network: i64,
                    site: i64,
                    at: i64,
                }
                sweep_homes(&sql)?;
                let homes: Vec<H> = sql.exec("SELECT network, site, at FROM sweep_home", None)?.to_array()?;
                let homes: Vec<[i64; 3]> = homes.iter().map(|h| [h.network, h.site, h.at]).collect();
                Response::from_json(
                    &serde_json::json!({ "next": rows.first().and_then(|r| r.next), "last": last, "homes": homes }),
                )
            }
            // The sweeper's cached network homes (`crate::sweep::homes`): `[network, site, at]` rows to store, and
            // networks no bucket holds any more to drop.
            (Method::Put, "/sweep_homes") => {
                let v: serde_json::Value = req.json().await?;
                sweep_homes(&sql)?;
                for h in v["homes"].as_array().into_iter().flatten() {
                    sql.exec(
                        "INSERT OR REPLACE INTO sweep_home (network, site, at) VALUES (?, ?, ?)",
                        vec![h[0].as_i64().into(), h[1].as_i64().into(), h[2].as_i64().into()],
                    )?;
                }
                for n in v["gone"].as_array().into_iter().flatten() {
                    sql.exec("DELETE FROM sweep_home WHERE network = ?", vec![n.as_i64().into()])?;
                }
                Response::from_json(&serde_json::json!({ "ok": true }))
            }
            _ => Response::error("not found", 404),
        }
    }

    /// The drained-site probe (`crate::replace::probe`): one round, then again in `PROBE_EVERY_MS` while the last
    /// `graft-ops` read named a site and is under `PROBE_HOLD_MS` old.
    async fn alarm(&self) -> Result<Response> {
        if crate::site_down(&self.env) {
            return Response::empty();
        }
        let sql = self.state.storage().sql();
        let Some((sites, until)) = probe_row(&sql)? else { return Response::empty() };
        if sites.is_empty() || crate::now_ms() >= until {
            return Response::empty();
        }
        let counts = crate::replace::probe(&self.env, &sites).await?;
        crate::gatecount::route(&sql, "site_health", Some(counts), &|_| None)?;
        self.state.storage().set_alarm(crate::replace::PROBE_EVERY_MS as i64).await?;
        Response::empty()
    }
}

impl Registry {
    /// Stores the sites `graft-ops` named and how long to probe them, and starts the alarm if none is set.
    async fn probe_until(&self, sql: &SqlStorage, q: &str) -> Result<()> {
        let me = crate::SiteEnv::read(&self.env)?.id;
        let sites = crate::replace::probe_sites(q, me);
        let csv = sites.iter().map(u8::to_string).collect::<Vec<_>>().join(",");
        let until = crate::now_ms() + crate::replace::PROBE_HOLD_MS;
        sql.exec(
            "CREATE TABLE IF NOT EXISTS probe (k INTEGER PRIMARY KEY CHECK (k = 0), sites TEXT NOT NULL, until INTEGER NOT NULL)",
            None,
        )?;
        sql.exec(
            "INSERT OR REPLACE INTO probe (k, sites, until) VALUES (0, ?, ?)",
            vec![csv.into(), (until as i64).into()],
        )?;
        if !sites.is_empty() && self.state.storage().get_alarm().await?.is_none() {
            self.state.storage().set_alarm(crate::replace::PROBE_EVERY_MS as i64).await?;
        }
        Ok(())
    }
}

/// The probe's stored sites and end, if a `graft-ops` read named any.
fn probe_row(sql: &SqlStorage) -> Result<Option<(Vec<u8>, u64)>> {
    sql.exec(
        "CREATE TABLE IF NOT EXISTS probe (k INTEGER PRIMARY KEY CHECK (k = 0), sites TEXT NOT NULL, until INTEGER NOT NULL)",
        None,
    )?;
    let rows = crate::host::rows(sql, "SELECT sites, until FROM probe WHERE k = 0", vec![])?;
    let Some(r) = rows.first() else { return Ok(None) };
    let sites = crate::host::text(&r[0])?.split(',').filter_map(|x| x.parse().ok()).collect();
    Ok(Some((sites, crate::host::int(&r[1])? as u64)))
}

/// The sweeper's home cache table, created on first use.
fn sweep_homes(sql: &SqlStorage) -> Result<()> {
    sql.exec(
        "CREATE TABLE IF NOT EXISTS sweep_home (network INTEGER PRIMARY KEY, site INTEGER NOT NULL, at INTEGER NOT NULL)",
        None,
    )?;
    Ok(())
}

/// Test installs only: whether the change feed is held (`/export_hold`).
#[cfg(feature = "test-faults")]
fn export_held(sql: &SqlStorage) -> Result<bool> {
    #[derive(Deserialize)]
    struct Held {
        held: i64,
    }
    sql.exec(crate::export_feed::HOLD_SCHEMA, None)?;
    let rows: Vec<Held> = sql.exec(crate::export_feed::HOLD_GET, None)?.to_array()?;
    Ok(rows.first().is_some_and(|h| h.held == 1))
}

pub(crate) fn query(url: &Url, key: &str) -> Result<String> {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .ok_or_else(|| Error::RustError(format!("missing ?{key}")))
}

/// One host key's public half, as `ssh-keygen` prints it: its type (`ssh-ed25519`), `"<type> <base64 blob>"`, and
/// `SHA256:` with the unpadded base64 of the blob's SHA-256.
#[derive(Debug, PartialEq, Eq)]
struct SshPublicKey {
    kind: String,
    key: String,
    fingerprint: String,
}

const OPENSSH_BEGIN: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";
const OPENSSH_END: &str = "-----END OPENSSH PRIVATE KEY-----";

/// The public half of every OpenSSH private key PEM block in a host-key slot's value (deploy/ssh-front/hostkeys.mjs
/// writes three), read from each block's unencrypted header: the `openssh-key-v1\0` magic, the cipher, KDF, and KDF
/// options strings, the key count, then the first public key. An error names what was wrong, never a key's bytes.
fn ssh_public_keys(value: &str) -> std::result::Result<Vec<SshPublicKey>, String> {
    use base64::Engine as _;
    use sha2::Digest as _;
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(i) = rest.find(OPENSSH_BEGIN) {
        let body = &rest[i + OPENSSH_BEGIN.len()..];
        let end = body.find(OPENSSH_END).ok_or("a PEM block has no end line")?;
        rest = &body[end + OPENSSH_END.len()..];
        let text: String = body[..end].chars().filter(|c| !c.is_ascii_whitespace()).collect();
        let raw = base64::engine::general_purpose::STANDARD.decode(text).map_err(|_| "a PEM block is not base64")?;
        let blob = openssh_public_blob(&raw).ok_or("a PEM block is not an openssh-key-v1 key")?;
        let kind = ssh_string(blob, &mut 0).and_then(|t| std::str::from_utf8(t).ok()).ok_or("a key has no type")?;
        let b64 = base64::engine::general_purpose::STANDARD;
        out.push(SshPublicKey {
            kind: kind.to_owned(),
            key: format!("{kind} {}", b64.encode(blob)),
            fingerprint: format!(
                "SHA256:{}",
                base64::engine::general_purpose::STANDARD_NO_PAD.encode(sha2::Sha256::digest(blob))
            ),
        });
    }
    Ok(out)
}

/// The first public key blob of a decoded `openssh-key-v1` key, `None` when the bytes are not one.
fn openssh_public_blob(raw: &[u8]) -> Option<&[u8]> {
    let mut at = b"openssh-key-v1\0".len();
    if raw.get(..at)? != b"openssh-key-v1\0" {
        return None;
    }
    for _ in 0..3 {
        ssh_string(raw, &mut at)?;
    }
    let count = u32::from_be_bytes(raw.get(at..at + 4)?.try_into().ok()?);
    at += 4;
    if count == 0 {
        return None;
    }
    ssh_string(raw, &mut at)
}

/// The SSH `string` (a big-endian u32 length, then the bytes) at `*at`, moving `*at` past it.
fn ssh_string<'a>(b: &'a [u8], at: &mut usize) -> Option<&'a [u8]> {
    let n = u32::from_be_bytes(b.get(*at..*at + 4)?.try_into().ok()?) as usize;
    let s = b.get(*at + 4..(*at + 4).checked_add(n)?)?;
    *at += 4 + n;
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::{SshPublicKey, ssh_public_keys};

    /// A test-only Ed25519 key `ssh-keygen -t ed25519 -N "" -C ""` made, and what its `.pub` and
    /// `ssh-keygen -l -E sha256` print.
    const PEM: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACDeZpHRQEtEkpLwgYPbpdEqialwWeThXtwWZrJBCEuZAwAAAIhNRNfPTUTX\nzwAAAAtzc2gtZWQyNTUxOQAAACDeZpHRQEtEkpLwgYPbpdEqialwWeThXtwWZrJBCEuZAw\nAAAEC3S44+BxDU/7zBw1gHB/Z8H/W6UfPhFjmpiPYkc7xtWt5mkdFAS0SSkvCBg9ul0SqJ\nqXBZ5OFe3BZmskEIS5kDAAAAAAECAwQF\n-----END OPENSSH PRIVATE KEY-----\n";
    const PUB: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIN5mkdFAS0SSkvCBg9ul0SqJqXBZ5OFe3BZmskEIS5kD";
    const FINGERPRINT: &str = "SHA256:CKea7oY7Ys3mkLc347XrzupVUKhVSeA9vn7EU3GMA2U";

    /// A slot's concatenated keys give each key's public half as hostkeys.mjs lists it; an empty or unbound slot and
    /// other PEM blocks give none.
    #[test]
    fn a_host_key_slot_gives_its_public_halves() {
        let one = SshPublicKey { kind: "ssh-ed25519".into(), key: PUB.into(), fingerprint: FINGERPRINT.into() };
        assert_eq!(ssh_public_keys(PEM), Ok(vec![one]));
        let three = ssh_public_keys(&PEM.repeat(3)).unwrap();
        assert_eq!(three.len(), 3);
        assert!(three.iter().all(|k| k.key == PUB));
        assert_eq!(ssh_public_keys(""), Ok(vec![]));
        assert_eq!(
            ssh_public_keys("-----BEGIN RSA PRIVATE KEY-----\nAAAA\n-----END RSA PRIVATE KEY-----\n"),
            Ok(vec![])
        );
    }

    /// A block whose bytes are not `openssh-key-v1` is refused, and the error carries none of them.
    #[test]
    fn a_block_with_a_bad_magic_is_refused() {
        use base64::Engine as _;
        let b = |raw: &[u8]| {
            let body = base64::engine::general_purpose::STANDARD.encode(raw);
            format!("{}\n{body}\n{}\n", super::OPENSSH_BEGIN, super::OPENSSH_END)
        };
        let e = ssh_public_keys(&b(b"openssh-key-v2\0\0\0\0\x04none")).unwrap_err();
        assert_eq!(e, "a PEM block is not an openssh-key-v1 key");
        assert!(ssh_public_keys(&b(b"openssh-key-v1\0\0\0")).is_err(), "a truncated header");
        let tampered = PEM.replacen("b3BlbnNzaC1rZXktdjE", "b3BlbnNzaC1rZXktdjI", 1);
        assert!(ssh_public_keys(&tampered).is_err());
        assert!(ssh_public_keys(&format!("{}\nAAAA\n", super::OPENSSH_BEGIN)).is_err(), "no end line");
    }
}
