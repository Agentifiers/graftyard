//! Narrow API reads (`graftyard_api::narrow`): a member answers only the rows one reply renders, with the selections
//! it made over its whole fold, and the edge renders the same reply from them. Every case checks the edge's reply
//! against the reply over both whole tiers, as bytes on the wire, and the rows the member sent against the rows the
//! reply needs. The repository has 40 issues and pull requests whose creation, update, and comment orders all differ
//! from their numbers, so a page cut from the wrong order, a filter applied after paging, or a count over the page
//! differs from the whole-tier reply.

mod common;

use common::*;
use graftyard_api::hosted::{HostedDir, TierRows};
use graftyard_api::narrow::{Picks, Route, Spec, answer, rest_get};
use graftyard_api::view::Repo;
use graftyard_api::wire::Host;
use graftyard_fold::{MemStore, Read};
use graftyard_git::issue_ops::{IssueOp, IssueOut};
use graftyard_git::issues::{
    AppId, CheckConclusion, CheckRunId, CheckRunUpdate, CheckStatus, CheckSuiteId, CommentTarget, DiffSide,
    IssueChange, IssueNumber, LabelId, LabelName, LineAnchor, RepoSettings, StatusId, StatusState, StatusUpdate,
    Visibility,
};
use graftyard_types::{AccountId, DbId, Index, Timestamp};
use serde_json::{Value, json};
use smallvec::{SmallVec, smallvec};

const GH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/gh");
/// An account only a review request names, on the same name shard as `WRITER`, so the shard cut alone would not drop
/// its login.
fn unnamed() -> i64 {
    let shard = graftyard_identity::account_shard(AccountId(DbId(WRITER)));
    (200..).find(|i| graftyard_identity::account_shard(AccountId(DbId(*i))) == shard).unwrap()
}

/// One item as the test reads it straight from the metadata rows, independently of `Repo` and its selections.
struct Row {
    n: u32,
    pull: bool,
    open: bool,
    merged: bool,
    created: u64,
    updated: u64,
    comments: u32,
    labels: Vec<String>,
}

fn rows(c: &Chain) -> Vec<Row> {
    use graftyard_git::tiers::{IssueRow, LabelRow, get_rec};
    let m = &c.members[0].meta;
    (1..=60u32)
        .filter_map(|n| {
            let r: IssueRow = get_rec(m, "issue", &[graftyard_fold::Val::Int(i64::from(n))])?;
            let labels = m
                .prefix("issue_label", &[graftyard_fold::Val::Int(i64::from(n))])
                .into_iter()
                .filter_map(|(k, _)| get_rec::<LabelRow>(m, "label", &[k[1].clone()]))
                .map(|l| l.name.0.to_lowercase())
                .collect();
            let head: Option<graftyard_git::issues::PullHead> =
                get_rec(&c.members[0].core, "pull_head", &[graftyard_fold::Val::Int(i64::from(n))]);
            Some(Row {
                n,
                pull: r.kind == graftyard_git::issues::IssueKind::PullRequest,
                open: r.open,
                merged: head.is_some_and(|h| matches!(h.state, graftyard_git::issues::PrState::Merged { .. })),
                created: r.created.0,
                updated: r.updated.0,
                comments: r.comments,
                labels,
            })
        })
        .collect()
}

fn param<'q>(q: &'q str, k: &str) -> Option<&'q str> {
    q.split('&').filter_map(|kv| kv.split_once('=')).find(|(a, _)| *a == k).map(|(_, v)| v)
}

/// `GET /issues`'s page by GHES's documented rules, from `rows`: state, then every label, then sort and direction
/// over all of them, then the page. The numbers, and whether a next page exists.
fn rest_oracle(c: &Chain, q: &str) -> (Vec<u32>, bool) {
    let mut all: Vec<Row> = rows(c)
        .into_iter()
        .filter(|r| match param(q, "state") {
            Some("closed") => !r.open,
            Some("all") => true,
            _ => r.open,
        })
        .filter(|r| param(q, "labels").is_none_or(|ls| ls.split(',').all(|l| r.labels.contains(&l.to_owned()))))
        .collect();
    let key = |r: &Row| match param(q, "sort") {
        Some("updated") => (r.updated, r.n),
        Some("comments") => (u64::from(r.comments), r.n),
        _ => (r.created, r.n),
    };
    all.sort_by_key(key);
    if param(q, "direction") != Some("asc") {
        all.reverse();
    }
    let per = param(q, "per_page").and_then(|v| v.parse().ok()).unwrap_or(30usize).min(100);
    let page = param(q, "page").and_then(|v| v.parse().ok()).unwrap_or(1usize);
    let page_items = all.iter().skip((page - 1) * per).take(per).map(|r| r.n).collect();
    (page_items, all.len() > page * per)
}

/// A connection's page by number, newest first: the items of `pull` kind in `states`, after the cursor, first `n`;
/// with the count of every match and whether more follow.
fn gql_oracle(c: &Chain, pull: bool, states: &[&str], after: Option<u32>, n: usize) -> (Vec<u32>, usize, bool) {
    let state = |r: &Row| match (r.open, r.merged) {
        (true, _) => "OPEN",
        (false, true) => "MERGED",
        (false, false) => "CLOSED",
    };
    let mut all: Vec<Row> =
        rows(c).into_iter().filter(|r| r.pull == pull && (states.is_empty() || states.contains(&state(r)))).collect();
    all.sort_by_key(|r| std::cmp::Reverse(r.n));
    let total = all.len();
    let page: Vec<u32> = all.iter().map(|r| r.n).filter(|i| after.is_none_or(|a| *i < a)).collect();
    (page.iter().copied().take(n).collect(), total, page.len() > n)
}

fn host() -> Host {
    Host::new("ghes.example.com", false).unwrap()
}

fn settings() -> RepoSettings {
    RepoSettings {
        owner: AccountId(DbId(OWNER)),
        enterprise: None,
        visibility: Visibility::Public,
        default_branch: name("refs/heads/main"),
        archived: false,
        description: None,
        homepage: None,
        topics: SmallVec::new(),
        has_issues: true,
        has_wiki: false,
        has_projects: false,
        merge_methods: smallvec![graftyard_git::issues::MergeMethod::Merge],
        delete_branch_on_merge: false,
        allow_auto_merge: false,
        allow_update_branch: false,
        check_retention_days: 90,
    }
}

/// 40 items: every third an issue, the rest pull requests (drafts among them), some closed, labelled `bug` or `ui`,
/// assigned, commented, and touched again in an order unrelated to their numbers; plus review comments, check runs,
/// statuses, and a release.
fn big() -> Chain {
    let mut c = Chain::new(1);
    c.push("refs/heads/main", oid(1));
    c.commit(OWNER, IssueOp::Settings(settings())).unwrap();
    let label = |c: &mut Chain, n: &str, color| match c
        .commit(WRITER, IssueOp::DefineLabel { id: None, name: LabelName(n.into()), color, description: None })
        .unwrap()
    {
        IssueOut::Id(id) => LabelId(id),
        o => panic!("{o:?}"),
    };
    let (bug, ui) = (label(&mut c, "bug", [1, 2, 3]), label(&mut c, "ui", [4, 5, 6]));
    // Creation times out of number order: item n is created at a time that is a permutation of n.
    for n in 1..=40u32 {
        c.now = Timestamp(1_793_000_000_000 + u64::from((n * 17) % 41) * 1_000);
        let labels: SmallVec<[LabelId; 4]> = match n % 4 {
            0 => smallvec![bug],
            1 => smallvec![bug, ui],
            2 => smallvec![ui],
            _ => SmallVec::new(),
        };
        let assignees = if n % 5 == 0 { smallvec![AccountId(DbId(WRITER))] } else { SmallVec::new() };
        let author = if n % 2 == 0 { READER } else { WRITER };
        if n % 3 == 0 {
            c.commit(
                author,
                IssueOp::OpenIssue { title: format!("issue {n}").into(), body: "b".into(), labels, assignees },
            )
            .unwrap();
        } else {
            let branch = format!("refs/heads/b{n}");
            c.push(&branch, oid(100 + n as u8));
            c.commit(
                author,
                IssueOp::OpenPr {
                    head: graftyard_git::issues::HeadSpec { repo: REPO, branch: name(&branch) },
                    base: name("refs/heads/main"),
                    title: format!("pr {n}").into(),
                    body: "p".into(),
                    draft: n % 7 == 0,
                    head_sha: oid(100 + n as u8),
                    base_sha: oid(1),
                },
            )
            .unwrap();
            if !labels.is_empty() {
                c.commit(WRITER, IssueOp::ApplyLabels { n: IssueNumber(n), add: labels, remove: SmallVec::new() })
                    .unwrap();
            }
        }
    }
    // Updates and comments in another order again; a few closed.
    for (i, n) in [7u32, 33, 2, 18, 40, 11, 25, 4, 29, 13].into_iter().enumerate() {
        c.now = Timestamp(1_793_100_000_000 + i as u64 * 1_000);
        for _ in 0..(n % 4) {
            c.commit(
                READER,
                IssueOp::Comment { on: CommentTarget::Issue(IssueNumber(n)), body: "c".into(), reply_to: None },
            )
            .unwrap();
        }
        c.commit(OWNER, IssueOp::Change { n: IssueNumber(n), change: IssueChange::Title(format!("t{n}").into()) })
            .unwrap();
    }
    for n in [5u32, 6, 12, 30, 31] {
        c.commit(OWNER, IssueOp::Close { n: IssueNumber(n) }).unwrap();
    }
    // Review comments on two pull requests, created out of id order.
    let at = |commit, line| LineAnchor { commit, path: "a.rs".into(), line, start_line: None, side: DiffSide::Right };
    for (pr, line) in [(2u32, 3u32), (4, 1), (2, 1)] {
        c.now = Timestamp(c.now.0 + 1_000);
        c.commit(
            READER,
            IssueOp::Comment {
                on: CommentTarget::Review { n: IssueNumber(pr), review: None, at: at(oid(100 + pr as u8), line) },
                body: format!("rc {pr}/{line}").into(),
                reply_to: None,
            },
        )
        .unwrap();
    }
    // A review requested of an account no other row names, whose login a whole-tier read never fetched.
    c.commit(
        OWNER,
        IssueOp::RequestReviews {
            n: IssueNumber(2),
            reviewers: smallvec![graftyard_git::issues::Grantee::User(graftyard_identity::UserId(AccountId(DbId(
                unnamed()
            ))))],
            removed: false,
        },
    )
    .unwrap();
    // #2 is an agent's pull request, which the trust view reads from its `agent_meta` row.
    let meta = graftyard_git::agent::AgentMeta { app: AppId(DbId(7)), session: None, requester: None, repo: REPO };
    c.entry(
        graftyard_raft::EntryBody::Events(vec![graftyard_git::repo::RepoEvent::AgentMeta { n: IssueNumber(2), meta }]),
        graftyard_raft::TierSet::CORE,
    );
    for (id, sha, name) in [(50i64, 102u8, "build"), (51, 102, "test"), (52, 104, "build")] {
        c.commit(
            WRITER,
            IssueOp::RecordCheck(CheckRunUpdate {
                run: CheckRunId(DbId(id)),
                suite: CheckSuiteId(DbId(1)),
                app: AppId(DbId(5)),
                head_sha: oid(sha),
                name: name.into(),
                version: 1,
                status: CheckStatus::Completed,
                conclusion: Some(CheckConclusion::Success),
                started: Some(Timestamp(1_700_000_000_000)),
                completed: Some(Timestamp(1_700_000_060_000)),
                external_id: None,
                details_url: None,
                output: None,
            }),
        )
        .unwrap();
    }
    for (id, sha) in [(60i64, 102u8), (61, 102), (62, 104)] {
        c.commit(
            WRITER,
            IssueOp::RecordStatus(StatusUpdate {
                id: StatusId(DbId(id)),
                sha: oid(sha),
                context: "ci".into(),
                state: StatusState::Success,
                target_url: None,
                description: None,
                creator: AccountId(DbId(WRITER)),
            }),
        )
        .unwrap();
    }
    // Two merged pull requests, so CLOSED and MERGED select different items while `issue_order` files both as not
    // open: a connection paged from the order's keys alone for `states: [CLOSED]` would list the merged ones too.
    for (n, tip) in [(8u32, 200u8), (26, 201)] {
        let main = c.repo(0, &host(), &Dir::new(WRITER)).ref_value(&name("refs/heads/main"));
        let ev = graftyard_git::repo::RepoEvent::Merged {
            n: IssueNumber(n),
            base: graftyard_git::refs::RefUpdate { name: name("refs/heads/main"), expect: main, new: Some(oid(tip)) },
            head: oid(100 + n as u8),
            method: graftyard_git::issues::MergeMethod::Merge,
            also: Vec::new(),
            pack: None,
            verified: graftyard_git::merge::TreeDiffDigest(graftyard_types::Digest([0; 32])),
            engine: graftyard_git::merge::EngineVersion("t".into()),
            by: graftyard_identity::UserId(AccountId(DbId(OWNER))),
        };
        let tag = graftyard_git::tiers::tier_of(&ev);
        c.entry(graftyard_raft::EntryBody::Events(vec![ev]), tag);
    }
    c
}

/// The bytes the edge sends: status, headers, body.
fn wire(r: &graftyard_wire::Reply) -> Vec<u8> {
    framed(None, graftyard_wire::Surface::Rest, r.clone())
}

/// What the member answers for `route` over member 0's tiers.
fn member(c: &Chain, route: Route) -> Value {
    let m = &c.members[0];
    let spec = Spec { at: Dir::new(WRITER).home(), route };
    let named = || graftyard_api::hosted::accounts(&m.core, &m.meta);
    answer(&m.core, Some(&m.meta), 9, &serde_json::to_value(spec).unwrap(), &named)
}

/// The narrow rows as the edge loads them, and the directory with the member's picks.
fn edge_side(v: &Value) -> (MemStore, MemStore, Picks) {
    let rows = TierRows::from_value(v).expect("a narrow answer is tier rows");
    assert!(rows.complete);
    (TierRows::store(&rows.core), TierRows::store(rows.meta.as_ref().unwrap()), Picks::of(rows.picks.clone()))
}

/// A directory that knows every login and permission, with the member's picks, as `HostedDir` holds them.
struct Edge<'a> {
    base: Dir,
    picks: &'a Picks,
}

impl graftyard_api::view::Directory for Edge<'_> {
    fn login(&self, id: AccountId) -> Option<String> {
        self.base.login(id)
    }
    fn name(&self, id: AccountId) -> Option<String> {
        self.base.name(id)
    }
    fn is_org(&self, id: AccountId) -> bool {
        self.base.is_org(id)
    }
    fn by_login(&self, l: &str) -> Option<AccountId> {
        self.base.by_login(l)
    }
    fn permission(&self, id: AccountId) -> Option<graftyard_identity::RepoPermission> {
        self.base.permission(id)
    }
    fn repo(&self, id: graftyard_types::RepoId) -> Option<graftyard_api::view::RepoRef> {
        self.base.repo(id)
    }
    fn ref_of(
        &self,
        r: graftyard_types::RepoId,
        n: &graftyard_git::refs::RefName,
    ) -> Option<graftyard_git::refs::ObjectId> {
        self.base.ref_of(r, n)
    }
    fn team(&self, id: DbId) -> Option<(String, String, String)> {
        self.base.team(id)
    }
    fn viewer(&self) -> Option<AccountId> {
        self.base.viewer()
    }
    fn compare(
        &self,
        a: graftyard_git::refs::ObjectId,
        b: graftyard_git::refs::ObjectId,
    ) -> graftyard_api::view::Compare {
        self.base.compare(a, b)
    }
    fn pick(&self, key: &str, select: &mut dyn FnMut() -> graftyard_api::view::Pick) -> graftyard_api::view::Pick {
        self.picks.get(key, select)
    }
}

fn rest_route(tail: &[&str], query: &str) -> Route {
    Route::Rest {
        tail: tail.iter().map(|s| (*s).to_owned()).collect(),
        query: query.into(),
        path: format!("/repos/o/r/{}", tail.join("/")),
        agent: false,
    }
}

/// A REST `GET` rendered over the narrow answer and over both whole tiers: the same bytes, and the narrow rows hold
/// no more than the reply read. Returns the narrow core and metadata rows.
fn same_rest(c: &Chain, tail: &[&str], query: &str) -> (MemStore, MemStore) {
    let h = host();
    let whole_dir = Dir::new(WRITER);
    let whole = rest_get(&c.repo(0, &h, &whole_dir), tail, query, Index(9)).unwrap();
    let v = member(c, rest_route(tail, query));
    assert!(v.get("whole").is_none(), "{tail:?}?{query} is answered narrowly: {v}");
    let (core, meta, picks) = edge_side(&v);
    let dir = Edge { base: Dir::new(WRITER), picks: &picks };
    let r = Repo { host: &h, at: whole_dir.home(), core: &core, meta: &meta, dir: &dir };
    let narrow = rest_get(&r, tail, query, Index(9)).unwrap();
    assert!(!picks.missed(), "{tail:?}?{query}: the edge asked for a selection the member did not make");
    assert_eq!(
        String::from_utf8_lossy(&wire(&narrow)),
        String::from_utf8_lossy(&wire(&whole)),
        "{tail:?}?{query}: the narrow reply differs from the whole-tier one"
    );
    // A 404 the description does not document (`check-runs/999`, a release that does not exist) is checked for its
    // bytes only.
    if let (Ok(dir), false) = (std::env::var("GRAFTYARD_REPLIES"), narrow.status == 404) {
        let path = match tail {
            [] => "/repos/{owner}/{repo}",
            ["issues"] => "/repos/{owner}/{repo}/issues",
            ["pulls", _] => "/repos/{owner}/{repo}/pulls/{pull_number}",
            ["issues", _, "comments"] => "/repos/{owner}/{repo}/issues/{issue_number}/comments",
            ["pulls", _, "reviews"] => "/repos/{owner}/{repo}/pulls/{pull_number}/reviews",
            ["pulls", _, "comments"] => "/repos/{owner}/{repo}/pulls/{pull_number}/comments",
            ["commits", _, "check-runs"] => "/repos/{owner}/{repo}/commits/{ref}/check-runs",
            ["commits", _, "statuses"] => "/repos/{owner}/{repo}/commits/{ref}/statuses",
            ["check-runs", _] => "/repos/{owner}/{repo}/check-runs/{check_run_id}",
            ["releases", "tags", _] => "/repos/{owner}/{repo}/releases/tags/{tag}",
            ["releases", _] => "/repos/{owner}/{repo}/releases/{release_id}",
            ["tarball", _] => "/repos/{owner}/{repo}/tarball/{ref}",
            _ => "/repos/{owner}/{repo}/zipball/{ref}",
        };
        let v = json!({ "method": "GET", "path": path, "status": narrow.status, "body": narrow.value(),
            "location": narrow.header("location") });
        let case = format!("narrow-{}-{}", tail.join("-"), query.replace(['&', '=', ','], "_"));
        std::fs::write(format!("{dir}/{case}.rest.json"), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }
    (core, meta)
}

fn gql_route(query: &str, variables: Value) -> Route {
    Route::Graphql { query: query.into(), variables: variables.as_object().cloned(), op: None }
}

fn ask(r: &Repo<'_>, query: &str, variables: &Value) -> Value {
    let req = graftyard_api::exec::Request::parse(query, variables.as_object().cloned(), None).unwrap();
    graftyard_api::graphql::query(&req, Some(r))
}

/// A GraphQL query over the narrow answer and over both whole tiers: the same JSON.
fn same_gql(c: &Chain, query: &str, variables: Value) -> (MemStore, MemStore) {
    let h = host();
    let whole_dir = Dir::new(WRITER);
    let whole = ask(&c.repo(0, &h, &whole_dir), query, &variables);
    let v = member(c, gql_route(query, variables.clone()));
    assert!(v.get("whole").is_none(), "{query} is answered narrowly: {v}");
    let (core, meta, picks) = edge_side(&v);
    let dir = Edge { base: Dir::new(WRITER), picks: &picks };
    let r = Repo { host: &h, at: whole_dir.home(), core: &core, meta: &meta, dir: &dir };
    let narrow = ask(&r, query, &variables);
    assert!(!picks.missed(), "{query}: a selection the member did not make");
    assert_eq!(narrow, whole, "{query} {variables}");
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (query, variables.to_string()).hash(&mut h);
        let v = json!({ "query": query, "variables": variables, "reply": narrow, "document": "narrow" });
        std::fs::write(format!("{dir}/narrow-gql-{:016x}.json", h.finish()), serde_json::to_vec_pretty(&v).unwrap())
            .unwrap();
    }
    (core, meta)
}

/// Issue and pull request numbers whose `issue` rows a narrow answer holds.
fn items(meta: &MemStore) -> Vec<u32> {
    meta.prefix("issue", &[])
        .into_iter()
        .filter_map(|(k, _)| match k.first() {
            Some(graftyard_fold::Val::Int(n)) => Some(*n as u32),
            _ => None,
        })
        .collect()
}

/// `GET /issues` over every sort, direction, state, label filter, and page boundary: the bytes of the whole-tier reply,
/// and only the page's items' rows. A page cut before the sort, a label filter applied after paging, a page off by one,
/// or a `Link` header from the page's own length would each differ here: creation, update, and comment orders differ
/// from number order, and the filters drop items between pages.
#[test]
fn rest_issue_lists_are_the_whole_tier_bytes_and_hold_only_their_page() {
    let c = big();
    let (mut cases, mut partial, mut empty) = (0, 0, 0);
    for state in ["", "state=open&", "state=closed&", "state=all&"] {
        for sort in ["", "sort=created&", "sort=updated&", "sort=comments&"] {
            for dir in ["", "direction=asc&"] {
                for labels in ["", "labels=bug&", "labels=bug,ui&"] {
                    for page in [
                        "per_page=7",
                        "per_page=7&page=2",
                        "per_page=7&page=4",
                        "per_page=7&page=5",
                        "per_page=7&page=6",
                        "per_page=7&page=7",
                        "per_page=100",
                        "page=9",
                    ] {
                        let q = format!("{state}{sort}{dir}{labels}{page}");
                        let (_, meta) = same_rest(&c, &["issues"], &q);
                        let h = host();
                        let d = Dir::new(WRITER);
                        let whole = rest_get(&c.repo(0, &h, &d), &["issues"], &q, Index(9)).unwrap();
                        let got: Vec<u32> = whole
                            .value()
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|i| i["number"].as_u64().unwrap() as u32)
                            .collect();
                        let (want, more) = rest_oracle(&c, &q);
                        assert_eq!(got, want, "{q}: GHES's page");
                        let per = if q.contains("per_page=7") { 7 } else { 30 };
                        partial += usize::from(!got.is_empty() && got.len() < per && q.contains("page="));
                        empty += usize::from(got.is_empty());
                        let next = whole.header("link").is_some_and(|l| l.contains("rel=\"next\""));
                        assert_eq!(next, more, "{q}: a next link iff more items follow");
                        let mut sorted = want.clone();
                        sorted.sort_unstable();
                        assert_eq!(items(&meta), sorted, "{q}: the narrow rows hold exactly the page's items");
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 768);
    assert!(partial > 20 && empty > 20, "last partial pages {partial} and empty pages {empty} are both covered");
    // A page of a list of 40 is under 1/4 of the whole tier's issue rows.
    let (_, meta) = same_rest(&c, &["issues"], "per_page=5");
    assert_eq!(items(&meta).len(), 5);
    assert!(!meta.0.contains_key("comment"), "a list reads no comment rows");
}

/// Single-object routes read only their object: a pull request's GET holds no other item's rows, nor another pull
/// request's head; its reviews, comments, check runs, statuses, and a release are the whole-tier bytes.
#[test]
fn single_object_routes_read_only_their_object() {
    let c = big();
    let (core, meta) = same_rest(&c, &["pulls", "2"], "");
    assert_eq!(items(&meta), [2], "GET /pulls/2 holds no other item");
    let heads: Vec<_> = core.prefix("pull_head", &[]).into_iter().map(|(k, _)| k).collect();
    assert_eq!(heads.len(), 1, "only #2's head: {heads:?}");
    assert!(core.0.get("ref").is_none_or(|r| r.len() <= 2), "the base's ref, not every branch");
    for missing in [&["pulls", "3"][..], &["pulls", "99"], &["pulls", "x"]] {
        let (_, meta) = same_rest(&c, missing, "");
        assert!(items(&meta).len() <= 1, "{missing:?}");
    }
    let (_, meta) = same_rest(&c, &["issues", "7", "comments"], "");
    assert_eq!(items(&meta), [7]);
    for c_ in meta.prefix("comment", &[]) {
        let row: graftyard_git::tiers::CommentRow = graftyard_git::tiers::get_rec(&meta, "comment", &c_.0).unwrap();
        assert!(matches!(row.on, CommentTarget::Issue(IssueNumber(7))), "another item's comment leaked: {row:?}");
    }
    same_rest(&c, &["issues", "7", "comments"], "per_page=1&page=2");
    same_rest(&c, &["pulls", "2", "reviews"], "");
    // Review comments are found by a scan of every comment: the member picks them, and sends only #2's.
    let (_, meta) = same_rest(&c, &["pulls", "2", "comments"], "");
    let bodies: Vec<String> = meta
        .prefix("comment", &[])
        .into_iter()
        .map(|(k, _)| {
            graftyard_git::tiers::get_rec::<graftyard_git::tiers::CommentRow>(&meta, "comment", &k).unwrap().body.into()
        })
        .collect();
    assert_eq!(bodies.len(), 2, "only #2's two review comments: {bodies:?}");
    same_rest(&c, &["pulls", "4", "comments"], "");
    let sha = oid(102).hex();
    let (_, meta) = same_rest(&c, &["commits", &sha, "check-runs"], "");
    assert_eq!(meta.prefix("check_run", &[]).len(), 2, "only the commit's runs");
    same_rest(&c, &["commits", &sha, "check-runs"], "per_page=1&page=2");
    same_rest(&c, &["commits", &sha, "statuses"], "per_page=1");
    let (_, meta) = same_rest(&c, &["commits", &sha, "statuses"], "");
    assert_eq!(meta.prefix("status", &[]).len(), 2, "only the commit's statuses, not the next commit's");
    // A check run by id is found by a scan: the member picks it, and sends only it.
    let (core, meta) = same_rest(&c, &["check-runs", "52"], "");
    assert_eq!(meta.prefix("check_run", &[]).len(), 1);
    // Every narrow answer carries the read gate's rows, so the edge's `authorize_read` decides as on both tiers.
    assert_eq!(TierRows::access_rows(&core), TierRows::access_rows(&c.members[0].core));
    same_rest(&c, &["check-runs", "999"], "");
    same_rest(&c, &[], "");
    same_rest(&c, &["tarball", "main"], "");
    same_rest(&c, &["releases", "tags", "v1"], "");
    same_rest(&c, &["releases", "1"], "");
}

/// GraphQL connections page by `first`/`after` and `last` from the member's selection: every captured gh list and
/// view document, and B0-29's `pullRequests(last: 10)`, at several page sizes and cursors, give the whole-tier reply.
#[test]
fn graphql_connections_are_the_whole_tier_reply() {
    let c = big();
    let b029 = r#"query { repository(owner: "o", name: "r") { pullRequests(last: 10) { nodes { number isDraft mergeable } } } }"#;
    let (_, meta) = same_gql(&c, b029, json!({}));
    assert_eq!(items(&meta).len(), 10, "B0-29's read holds 10 pull requests, not 40");
    // Two connections in one document each get their own selection.
    let both = r#"query { repository(owner: "o", name: "r") { issues(first: 3) { totalCount nodes { number } }
        pullRequests(last: 4, states: [OPEN]) { totalCount pageInfo { hasNextPage endCursor } nodes { number } } } }"#;
    let (_, meta) = same_gql(&c, both, json!({}));
    assert_eq!(items(&meta).len(), 7);
    let list = |f: &str| {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(format!("{GH}/{f}.json")).unwrap()).unwrap();
        (v["query"].as_str().unwrap().to_owned(), v["variables"].clone())
    };
    // An independent page: B0-29's ten newest open-or-not pull requests.
    let h = host();
    let d = Dir::new(WRITER);
    let got = ask(&c.repo(0, &h, &d), b029, &json!({}));
    let nums: Vec<u32> = got["data"]["repository"]["pullRequests"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["number"].as_u64().unwrap() as u32)
        .collect();
    assert_eq!(nums, gql_oracle(&c, true, &[], None, 10).0);
    // Two connections of one kind with different arguments are two selections.
    let two = r#"query { repository(owner: "o", name: "r") { a: pullRequests(first: 2) { nodes { number } }
        b: pullRequests(first: 3, states: [CLOSED]) { totalCount nodes { number } } } }"#;
    same_gql(&c, two, json!({}));
    for f in ["PullRequestList-1", "IssueList-1", "PullRequestList-2", "IssueList-2"] {
        let (q, vars) = list(f);
        let pull = f.starts_with("Pull");
        let root = if pull { "pullRequests" } else { "issues" };
        for (limit, after) in [
            (30, None),
            (5, None),
            (5, Some(30)),
            (5, Some(31)),
            (100, Some(2)),
            (1, Some(40)),
            (30, Some(4)),
            (5, Some(1)),
        ] {
            for (states, want_states) in [
                (json!(["OPEN"]), &["OPEN"][..]),
                (json!(["CLOSED"]), &["CLOSED"][..]),
                (json!(["MERGED"]), &["MERGED"][..]),
                (json!(["CLOSED", "MERGED"]), &["CLOSED", "MERGED"][..]),
                (json!(["OPEN", "MERGED"]), &["OPEN", "MERGED"][..]),
                (json!(["OPEN", "CLOSED", "MERGED"]), &[][..]),
            ] {
                // `IssueState` has no MERGED.
                let states = if pull {
                    states
                } else {
                    json!(["OPEN", "CLOSED"])
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|s| states.as_array().unwrap().contains(s))
                        .cloned()
                        .collect()
                };
                let mut v = vars.clone();
                v["limit"] = json!(limit);
                v["endCursor"] = after.map_or(Value::Null, |a| json!(graftyard_api::wire::cursor(i64::from(a))));
                let mut stated = false;
                for k in ["state", "states"] {
                    if v.get(k).is_some() {
                        v[k] = states.clone();
                        stated = true;
                    }
                }
                let (_, meta) = same_gql(&c, &q, v.clone());
                assert!(items(&meta).len() <= limit, "{f} {v}: no more items than the page");
                let conn = &ask(&c.repo(0, &h, &d), &q, &v)["data"]["repository"][root];
                if !stated || conn.is_null() {
                    continue;
                }
                // An issue has no MERGED state: its filtered list is what the query sends.
                let want_states: Vec<&str> = if pull {
                    want_states.to_vec()
                } else {
                    states.as_array().unwrap().iter().filter_map(Value::as_str).collect()
                };
                let (want, total, more) = gql_oracle(&c, pull, &want_states, after, limit);
                let got: Vec<u32> =
                    conn["nodes"].as_array().unwrap().iter().map(|n| n["number"].as_u64().unwrap() as u32).collect();
                assert_eq!(got, want, "{f} {v}");
                if let Some(t) = conn.get("totalCount") {
                    assert_eq!(t, &json!(total), "{f} {v}: totalCount counts every match");
                }
                if let Some(p) = conn.get("pageInfo") {
                    assert_eq!(p["hasNextPage"], json!(more), "{f} {v}");
                }
            }
        }
    }
    for f in ["PullRequestByNumber-1", "IssueByNumber-1", "PullRequestStatusChecks-1", "RepositoryInfo-1"] {
        let (q, mut v) = list(f);
        for k in ["pr_number", "issue_number", "number"] {
            if v.get(k).is_some() {
                v[k] = json!(2);
            }
        }
        same_gql(&c, &q, v);
    }
}

/// A filter by login (`filterBy`, search's `author:`) selects by the directory, which the member does not hold: those
/// queries, and mutations, are answered `whole`, and the edge reads both tiers as before.
#[test]
fn a_selection_by_login_or_a_mutation_reads_the_whole_tiers() {
    let c = big();
    let (q, mut v) = {
        let d: Value =
            serde_json::from_str(&std::fs::read_to_string(format!("{GH}/IssueList-1.json")).unwrap()).unwrap();
        (d["query"].as_str().unwrap().to_owned(), d["variables"].clone())
    };
    v["author"] = json!("alice");
    assert_eq!(member(&c, gql_route(&q, v)), json!({ "whole": true }));
    let s = r#"query { search(type: ISSUE_ADVANCED, first: 5, query: "repo:o/r type:pr author:bob") { issueCount } }"#;
    assert_eq!(member(&c, gql_route(s, json!({}))), json!({ "whole": true }));
    let m = r#"mutation { closeIssue(input: {issueId: "x"}) { issue { number } } }"#;
    assert_eq!(member(&c, gql_route(m, json!({}))), json!({ "whole": true }));
    assert_eq!(member(&c, rest_route(&["hooks"], "")), json!({ "whole": true }));
    // A search with no login qualifier is narrow, and `last` takes the oldest of the newest-first matches.
    let s = r#"query { search(type: ISSUE_ADVANCED, last: 5, query: "repo:o/r type:pr label:bug") { issueCount nodes { ... on PullRequest { number } } } }"#;
    same_gql(&c, s, json!({}));
    let h = host();
    let d = Dir::new(WRITER);
    let got = ask(&c.repo(0, &h, &d), s, &json!({}));
    let mut all: Vec<u32> =
        rows(&c).into_iter().filter(|r| r.pull && r.labels.contains(&"bug".into())).map(|r| r.n).collect();
    all.sort_unstable_by(|a, b| b.cmp(a));
    let want: Vec<Value> = all[all.len() - 5..].iter().map(|n| json!({ "number": n })).collect();
    assert_eq!(got["data"]["search"]["nodes"], json!(want));
    assert_eq!(got["data"]["search"]["issueCount"], json!(all.len()));
}

/// Stale reads: an answer is the rows at the member's state. After a new issue commits, the next answer lists it,
/// and an edge that renders an older answer's rows with a newer answer's picks would not match either.
#[test]
fn a_narrow_answer_is_the_state_it_was_read_at() {
    let mut c = big();
    let route = || rest_route(&["issues"], "per_page=3");
    let before = member(&c, route());
    c.now = Timestamp(1_793_900_000_000);
    c.commit(
        READER,
        IssueOp::OpenIssue {
            title: "new".into(),
            body: "".into(),
            labels: SmallVec::new(),
            assignees: SmallVec::new(),
        },
    )
    .unwrap();
    let after = member(&c, route());
    assert_ne!(before, after);
    let (_, meta, picks) = edge_side(&after);
    assert_eq!(
        picks.held.as_ref().unwrap().iter().find(|(k, _)| k.starts_with("issues?")).unwrap().1.items[0],
        vec![graftyard_fold::Val::Int(41)]
    );
    assert!(items(&meta).contains(&41), "the newest issue is on the first page");
    same_rest(&c, &["issues"], "per_page=3");
}

/// The member's answer carries the logins a whole-tier read knew: the accounts the reply renders that the whole tiers
/// name, on the whole-tier read's lowest name shards. The edge reads exactly those, so a reply never names an account
/// a whole-tier reply would render as `ghost`, and never misses one it would name.
#[test]
fn the_logins_are_the_whole_tier_reads_for_the_accounts_the_reply_names() {
    let c = big();
    let v = member(&c, rest_route(&["issues"], "per_page=100"));
    let (_, _, picks) = edge_side(&v);
    let mut got = picks.login_accounts(Some(AccountId(DbId(WRITER))), AccountId(DbId(OWNER))).unwrap();
    got.sort();
    assert_eq!(got, [AccountId(DbId(WRITER)), AccountId(DbId(READER))]);
    // The requested reviewer is asked for but named by no other row: a whole-tier read never fetched its login.
    let q = r#"query { repository(owner: "o", name: "r") { pullRequest(number: 2) { reviewRequests(first: 5) {
        nodes { requestedReviewer { ... on User { login } } } } } } }"#;
    let (_, _, picks) = edge_side(&member(&c, gql_route(q, json!({}))));
    let got = picks.login_accounts(None, AccountId(DbId(OWNER))).unwrap();
    assert!(!got.contains(&AccountId(DbId(unnamed()))), "{got:?}");
    // A viewer the tiers do not name is read, as a whole-tier read's edge added it.
    let got = picks.login_accounts(Some(AccountId(DbId(unnamed()))), AccountId(DbId(OWNER))).unwrap();
    assert!(got.contains(&AccountId(DbId(unnamed()))), "{got:?}");
    let none = member(&c, rest_route(&["check-runs", "50"], ""));
    let (_, _, picks) = edge_side(&none);
    assert_eq!(
        picks.login_accounts(None, AccountId(DbId(OWNER))).unwrap(),
        Vec::<AccountId>::new(),
        "a check run names no account"
    );
    assert!(
        Picks::of(None).login_accounts(None, AccountId(DbId(OWNER))).is_none(),
        "a whole-tier read reads every account"
    );
    let _ = HostedDir::default();
}

/// The trust view (LF-4): an opted-in reply over the narrow rows is the opted-in reply over both whole tiers.
#[test]
fn the_trust_view_over_narrow_rows_is_the_whole_tier_view() {
    let c = big();
    let h = host();
    let agent = Some(graftyard_api::trust::AGENT_MEDIA_TYPE);
    for (tail, path) in [
        (vec!["issues"], "/repos/o/r/issues"),
        (vec!["pulls", "2"], "/repos/o/r/pulls/2"),
        (vec!["issues", "7", "comments"], "/repos/o/r/issues/7/comments"),
        (vec!["pulls", "2", "comments"], "/repos/o/r/pulls/2/comments"),
    ] {
        let d = Dir::new(WRITER);
        let whole_r = c.repo(0, &h, &d);
        let whole =
            graftyard_api::trust::layer(agent, path, &whole_r, rest_get(&whole_r, &tail, "", Index(9)).unwrap());
        let route = Route::Rest {
            tail: tail.iter().map(|s| (*s).to_owned()).collect(),
            query: "".into(),
            path: path.into(),
            agent: true,
        };
        let (core, meta, picks) = edge_side(&member(&c, route));
        let dir = Edge { base: Dir::new(WRITER), picks: &picks };
        let r = Repo { host: &h, at: d.home(), core: &core, meta: &meta, dir: &dir };
        let narrow = graftyard_api::trust::layer(agent, path, &r, rest_get(&r, &tail, "", Index(9)).unwrap());
        assert_eq!(wire(&narrow), wire(&whole), "{path}");
        assert!(narrow.value().to_string().contains("graftyard_trust"), "{path}");
    }
    let dw = Dir::new(WRITER);
    let r = c.repo(0, &h, &dw);
    let pull = graftyard_api::trust::layer(
        agent,
        "/repos/o/r/pulls/2",
        &r,
        rest_get(&r, &["pulls", "2"], "", Index(9)).unwrap(),
    );
    assert_eq!(pull.value()["graftyard_trust"]["author"]["is_agent"], json!(true), "#2 is an agent's pull request");
}

/// An edge whose render asks for a selection the member did not make (a member of an older release) knows it, so it
/// reads both whole tiers instead of rendering from the rows it holds.
#[test]
fn a_selection_the_member_did_not_make_is_known() {
    let c = big();
    let (core, meta, mut picks) = edge_side(&member(&c, rest_route(&["issues"], "per_page=3")));
    picks.held.as_mut().unwrap().retain(|(k, _)| !k.starts_with("issues?"));
    let h = host();
    let d = Dir::new(WRITER);
    let dir = Edge { base: Dir::new(WRITER), picks: &picks };
    let r = Repo { host: &h, at: d.home(), core: &core, meta: &meta, dir: &dir };
    rest_get(&r, &["issues"], "per_page=3", Index(9));
    assert!(picks.missed());
}

/// Counts what a member's narrow build reads: storage statements (a `get`, or a `scan` of up to its limit, as
/// `TierSql` runs each as one SQLite query) and the rows they return.
struct Counted<'a> {
    inner: &'a dyn Read,
    reads: &'a std::cell::Cell<(usize, usize)>,
}

impl Read for Counted<'_> {
    fn get(&self, table: &str, key: &[graftyard_fold::Val]) -> Option<Vec<graftyard_fold::Val>> {
        let v = self.inner.get(table, key);
        let (s, r) = self.reads.get();
        self.reads.set((s + 1, r + usize::from(v.is_some())));
        v
    }
    fn scan(
        &self,
        table: &str,
        from: &[graftyard_fold::Val],
        limit: usize,
    ) -> Vec<(graftyard_fold::Key, Vec<graftyard_fold::Val>)> {
        let v = self.inner.scan(table, from, limit);
        let (s, r) = self.reads.get();
        self.reads.set((s + 1, r + v.len()));
        v
    }
}

/// A repository with `n` open pull requests and an issue, each pull request with one comment, as B0-29's is.
fn wide(n: u32) -> Chain {
    let mut c = Chain::new(1);
    c.push("refs/heads/main", oid(1));
    c.commit(OWNER, IssueOp::Settings(settings())).unwrap();
    for i in 1..=n {
        let branch = format!("refs/heads/b{i}");
        c.push(&branch, oid((i % 200) as u8 + 2));
        let author = if i % 2 == 0 { READER } else { WRITER };
        c.commit(
            author,
            IssueOp::OpenPr {
                head: graftyard_git::issues::HeadSpec { repo: REPO, branch: name(&branch) },
                base: name("refs/heads/main"),
                title: format!("pr {i}").into(),
                body: "p".into(),
                draft: false,
                head_sha: oid((i % 200) as u8 + 2),
                base_sha: oid(1),
            },
        )
        .unwrap();
        c.commit(
            READER,
            IssueOp::Comment { on: CommentTarget::Issue(IssueNumber(i)), body: "c".into(), reply_to: None },
        )
        .unwrap();
    }
    c
}

/// What one narrow build reads, (statements, rows), for the reply and for `named` apart.
fn build_reads(c: &Chain, route: Route) -> ((usize, usize), (usize, usize)) {
    let m = &c.members[0];
    let (reply, named) = (std::cell::Cell::new((0, 0)), std::cell::Cell::new((0, 0)));
    let (core, meta) = (Counted { inner: &m.core, reads: &reply }, Counted { inner: &m.meta, reads: &reply });
    let (nc, nm) = (Counted { inner: &m.core, reads: &named }, Counted { inner: &m.meta, reads: &named });
    let names = || graftyard_api::hosted::accounts(&nc, &nm);
    let spec = Spec { at: Dir::new(WRITER).home(), route };
    let v = answer(&core, Some(&meta), 9, &serde_json::to_value(spec).unwrap(), &names);
    assert!(v.get("whole").is_none() && v.get("error").is_none(), "answered narrowly: {v}");
    (reply.get(), named.get())
}

/// A 30-item page of a 500-pull-request repository reads the order index and the page's items, not every item: at
/// most 200 storage statements and 900 rows for the reply, where loading every item before paging took 1,556 to
/// 1,655 statements and 2,093 to 2,348 rows. A REST list sorts by the `issue` row's keys, so it reads the `issue`
/// table in two scans plus the page, not `issue`, `pull`, and `pull_head` of every item.
#[test]
fn a_page_of_a_wide_repository_reads_its_page_not_every_item() {
    let c = wide(500);
    let rest = |q: &str| Route::Rest {
        tail: vec!["issues".into()],
        query: q.into(),
        path: "/repos/o/r/issues".into(),
        agent: false,
    };
    let gql = |q: &str| gql_route(q, json!({}));
    let cases = [
        ("issues?per_page=30", rest("per_page=30")),
        ("issues?sort=updated&page=9", rest("sort=updated&direction=asc&per_page=30&page=9")),
        (
            "pullRequests(first: 30, OPEN)",
            gql(
                r#"query { repository(owner: "o", name: "r") { pullRequests(first: 30, states: [OPEN]) { totalCount nodes { number title } } } }"#,
            ),
        ),
        (
            "pullRequests(last: 10)",
            gql(
                r#"query { repository(owner: "o", name: "r") { pullRequests(last: 10) { nodes { number isDraft mergeable } } } }"#,
            ),
        ),
    ];
    for (what, route) in cases {
        let ((stmts, rows), _) = build_reads(&c, route);
        assert!(stmts <= 200, "{what}: {stmts} storage statements for a 30-item page");
        assert!(rows <= 900, "{what}: {rows} rows for a 30-item page");
    }
}
