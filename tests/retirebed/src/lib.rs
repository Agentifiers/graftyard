//! `retire-sticks`'s bed (`tests/retirebed/run.sh`): the site's retire in local workerd, on a real Durable Object's
//! storage and reset. `/seed` joins (a `join` row and a log row, as `/join` stores them); `/retire?arm=` retires and
//! resets the object as `host::abort` does: `main` deletes the storage only, as main's `/retire` did, `fix` and
//! `reuse` through `workers/site/src/retire.rs`'s `wipe`. `/join` answers as the member's `/join` gate does, through
//! `retire::refusal`, and stores the `join` row when it may join. `/unjoined?p=` is a request to a member that holds
//! no group, routed by `retire::unjoined` as the member's `fetch` routes it: a retire wipes and writes the row,
//! anything else answers 409 and writes nothing. `/status` names the rows left.
use wasm_bindgen::JsCast;
use worker::*;

#[path = "../../../workers/site/src/retire.rs"]
#[allow(dead_code)]
mod retire;

#[durable_object]
pub struct M {
    state: State,
}

fn abort(state: &State) {
    let raw: &wasm_bindgen::JsValue = state.as_raw().as_ref();
    let g = js_sys::global();
    let abort = js_sys::Reflect::get(raw, &"abort".into()).ok().and_then(|f| f.dyn_into::<js_sys::Function>().ok());
    let timer = js_sys::Reflect::get(&g, &"setTimeout".into()).ok().and_then(|f| f.dyn_into::<js_sys::Function>().ok());
    if let (Some(abort), Some(timer)) = (abort, timer) {
        let _ = timer.call2(&g, &abort.bind1(raw, &"retired".into()), &0.into());
    }
}

impl DurableObject for M {
    fn new(state: State, _env: Env) -> Self {
        let sql = state.storage().sql();
        sql.exec("CREATE TABLE IF NOT EXISTS me (k TEXT PRIMARY KEY, v TEXT NOT NULL)", None).expect("me schema");
        Self { state }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let url = req.url()?;
        let arm = url.query_pairs().find(|(k, _)| k == "arm").map(|(_, v)| v.to_string()).unwrap_or_default();
        let sql = self.state.storage().sql();
        match url.path() {
            "/seed" => {
                sql.exec("INSERT OR REPLACE INTO me (k, v) VALUES ('join', '{}')", None)?;
                sql.exec("CREATE TABLE IF NOT EXISTS log (idx INTEGER PRIMARY KEY, row BLOB NOT NULL)", None)?;
                sql.exec("INSERT OR REPLACE INTO log (idx, row) VALUES (1, x'00')", None)?;
                Response::ok("seeded")
            }
            "/retire" => {
                match arm.as_str() {
                    "main" => self.state.storage().delete_all().await?,
                    a => retire::wipe(&self.state, Date::now().as_millis(), a == "reuse").await?,
                }
                abort(&self.state);
                Response::ok("retired")
            }
            "/unjoined" => {
                let p = url.query_pairs().find(|(k, _)| k == "p").map(|(_, v)| v.to_string()).unwrap_or_default();
                match retire::unjoined(&Method::Post, &p, true) {
                    retire::Unjoined::Retire { .. } => {
                        retire::wipe(&self.state, Date::now().as_millis(), false).await?;
                        abort(&self.state);
                        Response::ok("retired")
                    }
                    _ => Ok(Response::ok("member not joined")?.with_status(409)),
                }
            }
            "/join" => match retire::refusal(retire::at(&sql)) {
                Some(why) => Response::ok(format!("refused {why}")),
                None => {
                    sql.exec("INSERT OR REPLACE INTO me (k, v) VALUES ('join', '{}')", None)?;
                    Response::ok("joined")
                }
            },
            "/status" => {
                let me: Vec<serde_json::Value> = sql.exec("SELECT k FROM me ORDER BY k", None)?.to_array()?;
                let log = sql.exec("SELECT name FROM sqlite_master WHERE name = 'log'", None)?.to_array::<serde_json::Value>()?;
                let keys: Vec<&str> = me.iter().filter_map(|r| r["k"].as_str()).collect();
                Response::ok(format!("me={} log={}", keys.join(","), !log.is_empty()))
            }
            _ => Response::error("not found", 404),
        }
    }
}

#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let o = req.url()?.query_pairs().find(|(k, _)| k == "o").map(|(_, v)| v.to_string()).unwrap_or_default();
    if o.is_empty() {
        return Response::error("not found", 404);
    }
    env.durable_object("M")?.id_from_name(&o)?.get_stub()?.fetch_with_request(req).await
}
