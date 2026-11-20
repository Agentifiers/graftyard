//! `feed-per-member`'s bed (`tests/feedbed/run.sh`): the site's activity-feed wake in local workerd, which runs every
//! object in one isolate, as B0-29 m3fp3's learner 195 and site 0's voter shared one. Two member objects of one
//! repository: `l` holds polls as `crate::feed::poll` does (a `oneshot` waiter in isolate-wide state, then
//! `within(rx, Delay)` and the member's own I/O after each wake), and `v`'s input drains the waiters as `Feed::fed`
//! does. `KEY=repo` keys that state by repository, as `FEEDS` did on main; `KEY=member` keys it by object name, as
//! now. A wake that runs one object's future in the other's context shows in workerd's log as "Cannot perform I/O on
//! behalf of a different Durable Object" or "closure invoked recursively or after being dropped".
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

use futures_channel::oneshot;
use futures_util::future::{Either, select};
use worker::*;

thread_local! {
    /// The feed waiters, keyed as `KEY` says.
    static WAITERS: RefCell<BTreeMap<String, Vec<oneshot::Sender<bool>>>> = const { RefCell::new(BTreeMap::new()) };
}

fn key(env: &Env, member: &str) -> String {
    let by_repo = env.var("KEY").map(|v| v.to_string() == "repo").unwrap_or(false);
    if by_repo { member.split('#').next().unwrap_or(member).to_owned() } else { member.to_owned() }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let who = url.query_pairs().find(|(k, _)| k == "m").map(|(_, v)| v.to_string()).unwrap_or_default();
    if who.is_empty() {
        return Response::error("not found", 404);
    }
    let stub = env.durable_object("MEMBER")?.get_by_name(&who)?;
    stub.fetch_with_str(&format!("https://m{}?{}", url.path(), url.query().unwrap_or(""))).await
}

#[durable_object]
pub struct Member {
    state: State,
    env: Env,
}

impl DurableObject for Member {
    fn new(state: State, env: Env) -> Self {
        Self { state, env }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let url = req.url()?;
        let me = url.query_pairs().find(|(k, _)| k == "m").map(|(_, v)| v.to_string()).unwrap_or_default();
        let k = key(&self.env, &me);
        let st = self.state.storage();
        match url.path() {
            // A held poll, up to `wait` ms: woken by its member's fed entries, it does its own I/O and waits again.
            "/poll" => {
                let wait: u64 = url.query_pairs().find(|(k, _)| k == "wait").and_then(|(_, v)| v.parse().ok()).unwrap_or(3_000);
                let end = Date::now().as_millis() + wait;
                let mut woken = 0u32;
                loop {
                    let now = Date::now().as_millis();
                    if now >= end {
                        break;
                    }
                    let (tx, rx) = oneshot::channel();
                    WAITERS.with(|w| w.borrow_mut().entry(k.clone()).or_default().push(tx));
                    match select(rx, Delay::from(Duration::from_millis(end - now))).await {
                        Either::Left((Ok(_), _)) => {
                            woken += 1;
                            let n: u64 = st.get("woken").await.ok().flatten().unwrap_or(0);
                            st.put("woken", n + 1).await?;
                        }
                        _ => break,
                    }
                }
                Response::ok(format!("held woken={woken}"))
            }
            // An input on this member: it applied an entry, and wakes its feed's waiters.
            "/step" => {
                let n: u64 = st.get("steps").await.ok().flatten().unwrap_or(0);
                st.put("steps", n + 1).await?;
                let w = WAITERS.with(|w| w.borrow_mut().remove(&k).unwrap_or_default());
                let woke = w.len();
                for tx in w {
                    let _ = tx.send(false);
                }
                Response::ok(format!("woke {woke}"))
            }
            _ => Response::error("not found", 404),
        }
    }
}
