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
use crate::host::{self, Host, Trace, err, within};
use crate::{SiteEnv, lookup, now_ms};

/// How long a learner's `ReadIndex` call stays open for the leader's answer when it needs a round [guess]: under
/// the asker's CALL_DEADLINE, past one round to the far voter (230 ms measured, `tests/m2/results/install-m2b.md`).
const READ_INDEX_HOLD: Duration = Duration::from_millis(500);

/// Round trips per peer; the row keeps their median.
const RTT_SAMPLES: usize = 20;
/// Whole-tier reads (`{"q": {"rows": true}}`) a leader serves at once [guess]: on a 500-PR repository, whose packed
/// dump is 1.18 MB, the leader's wasm memory was 59 MB at 40 in flight and 92 MB at 60, and 100 reset the isolate,
/// while throughput stopped growing past about 30 (`tests/am1/results/lm1-memory.md`, "Cause").
pub const HEAVY_READS: u32 = 16;
/// The `retryAfterMs` of a shed whole-tier read: about one cached dump's service time.
const OVERLOAD_RETRY_MS: u64 = 250;
/// Reads of any kind a member serves at once, counted from arrival until the answer's last piece is taken
/// [guess]: on B0-29 the apac leader, granted about 600 reads in one input, ran out of memory one second later, and
/// probes of 1,000 or more narrow reads at once moved an idle group's leader on `CommitErrors`
/// (`tests/am1/results/lm1-memory.md`, "Rows"). The leader takes about 300 req/s at journey 9, mostly reads of
/// 1 s or less, so 256 leaves room for a second's worth.
pub const READS_MAX: u32 = 256;

/// A whole-tier read: `{"rows": true}`, not a narrow `{"rows": {"access" | "agent" | "merge": ..}}` one.
fn heavy_read(q: &Value) -> bool {
    q.get("rows").is_some_and(Value::is_boolean)
}

/// How long a follower waits to apply the leader's read index before it answers `overloaded` [guess]: one that is
/// behind or installing a snapshot never answers from an older fold, and the edge asks the next member
/// (`design/DESIGN.md`, "Follower read"; `graftyard_lookup::read_spread`).
pub const FOLLOWER_WAIT: Duration = Duration::from_secs(1);

/// What a member does with a `/read` before it starts it.
#[derive(Debug, PartialEq, Eq)]
enum Gate {
    Serve,
    /// A follower asked for a read only the leader serves, or whose tier it is still loading: it names the leader.
    NotLeader,
    /// Past READS_MAX, or a whole-tier read past HEAVY_READS on a member that serves it: `overloaded`.
    Shed,
}

/// `index` is a read-index read, `servable` whether a follower serves its query (`Family::follower_serves`), `reads`
/// and `heavies` the reads and whole-tier reads in flight. The caps hold on every member that serves the read, the
/// leader or a follower by read-index; a plain read on a follower is not shed, because it answers `NotLeader`.
/// `me`, the member's address: HEAVY_READS does not hold on a learner, only READS_MAX. It bounded the copies of a dump in flight, and a
/// learner's reads at one state share one copy (`family::dump_bytes`), while a learner never votes, so its reads
/// delay no AppendEntries reply or confirm of the group's. At HEAVY_READS, learners shed 58 to 72% of journey 9's
/// whole-tier reads at 10% CPU and 65 MB (`tests/am1/results/learner-dumps.md`).
fn gate(index: bool, leading: bool, me: SiteId, servable: bool, heavy: bool, reads: u32, heavies: u32) -> Gate {
    let learner = graftyard_types::learner_slot(me).is_some();
    if index && !leading && !servable {
        return Gate::NotLeader;
    }
    if reads >= READS_MAX || (heavy && (leading || index) && !learner && heavies >= HEAVY_READS) {
        return Gate::Shed;
    }
    Gate::Serve
}

/// `/stats?summary=1`'s form of a sample array: its count and nearest-rank p50, p99, and max, in the samples' order of
/// size, not arrival.
fn percentiles(xs: &[u32]) -> Value {
    let mut s = xs.to_vec();
    s.sort_unstable();
    let at = |p: f64| s.get(((p * s.len() as f64) as usize).min(s.len().saturating_sub(1))).copied();
    json!({ "n": s.len(), "p50": at(0.5), "p99": at(0.99), "max": s.last() })
}

/// Whether a granted read whose answer's rebuild is due in `ms` waits for it and is granted again, rather than shed:
/// only a read-index read, only if the rebuild comes before `until`, its FOLLOWER_WAIT deadline, and for a whole-tier
/// dump only on a learner. A voter's dump read is shed, so the edge's next member or a 503 answers it at once; a narrow
/// API read (`narrow`) costs a page's rows, not a dump, so a follower holds it too (`family::narrow_wait`).
fn waits_for_rebuild(learner: bool, narrow: bool, index: bool, now: u64, ms: u64, until: u64) -> bool {
    (learner || narrow) && index && now + ms < until
}

/// How long a started read waits for its grant, and the answer when it does not come: a read-index read waits
/// FOLLOWER_WAIT and answers `overloaded`, which the edge takes to the next member without `suspect`; a leader's read
/// waits three call deadlines and answers `busy`.
fn read_wait(index: bool) -> (Duration, Value) {
    if index {
        (FOLLOWER_WAIT, graftyard_lookup::overloaded_reply(OVERLOAD_RETRY_MS))
    } else {
        (raft::CALL_DEADLINE * 3, json!({ "busy": true }))
    }
}

/// One read in flight on this member, counted in `Host::reads_in_flight` (and a whole-tier one in
/// `Host::heavy_reads`) until its answer's body has been sent or dropped (`held_body`).
struct InFlight(Rc<std::cell::Cell<u32>>);

impl InFlight {
    fn new(n: &Rc<std::cell::Cell<u32>>) -> InFlight {
        n.set(n.get() + 1);
        InFlight(n.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

/// How much of an answer's body one pull of its stream yields.
const BODY_CHUNK: usize = 64 * 1024;

/// `head`, then `shared`, then `tail`, as a stream of pieces of at most BODY_CHUNK that owns `held` until the last
/// piece is taken or the stream is dropped. A whole-tier answer of 0.5 to 1.2 MB stays in the isolate while it streams
/// to a far edge, and the 500-agent smoke's apac leader reached 258 MB with no more than HEAVY_READS answers being
/// built, because built ones were not counted (`tests/am1/results/lm1-memory.md`, "The answers' bodies"). `shared` is
/// one read's `value`, shared by every read of the same query granted in the same input (`host::shared_answer`):
/// each piece is copied out of it only when taken, so reads waiting to be sent hold no copy of their own.
fn held_body<G: 'static>(
    head: Vec<u8>,
    shared: Rc<[u8]>,
    tail: Vec<u8>,
    held: G,
) -> impl futures_util::Stream<Item = std::result::Result<Vec<u8>, Error>> + 'static {
    let (mut part, mut at, mut sent) = (0, 0, 0);
    let total = head.len() + shared.len() + tail.len();
    let mut held = Some(held);
    futures_util::stream::poll_fn(move |_| {
        let next = loop {
            let p: &[u8] = match part {
                0 => &head,
                1 => &shared,
                2 => &tail,
                _ => break None,
            };
            if at >= p.len() {
                (part, at) = (part + 1, 0);
                continue;
            }
            let end = (at + BODY_CHUNK).min(p.len());
            let piece = p[at..end].to_vec();
            at = end;
            sent += piece.len();
            break Some(piece);
        };
        if sent >= total {
            held.take();
        }
        std::task::Poll::Ready(next.map(Ok))
    })
}
/// A member measures again only on a day it served a request and has not measured yet.
const DAY_MS: u64 = 86_400_000;
/// How long the daily near check's marker holds off another check: past PLACE_TRIES re-placements.
const NEAR_CHECK_FOR: u64 = 600_000;

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
    Identity(Rc<Host<family::Identity>>),
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
    /// The edge's admission for a command it classed other than Human (AM1-WP2): `Member::commit_as` takes it. Absent,
    /// the command goes through `Member::commit` as Human with no actor, as before.
    #[serde(default)]
    admit: Option<graftyard_git::group::AdmitWire>,
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
            Hosted::Identity($x) => $body,
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
            Kind::Identity => Hosted::Identity(Host::new(state, env)),
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
            each!(&h, x => if x.joined() {
                x.alarm().await;
                if let Err(e) = (Routes { host: x.clone() }).recheck().await {
                    console_error!("daily near check: {e}");
                }
            });
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

/// The site id at `k` in a request body, refused when it is missing or past `u8`, so `{"site": 258}` never names
/// site 2. `what` names the route in the error.
fn site_field(b: &Value, k: &str, what: &str) -> Result<SiteId> {
    let n = b.get(k).and_then(Value::as_u64).ok_or_else(|| err(format!("{what} needs {k}")))?;
    u8::try_from(n).map(SiteId).map_err(|_| err(format!("{what}: {k} {n} is not a site id")))
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
        #[cfg(feature = "test-faults")]
        self.host.hop.set(req.headers().get("x-graft-hop").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(2));
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
            if !observe && url.path() != "/sever" && self.host.severed() {
                crate::faults::count("severed:in");
                return Response::error("member severed (injected)", 503);
            }
            if !observe && let Some(hang) = crate::faults::site_fault(&self.host.env).await {
                crate::faults::count("site_fault:in");
                if hang > 0 {
                    Delay::from(Duration::from_millis(hang)).await;
                }
                return Response::error("member class failing (injected site fault)", 503);
            }
        }
        match (req.method(), url.path()) {
            (Method::Get, "/ping") => Response::empty(),
            (Method::Get, "/rtt") => Response::from_json(&rtt_rows(&self.host.state, &self.host.env)?),
            (Method::Post, "/raft") => {
                // B0-02's per-KB append cost: the body's arrival and this member's work, split (`Stats::raft_in`).
                // The isolate's clock moves only across I/O, so `work` is decode, step, and the output gate together.
                let t0 = now_ms();
                let body = req.bytes().await?;
                let t1 = now_ms();
                let r = self.raft(&body).await;
                self.host.stats.borrow_mut().raft_in(t0, body.len(), t1 - t0, now_ms() - t1);
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
            (Method::Post, "/place") => {
                // Test installs: `{"near_ms": MS}` bounds every region at MS, so a hosted run can make every try miss.
                #[cfg(feature = "test-faults")]
                let near = req.json::<Value>().await.ok().and_then(|b| b["near_ms"].as_u64()).map(|ms| {
                    [graftyard_types::Region::Enam, graftyard_types::Region::Weur, graftyard_types::Region::Apac]
                        .map(|r| (r, ms as u16))
                        .to_vec()
                });
                #[cfg(not(feature = "test-faults"))]
                let near = None;
                self.ready_or(|| self.place(near)).await
            }
            (Method::Post, "/move") => {
                let body: Value = req.json().await?;
                self.ready_or(|| self.move_step(body)).await
            }
            (Method::Post, "/learners") => {
                let body: Value = req.json().await?;
                self.ready_or(|| self.learners(body)).await
            }
            (Method::Get, "/status") => Response::from_json(&self.status()),
            // The committed entries `from..=to` with the family's fields (a repository's packs): the git cache
            // shard's `ChainLog::entries`.
            // Leader only, so the entries reach the commit index a `fresh` read just reported, unless `local`: then
            // any member answers from its own committed log, which may end below the leader's.
            (_, "/entries") => {
                let q = |k: &str| crate::registry::query(&url, k).ok().and_then(|v| v.parse::<u64>().ok());
                let local = q("local") == Some(1);
                let nl = self
                    .host
                    .core
                    .borrow()
                    .as_ref()
                    .filter(|c| !local && c.m.role() != Role::Leader)
                    .map(|c| c.m.not_leader());
                if let Some(nl) = nl {
                    return self.reply(json!({ "error": host::call_error_json(&CallError::NotLeader(nl)) }));
                }
                let e = self.host.entries_json(q("from").unwrap_or(1), q("to").unwrap_or(u64::MAX))?;
                self.reply(json!({ "entries": e }))
            }
            (Method::Get, "/stats") => {
                // Test installs, B0-02's A/B: `body=array` or `body=stream` sets how this member's calls send their
                // body until it changes or the object resets (`Host::array_body`).
                #[cfg(feature = "test-faults")]
                if let Ok(b) = crate::registry::query(&url, "body") {
                    self.host.array_body.set(b == "array");
                }
                self.stats(
                    crate::registry::query(&url, "reset").is_ok(),
                    crate::registry::query(&url, "summary").is_ok(),
                )
            }
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
            // LF-13: `{"for_ms": MS}` severs this member for MS ms (`Host::severed_until`); 0 lifts it. Answers
            // this member's clock, so a driver times the lift on the clock the feed's `stoppedAt` uses. F67: with
            // `"half": true` the member is half-hung instead (`Host::half_hung_until`).
            (Method::Post, "/sever") => {
                let body = req.json::<Value>().await?;
                let ms = body["for_ms"].as_u64().unwrap_or(0);
                let now = now_ms();
                let until = if ms == 0 { 0 } else { now + ms };
                let cell = if body["half"].as_bool() == Some(true) {
                    &self.host.half_hung_until
                } else {
                    &self.host.severed_until
                };
                cell.set(until);
                Response::from_json(&json!({ "now": now, "until": cell.get() }))
            }
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

    /// A granted read's answer, `{"value": <value>, "index": n, "meta": ..}`, streamed out of the shared `value` bytes
    /// while it holds its places under READS_MAX and HEAVY_READS (`held_body`).
    fn reply_read(&self, value: Rc<[u8]>, index: u64, held: (InFlight, Option<InFlight>)) -> Result<Response> {
        if self.host.fold_lost() {
            return Response::error("member reset: fold storage lost", 503);
        }
        let tail = format!(r#","index":{index},"meta":{}}}"#, self.meta()).into_bytes();
        let headers = Headers::new();
        headers.set("content-type", "application/json")?;
        Ok(Response::from_stream(held_body(br#"{"value":"#.to_vec(), value, tail, held))?.with_headers(headers))
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

    /// Steps each envelope and answers with the envelopes addressed back to the caller. A learner's call also takes
    /// what this member held for it (`crate::callers`), and a `ReadIndex` the leader could not answer at once, because
    /// it needs a round, keeps the call open up to `READ_INDEX_HOLD` for its answer: this member never calls a learner
    /// back while it calls.
    async fn raft(&self, body: &[u8]) -> Result<Response> {
        if !self.host.joined() {
            return Response::error("member not joined", 409);
        }
        let envs: Vec<raft::Envelope<F::Ev>> = raft::codec::from_bytes(body).map_err(|e| err(e.to_string()))?;
        let Some(from) = envs.first().map(|e| e.from) else { return self.raft_reply(Vec::new()) };
        self.host.callers.borrow_mut().began(from, now_ms());
        let r = self.raft_steps(from, envs).await;
        // The output gate: the reply leaves only after the writes it may rest on.
        self.host.settled().await;
        let r = match r {
            Ok(mut back) => {
                let mut out = self.host.callers.borrow_mut().take(from);
                out.append(&mut back);
                for e in &out {
                    if let raft::Message::ReadIndexRes { batch, index } = &e.msg {
                        self.host.trace(Trace::ResSent, from.0, *batch, index.map_or(0, |(i, _)| i.0), 0);
                    }
                }
                self.raft_reply(out)
            }
            Err(e) => Err(e),
        };
        self.host.callers.borrow_mut().ended(from, now_ms());
        r
    }

    async fn raft_steps(&self, from: SiteId, envs: Vec<raft::Envelope<F::Ev>>) -> Result<Vec<raft::Envelope<F::Ev>>> {
        let mut back = Vec::new();
        let mut asked = Vec::new();
        for e in envs {
            if let raft::Message::ReadIndex { batch } = &e.msg {
                self.host.trace(Trace::IndexTaken, from.0, *batch, 0, 0);
                asked.push(*batch);
            }
            self.host.trace_res(&e, false);
            let (_, out) = self.host.step(|m, t| m.step(t, e.from, e.msg))?;
            back.extend(self.host.dispatch(out, Some(from)));
        }
        let answered = |b: &[raft::Envelope<F::Ev>], batch: u64| {
            b.iter().any(|e| matches!(e.msg, raft::Message::ReadIndexRes { batch: x, .. } if x == batch))
        };
        // Only a learner's call waits: a voter's answer leaves as a call, which forms no cycle.
        if graftyard_types::learner_slot(from).is_none() {
            return Ok(back);
        }
        let end = now_ms() + READ_INDEX_HOLD.as_millis() as u64;
        for batch in asked {
            while !answered(&back, batch)
                && !self.host.callers.borrow().holds(from, |e| answered(std::slice::from_ref(e), batch))
            {
                let left = end.saturating_sub(now_ms());
                if left == 0 {
                    break;
                }
                let rx = self.host.callers.borrow_mut().wake(from);
                if !matches!(within(Duration::from_millis(left), rx).await, Some(Ok(()))) {
                    break;
                }
            }
        }
        Ok(back)
    }

    fn raft_reply(&self, back: Vec<raft::Envelope<F::Ev>>) -> Result<Response> {
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
            let (r, out) = match c.admit {
                None => {
                    let (r, out) = self.host.step(|m, t| m.commit(t, rid, c.cmd))?;
                    (r.map_err(raft::admit::AdmitError::Call), out)
                }
                Some(a) => self.host.step(|m, t| m.commit_as(t, rid, c.cmd, a.admit()))?,
            };
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
                Err(raft::admit::AdmitError::Call(e)) => json!({ "error": host::call_error_json(&e) }),
                // Refused at admission: the outcome a ticket that waited AGENT_WAIT gets, at once.
                Err(raft::admit::AdmitError::Full { retry_after_s }) => {
                    json!({ "done": { "full": { "retry_after_s": retry_after_s } } })
                }
                Err(raft::admit::AdmitError::Stage) => json!({ "error": { "stage": true } }),
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
        // A whole-tier dump past HEAVY_READS in flight on a leader is shed at once, as a 200 the lookup retries
        // without a `suspect` report (`graftyard_lookup::overloaded_reply`): memory grew with these reads in flight
        // and 100 at once reset the isolate (`tests/am1/results/lm1-memory.md`). A follower answers a plain read
        // `NotLeader`, so the caller finds the leader.
        // Any read past READS_MAX in flight is shed the same way: one quorum round grants every read waiting on it.
        // A read-index read goes to a follower only for the queries `Family::follower_serves` names; the caps hold on
        // it too (`gate`).
        let heavy = heavy_read(&q);
        let narrow = q.get("rows").and_then(|r| r.get("api")).is_some();
        let (leading, me, servable, nl) = match self.host.core.borrow().as_ref() {
            Some(c) => (
                c.m.role() == Role::Leader,
                c.m.me(),
                F::follower_serves(&c.m.group_fold().fold, &q),
                Some(c.m.not_leader()),
            ),
            None => (false, SiteId(0), false, None),
        };
        let learner = graftyard_types::learner_slot(me).is_some();
        let (reads, heavies) = (self.host.reads_in_flight.get(), self.host.heavy_reads.get());
        match gate(index, leading, me, servable, heavy, reads, heavies) {
            Gate::Serve => {}
            Gate::NotLeader => {
                self.host.stats.borrow_mut().follower_refused += 1;
                let nl = nl.ok_or_else(|| err("member not joined"))?;
                return self.reply(json!({ "error": host::call_error_json(&CallError::NotLeader(nl)) }));
            }
            Gate::Shed => {
                self.host.stats.borrow_mut().shed_reads += 1;
                return self.reply(graftyard_lookup::overloaded_reply(OVERLOAD_RETRY_MS));
            }
        }
        if index && !leading {
            self.host.stats.borrow_mut().follower_reads += 1;
        }
        let held = (InFlight::new(&self.host.reads_in_flight), heavy.then(|| InFlight::new(&self.host.heavy_reads)));
        let (wait, late) = read_wait(index);
        let until = now_ms() + wait.as_millis() as u64;
        loop {
            let (r, out) = self.host.step(|m, t| if index { m.read_index(t) } else { m.read(t) })?;
            let rx = r.as_ref().ok().map(|id| self.host.wait_read(*id, q.clone()));
            self.host.dispatch(out, None);
            let rx = match (r, rx) {
                (Ok(_), Some(rx)) => rx,
                (Err(nl), _) => {
                    return self.reply(json!({ "error": host::call_error_json(&CallError::NotLeader(nl)) }));
                }
                (Ok(_), None) => unreachable!("a started read has a waiter"),
            };
            self.host.served();
            let left = Duration::from_millis(until.saturating_sub(now_ms()));
            return match within(left, rx).await {
                // A stale dump whose rebuild is not yet due (`family::dump_wait`). A learner's read waits for the
                // rebuild and is granted again, so it answers the state at its new grant; a voter's is shed.
                Some(Ok(Ok((host::ReadBody::Shed(ms), _)))) => {
                    if waits_for_rebuild(learner, narrow, index, now_ms(), ms, until) {
                        self.host.stats.borrow_mut().waited_reads += 1;
                        worker::Delay::from(Duration::from_millis(ms)).await;
                        continue;
                    }
                    self.host.stats.borrow_mut().shed_reads += 1;
                    self.reply(graftyard_lookup::overloaded_reply(ms))
                }
                Some(Ok(Ok((host::ReadBody::Json(value), index)))) => self.reply_read(value, index.0, held),
                Some(Ok(Err(nl))) => self.reply(json!({ "error": host::call_error_json(&CallError::NotLeader(nl)) })),
                Some(Err(_)) => Response::error("member reset", 503),
                None => {
                    if index {
                        self.host.stats.borrow_mut().follower_late += 1;
                    }
                    self.reply(late)
                }
            };
        }
    }

    /// `campaign` with no body, `suspect` with `{colo, term}`. Campaigns start only here, from requests.
    fn verdict(&self, report: Option<Value>) -> Result<Response> {
        let v = match report {
            None => self.host.input(|m, t| m.campaign(t))?,
            Some(b) => {
                let colo = raft::Colo(b.get("colo").and_then(Value::as_u64).unwrap_or(0) as u32);
                let term = raft::Term(b.get("term").and_then(Value::as_u64).unwrap_or(0));
                let about = site_field(&b, "about", "suspect")?;
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
            // AM1-WP4's recorded stall, persisted in the member's state row: L0-12 reads it hosted.
            "stalledSince": s.stalled_since.map(|t| t.0),
            "boots": [s.boots[0].0, s.boots[1].0],
            "boot": c.m.boot().to_string(),
            "meta": meta(&c.m),
            "config": c.m.committed_config().map(|(i, cfg)| json!({ "index": i.0, "config": cfg })),
            "boot1": c.m.digest_of(raft::Index(1)).map(|d| d.hex()),
            "family": format!("{:?}", Kind::of(&c.join)),
            "restores": self.host.stats.borrow().restores,
            // A repository member's activity feed: held, peak held, refusals, and its ring, for `place_learners`.
            "feed": match c.join.chain {
                graftyard_types::ChainId::Repo(r) => crate::feed::FEEDS.with(|f| f.borrow().get(&r).map(|f| f.borrow().status())),
                _ => None,
            },
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

    fn stats(&self, reset: bool, summary: bool) -> Result<Response> {
        let mut s = self.host.stats.borrow_mut();
        // `?summary=1`: percentiles in place of the sample arrays, and no trace, calls, or peers. A loaded leader's raw
        // `/stats` serialized megabytes, and its wasm instance was rebuilt right after (LM-1 round 6, B0-29).
        if summary {
            let deps: serde_json::Map<String, Value> =
                s.deps_ms.iter().map(|(k, xs)| (k.clone(), percentiles(xs))).collect();
            let append: serde_json::Map<String, Value> =
                s.append_ms.iter().map(|(k, xs)| (k.to_string(), percentiles(xs))).collect();
            let v = json!({
                "site": SiteEnv::read(&self.host.env).map(|s| s.id.0).ok(),
                "role": self.host.core.borrow().as_ref().map(|c| matches!(c.m.role(), Role::Leader)),
                "summary": true,
                "commitMs": percentiles(&s.commit_ms),
                "queueMs": percentiles(&s.queue_ms),
                "roundMs": percentiles(&s.round_ms),
                "confirmMs": percentiles(&s.confirm_ms),
                "depsMs": deps,
                "appendMs": append,
                "applyMs": percentiles(&s.apply_ms),
                "depFails": s.dep_fails,
                "entries": s.entries,
                "commands": s.commands,
                "inbound": s.inbound,
                "inboundMs": now_ms().saturating_sub(s.inbound_since),
                "failedCalls": s.failed_calls,
                "shedReads": s.shed_reads,
                "waitedReads": s.waited_reads,
                "dumpBuilds": crate::family::dump_builds(),
                "testMerges": { "shard": s.test_merges, "asked": s.test_merge_asks, "shed": s.test_merge_shed },
                // Round 7's memory instruments: the isolate's wasm memory (it only grows), the leader's queued
                // commands and agent bytes, and the commands this host holds a copy of until they resolve.
                "wasmMemMb": crate::load::wasm_mem_mb(),
                "admission": self.host.core.borrow().as_ref().and_then(|c| c.m.admission()).map(|a| json!({
                    "queued": a.queued, "agentQueued": a.agent_queued, "agentBytes": a.agent_bytes, "hot": a.hot,
                    "pressure": a.pressure,
                })),
                "admitted": self.host.core.borrow().as_ref().map(|c| c.admitted.len()),
            });
            if reset {
                s.reset();
            }
            return Response::from_json(&v);
        }
        let mut v = json!({
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
            "shedReads": s.shed_reads,
            "heavyReads": self.host.heavy_reads.get(),
            "readsInFlight": self.host.reads_in_flight.get(),
            "sharedReads": s.shared_reads,
            "followerReads": s.follower_reads,
            "followerRefused": s.follower_refused,
            "followerLate": s.follower_late,
            "catchups": s.catchups,
            "leases": s.leases,
            "markerFailures": s.marker_failures,
            "truncatedTo": s.truncated_to,
            "archiveSteps": s.archive_steps,
            "sqliteFull": s.sqlite_full,
            "received": s.received,
            "alarms": s.alarms,
            "alarmRuns": s.alarm_runs,
            "alarmLast": s.alarm_last,
            "alarmArms": s.alarm_arms,
            "prsyncSent": s.prsync_sent,
            "prsyncFirst": s.prsync_first,
            "prsyncParked": s.prsync_parked,
            "peers": s.peers,
            "calls": s.calls,
            "trace": s.trace,
            "hops": s.hops,
        });
        // B0-35's instruments: the network's test-merge budget as its shard last reported it, and this leader's
        // head-push merges asked and shed since the last reset (`crate::prsync`).
        // Round 5's commit split: each entry's dep reads, by chain kind, and each command's wait to its entry's append.
        v["depsMs"] = json!(s.deps_ms);
        v["depFails"] = json!(s.dep_fails);
        v["queueMs"] = json!(s.queue_ms);
        v["testMerges"] = json!({ "shard": s.test_merges, "asked": s.test_merge_asks, "shed": s.test_merge_shed });
        // learner-dumps: a learner's reads that waited for a dump rebuild, and the dumps this isolate built.
        v["waitedReads"] = json!(s.waited_reads);
        v["raftIn"] = json!(s.raft_in);
        v["placedOverNear"] = json!(s.placed_over_near);
        #[cfg(feature = "test-faults")]
        {
            v["body"] = json!(if self.host.array_body.get() { "array" } else { "stream" });
        }
        v["dumpBuilds"] = json!(crate::family::dump_builds());
        if reset {
            s.reset();
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
                self.store_rtt(peer.site, median, now)?;
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

    fn store_rtt(&self, peer: SiteId, median_ms: u16, at: u64) -> Result<()> {
        self.host.state.storage().sql().exec(
            "INSERT OR REPLACE INTO rtt (peer, median_ms, at) VALUES (?, ?, ?)",
            vec![i64::from(peer.0).into(), i64::from(median_ms).into(), (at as i64).into()],
        )?;
        Ok(())
    }

    /// The daily near check (`design/DESIGN.md`, "Placement is checked by latency"), on the leader's alarm, which
    /// runs only while the leader serves, so it never wakes an idle group. A leader on its group's home whose rows
    /// are a day old measures the other voters again; a home over `NEAR_RTT` is noted (`near_over`), and over again
    /// at the next day's measurement it re-places the nearest voter (`place_inner`). A home back under the bound
    /// clears the note.
    async fn recheck(&self) -> Result<()> {
        let (me, cfg) = {
            let core = self.host.core.borrow();
            let Some(c) = core.as_ref().filter(|c| c.m.role() == Role::Leader) else { return Ok(()) };
            (c.m.me(), c.m.committed_config().map(|c| c.1.clone()))
        };
        let Some(raft::GroupConfig::Normal(p)) = cfg else { return Ok(()) };
        if p.home() != me || p.voters().len() < 2 {
            return Ok(());
        }
        let now = now_ms();
        let rows = rtt_rows(&self.host.state, &self.host.env)?;
        if !rows.is_empty() && rows.iter().all(|r| now.saturating_sub(r.1.at.0) < DAY_MS) {
            return Ok(());
        }
        // One check at a time: the alarm fires again while this one measures. The marker is read and written with no
        // await between, and a check that died leaves it to expire.
        let sql = self.host.state.storage().sql();
        let busy: Option<u64> = host::rows(&sql, "SELECT v FROM me WHERE k = 'near_check'", vec![])?
            .first()
            .and_then(|r| host::text(&r[0]).ok().and_then(|t| t.parse().ok()));
        if busy.is_some_and(|t| now.saturating_sub(t) < NEAR_CHECK_FOR) {
            return Ok(());
        }
        sql.exec("INSERT OR REPLACE INTO me (k, v) VALUES ('near_check', ?)", vec![now.to_string().into()])?;
        let site = SiteEnv::read(&self.host.env)?;
        for v in p.voters().iter().filter(|v| v.site != me) {
            let ms = self.measure(&site, v).await;
            if ms.is_err() {
                sql.exec("DELETE FROM me WHERE k = 'near_check'", None)?;
            }
            self.store_rtt(v.site, ms?, now)?;
        }
        let rows = rtt_rows(&self.host.state, &self.host.env)?;
        let checks = graftyard_types::PlacementChecks {
            sites: &site.sites,
            separate_accounts: false,
            rtt: &rows,
            near: graftyard_types::NEAR_RTT,
        };
        let over = checks.check_near(p.voters(), me).is_err();
        let since: Option<u64> = host::rows(&sql, "SELECT v FROM me WHERE k = 'near_over'", vec![])?
            .first()
            .and_then(|r| host::text(&r[0]).ok().and_then(|t| t.parse().ok()));
        match (over, since) {
            (false, _) => {
                sql.exec("DELETE FROM me WHERE k = 'near_over'", None)?;
            }
            (true, None) => {
                sql.exec("INSERT OR REPLACE INTO me (k, v) VALUES ('near_over', ?)", vec![now.to_string().into()])?;
            }
            (true, Some(t)) if now.saturating_sub(t) >= DAY_MS => {
                sql.exec("DELETE FROM me WHERE k = 'near_over'", None)?;
                let r = self.place_inner(graftyard_types::NEAR_RTT).await?;
                console_log!("daily near check re-placed: {r:?}");
            }
            (true, Some(_)) => {}
        }
        sql.exec("DELETE FROM me WHERE k = 'near_check'", None)?;
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
        let to = site_field(&body, "to", "transfer")?;
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
        let to = site_field(&body, "site", "replace")?;
        // Test fault: the first new member's latency check fails, so the retry with the next generation runs.
        #[cfg(feature = "test-faults")]
        let fail_first = body.get("fail_first").and_then(Value::as_bool).unwrap_or(false);
        #[cfg(not(feature = "test-faults"))]
        let fail_first = false;
        match self.replace_on(to, &[], 3, fail_first, None).await? {
            Ok(v) | Err(v) => self.reply(v),
        }
    }

    /// The re-placement loop `/replace` and `/place` share: `Degraded { lost: to }`, then new members on `to` at
    /// successive generations, each joined and checked by `Placement::new` with the near bound `near`, up to `tries`;
    /// the first that passes goes in by `Normal`. `Err` when none passed: the config goes back to `Normal` with the
    /// old member, so a group that could not be placed keeps serving where it was, and the answer lists every try with
    /// its round trips. A refused member never joins the group; with `test-faults` it is also cleared. With
    /// `keep_below` (the near round trip of the member being replaced, from `/place`), a try refused only by the near
    /// bound is a candidate: when none passed, the nearest goes in if it is nearer than `keep_below`, and otherwise the
    /// old member stays with an `Ok` answer, so the bound alone never fails a create.
    async fn replace_on(
        &self,
        to: SiteId,
        near: &[(graftyard_types::Region, u16)],
        tries: usize,
        fail_first: bool,
        keep_below: Option<u16>,
    ) -> Result<std::result::Result<Value, Value>> {
        let (chain, record, me, cfg, c_owner, c_genesis) = {
            let core = self.host.core.borrow();
            let c = core.as_ref().ok_or_else(|| err("member not joined"))?;
            if c.m.role() != Role::Leader {
                return Ok(Err(json!({ "error": { "notLeader": host::nl_json(&c.m.not_leader()) } })));
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
            return Ok(Err(json!({ "error": "the committed config is not Normal" })));
        };
        if to == me {
            return Ok(Err(json!({ "error": "transfer first: the leader's own site" })));
        }
        let old = p.voters().iter().find(|m| m.site == to).cloned().ok_or_else(|| err("no member on that site"))?;
        let t0 = now_ms();
        if let Err(e) = self.change(raft::GroupConfig::Degraded { placement: p.clone(), lost: to }).await {
            return Ok(Err(json!({ "error": format!("degraded: {e}") })));
        }
        let site = SiteEnv::read(&self.host.env)?;
        let mine = rtt_rows(&self.host.state, &self.host.env)?;
        let others: Vec<MemberRef> = p.voters().iter().filter(|m| m.site != to).cloned().collect();
        let mut generation = p.next_generation();
        let mut tried = Vec::new();
        // Tries that passed every check but the near bound, kept until the loop ends: with `keep_below`, the nearest
        // goes in when none passed (`graftyard_types::nearest_try`).
        let mut over: Vec<(MemberRef, graftyard_types::Placement, u16, Option<Rtt>)> = Vec::new();
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
            // This member's row for `to` measured the member being replaced: measure the new one instead, so the
            // check, and the daily recheck after it, read the pair as it now is.
            let fresh = if status == 200 {
                let ms = self.measure(&site, &new).await?;
                vec![(me, Rtt { peer: to, median_ms: ms, at: Timestamp(now_ms()) })]
            } else {
                Vec::new()
            };
            let all: Vec<(SiteId, Rtt)> =
                mine.iter().filter(|(_, r)| r.peer != to).chain(&fresh).chain(&theirs).copied().collect();
            let checks =
                graftyard_types::PlacementChecks { sites: &site.sites, separate_accounts: false, rtt: &all, near };
            let r = if fail_first && tried.is_empty() {
                Err("injected: latency check failed".to_owned())
            } else if status == 200 {
                match graftyard_types::Placement::new(voters.clone(), arrayvec::ArrayVec::new(), p.home(), &checks) {
                    Err(e @ graftyard_types::PlacementError::NoNearFollower { ms, .. }) if keep_below.is_some() => {
                        let unbounded = graftyard_types::PlacementChecks { near: &[], ..checks };
                        if let Ok(np) =
                            graftyard_types::Placement::new(voters, arrayvec::ArrayVec::new(), p.home(), &unbounded)
                        {
                            over.push((new.clone(), np, ms, fresh.first().map(|f| f.1)));
                        }
                        Err(e.to_string())
                    }
                    r => r.map_err(|e| e.to_string()),
                }
            } else {
                Err(format!("join answered {status}: {rows}"))
            };
            let ms: Vec<(u8, u16)> = theirs.iter().map(|(_, r)| (r.peer.0, r.median_ms)).collect();
            tried.push(json!({ "member": new.do_name, "rtt": ms, "result": r.as_ref().err() }));
            match r {
                Ok(np) => {
                    for (_, r) in &fresh {
                        self.store_rtt(r.peer, r.median_ms, r.at.0)?;
                    }
                    break Some((np, None));
                }
                Err(_) => {
                    // A refused try is cleared at once, unless it is kept as a candidate for the nearest.
                    #[cfg(feature = "test-faults")]
                    if !over.last().is_some_and(|o| o.0 == new) {
                        let _ = self.post_member(&new, "/retire", &json!({})).await;
                    }
                    if tried.len() >= tries {
                        // No try met the bound: the nearest goes in if it beats the member it replaces, so a create
                        // never fails on the bound alone (`design/DESIGN.md`, "Placement is checked by latency").
                        let pick = keep_below.and_then(|c| {
                            graftyard_types::nearest_try(c, &over.iter().map(|o| o.2).collect::<Vec<_>>())
                        });
                        let best = pick.map(|i| over.swap_remove(i));
                        #[cfg(feature = "test-faults")]
                        for (m, ..) in &over {
                            let _ = self.post_member(m, "/retire", &json!({})).await;
                        }
                        if let Some((_, _, _, Some(r))) = &best {
                            self.store_rtt(r.peer, r.median_ms, r.at.0)?;
                        }
                        break best.map(|(_, np, ms, _)| (np, Some(ms)));
                    }
                    generation += 1;
                }
            }
        };
        let Some((np, near_ms)) = placed else {
            // The old member comes back: it kept its log, and the leader backs its next index up from its answers.
            let back = self.change(raft::GroupConfig::Normal(p.clone())).await;
            if keep_below.is_some() {
                // The bound is a goal: the member already placed stays, nearer than every try.
                self.host.stats.borrow_mut().placed_over_near += 1;
                let kept = json!({ "kept": old.do_name, "tried": tried, "restored": back.as_ref().err() });
                console_log!("placement of {chain}: no try nearer than the current member: {kept}");
                return Ok(match back {
                    Ok(_) => Ok(kept),
                    Err(e) => Err(json!({ "error": format!("restore: {e}"), "tried": tried })),
                });
            }
            let e = json!({ "error": "no placement passed", "tried": tried, "restored": back.as_ref().err() });
            console_error!("placement of {chain} failed: {e}");
            return Ok(Err(e));
        };
        if let Some(ms) = near_ms {
            self.host.stats.borrow_mut().placed_over_near += 1;
            console_log!("placement of {chain}: no try within NEAR_RTT; kept the nearest, {ms} ms");
        }
        let index = match self.change(raft::GroupConfig::Normal(Box::new(np))).await {
            Ok(i) => i,
            Err(e) => return Ok(Err(json!({ "error": format!("normal: {e}"), "tried": tried }))),
        };
        let mut v = json!({ "old": old.do_name, "tried": tried, "configIndex": index, "ms": now_ms() - t0 });
        if let Some(ms) = near_ms {
            v["nearOver"] = json!(ms);
        }
        Ok(Ok(v))
    }

    /// The near check on the leader (`design/DESIGN.md`, Shape, `Placement` and `NEAR_RTT`): reads the home's round
    /// trips to the other voters (this member's own when it is the home) and, when the home's nearest voter is over
    /// `NEAR_RTT` for the home's region, re-places that voter with new generations through `replace_on`, up to
    /// `PLACE_TRIES`. `{"placed": false}` when the group already passes; when no try passed, the nearest try
    /// (`nearOver`), or `{"kept"}` with the member it started with when that was nearer, with each try's round trips. `graftyard_lookup::create` calls it once a new group leads, and the daily recheck when
    /// a home was over the bound twice a day apart.
    async fn place(&self, near: Option<Vec<(graftyard_types::Region, u16)>>) -> Result<Response> {
        match self.place_inner(near.as_deref().unwrap_or(graftyard_types::NEAR_RTT)).await? {
            Ok(v) | Err(v) => self.reply(v),
        }
    }

    async fn place_inner(&self, near: &[(graftyard_types::Region, u16)]) -> Result<std::result::Result<Value, Value>> {
        let (me, cfg) = {
            let core = self.host.core.borrow();
            let c = core.as_ref().ok_or_else(|| err("member not joined"))?;
            if c.m.role() != Role::Leader {
                return Ok(Err(json!({ "error": { "notLeader": host::nl_json(&c.m.not_leader()) } })));
            }
            (c.m.me(), c.m.committed_config().map(|c| c.1.clone()))
        };
        let Some(raft::GroupConfig::Normal(p)) = cfg else {
            return Ok(Err(json!({ "error": "the committed config is not Normal" })));
        };
        let site = SiteEnv::read(&self.host.env)?;
        let home = p.home();
        let rtt = if home == me {
            rtt_rows(&self.host.state, &self.host.env)?
        } else {
            let h = p.voters().iter().find(|m| m.site == home).ok_or_else(|| err("no member on the home"))?;
            site.member(&self.host.env, home, &h.do_name)?.fetch_with_str("https://member/rtt").await?.json().await?
        };
        let checks = graftyard_types::PlacementChecks { sites: &site.sites, separate_accounts: false, rtt: &rtt, near };
        match checks.check_near(p.voters(), home) {
            Ok(()) => Ok(Ok(
                json!({ "placed": false, "rtt": rtt.iter().map(|(_, r)| (r.peer.0, r.median_ms)).collect::<Vec<_>>() }),
            )),
            Err(graftyard_types::PlacementError::NoNearFollower { nearest, ms, bound_ms, .. }) => {
                let mut r = self.replace_on(nearest, near, graftyard_types::PLACE_TRIES, false, Some(ms)).await?;
                let over = json!({ "site": nearest.0, "ms": ms, "boundMs": bound_ms });
                match &mut r {
                    Ok(v) | Err(v) => v["over"] = over,
                }
                Ok(r)
            }
            Err(e) => Ok(Err(json!({ "error": e.to_string() }))),
        }
    }

    /// One step of the group's move to `{"to": PlacementRecord}` (`graftyard_identity::group_move`), on the leader:
    /// a leadership transfer, a one-site `Joint` with a new member on the site it adds (joined first, at the next
    /// generation, measuring the config's other voters), its `Normal`, or the target's home. Answers the step and,
    /// once the step committed, the config; `{"step": "Done"}` when the group is where `to` places it. The caller
    /// asks again after each step, so a step lost with its leader is planned again from the next leader's config.
    /// The new member's placement is checked by `Placement::new` against the joined members' round trips.
    async fn move_step(&self, body: Value) -> Result<Response> {
        use graftyard_identity::group_move::{MoveStep, next_step};
        let to: PlacementRecord = serde_json::from_value(body["to"].clone())?;
        let (chain, record, me, cfg, c_owner, c_genesis) = {
            let core = self.host.core.borrow();
            let c = core.as_ref().ok_or_else(|| err("member not joined"))?;
            if c.m.role() != Role::Leader {
                return self.reply(json!({ "error": { "notLeader": host::nl_json(&c.m.not_leader()) } }));
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
        let Some(cfg) = cfg else { return self.reply(json!({ "error": "no committed config" })) };
        let step = next_step(&cfg, me, &to);
        let label = format!("{step:?}");
        let t0 = now_ms();
        let site = SiteEnv::read(&self.host.env)?;
        let checked = |voters: arrayvec::ArrayVec<MemberRef, 3>, home: SiteId, rtt: &[(SiteId, Rtt)]| {
            let checks =
                graftyard_types::PlacementChecks { sites: &site.sites, separate_accounts: false, rtt, near: &[] };
            graftyard_types::Placement::new(voters, arrayvec::ArrayVec::new(), home, &checks).map_err(|e| e.to_string())
        };
        let mut rtt = rtt_rows(&self.host.state, &self.host.env)?;
        let voters_of =
            |p: &graftyard_types::Placement| p.voters().iter().cloned().collect::<arrayvec::ArrayVec<_, 3>>();
        let r: std::result::Result<Value, String> = match (step, &cfg) {
            (MoveStep::Done, _) => Ok(json!({ "step": "Done" })),
            (MoveStep::Degraded, _) => Err("the config is Degraded: replace the lost member first".into()),
            (MoveStep::Transfer { to }, _) => {
                let r = self.host.input(|m, t| m.transfer(t, to))?;
                r.map(|id| json!({ "transfer": id.0.to_string() })).map_err(|e| format!("{e:?}"))
            }
            (MoveStep::Swap { add, drop }, raft::GroupConfig::Normal(p)) => {
                let old = p.voters().iter().find(|m| m.site == drop).cloned().ok_or_else(|| err("no member there"))?;
                let new = MemberRef::new(chain, add, old.kind, old.slot, p.next_generation());
                let peers: Vec<MemberRef> = p.voters().iter().filter(|m| m.site != drop).cloned().collect();
                let join = Join {
                    chain,
                    me: new.clone(),
                    record: record.clone(),
                    peers,
                    owner: c_owner,
                    genesis: c_genesis.clone(),
                    #[cfg(feature = "test-faults")]
                    no_claim: false,
                };
                let (status, rows) = self.post_member(&new, "/join", &serde_json::to_value(&join)?).await?;
                if status != 200 {
                    return self.reply(json!({ "step": label, "error": format!("join answered {status}: {rows}") }));
                }
                rtt.extend(serde_json::from_value::<Vec<(SiteId, Rtt)>>(rows).unwrap_or_default());
                for v in p.voters().iter().filter(|v| v.site != me && v.site != drop) {
                    let stub = site.member(&self.host.env, v.site, &v.do_name)?;
                    rtt.extend(stub.fetch_with_str("https://member/rtt").await?.json::<Vec<(SiteId, Rtt)>>().await?);
                }
                let voters: arrayvec::ArrayVec<MemberRef, 3> =
                    voters_of(p).into_iter().map(|m| if m.site == drop { new.clone() } else { m }).collect();
                // The home stays unless it leaves; then the target's home if the new half holds it as a Full voter,
                // else the new member's site, until `Home` moves it.
                let home = if p.home() != drop {
                    p.home()
                } else if voters
                    .iter()
                    .any(|m: &MemberRef| m.site == to.home && m.kind == graftyard_types::MemberKind::Full)
                {
                    to.home
                } else {
                    add
                };
                match checked(voters, home, &rtt) {
                    Ok(np) => self
                        .change(raft::GroupConfig::Joint { old: p.clone(), new: Box::new(np) })
                        .await
                        .map(|i| json!({ "configIndex": i, "joined": new.do_name })),
                    Err(e) => Err(e),
                }
            }
            (MoveStep::Leave, raft::GroupConfig::Joint { new, .. }) => {
                self.change(raft::GroupConfig::Normal(new.clone())).await.map(|i| json!({ "configIndex": i }))
            }
            (MoveStep::Home { home }, raft::GroupConfig::Normal(p)) => match checked(voters_of(p), home, &rtt) {
                Ok(np) => {
                    self.change(raft::GroupConfig::Normal(Box::new(np))).await.map(|i| json!({ "configIndex": i }))
                }
                // A pair the leader never measured (a member joined from another leader): the rows are each voter's
                // own, so read them.
                Err(_) => {
                    for v in p.voters().iter().filter(|v| v.site != me) {
                        let stub = site.member(&self.host.env, v.site, &v.do_name)?;
                        rtt.extend(
                            stub.fetch_with_str("https://member/rtt").await?.json::<Vec<(SiteId, Rtt)>>().await?,
                        );
                    }
                    match checked(voters_of(p), home, &rtt) {
                        Ok(np) => self
                            .change(raft::GroupConfig::Normal(Box::new(np)))
                            .await
                            .map(|i| json!({ "configIndex": i })),
                        Err(e) => Err(e),
                    }
                }
            },
            (s, c) => Err(format!("{s:?} does not apply to {c:?}")),
        };
        let config = self.host.core.borrow().as_ref().and_then(|c| c.m.committed_config().map(|c| json!(c)));
        match r {
            Ok(v) => self.reply(json!({ "step": label, "ok": v, "config": config, "ms": now_ms() - t0 })),
            Err(e) => self.reply(json!({ "step": label, "error": e, "config": config, "ms": now_ms() - t0 })),
        }
    }

    /// The leader's half of learner placement (`design/DESIGN.md`, Shape, `place_learners`): `{"load": GroupLoad}`
    /// names each region's peak read rate. `place_learners` picks the regions; each learner goes to the first
    /// non-spare site in its region, in slot order, under the next generation, joins, and one config change replaces
    /// the group's learners with them (none when the load is under `LEARNER_AT`). The design runs this from a daily
    /// `graft-ops` cron over Analytics Engine counters; a test install has neither, so its driver posts the load here.
    async fn learners(&self, body: Value) -> Result<Response> {
        let load: graftyard_types::GroupLoad = serde_json::from_value(body["load"].clone())?;
        let mut regions = graftyard_types::place_learners(&load);
        // The activity feed's half (`design/AGENT-LAYER.md`, "Activity feed"): with `{"feed": true}`, each voter's
        // reported peak held polls and refusals (`/status` `feed`) add learners until held plus refused fit in half
        // of FEED_HELD_MAX each (`graftyard_git::feed::learners_for_feed`), in the busiest read regions first, then
        // the voters' own regions.
        let mut feed = Vec::new();
        if body["feed"].as_bool() == Some(true) {
            let voters = {
                let core = self.host.core.borrow();
                let c = core.as_ref().ok_or_else(|| err("member not joined"))?;
                c.m.committed_config().map(|c| match &c.1 {
                    raft::GroupConfig::Normal(p) => p.voters().to_vec(),
                    _ => Vec::new(),
                })
            };
            let site = SiteEnv::read(&self.host.env)?;
            let mut reports = Vec::new();
            for v in voters.unwrap_or_default() {
                let stub = site.member(&self.host.env, v.site, &v.do_name)?;
                let Ok(mut r) = stub.fetch_with_str("https://member/status").await else { continue };
                let Ok(st) = r.json::<Value>().await else { continue };
                let peak = st.pointer("/feed/peak").and_then(Value::as_u64).unwrap_or(0) as u32;
                let refused = st.pointer("/feed/refused").and_then(Value::as_u64).unwrap_or(0) as u32;
                feed.push(json!({ "site": v.site.0, "peak": peak, "refused": refused }));
                reports.push((peak, refused));
            }
            let want = graftyard_git::feed::learners_for_feed(&reports);
            let order: Vec<graftyard_types::Region> = load
                .reads
                .iter()
                .map(|(g, _)| *g)
                .chain(site.sites.0.iter().filter(|s| !s.spare).map(|s| s.region))
                .collect();
            let mut k = 0;
            while regions.len() < want && !regions.is_full() && !order.is_empty() {
                regions.push(order[k % order.len()]);
                k += 1;
            }
        }
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
        let checks =
            graftyard_types::PlacementChecks { sites: &site.sites, separate_accounts: false, rtt: &rtt, near: &[] };
        let voters: arrayvec::ArrayVec<MemberRef, 3> = p.voters().iter().cloned().collect();
        let np = match graftyard_types::Placement::new(voters, learners, p.home(), &checks) {
            Ok(np) => np,
            Err(e) => return self.reply(json!({ "error": e.to_string(), "joined": joined })),
        };
        match self.change(raft::GroupConfig::Normal(Box::new(np))).await {
            Ok(i) => self.reply(json!({ "regions": regions, "learners": joined, "configIndex": i, "feed": feed })),
            Err(e) => self.reply(json!({ "error": e, "learners": joined })),
        }
    }

    /// B0-18's reads, generated inside this member as Milestone 0's learners did (`tests/m0/run/raft-learners.ts`):
    /// `rate` read-index reads a second for `secs`, each `{"q": {"ranges": true}}`, and each one's latency, start to
    /// answer. The leader's load is real; this member's own request limit is not exercised.
    #[cfg(feature = "test-faults")]
    async fn read_bench(&self, body: Value) -> Result<Response> {
        let rate = body["rate"].as_u64().unwrap_or(1_000).max(1);
        // `{"samples": true}`: every read's start offset, latency, and end (0 ok, 1 not leader, 2 dropped, 3 timeout).
        let samples = body["samples"].as_bool().unwrap_or(false);
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
        let mut each = Vec::new();
        for done in waits {
            let (end, at, ms) = done.await.unwrap_or(("dropped", t0, 0));
            if samples {
                let code = ["ok", "notLeader", "dropped", "timeout"].iter().position(|e| *e == end).unwrap_or(2);
                each.push([at.saturating_sub(t0), ms, code as u64]);
            }
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
            "samples": samples.then_some(each),
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

#[cfg(test)]
mod tests {
    use graftyard_types::SiteId;
    use serde_json::json;

    use super::{Gate, HEAVY_READS, InFlight, READS_MAX, gate, heavy_read, percentiles, read_wait, site_field};

    /// The summary's p50 and p99 are nearest-rank over the sorted samples, the same rule as `a1.mjs`'s `pct`, so a
    /// row judged from `?summary=1` matches one judged from the raw arrays; samples arrive out of order.
    #[test]
    fn a_summary_ranks_the_sorted_samples() {
        let xs: Vec<u32> = (1..=200).rev().collect();
        assert_eq!(percentiles(&xs), json!({ "n": 200, "p50": 101, "p99": 199, "max": 200 }));
        assert_eq!(percentiles(&[]), json!({ "n": 0, "p50": null, "p99": null, "max": null }));
    }

    /// A voter's address and a learner's, for `gate`.
    const V: SiteId = SiteId(1);
    const L: SiteId = SiteId(graftyard_types::LEARNER_ADDR + 3);

    /// Every read counts against READS_MAX, on any member; whole-tier reads on a leader also count against the lower
    /// HEAVY_READS; and the counts come back down when a read is answered or dropped, so a burst of timed-out callers
    /// does not leave the member shedding for good.
    #[test]
    fn every_read_is_shed_past_reads_max_and_whole_tier_ones_past_the_heavy_cap() {
        assert!(heavy_read(&json!({ "rows": true, "ranges": true, "packed": true })));
        for narrow in [json!({ "rows": { "access": true } }), json!({ "rows": { "merge": 7 } }), json!({ "dep": true })]
        {
            assert!(!heavy_read(&narrow), "{narrow}");
        }
        // (index, leading, servable, heavy, reads, heavies)
        assert_eq!(gate(false, true, V, false, true, HEAVY_READS, HEAVY_READS - 1), Gate::Serve);
        assert_eq!(gate(false, true, V, false, true, HEAVY_READS, HEAVY_READS), Gate::Shed);
        assert_eq!(
            gate(false, false, V, true, true, HEAVY_READS, HEAVY_READS),
            Gate::Serve,
            "a plain follower read answers NotLeader"
        );
        assert_eq!(gate(false, true, V, false, false, HEAVY_READS * 4, HEAVY_READS * 4), Gate::Serve);
        assert_eq!(gate(false, true, V, false, false, READS_MAX - 1, 0), Gate::Serve);
        assert_eq!(gate(false, true, V, false, false, READS_MAX, 0), Gate::Shed, "a narrow read past READS_MAX");
        assert_eq!(gate(false, false, V, false, false, READS_MAX, 0), Gate::Shed, "on a follower too");
        let n = std::rc::Rc::new(std::cell::Cell::new(0));
        let held: Vec<_> = (0..READS_MAX).map(|_| InFlight::new(&n)).collect();
        assert_eq!(gate(false, true, V, false, false, n.get(), 0), Gate::Shed);
        drop(held);
        assert_eq!(n.get(), 0);
    }

    /// A follower serves a read-index read only for the queries it may serve, under the same caps as the leader; a
    /// read it may not serve (a leader-only query, or tiers it is still loading) names the leader, before any cap, so
    /// the edge goes to the leader and does not retry the follower. One that waited FOLLOWER_WAIT without applying the
    /// leader's index answers `overloaded`, never a value and never a 5xx the edge would report with `suspect`.
    #[test]
    fn a_follower_serves_only_its_reads_under_the_caps_and_never_stale() {
        assert_eq!(gate(true, false, V, true, true, 0, 0), Gate::Serve);
        assert_eq!(gate(true, false, V, false, false, 0, 0), Gate::NotLeader);
        assert_eq!(gate(true, false, V, false, true, READS_MAX, HEAVY_READS), Gate::NotLeader);
        assert_eq!(gate(true, false, V, true, true, 0, HEAVY_READS), Gate::Shed, "HEAVY_READS on a follower");
        assert_eq!(gate(true, false, V, true, false, READS_MAX, 0), Gate::Shed, "READS_MAX on a follower");
        assert_eq!(gate(true, true, V, false, false, 0, 0), Gate::Serve, "a leader serves every read-index read");
        let (wait, late) = read_wait(true);
        assert_eq!(wait, super::FOLLOWER_WAIT);
        assert!(late.get("overloaded").is_some() && late.get("value").is_none(), "{late}");
        let (wait, late) = read_wait(false);
        assert!(wait > super::FOLLOWER_WAIT);
        assert_eq!(late, json!({ "busy": true }));
    }

    /// A learner serves whole-tier reads past HEAVY_READS, because its reads at one state share one dump and it never
    /// votes; READS_MAX still holds on it, and a follower and the leader keep HEAVY_READS. A learner asked for a read
    /// it may not serve still names the leader. Only a learner's read-index read waits for a dump rebuild, and only if
    /// the rebuild comes before its deadline.
    #[test]
    fn a_learner_serves_whole_tier_reads_past_the_heavy_cap_and_waits_for_a_due_rebuild() {
        use super::waits_for_rebuild;
        // (index, leading, me, servable, heavy, reads, heavies): V a voter's address, L a learner's
        assert_eq!(gate(true, false, L, true, true, 0, HEAVY_READS), Gate::Serve, "a learner past HEAVY_READS");
        assert_eq!(gate(true, false, L, true, true, 0, HEAVY_READS * 8), Gate::Serve);
        assert_eq!(gate(true, false, L, true, true, READS_MAX, 0), Gate::Shed, "READS_MAX on a learner");
        assert_eq!(gate(true, false, L, true, false, READS_MAX, 0), Gate::Shed);
        assert_eq!(gate(true, false, V, true, true, 0, HEAVY_READS), Gate::Shed, "a follower keeps HEAVY_READS");
        assert_eq!(gate(false, true, V, false, true, 0, HEAVY_READS), Gate::Shed, "the leader keeps HEAVY_READS");
        assert_eq!(gate(true, false, L, false, true, 0, 0), Gate::NotLeader, "a learner's leader-only read");
        assert_eq!(gate(false, false, L, true, true, 0, HEAVY_READS), Gate::Serve, "a plain read answers NotLeader");
        // (learner, index, now, ms until the rebuild, deadline)
        assert!(waits_for_rebuild(true, false, true, 1_000, 499, 1_500));
        assert!(!waits_for_rebuild(true, false, true, 1_000, 500, 1_500), "a rebuild at the deadline is too late");
        assert!(!waits_for_rebuild(false, false, true, 1_000, 10, 1_500), "a follower sheds a dump read");
        assert!(!waits_for_rebuild(true, false, false, 1_000, 10, 1_500), "a plain read never waits");
        assert!(waits_for_rebuild(false, true, true, 1_000, 10, 1_500), "a follower holds a narrow API read");
        assert!(!waits_for_rebuild(false, true, true, 1_000, 500, 1_500), "but not past its deadline");
        assert!(!waits_for_rebuild(false, true, false, 1_000, 10, 1_500), "nor a leader's plain read");
    }

    /// An answer's body keeps its read counted until its last piece is taken, so answers still streaming to a far
    /// edge count against HEAVY_READS; the pieces are the body in order; a body dropped half sent releases it too.
    #[test]
    fn a_streamed_answer_holds_its_read_until_sent() {
        use futures_util::StreamExt;
        let n = std::rc::Rc::new(std::cell::Cell::new(0));
        let body: Vec<u8> = (0..super::BODY_CHUNK * 2 + 10).map(|i| (i % 251) as u8).collect();
        let (head, tail) = (b"{\"value\":".to_vec(), b",\"index\":3}".to_vec());
        let shared: std::rc::Rc<[u8]> = body.clone().into();
        let mut s = std::pin::pin!(super::held_body(head.clone(), shared.clone(), tail.clone(), InFlight::new(&n)));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let whole = [head.as_slice(), &body, &tail].concat();
        let mut got = Vec::new();
        while let std::task::Poll::Ready(Some(Ok(piece))) = s.poll_next_unpin(&mut cx) {
            assert!(!piece.is_empty() && piece.len() <= super::BODY_CHUNK);
            got.extend(piece);
            let left = if got.len() < whole.len() { 1 } else { 0 };
            assert_eq!(n.get(), left, "counted until the last piece is taken");
        }
        assert_eq!(got, whole);
        let mut cut = Box::pin(super::held_body(head, shared, tail, InFlight::new(&n)));
        let _ = cut.poll_next_unpin(&mut cx);
        assert_eq!(n.get(), 1);
        drop(cut);
        assert_eq!(n.get(), 0);
    }

    /// A site id past `u8` is refused, never truncated onto another site.
    #[test]
    fn a_site_id_past_u8_is_refused() {
        assert_eq!(site_field(&json!({ "site": 2 }), "site", "replace").expect("in range"), SiteId(2));
        assert_eq!(site_field(&json!({ "site": 255 }), "site", "replace").expect("in range"), SiteId(255));
        let e = site_field(&json!({ "site": 258 }), "site", "replace").expect_err("out of range").to_string();
        assert!(e.contains("258 is not a site id"), "{e}");
        let e = site_field(&json!({}), "to", "transfer").expect_err("missing").to_string();
        assert!(e.contains("transfer needs to"), "{e}");
    }
}
