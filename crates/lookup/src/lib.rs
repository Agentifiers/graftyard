//! The group lookup rule over Durable Object bindings (`design/DESIGN.md`, Shape, "Sites and placement",
//! `ReplyMeta`), for every Worker that calls a group: a site member (a dependency read, an id-block lease, the
//! bootstrap check of an owner's record), the git edge, and the release endpoint.
//!
//! 1. The config cached from that group's last `NotLeader`, leader first.
//! 2. Without one, the bootstrap voter names built from the chain's record, in record order.
//! 3. If none answers with a config, the chain's row in the `MemberRegistry` of each record site.
//! 4. Then both steps again for each `moving_from` record.
//!
//! A member that answers `NotLeader` with a config teaches the caller the group's voters and leader. A caller reaches
//! site `n`'s classes through its `MEMBER_<n>` and `REGISTRY_<n>` bindings, with the site's location hint.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use futures_util::future::{Either, select};
use graftyard_raft::{GroupConfig, RequestId, Ulid};
use graftyard_types::{ChainId, MemberRegistry, PlacementRecord, RegistryRow, SiteId, SiteTable, Timestamp};
use serde_json::{Value, json};
use worker::*;

pub mod view;

/// How long one lookup-and-call keeps trying.
pub const LOOKUP_FOR: Duration = Duration::from_secs(20);
/// The header on every call to a member that carries the sender's clock in ms (the site's `clock::STAMP`).
pub const MEMBER_CLOCK: &str = "x-graft-clock";
/// One call's deadline.
pub const CALL: Duration = Duration::from_millis(3_500);
/// The header on a call an edge makes for a client: the member counts it as served for the daily near check
/// (`workers/site/src/member.rs`, `near_due`); a site's own calls (cron, fold audit, sweep, a peer's lookup) never carry
/// it.
pub const CLIENT_CALL: &str = "x-graft-client";

/// The install's sites and the bindings that reach their classes.
pub struct Bindings<'a> {
    pub env: &'a Env,
    pub sites: &'a SiteTable,
}

impl Bindings<'_> {
    fn stub(&self, class: &str, site: SiteId, do_name: &str) -> Result<Stub> {
        let region = self.sites.get(site).ok_or_else(|| Error::RustError(format!("unknown site {site:?}")))?.region;
        self.env.durable_object(&format!("{class}_{}", site.0))?.get_by_name_with_location_hint(do_name, region.hint())
    }

    /// The member-class binding for `site`. Every `get()` passes the site's hint, because only the first call
    /// respects it.
    pub fn member(&self, site: SiteId, do_name: &str) -> Result<Stub> {
        self.stub("MEMBER", site, do_name)
    }

    pub fn registry(&self, site: SiteId, do_name: &str) -> Result<Stub> {
        self.stub("REGISTRY", site, do_name)
    }

    /// An object of `class` on `site` by name, made under DO jurisdiction `jurisdiction` (`"eu"`, `"us"`) when one is
    /// given, as a `SearchShard` is (`design/DESIGN.md`, Shape, "Identity"). `worker` 0.8 has no `jurisdiction()`, so
    /// the namespace's own JavaScript method is called. A jurisdiction object takes no location hint: the
    /// jurisdiction decides where it runs.
    pub fn object(&self, class: &str, site: SiteId, do_name: &str, jurisdiction: Option<&str>) -> Result<Stub> {
        let Some(j) = jurisdiction else { return self.stub(class, site, do_name) };
        let ns = self.env.durable_object(&format!("{class}_{}", site.0))?;
        let raw: &wasm_bindgen::JsValue = ns.as_ref();
        let f: js_sys::Function = js_sys::Reflect::get(raw, &"jurisdiction".into())?.into();
        let sub: ObjectNamespace = wasm_bindgen::JsCast::unchecked_into(f.call1(raw, &j.into())?);
        sub.get_by_name(do_name)
    }
}

pub fn now_ms() -> u64 {
    Date::now().as_millis()
}

pub async fn within<T>(d: Duration, f: impl Future<Output = T>) -> Option<T> {
    match select(Box::pin(f), Box::pin(Delay::from(d))).await {
        Either::Left((v, _)) => Some(v),
        Either::Right(_) => None,
    }
}

/// Per isolate: each chain's voters and learners from its last config, and the leader its replies named.
#[derive(Clone, Default)]
struct Known {
    voters: Vec<(SiteId, String)>,
    learners: Vec<(SiteId, String)>,
    config_index: u64,
    leader: Option<SiteId>,
    /// The highest term a reply from the group carried.
    term: u64,
    /// The member generation of the config held: the highest any member in it carries (`Placement::generation`).
    generation: u32,
    /// The member that last answered as leader, for an isolate that holds no config naming it: a chain whose leader
    /// always answers never sends a `NotLeader`, so its voters stay unknown.
    answered: Option<Target>,
}

thread_local! {
    static KNOWN: RefCell<HashMap<ChainId, Known>> = RefCell::new(HashMap::new());
    static COLO: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static CLIENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Per isolate: each learner's answer times to `read_spread` as this isolate saw them (`Timed`).
    static ANSWER_MS: RefCell<HashMap<(SiteId, String), Timed>> = RefCell::new(HashMap::new());
    /// Per isolate: each chain's last DEP_HEDGE_SAMPLES dependency read times in ms, oldest first (`dep_hedge_after`).
    static DEP_MS: RefCell<HashMap<ChainId, Vec<u32>>> = RefCell::new(HashMap::new());
}

/// The colo this isolate's `suspect` reports from `call_opt` carry; an edge sets it per request.
pub fn set_colo(colo: u32) {
    COLO.with(|c| c.set(colo));
}

/// Marks this isolate's member calls as made for a client (`CLIENT_CALL`); an edge sets it, a site script never does.
pub fn set_client() {
    CLIENT.with(|c| c.set(true));
}

/// A member call's headers: the sender's clock, and `CLIENT_CALL` from an edge.
fn call_headers() -> Option<Headers> {
    // The sender's clock: a member raises its own clock to the freshest stamp it sees (`MEMBER_CLOCK`). Edge
    // isolates are many and rarely saturated, so theirs stay fresh when every member is busy.
    let h = Headers::new();
    h.set(MEMBER_CLOCK, &now_ms().to_string()).ok()?;
    if CLIENT.with(|c| c.get()) {
        h.set(CLIENT_CALL, "1").ok()?;
    }
    Some(h)
}

/// Test installs: which isolate sent a member call and which step of the lookup rule sent it (`SENT_BY`), so a
/// member's trace splits its `/read`s by isolate and by walk (m42p, `tests/am1/results/lm1-memory.md`, round 37).
#[cfg(feature = "test-faults")]
pub const SENT_BY: &str = "x-graft-sent-by";

/// What sent a member call, in `SENT_BY` as `isolate * 16 + step`. Append-only, as trace kinds are;
/// `tests/f74/inbound.py` keeps the same list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SentBy {
    Other = 0,
    /// `call_opt`'s post to the leader this isolate held.
    Leader = 1,
    /// `call_opt`'s post to any other member: a walk for the leader.
    Walk = 2,
    /// `read_spread`'s read-index read to a learner or follower.
    Spread = 3,
    /// `commit_hedged`'s `/commit` send.
    Send = 4,
    /// `commit_hedged`'s `/outcome_by_request` ask.
    Ask = 5,
}

#[cfg(feature = "test-faults")]
thread_local! {
    /// This isolate's id for `SENT_BY`, drawn once.
    static ISOLATE: u64 = (js_sys::Math::random() * 4_294_967_296.0) as u64;
}

/// `post_within` with `SENT_BY` on a test install; a product build sends what `post_within` sends.
pub async fn post_by(
    b: &Bindings<'_>,
    to: SiteId,
    do_name: &str,
    path: &str,
    body: &[u8],
    deadline: Duration,
    by: SentBy,
) -> Option<(u16, Value)> {
    post_kind(b, to, do_name, path, body, deadline, by).await.filter(|(s, _)| *s != THROWN)
}

/// The status `post_kind` answers when the stub's fetch threw: never an HTTP status.
const THROWN: u16 = 0;

/// What a thrown fetch said (lookup-miss-kind): its message and the flags Cloudflare sets on it. `overloaded` is not
/// to be retried, since retrying worsens the overload; `retryable` is transient; `remote` came from the object's own
/// code or its CPU or memory limit [doc, Durable Objects, "Error handling"].
fn thrown(message: &str, overloaded: bool, retryable: bool, remote: bool) -> Value {
    json!({ "thrown": message, "overloaded": overloaded, "retryable": retryable, "remote": remote })
}

/// `post_by`, and a fetch that threw as `(THROWN, thrown(..))` in place of `None`, so `call_opt` can tell an
/// overloaded object from a missing one. On B0-35 m60, 125 cached-leader posts to a leader whose turns were blocked
/// for about 5 s of CPU missed in 2 to 98 ms, logged only as "no answer" (`decisions/OPEN-QUESTIONS.md`, F104).
#[allow(clippy::too_many_arguments)]
async fn post_kind(
    b: &Bindings<'_>,
    to: SiteId,
    do_name: &str,
    path: &str,
    body: &[u8],
    deadline: Duration,
    by: SentBy,
) -> Option<(u16, Value)> {
    let stub = b.member(to, do_name).ok()?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(js_sys::Uint8Array::from(body).into()));
    let h = call_headers()?;
    #[cfg(feature = "test-faults")]
    h.set(SENT_BY, &ISOLATE.with(|i| *i * 16 + by as u64).to_string()).ok()?;
    #[cfg(not(feature = "test-faults"))]
    let _ = by;
    init.with_headers(h);
    let req = Request::new_with_init(&format!("https://member{path}"), &init).ok()?;
    let call = async move {
        let mut res = match stub.fetch_with_request(req).await {
            Ok(r) => r,
            Err(e) => {
                let js = e.to_js_value();
                let flag = |k: &str| js_sys::Reflect::get(&js, &k.into()).ok().and_then(|v| v.as_bool()) == Some(true);
                return Some((THROWN, thrown(&e.to_string(), flag("overloaded"), flag("retryable"), flag("remote"))));
            }
        };
        let status = res.status_code();
        let v = res.json::<Value>().await.unwrap_or(Value::Null);
        Some((status, v))
    };
    within(deadline, call).await.flatten()
}

/// Which step of the rule found the group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Cached,
    Bootstrap { depth: usize },
    Registry { depth: usize },
}

fn learn(chain: ChainId, nl: &Value) {
    let Some(c) = nl.get("config").filter(|c| !c.is_null()) else { return };
    let index = c.get("index").and_then(Value::as_u64).unwrap_or(0);
    let Ok(cfg) = serde_json::from_value::<GroupConfig>(c["config"].clone()) else { return };
    let (p, lost) = match &cfg {
        GroupConfig::Normal(p) => (p, None),
        GroupConfig::Degraded { placement, lost } => (placement, Some(*lost)),
        GroupConfig::Joint { new, .. } => (new, None),
    };
    // A `Degraded` config's lost site has no voter (`graftyard_raft::quorums::voters`): its old member is not asked
    // or reported to.
    let voters = p.voters().iter().filter(|m| Some(m.site) != lost).map(|m| (m.site, m.do_name.to_string())).collect();
    let learners = p.learners().iter().map(|m| (m.site, m.do_name.to_string())).collect();
    let generation = p.generation();
    KNOWN.with(|k| {
        let mut k = k.borrow_mut();
        let e = k.entry(chain).or_default();
        if index >= e.config_index {
            e.voters = voters;
            e.learners = learners;
            e.config_index = index;
            e.generation = generation;
        }
    });
}

/// Whether this isolate holds a config of `chain`'s group at `record`'s generation or later, learned from a member's
/// reply: the group exists with members the record names, so `create` has nothing to build or elect, and a group whose
/// leader is gone is found and elected by the caller's own lookup and `suspect` reports (`design/DESIGN.md`, journey
/// 1, step 1). A config of an older generation does not count: after a move the record names members the held config
/// does not, and only `create`'s joins bootstrap them.
pub fn config_known(chain: ChainId, record: &PlacementRecord) -> bool {
    KNOWN.with(|k| k.borrow().get(&chain).is_some_and(|e| !e.voters.is_empty() && e.generation >= record.generation))
}

/// Whether a member's `NotLeader` naming no leader says nothing about the group: the member says it was removed
/// (`meta.removed`, its committed config no longer names it), or its config is older than the one this isolate holds,
/// so it has not heard the group since. Such an answer is followed by its config, never reported with `suspect` as a
/// leaderless group (B0-32 m3fr2: a replaced voter at config index 3, against the group's 9, drew 72 reports at a
/// leader in term 2).
fn stale_answer(chain: ChainId, v: &Value, nl: &Value) -> bool {
    if v.pointer("/meta/removed").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    let index = nl.pointer("/config/index").and_then(Value::as_u64).unwrap_or(0);
    KNOWN.with(|k| k.borrow().get(&chain).is_some_and(|e| index < e.config_index))
}

/// The leader a follower's or learner's read answer names (`read_spread`), only from a term at or above the highest
/// this isolate has seen: a member behind the group says nothing about its leader. Through `learn_leader` such an
/// answer dropped the leader held, and the next request walked the voters to find it again (m42p's trace,
/// `tests/am1/results/lm1-memory.md`, round 37 [inference]).
fn learn_leader_from_follower(chain: ChainId, v: &Value) {
    let behind = v
        .pointer("/meta/term")
        .and_then(Value::as_u64)
        .is_some_and(|t| KNOWN.with(|k| k.borrow().get(&chain).is_some_and(|e| t < e.term)));
    if !behind {
        learn_leader(chain, v);
    }
}

/// The leader a reply names. A reply whose term is below the highest this isolate has seen from the group comes from a
/// member behind it, so its leader is not kept and the one held is dropped: the next commit looks the leader up
/// (`knows_leader`). A reply with no term (a member built before `meta.term`) keeps the old rule.
fn learn_leader(chain: ChainId, v: &Value) {
    let leader = v.pointer("/meta/leader").and_then(Value::as_u64).and_then(|s| u8::try_from(s).ok()).map(SiteId);
    let term = v.pointer("/meta/term").and_then(Value::as_u64);
    KNOWN.with(|k| {
        let mut k = k.borrow_mut();
        let e = k.entry(chain).or_default();
        match term {
            Some(t) if t < e.term => e.leader = None,
            Some(t) => (e.term, e.leader) = (t, leader),
            None => e.leader = leader,
        }
    });
}

/// Whether this isolate holds a leader for `chain`, named by its config or the member that answered as leader, so the
/// group exists and a commit needs no create probe first (`design/DESIGN.md`, journey 1, step 1). False after any
/// `NotLeader` (`commit_hedged` forgets the leader), after a reply from a term below the one held (`learn_leader`), and
/// before any reply named one.
pub fn knows_leader(chain: ChainId) -> bool {
    cached_leader(chain).is_some()
}

pub fn forget_leader(chain: ChainId) {
    KNOWN.with(|k| {
        if let Some(e) = k.borrow_mut().get_mut(&chain) {
            e.leader = None;
        }
    });
}

/// Holds `leader` as `chain`'s leader at `term`, at the end of a lookup pass in which two or more followers named it
/// and its only answer was one miss: it is most likely busy taking over, as f0 was for 6 to 9 s after B0-35 m58's
/// term-4 transfer, while every pass forgot it and every other request waited on the walk (F101). The next pass asks
/// it as the cached leader, with `LEADER_RETRY`, and other requests go to it. False, and nothing held, when the config
/// held names no such voter or this isolate has seen a later term.
fn keep_named_leader(chain: ChainId, leader: SiteId, term: u64) -> bool {
    KNOWN.with(|k| {
        let mut k = k.borrow_mut();
        let Some(e) = k.get_mut(&chain).filter(|e| term >= e.term && e.voters.iter().any(|(s, _)| *s == leader)) else {
            return false;
        };
        (e.term, e.leader) = (term, Some(leader));
        true
    })
}

/// Drops every cached config and leader, for a caller that must start cold.
pub fn forget_all() {
    KNOWN.with(|k| k.borrow_mut().clear());
}

/// The member that answered a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub site: SiteId,
    pub do_name: String,
}

/// One POST to a member with CALL, its status and JSON body; `None` for a missed or failed call.
pub async fn post(b: &Bindings<'_>, to: SiteId, do_name: &str, path: &str, body: &[u8]) -> Option<(u16, Value)> {
    post_within(b, to, do_name, path, body, CALL).await
}

/// `post` with its own deadline.
pub async fn post_within(
    b: &Bindings<'_>,
    to: SiteId,
    do_name: &str,
    path: &str,
    body: &[u8],
    deadline: Duration,
) -> Option<(u16, Value)> {
    post_by(b, to, do_name, path, body, deadline, SentBy::Other).await
}

/// `create`'s `/join` to one bootstrap member, within 60 s: its status, and its body with `JOINED_FRESH` set to whether
/// the member's answer carried that header.
async fn post_join(b: &Bindings<'_>, to: SiteId, do_name: &str, body: &[u8]) -> Option<(u16, Value)> {
    let stub = b.member(to, do_name).ok()?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(js_sys::Uint8Array::from(body).into()));
    init.with_headers(call_headers()?);
    let req = Request::new_with_init("https://member/join", &init).ok()?;
    let call = async move {
        let mut res = stub.fetch_with_request(req).await.ok()?;
        let (status, fresh) = (res.status_code(), res.headers().get(JOINED_FRESH).ok().flatten().is_some());
        let v = res.json::<Value>().await.unwrap_or(Value::Null);
        // A refusal's body stays as the member sent it, for `refused`'s `noRepoOwner` count.
        Some((status, if status == 200 { json!({ JOINED_FRESH: fresh, "rows": v }) } else { v }))
    };
    within(Duration::from_secs(60), call).await.flatten()
}

async fn registry_row(b: &Bindings<'_>, at: SiteId, chain: ChainId) -> Option<RegistryRow> {
    let reg = MemberRegistry::for_chain(at, chain);
    let stub = b.registry(at, &reg.do_name()).ok()?;
    let url = format!("https://registry/row?chain={chain}");
    let call = async move {
        let mut res = stub.fetch_with_str(&url).await.ok()?;
        if res.status_code() != 200 {
            return None;
        }
        res.json::<RegistryRow>().await.ok()
    };
    within(CALL, call).await.flatten()
}

/// Every site's registry answer for `chain`, read in parallel (`decisions/DECISIONS.md`, "`repo-gone`"): unlike
/// `registry_row`, a definite 404 is told apart from a read that failed.
pub async fn registry_reads(b: &Bindings<'_>, chain: ChainId) -> Vec<graftyard_types::RowRead> {
    let read = |at: SiteId| async move {
        let stub = b.registry(at, &MemberRegistry::for_chain(at, chain).do_name()).ok()?;
        let res = stub.fetch_with_str(&format!("https://registry/row?chain={chain}")).await.ok()?;
        Some(res.status_code())
    };
    let reads = b.sites.0.iter().map(|s| async move { within(CALL, read(s.id)).await.flatten() });
    futures_util::future::join_all(reads).await.into_iter().map(graftyard_types::RowRead::of).collect()
}

/// The record of a chain whose record the caller does not hold (the `Release` group, before a cold isolate has its
/// genesis): the voters and generation from the first `MemberRegistry` row any site holds for it, with the sites in
/// the row's order. Its bootstrap names are the row's voters' names at that generation, as `bootstrap_members` builds.
pub async fn registry_record(b: &Bindings<'_>, chain: ChainId) -> Option<PlacementRecord> {
    for s in &b.sites.0 {
        if let Some(row) = registry_row(b, s.id, chain).await {
            let name = match row.sites.len() {
                1 => graftyard_types::PlacementName::Solo,
                _ => graftyard_types::PlacementName::Tri,
            };
            return Some(PlacementRecord {
                name,
                home: row.sites.first().copied()?,
                sites: row.sites.clone(),
                generation: row.generation,
                moving_from: None,
                at: Timestamp(0),
            });
        }
    }
    None
}

/// One stage of a lookup pass (`design/DESIGN.md`, Shape, `ReplyMeta`): stage 0 is the cached config, leader first,
/// then the first record's bootstrap names; stage `2i + 1` is record `i`'s registry rows; stage `2i + 2` is record
/// `i + 1`'s bootstrap names. `None` past the last record. A pass reads a stage only once every earlier candidate
/// failed to answer, so a lookup that the cache or the bootstrap names answer reads no registry row: each row is a call
/// to a record site's registry, one per site in turn, and reading all three before calling anyone cost about 400 ms of
/// every edge read (`tests/m2/results/m2-wp6.md`, "Merge latency").
enum Stage {
    Names(Vec<(SiteId, String, Step)>),
    Registry { depth: usize, sites: Vec<SiteId> },
}

fn stage(known: &Known, record: &PlacementRecord, chain: ChainId, k: usize) -> Option<Stage> {
    let depth = k / 2;
    let r = record.lookup_order().nth(depth)?;
    if k % 2 == 1 {
        return Some(Stage::Registry { depth, sites: r.sites.to_vec() });
    }
    let mut out = Vec::new();
    if k == 0 && !known.voters.is_empty() {
        let mut vs = known.voters.clone();
        vs.sort_by_key(|(s, _)| Some(*s) != known.leader);
        out.extend(vs.into_iter().map(|(s, n)| (s, n, Step::Cached)));
    }
    // A bootstrap name the held config has replaced is no member of the group: a config at the record's generation
    // or later names another member on its site. It still answers `NotLeader` naming none from its old config, and on
    // B0-35 m58 it ended every pass of the term-4 walk (`repo:65572#1#f1` beside the voter `#8#f1`, F101).
    let superseded = |m: &graftyard_types::Member| {
        known.generation >= m.generation && known.voters.iter().any(|(s, n)| *s == m.site && **n != *m.do_name)
    };
    out.extend(
        r.bootstrap_members(chain)
            .into_iter()
            .filter(|m| !superseded(m))
            .map(|m| (m.site, m.do_name.to_string(), Step::Bootstrap { depth })),
    );
    Some(Stage::Names(out))
}

/// Stage `k`'s candidates, its registry rows read in parallel. `None` past the last stage.
async fn candidates(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    k: usize,
) -> Option<Vec<(SiteId, String, Step)>> {
    let known = KNOWN.with(|k| k.borrow().get(&chain).cloned()).unwrap_or_default();
    Some(match stage(&known, record, chain, k)? {
        Stage::Names(v) => v,
        Stage::Registry { depth, sites } => {
            let rows = futures_util::future::join_all(sites.iter().map(|s| registry_row(b, *s, chain))).await;
            sites
                .into_iter()
                .zip(rows)
                .filter_map(|(s, row)| Some((s, row?.do_name.to_string(), Step::Registry { depth })))
                .collect()
        }
    })
}

/// Calls `path` on the group's leader, found by the rule, with a raw body. Returns the leader's answer, the step
/// that found the group, and the member that answered; `None` when the chain has no group: on the rule's first pass
/// every bootstrap member answered that it is not joined, and no site's registry holds a row for the chain. An answer
/// with `error.notLeader` (or `done.notLeader`) teaches the config and leader and is retried there; a member that is
/// not joined, or that holds no config, is skipped. `busy` (a read past its deadline) is skipped too. A member's
/// `overloaded` answer is retried on it after its `retryAfterMs`, then returned as an error naming OVERLOADED; it
/// never raises `suspect` (`classify`). Only
/// leader-answered routes should be called this way: a command goes to one member at a time, and a member that is
/// not the leader refuses it.
pub async fn call_opt(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    path: &str,
    body: &[u8],
) -> std::result::Result<Option<(Value, Step, Target)>, String> {
    let colo = COLO.with(|c| c.get());
    lookup(
        chain,
        now_ms,
        |k| candidates(b, chain, record, k),
        move |s, name: String, by| async move { post_kind(b, s, &name, path, body, CALL, by).await },
        move |about: Target, term, all| async move { suspect(b, chain, &about, term, colo, all).await },
        Delay::from,
    )
    .await
}

/// How long a request waits after its cached leader's first miss before it asks that leader again (`lookup`) [guess]:
/// the same wait as between two passes.
pub const LEADER_RETRY: Duration = Duration::from_millis(250);

/// How long a chain's lookup mark (`LOOKUP_MARK`) holds other requests off after its holder last touched it [guess]: a
/// holder touches it before each post and pass, so a live lookup keeps it, and one whose request the runtime cancelled,
/// whose future then never runs or drops, holds the chain this long, not LOOKUP_FOR.
pub const LOOKUP_MARK: Duration = Duration::from_millis(1_000);
/// How often a request waiting on another's lookup looks at the cache again, on its own timer [guess].
pub const LOOKUP_POLL: Duration = Duration::from_millis(50);

thread_local! {
    /// Per isolate: each chain with a leader lookup running, as plain data: the holder's token and when it last touched
    /// the mark. No request holds or wakes another's future: a Workers request's I/O, timers and closures belong to it,
    /// and m42's edge, whose waiters were woken from the request that ended a lookup, logged "Cannot perform I/O on
    /// behalf of a different request" and "closure invoked ... after being dropped" (`tests/m0/results/m41-held.md`).
    static LOOKUP_MARKS: RefCell<HashMap<ChainId, (u64, u64)>> = RefCell::new(HashMap::new());
    static LOOKUP_TOKENS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The chains whose lookup marks this isolate logs (`trace_marks`).
    static MARK_TRACED: RefCell<std::collections::HashSet<ChainId>> = RefCell::new(std::collections::HashSet::new());
}

/// Logs `chain`'s lookup marks in this isolate from now on: each post the mark's holder sends, with its member, step,
/// answer and time, how the holder's lookup ended, each wait on another request's mark with its time and the requests
/// that held it, each miss of a post to the cached leader, and each `local_first` read not taken locally. A git
/// cache shard turns it on for its network's chain, so a row shows why its catch-up's lookups found no member for
/// about 2 min after B0-35 m57's term change (`decisions/OPEN-QUESTIONS.md`, F101). Logging only: no rule changes.
pub fn trace_marks(chain: ChainId) {
    MARK_TRACED.with(|t| t.borrow_mut().insert(chain));
}

fn mark_trace(chain: ChainId, line: impl FnOnce() -> String) {
    if MARK_TRACED.with(|t| t.borrow().contains(&chain)) {
        let line = line();
        #[cfg(target_arch = "wasm32")]
        console_log!("lookup mark {chain}: {line}");
        #[cfg(not(target_arch = "wasm32"))]
        MARK_LINES.with(|l| l.borrow_mut().push(line));
    }
}

/// Request tokens as a trace names them: `t12,t15`.
fn tokens(t: &[u64]) -> String {
    t.iter().map(|t| format!("t{t}")).collect::<Vec<_>>().join(",")
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    /// Off wasm (tests), `mark_trace`'s lines.
    static MARK_LINES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Takes or refreshes `chain`'s lookup mark for `token` at `now`: true if the mark is now this token's, because it was
/// free, stale (untouched for LOOKUP_MARK), or already this token's.
fn mark(chain: ChainId, token: u64, now: u64) -> bool {
    LOOKUP_MARKS.with(|m| {
        let mut m = m.borrow_mut();
        let stale = |at: u64| now >= at + LOOKUP_MARK.as_millis() as u64;
        match m.get(&chain) {
            Some((t, at)) if *t != token && !stale(*at) => false,
            _ => {
                m.insert(chain, (token, now));
                true
            }
        }
    })
}

/// Frees `chain`'s mark if `token` still holds it.
fn unmark(chain: ChainId, token: u64) {
    LOOKUP_MARKS.with(|m| {
        let mut m = m.borrow_mut();
        if m.get(&chain).is_some_and(|(t, _)| *t == token) {
            m.remove(&chain);
        }
    });
}

/// A request's hold on its chain's mark: frees it on drop if still held. A cancelled request may never drop it, which
/// LOOKUP_MARK bounds.
struct Marked(ChainId, u64);

impl Drop for Marked {
    fn drop(&mut self) {
        unmark(self.0, self.1);
    }
}

/// `call_opt`'s rule, with its clock, calls and sleeps passed in, so a test drives many at once. `candidates(k)` is
/// stage `k`'s members, `post(site, name)` one call, `suspect(about, term, all)` one report, `delay(d)` a sleep. A post
/// to the leader this isolate has cached needs nothing. Any other post needs the chain's lookup mark: a request that
/// finds another's mark fresh polls the cache on its own `delay` every LOOKUP_POLL, goes to a leader once one is
/// cached, and takes the mark once it is free or stale (`design/DESIGN.md`, Shape, `ReplyMeta`).
async fn lookup<C, CF, P, PF, S, SF, D, DF>(
    chain: ChainId,
    now: impl Fn() -> u64,
    candidates: C,
    post: P,
    suspect: S,
    delay: D,
) -> std::result::Result<Option<(Value, Step, Target)>, String>
where
    C: Fn(usize) -> CF,
    CF: Future<Output = Option<Vec<(SiteId, String, Step)>>>,
    P: Fn(SiteId, String, SentBy) -> PF,
    PF: Future<Output = Option<(u16, Value)>>,
    S: Fn(Target, u64, bool) -> SF,
    SF: Future<Output = Option<Target>>,
    D: Fn(Duration) -> DF,
    DF: Future<Output = ()>,
{
    let end = now() + LOOKUP_FOR.as_millis() as u64;
    let mut last = String::from("no member answered");
    let mut first_pass = true;
    let mut term = 0;
    let mut busy = 0;
    // Held from this request's first post to a member other than its cached leader until it returns, across passes,
    // and touched before each post, so a lookup that finds no leader is not run again beside it by every request.
    let token = LOOKUP_TOKENS.with(|t| {
        t.set(t.get() + 1);
        t.get()
    });
    let mut held: Option<Marked> = None;
    // Whether this request's post to its cached leader already missed once (`LEADER_RETRY`).
    let mut leader_missed = false;
    // Whether this request already kept a named leader past a pass (`keep_named_leader`): once only, so a leader
    // that keeps missing is walked away from on the pass after.
    let mut kept_once = false;
    // Answers this request had from an object that threw `overloaded`, for OBJECT_WAIT's doubling.
    let mut held_off = 0u32;
    while now() < end {
        let mut k = 0;
        let mut cands = candidates(k).await.unwrap_or_default();
        let mut unjoined = 0;
        let mut others = 0;
        let mut tried = std::collections::BTreeSet::new();
        // Members that missed: once per pass each is reported with `suspect` to the other voters, as a missed commit
        // receipt is, because campaigns are request-driven and a read-only caller is otherwise never one.
        let mut missed: Vec<Target> = Vec::new();
        // A voter that answered `NotLeader` naming no leader: reported to every voter at the end of a pass that found
        // no leader, as `commit_hedged` does, or a group whose members all reset stays leaderless for a caller
        // whose only call is this lookup (a commit's `/outcome_by_request`).
        let mut leaderless: Option<Target> = None;
        // Per leader a follower's `NotLeader` named this pass, the followers that named it; and per member, its
        // misses this pass. A leader two or more followers name, whose only answer this pass was one miss, is kept
        // for the next pass (`keep_named_leader`).
        let mut named: std::collections::BTreeMap<SiteId, std::collections::BTreeSet<SiteId>> = Default::default();
        let mut misses: std::collections::BTreeMap<SiteId, u32> = Default::default();
        let mut queue: std::collections::VecDeque<(SiteId, String, Step)> = std::mem::take(&mut cands).into();
        loop {
            let Some((s, name, step)) = queue.pop_front() else {
                k += 1;
                match candidates(k).await {
                    Some(next) => {
                        queue.extend(next);
                        continue;
                    }
                    None => break,
                }
            };
            if tried.contains(&name) {
                continue;
            }
            if held.is_some() {
                mark(chain, token, now());
            } else if cached_leader(chain).is_none_or(|l| l.do_name != name) {
                // Another request's lookup is running: look at the cache on this request's own timer until it names a
                // leader, or the mark is free or stale.
                let mut found = None;
                let waited_from = now();
                // Each request that held the mark while this one waited, in turn (F104).
                let mut holders: Vec<u64> = Vec::new();
                while !mark(chain, token, now()) {
                    if let Some(h) = LOOKUP_MARKS.with(|m| m.borrow().get(&chain).map(|(t, _)| *t))
                        && holders.last() != Some(&h)
                    {
                        holders.push(h);
                    }
                    if now() >= end {
                        mark_trace(chain, || {
                            format!("t{token} gave up after {} ms waiting on {}", now() - waited_from, tokens(&holders))
                        });
                        return Err(format!("{chain}: {last}"));
                    }
                    delay(LOOKUP_POLL).await;
                    found = cached_leader(chain).filter(|l| !tried.contains(&l.do_name));
                    if found.is_some() {
                        break;
                    }
                }
                if !holders.is_empty() {
                    mark_trace(chain, || {
                        let then = found
                            .as_ref()
                            .map_or("took the mark".into(), |l| format!("found {}/{}", l.site.0, l.do_name));
                        let tried_leader = cached_leader(chain).is_some_and(|l| tried.contains(&l.do_name));
                        format!(
                            "t{token} waited {} ms on {}, then {then} (cached leader tried by it: {tried_leader})",
                            now() - waited_from,
                            tokens(&holders)
                        )
                    });
                }
                if let Some(l) = found {
                    queue.push_front((s, name, step));
                    queue.push_front((l.site, l.do_name, Step::Cached));
                    continue;
                }
                held = Some(Marked(chain, token));
            }
            tried.insert(name.clone());
            let by =
                if cached_leader(chain).is_some_and(|l| l.do_name == name) { SentBy::Leader } else { SentBy::Walk };
            let posted = now();
            let r = post(s, name.clone(), by).await;
            let answer = classify(r.as_ref().map(|(s, v)| (*s, v)), matches!(step, Step::Bootstrap { .. }));
            if held.is_some() {
                mark_trace(chain, || {
                    let nl =
                        r.as_ref().and_then(|(_, v)| v.pointer("/error/notLeader").or(v.pointer("/done/notLeader")));
                    let leader = nl.and_then(|n| n.get("leader")).filter(|l| !l.is_null());
                    format!(
                        "t{token} post {}/{name} {step:?} {by:?}: {answer:?}{} in {} ms",
                        s.0,
                        nl.map(|_| format!(" notLeader(leader {leader:?})")).unwrap_or_default(),
                        now() - posted
                    )
                });
            } else if by == SentBy::Leader && answer != Answer::Reply {
                // A post to the cached leader takes no mark; its miss is what sends a request into the walk (F104).
                mark_trace(chain, || {
                    format!("t{token} cached leader {}/{name}: {answer:?} in {} ms", s.0, now() - posted)
                });
            }
            // lookup-retry: the first miss of a post to the cached leader, or its `busy`, is reported (a miss only) at
            // the term this isolate holds, and the leader is asked again after LEADER_RETRY; only a second miss walks
            // the voters. Before, the same pass posted every follower, whose `NotLeader` named the leader already
            // tried, then reported, forgot the leader and walked again: on B0-33 m45 each follower took 222 to 224 walk
            // `/read`s a second, all for the leader (`tests/am1/results/lm1-memory.md`, round 40).
            if by == SentBy::Leader
                && !leader_missed
                && matches!(&answer, Answer::Missed(_)) | matches!(&answer, Answer::Skip(w) if w == "busy")
            {
                leader_missed = true;
                if matches!(answer, Answer::Missed(_)) {
                    suspect(Target { site: s, do_name: name.clone() }, term.max(known_term(chain)), false).await;
                }
                delay(LEADER_RETRY).await;
                tried.remove(&name);
                queue.push_front((s, name, step));
                continue;
            }
            let v = match answer {
                Answer::Missed(why) => {
                    last = format!("{name}: {why}");
                    others += 1;
                    *misses.entry(s).or_default() += 1;
                    missed.push(Target { site: s, do_name: name });
                    continue;
                }
                Answer::NotJoined => {
                    last = format!("{name}: not joined");
                    unjoined += 1;
                    continue;
                }
                Answer::Skip(why) => {
                    last = format!("{name}: {why}");
                    others += 1;
                    continue;
                }
                // lookup-miss-kind: an object whose fetch threw `overloaded` is waited for, not retried at once, walked
                // away from or reported, since every post adds to its overload [doc, Durable Objects, "Error handling"].
                Answer::ObjectOverloaded(why) => {
                    let wait = (OBJECT_WAIT.as_millis() as u64 * (1 << held_off.min(3))).min(SHED_BACKOFF_MAX_MS);
                    held_off += 1;
                    if now() + wait >= end {
                        return Err(format!("{chain}: {name}: {OVERLOADED} ({why})"));
                    }
                    delay(Duration::from_millis(wait)).await;
                    tried.remove(&name);
                    queue.push_front((s, name, step));
                    continue;
                }
                // The leader is alive and shedding heavy reads: no `suspect`, not missed, not leaderless. The same
                // member is asked again after its `retryAfterMs`, OVERLOAD_TRIES times, then the caller answers 503.
                Answer::Overloaded(ms) => {
                    others += 1;
                    busy += 1;
                    if busy > OVERLOAD_TRIES {
                        return Err(format!("{chain}: {name}: {OVERLOADED} {ms} ms"));
                    }
                    delay(Duration::from_millis(ms.clamp(50, 2_000))).await;
                    tried.remove(&name);
                    queue.push_front((s, name, step));
                    continue;
                }
                Answer::Reply => {
                    others += 1;
                    r.map(|(_, v)| v).unwrap_or_default()
                }
            };
            if let Some(nl) = v.pointer("/error/notLeader").or_else(|| v.pointer("/done/notLeader")) {
                let stale = stale_answer(chain, &v, nl);
                if !stale {
                    term = term.max(nl.get("term").and_then(Value::as_u64).unwrap_or(0));
                }
                learn(chain, nl);
                if nl.get("config").is_none_or(Value::is_null) {
                    last = format!("{name}: no config");
                    continue;
                }
                if nl.get("leader").is_none_or(Value::is_null) && !stale {
                    leaderless.get_or_insert(Target { site: s, do_name: name.clone() });
                }
                // Follow a named leader first, then the config's voters.
                let known = KNOWN.with(|k| k.borrow().get(&chain).cloned()).unwrap_or_default();
                let leader = nl.get("leader").and_then(Value::as_u64).and_then(|l| u8::try_from(l).ok()).map(SiteId);
                if let Some(l) = leader.filter(|l| !stale && *l != s) {
                    named.entry(l).or_default().insert(s);
                }
                let mut next: Vec<(SiteId, String, Step)> =
                    known.voters.iter().map(|(vs, vn)| (*vs, vn.clone(), step)).collect();
                next.sort_by_key(|(vs, _, _)| Some(*vs) != leader);
                for c in next.into_iter().rev() {
                    queue.push_front(c);
                }
                last = format!("{name}: not leader");
                continue;
            }
            learn_leader(chain, &v);
            let t = Target { site: s, do_name: name };
            KNOWN.with(|k| {
                if let Some(e) = k.borrow_mut().get_mut(&chain).filter(|e| e.leader == Some(s)) {
                    e.answered = Some(t.clone());
                }
            });
            if held.is_some() {
                mark_trace(chain, || format!("t{token} found {}/{} by {step:?}", t.site.0, t.do_name));
            }
            return Ok(Some((v, step, t)));
        }
        if first_pass && unjoined > 0 && others == 0 {
            if held.is_some() {
                mark_trace(chain, || format!("t{token} found no group"));
            }
            return Ok(None);
        }
        first_pass = false;
        let reports = missed.iter().map(|m| suspect(m.clone(), term, false));
        futures_util::future::join_all(reports).await;
        if let Some(t) = &leaderless {
            suspect(t.clone(), term, true).await;
        }
        let kept = !kept_once
            && named
                .iter()
                .find(|(l, by)| by.len() >= 2 && misses.get(*l) == Some(&1))
                .is_some_and(|(l, _)| keep_named_leader(chain, *l, term));
        kept_once |= kept;
        if !kept {
            forget_leader(chain);
        }
        mark_trace(chain, || {
            format!("t{token} pass ended with no leader{}: {last}", if kept { ", leader kept" } else { "" })
        });
        delay(Duration::from_millis(250)).await;
    }
    if held.is_some() {
        mark_trace(chain, || format!("t{token} gave up: {last}"));
    }
    Err(format!("{chain}: {last}"))
}

/// How long the edge waits for one follower's read-index answer before it asks the next member [guess]: a follower
/// that has not applied the leader's index answers `overloaded` itself after the site's FOLLOWER_WAIT (1 s).
pub const FOLLOWER_CALL: Duration = Duration::from_millis(2_000);

/// Members `read_spread` asks at most for one read [guess]: as many as the activity feed tries, so a read every one
/// sheds waits at most about three FOLLOWER_WAITs before its 503.
const SPREAD_TRIES: usize = 3;

/// How far behind the best learner's score a learner may be and still share its reads [guess]: the near learners
/// answered at p99 0.8 to 1.2 s and the far ones (apac from an enam leader) at 2.2 to 3.0 s (`design/DESIGN.md`,
/// "Follower read"), so a margin well under that gap keeps the far ones out and the near ones sharing the load.
pub const ROUTE_MARGIN: Duration = Duration::from_millis(200);
/// The weight of a new answer time in a learner's moving average and deviation [guess]: one slow answer moves each a
/// fifth of the way.
const ROUTE_WEIGHT: f64 = 0.2;
/// A learner's score is its average plus this many deviations [guess], so one whose answers vary (it sheds, or
/// answers late now and then) scores worse than its average says: ordered by the average alone, one learner took 50.2%
/// of gate 15's reads and shed 54 and answered 720 late (`tests/am1/results/narrow-reads-3.md`, "The herding guard").
pub const ROUTE_DEV: f64 = 4.0;
/// At least this many learners share an isolate's reads [guess]: with fewer within ROUTE_MARGIN of the best, the next
/// best are added while they score under ROUTE_LATE, so no one learner takes much more than a third of them.
pub const ROUTE_SPREAD: usize = 3;
/// A learner scored at or past this is not added to the reads' share [guess]: a member answers `overloaded` once it
/// has not applied the leader's index in its FOLLOWER_WAIT (1 s), so such a learner mostly answers late.
pub const ROUTE_LATE: Duration = Duration::from_millis(1_000);
/// A learner this isolate has not timed for this long counts as untimed [guess]: eligible, and not setting the best,
/// so a learner that recovers is timed again, while one that stays slow is asked about that often by each isolate.
pub const ROUTE_STALE: Duration = Duration::from_secs(10);

/// One learner's answer times as one isolate saw them: their moving average and mean deviation, in ms, and when it
/// last timed one (`now_ms`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Timed {
    mean: f64,
    dev: f64,
    at: u64,
}

impl Timed {
    /// The learner's score at `now`: its average plus ROUTE_DEV deviations, or `None` once untimed for ROUTE_STALE.
    fn score(&self, now: u64) -> Option<f64> {
        (now.saturating_sub(self.at) < ROUTE_STALE.as_millis() as u64).then_some(self.mean + ROUTE_DEV * self.dev)
    }

    /// After one more answer time `sample_ms` at `now`: the first sample, or one after ROUTE_STALE, starts afresh;
    /// then the average and the deviation each move ROUTE_WEIGHT of the way.
    fn after(prev: Option<Timed>, sample_ms: f64, now: u64) -> Timed {
        match prev.filter(|p| p.score(now).is_some()) {
            None => Timed { mean: sample_ms, dev: 0.0, at: now },
            Some(p) => {
                let d = sample_ms - p.mean;
                Timed { mean: p.mean + ROUTE_WEIGHT * d, dev: p.dev + ROUTE_WEIGHT * (d.abs() - p.dev), at: now }
            }
        }
    }
}

/// `timed_spread_order` with no learner timed: the learners rotated by `pick`, then the followers.
#[cfg(test)]
fn spread_order(
    learners: Vec<(SiteId, String)>,
    voters: Vec<(SiteId, String)>,
    leader: Option<SiteId>,
    pick: usize,
) -> Vec<Target> {
    timed_spread_order(learners.into_iter().map(|(s, n)| (s, n, None)).collect(), voters, leader, pick)
}

/// The members a read-index read goes to, in order: the cached config's learners, then its followers (the voters,
/// else the record's bootstrap members, without the cached leader), rotated by `pick`, at most SPREAD_TRIES. With no
/// leader cached every voter is a candidate: a member that leads serves the read itself. Each learner carries this
/// isolate's score of it in ms (`Timed::score`), `None` if untimed. The eligible learners come first, in config order
/// rotated by `pick`: the untimed ones, those within ROUTE_MARGIN of the best timed score, and, while fewer than
/// ROUTE_SPREAD, the next best scored under ROUTE_LATE. The rest follow by ascending score.
fn timed_spread_order(
    learners: Vec<(SiteId, String, Option<f64>)>,
    voters: Vec<(SiteId, String)>,
    leader: Option<SiteId>,
    pick: usize,
) -> Vec<Target> {
    fn rotated<T>(mut v: Vec<T>, pick: usize) -> Vec<T> {
        if !v.is_empty() {
            let n = pick % v.len();
            v.rotate_left(n);
        }
        v
    }
    let best = learners.iter().filter_map(|l| l.2).fold(f64::INFINITY, f64::min);
    let (margin, late) = (ROUTE_MARGIN.as_millis() as f64, ROUTE_LATE.as_millis() as f64);
    let (mut near, mut far): (Vec<_>, Vec<_>) =
        learners.into_iter().partition(|l| l.2.is_none_or(|s| s <= best + margin));
    far.sort_by(|a, b| a.2.unwrap_or(0.0).total_cmp(&b.2.unwrap_or(0.0)));
    let pad = far.iter().take_while(|l| l.2.is_some_and(|s| s < late)).count();
    near.extend(far.drain(..pad.min(ROUTE_SPREAD.saturating_sub(near.len()))));
    let mut out: Vec<Target> =
        rotated(near, pick).into_iter().chain(far).map(|(site, do_name, _)| Target { site, do_name }).collect();
    let followers =
        voters.into_iter().filter(|(s, _)| Some(*s) != leader).map(|(site, do_name)| Target { site, do_name });
    out.extend(rotated(followers.collect(), pick));
    out.truncate(SPREAD_TRIES);
    out
}

/// The answer time a learner's answer to `read_spread` records, in ms: its elapsed time if its value is used, else
/// FOLLOWER_CALL (shed, overloaded, `busy`, `notLeader`, a 4xx or 5xx, or no answer).
fn answer_sample(r: Option<(u16, &Value)>, elapsed_ms: f64) -> f64 {
    if follower_answer_usable(r) { elapsed_ms } else { FOLLOWER_CALL.as_millis() as f64 }
}

/// Whether `q` (a `/read` body's `q`) is a read the acting leader never serves by read-index, and `read_spread` never
/// sends on to it: a whole-tier dump (`{"rows": true}`) or an `ls-refs` (`{"lsRefs": ..}`). On B0-33 m53 the acting
/// leader served 5,460 whole-tier dumps in two bursts after leader changes, 3,835 of them sent on after every member
/// declined; its first stall held 1,052 reads in 5.2 s with no write stepped (`design/DESIGN.md`, "Follower read";
/// `decisions/DECISIONS.md`, "`tier-off-leader`"). A merge's rows, a narrow API read and the read gate's rows are not.
pub fn leader_never_serves(q: &Value) -> bool {
    q.get("rows").is_some_and(Value::is_boolean) || q.get("lsRefs").is_some()
}

/// The reason a `TierOff` read fails with: it names OVERLOADED, so every caller treats it as a shed read (503 with
/// `Retry-After: 1`, a write's shed retry, an Agent push's `ng`), and TIER_OFF, so the edge logs it apart
/// (`decisions/DECISIONS.md`, "`tier-off-leader`", the flip).
pub const TIER_OFF: &str = "tierOff";

/// The read `Spread::Elect` sends the leader's way instead of the dump: it answers nothing of the fold, as `create`'s
/// probe does, so a new leader serves no dump for it.
const ELECT_PROBE: &str = r#"{"q":{"probe":true}}"#;

/// The body a pass sends the leader's way for its verdict, if any: the read itself after every member declined a read
/// the leader may serve (`Leader`), the probe for `Elect`, and nothing for any other.
fn leader_body(verdict: &Spread, body: &Value) -> Option<String> {
    match verdict {
        Spread::Leader => Some(body.to_string()),
        Spread::Elect => Some(ELECT_PROBE.to_owned()),
        _ => None,
    }
}

fn tier_off_why(chain: ChainId) -> String {
    format!("{chain}: every member declined a read the leader never serves: {OVERLOADED} ({TIER_OFF})")
}

/// What `read_spread` does with the answers it got, in order.
#[derive(Debug, PartialEq, Eq)]
enum Spread {
    /// The answer at this position is the read's value.
    Use(usize),
    /// None answered a value and at least one could not take it: `overloaded` (at its caps, behind, or installing),
    /// `busy`, a 5xx, or no answer within FOLLOWER_CALL ("Durable Object is overloaded" is a failed call).
    /// The leader is not asked, so a read the members shed never lands on the leader's thread. The ms to wait before
    /// asking again: the largest `retryAfterMs` an `overloaded` answer named, else OVERLOAD_RETRY_MS.
    Shed(u64),
    /// Every one asked declined it as not its to serve: it answered `notLeader` (a leader-only query, its metadata
    /// tier is loading, or it knows no leader) or a 4xx (not joined: a group not created yet), or none was asked. The
    /// leader by `call_opt`, which finds a group not yet created and reports a leaderless one with `suspect`.
    Leader,
    /// Every one asked declined a read the leader never serves (`leader_never_serves`) and one named a leader: shed as
    /// `Shed` is, with its own reason, TIER_OFF, so a row counts the 503s this rule makes.
    TierOff,
    /// Every one asked declined a read the leader never serves, at least one answered `notLeader`, and none named a
    /// leader: the group may have none. The read is shed as `TierOff` is, after a cheap read (`ELECT_PROBE`, never
    /// the read itself) goes the leader's way by `call_opt`, whose lookup reports the leaderless group with `suspect`
    /// and so starts its election: campaigns are request-driven (F67), and a group read only by dumps and `ls-refs`
    /// sent nothing else that would start one (M4-WP4 j4, repo:65536, leaderless at term 4 for over 7 minutes).
    Elect,
    /// Every one asked declined a read the leader never serves (`leader_never_serves`) and none answered `notLeader`:
    /// none is joined, so the group may not exist yet, and the caller creates it and asks again.
    Absent,
}

/// `read_spread`'s choice from the answers of the members `spread_order` named, in order. `leader_ok` is false for a
/// read the leader never serves: one that every member declined is then `TierOff` if any named a leader, `Elect` if
/// some answered `notLeader` and none named one, and `Absent` if none answered `notLeader` (none is joined).
fn spread_verdict(answers: &[Option<(u16, &Value)>], leader_ok: bool) -> Spread {
    if let Some(i) = answers.iter().position(|r| follower_answer_usable(*r)) {
        return Spread::Use(i);
    }
    let not_leader = |r: &Option<(u16, &Value)>| matches!(r, Some((200, v)) if v.pointer("/error/notLeader").is_some() || v.pointer("/done/notLeader").is_some());
    let declined = |r: &Option<(u16, &Value)>| not_leader(r) || matches!(r, Some((s, _)) if (400..500).contains(s));
    if answers.iter().all(declined) {
        let named = |r: &Option<(u16, &Value)>| {
            matches!(r, Some((200, v)) if v.pointer("/error/notLeader/leader").or(v.pointer("/done/notLeader/leader"))
                .is_some_and(Value::is_u64))
        };
        return match (leader_ok, answers.iter().any(named), answers.iter().any(not_leader)) {
            (true, _, _) => Spread::Leader,
            (false, true, _) => Spread::TierOff,
            (false, false, true) => Spread::Elect,
            (false, false, false) => Spread::Absent,
        };
    }
    let wait = answers.iter().filter_map(|r| match classify(*r, false) {
        Answer::Overloaded(ms) => Some(ms),
        _ => None,
    });
    Spread::Shed(wait.max().unwrap_or(OVERLOAD_RETRY_MS).clamp(50, 2_000))
}

/// Whether a follower's answer to a read-index read is used. Only a value is. `notLeader` (it names the leader, or its
/// metadata tier is loading), `overloaded` (behind, installing, or at its cap), `busy`, a 4xx or 5xx, and no answer
/// move to the next member, and none is reported with `suspect`: a follower that cannot serve a read is no evidence
/// that the leader is down.
fn follower_answer_usable(r: Option<(u16, &Value)>) -> bool {
    classify(r, false) == Answer::Reply
        && r.is_some_and(|(_, v)| v.pointer("/error/notLeader").is_none() && v.pointer("/done/notLeader").is_none())
}

/// A read off the leader (`design/DESIGN.md`, "Follower read"): `body` (`{"q": ..}`) goes with `"index": true` to
/// the members `timed_spread_order` names, learners first, until one answers with a value. If none did and one shed it,
/// the read fails with an error naming OVERLOADED, which the caller answers 503 with `Retry-After`, and the leader is
/// never asked; only if none shed it does it go to the leader by `call_opt` (`spread_verdict`). A member's answer
/// holds every entry acknowledged before the read arrived: it waited for the leader's confirmed commit index.
/// Learners and followers never raise `suspect` and never count as missed. Each learner asked records its answer
/// time in this isolate's moving average and deviation (`answer_sample`, `Timed`), whose score orders the learners
/// of the next read.
pub async fn read_spread(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    body: &Value,
) -> std::result::Result<Option<(Value, Step, Target)>, String> {
    read_spread_for(b, chain, record, body, Duration::ZERO).await
}

/// `read_spread`, asked again after the wait its members named while they shed it, for up to `retry_for`
/// (`retry_shed`). A write's lookup takes LOOKUP_FOR, as `call_opt` does: a stalled `Authz` leader left both followers
/// without a read index for 7.4 s, and the read's push failed 503 though the group answered again 7 s later
/// (`tests/am1/results/lease-clock.md`, "The shed Authz read"). A read-class request takes zero and answers 503 at once.
pub async fn read_spread_for(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    body: &Value,
    retry_for: Duration,
) -> std::result::Result<Option<(Value, Step, Target)>, String> {
    read_spread_traced(b, chain, record, body, retry_for, None).await
}

/// `read_spread_for`, and each pass's tries as one line in `trace` (`try_line`), with the pass's ms: the member
/// asked, whether a learner, how long it took, what it answered, and so whether the read moved on to the next member.
/// Logging only: the reads and their order are `read_spread_for`'s (`gate-trace`).
pub async fn read_spread_traced(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    body: &Value,
    retry_for: Duration,
    trace: Option<&RefCell<Vec<(u64, String)>>>,
) -> std::result::Result<Option<(Value, Step, Target)>, String> {
    let until = now_ms() + retry_for.as_millis() as u64;
    let wait = |ms| Delay::from(Duration::from_millis(ms));
    retry_shed(until, now_ms, || spread_once(b, chain, record, body, true, trace), wait, js_sys::Math::random)
        .await
        .and_then(|r| r)
}

/// One member try of a spread pass as a trace names it: `L1:1003:ov` is a learner on site 1 that answered
/// `overloaded` after 1,003 ms. `ok` is a value, `nl` a `notLeader`, `ov` `overloaded`, `none` no answer within
/// FOLLOWER_CALL (or a fetch that threw), and anything else the status or skip reason. Only `ok` ends the pass.
fn try_line(site: SiteId, learner: bool, ms: u64, r: Option<(u16, &Value)>) -> String {
    let what = match classify(r, false) {
        Answer::Reply if follower_answer_usable(r) => "ok".to_owned(),
        Answer::Reply => "nl".to_owned(),
        Answer::Overloaded(_) => "ov".to_owned(),
        Answer::Missed(_) if r.is_none() => "none".to_owned(),
        Answer::Missed(w) | Answer::Skip(w) | Answer::ObjectOverloaded(w) => w.replace([' ', ',', '"'], "_"),
        Answer::NotJoined => "nj".to_owned(),
    };
    format!("{}{}:{ms}:{what}", if learner { 'L' } else { 'F' }, site.0)
}

/// `read_spread` without the leader: one pass over the learners and followers, and `Ok(None)` where `read_spread`
/// would go on to the leader. For a read the caller has another place to serve from (the edge's `ls-refs`, whose
/// fallback is the network's shard), so it never adds to the leader's load (F91, `tests/am1/results/b029-m2b.md`).
pub async fn read_off_leader(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    body: &Value,
) -> std::result::Result<Option<(Value, Step, Target)>, String> {
    spread_once(b, chain, record, body, false, None).await.map_err(|(why, _)| why).and_then(|r| r)
}

/// How long a write asks again for a read its members shed (`read_spread_for`) [guess]: a few backed-off tries, then a
/// 503 with `Retry-After`, so a pass already under way ends it at about this plus one pass (three FOLLOWER_WAITs).
/// LOOKUP_FOR (20 s) at a fixed 250 ms made each held write a dozen or more passes: on B0-33 m51 the learners' reads
/// rose from 15 to 54 a second each to 136 to 514 after a leader change, and 4,200 writes answered 503 after 19 to
/// 24 s (`tests/am1/results/health-gate-lease.md`). At 4 s a write that met a shed made about five member reads before
/// its 503, against about 1.2 for a served one, which held B0-33 m56's learners at 520 to 604 reads a second for 166
/// offered requests; 2 s gives such a write one backed-off retry pass at most (`decisions/DECISIONS.md`,
/// "`shed-retry-2s`").
pub const SHED_RETRY_FOR: Duration = Duration::from_secs(2);
/// The longest wait between two tries of a shed read [guess].
const SHED_BACKOFF_MAX_MS: u64 = 2_000;

/// The wait after the `n`th shed answer in a row (0 for the first): OVERLOAD_RETRY_MS doubled per shed up to
/// SHED_BACKOFF_MAX_MS, of which a random half (`r` in 0..1) is taken off, so writes shed together do not ask again
/// together; never under the `retryAfterMs` the members named (`named`).
fn shed_wait(n: u32, named: u64, r: f64) -> u64 {
    let cap = OVERLOAD_RETRY_MS.saturating_mul(1 << n.min(16)).min(SHED_BACKOFF_MAX_MS);
    (cap / 2 + (r.clamp(0.0, 1.0) * (cap / 2) as f64) as u64).max(named)
}

/// Runs `attempt` until it answers, waiting `shed_wait` after each shed answer while that wait ends before `until`
/// (ms); a shed answer past it is the read's error, naming OVERLOADED. `rand` gives each wait's jitter.
async fn retry_shed<T, A, W>(
    until: u64,
    now: impl Fn() -> u64,
    mut attempt: impl FnMut() -> A,
    mut wait: impl FnMut(u64) -> W,
    mut rand: impl FnMut() -> f64,
) -> std::result::Result<T, String>
where
    A: Future<Output = std::result::Result<T, (String, u64)>>,
    W: Future<Output = ()>,
{
    let mut n = 0;
    loop {
        match attempt().await {
            Ok(v) => return Ok(v),
            Err((why, named)) => {
                let ms = shed_wait(n, named, rand());
                n += 1;
                if now() + ms >= until {
                    return Err(why);
                }
                wait(ms).await
            }
        }
    }
}

/// One pass of `read_spread`: `Err` with the wait when every member asked shed it. A read every member declines goes
/// to the leader only with `to_leader`; without it, `Ok(Ok(None))`.
#[allow(clippy::type_complexity)]
async fn spread_once(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    body: &Value,
    to_leader: bool,
    trace: Option<&RefCell<Vec<(u64, String)>>>,
) -> std::result::Result<std::result::Result<Option<(Value, Step, Target)>, String>, (String, u64)> {
    let pass = now_ms();
    let mut tries: Vec<String> = Vec::new();
    let note = |tries: &[String], end: &str| {
        if let Some(t) = trace {
            t.borrow_mut().push((now_ms().saturating_sub(pass), format!("{chain} {} {end}", tries.join(" "))));
        }
    };
    let known = KNOWN.with(|k| k.borrow().get(&chain).cloned()).unwrap_or_default();
    let voters = if known.voters.is_empty() {
        record.bootstrap_members(chain).into_iter().map(|m| (m.site, m.do_name.to_string())).collect()
    } else {
        known.voters
    };
    let pick = (js_sys::Math::random() * 1024.0) as usize;
    let now = now_ms();
    let learners: Vec<(SiteId, String, Option<f64>)> = ANSWER_MS.with(|m| {
        let m = m.borrow();
        known.learners.into_iter().map(|(s, n)| (s, n.clone(), m.get(&(s, n)).and_then(|t| t.score(now)))).collect()
    });
    let n_learners = learners.len();
    // A whole-tier dump or an `ls-refs` never goes on to the leader, unless the group has no other member to serve it
    // (a `Solo` group: one voter, no learners), whose leader serves every read.
    let alone = voters.len() <= 1 && n_learners == 0;
    let leader_ok = alone || !body.get("q").is_some_and(leader_never_serves);
    let mut indexed = body.clone();
    indexed["index"] = json!(true);
    let bytes = indexed.to_string();
    let mut answers = Vec::new();
    // The learners come first in the order, so the first `n_learners` asked are learners, and only they are timed.
    for (i, t) in timed_spread_order(learners, voters, known.leader, pick).into_iter().enumerate() {
        let start = now_ms();
        let r = post_by(b, t.site, &t.do_name, "/read", bytes.as_bytes(), FOLLOWER_CALL, SentBy::Spread).await;
        let seen = r.as_ref().map(|(s, v)| (*s, v));
        if trace.is_some() {
            tries.push(try_line(t.site, i < n_learners, now_ms().saturating_sub(start), seen));
        }
        if i < n_learners {
            let sample = answer_sample(seen, now_ms().saturating_sub(start) as f64);
            ANSWER_MS.with(|m| {
                let mut m = m.borrow_mut();
                let key = (t.site, t.do_name.clone());
                let a = Timed::after(m.get(&key).copied(), sample, now_ms());
                m.insert(key, a);
            });
        }
        let usable = follower_answer_usable(seen);
        if let Some((_, v)) = &r {
            learn_config(chain, v);
            // A leader that declined a whole-tier read names itself: this isolate leaves it out of its next pass.
            if let Some(nl) = v.pointer("/error/notLeader").filter(|nl| !leader_ok && nl["leader"].is_u64()) {
                learn_leader_from_follower(
                    chain,
                    &json!({ "meta": { "term": nl.get("term"), "leader": nl.get("leader") } }),
                );
            }
        }
        answers.push(r);
        if usable {
            let v = answers.pop().flatten().map(|(_, v)| v).unwrap_or_default();
            learn_leader_from_follower(chain, &v);
            refresh_config(b, chain, &t, &v).await;
            note(&tries, "use");
            return Ok(Ok(Some((v, Step::Cached, t))));
        }
    }
    let seen: Vec<Option<(u16, &Value)>> = answers.iter().map(|r| r.as_ref().map(|(s, v)| (*s, v))).collect();
    let verdict = spread_verdict(&seen, leader_ok);
    let to_leader_body = leader_body(&verdict, body);
    if trace.is_some() && !(to_leader && verdict == Spread::Leader) {
        let end = match &verdict {
            Spread::Shed(ms) => format!("shed:{ms}"),
            v => format!("{v:?}").to_lowercase(),
        };
        note(&tries, &end);
    }
    match verdict {
        Spread::Use(_) => unreachable!("a usable answer returned above"),
        Spread::Absent => Ok(Ok(None)),
        Spread::TierOff => Err((tier_off_why(chain), OVERLOAD_RETRY_MS)),
        Spread::Elect => {
            // The lookup's answer is not the read's: the read is shed, and its retry finds a follower with a leader.
            let probe = to_leader_body.unwrap_or_default();
            let _ = call_opt(b, chain, record, "/read", probe.as_bytes()).await;
            Err((tier_off_why(chain), OVERLOAD_RETRY_MS))
        }
        Spread::Shed(ms) => {
            // A member that shed it because its read-index call to the leader went unanswered (F67 rule 12) names
            // that leader: one `suspect` report about it, damped by SUSPECT_GAP, stands in for the leader walk a
            // `notLeader` answer used to start, so a half-hung leader is still voted off (`b032-shed`).
            if let Some((site, term)) = forgot_of(&seen)
                && let Some(about) = known_voters(chain).into_iter().find(|v| v.site == site)
            {
                let colo = COLO.with(|c| c.get());
                suspect(b, chain, &about, term, colo, false).await;
            }
            Err((format!("{chain}: every member asked shed the read: {OVERLOADED}"), ms))
        }
        Spread::Leader if !to_leader => Ok(Ok(None)),
        Spread::Leader => {
            let at = now_ms();
            let r = call_opt(b, chain, record, "/read", to_leader_body.unwrap_or_default().as_bytes()).await;
            note(&tries, &format!("leader:{}", now_ms().saturating_sub(at)));
            Ok(r)
        }
    }
}

/// The leader a shed answer says its member forgot (`"forgot": {"site", "term"}`, a site's read-index read that failed
/// naming no leader after F67 rule 12), with the highest term among the answers.
fn forgot_of(answers: &[Option<(u16, &Value)>]) -> Option<(SiteId, u64)> {
    answers
        .iter()
        .filter_map(|r| {
            let f = r.as_ref()?.1.get("forgot")?;
            Some((SiteId(u8::try_from(f.get("site")?.as_u64()?).ok()?), f.get("term")?.as_u64()?))
        })
        .max_by_key(|(_, t)| *t)
}

/// Learns the config a member's `notLeader` answer carries.
fn learn_config(chain: ChainId, v: &Value) {
    if let Some(nl) = v.pointer("/error/notLeader").or_else(|| v.pointer("/done/notLeader")) {
        learn(chain, nl);
    }
}

/// A value's `meta.configIndex` past the config this isolate holds means it has not seen the group's learners, or
/// learners were added since: the answering member's `/status` names them. Marked before the call, so one isolate
/// asks once per config, and unmarked if the call fails.
async fn refresh_config(b: &Bindings<'_>, chain: ChainId, from: &Target, v: &Value) {
    let Some(index) = v.pointer("/meta/configIndex").and_then(Value::as_u64) else { return };
    let had = KNOWN.with(|k| {
        let mut k = k.borrow_mut();
        let e = k.entry(chain).or_default();
        let had = e.config_index;
        if index > had {
            e.config_index = index;
        }
        had
    });
    if index <= had {
        return;
    }
    let status = async {
        let mut r = b.member(from.site, &from.do_name).ok()?.fetch_with_str("https://member/status").await.ok()?;
        r.json::<Value>().await.ok()
    };
    match within(CALL, status).await.flatten().and_then(|st| st.get("config").cloned()).filter(|c| !c.is_null()) {
        Some(config) => learn(chain, &json!({ "config": config })),
        None => KNOWN.with(|k| {
            if let Some(e) = k.borrow_mut().get_mut(&chain).filter(|e| e.config_index == index) {
                e.config_index = had;
            }
        }),
    }
}

/// The word in `call_opt`'s error when the leader kept shedding the read (`overloaded_reply`): the caller answers 503
/// with a short `Retry-After`.
pub const OVERLOADED: &str = "overloaded";
/// How many times `call_opt` asks a member that answered `overloaded` again before it gives up.
const OVERLOAD_TRIES: u32 = 4;
/// The first wait before `call_opt` asks an object whose fetch threw `overloaded` again, doubled per such answer up to
/// SHED_BACKOFF_MAX_MS [guess]: on B0-35 m60 the leader's turns were blocked for 2.7 s, then 2.5 s.
const OBJECT_WAIT: Duration = Duration::from_millis(250);
/// The wait before a shed spread read is asked again when no member named one (`Spread::Shed`): it timed out or was
/// `busy`. The members' own `retryAfterMs` for a shed read.
const OVERLOAD_RETRY_MS: u64 = 250;

/// A member's answer to a heavy read past its in-flight cap: a 200, so it is never a missed call, with `busy`, so a
/// lookup built before `overloaded` skips it without a `suspect` report too.
pub fn overloaded_reply(retry_after_ms: u64) -> Value {
    json!({ "busy": true, "overloaded": { "retryAfterMs": retry_after_ms } })
}

/// How `call_opt` takes one member's answer.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    /// No answer in CALL, or a 5xx: reported with `suspect` at the end of the pass.
    Missed(String),
    /// A bootstrap name's 409.
    NotJoined,
    /// Another status, or `busy` (a read past its deadline): the next candidate, with no report.
    Skip(String),
    /// Shed by the member's heavy-read cap: asked again after this many ms, with no report.
    Overloaded(u64),
    /// The object itself is overloaded (a thrown fetch with `overloaded`): not missed, not walked away from, and not
    /// asked again until OBJECT_WAIT has passed, doubling per answer, within LOOKUP_FOR.
    ObjectOverloaded(String),
    /// An answer to read: the value, or `NotLeader`.
    Reply,
}

fn classify(r: Option<(u16, &Value)>, bootstrap: bool) -> Answer {
    let Some((status, v)) = r else { return Answer::Missed("no answer".into()) };
    if status == THROWN {
        let mut why = format!("thrown {:?}", v["thrown"].as_str().unwrap_or_default());
        for k in ["retryable", "remote"].into_iter().filter(|k| v[*k] == true) {
            why += &format!(" {k}");
        }
        return if v["overloaded"] == true { Answer::ObjectOverloaded(why) } else { Answer::Missed(why) };
    }
    if status == 409 && bootstrap {
        return Answer::NotJoined;
    }
    if status != 200 {
        return if status >= 500 { Answer::Missed(status.to_string()) } else { Answer::Skip(status.to_string()) };
    }
    if let Some(o) = v.get("overloaded") {
        return Answer::Overloaded(o.get("retryAfterMs").and_then(Value::as_u64).unwrap_or(250));
    }
    if v.get("busy").is_some() {
        return Answer::Skip("busy".into());
    }
    Answer::Reply
}

/// `call_opt` for a group that must exist: no group is an error, which `is_no_group` reads.
pub async fn call_raw(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    path: &str,
    body: &[u8],
) -> std::result::Result<(Value, Step, Target), String> {
    call_opt(b, chain, record, path, body).await?.ok_or_else(|| format!("{chain}: {NO_GROUP}"))
}

/// The end of `call_raw`'s error when the chain has no group: on the rule's first pass every member it reached
/// answered that it is not joined and none answered any other way (`call_opt`). A group that is only down, moving, or
/// changing leaders has members that answer otherwise, so it never ends an error this way.
pub const NO_GROUP: &str = "no group";

/// Whether a call's or a commit's error says the chain has no group: `call_raw`'s error, or `commit_as` giving up
/// with it as its last attempt's error. Any other failure (no leader, a timeout, a refusal, a shed) is not, so a
/// caller that ends work on no group retries those (`decisions/DECISIONS.md`, "`repo-gone`").
pub fn is_no_group(e: &str) -> bool {
    e.ends_with(&format!(": {NO_GROUP}"))
}

/// The longest a dependency read waits for its first request before it sends a second one, and the wait before
/// the chain has DEP_HEDGE_MIN read times in this isolate (`dep_hedge_after`) [guess]: past `deps:authz` p99 (672 to
/// 725 ms on B0-31 m3fr12, an apac leader reading enam).
pub const DEP_HEDGE: Duration = Duration::from_millis(1_000);
/// The shortest wait before a dependency read's second request [guess]: run 5's `deps:names` p90 is about 76 ms, and
/// a hedge sooner than this would mostly race a first request that is already on its way back.
pub const DEP_HEDGE_FLOOR: Duration = Duration::from_millis(100);
/// How many of a chain's latest read times `dep_hedge_after` takes its p90 over, and how many it needs first.
const DEP_HEDGE_SAMPLES: usize = 64;
const DEP_HEDGE_MIN: usize = 16;

/// How long `chain`'s next dependency read waits before a second request (`dep-hedge-adaptive`): the p90 of its last
/// DEP_HEDGE_SAMPLES read times in this isolate, within DEP_HEDGE_FLOOR and DEP_HEDGE, or DEP_HEDGE before it has
/// DEP_HEDGE_MIN. So about one read in ten is hedged whether the chain's leader is near or a continent away: a fixed
/// 150 ms would hedge about 80% of B0-31 m3fr12's reads of enam from apac and 5% of B0-29 run 5's
/// (`tests/m0/results/dep-hedge-adaptive.md`).
fn dep_hedge_after(chain: ChainId) -> Duration {
    DEP_MS.with(|m| {
        let m = m.borrow();
        let Some(times) = m.get(&chain).filter(|t| t.len() >= DEP_HEDGE_MIN) else { return DEP_HEDGE };
        let mut sorted = times.clone();
        sorted.sort_unstable();
        Duration::from_millis(u64::from(sorted[sorted.len() * 9 / 10])).clamp(DEP_HEDGE_FLOOR, DEP_HEDGE)
    })
}

/// One more of `chain`'s dependency read times, the answer kept, hedged or not; the oldest past DEP_HEDGE_SAMPLES goes.
fn dep_read_took(chain: ChainId, ms: u64) {
    DEP_MS.with(|m| {
        let mut m = m.borrow_mut();
        let times = m.entry(chain).or_default();
        if times.len() >= DEP_HEDGE_SAMPLES {
            times.remove(0);
        }
        times.push(ms.min(u64::from(u32::MAX)) as u32);
    })
}

/// How a hedged call ended: answered by its first request with no second sent, by the first after a second was sent,
/// or by the second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hedged {
    First,
    Asked,
    Won,
}

/// `first`, and if it has not ended when `after` does, `second()` beside it; the first `Ok` of the two, else the
/// first's error. Neither is cancelled by the other's error, and the loser of a race is dropped unread.
pub async fn hedge<T, E, A, D, B, BF>(first: A, after: D, second: B) -> (Result<T, E>, Hedged)
where
    A: Future<Output = Result<T, E>>,
    D: Future<Output = ()>,
    B: FnOnce() -> BF,
    BF: Future<Output = Result<T, E>>,
{
    let first = Box::pin(first);
    let first = match select(first, Box::pin(after)).await {
        Either::Left((r, _)) => return (r, Hedged::First),
        Either::Right((_, first)) => first,
    };
    match select(first, Box::pin(second())).await {
        Either::Left((Ok(v), _)) => (Ok(v), Hedged::Asked),
        Either::Right((Ok(v), _)) => (Ok(v), Hedged::Won),
        Either::Left((Err(e), second)) => match second.await {
            Ok(v) => (Ok(v), Hedged::Won),
            Err(_) => (Err(e), Hedged::Asked),
        },
        Either::Right((Err(_), first)) => (first.await, Hedged::Asked),
    }
}

/// A leader's dependency read (`design/DESIGN.md`, Shape, \"The write path\", `dep-hedge`): `call` of `body` on
/// `chain`, and if it has not answered within the chain's delay (`dep_hedge_after`), a second `call` beside it, a new request to the leader the
/// isolate then holds (a dependency read is the leader's own `fresh`, so no follower may answer it), and the first
/// answer of the two. Both start after the read was asked, and each is answered
/// only by a leader through `fresh`, so the read the caller keeps is one `fresh` read made after the batch's commands
/// were admitted, whichever it is. F83: single requests to an identity leader were held 4.6 to 5.4 s before its
/// handler while it served a later request from the same isolate in 186 ms.
pub async fn call_hedged(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    path: &str,
    body: &Value,
) -> (std::result::Result<(Value, Step), String>, Hedged) {
    let bytes = body.to_string();
    let one = || async { call_raw(b, chain, record, path, bytes.as_bytes()).await.map(|(v, s, _)| (v, s)) };
    dep_read(chain, one, Delay::from, now_ms).await
}

/// `call_hedged`'s rule over any read, sleep and clock: `read()` now, and `read()` again beside it after
/// `dep_hedge_after(chain)`; an answer's time joins the chain's window.
async fn dep_read<T, E, R, RF, D, DF>(
    chain: ChainId,
    read: R,
    delay: D,
    now: impl Fn() -> u64,
) -> (Result<T, E>, Hedged)
where
    R: Fn() -> RF,
    RF: Future<Output = Result<T, E>>,
    D: Fn(Duration) -> DF,
    DF: Future<Output = ()>,
{
    let began = now();
    let got = hedge(read(), delay(dep_hedge_after(chain)), &read).await;
    if got.0.is_ok() {
        dep_read_took(chain, now().saturating_sub(began));
    }
    got
}

/// `call_raw` with a JSON body.
pub async fn call(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    path: &str,
    body: &Value,
) -> std::result::Result<(Value, Step), String> {
    call_raw(b, chain, record, path, body.to_string().as_bytes()).await.map(|(v, s, _)| (v, s))
}

/// A fresh request id: the time, then 80 random bits.
pub fn request_id() -> RequestId {
    let mut rand = [0u8; 10];
    for b in &mut rand {
        *b = (js_sys::Math::random() * 256.0) as u8;
    }
    RequestId(Ulid::new(Timestamp(now_ms()), rand))
}

/// Commits `cmd` (its serde JSON) on `chain` to an outcome under request id `rid`: re-sent under the same id after
/// `NotLeader`, a missed reply, or a pending ticket, as an edge does. Returns the outcome JSON (`ok` or `refused`).
pub async fn commit_as(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    rid: RequestId,
    cmd: Value,
) -> std::result::Result<Value, String> {
    let rid = rid.to_string();
    let end = now_ms() + LOOKUP_FOR.as_millis() as u64 * 2;
    let mut last = String::new();
    while now_ms() < end {
        match call(b, chain, record, "/commit", &json!({ "rid": rid, "cmd": cmd })).await {
            Ok((v, _)) => {
                if let Some(done) = v.get("done") {
                    if done.get("notLeader").is_some() {
                        forget_leader(chain);
                        continue;
                    }
                    return Ok(done.clone());
                }
                if v.get("admitted").is_some() {
                    // Held past COMMIT_HOLD: the same request id again joins the ticket, or reads its row.
                    Delay::from(Duration::from_millis(200)).await;
                    continue;
                }
                last = v.to_string();
                if v.pointer("/error/unsupported").is_some() || v.pointer("/error/tooLarge").is_some() {
                    return Err(last);
                }
            }
            Err(e) => last = e,
        }
        Delay::from(Duration::from_millis(200)).await;
    }
    Err(format!("{chain}: commit gave up: {last}"))
}

/// `commit_as` under a fresh request id.
pub async fn commit(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    cmd: Value,
) -> std::result::Result<Value, String> {
    commit_as(b, chain, record, request_id(), cmd).await
}

// ---- the edge's commit: one member at a time, hedged only by `suspect` ----

/// The receipt deadline of a commit: COMMIT_HOLD plus CALL_DEADLINE, so a `/commit` held its whole hold and answered
/// within one call is the send's reply with no ask (`commit-hold`).
pub const COMMIT_RECEIPT: Duration = Duration::from_millis(
    graftyard_raft::COMMIT_HOLD.as_millis() as u64 + graftyard_raft::CALL_DEADLINE.as_millis() as u64,
);
/// How long one `/commit` send stays open while its member is `busy` (`receipt`) [guess]: past B0-32 m3fr8's longest
/// leader handler, 3.8 s, plus both legs (`tests/am1/results/lm1-rows-m3f.md`, "m3fr8 after the run"). Past it the
/// command is sent again, so a send whose reply was lost costs this at most once.
pub const RECEIPT_OPEN: Duration = Duration::from_millis(8_000);

/// The member the caller last saw lead `chain`: from its config, else the member that answered as leader.
pub fn cached_leader(chain: ChainId) -> Option<Target> {
    let k = KNOWN.with(|k| k.borrow().get(&chain).cloned())?;
    let leader = k.leader?;
    k.voters
        .into_iter()
        .find(|(s, _)| *s == leader)
        .map(|(site, do_name)| Target { site, do_name })
        .or(k.answered.filter(|t| t.site == leader))
}

/// The highest term a reply from `chain` carried to this isolate, 0 before any.
fn known_term(chain: ChainId) -> u64 {
    KNOWN.with(|k| k.borrow().get(&chain).map_or(0, |e| e.term))
}

/// The voters the caller knows for `chain`, from the last config it learned.
pub fn known_voters(chain: ChainId) -> Vec<Target> {
    KNOWN
        .with(|k| k.borrow().get(&chain).map(|e| e.voters.clone()).unwrap_or_default())
        .into_iter()
        .map(|(site, do_name)| Target { site, do_name })
        .collect()
}

/// Learns `chain`'s config from its members' `/status` when this isolate knows no voters, keeping the highest index
/// any answers. A caller that picks a site's member by name, as a git cache shard's local `/entries` read does,
/// otherwise has only the record's bootstrap names, and a voter replaced since is another object: the shard's read
/// reached site b's retired first voter, which answered no entries, and every page of a rehydrate's tail went to the
/// leader instead (`tests/am1/results/rehydrate-batch.md`, "Resume, hosted").
pub async fn learn_voters(b: &Bindings<'_>, chain: ChainId, record: &PlacementRecord) {
    if !known_voters(chain).is_empty() {
        return;
    }
    let asks = record.bootstrap_members(chain).into_iter().map(|m| async move {
        let mut r = b.member(m.site, m.do_name.as_ref()).ok()?.fetch_with_str("https://member/status").await.ok()?;
        within(CALL, r.json::<Value>()).await?.ok()
    });
    for st in futures_util::future::join_all(asks).await.into_iter().flatten() {
        if let Some(config) = st.get("config").filter(|c| !c.is_null()) {
            learn(chain, &json!({ "config": config }));
        }
    }
}

/// The body of a git cache shard's `ranges` read on its own site's member: by read-index, so a follower answers it at
/// or above the commit index the leader confirmed when it arrived, as the leader would (`local_first`).
pub const LOCAL_RANGES: &str = r#"{"q":{"ranges":true},"index":true}"#;

/// Whether a member's answer to `LOCAL_RANGES` is the fold's ranges: a 200 with a value, not `NotLeader`, `busy`, or
/// `overloaded` (a follower behind the leader's read index, or an older release that keeps `ranges` on the leader).
pub fn local_ranges_usable(v: &Value) -> bool {
    follower_answer_usable(Some((200, v))) && v.get("error").is_none() && v["value"].is_object()
}

/// Whether a member's `/entries?local=1` answer is the page that starts at `from`: committed entries are the same on
/// every member, and one whose commit is below `from` answers a page that does not start there.
pub fn local_page_usable(v: &Value, from: u64) -> bool {
    v["entries"].as_array().and_then(|a| a.first()).and_then(|e| e["index"].as_u64()) == Some(from)
}

/// A git cache shard's catch-up read of its group's log, its own site's member first (`decisions/DECISIONS.md`,
/// `catchup-local`): `post` sends the read, `path` in the trace, to `local`, and its answer is the read's when `usable`
/// takes it.
/// No local member, a miss, or any other answer goes to `leader`, the lookup to the group's leader, which is the read
/// before this rule and waits on the isolate's lookup mark while another request's walk holds it; the local read
/// takes no mark. On B0-35 m59 one catch-up of a single entry took 21 s while 13 walks in a row held its chain's mark
/// in the shard isolate (F104). A local read not taken is logged for a traced chain (`trace_marks`).
pub async fn local_first<P, PF, L, LF>(
    chain: ChainId,
    now: impl Fn() -> u64,
    local: Option<Target>,
    path: &str,
    usable: impl Fn(&Value) -> bool,
    post: P,
    leader: L,
) -> std::result::Result<Value, String>
where
    P: FnOnce(Target) -> PF,
    PF: Future<Output = Option<(u16, Value)>>,
    L: FnOnce() -> LF,
    LF: Future<Output = std::result::Result<Value, String>>,
{
    let Some(t) = local else {
        mark_trace(chain, || format!("local {path}: no member on this site, to the leader"));
        return leader().await;
    };
    let t0 = now();
    let r = post(t.clone()).await;
    match r {
        Some((200, v)) if usable(&v) => Ok(v),
        r => {
            mark_trace(chain, || {
                let why = r.map_or("no answer".into(), |(s, v)| {
                    format!("{s} {}", v.to_string().chars().take(120).collect::<String>())
                });
                format!("local {path} at {}/{}: {why} in {} ms, to the leader", t.site.0, t.do_name, now() - t0)
            });
            leader().await
        }
    }
}

/// A `suspect` report about `about` to every other known voter (to every voter, `about` included, when `all`), in
/// parallel; returns the voter that answered it leads, if one did. The report carries no command. `all` is for a
/// member that answered `NotLeader` naming no leader: campaigns are request-driven, so a group whose leader died
/// stays leaderless until a report reaches its members.
pub async fn suspect(
    b: &Bindings<'_>,
    chain: ChainId,
    about: &Target,
    term: u64,
    colo: u32,
    all: bool,
) -> Option<Target> {
    if !report_due(chain, about.site, term, now_ms()) {
        return None;
    }
    let body = json!({ "colo": colo, "about": about.site.0, "term": term }).to_string();
    let others: Vec<Target> = known_voters(chain).into_iter().filter(|v| all || v.site != about.site).collect();
    let calls = others.iter().map(|v| post(b, v.site, &v.do_name, "/suspect", body.as_bytes()));
    let answers = futures_util::future::join_all(calls).await;
    others.into_iter().zip(answers).find_map(|(v, a)| {
        let verdict = a?.1.get("verdict")?.as_str()?.to_owned();
        matches!(verdict.as_str(), "leader" | "won" | "resumed").then_some(v)
    })
}

/// How often one isolate sends the same `suspect` report: about one member, in one term, to the same voters. A
/// member's verdict on a repeat inside this gap is the one it just gave, so a campaign is late by at most this much,
/// while a leader that sheds or drops reads drew up to 150,000 reports a run from every edge isolate's lookups
/// (`tests/am1/results/lm1-memory.md`).
/// 1 s since lookup-retry: at 500 ms each follower took 177 to 186 a second on B0-33 m45, 37 to 39% of what waited
/// ahead of its slow `/raft` calls, and 1 s keeps about half (`tests/am1/results/lm1-memory.md`, round 40). 5 s cut
/// more but left 7 of the long sweep's 4,000 live seeds past the 30 s bound (worst 42.5 s), and 2.5 s one, where 1 s
/// fails only the seed 500 ms fails (`tests/am1/results/lm1-memory.md`, "lookup-retry").
pub const SUSPECT_GAP: Duration = Duration::from_secs(1);

thread_local! {
    static REPORTED: RefCell<HashMap<(ChainId, SiteId, u64), u64>> = RefCell::new(HashMap::new());
}

/// Whether this isolate sends the report now (`SUSPECT_GAP`), recording it if so.
fn report_due(chain: ChainId, about: SiteId, term: u64, now: u64) -> bool {
    REPORTED.with(|r| {
        let mut r = r.borrow_mut();
        let gap = SUSPECT_GAP.as_millis() as u64;
        if r.len() > 4_096 {
            r.retain(|_, at| now < *at + gap);
        }
        match r.get(&(chain, about, term)) {
            Some(at) if now < *at + gap => false,
            _ => {
                r.insert((chain, about, term), now);
                true
            }
        }
    })
}

/// `commit_hedged`'s options. `first`, when given, is the body of the first `/commit` send only (a test install's
/// kill rides on it, so a fault fires once); every later send is the command's body. `trace`, when given, collects one
/// row per step: each `/commit` send with its target and why the edge chose it, each missed receipt, and each
/// `suspect` answer (BF-14's count). `lookup`, when given, is the body `/outcome_by_request` gets while a send is open
/// (`receipt`): the same request id and a command whose `replay` answers the same, without bytes `replay` never reads,
/// such as a push's inline pack. Without it the command's own body is sent.
#[derive(Clone, Copy, Default)]
pub struct Hooks<'a> {
    pub first: Option<&'a [u8]>,
    pub trace: Option<&'a RefCell<Vec<Value>>>,
    pub lookup: Option<&'a [u8]>,
    /// Test installs, the leader-sync A/B's arm B (`tests/am1/results/sync-ab.md`): each `/commit` send asks the
    /// leader to hold it up to LEAN_HOLD_MS for its outcome (`held_body`), so a commit slower than COMMIT_HOLD needs no
    /// `/outcome` call, and no `/outcome_by_request` ask goes out while a send is open (`send_only`). The client's
    /// answer is the same outcome or the same `admitted` ticket; only the number of calls to the leader changes.
    #[cfg(any(test, feature = "test-faults"))]
    pub lean: bool,
}

/// How long a lean send asks its leader to hold a `/commit` for its outcome (`Hooks::lean`) [guess]: under
/// RECEIPT_OPEN by both legs and a slow turn, so the held answer comes back before the send's own deadline.
#[cfg(any(test, feature = "test-faults"))]
pub const LEAN_HOLD_MS: u64 = 6_000;

/// A `/commit` body (`{"rid", "cmd", ..}`) with `"hold_ms": LEAN_HOLD_MS` first, for a lean send (`Hooks::lean`). The
/// field is spliced after the opening brace, so a push's inline pack is not parsed again. A body that is not a JSON
/// object goes as it came.
#[cfg(any(test, feature = "test-faults"))]
fn held_body(body: &[u8]) -> Vec<u8> {
    match body.strip_prefix(b"{") {
        Some(rest) => {
            let field = format!("{{\"hold_ms\":{LEAN_HOLD_MS}");
            let comma = if rest.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'}') { "" } else { "," };
            [field.as_bytes(), comma.as_bytes(), rest].concat()
        }
        None => body.to_vec(),
    }
}

thread_local! {
    /// Per isolate: when each member last answered a commit-path call from this isolate with a 200, and that answer's
    /// term (`heard`, `busy`).
    static HEARD: RefCell<HashMap<(ChainId, SiteId), (u64, u64)>> = RefCell::new(HashMap::new());
}

/// Records a member's 200 answer `v` on `chain`'s commit path at `now`, with the term its `meta` names.
fn heard(chain: ChainId, site: SiteId, v: &Value, now: u64) {
    let Some(term) = v.pointer("/meta/term").and_then(Value::as_u64) else { return };
    HEARD.with(|h| {
        let mut h = h.borrow_mut();
        if h.len() > 4_096 {
            h.clear();
        }
        h.insert((chain, site), (term, now));
    });
}

/// Whether `site` answered this isolate a 200 on `chain`'s commit path within COMMIT_RECEIPT of `now`, in `term` or
/// the highest term this isolate has seen from the group, whichever is higher: a member that misses a receipt then is
/// busy, not gone, and is not reported (`receipt`, `design/DESIGN.md`, "Commit and read paths").
fn busy(chain: ChainId, site: SiteId, term: u64, now: u64) -> bool {
    let within = COMMIT_RECEIPT.as_millis() as u64;
    let term = term.max(KNOWN.with(|k| k.borrow().get(&chain).map_or(0, |e| e.term)));
    HEARD
        .with(|h| h.borrow().get(&(chain, site)).is_some_and(|(t, at)| *t >= term && now.saturating_sub(*at) <= within))
}

/// What one `/commit` send came to.
#[derive(Debug, PartialEq)]
enum Receipt {
    /// A 200 to take as the send's reply: the send's own, in time or late, or the leader's answer by request id while
    /// the send was open, `done`, `error` with `notLeader`, or `pending` as `admitted`.
    Reply(Value),
    /// No usable answer: no 200 within COMMIT_RECEIPT from a member that is not `busy`, not even to the ask, an error
    /// status, or no answer by RECEIPT_OPEN. The command is sent again; `report` says whether the member is also
    /// reported with `suspect`: always for an error status, and otherwise only when it is not `busy`.
    Missed { report: bool },
}

/// One `/commit` send's receipt (`design/DESIGN.md`, "Commit and read paths"). `send` is the call, open until its own
/// deadline; `ask` is one `/outcome_by_request` to the same member, whose 200 the caller records (`heard`); `busy` is
/// `busy` for that member at the moment it is asked; `wait` sleeps. Each COMMIT_RECEIPT the send stays open, the
/// member is asked by request id. Any 200 to the ask means it answers, so the send stays open; a member with no 200
/// to the ask that is not `busy` either is a missed receipt. Only the send's own 200 or a by-request `done`,
/// `NotLeader`, or `pending` is a reply: `unknown`, any other answer, or none leaves the send open, so nothing here
/// makes a 2xx that the leader's `outcome` did not. Asked only when already `busy`, a leader whose held `/commit`
/// answers all came back past 2 s was never `busy` at the tick, though its `/outcome_by_request` answered within
/// 0.7 s: B0-32 m3fr11's 103 `suspect` reports and resends, every one in the burst's first 5 s with term 2 held
/// (`tests/am1/results/lm1-rows-m3f.md`, "m3fr11 after the run").
async fn receipt<S, A, AF, W, WF>(send: S, mut ask: A, busy: impl Fn() -> bool, mut wait: W) -> Receipt
where
    S: Future<Output = Option<(u16, Value)>>,
    A: FnMut() -> AF,
    AF: Future<Output = Option<(u16, Value)>>,
    W: FnMut(Duration) -> WF,
    WF: Future<Output = ()>,
{
    // An error status is a sick member's answer, reported whatever came before (`STEP_DOWN`'s gray case); only a send
    // with no answer, at RECEIPT_OPEN or a failed fetch, is spared while the member is `busy`.
    let ended = |r: Option<(u16, Value)>, busy: bool| match r {
        Some((200, v)) => Receipt::Reply(v),
        Some(_) => Receipt::Missed { report: true },
        None => Receipt::Missed { report: !busy },
    };
    let mut send = Box::pin(send);
    loop {
        if let Either::Left((r, _)) = select(send.as_mut(), Box::pin(wait(COMMIT_RECEIPT))).await {
            return ended(r, busy());
        }
        match select(send.as_mut(), Box::pin(ask())).await {
            Either::Left((r, _)) => return ended(r, busy()),
            Either::Right((Some((200, v)), _)) => {
                if let Some(p) = v.get("pending") {
                    return Receipt::Reply(json!({ "admitted": p, "meta": v.get("meta") }));
                }
                if v.get("done").is_some() || v.pointer("/error/notLeader").is_some() {
                    return Receipt::Reply(v);
                }
            }
            Either::Right(_) if !busy() => return Receipt::Missed { report: true },
            Either::Right(_) => {}
        }
    }
}

/// `receipt` with no ask (`Hooks::lean`): the send's 200 is the reply, an error status is reported, and a send with
/// no answer by RECEIPT_OPEN is sent again, reported only when the member is not `busy`, as `receipt` ends a send.
#[cfg(any(test, feature = "test-faults"))]
async fn send_only(send: impl Future<Output = Option<(u16, Value)>>, busy: impl Fn() -> bool) -> Receipt {
    match send.await {
        Some((200, v)) => Receipt::Reply(v),
        Some(_) => Receipt::Missed { report: true },
        None => Receipt::Missed { report: !busy() },
    }
}

/// A command's commit as the edge runs it (`design/DESIGN.md`, "Commit and read paths"): the leader is found with
/// `/outcome_by_request`, which only a leader answers and which returns the request's outcome if a re-send finds it
/// already decided; the command (`body`, `{"rid", "cmd"}` under one request id) then goes to that one member. A
/// missed receipt is hedged by `suspect` to the other voters, and the command is sent again to the same member unless
/// another answered that it leads. `NotLeader` naming no leader is hedged by `suspect` to every voter, and then, unless
/// one answered that it leads, starts the lookup again; a lookup that found no leader does the same about the member
/// last sent to. Returns the reply's `done` outcome, or the
/// reply's `error` (`unsupported`, `tooLarge`) as `Err(Ok(error))`; `Err(Err(text))` when no outcome came in time.
/// `hooks` are a test install's; a product caller passes `Hooks::default()`.
pub async fn commit_hedged(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    body: &[u8],
    colo: u32,
    for_ms: u64,
    hooks: Hooks<'_>,
) -> std::result::Result<Value, std::result::Result<Value, String>> {
    #[cfg(any(test, feature = "test-faults"))]
    let Hooks { first, trace, lookup, lean } = hooks;
    #[cfg(not(any(test, feature = "test-faults")))]
    let Hooks { first, trace, lookup } = hooks;
    let lookup = lookup.unwrap_or(body);
    let start = now_ms();
    let note = |mut v: Value| {
        if let Some(t) = trace {
            v["ms"] = json!(now_ms() - start);
            t.borrow_mut().push(v);
        }
    };
    // Why the next send goes where it goes: the cached leader (`cached`), the first lookup's answer (`lookup`), the
    // previous target again (`same`), a voter that answered a `suspect` as leader (`suspectLed`), a new lookup after
    // `NotLeader` named a leader (`notLeader`), the next voter after a `NotLeader` naming none (`leaderless`), or a new
    // lookup after one that found no leader (`relookup`).
    let end = now_ms() + for_ms;
    let mut first = first;
    // The leader this isolate's last reply from the group named, if it knows one: the first send goes straight to it,
    // as "Commit and read paths" finds members "from its cached `GroupConfig`". A member that no longer leads answers
    // `NotLeader`, which starts the lookup below. Looking it up first cost one more round trip to the leader, up to
    // 190 ms from another continent (`tests/m2/results/m2-wp6.md`, "Merge latency").
    let mut target: Option<Target> = cached_leader(chain);
    let mut why = if target.is_some() { "cached" } else { "lookup" };
    let mut prev: Option<Target> = None;
    let mut tries = 0u32;
    let mut term = 0;
    let mut last = String::from("no member answered");
    while now_ms() < end {
        let t = match &target {
            Some(t) => t.clone(),
            None => match call_raw(b, chain, record, "/outcome_by_request", lookup).await {
                Ok((v, _, t)) => {
                    heard(chain, t.site, &v, now_ms());
                    if let Some(e) = v.get("error").filter(|e| e.get("notLeader").is_none()) {
                        return Err(Ok(e.clone()));
                    }
                    if let Some(done) = v.get("done").filter(|d| d.get("notLeader").is_none()) {
                        return Ok(done.clone());
                    }
                    t
                }
                Err(e) => {
                    last = e;
                    if let Some(p) = &prev {
                        target = suspect(b, chain, p, term, colo, true).await;
                        note(json!({ "suspect": p.site.0, "all": true, "led": target.as_ref().map(|t| t.site.0) }));
                        why = if target.is_some() { "suspectLed" } else { "relookup" };
                    }
                    continue;
                }
            },
        };
        // The member found is the target until a reply names another or refuses: a missed receipt or outcome is
        // sent again to it (`design/DESIGN.md`, "Commit and read paths").
        target = Some(t.clone());
        prev = Some(t.clone());
        note(json!({ "send": t.site.0, "name": t.do_name, "why": why }));
        #[cfg(not(any(test, feature = "test-faults")))]
        let send = post_by(b, t.site, &t.do_name, "/commit", first.take().unwrap_or(body), RECEIPT_OPEN, SentBy::Send);
        #[cfg(any(test, feature = "test-faults"))]
        let sent: std::borrow::Cow<[u8]> = match first.take().unwrap_or(body) {
            s if lean => held_body(s).into(),
            s => s.into(),
        };
        #[cfg(any(test, feature = "test-faults"))]
        let send = post_by(b, t.site, &t.do_name, "/commit", &sent, RECEIPT_OPEN, SentBy::Send);
        let ask = || {
            let (t, note) = (&t, &note);
            async move {
                note(json!({ "ask": t.site.0 }));
                // CALL_DEADLINE, not COMMIT_RECEIPT: a member with no 200 to it is reported 3 s after the send, not
                // 4 s. B0-32 m3fr11's leader answered these within 625 ms while its held `/commit`s took up to 4.6 s.
                let r = post_by(
                    b,
                    t.site,
                    &t.do_name,
                    "/outcome_by_request",
                    lookup,
                    graftyard_raft::CALL_DEADLINE,
                    SentBy::Ask,
                )
                .await;
                if let Some((200, v)) = &r {
                    heard(chain, t.site, v, now_ms());
                }
                r
            }
        };
        #[cfg(any(test, feature = "test-faults"))]
        let r = if lean {
            send_only(send, || busy(chain, t.site, term, now_ms())).await
        } else {
            receipt(send, ask, || busy(chain, t.site, term, now_ms()), Delay::from).await
        };
        #[cfg(not(any(test, feature = "test-faults")))]
        let r = receipt(send, ask, || busy(chain, t.site, term, now_ms()), Delay::from).await;
        let v = match r {
            Receipt::Reply(v) => v,
            Receipt::Missed { report: false } => {
                last = format!("{}: missed", t.do_name);
                note(json!({ "missed": t.site.0, "busy": true }));
                why = "same";
                Delay::from(Duration::from_millis(200)).await;
                continue;
            }
            Receipt::Missed { report: true } => {
                last = format!("{}: missed", t.do_name);
                note(json!({ "missed": t.site.0 }));
                match suspect(b, chain, &t, term, colo, false).await {
                    Some(led) => {
                        note(json!({ "suspect": t.site.0, "all": false, "led": led.site.0 }));
                        target = Some(led);
                        why = "suspectLed";
                    }
                    None => {
                        note(json!({ "suspect": t.site.0, "all": false, "led": null }));
                        why = "same";
                        Delay::from(Duration::from_millis(200)).await
                    }
                }
                continue;
            }
        };
        heard(chain, t.site, &v, now_ms());
        let nl = v.pointer("/error/notLeader").or_else(|| v.pointer("/done/notLeader"));
        let stale = nl.is_some_and(|nl| stale_answer(chain, &v, nl));
        if !stale {
            term = term.max(v.pointer("/meta/term").and_then(Value::as_u64).unwrap_or(0));
        }
        if let Some(nl) = nl {
            learn(chain, nl);
            forget_leader(chain);
            let named = nl.get("leader").and_then(Value::as_u64);
            // A removed or stale member's answer is a lookup again, not a leaderless group.
            target = match named {
                Some(_) => None,
                None if stale => None,
                None => leaderless(b, chain, &t, term, colo, &mut tries).await,
            };
            note(json!({ "notLeader": t.site.0, "leader": named, "next": target.as_ref().map(|t| t.site.0) }));
            why = if named.is_some() { "notLeader" } else { "leaderless" };
            last = format!("{}: not leader", t.do_name);
            continue;
        }
        if let Some(e) = v.get("error") {
            return Err(Ok(e.clone()));
        }
        if !readable(&v, &["done", "admitted"]) {
            return Err(Ok(unreadable()));
        }
        let mut done = v.get("done").cloned();
        if let Some(ticket) = v.get("admitted") {
            let req = json!({ "ticket": ticket, "wait_ms": 10_000 }).to_string();
            match post_within(b, t.site, &t.do_name, "/outcome", req.as_bytes(), Duration::from_secs(11)).await {
                Some((200, o)) if o.get("error").is_some() => return Err(Ok(o["error"].clone())),
                Some((200, o)) if !readable(&o, &["done", "pending"]) => return Err(Ok(unreadable())),
                Some((200, o)) => done = o.get("done").cloned(),
                _ => {
                    note(json!({ "outcomeMissed": t.site.0 }));
                    if let Some(led) = suspect(b, chain, &t, term, colo, false).await {
                        note(json!({ "suspect": t.site.0, "all": false, "led": led.site.0 }));
                        target = Some(led);
                        why = "suspectLed";
                    } else {
                        why = "same";
                    }
                    continue;
                }
            }
        }
        match done {
            Some(d) if d.get("notLeader").is_some() => {
                learn(chain, &d["notLeader"]);
                let named = d.pointer("/notLeader/leader").and_then(Value::as_u64);
                target = match named {
                    Some(_) => None,
                    None => leaderless(b, chain, &t, term, colo, &mut tries).await,
                };
                note(json!({ "notLeader": t.site.0, "leader": named, "next": target.as_ref().map(|t| t.site.0) }));
                why = if named.is_some() { "notLeader" } else { "leaderless" };
            }
            None => why = "same",
            Some(d) => {
                learn_leader(chain, &v);
                note(json!({ "done": t.site.0 }));
                return Ok(d);
            }
        }
    }
    Err(Err(format!("{chain}: no outcome: {last}")))
}

/// Whether a member's 200 answer is one of the shapes `keys` names (`{"done": ..}`, `{"admitted": ..}`, ..). Any
/// other answer is one this caller cannot read, and `commit_hedged` returns it as `Unsupported` rather than treating it
/// as a lost reply: sending the command again under its request id would answer from the request's row, and a newer
/// member's answer would reach the client as whatever an older replay says (t5541 #20, `tests/m1/results/m1-wp6.md`).
fn readable(v: &Value, keys: &[&str]) -> bool {
    v.as_object().is_some_and(|o| keys.iter().any(|k| o.get(*k).is_some_and(|x| !x.is_null())))
}

/// `Unsupported` of format 0.0, which no release reads: a member answer that does not parse.
fn unreadable() -> Value {
    json!({ "unsupported": { "family": 0, "version": 0 } })
}

/// After `about` answered `NotLeader` naming no leader: a `suspect` report to every voter, and the voter that answered
/// it leads, else the next known voter after `about` (every third time after a 200 ms pause), so the next send reaches
/// a member that may have started a campaign, and its refusal brings the next report. Campaigns are request-driven:
/// a followers' promise to a dead leader lapses only with time, and until then a report changes nothing, so the
/// edge keeps reporting rather than waiting through a whole lookup (`call_opt`), during which no member campaigns.
async fn leaderless(
    b: &Bindings<'_>,
    chain: ChainId,
    about: &Target,
    term: u64,
    colo: u32,
    tries: &mut u32,
) -> Option<Target> {
    if let Some(led) = suspect(b, chain, about, term, colo, true).await {
        return Some(led);
    }
    *tries += 1;
    if (*tries).is_multiple_of(3) {
        Delay::from(Duration::from_millis(200)).await;
    }
    let vs = known_voters(chain);
    let i = vs.iter().position(|v| v.site == about.site)?;
    vs.get((i + 1) % vs.len()).cloned()
}

// ---- creating a group on first use ----

/// Creates `chain`'s group from `record` if the lookup finds none: joins its bootstrap members with `join` (the
/// `/join` fields beyond `chain`, `me`, and `record`), each of which checks the record against its source and claims
/// the generation, then campaigns on the record's home until a member leads, and then starts the leader's near check
/// (`place`), which runs after the create returns. Joins are idempotent, and a member of a generation that lost the
/// claim refuses, so two callers at once still build one group.
pub async fn create(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    join: &Value,
) -> std::result::Result<(), String> {
    // The group exists at this record's generation: no probe, joins, or `/campaign`. A probe that the leader shed or a
    // leaderless gap failed sent the joins and a `/campaign` to the home member every 200 ms, while another site led:
    // up to 370 in 20 s, and p50 13.7 s of each B0-33 push (m42p, `tests/am1/results/lm1-memory.md`, round 37).
    if config_known(chain, record) {
        return Ok(());
    }
    // A read that answers nothing of the fold: a repository's `{"q": {}}` is every ref, and at 303,144 refs it ran
    // the leader and the edge out of memory on every commit (`tests/m2/results/m2-wp1.md`, "2026-11-04"). A member
    // built before `probe` answers it as `{"q": {}}`.
    let probe = json!({ "q": { "probe": true } }).to_string();
    if let Ok(Some(_)) = call_opt(b, chain, record, "/read", probe.as_bytes()).await {
        return Ok(());
    }
    let members = record.bootstrap_members(chain);
    let joins = members.iter().map(|m| {
        let mut j = join.clone();
        j["chain"] = json!(chain);
        j["me"] = json!(m);
        j["record"] = json!(record);
        async move { post_join(b, m.site, &m.do_name, j.to_string().as_bytes()).await }
    });
    let answers = futures_util::future::join_all(joins).await;
    let joined = answers.iter().filter(|a| matches!(a, Some((200, _)))).count();
    if joined * 2 <= members.len() {
        // Another generation may hold the chain; the lookup finds its group through the registry rows.
        return call(b, chain, record, "/read", &json!({ "q": { "probe": true } }))
            .await
            .map(|_| ())
            .map_err(|e| refused(chain, &answers, &e));
    }
    let home = members.iter().find(|m| m.site == record.home).unwrap_or(&members[0]);
    let bootstrapped = bootstrapped_here(&answers);
    let end = now_ms() + LOOKUP_FOR.as_millis() as u64;
    while now_ms() < end {
        let _ = post(b, home.site, &home.do_name, "/campaign", b"{}").await;
        if let Ok(Some((_, _, leader))) = call_opt(b, chain, record, "/read", probe.as_bytes()).await {
            return if bootstrapped { place(b, chain, record, leader).await } else { Ok(()) };
        }
        Delay::from(Duration::from_millis(200)).await;
    }
    Err(format!("{chain}: created, but no member leads"))
}

/// Whether `create`'s own `/join`s built the group in this call: a majority of the bootstrap members joined fresh.
/// A member that was already joined answers its `Rtt` rows from before (`Routes::join` measures again only past a
/// day), and one replaced since refuses as `superseded`, so a group that already exists (a lookup that found no leader
/// in a leaderless gap) is never re-placed by a lookup: B0-30's rerun ran `create` on a 25-day-old group under load,
/// whose background `/place` committed `Degraded { lost: 1 }` while site c installed, and the parent never committed
/// again (`tests/am1/results/b030-split.md`, "Cause 2"). A member's answer carries `JOINED_FRESH` only on the join that
/// built its core; `join_answer` puts it beside the answer's body.
fn bootstrapped_here(answers: &[Option<(u16, Value)>]) -> bool {
    let fresh = answers
        .iter()
        .filter(|a| matches!(a, Some((200, v)) if v.get(JOINED_FRESH) == Some(&Value::Bool(true))))
        .count();
    fresh * 2 > answers.len()
}

/// The `/join` response header a member sets on the join that built its core (`Routes::join`): the body stays the bare
/// `Rtt` rows every other caller reads.
pub const JOINED_FRESH: &str = "x-graft-joined-fresh";

/// The word in `create`'s error when a majority of the bootstrap members refused to join because the chain's source
/// holds no row for it yet: a repository whose create has not committed `RepoOwner`. The edge answers a read of it as
/// for a repository the caller cannot see, 404, as GitHub answers before a create returns; every other refusal stays
/// a 503, so an outage never reads as "not found" (`decisions/DECISIONS.md`, "`fresh-repo`").
pub const NOT_CREATED: &str = "not created yet";

/// The word in `create`'s error when a majority of the bootstrap members refused to join because they are retired
/// (`{"refused": {"retired": at}}`, `retire-sticks`), or retired and not created between them: the group was retired
/// and its repository's reads find nothing to serve. The edge answers it 404 as it does `NOT_CREATED`, so a retired
/// test repository reads like a deleted one and a client does not retry it (`decisions/DECISIONS.md`, "`retired-gone`").
pub const RETIRED: &str = "retired group";

/// Whether `create`'s error says the chain has no group to read and never will without a new create: not created yet
/// (`NOT_CREATED`) or retired (`RETIRED`). The edge answers both as a repository the caller cannot see.
pub fn is_gone(e: &str) -> bool {
    e.contains(NOT_CREATED) || e.contains(RETIRED)
}

/// `create`'s error when too few members joined and no group answered (`probe`). It names `NOT_CREATED` only when a
/// majority of the bootstrap members answered `{"refused": {"noRepoOwner": ..}}`: each read `RepoOwner` through its
/// `Names` leader and found no row, which no outage answers. A member that did not answer, refused for any other
/// reason, or answered from a release before `noRepoOwner` counts against it.
fn refused(chain: ChainId, answers: &[Option<(u16, Value)>], probe: &str) -> String {
    let refusing =
        |key: &str| answers.iter().filter(|a| matches!(a, Some((409, v)) if v["refused"].get(key).is_some())).count();
    let (unsourced, retired) = (refusing("noRepoOwner"), refusing("retired"));
    let why = match (unsourced + retired) * 2 > answers.len() {
        true if retired > 0 => format!("{RETIRED}: "),
        true => format!("{NOT_CREATED}: "),
        false => String::new(),
    };
    let joined = answers.iter().filter(|a| matches!(a, Some((200, _)))).count();
    format!("{chain}: {why}{joined} of {} joined ({answers:?}); {probe}", answers.len())
}

/// How long `create` waits for a new group's `/place`. A member that takes `background` answers before it measures
/// anything; one built before it ignores the field and answers after PLACE_TRIES joins, each measuring two voters
/// 20 times, plus three config changes [inference from apac round trips of about 250 ms], so the bound stays.
const PLACE_FOR: Duration = Duration::from_secs(240);

/// Starts the near check on a new group (`design/DESIGN.md`, "Placement is checked by latency"): its leader
/// re-places a voter until the home has one within `NEAR_RTT`, or keeps the nearest try, under `wait_until` after it
/// answers (`{"background": true}`). Each try's join measures 20 round trips to each peer, so a home over the bound
/// on every try held a create for about 60 to 95 s, and a read of the repository in that time found no `RepoOwner`
/// row and failed "0 of 3 joined" (`tests/am1/results/fresh-repo.md`). The group serves while it is placed, as it
/// does through a daily re-placement. Only a leader that cannot be reached fails the create.
async fn place(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    leader: Target,
) -> std::result::Result<(), String> {
    let mut leader = leader;
    for _ in 0..3 {
        match post_within(b, leader.site, &leader.do_name, "/place", br#"{"background":true}"#, PLACE_FOR).await {
            Some((200, v)) if v.get("error").is_none() => return Ok(()),
            // Leadership moved, as it does to `home` after a create: place from the member that leads now.
            Some((200, v)) if v.pointer("/error/notLeader").is_some() => {
                let probe = json!({ "q": { "probe": true } }).to_string();
                match call_opt(b, chain, record, "/read", probe.as_bytes()).await {
                    Ok(Some((_, _, t))) => leader = t,
                    _ => return Err(format!("{chain}: placement: no member leads")),
                }
            }
            Some((status, v)) => return Err(format!("{chain}: placement failed ({status}): {v}")),
            None => return Err(format!("{chain}: placement: no answer from {} within {PLACE_FOR:?}", leader.do_name)),
        }
    }
    Err(format!("{chain}: placement: leadership kept moving"))
}

// ---- the `Release` view ----

/// The isolate's genesis and where it came from: the edge's deploy-time binding, or the `Release` view.
#[derive(Clone)]
enum Held {
    Bound(graftyard_release::Genesis),
    View(graftyard_release::Genesis),
}

thread_local! {
    static GENESIS: RefCell<Option<Held>> = const { RefCell::new(None) };
    /// The binding was refused, or the view held another genesis: this isolate reads the view from now on.
    static BOUND_OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The binding matched the view once (`check_bound`).
    static CHECKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Seeds the isolate's genesis from the edge's `GRAFTYARD_GENESIS_SIGNED` binding (the signed genesis's stored bytes in
/// hex, `design/DESIGN.md`, "Install operations", Genesis, step 6), accepted with `accept_genesis` against `anchor`
/// exactly as the bytes a `Release` member answers are. Only an isolate that holds no genesis and has not refused or
/// dropped the binding reads it; a binding that is not hex, unsigned, or another install's is refused once, and the
/// isolate reads the view instead. Returns the refusal.
pub fn bind_genesis(
    anchor: &graftyard_release::Anchor,
    bound: impl FnOnce() -> Option<String>,
) -> std::result::Result<(), String> {
    if BOUND_OFF.with(|o| o.get()) || GENESIS.with(|g| g.borrow().is_some()) {
        return Ok(());
    }
    let Some(text) = bound() else { return Ok(()) };
    let accepted = unhex(text.trim()).ok_or_else(|| "GRAFTYARD_GENESIS_SIGNED is not hex".to_string()).and_then(|b| {
        graftyard_release::accept_genesis(anchor, &b)
            .map_err(|e| format!("GRAFTYARD_GENESIS_SIGNED: {}: {e}", e.variant()))
    });
    match accepted {
        Ok(g) => {
            GENESIS.with(|c| *c.borrow_mut() = Some(Held::Bound(g)));
            Ok(())
        }
        Err(e) => {
            BOUND_OFF.with(|o| o.set(true));
            Err(e)
        }
    }
}

/// The genesis this isolate holds, from the binding or the view.
fn held_genesis() -> Option<graftyard_release::Genesis> {
    GENESIS.with(|g| g.borrow().clone()).map(|(Held::Bound(g) | Held::View(g))| g)
}

/// Whether the isolate's genesis came from the binding.
pub fn genesis_bound() -> bool {
    GENESIS.with(|g| matches!(*g.borrow(), Some(Held::Bound(_))))
}

/// A system chain call under the binding's record found no group or failed, which a placement that a later entry
/// moved (`LeaveEvaluation`) would cause, and so would a `Names` or `jti` shard not created yet. Reads the `Release`
/// view once and compares: the same genesis keeps the binding for the rest of the isolate; another one replaces it, for
/// good, and returns true, so the caller retries under the view's record. A failed read keeps the binding and asks
/// again on the next miss. A genesis from the view, or one already checked, returns false at once.
pub async fn check_bound(b: &Bindings<'_>, anchor: &graftyard_release::Anchor) -> bool {
    if !genesis_bound() || CHECKED.with(|c| c.get()) {
        return false;
    }
    let Ok(bytes) = read_genesis(b).await else { return false };
    settle_bound(graftyard_release::accept_genesis(anchor, &bytes).ok())
}

/// `check_bound`'s decision on the view it read: `None` (unreadable or refused) keeps the binding; the same genesis
/// keeps it and marks it checked; another replaces it for good and returns true.
fn settle_bound(view: Option<graftyard_release::Genesis>) -> bool {
    let Some(view) = view else { return false };
    if held_genesis().as_ref() == Some(&view) {
        CHECKED.with(|c| c.set(true));
        return false;
    }
    BOUND_OFF.with(|o| o.set(true));
    GENESIS.with(|c| *c.borrow_mut() = Some(Held::View(view)));
    true
}

pub fn system_of(chain: ChainId) -> Option<graftyard_types::SystemChain> {
    use graftyard_types::SystemChain as S;
    match chain {
        ChainId::Names(_) => Some(S::Names),
        ChainId::Tokens(_) => Some(S::Tokens),
        ChainId::Ids => Some(S::Ids),
        ChainId::Release => Some(S::Release),
        _ => None,
    }
}

/// The genesis bytes a `Release` leader holds, found through the chain's `MemberRegistry` rows: a cold isolate does
/// not hold the genesis that names the group's record yet.
pub async fn read_genesis(b: &Bindings<'_>) -> std::result::Result<Vec<u8>, String> {
    let rec = registry_record(b, ChainId::Release).await.ok_or("no Release group is registered")?;
    let (v, _) = call(b, ChainId::Release, &rec, "/read", &json!({ "q": { "genesis": true } })).await?;
    let hex = v.pointer("/value/genesis").and_then(Value::as_str).ok_or_else(|| format!("no genesis held: {v}"))?;
    unhex(hex).ok_or_else(|| "the genesis is not hex".into())
}

/// The accepted genesis: the edge's binding once `bind_genesis` accepted it, else read once per isolate from the
/// `Release` group and accepted against the caller's own trust anchor, so a member that answers with another install's
/// or an unsigned genesis is refused (`design/DESIGN.md`, Shape, the `Release` view). Milestone 1 builds no entry that
/// changes it.
pub async fn genesis(
    b: &Bindings<'_>,
    anchor: &graftyard_release::Anchor,
) -> std::result::Result<graftyard_release::Genesis, String> {
    if let Some(g) = held_genesis() {
        return Ok(g);
    }
    let bytes = read_genesis(b).await?;
    let g = graftyard_release::accept_genesis(anchor, &bytes)
        .map_err(|e| format!("the Release group's genesis: {}: {e}", e.variant()))?;
    GENESIS.with(|c| *c.borrow_mut() = Some(Held::View(g.clone())));
    Ok(g)
}

/// A system chain's record from the `Release` view: the cached view's genesis once this isolate holds one (which a
/// `LeaveEvaluation` moves), else the genesis this isolate accepted, else the chain's own `MemberRegistry` row, as
/// step 3 of the lookup rule finds any group whose record the caller lacks, and only then a read of the genesis from
/// the `Release` group. So a cold isolate reaches a group that exists without waiting on the `Release` group
/// (`design/DESIGN.md`, Shape, `POINTER_TTL`: "no request waits on the group"); only a group not created yet, such as
/// a `Names` shard on its first use, needs the genesis read.
pub async fn system_record(
    b: &Bindings<'_>,
    anchor: &graftyard_release::Anchor,
    chain: ChainId,
) -> std::result::Result<PlacementRecord, String> {
    let sc = system_of(chain).ok_or_else(|| format!("{chain} is not a system chain"))?;
    let held = view::with(|v| v.fold.genesis.clone()).or_else(held_genesis);
    if let Some(g) = held {
        return g.placement(sc).cloned().ok_or_else(|| format!("the genesis has no {sc:?} record"));
    }
    view::refresh_beside(b.env, b.sites, anchor, None, true);
    if let Some(r) = registry_record(b, chain).await {
        return Ok(r);
    }
    let g = genesis(b, anchor).await?;
    g.placement(sc).cloned().ok_or_else(|| format!("the genesis has no {sc:?} record"))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    s.len()
        .is_multiple_of(2)
        .then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `repo-gone`: only a call or commit whose last error is no group reads as no group; the stuck graft-m4a round's
    /// error does, and an outage's or a leaderless group's does not.
    #[test]
    fn only_a_missing_group_reads_as_no_group() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(65568)));
        assert!(is_no_group(&format!("{chain}: {NO_GROUP}")));
        // The measured graft-m4a log line's error, `commit_as` giving up on `call_raw`'s.
        assert!(is_no_group("repo:65568: commit gave up: repo:65568: no group"));
        for other in [
            "repo:65568: commit gave up: repo:65568#1#f0: not joined",
            "repo:65568: commit gave up: repo:65568#1#f1: no answer",
            "repo:65568: commit gave up: {\"error\":{\"notLeader\":{\"term\":3,\"leader\":null}}}",
            "repo:65568: repo:65568#1#f2: OVERLOADED 250 ms",
            "repo:65568: no group answered, then a member did: busy",
        ] {
            assert!(!is_no_group(other), "{other}");
        }
    }

    /// A read during a repository's create, before `RepoOwner` commits, is "not created" only when a majority of the
    /// bootstrap members found no row; members that did not answer, refused for another reason (such as a `Names`
    /// read that failed), or come from an older release keep it an outage, which the edge answers 503. A majority
    /// retired, or retired and unsourced between them, is a retired group, which the edge answers 404 too.
    #[test]
    fn only_a_majority_that_cannot_join_is_gone() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(131316)));
        let none = || Some((409, json!({ "refused": { "noRepoOwner": 131316 } })));
        let failed = || Some((409, json!({ "refused": { "source": "names:994: no member answered" } })));
        let old = || Some((409, json!({ "refused": { "source": "RepoId(DbId(131316)) has no RepoOwner row" } })));
        let joined = || Some((200, json!([])));
        let retired = || Some((409, json!({ "refused": { "retired": 1_763_622_565_000u64 } })));
        let cases = [
            (vec![retired(), retired(), retired()], true),
            (vec![retired(), retired(), None], true),
            (vec![retired(), none(), failed()], true),
            (vec![retired(), joined(), None], false),
            (vec![retired(), failed(), failed()], false),
            (vec![retired(), None, None], false),
            (vec![none(), none(), none()], true),
            (vec![none(), none(), None], true),
            (vec![none(), none(), failed()], true),
            (vec![none(), joined(), None], false),
            (vec![none(), None, None], false),
            (vec![none(), failed(), failed()], false),
            (vec![failed(), failed(), failed()], false),
            (vec![None, None, None], false),
            (vec![old(), old(), old()], false),
        ];
        for (answers, want) in cases {
            let e = refused(chain, &answers, "repo:131316: no group");
            assert_eq!(is_gone(&e), want, "{e}");
            let any_retired =
                answers.iter().any(|a| matches!(a, Some((409, v)) if v["refused"].get("retired").is_some()));
            assert_eq!(e.contains(RETIRED), want && any_retired, "{e}");
            assert!(e.starts_with("repo:131316: "), "{e}");
        }
    }

    /// `create` starts `/place` only for a group its own joins built: a majority of fresh joins. B0-30's lookup in a
    /// leaderless gap found every bootstrap member already joined, or replaced; a create racing another for a new
    /// group sees fresh joins only where it was first; an answer from a member that predates `fresh` counts as not.
    #[test]
    fn only_a_group_this_create_bootstrapped_is_placed() {
        let fresh = || Some((200, json!({ JOINED_FRESH: true, "rows": [] })));
        let again = || Some((200, json!({ JOINED_FRESH: false, "rows": [] })));
        let old = || Some((200, json!([])));
        let superseded = || Some((409, json!({ "refused": { "superseded": {} } })));
        let cases = [
            (vec![fresh(), fresh(), fresh()], true),
            (vec![fresh(), fresh(), None], true),
            (vec![fresh(), fresh(), again()], true),
            (vec![again(), again(), again()], false),
            (vec![again(), superseded(), again()], false),
            (vec![fresh(), again(), again()], false),
            (vec![fresh(), again(), None], false),
            (vec![old(), old(), old()], false),
        ];
        for (answers, want) in cases {
            assert_eq!(bootstrapped_here(&answers), want, "{answers:?}");
        }
    }

    #[test]
    fn only_the_commit_shapes_are_readable() {
        let keys = ["done", "admitted"];
        assert!(readable(&json!({ "done": { "ok": {} } }), &keys));
        assert!(readable(&json!({ "admitted": { "term": 1 }, "meta": {} }), &keys));
        // A body that did not parse as JSON (`post_within` reads it as null), an empty object, and a shape from a
        // newer release are not lost replies.
        for v in [Value::Null, json!({}), json!({ "done": null }), json!({ "decided": { "ok": {} } }), json!([1])] {
            assert!(!readable(&v, &keys), "{v}");
        }
        assert_eq!(unreadable()["unsupported"]["family"], 0);
    }

    /// A signed genesis for install `n` under customer seed `seed`, its `Names` record at `names_gen`.
    fn signed_genesis(n: u8, seed: &[u8; 32], names_gen: u32) -> (graftyard_release::Genesis, Vec<u8>) {
        use graftyard_release::*;
        use graftyard_types::{AccountId, DbId, Region, Site, SiteLink, SystemChain};
        let install = InstallId(Ulid::new(Timestamp(1_790_000_000_000), [n; 10]));
        let site = |id: u8, region| Site {
            id: SiteId(id),
            script: format!("graft-site-{id}"),
            account: "acct".into(),
            region,
            jurisdiction: None,
            bucket: format!("graft-packs-{id}"),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare: false,
        };
        let g = Genesis {
            install,
            account: AccountId(DbId(1)),
            keys: Keys {
                generation: 0,
                customer: public_key(seed),
                vendor: None,
                release: public_key(&[3; 32]),
                drain: public_key(&[4; 32]),
                executor: public_key(&[5; 32]),
                agents_executor: None,
                ring: Default::default(),
            },
            sites: [site(0, Region::Enam), site(1, Region::Weur), site(2, Region::Apac)].into_iter().collect(),
            placements: [SystemChain::Release, SystemChain::Names, SystemChain::Tokens, SystemChain::Ids]
                .into_iter()
                .map(|c| (c, rec(if c == SystemChain::Names { names_gen } else { 1 }, None)))
                .collect(),
            evaluation: false,
            restore: None,
        };
        let bound = Bound { install, group: ChainId::Release, prev: None, parent: None };
        let mut e = SignedEntry::unsigned(bound, ReleaseEntry::Genesis(Box::new(g.clone())));
        e.sign(seed);
        (g, e.encode())
    }

    fn anchor_for(n: u8, seed: &[u8; 32]) -> graftyard_release::Anchor {
        graftyard_release::Anchor {
            install: graftyard_release::InstallId(Ulid::new(Timestamp(1_790_000_000_000), [n; 10])),
            customer: graftyard_release::public_key(seed),
        }
    }

    fn reset_genesis() {
        GENESIS.with(|g| *g.borrow_mut() = None);
        BOUND_OFF.with(|o| o.set(false));
        CHECKED.with(|c| c.set(false));
    }

    /// The binding is accepted only as a `Release` member's answer is: this install's genesis, signed by its customer
    /// key. Another install's, another key's, an unsigned one, a flipped byte, or text that is not hex is refused, and
    /// the isolate then reads the view for good, even when a good binding is offered later.
    #[test]
    fn the_genesis_binding_is_accepted_only_against_the_anchor() {
        let (seed, other) = ([7; 32], [8; 32]);
        let a = anchor_for(1, &seed);
        let (g, good) = signed_genesis(1, &seed, 1);
        let unsigned = {
            let mut e = graftyard_release::SignedEntry::decode(&good).unwrap();
            e.sigs.clear();
            e.encode()
        };
        let mut flipped = good.clone();
        let mid = flipped.len() / 2;
        flipped[mid] ^= 1;
        let bad = [
            hex(&signed_genesis(2, &seed, 1).1),
            hex(&signed_genesis(1, &other, 1).1),
            hex(&unsigned),
            hex(&flipped),
            "zz".into(),
        ];
        for b in bad {
            reset_genesis();
            assert!(bind_genesis(&a, || Some(b.clone())).is_err(), "{}", &b[..8.min(b.len())]);
            assert!(!genesis_bound() && held_genesis().is_none());
            assert!(bind_genesis(&a, || Some(hex(&good))).is_ok());
            assert!(!genesis_bound(), "a refused binding is not retried in the same isolate");
        }
        reset_genesis();
        assert!(bind_genesis(&a, || None).is_ok());
        assert!(held_genesis().is_none(), "an absent binding reads the view");
        assert!(bind_genesis(&a, || Some(hex(&good))).is_ok());
        assert!(genesis_bound());
        assert_eq!(held_genesis(), Some(g));
    }

    /// A miss under the binding compares it with the view: an unreadable view keeps it and asks again later; the same
    /// genesis keeps it and is not read again; another one (a moved placement) replaces it for good, so the caller
    /// retries under the view's record.
    #[test]
    fn a_miss_under_the_binding_defers_to_a_different_view() {
        let seed = [7; 32];
        let a = anchor_for(1, &seed);
        let (g, good) = signed_genesis(1, &seed, 1);
        let (moved, _) = signed_genesis(1, &seed, 2);
        reset_genesis();
        bind_genesis(&a, || Some(hex(&good))).unwrap();
        assert!(!settle_bound(None));
        assert!(genesis_bound() && !CHECKED.with(|c| c.get()), "an unreadable view keeps the binding unchecked");
        assert!(!settle_bound(Some(g.clone())));
        assert!(genesis_bound() && CHECKED.with(|c| c.get()), "the same genesis keeps the binding, checked");
        reset_genesis();
        bind_genesis(&a, || Some(hex(&good))).unwrap();
        assert!(settle_bound(Some(moved.clone())), "another genesis tells the caller to retry");
        assert!(!genesis_bound());
        assert_eq!(held_genesis(), Some(moved.clone()));
        let names = held_genesis().unwrap().placement(graftyard_types::SystemChain::Names).unwrap().generation;
        assert_eq!(names, 2, "system chains now use the view's record");
        assert!(bind_genesis(&a, || Some(hex(&good))).is_ok());
        assert_eq!(held_genesis(), Some(moved), "the binding never comes back in this isolate");
    }

    fn rec(generation: u32, moving_from: Option<PlacementRecord>) -> PlacementRecord {
        PlacementRecord {
            name: graftyard_types::PlacementName::Tri,
            home: SiteId(0),
            sites: [SiteId(0), SiteId(1), SiteId(2)].into_iter().collect(),
            generation,
            moving_from: moving_from.map(Box::new),
            at: Timestamp(0),
        }
    }

    /// The rule's order, one stage at a time: the cached config leader first, then the record's bootstrap names; its
    /// registry rows only after them; then the same for `moving_from`. A stage that holds a registry read is never
    /// part of the first stage, so a lookup answered by the cache or the names makes no registry call.
    /// The first commit send goes to the leader the last reply named, when the cached config holds it; with no
    /// leader cached, or one the config does not name, the edge looks the leader up first.
    #[test]
    fn a_commit_starts_at_the_cached_leader() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(8)));
        assert_eq!(cached_leader(chain), None);
        let voters = vec![(SiteId(0), "a".to_string()), (SiteId(2), "c".to_string())];
        KNOWN.with(|k| {
            k.borrow_mut()
                .insert(chain, Known { voters, config_index: 1, leader: Some(SiteId(2)), ..Default::default() })
        });
        assert_eq!(cached_leader(chain), Some(Target { site: SiteId(2), do_name: "c".into() }));
        forget_leader(chain);
        assert_eq!(cached_leader(chain), None);
        KNOWN.with(|k| k.borrow_mut().get_mut(&chain).unwrap().leader = Some(SiteId(1)));
        assert_eq!(cached_leader(chain), None, "a leader outside the config is not sent to");
    }

    /// A commit skips the create probe only while the isolate holds a leader: a reply naming one sets it, a
    /// `NotLeader` (which `commit_hedged` answers with `forget_leader`) clears it, and so does a reply from a term below
    /// the highest seen, whose leader may be gone; a later reply at the current term sets it again.
    #[test]
    fn a_commit_probes_again_after_not_leader_or_a_stale_term() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(81)));
        let voters = vec![(SiteId(0), "a".to_string()), (SiteId(1), "b".to_string()), (SiteId(2), "c".to_string())];
        KNOWN.with(|k| k.borrow_mut().insert(chain, Known { voters, config_index: 1, ..Default::default() }));
        assert!(!knows_leader(chain), "no reply has named a leader");
        learn_leader(chain, &json!({ "meta": { "term": 5, "leader": 2 } }));
        assert!(knows_leader(chain));
        forget_leader(chain);
        assert!(!knows_leader(chain), "after NotLeader the next commit probes");
        learn_leader(chain, &json!({ "meta": { "term": 5, "leader": 2 } }));
        learn_leader(chain, &json!({ "meta": { "term": 4, "leader": 1 } }));
        assert!(!knows_leader(chain), "a reply from term 4 after term 5 is stale: probe again");
        assert_eq!(cached_leader(chain), None, "and its leader is not sent to");
        learn_leader(chain, &json!({ "meta": { "term": 6, "leader": 0 } }));
        assert_eq!(cached_leader(chain), Some(Target { site: SiteId(0), do_name: "a".into() }));
        learn_leader(chain, &json!({ "meta": { "leader": 1 } }));
        assert_eq!(cached_leader(chain).map(|t| t.site), Some(SiteId(1)), "a reply with no term keeps the old rule");
    }

    /// `receipt` driven by hand: `send` answers at `send_at` ms (or never), each `/outcome_by_request` answers `asks` in
    /// turn after `ask_ms`, and the clock moves only by the waits. Returns what the send came to, the asks made, and
    /// the clock at the end.
    fn run_receipt(
        send_at: Option<(u64, Option<(u16, Value)>)>,
        asks: Vec<Option<(u16, Value)>>,
        ask_ms: u64,
        busy: impl Fn(u64) -> bool,
    ) -> (Receipt, usize, u64) {
        use futures_util::FutureExt;
        use std::cell::Cell;
        use std::rc::Rc;
        let clock = Rc::new(Cell::new(0u64));
        let asked = Cell::new(0usize);
        let asks = RefCell::new(asks);
        // A future that is ready once the clock reaches `at`: polled again after each wait moves the clock.
        struct At(Rc<Cell<u64>>, u64);
        impl Future for At {
            type Output = ();
            fn poll(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<()> {
                if self.0.get() >= self.1 { std::task::Poll::Ready(()) } else { std::task::Poll::Pending }
            }
        }
        let c = clock.clone();
        let send = async move {
            match send_at {
                Some((at, r)) => {
                    At(c, at).await;
                    r
                }
                None => std::future::pending().await,
            }
        };
        let ask = || {
            asked.set(asked.get() + 1);
            let r = asks.borrow_mut().pop().flatten();
            let (c, at) = (clock.clone(), clock.get() + ask_ms);
            async move {
                At(c.clone(), at).await;
                r
            }
        };
        let wait = |d: Duration| At(clock.clone(), clock.get() + d.as_millis() as u64);
        let mut f = Box::pin(receipt(send, ask, || busy(clock.get()), wait));
        // One poll per ms of clock, so whatever is due at a ms is seen before the clock moves on.
        for _ in 0..20_000 {
            if let Some(r) = (&mut f).now_or_never() {
                return (r, asked.get(), clock.get());
            }
            clock.set(clock.get() + 1);
        }
        panic!("receipt never ended");
    }

    /// The receipt rule: a slow 200 from a member this isolate heard from in this term within COMMIT_RECEIPT is the
    /// send's reply, with no resend and no `suspect`; meanwhile the edge asks by request id and takes only a `done`,
    /// a `NotLeader`, or a `pending` from it, never `unknown`. Before the rule, the edge gave up at COMMIT_RECEIPT,
    /// reported the leader and sent the command again (B0-32 m3fr8: 110 of 121 extra sends, 82 `suspect` waves).
    #[test]
    fn a_slow_200_from_a_busy_member_is_the_reply_with_no_resend_or_report() {
        let ok = json!({ "done": { "ok": 1 }, "meta": { "term": 2 } });
        // The send's 200 lands at 3.1 s; the by-request answers say `unknown`, so the send stays open.
        let unknown = Some((200, json!({ "unknown": true, "meta": { "term": 2 } })));
        let (r, asked, at) =
            run_receipt(Some((3_100, Some((200, ok.clone())))), vec![unknown.clone(); 8], 300, |_| true);
        assert_eq!(r, Receipt::Reply(ok.clone()), "the late 200 is the reply, not a missed receipt");
        assert!(asked >= 1, "the edge asked by request id while the send was open");
        assert_eq!(at, 3_100);
        // A `done` by request id ends the send at once: it is the leader's committed outcome.
        let done = Some((200, json!({ "done": { "ok": 2 }, "meta": { "term": 2 } })));
        let (r, _, at) = run_receipt(None, vec![done], 300, |_| true);
        assert_eq!(r, Receipt::Reply(json!({ "done": { "ok": 2 }, "meta": { "term": 2 } })));
        assert_eq!(at, COMMIT_RECEIPT.as_millis() as u64 + 300);
        // `pending` is a ticket to wait on through `outcome`, as `admitted` is; never a success by itself.
        let pending = Some((200, json!({ "pending": { "seq": 4 }, "meta": { "term": 2 } })));
        let (r, _, _) = run_receipt(None, vec![pending], 300, |_| true);
        assert_eq!(r, Receipt::Reply(json!({ "admitted": { "seq": 4 }, "meta": { "term": 2 } })));
        // Nothing but `done`, `NotLeader`, or `pending` ends it: an `error` other than `NotLeader`, a 503, and no
        // answer leave the send open, and its own late 200, after all three asks and inside RECEIPT_OPEN, is still the
        // reply.
        let odd = vec![Some((200, json!({ "error": { "busy": true } }))), Some((503, Value::Null)), None];
        let late = 3 * COMMIT_RECEIPT.as_millis() as u64 + 2 * 100 + 200;
        assert!(late < RECEIPT_OPEN.as_millis() as u64);
        let (r, asked, _) = run_receipt(Some((late, Some((200, ok.clone())))), odd, 100, |_| true);
        assert_eq!((r, asked), (Receipt::Reply(ok.clone()), 3));
        // B0-32 m3fr11: the isolate's last 200 from the leader is older than COMMIT_RECEIPT at the deadline (no `busy`),
        // because every held `/commit` there took 2 to 4.6 s, but the leader answers the ask by request id in 300 ms.
        // It answers, so the send stays open and its late 200 is the reply: no `suspect` and no resend.
        let (r, asked, at) =
            run_receipt(Some((3_100, Some((200, ok.clone())))), vec![unknown.clone(); 8], 300, |_| false);
        assert_eq!((r, asked, at), (Receipt::Reply(ok.clone()), 1, 3_100));
        // A send that ends with no answer (RECEIPT_OPEN) while the member is busy is sent again, but not reported.
        let (r, _, at) = run_receipt(Some((8_000, None)), vec![unknown.clone(); 8], 300, |_| true);
        assert_eq!((r, at), (Receipt::Missed { report: false }, 8_000));
        // An error status is reported however busy the member was: a sick leader's 5xx is still the gray case.
        let after_ask = COMMIT_RECEIPT.as_millis() as u64 + 400;
        let (r, _, _) = run_receipt(Some((after_ask, Some((503, Value::Null)))), vec![unknown; 8], 300, |_| true);
        assert_eq!(r, Receipt::Missed { report: true });
    }

    /// The leader-sync A/B's arm B (`Hooks::lean`): a lean send carries `hold_ms` and makes no `/outcome_by_request`
    /// ask. Its receipt is the send's own answer: a late 200 is the reply as `receipt` takes it, but with no ask, and a
    /// send with no answer is sent again, reported only when the member is not busy. With the flag off (`Hooks`'
    /// default) the asks are `receipt`'s, as on every product edge.
    #[test]
    fn a_lean_send_holds_its_commit_and_asks_nothing() {
        use futures_util::FutureExt;
        assert!(!Hooks::default().lean, "off unless a test edge's request asks");
        // The body keeps every field byte for byte, `hold_ms` first, and still parses.
        let body = br#"{"rid":"r1","cmd":{"push":"QUJD"},"admit":{"inline":3}}"#;
        let held = held_body(body);
        assert_eq!(
            held,
            format!("{{\"hold_ms\":{LEAN_HOLD_MS},{}", &std::str::from_utf8(body).unwrap()[1..]).into_bytes()
        );
        let v: Value = serde_json::from_slice(&held).unwrap();
        assert_eq!(v["hold_ms"], json!(6_000));
        assert_eq!(
            (&v["rid"], &v["cmd"], &v["admit"]),
            (&json!("r1"), &json!({ "push": "QUJD" }), &json!({ "inline": 3 }))
        );
        assert_eq!(serde_json::from_slice::<Value>(&held_body(b"{}")).unwrap(), json!({ "hold_ms": LEAN_HOLD_MS }));
        assert_eq!(held_body(b"[1]"), b"[1]", "not an object: as it came");
        assert!(
            LEAN_HOLD_MS < RECEIPT_OPEN.as_millis() as u64,
            "the held answer comes back before the send's deadline"
        );
        // The receipt: the send's 200, held past COMMIT_RECEIPT, is the reply, done or admitted, as it came.
        let ok = json!({ "done": { "ok": 1 }, "meta": { "term": 2 } });
        let lean = |r: Option<(u16, Value)>, busy: bool| send_only(async { r }, || busy).now_or_never().unwrap();
        assert_eq!(lean(Some((200, ok.clone())), false), Receipt::Reply(ok.clone()));
        let ticket = json!({ "admitted": { "seq": 4 }, "meta": { "term": 2 } });
        assert_eq!(lean(Some((200, ticket.clone())), false), Receipt::Reply(ticket));
        assert_eq!(lean(Some((503, Value::Null)), true), Receipt::Missed { report: true }, "an error status");
        assert_eq!(lean(None, true), Receipt::Missed { report: false }, "no answer while busy: sent again");
        assert_eq!(lean(None, false), Receipt::Missed { report: true }, "no answer, not busy: reported");
        // The same late 200 under `receipt` (flag off) is asked about every COMMIT_RECEIPT while it is open.
        let unknown = Some((200, json!({ "unknown": true, "meta": { "term": 2 } })));
        let at = 2 * COMMIT_RECEIPT.as_millis() as u64 + 1_000;
        let (r, asked, _) = run_receipt(Some((at, Some((200, ok.clone())))), vec![unknown; 8], 300, |_| true);
        assert_eq!((r, asked), (Receipt::Reply(ok), 2), "flag off: today's asks");
    }

    /// The leader holds a `/commit` COMMIT_HOLD (1 s; `commit-hold`'s 1.5 s was reversed) for its outcome and the edge's
    /// receipt waits that hold plus one call. A leader modelled as `commit_all_held` answers: `done` when the ticket
    /// resolved inside the hold, else `admitted` at the hold's end, either one leg later. Each is the send's reply as it
    /// came, with no ask by request id, and an `admitted` stays a ticket the edge waits on through `outcome`, never a
    /// success by itself. A send the leader never answers still ends at RECEIPT_OPEN, asked by request id at most once
    /// per COMMIT_RECEIPT.
    #[test]
    fn a_commit_held_its_whole_hold_is_the_reply_with_no_ask() {
        assert_eq!(graftyard_raft::COMMIT_HOLD, Duration::from_secs(1));
        assert_eq!(COMMIT_RECEIPT, Duration::from_millis(2_000), "the hold plus one call");
        assert!(COMMIT_RECEIPT < RECEIPT_OPEN, "a held answer comes back before the send's own deadline");
        let hold = graftyard_raft::COMMIT_HOLD.as_millis() as u64;
        let meta = json!({ "term": 2 });
        let leader = |resolves_at: u64, leg: u64| {
            let (at, v) = if resolves_at <= hold {
                (resolves_at + leg, json!({ "done": { "ok": 1 }, "meta": meta }))
            } else {
                (hold + leg, json!({ "admitted": { "seq": 7 }, "meta": meta }))
            };
            run_receipt(Some((at, Some((200, v)))), vec![Some((200, json!({ "unknown": true })))], 300, |_| true)
        };
        // Resolved at 0.9 s, inside the hold: `done`, with no `/outcome` to follow.
        let (r, asked, at) = leader(900, 400);
        assert_eq!((r, asked, at), (Receipt::Reply(json!({ "done": { "ok": 1 }, "meta": meta })), 0, 1_300));
        // Resolved after the hold: `admitted` at 1 s, and its slowest leg inside the receipt is not asked about.
        let (r, asked, at) = leader(1_100, 900);
        assert_eq!((r, asked, at), (Receipt::Reply(json!({ "admitted": { "seq": 7 }, "meta": meta })), 0, 1_900));
        // A leader that never answers: the send ends at RECEIPT_OPEN, asked at 2.0, 4.3 and 6.6 s, and never at 8.9 s.
        let open = RECEIPT_OPEN.as_millis() as u64;
        let unknown = Some((200, json!({ "unknown": true, "meta": meta })));
        let (r, asked, at) = run_receipt(Some((open, None)), vec![unknown; 8], 300, |_| true);
        assert_eq!((r, asked, at), (Receipt::Missed { report: false }, 3, open));
    }

    /// A member that never answered this isolate a 200 in this term within COMMIT_RECEIPT is gone as far as the edge
    /// can tell: its missed receipt is reported with `suspect` at the deadline, as before, and it is never asked by
    /// request id. So is one whose last 200 is older than COMMIT_RECEIPT by the time its receipt misses, or from an
    /// older term.
    #[test]
    fn a_member_that_never_answered_is_still_reported() {
        let ok = json!({ "done": { "ok": 1 }, "meta": { "term": 2 } });
        // No 200 before the deadline and none to the ask by request id: reported when the ask ends.
        let r_ms = COMMIT_RECEIPT.as_millis() as u64;
        let (r, asked, at) = run_receipt(Some((3_100, Some((200, ok.clone())))), vec![], 300, |_| false);
        assert_eq!((r, asked, at), (Receipt::Missed { report: true }, 1, r_ms + 300));
        // An error status to the ask is no answer either.
        let (r, _, at) = run_receipt(None, vec![Some((503, Value::Null))], 300, |_| false);
        assert_eq!((r, at), (Receipt::Missed { report: true }, r_ms + 300));
        // Answering at the first deadline, then silent: asked at 2 s, answered at 2.3 s; asked again at 4.3 s, no
        // answer by 4.6 s, and no longer busy then, so it is reported.
        let unknown = Some((200, json!({ "unknown": true })));
        let mut asks = vec![None; 7];
        asks.push(unknown);
        let (r, _, at) = run_receipt(None, asks, 300, |now| now < 3_600);
        assert_eq!((r, at), (Receipt::Missed { report: true }, 2 * r_ms + 600));
        // `busy` itself: a 200 in this term within COMMIT_RECEIPT, and nothing else.
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(91)));
        assert!(!busy(chain, SiteId(0), 2, 10_000), "never heard");
        heard(chain, SiteId(0), &json!({ "meta": { "term": 2 } }), 10_000);
        assert!(busy(chain, SiteId(0), 2, 10_000 + r_ms));
        assert!(!busy(chain, SiteId(0), 2, 10_000 + r_ms + 1), "heard more than COMMIT_RECEIPT ago");
        assert!(!busy(chain, SiteId(1), 2, 10_500), "heard from another member");
        assert!(!busy(chain, SiteId(0), 3, 10_500), "heard in an older term than the send's");
        heard(chain, SiteId(0), &json!({ "done": {} }), 30_000);
        assert!(!busy(chain, SiteId(0), 2, 30_000), "an answer with no term counts for nothing");
        KNOWN.with(|k| k.borrow_mut().entry(chain).or_default().term = 4);
        heard(chain, SiteId(0), &json!({ "meta": { "term": 3 } }), 40_000);
        assert!(!busy(chain, SiteId(0), 0, 40_000), "older than the highest term this isolate saw from the group");
    }

    /// A capped read's answer is the leader saying "busy, retry": never a missed member, which `call_opt` reports
    /// with `suspect` and which fed the storm the cap is there to stop, and never a reply to return. A 5xx and no
    /// answer stay missed, and the old `busy` (a read past its deadline) is still skipped.
    #[test]
    fn an_overloaded_answer_is_retried_and_never_missed() {
        let shed = overloaded_reply(300);
        assert_eq!(classify(Some((200, &shed)), false), Answer::Overloaded(300));
        assert_eq!(classify(Some((200, &shed)), true), Answer::Overloaded(300));
        assert_eq!(classify(Some((200, &json!({ "busy": true }))), false), Answer::Skip("busy".into()));
        assert_eq!(classify(Some((503, &Value::Null)), false), Answer::Missed("503".into()));
        assert_eq!(classify(None, false), Answer::Missed("no answer".into()));
        assert_eq!(classify(Some((409, &Value::Null)), true), Answer::NotJoined);
        assert_eq!(classify(Some((409, &Value::Null)), false), Answer::Skip("409".into()));
        assert_eq!(classify(Some((200, &json!({ "value": 1 }))), false), Answer::Reply);
        let nl = json!({ "error": { "notLeader": { "term": 3 } } });
        assert_eq!(classify(Some((200, &nl)), false), Answer::Reply);
        // A lookup built before `overloaded` reads the same answer as `busy`, so it does not report it either.
        assert!(shed.get("busy").is_some());
    }

    /// A read-index read goes to the learners first, then the followers, never the cached leader, each rotated by
    /// `pick`, and to at most SPREAD_TRIES members; with no leader cached every voter is a candidate.
    #[test]
    fn spread_reads_go_to_learners_then_followers_and_skip_the_leader() {
        let v = |s: &[(u8, &str)]| s.iter().map(|(i, n)| (SiteId(*i), n.to_string())).collect::<Vec<_>>();
        let names = |t: Vec<Target>| t.into_iter().map(|t| t.do_name).collect::<Vec<_>>();
        let all = v(&[(0, "a"), (1, "b"), (2, "c")]);
        let none = Vec::new;
        assert_eq!(names(spread_order(none(), all.clone(), Some(SiteId(1)), 0)), ["a", "c"]);
        assert_eq!(names(spread_order(none(), all.clone(), Some(SiteId(1)), 1)), ["c", "a"]);
        assert_eq!(names(spread_order(none(), all.clone(), Some(SiteId(1)), 4)), ["a", "c"]);
        assert_eq!(names(spread_order(none(), all.clone(), None, 2)), ["c", "a", "b"]);
        assert_eq!(spread_order(none(), Vec::new(), Some(SiteId(1)), 3), Vec::new());
        // Learners (on sites 1 and 2, beside voters) come before any follower, rotated on their own.
        let learners = v(&[(1, "l0"), (2, "l1")]);
        assert_eq!(names(spread_order(learners.clone(), all.clone(), Some(SiteId(0)), 0)), ["l0", "l1", "b"]);
        assert_eq!(names(spread_order(learners.clone(), all.clone(), Some(SiteId(0)), 1)), ["l1", "l0", "c"]);
        // Four learners: three asked, no follower.
        let four = v(&[(0, "l0"), (1, "l1"), (2, "l2"), (0, "l3")]);
        assert_eq!(names(spread_order(four, all, Some(SiteId(0)), 3)), ["l3", "l0", "l1"]);
    }

    /// Learners in config order, on sites 1 and 2 by turns, each with this isolate's average in ms.
    fn timed(ls: &[(&str, Option<f64>)]) -> Vec<(SiteId, String, Option<f64>)> {
        ls.iter().enumerate().map(|(i, (n, a))| (SiteId(1 + (i % 2) as u8), n.to_string(), *a)).collect()
    }

    fn voters() -> Vec<(SiteId, String)> {
        [(0, "a"), (1, "b"), (2, "c")].iter().map(|(i, n)| (SiteId(*i), n.to_string())).collect()
    }

    fn first(t: &[Target]) -> &str {
        &t[0].do_name
    }

    /// Near learners at 300 and 350 ms and far ones at 1,800 and 2,400 ms, interleaved so config order is not
    /// ascending order.
    fn mixed() -> Vec<(SiteId, String, Option<f64>)> {
        timed(&[
            ("far2400", Some(2_400.0)),
            ("near300", Some(300.0)),
            ("far1800", Some(1_800.0)),
            ("near350", Some(350.0)),
        ])
    }

    /// (a, b) With two near learners within ROUTE_MARGIN and two far ones, no `pick` puts a far learner first, and
    /// each near learner comes first for some `pick`: the near set shares the reads, not only the single fastest.
    #[test]
    fn a_read_goes_to_a_near_learner_and_both_near_ones_share_it() {
        let mut firsts = std::collections::BTreeSet::new();
        for pick in 0..16 {
            let o = timed_spread_order(mixed(), voters(), Some(SiteId(0)), pick);
            assert!(first(&o).starts_with("near"), "pick {pick}: {o:?}");
            assert!(!o[1].do_name.starts_with("far"), "pick {pick}: the second is the other near learner: {o:?}");
            firsts.insert(first(&o).to_string());
        }
        assert_eq!(firsts.into_iter().collect::<Vec<_>>(), ["near300", "near350"]);
    }

    /// (c) With ROUTE_SPREAD learners already inside the margin, so none is padded in, a learner exactly ROUTE_MARGIN
    /// behind the best is in the near set and comes first for some `pick`; one a millisecond past it never does.
    #[test]
    fn the_margin_is_inclusive_at_its_edge() {
        let m = ROUTE_MARGIN.as_millis() as f64;
        let with =
            |edge: f64| timed(&[("fast", Some(500.0)), ("b", Some(510.0)), ("c", Some(520.0)), ("edge", Some(edge))]);
        let (at, past) = (with(500.0 + m), with(500.0 + m + 1.0));
        let firsts = |ls: &Vec<(SiteId, String, Option<f64>)>| {
            (0..16)
                .map(|p| first(&timed_spread_order(ls.clone(), voters(), Some(SiteId(0)), p)).to_string())
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert!(firsts(&at).contains("edge"), "{:?}", firsts(&at));
        assert_eq!(firsts(&past).into_iter().collect::<Vec<_>>(), ["b", "c", "fast"]);
    }

    /// (d) A learner this isolate has not timed, or not for ROUTE_STALE, is eligible and does not set the best: it
    /// shares the reads with the near learners, and the far ones stay out, so a recovered learner is timed again.
    #[test]
    fn an_untimed_learner_is_eligible_and_does_not_set_the_best() {
        let ls =
            timed(&[("near300", Some(300.0)), ("new", None), ("near350", Some(350.0)), ("far1800", Some(1_800.0))]);
        let mut firsts = std::collections::BTreeSet::new();
        for pick in 0..16 {
            let o = timed_spread_order(ls.clone(), voters(), Some(SiteId(0)), pick);
            assert_ne!(first(&o), "far1800", "pick {pick}: {o:?}");
            firsts.insert(first(&o).to_string());
        }
        assert_eq!(firsts.into_iter().collect::<Vec<_>>(), ["near300", "near350", "new"]);
        // More timed learners near the best than padding would admit: an untimed one must not shut any of them out.
        let ls = timed(&[("new", None), ("a300", Some(300.0)), ("b310", Some(310.0)), ("c320", Some(320.0))]);
        let firsts: std::collections::BTreeSet<String> =
            (0..16).map(|p| first(&timed_spread_order(ls.clone(), voters(), Some(SiteId(0)), p)).to_string()).collect();
        assert_eq!(firsts.into_iter().collect::<Vec<_>>(), ["a300", "b310", "c320", "new"]);
        let t = Timed { mean: 300.0, dev: 0.0, at: 1_000 };
        let stale = 1_000 + ROUTE_STALE.as_millis() as u64;
        assert_eq!(t.score(stale - 1), Some(300.0));
        assert_eq!(t.score(stale), None, "untimed once ROUTE_STALE has passed");
        assert_eq!(Timed::after(Some(t), 900.0, stale), Timed { mean: 900.0, dev: 0.0, at: stale }, "starts afresh");
    }

    /// (e) The herding guard. One learner answers fastest and steadily (200 ms), and every other is well past
    /// ROUTE_MARGIN but answers in time (600 to 900 ms): the fastest still gets no more than 1 in ROUTE_SPREAD of the
    /// first places, and the next best share them. Ordered by the average alone, gate 15's fastest learner took 50.2%
    /// of the reads and shed 54. A learner scored at ROUTE_LATE or past it (a far one answering at FOLLOWER_WAIT)
    /// never comes first while others answer in time.
    #[test]
    fn the_fastest_learner_gets_a_bounded_share() {
        let ls = timed(&[
            ("slow900", Some(900.0)),
            ("fast", Some(200.0)),
            ("late", Some(ROUTE_LATE.as_millis() as f64)),
            ("mid600", Some(600.0)),
            ("mid700", Some(700.0)),
        ]);
        let mut n = std::collections::BTreeMap::<String, usize>::new();
        let picks = 60;
        for pick in 0..picks {
            let o = timed_spread_order(ls.clone(), voters(), Some(SiteId(0)), pick);
            *n.entry(first(&o).to_string()).or_default() += 1;
            assert_eq!(o.len(), SPREAD_TRIES);
            assert!(o.iter().all(|t| t.do_name != "late"), "pick {pick}: a late learner was asked: {o:?}");
        }
        assert_eq!(n.keys().map(String::as_str).collect::<Vec<_>>(), ["fast", "mid600", "mid700"], "{n:?}");
        assert!(n["fast"] * ROUTE_SPREAD <= picks, "the fastest took {} of {picks}: {n:?}", n["fast"]);
        // With enough learners inside the margin, none is padded in.
        let three = timed(&[("a", Some(200.0)), ("b", Some(250.0)), ("c", Some(380.0)), ("d", Some(600.0))]);
        for pick in 0..16 {
            assert_ne!(first(&timed_spread_order(three.clone(), voters(), Some(SiteId(0)), pick)), "d");
        }
    }

    /// (f) A shed, overloaded, `notLeader`, 5xx, or missing answer records FOLLOWER_CALL, a value its elapsed time.
    /// A learner's first sample is its average with no deviation; then each sample moves the average and the mean
    /// deviation ROUTE_WEIGHT of the way, and the score is the average plus ROUTE_DEV deviations, so a learner whose
    /// answers swing scores worse than a steady one with the same average.
    #[test]
    fn a_learners_score_is_its_average_plus_its_deviation() {
        let call = FOLLOWER_CALL.as_millis() as f64;
        assert_eq!(answer_sample(Some((200, &json!({ "value": 1, "index": 3 }))), 312.0), 312.0);
        let nl = json!({ "error": { "notLeader": { "term": 3 } } });
        for (status, v) in [(200, overloaded_reply(250)), (200, json!({ "busy": true })), (200, nl), (503, Value::Null)]
        {
            assert_eq!(answer_sample(Some((status, &v)), 40.0), call, "{status} {v}");
        }
        assert_eq!(answer_sample(None, 2_000.0), call);
        let first = Timed::after(None, 300.0, 0);
        assert_eq!((first.mean, first.dev, first.score(0)), (300.0, 0.0, Some(300.0)));
        // 300 ms, then one 2,300 ms answer: average 300 + 0.2 * 2,000 = 700, deviation 0.2 * 2,000 = 400,
        // score 700 + 4 * 400 = 2,300.
        let t = Timed::after(Some(first), 2_300.0, 10);
        assert!((t.mean - 700.0).abs() < 1e-9 && (t.dev - 400.0).abs() < 1e-9, "{t:?}");
        assert!((t.score(10).unwrap() - (700.0 + ROUTE_DEV * 400.0)).abs() < 1e-9);
        // Steady at 500 against swinging 200 / 800 around the same average: the steady one scores better.
        let (mut steady, mut swing) = (Timed::after(None, 500.0, 0), Timed::after(None, 500.0, 0));
        for i in 1..40u64 {
            steady = Timed::after(Some(steady), 500.0, i);
            swing = Timed::after(Some(swing), if i % 2 == 0 { 200.0 } else { 800.0 }, i);
        }
        assert!(swing.score(40).unwrap() > steady.score(40).unwrap() + ROUTE_MARGIN.as_millis() as f64);
    }

    /// (g) After the near set the far learners come in ascending average, whatever their config order, then the
    /// followers rotated by `pick` with the cached leader skipped; SPREAD_TRIES still truncates.
    #[test]
    fn far_learners_follow_in_ascending_order_then_followers() {
        let ls = timed(&[("far2400", Some(2_400.0)), ("near300", Some(300.0)), ("far1800", Some(1_800.0))]);
        for pick in 0..16 {
            let o = timed_spread_order(ls.clone(), voters(), Some(SiteId(0)), pick);
            let n: Vec<&str> = o.iter().map(|t| t.do_name.as_str()).collect();
            assert_eq!(n, ["near300", "far1800", "far2400"], "pick {pick}");
        }
        let one_far = timed(&[("far1800", Some(1_800.0)), ("near300", Some(300.0))]);
        let o = timed_spread_order(one_far.clone(), voters(), Some(SiteId(0)), 0);
        assert_eq!(o.iter().map(|t| t.do_name.as_str()).collect::<Vec<_>>(), ["near300", "far1800", "b"]);
        let o = timed_spread_order(one_far, voters(), Some(SiteId(0)), 1);
        assert_eq!(o.iter().map(|t| t.do_name.as_str()).collect::<Vec<_>>(), ["near300", "far1800", "c"]);
    }

    /// When no member asked answers a value: if any shed it (`overloaded`, `busy`, a 5xx, no answer), the read is
    /// answered 503 and never sent to the leader; only when every one declined it as not its own (`notLeader`, or a
    /// 4xx) does it go to the leader by `call_opt`.
    #[test]
    fn a_shed_read_never_reaches_the_leader() {
        let value = json!({ "value": 1, "index": 3 });
        let shed = overloaded_reply(250);
        let busy = json!({ "busy": true });
        let named = json!({ "error": { "notLeader": { "term": 3, "leader": 0 } } });
        // A learner that lost its leader (it answers `notLeader` naming none until the next append) is no reason to
        // send the read to the leader while another member sheds it.
        let leaderless = json!({ "error": { "notLeader": { "term": 3, "leader": null } } });
        let null = Value::Null;
        assert_eq!(spread_verdict(&[Some((200, &shed)), Some((200, &value))], true), Spread::Use(1));
        for answers in [
            vec![Some((200, &shed)), Some((200, &shed)), Some((200, &shed))],
            vec![Some((200, &named)), Some((200, &shed))],
            vec![Some((200, &shed)), Some((200, &named))],
            vec![Some((200, &busy)), Some((200, &named))],
            vec![None, Some((200, &named))],
            vec![Some((503, &null)), Some((200, &named))],
            vec![Some((200, &named)), Some((409, &null)), None],
            vec![Some((200, &leaderless)), Some((200, &shed))],
        ] {
            assert!(matches!(spread_verdict(&answers, true), Spread::Shed(_)), "{answers:?}");
        }
        for answers in [
            vec![Some((200, &named)), Some((200, &named))],
            vec![Some((409, &null)), Some((409, &null))],
            vec![Some((200, &named)), Some((404, &null))],
            vec![Some((200, &leaderless)), Some((200, &named))],
            vec![],
        ] {
            assert_eq!(spread_verdict(&answers, true), Spread::Leader, "{answers:?}");
        }
    }

    /// `tier-off-leader`: a whole-tier dump or an `ls-refs` is a read the leader never serves; a merge's rows, a narrow
    /// API read and the read gate's rows still may. One that every member declined never goes on to the leader: it is
    /// shed (503 or asked again) when any member named a leader or a term, and `Absent` (create the group, ask again)
    /// only when none is joined. A value from any member is still used.
    #[test]
    fn a_whole_tier_read_every_member_declines_never_goes_to_the_leader() {
        for q in [
            json!({ "rows": true, "ranges": true, "packed": true }),
            json!({ "lsRefs": { "reads": [""], "head": true } }),
        ] {
            assert!(leader_never_serves(&q), "{q}");
        }
        for q in [
            json!({ "rows": { "merge": 7 }, "ranges": true, "packed": true }),
            json!({ "rows": { "api": { "issues": {} } } }),
            json!({ "rows": { "access": true, "agent": true } }),
            json!({ "probe": true }),
            json!({ "refs": { "prefix": "" } }),
        ] {
            assert!(!leader_never_serves(&q), "{q}");
        }
        let value = json!({ "value": 1, "index": 3 });
        let named = json!({ "error": { "notLeader": { "term": 3, "leader": 0 } } });
        let leaderless = json!({ "error": { "notLeader": { "term": 3, "leader": null } } });
        let null = Value::Null;
        for answers in [
            vec![Some((200, &named)), Some((200, &named)), Some((200, &named))],
            vec![Some((200, &leaderless)), Some((200, &named))],
            vec![Some((409, &null)), Some((200, &named))],
        ] {
            assert_eq!(spread_verdict(&answers, false), Spread::TierOff, "{answers:?}");
            assert_eq!(spread_verdict(&answers, true), Spread::Leader, "{answers:?}");
        }
        // No member names a leader: `Elect`, which sends the probe the leader's way and never the read.
        for answers in [
            vec![Some((200, &leaderless)), Some((200, &leaderless)), Some((200, &leaderless))],
            vec![Some((200, &leaderless)), Some((409, &null))],
        ] {
            assert_eq!(spread_verdict(&answers, false), Spread::Elect, "{answers:?}");
        }
        for answers in [vec![Some((409, &null)), Some((404, &null))], vec![]] {
            assert_eq!(spread_verdict(&answers, false), Spread::Absent, "{answers:?}");
        }
        assert_eq!(spread_verdict(&[Some((200, &named)), Some((200, &value))], false), Spread::Use(1));
        // Its reason is a shed's, which every caller already answers 503 or retries, and it names TIER_OFF apart.
        let why = tier_off_why(ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(9))));
        assert!(why.contains(OVERLOADED) && why.contains(TIER_OFF), "{why}");
        assert!(!format!("repo:9: every member asked shed the read: {OVERLOADED}").contains(TIER_OFF));
    }

    /// M4-WP4 j4: a group read only by dumps whose voters all answer `notLeader` naming no leader elects. The pass's
    /// verdict is `Elect`, which sends the probe, never the dump, the leader's way; that lookup reports the leaderless
    /// group to every voter with `suspect`, a voter campaigns on it, and the lookup finds the new leader within F67's
    /// 30 s bound. On tier-off-leader as first built the verdict was `TierOff`, which sent nothing: no report, no
    /// campaign, and the group stayed leaderless for over 7 minutes.
    #[test]
    fn a_leaderless_group_read_only_by_dumps_elects_and_never_serves_the_dump() {
        use futures_util::FutureExt;
        use std::cell::Cell;
        use std::rc::Rc;
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(65_536)));
        let m = |s: u8| graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Full, s, 1);
        let cfg = json!({ "index": 5, "config": { "normal": {
            "voters": [m(0), m(1), m(2)], "learners": [], "home": 0, "generation": 1 } } });
        learn(chain, &json!({ "config": cfg }));
        let dump = json!({ "q": { "rows": true, "ranges": true, "packed": true } });
        let leaderless = json!({ "error": { "notLeader": { "term": 4, "leader": null } } });
        let seen = [Some((200, &leaderless)), Some((200, &leaderless)), Some((200, &leaderless))];
        let verdict = spread_verdict(&seen, false);
        assert_eq!(verdict, Spread::Elect);
        let body = leader_body(&verdict, &dump).expect("a leaderless decline sends something the leader's way");
        assert!(!leader_never_serves(&serde_json::from_str::<Value>(&body).unwrap()["q"]), "never the dump: {body}");
        // A named leader's decline sends nothing: the read is `TierOff` and its retry finds a follower.
        let named = json!({ "error": { "notLeader": { "term": 4, "leader": 0 } } });
        assert_eq!(leader_body(&spread_verdict(&[Some((200, &named))], false), &dump), None);
        let names: Vec<(SiteId, String)> = KNOWN.with(|k| k.borrow()[&chain].voters.clone());
        let clock = Rc::new(Cell::new(0u64));
        // When a voter took the all-voters report that starts its campaign; it leads CAMPAIGN_MS later.
        const CAMPAIGN_MS: u64 = 1_000;
        let campaigned: Rc<Cell<Option<u64>>> = Rc::new(Cell::new(None));
        let bodies = Rc::new(RefCell::new(Vec::<String>::new()));
        struct At(Rc<Cell<u64>>, u64);
        impl Future for At {
            type Output = ();
            fn poll(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<()> {
                if self.0.get() >= self.1 { std::task::Poll::Ready(()) } else { std::task::Poll::Pending }
            }
        }
        let (c1, c2, c3, c4) = (clock.clone(), clock.clone(), clock.clone(), clock.clone());
        let (camp2, camp3, bodies2) = (campaigned.clone(), campaigned.clone(), bodies.clone());
        let mut req = Box::pin(lookup(
            chain,
            move || c1.get(),
            move |k| {
                let v = names.iter().map(|(s, n)| (*s, n.clone(), Step::Cached)).collect::<Vec<_>>();
                async move { (k == 0).then_some(v) }
            },
            move |s, _name, _by| {
                let (c, camp, bodies, body, cfg) =
                    (c2.clone(), camp2.clone(), bodies2.clone(), body.clone(), cfg.clone());
                async move {
                    bodies.borrow_mut().push(body);
                    let sent = c.get();
                    At(c.clone(), sent + 50).await;
                    let leads = camp.get().is_some_and(|t| c.get() >= t + CAMPAIGN_MS);
                    if leads && s == SiteId(1) {
                        return Some((200, json!({ "value": "probe", "meta": { "term": 5, "leader": 1 } })));
                    }
                    let leader = if leads { json!(1) } else { Value::Null };
                    let nl = json!({ "term": if leads { 5 } else { 4 }, "leader": leader, "config": cfg });
                    Some((200, json!({ "error": { "notLeader": nl }, "meta": { "term": 4 } })))
                }
            },
            move |_about: Target, _term, all| {
                let (c, camp) = (c3.clone(), camp3.clone());
                async move {
                    if all && camp.get().is_none() {
                        camp.set(Some(c.get()));
                    }
                    None
                }
            },
            move |d: Duration| At(c4.clone(), c4.get() + d.as_millis() as u64),
        ));
        let mut out = None;
        while out.is_none() && clock.get() < 30_000 {
            out = req.as_mut().now_or_never();
            clock.set(clock.get() + 1);
        }
        let (_, _, t) = out.expect("the lookup ended within F67's 30 s bound").expect("no error").expect("a group");
        assert_eq!(t.site, SiteId(1), "the lookup found the new leader");
        assert!(campaigned.get().is_some(), "the lookup reported the leaderless group, which starts the election");
        assert!(bodies.borrow().iter().all(|b| b == ELECT_PROBE), "only the probe went the leader's way");
    }

    /// `b032-shed` A: a member that forgot its leader sheds the read and names it. Every such answer is `Shed`, never a
    /// read for the leader, and `forgot_of` names the member to report, at the highest term named. A plain `notLeader`
    /// from every member still goes to the leader, and a shed with no `forgot` reports none.
    #[test]
    fn a_shed_that_names_a_forgotten_leader_is_reported_and_never_read_on_the_leader() {
        let forgot = |site: u8, term: u64| {
            let mut v = overloaded_reply(250);
            v["forgot"] = json!({ "site": site, "term": term });
            v
        };
        let (f3, f4) = (forgot(0, 3), forgot(0, 4));
        let named = json!({ "error": { "notLeader": { "term": 3, "leader": 0 } } });
        let shed = overloaded_reply(250);
        let all = [Some((200, &f3)), Some((200, &f4)), Some((200, &f3))];
        assert!(matches!(spread_verdict(&all, true), Spread::Shed(_)));
        assert_eq!(forgot_of(&all), Some((SiteId(0), 4)));
        assert_eq!(forgot_of(&[Some((200, &shed)), None]), None);
        assert_eq!(spread_verdict(&[Some((200, &named)), Some((200, &named))], true), Spread::Leader);
    }

    /// A shed spread read waits the longest `retryAfterMs` its members named, else OVERLOAD_RETRY_MS (one timed out or
    /// was `busy`).
    #[test]
    fn a_shed_spread_waits_what_its_members_named() {
        let (slow, fast, busy) = (overloaded_reply(700), overloaded_reply(300), json!({ "busy": true }));
        assert_eq!(spread_verdict(&[Some((200, &fast)), None, Some((200, &slow))], true), Spread::Shed(700));
        assert_eq!(spread_verdict(&[None, Some((200, &busy))], true), Spread::Shed(OVERLOAD_RETRY_MS));
    }

    /// `retry_shed` over shed answers naming `named` ms each (`sheds`), each pass taking `pass` ms, jitter `r`, from
    /// clock 1,000: the result, the waits taken, and the clock at the end.
    fn shed_run(
        until: u64,
        sheds: Vec<u64>,
        pass: u64,
        r: f64,
    ) -> (std::result::Result<&'static str, String>, Vec<u64>, u64) {
        use futures_util::FutureExt;
        use std::cell::{Cell, RefCell};
        let (clock, waits, left) = (Cell::new(1_000u64), RefCell::new(Vec::new()), RefCell::new(sheds));
        let r = retry_shed(
            until,
            || clock.get(),
            || {
                clock.set(clock.get() + pass);
                let mut l = left.borrow_mut();
                let next = (!l.is_empty()).then(|| l.remove(0));
                async move { next.map_or(Ok("value"), |ms| Err((format!("shed: {OVERLOADED}"), ms))) }
            },
            |ms| {
                waits.borrow_mut().push(ms);
                clock.set(clock.get() + ms);
                async {}
            },
            || r,
        )
        .now_or_never()
        .expect("no real wait");
        (r, waits.into_inner(), clock.get())
    }

    /// A write's shed read is asked again after each backed-off wait while that wait ends before its deadline, and
    /// answers once the members serve it; past the deadline, or with none (a read-class request), the shed is its
    /// error. A wait is never under what the members named.
    #[test]
    fn a_shed_read_is_retried_inside_its_deadline_only() {
        // Full jitter kept (r = 1): 250, then the members' 700 over the doubled 500.
        assert_eq!(shed_run(9_000, vec![250, 700], 0, 1.0), (Ok("value"), vec![250, 700], 1_950));
        // 1,000 + 250 = 1,250, then + 500 = 1,750 < 2,000, then + 1,000 is past it.
        let (r, waits, _) = shed_run(2_000, vec![250, 250, 250, 250], 0, 1.0);
        assert!(r.is_err_and(|e| e.contains(OVERLOADED)));
        assert_eq!(waits, vec![250, 500]);
        let (r, waits, _) = shed_run(1_000, vec![250], 0, 1.0);
        assert!(r.is_err() && waits.is_empty());
    }

    /// B0-33 m51: shed reads asked again every 250 ms for LOOKUP_FOR made the learners' reads ten times the clients'.
    /// Members that keep shedding (250 ms named each time) get a write a few doubling waits, each with up to half taken
    /// off at random, and then its error, well inside LOOKUP_FOR even with SHED_RETRY_FOR's deadline and slow passes.
    #[test]
    fn a_read_the_members_keep_shedding_backs_off_and_gives_up() {
        let all_shed = || vec![250; 100];
        let until = 1_000 + LOOKUP_FOR.as_millis() as u64;
        let (r, waits, _) = shed_run(until, all_shed(), 0, 1.0);
        assert!(r.is_err());
        assert_eq!(&waits[..5], &[250, 500, 1_000, 2_000, 2_000], "doubling to SHED_BACKOFF_MAX_MS");
        assert!(waits.len() <= 12, "{} tries in LOOKUP_FOR, against 80 at a fixed 250 ms", waits.len() + 1);
        let (_, low, _) = shed_run(until, all_shed(), 0, 0.0);
        assert_eq!(&low[..4], &[250, 250, 500, 1_000], "half taken off, never under the 250 ms the members named");
        // At the edge's deadline, with each pass taking three FOLLOWER_WAITs as on m51, the write has its error within
        // the deadline plus one pass, and asked at most three times.
        let until = 1_000 + SHED_RETRY_FOR.as_millis() as u64;
        let (r, waits, end) = shed_run(until, all_shed(), 3_000, 1.0);
        assert!(r.is_err());
        assert!(waits.len() < 3, "{waits:?}");
        assert!(end - 1_000 <= SHED_RETRY_FOR.as_millis() as u64 + 3_000, "gave up after {} ms", end - 1_000);
    }

    /// `shed-retry-2s`: on B0-33 m56 a write that met a shed made about five member reads before its 503, and those
    /// retries held the learners at 520 to 604 reads a second for 166 offered requests. A jammed pass takes at least
    /// one member's FOLLOWER_WAIT (1 s, a `late` shed, 62% of m56's learner sheds), and against such passes a write now
    /// asks at most twice (one pass and one backed-off retry) within SHED_RETRY_FOR, whatever the jitter: N writes make
    /// at most 2N passes, 6N member reads with SPREAD_TRIES members a pass. At 4 s they asked three times. Passes that
    /// shed at once still get at most four asks, the backoff's 250, 250 and 500 ms waits.
    #[test]
    fn writes_whose_members_always_shed_ask_at_most_twice_each() {
        let until = 1_000 + SHED_RETRY_FOR.as_millis() as u64;
        let writes = 50;
        let mut reads = 0;
        for (i, pass) in [1_000, 1_100, 1_250, 2_000, 3_000].into_iter().cycle().take(writes).enumerate() {
            let r = (i as f64 * 0.37).fract();
            let (res, waits, _) = shed_run(until, vec![250; 100], pass, r);
            assert!(res.is_err_and(|e| e.contains(OVERLOADED)));
            assert!(waits.len() <= 1, "write {i} (pass {pass} ms, jitter {r:.2}) asked {} times", waits.len() + 1);
            reads += (waits.len() + 1) * SPREAD_TRIES;
        }
        assert!(reads <= 2 * SPREAD_TRIES * writes, "{reads} member reads for {writes} writes");
        for r in [0.0, 0.5, 1.0] {
            let (_, waits, _) = shed_run(until, vec![250; 100], 0, r);
            assert!(waits.len() < 4, "an instant shed asked {} times", waits.len() + 1);
        }
    }

    /// A config's learners are learned with its voters, and an older config never replaces a newer one's.
    #[test]
    fn a_config_teaches_its_learners() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(13)));
        let p = |g: u32, learners: &[(u8, u8)]| {
            let voters =
                (0..3).map(|s| graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Full, 0, 1));
            let l = learners.iter().map(|&(s, k)| {
                graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Learner, k, g)
            });
            json!({ "voters": voters.collect::<Vec<_>>(), "learners": l.collect::<Vec<_>>(), "home": 0, "generation": g })
        };
        let nl = |index: u64, placement: Value| json!({ "term": 2, "config": { "index": index, "config": { "normal": placement } } });
        learn(chain, &nl(5, p(2, &[(1, 0), (2, 1)])));
        let k = KNOWN.with(|k| k.borrow().get(&chain).cloned()).unwrap();
        assert_eq!(k.voters.len(), 3);
        assert_eq!(k.learners.iter().map(|(s, _)| s.0).collect::<Vec<_>>(), [1, 2]);
        assert!(k.learners[0].1.contains("#l0"), "{:?}", k.learners);
        learn(chain, &nl(4, p(1, &[])));
        assert_eq!(KNOWN.with(|k| k.borrow().get(&chain).unwrap().learners.len()), 2, "an older config is ignored");
    }

    /// B0-32 m3fr2's retired voter: a `NotLeader` naming no leader from a member that says it was removed, or whose
    /// config is older than the one this isolate holds, is no evidence the group is leaderless; one from a member at
    /// the current config is. A `Degraded` config's lost site is no voter to ask.
    #[test]
    fn a_removed_or_stale_members_answer_is_not_a_leaderless_group() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(14)));
        let m = |s: u8, g: u32| graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Full, s, g);
        let place = |gen1: u32| json!({ "voters": [m(0, 1), m(1, gen1), m(2, 1)], "learners": [], "home": 0, "generation": gen1 });
        let nl = |index: u64, config: Value| json!({ "term": 2, "leader": null, "config": { "index": index, "config": config } });
        let answer = |nl: &Value, removed: bool| {
            let mut v = json!({ "error": { "notLeader": nl } });
            v["meta"] = if removed { json!({ "term": 2, "removed": true }) } else { json!({ "term": 2 }) };
            v
        };
        let current = nl(9, json!({ "normal": place(2) }));
        let old = nl(3, json!({ "degraded": { "placement": place(1), "lost": 1 } }));
        // Nothing learned yet: an old member's answer counts as stale only by its own `removed`.
        assert!(!stale_answer(chain, &answer(&old, false), &old));
        assert!(stale_answer(chain, &answer(&old, true), &old));
        learn(chain, &old);
        let k = KNOWN.with(|k| k.borrow().get(&chain).cloned()).unwrap();
        assert_eq!(k.voters.iter().map(|(s, _)| s.0).collect::<Vec<_>>(), [0, 2], "the lost site is no voter");
        learn(chain, &current);
        assert!(KNOWN.with(|k| k.borrow().get(&chain).unwrap().voters.iter().any(|(_, n)| n == "repo:14#2#f1")));
        assert!(stale_answer(chain, &answer(&old, false), &old), "index 3 against the 9 held");
        assert!(!stale_answer(chain, &answer(&current, false), &current), "the current config's answer counts");
        assert!(!stale_answer(
            chain,
            &answer(&nl(10, json!({ "normal": place(2) })), false),
            &nl(10, json!({ "normal": place(2) }))
        ));
    }

    /// Only a value from a follower is used. A follower that names the leader, sheds or is behind (`overloaded`), is
    /// `busy`, fails, or does not answer moves the read on, and none of these is a `Missed` answer here: the follower
    /// pass has no `suspect` path at all.
    #[test]
    fn only_a_followers_value_is_used() {
        assert!(follower_answer_usable(Some((200, &json!({ "value": 1, "index": 3 })))));
        let nl = json!({ "error": { "notLeader": { "term": 3, "leader": 1 } } });
        for (status, v) in [
            (200, nl),
            (200, json!({ "done": { "notLeader": { "term": 3 } } })),
            (200, overloaded_reply(250)),
            (200, json!({ "busy": true })),
            (503, Value::Null),
            (404, Value::Null),
        ] {
            assert!(!follower_answer_usable(Some((status, &v))), "{status} {v}");
        }
        assert!(!follower_answer_usable(None));
    }

    /// One isolate sends one report per (chain, member, term, scope) per SUSPECT_GAP; another member, another term,
    /// or the all-voters report about the same member is its own report, and the gap's end lets the next one out.
    #[test]
    fn suspect_reports_are_damped_per_member_and_term() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(11)));
        let gap = SUSPECT_GAP.as_millis() as u64;
        // lookup-retry: 1 s; 5 s broke the live sweep's bound.
        assert_eq!(gap, 1_000);
        assert!(2 * SUSPECT_GAP <= graftyard_raft::SUSPECT.1);
        assert!(report_due(chain, SiteId(0), 4, 1_000));
        assert!(!report_due(chain, SiteId(0), 4, 1_000 + gap - 1));
        assert!(report_due(chain, SiteId(1), 4, 1_001));
        assert!(report_due(chain, SiteId(0), 5, 1_002));
        assert!(report_due(chain, SiteId(0), 4, 1_000 + gap));
        let other = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(12)));
        assert!(report_due(other, SiteId(0), 4, 1_000 + gap));
    }

    /// m41's storm, driven by hand: 60 requests in one isolate find the cached leader gone and every voter answering
    /// `NotLeader` naming none for 2 s, until voter 1 leads. One request looks the leader up for all of them, the rest
    /// poll the cache on their own timers and go to the leader it found: posts to the voters while none led stay within one lookup's passes, and
    /// `suspect` reports within one per member and term per SUSPECT_GAP. On main each request ran its own lookup, and
    /// every request was answered by the leader as before, here within one post of the leader's first answer.
    #[test]
    fn one_lookup_per_chain_serves_every_request_that_met_not_leader() {
        use futures_util::FutureExt;
        use std::cell::Cell;
        use std::rc::Rc;
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(41)));
        let m = |s: u8| graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Full, s, 1);
        let config = json!({ "index": 5, "config": { "normal": {
            "voters": [m(0), m(1), m(2)], "learners": [], "home": 0, "generation": 1 } } });
        learn(chain, &json!({ "config": config }));
        learn_leader(chain, &json!({ "meta": { "term": 2, "leader": 0 } }));
        let names: Vec<(SiteId, String)> = KNOWN.with(|k| k.borrow()[&chain].voters.clone());
        let leads_at = 2_000u64;
        let post_ms = 50u64;
        let clock = Rc::new(Cell::new(0u64));
        struct At(Rc<Cell<u64>>, u64);
        impl Future for At {
            type Output = ();
            fn poll(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<()> {
                if self.0.get() >= self.1 { std::task::Poll::Ready(()) } else { std::task::Poll::Pending }
            }
        }
        // Posts per voter while no member led, and in all; `suspect` reports sent.
        let posts = Rc::new(RefCell::new([[0u32; 2]; 3]));
        let reports = Rc::new(Cell::new(0u32));
        let n = 60;
        let mut reqs: Vec<_> = (0..n)
            .map(|_| {
                let (c1, c2, c3, c4) = (clock.clone(), clock.clone(), clock.clone(), clock.clone());
                let (posts, reports, names, chain_) = (posts.clone(), reports.clone(), names.clone(), chain);
                let names2 = names.clone();
                Box::pin(lookup(
                    chain_,
                    move || c1.get(),
                    move |k| {
                        let v = names2.iter().map(|(s, n)| (*s, n.clone(), Step::Cached)).collect::<Vec<_>>();
                        async move { (k == 0).then_some(v) }
                    },
                    move |s, _name, _by| {
                        let (c, posts) = (c2.clone(), posts.clone());
                        let sent = c.get();
                        async move {
                            posts.borrow_mut()[s.0 as usize][usize::from(sent >= leads_at)] += 1;
                            At(c.clone(), sent + post_ms).await;
                            if c.get() >= leads_at && s == SiteId(1) {
                                return Some((200, json!({ "value": 1, "meta": { "term": 3, "leader": 1 } })));
                            }
                            let leader = if c.get() >= leads_at { json!(1) } else { Value::Null };
                            let nl = json!({ "term": 3, "leader": leader, "config": config_of(chain_) });
                            Some((200, json!({ "error": { "notLeader": nl }, "meta": { "term": 3 } })))
                        }
                    },
                    move |about: Target, term, _all| {
                        let (c, reports) = (c3.clone(), reports.clone());
                        async move {
                            if report_due(chain_, about.site, term, c.get()) {
                                reports.set(reports.get() + 1);
                            }
                            None
                        }
                    },
                    move |d: Duration| At(c4.clone(), c4.get() + d.as_millis() as u64),
                ))
            })
            .collect();
        fn config_of(chain: ChainId) -> Value {
            let m = |s: u8| graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Full, s, 1);
            json!({ "index": 5, "config": { "normal": {
                "voters": [m(0), m(1), m(2)], "learners": [], "home": 0, "generation": 1 } } })
        }
        let mut done: Vec<Option<u64>> = vec![None; n];
        while done.iter().any(Option::is_none) && clock.get() < 30_000 {
            // Several polls per ms, so a request woken by another's turn ending runs in the same ms.
            for _ in 0..3 {
                for (i, r) in reqs.iter_mut().enumerate() {
                    if done[i].is_none()
                        && let Some(out) = r.as_mut().now_or_never()
                    {
                        let (v, _, t) = out
                            .unwrap_or_else(|e| panic!("request {i} at {} ms: {e}; {:?}", clock.get(), posts.borrow()))
                            .expect("a group");
                        assert_eq!((t.site, v["value"].clone()), (SiteId(1), json!(1)), "the leader's own answer");
                        done[i] = Some(clock.get());
                    }
                }
            }
            clock.set(clock.get() + 1);
        }
        let posts = *posts.borrow();
        let before: u32 = posts.iter().map(|p| p[0]).sum();
        let latest = done.iter().map(|d| d.expect("every request answered")).max().unwrap();
        // One lookup's passes while none led: at most three posts a pass, a pass at least 250 ms apart, plus each
        // request's one post to its cached leader before it met `NotLeader`.
        let passes = leads_at.div_ceil(250 + post_ms) + 1;
        assert!(
            before <= n as u32 + 3 * passes as u32,
            "{before} posts to the voters while none led ({posts:?}), against {} for one lookup",
            n as u64 + 3 * passes
        );
        assert!(
            reports.get() <= 3 * (leads_at / SUSPECT_GAP.as_millis() as u64 + 1) as u32,
            "{} reports",
            reports.get()
        );
        // Every request is answered by the leader within one lookup pass and one post of its first answer.
        assert!(latest <= leads_at + 250 + 4 * post_ms, "the last request was answered at {latest} ms");
        assert_eq!(posts[1][1] as usize, n, "each request went to the leader once it led: {posts:?}");
    }

    /// probe-known: a config learned from a member's reply at the record's generation makes the group known, so
    /// `create` sends no probe, join, or `/campaign`; one of an older generation, held across a move to a record of the
    /// next, does not, or the moved group's new members would never be joined.
    #[test]
    fn a_config_is_known_only_at_the_records_generation_or_later() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(43)));
        let rec = |g: u32| PlacementRecord {
            name: graftyard_types::PlacementName::Tri,
            home: SiteId(0),
            sites: [SiteId(0), SiteId(1), SiteId(2)].into_iter().collect(),
            generation: g,
            moving_from: None,
            at: Timestamp(0),
        };
        let config = |index: u64, g: u32| {
            let m = |s: u8| graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Full, s, g);
            json!({ "config": { "index": index, "config": { "normal": {
                "voters": [m(0), m(1), m(2)], "learners": [], "home": 0, "generation": g } } } })
        };
        assert!(!config_known(chain, &rec(1)), "nothing learned");
        learn(chain, &config(5, 1));
        assert!(config_known(chain, &rec(1)));
        assert!(!config_known(chain, &rec(2)), "a move's record names members the held config does not");
        learn(chain, &config(9, 2));
        assert!(config_known(chain, &rec(2)), "the moved group's config, once a member of it answered");
        assert!(config_known(chain, &rec(1)), "an edge still on the old record");
        forget_leader(chain);
        assert!(config_known(chain, &rec(2)), "a lost leader is not a lost config");
    }

    /// A leader that answers the lookup's first post, from the record's bootstrap names, is cached: the next request
    /// posts it as the cached leader and takes no lookup mark. An isolate that never met a `NotLeader` holds no config,
    /// and when `cached_leader` needed the leader among the config's voters every later read walked under the mark,
    /// one request at a time (B0-29 run 5's edge, `tests/m0/results/merge-tail.md`).
    #[test]
    fn a_leader_found_by_its_bootstrap_name_is_cached() {
        use futures_util::FutureExt;
        let chain = ChainId::Names(graftyard_types::NameShard(708));
        let leader = "names:708#1#f0".to_string();
        let (n2, n3) = (leader.clone(), leader.clone());
        let out = lookup(
            chain,
            || 0,
            move |k| {
                let v = vec![(SiteId(0), n2.clone(), Step::Bootstrap { depth: 0 })];
                async move { (k == 0).then_some(v) }
            },
            |_s, _name, _by| async { Some((200, json!({ "value": 1, "meta": { "term": 3, "leader": 0 } }))) },
            |_about: Target, _term, _all| async { None },
            |_d: Duration| async {},
        )
        .now_or_never()
        .expect("ready")
        .expect("answered")
        .expect("a group");
        assert_eq!(out.2.do_name, n3);
        assert_eq!(cached_leader(chain).map(|t| t.do_name), Some(leader), "the answering leader is cached");
        assert!(mark(chain, 99, 0), "no lookup mark is left held");
        // A later reply names a leader on another site: the member kept is not that leader, so none is cached.
        learn_leader(chain, &json!({ "meta": { "term": 4, "leader": 1 } }));
        assert_eq!(cached_leader(chain), None, "site 0's member is not site 1's leader");
    }

    /// A group whose leader is truly gone is still elected without `create`: every voter answers `NotLeader` naming
    /// none, and each pass of the lookup reports the group with `suspect` to every voter (`all`), which makes a voter
    /// whose promise lapsed campaign. Once one leads, the lookup returns its answer.
    #[test]
    fn a_leaderless_group_is_reported_to_every_voter() {
        use futures_util::FutureExt;
        use std::cell::Cell;
        use std::rc::Rc;
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(44)));
        let m = |s: u8| graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Full, s, 1);
        let config = json!({ "index": 5, "config": { "normal": {
            "voters": [m(0), m(1), m(2)], "learners": [], "home": 0, "generation": 1 } } });
        learn(chain, &json!({ "config": config.clone() }));
        let names: Vec<(SiteId, String)> = KNOWN.with(|k| k.borrow()[&chain].voters.clone());
        let clock = Rc::new(Cell::new(0u64));
        let all_reports = Rc::new(Cell::new(0u32));
        let led = Rc::new(Cell::new(false));
        let (c1, c2, r, l1, l2) = (clock.clone(), clock.clone(), all_reports.clone(), led.clone(), led.clone());
        let mut req = Box::pin(lookup(
            chain,
            move || c1.get(),
            move |k| {
                let v = names.iter().map(|(s, n)| (*s, n.clone(), Step::Cached)).collect::<Vec<_>>();
                async move { (k == 0).then_some(v) }
            },
            move |s, _name, _by| {
                let (config, led) = (config.clone(), l1.get());
                async move {
                    if led && s == SiteId(2) {
                        return Some((200, json!({ "value": 1, "meta": { "term": 3, "leader": 2 } })));
                    }
                    let leader = if led { json!(2) } else { Value::Null };
                    let nl = json!({ "term": 3, "leader": leader, "config": config });
                    Some((200, json!({ "error": { "notLeader": nl }, "meta": { "term": 3 } })))
                }
            },
            move |_about: Target, _term, all| {
                // A report to every voter makes voter 2 campaign and win.
                if all {
                    r.set(r.get() + 1);
                    l2.set(true);
                }
                async move { None }
            },
            move |d: Duration| {
                c2.set(c2.get() + d.as_millis() as u64);
                async {}
            },
        ));
        let out = loop {
            if let Some(out) = req.as_mut().now_or_never() {
                break out;
            }
            assert!(clock.get() < 30_000, "no answer");
        };
        let (v, _, t) = out.expect("answered").expect("a group");
        assert_eq!((t.site, v["value"].clone()), (SiteId(2), json!(1)));
        assert_eq!(all_reports.get(), 1, "one pass that found no leader, reported to every voter");
    }

    /// lookup-retry's harness: one request on a 3-voter chain whose cached leader is voter 0 in term 2. `answer(site,
    /// n)` is voter `site`'s `n`th post (0-based): how long it takes, and its answer, `None` for a miss. Returns the
    /// result, the time it came, the posts per voter, and the reports as (about, all).
    #[allow(clippy::type_complexity)]
    fn drive_leader_lookup(
        chain: ChainId,
        answer: impl Fn(SiteId, u32) -> (u64, Option<(u16, Value)>) + 'static,
    ) -> (std::result::Result<Option<(Value, Step, Target)>, String>, u64, [u32; 3], Vec<(SiteId, bool)>) {
        use futures_util::FutureExt;
        use std::cell::Cell;
        use std::rc::Rc;
        let m = |s: u8| graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Full, s, 1);
        let config = json!({ "index": 5, "config": { "normal": {
            "voters": [m(0), m(1), m(2)], "learners": [], "home": 0, "generation": 1 } } });
        learn(chain, &json!({ "config": config }));
        learn_leader(chain, &json!({ "meta": { "term": 2, "leader": 0 } }));
        let names: Vec<(SiteId, String)> = KNOWN.with(|k| k.borrow()[&chain].voters.clone());
        let clock = Rc::new(Cell::new(0u64));
        let posts = Rc::new(RefCell::new([0u32; 3]));
        let reports = Rc::new(RefCell::new(Vec::new()));
        let (c1, c2, c3, p, r) = (clock.clone(), clock.clone(), clock.clone(), posts.clone(), reports.clone());
        let answer = Rc::new(answer);
        let mut req = Box::pin(lookup(
            chain,
            move || c1.get(),
            move |k| {
                let v = names.iter().map(|(s, n)| (*s, n.clone(), Step::Cached)).collect::<Vec<_>>();
                async move { (k == 0).then_some(v) }
            },
            move |s, _name, _by| {
                let n = p.borrow()[s.0 as usize];
                p.borrow_mut()[s.0 as usize] += 1;
                // Every mock future is ready at once, so a lookup that never ends loops inside one poll.
                assert!(p.borrow().iter().sum::<u32>() < 1_000, "no answer");
                let (c, answer) = (c2.clone(), answer.clone());
                async move {
                    let (ms, a) = answer(s, n);
                    c.set(c.get() + ms);
                    a
                }
            },
            move |about: Target, _term, all| {
                r.borrow_mut().push((about.site, all));
                async move { None }
            },
            move |d: Duration| {
                c3.set(c3.get() + d.as_millis() as u64);
                async {}
            },
        ));
        let out = loop {
            if let Some(out) = req.as_mut().now_or_never() {
                break out;
            }
            assert!(clock.get() < 60_000, "no answer");
        };
        let posts = *posts.borrow();
        let reports = reports.borrow().clone();
        (out, clock.get(), posts, reports)
    }

    /// lookup-retry: a cached leader whose read misses once, then answers, is asked again: no follower is posted, the
    /// miss is reported once, and the leader stays cached. On main the same miss posted both followers, whose
    /// `NotLeader` named the leader already tried, then forgot the leader and walked again.
    #[test]
    fn a_leader_that_misses_once_is_asked_again_and_no_follower_is_walked() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(46)));
        let nl = move |s: SiteId| {
            let m = |x: u8| graftyard_types::Member::new(chain, SiteId(x), graftyard_types::MemberKind::Full, x, 1);
            json!({ "error": { "notLeader": { "term": 2, "leader": 0, "config": { "index": 5, "config": { "normal": {
                "voters": [m(0), m(1), m(2)], "learners": [], "home": 0, "generation": 1 } } } } },
                "meta": { "term": 2, "leader": 0, "site": s.0 } })
        };
        for first in [None, Some((200, json!({ "busy": true }))), Some((503, Value::Null))] {
            let first2 = first.clone();
            let (out, _, posts, reports) = drive_leader_lookup(chain, move |s, n| match (s.0, n) {
                (0, 0) => (50, first2.clone()),
                (0, _) => (50, Some((200, json!({ "value": 7, "meta": { "term": 2, "leader": 0 } })))),
                _ => (50, Some((200, nl(s)))),
            });
            let (v, _, t) = out.expect("answered").expect("a group");
            assert_eq!((t.site, v["value"].clone()), (SiteId(0), json!(7)), "{first:?}");
            assert_eq!(posts, [2, 0, 0], "{first:?}: a follower was walked for a leader that answered on its retry");
            let busy = first.as_ref().is_some_and(|(st, v)| *st == 200 && v.get("busy").is_some());
            let want: Vec<(SiteId, bool)> = if busy { vec![] } else { vec![(SiteId(0), false)] };
            assert_eq!(reports, want, "{first:?}: a miss is reported once, a `busy` not at all");
            assert_eq!(cached_leader(chain).map(|l| l.site), Some(SiteId(0)), "{first:?}: the leader stays cached");
        }
    }

    /// lookup-miss-kind: a cached leader whose fetch throws `overloaded` (B0-35 m60's f0, CPU-blocked for about 5 s)
    /// is waited for and asked again, OBJECT_WAIT doubling, with no follower walked and no `suspect` sent: main read
    /// every thrown fetch as a miss, reported it, and after the second walked both followers. A thrown fetch without
    /// the flag is still a miss, now with its message and flags.
    #[test]
    fn an_overloaded_leader_is_waited_for_not_walked_away_from() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(53)));
        let over = thrown("Durable Object is overloaded. Too many requests queued.", true, false, false);
        let ok = json!({ "value": 3, "meta": { "term": 2, "leader": 0 } });
        let (o2, ok2) = (over.clone(), ok.clone());
        let (out, at, posts, reports) = drive_leader_lookup(chain, move |s, n| match (s.0, n) {
            (0, 0..=2) => (5, Some((THROWN, o2.clone()))),
            (0, _) => (50, Some((200, ok2.clone()))),
            _ => panic!("follower {s:?} walked for an overloaded leader"),
        });
        let (v, _, t) = out.expect("answered").expect("a group");
        assert_eq!((t.site, v["value"].clone()), (SiteId(0), json!(3)));
        assert_eq!(posts, [4, 0, 0]);
        assert!(reports.is_empty(), "an overloaded object is not reported: {reports:?}");
        let waits = (0..3).map(|n| (OBJECT_WAIT.as_millis() as u64) << n).sum::<u64>();
        assert!(at >= waits, "asked again after {at} ms, not after {waits} ms of waits");
        assert_eq!(cached_leader(chain).map(|l| l.site), Some(SiteId(0)));
        // One that stays overloaded ends the request as OVERLOADED, which callers answer 503, inside LOOKUP_FOR.
        let o3 = over.clone();
        let (out, at, posts, _) = drive_leader_lookup(chain, move |s, _| match s.0 {
            0 => (5, Some((THROWN, o3.clone()))),
            _ => panic!("follower {s:?} walked for an overloaded leader"),
        });
        assert!(out.is_err_and(|e| e.contains(OVERLOADED)), "{at} ms");
        assert!(at < LOOKUP_FOR.as_millis() as u64 && posts[0] > 3, "{at} ms, {posts:?}");
        let lost = thrown("Network connection lost.", false, true, false);
        assert_eq!(
            classify(Some((THROWN, &lost)), false),
            Answer::Missed("thrown \"Network connection lost.\" retryable".into())
        );
        assert_eq!(
            classify(Some((THROWN, &over)), true),
            Answer::ObjectOverloaded("thrown \"Durable Object is overloaded. Too many requests queued.\"".into())
        );
    }

    /// lookup-retry, the reverse: a cached leader that is gone (every post to it misses) is walked away from after its
    /// second miss, and the request is answered by the new leader within the old bound: two posts to the dead leader
    /// at CALL each, LEADER_RETRY between them, then the followers, where main took one CALL and the walk.
    #[test]
    fn a_leader_that_is_gone_is_walked_away_from_after_its_second_miss() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(47)));
        let call = CALL.as_millis() as u64;
        let m = |x: u8| graftyard_types::Member::new(chain, SiteId(x), graftyard_types::MemberKind::Full, x, 1);
        let cfg = json!({ "index": 5, "config": { "normal": {
            "voters": [m(0), m(1), m(2)], "learners": [], "home": 0, "generation": 1 } } });
        // A post to the dead leader runs to CALL; a follower answers in 50 ms. Voter 1 leads in term 3.
        let (out, at, posts, reports) = drive_leader_lookup(chain, move |s, _| match s.0 {
            0 => (call, None),
            1 => (50, Some((200, json!({ "value": 9, "meta": { "term": 3, "leader": 1 } })))),
            _ => (
                50,
                Some((
                    200,
                    json!({ "error": { "notLeader": { "term": 3, "leader": 1, "config": cfg.clone() } },
                "meta": { "term": 3, "leader": 1 } }),
                )),
            ),
        });
        let (v, _, t) = out.expect("answered").expect("a group");
        assert_eq!((t.site, v["value"].clone()), (SiteId(1), json!(9)));
        assert_eq!(posts[0], 2, "the dead leader is asked once more, not again and again");
        // The chain is not traced, so nothing was logged; `a_traced_lookup_logs_its_walk_and_answers_the_same` traces it.
        assert!(MARK_LINES.with(|l| l.borrow().is_empty()));
        assert_eq!(cached_leader(chain).map(|l| l.site), Some(SiteId(1)), "the new leader is cached");
        assert!(reports.iter().any(|(s, all)| *s == SiteId(0) && !all), "the dead leader is reported: {reports:?}");
        // Two misses at CALL, LEADER_RETRY between them, then at most two follower posts: main's one miss plus
        // LEADER_RETRY and one CALL, well inside LOOKUP_FOR.
        let bound = 2 * call + LEADER_RETRY.as_millis() as u64 + 2 * 50;
        assert!(at <= bound, "answered at {at} ms, past {bound}");
        assert!(at < LOOKUP_FOR.as_millis() as u64);
    }

    /// m58's term-4 walk (`tests/m4/raw/wp4/b035-m58/lookup-mark-t4.txt`) on chain `chain`: the held config names f0
    /// `#1#f0`, f1 by its new name `#8#f1` and f2 `#1#f2`, with no leader held, and the record is the first
    /// generation's, so its bootstrap names include the superseded `#1#f1`. f0 misses its first `f0_misses` posts in
    /// 10 ms, then answers as leader in term 4. f1 and f2 answer `NotLeader` naming `followers_name` (f2 only when
    /// `two_name`, else none); `#1#f1` answers `NotLeader` naming none from its old config. `generation` is the held
    /// config's. Returns the result and each post as (name, how it was sent, the leader held when it was sent).
    #[allow(clippy::type_complexity)]
    fn drive_m58_walk(
        chain: ChainId,
        f0_misses: u32,
        two_name: bool,
        generation: u32,
    ) -> (std::result::Result<Option<(Value, Step, Target)>, String>, Vec<(String, SentBy, Option<SiteId>)>) {
        use futures_util::FutureExt;
        use std::cell::Cell;
        use std::rc::Rc;
        let m = |s: u8, g: u32| graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Full, s, g);
        let cfg = json!({ "index": 9, "config": { "normal": {
            "voters": [m(0, 1), m(1, generation), m(2, 1)], "learners": [], "home": 0, "generation": generation } } });
        let old = json!({ "index": 3, "config": { "normal": {
            "voters": [m(0, 1), m(1, 1), m(2, 1)], "learners": [], "home": 0, "generation": 1 } } });
        learn(chain, &json!({ "config": cfg.clone() }));
        learn_leader(chain, &json!({ "meta": { "term": 3, "leader": null } }));
        let record = rec(1, None);
        let clock = Rc::new(Cell::new(0u64));
        let posts = Rc::new(RefCell::new(Vec::new()));
        let f0_posts = Rc::new(Cell::new(0u32));
        let (c1, c2, c3, p) = (clock.clone(), clock.clone(), clock.clone(), posts.clone());
        let (f1n, f2n) = (m(1, generation).do_name.to_string(), m(2, 1).do_name.to_string());
        let mut req = Box::pin(lookup(
            chain,
            move || c1.get(),
            move |k| {
                let known = KNOWN.with(|k| k.borrow().get(&chain).cloned()).unwrap_or_default();
                let names = match stage(&known, &record, chain, k) {
                    Some(Stage::Names(v)) => Some(v),
                    Some(Stage::Registry { .. }) => Some(vec![]),
                    None => None,
                };
                async move { names }
            },
            move |s, name: String, by| {
                p.borrow_mut().push((name.clone(), by, cached_leader(chain).map(|t| t.site)));
                assert!(p.borrow().len() < 1_000, "no answer");
                let nl = |leader: Value, cfg: &Value, term: u64| {
                    Some((
                        200,
                        json!({ "error": { "notLeader": { "term": term, "leader": leader, "config": cfg } },
                        "meta": { "term": term } }),
                    ))
                };
                let (ms, a) = if s == SiteId(0) {
                    f0_posts.set(f0_posts.get() + 1);
                    if f0_posts.get() <= f0_misses {
                        (10, None)
                    } else {
                        (35, Some((200, json!({ "value": 4, "meta": { "term": 4, "leader": 0 } }))))
                    }
                } else if name == f1n {
                    (100, nl(json!(0), &cfg, 4))
                } else if name == f2n {
                    (230, nl(if two_name { json!(0) } else { Value::Null }, &cfg, 4))
                } else {
                    (120, nl(Value::Null, &old, 2))
                };
                let c = c2.clone();
                async move {
                    c.set(c.get() + ms);
                    a
                }
            },
            |_about: Target, _term, _all| async { None },
            move |d: Duration| {
                c3.set(c3.get() + d.as_millis() as u64);
                async {}
            },
        ));
        let out = loop {
            if let Some(out) = req.as_mut().now_or_never() {
                break out;
            }
            assert!(clock.get() < 60_000, "no answer");
        };
        let posts = posts.borrow().clone();
        (out, posts)
    }

    /// F101, m58's term-4 walk: the record's bootstrap name `#1#f1`, which the held config replaced with `#8#f1`, is
    /// never posted, and after a pass in which f0 missed once while f1 and f2 named it, f0 stays held, so the next
    /// pass and every request waiting on the walk post to it as the leader. On main every pass posted `#1#f1`, and
    /// each one forgot f0.
    #[test]
    fn m58s_walk_skips_the_superseded_name_and_keeps_the_leader_two_followers_name() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(65572)));
        let (out, posts) = drive_m58_walk(chain, 2, true, 8);
        let (v, _, t) = out.expect("answered").expect("a group");
        assert_eq!((t.site, v["value"].clone()), (SiteId(0), json!(4)));
        assert!(!posts.iter().any(|(n, ..)| n.ends_with("#1#f1")), "posted the superseded name: {posts:#?}");
        let f0: Vec<_> = posts.iter().filter(|(n, ..)| n.ends_with("#1#f0")).collect();
        assert_eq!(f0[1].1, SentBy::Leader, "f0 was not kept after the first pass: {posts:#?}");
        assert_eq!(f0[1].2, Some(SiteId(0)), "{posts:#?}");
        assert_eq!(cached_leader(chain).map(|l| l.site), Some(SiteId(0)));
    }

    /// The reverse of the m58 case. A leader one follower names is not kept; a kept leader that misses again is
    /// forgotten on the next pass, so the walk goes on; and a config older than the record (a move's) leaves the
    /// record's bootstrap names in the walk, since its members are the ones being added.
    #[test]
    fn a_leader_is_kept_only_on_two_followers_word_and_once_and_bootstrap_names_stay_for_an_older_config() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(65573)));
        let (_, posts) = drive_m58_walk(chain, 2, false, 8);
        let f0: Vec<_> = posts.iter().filter(|(n, ..)| n.ends_with("#1#f0")).collect();
        assert_eq!((f0[1].1, f0[1].2), (SentBy::Walk, None), "kept on one follower's word: {posts:#?}");
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(65574)));
        let (out, posts) = drive_m58_walk(chain, 6, true, 8);
        assert_eq!(out.expect("answered").expect("a group").2.site, SiteId(0));
        let held: Vec<_> = posts.iter().filter(|(n, ..)| n.ends_with("#1#f0")).map(|p| p.2).collect();
        assert!(held.iter().filter(|h| h.is_some()).count() <= 2, "kept past one pass: {posts:#?}");
        assert!(held.last() == Some(&None), "a leader that kept missing was still held: {posts:#?}");
        let mut record = rec(9, None);
        record.generation = 9;
        let known = Known {
            voters: vec![(SiteId(0), "a".into()), (SiteId(1), "b".into()), (SiteId(2), "c".into())],
            generation: 8,
            ..Default::default()
        };
        let Some(Stage::Names(names)) = stage(&known, &record, chain, 0) else { panic!("stage 0 is names") };
        assert_eq!(names.len(), 6, "a move's new members dropped from the walk: {names:?}");
    }

    /// A follower's or learner's answer from a term below the highest this isolate saw keeps the leader held; one at
    /// or above it names the leader as any reply does.
    #[test]
    fn a_followers_answer_from_an_older_term_keeps_the_leader() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(45)));
        let voters = vec![(SiteId(0), "a".to_string()), (SiteId(1), "b".to_string()), (SiteId(2), "c".to_string())];
        KNOWN.with(|k| k.borrow_mut().insert(chain, Known { voters, config_index: 1, ..Default::default() }));
        learn_leader(chain, &json!({ "meta": { "term": 5, "leader": 2 } }));
        learn_leader_from_follower(chain, &json!({ "value": 1, "meta": { "term": 4, "leader": 1 } }));
        assert_eq!(cached_leader(chain).map(|t| t.site), Some(SiteId(2)), "a learner behind the term says nothing");
        learn_leader_from_follower(chain, &json!({ "value": 1, "meta": { "term": 6, "leader": 0 } }));
        assert_eq!(cached_leader(chain).map(|t| t.site), Some(SiteId(0)), "a newer term names the new leader");
        learn_leader_from_follower(chain, &json!({ "value": 1, "meta": { "term": 6, "leader": null } }));
        assert_eq!(cached_leader(chain), None, "the same term naming none: the follower lost it, as before");
    }

    /// The lookup mark is plain data with no future in it: the holder keeps it while it touches it within
    /// LOOKUP_MARK, another request takes it once it is free or stale, and only the holder frees it. A cancelled
    /// holder's request never drops its `Marked`, and the chain is free again LOOKUP_MARK after its last touch.
    #[test]
    fn a_lookup_mark_is_held_while_touched_and_free_once_stale() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(42)));
        let stale = LOOKUP_MARK.as_millis() as u64;
        assert!(mark(chain, 1, 1_000), "a free mark is taken");
        assert!(!mark(chain, 2, 1_000 + stale - 1), "held: another request waits");
        assert!(mark(chain, 1, 1_500), "the holder touches it");
        assert!(!mark(chain, 2, 1_500 + stale - 1), "touched, so still held");
        assert!(mark(chain, 2, 1_500 + stale), "untouched for LOOKUP_MARK: a cancelled holder no longer holds it");
        unmark(chain, 1);
        assert!(!mark(chain, 3, 1_500 + stale), "a former holder cannot free another's mark");
        drop(Marked(chain, 2));
        assert!(mark(chain, 3, 1_500 + stale), "the holder's drop frees it");
        let other = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(43)));
        assert!(mark(other, 4, 1_500), "marks are per chain");
    }

    /// Build 2 of `disk-first-reads`: a traced chain logs its mark holder's walk, each post with its member and answer,
    /// and how it ended, and the lookup answers exactly as an untraced one: the dead leader asked twice, then voter 1.
    #[test]
    fn a_traced_lookup_logs_its_walk_and_answers_the_same() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(48)));
        trace_marks(chain);
        let call = CALL.as_millis() as u64;
        let m = |x: u8| graftyard_types::Member::new(chain, SiteId(x), graftyard_types::MemberKind::Full, x, 1);
        let cfg = json!({ "index": 5, "config": { "normal": {
            "voters": [m(0), m(1), m(2)], "learners": [], "home": 0, "generation": 1 } } });
        let (out, _, posts, _) = drive_leader_lookup(chain, move |s, _| match s.0 {
            0 => (call, None),
            1 => (50, Some((200, json!({ "value": 9, "meta": { "term": 3, "leader": 1 } })))),
            _ => (
                50,
                Some((200, json!({ "error": { "notLeader": { "term": 3, "leader": 1, "config": cfg.clone() } } }))),
            ),
        });
        let (v, _, t) = out.expect("answered").expect("a group");
        assert_eq!((t.site, v["value"].clone(), posts[0]), (SiteId(1), json!(9), 2));
        let lines = MARK_LINES.with(|l| std::mem::take(&mut *l.borrow_mut()));
        assert!(lines.iter().any(|l| l.contains("post 1/") && l.contains("Reply")), "{lines:#?}");
        assert!(lines.last().is_some_and(|l| l.contains("found 1/")), "{lines:#?}");
    }

    /// `catchup-local`'s harness: a shard catch-up's read of `chain` while another request's walk (token 7) holds the
    /// chain's mark, touching it every poll until 15 s, as 13 walks in a row held `repo:65573`'s on B0-35 m59. The
    /// isolate holds the config but no leader, as each of those walks began. `local` is this site's member's answer,
    /// `None` for no member here; the leader (voter 0) answers `{"value": "leader"}` through `lookup`. Returns the
    /// answer, the virtual time it took, the posts to the leader's way, and the mark lines.
    fn drive_local_first(
        chain: ChainId,
        local: Option<Option<(u16, Value)>>,
        usable: impl Fn(&Value) -> bool,
    ) -> (std::result::Result<Value, String>, u64, u32, Vec<String>) {
        use futures_util::FutureExt;
        use std::cell::Cell;
        use std::rc::Rc;
        trace_marks(chain);
        let m = |s: u8| graftyard_types::Member::new(chain, SiteId(s), graftyard_types::MemberKind::Full, s, 1);
        let config = json!({ "index": 5, "config": { "normal": {
            "voters": [m(0), m(1), m(2)], "learners": [], "home": 0, "generation": 1 } } });
        learn(chain, &json!({ "config": config }));
        let names: Vec<(SiteId, String)> = KNOWN.with(|k| k.borrow()[&chain].voters.clone());
        let here = Target { site: names[1].0, do_name: names[1].1.clone() };
        assert!(mark(chain, 7, 0), "the other walk holds the mark");
        let clock = Rc::new(Cell::new(0u64));
        let walked = Rc::new(Cell::new(0u32));
        let (c0, c1, c2, w) = (clock.clone(), clock.clone(), clock.clone(), walked.clone());
        let leader = move || {
            lookup(
                chain,
                move || c1.get(),
                move |k| {
                    let v = names.iter().map(|(s, n)| (*s, n.clone(), Step::Cached)).collect::<Vec<_>>();
                    async move { (k == 0).then_some(v) }
                },
                move |s, _name, _by| {
                    w.set(w.get() + 1);
                    async move {
                        Some((
                            200,
                            if s == SiteId(0) {
                                json!({ "value": "leader", "meta": { "term": 3, "leader": 0 } })
                            } else {
                                json!({ "error": { "notLeader": { "term": 3, "leader": 0 } } })
                            },
                        ))
                    }
                },
                |_about: Target, _term, _all| async { None },
                move |d: Duration| {
                    c2.set(c2.get() + d.as_millis() as u64);
                    if c2.get() < 15_000 {
                        mark(chain, 7, c2.get());
                    }
                    async {}
                },
            )
            .map(|r| r.and_then(|o| o.map(|(v, _, _)| v).ok_or_else(|| "no group".to_string())))
        };
        let out = local_first(
            chain,
            move || c0.get(),
            local.as_ref().map(|_| here.clone()),
            "/read",
            usable,
            move |t: Target| {
                assert_eq!(t.site, SiteId(1), "the local read goes to this site's member");
                async move { local.flatten() }
            },
            leader,
        )
        .now_or_never()
        .expect("every mock is ready");
        let lines = MARK_LINES.with(|l| std::mem::take(&mut *l.borrow_mut()));
        unmark(chain, 7);
        (out, clock.get(), walked.get(), lines)
    }

    /// `catchup-local`: a catch-up's `ranges` read on its own site's member answers at once while another request's
    /// walk holds the chain's mark, and nothing goes the leader's way. Main sent it through the lookup, which waited
    /// on the mark until the walk let it go (15 s here, 19 s on B0-35 m59); that wait is the fallback's, and it is
    /// logged with the request that held the mark.
    #[test]
    fn a_catch_up_read_answers_from_its_site_while_another_walk_holds_the_mark() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(49)));
        let ranges =
            json!({ "value": { "first": 1, "promoted": null }, "index": 3529, "meta": { "term": 3, "leader": 0 } });
        let (out, ms, walked, lines) = drive_local_first(chain, Some(Some((200, ranges.clone()))), local_ranges_usable);
        assert_eq!((out, ms, walked), (Ok(ranges), 0, 0), "{lines:#?}");
        assert!(!lines.iter().any(|l| l.contains("waited")), "{lines:#?}");
        // A follower behind the leader's read index sheds it; the read then waits on the walk and is logged.
        let shed = json!({ "overloaded": { "retryAfterMs": 250 }, "meta": { "term": 3, "leader": 0 } });
        let (out, ms, walked, lines) = drive_local_first(chain, Some(Some((200, shed))), local_ranges_usable);
        assert_eq!((out, walked), (Ok(json!({ "value": "leader", "meta": { "term": 3, "leader": 0 } })), 1));
        assert!(ms >= 15_000, "the fallback waits out the walk: {ms} ms");
        assert!(lines.iter().any(|l| l.starts_with("local /read at 1/") && l.contains("to the leader")), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("waited") && l.contains("on t7, then took the mark")), "{lines:#?}");
        // No member on this site: the leader's way, as before.
        let (out, _, walked, _) = drive_local_first(chain, None, local_ranges_usable);
        assert_eq!((out.map(|v| v["value"].clone()), walked), (Ok(json!("leader")), 1));
    }

    /// `catchup-local`'s guards: a local `ranges` answer is taken only as a read-index value, never a `NotLeader`, a
    /// `busy` or a shed; a local page only when it starts at `from`, so a member whose commit is below the entry asked
    /// for is never read past.
    #[test]
    fn a_local_catch_up_read_is_taken_only_at_or_above_what_was_asked() {
        let v = json!({ "value": { "first": 1 }, "index": 9 });
        assert!(local_ranges_usable(&v));
        for bad in [
            json!({ "error": { "notLeader": { "term": 3, "leader": 0 } } }),
            json!({ "overloaded": { "retryAfterMs": 250 } }),
            json!({ "busy": true }),
            json!({ "value": null, "index": 9 }),
        ] {
            assert!(!local_ranges_usable(&bad), "{bad}");
        }
        let page = |first: Option<u64>| json!({ "entries": first.map_or(vec![], |i| vec![json!({ "index": i })]) });
        assert!(local_page_usable(&page(Some(3529)), 3529));
        for (p, why) in
            [(page(None), "empty: commit below from"), (page(Some(3530)), "a gap"), (page(Some(3528)), "below")]
        {
            assert!(!local_page_usable(&p, 3529), "{why}");
        }
        // A page that is not taken goes to the leader.
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(50)));
        let (out, _, walked, _) =
            drive_local_first(chain, Some(Some((200, page(None)))), |v| local_page_usable(v, 3529));
        assert_eq!((out.map(|v| v["value"].clone()), walked), (Ok(json!("leader")), 1));
        let (out, _, _, _) = drive_local_first(chain, Some(None), |v| local_page_usable(v, 3529));
        assert_eq!(out.map(|v| v["value"].clone()), Ok(json!("leader")), "a missed local read goes to the leader");
    }

    /// The mark-wait log (F104): a request that waits on another's mark logs how long and on whom once the wait ends,
    /// and a miss of a post to the cached leader, which is what sends a request into a walk, is logged too.
    #[test]
    fn a_wait_on_another_requests_mark_is_logged_with_its_holder() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(51)));
        let (_, ms, _, lines) = drive_local_first(chain, None, local_ranges_usable);
        let line = lines.iter().find(|l| l.contains(" waited ")).unwrap_or_else(|| panic!("{lines:#?}"));
        assert!(line.contains(&format!("waited {ms} ms on t7, then took the mark")), "{line}");
        assert!(lines.iter().any(|l| l.contains("found 0/")), "{lines:#?}");
        // A traced cached leader that misses is logged even though its post takes no mark.
        let traced = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(52)));
        trace_marks(traced);
        let (_, _, posts, _) = drive_leader_lookup(traced, |s, n| match (s.0, n) {
            (0, 0) => (CALL.as_millis() as u64, None),
            _ => (50, Some((200, json!({ "value": 1, "meta": { "term": 2, "leader": 0 } })))),
        });
        assert_eq!(posts[0], 2);
        let lines = MARK_LINES.with(|l| std::mem::take(&mut *l.borrow_mut()));
        assert!(lines.iter().any(|l| l.contains("cached leader 0/") && l.contains("Missed")), "{lines:#?}");
    }

    /// No future, waker or I/O object crosses requests through this crate's isolate-wide state: a Workers request owns
    /// its I/O and closures, and lookup-share's `LOOKING` wakers ran one request's lookup end in others' futures (m42,
    /// m3f: "Cannot perform I/O on behalf of a different request"). Every `thread_local!` here holds plain data; this
    /// reads the source, so a shared waker, task or stream added later fails here, and `tests/lookupbed/run.sh` checks
    /// the behaviour in workerd.
    #[test]
    fn no_isolate_state_holds_a_future_or_waker() {
        let src = include_str!("lib.rs");
        let mut statics = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find("thread_local! {") {
            let body = &rest[i..];
            let end = body.find("\n}\n").expect("a thread_local! block ends");
            statics.push(&body[..end]);
            rest = &body[end..];
        }
        assert!(statics.len() >= 4, "found {} thread_local! blocks", statics.len());
        for block in statics.iter().filter(|b| !b.contains("include_str!")) {
            for banned in ["Waker", "Future", "Promise", "Stream", "Closure", "Request", "Response", "Stub", "Pin<"] {
                assert!(!block.contains(banned), "isolate-wide state holds a {banned}:\n{block}");
            }
        }
    }

    #[test]
    fn a_lookup_reads_registry_rows_only_after_the_names_missed() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(7)));
        let record = rec(2, Some(rec(1, None)));
        let known = Known {
            voters: vec![(SiteId(0), "a".into()), (SiteId(1), "b".into())],
            config_index: 3,
            leader: Some(SiteId(1)),
            ..Default::default()
        };
        let Some(Stage::Names(first)) = stage(&known, &record, chain, 0) else { panic!("stage 0 reads names") };
        let order: Vec<(u8, Step)> = first.iter().map(|(s, _, st)| (s.0, *st)).collect();
        assert_eq!(&order[..2], &[(1, Step::Cached), (0, Step::Cached)]);
        assert!(order[2..].iter().all(|(_, st)| *st == Step::Bootstrap { depth: 0 }));
        assert_eq!(first.len(), 5);
        assert!(matches!(stage(&known, &record, chain, 1), Some(Stage::Registry { depth: 0, .. })));
        let Some(Stage::Names(moved)) = stage(&known, &record, chain, 2) else { panic!("stage 2 reads names") };
        assert!(moved.iter().all(|(_, n, st)| *st == Step::Bootstrap { depth: 1 } && n.contains("#1#")));
        assert!(matches!(stage(&known, &record, chain, 3), Some(Stage::Registry { depth: 1, .. })));
        assert!(stage(&known, &record, chain, 4).is_none());
        assert!(matches!(stage(&Known::default(), &record, chain, 0), Some(Stage::Names(v)) if v.len() == 3));
    }

    /// `dep_read` driven by hand: request `i` (in the order sent) is sent at the clock's time and ends `ends[i]` ms
    /// later with its result, or never. Returns the answer, how the hedge ended, the requests sent, and the clock then.
    fn run_dep_read(
        chain: ChainId,
        ends: Vec<Option<(u64, Result<u32, &'static str>)>>,
    ) -> (Result<u32, &'static str>, Hedged, usize, u64) {
        use futures_util::FutureExt;
        use std::cell::Cell;
        use std::rc::Rc;
        struct At(Rc<Cell<u64>>, u64);
        impl Future for At {
            type Output = ();
            fn poll(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<()> {
                if self.0.get() >= self.1 { std::task::Poll::Ready(()) } else { std::task::Poll::Pending }
            }
        }
        let clock = Rc::new(Cell::new(0u64));
        let sent = Cell::new(0usize);
        let read = || {
            let i = sent.get();
            sent.set(i + 1);
            let (c, end) = (clock.clone(), ends.get(i).cloned().flatten());
            async move {
                match end {
                    Some((ms, r)) => {
                        let at = c.get() + ms;
                        At(c, at).await;
                        r
                    }
                    None => std::future::pending().await,
                }
            }
        };
        let delay = |d: Duration| At(clock.clone(), clock.get() + d.as_millis() as u64);
        let now = || clock.get();
        let mut f = Box::pin(dep_read(chain, read, delay, now));
        for _ in 0..20_000 {
            if let Some((r, how)) = (&mut f).now_or_never() {
                return (r, how, sent.get(), clock.get());
            }
            clock.set(clock.get() + 1);
        }
        panic!("the read never ended");
    }

    /// `dep-hedge`: B0-31 m3fr12's opening stall had one `Authz` dependency read held 4.6 s before the leader's
    /// handler, while a later request from the same isolate was served in 186 ms (F83). A read with no answer by
    /// DEP_HEDGE sends a second and takes the first answer of the two; one answered in time sends nothing more; a
    /// request that fails does not end the read while the other may still answer. On main the read waited the held
    /// request out (`tests/m0carried/plant35.py`, mutant 0).
    #[test]
    fn a_dependency_read_with_no_answer_by_dep_hedge_asks_again_and_takes_the_first_answer() {
        let h = DEP_HEDGE.as_millis() as u64;
        // An untimed chain: every read here waits DEP_HEDGE, since it has fewer than DEP_HEDGE_MIN read times.
        let run_dep_read = |ends| run_dep_read(ChainId::Ids, ends);
        // m3fr12: the first request held 4.6 s, the second answered in 9 ms.
        assert_eq!(run_dep_read(vec![Some((4_611, Ok(1))), Some((9, Ok(2)))]), (Ok(2), Hedged::Won, 2, h + 9));
        // The typical read: answered at 301 ms (deps:authz p50), no second request.
        assert_eq!(run_dep_read(vec![Some((301, Ok(1))), Some((9, Ok(2)))]), (Ok(1), Hedged::First, 1, 301));
        // A slow first that still beats the second.
        assert_eq!(run_dep_read(vec![Some((h + 50, Ok(1))), Some((400, Ok(2)))]), (Ok(1), Hedged::Asked, 2, h + 50));
        // The second never answers: the first's answer, whenever it comes.
        assert_eq!(run_dep_read(vec![Some((3_000, Ok(1))), None]), (Ok(1), Hedged::Asked, 2, 3_000));
        // The first fails after the second was sent: the read waits for the second, not the error.
        assert_eq!(
            run_dep_read(vec![Some((h + 100, Err("no member answered"))), Some((500, Ok(2)))]),
            (Ok(2), Hedged::Won, 2, h + 500)
        );
        // The second fails first: the first's answer.
        assert_eq!(
            run_dep_read(vec![Some((h + 900, Ok(1))), Some((10, Err("busy")))]),
            (Ok(1), Hedged::Asked, 2, h + 900)
        );
        // Both fail: the first's error, once both have ended.
        assert_eq!(
            run_dep_read(vec![Some((h + 100, Err("first"))), Some((300, Err("second")))]),
            (Err("first"), Hedged::Asked, 2, h + 300)
        );
        // A first that fails before DEP_HEDGE ends the read with its error, as before (the caller tries the
        // owner's current record next).
        assert_eq!(
            run_dep_read(vec![Some((40, Err("first"))), Some((10, Ok(2)))]),
            (Err("first"), Hedged::First, 1, 40)
        );
    }

    /// `dep-hedge-adaptive`: each chain's hedge waits the p90 of its own last 64 read times in this isolate, within
    /// DEP_HEDGE_FLOOR and DEP_HEDGE. A near chain (B0-29 run 5, reads 40 to 170 ms) hedges a held read at its p90,
    /// where main waited 1 s; a far chain (B0-31 m3fr12, apac reading enam, 250 to 700 ms) keeps its own longer wait,
    /// so its typical read sends no second request; a chain read under the floor waits the floor. On main every read
    /// below waited DEP_HEDGE (`tests/m0carried/plant42.py`, mutant 0).
    #[test]
    fn a_dependency_reads_hedge_waits_its_chains_own_p90() {
        let near = ChainId::Tokens(graftyard_types::TokenShard(901));
        let far = ChainId::Authz(graftyard_types::AccountId(graftyard_types::DbId(7_100_901)));
        let fast = ChainId::Names(graftyard_types::NameShard(901));
        let h = DEP_HEDGE.as_millis() as u64;
        // 15 read times: still untimed, so a 400 ms read sends nothing more.
        for t in 0..15 {
            dep_read_took(near, 40 + t);
        }
        assert_eq!(run_dep_read(near, vec![Some((400, Ok(1))), Some((9, Ok(2)))]), (Ok(1), Hedged::First, 1, 400));
        // The window now holds 40..=54 and that 400. 64 more at 40, 42, ..., 166 ms push all of them out: the p90
        // (index 57 of 64) is 154 ms. A first request held 300 ms is hedged then, and the second, answering in 60,
        // wins; its 214 ms joins the window in place of the 40, so the p90 moves to 156.
        for t in 0..64 {
            dep_read_took(near, 40 + 2 * t);
        }
        assert_eq!(run_dep_read(near, vec![Some((300, Ok(1))), Some((60, Ok(2)))]), (Ok(2), Hedged::Won, 2, 214));
        // The far chain, timed apart: 64 reads at 250, 257, ..., 691 ms, p90 649 ms. Its 600 ms read sends nothing
        // more; a held one is hedged at 649. The near chain's wait is unchanged by them: 156, from its own window.
        for t in 0..64 {
            dep_read_took(far, 250 + 7 * t);
        }
        assert_eq!(run_dep_read(far, vec![Some((600, Ok(1))), Some((9, Ok(2)))]), (Ok(1), Hedged::First, 1, 600));
        assert_eq!(run_dep_read(far, vec![Some((4_000, Ok(1))), Some((9, Ok(2)))]), (Ok(2), Hedged::Won, 2, 658));
        assert_eq!(run_dep_read(near, vec![Some((300, Ok(1))), Some((60, Ok(2)))]), (Ok(2), Hedged::Won, 2, 216));
        // 64 reads answered at 4 s, each timed through `dep_read` (a second sent after 649 ms or more answers later
        // still): the wait stops at DEP_HEDGE.
        for _ in 0..64 {
            assert_eq!(run_dep_read(far, vec![Some((4_000, Ok(1))), Some((3_500, Ok(2)))]).0, Ok(1));
        }
        assert_eq!(run_dep_read(far, vec![Some((4_000, Ok(1))), Some((9, Ok(2)))]), (Ok(2), Hedged::Won, 2, h + 9));
        // A chain read in 20 ms waits the floor, not 20: a 90 ms read sends nothing more, a 300 ms one is hedged at 100.
        for _ in 0..64 {
            dep_read_took(fast, 20);
        }
        assert_eq!(run_dep_read(fast, vec![Some((90, Ok(1))), Some((9, Ok(2)))]), (Ok(1), Hedged::First, 1, 90));
        let floor = DEP_HEDGE_FLOOR.as_millis() as u64;
        assert_eq!(run_dep_read(fast, vec![Some((300, Ok(1))), Some((9, Ok(2)))]), (Ok(2), Hedged::Won, 2, floor + 9));
    }
}
