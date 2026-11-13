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

/// How long one lookup-and-call keeps trying.
pub const LOOKUP_FOR: Duration = Duration::from_secs(20);
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
    let p = match &cfg {
        GroupConfig::Normal(p) | GroupConfig::Degraded { placement: p, .. } => p,
        GroupConfig::Joint { new, .. } => new,
    };
    let voters = p.voters().iter().map(|m| (m.site, m.do_name.to_string())).collect();
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
                term = term.max(nl.get("term").and_then(Value::as_u64).unwrap_or(0));
                learn(chain, nl);
                if nl.get("config").is_none_or(Value::is_null) {
                    last = format!("{name}: no config");
                    continue;
                }
                if nl.get("leader").is_none_or(Value::is_null) {
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

/// How long `read_spread` waits for a usable answer from the members it asked before it asks the next one beside them
/// [inference]: a learner's answer took 0.75 s at the edge at the median of a 500-agent run, and one in five was
/// `late` after FOLLOWER_WAIT and asked again 0.9 to 1.5 s in, so a second member asked at 0.4 s answers a slow read
/// first; replayed over that run's samples it put the tier read's p99 near 1.9 s at about two calls a read
/// (`tests/am1/results/narrow-reads-3.md`, "The tier read's tail").
pub const SPREAD_HEDGE: Duration = Duration::from_millis(400);

thread_local! {
    /// Per member object, a moving average of its time to a usable read-index answer from this isolate, in ms; an
    /// answer that was not usable counts as FOLLOWER_CALL, and a call dropped unanswered counts as its time so far.
    static SPEED: RefCell<HashMap<String, f32>> = RefCell::new(HashMap::new());
}

/// `SPEED` entries kept per isolate before it starts again: about a thousand groups' learners and followers.
const SPEED_MAX: usize = 4_096;

fn speed_of(do_name: &str) -> f32 {
    SPEED.with(|s| s.borrow().get(do_name).copied().unwrap_or(0.0))
}

fn note_speed(do_name: &str, ms: f32) {
    SPEED.with(|s| {
        let mut s = s.borrow_mut();
        if s.len() >= SPEED_MAX && !s.contains_key(do_name) {
            s.clear();
        }
        let e = s.entry(do_name.to_string()).or_insert(ms);
        *e = 0.8 * *e + 0.2 * ms;
    });
}

/// The members a read-index read goes to, in order: the cached config's learners, rotated by `pick`, then its
/// followers (the voters, else the record's bootstrap members, without the cached leader), rotated by `pick` too, at
/// most SPREAD_TRIES. With no leader cached every voter is a candidate: a member that leads serves the read itself.
/// Of the learners, a second one is picked at random too, and of the two the one `speed` rates faster goes first (two
/// random choices, so no one learner draws every read); a member this isolate has not timed rates 0 and is tried.
fn spread_order(
    learners: Vec<(SiteId, String)>,
    voters: Vec<(SiteId, String)>,
    leader: Option<SiteId>,
    pick: usize,
    speed: impl Fn(&str) -> f32,
) -> Vec<Target> {
    let rotated = |v: Vec<(SiteId, String)>| {
        let mut out: Vec<Target> = v.into_iter().map(|(site, do_name)| Target { site, do_name }).collect();
        if !out.is_empty() {
            let n = pick % out.len();
            out.rotate_left(n);
        }
        out
    };
    let mut out = rotated(learners);
    if out.len() > 1 {
        let second = 1 + (pick / out.len()) % (out.len() - 1);
        if speed(&out[second].do_name) < speed(&out[0].do_name) {
            let t = out.remove(second);
            out.insert(0, t);
        }
    }
    out.extend(rotated(voters.into_iter().filter(|(s, _)| Some(*s) != leader).collect()));
    out.truncate(SPREAD_TRIES);
    out
}

/// One read raced over `n` members, in order: the first is asked at once, and the next once `hedge` passes with no
/// usable answer from those asked, or at once when one of them answers with no usable value. Only the first `hedged`
/// are asked on a hedge (learners: a voter's reads delay its AppendEntries replies); a later one only after an answer
/// with no value. The first usable answer is the read's, with its member's position, and the others in flight are
/// dropped; with none, every answer, by position. Any member's value is one the read may use, so the race picks none
/// that waiting would not.
async fn race<R, A: Future<Output = R>, T: Future<Output = ()>>(
    n: usize,
    hedged: usize,
    mut ask: impl FnMut(usize) -> A,
    mut hedge: impl FnMut() -> T,
    usable: impl Fn(&R) -> bool,
) -> std::result::Result<(usize, R), Vec<(usize, R)>> {
    use futures_util::FutureExt;
    use futures_util::stream::{FuturesUnordered, StreamExt};
    let mut flying = FuturesUnordered::new();
    let mut asked = 0;
    let mut timer: Option<std::pin::Pin<Box<T>>> = None;
    let mut got = Vec::new();
    loop {
        if asked < n && timer.is_none() {
            let i = asked;
            asked += 1;
            flying.push(ask(i).map(move |r| (i, r)));
            timer = (asked < n.min(hedged)).then(|| Box::pin(hedge()));
        }
        let next = match timer.as_mut() {
            Some(t) => match select(flying.next(), t).await {
                Either::Left((r, _)) => r,
                Either::Right(_) => {
                    timer = None;
                    continue;
                }
            },
            None => flying.next().await,
        };
        let Some((i, r)) = next else { break };
        if usable(&r) {
            return Ok((i, r));
        }
        got.push((i, r));
        timer = None;
    }
    got.sort_by_key(|(i, _)| *i);
    Err(got)
}

/// What `read_spread` does with the answers it got, in order.
#[derive(Debug, PartialEq, Eq)]
enum Spread {
    /// The answer at this position is the read's value.
    Use(usize),
    /// None answered a value and at least one could not take it: `overloaded` (at its caps, behind, or installing),
    /// `busy`, a 5xx, or no answer within FOLLOWER_CALL ("Durable Object is overloaded" is a failed call).
    /// The caller answers 503 with `Retry-After`, and the leader is not asked, so a read the members shed never lands
    /// on the leader's thread.
    Shed,
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
    if answers.iter().all(declined) { Spread::Leader } else { Spread::Shed }
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
/// the members `spread_order` names, learners first, until one answers with a value, the next asked beside the
/// earlier ones once SPREAD_HEDGE passes (`race`). If none did and one shed it,
/// the read fails with an error naming OVERLOADED, which the caller answers 503 with `Retry-After`, and the leader is
/// never asked; only if none shed it does it go to the leader by `call_opt` (`spread_verdict`). A member's answer
/// holds every entry acknowledged before the read arrived: it waited for the leader's confirmed commit index.
/// Learners and followers never raise `suspect` and never count as missed.
pub async fn read_spread(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    body: &Value,
) -> std::result::Result<Option<(Value, Step, Target)>, String> {
    let known = KNOWN.with(|k| k.borrow().get(&chain).cloned()).unwrap_or_default();
    let voters = if known.voters.is_empty() {
        record.bootstrap_members(chain).into_iter().map(|m| (m.site, m.do_name.to_string())).collect()
    } else {
        known.voters
    };
    let pick = (js_sys::Math::random() * 1024.0) as usize;
    let mut indexed = body.clone();
    indexed["index"] = json!(true);
    let bytes = indexed.to_string();
    let learners = known.learners.len();
    let order = spread_order(known.learners, voters, known.leader, pick, speed_of);
    let ask = |i: usize| {
        let t = order[i].clone();
        let bytes = bytes.as_bytes();
        async move {
            // Times the call even when the race drops it unanswered: its time so far is a floor on its answer's.
            struct Timed(String, u64, bool);
            impl Drop for Timed {
                fn drop(&mut self) {
                    if !self.2 {
                        note_speed(&self.0, now_ms().saturating_sub(self.1) as f32);
                    }
                }
            }
            let mut timed = Timed(t.do_name.clone(), now_ms(), false);
            let r = post_within(b, t.site, &t.do_name, "/read", bytes, FOLLOWER_CALL).await;
            if let Some((_, v)) = &r {
                learn_config(chain, v);
            }
            let usable = follower_answer_usable(r.as_ref().map(|(s, v)| (*s, v)));
            let ms = now_ms().saturating_sub(timed.1) as f32;
            note_speed(&t.do_name, if usable { ms } else { ms.max(FOLLOWER_CALL.as_millis() as f32) });
            timed.2 = true;
            r
        }
    };
    let usable = |r: &Option<(u16, Value)>| follower_answer_usable(r.as_ref().map(|(s, v)| (*s, v)));
    let answers = match race(order.len(), learners, ask, || Delay::from(SPREAD_HEDGE), usable).await {
        Ok((i, r)) => {
            let v = r.map(|(_, v)| v).unwrap_or_default();
            learn_leader(chain, &v);
            refresh_config(b, chain, &order[i], &v).await;
            return Ok(Some((v, Step::Cached, order[i].clone())));
        }
        Err(answers) => answers,
    };
    let seen: Vec<Option<(u16, &Value)>> = answers.iter().map(|(_, r)| r.as_ref().map(|(s, v)| (*s, v))).collect();
    match spread_verdict(&seen) {
        Spread::Use(_) => unreachable!("a usable answer returned above"),
        Spread::Shed => Err(format!("{chain}: every member asked shed the read: {OVERLOADED}")),
        Spread::Leader => call_opt(b, chain, record, "/read", body.to_string().as_bytes()).await,
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
        term = term.max(v.pointer("/meta/term").and_then(Value::as_u64).unwrap_or(0));
        if let Some(nl) = v.pointer("/error/notLeader").or_else(|| v.pointer("/done/notLeader")) {
            learn(chain, nl);
            forget_leader(chain);
            let named = nl.get("leader").and_then(Value::as_u64);
            target = match named {
                Some(_) => None,
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
/// the generation, then campaigns on the record's home until a member leads, and then has the leader run the near
/// check (`place`), which may re-place a follower and fails the create if no try passes. Joins are idempotent, and a
/// member of a generation that lost the claim refuses, so two callers at once still build one group.
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
            .map_err(|e| format!("{chain}: {joined} of {} joined ({answers:?}); {e}", members.len()));
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

/// How long `create` waits for a new group's `/place`: PLACE_TRIES joins, each measuring two voters 20 times, plus
/// the leader's own 20 round trips to each, and three config changes [inference from apac round trips of about 250 ms].
const PLACE_FOR: Duration = Duration::from_secs(240);

/// The near check on a new group (`design/DESIGN.md`, "Placement is checked by latency"): its leader re-places a
/// voter until the home has one within `NEAR_RTT`, or keeps the nearest try; only an error such as a lost leader or
/// a refused config change fails the create, never the bound alone.
async fn place(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    leader: Target,
) -> std::result::Result<(), String> {
    let mut leader = leader;
    for _ in 0..3 {
        match post_within(b, leader.site, &leader.do_name, "/place", b"{}", PLACE_FOR).await {
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

/// A system chain's record from the `Release` view.
pub async fn system_record(
    b: &Bindings<'_>,
    anchor: &graftyard_release::Anchor,
    chain: ChainId,
) -> std::result::Result<PlacementRecord, String> {
    let sc = system_of(chain).ok_or_else(|| format!("{chain} is not a system chain"))?;
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
        let flat = |_: &str| 0.0;
        assert_eq!(names(spread_order(none(), all.clone(), Some(SiteId(1)), 0, flat)), ["a", "c"]);
        assert_eq!(names(spread_order(none(), all.clone(), Some(SiteId(1)), 1, flat)), ["c", "a"]);
        assert_eq!(names(spread_order(none(), all.clone(), Some(SiteId(1)), 4, flat)), ["a", "c"]);
        assert_eq!(names(spread_order(none(), all.clone(), None, 2, flat)), ["c", "a", "b"]);
        assert_eq!(spread_order(none(), Vec::new(), Some(SiteId(1)), 3, flat), Vec::new());
        // Learners (on sites 1 and 2, beside voters) come before any follower, rotated on their own.
        let learners = v(&[(1, "l0"), (2, "l1")]);
        assert_eq!(names(spread_order(learners.clone(), all.clone(), Some(SiteId(0)), 0, flat)), ["l0", "l1", "b"]);
        assert_eq!(names(spread_order(learners.clone(), all.clone(), Some(SiteId(0)), 1, flat)), ["l1", "l0", "c"]);
        // Four learners: three asked, no follower.
        let four = v(&[(0, "l0"), (1, "l1"), (2, "l2"), (0, "l3")]);
        assert_eq!(names(spread_order(four.clone(), all.clone(), Some(SiteId(0)), 3, flat)), ["l3", "l0", "l1"]);
        // Of the rotated first learner and a second random one, the faster goes first; the rest keep their order and
        // followers never move ahead of a learner. `pick` 3 + 4 × 1: rotation 3, second pick 1 + 1 % 3 = position 2.
        let slow_l3 = |n: &str| if n == "l3" { 900.0 } else { 100.0 };
        assert_eq!(names(spread_order(four.clone(), all.clone(), Some(SiteId(0)), 7, slow_l3)), ["l1", "l3", "l0"]);
        // A slower second pick, or a tie, leaves the rotation as it was.
        let slow_l1 = |n: &str| if n == "l1" { 900.0 } else { 100.0 };
        assert_eq!(names(spread_order(four.clone(), all.clone(), Some(SiteId(0)), 7, slow_l1)), ["l3", "l0", "l1"]);
        // An untimed member (0) beats a timed one, so a new learner is tried.
        let timed_l3 = |n: &str| if n == "l3" { 50.0 } else { 0.0 };
        assert_eq!(names(spread_order(four, all.clone(), Some(SiteId(0)), 7, timed_l3)), ["l1", "l3", "l0"]);
        // One learner: no second pick, so a follower never goes before it however slow it is.
        let one = v(&[(2, "l0")]);
        assert_eq!(
            names(spread_order(one, all, Some(SiteId(0)), 5, |n: &str| if n == "l0" { 2e3 } else { 1.0 })),
            ["l0", "c", "b"]
        );
    }

    /// A test `race`: each member's answer and the hedge timer are channels the test fires; `asked` records who was
    /// asked, in order.
    type RaceOut = Option<std::result::Result<(usize, u32), Vec<(usize, u32)>>>;

    struct Race {
        pool: futures_executor::LocalPool,
        asked: std::rc::Rc<RefCell<Vec<usize>>>,
        answers: Vec<Option<futures_channel::oneshot::Sender<u32>>>,
        hedges: std::rc::Rc<RefCell<Vec<futures_channel::oneshot::Sender<()>>>>,
        out: std::rc::Rc<RefCell<RaceOut>>,
    }

    /// Answers are `u32`s: usable when even, and `u32::MAX` for a call that failed.
    fn start_race(n: usize) -> Race {
        start_race_hedged(n, n)
    }

    fn start_race_hedged(n: usize, hedged: usize) -> Race {
        use futures_util::task::LocalSpawnExt;
        let pool = futures_executor::LocalPool::new();
        let asked = std::rc::Rc::new(RefCell::new(Vec::new()));
        let hedges = std::rc::Rc::new(RefCell::new(Vec::new()));
        let out = std::rc::Rc::new(RefCell::new(None));
        let (mut txs, rxs): (Vec<_>, Vec<_>) = (0..n).map(|_| futures_channel::oneshot::channel::<u32>()).unzip();
        let mut rxs: Vec<_> = rxs.into_iter().map(Some).collect();
        let (a, h, o) = (asked.clone(), hedges.clone(), out.clone());
        pool.spawner()
            .spawn_local(async move {
                let ask = |i: usize| {
                    a.borrow_mut().push(i);
                    let rx = rxs[i].take().expect("each member asked once");
                    async move { rx.await.unwrap_or(u32::MAX) }
                };
                let hedge = || {
                    let (tx, rx) = futures_channel::oneshot::channel::<()>();
                    h.borrow_mut().push(tx);
                    async move {
                        let _ = rx.await;
                    }
                };
                *o.borrow_mut() = Some(race(n, hedged, ask, hedge, |r: &u32| r.is_multiple_of(2)).await);
            })
            .unwrap();
        let mut r = Race { pool, asked, answers: txs.drain(..).map(Some).collect(), hedges, out };
        r.pool.run_until_stalled();
        r
    }

    impl Race {
        fn answer(&mut self, i: usize, v: u32) {
            if let Some(tx) = self.answers[i].take() {
                let _ = tx.send(v);
            }
            self.pool.run_until_stalled();
        }
        /// Fires the newest hedge timer.
        fn hedge(&mut self) {
            let tx = self.hedges.borrow_mut().pop().expect("a hedge timer is running");
            let _ = tx.send(());
            self.pool.run_until_stalled();
        }
        fn asked(&self) -> Vec<usize> {
            self.asked.borrow().clone()
        }
        fn out(&self) -> RaceOut {
            self.out.borrow().clone()
        }
    }

    /// The first member is asked alone; the next only once the hedge passes with no usable answer; the third only on
    /// the next hedge. The first usable answer wins whichever member it came from, and is the read's.
    #[test]
    fn a_slow_member_is_hedged_and_the_first_value_wins() {
        let mut r = start_race(3);
        assert_eq!(r.asked(), [0], "one member at first");
        assert_eq!(r.out(), None);
        r.hedge();
        assert_eq!(r.asked(), [0, 1], "the second once the hedge passed");
        // The first member, asked earlier, answers first: its value is used though the second was asked.
        r.answer(0, 10);
        assert_eq!(r.out(), Some(Ok((0, 10))));
        assert_eq!(r.asked(), [0, 1], "no third member after a value");

        let mut r = start_race(3);
        r.hedge();
        r.answer(1, 4);
        assert_eq!(r.out(), Some(Ok((1, 4))), "the hedged member's value, while the first still waits");
        assert_eq!(r.asked(), [0, 1]);
    }

    /// An answer with no value moves the read to the next member at once, without waiting out the hedge; with no value
    /// from anyone, the read waits for every member asked and returns all answers by position for `spread_verdict`.
    #[test]
    fn an_unusable_answer_asks_the_next_at_once_and_none_usable_keeps_them_all() {
        let mut r = start_race(3);
        r.answer(0, 1);
        assert_eq!(r.asked(), [0, 1], "asked at once after a shed");
        r.hedge();
        assert_eq!(r.asked(), [0, 1, 2]);
        r.answer(2, 3);
        assert_eq!(r.out(), None, "member 1 is still in flight");
        r.answer(1, u32::MAX);
        assert_eq!(r.out(), Some(Err(vec![(0, 1), (1, u32::MAX), (2, 3)])), "every answer, by position");
        // A late value after unusable answers is still used.
        let mut r = start_race(2);
        r.answer(0, 7);
        r.answer(1, 8);
        assert_eq!(r.out(), Some(Ok((1, 8))));
        // Never more members than offered, however many hedges pass.
        let mut r = start_race(2);
        r.hedge();
        assert!(r.hedges.borrow().is_empty(), "no timer once every member was asked");
        assert_eq!(r.asked(), [0, 1]);
        r.answer(1, 5);
        r.answer(0, 9);
        assert_eq!(r.out(), Some(Err(vec![(0, 9), (1, 5)])));
        // No member at all: nothing asked, and no answers.
        let r = start_race(0);
        assert_eq!((r.asked(), r.out()), (vec![], Some(Err(vec![]))));
    }

    /// A hedge reaches only the first `hedged` members (the learners): a follower is asked only after an answer with no
    /// value, so hedging never doubles a voter's reads.
    #[test]
    fn a_hedge_never_reaches_a_follower() {
        let mut r = start_race_hedged(3, 2);
        r.hedge();
        assert_eq!(r.asked(), [0, 1]);
        assert!(r.hedges.borrow().is_empty(), "no timer toward the follower");
        r.answer(1, 3);
        assert_eq!(r.asked(), [0, 1, 2], "the follower once a learner answered with no value");
        r.answer(2, 6);
        assert_eq!(r.out(), Some(Ok((2, 6))));
        // No learners: one member at a time, as before the race.
        let mut r = start_race_hedged(2, 0);
        assert!(r.hedges.borrow().is_empty());
        r.answer(0, 1);
        assert_eq!(r.asked(), [0, 1]);
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
            assert_eq!(spread_verdict(&answers), Spread::Shed, "{answers:?}");
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
