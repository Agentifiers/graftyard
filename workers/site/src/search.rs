//! `SearchShard` hosted (`design/DESIGN.md`, Shape, "Identity", `SearchShard` and `Domain`; `build/BUILD-PLAN.md`,
//! M4-WP2). The pure half is `graftyard_git::search`.
//!
//! - **The object.** One per (domain, shard, generation), named `SearchShard::do_name`, on the site
//!   `SearchShard::site` names, created under the domain's DO jurisdiction (`graftyard_lookup::Bindings::object`).
//!   Its SQLite holds `search::DDL`. `/write` runs a batch of document ops in one transaction, `/query` runs one
//!   `query_sql` SELECT, and `/stats` reports where it runs (colo, jurisdiction) and how many documents it holds.
//! - **Generations.** A generation-1 object is built from its first write. A later generation is a rebuild: it
//!   answers 503 `building` to queries until `/built`, so a search over it sets `incomplete_results`. The newest
//!   generation of each (domain, shard) is the `search_map` rows of every site's `Registry` object `search_map`, the
//!   largest wins (`map`, `put_map`).
//! - **Lost.** `GRAFTYARD_SEARCH_LOST`, a site variable a site day sets, names shard objects lost with their site
//!   (comma-separated `do_name`s); the operator's `/lose`, through a repository leader's `/activity {"lose": name}`,
//!   marks one object lost as an instance loss would, with no site day. A lost object answers 503 `lost` to
//!   everything else and keeps its storage, as `GRAFTYARD_SITE_DOWN` does for a whole site. The `Search` consumer
//!   that meets it rebuilds the shard under the next generation (`crate::activity::rebuild`).
//!
//! No request path calls the Cloudflare API: every call here is a Durable Object binding.

use std::collections::BTreeMap;

use graftyard_git::search::{self, Arg, Doc, Domain, SearchShard as Key};
use graftyard_types::{AccountId, RepoId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeSet;
use worker::*;

use crate::SiteEnv;

/// One document op, as `/write` takes it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Op {
    Put(Doc),
    Delete {
        repo: RepoId,
        number: u32,
    },
    /// Every document of the repository, replacing what the shard holds of it.
    Repo {
        repo: RepoId,
        docs: Vec<Doc>,
    },
}

impl Op {
    fn stmts(&self) -> Vec<search::Stmt> {
        match self {
            Op::Put(d) => d.stmts(),
            Op::Delete { repo, number } => search::delete_stmts(*repo, *number),
            Op::Repo { repo, docs } => {
                let mut v = search::delete_repo_stmts(*repo);
                v.extend(docs.iter().flat_map(Doc::stmts));
                v
            }
        }
    }
}

fn arg(a: &Arg) -> SqlStorageValue {
    match a {
        Arg::Int(i) => SqlStorageValue::Integer(*i),
        Arg::Text(t) => SqlStorageValue::String(t.to_string()),
    }
}

/// `/query`'s body: `query_sql`'s statement and its arguments.
#[derive(Serialize, Deserialize)]
pub struct QueryReq {
    pub sql: String,
    pub args: Vec<QArg>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum QArg {
    Int(i64),
    Text(String),
}

/// A shard object's name in the `lost` list of this script's version.
fn lost_by_var(env: &Env, name: &str) -> bool {
    env.var("GRAFTYARD_SEARCH_LOST").is_ok_and(|v| v.to_string().split(',').any(|n| n.trim() == name))
}

/// The stub of shard `k` on its site, under its domain's jurisdiction.
pub fn stub(env: &Env, sites: &graftyard_types::SiteTable, k: &Key) -> std::result::Result<Stub, String> {
    let site = k.site(sites, &[]).ok_or("the domain has no site")?;
    let j = k.domain.jurisdiction().map(|j| match j {
        graftyard_types::Jurisdiction::Eu => "eu",
        graftyard_types::Jurisdiction::Us => "us",
    });
    graftyard_lookup::Bindings { env, sites }.object("SEARCH", site, &k.do_name(), j).map_err(|e| e.to_string())
}

/// POSTs `body` to shard `k`: `Ok(Err(why))` for a 503 (`lost`, `building`, `site down`).
pub async fn call(
    env: &Env,
    sites: &graftyard_types::SiteTable,
    k: &Key,
    path: &str,
    body: &Value,
) -> std::result::Result<std::result::Result<Value, String>, String> {
    let stub = stub(env, sites, k)?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(body.to_string().into()));
    let req = Request::new_with_init(&format!("https://search{path}"), &init).map_err(|e| e.to_string())?;
    let mut r = stub.fetch_with_request(req).await.map_err(|e| e.to_string())?;
    match r.status_code() {
        200 => Ok(Ok(r.json().await.map_err(|e| e.to_string())?)),
        503 => Ok(Err(r.text().await.unwrap_or_default())),
        s => Err(format!("{s} {}", r.text().await.unwrap_or_default())),
    }
}

// ---- the generation map ----

thread_local! {
    /// The (domain, shard) generations this isolate already recorded on the map.
    static MAPPED: RefCell<BTreeSet<Key>> = RefCell::default();
}

/// Records `k` on the map once per isolate: every domain written to is in the map, so a query reaches it.
pub async fn mapped(env: &Env, sites: &graftyard_types::SiteTable, k: &Key) -> std::result::Result<(), String> {
    if MAPPED.with(|m| m.borrow().contains(k)) {
        return Ok(());
    }
    put_map(env, sites, k).await?;
    MAPPED.with(|m| m.borrow_mut().insert(*k));
    Ok(())
}

const MAP: &str = "search_map";

fn map_key(d: Domain, shard: u8) -> String {
    Key { domain: d, shard, generation: 0 }.do_name().trim_start_matches("search#").trim_end_matches("#0").into()
}

/// The newest generation of each (domain, shard) any site's `search_map` holds, from the sites that answer.
pub async fn map(env: &Env, sites: &graftyard_types::SiteTable) -> BTreeMap<(Domain, u8), u32> {
    let reads = sites.0.iter().filter(|s| !s.spare).map(|s| async move {
        let stub = SiteEnv::read(env).ok()?.registry(env, s.id, MAP).ok()?;
        let mut r = stub.fetch_with_str("https://registry/search_map").await.ok()?;
        r.json::<BTreeMap<String, u32>>().await.ok()
    });
    let mut out = BTreeMap::new();
    for m in futures_util::future::join_all(reads).await.into_iter().flatten() {
        for (k, g) in m {
            let Some(s) = Key::parse(&format!("search#{k}#{g}")) else { continue };
            let e = out.entry((s.domain, s.shard)).or_insert(g);
            *e = (*e).max(g);
        }
    }
    out
}

/// Records shard `k`'s generation on every site's `search_map`; a lower one than a site holds changes nothing there.
/// Succeeds if any site took it.
pub async fn put_map(env: &Env, sites: &graftyard_types::SiteTable, k: &Key) -> std::result::Result<(), String> {
    let body = json!({ "key": map_key(k.domain, k.shard), "generation": k.generation }).to_string();
    let puts = sites.0.iter().filter(|s| !s.spare).map(|s| {
        let body = body.clone();
        async move {
            let stub = SiteEnv::read(env).ok()?.registry(env, s.id, MAP).ok()?;
            let mut init = RequestInit::new();
            init.with_method(Method::Put).with_body(Some(body.into()));
            let r = stub.fetch_with_request(Request::new_with_init("https://registry/search_map", &init).ok()?).await;
            r.ok().filter(|r| r.status_code() == 200).map(|_| ())
        }
    });
    let ok = futures_util::future::join_all(puts).await.into_iter().flatten().count();
    if ok == 0 { Err("no site took the search map".into()) } else { Ok(()) }
}

// ---- the object ----

#[durable_object(fetch)]
pub struct SearchShard {
    state: State,
    env: Env,
}

const META: &str =
    "CREATE TABLE IF NOT EXISTS shard_meta (k INTEGER PRIMARY KEY CHECK (k = 0), built INTEGER NOT NULL)";
const LOST: &str = "CREATE TABLE IF NOT EXISTS shard_lost (k INTEGER PRIMARY KEY CHECK (k = 0), at INTEGER NOT NULL)";

impl DurableObject for SearchShard {
    fn new(state: State, env: Env) -> Self {
        let sql = state.storage().sql();
        for s in search::DDL.iter().chain([META, LOST].iter()) {
            sql.exec(s, None).expect("search schema");
        }
        SearchShard { state, env }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if crate::site_down(&self.env) {
            return Response::error("site down", 503);
        }
        let name = self.state.id().name().unwrap_or_default();
        let sql = self.state.storage().sql();
        let path = req.path();
        if path == "/lose" {
            sql.exec("INSERT OR REPLACE INTO shard_lost (k, at) VALUES (0, ?)", vec![crate::host::i(crate::now_ms())])?;
            return Response::from_json(&json!({ "lost": name }));
        }
        if lost_by_var(&self.env, &name) || !crate::host::rows(&sql, "SELECT at FROM shard_lost", vec![])?.is_empty() {
            return Response::error("lost", 503);
        }
        let me = Key::parse(&name).ok_or_else(|| Error::RustError(format!("not a shard name: {name}")))?;
        // Generation 1 is built from its first write; a rebuild only once `/built`.
        let built = match self.meta()? {
            Some(b) => b,
            None => me.generation == 1,
        };
        match path.as_str() {
            "/write" => {
                let v: Value = req.json().await?;
                let ops: Vec<Op> = serde_json::from_value(v["ops"].clone())?;
                let mut stmts: Vec<crate::family::Stmt> = Vec::new();
                for op in &ops {
                    for s in op.stmts() {
                        stmts.push((s.sql, s.args.iter().map(arg).collect()));
                    }
                }
                if me.generation > 1 && self.meta()?.is_none() {
                    stmts.push(("INSERT OR REPLACE INTO shard_meta (k, built) VALUES (0, 0)", vec![]));
                }
                let n = stmts.len();
                crate::host::commit_tx(&self.state.storage(), stmts).map_err(Error::RustError)?;
                Response::from_json(&json!({ "stmts": n }))
            }
            "/built" => {
                sql.exec("INSERT OR REPLACE INTO shard_meta (k, built) VALUES (0, 1)", None)?;
                Response::from_json(&json!({ "built": true }))
            }
            "/query" => {
                if !built {
                    return Response::error("building", 503);
                }
                let q: QueryReq = req.json().await?;
                if !q.sql.starts_with("SELECT ") {
                    return Response::error("only a query_sql SELECT", 400);
                }
                let args: Vec<SqlStorageValue> = q
                    .args
                    .iter()
                    .map(|a| match a {
                        QArg::Int(i) => SqlStorageValue::Integer(*i),
                        QArg::Text(t) => SqlStorageValue::String(t.clone()),
                    })
                    .collect();
                let mut hits = Vec::new();
                let mut total = 0;
                for r in crate::host::rows(&sql, &q.sql, args)? {
                    let f = |v: &SqlStorageValue| match v {
                        SqlStorageValue::Float(f) => *f,
                        SqlStorageValue::Integer(i) => *i as f64,
                        _ => 0.0,
                    };
                    hits.push(json!([
                        crate::host::int(&r[0])?,
                        crate::host::int(&r[1])?,
                        crate::host::int(&r[2])?,
                        f(&r[3])
                    ]));
                    total = crate::host::int(&r[4])?;
                }
                Response::from_json(&json!({ "hits": hits, "total": total }))
            }
            "/stats" => {
                let v: Value = req.json().await.unwrap_or(Value::Null);
                let count = |q: &str, a: Vec<SqlStorageValue>| -> Result<i64> {
                    crate::host::rows(&sql, q, a)?.first().map_or(Ok(0), |r| crate::host::int(&r[0]))
                };
                let docs = count("SELECT count(*) FROM doc", vec![])?;
                let owner = v["owner"].as_i64().map(|o| AccountId(graftyard_types::DbId(o)));
                let owner_docs = match owner {
                    Some(o) => {
                        count("SELECT count(*) FROM doc WHERE owner = ?", vec![SqlStorageValue::Integer(o.0.0)])?
                    }
                    None => 0,
                };
                let fts = count("SELECT count(*) FROM doc_fts", vec![])?;
                Response::from_json(&json!({
                    "name": name, "site": SiteEnv::read(&self.env)?.id.0, "jurisdiction": self.jurisdiction(),
                    "colo": colo().await, "built": built, "docs": docs, "ftsRows": fts, "ownerDocs": owner_docs,
                }))
            }
            _ => Response::error("not found", 404),
        }
    }
}

impl SearchShard {
    /// Whether the object is built, as `shard_meta` says; `None` before anything was written.
    fn meta(&self) -> Result<Option<bool>> {
        let r = crate::host::rows(&self.state.storage().sql(), "SELECT built FROM shard_meta WHERE k = 0", vec![])?;
        r.first().map(|r| crate::host::int(&r[0]).map(|b| b != 0)).transpose()
    }

    /// The jurisdiction the runtime says this object's id was made under (`DurableObjectId.jurisdiction`).
    fn jurisdiction(&self) -> Value {
        let id: &wasm_bindgen::JsValue = self.state.as_raw().as_ref();
        let id = js_sys::Reflect::get(id, &"id".into()).unwrap_or(wasm_bindgen::JsValue::UNDEFINED);
        let j = js_sys::Reflect::get(&id, &"jurisdiction".into()).unwrap_or(wasm_bindgen::JsValue::UNDEFINED);
        j.as_string().map_or(Value::Null, Value::String)
    }
}

/// Where this isolate runs, from Cloudflare's trace endpoint, as the C15 probe read it.
pub async fn colo() -> Value {
    let r = async {
        let mut res = Fetch::Url(Url::parse("https://www.cloudflare.com/cdn-cgi/trace").ok()?).send().await.ok()?;
        let t = res.text().await.ok()?;
        t.lines().find_map(|l| l.strip_prefix("colo=").map(str::to_owned))
    };
    r.await.map_or(Value::Null, Value::String)
}
