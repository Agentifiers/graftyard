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
//! - R24: a run whose second job `needs` the first gets each job's `workflow_job` `queued`, `in_progress`, and
//!   `completed`, once each and in order, to an App installed on the owner that subscribes, and nothing to an App
//!   without `actions` read or whose installation does not reach the repository.
//! - `gh-token`: every way a job a runner held ends (finished, cancelled, its lease lost, its repository deleted)
//!   denies the job's `GITHUB_TOKEN` `jti` on its `Tokens` shard, through a deny that outlives the token, and nothing
//!   denies a token while its job runs.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::Group;
use graftyard_actions::run_owner::{ChecksAnswer, checks_answer, round, steps, touched};
use graftyard_actions::shard::{
    ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST, ActionsActor, ActionsCmd, ActionsErr, ActionsEvent, ActionsFold,
    ActionsOp, ActionsOut, JobSpec, RunnerRecord, RunnerScope, WorkflowRun,
};
use graftyard_actions::{Conclusion, JobEvent, JobId, JobState, LabelSet, RunId, RunStatus, RunnerId};
use graftyard_git::issues::{
    BranchRule, CheckConclusion, HeadSpec, IssueCapture, IssueNumber, MergeMethod, PrCapture, PrState, Protection,
    RepoSettings, RequiredChecks, Visibility,
};
use graftyard_git::merge::required_on;
use graftyard_git::refs::{ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{REPO_FORMAT_V1, RepoCmd, RepoEvent, RepoFold, RepoOut};
use graftyard_git::run_checks::REPO_FORMAT_ACTIONS;
use graftyard_identity::{Jti, TOKENS_FORMAT, TOKENS_FORMAT_V2, TokensCmd, TokensFold, TokensRead, jti_shard};
use graftyard_raft::{CommitError, RequestId, Ulid};
use graftyard_types::{AccountId, ChainId, DbId, IdBlock, Index, RepoId, Timestamp};
use smallvec::{SmallVec, smallvec};

const OWNER: AccountId = AccountId(DbId(100));
const REPO: RepoId = RepoId(DbId(7));
/// A second repository on the same Actions shard, after `REPO` in the round's order (`repo-gone`).
const REPO2: RepoId = RepoId(DbId(8));
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
    repo2: Group<RepoCmd>,
    /// Repositories whose members were purged: no group answers a commit to them.
    purged: BTreeSet<RepoId>,
    /// The owner's `Authz` names, which `RunOwner` reads when a repository's commit fails (`repo-gone`).
    authz: graftyard_identity::AuthzFold,
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
    /// R24: `RunOwner` sends each job's `workflow_job` steps (the site's `GRAFTYARD_APP_HOOKS`).
    app_hooks: bool,
    /// When each (head SHA, check name) first had a conclusion on the repository's leader.
    concluded_at: BTreeMap<(ObjectId, Box<str>), (Timestamp, CheckConclusion)>,
    /// `gh-token`: `RunOwner` denies ended jobs' tokens (the site's `GRAFTYARD_JOB_TOKENS`), on each `jti`'s shard.
    job_tokens: bool,
    tokens: BTreeMap<u16, Group<TokensCmd>>,
}

impl World {
    fn new() -> World {
        let now = Timestamp(1_000_000);
        let mut shard = Group::new(
            ChainId::Actions(OWNER, 3),
            vec![ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST, graftyard_actions::shard::ACTIONS_FORMAT_TOKENS],
            ActionsFold::default(),
            now,
        );
        shard.id_block(now, IdBlock::whole(0, 9).unwrap());
        let reads = vec![
            REPO_FORMAT_V1,
            graftyard_git::repo::REPO_FORMAT_REPOS,
            graftyard_git::issue_ops::call::REPO_FORMAT_CHECKS,
            REPO_FORMAT_ACTIONS,
            graftyard_git::workflow_job::REPO_FORMAT_WORKFLOW_JOB,
        ];
        let repo = Group::new(ChainId::Repo(REPO), reads.clone(), RepoFold::new(REPO, OWNER), now);
        let repo2 = Group::new(ChainId::Repo(REPO2), reads, RepoFold::new(REPO2, OWNER), now);
        let mut authz = graftyard_identity::AuthzFold::new(OWNER);
        for (n, r) in [("a", REPO), ("b", REPO2)] {
            let entry = graftyard_identity::RepoEntry::Live { repo: r, epoch: Default::default() };
            authz.names.insert(graftyard_identity::RepoName::parse(n).unwrap().key(), entry);
        }
        let mut w = World {
            now,
            shard,
            repo,
            repo2,
            purged: BTreeSet::new(),
            authz,
            next: 1,
            runners: Vec::new(),
            running: BTreeMap::new(),
            plans: BTreeMap::new(),
            owner_up: true,
            owner_down_until: Timestamp(0),
            gone: BTreeSet::new(),
            kill_owner_after_checks: false,
            rounds: 0,
            app_hooks: false,
            concluded_at: BTreeMap::new(),
            job_tokens: false,
            tokens: BTreeMap::new(),
        };
        w.seed_repo();
        w
    }

    fn rid(&mut self) -> u128 {
        self.next += 1;
        self.next << 32
    }

    /// Settings, `main`, a rule on `main` requiring every job's check, and two open pull requests; `REPO2` gets the
    /// settings alone.
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
        let other = RepoSettings { ..settings.clone() };
        self.repo.record(now, id, REPO_FORMAT_V1, vec![RepoEvent::Settings(settings), refs, RepoEvent::Rule(rule)]);
        let id = self.rid();
        self.repo2.record(now, id, REPO_FORMAT_V1, vec![RepoEvent::Settings(other)]);
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
        self.create_run_in(REPO, head, branch, index, jobs)
    }

    fn create_run_in(&mut self, repo: RepoId, head: ObjectId, branch: &str, index: u64, jobs: Vec<JobSpec>) -> RunId {
        let run = WorkflowRun {
            id: RunId(DbId(0)),
            repo,
            workflow: ".github/workflows/ci.yml".into(),
            event: "pull_request".into(),
            head_sha: head.0,
            head_ref: branch.into(),
            trigger: (ChainId::Repo(repo), Index(index)),
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
            // `gh-token`: the site's acquire marks the token it mints.
            let op = if self.job_tokens {
                ActionsOp::AcquireToken { job, version: version + 1 }
            } else {
                ActionsOp::Job { job, version: version + 1, event: JobEvent::Acquired }
            };
            let a = ActionsCmd { actor: ActionsActor::Runner(r), op };
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
        let jobs = if self.app_hooks {
            let l = self.shard.leader().unwrap();
            let timed: Vec<(Timestamp, ActionsEvent)> = self
                .shard
                .m(l)
                .log()
                .iter()
                .filter(|e| e.index > f.owner_cursor && e.index <= through)
                .flat_map(|e| match &e.body {
                    graftyard_raft::EntryBody::Events(v) => v.iter().map(|x| (e.at, x.clone())).collect(),
                    _ => Vec::new(),
                })
                .collect();
            steps(&f, timed.iter().map(|(at, e)| (*at, e)))
        } else {
            Vec::new()
        };
        let denies = if self.job_tokens {
            graftyard_actions::token::denies(&f, entries.iter().flat_map(|(_, e)| e.iter()))
        } else {
            Vec::new()
        };
        let r = round(self.shard.chain, &f, &runs, through, &self.gone, &jobs, &denies);
        if r.checks.is_empty() && r.then.is_empty() {
            return;
        }
        self.rounds += 1;
        for (repo, cmd) in r.checks {
            let id = cmd.request.0.0;
            let g = if repo == REPO2 { &mut self.repo2 } else { &mut self.repo };
            // No leader, a lost ticket, a refusal, or a purged repository's missing group: no outcome.
            let commit = (!self.purged.contains(&repo)).then(|| g.commit(self.now, id, RepoCmd::RunChecks(cmd)));
            let out = match commit.flatten() {
                Some(Ok(a)) => match a.out {
                    RepoOut::RunChecks(o) => Some(o),
                    _ => None,
                },
                _ => None,
            };
            let gone = out.is_none() && !self.authz.holds(repo);
            match checks_answer(out, gone) {
                ChecksAnswer::Applied => {}
                // The round is rebuilt with the repository gone, so its shard half cancels the run.
                ChecksAnswer::Gone => {
                    self.gone.insert(repo);
                    return;
                }
                ChecksAnswer::Retry => return,
            }
        }
        for cmd in r.denies {
            let (now, id) = (self.now, self.rid());
            let shard = cmd.shard();
            let g = self.tokens.entry(shard.0).or_insert_with(|| {
                let chain = ChainId::Tokens(shard);
                Group::new(chain, vec![TOKENS_FORMAT, TOKENS_FORMAT_V2], TokensFold::new(shard), now)
            });
            if !matches!(g.commit(now, id, cmd), Some(Ok(_))) {
                return;
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

    /// Whether `jti` is revoked at `at` on its shard's leader.
    fn revoked(&self, jti: Jti, at: Timestamp) -> bool {
        self.tokens.get(&jti_shard(&jti).0).is_some_and(|g| g.fold().jti_revoked(&jti, at))
    }

    fn step(&mut self) {
        self.now = Timestamp(self.now.0 + STEP);
        let now = self.now;
        self.shard.tick(now);
        self.repo.tick(now);
        self.repo2.tick(now);
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

// ---- gh-token ----

/// The job's `GITHUB_TOKEN` `jti`, as `acquirejob` mints it.
fn jti_of(w: &World, job: JobId) -> Jti {
    let f = w.fold();
    graftyard_actions::token::job_jti(job, f.run(f.job(job).unwrap().run).unwrap().run.attempt)
}

/// BF-4's case for the token: a runner that holds a job vanishes, and nothing more comes from it. Its job's
/// `GITHUB_TOKEN` lives the job limit plus a lease, so only the deny bounds it: `RunOwner` revokes the `jti` within a
/// round of the lease failing the job, and the row outlives the longest token the job could hold. Before the lease
/// ends nothing is revoked. On main, `RunOwner` revokes nothing.
#[test]
fn a_vanished_runners_job_token_is_denied_at_its_lease_end() {
    let mut w = World::new();
    w.job_tokens = true;
    w.register(1);
    let run = w.create_run(oid(11), "refs/heads/a", 5, vec![spec("build", &[])]);
    w.run_for(2_000);
    let job = w.fold().run(run).unwrap().jobs[0];
    let JobState::InProgress { lease_until, .. } = w.fold().job(job).unwrap().state else { panic!("not running") };
    let jti = jti_of(&w, job);
    // kill -9 after the acquire: no renew, no finish.
    w.running.clear();
    w.runners.clear();
    // The step that reaches `lease_until` is the one whose alarm fails the job.
    while w.now.0 + STEP < lease_until.0 {
        w.step();
        assert!(!w.revoked(jti, w.now), "denied while its lease was live");
    }
    let mut denied = None;
    for _ in 0..(60_000 / STEP) {
        w.step();
        if w.revoked(jti, w.now) {
            denied = Some(w.now);
            break;
        }
    }
    let denied = denied.expect("the token is still live a minute after the lease failed its job");
    let JobState::Completed { conclusion, at } = w.fold().job(job).unwrap().state else { panic!("not completed") };
    assert_eq!(conclusion, Conclusion::Failure);
    assert!(denied.0 - at.0 <= 5_000, "denied {} ms after the job failed", denied.0 - at.0);
    let longest = at.0 + graftyard_identity::stateless::JOB_TOKEN_MAX.as_millis() as u64;
    assert!(w.revoked(jti, Timestamp(longest - 1)), "the deny lapses before the longest token the job could hold");
}

/// The other three ways a held job ends, each denying its token: it finishes, an admin cancels its run, and its
/// repository is deleted, which `RunOwner` answers with `CancelRun`. A job that ended without a runner (a need that
/// failed, or a cancel while queued) had no token and costs no deny, and another job's token stays live.
#[test]
fn a_finished_cancelled_or_deleted_repositorys_job_token_is_denied() {
    let mut w = World::new();
    w.job_tokens = true;
    w.plans.insert("fin".into(), Plan { ms: 20_000, ends: Conclusion::Success });
    w.plans.insert("bad".into(), Plan { ms: 10_000, ends: Conclusion::Failure });
    w.register(4);
    let fin = w.create_run(oid(11), "refs/heads/a", 2, vec![spec("fin", &[])]);
    let cxl = w.create_run(oid(11), "refs/heads/a", 3, vec![spec("cxl", &[]), spec("bad", &[]), spec("after", &[1])]);
    let live = w.create_run(oid(12), "refs/heads/b", 4, vec![spec("live", &[])]);
    // Cancelled while queued: no runner was given its token, so nothing is denied for it.
    let early = w.create_run(oid(13), "refs/heads/c", 5, vec![spec("early", &[])]);
    let j_early = w.fold().run(early).unwrap().jobs[0];
    let stop = ActionsCmd { actor: ActionsActor::Admin(AccountId(DbId(5))), op: ActionsOp::CancelRun { run: early } };
    assert!(matches!(w.shard_cmd(stop), Some(Ok(_))));
    w.run_for(2_000);
    let jobs = |w: &World, r: RunId| w.fold().run(r).unwrap().jobs.clone();
    let (j_fin, j_cxl, j_bad, j_after, j_live) =
        (jobs(&w, fin)[0], jobs(&w, cxl)[0], jobs(&w, cxl)[1], jobs(&w, cxl)[2], jobs(&w, live)[0]);
    assert!(!w.revoked(jti_of(&w, j_cxl), w.now));
    w.run_for(25_000);
    assert!(w.revoked(jti_of(&w, j_fin), w.now), "finished");
    assert!(w.revoked(jti_of(&w, j_bad), w.now), "failed");
    assert_eq!(w.fold().job(j_after).unwrap().state.completed(), Some(Conclusion::Skipped));
    assert!(!w.revoked(jti_of(&w, j_after), w.now), "skipped before any runner held it");
    assert_eq!(w.fold().job(j_early).unwrap().state.completed(), Some(Conclusion::Cancelled));
    assert!(!w.revoked(jti_of(&w, j_early), w.now), "cancelled before any runner held it");
    let cancel = ActionsCmd { actor: ActionsActor::Admin(AccountId(DbId(5))), op: ActionsOp::CancelRun { run: cxl } };
    assert!(matches!(w.shard_cmd(cancel), Some(Ok(_))));
    w.run_for(5_000);
    assert!(w.revoked(jti_of(&w, j_cxl), w.now), "cancelled");
    assert!(!w.revoked(jti_of(&w, j_live), w.now), "another job's token is untouched");
    // The repository is deleted: RunOwner's next round finds it `Gone` and cancels the live run. A renew writes only
    // once less than `LEASE_REWRITE_UNDER` is left, and that write is the shard entry the round reads.
    w.run_for(6 * MIN);
    assert!(!w.revoked(jti_of(&w, j_live), w.now), "a running job's token stays live");
    let (now, id) = (w.now, w.rid());
    let gone = RepoEvent::Deleted { retain_until: Timestamp(now.0 + 86_400_000), actor: None };
    w.repo.record(now, id, graftyard_git::repo::REPO_FORMAT_REPOS, vec![gone]);
    let v = w.fold().job(j_live).unwrap().version;
    let renew = ActionsCmd {
        actor: ActionsActor::Job(j_live),
        op: ActionsOp::Job { job: j_live, version: v, event: JobEvent::Renewed },
    };
    assert!(w.shard_cmd(renew).is_some());
    w.run_for(10_000);
    assert_eq!(w.fold().job(j_live).unwrap().state.completed(), Some(Conclusion::Cancelled));
    assert!(w.revoked(jti_of(&w, j_live), w.now), "its repository deleted");
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

/// F76 (BF-54 hosted): a `Repo`-scope runner whose poll lost its match's answer gets that job on a later poll, within
/// `RESEND_AFTER` plus one poll, and never a second job. Each poll is the shard leader's `match`: `repo_pick` over the
/// leader's fold, then `Matched` committed for a match. The runner's first answer is dropped (its poll timed out), so
/// it holds nothing and polls again every 5 s, as a listener whose poll failed does.
#[test]
fn a_repo_scope_runner_whose_match_answer_was_lost_gets_the_job_on_a_later_poll() {
    use graftyard_actions::broker::{Pick, RESEND_AFTER, Taken};
    let mut w = World::new();
    w.register(1);
    // Polled only here, not by the world's simulated runners.
    let r = w.runners.pop().unwrap();
    let labels = LabelSet::parse(["self-hosted", "linux", "x64"]).unwrap();
    let run = w.create_run(oid(11), "refs/heads/a", 2, vec![spec("build", &[]), spec("test", &[])]);
    let jobs = w.fold().run(run).unwrap().jobs.clone();
    let mut taken = Taken::default();
    let mut answered: Vec<(Timestamp, Pick)> = Vec::new();
    let matched_at = w.now;
    for poll in 0.. {
        let f = w.fold();
        let p = f.repo_pick(3, &mut taken, r, &labels, false, w.now);
        if let Pick::Match { job, version, .. } = p {
            let m = ActionsCmd {
                actor: ActionsActor::Runner(r),
                op: ActionsOp::Job { job, version, event: JobEvent::Matched(r) },
            };
            assert!(matches!(w.shard_cmd(m), Some(Ok(_))), "the match commits");
        }
        // The first answer is lost; every later one reaches the runner.
        if poll > 0 && !matches!(p, Pick::Busy | Pick::Idle) {
            answered.push((w.now, p));
            break;
        }
        w.run_for(5_000);
        assert!(w.now.0 < matched_at.0 + 2 * 60_000, "no answer after 2 minutes: {p:?}");
    }
    let (at, p) = answered.pop().unwrap();
    assert_eq!(p, Pick::Resend { shard: 3, job: jobs[0] }, "the lost match again, not the second job");
    assert!(at.0 - matched_at.0 <= RESEND_AFTER.as_millis() as u64 + 5_000, "after {} ms", at.0 - matched_at.0);
    // The second job was never matched to it while it held the first.
    assert!(matches!(w.fold().job(jobs[1]).unwrap().state, JobState::Queued { .. }));
}

/// F79: an `acquirejob` the edge answers 503, because a read it makes before the commit failed, commits nothing, so the
/// job stays `Assigned`. The runner retries the acquire and then polls on (actions/runner v2.338.2 drops a job only on
/// 404, 409, or 422): its next poll after `RESEND_AFTER` hands it the same job, and a runner that never comes back
/// leaves the job to its lease, which puts it back in the queue, not `Completed { Failure }`.
#[test]
fn an_acquire_answered_503_leaves_the_job_assigned_for_a_resend_and_its_lease_requeues_it() {
    use graftyard_actions::broker::{Pick, RESEND_AFTER, Taken};
    let mut w = World::new();
    w.register(1);
    let r = w.runners.pop().unwrap();
    let labels = LabelSet::parse(["self-hosted", "linux", "x64"]).unwrap();
    let run = w.create_run(oid(11), "refs/heads/a", 2, vec![spec("build", &[])]);
    let job = w.fold().run(run).unwrap().jobs[0];
    let p = w.fold().repo_pick(3, &mut Taken::default(), r, &labels, false, w.now);
    let Pick::Match { job: j, version, .. } = p else { panic!("{p:?}") };
    assert_eq!(j, job);
    let m =
        ActionsCmd { actor: ActionsActor::Runner(r), op: ActionsOp::Job { job, version, event: JobEvent::Matched(r) } };
    assert!(matches!(w.shard_cmd(m), Some(Ok(_))));
    // Every acquire is answered 503: nothing commits. The runner polls again after its retries.
    w.run_for(RESEND_AFTER.as_millis() as u64 + 5_000);
    assert!(matches!(w.fold().job(job).unwrap().state, JobState::Assigned { runner, .. } if runner == r));
    let p = w.fold().repo_pick(3, &mut Taken::default(), r, &labels, false, w.now);
    assert_eq!(p, Pick::Resend { shard: 3, job }, "the same job again, for another acquire");
    // The runner never acquires: at the lease end the job is queued again, not failed.
    w.run_for(graftyard_actions::JOB_LEASE.as_millis() as u64);
    assert!(matches!(w.fold().job(job).unwrap().state, JobState::Queued { .. }), "{:?}", w.fold().job(job));
}

/// F77 (BF-54 run 2): a runner that lost its `acquirejob` answer asks again (actions/runner v2.338.2 retries
/// `GetJobMessageAsync` on anything but 404, 409, and 422, and drops the job on those). Its second `Acquired` of the
/// job it holds `InProgress` commits nothing and is answered, keeping the lease the first one set; another runner's is
/// still refused, as is any acquire once the job completed.
#[test]
fn a_runners_repeated_acquire_of_its_own_job_is_answered_and_anothers_is_refused() {
    let mut w = World::new();
    w.register(2);
    let (r1, r2) = (w.runners.pop().unwrap(), w.runners.pop().unwrap());
    let run = w.create_run(oid(11), "refs/heads/a", 2, vec![spec("build", &[])]);
    let job = w.fold().run(run).unwrap().jobs[0];
    let v = w.fold().job(job).unwrap().version;
    let ev =
        |r, version, event| ActionsCmd { actor: ActionsActor::Runner(r), op: ActionsOp::Job { job, version, event } };
    assert!(matches!(w.shard_cmd(ev(r1, v, JobEvent::Matched(r1))), Some(Ok(_))));
    assert!(matches!(w.shard_cmd(ev(r1, v + 1, JobEvent::Acquired)), Some(Ok(_))), "the first acquire");
    assert!(matches!(w.fold().job(job).unwrap().state, JobState::InProgress { runner, .. } if runner == r1));
    // The answer is lost; the runner asks again 5 to 15 s later, at the version the run service reads then.
    w.run_for(10_000);
    let first = w.fold().job(job).unwrap().clone();
    let again = w.shard_cmd(ev(r1, first.version, JobEvent::Acquired));
    assert_eq!(again, Some(Ok(ActionsOut::Job(first.state.clone()))), "answered with the same lease");
    assert_eq!(w.fold().job(job).unwrap(), &first, "and nothing written");
    assert_eq!(
        w.shard_cmd(ev(r2, first.version, JobEvent::Acquired)),
        Some(Err(ActionsErr::Forbidden)),
        "another runner"
    );
    let done = ActionsCmd {
        actor: ActionsActor::Job(job),
        op: ActionsOp::Job { job, version: first.version, event: JobEvent::Finished(Conclusion::Success) },
    };
    assert!(matches!(w.shard_cmd(done), Some(Ok(_))));
    let v = w.fold().job(job).unwrap().version;
    assert!(matches!(w.shard_cmd(ev(r1, v, JobEvent::Acquired)), Some(Err(_))), "a completed job is not acquired");
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

// ---- R24: `workflow_job` to an App ----

fn app(id: i64, events: &[&str]) -> graftyard_identity::AppRecord {
    use graftyard_identity::{AppId, AppRecord, ClientId, EventKind, PermissionSet, Slug, Url};
    AppRecord {
        id: AppId(DbId(id)),
        slug: Slug::parse(&format!("app-{id}")).unwrap(),
        name: "an App".into(),
        client: ClientId::for_app(AppId(DbId(id))),
        keys: SmallVec::new(),
        permissions: PermissionSet::default(),
        events: events.iter().map(|e| EventKind((*e).into())).collect(),
        webhook: Some(Url(format!("https://app-{id}.example/hook").into())),
        webhook_secret: None,
        client_secrets: SmallVec::new(),
        callbacks: SmallVec::new(),
        public: false,
    }
}

/// R24: `RunOwner` (killed once between its repository writes and its cursor) carries a run's `workflow_job` steps to
/// the repository chain, and the `Webhooks` consumer's `plan` turns them into deliveries to the Apps installed on the
/// owner. On main no `workflow_job` reaches any App, so `graft-runners` waits for its 5-minute sweep.
#[test]
fn an_installed_app_gets_each_jobs_workflow_job_steps_once_in_order() {
    use graftyard_git::sequencer::{app_targets, plan};
    use graftyard_git::workflow_job::{JobAction, WorkflowJobUpdate};
    use graftyard_identity::{
        Access, AppId, InstallationId, InstallationRow, InstallationState, PermissionSet, RepoScope,
    };
    let mut w = World::new();
    w.app_hooks = true;
    w.kill_owner_after_checks = true;
    w.plans.insert("build".into(), Plan { ms: 20_000, ends: Conclusion::Success });
    w.plans.insert("test".into(), Plan { ms: 10_000, ends: Conclusion::Failure });
    w.register(1);
    let run = w.create_run(oid(11), "refs/heads/a", 2, vec![spec("build", &[]), spec("test", &[0])]);
    w.run_for(3 * MIN);
    let f = w.fold();
    assert!(f.run_jobs(run).iter().all(|j| j.state.completed().is_some()), "{:?}", f.run_jobs(run));
    let (build, test) = (f.run(run).unwrap().jobs[0], f.run(run).unwrap().jobs[1]);

    // Three Apps on the owner: one subscribed with `actions` read on every repository; one subscribed without
    // `actions`; one subscribed with `actions` read on another repository only.
    let perms = |p: &[(&str, Access)]| PermissionSet(p.iter().map(|(k, a)| (Box::from(*k), *a)).collect());
    let reads = perms(&[("actions", Access::Read), ("metadata", Access::Read)]);
    let records = [app(51, &["workflow_job"]), app(52, &["workflow_job"]), app(53, &["*"])];
    let installed = |n: i64, a: i64, repos, permissions| {
        let row = InstallationRow { app: AppId(DbId(a)), repos, permissions };
        (InstallationId(DbId(n)), InstallationState { row: Some(row), not_before: Timestamp(0) })
    };
    let installations = [
        installed(901, 51, RepoScope::All, reads.clone()),
        installed(902, 52, RepoScope::All, perms(&[("metadata", Access::Read), ("contents", Access::Write)])),
        installed(903, 53, RepoScope::Selected(smallvec![RepoId(DbId(8))]), reads.clone()),
    ]
    .into_iter()
    .collect();
    let targets = app_targets(&installations, REPO, |a| records.iter().find(|r| r.id == a));

    let l = w.repo.leader().unwrap();
    let log = w.repo.m(l).log().to_vec();
    let core = &w.repo.fold().core;
    let render = |wh: &graftyard_git::hooks::Webhook, i: Option<InstallationId>| match &wh.subject {
        graftyard_git::hooks::Subject::WorkflowJob(u) => serde_json::to_vec(&(u, i.map(|i| i.0.0))).ok(),
        _ => None,
    };
    let mut got: Vec<(i64, WorkflowJobUpdate, Option<i64>, String)> = Vec::new();
    for e in &log {
        let rows = plan(core, REPO, 1, e, &targets, render);
        // The same entry planned again derives the same GUIDs.
        assert_eq!(rows, plan(core, REPO, 1, e, &targets, render));
        for r in rows {
            assert_eq!(r.event.as_ref(), "workflow_job");
            let (u, i): (WorkflowJobUpdate, Option<i64>) = serde_json::from_slice(&r.body).unwrap();
            assert_eq!(r.action.as_deref(), Some(u.action.name()));
            got.push((r.hook.0.0, u, i, r.guid.to_string()));
        }
    }
    assert!(got.iter().all(|(h, _, i, _)| *h == -51 && *i == Some(901)), "only App 51, as installation 901");
    let seen: Vec<(JobId, JobAction)> = got.iter().map(|(_, u, _, _)| (JobId(u.job), u.action)).collect();
    use JobAction::{Completed, InProgress, Queued};
    assert_eq!(
        seen,
        [
            (build, Queued),
            (build, InProgress),
            (build, Completed),
            (test, Queued),
            (test, InProgress),
            (test, Completed)
        ],
        "each step once, in the order the shard committed them, `test` queued only when `build` released it"
    );
    let guids: BTreeSet<&str> = got.iter().map(|(_, _, _, g)| g.as_str()).collect();
    assert_eq!(guids.len(), got.len());
    // Each GUID is the App's own, from (generation, chain, index, slot, App): a hook on the chain never shares one.
    for e in &log {
        for r in plan(core, REPO, 1, e, &targets, render) {
            let (_, _, slot) = r.entry.unwrap();
            let app = graftyard_git::hooks::HookId::app(AppId(DbId(51)));
            assert_eq!(r.guid, graftyard_git::hooks::delivery_guid(1, ChainId::Repo(REPO), e.index, slot, app));
        }
    }
    for (_, u, _, _) in &got {
        let j = f.job(JobId(u.job)).unwrap();
        assert_eq!((u.run, u.attempt, &*u.name), (run.0, 1, &*j.spec.name));
        assert_eq!(u.labels.iter().map(|l| &**l).collect::<Vec<_>>(), ["linux", "self-hosted"]);
        match u.action {
            Queued => assert!(u.runner.is_none() && u.started.is_none() && u.completed.is_none()),
            InProgress => assert!(u.runner.is_some() && u.started.is_some_and(|s| s >= u.created)),
            Completed => {
                let JobState::Completed { conclusion, at } = j.state else { unreachable!() };
                assert_eq!(u.conclusion, Some(graftyard_actions::run_owner::conclusion(conclusion)));
                assert_eq!(u.completed, Some(at));
                // What the in_progress step knew rides the completed one.
                let started = got.iter().find(|(_, x, _, _)| x.job == u.job && x.action == InProgress).unwrap();
                assert_eq!((u.runner, u.started, u.created), (started.1.runner, started.1.started, started.1.created));
            }
        }
    }
    assert_eq!(
        got.iter().filter(|(_, u, _, _)| u.action == Completed).map(|(_, u, _, _)| u.conclusion).collect::<Vec<_>>(),
        [Some(CheckConclusion::Success), Some(CheckConclusion::Failure)]
    );
}

// ---- write-cuts cut (2) ----

/// An Actions job lease follows the path leases' rule (SHARED, Actions, Leases): stalls under EXTEND_MIN move it not
/// at all, and a stall of EXTEND_MIN or more moves it by that stall alone.
#[test]
fn a_job_lease_moves_only_for_a_stall_of_extend_min() {
    let mut w = World::new();
    w.register(1);
    let run = w.create_run(oid(11), "refs/heads/a", 5, vec![spec("build", &[])]);
    w.run_for(2_000);
    let job = w.fold().run(run).unwrap().jobs[0];
    let until = |w: &World| match w.fold().job(job).unwrap().state {
        JobState::InProgress { lease_until, .. } => lease_until.0,
        ref s => panic!("{s:?}"),
    };
    let granted = until(&w);
    let mut n = 0;
    // One stall of `ms` on the shard: a runner registration whose AppendEntries are lost, re-sent at the leader's tick
    // `ms` later, then a second registration appended as the stall ends.
    let mut stall = |w: &mut World, ms: u64| {
        let l = w.shard.leader().unwrap();
        let mut put = |w: &mut World| {
            n += 1;
            let rec = RunnerRecord {
                scope: RunnerScope::Repo(REPO),
                name: format!("stall{n}").into(),
                labels: LabelSet::parse(["linux"]).unwrap(),
                key: Box::new([1]),
                ephemeral: false,
                hosted: false,
                version: "2.338.2".into(),
            };
            let (now, rid) = (w.now, w.rid());
            w.shard.on(l, now, |m, t| m.commit(t, RequestId(Ulid(rid)), op(ActionsOp::PutRunner(rec)))).unwrap();
        };
        put(w);
        w.shard.inbox.clear();
        w.now = Timestamp(w.now.0 + ms);
        w.shard.tick(w.now);
        put(w);
        w.shard.deliver(w.now);
        w.shard.m(l).log().last().unwrap().extend
    };
    for _ in 0..3 {
        let x = stall(&mut w, 1_500);
        assert_eq!(until(&w), granted, "a sub-floor stall moved the job lease");
        assert_eq!(x, None, "a 1.5 s stall stamps no extend");
    }
    let x = stall(&mut w, 3_000).expect("a 3 s stall stamps an extend");
    assert_eq!(x.by.as_millis(), 3_000, "{x:?}");
    assert_eq!(until(&w), granted + 3_000, "moved by the 3 s stall alone");
}

// ---- repo-gone ----

/// `repo-gone`: one repository on a shard is purged, so no group answers its `RunChecksCmd`, and the next
/// repository's job runs on the same shard. While the owner's `Authz` still holds the purged repository's name, its
/// failure is an outage: the cursor stays and its run is not cancelled. Once the name is `Deleted`, `RunOwner`
/// cancels its run, moves past it, and the next repository's `workflow_job` steps reach the App. On main, the round
/// retried the purged repository forever and held every later repository's checks and steps behind it (graft-m4a,
/// `actions:7100001.0`, 2026-11-19, 01:56Z to 11:40Z and on).
#[test]
fn a_purged_repository_does_not_hold_the_next_repositorys_workflow_jobs() {
    use graftyard_git::sequencer::{app_targets, plan};
    use graftyard_git::workflow_job::{JobAction, WorkflowJobUpdate};
    use graftyard_identity::{
        Access, AppId, InstallationId, InstallationRow, InstallationState, PermissionSet, RepoEntry, RepoName,
        RepoScope,
    };
    let mut w = World::new();
    w.app_hooks = true;
    w.plans.insert("build".into(), Plan { ms: 20_000, ends: Conclusion::Success });
    w.plans.insert("lint".into(), Plan { ms: 10_000, ends: Conclusion::Success });
    w.register(2);
    // `REPO`'s run is in progress when its members are purged.
    let old = w.create_run(oid(11), "refs/heads/a", 2, vec![spec("build", &[]), spec("test", &[0])]);
    w.run_for(3_000);
    w.purged.insert(REPO);
    // Its build finishes after the purge: a round now owes the purged repository that conclusion.
    w.run_for(30_000);
    let stuck = w.fold().owner_cursor;
    let next = w.create_run_in(REPO2, oid(21), "refs/heads/main", 2, vec![spec("lint", &[])]);
    w.run_for(2 * MIN);
    let f = w.fold();
    assert!(!w.gone.contains(&REPO), "a repository its owner still names is down, not gone");
    assert!(f.run_jobs(old).iter().all(|j| j.state.completed() != Some(Conclusion::Cancelled)));
    assert_eq!(f.owner_cursor, stuck, "the cursor holds below the purged repository's unsent conclusion");
    let lint_steps = |w: &World| {
        w.repo2
            .committed_after(Index(0))
            .into_iter()
            .flat_map(|(_, e)| e)
            .any(|e| matches!(e, RepoEvent::WorkflowJob(_)))
    };
    assert!(!lint_steps(&w), "held behind the purged repository while its owner still names it");

    // The delete's commit point: the owner's name for `REPO` is `Deleted`.
    let key = RepoName::parse("a").unwrap().key();
    w.authz.names.insert(key, RepoEntry::Deleted { repo: REPO, retain_until: Timestamp(w.now.0 + 86_400_000) });
    w.run_for(MIN);
    let f = w.fold();
    assert!(w.gone.contains(&REPO));
    assert!(f.run_jobs(old).iter().all(|j| j.state.completed().is_some()), "{:?}", f.run_jobs(old));
    assert!(f.run_jobs(old).iter().any(|j| j.state.completed() == Some(Conclusion::Cancelled)));
    assert_eq!(f.run(next).unwrap().status, RunStatus::Completed(Conclusion::Success));
    // Only `RunOwner`'s own entries are left past the cursor, which start no round.
    let left = w.shard.committed_after(f.owner_cursor);
    assert!(left.iter().flat_map(|(_, e)| e).all(|e| matches!(e, ActionsEvent::OwnerCursor { .. })), "{left:?}");

    // The next repository's steps reach an App installed on the owner.
    let reads = PermissionSet(
        [("actions", Access::Read), ("metadata", Access::Read)].iter().map(|(k, a)| (Box::from(*k), *a)).collect(),
    );
    let row = InstallationRow { app: AppId(DbId(51)), repos: RepoScope::All, permissions: reads };
    let installations = [(InstallationId(DbId(901)), InstallationState { row: Some(row), not_before: Timestamp(0) })]
        .into_iter()
        .collect();
    let records = [app(51, &["workflow_job"])];
    let targets = app_targets(&installations, REPO2, |a| records.iter().find(|r| r.id == a));
    let l = w.repo2.leader().unwrap();
    let core = &w.repo2.fold().core;
    let render = |wh: &graftyard_git::hooks::Webhook, _: Option<InstallationId>| match &wh.subject {
        graftyard_git::hooks::Subject::WorkflowJob(u) => serde_json::to_vec(u).ok(),
        _ => None,
    };
    let mut got = Vec::new();
    for e in w.repo2.m(l).log() {
        for r in plan(core, REPO2, 1, e, &targets, render) {
            let u: WorkflowJobUpdate = serde_json::from_slice(&r.body).unwrap();
            got.push((u.run, u.action));
        }
    }
    use JobAction::{Completed, InProgress, Queued};
    assert_eq!(got, [(next.0, Queued), (next.0, InProgress), (next.0, Completed)]);
}
