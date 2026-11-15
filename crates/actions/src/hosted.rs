//! Hosted runners on the install side (`design/SHARED.md`, "Actions", Hosted runners; `build/BUILD-PLAN.md`, M4-WP4):
//! the hosted label table, the per-owner cap that `decide` enforces on hosted registrations (`admit`, called from
//! `crate::shard`), and `demand`, which the owner's `Owner`-scope broker answers from its 16 shards' summaries.
//!
//! - **Registration.** Every hosted runner of an owner, org or user, registers at `RunnerScope::Owner` on
//!   `OWNER_SCOPE_SHARD` with exactly one table row's labels, so one group's `decide` counts all of them against the
//!   cap, and two overlapping demand reads never start more (`decisions/DECISIONS.md`, "M4-WP4").
//! - **Staleness** needs times the fold does not keep (when a runner was first seen, whether it ever held a job), so
//!   the broker keeps them (`Seen`) in its object's storage, read before and written after each demand read. In
//!   memory alone they did not last: an object idle for seconds leaves memory, so with reads 34 s or more apart every
//!   read saw each runner afresh and a runner whose Sandbox never started held its cap slot and its job's start for
//!   good [measured, `tests/m4/results/m4-wp4.md`, "The smoke run"].

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use graftyard_types::Timestamp;
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::ids::RunnerId;
use crate::job::JobState;
use crate::labels::LabelSet;
use crate::shard::{ActionsActor, ActionsErr, ActionsFold, ActionsScratch, RunnerRecord, RunnerScope};

/// One row of SHARED's hosted label table. Every value is a [guess] (SHARED).
#[derive(Debug, PartialEq, Eq)]
pub struct HostedLabel {
    /// The labels a runner of this row registers with; the first names the row.
    pub labels: &'static [&'static str],
    pub vcpu: u8,
    pub memory_gib: u8,
    pub disk_gb: u8,
}

pub const HOSTED_LABELS: &[HostedLabel] = &[
    HostedLabel { labels: &["ubuntu-latest", "ubuntu-24.04"], vcpu: 2, memory_gib: 8, disk_gb: 16 },
    HostedLabel { labels: &["ubuntu-22.04"], vcpu: 2, memory_gib: 8, disk_gb: 16 },
    HostedLabel { labels: &["ubuntu-24.04-4core"], vcpu: 4, memory_gib: 12, disk_gb: 20 },
];

impl HostedLabel {
    pub fn name(&self) -> &'static str {
        self.labels[0]
    }

    pub fn set(&self) -> LabelSet {
        LabelSet::parse(self.labels).expect("the table's labels parse")
    }

    /// The row a job's labels ask for: the first row that holds all of them. A job with no labels takes none.
    pub fn for_job(job: &LabelSet) -> Option<&'static HostedLabel> {
        job.iter().next()?;
        HOSTED_LABELS.iter().find(|row| job.fits(&row.set()))
    }

    /// The row a hosted registration names: its labels are exactly that row's.
    pub fn of_runner(labels: &LabelSet) -> Option<&'static HostedLabel> {
        HOSTED_LABELS.iter().find(|row| row.set() == *labels)
    }

    pub fn named(name: &str) -> Option<&'static HostedLabel> {
        HOSTED_LABELS.iter().find(|row| row.name() == name)
    }
}

/// Hosted runners an owner runs at once, unless its `OrgPolicy` sets another cap (SHARED) [guess]. No `OrgPolicy`
/// field carries a cap yet, so every owner has this one.
pub const HOSTED_CAP: u16 = 20;
/// A hosted runner that has held no job this long after the broker first saw it is stale (SHARED) [guess].
pub const HOSTED_IDLE: Duration = Duration::from_secs(600);
/// `graft-runners` starts no Sandbox past this share of the runner account's vCPU (DESIGN, Shape, `graft-runners`).
pub const RUNNER_VCPU_STOP_PCT: u8 = 70;

/// `decide`'s rule for a hosted registration (`PutRunner` with `hosted`): an ephemeral `Owner`-scope runner whose
/// labels are one table row's, refused `TooMany` once the owner has `HOSTED_CAP` hosted runners registered, those
/// committed earlier in the same entry included. The edge sends every hosted registration to `OWNER_SCOPE_SHARD`, so
/// this one group sees them all.
pub fn admit(s: &ActionsScratch<'_>, r: &RunnerRecord) -> Result<(), ActionsErr> {
    if r.scope != RunnerScope::Owner || !r.ephemeral || HostedLabel::of_runner(&r.labels).is_none() {
        return Err(ActionsErr::Illegal(
            "a hosted runner is ephemeral, owner-scoped, and one hosted label row's".into(),
        ));
    }
    if s.runners().filter(|(_, x)| x.hosted).count() >= usize::from(HOSTED_CAP) {
        return Err(ActionsErr::TooMany);
    }
    Ok(())
}

/// `decide`'s rule for deleting a hosted registration. An ephemeral runner deletes its own at the end of its one job,
/// which frees its place under the cap at once, and its Sandbox destroys itself when the runner exits
/// (`workers/runners`). The hosted-runner service deletes a `stale` runner before it destroys that runner's Sandbox,
/// and its delete is refused while the runner holds a job, so an idle runner matched since the service's demand read
/// keeps its job and its Sandbox.
pub fn may_delete(s: &ActionsScratch<'_>, actor: &ActionsActor, id: RunnerId) -> Result<(), ActionsErr> {
    if *actor != ActionsActor::Runner(id) && s.jobs().any(|(_, j)| j.state.runner() == Some(id)) {
        return Err(ActionsErr::Illegal("the hosted runner holds a job".into()));
    }
    Ok(())
}

/// What one shard tells its owner's broker: its queued jobs by label set (count and oldest), the runners holding a
/// job, and the hosted runners registered on it (only `OWNER_SCOPE_SHARD` has any). The site answers it for
/// `/read {"q": {"hosted": true}}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShardSummary {
    pub queued: Vec<(LabelSet, u32, Timestamp)>,
    pub held: Vec<RunnerId>,
    pub hosted: Vec<(RunnerId, LabelSet)>,
}

pub fn summary(f: &ActionsFold) -> ShardSummary {
    let mut queued: BTreeMap<&LabelSet, (u32, Timestamp)> = BTreeMap::new();
    let mut held = Vec::new();
    for j in f.jobs.values() {
        match &j.state {
            JobState::Queued { labels, since } => {
                let e = queued.entry(labels).or_insert((0, *since));
                e.0 += 1;
                e.1 = e.1.min(*since);
            }
            s => held.extend(s.runner()),
        }
    }
    ShardSummary {
        queued: queued.into_iter().map(|(l, (n, t))| (l.clone(), n, t)).collect(),
        held,
        hosted: f.runners.iter().filter(|(_, r)| r.hosted).map(|(id, r)| (*id, r.labels.clone())).collect(),
    }
}

/// SHARED's `LabelDemand`. `start` always names the owner's `Owner` scope, a user's as an org's, because the cap is
/// counted on that one group (`decisions/DECISIONS.md`, "M4-WP4").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelDemand {
    pub queued: u32,
    pub oldest: Option<Timestamp>,
    pub start: SmallVec<[(RunnerScope, u16); 2]>,
}

/// SHARED's `HostedDemand`, the body of `GET /api/graftyard/v1/owners/{owner}/actions/hosted/demand`. `runners` is
/// every hosted runner the owner has registered, with its row's name: `graft-runners` checks each one's Sandbox for
/// an exited runner, and sums their vCPU against `RUNNER_VCPU_STOP_PCT`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedDemand {
    pub labels: Vec<(Box<str>, LabelDemand)>,
    pub stale: Vec<RunnerId>,
    #[serde(default)]
    pub runners: Vec<(RunnerId, Box<str>)>,
}

impl HostedDemand {
    pub fn starts(&self) -> u32 {
        self.labels.iter().flat_map(|(_, d)| d.start.iter()).map(|(_, n)| u32::from(*n)).sum()
    }

    /// The vCPU the owner's registered hosted runners take; a row this release does not know counts as 4.
    pub fn vcpu(&self) -> u32 {
        self.runners.iter().map(|(_, l)| HostedLabel::named(l).map_or(4, |r| u32::from(r.vcpu))).sum()
    }
}

/// The record a hosted registration commits: ephemeral, `Owner`-scoped, with exactly the row's labels. `key` is the
/// runner's RSA public key, SPKI DER.
pub fn record(row: &HostedLabel, name: &str, key: Box<[u8]>, version: &str) -> RunnerRecord {
    RunnerRecord {
        scope: RunnerScope::Owner,
        name: name.into(),
        labels: row.set(),
        key,
        ephemeral: true,
        hosted: true,
        version: version.into(),
    }
}

/// The broker's memory of each hosted runner it has seen registered: when it first saw it, and whether it ever saw it
/// hold a job. A runner is forgotten once its registration is gone. The broker stores it as JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    first: BTreeMap<RunnerId, Timestamp>,
    held: BTreeSet<RunnerId>,
}

/// The owner's hosted demand from all its shards' summaries at `now`.
///
/// - **Stale**: a hosted runner holding no job that the broker saw hold one (its job completed), that has been idle
///   `HOSTED_IDLE` since the broker first saw it, whose labels name no row, or that is idle beyond its row's queued
///   jobs, the most recently seen first. Without the last rule, two overlapping reads near the end of a queue each
///   start runners for the same jobs, and the spare ones live out `HOSTED_IDLE` after the last job.
/// - **Start**, per row: the queued jobs no runner holds, less the row's idle runners that are not stale, within what
///   the cap leaves after every registered hosted runner, stale ones included until they are deleted.
pub fn demand(shards: &[ShardSummary], cap: u16, now: Timestamp, seen: &mut Seen) -> HostedDemand {
    let held: BTreeSet<RunnerId> = shards.iter().flat_map(|s| s.held.iter().copied()).collect();
    let hosted: BTreeMap<RunnerId, &LabelSet> =
        shards.iter().flat_map(|s| s.hosted.iter().map(|(r, l)| (*r, l))).collect();
    seen.first.retain(|r, _| hosted.contains_key(r));
    seen.held.retain(|r| hosted.contains_key(r));
    for r in hosted.keys() {
        seen.first.entry(*r).or_insert(now);
        if held.contains(r) {
            seen.held.insert(*r);
        }
    }

    let mut queued: BTreeMap<&str, (u32, Timestamp)> = BTreeMap::new();
    for (labels, n, oldest) in shards.iter().flat_map(|s| s.queued.iter()) {
        if let Some(row) = HostedLabel::for_job(labels) {
            let e = queued.entry(row.name()).or_insert((0, *oldest));
            e.0 += n;
            e.1 = e.1.min(*oldest);
        }
    }

    let mut stale = Vec::new();
    let mut idle: BTreeMap<&str, Vec<(Timestamp, RunnerId)>> = BTreeMap::new();
    let mut runners = Vec::new();
    for (r, labels) in &hosted {
        let row = HostedLabel::of_runner(labels);
        runners.push((*r, row.map_or_else(|| labels.iter().collect::<Vec<_>>().join(",").into(), |x| x.name().into())));
        if held.contains(r) {
            continue;
        }
        let first = seen.first[r];
        match row {
            Some(row) if !seen.held.contains(r) && now.0 < first.0 + HOSTED_IDLE.as_millis() as u64 => {
                idle.entry(row.name()).or_default().push((first, *r));
            }
            _ => stale.push(*r),
        }
    }

    let mut room = cap.saturating_sub(u16::try_from(hosted.len()).unwrap_or(u16::MAX));
    let mut labels = Vec::new();
    for row in HOSTED_LABELS {
        let (q, oldest) = queued.get(row.name()).map_or((0, None), |(n, t)| (*n, Some(*t)));
        let mut idle = idle.remove(row.name()).unwrap_or_default();
        idle.sort();
        while idle.len() > q as usize {
            stale.push(idle.pop().expect("more idle than queued").1);
        }
        let n = u16::try_from(q as usize - idle.len()).unwrap_or(u16::MAX).min(room);
        room -= n;
        if q > 0 {
            let start = if n > 0 { smallvec![(RunnerScope::Owner, n)] } else { SmallVec::new() };
            labels.push((row.name().into(), LabelDemand { queued: q, oldest, start }));
        }
    }
    stale.sort();
    HostedDemand { labels, stale, runners }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_types::DbId;

    fn r(n: i64) -> RunnerId {
        RunnerId(DbId(n))
    }

    fn l(labels: &[&str]) -> LabelSet {
        LabelSet::parse(labels).unwrap()
    }

    fn shard(queued: u32, hosted: &[i64], held: &[i64]) -> ShardSummary {
        ShardSummary {
            queued: if queued > 0 { vec![(l(&["ubuntu-latest"]), queued, Timestamp(5))] } else { Vec::new() },
            held: held.iter().map(|n| r(*n)).collect(),
            hosted: hosted.iter().map(|n| (r(*n), l(&["ubuntu-latest", "ubuntu-24.04"]))).collect(),
        }
    }

    #[test]
    fn jobs_and_runners_find_their_rows() {
        assert_eq!(HostedLabel::for_job(&l(&["ubuntu-latest"])).unwrap().vcpu, 2);
        assert_eq!(HostedLabel::for_job(&l(&["Ubuntu-24.04"])).unwrap().name(), "ubuntu-latest");
        assert_eq!(HostedLabel::for_job(&l(&["ubuntu-24.04-4core"])).unwrap().vcpu, 4);
        assert_eq!(HostedLabel::for_job(&l(&["self-hosted", "linux"])), None);
        assert_eq!(HostedLabel::for_job(&l(&["ubuntu-latest", "gpu"])), None, "a label no row has");
        assert_eq!(HostedLabel::for_job(&LabelSet::default()), None);
        assert_eq!(HostedLabel::of_runner(&l(&["ubuntu-24.04", "ubuntu-latest"])).unwrap().name(), "ubuntu-latest");
        assert_eq!(HostedLabel::of_runner(&l(&["ubuntu-latest"])), None, "only a whole row");
    }

    /// Start is the queued jobs less the idle runners, within the cap's room after every registered runner; a busy
    /// runner is never stale; a runner idle `HOSTED_IDLE` is, and so is one whose job completed.
    #[test]
    fn demand_counts_idle_runners_and_stays_under_the_cap() {
        let mut seen = Seen::default();
        // 30 queued on another shard, 18 hosted runners, 16 busy: start 2, the cap's room, not 28.
        let held: Vec<i64> = (1..=16).collect();
        let hosted: Vec<i64> = (1..=18).collect();
        let d = demand(&[shard(0, &hosted, &held), shard(30, &[], &[])], 20, Timestamp(1_000), &mut seen);
        assert_eq!((d.starts(), d.stale.len()), (2, 0), "{d:?}");
        assert_eq!((d.labels[0].1.queued, d.labels[0].1.oldest), (30, Some(Timestamp(5))));
        assert_eq!((d.runners.len(), d.runners[0].clone(), d.vcpu()), (18, (r(1), "ubuntu-latest".into()), 36));
        // Runners 1 and 2 held jobs above; now idle, they are stale though 5 jobs wait, so all 5 start afresh.
        let d = demand(&[shard(5, &[1, 2], &[])], 20, Timestamp(1_500), &mut seen);
        assert_eq!((d.stale.clone(), d.starts()), (vec![r(1), r(2)], 5));
        // 3 queued, 2 of 3 new runners idle: start 1.
        let mut seen = Seen::default();
        let d = demand(&[shard(3, &[1, 2, 3], &[3])], 20, Timestamp(2_000), &mut seen);
        assert_eq!((d.starts(), d.stale.clone()), (1, vec![]));
        // A runner that held a job and now holds none is stale; a busy one never is, however old.
        let t = Timestamp(2_000 + HOSTED_IDLE.as_millis() as u64);
        let d = demand(&[shard(5, &[1, 2, 3], &[1])], 20, t, &mut seen);
        assert_eq!(d.stale, vec![r(2), r(3)], "2 idle since first seen, 3 finished its job");
        assert_eq!(d.starts(), 5, "stale runners are not counted as idle");
        assert_eq!(d.labels[0].1.start.as_slice(), &[(RunnerScope::Owner, 5)]);
    }

    /// The cap's room is shared by every row: 15 `ubuntu-latest` and 10 4-core jobs under a cap of 20 start 15 and 5.
    #[test]
    fn rows_share_the_caps_room() {
        let s = ShardSummary {
            queued: vec![(l(&["ubuntu-latest"]), 15, Timestamp(5)), (l(&["ubuntu-24.04-4core"]), 10, Timestamp(6))],
            ..ShardSummary::default()
        };
        let d = demand(&[s], 20, Timestamp(10), &mut Seen::default());
        let starts: Vec<(&str, u16)> = d.labels.iter().map(|(n, x)| (&**n, x.start[0].1)).collect();
        assert_eq!(starts, vec![("ubuntu-latest", 15), ("ubuntu-24.04-4core", 5)]);
    }

    /// The broker keeps `Seen` as JSON in its storage and every demand read starts from what it stored, since an
    /// object idle for seconds leaves memory: a runner then still goes stale `HOSTED_IDLE` after its first sight, and
    /// one that held a job stays marked. Kept in memory only, runner 1704299 of the smoke run never went stale.
    #[test]
    fn seen_ages_runners_across_stored_reads() {
        let read = |hosted: &[i64], held: &[i64], t: u64, stored: &str| {
            let mut seen: Seen = serde_json::from_str(stored).unwrap();
            let d = demand(&[shard(2, hosted, held)], 20, Timestamp(t), &mut seen);
            (d, serde_json::to_string(&seen).unwrap())
        };
        let idle = HOSTED_IDLE.as_millis() as u64;
        let (_, j) = read(&[1, 2], &[2], 1_000, &serde_json::to_string(&Seen::default()).unwrap());
        let (d, j) = read(&[1, 2], &[], 1_000 + idle - 1, &j);
        assert_eq!(d.stale, vec![r(2)], "2 held a job and holds none; 1 is not yet idle HOSTED_IDLE");
        let (d, _) = read(&[1, 2], &[], 1_000 + idle, &j);
        assert_eq!((d.starts(), d.stale), (2, vec![r(1), r(2)]), "1 idle HOSTED_IDLE since its first sight");
    }

    /// Idle runners beyond the queued jobs are stale, the most recently seen first, and an empty queue starts nothing.
    #[test]
    fn idle_runners_past_the_queue_are_stale_newest_first() {
        let mut seen = Seen::default();
        demand(&[shard(1, &[7], &[])], 20, Timestamp(10), &mut seen);
        let d = demand(&[shard(1, &[4, 7], &[])], 20, Timestamp(20), &mut seen);
        assert_eq!((d.stale.clone(), d.starts()), (vec![r(4)], 0), "4 was seen after 7");
        let d = demand(&[shard(0, &[4, 7], &[])], 20, Timestamp(30), &mut seen);
        assert_eq!((d.stale, d.labels.len()), (vec![r(4), r(7)], 0));
    }

    /// The summary counts only queued jobs, by label set, with the oldest one's time, and lists the runners holding jobs
    /// and the hosted registrations.
    #[test]
    fn a_summary_counts_queued_jobs_and_names_holders() {
        use crate::shard::{ACTIONS_FORMAT, ActionsEvent, JobSpec, WorkflowRun};
        use crate::{JobEvent, JobId, RunId};
        use graftyard_raft::{EntryBody, Fold, LogEntry, PREV_ZERO, Term, TierSet};
        use graftyard_types::{ChainId, Index, RepoId};
        let spec = |n: i64| JobSpec {
            id: JobId(DbId(n)),
            name: "j".into(),
            labels: l(&["ubuntu-latest"]),
            needs: SmallVec::new(),
            matrix: SmallVec::new(),
            environment: None,
            steps: "[]".into(),
        };
        let run = |id: i64, repo: i64| WorkflowRun {
            id: RunId(DbId(id)),
            repo: RepoId(DbId(repo)),
            workflow: "ci.yml".into(),
            event: "push".into(),
            head_sha: [1; 20],
            head_ref: "refs/heads/main".into(),
            trigger: (ChainId::Repo(RepoId(DbId(repo))), Index(id as u64)),
            attempt: 1,
            concurrency: None,
            fork: false,
        };
        let entry = |i: u64, at: u64, evs| LogEntry {
            term: Term(1),
            index: Index(i),
            prev: PREV_ZERO,
            format: ACTIONS_FORMAT,
            tiers: TierSet::CORE,
            at: Timestamp(at),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(evs),
        };
        let mut f = ActionsFold::default();
        f.apply_entry(&entry(1, 100, vec![ActionsEvent::RunCreated { run: run(1, 8), jobs: vec![spec(2), spec(3)] }]));
        f.apply_entry(&entry(2, 50, vec![ActionsEvent::RunCreated { run: run(4, 9), jobs: vec![spec(5)] }]));
        let rec = RunnerRecord {
            scope: RunnerScope::Owner,
            name: "h".into(),
            labels: l(&["ubuntu-latest", "ubuntu-24.04"]),
            key: Box::new([1]),
            ephemeral: true,
            hosted: true,
            version: "2.338.2".into(),
        };
        let m = ActionsEvent::Job { job: JobId(DbId(3)), version: 0, event: JobEvent::Matched(r(6)) };
        f.apply_entry(&entry(3, 200, vec![ActionsEvent::Runner { id: r(6), record: Some(rec) }, m]));
        let s = summary(&f);
        assert_eq!(s.queued, vec![(l(&["ubuntu-latest"]), 2, Timestamp(50))]);
        assert_eq!((s.held, s.hosted.len()), (vec![r(6)], 1));
    }
}
