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
//!
//! On a hot group a cold shard's rehydrate reads a log tail that can hold gigabytes of inline packs, which outlasts any
//! caller (`tests/am1/results/b0-30.md`, "Blocked on", 9 to 11). So a catch-up runs inside the object, not inside the
//! call that started it: it goes on after its caller stops waiting, saving every page, and the shard's alarm restarts
//! one an isolate reset cut off. A caller that passes `wait=<s>` waits at most that long and is answered 503
//! `{"warming": true}` after it. A caller with another shard to try (`next=1`) gets that at once while a rehydrate runs
//! here, and so does a pool shard asked to validate a push while it has no cursor; the first shard validates the push
//! itself.

use std::collections::BTreeSet;

use graftyard_git::group::RefsPage;
use graftyard_git::host::{R2, ShardHost};
use graftyard_git::{ChainLog, Head, LogPacks, PackId, Promoted, Serve, StoreError};
use graftyard_lookup::Bindings;
use graftyard_types::{ChainId, DbId, Index, NetworkId, PlacementRecord, RepoId, SiteId, Timestamp};
use serde_json::{Value, json};
use worker::*;

use crate::{SiteEnv, now_ms};

thread_local! {
    /// Per (shard object instance, network): the catch-up running in the background, which a caller that stops waiting
    /// leaves running, and the callers waiting for it. Keyed by instance, not object: a reset object (`ctx.abort()`)
    /// can come back in the same isolate, where its old run's future never ends, and keyed by the object id every
    /// catch-up after the reset waited on that run (`tests/am1/results/rehydrate-batch.md`, "Resume, hosted").
    static RUNS: std::cell::RefCell<std::collections::HashMap<(String, i64), Run>> = Default::default();
    /// Per (shard object, network), the log index the latest catch-up reached, for `x-graft-cursor`.
    static CURSORS: std::cell::RefCell<std::collections::HashMap<(String, i64), u64>> = Default::default();
    /// Per (shard object, network), the latest catch-up's report, which `/catchup` answers.
    static REPORTS: std::cell::RefCell<std::collections::HashMap<(String, i64), Value>> = Default::default();
    /// Per network, the k its group's fold held at this isolate's latest catch-up (`ranges`' `fanout`), when this
    /// isolate's first shard of it last reported `NetworkLoad` (`crate::fanout`), and the pool slots its site's
    /// ledger last answered it holds.
    static FANOUT: std::cell::RefCell<std::collections::HashMap<i64, Fan>> = Default::default();
    /// Per pool shard object name, when it last answered a forwarded push `warming` (`graftyard_git::pool_forward`).
    static POOL_COLD: std::cell::RefCell<std::collections::HashMap<String, u64>> = Default::default();
    /// Per (shard object instance, network): the composed-miss chain's step running in the background (`Compose`),
    /// the depth-1 upload-packs waiting for it, and the chain its container last held: the head the builder last put
    /// to the buckets, or the one another shard installed.
    static COMPOSES: std::cell::RefCell<std::collections::HashMap<(String, i64), ComposeRun>> = Default::default();
}

#[derive(Default)]
struct ComposeRun {
    running: bool,
    waiters: Vec<futures_channel::oneshot::Sender<()>>,
    synced: Option<graftyard_git::compose::ChainHead>,
}

/// How long a composable depth-1 upload-pack waits for its shard's chain step before it goes on [guess]: a base of
/// linux took 11 CPU-s to build and is 300 MB to fetch, and past this the container serves stock, as with no chain.
const COMPOSE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// One network's composed-miss chain on one shard object (`graftyard_git::host::ShardHost::ready_chain`). It is its
/// own step, run in the background after this shard's catch-ups on the builder and before a composable upload-pack
/// everywhere: no validate or catch-up waits for it, only the depth-1 upload-packs that the chain may answer.
#[derive(Clone)]
struct Compose {
    state: std::rc::Rc<State>,
    env: Env,
    obj: String,
    net: NetworkId,
    record: PlacementRecord,
}

impl Compose {
    fn key(&self) -> (String, i64) {
        (self.obj.clone(), self.net.0.0)
    }

    /// Starts a step unless one runs. `miss`: a composable upload-pack asks, so a shard other than the builder installs
    /// the chain too; after a catch-up (`!miss`) only the builder steps.
    fn start(&self, miss: bool) {
        let started = COMPOSES.with(|m| {
            let mut m = m.borrow_mut();
            let run = m.entry(self.key()).or_default();
            !std::mem::replace(&mut run.running, true)
        });
        if !started {
            return;
        }
        let me = self.clone();
        self.state.wait_until(async move {
            let synced = COMPOSES.with(|m| m.borrow().get(&me.key()).and_then(|r| r.synced.clone()));
            let r = me.run(miss, synced).await;
            if let Err(e) = &r {
                console_error!("shard compose {}: {e}", me.net.0.0);
            }
            let waiters = COMPOSES.with(|m| {
                let mut m = m.borrow_mut();
                let run = m.entry(me.key()).or_default();
                run.running = false;
                if let Ok(Some(h)) = r {
                    run.synced = Some(h);
                }
                std::mem::take(&mut run.waiters)
            });
            for w in waiters {
                let _ = w.send(());
            }
        });
    }

    /// Waits for a step that starts after this call, at most `COMPOSE_WAIT`, only while this object has seen no chain
    /// on its container: one that has starts the step in the background and goes on, since the container composes
    /// from a chain behind HEAD too.
    async fn ready(&self) {
        if COMPOSES.with(|m| m.borrow().get(&self.key()).is_some_and(|r| r.synced.is_some())) {
            self.start(true);
            return;
        }
        let ended = || {
            let (tx, rx) = futures_channel::oneshot::channel();
            COMPOSES.with(|m| m.borrow_mut().entry(self.key()).or_default().waiters.push(tx));
            rx
        };
        let next = async {
            if COMPOSES.with(|m| m.borrow().get(&self.key()).is_some_and(|r| r.running)) {
                let _ = ended().await;
            }
            let rx = ended();
            self.start(true);
            let _ = rx.await;
        };
        crate::host::within(COMPOSE_WAIT, next).await;
    }

    async fn run(
        &self,
        miss: bool,
        synced: Option<graftyard_git::compose::ChainHead>,
    ) -> std::result::Result<Option<graftyard_git::compose::ChainHead>, String> {
        let site = SiteEnv::read(&self.env).map_err(|e| e.to_string())?;
        let region = site.sites.get(site.id).ok_or("this site is not in GRAFTYARD_SITES")?.region;
        let instance = instance(&self.env).map_err(|e| e.to_string())?.0;
        let host = ShardHost { state: &self.state, region, image: "git", instance };
        let home = self.record.home;
        let ns = self.env.durable_object(&format!("SHARD_{}", site.id.0)).map_err(|e| e.to_string())?;
        let first = ns.id_from_name(&format!("git#{}#net:{}", site.id.0, self.net.0.0)).map_err(|e| e.to_string())?;
        let builder = site.id == home && first.to_string() == self.state.id().to_string();
        // After a catch-up only the builder steps; any other shard installs the chain when a miss asks for it.
        if !builder && !miss {
            return Ok(None);
        }
        let ids: Vec<SiteId> = site.sites.0.iter().map(|s| s.id).collect();
        let mut warm = graftyard_git::warm_sites(self.net, home, &ids).to_vec();
        warm.dedup();
        let raw: Vec<(SiteId, R2)> = graftyard_git::catchup::own_first(site.id, buckets(&self.env, &site))
            .into_iter()
            .filter(|(s, _)| warm.contains(s))
            .collect();
        let bs: Vec<&R2> = raw.iter().map(|(_, b)| b).collect();
        // The builder builds the base at warm, on the first catch-up of a network with no chain, at nice 19 in the
        // container, so a cold first miss usually finds a chain rather than waiting out `COMPOSE_WAIT` for stock.
        host.ready_chain(self.net, &bs, builder, synced.as_ref()).await.map_err(|e| e.0)
    }
}

/// One network's `FANOUT` entry in this isolate.
#[derive(Default)]
struct Fan {
    /// The committed k as last read; 0 before any read, which counts as 1.
    k: u8,
    /// When the first shard last reported `NetworkLoad`.
    reported: u64,
    /// The pool slots the site's ledger last answered the network holds.
    slots: Vec<u16>,
}

/// The k `net`'s group committed, as this isolate last read it; 1 before any read.
fn fanout_k(net: NetworkId) -> u8 {
    FANOUT.with(|m| m.borrow().get(&net.0.0).map_or(1, |f| f.k.max(1)))
}

/// The pool slots `net`'s first shard here forwards to: those the site's ledger last answered, at most k - 1.
fn pool_slots(net: NetworkId) -> Vec<u16> {
    FANOUT.with(|m| {
        m.borrow()
            .get(&net.0.0)
            .map_or(Vec::new(), |f| f.slots.iter().take(usize::from(f.k.max(1)) - 1).copied().collect())
    })
}

/// Storage key of the site's `PoolLedger`, on the ledger object (`crate::fanout::ledger_name`).
const LEDGER_KEY: &str = "pool_ledger";

/// Arms the ledger object's alarm for its next heartbeat while it holds a lease. The object has one alarm, which the
/// container's renewal (`ShardHost::on_alarm`) and catch-ups also set, so an earlier one is kept: every alarm runs the
/// beat, and the beat re-arms after it.
async fn arm_beat(s: &Storage, ledger: &graftyard_git::PoolLedger, now: Timestamp) -> Result<()> {
    let Some(at) = ledger.beat_due(now) else { return Ok(()) };
    if s.get_alarm().await.ok().flatten().is_none_or(|a| a > at.0 as i64) {
        s.set_alarm(std::time::Duration::from_millis(at.0 - now.0)).await?;
    }
    Ok(())
}

/// A catch-up in the background, and the callers waiting for it to end.
#[derive(Default)]
struct Run {
    runs: graftyard_git::CatchUps,
    waiters: Vec<futures_channel::oneshot::Sender<std::result::Result<(), String>>>,
}

/// Tries of an `/entries` read that answers no entry, before the catch-up stops on it.
const EMPTY_RETRIES: u32 = 4;
/// While a catch-up runs, the shard's alarm comes this often, so one an isolate reset cut off starts again.
const WARMING_ALARM_MS: u64 = 30_000;
/// The longest an alarm waits for a catch-up it restarted, under the runtime's 15 minutes for an alarm.
const ALARM_HOLDS: std::time::Duration = std::time::Duration::from_secs(600);
/// Storage key: a catch-up is running or was cut off, with the record it reads the group through.
const WARMING_KEY: &str = "warming";
/// The container's read-only routes for the web app (`containers/git/server/ui.go`), forwarded as `/ui/<route>`.
const UI_ROUTES: [&str; 8] = ["resolve", "tree", "blame", "diff", "compare", "readme", "commits", "blob"];

/// The shard container's instance type when the site sets no `SHARD_INSTANCE`: the 50-developer tier's
/// (`design/DESIGN.md`, Shape, `HOT_POOL`). A site day sets the tier's size, `standard-4` at 2,000 developers, which
/// journey 9's test-merge budget is sized on.
const DEFAULT_INSTANCE: &str = "standard-2";

/// This site's shard instance type (`SHARD_INSTANCE`), and its vCPU, which `TEST_MERGE_BUDGET` is a share of. A name
/// that is not a Containers instance type is refused rather than budgeted at a guess.
fn instance(env: &Env) -> Result<(&'static str, f32)> {
    let name = env.var("SHARD_INSTANCE").map(|v| v.to_string()).unwrap_or_else(|_| DEFAULT_INSTANCE.to_owned());
    let name: &'static str = ["lite", "basic", "standard-1", "standard-2", "standard-3", "standard-4"]
        .into_iter()
        .find(|n| *n == name)
        .ok_or_else(|| Error::RustError(format!("SHARD_INSTANCE {name:?} is not a Containers instance type")))?;
    let vcpu = graftyard_git::prsync::instance_vcpu(name).expect("listed above");
    Ok((name, vcpu))
}

#[durable_object]
pub struct GitShard {
    state: std::rc::Rc<State>,
    env: Env,
    /// TEST_MERGE_BUDGET of each network whose test merges run here, in this object's memory
    /// (`graftyard_git::prsync::Budgets`).
    budgets: std::cell::RefCell<graftyard_git::prsync::Budgets>,
    /// This object instance's key in `RUNS`, `CURSORS` and `REPORTS`: the object id and a number drawn when it starts.
    run_key: String,
}

struct GroupLog<'a> {
    env: &'a Env,
    sites: &'a graftyard_types::SiteTable,
    /// This site, whose member of the group answers committed pages first.
    me: SiteId,
    chain: ChainId,
    record: PlacementRecord,
}

impl GroupLog<'_> {
    async fn call(&self, path: &str, body: &Value) -> std::result::Result<Value, StoreError> {
        let b = Bindings { env: self.env, sites: self.sites };
        let (v, _) = graftyard_lookup::call(&b, self.chain, &self.record, path, body).await.map_err(StoreError)?;
        Ok(v)
    }

    /// One `/entries` page: from this site's member of the group when it has committed `from` (committed entries are
    /// the same on every member, and the local one is a call inside the site rather than across regions to the
    /// leader), as a page of up to `ENTRIES_LOCAL_PAGE_BYTES_MAX`; else, or when that read misses the lookup's CALL,
    /// a standard page from the leader.
    async fn page(&self, from: Index, to: Index) -> std::result::Result<Value, StoreError> {
        let path = format!("/entries?from={}&to={}", from.0, to.0);
        let local_path = format!("{path}&local=1&bytes={}", crate::host::ENTRIES_LOCAL_PAGE_BYTES_MAX);
        let b = Bindings { env: self.env, sites: self.sites };
        let me = Some(self.me);
        graftyard_lookup::learn_voters(&b, self.chain, &self.record).await;
        let local = graftyard_lookup::known_voters(self.chain)
            .into_iter()
            .map(|t| (t.site, t.do_name))
            .chain(self.record.bootstrap_members(self.chain).into_iter().map(|m| (m.site, m.do_name.to_string())))
            .find(|(s, _)| Some(*s) == me);
        if let Some((site, name)) = local
            && let Some((200, v)) = graftyard_lookup::post(&b, site, &name, &local_path, b"{}").await
            && v["entries"].as_array().and_then(|a| a.first()).and_then(|e| e["index"].as_u64()) == Some(from.0)
        {
            return Ok(v);
        }
        self.call(&path, &json!({})).await
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
            FANOUT.with(|m| m.borrow_mut().entry(r.0.0).or_default().k = k as u8);
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
    /// One page of `from..=to`: what one `/entries` call answers, which a member cuts at `ENTRIES_PAGE_BYTES`. A newly
    /// elected leader whose commit index lags answers short or empty; `graftyard_git::catchup` applies only what came
    /// and asks again from there. An empty page is retried here a few times first, 250 ms apart, so a leader change
    /// costs a moment rather than the call.
    async fn entries(&self, _c: ChainId, from: Index, to: Index) -> std::result::Result<Vec<LogPacks>, StoreError> {
        let mut out: Vec<LogPacks> = Vec::new();
        for attempt in 0..EMPTY_RETRIES {
            if attempt > 0 {
                worker::Delay::from(std::time::Duration::from_millis(250)).await;
            }
            let v = self.page(from, to).await?;
            let page: Vec<Value> =
                serde_json::from_value(v["entries"].clone()).map_err(|e| StoreError(e.to_string()))?;
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
            if !out.is_empty() {
                break;
            }
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

/// 503 `{"warming": true}`: the shard's catch-up goes on in the background; ask again.
fn warming() -> Result<Response> {
    Ok(Response::from_json(&json!({ "warming": true }))?.with_status(503))
}

/// One network's catch-ups on one shard object, run in the background (`Run`).
#[derive(Clone)]
struct Jobs {
    state: std::rc::Rc<State>,
    env: Env,
    obj: String,
    net: NetworkId,
    record: PlacementRecord,
}

impl Jobs {
    fn key(&self) -> (String, i64) {
        (self.obj.clone(), self.net.0.0)
    }

    /// Starts a catch-up unless one runs. It runs to its end whoever waits, under `wait_until`, and keeps the
    /// `warming` key while it runs so the shard's alarm can restart it after a reset.
    fn start(&self) {
        let started = RUNS.with(|r| {
            let mut r = r.borrow_mut();
            r.entry(self.key()).or_default().runs.start()
        });
        if !started {
            return;
        }
        let me = self.clone();
        self.state.wait_until(async move {
            let r = me.run().await;
            if let Err(e) = &r {
                console_error!("shard catch-up {}: {e}", me.net.0.0);
            }
            let ok = r.is_ok();
            let waiters = RUNS.with(|m| {
                let mut m = m.borrow_mut();
                let run = m.entry(me.key()).or_default();
                run.runs.end(ok);
                std::mem::take(&mut run.waiters)
            });
            for w in waiters {
                let _ = w.send(r.clone());
            }
        });
    }

    async fn run(&self) -> std::result::Result<(), String> {
        let site = SiteEnv::read(&self.env).map_err(|e| e.to_string())?;
        let region = site.sites.get(site.id).ok_or("this site is not in GRAFTYARD_SITES")?.region;
        let instance = instance(&self.env).map_err(|e| e.to_string())?.0;
        let host = ShardHost { state: &self.state, region, image: "git", instance };
        let chain = ChainId::Repo(RepoId(self.net.0));
        let log = GroupLog { env: &self.env, sites: &site.sites, me: site.id, chain, record: self.record.clone() };
        // This site's bucket first for each stored pack (`graftyard_git::catchup::own_first`).
        let raw = graftyard_git::catchup::own_first(site.id, buckets(&self.env, &site));
        let bs: Vec<(SiteId, &R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
        let s = self.state.storage();
        s.put(WARMING_KEY, (self.net.0.0, &self.record)).await.map_err(|e| e.to_string())?;
        if s.get_alarm().await.ok().flatten().is_none_or(|at| at > (now_ms() + WARMING_ALARM_MS) as i64) {
            let _ = s.set_alarm(std::time::Duration::from_millis(WARMING_ALARM_MS)).await;
        }
        let r = host.catch_up(self.net, &[chain], &bs, &log).await;
        let _ = s.delete(WARMING_KEY).await;
        let c = r.map_err(|e| e.0)?;
        CURSORS.with(|m| m.borrow_mut().insert(self.key(), c.cursor.at(chain).0));
        let v = serde_json::to_value(&c).unwrap_or(Value::Null);
        if c.rehydrated.is_some() {
            console_log!("shard rehydrate {}: {v}", self.net.0.0);
        } else if c.stored.packs > 0 {
            // Where a catch-up's stored packs came from and how its time split between their fetch and `index-pack`.
            console_log!(
                "shard catch-up stored {}: {}",
                self.net.0.0,
                serde_json::json!({ "ms": c.ms, "stored": c.stored, "entries": c.entries })
            );
        }
        REPORTS.with(|m| m.borrow_mut().insert(self.key(), v));
        // The builder takes the composed-miss chain's next step in the background, its base on a network with none;
        // the catch-up does not wait for it.
        Compose {
            state: self.state.clone(),
            env: self.env.clone(),
            obj: self.obj.clone(),
            net: self.net,
            record: self.record.clone(),
        }
        .start(false);
        Ok(())
    }

    /// Waits for a catch-up that started after this call arrived, at most `wait` when given: `Err` with the reply to
    /// send, 503 `warming` past the wait, and 503 when the catch-up failed too: a failed catch-up is this shard's, not
    /// the pack's, so the caller tries another site's shard (`graftyard_git::validate_status`).
    async fn wait(&self, wait: Option<std::time::Duration>) -> std::result::Result<(), Result<Response>> {
        let (tx, rx) = futures_channel::oneshot::channel();
        RUNS.with(|r| r.borrow_mut().entry(self.key()).or_default().waiters.push(tx));
        // A catch-up that is running started before this call: this waits for it, then for the next one.
        let running = RUNS.with(|r| r.borrow().get(&self.key()).is_some_and(|run| run.runs.running));
        let next = async {
            if running {
                let _ = rx.await;
                let (tx2, rx2) = futures_channel::oneshot::channel();
                RUNS.with(|r| r.borrow_mut().entry(self.key()).or_default().waiters.push(tx2));
                self.start();
                rx2.await.unwrap_or_else(|_| Err("the catch-up ended".into()))
            } else {
                self.start();
                rx.await.unwrap_or_else(|_| Err("the catch-up ended".into()))
            }
        };
        let r = match wait {
            Some(d) => match crate::host::within(d, next).await {
                Some(r) => r,
                None => return Err(warming()),
            },
            None => next.await,
        };
        r.map_err(|e| Response::error(e, graftyard_git::validate_status(None)))
    }

    /// Starts a catch-up unless one runs, and answers the number of the first run to start after this call, which
    /// reads the group's head after anything committed before it (`wait_from`).
    fn start_from(&self) -> u64 {
        let from = RUNS.with(|r| r.borrow().get(&self.key()).map_or(1, |run| run.runs.next()));
        self.start();
        from
    }

    /// Waits until a catch-up numbered `from` or later (`start_from`) has ended without an error, at most `wait`, with
    /// `wait`'s answers past it or on a failed run (`graftyard_git::CatchUps::step`).
    async fn wait_from(
        &self,
        from: u64,
        wait: Option<std::time::Duration>,
    ) -> std::result::Result<(), Result<Response>> {
        use graftyard_git::CatchUpStep;
        let next = async {
            loop {
                let runs = RUNS.with(|r| r.borrow().get(&self.key()).map(|run| run.runs).unwrap_or_default());
                match runs.step(from) {
                    CatchUpStep::Done => return Ok::<(), String>(()),
                    CatchUpStep::Start => self.start(),
                    CatchUpStep::Await => {}
                }
                let (tx, rx) = futures_channel::oneshot::channel();
                RUNS.with(|r| r.borrow_mut().entry(self.key()).or_default().waiters.push(tx));
                rx.await.unwrap_or_else(|_| Err("the catch-up ended".into()))?;
            }
        };
        let r = match wait {
            Some(d) => match crate::host::within(d, next).await {
                Some(r) => r,
                None => return Err(warming()),
            },
            None => next.await,
        };
        r.map_err(|e| Response::error(e, graftyard_git::validate_status(None)))
    }

    /// Ends once no catch-up of the network runs here.
    async fn idle(&self) {
        if !RUNS.with(|r| r.borrow().get(&self.key()).is_some_and(|run| run.runs.running)) {
            return;
        }
        let (tx, rx) = futures_channel::oneshot::channel();
        RUNS.with(|r| r.borrow_mut().entry(self.key()).or_default().waiters.push(tx));
        let _ = rx.await;
    }

    /// The latest catch-up's report here: what it read, ingested, and applied, and how much it held at once.
    fn report(&self) -> Value {
        REPORTS.with(|m| m.borrow().get(&self.key()).cloned().unwrap_or(Value::Null))
    }
}

/// Storage key of a network's test-merge counts on this shard object (`graftyard_git::prsync::BudgetStats`).
fn budget_key(net: NetworkId) -> String {
    format!("budget:{}", net.0.0)
}

impl GitShard {
    /// Re-arms the pool ledger's heartbeat after another writer replaced the object's one alarm. Only the object
    /// that holds a ledger has `LEDGER_KEY`.
    async fn rearm_beat(&self) -> Result<()> {
        let s = self.state.storage();
        match s.get::<graftyard_git::PoolLedger>(LEDGER_KEY).await.ok().flatten() {
            Some(l) => arm_beat(&s, &l, Timestamp(now_ms())).await,
            None => Ok(()),
        }
    }

    /// The pool ledger's routes. Each reads the ledger, applies one call at the object's clock, and stores it before
    /// answering, so the object's input gate keeps two calls from interleaving.
    async fn pool(&self, req: &mut Request, path: &str, site: &SiteEnv) -> Result<Response> {
        let ns = self.env.durable_object(&format!("SHARD_{}", site.id.0))?;
        let ledger = ns.id_from_name(&crate::fanout::ledger_name(site.id))?;
        if ledger.to_string() != self.state.id().to_string() {
            return Response::error("not this site's pool ledger", 404);
        }
        let s = self.state.storage();
        let now = Timestamp(now_ms());
        #[cfg(feature = "test-faults")]
        if path == "/pool/down" {
            let b: Value = req.json().await?;
            s.put("pool_down_until", now.0 + b["ms"].as_u64().unwrap_or(0)).await?;
            return Response::from_json(&json!({ "down_ms": b["ms"] }));
        }
        #[cfg(feature = "test-faults")]
        if s.get::<u64>("pool_down_until").await.ok().flatten().is_some_and(|t| now.0 < t) {
            return Response::error("pool ledger down (test fault)", 503);
        }
        let mut ledger: graftyard_git::PoolLedger = s.get(LEDGER_KEY).await.ok().flatten().unwrap_or_default();
        if path == "/pool" {
            return Response::from_json(&ledger);
        }
        let b: Value = req.json().await?;
        let net = NetworkId(DbId(b["network"].as_i64().ok_or("network")?));
        let slots = match path {
            "/pool/grant" => {
                let owner = b["owner"].as_u64().ok_or("owner")?;
                let want = b["want"].as_u64().ok_or("want")?.min(u64::from(graftyard_git::FANOUT_MAX)) as u8;
                ledger.ask(net, owner, want, now, graftyard_git::owner_share)
            }
            "/pool/renew" => ledger.renew(net, now),
            _ => return Response::error("no such pool route", 404),
        };
        s.put(LEDGER_KEY, &ledger).await?;
        arm_beat(&s, &ledger, now).await?;
        Response::from_json(&json!({ "slots": slots }))
    }

    /// Puts back `net`'s counts this object stored before it last reset, the first time the object sees `net`
    /// (`Budgets::restore`). The object resets about 90 s after its last request, and B0-35's gate reads the counts
    /// after the load (`tests/am1/results/merge-budget-2.md`, design change 5).
    async fn restore_budget(&self, net: NetworkId) {
        if self.budgets.borrow().stats(net).is_some() {
            return;
        }
        if let Ok(Some(stats)) = self.state.storage().get::<graftyard_git::prsync::BudgetStats>(&budget_key(net)).await
        {
            self.budgets.borrow_mut().restore(net, stats, now_ms());
        }
    }

    /// Stores `net`'s counts as they are now, one row per network, before the reply that carries them leaves.
    async fn save_budget(&self, net: NetworkId) {
        let Some(stats) = self.budgets.borrow().stats(net) else { return };
        if let Err(e) = self.state.storage().put(&budget_key(net), stats).await {
            console_error!("shard budget {}: {e}", net.0.0);
        }
    }
}

fn se(e: StoreError) -> Error {
    Error::RustError(e.0)
}

/// A clone-cache miss's log line, `shard miss <net> <CloneKey digest>: <graftyard_git::compose::MissLog::line>`, which
/// the probe that sent it can find by its key. Written once: when its first chunk goes on to the edge, or when the miss
/// ends without one. `None` for any other request.
struct MissLine(String, Option<graftyard_git::compose::MissLog>);

impl MissLine {
    fn note(&mut self, f: impl FnOnce(&mut graftyard_git::compose::MissLog)) {
        if let Some(m) = self.1.as_mut() {
            f(m);
        }
    }

    /// `r`, the miss's answer, noting when its headers came and, through `host::on_first_chunk`, when its first chunk
    /// went on to the edge; the line is written then, or when the body ends without one.
    fn forward(mut self, r: Response) -> Response {
        self.note(|m| m.served = Some((now_ms(), r.status_code(), r.headers().get("x-graft-compose").ok().flatten())));
        if self.1.is_none() {
            return r;
        }
        graftyard_git::host::on_first_chunk(r, move || {
            self.note(|m| m.first = Some(now_ms()));
            self.write();
        })
    }

    fn write(&mut self) {
        if let Some(m) = self.1.take() {
            console_log!("shard miss {}: {}", self.0, m.line());
        }
    }
}

impl Drop for MissLine {
    fn drop(&mut self) {
        self.write();
    }
}

fn q(url: &Url, k: &str) -> Option<String> {
    url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned())
}

/// Every site's bucket, by its `BUCKET_<n>` binding, in site order. The catch-up reorders it own-first for its stored
/// pack reads (`Jobs::run`); its marker listing reads every bucket, so order does not matter there.
fn buckets(env: &Env, site: &SiteEnv) -> Vec<(SiteId, R2)> {
    site.sites.0.iter().filter_map(|s| env.bucket(&format!("BUCKET_{}", s.id.0)).ok().map(|b| (s.id, R2(b)))).collect()
}

impl DurableObject for GitShard {
    fn new(state: State, env: Env) -> Self {
        // An unknown `SHARD_INSTANCE` budgets nothing here; every route refuses it below.
        let vcpu = instance(&env).map_or(0.0, |(_, v)| v);
        // The 6 h inactivity timeout belongs to the object instance that set it, so one that restarts while its
        // container runs sets it again before any event; without it the runtime stopped hot-a's shards about a
        // minute after their last request, each time on a new disk (`tests/am1/results/hot-shards.md`).
        if let Some(c) = state.container().filter(|c| c.running()) {
            let held = state.block_concurrency_while(async move {
                graftyard_git::host::set_inactivity(&c, graftyard_git::shard::INACTIVITY.as_millis() as u64)
                    .await
                    .map_err(|e| Error::RustError(e.0))
            });
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(e) = held.await {
                    console_error!("shard inactivity timeout: {e}");
                }
            });
        }
        let run_key = format!("{}:{}", state.id(), (worker::js_sys::Math::random() * 1e15) as u64);
        GitShard {
            run_key,
            state: std::rc::Rc::new(state),
            env,
            budgets: std::cell::RefCell::new(graftyard_git::prsync::Budgets::new(vcpu)),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let arrived = now_ms();
        if crate::site_down(&self.env) {
            return Response::error("site down", 503);
        }
        let url = req.url()?;
        let site = SiteEnv::read(&self.env)?;
        let region = site.sites.get(site.id).ok_or("this site is not in GRAFTYARD_SITES")?.region;
        let host = ShardHost { state: &self.state, region, image: "git", instance: instance(&self.env)?.0 };
        let path = url.path().to_owned();
        // With `?repo=`, also the network's test-merge budget here (`testMerges`, B0-35's instruments) and the
        // container's running git processes and the network's pack counts, all and kept (`procs`).
        if path == "/status" {
            let running = self.state.container().is_some_and(|c| c.running());
            let net = q(&url, "repo").and_then(|v| v.parse().ok()).map(|r| NetworkId(DbId(r)));
            if let Some(net) = net {
                self.restore_budget(net).await;
            }
            let budget = net.and_then(|net| self.budgets.borrow().stats(net));
            // A31's instruments: the container's `NetworkLoad` as `fanout` reads it (derived work left out) and the k
            // this isolate last read from the group, so a run shows what each minute's report was and whether k moved.
            let load = match net {
                Some(_) => host.load().await.ok(),
                None => None,
            };
            // Whether a rehydrate's place is kept (`ShardHost::rehydrating`), and the cursor this isolate's latest
            // catch-up reached, so a kill run can show the mark holds until the shard reaches the head.
            let rehydrating = match net {
                Some(n) => Some(host.rehydrating(n).await),
                None => None,
            };
            let cursor = net.and_then(|n| CURSORS.with(|m| m.borrow().get(&(self.run_key.clone(), n.0.0)).copied()));
            // The git processes running on the container, so a saturated shard says which one holds its CPU and
            // memory (LM-1 round 24c, where only its signature named the repack).
            let procs = match net {
                Some(n) => host.procs(n).await.ok(),
                None => None,
            };
            return Response::from_json(&json!({
                "running": running,
                "alarm": self.state.storage().get_alarm().await?,
                "testMerges": budget,
                "load": load,
                "k": net.map(fanout_k),
                "pool": net.map(pool_slots),
                "rehydrating": rehydrating,
                "cursor": cursor,
                "procs": procs,
            }));
        }
        // `fanout`: this container's `NetworkLoad`, which the network's first shard reads from each pool shard.
        if path == "/load" {
            return Response::from_json(&host.load().await.map_err(se)?);
        }
        // Test installs: resets this object (`ctx.abort()`) as an isolate reset does, while its container keeps running
        // on the same disk, so a catch-up it cuts off resumes from its saved place when the alarm restarts it. A site
        // redeploy did not cut one: the busy object stayed on its version until its run ended
        // (`tests/am1/results/rehydrate-batch.md`).
        #[cfg(feature = "test-faults")]
        if path == "/abort" {
            crate::host::abort(&self.state, "graftyard shard abort");
            return Response::from_json(&json!({ "aborted": true }));
        }
        // The site's pool ledger (`graftyard_git::PoolLedger`), on its first pool shard only: a network leader's ask
        // (`/pool/grant {network, owner, want}`), a first shard's renewal (`/pool/renew {network}`), and the whole
        // ledger (`/pool`). Storage only: no container starts for these.
        if path.starts_with("/pool") {
            return self.pool(&mut req, &path, &site).await;
        }
        if path == "/teardown" {
            host.destroy().await.map_err(se)?;
            self.state.storage().delete_alarm().await?;
            // A pool ledger outlives its shard's container: its heartbeat goes on while it holds a lease.
            self.rearm_beat().await?;
            return Response::from_json(&json!({ "torn_down": true }));
        }
        let repo: i64 = q(&url, "repo").and_then(|v| v.parse().ok()).ok_or("repo")?;
        let repo = RepoId(DbId(repo));
        let (net, chain) = (NetworkId(repo.0), ChainId::Repo(repo));
        let cause = q(&url, "cause")
            .as_deref()
            .and_then(graftyard_git::prsync::MergeCause::parse)
            .unwrap_or(graftyard_git::prsync::MergeCause::HeadPush);
        let n = graftyard_git::issues::IssueNumber(q(&url, "n").and_then(|v| v.parse().ok()).unwrap_or(0));
        let stats = |b: &graftyard_git::prsync::Budgets| -> Result<Headers> {
            let h = Headers::new();
            h.set("x-graft-budget", &serde_json::to_string(&b.stats(net).unwrap_or_default())?)?;
            Ok(h)
        };
        // The counts this object stored before its last reset, before any route below counts or charges.
        if matches!(path.as_str(), "/ask" | "/demand" | "/merge") && q(&url, "method").is_none() {
            self.restore_budget(net).await;
        }
        // B0-35's list-field instrument (A29): the edge's one ask for a list read that rendered `count` pull
        // requests' `mergeable` without a current test merge. Counted, never computed, and no container is woken.
        if path == "/ask" {
            let count: u32 = q(&url, "count").and_then(|v| v.parse().ok()).unwrap_or(1).min(100);
            {
                let mut b = self.budgets.borrow_mut();
                for _ in 0..count {
                    b.of(net, now_ms()).ask(cause, n, now_ms());
                }
            }
            self.save_budget(net).await;
            let h = stats(&self.budgets.borrow())?;
            return Ok(Response::from_json(&json!({ "asked": count }))?.with_headers(h));
        }
        // An on-demand merge's ask (`cause=get` from the edge, `dispatch` from `crate::prsync::demand`) is counted and
        // coalesced before anything else runs, so a GET inside GET_COALESCE_MS costs this answer alone.
        if path == "/demand" && !self.budgets.borrow_mut().of(net, now_ms()).ask(cause, n, now_ms()) {
            self.save_budget(net).await;
            let h = stats(&self.budgets.borrow())?;
            return Ok(Response::from_json(&json!({ "coalesced": true }))?.with_headers(h));
        }
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
        let obj = self.run_key.clone();
        let log = GroupLog { env: &self.env, sites: &site.sites, me: site.id, chain, record };
        // A new leadership's `warm` (`crate::prsync`) and promote-ahead's calls (`crate::archive`) are the callers no
        // client asked for: they run on a running shard, start no stopped one, and never count as its last request.
        // The network leader's warm of a pool slot its network holds (`warm=pool`, `crate::fanout`) starts a stopped
        // container: a pool shard nobody started never becomes warm.
        let caller = graftyard_git::caller(&path, q(&url, "warm").as_deref());
        let running = self.state.container().is_some_and(|c| c.running());
        match graftyard_git::serve(caller, running) {
            // The header tells a skip from a route's own 200, such as `/promote`'s pack or `/idx`'s bytes.
            Serve::Skip => {
                let h = Headers::new();
                h.set("x-graft-skipped", "stopped")?;
                return Ok(Response::from_json(&json!({ "skipped": "stopped" }))?.with_headers(h));
            }
            Serve::Run { touch: true } => {
                // `touch` sets the renewal alarm an hour out, past a pool ledger's next heartbeat.
                if host.touch(Timestamp(now_ms())).await.map_err(se)? {
                    self.rearm_beat().await?;
                }
            }
            Serve::Run { touch: false } => {}
        }
        // Every catch-up runs in the background on this object (`Jobs`), so one a caller stops waiting for goes on,
        // saving each page, and the next caller finds the shard warmer. Requests that arrive while one runs share the
        // next one: hosted, each catch-up reads the leader, and one each held a network to about 2.7 validates/s
        // (`tests/am1/results/am1-runs.md`, "Diagnosis"). One at a time per network: two at once each rehydrated
        // repo:65536 from nothing, and the later one never finished (`tests/m3/results/wire-fix.md`). `wait=<s>`
        // bounds how long this call waits; past it the call answers 503 `warming`.
        let wait = q(&url, "wait").and_then(|v| v.parse::<u64>().ok()).map(std::time::Duration::from_secs);
        let jobs = Jobs {
            state: self.state.clone(),
            env: self.env.clone(),
            obj: obj.clone(),
            net,
            record: log.record.clone(),
        };
        let caught_up = || jobs.wait(wait);
        // A rehydrate here takes tens of minutes on a hot group, so a caller with another site's shard to try next
        // (`next=1`) is answered 503 `warming` at once, whichever isolate it comes from, rather than waiting out its
        // `wait`. A pool shard forwarded a push (`pool=1`) while it holds no cursor for its disk answers that too: a
        // rehydrate of a hot group's tail outlasts the push (`tests/am1/results/b0-30.md`, "Why it crossed"), and the
        // first shard validates the push itself. Either way the catch-up goes on in the background.
        // `/catchup` too: the edge's probe before a streamed pack (`validate_streamed`) asks it with `next=1`.
        if matches!(path.as_str(), "/validate" | "/info-refs" | "/upload-pack" | "/promote" | "/catchup") {
            let follows = q(&url, "next").is_some();
            let rehydrating = follows && host.rehydrating(net).await;
            let pool_cold = path == "/validate" && q(&url, "pool").is_some() && !host.warm(net).await.unwrap_or(false);
            if graftyard_git::warming_now(follows, rehydrating, pool_cold) {
                jobs.start();
                return warming();
            }
        }
        match path.as_str() {
            "/validate" => {
                // The archive's own promotion (`derived`) is not a push: it reports no load and never forwards.
                let derived = q(&url, "derived").is_some();
                // A push validates against this disk as it stands, with no catch-up first, and the shard catches up
                // only when the container finds a base or ref the disk lacks (`OnDisk`, below). Caught up first, every
                // forwarded push of m3fr5c's B0-32 burst waited out two catch-up runs, the network shard's and the
                // pool shard's, each grown to 4 to 9.5 s by every shard ingesting every burst pack
                // (`tests/am1/results/disk-validate.md`). A catch-up starts either way, in the background, so the disk
                // and the network's k keep moving, and a missing base waits only for one that started after this call
                // arrived, not for the one in progress and then another (`Jobs::wait`). A promotion, or a container
                // this isolate holds no cursor for, catches up first.
                let cursor = CURSORS.with(|m| m.borrow().contains_key(&(obj.clone(), net.0.0)));
                let on_disk = graftyard_git::validate_on(derived, running, cursor) == graftyard_git::ValidateOn::Disk;
                let from = jobs.start_from();
                if !on_disk && let Err(r) = jobs.wait_from(from, wait).await {
                    return r;
                }
                // The body is never read here: it streams to the container or to the pool shard (LM-1 round 6, where
                // buffered bodies put the shard's isolate over 128 MB). Its first `tipsn` bytes are the tips, then
                // `detailn` bytes of detail lines, then the pack.
                let num = |k: &str| q(&url, k).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                let (tipsn, detailn, agent) =
                    (num("tipsn"), num("detailn"), q(&url, "class").as_deref() == Some("agent"));
                let body = || req.inner().body().map(wasm_bindgen::JsValue::from);
                // `fanout`: the network's first shard forwards a push to the one of the k that rendezvous on its id
                // picks (`graftyard_git::push_validator`), which validates it on its own disk. The push id is the
                // pack's trailer, its own SHA-1, the same on every re-send, which the caller names (`trailer`). A pool
                // shard never forwards (`via`). One that cannot answer leaves the push to the first shard: the body
                // is gone with the forward, so the caller sends it again with `via` (`x-graft-fanout-failed`). The
                // first shard is index 0 of a one-shard `count` here: each network has its own first-shard object, so
                // only the pool indices, `1..1 + HOT_POOL`, are pool shards, each leased to one network at a time by
                // the site's ledger.
                let k = fanout_k(net);
                let slots = pool_slots(net);
                // Once a minute, on its own validates, the first shard reports its k shards' combined load to the
                // group's leader, which steps k (`crate::fanout`). A network with no pushes reports nothing.
                let due = FANOUT.with(|m| {
                    let mut m = m.borrow_mut();
                    let e = m.entry(net.0.0).or_default();
                    let due = now_ms() >= e.reported + crate::fanout::REPORT_EVERY_MS;
                    if due {
                        e.reported = now_ms();
                    }
                    due
                });
                if due && q(&url, "via").is_none() && !derived {
                    let own = host.load().await.unwrap_or_default();
                    let (env, sites, me, rec) = (self.env.clone(), site.sites.clone(), site.id, log.record.clone());
                    let slots = slots.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        // The network's lease on this site's ledger, renewed as it is while the network takes pushes,
                        // and the slots it holds. A ledger that does not answer leaves the slots as they were.
                        let mut held = slots.clone();
                        if k > 1 || !slots.is_empty() {
                            let body = json!({ "network": net.0.0 });
                            if let Some(s) = crate::fanout::pool_call(&env, &sites, me, "/pool/renew", &body).await {
                                FANOUT.with(|m| m.borrow_mut().entry(net.0.0).or_default().slots = s.clone());
                                held = s;
                            }
                        }
                        held.truncate(usize::from(k) - 1);
                        let mut loads = vec![own];
                        for s in graftyard_git::shards_for(net, me, 1, &held).iter().skip(1) {
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
                            Ok((k, granted)) => FANOUT.with(|m| {
                                let mut m = m.borrow_mut();
                                let e = m.entry(net.0.0).or_default();
                                e.k = k;
                                if let Some(g) = granted {
                                    e.slots = g;
                                }
                            }),
                            Err(e) => console_error!("fanout report {}: {e}", net.0.0),
                        }
                    });
                }
                let trailer = q(&url, "trailer").and_then(|t| graftyard_git::refs::ObjectId::parse_hex(t.as_bytes()));
                if !slots.is_empty()
                    && q(&url, "via").is_none()
                    && !derived
                    && let Some(trailer) = trailer
                {
                    let pick = graftyard_git::push_validator(net, site.id, 1, &slots, &trailer.0);
                    let name = graftyard_git::object_name(net, pick, 1);
                    let cold_at = POOL_COLD.with(|m| m.borrow().get(&name).copied());
                    if pick.shard != 0 && graftyard_git::pool_forward(cold_at, now_ms()) {
                        let rec = serde_json::to_string(&log.record)?;
                        let mut fwd = url.clone();
                        fwd.query_pairs_mut().append_pair("via", "0").append_pair("pool", "1");
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
                            Ok(r) if matches!(r.status_code(), 200 | 422 | 429) => {
                                POOL_COLD.with(|m| m.borrow_mut().remove(&name));
                                return Ok(r);
                            }
                            // A pool shard still catching up gets no pushes for a while (`graftyard_git::pool_forward`);
                            // this one goes back to the caller to send again with `via`, as any failed forward does.
                            Ok(r) if r.status_code() == 503 => {
                                POOL_COLD.with(|m| m.borrow_mut().insert(name.clone(), now_ms()));
                                console_log!(
                                    "fanout: {name} is warming; pushes validate on the first shard for a while"
                                );
                            }
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
                // Only the container's own 4xx (a 429 is `Busy`) is the pack's answer; a call to it that broke (a kill
                // mid-push) is this shard's, a 503 the edge moves past (`graftyard_git::validate_status`).
                use graftyard_git::host::{OnDisk, Validated};
                let disk = if on_disk { OnDisk::AsItStands } else { OnDisk::CaughtUp };
                let r = match host.validate(net, tipsn, detailn, agent, derived, disk, body()).await {
                    // The container kept the request: once a catch-up that started after it ran, the same request is
                    // checked again, and every failure then is the pack's, as on a shard caught up first.
                    Ok(Validated::MissingBase { held }) => {
                        if let Err(r) = jobs.wait_from(from, wait).await {
                            return r;
                        }
                        host.validate(net, tipsn, detailn, agent, derived, OnDisk::Held(&held), None).await
                    }
                    r => r,
                };
                match r {
                    Ok(Validated::MissingBase { .. }) => Response::error("validate: a held request missed again", 503),
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
                    Err((status, e)) => Response::error(e.0, graftyard_git::validate_status(status)),
                }
            }
            "/info-refs" | "/upload-pack" => {
                let proto = req.headers().get("git-protocol")?.unwrap_or_default();
                let enc = req.headers().get("content-encoding")?.unwrap_or_default();
                let body = if path == "/upload-pack" { Some(req.bytes().await?) } else { None };
                // A clone-cache miss (`fill=<CloneKey digest>`): the container spools the whole answer whether or not
                // the client reads it to the end, and this object puts it to the home bucket, where the edge reads.
                let fill = q(&url, "fill").and_then(|h| graftyard_types::Digest::parse_hex(&h));
                // Each miss logs one `shard miss <net> <key>` line: its wait for the chain, the `compose=only` answer,
                // the catch-up before stock, the serving answer's headers (the container's first byte) and its first
                // chunk on to the edge, so a run splits the client's time to first byte between the chain, the
                // container and the transit (`graftyard_git::compose::MissLog`).
                let mut line = MissLine(
                    format!("{} {}", net.0.0, fill.map(|k| k.hex()).unwrap_or_default()),
                    fill.map(|_| graftyard_git::compose::MissLog { arrived, ..Default::default() }),
                );
                let filled = |r: &Response| -> Result<()> {
                    let Some(spool) = fill.filter(|_| r.headers().get("x-graft-spool").ok().flatten().is_some()) else {
                        return Ok(());
                    };
                    let key = graftyard_git::key::R2Key::CloneCache { network: net, key: spool, replica: 0 };
                    let (state, env, home) = (self.state.clone(), self.env.clone(), log.record.home);
                    let inst = instance(&self.env)?.0;
                    self.state.wait_until(async move {
                        let host = ShardHost { state: &state, region, image: "git", instance: inst };
                        let Ok(b) = env.bucket(&format!("BUCKET_{}", home.0)).map(R2) else { return };
                        if let Err(e) = host.put_spooled(spool, &[&b], &key.to_string()).await {
                            console_error!("shard clone-cache fill {}: {e}", net.0.0);
                        }
                    });
                    Ok(())
                };
                let reply = |r: Response, at: u64| -> Result<Response> {
                    let h = Headers::new();
                    for (k, v) in r.headers().entries() {
                        h.set(&k, &v)?;
                    }
                    h.set("x-graft-cursor", &at.to_string())?;
                    Ok(r.with_headers(h))
                };
                // A depth-1 fetch the chain may answer waits, bounded, for this shard's chain step, and nothing else
                // does (`Compose`). On a running shard it is asked composed first, before any catch-up: a composed
                // answer needs only the chain and the want, and in round 26 a composed miss's first byte waited 1.3 to
                // 68.7 s, behind catch-ups that ran with every tip move. The container answers 409 when it would serve
                // stock, and only then does the miss wait for a catch-up. The edge sends the body gunzipped.
                let composable = body.as_deref().is_some_and(|b| {
                    enc.is_empty()
                        && graftyard_git::smart::parse_v2(b).is_ok_and(|v| graftyard_git::compose::composable(&v))
                });
                if composable {
                    let compose = Compose {
                        state: self.state.clone(),
                        env: self.env.clone(),
                        obj: obj.clone(),
                        net,
                        record: log.record.clone(),
                    };
                    compose.ready().await;
                    line.note(|m| m.ready = Some(now_ms()));
                    if running && let Some(b) = &body {
                        let at = CURSORS.with(|m| m.borrow().get(&(obj.clone(), net.0.0)).copied().unwrap_or(0));
                        let r = host.upload_pack_spooled(net, b.clone(), &proto, &enc, fill, true).await.map_err(se)?;
                        if r.status_code() == 200 {
                            filled(&r)?;
                            return Ok(line.forward(reply(r, at)?));
                        }
                        let why = r.headers().get("x-graft-compose")?;
                        // A container that restarted holds no chain: the next miss waits for the step again.
                        if why.as_deref() == Some("stock: no chain") {
                            COMPOSES.with(|m| m.borrow_mut().entry(compose.key()).or_default().synced = None);
                        }
                        line.note(|m| m.only = Some((now_ms(), r.status_code(), why)));
                    }
                }
                if let Err(r) = caught_up().await {
                    return Ok(line.forward(r?));
                }
                line.note(|m| m.caught_up = Some(now_ms()));
                let at = CURSORS.with(|m| m.borrow().get(&(obj.clone(), net.0.0)).copied().unwrap_or(0));
                let r = match (fill, body) {
                    (Some(_), Some(b)) => {
                        let r = host.upload_pack_spooled(net, b, &proto, &enc, fill, false).await.map_err(se)?;
                        filled(&r)?;
                        r
                    }
                    (_, body) => host.upload_pack(net, path == "/info-refs", body, &proto, &enc).await.map_err(se)?,
                };
                Ok(line.forward(reply(r, at)?))
            }
            // A new leadership's `warm` (`crate::prsync`) comes here; a merge that arrives meanwhile waits for it.
            "/catchup" => match caught_up().await {
                Ok(()) => Response::from_json(&jobs.report()),
                Err(r) => r,
            },
            "/promote" => {
                if let Err(r) = caught_up().await {
                    return r;
                }
                let sums: Vec<String> = req.json().await?;
                let sums: Vec<graftyard_types::Digest> = sums
                    .iter()
                    .map(|h| graftyard_types::Digest::parse_hex(h).ok_or_else(|| Error::RustError(format!("sum {h}"))))
                    .collect::<Result<_>>()?;
                // The container's pack streams through to the archive's promotion, which tees it to the buckets
                // in parts: buffered here, a promotion held the whole pack in this isolate.
                host.promote(net, &sums).await.map_err(se)
            }
            "/fsck" => Response::from_json(&host.fsck(net).await.map_err(se)?),
            // The `.idx` of a pack this shard's `/validate` kept aside, for the archive's promotion to store with it.
            // No catch-up: the promotion's own validate just wrote it.
            "/idx" => {
                let sum = q(&url, "sha256")
                    .and_then(|h| graftyard_types::Digest::parse_hex(&h))
                    .ok_or_else(|| Error::RustError("sha256".into()))?;
                match host.validated_idx(net, sum).await.map_err(se)? {
                    Some(b) => Response::from_bytes(b),
                    None => Response::error("no validated pack", 404),
                }
            }
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
                let caught = std::cell::Cell::new(0);
                let waited = std::cell::Cell::new(0);
                let warming_too = std::cell::Cell::new(false);
                let a = graftyard_git::merge_cache::merge_then_catch_up(
                    worker::Delay::from(graftyard_git::merge_cache::MERGE_WAIT),
                    || async { host.up().await.map(|_| ()) },
                    || {
                        let idle = jobs.idle();
                        let waited = &waited;
                        async move {
                            let w0 = now_ms();
                            idle.await;
                            waited.set(now_ms() - w0);
                        }
                    },
                    || host.merge_chain(net, base, &chain, method),
                    || async {
                        let c0 = now_ms();
                        let r = caught_up().await;
                        caught.set(now_ms() - c0);
                        r.map_err(|_| {
                            warming_too.set(true);
                            graftyard_git::StoreError("warming".into())
                        })
                    },
                )
                .await;
                let r = match a {
                    Ok(a) => Response::from_json(&a)?,
                    Err(e) if warming_too.get() || e.0 == graftyard_git::merge_cache::WARMING => warming()?,
                    Err(e) => Response::error(e.0, 422)?,
                };
                // Where a merge plan's time went: the whole, and the catch-up's if it ran one.
                let timing =
                    format!("merge;dur={}, waited;dur={}, catchup;dur={}", now_ms() - t0, waited.get(), caught.get());
                let h = Headers::new();
                h.set("x-graft-timing", &timing)?;
                Ok(r.with_headers(h))
            }
            // The `PrSync` consumer's test merge (`/merge`, a head push's unless `cause` names another) and an
            // on-demand one (`/demand`, whose ask passed above), under TEST_MERGE_BUDGET. Shed, `{"shed": true}`, with
            // no merge, and with no catch-up unless the budget refused it only after one. Otherwise it catches up so both
            // commits are here, merges in its turn, and is charged the merge's CPU (`graftyard_git::prsync::asked`).
            // `/demand` commits what it computed itself, as a `PrSync` with no cursor (`on_demand`). Either way
            // `x-graft-budget` carries the network's counts.
            "/merge" | "/demand" => {
                let oid = |k: &str| {
                    q(&url, k)
                        .and_then(|v| graftyard_git::refs::ObjectId::parse_hex(v.as_bytes()))
                        .ok_or_else(|| Error::RustError(k.to_owned()))
                };
                let (base, head) = (oid("base")?, oid("head")?);
                let a = graftyard_git::prsync::asked(
                    &self.budgets,
                    net,
                    cause,
                    n,
                    now_ms,
                    || async { caught_up().await.map_err(|_| graftyard_git::StoreError("warming".into())) },
                    || host.merge(net, base, head),
                )
                .await;
                self.save_budget(net).await;
                let a = match a {
                    Err(e) if e.0 == "warming" => return warming(),
                    a => a.map_err(se)?,
                }
                .unwrap_or(graftyard_git::prsync::MergeAnswer::Shed { shed: true });
                let h = stats(&self.budgets.borrow())?;
                if path == "/merge" {
                    return Ok(Response::from_json(&a)?.with_headers(h));
                }
                let shed = matches!(a, graftyard_git::prsync::MergeAnswer::Shed { .. });
                let Some(c) = graftyard_git::prsync::on_demand(n, base, head, a) else {
                    return Ok(Response::from_json(&json!({ "shed": shed, "committed": false }))?.with_headers(h));
                };
                let b = Bindings { env: &self.env, sites: &site.sites };
                let cmd = serde_json::to_value(graftyard_git::repo::RepoCmd::Sync(c.clone()))?;
                let done = graftyard_lookup::commit_as(&b, chain, &log.record, c.request, cmd)
                    .await
                    .map_err(Error::RustError)?;
                let events = done.pointer("/ok/out/Sync/events").and_then(Value::as_u64);
                Ok(Response::from_json(&json!({ "computed": true, "committed": events.is_some(), "events": events }))?
                    .with_headers(h))
            }
            // `git archive` of a commit the cache holds once it caught up (`codeload-archives`).
            "/archive" => {
                if let Err(r) = caught_up().await {
                    return r;
                }
                let q: String = url
                    .query_pairs()
                    .filter(|(k, _)| k != "repo")
                    .map(|(k, v)| format!("{k}={}", js_sys::encode_uri_component(&v)))
                    .collect::<Vec<_>>()
                    .join("&");
                host.archive(net, &q).await.map_err(se)
            }
            // The web app's private reads (`design/SHARED.md`, "Frontend and native app", the `/_t/ui/*` API): a
            // tree, blame, diff, compare, README, or ref resolution of a commit the cache holds once it caught up.
            p if p.strip_prefix("/ui/").is_some_and(|r| UI_ROUTES.contains(&r)) => {
                if let Err(r) = caught_up().await {
                    return r;
                }
                let q: String = url
                    .query_pairs()
                    .filter(|(k, _)| !matches!(&**k, "repo" | "wait" | "next"))
                    .map(|(k, v)| format!("{k}={}", js_sys::encode_uri_component(&v)))
                    .collect::<Vec<_>>()
                    .join("&");
                host.ui(net, &p[4..], &q).await.map_err(se)
            }
            _ => Response::error("not found", 404),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        if crate::site_down(&self.env) {
            return Response::empty();
        }
        let site = SiteEnv::read(&self.env)?;
        let region = site.sites.get(site.id).ok_or("this site is not in GRAFTYARD_SITES")?.region;
        let host = ShardHost { state: &self.state, region, image: "git", instance: instance(&self.env)?.0 };
        let renewed = host.on_alarm(Timestamp(now_ms())).await.map_err(se);
        // The pool ledger's heartbeat (`graftyard_git::PoolLedger::beat`), on the one object that holds a ledger. It
        // runs after the container's renewal, which replaces the object's alarm, so its nearer one is armed last, and
        // whether or not the renewal failed. A test-faults outage skips the beat, as a down object would.
        let s = self.state.storage();
        if let Some(mut ledger) = s.get::<graftyard_git::PoolLedger>(LEDGER_KEY).await.ok().flatten() {
            let now = Timestamp(now_ms());
            #[cfg(feature = "test-faults")]
            let down = s.get::<u64>("pool_down_until").await.ok().flatten().is_some_and(|t| now.0 < t);
            #[cfg(not(feature = "test-faults"))]
            let down = false;
            let mut idle = Vec::new();
            if !down {
                idle = ledger.beat(now);
                s.put(LEDGER_KEY, &ledger).await?;
            }
            arm_beat(&s, &ledger, now).await?;
            // Slots no lease holds any more are torn down, so the pool runs only for networks hot in the last
            // `LOAD_STALE` (`graftyard_git::PoolLedger::beat`). Slot 0 is this object's own container.
            for slot in idle {
                let r = if slot == 0 {
                    host.destroy().await.map_err(se)
                } else {
                    let name = format!("git#{}#pool:{}", site.id.0, 1 + slot);
                    match crate::fanout::stub(&self.env, &site.sites, site.id, &name) {
                        Ok(st) => {
                            let mut init = RequestInit::new();
                            init.with_method(Method::Post);
                            st.fetch_with_request(Request::new_with_init("https://shard/teardown", &init)?)
                                .await
                                .map(|_| ())
                        }
                        Err(e) => Err(e),
                    }
                };
                match r {
                    Ok(()) => console_log!("pool slot {slot} on site {} torn down: no lease holds it", site.id.0),
                    Err(e) => console_error!("pool slot {slot} teardown: {e}"),
                }
            }
        }
        renewed?;
        // A catch-up an isolate reset cut off (its `warming` key outlived it) starts again here, from its saved page,
        // and the alarm waits for it, so an object with no request in flight stays up while it runs.
        let s = self.state.storage();
        if let Some((net, record)) = s.get::<(i64, PlacementRecord)>(WARMING_KEY).await.ok().flatten() {
            s.set_alarm(std::time::Duration::from_millis(WARMING_ALARM_MS)).await?;
            let net = NetworkId(DbId(net));
            let obj = self.run_key.clone();
            let jobs = Jobs { state: self.state.clone(), env: self.env.clone(), obj, net, record };
            jobs.start();
            crate::host::within(ALARM_HOLDS, jobs.idle()).await;
        }
        Response::empty()
    }
}
