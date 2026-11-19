//! The `Counts`, `Notify`, and `Search` consumers hosted (`design/DESIGN.md`, Shape, "Identity"; `design/SHARED.md`,
//! "Event fan-out"; `build/BUILD-PLAN.md`, M4-WP2). The pure halves are `graftyard_git::watch`, `inbox`, and
//! `search`.
//!
//! Each runs in band on its chain's leader alarm, as `Webhooks` does (`crate::hookseq`), with no queue hop: SHARED
//! names Queues for these consumers, and `PrSync`'s hosted run measured a queue delivery of p50 2.0 s and p90 5.1 s,
//! with outliers near 37 s (`decisions/DECISIONS.md`, "`PrSync`'s first attempt runs in band"), against BF-5's 10 s.
//! A consumer whose target does not answer keeps its cursor and tries again on a later alarm, so the cursor, not a
//! queue, is what makes delivery at least once.
//!
//! - **`Notify` and `Search`** (repository leader). From each consumer's committed `consumer_cursor`, a batch of at
//!   most `BATCH` committed entries is read from the leader's log. `Notify` resolves mentions through the `Names`
//!   login shards, sends each account its notices to its `Inbox` (`crate::inbox`), and commits
//!   `ConsumerCursorCmd { Notify }` after every `Inbox` took them. `Search` writes each document op to the repository
//!   hash's `SearchShard` of the owner's domain, at the shard's newest generation (`crate::search::map`), and commits
//!   `ConsumerCursorCmd { Search }`. A shard that answers `lost` is rebuilt under the next generation (`rebuild`) by
//!   the first consumer that meets it; until then that repository's `Search` cursor holds.
//! - **`Counts`** (`Identity` leader). From the account's `CountsCursor`, `counts_plan` gives each repository's watcher
//!   updates, committed as `RepoCmd::Counts` on that repository's chain, at most `COUNTS_BATCH_MAX` a command; then
//!   `IdentityOp::CountsCursor` on the account's own chain.
//! - **Off by default.** Every consumer runs only where the site variable `GRAFTYARD_ACTIVITY` is `1`, which a site day
//!   sets once `FormatsEnable` enabled 32.19 and 0x14.3; without it no 32.19 or 0x14.3 command is ever sent.
//!
//! No request path calls the Cloudflare API: every call is a Durable Object or R2 binding.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use graftyard_git as git;
use graftyard_git::inbox::Notice;
use graftyard_git::issues::Consumer;
use graftyard_git::search::{self, Domain, RepoInfo, SearchShard as Key};
use graftyard_git::watch::{COUNTS_BATCH_MAX, ConsumerCursorCmd, CountsCmd};
use graftyard_identity as id;
use graftyard_raft::{EntryBody, Index, LogEntry, Role, codec};
use graftyard_types::{AccountId, ChainId, DbId, PlacementRecord, RepoId, RequestId, Timestamp, Ulid};
use serde_json::{Value, json};
use worker::wasm_bindgen_futures::spawn_local;
use worker::*;

use crate::SiteEnv;
use crate::family::{Identity, Repo};
use crate::host::Host;

/// Entries one batch reads at most [guess].
const BATCH: u64 = 100;
/// After a failed round, the alarm comes back in this long.
const RETRY_MS: i64 = 2_000;

/// How long an owner's record and a repository's owner serve later rounds in this isolate [guess]. A moved owner's
/// stale record still finds its groups through the lookup's `moving_from` and registry steps.
const RECORD_FOR_MS: u64 = 60_000;

thread_local! {
    /// Per member object and leadership: whether a round is running.
    static BUSY: RefCell<HashMap<String, (u64, u64)>> = RefCell::default();
    /// Owner records and repository owners this isolate read, with when.
    static RECORDS: RefCell<HashMap<AccountId, (u64, PlacementRecord)>> = RefCell::default();
    static OWNERS: RefCell<HashMap<RepoId, (u64, AccountId)>> = RefCell::default();
}

/// `member::owner_record`, cached for RECORD_FOR_MS.
async fn record_of(env: &Env, a: AccountId) -> std::result::Result<PlacementRecord, String> {
    let now = crate::now_ms();
    if let Some(r) =
        RECORDS.with(|m| m.borrow().get(&a).filter(|(at, _)| now < at + RECORD_FOR_MS).map(|(_, r)| r.clone()))
    {
        return Ok(r);
    }
    let r = crate::member::owner_record(env, a).await?;
    RECORDS.with(|m| m.borrow_mut().insert(a, (now, r.clone())));
    Ok(r)
}

/// `member::repo_owner`, cached for RECORD_FOR_MS.
async fn owner_of(env: &Env, repo: RepoId) -> std::result::Result<AccountId, String> {
    let now = crate::now_ms();
    if let Some(o) = OWNERS.with(|m| m.borrow().get(&repo).filter(|(at, _)| now < at + RECORD_FOR_MS).map(|(_, o)| *o))
    {
        return Ok(o);
    }
    let o = crate::member::repo_owner(env, repo).await?;
    OWNERS.with(|m| m.borrow_mut().insert(repo, (now, o)));
    Ok(o)
}

pub fn enabled(env: &Env) -> bool {
    env.var("GRAFTYARD_ACTIVITY").is_ok_and(|v| v.to_string() == "1")
}

/// One round at a time per member and leadership.
fn start(key: &str, leadership: (u64, u64)) -> bool {
    BUSY.with(|b| {
        let mut b = b.borrow_mut();
        if b.get(key) == Some(&leadership) {
            return false;
        }
        b.insert(key.to_owned(), leadership);
        true
    })
}

fn done(key: &str) {
    BUSY.with(|b| b.borrow_mut().remove(key));
}

/// A request id derived from the chain, the consumer, and the batch's end, so a round run twice (a reset leader, a
/// timeout) commits once: the tag `0x4143` ("AC") in the top 16 bits, then 112 bits of SHA-256.
fn rid(parts: &str) -> RequestId {
    use sha2::Digest as _;
    let h = sha2::Sha256::new().chain_update(parts).finalize();
    let low = u128::from_be_bytes(h[..16].try_into().expect("16 bytes")) & ((1u128 << 112) - 1);
    RequestId(Ulid((0x4143u128 << 112) | low))
}

/// A command through this member, waiting for its outcome.
pub(crate) async fn commit_here<F: crate::family::Family>(h: &Rc<Host<F>>, rid: RequestId, cmd: F::C) -> Result<()> {
    let admitted = h.input(|m, t| m.commit(t, rid, cmd))?;
    let a = admitted.map_err(|e| Error::RustError(crate::host::call_error_json(&e).to_string()))?;
    h.served();
    match crate::host::within(std::time::Duration::from_secs(10), h.wait_ticket(a.ticket, None)).await {
        Some(Ok(Ok(_))) => Ok(()),
        Some(Ok(Err(e))) => Err(Error::RustError(format!("refused: {e:?}"))),
        Some(Err(_)) => Err("member reset".into()),
        None => Err("did not commit in 10 s".into()),
    }
}

// ---- Notify and Search, on a repository leader ----

pub async fn on_alarm(h: &Rc<Host<Repo>>) {
    if !enabled(&h.env) {
        return;
    }
    let found = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref().filter(|c| c.m.role() == Role::Leader) else { return };
        let ChainId::Repo(repo) = c.join.chain else { return };
        let fold = &c.m.group_fold().fold;
        if !fold.meta.loaded() {
            return;
        }
        let applied = c.m.state().applied;
        let n = git::tiers::consumer_cursor(&*fold.core, Consumer::Notify);
        let s = git::tiers::consumer_cursor(&*fold.core, Consumer::Search);
        if n >= applied && s >= applied {
            return;
        }
        let Some(info) = RepoInfo::of(repo, &*fold.core) else { return };
        (c.join.me.do_name.to_string(), (c.m.term().0, c.m.boot()), info, n, s, applied)
    };
    let (key, leadership, info, notify_from, search_from, applied) = found;
    if !start(&key, leadership) {
        return;
    }
    let w = Rc::downgrade(h);
    spawn_local(async move {
        let Some(h) = w.upgrade() else { return };
        let r = round(&h, info, notify_from, search_from, applied).await;
        done(&key);
        match r {
            Ok(true) => h.arm_in(0),
            Ok(false) => {}
            Err(e) => {
                console_error!("activity {}: {e}", info.repo.0.0);
                h.arm_in(RETRY_MS);
            }
        }
    });
}

/// The committed entries `from + 1 ..= through` of this member's log.
fn entries(h: &Rc<Host<Repo>>, from: Index, through: Index) -> Result<Vec<LogEntry<git::repo::RepoEvent>>> {
    let core = h.core.borrow();
    let c = core.as_ref().ok_or("not joined")?;
    let sql = h.state.storage().sql();
    let first = c.m.group_fold().fold.first_index();
    let mut out = Vec::new();
    for i in from.0.max(first.0.saturating_sub(1)) + 1..=through.0 {
        let e = match c.m.entry_at(Index(i)) {
            Some(e) => e.clone(),
            None => crate::host::stored_entry::<Repo>(&sql, Index(i))
                .ok_or_else(|| Error::RustError(format!("entry {i} is in neither the member nor the stored log")))?,
        };
        out.push(e);
    }
    Ok(out)
}

/// One round: a batch for each consumer behind `applied`. `Ok(true)` when more is left.
async fn round(h: &Rc<Host<Repo>>, info: RepoInfo, notify: Index, search_at: Index, applied: Index) -> Result<bool> {
    let chain = ChainId::Repo(info.repo);
    let mut more = false;
    if notify < applied {
        let through = Index(applied.0.min(notify.0 + BATCH));
        let es = entries(h, notify, through)?;
        let notices = notices_of(h, info.repo, &es).await?;
        for (a, by_entry) in notices {
            let record = record_of(&h.env, a).await.map_err(Error::RustError)?;
            for (index, ns) in by_entry {
                let body =
                    codec::to_bytes(&crate::inbox::ApplyReq { record: record.clone(), chain, index, notices: ns });
                crate::inbox::call(&h.env, a, &record, "/apply", body).await?;
            }
        }
        if es.iter().any(|e| !own(e)) {
            let request = rid(&format!("{chain}/n/{}", through.0));
            let cmd = ConsumerCursorCmd { consumer: Consumer::Notify, through, request };
            commit_here(h, request, git::repo::RepoCmd::ConsumerCursor(cmd)).await?;
            more |= through < applied;
        }
    }
    if search_at < applied {
        let through = Index(applied.0.min(search_at.0 + BATCH));
        let es = entries(h, search_at, through)?;
        if es.iter().any(|e| !own(e)) && search_round(h, info, &es).await? {
            let request = rid(&format!("{chain}/s/{}", through.0));
            let cmd = ConsumerCursorCmd { consumer: Consumer::Search, through, request };
            commit_here(h, request, git::repo::RepoCmd::ConsumerCursor(cmd)).await?;
            more |= through < applied;
        }
    }
    Ok(more)
}

/// An entry holding only `Notify`'s and `Search`'s own cursor events: passing it alone commits nothing, or each
/// cursor commit would owe the next one [inference: the in-memory gates commit cursors once per pass, so they never
/// met it].
fn own(e: &LogEntry<git::repo::RepoEvent>) -> bool {
    match &e.body {
        EntryBody::Events(evs) => evs.iter().all(|ev| {
            matches!(ev, git::repo::RepoEvent::ConsumerCursor { consumer: Consumer::Notify | Consumer::Search, .. })
        }),
        _ => false,
    }
}

/// Each account's notices from `es`, by entry, read against the leader's metadata tier now. A mention resolves through
/// the login's `Names` shard.
async fn notices_of(
    h: &Rc<Host<Repo>>,
    repo: RepoId,
    es: &[LogEntry<git::repo::RepoEvent>],
) -> Result<BTreeMap<AccountId, Vec<(Index, Vec<Notice>)>>> {
    // Every `@login` the batch mentions, resolved once.
    let mut wanted: Vec<String> = Vec::new();
    for e in es {
        let EntryBody::Events(evs) = &e.body else { continue };
        for ev in evs {
            if let Some(b) = body_of(ev) {
                for l in git::inbox::mentions(b) {
                    if !wanted.iter().any(|w| w.eq_ignore_ascii_case(l)) {
                        wanted.push(l.to_owned());
                    }
                }
            }
        }
    }
    let logins = resolve_logins(&h.env, &wanted).await;
    let core = h.core.borrow();
    let c = core.as_ref().ok_or("not joined")?;
    let meta = &c.m.group_fold().fold.meta;
    let login = |l: &str| logins.iter().find(|(k, _)| k.eq_ignore_ascii_case(l)).map(|(_, a)| *a);
    let mut out: BTreeMap<AccountId, Vec<(Index, Vec<Notice>)>> = BTreeMap::new();
    for e in es {
        let mut by: BTreeMap<AccountId, Vec<Notice>> = BTreeMap::new();
        for (a, n) in git::inbox::notices(repo, &**meta, e, &login) {
            by.entry(a).or_default().push(n);
        }
        for (a, ns) in by {
            out.entry(a).or_default().push((e.index, ns));
        }
    }
    Ok(out)
}

fn body_of(ev: &git::repo::RepoEvent) -> Option<&str> {
    use git::issues::{CommentEvent, ReviewEvent};
    use git::repo::RepoEvent as E;
    match ev {
        E::IssueOpened { capture, .. } => Some(&capture.body),
        E::PrOpened { capture, .. } => Some(&capture.issue.body),
        E::Comment(CommentEvent::Created { body, .. }) => Some(body),
        E::Review(ReviewEvent::Submitted { body, .. }) => Some(body),
        _ => None,
    }
}

/// Each login's account, from its login row on its `Names` shard; a login with no row is left out.
async fn resolve_logins(env: &Env, logins: &[String]) -> Vec<(String, AccountId)> {
    let reads = logins.iter().map(|l| async move {
        let key = id::Login::parse(l).ok()?.key();
        let chain = ChainId::Names(id::name_shard(&id::NameKey::Login(key)));
        let record = crate::release::system_record(env, chain).await.ok()?;
        let (v, _) = crate::lookup::call(env, chain, &record, "/read", &json!({ "q": { "login": l } })).await.ok()?;
        match serde_json::from_value::<Option<id::LoginRow>>(v.pointer("/value/login").cloned()?).ok()?? {
            id::LoginRow::Claimed { account, .. } => Some((l.clone(), account)),
            id::LoginRow::Former { .. } => None,
        }
    });
    futures_util::future::join_all(reads).await.into_iter().flatten().collect()
}

/// The owner's residency domain, from its placement record's name.
async fn domain_of(env: &Env, owner: AccountId) -> std::result::Result<Domain, String> {
    Ok(Domain::of(record_of(env, owner).await?.name))
}

/// `Search` over entries `from + 1 ..= through`: each document op to its shard. `Ok(false)` when the shard is lost
/// and was not rebuilt yet, so the cursor holds.
async fn search_round(h: &Rc<Host<Repo>>, info: RepoInfo, es: &[LogEntry<git::repo::RepoEvent>]) -> Result<bool> {
    let ops: Vec<crate::search::Op> = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or("not joined")?;
        let f = &c.m.group_fold().fold;
        let mut v = Vec::new();
        for e in es {
            for op in search::docs_for(info, &*f.meta, e) {
                v.push(match op {
                    search::DocOp::Put(d) => crate::search::Op::Put(d),
                    search::DocOp::Delete(n) => crate::search::Op::Delete { repo: info.repo, number: n },
                    search::DocOp::Repo => {
                        crate::search::Op::Repo { repo: info.repo, docs: search::all_docs(info, &*f.core, &*f.meta) }
                    }
                });
            }
        }
        v
    };
    if ops.is_empty() {
        return Ok(true);
    }
    let env = &h.env;
    let sites = SiteEnv::read(env)?.sites;
    let domain = domain_of(env, info.owner).await.map_err(Error::RustError)?;
    let shard = search::shard_of(info.repo);
    let generation = crate::search::map(env, &sites).await.get(&(domain, shard)).copied().unwrap_or(1);
    let k = Key { domain, shard, generation };
    match crate::search::call(env, &sites, &k, "/write", &json!({ "ops": ops })).await.map_err(Error::RustError)? {
        Ok(_) => {
            crate::search::mapped(env, &sites, &k).await.map_err(Error::RustError)?;
            Ok(true)
        }
        Err(why) if why.contains("lost") => {
            console_error!("search {}: {} lost, rebuilding", info.repo.0.0, k.do_name());
            rebuild(env, k).await.map_err(Error::RustError)?;
            Ok(false)
        }
        Err(why) => Err(Error::RustError(format!("search {}: {why}", k.do_name()))),
    }
}

/// Rebuilds lost shard `old` under the next generation on the domain's next site, from the metadata tier of every
/// repository whose owner is in `old.domain` and whose hash is `old.shard`, as each site's `MemberRegistry` lists
/// them, read on each repository's leader (`/activity {"docs": true}`). Then records the generation, so queries and
/// later writes use it. Answers the new shard and how many documents it wrote.
pub async fn rebuild(env: &Env, old: Key) -> std::result::Result<Value, String> {
    let sites = SiteEnv::read(env).map_err(|e| e.to_string())?.sites;
    let new = Key { generation: old.generation + 1, ..old };
    let mut repos: Vec<(RepoId, Vec<graftyard_types::SiteId>, u32)> = Vec::new();
    for s in sites.0.iter().filter(|s| !s.spare) {
        for shard in 0..graftyard_types::REGISTRY_SHARDS {
            let reg = graftyard_types::MemberRegistry { site: s.id, shard };
            let Ok(stub) = SiteEnv::read(env).and_then(|x| x.registry(env, s.id, &reg.do_name())) else { continue };
            let Ok(mut res) = stub.fetch_with_str(&format!("https://registry/rows?site={}", s.id.0)).await else {
                continue;
            };
            let rows: Vec<graftyard_types::RegistryRow> = res.json().await.unwrap_or_default();
            for r in rows {
                if let ChainId::Repo(repo) = r.chain
                    && search::shard_of(repo) == old.shard
                    && !repos.iter().any(|(x, ..)| *x == repo)
                {
                    repos.push((repo, r.sites.to_vec(), r.generation));
                }
            }
        }
    }
    let mut written = 0;
    let mut seen = Vec::new();
    for (repo, rsites, _) in repos {
        let owner = owner_of(env, repo).await?;
        if domain_of(env, owner).await? != old.domain {
            continue;
        }
        let record = record_of(env, owner).await?;
        debug_assert!(record.sites.iter().any(|s| rsites.contains(s)));
        let (v, _) =
            crate::lookup::call(env, ChainId::Repo(repo), &record, "/activity", &json!({ "docs": true })).await?;
        let docs: Vec<search::Doc> =
            serde_json::from_value(v["docs"].clone()).map_err(|e| format!("{repo:?} docs: {e}"))?;
        written += docs.len();
        let op = crate::search::Op::Repo { repo, docs };
        match crate::search::call(env, &sites, &new, "/write", &json!({ "ops": [op] })).await? {
            Ok(_) => seen.push(repo.0.0),
            Err(why) => return Err(format!("{}: {why}", new.do_name())),
        }
    }
    // A shard with no document of the domain still answers queries once built.
    crate::search::call(env, &sites, &new, "/write", &json!({ "ops": [] })).await?.map_err(|w| w.to_string())?;
    crate::search::call(env, &sites, &new, "/built", &json!({})).await?.map_err(|w| w.to_string())?;
    crate::search::put_map(env, &sites, &new).await?;
    Ok(json!({ "shard": new.do_name(), "site": new.site(&sites, &[]).map(|s| s.0), "repos": seen, "docs": written }))
}

/// The repository member's `/activity` route. On the leader, as `/read` is: `{"docs": true}` every document of the
/// repository (a shard rebuild), or `{"notices": account, "login": l, "after": i | "since": ms}` that account's notices
/// from the stored log (an `Inbox` rebuild). On any member, the operator's shard controls, which name the object by its
/// `do_name`: `{"lose": name}` (BF-68's kill), `{"rebuild": name}` (the rebuild the consumer would start), and
/// `{"shardStats": name, "owner": id}` (where the object runs and what it holds).
pub async fn route(h: &Rc<Host<Repo>>, body: &Value) -> Result<Value> {
    for op in ["lose", "rebuild", "shardStats"] {
        let Some(name) = body.get(op).and_then(Value::as_str) else { continue };
        let k = Key::parse(name).ok_or("not a shard name")?;
        let env = &h.env;
        let sites = SiteEnv::read(env)?.sites;
        return match op {
            "rebuild" => rebuild(env, k).await.map_err(Error::RustError),
            _ => {
                let path = if op == "lose" { "/lose" } else { "/stats" };
                let v = crate::search::call(env, &sites, &k, path, body).await.map_err(Error::RustError)?;
                Ok(v.unwrap_or_else(|why| json!({ "unavailable": why })))
            }
        };
    }
    leader_route(h, body)
}

fn leader_route(h: &Rc<Host<Repo>>, body: &Value) -> Result<Value> {
    let core = h.core.borrow();
    let c = core.as_ref().ok_or("member not joined")?;
    if c.m.role() != Role::Leader {
        return Ok(
            json!({ "error": crate::host::call_error_json(&graftyard_raft::CallError::NotLeader(c.m.not_leader())) }),
        );
    }
    let ChainId::Repo(repo) = c.join.chain else { return Err("not a repository".into()) };
    let f = &c.m.group_fold().fold;
    if !f.meta.loaded() {
        return Err("the metadata tier is loading".into());
    }
    if body.get("docs").is_some() {
        let info = RepoInfo::of(repo, &*f.core).ok_or("no settings")?;
        return Ok(json!({ "docs": search::all_docs(info, &*f.core, &*f.meta) }));
    }
    let account = AccountId(DbId(body["notices"].as_i64().ok_or("notices")?));
    let from = match (body["after"].as_u64(), body["since"].as_u64()) {
        (Some(i), _) => git::inbox::From::After(Index(i)),
        (None, Some(t)) => git::inbox::From::Since(Timestamp(t)),
        _ => return Err("after or since".into()),
    };
    // Mentions resolve to the account by its login, which the caller sends.
    let me = body["login"].as_str().unwrap_or_default().to_owned();
    let login = |l: &str| (!me.is_empty() && l.eq_ignore_ascii_case(&me)).then_some(account);
    let sql = h.state.storage().sql();
    let first = f.first_index();
    let applied = c.m.state().applied;
    let mut out = Vec::new();
    let start = match from {
        git::inbox::From::After(i) => i.0 + 1,
        git::inbox::From::Since(_) => first.0,
    };
    for i in start.max(first.0)..=applied.0 {
        let e = match c.m.entry_at(Index(i)) {
            Some(e) => e.clone(),
            None => match crate::host::stored_entry::<Repo>(&sql, Index(i)) {
                Some(e) => e,
                None => continue,
            },
        };
        if !git::inbox::replays(from, &e) {
            continue;
        }
        let ns: Vec<Notice> = git::inbox::notices(repo, &*f.meta, &e, &login)
            .into_iter()
            .filter(|(a, _)| *a == account)
            .map(|(_, n)| n)
            .collect();
        if !ns.is_empty() {
            out.push((e.index, ns));
        }
    }
    Ok(json!({ "notices": crate::host::hex(&codec::to_bytes(&out)) }))
}

/// An `Inbox` rebuild's replay of one repository chain, through its leader's `/activity`.
pub async fn replay_notices(
    env: &Env,
    repo: RepoId,
    account: AccountId,
    mut body: Value,
) -> std::result::Result<Vec<(Index, Vec<Notice>)>, String> {
    let owner = owner_of(env, repo).await?;
    let record = record_of(env, owner).await?;
    let login = login_of(env, account).await?;
    body["notices"] = json!(account.0.0);
    body["login"] = json!(login);
    let (v, _) = crate::lookup::call(env, ChainId::Repo(repo), &record, "/activity", &body).await?;
    let hex = v["notices"].as_str().ok_or_else(|| format!("{repo:?}: {v}"))?;
    codec::from_bytes(&crate::host::unhex(hex).ok_or("not hex")?).map_err(|e| e.to_string())
}

async fn login_of(env: &Env, a: AccountId) -> std::result::Result<String, String> {
    let chain = ChainId::Names(id::account_shard(a));
    let record = crate::release::system_record(env, chain).await?;
    let (v, _) =
        crate::lookup::call(env, chain, &record, "/read", &json!({ "q": { "accountLogins": [a.0.0] } })).await?;
    v.pointer(&format!("/value/accountLogins/{}", a.0.0))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or("no login".into())
}

// ---- Counts, on an Identity leader ----

pub async fn counts_on_alarm(h: &Rc<Host<Identity>>) {
    if !enabled(&h.env) {
        return;
    }
    let found = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref().filter(|c| c.m.role() == Role::Leader) else { return };
        let ChainId::Identity(account) = c.join.chain else { return };
        let fold = &c.m.group_fold().fold;
        let applied = c.m.state().applied;
        if applied <= fold.user().counts_cursor {
            return;
        }
        (c.join.me.do_name.to_string(), (c.m.term().0, c.m.boot()), account, applied)
    };
    let (key, leadership, account, applied) = found;
    if !start(&key, leadership) {
        return;
    }
    let w = Rc::downgrade(h);
    spawn_local(async move {
        let Some(h) = w.upgrade() else { return };
        let r = counts_round(&h, account, applied).await;
        done(&key);
        if let Err(e) = r {
            console_error!("counts {}: {e}", account.0.0);
            h.arm_in(RETRY_MS);
        }
    });
}

async fn counts_round(h: &Rc<Host<Identity>>, account: AccountId, applied: Index) -> Result<()> {
    let plan = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or("not joined")?;
        let fold = c.m.group_fold().fold.clone();
        let from = fold.user().counts_cursor;
        let sql = h.state.storage().sql();
        let mut es = Vec::new();
        for i in from.0 + 1..=applied.0 {
            match c.m.entry_at(Index(i)) {
                Some(e) => es.push(e.clone()),
                None => es.extend(crate::host::stored_entry::<Identity>(&sql, Index(i))),
            }
        }
        // Entries holding only the consumer's own `CountsCursor` owe nothing, not even a cursor.
        let only_own = es.iter().all(|e| match &e.body {
            EntryBody::Events(evs) => evs.iter().all(|ev| matches!(ev, id::IdentityEvent::CountsCursor(_))),
            _ => false,
        });
        if only_own {
            return Ok(());
        }
        git::watch::counts_plan(account, &fold, applied, &es)
    };
    'repos: for (repo, ups) in plan {
        let owner = owner_of(&h.env, repo).await.map_err(Error::RustError)?;
        let record = record_of(&h.env, owner).await.map_err(Error::RustError)?;
        for (i, chunk) in ups.chunks(COUNTS_BATCH_MAX).enumerate() {
            let request = rid(&format!("{}/c/{}/{}/{i}", ChainId::Identity(account), applied.0, repo.0.0));
            let cmd = git::repo::RepoCmd::Counts(CountsCmd { updates: chunk.to_vec(), request });
            let site = SiteEnv::read(&h.env)?;
            let b = graftyard_lookup::Bindings { env: &h.env, sites: &site.sites };
            let cmd = serde_json::to_value(&cmd)?;
            let done = match graftyard_lookup::commit_as(&b, ChainId::Repo(repo), &record, request, cmd).await {
                Ok(d) => d,
                // A purged repository has no counts to keep, and its failure must not hold the cursor (`repo-gone`).
                Err(e) if crate::member::repo_gone(&h.env, repo, &e).await => {
                    console_log!("counts {}: repo:{} is gone ({e}); its updates are dropped", account.0.0, repo.0.0);
                    continue 'repos;
                }
                Err(e) => return Err(Error::RustError(e)),
            };
            if done.get("ok").is_none() {
                return Err(Error::RustError(format!("counts on {repo:?}: {done}")));
            }
        }
    }
    let request = rid(&format!("{}/cursor/{}", ChainId::Identity(account), applied.0));
    let cmd = id::IdentityCmd {
        cred: graftyard_types::CredentialShard { account: None, key: None },
        op: id::IdentityOp::CountsCursor { through: applied },
    };
    commit_here(h, request, cmd).await
}
