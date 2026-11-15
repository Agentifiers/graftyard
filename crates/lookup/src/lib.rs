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
}

thread_local! {
    static KNOWN: RefCell<HashMap<ChainId, Known>> = RefCell::new(HashMap::new());
    static COLO: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// Per isolate: each learner's answer times to `read_spread` as this isolate saw them (`Timed`).
    static ANSWER_MS: RefCell<HashMap<(SiteId, String), Timed>> = RefCell::new(HashMap::new());
}

/// The colo this isolate's `suspect` reports from `call_opt` carry; an edge sets it per request.
pub fn set_colo(colo: u32) {
    COLO.with(|c| c.set(colo));
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
    KNOWN.with(|k| {
        let mut k = k.borrow_mut();
        let e = k.entry(chain).or_default();
        if index >= e.config_index {
            e.voters = voters;
            e.learners = learners;
            e.config_index = index;
        }
    });
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

fn learn_leader(chain: ChainId, v: &Value) {
    let leader = v.pointer("/meta/leader").and_then(Value::as_u64).and_then(|s| u8::try_from(s).ok()).map(SiteId);
    KNOWN.with(|k| k.borrow_mut().entry(chain).or_default().leader = leader);
}

pub fn forget_leader(chain: ChainId) {
    KNOWN.with(|k| {
        if let Some(e) = k.borrow_mut().get_mut(&chain) {
            e.leader = None;
        }
    });
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
    let stub = b.member(to, do_name).ok()?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(js_sys::Uint8Array::from(body).into()));
    // The sender's clock: a member raises its own clock to the freshest stamp it sees (`MEMBER_CLOCK`). Edge
    // isolates are many and rarely saturated, so theirs stay fresh when every member is busy.
    let h = Headers::new();
    h.set(MEMBER_CLOCK, &now_ms().to_string()).ok()?;
    init.with_headers(h);
    let req = Request::new_with_init(&format!("https://member{path}"), &init).ok()?;
    let call = async move {
        let mut res = stub.fetch_with_request(req).await.ok()?;
        let status = res.status_code();
        let v = res.json::<Value>().await.unwrap_or(Value::Null);
        Some((status, v))
    };
    within(deadline, call).await.flatten()
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
    out.extend(
        r.bootstrap_members(chain).into_iter().map(|m| (m.site, m.do_name.to_string(), Step::Bootstrap { depth })),
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
    let end = now_ms() + LOOKUP_FOR.as_millis() as u64;
    let mut last = String::from("no member answered");
    let mut first_pass = true;
    let mut term = 0;
    let mut busy = 0;
    while now_ms() < end {
        let mut k = 0;
        let mut cands = candidates(b, chain, record, k).await.unwrap_or_default();
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
        let mut queue: std::collections::VecDeque<(SiteId, String, Step)> = std::mem::take(&mut cands).into();
        loop {
            let Some((s, name, step)) = queue.pop_front() else {
                k += 1;
                match candidates(b, chain, record, k).await {
                    Some(next) => {
                        queue.extend(next);
                        continue;
                    }
                    None => break,
                }
            };
            if !tried.insert(name.clone()) {
                continue;
            }
            let r = post(b, s, &name, path, body).await;
            let v = match classify(r.as_ref().map(|(s, v)| (*s, v)), matches!(step, Step::Bootstrap { .. })) {
                Answer::Missed(why) => {
                    last = format!("{name}: {why}");
                    others += 1;
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
                // The leader is alive and shedding heavy reads: no `suspect`, not missed, not leaderless. The same
                // member is asked again after its `retryAfterMs`, OVERLOAD_TRIES times, then the caller answers 503.
                Answer::Overloaded(ms) => {
                    others += 1;
                    busy += 1;
                    if busy > OVERLOAD_TRIES {
                        return Err(format!("{chain}: {name}: {OVERLOADED} {ms} ms"));
                    }
                    Delay::from(Duration::from_millis(ms.clamp(50, 2_000))).await;
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
            return Ok(Some((v, step, Target { site: s, do_name: name })));
        }
        if first_pass && unjoined > 0 && others == 0 {
            return Ok(None);
        }
        first_pass = false;
        let colo = COLO.with(|c| c.get());
        let reports = missed.iter().map(|m| suspect(b, chain, m, term, colo, false));
        futures_util::future::join_all(reports).await;
        if let Some(t) = &leaderless {
            suspect(b, chain, t, term, colo, true).await;
        }
        forget_leader(chain);
        Delay::from(Duration::from_millis(250)).await;
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
}

/// `read_spread`'s choice from the answers of the members `spread_order` named, in order.
fn spread_verdict(answers: &[Option<(u16, &Value)>]) -> Spread {
    if let Some(i) = answers.iter().position(|r| follower_answer_usable(*r)) {
        return Spread::Use(i);
    }
    let declined = |r: &Option<(u16, &Value)>| match r {
        Some((200, v)) => v.pointer("/error/notLeader").is_some() || v.pointer("/done/notLeader").is_some(),
        Some((s, _)) => (400..500).contains(s),
        None => false,
    };
    if answers.iter().all(declined) {
        return Spread::Leader;
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
    let until = now_ms() + retry_for.as_millis() as u64;
    let wait = |ms| Delay::from(Duration::from_millis(ms));
    retry_shed(until, now_ms, || spread_once(b, chain, record, body), wait).await.and_then(|r| r)
}

/// Runs `attempt` until it answers, waiting the ms each shed answer names, while that wait ends before `until` (ms);
/// a shed answer past it is the read's error, naming OVERLOADED.
async fn retry_shed<T, A, W>(
    until: u64,
    now: impl Fn() -> u64,
    mut attempt: impl FnMut() -> A,
    mut wait: impl FnMut(u64) -> W,
) -> std::result::Result<T, String>
where
    A: Future<Output = std::result::Result<T, (String, u64)>>,
    W: Future<Output = ()>,
{
    loop {
        match attempt().await {
            Ok(v) => return Ok(v),
            Err((_, ms)) if now() + ms < until => wait(ms).await,
            Err((why, _)) => return Err(why),
        }
    }
}

/// One pass of `read_spread`: `Err` with the wait when every member asked shed it.
#[allow(clippy::type_complexity)]
async fn spread_once(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    body: &Value,
) -> std::result::Result<std::result::Result<Option<(Value, Step, Target)>, String>, (String, u64)> {
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
    let mut indexed = body.clone();
    indexed["index"] = json!(true);
    let bytes = indexed.to_string();
    let mut answers = Vec::new();
    // The learners come first in the order, so the first `n_learners` asked are learners, and only they are timed.
    for (i, t) in timed_spread_order(learners, voters, known.leader, pick).into_iter().enumerate() {
        let start = now_ms();
        let r = post_within(b, t.site, &t.do_name, "/read", bytes.as_bytes(), FOLLOWER_CALL).await;
        let seen = r.as_ref().map(|(s, v)| (*s, v));
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
        }
        answers.push(r);
        if usable {
            let v = answers.pop().flatten().map(|(_, v)| v).unwrap_or_default();
            learn_leader(chain, &v);
            refresh_config(b, chain, &t, &v).await;
            return Ok(Ok(Some((v, Step::Cached, t))));
        }
    }
    let seen: Vec<Option<(u16, &Value)>> = answers.iter().map(|r| r.as_ref().map(|(s, v)| (*s, v))).collect();
    match spread_verdict(&seen) {
        Spread::Use(_) => unreachable!("a usable answer returned above"),
        Spread::Shed(ms) => Err((format!("{chain}: every member asked shed the read: {OVERLOADED}"), ms)),
        Spread::Leader => Ok(call_opt(b, chain, record, "/read", body.to_string().as_bytes()).await),
    }
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
    /// An answer to read: the value, or `NotLeader`.
    Reply,
}

fn classify(r: Option<(u16, &Value)>, bootstrap: bool) -> Answer {
    let Some((status, v)) = r else { return Answer::Missed("no answer".into()) };
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

/// `call_opt` for a group that must exist: no group is an error.
pub async fn call_raw(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    path: &str,
    body: &[u8],
) -> std::result::Result<(Value, Step, Target), String> {
    call_opt(b, chain, record, path, body).await?.ok_or_else(|| format!("{chain}: no group"))
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

/// The receipt deadline of a commit: COMMIT_HOLD plus CALL_DEADLINE.
pub const COMMIT_RECEIPT: Duration = Duration::from_millis(2_000);

/// The member the caller last saw lead `chain`, if its config names it.
pub fn cached_leader(chain: ChainId) -> Option<Target> {
    let k = KNOWN.with(|k| k.borrow().get(&chain).cloned())?;
    let leader = k.leader?;
    k.voters.into_iter().find(|(s, _)| *s == leader).map(|(site, do_name)| Target { site, do_name })
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
    if !report_due(chain, about.site, term, all, now_ms()) {
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
pub const SUSPECT_GAP: Duration = Duration::from_millis(500);

thread_local! {
    static REPORTED: RefCell<HashMap<(ChainId, SiteId, u64, bool), u64>> = RefCell::new(HashMap::new());
}

/// Whether this isolate sends the report now (`SUSPECT_GAP`), recording it if so.
fn report_due(chain: ChainId, about: SiteId, term: u64, all: bool, now: u64) -> bool {
    REPORTED.with(|r| {
        let mut r = r.borrow_mut();
        let gap = SUSPECT_GAP.as_millis() as u64;
        if r.len() > 4_096 {
            r.retain(|_, at| now < *at + gap);
        }
        match r.get(&(chain, about, term, all)) {
            Some(at) if now < *at + gap => false,
            _ => {
                r.insert((chain, about, term, all), now);
                true
            }
        }
    })
}

/// `commit_hedged`'s test hooks. `first`, when given, is the body of the first `/commit` send only (a test install's
/// kill rides on it, so a fault fires once); every later send is the command's body. `trace`, when given, collects one
/// row per step: each `/commit` send with its target and why the edge chose it, each missed receipt, and each
/// `suspect` answer (BF-14's count).
#[derive(Clone, Copy, Default)]
pub struct Hooks<'a> {
    pub first: Option<&'a [u8]>,
    pub trace: Option<&'a RefCell<Vec<Value>>>,
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
    let Hooks { first, trace } = hooks;
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
            None => match call_raw(b, chain, record, "/outcome_by_request", body).await {
                Ok((v, _, t)) => {
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
        let r = post_within(b, t.site, &t.do_name, "/commit", first.take().unwrap_or(body), COMMIT_RECEIPT).await;
        let Some((200, v)) = r else {
            last = format!("{}: {}", t.do_name, r.as_ref().map_or("missed".into(), |(s, _)| s.to_string()));
            note(json!({ "missed": t.site.0, "status": r.map(|(s, _)| s) }));
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
        };
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
    // A read that answers nothing of the fold: a repository's `{"q": {}}` is every ref, and at 303,144 refs it ran
    // the leader and the edge out of memory on every commit (`tests/m2/results/m2-wp1.md`, "2026-11-04"). A member
    // built before `probe` answers it as `{"q": {}}`.
    let probe = json!({ "q": { "probe": true } }).to_string();
    if let Ok(Some(_)) = call_opt(b, chain, record, "/read", probe.as_bytes()).await {
        return Ok(());
    }
    let members = record.bootstrap_members(chain);
    let joins =
        members.iter().map(|m| {
            let mut j = join.clone();
            j["chain"] = json!(chain);
            j["me"] = json!(m);
            j["record"] = json!(record);
            async move {
                post_within(b, m.site, &m.do_name, "/join", j.to_string().as_bytes(), Duration::from_secs(60)).await
            }
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
    let end = now_ms() + LOOKUP_FOR.as_millis() as u64;
    while now_ms() < end {
        let _ = post(b, home.site, &home.do_name, "/campaign", b"{}").await;
        if let Ok(Some((_, _, leader))) = call_opt(b, chain, record, "/read", probe.as_bytes()).await {
            return place(b, chain, record, leader).await;
        }
        Delay::from(Duration::from_millis(200)).await;
    }
    Err(format!("{chain}: created, but no member leads"))
}

/// The word in `create`'s error when a majority of the bootstrap members refused to join because the chain's source
/// holds no row for it yet: a repository whose create has not committed `RepoOwner`. The edge answers a read of it as
/// for a repository the caller cannot see, 404, as GitHub answers before a create returns; every other refusal stays
/// a 503, so an outage never reads as "not found" (`decisions/DECISIONS.md`, "`fresh-repo`").
pub const NOT_CREATED: &str = "not created yet";

/// Whether `create`'s error says the chain is not created yet (`NOT_CREATED`).
pub fn is_not_created(e: &str) -> bool {
    e.contains(NOT_CREATED)
}

/// `create`'s error when too few members joined and no group answered (`probe`). It names `NOT_CREATED` only when a
/// majority of the bootstrap members answered `{"refused": {"noRepoOwner": ..}}`: each read `RepoOwner` through its
/// `Names` leader and found no row, which no outage answers. A member that did not answer, refused for any other
/// reason, or answered from a release before `noRepoOwner` counts against it.
fn refused(chain: ChainId, answers: &[Option<(u16, Value)>], probe: &str) -> String {
    let unsourced =
        answers.iter().filter(|a| matches!(a, Some((409, v)) if v.pointer("/refused/noRepoOwner").is_some())).count();
    let why = if unsourced * 2 > answers.len() { format!("{NOT_CREATED}: ") } else { String::new() };
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

    /// A read during a repository's create, before `RepoOwner` commits, is "not created" only when a majority of the
    /// bootstrap members found no row; members that did not answer, refused for another reason (such as a `Names`
    /// read that failed), or come from an older release keep it an outage, which the edge answers 503.
    #[test]
    fn only_a_majority_with_no_repo_owner_row_is_not_created() {
        let chain = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(131316)));
        let none = || Some((409, json!({ "refused": { "noRepoOwner": 131316 } })));
        let failed = || Some((409, json!({ "refused": { "source": "names:994: no member answered" } })));
        let old = || Some((409, json!({ "refused": { "source": "RepoId(DbId(131316)) has no RepoOwner row" } })));
        let joined = || Some((200, json!([])));
        let cases = [
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
            assert_eq!(is_not_created(&e), want, "{e}");
            assert!(e.starts_with("repo:131316: "), "{e}");
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
        assert_eq!(spread_verdict(&[Some((200, &shed)), Some((200, &value))]), Spread::Use(1));
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
            assert!(matches!(spread_verdict(&answers), Spread::Shed(_)), "{answers:?}");
        }
        for answers in [
            vec![Some((200, &named)), Some((200, &named))],
            vec![Some((409, &null)), Some((409, &null))],
            vec![Some((200, &named)), Some((404, &null))],
            vec![Some((200, &leaderless)), Some((200, &named))],
            vec![],
        ] {
            assert_eq!(spread_verdict(&answers), Spread::Leader, "{answers:?}");
        }
    }

    /// A shed spread read waits the longest `retryAfterMs` its members named, else OVERLOAD_RETRY_MS (one timed out or
    /// was `busy`).
    #[test]
    fn a_shed_spread_waits_what_its_members_named() {
        let (slow, fast, busy) = (overloaded_reply(700), overloaded_reply(300), json!({ "busy": true }));
        assert_eq!(spread_verdict(&[Some((200, &fast)), None, Some((200, &slow))]), Spread::Shed(700));
        assert_eq!(spread_verdict(&[None, Some((200, &busy))]), Spread::Shed(OVERLOAD_RETRY_MS));
    }

    /// A write's shed read is asked again after each wait its members named while that wait ends before its deadline,
    /// and answers once they serve it; past the deadline, or with none (a read-class request), the shed is its error.
    #[test]
    fn a_shed_read_is_retried_inside_its_deadline_only() {
        use futures_util::FutureExt;
        use std::cell::{Cell, RefCell};
        let run = |until: u64, sheds: Vec<u64>| {
            let (clock, waits, left) = (Cell::new(1_000u64), RefCell::new(Vec::new()), RefCell::new(sheds));
            let r = retry_shed(
                until,
                || clock.get(),
                || {
                    let mut l = left.borrow_mut();
                    let next = (!l.is_empty()).then(|| l.remove(0));
                    async move { next.map_or(Ok("value"), |ms| Err((format!("shed: {OVERLOADED}"), ms))) }
                },
                |ms| {
                    waits.borrow_mut().push(ms);
                    clock.set(clock.get() + ms);
                    async {}
                },
            )
            .now_or_never()
            .expect("no real wait");
            (r, waits.into_inner())
        };
        assert_eq!(run(9_000, vec![700, 300]), (Ok("value"), vec![700, 300]));
        // 1,000 + 700 = 1,700 < 2,000, then 1,700 + 400 is past it.
        let (r, waits) = run(2_000, vec![700, 400, 100]);
        assert!(r.is_err_and(|e| e.contains(OVERLOADED)));
        assert_eq!(waits, vec![700]);
        let (r, waits) = run(1_000, vec![250]);
        assert!(r.is_err() && waits.is_empty());
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
        assert!(report_due(chain, SiteId(0), 4, false, 1_000));
        assert!(!report_due(chain, SiteId(0), 4, false, 1_000 + gap - 1));
        assert!(report_due(chain, SiteId(1), 4, false, 1_001));
        assert!(report_due(chain, SiteId(0), 5, false, 1_002));
        assert!(report_due(chain, SiteId(0), 4, true, 1_003));
        assert!(report_due(chain, SiteId(0), 4, false, 1_000 + gap));
        let other = ChainId::Repo(graftyard_types::RepoId(graftyard_types::DbId(12)));
        assert!(report_due(other, SiteId(0), 4, false, 1_000 + gap));
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
}
