//! A group member's object (`design/DESIGN.md`, Shape, "Sites and placement"). M1-WP1 built its placement half: the
//! registry row it writes before it stores anything, and its `Rtt` rows, which `Placement::new` reads. M1-WP2 hosts
//! the Raft core in it (`crate::host`), and this file is its routes:
//!
//! One object class hosts every command family; the chain kind in the member's name picks it (`crate::family`).
//!
//! - `POST /join`: the member's identity and record. A member with no stored identity checks the record against its
//!   source, claims the bootstrap generation, writes its registry row, then measures its peers.
//! - `POST /raft`: member-to-member messages, envelopes in the crate's codec (`graftyard_raft::codec`).
//! - `POST /commit`, `/outcome`, `/outcome_by_request`, `/read`, `/campaign`, `/suspect`: the edge's calls. Every
//!   reply carries `meta` (`ReplyMeta`), and a member that cannot lead answers `notLeader` with its committed config.
//!   `/read` with `{"q": {"dep": true}}` is another group's dependency read.
//! - `POST /transfer`, `/reconfigure`, `/replace`: leadership transfer, one config change, and a member replaced on
//!   its own site with a new generation (BF-11's member retry and `ReplaceMember` stub).
//! - `GET /status` and `/stats`: the member's role, log position, and latency samples.
//! - With the `test-faults` feature only: `GET /log`, `/audit`, `/faults`, `/deps_log`, `/isolate`, and `POST /reset`,
//!   `/retire`, `/probe_vote`, the test harness's reads, kill, lost-member stub, BF-10's vote-and-read probe, and the
//!   per-isolate fault counters; and the commit fields `kill`, `down_ms`, `hold_deps_ms`, `hold_ms`, the `/join` field
//!   `no_claim`, `/replace`'s `fail_first`, and `/transfer`'s `abandon`. A product build has none of them.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use graftyard_raft::{self as raft, ByRequest, CallError, Role, Verdict};
use graftyard_types::{
    AccountId, ChainId, Member as MemberRef, MemberRegistry, PlacementRecord, RegistryRow, Rtt, SiteId, Timestamp,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use worker::*;

use crate::family::{self, Family, Kind, Source};
#[cfg(feature = "test-faults")]
use crate::faults::Kill;
use crate::host::{self, Host, err, within};
use crate::{SiteEnv, lookup, now_ms};

/// Round trips per peer; the row keeps their median.
const RTT_SAMPLES: usize = 20;
/// A member measures again only on a day it served a request and has not measured yet.
const DAY_MS: u64 = 86_400_000;

#[durable_object(alarm)]
pub struct Member {
    state: Rc<State>,
    env: Env,
    /// The host for this member's command family, built once the chain is known: from the stored `/join`, or from
    /// the first `/join`.
    hosts: RefCell<Option<Hosted>>,
}

/// One host per command family, by the chain kind in the member's name (`crate::family::Kind`).
#[derive(Clone)]
pub enum Hosted {
    Names(Rc<Host<family::Names>>),
    Tokens(Rc<Host<family::Tokens>>),
    Ids(Rc<Host<family::Ids>>),
    Authz(Rc<Host<family::Authz>>),
    Repo(Rc<Host<family::Repo>>),
    Release(Rc<Host<crate::release::Release>>),
    #[cfg(feature = "test-faults")]
    Register(Rc<Host<family::Register>>),
}

/// What a member is told when it is created: who it is, and the record its group was built from.
#[derive(Clone, Serialize, Deserialize)]
pub struct Join {
    pub chain: ChainId,
    pub me: MemberRef,
    pub record: PlacementRecord,
    /// A member added to a running group measures these, the config's other voters; a bootstrap member measures the
    /// record's bootstrap names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<MemberRef>,
    /// A repository chain's owner, `RepoSettings.owner`, which its fold names for `deps`. A bootstrap member checks
    /// it against the repository's `RepoOwner` row on `Names` (`Source::Repo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<AccountId>,
    /// The `Release` group's members: the signed genesis, hex, which the bootstrap check accepts against this
    /// script's own trust anchor and reads the group's record from (`Source::Anchor`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genesis: Option<String>,
    /// Test fault for BF-10's control run: check the source as the design states it, the same generation and no
    /// claim.
    #[cfg(feature = "test-faults")]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_claim: bool,
}

#[derive(Deserialize)]
struct RttRow {
    peer: i64,
    median_ms: i64,
    at: i64,
}

#[derive(Deserialize)]
#[serde(bound = "")]
struct CommitReq<F: Family> {
    rid: String,
    cmd: F::C,
    /// Test fault: where to kill this leader for this command (`crate::faults::Kill`).
    #[cfg(feature = "test-faults")]
    kill: Option<String>,
    /// With `kill`: the reset object answers nothing for this long, so another member must take over.
    #[cfg(feature = "test-faults")]
    down_ms: Option<u64>,
    /// BF-14: the receipt of this send misses the edge's deadline (`crate::faults::Receipt`).
    #[cfg(feature = "test-faults")]
    receipt: Option<String>,
    /// BF-18 and BF-19: the leader that decides this command skips its `Committed` marker puts.
    #[cfg(feature = "test-faults")]
    #[serde(default)]
    no_marker: bool,
}

/// `POST /record` (test installs): events the leader records with `Member::record` under request id `rid`, checked
/// against the fold first. A repository's `Promoted` and `Archived`.
#[cfg(feature = "test-faults")]
#[derive(Deserialize)]
#[serde(bound = "")]
struct RecordReq<F: Family> {
    rid: String,
    events: Vec<F::Ev>,
}

#[derive(Deserialize)]
#[serde(bound = "")]
struct CommitMany<F: Family> {
    cmds: Vec<CommitReq<F>>,
    /// BF-15: hold this leader's dependency reads for this long, so the commands queue behind the first entry.
    #[cfg(feature = "test-faults")]
    hold_deps_ms: Option<u64>,
    /// Hold every command this long for its outcome, instead of COMMIT_HOLD.
    #[cfg(feature = "test-faults")]
    hold_ms: Option<u64>,
}

macro_rules! each {
    ($h:expr, $x:ident => $body:expr) => {
        match $h {
            Hosted::Names($x) => $body,
            Hosted::Tokens($x) => $body,
            Hosted::Ids($x) => $body,
            Hosted::Authz($x) => $body,
            Hosted::Repo($x) => $body,
            Hosted::Release($x) => $body,
            #[cfg(feature = "test-faults")]
            Hosted::Register($x) => $body,
        }
    };
}

impl Hosted {
    fn build(state: Rc<State>, env: Env, join: &Join) -> Option<Hosted> {
        Some(match Kind::of(join)? {
            Kind::Names => Hosted::Names(Host::new(state, env)),
            Kind::Tokens => Hosted::Tokens(Host::new(state, env)),
            Kind::Ids => Hosted::Ids(Host::new(state, env)),
            Kind::Authz => Hosted::Authz(Host::new(state, env)),
            Kind::Repo => Hosted::Repo(Host::new(state, env)),
            Kind::Release => Hosted::Release(Host::new(state, env)),
            #[cfg(feature = "test-faults")]
            Kind::Register => Hosted::Register(Host::new(state, env)),
        })
    }
}

impl DurableObject for Member {
    fn new(state: State, env: Env) -> Self {
        let sql = state.storage().sql();
        sql.exec(
            "CREATE TABLE IF NOT EXISTS rtt (peer INTEGER PRIMARY KEY, median_ms INTEGER NOT NULL, at INTEGER NOT NULL)",
            None,
        )
        .expect("rtt schema");
        sql.exec("CREATE TABLE IF NOT EXISTS me (k TEXT PRIMARY KEY, v TEXT NOT NULL)", None).expect("me schema");
        // The `Release` fold checks against this script's trust anchor; read it before any host is built.
        let _ = crate::release::anchor(&env);
        let state = Rc::new(state);
        let join = stored_join(&state).ok().flatten();
        let hosts = join.and_then(|j| Hosted::build(state.clone(), env.clone(), &j));
        Self { state, env, hosts: RefCell::new(hosts) }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let url = req.url()?;
        if let (Method::Post, "/join") = (req.method(), url.path()) {
            let join: Join = req.json().await?;
            if self.hosts.borrow().is_none() {
                let Some(h) = Hosted::build(self.state.clone(), self.env.clone(), &join) else {
                    return Response::error(format!("no member family hosts {} as joined", join.chain), 400);
                };
                *self.hosts.borrow_mut() = Some(h);
            }
            let h = self.hosts.borrow().clone().expect("built above");
            return each!(&h, x => Routes { host: x.clone() }.join(join).await);
        }
        let Some(h) = self.hosts.borrow().as_ref().map(Hosted::clone) else {
            return match (req.method(), url.path()) {
                (Method::Get, "/ping") => Response::empty(),
                (Method::Get, "/status") => Response::from_json(&json!({ "joined": false })),
                (Method::Get, "/rtt") => Response::from_json(&rtt_rows(&self.state, &self.env)?),
                _ => Response::error("member not joined", 409),
            };
        };
        each!(&h, x => Routes { host: x.clone() }.fetch(req, url).await)
    }

    async fn alarm(&self) -> Result<Response> {
        let hosted = self.hosts.borrow().as_ref().map(Hosted::clone);
        if let Some(h) = hosted {
            each!(&h, x => if x.joined() { x.alarm().await });
        }
        Response::empty()
    }
}

/// One hop of the depth gauge: call this object's own `/depth_hop` with `k + 1`. The deepest hop reached, and the
/// refusal that ended the chain.
#[cfg(feature = "test-faults")]
async fn depth_hop(env: &Env, me: &MemberRef, k: u64) -> Value {
    if k >= 40 {
        return json!({ "hops": k, "end": "cap" });
    }
    let call = async {
        let stub = SiteEnv::read(env)?.member(env, me.site, &me.do_name)?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(json!({ "k": k + 1 }).to_string().into()));
        let mut res = stub.fetch_with_request(Request::new_with_init("https://member/depth_hop", &init)?).await?;
        res.json::<Value>().await
    };
    match call.await {
        Ok(v) => v,
        Err(e) => json!({ "hops": k, "end": e.to_string() }),
    }
}

fn meta<C: graftyard_raft::Command>(m: &raft::Member<C>) -> Value {
    let r = m.meta();
    json!({ "term": r.term.0, "leader": r.leader.map(|s| s.0), "configIndex": r.config_index.0 })
}

fn verdict_str(v: Verdict) -> &'static str {
    match v {
        Verdict::Leader => "leader",
        Verdict::Resumed => "resumed",
        Verdict::Started => "started",
        Verdict::Bootstrapping => "bootstrapping",
        Verdict::Promised => "promised",
        Verdict::Busy => "busy",
        Verdict::Won => "won",
        Verdict::Lost => "lost",
        Verdict::MayNotLead => "mayNotLead",
    }
}

fn stored_join(state: &State) -> Result<Option<Join>> {
    let rows = host::rows(&state.storage().sql(), "SELECT v FROM me WHERE k = 'join'", vec![])?;
    match rows.first() {
        Some(r) => Ok(Some(serde_json::from_str(host::text(&r[0])?)?)),
        None => Ok(None),
    }
}

fn rtt_rows(state: &State, env: &Env) -> Result<Vec<(SiteId, Rtt)>> {
    let rows: Vec<RttRow> =
        state.storage().sql().exec("SELECT peer, median_ms, at FROM rtt ORDER BY peer", None)?.to_array()?;
    let me = SiteEnv::read(env)?.id;
    Ok(rows
        .into_iter()
        .map(|r| (me, Rtt { peer: SiteId(r.peer as u8), median_ms: r.median_ms as u16, at: Timestamp(r.at as u64) }))
        .collect())
}

/// The member's routes for one command family.
struct Routes<F: Family> {
    host: Rc<Host<F>>,
}

impl<F: Family> Routes<F> {
    async fn fetch(&self, mut req: Request, url: Url) -> Result<Response> {
        // Every request this object takes, by route: B0-18's leader load.
        *self.host.stats.borrow_mut().inbound.entry(url.path().to_owned()).or_insert(0) += 1;
        // Inside an injected `down_ms` window the member answers no Raft or edge call; the harness reads still work.
        #[cfg(feature = "test-faults")]
        {
            let raft_path = matches!(url.path(), "/raft" | "/read" | "/campaign" | "/suspect")
                || url.path().starts_with("/commit")
                || url.path().starts_with("/outcome");
            if raft_path && self.host.down() {
                return Response::error("member down (injected)", 503);
            }
            // A failing member class answers every call but the harness's observation routes.
            let observe = matches!(
                url.path(),
                "/status"
                    | "/stats"
                    | "/ping"
                    | "/rtt"
                    | "/log"
                    | "/audit"
                    | "/faults"
                    | "/isolate"
                    | "/deps_log"
                    | "/arrivals"
            );
            if !observe && crate::faults::site_down(&self.host.env).await {
                crate::faults::count("site_fault:in");
                return Response::error("member class failing (injected site fault)", 503);
            }
        }
        match (req.method(), url.path()) {
            (Method::Get, "/ping") => Response::empty(),
            (Method::Get, "/rtt") => Response::from_json(&rtt_rows(&self.host.state, &self.host.env)?),
            (Method::Post, "/raft") => {
                let body = req.bytes().await?;
                let r = self.raft(&body);
                self.host.settled().await;
                r
            }
            (Method::Post, "/commit") => {
                let body = match self.decode::<CommitReq<F>>(&req.bytes().await?) {
                    Ok(b) => b,
                    Err(r) => return r,
                };
                #[cfg(feature = "test-faults")]
                let (rid, receipt) =
                    (body.rid.clone(), body.receipt.as_deref().and_then(crate::faults::Receipt::parse));
                #[cfg(feature = "test-faults")]
                let role = self.role_name();
                let r = self
                    .ready_or(|| async {
                        let mut r = self.commit_all(vec![body]).await?;
                        self.reply(r.pop().expect("one result"))
                    })
                    .await;
                #[cfg(feature = "test-faults")]
                let r = self.receipt_fault(&rid, role, receipt, r).await;
                r
            }
            // Several commands admitted as separate `commit` inputs in one turn, so they share an entry.
            (Method::Post, "/commit_many") => {
                let body: CommitMany<F> = req.json().await?;
                #[cfg(feature = "test-faults")]
                if let Some(ms) = body.hold_deps_ms
                    && let Some(core) = self.host.core.borrow_mut().as_mut()
                {
                    core.hold_deps_until = now_ms() + ms;
                }
                #[cfg(feature = "test-faults")]
                let hold = body.hold_ms.map_or(raft::COMMIT_HOLD, Duration::from_millis);
                #[cfg(not(feature = "test-faults"))]
                let hold = raft::COMMIT_HOLD;
                self.ready_or(|| async {
                    let r = self.commit_all_held(body.cmds, hold).await?;
                    self.reply(json!({ "results": r }))
                })
                .await
            }
            (Method::Post, "/outcome") => {
                let body: Value = req.json().await?;
                self.ready_or(|| self.outcome(body)).await
            }
            (Method::Post, "/outcome_by_request") => {
                let body = match self.decode::<CommitReq<F>>(&req.bytes().await?) {
                    Ok(b) => b,
                    Err(r) => return r,
                };
                self.ready_or(|| async { self.outcome_by_request(body) }).await
            }
            (Method::Post, "/read") => {
                let body: Value = req.json().await?;
                self.ready_or(|| self.read(body)).await
            }
            (Method::Post, "/campaign") => self.ready_or(|| async { self.verdict(None) }).await,
            (Method::Post, "/suspect") => {
                let body: Value = req.json().await?;
                self.ready_or(|| async { self.verdict(Some(body)) }).await
            }
            (Method::Post, "/transfer") => {
                let body: Value = req.json().await?;
                self.ready_or(|| async { self.transfer(body) }).await
            }
            (Method::Post, "/reconfigure") => {
                let body: Value = req.json().await?;
                self.ready_or(|| self.reconfigure(body)).await
            }
            (Method::Post, "/replace") => {
                let body: Value = req.json().await?;
                self.ready_or(|| self.replace(body)).await
            }
            (Method::Post, "/learners") => {
                let body: Value = req.json().await?;
                self.ready_or(|| self.learners(body)).await
            }
            (Method::Get, "/status") => Response::from_json(&self.status()),
            // The committed entries `from..=to` with the family's fields (a repository's packs): the git cache
            // shard's `ChainLog::entries`.
            // Leader only, so the entries reach the commit index a `fresh` read just reported.
            (_, "/entries") => {
                let q = |k: &str| crate::registry::query(&url, k).ok().and_then(|v| v.parse::<u64>().ok());
                let nl =
                    self.host.core.borrow().as_ref().filter(|c| c.m.role() != Role::Leader).map(|c| c.m.not_leader());
                if let Some(nl) = nl {
                    return self.reply(json!({ "error": host::call_error_json(&CallError::NotLeader(nl)) }));
                }
                let e = self.host.entries_json(q("from").unwrap_or(1), q("to").unwrap_or(u64::MAX))?;
                self.reply(json!({ "entries": e }))
            }
            (Method::Get, "/stats") => self.stats(crate::registry::query(&url, "reset").is_ok()),
            (Method::Post, p) if F::route_paths().contains(&p) => {
                let body: Value = req.json().await?;
                let path = p.to_owned();
                self.ready_or(|| async {
                    match F::route(&self.host, &path, body).await {
                        Some(r) => self.reply(r?),
                        None => Response::error("not found", 404),
                    }
                })
                .await
            }
            #[cfg(feature = "test-faults")]
            _ => self.test_route(req, url).await,
            #[cfg(not(feature = "test-faults"))]
            _ => Response::error("not found", 404),
        }
    }

    /// The harness's routes, built only with `test-faults`.
    #[cfg(feature = "test-faults")]
    async fn test_route(&self, mut req: Request, url: Url) -> Result<Response> {
        match (req.method(), url.path()) {
            (Method::Get, "/log") => {
                let q = |k: &str| crate::registry::query(&url, k).ok().and_then(|v| v.parse().ok());
                Response::from_json(&self.host.log_json(q("from").unwrap_or(1), q("n").unwrap_or(500) as usize)?)
            }
            (Method::Get, "/audit") => {
                let prefix = crate::registry::query(&url, "prefix").unwrap_or_default();
                self.ready_or(|| async { Response::from_json(&self.host.audit_json(&prefix)?) }).await
            }
            (Method::Get, "/faults") => self.faults(crate::registry::query(&url, "rid").ok()),
            (Method::Get, "/deps_log") => self.deps_log(),
            (Method::Get, "/isolate") => Response::from_json(&crate::faults::isolate_json(&self.host.env)),
            (Method::Get, "/arrivals") => {
                let since = crate::registry::query(&url, "since").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
                self.arrivals(since)
            }
            (Method::Post, "/record") => {
                let body: RecordReq<F> = req.json().await?;
                self.ready_or(|| self.record(body)).await
            }
            (Method::Post, "/probe_vote") => self.ready_or(|| async { self.probe_vote() }).await,
            // B0-18's depth gauge: how many nested object calls are left from this member's ambient context. After
            // `afterMs`, a task (charged, as every send is, to the newest incoming request) starts a chain of calls
            // to this object's own `/depth_hop`, each made at once inside the previous hop's handler, and answers
            // the hops it reached before the platform refused one.
            (Method::Post, "/depth_left") => {
                let body: Value = req.json().await?;
                Delay::from(Duration::from_millis(body["afterMs"].as_u64().unwrap_or(20))).await;
                let (tx, rx) = futures_channel::oneshot::channel();
                let me =
                    self.host.core.borrow().as_ref().map(|c| c.join.me.clone()).ok_or_else(|| err("not joined"))?;
                let h = self.host.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let _ = tx.send(depth_hop(&h.env, &me, 0).await);
                });
                Response::from_json(&rx.await.unwrap_or_else(|_| json!({ "error": "dropped" })))
            }
            (Method::Post, "/depth_hop") => {
                let k = req.json::<Value>().await?["k"].as_u64().unwrap_or(0);
                let me =
                    self.host.core.borrow().as_ref().map(|c| c.join.me.clone()).ok_or_else(|| err("not joined"))?;
                Response::from_json(&depth_hop(&self.host.env, &me, k).await)
            }
            (Method::Post, "/read_bench") => {
                let body: Value = req.json().await?;
                self.ready_or(|| self.read_bench(body)).await
            }
            (Method::Post, "/reset") => {
                self.host.dying.set(true);
                host::abort(&self.host.state, "graftyard member reset");
                Response::error("reset", 503)
            }
            // Test stub of a lost member (BF-11's `ReplaceMember`): its storage is deleted and the object resets, so
            // it answers as a member that was never joined.
            (Method::Post, "/retire") => {
                let _ = &mut req;
                self.host.dying.set(true);
                self.host.state.storage().delete_all().await?;
                host::abort(&self.host.state, "graftyard member retired");
                Response::from_json(&json!({ "retired": true }))
            }
            _ => Response::error("not found", 404),
        }
    }

    fn meta(&self) -> Value {
        self.host.core.borrow().as_ref().map_or(Value::Null, |c| meta(&c.m))
    }

    #[cfg(feature = "test-faults")]
    fn role_name(&self) -> &'static str {
        match self.host.core.borrow().as_ref().map(|c| c.m.role()) {
            Some(Role::Leader) => "leader",
            Some(Role::Candidate) => "candidate",
            Some(Role::Follower) => "follower",
            None => "unjoined",
        }
    }

    /// BF-14: records the arrival of a `/commit` (its role on arrival, the receipt fault, and what it answered), then
    /// applies the fault: `delay` holds the reply past COMMIT_RECEIPT, `drop` replaces it with a 503.
    #[cfg(feature = "test-faults")]
    async fn receipt_fault(
        &self,
        rid: &str,
        role: &str,
        receipt: Option<crate::faults::Receipt>,
        r: Result<Response>,
    ) -> Result<Response> {
        use crate::faults::Receipt;
        let (r, answer) = match r {
            Ok(mut res) => {
                let status = res.status_code();
                let body = res.text().await.unwrap_or_default();
                let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let kind = ["done", "admitted", "error"]
                    .iter()
                    .find(|k| v.get(**k).is_some())
                    .map(|k| match (*k, v.pointer("/done/notLeader").or(v.pointer("/error/notLeader"))) {
                        (_, Some(_)) => "notLeader".to_owned(),
                        (k, None) => k.to_owned(),
                    })
                    .unwrap_or_else(|| format!("status {status}"));
                (Response::from_bytes(body.into_bytes()).map(|x| x.with_status(status)), kind)
            }
            Err(e) => (Err(e), "error".into()),
        };
        let _ = self.host.state.storage().sql().exec(
            "INSERT INTO arrival (rid, at, role, fault, answer) VALUES (?, ?, ?, ?, ?)",
            vec![
                rid.into(),
                host::i(now_ms()),
                role.into(),
                receipt.map(Receipt::name).map_or(SqlStorageValue::Null, Into::into),
                answer.into(),
            ],
        );
        match receipt {
            Some(Receipt::Delay) => {
                crate::faults::count("receipt_delay");
                Delay::from(Duration::from_millis(crate::faults::RECEIPT_DELAY_MS)).await;
                r
            }
            Some(Receipt::Drop) => {
                crate::faults::count("receipt_drop");
                Response::error("reply dropped (injected)", 503)
            }
            None => r,
        }
    }

    /// BF-14's arrivals since `since` (epoch ms), at most 20,000.
    #[cfg(feature = "test-faults")]
    fn arrivals(&self, since: u64) -> Result<Response> {
        let rows = host::rows(
            &self.host.state.storage().sql(),
            "SELECT rid, at, role, fault, answer FROM arrival WHERE at >= ? ORDER BY seq LIMIT 20000",
            vec![host::i(since)],
        )?
        .iter()
        .map(|r| {
            Ok(json!({
                "rid": host::text(&r[0])?,
                "at": host::int(&r[1])?,
                "role": host::text(&r[2])?,
                "fault": host::text(&r[3]).ok(),
                "answer": host::text(&r[4])?,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
        Response::from_json(&rows)
    }

    /// `POST /record`: the events checked against the fold, recorded, and held until their entry commits.
    #[cfg(feature = "test-faults")]
    async fn record(&self, body: RecordReq<F>) -> Result<Response> {
        use graftyard_raft::{Fold as _, Scratch as _};
        let rid = host::rid_from(&body.rid)?;
        {
            let core = self.host.core.borrow();
            let core = core.as_ref().ok_or_else(|| err("member not joined"))?;
            let mut s = core.m.group_fold().fold.scratch();
            for ev in &body.events {
                if let Err(why) = s.check(ev) {
                    return self.reply(json!({ "refused": why.to_string() }));
                }
                s.apply(ev);
            }
        }
        let format = *F::reads().first().ok_or_else(|| err("the family reads no format"))?;
        let rx = self.host.wait_recorded(rid);
        let r = self.host.input(|m, t| m.record(t, rid, format, body.events))?;
        if let Err(e) = r {
            return self.reply(json!({ "error": host::call_error_json(&e) }));
        }
        self.host.served();
        match within(Duration::from_secs(10), rx).await {
            Some(Ok(Ok(index))) => self.reply(json!({ "recorded": index.0 })),
            Some(Ok(Err(nl))) => self.reply(json!({ "error": host::call_error_json(&CallError::NotLeader(nl)) })),
            Some(Err(_)) => Response::error("member reset", 503),
            None => self.reply(json!({ "busy": true })),
        }
    }

    /// A commit body, or the reply to one whose command does not decode: the family's `Unsupported` for a format
    /// this release does not read, and 400 for the rest. Nothing is touched either way.
    #[allow(clippy::result_large_err)]
    fn decode<T: serde::de::DeserializeOwned>(&self, body: &[u8]) -> std::result::Result<T, Result<Response>> {
        serde_json::from_slice(body).map_err(|e| match F::unreadable(body) {
            Some(u) => {
                *self.host.stats.borrow_mut().received.entry("recv:unsupported".into()).or_insert(0) += 1;
                self.reply(json!({ "error": u }))
            }
            None => Response::error(format!("bad body: {e}"), 400),
        })
    }

    fn reply(&self, mut v: Value) -> Result<Response> {
        // An answer read while storage refused a fold row may lack rows: the caller sees a reset instead.
        if self.host.fold_lost() {
            return Response::error("member reset: fold storage lost", 503);
        }
        v["meta"] = self.meta();
        Response::from_json(&v)
    }

    async fn ready_or<T: std::future::Future<Output = Result<Response>>>(
        &self,
        f: impl FnOnce() -> T,
    ) -> Result<Response> {
        if let Some(e) = self.host.restore_error.borrow().clone() {
            return Response::error(format!("restore failed: {e}"), 500);
        }
        if !self.host.joined() {
            return Response::error("member not joined", 409);
        }
        let r = f().await;
        // A reply leaves only after the writes it may rest on: a deferred transaction runs first, and the output gate
        // then holds the reply until it is durable.
        self.host.settled().await;
        r
    }

    /// Steps each envelope and answers with the envelopes addressed back to the caller.
    fn raft(&self, body: &[u8]) -> Result<Response> {
        if !self.host.joined() {
            return Response::error("member not joined", 409);
        }
        let envs: Vec<raft::Envelope<F::Ev>> = raft::codec::from_bytes(body).map_err(|e| err(e.to_string()))?;
        let mut back = Vec::new();
        for e in envs {
            let from = e.from;
            let (_, out) = self.host.step(|m, t| m.step(t, from, e.msg))?;
            back.extend(self.host.dispatch(out, Some(from)));
        }
        let mut res = Response::from_bytes(raft::codec::to_bytes(&back))?;
        res.headers_mut().set("content-type", "application/octet-stream")?;
        Ok(res)
    }

    /// `commit` for each command in order, each its own input, with no await between them; each is held up to
    /// COMMIT_HOLD for its outcome, and past that answers its ticket.
    async fn commit_all(&self, cmds: Vec<CommitReq<F>>) -> Result<Vec<Value>> {
        self.commit_all_held(cmds, raft::COMMIT_HOLD).await
    }

    async fn commit_all_held(&self, cmds: Vec<CommitReq<F>>, hold: Duration) -> Result<Vec<Value>> {
        let mut waits = Vec::with_capacity(cmds.len());
        for c in cmds {
            let rid = host::rid_from(&c.rid)?;
            #[cfg(feature = "test-faults")]
            if let Some(k) = c.kill.as_deref() {
                let k = Kill::parse(k).ok_or_else(|| err(format!("unknown kill {k:?}")))?;
                if let Some(core) = self.host.core.borrow_mut().as_mut() {
                    core.kills.insert(rid, (k, c.down_ms.unwrap_or(0)));
                }
            }
            #[cfg(feature = "test-faults")]
            if c.no_marker
                && let Some(core) = self.host.core.borrow_mut().as_mut()
            {
                core.no_marker.insert(rid);
            }
            let mut c = c;
            let chain = self.host.core.borrow().as_ref().map(|k| k.join.chain);
            if let Some(chain) = chain {
                F::admit(&self.host.env, chain, &mut c.cmd).await;
            }
            let at = now_ms();
            let f = raft::Command::format(&c.cmd);
            *self.host.stats.borrow_mut().received.entry(format!("recv:{}.{}", f.family, f.version)).or_insert(0) += 1;
            let cmd = c.cmd.clone();
            let (r, out) = self.host.step(|m, t| m.commit(t, rid, c.cmd))?;
            let w = match r {
                Ok(a) => Ok((a.ticket, self.host.wait_ticket(a.ticket, Some((rid, at, Some(cmd)))))),
                Err(e) => {
                    #[cfg(feature = "test-faults")]
                    if let Some(core) = self.host.core.borrow_mut().as_mut() {
                        core.kills.remove(&rid);
                    }
                    Err(e)
                }
            };
            self.host.dispatch(out, None);
            waits.push(w);
        }
        self.host.served();
        // Every command is held up to the same COMMIT_HOLD from now; past it, each answers its ticket.
        let end = now_ms() + hold.as_millis() as u64;
        let mut out = Vec::with_capacity(waits.len());
        for w in waits {
            out.push(match w {
                Err(e) => json!({ "error": host::call_error_json(&e) }),
                Ok((ticket, mut rx)) => {
                    let left = Duration::from_millis(end.saturating_sub(now_ms()));
                    let r = if left.is_zero() {
                        match rx.try_recv() {
                            Ok(Some(o)) => Some(Ok(o)),
                            Ok(None) => None,
                            Err(c) => Some(Err(c)),
                        }
                    } else {
                        within(left, rx).await
                    };
                    match r {
                        Some(Ok(o)) => json!({ "done": self.host.reply_json(&o) }),
                        Some(Err(_)) => return Err(err("member reset")),
                        None => json!({ "admitted": host::ticket_json(&self.host.current(ticket)) }),
                    }
                }
            });
        }
        Ok(out)
    }

    /// `outcome` for a ticket, waiting up to `wait_ms` while it is pending.
    async fn outcome(&self, body: Value) -> Result<Response> {
        let ticket = self.host.current(host::ticket_from(&body["ticket"])?);
        let wait = body.get("wait_ms").and_then(Value::as_u64).unwrap_or(10_000);
        // A ticket refused for want of an id block is not done: the lease loop re-sends it.
        let now =
            self.host.core.borrow().as_ref().and_then(|c| c.m.outcome(ticket)).filter(|o| !host::needs_block::<F>(o));
        if let Some(o) = now {
            return self.reply(json!({ "done": self.host.reply_json(&o) }));
        }
        let rx = self.host.wait_ticket(ticket, None);
        self.host.kick();
        match within(Duration::from_millis(wait), rx).await {
            Some(Ok(o)) => self.reply(json!({ "done": self.host.reply_json(&o) })),
            Some(Err(_)) => Response::error("member reset", 503),
            None => self.reply(json!({ "pending": host::ticket_json(&ticket) })),
        }
    }

    fn outcome_by_request(&self, body: CommitReq<F>) -> Result<Response> {
        let rid = host::rid_from(&body.rid)?;
        let core = self.host.core.borrow();
        let m = &core.as_ref().ok_or_else(|| err("member not joined"))?.m;
        let v = match m.outcome_by_request(rid, &body.cmd) {
            Ok(ByRequest::Done(a)) => json!({ "done": self.host.reply_json(&Ok(a)) }),
            Ok(ByRequest::Pending(t)) => json!({ "pending": host::ticket_json(&t) }),
            Ok(ByRequest::Unknown) => json!({ "unknown": true }),
            Err(nl) => json!({ "error": host::call_error_json(&CallError::NotLeader(nl)) }),
        };
        drop(core);
        self.reply(v)
    }

    /// A `fresh` read: `{"key": k}` for the register, `{"q": {...}}` for a family's query (`Family::read_json`), or
    /// `{"q": {"dep": true}}` for the whole fold, which another group's dependency read decodes.
    async fn read(&self, body: Value) -> Result<Response> {
        let q = match body.get("q") {
            Some(q) => q.clone(),
            None => {
                json!({ "key": body.get("key").and_then(Value::as_str).ok_or_else(|| err("read needs key or q"))? })
            }
        };
        // `{"index": true}`: a follower's or learner's read, by read-index against the leader (`Member::read_index`).
        let index = body.get("index").and_then(Value::as_bool).unwrap_or(false);
        let (r, out) = self.host.step(|m, t| if index { m.read_index(t) } else { m.read(t) })?;
        let rx = r.as_ref().ok().map(|id| self.host.wait_read(*id, q));
        self.host.dispatch(out, None);
        let rx = match (r, rx) {
            (Ok(_), Some(rx)) => rx,
            (Err(nl), _) => return self.reply(json!({ "error": host::call_error_json(&CallError::NotLeader(nl)) })),
            (Ok(_), None) => unreachable!("a started read has a waiter"),
        };
        self.host.served();
        match within(raft::CALL_DEADLINE * 3, rx).await {
            Some(Ok(Ok((value, index)))) => self.reply(json!({ "value": value, "index": index.0 })),
            Some(Ok(Err(nl))) => self.reply(json!({ "error": host::call_error_json(&CallError::NotLeader(nl)) })),
            Some(Err(_)) => Response::error("member reset", 503),
            None => self.reply(json!({ "busy": true })),
        }
    }

    /// `campaign` with no body, `suspect` with `{colo, term}`. Campaigns start only here, from requests.
    fn verdict(&self, report: Option<Value>) -> Result<Response> {
        let v = match report {
            None => self.host.input(|m, t| m.campaign(t))?,
            Some(b) => {
                let colo = raft::Colo(b.get("colo").and_then(Value::as_u64).unwrap_or(0) as u32);
                let term = raft::Term(b.get("term").and_then(Value::as_u64).unwrap_or(0));
                let about =
                    SiteId(b.get("about").and_then(Value::as_u64).ok_or_else(|| err("suspect needs about"))? as u8);
                self.host.input(|m, t| m.suspect(t, colo, about, term))?
            }
        };
        self.reply(json!({ "verdict": verdict_str(v) }))
    }

    fn status(&self) -> Value {
        let core = self.host.core.borrow();
        let site = SiteEnv::read(&self.host.env).map(|s| s.id.0).ok();
        let Some(c) = core.as_ref() else {
            return json!({ "site": site, "joined": false, "restoreError": *self.host.restore_error.borrow() });
        };
        let s = c.m.state();
        json!({
            "site": site,
            "joined": true,
            "doName": c.join.me.do_name,
            "role": match c.m.role() { Role::Leader => "leader", Role::Candidate => "candidate", Role::Follower => "follower" },
            "term": s.term.0,
            "leader": c.m.leader().map(|l| l.0),
            "ledTerm": s.led_term.map(|t| t.0),
            "votedFor": s.voted_for.map(|v| v.0),
            "last": c.m.last_index().0,
            "base": c.m.log_base().index.0,
            "commit": s.commit.0,
            "applied": s.applied.0,
            "promiseUntil": s.promise_until.0,
            "boots": [s.boots[0].0, s.boots[1].0],
            "boot": c.m.boot().to_string(),
            "meta": meta(&c.m),
            "config": c.m.committed_config().map(|(i, cfg)| json!({ "index": i.0, "config": cfg })),
            "boot1": c.m.digest_of(raft::Index(1)).map(|d| d.hex()),
            "family": format!("{:?}", Kind::of(&c.join)),
            "restores": self.host.stats.borrow().restores,
            "now": now_ms(),
        })
    }

    #[cfg(feature = "test-faults")]
    fn faults(&self, rid: Option<String>) -> Result<Response> {
        let sql = self.host.state.storage().sql();
        let (q, args) = match &rid {
            Some(r) => (
                "SELECT seq, rid, mode, idx, term, result, at FROM fault WHERE rid = ? ORDER BY seq",
                vec![r.clone().into()],
            ),
            None => ("SELECT seq, rid, mode, idx, term, result, at FROM fault ORDER BY seq", vec![]),
        };
        let rows = host::rows(&sql, q, args)?
            .iter()
            .map(|r| {
                Ok(json!({
                    "seq": host::int(&r[0])?,
                    "rid": host::text(&r[1])?,
                    "mode": host::text(&r[2])?,
                    "index": host::int(&r[3])?,
                    "term": host::int(&r[4])?,
                    "result": serde_json::from_str::<Value>(host::text(&r[5])?)?,
                    "at": host::int(&r[6])?,
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        Response::from_json(&rows)
    }

    fn stats(&self, reset: bool) -> Result<Response> {
        let mut s = self.host.stats.borrow_mut();
        let v = json!({
            "site": SiteEnv::read(&self.host.env).map(|s| s.id.0).ok(),
            "role": self.host.core.borrow().as_ref().map(|c| matches!(c.m.role(), Role::Leader)),
            "commitMs": s.commit_ms,
            "roundMs": s.round_ms,
            "applyMs": s.apply_ms,
            "applySteps": s.apply_steps,
            "confirmMs": s.confirm_ms,
            "appendMs": s.append_ms,
            "entries": s.entries,
            "commands": s.commands,
            "restores": s.restores,
            "installs": s.installs,
            "inbound": s.inbound,
            "inboundMs": now_ms().saturating_sub(s.inbound_since),
            "failedCalls": s.failed_calls,
            "depReads": s.dep_reads,
            "catchups": s.catchups,
            "leases": s.leases,
            "markerFailures": s.marker_failures,
            "truncatedTo": s.truncated_to,
            "received": s.received,
            "alarms": s.alarms,
            "alarmRuns": s.alarm_runs,
            "alarmLast": s.alarm_last,
            "alarmArms": s.alarm_arms,
            "prsyncSent": s.prsync_sent,
            "prsyncFirst": s.prsync_first,
            "peers": s.peers,
            "calls": s.calls,
        });
        if reset {
            s.commit_ms.clear();
            s.round_ms.clear();
            s.apply_ms.clear();
            s.apply_steps.clear();
            s.confirm_ms.clear();
            s.append_ms.clear();
            s.entries = 0;
            s.commands = 0;
            s.failed_calls = 0;
            s.peers.clear();
            s.calls.clear();
            s.apply_ms.clear();
            s.inbound.clear();
            s.inbound_since = now_ms();
        }
        Response::from_json(&v)
    }

    /// Writes the registry row on this member's own site before anything else is stored, then measures the other
    /// voters. Joining again with the same identity measures again only if the last measurement is a day old. A
    /// member that has joined builds its Raft core, which stores nothing until the group's first message.
    async fn join(&self, join: Join) -> Result<Response> {
        let env = &self.host.env;
        let site = SiteEnv::read(env)?;
        if join.me.site != site.id {
            return Response::error(format!("member for {:?} reached site {:?}", join.me.site, site.id), 409);
        }
        let row = RegistryRow {
            chain: join.chain,
            do_name: join.me.do_name.clone(),
            generation: join.me.generation,
            sites: join.record.sites.clone(),
        };
        // A joined member keeps the identity and record it joined with: its bootstrap bytes rest on them.
        if let Some(c) = self.host.core.borrow().as_ref()
            && (c.join.me != join.me || c.join.record != join.record || c.join.chain != join.chain)
        {
            return Response::from_json(
                &json!({ "refused": { "joinedAs": c.join.me.do_name, "generation": c.join.record.generation } }),
            )
            .map(|r| r.with_status(409));
        }
        // A member added later, by a retry, a replacement, or a move, takes a generation above its record's and is
        // admitted by the group's config change; only bootstrap members check the record's source.
        let bootstrap = join.me.generation == join.record.generation;
        if bootstrap
            && !self.host.joined()
            && let Err(why) = self.check_source(&site, &join).await
        {
            return Response::from_json(&json!({ "refused": why })).map(|r| r.with_status(409));
        }
        // A learner writes no registry row: the row names the chain's voter on this site, which `ReplaceMember` and the
        // lookup rule read, and a learner holds no vote and is placed again by `place_learners`. It measures nothing:
        // `Placement::new` checks voter pairs only.
        let learner = join.me.kind == graftyard_types::MemberKind::Learner;
        if learner {
            self.store_join(&join)?;
            return Response::from_json(&json!([]));
        }
        let reg = MemberRegistry::for_chain(site.id, join.chain);
        let mut put = RequestInit::new();
        put.with_method(Method::Put).with_body(Some(serde_json::to_string(&row)?.into()));
        let res = site
            .registry(env, site.id, &reg.do_name())?
            .fetch_with_request(Request::new_with_init("https://registry/row", &put)?)
            .await?;
        if res.status_code() >= 300 {
            return Response::error(format!("registry write failed: {}", res.status_code()), 502);
        }
        // A member whose site's row names a later generation of its chain was replaced: it never stores anything
        // again, so a retired bootstrap name cannot bootstrap a second group.
        if !self.host.joined() {
            let url = format!("https://registry/row?chain={}", join.chain);
            let mut got = site.registry(env, site.id, &reg.do_name())?.fetch_with_str(&url).await?;
            if got.status_code() == 200 {
                let current: RegistryRow = got.json().await?;
                if current.generation > join.me.generation {
                    return Response::from_json(&json!({ "refused": { "superseded": current } }))
                        .map(|r| r.with_status(409));
                }
            }
        }
        self.store_join(&join)?;

        let now = now_ms();
        let mine = rtt_rows(&self.host.state, env)?;
        let fresh = mine.iter().all(|r| now.saturating_sub(r.1.at.0) < DAY_MS);
        let peers: Vec<MemberRef> = if join.peers.is_empty() {
            join.record.bootstrap_members(join.chain).into_iter().filter(|m| m.site != site.id).collect()
        } else {
            join.peers.iter().filter(|m| m.site != site.id).cloned().collect()
        };
        let measured = mine.len();
        if !(fresh && measured == peers.len()) {
            for peer in &peers {
                let median = self.measure(&site, peer).await?;
                self.host.state.storage().sql().exec(
                    "INSERT OR REPLACE INTO rtt (peer, median_ms, at) VALUES (?, ?, ?)",
                    vec![i64::from(peer.site.0).into(), i64::from(median).into(), (now as i64).into()],
                )?;
            }
        }
        Response::from_json(&rtt_rows(&self.host.state, env)?)
    }

    fn store_join(&self, join: &Join) -> Result<()> {
        self.host
            .state
            .storage()
            .sql()
            .exec("INSERT OR REPLACE INTO me (k, v) VALUES ('join', ?)", vec![serde_json::to_string(join)?.into()])?;
        if !self.host.joined() {
            self.host.restore()?;
        }
        Ok(())
    }

    /// Median of RTT_SAMPLES sequential binding round trips to the peer's object.
    async fn measure(&self, site: &SiteEnv, peer: &MemberRef) -> Result<u16> {
        let stub = site.member(&self.host.env, peer.site, &peer.do_name)?;
        let mut samples = Vec::with_capacity(RTT_SAMPLES);
        for _ in 0..RTT_SAMPLES {
            let t = now_ms();
            stub.fetch_with_str("https://member/ping").await?;
            samples.push(now_ms() - t);
        }
        samples.sort_unstable();
        Ok(samples[RTT_SAMPLES / 2].min(u64::from(u16::MAX)) as u16)
    }

    /// The bootstrap check (`design/DESIGN.md`, Shape, `bootstrap`): before a member stores anything it reads its
    /// chain's record from the record's source itself, and joins only if the record it was sent is the source's, or
    /// one the source keeps in `moving_from`. Then it claims the record generation that bootstraps the chain on one
    /// registry, the first record site of the oldest record in the source's `lookup_order`, and joins only if its
    /// generation won: edges holding two generations of an owner's record during a move build one group.
    async fn check_source(&self, site: &SiteEnv, join: &Join) -> std::result::Result<(), Value> {
        let source = match F::source(join) {
            Source::Given => return Ok(()),
            Source::Genesis => {
                crate::release::system_record(&self.host.env, join.chain).await.map_err(|e| json!({ "noRecord": e }))?
            }
            Source::Owner(owner) => owner_record(&self.host.env, owner).await.map_err(|e| json!({ "source": e }))?,
            Source::Repo { repo, owner } => {
                let named = repo_owner(&self.host.env, repo).await.map_err(|e| json!({ "source": e }))?;
                if named != owner {
                    return Err(json!({ "notTheRepoOwner": named.0.0 }));
                }
                owner_record(&self.host.env, owner).await.map_err(|e| json!({ "source": e }))?
            }
            // The genesis was accepted against this script's anchor when the member was built
            // (`Family::new_fold`); its `Release` record is the source, and no other generation exists.
            Source::Anchor => {
                let r = crate::release::genesis_record(&self.host.env, join).map_err(|e| json!({ "genesis": e }))?;
                return if r == join.record { Ok(()) } else { Err(json!({ "notTheGenesisRecord": r })) };
            }
        };
        // BF-10's control: the design's sentence alone ("after it reads the same record generation from the record's
        // source itself"), with no claim.
        #[cfg(feature = "test-faults")]
        if join.no_claim {
            return if source == join.record { Ok(()) } else { Err(json!({ "notTheSourcesGeneration": source })) };
        }
        if !source.lookup_order().any(|r| *r == join.record) {
            return Err(json!({ "notTheSourcesRecord": source }));
        }
        let oldest = source.lookup_order().last().expect("one record");
        let at = *oldest.sites.first().ok_or_else(|| json!("empty record"))?;
        let reg = MemberRegistry::for_chain(at, join.chain);
        let body = json!({ "chain": join.chain, "generation": join.record.generation });
        let won = async {
            let stub = site.registry(&self.host.env, at, &reg.do_name()).ok()?;
            let mut init = RequestInit::new();
            init.with_method(Method::Post).with_body(Some(body.to_string().into()));
            let mut res =
                stub.fetch_with_request(Request::new_with_init("https://registry/claim", &init).ok()?).await.ok()?;
            res.json::<Value>().await.ok()?.get("generation").and_then(Value::as_u64)
        };
        match within(Duration::from_secs(5), won).await.flatten() {
            Some(g) if g == u64::from(join.record.generation) => Ok(()),
            Some(g) => Err(json!({ "claimedBy": g, "source": source })),
            None => Err(json!("claim unanswered")),
        }
    }

    /// BF-10's probe: a vote request from another voter of the bootstrap config, for the next term. A member with no
    /// committed config must refuse it (`Deny::NoConfig`). Only for members with no committed config, so it never
    /// takes a real vote.
    #[cfg(feature = "test-faults")]
    fn probe_vote(&self) -> Result<Response> {
        let (from, term, configured) = {
            let core = self.host.core.borrow();
            let c = core.as_ref().ok_or_else(|| err("member not joined"))?;
            let from = c.join.record.sites.iter().copied().find(|s| *s != c.m.me()).ok_or_else(|| err("no peer"))?;
            (from, c.m.term(), c.m.committed_config().is_some())
        };
        if configured {
            return Response::error("member has a committed config: not probed", 409);
        }
        let req = raft::VoteReq {
            term: raft::Term(term.0 + 1),
            candidate: from,
            pre: false,
            last_index: raft::Index(u64::MAX / 2),
            last_term: raft::Term(term.0 + 1),
            transfer: None,
            boots: [Timestamp(0); 2],
            stepped_down_at: None,
            bootstrapped: false,
        };
        let (_, out) = self.host.step(|m, t| m.step(t, from, raft::Message::Vote(req)))?;
        let back = self.host.dispatch(out, Some(from));
        let res = back.into_iter().find_map(|e| match e.msg {
            raft::Message::VoteRes(r) => Some(r),
            _ => None,
        });
        // And a read, in the same turn, so nothing commits a config between the two probes.
        let (read, out) = self.host.step(|m, t| m.read(t))?;
        self.host.dispatch(out, None);
        Response::from_json(&json!({
            "granted": res.as_ref().map(|r| r.granted),
            "deny": res.and_then(|r| r.deny).map(|d| format!("{d:?}")),
            "readServed": read.is_ok(),
            "readConfig": read.err().map(|nl| nl.config.is_some()),
        }))
    }

    /// `transfer` to `{"to": site}`. With `test-faults`, `"abandon": true` drops its `TimeoutNow`, so the leader
    /// abandons it after the LEASE (B0-11's abandoned transfers).
    fn transfer(&self, body: Value) -> Result<Response> {
        let to = SiteId(body.get("to").and_then(Value::as_u64).ok_or_else(|| err("transfer needs to"))? as u8);
        let r = self.host.input(|m, t| m.transfer(t, to))?;
        #[cfg(feature = "test-faults")]
        if let Ok(id) = &r
            && body.get("abandon").and_then(Value::as_bool).unwrap_or(false)
        {
            self.host.abandon.borrow_mut().insert(id.0);
        }
        self.reply(json!({ "transfer": r.map(|id| id.0.to_string()).map_err(|e| format!("{e:?}")) }))
    }

    /// One config change, `{"config": GroupConfig}`, then waits up to 15 s for it to commit.
    async fn reconfigure(&self, body: Value) -> Result<Response> {
        let cfg: raft::GroupConfig = serde_json::from_value(body["config"].clone())?;
        let r = self.host.input(|m, t| m.reconfigure(t, cfg))?;
        let index = match r {
            Ok(i) => i,
            Err(e) => return self.reply(json!({ "error": format!("{e:?}") })),
        };
        self.host.served();
        let end = now_ms() + 15_000;
        while now_ms() < end {
            self.host.kick();
            let committed = self.host.core.borrow().as_ref().and_then(|c| c.m.committed_config().map(|c| c.0));
            if committed.is_some_and(|c| c >= index) {
                return self.reply(json!({ "committed": index.0 }));
            }
            Delay::from(Duration::from_millis(50)).await;
        }
        self.reply(json!({ "pending": index.0 }))
    }

    /// A config change committed by this leader, retried while it is busy: `Ok` with the commit index.
    async fn change(&self, cfg: raft::GroupConfig) -> std::result::Result<u64, String> {
        let end = now_ms() + 20_000;
        let index = loop {
            match self.host.input(|m, t| m.reconfigure(t, cfg.clone())).map_err(|e| e.to_string())? {
                Ok(i) => break i,
                Err(raft::ReconfigError::Busy) if now_ms() < end => Delay::from(Duration::from_millis(100)).await,
                Err(e) => return Err(format!("{e:?}")),
            }
        };
        self.host.served();
        while now_ms() < end {
            self.host.kick();
            let c = self.host.core.borrow().as_ref().and_then(|c| c.m.committed_config().map(|c| c.0));
            if c.is_some_and(|c| c >= index) {
                return Ok(index.0);
            }
            Delay::from(Duration::from_millis(50)).await;
        }
        Err(format!("config at {index:?} did not commit"))
    }

    async fn post_member(&self, to: &MemberRef, path: &str, body: &Value) -> Result<(u16, Value)> {
        let site = SiteEnv::read(&self.host.env)?;
        let stub = site.member(&self.host.env, to.site, &to.do_name)?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(body.to_string().into()));
        let mut res = stub.fetch_with_request(Request::new_with_init(&format!("https://member{path}"), &init)?).await?;
        let status = res.status_code();
        Ok((status, res.json::<Value>().await.unwrap_or(Value::Null)))
    }

    /// Replaces this group's member on `{"site": s}` with a new member of the next generation on the same site: the
    /// placement-failure retry (`design/DESIGN.md`, "Install operations", Placement failures), and the test
    /// install's stand-in for `ReplaceMember`, whose spare is the same site because the test install has no spare
    /// sites (the caller retires the lost member first, with `/retire`). Two config changes, `Degraded { lost: s }`
    /// and then `Normal` with the new member, because votes and acks are counted by site. The new member joins with
    /// the group's bootstrap record and measures the config's other voters; `Placement::new` checks those round
    /// trips, and a refused member is retired and retried with the next generation, up to three times. The old
    /// member is left as it is: a removed member keeps answering with its latest committed config (Shape, "Sites and
    /// placement").
    async fn replace(&self, body: Value) -> Result<Response> {
        let to = SiteId(body.get("site").and_then(Value::as_u64).ok_or_else(|| err("replace needs site"))? as u8);
        // Test fault: the first new member's latency check fails, so the retry with the next generation runs.
        #[cfg(feature = "test-faults")]
        let fail_first = body.get("fail_first").and_then(Value::as_bool).unwrap_or(false);
        #[cfg(not(feature = "test-faults"))]
        let fail_first = false;
        let (chain, record, me, cfg, c_owner, c_genesis) = {
            let core = self.host.core.borrow();
            let c = core.as_ref().ok_or_else(|| err("member not joined"))?;
            if c.m.role() != Role::Leader {
                drop(core);
                return self.reply(json!({ "error": { "notLeader": host::nl_json(&self.host.core.borrow().as_ref().unwrap().m.not_leader()) } }));
            }
            (
                c.join.chain,
                c.join.record.clone(),
                c.m.me(),
                c.m.committed_config().map(|c| c.1.clone()),
                c.join.owner,
                c.join.genesis.clone(),
            )
        };
        let Some(raft::GroupConfig::Normal(p)) = cfg else {
            return self.reply(json!({ "error": "the committed config is not Normal" }));
        };
        if to == me {
            return self.reply(json!({ "error": "transfer first: the leader's own site" }));
        }
        let old = p.voters().iter().find(|m| m.site == to).cloned().ok_or_else(|| err("no member on that site"))?;
        let t0 = now_ms();
        if let Err(e) = self.change(raft::GroupConfig::Degraded { placement: p.clone(), lost: to }).await {
            return self.reply(json!({ "error": format!("degraded: {e}") }));
        }
        let site = SiteEnv::read(&self.host.env)?;
        let mine = rtt_rows(&self.host.state, &self.host.env)?;
        let others: Vec<MemberRef> = p.voters().iter().filter(|m| m.site != to).cloned().collect();
        let mut generation = p.next_generation();
        let mut tried = Vec::new();
        let placed = loop {
            let new = MemberRef::new(chain, to, old.kind, old.slot, generation);
            let join = Join {
                chain,
                me: new.clone(),
                record: record.clone(),
                peers: others.clone(),
                owner: c_owner,
                genesis: c_genesis.clone(),
                #[cfg(feature = "test-faults")]
                no_claim: false,
            };
            let (status, rows) = self.post_member(&new, "/join", &serde_json::to_value(&join)?).await?;
            let theirs: Vec<(SiteId, Rtt)> = serde_json::from_value(rows.clone()).unwrap_or_default();
            let voters: arrayvec::ArrayVec<MemberRef, 3> =
                p.voters().iter().map(|m| if m.site == to { new.clone() } else { m.clone() }).collect();
            let all: Vec<(SiteId, Rtt)> = mine.iter().chain(&theirs).copied().collect();
            let checks = graftyard_types::PlacementChecks { sites: &site.sites, separate_accounts: false, rtt: &all };
            let r = if fail_first && tried.is_empty() {
                Err("injected: latency check failed".to_owned())
            } else if status == 200 {
                graftyard_types::Placement::new(voters, arrayvec::ArrayVec::new(), p.home(), &checks)
                    .map_err(|e| e.to_string())
            } else {
                Err(format!("join answered {status}: {rows}"))
            };
            tried.push(json!({ "member": new.do_name, "result": r.as_ref().err() }));
            match r {
                Ok(np) => break Some(np),
                Err(_) if tried.len() < 3 => {
                    // The refused member never joins the group; with `test-faults` it is also cleared, so the test
                    // install keeps no dead objects.
                    #[cfg(feature = "test-faults")]
                    let _ = self.post_member(&new, "/retire", &json!({})).await;
                    generation += 1;
                }
                Err(_) => break None,
            }
        };
        let Some(np) = placed else {
            return self.reply(json!({ "error": "no placement passed", "tried": tried }));
        };
        let index = match self.change(raft::GroupConfig::Normal(Box::new(np))).await {
            Ok(i) => i,
            Err(e) => return self.reply(json!({ "error": format!("normal: {e}"), "tried": tried })),
        };
        self.reply(json!({ "old": old.do_name, "tried": tried, "configIndex": index, "ms": now_ms() - t0 }))
    }

    /// The leader's half of learner placement (`design/DESIGN.md`, Shape, `place_learners`): `{"load": GroupLoad}`
    /// names each region's peak read rate. `place_learners` picks the regions; each learner goes to the first
    /// non-spare site in its region, in slot order, under the next generation, joins, and one config change replaces
    /// the group's learners with them (none when the load is under `LEARNER_AT`). The design runs this from a daily
    /// `graft-ops` cron over Analytics Engine counters; a test install has neither, so its driver posts the load here.
    async fn learners(&self, body: Value) -> Result<Response> {
        let load: graftyard_types::GroupLoad = serde_json::from_value(body["load"].clone())?;
        let regions = graftyard_types::place_learners(&load);
        let (chain, record, cfg, c_owner, c_genesis) = {
            let core = self.host.core.borrow();
            let c = core.as_ref().ok_or_else(|| err("member not joined"))?;
            if c.m.role() != Role::Leader {
                return self.reply(json!({ "error": { "notLeader": host::nl_json(&c.m.not_leader()) } }));
            }
            (
                c.join.chain,
                c.join.record.clone(),
                c.m.committed_config().map(|c| c.1.clone()),
                c.join.owner,
                c.join.genesis.clone(),
            )
        };
        let Some(raft::GroupConfig::Normal(p)) = cfg else {
            return self.reply(json!({ "error": "the committed config is not Normal" }));
        };
        let site = SiteEnv::read(&self.host.env)?;
        let generation = p.next_generation();
        let mut learners = arrayvec::ArrayVec::<MemberRef, 16>::new();
        let mut joined = Vec::new();
        for (k, region) in regions.iter().enumerate() {
            let Some(s) = site.sites.0.iter().find(|s| s.region == *region && !s.spare) else { continue };
            let l = MemberRef::new(chain, s.id, graftyard_types::MemberKind::Learner, k as u8, generation);
            let join = Join {
                chain,
                me: l.clone(),
                record: record.clone(),
                peers: p.voters().to_vec(),
                owner: c_owner,
                genesis: c_genesis.clone(),
                #[cfg(feature = "test-faults")]
                no_claim: false,
            };
            let (status, _) = self.post_member(&l, "/join", &serde_json::to_value(&join)?).await?;
            joined.push(
                json!({ "site": l.site.0, "slot": l.slot, "addr": l.addr().0, "doName": l.do_name, "join": status }),
            );
            if status != 200 {
                return self.reply(json!({ "error": "a learner did not join", "joined": joined }));
            }
            learners.push(l);
        }
        // Voter pairs' round trips, from each voter's own rows.
        let mut rtt = rtt_rows(&self.host.state, &self.host.env)?;
        for v in p.voters().iter().filter(|v| v.site != site.id) {
            let stub = site.member(&self.host.env, v.site, &v.do_name)?;
            let rows: Vec<(SiteId, Rtt)> = stub.fetch_with_str("https://member/rtt").await?.json().await?;
            rtt.extend(rows);
        }
        let checks = graftyard_types::PlacementChecks { sites: &site.sites, separate_accounts: false, rtt: &rtt };
        let voters: arrayvec::ArrayVec<MemberRef, 3> = p.voters().iter().cloned().collect();
        let np = match graftyard_types::Placement::new(voters, learners, p.home(), &checks) {
            Ok(np) => np,
            Err(e) => return self.reply(json!({ "error": e.to_string(), "joined": joined })),
        };
        match self.change(raft::GroupConfig::Normal(Box::new(np))).await {
            Ok(i) => self.reply(json!({ "regions": regions, "learners": joined, "configIndex": i })),
            Err(e) => self.reply(json!({ "error": e, "learners": joined })),
        }
    }

    /// B0-18's reads, generated inside this member as Milestone 0's learners did (`tests/m0/run/raft-learners.ts`):
    /// `rate` read-index reads a second for `secs`, each `{"q": {"ranges": true}}`, and each one's latency, start to
    /// answer. The leader's load is real; this member's own request limit is not exercised.
    #[cfg(feature = "test-faults")]
    async fn read_bench(&self, body: Value) -> Result<Response> {
        let rate = body["rate"].as_u64().unwrap_or(1_000).max(1);
        let secs = body["secs"].as_u64().unwrap_or(30);
        let n = rate * secs;
        let t0 = now_ms();
        let mut waits = Vec::with_capacity(n as usize);
        let mut refused = 0u64;
        for k in 0..n {
            let due = t0 + k * 1000 / rate;
            let now = now_ms();
            if due > now {
                Delay::from(Duration::from_millis(due - now)).await;
            }
            let at = now_ms();
            let (r, out) = self.host.step(|m, t| m.read_index(t))?;
            match r {
                // Each read times itself as it is answered, not when the loop gets to it.
                Ok(id) => {
                    let rx = self.host.wait_read(id, json!({ "ranges": true }));
                    let (tx, done) = futures_channel::oneshot::channel();
                    wasm_bindgen_futures::spawn_local(async move {
                        let end = match within(Duration::from_secs(10), rx).await {
                            Some(Ok(Ok(_))) => "ok",
                            Some(Ok(Err(_))) => "notLeader",
                            Some(Err(_)) => "dropped",
                            None => "timeout",
                        };
                        let _ = tx.send((end, at, now_ms().saturating_sub(at)));
                    });
                    waits.push(done);
                }
                Err(_) => refused += 1,
            }
            self.host.dispatch(out, None);
        }
        let (mut lat, mut failed) = (Vec::with_capacity(waits.len()), 0u64);
        // Failures by kind, and per second of the run: reads answered, failed, and the slowest answer.
        let mut ends: std::collections::BTreeMap<&'static str, u64> = std::collections::BTreeMap::new();
        let mut per_s: std::collections::BTreeMap<u64, (u32, u32, u64)> = std::collections::BTreeMap::new();
        for done in waits {
            let (end, at, ms) = done.await.unwrap_or(("dropped", t0, 0));
            *ends.entry(end).or_insert(0) += 1;
            let sec = per_s.entry(at.saturating_sub(t0) / 1000).or_default();
            if end == "ok" {
                lat.push(ms);
                sec.0 += 1;
                sec.2 = sec.2.max(ms);
            } else {
                failed += 1;
                sec.1 += 1;
            }
        }
        lat.sort_unstable();
        let pct = |p: f64| lat.get(((lat.len() as f64 * p) as usize).min(lat.len().saturating_sub(1))).copied();
        let site = SiteEnv::read(&self.host.env)?.id.0;
        Response::from_json(&json!({
            "site": site, "n": n, "ok": lat.len(), "refused": refused, "failed": failed, "ms": now_ms() - t0,
            "p50": pct(0.5), "p99": pct(0.99), "max": lat.last(), "t0": t0, "ends": ends,
            "perSecond": per_s.iter().map(|(k, (ok, f, max))| json!([k, ok, f, max])).collect::<Vec<_>>(),
        }))
    }

    /// Every dependency read this member's leaders made: when, and each chain's read index or error.
    #[cfg(feature = "test-faults")]
    fn deps_log(&self) -> Result<Response> {
        let sql = self.host.state.storage().sql();
        let rows = host::rows(&sql, "SELECT idx, at, reads FROM deps_log ORDER BY seq", vec![])?
            .iter()
            .map(|r| {
                Ok(json!({
                    "index": host::int(&r[0])?,
                    "at": host::int(&r[1])?,
                    "reads": serde_json::from_str::<Value>(host::text(&r[2])?)?,
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        Response::from_json(&rows)
    }
}

/// An owner's placement record, read through the `fresh` view of the `Names` leader of `account_shard(owner)`.
pub async fn owner_record(env: &Env, owner: AccountId) -> std::result::Result<PlacementRecord, String> {
    let chain = ChainId::Names(graftyard_identity::account_shard(owner));
    let record = crate::release::system_record(env, chain).await?;
    let (v, _) = lookup::call(env, chain, &record, "/read", &json!({ "q": { "placement": owner.0.0 } })).await?;
    serde_json::from_value::<Option<PlacementRecord>>(v.pointer("/value/placement").cloned().unwrap_or(Value::Null))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{owner:?} has no placement record"))
}

/// A repository's owner, from its `RepoOwner` row on `Names` shard `name_shard(Repo(repo))`, through `fresh`.
pub async fn repo_owner(env: &Env, repo: graftyard_types::RepoId) -> std::result::Result<AccountId, String> {
    let chain = ChainId::Names(graftyard_identity::name_shard(&graftyard_identity::NameKey::Repo(repo)));
    let record = crate::release::system_record(env, chain).await?;
    let (v, _) = lookup::call(env, chain, &record, "/read", &json!({ "q": { "repoOwner": repo.0.0 } })).await?;
    serde_json::from_value::<Option<AccountId>>(v.pointer("/value/repoOwner").cloned().unwrap_or(Value::Null))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{repo:?} has no RepoOwner row"))
}
