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

/// BF-70's oracle gap (F127): a `pull_request` run whose job declares `id-token: write`, taken the way the site's
/// `ActionsTrigger` takes it: `pull_request_run`'s declared permissions in `create_run_call` with the site day's flags,
/// decoded by the shard leader's `create_run_op`, committed on real members, and the job message rendered from the
/// fold's grant. With job tokens on, the granted job's message names `GenerateIdTokenUrl` and the job without the grant
/// gets none; with them off (graft-m4a before BF-70 r2), neither does. `runsvc_host` builds `CreateRunWith` itself, so
/// it never ran this path.
#[test]
fn a_pull_request_runs_granted_job_gets_the_id_token_url_only_with_job_tokens_on() {
    use graftyard_actions::shard::{ACTIONS_FORMAT_CACHE, ACTIONS_FORMAT_TOKENS};
    const OIDC: &str = "on: pull_request\npermissions:\n  contents: read\n  id-token: write\njobs:\n  sts:\n    \
                        runs-on: [self-hosted, linux]\n    steps:\n      - run: \"true\"\n  denied:\n    runs-on: \
                        [self-hosted, linux]\n    permissions:\n      contents: read\n    steps:\n      - run: \"true\"\n";
    let urls = graftyard_actions::runsvc::Urls::new("https://graft.example");
    for (tokens, want) in [(true, [("denied", false), ("sts", true)]), (false, [("denied", false), ("sts", false)])] {
        let mut w =
            World::new(vec![ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST, ACTIONS_FORMAT_TOKENS, ACTIONS_FORMAT_CACHE]);
        let e = w.push_both(oid(121), oid(122));
        w.test_merge(1, clean(1));
        let p = w.events(&e).remove(0);
        let v = w.view(p.n.0);
        let PrTarget::Merge(m) = pr_target(&v) else { panic!("clean merge") };
        let (run, jobs, permissions) =
            pull_request_run(REPO, e.index, WF, OIDC, p.n, p.action, &v.base, m).unwrap().unwrap();
        // The site day's flags: `job_tokens` as given, `actions_cache` on (graft-m4a's), so the base rides along.
        let base = v.base.display();
        let call =
            graftyard_actions::runsvc::create_run_call(&run, &jobs, &permissions, tokens, Some(&base), None, None);
        let (_, op) = graftyard_actions::runsvc::create_run_op(&call).unwrap();
        assert!(matches!(op, ActionsOp::CreateRunOn { .. }), "a pull_request run with its base: {op:?}");
        let (now, id) = (w.now, w.id());
        assert!(matches!(w.shard.commit(now, id, ActionsCmd { actor: ActionsActor::Operator, op }), Some(Ok(_))));
        let f = w.shard.fold();
        let row = f.runs.values().next().unwrap();
        let mut seen: Vec<(&str, bool)> = f
            .jobs
            .values()
            .map(|j| {
                let a = graftyard_actions::runsvc::Acquired {
                    repository: "m3org/bf70",
                    owner_login: "m3org",
                    relay_site: 0,
                    github_token: "",
                    job_token: "jt",
                    locked_until: Timestamp(9),
                    secrets: vec![],
                    variables: vec![],
                    cache: true,
                    // As the shard leader reads it (`workers/site/src/actions.rs`, `acquire`).
                    id_token: graftyard_actions::oidc::id_token_granted(f.more.tokens.get(&j.spec.id), row.run.fork),
                    artifacts_v4: false,
                    actor: None,
                    run_number: None,
                    reusable: None,
                    event: None,
                };
                let msg = graftyard_actions::runsvc::acquire_message(&urls, &row.run, j, &a);
                let name: &str = if &*j.spec.name == "sts" { "sts" } else { "denied" };
                (name, msg["resources"]["endpoints"][0]["data"].get("GenerateIdTokenUrl").is_some())
            })
            .collect();
        seen.sort();
        assert_eq!(seen, want, "job tokens {tokens}");
    }
}

/// F121's `head_ref` across a mixed release: a pull request's run, taken the way the site's `ActionsTrigger` takes it
/// (`pr_head` with the site day's `GRAFTYARD_OIDC_HEAD_REF`, `create_run_call`, the leader's `create_run_op`). With
/// the flag unset, the call is `CreateRunOn` and a shard that reads only up to 0x15.6 commits it with no `RunHead`.
/// With it set (a later site day, once every member reads 0x15.7), the call is `CreateRunFrom` and the head is
/// recorded; the same call to a member without 0x15.7 is refused, never committed, which is why the flag comes last.
/// A fork's pull request carries no head even with the flag set, so its call stays `CreateRunOn`.
#[test]
fn head_ref_rides_only_with_the_flag_and_never_for_a_fork() {
    use graftyard_actions::shard::{
        ACTIONS_FORMAT_CACHE, ACTIONS_FORMAT_HEAD_REF, ACTIONS_FORMAT_TOKENS, ActionsEvent,
    };
    use graftyard_actions::trigger::pr_head;
    let v6 = vec![ACTIONS_FORMAT, ACTIONS_FORMAT_PULL_REQUEST, ACTIONS_FORMAT_TOKENS, ACTIONS_FORMAT_CACHE];
    let v7 = [v6.clone(), vec![ACTIONS_FORMAT_HEAD_REF]].concat();
    // (reader, flag): what the call decodes to, whether it commits, and the head the fold then holds.
    for (reads, flag, from, commits) in
        [(&v6, false, false, true), (&v7, false, false, true), (&v6, true, true, false), (&v7, true, true, true)]
    {
        let mut w = World::new(reads.clone());
        let e = w.push_both(oid(121), oid(122));
        w.test_merge(1, clean(1));
        let p = w.events(&e).remove(0);
        let v = w.view(p.n.0);
        let PrTarget::Merge(m) = pr_target(&v) else { panic!("clean merge") };
        let (run, jobs, permissions) =
            pull_request_run(REPO, e.index, WF, SRC, p.n, p.action, &v.base, m).unwrap().unwrap();
        let head = pr_head(&*w.repo.fold().core, REPO, p.n, flag).map(|h| h.display());
        let base = v.base.display();
        let call = graftyard_actions::runsvc::create_run_call(
            &run,
            &jobs,
            &permissions,
            false,
            Some(&base),
            head.as_deref(),
            None,
        );
        let (_, op) = graftyard_actions::runsvc::create_run_op(&call).unwrap();
        assert_eq!(matches!(op, ActionsOp::CreateRunFrom { .. }), from, "flag {flag}: {op:?}");
        assert!(from || matches!(op, ActionsOp::CreateRunOn { .. }), "without the flag, 0x15.6's call: {op:?}");
        let before = w.shard.commit_index();
        let (now, id) = (w.now, w.id());
        let got = w.shard.commit(now, id, ActionsCmd { actor: ActionsActor::Operator, op });
        assert_eq!(matches!(got, Some(Ok(_))), commits, "reads {:?} flag {flag}: {got:?}", reads.len());
        let heads: Vec<_> = w
            .shard
            .committed_after(before)
            .into_iter()
            .flat_map(|(_, evs)| evs)
            .filter_map(|e| match e {
                ActionsEvent::RunHead { head_ref, .. } => Some(head_ref),
                _ => None,
            })
            .collect();
        let want: Vec<Box<str>> = if from && commits { vec!["refs/heads/a".into()] } else { Vec::new() };
        assert_eq!(heads, want, "reads {} flag {flag}", reads.len());
        let f = w.shard.fold();
        assert_eq!(f.runs.keys().next().and_then(|r| f.run_head(*r)), want.first().map(|h| &**h));
    }
    // A fork's pull request: no head with the flag on, so its call is 0x15.6's.
    let mut w = World::new(v7);
    let issue = IssueCapture {
        id: DbId((1 << 40) + 3),
        author: None,
        title: "t".into(),
        body: "".into(),
        labels: Default::default(),
        assignees: Default::default(),
    };
    let capture = PrCapture {
        head_sha: oid(13),
        base_sha: oid(1),
        state: PrState::Open { draft: false },
        labels: Vec::new(),
        issue,
        changes_from: None,
    };
    let fork = HeadSpec { repo: RepoId(DbId(8)), branch: name("refs/heads/a") };
    w.record(vec![RepoEvent::PrOpened { n: IssueNumber(3), head: fork, base: name("refs/heads/main"), capture }]);
    assert!(!w.view(3).same_repo);
    assert_eq!(pr_head(&*w.repo.fold().core, REPO, IssueNumber(3), true), None, "a fork's branch is the fork's");
    assert_eq!(pr_head(&*w.repo.fold().core, REPO, IssueNumber(1), true), Some(name("refs/heads/a")));
}

/// BF-73 (F119, F120) on real members, taken as the site's `ActionsTrigger` takes it: a push by account 55 moves
/// pull request 1's head, and its `synchronize` names 55 as the actor (`PrEvent::actor`). With `GRAFTYARD_OIDC_CLAIMS`
/// set the call carries the actor and its login and decodes to `CreateRunAs`. A member without 0x15.8 refuses it and
/// commits nothing, so the flag waits for the reader. On a 0x15.8 member the run records 55 and number 1, a second
/// push's run number 2, and the job message's `github.actor` and `github.run_number` and the token's `actor`,
/// `actor_id` and `run_number` say so. With the flag unset, the call is the one before 0x15.8, and the message keeps the
/// owner's login and number 1.
#[test]
fn a_pull_request_runs_actor_and_number_ride_with_the_claims_flag() {
    use graftyard_actions::shard::{
        ACTIONS_FORMAT_CACHE, ACTIONS_FORMAT_CLAIMS, ACTIONS_FORMAT_HEAD_REF, ACTIONS_FORMAT_TOKENS,
    };
    let v7 = vec![
        ACTIONS_FORMAT,
        ACTIONS_FORMAT_PULL_REQUEST,
        ACTIONS_FORMAT_TOKENS,
        ACTIONS_FORMAT_CACHE,
        ACTIONS_FORMAT_HEAD_REF,
    ];
    let v8 = [v7.clone(), vec![ACTIONS_FORMAT_CLAIMS]].concat();
    let pusher = AccountId(DbId(55));
    let urls = graftyard_actions::runsvc::Urls::new("https://graft.example");
    for (reads, flag, commits) in [(&v7, false, true), (&v7, true, false), (&v8, true, true), (&v8, false, true)] {
        let mut w = World::new(reads.clone());
        let mut seen = Vec::new();
        for (i, head) in [(0u32, oid(131)), (1, oid(132))] {
            let core = &w.repo.fold().core;
            let tip =
                graftyard_git::tiers::get_rec(&**core, "ref", &[graftyard_git::tiers::ref_key(&name("refs/heads/a"))]);
            let e = w.record(vec![RepoEvent::Refs {
                push: graftyard_raft::RequestId(graftyard_raft::Ulid(900 + u128::from(i))),
                actor: Some(pusher),
                updates: vec![RefUpdate { name: name("refs/heads/a"), expect: tip, new: Some(head) }],
                pack: None,
            }]);
            w.test_merge(1, clean(140 + i));
            let p = w.events(&e).remove(0);
            assert_eq!((p.action, p.actor), ("synchronize", Some(pusher)), "the pusher is the event's actor");
            let v = w.view(p.n.0);
            let PrTarget::Merge(m) = pr_target(&v) else { panic!("clean merge") };
            let (run, jobs, permissions) =
                pull_request_run(REPO, e.index, WF, SRC, p.n, p.action, &v.base, m).unwrap().unwrap();
            let base = v.base.display();
            // The site's `run_actor`: the event's actor and its login, only with the flag.
            let actor = p.actor.filter(|_| flag).map(|a| (a, "octocat"));
            let call =
                graftyard_actions::runsvc::create_run_call(&run, &jobs, &permissions, false, Some(&base), None, actor);
            let (_, op) = graftyard_actions::runsvc::create_run_op(&call).unwrap();
            assert_eq!(matches!(op, ActionsOp::CreateRunAs { .. }), flag, "{op:?}");
            let (now, id) = (w.now, w.id());
            let got = w.shard.commit(now, id, ActionsCmd { actor: ActionsActor::Operator, op });
            assert_eq!(matches!(got, Some(Ok(_))), commits, "reads {} flag {flag}: {got:?}", reads.len());
            let Some(Ok(a)) = got else { continue };
            let ActionsOut::Run(r) = a.out else { panic!() };
            let f = w.shard.fold();
            let (row, j) = (f.run(r).unwrap(), f.run_jobs(r)[0]);
            let actor = f.run_actor(r).map(|(_, l)| l.to_owned());
            let acq = graftyard_actions::runsvc::Acquired {
                repository: "m3org/bf73",
                owner_login: "m3org",
                relay_site: 0,
                github_token: "",
                job_token: "jt",
                locked_until: Timestamp(9),
                secrets: vec![],
                variables: vec![],
                cache: false,
                id_token: false,
                artifacts_v4: false,
                actor: actor.as_deref(),
                run_number: f.run_number(r),
                reusable: None,
                event: f.run_event(r),
            };
            let msg = graftyard_actions::runsvc::acquire_message(&urls, &row.run, j, &acq);
            let gh = |k: &str| {
                let d = &msg["contextData"]["github"]["d"];
                d.as_array().unwrap().iter().find(|e| e["k"] == k).map(|e| e["v"].clone()).unwrap()
            };
            let cf = f.claim_facts(j.spec.id, None).unwrap();
            let facts = cf.facts(
                "https://graft.example",
                "m3org/bf73",
                OWNER,
                graftyard_identity::Visibility::Private,
                None,
                false,
            );
            let c = graftyard_actions::oidc::claims(&facts, None, 1, "j");
            seen.push((
                gh("actor"),
                gh("run_number"),
                c.get("actor").cloned(),
                c.get("actor_id").cloned(),
                c.get("run_number").cloned(),
            ));
        }
        use serde_json::json;
        let want: Vec<_> = match (flag, commits) {
            (true, true) => vec![
                (json!("octocat"), json!("1"), Some(json!("octocat")), Some(json!("55")), Some(json!("1"))),
                (json!("octocat"), json!("2"), Some(json!("octocat")), Some(json!("55")), Some(json!("2"))),
            ],
            (false, _) => vec![(json!("m3org"), json!("1"), None, None, None); 2],
            (true, false) => vec![],
        };
        assert_eq!(seen, want, "reads {} flag {flag}", reads.len());
    }
}

/// BF-78's `on: push` on real members, taken as the site's `ActionsTrigger` takes it (`push_events`, `push_run`,
/// `push_run_call`, the leader's `create_run_op`). One entry by account 55 moves `main`, creates `rel` and tag `v1`, and
/// deletes `b`: the delete starts nothing, and the three others each start one run of the catch-all workflow at their
/// own new commit. The run key names the ref, so the shard creates all three from one entry and refuses a replay of
/// any. The branch-filtered workflow runs only for `main` and `rel`, the tag-filtered one only for `v1`, the
/// `branches-ignore: [rel]` one for `main` alone, and the path-filtered one only when the push changed `src/`. Each
/// run records the pusher, its number, and its `github.event`, which the job message carries with `forced` as the
/// trigger read it, and its token's `sub` is `repo:o/r:ref:<the pushed ref>` with the pusher as `actor`. A shard that
/// reads no 0x15.12 refuses the call and commits nothing.
#[test]
fn one_entry_of_pushes_starts_a_run_per_matching_ref_at_its_commit() {
    use graftyard_actions::shard::{
        ACTIONS_FORMAT_CACHE, ACTIONS_FORMAT_CLAIMS, ACTIONS_FORMAT_HEAD_REF, ACTIONS_FORMAT_PUSH,
        ACTIONS_FORMAT_TOKENS,
    };
    use graftyard_actions::trigger::{push_events, push_run};
    use serde_json::json;
    const ANY: &str = "on: push\njobs:\n  t:\n    runs-on: [self-hosted]\n    steps:\n      - run: \"true\"\n";
    const BRANCHES: &str = "on:\n  push:\n    branches: [main, 'rel*']\njobs:\n  t:\n    runs-on: [self-hosted]\n    steps:\n      - run: a\n";
    const TAGS: &str =
        "on:\n  push:\n    tags: ['v*']\njobs:\n  t:\n    runs-on: [self-hosted]\n    steps:\n      - run: a\n";
    const IGNORE: &str = "on:\n  push:\n    branches-ignore: [rel]\njobs:\n  t:\n    runs-on: [self-hosted]\n    steps:\n      - run: a\n";
    const PATHS: &str =
        "on:\n  push:\n    paths: ['src/**']\njobs:\n  t:\n    runs-on: [self-hosted]\n    steps:\n      - run: a\n";
    let v8 = vec![
        ACTIONS_FORMAT,
        ACTIONS_FORMAT_PULL_REQUEST,
        ACTIONS_FORMAT_TOKENS,
        ACTIONS_FORMAT_CACHE,
        ACTIONS_FORMAT_HEAD_REF,
        ACTIONS_FORMAT_CLAIMS,
    ];
    let v12 = [v8.clone(), vec![ACTIONS_FORMAT_PUSH]].concat();
    let pusher = AccountId(DbId(55));
    let up = |r: &str, expect: Option<ObjectId>, new: Option<ObjectId>| RefUpdate { name: name(r), expect, new };
    let push = |w: &mut World| {
        w.record(vec![RepoEvent::Refs {
            push: graftyard_raft::RequestId(graftyard_raft::Ulid(700)),
            actor: Some(pusher),
            updates: vec![
                up("refs/heads/main", Some(oid(1)), Some(oid(201))),
                up("refs/heads/rel", None, Some(oid(202))),
                up("refs/tags/v1", None, Some(oid(203))),
                up("refs/heads/b", Some(oid(12)), None),
            ],
            pack: None,
        }])
    };
    // On a shard without 0x15.12 the push's call is refused and commits nothing.
    let mut w = World::new(v8);
    let e = push(&mut w);
    let p = push_events(&e, w.now.0).remove(0);
    let (run, jobs, perms) = push_run(REPO, e.index, WF, ANY, &p, None).unwrap().unwrap();
    let call = graftyard_actions::runsvc::push_run_call(&run, &jobs, &perms, false, None, &json!({}));
    let (_, op) = graftyard_actions::runsvc::create_run_op(&call).unwrap();
    let (now, id) = (w.now, w.id());
    assert!(!matches!(w.shard.commit(now, id, ActionsCmd { actor: ActionsActor::Operator, op }), Some(Ok(_))));
    assert!(w.shard.fold().runs.is_empty(), "a member without 0x15.12 commits no push run");

    let mut w = World::new(v12);
    let e = push(&mut w);
    let evs = push_events(&e, w.now.0);
    let refs: Vec<_> = evs.iter().map(|p| (p.r#ref.display(), p.after, p.created(), p.pusher)).collect();
    assert_eq!(
        refs,
        [
            ("refs/heads/main".to_owned(), oid(201), false, Some(pusher)),
            ("refs/heads/rel".to_owned(), oid(202), true, Some(pusher)),
            ("refs/tags/v1".to_owned(), oid(203), true, Some(pusher)),
        ],
        "the delete of b starts nothing"
    );
    // What each workflow takes: `main` changed `src/`, the others nothing under it.
    let paths = |p: &graftyard_actions::trigger::PushEvent| -> Option<Vec<&str>> {
        match p.r#ref.display().as_str() {
            "refs/heads/main" => Some(vec!["src/lib.rs", "README.md"]),
            "refs/heads/rel" => Some(vec!["docs/a.md"]),
            _ => None,
        }
    };
    let matched = |src: &str| -> Vec<String> {
        evs.iter()
            .filter(|p| push_run(REPO, e.index, WF, src, p, paths(p).as_deref()).unwrap().is_some())
            .map(|p| p.r#ref.display())
            .collect()
    };
    assert_eq!(matched(BRANCHES), ["refs/heads/main", "refs/heads/rel"]);
    assert_eq!(matched(TAGS), ["refs/tags/v1"]);
    assert_eq!(matched(IGNORE), ["refs/heads/main"], "branches-ignore skips rel and, branch-only, the tag");
    assert_eq!(matched(PATHS), ["refs/heads/main", "refs/tags/v1"], "a tag push ignores paths");
    // Each run of the catch-all workflow, committed as the site sends it.
    let urls = graftyard_actions::runsvc::Urls::new("https://graft.example");
    let mut got = Vec::new();
    let mut calls = Vec::new();
    for p in &evs {
        let (run, jobs, perms) = push_run(REPO, e.index, WF, ANY, p, paths(p).as_deref()).unwrap().unwrap();
        let forced = p.r#ref.display() == "refs/heads/main";
        let event = json!({ "ref": p.r#ref.display(), "after": p.after.hex(), "forced": forced });
        let call =
            graftyard_actions::runsvc::push_run_call(&run, &jobs, &perms, false, Some((pusher, "octocat")), &event);
        let (_, op) = graftyard_actions::runsvc::create_run_op(&call).unwrap();
        assert!(matches!(op, ActionsOp::CreateRunPush { .. }), "{op:?}");
        calls.push(op.clone());
        let (now, id) = (w.now, w.id());
        let Some(Ok(a)) = w.shard.commit(now, id, ActionsCmd { actor: ActionsActor::Operator, op }) else {
            panic!("{} did not commit", p.r#ref.display())
        };
        let ActionsOut::Run(r) = a.out else { panic!() };
        let f = w.shard.fold();
        let (row, j) = (f.run(r).unwrap(), f.run_jobs(r)[0]);
        let actor = f.run_actor(r).map(|(_, l)| l.to_owned());
        let acq = graftyard_actions::runsvc::Acquired {
            repository: "m3org/bf78",
            owner_login: "m3org",
            relay_site: 0,
            github_token: "",
            job_token: "jt",
            locked_until: Timestamp(9),
            secrets: vec![],
            variables: vec![],
            cache: false,
            id_token: false,
            artifacts_v4: false,
            actor: actor.as_deref(),
            run_number: f.run_number(r),
            reusable: None,
            event: f.run_event(r),
        };
        let msg = graftyard_actions::runsvc::acquire_message(&urls, &row.run, j, &acq);
        let gh = |k: &str| {
            let d = &msg["contextData"]["github"]["d"];
            d.as_array().unwrap().iter().find(|e| e["k"] == k).map(|e| e["v"].clone())
        };
        let ev = gh("event").expect("github.event");
        let ev_field = |k: &str| ev["d"].as_array().unwrap().iter().find(|e| e["k"] == k).map(|e| e["v"].clone());
        let cf = f.claim_facts(j.spec.id, None).unwrap();
        let facts = cf.facts(
            "https://graft.example",
            "m3org/bf78",
            OWNER,
            graftyard_identity::Visibility::Private,
            None,
            false,
        );
        let c = graftyard_actions::oidc::claims(&facts, None, 1, "j");
        got.push((
            row.run.head_sha,
            gh("ref").unwrap(),
            gh("event_name").unwrap(),
            gh("actor").unwrap(),
            gh("run_number").unwrap(),
            ev_field("forced").unwrap(),
            c["sub"].clone(),
            c["actor"].clone(),
        ));
    }
    let sub = |r: &str| json!(format!("repo:m3org/bf78:ref:{r}"));
    assert_eq!(
        got,
        [
            (
                oid(201).0,
                json!("refs/heads/main"),
                json!("push"),
                json!("octocat"),
                json!("1"),
                json!(true),
                sub("refs/heads/main"),
                json!("octocat")
            ),
            (
                oid(202).0,
                json!("refs/heads/rel"),
                json!("push"),
                json!("octocat"),
                json!("2"),
                json!(false),
                sub("refs/heads/rel"),
                json!("octocat")
            ),
            (
                oid(203).0,
                json!("refs/tags/v1"),
                json!("push"),
                json!("octocat"),
                json!("3"),
                json!(false),
                sub("refs/tags/v1"),
                json!("octocat")
            ),
        ]
    );
    assert_eq!(w.shard.fold().runs.len(), 3, "one entry, one file, three refs: three runs");
    for op in calls {
        let (now, id) = (w.now, w.id());
        let again = w.shard.commit(now, id, ActionsCmd { actor: ActionsActor::Operator, op });
        assert!(
            matches!(again, Some(Err(CommitError::Refused { err: ActionsErr::Duplicate, .. }))),
            "a replay creates nothing: {again:?}"
        );
    }
}

/// `push_events`' edges: a force push still runs at its new tip; a ref the entry moves twice is one push from the
/// first `before` to the last `after`; four tags pushed at once start no tag run while the branch in the same entry
/// still does; a pull request merged into `main` is a push by the merger; a queue ref, a `refs/pull/` ref, and an
/// update that leaves the tip where it was start none; and the first-scan guard drops an entry over an hour old.
#[test]
fn push_events_skip_deletes_queue_refs_and_more_than_three_tags() {
    use graftyard_actions::trigger::{PR_REPLAY_MS, push_events};
    let w = World::new(vec![ACTIONS_FORMAT]);
    let entry = |evs: Vec<RepoEvent>| {
        let mut e = w.repo.m(w.repo.leader().unwrap()).log().last().unwrap().clone();
        e.body = graftyard_raft::EntryBody::Events(evs);
        e.at = w.now;
        e
    };
    let up = |r: &str, expect: Option<ObjectId>, new: Option<ObjectId>| RefUpdate { name: name(r), expect, new };
    let refs = |updates: Vec<RefUpdate>| RepoEvent::Refs {
        push: graftyard_raft::RequestId(graftyard_raft::Ulid(1)),
        actor: Some(AccountId(DbId(9))),
        updates,
        pack: None,
    };
    let names = |e: &LogEntry<graftyard_raft::Ev<RepoCmd>>| -> Vec<(String, ObjectId, ObjectId)> {
        push_events(e, w.now.0).into_iter().map(|p| (p.r#ref.display(), p.before, p.after)).collect()
    };
    let mut tags: Vec<RefUpdate> = (1..=4).map(|i| up(&format!("refs/tags/v{i}"), None, Some(oid(300 + i)))).collect();
    tags.push(up("refs/heads/main", Some(oid(1)), Some(oid(310))));
    assert_eq!(names(&entry(vec![refs(tags)])), [("refs/heads/main".into(), oid(1), oid(310))]);
    let three: Vec<RefUpdate> = (1..=3).map(|i| up(&format!("refs/tags/v{i}"), None, Some(oid(300 + i)))).collect();
    assert_eq!(names(&entry(vec![refs(three)])).len(), 3);
    let others = vec![
        up("refs/heads/gh-readonly-queue/main/pr-1-ab", None, Some(oid(320))),
        up("refs/pull/1/head", None, Some(oid(321))),
        up("refs/heads/same", Some(oid(5)), Some(oid(5))),
        up("refs/heads/gone", Some(oid(6)), None),
    ];
    assert!(names(&entry(vec![refs(others)])).is_empty());
    let twice = vec![
        refs(vec![up("refs/heads/main", Some(oid(1)), Some(oid(330)))]),
        refs(vec![up("refs/heads/main", Some(oid(330)), Some(oid(331)))]),
    ];
    assert_eq!(names(&entry(twice)), [("refs/heads/main".into(), oid(1), oid(331))]);
    let merged = RepoEvent::Merged {
        n: IssueNumber(1),
        base: up("refs/heads/main", Some(oid(1)), Some(oid(340))),
        head: oid(11),
        method: graftyard_git::issues::MergeMethod::Merge,
        also: vec![up("refs/heads/a", Some(oid(11)), None)],
        pack: None,
        verified: graftyard_git::merge::TreeDiffDigest(graftyard_types::Digest([0; 32])),
        engine: graftyard_git::merge::EngineVersion("2.47".into()),
        by: graftyard_identity::UserId(AccountId(DbId(77))),
    };
    let e = entry(vec![merged]);
    let got = push_events(&e, w.now.0);
    assert_eq!(
        got.iter().map(|p| (p.r#ref.display(), p.pusher)).collect::<Vec<_>>(),
        [("refs/heads/main".into(), Some(AccountId(DbId(77))))]
    );
    assert!(push_events(&e, w.now.0 + PR_REPLAY_MS + 1).is_empty(), "an entry over an hour old starts nothing");
}

/// GHES's skip markers (`push_run`'s caller reads them from the head commit): each marker anywhere, and the
/// `skip-checks` trailer only as the last line.
#[test]
fn a_head_commit_with_a_skip_marker_skips_push_runs() {
    use graftyard_actions::trigger::skips_ci;
    for m in
        ["fix [skip ci]", "[ci skip] x", "a\n\n[no ci]", "[skip actions]", "[actions skip]", "x\n\n\nskip-checks:true"]
    {
        assert!(skips_ci(m), "{m:?}");
    }
    assert!(skips_ci("x\n\n\nskip-checks: true\n"));
    for m in ["fix ci", "skip-checks: true\n\nmore", "[skip-ci]", "skip ci"] {
        assert!(!skips_ci(m), "{m:?}");
    }
}

/// F133: the base a push's `paths` filters and `github.event.commits` read from (`push_base`), and a force push's
/// two-dot diff (`two_dot`) from the cache's `/ui/diff` answers against the merge base (`diff_paths`). The force push's
/// trees, by path and blob, are mb `{a: 1, b: 1, c: 1, d: 1}`, before `{a: 2, b: 4, c: 5, d: 1, n: 7}` and after
/// `{a: 2, b: 3, c: 1, e: 1}`, where after renamed `d` to `e`: so before..after changes `b`, `c`, `d`, `e` and `n`, and
/// not `a`, which both sides changed to the same blob, nor the three-dot diff's answer `{a, b, d, e}`. A path both
/// sides removed, or one removed and the other renamed away, is no change.
#[test]
fn a_push_reads_paths_from_its_base_and_a_force_push_two_dot() {
    use graftyard_actions::trigger::{PushEvent, diff_paths, push_base, two_dot};
    use serde_json::json;
    let main = name("refs/heads/main");
    let ev = |r: &str, before: ObjectId| PushEvent { r#ref: name(r), before, after: oid(9), pusher: None };
    assert_eq!(push_base(&ev("refs/heads/main", oid(4)), &main, Some(oid(5))), Some(oid(4)), "an existing ref: before");
    assert_eq!(push_base(&ev("refs/heads/dev", oid(4)), &main, Some(oid(5))), Some(oid(4)), "a force push: before");
    assert_eq!(push_base(&ev("refs/heads/dev", ObjectId::ZERO), &main, Some(oid(5))), Some(oid(5)), "a new branch");
    assert_eq!(push_base(&ev("refs/heads/main", ObjectId::ZERO), &main, Some(oid(5))), None, "the default branch");
    assert_eq!(push_base(&ev("refs/tags/v1", ObjectId::ZERO), &main, Some(oid(5))), None, "a new tag");
    assert_eq!(push_base(&ev("refs/heads/dev", ObjectId::ZERO), &main, None), None, "no default tip");

    let to_after = json!({ "truncated": false, "files": [
        { "filename": "a", "status": "modified", "sha": "2" },
        { "filename": "b", "status": "modified", "sha": "3" },
        { "filename": "e", "previous_filename": "d", "status": "renamed", "sha": "1" },
    ]});
    let to_before = json!({ "truncated": false, "files": [
        { "filename": "a", "status": "modified", "sha": "2" },
        { "filename": "b", "status": "modified", "sha": "4" },
        { "filename": "c", "status": "modified", "sha": "5" },
        { "filename": "n", "status": "added", "sha": "7" },
    ]});
    let (a, b) = (diff_paths(&to_after).unwrap(), diff_paths(&to_before).unwrap());
    assert_eq!(a.keys().collect::<Vec<_>>(), ["a", "b", "d", "e"], "a rename lists both names");
    assert_eq!(two_dot(&a, &b), ["b", "c", "d", "e", "n"]);
    let removed = json!({ "files": [{ "filename": "c", "status": "removed", "sha": "1" }] });
    assert_eq!(two_dot(&diff_paths(&removed).unwrap(), &b), ["a", "b", "c", "n"], "c removed after, changed before");
    let gone = json!({ "files": [{ "filename": "d", "status": "removed", "sha": "1" }] });
    assert_eq!(two_dot(&a, &diff_paths(&gone).unwrap()), ["a", "b", "e"], "d renamed away after, removed before");
    assert_eq!(diff_paths(&json!({ "truncated": true, "files": [] })), None, "a truncated diff runs every filter");
}
