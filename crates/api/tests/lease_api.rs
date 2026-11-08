//! AM1-WP4's lease API and activity feed in memory: the renderers of `graftyard_api::leases` over the tiers of real
//! repository members (`crates/git/tests/common`, AM1's harness, included by path), and LF-3, LF-12, and LF-13's pure
//! parts.
//!
//! - LF-3: a feed cursor from another group, `wait=40`, and 33 `paths`: 410 with `oldest_cursor`, `wait` clamped to
//!   25, and 422 for the 33 paths.
//! - LF-12 and LF-13 (pure): learners serve, followers only while no learner answers, never the leader; at most
//!   FEED_HELD_MAX polls held per member, the rest 429 with `Retry-After`; a member that falls behind answers every
//!   held poll at once and refuses new ones until it follows again.
//! - The lease endpoints' replies and the task GET's `leases`.

#[path = "../../git/tests/common/mod.rs"]
mod common;
use common::*;

use std::time::Duration;

use graftyard_api::leases::{self, Route};
use graftyard_api::tasks::{self, Reader};
use graftyard_api::view::{Compare, Directory, Repo, RepoRef};
use graftyard_api::wire::Host;
use graftyard_git::feed::{
    self, ActivityEvent, Admit, FEED_BATCH_ALL, FEED_HELD_MAX, FeedCursor, FeedRole, Following, Serving, Start,
};
use graftyard_git::lease::{LeaseErr, LeaseMode, NewLease};
use graftyard_git::task::NewTask;
use graftyard_types::Index;
use graftyard_wire::{Body, Reply};
use serde_json::{Value, json};

struct Dir;

impl Directory for Dir {
    fn login(&self, id: AccountId) -> Option<String> {
        Some(format!("u{}", id.0.0))
    }
    fn name(&self, id: AccountId) -> Option<String> {
        self.login(id)
    }
    fn is_org(&self, _: AccountId) -> bool {
        false
    }
    fn by_login(&self, _: &str) -> Option<AccountId> {
        None
    }
    fn permission(&self, _: AccountId) -> Option<RepoPermission> {
        None
    }
    fn repo(&self, _: RepoId) -> Option<RepoRef> {
        None
    }
    fn ref_of(&self, _: RepoId, _: &RefName) -> Option<ObjectId> {
        None
    }
    fn team(&self, _: DbId) -> Option<(String, String, String)> {
        None
    }
    fn viewer(&self) -> Option<AccountId> {
        None
    }
    fn compare(&self, _: ObjectId, _: ObjectId) -> Compare {
        Compare { commits: 0, additions: 0, deletions: 0, changed_files: 0 }
    }
}

fn host() -> Host {
    Host::new("graftyard.example", false).unwrap()
}

fn here() -> RepoRef {
    RepoRef { id: REPO, owner: "o".into(), owner_id: AccountId(DbId(OWNER)), name: "r".into() }
}

fn json_of(r: &Reply) -> &Value {
    match &r.body {
        Body::Json(v) => v,
        other => panic!("not JSON: {other:?}"),
    }
}

fn acquire(paths: &[&str], mode: LeaseMode, session: &str) -> IssueOp<graftyard_git::blob::BlobRef> {
    IssueOp::AcquireLease(NewLease {
        paths: paths.iter().map(|p| Box::from(*p)).collect(),
        refs: Vec::new(),
        mode,
        ttl_seconds: None,
        intent: "Rework <!-- hidden -->auth".into(),
        session: Some(session.into()),
    })
}

fn task(w: &mut World, name: &str) {
    let op = IssueOp::CreateTask(NewTask {
        session: name.into(),
        requester: Some(UserId(AccountId(DbId(WRITER)))),
        issue: None,
        summary: "".into(),
    });
    w.commit(issue(app(CLAUDE), op)).expect("a task");
}

/// LF-3, through the feed's parser, `start`, and renderer.
#[test]
fn lf3_foreign_cursor_long_wait_and_too_many_paths() {
    let other = FeedCursor { group: RepoId(DbId(8)), index: Index(5) }.encode();
    let q = leases::parse_feed(&format!("cursor={other}&wait=40")).expect("a valid query");
    assert_eq!(q.wait, Duration::from_secs(25), "wait clamped to 25");
    let start = feed::start(REPO, q.cursor.as_deref(), Index(3), Index(40));
    let oldest = FeedCursor { group: REPO, index: Index(3) };
    assert_eq!(start, Start::Gone { oldest });
    let (h, d) = (host(), Dir);
    let core = graftyard_fold::MemStore::default();
    let r = Repo { host: &h, at: here(), core: &core, meta: &core, dir: &d };
    let reply = leases::activity(&r, start, Index(40), &[], &q, true);
    assert_eq!(reply.status, 410);
    assert_eq!(json_of(&reply)["oldest_cursor"], json!(oldest.encode()));
    // A cursor older than the ring, and a malformed one, get the same 410; one of this group inside it is served.
    for c in [FeedCursor { group: REPO, index: Index(2) }.encode(), "garbage".into()] {
        assert_eq!(feed::start(REPO, Some(&c), Index(3), Index(40)), Start::Gone { oldest });
    }
    let mine = FeedCursor { group: REPO, index: Index(10) }.encode();
    assert_eq!(feed::start(REPO, Some(&mine), Index(3), Index(40)), Start::After(Index(10)));
    let many: String = (0..33).map(|i| format!("paths=p{i}/**&")).collect();
    let refused = leases::parse_feed(&many).unwrap_err();
    assert_eq!(refused.status, 422);
    assert_eq!(json_of(&refused)["errors"][0]["field"], "paths");
    let ok: String = (0..32).map(|i| format!("paths=p{i}/**&")).collect();
    assert_eq!(leases::parse_feed(&ok).unwrap().paths.len(), 32);
    // Without `contents: read` the feed is 404, as every path field is.
    assert_eq!(leases::activity(&r, Start::After(Index(10)), Index(40), &[], &q, false).status, 404);
    eprintln!("LF-3: foreign cursor 410 with oldest_cursor, wait=40 -> 25 s, 33 paths 422 (32 accepted)");
}

/// LF-12 and LF-13 on the serving rules: 5,000 polls over a leader, two followers, and two learners, then a follower
/// that falls behind with 1,000 held.
#[test]
fn lf12_lf13_polls_go_to_following_members_and_never_the_leader() {
    let ok = Following { gap: false, installing: false, lag: Duration::from_millis(300) };
    let roles = [FeedRole::Leader, FeedRole::Follower, FeedRole::Follower, FeedRole::Learner, FeedRole::Learner];
    // Before `place_learners`: no learner answers, so the two followers serve.
    let mut members: Vec<Serving> = roles.iter().map(|_| Serving::default()).collect();
    let (mut held, mut too_many) = ([0u32; 5], 0);
    for i in 0..5_000usize {
        // The edge asks up to three members, starting at a different one each time.
        let order: Vec<usize> = (0..3).map(|k| (i + k) % 3).collect();
        let answers: Vec<_> =
            order.iter().map(|&m| (members[m].admit(roles[m], false, ok), roles[m], ok.lag)).collect();
        match feed::route(&answers) {
            feed::Route::Held(k) => {
                held[order[k]] += 1;
                // The other members that held it too drop their copy.
                for (j, (a, _, _)) in answers.iter().enumerate() {
                    if j != k && *a == Admit::Hold {
                        members[order[j]].done();
                    }
                }
            }
            feed::Route::TooMany { retry_after } => {
                assert_eq!(retry_after, FEED_BATCH_ALL);
                too_many += 1;
                for (j, (a, _, _)) in answers.iter().enumerate() {
                    if *a == Admit::Hold {
                        members[order[j]].done();
                    }
                }
            }
        }
    }
    assert_eq!(held[0], 0, "zero polls on the leader");
    assert!(members.iter().all(|m| m.held <= FEED_HELD_MAX && m.peak <= FEED_HELD_MAX));
    assert_eq!(held[1] + held[2], 2 * FEED_HELD_MAX);
    assert_eq!(too_many, 5_000 - 2 * FEED_HELD_MAX as usize, "the rest are 429 with Retry-After");
    let peaks: Vec<u32> = members.iter().map(|m| m.peak).collect();
    // After placement: learners answer, so followers refuse and only learners hold.
    let mut members: Vec<Serving> = roles.iter().map(|_| Serving::default()).collect();
    let mut after = [0u32; 5];
    for i in 0..1_500usize {
        let order = [0, 1 + i % 2, 3 + i % 2];
        let answers: Vec<_> = order.iter().map(|&m| (members[m].admit(roles[m], true, ok), roles[m], ok.lag)).collect();
        if let feed::Route::Held(k) = feed::route(&answers) {
            after[order[k]] += 1;
        }
    }
    assert_eq!((after[0], after[1], after[2]), (0, 0, 0), "zero on the leader and on followers after placement");
    assert_eq!(after[3] + after[4], 1_500);
    // LF-13: a follower with 1,000 held whose AppendEntries leave a gap answers them all at once, and refuses new
    // polls until an InstallSnapshot catch-up ends and it follows again.
    let mut f = Serving::default();
    for _ in 0..FEED_HELD_MAX {
        assert_eq!(f.admit(FeedRole::Follower, false, ok), Admit::Hold);
    }
    assert_eq!(f.drop_all(), FEED_HELD_MAX, "every held poll answered with its cursor and no events");
    let gap = Following { gap: true, ..ok };
    let installing = Following { installing: true, ..ok };
    assert_eq!(f.admit(FeedRole::Follower, false, gap), Admit::Lagging);
    assert_eq!(f.admit(FeedRole::Follower, false, installing), Admit::Lagging);
    assert_eq!(f.admit(FeedRole::Follower, false, Following { lag: Duration::from_secs(2), ..ok }), Admit::Lagging);
    assert_eq!(f.admit(FeedRole::Follower, false, ok), Admit::Hold);
    eprintln!(
        "LF-12 pure: 5,000 polls before placement held {:?} per member (peaks {peaks:?}), {too_many} 429; after \
         placement {after:?}. LF-13 pure: 1,000 held answered at once, new polls refused while behind",
        held
    );
}

/// The lease endpoints' replies over a real fold: 201 `{lease, overlaps}`, 409 with `conflicts`, the list with
/// `paths`, `per_page` and `Link`, 200 renew, 204 release, and the task GET's `leases`.
#[test]
fn the_lease_endpoints_serve_the_reference_shape() {
    let mut w = World::new();
    w.flag(claude_row());
    task(&mut w, "a");
    task(&mut w, "b");
    let mut acks = w.commit_all_acked(vec![
        issue(app(CLAUDE), acquire(&["pkg/auth/**"], LeaseMode::Exclusive, "a")),
        issue(app(CLAUDE), acquire(&["pkg/auth/tokens.rs", "docs/**"], LeaseMode::Shared, "b")),
        issue(app(CLAUDE), acquire(&["pkg/auth/x/**"], LeaseMode::Exclusive, "b")),
    ]);
    let conflict = acks.pop().unwrap().expect_err("an overlapping Exclusive acquire");
    let second = acks.pop().unwrap().expect("a Shared acquire");
    let first = acks.pop().unwrap().expect("an Exclusive acquire");
    let f = w.fold();
    let (h, d) = (host(), Dir);
    let r = Repo { host: &h, at: here(), core: &f.core, meta: &f.meta, dir: &d };
    let out = |a: &graftyard_raft::Acked<RepoOut>| match &a.out {
        RepoOut::Issue(c) => a.map(|_| c.out.clone()),
        x => panic!("{x:?}"),
    };
    let reply = leases::render_ok(&r, &w.ident.owner, w.now, &out(&second), true);
    assert_eq!(reply.status, 201);
    let v = json_of(&reply);
    assert_eq!(v["lease"]["mode"], "shared");
    assert_eq!(v["lease"]["holder"]["session"], format!("{}.b", CLAUDE.0.0));
    assert_eq!(v["lease"]["holder"]["app_id"], CLAUDE.0.0);
    assert_eq!(v["lease"]["intent"]["visible"], "Rework auth", "intent is a TrustView");
    assert_eq!(v["overlaps"].as_array().unwrap().len(), 1, "the Exclusive lease it overlaps");
    assert_eq!(v["overlaps"][0]["mode"], "exclusive");
    let RepoErr::Issue { err, .. } = &conflict else { panic!("{conflict:?}") };
    assert!(matches!(err, IssueErr::Lease(LeaseErr::Conflict(_))));
    let refused = leases::refused(Some((&r, w.now)), err);
    assert_eq!(refused.status, 409);
    assert_eq!(json_of(&refused)["conflicts"][0]["paths"], json!(["pkg/auth/**"]));
    // The list, filtered and paged.
    let list = leases::list(&r, &w.ident.owner, w.now, "", true, "/repos/o/r/leases");
    assert_eq!(json_of(&list).as_array().unwrap().len(), 2);
    let docs = leases::list(&r, &w.ident.owner, w.now, "paths=docs%2Fa.md", true, "/repos/o/r/leases");
    assert_eq!(json_of(&docs).as_array().unwrap().len(), 1);
    let page = leases::list(&r, &w.ident.owner, w.now, "per_page=1", true, "/repos/o/r/leases");
    let link = page.header("link").expect("more remain");
    assert!(
        link.starts_with("<https://graftyard.example/api/graftyard/v1/repos/o/r/leases?per_page=1&after="),
        "{link}"
    );
    assert_eq!(leases::list(&r, &w.ident.owner, w.now, "", false, "/x").status, 404, "needs contents: read");
    // The task GET lists the task's live leases.
    let g =
        tasks::get(&r, &w.ident.owner, w.now, &format!("{}.a", CLAUDE.0.0), Reader { metadata: true, contents: true });
    let tl = &json_of(&g)["leases"];
    assert_eq!(tl.as_array().unwrap().len(), 1);
    assert_eq!(tl[0]["paths"], json!(["pkg/auth/**"]));
    let id = match &first.out {
        RepoOut::Issue(c) => match c.out {
            IssueOut::Lease(id) => id,
            ref x => panic!("{x:?}"),
        },
        x => panic!("{x:?}"),
    };
    drop(f);
    // Renew is 200 with the lease; release is 204.
    let renew = leases::parse_renew(&id.text(), &json!({ "ttl_seconds": 1200 })).unwrap();
    let a = w.commit_all_acked(vec![issue(app(CLAUDE), renew)]).pop().unwrap().unwrap();
    let f = w.fold();
    let r = Repo { host: &h, at: here(), core: &f.core, meta: &f.meta, dir: &d };
    assert_eq!(leases::render_ok(&r, &w.ident.owner, w.now, &out(&a), false).status, 200);
    drop(f);
    let a = w.commit_all_acked(vec![issue(app(CLAUDE), IssueOp::ReleaseLease { id })]).pop().unwrap().unwrap();
    let f = w.fold();
    let r = Repo { host: &h, at: here(), core: &f.core, meta: &f.meta, dir: &d };
    assert_eq!(leases::render_ok(&r, &w.ident.owner, w.now, &out(&a), false).status, 204);
    let g =
        tasks::get(&r, &w.ident.owner, w.now, &format!("{}.a", CLAUDE.0.0), Reader { metadata: true, contents: true });
    assert_eq!(json_of(&g)["leases"], json!([]));
    // Every route sits under `/api/graftyard/v1`; nothing under `/api/v3/agents`.
    assert_eq!(leases::route("/repos/o/r/leases"), Some(Route::Leases { owner: "o", repo: "r" }));
    assert_eq!(leases::route("/agents/leases"), None);
    // The feed renders an event with its paths only under `contents: read`.
    let e = ActivityEvent::AgentPush {
        refname: name("refs/heads/claude/x"),
        head: oid(3),
        paths: smallvec!["a.rs".into()],
    };
    assert!(leases::event_json(&r, &e, w.now, true).get("paths").is_some());
    assert!(leases::event_json(&r, &e, w.now, false).get("paths").is_none());
}
