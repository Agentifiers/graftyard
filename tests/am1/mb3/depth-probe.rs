//! merge-budget-3 item 5 probe: a leader object's background loop calls a shard object, which calls back into the
//! leader; does the loop's depth ratchet, and does running the loop in a stateless invocation (SELF) stop it?
use worker::*;
use worker::wasm_bindgen_futures::spawn_local;
use std::cell::RefCell;

thread_local! { static OUT: RefCell<std::collections::HashMap<String, String>> = RefCell::default(); }

async fn lp(env: &Env, tag: &str, n: u32, rmode: &str) -> String {
    let shard = env.durable_object("SHARD").unwrap().get_by_name_with_location_hint(&format!("s{tag}"), "enam").unwrap();
    if rmode.contains("self") {
        // As `prsync::one`: the attempt's own read goes through the lookup to this leader.
        let me = env.durable_object("LEADER").unwrap().get_by_name_with_location_hint(tag, "weur").unwrap();
        if let Err(e) = me.fetch_with_str(&format!("https://leader/read?tag={tag}&tick=1")).await { return format!("self read: {e}"); }
    }
    for k in 1..=n {
        // As `prsync::shard_post`: a new stub for every merge.
        let fresh = env.durable_object("SHARD").unwrap().get_by_name_with_location_hint(&format!("s{tag}"), "enam").unwrap();
        let shard = if rmode.contains("stub") { &fresh } else { &shard };
        let r = shard.fetch_with_str(&format!("https://shard/merge?leader={tag}&rmode={}&tick=1", rmode)).await;
        match r {
            Ok(mut r) if r.status_code() == 200 => {}
            Ok(mut r) => return format!("failedAt {k}: {}", r.text().await.unwrap_or_default().chars().take(60).collect::<String>()),
            Err(e) => return format!("failedAt {k}: {e}"),
        }
    }
    format!("ok {n}")
}

#[durable_object]
pub struct Leader { state: State, env: Env }
impl DurableObject for Leader {
    fn new(state: State, env: Env) -> Self { Leader { state, env } }
    async fn fetch(&self, req: Request) -> Result<Response> {
        let u = req.url()?;
        let q = |k: &str| u.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned()).unwrap_or_default();
        match u.path() {
            "/read" => {
                // A read-index round: the leader asks a follower before it answers.
                let mode = q("rmode");
                let f = self.env.durable_object("LEADER")?.get_by_name(&format!("{}-f", q("tag")))?;
                if mode == "await" {
                    f.fetch_with_str("https://leader/raft").await?;
                } else if mode == "spawn" {
                    spawn_local(async move { let _ = f.fetch_with_str("https://leader/raft").await; });
                }
                if q("tick") == "1" {
                    // As `Host::kick`: a ticker loop spawned from the request, running past its reply.
                    spawn_local(async move {
                        for _ in 0..30 {
                            worker::Delay::from(std::time::Duration::from_millis(100)).await;
                        }
                    });
                }
                Response::ok("ok")
            }
            "/raft" => Response::ok("ok"),
            "/start" => {
                let (tag, n, mode, rmode) = (q("tag"), q("n").parse::<u32>().unwrap_or(10), q("mode"), q("rmode"));
                
                // As `prsync::on_alarm` does: spawn the attempt and answer at once.
                let env = self.env.clone();
                spawn_local(async move {
                    let r = if mode == "inband" {
                        lp(&env, &tag, n, &rmode).await
                    } else {
                        let svc = env.service("SELF").unwrap();
                        match svc.fetch(format!("https://self/consume?tag={tag}&n={n}&rmode={rmode}"), None).await {
                            Ok(mut r) => r.text().await.unwrap_or_default(),
                            Err(e) => format!("self: {e}"),
                        }
                    };
                    OUT.with(|o| o.borrow_mut().insert(tag, r));
                });
                let _ = &self.state;
                Response::ok("started")
            }
            "/alarm" => {
                
                self.state.storage().put("cfg", vec![q("tag"), q("n"), q("mode")]).await?;
                self.state.storage().set_alarm(10).await?;
                Response::ok("armed")
            }
            "/out" => Response::ok(OUT.with(|o| o.borrow().get(&q("tag")).cloned().unwrap_or_default())),
            _ => Response::error("?", 404),
        }
    }
}

#[durable_object]
pub struct Shard { env: Env }
impl DurableObject for Shard {
    fn new(_state: State, env: Env) -> Self { Shard { env } }
    async fn fetch(&self, req: Request) -> Result<Response> {
        let u = req.url()?;
        let g = |k: &str| u.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned()).unwrap_or_default();
        let tag = g("leader");
        let l = self.env.durable_object("LEADER")?.get_by_name_with_location_hint(&tag, "weur")?;
        for _ in 0..2 {
            if let Err(e) = l.fetch_with_str(&format!("https://leader/read?tag={tag}&rmode={}&tick={}", g("rmode"), g("tick"))).await {
                return Response::error(e.to_string(), 500);
            }
        }
        Response::ok("merged")
    }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let u = req.url()?;
    let q = |k: &str| u.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned()).unwrap_or_default();
    if u.path() == "/consume" {
        return Response::ok(lp(&env, &q("tag"), q("n").parse().unwrap_or(10), &q("rmode")).await);
    }
    let l = env.durable_object("LEADER")?.get_by_name_with_location_hint(&q("tag"), "weur")?;
    l.fetch_with_str(&format!("https://leader{}?{}", u.path(), u.query().unwrap_or(""))).await
}
