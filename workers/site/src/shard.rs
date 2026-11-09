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
use graftyard_git::{ChainLog, Head, LogPacks, PackId, Promoted, StoreError};
use graftyard_lookup::Bindings;
use graftyard_types::{ChainId, DbId, Index, NetworkId, PlacementRecord, RepoId, SiteId, Timestamp};
use serde_json::{Value, json};
use worker::*;

use crate::{SiteEnv, now_ms};

thread_local! {
    /// Per network, the catch-ups running on this isolate's shard objects, so a merge waits for one in flight
    /// (`merge_then_catch_up`'s `in_flight`). An object's requests share its isolate.
    static CATCH_UPS: std::cell::RefCell<std::collections::HashMap<i64, graftyard_git::merge_cache::CatchUps>> =
        Default::default();
}

/// This isolate's catch-up gate for network `net`.
fn catch_ups(net: NetworkId) -> graftyard_git::merge_cache::CatchUps {
    CATCH_UPS.with(|m| m.borrow_mut().entry(net.0.0).or_default().clone())
}

#[durable_object]
pub struct GitShard {
    state: State,
    env: Env,
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
    async fn head(&self, _chain: ChainId) -> std::result::Result<Head, StoreError> {
        let v = self.ranges().await?;
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
        GitShard { state, env }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let url = req.url()?;
        let site = SiteEnv::read(&self.env)?;
        let region = site.sites.get(site.id).ok_or("this site is not in GRAFTYARD_SITES")?.region;
        let host = ShardHost { state: &self.state, region, image: "git", instance: "standard-2" };
        let path = url.path().to_owned();
        if path == "/status" {
            let running = self.state.container().is_some_and(|c| c.running());
            return Response::from_json(
                &json!({ "running": running, "alarm": self.state.storage().get_alarm().await? }),
            );
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
        let log = GroupLog { env: &self.env, sites: &site.sites, chain, record };
        let raw = buckets(&self.env, &site);
        let bs: Vec<&R2> = raw.iter().map(|(_, b)| b).collect();
        host.touch(Timestamp(now_ms())).await.map_err(se)?;
        // Every catch-up is marked running while it runs, so a merge waits for it rather than racing it. A catch-up
        // also waits for one in flight on the same network, then goes on from the cursor that one saved: two at once
        // each rehydrated repo:65536 from nothing (762 `/entries` calls for 418 pages), and the later one hung until
        // the runtime ended it at 884 s ("Promise will never complete", `tests/m3/results/wire-fix.md`).
        let caught_up = || async {
            catch_ups(net).idle().await;
            let _running = catch_ups(net).start();
            host.catch_up(net, &[chain], &bs, &log).await
        };
        match path.as_str() {
            "/validate" => {
                caught_up().await.map_err(se)?;
                let body = req.bytes().await?;
                let n: usize = q(&url, "tipsn").and_then(|v| v.parse().ok()).unwrap_or(0);
                let tips: Vec<String> =
                    String::from_utf8_lossy(&body[..n.min(body.len())]).split_whitespace().map(str::to_owned).collect();
                let rest = &body[n.min(body.len())..];
                // `detailn` (AM1's agent pushes and REST ref writes): the next bytes are the updates the git cache
                // reports detail for. A default push sends none, and its reply is the `PackIndex` alone, as before.
                let d: usize = q(&url, "detailn").and_then(|v| v.parse().ok()).unwrap_or(0);
                if d == 0 {
                    return match host.validate(net, rest.to_vec(), &tips).await {
                        Ok(ix) => Response::from_json(&ix),
                        Err(e) => Response::error(e.0, 422),
                    };
                }
                let detail = String::from_utf8_lossy(&rest[..d.min(rest.len())]).into_owned();
                match host.validate_detail(net, rest[d.min(rest.len())..].to_vec(), &tips, &detail).await {
                    Ok((ix, detail)) => Response::from_json(&serde_json::json!({ "index": ix, "detail": detail })),
                    Err(e) => Response::error(e.0, 422),
                }
            }
            "/info-refs" | "/upload-pack" => {
                let c = caught_up().await.map_err(se)?;
                let proto = req.headers().get("git-protocol")?.unwrap_or_default();
                let enc = req.headers().get("content-encoding")?.unwrap_or_default();
                let body = if path == "/upload-pack" { Some(req.bytes().await?) } else { None };
                let r = host.upload_pack(net, path == "/info-refs", body, &proto, &enc).await.map_err(se)?;
                let h = Headers::new();
                for (k, v) in r.headers().entries() {
                    h.set(&k, &v)?;
                }
                h.set("x-graft-cursor", &c.cursor.at(chain).0.to_string())?;
                h.set("x-graft-rehydrated", if c.rehydrated.is_some() { "1" } else { "0" })?;
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
                        let idle = catch_ups(net).idle();
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
            // The `PrSync` consumer's test merge, after catching up so both commits are here.
            "/merge" => {
                let oid = |k: &str| {
                    q(&url, k)
                        .and_then(|v| graftyard_git::refs::ObjectId::parse_hex(v.as_bytes()))
                        .ok_or_else(|| Error::RustError(k.to_owned()))
                };
                let (base, head) = (oid("base")?, oid("head")?);
                caught_up().await.map_err(se)?;
                Response::from_json(&host.merge(net, base, head).await.map_err(se)?)
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
        let host = ShardHost { state: &self.state, region, image: "git", instance: "standard-2" };
        host.on_alarm(Timestamp(now_ms())).await.map_err(se)?;
        Response::empty()
    }
}
