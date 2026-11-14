//! M4-WP3's in-memory gates (`build/BUILD-PLAN.md`, M4-WP3) on real members: an owner's Actions shard is three
//! `graftyard_raft::Member<ActionsCmd>`s and the repository three `Member<RepoCmd>`s. Simulated runners commit what the
//! run service would (`Matched`, `Acquired`, `Renewed`, `Finished`); the leader's host commits `ExpireLeases` at the
//! fold's next deadline, as its DO alarm does; and `RunOwner` runs `graftyard_actions::run_owner` rounds against both
//! groups. Time moves in 500 ms steps.
//!
//! - BF-4: a runner killed mid-job, then the Actions group left idle. Pass: the job fails within 10 minutes of its last
//!   `renewjob`. Fail: still running at 11 minutes.
//! - BF-56: the repository's commit path killed (its leader site) between `RunCreated` and the `RequiredCheck` writes,
//!   and separately `RunOwner` killed after a round's repository writes and before its cursor; then every job finishes
//!   or is cancelled. Pass: every `RequiredCheck` on an open PR head has a conclusion within 60 s of its job's
//!   `Completed`. Fail: any `RequiredCheck` with `conclusion: None` after its job completed.
//! - One runner at a time (SHARED): two runners matched to one job at the same version; only the first commits.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::Group;
use graftyard_actions::run_owner::{round, touched};
use graftyard_actions::shard::{
    ACTIONS_FORMAT, ActionsActor, ActionsCmd, ActionsErr, ActionsEvent, ActionsFold, ActionsOp, ActionsOut, JobSpec,
    RunnerRecord, RunnerScope, WorkflowRun,
};
use graftyard_actions::{Conclusion, JobEvent, JobId, JobState, LabelSet, RunId, RunStatus, RunnerId};
use graftyard_git::issues::{
    BranchRule, CheckConclusion, HeadSpec, IssueCapture, IssueNumber, MergeMethod, PrCapture, PrState, Protection,
    RepoSettings, RequiredChecks, Visibility,
};
use graftyard_git::merge::required_on;
use graftyard_git::refs::{ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{REPO_FORMAT_V1, RepoCmd, RepoEvent, RepoFold, RepoOut};
use graftyard_git::run_checks::{REPO_FORMAT_ACTIONS, RunChecksOut};
use graftyard_raft::{CommitError, RequestId, Ulid};
use graftyard_types::{AccountId, ChainId, DbId, IdBlock, Index, RepoId, Timestamp};
use smallvec::{SmallVec, smallvec};

const OWNER: AccountId = AccountId(DbId(100));
const REPO: RepoId = RepoId(DbId(7));
const STEP: u64 = 500;
const MIN: u64 = 60_000;

fn name(s: &str) -> RefName {
    RefName::parse(s.as_bytes()).unwrap()
}

fn oid(n: u32) -> ObjectId {
    let mut o = [0u8; 20];
    o[..4].copy_from_slice(&n.to_be_bytes());
    o[19] = 9;
    ObjectId(o)
}

fn labels() -> LabelSet {
    LabelSet::parse(["self-hosted", "linux"]).unwrap()
}

fn spec(n: &str, needs: &[usize]) -> JobSpec {
    JobSpec {
        id: JobId(DbId(0)),
        name: n.into(),
        labels: labels(),
        needs: needs.iter().map(|p| JobId(DbId(*p as i64))).collect(),
        matrix: SmallVec::new(),
        environment: None,
        steps: "[]".into(),
    }
}

fn op(o: ActionsOp) -> ActionsCmd {
    ActionsCmd { actor: ActionsActor::Operator, op: o }
}

/// What each simulated job does once a runner takes it: run for `ms`, then finish with the conclusion.
#[derive(Clone, Copy)]
struct Plan {
    ms: u64,
    ends: Conclusion,
}

struct World {
    now: Timestamp,
    shard: Group<ActionsCmd>,
    repo: Group<RepoCmd>,
    next: u128,
    runners: Vec<RunnerId>,
    /// Per job: the runner holding it, and when it finishes.
    running: BTreeMap<JobId, (RunnerId, Timestamp)>,
    plans: BTreeMap<Box<str>, Plan>,
    /// RunOwner: whether it runs, until when it is down, and the repositories that answered `Gone`.
    owner_up: bool,
    owner_down_until: Timestamp,
    gone: BTreeSet<RepoId>,
    /// Kill `RunOwner` after its next round's repository writes, before its shard half.
    kill_owner_after_checks: bool,
    rounds: u32,
    /// When each (head SHA, check name) first had a conclusion on the repository's leader.
    concluded_at: BTreeMap<(ObjectId, Box<str>), (Timestamp, CheckConclusion)>,
}

impl World {
    fn new() -> World {
        let now = Timestamp(1_000_000);
        let mut shard = Group::new(ChainId::Actions(OWNER, 3), vec![ACTIONS_FORMAT], ActionsFold::default(), now);
        shard.id_block(now, IdBlock::whole(0, 9).unwrap());
        let reads = vec![
            REPO_FORMAT_V1,
            graftyard_git::repo::REPO_FORMAT_REPOS,
            graftyard_git::issue_ops::call::REPO_FORMAT_CHECKS,
            REPO_FORMAT_ACTIONS,
        ];
        let repo = Group::new(ChainId::Repo(REPO), reads, RepoFold::new(REPO, OWNER), now);
        let mut w = World {
            now,
            shard,
            repo,
            next: 1,
            runners: Vec::new(),
            running: BTreeMap::new(),
            plans: BTreeMap::new(),
            owner_up: true,
            owner_down_until: Timestamp(0),
            gone: BTreeSet::new(),
            kill_owner_after_checks: false,
            rounds: 0,
            concluded_at: BTreeMap::new(),
        };
        w.seed_repo();
        w
    }

    fn rid(&mut self) -> u128 {
        self.next += 1;
        self.next << 32
    }

    /// Settings, `main`, a rule on `main` requiring every job's check, and two open pull requests.
    fn seed_repo(&mut self) {
        let settings = RepoSettings {
            owner: OWNER,
            enterprise: None,
            visibility: Visibility::Private,
            default_branch: name("refs/heads/main"),
            archived: false,
            description: None,
            homepage: None,
            topics: SmallVec::new(),
            has_issues: true,
            has_wiki: false,
            has_projects: false,
            merge_methods: smallvec![MergeMethod::Merge],
            delete_branch_on_merge: false,
            allow_auto_merge: false,
            allow_update_branch: false,
            check_retention_days: 90,
        };
        let checks =
            ["build", "test", "lint", "deploy", "report", "notify"].iter().map(|c| (Box::from(*c), None)).collect();
        let rule = BranchRule {
            id: DbId(1),
            pattern: "main".into(),
            protection: Some(Protection {
                checks: Some(RequiredChecks { strict: false, checks }),
                reviews: None,
                push: None,
                enforce_admins: false,
                linear_history: false,
                force_pushes: false,
                deletions: false,
                block_creations: false,
                conversation_resolution: false,
                lock_branch: false,
                signatures: false,
                merge_queue: None,
            }),
        };
        let up = |r: &str, new: ObjectId| RefUpdate { name: name(r), expect: None, new: Some(new) };
        let refs = RepoEvent::Refs {
            push: RequestId(Ulid(1)),
            actor: None,
            updates: vec![up("refs/heads/main", oid(1)), up("refs/heads/a", oid(11)), up("refs/heads/b", oid(12))],
            pack: None,
        };
        let (now, id) = (self.now, self.rid());
        self.repo.record(now, id, REPO_FORMAT_V1, vec![RepoEvent::Settings(settings), refs, RepoEvent::Rule(rule)]);
        for (n, branch, head) in [(1, "refs/heads/a", oid(11)), (2, "refs/heads/b", oid(12))] {
            let issue = IssueCapture {
                id: DbId((1 << 40) + n),
                author: None,
                title: "t".into(),
                body: "".into(),
                labels: Default::default(),
                assignees: Default::default(),
            };
            let capture = PrCapture {
                head_sha: head,
                base_sha: oid(1),
                state: PrState::Open { draft: false },
                labels: Vec::new(),
                issue,
                changes_from: None,
            };
            let ev = RepoEvent::PrOpened {
                n: IssueNumber(n as u32),
                head: HeadSpec { repo: REPO, branch: name(branch) },
                base: name("refs/heads/main"),
                capture,
            };
            let id = self.rid();
            self.repo.record(now, id, REPO_FORMAT_V1, vec![ev]);
        }
    }

    fn shard_cmd(&mut self, cmd: ActionsCmd) -> Option<Result<ActionsOut, ActionsErr>> {
        let (now, id) = (self.now, self.rid());
        match self.shard.commit(now, id, cmd)? {
            Ok(a) => Some(Ok(a.out)),
            Err(CommitError::Refused { err, .. }) => Some(Err(err)),
            Err(_) => None,
        }
    }

    fn register(&mut self, n: usize) {
        for i in 0..n {
            let rec = RunnerRecord {
                scope: RunnerScope::Repo(REPO),
                name: format!("r{i}").into(),
                labels: LabelSet::parse(["self-hosted", "linux", "x64"]).unwrap(),
                key: Box::new([1, 2, 3]),
                ephemeral: false,
                hosted: false,
                version: "2.338.2".into(),
            };
            match self.shard_cmd(op(ActionsOp::PutRunner(rec))) {
                Some(Ok(ActionsOut::Runner(id))) => self.runners.push(id),
                o => panic!("{o:?}"),
            }
        }
    }

    /// `ActionsTrigger`'s commit point for a run on `head`, from the repository entry `index`.
    fn create_run(&mut self, head: ObjectId, branch: &str, index: u64, jobs: Vec<JobSpec>) -> RunId {
        let run = WorkflowRun {
            id: RunId(DbId(0)),
            repo: REPO,
            workflow: ".github/workflows/ci.yml".into(),
            event: "pull_request".into(),
            head_sha: head.0,
            head_ref: branch.into(),
            trigger: (ChainId::Repo(REPO), Index(index)),
            attempt: 1,
            concurrency: None,
            fork: false,
        };
        match self.shard_cmd(op(ActionsOp::CreateRun { run, jobs })) {
            Some(Ok(ActionsOut::Run(id))) => id,
            o => panic!("{o:?}"),
        }
    }

    fn fold(&self) -> ActionsFold {
        self.shard.fold().clone()
    }

    /// Simulated runners: each free runner takes the oldest queued job (`Matched`, then `acquirejob`'s `Acquired`), and
    /// each job whose time is up finishes. A job with no plan runs until killed.
    fn runners_step(&mut self) {
        let f = self.fold();
        for (job, (runner, until)) in self.running.clone() {
            if self.now < until {
                continue;
            }
            let j = f.job(job).unwrap();
            let ends = self.plans[&j.spec.name].ends;
            let cmd = ActionsCmd {
                actor: ActionsActor::Job(job),
                op: ActionsOp::Job { job, version: j.version, event: JobEvent::Finished(ends) },
            };
            if matches!(self.shard_cmd(cmd), Some(Ok(_)) | Some(Err(ActionsErr::Illegal(_)))) {
                self.running.remove(&job);
                let _ = runner;
            }
        }
        let busy: BTreeSet<RunnerId> = self.running.values().map(|(r, _)| *r).collect();
        let free: Vec<RunnerId> = self.runners.iter().filter(|r| !busy.contains(r)).copied().collect();
        let mut free = free.into_iter();
        for (job, _, _, version) in f.queued() {
            let Some(r) = free.next() else { break };
            let name = f.job(job).unwrap().spec.name.clone();
            let m = ActionsCmd {
                actor: ActionsActor::Runner(r),
                op: ActionsOp::Job { job, version, event: JobEvent::Matched(r) },
            };
            if !matches!(self.shard_cmd(m), Some(Ok(_))) {
                continue;
            }
            let a = ActionsCmd {
                actor: ActionsActor::Runner(r),
                op: ActionsOp::Job { job, version: version + 1, event: JobEvent::Acquired },
            };
            assert!(matches!(self.shard_cmd(a), Some(Ok(_))));
            let ms = self.plans.get(&name).map_or(u64::MAX / 2, |p| p.ms);
            self.running.insert(job, (r, Timestamp(self.now.0 + ms)));
        }
    }

    /// The leader's DO alarm: `ExpireLeases` once the fold's earliest deadline passed.
    fn alarm(&mut self) {
        if self.shard.leader().is_some() && self.fold().next_deadline().is_some_and(|d| d <= self.now) {
            self.shard_cmd(op(ActionsOp::ExpireLeases));
        }
    }

    /// One `RunOwner` round: the repository's writes, and only once each is acknowledged or `Gone`, the shard's.
    fn owner_step(&mut self) {
        if !self.owner_up {
            if self.now < self.owner_down_until {
                return;
            }
            self.owner_up = true;
        }
        if self.shard.leader().is_none() {
            return;
        }
        let f = self.fold();
        let entries = self.shard.committed_after(f.owner_cursor);
        let Some((through, _)) = entries.last() else { return };
        let through = *through;
        let runs = touched(&f, entries.iter().flat_map(|(_, e)| e.iter()));
        let r = round(self.shard.chain, &f, &runs, through, &self.gone);
        if r.checks.is_empty() && r.then.is_empty() {
            return;
        }
        self.rounds += 1;
        for (repo, cmd) in r.checks {
            let id = cmd.request.0.0;
            match self.repo.commit(self.now, id, RepoCmd::RunChecks(cmd)) {
                Some(Ok(a)) => match a.out {
                    RepoOut::RunChecks(RunChecksOut::Applied(_)) => {}
                    // The round is rebuilt with the repository gone, so its shard half cancels the run.
                    RepoOut::RunChecks(RunChecksOut::Gone) => {
                        self.gone.insert(repo);
                        return;
                    }
                    _ => return,
                },
                // No leader, a lost ticket, or a refusal: the round runs again next step.
                _ => return,
            }
        }
        if self.kill_owner_after_checks {
            self.kill_owner_after_checks = false;
            self.owner_up = false;
            self.owner_down_until = Timestamp(self.now.0 + 20_000);
            return;
        }
        for (i, o) in r.then.into_iter().enumerate() {
            let mut key = Vec::new();
            graftyard_fold::canon::write(&mut key, &(through.0, i as u32));
            let id = graftyard_actions::shard::consumer_request(self.shard.chain, &key).0.0;
            if !matches!(self.shard.commit(self.now, id, op(o)), Some(Ok(_))) {
                return;
            }
        }
    }

    /// Records when each required check on the open heads first had a conclusion.
    fn observe(&mut self) {
        let Some(l) = self.repo.leader() else { return };
        let core: &dyn graftyard_fold::Read = &self.repo.m(l).group_fold().fold.core;
        for head in [oid(11), oid(12)] {
            for v in required_on(core, &head) {
                if let Some(c) = v.conclusion {
                    self.concluded_at.entry((head, v.name.clone())).or_insert((self.now, c));
                }
            }
        }
    }

    fn step(&mut self) {
        self.now = Timestamp(self.now.0 + STEP);
        let now = self.now;
        self.shard.tick(now);
        self.repo.tick(now);
        self.runners_step();
        self.alarm();
        self.owner_step();
        self.observe();
    }

    fn run_for(&mut self, ms: u64) {
        for _ in 0..ms / STEP {
            self.step();
        }
    }
}

// ---- BF-4 ----

#[test]
fn bf4_a_killed_runners_job_fails_at_its_lease_end_with_the_group_idle() {
    let mut w = World::new();
    w.register(1);
    let run = w.create_run(oid(11), "refs/heads/a", 5, vec![spec("build", &[])]);
    // No plan: the job runs until its runner dies.
    w.run_for(2_000);
    let job = w.fold().run(run).unwrap().jobs[0];
    assert!(matches!(w.fold().job(job).unwrap().state, JobState::InProgress { .. }));
    // The runner renews every 60 s [measured, `tests/m4/wp3/runner-protocol.md`] for 7 minutes.
    let mut last_renew = w.now;
    for _ in 0..7 {
        w.run_for(MIN);
        let v = w.fold().job(job).unwrap().version;
        let cmd = ActionsCmd {
            actor: ActionsActor::Job(job),
            op: ActionsOp::Job { job, version: v, event: JobEvent::Renewed },
        };
        assert!(matches!(w.shard_cmd(cmd), Some(Ok(ActionsOut::Job(JobState::InProgress { .. })))));
        last_renew = w.now;
    }
    // Every renew answered, but only the ones under `LEASE_REWRITE_UNDER` committed an entry.
    let renews = w
        .shard
        .committed_after(Index(0))
        .iter()
        .flat_map(|(_, e)| e.clone())
        .filter(|e| matches!(e, ActionsEvent::Job { event: JobEvent::Renewed, .. }))
        .count();
    assert!((1..=2).contains(&renews), "{renews} renew writes for 7 renews");
    // kill -9: the runner sends nothing more [measured]. Nothing else writes to the group.
    w.running.clear();
    w.runners.clear();
    let commit_at_kill = w.shard.commit_index();
    let mut failed = None;
    for _ in 0..(11 * MIN) / STEP {
        w.step();
        if let JobState::Completed { conclusion, at } = w.fold().job(job).unwrap().state {
            failed = Some((conclusion, at));
            break;
        }
    }
    let (conclusion, at) = failed.expect("the job is still running 11 minutes after its last renewjob");
    assert_eq!(conclusion, Conclusion::Failure);
    assert!(at.0 <= last_renew.0 + 10 * MIN, "failed {} s after the last renewjob", (at.0 - last_renew.0) / 1_000);
    // The group stayed idle: the only entries after the kill are the lease alarm's and RunOwner's.
    for (_, evs) in w.shard.committed_after(commit_at_kill) {
        for e in evs {
            assert!(
                matches!(
                    e,
                    ActionsEvent::Job { event: JobEvent::LeaseExpired, .. }
                        | ActionsEvent::RunStatus { .. }
                        | ActionsEvent::OwnerCursor { .. }
                ),
                "{e:?}"
            );
        }
    }
    assert_eq!(w.fold().run(run).unwrap().status, RunStatus::Completed(Conclusion::Failure));
}

// ---- BF-56 ----

/// Run A on PR #1's head: build and test pass, lint fails, deploy (needs test) passes, report (needs lint) is skipped,
/// and so is notify (needs report).
/// Run B on PR #2's head: cancelled while build runs.
fn bf56_world() -> (World, RunId, RunId) {
    let mut w = World::new();
    for (n, ms, ends) in [
        ("build", 20_000, Conclusion::Success),
        ("test", 15_000, Conclusion::Success),
        // The run's last job to finish, so nothing after its failure releases report's dependents but the skip.
        ("lint", 60_000, Conclusion::Failure),
        ("deploy", 5_000, Conclusion::Success),
        ("report", 5_000, Conclusion::Success),
        ("notify", 5_000, Conclusion::Success),
    ] {
        w.plans.insert(n.into(), Plan { ms, ends });
    }
    w.register(4);
    let jobs = || {
        let names =
            [("build", &[][..]), ("test", &[0]), ("lint", &[]), ("deploy", &[1]), ("report", &[2]), ("notify", &[4])];
        names.iter().map(|(n, needs)| spec(n, needs)).collect::<Vec<_>>()
    };
    let a = w.create_run(oid(11), "refs/heads/a", 2, jobs());
    let b = w.create_run(oid(12), "refs/heads/b", 3, jobs());
    (w, a, b)
}

fn bf56_finish(mut w: World, a: RunId, b: RunId) {
    w.run_for(8_000);
    // What run B's jobs had finished by the cancel keeps its conclusion; the rest are cancelled.
    let before: BTreeMap<JobId, Conclusion> =
        w.fold().run_jobs(b).iter().filter_map(|j| j.state.completed().map(|c| (j.spec.id, c))).collect();
    let admin = ActionsCmd { actor: ActionsActor::Admin(OWNER), op: ActionsOp::CancelRun { run: b } };
    assert!(matches!(w.shard_cmd(admin), Some(Ok(_))));
    assert!(
        before.len() < 6
            && w.fold().run_jobs(b).iter().any(|j| j.spec.name.as_ref() == "build" && !before.contains_key(&j.spec.id))
    );
    w.run_for(3 * MIN);
    let f = w.fold();
    let want: BTreeMap<&str, Conclusion> = [
        ("build", Conclusion::Success),
        ("test", Conclusion::Success),
        ("lint", Conclusion::Failure),
        ("deploy", Conclusion::Success),
        ("report", Conclusion::Skipped),
        ("notify", Conclusion::Skipped),
    ]
    .into_iter()
    .collect();
    for (run, head) in [(a, oid(11)), (b, oid(12))] {
        for j in f.run_jobs(run) {
            let JobState::Completed { conclusion, at } = j.state else { panic!("{} never completed", j.spec.name) };
            let expect = if run == a {
                want[&*j.spec.name]
            } else {
                before.get(&j.spec.id).copied().unwrap_or(Conclusion::Cancelled)
            };
            assert_eq!(conclusion, expect, "{}", j.spec.name);
            let Some((seen, c)) = w.concluded_at.get(&(head, j.spec.name.clone())) else {
                panic!("RequiredCheck {} on {head:?} has no conclusion after its job completed", j.spec.name)
            };
            assert_eq!(*c, graftyard_actions::run_owner::conclusion(conclusion));
            assert!(seen.0 <= at.0 + MIN, "{}: conclusion {} s after Completed", j.spec.name, (seen.0 - at.0) / 1_000);
        }
    }
    assert_eq!(f.run(a).unwrap().status, RunStatus::Completed(Conclusion::Failure));
    assert!(matches!(f.run(b).unwrap().status, RunStatus::Completed(_)));
    // A replay appended nothing twice: each check run's versions committed at most once each, in order.
    let mut versions: BTreeMap<i64, Vec<u64>> = BTreeMap::new();
    for (_, evs) in w.repo.committed_after(Index(0)) {
        for e in evs {
            if let RepoEvent::CheckRun(u) = e {
                versions.entry(u.run.0.0).or_default().push(u.version);
            }
        }
    }
    assert_eq!(versions.len(), 12, "one check run per job");
    for (run, v) in &versions {
        assert!(v.windows(2).all(|p| p[0] < p[1]), "check run {run} committed versions {v:?}");
    }
}

#[test]
fn bf56_a_killed_repository_leader_between_run_created_and_the_checks() {
    let (mut w, a, b) = bf56_world();
    // RunOwner has written nothing yet: kill the repository's leader site before its first round.
    let l = w.repo.leader().unwrap();
    assert!(
        w.repo.committed_after(Index(0)).iter().all(|(_, e)| !e.iter().any(|x| matches!(x, RepoEvent::CheckRun(_))))
    );
    w.repo.crash(l);
    w.run_for(2_000);
    assert!(w.repo.leader().is_none(), "the commit path is down");
    bf56_finish(w, a, b);
}

#[test]
fn bf56_a_killed_run_owner_between_its_repository_writes_and_its_cursor() {
    let (mut w, a, b) = bf56_world();
    w.kill_owner_after_checks = true;
    w.step();
    assert!(!w.owner_up, "RunOwner died after writing checks and before its cursor");
    assert_eq!(w.fold().owner_cursor, Index(0));
    bf56_finish(w, a, b);
}

// ---- one runner at a time ----

#[test]
fn of_two_runners_matched_to_one_job_only_the_first_commits() {
    let mut w = World::new();
    w.register(2);
    let run = w.create_run(oid(11), "refs/heads/a", 2, vec![spec("build", &[])]);
    let job = w.fold().run(run).unwrap().jobs[0];
    let v = w.fold().job(job).unwrap().version;
    let (r1, r2) = (w.runners[0], w.runners[1]);
    let m = |r| ActionsCmd {
        actor: ActionsActor::Runner(r),
        op: ActionsOp::Job { job, version: v, event: JobEvent::Matched(r) },
    };
    // Both decided in one entry: the second sees the first's match.
    let l = w.shard.leader().unwrap();
    let now = w.now;
    let t1 = w.shard.on(l, now, |mm, t| mm.commit(t, RequestId(Ulid(901)), m(r1))).unwrap().ticket;
    let t2 = w.shard.on(l, now, |mm, t| mm.commit(t, RequestId(Ulid(902)), m(r2))).unwrap().ticket;
    w.shard.deliver(now);
    let res = |t| {
        w.shard.resolutions.iter().find_map(|r| match r {
            graftyard_raft::Resolution::Ticket { ticket, result } if *ticket == t => Some(result.clone()),
            _ => None,
        })
    };
    assert!(matches!(res(t1), Some(Ok(_))));
    assert!(matches!(res(t2), Some(Err(CommitError::Refused { err: ActionsErr::Stale(_), .. }))), "{:?}", res(t2));
    assert_eq!(w.fold().job(job).unwrap().state.runner(), Some(r1));
    // A runner whose labels do not include the job's is refused.
    let mut w2 = World::new();
    let rec = RunnerRecord {
        scope: RunnerScope::Repo(REPO),
        name: "gpu".into(),
        labels: LabelSet::parse(["gpu"]).unwrap(),
        key: Box::new([1]),
        ephemeral: false,
        hosted: false,
        version: "2.338.2".into(),
    };
    let Some(Ok(ActionsOut::Runner(g))) = w2.shard_cmd(op(ActionsOp::PutRunner(rec))) else { panic!() };
    let run = w2.create_run(oid(11), "refs/heads/a", 2, vec![spec("build", &[])]);
    let job = w2.fold().run(run).unwrap().jobs[0];
    let cmd = ActionsCmd {
        actor: ActionsActor::Runner(g),
        op: ActionsOp::Job { job, version: 0, event: JobEvent::Matched(g) },
    };
    assert!(matches!(w2.shard_cmd(cmd), Some(Err(ActionsErr::Illegal(_)))));
    // A second run for the same trigger is refused.
    let again = WorkflowRun {
        id: RunId(DbId(0)),
        repo: REPO,
        workflow: ".github/workflows/ci.yml".into(),
        event: "pull_request".into(),
        head_sha: oid(11).0,
        head_ref: "refs/heads/a".into(),
        trigger: (ChainId::Repo(REPO), Index(2)),
        attempt: 1,
        concurrency: None,
        fork: false,
    };
    assert_eq!(
        w2.shard_cmd(op(ActionsOp::CreateRun { run: again, jobs: vec![spec("build", &[])] })),
        Some(Err(ActionsErr::Duplicate))
    );
}

// ---- the abort path ----

/// The repository is deleted while a run is in progress: `RunChecksCmd` answers `Gone`, and `RunOwner` cancels the run
/// (SHARED, "Multi-chain operations", the abort path), so no job waits on a repository that takes no checks.
#[test]
fn a_deleted_repositorys_run_is_cancelled() {
    let mut w = World::new();
    w.plans.insert("build".into(), Plan { ms: 10 * MIN, ends: Conclusion::Success });
    w.register(1);
    let run = w.create_run(oid(11), "refs/heads/a", 2, vec![spec("build", &[]), spec("test", &[0])]);
    w.run_for(3_000);
    let (now, id) = (w.now, w.rid());
    let gone = RepoEvent::Deleted { retain_until: Timestamp(now.0 + 86_400_000), actor: None };
    w.repo.record(now, id, graftyard_git::repo::REPO_FORMAT_REPOS, vec![gone]);
    // A runner's renew is the next shard entry RunOwner reads.
    let job = w.fold().run(run).unwrap().jobs[0];
    w.run_for(6 * MIN);
    let v = w.fold().job(job).unwrap().version;
    let renew =
        ActionsCmd { actor: ActionsActor::Job(job), op: ActionsOp::Job { job, version: v, event: JobEvent::Renewed } };
    assert!(w.shard_cmd(renew).is_some());
    w.run_for(10_000);
    assert!(w.gone.contains(&REPO));
    let f = w.fold();
    assert!(
        f.run_jobs(run).iter().all(|j| j.state.completed() == Some(Conclusion::Cancelled)),
        "{:?}",
        f.run_jobs(run)
    );
    assert_eq!(f.run(run).unwrap().status, RunStatus::Completed(Conclusion::Cancelled));
}

/// `RunOwner`'s cursor only moves forward: a stale round's cursor commits nothing.
#[test]
fn the_run_owner_cursor_never_moves_back() {
    let mut w = World::new();
    assert!(matches!(w.shard_cmd(op(ActionsOp::OwnerCursor { through: Index(9) })), Some(Ok(_))));
    let before = w.shard.commit_index();
    assert!(matches!(w.shard_cmd(op(ActionsOp::OwnerCursor { through: Index(4) })), Some(Ok(_))));
    assert_eq!(w.fold().owner_cursor, Index(9));
    let after = w.shard.committed_after(before);
    assert!(after.iter().all(|(_, e)| e.is_empty()), "{after:?}");
}
