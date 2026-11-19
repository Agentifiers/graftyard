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
/// BF-14 arrivals kept in memory (`Host::arrivals_mem`): a one-minute B0-02 run at 330/s, twice over.
#[cfg(feature = "test-faults")]
const ARRIVALS_MEM_MAX: usize = 40_000;
/// Reads of any kind a member serves at once, counted from arrival until the answer's last piece is taken
/// [guess]: on B0-29 the apac leader, granted about 600 reads in one input, ran out of memory one second later, and
/// probes of 1,000 or more narrow reads at once moved an idle group's leader on `CommitErrors`
/// (`tests/am1/results/lm1-memory.md`, "Rows"). The leader takes about 300 req/s at journey 9, mostly reads of
/// 1 s or less, so 256 leaves room for a second's worth.
pub const READS_MAX: u32 = 256;
/// Distinct bytes of read answers a voter or learner streams at once, a shared answer counted once
/// (`host::Answers`) [guess]: past it any new read is shed `overloaded`. A new leader had 22.9 MB of answers in flight
/// one second after it won and reset 18 s later, and B0-33's leaders 26 to 30.5 MB, under the isolate's 128 MiB
/// (`tests/m0/results/memory-budgets.md`).
pub const ANSWER_BYTES_MAX: usize = 16_000_000;

/// A `/read`'s kind for `Stats::reads_by`: `rows` for a whole-tier dump, `rows.<selector>` for a narrow rows read
/// (`rows.api`, `rows.access`, `rows.merge`, ...), whatever else the query carries (`packed`, `at`), and otherwise the
/// query's first key (`refs`, `ranges`, `key`, ...).
fn read_kind(q: &Value) -> String {
    let first = |v: &Value| v.as_object().and_then(|o| o.keys().next().cloned());
    match q.get("rows") {
        Some(r) => first(r).map_or_else(|| "rows".into(), |sub| format!("rows.{sub}")),
        None => first(q).unwrap_or_else(|| "none".into()),
    }
}

/// A whole-tier read: `{"rows": true}`, not a narrow `{"rows": {"access" | "agent" | "merge": ..}}` one.
fn heavy_read(q: &Value) -> bool {
    q.get("rows").is_some_and(Value::is_boolean)
}

/// The start of an answer that missed a metadata child's rows (`crate::split::since`).
const MISSED_CHILD: &[u8] = br#"{"missedChild""#;

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
/// `answers`, the distinct answer bytes streaming, sheds any read past ANSWER_BYTES_MAX.
/// `off_leader`: the query is one the acting leader never serves by read-index (`graftyard_lookup::leader_never_serves`,
/// a whole-tier dump or an `ls-refs`) and the group has another member to serve it. A leader answers such a read
/// `NotLeader`, naming itself, whichever isolate sent it (`decisions/DECISIONS.md`, "`tier-off-leader`").
#[allow(clippy::too_many_arguments)]
fn gate(
    index: bool,
    leading: bool,
    me: SiteId,
    servable: bool,
    heavy: bool,
    reads: u32,
    heavies: u32,
    answers: usize,
    off_leader: bool,
) -> Gate {
    let learner = graftyard_types::learner_slot(me).is_some();
    if index && (if leading { off_leader } else { !servable }) {
        return Gate::NotLeader;
    }
    if reads >= READS_MAX
        || answers >= ANSWER_BYTES_MAX
        || (heavy && (leading || index) && !learner && heavies >= HEAVY_READS)
    {
        return Gate::Shed;
    }
    Gate::Serve
}

/// A read's `NotLeader` answer. On a follower or learner's read-index read that failed naming no leader while it forgot
/// one (`raft::Member::forgot`, F67 rule 12), the read is shed, `overloaded` as a behind member's is, with `forgot`
/// naming that member and its term, so the edge reports it with `suspect` and never sends the read to the leader
/// (`decisions/DECISIONS.md`, "`b032-shed`"). Any other is `notLeader`, as before.
fn not_leader_reply(index: bool, nl: &raft::NotLeader, forgot: Option<SiteId>) -> Value {
    match forgot {
        Some(f) if index && nl.leader.is_none() => {
            let mut v = graftyard_lookup::overloaded_reply(OVERLOAD_RETRY_MS);
            v["forgot"] = json!({ "site": f.0, "term": nl.term.0 });
            v
        }
        _ => json!({ "error": host::call_error_json(&CallError::NotLeader(nl.clone())) }),
    }
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

/// The leader's lease clock (`host::Stats::lease_clock`): per appended entry, how far its `at` (the clock leases are
/// judged by) fell behind the isolate's clock when the entry left, and one macrotask later. Test installs only.
#[cfg(feature = "test-faults")]
fn lease_clock_pcts(rows: &[[u32; 2]]) -> Value {
    let col = |i: usize| rows.iter().map(|r| r[i]).collect::<Vec<_>>();
    json!({ "atToSend": percentiles(&col(0)), "atToNextTask": percentiles(&col(1)) })
}

/// `{n, p50, p99, max}` of `v`, sorted in place.
fn pcts(v: &mut [u32]) -> Value {
    v.sort_unstable();
    let at = |p: f64| v.get(((v.len() as f64 * p) as usize).min(v.len().saturating_sub(1))).copied();
    json!({ "n": v.len(), "p50": at(0.5), "p99": at(0.99), "max": v.last() })
}

/// `pcts` of each of a read's stages (`host::Stats::read_stages`): `grant`, `waited`, and `total`, and how many waited.
fn read_stage_pcts(rows: &[[u32; 3]]) -> Value {
    let col = |i: usize| rows.iter().map(|r| r[i]).collect::<Vec<_>>();
    json!({ "grant": pcts(&mut col(0)), "waited": pcts(&mut col(1)), "total": pcts(&mut col(2)),
        "waitedN": rows.iter().filter(|r| r[1] > 0).count() })
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

/// Whether the daily near check measures now: a client call reached this member within DRAIN_IDLE_MS (`client_at`,
/// 0 in an isolate no client call reached), and it has no rows or one is a day old.
fn near_due(now: u64, client_at: u64, rows: &[(SiteId, graftyard_types::Rtt)]) -> bool {
    now.saturating_sub(client_at) < crate::host::DRAIN_IDLE_MS
        && (rows.is_empty() || rows.iter().any(|r| now.saturating_sub(r.1.at.0) >= DAY_MS))
}

/// What the daily near check does with a fresh measurement.
#[derive(Debug, PartialEq, Eq)]
enum NearStep {
    /// The home is within `NEAR_RECHECK`: any note is cleared.
    Clear,
    /// Over it for the first time: noted, nothing moves.
    Note,
    /// Over it again with the note under a day old.
    Wait,
    /// Over it on two measurements a day or more apart: the nearest voter is re-placed.
    Replace,
}

/// The daily near check's step from the home's rows (`rows`), its voters, and when it was first noted over the bound
/// (`since`): the bound is `NEAR_RECHECK`, not `NEAR_RTT`, so a pair inside its own spread never moves a member.
fn near_next(
    sites: &graftyard_types::SiteTable,
    rows: &[(SiteId, graftyard_types::Rtt)],
    voters: &[graftyard_types::Member],
    home: SiteId,
    since: Option<u64>,
    now: u64,
) -> NearStep {
    let checks = graftyard_types::PlacementChecks {
        sites,
        separate_accounts: false,
        rtt: rows,
        near: graftyard_types::NEAR_RECHECK,
    };
    match (checks.check_near(voters, home).is_err(), since) {
        (false, _) => NearStep::Clear,
        (true, None) => NearStep::Note,
        (true, Some(t)) if now.saturating_sub(t) >= DAY_MS => NearStep::Replace,
        (true, Some(_)) => NearStep::Wait,
    }
}
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
    Actions(Rc<Host<crate::actions::Actions>>),
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
            Hosted::Actions($x) => $body,
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
            Kind::Actions => Hosted::Actions(Host::new(state, env)),
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
        if crate::site_down(&self.env) {
            return Response::error("site down", 503);
        }
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
        if crate::site_down(&self.env) {
            return Response::empty();
        }
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

fn meta<C: graftyard_raft::Command>(m: &raft::Member<C>, me: &str) -> Value {
    let r = m.meta();
    let mut v = json!({ "term": r.term.0, "leader": r.leader.map(|s| s.0), "configIndex": r.config_index.0 });
    if removed(m.committed_config().map(|c| &c.1), me) {
        v["removed"] = json!(true);
    }
    v
}

/// Whether `config`, the member's latest committed one, no longer names the member `me` (its `do_name`): a voter a
/// replacement removed, or a learner dropped. It keeps answering with that config (`design/DESIGN.md`, Shape, "Sites
/// and placement"), and every reply's `meta` says `removed`, so a caller takes the config and never reads the
/// member's own `NotLeader` naming no leader as a leaderless group (B0-32 m3fr2: site b's first-generation voter,
/// replaced at config index 3, drew 72 `/suspect` reports at a leader in term 2). No config yet is not removed.
fn removed(config: Option<&raft::GroupConfig>, me: &str) -> bool {
    let Some(c) = config else { return false };
    let names = |p: &graftyard_types::Placement| p.voters().iter().chain(p.learners()).any(|m| &*m.do_name == me);
    match c {
        raft::GroupConfig::Normal(p) => !names(p),
        // The lost site's voter is out of the group, as `quorum::voters` counts it; learners stay.
        raft::GroupConfig::Degraded { placement, lost } => {
            !placement.voters().iter().any(|m| &*m.do_name == me && m.site != *lost)
                && !placement.learners().iter().any(|m| &*m.do_name == me)
        }
        raft::GroupConfig::Joint { old, new } => !names(old) && !names(new),
    }
}

/// The site id at `k` in a request body, refused when it is missing or past `u8`, so `{"site": 258}` never names
/// site 2. `what` names the route in the error.
fn site_field(b: &Value, k: &str, what: &str) -> Result<SiteId> {
    let n = b.get(k).and_then(Value::as_u64).ok_or_else(|| err(format!("{what} needs {k}")))?;
    u8::try_from(n).map(SiteId).map_err(|_| err(format!("{what}: {k} {n} is not a site id")))
}

/// A `/join` answer, with `graftyard_lookup::JOINED_FRESH` when this join built the member's core, so a lookup's
/// `create` places only a group it bootstrapped.
fn joined(r: Result<Response>, fresh: bool) -> Result<Response> {
    let r = r?;
    if fresh {
        r.headers().set(graftyard_lookup::JOINED_FRESH, "1")?;
    }
    Ok(r)
}

/// A `/move` step's new placement: `voters` and `home` checked by `Placement::new`, with the learners of `cfg`, the
/// committed config the step was planned from (a `Joint`'s new half). A move changes voters and the home; learners
/// are per site and slot, not per voter, so they stay as they are (`learners-kept`).
fn move_placement(
    cfg: &raft::GroupConfig,
    voters: arrayvec::ArrayVec<MemberRef, 3>,
    home: SiteId,
    checks: &graftyard_types::PlacementChecks,
) -> std::result::Result<graftyard_types::Placement, String> {
    let p = match cfg {
        raft::GroupConfig::Normal(p) | raft::GroupConfig::Degraded { placement: p, .. } => p,
        raft::GroupConfig::Joint { new, .. } => new,
    };
    p.next(voters, p.learners().iter().cloned().collect(), home, checks).map_err(|e| e.to_string())
}

/// `Ok` while the committed config is still the one at `based_on`, the config a change was built from: a change
/// built from an older read would undo every change committed since (`learners-kept`).
fn unchanged_since(at: Option<u64>, based_on: u64) -> std::result::Result<(), String> {
    if at == Some(based_on) { Ok(()) } else { Err(format!("the committed config moved from {based_on} to {at:?}")) }
}

/// How far a voter that a `Degraded` config needs may stand behind the leader's commit when the config is proposed
/// [guess: the leader's `CATCHUP_MAX`, one AppendEntries' worth]. A voter further behind may be installing a snapshot,
/// and acks nothing until its install ends: B0-30's rerun committed `Degraded { lost: 1 }` with site c 8,351 entries
/// behind and installing, and the parent committed nothing again (`tests/am1/results/b030-split.md`, "Cause 2").
const PLACE_LAG_MAX: u64 = 64;

/// Proof that every voter a `Degraded` config needs was caught up when the leader asked (`degraded_needs`); only it
/// lets `Routes::degrade` propose one.
struct Ready(());

/// For a leader at `commit`, the voters other than itself and the one being replaced, each with its applied index, or
/// `None` if its `/status` did not answer: `Ready`, or the ones behind by more than `PLACE_LAG_MAX` or unknown.
fn degraded_needs(commit: u64, applied: &[(SiteId, Option<u64>)]) -> std::result::Result<Ready, Value> {
    let behind: Vec<Value> = applied
        .iter()
        .filter(|(_, a)| a.is_none_or(|a| commit.saturating_sub(a) > PLACE_LAG_MAX))
        .map(|(s, a)| json!({ "site": s.0, "applied": a, "commit": commit }))
        .collect();
    if behind.is_empty() { Ok(Ready(())) } else { Err(json!(behind)) }
}

/// `Degraded { lost }`, whose quorum is every voter but `lost`: built only from `Ready`, which only `degraded_needs`
/// builds, so a re-placement proposes it only once every voter it needs was found caught up.
fn degraded(_ready: Ready, placement: Box<graftyard_types::Placement>, lost: SiteId) -> raft::GroupConfig {
    raft::GroupConfig::Degraded { placement, lost }
}

/// One try of a re-placement (`Steps::try_member`): the placement it passed; else, when only the near bound refused
/// it, that placement and the near round trip; the leader's fresh round trip to it; and its row for the answer.
struct TryOut {
    passed: Option<graftyard_types::Placement>,
    over: Option<(graftyard_types::Placement, u16)>,
    fresh: Option<Rtt>,
    row: Value,
}

/// What a re-placement does outside its own order (`replace_steps`): the leader's calls and storage, which a test
/// replaces with a recorder.
trait Steps {
    /// `voters_ready`: every voter `Degraded` would need is caught up.
    async fn ready(&self) -> Result<std::result::Result<Ready, Value>>;
    /// Joins `new`, measures it, and checks the placement it would make; `first` for the first try.
    async fn try_member(&self, new: &MemberRef, first: bool) -> Result<TryOut>;
    /// The committed config and its index, read now.
    fn committed(&self) -> Option<(u64, raft::GroupConfig)>;
    /// Appends `cfg` and waits for it to commit, only while the committed config is still the one at `based_on`
    /// (`Routes::change`).
    async fn change(&self, cfg: raft::GroupConfig, based_on: u64) -> std::result::Result<u64, String>;
    /// Clears a try the group never took (test installs).
    async fn clear(&self, m: &MemberRef);
    fn store_rtt(&self, r: Rtt) -> Result<()>;
    /// Puts `kept`'s registry row back over the tries' rows (`Routes::restore_row`).
    async fn restore_row(&self, kept: &MemberRef, joined: &[MemberRef]);
    fn placed_over_near(&self);
    fn now(&self) -> u64;
    /// A log line; `error` for `console_error!`.
    fn note(&self, error: bool, m: String);
}

/// A re-placement's order (G22, `decisions/DECISIONS.md`, "A re-placement runs its tries before it degrades the
/// group"): `ready`, every try under the committed `Normal` config, then, once a member is chosen, `ready` again and
/// `Degraded { lost }` and `Normal` back to back. A search in which no try was nearer, or a voter behind at the second
/// `ready`, changes no config. On b030r5 `Degraded` came before the first try and held a just-built group for 535 s.
/// Both changes are built from the config committed when they are proposed, not the one the search started from:
/// its learners go into both, and voters or a home that moved meanwhile end the re-placement with no config change
/// (`learners-kept`, `decisions/DECISIONS.md`). B0-32 m3fr13's create placement started before `/learners`, committed
/// after it from its first read, and dropped all 8 learners.
async fn replace_steps<S: Steps>(
    s: &S,
    chain: ChainId,
    p: Box<graftyard_types::Placement>,
    old: MemberRef,
    tries: usize,
    keep_below: Option<u16>,
) -> Result<std::result::Result<Value, Value>> {
    let behind = |b: Value, tried: &[Value]| {
        s.note(false, format!("placement of {chain} waits: {b}"));
        Ok(Err(json!({ "error": "a voter the degraded config needs is behind", "behind": b, "tried": tried })))
    };
    if let Err(b) = s.ready().await? {
        return behind(b, &[]);
    }
    let t0 = s.now();
    let to = old.site;
    let mut generation = p.next_generation();
    let mut tried: Vec<Value> = Vec::new();
    // Every try's member: each one's `/join` wrote its row on `to`'s registry, which `restore_row` puts back.
    let mut joined: Vec<MemberRef> = Vec::new();
    // Tries that passed every check but the near bound, kept until the loop ends: with `keep_below`, the nearest goes
    // in when none passed (`graftyard_types::nearest_try`).
    let mut over: Vec<(MemberRef, graftyard_types::Placement, u16, Option<Rtt>)> = Vec::new();
    let placed = loop {
        let new = MemberRef::new(chain, to, old.kind, old.slot, generation);
        joined.push(new.clone());
        let t = s.try_member(&new, tried.is_empty()).await?;
        tried.push(t.row);
        if let Some(np) = t.passed {
            if let Some(r) = t.fresh {
                s.store_rtt(r)?;
            }
            break Some((np, None, new));
        }
        match t.over.filter(|_| keep_below.is_some()) {
            Some((np, ms)) => over.push((new.clone(), np, ms, t.fresh)),
            None => s.clear(&new).await,
        }
        if tried.len() >= tries {
            // No try met the bound: the nearest goes in if it beats the member it replaces, so a create never fails
            // on the bound alone (`design/DESIGN.md`, "Placement is checked by latency").
            let ms: Vec<u16> = over.iter().map(|o| o.2).collect();
            let best = keep_below.and_then(|c| graftyard_types::nearest_try(c, &ms)).map(|i| over.swap_remove(i));
            for (m, ..) in &over {
                s.clear(m).await;
            }
            if let Some((_, _, _, Some(r))) = &best {
                s.store_rtt(*r)?;
            }
            break best.map(|(m, np, ms, _)| (np, Some(ms), m));
        }
        generation += 1;
    };
    let Some((np, near_ms, kept)) = placed else {
        s.restore_row(&old, &joined).await;
        if keep_below.is_some() {
            // The bound is a goal: the member already placed stays, nearer than every try, and no config changed.
            s.placed_over_near();
            let kept = json!({ "kept": old.do_name, "tried": tried });
            s.note(false, format!("placement of {chain}: no try nearer than the current member: {kept}"));
            return Ok(Ok(kept));
        }
        let e = json!({ "error": "no placement passed", "tried": tried });
        s.note(true, format!("placement of {chain} failed: {e}"));
        return Ok(Err(e));
    };
    // The voters `Degraded` needs may have fallen behind during the tries: asked again, and a voter behind changes no
    // config (operator, 2026-11-17).
    let ready = match s.ready().await? {
        Ok(r) => r,
        Err(b) => {
            s.clear(&kept).await;
            s.restore_row(&old, &joined).await;
            return behind(b, &tried);
        }
    };
    // The config committed now, not the one the tries started from: a `/learners` change may have committed during
    // them. Its learners go into both changes; voters or a home that moved end it with no config change.
    let fresh = match s.committed() {
        Some((i, raft::GroupConfig::Normal(c))) if c.voters() == p.voters() && c.home() == p.home() => {
            np.with_learners(c.learners()).ok().map(|np| (i, *c, np))
        }
        _ => None,
    };
    let Some((based_on, p, np)) = fresh else {
        s.clear(&kept).await;
        s.restore_row(&old, &joined).await;
        let at = s.committed().map(|c| c.0);
        s.note(false, format!("placement of {chain}: the committed config moved to {at:?} during the tries"));
        return Ok(Err(json!({ "error": "the committed config changed during the re-placement", "configIndex": at,
            "tried": tried })));
    };
    let degraded_at = match s.change(degraded(ready, Box::new(p), to), based_on).await {
        Ok(i) => i,
        Err(e) => {
            s.restore_row(&old, &joined).await;
            return Ok(Err(json!({ "error": format!("degraded: {e}"), "tried": tried })));
        }
    };
    if let Some(ms) = near_ms {
        s.placed_over_near();
        s.note(false, format!("placement of {chain}: no try within NEAR_RTT; kept the nearest, {ms} ms"));
    }
    let index = match s.change(raft::GroupConfig::Normal(Box::new(np)), degraded_at).await {
        Ok(i) => i,
        Err(e) => return Ok(Err(json!({ "error": format!("normal: {e}"), "tried": tried }))),
    };
    s.restore_row(&kept, &joined).await;
    let mut v = json!({ "old": old.do_name, "tried": tried, "configIndex": index, "ms": s.now() - t0 });
    if let Some(ms) = near_ms {
        v["nearOver"] = json!(ms);
    }
    Ok(Ok(v))
}

/// The leader's side of `Steps`.
struct RouteSteps<'a, F: Family> {
    r: &'a Routes<F>,
    chain: ChainId,
    record: PlacementRecord,
    me: SiteId,
    p: Box<graftyard_types::Placement>,
    to: SiteId,
    others: Vec<MemberRef>,
    site: SiteEnv,
    mine: Vec<(SiteId, Rtt)>,
    near: &'a [(graftyard_types::Region, u16)],
    fail_first: bool,
    keep_below: Option<u16>,
    owner: Option<AccountId>,
    genesis: Option<String>,
}

impl<F: Family> Steps for RouteSteps<'_, F> {
    async fn ready(&self) -> Result<std::result::Result<Ready, Value>> {
        self.r.voters_ready(&self.p, self.me, self.to).await
    }

    async fn try_member(&self, new: &MemberRef, first: bool) -> Result<TryOut> {
        let join = Join {
            chain: self.chain,
            me: new.clone(),
            record: self.record.clone(),
            peers: self.others.clone(),
            owner: self.owner,
            genesis: self.genesis.clone(),
            #[cfg(feature = "test-faults")]
            no_claim: false,
        };
        let (status, rows) = self.r.post_member(new, "/join", &serde_json::to_value(&join)?).await?;
        let theirs: Vec<(SiteId, Rtt)> = serde_json::from_value(rows.clone()).unwrap_or_default();
        let voters: arrayvec::ArrayVec<MemberRef, 3> =
            self.p.voters().iter().map(|m| if m.site == self.to { new.clone() } else { m.clone() }).collect();
        // This member's row for `to` measured the member being replaced: measure the new one instead, so the check,
        // and the daily recheck after it, read the pair as it now is.
        let fresh = if status == 200 {
            let ms = self.r.measure(&self.site, new).await?;
            Some(Rtt { peer: self.to, median_ms: ms, at: Timestamp(now_ms()) })
        } else {
            None
        };
        let all: Vec<(SiteId, Rtt)> = self
            .mine
            .iter()
            .filter(|(_, r)| r.peer != self.to)
            .copied()
            .chain(fresh.map(|r| (self.me, r)))
            .chain(theirs.iter().copied())
            .collect();
        let checks = graftyard_types::PlacementChecks {
            sites: &self.site.sites,
            separate_accounts: false,
            rtt: &all,
            near: self.near,
        };
        let mut over = None;
        let r = if self.fail_first && first {
            Err("injected: latency check failed".to_owned())
        } else if status == 200 {
            let learners: arrayvec::ArrayVec<MemberRef, 16> = self.p.learners().iter().cloned().collect();
            match self.p.next(voters.clone(), learners.clone(), self.p.home(), &checks) {
                Err(e @ graftyard_types::PlacementError::NoNearFollower { ms, .. }) if self.keep_below.is_some() => {
                    let unbounded = graftyard_types::PlacementChecks { near: &[], ..checks };
                    if let Ok(np) = self.p.next(voters, learners, self.p.home(), &unbounded) {
                        over = Some((np, ms));
                    }
                    Err(e.to_string())
                }
                r => r.map_err(|e| e.to_string()),
            }
        } else {
            Err(format!("join answered {status}: {rows}"))
        };
        let ms: Vec<(u8, u16)> = theirs.iter().map(|(_, r)| (r.peer.0, r.median_ms)).collect();
        let row = json!({ "member": new.do_name, "rtt": ms, "result": r.as_ref().err() });
        Ok(TryOut { passed: r.ok(), over, fresh, row })
    }

    fn committed(&self) -> Option<(u64, raft::GroupConfig)> {
        self.r.host.core.borrow().as_ref().and_then(|c| c.m.committed_config().map(|(i, c)| (i.0, c.clone())))
    }

    async fn change(&self, cfg: raft::GroupConfig, based_on: u64) -> std::result::Result<u64, String> {
        self.r.change(cfg, based_on).await
    }

    async fn clear(&self, m: &MemberRef) {
        #[cfg(feature = "test-faults")]
        let _ = self.r.post_member(m, "/retire", &json!({})).await;
        #[cfg(not(feature = "test-faults"))]
        let _ = m;
    }

    fn store_rtt(&self, r: Rtt) -> Result<()> {
        self.r.store_rtt(r.peer, r.median_ms, r.at.0)
    }

    async fn restore_row(&self, kept: &MemberRef, joined: &[MemberRef]) {
        self.r.restore_row(self.chain, &self.record, kept, joined).await
    }

    fn placed_over_near(&self) {
        self.r.host.stats.borrow_mut().placed_over_near += 1;
    }

    fn now(&self) -> u64 {
        now_ms()
    }

    fn note(&self, error: bool, m: String) {
        if error {
            console_error!("{m}");
        } else {
            console_log!("{m}");
        }
    }
}

/// A leader's transfer to a member that may not lead (`Family::may_lead`): refused at once, so the leader picks another
/// target instead of waiting out its LEASE; the member never steps it. `None` for every other message, or a member
/// that may lead.
fn refuse_lead<E>(may_lead: bool, e: &raft::Envelope<E>) -> Option<raft::Envelope<E>> {
    match e.msg {
        raft::Message::TimeoutNow { term, transfer } if !may_lead => {
            Some(raft::Envelope { from: e.to, to: e.from, msg: raft::Message::TransferRefused { term, transfer } })
        }
        _ => None,
    }
}

/// `Trace::Reported`'s verdict codes, in `verdict_str`'s names. Append-only, as trace kinds are.
#[cfg(feature = "test-faults")]
const VERDICTS: &[&str] =
    &["leader", "resumed", "started", "bootstrapping", "promised", "busy", "won", "lost", "mayNotLead"];

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

/// Until when a `StaleMode` flag, committed or pending, covers this member's chain in this isolate's cached `Release`
/// view, which a call here refreshes beside the request when it is due (`graftyard_lookup::view`): every isolate,
/// members included, caches the view. A `Release` member's own chain is never covered, so it reads no view.
fn stale_until<F: Family>(h: &Host<F>) -> Option<graftyard_types::Timestamp> {
    let (chain, owner) = h.core.try_borrow().ok()?.as_ref().map(|c| (c.join.chain, c.join.owner))?;
    if chain == ChainId::Release {
        return None;
    }
    if graftyard_lookup::view::due() {
        let anchor = crate::release::anchor(&h.env)?;
        let site = SiteEnv::read(&h.env).ok()?;
        #[cfg(feature = "test-faults")]
        if let Some(ms) = h.env.var("GRAFTYARD_STALE_MAX_MS").ok().and_then(|v| v.to_string().parse().ok()) {
            graftyard_lookup::view::set_stale_max(ms);
        }
        graftyard_lookup::view::refresh_beside(&h.env, &site.sites, &anchor, None, true);
    }
    graftyard_lookup::view::stale_until(chain, owner)
}

/// The member's routes for one command family.
struct Routes<F: Family> {
    host: Rc<Host<F>>,
}

impl<F: Family> Routes<F> {
    async fn fetch(&self, mut req: Request, url: Url) -> Result<Response> {
        // F74's call trace: the handler's entry, before any injected fault awaits, and the loop lag there.
        #[cfg(feature = "test-faults")]
        let (entered, entry_lag) = (now_ms(), self.host.loop_lag());
        // F74's inbound trace: every request, numbered as it reaches the handler, held open until the handler ends.
        #[cfg(feature = "test-faults")]
        let (_open, arrival) = {
            let header = |k: &str| req.headers().get(k).ok().flatten().and_then(|v| v.parse::<u64>().ok());
            let from = header("x-graft-from").and_then(|v| u8::try_from(v).ok()).unwrap_or(255);
            let (index, open, guard) = self.host.inbound_log.enter(url.path());
            self.host.trace_at(entered, Trace::Entered, from, header("x-graft-call").unwrap_or(0), index, open);
            if let Some(by) = header(graftyard_lookup::SENT_BY) {
                self.host.trace_at(entered, Trace::SentBy, from, index / 64, by / 16, by % 16);
            }
            (guard, index / 64)
        };
        // Every request this object takes, by route: B0-18's leader load.
        *self.host.stats.borrow_mut().inbound.entry(url.path().to_owned()).or_insert(0) += 1;
        // A peer's `/raft` call or an edge's request carries its sender's clock (`crate::clock`).
        self.host.stamped(req.headers().get(crate::clock::STAMP).ok().flatten());
        // An edge's call for a client: the daily near check runs only after one (`near_due`).
        if req.headers().has(graftyard_lookup::CLIENT_CALL)? {
            self.host.client_at.set(now_ms());
        }
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
                    | "/trace"
                    | "/elections"
            );
            if !observe && url.path() != "/sever" && self.host.severed() {
                crate::faults::count("severed:in");
                return Response::error("member severed (injected)", 503);
            }
            if !observe
                && let Some(hang) = crate::faults::site_fault(
                    &self.host.env,
                    self.host.chain(),
                    matches!(url.path(), "/raft" | "/campaign" | "/suspect"),
                )
                .await
            {
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
                // F74's call trace: the caller's call id and address.
                #[cfg(feature = "test-faults")]
                let (id, from) = {
                    let header = |k: &str| req.headers().get(k).ok().flatten().and_then(|v| v.parse::<u64>().ok());
                    (header("x-graft-call").unwrap_or(0), header("x-graft-from").unwrap_or(0) as u8)
                };
                #[cfg(feature = "test-faults")]
                if let Some(sent) = req.headers().get("x-graft-sent").ok().flatten().and_then(|v| v.parse().ok()) {
                    // From a header: decoding the body here as well held a second decoded copy of every append
                    // while `raft` decoded it again (gate 16's storm, `tests/am1/results/narrow-reads-3.md`).
                    self.host.trace_at(t0, Trace::CallArrived, from, id, sent, t1 - t0);
                }
                let r = self.raft(&body).await;
                self.host.stats.borrow_mut().raft_in(t0, body.len(), t1 - t0, now_ms() - t1);
                #[cfg(feature = "test-faults")]
                let r = r.and_then(|mut res| {
                    let left = now_ms();
                    res.headers_mut().set("x-graft-left", &left.to_string())?;
                    if id != 0 {
                        self.host.trace_at(left, Trace::CallLeft, from, id, entered, entry_lag);
                    }
                    Ok(res)
                });
                r
            }
            // Stale mode: writes return 503. Answered as an error the caller returns at once (`commit_hedged`), not
            // as a missed call it would send again for a minute.
            (Method::Post, "/commit" | "/commit_many") if let Some(until) = stale_until(&self.host) => {
                self.reply(json!({ "error": { "stale": { "until": until.0 } } }))
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
                self.ready_or(|| self.outcome_by_request(body)).await
            }
            (Method::Post, "/read") => {
                let body: Value = req.json().await?;
                // m41-held: which query this `/read` is, joined to its `Entered` row by arrival number.
                #[cfg(feature = "test-faults")]
                {
                    let index = body.get("index").and_then(Value::as_bool) == Some(true);
                    let code = crate::host::read_query_code(&body);
                    self.host.trace(Trace::ReadQuery, 0, arrival, code, u64::from(index));
                }
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
                let body = req.json::<Value>().await.unwrap_or(Value::Null);
                // Test installs: `{"near_ms": MS}` bounds every region at MS, so a hosted run can make every try miss.
                #[cfg(feature = "test-faults")]
                let near = body["near_ms"].as_u64().map(|ms| {
                    [graftyard_types::Region::Enam, graftyard_types::Region::Weur, graftyard_types::Region::Apac]
                        .map(|r| (r, ms as u16))
                        .to_vec()
                });
                #[cfg(not(feature = "test-faults"))]
                let near = None;
                if body["background"].as_bool() == Some(true) {
                    return self.ready_or(|| async { self.place_background(near) }).await;
                }
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
            // `?clock=1` (L0-12's clock line): this isolate's clock read now and again after one awaited macrotask.
            // A busy leader whose clock lags the others' shows it here: `now` read before the event loop runs again
            // is the time the input began, and the second read is the time a lease decision taken next would use.
            // Test installs only.
            #[cfg(feature = "test-faults")]
            (Method::Get, "/status") if url.query_pairs().any(|(k, v)| k == "clock" && v == "1") => {
                let before = self.host.now();
                Delay::from(Duration::ZERO).await;
                let mut v = self.status();
                v["clockBefore"] = json!(before);
                v["clockAfter"] = json!(self.host.now());
                v["leaseClock"] = json!(self.host.stats.borrow().lease_clock.last().copied());
                Response::from_json(&v)
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
                let reads = self.host.reads_in_flight.get();
                let page = host::entries_page_bytes(local, q("bytes"), reads);
                if local && reads >= host::ENTRIES_BUSY_READS && q("bytes").is_some_and(|b| b as usize > page) {
                    self.host.stats.borrow_mut().busy_pages += 1;
                    console_log!("entries page capped at {page} B: {reads} reads in flight");
                }
                let e = self.host.entries_json(q("from").unwrap_or(1), q("to").unwrap_or(u64::MAX), page)?;
                self.reply(json!({ "entries": e }))
            }
            (Method::Get, "/stats") => {
                // Test installs, B0-02's A/B: `body=array` or `body=stream` sets how this member's calls send their
                // body until it changes or the object resets (`Host::array_body`).
                #[cfg(feature = "test-faults")]
                if let Ok(b) = crate::registry::query(&url, "body") {
                    self.host.array_body.set(b == "array");
                }
                // `arrival=mem` or `arrival=sql`: where BF-14's arrival rows go (`Host::arrivals_mem`).
                #[cfg(feature = "test-faults")]
                if let Ok(a) = crate::registry::query(&url, "arrival") {
                    let mut m = self.host.arrivals_mem.borrow_mut();
                    match (a == "mem", m.is_some()) {
                        (true, false) => *m = Some(std::collections::VecDeque::new()),
                        (false, _) => *m = None,
                        _ => {}
                    }
                }
                // `entry=max`, `entry=soft`, or `entry=<bytes>`: where the leader stops filling an entry
                // (`Host::entry_soft`).
                #[cfg(feature = "test-faults")]
                if let Ok(e) = crate::registry::query(&url, "entry") {
                    let soft = match e.as_str() {
                        "max" => raft::ENTRY_MAX,
                        n => n.parse().unwrap_or(raft::ENTRY_SOFT),
                    };
                    self.host.entry_soft.set(soft);
                    if let Some(core) = self.host.core.borrow_mut().as_mut() {
                        core.m.set_entry_soft(soft);
                    }
                }
                self.stats(
                    crate::registry::query(&url, "reset").is_ok(),
                    crate::registry::query(&url, "summary").is_ok(),
                    crate::registry::query(&url, "high").is_ok(),
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
            // BF-56's kill of a repository's commit path on a product install: the member resets (`ctx.abort()`), as a
            // lost isolate does, and restores as a follower, so its group has no leader until an election. Only an
            // operator's binding sends it: the edge forwards no path a client names to a member.
            (Method::Post, "/abort") => {
                self.host.dying.set(true);
                host::abort(&self.host.state, "graftyard member abort");
                Response::from_json(&json!({ "aborted": true, "at": now_ms() }))
            }
            // A deleted repository's purge (`graftyard_git::purge`; `decisions/DECISIONS.md`, "`repo-purge`" and
            // "`purge-outside`"): this member's storage is deleted and the object resets, so it answers as never
            // joined. A member still outside the config the edge read, unchecked; otherwise only a repository member
            // whose fold holds the tombstone, or its metadata child, and only outside a group change; 409 otherwise. Only the edge's admin route sends it: the edge forwards no path a client names to a member.
            (Method::Post, "/purge_retire") => {
                // `{"outside": I}`: the edge read this member outside the newest committed config, at index I
                // (`decisions/DECISIONS.md`, "`purge-outside`"). It is retired unchecked only while it still is.
                let outside = req.json::<Value>().await.ok().and_then(|b| b["outside"].as_u64());
                let refused = {
                    let core = self.host.core.borrow();
                    let c = core.as_ref().ok_or_else(|| err("not joined"))?;
                    let own = c.m.committed_config().map(|(i, cfg)| (i.0, cfg));
                    if outside.is_some_and(|i| graftyard_git::purge::outside_may_retire(own, &c.join.me.do_name, i)) {
                        None
                    } else if !F::purgeable(&c.m.group_fold().fold) {
                        Some("notDeleted")
                    } else if self.changing(&c.m) {
                        Some("changing")
                    } else {
                        None
                    }
                };
                if let Some(why) = refused {
                    return Response::from_json(&json!({ "refused": why })).map(|r| r.with_status(409));
                }
                self.host.dying.set(true);
                self.host.state.storage().delete_alarm().await?;
                self.host.state.storage().delete_all().await?;
                host::abort(&self.host.state, "graftyard member purged");
                Response::from_json(&json!({ "retired": true }))
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
            // narrow-reads-3's trace saver (`tests/nr3/tsave.mjs`): the `Stats::trace` rows after `since` (ms), as one
            // array and no `Value` tree, so a run keeps a member's rows from before its isolate resets.
            (Method::Get, "/trace") => {
                let since: u64 = crate::registry::query(&url, "since").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
                let s = self.host.stats.borrow();
                let rows: Vec<&[u64; 6]> = s.trace.iter().filter(|r| r[0] > since).collect();
                Response::from_json(&rows)
            }
            (Method::Get, "/faults") => self.faults(crate::registry::query(&url, "rid").ok()),
            (Method::Get, "/deps_log") => self.deps_log(),
            (Method::Get, "/isolate") => Response::from_json(&crate::faults::isolate_json(&self.host.env)),
            // The election trace (`host::Elect`): rows `[at, kind, term, peer, index, extra]` with `at` past `since`,
            // and this isolate's clock.
            (Method::Get, "/elections") => {
                let since: u64 = crate::registry::query(&url, "since").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
                let rows: Vec<[u64; 6]> =
                    self.host.stats.borrow().elections.iter().filter(|r| r[0] > since).copied().collect();
                Response::from_json(&json!({ "rows": rows, "now": self.host.now() }))
            }
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
                let now = self.host.now();
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

    /// When a placement started on this member (`place_background`'s and the daily check's marker), while it runs.
    fn placing_since(&self) -> Option<u64> {
        host::rows(&self.host.state.storage().sql(), "SELECT v FROM me WHERE k = 'near_check'", vec![])
            .ok()
            .and_then(|r| r.first().and_then(|r| host::text(&r[0]).ok()).and_then(|t| t.parse::<u64>().ok()))
            .filter(|t| now_ms().saturating_sub(*t) < NEAR_CHECK_FOR)
    }

    /// Whether this member is in a group change (`graftyard_git::purge::changing`).
    fn changing(&self, m: &raft::Member<F::C>) -> bool {
        graftyard_git::purge::changing(
            m.committed_config().map(|(_, c)| c),
            m.latest_config(),
            self.placing_since().is_some(),
        )
    }

    fn meta(&self) -> Value {
        self.host.core.borrow().as_ref().map_or(Value::Null, |c| meta(&c.m, &c.join.me.do_name))
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
        // A bare `exec` here is a write outside the entry's transaction, and the output gate holds every later fetch,
        // the leader's AppendEntries included, until it is durable (`tests/m0/results/carried-6.md`).
        if let Some(mem) = self.host.arrivals_mem.borrow_mut().as_mut() {
            if mem.len() >= ARRIVALS_MEM_MAX {
                mem.pop_front();
            }
            mem.push_back(json!({
                "rid": rid, "at": now_ms(), "role": role, "fault": receipt.map(Receipt::name), "answer": answer,
            }));
        } else {
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
        }
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
        if let Some(mem) = self.host.arrivals_mem.borrow().as_ref() {
            let rows: Vec<&Value> = mem.iter().filter(|r| r["at"].as_u64().unwrap_or(0) >= since).collect();
            return Response::from_json(&rows);
        }
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
    /// `stages`, `[grant, waited]` ms, is how long the read waited for its grant (the leader's read index applied, on a
    /// follower or learner) and then for its answer's build (narrow-reads-3), which the edge adds to its `server-timing`.
    fn reply_read(
        &self,
        value: Rc<[u8]>,
        index: u64,
        held: (InFlight, Option<InFlight>),
        stages: [u32; 2],
    ) -> Result<Response> {
        if self.host.fold_lost() {
            return Response::error("member reset: fold storage lost", 503);
        }
        let tail =
            format!(r#","index":{index},"stages":[{},{}],"meta":{}}}"#, stages[0], stages[1], self.meta()).into_bytes();
        let headers = Headers::new();
        headers.set("content-type", "application/json")?;
        let counted = host::Answers::hold(&self.host.answers, &value);
        #[cfg(feature = "test-faults")]
        self.host.mark_mem();
        Ok(Response::from_stream(held_body(br#"{"value":"#.to_vec(), value, tail, (held, counted)))?
            .with_headers(headers))
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
                let batch_of = |e: &raft::Envelope<F::Ev>| match e.msg {
                    raft::Message::ReadIndexRes { batch, .. } => Some(batch),
                    _ => None,
                };
                let mut out = self.host.callers.borrow_mut().take(from, batch_of);
                // This call's own answers count as sent too: a sibling call held on one of them ends.
                let own: Vec<u64> = back.iter().filter_map(batch_of).collect();
                self.host.callers.borrow_mut().left(from, &own);
                out.append(&mut back);
                // AppendEntries held for this caller, or built for it while stepping its call, leave in this response,
                // not as calls: their bytes stop counting now (`raft::Member::call_ended`).
                let boot = self.host.core.borrow().as_ref().map(|c| c.m.boot());
                for e in &out {
                    if let raft::Message::Append(a) = &e.msg
                        && Some(a.boot) == boot
                    {
                        let _ = self.host.input(|m, t| m.call_ended(t, from, a.round));
                    }
                }
                for e in &out {
                    match &e.msg {
                        raft::Message::ReadIndexRes { batch, index } => {
                            self.host.trace(Trace::ResSent, from.0, *batch, index.map_or(0, |(i, _)| i.0), 0)
                        }
                        raft::Message::Append(a) => {
                            let last = a.entries.last().map_or(a.prev_index.0, |x| x.index.0);
                            self.host.trace(Trace::AppendTaken, from.0, a.round, last, a.commit.0)
                        }
                        _ => {}
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
        let may_lead = F::may_lead(&self.host);
        for e in envs {
            if let Some(no) = refuse_lead(may_lead, &e) {
                back.push(no);
                continue;
            }
            if let raft::Message::ReadIndex { batch } = &e.msg {
                self.host.trace(Trace::IndexTaken, from.0, *batch, 0, 0);
                asked.push(*batch);
            }
            self.host.heard(&e);
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
                && !self.host.callers.borrow().answered(from, batch, |e| answered(std::slice::from_ref(e), batch))
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
        res.headers_mut().set(crate::clock::STAMP, &self.host.now().to_string())?;
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
            // LM-1 round 14: what admission prices each command at (`Admission::cmd_bytes`), by the class it queued
            // under: its `stored_len` without an Agent inline pack.
            let priced = (raft::Command::stored_len(&c.cmd), c.admit.map_or(0, |a| a.inline));
            let marked = F::marked(&c.cmd);
            let (r, out) = match c.admit {
                None => {
                    let (r, out) = self.host.step(|m, t| m.commit(t, rid, c.cmd))?;
                    (r.map_err(raft::admit::AdmitError::Call), out)
                }
                Some(a) => self.host.step(|m, t| m.commit_as(t, rid, c.cmd, a.admit()))?,
            };
            if let Ok(a) = &r {
                self.host.stats.borrow_mut().admitted_bytes(a.class, priced.0, priced.1);
            }
            let w = match r {
                Ok(a) => Ok((a.ticket, self.host.wait_ticket(a.ticket, Some((rid, at, marked))))),
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
                        Some(Ok(o)) => json!({ "done": self.done(&o).await }),
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
            return self.reply(json!({ "done": self.done(&o).await }));
        }
        let rx = self.host.wait_ticket(ticket, None);
        self.host.kick();
        match within(Duration::from_millis(wait), rx).await {
            Some(Ok(o)) => self.reply(json!({ "done": self.done(&o).await })),
            Some(Err(_)) => Response::error("member reset", 503),
            None => self.reply(json!({ "pending": host::ticket_json(&ticket) })),
        }
    }

    async fn outcome_by_request(&self, body: CommitReq<F>) -> Result<Response> {
        let rid = host::rid_from(&body.rid)?;
        let r = {
            let core = self.host.core.borrow();
            let m = &core.as_ref().ok_or_else(|| err("member not joined"))?.m;
            m.outcome_by_request(rid, &body.cmd)
        };
        let v = match r {
            Ok(ByRequest::Done(a)) => json!({ "done": self.done(&Ok(a)).await }),
            Ok(ByRequest::Pending(t)) => json!({ "pending": host::ticket_json(&t) }),
            Ok(ByRequest::Unknown) => json!({ "unknown": true }),
            Err(nl) => json!({ "error": host::call_error_json(&CallError::NotLeader(nl)) }),
        };
        self.reply(v)
    }

    /// `Host::reply_json`, rendered again after reading the metadata child's rows a reply missed (`crate::split`):
    /// the fold it renders over is at or after the output's index either way.
    async fn done(&self, o: &host::Outcome<F>) -> Value {
        let mut v = self.host.reply_json(o);
        for _ in 0..3 {
            let Some(missed) = v.pointer("/ok/out/missedChild").filter(|m| !m.is_null()).cloned() else { break };
            if let Err(e) = F::fetch_missed(&self.host, &json!({ "missedChild": missed })).await {
                console_error!("a reply's metadata child rows: {e}");
                break;
            }
            v = self.host.reply_json(o);
        }
        v
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
        // While a customer-signed `StaleMode` covers this chain in this isolate's cached `Release` view, every read is
        // `stale()`, on any member and with no lease or read-index round; never without the flag.
        if let Some(until) = stale_until(&self.host) {
            return self.read_stale(&q, until);
        }
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
        let (leading, me, servable, nl, alone) = match self.host.core.borrow().as_ref() {
            Some(c) => (
                c.m.role() == Role::Leader,
                c.m.me(),
                F::follower_serves(&c.m.group_fold().fold, &q),
                Some(c.m.not_leader()),
                c.m.committed_config()
                    .is_none_or(|(_, g)| raft::quorums::voters(g).len() <= 1 && raft::quorums::learners(g).is_empty()),
            ),
            None => (false, SiteId(0), false, None, true),
        };
        let off_leader = !alone && graftyard_lookup::leader_never_serves(&q);
        let learner = graftyard_types::learner_slot(me).is_some();
        // LM-1 round 17: which reads reach this member, plain or read-index, and whether a follower could serve them.
        *self
            .host
            .stats
            .borrow_mut()
            .reads_by
            .entry(format!(
                "{}:{}:{}",
                read_kind(&q),
                if index { "index" } else { "plain" },
                if servable { "f" } else { "l" }
            ))
            .or_insert(0) += 1;
        let (reads, heavies) = (self.host.reads_in_flight.get(), self.host.heavy_reads.get());
        let answers = self.host.answers.borrow().bytes();
        match gate(index, leading, me, servable, heavy, reads, heavies, answers, off_leader) {
            Gate::Serve => {}
            Gate::NotLeader => {
                let mut st = self.host.stats.borrow_mut();
                if leading {
                    st.leader_refused += 1
                } else {
                    st.follower_refused += 1
                }
                drop(st);
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
        // One deadline for the read and each child refetch after it.
        let until = now_ms() + wait.as_millis() as u64;
        let mut fetched = 0;
        loop {
            let t0 = now_ms();
            let (r, out) = self.host.step(|m, t| if index { m.read_index(t) } else { m.read(t) })?;
            if leading && r.is_ok() && self.host.clock.borrow().suspect() {
                self.host.stats.borrow_mut().lease_fallbacks += 1;
            }
            let rx = r.as_ref().ok().map(|id| self.host.wait_read(*id, q.clone()));
            self.host.dispatch(out, None);
            let forgot = || self.host.core.borrow().as_ref().and_then(|c| c.m.forgot());
            let rx = match (r, rx) {
                (Ok(_), Some(rx)) => rx,
                (Err(nl), _) => return self.reply(not_leader_reply(index, &nl, forgot())),
                (Ok(_), None) => unreachable!("a started read has a waiter"),
            };
            self.host.served();
            let left = Duration::from_millis(until.saturating_sub(t0));
            let mut answer = within(left, rx).await;
            let granted = now_ms();
            // An answer whose rebuild is not yet due (`family::dump_wait`, `family::narrow_wait`). A read-index read that
            // may wait sleeps until the build is due, then is answered at the member's state then, which is at or above
            // the leader's read index for this read (`raft::Member::fresh_after`): no second read-index round. A voter's
            // dump read and a leader's plain read are shed.
            while let Some(Ok(Ok((host::ReadBody::Shed(ms, g), _)))) = &answer {
                let (ms, g) = (*ms, g.clone());
                let Some(g) = g.filter(|_| waits_for_rebuild(learner, narrow, index, now_ms(), ms, until)) else {
                    self.host.stats.borrow_mut().shed_reads += 1;
                    return self.reply(graftyard_lookup::overloaded_reply(ms));
                };
                self.host.stats.borrow_mut().waited_reads += 1;
                worker::Delay::from(Duration::from_millis(ms.max(1))).await;
                answer = match self.host.answer_after(&g, &q) {
                    Some(r) => Some(Ok(r)),
                    None => return Response::error("member reset: fold storage lost", 503),
                };
            }
            let done = now_ms();
            let d = |a: u64, b: u64| b.saturating_sub(a) as u32;
            let stages = [d(t0, granted), d(granted, done)];
            if index && matches!(answer, Some(Ok(Ok(_)))) {
                let mut s = self.host.stats.borrow_mut();
                if s.read_stages.len() < host::SAMPLES_MAX {
                    s.read_stages.push([stages[0], stages[1], d(t0, done)]);
                }
            }
            return match answer {
                // An answer that missed a metadata child's rows (`crate::split`): read them, and read again.
                Some(Ok(Ok((host::ReadBody::Json(value), _)))) if value.starts_with(MISSED_CHILD) && fetched < 3 => {
                    fetched += 1;
                    let missed: Value = serde_json::from_slice(&value)?;
                    if let Err(e) = F::fetch_missed(&self.host, &missed).await {
                        return Response::error(format!("the metadata child did not answer: {e}"), 503);
                    }
                    continue;
                }
                Some(Ok(Ok((host::ReadBody::Json(value), index)))) => self.reply_read(value, index.0, held, stages),
                Some(Ok(Ok((host::ReadBody::Shed(..), _)))) => unreachable!("a shed answer is waited out above"),
                Some(Ok(Err(nl))) => self.reply(not_leader_reply(index, &nl, forgot())),
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

    /// A stale read (`design/DESIGN.md`, Shape, "The read path", `stale()`): this member's fold at `applied`, with no
    /// lease and no read-index round, answered with `"stale": {"until": ms}` beside the value.
    fn read_stale(&self, q: &Value, until: graftyard_types::Timestamp) -> Result<Response> {
        self.host.stats.borrow_mut().stale_reads += 1;
        let (value, index) = {
            let core = self.host.core.borrow();
            let c = core.as_ref().ok_or_else(|| err("member not joined"))?;
            let s = c.m.stale();
            let v = match host::fold_answer::<F>(s.fold(), q, s.index().0, now_ms(), family::dump_every(c.m.me())) {
                host::ReadBody::Json(b) => b,
                host::ReadBody::Shed(ms, _) => return self.reply(graftyard_lookup::overloaded_reply(ms)),
            };
            (v, s.index().0)
        };
        let v: Value = serde_json::from_slice(&value).unwrap_or(Value::Null);
        self.reply(json!({ "value": v, "index": index, "stale": { "until": until.0 } }))
    }

    /// `campaign` with no body, `suspect` with `{colo, term}`. Campaigns start only here, from requests.
    fn verdict(&self, report: Option<Value>) -> Result<Response> {
        // A member whose install dropped its metadata tables does not campaign until the install has run
        // (`Family::may_lead`).
        if !F::may_lead(&self.host) {
            return self.reply(json!({ "verdict": verdict_str(Verdict::MayNotLead) }));
        }
        let v = match report {
            None => {
                let v = self.host.input(|m, t| m.campaign(t))?;
                #[cfg(feature = "test-faults")]
                self.host.elect_asked(255, 0, v == Verdict::Started);
                v
            }
            Some(b) => {
                let colo = raft::Colo(b.get("colo").and_then(Value::as_u64).unwrap_or(0) as u32);
                let term = raft::Term(b.get("term").and_then(Value::as_u64).unwrap_or(0));
                let about = site_field(&b, "about", "suspect")?;
                let v = self.host.input(|m, t| m.suspect(t, colo, about, term))?;
                #[cfg(feature = "test-faults")]
                {
                    self.host.elect_asked(about.0, term.0, v == Verdict::Started);
                    let code = VERDICTS.iter().position(|s| *s == verdict_str(v)).unwrap_or(0) as u64;
                    self.host.trace(Trace::Reported, about.0, term.0, u64::from(colo.0), code);
                }
                v
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
            "meta": meta(&c.m, &c.join.me.do_name),
            "config": c.m.committed_config().map(|(i, cfg)| json!({ "index": i.0, "config": cfg })),
            "boot1": c.m.digest_of(raft::Index(1)).map(|d| d.hex()),
            "family": format!("{:?}", Kind::of(&c.join)),
            "restores": self.host.stats.borrow().restores,
            // A repository member's activity feed: held, peak held, refusals, and its ring, for `place_learners`.
            "feed": match c.join.chain {
                graftyard_types::ChainId::Repo(r) => crate::feed::FEEDS.with(|f| f.borrow().get(&r).map(|f| f.borrow().status())),
                _ => None,
            },
            // The leader's admission as `capacity` sizes it, cheap enough to poll every second: LM-1's B0-31 trace
            // (does a mix of large commands shrink `cmd_bytes`'s capacity until agents wait out AGENT_WAIT).
            "admission": c.m.admission().map(|a| json!({
                "capacity": a.capacity, "roundMs": a.round_ms, "cmdBytes": a.cmd_bytes, "queued": a.queued,
                "agentQueued": a.agent_queued, "agentBytes": a.agent_bytes, "hot": a.hot, "pressure": a.pressure,
                "retryAfterS": a.retry_after_s,
            })),
            // L0-12's drain wait (`tests/fanout/drain.mjs`): the reads this member still serves, here as well as in
            // `/stats`, whose full form serialized megabytes on a loaded leader.
            "readsInFlight": self.host.reads_in_flight.get(),
            // A placement running on this member (`place_background`'s and the daily check's marker), for drivers that
            // must not change the config under it (`tests/fanout/learners.mjs`).
            "placingSince": self.placing_since(),
            // A deleted repository's purge reads both (`graftyard_git::purge`).
            "purgeable": F::purgeable(&c.m.group_fold().fold),
            "changing": self.changing(&c.m),
            "now": self.host.now(),
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

    /// The leader memory split the operator asked for on every gate run (narrow-reads-3, gate 13 on), in bytes: the
    /// fold's rows buffered until their transaction runs and the isolate's read caches; the log entries in memory;
    /// the messages held for learners' next calls; and in-flight bodies, the member calls sent and not yet ended,
    /// the read answers still streaming, and the statements queued for the next transaction.
    /// The clock guard's evidence (`crate::clock`): the lag of every stamp seen since the last reset, as counts per
    /// bucket (`lagBuckets[0]` a lag of 0, `[i]` below 2^i ms), the largest, the window the guard reads now, whether it
    /// is suspect, and the reads a leader sent to a round because it was.
    fn clock_json(&self, s: &host::Stats) -> Value {
        let c = self.host.clock.borrow();
        json!({
            "lagBuckets": s.clock_lag, "lagMax": s.clock_lag_max, "window": c.lags().collect::<Vec<_>>(),
            "suspect": c.suspect(), "leaseFallbacks": s.lease_fallbacks,
        })
    }

    fn mem_split(&self) -> Value {
        let core = self.host.core.borrow();
        let (rows, row_bytes) = core.as_ref().map_or((0, 0), |c| F::buffered(&c.m.group_fold().fold));
        let held = self.host.callers.borrow_mut().held_stats(false);
        let (stmts, stmt_bytes) = self.host.queued();
        json!({
            "fold": { "rows": rows, "rowBytes": row_bytes, "caches": crate::family::cache_bytes() },
            "log": core.as_ref().map(|c| c.m.log_bytes()),
            "held": held.iter().map(|h| h.2).sum::<usize>(),
            "inFlight": {
                "calls": self.host.call_bytes.get(),
                "callsOut": core.as_ref().map(|c| c.m.calls_out_bytes()),
                "answers": self.host.answers.borrow().bytes(),
                "stmts": stmts,
                "stmtBytes": stmt_bytes,
                "admitted": core.as_ref().map(|c| c.admitted.len()),
                // Every copy an open ticket holds: the core's queued, frozen, and decided commands
                // (`raft::Member::admitted_bytes`) and what the host kept with each ticket (`Core::admitted`).
                "admittedBytes": core.as_ref().map(|c| c.m.admitted_bytes()
                    + c.admitted.values().filter_map(|(_, _, k)| k.as_ref()).map(raft::Record::encoded_len).sum::<usize>()),
            },
        })
    }

    fn stats(&self, reset: bool, summary: bool, #[allow(unused)] high: bool) -> Result<Response> {
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
                "queueSlotMs": percentiles(&s.queue_slot_ms),
                "queuePostMs": percentiles(&s.queue_post_ms),
                "heldAtAppend": percentiles(&s.held_at_append),
                "cycleMs": percentiles(&s.cycle_ms),
                "roundMs": percentiles(&s.round_ms),
                "confirmMs": percentiles(&s.confirm_ms),
                "confirmSplit": host::confirm_split_summary(&s.confirm_split),
                "depsMs": deps,
                "appendMs": append,
                "applyMs": percentiles(&s.apply_ms),
                "depFails": s.dep_fails,
                "depHedges": { "asked": s.dep_hedges[0], "won": s.dep_hedges[1] },
                "entries": s.entries,
                "commands": s.commands,
                "inbound": s.inbound,
                "inboundMs": now_ms().saturating_sub(s.inbound_since),
                "failedCalls": s.failed_calls,
                "shedReads": s.shed_reads,
                "clock": self.clock_json(&s),
                "waitedReads": s.waited_reads,
                "dumpBuilds": crate::family::dump_builds(),
                "leaderRefused": s.leader_refused,
                "testMerges": { "shard": s.test_merges, "asked": s.test_merge_asks, "shed": s.test_merge_shed },
                // Round 7's memory instruments: the isolate's wasm memory (it only grows), the leader's queued
                // commands and agent bytes, and the commands this host holds a copy of until they resolve.
                "wasmMemMb": crate::load::wasm_mem_mb(),
                "admission": self.host.core.borrow().as_ref().and_then(|c| c.m.admission()).map(|a| json!({
                    "queued": a.queued, "agentQueued": a.agent_queued, "agentBytes": a.agent_bytes, "hot": a.hot,
                    "pressure": a.pressure,
                    // LM-1 round 14: B0-33 committed about 33 commands/s of small entries against 252 offered, so the
                    // measured capacity the token bucket refills from is read alongside the queue.
                    "capacity": a.capacity, "retryAfterS": a.retry_after_s, "cmdBytes": a.cmd_bytes,
                    "roundMs": a.round_ms,
                })),
                "admitted": self.host.core.borrow().as_ref().map(|c| c.admitted.len()),
                "admittedBytes": host::admitted_bytes_summary(&s.admitted_bytes),
                "bigTx": s.big_tx.iter().map(|(n, b, mix)| json!({ "stmts": n, "bytes": b, "mix": mix })).collect::<Vec<_>>(),
                // narrow-reads-3's leader memory: the log entries in memory, and per learner the messages held for its
                // next call, their bytes now and at most since the last reset, and its calls open.
                "logBytes": self.host.core.borrow().as_ref().map(|c| c.m.log_bytes()),
                "held": self.host.callers.borrow_mut().held_stats(reset).into_iter()
                    .map(|(s, n, b, peak, open)| json!([s.0, n, b, peak, open])).collect::<Vec<_>>(),
                "memSplit": self.mem_split(),
            });
            let mut v = v;
            v["busyPages"] = json!(s.busy_pages);
            v["readsBy"] = json!(s.reads_by);
            #[cfg(feature = "test-faults")]
            {
                v["leaseClock"] = lease_clock_pcts(&s.lease_clock);
                // The counted bytes' high-water since the last `high=1` read (`Host::mark_mem`), and now; `high=1`
                // starts the next one from now, so each 2 s sample shows the peak of the seconds since the one before.
                v["memHigh"] = json!(s.mem_high);
                // Where BF-14's arrival rows go (`?arrival=`), so a harness sees what each sample ran with.
                v["arrival"] = json!(if self.host.arrivals_mem.borrow().is_some() { "mem" } else { "sql" });
                if high {
                    s.mem_high = Default::default();
                }
            }
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
        v["readsBy"] = json!(s.reads_by);
        v["depsMs"] = json!(s.deps_ms);
        v["depFails"] = json!(s.dep_fails);
        v["depHedges"] = json!({ "asked": s.dep_hedges[0], "won": s.dep_hedges[1] });
        v["queueMs"] = json!(s.queue_ms);
        v["testMerges"] = json!({ "shard": s.test_merges, "asked": s.test_merge_asks, "shed": s.test_merge_shed });
        // learner-dumps: a learner's reads that waited for a dump rebuild, and the dumps this isolate built.
        v["waitedReads"] = json!(s.waited_reads);
        v["clock"] = self.clock_json(&s);
        // narrow-reads-3: each read-index read's grant, rebuild wait, and total ms, as p50/p99/max, and each narrow
        // answer's build ms.
        v["readStages"] = read_stage_pcts(&s.read_stages);
        v["narrowBuildMs"] = pcts(&mut s.narrow_build_ms.clone());
        v["narrowBuilds"] = json!(crate::family::narrow_builds());
        // M5-WP2: reads served by `stale()`, and this isolate's cached `Release` view.
        v["staleReads"] = json!(s.stale_reads);
        v["releaseView"] = graftyard_lookup::view::status();
        v["raftIn"] = json!(s.raft_in);
        v["placedOverNear"] = json!(s.placed_over_near);
        #[cfg(feature = "test-faults")]
        {
            v["body"] = json!(if self.host.array_body.get() { "array" } else { "stream" });
            v["arrival"] = json!(if self.host.arrivals_mem.borrow().is_some() { "mem" } else { "sql" });
            v["entrySoft"] = json!(self.host.entry_soft.get());
        }
        v["dumpBuilds"] = json!(crate::family::dump_builds());
        v["leaderRefused"] = json!(s.leader_refused);
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
        let fresh = !self.host.joined();
        if learner {
            self.store_join(&join)?;
            return joined(Response::from_json(&json!([])), fresh);
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
        joined(Response::from_json(&rtt_rows(&self.host.state, env)?), fresh)
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

    /// The daily near check (`design/DESIGN.md`, "Placement is checked by latency"), on the leader's alarm, while a
    /// client call reached this member in the last DRAIN_IDLE_MS (`near_due`): a cron, audit, sweep, or peer call
    /// keeps the alarm running but never starts a check. A leader on its group's home whose rows are a day old measures
    /// the other voters again and reads them against `NEAR_RECHECK` (`near_next`): a home over it is noted
    /// (`near_over`), and over it again at a measurement a day or more later re-places the nearest voter
    /// (`place_inner`, which still aims for `NEAR_RTT`). A home within it clears the note.
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
        if !near_due(now, self.host.client_at.get(), &rows) {
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
        let since: Option<u64> = host::rows(&sql, "SELECT v FROM me WHERE k = 'near_over'", vec![])?
            .first()
            .and_then(|r| host::text(&r[0]).ok().and_then(|t| t.parse().ok()));
        match near_next(&site.sites, &rows, p.voters(), me, since, now) {
            NearStep::Clear => {
                sql.exec("DELETE FROM me WHERE k = 'near_over'", None)?;
            }
            NearStep::Note => {
                sql.exec("INSERT OR REPLACE INTO me (k, v) VALUES ('near_over', ?)", vec![now.to_string().into()])?;
            }
            NearStep::Replace => {
                sql.exec("DELETE FROM me WHERE k = 'near_over'", None)?;
                let r = self.place_inner(graftyard_types::NEAR_RTT).await?;
                console_log!("daily near check re-placed: {r:?}");
            }
            NearStep::Wait => {}
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
                // No row yet is its own refusal: the repository's create has not committed `RepoOwner`, so the edge
                // answers its read as for a repository the caller cannot see (`graftyard_lookup::NOT_CREATED`).
                let named = repo_owner_row(&self.host.env, repo)
                    .await
                    .map_err(|e| json!({ "source": e }))?
                    .ok_or_else(|| json!({ "noRepoOwner": repo.0.0 }))?;
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

    /// A config change committed by this leader, retried while it is busy: `Ok` with the commit index. Appended only
    /// while the committed config is still the one at `based_on`, the config the caller built `cfg` from: a change
    /// built from an older read would undo the changes committed since, as a re-placement undid `/learners` on
    /// B0-32 m3fr13. The check and the append run in one turn, so no other change commits between them.
    async fn change(&self, cfg: raft::GroupConfig, based_on: u64) -> std::result::Result<u64, String> {
        let end = now_ms() + 20_000;
        let index = loop {
            unchanged_since(
                self.host.core.borrow().as_ref().and_then(|c| c.m.committed_config().map(|c| c.0.0)),
                based_on,
            )?;
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

    /// The re-placement `/replace` and `/place` share (`design/DESIGN.md`, "Install operations", Placement failures):
    /// new members on `to` at successive generations, each joined and checked by `Placement::new` with the near bound
    /// `near`, up to `tries`, while the committed config stays `Normal`; then, for the member chosen, `Degraded { lost:
    /// to }` and `Normal` back to back (`replace_steps`). `Err` when none passed, with every try and its round trips,
    /// and no config changed. With `keep_below` (the near round trip of the member being replaced, from `/place`), a
    /// try refused only by the near bound is a candidate: when none passed, the nearest goes in if it is nearer than
    /// `keep_below`, and otherwise the old member stays with an `Ok` answer, so the bound alone never fails a create.
    async fn replace_on(
        &self,
        to: SiteId,
        near: &[(graftyard_types::Region, u16)],
        tries: usize,
        fail_first: bool,
        keep_below: Option<u16>,
    ) -> Result<std::result::Result<Value, Value>> {
        let (chain, record, me, cfg, owner, genesis) = {
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
        let steps = RouteSteps {
            r: self,
            chain,
            record,
            me,
            p: p.clone(),
            to,
            others: p.voters().iter().filter(|m| m.site != to).cloned().collect(),
            site: SiteEnv::read(&self.host.env)?,
            mine: rtt_rows(&self.host.state, &self.host.env)?,
            near,
            fail_first,
            keep_below,
            owner,
            genesis,
        };
        replace_steps(&steps, chain, p, old, tries, keep_below).await
    }

    /// Each voter `Degraded { lost: to }` would need, other than this leader, by its `/status`: `Ready` when every one
    /// has applied to within `PLACE_LAG_MAX` of this leader's commit, else the ones that have not
    /// (`degraded_needs`).
    async fn voters_ready(
        &self,
        p: &graftyard_types::Placement,
        me: SiteId,
        to: SiteId,
    ) -> Result<std::result::Result<Ready, Value>> {
        let commit = self.host.core.borrow().as_ref().map_or(0, |c| c.m.state().commit.0);
        let site = SiteEnv::read(&self.host.env)?;
        let mut applied = Vec::new();
        for v in p.voters().iter().filter(|v| v.site != me && v.site != to) {
            let a = match site.member(&self.host.env, v.site, &v.do_name)?.fetch_with_str("https://member/status").await
            {
                Ok(mut r) if r.status_code() == 200 => r.json::<Value>().await.ok().and_then(|s| s["applied"].as_u64()),
                _ => None,
            };
            applied.push((v.site, a));
        }
        Ok(degraded_needs(commit, &applied))
    }

    /// Puts `kept`'s registry row back on its site once a re-placement's tries are done, over any row that still names
    /// another try (`RowPut::restore`): each try's `/join` wrote its row at a generation above `kept`'s, and a try that
    /// never went in left the edge's registry lookups calling a member outside the group (round 29's
    /// `repo:131342#9#f1`, `tests/am1/results/lm1-memory.md`, round 31). Best effort: a failed put is logged, and the
    /// lookup still finds the group through the cached config and the bootstrap names first.
    async fn restore_row(&self, chain: ChainId, record: &PlacementRecord, kept: &MemberRef, joined: &[MemberRef]) {
        let Some(put) = graftyard_types::RowPut::restore(chain, &record.sites, kept, joined) else { return };
        let r = async {
            let site = SiteEnv::read(&self.host.env)?;
            let reg = MemberRegistry::for_chain(kept.site, chain);
            let mut init = RequestInit::new();
            init.with_method(Method::Put).with_body(Some(serde_json::to_string(&put)?.into()));
            let mut res = site
                .registry(&self.host.env, kept.site, &reg.do_name())?
                .fetch_with_request(Request::new_with_init("https://registry/row", &init)?)
                .await?;
            Ok::<_, Error>((res.status_code(), res.text().await.unwrap_or_default()))
        };
        match r.await {
            Ok((200, _)) => {}
            Ok((s, t)) => console_error!("registry restore of {chain} on site {}: {s} {t}", kept.site.0),
            Err(e) => console_error!("registry restore of {chain} on site {}: {e}", kept.site.0),
        }
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

    /// `/place` with `{"background": true}`, as `graftyard_lookup::create` sends it: the same check, run under
    /// `wait_until` after the answer, so a create returns once its group leads instead of after up to `PLACE_TRIES`
    /// joins of about 7 s each (`decisions/DECISIONS.md`, "`fresh-repo`"). It answers `notLeader` at once, so the
    /// caller asks the member that leads, and `{"placing": true}` otherwise. It holds the daily check's marker
    /// (`near_check`), so two placements of one group never run at once; a second ask while one runs only answers.
    fn place_background(&self, near: Option<Vec<(graftyard_types::Region, u16)>>) -> Result<Response> {
        let (chain, nl) = {
            let core = self.host.core.borrow();
            let c = core.as_ref().ok_or_else(|| err("member not joined"))?;
            (c.join.chain, (c.m.role() != Role::Leader).then(|| host::nl_json(&c.m.not_leader())))
        };
        if let Some(nl) = nl {
            return self.reply(json!({ "error": { "notLeader": nl } }));
        }
        let sql = self.host.state.storage().sql();
        let now = now_ms();
        let busy: Option<u64> = host::rows(&sql, "SELECT v FROM me WHERE k = 'near_check'", vec![])?
            .first()
            .and_then(|r| host::text(&r[0]).ok().and_then(|t| t.parse().ok()));
        if busy.is_some_and(|t| now.saturating_sub(t) < NEAR_CHECK_FOR) {
            return self.reply(json!({ "placing": true, "since": busy }));
        }
        sql.exec("INSERT OR REPLACE INTO me (k, v) VALUES ('near_check', ?)", vec![now.to_string().into()])?;
        let me = Routes { host: self.host.clone() };
        self.host.state.wait_until(async move {
            let r = me.place_inner(near.as_deref().unwrap_or(graftyard_types::NEAR_RTT)).await;
            match &r {
                Ok(Ok(v)) => console_log!("placement of {chain}: {v}"),
                Ok(Err(v)) => console_error!("placement of {chain} failed: {v}"),
                Err(e) => console_error!("placement of {chain} failed: {e}"),
            }
            if let Err(e) = me.host.state.storage().sql().exec("DELETE FROM me WHERE k = 'near_check'", None) {
                console_error!("placement of {chain}: marker: {e}");
            }
        });
        self.reply(json!({ "placing": true }))
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
                c.m.committed_config().cloned(),
                c.join.owner,
                c.join.genesis.clone(),
            )
        };
        let Some((based_on, cfg)) = cfg else { return self.reply(json!({ "error": "no committed config" })) };
        let based_on = based_on.0;
        let step = next_step(&cfg, me, &to);
        let label = format!("{step:?}");
        let t0 = now_ms();
        let site = SiteEnv::read(&self.host.env)?;
        let checked = |voters: arrayvec::ArrayVec<MemberRef, 3>, home: SiteId, rtt: &[(SiteId, Rtt)]| {
            let checks =
                graftyard_types::PlacementChecks { sites: &site.sites, separate_accounts: false, rtt, near: &[] };
            move_placement(&cfg, voters, home, &checks)
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
                        .change(raft::GroupConfig::Joint { old: p.clone(), new: Box::new(np) }, based_on)
                        .await
                        .map(|i| json!({ "configIndex": i, "joined": new.do_name })),
                    Err(e) => Err(e),
                }
            }
            (MoveStep::Leave, raft::GroupConfig::Joint { new, .. }) => {
                self.change(raft::GroupConfig::Normal(new.clone()), based_on).await.map(|i| json!({ "configIndex": i }))
            }
            (MoveStep::Home { home }, raft::GroupConfig::Normal(p)) => match checked(voters_of(p), home, &rtt) {
                Ok(np) => self
                    .change(raft::GroupConfig::Normal(Box::new(np)), based_on)
                    .await
                    .map(|i| json!({ "configIndex": i })),
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
                            .change(raft::GroupConfig::Normal(Box::new(np)), based_on)
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
            (c.join.chain, c.join.record.clone(), c.m.committed_config().cloned(), c.join.owner, c.join.genesis.clone())
        };
        let Some((based_on, raft::GroupConfig::Normal(p))) = cfg else {
            return self.reply(json!({ "error": "the committed config is not Normal" }));
        };
        let based_on = based_on.0;
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
        let np = match p.next(voters, learners, p.home(), &checks) {
            Ok(np) => np,
            Err(e) => return self.reply(json!({ "error": e.to_string(), "joined": joined })),
        };
        // A voter re-placement that commits while the learners join makes this change stale: it would undo the
        // re-placement. Refused, and the caller asks again.
        match self.change(raft::GroupConfig::Normal(Box::new(np)), based_on).await {
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
    repo_owner_row(env, repo).await?.ok_or_else(|| format!("{repo:?} has no RepoOwner row"))
}

/// `repo_owner`, with `None` only when the `Names` leader answered and holds no row; a read that failed is `Err`.
async fn repo_owner_row(env: &Env, repo: graftyard_types::RepoId) -> std::result::Result<Option<AccountId>, String> {
    let chain = ChainId::Names(graftyard_identity::name_shard(&graftyard_identity::NameKey::Repo(repo)));
    let record = crate::release::system_record(env, chain).await?;
    let (v, _) = lookup::call(env, chain, &record, "/read", &json!({ "q": { "repoOwner": repo.0.0 } })).await?;
    serde_json::from_value::<Option<AccountId>>(v.pointer("/value/repoOwner").cloned().unwrap_or(Value::Null))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    /// `b032-shed` A: only a read-index read failing `NotLeader` naming none, on a member that forgot its leader, is
    /// shed with the report; one naming a leader, one on a member that forgot none, and a plain read stay `notLeader`.
    #[test]
    fn a_read_index_read_on_a_member_that_forgot_its_leader_is_shed_with_the_report() {
        use graftyard_types::SiteId;
        let none = graftyard_raft::NotLeader { term: graftyard_raft::Term(4), leader: None, config: None };
        let named = graftyard_raft::NotLeader { leader: Some(SiteId(2)), ..none.clone() };
        let shed = super::not_leader_reply(true, &none, Some(SiteId(0)));
        assert!(shed.pointer("/overloaded/retryAfterMs").is_some(), "{shed}");
        assert_eq!(shed["forgot"], serde_json::json!({ "site": 0, "term": 4 }));
        for (index, nl, forgot) in
            [(true, &named, Some(SiteId(0))), (true, &none, None), (false, &none, Some(SiteId(0)))]
        {
            let v = super::not_leader_reply(index, nl, forgot);
            assert!(v.pointer("/error/notLeader").is_some() && v.get("overloaded").is_none(), "{v}");
        }
    }

    use graftyard_types::SiteId;
    use serde_json::json;

    use super::{
        DAY_MS, Gate, HEAVY_READS, InFlight, NearStep, READS_MAX, gate, heavy_read, near_due, near_next, percentiles,
        read_kind, read_wait, site_field,
    };
    use crate::host;

    /// The daily near check moves a member only on readings past the pair's own spread, twice a day apart: an enam
    /// home's weur follower at 131 ms, the top of what re-placed healthy groups on 2026-11-16 read, never moves; one at
    /// 151 ms is noted, waits under a day, and moves at a day; 150 itself is within; an apac home takes its own bound,
    /// and a nearer far voter decides. The check starts only after a client call in the last minute.
    #[test]
    fn the_daily_near_check_moves_only_past_the_spread_and_only_after_a_client_call() {
        use graftyard_types::{
            ChainId, DbId, Member, MemberKind, Region, RepoId, Rtt, Site, SiteLink, SiteTable, Timestamp,
        };
        let site = |id: u8, region| Site {
            id: SiteId(id),
            script: String::new(),
            account: String::new(),
            region,
            jurisdiction: None,
            bucket: String::new(),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare: false,
        };
        let sites = SiteTable(vec![site(0, Region::Enam), site(1, Region::Weur), site(2, Region::Apac)]);
        let chain = ChainId::Repo(RepoId(DbId(65549)));
        let voters: Vec<Member> = (0..3).map(|s| Member::new(chain, SiteId(s), MemberKind::Full, s, 1)).collect();
        let now = 10 * DAY_MS;
        let rows = |home: u8, a: (u8, u16), b: (u8, u16)| {
            [a, b]
                .map(|(peer, ms)| (SiteId(home), Rtt { peer: SiteId(peer), median_ms: ms, at: Timestamp(now) }))
                .to_vec()
        };
        let step = |r: &[(SiteId, Rtt)], home: u8, since| near_next(&sites, r, &voters, SiteId(home), since, now);

        let healthy = rows(0, (1, 131), (2, 198));
        assert_eq!(step(&healthy, 0, None), NearStep::Clear);
        assert_eq!(step(&healthy, 0, Some(now - DAY_MS)), NearStep::Clear, "a note clears once within the spread");
        assert_eq!(step(&rows(0, (1, 150), (2, 198)), 0, Some(now - DAY_MS)), NearStep::Clear);
        let far = rows(0, (1, 151), (2, 198));
        assert_eq!(step(&far, 0, None), NearStep::Note);
        assert_eq!(step(&far, 0, Some(now - DAY_MS + 1)), NearStep::Wait);
        assert_eq!(step(&far, 0, Some(now - DAY_MS)), NearStep::Replace);
        // A nearer voter on the far site still counts as near.
        assert_eq!(step(&rows(0, (1, 151), (2, 140)), 0, Some(now - DAY_MS)), NearStep::Clear);
        // An apac home: 260 is inside B0-12's p99 enam to apac pair, 298 is past half again over 198.
        assert_eq!(step(&rows(2, (0, 260), (1, 300)), 2, Some(now - DAY_MS)), NearStep::Clear);
        assert_eq!(step(&rows(2, (0, 298), (1, 300)), 2, Some(now - DAY_MS)), NearStep::Replace);

        let old = rows(0, (1, 100), (2, 198))
            .into_iter()
            .map(|(s, r)| (s, Rtt { at: Timestamp(0), ..r }))
            .collect::<Vec<_>>();
        assert!(near_due(now, now - 1_000, &old), "a client call a second ago, rows a day old");
        assert!(near_due(now, now - 1_000, &[]), "no rows yet");
        assert!(!near_due(now, 0, &old), "only internal calls since the isolate started");
        assert!(!near_due(now, now - crate::host::DRAIN_IDLE_MS, &old), "the last client call a minute ago");
        assert!(!near_due(now, now - 1_000, &healthy), "rows measured today");
    }

    /// A member is removed once its latest committed config no longer names it: a replaced voter (B0-32 m3fr2's
    /// `#1#f1` after its site's `#2#f1` joined), and a `Degraded` config's lost voter. Its replacement, the other
    /// voters, a learner, and a member with no config yet are not.
    #[test]
    fn a_member_its_config_no_longer_names_is_removed() {
        use graftyard_raft::GroupConfig;
        use graftyard_types::{ChainId, DbId, Member, MemberKind, RepoId};
        let chain = ChainId::Repo(RepoId(DbId(65556)));
        let m = |s: u8, g: u32| Member::new(chain, SiteId(s), MemberKind::Full, s, g);
        let l = Member::new(chain, SiteId(2), MemberKind::Learner, 0, 3);
        let p = |v1: u32, ls: Vec<Member>| -> Box<graftyard_types::Placement> {
            serde_json::from_value(
                json!({ "voters": [m(0, 1), m(1, v1), m(2, 1)], "learners": ls, "home": 0, "generation": 3 }),
            )
            .unwrap()
        };
        let normal = GroupConfig::Normal(p(2, vec![l.clone()]));
        let degraded = GroupConfig::Degraded { placement: p(1, vec![]), lost: SiteId(1) };
        let joint = GroupConfig::Joint { old: p(1, vec![]), new: p(2, vec![]) };
        let removed = |c: Option<&GroupConfig>, me: &Member| super::removed(c, &me.do_name);
        assert!(removed(Some(&normal), &m(1, 1)), "the replaced voter");
        assert!(!removed(Some(&normal), &m(1, 2)), "its replacement");
        assert!(!removed(Some(&normal), &m(0, 1)));
        assert!(!removed(Some(&normal), &l), "a learner the config names");
        assert!(removed(Some(&degraded), &m(1, 1)), "the lost site's voter");
        assert!(!removed(Some(&degraded), &m(2, 1)));
        assert!(!removed(Some(&joint), &m(1, 1)) && !removed(Some(&joint), &m(1, 2)), "both halves of a Joint");
        assert!(!removed(None, &m(1, 1)), "no config yet");
    }

    /// G22's fake leader: records every config change and try, in order, and answers each try from `results` and the
    /// two `ready` asks from `ready`.
    mod place {
        use std::cell::RefCell;

        use graftyard_raft::GroupConfig;
        use graftyard_types::{ChainId, DbId, Member, MemberKind, Placement, RepoId, Rtt, SiteId, Timestamp};
        use serde_json::{Value, json};

        use super::super::{Ready, Steps, TryOut, degraded_needs, replace_steps};

        pub const CHAIN: ChainId = ChainId::Repo(RepoId(DbId(1048593)));

        /// How a try answers: it passes, only the near bound refused it at `ms`, or it fails outright.
        #[derive(Clone, Copy)]
        pub enum Try {
            Pass,
            Over(u16),
            Fail,
        }

        pub fn m(site: u8, generation: u32) -> Member {
            Member::new(CHAIN, SiteId(site), MemberKind::Full, site, generation)
        }

        pub fn placement(f1: u32) -> Placement {
            with(f1, &[])
        }

        /// `n` learners, one per slot, spread over the three sites as B0-32's 8 are.
        pub fn learners(n: u8) -> Vec<Member> {
            (0..n).map(|k| Member::new(CHAIN, SiteId(k % 3), MemberKind::Learner, k, 1)).collect()
        }

        /// Site 1's voter at generation `f1`, with `learners`.
        pub fn with(f1: u32, learners: &[Member]) -> Placement {
            serde_json::from_value(
                json!({ "voters": [m(0, 1), m(1, f1), m(2, 1)], "learners": learners, "home": 0, "generation": f1 }),
            )
            .unwrap()
        }

        #[derive(Default)]
        pub struct Fake {
            pub results: Vec<Try>,
            /// Each `ready` ask in turn: `true` when every voter is caught up.
            pub ready: Vec<bool>,
            pub log: RefCell<Vec<String>>,
            pub asked: RefCell<usize>,
            pub tried: RefCell<usize>,
            /// The committed config and its index; `run` starts it at index 5 with the placement it replaces from.
            pub committed: RefCell<Option<(u64, GroupConfig)>>,
            /// A change another route commits during the tries, at the next index, as the second `ready` is asked:
            /// B0-32 m3fr13's `/learners`.
            pub then: Option<GroupConfig>,
            /// The learners each committed `Degraded` and `Normal` named, in order.
            pub kept: RefCell<Vec<usize>>,
        }

        impl Fake {
            pub fn log(&self) -> Vec<String> {
                self.log.borrow().clone()
            }
        }

        impl Steps for Fake {
            async fn ready(&self) -> worker::Result<std::result::Result<Ready, Value>> {
                let n = *self.asked.borrow();
                *self.asked.borrow_mut() += 1;
                self.log.borrow_mut().push("ready".into());
                if let (1, Some(c)) = (n, &self.then) {
                    let mut at = self.committed.borrow_mut();
                    let i = at.as_ref().map_or(0, |a| a.0) + 1;
                    *at = Some((i, c.clone()));
                }
                let applied = if self.ready.get(n).copied().unwrap_or(true) { 1_000 } else { 10 };
                Ok(degraded_needs(1_000, &[(SiteId(2), Some(applied))]))
            }
            async fn try_member(&self, new: &Member, _first: bool) -> worker::Result<TryOut> {
                let n = *self.tried.borrow();
                *self.tried.borrow_mut() += 1;
                self.log.borrow_mut().push(format!("try {}", new.do_name));
                let p = placement(new.generation);
                let fresh = Some(Rtt { peer: SiteId(1), median_ms: 100, at: Timestamp(0) });
                let (passed, over) = match self.results[n] {
                    Try::Pass => (Some(p), None),
                    Try::Over(ms) => (None, Some((p, ms))),
                    Try::Fail => (None, None),
                };
                Ok(TryOut { passed, over, fresh, row: json!({ "member": new.do_name }) })
            }
            fn committed(&self) -> Option<(u64, GroupConfig)> {
                self.committed.borrow().clone()
            }
            /// Refuses a change built from a config older than the committed one, as `Routes::change` does.
            async fn change(&self, cfg: GroupConfig, based_on: u64) -> std::result::Result<u64, String> {
                let at = self.committed.borrow().as_ref().map(|c| c.0);
                super::super::unchanged_since(at, based_on)?;
                let (s, p) = match &cfg {
                    GroupConfig::Normal(p) => (format!("normal {}", p.voters()[1].do_name), p),
                    GroupConfig::Degraded { lost, placement } => (format!("degraded {}", lost.0), placement),
                    GroupConfig::Joint { new, .. } => ("joint".into(), new),
                };
                self.kept.borrow_mut().push(p.learners().len());
                self.log.borrow_mut().push(s);
                let i = at.unwrap_or(0) + 1;
                *self.committed.borrow_mut() = Some((i, cfg));
                Ok(i)
            }
            async fn clear(&self, m: &Member) {
                self.log.borrow_mut().push(format!("clear {}", m.do_name));
            }
            fn store_rtt(&self, _r: Rtt) -> worker::Result<()> {
                Ok(())
            }
            async fn restore_row(&self, kept: &Member, _joined: &[Member]) {
                self.log.borrow_mut().push(format!("row {}", kept.do_name));
            }
            fn placed_over_near(&self) {}
            fn now(&self) -> u64 {
                0
            }
            fn note(&self, _error: bool, _m: String) {}
        }

        /// Runs `replace_steps` on the fake to its end: site 1's `#1#f1` replaced, up to `tries`, with `keep_below`.
        pub fn run(f: &Fake, tries: usize, keep_below: Option<u16>) -> std::result::Result<Value, Value> {
            let start = f
                .committed
                .borrow_mut()
                .get_or_insert_with(|| (5, GroupConfig::Normal(Box::new(placement(1)))))
                .clone();
            let GroupConfig::Normal(p) = start.1 else { panic!("a re-placement starts from Normal") };
            let fut = replace_steps(f, CHAIN, p, m(1, 1), tries, keep_below);
            let mut fut = std::pin::pin!(fut);
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            match fut.as_mut().poll(&mut cx) {
                std::task::Poll::Ready(r) => r.unwrap(),
                std::task::Poll::Pending => panic!("the fake never waits"),
            }
        }

        use std::future::Future as _;
    }

    fn names(log: &[String]) -> Vec<&str> {
        log.iter().map(String::as_str).collect()
    }

    /// G22: b030r5's create placement committed `Degraded { lost: 1 }` before its first try and held a just-built group
    /// there for 535 s and 8 tries. Every try now runs under `Normal`, and `Degraded` then `Normal` commit back to back
    /// for the member chosen, with `ready` asked before the first try and again before `Degraded`.
    #[test]
    fn a_re_placement_commits_no_config_until_a_try_is_chosen() {
        use place::{Fake, Try};
        let f = Fake { results: vec![Try::Fail, Try::Fail, Try::Pass], ..Fake::default() };
        let v = place::run(&f, 8, None).unwrap();
        assert_eq!(
            names(&f.log()),
            [
                "ready",
                "try repo:1048593#2#f1",
                "clear repo:1048593#2#f1",
                "try repo:1048593#3#f1",
                "clear repo:1048593#3#f1",
                "try repo:1048593#4#f1",
                "ready",
                "degraded 1",
                "normal repo:1048593#4#f1",
                "row repo:1048593#4#f1",
            ]
        );
        assert_eq!(v["old"], "repo:1048593#1#f1", "the answer names the member it replaced");
    }

    /// No try nearer than the member it would replace: the old member stays and no config is appended at all, and the
    /// site's registry row goes back to it.
    #[test]
    fn no_nearer_try_changes_no_config() {
        use place::{Fake, Try};
        let f = Fake { results: vec![Try::Over(120), Try::Over(110), Try::Fail], ..Fake::default() };
        let v = place::run(&f, 3, Some(104)).unwrap();
        let log = f.log();
        assert!(!log.iter().any(|l| l.starts_with("degraded") || l.starts_with("normal")), "{log:?}");
        assert_eq!(log.last().map(String::as_str), Some("row repo:1048593#1#f1"));
        assert_eq!(v["kept"], "repo:1048593#1#f1");
        let f = Fake { results: vec![Try::Fail, Try::Fail], ..Fake::default() };
        assert!(place::run(&f, 2, None).is_err(), "no try passed and no bound to keep under");
        assert!(!f.log().iter().any(|l| l.starts_with("degraded")));
    }

    /// The operator's condition: a voter that falls behind during the tries is caught by the second `ready`, and the
    /// re-placement ends with no config change and the chosen try cleared; one behind at the first ask tries nothing.
    #[test]
    fn a_voter_behind_at_the_commit_ends_it_with_no_config() {
        use place::{Fake, Try};
        let f = Fake { results: vec![Try::Pass], ready: vec![true, false], ..Fake::default() };
        let e = place::run(&f, 8, None).unwrap_err();
        assert_eq!(e["error"], "a voter the degraded config needs is behind");
        assert_eq!(
            names(&f.log()),
            ["ready", "try repo:1048593#2#f1", "ready", "clear repo:1048593#2#f1", "row repo:1048593#1#f1"]
        );
        let f = Fake { results: vec![Try::Pass], ready: vec![false], ..Fake::default() };
        assert!(place::run(&f, 8, None).is_err());
        assert_eq!(names(&f.log()), ["ready"], "behind before the first try: nothing tried");
    }

    /// With every try over the bound, the nearest that beats the current member goes in (b030r5: `#2#f1` at 104 ms
    /// against 143), the others are cleared, and only then do `Degraded` and `Normal` commit.
    #[test]
    fn the_nearest_try_goes_in_when_none_passes() {
        use place::{Fake, Try};
        let f = Fake { results: vec![Try::Over(110), Try::Over(104), Try::Over(122)], ..Fake::default() };
        let v = place::run(&f, 3, Some(143)).unwrap();
        let log = f.log();
        let at = |s: &str| log.iter().position(|l| l == s).unwrap_or_else(|| panic!("{s} in {log:?}"));
        assert!(at("try repo:1048593#4#f1") < at("degraded 1"), "every try before Degraded");
        assert_eq!(at("normal repo:1048593#3#f1"), at("degraded 1") + 1, "back to back, the nearest");
        assert!(at("clear repo:1048593#2#f1") < at("degraded 1") && at("clear repo:1048593#4#f1") < at("degraded 1"));
        assert!(!log.contains(&"clear repo:1048593#3#f1".to_owned()));
        assert_eq!(v["nearOver"], 104);
    }

    /// `learners-kept`, B0-32 m3fr13: a re-placement of a group with 8 learners commits `Degraded` and `Normal` with
    /// all 8, though its tries' placements name none; a `/learners` change committed during the tries (2 more, same
    /// voters) goes into both; voters that moved during them end it with no config change and the try cleared.
    #[test]
    fn a_re_placement_keeps_the_committed_learners() {
        use graftyard_raft::GroupConfig;
        use place::{Fake, Try, learners, with};
        let normal = |n: u8| Some((5, GroupConfig::Normal(Box::new(with(1, &learners(n))))));
        let f = Fake { results: vec![Try::Pass], committed: normal(8).into(), ..Fake::default() };
        place::run(&f, 8, None).unwrap();
        assert_eq!(*f.kept.borrow(), [8, 8], "{:?}", f.log());

        let f = Fake {
            results: vec![Try::Fail, Try::Pass],
            committed: normal(8).into(),
            then: Some(GroupConfig::Normal(Box::new(with(1, &learners(10))))),
            ..Fake::default()
        };
        let v = place::run(&f, 8, None).unwrap();
        assert_eq!(*f.kept.borrow(), [10, 10], "the /learners commit is not lost: {:?}", f.log());
        assert_eq!(v["configIndex"], 8, "Degraded at 7 on the /learners commit at 6, then Normal");

        let f = Fake {
            results: vec![Try::Pass],
            committed: normal(8).into(),
            then: Some(GroupConfig::Normal(Box::new(with(9, &learners(8))))),
            ..Fake::default()
        };
        let e = place::run(&f, 8, None).unwrap_err();
        assert_eq!(e["error"], "the committed config changed during the re-placement");
        assert_eq!(e["configIndex"], 6);
        assert_eq!(
            names(&f.log()),
            ["ready", "try repo:1048593#2#f1", "ready", "clear repo:1048593#2#f1", "row repo:1048593#1#f1"]
        );
    }

    /// `learners-kept`: a `/move` step's placement keeps the learners of the config it was planned from (a `Joint`'s
    /// new half), and a change is refused once the committed config has moved past the one it was built from.
    #[test]
    fn a_move_keeps_the_committed_learners() {
        use arrayvec::ArrayVec;
        use graftyard_raft::GroupConfig;
        use graftyard_types::{Region, Rtt, Site, SiteLink, SiteTable, Timestamp};
        use place::{learners, m, with};
        let site = |id: u8, region| Site {
            id: SiteId(id),
            script: String::new(),
            account: String::new(),
            region,
            jurisdiction: None,
            bucket: String::new(),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare: false,
        };
        let sites = SiteTable(vec![site(0, Region::Enam), site(1, Region::Weur), site(2, Region::Apac)]);
        let rtt: Vec<(SiteId, Rtt)> = [(0, 1, 80), (0, 2, 180), (1, 2, 200)]
            .map(|(a, b, ms)| (SiteId(a), Rtt { peer: SiteId(b), median_ms: ms, at: Timestamp(0) }))
            .to_vec();
        let checks = graftyard_types::PlacementChecks { sites: &sites, separate_accounts: false, rtt: &rtt, near: &[] };
        let voters: ArrayVec<_, 3> = [m(0, 1), m(1, 2), m(2, 1)].into_iter().collect();
        let normal = GroupConfig::Normal(Box::new(with(1, &learners(8))));
        let np = super::move_placement(&normal, voters.clone(), SiteId(2), &checks).unwrap();
        assert_eq!((np.learners(), np.home()), (&learners(8)[..], SiteId(2)), "a rehome keeps all 8");
        let joint = GroupConfig::Joint { old: Box::new(with(1, &[])), new: Box::new(with(2, &learners(3))) };
        assert_eq!(super::move_placement(&joint, voters, SiteId(0), &checks).unwrap().learners(), &learners(3)[..]);
        assert!(super::unchanged_since(Some(5), 5).is_ok());
        assert!(super::unchanged_since(Some(6), 5).is_err(), "a /learners commit since the read");
        assert!(super::unchanged_since(None, 5).is_err());
    }

    /// (ii)(a): a re-placement starts only when every voter its `Degraded` config needs is within PLACE_LAG_MAX of the
    /// leader's commit. B0-30's case: site c installing at 199,808 against commit 208,159; a voter whose status did not
    /// answer is not taken as caught up; the boundary is inclusive; the member being replaced and the leader are not
    /// asked, so they are not in `applied`.
    #[test]
    fn a_degraded_config_waits_for_every_voter_it_needs() {
        let (c, commit) = (SiteId(2), 208_159);
        assert!(super::degraded_needs(commit, &[(c, Some(199_808))]).is_err(), "B0-30: site c installing");
        assert!(super::degraded_needs(commit, &[(c, None)]).is_err(), "no answer is not caught up");
        assert!(super::degraded_needs(commit, &[(c, Some(commit - super::PLACE_LAG_MAX))]).is_ok(), "at the bound");
        assert!(super::degraded_needs(commit, &[(c, Some(commit - super::PLACE_LAG_MAX - 1))]).is_err());
        assert!(super::degraded_needs(commit, &[(c, Some(commit + 3))]).is_ok(), "ahead of the leader's commit");
        assert!(super::degraded_needs(commit, &[]).is_ok(), "a two-voter group needs only the leader");
        let two = [(SiteId(1), Some(commit)), (c, Some(150_000))];
        let behind = super::degraded_needs(commit, &two).err().unwrap();
        assert_eq!(behind, json!([{ "site": 2, "applied": 150_000, "commit": commit }]), "names only the one behind");
    }

    /// (i-3): a member whose install dropped its metadata tables refuses a leader's transfer back to that leader, with
    /// the transfer's own term and id, and steps nothing else differently; a member that may lead takes it.
    #[test]
    fn a_member_that_may_not_lead_refuses_a_transfer() {
        use graftyard_raft::{Envelope, Message, Term, TransferId};
        let (leader, me) = (SiteId(0), SiteId(2));
        let t = Envelope::<()> {
            from: leader,
            to: me,
            msg: Message::TimeoutNow { term: Term(155), transfer: TransferId(7) },
        };
        let no = super::refuse_lead(false, &t).expect("refused");
        assert_eq!((no.from, no.to), (me, leader));
        assert!(matches!(no.msg, Message::TransferRefused { term: Term(155), transfer: TransferId(7) }));
        assert!(super::refuse_lead(true, &t).is_none(), "a member that may lead takes the transfer");
        let ping = Envelope::<()> { from: leader, to: me, msg: Message::Ping { probe: 1, n: 1 } };
        assert!(super::refuse_lead(false, &ping).is_none(), "only a transfer is refused");
    }

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
        // `readsBy` names a dump `rows` and a narrow read by its selector, whatever other keys sort before `rows`.
        for (q, kind) in [
            (json!({ "rows": true, "ranges": true, "packed": true }), "rows"),
            (json!({ "packed": true, "rows": { "merge": 7 } }), "rows.merge"),
            (json!({ "at": 9, "rows": { "api": { "list": "pulls" } } }), "rows.api"),
            (json!({ "refs": { "limit": 10 } }), "refs"),
        ] {
            assert_eq!(read_kind(&q), kind, "{q}");
        }
        // (index, leading, servable, heavy, reads, heavies)
        assert_eq!(gate(false, true, V, false, true, HEAVY_READS, HEAVY_READS - 1, 0, false), Gate::Serve);
        assert_eq!(gate(false, true, V, false, true, HEAVY_READS, HEAVY_READS, 0, false), Gate::Shed);
        assert_eq!(
            gate(false, false, V, true, true, HEAVY_READS, HEAVY_READS, 0, false),
            Gate::Serve,
            "a plain follower read answers NotLeader"
        );
        assert_eq!(gate(false, true, V, false, false, HEAVY_READS * 4, HEAVY_READS * 4, 0, false), Gate::Serve);
        assert_eq!(gate(false, true, V, false, false, READS_MAX - 1, 0, 0, false), Gate::Serve);
        assert_eq!(
            gate(false, true, V, false, false, READS_MAX, 0, 0, false),
            Gate::Shed,
            "a narrow read past READS_MAX"
        );
        assert_eq!(gate(false, false, V, false, false, READS_MAX, 0, 0, false), Gate::Shed, "on a follower too");
        let n = std::rc::Rc::new(std::cell::Cell::new(0));
        let held: Vec<_> = (0..READS_MAX).map(|_| InFlight::new(&n)).collect();
        assert_eq!(gate(false, true, V, false, false, n.get(), 0, 0, false), Gate::Shed);
        drop(held);
        assert_eq!(n.get(), 0);
    }

    /// A follower serves a read-index read only for the queries it may serve, under the same caps as the leader; a
    /// read it may not serve (a leader-only query, or tiers it is still loading) names the leader, before any cap, so
    /// the edge goes to the leader and does not retry the follower. One that waited FOLLOWER_WAIT without applying the
    /// leader's index answers `overloaded`, never a value and never a 5xx the edge would report with `suspect`.
    #[test]
    fn a_follower_serves_only_its_reads_under_the_caps_and_never_stale() {
        assert_eq!(gate(true, false, V, true, true, 0, 0, 0, false), Gate::Serve);
        assert_eq!(gate(true, false, V, false, false, 0, 0, 0, false), Gate::NotLeader);
        assert_eq!(gate(true, false, V, false, true, READS_MAX, HEAVY_READS, 0, false), Gate::NotLeader);
        assert_eq!(gate(true, false, V, true, true, 0, HEAVY_READS, 0, false), Gate::Shed, "HEAVY_READS on a follower");
        assert_eq!(gate(true, false, V, true, false, READS_MAX, 0, 0, false), Gate::Shed, "READS_MAX on a follower");
        assert_eq!(
            gate(true, true, V, false, false, 0, 0, 0, false),
            Gate::Serve,
            "a leader serves every read-index read"
        );
        // `tier-off-leader`: an acting leader refuses a whole-tier or `ls-refs` read-index read, naming itself, at no
        // load and whatever its caps; a follower and a learner still serve the same read; the leader's own plain read
        // and a Solo group's leader (`off_leader` false) are untouched.
        assert_eq!(
            gate(true, true, V, true, true, 0, 0, 0, true),
            Gate::NotLeader,
            "a leader refuses a dump read-index"
        );
        assert_eq!(gate(true, true, V, true, false, 0, 0, 0, true), Gate::NotLeader, "and an ls-refs");
        assert_eq!(gate(true, false, V, true, true, 0, 0, 0, true), Gate::Serve, "a follower serves it");
        assert_eq!(gate(true, false, L, true, true, 0, HEAVY_READS, 0, true), Gate::Serve, "a learner serves it");
        assert_eq!(gate(false, true, V, false, true, 0, 0, 0, true), Gate::Serve, "a plain read on the leader");
        assert_eq!(gate(true, true, V, true, true, 0, 0, 0, false), Gate::Serve, "a Solo leader serves it");
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
        assert_eq!(
            gate(true, false, L, true, true, 0, HEAVY_READS, 0, false),
            Gate::Serve,
            "a learner past HEAVY_READS"
        );
        assert_eq!(gate(true, false, L, true, true, 0, HEAVY_READS * 8, 0, false), Gate::Serve);
        assert_eq!(gate(true, false, L, true, true, READS_MAX, 0, 0, false), Gate::Shed, "READS_MAX on a learner");
        assert_eq!(gate(true, false, L, true, false, READS_MAX, 0, 0, false), Gate::Shed);
        assert_eq!(
            gate(true, false, V, true, true, 0, HEAVY_READS, 0, false),
            Gate::Shed,
            "a follower keeps HEAVY_READS"
        );
        assert_eq!(
            gate(false, true, V, false, true, 0, HEAVY_READS, 0, false),
            Gate::Shed,
            "the leader keeps HEAVY_READS"
        );
        assert_eq!(gate(true, false, L, false, true, 0, 0, 0, false), Gate::NotLeader, "a learner's leader-only read");
        assert_eq!(
            gate(false, false, L, true, true, 0, HEAVY_READS, 0, false),
            Gate::Serve,
            "a plain read answers NotLeader"
        );
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

    /// ANSWER_BYTES_MAX counts the distinct bytes of answers still streaming: one answer shared by many reads counts
    /// once, so a learner streaming one dump to READS_MAX - 1 readers is not shed, while distinct answers past it shed
    /// every read, voter or learner, until enough of them finish. Counted per read instead, 20 reads of one 1 MB dump
    /// would shed at 16; uncapped, a new leader held 22.9 MB of answers a second after it won and reset 18 s later.
    #[test]
    fn distinct_answer_bytes_past_the_cap_shed_and_a_shared_answer_counts_once() {
        use super::ANSWER_BYTES_MAX;
        let answers = std::rc::Rc::new(std::cell::RefCell::new(host::Answers::default()));
        let dump: std::rc::Rc<[u8]> = vec![7u8; 1_000_000].into();
        let shared: Vec<_> = (0..40).map(|_| host::Answers::hold(&answers, &dump)).collect();
        assert_eq!(answers.borrow().bytes(), 1_000_000, "one dump, however many reads stream it");
        let at = |a: usize| gate(true, false, L, true, true, 40, 40, a, false);
        assert_eq!(at(answers.borrow().bytes()), Gate::Serve);
        let distinct: Vec<std::rc::Rc<[u8]>> = (0..15).map(|i| vec![i as u8; 1_000_000].into()).collect();
        let held: Vec<_> = distinct.iter().map(|d| host::Answers::hold(&answers, d)).collect();
        assert_eq!(answers.borrow().bytes(), ANSWER_BYTES_MAX);
        assert_eq!(at(answers.borrow().bytes()), Gate::Shed, "16 MB of distinct answers sheds a learner's read");
        assert_eq!(
            gate(false, true, V, false, false, 0, 0, answers.borrow().bytes(), false),
            Gate::Shed,
            "and the leader's"
        );
        drop(held);
        assert_eq!(at(answers.borrow().bytes()), Gate::Serve, "the cap lifts as answers finish");
        drop(shared);
        assert_eq!(answers.borrow().bytes(), 0);
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
