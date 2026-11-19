//! B0-35's `pull_request` trigger (`decisions/DECISIONS.md`, "`pr-trigger`") on real members: a repository group whose
//! entries `ActionsTrigger` reads with `graftyard_actions::trigger`, and an Actions shard that takes each run's
//! `CreateRun`. Each test fails on one plausible bug: a run key that lets one entry start only one pull request's run,
//! a conflicting merge that still starts a run, a superseded event dropped or run at its own head instead of the
//! current merge, a merge counted stale once the base moves, a first scan that replays old entries, a `CreateRun`
//! sent to a member that cannot read it, and a shed merge that reruns its batch or lets the cursor pass it (F84).

mod common;

use common::Group;
use graftyard_actions::shard::{
    ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST, ActionsActor, ActionsCmd, ActionsErr, ActionsFold, ActionsOp,
    ActionsOut, WorkflowRun,
};
use graftyard_actions::trigger::{
    Carried, Item, PR_REPLAY_MS, PrEvent, PrTarget, Stream, Tried, pr_events, pr_target, pull_request_run, settle,
};
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

/// One round of `ActionsTrigger` (`workers/site/src/actions_trigger.rs`) over `items`: each event is tried once, its
/// run created when its merge is clean, and carried when the merge is still missing (shed). Returns what `settle`
/// leaves and how many `CreateRun`s the round sent.
fn round(w: &mut World, items: Vec<Carried>, through: Index) -> (Vec<Carried>, Vec<Carried>, Index, usize) {
    let mut sent = 0;
    let mut tried = Vec::new();
    for c in items {
        let Item::Pr(p) = &c.item else { unreachable!("this world builds no queue refs") };
        let t = match w.run_for(c.index, p) {
            Some(run) => {
                sent += 1;
                assert!(matches!(w.create(run), Some(Ok(ActionsOut::Run(_)))), "each run is created once");
                Tried::Done
            }
            None if pr_target(&w.view(p.n.0)) == PrTarget::Pending => Tried::Again,
            None => Tried::Done,
        };
        tried.push((c, t));
    }
    let (left, dropped, cursor) = settle(tried, through, w.now.0);
    (left, dropped, cursor, sent)
}

fn items(w: &World, e: &LogEntry<graftyard_raft::Ev<RepoCmd>>) -> Vec<Carried> {
    w.events(e).into_iter().map(|p| Carried { index: e.index, at: e.at.0, item: Item::Pr(p) }).collect()
}

/// F84: an event whose merge is shed is carried to the next round on its own, while the rest of its batch counts as
/// done. The other event's run is sent once, not again with every rerun of the batch; the shed event gets its run
/// once its merge commits, at that merge; and until then the cursor stays just below its entry, so a new leader
/// rescans it rather than skipping it. An event shed the whole hour of the replay guard is given up, `unmerged`.
#[test]
fn a_shed_merge_is_carried_alone_and_the_cursor_waits_below_it() {
    let mut w = World::new(vec![ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST]);
    let e = w.push_both(oid(101), oid(102));
    w.test_merge(2, clean(2));
    let todo = items(&w, &e);
    let first = round(&mut w, todo, e.index);
    assert_eq!(first.3, 1, "pull request 2's run is sent");
    assert_eq!(first.0.len(), 1, "only the shed event is carried");
    assert!(first.1.is_empty());
    assert_eq!(first.2, Index(e.index.0 - 1), "the cursor waits below the carried event's entry");

    // A later entry with no pull request event: the next round reads it and tries the carried event again.
    let later = w.record(vec![RepoEvent::Refs {
        push: graftyard_raft::RequestId(graftyard_raft::Ulid(993)),
        actor: None,
        updates: vec![RefUpdate { name: name("refs/heads/c"), expect: None, new: Some(oid(103)) }],
        pack: None,
    }]);
    assert!(w.events(&later).is_empty());
    let second = round(&mut w, first.0, later.index);
    assert_eq!((second.3, second.0.len(), second.2), (0, 1, Index(e.index.0 - 1)), "still shed: nothing sent twice");

    w.test_merge(1, clean(1));
    let third = round(&mut w, second.0, later.index);
    assert_eq!(third.3, 1);
    assert!(third.0.is_empty() && third.1.is_empty());
    assert_eq!(third.2, later.index, "with nothing carried the cursor passes every entry read");
    let shas: Vec<_> = w.shard.fold().runs.values().map(|r| r.run.head_sha).collect();
    assert_eq!(shas.len(), 2, "one run per event, none twice");
    assert!(shas.contains(&oid(901).0) && shas.contains(&oid(902).0));

    // Shed for the whole hour: given up, and the cursor moves past it.
    let gone = w.push_both(oid(111), oid(112));
    let mut left = items(&w, &gone);
    for _ in 0..3 {
        let r = round(&mut w, left, gone.index);
        assert_eq!(r.2, Index(gone.index.0 - 1));
        left = r.0;
        w.now = Timestamp(w.now.0 + PR_REPLAY_MS / 2);
    }
    let last = round(&mut w, left, gone.index);
    assert!(last.0.is_empty());
    assert_eq!(last.1.len(), 2, "both events given up, unmerged");
    assert_eq!(last.2, gone.index);
}

/// One item's try, as `ActionsTrigger` makes it: the run at the current merge when it is clean, `Again` while the
/// merge is still missing.
fn try_item(w: &mut World, c: &Carried) -> Tried {
    let Item::Pr(p) = &c.item else { unreachable!("this world builds no queue refs") };
    match w.run_for(c.index, p) {
        Some(run) => {
            assert!(matches!(w.create(run), Some(Ok(ActionsOut::Run(_)))), "each run is created once");
            Tried::Done
        }
        None if pr_target(&w.view(p.n.0)) == PrTarget::Pending => Tried::Again,
        None => Tried::Done,
    }
}

/// The runs a shard holds, without their ids, which depend on the order they were created in.
fn runs(w: &World) -> Vec<(String, String, [u8; 20], Index)> {
    let mut v: Vec<_> = w
        .shard
        .fold()
        .runs
        .values()
        .map(|r| (r.run.workflow.to_string(), r.run.head_ref.to_string(), r.run.head_sha, r.run.trigger.1))
        .collect();
    v.sort();
    v
}

/// A world with five entries that each move both pull requests' heads, ten events. Pull request 2's merge is there,
/// pull request 1's is shed until `merge_one` records it.
fn five_pushes(w: &mut World) -> Vec<LogEntry<graftyard_raft::Ev<RepoCmd>>> {
    let es: Vec<_> = (0..5).map(|i| w.push_both(oid(300 + 2 * i), oid(301 + 2 * i))).collect();
    w.test_merge(2, clean(2));
    es
}

/// `trigger-stream`: the same entries run as a stream (`Stream`, two items at once) and as rounds (`settle`) commit
/// the same `CreateRun`s, only at other times. Each event gets exactly one run, at the merge current when it is
/// created, and the cursor ends at the last entry read either way. While an item is unfinished, queued, running, or
/// waiting to be tried again, the stream's cursor stays below its entry, so a crash replays it and never skips it.
#[test]
fn a_stream_commits_the_runs_rounds_commit_and_its_cursor_never_passes_an_unfinished_event() {
    // Rounds.
    let mut r = World::new(vec![ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST]);
    let es = five_pushes(&mut r);
    let through = es.last().unwrap().index;
    let todo: Vec<Carried> = es.iter().flat_map(|e| items(&r, e)).collect();
    assert_eq!(todo.len(), 10);
    let first = round(&mut r, todo.clone(), through);
    assert_eq!((first.0.len(), first.2), (5, Index(es[0].index.0 - 1)), "pull request 1's five events carried");
    r.test_merge(1, clean(1));
    let second = round(&mut r, first.0, through);
    assert!(second.0.is_empty() && second.1.is_empty());
    assert_eq!(second.2, through);

    // A stream, two at a time.
    let mut s = World::new(vec![ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST]);
    let es2 = five_pushes(&mut s);
    assert_eq!(es2.iter().map(|e| e.index).collect::<Vec<_>>(), es.iter().map(|e| e.index).collect::<Vec<_>>());
    let mut st = Stream::new(Index(es[0].index.0 - 1), 2);
    st.read(todo.clone(), through);
    let mut now = s.now.0;
    let mut merged = false;
    let mut steps = 0;
    loop {
        let started = st.start(now);
        assert!(st.running() <= 2, "never more than the cap at once");
        if started.is_empty() && st.running() == 0 {
            match st.next_retry() {
                Some(t) => {
                    // Pull request 1's merge lands while its events wait to be tried again.
                    if !merged {
                        s.test_merge(1, clean(1));
                        merged = true;
                    }
                    now = t;
                    continue;
                }
                None => break,
            }
        }
        for (id, c) in started {
            let unfinished = st.cursor();
            assert!(unfinished < c.index, "running item at {} but the cursor may commit {}", c.index.0, unfinished.0);
            let t = try_item(&mut s, &c);
            assert!(st.finish(id, t, now).is_none(), "nothing is given up inside the hour");
            if t == Tried::Again {
                assert!(st.cursor() < c.index, "an item to be tried again holds the cursor below its entry");
            }
        }
        // Between starts: every item still unfinished is waiting to be tried again, and the oldest holds the cursor.
        if st.running() == 0 && st.queued() == 0 && st.next_retry().is_some() {
            assert_eq!(st.cursor(), Index(es[0].index.0 - 1), "pull request 1's retries hold the cursor");
        }
        steps += 1;
        assert!(steps < 100);
    }
    assert_eq!(st.cursor(), through, "every item done: the cursor passes every entry read");
    assert_eq!(runs(&s), runs(&r), "the same runs, at the same merges, for the same entries");
    assert_eq!(runs(&s).len(), 10, "one run per event");
}

/// `trigger-stream`: a slow item holds one slot, not the items behind it, even in its own entry. With a cap of two
/// and the first event never ending, the other nine still start and finish one at a time beside it, and the cursor
/// waits below the slow item's entry until it ends.
#[test]
fn a_slow_event_holds_one_slot_and_the_cursor_waits_below_it() {
    let mut w = World::new(vec![ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST]);
    let es = five_pushes(&mut w);
    w.test_merge(1, clean(1));
    let through = es.last().unwrap().index;
    let todo: Vec<Carried> = es.iter().flat_map(|e| items(&w, e)).collect();
    let mut st = Stream::new(Index(es[0].index.0 - 1), 2);
    st.read(todo, through);
    let now = w.now.0;
    let mut started = st.start(now);
    let (slow, slow_item) = started.remove(0);
    assert_eq!(slow_item.index, es[0].index, "the first event of the first entry");
    let (mut done, mut last) = (0, slow_item.index);
    while !started.is_empty() {
        let (id, c) = started.remove(0);
        assert!(c.index >= last, "items start in log order");
        last = c.index;
        let t = try_item(&mut w, &c);
        st.finish(id, t, now);
        done += 1;
        assert_eq!(st.cursor(), Index(es[0].index.0 - 1), "the slow item holds the cursor below its entry");
        started.extend(st.start(now));
        assert!(st.running() <= 2);
    }
    assert_eq!(done, 9, "the nine behind the slow item all ran beside it");
    let t = try_item(&mut w, &slow_item);
    st.finish(slow, t, now);
    assert_eq!(st.cursor(), through);
    assert_eq!(runs(&w).len(), 10);
}
