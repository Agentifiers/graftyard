//! The git cache shard class on a site script (`design/DESIGN.md`, Shape, "Git cache shards (derived)"): the M1-WP5
//! container behind `graftyard_git::host::ShardHost`, catching up behind its `IngestCursor` from the repository's
//! group before every call. The group is found by the lookup rule from the repository's owner (`RepoOwner` on
//! `Names`) and the owner's record, and read through its leader: `/read` for the core refs, `/entries` for the packs.
//!
//! Routes, each with `?repo=<id>` (a root repository's id is its network's): `POST /validate` (the tips, then the
//! pack), `GET /info-refs`, `POST /upload-pack`, `POST /catchup`, `GET /fsck`, `GET /status`, `POST /teardown`, and
//! `POST /promote` (a JSON list of ingested packs' SHA-256 hex: the home shard's half of `Promoted`), and
//! `POST /merge?base=<oid>&head=<oid>` (the `PrSync` consumer's test merge, `graftyard_git::prsync::MergeAnswer`), and
//! with `method=&heads=&ns=` `GitCacheShard::merge` for a real merge or a queue group
//! (`graftyard_git::merge_cache::ChainAnswer`).

use std::collections::BTreeSet;

use graftyard_git::group::RefsPage;
use graftyard_git::host::{R2, ShardHost};
use graftyard_git::{Caller, ChainLog, Head, LogPacks, PackId, Promoted, Serve, StoreError};
use graftyard_lookup::Bindings;
use graftyard_types::{ChainId, DbId, Index, NetworkId, PlacementRecord, RepoId, SiteId, Timestamp};
use serde_json::{Value, json};
use worker::*;

use crate::{SiteEnv, now_ms};

thread_local! {
    /// Per network, the catch-ups running on this isolate's shard objects, so a merge waits for one in flight
    /// (`merge_then_catch_up`'s `in_flight`). An object's requests share its isolate.
    ///
    /// Keyed by the shard object too: a hot-pool object of the network may share the isolate with its first shard,
    /// and each must catch up its own container.
    static CATCH_UPS: std::cell::RefCell<std::collections::HashMap<(String, i64), graftyard_git::merge_cache::CatchUps>> =
        Default::default();
    /// Per network, the log index the latest shared catch-up (`CatchUps::fresh`) reached, for `x-graft-cursor`.
    static CURSORS: std::cell::RefCell<std::collections::HashMap<(String, i64), u64>> = Default::default();
    /// Per network, the k its group's fold held at this isolate's latest catch-up (`ranges`' `fanout`), and when
    /// this isolate's first shard of it last reported `NetworkLoad` (`crate::fanout`).
    static FANOUT: std::cell::RefCell<std::collections::HashMap<i64, (u8, u64)>> = Default::default();
}

/// The k `net`'s group committed, as this isolate last read it; 1 before any read.
fn fanout_k(net: NetworkId) -> u8 {
    FANOUT.with(|m| m.borrow().get(&net.0.0).map_or(1, |(k, _)| *k))
}

/// This isolate's catch-up gate for network `net` on shard object `obj`.
fn catch_ups(obj: &str, net: NetworkId) -> graftyard_git::merge_cache::CatchUps {
    CATCH_UPS.with(|m| m.borrow_mut().entry((obj.to_owned(), net.0.0)).or_default().clone())
}

/// The shard container's instance type; every route builds its `ShardHost` with it.
const INSTANCE: &str = "standard-2";

#[durable_object]
pub struct GitShard {
    state: State,
    env: Env,
    /// TEST_MERGE_BUDGET of each network whose test merges run here, in this object's memory
    /// (`graftyard_git::prsync::Budgets`).
    budgets: std::cell::RefCell<graftyard_git::prsync::Budgets>,
}

struct GroupLog<'a> {
    env: &'a Env,
    sites: &'a graftyard_types::SiteTable,
    chain: ChainId,
    record: PlacementRecord,
}

impl GroupLog<'_> {
    async fn call(&self, path: &str, body: &Value) -> std::result::Result<Value, StoreError> {
        let b = Bindings { env: self.env, sites: self.sites };
        let (v, _) = graftyard_lookup::call(&b, self.chain, &self.record, path, body).await.map_err(StoreError)?;
        Ok(v)
    }

    /// The fold's last `Promoted` and `Archived` and its first log index, through `fresh`, with the read's index.
    async fn ranges(&self) -> std::result::Result<Value, StoreError> {
        let v = self.call("/read", &json!({ "q": { "ranges": true } })).await?;
        let mut r = v["value"].clone();
        if !r.is_object() {
            return Err(StoreError(format!("{}: ranges read answered {v}", self.chain)));
        }
        r["index"] = v["index"].clone();
        Ok(r)
    }
}

impl ChainLog for GroupLog<'_> {
    /// The fold's last `Archived` raises `first` (`RepoFold::first_index`); `applied` is the read's index.
    async fn head(&self, chain: ChainId) -> std::result::Result<Head, StoreError> {
        let v = self.ranges().await?;
        if let (ChainId::Repo(r), Some(k)) = (chain, v["fanout"].as_u64()) {
            FANOUT.with(|m| m.borrow_mut().entry(r.0.0).or_insert((1, 0)).0 = k as u8);
        }
        Ok(Head { first: Index(v["first"].as_u64().unwrap_or(1)), applied: Index(v["index"].as_u64().unwrap_or(0)) })
    }
    async fn refs_page(
        &self,
        _c: ChainId,
        after: Option<String>,
    ) -> std::result::Result<(RefsPage, Index), StoreError> {
        let q = graftyard_git::group::refs_query(b"", after.as_deref(), graftyard_git::repo::REFS_PAGE_MAX);
        let v = self.call("/read", &q).await?;
        let page: RefsPage = serde_json::from_value(v["value"].clone()).map_err(|e| {
            StoreError(format!(
                "{}: refs page: {e}: {}",
                self.chain,
                v.to_string().chars().take(200).collect::<String>()
            ))
        })?;
        Ok((page, Index(v["index"].as_u64().unwrap_or(0))))
    }
    /// Reads pages until `to` or an empty page. A newly elected leader may clamp `to` below the index a caller got
    /// from the previous leader, so the result may be short; `ShardHost` checks the complete range before applying it.
    async fn entries(&self, _c: ChainId, from: Index, to: Index) -> std::result::Result<Vec<LogPacks>, StoreError> {
        let mut out: Vec<LogPacks> = Vec::new();
        let mut at = from.0;
        while at <= to.0 {
            let v = self.call(&format!("/entries?from={at}&to={}", to.0), &json!({})).await?;
            let page: Vec<Value> =
                serde_json::from_value(v["entries"].clone()).map_err(|e| StoreError(e.to_string()))?;
            let Some(last) = page.last().and_then(|e| e["index"].as_u64()) else { break };
            for e in page {
                let promoted = e.get("promoted").cloned().unwrap_or(Value::Null);
                out.push(LogPacks {
                    index: Index(e["index"].as_u64().unwrap_or(0)),
                    packs: serde_json::from_value(e["packs"].clone()).map_err(|e| StoreError(e.to_string()))?,
                    promoted: serde_json::from_value(promoted).map_err(|e| StoreError(e.to_string()))?,
                    consumed: Vec::new(),
                    refs: serde_json::from_value(e.get("refs").cloned().unwrap_or(Value::Null))
                        .map_err(|e| StoreError(e.to_string()))?,
                });
            }
            at = last + 1;
        }
        Ok(out)
    }
    /// The fold's last `Promoted`. The fold does not keep the index of the entry that carried it, and no caller reads
    /// it (`ShardHost::rehydrate` reads the range), so the index is the read's applied index, an upper bound.
    async fn last_promoted(&self, _c: ChainId) -> std::result::Result<Option<(Index, Promoted)>, StoreError> {
        let v = self.ranges().await?;
        let p: Option<Promoted> =
            serde_json::from_value(v["promoted"].clone()).map_err(|e| StoreError(e.to_string()))?;
        Ok(p.map(|p| (Index(v["index"].as_u64().unwrap_or(0)), p)))
    }
    async fn consumed(&self, _n: NetworkId) -> std::result::Result<BTreeSet<PackId>, StoreError> {
        Ok(BTreeSet::new())
    }
}

fn se(e: StoreError) -> Error {
    Error::RustError(e.0)
}

fn q(url: &Url, k: &str) -> Option<String> {
    url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned())
}

/// Every site's bucket, by its `BUCKET_<n>` binding.
fn buckets(env: &Env, site: &SiteEnv) -> Vec<(SiteId, R2)> {
    site.sites.0.iter().filter_map(|s| env.bucket(&format!("BUCKET_{}", s.id.0)).ok().map(|b| (s.id, R2(b)))).collect()
}

impl DurableObject for GitShard {
    fn new(state: State, env: Env) -> Self {
        let vcpu = graftyard_git::prsync::instance_vcpu(INSTANCE);
        GitShard { state, env, budgets: std::cell::RefCell::new(graftyard_git::prsync::Budgets::new(vcpu)) }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let url = req.url()?;
        let site = SiteEnv::read(&self.env)?;
        let region = site.sites.get(site.id).ok_or("this site is not in GRAFTYARD_SITES")?.region;
        let host = ShardHost { state: &self.state, region, image: "git", instance: INSTANCE };
        let path = url.path().to_owned();
        // With `?repo=`, also the network's test-merge budget here (`testMerges`, B0-35's instruments).
        if path == "/status" {
            let running = self.state.container().is_some_and(|c| c.running());
            let budget = q(&url, "repo")
                .and_then(|v| v.parse().ok())
                .and_then(|r| self.budgets.borrow().stats(NetworkId(DbId(r))));
            return Response::from_json(
                &json!({ "running": running, "alarm": self.state.storage().get_alarm().await?, "testMerges": budget }),
            );
        }
        // `fanout`: this container's `NetworkLoad`, which the network's first shard reads from each pool shard.
        if path == "/load" {
            return Response::from_json(&host.load().await.map_err(se)?);
        }
        if path == "/teardown" {
            host.destroy().await.map_err(se)?;
            self.state.storage().delete_alarm().await?;
            return Response::from_json(&json!({ "torn_down": true }));
        }
        let repo: i64 = q(&url, "repo").and_then(|v| v.parse().ok()).ok_or("repo")?;
        let repo = RepoId(DbId(repo));
        let (net, chain) = (NetworkId(repo.0), ChainId::Repo(repo));
        // The caller's copy of the owner's record saves the two `Names` reads, about 400 ms once the shard's isolate
        // forgot the leaders (`tests/m2/results/m2-wp6.md`, "Merge latency"). A record that no longer finds the
        // group only slows the lookup: `GroupLog`'s reads go to the leader by the lookup rule either way.
        let record = match q(&url, "record").and_then(|r| serde_json::from_str::<PlacementRecord>(&r).ok()) {
            Some(r) => r,
            None => {
                let owner = crate::member::repo_owner(&self.env, repo).await.map_err(Error::RustError)?;
                crate::member::owner_record(&self.env, owner).await.map_err(Error::RustError)?
            }
        };
        let obj = self.state.id().to_string();
        let log = GroupLog { env: &self.env, sites: &site.sites, chain, record };
        let raw = buckets(&self.env, &site);
        let bs: Vec<&R2> = raw.iter().map(|(_, b)| b).collect();
        // A new leadership's `warm` (`crate::prsync`) is the one caller no client asked for: it catches up a running
        // shard and starts no stopped one, and it never counts as the shard's last request.
        let caller = if path == "/catchup" && q(&url, "warm").is_some() { Caller::Warm } else { Caller::Client };
        let running = self.state.container().is_some_and(|c| c.running());
        match graftyard_git::serve(caller, running) {
            Serve::Skip => return Response::from_json(&json!({ "skipped": "stopped" })),
            Serve::Run { touch: true } => host.touch(Timestamp(now_ms())).await.map_err(se)?,
            Serve::Run { touch: false } => {}
        }
        // Every catch-up is marked running while it runs, so a merge waits for it rather than racing it. A catch-up
        // also waits its turn behind one in flight on the same network, then goes on from the cursor that one saved:
        // two at once each rehydrated repo:65536 from nothing (762 `/entries` calls for 418 pages), and the later one
        // hung until the runtime ended it at 884 s ("Promise will never complete", `tests/m3/results/wire-fix.md`).
        let caught_up = || async {
            let _running = catch_ups(&obj, net).turn().await;
            host.catch_up(net, &[chain], &bs, &log).await
        };
        // A validate, clone, or fetch needs only every commit made before it arrived, so requests that arrive while a
        // catch-up runs share the next one (`CatchUps::fresh`): hosted, each catch-up reads the leader, and one each
        // held a network to about 2.7 validates/s (`tests/am1/results/am1-runs.md`, "Diagnosis").
        let fresh = || async {
            catch_ups(&obj, net)
                .fresh(|| async {
                    let c = host.catch_up(net, &[chain], &bs, &log).await?;
                    CURSORS.with(|m| m.borrow_mut().insert((obj.clone(), net.0.0), c.cursor.at(chain).0));
                    Ok(())
                })
                .await
        };
        match path.as_str() {
            "/validate" => {
                fresh().await.map_err(se)?;
                // The body is never read here: it streams to the container or to the pool shard (LM-1 round 6, where
                // buffered bodies put the shard's isolate over 128 MB). Its first `tipsn` bytes are the tips, then
                // `detailn` bytes of detail lines, then the pack.
                let num = |k: &str| q(&url, k).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                let (tipsn, detailn, agent) =
                    (num("tipsn"), num("detailn"), q(&url, "class").as_deref() == Some("agent"));
                let body = || req.inner().body().map(wasm_bindgen::JsValue::from);
                // `fanout`: the network's first shard forwards a push to the one of the k that rendezvous on its id
                // picks (`graftyard_git::push_validator`), which catches up before it validates. The push id is the
                // pack's trailer, its own SHA-1, the same on every re-send, which the caller names (`trailer`). A pool
                // shard never forwards (`via`). One that cannot answer leaves the push to the first shard: the body
                // is gone with the forward, so the caller sends it again with `via` (`x-graft-fanout-failed`). The
                // first shard is index 0 of a one-shard `count` here: each network has its own first-shard object, so
                // only the pool indices, `1..1 + HOT_POOL`, are shared between networks.
                let k = fanout_k(net);
                // Once a minute, on its own validates, the first shard reports its k shards' combined load to the
                // group's leader, which steps k (`crate::fanout`). A network with no pushes reports nothing.
                let due = FANOUT.with(|m| {
                    let mut m = m.borrow_mut();
                    let e = m.entry(net.0.0).or_insert((1, 0));
                    let due = now_ms() >= e.1 + crate::fanout::REPORT_EVERY_MS;
                    if due {
                        e.1 = now_ms();
                    }
                    due
                });
                if due && q(&url, "via").is_none() {
                    let own = host.load().await.unwrap_or_default();
                    let (env, sites, me, rec) = (self.env.clone(), site.sites.clone(), site.id, log.record.clone());
                    wasm_bindgen_futures::spawn_local(async move {
                        let mut loads = vec![own];
                        for s in graftyard_git::shards_for(net, me, 1, k).iter().skip(1) {
                            let name = graftyard_git::object_name(net, *s, 1);
                            let r = match crate::fanout::stub(&env, &sites, me, &name) {
                                Ok(st) => st.fetch_with_str("https://shard/load").await,
                                Err(e) => Err(e),
                            };
                            if let Ok(mut r) = r
                                && let Ok(l) = r.json::<graftyard_git::NetworkLoad>().await
                            {
                                loads.push(l);
                            }
                        }
                        match crate::fanout::report_from_shard(&env, &sites, me, net, &rec, &loads).await {
                            Ok(k) => FANOUT.with(|m| m.borrow_mut().entry(net.0.0).or_insert((1, 0)).0 = k),
                            Err(e) => console_error!("fanout report {}: {e}", net.0.0),
                        }
                    });
                }
                let trailer = q(&url, "trailer").and_then(|t| graftyard_git::refs::ObjectId::parse_hex(t.as_bytes()));
                if k > 1
                    && q(&url, "via").is_none()
                    && let Some(trailer) = trailer
                {
                    let pick = graftyard_git::push_validator(net, site.id, 1, k, &trailer.0);
                    if pick.shard != 0 {
                        let name = graftyard_git::object_name(net, pick, 1);
                        let rec = serde_json::to_string(&log.record)?;
                        let mut fwd = url.clone();
                        fwd.query_pairs_mut().append_pair("via", "0");
                        if q(&url, "record").is_none() {
                            fwd.query_pairs_mut().append_pair("record", &rec);
                        }
                        let mut init = RequestInit::new();
                        init.with_method(Method::Post).with_body(body());
                        let r = match crate::fanout::stub(&self.env, &site.sites, site.id, &name) {
                            Ok(s) => s.fetch_with_request(Request::new_with_init(fwd.as_str(), &init)?).await,
                            Err(e) => Err(e),
                        };
                        match r {
                            Ok(r) if matches!(r.status_code(), 200 | 422 | 429) => return Ok(r),
                            Ok(mut r) => console_error!(
                                "fanout: {name} answered {} {}; the caller validates on the first shard",
                                r.status_code(),
                                r.text().await.unwrap_or_default().chars().take(200).collect::<String>()
                            ),
                            Err(e) => console_error!("fanout: {name}: {e}; the caller validates on the first shard"),
                        }
                        let h = Headers::new();
                        h.set("x-graft-fanout-failed", "1")?;
                        return Ok(
                            Response::error("the pool shard did not answer; send again with via", 503)?.with_headers(h)
                        );
                    }
                }
                match host.validate(net, tipsn, detailn, agent, body()).await {
                    Ok(graftyard_git::host::Validated::Done(Some(ix), _)) if detailn == 0 => Response::from_json(&ix),
                    Ok(graftyard_git::host::Validated::Done(ix, detail)) => {
                        Response::from_json(&json!({ "index": ix, "detail": detail }))
                    }
                    Ok(graftyard_git::host::Validated::Busy { retry_after_s }) => {
                        let h = Headers::new();
                        h.set("retry-after", &retry_after_s.to_string())?;
                        Ok(Response::error(format!("validate queue full; retry after {retry_after_s} s"), 429)?
                            .with_headers(h))
                    }
                    Err(e) => Response::error(e.0, 422),
                }
            }
            "/info-refs" | "/upload-pack" => {
                fresh().await.map_err(se)?;
                let at = CURSORS.with(|m| m.borrow().get(&(obj.clone(), net.0.0)).copied().unwrap_or(0));
                let proto = req.headers().get("git-protocol")?.unwrap_or_default();
                let enc = req.headers().get("content-encoding")?.unwrap_or_default();
                let body = if path == "/upload-pack" { Some(req.bytes().await?) } else { None };
                let r = host.upload_pack(net, path == "/info-refs", body, &proto, &enc).await.map_err(se)?;
                let h = Headers::new();
                for (k, v) in r.headers().entries() {
                    h.set(&k, &v)?;
                }
                h.set("x-graft-cursor", &at.to_string())?;
                Ok(r.with_headers(h))
            }
            // A new leadership's `warm` (`crate::prsync`) comes here; a merge that arrives meanwhile waits for it.
            "/catchup" => Response::from_json(&caught_up().await.map_err(se)?),
            "/promote" => {
                caught_up().await.map_err(se)?;
                let sums: Vec<String> = req.json().await?;
                let sums: Vec<graftyard_types::Digest> = sums
                    .iter()
                    .map(|h| graftyard_types::Digest::parse_hex(h).ok_or_else(|| Error::RustError(format!("sum {h}"))))
                    .collect::<Result<_>>()?;
                Response::from_bytes(host.promote(net, &sums).await.map_err(se)?)
            }
            "/fsck" => Response::from_json(&host.fsck(net).await.map_err(se)?),
            // `GitCacheShard::merge` for a real merge or a merge-queue group (`method`), after catching up.
            "/merge" if q(&url, "method").is_some() => {
                let oid = |v: &str| {
                    graftyard_git::refs::ObjectId::parse_hex(v.as_bytes()).ok_or_else(|| Error::RustError(v.into()))
                };
                let base = oid(&q(&url, "base").unwrap_or_default())?;
                let ns = q(&url, "ns").unwrap_or_default();
                let heads = q(&url, "heads").unwrap_or_default();
                let chain: Vec<_> = ns
                    .split(',')
                    .zip(heads.split(','))
                    .map(|(n, h)| Ok((graftyard_git::issues::IssueNumber(n.parse().map_err(|_| "ns")?), oid(h)?)))
                    .collect::<Result<_>>()?;
                let method = match q(&url, "method").as_deref() {
                    Some("squash") => graftyard_git::issues::MergeMethod::Squash,
                    Some("rebase") => graftyard_git::issues::MergeMethod::Rebase,
                    _ => graftyard_git::issues::MergeMethod::Merge,
                };
                let t0 = now_ms();
                let caught = std::cell::RefCell::new(None);
                let waited = std::cell::Cell::new(0);
                let a = graftyard_git::merge_cache::merge_then_catch_up(
                    || async { host.up().await.map(|_| ()) },
                    || {
                        let idle = catch_ups(&obj, net).idle();
                        let waited = &waited;
                        async move {
                            let w0 = now_ms();
                            idle.await;
                            waited.set(now_ms() - w0);
                        }
                    },
                    || host.merge_chain(net, base, &chain, method),
                    || async {
                        *caught.borrow_mut() = Some(caught_up().await?.ms);
                        Ok(())
                    },
                )
                .await;
                let r = match a {
                    Ok(a) => Response::from_json(&a)?,
                    Err(e) => Response::error(e.0, 422)?,
                };
                // Where a merge plan's time went: the whole, and the catch-up's steps if it ran one.
                let timing = format!(
                    "merge;dur={}, waited;dur={}, catchup;desc=\"{:?}\"",
                    now_ms() - t0,
                    waited.get(),
                    caught.into_inner()
                );
                let h = Headers::new();
                h.set("x-graft-timing", &timing)?;
                Ok(r.with_headers(h))
            }
            // The `PrSync` consumer's test merge under TEST_MERGE_BUDGET (`cause`, a head push by default, and `n`).
            // Past the budget, or for a list field, `{"shed": true}` with no catch-up and no merge. Otherwise it
            // catches up so both commits are here, and the merge's own time is charged. Either way `x-graft-budget`
            // carries the network's counts.
            "/merge" => {
                let oid = |k: &str| {
                    q(&url, k)
                        .and_then(|v| graftyard_git::refs::ObjectId::parse_hex(v.as_bytes()))
                        .ok_or_else(|| Error::RustError(k.to_owned()))
                };
                let (base, head) = (oid("base")?, oid("head")?);
                // A head push's unless named: the consumer is the one caller.
                let cause = q(&url, "cause")
                    .as_deref()
                    .and_then(graftyard_git::prsync::MergeCause::parse)
                    .unwrap_or(graftyard_git::prsync::MergeCause::HeadPush);
                let n = graftyard_git::issues::IssueNumber(q(&url, "n").and_then(|v| v.parse().ok()).unwrap_or(0));
                let a = graftyard_git::prsync::budgeted(
                    &self.budgets,
                    net,
                    cause,
                    n,
                    now_ms,
                    || async { caught_up().await.map(|_| ()) },
                    || host.merge(net, base, head),
                )
                .await
                .map_err(se)?
                .unwrap_or(graftyard_git::prsync::MergeAnswer::Shed { shed: true });
                let stats = self.budgets.borrow().stats(net).unwrap_or_default();
                let h = Headers::new();
                h.set("x-graft-budget", &serde_json::to_string(&stats)?)?;
                Ok(Response::from_json(&a)?.with_headers(h))
            }
            // `git archive` of a commit the cache holds once it caught up (`codeload-archives`).
            "/archive" => {
                caught_up().await.map_err(se)?;
                let q: String = url
                    .query_pairs()
                    .filter(|(k, _)| k != "repo")
                    .map(|(k, v)| format!("{k}={}", js_sys::encode_uri_component(&v)))
                    .collect::<Vec<_>>()
                    .join("&");
                host.archive(net, &q).await.map_err(se)
            }
            _ => Response::error("not found", 404),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        let site = SiteEnv::read(&self.env)?;
        let region = site.sites.get(site.id).ok_or("this site is not in GRAFTYARD_SITES")?.region;
        let host = ShardHost { state: &self.state, region, image: "git", instance: INSTANCE };
        host.on_alarm(Timestamp(now_ms())).await.map_err(se)?;
        Response::empty()
    }
}
