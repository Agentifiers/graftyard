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

use graftyard_identity::PermissionSet;
use graftyard_raft::{Command, EntryBody, ExtendLeases, Fold, Format, LogEntry, Record, Scratch, TierSet};
use graftyard_types::{AccountId, ChainId, DbId, IdBlock, Index, RepoId, RequestId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::cache::{CacheEntry, CacheId, CacheRows, CacheState};
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
/// B0-35's `CreateRun` of a `pull_request` run, whose run key names its pull request's merge ref
/// (`WorkflowRun::key`), so one entry that moves several pull requests' heads starts a run for each. Only
/// `ActionsTrigger` sends one, and only where a later site day set `GRAFTYARD_PR_TRIGGER`, after the day that shipped
/// this reader to every member (`decisions/DECISIONS.md`, "`pr-trigger`").
pub const ACTIONS_FORMAT_PULL_REQUEST: Format = Format { family: 0x15, version: 4 };
/// `gh-token`'s `CreateRunWith`, whose entry also records the jobs' declared `permissions:` (`ActionsEvent::JobTokens`),
/// and `AcquireToken`, whose entry also marks the job's `GITHUB_TOKEN` issued (`ActionsEvent::TokenIssued`). Only a site
/// whose site day set `GRAFTYARD_JOB_TOKENS` sends either, after `FormatsEnable` enabled 0x15.5 and 0x12.14, and that
/// day follows the one that shipped this reader to every member (`decisions/DECISIONS.md`, "`gh-token`").
pub const ACTIONS_FORMAT_TOKENS: Format = Format { family: 0x15, version: 5 };
/// `actions-cache`'s commands and events (`crate::cache`), and `CreateRunOn`, whose entry also records a `pull_request`
/// run's base branch (`ActionsEvent::RunBase`). Only a site whose site day set `GRAFTYARD_ACTIONS_CACHE` sends any,
/// after `FormatsEnable` enabled 0x15.6, and that day follows the one that shipped this reader to every member
/// (`decisions/DECISIONS.md`, "`actions-cache`").
pub const ACTIONS_FORMAT_CACHE: Format = Format { family: 0x15, version: 6 };
/// `CreateRunFrom`, whose entry also records a same-repository `pull_request` run's head branch
/// (`ActionsEvent::RunHead`), which its jobs' ID tokens carry as `head_ref` (F121). Only a site whose site day set
/// `GRAFTYARD_OIDC_HEAD_REF` sends one, after 0x15.7 was enabled, and that day follows the one that shipped this reader
/// to every member (`decisions/DECISIONS.md`, "`actions-oidc`").
pub const ACTIONS_FORMAT_HEAD_REF: Format = Format { family: 0x15, version: 7 };

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
    /// holds). Only format 0x15.2 carries such a run, so a member that keys runs the old way never decides one. One
    /// entry also moves several pull requests' heads, so a `pull_request` run's key names its `refs/pull/N/merge`
    /// the same way, in format 0x15.4.
    pub fn key(&self) -> RunKey {
        let workflow = if self.is_merge_group() || self.is_pull_request() {
            format!("{}\n{}", self.workflow, self.head_ref).into()
        } else {
            self.workflow.clone()
        };
        RunKey { chain: self.trigger.0, index: self.trigger.1, workflow }
    }

    pub fn is_merge_group(&self) -> bool {
        &*self.event == "merge_group"
    }

    pub fn is_pull_request(&self) -> bool {
        &*self.event == "pull_request"
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
    /// `gh-token`: the `permissions:` the run's jobs declared, by the workflow or the job, in the same entry as their
    /// `RunCreated` (format 0x15.5). A job with none gets the repository's default at `acquirejob`.
    JobTokens {
        jobs: Vec<(JobId, PermissionSet)>,
    },
    /// `gh-token`: the job's runner was given a `GITHUB_TOKEN`, in the entry of its `Acquired` (format 0x15.5).
    /// `RunOwner` denies the token of a job so marked when the job ends (`crate::token::denies`).
    TokenIssued {
        job: JobId,
    },
    /// `actions-cache` (format 0x15.6): a save reserved entry `id`, which only its job uploads to and commits.
    CacheReserved {
        id: CacheId,
        entry: CacheEntry,
    },
    /// The reservation's chunks tile its size: the entry restores from the entry's time on.
    CacheCommitted {
        id: CacheId,
    },
    /// A hit at least `CACHE_USED_EVERY_MS` after the entry's last recorded use.
    CacheUsed {
        id: CacheId,
    },
    /// Entries removed by LRU, the 7-day expiry, a stale reservation, or the shard's entry cap. The leader deletes
    /// their chunks after the entry commits, and the daily sweep any it missed.
    CacheEvicted {
        ids: Vec<CacheId>,
    },
    /// A `pull_request` run's base branch, in its `RunCreated`'s entry, which its jobs read caches from.
    RunBase {
        run: RunId,
        base_ref: Box<str>,
    },
    /// A same-repository `pull_request` run's head branch, in its `RunCreated`'s entry, which its jobs' ID tokens
    /// carry as `head_ref` (0x15.7).
    RunHead {
        run: RunId,
        head_ref: Box<str>,
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

/// The shard's fold. It is stored and sent whole (`codec::to_bytes`), so rows added after M4-WP3 sit in the trailing
/// `more` part, written only once it holds a row, which only an event of a format `FormatsEnable` enabled writes
/// (`graftyard_identity::tail_serde`): until then a fold's bytes are the release before's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    pub more: ActionsMore,
}

graftyard_identity::tail_serde!(ActionsFold {
    runs: BTreeMap<RunId, RunRow>,
    keys: BTreeMap<RunKey, RunId>,
    jobs: BTreeMap<JobId, JobRow>,
    runners: BTreeMap<RunnerId, RunnerRecord>,
    sessions: BTreeMap<RunnerId, RunnerSessionId>,
    queue_moves: u64,
    owner_cursor: Index,
    ids: IdCursor,
} more: ActionsMore);

/// The fold's rows after M4-WP3: each job's declared token permissions (`ActionsEvent::JobTokens`), and the jobs whose
/// runner was given a token (`ActionsEvent::TokenIssued`). The cache rows (0x15.6) are a trailing part of their own,
/// so a fold with none keeps 0x15.5's bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionsMore {
    pub tokens: BTreeMap<JobId, PermissionSet>,
    pub issued: std::collections::BTreeSet<JobId>,
    pub more: CacheRows,
}

graftyard_identity::tail_serde!(ActionsMore {
    tokens: BTreeMap<JobId, PermissionSet>,
    issued: std::collections::BTreeSet<JobId>,
} more: CacheRows);

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
    /// The jobs' declared token permissions, and an issued token's mark. `decide` reads the fold's marks only, so the
    /// scratch drops both.
    fn put_tokens(&mut self, _jobs: &[(JobId, PermissionSet)]) {}
    fn put_issued(&mut self, _job: JobId) {}
    fn cache(&self, id: CacheId) -> Option<&CacheEntry>;
    fn put_cache(&mut self, id: CacheId, e: Option<CacheEntry>);
    /// A `pull_request` run's base branch, which `decide` never reads, so the scratch drops it.
    fn put_base(&mut self, _run: RunId, _base: &str) {}
    /// A `pull_request` run's head branch, likewise.
    fn put_head(&mut self, _run: RunId, _head: &str) {}
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
        ActionsEvent::JobTokens { jobs } => t.put_tokens(jobs),
        ActionsEvent::TokenIssued { job } => t.put_issued(*job),
        ActionsEvent::CacheReserved { id, entry } => t.put_cache(*id, Some(entry.clone())),
        ActionsEvent::CacheCommitted { id } => {
            if let Some(e) = t.cache(*id) {
                let e = CacheEntry { state: CacheState::Committed, created: at, last_used: at, ..e.clone() };
                t.put_cache(*id, Some(e));
            }
        }
        ActionsEvent::CacheUsed { id } => {
            if let Some(e) = t.cache(*id) {
                let e = CacheEntry { last_used: at, ..e.clone() };
                t.put_cache(*id, Some(e));
            }
        }
        ActionsEvent::CacheEvicted { ids } => {
            for id in ids {
                t.put_cache(*id, None);
            }
        }
        ActionsEvent::RunBase { run, base_ref } => t.put_base(*run, base_ref),
        ActionsEvent::RunHead { run, head_ref } => t.put_head(*run, head_ref),
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
    fn put_tokens(&mut self, jobs: &[(JobId, PermissionSet)]) {
        self.more.tokens.extend(jobs.iter().cloned());
    }
    fn put_issued(&mut self, job: JobId) {
        self.more.issued.insert(job);
    }
    fn cache(&self, id: CacheId) -> Option<&CacheEntry> {
        self.more.more.entries.get(&id)
    }
    fn put_cache(&mut self, id: CacheId, e: Option<CacheEntry>) {
        let c = &mut self.more.more;
        match e {
            Some(e) => {
                c.next = c.next.max(id.0);
                c.entries.insert(id, e);
            }
            None => {
                c.entries.remove(&id);
            }
        }
    }
    fn put_base(&mut self, run: RunId, base: &str) {
        self.more.more.bases.insert(run, base.into());
    }
    fn put_head(&mut self, run: RunId, head: &str) {
        self.more.more.more.heads.insert(run, head.into());
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
    /// `actions-cache`: the entry a lookup by job `job` restores (`crate::cache::find` over the run's scopes), with
    /// the run's own ref first, its base branch for a `pull_request` run, then `default_branch`. `Forbidden` for a job
    /// not running.
    pub fn cache_find(
        &self,
        job: JobId,
        keys: &[&str],
        version: &str,
        default_branch: &str,
    ) -> Result<Option<(CacheId, &CacheEntry)>, ActionsErr> {
        let run = self.cache_run(job)?;
        let base = self.run_base(run.id);
        let scopes = crate::cache::scopes(&run.head_ref, base, default_branch);
        let mine = self.more.more.entries.iter().filter(|(_, e)| e.repo == run.repo).map(|(k, v)| (*k, v));
        Ok(crate::cache::find(mine, keys, version, &scopes))
    }
    /// A `pull_request` run's base branch (`ActionsEvent::RunBase`), which its jobs' ID tokens carry as `base_ref`.
    pub fn run_base(&self, r: RunId) -> Option<&str> {
        self.more.more.bases.get(&r).map(|b| &**b)
    }
    /// A same-repository `pull_request` run's head branch (`ActionsEvent::RunHead`), which its jobs' ID tokens carry
    /// as `head_ref`.
    pub fn run_head(&self, r: RunId) -> Option<&str> {
        self.more.more.more.heads.get(&r).map(|h| &**h)
    }
    /// The run of a running job, which a cache call acts for.
    pub fn cache_run(&self, job: JobId) -> Result<&WorkflowRun, ActionsErr> {
        let j = self.job(job).ok_or(ActionsErr::NotFound)?;
        if !matches!(j.state, JobState::InProgress { .. }) {
            return Err(ActionsErr::Forbidden);
        }
        Ok(&self.run(j.run).ok_or(ActionsErr::NotFound)?.run)
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
    cache: BTreeMap<CacheId, Option<CacheEntry>>,
    cache_next: u64,
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
    fn cache(&self, id: CacheId) -> Option<&CacheEntry> {
        match self.cache.get(&id) {
            Some(e) => e.as_ref(),
            None => self.base.more.more.entries.get(&id),
        }
    }
    fn put_cache(&mut self, id: CacheId, e: Option<CacheEntry>) {
        if e.is_some() {
            self.cache_next = self.cache_next.max(id.0);
        }
        self.cache.insert(id, e);
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
            cache: BTreeMap::new(),
            cache_next: base.more.more.next,
        }
    }
    pub fn cache_entry(&self, id: CacheId) -> Option<&CacheEntry> {
        Tables::cache(self, id)
    }
    /// Every cache entry with its row, the overlay's over the fold's.
    pub fn cache_entries(&self) -> impl Iterator<Item = (CacheId, &CacheEntry)> {
        let over = self.cache.iter().filter_map(|(k, v)| v.as_ref().map(|v| (*k, v)));
        let base =
            self.base.more.more.entries.iter().filter(|(k, _)| !self.cache.contains_key(k)).map(|(k, v)| (*k, v));
        over.chain(base)
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
            ActionsEvent::JobTokens { jobs } if jobs.iter().any(|(j, _)| self.job(*j).is_none()) => {
                Err("permissions for a job the entry did not create".into())
            }
            ActionsEvent::TokenIssued { job }
                if !matches!(self.job(*job).map(|j| &j.state), Some(JobState::InProgress { .. })) =>
            {
                Err("a token for a job no runner took".into())
            }
            ActionsEvent::CacheReserved { id, .. } if id.0 <= self.cache_next => {
                Err(format!("cache id {} is not new", id.0).into())
            }
            ActionsEvent::CacheCommitted { id }
                if !matches!(self.cache_entry(*id).map(|e| &e.state), Some(CacheState::Reserved { .. })) =>
            {
                Err(format!("cache {} is not reserved", id.0).into())
            }
            ActionsEvent::RunBase { run, .. } if self.run(*run).is_none() => {
                Err("a base branch for a run the entry did not create".into())
            }
            ActionsEvent::RunHead { run, .. } if self.run(*run).is_none() => {
                Err("a head branch for a run the entry did not create".into())
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
    /// `gh-token`: `CreateRun` whose jobs declared `permissions:`, each by its position in `jobs` (format 0x15.5).
    CreateRunWith {
        run: WorkflowRun,
        jobs: Vec<JobSpec>,
        permissions: Vec<(u16, PermissionSet)>,
    },
    /// `gh-token`: a runner's `acquirejob` whose answer carries a `GITHUB_TOKEN`: the `Acquired` a `Job` op commits, and
    /// the token's mark when the job has none, so a repeated acquire after a lost answer marks it too (format 0x15.5).
    AcquireToken {
        job: JobId,
        version: u64,
    },
    /// `actions-cache` (0x15.6): the job's `POST caches`. `default_branch` is the repository's, which the edge read and
    /// the shard does not hold; `limit` the repository's cache bytes (`CACHE_REPO_MAX`, lower only on a test install).
    CacheReserve {
        job: JobId,
        key: Box<str>,
        version: Box<str>,
        size: u64,
        default_branch: Box<str>,
    },
    /// The job's `POST caches/{id} {size}`, with the chunks the edge listed under the entry's prefix, `(start, len)`.
    CacheCommit {
        job: JobId,
        id: CacheId,
        size: u64,
        chunks: Vec<(u64, u64)>,
        limit: u64,
    },
    /// A hit: moves the entry's last use, at most once per `CACHE_USED_EVERY_MS`.
    CacheUse {
        id: CacheId,
    },
    /// The leader's alarm: every entry past its 7-day expiry and every stale reservation.
    CacheSweep,
    /// `CreateRunWith` for a `pull_request` run, with its base branch (`ActionsEvent::RunBase`).
    CreateRunOn {
        run: WorkflowRun,
        jobs: Vec<JobSpec>,
        permissions: Vec<(u16, PermissionSet)>,
        base_ref: Box<str>,
    },
    /// F125: a deleted repository's cache entries, every one of them and no other repository's, which the `Audit`
    /// finisher's delete sends (`crate::cache`; `design/SHARED.md`, "Repository repair, tombstones, and retention").
    /// It commits only `CacheEvicted`, an 0x15.6 event, so it is format 0x15.6: it reaches the leader only as a
    /// `/runsvc` call, and no entry type holds a command.
    CacheDropRepo {
        repo: RepoId,
    },
    /// `CreateRunOn` for a same-repository `pull_request` run, with its head branch too (`ActionsEvent::RunHead`).
    CreateRunFrom {
        run: WorkflowRun,
        jobs: Vec<JobSpec>,
        permissions: Vec<(u16, PermissionSet)>,
        base_ref: Box<str>,
        head_ref: Box<str>,
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
    Cache(CacheId),
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
    /// A reservation or entry already holds the key in this scope (409, which the toolkit reads as "another job may be
    /// creating this cache").
    #[error("a cache entry or reservation already holds this key")]
    CacheExists,
    /// A size past the limit, or chunks that do not make the reserved entry (400).
    #[error("{0}")]
    CacheRefused(Box<str>),
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

    fn create_run(
        &self,
        s: &ActionsScratch<'_>,
        run: &WorkflowRun,
        jobs: &[JobSpec],
        permissions: &[(u16, PermissionSet)],
    ) -> Result<Decided, ActionsErr> {
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
        let mut tokens = Vec::with_capacity(permissions.len());
        for (p, set) in permissions {
            let id = ids.get(usize::from(*p)).ok_or(ActionsErr::BadNeeds)?;
            tokens.push((*id, set.clone()));
        }
        let id = run.id;
        let mut evs = smallvec![ActionsEvent::RunCreated { run, jobs: specs }];
        if !tokens.is_empty() {
            evs.push(ActionsEvent::JobTokens { jobs: tokens });
        }
        Ok((evs, ActionsOut::Run(id)))
    }

    /// The job a cache command acts for, running now, and its run.
    fn cache_job<'s>(&self, s: &'s ActionsScratch<'_>, job: JobId) -> Result<&'s WorkflowRun, ActionsErr> {
        if !matches!(self.actor, ActionsActor::Job(me) if me == job) {
            return Err(ActionsErr::Forbidden);
        }
        let j = s.job(job).ok_or(ActionsErr::NotFound)?;
        if !matches!(j.state, JobState::InProgress { .. }) {
            return Err(ActionsErr::Forbidden);
        }
        Ok(&s.run(j.run).ok_or(ActionsErr::NotFound)?.run)
    }

    fn cache_reserve(
        &self,
        s: &ActionsScratch<'_>,
        job: JobId,
        (key, version, size): (&str, &str, u64),
        default_branch: &str,
    ) -> Result<Decided, ActionsErr> {
        use crate::cache::{CACHE_ENTRIES_MAX, CACHE_REPO_MAX, may_write};
        let run = self.cache_job(s, job)?;
        if !may_write(&run.event, &run.head_ref, default_branch) {
            return Err(ActionsErr::Forbidden);
        }
        if size > CACHE_REPO_MAX {
            return Err(ActionsErr::CacheRefused(
                format!("Cache size of {size} B is over the repository's {CACHE_REPO_MAX} B limit").into(),
            ));
        }
        let at = s.at();
        let same = |e: &CacheEntry| {
            e.repo == run.repo && *e.scope == *run.head_ref && &*e.key == key && &*e.version == version
        };
        if s.cache_entries().any(|(_, e)| same(e) && e.holds(at)) {
            return Err(ActionsErr::CacheExists);
        }
        // A stale reservation of the same key gives it up; past the shard's cap, its least recently used entries go.
        let mut gone: Vec<CacheId> = s.cache_entries().filter(|(_, e)| same(e)).map(|(id, _)| id).collect();
        let live = s.cache_entries().count() - gone.len();
        if live >= CACHE_ENTRIES_MAX {
            let mut old: Vec<(CacheId, &CacheEntry)> = s
                .cache_entries()
                .filter(|(id, e)| !gone.contains(id) && !matches!(e.state, CacheState::Reserved { .. } if e.holds(at)))
                .collect();
            old.sort_by_key(|(id, e)| (e.last_used, e.created, *id));
            gone.extend(old.into_iter().take(live + 1 - CACHE_ENTRIES_MAX).map(|(id, _)| id));
        }
        // From 1: the toolkit reads a `cacheId` of 0 as no reservation (`if (reserveCacheResponse?.result?.cacheId)`).
        let id = CacheId(s.cache_next + 1);
        let entry = CacheEntry {
            repo: run.repo,
            scope: run.head_ref.clone(),
            key: key.into(),
            version: version.into(),
            size,
            created: at,
            last_used: at,
            state: CacheState::Reserved { job },
        };
        let mut evs = SmallVec::new();
        if !gone.is_empty() {
            gone.sort();
            evs.push(ActionsEvent::CacheEvicted { ids: gone });
        }
        evs.push(ActionsEvent::CacheReserved { id, entry });
        Ok((evs, ActionsOut::Cache(id)))
    }

    fn cache_commit(
        &self,
        s: &ActionsScratch<'_>,
        job: JobId,
        id: CacheId,
        size: u64,
        (chunks, limit): (&[(u64, u64)], u64),
    ) -> Result<Decided, ActionsErr> {
        use crate::cache::{CACHE_REPO_MAX, lru, tiles};
        self.cache_job(s, job)?;
        let e = s.cache_entry(id).ok_or(ActionsErr::NotFound)?;
        match e.state {
            CacheState::Committed => return Ok((SmallVec::new(), ActionsOut::Cache(id))),
            CacheState::Reserved { job: by } if by != job => return Err(ActionsErr::Forbidden),
            CacheState::Reserved { .. } => {}
        }
        if size != e.size {
            return Err(ActionsErr::CacheRefused(format!("size {size} is not the reserved {}", e.size).into()));
        }
        tiles(chunks, size)
            .map_err(|t| ActionsErr::CacheRefused(format!("the chunks do not make it: {t:?}").into()))?;
        let repo = e.repo;
        let committed = CacheEntry { state: CacheState::Committed, ..e.clone() };
        let rows: Vec<(CacheId, &CacheEntry)> = s
            .cache_entries()
            .filter(|(_, x)| x.repo == repo)
            .map(|(k, x)| if k == id { (k, &committed) } else { (k, x) })
            .collect();
        let gone = lru(rows, id, limit.min(CACHE_REPO_MAX));
        let mut evs = smallvec![ActionsEvent::CacheCommitted { id }];
        if !gone.is_empty() {
            evs.push(ActionsEvent::CacheEvicted { ids: gone });
        }
        Ok((evs, ActionsOut::Cache(id)))
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
            ActionsOp::CreateRun { run, .. } if run.is_pull_request() => ACTIONS_FORMAT_PULL_REQUEST,
            ActionsOp::MatchOwner { .. } => ACTIONS_FORMAT_OWNER_MATCH,
            ActionsOp::CreateRunWith { .. } | ActionsOp::AcquireToken { .. } => ACTIONS_FORMAT_TOKENS,
            ActionsOp::CacheReserve { .. }
            | ActionsOp::CacheCommit { .. }
            | ActionsOp::CacheUse { .. }
            | ActionsOp::CacheSweep
            | ActionsOp::CacheDropRepo { .. }
            | ActionsOp::CreateRunOn { .. } => ACTIONS_FORMAT_CACHE,
            ActionsOp::CreateRunFrom { .. } => ACTIONS_FORMAT_HEAD_REF,
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
                self.create_run(s, run, jobs, &[])
            }
            ActionsOp::CreateRunWith { run, jobs, permissions } => {
                only(operator)?;
                self.create_run(s, run, jobs, permissions)
            }
            ActionsOp::CreateRunOn { run, jobs, permissions, base_ref } => {
                only(operator)?;
                let (mut evs, out) = self.create_run(s, run, jobs, permissions)?;
                if let ActionsOut::Run(id) = out {
                    evs.push(ActionsEvent::RunBase { run: id, base_ref: base_ref.clone() });
                }
                Ok((evs, out))
            }
            ActionsOp::CreateRunFrom { run, jobs, permissions, base_ref, head_ref } => {
                only(operator)?;
                let (mut evs, out) = self.create_run(s, run, jobs, permissions)?;
                if let ActionsOut::Run(id) = out {
                    evs.push(ActionsEvent::RunBase { run: id, base_ref: base_ref.clone() });
                    evs.push(ActionsEvent::RunHead { run: id, head_ref: head_ref.clone() });
                }
                Ok((evs, out))
            }
            ActionsOp::CacheReserve { job, key, version, size, default_branch } => {
                self.cache_reserve(s, *job, (key, version, *size), default_branch)
            }
            ActionsOp::CacheCommit { job, id, size, chunks, limit } => {
                self.cache_commit(s, *job, *id, *size, (chunks, *limit))
            }
            ActionsOp::CacheUse { id } => {
                only(operator || matches!(self.actor, ActionsActor::Job(_)))?;
                let Some(e) = s.cache_entry(*id).filter(|e| e.committed()) else { return none() };
                if s.at().0.saturating_sub(e.last_used.0) < crate::cache::CACHE_USED_EVERY_MS {
                    return none();
                }
                Ok((smallvec![ActionsEvent::CacheUsed { id: *id }], ActionsOut::Done))
            }
            ActionsOp::CacheSweep => {
                only(operator)?;
                let at = s.at();
                let ids: Vec<CacheId> = s.cache_entries().filter(|(_, e)| e.due() <= at).map(|(id, _)| id).collect();
                if ids.is_empty() {
                    return none();
                }
                let mut ids = ids;
                ids.sort();
                Ok((smallvec![ActionsEvent::CacheEvicted { ids }], ActionsOut::Done))
            }
            ActionsOp::CacheDropRepo { repo } => {
                only(operator)?;
                let mut ids: Vec<CacheId> =
                    s.cache_entries().filter(|(_, e)| e.repo == *repo).map(|(id, _)| id).collect();
                if ids.is_empty() {
                    return none();
                }
                ids.sort();
                Ok((smallvec![ActionsEvent::CacheEvicted { ids }], ActionsOut::Done))
            }
            ActionsOp::Job { job, version, event } => self.job_event(s, *job, *version, event),
            ActionsOp::AcquireToken { job, version } => {
                let (mut evs, out) = self.job_event(s, *job, *version, &JobEvent::Acquired)?;
                if !s.base.more.issued.contains(job) {
                    evs.push(ActionsEvent::TokenIssued { job: *job });
                }
                Ok((evs, out))
            }
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
            Some(ActionsEvent::CacheReserved { id, .. } | ActionsEvent::CacheCommitted { id }) => {
                ActionsOut::Cache(*id)
            }
            Some(ActionsEvent::CacheEvicted { .. }) => match events.get(1) {
                Some(ActionsEvent::CacheReserved { id, .. }) => ActionsOut::Cache(*id),
                _ => ActionsOut::Done,
            },
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

    /// Main's derived `ActionsFold` writes the same bytes as this one with no 0x15.5 row, and reads back into it with
    /// `more` empty; a fold with a row reads back whole. So a site day that ships this reader changes no stored fold
    /// until `FormatsEnable` lets a leader write 0x15.5 (`graftyard_identity::tail_serde`).
    #[test]
    fn an_old_fold_keeps_its_bytes_until_a_token_row() {
        #[derive(Serialize, Deserialize)]
        struct Main {
            runs: BTreeMap<RunId, RunRow>,
            keys: BTreeMap<RunKey, RunId>,
            jobs: BTreeMap<JobId, JobRow>,
            runners: BTreeMap<RunnerId, RunnerRecord>,
            sessions: BTreeMap<RunnerId, RunnerSessionId>,
            queue_moves: u64,
            owner_cursor: Index,
            ids: IdCursor,
        }
        let l = LabelSet::parse(["linux"]).unwrap();
        let row = JobRow {
            run: RunId(DbId(1)),
            spec: job(2, &l),
            state: JobState::Queued { labels: l.clone(), since: Timestamp(5) },
            version: 3,
        };
        let jobs: BTreeMap<_, _> = [(JobId(DbId(2)), row)].into();
        let old = Main {
            runs: BTreeMap::new(),
            keys: BTreeMap::new(),
            jobs: jobs.clone(),
            runners: BTreeMap::new(),
            sessions: BTreeMap::new(),
            queue_moves: 7,
            owner_cursor: Index(4),
            ids: IdCursor::default(),
        };
        let old_bytes = postcard::to_allocvec(&old).unwrap();
        let mut f = ActionsFold { jobs, queue_moves: 7, owner_cursor: Index(4), ..ActionsFold::default() };
        assert_eq!(postcard::to_allocvec(&f).unwrap(), old_bytes, "an empty `more` writes main's bytes");
        assert_eq!(postcard::from_bytes::<ActionsFold>(&old_bytes).unwrap(), f, "main's blob reads with no rows");
        f.more.issued.insert(JobId(DbId(2)));
        f.more.tokens.insert(JobId(DbId(2)), graftyard_identity::PermissionSet::default());
        let new = postcard::to_allocvec(&f).unwrap();
        assert!(new.len() > old_bytes.len() && new.starts_with(&old_bytes));
        assert_eq!(postcard::from_bytes::<ActionsFold>(&new).unwrap(), f);
    }

    /// A fold with an issued token but no cache row writes 0x15.5's bytes, and reads them back with no cache rows; a
    /// cache row adds a trailing part after them. So the site day that ships 0x15.6's reader changes no stored fold.
    #[test]
    fn a_fold_keeps_its_bytes_until_a_cache_row() {
        let mut f = ActionsFold::default();
        f.more.issued.insert(JobId(DbId(2)));
        let old = postcard::to_allocvec(&f).unwrap();
        #[derive(Serialize)]
        struct Before<'a>(&'a BTreeMap<JobId, PermissionSet>, &'a std::collections::BTreeSet<JobId>);
        let more = postcard::to_allocvec(&Before(&f.more.tokens, &f.more.issued)).unwrap();
        assert!(old.ends_with(&more), "the 0x15.5 part is the two sets alone");
        assert_eq!(postcard::from_bytes::<ActionsFold>(&old).unwrap(), f);
        f.more.more.next = 4;
        let new = postcard::to_allocvec(&f).unwrap();
        assert!(new.starts_with(&old) && new.len() > old.len());
        assert_eq!(postcard::from_bytes::<ActionsFold>(&new).unwrap(), f);
    }

    mod cache {
        use super::*;
        use crate::cache::{CACHE_ENTRIES_MAX, CACHE_REPO_MAX, CACHE_RESERVE_STALE_MS, CACHE_UNUSED_MS};

        const MAIN: &str = "refs/heads/main";
        const T0: u64 = 1_000_000;

        /// A shard with one id block, and in-progress jobs 101 (run 100 on `main` by `push`), 103 (run 102 on
        /// `refs/heads/f` by `push`), and 105 (run 104 on `main` by `pull_request_target`).
        struct Shard {
            f: ActionsFold,
            at: u64,
            n: u64,
        }

        fn run(id: i64, head_ref: &str, event: &str) -> WorkflowRun {
            WorkflowRun {
                id: RunId(DbId(id)),
                repo: RepoId(DbId(7)),
                workflow: "ci.yml".into(),
                event: event.into(),
                head_sha: [1; 20],
                head_ref: head_ref.into(),
                trigger: (ChainId::Repo(RepoId(DbId(7))), Index(id as u64)),
                attempt: 1,
                concurrency: None,
                fork: false,
            }
        }

        impl Shard {
            fn new() -> Shard {
                let l = LabelSet::parse(["linux"]).unwrap();
                let mut sh = Shard { f: ActionsFold::default(), at: T0, n: 0 };
                for (r, head_ref, event) in
                    [(100, MAIN, "push"), (102, "refs/heads/f", "push"), (104, MAIN, "pull_request_target")]
                {
                    let j = r + 1;
                    sh.apply(vec![ActionsEvent::RunCreated { run: run(r, head_ref, event), jobs: vec![job(j, &l)] }]);
                    let rr = RunnerId(DbId(900 + r));
                    let m = ActionsEvent::Job { job: JobId(DbId(j)), version: 0, event: JobEvent::Matched(rr) };
                    let a = ActionsEvent::Job { job: JobId(DbId(j)), version: 1, event: JobEvent::Acquired };
                    sh.apply(vec![m, a]);
                }
                sh
            }
            fn apply(&mut self, evs: Vec<ActionsEvent>) {
                self.n += 1;
                self.f.apply_entry(&entry(self.n, self.at, None, evs));
            }
            /// `decide` at the shard's time, and its events applied in one entry, as a leader does.
            fn run(&mut self, actor: ActionsActor, op: ActionsOp) -> Result<ActionsOut, ActionsErr> {
                let cmd = ActionsCmd { actor, op };
                let (evs, out) = cmd.decide(&self.f.scratch_at(Timestamp(self.at), TierSet::CORE), &[])?;
                if !evs.is_empty() {
                    let mut sc = self.f.scratch_at(Timestamp(self.at), TierSet::CORE);
                    for ev in &evs {
                        sc.check(ev).expect("a follower verifies what decide wrote");
                        sc.apply(ev);
                    }
                    self.apply(evs.into_vec());
                }
                Ok(out)
            }
            fn reserve(&mut self, job: i64, key: &str, size: u64) -> Result<CacheId, ActionsErr> {
                let op = ActionsOp::CacheReserve {
                    job: JobId(DbId(job)),
                    key: key.into(),
                    version: "v".into(),
                    size,
                    default_branch: MAIN.into(),
                };
                match self.run(ActionsActor::Job(JobId(DbId(job))), op)? {
                    ActionsOut::Cache(id) => Ok(id),
                    o => panic!("{o:?}"),
                }
            }
            fn commit_with(
                &mut self,
                job: i64,
                id: CacheId,
                size: u64,
                chunks: Vec<(u64, u64)>,
                limit: u64,
            ) -> Result<ActionsOut, ActionsErr> {
                let op = ActionsOp::CacheCommit { job: JobId(DbId(job)), id, size, chunks, limit };
                self.run(ActionsActor::Job(JobId(DbId(job))), op)
            }
            fn save(&mut self, job: i64, key: &str, size: u64, limit: u64) -> CacheId {
                let id = self.reserve(job, key, size).unwrap();
                self.commit_with(job, id, size, vec![(0, size)], limit).unwrap();
                id
            }
            fn ids(&self) -> Vec<u64> {
                self.f.more.more.entries.keys().map(|k| k.0).collect()
            }
            fn entry(&self, id: CacheId) -> &CacheEntry {
                &self.f.more.more.entries[&id]
            }
        }

        /// A second reserve of a key its scope holds answers `CacheExists` (the toolkit's 409); another scope or
        /// version is another entry; a reservation an hour old gives its key up and leaves the fold.
        #[test]
        fn a_key_is_reserved_once_per_scope_until_its_reservation_is_stale() {
            let mut sh = Shard::new();
            let a = sh.reserve(101, "k", 10).unwrap();
            assert_eq!(a, CacheId(1), "the toolkit takes a cacheId of 0 for none");
            assert_eq!(sh.reserve(101, "k", 10), Err(ActionsErr::CacheExists));
            sh.reserve(103, "k", 10).unwrap();
            sh.at += CACHE_RESERVE_STALE_MS - 1;
            assert_eq!(sh.reserve(101, "k", 10), Err(ActionsErr::CacheExists), "not stale yet");
            sh.at += 1;
            let b = sh.reserve(101, "k", 10).unwrap();
            assert!(b > a && !sh.f.more.more.entries.contains_key(&a), "the stale reservation left with the new one");
        }

        /// Only the job that reserved an entry commits it, with the reserved size and chunks that tile it exactly.
        #[test]
        fn a_commit_binds_the_reserved_size_and_the_chunks() {
            let mut sh = Shard::new();
            let id = sh.reserve(101, "k", 40).unwrap();
            let refused = |r: Result<ActionsOut, ActionsErr>| matches!(r, Err(ActionsErr::CacheRefused(_)));
            assert!(refused(sh.commit_with(101, id, 41, vec![(0, 41)], CACHE_REPO_MAX)), "another size");
            assert!(refused(sh.commit_with(101, id, 40, vec![(0, 32), (33, 7)], CACHE_REPO_MAX)), "a gap");
            assert!(refused(sh.commit_with(101, id, 40, vec![(0, 32), (31, 9)], CACHE_REPO_MAX)), "an overlap");
            assert_eq!(sh.commit_with(103, id, 40, vec![(0, 40)], CACHE_REPO_MAX), Err(ActionsErr::Forbidden));
            assert!(!sh.entry(id).committed());
            sh.at += 5;
            assert_eq!(sh.commit_with(101, id, 40, vec![(32, 8), (0, 32)], CACHE_REPO_MAX), Ok(ActionsOut::Cache(id)));
            assert!(sh.entry(id).committed() && sh.entry(id).created == Timestamp(T0 + 5));
            assert_eq!(sh.commit_with(101, id, 40, vec![(0, 40)], CACHE_REPO_MAX), Ok(ActionsOut::Cache(id)), "again");
        }

        /// A `pull_request_target` run on the default branch reads its scope but never writes it; a job that ended
        /// commits nothing.
        #[test]
        fn a_low_trust_run_never_writes_the_default_branch() {
            let mut sh = Shard::new();
            assert_eq!(sh.reserve(105, "k", 10), Err(ActionsErr::Forbidden));
            let id = sh.reserve(101, "k", 10).unwrap();
            let fin =
                ActionsEvent::Job { job: JobId(DbId(101)), version: 2, event: JobEvent::Finished(Conclusion::Success) };
            sh.apply(vec![fin]);
            assert_eq!(sh.commit_with(101, id, 10, vec![(0, 10)], CACHE_REPO_MAX), Err(ActionsErr::Forbidden));
        }

        /// A commit past the repository's limit evicts its least recently used entries, not the oldest, and never
        /// the one it commits.
        #[test]
        fn a_commit_past_the_limit_evicts_the_least_recently_used() {
            let mut sh = Shard::new();
            let a = sh.save(101, "a", 30, 100);
            sh.at += 10;
            sh.save(101, "b", 30, 100);
            sh.at += crate::cache::CACHE_USED_EVERY_MS;
            sh.run(ActionsActor::Operator, ActionsOp::CacheUse { id: a }).unwrap();
            let c = sh.save(101, "c", 50, 100);
            assert_eq!(sh.ids(), [a.0, c.0], "b, used least recently, went; a was used later");
            let d = sh.save(103, "d", 200, 100);
            assert_eq!(sh.ids(), [d.0], "an entry over the limit alone stays, the rest go");
        }

        /// A hit moves last use once an hour at most, so many isolates' hits write once.
        #[test]
        fn a_hit_moves_last_use_at_most_hourly() {
            let mut sh = Shard::new();
            let a = sh.save(101, "a", 1, CACHE_REPO_MAX);
            let n = sh.n;
            sh.at += crate::cache::CACHE_USED_EVERY_MS - 1;
            sh.run(ActionsActor::Operator, ActionsOp::CacheUse { id: a }).unwrap();
            assert_eq!(sh.n, n, "within the hour writes nothing");
            sh.at += 1;
            sh.run(ActionsActor::Operator, ActionsOp::CacheUse { id: a }).unwrap();
            sh.run(ActionsActor::Operator, ActionsOp::CacheUse { id: a }).unwrap();
            assert_eq!((sh.n, sh.entry(a).last_used), (n + 1, Timestamp(sh.at)));
        }

        /// The sweep removes entries unread for 7 days and stale reservations, and keeps the rest.
        #[test]
        fn the_sweep_removes_entries_unused_for_seven_days() {
            let mut sh = Shard::new();
            let a = sh.save(101, "a", 1, CACHE_REPO_MAX);
            let b = sh.save(101, "b", 1, CACHE_REPO_MAX);
            sh.at += CACHE_UNUSED_MS - crate::cache::CACHE_USED_EVERY_MS;
            sh.run(ActionsActor::Operator, ActionsOp::CacheUse { id: b }).unwrap();
            let r = sh.reserve(101, "r", 1).unwrap();
            sh.at = T0 + CACHE_UNUSED_MS - 1;
            sh.run(ActionsActor::Operator, ActionsOp::CacheSweep).unwrap();
            assert_eq!(sh.ids(), [a.0, b.0, r.0]);
            sh.at += 1;
            sh.run(ActionsActor::Operator, ActionsOp::CacheSweep).unwrap();
            assert_eq!(sh.ids(), [b.0], "a expired; the reservation was stale");
            assert_eq!(sh.f.more.more.due(), Some(Timestamp(sh.entry(b).last_used.0 + CACHE_UNUSED_MS)));
        }

        /// F125: dropping a deleted repository evicts all of its entries, reserved and committed, and none of
        /// another repository's on the same shard; a second drop finds nothing and writes nothing; only the operator
        /// sends it.
        #[test]
        fn a_repository_drop_evicts_only_that_repositorys_entries() {
            let mut sh = Shard::new();
            let a1 = sh.save(101, "a1", 1, CACHE_REPO_MAX);
            let a2 = sh.reserve(103, "a2", 1).unwrap();
            let other = CacheEntry {
                repo: RepoId(DbId(8)),
                scope: MAIN.into(),
                key: "b".into(),
                version: "v".into(),
                size: 1,
                created: Timestamp(T0),
                last_used: Timestamp(T0),
                state: CacheState::Committed,
            };
            let b = CacheId(sh.f.more.more.next + 1);
            sh.f.more.more.entries.insert(b, other);
            let drop = ActionsOp::CacheDropRepo { repo: RepoId(DbId(7)) };
            assert_eq!(sh.run(ActionsActor::Job(JobId(DbId(101))), drop.clone()), Err(ActionsErr::Forbidden));
            sh.run(ActionsActor::Operator, drop.clone()).unwrap();
            assert_eq!(sh.ids(), [b.0], "a1 ({a1:?}) and a2 ({a2:?}) went, repository 8's entry stayed");
            let n = sh.n;
            sh.run(ActionsActor::Operator, drop).unwrap();
            assert_eq!(sh.n, n, "a retried drop writes nothing");
        }

        /// Past the shard's entry cap a reserve evicts its least recently used entries first.
        #[test]
        fn the_entry_cap_evicts_rather_than_refuses() {
            let mut sh = Shard::new();
            for i in 0..CACHE_ENTRIES_MAX {
                let e = CacheEntry {
                    repo: RepoId(DbId(7)),
                    scope: MAIN.into(),
                    key: format!("k{i}").into(),
                    version: "v".into(),
                    size: 1,
                    created: Timestamp(T0 + i as u64),
                    last_used: Timestamp(T0 + i as u64),
                    state: CacheState::Committed,
                };
                sh.f.more.more.entries.insert(CacheId(i as u64), e);
            }
            sh.f.more.more.next = CACHE_ENTRIES_MAX as u64 - 1;
            sh.f.more.more.entries.get_mut(&CacheId(0)).unwrap().last_used = Timestamp(T0 + 10_000);
            let id = sh.reserve(101, "new", 1).unwrap();
            assert_eq!(sh.f.more.more.entries.len(), CACHE_ENTRIES_MAX);
            assert!(
                sh.f.more.more.entries.contains_key(&CacheId(0)) && !sh.f.more.more.entries.contains_key(&CacheId(1))
            );
            assert!(sh.f.more.more.entries.contains_key(&id));
        }

        /// `CreateRunOn` records the base branch beside the run; a follower refuses a base for another run.
        #[test]
        fn a_pull_request_run_records_its_base() {
            let mut sh = Shard::new();
            sh.f.ids.apply_block(IdBlock::new(DbId(500), 10).unwrap());
            let l = LabelSet::parse(["linux"]).unwrap();
            let op = ActionsOp::CreateRunOn {
                run: run(0, "refs/pull/3/merge", "pull_request"),
                jobs: vec![job(0, &l)],
                permissions: Vec::new(),
                base_ref: "refs/heads/rel".into(),
            };
            let ActionsOut::Run(r) = sh.run(ActionsActor::Operator, op).unwrap() else { panic!() };
            assert_eq!(sh.f.run_base(r), Some("refs/heads/rel"));
            // Its jobs' ID tokens carry the base as GitHub does, the short name (F121).
            let row = sh.f.run_jobs(r)[0].clone();
            let rrow = &sh.f.run(r).unwrap().run;
            let facts = crate::oidc::Facts {
                server: "https://graft.example",
                repository: "octo/hello",
                repository_id: RepoId(DbId(7)),
                owner_id: graftyard_types::AccountId(DbId(1)),
                visibility: graftyard_identity::Visibility::Private,
                enterprise: None,
                run: rrow,
                job: &row,
                hosted: false,
                base_ref: sh.f.run_base(r),
                head_ref: None,
            };
            assert_eq!(crate::oidc::claims(&facts, None, 1, "j")["base_ref"], serde_json::json!("rel"));
            // The run's job reads `rel`'s entry over a newer one on `main`, then `main`'s once `rel` has none.
            let pj = sh.f.run_jobs(r)[0].spec.id;
            let rr = RunnerId(DbId(999));
            let m = ActionsEvent::Job { job: pj, version: 0, event: JobEvent::Matched(rr) };
            let a = ActionsEvent::Job { job: pj, version: 1, event: JobEvent::Acquired };
            sh.apply(vec![m, a]);
            let on_rel = CacheEntry {
                repo: RepoId(DbId(7)),
                scope: "refs/heads/rel".into(),
                key: "npm-rel".into(),
                version: "v".into(),
                size: 1,
                created: Timestamp(T0),
                last_used: Timestamp(T0),
                state: CacheState::Committed,
            };
            let on_main =
                CacheEntry { scope: MAIN.into(), key: "npm-main".into(), created: Timestamp(T0 + 9), ..on_rel.clone() };
            sh.f.more.more.entries.insert(CacheId(50), on_rel);
            sh.f.more.more.entries.insert(CacheId(51), on_main);
            let found = |f: &ActionsFold| f.cache_find(pj, &["npm-x", "npm-"], "v", MAIN).unwrap().map(|(id, _)| id);
            assert_eq!(found(&sh.f), Some(CacheId(50)));
            sh.f.more.more.entries.remove(&CacheId(50));
            assert_eq!(found(&sh.f), Some(CacheId(51)));
            assert_eq!(sh.f.cache_find(JobId(DbId(999_999)), &["npm-"], "v", MAIN).err(), Some(ActionsErr::NotFound));
            let sc = sh.f.scratch_at(Timestamp(sh.at), TierSet::CORE);
            assert!(sc.check(&ActionsEvent::RunBase { run: RunId(DbId(1)), base_ref: "x".into() }).is_err());
        }

        /// `CreateRunFrom` (0x15.7) records the base and the head beside the run, and its jobs' tokens carry both as
        /// short names; `CreateRunOn` records no head. A follower refuses a head for another run, and a fold with no
        /// head row keeps 0x15.6's bytes, so the site day that ships this reader changes no stored fold.
        #[test]
        fn a_same_repository_pull_request_run_records_its_head() {
            let mut sh = Shard::new();
            sh.f.ids.apply_block(IdBlock::new(DbId(500), 10).unwrap());
            let l = LabelSet::parse(["linux"]).unwrap();
            let on = ActionsOp::CreateRunOn {
                run: run(0, "refs/pull/2/merge", "pull_request"),
                jobs: vec![job(0, &l)],
                permissions: Vec::new(),
                base_ref: "refs/heads/rel".into(),
            };
            let ActionsOut::Run(r0) = sh.run(ActionsActor::Operator, on).unwrap() else { panic!() };
            assert_eq!((sh.f.run_base(r0), sh.f.run_head(r0)), (Some("refs/heads/rel"), None));
            let rows = &sh.f.more.more;
            let v6 = postcard::to_allocvec(&(&rows.entries, rows.next, &rows.bases)).unwrap();
            assert_eq!(postcard::to_allocvec(rows).unwrap(), v6, "no head row writes 0x15.6's cache rows");
            let before = postcard::to_allocvec(&sh.f).unwrap();
            assert_eq!(postcard::from_bytes::<ActionsFold>(&before).unwrap(), sh.f, "0x15.6's bytes read back");
            let op = ActionsOp::CreateRunFrom {
                run: run(1, "refs/pull/3/merge", "pull_request"),
                jobs: vec![job(1, &l)],
                permissions: Vec::new(),
                base_ref: "refs/heads/main".into(),
                head_ref: "refs/heads/fix/a".into(),
            };
            assert_eq!(ActionsCmd { actor: ActionsActor::Operator, op: op.clone() }.format(), ACTIONS_FORMAT_HEAD_REF);
            let ActionsOut::Run(r) = sh.run(ActionsActor::Operator, op).unwrap() else { panic!() };
            assert_eq!((sh.f.run_base(r), sh.f.run_head(r)), (Some("refs/heads/main"), Some("refs/heads/fix/a")));
            assert_eq!(sh.f.run_head(r0), None, "the other run has no head");
            let row = sh.f.run_jobs(r)[0].clone();
            let rrow = &sh.f.run(r).unwrap().run;
            let facts = crate::oidc::Facts {
                server: "https://graft.example",
                repository: "octo/hello",
                repository_id: RepoId(DbId(7)),
                owner_id: graftyard_types::AccountId(DbId(1)),
                visibility: graftyard_identity::Visibility::Private,
                enterprise: None,
                run: rrow,
                job: &row,
                hosted: false,
                base_ref: sh.f.run_base(r),
                head_ref: sh.f.run_head(r),
            };
            let c = crate::oidc::claims(&facts, None, 1, "j");
            assert_eq!((c["base_ref"].as_str(), c["head_ref"].as_str()), (Some("main"), Some("fix/a")));
            let after = postcard::to_allocvec(&sh.f).unwrap();
            assert_eq!(postcard::from_bytes::<ActionsFold>(&after).unwrap(), sh.f);
            let sc = sh.f.scratch_at(Timestamp(sh.at), TierSet::CORE);
            assert!(sc.check(&ActionsEvent::RunHead { run: RunId(DbId(1)), head_ref: "x".into() }).is_err());
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
