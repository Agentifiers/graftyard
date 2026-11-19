//! `ActionsTrigger`'s pure half for `on: merge_group` (`design/SHARED.md`, "Actions runs and jobs" and "Event fan-out";
//! `build/BUILD-PLAN.md`, M4-WP3, BF-37) and `on: pull_request` (B0-35; `decisions/DECISIONS.md`, "`pr-trigger`"):
//! which queue refs and pull request events a committed repository entry holds, and the run a workflow file starts on
//! each. The host reads `.github/workflows/*` at each ref's tip, or at the pull request's current test merge, from the
//! git cache, commits each run's `CreateRun` on the repository's Actions shard, and then the `ActionsTrigger` cursor
//! (`crate::shard`'s run key makes a replay create nothing twice).

use graftyard_fold::Read;
use graftyard_git::hooks::{Subject, webhooks};
use graftyard_git::issues::{IssueNumber, MergeabilityResult};
use graftyard_git::prsync::PrView;
use graftyard_git::queue::{QueueEvent, queue_ref};
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_git::repo::RepoEvent;
use graftyard_raft::{EntryBody, LogEntry};
use graftyard_types::{ChainId, DbId, Index, RepoId};

use crate::ids::RunId;
use crate::shard::{JobSpec, WorkflowRun};
use crate::workflow::filter::{Trigger, triggers};

/// One queue ref a `GroupBuilt` created: its base branch, its name, and its tip (the entry's `group_tip`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub base: RefName,
    pub queue_ref: RefName,
    pub tip: ObjectId,
}

/// Every queue ref entry `e` built, in order. A `GroupBuilt` that rebuilds from the base tip names every entry it
/// builds, so each new ref is listed once, by the entry that created it.
pub fn built(e: &LogEntry<RepoEvent>) -> Vec<Built> {
    let EntryBody::Events(evs) = &e.body else { return Vec::new() };
    let mut out = Vec::new();
    for ev in evs {
        if let RepoEvent::Queue(QueueEvent::GroupBuilt { base, entries, .. }) = ev {
            for x in entries {
                if let Some(r) = queue_ref(base, x) {
                    out.push(Built { base: base.clone(), queue_ref: r, tip: x.group_tip });
                }
            }
        }
    }
    out
}

/// The run workflow file `path` (source `src`) starts on queue ref `b`, built by entry `index` of `repo`'s chain:
/// `Ok(None)` when its `on:` takes no `merge_group.checks_requested` for `b`'s base, `Err` when it does not parse or
/// its jobs cannot run, which GitHub reports as a failed run and this skips [inference: no gate needs the failed run].
pub fn merge_group_run(
    repo: RepoId,
    index: Index,
    path: &str,
    src: &str,
    b: &Built,
) -> Result<Option<(WorkflowRun, Vec<JobSpec>, crate::specs::Declared)>, String> {
    let parsed = crate::workflow::parse(path, src);
    if !parsed.ok() {
        return Err(parsed.errors.join("; "));
    }
    let base = b.base.display();
    let t = Trigger::MergeGroup { action: "checks_requested", base: &base };
    if !parsed.workflow.events.as_ref().is_some_and(|ev| triggers(ev, &t)) {
        return Ok(None);
    }
    let (jobs, declared) = crate::specs::job_specs_declared(&parsed.workflow)?;
    let run = WorkflowRun {
        id: RunId(DbId(0)),
        repo,
        workflow: path.into(),
        event: "merge_group".into(),
        head_sha: b.tip.0,
        head_ref: b.queue_ref.display().into(),
        trigger: (ChainId::Repo(repo), index),
        attempt: 1,
        concurrency: None,
        fork: false,
    };
    Ok(Some((run, jobs, declared)))
}

/// An entry older than this starts no `pull_request` run: a first scan of a repository's log, where a site day just set
/// `GRAFTYARD_PR_TRIGGER`, would otherwise start a run for every push it ever took. Install-side only; GitHub's
/// trigger never replays (`decisions/DECISIONS.md`, "`pr-trigger`").
pub const PR_REPLAY_MS: u64 = 3_600_000;

/// One `pull_request` event that starts runs: GitHub's default activity types (`opened`, `synchronize`, `reopened`).
/// `after` is a `synchronize`'s new head, which the instrument compares with the head when the run is created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrEvent {
    pub n: IssueNumber,
    pub action: &'static str,
    pub after: Option<ObjectId>,
}

/// The `pull_request` events of entry `e` that start runs, read as `Webhooks` reads them (`graftyard_git::hooks`),
/// or none when `e` is more than PR_REPLAY_MS older than `now`.
pub fn pr_events(core: &dyn Read, repo: RepoId, e: &LogEntry<RepoEvent>, now: u64) -> Vec<PrEvent> {
    if now.saturating_sub(e.at.0) > PR_REPLAY_MS {
        return Vec::new();
    }
    webhooks(core, repo, e)
        .into_iter()
        .filter(|w| w.event == "pull_request")
        .filter_map(|w| match (w.action, w.subject) {
            (Some(a @ ("opened" | "synchronize" | "reopened")), Subject::Pull { n, after, .. }) => {
                Some(PrEvent { n, action: a, after })
            }
            _ => None,
        })
        .collect()
}

/// Where a pull request's runs go now, from its view on the leader (`graftyard_git::prsync::views`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrTarget {
    /// The latest test merge of the current head, at whichever base tip it was computed on: what `refs/pull/N/merge`
    /// holds, GitHub's `GITHUB_SHA`. Its commit is the runs' `head_sha`.
    Merge(ObjectId),
    /// That merge conflicts, and GitHub starts no run.
    Conflict,
    /// An open pull request with no test merge of its current head: the host asks for one (`demand`,
    /// `MergeCause::Dispatch`).
    Pending,
    /// A fork's, whose head this chain does not hold [inference: no route opens a fork's pull request yet], or one
    /// closed before any merge of its head was computed: no run.
    None,
}

/// A merge at an older base tip still counts: requiring the base tip now left B0-35's dispatch, whose base moves every
/// 6 s, waiting on a merge that is stale by the time it commits, round after round (`decisions/DECISIONS.md`,
/// "`pr-trigger`").
pub fn pr_target(v: &PrView) -> PrTarget {
    if !v.same_repo {
        return PrTarget::None;
    }
    match &v.merge {
        Some(m) if m.head == v.head => match m.result {
            MergeabilityResult::Clean { merge } => PrTarget::Merge(merge),
            MergeabilityResult::Conflict { .. } => PrTarget::Conflict,
        },
        _ if v.open => PrTarget::Pending,
        _ => PrTarget::None,
    }
}

/// The run workflow file `path` (source `src`) starts for pull request `n`'s event `action`, from entry `index` of
/// `repo`'s chain, at the pull request's current test merge `merge` into `base`: `Ok(None)` when its `on:` takes no
/// such `pull_request`, `Err` when it does not parse or its jobs cannot run. The run's ref is `refs/pull/N/merge` and
/// its commit the merge [doc: GitHub's "Events that trigger workflows", `pull_request`: `GITHUB_SHA` is the "Last merge
/// commit on the `GITHUB_REF` branch"], so an event whose head moved on still runs, at the merge current now.
#[allow(clippy::too_many_arguments)]
pub fn pull_request_run(
    repo: RepoId,
    index: Index,
    path: &str,
    src: &str,
    n: IssueNumber,
    action: &str,
    base: &RefName,
    merge: ObjectId,
) -> Result<Option<(WorkflowRun, Vec<JobSpec>, crate::specs::Declared)>, String> {
    let parsed = crate::workflow::parse(path, src);
    if !parsed.ok() {
        return Err(parsed.errors.join("; "));
    }
    let base = base.display();
    let t = Trigger::PullRequest { action, base: &base, paths: None };
    if !parsed.workflow.events.as_ref().is_some_and(|ev| triggers(ev, &t)) {
        return Ok(None);
    }
    let (jobs, declared) = crate::specs::job_specs_declared(&parsed.workflow)?;
    let run = WorkflowRun {
        id: RunId(DbId(0)),
        repo,
        workflow: path.into(),
        event: "pull_request".into(),
        head_sha: merge.0,
        head_ref: graftyard_git::tiers::merge_ref(n).display().into(),
        trigger: (ChainId::Repo(repo), index),
        attempt: 1,
        concurrency: None,
        fork: false,
    };
    Ok(Some((run, jobs, declared)))
}

/// One unit of `ActionsTrigger`'s work from entry `index` (stamped `at`, ms) whose runs have not committed yet: a
/// queue ref the entry built, or one of its pull request events. A round tries each item once; one not done is carried
/// to the next round instead of failing the round, so the rest of its batch is not run again (F84;
/// `decisions/DECISIONS.md`, "`pr-trigger`").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carried {
    pub index: Index,
    pub at: u64,
    pub item: Item,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Queue(Built),
    Pr(PrEvent),
}

/// How a round's try of one item ended. `Done`: its runs committed (or the shard already held them), or it starts
/// none (a conflict, a closed pull request, no matching workflow). `Again`: its test merge is still shed, or a read
/// or a `CreateRun` failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tried {
    Done,
    Again,
}

/// What a round leaves: the items carried to the next round, the pull request events given up (`unmerged`: still not
/// done more than PR_REPLAY_MS after their entry, the age at which the first-scan guard starts none), and the index the
/// cursor may commit. That is just below the oldest carried item's entry, so a new leader rescans it, or `through`,
/// the last entry the round read, when none is carried. A queue ref is never given up, as a failed round never gave it
/// up before.
pub fn settle(tried: Vec<(Carried, Tried)>, through: Index, now: u64) -> (Vec<Carried>, Vec<Carried>, Index) {
    let (mut carried, mut dropped) = (Vec::new(), Vec::new());
    for (c, t) in tried {
        match (t, &c.item) {
            (Tried::Done, _) => {}
            (Tried::Again, Item::Pr(_)) if now.saturating_sub(c.at) > PR_REPLAY_MS => dropped.push(c),
            (Tried::Again, _) => carried.push(c),
        }
    }
    let cursor = carried.iter().map(|c| Index(c.index.0.saturating_sub(1))).min().unwrap_or(through).min(through);
    (carried, dropped, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_git::issues::IssueNumber;
    use graftyard_git::queue::MergeQueueEntry;
    use graftyard_raft::{PREV_ZERO, Term, TierSet};
    use graftyard_types::Timestamp;
    use smallvec::SmallVec;

    fn name(s: &str) -> RefName {
        RefName::parse(s.as_bytes()).unwrap()
    }

    fn oid(n: u8) -> ObjectId {
        ObjectId([n; 20])
    }

    fn group(entries: &[(u32, u8, u8)]) -> LogEntry<RepoEvent> {
        let entries = entries
            .iter()
            .map(|(pr, tip, parent)| MergeQueueEntry {
                pr: IssueNumber(*pr),
                group_tip: oid(*tip),
                parent_tip: oid(*parent),
            })
            .collect();
        let ev = RepoEvent::Queue(QueueEvent::GroupBuilt {
            base: name("refs/heads/main"),
            entries,
            refs: Vec::new(),
            pack: None,
        });
        LogEntry {
            term: Term(2),
            index: Index(40),
            prev: PREV_ZERO,
            format: graftyard_git::merge::REPO_FORMAT_V4,
            tiers: TierSet::CORE,
            at: Timestamp(1),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![ev]),
        }
    }

    const WF: &str = "on:\n  merge_group:\n    branches: [main]\njobs:\n  ci:\n    runs-on: self-hosted\n    steps:\n      - run: echo ok\n";

    /// One `GroupBuilt` of two pull requests starts a run on each queue ref, at each ref's own tip, and the shard's run
    /// key tells the two apart although they share the entry and the workflow file (SHARED, Idempotent run creation).
    #[test]
    fn a_group_of_two_starts_a_run_on_each_queue_ref() {
        let e = group(&[(3, 0xa1, 0x01), (5, 0xb2, 0xa1)]);
        let bs = built(&e);
        assert_eq!(bs.len(), 2);
        let repo = RepoId(DbId(7));
        let runs: Vec<WorkflowRun> = bs
            .iter()
            .map(|b| merge_group_run(repo, e.index, ".github/workflows/q.yml", WF, b).unwrap().unwrap().0)
            .collect();
        assert_eq!(&*runs[1].head_ref, format!("refs/heads/gh-readonly-queue/main/pr-5-{}", oid(0xa1).hex()));
        assert_eq!((runs[0].head_sha, runs[1].head_sha), (oid(0xa1).0, oid(0xb2).0), "each run is on its ref's tip");
        assert!(runs.iter().all(|r| &*r.event == "merge_group" && r.trigger == (ChainId::Repo(repo), Index(40))));
        assert_ne!(runs[0].key(), runs[1].key(), "one entry, one file, two queue refs: two runs");
        // A replay of the same ref gives the same key, which `CreateRun` refuses a second time.
        let again = merge_group_run(repo, e.index, ".github/workflows/q.yml", WF, &bs[1]).unwrap().unwrap().0;
        assert_eq!(again.key(), runs[1].key());
    }

    /// Only a workflow whose `on:` takes `merge_group` for the queue's base runs; a push workflow or another base's
    /// `merge_group` does not, and a workflow that does not parse is an error, not a run.
    #[test]
    fn only_a_merge_group_workflow_for_the_base_runs() {
        let b = &built(&group(&[(3, 0xa1, 0x01)]))[0];
        let repo = RepoId(DbId(7));
        let run = |src: &str| merge_group_run(repo, Index(40), "w.yml", src, b);
        assert!(run(WF).unwrap().is_some());
        assert!(
            run("on: [push, merge_group]\njobs:\n  a:\n    runs-on: x\n    steps: [{run: a}]\n").unwrap().is_some()
        );
        assert!(run("on: push\njobs:\n  a:\n    runs-on: x\n    steps: [{run: a}]\n").unwrap().is_none());
        let other =
            "on:\n  merge_group:\n    branches: [release]\njobs:\n  a:\n    runs-on: x\n    steps: [{run: a}]\n";
        assert!(run(other).unwrap().is_none());
        assert!(run("on: merge_group\njobs: 3\n").is_err());
    }
}
