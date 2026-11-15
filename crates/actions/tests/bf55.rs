//! BF-55 in memory (`build/BUILD-PLAN.md`, M4-WP3): kill the leader site of an `Owner` scope's registration shard
//! during a 256-job matrix. Pass: polls reach a broker on the new leader site within 15 s; every job runs once; zero
//! jobs assigned twice. Fail: a job assigned twice, or dispatch stalled over 60 s.
//!
//! The owner's registration shard (`OWNER_SCOPE_SHARD`) and the two shards the matrix's repositories hash to are each
//! three real `graftyard_raft::Member<ActionsCmd>`s. Runners register `Owner` scope on shard 0, so no other shard holds
//! their records. Each poll goes the way the hosted broker's does (`workers/site/src/broker.rs`): the registration
//! shard's leader must answer (its site hosts the broker), the poll picks from every shard's summary with
//! `graftyard_actions::broker::pick`, and the chosen shard's leader commits `MatchOwner`. The kill crashes site 0, where
//! every group's leader is, and drops one match's answer with it, as a broker lost with its site loses a poll it held.

mod common;

use std::collections::BTreeMap;

use common::{Group, SITES};
use graftyard_actions::broker::{Pick, RESEND_AFTER, Summary, pick};
use graftyard_actions::shard::{
    ACTIONS_FORMAT, ACTIONS_FORMAT_OWNER_MATCH, ActionsActor, ActionsCmd, ActionsErr, ActionsEvent, ActionsFold,
    ActionsOp, ActionsOut, JobSpec, RunnerRecord, RunnerScope, WorkflowRun,
};
use graftyard_actions::{
    Conclusion, JobEvent, JobId, JobState, LabelSet, OWNER_SCOPE_SHARD, RunId, RunnerId, actions_shard,
};
use graftyard_raft::CommitError;
use graftyard_types::{AccountId, ChainId, DbId, IdBlock, Index, RepoId, SiteId, Timestamp};
use smallvec::SmallVec;

const OWNER: AccountId = AccountId(DbId(100));
const STEP: u64 = 500;
const RUNNERS: usize = 24;
const JOB_MS: u64 = 4_000;

fn cmd(actor: ActionsActor, op: ActionsOp) -> ActionsCmd {
    ActionsCmd { actor, op }
}

/// Two repositories of the owner on two different shards, neither the registration shard.
fn repos() -> [(RepoId, u8); 2] {
    let mut out = Vec::new();
    for n in 1..1_000 {
        let r = RepoId(DbId(n));
        let s = actions_shard(r);
        if s != OWNER_SCOPE_SHARD && out.iter().all(|(_, x)| *x != s) {
            out.push((r, s));
        }
        if out.len() == 2 {
            break;
        }
    }
    [out[0], out[1]]
}

struct Runner {
    id: RunnerId,
    /// The job this runner took: (shard, job, finishes at).
    job: Option<(u8, JobId, Timestamp)>,
}

struct World {
    now: Timestamp,
    next: u128,
    shards: BTreeMap<u8, Group<ActionsCmd>>,
    runners: Vec<Runner>,
    labels: LabelSet,
    /// Every `Matched` committed, by (shard, job): the runners.
    matched: BTreeMap<(u8, JobId), Vec<RunnerId>>,
    /// Every `Acquired` the runners sent that committed, by (shard, job).
    acquired: BTreeMap<(u8, JobId), u32>,
    /// Drop the next match's answer (a broker lost with its site after the leader committed).
    lose_next_answer: bool,
    /// Polls that reached a broker (a registration-shard leader), with the site.
    polls_served: Vec<(Timestamp, SiteId)>,
    /// When a job was last matched or acquired.
    last_dispatch: Timestamp,
    summaries: BTreeMap<u8, (Timestamp, Summary)>,
}

impl World {
    fn new() -> World {
        let now = Timestamp(1_000_000);
        let reads = vec![ACTIONS_FORMAT, ACTIONS_FORMAT_OWNER_MATCH];
        let mut shards = BTreeMap::new();
        let mut shard_ids = vec![OWNER_SCOPE_SHARD];
        shard_ids.extend(repos().map(|(_, s)| s));
        for (i, s) in shard_ids.into_iter().enumerate() {
            let mut g = Group::new(ChainId::Actions(OWNER, s), reads.clone(), ActionsFold::default(), now);
            g.id_block(now, IdBlock::whole(0, 10 + i as u32).unwrap());
            shards.insert(s, g);
        }
        World {
            now,
            next: 1,
            shards,
            runners: Vec::new(),
            labels: LabelSet::parse(["self-hosted", "linux", "x64"]).unwrap(),
            matched: BTreeMap::new(),
            acquired: BTreeMap::new(),
            lose_next_answer: false,
            polls_served: Vec::new(),
            last_dispatch: now,
            summaries: BTreeMap::new(),
        }
    }

    fn rid(&mut self) -> u128 {
        self.next += 1;
        self.next << 32
    }

    fn commit(&mut self, shard: u8, c: ActionsCmd) -> Option<Result<ActionsOut, ActionsErr>> {
        let (now, id) = (self.now, self.rid());
        match self.shards.get_mut(&shard).unwrap().commit(now, id, c)? {
            Ok(a) => Some(Ok(a.out)),
            Err(CommitError::Refused { err, .. }) => Some(Err(err)),
            Err(_) => None,
        }
    }

    fn register(&mut self) {
        for i in 0..RUNNERS {
            let rec = RunnerRecord {
                scope: RunnerScope::Owner,
                name: format!("bf55-{i}").into(),
                labels: self.labels.clone(),
                key: Box::new([1]),
                ephemeral: false,
                hosted: false,
                version: "2.338.2".into(),
            };
            let Some(Ok(ActionsOut::Runner(id))) =
                self.commit(OWNER_SCOPE_SHARD, cmd(ActionsActor::Operator, ActionsOp::PutRunner(rec)))
            else {
                panic!("register")
            };
            self.runners.push(Runner { id, job: None });
        }
    }

    /// The matrix: `per` jobs in each repository's run.
    fn create_runs(&mut self, per: usize) -> Vec<(u8, RunId)> {
        let mut out = Vec::new();
        for (i, (repo, shard)) in repos().into_iter().enumerate() {
            let jobs = (0..per)
                .map(|n| JobSpec {
                    id: JobId(DbId(0)),
                    name: format!("build ({n})").into(),
                    labels: LabelSet::parse(["self-hosted", "linux"]).unwrap(),
                    needs: SmallVec::new(),
                    matrix: SmallVec::new(),
                    environment: None,
                    steps: "[]".into(),
                })
                .collect();
            let run = WorkflowRun {
                id: RunId(DbId(0)),
                repo,
                workflow: ".github/workflows/bf55.yml".into(),
                event: "workflow_dispatch".into(),
                head_sha: [i as u8; 20],
                head_ref: "refs/heads/main".into(),
                trigger: (ChainId::Repo(repo), Index(2)),
                attempt: 1,
                concurrency: None,
                fork: false,
            };
            let Some(Ok(ActionsOut::Run(id))) =
                self.commit(shard, cmd(ActionsActor::Operator, ActionsOp::CreateRun { run, jobs }))
            else {
                panic!("create run")
            };
            out.push((shard, id));
        }
        out
    }

    /// The broker's summary of `shard`, read again once a second, as `SUMMARY_FRESH_MS`.
    fn summary(&mut self, shard: u8) -> Option<Summary> {
        let fresh = self.summaries.get(&shard).filter(|(at, _)| self.now.0 < at.0 + 1_000).map(|(_, s)| s.clone());
        if fresh.is_some() {
            return fresh;
        }
        let g = &self.shards[&shard];
        g.leader()?;
        let s = g.fold().summary();
        self.summaries.insert(shard, (self.now, s.clone()));
        Some(s)
    }

    /// One poll by runner `i`, as the hosted broker serves it.
    fn poll(&mut self, i: usize) {
        let Some(broker) = self.shards[&OWNER_SCOPE_SHARD].leader() else { return };
        // The registration shard holds the runner's record; it is what the broker's first ask reads.
        let id = self.runners[i].id;
        assert!(self.shards[&OWNER_SCOPE_SHARD].fold().runners.contains_key(&id), "the session outlived the kill");
        self.polls_served.push((self.now, broker));
        let shards: Vec<u8> = self.shards.keys().copied().collect();
        let sums: Vec<(u8, Summary)> = shards.iter().filter_map(|s| self.summary(*s).map(|x| (*s, x))).collect();
        let refs: Vec<(u8, &Summary)> = sums.iter().map(|(s, x)| (*s, x)).collect();
        let labels = self.labels.clone();
        let (shard, job) = match pick(&refs, id, &labels, self.now) {
            Pick::Busy | Pick::Idle => return,
            Pick::Resend { shard, job } => (shard, job),
            Pick::Match { shard, job, version } => {
                let op = ActionsOp::MatchOwner { job, version, runner: id, labels };
                let r = self.commit(shard, cmd(ActionsActor::Operator, op));
                self.summaries.remove(&shard);
                match r {
                    Some(Ok(_)) => {}
                    _ => return,
                }
                self.matched.entry((shard, job)).or_default().push(id);
                self.last_dispatch = self.now;
                if std::mem::take(&mut self.lose_next_answer) {
                    return;
                }
                (shard, job)
            }
        };
        // The runner's `acquirejob` on the job's shard.
        let Some(v) = self.shards[&shard].leader().map(|_| self.shards[&shard].fold().job(job).unwrap().version) else {
            return;
        };
        let a = cmd(ActionsActor::Runner(id), ActionsOp::Job { job, version: v, event: JobEvent::Acquired });
        if let Some(Ok(_)) = self.commit(shard, a) {
            *self.acquired.entry((shard, job)).or_default() += 1;
            self.last_dispatch = self.now;
            self.runners[i].job = Some((shard, job, Timestamp(self.now.0 + JOB_MS)));
        }
    }

    fn step(&mut self) {
        self.now = Timestamp(self.now.0 + STEP);
        let now = self.now;
        for g in self.shards.values_mut() {
            g.tick(now);
        }
        // The leader's lease alarm on each shard.
        let due: Vec<u8> = self
            .shards
            .iter()
            .filter(|(_, g)| g.leader().is_some() && g.fold().next_deadline().is_some_and(|d| d <= now))
            .map(|(s, _)| *s)
            .collect();
        for s in due {
            self.commit(s, cmd(ActionsActor::Operator, ActionsOp::ExpireLeases));
        }
        for i in 0..self.runners.len() {
            if let Some((shard, job, until)) = self.runners[i].job {
                if self.now < until {
                    continue;
                }
                let Some(v) =
                    self.shards[&shard].leader().map(|_| self.shards[&shard].fold().job(job).unwrap().version)
                else {
                    continue;
                };
                let f = ActionsOp::Job { job, version: v, event: JobEvent::Finished(Conclusion::Success) };
                if matches!(self.commit(shard, cmd(ActionsActor::Job(job), f)), Some(Ok(_))) {
                    self.runners[i].job = None;
                }
                continue;
            }
            self.poll(i);
        }
    }

    fn all_done(&self, runs: &[(u8, RunId)]) -> bool {
        runs.iter().all(|(s, r)| {
            let f = self.shards[s].fold();
            f.run_jobs(*r).iter().all(|j| matches!(j.state, JobState::Completed { .. }))
        })
    }
}

#[test]
fn bf55_killing_the_registration_shards_leader_site_mid_matrix() {
    let mut w = World::new();
    w.register();
    // Shard 0 holds every runner; the repositories' shards hold none.
    for (_, s) in repos() {
        assert!(w.shards[&s].fold().runners.is_empty());
    }
    let runs = w.create_runs(128);
    let total: usize = runs.iter().map(|(s, r)| w.shards[s].fold().run_jobs(*r).len()).sum();
    assert_eq!(total, 256);
    // Run until 80 jobs finished, then lose one match's answer and kill site 0, where every group leads.
    let done = |w: &World| -> usize {
        runs.iter()
            .map(|(s, r)| {
                w.shards[s].fold().run_jobs(*r).iter().filter(|j| matches!(j.state, JobState::Completed { .. })).count()
            })
            .sum()
    };
    while done(&w) < 80 {
        w.step();
    }
    for g in w.shards.values() {
        assert_eq!(g.leader(), Some(SITES[0]), "every group leads on site 0 before the kill");
    }
    // A match whose answer is lost with the broker's site: committed, never delivered.
    w.lose_next_answer = true;
    while w.lose_next_answer {
        w.step();
    }
    let lost: Vec<(u8, JobId)> =
        w.matched.iter().filter(|(k, _)| !w.acquired.contains_key(k)).map(|(k, _)| *k).collect();
    assert_eq!(lost.len(), 1, "one match without its answer");
    let kill_at = w.now;
    for g in w.shards.values_mut() {
        g.crash(SITES[0]);
    }
    let mut first_new_poll = None;
    let mut longest_stall = 0;
    let mut last = kill_at;
    for _ in 0..(20 * 60_000) / STEP {
        w.step();
        if first_new_poll.is_none()
            && let Some((t, s)) = w.polls_served.iter().rev().find(|(t, _)| *t > kill_at)
        {
            assert_ne!(*s, SITES[0]);
            first_new_poll = Some(*t);
        }
        if w.last_dispatch > last {
            longest_stall = longest_stall.max(w.last_dispatch.0 - last.0);
            last = w.last_dispatch;
        }
        if w.all_done(&runs) {
            break;
        }
    }
    assert!(w.all_done(&runs), "the matrix did not finish");
    let first = first_new_poll.expect("no poll reached a broker after the kill");
    // Polls reach a broker on the new leader site within 15 s.
    assert!(
        first.0 - kill_at.0 <= 15_000,
        "first poll on the new leader site {} ms after the kill",
        first.0 - kill_at.0
    );
    // Dispatch never stalled over 60 s, including the lost answer's job, which waits RESEND_AFTER, not its lease.
    assert!(longest_stall <= 60_000, "dispatch stalled {longest_stall} ms");
    let (ls, lj) = lost[0];
    assert_eq!(w.acquired.get(&(ls, lj)), Some(&1), "the lost match's job ran once, resent to its runner");
    assert!(RESEND_AFTER.as_millis() < 60_000);
    // Every job ran once and no job was assigned twice.
    for (s, r) in &runs {
        for j in w.shards[s].fold().run_jobs(*r) {
            let k = (*s, j.spec.id);
            assert_eq!(w.matched.get(&k).map(Vec::len), Some(1), "job {k:?} matched {:?}", w.matched.get(&k));
            assert_eq!(w.acquired.get(&k), Some(&1), "job {k:?} acquired {:?}", w.acquired.get(&k));
            assert_eq!(j.state.completed(), Some(Conclusion::Success));
        }
    }
    // The committed log agrees: one `Matched` per job on each shard, all on the shards that hold the jobs.
    for (s, _) in &runs {
        let matched = w.shards[s]
            .committed_after(Index(0))
            .into_iter()
            .flat_map(|(_, e)| e)
            .filter(|e| matches!(e, ActionsEvent::Job { event: JobEvent::Matched(_), .. }))
            .count();
        assert_eq!(matched, 128, "shard {s}");
    }
}

/// `MatchOwner` is an operator's, checks the runner's labels it carries, and commits only at the job's version.
#[test]
fn match_owner_refuses_a_non_operator_wrong_labels_and_a_stale_version() {
    let mut w = World::new();
    let runs = w.create_runs(1);
    let (s, r) = runs[0];
    let job = w.shards[&s].fold().run_jobs(r)[0].spec.id;
    let runner = RunnerId(DbId(77));
    let ok = LabelSet::parse(["self-hosted", "linux"]).unwrap();
    let m = |labels: &LabelSet, version| ActionsOp::MatchOwner { job, version, runner, labels: labels.clone() };
    assert_eq!(w.commit(s, cmd(ActionsActor::Runner(runner), m(&ok, 0))), Some(Err(ActionsErr::Forbidden)));
    let gpu = LabelSet::parse(["gpu"]).unwrap();
    assert!(matches!(w.commit(s, cmd(ActionsActor::Operator, m(&gpu, 0))), Some(Err(ActionsErr::Illegal(_)))));
    assert_eq!(w.commit(s, cmd(ActionsActor::Operator, m(&ok, 1))), Some(Err(ActionsErr::Stale(0))));
    assert!(matches!(w.commit(s, cmd(ActionsActor::Operator, m(&ok, 0))), Some(Ok(ActionsOut::Job(_)))));
    assert_eq!(w.shards[&s].fold().job(job).unwrap().state.runner(), Some(runner));
    // A second match of the assigned job, at its new version, is refused: no job is assigned twice.
    assert!(matches!(w.commit(s, cmd(ActionsActor::Operator, m(&ok, 1))), Some(Err(ActionsErr::Illegal(_)))));
}
