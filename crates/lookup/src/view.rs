//! The cached `Release` view of this isolate (`design/DESIGN.md`, Shape, "Release group", `POINTER_TTL`): one
//! `graftyard_release::view::Replica` per isolate, read from any `Release` member's `/view` (its applied entries by
//! position, with their times, and its `pending` rows), replayed through the group's own `check` against this script's
//! anchor, and never moved backward. `Freshness` says when to refresh: beside requests once the view is POINTER_TTL -
//! RETRY_MS old, and before a request only for a cold or idle isolate whose last refresh did not fail, so no request
//! waits on a group that is not answering.

use std::cell::RefCell;
use std::time::Duration;

use graftyard_release::view::{ABANDON_MS, COPY_MS, Freshness, Need, Replica, VIEW_CALL_MS};
use graftyard_release::{Anchor, Script, VersionId};
use graftyard_types::{ChainId, PlacementRecord, SiteTable, Timestamp};
use serde_json::{Value, json};
use worker::{Context, Delay, Env, js_sys};

use crate::{Bindings, Target, known_voters, now_ms, post_within, registry_record, unhex};

#[derive(Default)]
struct View {
    replica: Replica,
    fresh: Freshness,
    /// Refreshes that reached a member, and that reached none, in this isolate.
    oks: u64,
    fails: u64,
    last_error: Option<String>,
    isolate: Option<u32>,
    /// The `Release` leader's site, as the last member that answered named it (`meta.leader`).
    leader: Option<graftyard_types::SiteId>,
    /// A cold refresh's read of the sites' copies is still running.
    copying: bool,
    /// Each copy read that answered: site, ms from the cold refresh's start, the copy's entries, and how many this
    /// replica took from it.
    copies: Vec<(u8, u64, u64, u64)>,
}

thread_local! {
    static VIEW: RefCell<View> = RefCell::new(View::default());
}

/// How many pages one refresh reads at most: a cold isolate replays the whole history 100 entries at a time.
const PAGES: usize = 50;
/// After a refresh found no `Release` group at all (an install that runs none), the next one waits this long.
const NO_GROUP_MS: u64 = 60_000;

/// Test installs only: STALE_MAX lowered (`GRAFTYARD_STALE_MAX_MS`), so a hosted run sees it end a flag.
pub fn set_stale_max(ms: u64) {
    VIEW.with(|v| v.borrow_mut().replica.stale_max = ms);
}

/// Brings the view up to `Freshness::need` before a request is served (`design/DESIGN.md`, Shape, `POINTER_TTL`).
/// When the view is due for a wait (a cold isolate, or one idle past POINTER_TTL), the refresh runs beside the request
/// and also reads every site's copy of the group's entries (`copies`). The request waits until the group answers, or
/// until every copy has answered and one moved the view, and at most VIEW_CALL_MS from arrival (COPY_MS for an isolate
/// that holds no view); then it goes out with what is held, unpinned if nothing came, as before the copy existed. The
/// refresh goes on beside later requests, so the group's view replaces a copy. Otherwise a refresh starts beside the
/// request when one is due.
pub async fn ensure(b: &Bindings<'_>, anchor: &Anchor, ctx: Option<&Context>) {
    let arrived = now_ms();
    let bound = if with(|r| r.count()) == 0 { COPY_MS } else { VIEW_CALL_MS };
    match VIEW.with(|v| v.borrow().fresh.need(arrived)) {
        Need::Use => return,
        Need::Refresh => return refresh_beside(b.env, b.sites, anchor, ctx, false),
        // Another request of this isolate is waiting for a refresh: wait for the same one.
        Need::Wait if VIEW.with(|v| v.borrow().fresh.running.is_some()) => {}
        Need::Wait => {
            VIEW.with(|v| v.borrow_mut().fresh.start(arrived, true));
            beside(b.env, b.sites, anchor, ctx, arrived, true);
        }
    }
    while now_ms() < arrived + bound {
        // The waited refresh is still running, or it failed while the copies are still read.
        let waiting = VIEW.with(|v| {
            let v = v.borrow();
            matches!(v.fresh.running, Some((_, true))) || (v.copying && v.fresh.failing)
        });
        if !waiting {
            return;
        }
        Delay::from(Duration::from_millis(10)).await;
    }
    VIEW.with(|v| v.borrow_mut().fresh.detach());
}

/// A refresh now, whatever `Freshness` says, for at most VIEW_CALL_MS: the release endpoint's second look before it
/// refuses an entry whose signer its view does not hold, which a `Keys` committed since the last refresh may add.
pub async fn refresh_now(b: &Bindings<'_>, anchor: &Anchor) {
    let now = now_ms();
    VIEW.with(|v| v.borrow_mut().fresh.start(now, true));
    if crate::within(Duration::from_millis(VIEW_CALL_MS), refresh(b, anchor, now)).await.is_none() {
        failed(format!("no member answered within {VIEW_CALL_MS} ms"));
    }
}

/// Starts a refresh beside the request when one is due, and never waits: under `ctx.wait_until` in a Worker, whose
/// tasks left running after the response may be cancelled, or as a local task in a Durable Object. `cold` also starts
/// one where `ensure` would wait (a member, whose requests never wait on the view).
pub fn refresh_beside(env: &Env, sites: &SiteTable, anchor: &Anchor, ctx: Option<&Context>, cold: bool) {
    let now = now_ms();
    match VIEW.with(|v| v.borrow().fresh.need(now)) {
        Need::Refresh => {}
        Need::Wait if cold && VIEW.with(|v| v.borrow().fresh.running.is_none()) => {}
        _ => return,
    }
    VIEW.with(|v| v.borrow_mut().fresh.start(now, false));
    beside(env, sites, anchor, ctx, now, false);
}

/// Runs the refresh that started at `now` as a task, for at most ABANDON_MS. A refresh requests wait for (`waited`),
/// or one on an isolate that holds no view, also reads the sites' copies.
fn beside(env: &Env, sites: &SiteTable, anchor: &Anchor, ctx: Option<&Context>, now: u64, waited: bool) {
    let (env, sites, anchor) = (env.clone(), sites.clone(), *anchor);
    let copy = waited || with(|r| r.count()) == 0;
    if copy {
        VIEW.with(|v| v.borrow_mut().copying = true);
    }
    let task = async move {
        let b = Bindings { env: &env, sites: &sites };
        let group = async {
            if crate::within(Duration::from_millis(ABANDON_MS), refresh(&b, &anchor, now)).await.is_none() {
                failed(format!("no member answered within {ABANDON_MS} ms"));
            }
        };
        if copy {
            futures_util::future::join(group, copies(&b, &anchor, now)).await;
        } else {
            group.await;
        }
    };
    match ctx {
        Some(c) => c.wait_until(task),
        None => worker::wasm_bindgen_futures::spawn_local(task),
    }
}

fn failed(e: String) {
    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        v.fresh.failed(now_ms());
        v.fails += 1;
        v.last_error = Some(e);
    });
}

/// Whether a refresh may be due now: `refresh_beside` would start one, or `ensure` would wait.
pub fn due() -> bool {
    VIEW.with(|v| v.borrow().fresh.need(now_ms())) != Need::Use
}

/// Reads `f` of the replica this isolate holds.
pub fn with<R>(f: impl FnOnce(&Replica) -> R) -> R {
    VIEW.with(|v| f(&v.borrow().replica))
}

/// The pinned version of `script`, or `None` to follow the deployment split.
pub fn pin(script: Script) -> Option<VersionId> {
    with(|r| r.pin(script))
}

/// Until when `chain` is in stale mode now, by the view held (`Replica::stale_until`).
pub fn stale_until(chain: ChainId, owner: Option<graftyard_types::AccountId>) -> Option<Timestamp> {
    with(|r| r.stale_until(chain, owner, Timestamp(now_ms())))
}

/// The `Release` group's record from the view's genesis, if the view holds one.
pub fn release_record() -> Option<PlacementRecord> {
    with(|r| r.fold.genesis.as_ref().and_then(|g| g.placement(graftyard_types::SystemChain::Release).cloned()))
}

/// One line of what this isolate holds: entries, whether the last refresh failed, the view's age in ms, and this
/// isolate's id (drawn on first use), for a driver's per-request evidence.
pub fn brief() -> String {
    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        let id = *v.isolate.get_or_insert_with(|| (js_sys::Math::random() * 4_294_967_296.0) as u32);
        let age = v.fresh.fetched.map(|t| now_ms().saturating_sub(t).to_string()).unwrap_or_else(|| "none".into());
        // The copy this isolate took, as `<site>@<ms>`, or `none`.
        let copy = v.copies.iter().find(|c| c.3 > 0).map_or("none".into(), |c| format!("{}@{}", c.0, c.1));
        format!("count={};failing={};age={age};isolate={id:08x};copy={copy}", v.replica.count(), v.fresh.failing)
    })
}

/// What this isolate holds, for test drivers and `graft-front`'s status.
pub fn status() -> Value {
    VIEW.with(|v| {
        let v = v.borrow();
        let f = &v.replica.fold;
        json!({
            "count": v.replica.count(),
            "last": f.last.map(|d| d.hex()),
            "fetchedAgoMs": v.fresh.fetched.map(|t| now_ms().saturating_sub(t)),
            "failing": v.fresh.failing,
            "oks": v.oks,
            "fails": v.fails,
            "lastError": v.last_error,
            "copies": v.copies.iter().map(|c| json!({ "site": c.0, "ms": c.1, "entries": c.2, "took": c.3 }))
                .collect::<Vec<_>>(),
            "pointers": {
                "edge": f.pointers.edge.map(|x| x.to_string()),
                "ops": f.pointers.ops.map(|x| x.to_string()),
            },
            "stale": f.stale.iter().map(|(s, u)| json!([format!("{s:?}"), u.0])).collect::<Vec<_>>(),
            "pending": v.replica.pending.values().map(|p| json!({
                "scope": format!("{:?}", p.scope), "until": p.until.0, "arrived": p.arrived.0,
                "honoredUntil": p.honored_until(v.replica.stale_max).0,
            })).collect::<Vec<_>>(),
        })
    })
}

/// Reads every site's copy of the group's entries at once, for COPY_MS at most, and offers each to the replica as it
/// answers (`Replica::adopt`: all of a copy's entries checked against this isolate's anchor or none of them, and only
/// a copy longer than what is held). Once every copy has answered or COPY_MS has passed, an isolate that holds a view
/// releases the requests waiting on its refresh: they go out with the longest copy, never the first, because a site
/// whose members are followers of an idle group can hold one entry fewer (BF-8 run 3, `tests/m5/results/m5-wp2.md`,
/// "BF-8 run 3: two holes, fixed before run 4").
async fn copies(b: &Bindings<'_>, anchor: &Anchor, start: u64) {
    let reads = b.sites.0.iter().map(|s| async move {
        let name = graftyard_types::MemberRegistry::for_chain(s.id, ChainId::Release).do_name();
        let stub = b.registry(s.id, &name).ok()?;
        let mut res = stub.fetch_with_str("https://registry/release_copy").await.ok()?;
        let v = res.json::<Value>().await.ok()?;
        let copy: Vec<(Vec<u8>, Timestamp)> = v["entries"]
            .as_array()?
            .iter()
            .filter_map(|e| Some((unhex(e[0].as_str()?)?, Timestamp(e[1].as_u64()?))))
            .collect();
        let ms = now_ms().saturating_sub(start);
        VIEW.with(|w| {
            let mut w = w.borrow_mut();
            let took = match w.replica.adopt(anchor, &copy) {
                Ok(n) => n,
                Err(e) => {
                    w.last_error = Some(format!("site {} copy refused: {}: {e}", s.id.0, e.variant()));
                    0
                }
            };
            w.copies.push((s.id.0, ms, copy.len() as u64, took));
        });
        Some(())
    });
    let _ = crate::within(Duration::from_millis(COPY_MS), futures_util::future::join_all(reads)).await;
    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        v.copying = false;
        if v.replica.count() > 0 {
            v.fresh.detach();
        }
    });
}

/// One refresh that started at `start`: the members to ask, one at a time with VIEW_CALL_MS each, until one answers.
async fn refresh(b: &Bindings<'_>, anchor: &Anchor, start: u64) {
    match read(b, anchor).await {
        Ok(()) => VIEW.with(|v| {
            let mut v = v.borrow_mut();
            v.fresh.ok(start);
            v.oks += 1;
        }),
        Err((e, no_group)) => {
            failed(e);
            if no_group {
                VIEW.with(|v| v.borrow_mut().fresh.next = now_ms() + NO_GROUP_MS);
            }
        }
    }
}

/// The members a refresh asks, in order: the `Release` voters this isolate knows, else the genesis record's bootstrap
/// members. The leader the last answer named goes first, because a follower applies an entry only once a later round
/// carries its commit, so it can be one round behind; with no leader known, a random member goes first, so isolates
/// spread their refreshes over the group.
async fn members(b: &Bindings<'_>) -> Result<Vec<Target>, (String, bool)> {
    let record = match release_record() {
        Some(r) => r,
        None => registry_record(b, ChainId::Release).await.ok_or(("no Release group is registered".into(), true))?,
    };
    let mut out = known_voters(ChainId::Release);
    if out.is_empty() {
        out = record
            .bootstrap_members(ChainId::Release)
            .into_iter()
            .map(|m| Target { site: m.site, do_name: m.do_name.to_string() })
            .collect();
    }
    if !out.is_empty() {
        let leader = VIEW.with(|v| v.borrow().leader);
        let k = match leader.and_then(|l| out.iter().position(|t| t.site == l)) {
            Some(i) => i,
            None => (js_sys::Math::random() * out.len() as f64) as usize % out.len(),
        };
        out.rotate_left(k);
    }
    Ok(out)
}

/// Reads members in `members` order and stops after the first that names itself leader. A follower applies an entry
/// only once a later round carries its commit, and an idle group sends none, so a follower's applied state can lag a
/// commit until the group's next entry (BF-43's first run: 59 s, `tests/m5/results/m5-wp2.md`). A follower's page is
/// applied too, so a refresh that reaches no leader still moves the view as far as any member that answered.
async fn read(b: &Bindings<'_>, anchor: &Anchor) -> Result<(), (String, bool)> {
    let mut last = String::from("no member answered");
    let mut any = false;
    for t in members(b).await? {
        let mut answered = false;
        let mut leads = false;
        for _ in 0..PAGES {
            let from = with(|r| r.count());
            let body = json!({ "from": from }).to_string();
            let deadline = Duration::from_millis(VIEW_CALL_MS);
            let Some((200, v)) = post_within(b, t.site, &t.do_name, "/view", body.as_bytes(), deadline).await else {
                last = format!("{}: no answer", t.do_name);
                break;
            };
            let Some(count) = v.get("count").and_then(Value::as_u64) else {
                last = format!("{}: {}", t.do_name, v.to_string().chars().take(200).collect::<String>());
                break;
            };
            answered = true;
            // A follower answers no leader: it keeps the hint, unless the member hinted at no longer leads.
            let leader = v
                .pointer("/meta/leader")
                .and_then(Value::as_u64)
                .and_then(|l| u8::try_from(l).ok())
                .map(graftyard_types::SiteId);
            leads = leader == Some(t.site);
            VIEW.with(|w| {
                let mut w = w.borrow_mut();
                if leader.is_some() || w.leader == Some(t.site) {
                    w.leader = leader;
                }
            });
            let page: Vec<(Vec<u8>, Timestamp)> = v["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| Some((unhex(e[0].as_str()?)?, Timestamp(e[1].as_u64()?))))
                .collect();
            let now = Timestamp(now_ms());
            let applied = VIEW.with(|w| {
                let mut w = w.borrow_mut();
                let n = w.replica.extend(anchor, from, &page);
                for p in v["pending"].as_array().into_iter().flatten() {
                    if let (Some(bytes), Some(at)) = (p[0].as_str().and_then(unhex), p[1].as_u64()) {
                        let _ = w.replica.offer_pending(&bytes, Timestamp(at), now);
                    }
                }
                w.replica.prune(now);
                n
            });
            match applied {
                Err(e) => {
                    last = format!("{}: refused its page at {from}: {}: {e}", t.do_name, e.variant());
                    answered = false;
                    break;
                }
                Ok(0) => break,
                Ok(_) if with(|r| r.count()) >= count => break,
                Ok(_) => {}
            }
        }
        any |= answered;
        if leads {
            return Ok(());
        }
    }
    if any { Ok(()) } else { Err((last, false)) }
}
