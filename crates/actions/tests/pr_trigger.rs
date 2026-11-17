//! B0-35's `pull_request` trigger (`decisions/DECISIONS.md`, "`pr-trigger`") on real members: a repository group whose
//! entries `ActionsTrigger` reads with `graftyard_actions::trigger`, and an Actions shard that takes each run's
//! `CreateRun`. Each test fails on one plausible bug: a run key that lets one entry start only one pull request's run,
//! a conflicting merge that still starts a run, a superseded event dropped or run at its own head instead of the
//! current merge, a merge counted stale once the base moves, a first scan that replays old entries, and a `CreateRun`
//! sent to a member that cannot read it.

mod common;

use common::Group;
use graftyard_actions::shard::{
    ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST, ActionsActor, ActionsCmd, ActionsErr, ActionsFold, ActionsOp,
    ActionsOut, WorkflowRun,
};
use graftyard_actions::trigger::{PR_REPLAY_MS, PrEvent, PrTarget, pr_events, pr_target, pull_request_run};
use graftyard_git::issues::{HeadSpec, IssueCapture, IssueNumber, MergeabilityResult, PrCapture, PrState};
use graftyard_git::prsync::{REPO_FORMAT_V3, views};
use graftyard_git::refs::{ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{REPO_FORMAT_V1, RepoCmd, RepoEvent, RepoFold};
use graftyard_raft::{CommitError, LogEntry};
use graftyard_types::{AccountId, ChainId, DbId, IdBlock, Index, RepoId, Timestamp};

const OWNER: AccountId = AccountId(DbId(100));
const REPO: RepoId = RepoId(DbId(7));
const WF: &str = ".github/workflows/ci.yml";
const SRC: &str =
    "on: pull_request\njobs:\n  t:\n    runs-on: [self-hosted, b035]\n    steps:\n      - run: \"true\"\n";

fn name(s: &str) -> RefName {
    RefName::parse(s.as_bytes()).unwrap()
}

fn oid(n: u32) -> ObjectId {
    let mut o = [0u8; 20];
    o[..4].copy_from_slice(&n.to_be_bytes());
    o[19] = 3;
    ObjectId(o)
}

struct World {
    now: Timestamp,
    repo: Group<RepoCmd>,
    shard: Group<ActionsCmd>,
    next: u128,
}

impl World {
    /// `main`, branches `a` and `b`, and pull request 1 from `a` and 2 from `b`, both into `main`.
    fn new(shard_reads: Vec<graftyard_raft::Format>) -> World {
        let now = Timestamp(10_000_000);
        let repo =
            Group::new(ChainId::Repo(REPO), vec![REPO_FORMAT_V1, REPO_FORMAT_V3], RepoFold::new(REPO, OWNER), now);
        let mut shard = Group::new(ChainId::Actions(OWNER, 3), shard_reads, ActionsFold::default(), now);
        shard.id_block(now, IdBlock::whole(0, 9).unwrap());
        let mut w = World { now, repo, shard, next: 1 };
        let up = |r: &str, new| RefUpdate { name: name(r), expect: None, new: Some(new) };
        w.record(vec![RepoEvent::Refs {
            push: graftyard_raft::RequestId(graftyard_raft::Ulid(1)),
            actor: None,
            updates: vec![up("refs/heads/main", oid(1)), up("refs/heads/a", oid(11)), up("refs/heads/b", oid(12))],
            pack: None,
        }]);
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
            w.record(vec![RepoEvent::PrOpened {
                n: IssueNumber(n as u32),
                head: HeadSpec { repo: REPO, branch: name(branch) },
                base: name("refs/heads/main"),
                capture,
            }]);
        }
        w
    }

    fn id(&mut self) -> u128 {
        self.next += 1;
        self.next << 32
    }

    /// Records `evs` as one entry and answers it.
    fn record(&mut self, evs: Vec<RepoEvent>) -> LogEntry<graftyard_raft::Ev<RepoCmd>> {
        let format = if matches!(evs[0], RepoEvent::TestMerged { .. }) { REPO_FORMAT_V3 } else { REPO_FORMAT_V1 };
        let (now, id) = (self.now, self.id());
        self.repo.record(now, id, format, evs);
        let l = self.repo.leader().unwrap();
        self.repo.m(l).log().last().unwrap().clone()
    }

    /// One push that moves both pull requests' head branches.
    fn push_both(&mut self, a: ObjectId, b: ObjectId) -> LogEntry<graftyard_raft::Ev<RepoCmd>> {
        let core = &self.repo.fold().core;
        let tip = |r: &str| graftyard_git::tiers::get_rec(&**core, "ref", &[graftyard_git::tiers::ref_key(&name(r))]);
        let updates = vec![
            RefUpdate { name: name("refs/heads/a"), expect: tip("refs/heads/a"), new: Some(a) },
            RefUpdate { name: name("refs/heads/b"), expect: tip("refs/heads/b"), new: Some(b) },
        ];
        self.record(vec![RepoEvent::Refs {
            push: graftyard_raft::RequestId(graftyard_raft::Ulid(self.next + 7)),
            actor: None,
            updates,
            pack: None,
        }])
    }

    /// `PrSync`'s test merge of pull request `n` at the current base and head.
    fn test_merge(&mut self, n: u32, result: MergeabilityResult) {
        let v = self.view(n);
        self.record(vec![RepoEvent::TestMerged {
            n: IssueNumber(n),
            base: v.base_tip.unwrap(),
            head: v.head,
            result,
            pack: None,
        }]);
    }

    fn view(&self, n: u32) -> graftyard_git::prsync::PrView {
        views(&self.repo.fold().core, REPO, &[IssueNumber(n)]).1.pop().unwrap()
    }

    fn events(&self, e: &LogEntry<graftyard_raft::Ev<RepoCmd>>) -> Vec<PrEvent> {
        pr_events(&self.repo.fold().core, REPO, e, self.now.0)
    }

    /// `ActionsTrigger`'s step for one event: the run at the current merge, if the merge is clean.
    fn run_for(&self, index: Index, p: &PrEvent) -> Option<WorkflowRun> {
        let v = self.view(p.n.0);
        match pr_target(&v) {
            PrTarget::Merge(m) => {
                Some(pull_request_run(REPO, index, WF, SRC, p.n, p.action, &v.base, m).unwrap().unwrap().0)
            }
            _ => None,
        }
    }

    fn create(&mut self, run: WorkflowRun) -> Option<Result<ActionsOut, ActionsErr>> {
        let jobs = graftyard_actions::specs::job_specs(&graftyard_actions::workflow::parse(WF, SRC).workflow).unwrap();
        let cmd = ActionsCmd { actor: ActionsActor::Operator, op: ActionsOp::CreateRun { run, jobs } };
        let (now, id) = (self.now, self.id());
        match self.shard.commit(now, id, cmd)? {
            Ok(a) => Some(Ok(a.out)),
            Err(CommitError::Refused { err, .. }) => Some(Err(err)),
            Err(_) => None,
        }
    }
}

fn clean(n: u32) -> MergeabilityResult {
    MergeabilityResult::Clean { merge: oid(900 + n) }
}

/// One entry moves two pull requests' heads: each gets its own run at its own merge on `refs/pull/N/merge`, and the
/// shard creates both. A replay of the same event is the same key, which the shard refuses as a duplicate.
#[test]
fn one_entry_that_moves_two_pull_requests_starts_a_run_for_each() {
    let mut w = World::new(vec![ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST]);
    let e = w.push_both(oid(21), oid(22));
    let evs = w.events(&e);
    assert_eq!(evs.iter().map(|p| (p.n.0, p.action)).collect::<Vec<_>>(), [(1, "synchronize"), (2, "synchronize")]);
    w.test_merge(1, clean(1));
    w.test_merge(2, clean(2));
    let runs: Vec<WorkflowRun> = evs.iter().map(|p| w.run_for(e.index, p).unwrap()).collect();
    assert_eq!((&*runs[0].head_ref, &*runs[1].head_ref), ("refs/pull/1/merge", "refs/pull/2/merge"));
    assert_eq!((runs[0].head_sha, runs[1].head_sha), (oid(901).0, oid(902).0), "each run is on its own merge");
    for r in &runs {
        assert!(matches!(w.create(r.clone()), Some(Ok(ActionsOut::Run(_)))), "{r:?}");
    }
    assert_eq!(w.shard.fold().runs.len(), 2);
    assert!(matches!(w.create(runs[1].clone()), Some(Err(ActionsErr::Duplicate))));
}

/// A pull request whose current merge conflicts starts no run; once the test merge is missing it is pending, so the
/// host asks for it before dispatching.
#[test]
fn a_conflicting_merge_starts_no_run_and_a_missing_one_is_pending() {
    let mut w = World::new(vec![ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST]);
    let e = w.push_both(oid(31), oid(32));
    assert_eq!(pr_target(&w.view(1)), PrTarget::Pending);
    w.test_merge(1, MergeabilityResult::Conflict { paths: ["f".into()].into_iter().collect() });
    w.test_merge(2, clean(2));
    let evs = w.events(&e);
    assert_eq!(pr_target(&w.view(1)), PrTarget::Conflict);
    assert!(w.run_for(e.index, &evs[0]).is_none());
    assert!(w.run_for(e.index, &evs[1]).is_some());
}

/// An event whose head has moved on still gets its run, at the merge current when the run is created (GitHub's
/// `GITHUB_SHA` for `pull_request`), and a merge recorded for the old head is not current.
#[test]
fn a_superseded_event_runs_at_the_current_merge() {
    let mut w = World::new(vec![ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST]);
    let first = w.push_both(oid(41), oid(42));
    w.test_merge(1, clean(1));
    let p = w.events(&first).remove(0);
    assert_eq!(p.after, Some(oid(41)));
    let second = w.push_both(oid(51), oid(52));
    assert_eq!(pr_target(&w.view(1)), PrTarget::Pending, "the merge of the old head is not current");
    w.test_merge(1, MergeabilityResult::Clean { merge: oid(951) });
    let run = w.run_for(first.index, &p).expect("the superseded event still runs");
    assert_eq!(run.head_sha, oid(951).0);
    assert_ne!(p.after, Some(w.view(1).head), "the instrument counts it as superseded");
    let later = w.run_for(second.index, &w.events(&second)[0]).unwrap();
    assert!(matches!(w.create(run.clone()), Some(Ok(_))));
    assert!(matches!(w.create(later), Some(Ok(_))), "the later event's run is its own");
}

/// The base moves after the head's merge was computed (B0-35 moves it every 6 s): the merge still holds
/// `refs/pull/N/merge` and the run goes there, instead of waiting on a merge at a base tip that keeps moving. A pull
/// request closed before its head had a merge starts no run and asks for none.
#[test]
fn a_merge_at_an_older_base_still_counts() {
    let mut w = World::new(vec![ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST]);
    let e = w.push_both(oid(81), oid(82));
    w.test_merge(1, clean(1));
    let core = &w.repo.fold().core;
    let main =
        graftyard_git::tiers::get_rec(&**core, "ref", &[graftyard_git::tiers::ref_key(&name("refs/heads/main"))]);
    w.record(vec![RepoEvent::Refs {
        push: graftyard_raft::RequestId(graftyard_raft::Ulid(991)),
        actor: None,
        updates: vec![RefUpdate { name: name("refs/heads/main"), expect: main, new: Some(oid(2)) }],
        pack: None,
    }]);
    assert_eq!(w.view(1).base_tip, Some(oid(2)), "the base moved");
    assert_eq!(pr_target(&w.view(1)), PrTarget::Merge(oid(901)));
    let run = w.run_for(e.index, &w.events(&e)[0]).unwrap();
    assert_eq!(run.head_sha, oid(901).0);
    let mut closed = w.view(2);
    closed.open = false;
    assert_eq!(pr_target(&closed), PrTarget::None);
}

/// The first-scan guard: an entry more than PR_REPLAY_MS old starts nothing; one just inside still does.
#[test]
fn an_entry_older_than_the_replay_guard_starts_nothing() {
    let mut w = World::new(vec![ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST]);
    let e = w.push_both(oid(61), oid(62));
    let at = e.at.0;
    let core = &w.repo.fold().core;
    assert_eq!(pr_events(core, REPO, &e, at + PR_REPLAY_MS).len(), 2);
    assert!(pr_events(core, REPO, &e, at + PR_REPLAY_MS + 1).is_empty());
    w.now = Timestamp(at + PR_REPLAY_MS + 1);
    assert!(w.events(&e).is_empty());
}

/// A `pull_request` run's `CreateRun` is format 0x15.4, which a member of a release that does not read it refuses
/// before it decides: the reader ships on a site day before any member sends it.
#[test]
fn a_pull_request_run_needs_a_shard_that_reads_its_format() {
    let mut w = World::new(vec![ACTIONS_FORMAT]);
    let e = w.push_both(oid(71), oid(72));
    w.test_merge(1, clean(1));
    let run = w.run_for(e.index, &w.events(&e)[0]).unwrap();
    let cmd = ActionsCmd { actor: ActionsActor::Operator, op: ActionsOp::CreateRun { run: run.clone(), jobs: vec![] } };
    assert_eq!(graftyard_raft::Command::format(&cmd), ACTIONS_FORMAT_PULL_REQUEST);
    assert!(w.create(run).is_none(), "refused unread");
    assert!(w.shard.fold().runs.is_empty());
}
