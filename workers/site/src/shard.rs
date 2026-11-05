//! The git cache shard class on a site script (`design/DESIGN.md`, Shape, "Git cache shards (derived)"): the M1-WP5
//! container behind `graftyard_git::host::ShardHost`, catching up behind its `IngestCursor` from the repository's
//! group before every call. The group is found by the lookup rule from the repository's owner (`RepoOwner` on
//! `Names`) and the owner's record, and read through its leader: `/read` for the core refs, `/entries` for the packs.
//!
//! Routes, each with `?repo=<id>` (a root repository's id is its network's): `POST /validate` (the tips, then the
//! pack), `GET /info-refs`, `POST /upload-pack`, `POST /catchup`, `GET /fsck`, `GET /status`, `POST /teardown`, and
//! `POST /promote` (a JSON list of ingested packs' SHA-256 hex: the home shard's half of `Promoted`).

use std::collections::BTreeSet;

use graftyard_git::host::{R2, ShardHost};
use graftyard_git::{ChainLog, CoreRefs, LogPacks, PackId, Promoted, StoreError};
use graftyard_lookup::Bindings;
use graftyard_types::{ChainId, DbId, Index, NetworkId, PlacementRecord, RepoId, SiteId, Timestamp};
use serde_json::{Value, json};
use worker::*;

use crate::{SiteEnv, now_ms};

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
    async fn core_refs(&self, chain: ChainId) -> std::result::Result<CoreRefs, StoreError> {
        let v = self.call("/read", &json!({ "q": {} })).await?;
        let refs: Vec<(String, String)> =
            serde_json::from_value(v["value"].clone()).map_err(|e| StoreError(e.to_string()))?;
        Ok(CoreRefs { chain, applied: Index(v["index"].as_u64().unwrap_or(0)), refs })
    }
    /// The fold's last `Archived` raises it (`RepoFold::first_index`).
    async fn first_index(&self, _chain: ChainId) -> std::result::Result<Index, StoreError> {
        let v = self.ranges().await?;
        Ok(Index(v["first"].as_u64().unwrap_or(1)))
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
        let owner = crate::member::repo_owner(&self.env, repo).await.map_err(Error::RustError)?;
        let record = crate::member::owner_record(&self.env, owner).await.map_err(Error::RustError)?;
        let log = GroupLog { env: &self.env, sites: &site.sites, chain, record };
        let raw = buckets(&self.env, &site);
        let bs: Vec<&R2> = raw.iter().map(|(_, b)| b).collect();
        host.touch(Timestamp(now_ms())).await.map_err(se)?;
        match path.as_str() {
            "/validate" => {
                host.catch_up(net, &[chain], &bs, &log).await.map_err(se)?;
                let body = req.bytes().await?;
                let n: usize = q(&url, "tipsn").and_then(|v| v.parse().ok()).unwrap_or(0);
                let tips: Vec<String> =
                    String::from_utf8_lossy(&body[..n.min(body.len())]).split_whitespace().map(str::to_owned).collect();
                match host.validate(net, body[n.min(body.len())..].to_vec(), &tips).await {
                    Ok(ix) => Response::from_json(&ix),
                    Err(e) => Response::error(e.0, 422),
                }
            }
            "/info-refs" | "/upload-pack" => {
                let c = host.catch_up(net, &[chain], &bs, &log).await.map_err(se)?;
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
            "/catchup" => Response::from_json(&host.catch_up(net, &[chain], &bs, &log).await.map_err(se)?),
            "/promote" => {
                host.catch_up(net, &[chain], &bs, &log).await.map_err(se)?;
                let sums: Vec<String> = req.json().await?;
                let sums: Vec<graftyard_types::Digest> = sums
                    .iter()
                    .map(|h| graftyard_types::Digest::parse_hex(h).ok_or_else(|| Error::RustError(format!("sum {h}"))))
                    .collect::<Result<_>>()?;
                Response::from_bytes(host.promote(net, &sums).await.map_err(se)?)
            }
            "/fsck" => Response::from_json(&host.fsck(net).await.map_err(se)?),
            // `git archive` of a commit the cache holds once it caught up (`codeload-archives`).
            "/archive" => {
                host.catch_up(net, &[chain], &bs, &log).await.map_err(se)?;
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
