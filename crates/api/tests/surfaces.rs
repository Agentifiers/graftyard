//! Every GraphQL document gh 2.102.1 sent for `gh pr list/view/create/checks/merge` and `gh issue
//! list/view/create/comment` (`gh/*.json`), run on a repository chain's tiers, plus `GET /commits/{ref}/statuses`.
//!
//! Each reply is written to `$GRAFTYARD_REPLIES` when it is set, and `gh/validate.py` checks every one against the
//! pinned GHES 3.22 SDL and OpenAPI description. The assertions here check behavior the schema cannot: which rows
//! come back, and that a mutation's payload exists only from `&Acked`.

mod common;

use common::*;
use graftyard_api::exec::Request;
use graftyard_api::graphql::{
    Plan, Target, merge_command, merge_request, plan, query, refused, refused_merge, render, render_merge, route,
};
use graftyard_api::rest::list_commit_statuses;
use graftyard_api::view::Repo;
use graftyard_api::wire::Page;
use graftyard_api::wire::{Host, NodeId, NodeType};
use graftyard_fold::store::META;
use graftyard_fold::{MemStore, fold_digest, meta};
use graftyard_git::issue_ops::{IssueErr, IssueOp, IssueOut};
use graftyard_git::issues::{
    AppId, CheckConclusion, CheckRunId, CheckRunUpdate, CheckStatus, CheckSuiteId, CommentTarget, IssueNumber,
    LabelName, MergeMethod, Protection, RepoSettings, RequiredChecks, StatusId, StatusState, StatusUpdate, Url,
    Visibility,
};
use graftyard_raft::{EntryBody, TierSet};
use graftyard_types::{AccountId, DbId, Tier, Timestamp};
use serde_json::{Map, Value, json};
use smallvec::{SmallVec, smallvec};

const GH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/gh");

struct Doc {
    name: String,
    query: String,
    variables: Option<Map<String, Value>>,
}

fn doc(file: &str) -> Doc {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(format!("{GH}/{file}.json")).unwrap()).unwrap();
    Doc {
        name: file.into(),
        query: v["query"].as_str().unwrap().into(),
        variables: v["variables"].as_object().cloned(),
    }
}

fn every_doc() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(GH)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok()?.strip_suffix(".json").map(str::to_owned))
        .collect();
    v.sort();
    v
}

/// Writes a reply for `gh/validate.py`, after checking that the trust view leaves its bytes alone.
fn keep(case: &str, d: &Doc, vars: &Option<Map<String, Value>>, reply: &Value) {
    let framed = graftyard_api::wire::Reply::json(200, reply.clone());
    same_bytes_without_opt_in(case, graftyard_wire::Surface::Graphql, None, &framed);
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        let v = json!({ "query": d.query, "variables": vars, "reply": reply, "document": d.name });
        std::fs::write(format!("{dir}/{case}.json"), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }
}

fn keep_rest(case: &str, path: &str, reply: &graftyard_api::wire::Reply) {
    same_bytes_without_opt_in(case, graftyard_wire::Surface::Rest, Some(("GET", path)), reply);
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        let v = json!({ "method": "GET", "path": path, "status": reply.status, "body": reply.value() });
        std::fs::write(format!("{dir}/{case}.rest.json"), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }
}

/// `keep_rest` for a write route.
fn keep_rest_as(case: &str, method: &str, path: &str, reply: &graftyard_api::wire::Reply) {
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        let v = json!({ "method": method, "path": path, "status": reply.status, "body": reply.value() });
        std::fs::write(format!("{dir}/{case}.rest.json"), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }
}

/// The `rel="next"` URL of a reply's `link` header, as `octokit.paginate` follows it.
fn next(r: &graftyard_api::wire::Reply) -> Option<String> {
    r.header("link")?.split(", ").find(|l| l.ends_with("rel=\"next\"")).map(|l| l.split(';').next().unwrap().to_owned())
}

fn settings() -> RepoSettings {
    RepoSettings {
        owner: AccountId(DbId(OWNER)),
        enterprise: None,
        visibility: Visibility::Private,
        default_branch: name("refs/heads/main"),
        archived: false,
        description: Some("a repository".into()),
        homepage: None,
        topics: SmallVec::new(),
        has_issues: true,
        has_wiki: false,
        has_projects: false,
        merge_methods: smallvec![MergeMethod::Merge, MergeMethod::Squash],
        delete_branch_on_merge: false,
        allow_auto_merge: false,
        allow_update_branch: false,
        check_retention_days: 90,
    }
}

fn protect(checks: &[&str]) -> IssueOp {
    IssueOp::SetRule {
        id: None,
        pattern: "main".into(),
        protection: Some(Protection {
            checks: Some(RequiredChecks {
                strict: false,
                checks: checks.iter().map(|c| (Box::from(*c), None)).collect(),
            }),
            reviews: None,
            push: None,
            enforce_admins: false,
            linear_history: false,
            force_pushes: false,
            deletions: false,
            block_creations: false,
            conversation_resolution: true,
            lock_branch: false,
            signatures: false,
            merge_queue: None,
        }),
    }
}

fn run(id: i64, sha: u8, check: &str, conclusion: Option<CheckConclusion>) -> IssueOp {
    IssueOp::RecordCheck(CheckRunUpdate {
        run: CheckRunId(DbId(id)),
        suite: CheckSuiteId(DbId(1)),
        app: AppId(DbId(5)),
        head_sha: oid(sha),
        name: check.into(),
        version: 1,
        status: if conclusion.is_some() { CheckStatus::Completed } else { CheckStatus::InProgress },
        conclusion,
        started: Some(Timestamp(1_700_000_000_000)),
        completed: conclusion.map(|_| Timestamp(1_700_000_060_000)),
        external_id: None,
        details_url: Some(Url(format!("https://ci.example/run/{id}").into())),
        output: None,
    })
}

fn status(id: i64, sha: u8, context: &str, state: StatusState) -> IssueOp {
    IssueOp::RecordStatus(StatusUpdate {
        id: StatusId(DbId(id)),
        sha: oid(sha),
        context: context.into(),
        state,
        target_url: Some(Url("https://jenkins.example/1".into())),
        description: Some("build".into()),
        creator: AccountId(DbId(WRITER)),
    })
}

/// A repository with issue #1 (labelled `bug`, one comment), pull request #2 from `f` with a required `ci` run, a
/// non-required `lint` run, and a `jenkins` status, and a closed issue #3.
fn seeded() -> Chain {
    seeded_on(1)
}

/// `seeded` applied by `members` members.
fn seeded_on(members: usize) -> Chain {
    let mut c = Chain::new(members);
    c.push("refs/heads/main", oid(1));
    c.push("refs/heads/f", oid(2));
    c.commit(OWNER, IssueOp::Settings(settings())).unwrap();
    c.commit(OWNER, protect(&["ci"])).unwrap();
    let bug = match c
        .commit(
            WRITER,
            IssueOp::DefineLabel {
                id: None,
                name: LabelName("bug".into()),
                color: [0xd7, 0x3a, 0x4a],
                description: None,
            },
        )
        .unwrap()
    {
        IssueOut::Id(id) => graftyard_git::issues::LabelId(id),
        o => panic!("{o:?}"),
    };
    c.commit(
        READER,
        IssueOp::OpenIssue {
            title: "it breaks".into(),
            body: "steps".into(),
            labels: smallvec![bug],
            assignees: smallvec![AccountId(DbId(WRITER))],
        },
    )
    .unwrap();
    c.commit(
        READER,
        IssueOp::Comment { on: CommentTarget::Issue(IssueNumber(1)), body: "me too".into(), reply_to: None },
    )
    .unwrap();
    c.commit(
        WRITER,
        IssueOp::OpenPr {
            head: graftyard_git::issues::HeadSpec { repo: REPO, branch: name("refs/heads/f") },
            base: name("refs/heads/main"),
            title: "fix it".into(),
            body: "fixes #1".into(),
            draft: false,
            head_sha: oid(2),
            base_sha: oid(1),
        },
    )
    .unwrap();
    c.commit(WRITER, run(10, 2, "ci", Some(CheckConclusion::Success))).unwrap();
    c.commit(WRITER, run(11, 2, "lint", Some(CheckConclusion::Failure))).unwrap();
    c.commit(WRITER, status(12, 2, "jenkins", StatusState::Pending)).unwrap();
    c.commit(WRITER, status(13, 2, "jenkins", StatusState::Success)).unwrap();
    c.commit(
        READER,
        IssueOp::OpenIssue {
            title: "old".into(),
            body: "".into(),
            labels: SmallVec::new(),
            assignees: SmallVec::new(),
        },
    )
    .unwrap();
    c.commit(OWNER, IssueOp::Close { n: IssueNumber(3) }).unwrap();
    c
}

fn host() -> Host {
    Host::new("ghes.example.com", false).unwrap()
}

/// Replaces the capture fixture's node ids (`I_1`, `PR_1`, `R_1`) in `vars` with this repository's, as gh carries
/// the id from its previous reply into the next request, and points a pull request document at #2.
fn with_ids(d: &Doc, r: &Repo<'_>) -> Option<Map<String, Value>> {
    let mut vars = d.variables.clone();
    if let Some(v) = vars.as_mut() {
        if v.contains_key("pr_number") {
            v.insert("pr_number".into(), json!(2));
        }
        if d.name.starts_with("PullRequestProjectItems") {
            v.insert("number".into(), json!(2));
        }
    }
    let issue = r.item(IssueNumber(1)).unwrap().row.id.0;
    let pr = r.item(IssueNumber(2)).unwrap().row.id.0;
    let ids = [
        ("I_1", NodeId::scoped(NodeType::Issue, REPO, issue).encode()),
        ("PR_1", NodeId::scoped(NodeType::PullRequest, REPO, pr).encode()),
        ("R_1", NodeId::top(NodeType::Repository, REPO.0.0).encode()),
    ];
    let s = serde_json::to_string(&vars).unwrap();
    let s = ids.iter().fold(s, |s, (from, to)| s.replace(&format!("\"{from}\""), &format!("\"{to}\"")));
    serde_json::from_str(&s).unwrap()
}

fn ask(r: &Repo<'_>, d: &Doc, vars: Option<Map<String, Value>>) -> Value {
    let req = Request::parse(&d.query, vars.clone(), None).unwrap();
    match route(&req).unwrap() {
        None => query(&req, None),
        Some(t) => {
            match &t {
                Target::Name { owner, name } => assert_eq!((owner.as_str(), name.as_str()), ("o", "r")),
                Target::Id(id) => assert_eq!(*id, REPO.0.0),
            }
            query(&req, Some(r))
        }
    }
}

#[test]
fn every_captured_query_answers_without_errors() {
    let c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);
    let mut ran = 0;
    for f in every_doc() {
        let d = doc(&f);
        if d.query.trim_start().starts_with("mutation") {
            continue;
        }
        let vars = with_ids(&d, &r);
        let reply = ask(&r, &d, vars.clone());
        assert!(reply.get("errors").is_none(), "{f}: {reply}");
        keep(&f, &d, &vars, &reply);
        ran += 1;
    }
    assert_eq!(
        ran, 28,
        "28 of the 32 captured documents are queries (the 3 `gh release` ones too; tests/blobs.rs runs them on releases)"
    );
}

#[test]
fn lists_hold_exactly_the_rows_the_fold_holds() {
    let c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);
    let issues = ask(&r, &doc("IssueList-1"), doc("IssueList-1").variables);
    let nodes = &issues["data"]["repository"]["issues"]["nodes"];
    assert_eq!(nodes.as_array().unwrap().len(), 1, "only #1 is an open issue: {issues}");
    assert_eq!((nodes[0]["number"].clone(), nodes[0]["labels"]["nodes"][0]["name"].clone()), (json!(1), json!("bug")));
    assert_eq!(issues["data"]["repository"]["issues"]["totalCount"], 1);
    let mut closed = doc("IssueList-1").variables.unwrap();
    closed.insert("states".into(), json!(["CLOSED"]));
    let issues = ask(&r, &doc("IssueList-1"), Some(closed));
    assert_eq!(issues["data"]["repository"]["issues"]["nodes"][0]["number"], 3);
    // `--author` and `--assignee` filter by login.
    let mut by = doc("IssueList-1").variables.unwrap();
    by.insert("author".into(), json!("alice"));
    let none = ask(&r, &doc("IssueList-1"), Some(by.clone()));
    assert_eq!(none["data"]["repository"]["issues"]["totalCount"], 0, "alice opened no issue");
    by.remove("author");
    by.insert("assignee".into(), json!("alice"));
    assert_eq!(ask(&r, &doc("IssueList-1"), Some(by))["data"]["repository"]["issues"]["totalCount"], 1);
    let prs = ask(&r, &doc("PullRequestList-1"), doc("PullRequestList-1").variables);
    let nodes = &prs["data"]["repository"]["pullRequests"]["nodes"];
    assert_eq!(nodes.as_array().unwrap().len(), 1);
    assert_eq!((nodes[0]["number"].clone(), nodes[0]["headRefName"].clone()), (json!(2), json!("f")));
    let search = ask(&r, &doc("IssueSearch-1"), doc("IssueSearch-1").variables);
    let s = &search["data"]["search"];
    assert_eq!((s["issueCount"].clone(), s["nodes"][0]["number"].clone()), (json!(1), json!(1)));
}

#[test]
fn pr_view_and_issue_view_read_both_tiers() {
    let c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);
    let v = ask(&r, &doc("PullRequestByNumber-1"), with_ids(&doc("PullRequestByNumber-1"), &r));
    let p = &v["data"]["repository"]["pullRequest"];
    assert_eq!(p["number"], 2, "{v}");
    assert_eq!(p["title"], "fix it");
    assert_eq!(p["state"], "OPEN");
    assert_eq!(p["author"]["login"], "alice");
    assert_eq!(p["baseRefName"], "main");
    let ctx = &p["statusCheckRollup"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"];
    let names: Vec<&str> =
        ctx.as_array().unwrap().iter().map(|n| n["name"].as_str().or(n["context"].as_str()).unwrap()).collect();
    assert_eq!(names, ["ci", "lint", "jenkins"], "one row per run and the latest status per context");
    assert_eq!(ctx[2]["state"], "SUCCESS", "the newer jenkins status wins");
    let i = ask(&r, &doc("IssueByNumber-2"), doc("IssueByNumber-2").variables);
    let i = &i["data"]["repository"]["issue"];
    assert_eq!((i["__typename"].clone(), i["title"].clone()), (json!("Issue"), json!("it breaks")));
    assert_eq!(i["comments"]["nodes"][0]["body"], "me too");
    assert_eq!(i["comments"]["nodes"][0]["author"]["login"], "bob");
    assert_eq!(i["assignees"]["nodes"][0]["login"], "alice");
    let mut missing = doc("IssueByNumber-2").variables.unwrap();
    missing.insert("number".into(), json!(99));
    let gone = ask(&r, &doc("IssueByNumber-2"), Some(missing));
    assert_eq!(gone["errors"][0]["type"], "NOT_FOUND", "{gone}");
}

#[test]
fn pr_checks_marks_only_the_rule_s_checks_required() {
    let c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);
    let d = doc("PullRequestStatusChecks-1");
    let v = ask(&r, &d, with_ids(&d, &r));
    let nodes = v["data"]["node"]["statusCheckRollup"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"]
        .as_array()
        .unwrap()
        .clone();
    let req: Vec<(String, bool)> = nodes
        .iter()
        .map(|n| (n["name"].as_str().or(n["context"].as_str()).unwrap().into(), n["isRequired"].as_bool().unwrap()))
        .collect();
    assert_eq!(req, [("ci".into(), true), ("lint".into(), false), ("jenkins".into(), false)]);
}

#[test]
fn mutations_commit_then_render_from_acked() {
    let mut c = seeded();
    c.push("refs/heads/g", oid(4));
    let (h, dir) = (host(), Dir::new(WRITER));
    for (f, check) in
        [("IssueCreate-1", "issue"), ("PullRequestCreate-1", "pullRequest"), ("CommentCreate-1", "comment")]
    {
        let d = doc(f);
        let (planned, vars) = {
            let r = c.repo(0, &h, &dir);
            let mut vars = with_ids(&d, &r);
            if f == "PullRequestCreate-1" {
                // gh opens from `g`, since `f` already has an open pull request.
                vars.as_mut().unwrap()["input"]["headRefName"] = json!("g");
            }
            let req = Request::parse(&d.query, vars.clone(), None).unwrap();
            assert_eq!(route(&req).unwrap(), Some(Target::Id(REPO.0.0)));
            (plan(&req, &r).unwrap(), vars)
        };
        let Plan::Issue(op) = planned.plan.clone() else { panic!() };
        let out = c.commit(WRITER, *op).unwrap();
        let acked = ack(out);
        let r = c.repo(0, &h, &dir);
        let req = Request::parse(&d.query, vars.clone(), None).unwrap();
        let reply = render(&req, &planned, &acked, &r);
        assert!(reply.get("errors").is_none(), "{f}: {reply}");
        keep(f, &d, &vars, &reply);
        let url = match check {
            "issue" => reply["data"]["createIssue"]["issue"]["url"].clone(),
            "pullRequest" => reply["data"]["createPullRequest"]["pullRequest"]["url"].clone(),
            _ => reply["data"]["addComment"]["commentEdge"]["node"]["url"].clone(),
        };
        let want = match check {
            "issue" => "https://ghes.example.com/o/r/issues/4".to_string(),
            "pullRequest" => "https://ghes.example.com/o/r/pull/5".to_string(),
            _ => {
                let id = r.comments_of(IssueNumber(1)).last().unwrap().0.0.0;
                format!("https://ghes.example.com/o/r/issues/1#issuecomment-{id}")
            }
        };
        assert_eq!(url, json!(want), "{f}");
    }
    // A refused command renders GitHub's error and never a payload.
    let d = doc("PullRequestCreate-1");
    let r = c.repo(0, &h, &dir);
    let req = Request::parse(&d.query, with_ids(&d, &r), None).unwrap();
    let p = plan(&req, &r).unwrap();
    drop(r);
    let Plan::Issue(op) = p.plan.clone() else { panic!() };
    let err = c.commit(WRITER, *op).unwrap_err();
    assert_eq!(err, IssueErr::Exists);
    assert_eq!(refused(&p, &err)["errors"][0]["type"], "UNPROCESSABLE");
}

#[test]
fn merge_commits_the_merge_command_and_renders_its_payload() {
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);
    let d = doc("PullRequestMerge-1");
    let vars = with_ids(&d, &r);
    let req = Request::parse(&d.query, vars.clone(), None).unwrap();
    let p = plan(&req, &r).unwrap();
    assert_eq!(p.plan, Plan::Merge { n: IssueNumber(2), method: MergeMethod::Merge, expected_head: None });
    // A method the repository does not allow is refused before any commit.
    let mut rebase = vars.clone().unwrap();
    rebase["input"]["mergeMethod"] = json!("REBASE");
    let rq = Request::parse(&d.query, Some(rebase), None).unwrap();
    assert_eq!(plan(&rq, &r).unwrap_err()["errors"][0]["type"], "UNPROCESSABLE");
    // The seeded rule requires `ci`, which passed, and conversation resolution, with none open: the merge commits.
    let mr = merge_request(&p, &r).unwrap();
    assert_eq!((mr.base_old, mr.head), (oid(1), oid(2)));
    drop(r);
    let out = c.commit_merge(merge_command(&p, cache_plan(&mr), who(WRITER))).unwrap();
    let reply = render_merge(&req, &ack(out));
    assert_eq!(reply, json!({ "data": { "mergePullRequest": { "clientMutationId": null } } }));
    keep("PullRequestMerge-1", &d, &vars, &reply);
    let r = c.repo(0, &h, &dir);
    assert_eq!(r.ref_value(&name("refs/heads/main")), Some(out.merge_commit));
    let state = query(
        &Request::parse(
            "query { repository(owner: \"o\", name: \"r\") { pullRequest(number: 2) { state } } }",
            None,
            None,
        )
        .unwrap(),
        Some(&r),
    );
    assert_eq!(state["data"]["repository"]["pullRequest"]["state"], "MERGED");
    // A second merge of the merged pull request is refused before the git cache is asked.
    let e = merge_request(&p, &r).unwrap_err();
    assert_eq!(e["errors"][0]["message"], "Pull request is not open");
}

/// The queue mutations name the pull request as GHES 3.22's inputs do: `pullRequestId`, and `id` for a dequeue.
#[test]
fn the_queue_mutations_read_the_pull_request_by_their_schema_input_names() {
    let c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);
    let pr = NodeId::scoped(NodeType::PullRequest, REPO, r.item(IssueNumber(2)).unwrap().row.id.0).encode();
    let q = "mutation($input: DequeuePullRequestInput!) { dequeuePullRequest(input: $input) { clientMutationId } }";
    let req = Request::parse(q, json!({ "input": { "id": pr } }).as_object().cloned(), None).unwrap();
    assert_eq!(route(&req).unwrap(), Some(graftyard_api::graphql::Target::Id(REPO.0.0)));
    assert_eq!(plan(&req, &r).unwrap().plan, Plan::Dequeue { n: IssueNumber(2) });
    let req = Request::parse(q, json!({ "input": { "pullRequestId": pr } }).as_object().cloned(), None).unwrap();
    assert_eq!(plan(&req, &r).unwrap_err()["errors"][0]["type"], "NOT_FOUND");
}

#[test]
fn a_merge_on_a_stale_plan_or_a_failing_check_renders_githubs_error() {
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);
    let d = doc("PullRequestMerge-1");
    let req = Request::parse(&d.query, with_ids(&d, &r), None).unwrap();
    let p = plan(&req, &r).unwrap();
    let mr = merge_request(&p, &r).unwrap();
    drop(r);
    // The base moved after the plan.
    c.push("refs/heads/main", oid(9));
    let e = c.commit_merge(merge_command(&p, cache_plan(&mr), who(WRITER))).unwrap_err();
    let reply = refused_merge(&p, &e);
    assert_eq!(reply["errors"][0]["message"], "Base branch was modified. Review and try the merge again.");
    assert_eq!(reply["errors"][0]["type"], "UNPROCESSABLE");
    // The required check re-run as a new run, queued: refused with the protection text.
    c.commit(WRITER, run(14, 2, "ci", None)).unwrap();
    let r = c.repo(0, &h, &dir);
    let mr = merge_request(&p, &r).unwrap();
    drop(r);
    let e = c.commit_merge(merge_command(&p, cache_plan(&mr), who(WRITER))).unwrap_err();
    assert_eq!(refused_merge(&p, &e)["errors"][0]["message"], "Required status check \"ci\" is in progress.");
    // A reader may not merge.
    let r = c.repo(0, &h, &dir);
    let mr = merge_request(&p, &r).unwrap();
    drop(r);
    let e = c.commit_merge(merge_command(&p, cache_plan(&mr), who(READER))).unwrap_err();
    assert_eq!(refused_merge(&p, &e)["errors"][0]["type"], "FORBIDDEN");
}

/// A merge plans from `TierRows::merge_rows` alone exactly as from both whole tiers: the same `Planned`, the same
/// `MergeRequest`, and the same refusals (a closed pull request, a method the repository does not allow), so the edge
/// reads only those rows on a fresh leader. The pull request's id names only its own rows.
#[test]
fn a_merge_plans_from_its_rows_as_from_both_tiers() {
    use graftyard_api::hosted::{HostedDir, TierRows};
    let mut c = seeded();
    // A second open pull request, whose rows a merge of #2 must not need.
    c.push("refs/heads/g", oid(5));
    c.commit(
        WRITER,
        IssueOp::OpenPr {
            head: graftyard_git::issues::HeadSpec { repo: REPO, branch: name("refs/heads/g") },
            base: name("refs/heads/main"),
            title: "other".into(),
            body: "".into(),
            draft: false,
            head_sha: oid(5),
            base_sha: oid(1),
        },
    )
    .unwrap();
    let h = host();
    let dir = Dir::new(WRITER);
    let hosted = HostedDir { home: Some(dir.home()), viewer: Some(AccountId(DbId(WRITER))), ..Default::default() };
    let d = doc("PullRequestMerge-1");
    let plan_on = |core: &MemStore, meta: &MemStore, vars: &Option<Map<String, Value>>| {
        let r = Repo { host: &h, at: dir.home(), core, meta, dir: &hosted };
        let req = Request::parse(&d.query, vars.clone(), None).unwrap();
        plan(&req, &r).and_then(|p| merge_request(&p, &r).map(|m| (p, m)))
    };
    let m = &c.members[0];
    let (vars, id) = {
        let r = c.repo(0, &h, &dir);
        (with_ids(&d, &r), r.item(IssueNumber(2)).unwrap().row.id)
    };
    let rows = TierRows::merge_rows(&m.core, Some(&m.meta), id);
    let crossed: TierRows = serde_json::from_value(serde_json::to_value(&rows).unwrap()).unwrap();
    let (core, meta) = (TierRows::store(&crossed.core), TierRows::store(crossed.meta.as_ref().unwrap()));
    let small = plan_on(&core, &meta, &vars).unwrap();
    assert_eq!(small, plan_on(&m.core, &m.meta, &vars).unwrap());
    assert_eq!((small.1.n, small.1.base_old, small.1.head), (IssueNumber(2), oid(1), oid(2)));
    assert!(crossed.core.len() + crossed.meta.as_ref().unwrap().len() <= 6, "{crossed:?}");
    let mut rebase = vars.clone().unwrap();
    rebase["input"]["mergeMethod"] = json!("REBASE");
    assert_eq!(plan_on(&core, &meta, &Some(rebase.clone())), plan_on(&m.core, &m.meta, &Some(rebase)));
    // Once #2 merged, its rows refuse a second merge as both tiers do.
    c.commit_merge(merge_command(&small.0, cache_plan(&small.1), who(WRITER))).unwrap();
    let m = &c.members[0];
    let rows = TierRows::merge_rows(&m.core, Some(&m.meta), id);
    let (core, meta) = (TierRows::store(&rows.core), TierRows::store(rows.meta.as_ref().unwrap()));
    let refused = plan_on(&core, &meta, &vars).unwrap_err();
    assert_eq!(refused, plan_on(&m.core, &m.meta, &vars).unwrap_err());
    assert_eq!(refused["errors"][0]["message"], "Pull request is not open");
}

#[test]
fn an_unserved_field_is_undefined_field_not_null() {
    let c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);
    let q = "query { repository(owner: \"o\", name: \"r\") { pullRequest(number: 2) { number viewerCanUpdate } } }";
    let reply = query(&Request::parse(q, None, None).unwrap(), Some(&r));
    assert_eq!(reply["errors"][0]["type"], "undefinedField");
    assert!(reply.get("data").is_none());
}

#[test]
fn node_ids_in_either_format_reach_the_pull_request_but_a_legacy_one_cannot_route() {
    let c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);
    let pr = r.item(IssueNumber(2)).unwrap().row.id.0;
    let q = "query($id: ID!) { node(id: $id) { ... on PullRequest { number } } }";
    let ask_id = |id: String| {
        let vars = json!({ "id": id }).as_object().cloned();
        let req = Request::parse(q, vars, None).unwrap();
        (route(&req).unwrap(), query(&req, Some(&r)))
    };
    let (t, reply) = ask_id(NodeId::scoped(NodeType::PullRequest, REPO, pr).encode());
    assert_eq!((t, &reply["data"]["node"]["number"]), (Some(Target::Id(REPO.0.0)), &json!(2)));
    // A legacy id carries no repository id, so the edge cannot route it alone; on the repository's tiers it resolves.
    let (t, reply) = ask_id(NodeId::encode_legacy(NodeType::PullRequest, pr));
    assert_eq!((t, &reply["data"]["node"]["number"]), (None, &json!(2)), "{reply}");
    let (_, reply) = ask_id(NodeId::encode_legacy(NodeType::Issue, pr));
    assert_eq!(reply["errors"][0]["type"], "NOT_FOUND", "a pull request's id as an issue's names nothing");
}

#[test]
fn commit_statuses_list_newest_first_and_page() {
    let mut c = seeded();
    for i in 0..5 {
        c.commit(WRITER, status(20 + i, 2, &format!("s{i}"), StatusState::Success)).unwrap();
    }
    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);
    let page = list_commit_statuses(&r, &oid(2).hex(), Page::parse("per_page=3"));
    assert_eq!(page.status, 200);
    let ids: Vec<i64> = page.value().as_array().unwrap().iter().map(|s| s["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, [24, 23, 22]);
    assert!(next(&page).unwrap().contains("page=2"));
    keep_rest("statuses-page1", "/repos/{owner}/{repo}/commits/{ref}/statuses", &page);
    let rest = list_commit_statuses(&r, "f", Page::parse("per_page=3&page=3"));
    let ids: Vec<i64> = rest.value().as_array().unwrap().iter().map(|s| s["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, [12], "by branch name: seven statuses, so page 3 holds the oldest");
    assert_eq!(next(&rest), None, "page 3 is the last");
    assert!(rest.header("link").unwrap().contains("rel=\"prev\""));
    assert_eq!(list_commit_statuses(&r, "nope", Page::parse("")).status, 404);
}

/// Every core row, without the tier's own bookkeeping row (`applied` moves with every entry).
fn core_rows(m: &MemStore) -> MemStore {
    let mut rows = m.clone();
    rows.0.remove(META);
    rows.0.remove(graftyard_fold::DIGESTS);
    rows
}

/// Every context `gh pr checks` reads, paging its own document with `endCursor` as gh does. A commit with no checks
/// has a null rollup, as on GitHub.
fn pr_checks_contexts(r: &Repo<'_>) -> Vec<Value> {
    let d = doc("PullRequestStatusChecks-1");
    let mut vars = with_ids(&d, r).unwrap();
    let mut out = Vec::new();
    loop {
        let v = ask(r, &d, Some(vars.clone()));
        let rollup = &v["data"]["node"]["statusCheckRollup"]["nodes"][0]["commit"]["statusCheckRollup"];
        if rollup.is_null() && v.get("errors").is_none() {
            return out;
        }
        let page = &rollup["contexts"];
        out.extend(page["nodes"].as_array().unwrap_or_else(|| panic!("{v}")).iter().cloned());
        if page["pageInfo"]["hasNextPage"] != true {
            return out;
        }
        vars.insert("endCursor".into(), page["pageInfo"]["endCursor"].clone());
    }
}

/// BF-22's pure half: 10,000 non-required check runs and statuses on the head of an open pull request, read back by
/// `gh pr checks` and `GET /commits/{sha}/statuses`, then pruned once entry time passes `check_retention_days`.
#[test]
fn bf22_non_required_checks_live_in_metadata_and_prune_on_every_member() {
    const EACH: i64 = 5_000;
    let mut c = seeded_on(2);
    let core_before = core_rows(&c.members[0].core);
    let core_sum = meta(&c.members[0].core).sum;
    let ops: Vec<IssueOp> = (0..EACH)
        .map(|i| run(1_000 + i, 2, &format!("lint-{i}"), Some(CheckConclusion::Success)))
        .chain((0..EACH).map(|i| status(100_000 + i, 2, &format!("ctx-{i}"), StatusState::Success)))
        .collect();
    for chunk in ops.chunks(100) {
        for out in c.commit_all(chunk.iter().map(|op| (WRITER, op.clone())).collect()) {
            out.unwrap();
        }
    }
    assert_eq!(c.members[0], c.members[1]);
    assert_eq!(core_rows(&c.members[0].core), core_before, "a non-required check wrote a core row");
    assert_eq!(meta(&c.members[0].core).sum, core_sum);

    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(1, &h, &dir);
    let mut ids = Vec::new();
    for page in 1.. {
        let p = list_commit_statuses(&r, &oid(2).hex(), Page::parse(&format!("per_page=100&page={page}")));
        assert_eq!(p.status, 200);
        ids.extend(p.value().as_array().unwrap().iter().map(|s| s["id"].as_i64().unwrap()));
        if next(&p).is_none() {
            break;
        }
    }
    let want: Vec<i64> = (100_000..100_000 + EACH).rev().chain([13, 12]).collect();
    assert_eq!(ids, want, "every status, newest first");
    let ctx = pr_checks_contexts(&r);
    let runs = ctx.iter().filter(|n| n["__typename"] == "CheckRun").count();
    let statuses = ctx.iter().filter(|n| n["__typename"] == "StatusContext").count();
    assert_eq!((runs, statuses), (EACH as usize + 2, EACH as usize + 1), "ci, lint, and jenkins plus the new rows");
    let required: Vec<&str> =
        ctx.iter().filter(|n| n["isRequired"] == true).map(|n| n["name"].as_str().unwrap()).collect();
    assert_eq!(required, ["ci"]);

    c.now = Timestamp(c.now.0 + 91 * DAY);
    c.entry(EntryBody::Noop, TierSet::NONE);
    let r = c.repo(1, &h, &dir);
    assert!(r.check_runs(&oid(2)).is_empty() && r.check_suites(&oid(2)).is_empty());
    assert_eq!(list_commit_statuses(&r, &oid(2).hex(), Page::parse("")).value(), &json!([]));
    assert!(pr_checks_contexts(&r).is_empty());
    for t in ["check_run", "check_suite", "status", "check_expiry"] {
        assert!(c.members.iter().all(|m| m.meta.0.get(t).is_none_or(|rows| rows.is_empty())), "{t} not pruned");
    }
    assert_eq!(core_rows(&c.members[0].core), core_before, "pruning touched the core tier");
    assert_eq!(meta(&c.members[0].core).sum, core_sum);
    for tier in [Tier::Core, Tier::Metadata] {
        let store = |i: usize| if tier == Tier::Core { &c.members[i].core } else { &c.members[i].meta };
        assert_eq!(fold_digest(store(0), tier), fold_digest(store(1), tier), "{tier:?}");
    }
}

/// The hosted path: the edge reads the tiers as `TierRows` JSON and plans, the command crosses as JSON with its
/// `Render`, and the member renders from `&Acked` over its own tiers. The reply equals the in-process render.
#[test]
fn a_hosted_mutation_renders_on_the_member_as_in_process() {
    use graftyard_api::hosted::{HostedDir, Render, TierRows, What, render_ok};
    use graftyard_git::tiers::{CORE, METADATA};
    let mut c = seeded();
    let h = host();
    let dir = Dir::new(WRITER);
    let rows = |c: &Chain| {
        let (mut budget, m) = (100_000, &c.members[0]);
        let (core, ok1) = TierRows::dump(&m.core, &CORE, &mut budget);
        let (meta, ok2) = TierRows::dump(&m.meta, &METADATA, &mut budget);
        let t = TierRows { index: 1, core, meta: Some(meta), complete: ok1 && ok2 };
        serde_json::from_value::<TierRows>(serde_json::to_value(&t).unwrap()).unwrap()
    };
    let t = rows(&c);
    assert!(t.complete);
    let (core, meta) = (TierRows::store(&t.core), TierRows::store(t.meta.as_ref().unwrap()));
    // The rows `Repo` reads are the member's; the written index and the tier's `META` row do not cross.
    let again = |s: &MemStore, schema| TierRows::dump(s, schema, &mut 100_000).0;
    assert_eq!(again(&core, &CORE), again(&c.members[0].core, &CORE));
    assert_eq!(again(&meta, &METADATA), again(&c.members[0].meta, &METADATA));
    assert!(!t.core.is_empty() && !t.meta.as_ref().unwrap().is_empty());
    // The owner's `Authz` fold crosses too; its maps have tuple keys, which plain JSON cannot hold.
    let mut authz = graftyard_identity::AuthzFold::new(AccountId(DbId(1)));
    authz.opened = Some(graftyard_identity::AccountKind::Org);
    authz.grants.insert(
        (dir.home().id, graftyard_identity::UserId(AccountId(DbId(WRITER)))),
        graftyard_identity::RepoPermission::Write,
    );
    let hosted = HostedDir {
        home: Some(dir.home()),
        authz: Some(authz),
        viewer: Some(AccountId(DbId(WRITER))),
        ..Default::default()
    };
    let crossed: HostedDir = serde_json::from_value(serde_json::to_value(&hosted).unwrap()).unwrap();
    assert_eq!(crossed, hosted);
    let d = doc("CommentCreate-1");
    let (planned, vars) = {
        let r = Repo { host: &h, at: dir.home(), core: &core, meta: &meta, dir: &hosted };
        let vars = with_ids(&d, &r);
        let req = Request::parse(&d.query, vars.clone(), None).unwrap();
        (plan(&req, &r).unwrap(), vars)
    };
    let render_req = Render {
        host: h.name().to_owned(),
        isolation: false,
        dir: hosted.clone(),
        what: What::Graphql {
            query: d.query.clone(),
            variables: vars.clone().map(Value::Object).unwrap_or(Value::Null),
            field: planned.field.clone(),
            subject: planned.subject.map(|n| n.0),
        },
    };
    let render_req: Render = serde_json::from_value(serde_json::to_value(&render_req).unwrap()).unwrap();
    let Plan::Issue(op) = planned.plan.clone() else { panic!() };
    let acked = ack(c.commit(WRITER, *op).unwrap());
    let m = &c.members[0];
    let wire = render_ok(&render_req, &acked, &m.core, &m.meta);
    let req = Request::parse(&d.query, vars, None).unwrap();
    let direct =
        render(&req, &planned, &acked, &Repo { host: &h, at: dir.home(), core: &m.core, meta: &m.meta, dir: &hosted });
    assert_eq!(wire.status, 200);
    assert_eq!(wire.body.as_ref(), Some(&direct));
    assert!(direct["data"]["addComment"]["commentEdge"]["node"]["url"].as_str().unwrap().contains("#issuecomment-"));
    // A refusal renders GitHub's error from the same request.
    let e = graftyard_api::hosted::render_err(&render_req, &IssueErr::NotFound);
    assert_eq!(e.body.unwrap()["errors"][0]["type"], "NOT_FOUND");
}

#[test]
fn mergeable_reads_the_test_merge_of_the_current_head_and_base_tip() {
    use graftyard_git::issues::MergeabilityResult;
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let d = doc("PullRequestByNumber-2");
    let state = doc("PullRequestByNumber-4");
    let ask_pr = |c: &Chain| {
        let r = c.repo(0, &h, &dir);
        let m = ask(&r, &d, with_ids(&d, &r))["data"]["repository"]["pullRequest"]["mergeable"].clone();
        let s = ask(&r, &state, with_ids(&state, &r))["data"]["repository"]["pullRequest"]["mergeStateStatus"].clone();
        (m, s)
    };
    assert_eq!(ask_pr(&c), (json!("UNKNOWN"), json!("UNKNOWN")), "no test merge yet");
    let merged = |c: &mut Chain, base, head, result| {
        let ev = graftyard_git::repo::RepoEvent::TestMerged { n: IssueNumber(2), base, head, result, pack: None };
        c.entry(EntryBody::Events(vec![ev]), TierSet::CORE);
    };
    merged(&mut c, oid(1), oid(2), MergeabilityResult::Clean { merge: oid(9) });
    assert_eq!(ask_pr(&c), (json!("MERGEABLE"), json!("CLEAN")));
    merged(&mut c, oid(1), oid(2), MergeabilityResult::Conflict { paths: smallvec!["f".into()] });
    assert_eq!(ask_pr(&c), (json!("CONFLICTING"), json!("DIRTY")));
    // The base moves: the recorded pair is stale, so `mergeable` is unknown until the next test merge.
    c.push("refs/heads/main", oid(5));
    assert_eq!(ask_pr(&c), (json!("UNKNOWN"), json!("UNKNOWN")));
    merged(&mut c, oid(5), oid(2), MergeabilityResult::Clean { merge: oid(8) });
    assert_eq!(ask_pr(&c).0, json!("MERGEABLE"));
    // A head push makes it stale again.
    c.push("refs/heads/f", oid(6));
    assert_eq!(ask_pr(&c).0, json!("UNKNOWN"));
}

/// Journey 3 (`design/DESIGN.md`, "Commit and read paths"): `gh label create bug` is a `DefineLabel` whose 201 is the
/// label the leader holds after the entry, a taken name answers gh's `already_exists`, and an issue opened with the
/// label is in `GET /repos/o/r/issues?labels=bug&sort=updated` from the next read, newest update first.
#[test]
fn journey_3_label_then_issue_then_both_lists() {
    use graftyard_api::rest::{label_created, label_refused, list_issues, plan_create_label};
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let op = plan_create_label(&json!({ "name": "triage", "color": "c2e0c6", "description": "" })).unwrap();
    let out = c.commit(WRITER, op).unwrap();
    let r = c.repo(0, &h, &dir);
    let created = label_created(&ack(out), &r);
    assert_eq!(created.status, 201);
    assert_eq!(created.value()["name"], "triage");
    assert_eq!(created.value()["color"], "c2e0c6");
    keep_rest_as("label-created", "POST", "/repos/{owner}/{repo}/labels", &created);
    drop(r);
    let again = c.commit(WRITER, plan_create_label(&json!({ "name": "Triage", "color": "000000" })).unwrap());
    let refused = label_refused(&again.unwrap_err());
    assert_eq!((refused.status, &refused.value()["errors"][0]["code"]), (422, &json!("already_exists")));
    assert_eq!(plan_create_label(&json!({ "name": "x", "color": "zz" })).unwrap_err().status, 422);

    // The `bug` label from `seeded` on a new issue, committed after #1 (labelled `bug` too).
    let bug = {
        let r = c.repo(0, &h, &dir);
        r.labels().into_iter().find(|(_, l)| &*l.name.0 == "bug").unwrap().0
    };
    let n = match c
        .commit(
            READER,
            IssueOp::OpenIssue {
                title: "labelled".into(),
                body: "".into(),
                labels: smallvec![graftyard_git::issues::LabelId(bug)],
                assignees: smallvec![],
            },
        )
        .unwrap()
    {
        IssueOut::Number(n) => n,
        o => panic!("{o:?}"),
    };
    let r = c.repo(0, &h, &dir);
    let page = list_issues(&r, "sort=updated&labels=bug");
    assert_eq!(page.status, 200);
    let nums: Vec<i64> = page.value().as_array().unwrap().iter().map(|i| i["number"].as_i64().unwrap()).collect();
    assert_eq!(nums, [n.0 as i64, 1], "both `bug` issues, newest update first; no unlabelled or closed ones");
    assert_eq!(page.value()[0]["labels"][0]["name"], "bug");
    keep_rest("issues-bug-updated", "/repos/{owner}/{repo}/issues", &page);
    // Every open issue and pull request without the filter; a closed one only with `state=closed`.
    let all = list_issues(&r, "");
    let nums: Vec<i64> = all.value().as_array().unwrap().iter().map(|i| i["number"].as_i64().unwrap()).collect();
    assert_eq!(nums, [n.0 as i64, 2, 1]);
    assert!(all.value()[1].get("pull_request").is_some(), "a pull request carries `pull_request`");
    keep_rest("issues-all", "/repos/{owner}/{repo}/issues", &all);
    let closed = list_issues(&r, "state=closed");
    assert_eq!(closed.value().as_array().unwrap().len(), 1);
    assert_eq!(list_issues(&r, "sort=nope").status, 422);
    let paged = list_issues(&r, "per_page=2");
    assert_eq!(paged.value().as_array().unwrap().len(), 2);
    assert!(next(&paged).unwrap().contains("page=2"));
}

/// Journey 2's review: `gh pr review --approve` sends `addPullRequestReview` with `event: APPROVE`, a `Review` at the
/// pull request's head whose payload names the review the leader holds after the entry. The author's own approval
/// is refused, and a review with no event (a pending one) is not planned.
#[test]
fn add_pull_request_review_commits_a_review_at_the_head() {
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(OWNER));
    let q = "mutation PullRequestReviewAdd($input:AddPullRequestReviewInput!){addPullRequestReview(input: $input){clientMutationId pullRequestReview{state body author{login}}}}";
    let vars = |event: Option<&str>, r: &Repo<'_>| {
        let pr = r.item(IssueNumber(2)).unwrap().row.id.0;
        let mut input =
            json!({ "pullRequestId": NodeId::scoped(NodeType::PullRequest, REPO, pr).encode(), "body": "lgtm" });
        if let Some(e) = event {
            input["event"] = json!(e);
        }
        json!({ "input": input }).as_object().cloned()
    };
    let (planned, v) = {
        let r = c.repo(0, &h, &dir);
        let v = vars(Some("APPROVE"), &r);
        let req = Request::parse(q, v.clone(), None).unwrap();
        (plan(&req, &r).unwrap(), v)
    };
    let Plan::Issue(op) = planned.plan.clone() else { panic!() };
    assert!(matches!(&*op, IssueOp::Review { n, commit, .. } if *n == IssueNumber(2) && *commit == oid(2)), "{op:?}");
    let out = c.commit(OWNER, *op.clone()).unwrap();
    let r = c.repo(0, &h, &dir);
    let req = Request::parse(q, v.clone(), None).unwrap();
    let reply = render(&req, &planned, &ack(out), &r);
    assert!(reply.get("errors").is_none(), "{reply}");
    assert_eq!(reply["data"]["addPullRequestReview"]["pullRequestReview"]["state"], "APPROVED");
    assert_eq!(reply["data"]["addPullRequestReview"]["pullRequestReview"]["body"], "lgtm");
    let d = Doc { name: "PullRequestReviewAdd-1".into(), query: q.into(), variables: v };
    keep("PullRequestReviewAdd-1", &d, &d.variables, &reply);
    drop(r);
    assert_eq!(c.commit(WRITER, *op).unwrap_err(), IssueErr::OwnApproval, "#2's author cannot approve it");
    let r = c.repo(0, &h, &dir);
    let req = Request::parse(q, vars(None, &r), None).unwrap();
    assert_eq!(plan(&req, &r).unwrap_err()["errors"][0]["type"], "UNPROCESSABLE");
}

/// F50: the edge reads the logins of exactly the accounts a repository's tiers name, from their `AccountLogin` rows,
/// and every reply that names one renders its login, not `ghost`.
#[test]
fn hosted_dir_names_every_account_the_tiers_name() {
    use graftyard_api::hosted::{HostedDir, accounts};
    let c = seeded();
    let m = &c.members[0];
    let named = accounts(&m.core, &m.meta);
    // The owner only changed settings and closed an issue, which no row names; its login comes from the path.
    let want: std::collections::BTreeSet<AccountId> =
        [WRITER, READER].into_iter().map(|a| AccountId(DbId(a))).collect();
    assert_eq!(named, want, "authors, assignees, commenters, and status creators");
    let h = host();
    let dir = Dir::new(WRITER);
    // With logins only for the owner, as the edge had before F50, the other accounts render as `ghost`.
    let only_owner =
        HostedDir { home: Some(dir.home()), logins: [(OWNER, "o".to_owned())].into(), ..Default::default() };
    let d = doc("IssueByNumber-2");
    let render = |hd: &HostedDir| {
        let r = Repo { host: &h, at: dir.home(), core: &m.core, meta: &m.meta, dir: hd };
        ask(&r, &d, d.variables.clone())["data"]["repository"]["issue"].clone()
    };
    let i = render(&only_owner);
    assert_eq!(i["comments"]["nodes"][0]["author"]["login"], "ghost");
    // With each named account's login, as `account_logins` reads them, none does, and the reply is the in-process one.
    let logins =
        [(OWNER, "o"), (WRITER, "alice"), (READER, "bob")].into_iter().map(|(a, l)| (a, l.to_owned())).collect();
    let full = HostedDir { home: Some(dir.home()), logins, ..Default::default() };
    let i = render(&full);
    assert_eq!(i["comments"]["nodes"][0]["author"]["login"], "bob");
    assert_eq!(i["assignees"]["nodes"][0]["login"], "alice");
    assert!(!i.to_string().contains("\"ghost\""), "{i}");
}
