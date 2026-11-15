//! `ActionsTrigger`'s pure half for `on: merge_group` (`design/SHARED.md`, "Actions runs and jobs" and "Event fan-out";
//! `build/BUILD-PLAN.md`, M4-WP3, BF-37): which queue refs a committed repository entry built, and the run a workflow
//! file starts on one of them. The host reads `.github/workflows/*` at each ref's tip from the git cache, commits each
//! run's `CreateRun` on the repository's Actions shard, and then the `ActionsTrigger` cursor (`crate::shard`'s run key
//! makes a replay create nothing twice).

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
) -> Result<Option<(WorkflowRun, Vec<JobSpec>)>, String> {
    let parsed = crate::workflow::parse(path, src);
    if !parsed.ok() {
        return Err(parsed.errors.join("; "));
    }
    let base = b.base.display();
    let t = Trigger::MergeGroup { action: "checks_requested", base: &base };
    if !parsed.workflow.events.as_ref().is_some_and(|ev| triggers(ev, &t)) {
        return Ok(None);
    }
    let jobs = crate::specs::job_specs(&parsed.workflow)?;
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
    Ok(Some((run, jobs)))
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
