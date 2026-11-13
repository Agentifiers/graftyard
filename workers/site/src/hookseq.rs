//! `HookSequencer { owner }` hosted (`design/DESIGN.md`, Shape, "Identity"; `design/SHARED.md`, "Event fan-out";
//! `build/BUILD-PLAN.md`, M4-WP1). The pure half is `graftyard_git::sequencer`.
//!
//! - **The consumer** (`on_alarm`, on a repository leader's alarm). It reads the log from the chain's committed
//!   `HookCursor` (`consumer_start`), renders each entry's webhooks once against the leader's fold
//!   (`graftyard_api::webhooks::payload`), and records the rows (`plan`) in the owner's `HookSequencer`. It then commits
//!   `HookCursorCmd` (`cursor_to_commit`) through its own member, at most once a second. One batch runs at a time per
//!   leadership; a reset leader or a new one starts again from the committed cursor.
//! - **Where the store is.** One object per hook owner, `hookseq:<owner>`, on the owner's home site (its record's
//!   `home`). When that site does not answer, the consumer records in the object of the same name on the record's
//!   next site, and tries the home site again every `HOME_RETRY_MS`. A store the consumer has not talked to in this
//!   leadership is first asked what it holds, and one that holds nothing of the chain (new, rebuilt, or lost with its
//!   site) is filled from the committed `replay_from`, which redelivers unfinished deliveries under their GUIDs.
//! - **The object** (`HookSequencer`). It keeps the `Sequencer` in its SQLite (one `core` row and one row per delivery)
//!   and owns order, the cursor halves, and the delivery rows. Its alarm starts attempts (`Sequencer::start`) and hands
//!   each to a delivery Worker, a stateless invocation of this site script over its own `SITE` binding
//!   (`attempt_handled`), with the request already built and signed. The invocation POSTs it with the 10 s deadline and
//!   reports to the object (`/report`), whose answer may be the same queue's next attempt (`next_in_queue`).
//! - **Attempts in flight.** One per (chain, hook) queue and one on the hook's own queue (pings and manual
//!   redeliveries), so at most two per hook, and at most `HOOK_OWNER_CONCURRENCY` per owner, failing hooks at most
//!   half. A next attempt answered to a report keeps its queue's slot; it never takes another queue's.
//! - **Secrets.** A hook secret is unwrapped by `graft-ops` (`/unwrap` over the `OPS` binding) and kept only in this
//!   isolate's memory. A hook whose secret cannot be unwrapped gets a failed attempt, never an unsigned one.
//!
//! No request path calls the Cloudflare API: every call here is a Durable Object or service binding.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use graftyard_api::hosted::{HostedDir, WireReply};
use graftyard_api::view::RepoRef;
use graftyard_api::webhooks::{RouteCtx, payload, request, sequencer_route};
use graftyard_git as git;
use graftyard_git::hooks::{
    ATTEMPT_DEADLINE_MS, Guid, HookCursor, HookCursorCmd, HookId, HookRecord, HookRow, hook_cursor, hook_row, hooks,
};
use graftyard_git::sequencer::{HookDelivery, NewDelivery, Sequencer, Start, consumer_start, cursor_to_commit, plan};
use graftyard_identity::sealed::{HookSecret, Wrapped};
use graftyard_raft::{Index, Role, codec};
use graftyard_types::{AccountId, ChainId, DbId, PlacementRecord, RepoId, SiteId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use worker::wasm_bindgen_futures::spawn_local;
use worker::*;

use crate::family::Repo;
use crate::host::Host;
use crate::{SiteEnv, now_ms};

/// The install's restore generation, which every delivery GUID includes (`design/SHARED.md`, "Event fan-out",
/// Delivery identity). An install restored through a new genesis starts at the next one [inference: no restore is
/// built yet, so every install is generation 1].
pub const GENERATION: u64 = 1;
/// Entries one batch renders at most [guess].
const BATCH_ENTRIES: u64 = 200;
/// Rendered bytes one batch sends at most [guess: under the 32 MiB a Durable Object request body may be].
const BATCH_BYTES: usize = 4 << 20;
/// The consumer commits its cursor at most this often per chain (BF-57's cursor lag is this plus a commit).
const CURSOR_EVERY_MS: u64 = 1_000;
/// While the store holds unfinished deliveries, the leader asks it again this often, to move `replay_from`.
const POLL_MS: i64 = 1_000;
/// A store call with no answer by then counts as failed [guess].
const STORE_FOR: std::time::Duration = std::time::Duration::from_secs(5);
/// How long one delivery Worker invocation (`attempt_handled`) keeps taking its queue's next attempt [guess: its last
/// attempt, up to the 10 s deadline, and its report then end within the 30 s `waitUntil` grace after its 202].
const CHAIN_FOR_MS: u64 = 15_000;
/// After falling back from the home site's store, the consumer tries the home site again this often [guess].
const HOME_RETRY_MS: u64 = 30_000;
/// How long a batch's logins and `Authz` read serve the next batches of the same leadership before a refresh starts
/// beside them [guess]. A round that waited for that refresh stopped bf57b's consumer for 26 s in BF-57 run 4
/// [inference: the stall began about 30 s after the leadership's first read, and nothing else in the round logged].
const DIR_FOR_MS: u64 = 30_000;
/// The store drops finished deliveries past their 3 days at most this often [guess]. `prune` walks every delivery
/// held, about 35 ms of CPU per alarm with BF-57 run 8's 80,000 (graft-m4a), and the alarm ran 4 to 5 times a second:
/// a third of the object's time, which every attempt's report waited behind [measured,
/// `tests/m4/results/m4-wp1-hosted.md`, BF-57 run 8].
const PRUNE_EVERY_MS: u64 = 60_000;
/// Attempt reports are written together, once per this window, not one transaction each [guess]. A Durable Object
/// holds every outgoing fetch until all its earlier writes are confirmed, so an attempt's POST waits for any write still
/// unconfirmed. With one commit per report an attempt took 110 ms at p50 (BF-57 run 9), and with one every 50 ms 223 ms
/// at p50 against 6 ms when no write was pending (run 11) [measured, `tests/m4/results/m4-wp1-hosted.md`]: a window of
/// 50 ms left a write pending almost always. A report lost to a reset inside the window leaves its attempt to be
/// re-sent under its GUID after `RESEND_AFTER_MS`, which the at-least-once contract allows.
const REPORT_SAVE_MS: u64 = 1_000;

/// One leadership's consumer state, in this isolate's memory.
#[derive(Default)]
struct Consumer {
    leadership: (u64, u64),
    busy: bool,
    /// An alarm came while a round ran: the round's end starts the next one at once, rather than after `POLL_MS`, so
    /// an entry committed meanwhile is recorded without waiting a poll.
    again: bool,
    /// The site whose store the consumer records in, and how far that store said it holds the chain (`None` for
    /// nothing). `None` until a store answered in this leadership.
    store: Option<(SiteId, Option<Index>)>,
    /// When the home site's store last failed, so it is tried again after `HOME_RETRY_MS`.
    home_failed_at: u64,
    cursor_at: u64,
    /// Rows recorded since the last cursor commit.
    dirty: bool,
    dir: Option<(u64, HostedDir)>,
    /// A refresh of a stale `dir` is running beside the rounds.
    dir_refreshing: bool,
}

thread_local! {
    static CONSUMERS: RefCell<HashMap<String, Consumer>> = RefCell::default();
    /// Unwrapped hook secrets, by their wrapped bytes.
    static SECRETS: RefCell<HashMap<Vec<u8>, HookSecret>> = RefCell::default();
}

/// `/record`'s body, in the raft codec: `chain`'s rows of entries `from + 1 ..= through`, and the chain's hooks as the
/// fold had them. `replay` says `from` is the committed cursor's `replay_from - 1`, which a store always accepts
/// (`record` adds nothing it holds); any other batch it accepts only if it holds the chain through `from`. A probe
/// (`rows` empty, `from == through == 0`, `replay` false) only asks.
#[derive(Serialize, Deserialize)]
pub struct RecordReq {
    pub chain: ChainId,
    pub from: Index,
    pub through: Index,
    pub replay: bool,
    pub hooks: Vec<(HookId, HookRecord)>,
    pub rows: Vec<NewDelivery>,
    /// The consumer moved to another site's store: this one ends the chain's queued deliveries unattempted, so two
    /// stores never deliver one (chain, hook) at once.
    #[serde(default)]
    pub drop: bool,
}

impl RecordReq {
    fn probe(chain: ChainId) -> RecordReq {
        RecordReq { chain, from: Index(0), through: Index(0), replay: false, hooks: vec![], rows: vec![], drop: false }
    }
}

#[derive(Serialize, Deserialize)]
pub struct RecordRes {
    pub accepted: bool,
    pub recorded: Option<Index>,
    pub cursor: HookCursor,
    pub idle: bool,
}

/// The leader's half, on the member's alarm.
pub async fn on_alarm(h: &Rc<Host<Repo>>) {
    let found = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref().filter(|c| c.m.role() == Role::Leader) else { return };
        let ChainId::Repo(repo) = c.join.chain else { return };
        let fold = &c.m.group_fold().fold;
        let cursor = hook_cursor(&fold.core);
        let hooks = hooks(&fold.core);
        // A repository that never had a hook has nothing to deliver and no cursor to keep.
        if hooks.is_empty() && cursor == HookCursor::default() {
            return;
        }
        let owner = fold.authz_owner().or(c.join.owner);
        let Some(owner) = owner else { return };
        let leadership = (c.m.term().0, c.m.boot());
        (c.join.me.do_name.to_string(), leadership, repo, owner, c.join.record.clone(), cursor, c.m.state().applied)
    };
    let (key, leadership, repo, owner, record, cursor, applied) = found;
    let go = CONSUMERS.with(|m| {
        let mut m = m.borrow_mut();
        let s = m.entry(key.clone()).or_default();
        if s.leadership != leadership {
            *s = Consumer { leadership, ..Consumer::default() };
        }
        if s.busy {
            s.again = true;
            return false;
        }
        s.busy = true;
        true
    });
    if !go {
        return;
    }
    let w = Rc::downgrade(h);
    spawn_local(async move {
        let Some(h) = w.upgrade() else { return };
        let r = round(&h, &key, repo, owner, &record, cursor, applied, leadership).await;
        let again = match &r {
            Ok(again) => *again,
            Err(e) => {
                console_error!("webhooks {}: {e}", repo.0.0);
                Some(POLL_MS * 5)
            }
        };
        let woke = CONSUMERS.with(|m| {
            let mut m = m.borrow_mut();
            let s = m.get_mut(&key).filter(|s| s.leadership == leadership)?;
            s.busy = false;
            Some(std::mem::take(&mut s.again))
        });
        let again = if woke == Some(true) && r.is_ok() { Some(0) } else { again };
        if let Some(ms) = again {
            h.arm_in(ms);
        }
    });
}

/// One round: ask or fill the store, then commit the cursor if due. Answers when the alarm should come back.
#[expect(clippy::too_many_arguments, reason = "what `on_alarm` read from the member under one borrow")]
async fn round(
    h: &Rc<Host<Repo>>,
    key: &str,
    repo: RepoId,
    owner: AccountId,
    record: &PlacementRecord,
    committed: HookCursor,
    applied: Index,
    leadership: (u64, u64),
) -> std::result::Result<Option<i64>, String> {
    let chain = ChainId::Repo(repo);
    let now = now_ms();
    // A directory past `DIR_FOR_MS` still serves while a refresh runs beside the rounds, so its reads (the owner's
    // `Authz` and the logins) never hold a round; only a consumer with none yet waits for one.
    let (store, dir_cached, refresh) = CONSUMERS.with(|m| {
        let mut m = m.borrow_mut();
        let s = m.get_mut(key);
        let stale = s.as_ref().and_then(|s| s.dir.as_ref()).is_some_and(|(at, _)| now >= at + DIR_FOR_MS);
        let refresh = match s {
            Some(s) if stale && !s.dir_refreshing => {
                s.dir_refreshing = true;
                true
            }
            _ => false,
        };
        let s = m.get(key);
        (s.and_then(|s| s.store), s.and_then(|s| s.dir.as_ref()).map(|(_, d)| d.clone()), refresh)
    });
    if refresh {
        let (w, key, record) = (Rc::downgrade(h), key.to_owned(), record.clone());
        spawn_local(async move {
            let Some(h) = w.upgrade() else { return };
            let t0 = now_ms();
            let d = crate::host::within(STORE_FOR, directory(&h, repo, owner, &record)).await;
            console_log!("webhooks dir {} refreshed in {}ms", repo.0.0, now_ms() - t0);
            CONSUMERS.with(|m| {
                if let Some(s) = m.borrow_mut().get_mut(&key) {
                    s.dir_refreshing = false;
                    match d {
                        Some(Ok(d)) => s.dir = Some((now_ms(), d)),
                        Some(Err(e)) => console_error!("webhooks dir {}: {e}", repo.0.0),
                        None => console_error!("webhooks dir {}: no answer in {} s", repo.0.0, STORE_FOR.as_secs()),
                    }
                }
            });
        });
    }
    // The store in use, or the home site's when none answered yet or home is due to be tried again: asked first.
    let home_failed_at = CONSUMERS.with(|m| m.borrow().get(key).map_or(0, |s| s.home_failed_at));
    let home_down = now < home_failed_at + HOME_RETRY_MS;
    let (site, held) = match store {
        Some((s, held)) if s == record.home || home_down => (s, held),
        _ => {
            // While the home site failed lately, the probe starts at the record's next site.
            let first = if home_down {
                record.sites.iter().copied().find(|s| *s != record.home).unwrap_or(record.home)
            } else {
                record.home
            };
            let (site, res) = send(h, key, record, owner, first, &RecordReq::probe(chain)).await?;
            (site, res.recorded)
        }
    };
    // A store that holds nothing of the chain is filled from the committed `replay_from`; one that holds it only to
    // before the committed `read` (one the consumer left for another site's and came back to) continues past what it
    // holds, or from `replay_from` if that is later (`consumer_start`). Either batch is a replay: the store takes it
    // even though it holds the chain only to before `from`.
    let start = consumer_start(committed, held);
    let replay = held.is_none_or(|h| h < start);
    let through = Index(applied.0.min(start.0 + BATCH_ENTRIES));
    let (rows, hooks, through) = if through > start || replay {
        let dir = match dir_cached {
            Some(d) => d,
            None => {
                let d = crate::host::within(STORE_FOR, directory(h, repo, owner, record))
                    .await
                    .ok_or_else(|| format!("the directory read took over {} s", STORE_FOR.as_secs()))??;
                CONSUMERS.with(|m| {
                    if let Some(s) = m.borrow_mut().get_mut(key) {
                        s.dir = Some((now, d.clone()));
                    }
                });
                d
            }
        };
        render(h, repo, &dir, start, through)?
    } else {
        // Nothing new to render, but the store still takes the chain's hooks as they are: an empty list would read
        // as every hook deleted.
        let core = h.core.borrow();
        let c = core.as_ref().ok_or("not joined")?;
        (Vec::new(), hooks(&c.m.group_fold().fold.core), start)
    };
    let recorded_rows = !rows.is_empty();
    let req = RecordReq { chain, from: start, through, replay, hooks, rows, drop: false };
    let (site, res) = send(h, key, record, owner, site, &req).await?;
    if !res.accepted {
        // The store holds less than this leadership thought (it was rebuilt): ask again from what it holds.
        CONSUMERS.with(|m| {
            if let Some(s) = m.borrow_mut().get_mut(key) {
                s.store = None;
            }
        });
        return Ok(Some(0));
    }
    // Moving from one site's store to another's (to a fallback, or home again): the old one stops delivering the chain.
    if let Some((old, _)) = store.filter(|(old, _)| *old != site) {
        let drop = RecordReq { drop: true, ..RecordReq::probe(chain) };
        if let Err(e) = call(&h.env, owner, old, &codec::to_bytes(&drop)).await {
            console_error!("webhooks store on site {} kept {chain}: {e}", old.0);
        }
    }
    let dirty = CONSUMERS.with(|m| {
        let mut m = m.borrow_mut();
        let s = m.get_mut(key).ok_or("no consumer state")?;
        s.store = Some((site, res.recorded));
        s.dirty |= recorded_rows;
        Ok::<_, String>(s.dirty)
    })?;
    let cursor_at = CONSUMERS.with(|m| m.borrow().get(key).map_or(0, |s| s.cursor_at));
    let due = now_ms() >= cursor_at + CURSOR_EVERY_MS;
    let mut wait = None;
    if let Some(next) = cursor_to_commit(committed, res.cursor, dirty) {
        if due {
            // The commit runs beside the next rounds rather than holding them: a round waited for it, a quorum round
            // trip, before recording the next entries' rows. A failed one marks the rows uncommitted again; a late
            // one is harmless, as `HookCursorCmd` only moves forward.
            CONSUMERS.with(|m| {
                if let Some(s) = m.borrow_mut().get_mut(key) {
                    s.cursor_at = now_ms();
                    s.dirty = false;
                }
            });
            // BF-57's cursor lag: from the commit of the first entry this cursor passes that carries anything but a
            // cursor, to the cursor's own commit. A cursor entry has no webhook, and a lag from one only measures the
            // cursor chasing its own tail (run 9's one 8.6 s lag passed nothing else).
            let first_at = (committed.read.0 + 1..=next.read.0).find_map(|i| event_time(h, Index(i)));
            let (w, key) = (Rc::downgrade(h), key.to_owned());
            spawn_local(async move {
                let Some(h) = w.upgrade() else { return };
                let r = commit_cursor(&h, next).await;
                if let (Ok(()), Some(at)) = (&r, first_at) {
                    console_log!(
                        "webhooks cursor {} read {} lag {}ms",
                        repo.0.0,
                        next.read.0,
                        now_ms().saturating_sub(at.0)
                    );
                }
                if let Err(e) = r {
                    console_error!("webhooks cursor {}: {e}", repo.0.0);
                    CONSUMERS.with(|m| {
                        if let Some(s) = m.borrow_mut().get_mut(&key).filter(|s| s.leadership == leadership) {
                            s.dirty = true;
                        }
                    });
                }
            });
        } else {
            wait = Some((cursor_at + CURSOR_EVERY_MS).saturating_sub(now_ms()) as i64);
        }
    }
    // More of the log to read comes back at once; a store with unfinished deliveries is asked again to move
    // `replay_from`; a store on another site than the home site tries home again.
    let more = res.recorded.unwrap_or_default() < applied;
    Ok(if more {
        Some(0)
    } else if let Some(w) = wait {
        Some(w.max(50))
    } else if !res.idle || site != record.home {
        Some(POLL_MS)
    } else {
        None
    })
}

/// The owner's store on `site`, by name, with the site's location hint.
fn stub(env: &Env, owner: AccountId, site: SiteId) -> Result<Stub> {
    let s = SiteEnv::read(env)?;
    let region = s.sites.get(site).ok_or_else(|| Error::RustError(format!("unknown site {site:?}")))?.region;
    env.durable_object(&format!("HOOKSEQ_{}", site.0))?
        .get_by_name_with_location_hint(&format!("hookseq:{}", owner.0.0), region.hint())
}

/// Sends `req` to the store on `site`, or, when that site does not answer, on the record's next sites in order.
/// Answers the site that took it.
async fn send(
    h: &Rc<Host<Repo>>,
    key: &str,
    record: &PlacementRecord,
    owner: AccountId,
    site: SiteId,
    req: &RecordReq,
) -> std::result::Result<(SiteId, RecordRes), String> {
    let body = codec::to_bytes(req);
    let order: Vec<SiteId> = std::iter::once(site).chain(record.sites.iter().copied().filter(|s| *s != site)).collect();
    let mut last = String::new();
    for s in order {
        match crate::host::within(STORE_FOR, call(&h.env, owner, s, &body)).await {
            Some(Ok(r)) => return Ok((s, r)),
            Some(Err(e)) => last = format!("site {}: {e}", s.0),
            None => last = format!("site {}: no answer in {} s", s.0, STORE_FOR.as_secs()),
        }
        console_error!("webhooks store {last}");
        if s == record.home {
            CONSUMERS.with(|m| {
                if let Some(c) = m.borrow_mut().get_mut(key) {
                    c.home_failed_at = now_ms();
                }
            });
        }
        // A store on another site holds none of what this one did: ask it from the start.
        if !req.rows.is_empty() || req.replay || req.from > Index(0) {
            CONSUMERS.with(|m| {
                if let Some(c) = m.borrow_mut().get_mut(key) {
                    c.store = None;
                }
            });
            return Err(last);
        }
    }
    Err(format!("no store answered: {last}"))
}

async fn call(env: &Env, owner: AccountId, site: SiteId, body: &[u8]) -> std::result::Result<RecordRes, String> {
    let stub = stub(env, owner, site).map_err(|e| e.to_string())?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(js_sys::Uint8Array::from(body).into()));
    let req = Request::new_with_init("https://hookseq/record", &init).map_err(|e| e.to_string())?;
    let mut r = stub.fetch_with_request(req).await.map_err(|e| e.to_string())?;
    match r.status_code() {
        200 => r.json().await.map_err(|e| e.to_string()),
        s => Err(format!("{s} {}", r.text().await.unwrap_or_default())),
    }
}

/// The accounts a batch's payloads name: the owner's `Authz` (permissions, kind, the repository's name) through
/// `fresh`, and the logins of the accounts the tiers name, as the edge reads them (`AccountLogin` rows on each
/// account's `Names` shard, at most `LOGIN_SHARDS_MAX` shards).
async fn directory(
    h: &Rc<Host<Repo>>,
    repo: RepoId,
    owner: AccountId,
    record: &PlacementRecord,
) -> std::result::Result<HostedDir, String> {
    let env = &h.env;
    let (v, _) =
        crate::lookup::call(env, ChainId::Authz(owner), record, "/read", &json!({ "q": { "dep": true } })).await?;
    let bytes = v.get("value").and_then(Value::as_str).and_then(crate::host::unhex).ok_or("no Authz fold")?;
    let authz: graftyard_identity::AuthzFold = codec::from_bytes(&bytes).map_err(|e| e.to_string())?;
    let name = authz.id_name(repo).ok_or("the owner's Authz names no such repository")?.as_str().to_owned();
    let mut named = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or("not joined")?;
        let f = &c.m.group_fold().fold;
        graftyard_api::hosted::accounts(&f.core, &f.meta)
    };
    named.insert(owner);
    let mut by_shard: BTreeMap<_, Vec<i64>> = BTreeMap::new();
    for a in named {
        by_shard.entry(graftyard_identity::account_shard(a)).or_default().push(a.0.0);
    }
    let mut logins = BTreeMap::new();
    let reads = by_shard.into_iter().take(graftyard_api::narrow::LOGIN_SHARDS_MAX).map(|(shard, ids)| async move {
        let chain = ChainId::Names(shard);
        let record = crate::release::system_record(env, chain).await?;
        crate::lookup::call(env, chain, &record, "/read", &json!({ "q": { "accountLogins": ids } })).await
    });
    for (v, _) in futures_util::future::join_all(reads).await.into_iter().flatten() {
        for (id, l) in v.pointer("/value/accountLogins").and_then(Value::as_object).into_iter().flatten() {
            if let (Ok(id), Some(l)) = (id.parse::<i64>(), l.as_str()) {
                logins.insert(id, l.to_owned());
            }
        }
    }
    let login = logins.get(&owner.0.0).cloned().ok_or("the owner has no login row")?;
    let at = RepoRef { id: repo, owner: login, owner_id: owner, name };
    Ok(HostedDir { home: Some(at), authz: Some(authz), logins, ..HostedDir::default() })
}

/// The rows of entries `from + 1 ..= through`, rendered against the leader's fold, cut at `BATCH_BYTES`; the hooks;
/// and the last entry rendered.
#[allow(clippy::type_complexity)]
fn render(
    h: &Rc<Host<Repo>>,
    repo: RepoId,
    dir: &HostedDir,
    from: Index,
    through: Index,
) -> std::result::Result<(Vec<NewDelivery>, Vec<(HookId, HookRecord)>, Index), String> {
    let host = graftyard_api::wire::Host::new(&api_host(&h.env), false).map_err(|e| e.to_string())?;
    let core = h.core.borrow();
    let c = core.as_ref().ok_or("not joined")?;
    let fold = &c.m.group_fold().fold;
    // An issue's payload reads the metadata tier, which a new leader loads lazily.
    if !fold.meta.loaded() {
        return Err("the metadata tier is loading".into());
    }
    let at = dir.home.clone().ok_or("no repository")?;
    let view = graftyard_api::view::Repo { host: &host, at, core: &fold.core, meta: &fold.meta, dir };
    let sql = h.state.storage().sql();
    let first = fold.first_index();
    let mut rows = Vec::new();
    let mut bytes = 0;
    let mut last = from;
    for i in from.0 + 1..=through.0 {
        // Entries archived before the hook existed predate it; `plan` skips them by the hook's creation time anyway.
        if Index(i) < first {
            last = Index(i);
            continue;
        }
        let e = match c.m.entry_at(Index(i)) {
            Some(e) => e.clone(),
            None => crate::host::stored_entry::<Repo>(&sql, Index(i))
                .ok_or_else(|| format!("entry {i} is in neither the member nor the stored log"))?,
        };
        let planned = plan(&fold.core, repo, GENERATION, &e, |w| {
            let row = w.target.and_then(|t| hook_row(&fold.core, t).map(|r| (t, r)));
            let v = payload(&view, w, row.as_ref().map(|(t, r)| (*t, r)), e.at)?;
            serde_json::to_vec(&v).ok()
        });
        bytes += planned.iter().map(|r| r.body.len()).sum::<usize>();
        rows.extend(planned);
        last = Index(i);
        if bytes >= BATCH_BYTES {
            break;
        }
    }
    Ok((rows, hooks(&fold.core), last))
}

/// The API host payload URLs name: the edge's `HOST`, which the site scripts get as `GRAFTYARD_HOST`.
fn api_host(env: &Env) -> String {
    env.var("GRAFTYARD_HOST").map(|v| v.to_string()).unwrap_or_else(|_| "graftyard.dev".into())
}

/// When the entry at `i` was appended, by the leader's clock, if it carries an event other than `HookCursor`; from the
/// member's log or the stored log.
fn event_time(h: &Rc<Host<Repo>>, i: Index) -> Option<Timestamp> {
    let core = h.core.borrow();
    let c = core.as_ref()?;
    let e = match c.m.entry_at(i) {
        Some(e) => e.clone(),
        None => crate::host::stored_entry::<Repo>(&h.state.storage().sql(), i)?,
    };
    match &e.body {
        graftyard_raft::EntryBody::Events(evs)
            if evs.iter().any(|v| !matches!(v, git::repo::RepoEvent::HookCursor(_))) =>
        {
            Some(e.at)
        }
        _ => None,
    }
}

/// `HookCursorCmd` through this member, waiting for its outcome.
async fn commit_cursor(h: &Rc<Host<Repo>>, cursor: HookCursor) -> std::result::Result<(), String> {
    let rid = graftyard_lookup::request_id();
    let cmd = git::repo::RepoCmd::HookCursor(HookCursorCmd { cursor, request: rid });
    let admitted = h.input(|m, t| m.commit(t, rid, cmd)).map_err(|e| e.to_string())?;
    let a = admitted.map_err(|e| crate::host::call_error_json(&e).to_string())?;
    h.served();
    match crate::host::within(std::time::Duration::from_secs(10), h.wait_ticket(a.ticket, None)).await {
        Some(Ok(_)) => Ok(()),
        Some(Err(_)) => Err("member reset".into()),
        None => Err("the cursor did not commit in 10 s".into()),
    }
}

// ---- the object ----

#[durable_object(alarm)]
pub struct HookSequencer {
    state: Rc<State>,
    env: Env,
    seq: Rc<RefCell<Option<Sequencer>>>,
    pruned_at: std::cell::Cell<u64>,
    /// A `save` of attempt reports is scheduled (`save_soon`).
    save_due: Rc<std::cell::Cell<bool>>,
}

/// A delivery's payload is kept in `body`, apart from its `delivery` row, and the object holds in memory only the
/// bodies of deliveries not yet done: on graft-m4a at 10 events/s over 50 hooks, a store that held every 11.7 KB body
/// in its rows and in memory reached about 7,300 deliveries (85 MB of bodies) in 30 s, and its isolate then failed
/// inside `transactionSync` ("Critical", "Reinitializing Wasm application") 21 times in 13 minutes, losing the
/// attempts it was running each time [measured, `tests/m4/results/m4-wp1-hosted.md`, BF-57 run 2]. A done delivery's
/// body is read back from SQLite only for `GET .../deliveries/{id}` and a manual redelivery.
const SCHEMA: [&str; 5] = [
    "CREATE TABLE IF NOT EXISTS core (k INTEGER PRIMARY KEY CHECK (k = 0), v BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS delivery (guid BLOB PRIMARY KEY, row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS born (k INTEGER PRIMARY KEY CHECK (k = 0), at INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS body (guid BLOB PRIMARY KEY, b BLOB NOT NULL)",
    // One body per entry and slot, which every hook of that webhook shares (`body_key`), kept while the store holds a
    // delivery of it. The per-delivery `body` table above is read only for rows written before it.
    "CREATE TABLE IF NOT EXISTS shared_body (k BLOB PRIMARY KEY, b BLOB NOT NULL)",
];

impl DurableObject for HookSequencer {
    fn new(state: State, env: Env) -> Self {
        let sql = state.storage().sql();
        for s in SCHEMA {
            sql.exec(s, None).expect("hookseq schema");
        }
        HookSequencer {
            state: Rc::new(state),
            env,
            seq: Rc::default(),
            pruned_at: std::cell::Cell::new(0),
            save_due: Rc::default(),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if crate::site_down(&self.env) {
            return Response::error("site down", 503);
        }
        let url = req.url()?;
        self.load()?;
        match (req.method(), url.path()) {
            (Method::Post, "/record") => {
                let r: RecordReq =
                    codec::from_bytes(&req.bytes().await?).map_err(|e| Error::RustError(e.to_string()))?;
                let res = self.record(r)?;
                Response::from_json(&res)
            }
            (Method::Post, "/report") => {
                let r: Report = codec::from_bytes(&req.bytes().await?).map_err(|e| Error::RustError(e.to_string()))?;
                let next = self.report(r).await?;
                Response::from_bytes(codec::to_bytes(&next))
            }
            (Method::Post, "/route") => {
                let r: RouteReq = req.json().await?;
                let now = Timestamp(now_ms());
                let mut random = [0u8; 16];
                for b in random.iter_mut() {
                    *b = (js_sys::Math::random() * 256.0) as u8;
                }
                let tail: Vec<&str> = r.tail.iter().map(String::as_str).collect();
                // One delivery's page shows its payload, which a done delivery keeps only in SQLite.
                let one = match tail.as_slice() {
                    ["deliveries", n] => n
                        .parse::<u64>()
                        .ok()
                        .and_then(|n| self.seq.borrow().as_ref().and_then(|s| s.get(r.id, n).map(|(d, _)| d.guid))),
                    _ => None,
                };
                if let Some(g) = one {
                    self.ensure_body(&g)?;
                }
                let reply = {
                    let mut g = self.seq.borrow_mut();
                    let seq = g.as_mut().expect("loaded");
                    sequencer_route(seq, &r.cx, r.id, &r.row, &r.method, &tail, &r.query, now, random)
                };
                self.save()?;
                self.kick();
                match reply {
                    Some(reply) => Response::from_json(&WireReply::of(&reply)),
                    None => Response::error("not a sequencer route", 404),
                }
            }
            _ => Response::error("not found", 404),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        if crate::site_down(&self.env) {
            return Response::empty();
        }
        self.load()?;
        self.run().await?;
        Response::empty()
    }
}

/// One attempt as a stateless invocation of this site script runs it (`attempt_handled`): the request the store built,
/// signed, so the invocation never holds the hook's secret, and where to report.
#[derive(Serialize, Deserialize)]
pub struct Job {
    owner: AccountId,
    store: SiteId,
    attempt: u64,
    guid: Guid,
    hook: HookId,
    redelivery: bool,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// `/report`'s body: one attempt's result. `more` says the invocation may run the queue's next attempt, which the
/// store then answers with.
#[derive(Serialize, Deserialize)]
struct Report {
    guid: Guid,
    hook: HookId,
    attempt: u64,
    redelivery: bool,
    took_ms: u64,
    status: Option<u16>,
    timed_out: bool,
    response: Option<String>,
    more: bool,
}

/// `/route`'s body, from the edge: the sequencer route and what it needs of the repository.
#[derive(Serialize, Deserialize)]
pub struct RouteReq {
    pub cx: RouteCtx,
    pub id: HookId,
    pub row: HookRow,
    pub method: String,
    pub tail: Vec<String>,
    pub query: String,
}

impl HookSequencer {
    fn owner(&self) -> AccountId {
        let name = self.state.id().name().unwrap_or_default();
        AccountId(DbId(name.strip_prefix("hookseq:").and_then(|n| n.parse().ok()).unwrap_or(0)))
    }

    /// Reads the store from SQLite once per isolate. A store born before `GRAFTYARD_HOOKSEQ_RESET_BEFORE` (ms, a site
    /// variable a site day sets) starts empty: the operator's way to discard a site's stores as if lost with it, after
    /// which each consumer refills its chain from the committed `replay_from`.
    fn load(&self) -> Result<()> {
        if self.seq.borrow().is_some() {
            return Ok(());
        }
        let sql = self.state.storage().sql();
        let now = now_ms();
        let reset_before: u64 =
            self.env.var("GRAFTYARD_HOOKSEQ_RESET_BEFORE").ok().and_then(|v| v.to_string().parse().ok()).unwrap_or(0);
        let born = crate::host::rows(&sql, "SELECT at FROM born WHERE k = 0", vec![])?
            .first()
            .map(|r| crate::host::int(&r[0]))
            .transpose()?;
        if born.is_none_or(|b| (b as u64) < reset_before) {
            crate::host::commit_tx(
                &self.state.storage(),
                vec![
                    ("DELETE FROM core", vec![]),
                    ("DELETE FROM delivery", vec![]),
                    ("DELETE FROM body", vec![]),
                    ("DELETE FROM shared_body", vec![]),
                    ("INSERT OR REPLACE INTO born (k, at) VALUES (0, ?)", vec![crate::host::i(now)]),
                ],
            )
            .map_err(Error::RustError)?;
        }
        let core = crate::host::rows(&sql, "SELECT v FROM core WHERE k = 0", vec![])?;
        let seq = match core.first() {
            Some(r) => {
                let c: Sequencer =
                    codec::from_bytes(crate::host::blob(&r[0])?).map_err(|e| Error::RustError(e.to_string()))?;
                // One row at a time, keeping no done delivery's body: rows written before bodies had their own table
                // carry theirs inline, and reading every one at once kept the store's isolate failing in `save` in
                // BF-57's run 3, with about 14,000 of them from runs 1 and 2.
                let mut ds: Vec<HookDelivery> = Vec::new();
                for r in sql.exec("SELECT row FROM delivery", None)?.raw() {
                    let r = r?;
                    let mut d: HookDelivery =
                        codec::from_bytes(crate::host::blob(&r[0])?).map_err(|e| Error::RustError(e.to_string()))?;
                    if d.done {
                        d.body = Box::default();
                    }
                    ds.push(d);
                }
                let undone: Vec<(Guid, Vec<u8>)> =
                    ds.iter().filter(|d| !d.done).map(|d| (d.guid, body_key(d))).collect();
                let mut seq = Sequencer::restore(c, ds);
                for (g, k) in undone {
                    if let Some(b) = read_body(&sql, &g, k)? {
                        seq.set_body(&g, b);
                    }
                }
                seq
            }
            None => Sequencer::new(self.owner(), Timestamp(now)),
        };
        *self.seq.borrow_mut() = Some(seq);
        Ok(())
    }

    /// Writes the core row and the deliveries that changed, in one transaction.
    fn save(&self) -> Result<()> {
        save_seq(&self.state, &self.seq).map_err(Error::RustError)
    }

    fn record(&self, r: RecordReq) -> Result<RecordRes> {
        let now = Timestamp(now_ms());
        let res = {
            let mut g = self.seq.borrow_mut();
            let s = g.as_mut().expect("loaded");
            if r.drop {
                s.drop_chain(r.chain);
            }
            let held = s.recorded(r.chain);
            let probe = r.drop || (r.rows.is_empty() && r.from == Index(0) && r.through == Index(0) && !r.replay);
            let accepted = !probe && (r.replay || held.is_some_and(|h| h >= r.from));
            if accepted {
                s.set_chain_targets(r.chain, &r.hooks);
                s.record(r.chain, r.through, r.rows, now);
            }
            RecordRes {
                accepted: accepted || probe,
                recorded: s.recorded(r.chain),
                cursor: s.cursor(r.chain),
                idle: s.idle(),
            }
        };
        self.save()?;
        self.kick();
        Ok(res)
    }

    /// Asks for the alarm at once, which starts attempts. Attempts start only from the alarm, never from a request's
    /// handler: a Durable Object's calls are charged to the newest request it received, so an attempt started from the
    /// previous attempt's report would run one hop deeper each time, and the runtime refuses a call past its depth
    /// limit (`tests/m3/results/m3-wp5.md`, "Where the depth comes from").
    fn kick(&self) {
        let state = self.state.clone();
        spawn_local(async move {
            let s = state.storage();
            let due = s.get_alarm().await.ok().flatten();
            if due.is_none_or(|d| d > now_ms() as i64) {
                let _ = s.set_alarm(std::time::Duration::from_millis(0)).await;
            }
        });
    }

    /// The alarm's work: hands what the sequencer starts to stateless invocations of this site script
    /// (`attempt_handled`), without waiting for them, and comes back in a second while anything runs (retries and
    /// re-sends), or in an hour to prune finished deliveries.
    async fn run(&self) -> Result<()> {
        let now = Timestamp(now_ms());
        let starts = {
            let mut g = self.seq.borrow_mut();
            let Some(s) = g.as_mut() else { return Ok(()) };
            if now.0 >= self.pruned_at.get() + PRUNE_EVERY_MS {
                self.pruned_at.set(now.0);
                s.prune(now);
                // A shared body goes once the store holds none of its deliveries.
                let held: std::collections::HashSet<Vec<u8>> = s.deliveries().map(body_key).collect();
                let sql = self.state.storage().sql();
                let mut gone: Vec<Vec<u8>> = Vec::new();
                for r in sql.exec("SELECT k FROM shared_body", None)?.raw() {
                    let k = crate::host::blob(&r?[0])?.to_vec();
                    if !held.contains(&k) {
                        gone.push(k);
                    }
                }
                for k in gone {
                    sql.exec("DELETE FROM shared_body WHERE k = ?", vec![k.into()])?;
                }
            }
            s.start(now)
        };
        self.save()?;
        for s in starts {
            let Some(job) = self.job(&s).await? else { continue };
            let env = self.env.clone();
            self.state.wait_until(async move {
                if let Err(e) = dispatch(&env, &job).await {
                    // Unreported, the attempt is re-sent under its GUID after `RESEND_AFTER_MS`.
                    console_error!("hookseq attempt {}: {e}", job.attempt);
                }
            });
        }
        let (idle, empty) = self.seq.borrow().as_ref().map_or((true, true), |s| (s.idle(), s.is_empty()));
        let ms = if !idle {
            1_000
        } else if !empty {
            3_600_000
        } else {
            return Ok(());
        };
        let s = self.state.storage();
        if s.get_alarm().await?.is_none_or(|d| d > now_ms() as i64 + ms) {
            s.set_alarm(std::time::Duration::from_millis(ms as u64)).await?;
        }
        Ok(())
    }

    /// The attempt `s` as a job: the request for the delivery's hook as it is now, signed here, so the invocation that
    /// sends it never holds the secret. `None` once the attempt is reported failed here: its delivery or hook is gone,
    /// or the secret could not be unwrapped (never an unsigned attempt).
    async fn job(&self, s: &Start) -> Result<Option<Job>> {
        self.ensure_body(&s.guid)?;
        let built = {
            let g = self.seq.borrow();
            let seq = g.as_ref().expect("loaded");
            match (seq.delivery(&s.guid), seq.target(s.hook)) {
                (Some(d), Some(t)) => Some((d.clone(), t.clone())),
                _ => None,
            }
        };
        let failed = match built {
            None => "the delivery or its hook is gone".to_owned(),
            Some((d, t)) => match self.secret(&t).await {
                Err(e) => format!("the hook secret could not be unwrapped: {e}"),
                Ok(secret) => {
                    let target = ("repository", d.repo.map_or(0, |r| r.0.0));
                    let r = request(&api_host(&self.env), &d, &t, target, secret.as_ref());
                    return Ok(Some(Job {
                        owner: self.owner(),
                        store: SiteEnv::read(&self.env)?.id,
                        attempt: s.attempt,
                        guid: s.guid,
                        hook: s.hook,
                        redelivery: s.redelivery,
                        url: r.url,
                        headers: r.headers.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
                        body: r.body,
                    }));
                }
            },
        };
        self.seq.borrow_mut().as_mut().expect("loaded").report(
            s.attempt,
            Timestamp(now_ms()),
            None,
            false,
            Some(&failed),
        );
        self.save_soon();
        Ok(None)
    }

    /// One attempt's report from the invocation that sent it. While that invocation may go on (`more`), answers the
    /// queue's next attempt for it to send at once (`Sequencer::next_in_queue`), so a queue busy with one event after
    /// another does not wait a whole `start` round between its attempts, which held healthy first attempts to a p99 of
    /// 6.8 s in BF-57's run 2. Other queues' attempts start from the alarm, never from here: an object's calls are
    /// charged to the newest request it received, so a call made from a report would run one hop deeper each time
    /// (`tests/m3/results/m3-wp5.md`, "Where the depth comes from"). The answer is a response, not a call.
    async fn report(&self, r: Report) -> Result<Option<Job>> {
        let now = Timestamp(now_ms());
        let prev = Start { guid: r.guid, hook: r.hook, attempt: r.attempt, redelivery: r.redelivery };
        let mut next = {
            let mut g = self.seq.borrow_mut();
            let seq = g.as_mut().expect("loaded");
            seq.report(r.attempt, now, r.status, r.timed_out, r.response.as_deref());
            if r.more { seq.next_in_queue(&prev, now) } else { None }
        };
        console_log!(
            "hookseq {} {} hook {} attempt {} {:?} {}ms",
            self.owner().0.0,
            r.guid,
            r.hook.0.0,
            r.attempt,
            r.status,
            r.took_ms
        );
        self.save_soon();
        let mut job = None;
        while let Some(n) = next.take() {
            job = self.job(&n).await?;
            if job.is_some() {
                break;
            }
            next = self.seq.borrow_mut().as_mut().expect("loaded").next_in_queue(&n, Timestamp(now_ms()));
        }
        self.kick();
        Ok(job)
    }

    /// Writes what changed within `REPORT_SAVE_MS`, in one transaction with every other report of the window.
    fn save_soon(&self) {
        if self.save_due.replace(true) {
            return;
        }
        let (state, seq, due) = (self.state.clone(), self.seq.clone(), self.save_due.clone());
        spawn_local(async move {
            worker::Delay::from(std::time::Duration::from_millis(REPORT_SAVE_MS)).await;
            due.set(false);
            if let Err(e) = save_seq(&state, &seq) {
                console_error!("hookseq save: {e}");
                // The writes failed: reset the object, as a failed synchronous `save` does through its error.
                crate::host::abort(&state, "graftyard hookseq storage write failed");
            }
        });
    }

    /// Puts a delivery's body back in memory from `body` when it is not there: a done delivery's, for a manual
    /// redelivery or `GET .../deliveries/{id}`.
    fn ensure_body(&self, guid: &Guid) -> Result<()> {
        let mut g = self.seq.borrow_mut();
        let seq = g.as_mut().expect("loaded");
        let key = seq.delivery(guid).filter(|d| d.body.is_empty()).map(body_key);
        if let Some(k) = key
            && let Some(b) = read_body(&self.state.storage().sql(), guid, k)?
        {
            seq.set_body(guid, b);
        }
        Ok(())
    }

    /// The hook's secret, unwrapped by `graft-ops` once per isolate.
    async fn secret(&self, t: &HookRecord) -> std::result::Result<Option<HookSecret>, String> {
        let Some(w) = &t.secret else { return Ok(None) };
        let key = codec::to_bytes(w);
        if let Some(s) = SECRETS.with(|m| m.borrow().get(&key).cloned()) {
            return Ok(Some(s));
        }
        let s = unwrap(&self.env, w).await?;
        SECRETS.with(|m| m.borrow_mut().insert(key, s.clone()));
        Ok(Some(s))
    }
}

/// `HookSequencer::save`: the core row and the deliveries that changed, in one transaction.
fn save_seq(state: &State, seq: &RefCell<Option<Sequencer>>) -> std::result::Result<(), String> {
    let mut g = seq.borrow_mut();
    let Some(s) = g.as_mut() else { return Ok(()) };
    let mut stmts: Vec<crate::family::Stmt> = Vec::new();
    let mut done = Vec::new();
    for (guid, d) in s.take_dirty() {
        let key: SqlStorageValue = guid.0.to_vec().into();
        match d {
            Some(d) => {
                // A delivery's first write carries its body, which goes to `shared_body` once per entry and slot; the
                // row never does.
                if d.attempts.is_empty() && !d.body.is_empty() {
                    stmts.push((
                        "INSERT OR IGNORE INTO shared_body (k, b) VALUES (?, ?)",
                        vec![body_key(d).into(), d.body.to_vec().into()],
                    ));
                }
                let row = HookDelivery { body: Box::default(), ..d.clone() };
                stmts.push((
                    "INSERT OR REPLACE INTO delivery (guid, row) VALUES (?, ?)",
                    vec![key, codec::to_bytes(&row).into()],
                ));
                if d.done {
                    done.push(guid);
                }
            }
            None => {
                stmts.push(("DELETE FROM delivery WHERE guid = ?", vec![key.clone()]));
                stmts.push(("DELETE FROM body WHERE guid = ?", vec![key]));
            }
        }
    }
    for g in done {
        s.take_body(&g);
    }
    stmts.push(("INSERT OR REPLACE INTO core (k, v) VALUES (0, ?)", vec![codec::to_bytes(&*s).into()]));
    crate::host::commit_tx(&state.storage(), stmts)
}

/// Where a delivery's body is kept in `shared_body`: its (chain, index, slot), which every hook of one webhook shares,
/// or a ping's own GUID. One copy per entry and slot: graft-m4a's bf57b wrote 20 copies of each 11.7 KB payload, about
/// 234 KB an entry, and an attempt whose POST the output gate held behind such a `/record` write took 114 ms at p50
/// against 36 ms with none (BF-57 run 10, `tests/m4/results/m4-wp1-hosted.md`) [measured].
fn body_key(d: &HookDelivery) -> Vec<u8> {
    match d.entry {
        Some(e) => codec::to_bytes(&e),
        None => d.guid.0.to_vec(),
    }
}

/// A delivery's body: from `shared_body` by `key`; for one written before that table, from `body`, or from before
/// that, the row itself.
fn read_body(sql: &SqlStorage, guid: &Guid, key: Vec<u8>) -> Result<Option<Box<[u8]>>> {
    let rows = crate::host::rows(sql, "SELECT b FROM shared_body WHERE k = ?", vec![key.into()])?;
    if let Some(r) = rows.first() {
        return Ok(Some(crate::host::blob(&r[0])?.into()));
    }
    let key: SqlStorageValue = guid.0.to_vec().into();
    let rows = crate::host::rows(sql, "SELECT b FROM body WHERE guid = ?", vec![key.clone()])?;
    if let Some(r) = rows.first() {
        return Ok(Some(crate::host::blob(&r[0])?.into()));
    }
    let rows = crate::host::rows(sql, "SELECT row FROM delivery WHERE guid = ?", vec![key])?;
    let Some(r) = rows.first() else { return Ok(None) };
    let d: HookDelivery = codec::from_bytes(crate::host::blob(&r[0])?).map_err(|e| Error::RustError(e.to_string()))?;
    Ok((!d.body.is_empty()).then_some(d.body))
}

async fn unwrap(env: &Env, w: &Wrapped) -> std::result::Result<HookSecret, String> {
    let ops = env.service("OPS").map_err(|_| "no graft-ops binding")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(serde_json::to_string(w).map_err(|e| e.to_string())?.into()));
    let mut r = ops.fetch("https://graft-ops/unwrap", Some(init)).await.map_err(|e| e.to_string())?;
    if r.status_code() != 200 {
        return Err(format!("graft-ops answered {}", r.status_code()));
    }
    let v: Value = r.json().await.map_err(|e| e.to_string())?;
    let hex = v["secret"].as_str().ok_or("no secret")?;
    Ok(HookSecret(crate::host::unhex(hex).ok_or("not hex")?.into()))
}

/// POSTs one attempt under `ATTEMPT_DEADLINE_MS`: the status, whether it timed out, and the response body's start.
async fn post(r: &Job) -> (Option<u16>, bool, Option<String>) {
    let headers = Headers::new();
    for (k, v) in &r.headers {
        let _ = headers.set(k, v);
    }
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_headers(headers).with_body(Some(js_sys::Uint8Array::from(&r.body[..]).into()));
    let req = match Request::new_with_init(&r.url, &init) {
        Ok(q) => q,
        Err(e) => return (None, false, Some(e.to_string())),
    };
    let ctrl = AbortController::default();
    let signal = ctrl.signal();
    let deadline = std::time::Duration::from_millis(ATTEMPT_DEADLINE_MS);
    let sent = async {
        let mut res = Fetch::Request(req).send_with_signal(&signal).await?;
        let status = res.status_code();
        let text = res.text().await.unwrap_or_default();
        Ok::<_, Error>((status, text))
    };
    match crate::host::within(deadline, sent).await {
        Some(Ok((status, text))) => (Some(status), false, Some(text.chars().take(1024).collect())),
        Some(Err(e)) => (None, false, Some(e.to_string())),
        None => {
            ctrl.abort();
            (None, true, None)
        }
    }
}

/// Where the store's alarm hands a job to a stateless invocation of this site script, over its own `SITE` binding.
pub const ATTEMPT_PATH: &str = "/hookseq/attempt";

async fn dispatch(env: &Env, job: &Job) -> std::result::Result<(), String> {
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(js_sys::Uint8Array::from(&codec::to_bytes(job)[..]).into()));
    let req = Request::new_with_init(&format!("https://site{ATTEMPT_PATH}"), &init).map_err(|e| e.to_string())?;
    let r = env.service("SITE").map_err(|e| e.to_string())?.fetch_request(req).await.map_err(|e| e.to_string())?;
    match r.status_code() {
        202 => Ok(()),
        s => Err(format!("the attempt invocation answered {s}")),
    }
}

/// The site script's fetch handler for `dispatch`: a delivery Worker (DESIGN, Shape, `HookSequencer`). It answers 202
/// at once and, under `wait_until`, POSTs the job, reports it to the owner's store, and sends whatever next job the
/// report answers, for at most `CHAIN_FOR_MS`. It keeps nothing: a job it loses is re-sent by the store under its GUID
/// after `RESEND_AFTER_MS`. Attempts here run outside the store object, whose own outgoing fetches and storage writes
/// held each POST: with 49 queues busy, an attempt's cycle took 184 to 301 ms at p50 inside one object and 84 to 89
/// ms through stateless invocations [measured, probes `graft-m4-probe-sb5` and `-sb7`, 2026-11-13,
/// `tests/m4/results/m4-wp1-hosted.md`].
pub async fn attempt_handled(mut req: Request, env: Env, ctx: &Context) -> Result<Response> {
    let first: Job = codec::from_bytes(&req.bytes().await?).map_err(|e| Error::RustError(e.to_string()))?;
    ctx.wait_until(async move {
        let t0 = now_ms();
        let mut job = Some(first);
        while let Some(j) = job.take() {
            let started = now_ms();
            let (status, timed_out, response) = post(&j).await;
            let r = Report {
                guid: j.guid,
                hook: j.hook,
                attempt: j.attempt,
                redelivery: j.redelivery,
                took_ms: now_ms() - started,
                status,
                timed_out,
                response,
                more: now_ms() < t0 + CHAIN_FOR_MS,
            };
            match send_report(&env, &j, &r).await {
                Ok(next) => job = next,
                // Unreported, the attempt is re-sent under its GUID after `RESEND_AFTER_MS`.
                Err(e) => console_error!("hookseq report {}: {e}", j.attempt),
            }
        }
    });
    Response::empty().map(|r| r.with_status(202))
}

async fn send_report(env: &Env, job: &Job, r: &Report) -> std::result::Result<Option<Job>, String> {
    let stub = stub(env, job.owner, job.store).map_err(|e| e.to_string())?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(js_sys::Uint8Array::from(&codec::to_bytes(r)[..]).into()));
    let req = Request::new_with_init("https://hookseq/report", &init).map_err(|e| e.to_string())?;
    let mut res = stub.fetch_with_request(req).await.map_err(|e| e.to_string())?;
    match res.status_code() {
        200 => codec::from_bytes(&res.bytes().await.map_err(|e| e.to_string())?).map_err(|e| e.to_string()),
        s => Err(format!("{s} {}", res.text().await.unwrap_or_default())),
    }
}
