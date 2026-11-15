//! One owner's Actions shard (`ChainId::Actions(owner, shard)`): DESIGN's `ActionsEvent` and the fold and commands
//! that write it (`tests/m4/results/m4-wp3-design.md`, "The Actions chain").
//!
//! - **Runs.** `CreateRun` is the "Workflow run" commit point: one `RunCreated` carries the run and every `JobSpec`,
//!   refused for a second `(trigger chain, index, workflow path)` (`RunKey`). Re-runs are not built yet.
//! - **Jobs.** `Job { job, version, event }` applies `transition` against the job's current version, so of two
//!   runners matched to one job only the first commits (SHARED, One runner at a time). The fold keeps each job's
//!   version, which every committed `Job` event bumps.
//! - **Leases.** `ExpireLeases` commits `LeaseExpired` or `QueueExpired` for every job past its deadline at the
//!   entry's time, which the leader's alarm sends. A stall moves every lease and queue time by its length
//!   (`ExtendLeases`), so idle time never keeps a dead runner's job alive and a stalled group fails none.
//! - **Runners and sessions.** Registration writes a `RunnerRecord`; a session opened by a runner's `POST session` is a
//!   committed entry, so any broker accepts it.
//! - **Status.** Only `RunOwner` sends `SetRunStatus`, and only forward: a run never leaves `Completed`. `RunOwner`
//!   commits its cursor here (`OwnerCursor`) after its repository writes (`crate::run_owner`).
//!
//! Ids come from the group's leased block, as `Names` assigns its account ids.

use std::collections::BTreeMap;

use graftyard_raft::{Command, EntryBody, ExtendLeases, Fold, Format, LogEntry, Record, Scratch, TierSet};
use graftyard_types::{AccountId, ChainId, DbId, IdBlock, Index, RepoId, RequestId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::ids::{JobId, RunId, RunnerId, RunnerSessionId};
use crate::job::{Conclusion, JobEvent, JobState, RunStatus, derive_run_status, transition};
use crate::labels::LabelSet;

/// The Actions family's first format (`tests/m4/results/m4-wp3-design.md`, "Formats and tags").
pub const ACTIONS_FORMAT: Format = Format { family: 0x15, version: 1 };
/// BF-37's `CreateRun` of a `merge_group` run, whose run key names its queue ref (`WorkflowRun::key`). Only
/// `ActionsTrigger` on a repository leader of a release that reads it sends one; every other command stays 0x15.1.
pub const ACTIONS_FORMAT_MERGE_GROUP: Format = Format { family: 0x15, version: 2 };
/// BF-55's `MatchOwner`, which only a broker of a release that reads it sends; its event is the 0x15.1 `Job` event.
pub const ACTIONS_FORMAT_OWNER_MATCH: Format = Format { family: 0x15, version: 3 };

#[cfg(test)]
const JOB_LEASE_MS: u64 = crate::job::JOB_LEASE.as_millis() as u64;

/// Jobs a run holds at most: GitHub's matrix limit [doc: "A matrix can generate a maximum of 256 jobs per workflow
/// run"], which SHARED's one-entry run fits.
pub const RUN_JOBS_MAX: usize = 256;
/// Runners per `Repo` or `Owner` scope [guess].
pub const RUNNERS_PER_SCOPE_MAX: usize = 10_000;

/// The trigger a run is created from at most once: the repository chain's entry and the workflow file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RunKey {
    pub chain: ChainId,
    pub index: Index,
    pub workflow: Box<str>,
}

/// SHARED's `WorkflowRun`. `workflow` is the workflow file's path; `head_sha` and `head_ref` the commit and ref it
/// runs on (for `merge_group`, the queue ref).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: RunId,
    pub repo: RepoId,
    pub workflow: Box<str>,
    pub event: Box<str>,
    pub head_sha: [u8; 20],
    pub head_ref: Box<str>,
    pub trigger: (ChainId, Index),
    pub attempt: u16,
    pub concurrency: Option<Box<str>>,
    /// A pull request from a fork reads no secrets (SHARED).
    pub fork: bool,
}

impl WorkflowRun {
    /// SHARED's `(chain, index, workflow path)`. One `GroupBuilt` entry builds several queue refs, each its own
    /// `merge_group` event, so a `merge_group` run's key also names its queue ref (after a newline, which no path
    /// holds). Only format 0x15.2 carries such a run, so a member that keys runs the old way never decides one.
    pub fn key(&self) -> RunKey {
        let workflow = if self.is_merge_group() {
            format!("{}\n{}", self.workflow, self.head_ref).into()
        } else {
            self.workflow.clone()
        };
        RunKey { chain: self.trigger.0, index: self.trigger.1, workflow }
    }

    pub fn is_merge_group(&self) -> bool {
        &*self.event == "merge_group"
    }
}

/// One matrix value: a string, number, or bool as the workflow wrote it, kept as its text [inference: the job
/// message renders every matrix value as context data, and the runner reads them as strings or JSON scalars].
pub type MatrixValue = Box<str>;

/// SHARED's `JobSpec`. `steps` is the job's steps as the parser gave them, JSON, rendered into the job message at
/// `acquirejob` [inference: SHARED names no step type; the run-service message is built from it].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSpec {
    pub id: JobId,
    pub name: Box<str>,
    pub labels: LabelSet,
    pub needs: SmallVec<[JobId; 4]>,
    pub matrix: SmallVec<[(Box<str>, MatrixValue); 4]>,
    pub environment: Option<Box<str>>,
    pub steps: Box<str>,
}

/// SHARED's `RunnerScope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RunnerScope {
    Owner,
    Repo(RepoId),
}

/// SHARED's `RunnerRecord`. `key` is the runner's RSA public key (DER), which verifies its OAuth assertions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerRecord {
    pub scope: RunnerScope,
    pub name: Box<str>,
    pub labels: LabelSet,
    pub key: Box<[u8]>,
    pub ephemeral: bool,
    pub hosted: bool,
    pub version: Box<str>,
}

/// DESIGN's `ActionsEvent`. Wire tags: postcard stores and sends the variants by position
/// (`crates/git/tests/wire_tags.golden`). Add a variant only at the end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionsEvent {
    RunCreated {
        run: WorkflowRun,
        jobs: Vec<JobSpec>,
    },
    RunStatus {
        run: RunId,
        status: RunStatus,
    },
    Job {
        job: JobId,
        version: u64,
        event: JobEvent,
    },
    Runner {
        id: RunnerId,
        record: Option<RunnerRecord>,
    },
    Session {
        runner: RunnerId,
        session: Option<RunnerSessionId>,
    },
    /// `RunOwner`'s cursor on the shard it reads, committed after its repository writes (SHARED, \"Other consumers\").
    OwnerCursor {
        through: Index,
    },
}

impl Record for ActionsEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        canon(out, self);
    }
}

fn canon<T: Serialize>(out: &mut Vec<u8>, v: &T) {
    graftyard_fold::canon::write(out, v);
}

/// A job as the fold holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRow {
    pub run: RunId,
    pub spec: JobSpec,
    pub state: JobState,
    pub version: u64,
}

/// A run as the fold holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRow {
    pub run: WorkflowRun,
    pub jobs: Vec<JobId>,
    pub status: RunStatus,
}

/// The shard's fold.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionsFold {
    pub runs: BTreeMap<RunId, RunRow>,
    /// The run each trigger created, which `CreateRun` refuses a second of.
    pub keys: BTreeMap<RunKey, RunId>,
    pub jobs: BTreeMap<JobId, JobRow>,
    pub runners: BTreeMap<RunnerId, RunnerRecord>,
    pub sessions: BTreeMap<RunnerId, RunnerSessionId>,
    /// Counts every commit that moves a job into or out of `Queued`: the broker's hint count (SHARED).
    pub queue_moves: u64,
    /// The last shard entry `RunOwner` finished: its repository writes acknowledged or refused.
    pub owner_cursor: Index,
    pub ids: IdCursor,
}

/// The group's leased id block and its next free position, as `graftyard_identity::IdCursor`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdCursor {
    block: Option<IdBlock>,
    next: u32,
}

impl IdCursor {
    /// The first of `n` free ids in a row, if the block holds them.
    pub fn peek_n(&self, n: u32) -> Option<DbId> {
        let b = self.block?;
        (self.next + n <= b.len()).then(|| b.id(self.next)).flatten()
    }
    fn apply_block(&mut self, b: IdBlock) {
        if self.block.is_none_or(|cur| cur.followed_by(b)) {
            *self = IdCursor { block: Some(b), next: 0 };
        }
    }
    fn assigned(&mut self, id: DbId) {
        if let Some(i) = self.block.and_then(|b| b.index_of(id))
            && i >= self.next
        {
            self.next = i + 1;
        }
    }
}

/// What `apply` reads and writes: the fold itself, or `decide`'s overlay on it.
trait Tables {
    fn job(&self, j: JobId) -> Option<&JobRow>;
    fn put_job(&mut self, j: JobId, row: JobRow);
    fn run(&self, r: RunId) -> Option<&RunRow>;
    fn put_run(&mut self, r: RunId, row: RunRow, key: Option<RunKey>);
    fn set_runner(&mut self, id: RunnerId, r: Option<RunnerRecord>);
    fn set_session(&mut self, id: RunnerId, s: Option<RunnerSessionId>);
    fn ids(&mut self) -> &mut IdCursor;
    fn queue_moved(&mut self);
    fn set_owner_cursor(&mut self, through: Index);
}

fn is_queued(s: &JobState) -> bool {
    matches!(s, JobState::Queued { .. })
}

fn apply_event(t: &mut impl Tables, ev: &ActionsEvent, at: Timestamp) {
    match ev {
        ActionsEvent::RunCreated { run, jobs } => {
            t.ids().assigned(run.id.0);
            for s in jobs {
                t.ids().assigned(s.id.0);
                let state = if s.needs.is_empty() {
                    t.queue_moved();
                    JobState::Queued { labels: s.labels.clone(), since: at }
                } else {
                    JobState::Pending { needs: s.needs.clone() }
                };
                t.put_job(s.id, JobRow { run: run.id, spec: s.clone(), state, version: 0 });
            }
            let ids = jobs.iter().map(|s| s.id).collect();
            t.put_run(run.id, RunRow { run: run.clone(), jobs: ids, status: RunStatus::Queued }, Some(run.key()));
        }
        ActionsEvent::RunStatus { run, status } => {
            if let Some(r) = t.run(*run) {
                let r = RunRow { status: *status, ..r.clone() };
                t.put_run(*run, r, None);
            }
        }
        ActionsEvent::Job { job, event, .. } => {
            let Some(j) = t.job(*job) else { return };
            if let Ok(next) = transition(&j.state, event, &j.spec.labels, at)
                && next != j.state
            {
                let moved = is_queued(&j.state) != is_queued(&next);
                let row = JobRow { state: next, version: j.version + 1, ..j.clone() };
                t.put_job(*job, row);
                if moved {
                    t.queue_moved();
                }
            }
        }
        ActionsEvent::Runner { id, record } => {
            t.ids().assigned(id.0);
            if record.is_none() {
                t.set_session(*id, None);
            }
            t.set_runner(*id, record.clone());
        }
        ActionsEvent::Session { runner, session } => t.set_session(*runner, *session),
        ActionsEvent::OwnerCursor { through } => t.set_owner_cursor(*through),
    }
}

/// Every lease and queue time moved by a recorded stall: `Assigned` and `InProgress` leases that had not ended by its
/// start, and every queued job's `since`.
fn extended(s: &JobState, x: ExtendLeases) -> Option<JobState> {
    let by = x.by.as_millis() as u64;
    let mut s = s.clone();
    match &mut s {
        JobState::Assigned { lease_until, .. } | JobState::InProgress { lease_until, .. }
            if *lease_until >= x.since =>
        {
            lease_until.0 += by
        }
        JobState::Queued { since, .. } => since.0 += by,
        _ => return None,
    }
    Some(s)
}

impl Tables for ActionsFold {
    fn job(&self, j: JobId) -> Option<&JobRow> {
        self.jobs.get(&j)
    }
    fn put_job(&mut self, j: JobId, row: JobRow) {
        self.jobs.insert(j, row);
    }
    fn run(&self, r: RunId) -> Option<&RunRow> {
        self.runs.get(&r)
    }
    fn put_run(&mut self, r: RunId, row: RunRow, key: Option<RunKey>) {
        if let Some(k) = key {
            self.keys.insert(k, r);
        }
        self.runs.insert(r, row);
    }
    fn set_runner(&mut self, id: RunnerId, r: Option<RunnerRecord>) {
        match r {
            Some(r) => self.runners.insert(id, r),
            None => self.runners.remove(&id),
        };
    }
    fn set_session(&mut self, id: RunnerId, s: Option<RunnerSessionId>) {
        match s {
            Some(s) => self.sessions.insert(id, s),
            None => self.sessions.remove(&id),
        };
    }
    fn ids(&mut self) -> &mut IdCursor {
        &mut self.ids
    }
    fn queue_moved(&mut self) {
        self.queue_moves += 1;
    }
    fn set_owner_cursor(&mut self, through: Index) {
        self.owner_cursor = through;
    }
}

impl ActionsFold {
    pub fn job(&self, j: JobId) -> Option<&JobRow> {
        self.jobs.get(&j)
    }
    pub fn run(&self, r: RunId) -> Option<&RunRow> {
        self.runs.get(&r)
    }
    /// The run's jobs in spec order.
    pub fn run_jobs(&self, r: RunId) -> Vec<&JobRow> {
        self.runs.get(&r).map_or_else(Vec::new, |row| row.jobs.iter().filter_map(|j| self.jobs.get(j)).collect())
    }
    /// The queued jobs, oldest first: the summary a broker loads (SHARED, Label matching).
    pub fn queued(&self) -> Vec<(JobId, &LabelSet, Timestamp, u64)> {
        let mut out: Vec<_> = self
            .jobs
            .iter()
            .filter_map(|(id, j)| match &j.state {
                JobState::Queued { labels, since } => Some((*id, labels, *since, j.version)),
                _ => None,
            })
            .collect();
        out.sort_by_key(|(id, _, since, _)| (*since, *id));
        out
    }
    /// The earliest lease end or queue deadline, where the leader's alarm goes.
    pub fn next_deadline(&self) -> Option<Timestamp> {
        self.jobs.values().filter_map(|j| j.state.deadline()).min()
    }
}

/// `decide`'s and `verify`'s view: the fold at `applied`, with the entry's earlier events in an overlay over it, so an
/// entry copies only the rows it writes.
pub struct ActionsScratch<'a> {
    base: &'a ActionsFold,
    at: Timestamp,
    jobs: BTreeMap<JobId, JobRow>,
    runs: BTreeMap<RunId, RunRow>,
    keys: BTreeMap<RunKey, RunId>,
    runners: BTreeMap<RunnerId, Option<RunnerRecord>>,
    sessions: BTreeMap<RunnerId, Option<RunnerSessionId>>,
    ids: IdCursor,
    owner_cursor: Index,
}

impl Tables for ActionsScratch<'_> {
    fn job(&self, j: JobId) -> Option<&JobRow> {
        self.jobs.get(&j).or_else(|| self.base.jobs.get(&j))
    }
    fn put_job(&mut self, j: JobId, row: JobRow) {
        self.jobs.insert(j, row);
    }
    fn run(&self, r: RunId) -> Option<&RunRow> {
        self.runs.get(&r).or_else(|| self.base.runs.get(&r))
    }
    fn put_run(&mut self, r: RunId, row: RunRow, key: Option<RunKey>) {
        if let Some(k) = key {
            self.keys.insert(k, r);
        }
        self.runs.insert(r, row);
    }
    fn set_runner(&mut self, id: RunnerId, r: Option<RunnerRecord>) {
        self.runners.insert(id, r);
    }
    fn set_session(&mut self, id: RunnerId, s: Option<RunnerSessionId>) {
        self.sessions.insert(id, s);
    }
    fn ids(&mut self) -> &mut IdCursor {
        &mut self.ids
    }
    fn queue_moved(&mut self) {}
    fn set_owner_cursor(&mut self, through: Index) {
        self.owner_cursor = through;
    }
}

impl<'a> ActionsScratch<'a> {
    fn new(base: &'a ActionsFold, at: Timestamp) -> ActionsScratch<'a> {
        ActionsScratch {
            base,
            at,
            jobs: BTreeMap::new(),
            runs: BTreeMap::new(),
            keys: BTreeMap::new(),
            runners: BTreeMap::new(),
            sessions: BTreeMap::new(),
            ids: base.ids,
            owner_cursor: base.owner_cursor,
        }
    }
    pub fn at(&self) -> Timestamp {
        self.at
    }
    pub fn job(&self, j: JobId) -> Option<&JobRow> {
        Tables::job(self, j)
    }
    pub fn run(&self, r: RunId) -> Option<&RunRow> {
        Tables::run(self, r)
    }
    pub fn runner(&self, id: RunnerId) -> Option<&RunnerRecord> {
        match self.runners.get(&id) {
            Some(r) => r.as_ref(),
            None => self.base.runners.get(&id),
        }
    }
    pub fn session(&self, id: RunnerId) -> Option<RunnerSessionId> {
        match self.sessions.get(&id) {
            Some(s) => *s,
            None => self.base.sessions.get(&id).copied(),
        }
    }
    pub fn has_key(&self, k: &RunKey) -> bool {
        self.keys.contains_key(k) || self.base.keys.contains_key(k)
    }
    pub fn owner_cursor(&self) -> Index {
        self.owner_cursor
    }
    /// Every job with its row, the overlay's over the fold's.
    pub fn jobs(&self) -> impl Iterator<Item = (JobId, &JobRow)> {
        let over = self.jobs.iter().map(|(k, v)| (*k, v));
        let base = self.base.jobs.iter().filter(|(k, _)| !self.jobs.contains_key(k)).map(|(k, v)| (*k, v));
        over.chain(base)
    }
    pub fn run_jobs(&self, r: RunId) -> Vec<&JobRow> {
        self.run(r).map_or_else(Vec::new, |row| row.jobs.iter().filter_map(|j| self.job(*j)).collect())
    }
    /// Every runner with its record, the overlay's over the fold's.
    pub fn runners(&self) -> impl Iterator<Item = (RunnerId, &RunnerRecord)> {
        let over = self.runners.iter().filter_map(|(k, v)| v.as_ref().map(|v| (*k, v)));
        let base = self.base.runners.iter().filter(|(k, _)| !self.runners.contains_key(k)).map(|(k, v)| (*k, v));
        over.chain(base)
    }
    pub fn runners_in(&self, scope: RunnerScope) -> usize {
        let over = self.runners.values().flatten().filter(|r| r.scope == scope).count();
        let base = self.base.runners.iter().filter(|(k, r)| r.scope == scope && !self.runners.contains_key(k)).count();
        over + base
    }
}

impl Scratch for ActionsScratch<'_> {
    type Event = ActionsEvent;
    fn apply(&mut self, ev: &ActionsEvent) {
        let at = self.at;
        apply_event(self, ev, at);
    }
    fn apply_id_block(&mut self, b: IdBlock) {
        self.ids.apply_block(b);
    }
    fn extend(&mut self, x: ExtendLeases) {
        let moved: Vec<(JobId, JobRow)> = self
            .jobs()
            .filter_map(|(id, j)| extended(&j.state, x).map(|state| (id, JobRow { state, ..j.clone() })))
            .collect();
        self.jobs.extend(moved);
    }
    /// A follower's check of each event against the running state: a `Job` event names the job's current version and
    /// is a legal transition at the entry's time, and a run is new.
    fn check(&self, ev: &ActionsEvent) -> Result<(), Box<str>> {
        match ev {
            ActionsEvent::Job { job, version, event } => {
                let j = self.job(*job).ok_or("no such job")?;
                if j.version != *version {
                    return Err(format!("job {} at version {}, not {version}", job.0.0, j.version).into());
                }
                transition(&j.state, event, &j.spec.labels, self.at).map(|_| ()).map_err(|e| e.to_string().into())
            }
            ActionsEvent::RunCreated { run, .. } if self.has_key(&run.key()) => {
                Err("a second run for one trigger".into())
            }
            _ => Ok(()),
        }
    }
}

impl Fold for ActionsFold {
    type Event = ActionsEvent;
    type Scratch<'a> = ActionsScratch<'a>;
    fn scratch(&self) -> ActionsScratch<'_> {
        ActionsScratch::new(self, Timestamp(0))
    }
    fn scratch_at(&self, at: Timestamp, _tiers: TierSet) -> ActionsScratch<'_> {
        ActionsScratch::new(self, at)
    }
    fn apply(&mut self, _ev: &ActionsEvent) {
        unreachable!("apply_entry applies each event at its entry's time")
    }
    fn apply_id_block(&mut self, b: IdBlock) {
        self.ids.apply_block(b);
    }
    fn apply_entry(&mut self, e: &LogEntry<ActionsEvent>) {
        if let Some(x) = e.extend {
            for j in self.jobs.values_mut() {
                if let Some(s) = extended(&j.state, x) {
                    j.state = s;
                }
            }
        }
        match &e.body {
            EntryBody::Events(evs) => {
                for ev in evs {
                    apply_event(self, ev, e.at);
                }
            }
            EntryBody::IdBlock(b) => self.ids.apply_block(*b),
            _ => {}
        }
    }
}

// ---- commands ----

/// Who sent a command. The edge authenticates a runner's OAuth token and a job token before it sends one
/// (`Credential::RunnerJwt`, `RuntimeToken`), and an admin's repository or org permission against `Authz`, so the shard
/// checks only that the runner or job named is the one the command acts for. `Operator` is a consumer (`ActionsTrigger`,
/// `RunOwner`, the leader's lease alarm) or `graft-ops`, as identity commands from a consumer carry no account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionsActor {
    Operator,
    Admin(AccountId),
    Runner(RunnerId),
    Job(JobId),
}

/// Wire tags as `ActionsEvent`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionsOp {
    /// `ActionsTrigger`'s commit point. Ids come from the group's block: the run's, then each job's in order. Each
    /// spec's `needs` names earlier jobs by their position in `jobs`, as `JobId(DbId(position))`.
    CreateRun {
        run: WorkflowRun,
        jobs: Vec<JobSpec>,
    },
    /// A runner's poll matched a queued job (the broker), the runner took it (`acquirejob`), renewed it, or finished it.
    /// `version` is the job version the sender read.
    Job {
        job: JobId,
        version: u64,
        event: JobEvent,
    },
    /// The leader's alarm: `LeaseExpired` or `QueueExpired` for every job past its deadline at the entry's time.
    ExpireLeases,
    PutRunner(RunnerRecord),
    DeleteRunner(RunnerId),
    OpenSession {
        runner: RunnerId,
        session: RunnerSessionId,
    },
    CloseSession {
        runner: RunnerId,
    },
    /// `RunOwner`'s status flip, from the run's jobs. A completed run's jobs take no event, so its status stays.
    SetRunStatus {
        run: RunId,
    },
    /// A user's or a concurrency group's cancel, and `RunOwner`'s abort: `Cancel` for every unfinished job.
    CancelRun {
        run: RunId,
    },
    /// `RunOwner`'s cursor, only forward.
    OwnerCursor {
        through: Index,
    },
    /// An `Owner`-scope runner's match on a shard other than its registration shard (BF-55): the owner's broker read
    /// the runner's record on `OWNER_SCOPE_SHARD` and sends its labels, since this shard holds no record of it. Only
    /// `Operator` (the broker's call to the leader) sends it; it commits the same `Matched(runner)` a local match does.
    MatchOwner {
        job: JobId,
        version: u64,
        runner: RunnerId,
        labels: LabelSet,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionsCmd {
    pub actor: ActionsActor,
    pub op: ActionsOp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionsOut {
    Run(RunId),
    Job(JobState),
    Runner(RunnerId),
    Done,
}

/// Wire tags as `ActionsEvent`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ActionsErr {
    #[error("not permitted")]
    Forbidden,
    #[error("no such run, job, or runner")]
    NotFound,
    #[error("a run for this trigger already exists")]
    Duplicate,
    #[error("the job is at version {0}")]
    Stale(u64),
    #[error("{0}")]
    Illegal(Box<str>),
    #[error("too many jobs or runners")]
    TooMany,
    #[error("the group needs an id block")]
    NeedIdBlock,
    #[error("a job's needs name a job that is not before it")]
    BadNeeds,
}

impl Record for ActionsCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        canon(out, self);
    }
}

type Decided = (SmallVec<[ActionsEvent; 4]>, ActionsOut);

fn none() -> Result<Decided, ActionsErr> {
    Ok((SmallVec::new(), ActionsOut::Done))
}

/// The `needs` the entry's completions release (SHARED, `JobSpec`): `NeedsMet` for a `Pending` job whose needs all
/// succeeded, `NeedsFailed` (which skips it) once one failed, was cancelled, or was skipped, as GitHub skips a job whose
/// need did not succeed [doc: "If a job fails or is skipped, all jobs that need it are skipped"]. `needs` name only
/// earlier jobs of the run (`CreateRun`), so one pass in spec order also skips a skipped job's dependents. `evs` are the
/// command's events so far; the released ones are appended.
fn release_needs(s: &ActionsScratch<'_>, evs: &mut SmallVec<[ActionsEvent; 4]>) {
    let mut done: BTreeMap<JobId, Conclusion> = BTreeMap::new();
    let mut runs = std::collections::BTreeSet::new();
    for ev in evs.iter() {
        if let ActionsEvent::Job { job, event, .. } = ev
            && let Some(j) = s.job(*job)
            && let Ok(JobState::Completed { conclusion, .. }) = transition(&j.state, event, &j.spec.labels, s.at())
        {
            done.insert(*job, conclusion);
            runs.insert(j.run);
        }
    }
    for run in runs {
        for j in s.run_jobs(run) {
            let JobState::Pending { needs } = &j.state else { continue };
            if done.contains_key(&j.spec.id) {
                continue;
            }
            let of = |n: &JobId| done.get(n).copied().or_else(|| s.job(*n).and_then(|x| x.state.completed()));
            let Some(cs) = needs.iter().map(of).collect::<Option<Vec<_>>>() else { continue };
            let event = if cs.iter().all(|c| *c == Conclusion::Success) {
                JobEvent::NeedsMet
            } else {
                done.insert(j.spec.id, Conclusion::Skipped);
                JobEvent::NeedsFailed
            };
            evs.push(ActionsEvent::Job { job: j.spec.id, version: j.version, event });
        }
    }
}

impl ActionsCmd {
    fn job_event(
        &self,
        s: &ActionsScratch<'_>,
        job: JobId,
        version: u64,
        event: &JobEvent,
    ) -> Result<Decided, ActionsErr> {
        let j = s.job(job).ok_or(ActionsErr::NotFound)?;
        let allowed = match (&self.actor, event) {
            (ActionsActor::Operator, _) => true,
            (ActionsActor::Admin(_), JobEvent::Cancel) => true,
            // A runner's own match, and its job's acquire; then renew and finish under the job's token.
            (ActionsActor::Runner(r), JobEvent::Matched(m)) => r == m,
            (ActionsActor::Runner(r), JobEvent::Acquired) => j.state.runner() == Some(*r),
            (ActionsActor::Job(me), JobEvent::Renewed | JobEvent::Finished(_)) => *me == job,
            _ => false,
        };
        if !allowed {
            return Err(ActionsErr::Forbidden);
        }
        if let JobEvent::Matched(r) = event
            && !s.runner(*r).is_some_and(|rec| j.spec.labels.fits(&rec.labels))
        {
            return Err(ActionsErr::Illegal("the runner does not take this job's labels".into()));
        }
        if j.version != version {
            return Err(ActionsErr::Stale(j.version));
        }
        let next = transition(&j.state, event, &j.spec.labels, s.at())
            .map_err(|e| ActionsErr::Illegal(e.to_string().into()))?;
        // A renew with more than `LEASE_REWRITE_UNDER` left writes nothing (SHARED, Few writes per job).
        if next == j.state {
            return Ok((SmallVec::new(), ActionsOut::Job(next)));
        }
        let mut evs = smallvec![ActionsEvent::Job { job, version, event: event.clone() }];
        release_needs(s, &mut evs);
        Ok((evs, ActionsOut::Job(next)))
    }

    fn create_run(&self, s: &ActionsScratch<'_>, run: &WorkflowRun, jobs: &[JobSpec]) -> Result<Decided, ActionsErr> {
        if jobs.len() > RUN_JOBS_MAX {
            return Err(ActionsErr::TooMany);
        }
        if s.has_key(&run.key()) {
            return Err(ActionsErr::Duplicate);
        }
        let base = s.ids.peek_n(1 + jobs.len() as u32).ok_or(ActionsErr::NeedIdBlock)?;
        let run = WorkflowRun { id: RunId(base), ..run.clone() };
        let ids: Vec<JobId> = (0..jobs.len()).map(|i| JobId(DbId(base.0 + 1 + i as i64))).collect();
        let mut specs = Vec::with_capacity(jobs.len());
        for (i, j) in jobs.iter().enumerate() {
            let mut needs = SmallVec::new();
            for p in &j.needs {
                match usize::try_from(p.0.0) {
                    Ok(p) if p < i => needs.push(ids[p]),
                    _ => return Err(ActionsErr::BadNeeds),
                }
            }
            specs.push(JobSpec { id: ids[i], needs, ..j.clone() });
        }
        let id = run.id;
        Ok((smallvec![ActionsEvent::RunCreated { run, jobs: specs }], ActionsOut::Run(id)))
    }
}

impl Command for ActionsCmd {
    type Fold = ActionsFold;
    type Dep = ();
    type Out = ActionsOut;
    type Err = ActionsErr;

    fn format(&self) -> Format {
        match &self.op {
            ActionsOp::CreateRun { run, .. } if run.is_merge_group() => ACTIONS_FORMAT_MERGE_GROUP,
            ActionsOp::MatchOwner { .. } => ACTIONS_FORMAT_OWNER_MATCH,
            _ => ACTIONS_FORMAT,
        }
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, _fold: &ActionsFold) -> SmallVec<[ChainId; 3]> {
        SmallVec::new()
    }
    fn decide(&self, s: &ActionsScratch<'_>, _deps: &[(ChainId, ())]) -> Result<Decided, ActionsErr> {
        use ActionsActor::{Admin, Operator, Runner};
        let operator = self.actor == Operator;
        let admin = operator || matches!(self.actor, Admin(_));
        let only = |ok: bool| if ok { Ok(()) } else { Err(ActionsErr::Forbidden) };
        match &self.op {
            ActionsOp::CreateRun { run, jobs } => {
                only(operator)?;
                self.create_run(s, run, jobs)
            }
            ActionsOp::Job { job, version, event } => self.job_event(s, *job, *version, event),
            ActionsOp::ExpireLeases => {
                only(operator)?;
                let mut evs = SmallVec::new();
                for (id, j) in s.jobs() {
                    let event = match &j.state {
                        JobState::Queued { .. } => JobEvent::QueueExpired,
                        JobState::Assigned { .. } | JobState::InProgress { .. } => JobEvent::LeaseExpired,
                        _ => continue,
                    };
                    if transition(&j.state, &event, &j.spec.labels, s.at()).is_ok() {
                        evs.push(ActionsEvent::Job { job: id, version: j.version, event });
                    }
                }
                release_needs(s, &mut evs);
                Ok((evs, ActionsOut::Done))
            }
            ActionsOp::PutRunner(r) => {
                only(admin)?;
                if s.runners_in(r.scope) >= RUNNERS_PER_SCOPE_MAX {
                    return Err(ActionsErr::TooMany);
                }
                if r.hosted {
                    crate::hosted::admit(s, r)?;
                }
                let id = RunnerId(s.ids.peek_n(1).ok_or(ActionsErr::NeedIdBlock)?);
                Ok((smallvec![ActionsEvent::Runner { id, record: Some(r.clone()) }], ActionsOut::Runner(id)))
            }
            ActionsOp::DeleteRunner(id) => {
                only(admin || self.actor == Runner(*id))?;
                let Some(rec) = s.runner(*id) else { return none() };
                if rec.hosted {
                    crate::hosted::may_delete(s, &self.actor, *id)?;
                }
                Ok((smallvec![ActionsEvent::Runner { id: *id, record: None }], ActionsOut::Done))
            }
            ActionsOp::OpenSession { runner, session } => {
                only(operator || self.actor == Runner(*runner))?;
                if s.runner(*runner).is_none() {
                    return Err(ActionsErr::NotFound);
                }
                if s.session(*runner) == Some(*session) {
                    return none();
                }
                Ok((smallvec![ActionsEvent::Session { runner: *runner, session: Some(*session) }], ActionsOut::Done))
            }
            ActionsOp::CloseSession { runner } => {
                only(operator || self.actor == Runner(*runner))?;
                if s.session(*runner).is_none() {
                    return none();
                }
                Ok((smallvec![ActionsEvent::Session { runner: *runner, session: None }], ActionsOut::Done))
            }
            ActionsOp::SetRunStatus { run } => {
                only(operator)?;
                let row = s.run(*run).ok_or(ActionsErr::NotFound)?;
                let states: Vec<JobState> = s.run_jobs(*run).into_iter().map(|j| j.state.clone()).collect();
                let status = derive_run_status(&states);
                if status == row.status {
                    return none();
                }
                Ok((smallvec![ActionsEvent::RunStatus { run: *run, status }], ActionsOut::Done))
            }
            ActionsOp::CancelRun { run } => {
                only(admin)?;
                s.run(*run).ok_or(ActionsErr::NotFound)?;
                let evs = s
                    .run_jobs(*run)
                    .into_iter()
                    .filter(|j| j.state.completed().is_none())
                    .map(|j| ActionsEvent::Job { job: j.spec.id, version: j.version, event: JobEvent::Cancel })
                    .collect();
                Ok((evs, ActionsOut::Done))
            }
            ActionsOp::OwnerCursor { through } => {
                only(operator)?;
                if *through <= s.owner_cursor() {
                    return none();
                }
                Ok((smallvec![ActionsEvent::OwnerCursor { through: *through }], ActionsOut::Done))
            }
            ActionsOp::MatchOwner { job, version, runner, labels } => {
                only(operator)?;
                let j = s.job(*job).ok_or(ActionsErr::NotFound)?;
                if !j.spec.labels.fits(labels) {
                    return Err(ActionsErr::Illegal("the runner does not take this job's labels".into()));
                }
                if j.version != *version {
                    return Err(ActionsErr::Stale(j.version));
                }
                let event = JobEvent::Matched(*runner);
                let next = transition(&j.state, &event, &j.spec.labels, s.at())
                    .map_err(|e| ActionsErr::Illegal(e.to_string().into()))?;
                Ok((smallvec![ActionsEvent::Job { job: *job, version: *version, event }], ActionsOut::Job(next)))
            }
        }
    }
    fn replay(&self, events: &[ActionsEvent]) -> ActionsOut {
        match events.first() {
            Some(ActionsEvent::RunCreated { run, .. }) => ActionsOut::Run(run.id),
            Some(ActionsEvent::Runner { id, record: Some(_) }) => ActionsOut::Runner(*id),
            _ => ActionsOut::Done,
        }
    }
}

/// A command's request id when it comes from a consumer: derived from what it does, so a re-sent one is the same
/// request [inference: as `PrSync`'s content-derived ids].
pub fn consumer_request(chain: ChainId, what: &[u8]) -> RequestId {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"graftyard/actions-request/1\0");
    h.update(chain.to_string().as_bytes());
    h.update([0]);
    h.update(what);
    let d = h.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    RequestId(graftyard_types::Ulid(u128::from_be_bytes(b)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_raft::{PREV_ZERO, Term};
    use std::time::Duration;

    fn entry(index: u64, at: u64, extend: Option<ExtendLeases>, evs: Vec<ActionsEvent>) -> LogEntry<ActionsEvent> {
        LogEntry {
            term: Term(1),
            index: Index(index),
            prev: PREV_ZERO,
            format: ACTIONS_FORMAT,
            tiers: TierSet::CORE,
            at: Timestamp(at),
            requests: SmallVec::new(),
            extend,
            body: EntryBody::Events(evs),
        }
    }

    fn job(n: i64, labels: &LabelSet) -> JobSpec {
        JobSpec {
            id: JobId(DbId(n)),
            name: "j".into(),
            labels: labels.clone(),
            needs: SmallVec::new(),
            matrix: SmallVec::new(),
            environment: None,
            steps: "[]".into(),
        }
    }

    /// A recorded stall moves a live lease and a queued job's `since` by exactly its length, through the fold and
    /// through `decide`'s scratch alike; a lease that had ended before the stall began stays ended, and with no stall
    /// nothing moves, so idle time never keeps a dead runner's job alive (SHARED, Leases).
    #[test]
    fn a_stall_moves_live_leases_by_its_length_and_revives_none() {
        let l = LabelSet::parse(["linux"]).unwrap();
        let run = WorkflowRun {
            id: RunId(DbId(1)),
            repo: RepoId(DbId(7)),
            workflow: "ci.yml".into(),
            event: "push".into(),
            head_sha: [1; 20],
            head_ref: "refs/heads/main".into(),
            trigger: (ChainId::Repo(RepoId(DbId(7))), Index(3)),
            attempt: 1,
            concurrency: None,
            fork: false,
        };
        let (r, t0) = (RunnerId(DbId(9)), 1_000_000);
        let lease = JOB_LEASE_MS;
        let mut f = ActionsFold::default();
        let jobs = vec![job(2, &l), job(3, &l), job(4, &l)];
        f.apply_entry(&entry(1, t0, None, vec![ActionsEvent::RunCreated { run, jobs }]));
        let m = |j, v| ActionsEvent::Job { job: JobId(DbId(j)), version: v, event: JobEvent::Matched(r) };
        let a = |j, v| ActionsEvent::Job { job: JobId(DbId(j)), version: v, event: JobEvent::Acquired };
        // Job 2 starts at t0, job 3 a lease later, so job 2's lease ends before a stall that starts then.
        f.apply_entry(&entry(2, t0, None, vec![m(2, 0), a(2, 1)]));
        f.apply_entry(&entry(3, t0 + lease + 1_000, None, vec![m(3, 0), a(3, 1)]));
        let lease_of = |f: &ActionsFold, j| match f.job(JobId(DbId(j))).unwrap().state {
            JobState::InProgress { lease_until, .. } => lease_until.0,
            JobState::Queued { since, .. } => since.0,
            ref s => panic!("{s:?}"),
        };
        let (l2, l3, q4) = (lease_of(&f, 2), lease_of(&f, 3), lease_of(&f, 4));
        assert_eq!(l2, t0 + lease);
        let stall = ExtendLeases { since: Timestamp(t0 + lease + 2_000), by: Duration::from_secs(90) };
        let x = 90_000;
        // The scratch a leader decides on after the stall sees what the fold will apply.
        let mut sc = f.scratch_at(Timestamp(t0 + lease + 92_000), TierSet::CORE);
        sc.extend(stall);
        let in_scratch = |j| sc.job(JobId(DbId(j))).unwrap().state.deadline().unwrap().0;
        assert_eq!((in_scratch(2), in_scratch(3)), (l2, l3 + x));
        f.apply_entry(&entry(4, t0 + lease + 92_000, Some(stall), Vec::new()));
        assert_eq!(lease_of(&f, 2), l2, "a lease that ended before the stall is not revived");
        assert_eq!(lease_of(&f, 3), l3 + x, "a live lease moves by the stall");
        assert_eq!(lease_of(&f, 4), q4 + x, "a queued job's wait moves by the stall");
        // An idle entry with no stall moves nothing.
        f.apply_entry(&entry(5, t0 + 10 * lease, None, Vec::new()));
        assert_eq!(lease_of(&f, 3), l3 + x);
    }
}
