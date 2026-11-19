//! `Inbox { account, site }` hosted (`design/DESIGN.md`, Shape, "Identity", `Inbox` and INBOX_WINDOW;
//! `build/BUILD-PLAN.md`, M4-WP2). The pure half is `graftyard_git::inbox`.
//!
//! - **The object.** `inbox:<account>` on the account's home site (its record's `home`), created with that site's
//!   location hint. Its SQLite holds the `Inbox` (threads and `sources`) in one `core` row. `/apply` takes one chain
//!   entry's notices for the account from the `Notify` consumer (`crate::activity`), and `Inbox::apply` makes a
//!   redelivered or replayed entry change nothing. `/view` answers the `Inbox`, which the edge merges with the
//!   account's `Identity` read and done state (`graftyard_api::activity::notification_page`).
//! - **The snapshot.** The alarm runs at each day's start after a change (`Inbox::snapshot_due`) and writes the
//!   `Snapshot` to `inbox/<account>/<at>` in its own site's bucket and the record's next site's. `/snapshot` writes one
//!   at once, for an operator who wants a newer base than the day's.
//! - **The rebuild.** `/rebuild` with the account's `UserRows` (read from its `Identity` leader by the caller) drops
//!   what the object holds, loads the newest snapshot from either bucket, and replays DESIGN's rule
//!   (`Snapshot::rebuild_from`): each chain in `sources` after its index, each watched or subscribed repository from
//!   INBOX_WINDOW before the snapshot. Each repository's leader computes the account's notices over that range from
//!   its stored log (`crate::activity::replay`). `/delete` drops the object's storage, as a lost object would be.
//!
//! No request path calls the Cloudflare API: every call is a Durable Object or R2 binding.

use std::cell::RefCell;

use graftyard_git::inbox::{DAY_MS, From, Inbox as Threads, Notice, Snapshot};
use graftyard_identity::UserRows;
use graftyard_raft::codec;
use graftyard_types::{AccountId, ChainId, DbId, Index, PlacementRecord, SiteId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use worker::*;

use crate::{SiteEnv, now_ms};

/// `/apply`'s body, in the raft codec.
#[derive(Serialize, Deserialize)]
pub struct ApplyReq {
    /// The account's record, which the alarm's snapshot writes by.
    pub record: PlacementRecord,
    pub chain: ChainId,
    pub index: Index,
    pub notices: Vec<Notice>,
}

pub fn do_name(a: AccountId) -> String {
    format!("inbox:{}", a.0.0)
}

/// The account's Inbox object on its record's home site.
pub fn stub(env: &Env, a: AccountId, record: &PlacementRecord) -> Result<Stub> {
    let s = SiteEnv::read(env)?;
    let region = s.sites.get(record.home).ok_or_else(|| Error::RustError("unknown home site".into()))?.region;
    env.durable_object(&format!("INBOX_{}", record.home.0))?.get_by_name_with_location_hint(&do_name(a), region.hint())
}

/// POSTs `body` to the account's Inbox and answers its JSON.
pub async fn call(env: &Env, a: AccountId, record: &PlacementRecord, path: &str, body: Vec<u8>) -> Result<Value> {
    let stub = stub(env, a, record)?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(js_sys::Uint8Array::from(&body[..]).into()));
    let mut r = stub.fetch_with_request(Request::new_with_init(&format!("https://inbox{path}"), &init)?).await?;
    if r.status_code() != 200 {
        return Err(Error::RustError(format!("inbox {path}: {} {}", r.status_code(), r.text().await?)));
    }
    r.json().await
}

#[durable_object(alarm)]
pub struct Inbox {
    state: State,
    env: Env,
    inbox: RefCell<Option<Threads>>,
}

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS core (k INTEGER PRIMARY KEY CHECK (k = 0), v BLOB NOT NULL, \
                      record TEXT)";

impl DurableObject for Inbox {
    fn new(state: State, env: Env) -> Self {
        state.storage().sql().exec(SCHEMA, None).expect("inbox schema");
        Inbox { state, env, inbox: RefCell::default() }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if crate::site_down(&self.env) {
            return Response::error("site down", 503);
        }
        self.load()?;
        let path = req.path();
        match path.as_str() {
            "/apply" => {
                let r: ApplyReq =
                    codec::from_bytes(&req.bytes().await?).map_err(|e| Error::RustError(e.to_string()))?;
                let applied = self.with(|i| i.apply(r.chain, r.index, &r.notices));
                if applied {
                    self.save()?;
                    self.state.storage().sql().exec(
                        "UPDATE core SET record = ? WHERE k = 0",
                        vec![serde_json::to_string(&r.record)?.into()],
                    )?;
                    self.arm().await?;
                }
                Response::from_json(&json!({ "applied": applied }))
            }
            "/view" => {
                let b = self.inbox.borrow();
                let i = b.as_ref().expect("loaded");
                Response::from_json(&json!({
                    "inbox": crate::host::hex(&codec::to_bytes(i)), "threads": i.threads.len(),
                    "sources": i.sources.len(), "snapshotDay": i.snapshot_day,
                }))
            }
            "/snapshot" => {
                let v: Value = req.json().await.unwrap_or(Value::Null);
                let record: PlacementRecord = serde_json::from_value(v["record"].clone())?;
                let key = self.snapshot(&record, Timestamp(now_ms())).await?;
                Response::from_json(&json!({ "key": key }))
            }
            "/delete" => {
                self.state.storage().sql().exec("DELETE FROM core", None)?;
                *self.inbox.borrow_mut() = Some(Threads::default());
                Response::from_json(&json!({ "deleted": true }))
            }
            "/rebuild" => {
                let v: Value = req.json().await?;
                let record: PlacementRecord = serde_json::from_value(v["record"].clone())?;
                let user: UserRows = codec::from_bytes(
                    &crate::host::unhex(v["user"].as_str().unwrap_or_default()).ok_or("user not hex")?,
                )
                .map_err(|e| Error::RustError(e.to_string()))?;
                let out = self.rebuild(&record, &user).await?;
                Response::from_json(&out)
            }
            _ => Response::error("not found", 404),
        }
    }

    /// At each day's start after a change: the day's snapshot.
    async fn alarm(&self) -> Result<Response> {
        if crate::site_down(&self.env) {
            return Response::empty();
        }
        self.load()?;
        let now = Timestamp(now_ms());
        let due = self.inbox.borrow().as_ref().is_some_and(|i| i.snapshot_due(now));
        if due && let Some(record) = self.record()? {
            self.snapshot(&record, now).await?;
        }
        Response::empty()
    }
}

impl Inbox {
    fn account(&self) -> AccountId {
        let name = self.state.id().name().unwrap_or_default();
        AccountId(DbId(name.strip_prefix("inbox:").and_then(|n| n.parse().ok()).unwrap_or(0)))
    }

    fn load(&self) -> Result<()> {
        if self.inbox.borrow().is_some() {
            return Ok(());
        }
        let r = crate::host::rows(&self.state.storage().sql(), "SELECT v FROM core WHERE k = 0", vec![])?;
        let i = match r.first() {
            Some(r) => codec::from_bytes(crate::host::blob(&r[0])?).map_err(|e| Error::RustError(e.to_string()))?,
            None => Threads::default(),
        };
        *self.inbox.borrow_mut() = Some(i);
        Ok(())
    }

    fn with<T>(&self, f: impl FnOnce(&mut Threads) -> T) -> T {
        f(self.inbox.borrow_mut().as_mut().expect("loaded"))
    }

    fn save(&self) -> Result<()> {
        let v = codec::to_bytes(self.inbox.borrow().as_ref().expect("loaded"));
        self.state.storage().sql().exec(
            "INSERT INTO core (k, v) VALUES (0, ?) ON CONFLICT (k) DO UPDATE SET v = excluded.v",
            vec![v.into()],
        )?;
        Ok(())
    }

    fn record(&self) -> Result<Option<PlacementRecord>> {
        let r = crate::host::rows(&self.state.storage().sql(), "SELECT record FROM core WHERE k = 0", vec![])?;
        Ok(r.first().and_then(|r| crate::host::text(&r[0]).ok()).and_then(|t| serde_json::from_str(t).ok()))
    }

    /// The next day's start, unless an alarm is set sooner.
    async fn arm(&self) -> Result<()> {
        let s = self.state.storage();
        let next = (now_ms() / DAY_MS + 1) * DAY_MS + 1_000;
        if s.get_alarm().await?.is_none_or(|d| d as u64 > next) {
            s.set_alarm(std::time::Duration::from_millis(next - now_ms())).await?;
        }
        Ok(())
    }

    /// Writes the snapshot to the home site's bucket and the record's next site's; answers its key.
    async fn snapshot(&self, record: &PlacementRecord, at: Timestamp) -> Result<String> {
        let snap = self.with(|i| i.snapshot(at));
        self.save()?;
        self.state
            .storage()
            .sql()
            .exec("UPDATE core SET record = ? WHERE k = 0", vec![serde_json::to_string(record)?.into()])?;
        let key = format!("inbox/{}/{:016}", self.account().0.0, at.0);
        let bytes = codec::to_bytes(&snap);
        let mut wrote = 0;
        for s in sites(record) {
            match self.env.bucket(&format!("BUCKET_{}", s.0))?.put(&key, bytes.clone()).execute().await {
                Ok(_) => wrote += 1,
                Err(e) => console_error!("inbox snapshot {key} on site {}: {e}", s.0),
            }
        }
        if wrote == 0 {
            return Err(Error::RustError(format!("inbox snapshot {key}: no bucket took it")));
        }
        Ok(key)
    }

    /// The newest snapshot in either bucket.
    async fn newest(&self, record: &PlacementRecord) -> Result<Option<(String, Snapshot)>> {
        let prefix = format!("inbox/{}/", self.account().0.0);
        let mut best: Option<String> = None;
        let mut from: Option<SiteId> = None;
        for s in sites(record) {
            let listed =
                match self.env.bucket(&format!("BUCKET_{}", s.0))?.list().prefix(prefix.clone()).execute().await {
                    Ok(l) => l,
                    Err(e) => {
                        console_error!("inbox list on site {}: {e}", s.0);
                        continue;
                    }
                };
            for o in listed.objects() {
                if best.as_ref().is_none_or(|b| o.key() > *b) {
                    best = Some(o.key());
                    from = Some(s);
                }
            }
        }
        let (Some(key), Some(s)) = (best, from) else { return Ok(None) };
        let o = self.env.bucket(&format!("BUCKET_{}", s.0))?.get(&key).execute().await?;
        let o = o.ok_or("snapshot gone")?;
        let bytes = o.body().ok_or("snapshot body")?.bytes().await?;
        let snap: Snapshot = codec::from_bytes(&bytes).map_err(|e| Error::RustError(e.to_string()))?;
        Ok(Some((key, snap)))
    }

    async fn rebuild(&self, record: &PlacementRecord, user: &UserRows) -> Result<Value> {
        let Some((key, snap)) = self.newest(record).await? else {
            return Err(Error::RustError("no snapshot to rebuild from".into()));
        };
        let mut ib = snap.inbox();
        let account = self.account();
        let mut replayed = Vec::new();
        for (chain, from) in snap.rebuild_from(user) {
            let ChainId::Repo(repo) = chain else { continue };
            let body = match from {
                From::After(i) => json!({ "after": i.0 }),
                From::Since(t) => json!({ "since": t.0 }),
            };
            let entries = match crate::activity::replay_notices(&self.env, repo, account, body).await {
                Ok(x) => x,
                // A purged repository has no notices left to replay, and must not fail every later rebuild
                // (`repo-gone`).
                Err(e) if crate::member::repo_gone(&self.env, repo, &e).await => {
                    replayed.push(json!({ "chain": chain.to_string(), "gone": e }));
                    continue;
                }
                Err(e) => return Err(Error::RustError(format!("replay {chain}: {e}"))),
            };
            let n = entries.len();
            for (index, ns) in entries {
                ib.apply(chain, index, &ns);
            }
            replayed.push(json!({ "chain": chain.to_string(), "from": format!("{from:?}"), "entries": n }));
        }
        *self.inbox.borrow_mut() = Some(ib);
        self.save()?;
        let b = self.inbox.borrow();
        let i = b.as_ref().expect("loaded");
        Ok(json!({ "snapshot": key, "replayed": replayed, "threads": i.threads.len(),
            "inbox": crate::host::hex(&codec::to_bytes(i)) }))
    }
}

/// The `Identity` member's `/inbox` route (`crate::family::Identity::route`): leader only, with the account's record
/// and its `UserRows` from this leader's fold. `{"op": "view"}` also answers the `UserRows`, so a caller sees both.
pub async fn route(h: &std::rc::Rc<crate::host::Host<crate::family::Identity>>, body: Value) -> Result<Value> {
    let (account, user) = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or("member not joined")?;
        if c.m.role() != graftyard_raft::Role::Leader {
            let nl = graftyard_raft::CallError::NotLeader(c.m.not_leader());
            return Ok(json!({ "error": crate::host::call_error_json(&nl) }));
        }
        let ChainId::Identity(a) = c.join.chain else { return Err("not an identity chain".into()) };
        (a, c.m.group_fold().fold.user().clone())
    };
    let record = crate::member::owner_record(&h.env, account).await.map_err(Error::RustError)?;
    let op = body["op"].as_str().unwrap_or("view");
    let user_hex = crate::host::hex(&codec::to_bytes(&user));
    let (path, payload) = match op {
        "view" | "delete" => (format!("/{op}"), Vec::new()),
        "snapshot" => ("/snapshot".into(), json!({ "record": record }).to_string().into_bytes()),
        "rebuild" => ("/rebuild".into(), json!({ "record": record, "user": user_hex }).to_string().into_bytes()),
        _ => return Err("op: view, snapshot, delete, rebuild".into()),
    };
    let mut v = call(&h.env, account, &record, &path, payload).await?;
    v["user"] = json!(user_hex);
    v["home"] = json!(record.home.0);
    Ok(v)
}

/// The home site, then the record's next site: where an account's snapshots go.
fn sites(record: &PlacementRecord) -> Vec<SiteId> {
    let mut v = vec![record.home];
    v.extend(record.sites.iter().copied().find(|s| *s != record.home));
    v
}
