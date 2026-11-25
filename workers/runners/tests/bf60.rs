//! BF-60 in memory (`build/BUILD-PLAN.md`, M4-WP4): 50 `ubuntu-latest` jobs queued for one owner with the cap at 20,
//! while two `graft-runners` sweeps overlap. Pass: at most 20 hosted runners at once; every job finishes; no Sandbox is
//! left 10 minutes after the last job.
//!
//! The owner's registration shard and the shard its repository's runs hash to are each three real
//! `graftyard_raft::Member<ActionsCmd>`s, so the cap is `decide`'s. The row runs off shard 0, as every hosted run did
//! (`m3org/wp4-bf60` is on shard 5), so a runner registers on one group and holds its job on another. The install's
//! routes are what the edge and site do: demand is `hosted::summary` of each shard's fold through `hosted::demand`
//! with one broker's `Seen`, applied after the shard reads as the broker's storage is; a registration commits
//! `PutRunner`; a delete is `hosted::delete_stale`, the edge's guard over both shards (F78). `graft-runners` is `control::pass`, the code the Worker runs. The Sandboxes are a fake host:
//! each runs one simulated runner that boots, opens its session, takes the oldest queued job its labels fit (as the
//! broker's match commits it), runs it, finishes it, deletes its own registration, and exits, which stops its
//! Sandbox. Every seventh runner instead hangs after its job (its process never exits), and every eleventh exits
//! without deleting its registration, so the `stale` path has work. Every call a pass makes yields once, and while it
//! is suspended the other pass runs and every booted runner polls, so the two passes interleave at each call and
//! runners take jobs between a pass's demand read and its deletes.

#[path = "../../../crates/actions/tests/common/mod.rs"]
mod common;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use common::Group;
use graftyard_actions::broker::{Pick, Taken};
use graftyard_actions::hosted::{self, DeleteHost, Deleted, HostedDemand, HostedLabel, Seen, ShardSummary};
use graftyard_actions::shard::{
    ACTIONS_FORMAT, ACTIONS_FORMAT_OWNER_MATCH, ActionsActor, ActionsCmd, ActionsErr, ActionsFold, ActionsOp,
    ActionsOut, JobSpec, WorkflowRun,
};
use graftyard_actions::{
    ACTIONS_SHARDS, Conclusion, JobEvent, JobId, JobState, LabelSet, OWNER_SCOPE_SHARD, RunId, RunnerId,
    RunnerSessionId, actions_shard,
};
use graftyard_raft::CommitError;
use graftyard_runners::control::{self, Host, Instance, Registered};
use graftyard_types::{AccountId, ChainId, DbId, IdBlock, Index, RepoId, Timestamp};
use smallvec::SmallVec;

const OWNER: AccountId = AccountId(DbId(100));
const S: u64 = 1_000;
const MIN: u64 = 60 * S;
/// A Sandbox's start and the runner's configure before its first poll [guess].
const BOOT: u64 = 20 * S;
/// The Cron Trigger's period (SHARED: 5 minutes [guess]).
const SWEEP: u64 = 5 * MIN;

/// A runner's process in its Sandbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Booting {
        ready: Timestamp,
    },
    Idle,
    Running {
        job: JobId,
        ends: Timestamp,
    },
    /// Finished its job and never exits (plant: a stuck runner).
    Hung,
}

#[derive(Clone, Debug)]
struct Sandbox {
    phase: Phase,
}

struct World {
    now: Timestamp,
    /// The registration shard, `OWNER_SCOPE_SHARD`.
    shard: Group<ActionsCmd>,
    /// The shard the repository's runs hash to, and its group when it is not the registration shard.
    jobs_shard: u8,
    jobs: Option<Group<ActionsCmd>>,
    next: u128,
    /// Sandboxes by runner, live until their runner exits or `destroy`.
    boxes: BTreeMap<RunnerId, Sandbox>,
    started: u32,
    seen: Seen,
    /// Each job's run time.
    lengths: BTreeMap<JobId, u64>,
    /// When each job reached `Completed`, and its conclusion.
    done: BTreeMap<JobId, (Timestamp, Conclusion)>,
    max_registered: usize,
    max_boxes: usize,
    rng: u64,
    /// Runners poll newest first, else in a seeded random order.
    newest_first: bool,
    full: u32,
    kept_busy: u32,
    /// The broker's memory of the ephemeral runners it matched or saw holding a job (BF-55's `Taken`).
    taken: Taken,
    /// Matches committed to a hung runner, which never takes them.
    rematched: u32,
}

impl World {
    /// The row's world: the repository's runs on shard 5, as on the hosted runs.
    fn new() -> World {
        World::on(5)
    }

    fn on(jobs_shard: u8) -> World {
        let now = Timestamp(1_000_000);
        let reads = vec![ACTIONS_FORMAT, ACTIONS_FORMAT_OWNER_MATCH];
        let group = |s: u8, block: u32| {
            let mut g = Group::new(ChainId::Actions(OWNER, s), reads.clone(), ActionsFold::default(), now);
            g.id_block(now, IdBlock::whole(0, block).unwrap());
            g
        };
        World {
            now,
            shard: group(OWNER_SCOPE_SHARD, 9),
            jobs_shard,
            jobs: (jobs_shard != OWNER_SCOPE_SHARD).then(|| group(jobs_shard, 10)),
            next: 1,
            boxes: BTreeMap::new(),
            started: 0,
            seen: Seen::default(),
            lengths: BTreeMap::new(),
            done: BTreeMap::new(),
            max_registered: 0,
            max_boxes: 0,
            rng: 0x9e37_79b9_7f4a_7c15,
            newest_first: false,
            full: 0,
            kept_busy: 0,
            taken: Taken::default(),
            rematched: 0,
        }
    }

    fn rand(&mut self, n: u64) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng % n
    }

    /// A command on the registration shard: registrations, sessions, deletes.
    fn cmd(&mut self, actor: ActionsActor, op: ActionsOp) -> Result<ActionsOut, ActionsErr> {
        self.cmd_on(OWNER_SCOPE_SHARD, actor, op)
    }

    /// A command on the jobs shard: runs, matches, and job events.
    fn job_cmd(&mut self, actor: ActionsActor, op: ActionsOp) -> Result<ActionsOut, ActionsErr> {
        self.cmd_on(self.jobs_shard, actor, op)
    }

    fn cmd_on(&mut self, s: u8, actor: ActionsActor, op: ActionsOp) -> Result<ActionsOut, ActionsErr> {
        self.next += 1;
        let id = self.next << 32;
        let (now, g) =
            (self.now, if s == OWNER_SCOPE_SHARD { &mut self.shard } else { self.jobs.as_mut().expect("jobs") });
        match g.commit(now, id, ActionsCmd { actor, op }) {
            Some(Ok(a)) => Ok(a.out),
            Some(Err(CommitError::Refused { err, .. })) => Err(err),
            o => panic!("the shard did not answer: {o:?}"),
        }
    }

    /// The registration shard's fold.
    fn fold(&self) -> &ActionsFold {
        self.shard.fold()
    }

    /// The jobs shard's fold.
    fn jobs_fold(&self) -> &ActionsFold {
        self.jobs.as_ref().map_or_else(|| self.shard.fold(), |g| g.fold())
    }

    fn registered(&self) -> usize {
        self.fold().runners.values().filter(|r| r.hosted).count()
    }

    /// Runs of `per` `ubuntu-latest` jobs on a repository whose runs land on the registration shard (BF-55 builds
    /// matching across the owner's 16 shards), each job `1` to `8` minutes long.
    fn queue(&mut self, runs: usize, per: usize) {
        let repo = (1..).map(|i| RepoId(DbId(i))).find(|r| actions_shard(*r) == self.jobs_shard).unwrap();
        for i in 0..runs {
            let jobs = (0..per)
                .map(|j| JobSpec {
                    id: JobId(DbId(0)),
                    name: format!("test ({j})").into(),
                    labels: LabelSet::parse(["ubuntu-latest"]).unwrap(),
                    needs: SmallVec::new(),
                    matrix: SmallVec::new(),
                    environment: None,
                    steps: "[]".into(),
                })
                .collect();
            let run = WorkflowRun {
                id: RunId(DbId(0)),
                repo,
                workflow: ".github/workflows/ci.yml".into(),
                event: "workflow_dispatch".into(),
                head_sha: [i as u8; 20],
                head_ref: "refs/heads/main".into(),
                trigger: (ChainId::Repo(repo), Index(i as u64 + 1)),
                attempt: 1,
                concurrency: None,
                fork: false,
            };
            let Ok(ActionsOut::Run(id)) = self.job_cmd(ActionsActor::Operator, ActionsOp::CreateRun { run, jobs })
            else {
                panic!("run")
            };
            for j in self.jobs_fold().run_jobs(id).iter().map(|j| j.spec.id).collect::<Vec<_>>() {
                let len = (1 + self.rand(8)) * MIN;
                self.lengths.insert(j, len);
            }
        }
    }

    /// Every booted runner polls or works once, as the broker and the run service would answer it now.
    fn runners_act(&mut self) {
        let mut ids: Vec<RunnerId> = self.boxes.keys().copied().collect();
        if self.newest_first {
            ids.reverse();
        } else {
            for i in (1..ids.len()).rev() {
                let k = self.rand(i as u64 + 1) as usize;
                ids.swap(i, k);
            }
        }
        for r in ids {
            let Some(b) = self.boxes.get(&r).cloned() else { continue };
            match b.phase {
                Phase::Booting { ready } if self.now >= ready => {
                    let session = RunnerSessionId(r.0.0 as u128);
                    self.cmd(ActionsActor::Runner(r), ActionsOp::OpenSession { runner: r, session })
                        .expect("a registered runner opens its session");
                    self.set(r, Phase::Idle);
                    self.poll(r);
                }
                Phase::Idle => self.poll(r),
                Phase::Running { job, ends } if self.now >= ends => self.finish(r, job),
                // The listener polls while its worker runs, and a hung runner's listener keeps polling.
                Phase::Running { .. } | Phase::Hung => self.poll(r),
                _ => {}
            }
        }
        let n = self.registered();
        self.max_registered = self.max_registered.max(n);
        self.max_boxes = self.max_boxes.max(self.boxes.len());
    }

    fn set(&mut self, r: RunnerId, phase: Phase) {
        self.boxes.get_mut(&r).expect("live").phase = phase;
    }

    /// One poll as the owner's broker serves it (`workers/site/src/broker.rs`, BF-55's rules): the registration
    /// shard's record, then `Taken::pick` over both shards' summaries, and the operator's `MatchOwner` on the jobs
    /// shard for a match. A
    /// hosted runner is ephemeral and `Owner`-scoped, so a runner this broker matched or saw holding a job is never
    /// matched again. A runner that took its match acquires it at once; a hung one never does.
    fn poll(&mut self, r: RunnerId) {
        let Some(rec) = self.fold().runners.get(&r).cloned() else { return };
        let (reg, jobs) = (self.fold().summary(), self.jobs_fold().summary());
        let mut sums = vec![(OWNER_SCOPE_SHARD, &reg)];
        if self.jobs.is_some() {
            sums.push((self.jobs_shard, &jobs));
        }
        let picked = self.taken.pick(&sums, true, r, &rec.labels, rec.ephemeral, self.now);
        let Pick::Match { job, version, .. } = picked else { return };
        let op = ActionsOp::MatchOwner { job, version, runner: r, labels: rec.labels.clone() };
        self.job_cmd(ActionsActor::Operator, op).unwrap();
        self.taken.matched(r);
        if self.boxes[&r].phase == Phase::Hung {
            self.rematched += 1;
            return;
        }
        self.job_cmd(ActionsActor::Runner(r), ActionsOp::Job { job, version: version + 1, event: JobEvent::Acquired })
            .unwrap();
        let ends = Timestamp(self.now.0 + self.lengths[&job]);
        self.set(r, Phase::Running { job, ends });
    }

    fn finish(&mut self, r: RunnerId, job: JobId) {
        let version = self.jobs_fold().job(job).unwrap().version;
        let ev = JobEvent::Finished(Conclusion::Success);
        self.job_cmd(ActionsActor::Job(job), ActionsOp::Job { job, version, event: ev }).unwrap();
        match r.0.0 % 77 {
            n if n % 7 == 0 => self.set(r, Phase::Hung),
            n if n % 11 == 0 => {
                self.boxes.remove(&r);
            }
            _ => {
                self.cmd(ActionsActor::Runner(r), ActionsOp::CloseSession { runner: r }).unwrap();
                self.cmd(ActionsActor::Runner(r), ActionsOp::DeleteRunner(r)).unwrap();
                // The runner exits, and its exit stops the Sandbox (`workers/runners`, the Worker's start command).
                self.boxes.remove(&r);
            }
        }
    }

    /// The leader's lease alarm, as the site's `expire` commits it, and every job that reached `Completed`.
    fn alarm_and_record(&mut self) {
        if self.jobs_fold().next_deadline().is_some_and(|d| d <= self.now) {
            self.job_cmd(ActionsActor::Operator, ActionsOp::ExpireLeases).unwrap();
        }
        let done: Vec<(JobId, Conclusion)> =
            self.jobs_fold().jobs.iter().filter_map(|(id, j)| j.state.completed().map(|c| (*id, c))).collect();
        for (j, c) in done {
            self.done.entry(j).or_insert((self.now, c));
        }
    }

    /// One second: the shard's ticks, the runners, the lease alarm.
    fn second(&mut self) {
        let now = Timestamp(self.now.0 + S);
        self.now = now;
        self.shard.tick(now);
        if let Some(g) = self.jobs.as_mut() {
            g.tick(now);
        }
        self.runners_act();
        self.alarm_and_record();
    }

    fn summary(&self) -> Vec<ShardSummary> {
        let mut out = vec![ShardSummary::default(); ACTIONS_SHARDS as usize];
        out[OWNER_SCOPE_SHARD as usize] = hosted::summary(self.fold());
        out[self.jobs_shard as usize] = hosted::summary(self.jobs_fold());
        out
    }
}

/// Yields once, so `join` runs the other pass, then lets the runners act, as they do while a pass waits on the network.
struct Yield<'a>(&'a RefCell<World>, bool);

impl Future for Yield<'_> {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.1 {
            self.0.borrow_mut().runners_act();
            return Poll::Ready(());
        }
        self.1 = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

/// The install and the runner account as one pass sees them.
struct Fake<'a>(&'a RefCell<World>);

impl Host for Fake<'_> {
    fn owners(&self) -> Vec<String> {
        vec!["acme".into()]
    }
    fn account_vcpu(&self) -> u32 {
        1_500
    }
    /// The broker's read: the shards' summaries now, then, after the runners act, the stored `Seen` (as the broker
    /// reads its storage after its shard reads), and the runners act again while the answer travels back.
    async fn demand(&self, _: &str) -> Result<HostedDemand, String> {
        let (shards, began) = {
            let w = self.0.borrow();
            (w.summary(), w.now)
        };
        Yield(self.0, false).await;
        let d = {
            let mut w = self.0.borrow_mut();
            let now = w.now;
            hosted::demand(&shards, hosted::HOSTED_CAP, began, now, &mut w.seen)
        };
        Yield(self.0, false).await;
        Ok(d)
    }
    async fn register(&self, _: &str, row: &'static HostedLabel) -> Registered {
        Yield(self.0, false).await;
        let mut w = self.0.borrow_mut();
        let rec = hosted::record(row, "graftyard-hosted", Box::new([1, 2, 3]), "2.338.2");
        match w.cmd(ActionsActor::Operator, ActionsOp::PutRunner(rec)) {
            Ok(ActionsOut::Runner(id)) => Registered::Runner { id, jit: format!("jit-{}", id.0.0) },
            Err(ActionsErr::TooMany) => {
                w.full += 1;
                Registered::Full
            }
            o => Registered::Failed(format!("{o:?}")),
        }
    }
    /// The edge's delete: `hosted::delete_stale` over the shards the owner's repository gives, with the runners
    /// acting before each read and the commit.
    async fn delete(&self, _: &str, runner: RunnerId) -> Result<bool, String> {
        let shards = {
            let w = self.0.borrow();
            let repo = w.jobs_fold().runs.values().next().map(|r| r.run.repo);
            hosted::job_shards(repo)
        };
        match hosted::delete_stale(&FakeDelete(self.0, runner), &shards, runner).await? {
            Deleted::Gone => Ok(true),
            Deleted::Busy { .. } => {
                self.0.borrow_mut().kept_busy += 1;
                Ok(false)
            }
        }
    }
    async fn start(&self, _: &str, runner: RunnerId, instance: Instance, _: String) -> Result<(), String> {
        Yield(self.0, false).await;
        let mut w = self.0.borrow_mut();
        assert_eq!(instance, Instance::Standard3, "`ubuntu-latest` runs on standard-3");
        let ready = Timestamp(w.now.0 + BOOT);
        w.boxes.insert(runner, Sandbox { phase: Phase::Booting { ready } });
        w.started += 1;
        let n = w.boxes.len();
        w.max_boxes = w.max_boxes.max(n);
        Ok(())
    }
    async fn destroy(&self, runner: RunnerId, _: Instance) -> Result<(), String> {
        Yield(self.0, false).await;
        self.0.borrow_mut().boxes.remove(&runner);
        Ok(())
    }
}

/// The edge's side of a stale delete in the fake: fresh shard summaries, and `DeleteRunner` as the operator.
struct FakeDelete<'a>(&'a RefCell<World>, RunnerId);

impl DeleteHost for FakeDelete<'_> {
    type Err = String;
    async fn read(&self, shards: &std::collections::BTreeSet<u8>) -> Result<Vec<(u8, ShardSummary)>, String> {
        Yield(self.0, false).await;
        let all = self.0.borrow().summary();
        Ok(shards.iter().map(|s| (*s, all[*s as usize].clone())).collect())
    }
    async fn delete(&self) -> Result<bool, String> {
        Yield(self.0, false).await;
        match self.0.borrow_mut().cmd(ActionsActor::Operator, ActionsOp::DeleteRunner(self.1)) {
            Ok(_) => Ok(true),
            Err(ActionsErr::Illegal(_)) => Ok(false),
            Err(e) => Err(format!("{e:?}")),
        }
    }
}

/// `n` passes at once, interleaved at each of their calls.
fn passes(w: &RefCell<World>, n: usize) -> Vec<control::Report> {
    let host = Fake(w);
    futures_executor::block_on(futures_util::future::join_all((0..n).map(|_| control::pass(&host))))
}

/// The row: every job `Success`, at most 20 hosted runners and 20 Sandboxes at once, the cap reached and refused, and
/// no Sandbox 10 minutes after the last job.
#[test]
fn bf60_fifty_jobs_under_a_cap_of_twenty_with_overlapping_sweeps() {
    let w = RefCell::new(World::new());
    w.borrow_mut().queue(5, 10);
    let t0 = w.borrow().now;
    // The `workflow_job` `queued` webhooks prompt a pass, and two sweeps overlap it.
    let mut last_sweep = t0;
    let mut reports = passes(&w, 3);
    let mut completed = 0;
    let end = Timestamp(t0.0 + 4 * 3_600 * S);
    let mut last_job: Option<Timestamp> = None;
    while w.borrow().now < end {
        {
            w.borrow_mut().second();
        }
        let (now, done) = { (w.borrow().now, w.borrow().done.len()) };
        // A `workflow_job` `completed` webhook prompts a pass for each job that finished this second, and at every
        // Cron Trigger two sweeps run; all of this second's passes overlap.
        let mut n = done - completed;
        completed = done;
        if now.0 >= last_sweep.0 + SWEEP {
            last_sweep = now;
            n += 2;
        }
        if n > 0 {
            reports.extend(passes(&w, n));
        }
        if done == 50 && last_job.is_none() {
            last_job = Some(now);
        }
        if let Some(t) = last_job
            && now.0 >= t.0 + 10 * MIN
        {
            break;
        }
    }
    let w = w.into_inner();
    let failed: Vec<_> = w.done.iter().filter(|(_, (_, c))| *c != Conclusion::Success).collect();
    let errors: Vec<&String> = reports.iter().flat_map(|r| r.errors.iter()).collect();
    let last = last_job.unwrap_or_else(|| panic!("{} of 50 jobs finished in 4 h; {failed:?}", w.done.len()));
    assert!(failed.is_empty(), "every job succeeds: {failed:?}");
    assert!(w.max_registered <= 20, "{} hosted runners registered at once", w.max_registered);
    assert!(w.max_boxes <= 20, "{} Sandboxes at once", w.max_boxes);
    assert!(w.full > 0, "the overlapping passes reached the cap and `decide` refused past it");
    assert!(w.boxes.is_empty(), "Sandboxes left 10 minutes after the last job: {:?}", w.boxes);
    assert_eq!(w.registered(), 0, "hosted registrations left 10 minutes after the last job");
    assert_eq!(w.rematched, 0, "a hung ephemeral runner, still polling, was matched a second job");
    assert!(errors.is_empty(), "{errors:?}");
    eprintln!(
        "BF-60: last job {} s after queueing; {} Sandboxes started; at most {} registered and {} Sandboxes at once; \
         {} registrations refused at the cap; {} stale deletes refused (runner busy)",
        (last.0 - t0.0) / S,
        w.started,
        w.max_registered,
        w.max_boxes,
        w.full,
        w.kept_busy
    );
}

/// A runner the demand read listed stale that takes a job before the pass deletes it keeps its job and its Sandbox:
/// the install refuses the delete, and the pass destroys only what it deleted.
#[test]
fn a_stale_runner_that_takes_a_job_after_the_read_keeps_it() {
    let w = RefCell::new(World::new());
    w.borrow_mut().newest_first = true;
    w.borrow_mut().queue(1, 1);
    // Two runners for one job: the first pass registers one; a second, started alone, finds it idle and starts none.
    passes(&w, 1);
    {
        let mut x = w.borrow_mut();
        let rec = hosted::record(HostedLabel::named("ubuntu-latest").unwrap(), "extra", Box::new([1]), "2.338.2");
        let Ok(ActionsOut::Runner(id)) = x.cmd(ActionsActor::Operator, ActionsOp::PutRunner(rec)) else { panic!() };
        let ready = Timestamp(x.now.0);
        x.boxes.insert(id, Sandbox { phase: Phase::Booting { ready } });
        // Both are seen idle and neither has booted; the newer is stale (idle beyond the one queued job).
    }
    // While the pass waits on its delete, the newer runner boots first and takes the job.
    let r = passes(&w, 1);
    assert_eq!((r[0].destroyed, r[0].kept_busy, r[0].started), (0, 1, 0), "{r:?}");
    let mut x = w.into_inner();
    let running = x.boxes.values().filter(|b| matches!(b.phase, Phase::Running { .. })).count();
    assert_eq!(running, 1, "the runner that took the job still has its Sandbox");
    for _ in 0..(10 * 60) {
        x.second();
    }
    assert_eq!(x.done.values().map(|(_, c)| *c).collect::<Vec<_>>(), vec![Conclusion::Success]);
}

/// F78, run 1966233's sequence: a runner registered on shard 0 takes a job on shard 5 between two overlapping demand
/// reads. The later read stores its mark first; the earlier one, whose summaries show the runner idle, answers after
/// it, and the pass deletes what it lists stale. On main the earlier read counted the mark and listed the busy runner
/// stale, and the delete, checked on shard 0 alone, removed it and its Sandbox: the job failed at its lease end.
#[test]
fn a_runner_holding_a_job_on_another_shard_survives_an_overlapping_reads_stale_delete() {
    let w = RefCell::new(World::new());
    let runner = {
        let mut x = w.borrow_mut();
        x.queue(1, 1);
        let rec = hosted::record(HostedLabel::named("ubuntu-latest").unwrap(), "h", Box::new([1]), "2.338.2");
        let Ok(ActionsOut::Runner(id)) = x.cmd(ActionsActor::Operator, ActionsOp::PutRunner(rec)) else { panic!() };
        let ready = x.now;
        x.boxes.insert(id, Sandbox { phase: Phase::Booting { ready } });
        id
    };
    let host = Fake(&w);
    // Read A takes its summaries: the runner idle, its job queued on shard 5.
    let (early, began) = (w.borrow().summary(), w.borrow().now);
    // The runner boots and takes the job; read B, begun after, stores the mark first.
    w.borrow_mut().second();
    assert!(matches!(w.borrow().boxes[&runner].phase, Phase::Running { .. }), "the runner took the job");
    {
        let mut x = w.borrow_mut();
        let (now, late) = (x.now, x.summary());
        hosted::demand(&late, hosted::HOSTED_CAP, now, now, &mut x.seen);
    }
    // Read A answers from its own summaries, and its pass deletes what it lists, then destroys what it deleted.
    let d = {
        let mut x = w.borrow_mut();
        let now = x.now;
        hosted::demand(&early, hosted::HOSTED_CAP, began, now, &mut x.seen)
    };
    for r in &d.stale {
        if futures_executor::block_on(host.delete("acme", *r)).unwrap() {
            w.borrow_mut().boxes.remove(r);
        }
    }
    // Read A's stale list aside, a delete of the busy runner is refused on its jobs shard.
    assert!(!futures_executor::block_on(host.delete("acme", runner)).unwrap(), "the delete saw shard 5's job");
    let mut x = w.into_inner();
    assert!(x.boxes.contains_key(&runner), "the busy runner kept its Sandbox; read A listed {:?}", d.stale);
    assert!(d.stale.is_empty(), "read A judged the runner on read B's mark: {:?}", d.stale);
    for _ in 0..(10 * 60) {
        x.second();
    }
    assert_eq!(x.done.values().map(|(_, c)| *c).collect::<Vec<_>>(), vec![Conclusion::Success]);
}

/// `decide`'s half of the guard: on the registration shard a delete of a runner holding a job there is refused, which
/// covers a match committed between the edge's first read and the delete.
#[test]
fn decide_refuses_to_delete_a_runner_holding_a_job_on_its_shard() {
    let mut x = World::on(OWNER_SCOPE_SHARD);
    x.queue(1, 1);
    let rec = hosted::record(HostedLabel::named("ubuntu-latest").unwrap(), "h", Box::new([1]), "2.338.2");
    let Ok(ActionsOut::Runner(id)) = x.cmd(ActionsActor::Operator, ActionsOp::PutRunner(rec)) else { panic!() };
    let ready = x.now;
    x.boxes.insert(id, Sandbox { phase: Phase::Booting { ready } });
    x.second();
    assert!(matches!(x.boxes[&id].phase, Phase::Running { .. }), "the runner took the job");
    assert!(matches!(x.cmd(ActionsActor::Operator, ActionsOp::DeleteRunner(id)), Err(ActionsErr::Illegal(_))));
}

/// The cap counts registrations committed earlier in the same entry, so one entry never takes the owner past it, and
/// a hosted registration must be an ephemeral `Owner`-scope runner with one table row's labels.
#[test]
fn the_cap_holds_inside_one_entry_and_refuses_other_shapes() {
    use graftyard_actions::shard::{ActionsEvent, RunnerScope};
    use graftyard_raft::{Command, Fold, Scratch, TierSet};
    let row = HostedLabel::named("ubuntu-latest").unwrap();
    let mut f = ActionsFold::default();
    f.apply_id_block(IdBlock::whole(0, 9).unwrap());
    let mut s = f.scratch_at(Timestamp(1), TierSet::CORE);
    let put = |r| ActionsCmd { actor: ActionsActor::Operator, op: ActionsOp::PutRunner(r) };
    for i in 0..hosted::HOSTED_CAP {
        let (evs, _) = put(hosted::record(row, "h", Box::new([1]), "v")).decide(&s, &[]).expect("under the cap");
        assert!(matches!(evs[0], ActionsEvent::Runner { .. }), "{i}");
        for e in &evs {
            s.apply(e);
        }
    }
    let past = put(hosted::record(row, "h", Box::new([1]), "v")).decide(&s, &[]);
    assert_eq!(past.map(|_| ()), Err(ActionsErr::TooMany));
    let mut repo = hosted::record(row, "h", Box::new([1]), "v");
    repo.scope = RunnerScope::Repo(RepoId(DbId(7)));
    let mut bare = hosted::record(row, "h", Box::new([1]), "v");
    bare.labels = LabelSet::parse(["ubuntu-latest"]).unwrap();
    let s = f.scratch_at(Timestamp(1), TierSet::CORE);
    for r in [repo, bare] {
        assert!(matches!(put(r).decide(&s, &[]), Err(ActionsErr::Illegal(_))));
    }
    // A runner that is not hosted is not counted against the cap.
    let mut plain = hosted::record(row, "h", Box::new([1]), "v");
    plain.hosted = false;
    assert!(put(plain).decide(&s, &[]).is_ok());
    let _ = JobState::Completed { conclusion: Conclusion::Success, at: Timestamp(0) };
}
