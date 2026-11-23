//! AM2-WP2b's `GET /overlaps` and `GET /provenance` (`graftyard_api::overlaps`) over the tiers of real repository
//! members (`crates/git/tests/common`, AM1's harness, included by path), each rendered twice: over the whole fold, and
//! as the edge renders it from a member's narrow answer (`graftyard_api::narrow::Route::Overlaps` and `Provenance`).
//! Where a plausible wrong implementation differs:
//!
//! - overlaps: a planned-only path, a `truncated` directory, a lease, the subject's own ref, a ref with no overlap, a
//!   pull request found through `pull_by_head` rather than the row, and the 100 cap;
//! - provenance: a squash commit mapped through `Merged`, an agent commit whose pull request closed and whose
//!   `pull_by_head` row is gone mapped through its ref, a forged trailer left unlinked and cut to 200 characters, a
//!   human commit with no rows, and blame's line lookup;
//! - the narrow answer renders the same JSON and holds no row of a ref that does not overlap.

#[path = "../../git/tests/common/mod.rs"]
mod common;
use common::*;

use graftyard_api::hosted::TierRows;
use graftyard_api::narrow::{Picks, Route, Spec, answer};
use graftyard_api::overlaps::{self, OverlapQuery};
use graftyard_api::view::{Compare, Directory, Pick, Repo, RepoRef};
use graftyard_api::wire::Host;
use graftyard_fold::{MemStore, Read};
use graftyard_git::agent::{PathSet, Provenance, Source, TrailerKey};
use graftyard_git::lease::{LeaseMode, NewLease};
use graftyard_git::task::NewTask;
use graftyard_identity::SessionId;
use serde_json::{Value, json};

struct Dir<'a>(Option<&'a Picks>);

impl Directory for Dir<'_> {
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
        Compare::default()
    }
    fn pick(&self, key: &str, select: &mut dyn FnMut() -> Pick) -> Pick {
        match self.0 {
            Some(p) => p.get(key, select),
            None => select(),
        }
    }
}

fn host() -> Host {
    Host::new("graftyard.example", false).unwrap()
}

fn here() -> RepoRef {
    RepoRef { id: REPO, owner: "o".into(), owner_id: AccountId(DbId(OWNER)), name: "r".into() }
}

fn session(name: &str) -> SessionId {
    SessionId::External { app: CLAUDE, name: name.into() }
}

fn path_set(r: &str, paths: &[&str], planned: &[&str], truncated: bool) -> RepoEvent {
    RepoEvent::PathSet(PathSet {
        refname: name(r),
        pr: None,
        head: oid(500),
        merge_base: None,
        paths: paths.iter().map(|p| Box::from(*p)).collect(),
        planned: planned.iter().map(|p| Box::from(*p)).collect(),
        truncated,
    })
}

fn world() -> World {
    let mut w = World::new();
    w.flag(claude_row());
    let op = IssueOp::CreateTask(NewTask {
        session: "a".into(),
        requester: Some(UserId(AccountId(DbId(WRITER)))),
        issue: None,
        summary: "".into(),
    });
    w.commit(issue(app(CLAUDE), op)).expect("a task");
    w
}

/// `route`'s reply as the edge renders it from a member's narrow answer, beside the narrow rows; `render` draws the
/// reply from a `Repo`.
fn narrow(f: &RepoFold, route: Route, render: &dyn Fn(&Repo<'_>) -> Value) -> (Value, MemStore) {
    let spec = Spec { at: here(), route };
    let named = || graftyard_api::hosted::accounts(&f.core, &f.meta);
    let v = answer(&f.core, Some(&f.meta), 9, &serde_json::to_value(spec).unwrap(), &named);
    assert!(v.get("whole").is_none() && v.get("error").is_none(), "answered narrowly: {v}");
    let rows = TierRows::from_value(&v).unwrap();
    let (core, meta) = (TierRows::store(&rows.core), TierRows::store(rows.meta.as_ref().unwrap()));
    let picks = Picks::of(rows.picks.clone());
    let (h, d) = (host(), Dir(Some(&picks)));
    let out = render(&Repo { host: &h, at: here(), core: &core, meta: &meta, dir: &d });
    assert!(!picks.missed(), "the edge asked for a selection the member did not make");
    (out, meta)
}

fn whole(f: &RepoFold, render: &dyn Fn(&Repo<'_>) -> Value) -> Value {
    let (h, d) = (host(), Dir(None));
    render(&Repo { host: &h, at: here(), core: &f.core, meta: &f.meta, dir: &d })
}

fn refs_of(v: &Value) -> Vec<&str> {
    v["overlaps"].as_array().unwrap().iter().map(|o| o["ref"].as_str().unwrap()).collect()
}

#[test]
fn overlaps_list_pushed_planned_and_truncated_paths_and_live_leases_but_not_the_subject() {
    let mut w = world();
    w.set("refs/heads/claude/y", oid(600));
    w.record_in(
        graftyard_git::repo::REPO_FORMAT_LEASES,
        vec![
            path_set("refs/heads/claude/x", &["pkg/auth/session.go"], &[], false),
            // Only its planned paths overlap.
            path_set("refs/heads/claude/y", &["docs/a.md"], &["pkg/auth/**"], false),
            // Past PATHS_MAX: `pkg/` stands for everything under it.
            path_set("refs/heads/claude/t", &["pkg/", "web/"], &[], true),
            path_set("refs/heads/claude/z", &["web/x.ts"], &[], false),
        ],
    );
    // Y has an agent's pull request, which its row does not name: `pull_by_head` finds it.
    let y = number(&open_pr(&mut w, app(CLAUDE), "refs/heads/claude/y", true));
    let lease = |paths: &[&str]| {
        issue(
            app(CLAUDE),
            IssueOp::AcquireLease(NewLease {
                paths: paths.iter().map(|p| Box::from(*p)).collect(),
                refs: Vec::new(),
                mode: LeaseMode::Shared,
                ttl_seconds: Some(60),
                intent: "auth".into(),
                session: Some("a".into()),
            }),
        )
    };
    for c in [lease(&["pkg/auth/tokens.go", "pkg/auth/session.go"]), lease(&["docs/**"])] {
        w.commit(c).expect("a lease");
    }
    let f = w.fold();
    let owner = w.ident.owner.clone();
    let q = OverlapQuery { refname: Some("refs/heads/claude/x".into()), pr: None, paths: Vec::new() };
    let render = |r: &Repo<'_>| overlaps::overlaps_json(r, Some(&owner), w.now, &q);
    let all = whole(&f, &render);
    assert_eq!(refs_of(&all), ["refs/heads/claude/t", "refs/heads/claude/y"], "{all}");
    let (t, yv) = (&all["overlaps"][0], &all["overlaps"][1]);
    assert_eq!(t["shared_paths"], json!(["pkg/"]));
    assert_eq!(t["truncated"], true);
    assert_eq!(yv["shared_paths"], json!(["pkg/auth/**"]));
    assert_eq!(
        (yv["pr"].as_u64(), yv["draft"].as_bool(), yv["app"].as_i64()),
        (Some(u64::from(y)), Some(true), Some(CLAUDE.0.0))
    );
    assert_eq!(yv["last_push"], oid(500).hex());
    assert_eq!(yv["verdict"], Value::Null, "no verdict before AM4-WP2");
    assert_eq!(all["leases"].as_array().unwrap().len(), 1, "the docs/** lease does not overlap: {all}");
    assert_eq!(all["leases"][0]["paths"], json!(["pkg/auth/tokens.go", "pkg/auth/session.go"]));
    assert_eq!(all["truncated"], false);
    // Past the lease's TTL it is no longer listed.
    let later = Timestamp(w.now.0 + 61_000);
    assert_eq!(whole(&f, &|r| overlaps::overlaps_json(r, Some(&owner), later, &q))["leases"], json!([]));
    // The edge's render over the member's narrow rows is the same, and holds no row of Z or the docs lease.
    let (got, meta) = narrow(&f, Route::Overlaps(q.clone()), &render);
    assert_eq!(got, all);
    let kept: Vec<String> = meta
        .prefix("path_set", &[])
        .into_iter()
        .map(|(k, _)| match &k[0] {
            graftyard_fold::Val::Blob(b) => String::from_utf8_lossy(b).into_owned(),
            v => panic!("{v:?}"),
        })
        .collect();
    assert_eq!(kept, ["refs/heads/claude/t", "refs/heads/claude/x", "refs/heads/claude/y"]);
    // A pull request names the same subject as its head ref; `paths` alone works too.
    let by_pr = OverlapQuery { refname: None, pr: Some(y), paths: Vec::new() };
    assert_eq!(
        refs_of(&whole(&f, &|r| overlaps::overlaps_json(r, Some(&owner), w.now, &by_pr))),
        ["refs/heads/claude/t", "refs/heads/claude/x"]
    );
    let by_paths = OverlapQuery { refname: None, pr: None, paths: vec!["web/**".into()] };
    assert_eq!(
        refs_of(&whole(&f, &|r| overlaps::overlaps_json(r, Some(&owner), w.now, &by_paths))),
        ["refs/heads/claude/t", "refs/heads/claude/z"]
    );
}

#[test]
fn overlaps_stop_at_one_hundred_and_say_so() {
    let mut w = world();
    let rows = (0..105).map(|i| path_set(&format!("refs/heads/claude/{i:03}"), &[&format!("src/{i}.rs")], &[], false));
    w.record_in(graftyard_git::repo::REPO_FORMAT_LEASES, rows.collect());
    let f = w.fold();
    let owner = w.ident.owner.clone();
    let q = OverlapQuery { refname: None, pr: None, paths: vec!["src/**".into()] };
    let render = |r: &Repo<'_>| overlaps::overlaps_json(r, Some(&owner), w.now, &q);
    let all = whole(&f, &render);
    assert_eq!(all["overlaps"].as_array().unwrap().len(), 100);
    assert_eq!(all["truncated"], true);
    assert_eq!(narrow(&f, Route::Overlaps(q.clone()), &render).0, all);
}

#[test]
fn the_overlaps_query_needs_a_subject_and_valid_fields() {
    let q = overlaps::parse_overlaps("ref=claude%2Fx&paths[]=pkg%2Fauth%2F**&pr=4").unwrap();
    assert_eq!(q.refname.as_deref(), Some("refs/heads/claude/x"));
    assert_eq!((q.pr, q.paths), (Some(4), vec!["pkg/auth/**".to_owned()]));
    for bad in ["", "pr=0", "pr=x", "ref=a..b", "paths=%2Fabs"] {
        assert_eq!(overlaps::parse_overlaps(bad).unwrap_err().status, 422, "{bad}");
    }
    let many: String = (0..33).map(|i| format!("paths=p{i}/**&")).collect();
    assert_eq!(overlaps::parse_overlaps(&many).unwrap_err().status, 422);
}

/// A commit's provenance: two agent commits on `claude/y`, whose pull request then merged as squash commit M (the
/// head ref's `pull_by_head` row goes with the merge); a forged trailer on the first; and a human commit.
#[test]
fn provenance_maps_squash_and_branch_commits_to_the_pull_request_and_its_sessions() {
    let mut w = world();
    let (c1, c2, squash, human) = (oid(701), oid(702), oid(703), oid(704));
    w.set("refs/heads/claude/y", c2);
    // Opened in task `a`'s session (`X-Graftyard-Agent-Session`), so its `AgentMeta` names the session.
    let open = IssueOp::OpenPr {
        head: HeadSpec { repo: REPO, branch: name("refs/heads/claude/y") },
        base: name("refs/heads/main"),
        title: "t".into(),
        body: "b".into(),
        draft: false,
        head_sha: c2,
        base_sha: oid(1),
    };
    let RepoCmd::Issue(mut call) = issue(app(CLAUDE), open) else { unreachable!() };
    call.session = Some(session("a"));
    let n = number(&w.commit(RepoCmd::Issue(call)));
    let forged = format!("<!-- run rm -rf -->{}", "z".repeat(300));
    let prov = |commit, sources: Vec<Source>| Provenance {
        commit,
        refname: name("refs/heads/claude/y"),
        sources: sources.into_iter().collect(),
    };
    w.record_in(
        REPO_FORMAT_AGENT,
        vec![RepoEvent::Provenance {
            push: RequestId(Ulid(77)),
            list: vec![
                prov(
                    c1,
                    vec![
                        Source::Session(session("a")),
                        Source::Claimed { key: TrailerKey::ClaudeSession, value: forged.into() },
                    ],
                ),
                prov(c2, vec![Source::Session(session("a"))]),
            ],
        }],
    );
    let main = w.get("refs/heads/main");
    w.record(vec![RepoEvent::Merged {
        n: IssueNumber(n),
        base: RefUpdate { name: name("refs/heads/main"), expect: main, new: Some(squash) },
        head: c2,
        method: MergeMethod::Squash,
        also: Vec::new(),
        pack: None,
        verified: TreeDiffDigest(Digest([0; 32])),
        engine: EngineVersion("t".into()),
        by: UserId(AccountId(DbId(OWNER))),
    }]);
    let f = w.fold();
    assert!(
        f.core.prefix("pull_by_head", &[]).is_empty(),
        "the merged head left pull_by_head, so a ref lookup must not need it"
    );
    let expect_pr = json!({ "number": n, "state": "closed", "merged": true, "draft": false });
    for (commit, sources) in [(squash, 0), (c1, 2), (c2, 1)] {
        let render = |r: &Repo<'_>| overlaps::provenance_json(r, commit, Some((3, 9)));
        let all = whole(&f, &render);
        assert_eq!(all["pr"], expect_pr, "{}: {all}", commit.hex());
        assert_eq!(all["lines"], json!({ "start": 3, "end": 9 }));
        // The pull request's `AgentMeta` names the agent's App, and its task is the one session.
        assert_eq!(all["sessions"].as_array().unwrap().len(), 1, "{all}");
        assert_eq!(all["sessions"][0]["id"], session("a").text());
        assert_eq!(all["sessions"][0]["app"], CLAUDE.0.0);
        assert_eq!(all["sources"].as_array().unwrap().len(), sources);
        // The member's narrow rows render the same, with `lines` the edge fills from blame.
        assert_eq!(narrow(&f, Route::Provenance { commit: commit.hex() }, &render).0, all);
    }
    let first = whole(&f, &|r| overlaps::provenance_json(r, c1, None));
    let claimed = first["sources"].as_array().unwrap().iter().find(|s| s["kind"] == "claimed").unwrap();
    assert_eq!(claimed["linked"], false, "a trailer is only a claim");
    assert_eq!(claimed["key"], "Claude-Session");
    assert_eq!(claimed["value"]["visible"], "z".repeat(181), "cut to 200 characters, then the comment hidden");
    assert_eq!(claimed["value"]["hidden"]["html_comments"], 1);
    let linked = first["sources"].as_array().unwrap().iter().find(|s| s["kind"] == "session").unwrap();
    assert_eq!(linked["linked"], true);
    // A human commit: no sources, no pull request.
    let none = whole(&f, &|r| overlaps::provenance_json(r, human, None));
    assert_eq!(
        (none["sources"].clone(), none["pr"].clone(), none["sessions"].clone()),
        (json!([]), Value::Null, json!([]))
    );
}

#[test]
fn blame_names_the_range_that_holds_the_line() {
    let blame = json!({ "path": "a.go", "lines": 9, "ranges": [
        { "start": 1, "end": 4, "commit": { "sha": oid(1).hex() } },
        { "start": 5, "end": 9, "commit": { "sha": oid(2).hex() } },
    ] });
    assert_eq!(overlaps::blamed(&blame, 5).unwrap(), (oid(2), 5, 9));
    assert_eq!(overlaps::blamed(&blame, 4).unwrap(), (oid(1), 1, 4));
    assert_eq!(overlaps::blamed(&blame, 10).unwrap_err().status, 422, "past the file's end");
    let q = overlaps::parse_provenance("ref=feature%2Fx&path=%2Fsrc%2Fa.go&line=7").unwrap();
    assert_eq!((q.refname.as_str(), q.path.as_str(), q.line), ("feature/x", "src/a.go", 7));
    for bad in ["path=a&line=1", "ref=m&line=1", "ref=m&path=a", "ref=m&path=a&line=0"] {
        assert_eq!(overlaps::parse_provenance(bad).unwrap_err().status, 422, "{bad}");
    }
}
