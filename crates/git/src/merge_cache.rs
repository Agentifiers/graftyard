//! What the git cache answers for a real merge or a merge-queue group (`GitCacheShard::merge`; the container's `POST
//! /merge` with `method`), and how that answer becomes a `MergePlan` for `Merge` or a `CacheAnswer` for the
//! `MergeQueue` consumer's `step`. Pure: the hosts call the container and hand the answer here.
//!
//! The container merges each head in turn on the previous result, deterministically, and runs the tree-diff check on
//! each. The packs are inline (`PackRef::Inline`) and the member trusts them as it trusts a push's
//! (`DurablePack::trusted`). A group or plan whose objects pack larger than `MERGE_PACK_MAX` is refused here
//! [inference: a merge's own objects are a few KB; a larger one would need `PackStore::place`, which the edge's merge
//! path does not run yet].

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::bucket::StoreError;
use crate::issues::{IssueNumber, MergeMethod};
use crate::merge::{EngineVersion, MergePlan, MergeRequest, TreeDiffDigest};
use crate::pack::{DurablePack, PackRef};
use crate::queue::MergeQueueEntry;
use crate::queue_consumer::{CacheAnswer, CacheAsk};
use crate::refs::ObjectId;

/// Inline pack bytes one merge command may carry, under `COMMAND_MAX` with room for the rest of the command.
pub const MERGE_PACK_MAX: usize = crate::prsync::MERGE_BYTES_PER_COMMAND;

/// The container's name for a method.
pub fn method_name(m: MergeMethod) -> &'static str {
    match m {
        MergeMethod::Merge => "merge",
        MergeMethod::Squash => "squash",
        MergeMethod::Rebase => "rebase",
    }
}

/// One head merged onto the previous result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainStep {
    pub n: String,
    pub head: String,
    pub commit: String,
    #[serde(with = "crate::pack::b64_opt", default)]
    pub pack: Option<Vec<u8>>,
    pub up_to_date: bool,
    pub verified: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainConflict {
    pub index: usize,
    pub paths: Vec<String>,
}

/// The container's answer: every step that merged, the first conflict, one pack of every step's objects, and git's
/// version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainAnswer {
    pub steps: Vec<ChainStep>,
    pub conflict: Option<ChainConflict>,
    #[serde(with = "crate::pack::b64_opt", default)]
    pub pack: Option<Vec<u8>>,
    pub engine: String,
}

/// Why an answer cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheError {
    /// The direct merge conflicts: GitHub's 405 "Pull Request is not mergeable".
    Conflict(Vec<String>),
    /// The answer does not hold what was asked, or a pack is over `MERGE_PACK_MAX`.
    Bad(String),
}

fn oid(s: &str) -> Result<ObjectId, CacheError> {
    ObjectId::parse_hex(s.as_bytes()).ok_or_else(|| CacheError::Bad(format!("not an object id: {s}")))
}

fn inline(b: &[u8]) -> Result<DurablePack, CacheError> {
    if b.len() > MERGE_PACK_MAX {
        return Err(CacheError::Bad(format!("merge pack of {} bytes is over {MERGE_PACK_MAX}", b.len())));
    }
    Ok(DurablePack::trusted(PackRef::Inline(b.to_vec())))
}

impl ChainAnswer {
    /// Step `i` as a `MergePlan` from `base_old`.
    fn plan(&self, i: usize, base_old: ObjectId, method: MergeMethod) -> Result<MergePlan, CacheError> {
        let s = &self.steps[i];
        let digest = graftyard_types::Digest::parse_hex(&s.verified)
            .ok_or_else(|| CacheError::Bad(format!("verified {}", s.verified)))?;
        Ok(MergePlan {
            base_old,
            head: oid(&s.head)?,
            commit: oid(&s.commit)?,
            pack: inline(s.pack.as_deref().unwrap_or_default())?,
            method,
            up_to_date: s.up_to_date,
            verified: TreeDiffDigest(digest),
            engine: EngineVersion(self.engine.clone().into()),
        })
    }

    /// The direct merge's plan: the one step, of the head asked for.
    pub fn merge_plan(&self, req: &MergeRequest) -> Result<MergePlan, CacheError> {
        if let Some(c) = &self.conflict {
            return Err(CacheError::Conflict(c.paths.clone()));
        }
        let [s] = self.steps.as_slice() else { return Err(CacheError::Bad("one step".into())) };
        if oid(&s.head)? != req.head {
            return Err(CacheError::Bad("the answer is of another head".into()));
        }
        self.plan(0, req.base_old, req.method)
    }

    /// The consumer's `CacheAnswer` for `ask`. A `Build` keeps the entries before the first conflict and names the
    /// conflicting pull request. A `Plan` re-plans built entries; a step whose commit is not its entry's `group_tip`
    /// is planned anyway, and `QueueMerge::decide` refuses it `StaleChain`.
    pub fn answer(&self, ask: &CacheAsk) -> Result<CacheAnswer, CacheError> {
        let heads: Vec<(IssueNumber, ObjectId)> = match ask {
            CacheAsk::Build { prs, .. } => prs.clone(),
            CacheAsk::Plan { entries, .. } => entries.iter().map(|(e, h)| (e.pr, *h)).collect(),
        };
        if self.steps.len() > heads.len() {
            return Err(CacheError::Bad("more steps than heads".into()));
        }
        for (s, (_, h)) in self.steps.iter().zip(&heads) {
            if oid(&s.head)? != *h {
                return Err(CacheError::Bad("a step of another head".into()));
            }
        }
        match ask {
            CacheAsk::Build { on, .. } => {
                let mut tip = *on;
                let mut entries: SmallVec<[MergeQueueEntry; 8]> = SmallVec::new();
                for (s, (n, _)) in self.steps.iter().zip(&heads) {
                    let e = MergeQueueEntry { pr: *n, parent_tip: tip, group_tip: oid(&s.commit)? };
                    tip = e.group_tip;
                    entries.push(e);
                }
                let pack = match (&self.pack, entries.is_empty()) {
                    (Some(p), false) => Some(inline(p)?),
                    _ => None,
                };
                let conflict = self.conflict.as_ref().and_then(|c| heads.get(c.index)).map(|(n, _)| *n);
                if self.conflict.is_none() && entries.len() != heads.len() {
                    return Err(CacheError::Bad("a step is missing".into()));
                }
                Ok(CacheAnswer::Built { ask: ask.clone(), entries, pack, conflict })
            }
            CacheAsk::Plan { entries, method, .. } => {
                if self.conflict.is_some() || self.steps.len() != entries.len() {
                    return Err(CacheError::Bad("a built entry no longer merges".into()));
                }
                let plans = entries
                    .iter()
                    .enumerate()
                    .map(|(i, (e, _))| Ok((e.pr, self.plan(i, e.parent_tip, *method)?)))
                    .collect::<Result<_, CacheError>>()?;
                Ok(CacheAnswer::Planned { ask: ask.clone(), plans })
            }
        }
    }
}

/// What the container is asked for an ask: the base to start on, and each (n, head) in order.
pub fn chain_of(ask: &CacheAsk) -> (ObjectId, Vec<(IssueNumber, ObjectId)>, MergeMethod) {
    match ask {
        CacheAsk::Build { on, prs, method, .. } => (*on, prs.clone(), *method),
        CacheAsk::Plan { entries, method, .. } => (
            entries.first().map_or(ObjectId([0; 20]), |(e, _)| e.parent_tip),
            entries.iter().map(|(e, h)| (e.pr, *h)).collect(),
            *method,
        ),
    }
}

/// `GitCacheShard::merge`'s order: wait for a catch-up already in flight on this shard, merge, and catch the shard up
/// behind its `IngestCursor` only if the container answers that a commit is not there (`merge` answers `None`), then
/// merge again. A merge names its commits by id, so a shard that holds them answers the same before or after a
/// catch-up; `PrSync`'s test merge has usually caught it up already. Catching up first cost two to three leader reads,
/// 300 to 600 ms of every merge once leadership moved (`tests/m2/results/m2-wp6.md`, "Merge latency"). `up` starts
/// the container first (`ShardHost::up`): the catch-up used to, so without it a merge on a stopped container failed
/// "has not been started" and no catch-up followed, which stalled the `MergeQueue` consumer until another route
/// started the container (same file, BF-21). A merge that finds a commit missing waits for the catch-up in flight
/// (`in_flight`), such as the one a new leadership starts on its own site's shard (`workers/site/src/prsync.rs`,
/// `warm`), and merges again before it catches up itself: racing it cost a second catch-up, 250 to 700 ms on 38 of
/// 105 weur samples (same file, bm2-latency). It waits only after a miss: under journey 9's pushes some validate's
/// catch-up is nearly always running, and a merge that waited first waited 64 to 2,151 ms for commits it already had
/// (LM-1 round 7, 500 agents).
pub async fn merge_then_catch_up<U, Fu, W, Fw, M, Fm, C, Fc>(
    up: U,
    in_flight: W,
    mut merge: M,
    catch_up: C,
) -> Result<ChainAnswer, StoreError>
where
    U: FnOnce() -> Fu,
    Fu: std::future::Future<Output = Result<(), StoreError>>,
    W: FnOnce() -> Fw,
    Fw: std::future::Future<Output = ()>,
    M: FnMut() -> Fm,
    Fm: std::future::Future<Output = Result<Option<ChainAnswer>, StoreError>>,
    C: FnOnce() -> Fc,
    Fc: std::future::Future<Output = Result<(), StoreError>>,
{
    up().await?;
    if let Some(a) = merge().await? {
        return Ok(a);
    }
    in_flight().await;
    if let Some(a) = merge().await? {
        return Ok(a);
    }
    catch_up().await?;
    merge().await?.ok_or_else(|| StoreError("a commit of the merge is not in the git cache after catching up".into()))
}

/// The catch-ups running on one shard object for one network, so a merge can wait for them (`merge_then_catch_up`'s
/// `in_flight`). Single-threaded, as a Durable Object is: `start` marks one running until its `Running` drops, and
/// `idle` ends once none runs.
#[derive(Clone, Default)]
pub struct CatchUps(std::rc::Rc<std::cell::RefCell<(u32, Vec<std::task::Waker>)>>, std::rc::Rc<Turns>);

/// `CatchUps::fresh`'s count: catch-ups started on this gate, and the start number of the latest one that succeeded.
#[derive(Default)]
struct Turns {
    started: std::cell::Cell<u64>,
    ok: std::cell::Cell<u64>,
}

/// One running catch-up; dropping it ends it, whatever its outcome.
pub struct Running(CatchUps);

impl CatchUps {
    /// Brings the shard up to date for a request that arrived now, sharing catch-ups: it returns once a catch-up
    /// that *started* after the request arrived has succeeded, so the shard holds every commit the client could have
    /// seen. Requests that arrive while one runs wait for the next one, and all of them share it, so a burst of n
    /// validates costs two catch-ups, not n (hosted, each `heads` read is a leader round trip). `run` is called only
    /// when this request starts the catch-up; its error is returned to that request alone, and the others try again.
    pub async fn fresh<F, Fut, E>(&self, mut run: F) -> Result<(), E>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<(), E>>,
    {
        let arrived = self.1.started.get();
        let _running = self.turn().await;
        if self.1.ok.get() > arrived {
            return Ok(());
        }
        let n = self.1.started.get() + 1;
        self.1.started.set(n);
        run().await?;
        self.1.ok.set(n);
        Ok(())
    }

    pub fn start(&self) -> Running {
        self.0.borrow_mut().0 += 1;
        Running(self.clone())
    }

    /// Waits until no catch-up runs, then starts one, in the same poll, so catch-ups on one network run one at a time
    /// and each goes on from the cursor the one before saved. Two at once on `repo:65536` each rehydrated its 571,872
    /// refs from nothing, and the later one never finished (`tests/m3/results/wire-fix.md`).
    pub fn turn(&self) -> impl std::future::Future<Output = Running> + use<> {
        let me = self.clone();
        std::future::poll_fn(move |cx| {
            let mut s = me.0.borrow_mut();
            if s.0 == 0 {
                s.0 = 1;
                return std::task::Poll::Ready(Running(me.clone()));
            }
            s.1.push(cx.waker().clone());
            std::task::Poll::Pending
        })
    }

    /// Ends once no catch-up runs; at once if none does.
    pub fn idle(&self) -> impl std::future::Future<Output = ()> + use<> {
        let me = self.clone();
        std::future::poll_fn(move |cx| {
            let mut s = me.0.borrow_mut();
            if s.0 == 0 {
                return std::task::Poll::Ready(());
            }
            s.1.push(cx.waker().clone());
            std::task::Poll::Pending
        })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let wakers = {
            let mut s = self.0.0.borrow_mut();
            s.0 -= 1;
            if s.0 == 0 { std::mem::take(&mut s.1) } else { Vec::new() }
        };
        for w in wakers {
            w.wake();
        }
    }
}

/// Whether the container's answer to a merge says the shard must catch up first: 409, a commit is not in the
/// repository, or 404, the container holds no repository for the network (a new container, after its inactivity
/// timeout or a new image). Catching up rehydrates the repository in that case.
pub fn needs_catch_up(status: u16) -> bool {
    matches!(status, 404 | 409)
}

/// The container's query for a chain: `base=..&heads=..&ns=..&method=..`.
pub fn chain_query(base: ObjectId, heads: &[(IssueNumber, ObjectId)], method: MergeMethod) -> String {
    let hs: Vec<String> = heads.iter().map(|(_, h)| h.hex()).collect();
    let ns: Vec<String> = heads.iter().map(|(n, _)| n.0.to_string()).collect();
    format!("base={}&heads={}&ns={}&method={}", base.hex(), hs.join(","), ns.join(","), method_name(method))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refs::RefName;

    fn h(b: u8) -> String {
        ObjectId([b; 20]).hex()
    }

    fn step(head: u8, commit: u8) -> ChainStep {
        ChainStep {
            n: "1".into(),
            head: h(head),
            commit: h(commit),
            pack: Some(vec![commit]),
            up_to_date: false,
            verified: "ab".repeat(32),
        }
    }

    fn main() -> RefName {
        RefName::parse(b"refs/heads/main").unwrap()
    }

    #[test]
    fn a_build_answer_chains_from_on_and_keeps_the_entries_before_a_conflict() {
        let ask = CacheAsk::Build {
            base: main(),
            on: ObjectId([1; 20]),
            prs: vec![
                (IssueNumber(4), ObjectId([4; 20])),
                (IssueNumber(5), ObjectId([5; 20])),
                (IssueNumber(6), ObjectId([6; 20])),
            ],
            method: MergeMethod::Merge,
        };
        let a = ChainAnswer {
            steps: vec![step(4, 40)],
            conflict: Some(ChainConflict { index: 1, paths: vec!["f".into()] }),
            pack: Some(vec![1]),
            engine: "git version 2.56.0".into(),
        };
        let CacheAnswer::Built { entries, pack, conflict, .. } = a.answer(&ask).unwrap() else { panic!() };
        assert_eq!(
            entries.as_slice(),
            &[MergeQueueEntry { pr: IssueNumber(4), parent_tip: ObjectId([1; 20]), group_tip: ObjectId([40; 20]) }]
        );
        assert!(pack.is_some());
        assert_eq!(conflict, Some(IssueNumber(5)));
        // A step of a head that was not asked for is refused.
        let bad = ChainAnswer { steps: vec![step(9, 40)], ..a };
        assert!(bad.answer(&ask).is_err());
    }

    #[test]
    fn a_plan_answer_plans_each_entry_from_its_parent_tip() {
        let e1 = MergeQueueEntry { pr: IssueNumber(4), parent_tip: ObjectId([1; 20]), group_tip: ObjectId([40; 20]) };
        let e2 = MergeQueueEntry { pr: IssueNumber(5), parent_tip: ObjectId([40; 20]), group_tip: ObjectId([50; 20]) };
        let ask = CacheAsk::Plan {
            base: main(),
            entries: vec![(e1, ObjectId([4; 20])), (e2, ObjectId([5; 20]))],
            method: MergeMethod::Squash,
        };
        assert_eq!(chain_of(&ask).0, ObjectId([1; 20]));
        let a = ChainAnswer { steps: vec![step(4, 40), step(5, 50)], conflict: None, pack: None, engine: "g".into() };
        let CacheAnswer::Planned { plans, .. } = a.answer(&ask).unwrap() else { panic!() };
        assert_eq!(plans[1].1.base_old, ObjectId([40; 20]));
        assert_eq!(plans[1].1.commit, ObjectId([50; 20]));
        assert_eq!(plans[1].1.method, MergeMethod::Squash);
    }

    fn answer() -> ChainAnswer {
        ChainAnswer { steps: vec![step(4, 5)], conflict: None, pack: None, engine: "git".into() }
    }

    #[test]
    fn a_merge_on_a_container_with_no_repository_catches_up_first() {
        assert!(needs_catch_up(404), "a new container holds no repository for the network");
        assert!(needs_catch_up(409), "a commit is not there yet");
        assert!(!needs_catch_up(200) && !needs_catch_up(400) && !needs_catch_up(500));
    }

    /// A shard that holds both commits merges with no catch-up; one that lacks one catches up once, then merges. The
    /// container is stopped at first (its inactivity timeout, or a new image), and a merge on it fails.
    #[test]
    fn a_merge_catches_up_only_when_a_commit_is_missing() {
        let run = |held: bool| {
            let (merges, catchups) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
            let (caught, running) = (std::cell::Cell::new(held), std::cell::Cell::new(false));
            let r = futures_executor::block_on(merge_then_catch_up(
                || {
                    running.set(true);
                    async { Ok(()) }
                },
                || async {},
                || {
                    merges.set(merges.get() + 1);
                    let a = if running.get() {
                        Ok(caught.get().then(answer))
                    } else {
                        Err(StoreError("not started".into()))
                    };
                    async move { a }
                },
                || {
                    catchups.set(catchups.get() + 1);
                    running.set(true);
                    caught.set(true);
                    async { Ok(()) }
                },
            ));
            (r, merges.get(), catchups.get())
        };
        assert_eq!(run(true), (Ok(answer()), 1, 0), "both commits held: no catch-up");
        assert_eq!(run(false), (Ok(answer()), 2, 1), "a commit missing: one catch-up, then the merge again");
        let still_missing = futures_executor::block_on(merge_then_catch_up(
            || async { Ok(()) },
            || async {},
            || async { Ok(None) },
            || async { Ok(()) },
        ));
        assert!(still_missing.is_err());
    }

    /// A merge that arrives while the new leadership's catch-up runs on the shard and misses a commit waits for it,
    /// then merges with no catch-up of its own: the running catch-up brings the commits. Racing it, the merge catches
    /// up a second time. A merge whose commits are already here answers without waiting for the catch-up at all.
    #[test]
    fn a_merge_waits_for_the_catch_up_in_flight_instead_of_racing_it() {
        use futures_util::task::LocalSpawnExt;
        let run = |wait: bool| {
            let mut pool = futures_executor::LocalPool::new();
            let sp = pool.spawner();
            let gate = CatchUps::default();
            let caught = std::rc::Rc::new(std::cell::Cell::new(false));
            let (merges, catchups) =
                (std::rc::Rc::new(std::cell::Cell::new(0)), std::rc::Rc::new(std::cell::Cell::new(0)));
            // The new leadership's catch-up: running before the merge arrives, done a few polls later.
            let running = gate.start();
            let c = caught.clone();
            sp.spawn_local(async move {
                for _ in 0..3 {
                    futures_util::future::ready(()).await;
                    yield_once().await;
                }
                c.set(true);
                drop(running);
            })
            .unwrap();
            let out = std::rc::Rc::new(std::cell::RefCell::new(None));
            let (o, g, m, k, c2) = (out.clone(), gate.clone(), merges.clone(), catchups.clone(), caught.clone());
            sp.spawn_local(async move {
                let c3 = c2.clone();
                let r = merge_then_catch_up(
                    || async { Ok(()) },
                    || {
                        let idle = g.idle();
                        async move {
                            if wait {
                                idle.await
                            }
                        }
                    },
                    || {
                        m.set(m.get() + 1);
                        let a = Ok(c2.get().then(answer));
                        async move { a }
                    },
                    || {
                        k.set(k.get() + 1);
                        c3.set(true);
                        async { Ok(()) }
                    },
                )
                .await;
                *o.borrow_mut() = Some(r);
            })
            .unwrap();
            pool.run();
            let r = out.borrow_mut().take();
            (r, merges.get(), catchups.get())
        };
        assert_eq!(run(true), (Some(Ok(answer())), 2, 0), "misses, waits, then merges with no catch-up of its own");
        assert_eq!(run(false), (Some(Ok(answer())), 3, 1), "racing it: a second catch-up");
        // The commits are already here while some other catch-up runs (a validate's, under journey 9's pushes): the
        // merge answers at once, before that catch-up ends, and never waits for it (LM-1 round 7).
        use futures_util::FutureExt;
        let gate = CatchUps::default();
        let running = gate.start();
        let r = merge_then_catch_up(
            || async { Ok(()) },
            || gate.idle(),
            || async { Ok(Some(answer())) },
            || async { Ok(()) },
        )
        .now_or_never();
        assert_eq!(r, Some(Ok(answer())), "merged in one poll while the catch-up still runs");
        drop(running);
        // No catch-up running: the merge does not wait.
        let idle = CatchUps::default().idle();
        assert_eq!(futures_executor::block_on(idle), ());
    }

    /// Three catch-ups on one network, the first running when the other two arrive: they run one at a time, each after
    /// the one before ends, never two at once (`repo:65536`'s two concurrent rehydrates).
    #[test]
    fn catch_ups_on_one_network_take_turns() {
        use futures_util::task::LocalSpawnExt;
        let mut pool = futures_executor::LocalPool::new();
        let sp = pool.spawner();
        let gate = CatchUps::default();
        let (now, most) = (std::rc::Rc::new(std::cell::Cell::new(0)), std::rc::Rc::new(std::cell::Cell::new(0)));
        let order = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        for i in 0..3 {
            let (g, now, most, order) = (gate.clone(), now.clone(), most.clone(), order.clone());
            sp.spawn_local(async move {
                let _running = g.turn().await;
                now.set(now.get() + 1);
                most.set(most.get().max(now.get()));
                order.borrow_mut().push(i);
                for _ in 0..3 {
                    yield_once().await;
                }
                now.set(now.get() - 1);
            })
            .unwrap();
        }
        pool.run();
        assert_eq!(most.get(), 1, "two catch-ups ran at once");
        assert_eq!(*order.borrow(), vec![0, 1, 2]);
        assert_eq!(futures_executor::block_on(gate.idle()), ());
    }

    /// A burst of validates on one network shares catch-ups: the first starts one, the nine that arrive while it runs
    /// all wait for and share the next, so two catch-ups run, not ten, and never two at once. Each request returns only
    /// after a catch-up that started after it arrived succeeded. A failed catch-up fails only the request that ran it.
    #[test]
    fn fresh_shares_one_catch_up_among_the_requests_that_wait_for_it() {
        use futures_util::task::LocalSpawnExt;
        let mut pool = futures_executor::LocalPool::new();
        let sp = pool.spawner();
        let gate = CatchUps::default();
        let runs = std::rc::Rc::new(std::cell::RefCell::new(Vec::<u64>::new()));
        let (now, most) = (std::rc::Rc::new(std::cell::Cell::new(0)), std::rc::Rc::new(std::cell::Cell::new(0)));
        let clock = std::rc::Rc::new(std::cell::Cell::new(0u64));
        let done = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let fail_first = std::rc::Rc::new(std::cell::Cell::new(true));
        for i in 0..10u64 {
            let (g, runs, now, most, clock, done, fail) = (
                gate.clone(),
                runs.clone(),
                now.clone(),
                most.clone(),
                clock.clone(),
                done.clone(),
                fail_first.clone(),
            );
            sp.spawn_local(async move {
                // Request i arrives at tick i (one tick per spawn, before any catch-up ends).
                let arrived = clock.get();
                clock.set(clock.get() + 1);
                let r = g
                    .fresh(|| {
                        let (runs, now, most, clock, fail) =
                            (runs.clone(), now.clone(), most.clone(), clock.clone(), fail.clone());
                        async move {
                            let started = clock.get();
                            now.set(now.get() + 1);
                            most.set(most.get().max(now.get()));
                            for _ in 0..3 {
                                yield_once().await;
                            }
                            now.set(now.get() - 1);
                            runs.borrow_mut().push(started);
                            if fail.replace(false) && i == 0 {
                                return Err(started);
                            }
                            Ok(())
                        }
                    })
                    .await;
                done.borrow_mut().push((i, arrived, r));
            })
            .unwrap();
        }
        pool.run();
        assert_eq!(most.get(), 1, "two catch-ups ran at once");
        // The first fails (its request alone sees the error); the second serves the other nine.
        assert_eq!(runs.borrow().len(), 2, "one catch-up per turn, shared: {:?}", runs.borrow());
        let started_second = runs.borrow()[1];
        for (i, arrived, r) in done.borrow().iter() {
            if *i == 0 {
                assert!(r.is_err(), "the request whose catch-up failed sees the failure");
            } else {
                assert!(r.is_ok(), "request {i}");
                assert!(started_second >= *arrived, "request {i} served by a catch-up older than it");
            }
        }
        // A request after the burst starts a catch-up of its own: nothing it could have seen is assumed held.
        let before = runs.borrow().len();
        let r2 = runs.clone();
        futures_executor::block_on(gate.fresh(|| {
            r2.borrow_mut().push(99);
            async { Ok::<_, u64>(()) }
        }))
        .unwrap();
        assert_eq!(runs.borrow().len(), before + 1);
    }

    /// Pending once, then ready: lets the other task run.
    fn yield_once() -> impl std::future::Future<Output = ()> {
        let mut done = false;
        std::future::poll_fn(move |cx| {
            if done {
                std::task::Poll::Ready(())
            } else {
                done = true;
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        })
    }

    #[test]
    fn a_direct_conflict_and_an_oversized_pack_are_refused() {
        let req = MergeRequest {
            n: IssueNumber(1),
            base: main(),
            base_old: ObjectId([1; 20]),
            head: ObjectId([4; 20]),
            method: MergeMethod::Merge,
        };
        let c = ChainAnswer {
            steps: vec![],
            conflict: Some(ChainConflict { index: 0, paths: vec!["f".into()] }),
            pack: None,
            engine: "g".into(),
        };
        assert_eq!(c.merge_plan(&req), Err(CacheError::Conflict(vec!["f".into()])));
        let mut big = step(4, 40);
        big.pack = Some(vec![0; MERGE_PACK_MAX + 1]);
        let a = ChainAnswer { steps: vec![big], conflict: None, pack: None, engine: "g".into() };
        assert!(matches!(a.merge_plan(&req), Err(CacheError::Bad(_))));
    }
}
