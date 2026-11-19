//! `fanout` hosted (`design/DESIGN.md`, Shape, `NetworkLoad`, `fanout`, and "Git cache shards (derived)";
//! `build/BUILD-PLAN.md`, AM1-WP1, "the network leader's per-minute `fanout` commit and the shard's `NetworkLoad`
//! report"):
//!
//! - **Report.** A warm site's first shard of a network (`git#<site>#net:<repo>`, the one the edge has always called)
//!   reads its k shards' `/load` once a minute, combines them (`graftyard_git::combine`: the most loaded one's CPU),
//!   and posts the result to the network group's leader at `/fanout` ([`report_from_shard`]).
//! - **Grant.** The leader folds each warm site's latest report into `LoadReports`. While k is above 1 or `fanout`
//!   wants it above 1, it asks both warm sites' pool ledgers (`POST /pool/grant` on `git#<site>#pool:1`, a
//!   `graftyard_git::PoolLedger` in that object's SQLite) for the pool shards it wants, at once, and records
//!   `RepoEvent::Fanout { k }` (format 32.15) in an entry of its own when 1 plus the fewer either granted differs from
//!   the committed k. A ledger that does not answer keeps k as committed. One k serves both warm sites. The reports
//!   and the hysteresis live in the leader's isolate only, so a new leader starts from the committed k and at worst
//!   waits one more `FANOUT_UP_FOR` to step.
//! - **Use.** Every catch-up of the first shard reads k from the fold's `ranges`. Once a minute while it validates,
//!   the first shard renews the network's lease on its site's ledger (`POST /pool/renew`) and keeps the slots it
//!   answers, and its `/validate` forwards a push to the shard `push_validator` picks among the first shard and those
//!   slots, at most k - 1, by rendezvous on the push id. A pool shard (`git#<site>#pool:<n>`) catches up before it
//!   validates, like any shard, so it holds every commit the push could build on; if it cannot answer, the first
//!   shard validates the push itself.
//!
//! No API call or cron: the reports ride the shard's own validates, the grants are calls between this install's
//! objects, and the commit is the leader's own entry.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use graftyard_git::{self as git, LoadReports, NetworkLoad};
use graftyard_raft::Role;
use graftyard_types::{ChainId, NetworkId, PlacementRecord, SiteId, Timestamp};
use serde_json::{Value, json};
use worker::*;

use crate::family::Repo;
use crate::host::Host;
use crate::now_ms;

/// The shard reports this often [the design's "each minute"].
pub const REPORT_EVERY_MS: u64 = 60_000;

thread_local! {
    /// Per leading member (object name): the warm sites' latest reports and the hysteresis.
    static REPORTS: RefCell<HashMap<String, LoadReports>> = RefCell::new(HashMap::new());
    /// Per (network, site, pool slot): whether the slot's last `PoolWarm` catch-up answered 200, which gates a step
    /// up (`graftyard_git::warm_step`). A new leader starts with none, so its first step waits one more report.
    static WARM: RefCell<HashMap<(i64, u8, u16), bool>> = RefCell::new(HashMap::new());
}

/// The leader's `/fanout`: `{"site": n, "load": NetworkLoad}`. Answers the committed k, `stepped` when this report
/// moved it, and each warm site's grant when it asked the ledgers.
pub async fn report(h: &Rc<Host<Repo>>, body: Value) -> Result<Value> {
    let site = SiteId(body["site"].as_u64().ok_or("site")? as u8);
    let load: NetworkLoad = serde_json::from_value(body["load"].clone()).map_err(|e| e.to_string())?;
    let (key, g) = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or("member not joined")?;
        // A follower answers `notLeader`, as `/read` does, so the caller's lookup follows the leader. An error here
        // would be a 500, which lookup counts as a missed member and reports with `suspect` (hosted B0-34: 27 of 29
        // reports in its window ended so on the followers).
        if c.m.role() != Role::Leader {
            return Ok(not_leader(c.m.not_leader()));
        }
        (c.join.me.do_name.to_string(), Grant::of(c)?)
    };
    let now = Timestamp(now_ms());
    let (step, high, prior, reserved) = REPORTS.with(|r| {
        let mut r = r.borrow_mut();
        let e = r.entry(key.clone()).or_default();
        let prior = e.f;
        let step = e.report(g.warm, site, load, g.k, now);
        (step, e.high(), prior, e.reserved)
    });
    // While the load is above the up line, one slot past the k - 1 served is reserved and warmed ahead of the step;
    // once it is not, a held reservation is given back.
    let Some(want) = git::pool_reserve(g.k, step, high, reserved) else { return Ok(json!({ "k": g.k })) };
    let v = g.ask(h, want, step.unwrap_or(g.k), high).await?;
    let k = v["k"].as_u64().unwrap_or(u64::from(g.k));
    REPORTS.with(|r| r.borrow_mut().entry(key.clone()).or_default().reserved = high && u64::from(want) >= k);
    // A step `fanout` answered that waited on a cold slot: the high line keeps its start, so the step commits at the
    // first report after the slot answers caught up.
    if step.is_some_and(|s| s > g.k) && v["k"].as_u64() == Some(u64::from(g.k)) {
        REPORTS.with(|r| r.borrow_mut().entry(key).or_default().step_waited(prior));
    }
    Ok(v)
}

/// Test installs: ask the ledgers for `k` now, with no load behind it, and commit what both granted. With
/// `pool_down` (ms), first make both warm sites' ledgers answer 503 for that long.
#[cfg(feature = "test-faults")]
pub async fn force(h: &Rc<Host<Repo>>, k: u8, pool_down: Option<u64>) -> Result<Value> {
    let g = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or("member not joined")?;
        if c.m.role() != Role::Leader {
            return Ok(not_leader(c.m.not_leader()));
        }
        Grant::of(c)?
    };
    if let Some(ms) = pool_down {
        let site = crate::SiteEnv::read(&h.env)?;
        let sites = if g.warm[0] == g.warm[1] { vec![g.warm[0]] } else { g.warm.to_vec() };
        for s in sites {
            pool_call(&h.env, &site.sites, s, "/pool/down", &json!({ "ms": ms })).await;
        }
    }
    let k = k.clamp(1, git::FANOUT_MAX);
    g.ask(h, k - 1, k, false).await
}

/// What the leader asks both warm sites' pool ledgers with: the network, its owner, its two warm sites, and the
/// committed k.
struct Grant {
    net: NetworkId,
    owner: u64,
    warm: [SiteId; 2],
    k: u8,
}

impl Grant {
    fn of(c: &crate::host::Core<Repo>) -> Result<Grant> {
        let ChainId::Repo(repo) = c.join.chain else { return Err("not a repository chain".into()) };
        let net = NetworkId(repo.0);
        let sites: Vec<SiteId> = c.join.record.sites.iter().copied().collect();
        let fold = &c.m.group_fold().fold;
        Ok(Grant {
            net,
            owner: fold.authz_owner().map_or(0, |o| o.0.0 as u64),
            warm: git::warm_sites(net, c.join.record.home, &sites),
            k: fold.fanout_k(),
        })
    }

    /// Asks both warm sites' ledgers for `want` pool shards at once and commits the k `graftyard_git::granted_k`
    /// answers toward `target`: up to 1 plus the fewer either granted, down only as `fanout` asked, and unchanged when
    /// a ledger does not answer. A step up commits only once every slot it would serve from on both warm sites
    /// answered its last `PoolWarm` catch-up 200 (`graftyard_git::warm_step`). With `reserve`, the ledgers keep one
    /// slot past the k served; every held slot is warmed after the answer, so it starts, then stays caught up.
    async fn ask(&self, h: &Rc<Host<Repo>>, want: u8, target: u8, reserve: bool) -> Result<Value> {
        let site = crate::SiteEnv::read(&h.env)?;
        let body = json!({ "network": self.net.0.0, "owner": self.owner, "want": want });
        let ask = |s| pool_call(&h.env, &site.sites, s, "/pool/grant", &body);
        let (a, b) = if self.warm[0] == self.warm[1] {
            let a = ask(self.warm[0]).await;
            (a.clone(), a)
        } else {
            futures_util::future::join(ask(self.warm[0]), ask(self.warm[1])).await
        };
        let n = |g: &Option<Vec<u16>>| g.as_ref().map(|s| s.len() as u8);
        let granted = git::granted_k(self.k, target, [n(&a), n(&b)]);
        let held = |g: &Option<Vec<u16>>| g.clone().unwrap_or_default();
        let (ha, hb) = (held(&a), held(&b));
        let warmth = |site: SiteId, slots: &[u16]| -> Vec<bool> {
            WARM.with(|w| {
                let w = w.borrow();
                slots.iter().map(|s| w.get(&(self.net.0.0, site.0, *s)).copied().unwrap_or(false)).collect()
            })
        };
        let (wa, wb) = (warmth(self.warm[0], &ha), warmth(self.warm[1], &hb));
        let next = git::warm_step(self.k, granted, [&ha, &hb], [&wa, &wb]);
        if next != self.k {
            commit(h, next).await?;
        }
        // A ledger that granted more than the k both granted gives the rest back, so no site holds pool shards the
        // network does not serve from (hosted smoke r3: one site granted 6 and the other 0, and the 6 stayed leased).
        // A reservation keeps one slot past the k served, and a step waiting on cold slots keeps them all.
        let keep = git::pool_keep(want, granted, next, reserve);
        let trim = json!({ "network": self.net.0.0, "owner": self.owner, "want": keep });
        let (mut a, mut b) = (a, b);
        for (s, g) in [(self.warm[0], &mut a), (self.warm[1], &mut b)] {
            if g.as_ref().is_some_and(|x| x.len() > usize::from(keep)) {
                *g = pool_call(&h.env, &site.sites, s, "/pool/grant", &trim).await.or(g.take());
            }
        }
        // A slot the network no longer holds was torn down or went to another network: its old answer is forgotten,
        // so a later grant of it counts as cold until it answers again.
        WARM.with(|w| {
            w.borrow_mut().retain(|(n, s, slot), _| {
                *n != self.net.0.0
                    || [(self.warm[0], &a), (self.warm[1], &b)]
                        .iter()
                        .any(|(site, g)| site.0 == *s && g.iter().flatten().any(|x| x == slot))
            })
        });
        for (s, g) in [(self.warm[0], &a), (self.warm[1], &b)] {
            for slot in g.iter().flatten() {
                warm_slot(h, self.net, s, *slot);
            }
        }
        Ok(json!({ "k": next, "stepped": next != self.k, "want": want,
                   "grants": [[self.warm[0].0, a], [self.warm[1].0, b]] }))
    }
}

/// Warms one held pool slot in the background: `POST /catchup?warm=pool` on `git#<site>#pool:<1 + slot>`, which
/// starts a stopped container (`graftyard_git::Caller::PoolWarm`) and catches it up; a shard still rehydrating answers
/// `warming` at once (`next=1`) and goes on in the background. The answer is kept in `WARM` for the next step.
fn warm_slot(h: &Rc<Host<Repo>>, net: NetworkId, site: SiteId, slot: u16) {
    let record = {
        let core = h.core.borrow();
        match core.as_ref().and_then(|c| serde_json::to_string(&c.join.record).ok()) {
            Some(r) => r,
            None => return,
        }
    };
    let env = h.env.clone();
    worker::wasm_bindgen_futures::spawn_local(async move {
        let call = async {
            let sites = crate::SiteEnv::read(&env).ok()?.sites;
            let s = git::shards_for(net, site, 1, &[slot]).get(1).copied()?;
            let st = stub(&env, &sites, site, &git::object_name(net, s, 1)).ok()?;
            let url = format!(
                "https://shard/catchup?warm=pool&repo={}&record={}&wait=2&next=1",
                net.0.0,
                worker::js_sys::encode_uri_component(&record)
            );
            let mut init = RequestInit::new();
            init.with_method(Method::Post);
            let r = st.fetch_with_request(Request::new_with_init(&url, &init).ok()?).await.ok()?;
            Some(r.status_code() == 200)
        };
        let ok = crate::host::within(WARM_CALL, call).await.flatten().unwrap_or(false);
        WARM.with(|w| w.borrow_mut().insert((net.0.0, site.0, slot), ok));
    });
}

/// How long a pool warm's answer is waited for: a caught-up shard answers in about a second; past this the slot counts
/// as cold for the next step, and its catch-up goes on.
const WARM_CALL: std::time::Duration = std::time::Duration::from_secs(10);

/// The object that keeps a site's `PoolLedger`: the site's first pool shard. Each network's first shard here is
/// index 0 of a one-shard `count` (`crate::shard`), so the pool is `1..1 + HOT_POOL` and the ledger is shard 1.
pub fn ledger_name(site: SiteId) -> String {
    format!("git#{}#pool:1", site.0)
}

/// How long a ledger call may take before the caller counts the ledger unreachable [guess].
const LEDGER_CALL: std::time::Duration = std::time::Duration::from_secs(3);

/// One call to `site`'s pool ledger: the slots it answered for the network, or `None` when it did not answer 200
/// within `LEDGER_CALL`.
pub async fn pool_call(
    env: &Env,
    sites: &graftyard_types::SiteTable,
    site: SiteId,
    path: &str,
    body: &Value,
) -> Option<Vec<u16>> {
    let stub = stub(env, sites, site, &ledger_name(site)).ok()?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(wasm_bindgen::JsValue::from_str(&body.to_string())));
    let req = Request::new_with_init(&format!("https://shard{path}"), &init).ok()?;
    let call = async move {
        let mut r = stub.fetch_with_request(req).await.ok()?;
        if r.status_code() != 200 {
            return None;
        }
        r.json::<Value>().await.ok()
    };
    let v = crate::host::within(LEDGER_CALL, call).await.flatten()?;
    serde_json::from_value(v["slots"].clone()).ok()
}

/// What a member that does not lead answers `/fanout`: the `notLeader` shape `graftyard_lookup` follows.
fn not_leader(nl: graftyard_raft::NotLeader) -> Value {
    json!({ "error": crate::host::call_error_json(&graftyard_raft::CallError::NotLeader(nl)) })
}

/// Records `Fanout { k }` as the leader's own entry and waits for it to commit.
async fn commit(h: &Rc<Host<Repo>>, k: u8) -> Result<()> {
    let rid = graftyard_lookup::request_id();
    let rx = h.wait_recorded(rid);
    let events = vec![git::repo::RepoEvent::Fanout { k }];
    h.input(|m, t| m.record(t, rid, git::repo::REPO_FORMAT_FANOUT, events))?
        .map_err(|e| Error::RustError(crate::host::call_error_json(&e).to_string()))?;
    h.served();
    match crate::host::within(std::time::Duration::from_secs(10), rx).await {
        Some(Ok(Ok(_))) => Ok(()),
        Some(Ok(Err(nl))) => Err(format!("not leader: {}", crate::host::nl_json(&nl)).into()),
        Some(Err(_)) => Err("member reset".into()),
        None => Err("the fanout entry did not commit in 10 s".into()),
    }
}

/// The shard side: this site's combined `NetworkLoad` of `net`'s k shards, posted to the group's leader. `loads` are
/// the k shards' own reports, the first shard's first. Answers the committed k and, when the leader asked this site's
/// ledger, the slots it granted.
pub async fn report_from_shard(
    env: &Env,
    sites: &graftyard_types::SiteTable,
    me: SiteId,
    net: NetworkId,
    record: &PlacementRecord,
    loads: &[NetworkLoad],
) -> std::result::Result<(u8, Option<Vec<u16>>), String> {
    let body = json!({ "site": me.0, "load": git::combine(loads) });
    let b = graftyard_lookup::Bindings { env, sites };
    let (v, _) =
        graftyard_lookup::call(&b, ChainId::Repo(graftyard_types::RepoId(net.0)), record, "/fanout", &body).await?;
    let mine = v["grants"].as_array().and_then(|g| {
        g.iter()
            .find(|x| x[0].as_u64() == Some(u64::from(me.0)))
            .and_then(|x| serde_json::from_value(x[1].clone()).ok())
    });
    Ok((v["k"].as_u64().unwrap_or(1) as u8, mine))
}

/// The shard object `name` on `site`'s shard class (`graftyard_git::object_name`); every site binds every site's.
pub fn stub(env: &Env, sites: &graftyard_types::SiteTable, site: SiteId, name: &str) -> Result<Stub> {
    let region = sites.get(site).ok_or("the site is not in GRAFTYARD_SITES")?.region;
    env.durable_object(&format!("SHARD_{}", site.0))?.get_by_name_with_location_hint(name, region.hint())
}

#[cfg(test)]
mod tests {
    use graftyard_git::repo::{REPO_FORMAT_FANOUT, RepoEvent, RepoFold};
    use graftyard_raft::{EntryBody, Fold, LogEntry, TierSet};
    use graftyard_types::{AccountId, DbId, Digest, Index, RepoId, Timestamp};
    use serde_json::json;
    use smallvec::SmallVec;

    use crate::family::{Family, Repo};

    /// The `ranges` read a shard's catch-up makes carries the committed k, 1 before any `Fanout` entry, and this
    /// release reads 32.15, so a leader's own `Fanout` record is not refused `Unsupported`.
    #[test]
    fn the_ranges_read_answers_the_committed_k() {
        let mut fold = RepoFold::new(RepoId(DbId(9)), AccountId(DbId(1)));
        let q = json!({ "ranges": true });
        assert_eq!(Repo::read_json(&fold, &q)["fanout"], 1);
        fold.apply_entry(&LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(1),
            prev: Digest([0; 32]),
            format: REPO_FORMAT_FANOUT,
            tiers: TierSet::CORE,
            at: Timestamp(1),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Fanout { k: 4 }]),
        });
        assert_eq!(Repo::read_json(&fold, &q)["fanout"], 4);
        assert!(Repo::reads().contains(&REPO_FORMAT_FANOUT));
    }

    /// G26: the same read answers the `settings` row's default branch, which the shard points HEAD at, and before the
    /// first `Settings` entry `refs/heads/main`, as every other read of the repository takes it. With no row the
    /// container's own rule (`main` if it exists, else the page's first ref) put HEAD on a row push's branch in B0-34
    /// mt3 (`tests/am1/results/b034-mt.md`). The edge's ls-refs read (`lsRefs`) names the same HEAD both times.
    #[test]
    fn the_ranges_read_answers_the_default_branch() {
        use graftyard_git::issues::{MergeMethod, RepoSettings, Visibility};
        let mut fold = RepoFold::new(RepoId(DbId(9)), AccountId(DbId(1)));
        let q = json!({ "ranges": true });
        let ls = json!({ "lsRefs": { "reads": [], "head": true } });
        let ls_head = |fold: &RepoFold| -> Vec<u8> {
            let r: graftyard_git::ls_refs::LsRefsRead = serde_json::from_value(Repo::read_json(fold, &ls)).unwrap();
            r.head.expect("HEAD is named").target
        };
        assert_eq!(Repo::read_json(&fold, &q)["default_branch"], "refs/heads/main");
        assert_eq!(ls_head(&fold), b"refs/heads/main");
        let settings = RepoSettings {
            owner: AccountId(DbId(1)),
            enterprise: None,
            visibility: Visibility::Private,
            default_branch: graftyard_git::refs::RefName::parse(b"refs/heads/v1.x").unwrap(),
            archived: false,
            description: None,
            homepage: None,
            topics: SmallVec::new(),
            has_issues: true,
            has_wiki: false,
            has_projects: false,
            merge_methods: smallvec::smallvec![MergeMethod::Merge],
            delete_branch_on_merge: false,
            allow_auto_merge: false,
            allow_update_branch: false,
            check_retention_days: 90,
        };
        fold.apply_entry(&LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(1),
            prev: Digest([0; 32]),
            format: graftyard_git::repo::REPO_FORMAT_V1,
            tiers: TierSet::CORE.union(TierSet::METADATA),
            at: Timestamp(1),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Settings(settings)]),
        });
        assert_eq!(Repo::read_json(&fold, &q)["default_branch"], "refs/heads/v1.x");
        assert_eq!(ls_head(&fold), b"refs/heads/v1.x");
    }

    /// A push's one repository read (`{"access": true, "agent": true}`) answers the access rows and the agent rows,
    /// with `"agent": true` beside them so the edge reuses them; `{"access": true}` alone answers as before, without
    /// the agent rows or the flag, which is how an edge tells an older member's answer.
    #[test]
    fn a_push_reads_its_access_and_agent_rows_at_once() {
        use graftyard_api::hosted::TierRows;
        use graftyard_git::agent::AgentPolicy;
        let mut fold = RepoFold::new(RepoId(DbId(9)), AccountId(DbId(1)));
        let policy = AgentPolicy { extra_human_approvals: 2, ..AgentPolicy::default() };
        fold.apply_entry(&LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(1),
            prev: Digest([0; 32]),
            format: graftyard_git::repo::REPO_FORMAT_AGENT,
            tiers: TierSet::CORE,
            at: Timestamp(1),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![
                RepoEvent::Collaborator {
                    who: AccountId(DbId(7)),
                    permission: Some(graftyard_identity::RepoPermission::Write),
                },
                RepoEvent::AgentPolicy(policy.clone()),
            ]),
        });
        let tables = |v: &serde_json::Value| {
            let rows = TierRows::from_value(v).unwrap();
            rows.core.iter().map(|(t, _, _)| t.clone()).collect::<Vec<_>>()
        };
        let both = Repo::read_json(&fold, &json!({ "rows": { "access": true, "agent": true }, "ranges": true }));
        assert_eq!(both["agent"], true);
        assert_eq!(tables(&both), ["collaborator", "agent_policy"]);
        let store = TierRows::store(&TierRows::from_value(&both).unwrap().core);
        assert_eq!(graftyard_git::tiers::agent_policy(&store), policy);
        let access = Repo::read_json(&fold, &json!({ "rows": { "access": true }, "ranges": true }));
        assert!(access.get("agent").is_none());
        assert_eq!(tables(&access), ["collaborator"]);
    }

    /// A `rows` read with `packed` answers the whole tiers as one JSON string the edge reads back as the same rows;
    /// without it, the object a member built before `packed` answered.
    #[test]
    fn a_packed_rows_read_answers_one_string_of_the_same_rows() {
        use graftyard_api::hosted::TierRows;
        let mut fold = RepoFold::new(RepoId(DbId(9)), AccountId(DbId(1)));
        fold.apply_entry(&LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(1),
            prev: Digest([0; 32]),
            format: REPO_FORMAT_FANOUT,
            tiers: TierSet::CORE,
            at: Timestamp(1),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Fanout { k: 4 }]),
        });
        let packed = Repo::read_json(&fold, &json!({ "rows": true, "ranges": true, "packed": true }));
        let plain = Repo::read_json(&fold, &json!({ "rows": true, "ranges": true }));
        assert!(packed.is_string() && plain.is_object());
        let rows = TierRows::from_value(&packed).unwrap();
        assert_eq!(rows, TierRows::from_value(&plain).unwrap());
        assert!(rows.complete && rows.core.iter().any(|(t, _, _)| t == "fanout"));
    }

    /// The packed dump is cached by the tiers' state: a second read at the same state answers the same string, and an
    /// applied entry, another repository at the same index, or a dropped cache answers fresh rows, never a stale dump.
    #[test]
    fn a_cached_dump_never_outlives_its_tiers() {
        use graftyard_api::hosted::TierRows;
        let q = json!({ "rows": true, "ranges": true, "packed": true });
        let entry = |i: u64, k: u8, prev: Digest| LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(i),
            prev,
            format: REPO_FORMAT_FANOUT,
            tiers: TierSet::CORE,
            at: Timestamp(i),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Fanout { k }]),
        };
        let k_of = |v: &serde_json::Value| {
            let rows = TierRows::from_value(v).unwrap();
            rows.core.iter().find(|(t, _, _)| t == "fanout").map(|(_, _, c)| format!("{c:?}"))
        };
        let one = |k| entry(1, k, Digest([0; 32]));
        let two = |k, after: &LogEntry<RepoEvent>| entry(2, k, graftyard_raft::digest(after));
        let mut a = RepoFold::new(RepoId(DbId(31)), AccountId(DbId(1)));
        a.apply_entry(&one(2));
        let first = Repo::read_json(&a, &q);
        assert_eq!(Repo::read_json(&a, &q), first);
        a.apply_entry(&two(5, &one(2)));
        let second = Repo::read_json(&a, &q);
        assert_ne!(k_of(&second), k_of(&first), "an applied entry is in the next read");
        assert_eq!(TierRows::from_value(&second).unwrap().index, 2);
        // Another repository's tiers at the same index are not served the first one's dump.
        let mut b = RepoFold::new(RepoId(DbId(32)), AccountId(DbId(1)));
        b.apply_entry(&one(7));
        b.apply_entry(&two(3, &one(7)));
        assert_ne!(k_of(&Repo::read_json(&b, &q)), k_of(&second));
        crate::family::forget_dumps();
        assert_eq!(Repo::read_json(&a, &q), second);
    }

    /// A rebuild of a repository's dump is let through at most once per DUMP_EVERY_MS, and a read whose cached dump is
    /// at the tiers' state is served at once, however soon after the last rebuild.
    #[test]
    fn dump_rebuilds_are_spaced_but_cache_hits_are_not() {
        use crate::family::{DUMP_EVERY_MS, LEARNER_DUMP_EVERY_MS, dump_wait};
        let q = json!({ "rows": true, "ranges": true, "packed": true });
        let entry = |i: u64, k: u8, prev: Digest| LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(i),
            prev,
            format: REPO_FORMAT_FANOUT,
            tiers: TierSet::CORE,
            at: Timestamp(i),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Fanout { k }]),
        };
        let first = entry(1, 2, Digest([0; 32]));
        let mut a = RepoFold::new(RepoId(DbId(41)), AccountId(DbId(1)));
        a.apply_entry(&first);
        assert_eq!(dump_wait(&a, 10_000, DUMP_EVERY_MS), None, "the first rebuild is due");
        Repo::read_json(&a, &q);
        assert_eq!(dump_wait(&a, 10_100, DUMP_EVERY_MS), None, "the cached dump is at the tiers' state");
        a.apply_entry(&entry(2, 5, graftyard_raft::digest(&first)));
        assert_eq!(dump_wait(&a, 10_300, DUMP_EVERY_MS), Some(DUMP_EVERY_MS - 300));
        assert_eq!(dump_wait(&a, 10_000 + DUMP_EVERY_MS, DUMP_EVERY_MS), None, "due again");
        assert_eq!(dump_wait(&a, 10_001 + DUMP_EVERY_MS, DUMP_EVERY_MS), Some(DUMP_EVERY_MS - 1), "and recorded");
        // A learner's spacing is its own: at LEARNER_DUMP_EVERY_MS after the last rebuild one is due again.
        let at = 10_000 + DUMP_EVERY_MS;
        a.apply_entry(&entry(3, 6, graftyard_raft::digest(&entry(2, 5, graftyard_raft::digest(&first)))));
        assert_eq!(dump_wait(&a, at + 100, LEARNER_DUMP_EVERY_MS), Some(LEARNER_DUMP_EVERY_MS - 100));
        assert_eq!(dump_wait(&a, at + LEARNER_DUMP_EVERY_MS, LEARNER_DUMP_EVERY_MS), None, "due at its own interval");
    }

    /// A merge's rows build no dump, so neither waits for one: on a learner or a voter, right after a rebuild and at a
    /// state the dump does not hold, `{"rows": {"merge": id}}` and `{"rows": {"mergeN": n}}` answer their rows at
    /// once, and the next whole-tier read still finds its rebuild where it was (`merge-p99`).
    #[test]
    fn a_merge_rows_read_never_waits_for_a_dump_rebuild() {
        use crate::family::{DUMP_EVERY_MS, LEARNER_DUMP_EVERY_MS, dump_wait};
        use crate::host::{ReadBody, fold_answer};
        let entry = |i: u64, k: u8, prev: Digest| LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(i),
            prev,
            format: REPO_FORMAT_FANOUT,
            tiers: TierSet::CORE,
            at: Timestamp(i),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Fanout { k }]),
        };
        let first = entry(1, 2, Digest([0; 32]));
        let mut a = RepoFold::new(RepoId(DbId(61)), AccountId(DbId(1)));
        a.apply_entry(&first);
        let dump = json!({ "rows": true, "ranges": true, "packed": true });
        let at = 50_000;
        assert!(matches!(fold_answer::<Repo>(&a, &dump, 2, at, DUMP_EVERY_MS), ReadBody::Json(_)), "the first rebuild");
        a.apply_entry(&entry(2, 5, graftyard_raft::digest(&first)));
        for every in [DUMP_EVERY_MS, LEARNER_DUMP_EVERY_MS] {
            for q in [
                json!({ "rows": { "merge": 7 }, "ranges": true, "packed": true }),
                json!({ "rows": { "mergeN": 3 }, "ranges": true, "packed": true }),
            ] {
                let ReadBody::Json(b) = fold_answer::<Repo>(&a, &q, 5, at + 10, every) else {
                    panic!("{q} waited for a dump rebuild at every {every}")
                };
                assert_eq!(b.to_vec(), serde_json::to_vec(&Repo::read_json(&a, &q)).unwrap(), "{q} answers its rows");
            }
        }
        // A whole-tier read is still spaced, and the merges took no rebuild slot: its rebuild is due where it was.
        assert_eq!(fold_answer::<Repo>(&a, &dump, 5, at + 10, DUMP_EVERY_MS), ReadBody::Shed(DUMP_EVERY_MS - 10, None));
        assert_eq!(dump_wait(&a, at + DUMP_EVERY_MS, DUMP_EVERY_MS), None);
    }

    /// F77 (BF-54 run 2): `acquirejob`'s secrets read, `{"rows": {"actions": true}}`, is answered at once by a voter
    /// whose tiers changed right after a dump rebuild, where the whole-tier read that stood in for it is shed; it holds
    /// the repository's secrets, variables, and settings, and no other row; and a follower serves it by read-index with
    /// its metadata tier still loading, as it does the access rows.
    #[test]
    fn the_acquire_secrets_read_is_never_shed_for_a_dump_rebuild() {
        use crate::family::{DUMP_EVERY_MS, follower_query};
        use crate::host::{ReadBody, fold_answer};
        use graftyard_api::hosted::TierRows;
        use graftyard_git::secrets::{REPO_FORMAT_SECRETS, secrets, variables};
        use graftyard_identity::sealed::{ActionsName, SecretRecord, SecretScope, VariableRecord};
        let entry = |i: u64, prev: Digest, body: Vec<RepoEvent>| LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(i),
            prev,
            format: REPO_FORMAT_SECRETS,
            tiers: TierSet::CORE,
            at: Timestamp(i),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(body),
        };
        let n = |s: &str| ActionsName::parse(s).unwrap();
        let secret = RepoEvent::ActionsSecret {
            scope: SecretScope::Repo,
            name: n("TOKEN"),
            record: Some(SecretRecord { sealed: vec![7; 64].into(), key_id: "k1".into(), updated: Timestamp(1) }),
            actor: None,
        };
        let var = RepoEvent::ActionsVariable {
            scope: SecretScope::Repo,
            name: n("REGION"),
            record: Some(VariableRecord { value: "weur".into(), updated: Timestamp(1) }),
            actor: None,
        };
        let first = entry(1, Digest([0; 32]), vec![secret, var, RepoEvent::Fanout { k: 2 }]);
        let mut a = RepoFold::new(RepoId(DbId(77)), AccountId(DbId(1)));
        a.apply_entry(&first);
        let dump = json!({ "rows": true, "ranges": true, "packed": true });
        let actions = json!({ "rows": { "actions": true }, "ranges": true });
        let at = 70_000;
        assert!(matches!(fold_answer::<Repo>(&a, &dump, 1, at, DUMP_EVERY_MS), ReadBody::Json(_)), "the rebuild");
        // A job's check run commits to the repository: the tiers move on.
        a.apply_entry(&entry(2, graftyard_raft::digest(&first), vec![RepoEvent::Fanout { k: 3 }]));
        assert!(matches!(fold_answer::<Repo>(&a, &dump, 2, at + 10, DUMP_EVERY_MS), ReadBody::Shed(..)), "dump shed");
        let ReadBody::Json(b) = fold_answer::<Repo>(&a, &actions, 2, at + 10, DUMP_EVERY_MS) else {
            panic!("the secrets read waited for a dump rebuild")
        };
        let rows = TierRows::from_value(&serde_json::from_slice(&b).unwrap()).unwrap();
        assert_eq!(rows.index, 2, "at the read's state");
        let core = TierRows::store(&rows.core);
        assert_eq!(
            secrets(&core, &SecretScope::Repo).iter().map(|(n, r)| (n.as_str(), r.sealed.len())).collect::<Vec<_>>(),
            [("TOKEN", 64)]
        );
        assert_eq!(variables(&core, &SecretScope::Repo)[0].1.value.as_ref(), "weur");
        assert!(
            rows.core.iter().all(|(t, _, _)| ["settings", "actions_secret", "actions_variable"].contains(&t.as_str()))
        );
        assert!(follower_query(&actions, false), "core rows only: served with the metadata tier loading");
    }

    /// A packed dump is built once per state of the tiers and every read at that state, from any input, shares the
    /// same bytes, which are exactly `read_json`'s answer; an applied entry rebuilds it, another repository never gets
    /// it, and a narrow read is not a dump. A learner spaces its rebuilds by LEARNER_DUMP_EVERY_MS, a voter by
    /// DUMP_EVERY_MS.
    #[test]
    fn a_dump_is_built_once_per_state_and_shared_by_every_read() {
        use crate::family::{DUMP_EVERY_MS, LEARNER_DUMP_EVERY_MS, dump_builds, dump_bytes, dump_every};
        let q = json!({ "rows": true, "ranges": true, "packed": true });
        let entry = |i: u64, k: u8, prev: Digest| LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(i),
            prev,
            format: REPO_FORMAT_FANOUT,
            tiers: TierSet::CORE,
            at: Timestamp(i),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Fanout { k }]),
        };
        let first = entry(1, 2, Digest([0; 32]));
        let mut a = RepoFold::new(RepoId(DbId(51)), AccountId(DbId(1)));
        a.apply_entry(&first);
        let built = dump_builds();
        let one = dump_bytes(&a, &q).expect("a packed read is a dump");
        let again = dump_bytes(&a, &q).expect("dump");
        assert!(std::rc::Rc::ptr_eq(&one, &again), "one copy for every read at this state");
        assert_eq!(dump_builds(), built + 1, "built once");
        assert_eq!(serde_json::to_vec(&Repo::read_json(&a, &q)).unwrap(), one.to_vec(), "the bytes are the answer");
        a.apply_entry(&entry(2, 5, graftyard_raft::digest(&first)));
        let next = dump_bytes(&a, &q).expect("dump");
        assert_ne!(next, one, "an applied entry is in the next dump");
        assert_eq!(dump_builds(), built + 2);
        let mut b = RepoFold::new(RepoId(DbId(52)), AccountId(DbId(1)));
        b.apply_entry(&first);
        b.apply_entry(&entry(2, 7, graftyard_raft::digest(&first)));
        assert_ne!(dump_bytes(&b, &q).expect("dump"), next, "another repository at the same index");
        for narrow in [
            json!({ "rows": { "access": true } }),
            json!({ "rows": { "merge": 7 }, "ranges": true, "packed": true }),
            json!({ "rows": true }),
            json!({ "ranges": true }),
        ] {
            assert!(dump_bytes(&a, &narrow).is_none(), "{narrow}");
        }
        // Through the host, as a grant answers: two inputs' grants at one state get the same copy, not one each.
        let (crate::host::ReadBody::Json(x), crate::host::ReadBody::Json(y)) = (
            crate::host::fold_answer::<Repo>(&a, &q, 0, 0, DUMP_EVERY_MS),
            crate::host::fold_answer::<Repo>(&a, &q, 0, 1, LEARNER_DUMP_EVERY_MS),
        ) else {
            panic!("a cached dump is served")
        };
        assert!(std::rc::Rc::ptr_eq(&x, &y) && std::rc::Rc::ptr_eq(&x, &next), "one copy across grants");
        assert_eq!(dump_builds(), built + 3, "no build beyond the two of `a` and the one of `b`");
        assert_eq!(dump_every(graftyard_types::learner_addr(0)), LEARNER_DUMP_EVERY_MS);
        assert_eq!(dump_every(graftyard_types::learner_addr(15)), LEARNER_DUMP_EVERY_MS);
        assert_eq!(dump_every(graftyard_types::SiteId(2)), DUMP_EVERY_MS);
        const { assert!(LEARNER_DUMP_EVERY_MS < DUMP_EVERY_MS) };
    }

    /// A narrow API read (`graftyard_api::narrow`) is answered from the newest cached answer to the same query when it
    /// was built at or above the read's floor (the leader's read index for that read), and never below it: a read
    /// whose floor is newer than the cache waits for the next build, spaced by NARROW_EVERY_MS, and is built at the
    /// fold's state then. An answer never stands in for another query's, and an older build never replaces a newer
    /// one. A follower serves it only once its metadata tier is loaded, and it is never a whole-tier read.
    #[test]
    fn a_narrow_read_is_served_at_or_above_its_floor_and_spaced() {
        use crate::family::{NARROW_EVERY_MS, follower_query, narrow_builds, narrow_bytes, narrow_wait};
        use graftyard_api::narrow::{Route, Spec};
        use graftyard_api::view::RepoRef;
        let at = RepoRef { id: RepoId(DbId(61)), owner: "o".into(), owner_id: AccountId(DbId(1)), name: "r".into() };
        let q = |tail: &[&str]| {
            let route = Route::Rest {
                tail: tail.iter().map(|s| (*s).to_owned()).collect(),
                query: String::new(),
                path: String::new(),
                agent: false,
            };
            json!({ "rows": { "api": Spec { at: at.clone(), route } } })
        };
        let entry = |i: u64, k: u8, prev: Digest| LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(i),
            prev,
            format: REPO_FORMAT_FANOUT,
            tiers: TierSet::CORE,
            at: Timestamp(i),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Fanout { k }]),
        };
        let index_of =
            |b: &[u8]| graftyard_api::hosted::TierRows::from_value(&serde_json::from_slice(b).unwrap()).unwrap().index;
        let first = entry(1, 2, Digest([0; 32]));
        let second = entry(2, 5, graftyard_raft::digest(&first));
        let mut a = RepoFold::new(RepoId(DbId(61)), AccountId(DbId(1)));
        a.apply_entry(&first);
        let (repo, pulls) = (q(&[]), q(&["pulls", "1"]));
        assert_eq!(narrow_wait(&a, &repo, 1, 5_000), None, "the first build is due");
        let built = narrow_builds();
        let one = narrow_bytes(&a, &repo, 1).expect("a narrow read");
        assert_eq!(index_of(&one), 1);
        assert!(std::rc::Rc::ptr_eq(&one, &narrow_bytes(&a, &repo, 1).unwrap()), "one copy per query and state");
        assert_eq!(narrow_builds(), built + 1, "built once");
        assert_eq!(narrow_wait(&a, &repo, 1, 5_001), None, "a cached answer at the floor is served at once");
        assert_ne!(narrow_bytes(&a, &pulls, 1).unwrap(), one, "another query is its own answer");
        assert_eq!(serde_json::to_vec(&Repo::read_json(&a, &repo)).unwrap(), one.to_vec(), "the bytes are the answer");
        // Entry 2 applies. A read whose floor is still 1 is served the answer at 1 at once: it holds every entry
        // acknowledged before that read arrived. A read whose floor is 2 must not get it.
        a.apply_entry(&second);
        assert_eq!(narrow_wait(&a, &repo, 1, 5_050), None, "an older floor is covered by the cached answer");
        assert!(std::rc::Rc::ptr_eq(&narrow_bytes(&a, &repo, 1).unwrap(), &one), "served from the cache, not rebuilt");
        assert_eq!(narrow_wait(&a, &repo, 2, 5_050), Some(NARROW_EVERY_MS - 50), "a newer floor waits its spacing");
        assert_eq!(narrow_wait(&a, &repo, 2, 5_000 + NARROW_EVERY_MS), None, "then is due");
        let two = narrow_bytes(&a, &repo, 2).unwrap();
        assert_eq!(index_of(&two), 2, "built at the fold's state, at the read's floor");
        assert_eq!(narrow_builds(), built + 3, "the pulls answer and this rebuild");
        // A second member of the same repository in this isolate that is behind (a follower at 1) is served the
        // newer answer, which covers its read's floor, and builds nothing.
        let mut b = RepoFold::new(RepoId(DbId(61)), AccountId(DbId(1)));
        b.apply_entry(&first);
        let behind = narrow_bytes(&b, &repo, 1).unwrap();
        assert!(
            std::rc::Rc::ptr_eq(&behind, &two),
            "a member behind is served the newer answer, which covers its floor"
        );
        assert_eq!(narrow_builds(), built + 3, "and builds nothing");
        // A member of the same repository whose metadata tier is still loading, at the same index, answers without
        // the metadata rows: its answer is its own, and never serves a read on a member that has loaded the tier.
        let mut c = RepoFold::with_tiers(
            Some(RepoId(DbId(61))),
            Some(AccountId(DbId(1))),
            Box::new(graftyard_fold::MemStore::default()),
            Box::new(Unloaded),
        );
        c.apply_entry(&first);
        c.apply_entry(&second);
        let loading = narrow_bytes(&c, &repo, 2).unwrap();
        assert!(!std::rc::Rc::ptr_eq(&loading, &two), "a loading member builds its own answer");
        assert!(std::rc::Rc::ptr_eq(&narrow_bytes(&a, &repo, 2).unwrap(), &two), "a loaded member keeps its own");
        assert!(crate::family::dump_bytes(&a, &repo).is_none(), "not a dump");
        assert!(
            follower_query(&repo, true) && !follower_query(&repo, false),
            "served once the metadata tier is loaded"
        );
    }

    /// A metadata tier that has not loaded (`TierStore::loaded`).
    #[derive(Clone, Debug, Default)]
    struct Unloaded;

    impl graftyard_fold::Store for Unloaded {
        fn get(&self, _: &str, _: &[graftyard_fold::Val]) -> Option<graftyard_fold::Stored> {
            None
        }
        fn put(&mut self, _: &str, _: graftyard_fold::Key, _: graftyard_fold::Stored) {}
        fn delete(&mut self, _: &str, _: &[graftyard_fold::Val]) {}
        fn scan(
            &self,
            _: &str,
            _: &[graftyard_fold::Val],
            _: usize,
        ) -> Vec<(graftyard_fold::Key, graftyard_fold::Stored)> {
            Vec::new()
        }
    }

    impl graftyard_fold::TierStore for Unloaded {
        fn clone_box(&self) -> Box<dyn graftyard_fold::TierStore> {
            Box::new(self.clone())
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn loaded(&self) -> bool {
            false
        }
    }

    /// A follower serves by read-index only the reads the design names: whole-tier and merge rows once its metadata
    /// tier is loaded, and the core-only access and agent rows and the edge's `ls-refs` read; paged refs, `ranges`,
    /// deps, and the alarm reads stay on the leader.
    #[test]
    fn a_follower_serves_only_the_designs_reads() {
        use crate::family::follower_query;
        let tier = json!({ "rows": true, "ranges": true, "packed": true });
        let merge = json!({ "rows": { "merge": 7 }, "ranges": true, "packed": true });
        let merge_n = json!({ "rows": { "mergeN": 3 }, "ranges": true, "packed": true });
        for q in [&tier, &merge, &merge_n] {
            assert!(follower_query(q, true), "{q}");
            assert!(!follower_query(q, false), "a loading metadata tier sends {q} to the leader");
        }
        for q in [
            json!({ "rows": { "access": true }, "ranges": true }),
            json!({ "rows": { "agent": true }, "ranges": true }),
            json!({ "lsRefs": { "reads": [""], "head": true } }),
        ] {
            assert!(follower_query(&q, false), "{q} reads the core tier only");
        }
        for q in [
            json!({}),
            json!({ "ranges": true }),
            json!({ "refs": { "limit": 10 } }),
            json!({ "dep": true }),
            json!({ "prsync": [1] }),
            json!({ "mergequeue": "main" }),
            json!({ "tiers": true }),
            json!({ "settings": true }),
            json!({ "request": "01" }),
            json!({ "probe": true }),
        ] {
            assert!(!follower_query(&q, true), "{q} stays on the leader");
        }
    }

    /// A follower's answer to a shard's report names the leader in the shape lookup follows (`/error/notLeader`),
    /// not an error the lookup would count as a missed member.
    #[test]
    fn a_follower_answers_a_report_with_not_leader() {
        let nl = graftyard_raft::NotLeader {
            term: graftyard_raft::Term(7),
            leader: Some(graftyard_types::SiteId(2)),
            config: None,
        };
        let v = super::not_leader(nl);
        assert_eq!(v.pointer("/error/notLeader/leader"), Some(&json!(2)));
        assert_eq!(v.pointer("/error/notLeader/term"), Some(&json!(7)));
    }
}
