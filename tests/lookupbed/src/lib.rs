//! lookup-poll's bed (`tests/lookupbed/run.sh`): the edge's `graftyard_lookup::call_opt` in local workerd, many
//! requests at once in one isolate, against three member objects that answer `NotLeader` naming no leader until
//! `/arm?leads_in=MS&chain=N` passes, then member 1 leads. `/read?chain=N` is one edge request; the driver cancels some mid-way.
//! A request whose lookup another request's future woke, or whose I/O another request owns, fails in workerd with
//! "Cannot perform I/O on behalf of a different request" or "closure invoked ... after being dropped" in its log.
use std::cell::Cell;

use graftyard_lookup::{Bindings, call_opt};
use graftyard_types::{
    ChainId, DbId, Member as MemberRef, MemberKind, PlacementName, PlacementRecord, Region, RepoId, Site, SiteId,
    SiteLink, SiteTable, Timestamp,
};
use serde_json::{Value, json};
use worker::*;

thread_local! {
    static LEADS_AT: Cell<u64> = const { Cell::new(0) };
}

fn chain(n: u64) -> ChainId {
    ChainId::Repo(RepoId(DbId(n as i64)))
}

fn record() -> PlacementRecord {
    PlacementRecord {
        name: PlacementName::Tri,
        home: SiteId(0),
        sites: [SiteId(0), SiteId(1), SiteId(2)].into_iter().collect(),
        generation: 1,
        moving_from: None,
        at: Timestamp(0),
    }
}

fn sites() -> SiteTable {
    let site = |id: u8, region| Site {
        id: SiteId(id),
        script: String::new(),
        account: "x".into(),
        region,
        jurisdiction: None,
        bucket: String::new(),
        release: None,
        link: SiteLink::Binding { member_class: "Member".into() },
        spare: false,
    };
    SiteTable(vec![site(0, Region::Enam), site(1, Region::Weur), site(2, Region::Apac)])
}

fn config(c: ChainId) -> Value {
    let m = |s: u8| MemberRef::new(c, SiteId(s), MemberKind::Full, s, 1);
    json!({ "index": 5, "config": { "normal": { "voters": [m(0), m(1), m(2)], "learners": [], "home": 0, "generation": 1 } } })
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.parse::<u64>().unwrap_or(0));
    match url.path() {
        "/arm" => {
            // Every member object of this bed shares the deadline through its own `/arm`.
            let at = Date::now().as_millis() + q("leads_in").unwrap_or(2_000);
            for s in 0..3u8 {
                // A fresh chain per round, so no isolate holds its leader yet.
                for c in [q("chain").unwrap_or(1)] {
                    let name = MemberRef::new(chain(c), SiteId(s), MemberKind::Full, s, 1).do_name.to_string();
                    let stub = env.durable_object(&format!("MEMBER_{s}"))?.get_by_name(&name)?;
                    stub.fetch_with_str(&format!("https://m/arm?at={at}&site={s}&chain={c}")).await?;
                }
            }
            Response::ok(at.to_string())
        }
        // Posts the three members took before the leader change, for this round's chain.
        "/posts" => {
            let mut n = 0;
            for s in 0..3u8 {
                let name = MemberRef::new(chain(q("chain").unwrap_or(1)), SiteId(s), MemberKind::Full, s, 1).do_name.to_string();
                let stub = env.durable_object(&format!("MEMBER_{s}"))?.get_by_name(&name)?;
                n += stub.fetch_with_str("https://m/posts").await?.text().await?.parse::<u64>().unwrap_or(0);
            }
            Response::ok(n.to_string())
        }
        "/read" => {
            let c = chain(q("chain").unwrap_or(1));
            let sites = sites();
            let b = Bindings { env: &env, sites: &sites };
            let t0 = Date::now().as_millis();
            let r = call_opt(&b, c, &record(), "/read", b"{\"q\":{\"probe\":true}}").await;
            let ms = Date::now().as_millis() - t0;
            Response::ok(match r {
                Ok(Some((v, _, t))) => format!("ok site={} ms={ms} value={}", t.site.0, v["value"]),
                Ok(None) => format!("none ms={ms}"),
                Err(e) => format!("err ms={ms} {e}"),
            })
        }
        _ => Response::error("not found", 404),
    }
}

#[durable_object]
pub struct Member {
    state: State,
}

impl DurableObject for Member {
    fn new(state: State, _env: Env) -> Self {
        Self { state }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let url = req.url()?;
        let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.parse::<u64>().unwrap_or(0));
        let st = self.state.storage();
        if url.path() == "/arm" {
            st.put("at", q("at").unwrap_or(0)).await?;
            st.put("site", q("site").unwrap_or(0)).await?;
            st.put("chain", q("chain").unwrap_or(1)).await?;
            return Response::ok("armed");
        }
        if url.path() == "/posts" {
            let n: u64 = st.get("posts").await.ok().flatten().unwrap_or(0);
            return Response::ok(n.to_string());
        }
        let at: u64 = st.get("at").await.ok().flatten().unwrap_or(0);
        if Date::now().as_millis() < at {
            let n: u64 = st.get("posts").await.ok().flatten().unwrap_or(0);
            st.put("posts", n + 1).await?;
        }
        let site: u64 = st.get("site").await.ok().flatten().unwrap_or(0);
        let c = chain(st.get("chain").await.ok().flatten().unwrap_or(1));
        // A member's turn: 30 ms, as a busy voter's answer.
        Delay::from(std::time::Duration::from_millis(30)).await;
        let now = Date::now().as_millis();
        let led = now >= at;
        if led && site == 1 {
            return Response::from_json(&json!({ "value": 1, "meta": { "term": 3, "leader": 1 } }));
        }
        let leader = if led { json!(1) } else { Value::Null };
        Response::from_json(&json!({
            "error": { "notLeader": { "term": 3, "leader": leader, "config": config(c) } },
            "meta": { "term": 3 }
        }))
    }
}
